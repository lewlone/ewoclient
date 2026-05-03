//! Launching screen — synthetic progress + log stream.
//!
//! The prototype's `Launching` component renders a launch-in-progress
//! sequence over `DURATION = 8s`. Internally a script of progress
//! keyframes (`PROGRESS_KEYFRAMES`) and log entries (`LOG_SCRIPT`) is
//! sampled at scaled time `t * SCRIPT_DURATION / DURATION`.
//!
//! This v1 port keeps the same script verbatim. State lives in
//! `LaunchingState` (the App owns it, ticks it, and resets it when the
//! user re-enters the screen). Rendering is read-only.
//!
//! CSS reference: `.screen-launching`, `.launching-eyebrow`,
//! `.launching-name`, `.launching-meta`, `.launching-percent`,
//! `.launching-progress-region`, `.launching-stage`, `.launching-log-panel`,
//! `.launching-log-head`, `.launching-log`, `.log-line` in `StyleSheet2`.

use ewo_core::Settings;
use skia_safe::{Canvas, Color, Color4f, Paint, PaintStyle, Rect};

use crate::text::{self, FontStore};
use crate::widgets::{draw_glass_panel, draw_pbar, draw_vstatus, PbarState, VstatusState};

const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MAUVE_DEEP: Color = Color::from_argb(0xFF, 0x6B, 0x55, 0x5C);
const TEXT_MID_PEARL: Color = Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5);
const ACCENT_ROSE: Color = Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5);
const ACCENT_BERRY: Color = Color::from_argb(0xFF, 0xB4, 0x74, 0x91);
const ACCENT_LAV: Color = Color::from_argb(0xFF, 0xC9, 0xA5, 0xD4);
const ACCENT_CHAMP: Color = Color::from_argb(0xFF, 0xE8, 0xD4, 0xA8);

/// Wall-clock seconds the synthetic launch takes end-to-end.
pub const DURATION_SECONDS: f32 = 8.0;
/// "Script units" — opaque time scale used by `LOG_SCRIPT` and
/// `PROGRESS_KEYFRAMES`. Maps to `DURATION_SECONDS` of real time.
const SCRIPT_DURATION: f32 = 2400.0;
/// Hold the "Ready · handing off" state for 1.5s before auto-returning.
pub const HANDOFF_HOLD_SECONDS: f32 = 1.5;

#[derive(Copy, Clone, Debug)]
pub enum LogLevel {
    Info,
    Warn,
    Ok,
    Dim,
}

#[derive(Copy, Clone, Debug)]
pub struct LogEntry {
    /// Position on the script-unit timeline. The launching tick samples
    /// `LOG_SCRIPT` against `t * SCRIPT_DURATION / DURATION_SECONDS`.
    pub t: f32,
    pub level: LogLevel,
    pub source: &'static str,
    pub message: &'static str,
}

#[derive(Copy, Clone, Debug)]
pub struct ProgressKeyframe {
    pub t: f32,
    pub p: f32,
    pub stage: &'static str,
}

const PROGRESS_KEYFRAMES: &[ProgressKeyframe] = &[
    ProgressKeyframe { t: 0.0, p: 0.0, stage: "preparing" },
    ProgressKeyframe { t: 200.0, p: 6.0, stage: "preparing" },
    ProgressKeyframe { t: 600.0, p: 28.0, stage: "verifying assets" },
    ProgressKeyframe { t: 1100.0, p: 52.0, stage: "loading mods" },
    ProgressKeyframe { t: 1400.0, p: 68.0, stage: "spawning JVM" },
    ProgressKeyframe { t: 1800.0, p: 84.0, stage: "starting engine" },
    ProgressKeyframe { t: 2200.0, p: 96.0, stage: "reaching title" },
    ProgressKeyframe { t: 2400.0, p: 100.0, stage: "ready" },
];

const LOG_SCRIPT: &[LogEntry] = &[
    LogEntry { t: 0.0,    level: LogLevel::Info, source: "launcher", message: "Resolving instance · Velvet Hours" },
    LogEntry { t: 80.0,   level: LogLevel::Info, source: "launcher", message: "Java 21.0.4 (Adoptium) · 6144 MB heap" },
    LogEntry { t: 160.0,  level: LogLevel::Info, source: "assets",   message: "Verifying asset index 1.20.4 (449 MB)" },
    LogEntry { t: 280.0,  level: LogLevel::Info, source: "assets",   message: "objects/ab/abf3… ok" },
    LogEntry { t: 360.0,  level: LogLevel::Info, source: "assets",   message: "objects/c1/c1d8… ok" },
    LogEntry { t: 440.0,  level: LogLevel::Info, source: "assets",   message: "objects/9e/9eaa… ok" },
    LogEntry { t: 540.0,  level: LogLevel::Dim,  source: "assets",   message: "… 6,318 objects verified" },
    LogEntry { t: 620.0,  level: LogLevel::Info, source: "mods",     message: "Loading 6 mods" },
    LogEntry { t: 700.0,  level: LogLevel::Info, source: "mods",     message: "sodium-0.5.8.jar" },
    LogEntry { t: 760.0,  level: LogLevel::Info, source: "mods",     message: "iris-1.7.0.jar" },
    LogEntry { t: 820.0,  level: LogLevel::Info, source: "mods",     message: "distant-horizons-2.0.4.jar" },
    LogEntry { t: 880.0,  level: LogLevel::Info, source: "mods",     message: "lithium-0.12.7.jar" },
    LogEntry { t: 940.0,  level: LogLevel::Info, source: "mods",     message: "continuity-3.0.0.jar" },
    LogEntry { t: 1000.0, level: LogLevel::Info, source: "mods",     message: "modmenu-9.0.0.jar" },
    LogEntry { t: 1100.0, level: LogLevel::Info, source: "jvm",      message: "Spawning JVM · -Xmx6G -Xms6G" },
    LogEntry { t: 1240.0, level: LogLevel::Info, source: "jvm",      message: "Native libraries linked · LWJGL 3.3.3" },
    LogEntry { t: 1380.0, level: LogLevel::Info, source: "mc",       message: "Mojang main thread · setting up" },
    LogEntry { t: 1500.0, level: LogLevel::Info, source: "mc",       message: "Sound engine started · OpenAL Soft" },
    LogEntry { t: 1620.0, level: LogLevel::Info, source: "iris",     message: "Iris detected · no shaderpack" },
    LogEntry { t: 1740.0, level: LogLevel::Info, source: "sodium",   message: "Sodium · GL 4.6 · 1024 MB VRAM" },
    LogEntry { t: 1860.0, level: LogLevel::Info, source: "mc",       message: "Loaded resource pack · vanilla" },
    LogEntry { t: 1980.0, level: LogLevel::Warn, source: "mods",     message: "distant-horizons: chunk-loader fallback path" },
    LogEntry { t: 2100.0, level: LogLevel::Info, source: "mc",       message: "Built-in shaders compiled" },
    LogEntry { t: 2240.0, level: LogLevel::Info, source: "mc",       message: "Reaching title screen…" },
    LogEntry { t: 2400.0, level: LogLevel::Ok,   source: "mc",       message: "Title screen presented · handing off" },
];

/// One of the three error variants the dev overlay can simulate.
/// Maps 1:1 onto the corresponding `PbarState`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    Rose,
    Recede,
    Shimmer,
}

impl LaunchError {
    pub fn as_pbar_state(self) -> crate::widgets::PbarState {
        use crate::widgets::PbarState;
        match self {
            LaunchError::Rose => PbarState::ErrorRose,
            LaunchError::Recede => PbarState::ErrorRecede,
            LaunchError::Shimmer => PbarState::ErrorShimmer,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LaunchError::Rose => "halt · invalid manifest",
            LaunchError::Recede => "rolled back · jvm crashed",
            LaunchError::Shimmer => "stalled · network unreachable",
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct LaunchingState {
    /// Wall-clock second the launch entered the Launching screen. `None`
    /// when not active (paused/cancelled/never-started).
    pub start_time: Option<f32>,
    pub instance_name: String,
    pub instance_meta: String,
    /// Number of `LOG_SCRIPT` entries currently surfaced. Grows
    /// monotonically as the script-time crosses each entry's `t`.
    pub shown_logs: usize,
    /// `true` once the script reaches t=2400 — locks the percent at 100%
    /// and the stage at "Ready · handing off to the game" while we wait
    /// for the auto-handoff timer.
    pub done: bool,
    /// Wall-clock second the launch transitioned to `done`. Drives the
    /// `pbar` complete-ring expanding-halo timing — `None` while still
    /// in progress.
    pub done_at: Option<f32>,
    /// Active error variant — when `Some`, freezes progress at the snapshot
    /// fraction and styles the bar accordingly. Set by the dev overlay's
    /// "Sim error" affordance via main.rs.
    pub error: Option<LaunchError>,
    /// Wall-clock second the error was triggered. Drives the recede
    /// animation timing for `LaunchError::Recede`.
    pub error_at: Option<f32>,
    /// Snapshot of `progress()` at the moment of error so Recede can
    /// animate from a fixed value down to 0 without depending on the
    /// keyframe lookup as time advances past the error.
    pub error_fraction: f32,
    /// Cross-fade state for the italic stage label. Updated by
    /// `tick(time, dt)` whenever the keyframe-derived stage changes.
    pub stage: VstatusState,
}

impl LaunchingState {
    pub fn enter(&mut self, time: f32, instance_name: &str, instance_meta: &str) {
        self.start_time = Some(time);
        self.instance_name = instance_name.to_string();
        self.instance_meta = instance_meta.to_string();
        self.shown_logs = 0;
        self.done = false;
        self.done_at = None;
        self.error = None;
        self.error_at = None;
        self.error_fraction = 0.0;
        self.stage = VstatusState::new("preparing");
    }

    pub fn exit(&mut self) {
        self.start_time = None;
        self.shown_logs = 0;
        self.done = false;
        self.done_at = None;
        self.error = None;
        self.error_at = None;
        self.error_fraction = 0.0;
        self.stage = VstatusState::default();
    }

    /// Trigger an error variant. Snapshots the current progress fraction so
    /// `LaunchError::Recede` can animate from that value down to 0 without
    /// drifting as `time` advances. Calling with the same variant already
    /// active is a no-op; calling with a different variant resets `error_at`
    /// (so e.g. switching to Recede restarts the shrink animation).
    pub fn trigger_error(&mut self, variant: LaunchError, time: f32) {
        if self.error == Some(variant) {
            return;
        }
        let (pct, _) = self.progress(time);
        self.error = Some(variant);
        self.error_at = Some(time);
        self.error_fraction = (pct / 100.0).clamp(0.0, 1.0);
    }

    pub fn clear_error(&mut self) {
        self.error = None;
        self.error_at = None;
        self.error_fraction = 0.0;
    }

    /// Advance to wall-clock `time` with frame delta `dt`. Updates
    /// `shown_logs`, `done`, and the cross-fade animation. Returns
    /// `true` once on the frame that crosses `DURATION_SECONDS`,
    /// signaling the caller it can schedule the handoff.
    ///
    /// While an error is active, log/percent advancement is paused and
    /// `done` cannot trip — the error variant locks the screen until the
    /// dev overlay clears it.
    pub fn tick(&mut self, time: f32, dt: f32) -> bool {
        let Some(start) = self.start_time else {
            self.stage.tick(dt);
            return false;
        };
        if self.error.is_some() {
            self.stage.set(self.error.unwrap().label());
            self.stage.tick(dt);
            return false;
        }
        let elapsed = (time - start).max(0.0);
        let scaled = elapsed / DURATION_SECONDS * SCRIPT_DURATION;
        // Drain log script up to scaled time.
        while self.shown_logs < LOG_SCRIPT.len()
            && LOG_SCRIPT[self.shown_logs].t <= scaled
        {
            self.shown_logs += 1;
        }
        let just_finished = !self.done && elapsed >= DURATION_SECONDS;
        if elapsed >= DURATION_SECONDS {
            self.done = true;
            if self.done_at.is_none() {
                self.done_at = Some(time);
            }
        }

        // Drive the stage cross-fade.
        let target_stage = if self.done {
            "Ready · handing off to the game"
        } else {
            progress_at(scaled).1
        };
        self.stage.set(target_stage);
        self.stage.tick(dt);

        just_finished
    }

    /// Returns `(percent, stage)` at the given wall-clock time. When an
    /// error is active, returns the snapshot fraction; when `done`, locks
    /// at 100% / "ready".
    pub fn progress(&self, time: f32) -> (f32, &'static str) {
        if let Some(err) = self.error {
            return (self.error_fraction * 100.0, err.label());
        }
        if self.done {
            return (100.0, "ready");
        }
        let Some(start) = self.start_time else {
            return (0.0, "preparing");
        };
        let elapsed = (time - start).max(0.0);
        let scaled = elapsed / DURATION_SECONDS * SCRIPT_DURATION;
        progress_at(scaled)
    }

    /// `true` if the handoff hold window has elapsed and the screen
    /// should auto-return. Errors block the handoff.
    pub fn should_handoff(&self, time: f32) -> bool {
        if !self.done || self.error.is_some() {
            return false;
        }
        let Some(start) = self.start_time else {
            return false;
        };
        time - start >= DURATION_SECONDS + HANDOFF_HOLD_SECONDS
    }
}

fn progress_at(t: f32) -> (f32, &'static str) {
    let mut prev = PROGRESS_KEYFRAMES[0];
    for k in PROGRESS_KEYFRAMES.iter().skip(1) {
        if t < k.t {
            let frac = (t - prev.t) / (k.t - prev.t);
            return (prev.p + (k.p - prev.p) * frac, prev.stage);
        }
        prev = *k;
    }
    (100.0, "ready")
}

// ────────────────────────────────────────────────────────────────────────
// Render
// ────────────────────────────────────────────────────────────────────────

const HEADER_BOTTOM: f32 = 84.0;
const PAD_X: f32 = 48.0;

#[allow(clippy::too_many_arguments)]
pub fn draw_launching(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    h: f32,
    time: f32,
    settings: &Settings,
    state: &LaunchingState,
) {
    let (percent, _) = state.progress(time);
    let pbar_state = if let Some(err) = state.error {
        err.as_pbar_state()
    } else if state.done {
        PbarState::Complete
    } else {
        PbarState::Normal
    };

    let head_bottom = draw_header(canvas, fonts, w, time, percent, state);
    let progress_bottom = draw_progress_region(
        canvas,
        fonts,
        w,
        head_bottom,
        percent,
        state,
        pbar_state,
        time,
        settings,
    );
    draw_log_panel(canvas, fonts, w, h, progress_bottom, time, settings, state);
    draw_footer(canvas, fonts, w, h, state.done);
}

fn draw_header(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    _time: f32,
    percent: f32,
    state: &LaunchingState,
) -> f32 {
    let head_top = HEADER_BOTTOM + 24.0;

    // Right-side: percent display.
    let pct_num_font = fonts.fraunces_axes(80.0, 50.0, 1.0, 300.0, None);
    let pct_pct_font = fonts.fraunces_axes(24.0, 50.0, 0.0, 300.0, None);
    let mut pct_paint = Paint::default();
    pct_paint.set_anti_alias(true);
    pct_paint.set_color(TEXT_PEARL);
    let pct_str = format!("{}", percent.round() as i32);
    let (num_advance, _) = pct_num_font.measure_str(&pct_str, Some(&pct_paint));
    let (_, pct_m) = pct_num_font.metrics();
    let pct_baseline = head_top + (-pct_m.ascent);

    // "%" sign (smaller, mauve)
    let mut pct_pct_paint = Paint::default();
    pct_pct_paint.set_anti_alias(true);
    pct_pct_paint.set_color(TEXT_MAUVE);
    let (pct_pct_advance, _) = pct_pct_font.measure_str("%", Some(&pct_pct_paint));

    let pct_right = w - PAD_X;
    let pct_pct_x = pct_right - pct_pct_advance;
    let pct_num_x = pct_pct_x - num_advance - 2.0;
    canvas.draw_str(&pct_str, (pct_num_x, pct_baseline), &pct_num_font, &pct_paint);
    canvas.draw_str("%", (pct_pct_x, pct_baseline), &pct_pct_font, &pct_pct_paint);

    // Left side: eyebrow + name + meta.
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let mut eb_paint = Paint::default();
    eb_paint.set_anti_alias(true);
    eb_paint.set_color(ACCENT_ROSE);
    let (_, em) = eyebrow_font.metrics();
    let eb_baseline = head_top + (-em.ascent);
    text::draw_tracked_em(
        canvas,
        "LAUNCHING",
        (PAD_X, eb_baseline),
        &eyebrow_font,
        &eb_paint,
        0.35,
    );

    // Name (Fraunces 56)
    let name_font = fonts.fraunces_axes(56.0, 50.0, 1.0, 300.0, None);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color(TEXT_PEARL);
    let (_, nm) = name_font.metrics();
    let name_top = eb_baseline + em.descent + 8.0;
    let name_baseline = name_top + (-nm.ascent);
    let display_name = if state.instance_name.is_empty() {
        "Velvet Hours"
    } else {
        state.instance_name.as_str()
    };
    canvas.draw_str(display_name, (PAD_X, name_baseline), &name_font, &name_paint);

    // Meta row
    let meta_font = fonts.jetbrains_mono(11.0);
    let mut meta_paint = Paint::default();
    meta_paint.set_anti_alias(true);
    meta_paint.set_color(TEXT_MID_PEARL);
    let (_, mm) = meta_font.metrics();
    let meta_top = name_baseline + nm.descent + 10.0;
    let meta_baseline = meta_top + (-mm.ascent);
    let meta_str = if state.instance_meta.is_empty() {
        "VANILLA · 1.21 · ADOPTIUM 21"
    } else {
        state.instance_meta.as_str()
    };
    text::draw_tracked_em(
        canvas,
        meta_str,
        (PAD_X, meta_baseline),
        &meta_font,
        &meta_paint,
        0.15,
    );

    let head_bottom = (meta_baseline + mm.descent + 28.0).max(pct_baseline + pct_m.descent + 16.0);

    // Hairline divider
    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line((0.0, head_bottom), (w, head_bottom), &div);

    head_bottom
}

#[allow(clippy::too_many_arguments)]
fn draw_progress_region(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    top: f32,
    percent: f32,
    state: &LaunchingState,
    pbar_state: PbarState,
    time: f32,
    settings: &Settings,
) -> f32 {
    // CSS `padding: 28px 48px; gap: 14px` between stage label and pbar.
    let stage_top = top + 28.0;
    let stage_font = fonts.newsreader(18.0);
    let mut stage_paint = Paint::default();
    stage_paint.set_anti_alias(true);
    stage_paint.set_color(TEXT_MID_PEARL);
    let (_, sm) = stage_font.metrics();
    let stage_baseline = stage_top + (-sm.ascent);
    draw_vstatus(
        canvas,
        (PAD_X, stage_baseline),
        &stage_font,
        &stage_paint,
        &state.stage,
    );

    let pbar_top = stage_baseline + sm.descent + 14.0;
    let pbar_rect = Rect::from_xywh(PAD_X, pbar_top, w - 2.0 * PAD_X, 12.0);
    // The pbar's `state_age_seconds` parameter feeds either the complete-ring
    // animation (1.4s pearl halo) or the recede animation (1.2s shrink). When
    // an error is active it takes precedence — `done_at` doesn't exist while
    // errored.
    let state_age = if state.error.is_some() {
        state.error_at.map(|t0| (time - t0).max(0.0))
    } else {
        state.done_at.map(|t0| (time - t0).max(0.0))
    };
    draw_pbar(
        canvas,
        pbar_rect,
        percent / 100.0,
        pbar_state,
        time,
        settings,
        state_age,
    );

    let region_bottom = pbar_top + 12.0 + 28.0;

    // Hairline divider below progress region
    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line((0.0, region_bottom), (w, region_bottom), &div);

    region_bottom
}

#[allow(clippy::too_many_arguments)]
fn draw_log_panel(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    h: f32,
    top: f32,
    time: f32,
    settings: &Settings,
    state: &LaunchingState,
) {
    let panel_top = top + 28.0;
    let panel_bottom = h - 80.0;
    let panel = Rect::from_ltrb(PAD_X, panel_top, w - PAD_X, panel_bottom);
    let inner_pad_x = 24.0;
    let inner_pad_y = 20.0;

    draw_glass_panel(canvas, panel, true, time, settings, |canvas| {
        let content_left = panel.left + inner_pad_x;
        let content_right = panel.right - inner_pad_x;
        let content_top = panel.top + inner_pad_y;
        let content_bottom = panel.bottom - inner_pad_y;

        // Head: title + entry count.
        let head_font = fonts.jetbrains_mono(10.0);
        let mut head_title_paint = Paint::default();
        head_title_paint.set_anti_alias(true);
        head_title_paint.set_color(ACCENT_ROSE);
        let mut head_count_paint = Paint::default();
        head_count_paint.set_anti_alias(true);
        head_count_paint.set_color(TEXT_MAUVE);
        let (_, hm) = head_font.metrics();
        let head_baseline = content_top + (-hm.ascent);
        text::draw_tracked_em(
            canvas,
            "LOG · STDOUT",
            (content_left, head_baseline),
            &head_font,
            &head_title_paint,
            0.20,
        );
        let count_str = format!("{} ENTRIES", state.shown_logs);
        let count_advance = text::measure_tracked_em(&head_font, &count_str, 0.20);
        text::draw_tracked_em(
            canvas,
            &count_str,
            (content_right - count_advance, head_baseline),
            &head_font,
            &head_count_paint,
            0.20,
        );
        let head_bottom = head_baseline + hm.descent + 12.0;

        // Hairline below head
        let mut div = Paint::default();
        div.set_anti_alias(true);
        div.set_style(PaintStyle::Stroke);
        div.set_stroke_width(1.0);
        div.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
            None,
        );
        canvas.draw_line(
            (content_left, head_bottom),
            (content_right, head_bottom),
            &div,
        );

        let log_top = head_bottom + 12.0;
        draw_log_lines(
            canvas,
            fonts,
            content_left,
            content_right,
            log_top,
            content_bottom,
            state,
        );
    });
}

fn draw_log_lines(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    state: &LaunchingState,
) {
    // Render the most recent N entries that fit in the available height.
    // CSS specifies `font-size: 11.5px; line-height: 1.85` → ~21.3 px / line.
    let mono_font = fonts.jetbrains_mono(11.5);
    let dim_font = fonts.newsreader(12.0);
    let (_, mono_m) = mono_font.metrics();
    let line_height = 21.3_f32;

    let avail_h = (bottom - top).max(0.0);
    let max_lines = (avail_h / line_height).floor() as usize;
    if max_lines == 0 || state.shown_logs == 0 {
        return;
    }

    let total = state.shown_logs;
    let start = total.saturating_sub(max_lines);
    // CSS log columns: 56px time | 88px source | 1fr message, gap 14.
    let time_col_w = 56.0;
    let src_col_w = 88.0;
    let col_gap = 14.0;
    let time_x_right = left + time_col_w;
    let src_x_left = time_x_right + col_gap;
    let src_x_right = src_x_left + src_col_w;
    let msg_x_left = src_x_right + col_gap;

    let mut y = top;
    for i in start..total {
        let entry = LOG_SCRIPT[i];
        let baseline = y + line_height * 0.5 + mono_m.cap_height * 0.5;

        // Fade-in the most recent line so streaming reads as motion.
        let alpha = if i == total - 1 {
            // Ramp the last line in from 0.4 → 1.0 over the first 240ms after
            // it surfaces. We don't have per-entry surface time, so just use
            // a position-based gradient: top of view = full opacity, with the
            // bottom-most line slightly dimmed.
            0.85
        } else {
            1.0
        };

        // Time column — script-unit time formatted as 0.00s.
        let t_str = format!("{:.2}s", entry.t / 100.0);
        let mut time_paint = Paint::default();
        time_paint.set_anti_alias(true);
        time_paint.set_color(TEXT_MAUVE_DEEP);
        time_paint.set_alpha_f(alpha);
        let (t_advance, _) = mono_font.measure_str(&t_str, Some(&time_paint));
        canvas.draw_str(&t_str, (time_x_right - t_advance, baseline), &mono_font, &time_paint);

        // Source column
        let src_color = match entry.level {
            LogLevel::Info => ACCENT_BERRY,
            LogLevel::Warn => ACCENT_CHAMP,
            LogLevel::Ok => ACCENT_LAV,
            LogLevel::Dim => TEXT_MAUVE,
        };
        let mut src_paint = Paint::default();
        src_paint.set_anti_alias(true);
        src_paint.set_color(src_color);
        src_paint.set_alpha_f(if matches!(entry.level, LogLevel::Dim) { alpha * 0.6 } else { alpha });
        canvas.draw_str(entry.source, (src_x_left, baseline), &mono_font, &src_paint);

        // Message column — dim entries use italic Newsreader 12 instead of mono.
        match entry.level {
            LogLevel::Dim => {
                let mut msg_paint = Paint::default();
                msg_paint.set_anti_alias(true);
                msg_paint.set_color(TEXT_MAUVE_DEEP);
                msg_paint.set_alpha_f(alpha);
                canvas.draw_str(entry.message, (msg_x_left, baseline), &dim_font, &msg_paint);
            }
            _ => {
                let msg_color = match entry.level {
                    LogLevel::Warn => ACCENT_CHAMP,
                    LogLevel::Ok => TEXT_PEARL,
                    _ => TEXT_MID_PEARL,
                };
                let mut msg_paint = Paint::default();
                msg_paint.set_anti_alias(true);
                msg_paint.set_color(msg_color);
                msg_paint.set_alpha_f(alpha);
                let _ = right;
                canvas.draw_str(entry.message, (msg_x_left, baseline), &mono_font, &msg_paint);
            }
        }

        y += line_height;
    }
}

fn draw_footer(canvas: &Canvas, fonts: &FontStore, w: f32, h: f32, done: bool) {
    let label = if done {
        "the curtain rises."
    } else {
        "Cancel"
    };
    let font = if done {
        fonts.fraunces_axes(22.0, 50.0, 1.0, 300.0, None)
    } else {
        fonts.newsreader(14.0)
    };
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(if done { TEXT_PEARL } else { TEXT_MAUVE });
    let (advance, _) = font.measure_str(label, Some(&paint));
    let (_, m) = font.metrics();
    let cy = h - 32.0;
    let baseline = cy + m.cap_height * 0.5;
    canvas.draw_str(label, ((w - advance) * 0.5, baseline), &font, &paint);
}
