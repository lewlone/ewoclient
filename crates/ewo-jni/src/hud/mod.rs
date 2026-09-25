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

use std::cell::RefCell;
use std::path::PathBuf;

use ewo_core::modules as catalog;
use ewo_render::text::{draw_tracked_em, measure_tracked_em};
use ewo_render::widgets::{
    draw_liquid_glass, GlassBackdrop, LiquidGlassParams,
};
use ewo_render::FontStore;
use skia_safe::{
    gradient_shader, BlurStyle, Canvas, ClipOp, Color4f, Data, Font, Image, MaskFilter, Paint,
    PaintStyle, Point, RRect, Rect, TileMode,
};

// ── Velvet theme tokens — straight from `ewo_core::theme::Theme::VELVET` ────
const fn tok(c: ewo_core::color::Srgb) -> (u8, u8, u8) {
    (c.r, c.g, c.b)
}
const VELVET: ewo_core::theme::Theme = ewo_core::theme::Theme::VELVET;
const PEARL: (u8, u8, u8) = tok(VELVET.text_pearl); // --text-pearl
const MAUVE: (u8, u8, u8) = tok(VELVET.text_mauve); // --text-mauve
const ROSE: (u8, u8, u8) = tok(VELVET.accent_rose); // --accent-rose
const LAV: (u8, u8, u8) = tok(VELVET.accent_lav); // --accent-lav
const BERRY: (u8, u8, u8) = tok(VELVET.accent_berry); // --accent-berry
const CHAMP: (u8, u8, u8) = tok(VELVET.accent_champ); // --accent-champ
const WINE: (u8, u8, u8) = tok(VELVET.bg_wine_b); // --bg-wine-b

fn rgba(c: (u8, u8, u8), a: f32) -> Color4f {
    Color4f::new(c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0, a)
}

// ────────────────────────────────────────────────────────────────────────
// Per-frame context — the clock and the live-game snapshots.
// ────────────────────────────────────────────────────────────────────────

/// The two blurred snapshots of the live game that [`liquid_glass`] samples.
///
/// `Image` is a refcounted handle, so cloning one of these is a couple of
/// atomic bumps, not a pixel copy.
///
/// [`liquid_glass`]: ewo_render::widgets::liquid_glass
#[derive(Clone)]
pub struct GlassSource {
    /// Lightly blurred — what the refracting bevel bends. Needs structure.
    pub rim: Image,
    /// `rim`'s size relative to the framebuffer (0.5 = half resolution).
    pub rim_scale: f32,
    /// Heavily blurred — what shows through the flat interior. In-game this is
    /// the cached frost surface the overlay already maintains.
    pub frost: Image,
    pub frost_scale: f32,
}

/// Everything a frame needs that isn't widget data: the clock, the glass
/// sources, and the user's glass-strength preference.
#[derive(Clone)]
pub struct Frame {
    /// Wall-clock seconds since launch. Drives every animation.
    pub time: f32,
    /// `None` before the first game snapshot has been captured, or when the
    /// platform path can't provide one — widgets then fall back to the flat
    /// plate rather than drawing nothing.
    pub glass: Option<GlassSource>,
    /// Glass intensity, 0..2, from the overlay SETTINGS tab. 0 disables
    /// refraction entirely and uses the flat plate.
    pub glass_strength: f32,
}

thread_local! {
    /// Installed for the duration of one [`draw`] and cleared on the way out.
    ///
    /// A thread-local rather than a parameter because the plate is drawn from
    /// ~25 call sites nested several layers deep inside this module; threading
    /// a `&Frame` through every intermediate `draw_*` signature would be a far
    /// larger diff than the change it carries, and would bury it. The HUD is
    /// painted on exactly one thread (Minecraft's render thread, or the
    /// harness's main thread), and [`FrameGuard`] makes the "set for the
    /// duration of draw" invariant impossible to violate.
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
}

/// Clears the installed [`Frame`] on drop, including on unwind.
struct FrameGuard;

impl Drop for FrameGuard {
    fn drop(&mut self) {
        FRAME.with(|f| *f.borrow_mut() = None);
    }
}

fn install_frame(frame: Frame) -> FrameGuard {
    FRAME.with(|f| *f.borrow_mut() = Some(frame));
    FrameGuard
}

/// Seconds since launch. `0.0` outside a `draw` — callers get a still frame
/// rather than a panic.
fn now() -> f32 {
    FRAME.with(|f| f.borrow().as_ref().map(|fr| fr.time).unwrap_or(0.0))
}

/// Run `f` with the glass sources, if this frame has them and the user hasn't
/// turned glass off.
fn with_glass<R>(f: impl FnOnce(&GlassSource, f32) -> R) -> Option<R> {
    FRAME.with(|cell| {
        let borrow = cell.borrow();
        let frame = borrow.as_ref()?;
        if frame.glass_strength <= 0.0 {
            return None;
        }
        let src = frame.glass.as_ref()?;
        Some(f(src, frame.glass_strength))
    })
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
///
/// `pub(crate)` so [`crate::fixture`] can synthesise a buffer for the offscreen
/// render harness using the same offsets the real reader uses — a fixture that
/// hard-coded its own copy would drift silently on the next schema bump.
pub(crate) mod off {
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
/// Total bytes the reader may touch — one past the last field. Mirrors
/// `EwoHudData.CAPACITY`; [`crate::fixture`] allocates this much.
pub const BLOCK_BYTES: usize = 1312;
// `pub(crate)` for the same reason as `off` — the fixture sets these flags.
pub(crate) const FLAG_WORLD: i32 = 1; // a player + level exist → coords/keystrokes valid
pub(crate) const FLAG_PING: i32 = 1 << 1; // a server connection exists → ping valid
pub(crate) const FLAG_ARMOR: i32 = 1 << 2; // at least one armor piece is worn
pub(crate) const FLAG_TARGET: i32 = 1 << 3; // an entity is under the crosshair
pub(crate) const FLAG_OVERLAY: i32 = 1 << 4; // the EwoClient overlay is open
pub(crate) const FLAG_PVP_JUMP: i32 = 1 << 5; // a fresh jump-reset result is live
pub(crate) const FLAG_PVP_HIT: i32 = 1 << 6;  // a fresh hit-range result is live

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

/// Number of HUD widgets. Every per-widget array is sized by this, so adding
/// a variant is one edit here plus its `ALL` entry — never a stale `[_; N]`
/// that a `WidgetId::ALL[i]` index then walks off the end of.
const WIDGET_COUNT: usize = 17;

impl WidgetId {
    const ALL: [WidgetId; WIDGET_COUNT] = [
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

/// One widget's placement: an anchor, a fractional (0..1) anchor point, and a
/// size multiplier.
#[derive(Clone, Copy)]
struct WidgetLayout {
    enabled: bool,
    anchor: Anchor,
    x: f32,
    y: f32,
    /// Size multiplier, [`SCALE_MIN`]..=[`SCALE_MAX`].
    ///
    /// Applied as a canvas transform about the anchor point rather than by
    /// threading a scale into all 17 widget renderers. That is safe for text:
    /// Skia re-shapes and re-rasterizes glyphs at the effective size, so a
    /// scaled widget is as crisp as a natively-sized one (measured — only
    /// pre-rasterizing to a layer and *then* scaling goes soft). CLAUDE.md's
    /// "don't transform anything that contains text" is about *animating*
    /// scale, which re-rasterizes every frame; this is a static size the user
    /// chose, rasterized once.
    scale: f32,
}

/// Widget size-multiplier bounds. The floor keeps a widget big enough to still
/// grab its resize handle; the ceiling keeps one from swallowing the screen.
pub const SCALE_MIN: f32 = 0.5;
pub const SCALE_MAX: f32 = 3.0;

/// Scale `r` about `(cx, cy)` — maps a rect drawn under a canvas scale
/// transform back into screen coordinates for hit-testing.
fn scale_rect_about(r: Rect, cx: f32, cy: f32, s: f32) -> Rect {
    Rect::new(
        cx + (r.left - cx) * s,
        cy + (r.top - cy) * s,
        cx + (r.right - cx) * s,
        cy + (r.bottom - cy) * s,
    )
}

/// The persisted HUD config — the per-widget layout plus HUD prefs. Saved to
/// `hud.toml`.
struct HudLayout {
    widgets: [WidgetLayout; WIDGET_COUNT],
    /// The paint-rate cap — a pref, kept here so it shares `hud.toml`.
    paint_rate: crate::HudPaintRate,
    /// Liquid-glass intensity, [`GLASS_MIN`]..=[`GLASS_MAX`]. Scales the
    /// bevel width, refraction and dispersion together; `0` falls back to the
    /// flat plate entirely.
    glass_strength: f32,
}

/// Glass-strength bounds. `0` is a real setting — the flat plate — so the
/// minimum is 0 rather than something small-but-refracting.
pub const GLASS_MIN: f32 = 0.0;
pub const GLASS_MAX: f32 = 2.0;
const GLASS_DEFAULT: f32 = 1.0;

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
                WidgetLayout { enabled: true, anchor: Anchor::Tl, x: 0.0135, y: 0.0204, scale: 1.0 }, // fps
                WidgetLayout { enabled: true, anchor: Anchor::Tl, x: 0.0135, y: 0.0620, scale: 1.0 }, // coords
                WidgetLayout { enabled: true, anchor: Anchor::Br, x: 0.9865, y: 0.9759, scale: 1.0 }, // ping
                WidgetLayout { enabled: true, anchor: Anchor::Bl, x: 0.0135, y: 0.9759, scale: 1.0 }, // keystrokes
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.9330, scale: 1.0 }, // armor
                WidgetLayout { enabled: true, anchor: Anchor::Tr, x: 0.9865, y: 0.3000, scale: 1.0 }, // potions
                WidgetLayout { enabled: true, anchor: Anchor::Tc, x: 0.5000, y: 0.0593, scale: 1.0 }, // target
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.8700, scale: 1.0 }, // jump_reset_text
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.8300, scale: 1.0 }, // jump_reset_bar
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.7500, scale: 1.0 }, // hit_range
                WidgetLayout { enabled: true, anchor: Anchor::Tr, x: 0.9865, y: 0.0204, scale: 1.0 }, // cps
                WidgetLayout { enabled: true, anchor: Anchor::Bl, x: 0.0135, y: 0.9000, scale: 1.0 }, // items
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.6800, scale: 1.0 }, // shield_cooldown
                WidgetLayout { enabled: true, anchor: Anchor::Tc, x: 0.5000, y: 0.1100, scale: 1.0 }, // reach
                WidgetLayout { enabled: true, anchor: Anchor::Bc, x: 0.5000, y: 0.5800, scale: 1.0 }, // attack_charge
                WidgetLayout { enabled: true, anchor: Anchor::Tc, x: 0.5000, y: 0.1800, scale: 1.0 }, // combo
                // Media — default off (the user opts in) at top-right under the
                // CPS chip. Once they enable + place it the layout is persisted.
                WidgetLayout { enabled: false, anchor: Anchor::Tr, x: 0.9865, y: 0.1100, scale: 1.0 }, // media
            ],
            paint_rate: crate::HudPaintRate::Match,
            glass_strength: GLASS_DEFAULT,
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
                match key {
                    "paint_rate" => {
                        if let Some(r) = crate::HudPaintRate::from_str(value) {
                            layout.paint_rate = r;
                        }
                    }
                    "glass_strength" => {
                        if let Ok(v) = value.parse::<f32>() {
                            layout.glass_strength = v.clamp(GLASS_MIN, GLASS_MAX);
                        }
                    }
                    _ => {}
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
                "scale" => {
                    if let Ok(v) = value.parse::<f32>() {
                        wl.scale = v.clamp(SCALE_MIN, SCALE_MAX);
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
                "\n[{}]\nenabled = {}\nanchor = \"{}\"\nx = {:.5}\ny = {:.5}\nscale = {:.3}\n",
                id.key(),
                wl.enabled,
                wl.anchor.as_str(),
                wl.x,
                wl.y,
                wl.scale,
            ));
        }
        s.push_str(&format!(
            "\n[prefs]\npaint_rate = \"{}\"\nglass_strength = {:.3}\n",
            self.paint_rate.as_str(),
            self.glass_strength,
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
    Friends,
    Settings,
}

impl OverlayView {
    const ALL: [OverlayView; 8] = [
        OverlayView::Home,
        OverlayView::HudEditor,
        OverlayView::Crosshair,
        OverlayView::Modules,
        OverlayView::Pvp,
        OverlayView::Mods,
        OverlayView::Friends,
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
            OverlayView::Friends => "FRIENDS",
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

/// An in-progress corner-drag resize.
///
/// Scale tracks the cursor's distance from the *anchor* point rather than a
/// raw pixel delta, so dragging feels the same whichever corner the handle
/// sits on and whatever the widget's current size.
#[derive(Clone, Copy)]
struct ResizeDrag {
    id: WidgetId,
    /// Scale when the handle was grabbed.
    start_scale: f32,
    /// Cursor distance from the anchor point when it was grabbed.
    grab_dist: f32,
}

/// Side of the square resize handle drawn on the selected widget, in px.
const RESIZE_HANDLE: f32 = 14.0;

/// The handle's rect for a widget occupying `bounds`.
///
/// Placed at the corner furthest from the anchor, so dragging it always moves
/// *away* from the pinned point and never folds the widget through itself.
fn resize_handle_rect(bounds: Rect, anchor: Anchor) -> Rect {
    // Horizontal: anchored right → handle on the left edge, else the right.
    let hx = match anchor {
        Anchor::Tr | Anchor::Mr | Anchor::Br => bounds.left,
        _ => bounds.right,
    };
    // Vertical: anchored bottom → handle on the top edge, else the bottom.
    let hy = match anchor {
        Anchor::Bl | Anchor::Bc | Anchor::Br => bounds.top,
        _ => bounds.bottom,
    };
    Rect::from_xywh(
        hx - RESIZE_HANDLE * 0.5,
        hy - RESIZE_HANDLE * 0.5,
        RESIZE_HANDLE,
        RESIZE_HANDLE,
    )
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
    bounds: [Rect; WIDGET_COUNT],
    dragging: Option<Drag>,
    /// An in-progress corner-drag resize of the selected widget.
    resizing: Option<ResizeDrag>,
    /// Media state was pinned by [`Self::set_media`] — skip the SMTC poll so
    /// the pin survives. Only the offscreen harness sets this.
    media_pinned: bool,
    /// Spectrum was pinned by [`Self::set_spectrum`] — skip the audio poll.
    /// Only the offscreen harness sets this; a visualiser that depends on
    /// what is playing on the machine cannot be screenshotted deterministically.
    spectrum_pinned: bool,
    /// Quick-edit: the modifier is held over a cursor-free vanilla screen
    /// (inventory, pause, chat…), so widgets can be dragged and resized
    /// without opening the EwoClient overlay. Driven from Java each frame.
    quick_edit: bool,
    /// Whether the *overlay* is open this frame. Mirrors the data-block flag,
    /// recorded in `draw` so the input handlers — which run outside `draw` and
    /// have no `HudData` — can tell the two edit modes apart.
    overlay_open: bool,
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
    /// `ewo-skin.png`'s (mtime, length) when it was last loaded — the export
    /// thread rewrites the file after the `Editor` was built, and may also
    /// replace a stale png left by an earlier launch, so the viewer reloads on
    /// a change. A decode failure is cached under its stamp too: a corrupt
    /// file is retried when it changes, not every frame.
    skin_stamp: Option<(std::time::SystemTime, u64)>,
    /// Frame clock of the last `ewo-skin.png` stat — polled, not per paint.
    skin_checked_at: f32,
    /// PvP-Utils config — loaded from the active profile's `pvp.toml`,
    /// edited from the PVP overlay tab, saved on each commit. The Java mod
    /// polls the file's mtime and hot-reloads — so edits apply live.
    pvp: crate::pvp::PvpConfig,
    /// An in-progress PVP-tab slider drag — identifies which control is held.
    pvp_drag: Option<PvpDrag>,
    /// The SETTINGS-tab glass-strength slider is held.
    glass_drag: bool,
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
    /// System-audio capture feeding the media visualiser.
    pub(crate) audio_service: crate::audio::AudioService,
    /// This frame's spectrum. Polled once at the top of [`draw`] and read from
    /// several places below, so the whole frame agrees on one value.
    pub(crate) spectrum: crate::audio::Spectrum,
    /// Vertical scroll offset (in logical px) for the MODULES tab — the only
    /// dashboard view tall enough to need scrolling. Reset whenever the view
    /// changes so a switch in/out always starts at the top.
    pub(crate) modules_scroll: f32,
    /// Per-frame cache of the HOME quick-toggle chip rects. Chips are flowed
    /// at render time (their width depends on the text) so the press handler
    /// can't compute them without a `FontStore`; the renderer writes here.
    pub(crate) home_toggle_bounds: [Rect; WIDGET_COUNT],
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

mod editor;
use editor::*;
mod widgets_core;
pub use widgets_core::*;
mod widgets;
use widgets::*;
mod editor_view;
use editor_view::*;
mod overlay;
pub(crate) use overlay::*;
mod overlay_mods;
use overlay_mods::*;
mod overlay_pvp;
use overlay_pvp::*;
mod overlay_modules;
use overlay_modules::*;
mod overlay_crosshair;
use overlay_crosshair::*;
