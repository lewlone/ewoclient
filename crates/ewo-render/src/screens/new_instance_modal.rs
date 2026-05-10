//! New-instance modal (`.modal-shroud` + `.modal-card`).
//!
//! Top-most layer when open. Click on the shroud (anywhere outside the
//! card) closes; click on the card body is consumed; the form's three
//! interactive controls (version dropdown, loader dropdown, RAM slider)
//! work; the Name field is read-only because text input is post-v1.
//!
//! CSS reference: `StyleSheet2` `.modal-shroud` through `.modal-btn-ghost`.
//!
//! State lives in `App`; rendering is read-only. `widget_bounds` exposes
//! per-control hit-rects so `main.rs` can route input. The modal renders
//! after every screen so it can spill outside any panel clips, and closes
//! on shroud click / Cancel button / Esc / tab switch.

use ewo_core::{CubicBezier, Settings};
use skia_safe::{
    canvas::SaveLayerRec, gradient_shader, image_filters, BlurStyle, Canvas, ClipOp, Color,
    Color4f, Contains, MaskFilter, Paint, PaintStyle, Point, RRect, Rect, TileMode,
};

use crate::text::{self, FontStore};
use crate::widgets::{
    draw_vbtn, draw_vdrop_head, draw_vdrop_menu, draw_vghost_btn, draw_vslider, menu_layout,
    GhostKind, VbtnState, VdropState, VghostBtnState, VsliderState,
};

/// Fallback version list shown before the live `version_manifest_v2.json`
/// finishes loading (or when offline + no cache yet). Once the live
/// manifest arrives, `NewInstanceModalState::mc_versions` replaces this
/// with the real list. Kept short — just a placeholder.
pub const FALLBACK_MC_VERSIONS: &[&str] = &["loading…"];

/// Loader options shown in the new-instance modal's Loader dropdown. v2
/// phase D: only Vanilla and the in-development EwoLoader are wired —
/// other loaders (Fabric, Forge, NeoForge, Quilt) come once their meta
/// endpoints are integrated.
pub const LOADERS: &[&str] = &["Vanilla", "Ewo (development)"];

fn default_mc_versions() -> Vec<String> {
    FALLBACK_MC_VERSIONS.iter().map(|s| (*s).to_string()).collect()
}

const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MAUVE_DEEP: Color = Color::from_argb(0xFF, 0x6B, 0x55, 0x5C);
const TEXT_MID_PEARL: Color = Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5);
const ACCENT_BERRY: Color = Color::from_argb(0xFF, 0xB4, 0x74, 0x91);
const ACCENT_PEARL_HOT: Color = Color::from_argb(0xFF, 0xFF, 0xF6, 0xF0);

const CARD_RADIUS: f32 = 22.0;
const CARD_MAX_W: f32 = 560.0;
const CARD_PAD_TOP: f32 = 36.0;
const CARD_PAD_X: f32 = 40.0;
const CARD_PAD_BOTTOM: f32 = 28.0;
const SECTION_GAP: f32 = 24.0;
const FIELD_GAP: f32 = 22.0;
const FIELD_LABEL_TO_INPUT_GAP: f32 = 8.0;
const FOOTER_PAD_TOP: f32 = 20.0;

const ANIM_DURATION: f32 = 0.24;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    Version,
    Loader,
    Ram,
    Cancel,
    Create,
}

/// Form data returned when the modal successfully submits — `App` uses
/// this to construct a new `Instance` and append it to the list.
#[derive(Debug, Clone)]
pub struct NewInstanceForm {
    pub name: String,
    pub version: String,
    pub loader: String,
    pub ram: u32,
}

#[derive(Debug, Clone)]
pub struct NewInstanceModalState {
    pub open: bool,
    /// 0..1, drives the modal-card-in entrance. Set to 0 on `open()`,
    /// ramps to 1 over `ANIM_DURATION`. Stays at 1 while open.
    pub anim: f32,
    pub name: String,
    /// Whether the name field is the active input target. Auto-focuses on
    /// `open()`. Clicking the name input refocuses; clicking anything else
    /// inside the card unfocuses (so the placeholder/caret disappears).
    pub name_focused: bool,
    /// Wall-clock seconds since the focus state last changed — used to
    /// blink the caret at 1 Hz when focused.
    pub name_focus_time: f32,
    /// `true` after a Create-with-empty-name attempt. Drives the inline
    /// "name required" error message + lifts the input border to ember.
    /// Clears as soon as the user types something or refocuses.
    pub name_error: bool,
    pub version: VdropState,
    pub loader: VdropState,
    pub ram: VsliderState,
    pub cancel_btn: VghostBtnState,
    pub create_btn: VbtnState,
    /// Live Minecraft version IDs from `version_manifest_v2.json`. Populated
    /// each frame from `App::versions` (the `VersionService`). Falls back to
    /// `FALLBACK_MC_VERSIONS` until the manifest loads.
    pub mc_versions: Vec<String>,
}

impl Default for NewInstanceModalState {
    fn default() -> Self {
        Self {
            open: false,
            anim: 0.0,
            name: String::new(),
            name_focused: false,
            name_focus_time: 0.0,
            name_error: false,
            version: VdropState::new(0),
            loader: VdropState::new(0),
            ram: VsliderState::new(4.0, 1.0, 16.0).with_step(1.0),
            cancel_btn: VghostBtnState::default(),
            create_btn: VbtnState::default(),
            mc_versions: default_mc_versions(),
        }
    }
}

impl NewInstanceModalState {
    pub fn open(&mut self) {
        // Reset form per the React `useEffect` hook on `open` toggling.
        // Name field starts blurred — placeholder visible until the user
        // explicitly clicks the input to focus it.
        self.name.clear();
        self.name_focused = false;
        self.name_focus_time = 0.0;
        self.name_error = false;
        self.version = VdropState::new(0);
        self.loader = VdropState::new(0);
        self.ram = VsliderState::new(4.0, 1.0, 16.0).with_step(1.0);
        self.cancel_btn = VghostBtnState::default();
        self.create_btn = VbtnState::default();
        self.open = true;
        self.anim = 0.0;
    }

    /// Try to construct a `NewInstanceForm` from the current state.
    /// Returns `None` and sets `name_error = true` if the name is blank,
    /// otherwise returns the trimmed form values. Caller (App) uses the
    /// returned form to append to its instance list.
    pub fn try_submit(&mut self) -> Option<NewInstanceForm> {
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            self.name_error = true;
            return None;
        }
        Some(NewInstanceForm {
            name: trimmed.to_string(),
            version: self
                .mc_versions
                .get(self.version.selected)
                .cloned()
                .unwrap_or_else(|| {
                    self.mc_versions
                        .first()
                        .cloned()
                        .unwrap_or_else(|| FALLBACK_MC_VERSIONS[0].to_string())
                }),
            loader: LOADERS
                .get(self.loader.selected)
                .copied()
                .unwrap_or(LOADERS[0])
                .to_string(),
            ram: self.ram.value as u32,
        })
    }

    pub fn focus_name(&mut self, focused: bool) {
        if self.name_focused != focused {
            self.name_focused = focused;
            self.name_focus_time = 0.0;
            if focused {
                // Clearing the error when the user starts typing again
                // happens in the keyboard handler; clearing on focus is a
                // softer UX that hides the warning while they're actively
                // engaging with the field.
                self.name_error = false;
            }
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.anim = 0.0;
        self.version.close();
        self.loader.close();
    }

    pub fn tick(&mut self, dt: f32) {
        if self.open && self.anim < 1.0 {
            self.anim = (self.anim + dt / ANIM_DURATION).min(1.0);
        }
        if self.name_focused {
            self.name_focus_time += dt;
        }
        self.version.tick(dt);
        self.loader.tick(dt);
        self.cancel_btn.tick(dt);
        self.create_btn.tick(dt);
    }

    pub fn open_dropdown(&self) -> Option<Slot> {
        if self.version.open || self.version.anim > 0.001 {
            Some(Slot::Version)
        } else if self.loader.open || self.loader.anim > 0.001 {
            Some(Slot::Loader)
        } else {
            None
        }
    }

    pub fn dropdown_state(&self, slot: Slot) -> Option<&VdropState> {
        match slot {
            Slot::Version => Some(&self.version),
            Slot::Loader => Some(&self.loader),
            _ => None,
        }
    }

    pub fn dropdown_state_mut(&mut self, slot: Slot) -> Option<&mut VdropState> {
        match slot {
            Slot::Version => Some(&mut self.version),
            Slot::Loader => Some(&mut self.loader),
            _ => None,
        }
    }

    /// Replace `mc_versions` with a freshly-fetched list. Caller (App) is
    /// responsible for snapshot-stability — new entries shouldn't reorder
    /// existing ones in a way that breaks the user's selected index. The
    /// caller passes versions ordered newest-first (Mojang's manifest
    /// order). Calls clamp `version.selected` to the new list size.
    pub fn apply_versions(&mut self, versions: Vec<String>) {
        if versions.is_empty() {
            return;
        }
        if self.version.selected >= versions.len() {
            self.version.selected = 0;
        }
        self.mc_versions = versions;
    }

    /// List of option strings for an open dropdown. Returns `None` for
    /// non-dropdown slots. Allocates a `Vec<&str>` since `mc_versions` is
    /// owned `String`s; cheap, only called when a menu is open.
    pub fn dropdown_options(&self, slot: Slot) -> Option<Vec<&str>> {
        match slot {
            Slot::Version => Some(self.mc_versions.iter().map(|s| s.as_str()).collect()),
            Slot::Loader => Some(LOADERS.to_vec()),
            _ => None,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Layout helpers
// ────────────────────────────────────────────────────────────────────────

pub fn card_rect(card_w: f32, card_h: f32) -> Rect {
    let w = CARD_MAX_W.min(card_w - 80.0);
    let total_h = card_total_height();
    let h = total_h.min(card_h - 60.0);
    let cx = card_w * 0.5;
    let cy = card_h * 0.5;
    Rect::from_xywh(cx - w * 0.5, cy - h * 0.5, w, h)
}

/// Cached layout for the modal — single source of truth so renderer and
/// hit-tester agree on widget rects. Computed from actual font metrics.
/// All `*_baseline` fields are absolute y-positions of the text baseline.
#[derive(Debug, Clone, Copy)]
pub struct ModalLayout {
    pub card: Rect,
    pub content_left: f32,
    pub content_right: f32,
    pub head_bottom: f32,
    pub name_label_baseline: f32,
    pub name_input: Rect,
    pub name_hint_baseline: f32,
    pub dd_label_baseline: f32,
    pub version_head: Rect,
    pub loader_head: Rect,
    pub ram_label_baseline: f32,
    pub ram_slider: Rect,
    pub ram_value_left: f32,
    pub ram_value_baseline: f32,
    pub ram_hint_baseline: f32,
    pub cancel_btn: Rect,
    pub create_btn: Rect,
}

pub fn compute_layout(card_w: f32, card_h: f32, fonts: &FontStore) -> ModalLayout {
    let card = card_rect(card_w, card_h);
    let content_left = card.left + CARD_PAD_X;
    let content_right = card.right - CARD_PAD_X;
    let content_top = card.top + CARD_PAD_TOP;
    let content_bottom = card.bottom - CARD_PAD_BOTTOM;

    // Head — Mono 10 eyebrow + Fraunces 36 title + Newsreader 14 sub
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let title_font = fonts.fraunces_axes(36.0, 50.0, 1.0, 300.0, None);
    let sub_font = fonts.newsreader(14.0);
    let (_, em) = eyebrow_font.metrics();
    let (_, tm) = title_font.metrics();
    let (_, sm) = sub_font.metrics();
    let eyebrow_baseline = content_top + (-em.ascent);
    let title_baseline = eyebrow_baseline + em.descent + 4.0 + (-tm.ascent);
    let sub_baseline = title_baseline + tm.descent + 4.0 + (-sm.ascent);
    let head_bottom = sub_baseline + sm.descent;

    let label_font = fonts.newsreader(13.0);
    let (_, lm) = label_font.metrics();
    let hint_font = fonts.newsreader(12.0);
    let (_, hm) = hint_font.metrics();

    // Field 1: Name
    let mut y = head_bottom + SECTION_GAP;
    let name_label_baseline = y + (-lm.ascent);
    let input_top = name_label_baseline + 4.0 + FIELD_LABEL_TO_INPUT_GAP;
    let input_h = 50.0;
    let name_input = Rect::from_xywh(content_left, input_top, content_right - content_left, input_h);
    let name_hint_baseline = input_top + input_h + 6.0 + (-hm.ascent);
    let name_field_bottom = name_hint_baseline + hm.descent;

    y = name_field_bottom + FIELD_GAP;

    // Field 2: Version + Loader row
    let dd_label_baseline = y + (-lm.ascent);
    let head_top = dd_label_baseline + 4.0 + FIELD_LABEL_TO_INPUT_GAP;
    let head_h = 40.0;
    let half_w = (content_right - content_left - 18.0) * 0.5;
    let version_head = Rect::from_xywh(content_left, head_top, half_w, head_h);
    let loader_head = Rect::from_xywh(content_left + half_w + 18.0, head_top, half_w, head_h);

    y = head_top + head_h + FIELD_GAP;

    // Field 3: RAM allocation
    let ram_label_baseline = y + (-lm.ascent);
    let row_top = ram_label_baseline + 4.0 + FIELD_LABEL_TO_INPUT_GAP;
    let row_h = 32.0;
    let value_w = 84.0;
    let ram_slider = Rect::from_xywh(
        content_left,
        row_top,
        content_right - content_left - value_w - 18.0,
        row_h,
    );
    let ram_value_left = ram_slider.right + 18.0;
    let ram_value_baseline = row_top + row_h * 0.5
        + fonts.fraunces_axes(22.0, 50.0, 0.0, 300.0, None).metrics().1.cap_height * 0.5;
    let ram_hint_baseline = row_top + row_h + 6.0 + (-hm.ascent);

    // Footer — Cancel + Create
    let create_w: f32 = 150.0;
    let create_h: f32 = 50.0;
    let cancel_w: f32 = 90.0;
    let cancel_h: f32 = 38.0;
    let foot_top = content_bottom - create_h.max(cancel_h);
    let create_x = content_right - create_w;
    let cancel_x = create_x - 14.0 - cancel_w;
    let create_btn = Rect::from_xywh(create_x, foot_top, create_w, create_h);
    let cancel_btn = Rect::from_xywh(
        cancel_x,
        foot_top + (create_h - cancel_h) * 0.5,
        cancel_w,
        cancel_h,
    );

    ModalLayout {
        card,
        content_left,
        content_right,
        head_bottom,
        name_label_baseline,
        name_input,
        name_hint_baseline,
        dd_label_baseline,
        version_head,
        loader_head,
        ram_label_baseline,
        ram_slider,
        ram_value_left,
        ram_value_baseline,
        ram_hint_baseline,
        cancel_btn,
        create_btn,
    }
}

/// Approximate card height — kept for the static `card_rect` sizing.
/// `compute_layout` is the canonical source for inner widget rects.
fn card_total_height() -> f32 {
    // Conservative: head 76 + section + name 95 + gap + dd row 61 + gap +
    // ram 65 + section + footer 58 + paddings.
    CARD_PAD_TOP + 76.0 + SECTION_GAP
        + 95.0 + FIELD_GAP + 61.0 + FIELD_GAP + 65.0
        + SECTION_GAP + FOOTER_PAD_TOP + 38.0 + CARD_PAD_BOTTOM
}

pub fn widget_bounds(card_w: f32, card_h: f32, fonts: &FontStore) -> Vec<(Slot, Rect)> {
    let l = compute_layout(card_w, card_h, fonts);
    vec![
        (Slot::Version, l.version_head),
        (Slot::Loader, l.loader_head),
        (Slot::Ram, l.ram_slider),
        (Slot::Cancel, l.cancel_btn),
        (Slot::Create, l.create_btn),
    ]
}

/// Click-anywhere-outside dismiss test — the shroud rect is the entire
/// card content area minus the card itself.
pub fn shroud_consumes(mouse: (f32, f32), card_w: f32, card_h: f32) -> bool {
    let card = card_rect(card_w, card_h);
    !card.contains(Point::new(mouse.0, mouse.1))
}

// ────────────────────────────────────────────────────────────────────────
// Render
// ────────────────────────────────────────────────────────────────────────

pub fn draw_modal(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    _time: f32,
    settings: &Settings,
    state: &NewInstanceModalState,
) {
    if !state.open && state.anim <= 0.001 {
        return;
    }
    let anim = CubicBezier::SILK.eval(state.anim.clamp(0.0, 1.0));

    // Layer 1 — shroud (160ms opacity ramp; we share the 240ms anim and
    // clamp to >= ~67% when the card is at its peak so the shroud finishes
    // first). Use a simple linear scale for visual smoothness.
    let shroud_alpha = (state.anim / 0.67).clamp(0.0, 1.0);
    draw_shroud(canvas, card_w, card_h, shroud_alpha);

    // Layer 2 — card with entrance transform.
    let card = card_rect(card_w, card_h);
    let cx = (card.left + card.right) * 0.5;
    let cy = (card.top + card.bottom) * 0.5;
    let scale = 0.97 + 0.03 * anim;
    let ty = 10.0 * (1.0 - anim);

    let saved = canvas.save();
    canvas.translate((cx, cy + ty));
    canvas.scale((scale, scale));
    canvas.translate((-cx, -cy));

    let mut layer_paint = Paint::default();
    layer_paint.set_alpha_f(anim);
    canvas.save_layer(
        &SaveLayerRec::default()
            .bounds(&card)
            .paint(&layer_paint),
    );

    draw_card(canvas, fonts, &card, _time, settings, state);

    canvas.restore();
    canvas.restore_to_count(saved);

    // Layer 3 — any open dropdown menu, drawn after the card so it sits on
    // top of the form.
    if let Some(slot) = state.open_dropdown() {
        if let Some(opts) = state.dropdown_options(slot) {
            if let Some(head) = widget_bounds(card_w, card_h, fonts)
                .into_iter()
                .find_map(|(s, r)| if s == slot { Some(r) } else { None })
            {
                if let Some(state_ref) = state.dropdown_state(slot) {
                    let (menu, flip_up) = menu_layout(head, opts.len(), card_h);
                    draw_vdrop_menu(canvas, menu, flip_up, &opts, state_ref, fonts);
                }
            }
        }
    }
}

fn draw_shroud(canvas: &Canvas, card_w: f32, card_h: f32, alpha: f32) {
    let bounds = Rect::from_xywh(0.0, 0.0, card_w, card_h);

    // Backdrop blur 4px (CSS spec) — sigma 2.
    if let Some(blur) = image_filters::blur((2.0, 2.0), TileMode::Clamp, None, None) {
        let saved = canvas.save();
        canvas.clip_rect(bounds, ClipOp::Intersect, true);
        let rec = SaveLayerRec::default().bounds(&bounds).backdrop(&blur);
        canvas.save_layer(&rec);

        // Radial dim gradient, alpha ramped by `alpha`.
        let cx = card_w * 0.5;
        let cy = card_h * 0.5;
        let radius = (card_w.max(card_h)) * 0.7;
        if let Some(shader) = gradient_shader::radial(
            Point::new(cx, cy),
            radius,
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new(8.0 / 255.0, 2.0 / 255.0, 6.0 / 255.0, 0.55 * alpha),
                    Color4f::new(8.0 / 255.0, 2.0 / 255.0, 6.0 / 255.0, 0.85 * alpha),
                ],
                None,
            ),
            Some(&[0.0_f32, 1.0][..]),
            TileMode::Clamp,
            None,
            None,
        ) {
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_shader(shader);
            canvas.draw_rect(bounds, &p);
        }

        canvas.restore();
        canvas.restore_to_count(saved);
    }
}

fn draw_card(
    canvas: &Canvas,
    fonts: &FontStore,
    card: &Rect,
    _time: f32,
    settings: &Settings,
    state: &NewInstanceModalState,
) {
    let rrect = RRect::new_rect_xy(*card, CARD_RADIUS, CARD_RADIUS);

    // Drop shadow stack (CSS modal-card box-shadow):
    //   0 32 80 rgba(0,0,0,0.55), 0 8 28 rgba(0,0,0,0.4),
    //   0 0 60 rgba(180,116,145,0.12), inset 0 0 0 0.5 rim
    draw_card_shadows(canvas, card, &rrect);

    // Refract — backdrop blur 40 + dark wine fill.
    if let Some(blur) = image_filters::blur((20.0, 20.0), TileMode::Clamp, None, None) {
        let saved = canvas.save();
        canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
        let rec = SaveLayerRec::default().bounds(card).backdrop(&blur);
        canvas.save_layer(&rec);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (Point::new(card.left, card.top), Point::new(card.left, card.bottom)),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new(38.0 / 255.0, 18.0 / 255.0, 30.0 / 255.0, 0.92),
                    Color4f::new(28.0 / 255.0, 12.0 / 255.0, 22.0 / 255.0, 0.95),
                ],
                None,
            ),
            Some(&[0.0_f32, 1.0][..]),
            TileMode::Clamp,
            None,
            None,
        ) {
            fill.set_shader(shader);
            canvas.draw_rrect(rrect, &fill);
        }
        canvas.restore();
        canvas.restore_to_count(saved);
    }

    // Tint: 135° linear + top radial fade.
    draw_card_tint(canvas, card, &rrect);

    // Hairline rim
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.18),
        None,
    );
    let inset_rect = card.with_inset((0.5, 0.5));
    canvas.draw_rrect(
        RRect::new_rect_xy(inset_rect, CARD_RADIUS - 0.5, CARD_RADIUS - 0.5),
        &rim,
    );

    // Inner content
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
    draw_inner(canvas, fonts, card, settings, state);
    canvas.restore_to_count(saved);
}

fn draw_card_shadows(canvas: &Canvas, card: &Rect, rrect: &RRect) {
    // Big black drop
    let mut s1 = Paint::default();
    s1.set_anti_alias(true);
    s1.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55), None);
    s1.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 40.0, false));
    canvas.draw_rrect(
        RRect::new_rect_xy(card.with_offset((0.0, 32.0)), CARD_RADIUS, CARD_RADIUS),
        &s1,
    );
    // Closer drop
    let mut s2 = Paint::default();
    s2.set_anti_alias(true);
    s2.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.4), None);
    s2.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 14.0, false));
    canvas.draw_rrect(
        RRect::new_rect_xy(card.with_offset((0.0, 8.0)), CARD_RADIUS, CARD_RADIUS),
        &s2,
    );
    // Berry bloom
    let mut s3 = Paint::default();
    s3.set_anti_alias(true);
    s3.set_color4f(
        Color4f::new(180.0 / 255.0, 116.0 / 255.0, 145.0 / 255.0, 0.12),
        None,
    );
    s3.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 30.0, false));
    canvas.draw_rrect(*rrect, &s3);
}

fn draw_card_tint(canvas: &Canvas, card: &Rect, rrect: &RRect) {
    // 135° linear (rose 0.10 → berry 0.06 → lav 0.10)
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(card.left, card.top),
            Point::new(card.right, card.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10),
                Color4f::new(180.0 / 255.0, 116.0 / 255.0, 145.0 / 255.0, 0.06),
                Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, 0.10),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.40, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(shader);
        canvas.draw_rrect(*rrect, &p);
    }
    // Top radial fade — warm-white peak above the card
    let cx = (card.left + card.right) * 0.5;
    let cy = card.top - card.height() * 0.2;
    if let Some(shader) = gradient_shader::radial(
        Point::new(cx, cy),
        card.width() * 0.7,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 0.14),
                Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 0.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_shader(shader);
        canvas.draw_rrect(*rrect, &p);
    }
}

fn draw_inner(
    canvas: &Canvas,
    fonts: &FontStore,
    card: &Rect,
    settings: &Settings,
    state: &NewInstanceModalState,
) {
    // Re-derive layout from the card's parent extents (the layout was
    // computed against the card-content rect, so we recover those by
    // adding 2× the inset back). card.left / card.top already include
    // the centering offset, so this matches `compute_layout` exactly.
    let card_w = card.width() + 2.0 * card.left;
    let card_h = card.height() + 2.0 * card.top;
    let layout = compute_layout(card_w, card_h, fonts);

    // Head
    draw_head(canvas, fonts, layout.content_left, layout.content_right, card.top + CARD_PAD_TOP);

    // Body fields
    draw_name_field(
        canvas,
        fonts,
        &layout,
        &state.name,
        state.name_focused,
        state.name_focus_time,
        state.name_error,
    );
    draw_dropdown_row(canvas, fonts, &layout, state, settings);
    draw_ram_field(canvas, fonts, &layout, &state.ram, settings);

    // Footer
    draw_footer(canvas, fonts, &layout, state, settings);
}

fn draw_head(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    _right: f32,
    top: f32,
) -> f32 {
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let mut eb_paint = Paint::default();
    eb_paint.set_anti_alias(true);
    eb_paint.set_color(ACCENT_BERRY);
    let (_, em) = eyebrow_font.metrics();
    let eb_baseline = top + (-em.ascent);
    text::draw_tracked_em(
        canvas,
        "NEW INSTANCE",
        (left, eb_baseline),
        &eyebrow_font,
        &eb_paint,
        0.18,
    );

    // Title — Fraunces 36 with "Begin a new world"
    let title_font = fonts.fraunces_axes(36.0, 50.0, 1.0, 300.0, None);
    let mut t_paint = Paint::default();
    t_paint.set_anti_alias(true);
    t_paint.set_color(ACCENT_PEARL_HOT);
    let (_, tm) = title_font.metrics();
    let title_top = eb_baseline + em.descent + 4.0;
    let title_baseline = title_top + (-tm.ascent);
    canvas.draw_str("Begin a new world", (left, title_baseline), &title_font, &t_paint);

    // Subhead — italic Newsreader 14
    let sub_font = fonts.newsreader(14.0);
    let mut sub_paint = Paint::default();
    sub_paint.set_anti_alias(true);
    sub_paint.set_color(TEXT_MAUVE);
    let (_, sm) = sub_font.metrics();
    let sub_top = title_baseline + tm.descent + 4.0;
    let sub_baseline = sub_top + (-sm.ascent);
    canvas.draw_str(
        "name it. choose a version. the rest can change later.",
        (left, sub_baseline),
        &sub_font,
        &sub_paint,
    );

    sub_baseline + sm.descent
}

#[allow(clippy::too_many_arguments)]
fn draw_name_field(
    canvas: &Canvas,
    fonts: &FontStore,
    l: &ModalLayout,
    value: &str,
    focused: bool,
    focus_time: f32,
    error: bool,
) {
    draw_field_label_at(canvas, fonts, l.content_left, l.name_label_baseline, "name");
    draw_text_input(
        canvas,
        fonts,
        l.name_input,
        value,
        "Quiet Atrium, Slow Theatre, Pearl Antechamber…",
        focused,
        focus_time,
        error,
    );
    if error {
        // Replace the gentle hint with an ember-tinted error message.
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from_argb(0xFF, 0xD4, 0x88, 0x9A));
        let font = fonts.newsreader(12.0);
        canvas.draw_str(
            "name required",
            (l.content_left, l.name_hint_baseline),
            &font,
            &paint,
        );
    } else {
        draw_field_hint_at(
            canvas,
            fonts,
            l.content_left,
            l.name_hint_baseline,
            "a name you'll recognize at 2 a.m.",
        );
    }
}

fn draw_dropdown_row(
    canvas: &Canvas,
    fonts: &FontStore,
    l: &ModalLayout,
    state: &NewInstanceModalState,
    settings: &Settings,
) {
    draw_field_label_at(
        canvas,
        fonts,
        l.content_left,
        l.dd_label_baseline,
        "minecraft version",
    );
    draw_field_label_at(
        canvas,
        fonts,
        l.loader_head.left,
        l.dd_label_baseline,
        "mod loader",
    );

    let value_a = state
        .mc_versions
        .get(state.version.selected)
        .map(|s| s.as_str())
        .unwrap_or("");
    let value_b = LOADERS.get(state.loader.selected).copied().unwrap_or("");
    let time = 0.0;
    draw_vdrop_head(canvas, l.version_head, value_a, &state.version, time, settings, fonts);
    draw_vdrop_head(canvas, l.loader_head, value_b, &state.loader, time, settings, fonts);
}

fn draw_ram_field(
    canvas: &Canvas,
    fonts: &FontStore,
    l: &ModalLayout,
    state: &VsliderState,
    settings: &Settings,
) {
    draw_field_label_at(
        canvas,
        fonts,
        l.content_left,
        l.ram_label_baseline,
        "ram allocation",
    );
    draw_vslider(canvas, l.ram_slider, state, 0.0, settings);

    // Value pill — Fraunces 22 number + Mono 12 "GB" suffix.
    let num_font = fonts.fraunces_axes(22.0, 50.0, 0.0, 300.0, None);
    let unit_font = fonts.jetbrains_mono(12.0);
    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color(TEXT_PEARL);
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color(TEXT_MAUVE);

    let num_str = format!("{}", state.value as i32);
    let (num_advance, _) = num_font.measure_str(&num_str, Some(&num_paint));
    let unit_str = "GB";
    let unit_advance = text::measure_tracked_em(&unit_font, unit_str, 0.16);
    let total = num_advance + 4.0 + unit_advance;
    let value_x_left = l.content_right - total;
    canvas.draw_str(&num_str, (value_x_left, l.ram_value_baseline), &num_font, &num_paint);
    text::draw_tracked_em(
        canvas,
        unit_str,
        (value_x_left + num_advance + 4.0, l.ram_value_baseline),
        &unit_font,
        &unit_paint,
        0.16,
    );

    let hint = match state.value as i32 {
        0..=2 => "small and quick",
        3..=6 => "comfortable",
        7..=10 => "roomy — for shaders & many mods",
        _ => "palatial",
    };
    draw_field_hint_at(canvas, fonts, l.content_left, l.ram_hint_baseline, hint);
}

fn draw_text_input(
    canvas: &Canvas,
    fonts: &FontStore,
    bounds: Rect,
    value: &str,
    placeholder: &str,
    focused: bool,
    focus_time: f32,
    error: bool,
) {
    let rrect = RRect::new_rect_xy(bounds, 10.0, 10.0);

    // Background — slightly brighter when focused.
    let bg_alpha = if focused { 0.08 } else { 0.05 };
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, bg_alpha),
        None,
    );
    canvas.draw_rrect(rrect, &bg);

    // Focus / error glow.
    if focused || error {
        let glow_color = if error {
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.20)
        } else {
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.12)
        };
        let mut focus_glow = Paint::default();
        focus_glow.set_anti_alias(true);
        focus_glow.set_style(PaintStyle::Stroke);
        focus_glow.set_stroke_width(3.0);
        focus_glow.set_color4f(glow_color, None);
        focus_glow.set_mask_filter(skia_safe::MaskFilter::blur(
            skia_safe::BlurStyle::Normal,
            7.0,
            false,
        ));
        canvas.draw_rrect(rrect, &focus_glow);
    }

    // Border — ember when error, otherwise rose (lifted alpha on focus).
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    if error {
        border.set_color4f(
            Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.70),
            None,
        );
    } else {
        let border_alpha = if focused { 0.50 } else { 0.16 };
        border.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, border_alpha),
            None,
        );
    }
    canvas.draw_rrect(rrect, &border);

    // Placeholder shows only when the field is empty AND blurred — the
    // moment the user clicks in to focus the field, the hint disappears
    // (the caret + lit border take over the "type here" signal). Typing
    // anything also hides it, of course.
    let show_placeholder = value.is_empty() && !focused;
    let font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    if show_placeholder {
        paint.set_color(Color::from_argb(0x59, 0xC4, 0xAF, 0xB5));
    } else {
        paint.set_color(ACCENT_PEARL_HOT);
    }
    let (_, m) = font.metrics();
    let cy = (bounds.top + bounds.bottom) * 0.5;
    let baseline = cy + m.cap_height * 0.5;
    if !value.is_empty() {
        canvas.draw_str(value, (bounds.left + 16.0, baseline), &font, &paint);
    } else if show_placeholder {
        canvas.draw_str(placeholder, (bounds.left + 16.0, baseline), &font, &paint);
    }
    // else: focused + empty → just the caret, no text.

    // Caret — blink at 1 Hz, sit just past the typed text. When empty, sits
    // at the left padding edge.
    if focused {
        let blink = ((focus_time * 1.6).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
        let alpha = blink * blink; // sharper on/off than a sine
        if alpha > 0.05 {
            let typed_w = if value.is_empty() {
                0.0
            } else {
                font.measure_str(value, Some(&paint)).0
            };
            let caret_x = bounds.left + 16.0 + typed_w + 1.0;
            let caret_top = cy - m.cap_height * 0.5 - 2.0;
            let caret_bottom = cy + m.cap_height * 0.5 + 2.0;
            let mut caret_paint = Paint::default();
            caret_paint.set_anti_alias(true);
            caret_paint.set_style(PaintStyle::Stroke);
            caret_paint.set_stroke_width(1.5);
            caret_paint.set_color4f(
                Color4f::new(255.0 / 255.0, 246.0 / 255.0, 240.0 / 255.0, alpha),
                None,
            );
            canvas.draw_line((caret_x, caret_top), (caret_x, caret_bottom), &caret_paint);
        }
    }
}

fn draw_field_label_at(canvas: &Canvas, fonts: &FontStore, left: f32, baseline: f32, text: &str) {
    let font = fonts.newsreader(13.0);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(TEXT_MID_PEARL);
    canvas.draw_str(text, (left, baseline), &font, &paint);
}

fn draw_field_hint_at(canvas: &Canvas, fonts: &FontStore, left: f32, baseline: f32, text: &str) {
    let font = fonts.newsreader(12.0);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(TEXT_MAUVE_DEEP);
    canvas.draw_str(text, (left, baseline), &font, &paint);
}

fn draw_footer(
    canvas: &Canvas,
    fonts: &FontStore,
    l: &ModalLayout,
    state: &NewInstanceModalState,
    settings: &Settings,
) {
    // Border-top hairline (CSS `.modal-foot { border-top: 1px hairline; padding-top: 20 }`).
    let border_y = l.create_btn.top - FOOTER_PAD_TOP;
    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line((l.content_left, border_y), (l.content_right, border_y), &div);

    draw_vghost_btn(canvas, l.cancel_btn, "Cancel", &state.cancel_btn, GhostKind::Pearl, fonts);
    draw_vbtn(
        canvas,
        l.create_btn,
        "Create world",
        &state.create_btn,
        0.0,
        settings.motion_speed,
        fonts,
        true,
    );
}
