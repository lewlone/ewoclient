//! rewo-gpu — ash/Vulkan 1.3 bootstrap for the Rewo native client.
//!
//! M0 scope (REWO_PLAN.md): instance + device + queues + allocator,
//! swapchain (MAILBOX-preferring), the frame-time overlay pipeline, GPU
//! timestamp queries, and an offscreen render target for headless
//! verification (render N frames → PNG, no window needed).
//!
//! Design notes that persist past M0:
//! - Validation layers are enabled when present + requested; all messages
//!   route through `log`. The Vulkan SDK on the dev box provides them.
//! - Everything device-lost-critical lives behind explicit `destroy(&mut
//!   Gpu)` calls rather than Drop impls, because Vulkan teardown order is
//!   load-bearing and implicit drops hide it. `Gpu` itself is the one Drop
//!   (device → surface → debug → instance, after a wait_idle).

pub mod celestial;
pub mod clouds;
pub mod container;
pub mod gui_item;
pub mod hand;
pub mod hand_pass;
pub mod particles;
pub mod weather;
pub mod end_portal;
pub mod end_sky;
pub mod entities;
pub mod anim_defs;
pub mod cem;
pub mod cem_anim;
pub mod vanilla_hier;
pub mod mobs;
pub mod held;
pub mod hud;
pub mod offscreen;
pub mod velvet_chrome;
pub mod velvet_glyph;
pub mod velvet_text;
pub mod velvet_widgets;
pub mod tab_list;
pub mod text;
pub mod tooltip;
pub mod overlay;
pub mod renderer;
pub mod swapchain;
pub mod world;

use std::ffi::{c_char, c_void, CStr};
use std::mem::ManuallyDrop;

use ash::{ext, khr, vk};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

/// CPU frames in flight. 2 for latency (REWO_PLAN.md §8); tunable to 1 in
/// the M6 latency pass.
pub const FRAMES_IN_FLIGHT: usize = 2;

/// The largest value [`renderer::Renderer::with_frames_in_flight`] will accept
/// — the M6 knob clamps into `1..=MAX_FRAMES_IN_FLIGHT`.
///
/// Any per-frame ring a `WorldRenderer` owns must be at least this long, not
/// merely [`FRAMES_IN_FLIGHT`] long: the renderer's fence wait only guarantees
/// that frame `n - fif` has retired, so a ring shorter than the *actual* `fif`
/// would let a recording frame overwrite a slot an in-flight frame still reads.
/// `WorldRenderer` has no handle on the `Renderer` that drives it, so it sizes
/// its rings for the worst case the knob permits.
pub const MAX_FRAMES_IN_FLIGHT: usize = 3;

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

pub struct Gpu {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    debug: Option<(ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub surface_i: Option<khr::surface::Instance>,
    pub surface: vk::SurfaceKHR,
    pub physical: vk::PhysicalDevice,
    pub device_name: String,
    /// Nanoseconds per timestamp tick (`limits.timestamp_period`).
    pub timestamp_period_ns: f64,
    pub device: ash::Device,
    pub graphics_family: u32,
    pub graphics_queue: vk::Queue,
    pub allocator: ManuallyDrop<Allocator>,
    pub validation_active: bool,
    /// `VK_KHR_swapchain_mutable_format` (M50) — whether a swapchain can hand
    /// out a **UNORM** view of its own sRGB images.
    ///
    /// The enchantment glint's blend is `(SRC_COLOR, ONE)`, i.e. `dst + src²`,
    /// and vanilla evaluates it in gamma space because Minecraft binds no sRGB
    /// framebuffer. Squaring is not invariant under the transfer function, so
    /// blending it into an sRGB attachment loses almost all of the sheen — a
    /// worn armour foil measured a byte-delta of exactly 0. Rendering the glint
    /// through a UNORM view of the same image puts the blend back in vanilla's
    /// space. Without the extension there is no such view, and the glint is
    /// dropped rather than drawn wrong — the same degradation as a missing
    /// sheet.
    pub swapchain_mutable: bool,
}

impl Gpu {
    /// Windowed when `window` handles are given (creates a surface and the
    /// picked queue family must present to it); headless otherwise.
    pub fn new(
        window: Option<(RawDisplayHandle, RawWindowHandle)>,
        want_validation: bool,
    ) -> Result<Self, String> {
        unsafe {
            let entry = ash::Entry::load().map_err(|e| format!("load vulkan-1: {e}"))?;

            // Layers: validation only if requested AND actually installed.
            let layer_props = entry
                .enumerate_instance_layer_properties()
                .map_err(|e| format!("enumerate layers: {e}"))?;
            let validation_available = layer_props.iter().any(|p| {
                CStr::from_ptr(p.layer_name.as_ptr()) == VALIDATION_LAYER
            });
            let validation_active = want_validation && validation_available;
            if want_validation && !validation_available {
                log::warn!(
                    "vk: validation requested but VK_LAYER_KHRONOS_validation not installed \
                     (install the Vulkan SDK) — continuing without"
                );
            }
            let layers: Vec<*const c_char> = if validation_active {
                vec![VALIDATION_LAYER.as_ptr()]
            } else {
                Vec::new()
            };

            // Extensions: surface set from the window handles + debug utils.
            let mut extensions: Vec<*const c_char> = match window {
                Some((rdh, _)) => ash_window::enumerate_required_extensions(rdh)
                    .map_err(|e| format!("required surface extensions: {e}"))?
                    .to_vec(),
                None => Vec::new(),
            };
            if validation_active {
                extensions.push(ext::debug_utils::NAME.as_ptr());
            }

            let app_info = vk::ApplicationInfo::default()
                .application_name(c"Rewo")
                .application_version(vk::make_api_version(0, 0, 1, 0))
                .engine_name(c"rewo-gpu")
                .api_version(vk::API_VERSION_1_3);
            let instance_ci = vk::InstanceCreateInfo::default()
                .application_info(&app_info)
                .enabled_layer_names(&layers)
                .enabled_extension_names(&extensions);
            let instance = entry
                .create_instance(&instance_ci, None)
                .map_err(|e| format!("create instance: {e}"))?;

            let debug = if validation_active {
                let du = ext::debug_utils::Instance::new(&entry, &instance);
                let ci = vk::DebugUtilsMessengerCreateInfoEXT::default()
                    .message_severity(
                        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                            | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                    )
                    .message_type(
                        vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                            | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE
                            | vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
                    )
                    .pfn_user_callback(Some(debug_callback));
                let messenger = du
                    .create_debug_utils_messenger(&ci, None)
                    .map_err(|e| format!("create debug messenger: {e}"))?;
                Some((du, messenger))
            } else {
                None
            };

            // Surface before device pick — present support is per-family.
            let (surface_i, surface) = match window {
                Some((rdh, rwh)) => {
                    let si = khr::surface::Instance::new(&entry, &instance);
                    let s = ash_window::create_surface(&entry, &instance, rdh, rwh, None)
                        .map_err(|e| format!("create surface: {e}"))?;
                    (Some(si), s)
                }
                None => (None, vk::SurfaceKHR::null()),
            };

            // Physical device: prefer discrete, require graphics (+present).
            let physicals = instance
                .enumerate_physical_devices()
                .map_err(|e| format!("enumerate physical devices: {e}"))?;
            let mut best: Option<(vk::PhysicalDevice, u32, i32)> = None;
            for pd in physicals {
                let props = instance.get_physical_device_properties(pd);
                if props.api_version < vk::API_VERSION_1_3 {
                    continue;
                }
                let families = instance.get_physical_device_queue_family_properties(pd);
                let family = families.iter().enumerate().position(|(i, f)| {
                    let graphics = f.queue_flags.contains(vk::QueueFlags::GRAPHICS);
                    let present = match (&surface_i, surface) {
                        (Some(si), s) if s != vk::SurfaceKHR::null() => si
                            .get_physical_device_surface_support(pd, i as u32, s)
                            .unwrap_or(false),
                        _ => true,
                    };
                    graphics && present
                });
                let Some(family) = family else { continue };
                let score = match props.device_type {
                    vk::PhysicalDeviceType::DISCRETE_GPU => 1000,
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 100,
                    _ => 10,
                };
                if best.map(|(_, _, s)| score > s).unwrap_or(true) {
                    best = Some((pd, family as u32, score));
                }
            }
            let (physical, graphics_family, _) = best.ok_or_else(|| {
                "no Vulkan 1.3 device with a graphics(+present) queue family found".to_string()
            })?;

            let props = instance.get_physical_device_properties(physical);
            let device_name = CStr::from_ptr(props.device_name.as_ptr())
                .to_string_lossy()
                .into_owned();
            let timestamp_period_ns = props.limits.timestamp_period as f64;
            log::info!(
                "vk: device \"{}\" (api {}.{}.{}, driver {:#x}), graphics family {}, validation {}",
                device_name,
                vk::api_version_major(props.api_version),
                vk::api_version_minor(props.api_version),
                vk::api_version_patch(props.api_version),
                props.driver_version,
                graphics_family,
                if validation_active { "ON" } else { "off" },
            );

            // Logical device: Vulkan 1.3 core features we build on.
            let priorities = [1.0f32];
            let queue_infos = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(graphics_family)
                .queue_priorities(&priorities)];
            // M50: a UNORM view of the swapchain's sRGB images, for the glint's
            // gamma-space blend. Optional — advertised by every desktop driver
            // we target, but a machine without it loses the glint rather than
            // the client.
            let have_mutable = instance
                .enumerate_device_extension_properties(physical)
                .map(|props| {
                    props.iter().any(|p| {
                        p.extension_name_as_c_str()
                            == Ok(khr::swapchain_mutable_format::NAME)
                    })
                })
                .unwrap_or(false);
            let windowed = surface != vk::SurfaceKHR::null();
            let swapchain_mutable = windowed && have_mutable;
            let mut device_exts: Vec<*const c_char> = Vec::new();
            if windowed {
                device_exts.push(khr::swapchain::NAME.as_ptr());
                if swapchain_mutable {
                    device_exts.push(khr::swapchain_mutable_format::NAME.as_ptr());
                } else {
                    log::warn!(
                        "gpu: VK_KHR_swapchain_mutable_format absent — the enchantment \
                         glint needs a gamma-space target and will not be drawn"
                    );
                }
            }
            let mut features13 = vk::PhysicalDeviceVulkan13Features::default()
                .dynamic_rendering(true)
                .synchronization2(true);
            // M5 GPU-driven path: draw_indirect_count (core 1.2) +
            // multi_draw_indirect (core feature) so the compute cull pass can
            // emit an indirect draw list the GPU consumes in one call.
            let mut features12 =
                vk::PhysicalDeviceVulkan12Features::default().draw_indirect_count(true);
            let mut features_core = vk::PhysicalDeviceFeatures2::default();
            features_core.features.multi_draw_indirect = vk::TRUE;
            let device_ci = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_infos)
                .enabled_extension_names(&device_exts)
                .push_next(&mut features13)
                .push_next(&mut features12)
                .push_next(&mut features_core);
            let device = instance
                .create_device(physical, &device_ci, None)
                .map_err(|e| format!("create device: {e}"))?;
            let graphics_queue = device.get_device_queue(graphics_family, 0);

            let allocator = Allocator::new(&AllocatorCreateDesc {
                instance: instance.clone(),
                device: device.clone(),
                physical_device: physical,
                debug_settings: Default::default(),
                buffer_device_address: false,
                allocation_sizes: Default::default(),
            })
            .map_err(|e| format!("create allocator: {e}"))?;

            Ok(Self {
                entry,
                instance,
                debug,
                surface_i,
                surface,
                physical,
                device_name,
                timestamp_period_ns,
                device,
                graphics_family,
                graphics_queue,
                allocator: ManuallyDrop::new(allocator),
                validation_active,
                swapchain_mutable,
            })
        }
    }

    pub fn wait_idle(&self) {
        unsafe {
            let _ = self.device.device_wait_idle();
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            ManuallyDrop::drop(&mut self.allocator);
            self.device.destroy_device(None);
            if let Some(si) = &self.surface_i {
                if self.surface != vk::SurfaceKHR::null() {
                    si.destroy_surface(self.surface, None);
                }
            }
            if let Some((du, messenger)) = &self.debug {
                du.destroy_debug_utils_messenger(*messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut c_void,
) -> vk::Bool32 {
    let message = if data.is_null() {
        std::borrow::Cow::from("<null>")
    } else {
        CStr::from_ptr((*data).p_message).to_string_lossy()
    };
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        log::error!("[vk] {message}");
    } else {
        log::warn!("[vk] {message}");
    }
    vk::FALSE
}

/// The full-image color subresource range used everywhere in M0.
pub fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

/// Record a single sync2 image layout transition.
///
/// # Safety
/// `cb` must be in the recording state; standard Vulkan external-sync rules.
#[allow(clippy::too_many_arguments)]
pub unsafe fn image_barrier(
    device: &ash::Device,
    cb: vk::CommandBuffer,
    image: vk::Image,
    src_stage: vk::PipelineStageFlags2,
    src_access: vk::AccessFlags2,
    dst_stage: vk::PipelineStageFlags2,
    dst_access: vk::AccessFlags2,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_range());
    let dep = vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&barrier));
    device.cmd_pipeline_barrier2(cb, &dep);
}
