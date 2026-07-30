//! The block-break crack overlay (M81) — `RenderPipelines.CRUMBLING`.
//!
//! Geometry comes from `rewo_mesh::crumbling`, which re-collects the block's
//! own model quads and regenerates their UVs. This is the pipeline, and almost
//! all of what is interesting about it is state.
//!
//! # The three pieces of state that make it a decal
//!
//! * **Depth `GREATER_OR_EQUAL`, no write.** Vanilla's own constant is
//!   `DepthStencilState(CompareOp.GREATER_THAN_OR_EQUAL, false, 1.0F, 10.0F)`
//!   — which is also, incidentally, `DepthStencilState.DEFAULT`'s comparison,
//!   so 26.2 is reversed-Z exactly as Rewo is. The decal redraws geometry the
//!   terrain pass has already written depth for, so a strict `GREATER` rejects
//!   every fragment; the third time in this project a depth *comparison* has
//!   been the whole story (M48's trim, M49's GUI item layers).
//! * **A depth bias.** The two trailing floats are
//!   `(depthBiasScaleFactor, depthBiasConstant) = (1.0, 10.0)` — a
//!   slope-scaled and a constant term, pushing the decal toward the camera.
//!   Positive is toward the camera under reversed-Z, and a mutation battery
//!   confirmed the direction empirically: with the bias in place, a decal
//!   drawn against exactly-coplanar terrain passes even a *strict* `GREATER`.
//!
//!   Which is to say the two mechanisms are **individually redundant** here.
//!   `breakshot` can only observe the pair: mutating either one alone leaves
//!   the gate green, and mutating both makes the decal vanish (four witnesses
//!   fail). Recorded because a reader who deletes "the redundant one" will
//!   find the gate agrees with them.
//! * **A multiply blend, in gamma space.** `BlendFunction(DST_COLOR,
//!   SRC_COLOR, ONE, ZERO)` makes the output `2·src·dst` — the crack darkens
//!   whatever is under it rather than painting over it. **Squaring is not
//!   invariant under the sRGB transfer function** (M50's finding, in a second
//!   place): vanilla has no sRGB framebuffer, so it multiplies gamma-encoded
//!   numbers. This pass is therefore built against `world::unorm_of(format)`
//!   and drawn inside `WorldRenderer::with_gamma_space`, and its texture array
//!   is uploaded UNORM so the texel reaches the blender gamma-encoded too. In
//!   linear space a mid-grey crack over a mid-grey block comes out at 0.33
//!   instead of 0.5 — a third too dark, not a subtle shift.
//!
//! # What is not here
//!
//! No fog. Vanilla's `apply_fog` runs on the crumbling fragment, but the
//! geometry is culled at 32 blocks (`distToCenterSqr > 1024.0`) by the
//! extractor, which is inside the render-distance fade at any view distance
//! Rewo runs at. The environmental fog band (M33b) is where this could show;
//! stated as a scoped exclusion rather than half-reproduced.

use ash::vk;

use crate::buf_ring::BufRing;
use crate::Gpu;

/// One decal vertex. 24 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CrumblingVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// The destroy stage, 0..=9 — the texture-array layer.
    pub stage: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    view_proj: [[f32; 4]; 4],
}

pub struct CrumblingPass {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: (vk::Image, vk::ImageView),
    alloc: Option<gpu_allocator::vulkan::Allocation>,
    /// This frame's decals. A [`BufRing`] rather than a bare buffer because
    /// `set_verts` runs before the frame is submitted — see `buf_ring`'s module
    /// docs (M86).
    vbuf: BufRing,
    count: u32,
}

impl CrumblingPass {
    /// `stages` are the ten `destroy_stage_N` textures, `size` square, in
    /// stage order.
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        stages: &[Vec<u8>],
        size: u32,
    ) -> Result<Self, String> {
        let (img, alloc, view) = create_stage_array(gpu, stages, size)?;
        let device = gpu.device.clone();
        // NEAREST, and **REPEAT** rather than CLAMP: the regenerated decal
        // coordinate is signed per face (the projection flips it on four of
        // the six), so a face's tile arrives as `-1..0` as often as `0..1`.
        // Clamping would smear one edge texel across those faces.
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT),
                    None,
                )
                .map_err(|e| format!("crumbling sampler: {e}"))?
        };
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout = unsafe {
            device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("crumbling set layout: {e}"))?
        };
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("crumbling pool: {e}"))?
        };
        let layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&layouts),
                )
                .map_err(|e| format!("crumbling set: {e}"))?[0]
        };
        unsafe {
            let info = [vk::DescriptorImageInfo::default()
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image_view(view)
                .sampler(sampler)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
        }
        let one_layout = [set_layout];
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<Push>() as u32)];
        let layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&one_layout)
                        .push_constant_ranges(&push_range),
                    None,
                )
                .map_err(|e| format!("crumbling layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;
        Ok(Self {
            pipeline,
            layout,
            set_layout,
            pool,
            set,
            sampler,
            image: (img, view),
            alloc: Some(alloc),
            vbuf: BufRing::new(),
            count: 0,
        })
    }

    /// Replace the frame's decal geometry. An empty list draws nothing, which
    /// is the ordinary case.
    pub fn set_verts(&mut self, gpu: &mut Gpu, verts: &[CrumblingVertex]) -> Result<(), String> {
        self.count = verts.len() as u32;
        self.vbuf.set(
            gpu,
            bytemuck::cast_slice(verts),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn draw(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        if self.count == 0 {
            return;
        }
        let Some(vbuf) = self.vbuf.bind() else {
            return;
        };
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf], &[0]);
            let push = Push { view_proj };
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::bytes_of(&push),
            );
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_draw(cb, self.count, 1, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        self.vbuf.destroy(gpu);
        let device = gpu.device.clone();
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.image.1, None);
            device.destroy_image(self.image.0, None);
        }
        if let Some(a) = self.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// The ten stages as a `TYPE_2D_ARRAY`, **UNORM**.
///
/// UNORM and not SRGB, and the reason is the whole gamma story in the module
/// header: the multiply blend must see the texel's stored byte, not its
/// linearised value.
fn create_stage_array(
    gpu: &mut Gpu,
    stages: &[Vec<u8>],
    size: u32,
) -> Result<(vk::Image, gpu_allocator::vulkan::Allocation, vk::ImageView), String> {
    use gpu_allocator::vulkan::AllocationCreateDesc;
    use gpu_allocator::MemoryLocation;
    let count = stages.len().max(1) as u32;
    let bytes_per = (size * size * 4) as usize;
    unsafe {
        let image = gpu
            .device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D {
                        width: size,
                        height: size,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(count)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .map_err(|e| format!("crumbling array image: {e}"))?;
        let req = gpu.device.get_image_memory_requirements(image);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "crumbling-stage-array",
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: gpu_allocator::vulkan::AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("crumbling array alloc: {e}"))?;
        gpu.device
            .bind_image_memory(image, alloc.memory(), alloc.offset())
            .map_err(|e| format!("crumbling array bind: {e}"))?;
        let padded: Vec<Vec<u8>> = if stages.is_empty() {
            vec![vec![0u8; bytes_per]]
        } else {
            stages
                .iter()
                .map(|l| {
                    let mut v = vec![0u8; bytes_per];
                    let n = l.len().min(bytes_per);
                    v[..n].copy_from_slice(&l[..n]);
                    v
                })
                .collect()
        };
        crate::world::upload_texture_array(gpu, image, size, 1, &padded)?;
        let view = gpu
            .device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(0)
                            .layer_count(count),
                    ),
                None,
            )
            .map_err(|e| format!("crumbling array view: {e}"))?;
        Ok((image, alloc, view))
    }
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/crumbling.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/crumbling.frag.spv")),
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
        let binding = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<CrumblingVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(12),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .binding(0)
                .format(vk::Format::R32_UINT)
                .offset(20),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // `CullMode.NONE`, matching the terrain pass: a model element's
        // winding is whatever the model author wrote, and the depth test is
        // what hides the faces a neighbour covers.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .depth_bias_enable(true)
            // `DepthStencilState(_, _, depthBiasScaleFactor = 1.0F,
            // depthBiasConstant = 10.0F)`. Positive, and positive is toward
            // the camera under reversed-Z, which is the direction a decal
            // wants.
            .depth_bias_constant_factor(10.0)
            .depth_bias_slope_factor(1.0)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);
        // `BlendFunction(SRC_COLOR = DST_COLOR, DST_COLOR = SRC_COLOR,
        // SRC_ALPHA = ONE, DST_ALPHA = ZERO)` — `src·dst + dst·src`, i.e.
        // `2·src·dst`. A dark crack texel halves what is under it; a white
        // one doubles it, which is why `destroy_stage` is authored dark.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::DST_COLOR)
            .dst_color_blend_factor(vk::BlendFactor::SRC_COLOR)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend_state =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(crate::world::DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend_state)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("crumbling pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vertex_is_twenty_four_bytes_in_the_order_the_pipeline_declares() {
        assert_eq!(std::mem::size_of::<CrumblingVertex>(), 24);
        // The attribute offsets in `build_pipeline` are literals; if the
        // struct is ever reordered they must move with it.
        assert_eq!(std::mem::offset_of!(CrumblingVertex, pos), 0);
        assert_eq!(std::mem::offset_of!(CrumblingVertex, uv), 12);
        assert_eq!(std::mem::offset_of!(CrumblingVertex, stage), 20);
    }
}
