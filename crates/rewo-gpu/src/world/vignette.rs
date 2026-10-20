//! M282/M283 — the camera overlays (`Hud.extractCameraOverlays`) and the
//! sleep fade: full-screen images (the vignette, the carved pumpkin's blur,
//! the powder-snow frost, the spyglass scope) and plain fills, each an image
//! over a screen-pixel rect times a colour. The vignette multiplies the frame
//! by one minus it (`RenderPipelines.VIGNETTE`), the rest blend by alpha.
//! Drawn in the gamma scope: the overlays after the hand and before the HUD,
//! the late list (the sleep fade) after the HUD.

use ash::vk;
use gpu_allocator::vulkan::Allocation;

use super::WorldRenderer;
use crate::Gpu;

/// M283 — the images the overlay pass holds, by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayImage {
    /// `misc/vignette.png` (always image 0).
    Vignette = 0,
    /// A 1x1 white texel, for plain fills.
    White = 1,
    /// `misc/pumpkinblur.png` — the carved pumpkin's `camera_overlay`.
    PumpkinBlur = 2,
    /// `misc/powder_snow_outline.png`.
    PowderSnow = 3,
    /// `misc/spyglass_scope.png`.
    SpyglassScope = 4,
}

/// M283 — one overlay draw: an image over `rect` (screen px, x0 y0 x1 y1),
/// times `color` (gamma rgba), multiplying (the vignette) or alpha-blending.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraDraw {
    pub image: OverlayImage,
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub multiply: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    rect: [f32; 4],
    color: [f32; 4],
}

struct Image {
    image: vk::Image,
    alloc: Option<Allocation>,
    view: vk::ImageView,
    set: vk::DescriptorSet,
}

pub struct VignettePass {
    images: Vec<Option<Image>>,
    linear: vk::Sampler,
    nearest: vk::Sampler,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    layout: vk::PipelineLayout,
    multiply: vk::Pipeline,
    alpha: vk::Pipeline,
}

const IMAGE_SLOTS: u32 = 5;

impl VignettePass {
    /// `rgba` is `misc/vignette.png` (uploaded UNORM: vanilla samples raw
    /// bytes); `format` the frame's UNORM colour format.
    pub fn new(
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
        format: vk::Format,
    ) -> Result<Self, String> {
        unsafe {
            let device = gpu.device.clone();
            let sampler = |filter: vk::Filter| {
                device.create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(filter)
                        .min_filter(filter)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .max_lod(0.0),
                    None,
                )
            };
            // `blur: true` in a texture's mcmeta is bilinear; the rest nearest.
            let linear =
                sampler(vk::Filter::LINEAR).map_err(|e| format!("overlay sampler: {e}"))?;
            let nearest =
                sampler(vk::Filter::NEAREST).map_err(|e| format!("overlay sampler: {e}"))?;
            let binding = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&binding),
                    None,
                )
                .map_err(|e| format!("overlay set layout: {e}"))?;
            let sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(IMAGE_SLOTS)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(IMAGE_SLOTS)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("overlay pool: {e}"))?;
            let pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .size(std::mem::size_of::<Push>() as u32)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(std::slice::from_ref(&set_layout))
                        .push_constant_ranges(&pc),
                    None,
                )
                .map_err(|e| format!("overlay layout: {e}"))?;
            // `BlendFunction(ZERO, ONE_MINUS_SRC_COLOR)` and `GUI_TEXTURED`'s
            // translucent blend; the frame keeps its alpha either way.
            let multiply = pipeline(
                &device,
                layout,
                format,
                vk::BlendFactor::ZERO,
                vk::BlendFactor::ONE_MINUS_SRC_COLOR,
            )?;
            let alpha = pipeline(
                &device,
                layout,
                format,
                vk::BlendFactor::SRC_ALPHA,
                vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            )?;
            let mut pass = Self {
                images: (0..IMAGE_SLOTS).map(|_| None).collect(),
                linear,
                nearest,
                set_layout,
                pool,
                layout,
                multiply,
                alpha,
            };
            pass.set_image(gpu, OverlayImage::Vignette, rgba, w, h, true)?;
            pass.set_image(gpu, OverlayImage::White, &[255; 4], 1, 1, false)?;
            Ok(pass)
        }
    }

    /// Upload one overlay image (UNORM), bilinear when `linear`.
    pub fn set_image(
        &mut self,
        gpu: &mut Gpu,
        which: OverlayImage,
        rgba: &[u8],
        w: u32,
        h: u32,
        linear: bool,
    ) -> Result<(), String> {
        if self.images[which as usize].is_some() {
            return Err(format!("overlay image {which:?} set twice"));
        }
        let (image, alloc, view) =
            crate::entities::create_texture_fmt(gpu, rgba, w, h, vk::Format::R8G8B8A8_UNORM)?;
        unsafe {
            let device = &gpu.device;
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.pool)
                        .set_layouts(std::slice::from_ref(&self.set_layout)),
                )
                .map_err(|e| format!("overlay set: {e}"))?[0];
            let info = [vk::DescriptorImageInfo::default()
                .sampler(if linear { self.linear } else { self.nearest })
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
            self.images[which as usize] = Some(Image {
                image,
                alloc: Some(alloc),
                view,
                set,
            });
        }
        Ok(())
    }

    fn draw(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D, draws: &[CameraDraw]) {
        let d = &gpu.device;
        let full = [0.0, 0.0, extent.width as f32, extent.height as f32];
        for c in draws {
            let Some(img) = self.images[c.image as usize].as_ref() else {
                continue;
            };
            // The scissor is the rect, clipped to the frame.
            let x0 = c.rect[0].max(full[0]).floor();
            let y0 = c.rect[1].max(full[1]).floor();
            let x1 = c.rect[2].min(full[2]).ceil();
            let y1 = c.rect[3].min(full[3]).ceil();
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let p = Push {
                rect: c.rect,
                color: c.color,
            };
            unsafe {
                d.cmd_bind_pipeline(
                    cb,
                    vk::PipelineBindPoint::GRAPHICS,
                    if c.multiply {
                        self.multiply
                    } else {
                        self.alpha
                    },
                );
                d.cmd_set_viewport(
                    cb,
                    0,
                    &[vk::Viewport::default()
                        .width(extent.width as f32)
                        .height(extent.height as f32)
                        .max_depth(1.0)],
                );
                d.cmd_set_scissor(
                    cb,
                    0,
                    &[vk::Rect2D {
                        offset: vk::Offset2D {
                            x: x0 as i32,
                            y: y0 as i32,
                        },
                        extent: vk::Extent2D {
                            width: (x1 - x0) as u32,
                            height: (y1 - y0) as u32,
                        },
                    }],
                );
                d.cmd_bind_descriptor_sets(
                    cb,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout,
                    0,
                    &[img.set],
                    &[],
                );
                d.cmd_push_constants(
                    cb,
                    self.layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    0,
                    std::slice::from_raw_parts(
                        (&p as *const Push) as *const u8,
                        std::mem::size_of::<Push>(),
                    ),
                );
                d.cmd_draw(cb, 3, 1, 0, 0);
            }
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let d = gpu.device.clone();
            d.destroy_pipeline(self.multiply, None);
            d.destroy_pipeline(self.alpha, None);
            d.destroy_pipeline_layout(self.layout, None);
            d.destroy_descriptor_pool(self.pool, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
            d.destroy_sampler(self.linear, None);
            d.destroy_sampler(self.nearest, None);
            for img in self.images.iter_mut().flatten() {
                d.destroy_image_view(img.view, None);
                d.destroy_image(img.image, None);
            }
        }
        for img in self.images.iter_mut().flatten() {
            if let Some(a) = img.alloc.take() {
                let _ = gpu.allocator.free(a);
            }
        }
    }
}

fn pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    format: vk::Format,
    src: vk::BlendFactor,
    dst: vk::BlendFactor,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/outline_post.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/vignette.frag.spv")),
        )?;
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(entry),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let ia = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let vp = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let ms = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default();
        let attach = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(src)
            .dst_color_blend_factor(dst)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ZERO)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attach);
        let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
        let formats = [format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&formats)
            .depth_attachment_format(super::DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&ia)
            .viewport_state(&vp)
            .rasterization_state(&raster)
            .multisample_state(&ms)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("overlay pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

impl WorldRenderer {
    /// M282 — build the overlay pass from `misc/vignette.png`. Without the
    /// frame's UNORM twin there is no gamma scope to blend in, and none is
    /// drawn.
    pub fn init_vignette(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        let Some(format) = super::unorm_of(self.color_format) else {
            return Ok(());
        };
        if let Some(mut old) = self.vignette.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
        self.vignette = Some(VignettePass::new(gpu, rgba, w, h, format)?);
        Ok(())
    }

    /// M283 — add one of the other overlay images (after `init_vignette`).
    pub fn init_overlay_image(
        &mut self,
        gpu: &mut Gpu,
        which: OverlayImage,
        rgba: &[u8],
        w: u32,
        h: u32,
        linear: bool,
    ) -> Result<(), String> {
        match self.vignette.as_mut() {
            Some(p) => p.set_image(gpu, which, rgba, w, h, linear),
            None => Ok(()),
        }
    }

    /// M282 — this frame's vignette colour (`Hud.extractVignette`'s), or
    /// `None` for none (the `vignette` option off).
    pub fn set_vignette(&mut self, color: Option<[f32; 3]>) {
        self.gui.vignette = color;
    }

    /// M283 — this frame's other camera overlays (drawn after the vignette,
    /// before the HUD) and its late overlays (after the HUD: the sleep fade).
    pub fn set_camera_overlays(&mut self, early: Vec<CameraDraw>, late: Vec<CameraDraw>) {
        self.gui.camera_overlays = early;
        self.gui.late_overlays = late;
    }

    pub(super) fn draw_vignette(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        let Some(pass) = &self.vignette else {
            return;
        };
        let mut draws: Vec<CameraDraw> = self
            .gui
            .vignette
            .map(|c| CameraDraw {
                image: OverlayImage::Vignette,
                rect: [0.0, 0.0, extent.width as f32, extent.height as f32],
                color: [c[0], c[1], c[2], 1.0],
                multiply: true,
            })
            .into_iter()
            .collect();
        draws.extend_from_slice(&self.gui.camera_overlays);
        if draws.is_empty() {
            return;
        }
        self.in_gamma_space(gpu, cb, extent, || pass.draw(gpu, cb, extent, &draws));
    }

    pub(super) fn draw_late_overlays(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
    ) {
        let (Some(pass), false) = (&self.vignette, self.gui.late_overlays.is_empty()) else {
            return;
        };
        self.in_gamma_space(gpu, cb, extent, || {
            pass.draw(gpu, cb, extent, &self.gui.late_overlays)
        });
    }
}
