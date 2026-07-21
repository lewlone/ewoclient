//! Frame-time strip-chart overlay: one fullscreen triangle, fragment shader
//! reads a ring of frame times from a storage buffer. No text — M0 has no
//! font engine, and the chart is the number that matters (hitches are
//! visible as ember bars regardless of scale).

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::Gpu;

/// Ring capacity — one bar per sample.
pub const OVERLAY_SAMPLES: usize = 240;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct OverlayParams {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub scale_ms: f32,
    pub count: u32,
    pub head: u32,
    pub _pad: u32,
}

/// Per-draw input assembled by the caller each frame.
pub struct OverlayDraw<'a> {
    /// Exactly `OVERLAY_SAMPLES` entries, ring order.
    pub samples_ms: &'a [f32],
    /// Index of the OLDEST sample in the ring.
    pub head: u32,
    /// Frame time mapped to full chart height.
    pub scale_ms: f32,
    /// Chart rect in framebuffer pixels.
    pub origin: [f32; 2],
    pub size: [f32; 2],
}

pub struct OverlayPipeline {
    pub set_layout: vk::DescriptorSetLayout,
    pub layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
    pub pool: vk::DescriptorPool,
    pub color_format: vk::Format,
}

/// One per frame-in-flight: the sample buffer this frame's draw reads.
pub struct OverlayFrameRes {
    pub buffer: vk::Buffer,
    pub alloc: Option<Allocation>,
    pub set: vk::DescriptorSet,
}

impl OverlayPipeline {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        frame_count: usize,
    ) -> Result<(Self, Vec<OverlayFrameRes>), String> {
        unsafe {
            let device = &gpu.device;

            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("overlay set layout: {e}"))?;

            let pc_ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<OverlayParams>() as u32)];
            let set_layouts = [set_layout];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&pc_ranges),
                    None,
                )
                .map_err(|e| format!("overlay pipeline layout: {e}"))?;

            let vert = create_shader(
                device,
                include_bytes!(concat!(env!("OUT_DIR"), "/overlay.vert.spv")),
            )?;
            let frag = create_shader(
                device,
                include_bytes!(concat!(env!("OUT_DIR"), "/overlay.frag.spv")),
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
            let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);
            let multisample = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
                .alpha_blend_op(vk::BlendOp::ADD)
                // RGB only: keep the clear's alpha=1 in the attachment so
                // readback PNGs (and later present paths that honor alpha)
                // aren't ghosted by the overlay's blend alpha.
                .color_write_mask(
                    vk::ColorComponentFlags::R
                        | vk::ColorComponentFlags::G
                        | vk::ColorComponentFlags::B,
                )];
            let blend =
                vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let color_formats = [color_format];
            let mut rendering = vk::PipelineRenderingCreateInfo::default()
                .color_attachment_formats(&color_formats);
            let ci = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&input_assembly)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&multisample)
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
                .map_err(|(_, e)| format!("overlay pipeline: {e}"))?[0];

            device.destroy_shader_module(vert, None);
            device.destroy_shader_module(frag, None);

            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(frame_count as u32)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(frame_count as u32)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("overlay descriptor pool: {e}"))?;
            let layouts = vec![set_layout; frame_count];
            let sets = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&layouts),
                )
                .map_err(|e| format!("overlay descriptor sets: {e}"))?;

            let me = Self {
                set_layout,
                layout,
                pipeline,
                pool,
                color_format,
            };

            let mut frames = Vec::with_capacity(frame_count);
            for (i, set) in sets.into_iter().enumerate() {
                let size = (OVERLAY_SAMPLES * std::mem::size_of::<f32>()) as u64;
                let buffer = device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(size)
                            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("overlay buffer: {e}"))?;
                let requirements = device.get_buffer_memory_requirements(buffer);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: &format!("overlay-samples-{i}"),
                        requirements,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("overlay alloc: {e}"))?;
                device
                    .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("overlay bind: {e}"))?;

                let buffer_info = [vk::DescriptorBufferInfo::default()
                    .buffer(buffer)
                    .offset(0)
                    .range(vk::WHOLE_SIZE)];
                let writes = [vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&buffer_info)];
                device.update_descriptor_sets(&writes, &[]);

                frames.push(OverlayFrameRes {
                    buffer,
                    alloc: Some(alloc),
                    set,
                });
            }

            Ok((me, frames))
        }
    }

    /// Upload this frame's ring + record the overlay draw. The caller has
    /// already begun dynamic rendering on `cb`.
    pub fn draw(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        res: &mut OverlayFrameRes,
        draw: &OverlayDraw,
        extent: vk::Extent2D,
    ) {
        debug_assert_eq!(draw.samples_ms.len(), OVERLAY_SAMPLES);
        if let Some(alloc) = res.alloc.as_mut() {
            if let Some(slice) = alloc.mapped_slice_mut() {
                let bytes: &[u8] = bytemuck::cast_slice(draw.samples_ms);
                slice[..bytes.len()].copy_from_slice(bytes);
            }
        }
        let params = OverlayParams {
            origin: draw.origin,
            size: draw.size,
            scale_ms: draw.scale_ms,
            count: OVERLAY_SAMPLES as u32,
            head: draw.head,
            _pad: 0,
        };
        unsafe {
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default()
                .width(extent.width as f32)
                .height(extent.height as f32)
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            let scissor = vk::Rect2D::default().extent(extent);
            device.cmd_set_scissor(cb, 0, &[scissor]);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[res.set],
                &[],
            );
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::bytes_of(&params),
            );
            device.cmd_draw(cb, 3, 1, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu, frames: &mut Vec<OverlayFrameRes>) {
        unsafe {
            for f in frames.drain(..) {
                if let Some(alloc) = f.alloc {
                    let _ = gpu.allocator.free(alloc);
                }
                gpu.device.destroy_buffer(f.buffer, None);
            }
            gpu.device.destroy_descriptor_pool(self.pool, None);
            gpu.device.destroy_pipeline(self.pipeline, None);
            gpu.device.destroy_pipeline_layout(self.layout, None);
            gpu.device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

pub(crate) fn create_shader(device: &ash::Device, bytes: &[u8]) -> Result<vk::ShaderModule, String> {
    let code = ash::util::read_spv(&mut std::io::Cursor::new(bytes))
        .map_err(|e| format!("read spv: {e}"))?;
    unsafe {
        device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)
            .map_err(|e| format!("shader module: {e}"))
    }
}
