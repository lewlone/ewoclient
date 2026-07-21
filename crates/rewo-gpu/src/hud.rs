//! In-game HUD — a 2D screen-space overlay: crosshair, hotbar + selection
//! frame, health hearts, hunger drumsticks. Vanilla-look, from the jar's
//! `gui/sprites/hud/` sprites (REWO_PLAN §7's HUD pass).
//!
//! One combined sprite atlas + one alpha-blended pipeline. Geometry is a
//! CPU-built quad list rebuilt each frame at the auto GUI scale (the
//! element count is tiny). Drawn last in `WorldRenderer::draw`, over the
//! world/entities, with its own positive viewport (screen-pixel coords,
//! top-left origin) — no depth.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::create_texture;
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 16; // vec2 pos + vec2 uv
const MAX_VERTS: usize = 4096;
const RING: usize = 2;
const ATLAS_W: u32 = 256;
const ATLAS_H: u32 = 64;

/// Borrowed view of `rewo_data::assets::HudSprites` (keeps rewo-gpu free of
/// a rewo-data dependency — same pattern as the font/skin slices).
pub struct HudSpriteData<'a> {
    pub rgba: &'a [u8],
    pub w: u32,
    pub h: u32,
}

pub struct HudSpritesData<'a> {
    pub hotbar: HudSpriteData<'a>,
    pub selection: HudSpriteData<'a>,
    pub crosshair: HudSpriteData<'a>,
    pub heart_full: HudSpriteData<'a>,
    pub heart_half: HudSpriteData<'a>,
    pub heart_container: HudSpriteData<'a>,
    pub food_full: HudSpriteData<'a>,
    pub food_half: HudSpriteData<'a>,
    pub food_empty: HudSpriteData<'a>,
}

/// A sprite's atlas placement: normalized UV rect + pixel size.
#[derive(Clone, Copy, Default)]
struct Rect {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    w: f32,
    h: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

pub struct HudPass {
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
    // Atlas placements.
    hotbar: Rect,
    selection: Rect,
    crosshair: Rect,
    heart_full: Rect,
    heart_half: Rect,
    heart_container: Rect,
    food_full: Rect,
    food_half: Rect,
    food_empty: Rect,
}

impl HudPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        sprites: &HudSpritesData<'_>,
    ) -> Result<Self, String> {
        // Pack every sprite into one atlas at a fixed layout; record UVs.
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let mut place = |dst: &mut [u8], s: &HudSpriteData<'_>, x: u32, y: u32| -> Rect {
            for row in 0..s.h {
                let src = (row * s.w * 4) as usize;
                let d = (((y + row) * ATLAS_W + x) * 4) as usize;
                dst[d..d + (s.w * 4) as usize]
                    .copy_from_slice(&s.rgba[src..src + (s.w * 4) as usize]);
            }
            Rect {
                u0: x as f32 / ATLAS_W as f32,
                v0: y as f32 / ATLAS_H as f32,
                u1: (x + s.w) as f32 / ATLAS_W as f32,
                v1: (y + s.h) as f32 / ATLAS_H as f32,
                w: s.w as f32,
                h: s.h as f32,
            }
        };
        // Row 0: hotbar | crosshair | selection. Row 1: hearts + food (9px).
        let hotbar = place(&mut atlas, &sprites.hotbar, 0, 0);
        let crosshair = place(&mut atlas, &sprites.crosshair, 184, 0);
        let selection = place(&mut atlas, &sprites.selection, 200, 0);
        let heart_full = place(&mut atlas, &sprites.heart_full, 0, 32);
        let heart_half = place(&mut atlas, &sprites.heart_half, 10, 32);
        let heart_container = place(&mut atlas, &sprites.heart_container, 20, 32);
        let food_full = place(&mut atlas, &sprites.food_full, 30, 32);
        let food_half = place(&mut atlas, &sprites.food_half, 40, 32);
        let food_empty = place(&mut atlas, &sprites.food_empty, 50, 32);

        let (image, image_alloc, view) = create_texture(gpu, &atlas, ATLAS_W, ATLAS_H)?;
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
                .map_err(|e| format!("hud sampler: {e}"))?;
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
                .map_err(|e| format!("hud set layout: {e}"))?;
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
                .map_err(|e| format!("hud pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("hud set: {e}"))?[0];
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
                .map_err(|e| format!("hud layout: {e}"))?;
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
                    .map_err(|e| format!("hud vbuf: {e}"))?;
                let req = device.get_buffer_memory_requirements(buffer);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: "hud-verts",
                        requirements: req,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("hud vbuf alloc: {e}"))?;
                device
                    .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("hud vbuf bind: {e}"))?;
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
                hotbar,
                selection,
                crosshair,
                heart_full,
                heart_half,
                heart_container,
                food_full,
                food_half,
                food_empty,
            })
        }
    }

    /// Rebuild the HUD quads for this frame's health/food/selected slot and
    /// draw them. `health`/`food` are 0..20; `slot` is 0..8.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        health: f32,
        food: i32,
        slot: u8,
    ) {
        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        // Auto GUI scale (vanilla: largest integer fitting a ~320×240 base).
        let scale = ((h / 240.0).min(w / 320.0)).floor().clamp(1.0, 4.0);
        let (sw, sh) = (w / scale, h / scale);

        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(256);
        let mut quad = |x: f32, y: f32, r: &Rect| {
            // base (scaled-space) → pixels.
            let (px, py, pw, ph) = (x * scale, y * scale, r.w * scale, r.h * scale);
            let corners = [
                ([px, py], [r.u0, r.v0]),
                ([px + pw, py], [r.u1, r.v0]),
                ([px + pw, py + ph], [r.u1, r.v1]),
                ([px, py], [r.u0, r.v0]),
                ([px + pw, py + ph], [r.u1, r.v1]),
                ([px, py + ph], [r.u0, r.v1]),
            ];
            for (pos, uv) in corners {
                if v.len() < MAX_VERTS {
                    v.push(Vertex { pos, uv });
                }
            }
        };

        // Crosshair (centered).
        quad((sw - self.crosshair.w) / 2.0, (sh - self.crosshair.h) / 2.0, &self.crosshair);

        // Hotbar (bottom-center) + selection frame over the active slot.
        let bar_x = (sw - self.hotbar.w) / 2.0;
        let bar_y = sh - self.hotbar.h;
        quad(bar_x, bar_y, &self.hotbar);
        let sel_x = bar_x - 1.0 + slot.min(8) as f32 * 20.0;
        quad(sel_x, bar_y - 1.0, &self.selection);

        // Hearts (left) + hunger (right), same fill logic, mirrored origins.
        let row_top = sh - 39.0;
        let health_left = (sw - self.hotbar.w) / 2.0;
        let hunger_left = sw / 2.0 + 10.0;
        for j in 0..10u32 {
            let hp = health.round() as i32;
            let heart = if hp >= (j as i32 + 1) * 2 {
                &self.heart_full
            } else if hp > j as i32 * 2 {
                &self.heart_half
            } else {
                &self.heart_container
            };
            quad(health_left + j as f32 * 8.0, row_top, &self.heart_container);
            if hp > j as i32 * 2 {
                quad(health_left + j as f32 * 8.0, row_top, heart);
            }

            let drum = if food >= (j as i32 + 1) * 2 {
                &self.food_full
            } else if food > j as i32 * 2 {
                &self.food_half
            } else {
                &self.food_empty
            };
            quad(hunger_left + j as f32 * 8.0, row_top, &self.food_empty);
            if food > j as i32 * 2 {
                quad(hunger_left + j as f32 * 8.0, row_top, drum);
            }
        }

        self.verts = v.len() as u32;
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 16) };
            slice[..bytes.len()].copy_from_slice(bytes);
        }
        if self.verts == 0 {
            return;
        }

        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            // Positive viewport — standard top-left screen coords.
            let viewport = vk::Viewport::default()
                .width(w)
                .height(h)
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
            include_bytes!(concat!(env!("OUT_DIR"), "/hud.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/hud.frag.spv")),
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
        // No depth test/write, but the pass carries a depth attachment — the
        // format must be declared to match (same gotcha as the sky pass).
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
            .map_err(|(_, e)| format!("hud pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}
