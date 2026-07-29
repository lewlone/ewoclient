//! Velvet text pass (M52b step 2b) — the GPU half of the Velvet type stack.
//!
//! `velvet_glyph` does the CPU work: rasterize, blur, pack, lay out. This
//! uploads that atlas and draws the positioned quads. The structure
//! deliberately mirrors [`crate::text::TextPass`] — same ring-buffered vertex
//! stream, same push-constant screen size, same blend — so the two text
//! renderers stay comparable. The differences are exactly two:
//!
//! * The atlas is **R8_UNORM coverage**, not RGBA, so the fragment shader
//!   samples `.r`. Vanilla's font atlas puts its mask in `.a`.
//! * The atlas **grows**. `velvet_glyph` doubles and re-packs when it runs
//!   out, which invalidates every cached rect, so the upload path has to
//!   handle a size change rather than assuming a fixed texture.
//!
//! ## Sampling: LINEAR, and the deviation that buys
//!
//! Glyph quads land on fractional pen positions (`pen += advance + spacing`
//! accumulates floats), so the sampler is `LINEAR`. Skia rasterizes with
//! *subpixel positioning* — it re-rasterizes a glyph for its fractional
//! offset — where this rasterizes once at an integer origin and lets the
//! sampler interpolate. The result is a fraction of a pixel softer on
//! non-integer positions.
//!
//! The alternative, snapping each quad to whole pixels, is crisper per glyph
//! but makes *tracking* uneven: a 0.22em step rounds differently per glyph and
//! the letter-spacing visibly stutters. Even spacing matters more here than a
//! sub-pixel of sharpness, because tracked labels are a Velvet signature.
//! Recorded as a known deviation for the §6 gate to measure rather than
//! discover.
//!
//! ## Colour space
//!
//! Same requirement as the chrome pass: the Velvet UI blends in gamma space,
//! so this draws inside `WorldRenderer::with_gamma_space` and must be built
//! with `world::unorm_of(target_format)`. If the plate blends in gamma and
//! the type on top of it blends in linear they disagree worst exactly where
//! they overlap, which is the whole widget.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::velvet_glyph::{GlyphCache, PositionedGlyph};
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
const MAX_VERTS: usize = 65_536;
const RING: usize = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

/// An owned run, for handing a frame's worth of text to the renderer across
/// a `set_*` call the way `OwnedTextLine` already does.
#[derive(Debug, Clone)]
pub struct OwnedRun {
    pub glyphs: Vec<PositionedGlyph>,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// One drawable run: glyphs already positioned by
/// [`GlyphCache::layout_run`], plus the colour to tint them.
pub struct Run<'a> {
    pub glyphs: &'a [PositionedGlyph],
    /// Linear RGB.
    pub color: [f32; 3],
    pub alpha: f32,
}

pub struct VelvetTextPass {
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pipeline: vk::Pipeline,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    /// Edge length of the currently uploaded atlas, so a grow is detectable.
    uploaded_edge: u32,
    bufs: [vk::Buffer; RING],
    allocs: [Option<Allocation>; RING],
    cursor: usize,
    verts: u32,
}

impl VelvetTextPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        cache: &GlyphCache,
    ) -> Result<Self, String> {
        let edge = cache.atlas_edge();
        let (image, image_alloc, view) = crate::entities::create_texture_r8(
            gpu,
            cache.atlas(),
            edge,
            edge,
        )?;
        let device = gpu.device.clone();
        unsafe {
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("velvet text sampler: {e}"))?;
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
                .map_err(|e| format!("velvet text set layout: {e}"))?;
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
                .map_err(|e| format!("velvet text pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("velvet text set: {e}"))?[0];
            write_set(&device, set, sampler, view);

            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(8)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push),
                    None,
                )
                .map_err(|e| format!("velvet text layout: {e}"))?;
            let pipeline = build_pipeline(&device, layout, color_format)?;

            let mut bufs = [vk::Buffer::null(); RING];
            let mut allocs: [Option<Allocation>; RING] = [None, None];
            for (i, slot) in allocs.iter_mut().enumerate() {
                let buffer = device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(VERTEX_STRIDE * MAX_VERTS as u64)
                            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("velvet text buffer: {e}"))?;
                let req = device.get_buffer_memory_requirements(buffer);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: "velvet-text-vertices",
                        requirements: req,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("velvet text alloc: {e}"))?;
                device
                    .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("velvet text bind: {e}"))?;
                bufs[i] = buffer;
                *slot = Some(alloc);
            }

            Ok(Self {
                layout,
                set_layout,
                pipeline,
                pool,
                set,
                sampler,
                image,
                image_alloc: Some(image_alloc),
                view,
                uploaded_edge: edge,
                bufs,
                allocs,
                cursor: 0,
                verts: 0,
            })
        }
    }

    /// Re-upload the atlas if the cache has touched it.
    ///
    /// Must be called **outside** a render pass — it records a transfer. The
    /// cache marks itself dirty on any blit and on a grow; a grow additionally
    /// changes the edge, which is why the texture is rebuilt rather than
    /// updated in place.
    pub fn sync_atlas(&mut self, gpu: &mut Gpu, cache: &mut GlyphCache) -> Result<(), String> {
        if !cache.dirty() {
            return Ok(());
        }
        let edge = cache.atlas_edge();
        let (image, alloc, view) =
            crate::entities::create_texture_r8(gpu, cache.atlas(), edge, edge)?;
        // The old texture may still be referenced by frames in flight, so the
        // caller is expected to have idled or to be outside the ring. Rewo's
        // other passes take the same stance on a resize.
        unsafe {
            let device = gpu.device.clone();
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            if let Some(a) = self.image_alloc.take() {
                let _ = gpu.allocator.free(a);
            }
            self.image = image;
            self.image_alloc = Some(alloc);
            self.view = view;
            self.uploaded_edge = edge;
            write_set(&device, self.set, self.sampler, self.view);
        }
        cache.clear_dirty();
        Ok(())
    }

    /// Edge of the atlas currently on the GPU — used by the gate to confirm a
    /// grow actually reached the device.
    pub fn uploaded_edge(&self) -> u32 {
        self.uploaded_edge
    }

    pub fn draw(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        runs: &[Run<'_>],
    ) {
        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(1024);
        for run in runs {
            let c = [run.color[0], run.color[1], run.color[2], run.alpha];
            for g in run.glyphs {
                if v.len() + 6 > MAX_VERTS {
                    break;
                }
                let (x0, y0) = (g.dst_x, g.dst_y);
                let (x1, y1) = (x0 + g.dst_w, y0 + g.dst_h);
                let corners = [
                    ([x0, y0], [g.u0, g.v0]),
                    ([x1, y0], [g.u1, g.v0]),
                    ([x1, y1], [g.u1, g.v1]),
                    ([x0, y0], [g.u0, g.v0]),
                    ([x1, y1], [g.u1, g.v1]),
                    ([x0, y1], [g.u0, g.v1]),
                ];
                for (pos, uv) in corners {
                    v.push(Vertex { pos, uv, color: c });
                }
            }
        }
        self.verts = v.len() as u32;
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    v.as_ptr() as *const u8,
                    v.len() * VERTEX_STRIDE as usize,
                )
            };
            slice[..bytes.len()].copy_from_slice(bytes);
        }
        if self.verts == 0 {
            return;
        }

        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default().width(w).height(h).max_depth(1.0);
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
            let screen = [w, h];
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                std::slice::from_raw_parts(screen.as_ptr() as *const u8, 8),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[self.bufs[self.cursor]], &[0]);
            device.cmd_draw(cb, self.verts, 1, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let device = gpu.device.clone();
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            if let Some(a) = self.image_alloc.take() {
                let _ = gpu.allocator.free(a);
            }
            for (buf, alloc) in self.bufs.iter().zip(self.allocs.iter_mut()) {
                device.destroy_buffer(*buf, None);
                if let Some(a) = alloc.take() {
                    let _ = gpu.allocator.free(a);
                }
            }
        }
    }
}

fn write_set(
    device: &ash::Device,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    view: vk::ImageView,
) {
    let image_info = [vk::DescriptorImageInfo::default()
        .sampler(sampler)
        .image_view(view)
        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
    unsafe {
        device.update_descriptor_sets(
            &[vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&image_info)],
            &[],
        );
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
            include_bytes!(concat!(env!("OUT_DIR"), "/velvet_text.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/velvet_text.frag.spv")),
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
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(VERTEX_STRIDE as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(16),
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
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
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
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("velvet text pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}
