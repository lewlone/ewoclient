//! M2 world renderer: block texture array (with CPU-generated mips), the
//! textured/depth-tested world pipeline, per-column vertex/index buffers,
//! and CPU frustum culling.
//!
//! M2 constraints (deliberate): buffers are host-visible (CpuToGpu) and
//! uploaded before rendering starts — snapshot viewing, no mid-frame
//! streaming. The device-local mega-buffer arena + async transfer queue is
//! M5. Cull mode is NONE (correctness first); back-face culling is an M5
//! perf lever once winding is regression-tested.

use std::collections::HashMap;

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::Gpu;

pub const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

#[repr(C)]
#[derive(Clone, Copy)]
struct WorldPush {
    view_proj: [[f32; 4]; 4],
    origin: [f32; 4],
}

struct ColumnGpu {
    vbuf: vk::Buffer,
    valloc: Option<Allocation>,
    ibuf: vk::Buffer,
    ialloc: Option<Allocation>,
    index_count: u32,
    origin: [f32; 3],
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
}

pub struct WorldRenderer {
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    columns: HashMap<(i32, i32), ColumnGpu>,
    pub drawn_last_frame: usize,
    pub culled_last_frame: usize,
}

impl WorldRenderer {
    /// Build the pipeline + upload the texture array. `layers` are RGBA8
    /// sRGB texels, `tex_size`² each.
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        tex_size: u32,
        layers: &[Vec<u8>],
    ) -> Result<Self, String> {
        unsafe {
            let layer_count = layers.len().max(1) as u32;
            let mip_levels = 32 - tex_size.leading_zeros(); // 16 → 5

            // -- texture array image ---------------------------------------
            let image = gpu
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_SRGB)
                        .extent(vk::Extent3D {
                            width: tex_size,
                            height: tex_size,
                            depth: 1,
                        })
                        .mip_levels(mip_levels)
                        .array_layers(layer_count)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(|e| format!("texture array: {e}"))?;
            let req = gpu.device.get_image_memory_requirements(image);
            let image_alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "block-texture-array",
                    requirements: req,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("texture alloc: {e}"))?;
            gpu.device
                .bind_image_memory(image, image_alloc.memory(), image_alloc.offset())
                .map_err(|e| format!("texture bind: {e}"))?;

            upload_texture_array(gpu, image, tex_size, mip_levels, layers)?;

            let device = &gpu.device;
            let view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                        .format(vk::Format::R8G8B8A8_SRGB)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .base_mip_level(0)
                                .level_count(mip_levels)
                                .base_array_layer(0)
                                .layer_count(layer_count),
                        ),
                    None,
                )
                .map_err(|e| format!("texture view: {e}"))?;

            // Vanilla look: nearest magnification, linear between mips.
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT)
                        .max_lod(mip_levels as f32),
                    None,
                )
                .map_err(|e| format!("sampler: {e}"))?;

            // -- descriptors ----------------------------------------------
            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("world set layout: {e}"))?;
            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("world descriptor pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("world descriptor set: {e}"))?[0];
            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let writes = [vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&image_info)];
            device.update_descriptor_sets(&writes, &[]);

            // -- pipeline --------------------------------------------------
            let pc_ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(std::mem::size_of::<WorldPush>() as u32)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&pc_ranges),
                    None,
                )
                .map_err(|e| format!("world pipeline layout: {e}"))?;

            let vert = crate::overlay::create_shader(
                device,
                include_bytes!(concat!(env!("OUT_DIR"), "/world.vert.spv")),
            )?;
            let frag = crate::overlay::create_shader(
                device,
                include_bytes!(concat!(env!("OUT_DIR"), "/world.frag.spv")),
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
            let stride = 36u32; // pos 12 + uv 8 + layer 4 + color 12
            let bindings = [vk::VertexInputBindingDescription::default()
                .binding(0)
                .stride(stride)
                .input_rate(vk::VertexInputRate::VERTEX)];
            let attrs = [
                vk::VertexInputAttributeDescription::default()
                    .location(0)
                    .format(vk::Format::R32G32B32_SFLOAT)
                    .offset(0),
                vk::VertexInputAttributeDescription::default()
                    .location(1)
                    .format(vk::Format::R32G32_SFLOAT)
                    .offset(12),
                vk::VertexInputAttributeDescription::default()
                    .location(2)
                    .format(vk::Format::R32_UINT)
                    .offset(20),
                vk::VertexInputAttributeDescription::default()
                    .location(3)
                    .format(vk::Format::R32G32B32_SFLOAT)
                    .offset(24),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attrs);
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
            // Reversed-Z: clear to 0 (far), GREATER passes for nearer. Gives
            // usable depth precision across Minecraft's near/far range on a
            // float depth buffer, where standard [0,1] LESS z-fights the flat
            // terrain into holes at distance.
            let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(true)
                .depth_write_enable(true)
                .depth_compare_op(vk::CompareOp::GREATER);
            let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(false)
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
                .color_attachment_formats(&color_formats)
                .depth_attachment_format(DEPTH_FORMAT);
            let ci = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&input_assembly)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&multisample)
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
                .map_err(|(_, e)| format!("world pipeline: {e}"))?[0];
            device.destroy_shader_module(vert, None);
            device.destroy_shader_module(frag, None);

            Ok(Self {
                set_layout,
                layout,
                pipeline,
                pool,
                set,
                sampler,
                image,
                image_alloc: Some(image_alloc),
                view,
                columns: HashMap::new(),
                drawn_last_frame: 0,
                culled_last_frame: 0,
            })
        }
    }

    /// Create/replace a column's buffers. M2 contract: call only while the
    /// GPU is idle for this renderer (snapshot flow uploads before drawing).
    pub fn upload_column(
        &mut self,
        gpu: &mut Gpu,
        cx: i32,
        cz: i32,
        vertex_bytes: &[u8],
        indices: &[u32],
        y_min: f32,
        y_max: f32,
    ) -> Result<(), String> {
        self.remove_column(gpu, cx, cz);
        unsafe {
            let device = &gpu.device;
            let make_buffer = |gpu: &mut Gpu,
                               bytes: &[u8],
                               usage: vk::BufferUsageFlags,
                               name: &str|
             -> Result<(vk::Buffer, Allocation), String> {
                let buffer = gpu
                    .device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(bytes.len().max(4) as u64)
                            .usage(usage)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("{name}: {e}"))?;
                let req = gpu.device.get_buffer_memory_requirements(buffer);
                let mut alloc = gpu
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
                alloc
                    .mapped_slice_mut()
                    .ok_or_else(|| format!("{name}: not mapped"))?[..bytes.len()]
                    .copy_from_slice(bytes);
                Ok((buffer, alloc))
            };
            let _ = device;
            let (vbuf, valloc) = make_buffer(
                gpu,
                vertex_bytes,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                "column-vertices",
            )?;
            let (ibuf, ialloc) = make_buffer(
                gpu,
                bytemuck_cast(indices),
                vk::BufferUsageFlags::INDEX_BUFFER,
                "column-indices",
            )?;
            let origin = [cx as f32 * 16.0, 0.0, cz as f32 * 16.0];
            self.columns.insert(
                (cx, cz),
                ColumnGpu {
                    vbuf,
                    valloc: Some(valloc),
                    ibuf,
                    ialloc: Some(ialloc),
                    index_count: indices.len() as u32,
                    origin,
                    aabb_min: [origin[0], y_min, origin[2]],
                    aabb_max: [origin[0] + 16.0, y_max, origin[2] + 16.0],
                },
            );
        }
        Ok(())
    }

    pub fn remove_column(&mut self, gpu: &mut Gpu, cx: i32, cz: i32) {
        if let Some(col) = self.columns.remove(&(cx, cz)) {
            unsafe {
                gpu.device.destroy_buffer(col.vbuf, None);
                gpu.device.destroy_buffer(col.ibuf, None);
            }
            if let Some(a) = col.valloc {
                let _ = gpu.allocator.free(a);
            }
            if let Some(a) = col.ialloc {
                let _ = gpu.allocator.free(a);
            }
        }
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Record world draws. Caller has begun dynamic rendering with a depth
    /// attachment (`DEPTH_FORMAT`).
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let planes = frustum_planes(&view_proj);
        unsafe {
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            // Flip Y via negative viewport height: glam's RH/zero-to-one
            // projection is Y-up; Vulkan framebuffer space is Y-down.
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            let mut drawn = 0;
            let mut culled = 0;
            for col in self.columns.values() {
                if !aabb_visible(&planes, col.aabb_min, col.aabb_max) {
                    culled += 1;
                    continue;
                }
                let push = WorldPush {
                    view_proj,
                    origin: [col.origin[0], col.origin[1], col.origin[2], 0.0],
                };
                let bytes = std::slice::from_raw_parts(
                    (&push as *const WorldPush) as *const u8,
                    std::mem::size_of::<WorldPush>(),
                );
                device.cmd_push_constants(
                    cb,
                    self.layout,
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    bytes,
                );
                device.cmd_bind_vertex_buffers(cb, 0, &[col.vbuf], &[0]);
                device.cmd_bind_index_buffer(cb, col.ibuf, 0, vk::IndexType::UINT32);
                device.cmd_draw_indexed(cb, col.index_count, 1, 0, 0, 0);
                drawn += 1;
            }
            self.drawn_last_frame = drawn;
            self.culled_last_frame = culled;
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        let keys: Vec<_> = self.columns.keys().copied().collect();
        for (cx, cz) in keys {
            self.remove_column(gpu, cx, cz);
        }
        unsafe {
            let device = &gpu.device;
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(a) = self.image_alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

fn bytemuck_cast(indices: &[u32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(indices.as_ptr() as *const u8, indices.len() * 4)
    }
}

/// Infinite reversed-Z perspective (RH, Y-up world). Maps z_view=-near → 1,
/// z_view=-∞ → 0; pair with a 0.0 depth clear + `GREATER` compare. No far
/// plane. Column-major, matching glam's `Mat4::to_cols_array_2d`.
pub fn perspective_reverse_z(fov_y_rad: f32, aspect: f32, near: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov_y_rad * 0.5).tan();
    // Columns.
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, 0.0, -1.0],
        [0.0, 0.0, near, 0.0],
    ]
}

/// Box-filter mips in sRGB space (matches the vanilla look closely enough
/// for M2; alpha-coverage-preserving mips are the M4 cutout item).
fn generate_mips(tex_size: u32, mip_levels: u32, base: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![base.to_vec()];
    let mut prev_size = tex_size as usize;
    for _ in 1..mip_levels {
        let size = (prev_size / 2).max(1);
        let prev = out.last().unwrap();
        let mut mip = vec![0u8; size * size * 4];
        for y in 0..size {
            for x in 0..size {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        let sy = (y * 2 + dy).min(prev_size - 1);
                        let sx = (x * 2 + dx).min(prev_size - 1);
                        sum += prev[(sy * prev_size + sx) * 4 + c] as u32;
                    }
                    mip[(y * size + x) * 4 + c] = (sum / 4) as u8;
                }
            }
        }
        out.push(mip);
        prev_size = size;
    }
    out
}

fn upload_texture_array(
    gpu: &mut Gpu,
    image: vk::Image,
    tex_size: u32,
    mip_levels: u32,
    layers: &[Vec<u8>],
) -> Result<(), String> {
    // Staging layout: mip-major, layers contiguous inside each mip, so one
    // copy region per mip covers every layer.
    let layer_count = layers.len().max(1);
    let mut staging_data = Vec::new();
    let mut mip_offsets = Vec::new();
    let per_layer_mips: Vec<Vec<Vec<u8>>> = if layers.is_empty() {
        vec![generate_mips(tex_size, mip_levels, &vec![255u8; (tex_size * tex_size * 4) as usize])]
    } else {
        layers
            .iter()
            .map(|l| generate_mips(tex_size, mip_levels, l))
            .collect()
    };
    for mip in 0..mip_levels as usize {
        mip_offsets.push(staging_data.len() as u64);
        for layer_mips in &per_layer_mips {
            staging_data.extend_from_slice(&layer_mips[mip]);
        }
    }

    unsafe {
        let device = &gpu.device;
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(staging_data.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("staging: {e}"))?;
        let req = device.get_buffer_memory_requirements(staging);
        let mut staging_alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "texture-staging",
                requirements: req,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("staging alloc: {e}"))?;
        device
            .bind_buffer_memory(staging, staging_alloc.memory(), staging_alloc.offset())
            .map_err(|e| format!("staging bind: {e}"))?;
        staging_alloc
            .mapped_slice_mut()
            .ok_or("staging not mapped")?[..staging_data.len()]
            .copy_from_slice(&staging_data);

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                None,
            )
            .map_err(|e| format!("upload pool: {e}"))?;
        let cb = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("upload cb: {e}"))?[0];
        device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("upload begin: {e}"))?;

        let full_range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(mip_levels)
            .base_array_layer(0)
            .layer_count(layer_count as u32);
        let to_dst = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(full_range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_dst)),
        );

        let mut regions = Vec::new();
        for mip in 0..mip_levels {
            let size = (tex_size >> mip).max(1);
            regions.push(
                vk::BufferImageCopy::default()
                    .buffer_offset(mip_offsets[mip as usize])
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .mip_level(mip)
                            .base_array_layer(0)
                            .layer_count(layer_count as u32),
                    )
                    .image_extent(vk::Extent3D {
                        width: size,
                        height: size,
                        depth: 1,
                    }),
            );
        }
        device.cmd_copy_buffer_to_image(
            cb,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &regions,
        );

        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(full_range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_read)),
        );
        device
            .end_command_buffer(cb)
            .map_err(|e| format!("upload end: {e}"))?;

        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|e| format!("upload fence: {e}"))?;
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
        let submit = vk::SubmitInfo2::default().command_buffer_infos(&cbs);
        device
            .queue_submit2(gpu.graphics_queue, std::slice::from_ref(&submit), fence)
            .map_err(|e| format!("upload submit: {e}"))?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .map_err(|e| format!("upload wait: {e}"))?;
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        let _ = gpu.allocator.free(staging_alloc);
    }
    Ok(())
}

/// A shared depth target for the world pass.
pub struct DepthTarget {
    pub image: vk::Image,
    alloc: Option<Allocation>,
    pub view: vk::ImageView,
    pub extent: vk::Extent2D,
}

impl DepthTarget {
    pub fn new(gpu: &mut Gpu, extent: vk::Extent2D) -> Result<Self, String> {
        unsafe {
            let device = &gpu.device;
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

    /// Transition UNDEFINED → DEPTH_ATTACHMENT for this frame's clear+use.
    pub fn barrier_for_use(&self, gpu: &Gpu, cb: vk::CommandBuffer) {
        unsafe {
            let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
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

// -- frustum ----------------------------------------------------------------

type Plane = [f32; 4];

/// Gribb–Hartmann plane extraction from a column-major view_proj with
/// Vulkan-style clip (z in [0, w]).
fn frustum_planes(m: &[[f32; 4]; 4]) -> [Plane; 6] {
    let row = |i: usize| -> [f32; 4] { [m[0][i], m[1][i], m[2][i], m[3][i]] };
    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
    [
        add(r3, r0), // left
        sub(r3, r0), // right
        add(r3, r1), // bottom
        sub(r3, r1), // top
        r2,          // near (z >= 0)
        sub(r3, r2), // far
    ]
}

fn aabb_visible(planes: &[Plane; 6], min: [f32; 3], max: [f32; 3]) -> bool {
    for p in planes {
        // Positive vertex: the AABB corner furthest along the plane normal.
        let v = [
            if p[0] >= 0.0 { max[0] } else { min[0] },
            if p[1] >= 0.0 { max[1] } else { min[1] },
            if p[2] >= 0.0 { max[2] } else { min[2] },
        ];
        if p[0] * v[0] + p[1] * v[1] + p[2] * v[2] + p[3] < 0.0 {
            return false;
        }
    }
    true
}
