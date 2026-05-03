//! Dev overlay — tweaks panel for the five `Settings` tokens.
//!
//! Step 15 partial: ships the most useful sub-panel (the live tuning of
//! `--motion-speed` / `--breath-amp` / `--density` / `--warmth` /
//! `--accent-hue-shift`). The state-picker / layout-picker / error-picker
//! the prototype also exposes are launcher-screen-specific and add little
//! value for parity-tuning work — they can land later.
//!
//! Visible only when the launcher is started with `--dev`. Renders as a
//! floating dark-glass panel anchored to the top-right of the card,
//! drawn *after* everything else (including the new-instance modal) so
//! devs can tune the backdrop while any UI is up.
//!
//! Slider values flow back to `Settings` each frame via
//! `apply_to_settings`. Density changes also re-spawn the particle pools
//! via `Backdrop::resize`; everything else is read every frame so it
//! takes effect immediately.

use ewo_core::Settings;
use skia_safe::{
    canvas::SaveLayerRec, gradient_shader, image_filters, BlurStyle, Canvas, ClipOp, Color,
    Color4f, MaskFilter, Paint, PaintStyle, Point, RRect, Rect, TileMode,
};

use crate::screens::launching::LaunchError;
use crate::text::{self, FontStore};
use crate::widgets::{
    draw_vghost_btn, draw_vslider, GhostKind, VghostBtnState, VsliderState,
};

const PANEL_RADIUS: f32 = 14.0;
const PANEL_W: f32 = 280.0;
const PANEL_PAD: f32 = 18.0;
const PANEL_RIGHT_INSET: f32 = 16.0;
const PANEL_TOP_INSET: f32 = 60.0; // sit below the tab bar

const ROW_GAP: f32 = 16.0;
const LABEL_TO_SLIDER_GAP: f32 = 6.0;
const SLIDER_HEIGHT: f32 = 28.0;
const RESET_BUTTON_HEIGHT: f32 = 32.0;

const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MID_PEARL: Color = Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    MotionSpeed,
    BreathAmp,
    Density,
    Warmth,
    AccentHueShift,
    Reset,
    /// Toggle vsync on/off. Off mode uncaps the frame-rate so the FPS HUD
    /// can validate the 500fps OLED target. Persists across launches via
    /// `App::dev_vsync` (in-memory only — no on-disk preference).
    VsyncToggle,
    /// Cycle through the four `LaunchError` states (None → Rose → Recede →
    /// Shimmer → None). Lets the user see the pbar error variants without
    /// needing a real launch failure.
    SimError,
}

#[derive(Debug, Clone)]
pub struct DevOverlayState {
    pub motion_speed: VsliderState,
    pub breath_amp: VsliderState,
    pub density: VsliderState,
    pub warmth: VsliderState,
    pub accent_hue_shift: VsliderState,
    pub reset_btn: VghostBtnState,
    pub vsync_btn: VghostBtnState,
    pub vsync: bool,
    pub sim_error_btn: VghostBtnState,
    /// Currently-active simulated error variant (`None` = no simulation).
    /// Cycles through `[None, Rose, Recede, Shimmer]` on each click.
    pub sim_error: Option<LaunchError>,
}

impl Default for DevOverlayState {
    fn default() -> Self {
        let s = Settings::default();
        Self::from_settings(&s)
    }
}

impl DevOverlayState {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            motion_speed: VsliderState::new(s.motion_speed, 0.1, 3.0).with_step(0.1),
            breath_amp: VsliderState::new(s.breath_amp, 0.0, 2.0).with_step(0.1),
            density: VsliderState::new(s.density, 0.0, 2.0).with_step(0.1),
            warmth: VsliderState::new(s.warmth, 0.0, 1.0).with_step(0.05),
            accent_hue_shift: VsliderState::new(s.accent_hue_shift, -180.0, 180.0).with_step(5.0),
            reset_btn: VghostBtnState::default(),
            vsync_btn: VghostBtnState::default(),
            vsync: true,
            sim_error_btn: VghostBtnState::default(),
            sim_error: None,
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.reset_btn.tick(dt);
        self.vsync_btn.tick(dt);
        self.sim_error_btn.tick(dt);
    }

    /// Cycle the simulated-error variant: None → Rose → Recede → Shimmer → None.
    pub fn cycle_sim_error(&mut self) {
        self.sim_error = match self.sim_error {
            None => Some(LaunchError::Rose),
            Some(LaunchError::Rose) => Some(LaunchError::Recede),
            Some(LaunchError::Recede) => Some(LaunchError::Shimmer),
            Some(LaunchError::Shimmer) => None,
        };
    }

    /// Mutate `settings` to match the current slider values. Returns
    /// `true` when `density` changed since the last call — caller should
    /// re-init particle pools (`Backdrop::resize`) on `true`.
    pub fn apply_to_settings(&self, settings: &mut Settings) -> bool {
        let density_changed = (self.density.value - settings.density).abs() > 1e-3;
        settings.motion_speed = self.motion_speed.value;
        settings.breath_amp = self.breath_amp.value;
        settings.density = self.density.value;
        settings.warmth = self.warmth.value;
        settings.accent_hue_shift = self.accent_hue_shift.value;
        density_changed
    }

    pub fn reset_to_defaults(&mut self) {
        let s = Settings::default();
        *self = Self::from_settings(&s);
    }
}

// ────────────────────────────────────────────────────────────────────────
// Layout
// ────────────────────────────────────────────────────────────────────────

const ROW_DEFS: &[(Slot, &str)] = &[
    (Slot::MotionSpeed, "motion speed"),
    (Slot::BreathAmp, "breath amp"),
    (Slot::Density, "density"),
    (Slot::Warmth, "warmth"),
    (Slot::AccentHueShift, "accent hue shift"),
];

fn panel_rect(card_w: f32, _card_h: f32) -> Rect {
    let body_h = panel_total_height();
    Rect::from_xywh(
        card_w - PANEL_RIGHT_INSET - PANEL_W,
        PANEL_TOP_INSET,
        PANEL_W,
        body_h,
    )
}

fn panel_total_height() -> f32 {
    // Title (16) + gap + N rows × (label_h + gap + slider_h + ROW_GAP)
    // + (vsync | reset) bottom row + sim-error full-width row + bottom pad
    let title_h = 16.0;
    let row_h = 14.0 + LABEL_TO_SLIDER_GAP + SLIDER_HEIGHT;
    let body = title_h + 14.0 + (ROW_DEFS.len() as f32) * (row_h + ROW_GAP);
    PANEL_PAD + body + 8.0 + RESET_BUTTON_HEIGHT + 8.0 + RESET_BUTTON_HEIGHT + PANEL_PAD
}

pub fn widget_bounds(card_w: f32, card_h: f32) -> Vec<(Slot, Rect)> {
    let panel = panel_rect(card_w, card_h);
    let content_left = panel.left + PANEL_PAD;
    let content_right = panel.right - PANEL_PAD;
    let content_top = panel.top + PANEL_PAD;

    let mut out = Vec::with_capacity(ROW_DEFS.len() + 1);

    // Title height
    let title_h = 16.0;
    let mut y = content_top + title_h + 14.0;

    let label_h = 14.0;
    for (slot, _) in ROW_DEFS.iter() {
        let slider_top = y + label_h + LABEL_TO_SLIDER_GAP;
        out.push((
            *slot,
            Rect::from_xywh(content_left, slider_top, content_right - content_left, SLIDER_HEIGHT),
        ));
        y = slider_top + SLIDER_HEIGHT + ROW_GAP;
    }

    // Bottom rows. First row: vsync (left) + Reset (right) split 50/50.
    // Second row: full-width sim-error cycle button.
    let bottom_top = y - ROW_GAP + 8.0;
    let total_w = content_right - content_left;
    let split_gap = 10.0;
    let half_w = (total_w - split_gap) * 0.5;
    out.push((
        Slot::VsyncToggle,
        Rect::from_xywh(content_left, bottom_top, half_w, RESET_BUTTON_HEIGHT),
    ));
    out.push((
        Slot::Reset,
        Rect::from_xywh(
            content_left + half_w + split_gap,
            bottom_top,
            half_w,
            RESET_BUTTON_HEIGHT,
        ),
    ));
    let sim_top = bottom_top + RESET_BUTTON_HEIGHT + 8.0;
    out.push((
        Slot::SimError,
        Rect::from_xywh(content_left, sim_top, total_w, RESET_BUTTON_HEIGHT),
    ));

    out
}

/// Returns the panel rect for hover/cursor checks — `main.rs` uses this
/// to know when to suppress underlying-screen pointer-cursor changes.
pub fn panel_bounds(card_w: f32, card_h: f32) -> Rect {
    panel_rect(card_w, card_h)
}

// ────────────────────────────────────────────────────────────────────────
// Render
// ────────────────────────────────────────────────────────────────────────

/// Read-only frame timing snapshot rendered in the dev overlay header.
/// `fps == 0.0` and `frame_ms == 0.0` render as `—` placeholders.
#[derive(Copy, Clone, Debug, Default)]
pub struct FrameStats {
    pub fps: f32,
    pub frame_ms: f32,
    pub worst_ms: f32,
}

pub fn draw_dev_overlay(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    time: f32,
    settings: &Settings,
    state: &DevOverlayState,
    stats: FrameStats,
) {
    let panel = panel_rect(card_w, card_h);
    let rrect = RRect::new_rect_xy(panel, PANEL_RADIUS, PANEL_RADIUS);

    draw_chrome(canvas, &panel, &rrect);

    // Inner content
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
    draw_inner(canvas, fonts, &panel, time, settings, state, stats);
    canvas.restore_to_count(saved);
}

fn draw_chrome(canvas: &Canvas, panel: &Rect, rrect: &RRect) {
    // Drop shadow — `0 12 32 rgba(0,0,0,0.6)`
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 16.0, false));
    canvas.draw_rrect(
        RRect::new_rect_xy(panel.with_offset((0.0, 12.0)), PANEL_RADIUS, PANEL_RADIUS),
        &shadow,
    );

    // Backdrop blur 20 + dark wine fill (CSS: rgba(10,0,6,0.72) bg).
    if let Some(blur) = image_filters::blur((10.0, 10.0), TileMode::Clamp, None, None) {
        let saved = canvas.save();
        canvas.clip_rrect(*rrect, Some(ClipOp::Intersect), Some(true));
        let rec = SaveLayerRec::default().bounds(panel).backdrop(&blur);
        canvas.save_layer(&rec);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_color4f(
            Color4f::new(10.0 / 255.0, 0.0 / 255.0, 6.0 / 255.0, 0.72),
            None,
        );
        canvas.draw_rrect(*rrect, &fill);
        canvas.restore();
        canvas.restore_to_count(saved);
    }

    // Subtle top warm-fade (matches the launcher's overall aesthetic)
    let cx = (panel.left + panel.right) * 0.5;
    if let Some(shader) = gradient_shader::radial(
        Point::new(cx, panel.top - panel.height() * 0.2),
        panel.width() * 0.7,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.06),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.0),
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

    // Hairline rim
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.20),
        None,
    );
    let inset = panel.with_inset((0.5, 0.5));
    canvas.draw_rrect(
        RRect::new_rect_xy(inset, PANEL_RADIUS - 0.5, PANEL_RADIUS - 0.5),
        &rim,
    );
}

fn draw_inner(
    canvas: &Canvas,
    fonts: &FontStore,
    panel: &Rect,
    time: f32,
    settings: &Settings,
    state: &DevOverlayState,
    stats: FrameStats,
) {
    let content_left = panel.left + PANEL_PAD;
    let content_right = panel.right - PANEL_PAD;
    let content_top = panel.top + PANEL_PAD;

    // Title eyebrow + frame stats
    let title_font = fonts.jetbrains_mono(10.0);
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color(Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5));
    let (_, tm) = title_font.metrics();
    let title_baseline = content_top + (-tm.ascent);
    text::draw_tracked_em(
        canvas,
        "TWEAKS · DEV",
        (content_left, title_baseline),
        &title_font,
        &title_paint,
        0.20,
    );

    // Right-aligned: "60 FPS · 16.6 MS"
    let stats_str = if stats.fps > 0.0 {
        format!(
            "{} FPS · {:.1} MS",
            stats.fps.round() as i32,
            stats.frame_ms,
        )
    } else {
        "— FPS · — MS".to_string()
    };
    // Color the stats by frame budget — green-tinted pearl when under
    // 16.6ms (60fps), warm champagne when 16.6-33ms, ember when >33ms.
    let stats_color = if stats.frame_ms == 0.0 || stats.frame_ms <= 16.7 {
        Color::from_argb(0xFF, 0xC9, 0xA5, 0xD4)
    } else if stats.frame_ms <= 33.4 {
        Color::from_argb(0xFF, 0xE8, 0xD4, 0xA8)
    } else {
        Color::from_argb(0xFF, 0xC9, 0x6A, 0x7A)
    };
    let mut stats_paint = Paint::default();
    stats_paint.set_anti_alias(true);
    stats_paint.set_color(stats_color);
    let stats_advance = text::measure_tracked_em(&title_font, &stats_str, 0.16);
    text::draw_tracked_em(
        canvas,
        &stats_str,
        (content_right - stats_advance, title_baseline),
        &title_font,
        &stats_paint,
        0.16,
    );

    // Worst-frame line below — small and dim.
    if stats.worst_ms > 0.0 {
        let worst_font = fonts.jetbrains_mono(9.0);
        let mut worst_paint = Paint::default();
        worst_paint.set_anti_alias(true);
        worst_paint.set_color(Color::from_argb(0xFF, 0x6B, 0x55, 0x5C));
        let (_, wm) = worst_font.metrics();
        let worst_baseline = title_baseline + tm.descent + 2.0 + (-wm.ascent);
        let worst_str = format!("worst {:.1} ms", stats.worst_ms);
        let worst_advance = text::measure_tracked_em(&worst_font, &worst_str, 0.10);
        text::draw_tracked_em(
            canvas,
            &worst_str,
            (content_right - worst_advance, worst_baseline),
            &worst_font,
            &worst_paint,
            0.10,
        );
    }

    // Hairline below title
    let div_y = title_baseline + tm.descent + 8.0;
    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10),
        None,
    );
    canvas.draw_line((content_left, div_y), (content_right, div_y), &div);

    // Rows
    let bounds = widget_bounds(panel.width() + panel.left * 2.0, panel.height() + panel.top * 2.0);
    let label_font = fonts.newsreader(13.0);
    let value_font = fonts.jetbrains_mono(11.0);
    let (_, lm) = label_font.metrics();

    for (slot, label) in ROW_DEFS.iter() {
        let slider_rect = bounds
            .iter()
            .find(|(s, _)| s == slot)
            .map(|(_, r)| *r)
            .unwrap_or_default();

        // Label is positioned just above the slider rect.
        let label_top = slider_rect.top - LABEL_TO_SLIDER_GAP - 14.0;
        let label_baseline = label_top + (-lm.ascent);
        let mut label_paint = Paint::default();
        label_paint.set_anti_alias(true);
        label_paint.set_color(TEXT_MID_PEARL);
        canvas.draw_str(*label, (content_left, label_baseline), &label_font, &label_paint);

        // Right-aligned value pill (mono 11)
        let (slider_state, value_str) = state_and_value_for(*slot, state);
        let mut value_paint = Paint::default();
        value_paint.set_anti_alias(true);
        value_paint.set_color(TEXT_PEARL);
        let (advance, _) = value_font.measure_str(&value_str, Some(&value_paint));
        let (_, vm) = value_font.metrics();
        let value_baseline = label_top + (-vm.ascent) + 1.0;
        canvas.draw_str(
            &value_str,
            (content_right - advance, value_baseline),
            &value_font,
            &value_paint,
        );

        draw_vslider(canvas, slider_rect, slider_state, time, settings);
    }

    // Vsync toggle (left) + Reset button (right)
    if let Some((_, vsync_rect)) = bounds.iter().find(|(s, _)| *s == Slot::VsyncToggle) {
        let label = if state.vsync {
            "VSync · on"
        } else {
            "VSync · off"
        };
        draw_vghost_btn(
            canvas,
            *vsync_rect,
            label,
            &state.vsync_btn,
            GhostKind::Pearl,
            fonts,
        );
    }
    if let Some((_, reset_rect)) = bounds.iter().find(|(s, _)| *s == Slot::Reset) {
        draw_vghost_btn(
            canvas,
            *reset_rect,
            "Reset",
            &state.reset_btn,
            GhostKind::Pearl,
            fonts,
        );
    }

    // Simulate launch error — full-width cycling pill below vsync/reset.
    if let Some((_, sim_rect)) = bounds.iter().find(|(s, _)| *s == Slot::SimError) {
        let label = match state.sim_error {
            None => "Sim error · None",
            Some(LaunchError::Rose) => "Sim error · Rose",
            Some(LaunchError::Recede) => "Sim error · Recede",
            Some(LaunchError::Shimmer) => "Sim error · Shimmer",
        };
        let kind = if state.sim_error.is_some() {
            GhostKind::Danger
        } else {
            GhostKind::Pearl
        };
        draw_vghost_btn(canvas, *sim_rect, label, &state.sim_error_btn, kind, fonts);
    }

    let _ = TEXT_MAUVE;
}

fn state_and_value_for<'a>(
    slot: Slot,
    state: &'a DevOverlayState,
) -> (&'a VsliderState, String) {
    match slot {
        Slot::MotionSpeed => (
            &state.motion_speed,
            format!("{:.1}×", state.motion_speed.value),
        ),
        Slot::BreathAmp => (&state.breath_amp, format!("{:.1}", state.breath_amp.value)),
        Slot::Density => (&state.density, format!("{:.1}", state.density.value)),
        Slot::Warmth => (&state.warmth, format!("{:.2}", state.warmth.value)),
        Slot::AccentHueShift => (
            &state.accent_hue_shift,
            format!("{:.0}°", state.accent_hue_shift.value),
        ),
        Slot::Reset | Slot::VsyncToggle | Slot::SimError => {
            unreachable!("non-slider slot {:?} passed to state_and_value_for", slot)
        }
    }
}

pub fn slider_state_mut(state: &mut DevOverlayState, slot: Slot) -> Option<&mut VsliderState> {
    match slot {
        Slot::MotionSpeed => Some(&mut state.motion_speed),
        Slot::BreathAmp => Some(&mut state.breath_amp),
        Slot::Density => Some(&mut state.density),
        Slot::Warmth => Some(&mut state.warmth),
        Slot::AccentHueShift => Some(&mut state.accent_hue_shift),
        Slot::Reset | Slot::VsyncToggle | Slot::SimError => None,
    }
}
