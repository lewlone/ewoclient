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

/// The auto GUI scale — vanilla's largest integer that fits a ~320×240 base.
/// Shared so the icons cannot drift from the frame by recomputing it slightly
/// differently.
pub fn gui_scale(w: f32, h: f32) -> f32 {
    ((h / 240.0).min(w / 320.0)).floor().clamp(1.0, 4.0)
}

/// Where the nine hotbar item icons go, in **screen pixels** (M34).
///
/// Vanilla's hotbar sprite is 182×22 with 20-pixel slots starting 3 px in and
/// a 16×16 icon in each: `3 + i*20` from the bar's left, `3` from its top, all
/// in scaled space.
pub fn hotbar_slot_rects(bar_w: f32, bar_h: f32, w: f32, h: f32) -> [(f32, f32, f32); 9] {
    let scale = gui_scale(w, h);
    let (sw, sh) = (w / scale, h / scale);
    let bar_x = (sw - bar_w) / 2.0;
    let bar_y = sh - bar_h;
    std::array::from_fn(|i| {
        (
            (bar_x + 3.0 + i as f32 * 20.0) * scale,
            (bar_y + 3.0) * scale,
            16.0 * scale,
        )
    })
}

impl HudPass {
    /// [`hotbar_slot_rects`] against this HUD's own hotbar sprite. The HUD owns
    /// the scale, so this has to come from here rather than being recomputed by
    /// the caller.
    pub fn hotbar_slots(&self, w: f32, h: f32) -> [(f32, f32, f32); 9] {
        hotbar_slot_rects(self.hotbar.w, self.hotbar.h, w, h)
    }
}

// ── The held-item name (M66) ──────────────────────────────────────────────
//
// `Hud.extractSelectedItemName` — the label that fades in over the hotbar when
// you change what you are holding. Two clocks and a placement rule, none of
// which is guessable from a screenshot.

/// `Hud.tick`'s reset: `(int)(40.0 * options.notificationDisplayTime().get())`.
///
/// The multiplier's own default is 1.0 (`OptionInstance.IntRange(5, 100)`
/// mapped through `v / 10.0`), so the timer starts at 40 ticks — two seconds.
pub const TOOL_HIGHLIGHT_TICKS: f64 = 40.0;

/// `graphics.guiHeight() - 59` — the label's baseline row above the hotbar.
pub const SELECTED_ITEM_NAME_BOTTOM: i32 = 59;

/// `if (!this.minecraft.gameMode.canHurtPlayer()) y += 14;`
///
/// Creative and spectator draw no health bar, so the label drops into the row
/// the hearts would have used. Rewo cannot see the game mode, so this is the
/// rule and [`selected_item_name_pos`]'s caller supplies the input.
pub const SELECTED_ITEM_NAME_NO_HEALTH_SHIFT: i32 = 14;

/// `Hud.lastToolHighlight` + `toolHighlightTimer`, which vanilla keeps as two
/// fields on the HUD and ticks together.
///
/// The stack is reduced to `(item id, hover name)` because those are exactly
/// the two things the re-trigger compares:
///
/// ```java
/// if (selected.isEmpty()) {
///    this.toolHighlightTimer = 0;
/// } else if (this.lastToolHighlight.isEmpty()
///        || !selected.is(this.lastToolHighlight.getItem())
///        || !selected.getHoverName().equals(this.lastToolHighlight.getHoverName())) {
///    this.toolHighlightTimer = (int)(40.0 * options.notificationDisplayTime().get());
/// } else if (this.toolHighlightTimer > 0) {
///    this.toolHighlightTimer--;
/// }
/// this.lastToolHighlight = selected;
/// ```
///
/// **The hover-name half is the one worth naming.** Comparing item identity
/// alone is the obvious reading and it is wrong in a way a player notices:
/// renaming a sword on an anvil hands back the same item, so the label would
/// never re-show and the new name would never appear. Swapping two stacks of
/// the same item with different names is the same case.
///
/// The assignment is **unconditional** — it runs on the empty branch too, so
/// emptying your hand clears `lastToolHighlight` as well as the timer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolHighlight {
    /// `toolHighlightTimer`.
    pub timer: i32,
    /// `lastToolHighlight`, as `(item id, hover name)`. `None` is
    /// `ItemStack.EMPTY`.
    pub last: Option<(i32, String)>,
}

impl ToolHighlight {
    /// One `Hud.tick`.
    ///
    /// `selected` is `player.getInventory().getSelectedItem()` reduced to the
    /// two compared fields; `display_time` is `notificationDisplayTime`.
    pub fn tick(&mut self, selected: Option<(i32, &str)>, display_time: f64) {
        match selected {
            None => self.timer = 0,
            Some((id, name)) => {
                let changed = match &self.last {
                    None => true,
                    Some((last_id, last_name)) => *last_id != id || last_name != name,
                };
                if changed {
                    self.timer = (TOOL_HIGHLIGHT_TICKS * display_time) as i32;
                } else if self.timer > 0 {
                    self.timer -= 1;
                }
            }
        }
        self.last = selected.map(|(id, n)| (id, n.to_string()));
    }

    /// `if (this.toolHighlightTimer > 0 && !this.lastToolHighlight.isEmpty())`
    /// — what `extractSelectedItemName` renders, or nothing.
    pub fn showing(&self) -> Option<(i32, &str)> {
        if self.timer <= 0 {
            return None;
        }
        self.last.as_ref().map(|(id, n)| (*id, n.as_str()))
    }
}

/// `int alpha = (int)(timer * 256.0F / 10.0F); if (alpha > 255) alpha = 255;`
///
/// Opaque for the first thirty ticks of the default forty, then a linear fade
/// over the last ten. Fading over the whole timer instead — the natural
/// `timer / 40` reading — makes the label translucent the moment it appears.
///
/// Vanilla clamps only the top: a zero timer gives zero, and the caller's
/// `if (alpha > 0)` is what stops the draw.
pub fn tool_highlight_alpha(timer: i32) -> i32 {
    let alpha = (timer as f32 * 256.0 / 10.0) as i32;
    if alpha > 255 { 255 } else { alpha }
}

/// `extractSelectedItemName`'s placement, in **GUI pixels**.
///
/// ```java
/// int x = (graphics.guiWidth() - strWidth) / 2;
/// int y = graphics.guiHeight() - 59;
/// if (!this.minecraft.gameMode.canHurtPlayer()) y += 14;
/// ```
///
/// Integer division, so an odd-width label sits half a pixel left of centre —
/// the same truncation `centered_x` documents.
pub fn selected_item_name_pos(
    gui_w: i32,
    gui_h: i32,
    str_width: i32,
    can_hurt_player: bool,
) -> (i32, i32) {
    let x = (gui_w - str_width) / 2;
    let mut y = gui_h - SELECTED_ITEM_NAME_BOTTOM;
    if !can_hurt_player {
        y += SELECTED_ITEM_NAME_NO_HEALTH_SHIFT;
    }
    (x, y)
}

/// `GuiGraphicsExtractor.textWithBackdrop`'s fill, or `None`.
///
/// ```java
/// int backgroundColor = this.minecraft.options.getBackgroundColor(0.0F);
/// if (backgroundColor != 0) {
///    int padding = 2;
///    this.fill(textX - 2, textY - 2, textX + textWidth + 2, textY + 9 + 2, …);
/// }
/// this.text(font, str, textX, textY, textColor, true);
/// ```
///
/// **At vanilla's defaults there is no fill.** `getBackgroundColor(0.0F)` is
/// `colorFromFloat(getBackgroundOpacity(0.0F), 0, 0, 0)` and
/// `getBackgroundOpacity` returns the *fallback* while
/// `backgroundForChatOnly` is set — which it is by default — so the argument
/// `0.0F` makes the whole colour zero and the fill is skipped. It appears only
/// for a player who set "Text Background: Everywhere".
///
/// The box is asymmetric: 2 px of padding on every side of a **9** px-tall
/// text row, which is the font's line height and not the 10 the tooltip's
/// components use.
pub fn text_backdrop_rect(
    x: i32,
    y: i32,
    text_width: i32,
    background_color: u32,
) -> Option<(i32, i32, i32, i32)> {
    if background_color == 0 {
        return None;
    }
    Some((x - 2, y - 2, x + text_width + 2, y + 9 + 2))
}

#[cfg(test)]
mod selected_item_name_tests {
    use super::*;

    #[test]
    fn a_rename_re_shows_the_label_for_the_same_item() {
        let mut h = ToolHighlight::default();
        h.tick(Some((1, "Diamond Sword")), 1.0);
        assert_eq!(h.timer, 40);
        // Held still: the timer runs down.
        h.tick(Some((1, "Diamond Sword")), 1.0);
        assert_eq!(h.timer, 39);
        // MUTATION partner: comparing item identity alone leaves this at 38.
        h.tick(Some((1, "Skullcrusher")), 1.0);
        assert_eq!(h.timer, 40, "an anvil rename re-triggers it");
    }

    #[test]
    fn an_empty_hand_zeroes_it_and_clears_the_last_stack() {
        let mut h = ToolHighlight::default();
        h.tick(Some((1, "Dirt")), 1.0);
        h.tick(None, 1.0);
        assert_eq!(h.timer, 0);
        assert_eq!(h.last, None);
        assert_eq!(h.showing(), None);
        // …and picking the same item back up re-triggers, because
        // `lastToolHighlight` was assigned on the empty branch too.
        h.tick(Some((1, "Dirt")), 1.0);
        assert_eq!(h.timer, 40);
    }

    #[test]
    fn the_timer_stops_at_zero_rather_than_going_negative() {
        let mut h = ToolHighlight::default();
        h.tick(Some((1, "Dirt")), 1.0);
        for _ in 0..80 {
            h.tick(Some((1, "Dirt")), 1.0);
        }
        assert_eq!(h.timer, 0);
        assert_eq!(h.showing(), None);
        // The stack is still remembered — only the timer expired.
        assert!(h.last.is_some());
    }

    #[test]
    fn the_fade_is_the_last_ten_ticks_only() {
        assert_eq!(tool_highlight_alpha(40), 255);
        assert_eq!(tool_highlight_alpha(11), 255);
        assert_eq!(tool_highlight_alpha(10), 256_i32.min(255));
        assert_eq!(tool_highlight_alpha(9), 230);
        assert_eq!(tool_highlight_alpha(5), 128);
        assert_eq!(tool_highlight_alpha(1), 25);
        assert_eq!(tool_highlight_alpha(0), 0);
    }

    #[test]
    fn the_label_drops_fourteen_rows_when_there_is_no_health_bar() {
        let (x, y) = selected_item_name_pos(320, 240, 41, true);
        assert_eq!((x, y), ((320 - 41) / 2, 240 - 59));
        let (_, creative) = selected_item_name_pos(320, 240, 41, false);
        assert_eq!(creative, 240 - 59 + 14);
    }

    #[test]
    fn the_backdrop_is_absent_at_the_default_options() {
        assert_eq!(text_backdrop_rect(10, 20, 40, 0), None);
        assert_eq!(
            text_backdrop_rect(10, 20, 40, 0x80_00_00_00),
            Some((8, 18, 52, 31))
        );
    }
}

#[cfg(test)]
mod hotbar_slot_tests {
    use super::*;

    /// Vanilla's hotbar sprite.
    const BAR: (f32, f32) = (182.0, 22.0);

    /// Nine evenly-spaced slots that stay inside the bar, at every scale the
    /// HUD can pick. The icon row lining up with the frame is the whole point
    /// of sharing the scale.
    #[test]
    fn the_slots_are_evenly_spaced_and_fit_the_bar() {
        for (w, h) in [(854.0, 480.0), (1280.0, 720.0), (1920.0, 1080.0), (3840.0, 2160.0)] {
            let scale = gui_scale(w, h);
            let r = hotbar_slot_rects(BAR.0, BAR.1, w, h);
            let step = r[1].0 - r[0].0;
            assert!((step - 20.0 * scale).abs() < 1e-3, "{w}x{h}: step {step}");
            for i in 1..9 {
                assert!(
                    (r[i].0 - r[i - 1].0 - step).abs() < 1e-3,
                    "{w}x{h}: uneven at {i}"
                );
                assert_eq!(r[i].1, r[0].1, "one row");
                assert_eq!(r[i].2, 16.0 * scale, "16 scaled px per icon");
            }
            // The row sits inside the bar and on screen.
            let bar_left = (w / scale - BAR.0) / 2.0 * scale;
            assert!(r[0].0 >= bar_left, "{w}x{h}: first icon left of the bar");
            assert!(r[8].0 + r[8].2 <= bar_left + BAR.0 * scale, "past the bar");
            assert!(r[0].1 + r[0].2 <= h, "below the screen");
        }
    }

    /// The icons must sit on the bar, not above or below it: the bar's top is
    /// `h - 22*scale`, and a 16 px icon 3 px down leaves 3 px of frame under it.
    #[test]
    fn the_icon_row_sits_on_the_bar() {
        let (w, h) = (1280.0f32, 720.0f32);
        let scale = gui_scale(w, h);
        let r = hotbar_slot_rects(BAR.0, BAR.1, w, h);
        let bar_top = h - BAR.1 * scale;
        assert!((r[0].1 - (bar_top + 3.0 * scale)).abs() < 1e-3);
        assert!(r[0].1 + r[0].2 < h, "the icon ends above the screen edge");
    }
}
