//! Instances screen — list of saved Minecraft instances on the left, detail
//! panel on the right with metadata + Launch button.
//!
//! Step 13 scaffold: static layout, mock data, only the Launch button is
//! interactive. Sliders (RAM, render distance), version dropdown, mod list,
//! and rename input land when those widgets are built (steps 12+).
//!
//! CSS reference (`StyleSheet2`):
//! ```css
//! .screen-instances    { display: flex; flex-direction: column; }
//! .instances-body      { display: grid; grid-template-columns: 320px 1fr; }
//! .inst-list           { padding: 28px 0 28px 44px; border-right: 1px hairline; }
//! .inst-detail         { padding: 28px 44px; }
//! ```

use ewo_core::Settings;
use serde::{Deserialize, Serialize};
use skia_safe::{Canvas, Color, Color4f, Paint, PaintStyle, Rect};

use crate::text::{self, FontStore};
use crate::widgets::{
    draw_glass_panel, draw_scrollbar, draw_vbtn, draw_vdrop_head, draw_vdrop_menu, draw_vslider,
    menu_layout, VbtnState, VdropState, VsliderState,
};

const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MAUVE_DEEP: Color = Color::from_argb(0xFF, 0x6B, 0x55, 0x5C);
const TEXT_MID_PEARL: Color = Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5);

const HEADER_BOTTOM: f32 = 84.0; // tab bar takes 20-44, header content under it
const LIST_WIDTH: f32 = 320.0;
const LIST_LEFT_PAD: f32 = 44.0;
const DETAIL_PAD: f32 = 44.0;

// Glass-panel insets for the right-column detail. The CSS structure is
// `.inst-detail { padding: 28px 44px }` (outer container) wrapping a
// `<Panel style={{ padding: '36px 40px' }}>`. So:
//   panel_left = LIST_WIDTH + DETAIL_PAD (44 left padding of .inst-detail)
//   panel_top  = HEADER_BOTTOM + 28      (.inst-detail vertical padding)
// Inner content paddings (`PANEL_INNER_PAD_*`) are the panel's own padding.
const PANEL_INNER_PAD_X: f32 = 40.0;
const PANEL_INNER_PAD_Y: f32 = 36.0;
const PANEL_OUTER_PAD: f32 = 28.0;

// CSS `.inst-config { gap: 22; padding-bottom: 28 }` and
// `.inst-config-row { gap: 10 }`.
const CONFIG_ROW_GAP: f32 = 22.0;
const CONFIG_LABEL_TO_WIDGET_GAP: f32 = 10.0;
const CONFIG_TOP_GAP: f32 = 28.0; // gap below the head divider
const CONFIG_SLIDER_HEIGHT: f32 = 36.0;
const CONFIG_DROPDOWN_HEIGHT: f32 = 40.0;
const JAVA_RUNTIME_DROPDOWN_WIDTH: f32 = 320.0;

pub const JAVA_RUNTIME_OPTIONS: &[&str] = &[
    "Adoptium 21.0.4 (bundled)",
    "Adoptium 17.0.10",
    "Zulu 21.0.4",
    "Custom path…",
];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    Ram,
    RenderDist,
    JavaRuntime,
    /// Click-toggle for an individual mod's enable state. The `usize` is
    /// the row index in `selected_instance_mods()`.
    ModToggle(usize),
}

/// Sort mode for the Worlds list. Cycles through options when the sort
/// label is clicked: Newest → Oldest → A→Z → Z→A → Recently played → Newest.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    /// Most recent creation first (vec position 0 = newest because new
    /// instances are inserted at the front).
    #[default]
    Newest,
    Oldest,
    AlphaAsc,
    AlphaDesc,
    /// Most recently launched first.
    RecentlyPlayed,
}

impl SortMode {
    pub fn cycle(self) -> Self {
        match self {
            SortMode::Newest => SortMode::Oldest,
            SortMode::Oldest => SortMode::AlphaAsc,
            SortMode::AlphaAsc => SortMode::AlphaDesc,
            SortMode::AlphaDesc => SortMode::RecentlyPlayed,
            SortMode::RecentlyPlayed => SortMode::Newest,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::Newest => "newest first",
            SortMode::Oldest => "oldest first",
            SortMode::AlphaAsc => "A → Z",
            SortMode::AlphaDesc => "Z → A",
            SortMode::RecentlyPlayed => "recently played",
        }
    }
}

/// Compute the display order of instances under a given sort mode. The
/// returned `Vec<usize>` maps display position → underlying index in
/// `instances`. Renderer + click-to-select use this so the underlying
/// `Vec<Instance>` stays in stable insertion order.
pub fn display_order(instances: &[Instance], mode: SortMode) -> Vec<usize> {
    let mut order: Vec<usize> = (0..instances.len()).collect();
    match mode {
        SortMode::Newest => {} // vec is already newest-first via insert(0)
        SortMode::Oldest => order.reverse(),
        SortMode::AlphaAsc => order.sort_by(|&a, &b| {
            instances[a]
                .name
                .to_ascii_lowercase()
                .cmp(&instances[b].name.to_ascii_lowercase())
        }),
        SortMode::AlphaDesc => order.sort_by(|&a, &b| {
            instances[b]
                .name
                .to_ascii_lowercase()
                .cmp(&instances[a].name.to_ascii_lowercase())
        }),
        SortMode::RecentlyPlayed => {
            // Higher timestamp = more recent. Instances never launched
            // (timestamp 0.0) sort to the bottom.
            order.sort_by(|&a, &b| {
                instances[b]
                    .last_played_at
                    .partial_cmp(&instances[a].last_played_at)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
    order
}

impl Slot {
    pub fn is_dropdown(self) -> bool {
        matches!(self, Slot::JavaRuntime)
    }
}

/// State for the Instances detail panel's interactive controls. Lives in
/// `App`; the render path reads it; `main.rs` routes input back via the
/// per-widget `drive`/`handle` methods.
#[derive(Debug, Clone)]
pub struct InstancePrefs {
    /// Index into `INSTANCES` of the currently-selected instance. The
    /// detail panel renders this one; clicking a list row updates it.
    pub selected: usize,
    pub ram: VsliderState,
    pub render_dist: VsliderState,
    pub java_runtime: VdropState,
    /// Per-mod enable flag, indexed in lockstep with the selected
    /// instance's `mods` list. Re-initialized when `selected` changes via
    /// `select(idx)`.
    pub mods_on: Vec<bool>,
    /// Vertical scroll offset for the detail panel (pixels). Mouse wheel
    /// + clamp to `[0, max_scroll]` (computed from rendered content
    /// height vs panel inner height).
    pub detail_scroll: f32,
    /// Vertical scroll offset for the Worlds list (pixels). Independent
    /// of the detail-panel scroll — wheel events route to whichever side
    /// the cursor is over.
    pub list_scroll: f32,
    /// Index of the list row currently under the cursor (in *display*
    /// order, not underlying). Drives the row hover affordance.
    pub list_hover: Option<usize>,
    /// Cursor over the "+" new-instance button.
    pub add_hover: bool,
    /// Cursor over the sort-mode label.
    pub sort_hover: bool,
    /// Current display sort for the Worlds list.
    pub sort_mode: SortMode,
    /// Wall-clock second the most-recent instance was created. The
    /// renderer plays a fade + drop-in on the row at underlying index 0
    /// while `time - created_at < NEW_ROW_ANIM_S`. `None` once the
    /// animation has completed (or no instance has been created this
    /// session).
    pub created_at: Option<f32>,
    /// Wall-clock second the selection last changed. Drives a brief
    /// fade-in on the detail panel content so switching instances
    /// (whether by clicking a row or creating a new one) feels intentional.
    pub selected_at: Option<f32>,
    /// Display index of the row whose × button is currently under the
    /// cursor, if any. Brightens that one × glyph.
    pub delete_hover: Option<usize>,
    /// `true` while the user is editing the selected instance's name
    /// inline (clicked the ✎ icon next to it). Keyboard input goes to
    /// `rename_buffer`; Enter commits, Escape cancels.
    pub renaming: bool,
    pub rename_buffer: String,
    /// Wall-clock seconds since the rename field was focused — drives
    /// the caret blink, same pattern as the modal name field.
    pub rename_focus_time: f32,
    /// Cursor over the rename ✎ icon.
    pub rename_hover: bool,
}

/// Duration of the new-row drop-in animation. Silk-eased from 0 to 1.
pub const NEW_ROW_ANIM_S: f32 = 0.6;
/// Duration of the detail-panel fade-in on selection change.
pub const SELECT_ANIM_S: f32 = 0.35;

impl Default for InstancePrefs {
    fn default() -> Self {
        // Defaults match the React prototype. Selection starts at index 0;
        // the App initializes `mods_on` against the actual instance vec.
        Self {
            selected: 0,
            ram: VsliderState::new(8.0, 2.0, 16.0).with_step(1.0),
            render_dist: VsliderState::new(16.0, 4.0, 32.0).with_step(1.0),
            java_runtime: VdropState::new(0),
            mods_on: Vec::new(),
            detail_scroll: 0.0,
            list_scroll: 0.0,
            list_hover: None,
            add_hover: false,
            sort_hover: false,
            sort_mode: SortMode::Newest,
            created_at: None,
            selected_at: None,
            delete_hover: None,
            renaming: false,
            rename_buffer: String::new(),
            rename_focus_time: 0.0,
            rename_hover: false,
        }
    }
}

impl InstancePrefs {
    /// Switch the selected instance — pulls per-instance slider/dropdown
    /// values into the prefs state and resets transient UI state. No-op
    /// if the new index matches the current selection.
    pub fn select(&mut self, instances: &[Instance], idx: usize) {
        if idx >= instances.len() || idx == self.selected {
            return;
        }
        self.selected = idx;
        self.sync_from_instance(instances);
        self.detail_scroll = 0.0;
        self.java_runtime.close();
        // Cancel any in-flight rename — its buffer was for the previous
        // instance and would be confusing to keep.
        self.renaming = false;
        self.rename_buffer.clear();
    }

    /// Pull per-instance config into the prefs state for the
    /// currently-selected index. Called by `select()` and on App startup.
    pub fn sync_from_instance(&mut self, instances: &[Instance]) {
        if let Some(inst) = instances.get(self.selected) {
            self.ram.value = inst.ram as f32;
            self.render_dist.value = inst.render_distance as f32;
            self.java_runtime.selected = inst.java_runtime;
            self.mods_on = inst.mods.iter().map(|m| m.on).collect();
        }
    }

    /// Initialize per-instance state from the currently-selected
    /// instance. Kept under the original name as a back-compat alias for
    /// `sync_from_instance`.
    pub fn sync_mods(&mut self, instances: &[Instance]) {
        self.sync_from_instance(instances);
    }
}

impl InstancePrefs {
    pub fn tick(&mut self, dt: f32) {
        self.java_runtime.tick(dt);
    }

    pub fn close_dropdowns(&mut self) {
        self.java_runtime.close();
    }

    pub fn open_dropdown(&self) -> Option<Slot> {
        if self.java_runtime.open || self.java_runtime.anim > 0.001 {
            Some(Slot::JavaRuntime)
        } else {
            None
        }
    }

    pub fn dropdown_state(&self, slot: Slot) -> Option<&VdropState> {
        match slot {
            Slot::JavaRuntime => Some(&self.java_runtime),
            _ => None,
        }
    }

    pub fn dropdown_state_mut(&mut self, slot: Slot) -> Option<&mut VdropState> {
        match slot {
            Slot::JavaRuntime => Some(&mut self.java_runtime),
            _ => None,
        }
    }
}

pub fn dropdown_options(slot: Slot) -> Option<&'static [&'static str]> {
    match slot {
        Slot::JavaRuntime => Some(JAVA_RUNTIME_OPTIONS),
        _ => None,
    }
}

/// Runtime instance. Owned (not `&'static`) so the new-instance modal can
/// append to the list. Serialized to TOML on disk; deserialized on launch.
///
/// Per-instance config (`ram`, `render_distance`, `java_runtime`) lives
/// here so switching between worlds shows the right slider values for
/// each, instead of treating those as global preferences.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub version: String,
    /// Human-readable last-played string ("moments ago", "yesterday")
    /// shown in the list. Display-only; the canonical numeric form is
    /// `last_played_at`.
    pub last_played: String,
    /// Wall-clock seconds since the Unix epoch the last time this
    /// instance was launched. `0.0` for instances that have never been
    /// launched (the default mock list seeds these with hand-picked
    /// values so "recently played" sort still has meaningful order).
    #[serde(default)]
    pub last_played_at: f64,
    #[serde(default = "default_ram")]
    pub ram: u32,
    #[serde(default = "default_render_distance")]
    pub render_distance: u32,
    /// Index into `JAVA_RUNTIME_OPTIONS`.
    #[serde(default)]
    pub java_runtime: usize,
    #[serde(default)]
    pub mods: Vec<ModInfo>,
}

fn default_ram() -> u32 {
    8
}
fn default_render_distance() -> u32 {
    16
}

impl Instance {
    pub fn new(name: String, version: String, last_played: String, mods: Vec<ModInfo>) -> Self {
        Self {
            name,
            version,
            last_played,
            last_played_at: 0.0,
            ram: default_ram(),
            render_distance: default_render_distance(),
            java_runtime: 0,
            mods,
        }
    }

    pub fn with_config(mut self, ram: u32, render_distance: u32, java_runtime: usize) -> Self {
        self.ram = ram;
        self.render_distance = render_distance;
        self.java_runtime = java_runtime;
        self
    }

    /// Builder shortcut for assigning a relative-rank timestamp to the
    /// default instance list. Used by `default_instances` so the four
    /// pre-seeded worlds have meaningful "recently played" order.
    pub fn with_last_played_rank(mut self, secs_ago: f64) -> Self {
        let now = current_unix_seconds();
        self.last_played_at = now - secs_ago;
        self
    }
}

/// Current wall-clock seconds since the Unix epoch as `f64`. Used as the
/// canonical timestamp source for `Instance::last_played_at`.
pub fn current_unix_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModInfo {
    pub name: String,
    pub version: String,
    pub category: String,
    pub on: bool,
}

impl ModInfo {
    pub fn new(name: &str, version: &str, category: &str, on: bool) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            category: category.to_string(),
            on,
        }
    }
}

/// Build the default starter list. App holds a `Vec<Instance>` initialized
/// from this on launch; new instances created via the modal append to it.
pub fn default_instances() -> Vec<Instance> {
    let velvet_mods = vec![
        ModInfo::new("Sodium", "0.5.8", "performance", true),
        ModInfo::new("Iris Shaders", "1.7.0", "visuals", true),
        ModInfo::new("Distant Horizons", "2.0.4", "visuals", true),
        ModInfo::new("Continuity", "3.0.0", "visuals", true),
        ModInfo::new("Lithium", "0.12.7", "performance", true),
        ModInfo::new("Mod Menu", "9.0.0", "utility", true),
        ModInfo::new("Carpet", "1.4.112", "utility", false),
    ];
    let pearl_mods = vec![
        ModInfo::new("Sodium", "0.6.0", "performance", true),
        ModInfo::new("Create", "0.5.1", "gameplay", true),
        ModInfo::new("Farmer's Delight", "1.2.4", "gameplay", true),
        ModInfo::new("JEI", "15.3.0", "utility", true),
    ];
    vec![
        Instance::new(
            "Velvet Hours".into(),
            "VANILLA · 1.21".into(),
            "moments ago".into(),
            velvet_mods,
        )
        .with_config(8, 16, 0)
        .with_last_played_rank(60.0 * 5.0), // 5 min ago
        Instance::new(
            "Pearl Construct".into(),
            "FABRIC · 1.21".into(),
            "yesterday".into(),
            pearl_mods,
        )
        .with_config(12, 24, 0)
        .with_last_played_rank(60.0 * 60.0 * 24.0), // 1 day ago
        Instance::new(
            "Vanilla · L21".into(),
            "VANILLA · 1.21".into(),
            "3 days ago".into(),
            vec![],
        )
        .with_config(4, 12, 1)
        .with_last_played_rank(60.0 * 60.0 * 24.0 * 3.0), // 3 days ago
        Instance::new(
            "Snapshot · 24w40a".into(),
            "SNAPSHOT".into(),
            "last week".into(),
            vec![],
        )
        .with_config(6, 8, 0)
        .with_last_played_rank(60.0 * 60.0 * 24.0 * 7.0), // 7 days ago
    ]
}

/// Mods of the instance at index `selected`. Returns an empty slice if
/// the index is out of range.
pub fn instance_mods<'a>(instances: &'a [Instance], selected: usize) -> &'a [ModInfo] {
    instances.get(selected).map(|i| i.mods.as_slice()).unwrap_or(&[])
}

/// Card-local bounds of the Launch button. Hit-tested by `main.rs`.
///
/// Mirrors `.inst-detail-head .vbtn` — top-right of the glass panel's head
/// row, inset by the panel's inner padding.
pub fn launch_button_bounds(card_w: f32) -> Rect {
    let panel_right = card_w - DETAIL_PAD;
    let panel_top = HEADER_BOTTOM + PANEL_OUTER_PAD;
    let btn_w = 130.0;
    let btn_h = 50.0;
    let x = panel_right - PANEL_INNER_PAD_X - btn_w;
    let y = panel_top + PANEL_INNER_PAD_Y;
    Rect::from_xywh(x, y, btn_w, btn_h)
}

fn panel_bounds(card_w: f32, card_h: f32) -> Rect {
    Rect::from_ltrb(
        LIST_WIDTH + DETAIL_PAD,
        HEADER_BOTTOM + PANEL_OUTER_PAD,
        card_w - DETAIL_PAD,
        card_h - PANEL_OUTER_PAD,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn draw_instances(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    h: f32,
    time: f32,
    settings: &Settings,
    launch_button: &VbtnState,
    prefs: &InstancePrefs,
    instances: &[Instance],
) {
    draw_screen_head(canvas, fonts, w);
    draw_list(canvas, fonts, h, time, prefs, instances);
    draw_detail(canvas, fonts, w, h, time, settings, launch_button, prefs, instances);
}

// ────────────────────────────────────────────────────────────────────────
// Screen head — back button + screen-eyebrow
// ────────────────────────────────────────────────────────────────────────

fn draw_screen_head(canvas: &Canvas, fonts: &FontStore, w: f32) {
    // CSS: `.screen-head { padding: 28px 44px; border-bottom: 1px hairline }`
    // Sits under the tab bar (20-44 px region). We start at y=58 to leave
    // breathing room.
    let head_y = 58.0;

    // Left: back button "← Main menu"
    let back_font = fonts.fraunces_axes(20.0, 50.0, 0.0, 300.0, None);
    let mut back_paint = Paint::default();
    back_paint.set_anti_alias(true);
    back_paint.set_color(TEXT_MID_PEARL);
    let (_, bm) = back_font.metrics();
    let back_baseline = head_y + (-bm.ascent);
    canvas.draw_str("← Main menu", (LIST_LEFT_PAD, back_baseline), &back_font, &back_paint);

    // Right: screen-eyebrow "INSTANCES"
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let mut eyebrow_paint = Paint::default();
    eyebrow_paint.set_anti_alias(true);
    eyebrow_paint.set_color(TEXT_MAUVE);
    let label = "INSTANCES";
    let advance = text::measure_tracked_em(&eyebrow_font, label, 0.35);
    let (_, em) = eyebrow_font.metrics();
    let eyebrow_baseline = head_y + 4.0 + (-em.ascent);
    text::draw_tracked_em(
        canvas,
        label,
        (w - DETAIL_PAD - advance, eyebrow_baseline),
        &eyebrow_font,
        &eyebrow_paint,
        0.35,
    );

    // Hairline divider under header
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
// Instance list (left column)
// ────────────────────────────────────────────────────────────────────────

fn draw_list(
    canvas: &Canvas,
    fonts: &FontStore,
    h: f32,
    time: f32,
    prefs: &InstancePrefs,
    instances: &[Instance],
) {
    let selected = prefs.selected;
    // New-row drop-in envelope. `Some(progress)` while the most recent
    // create is within the animation window; `None` once it's expired.
    let new_row_anim = prefs.created_at.and_then(|start| {
        let elapsed = (time - start).max(0.0);
        if elapsed < NEW_ROW_ANIM_S {
            Some(elapsed / NEW_ROW_ANIM_S)
        } else {
            None
        }
    });
    let list_top = HEADER_BOTTOM + 28.0;
    let row_h: f32 = 70.0;

    // List head: "Worlds" title + "+" button
    let head_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let mut head_paint = Paint::default();
    head_paint.set_anti_alias(true);
    head_paint.set_color(TEXT_PEARL);
    let (_, hm) = head_font.metrics();
    let head_baseline = list_top + (-hm.ascent);
    canvas.draw_str("Worlds", (LIST_LEFT_PAD, head_baseline), &head_font, &head_paint);

    // "+" New-instance button — hairline-rose rrect at the right edge of
    // the list head with a Fraunces 20 plus-sign. Border + glyph brighten
    // on hover; soft outer rose glow ramps in.
    let add_rect = add_button_bounds();
    let add_rrect = skia_safe::RRect::new_rect_xy(add_rect, 8.0, 8.0);
    let add_hover = prefs.add_hover;

    if add_hover {
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_style(PaintStyle::Stroke);
        glow.set_stroke_width(4.0);
        glow.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.20),
            None,
        );
        glow.set_mask_filter(skia_safe::MaskFilter::blur(
            skia_safe::BlurStyle::Normal,
            6.0,
            false,
        ));
        canvas.draw_rrect(add_rrect, &glow);
    }

    let mut add_border = Paint::default();
    add_border.set_anti_alias(true);
    add_border.set_style(PaintStyle::Stroke);
    add_border.set_stroke_width(1.0);
    let border_alpha = if add_hover { 0.60 } else { 0.20 };
    add_border.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, border_alpha),
        None,
    );
    canvas.draw_rrect(add_rrect, &add_border);

    let plus_font = fonts.fraunces_axes(20.0, 50.0, 0.0, 300.0, None);
    let mut plus_paint = Paint::default();
    plus_paint.set_anti_alias(true);
    plus_paint.set_color(if add_hover {
        Color::from_argb(0xFF, 0xFF, 0xF6, 0xF0)
    } else {
        Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5)
    });
    let (plus_advance, _) = plus_font.measure_str("+", Some(&plus_paint));
    let (_, pm) = plus_font.metrics();
    let plus_cx = (add_rect.left + add_rect.right) * 0.5;
    let plus_cy = (add_rect.top + add_rect.bottom) * 0.5;
    let plus_baseline = plus_cy + pm.cap_height * 0.5;
    canvas.draw_str(
        "+",
        (plus_cx - plus_advance * 0.5, plus_baseline),
        &plus_font,
        &plus_paint,
    );

    // Sort label — small mono-tracked clickable below the head, above the
    // hairline divider. Click cycles through sort modes.
    let sort_baseline = head_baseline + hm.descent + 22.0;
    draw_sort_label(canvas, fonts, sort_baseline, prefs.sort_mode, prefs.sort_hover);

    // Hairline below head + sort row
    let head_div_y = sort_baseline + 8.0;
    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line(
        (LIST_LEFT_PAD, head_div_y),
        (LIST_WIDTH - 24.0, head_div_y),
        &div,
    );

    // List rows — clipped to the visible band, translated by -list_scroll.
    let rows_top = head_div_y + 12.0;
    let rows_bottom = h - 28.0;
    let order = display_order(instances, prefs.sort_mode);

    let saved = canvas.save();
    let clip_rect = Rect::from_ltrb(0.0, rows_top, LIST_WIDTH, rows_bottom);
    canvas.clip_rect(clip_rect, skia_safe::ClipOp::Intersect, true);
    canvas.translate((0.0, -prefs.list_scroll));

    let mut y = rows_top;
    let mut row_div_paint = Paint::default();
    row_div_paint.set_anti_alias(true);
    row_div_paint.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.06),
        None,
    );
    for (display_idx, &underlying_idx) in order.iter().enumerate() {
        let inst = &instances[underlying_idx];
        let is_selected = underlying_idx == selected;
        let is_hovered = prefs.list_hover == Some(display_idx);
        let is_delete_hover = prefs.delete_hover == Some(display_idx);
        // Newly-created instance always lives at underlying index 0 —
        // animate that one's drop-in.
        let row_anim = if underlying_idx == 0 { new_row_anim } else { None };
        // Hairline between consecutive rows. Skip before the first row;
        // the gradient mask on the hover background still sits cleanly
        // under the line.
        if display_idx > 0 {
            canvas.draw_line(
                (LIST_LEFT_PAD - 4.0, y),
                (LIST_WIDTH - 24.0, y),
                &row_div_paint,
            );
        }
        draw_list_row(
            canvas, fonts, inst, y, row_h, is_selected, is_hovered, row_anim, is_delete_hover,
        );
        y += row_h;
    }
    canvas.restore_to_count(saved);

    // Scrollbar inside the list region — only renders when content > visible.
    let list_visible = Rect::from_ltrb(0.0, rows_top, LIST_WIDTH, rows_bottom);
    let list_content_h = (instances.len() as f32) * row_h;
    draw_scrollbar(canvas, list_visible, prefs.list_scroll, list_content_h);

    // Right border separating list from detail
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line(
        (LIST_WIDTH, HEADER_BOTTOM),
        (LIST_WIDTH, h - 28.0),
        &border,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_list_row(
    canvas: &Canvas,
    fonts: &FontStore,
    inst: &Instance,
    y: f32,
    h: f32,
    selected: bool,
    hovered: bool,
    // `Some(progress)` (0..1) plays the new-instance drop-in animation:
    // fade + downward translate + a brief rose halo behind the row.
    anim: Option<f32>,
    // `true` when the cursor is specifically over this row's × button.
    delete_hover: bool,
) {
    // Apply the entrance animation as a save-layer that wraps the entire
    // row drawing — alpha + translate compose cleanly without needing
    // every paint to know about it.
    let saved_count = if let Some(p) = anim {
        let eased = ewo_core::CubicBezier::SILK.eval(p.clamp(0.0, 1.0));
        let alpha = eased;
        let dy = (1.0 - eased) * -14.0;
        let row_layer_bounds = Rect::from_xywh(0.0, y - 16.0, LIST_WIDTH, h + 32.0);
        let s = canvas.save_layer_alpha_f(row_layer_bounds, alpha);
        canvas.translate((0.0, dy));

        // Soft rose halo behind the row, fades out over the second half
        // of the animation. Reads as "this row just arrived."
        if eased < 1.0 {
            let halo_alpha = (1.0 - eased) * 0.25;
            let halo_rect = Rect::from_xywh(8.0, y + 2.0, LIST_WIDTH - 16.0, h - 4.0);
            let halo_rrect = skia_safe::RRect::new_rect_xy(halo_rect, 12.0, 12.0);
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, halo_alpha),
                None,
            );
            halo.set_mask_filter(skia_safe::MaskFilter::blur(
                skia_safe::BlurStyle::Normal,
                10.0,
                false,
            ));
            canvas.draw_rrect(halo_rrect, &halo);
        }

        Some(s)
    } else {
        None
    };

    let label_top = y + 14.0;

    // Hover background — faint rose fill across the row (inset from both
    // edges with rounded corners so the highlight sits as a card not a
    // full-bleed strip).
    if hovered && !selected {
        let mut hover_bg = Paint::default();
        hover_bg.set_anti_alias(true);
        hover_bg.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.04),
            None,
        );
        let hover_rect = Rect::from_xywh(12.0, y + 3.0, LIST_WIDTH - 24.0, h - 6.0);
        let hover_rrect = skia_safe::RRect::new_rect_xy(hover_rect, 10.0, 10.0);
        canvas.draw_rrect(hover_rrect, &hover_bg);
    }

    // Active accent mark (left vertical bar). Hover gets a thinner / dimmer
    // version so the cursor location is always visible.
    if selected {
        let mut mark = Paint::default();
        mark.set_anti_alias(true);
        mark.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.85),
            None,
        );
        canvas.draw_rect(
            Rect::from_xywh(LIST_LEFT_PAD - 16.0, label_top + 4.0, 3.0, 32.0),
            &mark,
        );
    } else if hovered {
        let mut mark = Paint::default();
        mark.set_anti_alias(true);
        mark.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.30),
            None,
        );
        canvas.draw_rect(
            Rect::from_xywh(LIST_LEFT_PAD - 16.0, label_top + 8.0, 2.0, 24.0),
            &mark,
        );
    }

    // Name (Fraunces 18, light) — pearl-hot when selected, plain pearl
    // otherwise, dimmer when neither selected nor hovered.
    let name_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color(if selected {
        Color::from_argb(0xFF, 0xFF, 0xF6, 0xF0)
    } else if hovered {
        TEXT_PEARL
    } else {
        TEXT_PEARL
    });
    if !selected && !hovered {
        name_paint.set_alpha_f(0.85);
    }
    let (_, nm) = name_font.metrics();
    let name_baseline = label_top + (-nm.ascent);
    canvas.draw_str(&inst.name, (LIST_LEFT_PAD, name_baseline), &name_font, &name_paint);

    // Meta below name (JetBrains Mono small caps tracked)
    let meta_font = fonts.jetbrains_mono(10.0);
    let mut meta_paint = Paint::default();
    meta_paint.set_anti_alias(true);
    meta_paint.set_color(TEXT_MAUVE);
    let (_, mm) = meta_font.metrics();
    let meta_top = name_baseline + nm.descent + 6.0;
    let meta_baseline = meta_top + (-mm.ascent);
    text::draw_tracked_em(
        canvas,
        &inst.version,
        (LIST_LEFT_PAD, meta_baseline),
        &meta_font,
        &meta_paint,
        0.18,
    );

    // Last-played timestamp on the right (italic Newsreader, very dim)
    let ts_font = fonts.newsreader(11.0);
    let mut ts_paint = Paint::default();
    ts_paint.set_anti_alias(true);
    ts_paint.set_color(TEXT_MAUVE_DEEP);
    let (ts_advance, _) = ts_font.measure_str(&inst.last_played, Some(&ts_paint));
    let (_, tm) = ts_font.metrics();
    let ts_baseline = meta_top + (-tm.ascent);
    let ts_right = LIST_WIDTH - 44.0; // leave room for the × button
    canvas.draw_str(
        &inst.last_played,
        (ts_right - ts_advance, ts_baseline),
        &ts_font,
        &ts_paint,
    );

    // × delete button — small JetBrains Mono glyph at the right edge.
    // Faint by default, brightens when the cursor is on it. We render it
    // even when the row isn't list-hovered so users can see the
    // affordance without rolling over the whole row first.
    let x_font = fonts.jetbrains_mono(13.0);
    let mut x_paint = Paint::default();
    x_paint.set_anti_alias(true);
    if delete_hover {
        x_paint.set_color(Color::from_argb(0xFF, 0xC9, 0x6A, 0x7A));
    } else if hovered || selected {
        x_paint.set_color4f(
            Color4f::new(196.0 / 255.0, 175.0 / 255.0, 181.0 / 255.0, 0.50),
            None,
        );
    } else {
        x_paint.set_color4f(
            Color4f::new(196.0 / 255.0, 175.0 / 255.0, 181.0 / 255.0, 0.20),
            None,
        );
    }
    let glyph = "×";
    let (glyph_advance, _) = x_font.measure_str(glyph, Some(&x_paint));
    let (_, xm) = x_font.metrics();
    let x_cy = y + h * 0.5;
    let x_cx = LIST_WIDTH - 22.0;
    let x_baseline = x_cy + xm.cap_height * 0.5;
    canvas.draw_str(
        glyph,
        (x_cx - glyph_advance * 0.5, x_baseline),
        &x_font,
        &x_paint,
    );

    // Close the entrance-animation layer if one was opened.
    if let Some(s) = saved_count {
        canvas.restore_to_count(s);
    }
}

// ────────────────────────────────────────────────────────────────────────
// Detail panel (right column)
// ────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_detail(
    canvas: &Canvas,
    fonts: &FontStore,
    w: f32,
    h: f32,
    time: f32,
    settings: &Settings,
    launch_button: &VbtnState,
    prefs: &InstancePrefs,
    instances: &[Instance],
) {
    let panel = panel_bounds(w, h);
    let content_left = panel.left + PANEL_INNER_PAD_X;
    let content_right = panel.right - PANEL_INNER_PAD_X;
    let content_top = panel.top + PANEL_INNER_PAD_Y;

    let Some(selected) = instances.get(prefs.selected).or_else(|| instances.first()) else {
        // Empty list — nothing to draw.
        return;
    };
    let btn_bounds = launch_button_bounds(w);
    let head_div_y = head_divider_y(content_top, fonts);
    let cfg_top = head_div_y + CONFIG_TOP_GAP;

    let scroll = prefs.detail_scroll;

    // Selection-change fade-in. Wraps the inner content in a save-layer
    // with progressive alpha + a 12px translateY so switching instances
    // (or creating a new one, which calls `select()` after insert) reads
    // as a deliberate transition rather than a hard snap.
    let select_anim = prefs.selected_at.and_then(|start| {
        let elapsed = (time - start).max(0.0);
        if elapsed < SELECT_ANIM_S {
            Some(elapsed / SELECT_ANIM_S)
        } else {
            None
        }
    });

    draw_glass_panel(canvas, panel, true, time, settings, |canvas| {
        // Scroll the entire detail content (head + Launch + config + mods)
        // as one unit. The glass panel's clip bounds the visible region;
        // anything outside is hidden.
        let saved = canvas.save();

        // Selection animation layer — wraps everything below.
        let layer_count = if let Some(p) = select_anim {
            let eased = ewo_core::CubicBezier::SILK.eval(p.clamp(0.0, 1.0));
            let alpha = eased;
            let dy = (1.0 - eased) * 12.0;
            let s = canvas.save_layer_alpha_f(panel, alpha);
            canvas.translate((0.0, dy));
            Some(s)
        } else {
            None
        };

        canvas.translate((0.0, -scroll));

        draw_head(canvas, fonts, content_left, content_top, selected, prefs, time, settings);
        draw_head_divider(canvas, content_left, content_right, head_div_y);

        // Launch button sits inside the panel at top-right of the head row.
        draw_vbtn(
            canvas,
            btn_bounds,
            "Launch",
            launch_button,
            time,
            settings.motion_speed,
            fonts,
            true,
        );

        // Config rows + mod section
        let mods_top = draw_config_rows(
            canvas, fonts, content_left, content_right, cfg_top, time, settings, prefs,
        );
        if !selected.mods.is_empty() {
            // Reuse `selected_at` for the mod-row stagger animation —
            // CSS `mod-row-in` plays each row at delay `i * 50ms` over
            // ~300ms (opacity 0.4 → 1, translateY 4 → 0). We compute
            // `reveal_age = time - selected_at` and pass it through.
            let reveal_age = prefs
                .selected_at
                .map(|start| (time - start).max(0.0));
            draw_mod_section(
                canvas,
                fonts,
                content_left,
                content_right,
                mods_top,
                &selected.mods,
                &prefs.mods_on,
                reveal_age,
            );
        }

        canvas.restore_to_count(saved);

        // Close the selection-animation layer if we opened one.
        if let Some(s) = layer_count {
            canvas.restore_to_count(s);
        }

        // Detail-panel scrollbar — sits inside the glass panel's clip so
        // it doesn't bleed past the rounded corners. Drawn after the
        // scrolled content (and outside the selection-animation layer so
        // the bar stays at full opacity when content fades in).
        let detail_content_h = detail_content_height(w, h, fonts, prefs, instances);
        draw_scrollbar(canvas, panel, prefs.detail_scroll, detail_content_h);
    });

    // Portal-draw any open Java-runtime dropdown after the panel returns.
    if let Some(slot) = prefs.open_dropdown() {
        if let Some(state) = prefs.dropdown_state(slot) {
            if let Some(opts) = dropdown_options(slot) {
                if let Some(head) = dropdown_head_for_slot(slot, fonts, w, h, prefs, instances) {
                    let (menu, flip_up) = menu_layout(head, opts.len(), h);
                    draw_vdrop_menu(canvas, menu, flip_up, opts, state, fonts);
                }
            }
        }
    }
}

fn draw_head(
    canvas: &Canvas,
    fonts: &FontStore,
    content_left: f32,
    content_top: f32,
    selected: &Instance,
    prefs: &InstancePrefs,
    time: f32,
    settings: &Settings,
) {
    // Eyebrow
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let mut eyebrow_paint = Paint::default();
    eyebrow_paint.set_anti_alias(true);
    eyebrow_paint.set_color(TEXT_MAUVE);
    let (_, em) = eyebrow_font.metrics();
    let eyebrow_baseline = content_top + (-em.ascent);
    text::draw_tracked_em(
        canvas,
        &selected.last_played,
        (content_left, eyebrow_baseline),
        &eyebrow_font,
        &eyebrow_paint,
        0.30,
    );

    // Name — either rendered as Fraunces 36 text, or an editable input
    // when the user has clicked the ✎ icon.
    let name_font = fonts.fraunces_axes(36.0, 50.0, 1.0, 300.0, None);
    let (_, nm) = name_font.metrics();
    let name_top = eyebrow_baseline + em.descent + 8.0;
    let name_baseline = name_top + (-nm.ascent);

    if prefs.renaming {
        // Editable name field. 380px wide rrect with the buffer text and
        // a blinking caret. Submits on Enter, cancels on Escape (handled
        // in main.rs's KeyboardInput).
        let input_h = -nm.ascent + nm.descent + 8.0;
        let input_w = 420.0;
        let input_rect = Rect::from_xywh(
            content_left - 8.0,
            name_top - 4.0,
            input_w,
            input_h,
        );
        let input_rrect = skia_safe::RRect::new_rect_xy(input_rect, 10.0, 10.0);

        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.06),
            None,
        );
        canvas.draw_rrect(input_rrect, &bg);

        // Focus glow + border.
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_style(PaintStyle::Stroke);
        glow.set_stroke_width(3.0);
        glow.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.12),
            None,
        );
        glow.set_mask_filter(skia_safe::MaskFilter::blur(
            skia_safe::BlurStyle::Normal,
            7.0,
            false,
        ));
        canvas.draw_rrect(input_rrect, &glow);

        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.0);
        border.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.50),
            None,
        );
        canvas.draw_rrect(input_rrect, &border);

        // Buffer text (or empty placeholder dim).
        let mut text_paint = Paint::default();
        text_paint.set_anti_alias(true);
        text_paint.set_color(TEXT_PEARL);
        let display = &prefs.rename_buffer;
        canvas.draw_str(display, (content_left, name_baseline), &name_font, &text_paint);

        // Blinking caret right after the typed text.
        let blink = ((prefs.rename_focus_time * 1.6).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
        let alpha = blink * blink;
        if alpha > 0.05 {
            let typed_w = if display.is_empty() {
                0.0
            } else {
                name_font.measure_str(display, Some(&text_paint)).0
            };
            let caret_x = content_left + typed_w + 1.0;
            let caret_top = name_baseline + nm.ascent;
            let caret_bottom = name_baseline + nm.descent;
            let mut caret = Paint::default();
            caret.set_anti_alias(true);
            caret.set_style(PaintStyle::Stroke);
            caret.set_stroke_width(2.0);
            caret.set_color4f(
                Color4f::new(255.0 / 255.0, 246.0 / 255.0, 240.0 / 255.0, alpha),
                None,
            );
            canvas.draw_line((caret_x, caret_top), (caret_x, caret_bottom), &caret);
        }
    } else {
        // Plain name + ✎ icon next to it.
        let mut name_paint = Paint::default();
        name_paint.set_anti_alias(true);
        name_paint.set_color(TEXT_PEARL);
        canvas.draw_str(&selected.name, (content_left, name_baseline), &name_font, &name_paint);

        // Rename icon — small custom-drawn pencil next to the name.
        // We can't rely on the bundled Latin/mono fonts having ✎ U+270E,
        // so we draw it as paths: diagonal body + small cap stroke at
        // the top-right + a tiny tip mark at the bottom-left. Faint by
        // default, warm-white on hover.
        let icon_color = if prefs.rename_hover {
            Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, 1.0)
        } else {
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.40)
        };
        let (name_advance, _) = name_font.measure_str(&selected.name, Some(&name_paint));
        let icon_cx = content_left + name_advance + 18.0;
        let icon_cy = name_baseline - nm.cap_height * 0.5;
        draw_rename_icon(canvas, icon_cx, icon_cy, icon_color);
    }

    // Meta row — version pill (with active-status dot) + mods-count pill.
    // CSS `.meta-pill` chip primitive (see widgets/meta_pill.rs).
    let meta_top = name_baseline + nm.descent + 12.0;
    let mods_label = format!(
        "{} {}",
        selected.mods.len(),
        if selected.mods.len() == 1 { "MOD" } else { "MODS" },
    );
    let active_count = prefs.mods_on.iter().filter(|&&v| v).count();
    let active_label = format!("{} ACTIVE", active_count);
    let items: &[(&str, bool)] = &[
        (selected.version.as_str(), true),
        (mods_label.as_str(), false),
        (active_label.as_str(), false),
    ];
    crate::widgets::draw_meta_pill_row(
        canvas,
        (content_left, meta_top),
        items,
        time,
        settings.motion_speed,
        fonts,
    );
}

/// Custom-drawn pencil icon, ~14×14 centered at `(cx, cy)`. Used as the
/// rename affordance next to the instance name — the bundled fonts don't
/// carry a pencil glyph, so we draw one directly.
fn draw_rename_icon(canvas: &Canvas, cx: f32, cy: f32, color: Color4f) {
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_style(PaintStyle::Stroke);
    p.set_stroke_width(1.6);
    p.set_color4f(color, None);
    p.set_stroke_cap(skia_safe::PaintCap::Round);
    // Diagonal pencil body: top-right → bottom-left.
    canvas.draw_line((cx + 5.0, cy - 5.0), (cx - 4.0, cy + 4.0), &p);
    // Cap stroke perpendicular to the body at the top-right end.
    canvas.draw_line((cx + 3.0, cy - 7.0), (cx + 7.0, cy - 3.0), &p);
    // Short tip mark at the bottom-left end (the writing point).
    let mut tip = Paint::default();
    tip.set_anti_alias(true);
    tip.set_color4f(color, None);
    canvas.draw_circle((cx - 5.0, cy + 5.0), 1.0, &tip);
}

/// Y of the hairline that separates the head row from the inst-config rows.
/// Mirrors the layout in `draw_head` exactly so widget hit-testing can use
/// the same value without re-rendering. Meta row is now a pill stack
/// (~22px tall) rather than tracked text.
fn head_divider_y(content_top: f32, fonts: &FontStore) -> f32 {
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let name_font = fonts.fraunces_axes(36.0, 50.0, 1.0, 300.0, None);
    let (_, em) = eyebrow_font.metrics();
    let (_, nm) = name_font.metrics();
    let eyebrow_baseline = content_top + (-em.ascent);
    let name_top = eyebrow_baseline + em.descent + 8.0;
    let name_baseline = name_top + (-nm.ascent);
    let meta_top = name_baseline + nm.descent + 12.0;
    // Pill height = 2*PAD_Y(6) + font_size(10) = 22.
    meta_top + 22.0 + 24.0
}

fn draw_head_divider(canvas: &Canvas, left: f32, right: f32, y: f32) {
    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line((left, y), (right, y), &div);
}

#[allow(clippy::too_many_arguments)]
fn draw_config_rows(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    time: f32,
    settings: &Settings,
    prefs: &InstancePrefs,
) -> f32 {
    let mut y = top;

    // Row 1: RAM allocation
    y = draw_slider_row(
        canvas, fonts, left, right, y,
        "RAM allocation",
        &format!("{} GB", prefs.ram.value as i32),
        &prefs.ram, time, settings,
    );

    // Row 2: Render distance
    y = draw_slider_row(
        canvas, fonts, left, right, y,
        "Render distance",
        &format!("{} CHUNKS", prefs.render_dist.value as i32),
        &prefs.render_dist, time, settings,
    );

    // Row 3: Java runtime — label + dropdown head (no inline value)
    y = draw_dropdown_row(
        canvas, fonts, left, right, y,
        "Java runtime",
        JAVA_RUNTIME_OPTIONS[prefs.java_runtime.selected.min(JAVA_RUNTIME_OPTIONS.len() - 1)],
        &prefs.java_runtime,
        time,
        settings,
    );

    // Bottom hairline of `.inst-config { padding-bottom: 28; border-bottom: ... }`
    let div_y = y + 28.0 - CONFIG_ROW_GAP;
    let mut div = Paint::default();
    div.set_anti_alias(true);
    div.set_style(PaintStyle::Stroke);
    div.set_stroke_width(1.0);
    div.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.08),
        None,
    );
    canvas.draw_line((left, div_y), (right, div_y), &div);
    div_y + 28.0 // CSS `.inst-config { margin-bottom: 28 }` before the mods section
}

#[allow(clippy::too_many_arguments)]
fn draw_slider_row(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    label: &str,
    value_text: &str,
    state: &VsliderState,
    time: f32,
    settings: &Settings,
) -> f32 {
    let label_baseline_y = draw_config_label(canvas, fonts, left, right, top, label, value_text);
    let widget_top = label_baseline_y + CONFIG_LABEL_TO_WIDGET_GAP;
    let widget_rect = Rect::from_xywh(left, widget_top, right - left, CONFIG_SLIDER_HEIGHT);
    draw_vslider(canvas, widget_rect, state, time, settings);
    widget_top + CONFIG_SLIDER_HEIGHT + CONFIG_ROW_GAP
}

#[allow(clippy::too_many_arguments)]
fn draw_dropdown_row(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    label: &str,
    value_text: &str,
    state: &VdropState,
    time: f32,
    settings: &Settings,
) -> f32 {
    let label_baseline_y = draw_config_label(canvas, fonts, left, right, top, label, "");
    let widget_top = label_baseline_y + CONFIG_LABEL_TO_WIDGET_GAP;
    let head_w = JAVA_RUNTIME_DROPDOWN_WIDTH.min(right - left);
    let head = Rect::from_xywh(left, widget_top, head_w, CONFIG_DROPDOWN_HEIGHT);
    draw_vdrop_head(canvas, head, value_text, state, time, settings, fonts);
    let _ = value_text; // already drawn above
    widget_top + CONFIG_DROPDOWN_HEIGHT + CONFIG_ROW_GAP
}

/// Draw a `.inst-config-label` row — left-aligned italic Newsreader 14
/// label with a right-aligned mono value pill. Returns the baseline-bottom
/// (descent included) so the caller can offset the widget below.
fn draw_config_label(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    label: &str,
    value_text: &str,
) -> f32 {
    let label_font = fonts.newsreader(14.0);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color(TEXT_MID_PEARL);
    let (_, lm) = label_font.metrics();
    let label_baseline = top + (-lm.ascent);
    canvas.draw_str(label, (left, label_baseline), &label_font, &label_paint);

    if !value_text.is_empty() {
        let value_font = fonts.jetbrains_mono(11.0);
        let mut value_paint = Paint::default();
        value_paint.set_anti_alias(true);
        value_paint.set_color(Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5));
        let advance = text::measure_tracked_em(&value_font, value_text, 0.12);
        let (_, vm) = value_font.metrics();
        // Align the mono baseline to the italic label's baseline so the row
        // reads as a single horizontal band.
        let value_baseline = label_baseline - (lm.cap_height - vm.cap_height) * 0.5;
        text::draw_tracked_em(
            canvas,
            value_text,
            (right - advance, value_baseline),
            &value_font,
            &value_paint,
            0.12,
        );
    }

    label_baseline + lm.descent
}

// ────────────────────────────────────────────────────────────────────────
// Mod list — `.inst-mods` section
// ────────────────────────────────────────────────────────────────────────

/// Card-local height of a single mod row (CSS padding 12px vertical + the
/// 22px toggle dimension). Used by both renderer + hit-tester.
const MOD_ROW_HEIGHT: f32 = 46.0;

/// Vertical gap from the mod section head to the first row.
const MOD_HEAD_GAP: f32 = 14.0;
/// Toggle dimensions (CSS `.mod-toggle { width:22 height:22 }`)
const MOD_TOGGLE_SIZE: f32 = 22.0;
/// Pearl-on dimensions (CSS `.mod-pearl { width:8 height:8 }`)
const MOD_PEARL_SIZE: f32 = 8.0;
/// Per-grid-column widths from `.mod-row { grid-template-columns: 28 1fr auto auto }`.
const MOD_TOGGLE_COL_W: f32 = 28.0;
const MOD_GRID_GAP: f32 = 14.0;

/// Per-row stagger delay (CSS `animation-delay: calc(var(--row-i) * 50ms)`).
const MOD_ROW_STAGGER_S: f32 = 0.05;
/// Per-row entrance duration (CSS `mod-row-in` keyframe block).
const MOD_ROW_REVEAL_S: f32 = 0.30;

fn draw_mod_section(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    mods: &[ModInfo],
    mods_on: &[bool],
    reveal_age: Option<f32>,
) {
    // Head: "Mods" title (Fraunces 18) + count "X of Y enabled" (mono 10).
    let title_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color(TEXT_PEARL);
    let (_, tm) = title_font.metrics();
    let title_baseline = top + (-tm.ascent);
    canvas.draw_str("Mods", (left, title_baseline), &title_font, &title_paint);

    let on_count = mods_on.iter().filter(|&&v| v).count();
    let count_font = fonts.jetbrains_mono(10.0);
    let mut count_paint = Paint::default();
    count_paint.set_anti_alias(true);
    count_paint.set_color(TEXT_MAUVE);
    let count_str = format!("{} OF {} ENABLED", on_count, mods.len());
    let advance = text::measure_tracked_em(&count_font, &count_str, 0.18);
    // Right-align with the title's baseline (CSS uses `align-items: baseline`).
    text::draw_tracked_em(
        canvas,
        &count_str,
        (right - advance, title_baseline),
        &count_font,
        &count_paint,
        0.18,
    );

    // Rows — staggered entrance animation when `reveal_age` is recent
    // enough (CSS `mod-row-in`: opacity 0.4 → 1, translateY 4 → 0, with
    // `animation-delay: i * 50ms` per row).
    let mut y = title_baseline + tm.descent + MOD_HEAD_GAP;
    for (i, m) in mods.iter().enumerate() {
        let on = mods_on.get(i).copied().unwrap_or(m.on);
        // Per-row reveal interpolation. `Some(t)` with t∈[0,1) means animate;
        // `None` means past the end (render at rest) or no animation pending.
        let reveal_t: Option<f32> = reveal_age.and_then(|age| {
            let local = age - (i as f32) * MOD_ROW_STAGGER_S;
            if local < 0.0 {
                Some(0.0)
            } else if local >= MOD_ROW_REVEAL_S {
                None
            } else {
                Some(local / MOD_ROW_REVEAL_S)
            }
        });

        let row_rect = Rect::from_xywh(left, y, right - left, MOD_ROW_HEIGHT + 4.0);
        let layer_handle = if let Some(t) = reveal_t {
            let eased = ewo_core::CubicBezier::SILK.eval(t.clamp(0.0, 1.0));
            // 0.4 → 1.0 opacity, 4 → 0 translateY.
            let alpha = 0.4 + 0.6 * eased;
            let dy = (1.0 - eased) * 4.0;
            let s = canvas.save_layer_alpha_f(row_rect, alpha);
            canvas.translate((0.0, dy));
            Some(s)
        } else {
            None
        };

        if i > 0 {
            let mut div = Paint::default();
            div.set_anti_alias(true);
            div.set_style(PaintStyle::Stroke);
            div.set_stroke_width(1.0);
            div.set_color4f(
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.06),
                None,
            );
            canvas.draw_line((left, y), (right, y), &div);
        }
        draw_mod_row(canvas, fonts, left, right, y, m, on);

        if let Some(s) = layer_handle {
            canvas.restore_to_count(s);
        }

        y += MOD_ROW_HEIGHT;
    }
}

fn draw_mod_row(
    canvas: &Canvas,
    fonts: &FontStore,
    left: f32,
    right: f32,
    top: f32,
    m: &ModInfo,
    on: bool,
) {
    let cy = top + MOD_ROW_HEIGHT * 0.5;

    // Toggle circle (col 1, 28px wide, toggle is 22px centered)
    let toggle_cx = left + MOD_TOGGLE_COL_W * 0.5;
    let mut toggle_border = Paint::default();
    toggle_border.set_anti_alias(true);
    toggle_border.set_style(PaintStyle::Stroke);
    toggle_border.set_stroke_width(1.0);
    toggle_border.set_color4f(
        Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.20),
        None,
    );
    canvas.draw_circle((toggle_cx, cy), MOD_TOGGLE_SIZE * 0.5, &toggle_border);

    if on {
        // Pearl center (CSS radial gradient circle, white→rose→lavender)
        let pearl_r = MOD_PEARL_SIZE * 0.5;
        // Outer halo
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_color4f(
            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.7),
            None,
        );
        halo.set_mask_filter(skia_safe::MaskFilter::blur(
            skia_safe::BlurStyle::Normal,
            4.0,
            false,
        ));
        canvas.draw_circle((toggle_cx, cy), pearl_r, &halo);

        // Pearl gradient body
        if let Some(shader) = skia_safe::gradient_shader::radial(
            skia_safe::Point::new(toggle_cx - pearl_r * 0.3, cy - pearl_r * 0.3),
            pearl_r * 1.6,
            skia_safe::gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new(255.0 / 255.0, 246.0 / 255.0, 240.0 / 255.0, 1.0),
                    Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 1.0),
                    Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, 1.0),
                ],
                None,
            ),
            Some(&[0.0_f32, 0.70, 1.0][..]),
            skia_safe::TileMode::Clamp,
            None,
            None,
        ) {
            let mut pearl = Paint::default();
            pearl.set_anti_alias(true);
            pearl.set_shader(shader);
            canvas.draw_circle((toggle_cx, cy), pearl_r, &pearl);
        }
    }

    // Mod name (col 2, Fraunces 15)
    let name_font = fonts.fraunces_axes(15.0, 50.0, 0.0, 400.0, None);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color(TEXT_PEARL);
    if !on {
        name_paint.set_alpha_f(0.4);
    }
    let (_, nm) = name_font.metrics();
    let name_baseline = cy + nm.cap_height * 0.5;
    let name_x = left + MOD_TOGGLE_COL_W + MOD_GRID_GAP;
    canvas.draw_str(&m.name, (name_x, name_baseline), &name_font, &name_paint);

    // Version on the far right (col 4, Mono 11) — track from there leftward
    let version_font = fonts.jetbrains_mono(11.0);
    let mut version_paint = Paint::default();
    version_paint.set_anti_alias(true);
    version_paint.set_color(TEXT_MID_PEARL);
    if !on {
        version_paint.set_alpha_f(0.4);
    }
    let (v_advance, _) = version_font.measure_str(&m.version, Some(&version_paint));
    let (_, vm) = version_font.metrics();
    let version_baseline = cy + vm.cap_height * 0.5;
    canvas.draw_str(&m.version, (right - v_advance, version_baseline), &version_font, &version_paint);

    // Category in col 3 (Mono 9, tracked uppercase)
    let cat_font = fonts.jetbrains_mono(9.0);
    let mut cat_paint = Paint::default();
    cat_paint.set_anti_alias(true);
    cat_paint.set_color(TEXT_MAUVE);
    if !on {
        cat_paint.set_alpha_f(0.5);
    }
    let cat_str = m.category.to_ascii_uppercase();
    let cat_advance = text::measure_tracked_em(&cat_font, &cat_str, 0.18);
    let cat_x = right - v_advance - MOD_GRID_GAP - cat_advance;
    let (_, cm) = cat_font.metrics();
    let cat_baseline = cy + cm.cap_height * 0.5;
    text::draw_tracked_em(
        canvas,
        &cat_str,
        (cat_x, cat_baseline),
        &cat_font,
        &cat_paint,
        0.18,
    );
}

// ────────────────────────────────────────────────────────────────────────
// Hit-testing
// ────────────────────────────────────────────────────────────────────────

/// Card-local bounds of every widget on the Instances detail panel. Same
/// pattern as `screens::settings::widget_bounds` — slot + actual widget
/// hit-rect (slider track rect / dropdown head rect / mod toggle rect).
///
/// `prefs` provides the per-instance mod count + the current scroll
/// offset. Returned rects are already shifted by `-prefs.detail_scroll`,
/// so `main.rs` can hit-test them against the raw cursor position.
pub fn widget_bounds(
    card_w: f32,
    card_h: f32,
    fonts: &FontStore,
    prefs: &InstancePrefs,
    instances: &[Instance],
) -> Vec<(Slot, Rect)> {
    let panel = panel_bounds(card_w, card_h);
    let content_left = panel.left + PANEL_INNER_PAD_X;
    let content_right = panel.right - PANEL_INNER_PAD_X;
    let content_top = panel.top + PANEL_INNER_PAD_Y;
    let head_div_y = head_divider_y(content_top, fonts);
    let mods = instance_mods(instances, prefs.selected);
    let mut out = Vec::with_capacity(3 + mods.len());

    // Same vertical layout as draw_config_rows.
    let label_font = fonts.newsreader(14.0);
    let (_, lm) = label_font.metrics();
    let label_h = -lm.ascent + lm.descent;

    let mut y = head_div_y + CONFIG_TOP_GAP;
    // Row 1: RAM
    let widget_top = y + label_h + CONFIG_LABEL_TO_WIDGET_GAP;
    out.push((
        Slot::Ram,
        Rect::from_xywh(content_left, widget_top, content_right - content_left, CONFIG_SLIDER_HEIGHT),
    ));
    y = widget_top + CONFIG_SLIDER_HEIGHT + CONFIG_ROW_GAP;

    // Row 2: Render distance
    let widget_top = y + label_h + CONFIG_LABEL_TO_WIDGET_GAP;
    out.push((
        Slot::RenderDist,
        Rect::from_xywh(content_left, widget_top, content_right - content_left, CONFIG_SLIDER_HEIGHT),
    ));
    y = widget_top + CONFIG_SLIDER_HEIGHT + CONFIG_ROW_GAP;

    // Row 3: Java runtime
    let widget_top = y + label_h + CONFIG_LABEL_TO_WIDGET_GAP;
    let head_w = JAVA_RUNTIME_DROPDOWN_WIDTH.min(content_right - content_left);
    out.push((
        Slot::JavaRuntime,
        Rect::from_xywh(content_left, widget_top, head_w, CONFIG_DROPDOWN_HEIGHT),
    ));
    y = widget_top + CONFIG_DROPDOWN_HEIGHT + CONFIG_ROW_GAP;

    // Mod section — replicates `draw_mod_section` layout for hit-testing.
    // Bottom of the inst-config border + 28 margin.
    let cfg_div_y = y + 28.0 - CONFIG_ROW_GAP;
    let mods_top = cfg_div_y + 28.0;
    // Head height: title baseline + descent + MOD_HEAD_GAP.
    let title_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let (_, tm) = title_font.metrics();
    let head_bottom = mods_top + (-tm.ascent) + tm.descent + MOD_HEAD_GAP;

    for (i, _) in mods.iter().enumerate() {
        let row_top = head_bottom + (i as f32) * MOD_ROW_HEIGHT;
        let cy = row_top + MOD_ROW_HEIGHT * 0.5;
        let toggle_cx = content_left + MOD_TOGGLE_COL_W * 0.5;
        let toggle_rect = Rect::from_xywh(
            toggle_cx - MOD_TOGGLE_SIZE * 0.5,
            cy - MOD_TOGGLE_SIZE * 0.5,
            MOD_TOGGLE_SIZE,
            MOD_TOGGLE_SIZE,
        );
        out.push((Slot::ModToggle(i), toggle_rect));
    }

    // Apply scroll offset so the rects align with what the user clicks.
    if prefs.detail_scroll != 0.0 {
        for (_, r) in out.iter_mut() {
            *r = r.with_offset((0.0, -prefs.detail_scroll));
        }
    }

    out
}

pub fn dropdown_head_for_slot(
    slot: Slot,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    prefs: &InstancePrefs,
    instances: &[Instance],
) -> Option<Rect> {
    widget_bounds(card_w, card_h, fonts, prefs, instances)
        .into_iter()
        .find_map(|(s, r)| if s == slot { Some(r) } else { None })
}

/// Card-local rect of the ✎ rename icon next to the selected instance's
/// name in the detail head. Returns `None` when the list is empty.
/// Result is shifted by `-prefs.detail_scroll` so `main.rs` can hit-test
/// it against the raw cursor position.
pub fn rename_button_bounds(
    card_w: f32,
    card_h: f32,
    fonts: &FontStore,
    instances: &[Instance],
    prefs: &InstancePrefs,
) -> Option<Rect> {
    let inst = instances.get(prefs.selected)?;
    let panel = panel_bounds(card_w, card_h);
    let content_left = panel.left + PANEL_INNER_PAD_X;
    let content_top = panel.top + PANEL_INNER_PAD_Y;
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let name_font = fonts.fraunces_axes(36.0, 50.0, 1.0, 300.0, None);
    let (_, em) = eyebrow_font.metrics();
    let (_, nm) = name_font.metrics();
    let eyebrow_baseline = content_top + (-em.ascent);
    let name_top = eyebrow_baseline + em.descent + 8.0;
    let name_baseline = name_top + (-nm.ascent);
    let name_paint = Paint::default();
    let (name_advance, _) = name_font.measure_str(&inst.name, Some(&name_paint));
    let pencil_x = content_left + name_advance + 12.0;
    // Generous hit-rect: 24×28 centered on the glyph baseline.
    let r = Rect::from_xywh(pencil_x - 4.0, name_baseline - 22.0, 28.0, 28.0);
    Some(r.with_offset((0.0, -prefs.detail_scroll)))
}

/// Card-local rects for each instance row in the left list. Returned in
/// order — index matches `INSTANCES`. Used by `main.rs` to route
/// click-to-select.
///
/// Rows occupy 70px of vertical space but the *clickable* hit-rect is
/// trimmed to leave a small gap above/below — so the cursor flips to the
/// default arrow in the spaces between rows and back to the pointer once
/// it's actually over an instance card.
/// Card-local hit-rect for the × button on a single display-order row.
/// Sized as a 22×22 square at the right edge so users can comfortably
/// click without pixel-perfect aim.
pub fn delete_button_bounds(row: Rect) -> Rect {
    let cx = LIST_WIDTH - 22.0;
    let cy = (row.top + row.bottom) * 0.5;
    Rect::from_xywh(cx - 11.0, cy - 11.0, 22.0, 22.0)
}

/// Card-local hit-rects for each visible instance row in *display* order
/// (not underlying). Caller maps display index → underlying via
/// `display_order`. Rects are already shifted by `-prefs.list_scroll` so
/// `main.rs` can hit-test them against the raw cursor position. Rows
/// that scroll outside the visible band get bounds outside the card,
/// which `rect_contains` will simply miss.
pub fn list_row_bounds(
    card_h: f32,
    fonts: &FontStore,
    instances: &[Instance],
    prefs: &InstancePrefs,
) -> Vec<Rect> {
    let list_top = HEADER_BOTTOM + 28.0;
    let row_h: f32 = 70.0;
    let row_gap: f32 = 6.0;
    let head_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let (_, hm) = head_font.metrics();
    let head_baseline = list_top + (-hm.ascent);
    let sort_baseline = head_baseline + hm.descent + 22.0;
    let head_div_y = sort_baseline + 8.0;
    let mut y = head_div_y + 12.0 - prefs.list_scroll;
    let _ = card_h;
    (0..instances.len())
        .map(|_| {
            let r = Rect::from_xywh(0.0, y + row_gap * 0.5, LIST_WIDTH, row_h - row_gap);
            y += row_h;
            r
        })
        .collect()
}

/// Panel rect for the detail column — used by `main.rs` to scope the
/// scroll-wheel handler to the detail panel only.
pub fn detail_panel_bounds(card_w: f32, card_h: f32) -> Rect {
    panel_bounds(card_w, card_h)
}

/// Total scrollable content height for the detail panel given the
/// current selection. Mirrors the `draw_detail` layout exactly so
/// `max_scroll = max(0, content_h - inner_h)` clamps correctly.
pub fn detail_content_height(
    card_w: f32,
    card_h: f32,
    fonts: &FontStore,
    prefs: &InstancePrefs,
    instances: &[Instance],
) -> f32 {
    let panel = panel_bounds(card_w, card_h);
    let content_top = panel.top + PANEL_INNER_PAD_Y;
    let head_div_y = head_divider_y(content_top, fonts);
    let cfg_top = head_div_y + CONFIG_TOP_GAP;

    // Reproduce config row vertical advance — three rows: slider, slider, dropdown.
    let label_font = fonts.newsreader(14.0);
    let (_, lm) = label_font.metrics();
    let label_h = -lm.ascent + lm.descent;

    let mut y = cfg_top;
    // RAM
    let widget_top = y + label_h + CONFIG_LABEL_TO_WIDGET_GAP;
    y = widget_top + CONFIG_SLIDER_HEIGHT + CONFIG_ROW_GAP;
    // Render distance
    let widget_top = y + label_h + CONFIG_LABEL_TO_WIDGET_GAP;
    y = widget_top + CONFIG_SLIDER_HEIGHT + CONFIG_ROW_GAP;
    // Java runtime
    let widget_top = y + label_h + CONFIG_LABEL_TO_WIDGET_GAP;
    y = widget_top + CONFIG_DROPDOWN_HEIGHT + CONFIG_ROW_GAP;
    // Mods section starts after the inst-config bottom hairline + 28px margin.
    let cfg_div_y = y + 28.0 - CONFIG_ROW_GAP;
    let mut y = cfg_div_y + 28.0;

    let mods = instance_mods(instances, prefs.selected);
    if !mods.is_empty() {
        // Head: title baseline + descent + MOD_HEAD_GAP
        let title_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
        let (_, tm) = title_font.metrics();
        let head_bottom_y = y + (-tm.ascent) + tm.descent + MOD_HEAD_GAP;
        y = head_bottom_y + (mods.len() as f32) * MOD_ROW_HEIGHT;
    }

    // Bottom padding so the last row has breathing room.
    y += PANEL_INNER_PAD_Y;
    (y - panel.top).max(0.0)
}

/// Maximum allowed scroll offset for the detail panel given the current
/// content height. Always `>= 0`.
pub fn detail_max_scroll(
    card_w: f32,
    card_h: f32,
    fonts: &FontStore,
    prefs: &InstancePrefs,
    instances: &[Instance],
) -> f32 {
    let panel = panel_bounds(card_w, card_h);
    let inner_h = panel.height();
    let content_h = detail_content_height(card_w, card_h, fonts, prefs, instances);
    (content_h - inner_h).max(0.0)
}

/// Card-local bounds of the sort-mode label below the Worlds head.
/// Used by `main.rs` to route hover + click-to-cycle.
pub fn sort_button_bounds(fonts: &FontStore) -> Rect {
    let list_top = HEADER_BOTTOM + 28.0;
    let head_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let (_, hm) = head_font.metrics();
    let head_baseline = list_top + (-hm.ascent);
    let baseline = head_baseline + hm.descent + 22.0;
    let sort_font = fonts.jetbrains_mono(10.0);
    let (_, sm) = sort_font.metrics();
    let top = baseline + sm.ascent - 4.0;
    let bottom = baseline - sm.descent + 8.0;
    Rect::from_ltrb(LIST_LEFT_PAD - 4.0, top, LIST_WIDTH - 24.0, bottom)
}

/// Render the small clickable sort label.
fn draw_sort_label(
    canvas: &Canvas,
    fonts: &FontStore,
    baseline: f32,
    mode: SortMode,
    hover: bool,
) {
    let label_font = fonts.jetbrains_mono(10.0);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(if hover {
        Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5)
    } else {
        Color::from_argb(0xFF, 0x9A, 0x80, 0x87)
    });
    let prefix = "SORT · ";
    text::draw_tracked_em(
        canvas,
        prefix,
        (LIST_LEFT_PAD, baseline),
        &label_font,
        &paint,
        0.18,
    );
    let prefix_w = text::measure_tracked_em(&label_font, prefix, 0.18);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    value_paint.set_color(if hover {
        Color::from_argb(0xFF, 0xFF, 0xF6, 0xF0)
    } else {
        Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5)
    });
    let label = mode.label();
    canvas.draw_str(label, (LIST_LEFT_PAD + prefix_w, baseline), &label_font, &value_paint);
}

/// Maximum allowed scroll offset for the Worlds list. `>= 0`.
pub fn list_max_scroll(card_h: f32, fonts: &FontStore, instances: &[Instance]) -> f32 {
    let row_h: f32 = 70.0;
    let list_top = HEADER_BOTTOM + 28.0;
    let head_font = fonts.fraunces_axes(18.0, 50.0, 0.0, 300.0, None);
    let (_, hm) = head_font.metrics();
    let head_baseline = list_top + (-hm.ascent);
    let sort_baseline = head_baseline + hm.descent + 22.0;
    let head_div_y = sort_baseline + 8.0;
    let rows_top = head_div_y + 12.0;
    let rows_bottom = card_h - 28.0;
    let visible_h = (rows_bottom - rows_top).max(0.0);
    let total_h = (instances.len() as f32) * row_h;
    (total_h - visible_h).max(0.0)
}

/// Card-local bounds of the "+" button in the Worlds list head. Used by
/// `main.rs` to open the new-instance modal on click.
pub fn add_button_bounds() -> Rect {
    // CSS `.inst-add` is a 28×28 square right-aligned in the list head.
    // We position it at `LIST_WIDTH - 24 - 28` (matches the head divider's
    // right edge minus the button width).
    let list_top = HEADER_BOTTOM + 28.0;
    let size = 28.0;
    let cx_y = list_top - 5.0; // visually align with the "Worlds" title midline
    Rect::from_xywh(LIST_WIDTH - 24.0 - size, cx_y, size, size)
}
