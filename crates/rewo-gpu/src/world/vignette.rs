//! M282 — the HUD vignette: `misc/vignette.png` over the whole GUI, the frame
//! multiplied by one minus it (`RenderPipelines.VIGNETTE`), drawn after the
//! hand and before the HUD (`Hud.extractCameraOverlays` runs first in the
//! HUD), in the gamma scope.

use ash::vk;
use gpu_allocator::vulkan::Allocation;

use super::WorldRenderer;
use crate::Gpu;

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    size: [f32; 2],
    pad: [f32; 2],
    color: [f32; 4],
}

pub struct VignettePass {
    image: vk::Image,
    alloc: Option<Allocation>,
    view: vk::ImageView,
    sampler: vk::Sampler,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

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
        let (image, alloc, view) =
            crate::entities::create_texture_fmt(gpu, rgba, w, h, vk::Format::R8G8B8A8_UNORM)?;
        unsafe {
            let device = gpu.device.clone();
            // `blur: true` in the texture's mcmeta: bilinear.
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .max_lod(0.0),
                    None,
                )
                .map_err(|e| format!("vignette sampler: {e}"))?;
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
                .map_err(|e| format!("vignette set layout: {e}"))?;
            let sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("vignette pool: {e}"))?;
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(std::slice::from_ref(&set_layout)),
                )
                .map_err(|e| format!("vignette set: {e}"))?[0];
            let info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
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
                .map_err(|e| format!("vignette layout: {e}"))?;
            let vert = crate::overlay::create_shader(
                &device,
                include_bytes!(concat!(env!("OUT_DIR"), "/outline_post.vert.spv")),
            )?;
            let frag = crate::overlay::create_shader(
                &device,
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
            // `BlendFunction(ZERO, ONE_MINUS_SRC_COLOR)`; the frame keeps its
            // alpha (the UI passes mask alpha writes).
            let attach = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ZERO)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_COLOR)
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
                .create_graphics_pipelines(
                    vk::PipelineCache::null(),
                    std::slice::from_ref(&ci),
                    None,
                )
                .map_err(|(_, e)| format!("vignette pipeline: {e}"))?[0];
            device.destroy_shader_module(vert, None);
            device.destroy_shader_module(frag, None);
            Ok(Self {
                image,
                alloc: Some(alloc),
                view,
                sampler,
                set_layout,
                pool,
                set,
                layout,
                pipeline,
            })
        }
    }

    fn draw(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D, color: [f32; 3]) {
        let d = &gpu.device;
        let p = Push {
            size: [extent.width as f32, extent.height as f32],
            pad: [0.0; 2],
            color: [color[0], color[1], color[2], 1.0],
        };
        unsafe {
            d.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            d.cmd_set_viewport(
                cb,
                0,
                &[vk::Viewport::default()
                    .width(extent.width as f32)
                    .height(extent.height as f32)
                    .max_depth(1.0)],
            );
            d.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            d.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.set],
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

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let d = gpu.device.clone();
            d.destroy_pipeline(self.pipeline, None);
            d.destroy_pipeline_layout(self.layout, None);
            d.destroy_descriptor_pool(self.pool, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
            d.destroy_sampler(self.sampler, None);
            d.destroy_image_view(self.view, None);
            d.destroy_image(self.image, None);
        }
        if let Some(a) = self.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

impl WorldRenderer {
    /// M282 — build the vignette pass from `misc/vignette.png`. Without the
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

    /// M282 — this frame's vignette colour (`Hud.extractVignette`'s), or
    /// `None` for none (the `vignette` option off).
    pub fn set_vignette(&mut self, color: Option<[f32; 3]>) {
        self.gui.vignette = color;
    }

    pub(super) fn draw_vignette(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        let (Some(pass), Some(color)) = (&self.vignette, self.gui.vignette) else {
            return;
        };
        self.in_gamma_space(gpu, cb, extent, || pass.draw(gpu, cb, extent, color));
    }
}
