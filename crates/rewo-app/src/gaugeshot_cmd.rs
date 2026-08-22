//! `rewo gaugeshot --check` — the survival HUD oracle (M168).
//!
//! Hearts, armour, food, air, the vehicle's hearts, the effect icons and the
//! jump bar, graded at four levels:
//!
//! ```text
//! literals from Hud.java            -> survival_hud::layout            (transcription)
//! raw metadata / effect / attribute -> the real decoders                (wire)
//! those decoders' state             -> live_cmd::survival_inputs_from  (the derivation
//!                                                                        the frame calls)
//! layout                            -> WorldRenderer::set_hud          (pixels, against
//!                                   -> Offscreen::read_rgba               the jar's bytes)
//! ```
//!
//! **Why the pixel predictions come from the sprite PNGs and the positions
//! from literals.** A witness that asks the implementation where to look
//! grades everything except the thing they share (M93q). Here the position of
//! every probe is re-derived from `Hud.java`'s numbers — `guiWidth / 2 - 91`,
//! `guiHeight - 39`, `xRight - i * 8 - 9` — and the colour it expects is read
//! out of the jar's own PNG at the sprite's own texel, which neither the
//! layout nor the atlas packer can influence. A packer that put `armor_half`
//! where `armor_full` goes, or a layout that put cell 0 at the wrong x, fails
//! the same probe for different reasons.
//!
//! **Four probe colours are degenerate and avoided on purpose**, measured
//! rather than assumed: `air.png`'s centre is transparent (its blue is
//! counted instead), the two effect backgrounds share a centre and differ
//! only on their rim, `armor_half`'s centre is black like the clear, and
//! `vehicle_half` shares its centre column with `vehicle_full`.
//!
//! Serverless; Vulkan required. Fail-closed on a declared witness count.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::DataPaths;
use rewo_gpu::hud::{HudBlit, HudGauges, HudIcon};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::survival_hud::{self as sh, SurvivalInputs};
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::hud_state::HudState;
use rewo_net::play::GameMode;
use rewo_proto::writer::PacketWriter;

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 29;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = 2; asserted by `p0`, not assumed.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

/// The local player's entity id for the wire witnesses — arbitrary.
const PLAYER: i32 = 7;

#[derive(ClapArgs)]
pub struct GaugeshotArgs {
    /// Assert every owned property (the oracle asserts unconditionally; this
    /// labels the run, as the other `*shot` gates do).
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Write the rendered frames here for eyeballing. Never read back.
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[gaugeshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

pub fn run(args: GaugeshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[gaugeshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Layout against Hud.java literals; pixels against \
         the jar's own sprite bytes."
    );
    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let registry = rewo_data::attributes::AttributeRegistry::load(&paths.registries_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_transcription(&mut c);
    check_wire(&mut c, &registry);
    check_derivation(&mut c, &registry);
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[gaugeshot] witnesses observed: {} / {}",
        c.witnessed, EXPECTED_WITNESSES
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "{} propert{} failed: {}",
            c.failures.len(),
            if c.failures.len() == 1 { "y" } else { "ies" },
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "witness count {} != expected {EXPECTED_WITNESSES} — a named \
             property was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[gaugeshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// Literals, re-declared from Hud.java rather than imported from the layout.
// ---------------------------------------------------------------------------

/// `Hud.java:783-785`.
fn ref_x_left(gw: i32) -> i32 {
    gw / 2 - 91
}
fn ref_x_right(gw: i32) -> i32 {
    gw / 2 + 91
}
fn ref_y_base(gh: i32) -> i32 {
    gh - 39
}
/// `Hud.java:789` — `Math.max(10 - (numHealthRows - 2), 3)`.
fn ref_row_height(rows: i32) -> i32 {
    (10 - (rows - 2)).max(3)
}
/// `Hud.java:788` — `Mth.ceil((maxHealth + totalAbsorption) / 2.0F / 10.0F)`.
fn ref_rows(max_health: f32, absorption: i32) -> i32 {
    ((max_health + absorption as f32) / 2.0 / 10.0).ceil() as i32
}
/// `Hud.java:820`.
fn ref_armor_y(gh: i32, rows: i32) -> i32 {
    ref_y_base(gh) - (rows - 1) * ref_row_height(rows) - 10
}
/// `Hud.java:922` — `xRight - (airBubble - 1) * 8 - 9`, bubbles 1-based.
fn ref_bubble_x(gw: i32, bubble: i32) -> i32 {
    ref_x_right(gw) - (bubble - 1) * 8 - 9
}
/// `ContextualBar.left/top`.
fn ref_bar_left(gw: i32) -> i32 {
    (gw - 182) / 2
}
fn ref_bar_top(gh: i32) -> i32 {
    gh - 24 - 5
}
/// `Mth.lerpDiscrete(alpha, 0, 182)` — `Mth.java:545-548`.
fn ref_lerp_discrete(alpha: f32) -> i32 {
    (alpha * 181.0).floor() as i32 + i32::from(alpha > 0.0)
}

fn only(v: &[HudBlit], f: impl Fn(&HudIcon) -> bool) -> Vec<HudBlit> {
    v.iter().copied().filter(|b| f(&b.icon)).collect()
}

fn containers(v: &[HudBlit]) -> Vec<HudBlit> {
    only(v, |i| {
        matches!(
            i,
            HudIcon::PlayerHeart {
                kind: sh::HeartKind::Container,
                ..
            }
        )
    })
}

fn check_transcription(c: &mut Checker) {
    let full = sh::layout(&SurvivalInputs::simple(20.0, 20), GUI_W, GUI_H);
    let conts = containers(&full);
    let foods = only(&full, |i| matches!(i, HudIcon::Food(_)));
    c.record(
        "t0.the_three_anchors_are_the_decompiles",
        conts.len() == 10
            && conts.last().map(|b| (b.x as i32, b.y as i32))
                == Some((ref_x_left(GUI_W), ref_y_base(GUI_H)))
            && conts.first().map(|b| b.x as i32) == Some(ref_x_left(GUI_W) + 9 * 8)
            && foods.first().map(|b| b.x as i32) == Some(ref_x_right(GUI_W) - 9),
        format!(
            "xLeft = guiWidth/2 - 91 = {}, xRight = guiWidth/2 + 91 = {}, yLineBase = \
             guiHeight - 39 = {}; containers run last-to-first and the food's FIRST cell \
             is its rightmost (`xRight - i * 8 - 9`). A left-to-right food row mirrors \
             exactly at ten cells and is wrong at any other count",
            ref_x_left(GUI_W),
            ref_x_right(GUI_W),
            ref_y_base(GUI_H)
        ),
    );

    // t1 — the row ladder. rows 1, 2, 3, 8, 9 -> height 10, 10, 9, 4, 3.
    let mut ladder = Vec::new();
    let mut all_ok = true;
    for max in [20.0f32, 40.0, 60.0, 160.0, 180.0] {
        let rows = ref_rows(max, 0);
        let inp = SurvivalInputs {
            health: max,
            max_health_attr: max,
            display_health: max as i32,
            armor: 1,
            ..Default::default()
        };
        let v = sh::layout(&inp, GUI_W, GUI_H);
        let ys: std::collections::BTreeSet<i32> =
            containers(&v).iter().map(|b| b.y as i32).collect();
        let pitch = if ys.len() >= 2 {
            let v: Vec<i32> = ys.iter().copied().collect();
            v[v.len() - 1] - v[v.len() - 2]
        } else {
            0
        };
        let armor_y = only(&v, |i| matches!(i, HudIcon::Armor(_)))[0].y as i32;
        let ok = ys.len() as i32 == rows
            && (rows == 1 || pitch == ref_row_height(rows))
            && armor_y == ref_armor_y(GUI_H, rows);
        all_ok &= ok;
        ladder.push((rows, pitch, armor_y, ref_row_height(rows), ref_armor_y(GUI_H, rows)));
    }
    c.record(
        "t1.the_rows_compress_and_the_armour_rides_on_top",
        all_ok,
        format!(
            "(rows, pitch, armour y, ref height, ref armour y): {ladder:?} — \
             `max(10 - (rows - 2), 3)`, and `yLineBase - (rows - 1) * rowHeight - 10`"
        ),
    );

    // t2 — ceil, not round.
    let third = sh::layout(
        &SurvivalInputs {
            health: 0.3,
            display_health: 1,
            ..Default::default()
        },
        GUI_W,
        GUI_H,
    );
    let fills = only(&third, |i| {
        matches!(
            i,
            HudIcon::PlayerHeart {
                kind: sh::HeartKind::Normal,
                ..
            }
        )
    });
    c.record(
        "t2.health_is_ceiled_so_a_third_of_a_point_is_half_a_heart",
        fills.len() == 1 && matches!(fills[0].icon, HudIcon::PlayerHeart { half: true, .. }),
        format!(
            "0.3 hp draws {} fill(s): {:?}. `Mth.ceil(player.getHealth())`; M3 rounded and drew none",
            fills.len(),
            fills.first().map(|b| b.icon)
        ),
    );

    // t3 — the regeneration wave: `tickCount % Mth.ceil(maxHealth + 5.0F)`.
    let regen = sh::layout(
        &SurvivalInputs {
            regeneration: true,
            tick_count: 28,
            ..Default::default()
        },
        GUI_W,
        GUI_H,
    );
    let lifted: Vec<i32> = containers(&regen)
        .iter()
        .filter(|b| b.y as i32 == ref_y_base(GUI_H) - 2)
        .map(|b| (b.x as i32 - ref_x_left(GUI_W)) / 8)
        .collect();
    c.record(
        "t3.the_regeneration_wave_lifts_the_tick_mod_max_plus_five_heart",
        lifted == vec![28 % 25],
        format!(
            "tick 28, max 20 -> index 28 % ceil(25) = 3; lifted {lifted:?}. A `% 20` would lift index 8"
        ),
    );

    // t4 — the effect order is the REVERSE of `compareTo`, so ambient first.
    let e = |id: i32, beneficial: bool, ambient: bool, dur: i32| sh::EffectInput {
        id,
        duration: dur,
        ambient,
        show_icon: true,
        beneficial,
        color: id as u32,
    };
    let fx = sh::layout(
        &SurvivalInputs {
            can_hurt: false,
            effects: vec![e(1, true, false, 1000), e(2, true, true, 1000), e(3, true, false, 2000)],
            ..Default::default()
        },
        GUI_W,
        GUI_H,
    );
    let order: Vec<i32> = only(&fx, |i| matches!(i, HudIcon::Effect(_)))
        .iter()
        .map(|b| if let HudIcon::Effect(id) = b.icon { id } else { -1 })
        .collect();
    c.record(
        "t4.effects_sort_ambient_first_then_longest_because_natural_is_reversed",
        order == vec![2, 3, 1],
        format!(
            "{order:?}: `compareFalseFirst(ambient)` puts ambient LAST ascending, \
             `Ordering.natural().reverse()` puts it first; then duration descending"
        ),
    );

    // t5 — three ceils: 121 of 300 underwater pops bubble 5.
    let air = sh::layout(
        &SurvivalInputs {
            air_supply: 121,
            eye_in_water: true,
            ..Default::default()
        },
        GUI_W,
        GUI_H,
    );
    let pops: Vec<i32> = only(&air, |i| matches!(i, HudIcon::Air(sh::AirSprite::Bursting)))
        .iter()
        .map(|b| (ref_x_right(GUI_W) - 9 - b.x as i32) / 8 + 1)
        .collect();
    let fulls = only(&air, |i| matches!(i, HudIcon::Air(sh::AirSprite::Full))).len();
    c.record(
        "t5.the_popping_bubble_is_where_the_minus_two_and_zero_ceils_disagree",
        pops == vec![5] && fulls == 4,
        format!(
            "full = ceil(119 * 10 / 300) = 4, popping = ceil(121 * 10 / 300) = 5 -> bubble 5 \
             bursts; got full {fulls}, bursting at {pops:?}"
        ),
    );

    // t6 — `(int)(maxHealth + 0.5F) / 2`, capped at 30.
    let vmh = |m: f32| {
        sh::vehicle_max_hearts(Some(sh::VehicleInput {
            max_health: m,
            health: 1.0,
        }))
    };
    c.record(
        "t6.vehicle_hearts_cast_before_the_halving_and_cap_at_thirty",
        vmh(29.0) == 14 && vmh(29.5) == 15 && vmh(30.0) == 15 && vmh(200.0) == 30 && vmh(1.0) == 0,
        format!(
            "29 -> {} ((int)29.5 = 29, / 2 = 14), 29.5 -> {} ((int)30.0 = 30, / 2 = 15: the              `+ 0.5F` is what a fractional max health rounds through), 30 -> {}, 200 -> {}              (cap), 1 -> {} (no bar at all)",
            vmh(29.0),
            vmh(29.5),
            vmh(30.0),
            vmh(200.0),
            vmh(1.0)
        ),
    );

    // t7 — lerpDiscrete.
    let ok = [0.0f32, 0.001, 0.5, 1.0]
        .iter()
        .all(|&a| sh::jump_progress_px(a) == ref_lerp_discrete(a));
    c.record(
        "t7.the_jump_progress_is_lerp_discrete",
        ok && sh::jump_progress_px(0.001) == 1 && sh::jump_progress_px(1.0) == 182,
        format!(
            "0 -> {}, 0.001 -> {}, 0.5 -> {}, 1 -> {}: `floor(alpha * 181) + (alpha > 0)`",
            sh::jump_progress_px(0.0),
            sh::jump_progress_px(0.001),
            sh::jump_progress_px(0.5),
            sh::jump_progress_px(1.0)
        ),
    );
}

// ---------------------------------------------------------------------------
// Wire: the real decoders.
// ---------------------------------------------------------------------------

fn meta_body(eid: i32, air: i32, frozen: i32, absorption: f32) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.varint(eid);
    w.u8(1);
    w.varint(1);
    w.varint(air);
    w.u8(7);
    w.varint(1);
    w.varint(frozen);
    w.u8(17);
    w.varint(3);
    w.f32(absorption);
    w.u8(0xFF);
    w.into_bytes()
}

fn effect_body(eid: i32, effect: i32, amp: i32, dur: i32, flags: u8) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.varint(eid);
    w.varint(effect);
    w.varint(amp);
    w.varint(dur);
    w.u8(flags);
    w.into_bytes()
}

fn attrs_body(eid: i32, snaps: &[(i32, f64)]) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.varint(eid);
    w.varint(snaps.len() as i32);
    for (id, base) in snaps {
        w.varint(*id);
        w.f64(*base);
        w.varint(0);
    }
    w.into_bytes()
}

/// A `SurvivalSources` over the gate's own state, with the session's
/// assembler bypassed — that is the point: this is the function the frame
/// calls, driven by hand.
struct Sources {
    attrs: rewo_world::attributes::EntityAttributes,
    effects: rewo_net::effects::VisualEffects,
    local: rewo_net::local_player_data::LocalPlayerData,
    hud: HudState,
}

impl Sources {
    fn new() -> Self {
        let mut effects = rewo_net::effects::VisualEffects::new(None, None);
        effects.set_player_id(PLAYER);
        Self {
            attrs: Default::default(),
            effects,
            local: Default::default(),
            hud: HudState::default(),
        }
    }

    fn inputs(
        &mut self,
        reg: &rewo_data::attributes::AttributeRegistry,
        game_mode: Option<GameMode>,
        health: f32,
        millis: u64,
        vehicle: Option<crate::live_cmd::VehicleSource<'_>>,
    ) -> SurvivalInputs {
        crate::live_cmd::survival_inputs_from(
            crate::live_cmd::SurvivalSources {
                game_mode,
                local_attributes: &self.attrs,
                attribute_registry: Some(reg),
                effects: &self.effects,
                local: self.local,
                underwater: false,
                vehicle,
                health,
                food: 20,
                saturation: 5.0,
                hardcore: false,
                millis,
            },
            &mut self.hud,
        )
    }
}

fn check_wire(c: &mut Checker, reg: &rewo_data::attributes::AttributeRegistry) {
    // w0 — metadata: air (raw, negative), frozen, absorption; no index 0.
    let mut s = Sources::new();
    rewo_net::local_player_data::apply_local_metadata(
        &meta_body(PLAYER, -7, 140, 4.0),
        Some(PLAYER),
        None,
        &mut s.local,
    );
    rewo_net::local_player_data::apply_local_metadata(
        &meta_body(PLAYER + 1, 300, 0, 0.0),
        Some(PLAYER),
        None,
        &mut s.local,
    );
    let inp = s.inputs(reg, Some(GameMode::Survival), 20.0, 5000, None);
    c.record(
        "w0.metadata_one_seven_and_seventeen_reach_the_inputs_without_index_zero",
        inp.air_supply == -7
            && inp.absorption == 4.0
            && inp.heart_type == sh::HeartKind::Frozen
            && s.local.ticks_frozen() == 140,
        format!(
            "air {} (raw; the layout clamps), absorption {}, frozen ticks {} -> {:?}; the \
             other entity's packet changed nothing. A diving player's packet carries no \
             shared flags, so these are copied BEFORE the elytra guard",
            inp.air_supply,
            inp.absorption,
            s.local.ticks_frozen(),
            inp.heart_type
        ),
    );

    // w1 — effects: every effect is kept, the three flag bits land, a remove
    // drops one.
    let mut s = Sources::new();
    let speed = rewo_data::mob_effect_table::id_of("speed").expect("speed");
    let poison = rewo_data::mob_effect_table::id_of("poison").expect("poison");
    s.effects.apply_update(&effect_body(PLAYER, speed, 1, 1000, 0b0101));
    s.effects.apply_update(&effect_body(PLAYER, poison, 0, 300, 0b0010));
    s.effects.apply_update(&effect_body(PLAYER + 1, poison, 0, 300, 0b0111));
    let inp = s.inputs(reg, Some(GameMode::Survival), 20.0, 5000, None);
    let sp = inp.effects.iter().find(|e| e.id == speed).copied();
    let po = inp.effects.iter().find(|e| e.id == poison).copied();
    let before = inp.effects.len();
    s.effects.apply_remove(&{
        let mut w = PacketWriter::default();
        w.varint(PLAYER);
        w.varint(poison);
        w.into_bytes()
    });
    let after = s.inputs(reg, Some(GameMode::Survival), 20.0, 5000, None);
    c.record(
        "w1.every_effect_on_the_player_is_kept_with_its_three_flag_bits",
        before == 2
            && sp.is_some_and(|e| e.ambient && e.show_icon && e.beneficial && e.duration == 1000)
            && po.is_some_and(|e| !e.ambient && !e.show_icon && !e.beneficial)
            && inp.heart_type == sh::HeartKind::Poisoned
            && after.effects.len() == 1
            && after.heart_type == sh::HeartKind::Normal,
        format!(
            "speed {sp:?}, poison {po:?} (FLAG_AMBIENT 1, FLAG_VISIBLE 2, FLAG_SHOW_ICON 4); \
             poison selects POISIONED; the remove leaves {} and NORMAL. Before M168 the \
             map kept night vision and darkness only",
            after.effects.len()
        ),
    );

    // w2 — attributes through the local store: armour floors, max health rows.
    let mut s = Sources::new();
    let armor_id = reg.id_of("armor").expect("armor");
    let mh_id = reg.id_of("max_health").expect("max_health");
    let stored = rewo_net::attributes::apply_local_attributes(
        &attrs_body(PLAYER, &[(armor_id, 6.5), (mh_id, 40.0)]),
        Some(PLAYER),
        &mut s.attrs,
    );
    let inp = s.inputs(reg, Some(GameMode::Survival), 40.0, 5000, None);
    let rows = containers(&sh::layout(&inp, GUI_W, GUI_H))
        .iter()
        .map(|b| b.y as i32)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    c.record(
        "w2.update_attributes_for_you_floors_the_armour_and_sizes_the_rows",
        stored && inp.armor == 6 && inp.max_health_attr == 40.0 && rows == 2,
        format!(
            "armor attribute 6.5 -> getArmorValue {} (Mth.floor), max_health 40 -> {} and {rows} heart rows. \
             Armour is `Mth.floor(getAttributeValue(ARMOR))`, not a count of worn pieces",
            inp.armor, inp.max_health_attr
        ),
    );

    // w3 — the hurt window and the blink clock, through the same function.
    let mut s = Sources::new();
    // The join-time sync LOWERS the health (the client's default is 20)
    // and must still arm nothing — equal health could not tell the
    // `flashOnSetHealth` guard from `dmg <= 0`.
    s.hud.local_hurt.hurt_to(20.0, 18.0);
    let join = s.inputs(reg, Some(GameMode::Survival), 18.0, 5000, None);
    let join_armed = s.hud.local_hurt.is_invulnerable();
    s.hud.local_hurt.hurt_to(18.0, 14.0); // the hit, as `set_health` applies it
    s.hud.gui_tick = 10;
    let landing = s.inputs(reg, Some(GameMode::Survival), 14.0, 5100, None);
    s.hud.gui_tick = 13;
    let later = s.inputs(reg, Some(GameMode::Survival), 14.0, 5250, None);
    s.hud.gui_tick = 16;
    let later2 = s.inputs(reg, Some(GameMode::Survival), 14.0, 5400, None);
    c.record(
        "w3.a_hit_ghosts_the_old_health_and_blinks_on_the_odd_thirds_of_twenty_ticks",
        !join.blink
            && join.display_health == 18
            && !join_armed
            && !landing.blink
            && landing.display_health == 18
            && later.blink
            && later.display_health == 18
            && !later2.blink,
        format!(
            "join (blink {}, display {}), landing frame at tick 10 (blink {}, display {}), \n             tick 13 (blink {}, display {}), tick 16 (blink {}). The first `hurtTo` of a life \n             arms nothing; the hit arms `tickCount + 20`; `blink` is computed BEFORE the re-arm; \n             `(30 - 13) / 3 = 5` is odd (blink) and `(30 - 16) / 3 = 4` is even (dark)",
            join.blink,
            join.display_health,
            landing.blink,
            landing.display_health,
            later.blink,
            later.display_health,
            later2.blink
        ),
    );
}

// ---------------------------------------------------------------------------
// The derivation the frame calls.
// ---------------------------------------------------------------------------

fn check_derivation(c: &mut Checker, reg: &rewo_data::attributes::AttributeRegistry) {
    // d0 — canHurtPlayer is `isSurvival()` = SURVIVAL || ADVENTURE; unknown -> true.
    let mut s = Sources::new();
    let modes = [
        (None, true),
        (Some(GameMode::Survival), true),
        (Some(GameMode::Adventure), true),
        (Some(GameMode::Creative), false),
        (Some(GameMode::Spectator), false),
    ];
    let got: Vec<bool> = modes
        .iter()
        .map(|(m, _)| s.inputs(reg, *m, 20.0, 5000, None).can_hurt)
        .collect();
    c.record(
        "d0.can_hurt_is_survival_or_adventure_and_unknown_is_survival",
        got == modes.iter().map(|(_, w)| *w).collect::<Vec<_>>(),
        format!("(none, survival, adventure, creative, spectator) -> {got:?}"),
    );

    // d1 — HeartType.forPlayer precedence.
    let wither = rewo_data::mob_effect_table::id_of("wither").expect("wither");
    let poison = rewo_data::mob_effect_table::id_of("poison").expect("poison");
    let mut s = Sources::new();
    rewo_net::local_player_data::apply_local_metadata(
        &meta_body(PLAYER, 300, 140, 0.0),
        Some(PLAYER),
        None,
        &mut s.local,
    );
    s.effects.apply_update(&effect_body(PLAYER, wither, 0, 100, 0));
    let withered_frozen = s.inputs(reg, None, 20.0, 5000, None).heart_type;
    s.effects.apply_update(&effect_body(PLAYER, poison, 0, 100, 0));
    let all_three = s.inputs(reg, None, 20.0, 5000, None).heart_type;
    c.record(
        "d1.poison_beats_wither_beats_frozen",
        withered_frozen == sh::HeartKind::Withered && all_three == sh::HeartKind::Poisoned,
        format!("wither + frozen -> {withered_frozen:?}; + poison -> {all_three:?}"),
    );

    // d2 — the table supplies isBeneficial and getColor; an unknown id is neither.
    let mut s = Sources::new();
    let glowing = rewo_data::mob_effect_table::id_of("glowing").expect("glowing");
    let speed = rewo_data::mob_effect_table::id_of("speed").expect("speed");
    s.effects.apply_update(&effect_body(PLAYER, speed, 0, 100, 4));
    s.effects.apply_update(&effect_body(PLAYER, glowing, 0, 100, 4));
    s.effects.apply_update(&effect_body(PLAYER, 99, 0, 100, 4));
    let inp = s.inputs(reg, None, 20.0, 5000, None);
    let find = |id: i32| inp.effects.iter().find(|e| e.id == id).copied();
    let sp = find(speed);
    let gl = find(glowing);
    let un = find(99);
    c.record(
        "d2.beneficial_and_colour_come_from_the_jar_table_and_neutral_is_not_beneficial",
        sp.is_some_and(|e| e.beneficial && e.color == 3402751)
            && gl.is_some_and(|e| !e.beneficial && e.color == 9740385)
            && un.is_some_and(|e| !e.beneficial && e.color == 0),
        format!(
            "speed {sp:?} (BENEFICIAL, 3402751), glowing {gl:?} (NEUTRAL -> the harmful row), \
             id 99 {un:?} (past the table: not beneficial, colour 0)"
        ),
    );

    // d3 — the vehicle's max health resolves against ITS type's supplier.
    let horse_default = reg
        .default_base("minecraft:horse", "max_health")
        .expect("horse supplier");
    let mut s = Sources::new();
    let unsynced = s
        .inputs(
            reg,
            None,
            20.0,
            5000,
            Some(crate::live_cmd::VehicleSource {
                type_name: Some("minecraft:horse"),
                attributes: None,
                health: 12.0,
            }),
        )
        .vehicle;
    let mut synced_attrs = rewo_world::attributes::EntityAttributes::default();
    synced_attrs.apply(reg.id_of("max_health").expect("max_health"), 30.0, Vec::new());
    let synced = s
        .inputs(
            reg,
            None,
            20.0,
            5000,
            Some(crate::live_cmd::VehicleSource {
                type_name: Some("minecraft:horse"),
                attributes: Some(&synced_attrs),
                health: 12.0,
            }),
        )
        .vehicle;
    c.record(
        "d3.the_vehicles_max_health_is_its_own_suppliers_default_until_synced",
        unsynced == Some(sh::VehicleInput { max_health: horse_default as f32, health: 12.0 })
            && synced == Some(sh::VehicleInput { max_health: 30.0, health: 12.0 })
            && horse_default == 53.0,
        format!(
            "unsynced {unsynced:?} (`AbstractHorse.createBaseHorseAttributes` declares \
             MAX_HEALTH 53.0, read from the generated supplier table: {horse_default}), \
             synced {synced:?}"
        ),
    );

    // d4 — REGENERATION and HUNGER are `hasEffect` reads of the same map.
    let mut s = Sources::new();
    let regen = rewo_data::mob_effect_table::id_of("regeneration").expect("regeneration");
    let hunger = rewo_data::mob_effect_table::id_of("hunger").expect("hunger");
    let none = s.inputs(reg, None, 20.0, 5000, None);
    s.effects.apply_update(&effect_body(PLAYER, regen, 0, 100, 0));
    s.effects.apply_update(&effect_body(PLAYER, hunger, 0, 100, 0));
    let both = s.inputs(reg, None, 20.0, 5000, None);
    c.record(
        "d4.regeneration_and_hunger_are_has_effect_reads",
        !none.regeneration && !none.hunger_effect && both.regeneration && both.hunger_effect,
        format!(
            "none -> ({}, {}), both -> ({}, {})",
            none.regeneration, none.hunger_effect, both.regeneration, both.hunger_effect
        ),
    );
}

// ---------------------------------------------------------------------------
// Pixels.
// ---------------------------------------------------------------------------

struct Frame {
    px: Vec<u8>,
}

impl Frame {
    fn at(&self, x: i32, y: i32) -> [u8; 4] {
        let i = ((y as usize) * W as usize + x as usize) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
    /// The GUI pixel at `(gx, gy)`, sampled at its centre.
    fn gui(&self, gx: i32, gy: i32) -> [u8; 4] {
        self.at(gx * SCALE + SCALE / 2, gy * SCALE + SCALE / 2)
    }
    /// Count the GUI pixels in a GUI rect whose colour is `want` within `tol`.
    fn count_gui(&self, gx: i32, gy: i32, gw: i32, gh: i32, want: [u8; 3], tol: i32) -> usize {
        let mut n = 0;
        for y in gy..gy + gh {
            for x in gx..gx + gw {
                if close(self.gui(x, y), want, tol) {
                    n += 1;
                }
            }
        }
        n
    }
}

/// The sprite's own texel, straight out of the jar's PNG.
fn texel(s: &assets::HudSprite, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * s.w + x) * 4) as usize;
    [s.rgba[i], s.rgba[i + 1], s.rgba[i + 2], s.rgba[i + 3]]
}

fn rgb(p: [u8; 4]) -> [u8; 3] {
    [p[0], p[1], p[2]]
}

/// An opaque texel survives the atlas's sRGB decode and the attachment's
/// re-encode to within a byte; `tol` 2 covers the hardware's rounding.
fn close(got: [u8; 4], want: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (i32::from(got[i]) - i32::from(want[i])).abs() <= tol)
}

fn srgb_decode(b: u8) -> f32 {
    let s = b as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_encode(l: f32) -> u8 {
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

fn check_pixels(
    c: &mut Checker,
    args: &GaugeshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let hud = baked.hud.as_ref().ok_or("hud sprites missing from the jar")?;
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[gaugeshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("gaugeshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.set_sky_mode(SkyMode::None);
    let sprites = crate::live_cmd::hud_sprites(baked).ok_or("hud sprites missing from the jar")?;
    wr.init_hud(&mut gpu, &sprites)?;

    c.record(
        "p0.the_gui_scale_is_the_one_the_predictions_assume",
        rewo_gpu::hud::gui_scale(W as f32, H as f32) == SCALE as f32,
        format!("{W}x{H} -> scale {SCALE}, GUI space {GUI_W}x{GUI_H}"),
    );

    let ring = crate::stats::OverlayRing::default();
    let overlay_draw = rewo_gpu::overlay::OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    let clear = [0.0, 0.0, 0.0, 1.0];
    let mut shot_n = 0;
    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    inp: &SurvivalInputs|
     -> Result<Frame, String> {
        wr.set_hud(0, HudGauges::default(), sh::layout(inp, GUI_W, GUI_H));
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        if let Some(d) = &args.out_dir {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            off.save_png(gpu, &d.join(format!("gaugeshot_{shot_n:02}.png")))?;
        }
        shot_n += 1;
        Ok(Frame { px: off.read_rgba(gpu)? })
    };

    let xl = ref_x_left(GUI_W);
    let xr = ref_x_right(GUI_W);
    let yb = ref_y_base(GUI_H);

    // p1 — full health control: the heart and drumstick texels, and no armour.
    let full = shot(&mut gpu, &mut off, &mut wr, &SurvivalInputs::simple(20.0, 20))?;
    let heart_full = rgb(texel(&hud.player_hearts[8], 4, 4));
    let food_full = rgb(texel(&hud.food_full, 4, 4));
    let armour_row = ref_armor_y(GUI_H, 1);
    c.record(
        "p1.full_health_draws_the_full_heart_and_drumstick_texels_and_no_armour_row",
        close(full.gui(xl + 4, yb + 4), heart_full, 2)
            && close(full.gui(xr - 9 + 4, yb + 4), food_full, 2)
            && full.gui(xl + 4, armour_row + 4) == [0, 0, 0, 255]
            && texel(&hud.player_hearts[8], 4, 4)[3] == 255,
        format!(
            "cell 0 centre {:?} vs heart/full.png (4,4) {:?}; rightmost drumstick {:?} vs \
             food_full.png {:?}; armour row at y {} is clear (armor 0 draws nothing)",
            full.gui(xl + 4, yb + 4),
            heart_full,
            full.gui(xr - 9 + 4, yb + 4),
            food_full,
            armour_row
        ),
    );

    // p2 — armour 5: full, half (its two halves), empty; literal colours from the
    // PNGs measured beforehand, so a swapped atlas slot fails too.
    let armored = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            armor: 5,
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let ay = armour_row;
    c.record(
        "p2.armour_five_is_two_full_one_half_and_seven_empty_at_the_literal_colours",
        close(armored.gui(xl + 4, ay + 4), [184, 185, 196], 2)
            && close(armored.gui(xl + 8 + 4, ay + 4), [184, 185, 196], 2)
            && close(armored.gui(xl + 16 + 2, ay + 4), [184, 185, 196], 2)
            && close(armored.gui(xl + 16 + 6, ay + 4), [61, 61, 61], 2)
            && close(armored.gui(xl + 72 + 4, ay + 4), [61, 61, 61], 2)
            && close(armored.gui(xl + 4, ay + 4), rgb(texel(&hud.armor[0], 4, 4)), 2)
            && close(armored.gui(xl + 72 + 4, ay + 4), rgb(texel(&hud.armor[2], 4, 4)), 2),
        format!(
            "cells 0,1 centre {:?} (armor_full 184,185,196); cell 2 left {:?} / right {:?} \
             (armor_half is full on the left, empty-grey on the right, black down its middle); \
             cell 9 centre {:?} (armor_empty 61,61,61); at y = yLineBase - 10 = {ay}",
            armored.gui(xl + 4, ay + 4),
            armored.gui(xl + 16 + 2, ay + 4),
            armored.gui(xl + 16 + 6, ay + 4),
            armored.gui(xl + 72 + 4, ay + 4)
        ),
    );

    // p3 — air 150 underwater, no vehicle: the line sits at yLineBase - 10;
    // bubble 1 is full (nine blue texels), bubble 6 is the gap, bubble 10 empty.
    let diving = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            air_supply: 150,
            eye_in_water: true,
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let air_y = yb - 10;
    let blue = [0u8, 148, 255];
    let blue_in_png = (0..9)
        .flat_map(|y| (0..9).map(move |x| (x, y)))
        .filter(|&(x, y)| rgb(texel(&hud.air[0], x, y)) == blue && texel(&hud.air[0], x, y)[3] == 255)
        .count();
    let b1 = diving.count_gui(ref_bubble_x(GUI_W, 1), air_y, 9, 9, blue, 2);
    let gap = diving.count_gui(ref_bubble_x(GUI_W, 6), air_y, 9, 9, [0, 0, 0], 0);
    let b10 = diving.gui(ref_bubble_x(GUI_W, 10) + 4, air_y + 4);
    c.record(
        "p3.air_150_fills_bubble_one_leaves_bubble_six_empty_and_empties_bubble_ten",
        b1 == blue_in_png && blue_in_png == 9 && gap == 81 && close(b10, rgb(texel(&hud.air[2], 4, 4)), 2),
        format!(
            "bubble 1 has {b1} blue GUI pixels (air.png carries {blue_in_png}); bubble 6's cell \
             is {gap}/81 black (neither full, popping nor empty: a gap); bubble 10 centre {b10:?} \
             vs air_empty.png (4,4) {:?}; line at yLineBase - 10 = {air_y} because the food row \
             took ten and `getAirBubbleYLine`'s `(0 - 1) * 10` gave ten back",
            rgb(texel(&hud.air[2], 4, 4))
        ),
    );

    // p4 — a living vehicle: food gone, its hearts in its place, two rows, air
    // pushed up a row.
    let riding = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            vehicle: Some(sh::VehicleInput {
                max_health: 30.0,
                health: 15.0,
            }),
            air_supply: 150,
            eye_in_water: true,
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let vfull = riding.gui(xr - 9 + 4, yb + 4);
    let vrow2 = riding.gui(xr - 9 + 4, yb - 10 + 4);
    let b1_up = riding.count_gui(ref_bubble_x(GUI_W, 1), yb - 20, 9, 9, blue, 2);
    let b1_old = riding.count_gui(ref_bubble_x(GUI_W, 1), yb - 10, 9, 9, blue, 2);
    c.record(
        "p4.a_living_vehicle_replaces_the_food_with_its_hearts_and_lifts_the_air_line",
        close(vfull, [218, 102, 44], 2)
            && close(vfull, rgb(texel(&hud.vehicle_hearts[1], 4, 4)), 2)
            && close(vrow2, rgb(texel(&hud.vehicle_hearts[0], 4, 4)), 2)
            && !close(vfull, food_full, 2)
            && b1_up == 9
            && b1_old == 0,
        format!(
            "where the first drumstick was: {vfull:?} (vehicle_full 218,102,44, not \
             food_full {food_full:?}); second row {vrow2:?} (vehicle_container — 15 hearts, \
             the sixth is container-only); air bubble 1 at yLineBase - 20 has {b1_up} blue \
             and at - 10 has {b1_old}"
        ),
    );

    // p5 — two effects: the backgrounds' centres, the ambient one's teal rim,
    // and the icons' own texels.
    let speed = rewo_data::mob_effect_table::id_of("speed").expect("speed");
    let poison = rewo_data::mob_effect_table::id_of("poison").expect("poison");
    let eff = |id: i32, ambient: bool, dur: i32| sh::EffectInput {
        id,
        duration: dur,
        ambient,
        show_icon: true,
        beneficial: rewo_data::mob_effect_table::def(id).is_some_and(|d| d.category.is_beneficial()),
        color: rewo_data::mob_effect_table::def(id).map_or(0, |d| d.color),
    };
    let effects = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            effects: vec![eff(speed, false, 1000), eff(poison, true, 1000)],
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let (sx, sy) = (GUI_W - 25, 1);
    let (px_, py_) = (GUI_W - 25, 27);
    // The background's CENTRE is under the 18x18 icon (drawn at +3,+3), so
    // the background is probed on its row 2 — inside the frame, above the
    // icon — and predicted from the PNG's own texel there. A first cut probed
    // (12,12) and read the speed icon's centre, correctly.
    let bg_plain = rgb(texel(&hud.effect_background, 12, 2));
    let bg_ambient = rgb(texel(&hud.effect_background_ambient, 12, 2));
    let speed_px = rgb(texel(&hud.effect_icons[speed as usize], 9, 6));
    let poison_px = rgb(texel(&hud.effect_icons[poison as usize], 9, 6));
    c.record(
        "p5.two_effects_sit_in_two_rows_with_the_ambient_rim_and_their_own_icons",
        close(effects.gui(sx + 12, sy + 2), bg_plain, 2)
            && close(effects.gui(px_ + 12, py_ + 2), bg_ambient, 2)
            && close(effects.gui(px_ + 1, py_ + 1), [0, 84, 84], 2)
            && close(effects.gui(sx + 1, sy + 1), [0, 0, 0], 2)
            && close(effects.gui(sx + 3 + 9, sy + 3 + 6), [139, 56, 51], 2)
            && close(effects.gui(sx + 3 + 9, sy + 3 + 6), speed_px, 2)
            && close(effects.gui(px_ + 3 + 9, py_ + 3 + 6), [100, 166, 58], 2)
            && close(effects.gui(px_ + 3 + 9, py_ + 3 + 6), poison_px, 2),
        format!(
            "speed (beneficial) at ({sx},{sy}): row 2 {:?} vs png {bg_plain:?}, rim {:?}              (the plain background is black-rimmed); poison (harmful, ambient) at              ({px_},{py_}): row 2 {:?} vs png {bg_ambient:?}, rim {:?} (0,84,84 teal: the two              backgrounds differ ONLY on their rim); icons at +3,+3: speed (9,6) {:?} =              139,56,51; poison (9,6) {:?} = 100,166,58",
            effects.gui(sx + 12, sy + 2),
            effects.gui(sx + 1, sy + 1),
            effects.gui(px_ + 12, py_ + 2),
            effects.gui(px_ + 1, py_ + 1),
            effects.gui(sx + 3 + 9, sy + 3 + 6),
            effects.gui(px_ + 3 + 9, py_ + 3 + 6)
        ),
    );

    // p6 — the fade is a VERTEX tint, blended in linear space over the
    // background's own texel.
    let fading = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            effects: vec![eff(speed, false, 100)],
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let alpha = sh::effect_icon_alpha(&eff(speed, false, 100));
    let icon = texel(&hud.effect_icons[speed as usize], 9, 6);
    let under = texel(&hud.effect_background, 12, 9);
    let predicted: [u8; 3] = std::array::from_fn(|i| {
        let b = srgb_decode(under[i]);
        let f = srgb_decode(icon[i]);
        srgb_encode(b + (f - b) * alpha)
    });
    let got = fading.gui(sx + 3 + 9, sy + 3 + 6);
    c.record(
        "p6.the_last_two_hundred_ticks_fade_the_icon_by_a_linear_space_tint",
        (alpha - 0.625).abs() < 2e-3 && close(got, predicted, 3) && !close(got, rgb(icon), 3),
        format!(
            "100 ticks left -> alpha {alpha:.4}; icon texel {:?} over background texel {:?} \
             predicts {predicted:?} (lerp in linear, re-encode); got {got:?}. The opaque icon \
             would read {:?}",
            rgb(icon),
            rgb(under),
            rgb(icon)
        ),
    );

    // p7 — the jump bar: half a charge, then a cooldown.
    let half = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            jump: Some(sh::JumpInput {
                scale: 0.5,
                cooldown: 0,
            }),
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let cool = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            jump: Some(sh::JumpInput {
                scale: 0.5,
                cooldown: 3,
            }),
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let (bl, bt) = (ref_bar_left(GUI_W), ref_bar_top(GUI_H));
    let progress_px = ref_lerp_discrete(0.5);
    let inside = half.gui(bl + progress_px - 1, bt + 2);
    let outside = half.gui(bl + progress_px, bt + 2);
    c.record(
        "p7.the_jump_bar_fills_lerp_discrete_pixels_and_the_cooldown_replaces_it",
        close(inside, [19, 46, 101], 2)
            && close(outside, [60, 50, 68], 2)
            && close(cool.gui(bl + 45, bt + 2), [57, 0, 44], 2)
            && close(cool.gui(bl + 150, bt + 2), [57, 0, 44], 2),
        format!(
            "scale 0.5 -> {progress_px} px: column {} is progress {inside:?} (19,46,101) and \
             column {} is background {outside:?} (60,50,68); cooldown 3 paints (57,0,44) at \
             both {:?} / {:?}. Bar at ({bl},{bt}), the XP bar's slot",
            progress_px - 1,
            progress_px,
            cool.gui(bl + 45, bt + 2),
            cool.gui(bl + 150, bt + 2)
        ),
    );

    // p8 — hardcore + blink: the old health's blink fill lands on cell 9.
    let blinking = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            health: 14.0,
            display_health: 20,
            blink: true,
            hardcore: true,
            ..SurvivalInputs::simple(14.0, 20)
        },
    )?;
    // Cell 9: container_hardcore_blinking (slot 0*8 + 4 + 0 + 1 = 5) under
    // hardcore_full_blinking (slot 1*8 + 4 + 0 + 1 = 13); the fill wins where
    // it is opaque.
    let fill = texel(&hud.player_hearts[13], 4, 4);
    let cont = texel(&hud.player_hearts[5], 4, 4);
    let want9 = if fill[3] == 255 { rgb(fill) } else { rgb(cont) };
    // Cell 0: container_hardcore_blinking under TWO fills: the blink ghost and
    // the current hardcore_full (slot 1*8 + 4 = 12) drawn last.
    let cur = texel(&hud.player_hearts[12], 4, 4);
    let want0 = if cur[3] == 255 { rgb(cur) } else { want9 };
    c.record(
        "p8.hardcore_blink_draws_the_ghost_on_the_lost_hearts_and_the_live_fill_on_the_rest",
        close(blinking.gui(xl + 72 + 4, yb + 4), want9, 2)
            && close(blinking.gui(xl + 4, yb + 4), want0, 2)
            && !close(blinking.gui(xl + 72 + 4, yb + 4), heart_full, 2),
        format!(
            "cell 9 {:?} vs hardcore_full_blinking (4,4) {want9:?}; cell 0 {:?} vs \
             hardcore_full {want0:?}; neither is the plain full heart {heart_full:?}. Indices \
             are `hardcore * 4 + half * 2 + blink` inside the kind's eight",
            blinking.gui(xl + 72 + 4, yb + 4),
            blinking.gui(xl + 4, yb + 4)
        ),
    );

    // p9 — frozen + absorption: frozen fills, gold containers on a second row.
    let frozen = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            heart_type: sh::HeartKind::Frozen,
            absorption: 4.0,
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let frozen_full = rgb(texel(&hud.player_hearts[40], 4, 4));
    let absorbing_full = rgb(texel(&hud.player_hearts[32], 4, 4));
    c.record(
        "p9.frozen_hearts_and_absorption_hearts_draw_their_own_kinds",
        close(frozen.gui(xl + 4, yb + 4), frozen_full, 2)
            && close(frozen.gui(xl + 4, yb - 10 + 4), absorbing_full, 2)
            && !close(frozen.gui(xl + 4, yb + 4), heart_full, 2),
        format!(
            "cell 0 {:?} vs frozen_full (slot 40) {frozen_full:?}; cell 10 on the second row \
             {:?} vs absorbing_full (slot 32) {absorbing_full:?} — absorption 4 is two more \
             containers at rowHeight 10",
            frozen.gui(xl + 4, yb + 4),
            frozen.gui(xl + 4, yb - 10 + 4)
        ),
    );

    // p10 — creative: the whole gauge band is clear.
    let creative = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            can_hurt: false,
            armor: 10,
            air_supply: 0,
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let lit = (yb - 30..yb + 9)
        .flat_map(|y| (xl..xr).map(move |x| (x, y)))
        .filter(|&(x, y)| creative.gui(x, y) != [0, 0, 0, 255])
        .count();
    c.record(
        "p10.creative_draws_none_of_the_player_gauges",
        lit == 0,
        format!(
            "{lit} lit GUI pixels in the band x {xl}..{xr}, y {}..{} with armour 10 and no air \
             — `canHurtPlayer()` gates the whole of extractPlayerHealth",
            yb - 30,
            yb + 9
        ),
    );

    // p11 — the hunger effect swaps the drumstick sheet.
    let hungry = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        &SurvivalInputs {
            hunger_effect: true,
            ..SurvivalInputs::simple(20.0, 20)
        },
    )?;
    let hunger_full = rgb(texel(&hud.food_hunger[0], 4, 4));
    c.record(
        "p11.the_hunger_effect_swaps_every_drumstick_for_its_hunger_sprite",
        close(hungry.gui(xr - 9 + 4, yb + 4), hunger_full, 2)
            && close(hungry.gui(xr - 72 - 9 + 4, yb + 4), hunger_full, 2)
            && hunger_full != food_full,
        format!(
            "cells 0 and 9 {:?} / {:?} vs food_full_hunger (4,4) {hunger_full:?}, which differs \
             from food_full {food_full:?}",
            hungry.gui(xr - 9 + 4, yb + 4),
            hungry.gui(xr - 72 - 9 + 4, yb + 4)
        ),
    );

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}
