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
use skia_safe::{Canvas, Color, Color4f, Paint, PaintCap, PaintStyle, RRect, Rect};

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
    Profiles,
    Keybinds,
    Modules,
    Graphics,
    Audio,
    Paths,
    Advanced,
}

impl SettingsTab {
    const ALL: [SettingsTab; 8] = [
        SettingsTab::Account,
        SettingsTab::Profiles,
        SettingsTab::Keybinds,
        SettingsTab::Modules,
        SettingsTab::Graphics,
        SettingsTab::Audio,
        SettingsTab::Paths,
        SettingsTab::Advanced,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsTab::Account => "Account",
            SettingsTab::Profiles => "Profiles",
            SettingsTab::Keybinds => "Keybinds",
            SettingsTab::Modules => "Modules",
            SettingsTab::Graphics => "Graphics",
            SettingsTab::Audio => "Audio",
            SettingsTab::Paths => "Paths",
            SettingsTab::Advanced => "Advanced",
        }
    }
}

/// One account row for the Account tab's list. Decoupled from the
/// launcher's `MinecraftAccount` so `ewo-render` stays ignorant of the
/// auth chain.
#[derive(Copy, Clone, Debug)]
pub struct AccountRowView<'a> {
    pub name: &'a str,
    pub uuid: &'a str,
    /// Whether this is the active account — the one launches use.
    pub active: bool,
}

/// Status of the in-flight auth operation, as the Account tab sees it.
#[derive(Copy, Clone, Debug)]
pub enum AccountOpView<'a> {
    Idle,
    Working { stage: &'a str },
    Failed { message: &'a str },
}

/// The Account tab's full render input. Decoupled from the launcher's
/// `AuthService` so `ewo-render` doesn't need to know about HTTP / threads.
#[derive(Copy, Clone, Debug)]
pub struct AccountView<'a> {
    pub accounts: &'a [AccountRowView<'a>],
    pub op: AccountOpView<'a>,
}

/// A pending account-management action. The Account-tab press handler
/// sets it into `Prefs::account_request`; the main loop dispatches it to
/// the `AuthService` (which it owns mutably) and clears it.
#[derive(Clone, Debug)]
pub enum AccountRequest {
    /// Run the interactive OAuth flow — add, or first-time sign in.
    Add,
    /// Make the account with this UUID active.
    SetActive(String),
    /// Remove the account with this UUID.
    Remove(String),
}

/// Which Account-tab control the cursor is over — drives hover highlight.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AccountHover {
    /// The body of account row `index` (click to make active).
    Row(usize),
    /// The remove-× of account row `index`.
    Remove(usize),
}

/// One profile row for the Profiles tab's list.
#[derive(Copy, Clone, Debug)]
pub struct ProfileRowView<'a> {
    pub name: &'a str,
    /// Whether this is the active profile.
    pub active: bool,
}

/// The Profiles tab's render input.
#[derive(Copy, Clone, Debug)]
pub struct ProfileView<'a> {
    pub profiles: &'a [ProfileRowView<'a>],
}

/// A pending profile-management action. The Profiles-tab press handler
/// sets it into `Prefs::profile_request`; the main loop dispatches it to
/// the `profile` module and clears it.
#[derive(Clone, Debug)]
pub enum ProfileRequest {
    /// Make the named profile active.
    Switch(String),
    /// Create a new profile.
    New,
    /// Duplicate the active profile.
    Duplicate,
    /// Delete the named profile.
    Delete(String),
    /// Rename the profile at this registry index to `new_name`.
    Rename { index: usize, new_name: String },
}

/// Which Profiles-tab control the cursor is over — drives hover highlight.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ProfileHover {
    /// The body of profile row `index` (click to make active).
    Row(usize),
    /// The rename (✎) button of profile row `index`.
    Rename(usize),
    /// The delete-× of profile row `index`.
    Delete(usize),
}

/// One keybind row for the Keybinds tab's list. The launcher resolves the
/// action + its bound chord into plain strings so `ewo-render` stays
/// ignorant of GLFW codes and the keybind registry.
#[derive(Copy, Clone, Debug)]
pub struct KeybindRowView<'a> {
    /// The action's human label, e.g. "Open the overlay".
    pub action_label: &'a str,
    /// The module the action belongs to, e.g. "Core".
    pub module: &'a str,
    /// The bound key, pre-formatted, e.g. "Right Shift".
    pub chord_label: &'a str,
    /// True while this row is waiting for a key press to rebind.
    pub capturing: bool,
}

/// The Keybinds tab's render input.
#[derive(Copy, Clone, Debug)]
pub struct KeybindView<'a> {
    pub rows: &'a [KeybindRowView<'a>],
}

/// A pending keybind action — set by the Keybinds-tab press handler into
/// `Prefs::keybind_request`, dispatched (and cleared) by the main loop.
#[derive(Clone, Debug)]
pub enum KeybindRequest {
    /// Begin capturing a new key for the action with this index in the
    /// registry (matches `KeybindView::rows`).
    Capture(usize),
    /// Restore every keybind to its registry default.
    ResetAll,
}

/// Identifies a control slot in the Settings screen — used for hit-testing
/// + driving the right widget state from `main.rs`. Only the slots that
/// have a concrete widget implementation today are listed; placeholder
/// slots (Window mode dropdown, Theme dropdown, path fields, Reset
/// preferences button) land when their primitives ship.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
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
    /// Hover state for the Account tab's "Add account" button.
    pub account_add: VghostBtnState,
    /// A pending account-management action — set by the Account-tab press
    /// handler, dispatched (and cleared) by the main loop, which owns the
    /// `&mut AuthService` the action needs.
    pub account_request: Option<AccountRequest>,
    /// Which Account-tab row / remove-button the cursor is over. Updated
    /// on cursor motion, read by `draw_account_tab` for hover highlight.
    pub account_hover: Option<AccountHover>,
    /// Hover state for the Profiles tab's "New profile" / "Duplicate
    /// current" buttons.
    pub profile_new: VghostBtnState,
    pub profile_dup: VghostBtnState,
    /// A pending profile-management action — set by the Profiles-tab press
    /// handler, dispatched (and cleared) by the main loop.
    pub profile_request: Option<ProfileRequest>,
    /// Which Profiles-tab row / rename / delete control the cursor is over.
    pub profile_hover: Option<ProfileHover>,
    /// While `Some`, profile row `index` is being inline-renamed — the row
    /// draws a text field instead of the name. Driven by `main.rs`.
    pub profile_renaming: Option<usize>,
    /// The in-progress rename's text buffer.
    pub profile_rename_buffer: String,
    /// Seconds since the rename buffer last changed — drives the caret blink.
    pub profile_rename_focus_time: f32,
    /// Hover state for the Keybinds tab's "Reset to defaults" button.
    pub keybind_reset: VghostBtnState,
    /// A pending keybind action — set by the Keybinds-tab press handler,
    /// dispatched (and cleared) by the main loop.
    pub keybind_request: Option<KeybindRequest>,
    /// Which Keybinds-tab chord button the cursor is over (row index).
    pub keybind_hover: Option<usize>,
    /// Modules tab — one toggle state per catalog module (`.on` = enabled).
    pub module_toggles: Vec<VtoggleState>,
    /// Modules tab — the FOV Control setting slider.
    pub module_fov: VsliderState,
    /// Set by a Modules-tab edit; the main loop writes `modules.toml` + clears.
    pub modules_changed: bool,
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
            account_add: VghostBtnState::default(),
            account_request: None,
            account_hover: None,
            profile_new: VghostBtnState::default(),
            profile_dup: VghostBtnState::default(),
            profile_request: None,
            profile_hover: None,
            profile_renaming: None,
            profile_rename_buffer: String::new(),
            profile_rename_focus_time: 0.0,
            keybind_reset: VghostBtnState::default(),
            keybind_request: None,
            keybind_hover: None,
            module_toggles: ewo_core::modules::REGISTRY
                .iter()
                .map(|m| VtoggleState::new(m.default_enabled))
                .collect(),
            // FOV Control's slider — range from the catalog (30–150°).
            module_fov: VsliderState::new(90.0, 30.0, 150.0).with_step(1.0),
            modules_changed: false,
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
        self.account_add.tick(dt);
        self.profile_new.tick(dt);
        self.profile_dup.tick(dt);
        for toggle in &mut self.module_toggles {
            toggle.tick(dt);
        }
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

    /// Apply a loaded module config — `enabled[i]` for catalog module `i`,
    /// plus the FOV Control value. Toggle anim states reset to the new
    /// positions (no slide-in), as `apply_config` does for the others.
    pub fn apply_modules(&mut self, enabled: &[bool], fov: f32) {
        for (toggle, &on) in self.module_toggles.iter_mut().zip(enabled) {
            *toggle = VtoggleState::new(on);
        }
        self.module_fov.value = fov;
    }

    /// Snapshot the Modules tab for persistence — per-module enabled flags
    /// (catalog order) and the FOV Control value.
    pub fn modules_snapshot(&self) -> (Vec<bool>, f32) {
        (
            self.module_toggles.iter().map(|t| t.on).collect(),
            self.module_fov.value,
        )
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
    profiles: ProfileView<'_>,
    keybinds: KeybindView<'_>,
) {
    draw_screen_head(canvas, fonts, w);
    draw_sidebar(canvas, fonts, h, active);
    draw_panel(
        canvas, fonts, w, h, time, settings, active, prefs, account, profiles, keybinds,
    );
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
    profiles: ProfileView<'_>,
    keybinds: KeybindView<'_>,
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
            draw_account_tab(canvas, fonts, w, prefs, account);
        } else if active == SettingsTab::Profiles {
            draw_profiles_tab(canvas, fonts, w, prefs, profiles);
        } else if active == SettingsTab::Keybinds {
            draw_keybinds_tab(canvas, fonts, w, prefs, keybinds);
        } else if active == SettingsTab::Modules {
            draw_modules_tab(canvas, fonts, w, prefs, time, settings);
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
        SettingsTab::Profiles => "named looks you can switch between.",
        SettingsTab::Keybinds => "the keys this profile answers to.",
        SettingsTab::Modules => "legit-client features, per profile.",
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
        // Account / Profiles / Keybinds use custom layouts (see the
        // draw_*_tab fns) — no row-grid entries.
        SettingsTab::Account
        | SettingsTab::Profiles
        | SettingsTab::Keybinds
        | SettingsTab::Modules => ACCOUNT_ROWS,
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

/// Account-tab list metrics.
const ACCOUNT_ROW_H: f32 = 56.0;
const ACCOUNT_ROW_GAP: f32 = 8.0;
const ACCOUNT_AVATAR: f32 = 36.0;
/// Vertical space the body-copy line reserves above the account list.
const ACCOUNT_COPY_BLOCK: f32 = 40.0;
const ADD_BTN_W: f32 = 240.0;
const ADD_BTN_H: f32 = 44.0;

/// Card-local layout of one account row: the full row rect (click to make
/// active) and the remove-× rect nested at its right edge.
pub struct AccountRowLayout {
    pub index: usize,
    pub row: Rect,
    pub remove: Rect,
}

/// Card-local layout of the whole Account tab — the body-copy anchor, the
/// per-account rows, and the "Add account" button. `draw_account_tab` and
/// the `main.rs` hit-testers both build this so visuals + input agree.
pub struct AccountTabLayout {
    pub content_left: f32,
    pub content_right: f32,
    pub header_top: f32,
    pub rows: Vec<AccountRowLayout>,
    pub add_button: Rect,
}

/// Compute the Account tab's layout for `account_count` accounts.
pub fn account_tab_layout(fonts: &FontStore, card_w: f32, account_count: usize) -> AccountTabLayout {
    // Mirror the content box from draw_panel + section_head_bottom.
    let body_top = HEADER_BOTTOM + 8.0 + 16.0;
    let panel_left = BODY_PAD_X + SIDEBAR_WIDTH + COL_GAP;
    let panel_right = card_w - BODY_PAD_X;
    let content_left = panel_left + PANEL_INNER_PAD_X;
    let content_right = panel_right - PANEL_INNER_PAD_X;
    let content_top = body_top + PANEL_INNER_PAD_Y;
    let header_top = section_head_bottom(content_top, fonts);

    let list_top = header_top + ACCOUNT_COPY_BLOCK;
    let mut rows = Vec::with_capacity(account_count);
    for i in 0..account_count {
        let top = list_top + i as f32 * (ACCOUNT_ROW_H + ACCOUNT_ROW_GAP);
        let row = Rect::from_ltrb(content_left, top, content_right, top + ACCOUNT_ROW_H);
        let rm = 30.0;
        let cy = (row.top + row.bottom) * 0.5;
        let remove = Rect::from_xywh(row.right - rm - 8.0, cy - rm * 0.5, rm, rm);
        rows.push(AccountRowLayout { index: i, row, remove });
    }
    let list_bottom = if account_count == 0 {
        list_top
    } else {
        list_top + account_count as f32 * (ACCOUNT_ROW_H + ACCOUNT_ROW_GAP) - ACCOUNT_ROW_GAP
    };
    let add_button = Rect::from_xywh(content_left, list_bottom + 18.0, ADD_BTN_W, ADD_BTN_H);

    AccountTabLayout {
        content_left,
        content_right,
        header_top,
        rows,
        add_button,
    }
}

fn draw_account_tab(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    prefs: &Prefs,
    account: AccountView<'_>,
) {
    let layout = account_tab_layout(fonts, card_w, account.accounts.len());

    // Body copy.
    let body_font = fonts.newsreader(15.0);
    let mut body_paint = Paint::default();
    body_paint.set_anti_alias(true);
    body_paint.set_color(Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5)); // mid-pearl
    let (_, bm) = body_font.metrics();
    let copy = if account.accounts.is_empty() {
        "sign in with the Microsoft account that owns your copy of Minecraft."
    } else {
        "the active account is used for launches — click another to switch."
    };
    canvas.draw_str(
        copy,
        (layout.content_left, layout.header_top + (-bm.ascent)),
        &body_font,
        &body_paint,
    );

    // Account rows.
    for (rl, row) in layout.rows.iter().zip(account.accounts) {
        let hovered = prefs.account_hover == Some(AccountHover::Row(rl.index));
        let remove_hovered = prefs.account_hover == Some(AccountHover::Remove(rl.index));
        draw_account_row(canvas, fonts, rl, row, hovered, remove_hovered);
    }

    // "Add account" button — label + kind depend on first sign-in vs.
    // addition vs. retry after a failure.
    let add_label = match account.op {
        AccountOpView::Failed { .. } => "Try again",
        _ if account.accounts.is_empty() => "Sign in with Microsoft",
        _ => "Add another account",
    };
    let add_kind = match account.op {
        AccountOpView::Failed { .. } => GhostKind::Danger,
        _ => GhostKind::Pearl,
    };
    draw_vghost_btn(
        canvas,
        layout.add_button,
        add_label,
        &prefs.account_add,
        add_kind,
        fonts,
    );

    // In-flight / error status line below the button.
    let status_baseline = layout.add_button.bottom + 26.0;
    match account.op {
        AccountOpView::Idle => {}
        AccountOpView::Working { stage } => {
            let f = fonts.newsreader(13.0);
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_color(Color::from_argb(0xFF, 0x9A, 0x80, 0x87)); // mauve
            canvas.draw_str(stage, (layout.content_left, status_baseline), &f, &p);
        }
        AccountOpView::Failed { message } => {
            let f = fonts.newsreader(13.0);
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_color(Color::from_argb(0xFF, 0xD4, 0x88, 0x9A)); // error text
            let (_, m) = f.metrics();
            let line_h = -m.ascent + m.descent + 2.0;
            draw_wrapped_text(
                canvas,
                message,
                layout.content_left,
                layout.content_right,
                status_baseline,
                line_h,
                &f,
                &p,
            );
        }
    }
}

/// Draw one account row — background, monogram avatar, name + short UUID,
/// active marker, and the remove-× button.
fn draw_account_row(
    canvas: &Canvas,
    fonts: &FontStore,
    layout: &AccountRowLayout,
    view: &AccountRowView<'_>,
    hovered: bool,
    remove_hovered: bool,
) {
    let row = layout.row;
    let rrect = RRect::new_rect_xy(row, 12.0, 12.0);

    // Background — tinted + rimmed when active, faintly lit when hovered.
    if view.active {
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10), None);
        canvas.draw_rrect(rrect, &bg);
        let mut rim = Paint::default();
        rim.set_anti_alias(true);
        rim.set_style(PaintStyle::Stroke);
        rim.set_stroke_width(1.0);
        rim.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.32), None);
        canvas.draw_rrect(rrect, &rim);
    } else if hovered {
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 0.05), None);
        canvas.draw_rrect(rrect, &bg);
    }

    let cy = (row.top + row.bottom) * 0.5;

    // Monogram avatar.
    let av = Rect::from_xywh(
        row.left + 12.0,
        cy - ACCOUNT_AVATAR * 0.5,
        ACCOUNT_AVATAR,
        ACCOUNT_AVATAR,
    );
    draw_monogram(canvas, fonts, av, view.name, view.uuid);

    // Name (Fraunces) + short UUID (mono), stacked.
    let text_left = av.right + 14.0;
    let name_font = fonts.fraunces_axes(17.0, 50.0, 0.0, 360.0, None);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color(Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA));
    canvas.draw_str(view.name, (text_left, cy - 2.0), &name_font, &name_paint);

    let uuid_font = fonts.jetbrains_mono(10.0);
    let mut uuid_paint = Paint::default();
    uuid_paint.set_anti_alias(true);
    uuid_paint.set_color(Color::from_argb(0xFF, 0x6B, 0x55, 0x5C)); // deep mauve
    let (_, um) = uuid_font.metrics();
    canvas.draw_str(
        short_uuid(view.uuid),
        (text_left, cy + (-um.ascent) + 5.0),
        &uuid_font,
        &uuid_paint,
    );

    // Active marker — a small rose dot left of the remove button.
    if view.active {
        let mut dot = Paint::default();
        dot.set_anti_alias(true);
        dot.set_color(Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5));
        canvas.draw_circle((layout.remove.left - 14.0, cy), 3.5, &dot);
    }

    // Remove-× — two round strokes, faint at rest, bright with a disc on hover.
    let rm = layout.remove;
    let rcx = (rm.left + rm.right) * 0.5;
    let rcy = (rm.top + rm.bottom) * 0.5;
    if remove_hovered {
        let mut disc = Paint::default();
        disc.set_anti_alias(true);
        disc.set_color4f(Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.18), None);
        canvas.draw_circle((rcx, rcy), 13.0, &disc);
    }
    let mut x = Paint::default();
    x.set_anti_alias(true);
    x.set_style(PaintStyle::Stroke);
    x.set_stroke_width(1.6);
    x.set_stroke_cap(PaintCap::Round);
    let x_alpha = if remove_hovered { 0.95 } else { 0.40 };
    x.set_color4f(Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, x_alpha), None);
    let d = 5.0;
    canvas.draw_line((rcx - d, rcy - d), (rcx + d, rcy + d), &x);
    canvas.draw_line((rcx + d, rcy - d), (rcx - d, rcy + d), &x);
}

/// A monogram avatar — a Velvet-tinted disc with the account's initial.
/// The tint is picked from the four accent hues by hashing the UUID, so
/// each account reads distinct without a network fetch. (Real skin-head
/// avatars are a Phase F polish follow-up — they need the profile fetch
/// to capture the skin URL plus a threaded skin-image cache.)
fn draw_monogram(canvas: &Canvas, fonts: &FontStore, rect: Rect, name: &str, uuid: &str) {
    let cx = (rect.left + rect.right) * 0.5;
    let cy = (rect.top + rect.bottom) * 0.5;
    let r = rect.width() * 0.5;
    let (tr, tg, tb) = monogram_tint(uuid);

    let mut disc = Paint::default();
    disc.set_anti_alias(true);
    disc.set_color4f(Color4f::new(tr, tg, tb, 0.30), None);
    canvas.draw_circle((cx, cy), r, &disc);
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(Color4f::new(tr, tg, tb, 0.55), None);
    canvas.draw_circle((cx, cy), r - 0.5, &rim);

    let initial: String = name.chars().next().unwrap_or('?').to_uppercase().collect();
    let font = fonts.fraunces_axes(18.0, 50.0, 1.0, 460.0, None);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA));
    let (tw, _) = font.measure_str(&initial, Some(&paint));
    let (_, m) = font.metrics();
    canvas.draw_str(
        &initial,
        (cx - tw * 0.5, cy + m.cap_height * 0.5),
        &font,
        &paint,
    );
}

/// Pick one of the four Velvet accent hues for an account's monogram,
/// deterministically from its UUID.
fn monogram_tint(uuid: &str) -> (f32, f32, f32) {
    const TINTS: [(f32, f32, f32); 4] = [
        (180.0 / 255.0, 116.0 / 255.0, 145.0 / 255.0), // berry
        (229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0), // rose
        (201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0), // lavender
        (232.0 / 255.0, 212.0 / 255.0, 168.0 / 255.0), // champagne
    ];
    let sum: u32 = uuid.bytes().map(u32::from).sum();
    TINTS[(sum % 4) as usize]
}

// ── Profiles tab ─────────────────────────────────────────────────────────

/// Width of the Profiles tab's New / Duplicate buttons.
const PROFILE_BTN_W: f32 = 188.0;

/// Card-local layout of one profile row: the full row rect (click to make
/// active) and the rename-✎ / delete-× buttons nested at its right edge.
pub struct ProfileRowLayout {
    pub index: usize,
    pub row: Rect,
    pub rename: Rect,
    pub delete: Rect,
}

/// Card-local layout of the Profiles tab.
pub struct ProfileTabLayout {
    pub content_left: f32,
    pub content_right: f32,
    pub header_top: f32,
    pub rows: Vec<ProfileRowLayout>,
    pub new_button: Rect,
    pub dup_button: Rect,
}

/// Compute the Profiles tab's layout for `profile_count` profiles.
pub fn profiles_tab_layout(fonts: &FontStore, card_w: f32, profile_count: usize) -> ProfileTabLayout {
    let body_top = HEADER_BOTTOM + 8.0 + 16.0;
    let panel_left = BODY_PAD_X + SIDEBAR_WIDTH + COL_GAP;
    let panel_right = card_w - BODY_PAD_X;
    let content_left = panel_left + PANEL_INNER_PAD_X;
    let content_right = panel_right - PANEL_INNER_PAD_X;
    let content_top = body_top + PANEL_INNER_PAD_Y;
    let header_top = section_head_bottom(content_top, fonts);

    let list_top = header_top + ACCOUNT_COPY_BLOCK;
    let mut rows = Vec::with_capacity(profile_count);
    for i in 0..profile_count {
        let top = list_top + i as f32 * (ACCOUNT_ROW_H + ACCOUNT_ROW_GAP);
        let row = Rect::from_ltrb(content_left, top, content_right, top + ACCOUNT_ROW_H);
        let rm = 30.0;
        let cy = (row.top + row.bottom) * 0.5;
        let delete = Rect::from_xywh(row.right - rm - 8.0, cy - rm * 0.5, rm, rm);
        let rename = Rect::from_xywh(delete.left - rm - 4.0, cy - rm * 0.5, rm, rm);
        rows.push(ProfileRowLayout { index: i, row, rename, delete });
    }
    let list_bottom = if profile_count == 0 {
        list_top
    } else {
        list_top + profile_count as f32 * (ACCOUNT_ROW_H + ACCOUNT_ROW_GAP) - ACCOUNT_ROW_GAP
    };
    let btn_top = list_bottom + 18.0;
    let new_button = Rect::from_xywh(content_left, btn_top, PROFILE_BTN_W, ADD_BTN_H);
    let dup_button = Rect::from_xywh(new_button.right + 12.0, btn_top, PROFILE_BTN_W, ADD_BTN_H);

    ProfileTabLayout {
        content_left,
        content_right,
        header_top,
        rows,
        new_button,
        dup_button,
    }
}

fn draw_profiles_tab(canvas: &Canvas, fonts: &FontStore, card_w: f32, prefs: &Prefs, view: ProfileView<'_>) {
    let layout = profiles_tab_layout(fonts, card_w, view.profiles.len());

    // Body copy.
    let body_font = fonts.newsreader(15.0);
    let mut body_paint = Paint::default();
    body_paint.set_anti_alias(true);
    body_paint.set_color(Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5)); // mid-pearl
    let (_, bm) = body_font.metrics();
    canvas.draw_str(
        "a client profile bundles your cosmetic + perf settings — switch any time.",
        (layout.content_left, layout.header_top + (-bm.ascent)),
        &body_font,
        &body_paint,
    );

    // Profile rows.
    let can_delete = view.profiles.len() > 1;
    for (rl, row) in layout.rows.iter().zip(view.profiles) {
        let hovered = prefs.profile_hover == Some(ProfileHover::Row(rl.index));
        let rename_hovered = prefs.profile_hover == Some(ProfileHover::Rename(rl.index));
        let delete_hovered = prefs.profile_hover == Some(ProfileHover::Delete(rl.index));
        let renaming = if prefs.profile_renaming == Some(rl.index) {
            Some((
                prefs.profile_rename_buffer.as_str(),
                prefs.profile_rename_focus_time,
            ))
        } else {
            None
        };
        draw_profile_row(
            canvas,
            fonts,
            rl,
            row,
            hovered,
            rename_hovered,
            delete_hovered,
            can_delete,
            renaming,
        );
    }

    // New / Duplicate buttons.
    draw_vghost_btn(
        canvas,
        layout.new_button,
        "New profile",
        &prefs.profile_new,
        GhostKind::Pearl,
        fonts,
    );
    draw_vghost_btn(
        canvas,
        layout.dup_button,
        "Duplicate current",
        &prefs.profile_dup,
        GhostKind::Pearl,
        fonts,
    );
}

/// Draw one profile row — background, name (or rename field), rename-✎,
/// active marker, delete-×.
#[allow(clippy::too_many_arguments)]
fn draw_profile_row(
    canvas: &Canvas,
    fonts: &FontStore,
    layout: &ProfileRowLayout,
    view: &ProfileRowView<'_>,
    hovered: bool,
    rename_hovered: bool,
    delete_hovered: bool,
    can_delete: bool,
    renaming: Option<(&str, f32)>,
) {
    let row = layout.row;
    let rrect = RRect::new_rect_xy(row, 12.0, 12.0);

    if view.active {
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.10), None);
        canvas.draw_rrect(rrect, &bg);
        let mut rim = Paint::default();
        rim.set_anti_alias(true);
        rim.set_style(PaintStyle::Stroke);
        rim.set_stroke_width(1.0);
        rim.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.32), None);
        canvas.draw_rrect(rrect, &rim);
    } else if hovered {
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 0.05), None);
        canvas.draw_rrect(rrect, &bg);
    }

    let cy = (row.top + row.bottom) * 0.5;

    let name_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 360.0, None);
    let (_, nm) = name_font.metrics();
    let name_x = row.left + 18.0;
    let name_baseline = cy + nm.cap_height * 0.5;

    if let Some((buffer, focus_time)) = renaming {
        // Inline rename — a text field with the buffer + a blinking caret.
        // Submits on Enter, cancels on Escape (handled in main.rs).
        let ih = -nm.ascent + nm.descent + 8.0;
        let input = Rect::from_ltrb(
            name_x - 8.0,
            cy - ih * 0.5,
            layout.rename.left - 12.0,
            cy + ih * 0.5,
        );
        let irr = RRect::new_rect_xy(input, 9.0, 9.0);
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.06), None);
        canvas.draw_rrect(irr, &bg);
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_style(PaintStyle::Stroke);
        glow.set_stroke_width(3.0);
        glow.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.12), None);
        glow.set_mask_filter(skia_safe::MaskFilter::blur(
            skia_safe::BlurStyle::Normal,
            7.0,
            false,
        ));
        canvas.draw_rrect(irr, &glow);
        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.0);
        border.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.50), None);
        canvas.draw_rrect(irr, &border);

        let mut text_paint = Paint::default();
        text_paint.set_anti_alias(true);
        text_paint.set_color(TEXT_PEARL);
        canvas.draw_str(buffer, (name_x, name_baseline), &name_font, &text_paint);

        let blink = ((focus_time * 1.6).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
        let alpha = blink * blink;
        if alpha > 0.05 {
            let typed_w = if buffer.is_empty() {
                0.0
            } else {
                name_font.measure_str(buffer, Some(&text_paint)).0
            };
            let caret_x = name_x + typed_w + 1.0;
            let mut caret = Paint::default();
            caret.set_anti_alias(true);
            caret.set_style(PaintStyle::Stroke);
            caret.set_stroke_width(2.0);
            caret.set_color4f(Color4f::new(1.0, 246.0 / 255.0, 240.0 / 255.0, alpha), None);
            canvas.draw_line(
                (caret_x, name_baseline + nm.ascent),
                (caret_x, name_baseline + nm.descent),
                &caret,
            );
        }
    } else {
        // Name — Fraunces, vertically centered.
        let mut name_paint = Paint::default();
        name_paint.set_anti_alias(true);
        name_paint.set_color(Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA));
        canvas.draw_str(view.name, (name_x, name_baseline), &name_font, &name_paint);
    }

    // Active marker — rose dot left of the rename button.
    if view.active {
        let mut dot = Paint::default();
        dot.set_anti_alias(true);
        dot.set_color(Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5));
        canvas.draw_circle((layout.rename.left - 14.0, cy), 3.5, &dot);
    }

    // Rename-✎ button.
    {
        let rn = layout.rename;
        let rcx = (rn.left + rn.right) * 0.5;
        let rcy = (rn.top + rn.bottom) * 0.5;
        if rename_hovered {
            let mut disc = Paint::default();
            disc.set_anti_alias(true);
            disc.set_color4f(Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.14), None);
            canvas.draw_circle((rcx, rcy), 13.0, &disc);
        }
        let icon_color = if rename_hovered || renaming.is_some() {
            Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 1.0)
        } else {
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.45)
        };
        draw_pencil_icon(canvas, rcx, rcy, icon_color);
    }

    // Delete-× — hidden when there's only one profile (can't delete the last).
    if can_delete {
        let rm = layout.delete;
        let rcx = (rm.left + rm.right) * 0.5;
        let rcy = (rm.top + rm.bottom) * 0.5;
        if delete_hovered {
            let mut disc = Paint::default();
            disc.set_anti_alias(true);
            disc.set_color4f(Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, 0.18), None);
            canvas.draw_circle((rcx, rcy), 13.0, &disc);
        }
        let mut x = Paint::default();
        x.set_anti_alias(true);
        x.set_style(PaintStyle::Stroke);
        x.set_stroke_width(1.6);
        x.set_stroke_cap(PaintCap::Round);
        let alpha = if delete_hovered { 0.95 } else { 0.40 };
        x.set_color4f(Color4f::new(201.0 / 255.0, 106.0 / 255.0, 122.0 / 255.0, alpha), None);
        let d = 5.0;
        canvas.draw_line((rcx - d, rcy - d), (rcx + d, rcy + d), &x);
        canvas.draw_line((rcx + d, rcy - d), (rcx - d, rcy + d), &x);
    }
}

/// Custom-drawn pencil icon, ~14×14 centred at `(cx, cy)` — the rename
/// affordance on a profile row. The bundled fonts carry no pencil glyph,
/// so it's drawn directly (mirrors `instances::draw_rename_icon`).
fn draw_pencil_icon(canvas: &Canvas, cx: f32, cy: f32, color: Color4f) {
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_style(PaintStyle::Stroke);
    p.set_stroke_width(1.6);
    p.set_color4f(color, None);
    p.set_stroke_cap(PaintCap::Round);
    // Diagonal pencil body, top-right → bottom-left.
    canvas.draw_line((cx + 5.0, cy - 5.0), (cx - 4.0, cy + 4.0), &p);
    // Cap stroke perpendicular to the body at the top-right end.
    canvas.draw_line((cx + 3.0, cy - 7.0), (cx + 7.0, cy - 3.0), &p);
    // Tiny writing tip at the bottom-left end.
    let mut tip = Paint::default();
    tip.set_anti_alias(true);
    tip.set_color4f(color, None);
    canvas.draw_circle((cx - 5.0, cy + 5.0), 1.0, &tip);
}

// ── Keybinds tab ─────────────────────────────────────────────────────────

/// Width of a keybind row's chord button.
const KEYBIND_CHORD_W: f32 = 176.0;
/// Height of a keybind row's chord button.
const KEYBIND_CHORD_H: f32 = 34.0;

/// Card-local layout of one keybind row: the full row rect and the chord
/// button nested at its right edge (the only clickable part — click to
/// start a rebind).
pub struct KeybindRowLayout {
    pub index: usize,
    pub row: Rect,
    pub chord: Rect,
}

/// Card-local layout of the Keybinds tab.
pub struct KeybindTabLayout {
    pub content_left: f32,
    pub content_right: f32,
    pub header_top: f32,
    pub rows: Vec<KeybindRowLayout>,
    pub reset_button: Rect,
}

/// Compute the Keybinds tab's layout for `row_count` keybind actions.
pub fn keybinds_tab_layout(fonts: &FontStore, card_w: f32, row_count: usize) -> KeybindTabLayout {
    let body_top = HEADER_BOTTOM + 8.0 + 16.0;
    let panel_left = BODY_PAD_X + SIDEBAR_WIDTH + COL_GAP;
    let panel_right = card_w - BODY_PAD_X;
    let content_left = panel_left + PANEL_INNER_PAD_X;
    let content_right = panel_right - PANEL_INNER_PAD_X;
    let content_top = body_top + PANEL_INNER_PAD_Y;
    let header_top = section_head_bottom(content_top, fonts);

    let list_top = header_top + ACCOUNT_COPY_BLOCK;
    let mut rows = Vec::with_capacity(row_count);
    for i in 0..row_count {
        let top = list_top + i as f32 * (ACCOUNT_ROW_H + ACCOUNT_ROW_GAP);
        let row = Rect::from_ltrb(content_left, top, content_right, top + ACCOUNT_ROW_H);
        let cy = (row.top + row.bottom) * 0.5;
        let chord = Rect::from_xywh(
            row.right - KEYBIND_CHORD_W - 8.0,
            cy - KEYBIND_CHORD_H * 0.5,
            KEYBIND_CHORD_W,
            KEYBIND_CHORD_H,
        );
        rows.push(KeybindRowLayout { index: i, row, chord });
    }
    let list_bottom = if row_count == 0 {
        list_top
    } else {
        list_top + row_count as f32 * (ACCOUNT_ROW_H + ACCOUNT_ROW_GAP) - ACCOUNT_ROW_GAP
    };
    let btn_top = list_bottom + 18.0;
    let reset_button = Rect::from_xywh(content_left, btn_top, PROFILE_BTN_W, ADD_BTN_H);

    KeybindTabLayout {
        content_left,
        content_right,
        header_top,
        rows,
        reset_button,
    }
}

fn draw_keybinds_tab(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    prefs: &Prefs,
    view: KeybindView<'_>,
) {
    let layout = keybinds_tab_layout(fonts, card_w, view.rows.len());

    // Body copy.
    let body_font = fonts.newsreader(15.0);
    let mut body_paint = Paint::default();
    body_paint.set_anti_alias(true);
    body_paint.set_color(Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5)); // mid-pearl
    let (_, bm) = body_font.metrics();
    canvas.draw_str(
        "keys are stored per profile — bind once, they follow this profile.",
        (layout.content_left, layout.header_top + (-bm.ascent)),
        &body_font,
        &body_paint,
    );

    // Keybind rows.
    for (rl, row) in layout.rows.iter().zip(view.rows) {
        let hovered = prefs.keybind_hover == Some(rl.index);
        draw_keybind_row(canvas, fonts, rl, row, hovered);
    }

    // Reset button.
    draw_vghost_btn(
        canvas,
        layout.reset_button,
        "Reset to defaults",
        &prefs.keybind_reset,
        GhostKind::Pearl,
        fonts,
    );
}

/// Draw one keybind row — action label, module eyebrow, and the chord button.
fn draw_keybind_row(
    canvas: &Canvas,
    fonts: &FontStore,
    layout: &KeybindRowLayout,
    view: &KeybindRowView<'_>,
    hovered: bool,
) {
    let row = layout.row;
    let cy = (row.top + row.bottom) * 0.5;

    // Action label — Fraunces, just above the row centre.
    let name_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 360.0, None);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color(TEXT_PEARL);
    canvas.draw_str(view.action_label, (row.left + 4.0, cy - 1.0), &name_font, &name_paint);

    // Module eyebrow — mono, tracked, below the label.
    let eb_font = fonts.jetbrains_mono(9.0);
    let mut eb_paint = Paint::default();
    eb_paint.set_anti_alias(true);
    eb_paint.set_color(TEXT_MAUVE_DEEP);
    canvas.draw_str(
        view.module.to_uppercase(),
        (row.left + 4.0, cy + 15.0),
        &eb_font,
        &eb_paint,
    );

    // Chord button — a pill carrying the bound key (or the capture prompt).
    let cb = layout.chord;
    let rr = RRect::new_rect_xy(cb, 9.0, 9.0);
    // Capturing: champagne; idle/hover: rose.
    let (r, g, b) = if view.capturing {
        (232.0 / 255.0, 212.0 / 255.0, 168.0 / 255.0)
    } else {
        (229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0)
    };
    let fill_a = if view.capturing {
        0.16
    } else if hovered {
        0.14
    } else {
        0.07
    };
    let rim_a = if view.capturing {
        0.60
    } else if hovered {
        0.44
    } else {
        0.24
    };
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(Color4f::new(r, g, b, fill_a), None);
    canvas.draw_rrect(rr, &fill);
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(Color4f::new(r, g, b, rim_a), None);
    canvas.draw_rrect(rr, &rim);

    // Chord text — centred, mono.
    let label = if view.capturing { "press a key…" } else { view.chord_label };
    let txt_font = fonts.jetbrains_mono(12.0);
    let mut txt_paint = Paint::default();
    txt_paint.set_anti_alias(true);
    if view.capturing {
        txt_paint.set_color(Color::from_argb(0xFF, 0xE8, 0xD4, 0xA8));
    } else {
        txt_paint.set_color(TEXT_PEARL);
    }
    let (tw, _) = txt_font.measure_str(label, Some(&txt_paint));
    let (_, tm) = txt_font.metrics();
    let tx = cb.left + (cb.width() - tw) * 0.5;
    let ty = (cb.top + cb.bottom) * 0.5 + tm.cap_height * 0.5;
    canvas.draw_str(label, (tx, ty), &txt_font, &txt_paint);
}

// ────────────────────────────────────────────────────────────────────────
// Modules tab — toggle EwoClient modules + the FOV Control slider (Phase G)
// ────────────────────────────────────────────────────────────────────────

/// Extra height below a module's name row for a setting slider strip.
const MODULE_SLIDER_STRIP: f32 = 42.0;

/// Card-local layout of one Modules-tab row: the full row, the on/off toggle
/// at its top-right, and — for a module with a setting — the slider strip.
pub struct ModuleRowLayout {
    pub index: usize,
    pub row: Rect,
    pub toggle: Rect,
    /// Slider rect for a module that carries a setting (FOV Control), else None.
    pub slider: Option<Rect>,
}

/// Card-local layout of the Modules tab.
pub struct ModuleTabLayout {
    pub content_left: f32,
    pub content_right: f32,
    pub header_top: f32,
    pub rows: Vec<ModuleRowLayout>,
}

/// Compute the Modules tab's layout — one row per `ewo_core::modules` entry.
pub fn modules_tab_layout(fonts: &FontStore, card_w: f32) -> ModuleTabLayout {
    let body_top = HEADER_BOTTOM + 8.0 + 16.0;
    let panel_left = BODY_PAD_X + SIDEBAR_WIDTH + COL_GAP;
    let panel_right = card_w - BODY_PAD_X;
    let content_left = panel_left + PANEL_INNER_PAD_X;
    let content_right = panel_right - PANEL_INNER_PAD_X;
    let content_top = body_top + PANEL_INNER_PAD_Y;
    let header_top = section_head_bottom(content_top, fonts);

    let list_top = header_top + ACCOUNT_COPY_BLOCK;
    let mut rows = Vec::with_capacity(ewo_core::modules::REGISTRY.len());
    let mut y = list_top;
    for (i, m) in ewo_core::modules::REGISTRY.iter().enumerate() {
        let has_slider = !m.settings.is_empty();
        let row_h = ACCOUNT_ROW_H + if has_slider { MODULE_SLIDER_STRIP } else { 0.0 };
        let row = Rect::from_ltrb(content_left, y, content_right, y + row_h);
        let top_cy = y + ACCOUNT_ROW_H * 0.5;
        let toggle = Rect::from_xywh(
            row.right - crate::widgets::TOGGLE_W,
            top_cy - crate::widgets::TOGGLE_H * 0.5,
            crate::widgets::TOGGLE_W,
            crate::widgets::TOGGLE_H,
        );
        let slider = if has_slider {
            Some(Rect::from_ltrb(
                content_left,
                y + ACCOUNT_ROW_H + 4.0,
                content_right - 56.0,
                y + ACCOUNT_ROW_H + MODULE_SLIDER_STRIP - 4.0,
            ))
        } else {
            None
        };
        rows.push(ModuleRowLayout { index: i, row, toggle, slider });
        y += row_h + ACCOUNT_ROW_GAP;
    }
    ModuleTabLayout { content_left, content_right, header_top, rows }
}

fn draw_modules_tab(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    prefs: &Prefs,
    time: f32,
    settings: &Settings,
) {
    let layout = modules_tab_layout(fonts, card_w);

    // Body copy.
    let body_font = fonts.newsreader(15.0);
    let mut body_paint = Paint::default();
    body_paint.set_anti_alias(true);
    body_paint.set_color(Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5));
    let (_, bm) = body_font.metrics();
    canvas.draw_str(
        "legit-client features — stored per profile, applied live in-game.",
        (layout.content_left, layout.header_top + (-bm.ascent)),
        &body_font,
        &body_paint,
    );

    for rl in &layout.rows {
        let def = &ewo_core::modules::REGISTRY[rl.index];
        draw_module_row(canvas, fonts, rl, def, prefs, time, settings);
    }
}

/// Draw one Modules-tab row — name, description, on/off toggle, and (for a
/// module with a setting) the slider strip beneath.
fn draw_module_row(
    canvas: &Canvas,
    fonts: &FontStore,
    layout: &ModuleRowLayout,
    def: &ewo_core::modules::ModuleDef,
    prefs: &Prefs,
    time: f32,
    settings: &Settings,
) {
    let row = layout.row;
    let top_cy = row.top + ACCOUNT_ROW_H * 0.5;

    // Name — Fraunces, just above the top-row centre.
    let name_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 360.0, None);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color(TEXT_PEARL);
    canvas.draw_str(def.name, (row.left + 4.0, top_cy - 1.0), &name_font, &name_paint);

    // Description — Newsreader, mauve, below.
    let desc_font = fonts.newsreader(13.0);
    let mut desc_paint = Paint::default();
    desc_paint.set_anti_alias(true);
    desc_paint.set_color(TEXT_MAUVE);
    canvas.draw_str(def.description, (row.left + 4.0, top_cy + 16.0), &desc_font, &desc_paint);

    // On/off toggle.
    if let Some(toggle) = prefs.module_toggles.get(layout.index) {
        draw_vtoggle(canvas, layout.toggle, toggle);
    }

    // Setting slider (FOV Control).
    if let Some(slider) = layout.slider {
        draw_vslider(canvas, slider, &prefs.module_fov, time, settings);
        draw_slider_value_label(
            canvas,
            fonts,
            Rect::from_ltrb(slider.right, slider.top, row.right, slider.bottom),
            prefs.module_fov.value,
            |v| format!("{}°", v.round() as i32),
        );
    }
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
            };
            out.push((slot, widget));
        }
        y = row_bottom;
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
pub fn sidebar_tab_bounds(fonts: &FontStore) -> [(SettingsTab, Rect); 8] {
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

    let mut out: [(SettingsTab, Rect); 8] = [
        (SettingsTab::Account, Rect::default()),
        (SettingsTab::Profiles, Rect::default()),
        (SettingsTab::Keybinds, Rect::default()),
        (SettingsTab::Modules, Rect::default()),
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
