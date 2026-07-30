//! The screen framework's render half (M82) — a screen's backdrop gradient
//! and its buttons.
//!
//! Deliberately *only* those two. Everything else a screen draws already has a
//! pass: its text is [`crate::text`] (the vanilla bitmap font, the same one
//! the HUD and the F3 overlay use), its item icons are [`crate::gui_item`],
//! and the inventory's panel and slot highlights stay in [`crate::container`],
//! which is inventory-specific from `slot_at` to `preview_rect` and has no
//! business growing a second screen's chrome.
//!
//! # The pipeline is `container`'s, not a copy of it
//!
//! One textured quad × a per-vertex sRGB colour, blended src-over, no depth —
//! byte-for-byte what `container.vert`/`container.frag` already are. So this
//! pass builds [`crate::container::build_pipeline`] rather than shipping a
//! second GLSL pair that could drift from it. The colour is linearised in the
//! fragment shader because the attachment is an sRGB format that encodes on
//! store; the texture is sampled from an `R8G8B8A8_SRGB` image and is already
//! linear. Same discipline as every other Rewo UI pass.
//!
//! # The nine-slice, and the half of it that is still a named gap (M85)
//!
//! `widget/button.png` is 200×20 with a `nine_slice` `.mcmeta` (`border: 3`,
//! and **no `stretch_inner`**, so `GuiSpriteScaling.NineSlice.stretchInner`
//! defaults to `false` → `blitTiledSprite`). At `Button.BIG_WIDTH` the
//! nine-slice degenerates to a 1:1 blit, which is why M82 shipped one and
//! asserted that any other size was *skipped*.
//!
//! M85's server-links dialog draws **310**-wide buttons and the pause menu
//! draws 204 and 98, so that assertion came due. What is transcribed now is
//! `blitNineSlicedSprite`'s first two branches:
//!
//! * `width == sprite width && height == sprite height` — one quad.
//! * `height == sprite height` — left border, **tiled** inner, right border.
//!
//! The two branches that resize *vertically* are still not here, and a button
//! whose height is not the sheet's 20 is still skipped with a warning. That is
//! not laziness twice over: every `Button` Rewo builds is
//! `Button.DEFAULT_HEIGHT`, so the vertical branches would be untranscribed
//! *and* unexercised, which is the shape M82 declined for arrow-key navigation.
//! `deathshot`'s `p11` still asserts the skip; `serverlinkshot` asserts that a
//! 310-wide button now draws.
//!
//! **The inner segment tiles, it does not stretch.** `TiledBlitRenderState`
//! repeats the 194×20 middle across the gap and clips the last partial tile by
//! `Mth.lerp(remaining / tileWidth, u0, u1)`. Stretching instead is the
//! plausible shortcut and it is visibly wrong on a 310-wide button: the sheet's
//! centre has a vertical highlight gradient, and stretched it smears where
//! tiled it repeats.
//!
//! # The menu background (M85)
//!
//! `Screen.extractBackground`'s *other* branch — `extractMenuBackground`, a
//! 16×16 texture blitted with a declared size of `32, 32` so the on-screen tile
//! is 32 GUI px of a 2×-magnified sheet, repeating. Rewo's sampler is
//! `CLAMP_TO_EDGE` over an atlas, so the repeat is done on the CPU: one quad
//! per tile, each carrying the sprite's own UV rect. At 4× GUI scale on a 4K
//! display that is ~250 quads, which is why [`MAX_VERTS`] is what it is.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::container::{build_pipeline, Rect, Vertex};
use crate::entities::create_texture;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
/// Enough for the tiled menu background plus every button a screen can hold.
///
/// The background dominates: it is one quad per 32×32 GUI-pixel cell, so a
/// 3840×2160 display at GUI scale 4 is `ceil(960/32) * ceil(540/32)` = 30 × 17
/// = 510 quads = 3060 vertices. The buttons are noise beside that (a nine-slice
/// button is at most three quads plus its tiles). 8192 leaves headroom for a
/// GUI scale of 2 on the same display, which is 120 × 68 = 8160 — over the cap,
/// and [`push_quad`] drops the overflow rather than corrupting the buffer.
const MAX_VERTS: usize = 8192;
const RING: usize = 2;
const ATLAS_W: u32 = 256;
/// Three 20-px button rows, then the two 16×16 menu-background sheets.
const ATLAS_H: u32 = 96;

/// `Button.BIG_WIDTH` / `Button.DEFAULT_HEIGHT`, and the three sheets' own
/// size. Mirrored from `rewo_world::screen` rather than imported: `rewo-gpu`
/// holds no dependency on the world crate, the same arrangement the font, skin
/// and container slices use.
pub const SPRITE_W: i32 = 200;
pub const SPRITE_H: i32 = 20;

/// Which of `AbstractButton.SPRITES` to blit.
///
/// A mirror of `rewo_world::screen::ButtonSprite` for the crate-boundary
/// reason above. The *choice* between the three is the world crate's — it is
/// `WidgetSprites.get(active, hoveredOrFocused)` and belongs with the widget
/// model — so this type never decides, it only names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonSprite {
    Enabled,
    Disabled,
    Highlighted,
}

/// One button to draw, in **GUI space** (the app multiplies nothing; this pass
/// applies the GUI scale, exactly as [`crate::container`] does for the panel).
#[derive(Clone, Copy, Debug)]
pub struct ButtonDraw {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub sprite: ButtonSprite,
}

/// `Screen.extractMenuBackground`'s tiled texture (M85).
///
/// Mutually exclusive with [`ScreenDraw::backdrop`] in vanilla, because
/// `extractBackground`'s two branches are an if/else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuBackgroundDraw {
    /// `minecraft.level != null` — selects `INWORLD_MENU_BACKGROUND` over
    /// `MENU_BACKGROUND`.
    pub in_world: bool,
}

/// Everything a screen asks this pass for in one frame.
#[derive(Clone, Debug, Default)]
pub struct ScreenDraw {
    /// `(top, bottom)` sRGB + straight alpha, or `None` for a screen that
    /// paints no backdrop of its own. `col1` is the top — see
    /// `ColoredRectangleRenderState.buildVertices`.
    pub backdrop: Option<([f32; 4], [f32; 4])>,
    /// The other branch of `extractBackground` — see [`MenuBackgroundDraw`].
    pub menu_background: Option<MenuBackgroundDraw>,
    pub buttons: Vec<ButtonDraw>,
}

impl ScreenDraw {
    pub fn is_empty(&self) -> bool {
        self.backdrop.is_none() && self.menu_background.is_none() && self.buttons.is_empty()
    }
}

/// Borrowed view of `rewo_data::assets::WidgetSprites`.
pub struct WidgetSpriteData<'a> {
    pub button: crate::hud::HudSpriteData<'a>,
    pub button_disabled: crate::hud::HudSpriteData<'a>,
    pub button_highlighted: crate::hud::HudSpriteData<'a>,
    pub menu_background: crate::hud::HudSpriteData<'a>,
    pub inworld_menu_background: crate::hud::HudSpriteData<'a>,
}

/// `GuiSpriteScaling.NineSlice` from `widget/button.png.mcmeta`:
/// `{"type":"nine_slice","width":200,"height":20,"border":3}`.
///
/// `border` is the one-number form, so all four edges are 3. `stretch_inner` is
/// absent and its default is **false**, which is what sends the middle segments
/// through `blitTiledSprite`.
const NINE_SLICE_BORDER: i32 = 3;
/// The tile size of the menu background, in GUI pixels — the `32, 32` declared
/// size in `extractMenuBackgroundTexture`, not the 16×16 file.
const MENU_BACKGROUND_TILE: f32 = 32.0;

pub struct ScreenPass {
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
    enabled: Rect,
    disabled: Rect,
    highlighted: Rect,
    white: Rect,
    menu_bg: Rect,
    inworld_menu_bg: Rect,
}

impl ScreenPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        sprites: &WidgetSpriteData<'_>,
    ) -> Result<Self, String> {
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let place = |dst: &mut [u8], s: &crate::hud::HudSpriteData<'_>, x: u32, y: u32| {
            for row in 0..s.h.min(ATLAS_H - y) {
                let src = (row * s.w * 4) as usize;
                let d = (((y + row) * ATLAS_W + x) * 4) as usize;
                let n = (s.w.min(ATLAS_W - x) * 4) as usize;
                dst[d..d + n].copy_from_slice(&s.rgba[src..src + n]);
            }
        };
        place(&mut atlas, &sprites.button, 0, 0);
        place(&mut atlas, &sprites.button_disabled, 0, 20);
        place(&mut atlas, &sprites.button_highlighted, 0, 40);
        place(&mut atlas, &sprites.menu_background, 0, 60);
        place(&mut atlas, &sprites.inworld_menu_background, 16, 60);
        // One opaque white texel so the untextured backdrop can share this
        // pipeline — the fragment shader's `texture * color` then leaves the
        // gradient alone.
        let w = ((4 * ATLAS_W + 208) * 4) as usize;
        atlas[w..w + 4].copy_from_slice(&[255, 255, 255, 255]);

        let uv = |x: f32, y: f32, w: f32, h: f32| Rect {
            u0: x / ATLAS_W as f32,
            v0: y / ATLAS_H as f32,
            u1: (x + w) / ATLAS_W as f32,
            v1: (y + h) / ATLAS_H as f32,
        };
        let enabled = uv(0.0, 0.0, SPRITE_W as f32, SPRITE_H as f32);
        let disabled = uv(0.0, 20.0, SPRITE_W as f32, SPRITE_H as f32);
        let highlighted = uv(0.0, 40.0, SPRITE_W as f32, SPRITE_H as f32);
        // Half a texel in, so filtering cannot reach a neighbour.
        let white = uv(208.5, 4.5, 0.0, 0.0);
        let menu_bg = uv(0.0, 60.0, 16.0, 16.0);
        let inworld_menu_bg = uv(16.0, 60.0, 16.0, 16.0);

        let (image, image_alloc, view) = create_texture(gpu, &atlas, ATLAS_W, ATLAS_H)?;
        let device = gpu.device.clone();
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("screen sampler: {e}"))?
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
                .map_err(|e| format!("screen set layout: {e}"))?
        };
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("screen pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("screen set: {e}"))?[0]
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
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(8)];
        let layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push),
                    None,
                )
                .map_err(|e| format!("screen layout: {e}"))?
        };
        let pipeline = build_pipeline(&device, layout, color_format)?;

        let mut bufs = [vk::Buffer::null(); RING];
        let mut allocs: [Option<Allocation>; RING] = [None, None];
        for i in 0..RING {
            let (b, a) = new_vertex_buffer(gpu)?;
            bufs[i] = b;
            allocs[i] = Some(a);
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
            enabled,
            disabled,
            highlighted,
            white,
            menu_bg,
            inworld_menu_bg,
        })
    }

    /// Build this frame's geometry from a screen's draw list.
    pub fn set_state(&mut self, extent: vk::Extent2D, draw: &ScreenDraw) {
        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        let scale = crate::hud::gui_scale(w, h);
        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(64);

        // 1. The backdrop, over everything the world drew, in *screen* pixels
        //    so it covers the leftover strip an odd window size leaves past
        //    `floor(h / scale) * scale`.
        if let Some((top, bottom)) = draw.backdrop {
            push_quad(&mut v, 0.0, 0.0, w, h, self.white, top, bottom);
        }

        // 2. The menu background, tiled in GUI space. Drawn after the
        //    gradient because the two are an if/else in vanilla and never
        //    coexist; the order only matters if a caller sets both.
        if let Some(bg) = draw.menu_background {
            let rect = if bg.in_world {
                self.inworld_menu_bg
            } else {
                self.menu_bg
            };
            let tile = MENU_BACKGROUND_TILE * scale;
            let (mut ty, mut rows) = (0.0f32, 0usize);
            while ty < h && rows < 512 {
                let (mut tx, mut cols) = (0.0f32, 0usize);
                while tx < w && cols < 512 {
                    // The last tile in each direction runs past the edge in
                    // vanilla too — `blit` draws the whole quad and the
                    // framebuffer clips it — so no partial-UV handling is
                    // needed here.
                    push_quad(&mut v, tx, ty, tile, tile, rect, [1.0; 4], [1.0; 4]);
                    tx += tile;
                    cols += 1;
                }
                ty += tile;
                rows += 1;
            }
        }

        // 3. The buttons, in GUI space.
        for b in &draw.buttons {
            let rect = match b.sprite {
                ButtonSprite::Enabled => self.enabled,
                ButtonSprite::Disabled => self.disabled,
                ButtonSprite::Highlighted => self.highlighted,
            };
            // `blitNineSlicedSprite`'s vertical branches are not transcribed —
            // see the module docs — so a button whose height is not the
            // sheet's is skipped rather than stretched.
            if b.height != SPRITE_H {
                log::warn!(
                    "screen: {}x{} button needs blitNineSlicedSprite's vertical branches, \
                     which are not implemented — skipped",
                    b.width,
                    b.height
                );
                continue;
            }
            push_nine_slice(
                &mut v,
                b.x as f32 * scale,
                b.y as f32 * scale,
                b.width,
                b.height,
                rect,
                scale,
            );
        }

        self.verts = v.len() as u32;
        if let Some(alloc) = self.allocs[self.cursor].as_ref() {
            if let Some(ptr) = alloc.mapped_ptr() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        v.as_ptr() as *const u8,
                        ptr.as_ptr() as *mut u8,
                        std::mem::size_of_val(&v[..]),
                    );
                }
            }
        }
    }

    pub fn draw(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        if self.verts == 0 {
            return;
        }
        let device = &gpu.device;
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let viewport = vk::Viewport::default()
                .width(extent.width as f32)
                .height(extent.height as f32)
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
            let push = [extent.width as f32, extent.height as f32];
            device.cmd_push_constants(
                cb,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::cast_slice(&push),
            );
            device.cmd_bind_vertex_buffers(cb, 0, &[self.bufs[self.cursor]], &[0]);
            device.cmd_draw(cb, self.verts, 1, 0, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        let device = gpu.device.clone();
        unsafe {
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
        if let Some(a) = self.image_alloc.take() {
            let _ = gpu.allocator.free(a);
        }
        for a in self.allocs.iter_mut() {
            if let Some(a) = a.take() {
                let _ = gpu.allocator.free(a);
            }
        }
    }
}

/// One textured quad, top colour `c0` and bottom `c1`.
///
/// A near-twin of `container::push_quad` and separate from it for one reason:
/// the cap is this pass's `MAX_VERTS`, not that one's. Sharing the function
/// would mean threading a cap through every container call site to buy nothing.
fn push_quad(
    v: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    qw: f32,
    qh: f32,
    r: Rect,
    c0: [f32; 4],
    c1: [f32; 4],
) {
    let corners = [
        ([x, y], [r.u0, r.v0], c0),
        ([x + qw, y], [r.u1, r.v0], c0),
        ([x + qw, y + qh], [r.u1, r.v1], c1),
        ([x, y], [r.u0, r.v0], c0),
        ([x + qw, y + qh], [r.u1, r.v1], c1),
        ([x, y + qh], [r.u0, r.v1], c1),
    ];
    if v.len() + corners.len() > MAX_VERTS {
        return;
    }
    for (pos, uv, color) in corners {
        v.push(Vertex { pos, uv, color });
    }
}

/// `blitNineSlicedSprite`, restricted to the two branches whose height matches
/// the sheet's — see the module docs.
///
/// `rect` is the sheet's UV rectangle inside the atlas and `SPRITE_W`/`SPRITE_H`
/// its texel size, so a fraction `f` of the sheet's width is
/// `u0 + f * (u1 - u0)`.
fn push_nine_slice(
    v: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    width: i32,
    height: i32,
    rect: Rect,
    scale: f32,
) {
    let white = [1.0f32; 4];
    // `int borderLeft = Math.min(border.left(), width / 2);` — the clamp is
    // what keeps a button narrower than twice the border from drawing
    // overlapping corners. It bites at width 5 and below, which nothing here
    // reaches, and it is transcribed rather than skipped because the
    // alternative is a silent negative inner width.
    let bl = NINE_SLICE_BORDER.min(width / 2);
    let br = NINE_SLICE_BORDER.min(width / 2);
    // Texel -> UV inside the sheet's atlas rect.
    let ur = |tx: f32| rect.u0 + (tx / SPRITE_W as f32) * (rect.u1 - rect.u0);
    let sub = |tx0: f32, tx1: f32| Rect {
        u0: ur(tx0),
        v0: rect.v0,
        u1: ur(tx1),
        v1: rect.v1,
    };
    let h = height as f32 * scale;

    if width == SPRITE_W {
        // `width == nineSlice.width() && height == nineSlice.height()` — the
        // 1:1 blit M82 shipped, unchanged.
        push_quad(v, x, y, width as f32 * scale, h, rect, white, white);
        return;
    }

    // `height == nineSlice.height()`: left border, tiled inner, right border.
    push_quad(v, x, y, bl as f32 * scale, h, sub(0.0, bl as f32), white, white);
    let inner_x = x + bl as f32 * scale;
    let inner_w = (width - br - bl) as f32 * scale;
    let tile_w = (SPRITE_W - br - bl) as f32;
    push_tiled(
        v,
        inner_x,
        y,
        inner_w,
        h,
        tile_w * scale,
        sub(bl as f32, (SPRITE_W - br) as f32),
    );
    push_quad(
        v,
        x + (width - br) as f32 * scale,
        y,
        br as f32 * scale,
        h,
        sub((SPRITE_W - br) as f32, SPRITE_W as f32),
        white,
        white,
    );
}

/// `TiledBlitRenderState.buildVertices`, horizontal only (every caller here has
/// `tileHeight == height`).
///
/// The partial last tile is **clipped by UV**, not stretched:
/// `u1 = Mth.lerp(remaining / tileWidth, u0, u1)`. Stretching it instead makes a
/// 310-wide button's centre highlight visibly wider than a 204-wide one's.
fn push_tiled(v: &mut Vec<Vertex>, x: f32, y: f32, width: f32, height: f32, tile: f32, r: Rect) {
    let white = [1.0f32; 4];
    if width <= 0.0 || height <= 0.0 || tile <= 0.0 {
        return;
    }
    let mut done = 0.0f32;
    while done < width {
        let remaining = width - done;
        let (w, u1) = if tile <= remaining {
            (tile, r.u1)
        } else {
            (remaining, r.u0 + (remaining / tile) * (r.u1 - r.u0))
        };
        push_quad(v, x + done, y, w, height, Rect { u1, ..r }, white, white);
        done += tile;
    }
}

fn new_vertex_buffer(gpu: &mut Gpu) -> Result<(vk::Buffer, Allocation), String> {
    let size = VERTEX_STRIDE * MAX_VERTS as u64;
    let buf = unsafe {
        gpu.device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("screen vertex buffer: {e}"))?
    };
    let req = unsafe { gpu.device.get_buffer_memory_requirements(buf) };
    let alloc = gpu
        .allocator
        .allocate(&AllocationCreateDesc {
            name: "screen-verts",
            requirements: req,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| format!("screen vertex alloc: {e}"))?;
    unsafe {
        gpu.device
            .bind_buffer_memory(buf, alloc.memory(), alloc.offset())
            .map_err(|e| format!("screen bind: {e}"))?;
    }
    Ok((buf, alloc))
}
