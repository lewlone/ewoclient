//! The survival HUD's gauges as a pure layout (M168): hearts, armour, food,
//! air, the vehicle's hearts, the effect icons and the jump bar — each a list
//! of [`HudBlit`]s in **GUI pixels**, in the order vanilla draws them.
//!
//! # Why a layout and not a draw
//!
//! The heart row that shipped with M3 was drawn inside `HudPass::draw` from
//! two integers, and stayed an approximation for 165 milestones because a
//! draw inside a Vulkan pass has no unit test: it rounded where vanilla
//! ceils, drew one row, knew nothing of absorption, blink, the heart types,
//! the regeneration wave or the low-health jitter, and filled the hunger row
//! **from the left** where `extractFood`'s `xRight - i * 8 - 9` fills it from
//! the right — identical pixels only because ten cells of eight mirror
//! exactly, and wrong the moment anything else used the same shape.
//!
//! This module is the tab list's pattern (M151): the *geometry* is a function
//! of plain inputs, the pass draws whatever it is handed, and a witness can
//! ask the function a question without a GPU. `gaugeshot` grades both halves.
//!
//! # The oracle
//!
//! `net.minecraft.client.gui.Hud` (26.2 decompile): `extractPlayerHealth`
//! (:762-812), `extractArmor` (:815-836), `extractHearts` (:839-892),
//! `extractAirBubbles` (:906-934) with its three helpers (:936-947),
//! `extractFood` (:958-991), `extractVehicleHealth` (:993-1020) with
//! `getPlayerVehicleWithHealth` / `getVehicleMaxHearts` /
//! `getVisibleVehicleHeartRows` (:728-760), `extractEffects` (:486-526),
//! `HeartType` (:1356-1476); `contextualbar/JumpableVehicleBar.java`;
//! `MobEffectInstance.compareTo` (:339-351); `Mth.lerpDiscrete` (:545-548).
//!
//! # Things a plausible implementation gets wrong, each pinned below
//!
//! * **Health is `Mth.ceil`ed**, so 0.3 health is a half heart. M3 rounded.
//! * **The rows compress.** `healthRowHeight = max(10 - (rows - 2), 3)`, so
//!   a second row sits 10 px up, a third 9 px, and the armour row rides on
//!   top of whatever that is: `yLineBase - (rows - 1) * rowHeight - 10`.
//! * **The jitter is a seeded LCG, not noise.** `random.setSeed(tickCount *
//!   312871)` once per frame (an `int` multiply that wraps, then widens),
//!   and every `nextInt` after it is in draw order — hearts first (one draw
//!   per container while `health + absorption <= 4`), then the food's
//!   `nextInt(3)` per icon on the wobble ticks, then the air's `nextInt(2)`
//!   per empty bubble when all ten are empty. Draw one out of order and every
//!   later offset changes.
//! * **Absorption hearts are WITHERED when withered**, else ABSORBING — not
//!   the player's own type.
//! * **Food is drawn right to left** and is skipped entirely while a living
//!   vehicle has hearts; the air row moves to where the food was.
//! * **`getAirBubbleYLine` subtracts `(rows - 1) * 10`** with `rows =
//!   ceil(hearts / 10.0)`, so with no vehicle `rows - 1 == -1` and the line
//!   moves DOWN ten — cancelling the ten the food branch took off.
//!   Transcribed, not simplified.
//! * **Three different `ceil`s decide the bubbles**: full ones with `-2`
//!   ticks of slack, the popping one with `0`, the empty count with `+1` only
//!   while underwater with air left.
//! * **NEUTRAL effects share the harmful row**, because the only test is
//!   `isBeneficial()`. An infinite effect never fades.
//! * **`lerpDiscrete(alpha, 0, 182)` is `floor(alpha * 181) + (alpha > 0)`**
//!   — a barely-pressed jump already shows one pixel.

use crate::hud::{HudBlit, HudIcon};

/// `Hud.HeartType`, in declaration order — which is also the order the
/// atlas packs their sprites (`HudSpritesData::player_hearts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeartKind {
    Container,
    Normal,
    /// `POISIONED` in the source, spelled that way.
    Poisoned,
    Withered,
    Absorbing,
    Frozen,
}

impl HeartKind {
    pub const ALL: [HeartKind; 6] = [
        HeartKind::Container,
        HeartKind::Normal,
        HeartKind::Poisoned,
        HeartKind::Withered,
        HeartKind::Absorbing,
        HeartKind::Frozen,
    ];

    pub fn index(self) -> usize {
        match self {
            HeartKind::Container => 0,
            HeartKind::Normal => 1,
            HeartKind::Poisoned => 2,
            HeartKind::Withered => 3,
            HeartKind::Absorbing => 4,
            HeartKind::Frozen => 5,
        }
    }

    /// `HeartType.forPlayer`: POISON, then WITHER, then `isFullyFrozen()`,
    /// else NORMAL — the order is the precedence.
    pub fn for_player(poisoned: bool, withered: bool, fully_frozen: bool) -> HeartKind {
        if poisoned {
            HeartKind::Poisoned
        } else if withered {
            HeartKind::Withered
        } else if fully_frozen {
            HeartKind::Frozen
        } else {
            HeartKind::Normal
        }
    }
}

/// `HeartType.getSprite(isHardcore, isHalf, isBlink)` as an index into the
/// eight sprites the enum constructor lists, in its order: full,
/// full_blinking, half, half_blinking, hardcore_full,
/// hardcore_full_blinking, hardcore_half, hardcore_half_blinking.
pub const HEART_SPRITES_PER_KIND: usize = 8;

pub fn heart_sprite_index(kind: HeartKind, hardcore: bool, half: bool, blink: bool) -> usize {
    kind.index() * HEART_SPRITES_PER_KIND
        + usize::from(hardcore) * 4
        + usize::from(half) * 2
        + usize::from(blink)
}

/// The three `hud/armor_*` sprites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmorSprite {
    Full,
    Half,
    Empty,
}

/// The six `hud/food_*` sprites: plain and `_hunger`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoodSprite {
    Full,
    Half,
    Empty,
    FullHunger,
    HalfHunger,
    EmptyHunger,
}

/// `hud/air`, `hud/air_bursting` (the field is `AIR_POPPING_SPRITE`; the
/// identifier is `air_bursting`), `hud/air_empty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AirSprite {
    Full,
    Bursting,
    Empty,
}

/// `hud/heart/vehicle_{container,full,half}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VehicleHeartSprite {
    Container,
    Full,
    Half,
}

/// `hud/jump_bar_{background,cooldown,progress}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JumpBarSprite {
    Background,
    Cooldown,
    /// Drawn as a sub-rectangle: the blit's `w` is the filled width and the
    /// pass clips the UV to `w / 182`.
    Progress,
}

/// `Entity.getMaxAirSupply()` — 300 for a player; carried as an input
/// because the gauge divides by it.
pub const MAX_AIR_SUPPLY: i32 = 300;

/// `Hud`'s own `RandomSource.create()` — a `LegacyRandomSource`, i.e.
/// `java.util.Random`'s 48-bit LCG, reseeded every frame with
/// `tickCount * 312871`.
///
/// A fourth copy of the LCG in this workspace, and deliberately so: the three
/// others are private to their modules (`mobs.rs`, `entities.rs`,
/// `celestial.rs`) and a public one in `rewo-world` is a crate this one does
/// not depend on. The vectors below pin it against the algorithm's own
/// definition rather than against a sibling copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudRandom {
    seed: i64,
}

impl HudRandom {
    const MULTIPLIER: i64 = 0x5DEECE66D;
    const ADDEND: i64 = 0xB;
    const MASK: i64 = (1 << 48) - 1;

    /// `setSeed(long)`: scramble with the multiplier and mask to 48 bits.
    pub fn with_seed(seed: i64) -> Self {
        Self {
            seed: (seed ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    /// `Hud.java:783` — `this.random.setSeed(this.tickCount * 312871)`. The
    /// multiply is `int * int` and WRAPS before the widening to `long`.
    pub fn for_tick(tick_count: i32) -> Self {
        Self::with_seed(i64::from(tick_count.wrapping_mul(312871)))
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(Self::MULTIPLIER).wrapping_add(Self::ADDEND)) & Self::MASK;
        (self.seed >> (48 - bits)) as i32
    }

    /// `nextInt(bound)` — the power-of-two shortcut takes the TOP bits of one
    /// draw; everything else rejects-and-retries on the low bits.
    pub fn next_int(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0);
        if bound & (bound - 1) == 0 {
            ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32
        } else {
            loop {
                let bits = self.next(31);
                let val = bits % bound;
                if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                    return val;
                }
            }
        }
    }
}

/// One of the local player's active effects, as `extractEffects` needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectInput {
    /// Raw `minecraft:mob_effect` registry id — the atlas slot.
    pub id: i32,
    /// `getDuration()`; `-1` is infinite.
    pub duration: i32,
    pub ambient: bool,
    pub show_icon: bool,
    /// `effect.value().isBeneficial()`.
    pub beneficial: bool,
    /// `effect.value().getColor()` — the comparator's last key.
    pub color: u32,
}

impl EffectInput {
    fn is_infinite(&self) -> bool {
        self.duration == -1
    }

    fn ends_within(&self, ticks: i32) -> bool {
        !self.is_infinite() && self.duration <= ticks
    }
}

/// `MobEffectInstance.compareTo` — NOT a total order. The first branch
/// (either finite-ish, not both ambient) keys on ambient, infinite,
/// duration, colour; the second on ambient, colour. Vanilla sorts with
/// `Ordering.natural().reverse()`, and Rewo reverses the same function.
///
/// Ties (same ambient, infinite, duration AND colour — `instant_health` and
/// `saturation` share `16262179`) are broken by `HashMap` iteration order in
/// vanilla, which nothing can reproduce; Rewo keeps the list's own order.
pub fn effect_compare(a: &EffectInput, b: &EffectInput) -> std::cmp::Ordering {
    const UPDATE_CUT_OFF: i32 = 32147;
    if (a.duration <= UPDATE_CUT_OFF || b.duration <= UPDATE_CUT_OFF) && (!a.ambient || !b.ambient) {
        // `compareFalseFirst`: false < true, which is `bool`'s own `Ord`.
        a.ambient
            .cmp(&b.ambient)
            .then(a.is_infinite().cmp(&b.is_infinite()))
            .then(a.duration.cmp(&b.duration))
            .then(a.color.cmp(&b.color))
    } else {
        a.ambient.cmp(&b.ambient).then(a.color.cmp(&b.color))
    }
}

/// `Hud.extractEffects`' icon alpha for a non-ambient effect
/// (`Hud.java:515-520`): 1.0 until the last 200 ticks, then a pulse that
/// dims as the seconds run out.
pub fn effect_icon_alpha(e: &EffectInput) -> f32 {
    if e.ambient || !e.ends_within(200) {
        return 1.0;
    }
    let remaining = e.duration;
    let used_seconds = 10 - remaining / 20;
    let a = (remaining as f32 / 10.0 / 5.0 * 0.5).clamp(0.0, 0.5)
        + crate::entities::mth_cos(f64::from(remaining as f32 * std::f32::consts::PI / 5.0))
            * (used_seconds as f32 / 10.0 * 0.25).clamp(0.0, 0.25);
    a.clamp(0.0, 1.0)
}

/// The living vehicle under the player, for `extractVehicleHealth`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleInput {
    /// `vehicle.getMaxHealth()` — the MAX_HEALTH attribute.
    pub max_health: f32,
    /// `vehicle.getHealth()`.
    pub health: f32,
}

/// `getVehicleMaxHearts`: `(int)(maxHealth + 0.5F) / 2`, capped at 30 —
/// the cast happens BEFORE the integer divide.
pub fn vehicle_max_hearts(v: Option<VehicleInput>) -> i32 {
    match v {
        Some(v) => ((v.max_health + 0.5) as i32 / 2).min(30),
        None => 0,
    }
}

/// The jump bar's inputs (`JumpableVehicleBar`), present only while
/// `player.jumpableVehicle()` is non-null.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JumpInput {
    /// `player.getJumpRidingScale()`.
    pub scale: f32,
    /// `vehicle.getJumpCooldown()` — a camel's or nautilus's dash cooldown;
    /// a horse's is the interface default, 0.
    pub cooldown: i32,
}

/// Everything `extractPlayerHealth`, `extractVehicleHealth`,
/// `extractEffects` and the jump bar read, as plain values.
#[derive(Clone, Debug, PartialEq)]
pub struct SurvivalInputs {
    /// `gameMode.canHurtPlayer()` — gates armour, hearts, food and air
    /// (`Hud.java:541`). The vehicle hearts and the effects are NOT gated.
    pub can_hurt: bool,
    /// `player.getHealth()`, un-ceiled.
    pub health: f32,
    /// `player.getAttributeValue(Attributes.MAX_HEALTH)`.
    pub max_health_attr: f32,
    /// `player.getAbsorptionAmount()`, un-ceiled.
    pub absorption: f32,
    /// `Hud.displayHealth` after this frame's update — `oldHealth`.
    pub display_health: i32,
    pub blink: bool,
    /// `HeartType.forPlayer(player)`.
    pub heart_type: HeartKind,
    pub hardcore: bool,
    /// `player.hasEffect(MobEffects.REGENERATION)`.
    pub regeneration: bool,
    /// `player.getArmorValue()` — `Mth.floor` of the ARMOR attribute.
    pub armor: i32,
    /// `FoodData.getFoodLevel()`.
    pub food: i32,
    /// `FoodData.getSaturationLevel()`.
    pub saturation: f32,
    /// `player.hasEffect(MobEffects.HUNGER)`.
    pub hunger_effect: bool,
    /// `player.getAirSupply()`, RAW (negative while drowning).
    pub air_supply: i32,
    /// `player.getMaxAirSupply()`.
    pub max_air: i32,
    /// `player.isEyeInFluid(FluidTags.WATER)`.
    pub eye_in_water: bool,
    /// `getPlayerVehicleWithHealth()` — `None` unless the DIRECT vehicle is a
    /// `LivingEntity`.
    pub vehicle: Option<VehicleInput>,
    /// `player.getActiveEffects()`, unsorted.
    pub effects: Vec<EffectInput>,
    /// `Hud.tickCount`.
    pub tick_count: i32,
    /// The jump bar, when it is the contextual bar this frame.
    pub jump: Option<JumpInput>,
}

impl Default for SurvivalInputs {
    /// A full-health survival player on foot with nothing else going on —
    /// what the older gates' `set_hud(20.0, 20, ..)` meant.
    fn default() -> Self {
        Self {
            can_hurt: true,
            health: 20.0,
            max_health_attr: 20.0,
            absorption: 0.0,
            display_health: 20,
            blink: false,
            heart_type: HeartKind::Normal,
            hardcore: false,
            regeneration: false,
            armor: 0,
            food: 20,
            saturation: 5.0,
            hunger_effect: false,
            air_supply: MAX_AIR_SUPPLY,
            max_air: MAX_AIR_SUPPLY,
            eye_in_water: false,
            vehicle: None,
            effects: Vec::new(),
            tick_count: 0,
            jump: None,
        }
    }
}

impl SurvivalInputs {
    /// What the pre-M168 `set_hud(health, food, ..)` meant: a survival player
    /// with that health and food and nothing else — the gates' fixture.
    pub fn simple(health: f32, food: i32) -> Self {
        Self {
            health,
            display_health: ceil_f(health),
            food,
            ..Self::default()
        }
    }
}

/// [`layout`] for a screen of `w` x `h` pixels, deriving the GUI size the
/// way [`crate::hud::HudPass::draw`] does — `gui_scale`, then truncate.
pub fn layout_for_screen(inp: &SurvivalInputs, w: f32, h: f32) -> Vec<HudBlit> {
    let s = crate::hud::gui_scale(w, h);
    layout(inp, (w / s) as i32, (h / s) as i32)
}

/// `Mth.ceil(float)` — `(int)Math.ceil(v)`.
fn ceil_f(v: f32) -> i32 {
    v.ceil() as i32
}

/// `Mth.ceil(double)`.
fn ceil_d(v: f64) -> i32 {
    v.ceil() as i32
}

fn blit(x: i32, y: i32, w: i32, h: i32, icon: HudIcon) -> HudBlit {
    HudBlit {
        x: x as f32,
        y: y as f32,
        w: w as f32,
        h: h as f32,
        alpha: 1.0,
        icon,
    }
}

/// The whole of `extractHotbarAndDecorations`' gauges plus `extractEffects`,
/// for a GUI-scaled screen of `gui_w` × `gui_h`, in draw order.
pub fn layout(inp: &SurvivalInputs, gui_w: i32, gui_h: i32) -> Vec<HudBlit> {
    let mut out = Vec::with_capacity(96);
    if inp.can_hurt {
        player_health(inp, gui_w, gui_h, &mut out);
    }
    vehicle_health(inp, gui_w, gui_h, &mut out);
    if let Some(j) = inp.jump {
        jump_bar(j, gui_w, gui_h, &mut out);
    }
    effects(inp, gui_w, &mut out);
    out
}

/// `extractPlayerHealth` (`Hud.java:762-812`): armour, hearts, food (unless
/// a vehicle has hearts), air.
fn player_health(inp: &SurvivalInputs, gui_w: i32, gui_h: i32, out: &mut Vec<HudBlit>) {
    let current_health = ceil_f(inp.health);
    let old_health = inp.display_health;
    let mut rng = HudRandom::for_tick(inp.tick_count);
    let x_left = gui_w / 2 - 91;
    let x_right = gui_w / 2 + 91;
    let y_line_base = gui_h - 39;
    let max_health = inp.max_health_attr.max(old_health.max(current_health) as f32);
    let total_absorption = ceil_f(inp.absorption);
    let num_health_rows = ceil_f((max_health + total_absorption as f32) / 2.0 / 10.0);
    let health_row_height = (10 - (num_health_rows - 2)).max(3);
    let mut y_line_air = y_line_base - 10;
    let heart_offset_index = if inp.regeneration {
        inp.tick_count % ceil_f(max_health + 5.0)
    } else {
        -1
    };

    armor(inp.armor, y_line_base, num_health_rows, health_row_height, x_left, out);
    hearts(
        inp,
        &mut rng,
        x_left,
        y_line_base,
        health_row_height,
        heart_offset_index,
        max_health,
        current_health,
        old_health,
        total_absorption,
        out,
    );
    let vehicle_hearts = vehicle_max_hearts(inp.vehicle);
    if vehicle_hearts == 0 {
        food(inp, &mut rng, y_line_base, x_right, out);
        y_line_air -= 10;
    }
    air_bubbles(inp, &mut rng, vehicle_hearts, y_line_air, x_right, out);
}

/// `extractArmor` (`Hud.java:815-836`).
fn armor(
    armor: i32,
    y_line_base: i32,
    num_health_rows: i32,
    health_row_height: i32,
    x_left: i32,
    out: &mut Vec<HudBlit>,
) {
    if armor <= 0 {
        return;
    }
    let y = y_line_base - (num_health_rows - 1) * health_row_height - 10;
    for i in 0..10 {
        let xo = x_left + i * 8;
        let sprite = match (i * 2 + 1).cmp(&armor) {
            std::cmp::Ordering::Less => ArmorSprite::Full,
            std::cmp::Ordering::Equal => ArmorSprite::Half,
            std::cmp::Ordering::Greater => ArmorSprite::Empty,
        };
        out.push(blit(xo, y, 9, 9, HudIcon::Armor(sprite)));
    }
}

/// `extractHearts` (`Hud.java:839-892`), containers last-to-first.
#[allow(clippy::too_many_arguments)]
fn hearts(
    inp: &SurvivalInputs,
    rng: &mut HudRandom,
    x_left: i32,
    y_line_base: i32,
    health_row_height: i32,
    heart_offset_index: i32,
    max_health: f32,
    current_health: i32,
    old_health: i32,
    absorption: i32,
    out: &mut Vec<HudBlit>,
) {
    let kind = inp.heart_type;
    let hardcore = inp.hardcore;
    let blink = inp.blink;
    let health_container_count = ceil_d(f64::from(max_health) / 2.0);
    let absorption_container_count = ceil_d(f64::from(absorption) / 2.0);
    let max_health_halves_count = health_container_count * 2;
    let heart = |k: HeartKind, xo: i32, yo: i32, blinks: bool, half: bool, out: &mut Vec<HudBlit>| {
        out.push(blit(
            xo,
            yo,
            9,
            9,
            HudIcon::PlayerHeart {
                kind: k,
                hardcore,
                half,
                blink: blinks,
            },
        ));
    };
    for container_index in (0..health_container_count + absorption_container_count).rev() {
        let row = container_index / 10;
        let column = container_index % 10;
        let xo = x_left + column * 8;
        let mut yo = y_line_base - row * health_row_height;
        if current_health + absorption <= 4 {
            yo += rng.next_int(2);
        }
        if container_index < health_container_count && container_index == heart_offset_index {
            yo -= 2;
        }
        heart(HeartKind::Container, xo, yo, blink, false, out);
        let halves = container_index * 2;
        let is_absorption_heart = container_index >= health_container_count;
        if is_absorption_heart {
            let absorption_halves = halves - max_health_halves_count;
            if absorption_halves < absorption {
                let half_heart = absorption_halves + 1 == absorption;
                let k = if kind == HeartKind::Withered {
                    kind
                } else {
                    HeartKind::Absorbing
                };
                heart(k, xo, yo, false, half_heart, out);
            }
        }
        if blink && halves < old_health {
            let half_heart = halves + 1 == old_health;
            heart(kind, xo, yo, true, half_heart, out);
        }
        if halves < current_health {
            let half_heart = halves + 1 == current_health;
            heart(kind, xo, yo, false, half_heart, out);
        }
    }
}

/// `extractFood` (`Hud.java:958-991`) — right to left.
fn food(inp: &SurvivalInputs, rng: &mut HudRandom, y_line_base: i32, x_right: i32, out: &mut Vec<HudBlit>) {
    let food = inp.food;
    let (empty, half, full) = if inp.hunger_effect {
        (FoodSprite::EmptyHunger, FoodSprite::HalfHunger, FoodSprite::FullHunger)
    } else {
        (FoodSprite::Empty, FoodSprite::Half, FoodSprite::Full)
    };
    for i in 0..10 {
        let mut yo = y_line_base;
        if inp.saturation <= 0.0 && inp.tick_count % (food * 3 + 1) == 0 {
            yo += rng.next_int(3) - 1;
        }
        let xo = x_right - i * 8 - 9;
        out.push(blit(xo, yo, 9, 9, HudIcon::Food(empty)));
        if i * 2 + 1 < food {
            out.push(blit(xo, yo, 9, 9, HudIcon::Food(full)));
        }
        if i * 2 + 1 == food {
            out.push(blit(xo, yo, 9, 9, HudIcon::Food(half)));
        }
    }
}

/// `getCurrentAirSupplyBubble` — `Mth.ceil((float)((cur + off) * 10) / max)`.
pub fn air_bubble_count(current: i32, max: i32, tick_offset: i32) -> i32 {
    ceil_f(((current + tick_offset) * 10) as f32 / max as f32)
}

/// `getEmptyBubbleDelayDuration`.
fn empty_bubble_delay(current: i32, under_water: bool) -> i32 {
    if current != 0 && under_water {
        1
    } else {
        0
    }
}

/// `getAirBubbleYLine`: `yLineAir - (ceil(vehicleHearts / 10.0) - 1) * 10`.
pub fn air_bubble_y_line(vehicle_hearts: i32, y_line_air: i32) -> i32 {
    let row_offset = ceil_d(f64::from(vehicle_hearts) / 10.0) - 1;
    y_line_air - row_offset * 10
}

/// `extractAirBubbles` (`Hud.java:906-934`). The bubble-pop sound it also
/// plays is not emitted here — a layout has no sound channel — and is
/// recorded as open.
fn air_bubbles(
    inp: &SurvivalInputs,
    rng: &mut HudRandom,
    vehicle_hearts: i32,
    y_line_air: i32,
    x_right: i32,
    out: &mut Vec<HudBlit>,
) {
    let max = inp.max_air;
    let current = inp.air_supply.clamp(0, max);
    let under_water = inp.eye_in_water;
    if !(under_water || current < max) {
        return;
    }
    let y = air_bubble_y_line(vehicle_hearts, y_line_air);
    let full = air_bubble_count(current, max, -2);
    let popping_pos = air_bubble_count(current, max, 0);
    let empty = 10 - air_bubble_count(current, max, empty_bubble_delay(current, under_water));
    let is_popping = full != popping_pos;
    for bubble in 1..=10 {
        let x = x_right - (bubble - 1) * 8 - 9;
        if bubble <= full {
            out.push(blit(x, y, 9, 9, HudIcon::Air(AirSprite::Full)));
        } else if is_popping && bubble == popping_pos && under_water {
            out.push(blit(x, y, 9, 9, HudIcon::Air(AirSprite::Bursting)));
        } else if bubble > 10 - empty {
            let wobble = if empty == 10 && inp.tick_count % 2 == 0 {
                rng.next_int(2)
            } else {
                0
            };
            out.push(blit(x, y + wobble, 9, 9, HudIcon::Air(AirSprite::Empty)));
        }
    }
}

/// `extractVehicleHealth` (`Hud.java:993-1020`): rows of ten from the
/// bottom, each `baseHealth += 20`.
fn vehicle_health(inp: &SurvivalInputs, gui_w: i32, gui_h: i32, out: &mut Vec<HudBlit>) {
    let Some(v) = inp.vehicle else {
        return;
    };
    let mut hearts = vehicle_max_hearts(Some(v));
    if hearts == 0 {
        return;
    }
    let current = ceil_d(f64::from(v.health));
    let x_right = gui_w / 2 + 91;
    let mut yo = gui_h - 39;
    let mut base_health = 0;
    while hearts > 0 {
        let row_hearts = hearts.min(10);
        hearts -= row_hearts;
        for i in 0..row_hearts {
            let xo = x_right - i * 8 - 9;
            out.push(blit(xo, yo, 9, 9, HudIcon::VehicleHeart(VehicleHeartSprite::Container)));
            if i * 2 + 1 + base_health < current {
                out.push(blit(xo, yo, 9, 9, HudIcon::VehicleHeart(VehicleHeartSprite::Full)));
            }
            if i * 2 + 1 + base_health == current {
                out.push(blit(xo, yo, 9, 9, HudIcon::VehicleHeart(VehicleHeartSprite::Half)));
            }
        }
        yo -= 10;
        base_health += 20;
    }
}

/// `ContextualBar.left/top` — the XP bar's slot.
pub fn contextual_bar_pos(gui_w: i32, gui_h: i32) -> (i32, i32) {
    ((gui_w - 182) / 2, gui_h - 24 - 5)
}

/// `Mth.lerpDiscrete(alpha, 0, 182)`.
pub fn jump_progress_px(scale: f32) -> i32 {
    const P0: i32 = 0;
    const P1: i32 = 182;
    let delta = P1 - P0;
    P0 + (scale * (delta - 1) as f32).floor() as i32 + i32::from(scale > 0.0)
}

/// `JumpableVehicleBar.extractBackground`.
fn jump_bar(j: JumpInput, gui_w: i32, gui_h: i32, out: &mut Vec<HudBlit>) {
    let (left, top) = contextual_bar_pos(gui_w, gui_h);
    out.push(blit(left, top, 182, 5, HudIcon::JumpBar(JumpBarSprite::Background)));
    if j.cooldown > 0 {
        out.push(blit(left, top, 182, 5, HudIcon::JumpBar(JumpBarSprite::Cooldown)));
    } else {
        let progress = jump_progress_px(j.scale);
        if progress > 0 {
            out.push(blit(left, top, progress, 5, HudIcon::JumpBar(JumpBarSprite::Progress)));
        }
    }
}

/// `extractEffects` (`Hud.java:486-526`), minus the demo shift and the
/// open-screen gate, which are the caller's.
fn effects(inp: &SurvivalInputs, gui_w: i32, out: &mut Vec<HudBlit>) {
    if inp.effects.is_empty() {
        return;
    }
    let mut sorted: Vec<EffectInput> = inp.effects.clone();
    // `Ordering.natural().reverse().sortedCopy(..)` — a stable sort under the
    // reversed comparator.
    sorted.sort_by(|a, b| effect_compare(b, a));
    let mut beneficial_count = 0;
    let mut harmful_count = 0;
    for e in &sorted {
        if !e.show_icon {
            continue;
        }
        let mut x = gui_w;
        let mut y = 1;
        if e.beneficial {
            beneficial_count += 1;
            x -= 25 * beneficial_count;
        } else {
            harmful_count += 1;
            x -= 25 * harmful_count;
            y += 26;
        }
        out.push(blit(x, y, 24, 24, HudIcon::EffectBackground { ambient: e.ambient }));
        let mut icon = blit(x + 3, y + 3, 18, 18, HudIcon::Effect(e.id));
        icon.alpha = effect_icon_alpha(e);
        out.push(icon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GW: i32 = 320;
    const GH: i32 = 240;

    fn icons(v: &[HudBlit], f: impl Fn(&HudIcon) -> bool) -> Vec<HudBlit> {
        v.iter().copied().filter(|b| f(&b.icon)).collect()
    }

    fn is_heart(i: &HudIcon) -> bool {
        matches!(i, HudIcon::PlayerHeart { .. })
    }

    /// `java.util.Random`'s definition, worked by hand for one seed: the
    /// scramble, then `next(31)` twice. Pins the LCG against the algorithm
    /// rather than against one of this workspace's other copies.
    #[test]
    fn the_lcg_is_java_util_random() {
        // seed 0 -> scrambled 0x5DEECE66D; next: 0x5DEECE66D * 0x5DEECE66D + 0xB
        // masked to 48 bits = 0xBB20B4F866 -> >> 17 = 1569741360... computed
        // below by the same arithmetic in i128 so the test is not circular
        // with `next` itself.
        let mut r = HudRandom::with_seed(0);
        let mut s: i128 = 0x5DEECE66D;
        let mut expect = |bits: u32| {
            s = (s * 0x5DEECE66D + 0xB) & ((1i128 << 48) - 1);
            (s >> (48 - bits)) as i32
        };
        let e0 = expect(31);
        let e1 = expect(31);
        assert_eq!(r.next(31), e0);
        assert_eq!(r.next(31), e1);
        // Known `new Random(42).nextInt()` (32 bits) is -1170105035.
        let mut r = HudRandom::with_seed(42);
        assert_eq!(r.next(32), -1170105035);
        // nextInt(2) takes the TOP bit of one 31-bit draw; nextInt(3) rejects.
        let mut a = HudRandom::with_seed(7);
        let mut b = HudRandom::with_seed(7);
        assert_eq!(a.next_int(2), (b.next(31) >> 30) & 1);
        let mut c = HudRandom::with_seed(7);
        for _ in 0..50 {
            let v = c.next_int(3);
            assert!((0..3).contains(&v));
        }
    }

    /// `tickCount * 312871` wraps as an `int` before it widens.
    #[test]
    fn the_per_frame_seed_wraps_as_an_int() {
        let t = 7000; // 7000 * 312871 = 2_190_097_000 > i32::MAX
        let wrapped = 7000i32.wrapping_mul(312871);
        assert!(wrapped < 0, "must have wrapped for the test to mean anything");
        assert_eq!(HudRandom::for_tick(t), HudRandom::with_seed(i64::from(wrapped)));
        assert_ne!(HudRandom::for_tick(t), HudRandom::with_seed(7000i64 * 312871));
    }

    #[test]
    fn the_heart_sprite_index_is_get_sprites_table() {
        // full, full_blinking, half, half_blinking, hardcore_full, ...
        assert_eq!(heart_sprite_index(HeartKind::Normal, false, false, false), 8);
        assert_eq!(heart_sprite_index(HeartKind::Normal, false, false, true), 9);
        assert_eq!(heart_sprite_index(HeartKind::Normal, false, true, false), 10);
        assert_eq!(heart_sprite_index(HeartKind::Normal, true, false, false), 12);
        assert_eq!(heart_sprite_index(HeartKind::Normal, true, true, true), 15);
        assert_eq!(heart_sprite_index(HeartKind::Frozen, true, true, true), 47);
        assert_eq!(
            HeartKind::for_player(true, true, true),
            HeartKind::Poisoned,
            "POISON first"
        );
        assert_eq!(HeartKind::for_player(false, true, true), HeartKind::Withered);
        assert_eq!(HeartKind::for_player(false, false, true), HeartKind::Frozen);
    }

    /// Full health, full food: ten containers, ten full hearts, ten empty
    /// drumsticks under ten full ones, at vanilla's coordinates.
    #[test]
    fn full_health_is_one_row_at_the_vanilla_coordinates() {
        let v = layout(&SurvivalInputs::default(), GW, GH);
        let hearts = icons(&v, is_heart);
        assert_eq!(hearts.len(), 20);
        let x_left = GW / 2 - 91;
        let y = GH - 39;
        // Containers run from the LAST index to the first.
        assert_eq!((hearts[0].x, hearts[0].y), ((x_left + 9 * 8) as f32, y as f32));
        assert_eq!(
            hearts[0].icon,
            HudIcon::PlayerHeart { kind: HeartKind::Container, hardcore: false, half: false, blink: false }
        );
        assert_eq!(
            hearts[1].icon,
            HudIcon::PlayerHeart { kind: HeartKind::Normal, hardcore: false, half: false, blink: false }
        );
        assert_eq!(hearts[18].x, x_left as f32);
        let food = icons(&v, |i| matches!(i, HudIcon::Food(_)));
        assert_eq!(food.len(), 20);
        // Right to left: the FIRST drumstick is the rightmost cell.
        let x_right = GW / 2 + 91;
        assert_eq!(food[0].x, (x_right - 9) as f32);
        assert_eq!(food[1].icon, HudIcon::Food(FoodSprite::Full));
        assert_eq!(food[18].x, (x_right - 9 * 8 - 9) as f32);
        // Nothing else.
        assert!(icons(&v, |i| matches!(i, HudIcon::Armor(_) | HudIcon::Air(_) | HudIcon::VehicleHeart(_) | HudIcon::Effect(_) | HudIcon::EffectBackground { .. } | HudIcon::JumpBar(_))).is_empty());
    }

    /// 0.3 health is a half heart: `Mth.ceil`, where M3 rounded to nothing.
    #[test]
    fn health_is_ceiled_not_rounded() {
        let inp = SurvivalInputs { health: 0.3, display_health: 1, ..Default::default() };
        let v = layout(&inp, GW, GH);
        let fills: Vec<_> = icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Normal, .. }));
        assert_eq!(fills.len(), 1);
        assert_eq!(
            fills[0].icon,
            HudIcon::PlayerHeart { kind: HeartKind::Normal, hardcore: false, half: true, blink: false }
        );
    }

    /// Three rows compress to 9 px, and the armour row rides on the top one.
    #[test]
    fn rows_compress_and_the_armour_row_rides_on_top() {
        let inp = SurvivalInputs {
            health: 60.0,
            max_health_attr: 60.0,
            display_health: 60,
            armor: 5,
            ..Default::default()
        };
        let v = layout(&inp, GW, GH);
        let containers = icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Container, .. }));
        assert_eq!(containers.len(), 30);
        let y = GH - 39;
        // rows = ceil(60/2/10) = 3; rowHeight = max(10 - 1, 3) = 9.
        let ys: std::collections::BTreeSet<i32> = containers.iter().map(|b| b.y as i32).collect();
        assert_eq!(ys.into_iter().collect::<Vec<_>>(), vec![y - 18, y - 9, y]);
        let armor = icons(&v, |i| matches!(i, HudIcon::Armor(_)));
        assert_eq!(armor.len(), 10);
        assert_eq!(armor[0].y as i32, y - 2 * 9 - 10);
        assert_eq!(armor[0].x as i32, GW / 2 - 91);
        // armor 5: i*2+1 < 5 for i in 0,1 -> full; == for i=2 -> half; rest empty.
        let kinds: Vec<_> = armor.iter().map(|b| b.icon).collect();
        assert_eq!(kinds[0], HudIcon::Armor(ArmorSprite::Full));
        assert_eq!(kinds[1], HudIcon::Armor(ArmorSprite::Full));
        assert_eq!(kinds[2], HudIcon::Armor(ArmorSprite::Half));
        assert_eq!(kinds[3], HudIcon::Armor(ArmorSprite::Empty));
        // Armour 0 draws NOTHING, not ten empties.
        let v0 = layout(&SurvivalInputs::default(), GW, GH);
        assert!(icons(&v0, |i| matches!(i, HudIcon::Armor(_))).is_empty());
    }

    /// Absorption adds containers past the health ones, drawn ABSORBING —
    /// or WITHERED when withered — and never as the player's own type.
    #[test]
    fn absorption_hearts_are_absorbing_unless_withered() {
        let base = SurvivalInputs { absorption: 3.0, ..Default::default() };
        let v = layout(&base, GW, GH);
        let containers = icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Container, .. }));
        assert_eq!(containers.len(), 12, "10 health + ceil(3/2) = 2 absorption");
        let abs = icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Absorbing, .. }));
        assert_eq!(abs.len(), 2);
        // The outer container (index 11) is halves 22 -> absHalves 2 -> 2 < 3 and 3 == 3 -> half.
        assert!(matches!(abs[0].icon, HudIcon::PlayerHeart { half: true, .. }));
        assert!(matches!(abs[1].icon, HudIcon::PlayerHeart { half: false, .. }));
        // Second row: containers 10 and 11 sit one row up at rowHeight 10.
        assert_eq!(abs[0].y as i32, GH - 39 - 10);
        let withered = SurvivalInputs { heart_type: HeartKind::Withered, ..base.clone() };
        let v = layout(&withered, GW, GH);
        assert!(icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Absorbing, .. })).is_empty());
        assert_eq!(icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Withered, .. })).len(), 12);
        let poisoned = SurvivalInputs { heart_type: HeartKind::Poisoned, ..base };
        let v = layout(&poisoned, GW, GH);
        assert_eq!(icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Absorbing, .. })).len(), 2, "poison does not recolour absorption");
    }

    /// Blink: the container blinks, the OLD health is drawn blinking under
    /// the current health, and the hardcore flag selects the other half of
    /// the table.
    #[test]
    fn blink_draws_the_old_health_ghosted_and_hardcore_swaps_the_table() {
        let inp = SurvivalInputs {
            health: 14.0,
            display_health: 20,
            blink: true,
            hardcore: true,
            ..Default::default()
        };
        let v = layout(&inp, GW, GH);
        let hearts = icons(&v, is_heart);
        // Per container: container(blink) + [old blink fill] + [current fill].
        // Container 9 (halves 18): 18 < 20 -> old blink fill; 18 < 14 false.
        assert_eq!(hearts[0].icon, HudIcon::PlayerHeart { kind: HeartKind::Container, hardcore: true, half: false, blink: true });
        assert_eq!(hearts[1].icon, HudIcon::PlayerHeart { kind: HeartKind::Normal, hardcore: true, half: false, blink: true });
        // Container 6 (halves 12): old 12 < 20 blink fill, then 12 < 14 with 13 == 14 false -> full.
        let c6: Vec<_> = hearts.iter().filter(|b| b.x as i32 == GW / 2 - 91 + 6 * 8).collect();
        assert_eq!(c6.len(), 3);
        assert_eq!(c6[2].icon, HudIcon::PlayerHeart { kind: HeartKind::Normal, hardcore: true, half: false, blink: false });
        let total: usize = hearts.len();
        assert_eq!(total, 10 + 10 + 7);
    }

    /// The regeneration wave lifts one container by two, the one whose index
    /// is `tickCount % ceil(maxHealth + 5)`, and only among HEALTH hearts.
    #[test]
    fn the_regeneration_wave_lifts_one_heart() {
        let inp = SurvivalInputs { regeneration: true, tick_count: 3, ..Default::default() };
        let v = layout(&inp, GW, GH);
        let containers = icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Container, .. }));
        let lifted: Vec<_> = containers.iter().filter(|b| b.y as i32 == GH - 39 - 2).collect();
        assert_eq!(lifted.len(), 1);
        assert_eq!(lifted[0].x as i32, GW / 2 - 91 + 3 * 8);
        // tick 12 % 25 = 12 -> no container has that index: nothing lifts.
        let v = layout(&SurvivalInputs { tick_count: 12, ..inp }, GW, GH);
        assert!(icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Container, .. })).iter().all(|b| b.y as i32 == GH - 39));
    }

    /// Low health jitters every container by `nextInt(2)` off the seeded
    /// LCG, consumed in container order (last to first) — so the offsets are
    /// a pure function of the tick and two frames with the same tick agree.
    #[test]
    fn low_health_jitter_is_the_seeded_lcg_in_draw_order() {
        let inp = SurvivalInputs { health: 4.0, display_health: 4, tick_count: 77, ..Default::default() };
        let v = layout(&inp, GW, GH);
        let containers = icons(&v, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Container, .. }));
        let mut rng = HudRandom::for_tick(77);
        for c in &containers {
            assert_eq!(c.y as i32, GH - 39 + rng.next_int(2));
        }
        assert_eq!(layout(&inp, GW, GH), v, "deterministic per tick");
        // 5 health: no jitter at all.
        let v5 = layout(&SurvivalInputs { health: 5.0, display_health: 5, tick_count: 77, ..Default::default() }, GW, GH);
        assert!(icons(&v5, |i| matches!(i, HudIcon::PlayerHeart { kind: HeartKind::Container, .. })).iter().all(|b| b.y as i32 == GH - 39));
    }

    /// Food: the hunger variant, the half, and the saturation wobble on the
    /// ticks `tickCount % (food * 3 + 1) == 0`, drawing `nextInt(3) - 1`
    /// AFTER the hearts' draws.
    #[test]
    fn food_swaps_sprites_under_hunger_and_wobbles_at_zero_saturation() {
        let inp = SurvivalInputs { food: 7, hunger_effect: true, ..Default::default() };
        let v = layout(&inp, GW, GH);
        let food = icons(&v, |i| matches!(i, HudIcon::Food(_)));
        // 10 empties + 3 full (i=0,1,2: 1,3,5 < 7) + 1 half (i=3: 7 == 7).
        assert_eq!(food.len(), 14);
        assert!(food.iter().all(|b| matches!(b.icon, HudIcon::Food(FoodSprite::EmptyHunger | FoodSprite::FullHunger | FoodSprite::HalfHunger))));
        assert_eq!(food.iter().filter(|b| b.icon == HudIcon::Food(FoodSprite::HalfHunger)).count(), 1);
        // Wobble: food 20 -> period 61; tick 61 wobbles, tick 62 does not.
        let wob = SurvivalInputs { saturation: 0.0, tick_count: 61, ..Default::default() };
        let v = layout(&wob, GW, GH);
        let food = icons(&v, |i| matches!(i, HudIcon::Food(_)));
        let mut rng = HudRandom::for_tick(61); // hearts at 20 hp draw nothing first
        for pair in food.chunks(2) {
            let dy = rng.next_int(3) - 1;
            assert!(pair.iter().all(|b| b.y as i32 == GH - 39 + dy));
        }
        let v = layout(&SurvivalInputs { tick_count: 62, ..wob }, GW, GH);
        assert!(icons(&v, |i| matches!(i, HudIcon::Food(_))).iter().all(|b| b.y as i32 == GH - 39));
    }

    /// Air: hidden when full and dry; shown while refilling out of water;
    /// three `ceil`s; the popping bubble only underwater; the empty wobble
    /// only when all ten are empty on an even tick.
    #[test]
    fn air_bubbles_count_the_way_the_three_ceils_do() {
        let dry_full = layout(&SurvivalInputs::default(), GW, GH);
        assert!(icons(&dry_full, |i| matches!(i, HudIcon::Air(_))).is_empty());
        // 150 of 300, underwater: full = ceil(148*10/300)=ceil(4.93)=5,
        // popping = ceil(5.0)=5 -> not popping; empty = 10 - ceil(151*10/300)=10-6=4.
        let inp = SurvivalInputs { air_supply: 150, eye_in_water: true, ..Default::default() };
        let v = layout(&inp, GW, GH);
        let air = icons(&v, |i| matches!(i, HudIcon::Air(_)));
        assert_eq!(air.iter().filter(|b| b.icon == HudIcon::Air(AirSprite::Full)).count(), 5);
        assert_eq!(air.iter().filter(|b| b.icon == HudIcon::Air(AirSprite::Bursting)).count(), 0);
        assert_eq!(air.iter().filter(|b| b.icon == HudIcon::Air(AirSprite::Empty)).count(), 4);
        assert_eq!(air.len(), 9, "bubble 6 is neither full, popping nor empty: a gap");
        // y: food drawn (no vehicle) -> yLineAir = base - 20, then
        // getAirBubbleYLine with rows-1 = -1 -> +10 -> base - 10.
        assert!(air.iter().all(|b| b.y as i32 == GH - 39 - 10));
        assert_eq!(air[0].x as i32, GW / 2 + 91 - 9, "bubble 1 is the rightmost");
        // 149: full = ceil(147*10/300)=ceil(4.9)=5; popping = ceil(4.967)=5 -> no.
        // 141: full = ceil(139/30)=ceil(4.63)=5; popping = ceil(4.7)=5.
        // 120: full = ceil(118/30)=ceil(3.93)=4; popping = ceil(4.0)=4 -> no.
        // 121: full = ceil(3.967)=4; popping = ceil(4.03)=5 -> POPPING at bubble 5.
        let v = layout(&SurvivalInputs { air_supply: 121, ..inp.clone() }, GW, GH);
        let air = icons(&v, |i| matches!(i, HudIcon::Air(_)));
        let pop: Vec<_> = air.iter().filter(|b| b.icon == HudIcon::Air(AirSprite::Bursting)).collect();
        assert_eq!(pop.len(), 1);
        assert_eq!(pop[0].x as i32, GW / 2 + 91 - 4 * 8 - 9);
        // The same air OUT of the water: no popping sprite, and the bubbles
        // still show because current < max.
        let v = layout(&SurvivalInputs { air_supply: 121, eye_in_water: false, ..inp.clone() }, GW, GH);
        let air = icons(&v, |i| matches!(i, HudIcon::Air(_)));
        assert!(!air.is_empty());
        assert!(air.iter().all(|b| b.icon != HudIcon::Air(AirSprite::Bursting)));
        // Drowning (negative) clamps to 0: ten empties, wobbling on even ticks.
        let v = layout(&SurvivalInputs { air_supply: -15, tick_count: 4, ..inp.clone() }, GW, GH);
        let air = icons(&v, |i| matches!(i, HudIcon::Air(_)));
        assert_eq!(air.len(), 10);
        assert!(air.iter().all(|b| b.icon == HudIcon::Air(AirSprite::Empty)));
        let mut rng = HudRandom::for_tick(4);
        // 20 hp: no heart draws; full food + saturation 5: no food draws.
        for b in &air {
            assert_eq!(b.y as i32, GH - 39 - 10 + rng.next_int(2));
        }
        let v = layout(&SurvivalInputs { air_supply: -15, tick_count: 5, ..inp }, GW, GH);
        assert!(icons(&v, |i| matches!(i, HudIcon::Air(_))).iter().all(|b| b.y as i32 == GH - 39 - 10));
    }

    /// A living vehicle replaces the food row with its hearts, capped at 30,
    /// rows of ten from the bottom, and pushes the air line up by its rows.
    #[test]
    fn a_vehicle_replaces_food_and_moves_the_air_line() {
        let horse = VehicleInput { max_health: 30.0, health: 15.0 };
        assert_eq!(vehicle_max_hearts(Some(horse)), 15);
        assert_eq!(vehicle_max_hearts(Some(VehicleInput { max_health: 29.0, health: 1.0 })), 14, "(int)(29.5)/2 = 14");
        assert_eq!(vehicle_max_hearts(Some(VehicleInput { max_health: 200.0, health: 1.0 })), 30, "capped");
        assert_eq!(vehicle_max_hearts(Some(VehicleInput { max_health: 1.0, health: 1.0 })), 0, "(int)(1.5)/2 = 0 -> no bar at all");
        let inp = SurvivalInputs { vehicle: Some(horse), air_supply: 100, ..Default::default() };
        let v = layout(&inp, GW, GH);
        assert!(icons(&v, |i| matches!(i, HudIcon::Food(_))).is_empty(), "food skipped");
        let vh = icons(&v, |i| matches!(i, HudIcon::VehicleHeart(_)));
        // 15 containers (10 + 5), 15 health -> 7 full + 1 half.
        assert_eq!(vh.iter().filter(|b| b.icon == HudIcon::VehicleHeart(VehicleHeartSprite::Container)).count(), 15);
        assert_eq!(vh.iter().filter(|b| b.icon == HudIcon::VehicleHeart(VehicleHeartSprite::Full)).count(), 7);
        assert_eq!(vh.iter().filter(|b| b.icon == HudIcon::VehicleHeart(VehicleHeartSprite::Half)).count(), 1);
        let y = GH - 39;
        let row2: Vec<_> = vh.iter().filter(|b| b.y as i32 == y - 10).collect();
        assert_eq!(row2.len(), 5, "the second row holds the five leftover containers and no fills");
        // Air: yLineAir = base - 10 (no food), rows = 2 -> rowOffset 1 -> base - 20.
        let air = icons(&v, |i| matches!(i, HudIcon::Air(_)));
        assert!(!air.is_empty());
        assert!(air.iter().all(|b| b.y as i32 == y - 20));
    }

    /// `canHurtPlayer()` false (creative/spectator) draws no player gauges but
    /// still draws the vehicle's hearts and the effects.
    #[test]
    fn creative_hides_the_player_gauges_and_not_the_rest() {
        let inp = SurvivalInputs {
            can_hurt: false,
            armor: 10,
            vehicle: Some(VehicleInput { max_health: 20.0, health: 20.0 }),
            effects: vec![EffectInput { id: 0, duration: 100, ambient: false, show_icon: true, beneficial: true, color: 1 }],
            ..Default::default()
        };
        let v = layout(&inp, GW, GH);
        assert!(icons(&v, |i| matches!(i, HudIcon::PlayerHeart { .. } | HudIcon::Armor(_) | HudIcon::Food(_) | HudIcon::Air(_))).is_empty());
        assert_eq!(icons(&v, |i| matches!(i, HudIcon::VehicleHeart(_))).len(), 20);
        assert_eq!(icons(&v, |i| matches!(i, HudIcon::Effect(_))).len(), 1);
    }

    /// Effects: beneficial on the top row, harmful AND neutral on the second,
    /// each 25 px further left; ambient picks the other background; the
    /// icon sits at +3,+3 and carries the alpha; `showIcon` false skips.
    #[test]
    fn effects_lay_out_in_two_rows_by_is_beneficial_only() {
        let e = |id: i32, beneficial: bool, ambient: bool, show: bool, dur: i32| EffectInput {
            id,
            duration: dur,
            ambient,
            show_icon: show,
            beneficial,
            color: id as u32,
        };
        let inp = SurvivalInputs {
            effects: vec![
                e(1, true, false, true, 1000),
                e(2, false, false, true, 1000), // harmful
                e(3, false, true, true, 1000),  // NEUTRAL, ambient -> harmful row
                e(4, true, false, false, 1000), // hidden
                e(5, true, false, true, 2000),  // longer: sorts first among beneficial
            ],
            ..Default::default()
        };
        let v = layout(&inp, GW, GH);
        let bgs = icons(&v, |i| matches!(i, HudIcon::EffectBackground { .. }));
        let ics = icons(&v, |i| matches!(i, HudIcon::Effect(_)));
        assert_eq!((bgs.len(), ics.len()), (4, 4));
        // Ascending `compareTo` (every pair hits the first branch: all
        // durations are below the cut-off and id 3 is the only ambient one):
        // `compareFalseFirst(ambient)` puts the AMBIENT effect last, then
        // infinite (none), then duration, then colour — id 1, 2, 4, 5, 3.
        // `Ordering.natural().reverse()` flips it: 3, 5, 4, 2, 1; `showIcon`
        // drops 4. So the ambient effect is drawn FIRST — a witness written
        // from "ambient goes last" was wrong before the code was.
        let order: Vec<i32> = ics.iter().map(|b| if let HudIcon::Effect(id) = b.icon { id } else { -1 }).collect();
        assert_eq!(order, vec![3, 5, 2, 1]);
        // Positions: id 3 is harmful #1 (neutral shares the row) at
        // (GW - 25, 27); id 5 beneficial #1 at (GW - 25, 1); id 2 harmful #2 at
        // (GW - 50, 27); id 1 beneficial #2 at (GW - 50, 1).
        let pos: Vec<(i32, i32)> = bgs.iter().map(|b| (b.x as i32, b.y as i32)).collect();
        assert_eq!(pos, vec![(GW - 25, 27), (GW - 25, 1), (GW - 50, 27), (GW - 50, 1)]);
        assert_eq!((ics[0].x as i32, ics[0].y as i32, ics[0].w as i32), (GW - 25 + 3, 30, 18));
        assert_eq!(bgs[0].icon, HudIcon::EffectBackground { ambient: true });
        assert_eq!(bgs[1].icon, HudIcon::EffectBackground { ambient: false });
        assert!(ics.iter().all(|b| b.alpha == 1.0));
    }

    /// The fade: 1.0 outside the last 200 ticks and for an infinite effect;
    /// inside it, the transcribed pulse — pinned at two points worked by hand
    /// off `Mth.cos`'s table.
    #[test]
    fn the_effect_fade_is_the_transcribed_pulse() {
        let e = |dur: i32, ambient: bool| EffectInput { id: 0, duration: dur, ambient, show_icon: true, beneficial: true, color: 0 };
        assert_eq!(effect_icon_alpha(&e(201, false)), 1.0);
        assert_eq!(effect_icon_alpha(&e(-1, false)), 1.0, "infinite never ends within 200");
        assert_eq!(effect_icon_alpha(&e(5, true)), 1.0, "ambient never fades");
        // remaining 200: used = 0 -> second term 0; first = clamp(200/50*0.5=2.0, 0, 0.5) = 0.5.
        assert_eq!(effect_icon_alpha(&e(200, false)), 0.5);
        // remaining 0: first 0; used 10 -> cos(0) * 0.25 = 0.25.
        assert!((effect_icon_alpha(&e(0, false)) - 0.25).abs() < 1e-6);
        // remaining 100: first = clamp(1.0, 0, .5) = .5; used = 5;
        // cos(100 * PI / 5) = cos(20 PI) ~ 1 -> + .125 -> .625.
        let a = effect_icon_alpha(&e(100, false));
        assert!((a - 0.625).abs() < 2e-3, "{a}");
        // remaining 95: cos(19 PI) = -1 -> .5 - .125 = .375 (used = 10 - 4 = 6 -> .15 -> .35).
        let a = effect_icon_alpha(&e(95, false));
        assert!((a - 0.35).abs() < 2e-3, "{a}");
    }

    /// The jump bar: `lerpDiscrete`, the cooldown sprite replacing the
    /// progress, and the XP bar's slot.
    #[test]
    fn the_jump_bar_is_lerp_discrete_in_the_contextual_slot() {
        assert_eq!(jump_progress_px(0.0), 0);
        assert_eq!(jump_progress_px(0.001), 1, "a barely-pressed jump is one pixel");
        assert_eq!(jump_progress_px(0.5), 91);
        assert_eq!(jump_progress_px(1.0), 182);
        assert_eq!(contextual_bar_pos(GW, GH), ((GW - 182) / 2, GH - 29));
        let inp = SurvivalInputs { jump: Some(JumpInput { scale: 0.5, cooldown: 0 }), ..Default::default() };
        let v = layout(&inp, GW, GH);
        let jb = icons(&v, |i| matches!(i, HudIcon::JumpBar(_)));
        assert_eq!(jb.len(), 2);
        assert_eq!(jb[0].icon, HudIcon::JumpBar(JumpBarSprite::Background));
        assert_eq!((jb[1].icon, jb[1].w as i32), (HudIcon::JumpBar(JumpBarSprite::Progress), 91));
        let v = layout(&SurvivalInputs { jump: Some(JumpInput { scale: 0.5, cooldown: 3 }), ..Default::default() }, GW, GH);
        let jb = icons(&v, |i| matches!(i, HudIcon::JumpBar(_)));
        assert_eq!(jb[1].icon, HudIcon::JumpBar(JumpBarSprite::Cooldown), "cooldown replaces the progress entirely");
        let v = layout(&SurvivalInputs { jump: Some(JumpInput { scale: 0.0, cooldown: 0 }), ..Default::default() }, GW, GH);
        assert_eq!(icons(&v, |i| matches!(i, HudIcon::JumpBar(_))).len(), 1, "progress 0 draws no progress");
    }

    /// Draw order is vanilla's: armour, hearts, food, air, vehicle, effects.
    #[test]
    fn the_draw_order_is_vanillas() {
        let inp = SurvivalInputs {
            armor: 2,
            air_supply: 10,
            effects: vec![EffectInput { id: 0, duration: 1, ambient: false, show_icon: true, beneficial: true, color: 0 }],
            ..Default::default()
        };
        let v = layout(&inp, GW, GH);
        let rank = |i: &HudIcon| match i {
            HudIcon::Armor(_) => 0,
            HudIcon::PlayerHeart { .. } => 1,
            HudIcon::Food(_) => 2,
            HudIcon::Air(_) => 3,
            HudIcon::VehicleHeart(_) => 4,
            HudIcon::JumpBar(_) => 5,
            HudIcon::EffectBackground { .. } | HudIcon::Effect(_) => 6,
            _ => 99,
        };
        let ranks: Vec<i32> = v.iter().map(|b| rank(&b.icon)).collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
    }
}
