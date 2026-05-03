//! Settings screen — `screen-settings` from the prototype. Two-column layout:
//! a left sidebar of four tabs (Graphics / Audio / Paths / Advanced) and a
//! glass-panel content area on the right showing the active tab.
//!
//! Step 13 scaffold: the layout chrome + Graphics tab's section head +
//! placeholder label rows. Real toggles, sliders, dropdowns, and path fields
//! land with step 12 widgets. For now the active tab is fixed at Graphics
//! and clicking the sidebar items is a no-op — interactivity wires in once
//! the SettingsTab state lives in `App`.
//!
//! CSS reference (`StyleSheet2`):
//! ```css
//! .settings-body  { grid-template-columns: 240px 1fr; gap: 36px; padding: 8px 60px 48px; }
//! .settings-tabs-title { font: 32 Fraunces, weight 300, letter-spacing -0.01em }
//! .settings-tab        { padding: 14 14 14 0, transition padding-left 320ms }
//! .settings-tab-mark   { width: 4 height: 18 (gradient when active) }
//! .settings-tab-label  { font: 18 Fraunces, color #9A8087 (active #F4E8EA) }
//! ```

use ewo_core::Settings;
use serde::{Deserialize, Serialize};
use skia_safe::{Canvas, Color, Color4f, Paint, PaintStyle, Rect};

use crate::text::{self, FontStore};
use crate::widgets::{
    draw_glass_panel, draw_vdrop_head, draw_vdrop_menu, draw_vghost_btn, draw_vpathfield,
    draw_vslider, draw_vtoggle, menu_layout, vpathfield, GhostKind, VdropState, VghostBtnState,
    VpathfieldState, VsliderState, VtoggleState,
};

const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MAUVE_DEEP: Color = Color::from_argb(0xFF, 0x6B, 0x55, 0x5C);

const HEADER_BOTTOM: f32 = 84.0;
const BODY_PAD_X: f32 = 60.0;
const BODY_PAD_BOTTOM: f32 = 48.0;
const SIDEBAR_WIDTH: f32 = 240.0;
const COL_GAP: f32 = 36.0;

const PANEL_INNER_PAD_X: f32 = 44.0;
const PANEL_INNER_PAD_Y: f32 = 40.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Account,
    Graphics,
    Audio,
    Paths,
    Advanced,
}

impl SettingsTab {
    const ALL: [SettingsTab; 5] = [
        SettingsTab::Account,
        SettingsTab::Graphics,
        SettingsTab::Audio,
        SettingsTab::Paths,
        SettingsTab::Advanced,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsTab::Account => "Account",
            SettingsTab::Graphics => "Graphics",
            SettingsTab::Audio => "Audio",
            SettingsTab::Paths => "Paths",
            SettingsTab::Advanced => "Advanced",
        }
    }
}

/// Shape of the Account tab's render input. Decoupled from the launcher's
/// `AuthService` so `ewo-render` doesn't need to know about HTTP / threads.
/// Caller (main.rs) converts its `AuthState` snapshot into one of these.
#[derive(Copy, Clone, Debug)]
pub enum AccountView<'a> {
    SignedOut,
    Working { stage: &'a str },
    SignedIn { name: &'a str, uuid: &'a str },
    Failed { message: &'a str },
}

/// Identifies a control slot in the Settings screen — used for hit-testing
/// + driving the right widget state from `main.rs`. Only the slots that
/// have a concrete widget implementation today are listed; placeholder
/// slots (Window mode dropdown, Theme dropdown, path fields, Reset
/// preferences button) land when their primitives ship.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    /// The Account tab's primary button. Meaning depends on `AccountView`:
    ///   - `SignedOut` → "Sign in with Microsoft"
    ///   - `Working`   → disabled (the slot still hit-tests but click is ignored)
    ///   - `SignedIn`  → "Sign out"
    ///   - `Failed`    → "Try again"
    AccountAction,
    Vsync,
    MaxFps,
    WindowMode,
    Theme,
    Master,
    Music,
    Effects,
    AmbientHum,
    GameDir,
    Downloads,
    AutoBackup,
    LogLevel,
    Telemetry,
    ResetPrefs,
}

impl Slot {
    pub fn is_dropdown(self) -> bool {
        matches!(self, Slot::WindowMode | Slot::Theme | Slot::LogLevel)
    }
}

pub const WINDOW_MODE_OPTIONS: &[&str] = &["Windowed", "Borderless", "Fullscreen"];
pub const THEME_OPTIONS: &[&str] = &["Velvet · default", "Pearl · light", "Obsidian", "Champagne"];
pub const LOG_LEVEL_OPTIONS: &[&str] = &["Trace", "Debug", "Info", "Warn", "Error"];

/// Look up the option list for a dropdown slot. Returns `None` for non-
/// dropdown slots.
pub fn dropdown_options(slot: Slot) -> Option<&'static [&'static str]> {
    match slot {
        Slot::WindowMode => Some(WINDOW_MODE_OPTIONS),
        Slot::Theme => Some(THEME_OPTIONS),
        Slot::LogLevel => Some(LOG_LEVEL_OPTIONS),
        _ => None,
    }
}

/// User preferences that the Settings screen renders + edits. Lives in App
/// state; the render path reads from it and `main.rs` routes input back
/// into it via `widget_at` + the per-widget `handle`/`drive` methods.
#[derive(Debug, Clone)]
pub struct Prefs {
    pub vsync: VtoggleState,
    pub max_fps: VsliderState,
    pub window_mode: VdropState,
    pub theme: VdropState,
    pub master: VsliderState,
    pub music: VsliderState,
    pub effects: VsliderState,
    pub ambient_hum: VtoggleState,
    pub game_dir: VpathfieldState,
    pub downloads: VpathfieldState,
    pub auto_backup: VtoggleState,
    pub log_level: VdropState,
    pub telemetry: VtoggleState,
    pub reset_prefs: VghostBtnState,
    /// Hover state for the Account tab's primary button (sign-in /
    /// sign-out / try-again — text changes per `AccountView`).
    pub account_action: VghostBtnState,
    /// Set to `true` when the user clicks the Account tab's primary
    /// button. The main loop reads it, dispatches the appropriate action
    /// based on the current `AuthState`, and clears the flag.
    pub account_action_requested: bool,
    /// Wall-clock second the active tab last changed. Drives a brief
    /// fade-in on the tab's content so the switch feels intentional.
    pub tab_changed_at: Option<f32>,
    /// Set to `true` when the user clicks "Reset preferences". The main
    /// loop checks this each frame, runs the reset, and clears the flag.
    /// We use a flag instead of a return-value path because the reset
    /// needs side effects (disk save, GL backend vsync resync) that the
    /// press handler doesn't have access to.
    pub reset_requested: bool,
}

/// Duration of the tab-content fade-in.
pub const TAB_ANIM_S: f32 = 0.35;

/// Persisted form of the Settings prefs — stripped of transient widget
/// state (hover anims, dropdown open/anim, etc.) so the on-disk file
/// stays clean. The launcher serializes this alongside the instance
/// list, loads it on startup via `Prefs::apply_config`, and saves a
/// fresh one whenever any setting changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsConfig {
    #[serde(default = "default_true")]
    pub vsync: bool,
    #[serde(default = "default_max_fps")]
    pub max_fps: f32,
    #[serde(default = "default_window_mode")]
    pub window_mode: usize,
    #[serde(default)]
    pub theme: usize,
    #[serde(default = "default_master")]
    pub master: f32,
    #[serde(default = "default_music")]
    pub music: f32,
    #[serde(default = "default_effects")]
    pub effects: f32,
    #[serde(default = "default_true")]
    pub ambient_hum: bool,
    #[serde(default = "default_game_dir")]
    pub game_dir: String,
    #[serde(default = "default_downloads")]
    pub downloads: String,
    #[serde(default = "default_true")]
    pub auto_backup: bool,
    #[serde(default = "default_log_level")]
    pub log_level: usize,
    #[serde(default)]
    pub telemetry: bool,
}

fn default_true() -> bool { true }
fn default_max_fps() -> f32 { 144.0 }
fn default_window_mode() -> usize { 1 }
fn default_master() -> f32 { 0.70 }
fn default_music() -> f32 { 0.55 }
fn default_effects() -> f32 { 0.50 }
fn default_log_level() -> usize { 2 }
fn default_game_dir() -> String {
    "~/Library/Application Support/EwoClient".to_string()
}
fn default_downloads() -> String {
    "~/Downloads/ewo".to_string()
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            vsync: true,
            max_fps: default_max_fps(),
            window_mode: default_window_mode(),
            theme: 0,
            master: default_master(),
            music: default_music(),
            effects: default_effects(),
            ambient_hum: true,
            game_dir: default_game_dir(),
            downloads: default_downloads(),
            auto_backup: true,
            log_level: default_log_level(),
            telemetry: false,
        }
    }
}

impl Default for Prefs {
    fn default() -> Self {
        // Defaults mirror the React prototype's `useState` initial values.
        Self {
            vsync: VtoggleState::new(true),
            max_fps: VsliderState::new(144.0, 30.0, 240.0).with_step(10.0),
            window_mode: VdropState::new(1), // "Borderless"
            theme: VdropState::new(0),       // "Velvet · default"
            master: VsliderState::new(0.70, 0.0, 1.0),
            music: VsliderState::new(0.55, 0.0, 1.0),
            effects: VsliderState::new(0.50, 0.0, 1.0),
            ambient_hum: VtoggleState::new(true),
            game_dir: VpathfieldState::new("~/Library/Application Support/EwoClient"),
            downloads: VpathfieldState::new("~/Downloads/ewo"),
            auto_backup: VtoggleState::new(true),
            log_level: VdropState::new(2), // "Info"
            telemetry: VtoggleState::new(false),
            reset_prefs: VghostBtnState::default(),
            account_action: VghostBtnState::default(),
            account_action_requested: false,
            tab_changed_at: None,
            reset_requested: false,
        }
    }
}

impl Prefs {
    pub fn tick(&mut self, dt: f32) {
        self.vsync.tick(dt);
        self.ambient_hum.tick(dt);
        self.auto_backup.tick(dt);
        self.telemetry.tick(dt);
        self.window_mode.tick(dt);
        self.theme.tick(dt);
        self.log_level.tick(dt);
        self.game_dir.tick(dt);
        self.downloads.tick(dt);
        self.reset_prefs.tick(dt);
        self.account_action.tick(dt);
    }

    /// Snapshot the persisted form of the current prefs. App writes this
    /// to disk whenever a setting changes.
    pub fn to_config(&self) -> SettingsConfig {
        SettingsConfig {
            vsync: self.vsync.on,
            max_fps: self.max_fps.value,
            window_mode: self.window_mode.selected,
            theme: self.theme.selected,
            master: self.master.value,
            music: self.music.value,
            effects: self.effects.value,
            ambient_hum: self.ambient_hum.on,
            game_dir: self.game_dir.value.clone(),
            downloads: self.downloads.value.clone(),
            auto_backup: self.auto_backup.on,
            log_level: self.log_level.selected,
            telemetry: self.telemetry.on,
        }
    }

    /// Restore from a persisted config — typically called once on App
    /// startup with the loaded TOML. Widget anim states are reset to
    /// match the new on/off positions instantly (no slide-in).
    pub fn apply_config(&mut self, c: &SettingsConfig) {
        self.vsync = VtoggleState::new(c.vsync);
        self.max_fps.value = c.max_fps;
        self.window_mode = VdropState::new(c.window_mode);
        self.theme = VdropState::new(c.theme);
        self.master.value = c.master;
        self.music.value = c.music;
        self.effects.value = c.effects;
        self.ambient_hum = VtoggleState::new(c.ambient_hum);
        self.game_dir = VpathfieldState::new(&c.game_dir);
        self.downloads = VpathfieldState::new(&c.downloads);
        self.auto_backup = VtoggleState::new(c.auto_backup);
        self.log_level = VdropState::new(c.log_level);
        self.telemetry = VtoggleState::new(c.telemetry);
    }

    /// Close every dropdown. Call this on screen/tab switch and on
    /// click-outside to dismiss any stale open menu.
    pub fn close_dropdowns(&mut self) {
        self.window_mode.close();
        self.theme.close();
        self.log_level.close();
    }

    /// Returns `Some(slot)` for the dropdown that's currently open (or
    /// still animating closed), if any. Used so the Settings render path
    /// knows which menu to portal-draw after the panel returns.
    pub fn open_dropdown(&self) -> Option<Slot> {
        if self.window_mode.open || self.window_mode.anim > 0.001 {
            Some(Slot::WindowMode)
        } else if self.theme.open || self.theme.anim > 0.001 {
            Some(Slot::Theme)
        } else if self.log_level.open || self.log_level.anim > 0.001 {
            Some(Slot::LogLevel)
        } else {
            None
        }
    }

    pub fn dropdown_state(&self, slot: Slot) -> Option<&VdropState> {
        match slot {
            Slot::WindowMode => Some(&self.window_mode),
            Slot::Theme => Some(&self.theme),
            Slot::LogLevel => Some(&self.log_level),
            _ => None,
        }
    }

    pub fn dropdown_state_mut(&mut self, slot: Slot) -> Option<&mut VdropState> {
        match slot {
            Slot::WindowMode => Some(&mut self.window_mode),
            Slot::Theme => Some(&mut self.theme),
            Slot::LogLevel => Some(&mut self.log_level),
            _ => None,
        }
    }
}

pub fn draw_settings(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    h: f32,
    time: f32,
    settings: &Settings,
    active: SettingsTab,
    prefs: &Prefs,
    account: AccountView<'_>,
) {
    draw_screen_head(canvas, fonts, w);
    draw_sidebar(canvas, fonts, h, active);
    draw_panel(canvas, fonts, w, h, time, settings, active, prefs, account);
}

// ────────────────────────────────────────────────────────────────────────
// Screen head (back button + eyebrow). Reuses Instances' visual treatment.
// ────────────────────────────────────────────────────────────────────────

fn draw_screen_head(canvas: &Canvas, fonts: &FontStore, w: f32) {
    let head_y = 58.0;

    let back_font = fonts.fraunces_axes(20.0, 50.0, 0.0, 300.0, None);
    let mut back_paint = Paint::default();
    back_paint.set_anti_alias(true);
    back_paint.set_color(Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5));
    let (_, bm) = back_font.metrics();
    let back_baseline = head_y + (-bm.ascent);
    canvas.draw_str("← Main menu", (BODY_PAD_X, back_baseline), &back_font, &back_paint);

    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let mut eyebrow_paint = Paint::default();
    eyebrow_paint.set_anti_alias(true);
    eyebrow_paint.set_color(TEXT_MAUVE);
    let label = "SETTINGS";
    let advance = text::measure_tracked_em(&eyebrow_font, label, 0.35);
    let (_, em) = eyebrow_font.metrics();
    let eyebrow_baseline = head_y + 4.0 + (-em.ascent);
    text::draw_tracked_em(
        canvas,
        label,
        (w - BODY_PAD_X - advance, eyebrow_baseline),
        &eyebrow_font,
        &eyebrow_paint,
        0.35,
    );

    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line((0.0, HEADER_BOTTOM), (w, HEADER_BOTTOM), &div);
}

// ────────────────────────────────────────────────────────────────────────
// Sidebar — "Settings" title + 4 tab rows
// ────────────────────────────────────────────────────────────────────────

fn draw_sidebar(canvas: &Canvas, fonts: &FontStore, _h: f32, active: SettingsTab) {
    let sidebar_left = BODY_PAD_X;
    let body_top = HEADER_BOTTOM + 8.0; // settings-body padding-top: 8
    let title_top = body_top + 16.0; // settings-tabs padding-top: 16

    // Title
    let title_font = fonts.fraunces_axes(32.0, 50.0, 0.0, 300.0, None);
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color(TEXT_PEARL);
    let (_, tm) = title_font.metrics();
    let title_baseline = title_top + (-tm.ascent);
    canvas.draw_str("Settings", (sidebar_left + 14.0, title_baseline), &title_font, &title_paint);

    // Tabs
    let tab_label_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let (_, lm) = tab_label_font.metrics();
    let row_h = 14.0 + (-lm.ascent + lm.descent) + 14.0; // padding 14 vert + label height
    let mut y = title_baseline + tm.descent + 28.0;

    for tab in SettingsTab::ALL.iter() {
        let is_active = *tab == active;

        // Mark — left bar 4×18 px. Active: gradient; inactive: transparent.
        if is_active {
            let mark_y = y + (row_h - 18.0) * 0.5;
            let mut mark = Paint::default();
            mark.set_anti_alias(true);
            mark.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0),
                None,
            );
            canvas.draw_rect(Rect::from_xywh(sidebar_left, mark_y, 4.0, 18.0), &mark);
            // Subtle glow
            let mut glow = Paint::default();
            glow.set_anti_alias(true);
            glow.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.20),
                None,
            );
            canvas.draw_rect(
                Rect::from_xywh(sidebar_left - 2.0, mark_y - 2.0, 8.0, 22.0),
                &glow,
            );
        }

        // Label
        let label_x = sidebar_left + 4.0 + 14.0; // mark width + gap
        let label_baseline = y + 14.0 + (-lm.ascent);
        let mut label_paint = Paint::default();
        label_paint.set_anti_alias(true);
        label_paint.set_color(if is_active { TEXT_PEARL } else { TEXT_MAUVE });
        canvas.draw_str(tab.label(), (label_x, label_baseline), &tab_label_font, &label_paint);

        y += row_h + 2.0;
    }
}

// ────────────────────────────────────────────────────────────────────────
// Detail panel — glass panel containing the active tab's content
// ────────────────────────────────────────────────────────────────────────

fn draw_panel(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    h: f32,
    time: f32,
    settings: &Settings,
    active: SettingsTab,
    prefs: &Prefs,
    account: AccountView<'_>,
) {
    let body_top = HEADER_BOTTOM + 8.0 + 16.0;
    let panel_left = BODY_PAD_X + SIDEBAR_WIDTH + COL_GAP;
    let panel_right = w - BODY_PAD_X;
    let panel_top = body_top;
    let panel_bottom = h - BODY_PAD_BOTTOM;
    let panel = Rect::from_ltrb(panel_left, panel_top, panel_right, panel_bottom);

    let content_left = panel.left + PANEL_INNER_PAD_X;
    let content_right = panel.right - PANEL_INNER_PAD_X;
    let content_top = panel.top + PANEL_INNER_PAD_Y;

    // Tab-change fade-in. Mirrors the Instances detail-panel animation:
    // 350ms silk-eased opacity ramp + 12px translateY downward when
    // switching tabs.
    let tab_anim = prefs.tab_changed_at.and_then(|start| {
        let elapsed = (time - start).max(0.0);
        if elapsed < TAB_ANIM_S {
            Some(elapsed / TAB_ANIM_S)
        } else {
            None
        }
    });

    draw_glass_panel(canvas, panel, true, time, settings, |canvas| {
        let layer_count = if let Some(p) = tab_anim {
            let eased = ewo_core::CubicBezier::SILK.eval(p.clamp(0.0, 1.0));
            let alpha = eased;
            let dy = (1.0 - eased) * 12.0;
            let s = canvas.save_layer_alpha_f(panel, alpha);
            canvas.translate((0.0, dy));
            Some(s)
        } else {
            None
        };

        draw_section_head(canvas, fonts, content_left, content_right, content_top, active);
        let body_top = section_head_bottom(content_top, fonts);
        if active == SettingsTab::Account {
            draw_account_tab(
                canvas, fonts, content_left, content_right, body_top, time, settings,
                prefs, account,
            );
        } else {
            draw_section_body(
                canvas,
                fonts,
                content_left,
                content_right,
                body_top,
                active,
                prefs,
                time,
                settings,
            );
        }

        if let Some(s) = layer_count {
            canvas.restore_to_count(s);
        }
    });

    // Portal-draw any open dropdown menu *after* the glass panel returns,
    // so the menu can spill outside the panel's clip. Card height is the
    // bound for flip-up detection (the menu must stay inside the card).
    if let Some(slot) = prefs.open_dropdown() {
        if let Some(state) = prefs.dropdown_state(slot) {
            if let Some(opts) = dropdown_options(slot) {
                if let Some(head) = dropdown_head_for_slot(slot, fonts, w, h) {
                    let (menu, flip_up) = menu_layout(head, opts.len(), h);
                    draw_vdrop_menu(canvas, menu, flip_up, opts, state, fonts);
                }
            }
        }
    }
}

/// Find the head bounds of a dropdown by slot — used by both rendering
/// (to compute menu position) and `main.rs` input routing (to know where
/// the dropdown clicks land). Returns `None` if the dropdown isn't on the
/// active tab.
pub fn dropdown_head_for_slot(
    slot: Slot,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
) -> Option<Rect> {
    // Walk every tab — the open dropdown might belong to a tab the user
    // already switched away from (the tab-switch should close them, but
    // until that lands, this still finds the head).
    for tab in SettingsTab::ALL.iter() {
        for (s, rect) in widget_bounds(*tab, fonts, card_w, card_h) {
            if s == slot {
                return Some(rect);
            }
        }
    }
    None
}

fn draw_section_head(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    active: SettingsTab,
) {
    // Title (Fraunces 32, weight 300, letter-spacing -0.015em)
    let h_font = fonts.fraunces_axes(32.0, 50.0, 1.0, 300.0, None);
    let mut h_paint = Paint::default();
    h_paint.set_anti_alias(true);
    h_paint.set_color(TEXT_PEARL);
    let (_, hm) = h_font.metrics();
    let h_baseline = top + (-hm.ascent);
    canvas.draw_str(active.label(), (left, h_baseline), &h_font, &h_paint);

    // Subhead (Newsreader italic 14, mauve)
    let sub_font = fonts.newsreader(14.0);
    let mut sub_paint = Paint::default();
    sub_paint.set_anti_alias(true);
    sub_paint.set_color(TEXT_MAUVE);
    let (_, sm) = sub_font.metrics();
    let sub_top = h_baseline + hm.descent + 6.0;
    let sub_baseline = sub_top + (-sm.ascent);
    canvas.draw_str(subhead_for(active), (left, sub_baseline), &sub_font, &sub_paint);

    // Hairline border-bottom under section head
    let div_y = sub_baseline + sm.descent + 18.0;
    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line((left, div_y), (right, div_y), &div);
}

fn section_head_bottom(top: f32, fonts: &FontStore) -> f32 {
    let h_font = fonts.fraunces_axes(32.0, 50.0, 1.0, 300.0, None);
    let (_, hm) = h_font.metrics();
    let sub_font = fonts.newsreader(14.0);
    let (_, sm) = sub_font.metrics();
    top + (-hm.ascent) + hm.descent + 6.0 + (-sm.ascent) + sm.descent + 18.0 + 28.0
}

fn subhead_for(tab: SettingsTab) -> &'static str {
    match tab {
        SettingsTab::Account => "your Microsoft account.",
        SettingsTab::Graphics => "how the cloth is rendered.",
        SettingsTab::Audio => "tend to the boudoir's hum.",
        SettingsTab::Paths => "where the launcher keeps its things.",
        SettingsTab::Advanced => "for the curious.",
    }
}

/// One row of the section body. `slot` is `Some` when the row is wired to
/// a real widget today; `None` rows render a "(widget pending)" placeholder.
/// `stack: true` mirrors CSS `.settings-row-stack` — label sits *above* the
/// widget at full row width, used by the Paths tab for path fields.
struct RowDef {
    label: &'static str,
    hint: &'static str,
    slot: Option<Slot>,
    stack: bool,
}

impl RowDef {
    const fn row(label: &'static str, hint: &'static str, slot: Option<Slot>) -> Self {
        Self { label, hint, slot, stack: false }
    }
    const fn stacked(label: &'static str, slot: Option<Slot>) -> Self {
        Self { label, hint: "", slot, stack: true }
    }
}

const GRAPHICS_ROWS: &[RowDef] = &[
    RowDef::row("VSync", "tear-free, gentle pacing.", Some(Slot::Vsync)),
    RowDef::row("Max framerate", "cap at 144 fps", Some(Slot::MaxFps)),
    RowDef::row("Window mode", "", Some(Slot::WindowMode)),
    RowDef::row("Theme", "launcher chrome only.", Some(Slot::Theme)),
];
const AUDIO_ROWS: &[RowDef] = &[
    RowDef::row("Master", "", Some(Slot::Master)),
    RowDef::row("Music", "", Some(Slot::Music)),
    RowDef::row("Effects", "", Some(Slot::Effects)),
    RowDef::row("Ambient hum", "soft amber drone.", Some(Slot::AmbientHum)),
];
const PATHS_ROWS: &[RowDef] = &[
    RowDef::stacked("Game directory", Some(Slot::GameDir)),
    RowDef::stacked("Downloads", Some(Slot::Downloads)),
    RowDef::row("Auto-backup worlds", "", Some(Slot::AutoBackup)),
];
const ADVANCED_ROWS: &[RowDef] = &[
    RowDef::row("Log level", "", Some(Slot::LogLevel)),
    RowDef::row("Telemetry", "", Some(Slot::Telemetry)),
    RowDef::row("Reset preferences", "", Some(Slot::ResetPrefs)),
];

/// The Account tab is rendered with a custom layout (see
/// `draw_account_tab`); this returns an empty slice so any code path that
/// iterates rows on the Account tab is a no-op.
const ACCOUNT_ROWS: &[RowDef] = &[];

fn rows_for_tab(tab: SettingsTab) -> &'static [RowDef] {
    match tab {
        SettingsTab::Account => ACCOUNT_ROWS,
        SettingsTab::Graphics => GRAPHICS_ROWS,
        SettingsTab::Audio => AUDIO_ROWS,
        SettingsTab::Paths => PATHS_ROWS,
        SettingsTab::Advanced => ADVANCED_ROWS,
    }
}

const ROW_PAD_V: f32 = 18.0;
// CSS `.settings-row { grid-template-columns: 220px 1fr; gap: 28px }`
const ROW_LABEL_WIDTH: f32 = 220.0;
const ROW_GAP: f32 = 28.0;

/// Vertical gap from the label down to the path-field row in a stacked row.
const STACK_LABEL_TO_FIELD_GAP: f32 = 12.0;

/// Compute one row's vertical extent given the row's top and the row def.
/// Returns `(row_bottom, label_baseline, hint_bottom_y)` so callers can
/// position the widget inside the row. For stacked rows, the row also
/// reserves space below the label for the path-field widget.
fn row_extents(top: f32, row: &RowDef, fonts: &FontStore) -> (f32, f32, f32) {
    let label_font = fonts.fraunces_axes(16.0, 50.0, 0.0, 300.0, None);
    let hint_font = fonts.newsreader(12.0);
    let (_, lm) = label_font.metrics();
    let (_, hm) = hint_font.metrics();
    let label_baseline = top + ROW_PAD_V + (-lm.ascent);
    let hint_bottom = if !row.hint.is_empty() {
        label_baseline + lm.descent + 4.0 + (-hm.ascent) + hm.descent
    } else {
        label_baseline + lm.descent
    };
    let row_bottom = if row.stack {
        hint_bottom + STACK_LABEL_TO_FIELD_GAP + vpathfield::ROW_HEIGHT + ROW_PAD_V
    } else {
        hint_bottom + ROW_PAD_V
    };
    (row_bottom, label_baseline, hint_bottom)
}

/// Compute the control-column rect for a row.
/// - Inline rows: control sits to the right of the 220px label column.
/// - Stacked rows: control fills the full content width below the label.
fn control_rect(
    row_top: f32,
    row_bottom: f32,
    content_left: f32,
    content_right: f32,
    row: &RowDef,
    label_baseline: f32,
    fonts: &FontStore,
) -> Rect {
    if row.stack {
        let label_font = fonts.fraunces_axes(16.0, 50.0, 0.0, 300.0, None);
        let (_, lm) = label_font.metrics();
        let label_bottom = label_baseline + lm.descent;
        let field_top = label_bottom + STACK_LABEL_TO_FIELD_GAP;
        Rect::from_ltrb(
            content_left,
            field_top,
            content_right,
            field_top + vpathfield::ROW_HEIGHT,
        )
    } else {
        let _ = row_top;
        let _ = row_bottom;
        let control_left = content_left + ROW_LABEL_WIDTH + ROW_GAP;
        Rect::from_ltrb(control_left, row_top, content_right, row_bottom)
    }
}

// ────────────────────────────────────────────────────────────────────────
// Account tab — custom layout (the row-grid system doesn't fit)
// ────────────────────────────────────────────────────────────────────────

/// Width of the Account tab's primary action button.
const ACCOUNT_BTN_W: f32 = 240.0;
const ACCOUNT_BTN_H: f32 = 44.0;

/// Card-local rect of the Account tab's primary action button. Returned
/// regardless of `AccountView` (the button is always visible — its label
/// changes between sign-in / sign-out / try-again, but the rect doesn't).
pub fn account_button_bounds(fonts: &FontStore, card_w: f32, _card_h: f32) -> Rect {
    // Mirror the body-top math from draw_panel + section_head_bottom.
    let body_top = HEADER_BOTTOM + 8.0 + 16.0;
    let panel_left = BODY_PAD_X + SIDEBAR_WIDTH + COL_GAP;
    let panel_right = card_w - BODY_PAD_X;
    let content_left = panel_left + PANEL_INNER_PAD_X;
    let content_right = panel_right - PANEL_INNER_PAD_X;
    let content_top = body_top + PANEL_INNER_PAD_Y;
    let body_top_inner = section_head_bottom(content_top, fonts);

    // Same vertical math as `draw_account_tab` — kept in lock-step.
    // Eyebrow + body line both eat ~30 px; place the button at body+90 so
    // there's breathing room above. Centered horizontally in the content
    // column.
    let btn_top = body_top_inner + 110.0;
    let cx = (content_left + content_right) * 0.5;
    Rect::from_xywh(cx - ACCOUNT_BTN_W * 0.5, btn_top, ACCOUNT_BTN_W, ACCOUNT_BTN_H)
}

#[allow(clippy::too_many_arguments)]
fn draw_account_tab(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    _time: f32,
    _settings: &Settings,
    prefs: &Prefs,
    account: AccountView<'_>,
) {
    // Main copy line — explains the auth state to the user.
    let body_font = fonts.newsreader(15.0);
    let mut body_paint = Paint::default();
    body_paint.set_anti_alias(true);
    body_paint.set_color(Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5)); // mid-pearl
    let (_, bm) = body_font.metrics();
    let body_baseline = top + (-bm.ascent);

    let body_text = match account {
        AccountView::SignedOut => {
            "to launch Minecraft, sign in with the Microsoft account that owns your copy."
        }
        AccountView::Working { .. } => "one moment…",
        AccountView::SignedIn { .. } => "you're signed in. you can launch Minecraft any time.",
        AccountView::Failed { .. } => "we couldn't sign you in.",
    };
    canvas.draw_str(body_text, (left, body_baseline), &body_font, &body_paint);

    // Detail line — name (signed-in), stage (working), error (failed). Mauve italic.
    // Wraps onto multiple lines if the message is long (auth errors often
    // are) so it doesn't overflow the panel.
    let italic_font = fonts.newsreader(13.0);
    let mut italic_paint = Paint::default();
    italic_paint.set_anti_alias(true);
    italic_paint.set_color(Color::from_argb(0xFF, 0x9A, 0x80, 0x87)); // mauve
    let (_, im) = italic_font.metrics();
    let italic_top = body_baseline + bm.descent + 8.0;
    let detail_owned: String;
    let detail = match account {
        AccountView::SignedOut => "no account on this machine.",
        AccountView::Working { stage } => stage,
        AccountView::SignedIn { name, uuid } => {
            detail_owned = format!("{}  ·  {}", name, short_uuid(uuid));
            detail_owned.as_str()
        }
        AccountView::Failed { message } => message,
    };
    let line_h = -im.ascent + im.descent + 2.0;
    draw_wrapped_text(
        canvas,
        detail,
        left,
        right,
        italic_top,
        line_h,
        &italic_font,
        &italic_paint,
    );

    // Primary action button. Always rendered — when working it just won't
    // do anything (main.rs gates the click). Position kept in lock-step
    // with `account_button_bounds` so hit-testing matches the visual rect.
    let btn_label = match account {
        AccountView::SignedOut => "Sign in with Microsoft",
        AccountView::Working { .. } => "Cancel",
        AccountView::SignedIn { .. } => "Sign out",
        AccountView::Failed { .. } => "Try again",
    };
    let btn_kind = match account {
        AccountView::Failed { .. } => GhostKind::Danger,
        _ => GhostKind::Pearl,
    };
    let cx = (left + right) * 0.5;
    let btn_top = top + 110.0;
    let btn_rect = Rect::from_xywh(
        cx - ACCOUNT_BTN_W * 0.5,
        btn_top,
        ACCOUNT_BTN_W,
        ACCOUNT_BTN_H,
    );
    crate::widgets::draw_vghost_btn(
        canvas,
        btn_rect,
        btn_label,
        &prefs.account_action,
        btn_kind,
        fonts,
    );
}

/// Render a Minecraft UUID short — first 8 chars uppercase, mono-feel.
/// (UUIDs come back without dashes from the profile endpoint.)
fn short_uuid(uuid: &str) -> String {
    let len = uuid.len().min(8);
    uuid.chars().take(len).collect::<String>().to_uppercase()
}

/// Word-wrap `text` to fit within `[left, right]`, drawing each line at
/// `line_h` vertical step starting at `top`. Greedy word-wrap — splits on
/// ASCII whitespace, never breaks mid-word. Suitable for body copy and
/// short error messages; not a full text layout engine.
fn draw_wrapped_text(
    canvas: &Canvas,
    text: &str,
    left: f32,
    right: f32,
    top: f32,
    line_h: f32,
    font: &skia_safe::Font,
    paint: &Paint,
) {
    let max_w = right - left;
    let (_, m) = font.metrics();
    let mut current = String::new();
    let mut y = top;

    let flush = |canvas: &Canvas, line: &str, y: f32| {
        let baseline = y + (-m.ascent);
        canvas.draw_str(line, (left, baseline), font, paint);
    };

    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
            continue;
        }
        let candidate = format!("{} {}", current, word);
        let (w, _) = font.measure_str(&candidate, Some(paint));
        if w <= max_w {
            current = candidate;
        } else {
            flush(canvas, &current, y);
            y += line_h;
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        flush(canvas, &current, y);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_section_body(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    active: SettingsTab,
    prefs: &Prefs,
    time: f32,
    settings: &Settings,
) {
    let rows = rows_for_tab(active);
    let label_font = fonts.fraunces_axes(16.0, 50.0, 0.0, 300.0, None);
    let hint_font = fonts.newsreader(12.0);

    let mut y = top;
    for (i, row) in rows.iter().enumerate() {
        let row_top = y;
        let (row_bottom, label_baseline, hint_bottom) = row_extents(row_top, row, fonts);

        // Label
        let mut label_paint = Paint::default();
        label_paint.set_anti_alias(true);
        label_paint.set_color(TEXT_PEARL);
        canvas.draw_str(row.label, (left, label_baseline), &label_font, &label_paint);

        // Hint
        if !row.hint.is_empty() {
            let mut hint_paint = Paint::default();
            hint_paint.set_anti_alias(true);
            hint_paint.set_color(TEXT_MAUVE_DEEP);
            let hint_baseline = hint_bottom; // already includes descent
            // Re-derive the proper baseline: hint_bottom = baseline + descent.
            let (_, hm) = hint_font.metrics();
            let hb = hint_baseline - hm.descent;
            canvas.draw_str(row.hint, (left, hb), &hint_font, &hint_paint);
        }

        // Control box — right column for inline rows, full-width below the
        // label for stacked rows.
        let ctrl_box = control_rect(row_top, row_bottom, left, right, row, label_baseline, fonts);
        match row.slot {
            Some(Slot::Vsync) => draw_toggle_in(canvas, ctrl_box, &prefs.vsync),
            Some(Slot::AmbientHum) => draw_toggle_in(canvas, ctrl_box, &prefs.ambient_hum),
            Some(Slot::AutoBackup) => draw_toggle_in(canvas, ctrl_box, &prefs.auto_backup),
            Some(Slot::Telemetry) => draw_toggle_in(canvas, ctrl_box, &prefs.telemetry),
            Some(Slot::MaxFps) => {
                draw_slider_in(canvas, ctrl_box, &prefs.max_fps, time, settings);
                draw_slider_value_label(canvas, fonts, ctrl_box, prefs.max_fps.value, |v| {
                    if v >= 240.0 { "∞ fps".to_string() } else { format!("{} fps", v as i32) }
                });
            }
            Some(Slot::Master) => {
                draw_slider_in(canvas, ctrl_box, &prefs.master, time, settings);
                draw_slider_value_label(canvas, fonts, ctrl_box, prefs.master.value, percent);
            }
            Some(Slot::Music) => {
                draw_slider_in(canvas, ctrl_box, &prefs.music, time, settings);
                draw_slider_value_label(canvas, fonts, ctrl_box, prefs.music.value, percent);
            }
            Some(Slot::Effects) => {
                draw_slider_in(canvas, ctrl_box, &prefs.effects, time, settings);
                draw_slider_value_label(canvas, fonts, ctrl_box, prefs.effects.value, percent);
            }
            Some(Slot::WindowMode) => draw_dropdown_head_in(
                canvas,
                ctrl_box,
                WINDOW_MODE_OPTIONS,
                &prefs.window_mode,
                time,
                settings,
                fonts,
            ),
            Some(Slot::Theme) => draw_dropdown_head_in(
                canvas,
                ctrl_box,
                THEME_OPTIONS,
                &prefs.theme,
                time,
                settings,
                fonts,
            ),
            Some(Slot::LogLevel) => draw_dropdown_head_in(
                canvas,
                ctrl_box,
                LOG_LEVEL_OPTIONS,
                &prefs.log_level,
                time,
                settings,
                fonts,
            ),
            Some(Slot::GameDir) => draw_vpathfield(canvas, ctrl_box, &prefs.game_dir, fonts),
            Some(Slot::Downloads) => draw_vpathfield(canvas, ctrl_box, &prefs.downloads, fonts),
            Some(Slot::ResetPrefs) => draw_danger_button_in(canvas, ctrl_box, &prefs.reset_prefs, fonts),
            // AccountAction never appears as a row — the Account tab uses
            // a custom layout (see `draw_account_tab`).
            Some(Slot::AccountAction) => {}
            None => draw_pending_placeholder(canvas, fonts, ctrl_box),
        }

        // Row divider (omit on last row)
        if i < rows.len() - 1 {
            let mut div = Paint::default();
            div.set_anti_alias(true);
            div.set_style(PaintStyle::Stroke);
            div.set_stroke_width(1.0);
            div.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.05),
                None,
            );
            canvas.draw_line((left, row_bottom), (right, row_bottom), &div);
        }
        y = row_bottom;
    }
}

fn percent(v: f32) -> String {
    format!("{}%", (v * 100.0).round() as i32)
}

fn draw_toggle_in(canvas: &Canvas, ctrl: Rect, state: &VtoggleState) {
    // CSS `justify-self: end` — toggle is right-aligned inside the control column.
    let cy = (ctrl.top + ctrl.bottom) * 0.5;
    let toggle = Rect::from_xywh(
        ctrl.right - crate::widgets::TOGGLE_W,
        cy - crate::widgets::TOGGLE_H * 0.5,
        crate::widgets::TOGGLE_W,
        crate::widgets::TOGGLE_H,
    );
    draw_vtoggle(canvas, toggle, state);
}

fn draw_slider_in(
    canvas: &Canvas,
    ctrl: Rect,
    state: &VsliderState,
    time: f32,
    settings: &Settings,
) {
    // Sliders fill most of the control column; leave room for the value label
    // on the right.
    let label_reserve = 56.0;
    let slider_rect = Rect::from_ltrb(
        ctrl.left,
        ctrl.top,
        (ctrl.right - label_reserve).max(ctrl.left + 80.0),
        ctrl.bottom,
    );
    draw_vslider(canvas, slider_rect, state, time, settings);
}

fn draw_slider_value_label<F>(canvas: &Canvas, fonts: &FontStore, ctrl: Rect, value: f32, fmt: F)
where
    F: FnOnce(f32) -> String,
{
    let font = fonts.jetbrains_mono(11.0);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5));
    let label = fmt(value);
    let advance = text::measure_tracked_em(&font, &label, 0.10);
    let (_, m) = font.metrics();
    let cy = (ctrl.top + ctrl.bottom) * 0.5;
    let baseline = cy + m.cap_height * 0.5;
    text::draw_tracked_em(
        canvas,
        &label,
        (ctrl.right - advance, baseline),
        &font,
        &paint,
        0.10,
    );
}

/// CSS `.vdrop-head { padding: 10 16; font-size: 14; min-width: 160 }` →
/// roughly a 220×40 pill, right-aligned in the control column.
const DROPDOWN_HEAD_WIDTH: f32 = 220.0;
const DROPDOWN_HEAD_HEIGHT: f32 = 40.0;

fn dropdown_head_rect(ctrl: Rect) -> Rect {
    let cy = (ctrl.top + ctrl.bottom) * 0.5;
    let head_w = DROPDOWN_HEAD_WIDTH.min(ctrl.width());
    Rect::from_xywh(
        ctrl.right - head_w,
        cy - DROPDOWN_HEAD_HEIGHT * 0.5,
        head_w,
        DROPDOWN_HEAD_HEIGHT,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_dropdown_head_in(
    canvas: &Canvas,
    ctrl: Rect,
    options: &[&str],
    state: &VdropState,
    time: f32,
    settings: &Settings,
    fonts: &FontStore,
) {
    let head = dropdown_head_rect(ctrl);
    let value = options
        .get(state.selected)
        .copied()
        .unwrap_or("");
    draw_vdrop_head(canvas, head, value, state, time, settings, fonts);
}

/// Right-align a small "Reset preferences" pill in the control column.
fn draw_danger_button_in(
    canvas: &Canvas,
    ctrl: Rect,
    state: &VghostBtnState,
    fonts: &FontStore,
) {
    let label = "Reset preferences";
    let btn_w = 180.0_f32.min(ctrl.width());
    let btn_h = 38.0_f32;
    let cy = (ctrl.top + ctrl.bottom) * 0.5;
    let bounds = Rect::from_xywh(ctrl.right - btn_w, cy - btn_h * 0.5, btn_w, btn_h);
    draw_vghost_btn(canvas, bounds, label, state, GhostKind::Danger, fonts);
}

fn draw_pending_placeholder(canvas: &Canvas, fonts: &FontStore, ctrl: Rect) {
    let font = fonts.newsreader(13.0);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(TEXT_MAUVE_DEEP);
    let label = "(widget pending)";
    let (advance, _) = font.measure_str(label, Some(&paint));
    let (_, m) = font.metrics();
    let cy = (ctrl.top + ctrl.bottom) * 0.5;
    let baseline = cy + m.cap_height * 0.5;
    canvas.draw_str(label, (ctrl.right - advance, baseline), &font, &paint);
}

// ────────────────────────────────────────────────────────────────────────
// Hit-testing — sidebar tabs + per-row widget bounds
// ────────────────────────────────────────────────────────────────────────

/// Compute card-local bounds for every widget on the active tab. Returns
/// (slot, widget_bounds) — `widget_bounds` is the actual hit-rect of the
/// widget, not the row, so click detection lines up with what the user
/// sees. Sliders use the full slider track rect; toggles use the toggle
/// pill rect (44×22, right-aligned).
pub fn widget_bounds(
    tab: SettingsTab,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
) -> Vec<(Slot, Rect)> {
    let body_top = HEADER_BOTTOM + 8.0 + 16.0;
    let panel_left = BODY_PAD_X + SIDEBAR_WIDTH + COL_GAP;
    let panel_right = card_w - BODY_PAD_X;
    let panel_top = body_top;
    let panel_bottom = card_h - BODY_PAD_BOTTOM;
    let panel = Rect::from_ltrb(panel_left, panel_top, panel_right, panel_bottom);

    let content_left = panel.left + PANEL_INNER_PAD_X;
    let content_right = panel.right - PANEL_INNER_PAD_X;
    let content_top = panel.top + PANEL_INNER_PAD_Y;

    let body_top_y = section_head_bottom(content_top, fonts);

    let mut out: Vec<(Slot, Rect)> = Vec::new();
    let mut y = body_top_y;
    for row in rows_for_tab(tab) {
        let row_top = y;
        let (row_bottom, label_baseline, _) = row_extents(row_top, row, fonts);
        if let Some(slot) = row.slot {
            let ctrl = control_rect(
                row_top,
                row_bottom,
                content_left,
                content_right,
                row,
                label_baseline,
                fonts,
            );
            let widget = match slot {
                Slot::Vsync | Slot::AmbientHum | Slot::AutoBackup | Slot::Telemetry => {
                    let cy = (ctrl.top + ctrl.bottom) * 0.5;
                    Rect::from_xywh(
                        ctrl.right - crate::widgets::TOGGLE_W,
                        cy - crate::widgets::TOGGLE_H * 0.5,
                        crate::widgets::TOGGLE_W,
                        crate::widgets::TOGGLE_H,
                    )
                }
                Slot::MaxFps | Slot::Master | Slot::Music | Slot::Effects => {
                    let label_reserve = 56.0;
                    Rect::from_ltrb(
                        ctrl.left,
                        ctrl.top,
                        (ctrl.right - label_reserve).max(ctrl.left + 80.0),
                        ctrl.bottom,
                    )
                }
                Slot::WindowMode | Slot::Theme | Slot::LogLevel => dropdown_head_rect(ctrl),
                Slot::GameDir | Slot::Downloads => ctrl, // full path-field row
                Slot::ResetPrefs => {
                    let btn_w = 180.0_f32.min(ctrl.width());
                    let btn_h = 38.0_f32;
                    let cy = (ctrl.top + ctrl.bottom) * 0.5;
                    Rect::from_xywh(ctrl.right - btn_w, cy - btn_h * 0.5, btn_w, btn_h)
                }
                Slot::AccountAction => {
                    // The Account tab's button doesn't go through the row
                    // grid — it has its own custom layout. We special-case
                    // it below the loop. Unreachable from any row.
                    unreachable!("AccountAction has no row entry")
                }
            };
            out.push((slot, widget));
        }
        y = row_bottom;
    }
    // Account tab — custom layout. Append the button rect directly.
    if tab == SettingsTab::Account {
        out.push((
            Slot::AccountAction,
            account_button_bounds(fonts, card_w, card_h),
        ));
    }
    out
}

/// Browse-button rect for a path-field slot. Returns `None` for non-path
/// slots. Used by `main.rs` to route Browse button clicks separately from
/// the input rect (which is currently inert — text input is post-v1).
pub fn path_browse_bounds(slot: Slot, fonts: &FontStore, card_w: f32, card_h: f32) -> Option<Rect> {
    if !matches!(slot, Slot::GameDir | Slot::Downloads) {
        return None;
    }
    widget_bounds(SettingsTab::Paths, fonts, card_w, card_h)
        .into_iter()
        .find_map(|(s, row)| if s == slot { Some(crate::widgets::vpathfield::browse_bounds(row)) } else { None })
}

// ────────────────────────────────────────────────────────────────────────
// Sidebar hit-testing
// ────────────────────────────────────────────────────────────────────────

/// Card-local bounds for each sidebar tab, indexed by `SettingsTab::ALL`.
/// Returned in the same order as `SettingsTab::ALL`.
pub fn sidebar_tab_bounds(fonts: &FontStore) -> [(SettingsTab, Rect); 5] {
    let sidebar_left = BODY_PAD_X;
    let body_top = HEADER_BOTTOM + 8.0;
    let title_top = body_top + 16.0;

    let title_font = fonts.fraunces_axes(32.0, 50.0, 0.0, 300.0, None);
    let (_, tm) = title_font.metrics();
    let title_baseline = title_top + (-tm.ascent);

    let tab_label_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let (_, lm) = tab_label_font.metrics();
    let row_h = 14.0 + (-lm.ascent + lm.descent) + 14.0;
    let mut y = title_baseline + tm.descent + 28.0;

    let mut out: [(SettingsTab, Rect); 5] = [
        (SettingsTab::Account, Rect::default()),
        (SettingsTab::Graphics, Rect::default()),
        (SettingsTab::Audio, Rect::default()),
        (SettingsTab::Paths, Rect::default()),
        (SettingsTab::Advanced, Rect::default()),
    ];
    for (i, tab) in SettingsTab::ALL.iter().enumerate() {
        out[i] = (*tab, Rect::from_xywh(sidebar_left, y, SIDEBAR_WIDTH, row_h));
        y += row_h + 2.0;
    }
    out
}
