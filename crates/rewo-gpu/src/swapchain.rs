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
    ///
    /// Passing `self.handle` as `oldSwapchain` RETIRES it, and that holds even
    /// when `create_swapchain` or a later step fails (the spec: "even if
    /// creation of the new swapchain fails"). A retired swapchain must not be
    /// passed as `oldSwapchain` again (VUID-VkSwapchainCreateInfoKHR-
    /// oldSwapchain-01933), so on failure the old one is destroyed here and
    /// `self` becomes an empty 0x0 swapchain (null handle, no images):
    /// [`crate::Renderer::render`] skips it, and the next `recreate` passes a
    /// null `oldSwapchain`.
    pub fn recreate(
        &mut self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        preferred: vk::PresentModeKHR,
    ) -> Result<(), String> {
        gpu.wait_idle();
        match Self::create(gpu, width, height, preferred, self.handle) {
            Ok(next) => {
                let old = self.take_retired();
                old.destroy(gpu, &self.fns);
                *self = next;
                Ok(())
            }
            Err(e) => {
                let old = self.take_retired();
                old.destroy(gpu, &self.fns);
                Err(e)
            }
        }
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

            let format = pick_surface_format(&formats).ok_or("no surface formats")?;
            if !is_srgb_format(format.format) {
                log::warn!(
                    "vk: surface offers no sRGB format; using {:?} {:?}, so colours will \
                     be stored without sRGB encoding (too dark)",
                    format.format,
                    format.color_space
                );
            }

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
            let view_formats: Vec<vk::Format> = unorm
                .into_iter()
                .chain(std::iter::once(format.format))
                .collect();
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
            // From here on a failure must release what was created, or the
            // new swapchain (and the surface it holds) leaks.
            let mut views: Vec<vk::ImageView> = Vec::new();
            let mut views_unorm: Vec<vk::ImageView> = Vec::new();
            let release = |views: &[vk::ImageView], views_unorm: &[vk::ImageView]| {
                for &v in views.iter().chain(views_unorm) {
                    gpu.device.destroy_image_view(v, None);
                }
                fns.destroy_swapchain(handle, None);
            };
            let images = match fns.get_swapchain_images(handle) {
                Ok(i) => i,
                Err(e) => {
                    release(&views, &views_unorm);
                    return Err(format!("swapchain images: {e}"));
                }
            };
            let view_of = |img: vk::Image, f: vk::Format| {
                let vi = vk::ImageViewCreateInfo::default()
                    .image(img)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(f)
                    .subresource_range(crate::color_range());
                gpu.device.create_image_view(&vi, None)
            };
            for &img in &images {
                match view_of(img, format.format) {
                    Ok(v) => views.push(v),
                    Err(e) => {
                        release(&views, &views_unorm);
                        return Err(format!("image view: {e}"));
                    }
                }
            }
            if let (true, Some(f)) = (mutable, unorm) {
                for &img in &images {
                    match view_of(img, f) {
                        Ok(v) => views_unorm.push(v),
                        Err(e) => {
                            release(&views, &views_unorm);
                            return Err(format!("unorm image view: {e}"));
                        }
                    }
                }
            }

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

/// What `recreate` takes out of a [`Swapchain`] once its handle has been
/// passed as `oldSwapchain`: the retired handle and the views onto its images.
struct Retired {
    handle: vk::SwapchainKHR,
    views: Vec<vk::ImageView>,
}

impl Retired {
    /// The device must be idle with respect to these images.
    fn destroy(self, gpu: &Gpu, fns: &khr::swapchain::Device) {
        unsafe {
            for &v in &self.views {
                gpu.device.destroy_image_view(v, None);
            }
            if self.handle != vk::SwapchainKHR::null() {
                fns.destroy_swapchain(self.handle, None);
            }
        }
    }
}

impl Swapchain {
    /// The state transition after this swapchain's handle has been passed as
    /// `oldSwapchain`: move the handle and views out, leaving a null handle,
    /// no images and a 0x0 extent. Whatever the create call returned, the old
    /// handle is never reachable from `self` again, so it cannot be passed as
    /// `oldSwapchain` twice.
    fn take_retired(&mut self) -> Retired {
        take_retired(
            &mut self.handle,
            &mut self.images,
            &mut self.views,
            &mut self.views_unorm,
            &mut self.extent,
        )
    }
}

/// [`Swapchain::take_retired`] on the bare fields (a `Swapchain` cannot be
/// built without a device, and this is the part worth testing).
fn take_retired(
    handle: &mut vk::SwapchainKHR,
    images: &mut Vec<vk::Image>,
    views: &mut Vec<vk::ImageView>,
    views_unorm: &mut Vec<vk::ImageView>,
    extent: &mut vk::Extent2D,
) -> Retired {
    let mut all_views = std::mem::take(views);
    all_views.append(views_unorm);
    images.clear();
    *extent = vk::Extent2D {
        width: 0,
        height: 0,
    };
    Retired {
        handle: std::mem::replace(handle, vk::SwapchainKHR::null()),
        views: all_views,
    }
}

/// The `*_SRGB` formats a swapchain is likely to offer: the ones whose
/// attachment stores encode linear shader output, which every pass assumes.
fn is_srgb_format(f: vk::Format) -> bool {
    matches!(
        f,
        vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB | vk::Format::A8B8G8R8_SRGB_PACK32
    )
}

/// The surface format to render to: `B8G8R8A8_SRGB` / sRGB-nonlinear when
/// offered, else any sRGB-encoded format in that colour space, else the first
/// format (which the caller warns about: shaders write linear colour, so a
/// UNORM target renders too dark).
fn pick_surface_format(formats: &[vk::SurfaceFormatKHR]) -> Option<vk::SurfaceFormatKHR> {
    let nonlinear = |f: &&vk::SurfaceFormatKHR| f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR;
    formats
        .iter()
        .filter(nonlinear)
        .find(|f| f.format == vk::Format::B8G8R8A8_SRGB)
        .or_else(|| {
            formats
                .iter()
                .filter(nonlinear)
                .find(|f| is_srgb_format(f.format))
        })
        .or_else(|| formats.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sf(format: vk::Format) -> vk::SurfaceFormatKHR {
        vk::SurfaceFormatKHR {
            format,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        }
    }

    #[test]
    fn surface_format_prefers_bgra_srgb_then_any_srgb_then_first() {
        let bgra = sf(vk::Format::B8G8R8A8_SRGB);
        let rgba = sf(vk::Format::R8G8B8A8_SRGB);
        let unorm = sf(vk::Format::B8G8R8A8_UNORM);
        assert_eq!(pick_surface_format(&[unorm, rgba, bgra]), Some(bgra));
        // The old fallback took `formats.first()`, the UNORM one, here.
        assert_eq!(pick_surface_format(&[unorm, rgba]), Some(rgba));
        assert_eq!(pick_surface_format(&[unorm]), Some(unorm));
        assert_eq!(pick_surface_format(&[]), None);
        // An sRGB format in another colour space is not preferred over one in
        // sRGB-nonlinear.
        let hdr = vk::SurfaceFormatKHR {
            format: vk::Format::B8G8R8A8_SRGB,
            color_space: vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
        };
        assert_eq!(pick_surface_format(&[hdr, rgba]), Some(rgba));
    }

    #[test]
    fn retiring_leaves_no_reusable_old_handle() {
        use ash::vk::Handle;
        let mut handle = vk::SwapchainKHR::from_raw(0xABCD);
        let mut images = vec![vk::Image::from_raw(1), vk::Image::from_raw(2)];
        let mut views = vec![vk::ImageView::from_raw(10), vk::ImageView::from_raw(11)];
        let mut unorm = vec![vk::ImageView::from_raw(20), vk::ImageView::from_raw(21)];
        let mut extent = vk::Extent2D {
            width: 800,
            height: 600,
        };
        let r = take_retired(
            &mut handle,
            &mut images,
            &mut views,
            &mut unorm,
            &mut extent,
        );
        // The retired handle and every view went to the destroyer ...
        assert_eq!(r.handle.as_raw(), 0xABCD);
        assert_eq!(r.views.len(), 4);
        // ... and nothing left behind can be passed as `oldSwapchain` again
        // (VUID-VkSwapchainCreateInfoKHR-oldSwapchain-01933): the next create
        // gets a null old handle, and `render` skips the 0x0 extent.
        assert_eq!(handle, vk::SwapchainKHR::null());
        assert!(images.is_empty() && views.is_empty() && unorm.is_empty());
        assert_eq!((extent.width, extent.height), (0, 0));
        // A second retire (a failed retry) hands out nothing to destroy twice.
        let r2 = take_retired(
            &mut handle,
            &mut images,
            &mut views,
            &mut unorm,
            &mut extent,
        );
        assert_eq!(r2.handle, vk::SwapchainKHR::null());
        assert!(r2.views.is_empty());
    }
}
