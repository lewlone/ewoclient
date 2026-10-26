//! Line-style world overlays: the block-selection outline and the leash
//! ribbons. Both are small per-frame vertex rings drawn depth-tested over the
//! terrain.

use ash::vk;

use super::{unorm_of, WorldRenderer, DEPTH_FORMAT};
use crate::buf_ring::BufRing;
use crate::Gpu;

#[repr(C)]
#[derive(Clone, Copy)]
struct LinePush {
    view_proj: [[f32; 4]; 4],
    color: [f32; 4],
    /// Viewport width and height in pixels, then the line width in pixels.
    screen: [f32; 4],
}

/// One corner of a selection-outline segment's screen-space quad
/// (`rendertype_lines.vsh`): this end, the other end, and which side.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub other: [f32; 3],
    pub side: f32,
}

const LINE_VERTEX_STRIDE: u64 = 28;
const _: () = assert!(std::mem::size_of::<LineVertex>() == LINE_VERTEX_STRIDE as usize);

/// The selection outline's box budget (a block's outline shape); a shape
/// with more draws its first boxes.
const SELECTION_MAX_BOXES: usize = 64;

/// `RenderTypes.lines()`' `VIEW_OFFSET_Z_LAYERING`: the outline is scaled
/// toward the camera by this, so it wins the depth test against the faces it
/// lies on (vanilla's `0.99975586F`, `1 - 1/4096`).
const VIEW_OFFSET_Z: f32 = 0.999_755_86;

/// `Window.getAppropriateLineWidth()`: `max(2.5, width / 1920 * 2.5)`.
pub fn outline_line_width(window_width: u32) -> f32 {
    (window_width as f32 / 1920.0 * 2.5).max(2.5)
}

/// The selection outline's vertices: every edge of every box (block-local
/// `0..1` coordinates, offset by `block`), pulled toward `eye` by
/// [`VIEW_OFFSET_Z`], six vertices (two triangles) per edge.
///
/// Vanilla walks the optimised shape's edges (`VoxelShape.forAllEdges`), so a
/// stair shows no seam between its boxes; here each box draws its own twelve
/// edges, which adds those seams on multi-box shapes.
pub fn selection_vertices(block: [i32; 3], boxes: &[[f32; 6]], eye: [f32; 3]) -> Vec<LineVertex> {
    let unit = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    let boxes = if boxes.is_empty() { &unit[..] } else { boxes };
    let pull = |p: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|i| eye[i] + (p[i] - eye[i]) * VIEW_OFFSET_Z)
    };
    let mut out = Vec::with_capacity(boxes.len().min(SELECTION_MAX_BOXES) * 72);
    for b in boxes.iter().take(SELECTION_MAX_BOXES) {
        let o = [block[0] as f32, block[1] as f32, block[2] as f32];
        let (x0, y0, z0) = (o[0] + b[0], o[1] + b[1], o[2] + b[2]);
        let (x1, y1, z1) = (o[0] + b[3], o[1] + b[4], o[2] + b[5]);
        let c = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y0, z1],
            [x0, y0, z1],
            [x0, y1, z0],
            [x1, y1, z0],
            [x1, y1, z1],
            [x0, y1, z1],
        ]
        .map(pull);
        const EDGES: [(usize, usize); 12] = [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];
        for (a, b) in EDGES {
            let (pa, pb) = (c[a], c[b]);
            let v = |pos, other, side| LineVertex { pos, other, side };
            out.extend([
                v(pa, pb, 1.0),
                v(pa, pb, -1.0),
                v(pb, pa, 1.0),
                v(pb, pa, 1.0),
                v(pa, pb, -1.0),
                v(pb, pa, -1.0),
            ]);
        }
    }
    out
}

/// The leash ribbon's push (M170) — colour is per-vertex, so only the camera.
#[repr(C)]
#[derive(Clone, Copy)]
struct LeashPush {
    view_proj: [[f32; 4]; 4],
}

/// Ribbon vertex budget: ~13 leashes at 294 verts each (98 triangles). A
/// leash beyond the cap is dropped whole rather than torn.
const LEASH_MAX_VERTS: usize = 4096;

/// Line pipeline for the selection outline: LINE_LIST, depth-tested
/// (reversed-Z GREATER) but no depth write, alpha-blended.
fn build_leash_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/leash.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/leash.frag.spv")),
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
        // pos vec3 @0, color vec3 @12: one `LeashVertex`.
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<crate::leash::LeashVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(12),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // Cull NONE: the strip's winding alternates, and the ribbon is meant to
        // be seen from both sides.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::GREATER); // reversed-Z
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
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("leash pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

fn build_line_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/selection_line.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/selection_line.frag.spv")),
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
            .stride(LINE_VERTEX_STRIDE as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(12),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .format(vk::Format::R32_SFLOAT)
                .offset(24),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        // Screen-space quads, two triangles per segment (`line.vert`).
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
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::GREATER); // reversed-Z
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
            .map_err(|(_, e)| format!("line pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

/// The block-selection outline (screen-space quads, depth-tested against
/// terrain) and the vertex ring its vertices are written to. `gamma_pipeline`
/// is the same pipeline for the UNORM twin format, so the 0.4-alpha black
/// blends in gamma space as vanilla's does; `None` without such a view.
pub(super) struct SelectOutline {
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    gamma_pipeline: Option<vk::Pipeline>,
    ring: BufRing,
    /// The outline shape's boxes for this frame's selection (empty = a cube).
    pub(super) boxes: Vec<[f32; 6]>,
}

/// Leash ribbons (M170): a TRIANGLE_LIST of position+colour verts, world
/// space, depth-tested against terrain and entities. Rebuilt each frame.
pub(super) struct LeashRibbon {
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    ring: BufRing,
    warned: bool,
    verts: Vec<crate::leash::LeashVertex>,
}

/// Build both overlays' pipelines and rings.
pub(super) fn create_line_overlays(
    gpu: &mut Gpu,
    color_format: vk::Format,
) -> Result<(SelectOutline, LeashRibbon), String> {
    let device = gpu.device.clone();
    unsafe {
        // ---- selection outline (lines) ----
        let line_pc = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<LinePush>() as u32)];
        let select_layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&line_pc),
                None,
            )
            .map_err(|e| format!("select layout: {e}"))?;
        let select_pipeline = build_line_pipeline(&device, select_layout, color_format)?;
        let select_gamma_pipeline = match unorm_of(color_format) {
            Some(unorm) => Some(build_line_pipeline(&device, select_layout, unorm)?),
            None => None,
        };
        // ---- leash ribbon (M170) ----
        let leash_pc = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<LeashPush>() as u32)];
        let leash_layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&leash_pc),
                None,
            )
            .map_err(|e| format!("leash layout: {e}"))?;
        let leash_pipeline = build_leash_pipeline(&device, leash_layout, color_format)?;
        // Both written from `draw`, so they cannot grow; sized for their
        // budgets up front.
        let leash_ring = BufRing::with_capacity(
            gpu,
            "leash-ribbon",
            vk::BufferUsageFlags::VERTEX_BUFFER,
            (LEASH_MAX_VERTS * std::mem::size_of::<crate::leash::LeashVertex>()) as u64,
        )?;
        let select_ring = BufRing::with_capacity(
            gpu,
            "selection-lines",
            vk::BufferUsageFlags::VERTEX_BUFFER,
            SELECTION_MAX_BOXES as u64 * 72 * LINE_VERTEX_STRIDE,
        )?;
        Ok((
            SelectOutline {
                layout: select_layout,
                pipeline: select_pipeline,
                gamma_pipeline: select_gamma_pipeline,
                ring: select_ring,
                boxes: Vec::new(),
            },
            LeashRibbon {
                layout: leash_layout,
                pipeline: leash_pipeline,
                ring: leash_ring,
                warned: false,
                verts: Vec::new(),
            },
        ))
    }
}

impl SelectOutline {
    pub(super) fn destroy_ring(&mut self, gpu: &mut Gpu) {
        self.ring.destroy(gpu);
    }

    pub(super) fn destroy_objects(&self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            if let Some(p) = self.gamma_pipeline {
                device.destroy_pipeline(p, None);
            }
            device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

impl LeashRibbon {
    pub(super) fn destroy_ring(&mut self, gpu: &mut Gpu) {
        self.ring.destroy(gpu);
    }

    pub(super) fn destroy_objects(&self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

impl WorldRenderer {
    /// Set the targeted block for the selection outline (`None` = hidden).
    pub fn set_selection(&mut self, block: Option<[i32; 3]>) {
        self.scene.selection = block;
    }

    /// The selected block's outline shape (`BakedAssets::outline`, block-local
    /// boxes); empty draws the unit cube.
    pub fn set_selection_boxes(&mut self, boxes: &[[f32; 6]]) {
        self.select_outline.boxes.clear();
        self.select_outline.boxes.extend_from_slice(boxes);
    }

    /// Replace this frame's leash ribbons. `verts` is a TRIANGLE_LIST already
    /// folded (position + linear colour) by `leash::build_ribbon`, concatenated
    /// across every visible leash. Truncated to the vertex budget.
    pub fn set_leash(&mut self, verts: &[crate::leash::LeashVertex]) {
        self.leash_ribbon.verts.clear();
        // Whole triangles only, so a truncated ribbon never draws a torn one.
        let n = verts.len().min(LEASH_MAX_VERTS) / 3 * 3;
        if n < verts.len() && !self.leash_ribbon.warned {
            self.leash_ribbon.warned = true;
            log::warn!(
                "world: {} leash vertices exceed the {LEASH_MAX_VERTS}-vertex budget — truncating",
                verts.len()
            );
        }
        self.leash_ribbon.verts.extend_from_slice(&verts[..n]);
    }

    /// This frame's leash-ribbon vertex count — the render-check's r60
    /// observable (a leash gathered by `collect_leashes` reached the pass).
    pub fn leash_vert_count(&self) -> usize {
        self.leash_ribbon.verts.len()
    }

    /// Draw this frame's leash ribbons, if any. Depth-tested against the
    /// terrain and entities (GREATER, reversed-Z, no write) so a rope is
    /// occluded by blocks and closer mobs but composites over the sky.
    pub(super) fn draw_leash(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let count = self.leash_ribbon.verts.len() as u32;
        if count < 3 {
            return;
        }
        let stride = std::mem::size_of::<crate::leash::LeashVertex>();
        let bytes = self
            .leash_ribbon
            .ring
            .write_fixed(bytemuck::cast_slice(&self.leash_ribbon.verts), stride * 3);
        let count = (bytes / stride) as u32;
        let Some(vbuf) = self.leash_ribbon.ring.bind() else {
            return;
        };
        let push = LeashPush { view_proj };
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.leash_ribbon.pipeline,
            );
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
            device.cmd_push_constants(
                cb,
                self.leash_ribbon.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                std::slice::from_raw_parts(
                    (&push as *const LeashPush) as *const u8,
                    std::mem::size_of::<LeashPush>(),
                ),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf], &[0]);
            device.cmd_draw(cb, count, 1, 0, 0);
        }
    }

    /// The block-selection outline (`LevelRenderer.submitBlockOutline`): the
    /// selected block's outline shape, black at alpha 102/255
    /// (`ARGB.black(102)`), `getAppropriateLineWidth()` pixels wide, pulled
    /// toward the camera (`VIEW_OFFSET_Z_LAYERING`), depth-tested against the
    /// terrain, blended in gamma space when a UNORM view exists.
    pub(super) fn draw_selection(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let Some(b) = self.scene.selection else {
            return;
        };
        let verts = selection_vertices(b, &self.select_outline.boxes, self.scene.camera_eye);
        let bytes: &[u8] = bytemuck::cast_slice(&verts);
        let kept = self
            .select_outline
            .ring
            .write_fixed(bytes, 6 * LINE_VERTEX_STRIDE as usize);
        let count = (kept / LINE_VERTEX_STRIDE as usize) as u32;
        if count == 0 {
            return;
        }
        let push = LinePush {
            view_proj,
            color: [0.0, 0.0, 0.0, 102.0 / 255.0],
            screen: [
                extent.width as f32,
                extent.height as f32,
                outline_line_width(extent.width),
                0.0,
            ],
        };
        let record = |pipeline: vk::Pipeline| {
            let Some(vbuf) = self.select_outline.ring.bind() else {
                return;
            };
            let device = &gpu.device;
            unsafe {
                device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
                let viewport = vk::Viewport::default()
                    .y(extent.height as f32)
                    .width(extent.width as f32)
                    .height(-(extent.height as f32))
                    .max_depth(1.0);
                device.cmd_set_viewport(cb, 0, &[viewport]);
                device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
                device.cmd_push_constants(
                    cb,
                    self.select_outline.layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    std::slice::from_raw_parts(
                        (&push as *const LinePush) as *const u8,
                        std::mem::size_of::<LinePush>(),
                    ),
                );
                device.cmd_bind_vertex_buffers(cb, 0, &[vbuf], &[0]);
                device.cmd_draw(cb, count, 1, 0, 0);
            }
        };
        match (self.select_outline.gamma_pipeline, self.gamma.is_some()) {
            (Some(p), true) => self.in_gamma_space(gpu, cb, extent, || record(p)),
            _ => record(self.select_outline.pipeline),
        }
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    /// `getAppropriateLineWidth`: 2.5 px up to a 1920-wide window, then
    /// proportional.
    #[test]
    fn the_line_width_is_the_windows() {
        assert_eq!(outline_line_width(1280), 2.5);
        assert_eq!(outline_line_width(1920), 2.5);
        assert_eq!(outline_line_width(3840), 5.0);
    }

    /// Twelve edges of six vertices per box, each a quad whose two ends name
    /// each other, pulled toward the eye by `1 - 1/4096`; no boxes is the cube.
    #[test]
    fn every_box_edge_is_a_quad_pulled_toward_the_camera() {
        let eye = [0.5, 10.0, 0.5];
        let v = selection_vertices([0, 0, 0], &[], eye);
        assert_eq!(v.len(), 72);
        // The first edge runs (0,0,0) -> (1,0,0).
        let pulled = |p: [f32; 3]| -> [f32; 3] {
            std::array::from_fn(|i| eye[i] + (p[i] - eye[i]) * VIEW_OFFSET_Z)
        };
        assert_eq!(v[0].pos, pulled([0.0, 0.0, 0.0]));
        assert_eq!(v[0].other, pulled([1.0, 0.0, 0.0]));
        assert_eq!(v[2].pos, v[0].other);
        assert_eq!(v[2].other, v[0].pos);
        assert_eq!(
            v[..6].iter().map(|x| x.side).collect::<Vec<_>>(),
            [1.0, -1.0, 1.0, 1.0, -1.0, -1.0]
        );
        // A slab's outline: one box, half height, at its own block.
        let slab = selection_vertices([3, 4, 5], &[[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]], eye);
        assert_eq!(slab.len(), 72);
        let top = slab.iter().map(|x| x.pos[1]).fold(f32::MIN, f32::max);
        assert!((top - pulled([0.0, 4.5, 0.0])[1]).abs() < 1e-5);
        // Two boxes, two sets of edges.
        let two = [[0.0; 6], [0.0, 0.5, 0.0, 1.0, 1.0, 0.5]];
        assert_eq!(selection_vertices([0, 0, 0], &two, eye).len(), 144);
    }
}
