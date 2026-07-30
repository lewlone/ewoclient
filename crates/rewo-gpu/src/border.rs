//! The world-border wall (M80) — `WorldBorderRenderer`.
//!
//! Four quads, one per wall, textured with `misc/forcefield.png` and tinted by
//! the border's [status colour][crate::border::BorderState::tint]. The state
//! that decides *whether* to draw and *what alpha* lives in
//! `rewo_world::border`; this module is the geometry and the pipeline.
//!
//! # World space, not camera space
//!
//! Vanilla emits positions relative to `(minX, 0, minZ)` and hands the pass a
//! `ModelOffset` of `(minX - cameraX, -cameraY, minZ - cameraZ)` against a
//! camera-relative model-view. Those cancel: the wall is at world
//! `(minX + px, py, minZ + pz)`. Rewo's `view_proj` already carries the camera
//! (M33's lesson — the relative form draws every wall around the world origin),
//! so [`BorderDraw::build`] emits the world-space form directly.
//!
//! The **UVs** keep their camera dependence, because vanilla's do:
//! `v0 = -frac(cameraY * 0.5)` slides the vertical texture phase as the camera
//! rises. That is not an artefact of the offset trick above; it is a separate
//! term.
//!
//! # One scoped deviation: the buffer is rebuilt every frame
//!
//! `shouldRebuildWorldBorderBuffer` keys only on the *border box*, so vanilla
//! does not rebuild when the camera moves — and the horizontal clip
//! (`max(floor(cameraX - renderDistance), borderMinX)`) is therefore stale for
//! a stationary border with a moving camera. It is invisible for any border
//! small enough to fit inside the view, which is every border you can see two
//! walls of. Rewo rebuilds sixteen vertices per frame instead of reproducing
//! the staleness.

use crate::end_sky::{upload_buffer, Buf};
use crate::Gpu;
use ash::vk;

/// One frame's border state, mirrored from `rewo_world::border::BorderRender`
/// so this crate needs no dependency on that one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderState {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
    /// `BorderStatus.getColor()`, 0x00RRGGBB.
    pub tint: u32,
    pub alpha: f64,
}

/// `DefaultVertexFormat.POSITION_TEX`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BorderVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    mvp: [[f32; 4]; 4],
    color: [f32; 4],
    tex_offset: [f32; 2],
    _pad: [f32; 2],
}

/// The four walls, in vanilla's `Direction.get2DDataValue()` order — which is
/// also the order `rebuildWorldBorderBuffer` writes the quads in, so a side's
/// index *is* its quad.
pub const SOUTH: usize = 0;
pub const WEST: usize = 1;
pub const NORTH: usize = 2;
pub const EAST: usize = 3;

/// The texture scroll's period, in milliseconds. The one wall-clock quantity in
/// the whole world-border feature.
pub const SCROLL_PERIOD_MS: u64 = 3000;

/// One frame's wall, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct BorderDraw {
    /// Sixteen vertices — four per wall, in `SOUTH, WEST, NORTH, EAST` order.
    pub verts: Vec<BorderVertex>,
    /// Linearised tint in rgb, `state.alpha` in a — the shader's
    /// `ColorModulator`.
    pub color: [f32; 4],
    pub tex_offset: [f32; 2],
    /// Which walls to draw, nearest first. **The order is load-bearing**: the
    /// pipeline writes depth, so drawing the near wall first lets it occlude
    /// the far one where they overlap on screen.
    pub sides: Vec<usize>,
}

/// `Mth.frac(double)`.
fn frac(v: f64) -> f64 {
    v - v.floor()
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c as f64 + 0.055) / 1.055).powf(2.4) as f32
    }
}

impl BorderDraw {
    /// `rebuildWorldBorderBuffer` + the uniform setup in
    /// `WorldBorderRenderer.render`.
    ///
    /// `render_distance` is in **blocks** (`renderDistanceChunks * 16`);
    /// `depth_far` is the camera's far plane, which is literally the wall's
    /// half-height — vanilla writes `float halfHeightY = (float)depthFar`.
    pub fn build(
        state: &BorderState,
        camera: [f64; 3],
        render_distance: f64,
        depth_far: f32,
        millis: u64,
    ) -> Self {
        let (camera_x, camera_y, camera_z) = (camera[0], camera[1], camera[2]);
        let half_height = depth_far;
        let (border_min_x, border_max_x) = (state.min_x, state.max_x);
        let (border_min_z, border_max_z) = (state.min_z, state.max_z);

        let min_z = (camera_z - render_distance).floor().max(border_min_z);
        let max_z = (camera_z + render_distance).ceil().min(border_max_z);
        // `(Mth.floor(minZ) & 1) * 0.5F` — a one-block parity term that keeps
        // the two-block texture repeat aligned to the world grid.
        let u0z = (min_z.floor() as i64 & 1) as f32 * 0.5;
        let u1z = (max_z - min_z) as f32 / 2.0;
        let min_x = (camera_x - render_distance).floor().max(border_min_x);
        let max_x = (camera_x + render_distance).ceil().min(border_max_x);
        let u0x = (min_x.floor() as i64 & 1) as f32 * 0.5;
        let u1x = (max_x - min_x) as f32 / 2.0;

        let v0 = -frac(camera_y * 0.5) as f32;
        let v1 = v0 + half_height;

        // The model origin is `(minX, 0, minZ)`; see the module doc for why the
        // camera does not appear.
        let ox = min_x as f32;
        let oz = min_z as f32;
        let v = |x: f32, y: f32, z: f32, u: f32, tv: f32| BorderVertex {
            pos: [ox + x, y, oz + z],
            uv: [u, tv],
        };
        let span_x = (max_x - min_x) as f32;
        let span_z = (max_z - min_z) as f32;
        let far_z = (border_max_z - min_z) as f32;
        let far_x = (border_max_x - min_x) as f32;
        let verts = vec![
            // SOUTH — the +Z wall.
            v(0.0, -half_height, far_z, u0x, v1),
            v(span_x, -half_height, far_z, u1x + u0x, v1),
            v(span_x, half_height, far_z, u1x + u0x, v0),
            v(0.0, half_height, far_z, u0x, v0),
            // WEST — the -X wall, at model x = 0.
            v(0.0, -half_height, 0.0, u0z, v1),
            v(0.0, -half_height, span_z, u1z + u0z, v1),
            v(0.0, half_height, span_z, u1z + u0z, v0),
            v(0.0, half_height, 0.0, u0z, v0),
            // NORTH — the -Z wall, at model z = 0. Wound the other way round
            // from SOUTH, which costs nothing: the pipeline does not cull.
            v(span_x, -half_height, 0.0, u0x, v1),
            v(0.0, -half_height, 0.0, u1x + u0x, v1),
            v(0.0, half_height, 0.0, u1x + u0x, v0),
            v(span_x, half_height, 0.0, u0x, v0),
            // EAST — the +X wall.
            v(far_x, -half_height, span_z, u0z, v1),
            v(far_x, -half_height, 0.0, u1z + u0z, v1),
            v(far_x, half_height, 0.0, u1z + u0z, v0),
            v(far_x, half_height, span_z, u0z, v0),
        ];

        let ch = |shift: u32| ((state.tint >> shift) & 0xFF) as f32 / 255.0;
        let color = [
            srgb_to_linear(ch(16)),
            srgb_to_linear(ch(8)),
            srgb_to_linear(ch(0)),
            state.alpha as f32,
        ];

        // `(float)(Util.getMillis() % 3000L) / 3000.0F`, applied to u and v
        // alike by vanilla's `translation(offset, offset, 0)`.
        let scroll = (millis % SCROLL_PERIOD_MS) as f32 / SCROLL_PERIOD_MS as f32;

        Self {
            verts,
            color,
            tex_offset: [scroll, scroll],
            sides: closest_sides(state, camera_x, camera_z, render_distance),
        }
    }
}

/// `WorldBorderRenderState.closestBorder` filtered by `distance <
/// renderDistance` — the walls to draw, nearest first.
///
/// The array is built NORTH, SOUTH, WEST, EAST and sorted **stably**, so an
/// exact tie (a camera dead centre in a square border) keeps that order.
pub fn closest_sides(
    state: &BorderState,
    camera_x: f64,
    camera_z: f64,
    render_distance: f64,
) -> Vec<usize> {
    let mut d = [
        (NORTH, camera_z - state.min_z),
        (SOUTH, state.max_z - camera_z),
        (WEST, camera_x - state.min_x),
        (EAST, state.max_x - camera_x),
    ];
    d.sort_by(|a, b| a.1.total_cmp(&b.1));
    d.iter()
        .filter(|(_, dist)| *dist < render_distance)
        .map(|(side, _)| *side)
        .collect()
}

/// A decoded `misc/forcefield.png`.
pub struct BorderImage<'a> {
    pub rgba: &'a [u8],
    pub w: u32,
    pub h: u32,
}

pub struct BorderPass {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    view: vk::ImageView,
    alloc: Option<gpu_allocator::vulkan::Allocation>,
    vbuf: Option<Buf>,
    ibuf: Option<Buf>,
    color: [f32; 4],
    tex_offset: [f32; 2],
    sides: Vec<usize>,
}

impl BorderPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        tex: &BorderImage,
    ) -> Result<Self, String> {
        let device = gpu.device.clone();
        let (image, alloc, view) = crate::entities::create_texture(gpu, tex.rgba, tex.w, tex.h)?;
        // REPEAT because the wall tiles the texture along its whole length, and
        // LINEAR because `forcefield.png` is a soft gradient.
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT),
                    None,
                )
                .map_err(|e| format!("border sampler: {e}"))?
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
                .map_err(|e| format!("border set layout: {e}"))?
        };
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("border pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("border set: {e}"))?[0]
        };
        let info = [vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        unsafe {
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
        }
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<Push>() as u32)];
        let layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push_range),
                    None,
                )
                .map_err(|e| format!("border layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;
        Ok(Self {
            pipeline,
            layout,
            set_layout,
            pool,
            set,
            sampler,
            image,
            view,
            alloc: Some(alloc),
            vbuf: None,
            ibuf: None,
            color: [0.0; 4],
            tex_offset: [0.0; 2],
            sides: Vec::new(),
        })
    }

    /// Upload one frame's wall. Passing `None` clears it — the frame after an
    /// `extract` that decided the wall is invisible must draw nothing, not the
    /// last visible one.
    pub fn set_draw(&mut self, gpu: &mut Gpu, draw: Option<&BorderDraw>) -> Result<(), String> {
        free_buf(gpu, self.vbuf.take());
        free_buf(gpu, self.ibuf.take());
        self.sides.clear();
        let Some(draw) = draw else {
            return Ok(());
        };
        if draw.sides.is_empty() {
            return Ok(());
        }
        self.color = draw.color;
        self.tex_offset = draw.tex_offset;
        self.sides = draw.sides.clone();
        self.vbuf = Some(upload_buffer(
            gpu,
            bytemuck::cast_slice(&draw.verts),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?);
        // Six indices per quad, the `AutoStorageIndexBuffer` vanilla shares
        // across every QUADS pipeline.
        let mut idx: Vec<u32> = Vec::with_capacity(24);
        for q in 0..4u32 {
            let b = q * 4;
            idx.extend_from_slice(&[b, b + 1, b + 2, b + 2, b + 3, b]);
        }
        self.ibuf = Some(upload_buffer(
            gpu,
            bytemuck::cast_slice(&idx),
            vk::BufferUsageFlags::INDEX_BUFFER,
        )?);
        Ok(())
    }

    pub fn draw(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let (Some(vbuf), Some(ibuf)) = (self.vbuf.as_ref(), self.ibuf.as_ref()) else {
            return;
        };
        if self.sides.is_empty() {
            return;
        }
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
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            let push = Push {
                mvp: view_proj,
                color: self.color,
                tex_offset: self.tex_offset,
                _pad: [0.0; 2],
            };
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::bytes_of(&push),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[vbuf.buffer], &[0]);
            device.cmd_bind_index_buffer(cb, ibuf.buffer, 0, vk::IndexType::UINT32);
            // One draw per visible wall, nearest first — vanilla's
            // `drawMultipleIndexed` over `closestBorder`.
            for side in &self.sides {
                device.cmd_draw_indexed(cb, 6, 1, (*side as u32) * 6, 0, 0);
            }
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        free_buf(gpu, self.vbuf.take());
        free_buf(gpu, self.ibuf.take());
        let device = gpu.device.clone();
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(a) = self.alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

fn free_buf(gpu: &mut Gpu, buf: Option<Buf>) {
    if let Some(mut b) = buf {
        unsafe { gpu.device.destroy_buffer(b.buffer, None) };
        if let Some(a) = b.alloc.take() {
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
            include_bytes!(concat!(env!("OUT_DIR"), "/border.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/border.frag.spv")),
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
            .stride(std::mem::size_of::<BorderVertex>() as u32)
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
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // `RenderPipelines.WORLD_BORDER` is `.withCull(false)` — which is why
        // the NORTH and EAST quads may be wound opposite to SOUTH and WEST
        // without disappearing. The depth bias is its
        // `DepthStencilState(…, 3.0F, 3.0F)`: scale factor then constant.
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .depth_bias_enable(true)
            .depth_bias_slope_factor(3.0)
            .depth_bias_constant_factor(3.0)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // `CompareOp.GREATER_THAN_OR_EQUAL` with depth **write on**. 26.x is
        // reversed-Z throughout, so this is directly comparable to Rewo's own
        // `GREATER`; the `_OR_EQUAL` is what lets a wall coplanar with itself
        // survive, and the write is what makes the nearest-first draw order in
        // [`BorderDraw::sides`] occlude the far wall.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);
        // `BlendFunction.OVERLAY` — `(SRC_ALPHA, ONE)` for colour and
        // `(ONE, ZERO)` for alpha. The wall *adds* light to the scene rather
        // than covering it, which is why it glows over dark terrain, and the
        // alpha channel is overwritten outright.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B
                    | vk::ColorComponentFlags::A,
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
            .map_err(|(_, e)| format!("border pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(half: f64) -> BorderState {
        BorderState {
            min_x: -half,
            max_x: half,
            min_z: -half,
            max_z: half,
            tint: 0x0020_A0FF,
            alpha: 1.0,
        }
    }

    #[test]
    fn the_quads_sit_on_the_four_walls_in_side_order() {
        let s = square(100.0);
        let d = BorderDraw::build(&s, [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        assert_eq!(d.verts.len(), 16);
        let side = |i: usize| &d.verts[i * 4..i * 4 + 4];
        for v in side(SOUTH) {
            assert_eq!(v.pos[2], 100.0, "SOUTH is the +Z wall");
        }
        for v in side(NORTH) {
            assert_eq!(v.pos[2], -100.0, "NORTH is the -Z wall");
        }
        for v in side(WEST) {
            assert_eq!(v.pos[0], -100.0, "WEST is the -X wall");
        }
        for v in side(EAST) {
            assert_eq!(v.pos[0], 100.0, "EAST is the +X wall");
        }
    }

    #[test]
    fn the_wall_is_world_space_so_moving_the_camera_leaves_it_put() {
        // The M33 lesson. Vanilla's positions are camera-relative and its
        // `ModelOffset` cancels the camera back out; if only half of that were
        // ported the wall would follow the player around.
        let s = square(100.0);
        let a = BorderDraw::build(&s, [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        let b = BorderDraw::build(&s, [40.0, 64.0, -30.0], 160.0, 512.0, 0);
        let za: Vec<f32> = a.verts[SOUTH * 4..SOUTH * 4 + 4].iter().map(|v| v.pos[2]).collect();
        let zb: Vec<f32> = b.verts[SOUTH * 4..SOUTH * 4 + 4].iter().map(|v| v.pos[2]).collect();
        assert_eq!(za, zb, "the +Z wall did not move with the camera");
        assert_eq!(za, vec![100.0; 4]);
    }

    #[test]
    fn the_wall_spans_the_far_plane_symmetrically_about_y_zero() {
        let s = square(100.0);
        let d = BorderDraw::build(&s, [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        let ys: Vec<f32> = d.verts.iter().map(|v| v.pos[1]).collect();
        assert!(ys.iter().all(|y| y.abs() == 512.0), "half-height is depth_far");
        assert!(ys.contains(&512.0) && ys.contains(&-512.0));
    }

    #[test]
    fn the_horizontal_clip_bites_only_on_a_border_wider_than_the_view() {
        // A 200-block border inside a 160-block view is drawn end to end.
        let d = BorderDraw::build(&square(100.0), [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        let xs: Vec<f32> = d.verts[SOUTH * 4..SOUTH * 4 + 4].iter().map(|v| v.pos[0]).collect();
        assert_eq!(xs[0], -100.0);
        assert_eq!(xs[1], 100.0);
        // A 20,000-block one is clipped to the camera's reach.
        let d = BorderDraw::build(&square(10_000.0), [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        let xs: Vec<f32> = d.verts[SOUTH * 4..SOUTH * 4 + 4].iter().map(|v| v.pos[0]).collect();
        assert_eq!(xs[0], -160.0);
        assert_eq!(xs[1], 160.0);
    }

    #[test]
    fn the_u_parity_term_keeps_the_two_block_repeat_on_the_world_grid() {
        // `(floor(minX) & 1) * 0.5`: the texture repeats every two blocks, so a
        // strip starting on an odd block starts half a repeat in.
        let even = BorderDraw::build(&square(100.0), [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        assert_eq!(even.verts[SOUTH * 4].uv[0], 0.0, "minX -100 is even");
        let odd = BorderDraw::build(&square(101.0), [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        assert_eq!(odd.verts[SOUTH * 4].uv[0], 0.5, "minX -101 is odd");
    }

    #[test]
    fn the_vertical_uv_phase_tracks_the_camera_height() {
        // `v0 = -frac(cameraY * 0.5)` — a separate camera term from the
        // position offset, and it survives the world-space rewrite.
        let s = square(100.0);
        let a = BorderDraw::build(&s, [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        let b = BorderDraw::build(&s, [0.0, 65.0, 0.0], 160.0, 512.0, 0);
        assert_eq!(a.verts[SOUTH * 4 + 3].uv[1], 0.0, "64 * 0.5 = 32, frac 0");
        assert_eq!(b.verts[SOUTH * 4 + 3].uv[1], -0.5, "65 * 0.5 = 32.5");
    }

    #[test]
    fn the_scroll_is_wall_clock_milliseconds_on_a_three_second_period() {
        let s = square(100.0);
        assert_eq!(
            BorderDraw::build(&s, [0.0, 64.0, 0.0], 160.0, 512.0, 0).tex_offset,
            [0.0, 0.0]
        );
        assert_eq!(
            BorderDraw::build(&s, [0.0, 64.0, 0.0], 160.0, 512.0, 1500).tex_offset,
            [0.5, 0.5]
        );
        assert_eq!(
            BorderDraw::build(&s, [0.0, 64.0, 0.0], 160.0, 512.0, 3000).tex_offset,
            [0.0, 0.0],
            "and it wraps"
        );
    }

    #[test]
    fn only_walls_within_the_view_are_drawn_and_the_nearest_goes_first() {
        // Deep in a huge border: nothing is close enough.
        let s = square(10_000.0);
        assert!(closest_sides(&s, 0.0, 0.0, 160.0).is_empty());
        // Near the north-west corner: two walls, the nearer first.
        let sides = closest_sides(&s, -9_950.0, -9_900.0, 160.0);
        assert_eq!(sides, vec![WEST, NORTH], "50 away beats 100 away");
        // A tie keeps the array's own NORTH, SOUTH, WEST, EAST order.
        let sides = closest_sides(&s, -9_900.0, -9_900.0, 160.0);
        assert_eq!(sides, vec![NORTH, WEST]);
    }

    #[test]
    fn the_tint_reaches_the_shader_linearised() {
        let mut s = square(100.0);
        s.tint = 0x00FF_3030; // SHRINKING
        s.alpha = 0.25;
        let d = BorderDraw::build(&s, [0.0, 64.0, 0.0], 160.0, 512.0, 0);
        assert_eq!(d.color[0], 1.0, "255 is 1.0 in either space");
        assert!(
            d.color[1] < 48.0 / 255.0,
            "0x30 linearises downward, got {}",
            d.color[1]
        );
        assert_eq!(d.color[1], d.color[2]);
        assert_eq!(d.color[3], 0.25, "alpha is not a colour and is not encoded");
    }
}
