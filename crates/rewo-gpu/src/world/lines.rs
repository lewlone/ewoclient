//! Line-style world overlays: the block-selection outline and the leash
//! ribbons. Both are small per-frame vertex rings drawn depth-tested over the
//! terrain.

use ash::vk;

use super::{WorldRenderer, DEPTH_FORMAT};
use crate::buf_ring::BufRing;
use crate::Gpu;

#[repr(C)]
#[derive(Clone, Copy)]
struct LinePush {
    view_proj: [[f32; 4]; 4],
    color: [f32; 4],
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
            include_bytes!(concat!(env!("OUT_DIR"), "/line.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/line.frag.spv")),
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
            .stride(std::mem::size_of::<[f32; 3]>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [vk::VertexInputAttributeDescription::default()
            .location(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0)];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::LINE_LIST);
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

/// The block-selection outline (LINE_LIST, depth-tested against terrain) and
/// the vertex ring its 24 line vertices are written to.
pub(super) struct SelectOutline {
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    ring: BufRing,
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
            24 * 12, // 24 line verts × vec3
        )?;
        Ok((
            SelectOutline {
                layout: select_layout,
                pipeline: select_pipeline,
                ring: select_ring,
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

    /// The block-selection wireframe: the 12 edges of the targeted block,
    /// slightly inflated to avoid z-fighting, depth-tested against terrain.
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
        let (x0, y0, z0) = (
            b[0] as f32 - 0.002,
            b[1] as f32 - 0.002,
            b[2] as f32 - 0.002,
        );
        let (x1, y1, z1) = (
            b[0] as f32 + 1.002,
            b[1] as f32 + 1.002,
            b[2] as f32 + 1.002,
        );
        // 8 corners → 12 edges → 24 line vertices.
        let c = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y0, z1],
            [x0, y0, z1], // bottom
            [x0, y1, z0],
            [x1, y1, z0],
            [x1, y1, z1],
            [x0, y1, z1], // top
        ];
        let edges = [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0), // bottom loop
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4), // top loop
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7), // verticals
        ];
        let mut verts = [[0f32; 3]; 24];
        for (i, &(a, bb)) in edges.iter().enumerate() {
            verts[i * 2] = c[a];
            verts[i * 2 + 1] = c[bb];
        }

        self.select_outline
            .ring
            .write_fixed(bytemuck::cast_slice(&verts), std::mem::size_of_val(&verts));
        let Some(vbuf) = self.select_outline.ring.bind() else {
            return;
        };
        let push = LinePush {
            view_proj,
            color: [0.0, 0.0, 0.0, 0.7], // black outline, like vanilla
        };
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.select_outline.pipeline,
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
                self.select_outline.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    (&push as *const LinePush) as *const u8,
                    std::mem::size_of::<LinePush>(),
                ),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf], &[0]);
            device.cmd_draw(cb, 24, 1, 0, 0);
        }
    }
}
