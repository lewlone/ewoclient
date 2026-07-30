//! The player inventory screen — panel, backdrop and slot highlights (M35).
//!
//! The screen is four kinds of draw, and only three of them are here:
//!
//! 1. the dimmed backdrop over the world,
//! 2. `inventory.png`, the panel itself,
//! 3. the two slot-highlight sprites around whatever the cursor is over,
//! 4. the item icons and their stack counts — which are the
//!    [`crate::gui_item`] and [`crate::text`] passes, unchanged. An icon in an
//!    inventory slot is the same draw as an icon in a hotbar slot; only the
//!    rectangle differs, which is the whole reason M34's pass took a slot rect
//!    rather than a hotbar index.
//!
//! # Where the panel goes
//!
//! `AbstractContainerScreen.init` is `leftPos = (width - imageWidth) / 2` and
//! `topPos = (height - imageHeight) / 2`, in **GUI space** — the screen scaled
//! down by the same auto GUI scale the HUD uses. Integer division, so an odd
//! leftover pixel lands on the right and bottom; keeping the truncation is
//! what makes the panel sit on whole pixels and the sprite art stay crisp.
//!
//! # The backdrop
//!
//! `Screen.renderTransparentBackground` fills the whole screen with a vertical
//! gradient from `0xC0101010` to `0xD0101010` — a near-black at about 75% and
//! 82% alpha. Not a flat fill: the bottom is fractionally darker.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::create_texture;
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
/// Headroom for the worst frame this pass can build: 46 durability bars (two
/// quads each), the panel and its two highlights, the tooltip's two nine-slices,
/// and a full bundle grid's twelve cells plus its progress bar (M58).
const MAX_VERTS: usize = 2048;
const RING: usize = 2;
const ATLAS_W: u32 = 512;
const ATLAS_H: u32 = 288;

/// `AbstractContainerScreen.imageWidth` / `imageHeight` for the player
/// inventory. Mirrored from [`rewo_world::inventory`] rather than imported —
/// `rewo-gpu` deliberately holds no dependency on the world crate, the same
/// arrangement the font and skin slices use.
pub const GUI_WIDTH: f32 = 176.0;
pub const GUI_HEIGHT: f32 = 166.0;

/// `Screen.renderTransparentBackground`'s two gradient stops, as sRGB + alpha.
const BACKDROP_TOP: [f32; 4] = [16.0 / 255.0, 16.0 / 255.0, 16.0 / 255.0, 192.0 / 255.0];
const BACKDROP_BOTTOM: [f32; 4] = [16.0 / 255.0, 16.0 / 255.0, 16.0 / 255.0, 208.0 / 255.0];

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Borrowed view of `rewo_data::assets::ContainerSprites`.
pub struct ContainerSpriteData<'a> {
    pub background: crate::hud::HudSpriteData<'a>,
    pub highlight_back: crate::hud::HudSpriteData<'a>,
    pub highlight_front: crate::hud::HudSpriteData<'a>,
    /// The two 100x100 nine-slice tooltip sprites (M40).
    pub tooltip_background: crate::hud::HudSpriteData<'a>,
    pub tooltip_frame: crate::hud::HudSpriteData<'a>,
    /// `ClientBundleTooltip`'s six (M58). The three 24x24 cell sprites, then
    /// the progress bar's 12x12 border and its two 6x6 fills.
    pub bundle_slot: crate::hud::HudSpriteData<'a>,
    pub bundle_highlight_back: crate::hud::HudSpriteData<'a>,
    pub bundle_highlight_front: crate::hud::HudSpriteData<'a>,
    pub bundle_bar_border: crate::hud::HudSpriteData<'a>,
    pub bundle_bar_fill: crate::hud::HudSpriteData<'a>,
    pub bundle_bar_full: crate::hud::HudSpriteData<'a>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Rect {
    pub(crate) u0: f32,
    pub(crate) v0: f32,
    pub(crate) u1: f32,
    pub(crate) v1: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Vertex {
    pub(crate) pos: [f32; 2],
    pub(crate) uv: [f32; 2],
    pub(crate) color: [f32; 4],
}

/// The GUI scale, and where the panel's top-left corner sits in screen pixels.
///
/// Returned together because every caller needs all three: the render places
/// sprites with them, and the app maps the cursor back through them to find
/// which slot it is over.
pub fn gui_origin(w: f32, h: f32) -> (f32, f32, f32) {
    let scale = crate::hud::gui_scale(w, h);
    let (sw, sh) = (w / scale, h / scale);
    // Integer division in vanilla, and it matters: a half-pixel origin would
    // resample every sprite in the panel.
    let left = ((sw - GUI_WIDTH) / 2.0).floor();
    let top = ((sh - GUI_HEIGHT) / 2.0).floor();
    (left * scale, top * scale, scale)
}

/// Turn a cursor position in screen pixels into GUI-space coordinates relative
/// to the panel's top-left — which is what
/// `rewo_world::inventory::slot_at` expects.
pub fn screen_to_gui(mouse: (f64, f64), w: f32, h: f32) -> (f64, f64) {
    let (left, top, scale) = gui_origin(w, h);
    (
        (mouse.0 - left as f64) / scale as f64,
        (mouse.1 - top as f64) / scale as f64,
    )
}


// -- the player preview (M36) -------------------------------------------------
//
// `InventoryScreen.extractBackground` ends with
//
// ```java
// extractEntityInInventoryFollowsMouse(graphics, xo + 26, yo + 8, xo + 75, yo + 78,
//                                      30, 0.0625F, xMouse, yMouse, minecraft.player);
// ```
//
// so the viewport is GUI-space `(26, 8)..(75, 78)` inside the panel — exactly
// the black window `inventory.png` paints, which is there so the model has
// something to stand against.

/// The preview window, in GUI pixels relative to the panel's top-left.
pub const PREVIEW: (i32, i32, i32, i32) = (26, 8, 75, 78);
/// The `size` argument — the model's scale in GUI pixels per block.
pub const PREVIEW_SIZE: f32 = 30.0;
/// The `offsetY` argument, in blocks. Lifts the model so its feet sit on the
/// bottom of the window rather than its centre.
pub const PREVIEW_OFFSET_Y: f32 = 0.0625;

/// Where the preview window lands on screen, as `(x, y, w, h)` in pixels.
pub fn preview_rect(w: f32, h: f32) -> (f32, f32, f32, f32) {
    let (left, top, scale) = gui_origin(w, h);
    let (x0, y0, x1, y1) = PREVIEW;
    (
        left + x0 as f32 * scale,
        top + y0 as f32 * scale,
        (x1 - x0) as f32 * scale,
        (y1 - y0) as f32 * scale,
    )
}

/// The two angles the model turns by as the cursor moves, in **degrees**.
///
/// `xAngle = atan((centreX - mouseX) / 40)` and likewise for y, both then
/// multiplied by 20 wherever they are used. The `40` is in GUI pixels, so the
/// cursor is converted back into that space first — at GUI scale 3 a mouse a
/// third of the way across the window turns the model as far as vanilla does.
pub fn preview_angles(mouse: (f64, f64), w: f32, h: f32) -> (f32, f32) {
    let (left, top, scale) = gui_origin(w, h);
    let (x0, y0, x1, y1) = PREVIEW;
    let centre_x = (x0 + x1) as f32 / 2.0;
    let centre_y = (y0 + y1) as f32 / 2.0;
    let mx = (mouse.0 as f32 - left) / scale;
    let my = (mouse.1 as f32 - top) / scale;
    (
        ((centre_x - mx) / 40.0).atan() * 20.0,
        ((centre_y - my) / 40.0).atan() * 20.0,
    )
}

/// The preview's view-projection, mapping the model's own space straight into
/// the screen's clip space.
///
/// Vanilla renders this into an offscreen texture and blits it; Rewo draws it
/// where it belongs and skips the round trip, so the projection has to place
/// the window itself rather than assuming it fills the target. The model-view
/// is `PictureInPictureRenderer.prepare` and `GuiEntityRenderer.renderToTexture`
/// composed, in that order:
///
/// ```text
/// T(w/2, h/2, 0) · S(s, s, -s) · T(0, bbHeight/2 + offsetY, 0) · Rz(π) · Rx(yAngle)
/// ```
///
/// with `s = guiScale * size`. Two details in there are load-bearing. The
/// **negative z scale** is what turns the model to face the viewer — without
/// it you see its back. And `Rz(π)` is a half-turn about the view axis, which
/// is what puts a y-up model the right way up in a y-down GUI; it is not a
/// spare 180° that can be folded into the body rotation.
///
/// `bb_height` is the entity's bounding-box height **divided by its scale**,
/// because vanilla divides it out and then sets the render scale to 1.
pub fn preview_view_proj(
    screen_w: f32,
    screen_h: f32,
    bb_height: f32,
    y_angle_deg: f32,
) -> [[f32; 4]; 4] {
    let (rx, ry, rw, rh) = preview_rect(screen_w, screen_h);
    let (_, _, gui_scale) = gui_origin(screen_w, screen_h);
    let s = gui_scale * PREVIEW_SIZE;

    // Model → preview-local pixels, y down, origin at the window's top-left.
    let model_view = glam::Mat4::from_translation(glam::Vec3::new(rw / 2.0, rh / 2.0, 0.0))
        * glam::Mat4::from_scale(glam::Vec3::new(s, s, -s))
        * glam::Mat4::from_translation(glam::Vec3::new(
            0.0,
            bb_height / 2.0 + PREVIEW_OFFSET_Y,
            0.0,
        ))
        * glam::Mat4::from_rotation_z(std::f32::consts::PI)
        * glam::Mat4::from_rotation_x(y_angle_deg.to_radians())
        // `GuiEntityRenderer` finishes by turning the *camera* half a turn:
        // `cameraRenderState.orientation = overrideCameraAngle.conjugate().rotateY(PI)`.
        // Rewo's entity pass takes no camera state, so the same half turn is
        // applied to the model instead — and it is not optional, because
        // `bodyRot = 180 + xAngle` already points the model away from a camera
        // that has not turned. Without it you are looking at Steve's back.
        * glam::Mat4::from_rotation_y(std::f32::consts::PI);

    // Preview-local pixels → the screen's clip space. The entity pass draws
    // through a **flipped** viewport (`y = height, height = -height`), so clip
    // y is up; the `-2/h` below cancels that back to the y-down pixels above.
    //
    // Depth is reversed-Z to match the rest of Rewo — `Projection.getMatrix`
    // swaps `near` and `far` for the same reason — over vanilla's ±1000 range,
    // which is far wider than a 2-block model needs and keeps the mapping
    // obviously linear.
    let sx = 2.0 / screen_w;
    let sy = -2.0 / screen_h;
    let ox = 2.0 * rx / screen_w - 1.0;
    let oy = -(2.0 * ry / screen_h - 1.0);
    let to_clip = glam::Mat4::from_cols(
        glam::Vec4::new(sx, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, sy, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, -1.0 / 2000.0, 0.0),
        glam::Vec4::new(ox, oy, 0.5, 1.0),
    );
    (to_clip * model_view).to_cols_array_2d()
}

// -- the tooltip (M40, M52) ---------------------------------------------------
//
// `GuiGraphicsExtractor.tooltip` measures its components, asks a positioner
// where the text goes, blits two sprites behind it, then walks the list
// **twice** — all the text, then all the images. Every number below is from
// that method and the two classes it calls.
//
// The components themselves, their polymorphic width/height, the index-1
// insertion of an image and `ClientBundleTooltip`'s grid live in
// [`crate::tooltip`] (M52); what stays here is the box the two passes are
// drawn into. The split is where the crate already draws it: this module owns
// Vulkan, that one owns arithmetic.

/// `ClientTextTooltip.getHeight` — one line of tooltip text.
pub const TOOLTIP_LINE_HEIGHT: i32 = crate::tooltip::LINE_HEIGHT;
/// `TooltipRenderUtil.PADDING` (3) + `MARGIN` (9). The sprite is blitted this
/// far outside the text on every side; the padding is the visible gap and the
/// margin is the sprite's own border art.
pub const TOOLTIP_INSET: i32 = 12;
/// The nine-slice border of `tooltip/background.png.mcmeta`.
const TOOLTIP_BG_BORDER: i32 = 9;
/// …and of `tooltip/frame.png.mcmeta`, which also sets `stretch_inner`.
const TOOLTIP_FRAME_BORDER: i32 = 10;
/// Both sprites are authored at this size.
const TOOLTIP_SPRITE: i32 = 100;

// -- the bundle grid's chrome (M58) -------------------------------------------
//
// `ClientBundleTooltip.extractSlot` and `extractProgressbar` — the six
// `blitSprite` calls M52 left out. Every authored size and border below is the
// sprite's own `.mcmeta`, not a guess: all six declare `nine_slice`, and none
// of them sets `stretch_inner`.

/// `container/bundle/{slot_background, slot_highlight_back, slot_highlight_front}`
/// are all 24x24 with `border: 4` — and all three are blitted at exactly 24x24,
/// so they take `blitNineSlicedSprite`'s identity branch every time.
const BUNDLE_CELL_SPRITE: i32 = 24;
const BUNDLE_CELL_BORDER: i32 = 4;
/// `bundle_progressbar_border` is 12x12 `border: 2`, blitted at 96x13.
const BAR_BORDER_SPRITE: i32 = 12;
const BAR_BORDER_BORDER: i32 = 2;
/// The two fills are 6x6 `border: 2`, blitted at `getProgressBarFill`x13.
const BAR_FILL_SPRITE: i32 = 6;
const BAR_FILL_BORDER: i32 = 2;

/// What `draw_tooltip` paints.
///
/// The box is M40's; `bundle` is `ClientBundleTooltip.extractImage`, already
/// resolved to GUI-space geometry by [`crate::tooltip::bundle_image`] — which
/// is handed the box's own `x`, the `localY` the **image** pass reached, and
/// the tooltip's measured `w`. One struct rather than two setters, so the box
/// and the grid inside it cannot be set out of step.
#[derive(Clone, Debug, Default)]
pub struct TooltipDraw {
    /// The text block's top-left in GUI pixels, and its measured size.
    pub pos: (i32, i32),
    pub size: (i32, i32),
    pub bundle: Option<crate::tooltip::BundleImage>,
}

/// Where the text block's top-left corner goes, in GUI pixels.
///
/// `DefaultTooltipPositioner`: start at the cursor plus `(12, -12)`, then pull
/// back inside the screen. The horizontal recovery is **not** a clamp — it
/// flips the tooltip to the cursor's other side (`x - 24 - width`) and only
/// then floors at 4, so a tooltip near the right edge jumps rather than
/// squashing against it.
pub fn tooltip_position(
    screen_w: i32,
    screen_h: i32,
    mouse_x: i32,
    mouse_y: i32,
    w: i32,
    h: i32,
) -> (i32, i32) {
    let mut x = mouse_x + 12;
    let mut y = mouse_y - 12;
    if x + w > screen_w {
        x = (x - 24 - w).max(4);
    }
    // The vertical recovery *is* a clamp, and it uses `h + 3` — the text
    // height plus the bottom padding but not the margin, so a tooltip may sit
    // with its border art off the bottom of the screen. That is vanilla.
    if y + h + 3 > screen_h {
        y = screen_h - (h + 3);
    }
    (x, y)
}

/// The text block's size for a list of **text** lines, in GUI pixels.
///
/// The height starts at **-2 for a single line** and 0 otherwise
/// (`lines.size() == 1 ? -2 : 0`), and the loop that draws them adds 2 after
/// the first — so a one-line tooltip is 8 px of box for a 10 px line, and a
/// two-line one is 22. Dropping the -2 makes every single-line tooltip two
/// pixels taller than vanilla's, which is invisible until you diff a frame.
///
/// The shorthand for the common case. `lines.size() == 1` counts
/// **components**, not text lines, so a tooltip carrying an image cannot be
/// measured through here — it goes through [`crate::tooltip::measure`], which
/// this delegates to so there is one loop rather than two that can drift.
pub fn tooltip_size(line_widths: &[i32]) -> (i32, i32) {
    let components: Vec<crate::tooltip::Component> = line_widths
        .iter()
        .map(|&w| crate::tooltip::Component::text(w))
        .collect();
    crate::tooltip::measure(&components)
}

/// `blitNineSlicedSprite` — nine pieces of a `sw`x`sh` source into an
/// arbitrary destination rect.
///
/// The corners keep their natural size; the four edges and the middle fill
/// whatever is left. Vanilla either **stretches** those five
/// (`stretch_inner`) or **tiles** them, and the sprites in play disagree —
/// tooltip background `border: 9` tiles, tooltip frame `border: 10` stretches,
/// and all six of the bundle's tile. Every one of their inner slices is
/// uniform along the axis it repeats on, so a stretch reproduces a tile
/// texel-for-texel; that is a property of this version's art rather than a
/// theorem, so the gate measures it (`bc7`) instead of this comment asserting
/// it.
///
/// Vanilla's first branch is transcribed because it is the *usual* case for
/// three of these sprites: `width == nineSlice.width() && height ==
/// nineSlice.height()` blits the whole sprite once and does not slice at all,
/// which is what the bundle's three 24x24 cell sprites always hit.
#[allow(clippy::too_many_arguments)]
fn nine_slice(
    v: &mut Vec<Vertex>,
    r: Rect,
    sw: i32,
    sh: i32,
    border: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    left: f32,
    top: f32,
    scale: f32,
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let blit = |v: &mut Vec<Vertex>, r: Rect, dx: i32, dy: i32, dw: i32, dh: i32| {
        push_quad(
            v,
            left + dx as f32 * scale,
            top + dy as f32 * scale,
            dw as f32 * scale,
            dh as f32 * scale,
            r,
            WHITE,
            WHITE,
        );
    };
    if w == sw && h == sh {
        blit(v, r, x, y, w, h);
        return;
    }
    // `Math.min(border, width / 2)` — a destination narrower than two borders
    // shrinks them rather than overlapping the corners.
    let bl = border.min(w / 2).max(0);
    let bt = border.min(h / 2).max(0);
    let (du, dv) = (r.u1 - r.u0, r.v1 - r.v0);
    let (fw, fh) = (sw as f32, sh as f32);
    let cols = [
        (0.0, bl as f32, x, bl),
        (bl as f32, fw - 2.0 * bl as f32, x + bl, w - 2 * bl),
        (fw - bl as f32, bl as f32, x + w - bl, bl),
    ];
    let rows = [
        (0.0, bt as f32, y, bt),
        (bt as f32, fh - 2.0 * bt as f32, y + bt, h - 2 * bt),
        (fh - bt as f32, bt as f32, y + h - bt, bt),
    ];
    for &(su, suw, dx, dw) in &cols {
        for &(sv, svh, dy, dh) in &rows {
            if dw <= 0 || dh <= 0 {
                continue;
            }
            let piece = Rect {
                u0: r.u0 + du * (su / fw),
                v0: r.v0 + dv * (sv / fh),
                u1: r.u0 + du * ((su + suw) / fw),
                v1: r.v0 + dv * ((sv + svh) / fh),
            };
            blit(v, piece, dx, dy, dw, dh);
        }
    }
}

/// One textured quad, top colour `c0` and bottom `c1` — equal for a flat blit,
/// different for the backdrop's gradient.
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
    // All six or none: dropping a corner past the cap would leave a torn
    // triangle in the buffer rather than one fewer quad.
    if v.len() + corners.len() > MAX_VERTS {
        return;
    }
    for (pos, uv, color) in corners {
        v.push(Vertex { pos, uv, color });
    }
}

// -- the durability bar (M41) -------------------------------------------------
//
// `GuiGraphicsExtractor.itemBar`, which runs for any stack whose
// `isBarVisible()` is true — `Item.isBarVisible` is `stack.isDamaged()`, so a
// pristine tool has no bar at all rather than a full one.

/// One slot's durability bar, in **screen pixels**.
#[derive(Clone, Copy, Debug)]
pub struct ItemBar {
    /// The slot's top-left, the same origin the icon is placed from.
    pub x: f32,
    pub y: f32,
    /// Pixels per GUI pixel.
    pub scale: f32,
    /// `getBarWidth()`, already clamped to `0..=13`.
    pub width: i32,
    /// `getBarColor()` as linear-ish sRGB.
    pub color: [f32; 3],
}

/// `Item.getBarWidth` — `clamp(round(13 - damage * 13 / maxDamage), 0, 13)`.
///
/// Note it counts **down** from 13: the argument is damage, and the bar shows
/// what is left. Computing it as `13 * remaining / max` is off by a rounding
/// step in the middle of the range, which is visible as a bar that disagrees
/// with vanilla by a pixel on most of a tool's life.
pub fn bar_width(damage: i32, max_damage: i32) -> i32 {
    if max_damage <= 0 {
        return 0;
    }
    let raw = 13.0 - damage as f32 * 13.0 / max_damage as f32;
    raw.round().clamp(0.0, 13.0) as i32
}

/// `Item.getBarColor` — `Mth.hsvToRgb(healthPercentage / 3, 1, 1)`.
///
/// A third of the hue circle is red through yellow to green, which is why the
/// division is by three and not by anything to do with the bar's length.
pub fn bar_color(damage: i32, max_damage: i32) -> [f32; 3] {
    if max_damage <= 0 {
        return [1.0, 0.0, 0.0];
    }
    let health = ((max_damage - damage) as f32 / max_damage as f32).max(0.0);
    hsv_to_rgb(health / 3.0, 1.0, 1.0)
}

/// `Mth.hsvToArgb`, transcribed including its integer sector arithmetic.
fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let h = ((hue * 6.0) as i32).rem_euclid(6);
    let f = hue * 6.0 - h as f32;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - f * saturation);
    let t = value * (1.0 - (1.0 - f) * saturation);
    match h {
        0 => [value, t, p],
        1 => [q, value, p],
        2 => [p, value, t],
        3 => [p, q, value],
        4 => [t, p, value],
        _ => [value, p, q],
    }
}

/// `itemBar`'s background fill, `-16777216` — opaque black.
const BAR_BED: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

pub struct ContainerPass {
    layout: vk::PipelineLayout,
    tooltip_bg: Rect,
    tooltip_frame: Rect,
    tooltip_verts: u32,
    /// `(first vertex, count)` of this frame's durability bars.
    bar_range: (u32, u32),
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
    /// The back half's vertex count, and the total. The front highlight is the
    /// tail of the same buffer: vanilla brackets the item between the two
    /// sprites, so this pass is drawn twice per frame with the icons in
    /// between, and two ranges of one upload is simpler than two buffers.
    back_verts: u32,
    total_verts: u32,
    panel: Rect,
    highlight_back: Rect,
    highlight_front: Rect,
    white: Rect,
    /// `ClientBundleTooltip`'s six (M58).
    bundle_slot: Rect,
    bundle_hl_back: Rect,
    bundle_hl_front: Rect,
    bar_border: Rect,
    bar_fill: Rect,
    bar_full: Rect,
}

impl ContainerPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        sprites: &ContainerSpriteData<'_>,
    ) -> Result<Self, String> {
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let place = |dst: &mut [u8], s: &crate::hud::HudSpriteData<'_>, x: u32, y: u32| {
            for row in 0..s.h {
                let src = (row * s.w * 4) as usize;
                let d = (((y + row) * ATLAS_W + x) * 4) as usize;
                let n = (s.w * 4) as usize;
                dst[d..d + n].copy_from_slice(&s.rgba[src..src + n]);
            }
        };
        // The background sheet is 256×256 and only its top-left 176×166 is the
        // panel, so the UVs below crop what the blit crops.
        place(&mut atlas, &sprites.background, 0, 0);
        place(&mut atlas, &sprites.highlight_back, 256, 0);
        place(&mut atlas, &sprites.highlight_front, 280, 0);
        // The two 100x100 tooltip sprites, below the 176x166 panel.
        place(&mut atlas, &sprites.tooltip_background, 0, 166);
        place(&mut atlas, &sprites.tooltip_frame, 100, 166);
        // The bundle's six (M58), on a row of their own to the right of the
        // 256x256 background sheet, so nothing above moves.
        place(&mut atlas, &sprites.bundle_slot, 256, 32);
        place(&mut atlas, &sprites.bundle_highlight_back, 280, 32);
        place(&mut atlas, &sprites.bundle_highlight_front, 304, 32);
        place(&mut atlas, &sprites.bundle_bar_border, 328, 32);
        place(&mut atlas, &sprites.bundle_bar_fill, 340, 32);
        place(&mut atlas, &sprites.bundle_bar_full, 346, 32);
        // One opaque texel, so the untextured backdrop can share this pipeline.
        let w = (304 * 4) as usize;
        atlas[w..w + 4].copy_from_slice(&[255, 255, 255, 255]);

        let uv = |x: f32, y: f32, w: f32, h: f32| Rect {
            u0: x / ATLAS_W as f32,
            v0: y / ATLAS_H as f32,
            u1: (x + w) / ATLAS_W as f32,
            v1: (y + h) / ATLAS_H as f32,
        };
        let panel = uv(0.0, 0.0, GUI_WIDTH, GUI_HEIGHT);
        let highlight_back = uv(256.0, 0.0, 24.0, 24.0);
        let highlight_front = uv(280.0, 0.0, 24.0, 24.0);
        // Half a texel in, so bilinear filtering cannot reach a neighbour.
        let white = uv(304.5, 0.5, 0.0, 0.0);
        let tooltip_bg = uv(0.0, 166.0, 100.0, 100.0);
        let tooltip_frame = uv(100.0, 166.0, 100.0, 100.0);
        let bundle_slot = uv(256.0, 32.0, 24.0, 24.0);
        let bundle_hl_back = uv(280.0, 32.0, 24.0, 24.0);
        let bundle_hl_front = uv(304.0, 32.0, 24.0, 24.0);
        let bar_border = uv(328.0, 32.0, 12.0, 12.0);
        let bar_fill = uv(340.0, 32.0, 6.0, 6.0);
        let bar_full = uv(346.0, 32.0, 6.0, 6.0);

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
                .map_err(|e| format!("container sampler: {e}"))?
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
                .map_err(|e| format!("container set layout: {e}"))?
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
                .map_err(|e| format!("container pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("container set: {e}"))?[0]
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
                .map_err(|e| format!("container layout: {e}"))?
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
            back_verts: 0,
            total_verts: 0,
            panel,
            highlight_back,
            highlight_front,
            white,
            tooltip_bg,
            tooltip_frame,
            bundle_slot,
            bundle_hl_back,
            bundle_hl_front,
            bar_border,
            bar_fill,
            bar_full,
            tooltip_verts: 0,
            bar_range: (0, 0),
        })
    }

    /// Build this frame's geometry. `hovered` is a menu slot's GUI-space
    /// top-left, already resolved by the caller — this pass deliberately does
    /// not know the slot layout, which lives once in
    /// `rewo_world::inventory::slot_position`.
    pub fn set_state(
        &mut self,
        extent: vk::Extent2D,
        open: bool,
        hovered: Option<(i32, i32)>,
        tooltip: Option<&TooltipDraw>,
        bars: &[ItemBar],
    ) {
        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        let (left, top, scale) = gui_origin(w, h);
        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(192);
        let quad = push_quad;

        // 1. The backdrop, over everything the world drew.
        if open {
            quad(&mut v, 0.0, 0.0, w, h, self.white, BACKDROP_TOP, BACKDROP_BOTTOM);
            // 2. The panel.
            let (pw, ph) = (GUI_WIDTH * scale, GUI_HEIGHT * scale);
            quad(&mut v, left, top, pw, ph, self.panel, WHITE, WHITE);
        }
        // 3. The hovered slot's *back* highlight, at `slot - 4` and 24×24 —
        //    a four-pixel bleed on every side of the 16 px icon.
        let hl = hovered.map(|(sx, sy)| {
            (
                left + (sx - 4) as f32 * scale,
                top + (sy - 4) as f32 * scale,
                24.0 * scale,
            )
        });
        if let Some((x, y, size)) = hl {
            quad(&mut v, x, y, size, size, self.highlight_back, WHITE, WHITE);
        }
        self.back_verts = v.len() as u32;

        // 4. And the front half, which the second draw picks up after the
        //    icons have gone down between them.
        if let Some((x, y, size)) = hl {
            quad(&mut v, x, y, size, size, self.highlight_front, WHITE, WHITE);
        }
        // The bars belong with the front half — they go over the icons —
        // and they are built whether or not the screen is open, because the
        // hotbar draws them too.
        let bars_from = v.len() as u32;
        // 6. The durability bars, over the icons. Two fills per slot: a 13x2
        //    black bed, then `getBarWidth()` x **1** of colour on top of it, so
        //    the bottom row of black reads as the bar's shadow.
        for bar in bars {
            let px = |n: i32| n as f32 * bar.scale;
            push_quad(
                &mut v,
                bar.x + px(2),
                bar.y + px(13),
                px(13),
                px(2),
                self.white,
                BAR_BED,
                BAR_BED,
            );
            if bar.width > 0 {
                let c = [bar.color[0], bar.color[1], bar.color[2], 1.0];
                push_quad(
                    &mut v,
                    bar.x + px(2),
                    bar.y + px(13),
                    px(bar.width),
                    px(1),
                    self.white,
                    c,
                    c,
                );
            }
        }
        self.bar_range = (bars_from, v.len() as u32 - bars_from);
        self.total_verts = v.len() as u32;

        // 5. The tooltip, last of all — `extractTooltipBackground` blits the
        //    two sprites at the same rect, the background painting the fill
        //    and the frame the edge over it.
        if let Some(tip) = tooltip {
            let ((tx, ty), (tw, th)) = (tip.pos, tip.size);
            let (bx, by) = (tx - TOOLTIP_INSET, ty - TOOLTIP_INSET);
            let (bw, bh) = (tw + TOOLTIP_INSET * 2, th + TOOLTIP_INSET * 2);
            for (rect, border) in [
                (self.tooltip_bg, TOOLTIP_BG_BORDER),
                (self.tooltip_frame, TOOLTIP_FRAME_BORDER),
            ] {
                // **Screen space, not panel space.** `tooltip_position` works
                // from the whole screen's GUI dimensions — it has to, because
                // a tooltip flips against the *screen's* edge, not the
                // panel's — so its result must not be offset by the panel's
                // origin the way a slot's is. Adding `left`/`top` here puts
                // the box a panel's width away from its own text.
                nine_slice(
                    &mut v,
                    rect,
                    TOOLTIP_SPRITE,
                    TOOLTIP_SPRITE,
                    border,
                    bx,
                    by,
                    bw,
                    bh,
                    0.0,
                    0.0,
                    scale,
                );
            }
            // 6. …and the image pass's own blits, over the box and in the same
            //    screen space.
            if let Some(image) = tip.bundle.as_ref() {
                self.bundle_chrome(&mut v, image, scale);
            }
        }
        self.tooltip_verts = v.len() as u32 - self.total_verts;
        self.upload(&v);
    }

    /// `ClientBundleTooltip.extractImage`'s six sprites, in its order.
    ///
    /// ```java
    /// if (hasHighlight) blitSprite(SLOT_HIGHLIGHT_BACK_SPRITE, drawX, drawY, 24, 24);
    /// else              blitSprite(SLOT_BACKGROUND_SPRITE,     drawX, drawY, 24, 24);
    /// graphics.item(item, drawX + 4, drawY + 4, slotIndex);
    /// graphics.itemDecorations(font, item, drawX + 4, drawY + 4);
    /// if (hasHighlight) blitSprite(SLOT_HIGHLIGHT_FRONT_SPRITE, drawX, drawY, 24, 24);
    /// ```
    ///
    /// Three things in there read as choices and are not:
    ///
    /// - the selected cell's background is **replaced** by
    ///   `slot_highlight_back`, not covered by it. Drawing both would put the
    ///   ordinary slot art under the highlight, which is a different picture.
    /// - only `extractSlot` blits anything. `extractCount`, the `+N` badge, is
    ///   a bare `centeredText` — the badge cell has **no** background, and the
    ///   grid's art stops at the last occupied slot.
    /// - the progress bar blits its **fill first and its border over it**, so
    ///   the border's 1 px frame wins wherever the two meet. Reversing them
    ///   paints the fill's top row across the frame.
    ///
    /// The gap between the two highlight blits is where `graphics.item` and
    /// `itemDecorations` go. Rewo draws nothing there yet: the grid's contents
    /// come from the `minecraft:bundle_contents` component, which
    /// `rewo-net`'s patch reader walks past without keeping (M58 gap).
    fn bundle_chrome(&self, v: &mut Vec<Vertex>, image: &crate::tooltip::BundleImage, scale: f32) {
        use crate::tooltip::{CellKind, GRID_WIDTH, PROGRESSBAR_BORDER, PROGRESSBAR_HEIGHT};
        let cell = |v: &mut Vec<Vertex>, r: Rect, x: i32, y: i32| {
            nine_slice(
                v,
                r,
                BUNDLE_CELL_SPRITE,
                BUNDLE_CELL_SPRITE,
                BUNDLE_CELL_BORDER,
                x,
                y,
                crate::tooltip::SLOT_SIZE,
                crate::tooltip::SLOT_SIZE,
                0.0,
                0.0,
                scale,
            );
        };
        for c in &image.cells {
            let CellKind::Slot { selected, .. } = c.kind else {
                continue;
            };
            if selected {
                cell(v, self.bundle_hl_back, c.x, c.y);
                // (the item and its decorations)
                cell(v, self.bundle_hl_front, c.x, c.y);
            } else {
                cell(v, self.bundle_slot, c.x, c.y);
            }
        }
        if let Some(bar) = image.bar {
            let fill = if bar.full {
                self.bar_full
            } else {
                self.bar_fill
            };
            nine_slice(
                v,
                fill,
                BAR_FILL_SPRITE,
                BAR_FILL_SPRITE,
                BAR_FILL_BORDER,
                bar.x + PROGRESSBAR_BORDER,
                bar.y,
                bar.fill,
                PROGRESSBAR_HEIGHT,
                0.0,
                0.0,
                scale,
            );
            nine_slice(
                v,
                self.bar_border,
                BAR_BORDER_SPRITE,
                BAR_BORDER_SPRITE,
                BAR_BORDER_BORDER,
                bar.x,
                bar.y,
                GRID_WIDTH,
                PROGRESSBAR_HEIGHT,
                0.0,
                0.0,
                scale,
            );
        }
    }

    fn upload(&mut self, v: &[Vertex]) {
        if v.is_empty() {
            return;
        }
        if let Some(alloc) = self.allocs[self.cursor].as_ref() {
            if let Some(ptr) = alloc.mapped_ptr() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        v.as_ptr() as *const u8,
                        ptr.as_ptr() as *mut u8,
                        std::mem::size_of_val(v),
                    );
                }
            }
        }
    }

    /// The backdrop, the panel and the back highlight — everything that goes
    /// *under* the item icons.
    pub fn draw_back(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        self.draw_range(gpu, cb, extent, 0, self.back_verts);
    }

    /// The front highlight, over the icons.
    pub fn draw_front(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        self.draw_range(
            gpu,
            cb,
            extent,
            self.back_verts,
            self.total_verts - self.back_verts,
        );
    }

    /// The durability bars. Drawn on their own so the hotbar can have them
    /// without the panel behind it.
    pub fn draw_bars(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        self.draw_range(gpu, cb, extent, self.bar_range.0, self.bar_range.1);
    }

    /// The tooltip, over the icons, the front highlight and the carried stack.
    pub fn draw_tooltip(&self, gpu: &Gpu, cb: vk::CommandBuffer, extent: vk::Extent2D) {
        self.draw_range(gpu, cb, extent, self.total_verts, self.tooltip_verts);
    }

    fn draw_range(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        first: u32,
        count: u32,
    ) {
        if count == 0 {
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
            device.cmd_draw(cb, count, 1, first, 0);
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
            .map_err(|e| format!("container vertex buffer: {e}"))?
    };
    let req = unsafe { gpu.device.get_buffer_memory_requirements(buf) };
    let alloc = gpu
        .allocator
        .allocate(&AllocationCreateDesc {
            name: "container-verts",
            requirements: req,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| format!("container vertex alloc: {e}"))?;
    unsafe {
        gpu.device
            .bind_buffer_memory(buf, alloc.memory(), alloc.offset())
            .map_err(|e| format!("container bind: {e}"))?;
    }
    Ok((buf, alloc))
}

pub(crate) fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/container.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/container.frag.spv")),
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
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend_state =
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
            .color_blend_state(&blend_state)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("container pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel is centred by vanilla's integer division, and the result is
    /// scaled up — so at GUI scale 2 on a 1280×720 window the origin is an even
    /// number of pixels and the sprite art lands on whole texels.
    #[test]
    fn the_panel_is_centred_on_whole_pixels() {
        let (left, top, scale) = gui_origin(1280.0, 720.0);
        assert_eq!(scale, 3.0, "1280×720 gives GUI scale 3");
        // 1280/3 = 426.67 wide in GUI space, so (426.67 - 176)/2 = 125.33 → 125.
        assert_eq!(left, 125.0 * 3.0);
        assert_eq!(top, ((720.0 / 3.0 - GUI_HEIGHT) / 2.0).floor() * 3.0);
        assert_eq!(left.fract(), 0.0);
        assert_eq!(top.fract(), 0.0);
    }

    /// The cursor maps back through the same origin, so the point the panel is
    /// drawn at and the point a click is tested against cannot drift apart.
    #[test]
    fn the_cursor_maps_back_through_the_same_origin() {
        let (w, h) = (1280.0, 720.0);
        let (left, top, scale) = gui_origin(w, h);
        // The panel's own top-left must map to GUI (0, 0).
        let g = screen_to_gui((left as f64, top as f64), w, h);
        assert!(g.0.abs() < 1e-9 && g.1.abs() < 1e-9, "{g:?}");
        // And one GUI pixel across is `scale` screen pixels across.
        let g = screen_to_gui(((left + scale) as f64, top as f64), w, h);
        assert!((g.0 - 1.0).abs() < 1e-9, "{g:?}");
    }

    /// The whole source rect, as the atlas would hand it over.
    const FULL: Rect = Rect {
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
    };

    /// The destination rect of each quad in a vertex list, as
    /// `(x, y, w, h)` — `push_quad` writes its top-left first and its
    /// bottom-right third.
    fn quads(v: &[Vertex]) -> Vec<(f32, f32, f32, f32)> {
        v.chunks(6)
            .map(|q| {
                let (x, y) = (q[0].pos[0], q[0].pos[1]);
                (x, y, q[2].pos[0] - x, q[2].pos[1] - y)
            })
            .collect()
    }

    /// `blitNineSlicedSprite`'s first branch: at the sprite's authored size it
    /// blits the whole thing once and does not slice at all.
    ///
    /// This is the branch all three of the bundle's 24x24 cell sprites take,
    /// every time — which is why their `.mcmeta` borders never affect
    /// anything, and why a cell reproduces its texture exactly.
    #[test]
    fn a_native_size_blit_is_one_quad_of_the_whole_sprite() {
        let mut v = Vec::new();
        nine_slice(&mut v, FULL, 24, 24, 4, 10, 20, 24, 24, 0.0, 0.0, 1.0);
        assert_eq!(quads(&v), vec![(10.0, 20.0, 24.0, 24.0)]);
        // …and it samples the sprite's own corners, not an interior slice.
        assert_eq!((v[0].uv, v[2].uv), ([0.0, 0.0], [1.0, 1.0]));
    }

    /// However the borders shrink against a small destination, the pieces
    /// still tile it exactly — no gap along a seam, no doubled band.
    ///
    /// The third case is the one that really happens: `getProgressBarFill` is
    /// a pixel count, so a nearly-empty bundle asks for a bar three pixels
    /// wide out of a sprite whose two borders are four.
    #[test]
    fn the_sliced_pieces_tile_the_destination_exactly() {
        for (sw, sh, b, w, h) in [
            (12, 12, 2, 96, 13), // the progress bar's border
            (6, 6, 2, 94, 13),   // a full fill
            (6, 6, 2, 3, 13),    // a fill narrower than its own two borders
            (100, 100, 9, 64, 32), // the tooltip's background
        ] {
            let mut v = Vec::new();
            nine_slice(&mut v, FULL, sw, sh, b, 0, 0, w, h, 0.0, 0.0, 1.0);
            let qs = quads(&v);
            let area: f32 = qs.iter().map(|q| q.2 * q.3).sum();
            assert_eq!(
                area,
                (w * h) as f32,
                "{sw}x{sh} border {b} into {w}x{h} covered {area} of {}",
                w * h
            );
            assert!(qs.iter().all(|q| q.2 > 0.0 && q.3 > 0.0));
            let right = qs.iter().map(|q| q.0 + q.2).fold(0.0f32, f32::max);
            let bottom = qs.iter().map(|q| q.1 + q.3).fold(0.0f32, f32::max);
            assert_eq!((right, bottom), (w as f32, h as f32));
        }
    }

    /// A zero-width blit draws nothing rather than a degenerate quad —
    /// `getProgressBarFill` really returns 0 for an empty bundle, and vanilla
    /// calls `blitSprite` with it anyway.
    #[test]
    fn an_empty_fill_emits_no_geometry() {
        let mut v = Vec::new();
        nine_slice(&mut v, FULL, 6, 6, 2, 0, 0, 0, 13, 0.0, 0.0, 1.0);
        assert!(v.is_empty());
    }

    /// The backdrop's bottom is darker than its top — `0xD0` against `0xC0`.
    /// A flat fill would look almost right, which is why this is pinned.
    #[test]
    fn the_backdrop_is_a_gradient_not_a_fill() {
        assert!(BACKDROP_BOTTOM[3] > BACKDROP_TOP[3]);
        assert_eq!(BACKDROP_TOP[3], 192.0 / 255.0);
        assert_eq!(BACKDROP_BOTTOM[3], 208.0 / 255.0);
    }
}
