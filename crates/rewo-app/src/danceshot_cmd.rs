//! `rewo danceshot` — M18's permanent **serverless** Allay-dance oracle.
//!
//! The Allay dance is a *metadata* animation, not an entity event (M17 proved
//! `handleEntityEvent(18)` is heart particles). It is driven by `DATA_DANCING`
//! — `SynchedEntityData` **index 16, BOOLEAN (serializer 8)** — plus the
//! client-side `dancingAnimationTicks` / `spinningAnimationTicks(0)` counters
//! that `Allay.tick()` advances, feeding `AllayModel.setupAnim`'s root/head
//! transforms. This gate proves the whole continuous production path with no
//! socket and no GPU device:
//!
//! ```text
//! raw set_entity_data body (VarInt id + metadata delta stream, built here)
//!   -> rewo_net::route_set_entity_data      (real packet-id selection seam)
//!   -> apply_set_entity_data                (missing-entity drop + kind-aware DANCING vs BABY)
//!   -> EntityTable::set_dancing + tick_lerp (the real client counter lifecycle)
//!   -> live_cmd::resolve_allay_dance        (the SAME app resolver collect_entities uses)
//!   -> rewo_gpu::entities::oracle_part_deltas   (the exact AllayRoot/AllayHead math)
//! ```
//!
//! **Fail-closed by construction.** A fixed [`EXPECTED_WITNESSES`] count equals
//! the number of named properties. Each property is *observed* (the real value
//! is read and printed) and increments the counter only on a real pass; the run
//! errors if any property failed **or** the observed count differs — the latter
//! catches a property silently skipped (a missing part, a `None` from the
//! oracle).
//!
//! **The expected values are independent literals/reimplementations.** The dance
//! formula ([`expect_dance`]) and the counter simulation ([`sim`]) are
//! hand-transcribed from the decompiled `AllayModel.setupAnim` / `Allay.tick()`
//! — nothing reads the production `anim_delta` or `EntityTable` as its
//! expectation (grading either against itself would verify nothing). The
//! production counters are separately proven equal to the independent
//! simulation, then fed through the production pose math and graded against the
//! independent formula.
//!
//! **Every property carries a mutation/sensitivity partner.** The wrong-packet-id
//! case proves id selection is load-bearing; the non-Allay case proves the
//! dancing bit is not the pre-M18 generic baby (the latent bug); the not-dancing
//! case proves the deflections come from the dance and not the base pose; the
//! dance-suppresses-look case proves the head ignores the look while dancing; the
//! animationSpeed case proves that `danceSpeed` term; the alpha case proves the
//! partial-tick interpolation direction.

use clap::Args as ClapArgs;
use rewo_data::{entity_types::EntityTypes, packets::Packets, DataPaths};
use rewo_gpu::entities::{oracle_part_deltas, OracleInputs};
use rewo_gpu::mobs::EntityModelKind;
use rewo_net::{ids::Ids, route_set_entity_data};
use rewo_world::entities::{EntityState, EntityTable};

/// Total named properties this gate asserts. Locked so a skipped property fails
/// the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 24;

/// Degrees → radians, the factor `AllayModel` writes inline as `Math.PI/180`.
const DEG: f32 = std::f32::consts::PI / 180.0;

#[derive(ClapArgs)]
pub struct DanceshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `eventshot`/`dimensioncheck` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Version whose `packets.json` / `registries.json` resolve the real
    /// `set_entity_data` packet id and the `minecraft:allay` type id.
    #[arg(long, default_value = "26.2")]
    version: String,
}

pub fn run(args: DanceshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[danceshot] mode: {mode} (serverless; the oracle asserts unconditionally — a \
         failure exits nonzero with or without --check)"
    );

    // Resolve the real `set_entity_data` packet id (the dispatch seam keys off
    // it) from the datagen report only — no jar, no GPU. `entity_event` is a
    // *different real* clientbound-play id, the control that proves
    // `route_set_entity_data` selects on the id, not the body shape.
    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let sed_id = ids.cb_play_set_entity_data;
    let wrong_id = ids.cb_play_entity_event;
    if wrong_id == sed_id {
        return Err("set_entity_data and entity_event resolved to the same id".into());
    }
    println!("[danceshot] set_entity_data id = {sed_id}, control id (entity_event) = {wrong_id}");

    let mut c = Checker::new();

    // Resolve the Allay type id through the exact production path
    // (`EntityTypes::id_of` on `registries.json`, the same call
    // `live_cmd`/`play_cmd` make). A missing type, or a collision with the
    // control kind, fails closed here — before any positive check — and the
    // resolved id is what every entity creation + route call below uses.
    let entity_types = EntityTypes::load(&paths.registries_json())?;
    let allay = entity_types.id_of("minecraft:allay");
    let zombie = entity_types.id_of("minecraft:zombie");
    let resolved = match (allay, zombie) {
        (Some(a), Some(z)) if a != z => Some((a, z)),
        _ => None,
    };
    c.record(
        "c0.set_entity_data_id_resolved",
        sed_id != wrong_id,
        format!("set_entity_data={sed_id} != entity_event={wrong_id}"),
    );
    c.record(
        "c0.allay_type_id_resolved",
        resolved.is_some(),
        format!(
            "allay={allay:?} zombie={zombie:?} (want two distinct Some via EntityTypes::id_of)"
        ),
    );
    let Some((allay_tid, zombie_tid)) = resolved else {
        return Err(format!(
            "EntityTypes::id_of failed to resolve distinct allay/zombie ids \
             (allay={allay:?} zombie={zombie:?})"
        ));
    };
    println!("[danceshot] entity types (via EntityTypes::id_of): allay = {allay_tid}, zombie = {zombie_tid}");

    check_routing(&mut c, &ids, sed_id, wrong_id, allay_tid, zombie_tid);
    check_counters(&mut c, &ids, sed_id, allay_tid);
    check_pose(&mut c, &ids, sed_id, allay_tid);

    println!(
        "[danceshot] witnesses observed: {} / {}",
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
    println!("[danceshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// --------------------------------------------------------------- accumulator

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
        println!("[danceshot] {status}  {name}: {detail}");
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

// -------------------------------------------------------- independent oracle

/// Independent simulation of `Allay.tick()`'s client counter block, run for
/// `dancing_ticks_to_run` consecutive dancing ticks from a fresh dance start.
/// Returns `(dancing_ticks, spinning_ticks, spinning_ticks0)` — a hand
/// transcription of the decompiled statements, deliberately NOT calling the
/// production `AllayDance::tick`.
fn sim(dancing_ticks_to_run: u32) -> (f32, f32, f32) {
    let (mut dt, mut st, mut st0) = (0.0f32, 0.0f32, 0.0f32);
    for _ in 0..dancing_ticks_to_run {
        dt += 1.0;
        st0 = st;
        // isSpinning() reads the just-incremented dancing_ticks.
        if dt.rem_euclid(55.0) < 15.0 {
            st += 1.0;
        } else {
            st -= 1.0;
        }
        st = st.clamp(0.0, 15.0);
    }
    (dt, st, st0)
}

/// `Allay.isSpinning()` — `dancingAnimationTicks % 55 < 15`, independent.
fn sim_is_spinning(dancing_ticks: f32) -> bool {
    dancing_ticks.rem_euclid(55.0) < 15.0
}

/// `Allay.getSpinningProgress(alpha)` — `lerp(alpha, ticks0, ticks) / 15`.
fn sim_progress(spinning_ticks: f32, spinning_ticks0: f32, alpha: f32) -> f32 {
    (spinning_ticks0 + (spinning_ticks - spinning_ticks0) * alpha) / 15.0
}

/// Independent transcription of `AllayModel.setupAnim`'s `isDancing` branch.
/// Returns `(root_rot, head_rot)` Euler deltas (radians, ZYX). `anim_speed` is
/// `walkAnimationSpeed` (`animationSpeed`); `age_seconds · 20 = ageInTicks`.
fn expect_dance(
    age_seconds: f32,
    anim_speed: f32,
    is_spinning: bool,
    spin: f32,
) -> ([f32; 3], [f32; 3]) {
    let age_in_ticks = age_seconds * 20.0;
    let dance_speed = age_in_ticks * 8.0 * DEG + anim_speed;
    let cos_ds = dance_speed.cos();
    let root_y = if is_spinning {
        std::f32::consts::PI * 4.0 * spin
    } else {
        0.0
    };
    let root_z = cos_ds * 16.0 * DEG * (1.0 - spin);
    let head_y = cos_ds * 30.0 * DEG * (1.0 - spin);
    let head_z = cos_ds * 14.0 * DEG * (1.0 - spin);
    ([0.0, root_y, root_z], [0.0, head_y, head_z])
}

/// Independent transcription of `AllayModel.setupAnim`'s wing flap — the
/// always-applied hover motion (outside the dance branch). `anim_pos` is
/// `walkAnimationPos`, `anim_speed` is `walkAnimationSpeed`. Used to prove the
/// wings survive the hierarchy restructure and animate independently of the
/// dance.
fn expect_wing(age_seconds: f32, anim_pos: f32, anim_speed: f32, left: bool) -> [f32; 3] {
    let age_in_ticks = age_seconds * 20.0;
    let flap = (age_in_ticks * 20.0 * DEG + anim_pos).cos() * std::f32::consts::PI * 0.15 + anim_speed;
    let fly = (anim_speed / 0.3).min(1.0);
    let x = 0.43633232 * (1.0 - fly);
    let y = if left {
        std::f32::consts::PI / 4.0 - flap
    } else {
        -std::f32::consts::PI / 4.0 + flap
    };
    [x, y, 0.0]
}

// ------------------------------------------------------------------- helpers

/// A `ClientboundSetEntityDataPacket` body: VarInt entity id then a single
/// `SynchedEntityData` entry (index u8 + serializer VarInt + value) terminated
/// by 0xFF. Built independently of any writer. `eid` kept < 128 (one VarInt
/// byte).
fn sed_body(eid: u8, index: u8, serializer: u8, value: &[u8]) -> Vec<u8> {
    let mut b = vec![eid, index, serializer];
    b.extend_from_slice(value);
    b.push(0xFF);
    b
}

/// Index-16 BOOLEAN carrying `dancing` — polymorphic (dancing on an Allay).
fn sed_dancing(eid: u8, dancing: bool) -> Vec<u8> {
    sed_body(eid, 16, 8, &[dancing as u8])
}

/// Add a bare entity of `type_id` at the origin.
fn add(t: &mut EntityTable, id: i32, type_id: i32) {
    t.add(id, EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0));
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

/// A named part's `(rot delta, pos delta)` in the oracle output.
fn find(deltas: &[(&'static str, [f32; 3], [f32; 3])], name: &str) -> Option<([f32; 3], [f32; 3])> {
    deltas
        .iter()
        .find(|&&(n, _, _)| n == name)
        .map(|&(_, r, o)| (r, o))
}

/// Drive the FULL production path for a dancing Allay: route the real
/// `set_entity_data` packet (index-16 BOOLEAN true), tick the real EntityTable
/// `dancing_ticks_to_run` times, resolve the render state through the SAME app
/// resolver the live client uses (`live_cmd::resolve_allay_dance`), and run the
/// GPU rig oracle at `(age_seconds, anim_speed, pitch, net)` — no device.
/// Returns the per-part deltas plus the *production* `(is_spinning,
/// spinning_progress)` the resolver produced (so the caller can grade both
/// counters and pose math).
fn drive_dance(
    ids: &Ids,
    sed_id: i32,
    allay_tid: i32,
    dancing_ticks_to_run: u32,
    alpha: f32,
    age_seconds: f32,
    anim_speed: f32,
    pitch: f32,
    net: f32,
) -> (Vec<(&'static str, [f32; 3], [f32; 3])>, Option<(bool, f32)>) {
    let mut t = EntityTable::default();
    add(&mut t, 1, allay_tid);
    // Dance ON via the real packet routing (proves id selection + kind-aware
    // disambiguation en route to the counters).
    route_set_entity_data(sed_id, &sed_dancing(1, true), ids, &mut t, Some(allay_tid));
    for _ in 0..dancing_ticks_to_run {
        t.tick_lerp();
    }
    // The exact production app resolver — collect_entities calls this same fn,
    // so a regression there fails this gate too.
    let dance_input = crate::live_cmd::resolve_allay_dance(EntityModelKind::Allay, &t, 1, alpha);
    let dance = dance_input.map(|d| (d.is_spinning, d.spinning_progress));
    let inputs = OracleInputs {
        pitch,
        net,
        limb_amount: anim_speed,
        age_seconds,
        allay_dance: dance_input,
        ..Default::default()
    };
    let deltas =
        oracle_part_deltas(EntityModelKind::Allay, &inputs).expect("allay has a built-in model");
    (deltas, dance)
}

// ------------------------------------------------------ 1. routing / disambig

fn check_routing(
    c: &mut Checker,
    ids: &Ids,
    sed: i32,
    wrong: i32,
    allay_tid: i32,
    zombie_tid: i32,
) {
    // The core bug fix: an Allay's index-16 BOOLEAN is DANCING, not baby.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        let m = route_set_entity_data(sed, &sed_dancing(1, true), ids, &mut t, Some(allay_tid));
        let dance = t.allay_dance_render(1, 1.0);
        let baby = t.is_baby(1);
        c.record(
            "c1.allay_index16_is_dancing_not_baby",
            m && dance.is_some() && !baby,
            format!("matched={m} dance={dance:?} baby={baby} (want matched=true, dance=Some, baby=false)"),
        );
    }
    // Sensitivity — the latent pre-M18 bug: on a NON-Allay the same bit is baby,
    // and the entity never dances. If dancing routed generically to baby, the
    // Allay above would be baby too; this pins the kind-aware split.
    {
        let mut t = EntityTable::default();
        add(&mut t, 2, zombie_tid);
        let m = route_set_entity_data(sed, &sed_dancing(2, true), ids, &mut t, Some(allay_tid));
        let dance = t.allay_dance_render(2, 1.0);
        let baby = t.is_baby(2);
        c.record(
            "c1.nonallay_index16_is_baby_not_dancing",
            m && baby && dance.is_none(),
            format!("matched={m} baby={baby} dance={dance:?} (want matched=true, baby=true, dance=None)"),
        );
    }
    // Sensitivity: the WRONG packet id must not dispatch — id selection is
    // load-bearing (nothing dances, the chain continues).
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        let m = route_set_entity_data(wrong, &sed_dancing(1, true), ids, &mut t, Some(allay_tid));
        let dance = t.allay_dance_render(1, 1.0);
        c.record(
            "c1.wrong_packet_id_inert",
            !m && dance.is_none(),
            format!("matched={m} dance={dance:?} (want matched=false, dance=None)"),
        );
    }
    // The INT serializer at index 16 is a cube size, never the polymorphic
    // BOOLEAN — so an Allay-typed entity with (16,INT) does not dance.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        route_set_entity_data(
            sed,
            &sed_body(1, 16, 1, &[0x04]),
            ids,
            &mut t,
            Some(allay_tid),
        );
        let size = t.size(1);
        let dance = t.allay_dance_render(1, 1.0);
        c.record(
            "c1.index16_int_is_size_not_dance",
            size == Some(4) && dance.is_none(),
            format!("size={size:?} dance={dance:?} (want size=Some(4), dance=None)"),
        );
    }
    // A false update stops the dance (isDancing() is the metadata flag).
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        route_set_entity_data(sed, &sed_dancing(1, true), ids, &mut t, Some(allay_tid));
        t.tick_lerp();
        let before = t.allay_dance_render(1, 1.0).is_some();
        route_set_entity_data(sed, &sed_dancing(1, false), ids, &mut t, Some(allay_tid));
        let after = t.allay_dance_render(1, 1.0);
        c.record(
            "c1.dancing_false_stops",
            before && after.is_none(),
            format!("before={before} after_false={after:?} (want true then None)"),
        );
    }
    // Missing entity → vanilla drops the WHOLE metadata packet
    // (`ClientPacketListener.handleSetEntityData`: getEntity == null → no
    // assignValues). The packet id still matches (route returns true), but NO
    // state is mutated — not dance, not baby, and not any other field the same
    // packet carries (here a pose at index 6 that WOULD set on a tracked entity).
    {
        let mut t = EntityTable::default();
        // Entity 9 is NOT added — the table has no entity for it.
        let mut b = vec![9u8]; // eid VarInt
        b.extend_from_slice(&[6, 20, 11]); // index 6 POSE(20) = 11 (ROARING)
        b.extend_from_slice(&[16, 8, 1]); // index 16 BOOLEAN(8) = true
        b.push(0xFF);
        let m = route_set_entity_data(sed, &b, ids, &mut t, Some(allay_tid));
        let dance = t.allay_dance_render(9, 1.0);
        let baby = t.is_baby(9);
        let pose = t.pose(9);
        c.record(
            "c1.missing_entity_inert",
            m && dance.is_none() && !baby && pose == 0,
            format!(
                "matched={m} dance={dance:?} baby={baby} pose={pose} \
                 (want matched=true, dance=None, baby=false, pose=0 default)"
            ),
        );
    }
    // Wrong index: an Allay with a BOOLEAN at a NON-16 index does not dance or
    // baby — only slot 16 is the dancing/baby slot. Uses a production-shaped
    // value the parser skips exactly.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        // index 20, BOOLEAN(8), true — a valid skippable entry at the wrong slot.
        route_set_entity_data(sed, &sed_body(1, 20, 8, &[0x01]), ids, &mut t, Some(allay_tid));
        let dance = t.allay_dance_render(1, 1.0);
        let baby = t.is_baby(1);
        c.record(
            "c1.wrong_index_boolean_inert",
            dance.is_none() && !baby,
            format!("dance={dance:?} baby={baby} (want both inert — only slot 16 dances/babies)"),
        );
    }
    // Wrong serializer at slot 16: a FLOAT (serializer 3) at index 16 is neither
    // the INT size nor the BOOLEAN dancing/baby — the parser skips it exactly
    // (reads 4 bytes), so nothing is set.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        // index 16, FLOAT(3), 4 bytes — captured by neither (16,1) nor (16,8).
        route_set_entity_data(sed, &sed_body(1, 16, 3, &[0, 0, 0, 0]), ids, &mut t, Some(allay_tid));
        let dance = t.allay_dance_render(1, 1.0);
        let baby = t.is_baby(1);
        let size = t.size(1);
        c.record(
            "c1.wrong_serializer_at_16_inert",
            dance.is_none() && !baby && size.is_none(),
            format!(
                "dance={dance:?} baby={baby} size={size:?} \
                 (want all inert — slot 16 needs BOOLEAN or INT)"
            ),
        );
    }
}

// ----------------------------------------------- 2. client counter lifecycle

fn check_counters(c: &mut Checker, ids: &Ids, sed: i32, allay_tid: i32) {
    // After 5 dancing ticks the real counters must equal the independent sim,
    // isSpinning=true (5 % 55 < 15), spinningProgress @alpha=1 = 5/15.
    {
        let (_, dance) = drive_dance(ids, sed, allay_tid, 5, 1.0, 0.0, 0.0, 0.0, 0.0);
        let (dt, st, st0) = sim(5);
        let want = (sim_is_spinning(dt), sim_progress(st, st0, 1.0));
        c.record(
            "c2.ticks5_spinning_and_progress",
            dance.is_some_and(|(s, p)| s == want.0 && near(p, want.1, 1e-5)),
            format!(
                "prod={dance:?} indep=(spinning={}, progress={:.6}) [sim ticks=({dt},{st},{st0})]",
                want.0, want.1
            ),
        );
    }
    // Spin-window boundary — the EXACT off-by-one: `dancing_ticks % 55 < 15`.
    // Tick 14 (14 < 15) is spinning; tick 15 (15 is NOT < 15) is the first
    // non-spinning tick; tick 55 (55 % 55 = 0 < 15) re-enters. A `<= 15`
    // regression would report tick 15 as spinning; a wrong loop length would
    // miss the tick-55 re-entry.
    {
        let (_, at14) = drive_dance(ids, sed, allay_tid, 14, 1.0, 0.0, 0.0, 0.0, 0.0);
        let (_, at15) = drive_dance(ids, sed, allay_tid, 15, 1.0, 0.0, 0.0, 0.0, 0.0);
        let (_, at55) = drive_dance(ids, sed, allay_tid, 55, 1.0, 0.0, 0.0, 0.0, 0.0);
        let ok = at14.is_some_and(|(s, _)| s)
            && at15.is_some_and(|(s, _)| !s)
            && at55.is_some_and(|(s, _)| s);
        // Independent confirmation of the exact boundary.
        let indep = sim_is_spinning(sim(14).0) && !sim_is_spinning(sim(15).0) && sim_is_spinning(sim(55).0);
        c.record(
            "c2.spin_window_boundary_14_15_55",
            ok && indep,
            format!("spinning@14={at14:?} @15={at15:?} @55={at55:?} (want true, false, true)"),
        );
    }
    // Repeated DATA_DANCING=true while already dancing does NOT restart the
    // counter — vanilla `setDancing` just re-writes the (already-true) metadata;
    // only `tick()` advances the clock. After 5 + 3 = 8 continuous dancing ticks
    // (with a redundant `true` in between), the counter must read 8 ticks, not a
    // 3-tick restart.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        route_set_entity_data(sed, &sed_dancing(1, true), ids, &mut t, Some(allay_tid));
        for _ in 0..5 {
            t.tick_lerp();
        }
        route_set_entity_data(sed, &sed_dancing(1, true), ids, &mut t, Some(allay_tid)); // redundant
        for _ in 0..3 {
            t.tick_lerp();
        }
        let dance = t.allay_dance_render(1, 1.0);
        let (dt, st, st0) = sim(8);
        let want = sim_progress(st, st0, 1.0);
        let restart = sim_progress(sim(3).1, sim(3).2, 1.0); // what a restart would read
        c.record(
            "c2.repeated_true_does_not_restart",
            dance.is_some_and(|(s, p)| s == sim_is_spinning(dt) && near(p, want, 1e-5))
                && (want - restart).abs() > 1e-3,
            format!("prod={dance:?} indep(8 ticks)={want:.6} (a restart would read {restart:.6})"),
        );
    }
    // A false update then a true update restarts from zero on the next tick:
    // dance 7 ticks, false, tick (counters zero), true, tick (one fresh tick) →
    // progress 1/15, spinning true. Proves the false-reset happens on the next
    // client tick and the resume starts a clean cycle.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        route_set_entity_data(sed, &sed_dancing(1, true), ids, &mut t, Some(allay_tid));
        for _ in 0..7 {
            t.tick_lerp();
        }
        route_set_entity_data(sed, &sed_dancing(1, false), ids, &mut t, Some(allay_tid));
        t.tick_lerp(); // counters reset to 0 this tick
        route_set_entity_data(sed, &sed_dancing(1, true), ids, &mut t, Some(allay_tid));
        t.tick_lerp(); // one fresh dancing tick
        let dance = t.allay_dance_render(1, 1.0);
        let (dt, st, st0) = sim(1);
        let want = (sim_is_spinning(dt), sim_progress(st, st0, 1.0));
        c.record(
            "c2.false_then_true_restarts_from_zero",
            dance.is_some_and(|(s, p)| s == want.0 && near(p, want.1, 1e-6)),
            format!("prod={dance:?} indep(fresh 1 tick)=({}, {:.6})", want.0, want.1),
        );
    }
    // `isSpinning` and `spinningProgress` are DISTINCT quantities. At 20 ticks
    // the counters give isSpinning=false (20 % 55 = 20, not < 15) yet
    // spinningProgress = 8/15 ≈ 0.53 > 0. A bug conflating the two (e.g.
    // isSpinning := progress > 0) would report spinning=true here.
    {
        let (_, dance) = drive_dance(ids, sed, allay_tid, 20, 1.0, 0.0, 0.0, 0.0, 0.0);
        let (dt, st, st0) = sim(20);
        let want_spin = sim_is_spinning(dt); // false
        let want_prog = sim_progress(st, st0, 1.0); // ≈0.533
        c.record(
            "c2.isspinning_distinct_from_progress",
            dance.is_some_and(|(s, p)| s == want_spin && !s && p > 0.1 && near(p, want_prog, 1e-5)),
            format!("prod={dance:?} want(spinning={want_spin}, progress={want_prog:.6} > 0.1)"),
        );
    }
    // Partial-tick interpolation direction: at 5 ticks, ticks0=4, ticks=5, so
    // @alpha=0.5 = 4.5/15 (between them), distinct from @alpha=1 = 5/15. A
    // dropped/reversed alpha would collapse them.
    {
        let (_, half) = drive_dance(ids, sed, allay_tid, 5, 0.5, 0.0, 0.0, 0.0, 0.0);
        let (_, full) = drive_dance(ids, sed, allay_tid, 5, 1.0, 0.0, 0.0, 0.0, 0.0);
        let (_, st, st0) = sim(5);
        let want_half = sim_progress(st, st0, 0.5);
        let want_full = sim_progress(st, st0, 1.0);
        let ok = half.is_some_and(|(_, p)| near(p, want_half, 1e-5))
            && full.is_some_and(|(_, p)| near(p, want_full, 1e-5))
            && (want_half - want_full).abs() > 1e-3;
        c.record(
            "c2.partial_tick_interpolation",
            ok,
            format!(
                "progress@0.5={half:?} (want {want_half:.6}) progress@1.0={full:?} (want {want_full:.6})"
            ),
        );
    }
    // Lifecycle: removal and id-reuse drop the stale dance clock.
    {
        let mut t = EntityTable::default();
        add(&mut t, 1, allay_tid);
        route_set_entity_data(sed, &sed_dancing(1, true), ids, &mut t, Some(allay_tid));
        t.tick_lerp();
        let before = t.allay_dance_render(1, 1.0).is_some();
        t.remove(1);
        let after_remove = t.allay_dance_render(1, 1.0);
        // Reuse the id with a fresh (non-dancing) occupant → no inherited clock.
        add(&mut t, 1, allay_tid);
        let after_readd = t.allay_dance_render(1, 1.0);
        c.record(
            "c2.removal_and_readd_clear",
            before && after_remove.is_none() && after_readd.is_none(),
            format!("before={before} after_remove={after_remove:?} after_readd={after_readd:?}"),
        );
    }
}

// ---------------------------------------------------- 3. AllayRoot/Head pose

fn check_pose(c: &mut Checker, ids: &Ids, sed: i32, allay_tid: i32) {
    // Scenario A — swaying (20 ticks: dancing, NOT spinning). At age 0.5 s the
    // root/head carry the sway; root.yRot is 0 (not spinning). Graded against
    // the independent formula at the independent counter values.
    {
        let age = 0.5;
        let (d, dance) = drive_dance(ids, sed, allay_tid, 20, 1.0, age, 0.0, 0.0, 0.0);
        let (dt, st, st0) = sim(20);
        let (want_root, want_head) =
            expect_dance(age, 0.0, sim_is_spinning(dt), sim_progress(st, st0, 1.0));
        let root = find(&d, "root").map(|(r, _)| r);
        let head = find(&d, "head").map(|(r, _)| r);
        let ok = root.is_some_and(|r| near3(r, want_root, 1e-5))
            && head.is_some_and(|h| near3(h, want_head, 1e-5))
            && dance.is_some_and(|(s, _)| !s); // not spinning
        c.record(
            "c3.sway_root_and_head",
            ok,
            format!(
                "root={} want={} head={} want={} (dance={dance:?})",
                root.map(fmt3).unwrap_or_else(|| "missing".into()),
                fmt3(want_root),
                head.map(fmt3).unwrap_or_else(|| "missing".into()),
                fmt3(want_head),
            ),
        );
    }
    // Scenario B — spinning (5 ticks: dancing, spinning). root.yRot = 4π·spin
    // (a big value, ~4.19 at spin=1/3), distinct from the sway's 0; root/head
    // zRot are scaled by (1 − spin).
    {
        let age = 0.5;
        let (d, dance) = drive_dance(ids, sed, allay_tid, 5, 1.0, age, 0.0, 0.0, 0.0);
        let (dt, st, st0) = sim(5);
        let spin = sim_progress(st, st0, 1.0);
        let (want_root, want_head) = expect_dance(age, 0.0, sim_is_spinning(dt), spin);
        let root = find(&d, "root").map(|(r, _)| r);
        let head = find(&d, "head").map(|(r, _)| r);
        let big_yrot = want_root[1].abs() > 3.0; // 4π·(5/15) ≈ 4.19
        let ok = root.is_some_and(|r| near3(r, want_root, 1e-5))
            && head.is_some_and(|h| near3(h, want_head, 1e-5))
            && big_yrot
            && dance.is_some_and(|(s, _)| s);
        c.record(
            "c3.spin_root_yrot",
            ok,
            format!(
                "root={} want={} (yRot big={big_yrot}) head={} want={}",
                root.map(fmt3).unwrap_or_else(|| "missing".into()),
                fmt3(want_root),
                head.map(fmt3).unwrap_or_else(|| "missing".into()),
                fmt3(want_head),
            ),
        );
    }
    // Not dancing → the ordinary head-look: with a nonzero pitch/net and NO
    // dance, head.xRot = pitch, head.yRot = net, head.zRot = 0, and the root is
    // fully neutral. Proves the deflections above are the dance, not the base
    // pose. Uses OracleInputs directly (a non-dancing Allay yields None).
    {
        let (pitch, net) = (0.3_f32, -0.4_f32);
        let inputs = OracleInputs {
            pitch,
            net,
            allay_dance: None,
            ..Default::default()
        };
        let d = oracle_part_deltas(EntityModelKind::Allay, &inputs).expect("allay model");
        let root = find(&d, "root").map(|(r, _)| r);
        let head = find(&d, "head").map(|(r, _)| r);
        let ok = root.is_some_and(|r| near3(r, [0.0; 3], 1e-6))
            && head.is_some_and(|h| near3(h, [pitch, net, 0.0], 1e-6));
        c.record(
            "c3.not_dancing_head_is_look",
            ok,
            format!(
                "root={} (want 0) head={} (want [{pitch:+.3}, {net:+.3}, 0])",
                root.map(fmt3).unwrap_or_else(|| "missing".into()),
                head.map(fmt3).unwrap_or_else(|| "missing".into()),
            ),
        );
    }
    // Dancing SUPPRESSES the look: feed a nonzero pitch/net WHILE dancing → the
    // head uses the dance (xRot = 0, yRot = the dance tilt, NOT `net`). A model
    // that let the look leak through would show head.xRot = pitch.
    {
        let age = 0.5;
        let (pitch, net) = (0.3_f32, -0.4_f32);
        let (d, _) = drive_dance(ids, sed, allay_tid, 20, 1.0, age, 0.0, pitch, net);
        let (dt, st, st0) = sim(20);
        let (_, want_head) =
            expect_dance(age, 0.0, sim_is_spinning(dt), sim_progress(st, st0, 1.0));
        let head = find(&d, "head").map(|(r, _)| r);
        let ok = head.is_some_and(|h| {
            near(h[0], 0.0, 1e-6)        // xRot NOT pitch
                && near(h[1], want_head[1], 1e-5) // yRot the dance, NOT net
                && (h[1] - net).abs() > 0.1 // and clearly distinct from net
        });
        c.record(
            "c3.dance_suppresses_look",
            ok,
            format!(
                "head={} want=[0, {:+.6}, {:+.6}] (net={net:+.3} must NOT appear)",
                head.map(fmt3).unwrap_or_else(|| "missing".into()),
                want_head[1],
                want_head[2],
            ),
        );
    }
    // The dance BIT is load-bearing: the same swaying scenario with the dance
    // toggled OFF (None) leaves root.zRot at 0. Sensitivity partner to
    // c3.sway_root_and_head — flips one input, the deflection vanishes.
    {
        let age = 0.5;
        let (on, _) = drive_dance(ids, sed, allay_tid, 20, 1.0, age, 0.0, 0.0, 0.0);
        let off = oracle_part_deltas(
            EntityModelKind::Allay,
            &OracleInputs {
                age_seconds: age,
                allay_dance: None,
                ..Default::default()
            },
        )
        .expect("allay model");
        let root_on = find(&on, "root").map(|(r, _)| r[2]);
        let root_off = find(&off, "root").map(|(r, _)| r[2]);
        let ok =
            root_on.is_some_and(|z| z.abs() > 1e-3) && root_off.is_some_and(|z| near(z, 0.0, 1e-6));
        c.record(
            "c3.dance_bit_load_bearing",
            ok,
            format!("root.zRot dance_on={root_on:?} (want !=0) dance_off={root_off:?} (want 0)"),
        );
    }
    // `danceSpeed` includes `animationSpeed`: with a nonzero walk speed the
    // cos(danceSpeed) term shifts, so root.zRot differs from the anim_speed=0
    // case at the same age/counters. Proves that additive term of the formula.
    {
        let age = 0.5;
        let (d0, _) = drive_dance(ids, sed, allay_tid, 20, 1.0, age, 0.0, 0.0, 0.0);
        let (d1, _) = drive_dance(ids, sed, allay_tid, 20, 1.0, age, 0.6, 0.0, 0.0);
        let (dt, st, st0) = sim(20);
        let spin = sim_progress(st, st0, 1.0);
        let (want0, _) = expect_dance(age, 0.0, sim_is_spinning(dt), spin);
        let (want1, _) = expect_dance(age, 0.6, sim_is_spinning(dt), spin);
        let z0 = find(&d0, "root").map(|(r, _)| r[2]);
        let z1 = find(&d1, "root").map(|(r, _)| r[2]);
        let ok = z0.is_some_and(|z| near(z, want0[2], 1e-5))
            && z1.is_some_and(|z| near(z, want1[2], 1e-5))
            && (want0[2] - want1[2]).abs() > 1e-3;
        c.record(
            "c3.dancespeed_includes_animationspeed",
            ok,
            format!(
                "root.zRot amt0={z0:?} (want {:.6}) amt0.6={z1:?} (want {:.6}) — must differ",
                want0[2], want1[2]
            ),
        );
    }
    // Wings/hover intact after the hierarchy restructure. The wing flap
    // (`Anim::AllayWing`) is applied unconditionally, OUTSIDE the dance branch,
    // so both wings must carry the independent flap formula whether the Allay is
    // dancing or still — and identically in both, since the dance never touches
    // them. (The wings are now children of `body`; a broken parenting would
    // leave the local delta right but move the pivot — a *composed* regression
    // the mobshot rest-pose gate catches. Here we pin the local delta.)
    {
        let age = 0.5;
        let want_r = expect_wing(age, 0.0, 0.0, false);
        let want_l = expect_wing(age, 0.0, 0.0, true);
        // Dancing (through the full production path) …
        let (dancing, _) = drive_dance(ids, sed, allay_tid, 20, 1.0, age, 0.0, 0.0, 0.0);
        // … and still (a non-dancing Allay at the same age).
        let still = oracle_part_deltas(
            EntityModelKind::Allay,
            &OracleInputs {
                age_seconds: age,
                allay_dance: None,
                ..Default::default()
            },
        )
        .expect("allay model");
        let rd = find(&dancing, "right_wing").map(|(r, _)| r);
        let ld = find(&dancing, "left_wing").map(|(r, _)| r);
        let rs = find(&still, "right_wing").map(|(r, _)| r);
        let ls = find(&still, "left_wing").map(|(r, _)| r);
        let flapping = want_r[1].abs() > 0.1; // the flap is a real, nonzero motion
        let ok = rd.is_some_and(|r| near3(r, want_r, 1e-5))
            && ld.is_some_and(|l| near3(l, want_l, 1e-5))
            && rs.is_some_and(|r| near3(r, want_r, 1e-5))
            && ls.is_some_and(|l| near3(l, want_l, 1e-5))
            && flapping;
        c.record(
            "c3.wings_intact_and_dance_independent",
            ok,
            format!(
                "dancing R={} L={} still R={} L={} (want R={} L={} in both)",
                rd.map(fmt3).unwrap_or_else(|| "missing".into()),
                ld.map(fmt3).unwrap_or_else(|| "missing".into()),
                rs.map(fmt3).unwrap_or_else(|| "missing".into()),
                ls.map(fmt3).unwrap_or_else(|| "missing".into()),
                fmt3(want_r),
                fmt3(want_l),
            ),
        );
    }
}
