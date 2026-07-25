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
const EXPECTED_WITNESSES: usize = 18;

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
        kind: EntityModelKind::Capsule,
        yaw: 0.0,
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
        anim_id: 0.0,
        light,
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
    // Tear the GPU objects down explicitly: dropping the device with live
    // pipelines is a validation error, and this gate runs with layers on.
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}
