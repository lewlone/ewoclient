//! In-game HUD widgets — painted by `ewo-jni` onto the offscreen HUD surface.
//!
//! Phase E. Each widget re-skins a `hud.jsx` prototype element to the Velvet
//! theme (constraint #3) and draws purely with `ewo-render`'s Skia stack — no
//! Minecraft-vanilla UI shape (constraint #2).
//!
//! Widget data arrives through [`HudData`], a read-only view over the shared
//! JVM→Rust buffer (`EwoHudData` on the Java side). E3 shipped the full
//! read-only widget set: FPS, Coords, Ping, Keystrokes, Armor, Potions and
//! TargetHUD.
//!
//! E5 adds the HUD editor: widget placement is data-driven from a persisted
//! [`HudLayout`] (`hud.toml`), and while the overlay is open ([`Editor`]) each
//! widget can be dragged to reposition it.

use std::path::PathBuf;

use ewo_render::text::{draw_tracked_em, measure_tracked_em};
use ewo_render::FontStore;
use skia_safe::{
    gradient_shader, BlurStyle, Canvas, ClipOp, Color4f, MaskFilter, Paint, PaintStyle, Point,
    RRect, Rect, TileMode,
};

// ── Velvet theme tokens (see CLAUDE.md "Velvet theme tokens") ──────────────
const PEARL: (u8, u8, u8) = (0xF4, 0xE8, 0xEA); // --text-pearl
const MAUVE: (u8, u8, u8) = (0x9A, 0x80, 0x87); // --text-mauve
const ROSE: (u8, u8, u8) = (0xE5, 0xB8, 0xC5); // --accent-rose
const LAV: (u8, u8, u8) = (0xC9, 0xA5, 0xD4); // --accent-lav
const WINE: (u8, u8, u8) = (0x12, 0x00, 0x10); // --bg-wine-b

fn rgba(c: (u8, u8, u8), a: f32) -> Color4f {
    Color4f::new(c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0, a)
}

/// A zero-size rect — used as the recorded bounds of a widget that wasn't drawn.
fn empty_rect() -> Rect {
    Rect::from_xywh(0.0, 0.0, 0.0, 0.0)
}

// ────────────────────────────────────────────────────────────────────────
// Shared data block — read-only view over the JVM→Rust buffer.
// ────────────────────────────────────────────────────────────────────────

/// Layout version. Bumped whenever the buffer layout below changes; the Java
/// side (`EwoHudData.SCHEMA_VERSION`) must match or the HUD draws no data.
pub const SCHEMA_VERSION: i32 = 2;

/// Byte offsets into the shared block — mirror of `EwoHudData.java`.
mod off {
    pub const FLAGS: usize = 4;
    pub const FPS: usize = 8;
    pub const PING: usize = 12;
    pub const KEYS: usize = 16;
    pub const X: usize = 24;
    pub const Y: usize = 32;
    pub const Z: usize = 40;
    pub const ARMOR: usize = 48; // 4 × { i32 present, f32 durability }
    pub const POTION_COUNT: usize = 80;
    pub const POTIONS: usize = 84; // MAX_POTIONS × POTION_REC
    pub const TARGET_PRESENT: usize = 436;
    pub const TARGET_DIST: usize = 440;
    pub const TARGET_HP: usize = 444;
    pub const TARGET_MAXHP: usize = 448;
    pub const TARGET_NAME: usize = 452;
}
const FLAG_WORLD: i32 = 1; // a player + level exist → coords/keystrokes valid
const FLAG_PING: i32 = 1 << 1; // a server connection exists → ping valid
const FLAG_ARMOR: i32 = 1 << 2; // at least one armor piece is worn
const FLAG_TARGET: i32 = 1 << 3; // an entity is under the crosshair
const FLAG_OVERLAY: i32 = 1 << 4; // the EwoClient overlay is open

const MAX_POTIONS: usize = 8;
const POTION_REC: usize = 44; // bytes per potion record
const POTION_NAME_CAP: usize = 28;
const TARGET_NAME_CAP: usize = 44;

/// One active potion effect, decoded from the shared block.
pub struct Potion {
    /// Remaining ticks; negative means an infinite effect.
    pub duration: i32,
    /// 0-based amplifier (0 = level I).
    pub amplifier: i32,
    /// Packed `0xRRGGBB` effect color.
    pub color: i32,
    pub name: String,
}

/// Read-only view over the JVM→Rust HUD data block. The backing memory is a
/// direct `ByteBuffer` the mod holds for the process lifetime; all reads are
/// unaligned so the view is robust regardless of field packing.
pub struct HudData {
    base: *const u8,
}

impl HudData {
    /// # Safety
    /// `base` must point to at least `EwoHudData.CAPACITY` readable bytes,
    /// valid for the lifetime of this view.
    pub unsafe fn new(base: *const u8) -> Self {
        HudData { base }
    }

    fn i32_at(&self, offset: usize) -> i32 {
        unsafe { (self.base.add(offset) as *const i32).read_unaligned() }
    }
    fn f32_at(&self, offset: usize) -> f32 {
        unsafe { (self.base.add(offset) as *const f32).read_unaligned() }
    }
    fn f64_at(&self, offset: usize) -> f64 {
        unsafe { (self.base.add(offset) as *const f64).read_unaligned() }
    }
    /// Decode a length-prefixed UTF-8 string written by `EwoHudData.putString`.
    fn str_at(&self, offset: usize, cap: usize) -> String {
        let len = (self.i32_at(offset).max(0) as usize).min(cap);
        let bytes = unsafe { std::slice::from_raw_parts(self.base.add(offset + 4), len) };
        String::from_utf8_lossy(bytes).into_owned()
    }
    fn flag(&self, bit: i32) -> bool {
        self.i32_at(off::FLAGS) & bit != 0
    }

    pub fn schema_version(&self) -> i32 {
        self.i32_at(0)
    }
    pub fn fps(&self) -> i32 {
        self.i32_at(off::FPS)
    }
    pub fn ping(&self) -> i32 {
        self.i32_at(off::PING)
    }
    pub fn keys(&self) -> i32 {
        self.i32_at(off::KEYS)
    }
    pub fn player_x(&self) -> f64 {
        self.f64_at(off::X)
    }
    pub fn player_y(&self) -> f64 {
        self.f64_at(off::Y)
    }
    pub fn player_z(&self) -> f64 {
        self.f64_at(off::Z)
    }
    /// A player and level exist — coords and keystrokes are meaningful.
    pub fn world_active(&self) -> bool {
        self.flag(FLAG_WORLD)
    }
    /// A server connection exists — the ping reading is meaningful.
    pub fn ping_valid(&self) -> bool {
        self.flag(FLAG_PING)
    }
    /// At least one armor piece is worn.
    pub fn armor_active(&self) -> bool {
        self.flag(FLAG_ARMOR)
    }
    /// `true` if armor slot `i` (0=head … 3=feet) holds an item.
    pub fn armor_present(&self, i: usize) -> bool {
        self.i32_at(off::ARMOR + i * 8) != 0
    }
    /// Durability fraction (0..1) of armor slot `i`.
    pub fn armor_durability(&self, i: usize) -> f32 {
        self.f32_at(off::ARMOR + i * 8 + 4)
    }
    pub fn potion_count(&self) -> usize {
        (self.i32_at(off::POTION_COUNT).max(0) as usize).min(MAX_POTIONS)
    }
    pub fn potion(&self, i: usize) -> Potion {
        let rec = off::POTIONS + i * POTION_REC;
        Potion {
            duration: self.i32_at(rec),
            amplifier: self.i32_at(rec + 4),
            color: self.i32_at(rec + 8),
            name: self.str_at(rec + 12, POTION_NAME_CAP),
        }
    }
    /// An entity is under the crosshair.
    pub fn target_active(&self) -> bool {
        self.flag(FLAG_TARGET) && self.i32_at(off::TARGET_PRESENT) != 0
    }
    pub fn target_distance(&self) -> f32 {
        self.f32_at(off::TARGET_DIST)
    }
    pub fn target_health(&self) -> f32 {
        self.f32_at(off::TARGET_HP)
    }
    pub fn target_max_health(&self) -> f32 {
        self.f32_at(off::TARGET_MAXHP)
    }
    pub fn target_name(&self) -> String {
        self.str_at(off::TARGET_NAME, TARGET_NAME_CAP)
    }
    /// The EwoClient overlay is open — input is being captured.
    pub fn overlay_open(&self) -> bool {
        self.flag(FLAG_OVERLAY)
    }
}

// ────────────────────────────────────────────────────────────────────────
// Anchoring — `hud.jsx`'s 9-point model.
// ────────────────────────────────────────────────────────────────────────

/// Which of a widget's nine reference points is pinned to its anchor coord.
/// Mirrors `hud.jsx`'s anchor model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Tl,
    Tc,
    Tr,
    Ml,
    Mc,
    Mr,
    Bl,
    Bc,
    Br,
}

impl Anchor {
    /// The widget-relative fraction (0 / 0.5 / 1 per axis) this anchor pins.
    fn fractions(self) -> (f32, f32) {
        match self {
            Anchor::Tl => (0.0, 0.0),
            Anchor::Tc => (0.5, 0.0),
            Anchor::Tr => (1.0, 0.0),
            Anchor::Ml => (0.0, 0.5),
            Anchor::Mc => (0.5, 0.5),
            Anchor::Mr => (1.0, 0.5),
            Anchor::Bl => (0.0, 1.0),
            Anchor::Bc => (0.5, 1.0),
            Anchor::Br => (1.0, 1.0),
        }
    }

    /// Top-left draw origin for a `w`×`h` widget whose anchor point is `(ax, ay)`.
    fn origin(self, ax: f32, ay: f32, w: f32, h: f32) -> (f32, f32) {
        let (fx, fy) = self.fractions();
        (ax - w * fx, ay - h * fy)
    }

    /// Stable token for `hud.toml`.
    fn as_str(self) -> &'static str {
        match self {
            Anchor::Tl => "tl",
            Anchor::Tc => "tc",
            Anchor::Tr => "tr",
            Anchor::Ml => "ml",
            Anchor::Mc => "mc",
            Anchor::Mr => "mr",
            Anchor::Bl => "bl",
            Anchor::Bc => "bc",
            Anchor::Br => "br",
        }
    }

    fn from_str(s: &str) -> Option<Anchor> {
        Some(match s {
            "tl" => Anchor::Tl,
            "tc" => Anchor::Tc,
            "tr" => Anchor::Tr,
            "ml" => Anchor::Ml,
            "mc" => Anchor::Mc,
            "mr" => Anchor::Mr,
            "bl" => Anchor::Bl,
            "bc" => Anchor::Bc,
            "br" => Anchor::Br,
            _ => return None,
        })
    }
}

// ────────────────────────────────────────────────────────────────────────
// Widget identity + the persisted layout.
// ────────────────────────────────────────────────────────────────────────

/// Every HUD widget, in draw order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WidgetId {
    Fps,
    Coords,
    Ping,
    Keystrokes,
    Armor,
    Potions,
    Target,
}

impl WidgetId {
    const ALL: [WidgetId; 7] = [
        WidgetId::Fps,
        WidgetId::Coords,
        WidgetId::Ping,
        WidgetId::Keystrokes,
        WidgetId::Armor,
        WidgetId::Potions,
        WidgetId::Target,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// Stable key for `hud.toml` sections.
    fn key(self) -> &'static str {
        match self {
            WidgetId::Fps => "fps",
            WidgetId::Coords => "coords",
            WidgetId::Ping => "ping",
            WidgetId::Keystrokes => "keystrokes",
            WidgetId::Armor => "armor",
            WidgetId::Potions => "potions",
            WidgetId::Target => "target",
        }
    }

    /// Display name for the editor's drag-outline label.
    fn title(self) -> &'static str {
        match self {
            WidgetId::Fps => "FPS",
            WidgetId::Coords => "COORDS",
            WidgetId::Ping => "PING",
            WidgetId::Keystrokes => "KEYSTROKES",
            WidgetId::Armor => "ARMOR",
            WidgetId::Potions => "POTIONS",
            WidgetId::Target => "TARGET",
        }
    }
}

/// One widget's placement: an anchor and a fractional (0..1) anchor point.
#[derive(Clone, Copy)]
struct WidgetLayout {
    enabled: bool,
    anchor: Anchor,
    x: f32,
    y: f32,
}

/// The whole HUD layout — one entry per [`WidgetId`], persisted to `hud.toml`.
struct HudLayout {
    widgets: [WidgetLayout; 7],
}

impl HudLayout {
    fn get(&self, id: WidgetId) -> WidgetLayout {
        self.widgets[id.index()]
    }
    fn get_mut(&mut self, id: WidgetId) -> &mut WidgetLayout {
        &mut self.widgets[id.index()]
    }

    /// The default layout — the positions E3 shipped, expressed as fractions.
    fn defaults() -> Self {
        HudLayout {
            widgets: [
                WidgetLayout { enabled: true, anchor: Anchor::Tl, x: 0.0135, y: 0.0204 }, // fps
                WidgetLayout { enabled: true, anchor: Anchor::Tl, x: 0.0135, y: 0.0620 }, // coords
                WidgetLayout { enabled: true, anchor: Anchor::Br, x: 0.9865, y: 0.9759 }, // ping
                WidgetLayout { enabled: true, anchor: Anchor::Bl, x: 0.0135, y: 0.9759 }, // keystrokes
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.9330 }, // armor
                WidgetLayout { enabled: true, anchor: Anchor::Tr, x: 0.9865, y: 0.3000 }, // potions
                WidgetLayout { enabled: true, anchor: Anchor::Tc, x: 0.5000, y: 0.0593 }, // target
            ],
        }
    }

    /// Load `hud.toml`, falling back to the default for anything missing or
    /// malformed — so a hand-edited or absent file never breaks the HUD.
    fn load() -> Self {
        let mut layout = Self::defaults();
        let Some(path) = hud_toml_path() else {
            return layout;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return layout;
        };
        let mut current: Option<WidgetId> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                current = WidgetId::ALL.into_iter().find(|id| id.key() == section);
                continue;
            }
            let Some(id) = current else {
                continue;
            };
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            let wl = layout.get_mut(id);
            match key.trim() {
                "enabled" => wl.enabled = value == "true",
                "anchor" => {
                    if let Some(a) = Anchor::from_str(value) {
                        wl.anchor = a;
                    }
                }
                "x" => {
                    if let Ok(v) = value.parse() {
                        wl.x = v;
                    }
                }
                "y" => {
                    if let Ok(v) = value.parse() {
                        wl.y = v;
                    }
                }
                _ => {}
            }
        }
        layout
    }

    /// Write the layout to `hud.toml`.
    fn save(&self) {
        let Some(path) = hud_toml_path() else {
            return;
        };
        let mut s = String::from("# EwoClient HUD layout — written by the in-game editor.\n");
        for id in WidgetId::ALL {
            let wl = self.get(id);
            s.push_str(&format!(
                "\n[{}]\nenabled = {}\nanchor = \"{}\"\nx = {:.5}\ny = {:.5}\n",
                id.key(),
                wl.enabled,
                wl.anchor.as_str(),
                wl.x,
                wl.y,
            ));
        }
        let _ = std::fs::write(&path, s);
    }
}

/// `<config>/EwoClient/hud.toml`. Resolved from `%APPDATA%` — the cdylib runs
/// inside the Minecraft JVM, which inherits the launcher's environment.
fn hud_toml_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join("EwoClient").join("hud.toml"))
}

// ────────────────────────────────────────────────────────────────────────
// Editor state.
// ────────────────────────────────────────────────────────────────────────

/// An active widget drag, with the grab offset from the widget's anchor point.
struct Drag {
    id: WidgetId,
    grab_dx: f32,
    grab_dy: f32,
}

/// In-game HUD-editor state. Owns the [`HudLayout`], tracks the cursor and any
/// active drag, and records each widget's drawn bounds for hit-testing. Fed by
/// the `nativeMouse*` JNI exports while the overlay is open.
pub struct Editor {
    layout: HudLayout,
    /// Window size from the last paint — for pixel ↔ fraction conversion.
    window: (f32, f32),
    /// Cursor position in window pixels.
    cursor: (f32, f32),
    /// Each widget's drawn bounds, recorded each paint (indexed by `WidgetId`).
    bounds: [Rect; 7],
    dragging: Option<Drag>,
    /// Active snap guide lines (window pixels) while a drag is alignment-snapped.
    snap_x: Option<f32>,
    snap_y: Option<f32>,
    /// The widget selected in the side panel — drives the anchor grid.
    selected: Option<WidgetId>,
}

/// How close (window px) an edge must come to another widget's edge before the
/// drag snaps to align. Small on purpose — a gentle assist, easy to drag past.
const SNAP_PX: f32 = 6.0;

impl Editor {
    /// Build the editor, loading the persisted layout from `hud.toml`.
    pub fn new() -> Self {
        Editor {
            layout: HudLayout::load(),
            window: (1.0, 1.0),
            cursor: (0.0, 0.0),
            bounds: [empty_rect(); 7],
            dragging: None,
            snap_x: None,
            snap_y: None,
            selected: None,
        }
    }

    /// Cursor moved — drag the active widget if one is held, snapping its
    /// edges/centres to other widgets' for a gentle alignment assist.
    pub fn on_mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        let Some(drag) = &self.dragging else {
            return;
        };
        let drag_id = drag.id;
        let grab_dx = drag.grab_dx;
        let grab_dy = drag.grab_dy;
        let (w, h) = self.window;
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        let wl = self.layout.get(drag_id);
        let dragged = self.bounds[drag_id.index()];
        let (ww, wh) = (dragged.width(), dragged.height());

        // Proposed anchor point (window pixels), before snapping.
        let mut ap_x = x - grab_dx;
        let mut ap_y = y - grab_dy;
        self.snap_x = None;
        self.snap_y = None;

        // Gather every other visible widget's edge + centre lines and snap the
        // dragged widget's own edges/centre onto the nearest within SNAP_PX.
        if ww > 0.0 && wh > 0.0 {
            let mut fixed_x: Vec<f32> = Vec::new();
            let mut fixed_y: Vec<f32> = Vec::new();
            for id in WidgetId::ALL {
                if id == drag_id {
                    continue;
                }
                let b = self.bounds[id.index()];
                if b.width() <= 0.0 {
                    continue;
                }
                fixed_x.push(b.left);
                fixed_x.push((b.left + b.right) * 0.5);
                fixed_x.push(b.right);
                fixed_y.push(b.top);
                fixed_y.push((b.top + b.bottom) * 0.5);
                fixed_y.push(b.bottom);
            }
            let (tlx, tly) = wl.anchor.origin(ap_x, ap_y, ww, wh);
            if let Some((dx, line)) = nearest_snap([tlx, tlx + ww * 0.5, tlx + ww], &fixed_x) {
                ap_x += dx;
                self.snap_x = Some(line);
            }
            if let Some((dy, line)) = nearest_snap([tly, tly + wh * 0.5, tly + wh], &fixed_y) {
                ap_y += dy;
                self.snap_y = Some(line);
            }
        }

        let wl = self.layout.get_mut(drag_id);
        wl.x = (ap_x / w).clamp(0.0, 1.0);
        wl.y = (ap_y / h).clamp(0.0, 1.0);
    }

    /// Mouse button — routes a press to the side panel (toggles, widget rows,
    /// anchor grid) or to widget dragging; ends + persists a drag on release.
    pub fn on_mouse_button(&mut self, pressed: bool, x: f32, y: f32) {
        self.cursor = (x, y);
        if !pressed {
            if self.dragging.take().is_some() {
                // A drag finished — drop the snap guides and persist.
                self.snap_x = None;
                self.snap_y = None;
                self.layout.save();
            }
            return;
        }

        let panel = panel_layout(self.window.1);

        // A widget's enable toggle.
        for (i, &toggle) in panel.toggles.iter().enumerate() {
            if point_in(toggle, x, y) {
                let wl = self.layout.get_mut(WidgetId::ALL[i]);
                wl.enabled = !wl.enabled;
                self.layout.save();
                return;
            }
        }
        // A widget row — select it (so the anchor grid targets it).
        for (i, &row) in panel.rows.iter().enumerate() {
            if point_in(row, x, y) {
                self.selected = Some(WidgetId::ALL[i]);
                return;
            }
        }
        // An anchor preset cell — jump the selected widget to that corner.
        if let Some(sel) = self.selected {
            for (i, &cell) in panel.cells.iter().enumerate() {
                if point_in(cell, x, y) {
                    let (anchor, px, py) = ANCHOR_PRESETS[i];
                    let wl = self.layout.get_mut(sel);
                    wl.anchor = anchor;
                    wl.x = px;
                    wl.y = py;
                    self.layout.save();
                    return;
                }
            }
        }
        // A press anywhere else inside the panel is swallowed — no drag.
        if point_in(panel.panel, x, y) {
            return;
        }

        // Outside the panel — select + start dragging the widget under the cursor.
        let (w, h) = self.window;
        for id in WidgetId::ALL {
            let b = self.bounds[id.index()];
            if b.width() > 0.0 && point_in(b, x, y) {
                let wl = self.layout.get(id);
                self.selected = Some(id);
                self.dragging = Some(Drag {
                    id,
                    grab_dx: x - wl.x * w,
                    grab_dy: y - wl.y * h,
                });
                break;
            }
        }
    }

    /// The widget the cursor is over (the one being dragged always wins).
    fn active_widget(&self) -> Option<WidgetId> {
        if let Some(drag) = &self.dragging {
            return Some(drag.id);
        }
        WidgetId::ALL.into_iter().find(|id| {
            let b = self.bounds[id.index()];
            b.width() > 0.0 && point_in(b, self.cursor.0, self.cursor.1)
        })
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// `(x, y)` lies inside `r`.
fn point_in(r: Rect, x: f32, y: f32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

/// Find the smallest shift that aligns one of the `moving` lines onto one of
/// the `fixed` lines, within [`SNAP_PX`]. Returns `(shift, snapped-onto line)`.
fn nearest_snap(moving: [f32; 3], fixed: &[f32]) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    let mut best_dist = SNAP_PX;
    for &m in &moving {
        for &f in fixed {
            let shift = f - m;
            if shift.abs() < best_dist {
                best_dist = shift.abs();
                best = Some((shift, f));
            }
        }
    }
    best
}

// ────────────────────────────────────────────────────────────────────────
// Full-HUD dispatch.
// ────────────────────────────────────────────────────────────────────────

/// Draw the whole HUD for one frame: place every widget from the persisted
/// layout, record its bounds, and — while the overlay is open — draw the
/// editor chrome on top.
pub fn draw(canvas: &Canvas, data: &HudData, editor: &mut Editor, fonts: &FontStore, w: f32, h: f32) {
    editor.window = (w, h);

    for id in WidgetId::ALL {
        let wl = editor.layout.get(id);
        let bounds = if wl.enabled && widget_available(id, data) {
            draw_widget(canvas, id, data, fonts, wl.anchor, wl.x * w, wl.y * h)
        } else {
            empty_rect()
        };
        editor.bounds[id.index()] = bounds;
    }

    if data.overlay_open() {
        draw_editor(canvas, editor, fonts, w, h);
    }
}

/// Whether `id`'s underlying data is present this frame.
fn widget_available(id: WidgetId, data: &HudData) -> bool {
    match id {
        WidgetId::Fps => true,
        WidgetId::Ping => data.ping_valid(),
        WidgetId::Coords | WidgetId::Keystrokes => data.world_active(),
        WidgetId::Armor => data.world_active() && data.armor_active(),
        WidgetId::Potions => data.world_active() && data.potion_count() > 0,
        WidgetId::Target => data.world_active() && data.target_active(),
    }
}

/// Draw one widget at `(ax, ay)` with `anchor`; returns its drawn bounds.
fn draw_widget(
    canvas: &Canvas,
    id: WidgetId,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    match id {
        WidgetId::Fps => draw_stat(canvas, &data.fps().to_string(), "FPS", fonts, anchor, ax, ay),
        WidgetId::Coords => draw_coords(
            canvas,
            data.player_x(),
            data.player_y(),
            data.player_z(),
            fonts,
            anchor,
            ax,
            ay,
        ),
        WidgetId::Ping => draw_stat(canvas, &data.ping().to_string(), "MS", fonts, anchor, ax, ay),
        WidgetId::Keystrokes => draw_keystrokes(canvas, data.keys(), fonts, anchor, ax, ay),
        WidgetId::Armor => draw_armor(canvas, data, fonts, anchor, ax, ay),
        WidgetId::Potions => draw_potions(canvas, data, fonts, anchor, ax, ay),
        WidgetId::Target => draw_target(canvas, data, fonts, anchor, ax, ay),
    }
}

// ────────────────────────────────────────────────────────────────────────
// Shared helpers.
// ────────────────────────────────────────────────────────────────────────

/// The shared HUD chip — a Velvet re-skin of the prototype's `.hud-stat`
/// background (a translucent rounded panel with a hairline).
///
/// CSS `backdrop-filter: blur(8px)` can't sample the live game — the HUD
/// paints to an offscreen surface (the E1 tradeoff) — so this is a flat wine
/// fill, not a true backdrop blur.
fn draw_chip(canvas: &Canvas, rect: Rect, radius: f32) {
    let rrect = RRect::new_rect_xy(rect, radius, radius);
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.62), None);
    canvas.draw_rrect(rrect, &fill);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.12), None);
    canvas.draw_rrect(rrect, &border);
}

/// Roman numeral for `n` (1-based level). Empty string for `n <= 0`.
fn roman(n: i32) -> String {
    const TABLE: &[(i32, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    if n <= 0 {
        return String::new();
    }
    let mut n = n;
    let mut s = String::new();
    for &(value, sym) in TABLE {
        while n >= value {
            s.push_str(sym);
            n -= value;
        }
    }
    s
}

/// Format a tick duration as `m:ss`. Negative durations are infinite effects.
fn format_duration(ticks: i32) -> String {
    if ticks < 0 {
        return "∞".to_string();
    }
    let secs = ticks / 20;
    format!("{}:{:02}", secs / 60, secs % 60)
}

// ────────────────────────────────────────────────────────────────────────
// Widgets. Each takes an anchor + anchor point, draws, and returns its bounds.
// ────────────────────────────────────────────────────────────────────────

/// A stat chip — re-skin of `hud.jsx`'s `.hud-stat` (FPS / Ping): a Fraunces
/// number and a tracked JetBrains Mono unit on a wine chip.
fn draw_stat(
    canvas: &Canvas,
    value: &str,
    unit: &str,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let num_font = fonts.fraunces_axes(30.0, 34.0, 0.0, 600.0, None);
    let unit_font = fonts.jetbrains_mono(14.0);
    let unit_tracking_em = 0.18; // tracked eyebrow — Velvet label idiom

    let pad_x = 14.0;
    let pad_y = 8.0;
    let gap = 8.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (num_w, _) = num_font.measure_str(value, Some(&probe));
    let unit_w = measure_tracked_em(&unit_font, unit, unit_tracking_em);

    // Size the chip to the number's cap height — digits and the uppercase
    // unit have no descenders, so capped sizing hugs the glyphs.
    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 30.0 * 0.72 };
    let chip_w = pad_x * 2.0 + num_w + gap + unit_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_chip(canvas, chip, radius);

    // Number cap-top at `y + pad_y`; the unit shares the baseline (CSS
    // `align-items: baseline`).
    let baseline_y = y + pad_y + cap;
    let num_x = x + pad_x;
    let unit_x = num_x + num_w + gap;

    // Number — Fraunces, with a soft drop shadow for legibility over any
    // game background (CSS `text-shadow: 0 2px 6px rgba(0,0,0,0.6)`).
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.6), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    canvas.draw_str(value, (num_x, baseline_y + 2.0), &num_font, &shadow);

    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(value, (num_x, baseline_y), &num_font, &num_paint);

    // Unit — tracked JetBrains Mono eyebrow.
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        unit,
        (unit_x, baseline_y),
        &unit_font,
        &unit_paint,
        unit_tracking_em,
    );

    chip
}

/// Coords chip — re-skin of `hud.jsx`'s `coords` element: a tracked rose "XYZ"
/// label and the player position in pearl JetBrains Mono.
fn draw_coords(
    canvas: &Canvas,
    x: f64,
    y: f64,
    z: f64,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let label = "XYZ";
    // x/z to one decimal, y rounded — matches the prototype's `-128.4 64 -1492.0`.
    let value = format!("{:.1}  {}  {:.1}", x, y.round() as i64, z);

    let label_font = fonts.jetbrains_mono(12.0);
    let value_font = fonts.jetbrains_mono(16.0);
    let label_tracking_em = 0.18;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let gap = 11.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let label_w = measure_tracked_em(&label_font, label, label_tracking_em);
    let (value_w, _) = value_font.measure_str(&value, Some(&probe));

    let (_, m) = value_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 16.0 * 0.72 };
    let chip_w = pad_x * 2.0 + label_w + gap + value_w;
    let chip_h = pad_y * 2.0 + cap;
    let (cx, cy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(cx, cy, chip_w, chip_h);
    draw_chip(canvas, chip, radius);

    let baseline_y = cy + pad_y + cap;

    // "XYZ" — tracked rose label.
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        label,
        (cx + pad_x, baseline_y),
        &label_font,
        &label_paint,
        label_tracking_em,
    );

    // Position — pearl mono.
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    value_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(
        &value,
        (cx + pad_x + label_w + gap, baseline_y),
        &value_font,
        &value_paint,
    );

    chip
}

/// One key-cap glyph.
enum KeyGlyph {
    Letter(&'static str),
    Bar, // the space bar
}

/// Draw one WASD/space key cap — re-skin of `hud.jsx`'s `.hud-key`.
fn draw_key(canvas: &Canvas, rect: Rect, active: bool, fonts: &FontStore, glyph: KeyGlyph) {
    let radius = 9.0;
    // Active keys press down 2px (CSS `.hud-key.active { transform: translateY(2px) }`).
    let rect = if active {
        rect.with_offset((0.0, 2.0))
    } else {
        rect
    };
    let rrect = RRect::new_rect_xy(rect, radius, radius);

    if active {
        // Rose glow behind the cap (CSS `box-shadow: 0 0 20px`).
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(rgba(ROSE, 0.5), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 10.0, false));
        canvas.draw_rrect(rrect, &glow);

        // 135° rose→lavender gradient fill.
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (
                Point::new(rect.left, rect.top),
                Point::new(rect.right, rect.bottom),
            ),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[rgba(ROSE, 0.55), rgba(LAV, 0.45)],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            fill.set_shader(shader);
        }
        canvas.draw_rrect(rrect, &fill);

        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.5);
        border.set_color4f(rgba(ROSE, 0.7), None);
        canvas.draw_rrect(rrect, &border);
    } else {
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_color4f(rgba(WINE, 0.55), None);
        canvas.draw_rrect(rrect, &fill);

        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.5);
        border.set_color4f(Color4f::new(1.0, 1.0, 1.0, 0.15), None);
        canvas.draw_rrect(rrect, &border);
    }

    let glyph_color = if active {
        rgba(PEARL, 1.0)
    } else {
        rgba(MAUVE, 1.0)
    };
    let cx = rect.left + rect.width() / 2.0;
    let cy = rect.top + rect.height() / 2.0;
    match glyph {
        KeyGlyph::Letter(s) => {
            let font = fonts.jetbrains_mono(20.0);
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_color4f(glyph_color, None);
            let (lw, _) = font.measure_str(s, Some(&p));
            let (_, m) = font.metrics();
            let cap = if m.cap_height > 0.0 { m.cap_height } else { 14.0 };
            canvas.draw_str(s, (cx - lw / 2.0, cy + cap / 2.0), &font, &p);
        }
        KeyGlyph::Bar => {
            // The space bar — a short rounded underscore.
            let bw = 18.0;
            let bh = 3.0;
            let bar = Rect::from_xywh(cx - bw / 2.0, cy - bh / 2.0, bw, bh);
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_color4f(glyph_color, None);
            canvas.draw_rrect(RRect::new_rect_xy(bar, bh / 2.0, bh / 2.0), &p);
        }
    }
}

/// Keystrokes widget — re-skin of `hud.jsx`'s `keystrokes` element: a WASD
/// cross with a space bar below, lit as the keys are held.
fn draw_keystrokes(
    canvas: &Canvas,
    keys: i32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    const KEY: f32 = 36.0;
    const GAP: f32 = 4.0;
    let grid_w = KEY * 3.0 + GAP * 2.0;
    let row3_h = KEY * 0.7;
    let grid_h = KEY + GAP + KEY + GAP + row3_h;
    let (ox, oy) = anchor.origin(ax, ay, grid_w, grid_h);

    let col = |c: f32| ox + c * (KEY + GAP);
    let row2 = oy + KEY + GAP;
    let row3 = row2 + KEY + GAP;

    // Keys bitmask — mirror of `EwoHudData`'s bit layout.
    const FWD: i32 = 1;
    const LEFT: i32 = 1 << 1;
    const BACK: i32 = 1 << 2;
    const RIGHT: i32 = 1 << 3;
    const JUMP: i32 = 1 << 4;

    draw_key(
        canvas,
        Rect::from_xywh(col(1.0), oy, KEY, KEY),
        keys & FWD != 0,
        fonts,
        KeyGlyph::Letter("W"),
    );
    draw_key(
        canvas,
        Rect::from_xywh(col(0.0), row2, KEY, KEY),
        keys & LEFT != 0,
        fonts,
        KeyGlyph::Letter("A"),
    );
    draw_key(
        canvas,
        Rect::from_xywh(col(1.0), row2, KEY, KEY),
        keys & BACK != 0,
        fonts,
        KeyGlyph::Letter("S"),
    );
    draw_key(
        canvas,
        Rect::from_xywh(col(2.0), row2, KEY, KEY),
        keys & RIGHT != 0,
        fonts,
        KeyGlyph::Letter("D"),
    );
    draw_key(
        canvas,
        Rect::from_xywh(ox, row3, grid_w, row3_h),
        keys & JUMP != 0,
        fonts,
        KeyGlyph::Bar,
    );

    Rect::from_xywh(ox, oy, grid_w, grid_h)
}

/// Armor widget — re-skin of `hud.jsx`'s `armor` element: four durability
/// gauges (head/chest/legs/feet), each a dark slot with a rose→lavender fill
/// rising from the bottom and the percentage centred on it.
fn draw_armor(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    const SLOT_W: f32 = 50.0;
    const SLOT_H: f32 = 44.0;
    const GAP: f32 = 6.0;
    const PAD: f32 = 6.0;
    let chip_w = PAD * 2.0 + SLOT_W * 4.0 + GAP * 3.0;
    let chip_h = PAD * 2.0 + SLOT_H;
    let (ox, oy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(ox, oy, chip_w, chip_h);
    draw_chip(canvas, chip, 12.0);

    let pct_font = fonts.jetbrains_mono(14.0);

    for i in 0..4 {
        let sx = ox + PAD + i as f32 * (SLOT_W + GAP);
        let sy = oy + PAD;
        let slot = Rect::from_xywh(sx, sy, SLOT_W, SLOT_H);
        let slot_rr = RRect::new_rect_xy(slot, 6.0, 6.0);

        // Recessed dark track.
        let mut track = Paint::default();
        track.set_anti_alias(true);
        track.set_color4f(rgba(WINE, 0.85), None);
        canvas.draw_rrect(slot_rr, &track);

        if !data.armor_present(i) {
            continue;
        }
        let durability = data.armor_durability(i).clamp(0.0, 1.0);
        let fill_h = SLOT_H * durability;
        if fill_h > 0.5 {
            let fill_top = sy + SLOT_H - fill_h;
            let fill = Rect::from_xywh(sx, fill_top, SLOT_W, fill_h);
            // Clip to the slot's rounded corners while the fill rect is square.
            canvas.save();
            canvas.clip_rrect(slot_rr, Some(ClipOp::Intersect), Some(true));
            let mut fill_paint = Paint::default();
            fill_paint.set_anti_alias(true);
            // 180° rose→lavender down the bar (CSS `.hud-armor-bar`).
            if let Some(shader) = gradient_shader::linear(
                (Point::new(sx, fill_top), Point::new(sx, sy + SLOT_H)),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[rgba(ROSE, 1.0), rgba(LAV, 1.0)],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fill_paint.set_shader(shader);
            }
            canvas.draw_rect(fill, &fill_paint);
            canvas.restore();
        }

        // Percentage, centred on the slot, pearl with a hard shadow so it
        // reads over both the filled and empty parts of the gauge.
        let pct = format!("{}", (durability * 100.0).round() as i32);
        let mut probe = Paint::default();
        let (pw, _) = pct_font.measure_str(&pct, Some(&probe));
        let (_, pm) = pct_font.metrics();
        let pcap = if pm.cap_height > 0.0 { pm.cap_height } else { 10.0 };
        let px = sx + SLOT_W / 2.0 - pw / 2.0;
        let py = sy + SLOT_H / 2.0 + pcap / 2.0;

        probe.set_anti_alias(true);
        let mut shadow = Paint::default();
        shadow.set_anti_alias(true);
        shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.9), None);
        shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 2.0, false));
        canvas.draw_str(&pct, (px, py + 1.5), &pct_font, &shadow);

        let mut pct_paint = Paint::default();
        pct_paint.set_anti_alias(true);
        pct_paint.set_color4f(rgba(PEARL, 1.0), None);
        canvas.draw_str(&pct, (px, py), &pct_font, &pct_paint);
    }

    chip
}

/// Potion widget — re-skin of `hud.jsx`'s `potion` element: a column of active
/// effects, each a colour-keyed icon with the effect name and remaining time.
fn draw_potions(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let count = data.potion_count();
    if count == 0 {
        return empty_rect();
    }

    const ICON: f32 = 32.0;
    const ROW_GAP: f32 = 8.0;
    const PAD: f32 = 10.0;
    const ICON_GAP: f32 = 12.0;

    let name_font = fonts.newsreader(15.0);
    let time_font = fonts.jetbrains_mono(12.0);

    // Decode every row and measure the widest, so the chip is content-sized.
    let mut rows: Vec<(String, String, i32)> = Vec::with_capacity(count);
    let mut text_w = 0.0_f32;
    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    for i in 0..count {
        let p = data.potion(i);
        let name_line = if p.amplifier > 0 {
            format!("{} {}", p.name, roman(p.amplifier + 1))
        } else {
            p.name.clone()
        };
        let time_line = format_duration(p.duration);
        let (nw, _) = name_font.measure_str(&name_line, Some(&probe));
        let (tw, _) = time_font.measure_str(&time_line, Some(&probe));
        text_w = text_w.max(nw).max(tw);
        rows.push((name_line, time_line, p.color));
    }

    let chip_w = PAD * 2.0 + ICON + ICON_GAP + text_w;
    let chip_h = PAD * 2.0 + count as f32 * ICON + (count as f32 - 1.0) * ROW_GAP;
    let (ox, oy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(ox, oy, chip_w, chip_h);
    draw_chip(canvas, chip, 12.0);

    for (i, (name, time, color)) in rows.iter().enumerate() {
        let ry = oy + PAD + i as f32 * (ICON + ROW_GAP);

        // Colour-keyed icon — a vertical gradient (lighter top, darker bottom)
        // from the effect's packed RGB.
        let icon = Rect::from_xywh(ox + PAD, ry, ICON, ICON);
        let icon_rr = RRect::new_rect_xy(icon, 8.0, 8.0);
        let r = ((color >> 16) & 0xFF) as f32 / 255.0;
        let g = ((color >> 8) & 0xFF) as f32 / 255.0;
        let b = (color & 0xFF) as f32 / 255.0;
        let mut icon_paint = Paint::default();
        icon_paint.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (
                Point::new(icon.left, icon.top),
                Point::new(icon.left, icon.bottom),
            ),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new((r * 1.3).min(1.0), (g * 1.3).min(1.0), (b * 1.3).min(1.0), 1.0),
                    Color4f::new(r * 0.62, g * 0.62, b * 0.62, 1.0),
                ],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            icon_paint.set_shader(shader);
        }
        canvas.draw_rrect(icon_rr, &icon_paint);

        let text_x = ox + PAD + ICON + ICON_GAP;

        // Name — pearl Newsreader.
        let mut name_paint = Paint::default();
        name_paint.set_anti_alias(true);
        name_paint.set_color4f(rgba(PEARL, 1.0), None);
        canvas.draw_str(name, (text_x, ry + 14.0), &name_font, &name_paint);

        // Remaining time — mauve mono.
        let mut time_paint = Paint::default();
        time_paint.set_anti_alias(true);
        time_paint.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(time, (text_x, ry + 28.0), &time_font, &time_paint);
    }

    chip
}

/// TargetHUD — re-skin of `hud.jsx`'s `targethud` element: the looked-at
/// entity's initial avatar, name, distance and a health bar.
fn draw_target(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let name = data.target_name();
    let distance = data.target_distance();
    let health = data.target_health();
    let max_health = data.target_max_health();
    let has_health = max_health > 0.0;

    const AV: f32 = 64.0;
    const PAD_X: f32 = 18.0;
    const PAD_Y: f32 = 14.0;
    const GAP: f32 = 16.0;

    let name_font = fonts.fraunces_axes(21.0, 30.0, 0.0, 600.0, None);
    let dist_font = fonts.jetbrains_mono(13.0);
    let avatar_font = fonts.fraunces_axes(30.0, 40.0, 1.0, 700.0, None);

    let dist_str = format!("{:.1}m", distance);

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (name_w, _) = name_font.measure_str(&name, Some(&probe));
    let (dist_w, _) = dist_font.measure_str(&dist_str, Some(&probe));
    let meta_w = (name_w + 16.0 + dist_w).max(190.0); // min width keeps the bar usable

    let chip_w = PAD_X * 2.0 + AV + GAP + meta_w;
    let chip_h = PAD_Y * 2.0 + AV;
    let (ox, oy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(ox, oy, chip_w, chip_h);
    draw_chip(canvas, chip, 16.0);

    // ── Avatar — a rose→lavender tile with the name's initial ──────────────
    let avatar = Rect::from_xywh(ox + PAD_X, oy + PAD_Y, AV, AV);
    let avatar_rr = RRect::new_rect_xy(avatar, 14.0, 14.0);

    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_color4f(rgba(ROSE, 0.5), None);
    glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 10.0, false));
    canvas.draw_rrect(avatar_rr, &glow);

    let mut avatar_paint = Paint::default();
    avatar_paint.set_anti_alias(true);
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(avatar.left, avatar.top),
            Point::new(avatar.right, avatar.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[rgba(ROSE, 1.0), rgba(LAV, 1.0)],
            None,
        ),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        avatar_paint.set_shader(shader);
    }
    canvas.draw_rrect(avatar_rr, &avatar_paint);

    let initial = name
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('?')
        .to_string();
    let mut initial_paint = Paint::default();
    initial_paint.set_anti_alias(true);
    initial_paint.set_color4f(rgba(PEARL, 1.0), None);
    let (iw, _) = avatar_font.measure_str(&initial, Some(&initial_paint));
    let (_, im) = avatar_font.metrics();
    let icap = if im.cap_height > 0.0 { im.cap_height } else { 22.0 };
    canvas.draw_str(
        &initial,
        (
            avatar.left + AV / 2.0 - iw / 2.0,
            avatar.top + AV / 2.0 + icap / 2.0,
        ),
        &avatar_font,
        &initial_paint,
    );

    // ── Meta — name + distance row, then a health bar ──────────────────────
    let meta_x = ox + PAD_X + AV + GAP;
    let meta_right = ox + chip_w - PAD_X;
    let name_baseline = oy + PAD_Y + 22.0;

    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&name, (meta_x, name_baseline), &name_font, &name_paint);

    let mut dist_paint = Paint::default();
    dist_paint.set_anti_alias(true);
    dist_paint.set_color4f(rgba(ROSE, 0.9), None);
    canvas.draw_str(
        &dist_str,
        (meta_right - dist_w, name_baseline),
        &dist_font,
        &dist_paint,
    );

    if has_health {
        let bar_y = name_baseline + 12.0;
        let bar_h = 6.0;
        let bar = Rect::from_xywh(meta_x, bar_y, meta_right - meta_x, bar_h);
        let bar_rr = RRect::new_rect_xy(bar, bar_h / 2.0, bar_h / 2.0);

        let mut track = Paint::default();
        track.set_anti_alias(true);
        track.set_color4f(rgba(WINE, 0.85), None);
        canvas.draw_rrect(bar_rr, &track);

        let frac = (health / max_health).clamp(0.0, 1.0);
        let fill_w = bar.width() * frac;
        if fill_w > 1.0 {
            let fill = Rect::from_xywh(bar.left, bar_y, fill_w, bar_h);
            let mut fill_paint = Paint::default();
            fill_paint.set_anti_alias(true);
            // 90° rose→lavender across the bar (CSS `.hud-target-fill`).
            if let Some(shader) = gradient_shader::linear(
                (Point::new(bar.left, bar_y), Point::new(bar.right, bar_y)),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[rgba(ROSE, 1.0), rgba(LAV, 1.0)],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fill_paint.set_shader(shader);
            }
            canvas.draw_rrect(RRect::new_rect_xy(fill, bar_h / 2.0, bar_h / 2.0), &fill_paint);
        }
    }

    chip
}

// ────────────────────────────────────────────────────────────────────────
// HUD editor chrome — drawn over the widgets while the overlay is open.
// ────────────────────────────────────────────────────────────────────────

/// Draw the editor: a faint tint, a drag outline around each visible widget
/// (the hovered/dragged one highlighted + labelled), and a hint line.
fn draw_editor(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    // Faint tint so the editor reads as a distinct mode.
    let mut tint = Paint::default();
    tint.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.22), None);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, h), &tint);

    // Alignment guides — drawn while a drag is snapped to another widget.
    let mut guide = Paint::default();
    guide.set_anti_alias(true);
    guide.set_style(PaintStyle::Stroke);
    guide.set_stroke_width(1.0);
    guide.set_color4f(rgba(ROSE, 0.5), None);
    if let Some(sx) = editor.snap_x {
        canvas.draw_line((sx, 0.0), (sx, h), &guide);
    }
    if let Some(sy) = editor.snap_y {
        canvas.draw_line((0.0, sy), (w, sy), &guide);
    }

    // Widget outlines — the hovered/dragged or panel-selected one is lit.
    let active = editor.active_widget();
    for id in WidgetId::ALL {
        let b = editor.bounds[id.index()];
        if b.width() <= 0.0 {
            continue;
        }
        let lit = Some(id) == active || Some(id) == editor.selected;
        draw_widget_outline(canvas, b, lit, fonts, id.title());
    }

    draw_side_panel(canvas, editor, fonts, h);

    // Hint, centred along the bottom.
    let hint_font = fonts.jetbrains_mono(12.0);
    let hint = "DRAG WIDGETS OR USE THE PANEL  ·  RIGHT SHIFT OR ESC TO CLOSE";
    let hint_tracking = 0.14;
    let hint_w = measure_tracked_em(&hint_font, hint, hint_tracking);
    let mut hint_paint = Paint::default();
    hint_paint.set_anti_alias(true);
    hint_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        hint,
        ((w - hint_w) * 0.5, h - 28.0),
        &hint_font,
        &hint_paint,
        hint_tracking,
    );
}

/// A drag outline around one widget — a rose rounded-rect; the active widget
/// gets a brighter ring and a name label above it.
fn draw_widget_outline(canvas: &Canvas, bounds: Rect, active: bool, fonts: &FontStore, title: &str) {
    let pad = 4.0;
    let outline = Rect::from_xywh(
        bounds.left - pad,
        bounds.top - pad,
        bounds.width() + pad * 2.0,
        bounds.height() + pad * 2.0,
    );
    let rrect = RRect::new_rect_xy(outline, 8.0, 8.0);

    let mut stroke = Paint::default();
    stroke.set_anti_alias(true);
    stroke.set_style(PaintStyle::Stroke);
    if active {
        stroke.set_stroke_width(2.0);
        stroke.set_color4f(rgba(ROSE, 0.95), None);
    } else {
        stroke.set_stroke_width(1.5);
        stroke.set_color4f(rgba(ROSE, 0.4), None);
    }
    canvas.draw_rrect(rrect, &stroke);

    if active {
        let label_font = fonts.jetbrains_mono(11.0);
        let mut label_paint = Paint::default();
        label_paint.set_anti_alias(true);
        label_paint.set_color4f(rgba(ROSE, 1.0), None);
        draw_tracked_em(
            canvas,
            title,
            (outline.left + 2.0, outline.top - 7.0),
            &label_font,
            &label_paint,
            0.12,
        );
    }
}

// ────────────────────────────────────────────────────────────────────────
// Editor side panel — widget toggles + the 9-anchor preset grid.
// ────────────────────────────────────────────────────────────────────────

/// The 9 anchor presets the grid offers — an anchor plus the fractional
/// corner/edge/centre it sends the widget to.
const ANCHOR_PRESETS: [(Anchor, f32, f32); 9] = [
    (Anchor::Tl, 0.02, 0.03),
    (Anchor::Tc, 0.50, 0.03),
    (Anchor::Tr, 0.98, 0.03),
    (Anchor::Ml, 0.02, 0.50),
    (Anchor::Mc, 0.50, 0.50),
    (Anchor::Mr, 0.98, 0.50),
    (Anchor::Bl, 0.02, 0.97),
    (Anchor::Bc, 0.50, 0.97),
    (Anchor::Br, 0.98, 0.97),
];

/// Hit-rects of the editor side panel, computed from the window height.
struct PanelLayout {
    panel: Rect,
    rows: [Rect; 7],
    toggles: [Rect; 7],
    cells: [Rect; 9],
}

/// Lay out the left-edge editor panel — chip, widget rows + toggles, anchor
/// grid. Deterministic in the window height, so the renderer and the
/// hit-tester agree.
fn panel_layout(h: f32) -> PanelLayout {
    const PANEL_W: f32 = 234.0;
    const PANEL_X: f32 = 20.0;
    const PAD: f32 = 18.0;
    const HEADER_H: f32 = 30.0; // eyebrow + gap
    const WLABEL_H: f32 = 22.0; // "WIDGETS" label + gap
    const ROW_H: f32 = 30.0;
    const SECTION_GAP: f32 = 20.0;
    const ALABEL_H: f32 = 22.0; // "ANCHOR" label + gap
    const CELL: f32 = 42.0;
    const CELL_GAP: f32 = 6.0;

    let grid_h = CELL * 3.0 + CELL_GAP * 2.0;
    let panel_h =
        PAD * 2.0 + HEADER_H + WLABEL_H + ROW_H * 7.0 + SECTION_GAP + ALABEL_H + grid_h;
    let panel_y = (h - panel_h) * 0.5;
    let panel = Rect::from_xywh(PANEL_X, panel_y, PANEL_W, panel_h);

    let content_x = PANEL_X + PAD;
    let content_w = PANEL_W - PAD * 2.0;

    let rows_top = panel_y + PAD + HEADER_H + WLABEL_H;
    let mut rows = [empty_rect(); 7];
    let mut toggles = [empty_rect(); 7];
    for i in 0..7 {
        let row = Rect::from_xywh(content_x, rows_top + i as f32 * ROW_H, content_w, ROW_H);
        rows[i] = row;
        let tw = 34.0;
        let th = 18.0;
        toggles[i] = Rect::from_xywh(row.right - tw, row.top + (ROW_H - th) * 0.5, tw, th);
    }

    let grid_top = rows_top + ROW_H * 7.0 + SECTION_GAP + ALABEL_H;
    let grid_w = CELL * 3.0 + CELL_GAP * 2.0;
    let grid_x = content_x + (content_w - grid_w) * 0.5;
    let mut cells = [empty_rect(); 9];
    for r in 0..3 {
        for c in 0..3 {
            cells[r * 3 + c] = Rect::from_xywh(
                grid_x + c as f32 * (CELL + CELL_GAP),
                grid_top + r as f32 * (CELL + CELL_GAP),
                CELL,
                CELL,
            );
        }
    }

    PanelLayout {
        panel,
        rows,
        toggles,
        cells,
    }
}

/// Draw the editor side panel: a widget list (name + enable toggle) and a
/// 3×3 anchor preset grid for the selected widget.
fn draw_side_panel(canvas: &Canvas, editor: &Editor, fonts: &FontStore, h: f32) {
    let pl = panel_layout(h);
    draw_chip(canvas, pl.panel, 14.0);

    let pad = 18.0;
    let left = pl.panel.left + pad;

    // Eyebrow.
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        "HUD EDITOR",
        (left, pl.panel.top + pad + 4.0),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    // Section labels.
    let label_font = fonts.jetbrains_mono(10.0);
    let mut label = Paint::default();
    label.set_anti_alias(true);
    label.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "WIDGETS",
        (left, pl.rows[0].top - 8.0),
        &label_font,
        &label,
        0.18,
    );
    draw_tracked_em(
        canvas,
        "ANCHOR",
        (left, pl.cells[0].top - 8.0),
        &label_font,
        &label,
        0.18,
    );

    // Widget rows.
    let name_font = fonts.jetbrains_mono(12.0);
    let (_, nm) = name_font.metrics();
    let ncap = if nm.cap_height > 0.0 { nm.cap_height } else { 9.0 };
    for (i, id) in WidgetId::ALL.into_iter().enumerate() {
        let row = pl.rows[i];
        let wl = editor.layout.get(id);
        let mid_y = row.top + row.height() * 0.5;

        if editor.selected == Some(id) {
            let mut hl = Paint::default();
            hl.set_anti_alias(true);
            hl.set_color4f(rgba(ROSE, 0.14), None);
            canvas.draw_rrect(RRect::new_rect_xy(row, 7.0, 7.0), &hl);
        }

        // Enabled dot.
        let mut dot = Paint::default();
        dot.set_anti_alias(true);
        dot.set_color4f(
            if wl.enabled { rgba(ROSE, 1.0) } else { rgba(MAUVE, 0.4) },
            None,
        );
        canvas.draw_circle((row.left + 9.0, mid_y), 3.5, &dot);

        // Name.
        let mut name = Paint::default();
        name.set_anti_alias(true);
        name.set_color4f(
            if wl.enabled { rgba(PEARL, 1.0) } else { rgba(MAUVE, 0.7) },
            None,
        );
        draw_tracked_em(
            canvas,
            id.title(),
            (row.left + 24.0, mid_y + ncap * 0.5),
            &name_font,
            &name,
            0.06,
        );

        draw_panel_toggle(canvas, pl.toggles[i], wl.enabled);
    }

    // Anchor preset grid — highlights the selected widget's current anchor.
    let has_selection = editor.selected.is_some();
    let selected_anchor = editor.selected.map(|s| editor.layout.get(s).anchor);
    for (i, &(cell_anchor, _, _)) in ANCHOR_PRESETS.iter().enumerate() {
        let current = selected_anchor == Some(cell_anchor);
        draw_anchor_cell(canvas, pl.cells[i], cell_anchor, current, has_selection);
    }
}

/// A small on/off pill toggle for a widget row.
fn draw_panel_toggle(canvas: &Canvas, rect: Rect, on: bool) {
    let r = rect.height() * 0.5;
    let rrect = RRect::new_rect_xy(rect, r, r);

    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(if on { rgba(ROSE, 0.6) } else { rgba(WINE, 0.85) }, None);
    canvas.draw_rrect(rrect, &bg);
    if !on {
        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.0);
        border.set_color4f(rgba(ROSE, 0.25), None);
        canvas.draw_rrect(rrect, &border);
    }

    let knob_r = r - 3.0;
    let cx = if on {
        rect.right - knob_r - 3.0
    } else {
        rect.left + knob_r + 3.0
    };
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(if on { rgba(PEARL, 1.0) } else { rgba(MAUVE, 0.9) }, None);
    canvas.draw_circle((cx, rect.top + r), knob_r, &knob);
}

/// One cell of the anchor grid — a mini screen-position map: a dot sits where
/// the cell's anchor would pin the widget. The selected widget's current
/// anchor cell is highlighted.
fn draw_anchor_cell(canvas: &Canvas, rect: Rect, anchor: Anchor, current: bool, active: bool) {
    let rrect = RRect::new_rect_xy(rect, 6.0, 6.0);

    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(
        if current { rgba(ROSE, 0.28) } else { rgba(WINE, 0.7) },
        None,
    );
    canvas.draw_rrect(rrect, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(
        if current { rgba(ROSE, 0.8) } else { rgba(ROSE, 0.15) },
        None,
    );
    canvas.draw_rrect(rrect, &border);

    // The position dot.
    let (fx, fy) = anchor.fractions();
    let inset = 10.0;
    let dx = rect.left + inset + (rect.width() - inset * 2.0) * fx;
    let dy = rect.top + inset + (rect.height() - inset * 2.0) * fy;
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(
        if !active {
            rgba(MAUVE, 0.45)
        } else if current {
            rgba(PEARL, 1.0)
        } else {
            rgba(ROSE, 0.85)
        },
        None,
    );
    canvas.draw_circle((dx, dy), 3.0, &dot);
}
