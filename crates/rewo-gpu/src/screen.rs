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
//! # `blitNineSlicedSprite` — one implementation, reconciled (M84 + M85)
//!
//! M82 declined to transcribe it and named what would force the issue: "a
//! screen that wants a 150-wide button needs it". Two milestones then needed
//! it in the same session — M85's server-links dialog draws **310**-wide
//! buttons, and M84's statistics screen draws a **98-wide tab out of a 130-wide
//! sheet** and a **6×32 scroller at 6×35**. Both shipped one; this is the
//! survivor, and it is M84's, because M85's could not reach either of M84's two
//! call sites:
//!
//! * M85's took its sheet size from the `SPRITE_W`/`SPRITE_H` constants and its
//!   border from a single `NINE_SLICE_BORDER = 3`, so it could express the
//!   200×20 button and nothing else. A tab is 130×24 with an **asymmetric**
//!   border (`{left 2, top 2, right 2, bottom 0}`), which a single number
//!   cannot say.
//! * M85 transcribed only the two branches whose height matches the sheet's,
//!   and skipped a button of any other height with a warning — deliberately,
//!   because every `Button` is `Button.DEFAULT_HEIGHT`. The scroller is not a
//!   button: it is a 32-tall sheet drawn at whatever the list's box allows, so
//!   the vertical branches are exercised here and are transcribed.
//!
//! What survives from M85 is its two findings, both of which this
//! implementation already obeys and neither of which was ever in doubt: the
//! inner segment **tiles rather than stretching**, and the borders are
//! **clamped to half the target**. The 200×20 button still comes out as the 1:1
//! blit M82 shipped — that is [`nine_slice`]'s first branch — so nothing about
//! either earlier milestone's pixels moves.
//!
//! Vanilla's four-way fork on `width == sprite.width` / `height ==
//! sprite.height` is not reproduced branch for branch: those branches exist so
//! an exactly-sized axis skips its corner splits, and emitting a zero-width
//! corner produces identical geometry, which [`push_src`] already drops. The
//! whole-sprite case *is* kept, because a 1:1 blit must stay one quad.
//!
//! # Tiling in an atlas
//!
//! Vanilla tiles by sampling `0 .. width / textureWidth` out of a `REPEAT`
//! texture. Everything here lives in one atlas with `CLAMP_TO_EDGE`, so a tile
//! is one quad and [`push_tiled`] emits the grid — which is what
//! `blitTiledSprite` does anyway. The last row and column are clipped by
//! **shortening their UVs**, not by overdrawing: `TiledBlitRenderState` ends
//! its partial tile at `Mth.lerp(remaining / tileWidth, u0, u1)`, and a screen
//! that tiles a *sub-rectangle* (the statistics screen's header strip) would
//! otherwise spill into what sits below it.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::container::{build_pipeline, Rect, Vertex};
use crate::entities::create_texture;
use crate::Gpu;

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
/// The backdrop, the tiled menu backgrounds, the buttons, and the statistics
/// screen's row sprites.
///
/// The full-screen tiled background dominates, and M85's own arithmetic is the
/// binding case: one quad per 32×32 GUI cell, so a 3840×2160 display at GUI
/// scale **2** is 120 × 68 = 8,160 quads = 48,960 vertices — which is why
/// M85's 8,192 was already over its own cap. This is sized for the same display
/// at GUI scale 4 (30 × 17 = 510 quads) with room for every widget on top;
/// past that [`push_quad`] drops the overflow rather than corrupting the
/// buffer, which is the same degradation M85 documented.
const MAX_VERTS: usize = 16384;
const RING: usize = 2;
const ATLAS_W: u32 = 512;
// 512 as of M172: the 192x192 book background did not fit the 256-tall
// atlas. Every pre-M172 placement is unchanged in TEXELS, and the `uv`
// closure divides by the const, so the old sheets render pixel-identically.
// 1024 as of M178: the advancements shelf (a 252x140 window crop, 24 tab
// sprites, 6 frames, three nine-sliced boxes, five root backdrops) needs more
// height than the post-M174 free space held. Same append-only rule as M172:
// nothing below y=512 moves, so every older sheet renders identically.
const ATLAS_H: u32 = 1024;

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

/// Every sheet in this pass's atlas, as a flat index (M84).
///
/// A mirror of `rewo_world::screen::Sprite` widened with the sheets no widget
/// selects — the two tiled backgrounds and the separators, which are screen
/// *chrome* rather than widget state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sheet {
    Button,
    ButtonDisabled,
    ButtonHighlighted,
    /// `MenuTabButton.SPRITES` in declared order: selected, plain,
    /// selected-highlighted, highlighted.
    Tab(u8),
    Scroller,
    ScrollerBackground,
    Slot,
    StatHeader,
    StatColumn(u8),
    SortUp,
    SortDown,
    TabHeaderBackground,
    InworldMenuBackground,
    InworldHeaderSeparator,
    InworldFooterSeparator,
    /// `Screen.MENU_BACKGROUND`, which a selected tab paints inside itself.
    MenuBackground,
    /// One opaque white texel — `GuiGraphicsExtractor.fill`, which is how a
    /// selected tab draws its focus underline.
    White,
    /// `textures/gui/book.png` cropped to the 192×192 the blit samples (M172).
    BookBackground,
    /// The four `widget/page_*` arrows (M172), indexed in the bake's declared
    /// order: 0 forward, 1 forward_highlighted, 2 backward,
    /// 3 backward_highlighted. 23×13, `Stretch` at native size = a 1:1 blit.
    PageArrow(u8),
    /// The `AbstractSliderButton` sheets (M173): 0 track, 1 track_highlighted
    /// (200×20, nine-slice border 1), 2 handle, 3 handle_highlighted (8×20,
    /// nine-slice border `{2, 2, 2, 3}`).
    SliderSheet(u8),
    /// A `gui/signs/<wood>.png` sheet (M174), 24x26, indexed by
    /// `rewo_data::assets::SIGN_WOODS` order. The edit screen blits the top
    /// 26 rows (standing) or the top 12 (wall — the board without the post)
    /// at scale 3.9, so a wall board is drawn via [`Fill::Crop`]-less partial
    /// height: the sprite rect samples `v 0..12/26` of the sheet.
    SignBoard(u8),
    /// A `gui/hanging_signs/<wood>.png` sheet (M174), 16x16, scale 4.5 —
    /// chains baked in.
    HangingSignBoard(u8),
    /// The advancements window (M178): `advancements/window.png` CROPPED to
    /// the 252x140 the blit samples (`blit(.., 0, 0, 252, 140, 256, 256)`),
    /// so a whole-sheet `Stretch` is the exact blit — M172's book rule.
    AdvWindow,
    /// One of the 24 `advancements/tab_*` sprites (M178), indexed
    /// `kind * 6 + cap * 2 + selected` where `kind` is Above/Below/Left/Right
    /// in declaration order and `cap` is First/Middle/Last. Above/Below are
    /// 28x32; Left/Right are 32x28. The INDEX is part of the screen-pass
    /// contract, append-only like the sign boards'.
    AdvTab(u8),
    /// One of the six frame sprites (M178): `type_ * 2 + obtained`, types in
    /// Task/Challenge/Goal order. All 26x26.
    AdvFrame(u8),
    /// `advancements/box_obtained` (M178) — 200x26, nine-slice border 10,
    /// the hover tooltip's progress bar's OBTAINED half-sheet.
    AdvBoxObtained,
    /// `advancements/box_unobtained` (M178) — same shape, the UNOBTAINED half.
    AdvBoxUnobtained,
    /// `advancements/title_box` (M178) — 200x26 nine-slice border 10, the
    /// tooltip's background panel.
    AdvTitleBox,
    /// A root advancement's tiled backdrop (M178):
    /// `advancements/backgrounds/<name>.png`, 16x16, indexed by the bake's
    /// [`crate::ADV_BACKGROUNDS`] order. An identifier outside the table
    /// draws nothing rather than guessing (the caller falls back).
    AdvBackground(u8),
}

/// One blit, in **GUI space** (the app multiplies nothing; this pass applies
/// the GUI scale, exactly as [`crate::container`] does for the panel).
#[derive(Clone, Copy, Debug)]
pub struct SpriteDraw {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub sheet: Sheet,
    /// How the sheet fills the rect.
    pub fill: Fill,
    /// `ARGB` tint, multiplied into the texel. `[1; 4]` for every blit vanilla
    /// makes without a colour argument.
    pub color: [f32; 4],
}

/// How a sheet is mapped onto a rect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fill {
    /// One quad over the whole rect — `blitSprite` on a `Stretch`-scaled
    /// sprite, and what an 18×18 sheet at 18×18 degenerates to anyway.
    Stretch,
    /// `blitNineSlicedSprite` with this border, `{left, top, right, bottom}`.
    NineSlice([i32; 4]),
    /// `Screen.extractMenuBackgroundTexture` — the sheet repeated at
    /// `(tile_w, tile_h)` GUI pixels. The tile size is the **declared**
    /// texture size at the call site, which is not always the file's own.
    Tiled(i32, i32),
    /// Vanilla's eight-arg `blit(x, y, u, v, w, h, texW, texH)` sampling only
    /// a texel sub-rect of the sheet (M174 — a wall sign board is the top
    /// `24x12` of the 24x26 texture). `(u, v, w, h)` in sheet texels.
    SubRect(i32, i32, i32, i32),
}

/// `Screen.extractMenuBackground`'s tiled texture (M85).
///
/// Mutually exclusive with [`ScreenDraw::backdrop`] in vanilla, because
/// `extractBackground`'s two branches are an if/else. Kept as its own field
/// rather than folded into [`ScreenDraw::sprites`] so M85's three screens and
/// `serverlinkshot` are untouched; it lowers to exactly the same
/// [`Fill::Tiled`] emission they would have written by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuBackgroundDraw {
    /// `minecraft.level != null` — selects `INWORLD_MENU_BACKGROUND` over
    /// `MENU_BACKGROUND`.
    pub in_world: bool,
}

/// The tile size of the menu background, in GUI pixels — the `32, 32` declared
/// size in `extractMenuBackgroundTexture`, **not** the 16×16 file. The sheet is
/// therefore drawn 2× magnified, and its neighbour `tab_header_background` is
/// declared at its true 16×16 and is not.
pub const MENU_BACKGROUND_TILE: i32 = 32;

/// One button to draw. Kept as its own type so M82's death-screen path and its
/// gate are untouched; it lowers to a [`SpriteDraw`] with
/// [`Fill::NineSlice`] and `border 3`.
#[derive(Clone, Copy, Debug)]
pub struct ButtonDraw {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub sprite: ButtonSprite,
}

/// `widget/button*.png`'s `.mcmeta` — `"border": 3`, all four sides.
pub const BUTTON_BORDER: [i32; 4] = [3, 3, 3, 3];
/// `widget/tab*.png`'s — `{left 2, top 2, right 2, bottom 0}`. The bottom is
/// **zero**, which is what lets a selected tab's underline reach the sheet's
/// last row unscaled.
pub const TAB_BORDER: [i32; 4] = [2, 2, 2, 0];
/// `widget/scroller*.png`'s — `"border": 1`.
pub const SCROLLER_BORDER: [i32; 4] = [1, 1, 1, 1];

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
    /// Everything else, in draw order. Drawn **after** the backdrop and
    /// **before** the buttons, so a screen's tiled background sits under its
    /// widgets.
    pub sprites: Vec<SpriteDraw>,
    /// Scissored batches (M178), drawn between `menu_background` and
    /// `sprites` — `AdvancementsScreen.extractInside` runs BEFORE
    /// `extractWindow`, so the clipped contents sit under the chrome that
    /// frames them. Each batch is one `enableScissor` region: a
    /// `cmd_set_scissor`, its sprites, then on to the next.
    pub scissored: Vec<ScissorBatch>,
}

/// One `graphics.enableScissor` region and the sprites inside it (M178).
#[derive(Clone, Debug, Default)]
pub struct ScissorBatch {
    /// GUI pixels, y-down: `(x, y, w, h)`.
    pub rect: (i32, i32, i32, i32),
    pub sprites: Vec<SpriteDraw>,
}

impl ScreenDraw {
    pub fn is_empty(&self) -> bool {
        self.backdrop.is_none()
            && self.menu_background.is_none()
            && self.buttons.is_empty()
            && self.sprites.is_empty()
            && self.scissored.is_empty()
    }
}

/// Borrowed view of `rewo_data::assets::WidgetSprites`.
pub struct WidgetSpriteData<'a> {
    pub button: crate::hud::HudSpriteData<'a>,
    pub button_disabled: crate::hud::HudSpriteData<'a>,
    pub button_highlighted: crate::hud::HudSpriteData<'a>,
    pub tabs: [crate::hud::HudSpriteData<'a>; 4],
    pub scroller: crate::hud::HudSpriteData<'a>,
    pub scroller_background: crate::hud::HudSpriteData<'a>,
    pub slot: crate::hud::HudSpriteData<'a>,
    pub stat_header: crate::hud::HudSpriteData<'a>,
    pub stat_columns: [crate::hud::HudSpriteData<'a>; 6],
    pub sort_up: crate::hud::HudSpriteData<'a>,
    pub sort_down: crate::hud::HudSpriteData<'a>,
    pub tab_header_background: crate::hud::HudSpriteData<'a>,
    pub inworld_menu_background: crate::hud::HudSpriteData<'a>,
    pub menu_background: crate::hud::HudSpriteData<'a>,
    pub inworld_header_separator: crate::hud::HudSpriteData<'a>,
    pub inworld_footer_separator: crate::hud::HudSpriteData<'a>,
    /// The cropped 192×192 book background (M172).
    pub book_background: crate::hud::HudSpriteData<'a>,
    /// forward, forward_highlighted, backward, backward_highlighted (M172).
    pub page_buttons: [crate::hud::HudSpriteData<'a>; 4],
    /// track, track_highlighted, handle, handle_highlighted (M173).
    pub slider: [crate::hud::HudSpriteData<'a>; 4],
    /// The 12 standing/wall sign boards (M174), `SIGN_WOODS` order.
    pub sign_boards: [crate::hud::HudSpriteData<'a>; 12],
    /// The 12 hanging sign boards (M174), `SIGN_WOODS` order.
    pub hanging_sign_boards: [crate::hud::HudSpriteData<'a>; 12],
    /// The advancements window, cropped to its sampled 252x140 (M178).
    pub adv_window: crate::hud::HudSpriteData<'a>,
    /// The 24 tab sprites (M178), indexed `kind*6 + cap*2 + selected` — see
    /// [`Sheet::AdvTab`].
    pub adv_tabs: [crate::hud::HudSpriteData<'a>; 24],
    /// The six frame sprites (M178), `type*2 + obtained` — see
    /// [`Sheet::AdvFrame`].
    pub adv_frames: [crate::hud::HudSpriteData<'a>; 6],
    /// box_obtained, box_unobtained, title_box (M178) — all 200x26 nine-slice
    /// border 10.
    pub adv_boxes: [crate::hud::HudSpriteData<'a>; 3],
    /// The five vanilla root backdrops (M178), [`crate::ADV_BACKGROUNDS`]
    /// order — wait, the table lives in `rewo_data`; this array is indexed by
    /// the same order that file's bake uses.
    pub adv_backgrounds: [crate::hud::HudSpriteData<'a>; 5],
}

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
    /// The head segment's vertex count — backdrop + menu background, drawn
    /// under the full-extent scissor before any batch.
    head_verts: u32,
    /// The unscissored vertex count — everything through `buttons`. The
    /// scissored batches (M178) sit between the two, each drawn under its own
    /// scissor.
    verts: u32,
    /// `(first_vertex, vertex_count, ox, oy, w, h)` per scissored batch, in
    /// queue order. Rebuilt each `set_state`.
    scissor_batches: Vec<(u32, u32, i32, i32, u32, u32)>,
    enabled: Rect,
    disabled: Rect,
    highlighted: Rect,
    white: Rect,
    /// Every sheet's atlas placement in **texels**, `(x, y, w, h)`, keyed the
    /// same way [`Sheet`] is. Texels rather than UVs because the nine-slice
    /// arithmetic is all in source pixels and converting once at the end is
    /// both simpler and exact.
    sheets: Vec<((u32, u32), (u32, u32))>,
}

/// The order [`Sheet`] indexes into [`ScreenPass::sheets`].
fn sheet_index(s: Sheet) -> usize {
    match s {
        Sheet::Button => 0,
        Sheet::ButtonDisabled => 1,
        Sheet::ButtonHighlighted => 2,
        Sheet::Tab(i) => 3 + (i as usize).min(3),
        Sheet::Scroller => 7,
        Sheet::ScrollerBackground => 8,
        Sheet::Slot => 9,
        Sheet::StatHeader => 10,
        Sheet::StatColumn(i) => 11 + (i as usize).min(5),
        Sheet::SortUp => 17,
        Sheet::SortDown => 18,
        Sheet::TabHeaderBackground => 19,
        Sheet::InworldMenuBackground => 20,
        Sheet::InworldHeaderSeparator => 21,
        Sheet::InworldFooterSeparator => 22,
        Sheet::MenuBackground => 23,
        Sheet::White => 24,
        Sheet::BookBackground => 25,
        Sheet::PageArrow(i) => 26 + (i as usize).min(3),
        Sheet::SliderSheet(i) => 30 + (i as usize).min(3),
        Sheet::SignBoard(i) => 34 + (i as usize).min(11),
        Sheet::HangingSignBoard(i) => 46 + (i as usize).min(11),
        Sheet::AdvWindow => 58,
        Sheet::AdvTab(i) => 59 + (i as usize).min(23),
        Sheet::AdvFrame(i) => 83 + (i as usize).min(5),
        Sheet::AdvBoxObtained => 89,
        Sheet::AdvBoxUnobtained => 90,
        Sheet::AdvTitleBox => 91,
        Sheet::AdvBackground(i) => 92 + (i as usize).min(4),
    }
}
const SHEET_COUNT: usize = 97;

impl ScreenPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        sprites: &WidgetSpriteData<'_>,
    ) -> Result<Self, String> {
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let place = |dst: &mut [u8], s: &crate::hud::HudSpriteData<'_>, x: u32, y: u32| {
            for row in 0..s.h.min(ATLAS_H.saturating_sub(y)) {
                let src = (row * s.w * 4) as usize;
                let d = (((y + row) * ATLAS_W + x) * 4) as usize;
                let n = (s.w.min(ATLAS_W - x) * 4) as usize;
                dst[d..d + n].copy_from_slice(&s.rgba[src..src + n]);
            }
        };
        // A fixed shelf layout rather than a packer: 23 sheets whose sizes are
        // known at compile time, and a stable placement means the UVs below
        // are readable constants instead of a lookup.
        let mut sheets = vec![((0u32, 0u32), (0u32, 0u32)); SHEET_COUNT];
        let mut put = |atlas: &mut Vec<u8>,
                       sheets: &mut Vec<((u32, u32), (u32, u32))>,
                       s: Sheet,
                       data: &crate::hud::HudSpriteData<'_>,
                       x: u32,
                       y: u32| {
            place(atlas, data, x, y);
            sheets[sheet_index(s)] = ((x, y), (data.w, data.h));
        };
        put(&mut atlas, &mut sheets, Sheet::Button, &sprites.button, 0, 0);
        put(
            &mut atlas,
            &mut sheets,
            Sheet::ButtonDisabled,
            &sprites.button_disabled,
            0,
            20,
        );
        put(
            &mut atlas,
            &mut sheets,
            Sheet::ButtonHighlighted,
            &sprites.button_highlighted,
            0,
            40,
        );
        // The four 130x24 tabs, two to a row.
        for (i, t) in sprites.tabs.iter().enumerate() {
            let (x, y) = (130 * (i as u32 % 2), 60 + 24 * (i as u32 / 2));
            put(&mut atlas, &mut sheets, Sheet::Tab(i as u8), t, x, y);
        }
        // The 18x18 family, one row.
        let row = 108;
        put(&mut atlas, &mut sheets, Sheet::Slot, &sprites.slot, 0, row);
        put(
            &mut atlas,
            &mut sheets,
            Sheet::StatHeader,
            &sprites.stat_header,
            18,
            row,
        );
        for (i, c) in sprites.stat_columns.iter().enumerate() {
            let x = 36 + 18 * i as u32;
            put(&mut atlas, &mut sheets, Sheet::StatColumn(i as u8), c, x, row);
        }
        put(
            &mut atlas,
            &mut sheets,
            Sheet::SortUp,
            &sprites.sort_up,
            144,
            row,
        );
        put(
            &mut atlas,
            &mut sheets,
            Sheet::SortDown,
            &sprites.sort_down,
            162,
            row,
        );
        // The two 16x16 backgrounds and the two 6x32 scrollers.
        put(
            &mut atlas,
            &mut sheets,
            Sheet::TabHeaderBackground,
            &sprites.tab_header_background,
            0,
            128,
        );
        put(
            &mut atlas,
            &mut sheets,
            Sheet::InworldMenuBackground,
            &sprites.inworld_menu_background,
            16,
            128,
        );
        put(
            &mut atlas,
            &mut sheets,
            Sheet::Scroller,
            &sprites.scroller,
            32,
            128,
        );
        put(
            &mut atlas,
            &mut sheets,
            Sheet::ScrollerBackground,
            &sprites.scroller_background,
            38,
            128,
        );
        // The two 32x2 separators.
        put(
            &mut atlas,
            &mut sheets,
            Sheet::InworldHeaderSeparator,
            &sprites.inworld_header_separator,
            48,
            128,
        );
        put(
            &mut atlas,
            &mut sheets,
            Sheet::InworldFooterSeparator,
            &sprites.inworld_footer_separator,
            80,
            128,
        );
        put(
            &mut atlas,
            &mut sheets,
            Sheet::MenuBackground,
            &sprites.menu_background,
            112,
            128,
        );
        // M172 — the book shelf, below everything pre-M172 so no old texel
        // moves. The background at (0, 256); the four 23×13 arrows beside it.
        put(
            &mut atlas,
            &mut sheets,
            Sheet::BookBackground,
            &sprites.book_background,
            0,
            256,
        );
        for (i, a) in sprites.page_buttons.iter().enumerate() {
            put(
                &mut atlas,
                &mut sheets,
                Sheet::PageArrow(i as u8),
                a,
                200 + 24 * i as u32,
                256,
            );
        }
        // M173 — the slider shelf, under the arrows: the two 200x20 tracks
        // stacked, then the two 8x20 handles beside them.
        put(&mut atlas, &mut sheets, Sheet::SliderSheet(0), &sprites.slider[0], 200, 280);
        put(&mut atlas, &mut sheets, Sheet::SliderSheet(1), &sprites.slider[1], 200, 300);
        put(&mut atlas, &mut sheets, Sheet::SliderSheet(2), &sprites.slider[2], 404, 280);
        put(&mut atlas, &mut sheets, Sheet::SliderSheet(3), &sprites.slider[3], 414, 280);
        // M174 — the sign-board shelves: the 12 standing boards (24x26) in a
        // row at y 320, the 12 hanging boards (16x16) below them at y 352.
        for i in 0..12 {
            put(
                &mut atlas,
                &mut sheets,
                Sheet::SignBoard(i as u8),
                &sprites.sign_boards[i],
                200 + 24 * i as u32,
                320,
            );
            put(
                &mut atlas,
                &mut sheets,
                Sheet::HangingSignBoard(i as u8),
                &sprites.hanging_sign_boards[i],
                200 + 16 * i as u32,
                352,
            );
        }
        // One opaque white texel so the untextured backdrop can share this
        // pipeline — the fragment shader's `texture * color` then leaves the
        // gradient alone. Parked in the empty bottom-right of the atlas.
        let w = ((250 * ATLAS_W + 500) * 4) as usize;
        atlas[w..w + 4].copy_from_slice(&[255, 255, 255, 255]);
        sheets[sheet_index(Sheet::White)] = ((500, 250), (1, 1));

        // M178 — the advancements shelf, everything below y=512 so no older
        // texel moves. The window crop at (0,512); then rows of tabs, frames,
        // boxes and backdrops.
        put(&mut atlas, &mut sheets, Sheet::AdvWindow, &sprites.adv_window, 0, 512);
        for (i, t) in sprites.adv_tabs.iter().enumerate() {
            let i = i as u32;
            let (x, y) = if i < 12 {
                (28 * i, 656) // Above/Below: 28x32, twelve to a row
            } else {
                (32 * (i - 12), 692) // Left/Right: 32x28
            };
            put(&mut atlas, &mut sheets, Sheet::AdvTab(i as u8), t, x, y);
        }
        for (i, f) in sprites.adv_frames.iter().enumerate() {
            put(
                &mut atlas,
                &mut sheets,
                Sheet::AdvFrame(i as u8),
                f,
                26 * i as u32,
                724,
            );
        }
        for (i, b) in sprites.adv_boxes.iter().enumerate() {
            put(
                &mut atlas,
                &mut sheets,
                [Sheet::AdvBoxObtained, Sheet::AdvBoxUnobtained, Sheet::AdvTitleBox][i],
                b,
                0,
                754 + 28 * i as u32,
            );
        }
        for (i, bg) in sprites.adv_backgrounds.iter().enumerate() {
            put(
                &mut atlas,
                &mut sheets,
                Sheet::AdvBackground(i as u8),
                bg,
                16 * i as u32,
                840,
            );
        }

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
        let white = uv(500.5, 250.5, 0.0, 0.0);

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
            head_verts: 0,
            verts: 0,
            scissor_batches: Vec::new(),
            enabled,
            disabled,
            highlighted,
            white,
            sheets,
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

        // 2. `extractMenuBackground` (M85), over the whole frame. After the
        //    gradient because the two are an if/else in vanilla and never
        //    coexist; the order only matters if a caller sets both.
        if let Some(bg) = draw.menu_background {
            let sheet = if bg.in_world {
                Sheet::InworldMenuBackground
            } else {
                Sheet::MenuBackground
            };
            let ((sx, sy), (sw, sh)) = self.sheets[sheet_index(sheet)];
            let src = Src {
                x: sx as i32,
                y: sy as i32,
                w: sw as i32,
                h: sh as i32,
            };
            // In GUI pixels, so the last tile is clipped by UV exactly as a
            // sub-rectangle's would be. Vanilla lets its final quad overrun and
            // the framebuffer clip it, which paints the same visible pixels.
            let (gw, gh) = ((w / scale).ceil() as i32, (h / scale).ceil() as i32);
            push_tiled(
                &mut v,
                scale,
                (0, 0, gw, gh),
                src,
                (0, 0, src.w, src.h),
                (MENU_BACKGROUND_TILE, MENU_BACKGROUND_TILE),
                [1.0; 4],
            );
        }
        let head_end = v.len() as u32;

        // 2.5 The scissored batches (M178), between the menu background and
        //     the plain sprites — vanilla's extractInside runs before
        //     extractWindow, so clipped contents sit under their chrome.
        self.scissor_batches.clear();
        for b in &draw.scissored {
            let start = v.len() as u32;
            for s in &b.sprites {
                lower_sprite(&mut v, scale, s, &self.sheets);
            }
            let count = v.len() as u32 - start;
            if count == 0 {
                continue;
            }
            // Device pixels: floor the leading edge, ceil the trailing one,
            // so a boundary pixel is inside exactly when vanilla's scissor
            // includes it.
            let (gx, gy, gw, gh) = b.rect;
            let ox = (gx as f32 * scale).floor() as i32;
            let oy = (gy as f32 * scale).floor() as i32;
            let x1 = ((gx + gw).max(gx) as f32 * scale).ceil();
            let y1 = ((gy + gh).max(gy) as f32 * scale).ceil();
            let w = (x1.max(ox as f32 + 1.0) as u32).saturating_sub(ox as u32);
            let h = (y1.max(oy as f32 + 1.0) as u32).saturating_sub(oy as u32);
            self.scissor_batches.push((start, count, ox, oy, w, h));
        }

        // 3. The screen's own chrome, in GUI space and in the order it was
        //    queued: a tiled background before the widgets that sit on it.
        for s in &draw.sprites {
            lower_sprite(&mut v, scale, s, &self.sheets);
        }

        // 4. The buttons, in GUI space. Nine-sliced with `border: 3`, which
        //    at the death screen's own 200x20 degenerates to the 1:1 blit M82
        //    shipped — its pixels do not move. A 310-wide dialog button (M85)
        //    and a 150-wide one both draw; so does one whose *height* is not
        //    the sheet's, which M85's horizontal-only transcription skipped.
        for b in &draw.buttons {
            let sheet = match b.sprite {
                ButtonSprite::Enabled => Sheet::Button,
                ButtonSprite::Disabled => Sheet::ButtonDisabled,
                ButtonSprite::Highlighted => Sheet::ButtonHighlighted,
            };
            let ((sx, sy), (sw, sh)) = self.sheets[sheet_index(sheet)];
            let src = Src {
                x: sx as i32,
                y: sy as i32,
                w: sw as i32,
                h: sh as i32,
            };
            nine_slice(
                &mut v,
                scale,
                (b.x, b.y, b.width, b.height),
                src,
                BUTTON_BORDER,
                [1.0; 4],
            );
        }

        self.verts = v.len() as u32;
        self.head_verts = head_end;
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
            // The head (backdrop, menu background) under the full-extent
            // scissor…
            device.cmd_draw(cb, self.head_verts, 1, 0, 0);
            let mut drawn = self.head_verts;
            // …each batch under its own scissor…
            for (start, count, ox, oy, w, h) in &self.scissor_batches {
                if *count == 0 {
                    continue;
                }
                device.cmd_set_scissor(
                    cb,
                    0,
                    &[vk::Rect2D::default()
                        .offset(vk::Offset2D::default().x(*ox).y(*oy))
                        .extent(vk::Extent2D::default().width(*w).height(*h))],
                );
                device.cmd_draw(cb, *count, 1, *start, 0);
                drawn += *count;
            }
            // …and the tail (sprites then buttons) back under the full one.
            let tail = self.verts - drawn;
            if tail > 0 {
                device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
                device.cmd_draw(cb, tail, 1, drawn, 0);
            }
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

/// A sheet's placement in the atlas, in texels.
#[derive(Clone, Copy, Debug)]
struct Src {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// One quad, taking a sub-rectangle `(tx, ty, tw, th)` of `src` in the sheet's
/// **own** pixel coordinates onto the GUI rect `(x, y, w, h)`.
#[allow(clippy::too_many_arguments)]
fn push_src(
    v: &mut Vec<Vertex>,
    scale: f32,
    (x, y, w, h): (i32, i32, i32, i32),
    src: Src,
    (tx, ty, tw, th): (i32, i32, i32, i32),
    color: [f32; 4],
) {
    if w <= 0 || h <= 0 || tw <= 0 || th <= 0 {
        return;
    }
    let rect = Rect {
        u0: (src.x + tx) as f32 / ATLAS_W as f32,
        v0: (src.y + ty) as f32 / ATLAS_H as f32,
        u1: (src.x + tx + tw) as f32 / ATLAS_W as f32,
        v1: (src.y + ty + th) as f32 / ATLAS_H as f32,
    };
    push_quad(
        v,
        x as f32 * scale,
        y as f32 * scale,
        w as f32 * scale,
        h as f32 * scale,
        rect,
        color,
        color,
    );
}

/// `blitTiledSprite` — the sub-rectangle repeated at `(tile_w, tile_h)` GUI
/// pixels, with the last row and column **clipped by shortening their UVs**
/// rather than by overdrawing.
#[allow(clippy::too_many_arguments)]
/// Lower one [`SpriteDraw`] into quads — shared by the plain sprite list and
/// the scissored batches (M178), so the two paths cannot drift.
fn lower_sprite(
    v: &mut Vec<Vertex>,
    scale: f32,
    s: &SpriteDraw,
    sheets: &[((u32, u32), (u32, u32))],
) {
    let ((sx, sy), (sw, sh)) = sheets[sheet_index(s.sheet)];
    if sw == 0 || sh == 0 {
        return;
    }
    let src = Src {
        x: sx as i32,
        y: sy as i32,
        w: sw as i32,
        h: sh as i32,
    };
    match s.fill {
        Fill::Stretch => push_src(
            v,
            scale,
            (s.x, s.y, s.width, s.height),
            src,
            (0, 0, src.w, src.h),
            s.color,
        ),
        Fill::NineSlice(border) => {
            nine_slice(v, scale, (s.x, s.y, s.width, s.height), src, border, s.color)
        }
        Fill::SubRect(u, sv, sw, sh) => push_src(
            v,
            scale,
            (s.x, s.y, s.width, s.height),
            src,
            (u, sv, sw, sh),
            s.color,
        ),
        Fill::Tiled(tw, th) => push_tiled(
            v,
            scale,
            (s.x, s.y, s.width, s.height),
            src,
            (0, 0, src.w, src.h),
            (tw, th),
            s.color,
        ),
    }
}

fn push_tiled(
    v: &mut Vec<Vertex>,
    scale: f32,
    (x, y, w, h): (i32, i32, i32, i32),
    src: Src,
    (tx, ty, tw, th): (i32, i32, i32, i32),
    (tile_w, tile_h): (i32, i32),
    color: [f32; 4],
) {
    if w <= 0 || h <= 0 || tile_w <= 0 || tile_h <= 0 {
        return;
    }
    let mut dy = 0;
    while dy < h {
        let ph = tile_h.min(h - dy);
        let mut dx = 0;
        while dx < w {
            let pw = tile_w.min(w - dx);
            // The clipped tile takes the same *fraction* of the source it
            // takes of the tile, which is what sampling `u .. u + w/texW` out
            // of a repeating texture does.
            let ctw = ((pw as i64 * tw as i64) / tile_w as i64) as i32;
            let cth = ((ph as i64 * th as i64) / tile_h as i64) as i32;
            push_src(
                v,
                scale,
                (x + dx, y + dy, pw, ph),
                src,
                (tx, ty, ctw.max(1), cth.max(1)),
                color,
            );
            dx += tile_w;
        }
        dy += tile_h;
    }
}

/// `blitNineSlicedSprite`, with `stretchInner == false` (the default).
///
/// Vanilla's four-way fork on `width == sprite.width` / `height ==
/// sprite.height` is not reproduced branch for branch: those branches exist so
/// an exactly-sized axis skips its corner splits, and emitting a zero-width
/// corner produces the identical geometry, which [`push_src`] already drops.
/// The one branch that *is* kept is the whole-sprite case, because a 1:1 blit
/// must stay one quad — the death screen's buttons go through here.
fn nine_slice(
    v: &mut Vec<Vertex>,
    scale: f32,
    (x, y, w, h): (i32, i32, i32, i32),
    src: Src,
    border: [i32; 4],
    color: [f32; 4],
) {
    if w == src.w && h == src.h {
        push_src(v, scale, (x, y, w, h), src, (0, 0, src.w, src.h), color);
        return;
    }
    // `Math.min(border.left(), width / 2)` and its three siblings — a sprite
    // drawn narrower than its own border must not draw its corners twice.
    let l = border[0].min(w / 2).max(0);
    let t = border[1].min(h / 2).max(0);
    let r = border[2].min(w / 2).max(0);
    let b = border[3].min(h / 2).max(0);
    let (mw, mh) = (w - l - r, h - t - b);
    let (sw, sh) = (src.w - l - r, src.h - t - b);
    // corners
    push_src(v, scale, (x, y, l, t), src, (0, 0, l, t), color);
    push_src(
        v,
        scale,
        (x + w - r, y, r, t),
        src,
        (src.w - r, 0, r, t),
        color,
    );
    push_src(
        v,
        scale,
        (x, y + h - b, l, b),
        src,
        (0, src.h - b, l, b),
        color,
    );
    push_src(
        v,
        scale,
        (x + w - r, y + h - b, r, b),
        src,
        (src.w - r, src.h - b, r, b),
        color,
    );
    // edges + centre, tiled
    push_tiled(v, scale, (x + l, y, mw, t), src, (l, 0, sw, t), (sw, t), color);
    push_tiled(
        v,
        scale,
        (x + l, y + h - b, mw, b),
        src,
        (l, src.h - b, sw, b),
        (sw, b),
        color,
    );
    push_tiled(
        v,
        scale,
        (x, y + t, l, mh),
        src,
        (0, t, l, sh),
        (l, sh),
        color,
    );
    push_tiled(
        v,
        scale,
        (x + w - r, y + t, r, mh),
        src,
        (src.w - r, t, r, sh),
        (r, sh),
        color,
    );
    push_tiled(
        v,
        scale,
        (x + l, y + t, mw, mh),
        src,
        (l, t, sw, sh),
        (sw, sh),
        color,
    );
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
