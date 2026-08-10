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

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
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
    /// M79's XP gauge, 182×5 each.
    pub experience_bar_background: HudSpriteData<'a>,
    pub experience_bar_progress: HudSpriteData<'a>,
}

/// This frame's M79 gauges, as the renderer needs them.
///
/// Resolved by the caller from [`rewo_net::hud_state`] because the two halves
/// live in different crates: the *state* is a decoded packet and the
/// *geometry* is here.
#[derive(Clone, Copy, Debug, Default)]
pub struct HudGauges {
    /// `player.experienceProgress`, 0..1. `None` when the XP bar is not the
    /// contextual bar this frame (`gameMode.hasExperience()` is false), which
    /// draws neither the bar nor the level number.
    pub experience: Option<f32>,
    /// `getXpNeededForNextLevel()`. `ExperienceBar.extractBackground` draws
    /// **nothing at all** when this is `<= 0`.
    pub xp_needed: i32,
    /// `ItemCooldowns.getCooldownPercent` per hotbar slot, 0..1.
    pub cooldowns: [f32; 9],
}

/// One `graphics.fill` the HUD pass draws, in **GUI pixels**.
///
/// Named for what it is rather than for its first caller: M109 added it for the
/// chat rows' backdrops, M110's input bar joined it, and M111's scrollbar needs
/// a **colour** as well as an alpha — which is what turned a "chat backdrop"
/// into a tinted fill.
///
/// `ChatComponent.extractRenderState` draws these before any text:
///
/// ```java
/// int entryBottom = chatBottom - lineIndex * entryHeight;
/// int entryTop    = entryBottom - entryHeight;
/// graphics.fill(-4, entryTop, maxWidth + 4 + 4, entryBottom, ARGB.black(alpha * backgroundOpacity));
/// ```
///
/// The rect is **asymmetric about the text**: the pose is translated by
/// `MESSAGE_INDENT` (4) before the fill, so `-4` lands at absolute 0 — four
/// pixels of padding to the left of the text — while `maxWidth + 4 + 4` lands
/// at `maxWidth + 12`, eight past where a full-width line can reach. Centring
/// it would be the tidy reading and is not vanilla's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudFill {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub alpha: f32,
    /// The fill's colour in **LINEAR** space, because the atlas is an SRGB
    /// image and `texture()` has already decoded by the time the vertex tint
    /// multiplies in. A caller holding an sRGB byte must convert — black is 0
    /// in both and hid this for two milestones; `0x3333AA` does not.
    pub rgb: [f32; 3],
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
    /// Multiplied into the sampled texel (M109).
    ///
    /// `[1.0; 4]` for every blit that existed before the chat backdrop, which
    /// is an exact no-op: the atlas is an SRGB image, so `texture()` already
    /// returns linear and a multiply by one changes nothing. The channel
    /// exists because a *varying* alpha cannot come from a baked texel — the
    /// cooldown overlay's does, which is why that one needed no colour and the
    /// chat's fade does.
    color: [f32; 4],
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
    xp_background: Rect,
    xp_progress: Rect,
    /// A synthesised patch of `Integer.MAX_VALUE` — see [`COOLDOWN_OVERLAY_ARGB`].
    /// The HUD pipeline has no per-vertex colour, so the overlay's exact
    /// half-transparent white is baked into the atlas as texels instead.
    cooldown_fill: Rect,
    /// An opaque white texel, for a tinted solid fill (M109).
    white_fill: Rect,
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
        // Row 0: hotbar | crosshair | selection. Row 1: hearts + food (9px),
        // then M79's two 182×5 XP strips clear of them at x = 64. Row 2 (y=48)
        // is the synthesised cooldown fill.
        let hotbar = place(&mut atlas, &sprites.hotbar, 0, 0);
        let crosshair = place(&mut atlas, &sprites.crosshair, 184, 0);
        let selection = place(&mut atlas, &sprites.selection, 200, 0);
        let heart_full = place(&mut atlas, &sprites.heart_full, 0, 32);
        let heart_half = place(&mut atlas, &sprites.heart_half, 10, 32);
        let heart_container = place(&mut atlas, &sprites.heart_container, 20, 32);
        let food_full = place(&mut atlas, &sprites.food_full, 30, 32);
        let food_half = place(&mut atlas, &sprites.food_half, 40, 32);
        let food_empty = place(&mut atlas, &sprites.food_empty, 50, 32);
        let xp_background = place(&mut atlas, &sprites.experience_bar_background, 64, 32);
        let xp_progress = place(&mut atlas, &sprites.experience_bar_progress, 64, 40);
        // `GuiGraphicsExtractor.itemCooldown` fills with `Integer.MAX_VALUE`,
        // which as ARGB is a half-transparent white. Nothing in the jar carries
        // that texel, so it is written here — 4×4 so nearest sampling has room
        // and the exact bytes are `FF FF FF 7F` rather than anything derived.
        let fill_x = 0u32;
        let fill_y = 48u32;
        for row in 0..COOLDOWN_FILL_PX {
            for col in 0..COOLDOWN_FILL_PX {
                let d = (((fill_y + row) * ATLAS_W + fill_x + col) * 4) as usize;
                atlas[d] = ((COOLDOWN_OVERLAY_ARGB >> 16) & 0xFF) as u8;
                atlas[d + 1] = ((COOLDOWN_OVERLAY_ARGB >> 8) & 0xFF) as u8;
                atlas[d + 2] = (COOLDOWN_OVERLAY_ARGB & 0xFF) as u8;
                atlas[d + 3] = ((COOLDOWN_OVERLAY_ARGB >> 24) & 0xFF) as u8;
            }
        }
        // M109: an OPAQUE white patch, which the cooldown one is NOT — it is
        // `Integer.MAX_VALUE`, i.e. alpha 127, so tinting through it would
        // halve every alpha asked for. A tint needs a texel that is exactly 1
        // in all four channels or the multiply is not the caller's colour.
        let white_x = 8u32;
        let white_y = 48u32;
        for row in 0..COOLDOWN_FILL_PX {
            for col in 0..COOLDOWN_FILL_PX {
                let d = (((white_y + row) * ATLAS_W + white_x + col) * 4) as usize;
                atlas[d..d + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
            }
        }
        let white_fill = Rect {
            u0: (white_x as f32 + 1.0) / ATLAS_W as f32,
            v0: (white_y as f32 + 1.0) / ATLAS_H as f32,
            u1: (white_x as f32 + 3.0) / ATLAS_W as f32,
            v1: (white_y as f32 + 3.0) / ATLAS_H as f32,
            w: 1.0,
            h: 1.0,
        };
        // Sample the middle of the patch so nearest filtering cannot pick up a
        // neighbouring (transparent) texel at any scale.
        let cooldown_fill = Rect {
            u0: (fill_x as f32 + 1.0) / ATLAS_W as f32,
            v0: (fill_y as f32 + 1.0) / ATLAS_H as f32,
            u1: (fill_x as f32 + 3.0) / ATLAS_W as f32,
            v1: (fill_y as f32 + 3.0) / ATLAS_H as f32,
            w: 1.0,
            h: 1.0,
        };

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
                xp_background,
                xp_progress,
                cooldown_fill,
                white_fill,
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
        gauges: HudGauges,
        chat: &[HudFill],
    ) {
        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        // Auto GUI scale (vanilla: largest integer fitting a ~320×240 base).
        // `gui_scale`, not a fourth copy of its body: this line WAS a copy, and
        // the drift its doc warns about is exactly what M135 found — the chat
        // fills were built against a caller's own copy and landed off-screen.
        let scale = gui_scale(w, h);
        let (sw, sh) = (w / scale, h / scale);

        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(256);
        // A quad whose *pixel* size comes from the caller rather than the
        // sprite. `blitSprite(…, 182, 5, 0, 0, left, top, progress, 5)` is a
        // sub-rectangle blit, and the cooldown fill is a solid rect with no
        // sprite size at all, so both need this rather than `quad`.
        let mut tinted_quad =
            |x: f32, y: f32, qw: f32, qh: f32, r: &Rect, uw: f32, color: [f32; 4]| {
                if qw <= 0.0 || qh <= 0.0 {
                    return;
                }
                let (px, py, pw, ph) = (x * scale, y * scale, qw * scale, qh * scale);
                let u1 = r.u0 + (r.u1 - r.u0) * uw;
                let corners = [
                    ([px, py], [r.u0, r.v0]),
                    ([px + pw, py], [u1, r.v0]),
                    ([px + pw, py + ph], [u1, r.v1]),
                    ([px, py], [r.u0, r.v0]),
                    ([px + pw, py + ph], [u1, r.v1]),
                    ([px, py + ph], [r.u0, r.v1]),
                ];
                for (pos, uv) in corners {
                    if v.len() < MAX_VERTS {
                        v.push(Vertex { pos, uv, color });
                    }
                }
            };
        // Every blit that predates the chat backdrop is untinted, and goes
        // through this so adding the parameter could not quietly change one.
        let mut sub_quad = |x: f32, y: f32, qw: f32, qh: f32, r: &Rect, uw: f32| {
            tinted_quad(x, y, qw, qh, r, uw, [1.0; 4]);
        };
        // The whole sprite at its own size: the sub-rectangle case with the
        // full UV span. One emitter, so a sprite and a sub-rectangle cannot
        // drift in how they map pixels to texels.
        macro_rules! quad {
            ($x:expr, $y:expr, $r:expr) => {{
                let r: &Rect = $r;
                sub_quad($x, $y, r.w, r.h, r, 1.0);
            }};
        }

        // Crosshair (centered).
        quad!((sw - self.crosshair.w) / 2.0, (sh - self.crosshair.h) / 2.0, &self.crosshair);

        // Hotbar (bottom-center) + selection frame over the active slot.
        let bar_x = (sw - self.hotbar.w) / 2.0;
        let bar_y = sh - self.hotbar.h;
        quad!(bar_x, bar_y, &self.hotbar);
        let sel_x = bar_x - 1.0 + slot.min(8) as f32 * 20.0;
        quad!(sel_x, bar_y - 1.0, &self.selection);

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
            quad!(health_left + j as f32 * 8.0, row_top, &self.heart_container);
            if hp > j as i32 * 2 {
                quad!(health_left + j as f32 * 8.0, row_top, heart);
            }

            let drum = if food >= (j as i32 + 1) * 2 {
                &self.food_full
            } else if food > j as i32 * 2 {
                &self.food_half
            } else {
                &self.food_empty
            };
            quad!(hunger_left + j as f32 * 8.0, row_top, &self.food_empty);
            if food > j as i32 * 2 {
                quad!(hunger_left + j as f32 * 8.0, row_top, drum);
            }
        }

        // M79's XP gauge — `ExperienceBar.extractBackground`. The bar is
        // positioned by `ContextualBar`'s own `left`/`top`, not by the hotbar,
        // and both use the **integer** GUI dimensions.
        if let Some(progress) = gauges.experience {
            if gauges.xp_needed > 0 {
                let (left, top) = experience_bar_pos(sw as i32, sh as i32);
                let (left, top) = (left as f32, top as f32);
                quad!(left, top, &self.xp_background);
                let filled = experience_progress_px(progress);
                if filled > 0 {
                    // `blitSprite(pipeline, SPRITE, 182, 5, 0, 0, left, top,
                    // progress, 5)` — the left `progress` pixels of a 182-wide
                    // sprite, so the UV span is clipped by the same fraction.
                    sub_quad(
                        left,
                        top,
                        filled as f32,
                        EXPERIENCE_BAR_H as f32,
                        &self.xp_progress,
                        filled as f32 / EXPERIENCE_BAR_W as f32,
                    );
                }
            }
        }

        // `GuiGraphicsExtractor.itemCooldown` — one shrinking rect per hotbar
        // slot, over its icon. The icon rects come from the same
        // `hotbar_slot_rects` the item pass uses, in GUI pixels.
        for (i, &cooldown) in gauges.cooldowns.iter().enumerate() {
            let x = bar_x + 3.0 + i as f32 * 20.0;
            let y = bar_y + 3.0;
            if let Some((top, bottom)) = cooldown_overlay_offsets(cooldown) {
                sub_quad(
                    x,
                    y + top as f32,
                    ICON_PX as f32,
                    (bottom - top) as f32,
                    &self.cooldown_fill,
                    1.0,
                );
            }
        }

        // The chat backdrops go LAST (M109), and the order is transcribed
        // rather than chosen. `Hud.extractRenderState` calls
        // `extractHotbarAndDecorations` at line 225 and `extractChat` at 236,
        // and `extractChat` opens with its own `graphics.nextStratum()` — so
        // chat is a LATER stratum than the hotbar and draws over it.
        //
        // **In the real layout they never overlap**, because `BOTTOM_MARGIN`
        // is 40 and the hotbar is 22 tall, so the chat box clears it by design.
        // The order is therefore unobservable in play and observable only to a
        // gate that deliberately puts a band over the hotbar, which is what
        // `inventoryshot`'s chat-backdrop witnesses do. An earlier cut emitted
        // the fills FIRST and justified it in a comment; the comment was
        // invented and the witness caught it.
        //
        // Rewo's chat TEXT is a separate pass drawn after this one, so it lands
        // on top of these either way — that half is a property of the render
        // graph rather than of vanilla's ordering.
        for b in chat {
            tinted_quad(
                b.x,
                b.y,
                b.w,
                b.h,
                &self.white_fill,
                1.0,
                [b.rgb[0], b.rgb[1], b.rgb[2], b.alpha],
            );
        }

        v.rotate_right(0);
        v.rotate_right(0);
        self.verts = v.len() as u32;
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] =
                // `VERTEX_STRIDE`, not a literal. A hardcoded 16 beside a
                // named stride is exactly what M21 found silently uploading
                // 36 of every 52 bytes in the entity pass, and this line was
                // that shape until the vertex grew.
                unsafe {
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

// ── The title overlay and two gauges (M79) ────────────────────────────────
//
// `Hud.extractTitle`, `Hud.extractOverlayMessage`,
// `ExperienceBar.extractBackground`, `ContextualBar.extractExperienceLevel`
// and `GuiGraphicsExtractor.itemCooldown`. Every constant below is a literal
// from one of those five methods; nothing here is derived from a screenshot.

/// `graphics.pose().scale(4.0F, 4.0F)` around the title.
pub const TITLE_SCALE: i32 = 4;
/// `graphics.pose().scale(2.0F, 2.0F)` around the subtitle.
pub const SUBTITLE_SCALE: i32 = 2;
/// The title's y inside the 4×-scaled space: `textWithBackdrop(…, -10, …)`.
pub const TITLE_Y: i32 = -10;
/// The subtitle's y inside the 2×-scaled space: `textWithBackdrop(…, 5, …)`.
pub const SUBTITLE_Y: i32 = 5;
/// `translate(guiWidth / 2, guiHeight - 68)` for the action bar.
pub const ACTION_BAR_BOTTOM: i32 = 68;
/// The action bar's y inside that translate: `textWithBackdrop(…, -4, …)`.
pub const ACTION_BAR_Y: i32 = -4;
/// `int alpha = (int)(t * 255.0F / 20.0F)` — a hard constant, **not** the
/// title's `fadeOut`. An action bar always fades over its last twenty ticks
/// whatever `set_titles_animation` last said.
pub const ACTION_BAR_FADE_TICKS: f32 = 20.0;

/// `ContextualBar.WIDTH`.
pub const EXPERIENCE_BAR_W: i32 = 182;
/// `ContextualBar.HEIGHT`.
pub const EXPERIENCE_BAR_H: i32 = 5;
/// `ContextualBar.MARGIN_BOTTOM`.
pub const CONTEXTUAL_BAR_MARGIN_BOTTOM: i32 = 24;
/// `int progress = (int)(player.experienceProgress * **183.0F**)`.
///
/// One more than the 182-wide background it is drawn over, so a full bar
/// overhangs its own frame by a pixel. Transcribed rather than tidied: the
/// overhang is what vanilla renders.
pub const EXPERIENCE_PROGRESS_SPAN: f32 = 183.0;
/// `extractExperienceLevel`'s green — `-8323296`, i.e. `0xFF80FF20`.
pub const EXPERIENCE_LEVEL_COLOR: u32 = 0xFF80_FF20;
/// Its four-way outline — `-16777216`, i.e. opaque black. Drawn at ±1 on each
/// axis **without a drop shadow**, which is why the level number needs a
/// shadow-suppressing text line where every other HUD label does not.
pub const EXPERIENCE_LEVEL_OUTLINE: u32 = 0xFF00_0000;

/// `GuiGraphicsExtractor.itemCooldown`'s fill colour — `Integer.MAX_VALUE`.
///
/// Reading that as a *colour* is the whole trick: `0x7FFFFFFF` is ARGB white
/// at alpha 127, i.e. a half-transparent white wash, not the opaque white the
/// constant's name suggests.
pub const COOLDOWN_OVERLAY_ARGB: u32 = 0x7FFF_FFFF;
/// Edge of the synthesised atlas patch that carries that exact texel.
const COOLDOWN_FILL_PX: u32 = 4;
/// A hotbar icon is 16 GUI px, and `itemCooldown`'s arithmetic is all against
/// that literal.
pub const ICON_PX: i32 = 16;

/// `Hud.extractTitle`'s alpha ramp.
///
/// ```java
/// float t = this.titleTime - partial;
/// int alpha = 255;
/// if (this.titleTime > this.titleFadeOutTime + this.titleStayTime) {
///    float time = this.titleFadeInTime + this.titleStayTime + this.titleFadeOutTime - t;
///    alpha = (int)(time * 255.0F / this.titleFadeInTime);
/// }
/// if (this.titleTime <= this.titleFadeOutTime) {
///    alpha = (int)(t * 255.0F / this.titleFadeOutTime);
/// }
/// alpha = Mth.clamp(alpha, 0, 255);
/// ```
///
/// Three things a plausible implementation gets wrong.
///
/// **The two branches gate on the integer `titleTime` and compute from the
/// float `t`.** Using `t` in the guard as well would flip a frame early at
/// every boundary.
///
/// **The second `if` is not an `else if`.** For any non-negative triple they
/// are mutually exclusive (`titleTime > fadeOut + stay` and
/// `titleTime <= fadeOut` need `stay < 0`), so the divisions are also
/// unreachable when their divisor is zero — `fadeIn == 0` makes the first
/// guard `titleTime > stay + fadeOut`, which the arming value cannot exceed,
/// and `fadeOut == 0` makes the second `titleTime <= 0`, which the caller's
/// own guard already excluded. That is why vanilla can divide by these
/// unguarded, and why `/title times 0 0 0` is not a crash.
///
/// **The fade-in is measured from the *start*, not from the remaining time.**
/// `time` is elapsed ticks, so it counts up while `t` counts down.
pub fn title_alpha(title_time: i32, fade_in: i32, stay: i32, fade_out: i32, partial: f32) -> i32 {
    let t = title_time as f32 - partial;
    let mut alpha = 255;
    if title_time > fade_out + stay {
        let elapsed = (fade_in + stay + fade_out) as f32 - t;
        alpha = (elapsed * 255.0 / fade_in as f32) as i32;
    }
    if title_time <= fade_out {
        alpha = (t * 255.0 / fade_out as f32) as i32;
    }
    alpha.clamp(0, 255)
}

/// `Hud.extractOverlayMessage`'s alpha ramp.
///
/// ```java
/// float t = this.overlayMessageTime - partial;
/// int alpha = (int)(t * 255.0F / 20.0F);
/// if (alpha > 255) alpha = 255;
/// ```
///
/// **Clamped on one side only.** There is no `Mth.clamp` here — the draw's own
/// `if (alpha > 0)` is what stops a negative from being used, and a caller
/// that clamped to `0..=255` instead would be right by accident, because `t`
/// cannot be negative while `overlayMessageTime > 0` gates the block.
pub fn action_bar_alpha(overlay_message_time: i32, partial: f32) -> i32 {
    let t = overlay_message_time as f32 - partial;
    let alpha = (t * 255.0 / ACTION_BAR_FADE_TICKS) as i32;
    if alpha > 255 {
        255
    } else {
        alpha
    }
}

/// The title's top-left in **GUI pixels**.
///
/// ```java
/// translate(guiWidth / 2, guiHeight / 2);
/// scale(4.0F, 4.0F);
/// textWithBackdrop(font, title, -titleWidth / 2, -10, …);
/// ```
///
/// **The halving happens before the scale**, in integer arithmetic, so an
/// odd-width title lands two GUI pixels left of where `-(width * 4) / 2`
/// would put it. Both the translate and the centring truncate toward zero.
pub fn title_pos(gui_w: i32, gui_h: i32, title_width: i32) -> (i32, i32) {
    (
        gui_w / 2 + TITLE_SCALE * (-(title_width / 2)),
        gui_h / 2 + TITLE_SCALE * TITLE_Y,
    )
}

/// The subtitle's top-left in GUI pixels — the same construction at 2×, and
/// **below** the centre rather than above it.
pub fn subtitle_pos(gui_w: i32, gui_h: i32, subtitle_width: i32) -> (i32, i32) {
    (
        gui_w / 2 + SUBTITLE_SCALE * (-(subtitle_width / 2)),
        gui_h / 2 + SUBTITLE_SCALE * SUBTITLE_Y,
    )
}

/// The action bar's top-left in GUI pixels. Unscaled — vanilla pushes no
/// `scale` around it, so it is the one of the three drawn at 1×.
pub fn action_bar_pos(gui_w: i32, gui_h: i32, width: i32) -> (i32, i32) {
    (
        gui_w / 2 + -(width / 2),
        gui_h - ACTION_BAR_BOTTOM + ACTION_BAR_Y,
    )
}

/// `ContextualBar.left` / `ContextualBar.top` — the XP bar's top-left in GUI
/// pixels.
///
/// ```java
/// default int left(final Window window) { return (window.getGuiScaledWidth() - 182) / 2; }
/// default int top(final Window window)  { return window.getGuiScaledHeight() - 24 - 5; }
/// ```
pub fn experience_bar_pos(gui_w: i32, gui_h: i32) -> (i32, i32) {
    (
        (gui_w - EXPERIENCE_BAR_W) / 2,
        gui_h - CONTEXTUAL_BAR_MARGIN_BOTTOM - EXPERIENCE_BAR_H,
    )
}

/// `int progress = (int)(player.experienceProgress * 183.0F)`.
///
/// A `float`→`int` cast in Java truncates toward zero, which `as i32` matches.
pub fn experience_progress_px(progress: f32) -> i32 {
    (progress * EXPERIENCE_PROGRESS_SPAN) as i32
}

/// `ContextualBar.extractExperienceLevel`'s placement in GUI pixels.
///
/// ```java
/// int x = (graphics.guiWidth() - font.width(str)) / 2;
/// int y = graphics.guiHeight() - 24 - 9 - 2;
/// ```
///
/// Note the `y` is **not** the bar's `top` minus something: it is its own
/// literal chain, and it lands six pixels above the bar rather than adjacent
/// to it.
pub fn experience_level_pos(gui_w: i32, gui_h: i32, str_width: i32) -> (i32, i32) {
    (
        (gui_w - str_width) / 2,
        gui_h - CONTEXTUAL_BAR_MARGIN_BOTTOM - 9 - 2,
    )
}

/// `GuiGraphicsExtractor.itemCooldown`'s rect, as `(top, bottom)` in GUI
/// pixels for an icon whose top is `y`. `None` when nothing is drawn.
///
/// ```java
/// if (cooldown > 0.0F) {
///    int top = y + Mth.floor(16.0F * (1.0F - cooldown));
///    int bottom = top + Mth.ceil(16.0F * cooldown);
///    this.fill(pipeline, x, top, x + 16, bottom, Integer.MAX_VALUE);
/// }
/// ```
///
/// **The bottom edge is pinned.** `floor(16 - s) + ceil(s)` is exactly 16 for
/// every `s`, so `bottom` is always `y + 16` and only the top moves — the
/// wash shrinks *upward off the bottom of the slot* as the cooldown runs out.
/// The obvious reading, a bar that grows from the top, is upside down.
///
/// The guard is `> 0.0F` and not `>= `: a group that has just expired draws
/// nothing rather than a zero-height rect.
pub fn cooldown_overlay_span(y: i32, cooldown: f32) -> Option<(i32, i32)> {
    cooldown_overlay_offsets(cooldown).map(|(t, b)| (y + t, y + b))
}

/// The same, as offsets from the icon's top.
///
/// The floor/ceil pair is where vanilla's integer arithmetic lives; the icon's
/// own position is not, and in Rewo it can be fractional (the GUI space is
/// `screen / scale`, which does not divide evenly at every window size). So the
/// renderer applies these offsets to its float row rather than truncating the
/// row first, which would slide the wash off the icon by up to a pixel at some
/// resolutions while leaving it exact at the ones a gate happens to pick.
pub fn cooldown_overlay_offsets(cooldown: f32) -> Option<(i32, i32)> {
    if !(cooldown > 0.0) {
        return None;
    }
    let top = (ICON_PX as f32 * (1.0 - cooldown)).floor() as i32;
    let bottom = top + (ICON_PX as f32 * cooldown).ceil() as i32;
    Some((top, bottom))
}

#[cfg(test)]
mod title_and_gauge_tests {
    use super::*;

    /// The ramp is 0 → 255 over `fadeIn`, flat through `stay`, 255 → 0 over
    /// `fadeOut`, at the defaults 10 / 70 / 20 the packet's own reset gives.
    #[test]
    fn the_title_alpha_ramps_up_holds_and_ramps_down() {
        let a = |tt| title_alpha(tt, 10, 70, 20, 0.0);
        // Armed at 100: the first tick is the *start* of the fade-in.
        assert_eq!(a(100), 0);
        assert_eq!(a(95), 127);
        assert_eq!(a(91), 229);
        // 90 is `fadeOut + stay`, so the fade-in guard stops being true.
        assert_eq!(a(90), 255);
        assert_eq!(a(21), 255);
        // …and the fade-out is the last `fadeOut` ticks.
        assert_eq!(a(20), 255);
        assert_eq!(a(10), 127);
        assert_eq!(a(1), 12);
    }

    /// MUTATION partner for the `titleTime` / `t` split: gating on the float
    /// `t` instead flips a frame early, which shows here as a non-255 alpha at
    /// the top of the hold.
    #[test]
    fn the_guards_use_the_integer_tick_and_the_maths_uses_the_partial() {
        // titleTime 90 with a partial: t = 89.4, so a `t > fadeOut + stay`
        // guard would be false and this would take the fade-in branch.
        assert_eq!(title_alpha(90, 10, 70, 20, 0.6), 255);
        // …and the partial does move the value inside a branch that is taken.
        assert_eq!(title_alpha(10, 10, 70, 20, 0.0), 127);
        assert_eq!(title_alpha(10, 10, 70, 20, 0.5), 121);
    }

    /// `/title times 0 0 0` divides by zero in both branches — except that
    /// neither branch is reachable, which is why vanilla ships it unguarded.
    #[test]
    fn a_zero_duration_title_never_reaches_either_division() {
        // Armed at 0, so `extractTitle`'s own guard excludes it entirely; the
        // only value a caller can pass is a positive one, and with 0/0/0 the
        // arming value is 0. Feed the degenerate case anyway and prove it is
        // finite rather than NaN.
        assert_eq!(title_alpha(1, 0, 0, 0, 0.0), 0);
        // fadeIn = 0, stay + fadeOut = 30: titleTime can never exceed 30, so
        // the fade-in branch (and its /0) is unreachable.
        assert_eq!(title_alpha(30, 0, 10, 20, 0.0), 255);
        // fadeOut = 0: the second guard is `titleTime <= 0`, excluded above.
        assert_eq!(title_alpha(1, 10, 70, 0, 0.0), 255);
    }

    /// MUTATION partner: an `else if` on the fade-out branch. Unreachable for
    /// any non-negative triple, so this pins the *reason* rather than the
    /// behaviour — the two guards cannot both hold.
    #[test]
    fn the_two_alpha_branches_are_mutually_exclusive() {
        for fade_in in 0..12 {
            for stay in 0..12 {
                for fade_out in 0..12 {
                    for tt in 1..=(fade_in + stay + fade_out).max(1) {
                        let fade_in_branch = tt > fade_out + stay;
                        let fade_out_branch = tt <= fade_out;
                        assert!(
                            !(fade_in_branch && fade_out_branch),
                            "{tt} {fade_in} {stay} {fade_out}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_action_bar_fades_over_its_last_twenty_ticks_only() {
        assert_eq!(action_bar_alpha(60, 0.0), 255);
        assert_eq!(action_bar_alpha(21, 0.0), 255);
        // MUTATION partner: using the title's `fadeOut` here (default 20 —
        // the same number!) is invisible until a server sends
        // `set_titles_animation` with a different fade-out. The constant is
        // what makes that impossible.
        assert_eq!(action_bar_alpha(20, 0.0), 255);
        assert_eq!(action_bar_alpha(10, 0.0), 127);
        assert_eq!(action_bar_alpha(1, 0.0), 12);
    }

    /// The halving is inside the scale, so an odd width truncates by two GUI
    /// pixels rather than by half of one.
    #[test]
    fn the_title_centres_before_it_scales() {
        // width 41 → -(41/2) = -20 → -80 from centre.
        let (x, y) = title_pos(320, 240, 41);
        assert_eq!(x, 160 - 80);
        assert_eq!(y, 120 - 40);
        // MUTATION partner: `-(41 * 4) / 2` would be -82.
        assert_ne!(x, 160 - 82);
        // An even width is where the two readings agree, which is why an odd
        // one is the sample that bites.
        assert_eq!(title_pos(320, 240, 40).0, 160 - 80);
    }

    #[test]
    fn the_subtitle_sits_below_the_centre_at_half_the_scale() {
        let (x, y) = subtitle_pos(320, 240, 40);
        assert_eq!(x, 160 - 40);
        assert_eq!(y, 120 + 10);
        // …and the title is above it, which is the ordering an inverted sign
        // would swap.
        assert!(title_pos(320, 240, 40).1 < y);
    }

    #[test]
    fn the_action_bar_sits_above_the_hotbar_at_one_times_scale() {
        let (x, y) = action_bar_pos(320, 240, 40);
        assert_eq!(x, 160 - 20);
        assert_eq!(y, 240 - 72);
        // Above the hotbar (22 tall) and below the XP bar's level number.
        assert!(y < 240 - 22);
    }

    #[test]
    fn the_experience_bar_sits_between_the_hearts_and_the_hotbar() {
        let (left, top) = experience_bar_pos(320, 240);
        assert_eq!(left, (320 - 182) / 2);
        assert_eq!(top, 240 - 29);
        // The hotbar's top is `h - 22`, so the bar clears it by two pixels.
        assert!(top + EXPERIENCE_BAR_H < 240 - 22);
        // The level number **straddles** the bar rather than clearing it: its
        // 9-px row starts six pixels higher and its last three rows overlap
        // the bar's first three. My first witness here asserted
        // `level_y + 9 <= top` — that the number sits entirely above — and it
        // failed, which is the correct outcome: `y = h - 24 - 9 - 2` is its
        // own literal chain, not the bar's `top` minus a gap.
        let (_, level_y) = experience_level_pos(320, 240, 6);
        assert_eq!(level_y, 240 - 35);
        assert_eq!(top - level_y, 6);
        assert_eq!(level_y + 9 - top, 3);
    }

    /// The 183 against a 182-wide frame: a full bar is one pixel wider than
    /// what it fills.
    #[test]
    fn a_full_experience_bar_overhangs_its_frame_by_one_pixel() {
        assert_eq!(experience_progress_px(0.0), 0);
        assert_eq!(experience_progress_px(0.5), 91);
        // MUTATION partner: `* 182.0` gives 182 here.
        assert_eq!(experience_progress_px(1.0), 183);
        assert_eq!(EXPERIENCE_BAR_W, 182);
        // …and it truncates rather than rounding.
        assert_eq!(experience_progress_px(0.999), 182);
    }

    /// The wash shrinks upward off the bottom of the slot, and its bottom edge
    /// never moves.
    #[test]
    fn the_cooldown_overlay_is_pinned_at_the_bottom_of_the_slot() {
        assert_eq!(cooldown_overlay_span(100, 0.0), None);
        assert_eq!(cooldown_overlay_span(100, 1.0), Some((100, 116)));
        assert_eq!(cooldown_overlay_span(100, 0.5), Some((108, 116)));
        // MUTATION partner: `top = y` with a height of `16 * cooldown` draws
        // it growing downward from the top, which is the natural reading.
        assert_eq!(cooldown_overlay_span(100, 0.25), Some((112, 116)));
        // floor + ceil sum to exactly 16 at every fraction, so `bottom` is
        // always `y + 16` — check the awkward ones too.
        for i in 1..=100 {
            let c = i as f32 / 100.0;
            let (_, bottom) = cooldown_overlay_span(100, c).expect("drawn");
            assert_eq!(bottom, 116, "cooldown {c}");
        }
    }

    /// `Integer.MAX_VALUE` as a colour.
    #[test]
    fn the_cooldown_wash_is_half_transparent_white() {
        assert_eq!(COOLDOWN_OVERLAY_ARGB, i32::MAX as u32);
        assert_eq!((COOLDOWN_OVERLAY_ARGB >> 24) & 0xFF, 127);
        assert_eq!(COOLDOWN_OVERLAY_ARGB & 0x00FF_FFFF, 0x00FF_FFFF);
    }

    #[test]
    fn the_level_number_is_green_over_a_black_outline() {
        assert_eq!(EXPERIENCE_LEVEL_COLOR, (-8323296i32) as u32);
        assert_eq!(EXPERIENCE_LEVEL_OUTLINE, (-16777216i32) as u32);
        // The green is `0x80FF20`, not a pure green — a mis-transcription to
        // 0x00FF00 is the kind of thing nothing else would catch.
        assert_eq!(EXPERIENCE_LEVEL_COLOR & 0x00FF_FFFF, 0x0080_FF20);
    }
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
