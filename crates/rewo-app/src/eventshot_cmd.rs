//! `rewo eventshot` — M17's permanent **serverless** entity-event oracle.
//!
//! `ClientboundEntityEventPacket` carries the three model-visible one-shot
//! animations (warden attack id 4, warden sonic boom id 62, armadillo peek id
//! 64). This gate proves the whole continuous production path with no socket
//! and no GPU device:
//!
//! ```text
//! raw BE-i32 id + signed-byte event      (built here, independent of any writer)
//!   -> rewo_net::route_entity_event       (real packet-id selection seam)
//!   -> EntityTable::start_event           (receipt-tick side table + lifecycle)
//!   -> live_cmd::resolve_mob_anim         (vanilla ownership rules, event ages)
//!   -> rewo_gpu::entities::oracle_part_deltas   (the exact renderer rig math)
//! ```
//!
//! **Fail-closed by construction.** A fixed [`EXPECTED_WITNESSES`] count is set
//! to the number of named properties. Each property is *observed* (the real
//! value is read and printed) and only increments the counter when it actually
//! passes; a mismatch is gathered into a failures list instead of panicking, so
//! every property is reported, not just the first bad one. The run errors if
//! **either** any property failed **or** the observed count differs from
//! `EXPECTED_WITNESSES` — the latter catches a property that was silently
//! skipped (a missing part, an oracle that returned `None`) even when the
//! failures list is empty.
//!
//! **The expected values are independent literals.** Every target is a
//! hand-written decompiled-vanilla number with a source comment; nothing here
//! reads `anim_defs` as its expectation (that is the table the renderer reads —
//! grading it against itself would verify nothing). The catmull-rom target is
//! recomputed by a private reimplementation of `Mth.catmullrom` over the four
//! decompiled frame literals.
//!
//! **Every property carries a mutation/sensitivity partner.** The wrong-packet
//! id case proves id selection is load-bearing; the neutral-isolation case
//! proves the deflections come from the rig and not the base pose; the
//! catmull-rom case asserts the value sits *far* from the linear interpolant
//! (so a lerp regression fails); the roar-ownership case flips suppression by
//! reordering ticks; the armadillo case distinguishes a held pose from a fresh
//! restart in an actual part delta.
//!
//! **Tolerances are tight — they only absorb time reconstruction.** A keyframe's
//! decimal time (e.g. `0.1667 s = 3.334` ticks) cannot land on an integer tick,
//! so the event age reconstructed through the real `ageInTicks =
//! (tick − start + partial) · 0.05` clock — plus f32 rounding of the stored
//! keyframe times — drifts only ~1e-6 from the exact decompiled target. Keyframe
//! rotations/positions and the catmull-rom value are therefore graded at ~1e-4
//! (the armadillo head position.z at ~1e-3, where a near-zero restart age
//! interpolates a hair off its keyframe). The far-from-linear check (`> 0.05`)
//! is a separate *sensitivity* discriminator, not an accuracy bound; the
//! expected numbers themselves are exact.

use clap::Args as ClapArgs;
use rewo_data::{entity_types::EntityTypes, packets::Packets, DataPaths};
use rewo_gpu::entities::{oracle_part_deltas, OracleInputs};
use rewo_gpu::mobs::{EntityModelKind, Gesture, ModelEvent};
use rewo_net::{ids::Ids, route_entity_event};
use rewo_world::entities::{EntityEvent, EntityState, EntityTable};

use crate::live_cmd::{resolve_mob_anim, GestureTracker};

/// Total number of named properties this gate asserts. Locked so a skipped
/// property (fewer observations) fails the run even if nothing mismatched.
const EXPECTED_WITNESSES: usize = 28;

/// The production entity-type protocol ids for the two kinds whose entity events
/// this client models. Resolved once in [`run`] from the pinned version's
/// `registries.json` through the exact `EntityTypes::id_of` path that
/// `live_cmd`/`play_cmd` use — no synthetic ids leak into any positive check.
/// Read by the check helpers via [`warden_tid`] / [`armadillo_tid`], held in a
/// process-local cell so proving the real activation path needs no signature
/// churn on the check functions.
struct EventKinds {
    warden: i32,
    armadillo: i32,
}

static KINDS: std::sync::OnceLock<EventKinds> = std::sync::OnceLock::new();

fn kinds() -> &'static EventKinds {
    KINDS
        .get()
        .expect("EventKinds resolved by run() before any check runs")
}

fn warden_tid() -> i32 {
    kinds().warden
}

fn armadillo_tid() -> i32 {
    kinds().armadillo
}

/// A fixed base receipt tick for the rig-timing checks; ages are reconstructed
/// relative to it through [`split_age`].
const START_TICK: i64 = 1_000;

#[derive(ClapArgs)]
pub struct EventshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `dimensioncheck`/`meshshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Version whose `packets.json` datagen report resolves the real
    /// `entity_event` packet id.
    #[arg(long, default_value = "26.2")]
    version: String,
}

pub fn run(args: EventshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[eventshot] mode: {mode} (serverless; the oracle asserts unconditionally — a \
         failure exits nonzero with or without --check)"
    );

    // The real resolved `ClientboundEntityEventPacket` id — the dispatch seam
    // keys off it. Loaded from the packets datagen report only (no jar, no
    // GPU): this is the production id, not a fabricated one. `block_update` is
    // a *different real* clientbound-play id, used to prove `route_entity_event`
    // selects on the id and not merely on the body shape.
    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let ev_id = ids.cb_play_entity_event;
    let wrong_id = ids.cb_play_block_update;
    if wrong_id == ev_id {
        return Err("entity_event and block_update resolved to the same id".into());
    }
    println!("[eventshot] entity_event id = {ev_id}, control id (block_update) = {wrong_id}");

    let mut c = Checker::new();

    // Resolve the real warden/armadillo protocol ids through the exact
    // production path (`EntityTypes::id_of` on `registries.json`, the same call
    // `live_cmd`/`play_cmd` make). This proves the id-of activation seam, not a
    // fabricated trio: a missing type, or the two colliding, fails closed here
    // — before any positive check runs — and the resolved ids are what every
    // entity creation and route call below use.
    let entity_types = EntityTypes::load(&paths.registries_json())?;
    let warden = entity_types.id_of("minecraft:warden");
    let armadillo = entity_types.id_of("minecraft:armadillo");
    let resolved = match (warden, armadillo) {
        (Some(w), Some(a)) if w != a => Some((w, a)),
        _ => None,
    };
    c.record(
        "c0.production_type_ids_resolved",
        resolved.is_some(),
        format!(
            "warden={warden:?} armadillo={armadillo:?} \
             (want two distinct Some via EntityTypes::id_of)"
        ),
    );
    let Some((warden, armadillo)) = resolved else {
        return Err(format!(
            "EntityTypes::id_of failed to resolve distinct warden/armadillo ids \
             (warden={warden:?} armadillo={armadillo:?})"
        ));
    };
    KINDS.set(EventKinds { warden, armadillo }).ok();
    println!(
        "[eventshot] entity types (via EntityTypes::id_of): warden = {warden}, armadillo = {armadillo}"
    );

    check_dispatch(&mut c, &ids, ev_id, wrong_id);
    check_lifecycle(&mut c, &ids, ev_id);
    check_warden_attack(&mut c, &ids, ev_id);
    check_warden_sonic(&mut c, &ids, ev_id);
    check_roar_ownership(&mut c, &ids, ev_id);
    check_armadillo_scared(&mut c, &ids, ev_id);
    check_neutral_ribcage(&mut c);

    println!(
        "[eventshot] witnesses observed: {} / {}",
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
            "witness count {} != expected {} — a named property was skipped (fail-closed)",
            c.witnessed, EXPECTED_WITNESSES
        ));
    }
    println!("[eventshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// --------------------------------------------------------------- accumulator

/// Gathers named observations. Each property increments `witnessed` only on a
/// real pass; a failure is recorded (no increment) so the run reports every bad
/// property and the count-vs-`EXPECTED_WITNESSES` guard catches skips.
struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn new() -> Self {
        Self {
            witnessed: 0,
            failures: Vec::new(),
        }
    }

    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        let status = if pass { " ok " } else { "FAIL" };
        println!("[eventshot] {status}  {name}: {detail}");
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

// ------------------------------------------------------------------- helpers

/// `ClientboundEntityEventPacket` wire form (decompiled): a signed big-endian
/// i32 entity id followed by a signed byte event — neither a VarInt. Built here
/// independently of `rewo_net`'s own writer.
fn body(entity: i32, event: i8) -> Vec<u8> {
    let mut b = entity.to_be_bytes().to_vec();
    b.push(event as u8);
    b
}

/// Add a bare entity of `type_id` at the origin.
fn add(t: &mut EntityTable, id: i32, type_id: i32) {
    t.add(id, EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0));
}

/// Reconstruct an event age (seconds) as `(Δtick, partial-tick alpha)` so the
/// dispatch → `resolve_mob_anim` path reproduces it through the real
/// `ageInTicks = (tick − start + partial) · 0.05` clock.
fn split_age(age: f32) -> (i64, f32) {
    let ticks = age / 0.05;
    let whole = ticks.floor();
    (whole as i64, ticks - whole)
}

/// Vanilla `Mth.catmullrom`, reimplemented independently of the renderer so the
/// oracle output is graded against a value this checker computes rather than the
/// same code path it exercises.
fn catmull(a: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    0.5 * (2.0 * p1
        + (p2 - p0) * a
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * a * a
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * a * a * a)
}

/// `shell` follows `live_cmd::collect_entities`: the armadillo hides in its
/// shell (walk yields) while SCARED, and part-way through roll/unroll.
fn shell_for(gesture: Option<(Gesture, f32)>) -> bool {
    match gesture {
        Some((Gesture::ArmadilloRoll, age)) => age > 0.25,
        Some((Gesture::ArmadilloScared, _)) => true,
        Some((Gesture::ArmadilloUnroll, age)) => age < 1.3,
        _ => false,
    }
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn near3(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    near(a[0], b[0], tol) && near(a[1], b[1], tol) && near(a[2], b[2], tol)
}

fn fmt3(v: [f32; 3]) -> String {
    format!("[{:+.6}, {:+.6}, {:+.6}]", v[0], v[1], v[2])
}

/// A named part's `(rot delta, pos delta)` in the oracle output, or `None` if
/// the model has no such part.
fn find(deltas: &[(&'static str, [f32; 3], [f32; 3])], name: &str) -> Option<([f32; 3], [f32; 3])> {
    deltas
        .iter()
        .find(|&&(n, _, _)| n == name)
        .map(|&(_, r, o)| (r, o))
}

/// The full production path for a warden wire event at `age` seconds: dispatch
/// the packet, resolve it through `resolve_mob_anim`, and run the GPU rig oracle
/// at the reconstructed age — no device. Pose/state are neutral (no gesture),
/// look/limb are neutral, so the only nonzero deltas are the event rig's.
fn warden_event_deltas(
    ids: &Ids,
    ev: i32,
    event_byte: i8,
    age: f32,
) -> Vec<(&'static str, [f32; 3], [f32; 3])> {
    let mut t = EntityTable::default();
    add(&mut t, 1, warden_tid());
    route_entity_event(
        ev,
        &body(1, event_byte),
        ids,
        &mut t,
        Some(warden_tid()),
        Some(armadillo_tid()),
        START_TICK,
        None,
    );
    let (dtick, alpha) = split_age(age);
    let mut gestures = GestureTracker::default();
    let (gesture, events) = resolve_mob_anim(
        EntityModelKind::Warden,
        0, // neutral pose → no gesture
        0, // neutral state
        t.event_start(1, EntityEvent::WardenAttack),
        t.event_start(1, EntityEvent::WardenSonicBoom),
        t.event_start(1, EntityEvent::ArmadilloPeek),
        &mut gestures,
        1,
        0.0,
        START_TICK + dtick,
        alpha,
    );
    let inputs = OracleInputs {
        gesture,
        events,
        ..Default::default()
    };
    oracle_part_deltas(EntityModelKind::Warden, &inputs).expect("warden has a built-in model")
}

/// Sample the armadillo's built-in model with a resolved SCARED gesture — the
/// PEEK rig's contribution at the gesture's current age.
fn armadillo_scared_deltas(
    gesture: Option<(Gesture, f32)>,
) -> Vec<(&'static str, [f32; 3], [f32; 3])> {
    let inputs = OracleInputs {
        gesture,
        shell: shell_for(gesture),
        ..Default::default()
    };
    oracle_part_deltas(EntityModelKind::Armadillo, &inputs).expect("armadillo has a built-in model")
}

// --------------------------------------------------------- 1. packet dispatch

fn check_dispatch(c: &mut Checker, ids: &Ids, ev: i32, wrong: i32) {
    // Warden 4 / 62 and armadillo 64 map through the correct packet id.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        let m = route_entity_event(
            ev,
            &body(1, 4),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            10,
            None,
        );
        let got = t.event_start(1, EntityEvent::WardenAttack);
        c.record(
            "c1.warden_attack_maps",
            m && got == Some(10),
            format!("matched={m} start={got:?} (want matched=true start=Some(10))"),
        );
    }
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        let m = route_entity_event(
            ev,
            &body(1, 62),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            11,
            None,
        );
        let got = t.event_start(1, EntityEvent::WardenSonicBoom);
        c.record(
            "c1.warden_sonic_maps",
            m && got == Some(11),
            format!("matched={m} start={got:?} (want Some(11))"),
        );
    }
    {
        let mut t = EntityTable::default();
        add(&mut t, 2, armadillo_tid());
        let m = route_entity_event(
            ev,
            &body(2, 64),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            12,
            None,
        );
        let got = t.event_start(2, EntityEvent::ArmadilloPeek);
        c.record(
            "c1.armadillo_peek_maps",
            m && got == Some(12),
            format!("matched={m} start={got:?} (want Some(12))"),
        );
    }
    // Sensitivity: the WRONG packet id must not dispatch — `route_entity_event`
    // returns false and nothing is recorded (id selection is load-bearing).
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        let m = route_entity_event(
            wrong,
            &body(1, 4),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            20,
            None,
        );
        let got = t.event_start(1, EntityEvent::WardenAttack);
        c.record(
            "c1.wrong_packet_id_inert",
            !m && got.is_none(),
            format!("matched={m} start={got:?} (want matched=false start=None)"),
        );
    }
    // Missing entity: the packet id matches but there is nothing to attach to.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        let m = route_entity_event(
            ev,
            &body(999, 4),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            21,
            None,
        );
        let got = t.event_start(999, EntityEvent::WardenAttack);
        c.record(
            "c1.missing_entity_inert",
            m && got.is_none(),
            format!("matched={m} start={got:?} (want matched=true start=None)"),
        );
    }
    // Wrong kind: id 4 on an armadillo and id 64 on a warden are both polymorphic
    // no-ops — the *(kind, byte)* pair names the animation, not the byte alone.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        add(&mut t, 2, armadillo_tid());
        route_entity_event(
            ev,
            &body(2, 4),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            22,
            None,
        );
        route_entity_event(
            ev,
            &body(1, 64),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            22,
            None,
        );
        let a = t.event_start(2, EntityEvent::WardenAttack);
        let p = t.event_start(1, EntityEvent::ArmadilloPeek);
        c.record(
            "c1.wrong_kind_inert",
            a.is_none() && p.is_none(),
            format!("armadillo.attack={a:?} warden.peek={p:?} (want both None)"),
        );
    }
    // An unmodelled event byte (hurt, particles, ...) records nothing.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        route_entity_event(
            ev,
            &body(1, 99),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            23,
            None,
        );
        let a = t.event_start(1, EntityEvent::WardenAttack);
        let s = t.event_start(1, EntityEvent::WardenSonicBoom);
        c.record(
            "c1.unknown_event_inert",
            a.is_none() && s.is_none(),
            format!("attack={a:?} sonic={s:?} (want both None)"),
        );
    }
    // The warden tendril-shiver status (id 61) maps to its OWN slot (M52) and
    // must not touch either keyframe rig. `Warden.handleEntityEvent` is an
    // if/else-if chain, so 61 reaching the attack or sonic-boom
    // `AnimationState` would mean the dispatch had fallen through.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        route_entity_event(
            ev,
            &body(1, 61),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            24,
            None,
        );
        let a = t.event_start(1, EntityEvent::WardenAttack);
        let s = t.event_start(1, EntityEvent::WardenSonicBoom);
        let d = t.event_start(1, EntityEvent::WardenTendril);
        c.record(
            "c1.tendril_event_stamps_only_the_tendril",
            a.is_none() && s.is_none() && d == Some(24),
            format!("attack={a:?} sonic={s:?} tendril={d:?} (want None, None, Some(24))"),
        );
    }
}

// ------------------------------------------------------- 2. receipt lifecycle

fn check_lifecycle(c: &mut Checker, ids: &Ids, ev: i32) {
    // A repeated receipt is an unconditional restart (dispatch, not start_event).
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        route_entity_event(
            ev,
            &body(1, 4),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            10,
            None,
        );
        route_entity_event(
            ev,
            &body(1, 4),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            55,
            None,
        );
        let got = t.event_start(1, EntityEvent::WardenAttack);
        c.record(
            "c2.repeated_dispatch_restarts",
            got == Some(55),
            format!("start={got:?} (want Some(55), the later receipt)"),
        );
    }
    // Removal clears the slot.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        route_entity_event(
            ev,
            &body(1, 4),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            30,
            None,
        );
        let before = t.event_start(1, EntityEvent::WardenAttack);
        t.remove(1);
        let after = t.event_start(1, EntityEvent::WardenAttack);
        c.record(
            "c2.removal_clears",
            before == Some(30) && after.is_none(),
            format!("before={before:?} after_remove={after:?} (want Some(30) then None)"),
        );
    }
    // Re-adding the same id (a dropped despawn, then a new occupant) drops the
    // stale animation clock — vanilla's AnimationStates die with the entity.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, warden_tid());
        route_entity_event(
            ev,
            &body(1, 4),
            ids,
            &mut t,
            Some(warden_tid()),
            Some(armadillo_tid()),
            40,
            None,
        );
        let before = t.event_start(1, EntityEvent::WardenAttack);
        add(&mut t, 1, warden_tid());
        let after = t.event_start(1, EntityEvent::WardenAttack);
        c.record(
            "c2.readd_clears_stale",
            before == Some(40) && after.is_none(),
            format!("before={before:?} after_readd={after:?} (want Some(40) then None)"),
        );
    }
}

// ---------------------------------------------- 3. warden attack (resolve+GPU)

fn check_warden_attack(c: &mut Checker, ids: &Ids, ev: i32) {
    // Decompiled WardenAnimation.ATTACK, right_arm ROTATION @0.1667 s:
    //   degreeVec(-90, -45, 0) = [-1.57079633, -0.785398163, 0.0] rad.
    let d = warden_event_deltas(ids, ev, 4, 0.1667);
    match find(&d, "right_arm") {
        Some((rot, _)) => {
            let want = [-1.57079633, -0.785398163, 0.0];
            c.record(
                "c3.right_arm_rot@0.1667",
                near3(rot, want, 1e-4),
                format!("rot={} want={}", fmt3(rot), fmt3(want)),
            );
        }
        None => c.record("c3.right_arm_rot@0.1667", false, "right_arm part missing"),
    }
    // WardenAnimation.ATTACK, head ROTATION.x @0.25 s = -0.52665188 rad.
    let d = warden_event_deltas(ids, ev, 4, 0.25);
    match find(&d, "head") {
        Some((rot, _)) => c.record(
            "c3.head_rot_x@0.25",
            near(rot[0], -0.52665188, 1e-4),
            format!("head.rot.x={:+.6} want=-0.52665188", rot[0]),
        ),
        None => c.record("c3.head_rot_x@0.25", false, "head part missing"),
    }
    // WardenAnimation.ATTACK, body POSITION @0.2083 s = [0, 1, -2] model px.
    let d = warden_event_deltas(ids, ev, 4, 0.2083);
    match find(&d, "body") {
        Some((_, pos)) => {
            let want = [0.0, 1.0, -2.0];
            c.record(
                "c3.body_pos@0.2083",
                near3(pos, want, 1e-4),
                format!("pos={} want={}", fmt3(pos), fmt3(want)),
            );
        }
        None => c.record("c3.body_pos@0.2083", false, "body part missing"),
    }
    // Mutation: with NO event fired the same parts are neutral (0). The warden's
    // body is Anim::None and head/arm are Anim::Head/Anim::ArmRight, which are 0
    // at neutral look/limb — so the deflections above cannot be the base pose.
    let d = oracle_part_deltas(EntityModelKind::Warden, &OracleInputs::default())
        .expect("warden model");
    let ra = find(&d, "right_arm").map(|(r, _)| r);
    let hd = find(&d, "head").map(|(r, _)| r);
    let bp = find(&d, "body").map(|(_, o)| o);
    let ok = matches!((ra, hd, bp), (Some(r), Some(h), Some(b))
        if near3(r, [0.0; 3], 1e-4) && near3(h, [0.0; 3], 1e-4) && near3(b, [0.0; 3], 1e-4));
    c.record(
        "c3.neutral_isolation",
        ok,
        format!(
            "neutral right_arm={} head={} body_pos={} (want all ~0)",
            ra.map(fmt3).unwrap_or_else(|| "missing".into()),
            hd.map(fmt3).unwrap_or_else(|| "missing".into()),
            bp.map(fmt3).unwrap_or_else(|| "missing".into()),
        ),
    );
}

// ------------------------------------------------ 4. warden sonic boom (+ catmull)

fn check_warden_sonic(c: &mut Checker, ids: &Ids, ev: i32) {
    // WardenAnimation.SONIC_BOOM, right/left_ribcage ROTATION.y @1.875 s:
    //   degreeVec(0, ±125, 0) = ±2.18166156 rad (the plates swing open).
    let d = warden_event_deltas(ids, ev, 62, 1.875);
    match find(&d, "right_ribcage") {
        Some((rot, _)) => c.record(
            "c4.right_ribcage_rot_y@1.875",
            near(rot[1], 2.18166156, 1e-4),
            format!("y={:+.6} want=+2.18166156", rot[1]),
        ),
        None => c.record(
            "c4.right_ribcage_rot_y@1.875",
            false,
            "right_ribcage missing",
        ),
    }
    match find(&d, "left_ribcage") {
        Some((rot, _)) => c.record(
            "c4.left_ribcage_rot_y@1.875",
            near(rot[1], -2.18166156, 1e-4),
            format!("y={:+.6} want=-2.18166156", rot[1]),
        ),
        None => c.record("c4.left_ribcage_rot_y@1.875", false, "left_ribcage missing"),
    }
    // WardenAnimation.SONIC_BOOM, head ROTATION.x @1.75 s = 1.3962634 rad.
    let d = warden_event_deltas(ids, ev, 62, 1.75);
    match find(&d, "head") {
        Some((rot, _)) => c.record(
            "c4.head_rot_x@1.75",
            near(rot[0], 1.3962634, 1e-4),
            format!("x={:+.6} want=1.3962634", rot[0]),
        ),
        None => c.record("c4.head_rot_x@1.75", false, "head missing"),
    }
    // CATMULL @1.375 s: head ROTATION.x sits mid-segment between the @1.0 and
    // @1.75 keyframes, catmull-rom over the four surrounding frames. Independent
    // expected value from the decompiled head-ROT-x literals:
    //   @0.0 0.0, @1.0 1.17809725, @1.75 1.3962634, @1.9167 -0.785398163;
    //   a = (1.375 - 1.0) / (1.75 - 1.0) = 0.5.
    // It must match production AND sit far from the linear interpolant
    // (~1.28718) — a lerp regression would land on the linear value and fail.
    let (p0, p1, p2, p3) = (0.0f32, 1.17809725f32, 1.3962634f32, -0.785398163f32);
    let a = 0.5f32;
    let expected = catmull(a, p0, p1, p2, p3);
    let linear = p1 + (p2 - p1) * a;
    let d = warden_event_deltas(ids, ev, 62, 1.375);
    match find(&d, "head") {
        Some((rot, _)) => {
            let obs = rot[0];
            let matches_catmull = near(obs, expected, 1e-4);
            let far_from_linear = (obs - linear).abs() > 0.05;
            c.record(
                "c4.head_rot_x@1.375_catmull",
                matches_catmull && far_from_linear,
                format!(
                    "obs={obs:+.6} catmull={expected:+.6} linear={linear:+.6} \
                     |obs-catmull|={:.6} |obs-linear|={:.6} (want ~catmull, far from linear)",
                    (obs - expected).abs(),
                    (obs - linear).abs()
                ),
            );
        }
        None => c.record("c4.head_rot_x@1.375_catmull", false, "head missing"),
    }
}

// -------------------------------------------------- 5. warden roar ownership

fn check_roar_ownership(c: &mut Checker, ids: &Ids, ev: i32) {
    let mut t = EntityTable::default();
    add(&mut t, 1, warden_tid());
    let mut g = GestureTracker::default();

    // Enter the ROARING pose (Pose ordinal 11) — the roar gesture starts (tick 100).
    let (roar0, _) = resolve_mob_anim(
        EntityModelKind::Warden,
        11,
        0,
        None,
        None,
        None,
        &mut g,
        1,
        0.0,
        100,
        0.0,
    );
    let entered = matches!(roar0, Some((Gesture::WardenRoar, _)));

    // An attack (id 4) lands at a tick at/after the roar's start → vanilla
    // `roarAnimationState.stop()` suppresses the roar durably, while the attack
    // rig itself still plays.
    route_entity_event(
        ev,
        &body(1, 4),
        ids,
        &mut t,
        Some(warden_tid()),
        Some(armadillo_tid()),
        105,
        None,
    );
    let (roar1, ev1) = resolve_mob_anim(
        EntityModelKind::Warden,
        11,
        0,
        t.event_start(1, EntityEvent::WardenAttack),
        None,
        None,
        &mut g,
        1,
        0.5,
        110,
        0.0,
    );
    let suppressed = roar1.is_none() && ev1[ModelEvent::WardenAttack.index()].is_some();
    c.record(
        "c5.roar_suppressed_by_attack",
        entered && suppressed,
        format!(
            "entered={entered} roar_after_attack={roar1:?} attack_age={:?} (want None + Some)",
            ev1[ModelEvent::WardenAttack.index()]
        ),
    );

    // Mutation: leave ROARING (a forced tracker transition), then re-enter at a
    // tick AFTER the stale attack. The fresh roar starts later than the attack,
    // so the tick-ordered ownership rule does NOT suppress it — proving the rule
    // compares ticks and is not just "any attack kills the roar".
    let _ = resolve_mob_anim(
        EntityModelKind::Warden,
        0,
        0,
        t.event_start(1, EntityEvent::WardenAttack),
        None,
        None,
        &mut g,
        1,
        1.0,
        120,
        0.0,
    );
    let (roar2, _) = resolve_mob_anim(
        EntityModelKind::Warden,
        11,
        0,
        t.event_start(1, EntityEvent::WardenAttack),
        None,
        None,
        &mut g,
        1,
        1.5,
        130,
        0.0,
    );
    let fresh = matches!(roar2, Some((Gesture::WardenRoar, _)));
    c.record(
        "c5.fresh_roar_after_reentry_not_suppressed",
        fresh,
        format!(
            "roar_after_reentry={roar2:?} (attack tick 105 < re-entry tick 130 → not suppressed)"
        ),
    );
}

// ---------------------------------------------------- 6. armadillo scared/peek

fn check_armadillo_scared(c: &mut Checker, ids: &Ids, ev: i32) {
    let mut t = EntityTable::default();
    add(&mut t, 2, armadillo_tid());
    let mut g = GestureTracker::default();

    // Enter SCARED (ArmadilloState ordinal 2 at metadata index 17). Vanilla
    // fastForwards the peek to its end pose (+2.5 s), so with NO event fired the
    // peek holds at age 2.5.
    let (g_scared, ev0) = resolve_mob_anim(
        EntityModelKind::Armadillo,
        0,
        2,
        None,
        None,
        None,
        &mut g,
        2,
        100.0,
        START_TICK,
        0.0,
    );
    let held_age = match g_scared {
        Some((Gesture::ArmadilloScared, age)) => Some(age),
        _ => None,
    };
    c.record(
        "c6.no_event_holds_age_2.5",
        held_age.is_some_and(|a| near(a, 2.5, 1e-3)) && ev0.iter().all(|e| e.is_none()),
        format!("scared_age={held_age:?} (want ~2.5) events={ev0:?}"),
    );

    // The held pose reaches the GPU oracle. Decompiled ArmadilloAnimation.PEEK,
    // right_front_leg ROTATION last keyframe @1.95 = degreeVec(-90, 0, 0) =
    // [-1.57079633, 0, 0]; held for age >= 1.95, so age 2.5 samples it.
    let held = armadillo_scared_deltas(g_scared);
    let leg_held = find(&held, "right_front_leg").map(|(r, _)| r);
    c.record(
        "c6.held_peek_leg_literal",
        leg_held.is_some_and(|r| near3(r, [-1.57079633, 0.0, 0.0], 1e-4)),
        format!(
            "right_front_leg.rot={} want=[-1.570796, 0, 0]",
            leg_held.map(fmt3).unwrap_or_else(|| "missing".into())
        ),
    );
    // head POSITION.z at the held pose is the final PEEK frame's z (5.2). z is
    // not touched by the generator's pos-Y negation, so it is a clean literal.
    let head_z_held = find(&held, "head").map(|(_, o)| o[2]);

    // id 64 restarts the SAME peek from age 0 (vanilla re-`startIfStopped`),
    // because the receipt tick is at/after the SCARED start.
    route_entity_event(
        ev,
        &body(2, 64),
        ids,
        &mut t,
        Some(warden_tid()),
        Some(armadillo_tid()),
        START_TICK + 5,
        None,
    );
    let (g_restart, _) = resolve_mob_anim(
        EntityModelKind::Armadillo,
        0,
        2,
        None,
        None,
        t.event_start(2, EntityEvent::ArmadilloPeek),
        &mut g,
        2,
        100.3,
        START_TICK + 5,
        0.0,
    );
    let restart_age = match g_restart {
        Some((Gesture::ArmadilloScared, age)) => Some(age),
        _ => None,
    };
    c.record(
        "c6.id64_restarts_age_0",
        restart_age.is_some_and(|a| near(a, 0.0, 1e-3)),
        format!("scared_age_after_id64={restart_age:?} (want ~0)"),
    );

    // The restart is visible in a part delta: decompiled PEEK head POSITION.z at
    // age 0 = 7 (peeking out), distinct from the held-pose z = 5.2. A missing
    // re-clock would leave the head at the held pose.
    let restarted = armadillo_scared_deltas(g_restart);
    let head_z_0 = find(&restarted, "head").map(|(_, o)| o[2]);
    let visible = matches!((head_z_0, head_z_held),
        (Some(z0), Some(zh)) if near(z0, 7.0, 1e-3) && near(zh, 5.2, 1e-3) && (z0 - zh).abs() > 1.5);
    c.record(
        "c6.restart_visible_in_part_delta",
        visible,
        format!("head.pos.z restart(age0)={head_z_0:?} held={head_z_held:?} (want ~7.0 vs ~5.2)"),
    );

    // Later the re-clocked peek runs past its 2.5 s length and holds its final
    // pose again (age grows; head z returns to the held 5.2).
    let (g_late, _) = resolve_mob_anim(
        EntityModelKind::Armadillo,
        0,
        2,
        None,
        None,
        t.event_start(2, EntityEvent::ArmadilloPeek),
        &mut g,
        2,
        103.0,
        START_TICK + 5 + 60,
        0.0,
    );
    let late_age = match g_late {
        Some((Gesture::ArmadilloScared, age)) => Some(age),
        _ => None,
    };
    let late = armadillo_scared_deltas(g_late);
    let head_z_late = find(&late, "head").map(|(_, o)| o[2]);
    let holds_final = matches!((late_age, head_z_late),
        (Some(a), Some(z)) if a > 2.5 && near(z, 5.2, 1e-3));
    c.record(
        "c6.later_holds_final",
        holds_final,
        format!("late_age={late_age:?} (want >2.5) head.pos.z={head_z_late:?} (want ~5.2 held)"),
    );
}

// -------------------------------------- 7. named warden ribcage parts at rest

fn check_neutral_ribcage(c: &mut Checker) {
    // The SONIC_BOOM rig targets "right_ribcage"/"left_ribcage" by name, which
    // forces them to be real model parts (they were promoted from folded cubes
    // on `body`). Prove they exist at the oracle boundary and contribute zero
    // delta when idle — the sonic-boom checks above then show they actually move.
    let d = oracle_part_deltas(EntityModelKind::Warden, &OracleInputs::default())
        .expect("warden model");
    let names: Vec<&str> = d.iter().map(|&(n, _, _)| n).collect();
    let r = find(&d, "right_ribcage");
    let l = find(&d, "left_ribcage");
    let present = r.is_some() && l.is_some();
    let idle = r.is_some_and(|(rot, pos)| near3(rot, [0.0; 3], 1e-6) && near3(pos, [0.0; 3], 1e-6))
        && l.is_some_and(|(rot, pos)| near3(rot, [0.0; 3], 1e-6) && near3(pos, [0.0; 3], 1e-6));
    c.record(
        "c7.warden_ribcage_parts_present_and_idle",
        present && idle,
        format!(
            "right_ribcage_present={} left_ribcage_present={} parts={names:?}",
            r.is_some(),
            l.is_some()
        ),
    );
}
