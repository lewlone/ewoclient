//! Screen-space text — the vanilla bitmap font rendered as 2D glyph quads,
//! for the chat overlay + the coordinates/debug line. Drawn last (over the
//! HUD) with its own positive viewport (top-left pixel origin), alpha
//! blended, no depth.
//!
//! Each glyph is drawn twice: a black copy offset (+1,+1) scaled px — the
//! vanilla drop shadow — then the tinted glyph, so text stays readable on
//! any background. Layout uses the font's per-glyph advances.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::{create_texture, FontData};
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
const MAX_VERTS: usize = 24_576; // ~1000 glyphs × 2 (shadow) × 6 verts / ~2
const RING: usize = 2;

/// One line of text to draw this frame.
pub struct TextLine<'a> {
    /// Top-left pixel origin (screen-space, before GUI scaling by `px`).
    pub x: f32,
    pub y: f32,
    /// Pixel size of one font pixel (GUI scale; vanilla text cell is 8px).
    pub px: f32,
    /// Linear-space color of the text (shadow is a darkened copy).
    pub color: [f32; 3],
    /// Opacity (chat fades old lines).
    pub alpha: f32,
    /// `graphics.text(font, str, x, y, color, **shadow**)`'s last argument.
    ///
    /// Almost everything the HUD draws passes `true`, and before M79 this pass
    /// hard-coded it. `ContextualBar.extractExperienceLevel` passes `false`
    /// for all five of its draws, because the XP level number carries a
    /// four-way black **outline** instead — and drawing both would put a
    /// shadow copy of every outline copy inside the outline, thickening the
    /// glyph rather than framing it.
    pub shadow: bool,
    pub text: &'a str,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

/// `Font.width(String)` — the sum of the per-glyph advances.
///
/// The same sum [`push_line`](TextPass::push_line) pens out, in whole pixels
/// rather than scaled ones, so a caller can place text it has not drawn yet.
pub fn width(text: &str, advance: &[u8; 256]) -> i32 {
    width_styled(text, advance, false)
}

/// `StringSplitter`'s width provider —
/// `getGlyph(cp).info().getAdvance(style.isBold())`.
///
/// **`GlyphInfo.getBoldOffset()` is `1.0F`, and it is charged per character**,
/// not once per run: a five-character bold word is five pixels wider, not one.
/// That is what makes the style argument load-bearing rather than cosmetic — a
/// style-blind measure wraps a bold chat line late and lets it overhang the
/// box (M126b).
///
/// The byte-wise sum is the pre-existing approximation this inherits: the
/// atlas is indexed by byte, so a multi-byte UTF-8 character is measured as
/// two glyphs. Bold doubles that error rather than creating it.
pub fn width_styled(text: &str, advance: &[u8; 256], bold: bool) -> i32 {
    let extra = i32::from(bold);
    text.bytes()
        .map(|b| advance[b as usize] as i32 + extra)
        .sum()
}

/// `GuiGraphicsExtractor.centeredText` — `text(font, str, x - font.width(str)
/// / 2, y, color)`.
///
/// **Integer division**, and it truncates toward zero, so an odd-width string
/// sits half a pixel left of the true centre. Rounding it instead moves the
/// `+N` bundle badge — and every other centred label — by a pixel against
/// vanilla.
pub fn centered_x(text: &str, advance: &[u8; 256], center_x: i32) -> i32 {
    center_x - width(text, advance) / 2
}

pub struct TextPass {
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pipeline: vk::Pipeline,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    bufs: [vk::Buffer; RING],
    allocs: [Option<Allocation>; RING],
    cursor: usize,
    verts: u32,
    atlas_size: u32,
    cell: u32,
    advance: [u8; 256],
}

impl TextPass {
    pub fn new(gpu: &mut Gpu, color_format: vk::Format, font: &FontData<'_>) -> Result<Self, String> {
        let (image, image_alloc, view) = create_texture(gpu, font.atlas, font.size, font.size)?;
        let device = gpu.device.clone();
        unsafe {
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("text sampler: {e}"))?;
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
                .map_err(|e| format!("text set layout: {e}"))?;
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
                .map_err(|e| format!("text pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("text set: {e}"))?[0];
            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&image_info)],
                &[],
            );

            let pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(8)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&pc),
                    None,
                )
                .map_err(|e| format!("text layout: {e}"))?;
            let pipeline = build_pipeline(&device, layout, color_format)?;

            let mut bufs = [vk::Buffer::null(); RING];
            let mut allocs: [Option<Allocation>; RING] = [None, None];
            for (i, slot) in allocs.iter_mut().enumerate() {
                let buffer = device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(MAX_VERTS as u64 * VERTEX_STRIDE)
                            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("text vbuf: {e}"))?;
                let req = device.get_buffer_memory_requirements(buffer);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: "text-verts",
                        requirements: req,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("text vbuf alloc: {e}"))?;
                device
                    .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("text vbuf bind: {e}"))?;
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
                bufs,
                allocs,
                cursor: 0,
                verts: 0,
                atlas_size: font.size,
                cell: font.cell,
                advance: *font.advance,
            })
        }
    }

    /// Build this frame's glyph quads and draw them.
    pub fn draw(&mut self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D, lines: &[TextLine<'_>]) {
        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(1024);
        for line in lines {
            // Shadow first (offset +1 font-px, darkened), then the glyph.
            if line.shadow {
                let sh = [line.color[0] * 0.25, line.color[1] * 0.25, line.color[2] * 0.25];
                self.push_line(&mut v, line, line.px, line.px, sh, line.alpha);
            }
            self.push_line(&mut v, line, 0.0, 0.0, line.color, line.alpha);
        }
        self.verts = v.len() as u32;
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 32) };
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

    /// Emit one line's glyph quads at (line.x+ox, line.y+oy) in `color`.
    fn push_line(&self, v: &mut Vec<Vertex>, line: &TextLine<'_>, ox: f32, oy: f32, color: [f32; 3], alpha: f32) {
        let px = line.px;
        let cell = self.cell as f32;
        let atlas = self.atlas_size as f32;
        let color4 = [color[0], color[1], color[2], alpha];
        let mut pen = line.x + ox;
        for b in line.text.bytes() {
            let adv = self.advance[b as usize] as f32;
            if b != b' ' && v.len() + 12 <= MAX_VERTS {
                let (cx, cy) = ((b as u32 % 16 * self.cell) as f32, (b as u32 / 16 * self.cell) as f32);
                let (x0, y0) = (pen, line.y + oy);
                let (x1, y1) = (x0 + cell * px, y0 + cell * px);
                let (u0, u1) = (cx / atlas, (cx + cell) / atlas);
                let (t0, t1) = (cy / atlas, (cy + cell) / atlas);
                let corners = [
                    ([x0, y0], [u0, t0]),
                    ([x1, y0], [u1, t0]),
                    ([x1, y1], [u1, t1]),
                    ([x0, y0], [u0, t0]),
                    ([x1, y1], [u1, t1]),
                    ([x0, y1], [u0, t1]),
                ];
                for (pos, uv) in corners {
                    v.push(Vertex { pos, uv, color: color4 });
                }
            }
            pen += adv * px;
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let device = &gpu.device;
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            for b in self.bufs {
                device.destroy_buffer(b, None);
            }
        }
        for a in self.allocs.iter_mut().filter_map(|a| a.take()) {
            let _ = gpu.allocator.free(a);
        }
        if let Some(a) = self.image_alloc.take() {
            let _ = gpu.allocator.free(a);
        }
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
            include_bytes!(concat!(env!("OUT_DIR"), "/text.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/text.frag.spv")),
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
                vk::ColorComponentFlags::R | vk::ColorComponentFlags::G | vk::ColorComponentFlags::B,
            )];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
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
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("text pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}
