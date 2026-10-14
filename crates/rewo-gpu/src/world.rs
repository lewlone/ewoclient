//! M5 GPU-driven world renderer.
//!
//! All chunk geometry lives in two device-local **mega-buffers** (vertices +
//! indices), each column a free-list-suballocated region. A per-column
//! **metadata SSBO** (world-space AABB + draw params) feeds a **compute cull
//! pass**: one invocation per column frustum-tests its AABB and, if visible,
//! appends a `VkDrawIndexedIndirectCommand` and bumps an atomic count. The
//! CPU then issues a single **`vkCmdDrawIndexedIndirectCount`** — the draw
//! cost stops scaling with column count and culling runs on the GPU.
//!
//! Vertices are world-space (the mesher emits `cx*16+lx+corner`), so no
//! per-column origin is needed — exactly what one shared indirect draw wants.
//!
//! Frame split: `cull()` runs the compute + count reset BEFORE dynamic
//! rendering begins (compute can't run inside a render pass); `draw()` binds
//! the mega-buffers and issues the indirect draw INSIDE the pass.
//!
//! Column uploads (rare — only on chunk load / block edit) go through a
//! one-shot staging copy with its own fence, so the per-FRAME path never
//! stalls (removing M4's per-frame `wait_idle`). The dedicated async
//! transfer queue + staging ring is the M5 follow-on.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::EntityPass;
use crate::hud::HudPass;
use crate::text::TextPass;
use crate::Gpu;

mod arena;
mod cull;
mod entity_passes;
mod frame;
mod gamma;
mod gui_passes;
mod lightmap;
mod lines;
mod scene_passes;
mod sky;
mod terrain;
mod textures;

use arena::Arena;
pub use arena::{MAX_COLUMNS, MAX_INDICES, MAX_VERTS, VERTEX_STRIDE};
use cull::Culler;
pub use entity_passes::PlayerTexture;
pub use gamma::{unorm_of, GammaScope};
use gui_passes::GuiFrame;
pub use gui_passes::OwnedTextLine;
use lightmap::LightmapRing;
pub use lightmap::{WorldLightmapState, BLOCK_LIGHT_FACTOR};
use lines::{LeashRibbon, SelectOutline};
use sky::{parse_fog_env, SkyGradient};
pub use sky::{SkyMode, SKY_HORIZON};
use terrain::TerrainPass;
pub(crate) use textures::upload_texture_array;
pub use textures::LayerAnimation;
use textures::TextureArray;

pub const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;
pub struct WorldRenderer {
    // -- graphics --
    /// The terrain pipelines (opaque + water) and their descriptor sets.
    terrain: TerrainPass,
    /// Fullscreen gradient sky (no descriptor set, no vertex input).
    sky_gradient: SkyGradient,
    /// Block-selection outline (LINE_LIST, depth-tested against terrain).
    select_outline: SelectOutline,
    /// Leash ribbons (M170).
    leash_ribbon: LeashRibbon,
    /// The `LightmapExtra` uniform buffers (one per ring slot, host-mapped)
    /// and the cursor naming the slot this frame writes and binds.
    lm: LightmapRing,
    /// The block texture array, its sampler and the animated layers.
    textures: TextureArray,
    /// The terrain arena: mega-buffers, free-lists, registered columns and the
    /// async upload ring (see `arena.rs`).
    arena: Arena,
    /// Players whose skin / cape / face slot an upload took since the last
    /// [`WorldRenderer::take_evicted_textures`].
    evicted_textures: Vec<(u128, PlayerTexture)>,
    /// GPU culling: the compute pipeline, its buffer rings and the column
    /// metadata they are filled from (see `cull.rs`).
    culler: Culler,
    // -- entities + HUD (optional passes; drawn after terrain in `draw`) --
    color_format: vk::Format,
    /// This frame's attachment views for the glint's gamma-space scope (M50).
    gamma: Option<GammaScope>,
    entities: Option<EntityPass>,
    /// Sun/moon/star pass (M12), drawn between the gradient sky and terrain.
    /// `None` when celestial textures were unavailable (degrade to bare sky).
    celestial: Option<crate::celestial::CelestialPass>,
    /// The End skybox pass (M16). `None` until `init_end_sky` supplies
    /// `textures/environment/end_sky.png` — a missing asset degrades to no sky
    /// contribution at all, never to an invented flat colour.
    end_sky: Option<crate::end_sky::EndSkyPass>,
    /// The end portal / gateway pass (M32). `None` until `init_end_portal`
    /// supplies both textures — a missing asset degrades to no portal drawn,
    /// never to an invented colour.
    end_portal: Option<crate::end_portal::EndPortalPass>,
    /// The cloud deck (M33). `None` until `init_clouds`; a dimension whose
    /// `cloud_color` alpha is zero simply never gets a draw.
    clouds: Option<crate::clouds::CloudPass>,
    /// Rain and snow (M33). `None` until `init_weather` supplies both textures.
    weather: Option<crate::weather::WeatherPass>,
    /// The world-border wall (M80). `None` until `init_border` supplies
    /// `misc/forcefield.png`; `Some` with no `set_border` draws nothing, which
    /// is what a session inside a 60-million-block default border wants.
    border: Option<crate::border::BorderPass>,
    /// Item icons in hotbar slots (M34). `None` until `init_gui_items`.
    gui_items: Option<crate::gui_item::GuiItemPass>,
    /// Rebuild counters for the two ringed passes the app repacks mid-session,
    /// so `live --render-check` can tell a legitimate ring reset from a
    /// use-after-free. See [`Self::gui_item_generation`].
    gui_item_generation: u64,
    hand_generation: u64,
    /// The inventory screen (M35). `None` until `init_container`; `Some` but
    /// closed draws nothing.
    container: Option<crate::container::ContainerPass>,
    /// The screen framework's chrome pass (M82) — a screen's backdrop gradient
    /// and its buttons. Distinct from `container`, which is the *inventory's*
    /// panel and slot highlights and is inventory-specific throughout.
    screen: Option<crate::screen::ScreenPass>,
    /// This frame's GUI inputs — what the `set_*` calls stored for the
    /// GUI passes to draw at `render` (the passes themselves stay above).
    gui: GuiFrame,
    /// This frame's scene inputs — camera, lightmap, sky and fog, celestial
    /// and End state, the selection — what the `set_*` calls stored for the
    /// world passes to draw at `render`.
    scene: SceneFrame,
    /// A **second** entity pass, for the inventory screen's player preview
    /// (M36).
    ///
    /// Its own instance rather than a mode on the world's, because the two
    /// disagree about everything the pass holds: the world's buffer carries
    /// every visible entity and is drawn once through one matrix, and the
    /// preview carries exactly one model drawn through a different one. Two
    /// `set_draws` calls into a single ring would leave the first draw reading
    /// the second's vertices.
    ///
    /// Built on first use, so a session that never opens the inventory never
    /// pays for the second atlas.
    preview: Option<crate::entities::EntityPass>,
    /// The first-person hand (M38). `None` until `init_hand`.
    hand: Option<crate::hand_pass::HandPass>,
    /// Particles (M37). `None` until `init_particles` supplies the atlas.
    particles: Option<crate::particles::ParticlePass>,
    /// The block-break crack overlay (M81). `None` until `init_crumbling`
    /// supplies the ten stage textures.
    crumbling: Option<crate::crumbling::CrumblingPass>,
    hud: Option<HudPass>,
    locator: Option<crate::locator_bar::LocatorBarPass>,
    text: Option<TextPass>,
    /// The Velvet type stack (M52b) — real variable-font text, for the pieces
    /// that need per-run styling the bitmap pass cannot express (tooltips
    /// first; chat and F3 next).
    velvet_text: Option<crate::velvet_text::VelvetTextPass>,
    pub drawn_last_frame: usize,
    pub culled_last_frame: usize,
}

/// One frame's scene inputs, separate from the renderer's GPU resources:
/// what the world passes (sky, terrain, entities, celestial, End, selection)
/// read when the frame is recorded. Each field is written by a
/// `WorldRenderer::set_*` call. The counterpart of [`GuiFrame`].
struct SceneFrame {
    /// Eye position for translucent sort + fog origin (`set_camera`).
    camera_eye: [f32; 3],
    /// The resolved lightmap uniforms (sky/block factors, sky colour, gamma,
    /// darkness, night vision). Set by `set_lightmap` / `set_lightmap_state`.
    lightmap: WorldLightmapState,
    sky_tint: [f32; 3],
    fog_tint: [f32; 3],
    /// Biome sky/fog base color, LINEAR (M14). Defaults to the `SKY_HORIZON`
    /// constant so the sky/demo/skyshot render exactly as before until the live
    /// app supplies the camera biome color. The horizon = `sky_base`, the zenith
    /// = `sky_base * ZENITH_RATIO`, both then multiplied by the day/night
    /// `sky_tint`; fog = `fog_base * fog_tint`.
    sky_base: [f32; 3],
    fog_base: [f32; 3],
    /// Distance-fog band [start, end] (env `REWO_FOG=start,end`).
    fog: [f32; 2],
    /// Which sky `draw` renders (`DimensionType.Skybox`). Default
    /// [`SkyMode::Overworld`] — the pre-M16 behaviour.
    sky_mode: SkyMode,
    /// Current clear-weather celestial state (set by `set_celestial`).
    celestial_state: crate::celestial::CelestialState,
    /// `(intensity, xAngle, yAngle)` for the End flash, or `None` where the
    /// level has no `EndFlashState` at all — which is every dimension whose
    /// skybox is not END.
    end_flash: Option<(f32, f32, f32)>,
    /// The game time the end portal shader scrolls on (`set_end_portals`).
    end_portal_time: f32,
    /// The targeted block (world coords), or `None` — set each frame.
    selection: Option<[i32; 3]>,
}

impl WorldRenderer {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        tex_size: u32,
        layers: &[Vec<u8>],
    ) -> Result<Self, String> {
        let textures = TextureArray::create(gpu, tex_size, layers)?;
        let device = gpu.device.clone();

        let terrain_descriptors = terrain::create_terrain_descriptors(&device)?;

        let mut lm = LightmapRing::create(gpu)?;

        let terrain =
            TerrainPass::create(&device, color_format, terrain_descriptors, &textures, &lm)?;
        // Seed every slot with the default state so a first frame that
        // never reaches `draw` still binds initialized memory.
        lm.seed_defaults();

        let sky_gradient = SkyGradient::create(&device, color_format)?;

        // ---- selection outline (lines) + leash ribbon (M170) ----
        let (select_outline, leash_ribbon) = lines::create_line_overlays(gpu, color_format)?;

        // ---- compute cull descriptor + pipeline ----
        let cull_pipeline = cull::create_cull_pipeline(&device)?;

        // ---- mega-buffers + metadata + indirect + count ----
        let mega = arena::create_mega_buffers(gpu)?;
        let cull_bufs = cull::create_cull_buffers(gpu)?;

        // Cull descriptor set (meta, indirect cmds, count).
        let cull_descriptors = cull::create_cull_descriptors(&device, &cull_pipeline, &cull_bufs)?;

        // ---- async upload ring ----
        let upload_ring = arena::create_upload_ring(gpu)?;
        let (count_readback, count_readback_alloc) = create_host_buffer(
            gpu,
            16,
            vk::BufferUsageFlags::TRANSFER_DST,
            "count-readback",
        )?;

        Ok(Self {
            velvet_text: None,
            gui: GuiFrame::default(),
            scene: SceneFrame {
                camera_eye: [0.0; 3],
                lightmap: WorldLightmapState::default(),
                sky_tint: [1.0; 3],
                fog_tint: [1.0; 3],
                sky_base: SKY_HORIZON,
                fog_base: SKY_HORIZON,
                fog: parse_fog_env(),
                sky_mode: SkyMode::default(),
                celestial_state: crate::celestial::CelestialState::default(),
                end_flash: None,
                end_portal_time: 0.0,
                selection: None,
            },
            terrain,
            sky_gradient,
            select_outline,
            leash_ribbon,
            lm,
            textures,
            arena: Arena::new(mega, upload_ring),
            evicted_textures: Vec::new(),
            culler: Culler::new(
                cull_pipeline,
                cull_bufs,
                cull_descriptors,
                (count_readback, count_readback_alloc),
            ),
            color_format,
            gamma: None,
            entities: None,
            celestial: None,
            end_sky: None,
            end_portal: None,
            clouds: None,
            border: None,
            weather: None,
            gui_items: None,
            gui_item_generation: 0,
            hand_generation: 0,
            container: None,
            screen: None,
            preview: None,
            hand: None,
            particles: None,
            crumbling: None,
            hud: None,
            locator: None,
            text: None,
            drawn_last_frame: 0,
            culled_last_frame: 0,
        })
    }

    /// Set the frame's eye position — drives translucent back-to-front
    /// column sorting. Callers that never render water can skip it.
    pub fn set_camera(&mut self, eye: [f32; 3]) {
        self.scene.camera_eye = eye;
    }
    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        if let Some(mut pass) = self.celestial.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.end_sky.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.clouds.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.border.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.hand.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.preview.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.locator.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.container.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.screen.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.gui_items.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.particles.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.crumbling.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.weather.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.end_portal.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.entities.take() {
            pass.destroy(gpu);
        }
        if let Some(mut hud) = self.hud.take() {
            hud.destroy(gpu);
        }
        if let Some(mut vt) = self.velvet_text.take() {
            vt.destroy(gpu);
        }
        if let Some(mut text) = self.text.take() {
            text.destroy(gpu);
        }
        self.select_outline.destroy_ring(gpu);
        self.leash_ribbon.destroy_ring(gpu);
        unsafe {
            let device = &gpu.device;
            for s in &mut self.arena.upload_slots {
                device.destroy_fence(s.fence, None);
                device.destroy_buffer(s.staging, None);
            }
            device.destroy_command_pool(self.arena.upload_pool, None);
            self.culler.destroy_objects(device);
            self.terrain.destroy_pipelines(device);
            self.sky_gradient.destroy(device);
            self.select_outline.destroy_objects(device);
            self.leash_ribbon.destroy_objects(device);
            for b in self.lm.bufs {
                device.destroy_buffer(b, None);
            }
            self.terrain.destroy_layouts(device);
            self.textures.destroy_objects(device);
            for b in [self.arena.vbuf, self.arena.ibuf, self.culler.readback] {
                device.destroy_buffer(b, None);
            }
        }
        let mut slot_allocs: Vec<Option<Allocation>> = self
            .arena
            .upload_slots
            .iter_mut()
            .map(|s| s.staging_alloc.take())
            .collect();
        for a in self.lm.allocs.iter_mut() {
            slot_allocs.push(a.take());
        }
        for a in [
            self.textures.image_alloc.take(),
            self.arena.vbuf_alloc.take(),
            self.arena.ibuf_alloc.take(),
            self.culler.readback_alloc.take(),
        ]
        .into_iter()
        .chain(slot_allocs)
        .chain(self.culler.meta_allocs.iter_mut().map(|a| a.take()))
        .chain(self.culler.indirect_allocs.iter_mut().map(|a| a.take()))
        .chain(self.culler.count_allocs.iter_mut().map(|a| a.take()))
        .flatten()
        {
            let _ = gpu.allocator.free(a);
        }
    }
}

// -- helpers -----------------------------------------------------------------

fn create_device_buffer(
    gpu: &mut Gpu,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> Result<(vk::Buffer, Allocation), String> {
    unsafe {
        let buffer = gpu
            .device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("{name}: {e}"))?;
        let req = gpu.device.get_buffer_memory_requirements(buffer);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name,
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("{name} alloc: {e}"))?;
        gpu.device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
            .map_err(|e| format!("{name} bind: {e}"))?;
        Ok((buffer, alloc))
    }
}

fn create_host_buffer(
    gpu: &mut Gpu,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> Result<(vk::Buffer, Allocation), String> {
    unsafe {
        let buffer = gpu
            .device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("{name}: {e}"))?;
        let req = gpu.device.get_buffer_memory_requirements(buffer);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name,
                requirements: req,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("{name} alloc: {e}"))?;
        gpu.device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
            .map_err(|e| format!("{name} bind: {e}"))?;
        Ok((buffer, alloc))
    }
}

// -- texture upload (unchanged from M2/M4) -----------------------------------

// -- depth target (unchanged) ------------------------------------------------

pub struct DepthTarget {
    pub image: vk::Image,
    alloc: Option<Allocation>,
    pub view: vk::ImageView,
    pub extent: vk::Extent2D,
}

impl DepthTarget {
    pub fn new(gpu: &mut Gpu, extent: vk::Extent2D) -> Result<Self, String> {
        unsafe {
            let device = gpu.device.clone();
            let image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(DEPTH_FORMAT)
                        .extent(vk::Extent3D {
                            width: extent.width.max(1),
                            height: extent.height.max(1),
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(|e| format!("depth image: {e}"))?;
            let req = device.get_image_memory_requirements(image);
            let alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "depth-target",
                    requirements: req,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("depth alloc: {e}"))?;
            device
                .bind_image_memory(image, alloc.memory(), alloc.offset())
                .map_err(|e| format!("depth bind: {e}"))?;
            let view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(DEPTH_FORMAT)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::DEPTH)
                                .base_mip_level(0)
                                .level_count(1)
                                .base_array_layer(0)
                                .layer_count(1),
                        ),
                    None,
                )
                .map_err(|e| format!("depth view: {e}"))?;
            Ok(Self {
                image,
                alloc: Some(alloc),
                view,
                extent,
            })
        }
    }

    pub fn barrier_for_use(&self, gpu: &Gpu, cb: vk::CommandBuffer) {
        unsafe {
            // One depth image serves every frame in flight, so the previous
            // frame's depth writes must finish before this frame's clear
            // (a WAW the old TOP_OF_PIPE/no-access source left unordered).
            let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(
                    vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                )
                .src_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                )
                .dst_access_mask(
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                        | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                )
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(self.image)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::DEPTH)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                );
            gpu.device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&barrier)),
            );
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            gpu.device.destroy_image_view(self.view, None);
            gpu.device.destroy_image(self.image, None);
        }
        if let Some(a) = self.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

// -- frustum -----------------------------------------------------------------

/// Infinite reversed-Z perspective (RH, Y-up). Maps z=-near→1, z=-∞→0; pair
/// with a 0.0 depth clear + `GREATER`. Column-major.
pub fn perspective_reverse_z(fov_y_rad: f32, aspect: f32, near: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov_y_rad * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, 0.0, -1.0],
        [0.0, 0.0, near, 0.0],
    ]
}

/// Clear the depth attachment over one rectangle, mid-render-pass.
///
/// The preview shares the frame's depth buffer with the world, so without this
/// the model would be depth-tested against whatever terrain happens to be
/// behind the panel and would appear cut in half by a hillside. Reversed-Z, so
/// the cleared value is **0.0** — the far plane — which is what the world pass
/// clears to as well.
fn clear_depth_rect(gpu: &Gpu, cb: vk::CommandBuffer, rect: vk::Rect2D) {
    let attachment = vk::ClearAttachment::default()
        .aspect_mask(vk::ImageAspectFlags::DEPTH)
        .clear_value(vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 0.0,
                stencil: 0,
            },
        });
    let area = vk::ClearRect::default()
        .rect(rect)
        .base_array_layer(0)
        .layer_count(1);
    unsafe { gpu.device.cmd_clear_attachments(cb, &[attachment], &[area]) };
}
