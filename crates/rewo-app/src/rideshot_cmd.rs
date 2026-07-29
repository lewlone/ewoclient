//! `rewo rideshot --check` — the M72 passenger-positioning oracle.
//!
//! Serverless and GPU-less: no socket and no Vulkan device. It builds raw
//! `set_passengers` bodies, pushes them through the production router into a
//! real [`EntityTable`], ticks that table, and reads positions back out of
//! [`EntityState::render_pos`] — the one function every rendered world
//! position in this client comes from.
//!
//! ```text
//! raw set_passengers body (VarInt vehicle + VarInt array of riders)
//!   -> rewo_net::route_set_passengers          (real packet-id selection seam)
//!   -> EntityTable::set_passengers             (the riding graph, M70)
//!   -> EntityTable::tick_lerp                  (which now ends in position_riders)
//!   -> rewo_world::riding::rider_position      (the transcribed arithmetic)
//!   -> EntityState::render_pos                 (what the collector reads)
//! ```
//!
//! The attachment table and the class sets come from
//! `rewo_data::entity_attachments::Attachments::resolve` against the real
//! `registries.json`, so a regenerated table that dropped a column fails here.
//! Vehicle motion is driven through `EntityState::set_target`, which is the
//! writer every position packet calls (`rewo-net`'s movement handling lives
//! inline in `play.rs` and exposes no `route_*` seam to drive instead).
//!
//! **Every witness names its mutation partner in its detail string, and every
//! mutation was run** — see the M72 entry in `REWO_PLAN.md` §15 for which ones
//! failed first. Two of this project's earned lessons shaped them:
//!
//! * *A sample must sit where the mutation bites.* `Spider`'s override tests
//!   `vehicle.getBbWidth() <= this.getBbWidth()`, so `r4.spider_width_bound`
//!   rides a **polar bear**, whose 1.4 width is exactly the spider's — a
//!   `<=` → `<` flip has nowhere to hide. `r1.seat_index_clamp` samples index
//!   3 and 4 on a four-seat vehicle for the same reason.
//! * *A detector must not be able to count something other than its subject.*
//!   Every positional witness asserts a **distance between two entities'
//!   positions**, never "the rider moved" — a rider that moved for its own
//!   reasons (its own stale lerp, which is precisely the pre-M72 bug) passes
//!   the second and fails the first.
//!
//! And one this milestone earned: **a named mutation partner that cannot be
//! reached is not a partner.** `r4.a_rider_that_moved_on` originally claimed
//! `position_riders`' per-rider `vehicle_of` re-check; running that mutation
//! left the gate green, because `set_passengers` maintains both maps together
//! and an inconsistent pair is unreachable by construction. The witness now
//! names the detach that *is* load-bearing, and the re-check is labelled as
//! the belt it actually is — the same status as the cycle walk's visited set.

use clap::Args as ClapArgs;
use rewo_data::entity_attachments::Attachments;
use rewo_data::entity_types::EntityTypes;
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_net::ids::Ids;
use rewo_world::entities::{EntityState, EntityTable};

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 24;

/// Positions are exact `f64` arithmetic on both sides, but the seat rotation
/// runs through `Mth`'s float sine table, so anything rotated carries float
/// error. 1e-5 blocks is four orders of magnitude below the smallest offset
/// any witness discriminates on (0.0625).
const EPS: f64 = 1e-5;

#[derive(ClapArgs)]
pub struct RideshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `eventshot`/`danceshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Version whose `packets.json` / `registries.json` resolve the real
    /// `set_passengers` id and every entity type id below.
    #[arg(long, default_value = "26.2")]
    version: String,
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
        println!("[rideshot] {status}  {name}: {detail}");
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

// ------------------------------------------------------------------ wire bits

fn varint(mut v: i32, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v = ((v as u32) >> 7) as i32;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// `ClientboundSetPassengersPacket` — `readVarInt()` then `readVarIntArray()`.
fn passengers_body(vehicle: i32, riders: &[i32]) -> Vec<u8> {
    let mut b = Vec::new();
    varint(vehicle, &mut b);
    varint(riders.len() as i32, &mut b);
    for r in riders {
        varint(*r, &mut b);
    }
    b
}

// ---------------------------------------------------------------- the fixture

/// Every type id the gate needs, resolved through the production
/// `EntityTypes::id_of`.
struct Types {
    player: i32,
    pig: i32,
    happy_ghast: i32,
    oak_boat: i32,
    oak_chest_boat: i32,
    bamboo_raft: i32,
    minecart: i32,
    villager: i32,
    camel: i32,
    spider: i32,
    polar_bear: i32,
    horse: i32,
    zombie: i32,
}

struct Fixture {
    ids: Ids,
    att: std::sync::Arc<Attachments>,
    t: Types,
}

impl Fixture {
    /// A table with the attachment data installed and `n` entities spawned at
    /// the origin, ids 1..=n, from `spec` (type id, position, yaw).
    fn table(&self, spec: &[(i32, [f64; 3], f32)]) -> EntityTable {
        let mut t = EntityTable::default();
        t.set_attachments(self.att.clone());
        for (i, (tid, pos, yaw)) in spec.iter().enumerate() {
            t.add(
                i as i32 + 1,
                EntityState::new(0, *tid, pos[0], pos[1], pos[2], *yaw, 0.0),
            );
        }
        t
    }

    /// Push a roster through the real router.
    fn mount(&self, t: &mut EntityTable, vehicle: i32, riders: &[i32]) {
        let ok = rewo_net::route_set_passengers(
            self.ids.cb_play_set_passengers,
            &passengers_body(vehicle, riders),
            &self.ids,
            t,
        );
        assert!(ok, "route_set_passengers rejected a well-formed body");
    }

    /// Mount, then tick once so `position_riders` runs.
    fn mounted(&self, spec: &[(i32, [f64; 3], f32)], vehicle: i32, riders: &[i32]) -> EntityTable {
        let mut t = self.table(spec);
        self.mount(&mut t, vehicle, riders);
        t.tick_lerp();
        t
    }
}

fn pos(t: &EntityTable, id: i32, alpha: f32) -> [f64; 3] {
    t.get(id).expect("entity present").render_pos(alpha)
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The rider's position **relative to its vehicle**, which is what every
/// positional witness discriminates on.
fn seat(t: &EntityTable, vehicle: i32, rider: i32, alpha: f32) -> [f64; 3] {
    let v = pos(t, vehicle, alpha);
    let r = pos(t, rider, alpha);
    [r[0] - v[0], r[1] - v[1], r[2] - v[2]]
}

// ------------------------------------------------------------------- the run

pub fn run(args: RideshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[rideshot] mode: {mode} (serverless, no GPU; the oracle asserts \
         unconditionally — a failure exits nonzero with or without --check)"
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let entity_types = EntityTypes::load(&paths.registries_json())?;
    let att = std::sync::Arc::new(Attachments::resolve(&entity_types)?);

    let mut c = Checker::new();

    let id_of = |name: &str| entity_types.id_of(name);
    let want = [
        "minecraft:player",
        "minecraft:pig",
        "minecraft:happy_ghast",
        "minecraft:oak_boat",
        "minecraft:oak_chest_boat",
        "minecraft:bamboo_raft",
        "minecraft:minecart",
        "minecraft:villager",
        "minecraft:camel",
        "minecraft:spider",
        "minecraft:polar_bear",
        "minecraft:horse",
        "minecraft:zombie",
    ];
    let resolved: Vec<Option<i32>> = want.iter().map(|n| id_of(n)).collect();
    let all = resolved.iter().all(|r| r.is_some());
    c.record(
        "r0.every_type_id_resolves_through_the_production_registry",
        all,
        format!(
            "{} of {} resolved via EntityTypes::id_of (want all — a missing type would \
             make its witness silently vacuous). MUTATION PARTNER: r0.attachment_dispatch, \
             which proves the resolved ids reach distinct override branches",
            resolved.iter().filter(|r| r.is_some()).count(),
            want.len()
        ),
    );
    if !all {
        return Err(format!("unresolved entity types: {resolved:?}"));
    }
    let t = Types {
        player: resolved[0].unwrap(),
        pig: resolved[1].unwrap(),
        happy_ghast: resolved[2].unwrap(),
        oak_boat: resolved[3].unwrap(),
        oak_chest_boat: resolved[4].unwrap(),
        bamboo_raft: resolved[5].unwrap(),
        minecart: resolved[6].unwrap(),
        villager: resolved[7].unwrap(),
        camel: resolved[8].unwrap(),
        spider: resolved[9].unwrap(),
        polar_bear: resolved[10].unwrap(),
        horse: resolved[11].unwrap(),
        zombie: resolved[12].unwrap(),
    };

    // The class dispatch must actually separate the branches, or every
    // override witness below would be testing the default path.
    use rewo_data::entity_attachments::VehicleClass;
    let dispatch = [
        (t.pig, VehicleClass::Default),
        (t.oak_boat, VehicleClass::Boat { raft: false, chest: false }),
        (t.oak_chest_boat, VehicleClass::Boat { raft: false, chest: true }),
        (t.bamboo_raft, VehicleClass::Boat { raft: true, chest: false }),
        (t.minecart, VehicleClass::Minecart),
        (t.camel, VehicleClass::Camel),
        (t.horse, VehicleClass::Horse),
    ];
    let bad: Vec<_> = dispatch
        .iter()
        .filter(|(id, want)| att.class(*id) != *want)
        .map(|(id, want)| (*id, att.class(*id), *want))
        .collect();
    c.record(
        "r0.attachment_dispatch_selects_the_declaring_class",
        bad.is_empty(),
        format!(
            "mismatches={bad:?} (want none). The order is most-derived-first, so a camel — \
             which IS an AbstractHorse — must come back Camel. MUTATION PARTNER: reorder the \
             chain so Horse precedes Camel and the camel row flips",
        ),
    );

    println!(
        "[rideshot] set_passengers id = {}; player={} pig={} happy_ghast={} boat={} raft={} \
         minecart={} camel={} spider={} polar_bear={} horse={}",
        ids.cb_play_set_passengers,
        t.player,
        t.pig,
        t.happy_ghast,
        t.oak_boat,
        t.bamboo_raft,
        t.minecart,
        t.camel,
        t.spider,
        t.polar_bear,
        t.horse
    );

    let f = Fixture { ids, att, t };
    check_seat(&f, &mut c);
    check_interpolation(&f, &mut c);
    check_overrides(&f, &mut c);
    check_graph(&f, &mut c);

    println!(
        "[rideshot] witnesses observed: {} / {}",
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
    println!("[rideshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ------------------------------------------------- r1: where the seat lands

fn check_seat(f: &Fixture, c: &mut Checker) {
    let t = &f.t;

    // A player on a pig. The pig declares `passengerAttachments(0.86875F)` and
    // the player declares `vehicleAttachment(0, 0.6, 0)`.
    let tab = f.mounted(
        &[(t.pig, [10.0, 64.0, -3.0], 0.0), (t.player, [0.0, 0.0, 0.0], 0.0)],
        1,
        &[2],
    );
    let s = seat(&tab, 1, 2, 1.0);
    let want_y = 0.86875f32 as f64 - 0.6f32 as f64;
    c.record(
        "r1.the_seat_is_the_declared_point_not_the_bounding_box_top",
        (s[1] - want_y).abs() < EPS && s[0].abs() < EPS && s[2].abs() < EPS,
        format!(
            "seat={s:?} want [0, {want_y}, 0] — 0.86875 declared minus the player's own \
             0.6 VEHICLE point. MUTATION PARTNER: the AT_HEIGHT fallback would give the \
             pig's bbox top 0.9, which differs by 0.03125 and is what dropping the \
             declaration produces"
        ),
    );

    // The same mount with a rider that declares no VEHICLE point at all.
    let tab2 = f.mounted(
        &[(t.pig, [0.0, 0.0, 0.0], 0.0), (t.villager, [0.0, 0.0, 0.0], 0.0)],
        1,
        &[2],
    );
    let sv = seat(&tab2, 1, 2, 1.0);
    c.record(
        "r1.the_riders_own_vehicle_point_is_subtracted",
        (sv[1] - 0.86875f32 as f64).abs() < EPS && (sv[1] - s[1]).abs() > 0.5,
        format!(
            "villager seat={sv:?} vs player seat={s:?} — the villager declares no VEHICLE \
             point (AT_FEET) and sits 0.6 higher on the same mount. MUTATION PARTNER: drop \
             the subtraction and the two coincide, which is the bug that would put every \
             mounted player on the horse's head"
        ),
    );

    // Four seats on the happy ghast, and the clamp.
    let ghast = [
        (t.happy_ghast, [0.0, 0.0, 0.0], 0.0),
        (t.player, [0.0, 0.0, 0.0], 0.0),
        (t.player, [0.0, 0.0, 0.0], 0.0),
        (t.player, [0.0, 0.0, 0.0], 0.0),
        (t.player, [0.0, 0.0, 0.0], 0.0),
        (t.player, [0.0, 0.0, 0.0], 0.0),
    ];
    let tab = f.mounted(&ghast, 1, &[2, 3, 4, 5, 6]);
    let seats: Vec<[f64; 3]> = (2..=6).map(|r| seat(&tab, 1, r, 1.0)).collect();
    // `passengerAttachments(new Vec3(0,4,1.7), (-1.7,4,0), (0,4,-1.7), (1.7,4,0))`.
    let want = [
        [0.0, 1.7f64],
        [-1.7, 0.0],
        [0.0, -1.7],
        [1.7, 0.0],
    ];
    let matched = seats
        .iter()
        .take(4)
        .zip(want.iter())
        .all(|(s, w)| (s[0] - w[0]).abs() < EPS && (s[2] - w[1]).abs() < EPS);
    c.record(
        "r1.the_nth_passenger_takes_the_nth_declared_seat",
        matched,
        format!(
            "seats(xz)={:?} want {want:?} — four distinct seats around the ghast. MUTATION \
             PARTNER: index every rider at 0 and all four collapse onto (0, 1.7), which is \
             exactly how riders stack",
            seats.iter().take(4).map(|s| [s[0], s[2]]).collect::<Vec<_>>()
        ),
    );
    c.record(
        "r1.seat_index_clamp",
        dist(seats[4], seats[3]) < EPS && dist(seats[3], seats[2]) > 0.5,
        format!(
            "fifth={:?} fourth={:?} third={:?} — `getClamped` puts the fifth rider on the \
             LAST seat, not the first and not off the model. The sample sits ON the bound \
             (index 3 = size-1, index 4 = size): a `min(index, len-1)` → `min(index, len)` \
             mutation panics and a wrap-to-0 mutation moves the fifth onto seat 0",
            seats[4], seats[3], seats[2]
        ),
    );

    // Rotation. Seat 0 is `+1.7 z`; a quarter turn must put it on `-x`.
    let turned = f.mounted(
        &[(t.happy_ghast, [0.0, 0.0, 0.0], 90.0), (t.player, [0.0; 3], 0.0)],
        1,
        &[2],
    );
    let st = seat(&turned, 1, 2, 1.0);
    c.record(
        "r1.the_seat_rotates_with_the_vehicles_yaw",
        (st[0] + 1.7).abs() < 1e-3 && st[2].abs() < 1e-3,
        format!(
            "seat at yaw 90 = {st:?} want [-1.7, _, 0] — `transformPoint` rotates by \
             **negated** degrees. MUTATION PARTNER: drop the negation and the seat lands at \
             x=+1.7, the mirror; drop the rotation entirely and it stays at z=+1.7"
        ),
    );

    // The rider-side rotation. `getVehicleAttachmentPoint` rotates the RIDER's
    // point by the RIDER's yaw — but no discriminating sample exists, and that
    // is itself the checkable fact: every VEHICLE point 26.2 declares comes
    // from `ridingOffset(float)` or `Avatar.DEFAULT_VEHICLE_ATTACHMENT`, both
    // of which build `(0, y, 0)`. A pure-y vector is invariant under `yRot`,
    // so which yaw is used is UNOBSERVABLE in vanilla data.
    //
    // Asserting the invariant instead of faking a sample is the honest move:
    // if a future version declares an off-axis VEHICLE point this fails, and
    // the failure says "now go build the discriminating case" rather than
    // silently starting to matter.
    let off_axis: Vec<&str> = rewo_data::entity_attachments_table::TYPES
        .iter()
        .filter_map(|r| {
            r.vehicle
                .filter(|v| v[0] != 0.0 || v[2] != 0.0)
                .map(|_| r.name)
        })
        .collect();
    let spun = f.mounted(
        &[(t.happy_ghast, [0.0; 3], 0.0), (t.player, [0.0; 3], 137.0)],
        1,
        &[2],
    );
    let ss = seat(&spun, 1, 2, 1.0);
    c.record(
        "r1.every_vehicle_point_is_on_the_y_axis_so_the_rider_side_rotation_is_unobservable",
        off_axis.is_empty() && dist(ss, seats[0]) < EPS,
        format!(
            "off-axis VEHICLE points in the generated table: {off_axis:?} (want none); and \
             a rider turned to yaw 137 sits at {ss:?}, identical to yaw 0. MUTATION \
             PARTNER: the invariant itself — the moment a type declares an off-axis point \
             this witness fails and the second clause becomes a real discriminator, which \
             is exactly when it needs to. Rotating by the vehicle's yaw instead would be \
             indistinguishable today"
        ),
    );
}

// ------------------------------------- r2: it does not interpolate its own way

fn check_interpolation(f: &Fixture, c: &mut Checker) {
    let t = &f.t;
    let spec = [
        (t.pig, [0.0, 64.0, 0.0], 0.0),
        (t.player, [0.0, 64.0, 0.0], 0.0),
    ];

    // Move the vehicle a long way, then walk the 3-tick lerp.
    let mut tab = f.table(&spec);
    f.mount(&mut tab, 1, &[2]);
    tab.tick_lerp();
    tab.get_mut(1).unwrap().set_target(30.0, 70.0, -20.0);
    // The rider is ALSO told to go somewhere else entirely. Vanilla computes
    // its lerp and then overwrites it; nothing of it may reach the screen.
    tab.get_mut(2).unwrap().set_target(-500.0, 5.0, 900.0);

    let mut worst_at_ticks = 0.0f64;
    let mut worst_sub = 0.0f64;
    let expected = [0.0, 0.86875f32 as f64 - 0.6f32 as f64, 0.0];
    for _ in 0..3 {
        tab.tick_lerp();
        // The full sub-tick sweep, not just the tick boundary: a rider derived
        // once per tick and lerped between two derived positions holds the
        // offset at EVERY fraction when the yaw is constant. That equality is
        // what "follows without jitter" means, and it is checkable.
        for step in 0..=8 {
            let a = step as f32 / 8.0;
            let s = seat(&tab, 1, 2, a);
            let d = dist(s, expected);
            if a == 1.0 {
                worst_at_ticks = worst_at_ticks.max(d);
            } else {
                worst_sub = worst_sub.max(d);
            }
        }
    }
    c.record(
        "r2.the_rider_holds_its_seat_across_sub_tick_fractions",
        worst_sub < EPS && worst_at_ticks < EPS,
        format!(
            "worst deviation from the seat: {worst_sub:.2e} at sub-tick fractions, \
             {worst_at_ticks:.2e} on tick boundaries, over a 3-tick lerp across ~37 blocks \
             (want both < {EPS:.0e}). MUTATION PARTNER: r2.no_table_no_tracking below, the \
             same run with the attachment table absent"
        ),
    );

    let final_rider = pos(&tab, 2, 1.0);
    c.record(
        "r2.the_riders_own_synced_target_never_reaches_the_screen",
        dist(final_rider, [-500.0, 5.0, 900.0]) > 100.0,
        format!(
            "rider rendered at {final_rider:?} after being told to go to \
             [-500, 5, 900] (want nowhere near it). `rideTick` ticks the passenger and \
             THEN calls positionRider, so its own lerp is computed and discarded. \
             MUTATION PARTNER: r2.dismount_restores, where the same target does take effect"
        ),
    );

    // The control: the identical run with no attachment table.
    let mut plain = EntityTable::default();
    for (i, (tid, p, yaw)) in spec.iter().enumerate() {
        plain.add(
            i as i32 + 1,
            EntityState::new(0, *tid, p[0], p[1], p[2], *yaw, 0.0),
        );
    }
    f.mount(&mut plain, 1, &[2]);
    plain.tick_lerp();
    plain.get_mut(1).unwrap().set_target(30.0, 70.0, -20.0);
    plain.get_mut(2).unwrap().set_target(-500.0, 5.0, 900.0);
    let mut worst_plain = 0.0f64;
    for _ in 0..3 {
        plain.tick_lerp();
        worst_plain = worst_plain.max(dist(seat(&plain, 1, 2, 1.0), expected));
    }
    c.record(
        "r2.no_table_no_tracking",
        worst_plain > 100.0,
        format!(
            "without the attachment table the same rider ends {worst_plain:.1} blocks off \
             its seat (want > 100 — this is the pre-M72 behaviour, and it is what makes \
             r2's first two witnesses non-vacuous)"
        ),
    );

    // Dismount: an empty roster returns the rider to its own positioning.
    let mut tab = f.table(&spec);
    f.mount(&mut tab, 1, &[2]);
    tab.tick_lerp();
    tab.get_mut(1).unwrap().set_target(50.0, 64.0, 0.0);
    for _ in 0..3 {
        tab.tick_lerp();
    }
    let while_ridden = pos(&tab, 2, 1.0);
    f.mount(&mut tab, 1, &[]);
    tab.get_mut(2).unwrap().set_target(0.0, 64.0, 0.0);
    for _ in 0..3 {
        tab.tick_lerp();
    }
    let after = pos(&tab, 2, 1.0);
    c.record(
        "r2.dismounting_restores_independent_positioning",
        while_ridden[0] > 40.0 && after[0].abs() < EPS,
        format!(
            "rider x while ridden = {:.4}, after an empty roster + its own target = {:.4} \
             (want ~50 then ~0). MUTATION PARTNER: treat an empty roster as a no-op — a \
             very easy decode to get wrong, since it is the only thing that ends a ride — \
             and the rider stays welded to the vehicle forever",
            while_ridden[0], after[0]
        ),
    );
}

// ------------------------------------------------------- r3: the overrides

fn check_overrides(f: &Fixture, c: &mut Checker) {
    let t = &f.t;
    let mount = |vehicle_type: i32, riders: &[i32], n: usize| {
        let mut spec = vec![(vehicle_type, [0.0, 0.0, 0.0], 0.0)];
        for r in riders.iter().take(n) {
            spec.push((*r, [0.0, 0.0, 0.0], 0.0));
        }
        spec
    };

    // A boat replaces the lookup. It declares NO passenger points, so the
    // default path would give the AT_HEIGHT fallback 0.5625.
    let ids: Vec<i32> = (2..=2).collect();
    let tab = f.mounted(&mount(t.oak_boat, &[t.player], 1), 1, &ids);
    let s = seat(&tab, 1, 2, 1.0);
    let ride_height = 0.5625f64 / 3.0;
    c.record(
        "r3.a_boat_replaces_the_lookup_rather_than_falling_back",
        (s[1] - (ride_height - 0.6f32 as f64)).abs() < EPS && s[2].abs() < EPS,
        format!(
            "boat seat={s:?} want y={} — `rideHeight` is height/3 = {ride_height:.4}. \
             MUTATION PARTNER: take the default path and the AT_HEIGHT fallback gives \
             0.5625, three times as high; a lone passenger also sits amidships (z=0) where \
             the two-seat branch would put it at 0.2",
            ride_height - 0.6f32 as f64
        ),
    );

    // A raft's rideHeight is a different formula on the same bounding box.
    let raft = f.mounted(&mount(t.bamboo_raft, &[t.player], 1), 1, &ids);
    let sr = seat(&raft, 1, 2, 1.0);
    let raft_height = 0.5625f64 * 0.8888889f32 as f64;
    c.record(
        "r3.a_raft_rides_higher_than_a_boat_of_the_same_size",
        (sr[1] - (raft_height - 0.6f32 as f64)).abs() < EPS && sr[1] > s[1] + 0.3,
        format!(
            "raft seat y={:.5} vs boat {:.5} — both are 0.5625 tall, and the split is by \
             LEAF class (`Raft` and `ChestRaft` share it ACROSS the chest boundary, so it \
             is not the chest/plain split it looks like). MUTATION PARTNER: use height/3 \
             for rafts too and the two coincide",
            sr[1], s[1]
        ),
    );

    // A chest boat shifts its lone passenger forward.
    let chest = f.mounted(&mount(t.oak_chest_boat, &[t.player], 1), 1, &ids);
    let sc = seat(&chest, 1, 2, 1.0);
    c.record(
        "r3.a_chest_boat_shifts_its_lone_passenger_forward",
        (sc[2] - 0.15).abs() < EPS && s[2].abs() < EPS,
        format!(
            "chest boat z={:.4} vs plain boat z={:.4} (want 0.15 then 0). \
             `AbstractChestBoat.getSinglePassengerXOffset` is 0.15 against `AbstractBoat`'s \
             0.0 — the chest takes the stern. MUTATION PARTNER: the plain boat in the same \
             comparison, which must stay amidships",
            sc[2], s[2]
        ),
    );

    // Two in a boat split fore and aft, and an animal is nudged forward.
    let two = f.mounted(&mount(t.oak_boat, &[t.player, t.player], 2), 1, &[2, 3]);
    let fore = seat(&two, 1, 2, 1.0);
    let aft = seat(&two, 1, 3, 1.0);
    let with_pig = f.mounted(&mount(t.oak_boat, &[t.player, t.pig], 2), 1, &[2, 3]);
    let pig_aft = seat(&with_pig, 1, 3, 1.0);
    c.record(
        "r3.a_boats_second_seat_and_its_animal_bump",
        (fore[2] - 0.2).abs() < EPS
            && (aft[2] + 0.6).abs() < EPS
            && (pig_aft[2] + 0.4).abs() < EPS,
        format!(
            "fore z={:.4} aft z={:.4} pig-aft z={:.4} (want 0.2, -0.6, -0.4). The +0.2 is \
             `passenger instanceof Animal`, read of the RIDER not the vehicle. MUTATION \
             PARTNER: the player in the same aft seat, which must stay at -0.6",
            fore[2], aft[2], pig_aft[2]
        ),
    );

    // A villager in a minecart drops to the cart's feet.
    let cart_v = f.mounted(&mount(t.minecart, &[t.villager], 1), 1, &ids);
    let cart_p = f.mounted(&mount(t.minecart, &[t.zombie], 1), 1, &ids);
    let sv = seat(&cart_v, 1, 2, 1.0);
    let sp = seat(&cart_p, 1, 2, 1.0);
    c.record(
        "r3.a_villager_in_a_minecart_rides_at_the_carts_feet",
        sv[1].abs() < EPS && (sp[1] - (0.1875f32 as f64 - 0.7f32 as f64)).abs() < EPS,
        format!(
            "villager seat y={:.5} (villager declares no VEHICLE point, so 0 exactly), \
             zombie seat y={:.5} = 0.1875 declared − its own 0.7. MUTATION PARTNER: the \
             zombie in the identical cart — if `LOWERED_PASSENGER_ATTACHMENT` were applied \
             to everyone, or to nobody, the two would agree",
            sv[1], sp[1]
        ),
    );

    // A camel declares nothing, so the fallback would be its 2.375 bbox top.
    let camel = f.mounted(&mount(t.camel, &[t.player], 1), 1, &ids);
    let sca = seat(&camel, 1, 2, 1.0);
    let want = 2.375f32 as f64 - 0.375 - 0.6f32 as f64;
    c.record(
        "r3.a_camel_uses_its_body_anchor_not_the_bounding_box_top",
        (sca[1] - want).abs() < EPS && (sca[2] - 0.5).abs() < EPS,
        format!(
            "camel seat={sca:?} want y={want:.5} z=0.5 — `getBodyAnchorAnimationYOffset` at \
             rest is `height − 0.375`. MUTATION PARTNER: the AT_HEIGHT fallback, 0.375 \
             higher, which is what a camel gets if its override is not implemented at all \
             — and unlike a horse's rearing term this one does NOT vanish at rest"
        ),
    );
}

// ------------------------------------------------ r4: the graph and the rider

fn check_graph(f: &Fixture, c: &mut Checker) {
    let t = &f.t;

    // A rider of a rider. The pig carries a player; the player carries a pig.
    let spec = [
        (t.happy_ghast, [0.0, 100.0, 0.0], 0.0),
        (t.player, [0.0, 0.0, 0.0], 0.0),
        (t.pig, [0.0, 0.0, 0.0], 0.0),
    ];
    let mut tab = f.table(&spec);
    f.mount(&mut tab, 1, &[2]);
    f.mount(&mut tab, 2, &[3]);
    tab.tick_lerp();
    let mid = seat(&tab, 1, 2, 1.0);
    let top = seat(&tab, 2, 3, 1.0);
    // The player's seat on the ghast, then the pig's on the player: the pig
    // declares no VEHICLE point, so it sits at the player's AT_HEIGHT (1.8).
    c.record(
        "r4.a_rider_of_a_rider_is_positioned_after_its_own_vehicle",
        (mid[2] - 1.7).abs() < 1e-3 && (top[1] - 1.8f32 as f64).abs() < EPS,
        format!(
            "player-on-ghast={mid:?}, pig-on-player={top:?} (want z≈1.7 then y≈1.8). The \
             second is measured against the player's DERIVED position, so it can only be \
             right if the walk positioned the player first. MUTATION PARTNER: seed the \
             stack from every vehicle rather than from the roots and the pig lands on the \
             player's stale position one tick behind"
        ),
    );

    // A malformed roster describing a cycle. Reaching the `record` call at all
    // proves termination; the assertion is the stronger claim underneath —
    // that a chain which re-enters a node already positioned this tick stops
    // there rather than re-deriving it from a position it just wrote.
    //
    let mut cyc = f.table(&[
        (t.pig, [11.0, 0.0, 0.0], 0.0),
        (t.pig, [22.0, 0.0, 0.0], 0.0),
    ]);
    f.mount(&mut cyc, 1, &[2]);
    f.mount(&mut cyc, 2, &[1]);
    cyc.tick_lerp();
    let (a, b) = (pos(&cyc, 1, 1.0)[0], pos(&cyc, 2, 1.0)[0]);
    c.record(
        "r4.a_cyclic_roster_terminates_and_moves_nobody",
        (a - 11.0).abs() < EPS && (b - 22.0).abs() < EPS,
        format!(
            "two entities each named as the other's passenger; both stayed where they \
             spawned (x = {a:.4}, {b:.4}). Neither is a ROOT — a root carries passengers \
             and rides nothing — so the walk never starts, which is the real guard: \
             consistent `vehicle_of` makes a cycle unreachable from any root, and the \
             visited set is belt to that braces. MUTATION PARTNER: seed the walk from \
             every vehicle instead of from the roots and this hangs — a hang, not a wrong \
             number, which is why the assertion has to be a position and not merely 'it \
             finished'"
        ),
    );

    // A roster naming someone who rides elsewhere positions nobody:
    // `tickPassenger` starts with `getVehicle() != vehicle -> stopRiding()`.
    let mut moved = f.table(&[
        (t.happy_ghast, [0.0, 100.0, 0.0], 0.0),
        (t.pig, [0.0, 200.0, 0.0], 0.0),
        (t.player, [0.0, 0.0, 0.0], 0.0),
    ]);
    f.mount(&mut moved, 1, &[3]);
    f.mount(&mut moved, 2, &[3]); // the player switches mounts
    moved.tick_lerp();
    let on_pig = pos(&moved, 3, 1.0);
    let ghast_still_ridden = moved.is_vehicle(1);
    c.record(
        "r4.a_rider_that_moved_on_is_positioned_by_its_new_vehicle_only",
        (on_pig[1] - (200.0 + 0.86875f32 as f64 - 0.6f32 as f64)).abs() < EPS
            && !ghast_still_ridden,
        format!(
            "player at y={:.5} (want the pig's seat at 200, not the ghast's at 100), and \
             the ghast reads ridden={ghast_still_ridden} (want false). The second clause \
             is the load-bearing one: `handleSetEntityPassengers` calls `startRiding`, \
             which DETACHES first, so the old roster is emptied and the walk never reaches \
             the stale entry. MUTATION PARTNER: drop that detach in \
             `EntityTable::set_passengers` and the rider sits in two rosters at once, \
             where which one wins is HashMap iteration order — a coin flip. \
             (`position_riders`' own per-rider `vehicle_of` re-check, which mirrors \
             `tickPassenger`'s `getVehicle() != vehicle` guard, is NOT what this witness \
             proves: with both maps consistently maintained it is unreachable, and a \
             mutation removing it leaves this green. It is kept as vanilla's own belt, \
             like the visited set — see r4.a_cyclic_roster.)",
            on_pig[1]
        ),
    );

    // `Spider.getVehicleAttachmentPoint` — the bound is `<=`, and the polar
    // bear's width is EXACTLY the spider's.
    let on_bear = f.mounted(
        &[(t.polar_bear, [0.0; 3], 0.0), (t.spider, [0.0; 3], 0.0)],
        1,
        &[2],
    );
    let on_ghast = f.mounted(
        &[(t.happy_ghast, [0.0; 3], 0.0), (t.spider, [0.0; 3], 0.0)],
        1,
        &[2],
    );
    let bear = seat(&on_bear, 1, 2, 1.0);
    let ghast = seat(&on_ghast, 1, 2, 1.0);
    c.record(
        "r4.spider_width_bound",
        (bear[1] - (1.4f32 as f64 - 0.3125)).abs() < EPS
            && (ghast[1] - 4.0).abs() < EPS,
        format!(
            "on a polar bear seat y={:.5} (bear height 1.4 − the 0.3125 override), on a \
             happy ghast y={:.5} (4.0 − 0, the AT_FEET default). The bear's width is \
             EXACTLY 1.4, the spider's own, so the sample sits ON the `<=` bound and a \
             `<` mutation flips the first row; the ghast at 4.0 wide is the far side. \
             MUTATION PARTNER: each other",
            bear[1], ghast[1]
        ),
    );

    // The body-yaw force, and the `instanceof LivingEntity` that gates it.
    let mut horse = f.table(&[
        (t.horse, [0.0; 3], 42.0),
        (t.player, [0.0; 3], 137.0),
    ]);
    f.mount(&mut horse, 1, &[2]);
    horse.tick_lerp();
    let rider = horse.get(2).unwrap();
    let (yaw, head) = (rider.yaw, rider.head_yaw);
    c.record(
        "r4.a_horse_forces_its_riders_body_yaw_but_not_its_head",
        (yaw - 42.0).abs() < 1e-4 && (head - 137.0).abs() < 1e-4,
        format!(
            "rider yaw={yaw} head_yaw={head} (want 42 from the horse, 137 its own). \
             `positionRider` assigns `yBodyRot` and nothing else, which is why a player on \
             a horse can look sideways. MUTATION PARTNER: r4.only_some_vehicles_force_it"
        ),
    );

    let mut ghastv = f.table(&[
        (t.happy_ghast, [0.0; 3], 42.0),
        (t.player, [0.0; 3], 137.0),
    ]);
    f.mount(&mut ghastv, 1, &[2]);
    ghastv.tick_lerp();
    let free = ghastv.get(2).unwrap().yaw;
    let mut boat_rider = f.table(&[
        (t.horse, [0.0; 3], 42.0),
        (t.oak_boat, [0.0; 3], 137.0),
    ]);
    f.mount(&mut boat_rider, 1, &[2]);
    boat_rider.tick_lerp();
    let not_living = boat_rider.get(2).unwrap().yaw;
    c.record(
        "r4.only_some_vehicles_force_it_and_only_onto_a_living_rider",
        (free - 137.0).abs() < 1e-4 && (not_living - 137.0).abs() < 1e-4,
        format!(
            "on a happy ghast the rider keeps yaw={free}; a BOAT riding a horse keeps \
             yaw={not_living} (want 137 both). The first is the vehicle-class gate, the \
             second the `passenger instanceof LivingEntity` cast — a boat is an `Entity` \
             and not a `LivingEntity`, so the assignment never runs. MUTATION PARTNER: \
             r4.a_horse_forces..., where both gates pass and the yaw does change"
        ),
    );
}
