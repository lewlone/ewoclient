//! Synthetic [`HudData`](crate::hud::HudData) blocks for the offscreen render
//! harness (`examples/hudshot.rs`).
//!
//! In-game the HUD reads a direct `ByteBuffer` the Java mod fills each frame.
//! Nothing outside a running Minecraft can produce one — which is why the
//! overlay historically could only be looked at by launching the game. This
//! module writes the same wire layout into a plain `Vec<u8>`, so the whole HUD
//! can be rendered to a PNG from a `cargo run`.
//!
//! Field offsets come from [`crate::hud::off`] rather than a local copy, so a
//! schema bump moves the fixture with the reader instead of silently drifting
//! into garbage.

use crate::hud::{
    off, BLOCK_BYTES, FLAG_ARMOR, FLAG_OVERLAY, FLAG_PING, FLAG_PVP_HIT, FLAG_PVP_JUMP,
    FLAG_TARGET, FLAG_WORLD, INDICATOR_RECORD, SCHEMA_VERSION,
};

/// Which slice of game state the fixture should portray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    /// Mid-fight on a server: a target under the crosshair, armor damaged,
    /// potions up, every PvP readout live. The stress case — this is what
    /// shows whether the HUD stays legible when everything is on at once.
    Combat,
    /// Walking around, nothing hostile. Most widgets idle or absent — the
    /// case that shows whether the HUD is quiet when it should be.
    Explore,
    /// Sitting on the main menu: no world, no server. Almost everything
    /// should hide itself.
    Menu,
}

impl Scene {
    /// Parse a `--scene` argument. Returns `None` for an unknown name.
    pub fn parse(s: &str) -> Option<Scene> {
        match s.to_ascii_lowercase().as_str() {
            "combat" => Some(Scene::Combat),
            "explore" => Some(Scene::Explore),
            "menu" => Some(Scene::Menu),
            _ => None,
        }
    }

    pub const ALL: [Scene; 3] = [Scene::Combat, Scene::Explore, Scene::Menu];

    pub fn name(self) -> &'static str {
        match self {
            Scene::Combat => "combat",
            Scene::Explore => "explore",
            Scene::Menu => "menu",
        }
    }
}

/// Little-endian scalar writer over the block.
struct Block(Vec<u8>);

impl Block {
    fn new() -> Self {
        Block(vec![0u8; BLOCK_BYTES])
    }
    fn i32(&mut self, at: usize, v: i32) -> &mut Self {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(&mut self, at: usize, v: f32) -> &mut Self {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
        self
    }
    fn f64(&mut self, at: usize, v: f64) -> &mut Self {
        self.0[at..at + 8].copy_from_slice(&v.to_le_bytes());
        self
    }
    /// Mirror of `EwoHudData.putString`: i32 byte-length, then the UTF-8.
    /// Truncated at `cap` on a char boundary so a clipped string never
    /// decodes as replacement characters.
    fn string(&mut self, at: usize, cap: usize, v: &str) -> &mut Self {
        let mut len = v.len().min(cap);
        while len > 0 && !v.is_char_boundary(len) {
            len -= 1;
        }
        self.i32(at, len as i32);
        self.0[at + 4..at + 4 + len].copy_from_slice(&v.as_bytes()[..len]);
        self
    }
}

/// One `{ i32 duration_ticks, i32 amplifier, i32 packed_rgb, str name }` record.
fn potion(b: &mut Block, slot: usize, ticks: i32, amp: i32, rgb: i32, name: &str) {
    // POTION_REC / POTION_NAME_CAP are private to `hud`; the record shape is
    // fixed by the schema, so mirror it here rather than widen their scope.
    const REC: usize = 44;
    const NAME_CAP: usize = 28;
    let at = off::POTIONS + slot * REC;
    b.i32(at, ticks).i32(at + 4, amp).i32(at + 8, rgb).string(at + 12, NAME_CAP, name);
}

/// One indicator record — a world-anchored entity the overhead totem counter
/// and floating health bar attach to. `sx`/`sy` are framebuffer pixels.
#[allow(clippy::too_many_arguments)]
fn indicator(
    b: &mut Block,
    slot: usize,
    id: i32,
    sx: f32,
    sy: f32,
    distance: f32,
    totems: i32,
    hp: f32,
    max_hp: f32,
    last_damage: f32,
    damage_age: f32,
) {
    let at = off::INDICATORS + 4 + slot * INDICATOR_RECORD;
    b.i32(at, id)
        .f32(at + 4, sx)
        .f32(at + 8, sy)
        .f32(at + 12, distance)
        .i32(at + 16, 1) // in_view
        .i32(at + 20, totems)
        .f32(at + 24, hp)
        .f32(at + 28, max_hp)
        .f32(at + 32, last_damage)
        .f32(at + 36, damage_age);
}

/// Build a data block for `scene`.
///
/// `overlay_open` sets the flag the dashboard branches on. `(w, h)` is the
/// framebuffer size — world-anchored indicators are positioned in pixels, so
/// they need to know how big the frame is to land somewhere sensible.
pub fn block(scene: Scene, overlay_open: bool, w: f32, h: f32) -> Vec<u8> {
    let mut b = Block::new();
    b.i32(0, SCHEMA_VERSION);

    let overlay = if overlay_open { FLAG_OVERLAY } else { 0 };

    match scene {
        Scene::Menu => {
            // No world, no server. Only the FPS widget is `widget_available`.
            b.i32(off::FLAGS, overlay);
            b.i32(off::FPS, 1240);
            b.i32(off::PLAYTIME, 96);
            b.string(off::SERVER, 48, "");
            b.string(off::PLAYER_NAME, 24, "lewlone");
        }

        Scene::Explore => {
            b.i32(off::FLAGS, FLAG_WORLD | FLAG_PING | FLAG_ARMOR | overlay);
            b.i32(off::FPS, 487);
            b.i32(off::PING, 31);
            // Holding W + space — a walk-and-jump, so the keystroke widget
            // shows a mix of lit and unlit keys rather than all-off.
            b.i32(off::KEYS, 0b1_0001);
            b.f64(off::X, 214.5).f64(off::Y, 72.0).f64(off::Z, -1043.31);

            // Fresh-ish gear: one worn helmet, the rest healthy.
            for (i, dura) in [0.41_f32, 0.97, 0.88, 0.93].iter().enumerate() {
                b.i32(off::ARMOR + i * 8, 1).f32(off::ARMOR + i * 8 + 4, *dura);
            }

            b.i32(off::POTION_COUNT, 1);
            potion(&mut b, 0, 3_180, 0, 0x33EB_FF, "Night Vision");

            b.i32(off::PLAYTIME, 4_127);
            b.string(off::SERVER, 48, "play.frogsy.net");
            b.string(off::PLAYER_NAME, 24, "lewlone");

            b.i32(off::CPS_LEFT, 0).i32(off::CPS_RIGHT, 0);
            b.i32(off::ITEM_PEARLS, 4)
                .i32(off::ITEM_ARROWS, 64)
                .i32(off::ITEM_TOTEMS, 1)
                .i32(off::ITEM_GAPPLES, 6);
            b.f32(off::ATTACK_CHARGE, 1.0);
        }

        Scene::Combat => {
            b.i32(
                off::FLAGS,
                FLAG_WORLD | FLAG_PING | FLAG_ARMOR | FLAG_TARGET | FLAG_PVP_JUMP | FLAG_PVP_HIT
                    | overlay,
            );
            b.i32(off::FPS, 512);
            b.i32(off::PING, 23);
            // Strafing left while sprinting forward, mid-jump.
            b.i32(off::KEYS, 0b1_0011);
            b.f64(off::X, 128.5).f64(off::Y, 71.0).f64(off::Z, -1042.25);

            // Deliberately spread across the durability range so every bar
            // state (healthy / worn / critical) appears in one shot.
            for (i, dura) in [0.92_f32, 0.64, 0.38, 0.11].iter().enumerate() {
                b.i32(off::ARMOR + i * 8, 1).f32(off::ARMOR + i * 8 + 4, *dura);
            }

            b.i32(off::POTION_COUNT, 3);
            potion(&mut b, 0, 1_684, 1, 0x7CAF_C6, "Speed");
            potion(&mut b, 1, 947, 0, 0x9324_23, "Strength");
            // Negative duration = infinite — exercises the "∞" formatting path.
            potion(&mut b, 2, -1, 0, 0xE49A_3A, "Fire Resistance");

            b.i32(off::TARGET_PRESENT, 1)
                .f32(off::TARGET_DIST, 3.24)
                .f32(off::TARGET_HP, 12.5)
                .f32(off::TARGET_MAXHP, 20.0)
                .string(off::TARGET_NAME, 44, "Vwyla");

            b.i32(off::PLAYTIME, 4_127);
            b.string(off::SERVER, 48, "play.frogsy.net");
            b.string(off::PLAYER_NAME, 24, "lewlone");

            // Jump reset: a PERFECT, 12 ms early, one fifth into its fade.
            b.i32(off::PVP_JUMP, 1)
                .i32(off::PVP_JUMP + 4, -12)
                .i32(off::PVP_JUMP + 8, 4)
                .i32(off::PVP_JUMP + 12, 20);
            // Hit range: 3.04 blocks, green zone, a bit further into its fade.
            b.f32(off::PVP_HIT, 3.04)
                .i32(off::PVP_HIT + 4, 0x8BE2_8B)
                .i32(off::PVP_HIT + 8, 6)
                .i32(off::PVP_HIT + 12, 20);

            b.i32(off::CPS_LEFT, 7).i32(off::CPS_RIGHT, 1);
            b.i32(off::ITEM_PEARLS, 12)
                .i32(off::ITEM_ARROWS, 64)
                .i32(off::ITEM_TOTEMS, 3)
                .i32(off::ITEM_GAPPLES, 17);

            // Three tracked entities spread across the frame: a heavily-popped
            // totem user, a fresh-damaged one, and a distant full-health one.
            b.i32(off::INDICATORS, 3);
            indicator(&mut b, 0, 101, w * 0.50, h * 0.42, 3.2, 4, 12.5, 20.0, 6.5, 0.25);
            indicator(&mut b, 1, 102, w * 0.28, h * 0.55, 8.7, 0, 3.0, 20.0, 9.0, 0.60);
            indicator(&mut b, 2, 103, w * 0.74, h * 0.48, 17.4, 1, 20.0, 20.0, 0.0, -1.0);

            b.f32(off::SHIELD_COOLDOWN, 0.42);
            b.i32(off::HIT_PRESENT, 1)
                .f32(off::HIT_REL_YAW, -117.0)
                .f32(off::HIT_AGE, 0.40);
            b.f32(off::ATTACK_CHARGE, 0.73);
            b.i32(off::COMBO_COUNT, 5).f32(off::COMBO_AGE, 0.8);
        }
    }

    b.0
}
