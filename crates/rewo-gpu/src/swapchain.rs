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
            for &v in &self.views {
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

            let ci = vk::SwapchainCreateInfoKHR::default()
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
                format: format.format,
                extent,
                present_mode,
            })
        }
    }

    pub fn destroy(&mut self, gpu: &Gpu) {
        unsafe {
            for &v in &self.views {
                gpu.device.destroy_image_view(v, None);
            }
            self.views.clear();
            if self.handle != vk::SwapchainKHR::null() {
                self.fns.destroy_swapchain(self.handle, None);
                self.handle = vk::SwapchainKHR::null();
            }
        }
    }
}
