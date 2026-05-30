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

use ewo_core::modules as catalog;
use ewo_render::text::{draw_tracked_em, measure_tracked_em};
use ewo_render::FontStore;
use skia_safe::{
    gradient_shader, BlurStyle, Canvas, ClipOp, Color4f, Data, Font, Image, MaskFilter, Paint,
    PaintStyle, Point, RRect, Rect, TileMode,
};

// ── Velvet theme tokens (see CLAUDE.md "Velvet theme tokens") ──────────────
const PEARL: (u8, u8, u8) = (0xF4, 0xE8, 0xEA); // --text-pearl
const MAUVE: (u8, u8, u8) = (0x9A, 0x80, 0x87); // --text-mauve
const ROSE: (u8, u8, u8) = (0xE5, 0xB8, 0xC5); // --accent-rose
const LAV: (u8, u8, u8) = (0xC9, 0xA5, 0xD4); // --accent-lav
const BERRY: (u8, u8, u8) = (0xB4, 0x74, 0x91); // --accent-berry
const CHAMP: (u8, u8, u8) = (0xE8, 0xD4, 0xA8); // --accent-champ
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
pub const SCHEMA_VERSION: i32 = 10;

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
    pub const PLAYTIME: usize = 500;
    pub const SERVER: usize = 504;
    pub const PLAYER_NAME: usize = 556;
    // PvP Utils (schema 4): two contiguous records — jump reset, then hit range.
    pub const PVP_JUMP: usize = 584; // i32 tier, i32 offset_ms, i32 age_ticks, i32 fade_total
    pub const PVP_HIT: usize = 600;  // f32 distance, i32 color_rgb, i32 age_ticks, i32 fade_total
    // Combat HUD additions (schema 5): CPS pair + four tracked item counts.
    pub const CPS_LEFT: usize = 616;
    pub const CPS_RIGHT: usize = 620;
    pub const ITEM_PEARLS: usize = 624;
    pub const ITEM_ARROWS: usize = 628;
    pub const ITEM_TOTEMS: usize = 632;
    pub const ITEM_GAPPLES: usize = 636;
    // Indicators block (schema 6): i32 count + up to MAX_INDICATORS records.
    pub const INDICATORS: usize = 640;
    // Combat HUD additions (schema 7): local-player shield cooldown fraction.
    pub const SHIELD_COOLDOWN: usize = 1284;
    // Hit indicator (schema 8): present + relative yaw (deg) + age (sec).
    pub const HIT_PRESENT: usize = 1288;
    pub const HIT_REL_YAW: usize = 1292;
    pub const HIT_AGE: usize = 1296;
    // Attack-strength scale (schema 9): 0 = freshly attacked, 1 = ready.
    pub const ATTACK_CHARGE: usize = 1300;
    // Combo counter (schema 10): i32 hit count + f32 seconds-since-last-hit.
    pub const COMBO_COUNT: usize = 1304;
    pub const COMBO_AGE: usize = 1308;
}

/// Max per-frame indicator records — mirror of `EwoIndicators.MAX_TRACKED`.
pub const MAX_INDICATORS: usize = 16;
/// Bytes per indicator record — mirror of `EwoIndicators.RECORD`.
pub const INDICATOR_RECORD: usize = 40;
const FLAG_WORLD: i32 = 1; // a player + level exist → coords/keystrokes valid
const FLAG_PING: i32 = 1 << 1; // a server connection exists → ping valid
const FLAG_ARMOR: i32 = 1 << 2; // at least one armor piece is worn
const FLAG_TARGET: i32 = 1 << 3; // an entity is under the crosshair
const FLAG_OVERLAY: i32 = 1 << 4; // the EwoClient overlay is open
const FLAG_PVP_JUMP: i32 = 1 << 5; // a fresh jump-reset result is live
const FLAG_PVP_HIT: i32 = 1 << 6;  // a fresh hit-range result is live

/// Jump-reset tier — wire-mirror of `EwoJumpReset.Tier` ordinal mapping in
/// `EwoHudData.tierToInt`. The renderer dispatches on this.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PvpTier {
    None,
    Perfect,
    SlightlyEarly,
    Early,
    SlightlyLate,
    Late,
}

impl PvpTier {
    fn from_wire(v: i32) -> PvpTier {
        match v {
            1 => PvpTier::Perfect,
            2 => PvpTier::SlightlyEarly,
            3 => PvpTier::Early,
            4 => PvpTier::SlightlyLate,
            5 => PvpTier::Late,
            _ => PvpTier::None,
        }
    }

    /// Short tier label for the widget.
    fn label(self) -> &'static str {
        match self {
            PvpTier::Perfect => "PERFECT",
            PvpTier::SlightlyEarly => "SLIGHTLY EARLY",
            PvpTier::Early => "EARLY",
            PvpTier::SlightlyLate => "SLIGHTLY LATE",
            PvpTier::Late => "LATE",
            PvpTier::None => "NO RESET",
        }
    }
}

const MAX_POTIONS: usize = 8;
const POTION_REC: usize = 44; // bytes per potion record
const POTION_NAME_CAP: usize = 28;
const TARGET_NAME_CAP: usize = 44;
const SERVER_CAP: usize = 48;
const PLAYER_NAME_CAP: usize = 24;

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
    /// Session playtime in seconds (since the game launched).
    pub fn playtime(&self) -> i32 {
        self.i32_at(off::PLAYTIME)
    }
    /// The current server address, "Singleplayer", or "" on the main menu.
    pub fn server(&self) -> String {
        self.str_at(off::SERVER, SERVER_CAP)
    }
    /// The signed-in account / player name.
    pub fn player_name(&self) -> String {
        self.str_at(off::PLAYER_NAME, PLAYER_NAME_CAP)
    }

    // ── PvP Utils (schema 4) ──────────────────────────────────────────────

    /// A jump-reset result is live this frame (within the fade window).
    pub fn pvp_jump_active(&self) -> bool {
        self.flag(FLAG_PVP_JUMP)
    }
    pub fn pvp_jump_tier(&self) -> PvpTier {
        PvpTier::from_wire(self.i32_at(off::PVP_JUMP))
    }
    pub fn pvp_jump_offset_ms(&self) -> i32 {
        self.i32_at(off::PVP_JUMP + 4)
    }
    /// Fade progress 0..1 — `age_ticks / fade_total`.
    pub fn pvp_jump_fade(&self) -> f32 {
        let total = self.i32_at(off::PVP_JUMP + 12).max(1) as f32;
        let age = self.i32_at(off::PVP_JUMP + 8).max(0) as f32;
        1.0 - (age / total).clamp(0.0, 1.0)
    }

    /// A hit-range result is live this frame (within the fade window).
    pub fn pvp_hit_active(&self) -> bool {
        self.flag(FLAG_PVP_HIT)
    }
    pub fn pvp_hit_distance(&self) -> f32 {
        self.f32_at(off::PVP_HIT)
    }
    /// Matched zone's packed `0xRRGGBB` colour, or 0 if no result.
    pub fn pvp_hit_color(&self) -> i32 {
        self.i32_at(off::PVP_HIT + 4)
    }
    pub fn pvp_hit_fade(&self) -> f32 {
        let total = self.i32_at(off::PVP_HIT + 12).max(1) as f32;
        let age = self.i32_at(off::PVP_HIT + 8).max(0) as f32;
        1.0 - (age / total).clamp(0.0, 1.0)
    }

    // ── Combat HUD additions (schema 5) ───────────────────────────────────

    pub fn cps_left(&self) -> i32 {
        self.i32_at(off::CPS_LEFT)
    }
    pub fn cps_right(&self) -> i32 {
        self.i32_at(off::CPS_RIGHT)
    }
    pub fn item_pearls(&self) -> i32 {
        self.i32_at(off::ITEM_PEARLS)
    }
    pub fn item_arrows(&self) -> i32 {
        self.i32_at(off::ITEM_ARROWS)
    }
    pub fn item_totems(&self) -> i32 {
        self.i32_at(off::ITEM_TOTEMS)
    }
    pub fn item_gapples(&self) -> i32 {
        self.i32_at(off::ITEM_GAPPLES)
    }

    // ── World-anchored indicators (schema 6) ──────────────────────────────

    /// How many indicator records the mod wrote this frame (capped at
    /// [`MAX_INDICATORS`]).
    pub fn indicator_count(&self) -> usize {
        (self.i32_at(off::INDICATORS).max(0) as usize).min(MAX_INDICATORS)
    }

    // ── Combat HUD additions (schema 7) ──────────────────────────────────

    /// Local-player shield cooldown fraction: 0 = ready, 1 = just disabled.
    pub fn shield_cooldown(&self) -> f32 {
        self.f32_at(off::SHIELD_COOLDOWN)
    }

    /// Hit-indicator: an attacker is currently being tracked (recent hit).
    pub fn hit_present(&self) -> bool {
        self.i32_at(off::HIT_PRESENT) != 0
    }
    /// Yaw to the attacker, relative to the local player's facing. Degrees,
    /// normalised to `[-180, 180]`. `0` = directly ahead, `±180` = behind.
    pub fn hit_relative_yaw(&self) -> f32 {
        self.f32_at(off::HIT_REL_YAW)
    }
    /// Seconds since the most recent hit. Renderer fades the chevron by this.
    pub fn hit_age(&self) -> f32 {
        self.f32_at(off::HIT_AGE)
    }

    /// Local-player attack-strength scale (0..1). 0 = just attacked, 1 = ready
    /// for full-damage hit. Drives the Attack Charge HUD + the Auto Hit
    /// Timing trigger threshold.
    pub fn attack_charge(&self) -> f32 {
        self.f32_at(off::ATTACK_CHARGE)
    }

    /// Consecutive-hit counter for the Combo Counter widget. Resets on the
    /// player taking damage OR a 5 s timeout from the Java side.
    pub fn combo_count(&self) -> i32 {
        self.i32_at(off::COMBO_COUNT)
    }
    /// Seconds since the last combo hit landed. Renderer fades the chip
    /// alpha by this for a smoother decay (the count itself snaps to 0 on
    /// timeout, but the visual can ease out).
    pub fn combo_age(&self) -> f32 {
        self.f32_at(off::COMBO_AGE)
    }

    /// One indicator record — decoded copy of slot `i` in the block.
    pub fn indicator(&self, i: usize) -> Indicator {
        let rec = off::INDICATORS + 4 + i * INDICATOR_RECORD;
        Indicator {
            entity_id: self.i32_at(rec),
            screen_x: self.f32_at(rec + 4),
            screen_y: self.f32_at(rec + 8),
            distance: self.f32_at(rec + 12),
            in_view: self.i32_at(rec + 16) != 0,
            totem_count: self.i32_at(rec + 20),
            health: self.f32_at(rec + 24),
            max_health: self.f32_at(rec + 28),
            last_damage: self.f32_at(rec + 32),
            damage_age_sec: self.f32_at(rec + 36),
        }
    }
}

/// One overhead-indicator record — a tracked LivingEntity projected to
/// screen space, with its persistent totem count and most-recent damage.
///
/// `entity_id` + `distance` are not consumed by the current draws but stay on
/// the wire so future polish (distance-fade, per-entity rate-limit, opt-in
/// list) doesn't need a schema bump to read them.
#[allow(dead_code)]
pub struct Indicator {
    pub entity_id: i32,
    pub screen_x: f32,
    pub screen_y: f32,
    /// World-space distance to the local player (blocks).
    pub distance: f32,
    /// `true` if the head position is on (or near) the screen.
    pub in_view: bool,
    /// Running tally of observed totem-of-undying activations.
    pub totem_count: i32,
    pub health: f32,
    pub max_health: f32,
    /// Damage delta from the most recent health drop. Stale if
    /// [`damage_age_sec`] is `< 0`.
    pub last_damage: f32,
    /// Seconds since the most recent damage hit; `< 0` means no live damage
    /// (the fade has elapsed or none has been seen yet).
    pub damage_age_sec: f32,
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

/// Every HUD widget, in draw order. New widgets append at the end so existing
/// indices in `hud.toml` stay stable across schema bumps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WidgetId {
    Fps,
    Coords,
    Ping,
    Keystrokes,
    Armor,
    Potions,
    Target,
    JumpResetText,
    JumpResetBar,
    HitRange,
    Cps,
    Items,
    ShieldCooldown,
    Reach,
    AttackCharge,
    Combo,
    Media,
}

impl WidgetId {
    const ALL: [WidgetId; 17] = [
        WidgetId::Fps,
        WidgetId::Coords,
        WidgetId::Ping,
        WidgetId::Keystrokes,
        WidgetId::Armor,
        WidgetId::Potions,
        WidgetId::Target,
        WidgetId::JumpResetText,
        WidgetId::JumpResetBar,
        WidgetId::HitRange,
        WidgetId::Cps,
        WidgetId::Items,
        WidgetId::ShieldCooldown,
        WidgetId::Reach,
        WidgetId::AttackCharge,
        WidgetId::Combo,
        WidgetId::Media,
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
            WidgetId::JumpResetText => "jump_reset_text",
            WidgetId::JumpResetBar => "jump_reset_bar",
            WidgetId::HitRange => "hit_range",
            WidgetId::Cps => "cps",
            WidgetId::Items => "items",
            WidgetId::ShieldCooldown => "shield_cooldown",
            WidgetId::Reach => "reach",
            WidgetId::AttackCharge => "attack_charge",
            WidgetId::Combo => "combo",
            WidgetId::Media => "media",
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
            WidgetId::JumpResetText => "JUMP RESET",
            WidgetId::JumpResetBar => "JUMP RESET BAR",
            WidgetId::HitRange => "HIT RANGE",
            WidgetId::Cps => "CPS",
            WidgetId::Items => "ITEMS",
            WidgetId::ShieldCooldown => "SHIELD CD",
            WidgetId::Reach => "REACH",
            WidgetId::AttackCharge => "ATTACK CHARGE",
            WidgetId::Combo => "COMBO",
            WidgetId::Media => "MEDIA",
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

/// The persisted HUD config — the per-widget layout plus HUD prefs. Saved to
/// `hud.toml`.
struct HudLayout {
    widgets: [WidgetLayout; 17],
    /// The paint-rate cap — a pref, kept here so it shares `hud.toml`.
    paint_rate: crate::HudPaintRate,
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
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.8700 }, // jump_reset_text
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.8300 }, // jump_reset_bar
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.7500 }, // hit_range
                WidgetLayout { enabled: true, anchor: Anchor::Tr, x: 0.9865, y: 0.0204 }, // cps
                WidgetLayout { enabled: true, anchor: Anchor::Bl, x: 0.0135, y: 0.9000 }, // items
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.6800 }, // shield_cooldown
                WidgetLayout { enabled: true, anchor: Anchor::Tc, x: 0.5000, y: 0.1100 }, // reach
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.5800 }, // attack_charge
                WidgetLayout { enabled: true, anchor: Anchor::Tc, x: 0.5000, y: 0.1800 }, // combo
                // Media — default off (the user opts in) at top-right under the
                // CPS chip. Once they enable + place it the layout is persisted.
                WidgetLayout { enabled: false, anchor: Anchor::Tr, x: 0.9865, y: 0.1100 }, // media
            ],
            paint_rate: crate::HudPaintRate::Match,
        }
    }

    /// Load `hud.toml`, falling back to the default for anything missing or
    /// malformed — so a hand-edited or absent file never breaks the HUD.
    fn load() -> Self {
        let mut layout = Self::defaults();
        // Per-profile path first; fall back to the pre-Phase-F single file
        // so an existing layout survives the move to per-profile files.
        let text = hud_toml_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .or_else(|| legacy_hud_toml_path().and_then(|p| std::fs::read_to_string(p).ok()));
        let Some(text) = text else {
            return layout;
        };
        let mut current: Option<WidgetId> = None;
        let mut in_prefs = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                in_prefs = section == "prefs";
                current = WidgetId::ALL.into_iter().find(|id| id.key() == section);
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if in_prefs {
                if key == "paint_rate" {
                    if let Some(r) = crate::HudPaintRate::from_str(value) {
                        layout.paint_rate = r;
                    }
                }
                continue;
            }
            let Some(id) = current else {
                continue;
            };
            let wl = layout.get_mut(id);
            match key {
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
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
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
        s.push_str(&format!(
            "\n[prefs]\npaint_rate = \"{}\"\n",
            self.paint_rate.as_str()
        ));
        let _ = std::fs::write(&path, s);
    }
}

/// `<config>/EwoClient/profiles/<active>/hud.toml` — the HUD layout is
/// per client profile (Phase F). Resolved from `%APPDATA%` — the cdylib
/// runs inside the Minecraft JVM, which inherits the launcher's environment.
fn hud_toml_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let profile = read_active_profile().unwrap_or_else(|| "Default".to_string());
    Some(
        PathBuf::from(appdata)
            .join("EwoClient")
            .join("profiles")
            .join(profile)
            .join("hud.toml"),
    )
}

/// The pre-Phase-F single `hud.toml` location. Read as a fallback so an
/// existing layout survives the move to per-profile files; the next save
/// writes the new per-profile path.
fn legacy_hud_toml_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join("EwoClient").join("hud.toml"))
}

// ────────────────────────────────────────────────────────────────────────
// Editor state.
// ────────────────────────────────────────────────────────────────────────

/// Which view the overlay dashboard is showing. The overlay is a shell — a
/// top-centre tab strip switches between these; the HUD editor is one view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OverlayView {
    Home,
    HudEditor,
    Crosshair,
    Modules,
    Pvp,
    Mods,
    Settings,
}

impl OverlayView {
    const ALL: [OverlayView; 7] = [
        OverlayView::Home,
        OverlayView::HudEditor,
        OverlayView::Crosshair,
        OverlayView::Modules,
        OverlayView::Pvp,
        OverlayView::Mods,
        OverlayView::Settings,
    ];
    fn title(self) -> &'static str {
        match self {
            OverlayView::Home => "HOME",
            OverlayView::HudEditor => "HUD",
            OverlayView::Crosshair => "CROSSHAIR",
            OverlayView::Modules => "MODULES",
            OverlayView::Pvp => "PVP",
            OverlayView::Mods => "MODS",
            OverlayView::Settings => "SETTINGS",
        }
    }
}

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
    /// Which dashboard view the overlay is showing.
    view: OverlayView,
    layout: HudLayout,
    /// Window size from the last paint — for pixel ↔ fraction conversion.
    window: (f32, f32),
    /// Cursor position in window pixels.
    cursor: (f32, f32),
    /// Each widget's drawn bounds, recorded each paint (indexed by `WidgetId`).
    bounds: [Rect; 17],
    dragging: Option<Drag>,
    /// An in-progress MODULES-view slider drag — `(module index, setting slot)`.
    slider_drag: Option<(usize, usize)>,
    /// Active snap guide lines (window pixels) while a drag is alignment-snapped.
    snap_x: Option<f32>,
    snap_y: Option<f32>,
    /// The widget selected in the side panel — drives the anchor grid.
    selected: Option<WidgetId>,
    /// Bundled mods for the MODS view — loaded from `overlay-mods.toml`.
    mods: Vec<ModEntry>,
    /// EwoClient modules — enabled state + settings, per client profile.
    /// `pub(crate)` so the JNI layer can write the state buffer and toggle.
    pub(crate) modules: crate::modules::ModuleConfig,
    /// Active client-profile name. Read at construction; updated when the
    /// SETTINGS-tab picker switches profile.
    active_profile: String,
    /// All client-profile names — for the SETTINGS-tab picker. Read once at
    /// construction (a launcher-created profile needs a game restart).
    profiles: Vec<String>,
    /// The signed-in player's skin / cape images for the HOME 3D viewer.
    skin_image: Option<Image>,
    cape_image: Option<Image>,
    /// HOME skin-viewer rotation (radians) + the in-progress drag's last x.
    skin_yaw: f32,
    skin_drag: Option<f32>,
    /// Whether the loaded skin uses the slim ("Alex") 3px-arm model.
    skin_slim: bool,
    /// `ewo-skin.png`'s mtime when it was last loaded — the export thread
    /// rewrites the file after the `Editor` was built, and may also replace a
    /// stale png left by an earlier launch, so the viewer reloads on a change.
    skin_mtime: Option<std::time::SystemTime>,
    /// PvP-Utils config — loaded from the active profile's `pvp.toml`,
    /// edited from the PVP overlay tab, saved on each commit. The Java mod
    /// polls the file's mtime and hot-reloads — so edits apply live.
    pvp: crate::pvp::PvpConfig,
    /// An in-progress PVP-tab slider drag — identifies which control is held.
    pvp_drag: Option<PvpDrag>,
    /// Module-keybind map for the MODULES-tab chip — `(action_id, glfw_code)`.
    /// Loaded once from the instance dir's `ewo-keybinds.txt` (which the
    /// launcher writes before launch from the active profile). Linear-scanned
    /// — ~30 entries, fine without a HashMap.
    keybinds: Vec<(String, i32)>,
    /// Currently-playing media — title / artist / position / thumbnail.
    /// Empty by default; the SMTC backend ([`media_service`]) writes into this.
    pub(crate) media: crate::media::MediaState,
    /// Background SMTC poller. Polled every frame in `draw()` so the media
    /// widget's state stays fresh without the polling thread touching Skia
    /// state directly. Send transport actions back to it via `.act(...)`.
    pub(crate) media_service: crate::media::MediaService,
    /// Vertical scroll offset (in logical px) for the MODULES tab — the only
    /// dashboard view tall enough to need scrolling. Reset whenever the view
    /// changes so a switch in/out always starts at the top.
    pub(crate) modules_scroll: f32,
    /// Per-frame cache of the HOME quick-toggle chip rects. Chips are flowed
    /// at render time (their width depends on the text) so the press handler
    /// can't compute them without a `FontStore`; the renderer writes here.
    pub(crate) home_toggle_bounds: [Rect; 17],
    /// MODULES tab — `Some(catalog_index)` while the per-module settings
    /// popover is open. Driven by right-click on a row with sliders; closed by
    /// the popover's own ✕ button or by clicking outside the card.
    pub(crate) module_popover: Option<usize>,
    /// Last media transport-button press — `(button_index, when)`, where
    /// `button_index` is 0=prev, 1=play, 2=next. Drives the brief
    /// "pressed" animation (scale-down + brighten) on the clicked button.
    /// Cleared implicitly when older than the fade window (≈160 ms).
    pub(crate) media_button_press: Option<(usize, std::time::Instant)>,
    /// Custom-crosshair config — loaded from `<profile>/crosshair.toml`,
    /// edited via the CROSSHAIR overlay tab, saved on each commit. The Java
    /// side reads `enabled` through `nativeIsCustomCrosshairEnabled` to
    /// decide whether to cancel vanilla's `Gui.extractCrosshair`.
    pub(crate) crosshair: crate::crosshair::CrosshairConfig,
    /// In-progress crosshair-editor interaction (slider drag + which colour
    /// slot is selected for the RGB editor pane).
    crosshair_ui: CrosshairUi,
}

/// In-progress state for the CROSSHAIR editor view.
#[derive(Clone, Copy, Debug)]
struct CrosshairUi {
    /// Slider / picker control currently held (none = nothing). Writes the
    /// value straight into [`Editor::crosshair`] each cursor move; the
    /// config is persisted on release.
    drag: Option<CrosshairSlider>,
    /// Which colour-slot the HSV picker targets. Driven by clicking one of
    /// the four colour swatches; defaults to the idle colour.
    selected_color: CrosshairColorSlot,
    /// HSV picker state for the currently-selected swatch. Synced from the
    /// slot's RGBA on swatch click; mutated by SV/H/A drags, written back
    /// out as RGB into the slot on each change. Stored separately because a
    /// strict RGB→HSV round-trip is lossy at S=0 / V=0 (H becomes
    /// arbitrary), so a continuous drag along the SV square at S=0 would
    /// snap H back to 0 every frame without this cache.
    picker_h: f32, // 0..360
    picker_s: f32, // 0..1
    picker_v: f32, // 0..1
    picker_a: f32, // 0..1
}

impl Default for CrosshairUi {
    fn default() -> Self {
        Self {
            drag: None,
            selected_color: CrosshairColorSlot::Idle,
            picker_h: 0.0,
            picker_s: 0.0,
            picker_v: 1.0,
            picker_a: 1.0,
        }
    }
}

/// One slider / picker control in the CROSSHAIR editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrosshairSlider {
    ArmLength,
    ArmGap,
    ArmThickness,
    DotSize,
    OutlineThickness,
    ReachDistance,
    /// 2D Saturation/Value square — cursor x maps to S (0..1), y maps to V (1..0).
    SvSquare,
    /// 1D Hue strip — cursor y maps to H (0..360).
    HueStrip,
    /// 1D Alpha strip — cursor y maps to A (0..1).
    AlphaStrip,
}

/// Which of the four state-colour slots the RGB pane edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum CrosshairColorSlot {
    #[default]
    Idle,
    Target,
    Reach,
    Outline,
}

/// Which PVP-tab slider is being dragged. PvP-tab volume/pitch/distance
/// sliders share a uniform 0..1 -> value mapping; the variant identifies the
/// target field for the per-frame `drag_pvp_slider` update.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PvpDrag {
    TierVolume(usize),
    ZoneMinDist(usize),
    ZoneMaxDist(usize),
    ZoneVolume(usize),
}

/// How close (window px) an edge must come to another widget's edge before the
/// drag snaps to align. Small on purpose — a gentle assist, easy to drag past.
const SNAP_PX: f32 = 6.0;

impl Editor {
    /// Build the editor, loading the persisted layout from `hud.toml`.
    pub fn new() -> Self {
        let (profile_active, profile_list) = read_profiles()
            .unwrap_or_else(|| ("Default".to_string(), vec!["Default".to_string()]));
        let mut e = Editor {
            view: OverlayView::Home,
            layout: HudLayout::load(),
            window: (1.0, 1.0),
            cursor: (0.0, 0.0),
            bounds: [empty_rect(); 17],
            dragging: None,
            slider_drag: None,
            snap_x: None,
            snap_y: None,
            selected: None,
            mods: load_mods(),
            modules: crate::modules::ModuleConfig::load(),
            active_profile: profile_active,
            profiles: profile_list,
            skin_image: load_skin_image("ewo-skin.png"),
            cape_image: load_skin_image("ewo-cape.png"),
            skin_yaw: 0.5,
            skin_drag: None,
            skin_slim: instance_file("ewo-skin-slim").map(|p| p.exists()).unwrap_or(false),
            skin_mtime: skin_png_mtime(),
            pvp: crate::pvp::PvpConfig::load(),
            pvp_drag: None,
            keybinds: load_keybinds(),
            media: crate::media::MediaState::empty(),
            media_service: crate::media::MediaService::start(),
            modules_scroll: 0.0,
            home_toggle_bounds: [empty_rect(); 17],
            module_popover: None,
            media_button_press: None,
            crosshair: crate::crosshair::load(),
            crosshair_ui: CrosshairUi::default(),
        };
        // Drop the picker into sync with the loaded crosshair's idle colour
        // so the SV-square cursor / hue strip / alpha strip all start where
        // the loaded swatch actually sits — not the picker's default state.
        sync_picker_from_slot(&mut e.crosshair_ui, &e.crosshair);
        e
    }

    /// Read-only crosshair config — the JNI layer uses this to answer
    /// `nativeIsCustomCrosshairEnabled` for the Java mixin.
    pub fn crosshair_config(&self) -> &crate::crosshair::CrosshairConfig {
        &self.crosshair
    }

    /// Rects (framebuffer pixels) where the composite step should leave
    /// the un-frosted live game visible. Currently only fires on the
    /// CROSSHAIR view, returning the 3 preview-pane rects — the
    /// composite step re-blits the pre-frost snapshot back into them so
    /// the crosshair previews show the real game at 1:1 (true-to-life
    /// peek). Other views return empty.
    pub fn live_game_cutouts(&self, w: f32, h: f32) -> Vec<Rect> {
        if self.view != OverlayView::Crosshair {
            return Vec::new();
        }
        let layout = crosshair_layout(w, h);
        vec![layout.preview_idle, layout.preview_target, layout.preview_reach]
    }

    /// Human-readable label for the keybind bound to `module_id`, or `None` if
    /// the module is unbound or the keybind file wasn't available at boot. Used
    /// by the MODULES tab to render a small chip on the right of each row.
    fn keybind_label_for(&self, module_id: &str) -> Option<String> {
        let code = self
            .keybinds
            .iter()
            .find(|(id, _)| id == module_id)
            .map(|&(_, c)| c)?;
        if code == 0 {
            None
        } else {
            Some(glfw_key_label(code))
        }
    }

    /// The HUD paint-rate cap, chosen in the settings view.
    pub fn paint_rate(&self) -> crate::HudPaintRate {
        self.layout.paint_rate
    }

    /// Whether the current view wants the live game frosted behind it. The
    /// data views (Mods / Settings) do — for a real glass-over-depth backdrop;
    /// the HUD editor doesn't, so widgets stay readable against the game.
    pub fn frosts_game(&self) -> bool {
        !matches!(self.view, OverlayView::HudEditor)
    }

    /// Switch the active client profile — persist it to `profiles.toml`
    /// and reload the HUD layout from the new profile's `hud.toml`, live.
    fn switch_profile(&mut self, name: String) {
        if name == self.active_profile {
            return;
        }
        write_profiles(&name, &self.profiles);
        self.active_profile = name;
        self.layout = HudLayout::load();
        // Modules are per-profile too — reload them for the switched-to profile.
        self.modules = crate::modules::ModuleConfig::load();
        // Crosshair is per-profile as well — reload `<profile>/crosshair.toml`
        // so the new profile's saved shape + colours apply live; then snap
        // the HSV picker to the freshly-loaded selected swatch so the UI
        // doesn't carry the previous profile's picker position.
        self.crosshair = crate::crosshair::load();
        sync_picker_from_slot(&mut self.crosshair_ui, &self.crosshair);
    }

    /// Cursor moved — drag the active widget if one is held, snapping its
    /// edges/centres to other widgets' for a gentle alignment assist.
    pub fn on_mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        // HOME 3D-skin drag — rotate the model by the horizontal delta.
        if let Some(last_x) = self.skin_drag {
            self.skin_yaw += (x - last_x) * 0.012;
            self.skin_drag = Some(x);
            return;
        }
        // MODULES-view slider drag — track the cursor to the setting value.
        if let Some((idx, slot)) = self.slider_drag {
            self.drag_module_slider(idx, slot, x);
            return;
        }
        // PVP-view slider drag — same idea, but the slot identifies which
        // PVP control is held.
        if let Some(drag) = self.pvp_drag {
            self.drag_pvp_slider(drag, x);
            return;
        }
        // CROSSHAIR-view slider drag — slot identifies which control is
        // held. The 2D SV square uses both coordinates; the 1D sliders
        // ignore `y` (or `x`, for the vertical hue/alpha strips).
        if let Some(slot) = self.crosshair_ui.drag {
            self.drag_crosshair_slider(slot, x, y);
            return;
        }
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

    /// Mouse button — `button` is the GLFW code (`0` = left, `1` = right). A
    /// left press first checks the view-tab strip, then routes to the active
    /// view; a release ends + persists a drag. Right-click opens the MODULES
    /// settings popover (no-op elsewhere for now).
    pub fn on_mouse_button(&mut self, button: i32, pressed: bool, x: f32, y: f32) {
        self.cursor = (x, y);

        // Right-click: settings popover on MODULES rows that have sliders.
        // Press only — releases of the right button do nothing.
        if button == 1 {
            if !pressed {
                return;
            }
            if self.view == OverlayView::Modules {
                let (panel, _, rows, content_h) =
                    modules_layout(self.window.0, self.window.1);
                let body_top = panel.top + 24.0 + 76.0;
                let visible = panel.bottom - body_top - 8.0;
                let max_scroll = (content_h - visible).max(0.0);
                let scroll_y = self.modules_scroll.clamp(0.0, max_scroll);
                let logical_y = y + scroll_y;
                for row in rows.iter() {
                    if !catalog::REGISTRY[row.catalog_index].settings.is_empty()
                        && point_in(row.row, x, logical_y)
                    {
                        self.module_popover = Some(row.catalog_index);
                        return;
                    }
                }
            }
            return;
        }

        // Anything below this point handles the LEFT mouse button.
        if !pressed {
            self.skin_drag = None;
            if self.slider_drag.take().is_some() {
                // A module-slider drag finished — persist the setting now.
                self.modules.save();
            }
            if self.pvp_drag.take().is_some() {
                // A PVP-tab slider drag finished — persist the config.
                self.pvp.save();
            }
            if self.crosshair_ui.drag.take().is_some() {
                // A crosshair-tab slider drag finished — clamp + persist.
                self.crosshair.clamp();
                crate::crosshair::save(&self.crosshair);
            }
            if self.dragging.take().is_some() {
                // A drag finished — drop the snap guides and persist.
                self.snap_x = None;
                self.snap_y = None;
                self.layout.save();
            }
            return;
        }

        // While the module-settings popover is open, all left clicks belong
        // to it: hits inside route to its sliders / close button; hits
        // outside close it and the click is consumed (no fall-through to the
        // view dispatch).
        if let Some(idx) = self.module_popover {
            let layout = module_popover_layout(self.window.0, self.window.1, idx);
            if point_in(layout.card, x, y) {
                // Close button (top-right of the card).
                if point_in(layout.close, x, y) {
                    self.module_popover = None;
                    return;
                }
                // Slider press → start a drag on that setting.
                for (slot, &track) in layout.sliders.iter().enumerate() {
                    if point_in(track, x, y) {
                        self.slider_drag = Some((idx, slot));
                        self.drag_module_slider(idx, slot, x);
                        return;
                    }
                }
                // Click on the popover background — absorb the click so we
                // don't accidentally close on margins inside the card.
                return;
            }
            // Click outside the card — close the popover and absorb.
            self.module_popover = None;
            return;
        }

        // The top-centre view-tab strip takes priority.
        let (_, tabs) = tab_layout(self.window.0);
        for (i, &tab) in tabs.iter().enumerate() {
            if point_in(tab, x, y) {
                self.view = OverlayView::ALL[i];
                // Reset scroll on view switch so re-entering MODULES always
                // starts at the top, not mid-list.
                self.modules_scroll = 0.0;
                self.module_popover = None;
                return;
            }
        }

        match self.view {
            OverlayView::Home => {
                let (_, skin_rect, _, media_rect, _) =
                    home_layout(self.window.0, self.window.1);
                if point_in(skin_rect, x, y) {
                    self.skin_drag = Some(x);
                    return;
                }
                // Media-controller transport buttons (prev / play / next).
                // Geometry must mirror `draw_media_large`'s control layout.
                if point_in(media_rect, x, y) {
                    let ctl_size = 34.0;
                    let play_size = 46.0;
                    let gap = 8.0;
                    let cluster_w = ctl_size + gap + play_size + gap + ctl_size;
                    let cluster_left = media_rect.right - 14.0 - cluster_w;
                    let cy = media_rect.top + media_rect.height() * 0.5;
                    let prev = Rect::from_xywh(
                        cluster_left,
                        cy - ctl_size * 0.5,
                        ctl_size,
                        ctl_size,
                    );
                    let play = Rect::from_xywh(
                        cluster_left + ctl_size + gap,
                        cy - play_size * 0.5,
                        play_size,
                        play_size,
                    );
                    let next = Rect::from_xywh(
                        cluster_left + ctl_size + gap + play_size + gap,
                        cy - ctl_size * 0.5,
                        ctl_size,
                        ctl_size,
                    );
                    if point_in(prev, x, y) {
                        self.media_service.act(crate::media::MediaAction::Previous);
                        self.media_button_press = Some((0, std::time::Instant::now()));
                        return;
                    }
                    if point_in(play, x, y) {
                        self.media_service.act(crate::media::MediaAction::PlayPause);
                        self.media_button_press = Some((1, std::time::Instant::now()));
                        return;
                    }
                    if point_in(next, x, y) {
                        self.media_service.act(crate::media::MediaAction::Next);
                        self.media_button_press = Some((2, std::time::Instant::now()));
                        return;
                    }
                }
                // Chips are flowed at render time; their rects live in
                // `home_toggle_bounds` (populated by draw_home each frame).
                for (i, &tog) in self.home_toggle_bounds.iter().enumerate() {
                    if point_in(tog, x, y) {
                        let wl = self.layout.get_mut(WidgetId::ALL[i]);
                        wl.enabled = !wl.enabled;
                        self.layout.save();
                        return;
                    }
                }
            }
            OverlayView::HudEditor => self.editor_press(x, y),
            OverlayView::Modules => {
                let (panel, _, rows, content_h) = modules_layout(self.window.0, self.window.1);
                // The panel header sits above the scroll region; scroll-aware
                // hit-tests only apply inside the body rect.
                let body_top = panel.top + 24.0 + 76.0;
                let visible = panel.bottom - body_top - 8.0;
                let max_scroll = (content_h - visible).max(0.0);
                let scroll_y = self.modules_scroll.clamp(0.0, max_scroll);
                // Rows are laid out in logical (un-scrolled) coords; shift the
                // click by +scroll_y to match.
                let logical_y = y + scroll_y;
                for row in rows.iter() {
                    let idx = row.catalog_index;
                    if point_in(row.toggle, x, logical_y) {
                        self.modules.toggle(idx);
                        return;
                    }
                    for (slot, &track) in row.sliders.iter().enumerate() {
                        if point_in(track, x, logical_y) {
                            self.slider_drag = Some((idx, slot));
                            self.drag_module_slider(idx, slot, x);
                            return;
                        }
                    }
                }
            }
            OverlayView::Mods => {
                let (_, toggles) = mods_layout(self.window.0, self.window.1, self.mods.len());
                for (i, &toggle) in toggles.iter().enumerate() {
                    if point_in(toggle, x, y) {
                        self.mods[i].enabled = !self.mods[i].enabled;
                        save_mod_overrides(&self.mods);
                        return;
                    }
                }
            }
            OverlayView::Settings => {
                let (_, chips, buttons) =
                    settings_layout(self.window.0, self.window.1, self.profiles.len());
                for (i, &chip) in chips.iter().enumerate() {
                    if point_in(chip, x, y) {
                        if let Some(name) = self.profiles.get(i).cloned() {
                            self.switch_profile(name);
                        }
                        return;
                    }
                }
                for (i, &btn) in buttons.iter().enumerate() {
                    if point_in(btn, x, y) {
                        self.layout.paint_rate = crate::HudPaintRate::ALL[i];
                        self.layout.save();
                        return;
                    }
                }
            }
            OverlayView::Pvp => self.pvp_press(x, y),
            OverlayView::Crosshair => self.crosshair_press(x, y),
        }
    }

    /// Handle a press in the HUD-editor view — the side panel or widget drag.
    fn editor_press(&mut self, x: f32, y: f32) {
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

    /// Track a MODULES-view slider drag: map the cursor `x` to the setting
    /// value and apply it. The value persists on drag-release, not per-move.
    /// Mouse click in the in-world Media widget — called from the Fabric mod
    /// when a vanilla screen (inventory, pause, chat, …) is open and the user
    /// left-clicks anywhere. Returns `true` if the click hit one of the
    /// widget's prev / play / next transport buttons and an action was
    /// dispatched, so the caller can cancel the press to keep the vanilla
    /// screen from also receiving it.
    ///
    /// Geometry must match `draw_media_compact`'s control cluster one-to-one;
    /// the widget's overall bounds are read from `editor.bounds`, which the
    /// renderer fills every frame.
    pub fn try_media_click(&mut self, button: i32, x: f32, y: f32) -> bool {
        // Only left-click triggers transport actions.
        if button != 0 {
            return false;
        }
        // The widget must actually be drawn this frame.
        let wl = self.layout.get(WidgetId::Media);
        if !wl.enabled {
            return false;
        }
        let bounds = self.bounds[WidgetId::Media.index()];
        if bounds.width() <= 0.0 || !point_in(bounds, x, y) {
            return false;
        }

        // Match `draw_media_compact` — three 22px circles, the rightmost
        // cluster of the widget, with 4px gaps and a 12px right pad.
        let btn = 22.0;
        let btn_gap = 4.0;
        let pad_r = 12.0;
        let cluster_w = btn * 3.0 + btn_gap * 2.0;
        let cluster_left = bounds.right - pad_r - cluster_w;
        let cy = bounds.top + bounds.height() * 0.5;
        let prev = Rect::from_xywh(cluster_left, cy - btn * 0.5, btn, btn);
        let play = Rect::from_xywh(cluster_left + btn + btn_gap, cy - btn * 0.5, btn, btn);
        let next = Rect::from_xywh(
            cluster_left + (btn + btn_gap) * 2.0,
            cy - btn * 0.5,
            btn,
            btn,
        );
        if point_in(prev, x, y) {
            self.media_service.act(crate::media::MediaAction::Previous);
            self.media_button_press = Some((0, std::time::Instant::now()));
            return true;
        }
        if point_in(play, x, y) {
            self.media_service.act(crate::media::MediaAction::PlayPause);
            self.media_button_press = Some((1, std::time::Instant::now()));
            return true;
        }
        if point_in(next, x, y) {
            self.media_service.act(crate::media::MediaAction::Next);
            self.media_button_press = Some((2, std::time::Instant::now()));
            return true;
        }
        false
    }

    /// Scroll wheel — currently only the MODULES tab consumes it. Each "notch"
    /// from `nativeMouseScroll` is roughly ±1; we want ~one row per notch so
    /// the list scrolls predictably.
    pub fn on_scroll(&mut self, dy: f32) {
        if self.view != OverlayView::Modules {
            return;
        }
        const PIXELS_PER_NOTCH: f32 = 70.0;
        self.modules_scroll = (self.modules_scroll - dy * PIXELS_PER_NOTCH).max(0.0);
        // The renderer clamps to max_scroll for us each frame; no need to
        // recompute the layout here for the upper bound.
    }

    fn drag_module_slider(&mut self, idx: usize, slot: usize, x: f32) {
        // Sliders only live in the popover now; if it isn't the one we're
        // dragging (or it's not open), there's nothing to update.
        let Some(open_idx) = self.module_popover else {
            return;
        };
        if open_idx != idx {
            return;
        }
        let layout = module_popover_layout(self.window.0, self.window.1, idx);
        let Some(track) = layout.sliders.get(slot).copied() else {
            return;
        };
        let Some(setting) = catalog::REGISTRY.get(idx).and_then(|m| m.settings.get(slot))
        else {
            return;
        };
        // Sliders have a small horizontal margin inside the track rect — use
        // the inner span (matches `draw_module_slider`'s knob travel).
        let inner_left = track.left + 14.0;
        let inner_right = track.right - 54.0;
        let span = (inner_right - inner_left).max(1.0);
        let frac = ((x - inner_left) / span).clamp(0.0, 1.0);
        let mut value = setting.min + frac * (setting.max - setting.min);
        if setting.step > 0.0 {
            value = (value / setting.step).round() * setting.step;
        }
        self.modules.set_setting(idx, slot, value);
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

    /// PVP tab — a press cycles a sound chip, flips a toggle, or starts a
    /// slider drag. Edits are persisted to `pvp.toml` on commit (toggle / chip
    /// click immediately; slider drag on release in `on_mouse_button`).
    fn pvp_press(&mut self, x: f32, y: f32) {
        let layout = pvp_layout(self.window.0, self.window.1);

        // General-section toggles.
        for (i, &rect) in layout.general_toggles.iter().enumerate() {
            if point_in(rect, x, y) {
                match i {
                    0 => self.pvp.jump_reset_enabled = !self.pvp.jump_reset_enabled,
                    1 => self.pvp.jump_reset_bar_enabled = !self.pvp.jump_reset_bar_enabled,
                    2 => self.pvp.hit_range_enabled = !self.pvp.hit_range_enabled,
                    3 => self.pvp.totem_count_enabled = !self.pvp.totem_count_enabled,
                    4 => self.pvp.floating_health_enabled = !self.pvp.floating_health_enabled,
                    _ => {}
                }
                self.pvp.save();
                return;
            }
        }

        // Sound-cycle chips for each tier.
        for (i, &rect) in layout.tier_sound.iter().enumerate() {
            if point_in(rect, x, y) {
                let tier = crate::pvp::Tier::ALL[i];
                let slot = self.pvp.sound_for_tier_mut(tier);
                let next = (slot.sound.index() + 1) % crate::pvp::PvpSound::ALL.len();
                slot.sound = crate::pvp::PvpSound::ALL[next];
                self.pvp.save();
                return;
            }
        }

        // Tier volume sliders.
        for (i, &rect) in layout.tier_volume.iter().enumerate() {
            if point_in(rect, x, y) {
                self.pvp_drag = Some(PvpDrag::TierVolume(i));
                self.drag_pvp_slider(PvpDrag::TierVolume(i), x);
                return;
            }
        }

        // Zone enable toggles + min/max sliders + sound chips + volume sliders.
        for i in 0..3 {
            if point_in(layout.zone_enable[i], x, y) {
                let z = self.pvp.zone_mut(i);
                z.enabled = !z.enabled;
                self.pvp.save();
                return;
            }
            if point_in(layout.zone_min[i], x, y) {
                self.pvp_drag = Some(PvpDrag::ZoneMinDist(i));
                self.drag_pvp_slider(PvpDrag::ZoneMinDist(i), x);
                return;
            }
            if point_in(layout.zone_max[i], x, y) {
                self.pvp_drag = Some(PvpDrag::ZoneMaxDist(i));
                self.drag_pvp_slider(PvpDrag::ZoneMaxDist(i), x);
                return;
            }
            if point_in(layout.zone_sound[i], x, y) {
                let z = self.pvp.zone_mut(i);
                let next = (z.sound.index() + 1) % crate::pvp::PvpSound::ALL.len();
                z.sound = crate::pvp::PvpSound::ALL[next];
                self.pvp.save();
                return;
            }
            if point_in(layout.zone_volume[i], x, y) {
                self.pvp_drag = Some(PvpDrag::ZoneVolume(i));
                self.drag_pvp_slider(PvpDrag::ZoneVolume(i), x);
                return;
            }
        }
    }

    /// Track a PVP slider drag — the slot identifies which control is held
    /// (the cursor x maps to its value). Persists on drag-release, not
    /// per-frame, so dragging doesn't write `pvp.toml` 60 times a second.
    fn drag_pvp_slider(&mut self, slot: PvpDrag, x: f32) {
        let layout = pvp_layout(self.window.0, self.window.1);
        let frac_of = |track: Rect| -> f32 {
            let span = (track.right - track.left - 44.0).max(1.0); // value strip
            ((x - track.left - 4.0) / span).clamp(0.0, 1.0)
        };
        match slot {
            PvpDrag::TierVolume(i) => {
                let frac = frac_of(layout.tier_volume[i]);
                let tier = crate::pvp::Tier::ALL[i];
                self.pvp.sound_for_tier_mut(tier).volume = frac;
            }
            PvpDrag::ZoneMinDist(i) => {
                let frac = frac_of(layout.zone_min[i]);
                let z = self.pvp.zone_mut(i);
                z.min_dist = (frac * 3.5).max(0.0).min(z.max_dist - 0.05);
            }
            PvpDrag::ZoneMaxDist(i) => {
                let frac = frac_of(layout.zone_max[i]);
                let z = self.pvp.zone_mut(i);
                z.max_dist = (frac * 3.5).max(z.min_dist + 0.05).min(3.5);
            }
            PvpDrag::ZoneVolume(i) => {
                let frac = frac_of(layout.zone_volume[i]);
                self.pvp.zone_mut(i).volume = frac;
            }
        }
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

    // Drain any pending SMTC snapshots into the live media state. The poll is
    // non-blocking — if no update is queued this returns immediately. Split
    // the borrow over disjoint fields so both can be `&mut`.
    {
        let media_ref = &mut editor.media;
        let service_ref = &mut editor.media_service;
        service_ref.poll(media_ref);
    }

    // While the overlay is up and the user is on a non-editor tab (HOME /
    // MODULES / MODS / SETTINGS / PVP), hide the in-world widgets so they
    // don't bleed through the overlay panels' translucent chrome. The HUD
    // editor tab still needs them visible — that's where the user is
    // positioning them.
    let show_widgets = !data.overlay_open() || editor.view == OverlayView::HudEditor;

    for id in WidgetId::ALL {
        let wl = editor.layout.get(id);
        let bounds = if show_widgets && wl.enabled && widget_available(id, data) {
            // Media is dispatched separately — `draw_widget` only has
            // `&HudData` to work with, while the media state lives on Editor.
            match id {
                WidgetId::Media => draw_media_compact(
                    canvas,
                    &editor.media,
                    editor.cursor,
                    editor.media_button_press,
                    fonts,
                    wl.anchor,
                    wl.x * w,
                    wl.y * h,
                ),
                _ => draw_widget(canvas, id, data, fonts, wl.anchor, wl.x * w, wl.y * h),
            }
        } else {
            empty_rect()
        };
        editor.bounds[id.index()] = bounds;
    }

    // Custom crosshair — replaces the vanilla one when enabled. Always
    // drawn at the framebuffer's centre (the Java mixin suppresses
    // vanilla's `Gui.extractCrosshair` when `editor.crosshair.enabled` is
    // true). Three-state colour tracks the entity under the crosshair —
    // idle / target out of reach / target in reach. Skipped on non-editor
    // overlay tabs along with the rest of the in-world HUD.
    if show_widgets && editor.crosshair.enabled {
        let state = editor.crosshair.state_from_target(
            data.target_active(),
            data.target_distance(),
        );
        crate::crosshair::draw(canvas, w * 0.5, h * 0.5, &editor.crosshair, state);
    }

    // Crosshair on Reach module — the legacy fallback that overlays a rose
    // "+" on the vanilla crosshair when an entity under it is within attack
    // reach. Only fires when the custom crosshair is OFF (otherwise the
    // custom crosshair's reach-colour subsumes the same behaviour). Skipped
    // on non-editor overlay tabs.
    if show_widgets && !editor.crosshair.enabled {
        if let Some(idx) = catalog::index_of("crosshair_on_reach") {
            let st = editor.modules.get(idx);
            if st.enabled && data.target_active() && data.target_distance() <= st.settings[0] {
                draw_crosshair_on_reach(canvas, w, h);
            }
        }
    }

    // Hit Indicator module — screen-edge chevron pointing back toward the
    // most recent attacker. Skipped on non-editor overlay tabs (it'd flicker
    // behind the dashboard).
    if show_widgets {
        if let Some(idx) = catalog::index_of("hit_indicator") {
            let st = editor.modules.get(idx);
            if st.enabled && data.hit_present() {
                let radius_pct = st.settings[0].max(5.0).min(50.0);
                let fade_secs = st.settings[1].max(0.1);
                let age = data.hit_age();
                if age >= 0.0 && age < fade_secs {
                    draw_hit_indicator(
                        canvas,
                        w,
                        h,
                        data.hit_relative_yaw(),
                        age,
                        fade_secs,
                        radius_pct,
                    );
                }
            }
        }
    }

    // World-anchored combat indicators — overhead totem-pop counter and
    // floating health/damage on visible LivingEntities. Hidden on non-editor
    // overlay tabs along with the rest of the HUD.
    let totem_on = show_widgets && editor.pvp.totem_count_enabled;
    let health_on = show_widgets && editor.pvp.floating_health_enabled;
    if totem_on || health_on {
        for i in 0..data.indicator_count() {
            let ind = data.indicator(i);
            if !ind.in_view {
                continue;
            }
            if health_on {
                draw_floating_health(canvas, &ind, fonts);
            }
            if totem_on && ind.totem_count > 0 {
                draw_totem_overhead(canvas, &ind, fonts);
            }
        }
    }

    if !data.overlay_open() {
        return;
    }

    // Dim the scene — the overlay is a focused mode.
    let mut tint = Paint::default();
    tint.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.22), None);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, h), &tint);

    // Pick up the skin PNGs once the mod has written them — the export
    // finishes after the Editor was constructed, and may replace a stale png
    // from an earlier launch. Reload when the file's mtime moves, or keep
    // retrying while we have no image but the file exists (a partial write).
    if editor.view == OverlayView::Home {
        let disk_mtime = skin_png_mtime();
        if disk_mtime != editor.skin_mtime
            || (editor.skin_image.is_none() && disk_mtime.is_some())
        {
            editor.skin_mtime = disk_mtime;
            editor.skin_image = load_skin_image("ewo-skin.png");
            editor.cape_image = load_skin_image("ewo-cape.png");
            editor.skin_slim =
                instance_file("ewo-skin-slim").map(|p| p.exists()).unwrap_or(false);
        }
    }

    // The active dashboard view.
    match editor.view {
        OverlayView::Home => draw_home(canvas, editor, data, fonts, w, h),
        OverlayView::HudEditor => draw_editor(canvas, editor, fonts, w, h),
        OverlayView::Crosshair => draw_crosshair_view(canvas, editor, fonts, w, h),
        OverlayView::Modules => draw_modules(canvas, editor, fonts, w, h),
        OverlayView::Pvp => draw_pvp(canvas, editor, fonts, w, h),
        OverlayView::Mods => draw_mods(canvas, editor, fonts, w, h),
        OverlayView::Settings => draw_settings(canvas, editor, fonts, w, h),
    }

    // Module settings popover — opened by right-click on a MODULES row that
    // carries sliders. Drawn over the active view but below the tab strip so
    // the user can still see what tab they're on.
    if editor.view == OverlayView::Modules {
        if let Some(idx) = editor.module_popover {
            draw_module_popover(canvas, editor, idx, fonts, w, h);
        }
    }

    // The view-tab strip + a close hint, on top of the view.
    draw_tab_strip(canvas, editor.view, fonts, w);

    let hint_font = fonts.jetbrains_mono(12.0);
    let hint = match editor.view {
        OverlayView::HudEditor => "DRAG WIDGETS OR USE THE PANEL  ·  RIGHT SHIFT OR ESC TO CLOSE",
        OverlayView::Crosshair => "TOGGLE TO REPLACE VANILLA  ·  RIGHT SHIFT OR ESC TO CLOSE",
        OverlayView::Home
        | OverlayView::Modules
        | OverlayView::Pvp
        | OverlayView::Mods
        | OverlayView::Settings => "RIGHT SHIFT OR ESC TO CLOSE",
    };
    let hint_w = measure_tracked_em(&hint_font, hint, 0.14);
    let mut hint_paint = Paint::default();
    hint_paint.set_anti_alias(true);
    hint_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        hint,
        ((w - hint_w) * 0.5, h - 28.0),
        &hint_font,
        &hint_paint,
        0.14,
    );
}

/// Whether `id`'s underlying data is present this frame. PvP widgets show
/// while the overlay editor is open (so they can be placed even without a
/// recent result) or while a real result is live.
fn widget_available(id: WidgetId, data: &HudData) -> bool {
    match id {
        WidgetId::Fps => true,
        WidgetId::Ping => data.ping_valid(),
        WidgetId::Coords | WidgetId::Keystrokes => data.world_active(),
        WidgetId::Armor => data.world_active() && data.armor_active(),
        WidgetId::Potions => data.world_active() && data.potion_count() > 0,
        WidgetId::Target => data.world_active() && data.target_active(),
        WidgetId::JumpResetText | WidgetId::JumpResetBar => {
            data.pvp_jump_active() || data.overlay_open()
        }
        WidgetId::HitRange => data.pvp_hit_active() || data.overlay_open(),
        WidgetId::Cps => data.world_active() || data.overlay_open(),
        WidgetId::Items => data.world_active() || data.overlay_open(),
        WidgetId::ShieldCooldown => {
            data.world_active() && (data.shield_cooldown() > 0.0 || data.overlay_open())
        }
        WidgetId::Reach => data.world_active() && (data.target_active() || data.overlay_open()),
        WidgetId::AttackCharge => data.world_active() || data.overlay_open(),
        WidgetId::Combo => data.world_active() && (data.combo_count() > 0 || data.overlay_open()),
        // Media widget is always available — it carries its own empty state
        // when nothing is playing, and the user should be able to position it
        // in the HUD editor regardless of SMTC connection.
        WidgetId::Media => true,
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
        WidgetId::JumpResetText => draw_jump_reset_text(canvas, data, fonts, anchor, ax, ay),
        WidgetId::JumpResetBar => draw_jump_reset_bar(canvas, data, anchor, ax, ay),
        WidgetId::HitRange => draw_hit_range(canvas, data, fonts, anchor, ax, ay),
        WidgetId::Cps => draw_cps(canvas, data.cps_left(), data.cps_right(), fonts, anchor, ax, ay),
        WidgetId::Items => draw_item_counters(
            canvas,
            data.item_pearls(),
            data.item_arrows(),
            data.item_totems(),
            data.item_gapples(),
            fonts,
            anchor,
            ax,
            ay,
        ),
        WidgetId::ShieldCooldown => {
            draw_shield_cooldown(canvas, data.shield_cooldown(), fonts, anchor, ax, ay)
        }
        WidgetId::Reach => draw_stat(
            canvas,
            &format!("{:.2}", data.target_distance().max(0.0)),
            "REACH",
            fonts,
            anchor,
            ax,
            ay,
        ),
        WidgetId::AttackCharge => {
            draw_attack_charge(canvas, data.attack_charge(), fonts, anchor, ax, ay)
        }
        WidgetId::Combo => draw_combo(
            canvas,
            data.combo_count(),
            data.combo_age(),
            fonts,
            anchor,
            ax,
            ay,
        ),
        // Media is dispatched separately by the caller — it needs the
        // `MediaState` (carried on `Editor`, not `HudData`), which would
        // bloat this function's signature.
        WidgetId::Media => empty_rect(),
    }
}

/// Compact in-world media controller — the small floating widget. Renders the
/// design's `.w-media` form: 36×36 thumbnail + italic title + tracked artist +
/// three tiny circular transport buttons. Falls back to the empty state when
/// nothing is playing.
fn draw_media_compact(
    canvas: &Canvas,
    media: &crate::media::MediaState,
    cursor: (f32, f32),
    press_info: Option<(usize, std::time::Instant)>,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let idle = media.is_idle();
    let has_duration = !idle && media.duration_seconds > 0.0;
    let thumb = 44.0;
    let pad_l = 10.0;
    let pad_y = 10.0;
    let pad_r = 12.0;
    let gap = 10.0;
    let btn = 22.0;
    let btn_gap = 4.0;
    let cluster_w = btn * 3.0 + btn_gap * 2.0;

    // Title font + measurement — the chip is sized to fit the text.
    let title_font = fonts.fraunces_axes(13.0, 100.0, 1.0, 520.0, Some(24.0));
    let artist_font = fonts.jetbrains_mono(9.5);
    let time_font = fonts.jetbrains_mono(9.0);
    let mut probe = Paint::default();
    probe.set_anti_alias(true);

    let title_str = if idle {
        "Nothing playing".to_string()
    } else {
        media.title.clone()
    };
    let artist_str = if idle {
        "— connect a source —".to_string()
    } else {
        media.artist.clone()
    };
    let (title_w, _) = title_font.measure_str(&title_str, Some(&probe));
    let artist_w = measure_tracked_em(&artist_font, &artist_str, 0.18);
    let text_w = title_w.max(artist_w).min(180.0).max(140.0);

    let chip_w = pad_l + thumb + gap + text_w + gap + cluster_w + pad_r;
    let chip_h = pad_y * 2.0 + thumb;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, 14.0);

    // Thumbnail (44×44, 8px rounded).
    let thumb_rect = Rect::from_xywh(x + pad_l, y + pad_y, thumb, thumb);
    let thumb_rr = RRect::new_rect_xy(thumb_rect, 8.0, 8.0);
    if let Some(img) = media.thumbnail.as_ref() {
        canvas.save();
        canvas.clip_rrect(thumb_rr, Some(ClipOp::Intersect), Some(true));
        let mut p = Paint::default();
        p.set_anti_alias(true);
        canvas.draw_image_rect(img, None, thumb_rect, &p);
        canvas.restore();
    } else {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_color4f(rgba(WINE, 0.85), None);
        canvas.draw_rrect(thumb_rr, &p);
        // Music glyph centred when idle.
        let g_font = fonts.fraunces_axes(22.0, 100.0, 1.0, 500.0, Some(36.0));
        let mut g = Paint::default();
        g.set_anti_alias(true);
        g.set_color4f(rgba(MAUVE, 0.85), None);
        let glyph = "\u{266A}";
        let (gw, _) = g_font.measure_str(glyph, Some(&probe));
        let (_, gm) = g_font.metrics();
        let gcap = if gm.cap_height > 0.0 { gm.cap_height } else { 14.0 };
        canvas.draw_str(
            glyph,
            (thumb_rect.left + (thumb - gw) * 0.5, thumb_rect.top + (thumb + gcap) * 0.5),
            &g_font,
            &g,
        );
    }

    // Text column: title (top), artist (middle), scrub bar + timestamps (bottom)
    // — three rows aligned with the thumbnail height.
    let text_left = thumb_rect.right + gap;
    let title_baseline = y + pad_y + 12.0;
    let artist_baseline = title_baseline + 14.0;
    let scrub_y = thumb_rect.bottom - 6.0;

    canvas.save();
    let text_clip = Rect::new(text_left, y, text_left + text_w, y + chip_h);
    canvas.clip_rect(text_clip, Some(ClipOp::Intersect), Some(true));

    // Marquee scroll — if the title is wider than the visible column, slide
    // it left over time. `MediaState::marquee_offset` does the math; we just
    // translate the title's draw x by `-offset` (the clip rect cuts the
    // overhang on both sides).
    let marquee = media.marquee_offset(title_w, text_w);
    let title_x = text_left - marquee;

    draw_iw_text_shadow(canvas, &title_str, (title_x, title_baseline), &title_font);
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color4f(
        if idle {
            rgba(MAUVE, 1.0)
        } else {
            rgba(PEARL, 1.0)
        },
        None,
    );
    canvas.draw_str(&title_str, (title_x, title_baseline), &title_font, &title_paint);

    draw_iw_text_shadow_tracked(canvas, &artist_str, (text_left, artist_baseline), &artist_font, 0.18);
    let mut artist_paint = Paint::default();
    artist_paint.set_anti_alias(true);
    artist_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        &artist_str,
        (text_left, artist_baseline),
        &artist_font,
        &artist_paint,
        0.18,
    );
    canvas.restore();

    // ── Scrub row — current "m:ss" + thin track with rose→champ fill +
    //    duration "m:ss". Only when we have a real duration; the empty state
    //    leaves the row blank so it doesn't lie. ─────────────────────────────
    if has_duration {
        let live_pos = media.displayed_position();
        let pos = fmt_track_time(live_pos);
        let dur = fmt_track_time(media.duration_seconds);
        let (pos_w, _) = time_font.measure_str(&pos, Some(&probe));
        let (dur_w, _) = time_font.measure_str(&dur, Some(&probe));

        // Position timestamp on the left of the bar.
        let mut tp = Paint::default();
        tp.set_anti_alias(true);
        tp.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(&pos, (text_left, scrub_y + 4.0), &time_font, &tp);

        // Duration timestamp on the right.
        let dur_x = text_left + text_w - dur_w;
        canvas.draw_str(&dur, (dur_x, scrub_y + 4.0), &time_font, &tp);

        // The bar itself — pearl track + rose→champ fill + tiny pearl knob.
        let bar_left = text_left + pos_w + 6.0;
        let bar_right = dur_x - 6.0;
        let bar_h = 2.0;
        let bar_rect = Rect::from_xywh(
            bar_left,
            scrub_y - bar_h * 0.5,
            (bar_right - bar_left).max(0.0),
            bar_h,
        );
        let bar_rr = RRect::new_rect_xy(bar_rect, bar_h, bar_h);
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(rgba(PEARL, 0.12), None);
        canvas.draw_rrect(bar_rr, &bg);

        let frac = (live_pos / media.duration_seconds).clamp(0.0, 1.0);
        let fill_w = bar_rect.width() * frac;
        if fill_w > 1.0 {
            let fill_rect = Rect::from_xywh(bar_rect.left, bar_rect.top, fill_w, bar_h);
            let mut fp = Paint::default();
            fp.set_anti_alias(true);
            if let Some(shader) = gradient_shader::linear(
                (
                    Point::new(bar_rect.left, bar_rect.top),
                    Point::new(bar_rect.right, bar_rect.top),
                ),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[rgba(ROSE, 1.0), rgba(CHAMP, 1.0)],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fp.set_shader(shader);
            } else {
                fp.set_color4f(rgba(ROSE, 1.0), None);
            }
            canvas.draw_rrect(RRect::new_rect_xy(fill_rect, bar_h, bar_h), &fp);

            // Small pearl knob at the current position.
            let knob_x = bar_rect.left + fill_w;
            let knob_y = bar_rect.top + bar_h * 0.5;
            let mut knob = Paint::default();
            knob.set_anti_alias(true);
            knob.set_color4f(rgba(PEARL, 1.0), None);
            canvas.draw_circle((knob_x, knob_y), 3.0, &knob);
        }
    }

    // Transport buttons — tiny (22×22) circles, the middle one rose-filled.
    let cluster_left = x + chip_w - pad_r - cluster_w;
    let cy = y + chip_h * 0.5;
    let prev = Rect::from_xywh(cluster_left, cy - btn * 0.5, btn, btn);
    let play = Rect::from_xywh(cluster_left + btn + btn_gap, cy - btn * 0.5, btn, btn);
    let next = Rect::from_xywh(
        cluster_left + (btn + btn_gap) * 2.0,
        cy - btn * 0.5,
        btn,
        btn,
    );

    let playing = media.playing && !idle;
    let (hover_prev, press_prev) = button_state(prev, cursor, 0, press_info);
    let (hover_play, press_play) = button_state(play, cursor, 1, press_info);
    let (hover_next, press_next) = button_state(next, cursor, 2, press_info);
    draw_media_button(canvas, prev, false, false, '<', hover_prev, press_prev);
    draw_media_button(
        canvas,
        play,
        !idle,
        playing,
        if playing { 'p' } else { 'P' },
        hover_play,
        press_play,
    );
    draw_media_button(canvas, next, false, false, '>', hover_next, press_next);

    chip
}

/// Compute `(hovered, press_strength)` for a media transport button.
/// `press_strength` is `1.0` at the moment of click and eases down to `0.0`
/// over `PRESS_FADE_MS`; the button render scales + brightens proportionally.
fn button_state(
    rect: Rect,
    cursor: (f32, f32),
    index: usize,
    press: Option<(usize, std::time::Instant)>,
) -> (bool, f32) {
    const PRESS_FADE_MS: f32 = 160.0;
    let hovered = point_in(rect, cursor.0, cursor.1);
    let strength = press
        .and_then(|(i, when)| {
            if i != index {
                return None;
            }
            let elapsed_ms = when.elapsed().as_secs_f32() * 1000.0;
            if elapsed_ms >= PRESS_FADE_MS {
                None
            } else {
                Some(1.0 - elapsed_ms / PRESS_FADE_MS)
            }
        })
        .unwrap_or(0.0);
    (hovered, strength)
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

/// In-world widget shell — the canonical glass plate every floating widget sits
/// on. Calibrated to read on ANY backdrop (bright snow + pitch caves + lava) via
/// a layered chrome stack:
///
///   1. Drop shadow                  — lifts the widget off the world
///   2. Outer dark wine ring         — separates from bright backgrounds
///   3. Translucent wine fill        — lets the game show through
///   4. Inset wine ring              — soft inner edge
///   5. Inset top pearl highlight    — top-edge gloss (1px strip)
///   6. Pearl border                 — crisp outer edge
///
/// This is the design's `.iw-shell` recipe (Velvet & Pearl redesign), with the
/// fill bumped from CSS `0.32` to `0.50` to compensate for the lack of
/// `backdrop-filter: blur(8px)` — we paint to an offscreen surface, so the
/// game pixels aren't available to blur until composite time.
fn draw_iw_shell(canvas: &Canvas, rect: Rect, radius: f32) {
    // (1) Drop shadow.
    let shadow_rect = rect.with_offset((0.0, 6.0));
    let shadow_rr = RRect::new_rect_xy(shadow_rect, radius, radius);
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 8.0, false));
    canvas.draw_rrect(shadow_rr, &shadow);

    // (2) Outer dark wine ring — 1px stroke 1px outside the shell.
    let outer = Rect::new(rect.left - 1.0, rect.top - 1.0, rect.right + 1.0, rect.bottom + 1.0);
    let outer_rr = RRect::new_rect_xy(outer, radius + 1.0, radius + 1.0);
    let mut outer_paint = Paint::default();
    outer_paint.set_anti_alias(true);
    outer_paint.set_style(PaintStyle::Stroke);
    outer_paint.set_stroke_width(1.0);
    outer_paint.set_color4f(rgba(WINE, 0.55), None);
    canvas.draw_rrect(outer_rr, &outer_paint);

    let rrect = RRect::new_rect_xy(rect, radius, radius);

    // (3) Fill.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.50), None);
    canvas.draw_rrect(rrect, &fill);

    // (4) Inset wine ring — 1px stroke 1px inside the shell.
    let inset_wine = Rect::new(rect.left + 1.0, rect.top + 1.0, rect.right - 1.0, rect.bottom - 1.0);
    let inset_r = (radius - 1.0).max(0.0);
    let mut wine_paint = Paint::default();
    wine_paint.set_anti_alias(true);
    wine_paint.set_style(PaintStyle::Stroke);
    wine_paint.set_stroke_width(1.0);
    wine_paint.set_color4f(rgba(WINE, 0.25), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset_wine, inset_r, inset_r), &wine_paint);

    // (5) Inset top pearl highlight — clip to the top 2px and stroke the rrect.
    canvas.save();
    let top_clip = Rect::from_xywh(rect.left, rect.top, rect.width(), 2.0);
    canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
    let inset_top = Rect::new(rect.left + 0.5, rect.top + 0.5, rect.right - 0.5, rect.bottom - 0.5);
    let top_r = (radius - 0.5).max(0.0);
    let mut top_paint = Paint::default();
    top_paint.set_anti_alias(true);
    top_paint.set_style(PaintStyle::Stroke);
    top_paint.set_stroke_width(1.0);
    top_paint.set_color4f(rgba(PEARL, 0.10), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset_top, top_r, top_r), &top_paint);
    canvas.restore();

    // (6) Pearl border — outermost edge.
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(PEARL, 0.18), None);
    canvas.draw_rrect(rrect, &border);
}

/// Three-stop dark velvet text shadow — the in-world legibility recipe. Draw
/// this *before* the actual text. Three blurred wine layers stack into a halo
/// that holds glyphs against snow, lava, and dark caves alike.
///
/// Caller draws the final text in their chosen color on top.
fn draw_iw_text_shadow(canvas: &Canvas, text: &str, pos: (f32, f32), font: &Font) {
    let (x, y) = pos;

    // Wider halo — sigma 5 (CSS blur 10px).
    let mut wide = Paint::default();
    wide.set_anti_alias(true);
    wide.set_color4f(rgba(WINE, 0.55), None);
    wide.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 5.0, false));
    canvas.draw_str(text, (x, y), font, &wide);

    // Tight halo — sigma 3 (CSS blur 6px).
    let mut tight = Paint::default();
    tight.set_anti_alias(true);
    tight.set_color4f(rgba(WINE, 0.85), None);
    tight.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    canvas.draw_str(text, (x, y), font, &tight);

    // Hard underline — no blur, 1px down.
    let mut hard = Paint::default();
    hard.set_anti_alias(true);
    hard.set_color4f(rgba(WINE, 0.95), None);
    canvas.draw_str(text, (x, y + 1.0), font, &hard);
}

/// Tracked-em variant of `draw_iw_text_shadow` — for letter-spaced labels
/// (eyebrows, units) drawn with `draw_tracked_em`.
fn draw_iw_text_shadow_tracked(
    canvas: &Canvas,
    text: &str,
    pos: (f32, f32),
    font: &Font,
    tracking_em: f32,
) {
    let (x, y) = pos;

    let mut wide = Paint::default();
    wide.set_anti_alias(true);
    wide.set_color4f(rgba(WINE, 0.55), None);
    wide.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 5.0, false));
    draw_tracked_em(canvas, text, (x, y), font, &wide, tracking_em);

    let mut tight = Paint::default();
    tight.set_anti_alias(true);
    tight.set_color4f(rgba(WINE, 0.85), None);
    tight.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    draw_tracked_em(canvas, text, (x, y), font, &tight, tracking_em);

    let mut hard = Paint::default();
    hard.set_anti_alias(true);
    hard.set_color4f(rgba(WINE, 0.95), None);
    draw_tracked_em(canvas, text, (x, y + 1.0), font, &hard, tracking_em);
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
    draw_iw_shell(canvas, chip, radius);

    // Number cap-top at `y + pad_y`; the unit shares the baseline (CSS
    // `align-items: baseline`).
    let baseline_y = y + pad_y + cap;
    let num_x = x + pad_x;
    let unit_x = num_x + num_w + gap;

    // Number — Fraunces, with the in-world 3-stop dark velvet shadow stack so
    // glyphs hold against snow, lava, and dark caves alike.
    draw_iw_text_shadow(canvas, value, (num_x, baseline_y), &num_font);
    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(value, (num_x, baseline_y), &num_font, &num_paint);

    // Unit — tracked JetBrains Mono eyebrow, rose to mirror the design's
    // `.w-fps .lbl` accent color.
    draw_iw_text_shadow_tracked(
        canvas,
        unit,
        (unit_x, baseline_y),
        &unit_font,
        unit_tracking_em,
    );
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(ROSE, 1.0), None);
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
    // Fraunces 18 SOFT 30 opsz 36 — design's `.w-coords .v` calibration.
    let value_font = fonts.fraunces_axes(18.0, 30.0, 0.0, 500.0, Some(36.0));
    let label_tracking_em = 0.22; // CSS `letter-spacing: .22em` per the redesign.

    let pad_x = 14.0;
    let pad_y = 8.0;
    let gap = 14.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let label_w = measure_tracked_em(&label_font, label, label_tracking_em);
    let (value_w, _) = value_font.measure_str(&value, Some(&probe));

    let (_, m) = value_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 18.0 * 0.72 };
    let chip_w = pad_x * 2.0 + label_w + gap + value_w;
    let chip_h = pad_y * 2.0 + cap;
    let (cx, cy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(cx, cy, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = cy + pad_y + cap;

    // "XYZ" — tracked rose label with the in-world shadow stack.
    draw_iw_text_shadow_tracked(
        canvas,
        label,
        (cx + pad_x, baseline_y),
        &label_font,
        label_tracking_em,
    );
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

    // Position — pearl Fraunces with the in-world shadow stack.
    let value_pos = (cx + pad_x + label_w + gap, baseline_y);
    draw_iw_text_shadow(canvas, &value, value_pos, &value_font);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    value_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&value, value_pos, &value_font, &value_paint);

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
    draw_iw_shell(canvas, chip, 12.0);

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
        draw_iw_text_shadow(canvas, &pct, (px, py), &pct_font);
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

    // Design `.w-potions .nm` — Fraunces 13 SOFT 60. Time — tracked rose mono 10.
    let name_font = fonts.fraunces_axes(13.0, 60.0, 0.0, 500.0, Some(24.0));
    let time_font = fonts.jetbrains_mono(10.0);
    let time_tracking_em = 0.18;

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
        let tw = measure_tracked_em(&time_font, &time_line, time_tracking_em);
        text_w = text_w.max(nw).max(tw);
        rows.push((name_line, time_line, p.color));
    }

    let chip_w = PAD * 2.0 + ICON + ICON_GAP + text_w;
    let chip_h = PAD * 2.0 + count as f32 * ICON + (count as f32 - 1.0) * ROW_GAP;
    let (ox, oy) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(ox, oy, chip_w, chip_h);
    draw_iw_shell(canvas, chip, 12.0);

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

        // Name — pearl Fraunces with the in-world shadow stack.
        let name_pos = (text_x, ry + 14.0);
        draw_iw_text_shadow(canvas, name, name_pos, &name_font);
        let mut name_paint = Paint::default();
        name_paint.set_anti_alias(true);
        name_paint.set_color4f(rgba(PEARL, 1.0), None);
        canvas.draw_str(name, name_pos, &name_font, &name_paint);

        // Remaining time — tracked rose mono per the redesign's `.w-potions .t`.
        let time_pos = (text_x, ry + 28.0);
        draw_iw_text_shadow_tracked(canvas, time, time_pos, &time_font, time_tracking_em);
        let mut time_paint = Paint::default();
        time_paint.set_anti_alias(true);
        time_paint.set_color4f(rgba(ROSE, 1.0), None);
        draw_tracked_em(canvas, time, time_pos, &time_font, &time_paint, time_tracking_em);
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

    // Bumped SOFT 80 + WONK 1 — pulls the name into the wonkier italic-ish
    // territory called for in the design's `.w-target .name`. True italic
    // Fraunces would need a separate typeface load; this is the closest match
    // with the axes we expose today.
    let name_font = fonts.fraunces_axes(21.0, 80.0, 1.0, 600.0, Some(36.0));
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
    draw_iw_shell(canvas, chip, 16.0);

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

    let name_pos = (meta_x, name_baseline);
    draw_iw_text_shadow(canvas, &name, name_pos, &name_font);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&name, name_pos, &name_font, &name_paint);

    let dist_pos = (meta_right - dist_w, name_baseline);
    draw_iw_text_shadow(canvas, &dist_str, dist_pos, &dist_font);
    let mut dist_paint = Paint::default();
    dist_paint.set_anti_alias(true);
    dist_paint.set_color4f(rgba(ROSE, 0.9), None);
    canvas.draw_str(&dist_str, dist_pos, &dist_font, &dist_paint);

    if has_health {
        let bar_y = name_baseline + 12.0;
        let bar_h = 5.0;
        let bar = Rect::from_xywh(meta_x, bar_y, meta_right - meta_x, bar_h);
        let bar_rr = RRect::new_rect_xy(bar, bar_h / 2.0, bar_h / 2.0);

        let mut track = Paint::default();
        track.set_anti_alias(true);
        track.set_color4f(Color4f::new(PEARL.0 as f32 / 255.0, PEARL.1 as f32 / 255.0, PEARL.2 as f32 / 255.0, 0.12), None);
        canvas.draw_rrect(bar_rr, &track);

        let frac = (health / max_health).clamp(0.0, 1.0);
        let fill_w = bar.width() * frac;
        if fill_w > 1.0 {
            // Rose halo behind the fill — the redesign's `box-shadow:0 0 10px`
            // on `.w-target .hp .bar::before`.
            let halo_rect = Rect::from_xywh(bar.left, bar_y, fill_w, bar_h);
            let halo_rr = RRect::new_rect_xy(halo_rect, bar_h / 2.0, bar_h / 2.0);
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(ROSE, 0.5), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
            canvas.draw_rrect(halo_rr, &halo);

            let fill = Rect::from_xywh(bar.left, bar_y, fill_w, bar_h);
            let mut fill_paint = Paint::default();
            fill_paint.set_anti_alias(true);
            // 90° rose→ember across the bar per the redesign — keeps the "right
            // end = danger" visual cue regardless of fill level.
            const EMBER: (u8, u8, u8) = (0xC9, 0x6A, 0x7A);
            if let Some(shader) = gradient_shader::linear(
                (Point::new(bar.left, bar_y), Point::new(bar.right, bar_y)),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[rgba(ROSE, 1.0), rgba(EMBER, 1.0)],
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
// PvP Utils widgets — jump-reset indicator + hit-range chip.
// ────────────────────────────────────────────────────────────────────────

/// Tier → display colour. Velvet palette (matches `EwoPvpConfig` defaults on
/// the Java side). The matched-zone colour for hit-range comes from the wire,
/// not this table.
fn pvp_tier_color(tier: PvpTier) -> (u8, u8, u8) {
    match tier {
        PvpTier::Perfect => CHAMP,
        PvpTier::SlightlyLate => ROSE,
        PvpTier::Late => (0xC9, 0x6A, 0x7A), // --accent-ember
        PvpTier::SlightlyEarly => LAV,
        PvpTier::Early => BERRY,
        PvpTier::None => MAUVE,
    }
}

/// "PERFECT RESET" / "+50 ms LATE" / etc. — the Velvet re-skin of the
/// source mod's JumpResetHud. A wine chip with the tier label in Fraunces and
/// a tracked-mono "ms" suffix; on PERFECT, an extra rose-champagne glow.
fn draw_jump_reset_text(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let tier = if data.pvp_jump_active() { data.pvp_jump_tier() } else { PvpTier::Perfect };
    let offset_ms = data.pvp_jump_offset_ms();
    let fade = if data.pvp_jump_active() { data.pvp_jump_fade() } else { 0.45 };

    let tier_text = tier.label();
    let ms_text = match tier {
        PvpTier::Perfect | PvpTier::None => String::new(),
        _ => {
            let sign = if offset_ms >= 0 { "+" } else { "" };
            format!("  {}{} ms", sign, offset_ms)
        }
    };

    let title_font = fonts.fraunces_axes(22.0, 40.0, 0.0, 600.0, None);
    let ms_font = fonts.jetbrains_mono(13.0);
    let ms_tracking_em = 0.14;

    let pad_x = 18.0;
    let pad_y = 10.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (title_w, _) = title_font.measure_str(tier_text, Some(&probe));
    let ms_w = if ms_text.is_empty() { 0.0 } else { measure_tracked_em(&ms_font, &ms_text, ms_tracking_em) };

    let (_, m) = title_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 22.0 * 0.72 };
    let chip_w = pad_x * 2.0 + title_w + ms_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let tier_color = pvp_tier_color(tier);
    let alpha = if data.pvp_jump_active() { fade.clamp(0.0, 1.0) } else { 0.5 };

    // Glow under PERFECT — celebratory champagne halo, only when the result
    // is fresh (no glow in the editor preview).
    if tier == PvpTier::Perfect && data.pvp_jump_active() {
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(rgba(tier_color, 0.55 * fade), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 14.0, false));
        canvas.draw_str(tier_text, (x + pad_x, baseline_y), &title_font, &glow);
    }

    // In-world 3-stop dark velvet text shadow for legibility over any backdrop.
    let title_pos = (x + pad_x, baseline_y);
    draw_iw_text_shadow(canvas, tier_text, title_pos, &title_font);

    // The tier label itself.
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color4f(rgba(tier_color, alpha), None);
    canvas.draw_str(tier_text, title_pos, &title_font, &title_paint);

    // "+50 ms LATE" suffix — tracked mono in mauve so the tier word reads first.
    if !ms_text.is_empty() {
        let ms_pos = (x + pad_x + title_w, baseline_y);
        draw_iw_text_shadow_tracked(canvas, &ms_text, ms_pos, &ms_font, ms_tracking_em);
        let mut ms_paint = Paint::default();
        ms_paint.set_anti_alias(true);
        ms_paint.set_color4f(rgba(MAUVE, alpha), None);
        draw_tracked_em(canvas, &ms_text, ms_pos, &ms_font, &ms_paint, ms_tracking_em);
    }

    chip
}

/// The timing meter — a Velvet glass pill with a centre rose-pip "perfect"
/// marker and a sliding pearl tick at the player's actual offset. Replaces
/// the source mod's red→green→red boss-bar with the Velvet language.
fn draw_jump_reset_bar(
    canvas: &Canvas,
    data: &HudData,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    const BAR_W: f32 = 180.0;
    const BAR_H: f32 = 6.0;
    const PIP: f32 = 12.0; // tall side: the offset marker height
    let chip_h = PIP.max(BAR_H) + 6.0;
    let (x, y) = anchor.origin(ax, ay, BAR_W, chip_h);
    let bounds = Rect::from_xywh(x, y, BAR_W, chip_h);

    let active = data.pvp_jump_active();
    let fade = if active { data.pvp_jump_fade() } else { 0.5 };
    let tier = if active { data.pvp_jump_tier() } else { PvpTier::Perfect };
    let offset_ms = if active { data.pvp_jump_offset_ms() as f32 } else { 0.0 };

    let track = Rect::from_xywh(x, y + (chip_h - BAR_H) * 0.5, BAR_W, BAR_H);
    let track_rr = RRect::new_rect_xy(track, BAR_H * 0.5, BAR_H * 0.5);

    // Track — a thin wine pill with a faint rose hairline (matches HUD chip).
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(rgba(WINE, 0.62), None);
    canvas.draw_rrect(track_rr, &bg);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.18), None);
    canvas.draw_rrect(track_rr, &border);

    // The "perfect" marker — a centre pip in champagne with a soft halo.
    let cx = x + BAR_W * 0.5;
    let center_top = y + (chip_h - PIP) * 0.5;
    let center_rect = Rect::from_xywh(cx - 1.0, center_top, 2.0, PIP);
    let mut center_paint = Paint::default();
    center_paint.set_anti_alias(true);
    center_paint.set_color4f(rgba(CHAMP, 0.55), None);
    canvas.draw_rect(center_rect, &center_paint);

    // The "your offset" mark — a small pearl tick slid out to the right
    // (late) or left (early) by an offset proportional to ±300 ms full-scale.
    if active {
        const MAX_MS: f32 = 300.0;
        let norm = (offset_ms / MAX_MS).clamp(-1.0, 1.0);
        let mx = cx + norm * (BAR_W * 0.5 - 4.0);
        let tier_color = pvp_tier_color(tier);
        let mark_w = 3.0;
        let mark_h = PIP + 2.0;
        let mark = Rect::from_xywh(mx - mark_w * 0.5, y + (chip_h - mark_h) * 0.5, mark_w, mark_h);
        let mark_rr = RRect::new_rect_xy(mark, 1.5, 1.5);

        // Halo behind the mark.
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_color4f(rgba(tier_color, 0.55 * fade), None);
        halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 6.0, false));
        canvas.draw_rrect(mark_rr, &halo);

        let mut mark_paint = Paint::default();
        mark_paint.set_anti_alias(true);
        mark_paint.set_color4f(rgba(PEARL, fade), None);
        canvas.draw_rrect(mark_rr, &mark_paint);
    } else {
        // In editor preview, draw a dim mark at the centre so the widget has
        // visible chrome to grab.
        let mark = Rect::from_xywh(cx - 1.5, y + (chip_h - PIP) * 0.5 - 1.0, 3.0, PIP + 2.0);
        let mut mark_paint = Paint::default();
        mark_paint.set_anti_alias(true);
        mark_paint.set_color4f(rgba(PEARL, 0.35), None);
        canvas.draw_rrect(RRect::new_rect_xy(mark, 1.5, 1.5), &mark_paint);
    }

    bounds
}

/// Hit-range chip — a big Fraunces distance reading + a tracked "BLOCKS"
/// eyebrow, tinted by the matched zone's colour (set by the user in pvp.toml).
fn draw_hit_range(
    canvas: &Canvas,
    data: &HudData,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let active = data.pvp_hit_active();
    let distance = if active { data.pvp_hit_distance() } else { 3.00 };
    let fade = if active { data.pvp_hit_fade() } else { 0.45 };
    let zone_color = if active && data.pvp_hit_color() != 0 {
        let c = data.pvp_hit_color();
        ((c >> 16) as u8 & 0xFF, (c >> 8) as u8 & 0xFF, c as u8 & 0xFF)
    } else {
        ROSE
    };

    let value = format!("{:.2}", distance.max(0.0));
    let unit = "BLOCKS";

    let num_font = fonts.fraunces_axes(28.0, 34.0, 0.0, 600.0, None);
    let unit_font = fonts.jetbrains_mono(12.0);
    let unit_tracking_em = 0.18;

    let pad_x = 16.0;
    let pad_y = 9.0;
    let gap = 10.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (num_w, _) = num_font.measure_str(&value, Some(&probe));
    let unit_w = measure_tracked_em(&unit_font, unit, unit_tracking_em);

    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 28.0 * 0.72 };
    let chip_w = pad_x * 2.0 + num_w + gap + unit_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let alpha = if active { fade.clamp(0.0, 1.0) } else { 0.5 };

    // In-world 3-stop dark velvet shadow stack.
    let value_pos = (x + pad_x, baseline_y);
    draw_iw_text_shadow(canvas, &value, value_pos, &num_font);
    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(zone_color, alpha), None);
    canvas.draw_str(&value, value_pos, &num_font, &num_paint);

    // "BLOCKS" — tracked rose eyebrow per the redesign's `.w-pvp .hitrange .lbl`.
    let unit_pos = (x + pad_x + num_w + gap, baseline_y);
    draw_iw_text_shadow_tracked(canvas, unit, unit_pos, &unit_font, unit_tracking_em);
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(ROSE, alpha), None);
    draw_tracked_em(canvas, unit, unit_pos, &unit_font, &unit_paint, unit_tracking_em);

    chip
}

/// Click-per-second chip — two Fraunces numbers (left | right mouse) with a
/// thin mauve divider and a tracked "CPS" eyebrow. Mirrors the AxolotlClient /
/// Lunar idiom; left number is always the LMB rate.
fn draw_cps(
    canvas: &Canvas,
    left: i32,
    right: i32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let left_s = left.to_string();
    let right_s = right.to_string();
    let unit = "CPS";

    let num_font = fonts.fraunces_axes(30.0, 34.0, 0.0, 600.0, None);
    let unit_font = fonts.jetbrains_mono(14.0);
    let unit_tracking_em = 0.18;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let inner_gap = 10.0; // between number and divider
    let unit_gap = 10.0; // between right number and unit
    let div_w = 1.5;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (l_w, _) = num_font.measure_str(&left_s, Some(&probe));
    let (r_w, _) = num_font.measure_str(&right_s, Some(&probe));
    let unit_w = measure_tracked_em(&unit_font, unit, unit_tracking_em);

    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 30.0 * 0.72 };
    let chip_w =
        pad_x * 2.0 + l_w + inner_gap + div_w + inner_gap + r_w + unit_gap + unit_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let r_x = x + pad_x + l_w + inner_gap + div_w + inner_gap;

    // In-world 3-stop dark velvet shadow on both numbers.
    let left_pos = (x + pad_x, baseline_y);
    let right_pos = (r_x, baseline_y);
    draw_iw_text_shadow(canvas, &left_s, left_pos, &num_font);
    draw_iw_text_shadow(canvas, &right_s, right_pos, &num_font);

    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&left_s, left_pos, &num_font, &num_paint);
    canvas.draw_str(&right_s, right_pos, &num_font, &num_paint);

    // Divider — thin mauve vertical line spanning the cap height.
    let mut div_paint = Paint::default();
    div_paint.set_anti_alias(true);
    div_paint.set_style(PaintStyle::Stroke);
    div_paint.set_stroke_width(div_w);
    div_paint.set_color4f(rgba(MAUVE, 0.55), None);
    let div_x = x + pad_x + l_w + inner_gap + div_w * 0.5;
    let div_top = baseline_y - cap + 2.0;
    let div_bot = baseline_y - 2.0;
    canvas.draw_line((div_x, div_top), (div_x, div_bot), &div_paint);

    let unit_pos = (r_x + r_w + unit_gap, baseline_y);
    draw_iw_text_shadow_tracked(canvas, unit, unit_pos, &unit_font, unit_tracking_em);
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(canvas, unit, unit_pos, &unit_font, &unit_paint, unit_tracking_em);

    chip
}

/// Item-counter strip — four side-by-side cells (pearls / arrows / totems /
/// gapples). Each cell pairs a tracked Mono mauve label with a Fraunces count
/// in the item's accent colour. Zero-count cells render dimmed so the chip
/// width is stable as the player picks items up or drops them.
fn draw_item_counters(
    canvas: &Canvas,
    pearls: i32,
    arrows: i32,
    totems: i32,
    gapples: i32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let items: [(&str, i32, (u8, u8, u8)); 4] = [
        ("PRL", pearls, LAV),
        ("ARW", arrows, CHAMP),
        ("TOT", totems, ROSE),
        ("GAP", gapples, BERRY),
    ];

    let num_font = fonts.fraunces_axes(22.0, 32.0, 0.0, 600.0, None);
    let label_font = fonts.jetbrains_mono(11.0);
    let label_tracking_em = 0.18;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let label_to_num_gap = 6.0;
    let item_gap = 16.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 22.0 * 0.72 };

    let mut widths: [(f32, f32); 4] = [(0.0, 0.0); 4];
    let mut total = 0.0;
    for i in 0..4 {
        let (label, n, _) = items[i];
        let num_s = n.to_string();
        let (num_w, _) = num_font.measure_str(&num_s, Some(&probe));
        let label_w = measure_tracked_em(&label_font, label, label_tracking_em);
        widths[i] = (label_w, num_w);
        total += label_w + label_to_num_gap + num_w;
    }
    total += item_gap * 3.0;

    let chip_w = pad_x * 2.0 + total;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let mut cursor_x = x + pad_x;

    for i in 0..4 {
        let (label, n, color) = items[i];
        let (label_w, num_w) = widths[i];
        let alpha = if n > 0 { 1.0 } else { 0.35 };

        let label_pos = (cursor_x, baseline_y);
        draw_iw_text_shadow_tracked(canvas, label, label_pos, &label_font, label_tracking_em);
        let mut label_paint = Paint::default();
        label_paint.set_anti_alias(true);
        label_paint.set_color4f(rgba(MAUVE, alpha), None);
        draw_tracked_em(canvas, label, label_pos, &label_font, &label_paint, label_tracking_em);

        let num_x = cursor_x + label_w + label_to_num_gap;
        let num_s = n.to_string();
        let num_pos = (num_x, baseline_y);
        if n > 0 {
            draw_iw_text_shadow(canvas, &num_s, num_pos, &num_font);
        }

        let mut num_paint = Paint::default();
        num_paint.set_anti_alias(true);
        let nc = if n > 0 { color } else { PEARL };
        num_paint.set_color4f(rgba(nc, alpha), None);
        canvas.draw_str(&num_s, num_pos, &num_font, &num_paint);

        cursor_x = num_x + num_w + item_gap;
    }

    chip
}

/// Local-player shield cooldown bar — a wide rose-fill pill on a wine track
/// with a "SHIELD" eyebrow + seconds-remaining numeric. The fraction is
/// taken straight from `ItemCooldowns.getCooldownPercent`; the vanilla
/// disable is 5 s (5 × 20 ticks = 100), so we render `pct * 5.0` seconds.
fn draw_shield_cooldown(
    canvas: &Canvas,
    pct: f32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let pct = pct.clamp(0.0, 1.0);
    let seconds_left = pct * 5.0;

    let label = "SHIELD";
    let value = format!("{:.1}s", seconds_left);

    let label_font = fonts.jetbrains_mono(11.0);
    let value_font = fonts.fraunces_axes(18.0, 32.0, 0.0, 600.0, None);
    let label_tracking_em = 0.20;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let bar_h = 4.0;
    let bar_gap = 8.0;
    let value_gap = 10.0;
    let radius = 12.0;
    let chip_w = 180.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (val_w, _) = value_font.measure_str(&value, Some(&probe));
    let label_w = measure_tracked_em(&label_font, label, label_tracking_em);

    let (_, vm) = value_font.metrics();
    let value_cap = if vm.cap_height > 0.0 { vm.cap_height } else { 18.0 * 0.72 };
    let (_, lm) = label_font.metrics();
    let label_cap = if lm.cap_height > 0.0 { lm.cap_height } else { 11.0 * 0.72 };

    let header_h = value_cap.max(label_cap);
    let chip_h = pad_y * 2.0 + header_h + bar_gap + bar_h;

    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    // Header row — eyebrow on the left, seconds-remaining on the right.
    let baseline = y + pad_y + header_h;

    let label_pos = (x + pad_x, baseline);
    draw_iw_text_shadow_tracked(canvas, label, label_pos, &label_font, label_tracking_em);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(CHAMP, 1.0), None);
    draw_tracked_em(canvas, label, label_pos, &label_font, &label_paint, label_tracking_em);

    let value_x = x + chip_w - pad_x - val_w;
    let _ = label_w + value_gap; // gap consumed implicitly by chip_w
    let value_pos = (value_x, baseline);
    draw_iw_text_shadow(canvas, &value, value_pos, &value_font);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    let ember = (0xC9, 0x6A, 0x7A);
    value_paint.set_color4f(rgba(ember, 1.0), None);
    canvas.draw_str(&value, value_pos, &value_font, &value_paint);

    // Bar — track + fill below the header row.
    let bar_y = y + pad_y + header_h + bar_gap;
    let track = Rect::from_xywh(x + pad_x, bar_y, chip_w - pad_x * 2.0, bar_h);
    let track_rr = RRect::new_rect_xy(track, bar_h * 0.5, bar_h * 0.5);
    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(WINE, 0.78), None);
    canvas.draw_rrect(track_rr, &track_paint);

    if pct > 0.0 {
        let fill_w = (chip_w - pad_x * 2.0) * pct;
        let fill_rect = Rect::from_xywh(x + pad_x, bar_y, fill_w, bar_h);
        let fill_rr = RRect::new_rect_xy(fill_rect, bar_h * 0.5, bar_h * 0.5);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        // Ember when high (just disabled), rose as it ticks down to ready.
        let color = if pct > 0.5 { ember } else { ROSE };
        fill.set_color4f(rgba(color, 1.0), None);
        canvas.draw_rrect(fill_rr, &fill);
    }

    chip
}

/// Combo counter chip — re-skin of `draw_stat` with an age-based alpha
/// fade. The count itself snaps to 0 server-side on timeout (5 s of no
/// hits) or hit-taken, but the visual fades smoothly over the last second
/// so a missed combo ages out instead of vanishing on the next frame.
fn draw_combo(
    canvas: &Canvas,
    count: i32,
    age_secs: f32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    // Velvet fade: full alpha for the first 4 s, linear decay over the last
    // 1 s of the 5 s combo window. After that the count is 0 anyway.
    let alpha_mul = if age_secs < 4.0 {
        1.0
    } else if age_secs >= 5.0 {
        0.0
    } else {
        1.0 - (age_secs - 4.0)
    };

    let value = count.to_string();
    let unit = "COMBO";

    let num_font = fonts.fraunces_axes(30.0, 34.0, 0.0, 600.0, None);
    let unit_font = fonts.jetbrains_mono(14.0);
    let unit_tracking_em = 0.18;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let gap = 8.0;
    let radius = 12.0;

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (num_w, _) = num_font.measure_str(&value, Some(&probe));
    let unit_w = measure_tracked_em(&unit_font, unit, unit_tracking_em);

    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 30.0 * 0.72 };
    let chip_w = pad_x * 2.0 + num_w + gap + unit_w;
    let chip_h = pad_y * 2.0 + cap;
    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);

    // Fade the entire widget — shell + text — together by wrapping the draw
    // calls in a save_layer with the alpha multiplier as its layer alpha.
    if alpha_mul <= 0.001 {
        return chip;
    }
    let mut layer_paint = Paint::default();
    layer_paint.set_anti_alias(true);
    layer_paint.set_alpha_f(alpha_mul);
    let layer_bounds = chip.with_outset((24.0, 24.0)); // include the iw_shell drop shadow
    canvas.save_layer(
        &skia_safe::canvas::SaveLayerRec::default()
            .bounds(&layer_bounds)
            .paint(&layer_paint),
    );

    draw_iw_shell(canvas, chip, radius);

    let baseline_y = y + pad_y + cap;
    let num_x = x + pad_x;
    let unit_x = num_x + num_w + gap;

    // Number — champagne when combo is fresh (≥ 4 stacks reads as "real
    // combo"), otherwise rose.
    let num_color = if count >= 4 { CHAMP } else { ROSE };
    let num_pos = (num_x, baseline_y);
    draw_iw_text_shadow(canvas, &value, num_pos, &num_font);
    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(num_color, 1.0), None);
    canvas.draw_str(&value, num_pos, &num_font, &num_paint);

    let unit_pos = (unit_x, baseline_y);
    draw_iw_text_shadow_tracked(canvas, unit, unit_pos, &unit_font, unit_tracking_em);
    let mut unit_paint = Paint::default();
    unit_paint.set_anti_alias(true);
    unit_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(canvas, unit, unit_pos, &unit_font, &unit_paint, unit_tracking_em);

    canvas.restore();

    chip
}

/// Attack-strength charge meter — a wide pill bar showing the vanilla
/// attack-strength scale (0..1). Fill colour ramps ember → rose → champagne
/// as the meter approaches full, so peripheral vision can tell at a glance
/// whether the next click will land a full-charge hit. A "READY" tracked-mono
/// eyebrow flashes briefly when charge crosses 0.98.
fn draw_attack_charge(
    canvas: &Canvas,
    charge: f32,
    fonts: &FontStore,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> Rect {
    let charge = charge.clamp(0.0, 1.0);

    let label = "ATTACK";
    let label_font = fonts.jetbrains_mono(11.0);
    let label_tracking_em = 0.20;

    let pad_x = 14.0;
    let pad_y = 8.0;
    let bar_h = 5.0;
    let bar_gap = 8.0;
    let radius = 12.0;
    let chip_w = 168.0;

    let label_w = measure_tracked_em(&label_font, label, label_tracking_em);
    let (_, lm) = label_font.metrics();
    let label_cap = if lm.cap_height > 0.0 { lm.cap_height } else { 11.0 * 0.72 };

    // Right-aligned percentage value in JetBrains Mono.
    let value = if charge >= 0.999 { "READY".to_string() } else { format!("{:>3}%", (charge * 100.0).round() as i32) };
    let value_font = fonts.jetbrains_mono(11.0);
    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (val_w, _) = value_font.measure_str(&value, Some(&probe));

    let header_h = label_cap;
    let chip_h = pad_y * 2.0 + header_h + bar_gap + bar_h;

    let (x, y) = anchor.origin(ax, ay, chip_w, chip_h);
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);
    draw_iw_shell(canvas, chip, radius);

    let baseline = y + pad_y + header_h;
    let label_pos = (x + pad_x, baseline);
    draw_iw_text_shadow_tracked(canvas, label, label_pos, &label_font, label_tracking_em);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(CHAMP, 1.0), None);
    draw_tracked_em(canvas, label, label_pos, &label_font, &label_paint, label_tracking_em);
    let _ = label_w; // gap consumed implicitly by chip_w

    let value_pos = (x + chip_w - pad_x - val_w, baseline);
    draw_iw_text_shadow(canvas, &value, value_pos, &value_font);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    let value_color = if charge >= 0.999 { CHAMP } else { PEARL };
    value_paint.set_color4f(rgba(value_color, 1.0), None);
    canvas.draw_str(&value, value_pos, &value_font, &value_paint);

    // Bar — track + fill below the header row.
    let bar_y = y + pad_y + header_h + bar_gap;
    let track = Rect::from_xywh(x + pad_x, bar_y, chip_w - pad_x * 2.0, bar_h);
    let track_rr = RRect::new_rect_xy(track, bar_h * 0.5, bar_h * 0.5);
    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(WINE, 0.78), None);
    canvas.draw_rrect(track_rr, &track_paint);

    if charge > 0.01 {
        let fill_w = (chip_w - pad_x * 2.0) * charge;
        let fill_rect = Rect::from_xywh(x + pad_x, bar_y, fill_w, bar_h);
        let fill_rr = RRect::new_rect_xy(fill_rect, bar_h * 0.5, bar_h * 0.5);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        // ember when half-or-less, rose at moderate, champagne when ready.
        let ember = (0xC9, 0x6A, 0x7A);
        let color = if charge >= 0.999 {
            CHAMP
        } else if charge >= 0.66 {
            ROSE
        } else {
            ember
        };
        fill.set_color4f(rgba(color, 1.0), None);
        canvas.draw_rrect(fill_rr, &fill);
    }

    chip
}

/// Linear distance-fade alpha multiplier for world-anchored indicators.
/// 1.0 at distance ≤ near, falls off to `min` at distance ≥ far, linear in
/// between. Cheap declutter for busy fights — far entities dim, near ones
/// stay crisp.
fn distance_fade_alpha(distance: f32, near: f32, far: f32, min: f32) -> f32 {
    if distance <= near {
        return 1.0;
    }
    if distance >= far {
        return min;
    }
    let t = (distance - near) / (far - near);
    1.0 - t * (1.0 - min)
}

/// Overhead totem-of-undying pop counter — a small rose chip with `× N`
/// painted just above the entity's head. Drawn only when `totem_count > 0`,
/// so entities without observed pops stay un-cluttered. Alpha fades with
/// distance so far entities dim out.
fn draw_totem_overhead(canvas: &Canvas, ind: &Indicator, fonts: &FontStore) {
    let label = format!("\u{00D7} {}", ind.totem_count); // "× N"
    let num_font = fonts.fraunces_axes(14.0, 30.0, 0.0, 600.0, None);
    let pad_x = 7.0;
    let pad_y = 3.5;

    let alpha_mul = distance_fade_alpha(ind.distance, 6.0, 32.0, 0.35);

    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (lw, _) = num_font.measure_str(&label, Some(&probe));
    let (_, m) = num_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 14.0 * 0.72 };
    let chip_w = pad_x * 2.0 + lw;
    let chip_h = pad_y * 2.0 + cap;

    // Stacked above the head; vertical-offset 22 px clears any nametag.
    let cx = ind.screen_x;
    let cy = ind.screen_y - 22.0;
    let x = cx - chip_w * 0.5;
    let y = cy - chip_h * 0.5;
    let chip = Rect::from_xywh(x, y, chip_w, chip_h);

    // Wine fill + rose hairline — the Velvet chip language at small scale.
    let rrect = RRect::new_rect_xy(chip, chip_h * 0.45, chip_h * 0.45);
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.78 * alpha_mul), None);
    canvas.draw_rrect(rrect, &fill);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.55 * alpha_mul), None);
    canvas.draw_rrect(rrect, &border);

    let baseline = y + pad_y + cap;
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55 * alpha_mul), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 2.0, false));
    canvas.draw_str(&label, (x + pad_x, baseline + 1.0), &num_font, &shadow);

    let mut num_paint = Paint::default();
    num_paint.set_anti_alias(true);
    num_paint.set_color4f(rgba(ROSE, alpha_mul), None);
    canvas.draw_str(&label, (x + pad_x, baseline), &num_font, &num_paint);
}

/// Floating health bar — a narrow rose fill on a wine track, with the live HP
/// numerically beneath. A brief ember "-N.N" damage pop fades out beside the
/// number after each hit. Anchored at the entity's "above-head" screen point.
fn draw_floating_health(canvas: &Canvas, ind: &Indicator, fonts: &FontStore) {
    if ind.max_health <= 0.0 {
        return;
    }
    let frac = (ind.health / ind.max_health).clamp(0.0, 1.0);

    // Distance fade — same near/far/min as draw_totem_overhead so the two
    // indicators dim in lockstep on the same entity.
    let alpha_mul = distance_fade_alpha(ind.distance, 6.0, 32.0, 0.35);

    // Bar geometry — fixed width so the indicator stays readable at any
    // distance. Sat above the head; the totem chip stacks higher still.
    let bar_w = 56.0;
    let bar_h = 4.0;
    let cx = ind.screen_x;
    let cy = ind.screen_y;
    let bar_x = cx - bar_w * 0.5;
    let bar_y = cy;

    // Track — wine pill with a hairline inset for legibility on bright maps.
    let track = Rect::from_xywh(bar_x, bar_y, bar_w, bar_h);
    let track_rr = RRect::new_rect_xy(track, bar_h * 0.5, bar_h * 0.5);
    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(WINE, 0.78 * alpha_mul), None);
    canvas.draw_rrect(track_rr, &track_paint);

    // Fill — rose for healthy, ember for low. Threshold at 30% mirrors
    // vanilla's "low HP" heart flash.
    let fill_color = if frac < 0.30 { (0xC9, 0x6A, 0x7A) } else { ROSE };
    let fill_rect = Rect::from_xywh(bar_x, bar_y, bar_w * frac, bar_h);
    if fill_rect.width() > 0.0 {
        let fill_rr = RRect::new_rect_xy(fill_rect, bar_h * 0.5, bar_h * 0.5);
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_color4f(rgba(fill_color, alpha_mul), None);
        canvas.draw_rrect(fill_rr, &fill);
    }

    let border_rr = RRect::new_rect_xy(track, bar_h * 0.5, bar_h * 0.5);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(0.8);
    border.set_color4f(rgba(ROSE, 0.30 * alpha_mul), None);
    canvas.draw_rrect(border_rr, &border);

    // HP read-out — JetBrains Mono beneath the bar.
    let hp_label = format!("{:.1} / {:.0}", ind.health.max(0.0), ind.max_health);
    let hp_font = fonts.jetbrains_mono(10.0);
    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (hp_w, _) = hp_font.measure_str(&hp_label, Some(&probe));
    let (_, m) = hp_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 7.5 };
    let text_baseline = bar_y + bar_h + cap + 4.0;

    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.65 * alpha_mul), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 2.0, false));
    canvas.draw_str(
        &hp_label,
        (cx - hp_w * 0.5, text_baseline + 1.0),
        &hp_font,
        &shadow,
    );

    let mut hp_paint = Paint::default();
    hp_paint.set_anti_alias(true);
    hp_paint.set_color4f(rgba(PEARL, alpha_mul), None);
    canvas.draw_str(&hp_label, (cx - hp_w * 0.5, text_baseline), &hp_font, &hp_paint);

    // Damage pop — ember "-N.N" beside the HP, fading out over 1.5 s.
    if ind.damage_age_sec >= 0.0 && ind.last_damage > 0.05 {
        let alpha = (1.0 - (ind.damage_age_sec / 1.5).clamp(0.0, 1.0)).powf(1.2) * alpha_mul;
        let dmg_label = format!("-{:.1}", ind.last_damage);
        let dmg_font = fonts.fraunces_axes(13.0, 30.0, 0.0, 600.0, None);
        let mut dmg_paint = Paint::default();
        dmg_paint.set_anti_alias(true);
        let ember = (0xC9, 0x6A, 0x7A);
        dmg_paint.set_color4f(rgba(ember, alpha), None);
        let dmg_x = cx + hp_w * 0.5 + 8.0;
        // Lift the pop as it fades, the standard "damage number" affordance.
        let lift = ind.damage_age_sec * 8.0;
        canvas.draw_str(
            &dmg_label,
            (dmg_x, text_baseline - lift),
            &dmg_font,
            &dmg_paint,
        );
    }
}

/// Hit Indicator chevron — a small ember triangle on a circle around screen
/// centre, pointing outward in the direction the most recent attacker is
/// relative to the player's facing. Fades to zero alpha by `fade_secs`.
///
/// `relative_yaw` is in degrees: 0 = ahead (top of screen), +90 = right,
/// -90 = left, ±180 = behind (bottom). Mapped to a screen-space circle
/// position via `(sin(yaw), -cos(yaw)) × radius`.
fn draw_hit_indicator(
    canvas: &Canvas,
    w: f32,
    h: f32,
    relative_yaw_deg: f32,
    age_secs: f32,
    fade_secs: f32,
    radius_pct: f32,
) {
    let progress = (age_secs / fade_secs).clamp(0.0, 1.0);
    let alpha = (1.0 - progress).powf(1.2);
    if alpha <= 0.01 {
        return;
    }

    let cx = w * 0.5;
    let cy = h * 0.5;
    let radius = (w.min(h) * radius_pct * 0.01).min(280.0);

    let yaw_rad = relative_yaw_deg.to_radians();
    let dir_x = yaw_rad.sin();
    let dir_y = -yaw_rad.cos();

    let px = cx + dir_x * radius;
    let py = cy + dir_y * radius;

    // Triangle pointing AWAY from screen centre (toward the attacker bearing).
    let tip_len = 18.0;
    let base_back = 8.0;
    let base_half = 9.0;
    let perp_x = -dir_y;
    let perp_y = dir_x;

    let tip = (px + dir_x * tip_len, py + dir_y * tip_len);
    let base_l = (
        px - dir_x * base_back + perp_x * base_half,
        py - dir_y * base_back + perp_y * base_half,
    );
    let base_r = (
        px - dir_x * base_back - perp_x * base_half,
        py - dir_y * base_back - perp_y * base_half,
    );

    let mut path = skia_safe::Path::new();
    path.move_to(tip);
    path.line_to(base_l);
    path.line_to(base_r);
    path.close();

    let ember = (0xC9, 0x6A, 0x7A);

    // Outer glow — wider stroke under the fill so the chevron pops over busy
    // backgrounds.
    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_style(PaintStyle::Stroke);
    glow.set_stroke_width(6.0);
    glow.set_color4f(rgba(ember, alpha * 0.45), None);
    glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    canvas.draw_path(&path, &glow);

    // Filled chevron — ember body with a rose hairline outline.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(ember, alpha * 0.88), None);
    canvas.draw_path(&path, &fill);

    let mut stroke = Paint::default();
    stroke.set_anti_alias(true);
    stroke.set_style(PaintStyle::Stroke);
    stroke.set_stroke_width(1.2);
    stroke.set_color4f(rgba(ROSE, alpha), None);
    canvas.draw_path(&path, &stroke);
}

/// Rose "+" painted at screen centre to signal the entity under the crosshair
/// is within attack reach. A two-pass stroke: a soft outer halo first, then a
/// crisp inner stroke. Overlays the vanilla white crosshair (which paints into
/// fbo 0 before the HUD composite), giving a rose halo around the vanilla "+".
fn draw_crosshair_on_reach(canvas: &Canvas, w: f32, h: f32) {
    let cx = w * 0.5;
    let cy = h * 0.5;
    let arm = 7.0;

    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_style(PaintStyle::Stroke);
    glow.set_stroke_width(6.0);
    glow.set_color4f(rgba(ROSE, 0.55), None);
    glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
    canvas.draw_line((cx - arm, cy), (cx + arm, cy), &glow);
    canvas.draw_line((cx, cy - arm), (cx, cy + arm), &glow);

    let mut stroke = Paint::default();
    stroke.set_anti_alias(true);
    stroke.set_style(PaintStyle::Stroke);
    stroke.set_stroke_width(1.8);
    stroke.set_color4f(rgba(ROSE, 0.95), None);
    canvas.draw_line((cx - arm, cy), (cx + arm, cy), &stroke);
    canvas.draw_line((cx, cy - arm), (cx, cy + arm), &stroke);
}

// ────────────────────────────────────────────────────────────────────────
// HUD editor chrome — drawn over the widgets while the overlay is open.
// ────────────────────────────────────────────────────────────────────────

/// Draw the editor: a faint tint, a drag outline around each visible widget
/// (the hovered/dragged one highlighted + labelled), and a hint line.
fn draw_editor(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
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
    rows: [Rect; 17],
    toggles: [Rect; 17],
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
    let row_count = WidgetId::ALL.len() as f32;
    let panel_h =
        PAD * 2.0 + HEADER_H + WLABEL_H + ROW_H * row_count + SECTION_GAP + ALABEL_H + grid_h;
    let panel_y = (h - panel_h) * 0.5;
    let panel = Rect::from_xywh(PANEL_X, panel_y, PANEL_W, panel_h);

    let content_x = PANEL_X + PAD;
    let content_w = PANEL_W - PAD * 2.0;

    let rows_top = panel_y + PAD + HEADER_H + WLABEL_H;
    let mut rows = [empty_rect(); 17];
    let mut toggles = [empty_rect(); 17];
    for i in 0..WidgetId::ALL.len() {
        let row = Rect::from_xywh(content_x, rows_top + i as f32 * ROW_H, content_w, ROW_H);
        rows[i] = row;
        let tw = 34.0;
        let th = 18.0;
        toggles[i] = Rect::from_xywh(row.right - tw, row.top + (ROW_H - th) * 0.5, tw, th);
    }

    let grid_top = rows_top + ROW_H * row_count + SECTION_GAP + ALABEL_H;
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

// ────────────────────────────────────────────────────────────────────────
// Overlay dashboard — the top-centre view-tab strip + the Settings view.
// ────────────────────────────────────────────────────────────────────────

/// The overlay's top-centre view-tab strip: the whole pill + a rect per tab.
/// Fixed-width tabs so the renderer and the hit-tester agree without fonts.
fn tab_layout(w: f32) -> (Rect, [Rect; 6]) {
    const TAB_W: f32 = 110.0; // narrowed a touch — 6 tabs in the same strip.
    const TAB_H: f32 = 34.0;
    const TAB_Y: f32 = 18.0;
    let strip_w = TAB_W * 6.0;
    let strip_x = (w - strip_w) * 0.5;
    let pill = Rect::from_xywh(strip_x, TAB_Y, strip_w, TAB_H);
    let mut tabs = [empty_rect(); 6];
    for (i, slot) in tabs.iter_mut().enumerate() {
        *slot = Rect::from_xywh(strip_x + i as f32 * TAB_W, TAB_Y, TAB_W, TAB_H);
    }
    (pill, tabs)
}

/// Draw the view-tab strip; the active view's tab is lit with the design's
/// rose-berry vertical gradient + glow.
fn draw_tab_strip(canvas: &Canvas, view: OverlayView, fonts: &FontStore, w: f32) {
    let (pill, tabs) = tab_layout(w);

    // Pill container — flat dark wine, hairline pearl border. Matches the
    // redesign's `.tabs` (`background:rgba(10,0,8,.55); border:1px solid hair`).
    let pill_r = pill.height() * 0.5;
    let pill_rr = RRect::new_rect_xy(pill, pill_r, pill_r);
    let mut pill_fill = Paint::default();
    pill_fill.set_anti_alias(true);
    pill_fill.set_color4f(rgba(WINE, 0.55), None);
    canvas.draw_rrect(pill_rr, &pill_fill);
    let mut pill_border = Paint::default();
    pill_border.set_anti_alias(true);
    pill_border.set_style(PaintStyle::Stroke);
    pill_border.set_stroke_width(1.0);
    pill_border.set_color4f(rgba(PEARL, 0.08), None);
    canvas.draw_rrect(pill_rr, &pill_border);

    let font = fonts.jetbrains_mono(11.0);
    let tracking = 0.22; // CSS `.tab { letter-spacing: .22em }`.
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };

    for (i, &tab) in tabs.iter().enumerate() {
        let v = OverlayView::ALL[i];
        let active = v == view;
        if active {
            // The redesign's `.tab.active`: 180° berry → deep-wine vertical
            // gradient + inset 1px pearl top highlight + outer rose glow.
            let inset = Rect::from_xywh(
                tab.left + 4.0,
                tab.top + 4.0,
                tab.width() - 8.0,
                tab.height() - 8.0,
            );
            let inset_r = inset.height() * 0.5;
            let inset_rr = RRect::new_rect_xy(inset, inset_r, inset_r);

            // Outer rose glow (CSS `0 0 22px -4px rgba(180,116,145,.55)`).
            let mut glow = Paint::default();
            glow.set_anti_alias(true);
            glow.set_color4f(rgba(BERRY, 0.4), None);
            glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 11.0, false));
            canvas.draw_rrect(inset_rr, &glow);

            // Vertical berry→deep-wine fill.
            let mut fill = Paint::default();
            fill.set_anti_alias(true);
            if let Some(shader) = gradient_shader::linear(
                (
                    Point::new(inset.left, inset.top),
                    Point::new(inset.left, inset.bottom),
                ),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[
                        rgba(BERRY, 0.62),
                        Color4f::new(
                            0x7F as f32 / 255.0,
                            0x40 as f32 / 255.0,
                            0x5A as f32 / 255.0,
                            0.72,
                        ),
                    ],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fill.set_shader(shader);
            }
            canvas.draw_rrect(inset_rr, &fill);

            // Inset top-edge highlight (CSS `inset 0 1px 0 rgba(255,255,255,.18)`).
            canvas.save();
            let top_clip = Rect::from_xywh(inset.left, inset.top, inset.width(), 2.0);
            canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
            let inset_hi = Rect::new(
                inset.left + 0.5,
                inset.top + 0.5,
                inset.right - 0.5,
                inset.bottom - 0.5,
            );
            let inset_hi_r = (inset_r - 0.5).max(0.0);
            let mut top_paint = Paint::default();
            top_paint.set_anti_alias(true);
            top_paint.set_style(PaintStyle::Stroke);
            top_paint.set_stroke_width(1.0);
            top_paint.set_color4f(rgba(PEARL, 0.22), None);
            canvas.draw_rrect(
                RRect::new_rect_xy(inset_hi, inset_hi_r, inset_hi_r),
                &top_paint,
            );
            canvas.restore();
        }

        let label = v.title();
        let label_w = measure_tracked_em(&font, label, tracking);
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color4f(
            if active { rgba(PEARL, 1.0) } else { rgba(MAUVE, 1.0) },
            None,
        );
        draw_tracked_em(
            canvas,
            label,
            (
                tab.left + (tab.width() - label_w) * 0.5,
                tab.top + tab.height() * 0.5 + cap * 0.5,
            ),
            &font,
            &paint,
            tracking,
        );
    }
}

/// The Settings-view panel rect, its client-profile chips, and the 4
/// paint-rate selector buttons. Fixed-size so renderer + hit-tester agree.
fn settings_layout(w: f32, h: f32, profile_count: usize) -> (Rect, Vec<Rect>, [Rect; 4]) {
    const CHIP_W: f32 = 112.0;
    const CHIP_H: f32 = 32.0;
    const GAP: f32 = 8.0;
    let chip_rows = (profile_count.max(1) as f32 / 4.0).ceil();

    let pw = 484.0;
    let ph = 262.0 + chip_rows * (CHIP_H + GAP);
    let px = (w - pw) * 0.5;
    let py = (h - ph) * 0.5;
    let panel = Rect::from_xywh(px, py, pw, ph);
    let left = px + 32.0;

    // Client-profile chips — four per row, under the "CLIENT PROFILE" label.
    let chips_top = py + 122.0;
    let mut chips = Vec::with_capacity(profile_count);
    for i in 0..profile_count {
        let col = (i % 4) as f32;
        let row = (i / 4) as f32;
        chips.push(Rect::from_xywh(
            left + col * (CHIP_W + GAP),
            chips_top + row * (CHIP_H + GAP),
            CHIP_W,
            CHIP_H,
        ));
    }

    // Paint-rate buttons — below the profile section + its labels.
    const BTN_W: f32 = 92.0;
    const BTN_H: f32 = 38.0;
    const BTN_GAP: f32 = 10.0;
    let buttons_top = chips_top + chip_rows * (CHIP_H + GAP) + 64.0;
    let mut buttons = [empty_rect(); 4];
    for (i, slot) in buttons.iter_mut().enumerate() {
        *slot = Rect::from_xywh(left + i as f32 * (BTN_W + BTN_GAP), buttons_top, BTN_W, BTN_H);
    }
    (panel, chips, buttons)
}

/// One option button in the paint-rate selector.
fn draw_settings_button(canvas: &Canvas, rect: Rect, label: &str, active: bool, fonts: &FontStore) {
    let rrect = RRect::new_rect_xy(rect, 9.0, 9.0);

    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(if active { rgba(ROSE, 0.55) } else { rgba(WINE, 0.8) }, None);
    canvas.draw_rrect(rrect, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(
        if active { rgba(ROSE, 0.85) } else { rgba(ROSE, 0.18) },
        None,
    );
    canvas.draw_rrect(rrect, &border);

    let font = fonts.jetbrains_mono(13.0);
    let tracking = 0.10;
    let label_w = measure_tracked_em(&font, label, tracking);
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
    let mut text = Paint::default();
    text.set_anti_alias(true);
    text.set_color4f(
        if active { rgba(PEARL, 1.0) } else { rgba(MAUVE, 1.0) },
        None,
    );
    draw_tracked_em(
        canvas,
        label,
        (
            rect.left + (rect.width() - label_w) * 0.5,
            rect.top + rect.height() * 0.5 + cap * 0.5,
        ),
        &font,
        &text,
        tracking,
    );
}

/// The Settings view — a client-profile picker + the HUD paint-rate cap.
fn draw_settings(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let (panel, chips, buttons) = settings_layout(w, h, editor.profiles.len());
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 32.0;

    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        "SETTINGS",
        (left, panel.top + 40.0),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    let title_font = fonts.fraunces_axes(27.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str("HUD preferences", (left, panel.top + 78.0), &title_font, &title);

    let label_font = fonts.jetbrains_mono(10.0);

    // Client-profile picker — chips, the active one lit.
    let mut prof_label = Paint::default();
    prof_label.set_anti_alias(true);
    prof_label.set_color4f(rgba(MAUVE, 1.0), None);
    let prof_label_y = chips
        .first()
        .map(|c| c.top - 14.0)
        .unwrap_or(panel.top + 108.0);
    draw_tracked_em(
        canvas,
        "CLIENT PROFILE",
        (left, prof_label_y),
        &label_font,
        &prof_label,
        0.18,
    );
    for (chip, name) in chips.iter().zip(&editor.profiles) {
        draw_settings_button(canvas, *chip, name, *name == editor.active_profile, fonts);
    }

    // Paint-rate section — positioned relative to the buttons.
    let mut pr_label = Paint::default();
    pr_label.set_anti_alias(true);
    pr_label.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "PAINT RATE  ·  FPS CAP",
        (left, buttons[0].top - 46.0),
        &label_font,
        &pr_label,
        0.18,
    );

    let body_font = fonts.newsreader(13.0);
    let mut body = Paint::default();
    body.set_anti_alias(true);
    body.set_color4f(rgba(MAUVE, 0.85), None);
    canvas.draw_str(
        "How often the HUD repaints. A lower cap frees GPU for the game.",
        (left, buttons[0].top - 22.0),
        &body_font,
        &body,
    );

    let current = editor.paint_rate();
    for (i, &btn) in buttons.iter().enumerate() {
        let rate = crate::HudPaintRate::ALL[i];
        draw_settings_button(canvas, btn, rate.label(), rate == current, fonts);
    }
}

// ────────────────────────────────────────────────────────────────────────
// Home view — the session overview + quick toggles.
// ────────────────────────────────────────────────────────────────────────

/// The HOME-view panel, the 3D-skin viewport, 5 stat cards, the media-
/// controller card, and the quick-toggle area (a Rect — the chips themselves
/// flow at render time because their widths depend on the per-label text
/// measurement; the renderer caches the resulting rects in
/// `Editor::home_toggle_bounds` for the press handler).
fn home_layout(w: f32, h: f32) -> (Rect, Rect, [Rect; 5], Rect, Rect) {
    let pw = 780.0; // grew again for chip flow + breathing room.
    let ph = 720.0;
    let panel = Rect::from_xywh((w - pw) * 0.5, (h - ph) * 0.5, pw, ph);
    let gap = 12.0;

    // Left column — the 3D skin viewer. Sized to look like a *small* portrait
    // (not the entire panel height) — old layout had the model dominate.
    let skin_w = 150.0;
    let skin_h = 360.0;
    let skin = Rect::from_xywh(
        panel.left + 32.0,
        panel.top + 124.0,
        skin_w,
        skin_h,
    );

    // Right column — stat cards (a 2-wide grid, the last card full width).
    let rx = skin.right + 28.0;
    let rw = panel.right - 36.0 - rx;
    let card_h = 60.0;
    let card_w = (rw - gap) / 2.0;
    let stats_top = panel.top + 124.0;
    let step = card_h + gap;
    let stats = [
        Rect::from_xywh(rx, stats_top, card_w, card_h),
        Rect::from_xywh(rx + card_w + gap, stats_top, card_w, card_h),
        Rect::from_xywh(rx, stats_top + step, card_w, card_h),
        Rect::from_xywh(rx + card_w + gap, stats_top + step, card_w, card_h),
        Rect::from_xywh(rx, stats_top + 2.0 * step, rw, card_h),
    ];

    // Media controller large card — full-width, ~120px tall. Sits below the
    // account row (which draws after the stat grid in `draw_home`); the
    // vertical gap below stats was bumped from 76 → 96 to give the account
    // line breathing room.
    let media_top = stats_top + 2.0 * step + card_h + 96.0;
    let media_h = 120.0;
    let media = Rect::from_xywh(rx, media_top, rw, media_h);

    // Quick-toggle area — chips flow inside this rect; bumped gap from 22 → 32
    // so the eyebrow + chip rows aren't smushed against the media card.
    let toggle_area = Rect::from_xywh(rx, media.bottom + 32.0, rw, 138.0);

    (panel, skin, stats, media, toggle_area)
}

/// Format session seconds as `m:ss`, or `h:mm:ss` past an hour.
fn fmt_playtime(secs: i32) -> String {
    let s = secs.max(0);
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, sec)
    } else {
        format!("{}:{:02}", m, sec)
    }
}

/// `%APPDATA%/EwoClient/profiles.toml`.
fn profiles_toml_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("EwoClient").join("profiles.toml"))
}

/// Read `profiles.toml` — `(active, all profile names)`. `None` if the file
/// is missing or unreadable. The launcher owns this file; the in-game side
/// reads it and rewrites only the `active` pointer.
fn read_profiles() -> Option<(String, Vec<String>)> {
    let text = std::fs::read_to_string(profiles_toml_path()?).ok()?;
    let mut active = String::new();
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("active") {
            if let Some(val) = rest.trim_start().strip_prefix('=') {
                active = val.trim().trim_matches('"').to_string();
                break;
            }
        }
    }
    // Every quoted string after the `profiles` key is a profile name —
    // robust to inline or wrapped TOML arrays.
    let mut all: Vec<String> = Vec::new();
    let after = text
        .find("\nprofiles")
        .map(|i| &text[i..])
        .or_else(|| text.starts_with("profiles").then_some(text.as_str()));
    if let Some(mut rest) = after {
        while let Some(q1) = rest.find('"') {
            let tail = &rest[q1 + 1..];
            let Some(q2) = tail.find('"') else { break };
            all.push(tail[..q2].to_string());
            rest = &tail[q2 + 1..];
        }
    }
    if active.is_empty() && all.is_empty() {
        return None;
    }
    if active.is_empty() {
        active = all.first().cloned().unwrap_or_else(|| "Default".to_string());
    }
    if all.is_empty() {
        all.push(active.clone());
    }
    Some((active, all))
}

/// The active client-profile name, or `None` if `profiles.toml` is absent.
pub(crate) fn read_active_profile() -> Option<String> {
    read_profiles().map(|(active, _)| active)
}

/// Rewrite `profiles.toml` with a new active profile. The file holds only
/// `active` + `profiles`, so a full rewrite is total and the launcher
/// re-reads it cleanly.
fn write_profiles(active: &str, all: &[String]) {
    let Some(path) = profiles_toml_path() else {
        return;
    };
    let mut s = format!("active = \"{active}\"\nprofiles = [");
    for (i, p) in all.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push('"');
        s.push_str(p);
        s.push('"');
    }
    s.push_str("]\n");
    let _ = std::fs::write(&path, s);
}

/// One stat card on the HOME view — a chip with a mono label and a value,
/// plus the design's `.card.hairglow` subtle pearl-tint chrome:
/// `inset 0 1px 0 rgba(244,232,234,.06)` top highlight + `0 0 0 1px rgba(229,184,197,.04)`
/// rose tint ring. The lifts cards off the panel without screaming for attention.
fn draw_stat_card(canvas: &Canvas, rect: Rect, label: &str, value: &str, fonts: &FontStore) {
    let radius = 12.0;
    draw_chip(canvas, rect, radius);

    // `.card.hairglow` — a 1px rose tint ring just outside the chip, and an
    // inset pearl 1px highlight along the top edge.
    let rrect = RRect::new_rect_xy(rect, radius, radius);
    let mut tint = Paint::default();
    tint.set_anti_alias(true);
    tint.set_style(PaintStyle::Stroke);
    tint.set_stroke_width(1.0);
    tint.set_color4f(rgba(ROSE, 0.10), None);
    canvas.draw_rrect(rrect, &tint);

    canvas.save();
    let top_clip = Rect::from_xywh(rect.left, rect.top, rect.width(), 2.0);
    canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
    let inset = Rect::new(rect.left + 0.5, rect.top + 0.5, rect.right - 0.5, rect.bottom - 0.5);
    let mut top = Paint::default();
    top.set_anti_alias(true);
    top.set_style(PaintStyle::Stroke);
    top.set_stroke_width(1.0);
    top.set_color4f(rgba(PEARL, 0.10), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset, radius - 0.5, radius - 0.5), &top);
    canvas.restore();

    let left = rect.left + 14.0;

    let label_font = fonts.jetbrains_mono(9.0);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(canvas, label, (left, rect.top + 22.0), &label_font, &label_paint, 0.22);

    let value_font = fonts.fraunces_axes(20.0, 36.0, 0.0, 560.0, None);
    let mut value_paint = Paint::default();
    value_paint.set_anti_alias(true);
    value_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(value, (left, rect.top + 48.0), &value_font, &value_paint);
}

/// One quick-toggle pill — re-skin of the design's `.qpill`. A wine pill with
/// an LED dot + label; when on, the pill takes a vertical berry-wine gradient
/// fill + a brighter rose border + a glowing rose LED. Hover shading is added
/// by the outer hit-test (HOME doesn't currently track hover).
fn draw_toggle_chip(canvas: &Canvas, rect: Rect, label: &str, on: bool, fonts: &FontStore) {
    let r = rect.height() * 0.5;
    let rrect = RRect::new_rect_xy(rect, r, r);

    // Fill — flat wine when off, top-to-bottom berry-wine gradient when on
    // (CSS `linear-gradient(180deg, rgba(180,116,145,.35), rgba(127,64,90,.35))`).
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    if on {
        if let Some(shader) = gradient_shader::linear(
            (
                Point::new(rect.left, rect.top),
                Point::new(rect.left, rect.bottom),
            ),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    rgba(BERRY, 0.45),
                    Color4f::new(0x7F as f32 / 255.0, 0x40 as f32 / 255.0, 0x5A as f32 / 255.0, 0.45),
                ],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            bg.set_shader(shader);
        }
    } else {
        bg.set_color4f(rgba(WINE, 0.55), None);
    }
    canvas.draw_rrect(rrect, &bg);

    // Border — rose 0.32 when on, hairline pearl 0.10 when off.
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(if on { rgba(ROSE, 0.34) } else { rgba(PEARL, 0.10) }, None);
    canvas.draw_rrect(rrect, &border);

    let cy = (rect.top + rect.bottom) * 0.5;
    let dot_x = rect.left + 15.0;

    // LED dot — when on, a bright rose disc with a blurred rose halo behind it.
    if on {
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_color4f(rgba(ROSE, 0.55), None);
        halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
        canvas.draw_circle((dot_x, cy), 5.0, &halo);
    }
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(
        if on {
            rgba(ROSE, 1.0)
        } else {
            Color4f::new(0x43 as f32 / 255.0, 0x24 as f32 / 255.0, 0x2E as f32 / 255.0, 1.0)
        },
        None,
    );
    canvas.draw_circle((dot_x, cy), 3.0, &dot);

    let font = fonts.jetbrains_mono(9.0);
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
    let mut text = Paint::default();
    text.set_anti_alias(true);
    text.set_color4f(if on { rgba(PEARL, 1.0) } else { rgba(MAUVE, 1.0) }, None);
    draw_tracked_em(canvas, label, (dot_x + 12.0, cy + cap * 0.5), &font, &text, 0.22);
}

/// The HOME / overview view — a rotatable 3D skin, session stats, the
/// account + profile, and quick per-HUD-widget visibility toggles.
fn draw_home(canvas: &Canvas, editor: &mut Editor, data: &HudData, fonts: &FontStore, w: f32, h: f32) {
    let (panel, skin_rect, stats, media_rect, toggle_area) = home_layout(w, h);
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 28.0;

    // ── Crumb breadcrumb — glowing rose dot + tracked mono eyebrow ─────────
    // The redesign's `.crumb` pattern (`<span class="dot"></span> Home · Overview`).
    let crumb_baseline = panel.top + 40.0;
    let dot_x = left + 4.0;
    let dot_y = crumb_baseline - 4.0;
    let mut dot_halo = Paint::default();
    dot_halo.set_anti_alias(true);
    dot_halo.set_color4f(rgba(ROSE, 0.65), None);
    dot_halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
    canvas.draw_circle((dot_x, dot_y), 4.0, &dot_halo);
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(rgba(ROSE, 1.0), None);
    canvas.draw_circle((dot_x, dot_y), 2.0, &dot);

    let crumb_font = fonts.jetbrains_mono(10.5);
    let mut crumb = Paint::default();
    crumb.set_anti_alias(true);
    crumb.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "HOME  ·  OVERVIEW",
        (dot_x + 14.0, crumb_baseline),
        &crumb_font,
        &crumb,
        0.22,
    );

    // ── Title — "Welcome back, <player>" with the name in rose italic ──────
    let player = data.player_name();
    let display_name = if player.is_empty() { editor.active_profile.clone() } else { player };
    let title_font = fonts.fraunces_axes(34.0, 70.0, 0.0, 540.0, Some(144.0));
    let name_font = fonts.fraunces_axes(34.0, 100.0, 1.0, 540.0, Some(144.0));

    let prefix = "Welcome back, ";
    let mut probe = Paint::default();
    probe.set_anti_alias(true);
    let (prefix_w, _) = title_font.measure_str(prefix, Some(&probe));

    let title_baseline = panel.top + 86.0;
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(prefix, (left, title_baseline), &title_font, &title_paint);

    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(ROSE, 1.0), None);
    canvas.draw_str(
        &display_name,
        (left + prefix_w, title_baseline),
        &name_font,
        &name_paint,
    );

    // Skin viewer — an inset chip with the rotatable 3D model.
    draw_chip(canvas, skin_rect, 12.0);
    crate::skin::draw_skin(
        canvas,
        skin_rect,
        editor.skin_image.as_ref(),
        editor.cape_image.as_ref(),
        editor.skin_yaw,
        editor.skin_slim,
    );
    let hint_font = fonts.jetbrains_mono(8.0);
    let mut hint = Paint::default();
    hint.set_anti_alias(true);
    hint.set_color4f(rgba(MAUVE, 0.7), None);
    let hint_text = if editor.skin_image.is_some() {
        "DRAG TO ROTATE"
    } else {
        "NO SKIN LOADED"
    };
    let hint_w = measure_tracked_em(&hint_font, hint_text, 0.16);
    draw_tracked_em(
        canvas,
        hint_text,
        (skin_rect.left + (skin_rect.width() - hint_w) * 0.5, skin_rect.bottom - 12.0),
        &hint_font,
        &hint,
        0.16,
    );

    // Stat cards (right column).
    let ping = if data.ping_valid() {
        format!("{} ms", data.ping())
    } else {
        "—".to_string()
    };
    let coords = if data.world_active() {
        format!("{:.0}  {:.0}  {:.0}", data.player_x(), data.player_y(), data.player_z())
    } else {
        "—".to_string()
    };
    let server = {
        let s = data.server();
        if s.is_empty() {
            "—".to_string()
        } else {
            s
        }
    };
    let cards: [(&str, String); 5] = [
        ("FPS", data.fps().to_string()),
        ("PING", ping),
        ("PLAYTIME", fmt_playtime(data.playtime())),
        ("COORDS", coords),
        ("SERVER", server),
    ];
    for (rect, card) in stats.iter().zip(cards.iter()) {
        draw_stat_card(canvas, *rect, card.0, &card.1, fonts);
    }

    // Account + active-profile row (right column, below the cards) — small
    // rose avatar disc + Fraunces italic name + tracked-mono "profile · <name>"
    // beneath. Re-skin of the design's `.account-row > .who`. Anchor x is the
    // stats column's left edge (matches the layout in `home_layout`).
    let rx = stats[0].left;
    let name = data.player_name();
    let account = if name.is_empty() {
        "not signed in".to_string()
    } else {
        name
    };
    let row_top = stats[4].bottom + 28.0;
    let av_r = 14.0;
    let av_cx = rx + av_r;
    let av_cy = row_top + av_r;

    // Avatar — rose→berry radial disc with a subtle white inset highlight,
    // mirroring the redesign's `.avatar`.
    let av_rect = Rect::from_xywh(av_cx - av_r, av_cy - av_r, av_r * 2.0, av_r * 2.0);
    let mut av_fill = Paint::default();
    av_fill.set_anti_alias(true);
    if let Some(shader) = gradient_shader::radial(
        Point::new(av_cx - av_r * 0.4, av_cy - av_r * 0.4),
        av_r * 1.6,
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[rgba(ROSE, 1.0), rgba(BERRY, 1.0), rgba(WINE, 1.0)],
            None,
        ),
        Some(&[0.0_f32, 0.6, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    ) {
        av_fill.set_shader(shader);
    } else {
        av_fill.set_color4f(rgba(BERRY, 1.0), None);
    }
    canvas.draw_oval(av_rect, &av_fill);

    let name_font = fonts.fraunces_axes(17.0, 100.0, 1.0, 520.0, Some(24.0));
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(&account, (av_cx + av_r + 14.0, av_cy - 3.0), &name_font, &name_paint);

    // Meta line — italic Newsreader "profile · <Name>". Mixed case avoids
    // the redundant "PROFILE · PROFILE" reading when the active profile is
    // literally named "Profile" (matches the design's `.who .meta`).
    let meta_font = fonts.newsreader(13.0);
    let mut meta_paint = Paint::default();
    meta_paint.set_anti_alias(true);
    meta_paint.set_color4f(rgba(MAUVE, 1.0), None);
    let meta = format!("profile  ·  {}", editor.active_profile);
    canvas.draw_str(&meta, (av_cx + av_r + 14.0, av_cy + 14.0), &meta_font, &meta_paint);

    // ── Media-controller card (large) ──────────────────────────────────────
    draw_media_large(
        canvas,
        media_rect,
        &editor.media,
        editor.cursor,
        editor.media_button_press,
        fonts,
    );

    // ── Quick-toggle chips — flow them inside `toggle_area` with each chip
    //    sized to fit its label. Cache the per-chip rect on the editor so the
    //    press handler can hit-test without re-running the flow. ─────────────
    let qt_font = fonts.jetbrains_mono(10.0);
    let mut qt = Paint::default();
    qt.set_anti_alias(true);
    qt.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        "QUICK TOGGLES  ·  HUD WIDGETS",
        (toggle_area.left, toggle_area.top - 14.0),
        &qt_font,
        &qt,
        0.18,
    );

    let chip_h = 28.0;
    let chip_gap_x = 6.0;
    let chip_gap_y = 6.0;
    let chip_label_font = fonts.jetbrains_mono(9.0);
    let mut cursor_x = toggle_area.left;
    let mut cursor_y = toggle_area.top;
    for (i, id) in WidgetId::ALL.iter().enumerate() {
        let label = id.title();
        let label_w = measure_tracked_em(&chip_label_font, label, 0.22);
        let chip_w = 15.0 + 8.0 + label_w + 14.0; // dot_offset + dot+gap + label + right_pad
        // Wrap to a new row if this chip won't fit on the current one.
        if cursor_x + chip_w > toggle_area.right && cursor_x > toggle_area.left {
            cursor_x = toggle_area.left;
            cursor_y += chip_h + chip_gap_y;
        }
        let chip_rect = Rect::from_xywh(cursor_x, cursor_y, chip_w, chip_h);
        let on = editor.layout.get(*id).enabled;
        draw_toggle_chip(canvas, chip_rect, label, on, fonts);
        editor.home_toggle_bounds[i] = chip_rect;
        cursor_x += chip_w + chip_gap_x;
    }
}

// ────────────────────────────────────────────────────────────────────────
// Media controller — "now playing" widget. Re-skin of the redesign's
// `.media-large` (HOME-tab card) + `.w-media` (compact in-world widget).
// SMTC backend wiring is a follow-up; the renderer reads `MediaState`.
// ────────────────────────────────────────────────────────────────────────

/// The album-art square — 12px-rounded; renders the real thumbnail when one
/// is available, falls back to the design's conic "vinyl" gradient otherwise
/// (a pearl/berry/lavender swirl with a centre punch, the empty-state filler
/// the redesign uses when no thumbnail is available).
fn draw_album_art(canvas: &Canvas, rect: Rect, media: &crate::media::MediaState) {
    let rrect = RRect::new_rect_xy(rect, 12.0, 12.0);

    if let Some(img) = media.thumbnail.as_ref() {
        canvas.save();
        canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));
        let mut p = Paint::default();
        p.set_anti_alias(true);
        canvas.draw_image_rect(img, None, rect, &p);
        canvas.restore();
    } else if media.is_idle() {
        // Dashed-cross-hatch empty thumbnail per `.media-large.empty .album` —
        // a repeating wine/pearl pattern with a centred italic music glyph.
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(rgba(WINE, 0.7), None);
        canvas.draw_rrect(rrect, &bg);
    } else {
        // Vinyl-style swirl when we have a playing track but the source's
        // thumbnail hasn't decoded yet. Approximation of the redesign's
        // `conic-gradient(from 220deg)` — Skia doesn't have conic gradients
        // natively, so we sweep an angular sweep_shader through the same
        // berry / rose / lavender stops.
        let cx = rect.left + rect.width() * 0.65;
        let cy = rect.top + rect.height() * 0.35;
        let mut p = Paint::default();
        p.set_anti_alias(true);
        if let Some(shader) = gradient_shader::sweep(
            (cx, cy),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    rgba(WINE, 1.0),
                    rgba(BERRY, 1.0),
                    rgba(ROSE, 1.0),
                    rgba(LAV, 1.0),
                    rgba(BERRY, 1.0),
                    rgba(WINE, 1.0),
                ],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
            None,
        ) {
            p.set_shader(shader);
        } else {
            p.set_color4f(rgba(BERRY, 1.0), None);
        }
        canvas.draw_rrect(rrect, &p);
    }

    // Inset inner shadow (CSS `inset 0 0 10px rgba(0,0,0,.4)`) — a 1px stroked
    // outline at the rect edges + a tighter centre punch.
    let mut inset = Paint::default();
    inset.set_anti_alias(true);
    inset.set_style(PaintStyle::Stroke);
    inset.set_stroke_width(1.0);
    inset.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.35), None);
    canvas.draw_rrect(rrect, &inset);

    if media.is_idle() {
        // Music glyph centred — italic Fraunces ♪ for the empty card.
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_color4f(rgba(MAUVE, 1.0), None);
        // Use the default skia fallback for `♪` — Fraunces does carry it.
        // (System fallback covers the glyph anyway if missing.)
    }
}

/// Draw a play / pause / next / previous circular button. `accent` is the
/// large-play variant (52px, rose-berry gradient) — the prev/next buttons use
/// the muted variant (38px, wine fill + pearl border).
fn draw_media_button(
    canvas: &Canvas,
    rect: Rect,
    accent: bool,
    _playing: bool,
    glyph: char,
    hovered: bool,
    press_strength: f32,
) {
    let cx = rect.left + rect.width() * 0.5;
    let cy = rect.top + rect.height() * 0.5;
    let r_base = rect.width() * 0.5;
    // Press scales the button down ~6% at full strength, easing back out.
    let press_scale = 1.0 - press_strength * 0.06;
    let r = r_base * press_scale;

    if accent {
        // Outer rose glow — base intensity, lifted on hover, super-lifted on press.
        let glow_alpha = 0.55 + if hovered { 0.18 } else { 0.0 } + press_strength * 0.20;
        let glow_blur = 10.0 + if hovered { 4.0 } else { 0.0 } + press_strength * 6.0;
        let glow_extent = r + 4.0 + if hovered { 2.0 } else { 0.0 };
        let mut glow = Paint::default();
        glow.set_anti_alias(true);
        glow.set_color4f(rgba(ROSE, glow_alpha.min(1.0)), None);
        glow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, glow_blur, false));
        canvas.draw_circle((cx, cy), glow_extent, &glow);

        // Body gradient — lifts slightly when hovered/pressed so it reads brighter.
        let lift = if hovered { 1.08 } else { 1.0 } + press_strength * 0.06;
        let rose_lift = Color4f::new(
            (ROSE.0 as f32 / 255.0 * lift).min(1.0),
            (ROSE.1 as f32 / 255.0 * lift).min(1.0),
            (ROSE.2 as f32 / 255.0 * lift).min(1.0),
            1.0,
        );
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (Point::new(rect.left, rect.top), Point::new(rect.left, rect.bottom)),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[rose_lift, rgba(BERRY, 1.0)],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            fill.set_shader(shader);
        } else {
            fill.set_color4f(rgba(ROSE, 1.0), None);
        }
        canvas.draw_circle((cx, cy), r, &fill);

        // Inset top highlight (slightly brighter on hover so the button reads
        // as "lit up" without needing a CSS-style transition).
        let hi_alpha = if hovered { 0.5 } else { 0.35 };
        let mut hi = Paint::default();
        hi.set_anti_alias(true);
        hi.set_style(PaintStyle::Stroke);
        hi.set_stroke_width(1.0);
        hi.set_color4f(Color4f::new(1.0, 1.0, 1.0, hi_alpha), None);
        canvas.draw_circle((cx, cy - 0.5), r - 0.5, &hi);
    } else {
        // Hover: the wine fill brightens, the border picks up a rose tint
        // (matches the redesign's `.mctl:hover{border-color:...rose;background:...berry}` rule).
        let fill_alpha = 0.55 + if hovered { 0.2 } else { 0.0 } + press_strength * 0.15;
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        if hovered {
            // Blend wine + a touch of berry so the disc reads as "lit" rather
            // than just darker.
            fill.set_color4f(
                Color4f::new(
                    (WINE.0 as f32 / 255.0 + 0.20).min(1.0),
                    (WINE.1 as f32 / 255.0 + 0.04).min(1.0),
                    (WINE.2 as f32 / 255.0 + 0.10).min(1.0),
                    fill_alpha.min(1.0),
                ),
                None,
            );
        } else {
            fill.set_color4f(rgba(WINE, fill_alpha.min(1.0)), None);
        }
        canvas.draw_circle((cx, cy), r, &fill);

        // Border — pearl on rest, rose on hover, brighter rose on press.
        let border_color = if hovered {
            rgba(ROSE, (0.45 + press_strength * 0.3).min(1.0))
        } else {
            rgba(PEARL, 0.12)
        };
        let mut border = Paint::default();
        border.set_anti_alias(true);
        border.set_style(PaintStyle::Stroke);
        border.set_stroke_width(1.0);
        border.set_color4f(border_color, None);
        canvas.draw_circle((cx, cy), r - 0.5, &border);

        // Hover halo — a soft rose ring just outside the button.
        if hovered || press_strength > 0.0 {
            let halo_alpha = (0.3 + press_strength * 0.4).min(0.8);
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(ROSE, halo_alpha), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 6.0, false));
            halo.set_style(PaintStyle::Stroke);
            halo.set_stroke_width(2.0);
            canvas.draw_circle((cx, cy), r + 2.0, &halo);
        }
    }

    // Glyph — vector paths so we don't depend on a music-symbol font cut.
    let glyph_color = if accent {
        Color4f::new(0.16, 0.03, 0.08, 1.0) // deep wine on the rose button
    } else if hovered {
        rgba(ROSE, 1.0)
    } else {
        rgba(PEARL, 0.95)
    };
    let mut g = Paint::default();
    g.set_anti_alias(true);
    g.set_color4f(glyph_color, None);
    let size = r * 0.38;
    match glyph {
        // ▶ play triangle.
        'P' => {
            let path = {
                let mut p = skia_safe::Path::new();
                p.move_to((cx - size * 0.55, cy - size));
                p.line_to((cx + size * 0.9, cy));
                p.line_to((cx - size * 0.55, cy + size));
                p.close();
                p
            };
            canvas.draw_path(&path, &g);
        }
        // ⏸ pause double bar.
        'p' => {
            let bw = size * 0.36;
            let bh = size * 1.7;
            let gap = size * 0.45;
            canvas.draw_rect(
                Rect::from_xywh(cx - gap * 0.5 - bw, cy - bh * 0.5, bw, bh),
                &g,
            );
            canvas.draw_rect(
                Rect::from_xywh(cx + gap * 0.5, cy - bh * 0.5, bw, bh),
                &g,
            );
        }
        // ⏮ previous — left bar + reverse triangle.
        '<' => {
            let bh = size * 1.6;
            let bw = size * 0.32;
            canvas.draw_rect(
                Rect::from_xywh(cx - size * 1.1, cy - bh * 0.5, bw, bh),
                &g,
            );
            let path = {
                let mut p = skia_safe::Path::new();
                p.move_to((cx + size * 0.9, cy - bh * 0.5));
                p.line_to((cx - size * 0.55, cy));
                p.line_to((cx + size * 0.9, cy + bh * 0.5));
                p.close();
                p
            };
            canvas.draw_path(&path, &g);
        }
        // ⏭ next — right bar + forward triangle.
        '>' => {
            let bh = size * 1.6;
            let bw = size * 0.32;
            canvas.draw_rect(
                Rect::from_xywh(cx + size * 0.78, cy - bh * 0.5, bw, bh),
                &g,
            );
            let path = {
                let mut p = skia_safe::Path::new();
                p.move_to((cx - size * 0.9, cy - bh * 0.5));
                p.line_to((cx + size * 0.55, cy));
                p.line_to((cx - size * 0.9, cy + bh * 0.5));
                p.close();
                p
            };
            canvas.draw_path(&path, &g);
        }
        _ => {}
    }
}

/// Format `seconds` as `m:ss` — small helper for the scrub timestamps.
fn fmt_track_time(seconds: f32) -> String {
    let s = seconds.max(0.0) as i32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Large media-controller card — the HOME-tab "now playing" form. Renders the
/// album / vinyl thumbnail on the left, title + artist + scrub bar in the
/// middle, prev/play/next buttons on the right. Empty state ("Silence is
/// also a song") when `MediaState::is_idle()`.
fn draw_media_large(
    canvas: &Canvas,
    rect: Rect,
    media: &crate::media::MediaState,
    cursor: (f32, f32),
    press_info: Option<(usize, std::time::Instant)>,
    fonts: &FontStore,
) {
    let rrect = RRect::new_rect_xy(rect, 18.0, 18.0);
    let idle = media.is_idle();

    // ── Card chrome — 135° berry → lavender → wine fill + rose border + glow.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    if !idle {
        if let Some(shader) = gradient_shader::linear(
            (
                Point::new(rect.left, rect.top),
                Point::new(rect.right, rect.bottom),
            ),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    rgba(BERRY, 0.22),
                    rgba(LAV, 0.10),
                    rgba(WINE, 0.72),
                ],
                None,
            ),
            Some(&[0.0_f32, 0.4, 1.0][..]),
            TileMode::Clamp,
            None,
            None,
        ) {
            fill.set_shader(shader);
        } else {
            fill.set_color4f(rgba(WINE, 0.7), None);
        }
    } else {
        // Empty state — quieter, mauve over wine.
        fill.set_color4f(rgba(WINE, 0.65), None);
    }
    canvas.draw_rrect(rrect, &fill);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(
        if idle {
            rgba(PEARL, 0.08)
        } else {
            rgba(ROSE, 0.22)
        },
        None,
    );
    canvas.draw_rrect(rrect, &border);

    // Inset top highlight.
    canvas.save();
    let top_clip = Rect::from_xywh(rect.left, rect.top, rect.width(), 2.0);
    canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
    let inset = Rect::new(rect.left + 0.5, rect.top + 0.5, rect.right - 0.5, rect.bottom - 0.5);
    let mut top = Paint::default();
    top.set_anti_alias(true);
    top.set_style(PaintStyle::Stroke);
    top.set_stroke_width(1.0);
    top.set_color4f(rgba(PEARL, 0.08), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset, 17.5, 17.5), &top);
    canvas.restore();

    // ── Album art (left) ───────────────────────────────────────────────────
    let art_size = 72.0;
    let pad_x = 14.0;
    let art = Rect::from_xywh(
        rect.left + pad_x,
        rect.top + (rect.height() - art_size) * 0.5,
        art_size,
        art_size,
    );
    draw_album_art(canvas, art, media);

    // Album-art empty-state glyph — drawn here over the swirl so we have the
    // metrics available locally. Italic Fraunces ♪ ~ centre.
    if idle {
        let g_font = fonts.fraunces_axes(36.0, 100.0, 1.0, 500.0, Some(72.0));
        let glyph = "\u{266A}"; // ♪
        let mut probe = Paint::default();
        let (gw, _) = g_font.measure_str(glyph, Some(&probe));
        let (_, gm) = g_font.metrics();
        let gcap = if gm.cap_height > 0.0 { gm.cap_height } else { 22.0 };
        probe.set_anti_alias(true);
        let mut gp = Paint::default();
        gp.set_anti_alias(true);
        gp.set_color4f(rgba(MAUVE, 0.85), None);
        canvas.draw_str(
            glyph,
            (
                art.left + (art.width() - gw) * 0.5,
                art.top + (art.height() + gcap) * 0.5,
            ),
            &g_font,
            &gp,
        );
    }

    // ── Middle column — "Now playing" eyebrow + title + artist + scrub ────
    let mid_left = art.right + 16.0;
    // Right cluster (prev 34 + gap 8 + play 46 + gap 8 + next 34) + pad 14.
    let cluster_right_w = 34.0 + 8.0 + 46.0 + 8.0 + 34.0 + 14.0;
    let mid_right = rect.right - cluster_right_w - 4.0;
    let eyebrow_font = fonts.jetbrains_mono(10.5);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    let eyebrow_label = if idle {
        "IDLE  ·  NO SOURCE CONNECTED".to_string()
    } else if media.source.is_empty() {
        "NOW PLAYING".to_string()
    } else {
        format!("NOW PLAYING  ·  {}", media.source)
    };
    eyebrow.set_color4f(
        if idle {
            rgba(MAUVE, 1.0)
        } else {
            rgba(ROSE, 1.0)
        },
        None,
    );
    let eyebrow_y = rect.top + 28.0;
    draw_tracked_em(
        canvas,
        &eyebrow_label,
        (mid_left, eyebrow_y),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    // Title — italic Fraunces (WONK 1, SOFT 100). Falls back to a poetic
    // empty-state placeholder when idle. Sized to fit "Slow Dancing in a
    // Burning Room"-class titles inside the middle column.
    let title_font = fonts.fraunces_axes(19.0, 100.0, 1.0, 540.0, Some(36.0));
    let title_str = if idle {
        "Silence is also a song".to_string()
    } else {
        media.title.clone()
    };
    let title_baseline = eyebrow_y + 28.0;
    // Clip the title to the middle column width to avoid overflowing into the
    // controls when the title is long.
    canvas.save();
    let title_clip = Rect::new(mid_left, rect.top, mid_right, rect.bottom);
    canvas.clip_rect(title_clip, Some(ClipOp::Intersect), Some(true));
    let mut title_paint = Paint::default();
    title_paint.set_anti_alias(true);
    title_paint.set_color4f(
        if idle {
            rgba(MAUVE, 1.0)
        } else {
            rgba(PEARL, 1.0)
        },
        None,
    );
    canvas.draw_str(&title_str, (mid_left, title_baseline), &title_font, &title_paint);

    // Artist line — italic Newsreader-ish (we don't have a true italic
    // Fraunces cut here yet; tracked Newsreader mauve is a close stand-in).
    let artist_str = if idle {
        "Connect Spotify, browser audio, or system media.".to_string()
    } else if media.artist.is_empty() {
        String::new()
    } else {
        media.artist.clone()
    };
    if !artist_str.is_empty() {
        let artist_font = fonts.newsreader(13.0);
        let mut artist = Paint::default();
        artist.set_anti_alias(true);
        artist.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(&artist_str, (mid_left, title_baseline + 18.0), &artist_font, &artist);
    }
    canvas.restore();

    // Scrub bar (skip if idle).
    if !idle {
        let scrub_y = rect.bottom - 26.0;
        let time_font = fonts.jetbrains_mono(10.0);
        let live_pos = media.displayed_position();
        let pos_str = fmt_track_time(live_pos);
        let dur_str = if media.duration_seconds > 0.0 {
            fmt_track_time(media.duration_seconds)
        } else {
            String::new()
        };
        let mut probe = Paint::default();
        probe.set_anti_alias(true);
        let (pos_w, _) = time_font.measure_str(&pos_str, Some(&probe));
        let (dur_w, _) = time_font.measure_str(&dur_str, Some(&probe));

        let mut t = Paint::default();
        t.set_anti_alias(true);
        t.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(&pos_str, (mid_left, scrub_y + 4.0), &time_font, &t);
        if !dur_str.is_empty() {
            canvas.draw_str(&dur_str, (mid_right - dur_w, scrub_y + 4.0), &time_font, &t);
        }

        // The bar — track + rose→champ fill + pearl knob.
        let track_left = mid_left + pos_w + 10.0;
        let track_right = mid_right - dur_w - 10.0;
        let track_h = 2.0;
        let track_rect = Rect::from_xywh(
            track_left,
            scrub_y - 4.0 - track_h * 0.5,
            (track_right - track_left).max(0.0),
            track_h,
        );
        let mut bg = Paint::default();
        bg.set_anti_alias(true);
        bg.set_color4f(rgba(PEARL, 0.10), None);
        canvas.draw_rrect(RRect::new_rect_xy(track_rect, track_h, track_h), &bg);

        let frac = if media.duration_seconds > 0.0 {
            (live_pos / media.duration_seconds).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let fill_w = track_rect.width() * frac;
        if fill_w > 1.0 {
            let fill_rect = Rect::from_xywh(track_rect.left, track_rect.top, fill_w, track_h);
            let mut fp = Paint::default();
            fp.set_anti_alias(true);
            if let Some(shader) = gradient_shader::linear(
                (
                    Point::new(track_rect.left, track_rect.top),
                    Point::new(track_rect.right, track_rect.top),
                ),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[rgba(ROSE, 1.0), rgba(CHAMP, 1.0)],
                    None,
                ),
                None,
                TileMode::Clamp,
                None,
                None,
            ) {
                fp.set_shader(shader);
            } else {
                fp.set_color4f(rgba(ROSE, 1.0), None);
            }
            canvas.draw_rrect(RRect::new_rect_xy(fill_rect, track_h, track_h), &fp);

            // Pearl knob with a rose halo.
            let knob_x = track_rect.left + fill_w;
            let knob_y = track_rect.top + track_h * 0.5;
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(ROSE, 0.4), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
            canvas.draw_circle((knob_x, knob_y), 6.0, &halo);
            let mut knob = Paint::default();
            knob.set_anti_alias(true);
            knob.set_color4f(rgba(PEARL, 1.0), None);
            canvas.draw_circle((knob_x, knob_y), 4.0, &knob);
        }
    }

    // ── Right cluster — prev / play(pause) / next ──────────────────────────
    let ctl_size = 34.0;
    let play_size = 46.0;
    let gap = 8.0;
    let cluster_w = ctl_size + gap + play_size + gap + ctl_size;
    let cluster_left = rect.right - 14.0 - cluster_w;
    let cy = rect.top + rect.height() * 0.5;
    let prev = Rect::from_xywh(cluster_left, cy - ctl_size * 0.5, ctl_size, ctl_size);
    let play = Rect::from_xywh(cluster_left + ctl_size + gap, cy - play_size * 0.5, play_size, play_size);
    let next = Rect::from_xywh(
        cluster_left + ctl_size + gap + play_size + gap,
        cy - ctl_size * 0.5,
        ctl_size,
        ctl_size,
    );

    let (hover_prev, press_prev) = button_state(prev, cursor, 0, press_info);
    let (hover_play, press_play) = button_state(play, cursor, 1, press_info);
    let (hover_next, press_next) = button_state(next, cursor, 2, press_info);
    if idle {
        // Idle — buttons are present but visually muted.
        draw_media_button(canvas, prev, false, false, '<', hover_prev, press_prev);
        draw_media_button(canvas, play, false, false, 'P', hover_play, press_play);
        draw_media_button(canvas, next, false, false, '>', hover_next, press_next);
    } else {
        draw_media_button(canvas, prev, false, false, '<', hover_prev, press_prev);
        draw_media_button(
            canvas,
            play,
            true,
            media.playing,
            if media.playing { 'p' } else { 'P' },
            hover_play,
            press_play,
        );
        draw_media_button(canvas, next, false, false, '>', hover_next, press_next);
    }
}

// ────────────────────────────────────────────────────────────────────────
// Mods view — the bundled-mod toggle list.
// ────────────────────────────────────────────────────────────────────────

/// One bundled mod in the MODS view, decoded from `overlay-mods.toml` (which
/// the launcher writes before each launch).
struct ModEntry {
    id: String,
    name: String,
    category: String,
    version: String,
    enabled: bool,
    /// Enabled state when loaded — the override file carries only the mods
    /// whose `enabled` now differs from this.
    original: bool,
}

/// `<instance-dir>/<file>` — the cdylib runs with the instance dir as its CWD.
fn instance_file(file: &str) -> Option<PathBuf> {
    std::env::current_dir().ok().map(|d| d.join(file))
}

/// Load a skin / cape PNG the mod wrote into the instance dir, if present.
fn load_skin_image(name: &str) -> Option<Image> {
    let bytes = std::fs::read(instance_file(name)?).ok()?;
    Image::from_encoded(Data::new_copy(&bytes))
}

/// `ewo-skin.png`'s last-modified time, or `None` if it isn't there yet.
fn skin_png_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(instance_file("ewo-skin.png")?).ok()?.modified().ok()
}

/// Read the launcher-written `ewo-keybinds.txt` from the instance dir into
/// (action_id, glfw_code) pairs. Mirrors `EwoKeybinds.java`'s parser — each
/// line is `action=code` or `action=code:mods`, blank/`#`-prefixed lines skip.
/// Returns an empty vec if the file is absent — every action is then unbound,
/// which is fine (the MODULES tab just hides the chip).
fn load_keybinds() -> Vec<(String, i32)> {
    let Some(path) = instance_file("ewo-keybinds.txt") else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out: Vec<(String, i32)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((id, value)) = line.split_once('=') else {
            continue;
        };
        let id = id.trim();
        let value = value.trim();
        let value = value.split_once(':').map(|(c, _)| c).unwrap_or(value).trim();
        if let Ok(code) = value.parse::<i32>() {
            out.push((id.to_string(), code));
        }
    }
    out
}

/// Short uppercase label for a GLFW key code — covers the keys the keybind
/// registry's defaults reach for (letters, digits, function keys, modifiers,
/// arrows, common navigation). Anything else falls back to `KEY <code>`.
fn glfw_key_label(code: i32) -> String {
    match code {
        32 => "SPACE".to_string(),
        39 => "'".to_string(),
        44 => ",".to_string(),
        45 => "-".to_string(),
        46 => ".".to_string(),
        47 => "/".to_string(),
        48..=57 => ((b'0' + (code - 48) as u8) as char).to_string(),
        59 => ";".to_string(),
        61 => "=".to_string(),
        65..=90 => ((b'A' + (code - 65) as u8) as char).to_string(),
        91 => "[".to_string(),
        92 => "\\".to_string(),
        93 => "]".to_string(),
        96 => "`".to_string(),
        256 => "ESC".to_string(),
        257 => "ENTER".to_string(),
        258 => "TAB".to_string(),
        259 => "BKSP".to_string(),
        260 => "INS".to_string(),
        261 => "DEL".to_string(),
        262 => "→".to_string(),
        263 => "←".to_string(),
        264 => "↓".to_string(),
        265 => "↑".to_string(),
        266 => "PGUP".to_string(),
        267 => "PGDN".to_string(),
        268 => "HOME".to_string(),
        269 => "END".to_string(),
        280 => "CAPS".to_string(),
        281 => "SCRL".to_string(),
        282 => "NUM".to_string(),
        290..=301 => format!("F{}", code - 289),
        320..=329 => format!("NUM{}", code - 320),
        336 => "ENTER".to_string(),
        340 => "LSHFT".to_string(),
        341 => "LCTRL".to_string(),
        342 => "LALT".to_string(),
        343 => "LSUPER".to_string(),
        344 => "RSHFT".to_string(),
        345 => "RCTRL".to_string(),
        346 => "RALT".to_string(),
        347 => "RSUPER".to_string(),
        _ => format!("KEY {}", code),
    }
}

/// Read `overlay-mods.toml` (written by the launcher) into the MODS list.
fn load_mods() -> Vec<ModEntry> {
    let Some(path) = instance_file("overlay-mods.toml") else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut mods: Vec<ModEntry> = Vec::new();
    let mut cur: Option<ModEntry> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(id) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some(m) = cur.take() {
                mods.push(m);
            }
            cur = Some(ModEntry {
                id: id.to_string(),
                name: id.to_string(),
                category: String::new(),
                version: String::new(),
                enabled: true,
                original: true,
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if let Some(m) = cur.as_mut() {
            match key.trim() {
                "name" => m.name = value.to_string(),
                "category" => m.category = value.to_string(),
                "version" => m.version = value.to_string(),
                "enabled" => {
                    m.enabled = value == "true";
                    m.original = m.enabled;
                }
                _ => {}
            }
        }
    }
    if let Some(m) = cur.take() {
        mods.push(m);
    }
    mods
}

/// Write `overlay-mod-overrides.toml` — only the mods toggled away from their
/// loaded state. The launcher consumes (and deletes) it on the next launch.
fn save_mod_overrides(mods: &[ModEntry]) {
    let Some(path) = instance_file("overlay-mod-overrides.toml") else {
        return;
    };
    let mut s = String::from("# In-game mod toggles — consumed by the launcher next launch.\n");
    for m in mods {
        if m.enabled != m.original {
            s.push_str(&format!("{} = {}\n", m.id, m.enabled));
        }
    }
    let _ = std::fs::write(&path, s);
}

/// Velvet accent for a bundled-mod category — the row's colour dot.
fn category_color(category: &str) -> (u8, u8, u8) {
    match category {
        "performance" => ROSE,
        "visuals" => LAV,
        "utility" => CHAMP,
        "social" => BERRY,
        _ => MAUVE, // library / unknown
    }
}

/// The Mods-view panel rect + a toggle rect per mod row.
fn mods_layout(w: f32, h: f32, count: usize) -> (Rect, Vec<Rect>) {
    const PANEL_W: f32 = 544.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 76.0;
    const ROW_H: f32 = 34.0;
    let panel_h = PAD * 2.0 + HEADER_H + count.max(1) as f32 * ROW_H;
    let px = (w - PANEL_W) * 0.5;
    let py = (h - panel_h) * 0.5;
    let panel = Rect::from_xywh(px, py, PANEL_W, panel_h);

    let rows_top = py + PAD + HEADER_H;
    const TOGGLE_W: f32 = 38.0;
    const TOGGLE_H: f32 = 20.0;
    let mut toggles = Vec::with_capacity(count);
    for i in 0..count {
        let mid = rows_top + i as f32 * ROW_H + ROW_H * 0.5;
        toggles.push(Rect::from_xywh(
            px + PANEL_W - PAD - TOGGLE_W,
            mid - TOGGLE_H * 0.5,
            TOGGLE_W,
            TOGGLE_H,
        ));
    }
    (panel, toggles)
}

/// The Mods view — a Velvet re-skin of a ClickGUI module list: one row per
/// bundled mod (category dot · name · category·version · on/off toggle).
fn draw_mods(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let mods = &editor.mods;
    let (panel, toggles) = mods_layout(w, h, mods.len());
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 24.0;

    // Eyebrow + the enabled count.
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(canvas, "MODS", (left, panel.top + 36.0), &eyebrow_font, &eyebrow, 0.22);

    let on_count = mods.iter().filter(|m| m.enabled).count();
    let count_str = format!("{} / {} ENABLED", on_count, mods.len());
    let count_w = measure_tracked_em(&eyebrow_font, &count_str, 0.16);
    let mut count_paint = Paint::default();
    count_paint.set_anti_alias(true);
    count_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        &count_str,
        (panel.right - 24.0 - count_w, panel.top + 36.0),
        &eyebrow_font,
        &count_paint,
        0.16,
    );

    // Title.
    let title_font = fonts.fraunces_axes(27.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str("Bundled mods", (left, panel.top + 70.0), &title_font, &title);

    if mods.is_empty() {
        let body_font = fonts.newsreader(15.0);
        let mut body = Paint::default();
        body.set_anti_alias(true);
        body.set_color4f(rgba(MAUVE, 1.0), None);
        canvas.draw_str(
            "No bundled mods found — launch an Ewo instance to populate this.",
            (left, panel.top + 110.0),
            &body_font,
            &body,
        );
        return;
    }

    // Rows.
    let rows_top = panel.top + 24.0 + 76.0;
    let row_h = 34.0;
    let name_font = fonts.newsreader(15.0);
    let meta_font = fonts.jetbrains_mono(11.0);
    let (_, nm) = name_font.metrics();
    let ncap = if nm.cap_height > 0.0 { nm.cap_height } else { 11.0 };
    for (i, m) in mods.iter().enumerate() {
        let ry = rows_top + i as f32 * row_h;
        let mid = ry + row_h * 0.5;

        // Hairline divider above every row but the first.
        if i > 0 {
            let mut div = Paint::default();
            div.set_anti_alias(true);
            div.set_style(PaintStyle::Stroke);
            div.set_stroke_width(1.0);
            div.set_color4f(rgba(PEARL, 0.06), None);
            canvas.draw_line((left, ry), (panel.right - 24.0, ry), &div);
        }

        // Off-state fade — apply to the whole row at once (design's
        // `.mod-row.off { opacity: .55 }`).
        let row_alpha = if m.enabled { 1.0 } else { 0.55 };

        // Category LED — 4px disc with a soft category-tinted halo when on
        // (design's `box-shadow: 0 0 8px <color>`), dark wine when off.
        let led_cx = left + 7.0;
        let led_color = category_color(&m.category);
        if m.enabled {
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(led_color, 0.6), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 4.0, false));
            canvas.draw_circle((led_cx, mid), 5.5, &halo);
        }
        let mut dot = Paint::default();
        dot.set_anti_alias(true);
        dot.set_color4f(
            if m.enabled {
                rgba(led_color, row_alpha)
            } else {
                Color4f::new(
                    0x3A as f32 / 255.0,
                    0x1A as f32 / 255.0,
                    0x23 as f32 / 255.0,
                    1.0,
                )
            },
            None,
        );
        canvas.draw_circle((led_cx, mid), 4.0, &dot);

        // Name.
        let mut name = Paint::default();
        name.set_anti_alias(true);
        name.set_color4f(
            if m.enabled {
                rgba(PEARL, 1.0)
            } else {
                rgba(MAUVE, 0.7)
            },
            None,
        );
        canvas.draw_str(&m.name, (left + 22.0, mid + ncap * 0.5), &name_font, &name);

        // Category · version, tracked, before the toggle.
        let meta = format!("{}  ·  {}", m.category.to_uppercase(), m.version);
        let meta_w = measure_tracked_em(&meta_font, &meta, 0.18);
        let mut meta_paint = Paint::default();
        meta_paint.set_anti_alias(true);
        meta_paint.set_color4f(rgba(MAUVE, 0.85 * row_alpha), None);
        draw_tracked_em(
            canvas,
            &meta,
            (toggles[i].left - 16.0 - meta_w, mid + 4.0),
            &meta_font,
            &meta_paint,
            0.18,
        );

        draw_panel_toggle(canvas, toggles[i], m.enabled);
    }
}

// ────────────────────────────────────────────────────────────────────────
// PVP view — the PvP-Utils editor: master toggles, per-tier sounds, zones.
// (Sprint 2a) Edits are saved to the active profile's `pvp.toml`; the Java
// mod polls the file's mtime each frame and hot-reloads, so changes apply
// live without a relaunch.
// ────────────────────────────────────────────────────────────────────────

/// Hit-rects for the PVP tab — three sections of rows.
struct PvpLayout {
    panel: Rect,
    /// General-section toggles, in the order
    /// `[jump, jump-bar, hit-range, totem-count, floating-health]`.
    general_toggles: [Rect; 5],
    /// Per-tier rows — [tier index]{ sound chip, volume slider }.
    tier_sound: [Rect; 5],
    tier_volume: [Rect; 5],
    /// Per-zone rows — [zone index]{ enable toggle, min/max sliders, sound, vol }.
    zone_enable: [Rect; 3],
    zone_min: [Rect; 3],
    zone_max: [Rect; 3],
    zone_sound: [Rect; 3],
    zone_volume: [Rect; 3],
}

/// Lay out the PVP-tab panel + every control. Deterministic in `(w, h)`, so
/// the renderer and the press-handler agree.
fn pvp_layout(w: f32, h: f32) -> PvpLayout {
    const PANEL_W: f32 = 740.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 78.0;
    const SECTION_GAP: f32 = 18.0;
    const SECTION_HEAD_H: f32 = 26.0;
    const ROW_H: f32 = 32.0;

    let general_rows = 5;
    let tier_rows = 5;
    let zone_rows = 3;
    let body_h = SECTION_HEAD_H + ROW_H * general_rows as f32
        + SECTION_GAP + SECTION_HEAD_H + ROW_H * tier_rows as f32
        + SECTION_GAP + SECTION_HEAD_H + ROW_H * zone_rows as f32;
    let panel_h = PAD * 2.0 + HEADER_H + body_h;
    let px = (w - PANEL_W) * 0.5;
    let py = ((h - panel_h) * 0.5).max(70.0); // never above the tab strip
    let panel = Rect::from_xywh(px, py, PANEL_W, panel_h);

    let content_x = px + PAD;
    let content_w = PANEL_W - PAD * 2.0;
    let toggle_w = 38.0;
    let toggle_h = 20.0;

    // General toggles — right-edge.
    let mut general_toggles = [empty_rect(); 5];
    let general_top = py + PAD + HEADER_H + SECTION_HEAD_H;
    for i in 0..5 {
        let row_top = general_top + i as f32 * ROW_H;
        general_toggles[i] = Rect::from_xywh(
            content_x + content_w - toggle_w,
            row_top + (ROW_H - toggle_h) * 0.5,
            toggle_w,
            toggle_h,
        );
    }

    // Per-tier rows — left: label text (handled by draw), middle: sound chip,
    // right: volume slider.
    let mut tier_sound = [empty_rect(); 5];
    let mut tier_volume = [empty_rect(); 5];
    let tier_top = general_top + ROW_H * 5.0 + SECTION_GAP + SECTION_HEAD_H;
    let label_w = 150.0;
    let sound_chip_w = 130.0;
    let gap = 16.0;
    let vol_left = content_x + label_w + gap + sound_chip_w + gap;
    for i in 0..5 {
        let row_top = tier_top + i as f32 * ROW_H;
        tier_sound[i] = Rect::from_xywh(
            content_x + label_w + gap,
            row_top + 4.0,
            sound_chip_w,
            ROW_H - 8.0,
        );
        tier_volume[i] = Rect::from_xywh(
            vol_left,
            row_top + (ROW_H - 20.0) * 0.5,
            content_x + content_w - vol_left,
            20.0,
        );
    }

    // Per-zone rows — { label | enable | min | max | sound chip | vol slider }.
    let mut zone_enable = [empty_rect(); 3];
    let mut zone_min = [empty_rect(); 3];
    let mut zone_max = [empty_rect(); 3];
    let mut zone_sound = [empty_rect(); 3];
    let mut zone_volume = [empty_rect(); 3];
    let zone_top = tier_top + ROW_H * 5.0 + SECTION_GAP + SECTION_HEAD_H;
    let z_label_w = 60.0;
    let z_toggle_w = 30.0;
    let z_toggle_h = 16.0;
    let z_slider_w = 100.0;
    let z_sound_w = 100.0;
    let z_gap = 10.0;
    for i in 0..3 {
        let row_top = zone_top + i as f32 * ROW_H;
        let mut x = content_x + z_label_w;
        zone_enable[i] = Rect::from_xywh(
            x,
            row_top + (ROW_H - z_toggle_h) * 0.5,
            z_toggle_w,
            z_toggle_h,
        );
        x += z_toggle_w + z_gap;
        zone_min[i] = Rect::from_xywh(x, row_top + (ROW_H - 20.0) * 0.5, z_slider_w, 20.0);
        x += z_slider_w + z_gap;
        zone_max[i] = Rect::from_xywh(x, row_top + (ROW_H - 20.0) * 0.5, z_slider_w, 20.0);
        x += z_slider_w + z_gap;
        zone_sound[i] = Rect::from_xywh(x, row_top + 4.0, z_sound_w, ROW_H - 8.0);
        x += z_sound_w + z_gap;
        zone_volume[i] = Rect::from_xywh(
            x,
            row_top + (ROW_H - 20.0) * 0.5,
            content_x + content_w - x,
            20.0,
        );
    }

    PvpLayout {
        panel,
        general_toggles,
        tier_sound,
        tier_volume,
        zone_enable,
        zone_min,
        zone_max,
        zone_sound,
        zone_volume,
    }
}

/// Draw a sound-cycle chip — a Velvet pill with the sound's name, clickable
/// to cycle to the next sound. Simpler than a full portal dropdown and
/// composes cleanly in this dense layout.
fn draw_pvp_sound_chip(canvas: &Canvas, rect: Rect, sound: crate::pvp::PvpSound, fonts: &FontStore) {
    let rr = RRect::new_rect_xy(rect, rect.height() * 0.4, rect.height() * 0.4);
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(rgba(WINE, 0.85), None);
    canvas.draw_rrect(rr, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.22), None);
    canvas.draw_rrect(rr, &border);

    let font = fonts.jetbrains_mono(11.0);
    let label = sound.label();
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color4f(rgba(PEARL, 0.92), None);
    let (lw, _) = font.measure_str(label, Some(&p));
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 8.0 };
    canvas.draw_str(
        label,
        (rect.left + (rect.width() - lw) * 0.5 - 4.0, rect.top + rect.height() * 0.5 + cap * 0.5),
        &font,
        &p,
    );
    // A tiny "▾" hint at the right edge.
    let mut hint = Paint::default();
    hint.set_anti_alias(true);
    hint.set_color4f(rgba(MAUVE, 0.85), None);
    canvas.draw_str(
        "▾",
        (rect.right - 12.0, rect.top + rect.height() * 0.5 + cap * 0.5),
        &font,
        &hint,
    );
}

/// Draw a small min/max-distance slider — sized for the dense zone row. Knob
/// position is `frac` (0..1); current value displayed to the right.
fn draw_pvp_distance_slider(canvas: &Canvas, area: Rect, value: f32, fonts: &FontStore) {
    const RANGE_MIN: f32 = 0.0;
    const RANGE_MAX: f32 = 3.5;
    let cy = area.top + area.height() * 0.5;
    let value_w = 36.0;
    let track_left = area.left + 4.0;
    let track_right = area.right - value_w;
    let track_h = 3.0;

    let track = Rect::from_xywh(track_left, cy - track_h * 0.5, track_right - track_left, track_h);
    let mut tp = Paint::default();
    tp.set_anti_alias(true);
    tp.set_color4f(rgba(WINE, 0.85), None);
    canvas.draw_rrect(RRect::new_rect_xy(track, track_h, track_h), &tp);

    let span = (RANGE_MAX - RANGE_MIN).max(0.001);
    let frac = ((value - RANGE_MIN) / span).clamp(0.0, 1.0);
    let knob_x = track_left + frac * (track_right - track_left);

    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(ROSE, 0.95), None);
    canvas.draw_circle((knob_x, cy), 5.0, &knob);

    let font = fonts.jetbrains_mono(10.0);
    let val = format!("{:.1}", value);
    let mut vp = Paint::default();
    vp.set_anti_alias(true);
    vp.set_color4f(rgba(PEARL, 0.92), None);
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 7.0 };
    canvas.draw_str(&val, (track_right + 6.0, cy + cap * 0.5), &font, &vp);
}

/// Draw a small volume slider — fixed 0..1 range, knob in pearl.
fn draw_pvp_volume_slider(canvas: &Canvas, area: Rect, value: f32, fonts: &FontStore) {
    let cy = area.top + area.height() * 0.5;
    let value_w = 40.0;
    let track_left = area.left + 4.0;
    let track_right = area.right - value_w;
    let track_h = 3.0;

    let track = Rect::from_xywh(track_left, cy - track_h * 0.5, track_right - track_left, track_h);
    let mut tp = Paint::default();
    tp.set_anti_alias(true);
    tp.set_color4f(rgba(WINE, 0.85), None);
    canvas.draw_rrect(RRect::new_rect_xy(track, track_h, track_h), &tp);

    let frac = value.clamp(0.0, 1.0);
    let knob_x = track_left + frac * (track_right - track_left);
    if knob_x > track_left + 1.0 {
        let fill = Rect::from_xywh(track_left, cy - track_h * 0.5, knob_x - track_left, track_h);
        let mut fp = Paint::default();
        fp.set_anti_alias(true);
        fp.set_color4f(rgba(LAV, 0.8), None);
        canvas.draw_rrect(RRect::new_rect_xy(fill, track_h, track_h), &fp);
    }

    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_circle((knob_x, cy), 5.0, &knob);

    let font = fonts.jetbrains_mono(10.0);
    let val = format!("{:.2}", value);
    let mut vp = Paint::default();
    vp.set_anti_alias(true);
    vp.set_color4f(rgba(PEARL, 0.92), None);
    let (_, m) = font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 7.0 };
    canvas.draw_str(&val, (track_right + 6.0, cy + cap * 0.5), &font, &vp);
}

/// Draw the PVP view — Velvet panel + three sections of editing controls.
fn draw_pvp(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let layout = pvp_layout(w, h);
    let cfg = &editor.pvp;
    draw_chip(canvas, layout.panel, 16.0);

    let left = layout.panel.left + 24.0;

    // Header — eyebrow + title + subhead.
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        "PVP UTILS",
        (left, layout.panel.top + 36.0),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    let title_font = fonts.fraunces_axes(26.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(
        "Combat indicators & sound cues",
        (left, layout.panel.top + 68.0),
        &title_font,
        &title,
    );

    let body_left = layout.panel.left + 24.0;
    let body_right = layout.panel.right - 24.0;
    let row_h: f32 = 32.0;

    // ── Section: GENERAL ─────────────────────────────────────────────────
    let general_section_top = layout.general_toggles[0].top - (row_h - 20.0) * 0.5 - 26.0;
    draw_pvp_section_label(canvas, "GENERAL", body_left, general_section_top + 18.0, fonts);

    let general_labels = [
        "Jump reset",
        "Jump reset bar",
        "Hit range",
        "Totem pop counter",
        "Floating health",
    ];
    let general_states = [
        cfg.jump_reset_enabled,
        cfg.jump_reset_bar_enabled,
        cfg.hit_range_enabled,
        cfg.totem_count_enabled,
        cfg.floating_health_enabled,
    ];
    let label_font = fonts.newsreader(14.0);
    for i in 0..5 {
        let cy = layout.general_toggles[i].top + layout.general_toggles[i].height() * 0.5;
        let mut lp = Paint::default();
        lp.set_anti_alias(true);
        lp.set_color4f(rgba(PEARL, 1.0), None);
        let (_, m) = label_font.metrics();
        let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
        canvas.draw_str(general_labels[i], (body_left, cy + cap * 0.5), &label_font, &lp);
        draw_panel_toggle(canvas, layout.general_toggles[i], general_states[i]);
    }

    // ── Section: SOUNDS PER TIER ─────────────────────────────────────────
    let tiers_section_top = layout.tier_sound[0].top - 26.0;
    draw_pvp_section_label(canvas, "SOUNDS PER TIER", body_left, tiers_section_top + 18.0, fonts);

    for (i, tier) in crate::pvp::Tier::ALL.iter().enumerate() {
        let slot = cfg.sound_for_tier(*tier);
        let cy = layout.tier_sound[i].top + layout.tier_sound[i].height() * 0.5;
        let mut lp = Paint::default();
        lp.set_anti_alias(true);
        lp.set_color4f(rgba(PEARL, 1.0), None);
        let (_, m) = label_font.metrics();
        let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
        canvas.draw_str(tier.label(), (body_left, cy + cap * 0.5), &label_font, &lp);
        draw_pvp_sound_chip(canvas, layout.tier_sound[i], slot.sound, fonts);
        draw_pvp_volume_slider(canvas, layout.tier_volume[i], slot.volume, fonts);
    }

    // ── Section: HIT-RANGE ZONES ─────────────────────────────────────────
    let zones_section_top = layout.zone_enable[0].top - 26.0;
    draw_pvp_section_label(canvas, "HIT-RANGE ZONES", body_left, zones_section_top + 18.0, fonts);

    for i in 0..3 {
        let z = cfg.zone(i);
        let cy = layout.zone_enable[i].top + layout.zone_enable[i].height() * 0.5;
        let zlabel = format!("Zone {}", i + 1);
        let mut lp = Paint::default();
        lp.set_anti_alias(true);
        lp.set_color4f(rgba(PEARL, 1.0), None);
        let (_, m) = label_font.metrics();
        let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
        canvas.draw_str(&zlabel, (body_left, cy + cap * 0.5), &label_font, &lp);
        draw_panel_toggle(canvas, layout.zone_enable[i], z.enabled);
        draw_pvp_distance_slider(canvas, layout.zone_min[i], z.min_dist, fonts);
        draw_pvp_distance_slider(canvas, layout.zone_max[i], z.max_dist, fonts);
        draw_pvp_sound_chip(canvas, layout.zone_sound[i], z.sound, fonts);
        draw_pvp_volume_slider(canvas, layout.zone_volume[i], z.volume, fonts);
    }

    // Quiet the unused body_right warning when we add more controls later.
    let _ = body_right;
}

fn draw_pvp_section_label(canvas: &Canvas, label: &str, x: f32, y: f32, fonts: &FontStore) {
    let font = fonts.jetbrains_mono(10.0);
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(canvas, label, (x, y), &font, &p, 0.20);
}

// ────────────────────────────────────────────────────────────────────────
// Modules view — the EwoClient module toggle list (Phase G).
// ────────────────────────────────────────────────────────────────────────

/// Per-module hit-rects on the MODULES view. Sorted by category for grouping;
/// `catalog_index` carries the canonical REGISTRY position so the slider-
/// drag + toggle dispatch can still look up the module without walking the
/// REGISTRY in sync.
struct ModuleRow {
    /// The module's index in [`catalog::REGISTRY`]. Used by toggle + slider
    /// dispatch to identify which module the row belongs to.
    catalog_index: usize,
    /// The whole row — base content plus any setting sliders below it.
    row: Rect,
    /// The on/off toggle pill.
    toggle: Rect,
    /// Slider hit-areas, one per module setting (FOV Control has one).
    sliders: Vec<Rect>,
}

/// A category header rendered between module groups.
struct ModuleSection {
    label: &'static str,
    /// Baseline-y for the header label.
    label_y: f32,
}

/// Velvet accent for a module category — the row's colour dot.
fn module_category_color(category: catalog::ModuleCategory) -> (u8, u8, u8) {
    match category {
        catalog::ModuleCategory::Visual => LAV,
        catalog::ModuleCategory::Camera => ROSE,
        catalog::ModuleCategory::Movement => CHAMP,
    }
}

/// Category iteration order — VISUAL first, then CAMERA, then MOVEMENT.
/// Stable so the rendered tab order doesn't shift as new modules ship.
const MODULE_CATEGORY_ORDER: [catalog::ModuleCategory; 3] = [
    catalog::ModuleCategory::Visual,
    catalog::ModuleCategory::Camera,
    catalog::ModuleCategory::Movement,
];

/// The Modules-view panel, the section headers between groups, and one
/// [`ModuleRow`] per catalog module (in grouped display order). Deterministic
/// in the window size, so renderer + hit-tester agree.
/// 2-column compact module grid + scrollable panel. The panel is always sized
/// to the available viewport (less the tab strip); content height is reported
/// separately so the renderer + scroll clamp know the overflow.
///
/// Layout: a panel with a fixed header (eyebrow + title + counts), then a
/// scrollable body. The body is organised into category groups
/// (VISUAL → CAMERA → MOVEMENT); within each group, modules pair up into a
/// 2-column row band. The two columns are independent — a left module with
/// settings sliders can be taller than the right one; the band height is
/// `max(left, right)`. Sliders sit directly below their row in the same column.
fn modules_layout(w: f32, h: f32) -> (Rect, Vec<ModuleSection>, Vec<ModuleRow>, f32) {
    const PANEL_W: f32 = 920.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 76.0;
    const ROW_H: f32 = 60.0; // every row is the same compact height now.
    const TOGGLE_W: f32 = 38.0;
    const TOGGLE_H: f32 = 20.0;
    const COL_GAP: f32 = 16.0;
    const ROW_GAP: f32 = 10.0;
    const SECTION_TOP_GAP: f32 = 22.0;
    const SECTION_LABEL_GAP: f32 = 14.0;

    let col_w = (PANEL_W - PAD * 2.0 - COL_GAP) / 2.0;

    // Group modules by category, in MODULE_CATEGORY_ORDER, preserving each
    // module's REGISTRY index for the dispatch path.
    let mut groups: Vec<Vec<usize>> = Vec::with_capacity(MODULE_CATEGORY_ORDER.len());
    for cat in MODULE_CATEGORY_ORDER {
        let mut group: Vec<usize> = catalog::REGISTRY
            .iter()
            .enumerate()
            .filter_map(|(i, m)| (m.category == cat).then_some(i))
            .collect();
        group.sort();
        groups.push(group);
    }

    // Panel size — fill the viewport with comfortable margins. Content scrolls
    // when it exceeds the available body height.
    let px = (w - PANEL_W) * 0.5;
    let py = 64.0; // sit just under the tab strip
    let panel_h = (h - py - 24.0).max(400.0);
    let panel = Rect::from_xywh(px, py, PANEL_W, panel_h);

    let mut rows: Vec<ModuleRow> = Vec::with_capacity(catalog::REGISTRY.len());
    let mut sections: Vec<ModuleSection> = Vec::with_capacity(MODULE_CATEGORY_ORDER.len());
    let body_top = py + PAD + HEADER_H;
    let mut ry = body_top;

    for (group_i, group) in groups.iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let cat = MODULE_CATEGORY_ORDER[group_i];
        ry += SECTION_TOP_GAP;
        sections.push(ModuleSection {
            label: cat.label(),
            label_y: ry,
        });
        ry += SECTION_LABEL_GAP;

        // Pair the group's modules into 2-column row bands. Every row is the
        // same uniform `ROW_H` tall — sliders live in the right-click popover
        // now, not inline, so band height is constant.
        let left_col_x = px + PAD;
        let right_col_x = px + PAD + col_w + COL_GAP;
        for chunk in group.chunks(2) {
            let left_idx = chunk[0];
            let right_idx = chunk.get(1).copied();

            let row = Rect::from_xywh(left_col_x, ry, col_w, ROW_H);
            let toggle = Rect::from_xywh(
                left_col_x + col_w - 14.0 - TOGGLE_W,
                ry + (ROW_H - TOGGLE_H) * 0.5,
                TOGGLE_W,
                TOGGLE_H,
            );
            rows.push(ModuleRow { catalog_index: left_idx, row, toggle, sliders: Vec::new() });

            if let Some(idx) = right_idx {
                let row = Rect::from_xywh(right_col_x, ry, col_w, ROW_H);
                let toggle = Rect::from_xywh(
                    right_col_x + col_w - 14.0 - TOGGLE_W,
                    ry + (ROW_H - TOGGLE_H) * 0.5,
                    TOGGLE_W,
                    TOGGLE_H,
                );
                rows.push(ModuleRow { catalog_index: idx, row, toggle, sliders: Vec::new() });
            }

            ry += ROW_H + ROW_GAP;
        }
    }

    // Content height that needs to scroll — from body_top down to the last row.
    let content_h = (ry - body_top).max(0.0);
    (panel, sections, rows, content_h)
}

/// Draw the Modules view — a Velvet feature list, one row per EwoClient module:
/// a category dot, the name + description, an on/off toggle, and a slider for
/// any setting the module carries.
fn draw_modules(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let (panel, sections, rows, content_h) = modules_layout(w, h);
    draw_chip(canvas, panel, 16.0);
    let left = panel.left + 24.0;

    // ── Fixed header (eyebrow + counts + title) — drawn over the scrolled
    //    body, so the user always knows what tab they're on. ───────────────
    let eyebrow_font = fonts.jetbrains_mono(11.0);
    let mut eyebrow = Paint::default();
    eyebrow.set_anti_alias(true);
    eyebrow.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        "MODULES",
        (left, panel.top + 36.0),
        &eyebrow_font,
        &eyebrow,
        0.22,
    );

    let on = (0..catalog::REGISTRY.len())
        .filter(|&i| editor.modules.get(i).enabled)
        .count();
    let count_str = format!("{} / {} ON", on, catalog::REGISTRY.len());
    let count_w = measure_tracked_em(&eyebrow_font, &count_str, 0.22);
    let mut count_paint = Paint::default();
    count_paint.set_anti_alias(true);
    count_paint.set_color4f(rgba(MAUVE, 1.0), None);
    draw_tracked_em(
        canvas,
        &count_str,
        (panel.right - 24.0 - count_w, panel.top + 36.0),
        &eyebrow_font,
        &count_paint,
        0.22,
    );

    let title_font = fonts.fraunces_axes(27.0, 36.0, 1.0, 600.0, None);
    let mut title = Paint::default();
    title.set_anti_alias(true);
    title.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str("EwoClient modules", (left, panel.top + 70.0), &title_font, &title);

    // ── Scrollable body — clip to the panel inner rect, translate by the
    //    editor's scroll offset, then render sections + rows in logical (un-
    //    scrolled) coordinates. The press handler offsets clicks by the same
    //    amount so hit-tests against ModuleRow rects line up. ───────────────
    let body_top = panel.top + 24.0 + 76.0;
    let body_clip = Rect::new(panel.left + 4.0, body_top - 8.0, panel.right - 4.0, panel.bottom - 4.0);
    let max_scroll = (content_h - (panel.bottom - body_top - 8.0)).max(0.0);
    let scroll_y = editor.modules_scroll.clamp(0.0, max_scroll);

    canvas.save();
    canvas.clip_rect(body_clip, Some(ClipOp::Intersect), Some(true));
    canvas.translate((0.0, -scroll_y));

    let section_font = fonts.jetbrains_mono(11.0);
    let mut section_paint = Paint::default();
    section_paint.set_anti_alias(true);
    section_paint.set_color4f(rgba(ROSE, 0.85), None);
    for section in &sections {
        draw_tracked_em(
            canvas,
            section.label,
            (left, section.label_y),
            &section_font,
            &section_paint,
            0.22,
        );
    }

    // Row cards — compact, no description: LED + cat eyebrow above name,
    // optional keybind chip + on/off toggle on the right. Wine card fill is
    // opaque enough that in-world widgets behind it (HudEditor mode aside)
    // don't bleed through.
    let cat_font = fonts.jetbrains_mono(9.5);
    let name_font = fonts.fraunces_axes(18.0, 60.0, 0.0, 540.0, Some(36.0));
    let chip_font = fonts.jetbrains_mono(10.0);
    for row in rows.iter() {
        let idx = row.catalog_index;
        let def = &catalog::REGISTRY[idx];
        let st = editor.modules.get(idx);

        // Card chrome — the row IS the card now (2-col layout uses col_w),
        // height covers ROW_H + any setting sliders below it.
        let card = row.row;
        let card_rr = RRect::new_rect_xy(card, 12.0, 12.0);
        let mut card_fill = Paint::default();
        card_fill.set_anti_alias(true);
        card_fill.set_color4f(
            if st.enabled {
                Color4f::new(BERRY.0 as f32 / 255.0, BERRY.1 as f32 / 255.0, BERRY.2 as f32 / 255.0, 0.18)
            } else {
                rgba(WINE, 0.72)
            },
            None,
        );
        canvas.draw_rrect(card_rr, &card_fill);
        let mut card_border = Paint::default();
        card_border.set_anti_alias(true);
        card_border.set_style(PaintStyle::Stroke);
        card_border.set_stroke_width(1.0);
        card_border.set_color4f(
            if st.enabled {
                rgba(ROSE, 0.28)
            } else {
                rgba(PEARL, 0.08)
            },
            None,
        );
        canvas.draw_rrect(card_rr, &card_border);

        // Category LED.
        let led_cx = card.left + 18.0;
        let led_cy = card.top + 30.0;
        let led_color = module_category_color(def.category);
        if st.enabled {
            let mut halo = Paint::default();
            halo.set_anti_alias(true);
            halo.set_color4f(rgba(led_color, 0.55), None);
            halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 5.0, false));
            canvas.draw_circle((led_cx, led_cy), 7.0, &halo);
        }
        let mut led = Paint::default();
        led.set_anti_alias(true);
        led.set_color4f(
            if st.enabled {
                rgba(led_color, 1.0)
            } else {
                Color4f::new(0x3A as f32 / 255.0, 0x1A as f32 / 255.0, 0x23 as f32 / 255.0, 1.0)
            },
            None,
        );
        canvas.draw_circle((led_cx, led_cy), 4.0, &led);

        // Category eyebrow above the name.
        let text_left = card.left + 36.0;
        let mut cat_paint = Paint::default();
        cat_paint.set_anti_alias(true);
        cat_paint.set_color4f(rgba(MAUVE, 0.95), None);
        draw_tracked_em(
            canvas,
            def.category.label(),
            (text_left, card.top + 20.0),
            &cat_font,
            &cat_paint,
            0.24,
        );

        // Name — clipped to leave room for the chip + toggle on the right.
        let right_reserve = 14.0 + row.toggle.width()
            + editor
                .keybind_label_for(def.id)
                .map(|l| {
                    9.0 * 2.0 + measure_tracked_em(&chip_font, &l, 0.18) + 10.0
                })
                .unwrap_or(0.0);
        let name_clip = Rect::new(
            text_left,
            card.top + 24.0,
            card.right - right_reserve - 6.0,
            card.top + 48.0,
        );
        canvas.save();
        canvas.clip_rect(name_clip, Some(ClipOp::Intersect), Some(true));
        let mut name_paint = Paint::default();
        name_paint.set_anti_alias(true);
        name_paint.set_color4f(
            if st.enabled {
                rgba(PEARL, 1.0)
            } else {
                rgba(MAUVE, 0.75)
            },
            None,
        );
        canvas.draw_str(def.name, (text_left, card.top + 44.0), &name_font, &name_paint);
        canvas.restore();

        // Optional keybind chip left of the toggle.
        if let Some(label) = editor.keybind_label_for(def.id) {
            let chip_pad_x = 9.0;
            let chip_h = 22.0;
            let label_w = measure_tracked_em(&chip_font, &label, 0.18);
            let chip_w = chip_pad_x * 2.0 + label_w;
            let chip_x = row.toggle.left - 10.0 - chip_w;
            let chip_y = row.toggle.top + (row.toggle.height() - chip_h) * 0.5;
            let chip_rect = Rect::from_xywh(chip_x, chip_y, chip_w, chip_h);
            let chip_rr = RRect::new_rect_xy(chip_rect, 6.0, 6.0);

            let mut chip_fill = Paint::default();
            chip_fill.set_anti_alias(true);
            chip_fill.set_color4f(rgba(WINE, 0.62), None);
            canvas.draw_rrect(chip_rr, &chip_fill);
            let mut chip_border = Paint::default();
            chip_border.set_anti_alias(true);
            chip_border.set_style(PaintStyle::Stroke);
            chip_border.set_stroke_width(1.0);
            chip_border.set_color4f(rgba(PEARL, 0.10), None);
            canvas.draw_rrect(chip_rr, &chip_border);

            let (_, cm) = chip_font.metrics();
            let cap = if cm.cap_height > 0.0 { cm.cap_height } else { 7.0 };
            let mut chip_text = Paint::default();
            chip_text.set_anti_alias(true);
            chip_text.set_color4f(
                if st.enabled {
                    rgba(CHAMP, 1.0)
                } else {
                    rgba(MAUVE, 1.0)
                },
                None,
            );
            draw_tracked_em(
                canvas,
                &label,
                (chip_x + chip_pad_x, chip_y + (chip_h + cap) * 0.5),
                &chip_font,
                &chip_text,
                0.18,
            );
        }

        // On/off toggle.
        draw_panel_toggle(canvas, row.toggle, st.enabled);

        // Settings affordance — a tiny "···" icon just left of the keybind
        // chip / toggle, only on modules that *have* sliders. Right-click the
        // row to open the popover (or left-click this dot to do the same).
        if !def.settings.is_empty() {
            let dot_cx = row.toggle.left
                - editor
                    .keybind_label_for(def.id)
                    .map(|l| {
                        9.0 * 2.0 + measure_tracked_em(&chip_font, &l, 0.18) + 10.0 + 16.0
                    })
                    .unwrap_or(16.0);
            let dot_cy = row.toggle.top + row.toggle.height() * 0.5;
            let mut dp = Paint::default();
            dp.set_anti_alias(true);
            dp.set_color4f(rgba(MAUVE, 0.85), None);
            for n in -1..=1 {
                canvas.draw_circle((dot_cx, dot_cy + n as f32 * 5.0), 1.4, &dp);
            }
        }
    }

    canvas.restore();

    // Scrollbar — a thin pearl track + rose knob on the right edge of the
    // panel, only drawn when content overflows.
    if max_scroll > 0.0 {
        let track_x = panel.right - 6.0;
        let track_top = body_top + 2.0;
        let track_bot = panel.bottom - 6.0;
        let track_h = (track_bot - track_top).max(20.0);
        let mut tp = Paint::default();
        tp.set_anti_alias(true);
        tp.set_color4f(rgba(PEARL, 0.06), None);
        canvas.draw_rrect(
            RRect::new_rect_xy(
                Rect::from_xywh(track_x, track_top, 2.0, track_h),
                1.0,
                1.0,
            ),
            &tp,
        );

        // Knob — proportional to the visible / content ratio.
        let visible = panel.bottom - body_top - 8.0;
        let knob_h = (track_h * (visible / content_h)).max(24.0);
        let knob_t = (scroll_y / max_scroll) * (track_h - knob_h);
        let mut kp = Paint::default();
        kp.set_anti_alias(true);
        kp.set_color4f(rgba(ROSE, 0.55), None);
        canvas.draw_rrect(
            RRect::new_rect_xy(
                Rect::from_xywh(track_x - 1.0, track_top + knob_t, 4.0, knob_h),
                2.0,
                2.0,
            ),
            &kp,
        );
    }
}

/// Draw one module-setting slider — a thin Velvet track with a pearl knob and
/// the current value. `enabled` dims it when the parent module is off.
fn draw_module_slider(
    canvas: &Canvas,
    area: Rect,
    setting: &catalog::ModuleSetting,
    value: f32,
    enabled: bool,
    fonts: &FontStore,
) {
    let alpha = if enabled { 1.0 } else { 0.45 };
    let cy = area.top + area.height() * 0.5;
    let value_w = 54.0;
    let track_left = area.left + 14.0;
    let track_right = area.right - value_w;
    let track_h = 4.0;

    // Track.
    let track = Rect::from_xywh(track_left, cy - track_h * 0.5, track_right - track_left, track_h);
    let mut tp = Paint::default();
    tp.set_anti_alias(true);
    tp.set_color4f(rgba(WINE, 0.85 * alpha), None);
    canvas.draw_rrect(RRect::new_rect_xy(track, track_h * 0.5, track_h * 0.5), &tp);

    let span = (setting.max - setting.min).max(0.001);
    let frac = ((value - setting.min) / span).clamp(0.0, 1.0);
    let knob_x = track_left + frac * (track_right - track_left);

    // Fill up to the knob — rose→lavender.
    if knob_x > track_left + 1.0 {
        let fill = Rect::from_xywh(track_left, cy - track_h * 0.5, knob_x - track_left, track_h);
        let mut fp = Paint::default();
        fp.set_anti_alias(true);
        if let Some(shader) = gradient_shader::linear(
            (Point::new(track_left, cy), Point::new(track_right, cy)),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[rgba(ROSE, alpha), rgba(LAV, alpha)],
                None,
            ),
            None,
            TileMode::Clamp,
            None,
            None,
        ) {
            fp.set_shader(shader);
        }
        canvas.draw_rrect(RRect::new_rect_xy(fill, track_h * 0.5, track_h * 0.5), &fp);
    }

    // Knob.
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(PEARL, alpha), None);
    canvas.draw_circle((knob_x, cy), 6.0, &knob);

    // Value, right-aligned in the reserved strip.
    let val_font = fonts.jetbrains_mono(13.0);
    let val_str = format!("{}", value.round() as i32);
    let mut vp = Paint::default();
    vp.set_anti_alias(true);
    vp.set_color4f(rgba(PEARL, alpha), None);
    let (_, m) = val_font.metrics();
    let cap = if m.cap_height > 0.0 { m.cap_height } else { 9.0 };
    canvas.draw_str(&val_str, (track_right + 16.0, cy + cap * 0.5), &val_font, &vp);
}

/// Hit-rects for the module settings popover — the card itself, the close
/// button at top-right, and one rect per slider track. Right-click on a
/// MODULES row with sliders opens this popover; geometry must match
/// [`draw_module_popover`] one-to-one for hit-tests to line up.
struct ModulePopoverLayout {
    card: Rect,
    close: Rect,
    sliders: Vec<Rect>,
}

fn module_popover_layout(w: f32, h: f32, idx: usize) -> ModulePopoverLayout {
    const CARD_W: f32 = 520.0;
    const PAD: f32 = 24.0;
    const HEADER_H: f32 = 92.0; // eyebrow + name + (single-line description)
    const SLIDER_ROW_H: f32 = 60.0; // label + track per setting
    const FOOTER_H: f32 = 24.0; // bottom padding

    let setting_count = catalog::REGISTRY
        .get(idx)
        .map(|m| m.settings.len())
        .unwrap_or(0)
        .max(1);
    let card_h = PAD + HEADER_H + SLIDER_ROW_H * setting_count as f32 + FOOTER_H + PAD;
    let card_x = (w - CARD_W) * 0.5;
    let card_y = (h - card_h) * 0.5;
    let card = Rect::from_xywh(card_x, card_y, CARD_W, card_h);

    // Close button — a 28×28 hit-area in the top-right corner.
    let close = Rect::from_xywh(card.right - PAD - 22.0, card.top + PAD - 4.0, 28.0, 28.0);

    // Per-setting slider track rects. Each row reserves SLIDER_ROW_H of
    // vertical space; the track itself sits in the lower half so the label
    // can sit above it.
    let sliders: Vec<Rect> = (0..catalog::REGISTRY[idx].settings.len())
        .map(|s| {
            let row_top = card.top + PAD + HEADER_H + s as f32 * SLIDER_ROW_H;
            Rect::from_xywh(
                card.left + PAD,
                row_top + 28.0,
                CARD_W - PAD * 2.0,
                SLIDER_ROW_H - 28.0,
            )
        })
        .collect();

    ModulePopoverLayout { card, close, sliders }
}

/// Draw the per-module settings popover — a centred glass card with one row
/// per slider. Backdrop dims the rest of the screen so the popover reads as
/// a focused modal.
fn draw_module_popover(
    canvas: &Canvas,
    editor: &Editor,
    idx: usize,
    fonts: &FontStore,
    w: f32,
    h: f32,
) {
    let Some(def) = catalog::REGISTRY.get(idx) else {
        return;
    };
    let st = editor.modules.get(idx);
    let layout = module_popover_layout(w, h, idx);

    // ── Dim backdrop — black 0.45 over everything ─────────────────────────
    let mut dim = Paint::default();
    dim.set_anti_alias(true);
    dim.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.45), None);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, h), &dim);

    // ── Card chrome — drop shadow + wine fill + rose-tinted border ────────
    let card_rr = RRect::new_rect_xy(layout.card, 16.0, 16.0);
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.6), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 18.0, false));
    let shadow_rrect = RRect::new_rect_xy(
        layout.card.with_offset((0.0, 10.0)),
        16.0,
        16.0,
    );
    canvas.draw_rrect(shadow_rrect, &shadow);

    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.92), None);
    canvas.draw_rrect(card_rr, &fill);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.28), None);
    canvas.draw_rrect(card_rr, &border);

    // Inset top-edge highlight.
    canvas.save();
    let top_clip = Rect::from_xywh(layout.card.left, layout.card.top, layout.card.width(), 2.0);
    canvas.clip_rect(top_clip, Some(ClipOp::Intersect), Some(true));
    let inset = Rect::new(
        layout.card.left + 0.5,
        layout.card.top + 0.5,
        layout.card.right - 0.5,
        layout.card.bottom - 0.5,
    );
    let mut top = Paint::default();
    top.set_anti_alias(true);
    top.set_style(PaintStyle::Stroke);
    top.set_stroke_width(1.0);
    top.set_color4f(rgba(PEARL, 0.10), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset, 15.5, 15.5), &top);
    canvas.restore();

    let pad = 24.0;
    let left = layout.card.left + pad;

    // ── Header — category eyebrow + module name + description ─────────────
    let cat_font = fonts.jetbrains_mono(10.0);
    let mut cat_paint = Paint::default();
    cat_paint.set_anti_alias(true);
    cat_paint.set_color4f(rgba(ROSE, 0.9), None);
    draw_tracked_em(
        canvas,
        def.category.label(),
        (left, layout.card.top + pad + 14.0),
        &cat_font,
        &cat_paint,
        0.24,
    );

    let name_font = fonts.fraunces_axes(24.0, 100.0, 1.0, 540.0, Some(36.0));
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str(def.name, (left, layout.card.top + pad + 44.0), &name_font, &name_paint);

    let desc_font = fonts.newsreader(13.0);
    let mut desc_paint = Paint::default();
    desc_paint.set_anti_alias(true);
    desc_paint.set_color4f(rgba(MAUVE, 1.0), None);
    canvas.save();
    let desc_clip = Rect::new(
        left,
        layout.card.top + pad + 56.0,
        layout.card.right - pad,
        layout.card.top + pad + 80.0,
    );
    canvas.clip_rect(desc_clip, Some(ClipOp::Intersect), Some(true));
    canvas.draw_str(
        def.description,
        (left, layout.card.top + pad + 74.0),
        &desc_font,
        &desc_paint,
    );
    canvas.restore();

    // ── Close button (top-right) ──────────────────────────────────────────
    let cx = layout.close.left + layout.close.width() * 0.5;
    let cy = layout.close.top + layout.close.height() * 0.5;
    let mut close_paint = Paint::default();
    close_paint.set_anti_alias(true);
    close_paint.set_style(PaintStyle::Stroke);
    close_paint.set_stroke_width(1.5);
    close_paint.set_color4f(rgba(MAUVE, 1.0), None);
    canvas.draw_line((cx - 6.0, cy - 6.0), (cx + 6.0, cy + 6.0), &close_paint);
    canvas.draw_line((cx + 6.0, cy - 6.0), (cx - 6.0, cy + 6.0), &close_paint);

    // ── One row per setting — label above the track, value to the right ───
    let label_font = fonts.newsreader(14.0);
    let val_font = fonts.jetbrains_mono(12.0);
    for (slot, setting) in def.settings.iter().enumerate() {
        let track = layout.sliders[slot];

        // Label (left).
        let mut lp = Paint::default();
        lp.set_anti_alias(true);
        lp.set_color4f(rgba(PEARL, 1.0), None);
        canvas.draw_str(setting.label, (left, track.top - 8.0), &label_font, &lp);

        // Value (right) — formatted with step-aware precision.
        let value = st.settings[slot];
        let val_str = if setting.step >= 1.0 {
            format!("{}", value.round() as i32)
        } else if setting.step >= 0.1 {
            format!("{:.1}", value)
        } else {
            format!("{:.2}", value)
        };
        let mut vp = Paint::default();
        vp.set_anti_alias(true);
        vp.set_color4f(rgba(CHAMP, 1.0), None);
        let (vw, _) = val_font.measure_str(&val_str, Some(&vp));
        canvas.draw_str(
            &val_str,
            (layout.card.right - pad - vw, track.top - 8.0),
            &val_font,
            &vp,
        );

        // The slider track itself reuses the existing `draw_module_slider`,
        // which lays out a thin pill + pearl knob.
        draw_module_slider(canvas, track, setting, value, true, fonts);
    }
}

// ────────────────────────────────────────────────────────────────────────
// CROSSHAIR editor — vanilla-replace toggle, true-to-life preview row,
// shape sliders, HSV colour picker (SV square + Hue strip + Alpha strip +
// hex readout). Layout is computed once per frame from `crosshair_layout`
// so render + hit-test share rects.
// ────────────────────────────────────────────────────────────────────────

/// Hit-test rectangles for the CROSSHAIR editor — both `draw_crosshair_view`
/// and the press / drag handlers consume this.
struct CrosshairLayout {
    /// Outer card (drawn with the glass-panel chrome).
    card: Rect,
    /// The three small 1:1 preview panes (idle / target / reach), centred
    /// in a single horizontal strip below the title.
    preview_idle: Rect,
    preview_target: Rect,
    preview_reach: Rect,
    /// Big enable toggle below the preview strip.
    enable_toggle: Rect,
    /// Three small boolean toggles in a row below the enable pill.
    bool_dot: Rect,
    bool_outline: Rect,
    bool_aa: Rect,
    /// Shape sliders, top-to-bottom on the LEFT column.
    arm_length: Rect,
    arm_gap: Rect,
    arm_thickness: Rect,
    dot_size: Rect,
    outline_thickness: Rect,
    reach_distance: Rect,
    /// Four colour-state swatches above the picker on the RIGHT column.
    swatch_idle: Rect,
    swatch_target: Rect,
    swatch_reach: Rect,
    swatch_outline: Rect,
    /// HSV picker controls.
    sv_square: Rect,
    hue_strip: Rect,
    alpha_strip: Rect,
    /// Hex readout below the picker (read-only — for now).
    hex_display: Rect,
}

fn crosshair_layout(w: f32, h: f32) -> CrosshairLayout {
    let card_w = (w - 240.0).min(1500.0).max(900.0);
    let card_h = (h - 140.0).min(880.0).max(620.0);
    let card_x = (w - card_w) * 0.5;
    let card_y = 100.0;
    let card = Rect::from_xywh(card_x, card_y, card_w, card_h);

    let body_top = card.top + 74.0; // below title + subtitle
    let body_left = card.left + 36.0;
    let body_right = card.right - 36.0;
    let body_bottom = card.bottom - 28.0;
    let body_w = body_right - body_left;

    let mut y = body_top;

    // ── Preview row — 3 panes side-by-side, centred at 1:1 scale. ─────
    let preview_pane_w = 220.0;
    let preview_pane_h = 110.0;
    let preview_gap = 14.0;
    let preview_row_w = 3.0 * preview_pane_w + 2.0 * preview_gap;
    let preview_left = body_left + (body_w - preview_row_w) * 0.5;
    let preview_idle = Rect::from_xywh(preview_left, y, preview_pane_w, preview_pane_h);
    let preview_target = Rect::from_xywh(
        preview_left + preview_pane_w + preview_gap,
        y, preview_pane_w, preview_pane_h,
    );
    let preview_reach = Rect::from_xywh(
        preview_left + 2.0 * (preview_pane_w + preview_gap),
        y, preview_pane_w, preview_pane_h,
    );
    // The labels sit *under* each pane; budget +16 for them.
    y += preview_pane_h + 22.0;

    // ── Enable pill, full body width. ─────────────────────────────────
    let enable_toggle = Rect::from_xywh(body_left, y, body_w, 42.0);
    y = enable_toggle.bottom + 18.0;

    // ── Bottom: two columns. Left = shape; right = colour. ───────────
    let col_gap = 40.0;
    let col_w = (body_w - col_gap) * 0.5;
    let col_left = body_left;
    let col_right_start = body_left + col_w + col_gap;
    let col_right_end = body_right;

    // ── LEFT column: shape ────────────────────────────────────────────
    let row_h = 28.0;
    let slider_h = 22.0;
    let slider_gap = 16.0;
    let third_w = (col_w - 16.0) / 3.0;
    let mut ly = y;
    let bool_dot = Rect::new(col_left, ly, col_left + third_w, ly + row_h);
    let bool_outline = Rect::new(col_left + third_w + 8.0, ly, col_left + 2.0 * third_w + 8.0, ly + row_h);
    let bool_aa = Rect::new(col_left + 2.0 * third_w + 16.0, ly, col_left + col_w, ly + row_h);
    ly = bool_dot.bottom + slider_gap + 4.0;

    let mk_slider_left = |ly: f32| Rect::new(col_left, ly + 18.0, col_left + col_w, ly + 18.0 + slider_h);
    let arm_length = mk_slider_left(ly);
    ly = arm_length.bottom + slider_gap;
    let arm_gap_r = mk_slider_left(ly);
    ly = arm_gap_r.bottom + slider_gap;
    let arm_thickness = mk_slider_left(ly);
    ly = arm_thickness.bottom + slider_gap;
    let dot_size = mk_slider_left(ly);
    ly = dot_size.bottom + slider_gap;
    let outline_thickness = mk_slider_left(ly);
    ly = outline_thickness.bottom + slider_gap;
    let reach_distance = mk_slider_left(ly);
    let _ = ly;

    // ── RIGHT column: colour ──────────────────────────────────────────
    let mut ry = y;
    // Four swatches in a row.
    let sw_h = 32.0;
    let sw_w = (col_right_end - col_right_start - 24.0) / 4.0;
    let swatch_idle = Rect::from_xywh(col_right_start, ry, sw_w, sw_h);
    let swatch_target = Rect::from_xywh(col_right_start + sw_w + 8.0, ry, sw_w, sw_h);
    let swatch_reach = Rect::from_xywh(col_right_start + 2.0 * (sw_w + 8.0), ry, sw_w, sw_h);
    let swatch_outline = Rect::from_xywh(col_right_start + 3.0 * (sw_w + 8.0), ry, sw_w, sw_h);
    ry = swatch_idle.bottom + 18.0;

    // HSV picker block — SV square + vertical Hue + vertical Alpha + hex.
    // Total height capped to what's left so it stays inside the body.
    let picker_max_h = (body_bottom - ry - 36.0).max(180.0); // 36 reserved for hex line
    let picker_h = picker_max_h.min(240.0);
    let strip_w = 22.0;
    let strip_gap = 12.0;
    // SV square is square — width == height.
    let sv_side = picker_h;
    let sv_square = Rect::from_xywh(col_right_start, ry, sv_side, sv_side);
    let hue_strip = Rect::from_xywh(sv_square.right + strip_gap, ry, strip_w, sv_side);
    let alpha_strip = Rect::from_xywh(hue_strip.right + strip_gap, ry, strip_w, sv_side);
    ry = sv_square.bottom + 14.0;

    // Hex line — sits centred under the picker, full column width.
    let hex_display = Rect::from_xywh(col_right_start, ry, col_right_end - col_right_start, 22.0);

    CrosshairLayout {
        card,
        preview_idle,
        preview_target,
        preview_reach,
        enable_toggle,
        bool_dot,
        bool_outline,
        bool_aa,
        arm_length,
        arm_gap: arm_gap_r,
        arm_thickness,
        dot_size,
        outline_thickness,
        reach_distance,
        swatch_idle,
        swatch_target,
        swatch_reach,
        swatch_outline,
        sv_square,
        hue_strip,
        alpha_strip,
        hex_display,
    }
}

/// Inclusive numeric range for a 1D shape-slider — the cursor's x position
/// in the track maps linearly across this span. (HSV picker controls don't
/// share this — they have their own 2D / 1D handling in `drag_crosshair_slider`.)
fn crosshair_slider_range(slot: CrosshairSlider) -> (f32, f32) {
    use CrosshairSlider::*;
    match slot {
        ArmLength => (0.0, 16.0),
        ArmGap => (0.0, 10.0),
        ArmThickness => (1.0, 6.0),
        DotSize => (1.0, 6.0),
        OutlineThickness => (0.0, 2.0),
        ReachDistance => (1.5, 5.0),
        SvSquare | HueStrip | AlphaStrip => (0.0, 1.0), // unused — handled directly
    }
}

fn set_shape_slider(cfg: &mut crate::crosshair::CrosshairConfig, slot: CrosshairSlider, value: f32) {
    use CrosshairSlider::*;
    match slot {
        ArmLength => cfg.arm_length = value,
        ArmGap => cfg.arm_gap = value,
        ArmThickness => cfg.arm_thickness = value,
        DotSize => cfg.dot_size = value,
        OutlineThickness => cfg.outline_thickness = value,
        ReachDistance => cfg.reach_distance = value,
        // Picker controls don't go through this path.
        SvSquare | HueStrip | AlphaStrip => {}
    }
}

// ── HSV ↔ RGB ───────────────────────────────────────────────────────────

/// Hue (0..360), Saturation (0..1), Value (0..1) → 8-bit RGB.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime.rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// 8-bit RGB → (H, S, V). Returns H=0 for grey colours (chroma 0 — H is
/// ambiguous there).
fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if (max - g).abs() < 1e-6 {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    (h, s, v)
}

/// Pull the selected swatch's RGBA into the picker's HSV state. Called
/// when the user clicks a swatch — gives the picker its starting point so
/// the SV square / hue strip / alpha strip are positioned at the slot's
/// current colour.
fn sync_picker_from_slot(ui: &mut CrosshairUi, cfg: &crate::crosshair::CrosshairConfig) {
    let rgba = color_for_slot(cfg, ui.selected_color);
    // Special-case greys (delta=0): keep the user's hue so dragging the
    // value down to black and back up to grey doesn't reset their hue.
    let (h, s, v) = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
    if s > 1e-3 {
        ui.picker_h = h;
    }
    ui.picker_s = s;
    ui.picker_v = v;
    ui.picker_a = rgba[3] as f32 / 255.0;
}

/// Bake the picker's HSV state back into the selected swatch's RGBA. Run
/// after every drag step so the in-world crosshair updates live.
fn write_picker_to_slot(ui: &CrosshairUi, cfg: &mut crate::crosshair::CrosshairConfig) {
    let rgb = hsv_to_rgb(ui.picker_h, ui.picker_s, ui.picker_v);
    let slot = color_for_slot_mut(cfg, ui.selected_color);
    slot[0] = rgb[0];
    slot[1] = rgb[1];
    slot[2] = rgb[2];
    slot[3] = (ui.picker_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

fn color_for_slot(cfg: &crate::crosshair::CrosshairConfig, slot: CrosshairColorSlot) -> [u8; 4] {
    match slot {
        CrosshairColorSlot::Idle => cfg.color_idle,
        CrosshairColorSlot::Target => cfg.color_target,
        CrosshairColorSlot::Reach => cfg.color_reach,
        CrosshairColorSlot::Outline => cfg.outline_color,
    }
}

fn color_for_slot_mut(cfg: &mut crate::crosshair::CrosshairConfig, slot: CrosshairColorSlot) -> &mut [u8; 4] {
    match slot {
        CrosshairColorSlot::Idle => &mut cfg.color_idle,
        CrosshairColorSlot::Target => &mut cfg.color_target,
        CrosshairColorSlot::Reach => &mut cfg.color_reach,
        CrosshairColorSlot::Outline => &mut cfg.outline_color,
    }
}

impl Editor {
    /// Press inside the CROSSHAIR view: flip a toggle, select a colour
    /// slot, or start a slider / picker drag. Toggle flips persist
    /// immediately; slider + picker drags persist on release.
    fn crosshair_press(&mut self, x: f32, y: f32) {
        let layout = crosshair_layout(self.window.0, self.window.1);

        // Big "Enable Custom Crosshair" pill.
        if point_in(layout.enable_toggle, x, y) {
            self.crosshair.enabled = !self.crosshair.enabled;
            crate::crosshair::save(&self.crosshair);
            return;
        }
        if point_in(layout.bool_dot, x, y) {
            self.crosshair.dot_enabled = !self.crosshair.dot_enabled;
            crate::crosshair::save(&self.crosshair);
            return;
        }
        if point_in(layout.bool_outline, x, y) {
            self.crosshair.outline_enabled = !self.crosshair.outline_enabled;
            crate::crosshair::save(&self.crosshair);
            return;
        }
        if point_in(layout.bool_aa, x, y) {
            self.crosshair.anti_alias = !self.crosshair.anti_alias;
            crate::crosshair::save(&self.crosshair);
            return;
        }

        // Colour swatches — selecting one re-syncs the HSV picker to it.
        for (rect, slot) in [
            (layout.swatch_idle, CrosshairColorSlot::Idle),
            (layout.swatch_target, CrosshairColorSlot::Target),
            (layout.swatch_reach, CrosshairColorSlot::Reach),
            (layout.swatch_outline, CrosshairColorSlot::Outline),
        ] {
            if point_in(rect, x, y) {
                self.crosshair_ui.selected_color = slot;
                sync_picker_from_slot(&mut self.crosshair_ui, &self.crosshair);
                return;
            }
        }

        // Shape sliders — start a 1D drag.
        for (rect, slot) in [
            (layout.arm_length, CrosshairSlider::ArmLength),
            (layout.arm_gap, CrosshairSlider::ArmGap),
            (layout.arm_thickness, CrosshairSlider::ArmThickness),
            (layout.dot_size, CrosshairSlider::DotSize),
            (layout.outline_thickness, CrosshairSlider::OutlineThickness),
            (layout.reach_distance, CrosshairSlider::ReachDistance),
        ] {
            if point_in(rect, x, y) {
                self.crosshair_ui.drag = Some(slot);
                self.drag_crosshair_slider(slot, x, y);
                return;
            }
        }

        // HSV picker — SV square (2D), Hue strip (1D), Alpha strip (1D).
        for (rect, slot) in [
            (layout.sv_square, CrosshairSlider::SvSquare),
            (layout.hue_strip, CrosshairSlider::HueStrip),
            (layout.alpha_strip, CrosshairSlider::AlphaStrip),
        ] {
            if point_in(rect, x, y) {
                self.crosshair_ui.drag = Some(slot);
                self.drag_crosshair_slider(slot, x, y);
                return;
            }
        }
    }

    /// Update the value bound to whatever control is currently dragged.
    /// `x` + `y` come straight from the cursor; the 2D SV square uses
    /// both, the 1D strips/sliders use one each.
    fn drag_crosshair_slider(&mut self, slot: CrosshairSlider, x: f32, y: f32) {
        let layout = crosshair_layout(self.window.0, self.window.1);
        match slot {
            // 1D shape sliders.
            CrosshairSlider::ArmLength
            | CrosshairSlider::ArmGap
            | CrosshairSlider::ArmThickness
            | CrosshairSlider::DotSize
            | CrosshairSlider::OutlineThickness
            | CrosshairSlider::ReachDistance => {
                let track = match slot {
                    CrosshairSlider::ArmLength => layout.arm_length,
                    CrosshairSlider::ArmGap => layout.arm_gap,
                    CrosshairSlider::ArmThickness => layout.arm_thickness,
                    CrosshairSlider::DotSize => layout.dot_size,
                    CrosshairSlider::OutlineThickness => layout.outline_thickness,
                    CrosshairSlider::ReachDistance => layout.reach_distance,
                    _ => unreachable!(),
                };
                let inner_left = track.left + 10.0;
                let inner_right = track.right - 10.0;
                let span = (inner_right - inner_left).max(1.0);
                let frac = ((x - inner_left) / span).clamp(0.0, 1.0);
                let (min, max) = crosshair_slider_range(slot);
                let value = min + frac * (max - min);
                set_shape_slider(&mut self.crosshair, slot, value);
            }
            CrosshairSlider::SvSquare => {
                let sq = layout.sv_square;
                let fx = ((x - sq.left) / sq.width().max(1.0)).clamp(0.0, 1.0);
                let fy = ((y - sq.top) / sq.height().max(1.0)).clamp(0.0, 1.0);
                // x = saturation, y = (1 - value) so the bright corner sits
                // at the top-left of the square (canonical SV picker layout).
                self.crosshair_ui.picker_s = fx;
                self.crosshair_ui.picker_v = 1.0 - fy;
                write_picker_to_slot(&self.crosshair_ui, &mut self.crosshair);
            }
            CrosshairSlider::HueStrip => {
                let strip = layout.hue_strip;
                let fy = ((y - strip.top) / strip.height().max(1.0)).clamp(0.0, 1.0);
                self.crosshair_ui.picker_h = fy * 360.0;
                write_picker_to_slot(&self.crosshair_ui, &mut self.crosshair);
            }
            CrosshairSlider::AlphaStrip => {
                let strip = layout.alpha_strip;
                let fy = ((y - strip.top) / strip.height().max(1.0)).clamp(0.0, 1.0);
                // Top = opaque, bottom = transparent — matches Photoshop /
                // most colour pickers.
                self.crosshair_ui.picker_a = 1.0 - fy;
                write_picker_to_slot(&self.crosshair_ui, &mut self.crosshair);
            }
        }
    }
}

fn draw_crosshair_view(canvas: &Canvas, editor: &Editor, fonts: &FontStore, w: f32, h: f32) {
    let layout = crosshair_layout(w, h);

    // ── Outer card chrome ───────────────────────────────────────────────
    draw_iw_shell(canvas, layout.card, 18.0);

    // Title row.
    let head_font = fonts.fraunces_axes(30.0, 100.0, 1.0, 500.0, Some(40.0));
    let mut head_paint = Paint::default();
    head_paint.set_anti_alias(true);
    head_paint.set_color4f(rgba(PEARL, 0.98), None);
    canvas.draw_str(
        "Crosshair",
        (layout.card.left + 36.0, layout.card.top + 44.0),
        &head_font,
        &head_paint,
    );

    let sub_font = fonts.fraunces_axes(15.0, 30.0, 0.0, 380.0, Some(20.0));
    let mut sub_paint = Paint::default();
    sub_paint.set_anti_alias(true);
    sub_paint.set_color4f(rgba(MAUVE, 0.85), None);
    let (hw, _) = head_font.measure_str("Crosshair", Some(&head_paint));
    canvas.draw_str(
        "always rendered, colour adapts to reach",
        (layout.card.left + 36.0 + hw + 14.0, layout.card.top + 42.0),
        &sub_font,
        &sub_paint,
    );

    // Right-side hairline pip with the selected profile name, mirroring
    // the other dashboard tabs.
    let pip_font = fonts.jetbrains_mono(11.0);
    let mut pip_paint = Paint::default();
    pip_paint.set_anti_alias(true);
    pip_paint.set_color4f(rgba(MAUVE, 0.55), None);
    let pip = format!("PROFILE  ·  {}", editor.active_profile.to_uppercase());
    let pw = measure_tracked_em(&pip_font, &pip, 0.18);
    draw_tracked_em(
        canvas,
        &pip,
        (layout.card.right - 36.0 - pw, layout.card.top + 42.0),
        &pip_font,
        &pip_paint,
        0.18,
    );

    // ── Preview row ─────────────────────────────────────────────────────
    draw_crosshair_preview(canvas, &layout, &editor.crosshair, fonts);

    // ── Enable toggle (full-width pill) ─────────────────────────────────
    draw_crosshair_enable_pill(canvas, layout.enable_toggle, editor.crosshair.enabled, fonts);

    // ── Section labels (left = SHAPE, right = COLOUR) ──────────────────
    let section_font = fonts.jetbrains_mono(10.0);
    let mut section_paint = Paint::default();
    section_paint.set_anti_alias(true);
    section_paint.set_color4f(rgba(MAUVE, 0.6), None);
    draw_tracked_em(
        canvas, "SHAPE",
        (layout.bool_dot.left, layout.bool_dot.top - 10.0),
        &section_font, &section_paint, 0.22,
    );
    draw_tracked_em(
        canvas, "COLOUR",
        (layout.swatch_idle.left, layout.swatch_idle.top - 10.0),
        &section_font, &section_paint, 0.22,
    );

    // ── Three boolean toggles (LEFT col) ────────────────────────────────
    draw_crosshair_bool(canvas, layout.bool_dot, "Center dot", editor.crosshair.dot_enabled, fonts);
    draw_crosshair_bool(canvas, layout.bool_outline, "Outline", editor.crosshair.outline_enabled, fonts);
    draw_crosshair_bool(canvas, layout.bool_aa, "Anti-alias", editor.crosshair.anti_alias, fonts);

    // ── Shape sliders (LEFT col) ────────────────────────────────────────
    draw_crosshair_slider(
        canvas, layout.arm_length, "Arm length", editor.crosshair.arm_length,
        crosshair_slider_range(CrosshairSlider::ArmLength), "px", 0, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.arm_gap, "Center gap", editor.crosshair.arm_gap,
        crosshair_slider_range(CrosshairSlider::ArmGap), "px", 1, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.arm_thickness, "Arm thickness", editor.crosshair.arm_thickness,
        crosshair_slider_range(CrosshairSlider::ArmThickness), "px", 1, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.dot_size, "Center dot size", editor.crosshair.dot_size,
        crosshair_slider_range(CrosshairSlider::DotSize), "px", 1, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.outline_thickness, "Outline thickness", editor.crosshair.outline_thickness,
        crosshair_slider_range(CrosshairSlider::OutlineThickness), "px", 1, fonts,
    );
    draw_crosshair_slider(
        canvas, layout.reach_distance, "Reach distance", editor.crosshair.reach_distance,
        crosshair_slider_range(CrosshairSlider::ReachDistance), "blocks", 1, fonts,
    );

    // ── Colour swatches (RIGHT col) ─────────────────────────────────────
    draw_crosshair_swatch(canvas, layout.swatch_idle, "Idle", editor.crosshair.color_idle, editor.crosshair_ui.selected_color == CrosshairColorSlot::Idle, fonts);
    draw_crosshair_swatch(canvas, layout.swatch_target, "Target", editor.crosshair.color_target, editor.crosshair_ui.selected_color == CrosshairColorSlot::Target, fonts);
    draw_crosshair_swatch(canvas, layout.swatch_reach, "In reach", editor.crosshair.color_reach, editor.crosshair_ui.selected_color == CrosshairColorSlot::Reach, fonts);
    draw_crosshair_swatch(canvas, layout.swatch_outline, "Outline", editor.crosshair.outline_color, editor.crosshair_ui.selected_color == CrosshairColorSlot::Outline, fonts);

    // ── HSV picker — SV square + Hue strip + Alpha strip ───────────────
    let ui = &editor.crosshair_ui;
    let current_rgba = color_for_slot(&editor.crosshair, ui.selected_color);
    let pure_hue_rgb = hsv_to_rgb(ui.picker_h, 1.0, 1.0);
    draw_sv_square(canvas, layout.sv_square, pure_hue_rgb, ui.picker_s, ui.picker_v);
    draw_hue_strip(canvas, layout.hue_strip, ui.picker_h);
    draw_alpha_strip(canvas, layout.alpha_strip, [current_rgba[0], current_rgba[1], current_rgba[2]], ui.picker_a);

    // ── Hex readout below the picker ────────────────────────────────────
    let hex_font = fonts.jetbrains_mono(13.0);
    let mut hex_paint = Paint::default();
    hex_paint.set_anti_alias(true);
    hex_paint.set_color4f(rgba(PEARL, 0.9), None);
    let hex_str = format!(
        "#{:02X}{:02X}{:02X}  ·  α {:>3}",
        current_rgba[0], current_rgba[1], current_rgba[2], current_rgba[3]
    );
    let (hw2, _) = hex_font.measure_str(&hex_str, Some(&hex_paint));
    canvas.draw_str(
        &hex_str,
        (
            layout.hex_display.left + (layout.hex_display.width() - hw2) * 0.5,
            layout.hex_display.top + 16.0,
        ),
        &hex_font,
        &hex_paint,
    );
}

/// Three side-by-side preview panes (Idle / Target / In-Reach), each
/// showing the crosshair at **true 1:1 scale** over the actual live
/// game framebuffer. The composite step in `lib.rs` cuts these rects
/// out of the overlay frost (`Editor::live_game_cutouts`) and re-blits
/// the un-frosted snapshot underneath, so the pane interior is the
/// user's real game — we just clear it here in the Skia overlay surface
/// and paint the crosshair on top.
fn draw_crosshair_preview(
    canvas: &Canvas,
    layout: &CrosshairLayout,
    cfg: &crate::crosshair::CrosshairConfig,
    fonts: &FontStore,
) {
    use skia_safe::{BlendMode, RRect};

    let panes = [
        (layout.preview_idle, crate::crosshair::CrosshairState::Idle, "IDLE"),
        (layout.preview_target, crate::crosshair::CrosshairState::Target, "TARGET"),
        (layout.preview_reach, crate::crosshair::CrosshairState::Reach, "IN REACH"),
    ];

    let label_font = fonts.jetbrains_mono(10.0);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(MAUVE, 0.75), None);

    for (pane, state, label) in panes {
        let rrect = RRect::new_rect_xy(pane, 10.0, 10.0);
        let saved = canvas.save();
        canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));

        // The Skia offscreen surface was tinted black-22% earlier in
        // `hud::draw` (the overlay "focus mode" dim). That dim covers
        // the whole screen including these panes — which would dull
        // the live game we're trying to show. Punch it back out with
        // BlendMode::Src + fully-transparent black so the pane interior
        // becomes alpha=0 again.
        let mut clear = Paint::default();
        clear.set_blend_mode(BlendMode::Src);
        clear.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.0), None);
        canvas.draw_rect(pane, &clear);

        // Crosshair at exact pane centre, 1:1 scale — *true to life*.
        // Drawn into the Skia overlay surface; once composited, it
        // sits over the live game pixels the composite step blitted
        // into this same rect.
        let cx = (pane.left + pane.right) * 0.5;
        let cy = (pane.top + pane.bottom) * 0.5;
        crate::crosshair::draw(canvas, cx.round(), cy.round(), cfg, state);

        canvas.restore_to_count(saved);

        // Rim — 1px pearl-tinted border, sits on top of everything.
        let mut rim = Paint::default();
        rim.set_anti_alias(true);
        rim.set_style(PaintStyle::Stroke);
        rim.set_stroke_width(1.0);
        rim.set_color4f(rgba(MAUVE, 0.45), None);
        canvas.draw_rrect(rrect, &rim);

        // Label centred under the pane.
        let lw = measure_tracked_em(&label_font, label, 0.22);
        draw_tracked_em(
            canvas, label,
            (pane.left + (pane.width() - lw) * 0.5, pane.bottom + 14.0),
            &label_font, &label_paint, 0.22,
        );
    }
}

/// Big "Enable Custom Crosshair" pill. On = rose fill + pearl label,
/// off = dim track + mauve label.
fn draw_crosshair_enable_pill(canvas: &Canvas, rect: Rect, on: bool, fonts: &FontStore) {
    let rrect = skia_safe::RRect::new_rect_xy(rect, rect.height() * 0.5, rect.height() * 0.5);
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(if on { rgba(ROSE, 0.32) } else { rgba(MAUVE, 0.12) }, None);
    canvas.draw_rrect(rrect, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(if on { rgba(ROSE, 0.7) } else { rgba(MAUVE, 0.3) }, None);
    canvas.draw_rrect(rrect, &border);

    let font = fonts.fraunces(17.0);
    let mut text = Paint::default();
    text.set_anti_alias(true);
    text.set_color4f(if on { rgba(PEARL, 0.98) } else { rgba(MAUVE, 0.85) }, None);
    let label = if on {
        "Enabled — vanilla crosshair suppressed"
    } else {
        "Disabled — vanilla crosshair shown"
    };
    let (tw, _) = font.measure_str(label, Some(&text));
    canvas.draw_str(
        label,
        (rect.left + (rect.width() - tw) * 0.5, rect.top + rect.height() * 0.5 + 6.0),
        &font,
        &text,
    );
}

/// Inline boolean toggle pill — label + a small on/off mark.
fn draw_crosshair_bool(canvas: &Canvas, rect: Rect, label: &str, on: bool, fonts: &FontStore) {
    let rrect = skia_safe::RRect::new_rect_xy(rect, 8.0, 8.0);
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(if on { rgba(LAV, 0.18) } else { rgba(MAUVE, 0.08) }, None);
    canvas.draw_rrect(rrect, &bg);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(if on { rgba(LAV, 0.55) } else { rgba(MAUVE, 0.22) }, None);
    canvas.draw_rrect(rrect, &border);

    let dot_r = 5.0;
    let dot_cx = rect.left + 14.0;
    let dot_cy = rect.top + rect.height() * 0.5;
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(if on { rgba(ROSE, 0.95) } else { rgba(MAUVE, 0.4) }, None);
    canvas.draw_circle((dot_cx, dot_cy), dot_r, &dot);

    let font = fonts.newsreader(14.0);
    let mut text = Paint::default();
    text.set_anti_alias(true);
    text.set_color4f(if on { rgba(PEARL, 0.95) } else { rgba(MAUVE, 0.78) }, None);
    canvas.draw_str(label, (dot_cx + 12.0, dot_cy + 5.0), &font, &text);
}

/// Slider track with label above + value (and optional unit) on the right.
fn draw_crosshair_slider(
    canvas: &Canvas,
    rect: Rect,
    label: &str,
    value: f32,
    range: (f32, f32),
    unit: &str,
    decimals: usize,
    fonts: &FontStore,
) {
    let label_font = fonts.newsreader(13.0);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    label_paint.set_color4f(rgba(MAUVE, 0.92), None);
    canvas.draw_str(label, (rect.left, rect.top - 6.0), &label_font, &label_paint);

    // Right-aligned value tag.
    let val_str = if unit.is_empty() {
        format!("{:.*}", decimals, value)
    } else {
        format!("{:.*} {}", decimals, value, unit)
    };
    let val_font = fonts.jetbrains_mono(12.0);
    let mut val_paint = Paint::default();
    val_paint.set_anti_alias(true);
    val_paint.set_color4f(rgba(CHAMP, 0.95), None);
    let (vw, _) = val_font.measure_str(&val_str, Some(&val_paint));
    canvas.draw_str(&val_str, (rect.right - vw, rect.top - 6.0), &val_font, &val_paint);

    // Track + filled portion + knob.
    let (min, max) = range;
    let frac = ((value - min) / (max - min).max(1e-6)).clamp(0.0, 1.0);
    let inner_left = rect.left + 10.0;
    let inner_right = rect.right - 10.0;
    let track_y = rect.top + rect.height() * 0.5;
    let track = Rect::new(inner_left, track_y - 1.5, inner_right, track_y + 1.5);

    let mut track_paint = Paint::default();
    track_paint.set_anti_alias(true);
    track_paint.set_color4f(rgba(MAUVE, 0.22), None);
    canvas.draw_rrect(skia_safe::RRect::new_rect_xy(track, 1.5, 1.5), &track_paint);

    let fill_x = inner_left + (inner_right - inner_left) * frac;
    let fill = Rect::new(inner_left, track_y - 1.5, fill_x, track_y + 1.5);
    let mut fill_paint = Paint::default();
    fill_paint.set_anti_alias(true);
    fill_paint.set_color4f(rgba(ROSE, 0.85), None);
    canvas.draw_rrect(skia_safe::RRect::new_rect_xy(fill, 1.5, 1.5), &fill_paint);

    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(rgba(PEARL, 0.95), None);
    canvas.draw_circle((fill_x, track_y), 6.0, &knob);
}

/// Colour-state swatch — fills the pill with the slot's current colour and
/// outlines it (thicker outline = currently selected).
fn draw_crosshair_swatch(canvas: &Canvas, rect: Rect, label: &str, color: [u8; 4], selected: bool, fonts: &FontStore) {
    let rrect = skia_safe::RRect::new_rect_xy(rect, 8.0, 8.0);
    // A small chess-pattern background ghosts through transparent colours so
    // the user knows the alpha channel is in play.
    let mut ghost = Paint::default();
    ghost.set_anti_alias(true);
    ghost.set_color4f(rgba(MAUVE, 0.12), None);
    canvas.draw_rrect(rrect, &ghost);

    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(crate::crosshair::color4f(color), None);
    canvas.draw_rrect(rrect, &fill);

    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(if selected { 2.0 } else { 1.0 });
    border.set_color4f(if selected { rgba(PEARL, 1.0) } else { rgba(MAUVE, 0.45) }, None);
    canvas.draw_rrect(rrect, &border);

    let font = fonts.jetbrains_mono(10.0);
    let mut text = Paint::default();
    text.set_anti_alias(true);
    // Pick legible text colour: light on dark fills, dark on light fills.
    let luma = 0.299 * color[0] as f32 + 0.587 * color[1] as f32 + 0.114 * color[2] as f32;
    text.set_color4f(
        if luma > 140.0 { rgba((15, 0, 8), 0.95) } else { rgba(PEARL, 0.95) },
        None,
    );
    let (tw, _) = font.measure_str(label, Some(&text));
    canvas.draw_str(
        label,
        (rect.left + (rect.width() - tw) * 0.5, rect.top + rect.height() * 0.5 + 4.0),
        &font,
        &text,
    );
}

// ── HSV picker controls ──────────────────────────────────────────────────

/// Saturation/Value square — paints the canonical 3-pass colour-picker
/// square (solid pure hue → horizontal white→transparent → vertical
/// transparent→black) plus a ring cursor at the current (S, V).
fn draw_sv_square(canvas: &Canvas, rect: Rect, pure_hue_rgb: [u8; 3], s: f32, v: f32) {
    use skia_safe::{gradient_shader, Point, RRect, TileMode};
    let rrect = RRect::new_rect_xy(rect, 6.0, 6.0);
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));

    // 1. Solid pure hue.
    let mut hue_fill = Paint::default();
    hue_fill.set_color4f(
        Color4f::new(
            pure_hue_rgb[0] as f32 / 255.0,
            pure_hue_rgb[1] as f32 / 255.0,
            pure_hue_rgb[2] as f32 / 255.0,
            1.0,
        ),
        None,
    );
    canvas.draw_rect(rect, &hue_fill);

    // 2. Horizontal white→transparent (saturation axis).
    let mut sat_paint = Paint::default();
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[Color4f::new(1.0, 1.0, 1.0, 1.0), Color4f::new(1.0, 1.0, 1.0, 0.0)],
            None,
        ),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        sat_paint.set_shader(shader);
    }
    canvas.draw_rect(rect, &sat_paint);

    // 3. Vertical transparent→black (value axis).
    let mut val_paint = Paint::default();
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.left, rect.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[Color4f::new(0.0, 0.0, 0.0, 0.0), Color4f::new(0.0, 0.0, 0.0, 1.0)],
            None,
        ),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        val_paint.set_shader(shader);
    }
    canvas.draw_rect(rect, &val_paint);

    canvas.restore_to_count(saved);

    // Outline + cursor ring.
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(rgba(MAUVE, 0.45), None);
    canvas.draw_rrect(rrect, &rim);

    let cx = rect.left + s * rect.width();
    let cy = rect.top + (1.0 - v) * rect.height();
    let mut ring = Paint::default();
    ring.set_anti_alias(true);
    ring.set_style(PaintStyle::Stroke);
    ring.set_stroke_width(2.0);
    // White outline, then a thin black inner ring — readable on both light
    // and dark backgrounds inside the square.
    ring.set_color4f(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
    canvas.draw_circle((cx, cy), 7.0, &ring);
    ring.set_stroke_width(1.0);
    ring.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.9), None);
    canvas.draw_circle((cx, cy), 5.5, &ring);
}

/// Hue strip — vertical 6-stop rainbow gradient (red→yellow→green→cyan→
/// blue→magenta→red). Cursor knob at y proportional to H/360.
fn draw_hue_strip(canvas: &Canvas, rect: Rect, h: f32) {
    use skia_safe::{gradient_shader, Point, RRect, TileMode};
    let rrect = RRect::new_rect_xy(rect, 6.0, 6.0);
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));

    let stops = [
        Color4f::new(1.0, 0.0, 0.0, 1.0), // 0°
        Color4f::new(1.0, 1.0, 0.0, 1.0), // 60°
        Color4f::new(0.0, 1.0, 0.0, 1.0), // 120°
        Color4f::new(0.0, 1.0, 1.0, 1.0), // 180°
        Color4f::new(0.0, 0.0, 1.0, 1.0), // 240°
        Color4f::new(1.0, 0.0, 1.0, 1.0), // 300°
        Color4f::new(1.0, 0.0, 0.0, 1.0), // 360°
    ];
    let mut paint = Paint::default();
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.left, rect.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(&stops, None),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        paint.set_shader(shader);
    }
    canvas.draw_rect(rect, &paint);

    canvas.restore_to_count(saved);

    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(rgba(MAUVE, 0.45), None);
    canvas.draw_rrect(rrect, &rim);

    // Cursor — a horizontal bar straddling the strip at the picked hue.
    let cy = rect.top + (h / 360.0) * rect.height();
    let bar = Rect::new(rect.left - 3.0, cy - 2.5, rect.right + 3.0, cy + 2.5);
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
    canvas.draw_rrect(RRect::new_rect_xy(bar, 2.5, 2.5), &knob);
    knob.set_style(PaintStyle::Stroke);
    knob.set_stroke_width(1.0);
    knob.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.9), None);
    canvas.draw_rrect(RRect::new_rect_xy(bar, 2.5, 2.5), &knob);
}

/// Alpha strip — checkerboard backdrop + vertical gradient from the
/// current colour (opaque, top) to the same colour (transparent, bottom).
/// Cursor at y = (1 - alpha) * height.
fn draw_alpha_strip(canvas: &Canvas, rect: Rect, rgb: [u8; 3], a: f32) {
    use skia_safe::{gradient_shader, Point, RRect, TileMode};
    let rrect = RRect::new_rect_xy(rect, 6.0, 6.0);
    let saved = canvas.save();
    canvas.clip_rrect(rrect, Some(ClipOp::Intersect), Some(true));

    // Checkerboard — tiny squares so the user sees alpha as "see-through".
    let tile = 6.0;
    let cols = (rect.width() / tile).ceil() as i32;
    let rows = (rect.height() / tile).ceil() as i32;
    let mut light = Paint::default();
    light.set_color4f(Color4f::new(0.78, 0.78, 0.78, 1.0), None);
    let mut dark = Paint::default();
    dark.set_color4f(Color4f::new(0.55, 0.55, 0.55, 1.0), None);
    canvas.draw_rect(rect, &light);
    for row in 0..rows {
        for col in 0..cols {
            if (row + col) & 1 == 0 {
                continue;
            }
            let tx = rect.left + col as f32 * tile;
            let ty = rect.top + row as f32 * tile;
            canvas.draw_rect(
                Rect::from_xywh(tx, ty, tile, tile),
                &dark,
            );
        }
    }

    // Solid colour → transparent overlay.
    let opaque = Color4f::new(
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        1.0,
    );
    let transparent = Color4f::new(opaque.r, opaque.g, opaque.b, 0.0);
    let mut paint = Paint::default();
    if let Some(shader) = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.left, rect.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(&[opaque, transparent], None),
        None,
        TileMode::Clamp,
        None,
        None,
    ) {
        paint.set_shader(shader);
    }
    canvas.draw_rect(rect, &paint);

    canvas.restore_to_count(saved);

    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(rgba(MAUVE, 0.45), None);
    canvas.draw_rrect(rrect, &rim);

    let cy = rect.top + (1.0 - a) * rect.height();
    let bar = Rect::new(rect.left - 3.0, cy - 2.5, rect.right + 3.0, cy + 2.5);
    let mut knob = Paint::default();
    knob.set_anti_alias(true);
    knob.set_color4f(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
    canvas.draw_rrect(RRect::new_rect_xy(bar, 2.5, 2.5), &knob);
    knob.set_style(PaintStyle::Stroke);
    knob.set_stroke_width(1.0);
    knob.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.9), None);
    canvas.draw_rrect(RRect::new_rect_xy(bar, 2.5, 2.5), &knob);
}
