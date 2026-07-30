//! `rewo hurtshot` — M21's permanent combat-damage oracle.
//!
//! `ClientboundDamageEventPacket` was falling off the dispatch chain, so
//! nothing an entity took ever showed: no red flash, no limb kick. M21 wires
//! it, and this gate proves the whole receipt path plus the one property that
//! only a GPU can answer — the exact colour of the flash.
//!
//! ```text
//! raw damage_event body (VarInt id + holder + 2 optional ids + Optional<Vec3>)
//!   -> rewo_net::route_damage_event    (production packet-id selection)
//!   -> apply_damage_event              (missing-entity + living gates)
//!   -> EntityTable::hurt / tick_lerp   (the exact hurtTime clock + walk kick)
//!   -> EntityDraw.hurt                 (`hasRedOverlay`)
//!   -> the entity pipeline, read back  (the shader's overlay mix)
//! ```
//!
//! **The flash is verified by prediction, not by a hard-coded colour.** The
//! capsule is rendered twice — unhurt and hurt — and the hurt pixel is
//! predicted *from the unhurt one* by undoing the light, encoding to sRGB,
//! mixing vanilla's red overlay texel, decoding, and re-applying the light.
//! That tests the shader's mix and its position in the pipeline without the
//! oracle needing to know the face shade, and it fails if the mix is done in
//! linear space or applied after the lightmap instead of before it.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count, like `eventshot`,
//! `danceshot` and `swingshot`.

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::entity_types::{EntityClasses, EntityTypes};
use rewo_data::{packets::Packets, DataPaths};
use rewo_gpu::entities::{EntityDraw, EntityModelKind, MobTextures};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::ids::Ids;
use rewo_net::route_damage_event;
use rewo_world::entities::{EntityState, EntityTable};

use crate::stats::OverlayRing;

/// Total named properties this gate asserts.
///
/// a1-a7 receipt, b1-b6 clock, c1-c5 flash, d1-d20 death/topple, and M81's
/// e1-e18 — the `hurt_animation` receipt, the tilt's easing and conjugation,
/// and the four that need a rendered frame.
const EXPECTED_WITNESSES: usize = 56;

/// `OverlayTexture`'s red row is 0xB3FF0000 — alpha 0xB3 = 179.
const HURT_ALPHA: f32 = 179.0 / 255.0;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const TEX: u32 = 16;
const W: u32 = 128;
const H: u32 = 128;
/// Per-channel tolerance in sRGB bytes. The prediction rounds through the same
/// encode the GPU store does, so this absorbs rounding only.
const CHANNEL_TOL: i32 = 2;

#[derive(ClapArgs)]
pub struct HurtshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally.
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Skip the Vulkan half (CPU witnesses only). For a box with no device —
    /// the count then fails closed, so it can never look green.
    #[arg(long, default_value_t = false)]
    no_gpu: bool,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[hurtshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

pub fn run(args: HurtshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!("[hurtshot] mode: {mode} (the oracle asserts unconditionally)");

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let types = EntityTypes::load(&paths.registries_json())?;
    let classes = EntityClasses::resolve(&types)?;
    let zombie = types
        .id_of("minecraft:zombie")
        .ok_or("registries.json: no minecraft:zombie")?;
    let boat = types
        .id_of("minecraft:oak_boat")
        .ok_or("registries.json: no minecraft:oak_boat")?;
    // M24: the one type death event 3 must NOT kill client-side.
    let player = types.player_id;

    println!(
        "[hurtshot] ids: damage_event={} (animate={} — a distinct real id); \
         types: zombie={zombie} boat={boat}",
        ids.cb_play_damage_event, ids.cb_play_animate
    );

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_receipt(&mut c, &ids, &classes, zombie, boat);
    check_clock(&mut c, &ids, &classes, zombie);
    check_death(&mut c, &ids, &classes, zombie, boat, player);
    check_tilt(&mut c, &ids, &classes, zombie, player);
    if args.no_gpu {
        println!("[hurtshot] --no-gpu: the flash witnesses are SKIPPED (count fails closed)");
    } else {
        check_flash(&mut c)?;
    }

    println!(
        "[hurtshot] witnesses observed: {} / {}",
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
            "witness count {} != expected {EXPECTED_WITNESSES} — a named property was \
             skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[hurtshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ------------------------------------------------------------------- bodies

fn varint(mut v: i32, out: &mut Vec<u8>) {
    let mut u = v as u32;
    loop {
        let b = (u & 0x7F) as u8;
        u >>= 7;
        if u == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
    v = 0;
    let _ = v;
}

/// A full `ClientboundDamageEventPacket` body.
fn damage_body(eid: i32, damage_type: i32, cause: i32, direct: i32, pos: Option<[f64; 3]>) -> Vec<u8> {
    let mut b = Vec::new();
    varint(eid, &mut b);
    // `ByteBufCodecs.holderRegistry` — the raw 0-based registry id.
    varint(damage_type, &mut b);
    // `writeOptionalEntityId` writes id + 1.
    varint(cause + 1, &mut b);
    varint(direct + 1, &mut b);
    match pos {
        Some(p) => {
            b.push(1);
            for v in p {
                b.extend_from_slice(&v.to_be_bytes());
            }
        }
        None => b.push(0),
    }
    b
}

fn table(id: i32, type_id: i32) -> EntityTable {
    let mut t = EntityTable::default();
    t.add(id, EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0));
    t
}

/// `ClientboundHurtAnimationPacket` — VarInt id then a **fixed big-endian f32**
/// yaw (M81).
fn hurt_animation_body(id: i32, yaw: f32) -> Vec<u8> {
    let mut b = Vec::new();
    varint(id, &mut b);
    b.extend_from_slice(&yaw.to_be_bytes());
    b
}

// ------------------------------------------------------------------ witnesses

fn check_receipt(c: &mut Checker, ids: &Ids, classes: &EntityClasses, zombie: i32, boat: i32) {
    c.record(
        "a1.the_damage_event_id_resolves_and_is_distinct",
        ids.cb_play_damage_event != ids.cb_play_animate
            && ids.cb_play_damage_event != ids.cb_play_entity_event,
        format!(
            "damage_event={} vs animate={} vs entity_event={}",
            ids.cb_play_damage_event, ids.cb_play_animate, ids.cb_play_entity_event
        ),
    );

    // The happy path, with a source position present.
    let mut t = table(1, zombie);
    let routed = route_damage_event(
        ids.cb_play_damage_event,
        &damage_body(1, 0, -1, -1, Some([1.0, 2.0, 3.0])),
        ids,
        &mut t,
        Some(classes),
    );
    c.record(
        "a2.a_damage_event_arms_the_hurt_clock",
        routed && t.hurt_state(1).hurt_time == 10 && t.hurt_state(1).hurt_duration == 10,
        format!(
            "routed={routed} → hurtTime={} hurtDuration={} (handleDamageEvent sets both to 10)",
            t.hurt_state(1).hurt_time,
            t.hurt_state(1).hurt_duration
        ),
    );

    // Same body, wrong packet id: the router must select on the id.
    let mut t = table(1, zombie);
    let routed = route_damage_event(
        ids.cb_play_animate,
        &damage_body(1, 0, -1, -1, None),
        ids,
        &mut t,
        Some(classes),
    );
    c.record(
        "a3.a_wrong_packet_id_is_inert",
        !routed && t.hurt_state(1).hurt_time == 0,
        format!("routed={routed} hurtTime={}", t.hurt_state(1).hurt_time),
    );

    // An untracked entity: vanilla's `if (entity != null)`.
    let mut t = table(1, zombie);
    route_damage_event(
        ids.cb_play_damage_event,
        &damage_body(77, 0, -1, -1, None),
        ids,
        &mut t,
        Some(classes),
    );
    c.record(
        "a4.an_untracked_entity_is_inert",
        t.hurt_state(1).hurt_time == 0 && t.hurt_state(77).hurt_time == 0,
        format!(
            "entity 77 unknown → tracked 1 hurtTime={} and 77 hurtTime={}",
            t.hurt_state(1).hurt_time,
            t.hurt_state(77).hurt_time
        ),
    );

    // A non-living entity has `Entity.handleDamageEvent`, which has no clock.
    let mut t = table(1, boat);
    route_damage_event(
        ids.cb_play_damage_event,
        &damage_body(1, 0, -1, -1, None),
        ids,
        &mut t,
        Some(classes),
    );
    c.record(
        "a5.a_non_living_entity_has_no_hurt_clock",
        t.hurt_state(1).hurt_time == 0,
        format!(
            "boat hurtTime={} (LivingEntity.handleDamageEvent is an override)",
            t.hurt_state(1).hurt_time
        ),
    );

    // The optional-position flag really is walked: a body that claims a
    // position but is truncated must not arm the clock.
    let mut short = damage_body(1, 0, -1, -1, Some([0.0, 0.0, 0.0]));
    short.truncate(short.len() - 8);
    let mut t = table(1, zombie);
    route_damage_event(ids.cb_play_damage_event, &short, ids, &mut t, Some(classes));
    let truncated_inert = t.hurt_state(1).hurt_time == 0;
    // …while the same body intact does arm it.
    let mut t2 = table(1, zombie);
    route_damage_event(
        ids.cb_play_damage_event,
        &damage_body(1, 0, -1, -1, Some([0.0, 0.0, 0.0])),
        ids,
        &mut t2,
        Some(classes),
    );
    c.record(
        "a6.a_truncated_body_is_rejected_but_the_intact_one_is_not",
        truncated_inert && t2.hurt_state(1).hurt_time == 10,
        format!(
            "truncated → hurtTime 0 ({truncated_inert}); intact → hurtTime {} \
             (a short read would desync the next packet in the buffer)",
            t2.hurt_state(1).hurt_time
        ),
    );

    // The two optional entity ids are written as id + 1, so -1 means "none"
    // and a real id round-trips. Both forms must leave the walk aligned.
    let mut t = table(1, zombie);
    route_damage_event(
        ids.cb_play_damage_event,
        &damage_body(1, 12, 5, 9, None),
        ids,
        &mut t,
        Some(classes),
    );
    c.record(
        "a7.real_cause_and_direct_ids_keep_the_body_aligned",
        t.hurt_state(1).hurt_time == 10,
        format!(
            "cause=5 direct=9 damageType=12 → hurtTime={} (each id is written +1)",
            t.hurt_state(1).hurt_time
        ),
    );
}

fn check_clock(c: &mut Checker, ids: &Ids, classes: &EntityClasses, zombie: i32) {
    let hit = |t: &mut EntityTable| {
        route_damage_event(
            ids.cb_play_damage_event,
            &damage_body(1, 0, -1, -1, None),
            ids,
            t,
            Some(classes),
        );
    };

    // 10 → 0 over exactly ten ticks, one per tick.
    let mut t = table(1, zombie);
    hit(&mut t);
    let mut seq = vec![t.hurt_state(1).hurt_time];
    for _ in 0..11 {
        t.tick_lerp();
        seq.push(t.hurt_state(1).hurt_time);
    }
    c.record(
        "b1.the_hurt_clock_counts_down_one_per_tick",
        seq == vec![10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0],
        format!("hurtTime by tick: {seq:?}"),
    );

    // `hasRedOverlay` is `hurtTime > 0`, so the flash lasts exactly 10 ticks.
    let mut t = table(1, zombie);
    hit(&mut t);
    let mut lit = 0;
    for _ in 0..14 {
        if t.hurt_state(1).has_red_overlay() {
            lit += 1;
        }
        t.tick_lerp();
    }
    c.record(
        "b2.the_red_overlay_lasts_exactly_ten_ticks",
        lit == 10,
        format!("frames with hasRedOverlay over 14 ticks: {lit} (want 10)"),
    );

    // A second hit re-arms from 10 rather than extending.
    let mut t = table(1, zombie);
    hit(&mut t);
    t.tick_lerp();
    t.tick_lerp();
    let mid = t.hurt_state(1).hurt_time;
    hit(&mut t);
    c.record(
        "b3.a_repeat_hit_re_arms_the_clock",
        mid == 8 && t.hurt_state(1).hurt_time == 10,
        format!("hurtTime {mid} → {} after a second hit", t.hurt_state(1).hurt_time),
    );

    // `walkAnimation.setSpeed(1.5F)` is the first line of handleDamageEvent.
    // The stored speed is 1.5; the *rendered* amplitude clamps to 1.0, exactly
    // as `WalkAnimationState.speed(partialTicks)` does.
    let mut t = table(1, zombie);
    let before = t.get(1).map(|e| e.limb().1).unwrap_or(-1.0);
    hit(&mut t);
    let after = t.get(1).map(|e| e.limb().1).unwrap_or(-1.0);
    c.record(
        "b4.the_hit_kicks_the_walk_animation_and_the_render_clamps_it",
        before == 0.0 && (after - 1.0).abs() < 1e-6,
        format!(
            "limb amplitude {before} → {after}: setSpeed(1.5) stored, clamped to 1.0 by \
             `speed(partialTicks) = min(lerp(...), 1.0)`"
        ),
    );

    // The kick decays back through the ordinary smoothing once ticking.
    let mut t = table(1, zombie);
    hit(&mut t);
    t.tick_lerp();
    let decayed = t.get(1).map(|e| e.limb().1).unwrap_or(-1.0);
    c.record(
        "b5.the_kick_decays_through_the_ordinary_smoothing",
        decayed < 1.0 - 1e-3,
        format!(
            "after one still tick the amplitude fell to {decayed:.4} (target 0, factor 0.4 \
             → 1.5·0.6 = 0.9)"
        ),
    );

    // Removal drops the clock, and a recycled id does not inherit it.
    let mut t = table(1, zombie);
    hit(&mut t);
    t.remove(1);
    let cleared = t.hurt_state(1).hurt_time;
    t.add(1, EntityState::new(0, zombie, 0.0, 0.0, 0.0, 0.0, 0.0));
    c.record(
        "b6.removal_clears_the_clock_and_a_reused_id_does_not_inherit_it",
        cleared == 0 && t.hurt_state(1).hurt_time == 0,
        format!("after remove hurtTime={cleared}, after re-add {}", t.hurt_state(1).hurt_time),
    );
}

// ---------------------------------------------------------------- the flash

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Render one capsule with the given light and hurt flag, return the sampled
/// pixel as sRGB bytes.
#[allow(clippy::too_many_arguments)]
fn render_capsule(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &OverlayDraw,
    light: [f32; 3],
    hurt: bool,
) -> Result<[u8; 3], String> {
    let eye = Vec3::new(0.0, 1.0, 3.0);
    let dir = Vec3::new(0.0, 0.0, -1.0);
    let up = Vec3::Y;
    wr.set_camera(eye.to_array());
    let view = Mat4::look_to_rh(eye, dir, up);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(
        60f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    let view_proj = (proj * view).to_cols_array_2d();
    let right = dir.cross(up).normalize_or_zero().to_array();

    let capsule = EntityDraw {
        pos: [0.0, 0.0, 0.0],
        width: 1.0,
        height: 2.0,
        color: [1.0, 1.0, 1.0],
        name: None,
        // M59: no health bar in a still — the gate renders its own.
        health: None,
        kind: EntityModelKind::Capsule,
        yaw: 0.0,
        death_time: 0.0,
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        bob_offset: 0.0,
        ground_seed: 0,
        ground_age: None,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt,
        held: [None, None],
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        skin_uv: None,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        light,
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: None,
    };
    wr.set_entities(&[capsule], right, up.to_array(), 0.0);
    off.render(gpu, Some((&mut *wr, view_proj)), draw, CLEAR)?;
    let img = off.read_rgba(gpu)?;
    // Average a small patch at the frame centre — the capsule sits at the
    // origin directly ahead. A brightest-texel search would find the *sky*,
    // which is drawn by the same pass and is brighter than a shaded capsule.
    let (cx, cy) = (W / 2, H / 2);
    let r = 6u32;
    let (mut sum, mut n) = ([0f64; 3], 0f64);
    for py in cy.saturating_sub(r)..(cy + r).min(H) {
        for px in cx.saturating_sub(r)..(cx + r).min(W) {
            let i = ((py * W + px) * 4) as usize;
            for ch in 0..3 {
                sum[ch] += img[i + ch] as f64;
            }
            n += 1.0;
        }
    }
    Ok(std::array::from_fn(|ch| (sum[ch] / n).round() as u8))
}

/// M24 — the death clock, the topple, and the red overlay's second term.
///
/// The clock witnesses drive `entity_event` through the production router, so
/// they prove the packet-id selection, the living gate and the `instanceof
/// Player` exclusion together. The topple witnesses compare the production
/// `death_flip_degrees` / roll against this file's independent transcription of
/// `LivingEntityRenderer.setupRotations`.
fn check_death(
    c: &mut Checker,
    ids: &Ids,
    classes: &EntityClasses,
    zombie: i32,
    boat: i32,
    player: i32,
) {
    use rewo_world::entities::DeathState;

    // `ClientboundEntityEventPacket`: fixed BE i32 id + signed byte event.
    let ev_body = |eid: i32, event: i8| -> Vec<u8> {
        let mut b = eid.to_be_bytes().to_vec();
        b.push(event as u8);
        b
    };
    let send = |t: &mut EntityTable, eid: i32, event: i8| {
        rewo_net::route_entity_event(
            ids.cb_play_entity_event,
            &ev_body(eid, event),
            ids,
            t,
            None,
            None,
            0,
            Some(classes),
        );
    };

    // --- what makes an entity dying ---------------------------------------
    let fresh = table(1, zombie);
    c.record(
        "d1.an_entity_that_sent_nothing_is_alive_at_one_health",
        fresh.death_state(1) == DeathState::ALIVE && !fresh.death_state(1).is_dead_or_dying(),
        format!(
            "{:?} — `entityData.define(DATA_HEALTH_ID, 1.0F)`, so a Default of 0.0 \
             would make every freshly-spawned entity read as dead",
            fresh.death_state(1)
        ),
    );

    let mut t = table(1, zombie);
    t.set_health(1, 0.0);
    let zero = t.death_state(1).is_dead_or_dying();
    t.set_health(1, 0.5);
    let alive = t.death_state(1).is_dead_or_dying();
    c.record(
        "d2.zero_health_is_dying_and_partial_health_is_not",
        zero && !alive,
        format!("health 0.0 -> dying={zero}; health 0.5 -> dying={alive} (the test is <= 0)"),
    );

    // --- entity event 3 ----------------------------------------------------
    let mut t = table(1, zombie);
    send(&mut t, 1, 3);
    let killed = t.death_state(1);
    c.record(
        "d3.death_event_three_kills_a_mob_client_side",
        killed.health == 0.0 && killed.dead && killed.is_dead_or_dying(),
        format!(
            "zombie + event 3 -> {killed:?} — `setHealth(0); die(...)`, so the client \
             starts the death animation without waiting for a health update"
        ),
    );

    let mut t = table(1, player);
    send(&mut t, 1, 3);
    c.record(
        "d4.death_event_three_does_not_kill_a_player",
        !t.death_state(1).is_dead_or_dying(),
        format!(
            "player + event 3 -> {:?} — vanilla guards the kill with \
             `if (!(this instanceof Player))`, and the set is machine-extracted so \
             Mannequin (an Avatar, not a Player) is not caught by it",
            t.death_state(1)
        ),
    );

    let mut t = table(1, boat);
    send(&mut t, 1, 3);
    c.record(
        "d5.a_non_living_entity_ignores_the_death_event",
        !t.death_state(1).is_dead_or_dying(),
        format!(
            "boat + event 3 -> {:?} — the switch lives on LivingEntity",
            t.death_state(1)
        ),
    );

    // Mutation partner: a neighbouring event byte must do nothing.
    let mut t = table(1, zombie);
    send(&mut t, 1, 2);
    let e2 = t.death_state(1).is_dead_or_dying();
    send(&mut t, 1, 4);
    let e4 = t.death_state(1).is_dead_or_dying();
    c.record(
        "d6.only_event_three_kills",
        !e2 && !e4,
        format!("event 2 -> dying={e2}; event 4 -> dying={e4} (both must be false)"),
    );

    // --- the clock ---------------------------------------------------------
    let mut t = table(1, zombie);
    send(&mut t, 1, 3);
    let mut seq = vec![t.death_state(1).death_time];
    for _ in 0..5 {
        t.tick_lerp();
        seq.push(t.death_state(1).death_time);
    }
    c.record(
        "d7.the_death_clock_counts_up_one_per_tick",
        seq == vec![0, 1, 2, 3, 4, 5],
        format!("deathTime by tick: {seq:?} — tickDeath is `this.deathTime++`"),
    );

    // A living entity's clock must never start.
    let mut t = table(1, zombie);
    for _ in 0..5 {
        t.tick_lerp();
    }
    c.record(
        "d8.a_living_entity_never_starts_the_death_clock",
        t.death_state(1).death_time == 0,
        format!("deathTime after 5 ticks alive = {}", t.death_state(1).death_time),
    );

    // A repeat must not rewind: `die()` early-returns when already dead.
    let mut t = table(1, zombie);
    send(&mut t, 1, 3);
    t.tick_lerp();
    t.tick_lerp();
    send(&mut t, 1, 3);
    c.record(
        "d9.a_repeated_death_event_does_not_rewind_the_clock",
        t.death_state(1).death_time == 2,
        format!(
            "deathTime={} after a second event at tick 2 (want 2, NOT 0)",
            t.death_state(1).death_time
        ),
    );

    // Removal clears it, so a reused server id cannot inherit a corpse.
    let mut t = table(1, zombie);
    send(&mut t, 1, 3);
    t.tick_lerp();
    let before_removal = t.death_state(1);
    t.remove(1);
    c.record(
        "d10.removal_drops_the_death_state",
        t.death_state(1) == DeathState::ALIVE,
        format!(
            "before remove: {before_removal:?}; after: {:?} — a reused server id must not inherit a corpse",
            t.death_state(1)
        ),
    );

    // --- the render-side clock --------------------------------------------
    let mut t = table(1, zombie);
    send(&mut t, 1, 3);
    let before = t.death_state(1).render_death_time(0.5);
    t.tick_lerp();
    let after = t.death_state(1).render_death_time(0.5);
    c.record(
        "d11.the_render_clock_is_guarded_on_the_integer_count",
        before == 0.0 && (after - 1.5).abs() < 1e-6,
        format!(
            "dying but deathTime 0 -> {before} (want 0, not 0.5); deathTime 1 -> {after} \
             (want 1.5) — `deathTime > 0 ? deathTime + partialTicks : 0`"
        ),
    );

    // --- the overlay's second term ----------------------------------------
    let mut t = table(1, zombie);
    send(&mut t, 1, 3);
    let dying_unlit = t.has_red_overlay(1);
    t.tick_lerp();
    let dying_lit = t.has_red_overlay(1);
    for _ in 0..40 {
        t.tick_lerp();
    }
    let still_lit = t.has_red_overlay(1);
    c.record(
        "d12.a_dying_entity_stays_red_after_the_hurt_clock_expires",
        !dying_unlit && dying_lit && still_lit,
        format!(
            "deathTime 0 -> {dying_unlit}; deathTime 1 -> {dying_lit}; after 41 ticks -> \
             {still_lit} — `hasRedOverlay = hurtTime > 0 || deathTime > 0`, and M21 \
             shipped only the first term"
        ),
    );

    // --- the topple --------------------------------------------------------
    // Independently transcribed from `LivingEntityRenderer.setupRotations`.
    let want_roll = |death_time: f32, flip: f32| -> f32 {
        if death_time <= 0.0 {
            return 0.0;
        }
        let mut fall = ((death_time - 1.0) / 20.0 * 1.6).max(0.0).sqrt();
        if fall > 1.0 {
            fall = 1.0;
        }
        (fall * flip).to_radians()
    };
    let k = rewo_gpu::mobs::EntityModelKind::Zombie;
    let mut roll_ok = true;
    let mut rows = Vec::new();
    for dt in [0.0f32, 1.0, 1.5, 6.0, 13.5, 20.0, 40.0] {
        let got = rewo_gpu::entities::death_roll_for(k, dt);
        let want = want_roll(dt, 90.0);
        roll_ok &= (got - want).abs() < 1e-6;
        rows.push(format!("t={dt}: {:.6} (want {:.6})", got, want));
    }
    c.record(
        "d13.the_topple_angle_matches_the_sqrt_curve",
        roll_ok,
        format!("{} — sqrt((deathTime-1)/20*1.6) clamped to 1, times 90 deg", rows.join("; ")),
    );

    // The curve must saturate: at deathTime 13.5 the argument is exactly 1.0.
    let saturated = rewo_gpu::entities::death_roll_for(k, 13.5);
    let past = rewo_gpu::entities::death_roll_for(k, 40.0);
    c.record(
        "d14.the_topple_saturates_at_ninety_degrees_and_stays",
        (saturated - std::f32::consts::FRAC_PI_2).abs() < 1e-6
            && (past - std::f32::consts::FRAC_PI_2).abs() < 1e-6,
        format!(
            "deathTime 13.5 -> {:.6} rad, deathTime 40 -> {:.6} rad (want pi/2 = {:.6}) \
             — (13.5-1)/20*1.6 is exactly 1.0, which is where the clamp bites",
            saturated,
            past,
            std::f32::consts::FRAC_PI_2
        ),
    );

    // The three overriding renderers topple twice as far.
    use rewo_gpu::mobs::EntityModelKind as K;
    let flip = rewo_gpu::entities::death_flip_degrees;
    let over = [K::Spider, K::CaveSpider, K::Silverfish, K::Endermite];
    let normal = [K::Zombie, K::Player, K::Cow, K::Skeleton, K::Creeper];
    c.record(
        "d15.only_the_three_overriding_renderers_flip_one_eighty",
        over.iter().all(|&k| flip(k) == 180.0) && normal.iter().all(|&k| flip(k) == 90.0),
        format!(
            "spider/cave_spider/silverfish/endermite -> {:?}; zombie/player/cow/skeleton/creeper \
             -> {:?} — SpiderRenderer, SilverfishRenderer and EndermiteRenderer are the only \
             getFlipDegrees overrides in the client",
            over.iter().map(|&k| flip(k)).collect::<Vec<_>>(),
            normal.iter().map(|&k| flip(k)).collect::<Vec<_>>(),
        ),
    );

    // And the flip actually reaches the angle, not just the table.
    c.record(
        "d16.the_flip_override_doubles_the_rendered_angle",
        (rewo_gpu::entities::death_roll_for(K::Spider, 13.5) - std::f32::consts::PI).abs() < 1e-6,
        format!(
            "a fully toppled spider = {:.6} rad (want pi); a fully toppled zombie = {:.6}",
            rewo_gpu::entities::death_roll_for(K::Spider, 13.5),
            rewo_gpu::entities::death_roll_for(K::Zombie, 13.5),
        ),
    );
}

/// Render a mob's silhouette and return `(min_x, max_x, min_y, max_y, count)`
/// over the pixels that differ from the empty frame.
///
/// Used to prove the death topple reaches the geometry — the angle witnesses
/// only exercise the formula, and a rotation that was computed but never
/// applied would pass those while rendering an upright corpse.
fn silhouette(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &OverlayDraw,
    death_time: f32,
) -> Result<(u32, u32, u32, u32, u32), String> {
    let eye = Vec3::new(0.0, 1.0, 4.0);
    let dir = Vec3::new(0.0, 0.0, -1.0);
    let up = Vec3::Y;
    wr.set_camera(eye.to_array());
    let view = Mat4::look_to_rh(eye, dir, up);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(
        60f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    let view_proj = (proj * view).to_cols_array_2d();
    let right = dir.cross(up).normalize_or_zero().to_array();

    let mut d = EntityDraw {
        pos: [0.0, 0.0, 0.0],
        width: 1.0,
        height: 2.0,
        color: [1.0, 1.0, 1.0],
        name: None,
        // M59: no health bar in a still — the gate renders its own.
        health: None,
        kind: EntityModelKind::Capsule,
        yaw: 0.0,
        death_time,
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        bob_offset: 0.0,
        ground_seed: 0,
        ground_age: None,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held: [None, None],
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        skin_uv: None,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        light: [1.0, 1.0, 1.0],
        // M52: no emissive state, no pack variant, no dye — the
        // vanilla defaults, which is what this gate renders.
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: None,
    };
    d.kind = EntityModelKind::Zombie;

    // The empty frame, to subtract the sky.
    wr.set_entities(&[], right, up.to_array(), 0.0);
    off.render(gpu, Some((&mut *wr, view_proj)), draw, CLEAR)?;
    let empty = off.read_rgba(gpu)?;

    wr.set_entities(&[d], right, up.to_array(), 0.0);
    off.render(gpu, Some((&mut *wr, view_proj)), draw, CLEAR)?;
    let img = off.read_rgba(gpu)?;

    let (mut minx, mut maxx, mut miny, mut maxy, mut n) = (u32::MAX, 0u32, u32::MAX, 0u32, 0u32);
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            if img[i..i + 3] != empty[i..i + 3] {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
                n += 1;
            }
        }
    }
    if n == 0 {
        return Err("silhouette: the mob rendered no pixels at all".into());
    }
    Ok((minx, maxx, miny, maxy, n))
}

// -------------------------------------------------- M81: the camera tilt (e)

/// An independent transcription of `bobHurt`'s magnitude, in f64.
///
/// Deliberately **not** a call into `live_cmd::bob_hurt`: this is the number
/// the witnesses compare against, and reading the implementation for its own
/// expectation is how a gate stops testing anything.
fn oracle_tilt_degrees(hurt_time: f64, hurt_duration: f64, strength: f64) -> f64 {
    if hurt_time < 0.0 {
        return 0.0;
    }
    let x = hurt_time / hurt_duration;
    let hurt = (x * x * x * x * std::f64::consts::PI).sin();
    -hurt * 14.0 * strength
}

fn check_tilt(c: &mut Checker, ids: &Ids, classes: &EntityClasses, zombie: i32, player: i32) {
    use crate::live_cmd::{bob_hurt, HurtTilt};
    use rewo_net::route_hurt_animation;

    c.record(
        "e1.the_hurt_animation_id_resolves_and_is_distinct",
        ids.cb_play_hurt_animation != ids.cb_play_damage_event
            && ids.cb_play_hurt_animation != ids.cb_play_animate,
        format!(
            "hurt_animation={} vs damage_event={} vs animate={} \
             (two packets arm one clock; only one carries a direction)",
            ids.cb_play_hurt_animation, ids.cb_play_damage_event, ids.cb_play_animate
        ),
    );

    // **The local player is not in the entity table, and it is the only
    // recipient this packet ever has.** `ServerPlayer.indicateDamage` sends to
    // the victim alone, so the id is always your own — and Rewo's table holds
    // only entities the server sent an `add_entity` for, which it never does
    // for you.
    //
    // This witness exists because the first M81 build failed exactly here and
    // **the gate did not catch it**: every other row constructs a table
    // containing the id, which is a world this packet never arrives in. A live
    // run did. Written first, because it is the case that matters.
    //
    // MUTATION: dropping the `local_player` door leaves an *empty* table's
    // lookup to fail, and nothing at all happens — silently, which is how it
    // shipped for an afternoon.
    let mut t = EntityTable::default();
    route_hurt_animation(
        ids.cb_play_hurt_animation,
        &hurt_animation_body(4242, -90.0),
        ids,
        &mut t,
        Some(classes),
        Some(player),
        Some(4242),
    );
    c.record(
        "e2.the_local_player_is_reached_though_it_is_not_in_the_table",
        t.hurt_state(4242).hurt_time == 10 && (t.hurt_dir(4242) + 90.0).abs() < 1e-6,
        format!(
            "empty table, local id 4242: hurtTime={} hurtDir={} (want 10 / -90) — \
             a table-only lookup drops every packet this handler will ever see",
            t.hurt_state(4242).hurt_time,
            t.hurt_dir(4242)
        ),
    );

    // A *remote* player takes a directed hit: the clock arms AND the yaw is
    // stored, through the table.
    let mut t = table(1, player);
    let routed = route_hurt_animation(
        ids.cb_play_hurt_animation,
        &hurt_animation_body(1, 37.5),
        ids,
        &mut t,
        Some(classes),
        Some(player),
        None,
    );
    c.record(
        "e3.a_player_stores_both_the_clock_and_the_direction",
        routed && t.hurt_state(1).hurt_time == 10 && (t.hurt_dir(1) - 37.5).abs() < 1e-6,
        format!(
            "routed={routed} hurtTime={} hurtDir={} (want 10 / 37.5)",
            t.hurt_state(1).hurt_time,
            t.hurt_dir(1)
        ),
    );

    // MUTATION: the same packet on a **non-player** living entity. Vanilla's
    // `LivingEntity.animateHurt` takes the yaw and throws it away —
    // `getHurtDir()` is a flat `0.0F` — and only `Player` overrides. An
    // implementation that stored the yaw unconditionally passes e2 and fails
    // here.
    let mut t = table(1, zombie);
    route_hurt_animation(
        ids.cb_play_hurt_animation,
        &hurt_animation_body(1, 37.5),
        ids,
        &mut t,
        Some(classes),
        Some(player),
        None,
    );
    c.record(
        "e4.a_non_player_arms_the_clock_and_discards_the_yaw",
        t.hurt_state(1).hurt_time == 10 && t.hurt_dir(1) == 0.0,
        format!(
            "zombie: hurtTime={} hurtDir={} (want 10 / 0.0 — the override is on Player)",
            t.hurt_state(1).hurt_time,
            t.hurt_dir(1)
        ),
    );

    // MUTATION: an untracked id. `if (entity != null)` — inert, not an error.
    let mut t = table(1, player);
    route_hurt_animation(
        ids.cb_play_hurt_animation,
        &hurt_animation_body(99, 37.5),
        ids,
        &mut t,
        Some(classes),
        Some(player),
        None,
    );
    c.record(
        "e5.an_untracked_entity_is_inert",
        t.hurt_state(1).hurt_time == 0 && t.hurt_dir(1) == 0.0,
        format!(
            "hurtTime={} hurtDir={} for the tracked id after a packet for id 99",
            t.hurt_state(1).hurt_time,
            t.hurt_dir(1)
        ),
    );

    // The direction **outlives the clock**. `Player.hurtDir` is a plain field
    // that nothing resets, while `hurtTime` self-evicts at zero — so a hit
    // that arms the clock without a fresh direction (a blocked one:
    // `dealDefaultKnockback` calls `indicateDamage` only when `!blocked`)
    // tilts along the previous hit's.
    //
    // MUTATION: folding `hurt_dir` into `HurtState`, which self-evicts, zeroes
    // it here.
    let mut t = table(1, player);
    route_hurt_animation(
        ids.cb_play_hurt_animation,
        &hurt_animation_body(1, -120.0),
        ids,
        &mut t,
        Some(classes),
        Some(player),
        None,
    );
    for _ in 0..12 {
        t.tick_lerp();
    }
    let after = (t.hurt_state(1).hurt_time, t.hurt_dir(1));
    c.record(
        "e6.the_direction_outlives_the_clock",
        after.0 == 0 && (after.1 + 120.0).abs() < 1e-6,
        format!(
            "after 12 ticks: hurtTime={} hurtDir={} (the clock is spent, the direction is not)",
            after.0, after.1
        ),
    );

    // …but it dies with the entity, because vanilla's field is on the object.
    t.remove(1);
    t.add(1, EntityState::new(0, player, 0.0, 0.0, 0.0, 0.0, 0.0));
    c.record(
        "e7.a_recycled_id_inherits_no_direction",
        t.hurt_dir(1) == 0.0,
        format!("hurtDir={} after remove + re-add", t.hurt_dir(1)),
    );

    // The easing. `sin(x⁴·π)` with x = hurtTime/hurtDuration, and hurtTime
    // counts DOWN — so it is zero at the moment of the hit, peaks a fifth of a
    // second later, and returns to zero.
    //
    // MUTATION: a linear ramp, or `sin(x·π)`, peaks at x = 0.5 and is
    // symmetric; this asserts the peak is where the fourth power puts it.
    let sample = |x: f64| oracle_tilt_degrees(x * 10.0, 10.0, 1.0).abs();
    let peak_x = 0.5f64.powf(0.25);
    let at_peak = sample(peak_x);
    let (lo, hi) = (sample(0.5), sample(0.99));
    c.record(
        "e8.the_easing_peaks_at_the_fourth_root_of_a_half",
        (at_peak - 14.0).abs() < 1e-9 && lo < at_peak && hi < at_peak,
        format!(
            "x={peak_x:.4} → {at_peak:.4}°, x=0.5 → {lo:.4}°, x=0.99 → {hi:.4}° \
             (sin(x·π) would peak at x=0.5)"
        ),
    );

    // The implementation agrees with the independent transcription, at a
    // frame the tilt is actually moving.
    let probe = glam::Vec3::new(0.0, 1.0, -4.0);
    let tilt = HurtTilt {
        hurt_time: 8.41,
        hurt_duration: 10,
        hurt_dir: 0.0,
        strength: 1.0,
    };
    let want = oracle_tilt_degrees(8.41, 10.0, 1.0);
    let got = bob_hurt(tilt).transform_point3(probe);
    // A roll about +Z with `hurt_dir` 0: the probe swings in x by
    // `|p| · sin(θ)` where |p| is its distance from the roll axis.
    let want_x = (probe.y as f64) * -(want.to_radians()).sin();
    c.record(
        "e9.the_implementation_matches_the_independent_transcription",
        ((got.x as f64) - want_x).abs() < 1e-4,
        format!(
            "probe.x {} vs predicted {want_x:.6} for a {want:.4}° roll \
             (sampled off-axis: transform_point3(ZERO) would see nothing)",
            got.x
        ),
    );

    // MUTATION: `hurt_time` below zero must return the identity outright —
    // the render state is `hurtTime - partialTicks`, so it goes negative on
    // the frames after the clock expires.
    let flat = bob_hurt(HurtTilt {
        hurt_time: -0.25,
        ..tilt
    })
    .transform_point3(probe);
    c.record(
        "e10.a_negative_hurt_time_is_the_guard_not_a_wrap",
        (flat - probe).length() < 1e-6,
        format!("probe {probe:?} → {flat:?} at hurtTime -0.25"),
    );

    // The conjugation. `Ry(-rr)·Rz(θ)·Ry(rr)` rotates about `Ry(-rr)·ẑ`, so
    // `hurt_dir` selects the *plane*: 0° rolls the horizon, 90° pitches the
    // camera. Measured on a probe directly above the axis — a roll moves it
    // sideways, a pitch moves it forward/back and leaves x alone.
    //
    // MUTATION: dropping the two `Ry` factors (a plain roll regardless of
    // direction) makes both rows measure the same displacement.
    let roll = bob_hurt(HurtTilt {
        hurt_dir: 0.0,
        ..tilt
    })
    .transform_point3(probe);
    let pitch = bob_hurt(HurtTilt {
        hurt_dir: 90.0,
        ..tilt
    })
    .transform_point3(probe);
    let dx_roll = (roll.x - probe.x).abs();
    let dx_pitch = (pitch.x - probe.x).abs();
    let dz_pitch = (pitch.z - probe.z).abs();
    c.record(
        "e11.hurt_dir_selects_the_plane_rather_than_the_side",
        dx_roll > 0.15 && dx_pitch < 1e-4 && dz_pitch > 0.15,
        format!(
            "rr=0 moves x by {dx_roll:.4}; rr=90 moves x by {dx_pitch:.6} and z by \
             {dz_pitch:.4} — a conjugated roll, not a lean"
        ),
    );

    // And the two are opposite ends of the same axis rather than the same
    // rotation twice: 180° must mirror 0°.
    let back = bob_hurt(HurtTilt {
        hurt_dir: 180.0,
        ..tilt
    })
    .transform_point3(probe);
    c.record(
        "e12.the_direction_is_signed",
        ((back.x + roll.x) as f64).abs() < 1e-4 && (back.x - roll.x).abs() > 0.15,
        format!("rr=0 → x {}, rr=180 → x {} (mirrored)", roll.x, back.x),
    );

    // The module. M52a recorded `no_damage_tilt` as vacuous "until Rewo
    // ships the vanilla behaviour it suppresses" — this is the packet that
    // shipped it.
    //
    // MUTATION: an implementation that branched around the tilt instead of
    // driving `damageTiltStrength` to 0 would also pass the second half; the
    // first half is what pins it to vanilla's own accessibility knob.
    let mut m = crate::modules::Modules::from_defaults();
    let before = m.render().damage_tilt_strength;
    m.toggle("no_damage_tilt");
    let after = m.render().damage_tilt_strength;
    let off = bob_hurt(HurtTilt {
        strength: after,
        ..tilt
    })
    .transform_point3(probe);
    // The **duration** half of the guard, which the `hurt_time < 0` half does
    // not cover and which a mutation battery showed is load-bearing.
    //
    // An entity that has never been hurt has `HurtState::default()` — clock 0
    // *and* duration 0 — and the render state is `hurtTime - partialTicks`, so
    // on the one frame where the partial tick is exactly 0 the numerator is 0
    // too. `0.0 / 0.0` is NaN, `sin(NaN)` is NaN, and a NaN rotation poisons
    // the whole view-projection: every vertex in the frame vanishes.
    //
    // Vanilla cannot reach this — `hurtDuration` is only ever written together
    // with `hurtTime` — so the guard is Rewo's, not a transcription, and it is
    // recorded as such.
    //
    // MUTATION: dropping `|| t.hurt_duration == 0` makes every component NaN.
    let unhurt = bob_hurt(HurtTilt {
        hurt_time: 0.0,
        hurt_duration: 0,
        hurt_dir: 0.0,
        strength: 1.0,
    })
    .transform_point3(probe);
    c.record(
        "e13.an_unhurt_entity_at_partial_zero_does_not_divide_by_zero",
        unhurt.is_finite() && (unhurt - probe).length() < 1e-6,
        format!(
            "hurtTime 0 / hurtDuration 0 → probe {unhurt:?} (finite, unmoved); \
             without the duration guard this is NaN and the frame renders nothing"
        ),
    );

    c.record(
        "e14.no_damage_tilt_is_a_real_module_now",
        before == crate::modules::VANILLA_DAMAGE_TILT
            && after == 0.0
            && (off - probe).length() < 1e-6,
        format!(
            "damageTiltStrength {before} → {after}; probe unmoved at {off:?} \
             (M52a's vacuity note is closed)"
        ),
    );
}

fn check_flash(c: &mut Checker) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let ring = OverlayRing::default();
    // The overlay strip parked off-screen: the entity pass is what is under
    // test, and a visible FPS chart would pollute the brightest-texel search.
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let white: Vec<u8> = vec![255u8; (TEX * TEX * 4) as usize];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, TEX, &layers)?;
    wr.init_entities(&mut gpu, None, MobTextures::default())?;

    // A deliberately non-grey light so a channel swap or a dropped light
    // multiply cannot pass: R > G > B, none saturated.
    let light = [0.80f32, 0.55, 0.30];
    let cold = render_capsule(&mut gpu, &mut off, &mut wr, &draw, light, false)?;
    let hot = render_capsule(&mut gpu, &mut off, &mut wr, &draw, light, true)?;

    // Predict `hot` FROM `cold`: undo the light, encode to sRGB, mix vanilla's
    // red overlay texel, decode, re-apply the light. This never needs to know
    // the face shade — and it is only correct if the shader mixes in sRGB
    // space and does it *before* the lightmap multiply.
    let mut want = [0u8; 3];
    for i in 0..3 {
        let lin = srgb_to_linear(cold[i] as f32 / 255.0);
        let base = lin / light[i];
        let s = linear_to_srgb(base);
        let mixed = 1.0 * (1.0 - HURT_ALPHA) + s * HURT_ALPHA;
        // `mix(overlay.rgb, color.rgb, overlay.a)`; overlay.rgb is (1,0,0), so
        // green and blue mix toward 0 while red mixes toward 1.
        let mixed = if i == 0 { mixed } else { s * HURT_ALPHA };
        let out = srgb_to_linear(mixed) * light[i];
        want[i] = (linear_to_srgb(out) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    let close = (0..3).all(|i| (hot[i] as i32 - want[i] as i32).abs() <= CHANNEL_TOL);
    c.record(
        "c1.the_hurt_flash_is_the_exact_overlay_mix",
        close,
        format!(
            "unhurt {cold:?} → hurt {hot:?}, predicted {want:?} (mix(red, c, {HURT_ALPHA:.4}) \
             in sRGB space, before the lightmap)"
        ),
    );
    c.record(
        "c2.the_flash_actually_reddens",
        hot[0] > cold[0] && hot[1] < cold[1] && hot[2] < cold[2],
        format!(
            "R {}→{} (up), G {}→{} (down), B {}→{} (down)",
            cold[0], hot[0], cold[1], hot[1], cold[2], hot[2]
        ),
    );
    // Sensitivity: mixing in LINEAR space instead of sRGB would land here, and
    // must be measurably different from what the GPU produced.
    let mut linear_mix = [0u8; 3];
    for i in 0..3 {
        let lin = srgb_to_linear(cold[i] as f32 / 255.0);
        let base = lin / light[i];
        let red = if i == 0 { 1.0 } else { 0.0 };
        let mixed = red * (1.0 - HURT_ALPHA) + base * HURT_ALPHA;
        linear_mix[i] = (linear_to_srgb(mixed * light[i]) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    let differs = (0..3).any(|i| (linear_mix[i] as i32 - hot[i] as i32).abs() > CHANNEL_TOL);
    c.record(
        "c3.the_mix_is_not_done_in_linear_space",
        differs,
        format!(
            "a linear-space mix would give {linear_mix:?}, the GPU gave {hot:?} — the \
             transfer function is load-bearing"
        ),
    );
    // Sensitivity: applying the overlay AFTER the lightmap would land here.
    let mut after_light = [0u8; 3];
    for i in 0..3 {
        let s = cold[i] as f32 / 255.0;
        let red = if i == 0 { 1.0 } else { 0.0 };
        let mixed = red * (1.0 - HURT_ALPHA) + s * HURT_ALPHA;
        after_light[i] = (mixed * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    let differs2 = (0..3).any(|i| (after_light[i] as i32 - hot[i] as i32).abs() > CHANNEL_TOL);
    c.record(
        "c4.the_overlay_is_applied_before_the_lightmap",
        differs2,
        format!(
            "mixing after the lightmap would give {after_light:?}, the GPU gave {hot:?} — \
             a hurt mob in the dark must not flash at full brightness"
        ),
    );
    // And an unhurt entity is untouched by the overlay path at all: the
    // no-overlay texel is white with alpha 1, so the mix is the identity.
    let cold2 = render_capsule(&mut gpu, &mut off, &mut wr, &draw, light, false)?;
    c.record(
        "c5.an_unhurt_entity_is_bit_identical_across_renders",
        cold == cold2,
        format!("unhurt rendered twice: {cold:?} == {cold2:?}"),
    );
    // --- M24: the topple must reach the geometry -------------------------
    let alive = silhouette(&mut gpu, &mut off, &mut wr, &draw, 0.0)?;
    let toppled = silhouette(&mut gpu, &mut off, &mut wr, &draw, 13.5)?;
    let (aw, ah) = (alive.1 - alive.0, alive.3 - alive.2);
    let (tw, th) = (toppled.1 - toppled.0, toppled.3 - toppled.2);
    c.record(
        "d17.a_fully_toppled_mob_renders_lying_down",
        aw < ah && tw > th,
        format!(
            "alive silhouette {aw}x{ah} (upright: taller than wide); toppled {tw}x{th} \
             (wider than tall) — the 90 deg roll is applied to the vertices, not merely \
             computed"
        ),
    );
    c.record(
        "d18.the_topple_is_a_rotation_not_a_resize",
        (aw.max(ah) as i32 - tw.max(th) as i32).abs() <= 2
            && (aw.min(ah) as i32 - tw.min(th) as i32).abs() <= 2,
        format!(
            "long axis {} -> {}, short axis {} -> {} — a rotation preserves both extents; \
             a scale or a translate would not",
            aw.max(ah),
            tw.max(th),
            aw.min(ah),
            tw.min(th)
        ),
    );
    let half = silhouette(&mut gpu, &mut off, &mut wr, &draw, 1.5)?;
    let (hw, hh) = (half.1 - half.0, half.3 - half.2);
    c.record(
        "d19.the_topple_is_progressive",
        hw > aw && hw < tw && hh < ah && hh > th,
        format!(
            "at deathTime 1.5 the silhouette is {hw}x{hh}, strictly between the upright \
             {aw}x{ah} and the toppled {tw}x{th} — the sqrt curve is sampled, not stepped"
        ),
    );
    let unmoved = silhouette(&mut gpu, &mut off, &mut wr, &draw, 1.0)?;
    c.record(
        "d20.death_time_one_has_not_toppled_yet",
        (unmoved.1 - unmoved.0) == aw && (unmoved.3 - unmoved.2) == ah,
        format!(
            "deathTime exactly 1.0 renders {}x{} — identical to alive, because \
             (1-1)/20*1.6 is 0 and the fall starts from zero rather than jumping",
            unmoved.1 - unmoved.0,
            unmoved.3 - unmoved.2
        ),
    );

    // --- M81: the tilt has to reach the frame ----------------------------
    let tilt = check_tilt_pixels(c, &mut gpu, &mut off, &mut wr, &draw);

    // Tear the GPU objects down explicitly: dropping the device with live
    // pipelines is a validation error, and this gate runs with layers on.
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);

    tilt
}

/// The centroid of everything that differs from an empty frame, in **NDC**
/// (x right, y up, origin at the principal point).
///
/// NDC rather than pixels because the viewport is y-flipped: measuring a
/// rotation's sign in pixel space would measure the flip as well.
fn marker_ndc(img: &[u8], empty: &[u8]) -> Option<(f64, f64, u32)> {
    let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0u32);
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            if img[i..i + 3] != empty[i..i + 3] {
                sx += x as f64 + 0.5;
                sy += y as f64 + 0.5;
                n += 1;
            }
        }
    }
    if n == 0 {
        return None;
    }
    let (px, py) = (sx / n as f64, sy / n as f64);
    Some((
        px / W as f64 * 2.0 - 1.0,
        1.0 - py / H as f64 * 2.0,
        n,
    ))
}

/// The tilt, measured on rendered pixels through the production emitter.
///
/// **The subject is a synthetic magenta capsule against a black sky**, and the
/// detector is a difference against an empty frame — the shape `handshot`
/// settled on after three detector errors, each of which looked for a signal
/// against a background that already contained it. Two further guards:
/// the marker's *count* is asserted stable across the tilted and untilted
/// frames (a detector that started counting something else would move it), and
/// the empty frame is asserted to contain nothing.
///
/// What is measured is the marker's **polar angle about the principal point**,
/// not a pixel count: a camera tilt moves the whole frame, so any count-based
/// detector changes for reasons unrelated to the angle. The frame is square,
/// so a `Rz` between the projection and the view is exactly a rotation of the
/// NDC image about its origin — which makes the angle directly comparable with
/// the transcription in [`oracle_tilt_degrees`].
fn check_tilt_pixels(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &OverlayDraw,
) -> Result<(), String> {
    use crate::live_cmd::{eye_view_proj_hurt, HurtTilt};

    // A magenta capsule up and to the right of the eye's forward axis, far
    // enough off-centre that a 14 degree roll is tens of pixels and close
    // enough in that the roll cannot carry it off the frame.
    let marker = || EntityDraw {
        pos: [1.0, 1.0, -4.0],
        width: 0.5,
        height: 0.5,
        color: [1.0, 0.0, 1.0],
        ..render_marker_template()
    };
    let eye = Vec3::new(0.0, 0.0, 0.0);
    let up = Vec3::Y.to_array();
    let right = [1.0, 0.0, 0.0];
    wr.set_camera(eye.to_array());

    // `eye_view_proj_hurt` is the production emitter: yaw 0 faces +Z, so the
    // camera is turned 180 degrees to look at the marker down -Z.
    let vp = |t: HurtTilt| eye_view_proj_hurt(eye, 180.0, 0.0, 1.0, 60.0, t);

    // **The background is re-rendered at every tilt.** A camera tilt turns the
    // sky gradient with it, so differencing a tilted frame against an
    // *untilted* empty one measures the whole sky as "the marker" — 13,119
    // pixels of it, on the first run of this witness. That is the same
    // detector error `handshot` hit three times: a signal looked for against a
    // background that already contained it. The subject is isolated by
    // comparing each frame only with its own background.
    let mut shot = |t: HurtTilt| -> Result<(Vec<u8>, Option<(f64, f64, u32)>), String> {
        wr.set_entities(&[], right, up, 0.0);
        off.render(gpu, Some((&mut *wr, vp(t))), draw, CLEAR)?;
        let empty = off.read_rgba(gpu)?;
        wr.set_entities(&[marker()], right, up, 0.0);
        off.render(gpu, Some((&mut *wr, vp(t))), draw, CLEAR)?;
        let img = off.read_rgba(gpu)?;
        let m = marker_ndc(&img, &empty);
        Ok((img, m))
    };

    let (flat, flat_marker) = shot(HurtTilt::NONE)?;
    let Some((fx, fy, fn_)) = flat_marker else {
        return Err("hurtshot: the tilt marker rendered no pixels at all".into());
    };
    c.record(
        "e15.the_marker_is_present_and_off_centre",
        fn_ > 40 && (fx * fx + fy * fy).sqrt() > 0.3,
        format!(
            "{fn_} pixels at NDC ({fx:.4}, {fy:.4}), radius {:.4} — an empty frame \
             contains none of them, so the detector cannot be measuring the sky",
            (fx * fx + fy * fy).sqrt()
        ),
    );

    // The peak frame of a full-strength tilt, `hurt_dir = 0` (a pure roll).
    let peak = HurtTilt {
        hurt_time: 8.41,
        hurt_duration: 10,
        hurt_dir: 0.0,
        strength: 1.0,
    };
    let Some((rx, ry, rn)) = shot(peak)?.1 else {
        return Err("hurtshot: the tilted marker rendered no pixels".into());
    };
    let angle = ry.atan2(rx).to_degrees() - fy.atan2(fx).to_degrees();
    let want = oracle_tilt_degrees(8.41, 10.0, 1.0);
    c.record(
        "e16.the_rendered_roll_is_the_transcribed_angle",
        (angle - want).abs() < 2.0 && (rn as i32 - fn_ as i32).abs() < (fn_ as i32) / 3,
        format!(
            "the marker swung {angle:.3} degrees about the principal point; \
             bobHurt predicts {want:.3} ({rn} pixels vs {fn_} — the subject did not \
             change, only its position)"
        ),
    );

    // MUTATION: `hurt_dir = 90` is a **pitch**, not a mirrored roll — and the
    // property that separates them is the marker's **radius** from the
    // principal point, not its x. A roll is a rotation of the NDC image about
    // its origin and preserves the radius exactly; a pitch moves the marker
    // through the perspective divide, so its radius changes.
    //
    // The first version of this witness asserted a pitch leaves x alone. It
    // does not: rotating the marker about the camera's X axis changes its
    // depth, and 1/w scales x with it — measured, 0.437 → 0.474. The
    // measurement was right and the expectation was wrong.
    //
    // An implementation that dropped the two `Ry` conjugation factors rolls
    // regardless of direction, preserving the radius in both rows, and fails
    // the second half while passing e16.
    let Some((px, py, _)) = shot(HurtTilt {
        hurt_dir: 90.0,
        ..peak
    })?
    .1
    else {
        return Err("hurtshot: the pitched marker rendered no pixels".into());
    };
    let radius = |x: f64, y: f64| (x * x + y * y).sqrt();
    let (r_flat, r_roll, r_pitch) = (radius(fx, fy), radius(rx, ry), radius(px, py));
    c.record(
        "e17.a_roll_preserves_the_radius_and_a_pitch_does_not",
        (r_roll - r_flat).abs() < 0.02 && (r_pitch - r_flat).abs() > 0.2,
        format!(
            "radius: untilted {r_flat:.4}, rr=0 {r_roll:.4} (a rotation about the \
             principal point), rr=90 {r_pitch:.4} at NDC ({px:.4}, {py:.4}) — the \
             conjugation puts the roll axis somewhere other than the view axis"
        ),
    );

    // MUTATION: the module. `no_damage_tilt` drives `damageTiltStrength` to
    // zero, and the rendered frame must then be **byte-identical** to the
    // untilted one — not merely close.
    let strength = {
        let mut m = crate::modules::Modules::from_defaults();
        m.toggle("no_damage_tilt");
        m.render().damage_tilt_strength
    };
    let (suppressed, _) = shot(HurtTilt { strength, ..peak })?;
    c.record(
        "e18.the_module_suppresses_the_tilt_at_the_pixel",
        suppressed == flat,
        format!(
            "with damageTiltStrength {strength}, the peak frame is {} the untilted one",
            if suppressed == flat {
                "byte-identical to"
            } else {
                "DIFFERENT from"
            }
        ),
    );

    Ok(())
}

/// A neutral `EntityDraw` for the tilt marker — everything but position, size
/// and colour left at rest.
fn render_marker_template() -> EntityDraw<'static> {
    EntityDraw {
        pos: [0.0; 3],
        width: 1.0,
        height: 1.0,
        color: [1.0, 0.0, 1.0],
        name: None,
        health: None,
        kind: EntityModelKind::Capsule,
        yaw: 0.0,
        death_time: 0.0,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held: [None, None],
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        ground_seed: 0,
        ground_age: None,
        bob_offset: 0.0,
        skin_uv: None,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        light: [1.0, 1.0, 1.0],
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: None,
    }
}
