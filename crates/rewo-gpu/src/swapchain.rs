//! Swapchain create/recreate. MAILBOX-preferring (REWO_PLAN.md §8): the
//! preference order is [user pref, MAILBOX, IMMEDIATE, FIFO] — FIFO is the
//! only spec-guaranteed mode, so it stays as the fallback of last resort.

use ash::{khr, vk};

use crate::Gpu;

pub struct Swapchain {
    pub fns: khr::swapchain::Device,
    pub handle: vk::SwapchainKHR,
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    /// Gamma-space counterparts of [`Self::views`] (M50) — a UNORM view of each
    /// sRGB swapchain image, where the enchantment glint blends. Empty when
    /// `VK_KHR_swapchain_mutable_format` is unavailable, in which case no glint
    /// is drawn rather than one blended in the wrong space.
    pub views_unorm: Vec<vk::ImageView>,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub present_mode: vk::PresentModeKHR,
}

impl Swapchain {
    pub fn new(
        gpu: &Gpu,
        width: u32,
        height: u32,
        preferred: vk::PresentModeKHR,
    ) -> Result<Self, String> {
        Self::create(gpu, width, height, preferred, vk::SwapchainKHR::null())
    }

    /// Recreate in place (resize / OUT_OF_DATE). Caller ensures the GPU is
    /// idle enough (we `wait_idle` here for M0 simplicity).
    pub fn recreate(
        &mut self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        preferred: vk::PresentModeKHR,
    ) -> Result<(), String> {
        gpu.wait_idle();
        let next = Self::create(gpu, width, height, preferred, self.handle)?;
        unsafe {
            for &v in self.views.iter().chain(&self.views_unorm) {
                gpu.device.destroy_image_view(v, None);
            }
            self.fns.destroy_swapchain(self.handle, None);
        }
        *self = next;
        Ok(())
    }

    fn create(
        gpu: &Gpu,
        width: u32,
        height: u32,
        preferred: vk::PresentModeKHR,
        old: vk::SwapchainKHR,
    ) -> Result<Self, String> {
        let si = gpu
            .surface_i
            .as_ref()
            .ok_or("swapchain requires a windowed Gpu")?;
        unsafe {
            let caps = si
                .get_physical_device_surface_capabilities(gpu.physical, gpu.surface)
                .map_err(|e| format!("surface caps: {e}"))?;
            let formats = si
                .get_physical_device_surface_formats(gpu.physical, gpu.surface)
                .map_err(|e| format!("surface formats: {e}"))?;
            let modes = si
                .get_physical_device_surface_present_modes(gpu.physical, gpu.surface)
                .map_err(|e| format!("present modes: {e}"))?;

            let format = formats
                .iter()
                .find(|f| {
                    f.format == vk::Format::B8G8R8A8_SRGB
                        && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
                })
                .or_else(|| formats.first())
                .copied()
                .ok_or("no surface formats")?;

            let present_mode = [
                preferred,
                vk::PresentModeKHR::MAILBOX,
                vk::PresentModeKHR::IMMEDIATE,
                vk::PresentModeKHR::FIFO,
            ]
            .into_iter()
            .find(|m| modes.contains(m))
            .unwrap_or(vk::PresentModeKHR::FIFO);

            let extent = if caps.current_extent.width != u32::MAX {
                caps.current_extent
            } else {
                vk::Extent2D {
                    width: width.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                    height: height
                        .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
                }
            };

            // MAILBOX wants at least a triple; clamp to the surface max.
            let mut image_count = (caps.min_image_count + 1).max(3);
            if caps.max_image_count > 0 {
                image_count = image_count.min(caps.max_image_count);
            }

            // M50: a UNORM twin of each image, for the glint's gamma-space
            // blend. Needs the swapchain itself to be mutable-format, which is
            // an extension rather than core.
            let unorm = crate::world::unorm_of(format.format).filter(|f| *f != format.format);
            let mutable = gpu.swapchain_mutable && unorm.is_some();
            let view_formats: Vec<vk::Format> =
                unorm.into_iter().chain(std::iter::once(format.format)).collect();
            let mut format_list =
                vk::ImageFormatListCreateInfo::default().view_formats(&view_formats);
            let mut ci = vk::SwapchainCreateInfoKHR::default()
                .surface(gpu.surface)
                .min_image_count(image_count)
                .image_format(format.format)
                .image_color_space(format.color_space)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
                .present_mode(present_mode)
                .clipped(true)
                .old_swapchain(old);
            if mutable {
                ci = ci
                    .flags(vk::SwapchainCreateFlagsKHR::MUTABLE_FORMAT)
                    .push_next(&mut format_list);
            }

            let fns = khr::swapchain::Device::new(&gpu.instance, &gpu.device);
            let handle = fns
                .create_swapchain(&ci, None)
                .map_err(|e| format!("create swapchain: {e}"))?;
            let images = fns
                .get_swapchain_images(handle)
                .map_err(|e| format!("swapchain images: {e}"))?;
            let views = images
                .iter()
                .map(|&img| {
                    let vi = vk::ImageViewCreateInfo::default()
                        .image(img)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format.format)
                        .subresource_range(crate::color_range());
                    gpu.device
                        .create_image_view(&vi, None)
                        .map_err(|e| format!("image view: {e}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let views_unorm = match (mutable, unorm) {
                (true, Some(f)) => images
                    .iter()
                    .map(|&img| {
                        let vi = vk::ImageViewCreateInfo::default()
                            .image(img)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(f)
                            .subresource_range(crate::color_range());
                        gpu.device
                            .create_image_view(&vi, None)
                            .map_err(|e| format!("unorm image view: {e}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => Vec::new(),
            };

            log::info!(
                "vk: swapchain {}x{} {:?} {:?} ({} images)",
                extent.width,
                extent.height,
                format.format,
                present_mode,
                images.len()
            );

            Ok(Self {
                fns,
                handle,
                images,
                views,
                views_unorm,
                format: format.format,
                extent,
                present_mode,
            })
        }
    }

    pub fn destroy(&mut self, gpu: &Gpu) {
        unsafe {
            for &v in self.views.iter().chain(&self.views_unorm) {
                gpu.device.destroy_image_view(v, None);
            }
            self.views.clear();
            self.views_unorm.clear();
            if self.handle != vk::SwapchainKHR::null() {
                self.fns.destroy_swapchain(self.handle, None);
                self.handle = vk::SwapchainKHR::null();
            }
        }
    }
}
