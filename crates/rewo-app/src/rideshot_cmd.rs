//! `rewo rideshot --check` — the derived-position and entity-link oracle
//! (M72 passenger positioning, M77 minecart schedule / leash / projectile
//! power).
//!
//! Serverless and GPU-less: no socket and no Vulkan device. It builds raw
//! `set_passengers` bodies, pushes them through the production router into a
//! real [`EntityTable`], ticks that table, and reads positions back out of
//! [`EntityState::render_pos`] — the one function every rendered world
//! position in this client comes from.
//!
//! **M77 extends it rather than adding a command** because its three packets
//! land in the same place: two of them write a *derived* position or an
//! entity→entity relation, and the minecart schedule is the second thing in
//! this client (after `positionRider`) that overrides `render_pos`. The `m*`
//! witnesses below share this file's fixture, its router-driven shape and its
//! rule that a positional assertion measures a *relationship* between two
//! positions rather than "something moved".
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
use rewo_data::entity_types::{EntityClasses, EntityTypes};
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_net::ids::Ids;
use rewo_world::entities::{EntityState, EntityTable};

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 45;

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

/// One `NewMinecartBehavior.MinecartStep` as the gate authors it — rotations
/// stay **raw bytes**, so `Mth.unpackDegrees` is under test rather than
/// re-implemented on the encoding side.
#[derive(Clone, Copy)]
struct WireStep {
    position: [f64; 3],
    movement: [f64; 3],
    y_rot: i8,
    x_rot: i8,
    weight: f32,
}

impl WireStep {
    /// A step at `(x, y, z)` with no movement and no rotation.
    fn at(position: [f64; 3], weight: f32) -> Self {
        Self {
            position,
            movement: [0.0; 3],
            y_rot: 0,
            x_rot: 0,
            weight,
        }
    }
}

/// `ClientboundMoveMinecartPacket` — `VAR_INT` then
/// `MinecartStep.STREAM_CODEC.apply(list())`: a var-int count, then per step
/// two `Vec3.STREAM_CODEC` (three big-endian **f64** each), two
/// `ROTATION_BYTE`, one big-endian f32. 54 bytes an element.
fn minecart_body(eid: i32, steps: &[WireStep]) -> Vec<u8> {
    let mut b = Vec::new();
    varint(eid, &mut b);
    varint(steps.len() as i32, &mut b);
    for s in steps {
        for v in s.position {
            b.extend_from_slice(&v.to_be_bytes());
        }
        for v in s.movement {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.push(s.y_rot as u8);
        b.push(s.x_rot as u8);
        b.extend_from_slice(&s.weight.to_be_bytes());
    }
    b
}

/// `ClientboundSetEntityLinkPacket` — `readInt()` twice, so **two fixed
/// big-endian i32s** among a protocol that is otherwise var-ints.
fn entity_link_body(source: i32, dest: i32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&source.to_be_bytes());
    b.extend_from_slice(&dest.to_be_bytes());
    b
}

/// `ClientboundProjectilePowerPacket` — `readVarInt()` then `readDouble()`.
fn projectile_power_body(eid: i32, power: f64) -> Vec<u8> {
    let mut b = Vec::new();
    varint(eid, &mut b);
    b.extend_from_slice(&power.to_be_bytes());
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
    // M77.
    armor_stand: i32,
    fireball: i32,
    arrow: i32,
}

struct Fixture {
    ids: Ids,
    att: std::sync::Arc<Attachments>,
    /// M77's three packets all gate on a Java class the wire cannot carry, so
    /// the gate resolves the same machine-extracted table the client does.
    classes: EntityClasses,
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

    /// Push a step list through the real router. Returns whether the router
    /// claimed the id — which is a different question from whether it applied
    /// anything, and the two witnesses below separate them.
    fn send_steps(&self, t: &mut EntityTable, eid: i32, steps: &[WireStep]) -> bool {
        rewo_net::route_move_minecart_along_track(
            self.ids.cb_play_move_minecart_along_track,
            &minecart_body(eid, steps),
            &self.ids,
            t,
            Some(&self.classes),
        )
    }

    fn send_link(&self, t: &mut EntityTable, source: i32, dest: i32) -> bool {
        rewo_net::route_set_entity_link(
            self.ids.cb_play_set_entity_link,
            &entity_link_body(source, dest),
            &self.ids,
            t,
            Some(&self.classes),
        )
    }

    fn send_power(&self, t: &mut EntityTable, eid: i32, power: f64) -> bool {
        rewo_net::route_projectile_power(
            self.ids.cb_play_projectile_power,
            &projectile_power_body(eid, power),
            &self.ids,
            t,
            Some(&self.classes),
        )
    }
}

/// How many steps are queued for this cart's next segment — `0` for a cart
/// with no schedule at all, which is what "the packet was inert" looks like.
fn inbox(t: &EntityTable, id: i32) -> usize {
    t.minecart_lerp(id).map_or(0, |l| l.inbox_len())
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
    // M77. Resolving this is itself an assertion: `EntityClasses::resolve`
    // hard-fails on a generated name the registry does not hold, on a mob that
    // is not leashable, on a leashable set no larger than the mob set, and on
    // a hurting projectile that is also living.
    let classes = EntityClasses::resolve(&entity_types)?;

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
        // M77's three class boundaries: an armour stand is living but not a
        // `Mob` (so not `Leashable`), a fireball is an
        // `AbstractHurtingProjectile`, and an arrow is an `AbstractArrow` —
        // the sibling branch that looks like a projectile and is not one.
        "minecraft:armor_stand",
        "minecraft:fireball",
        "minecraft:arrow",
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
        armor_stand: resolved[13].unwrap(),
        fireball: resolved[14].unwrap(),
        arrow: resolved[15].unwrap(),
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

    let f = Fixture {
        ids,
        att,
        classes,
        t,
    };
    check_seat(&f, &mut c);
    check_interpolation(&f, &mut c);
    check_overrides(&f, &mut c);
    check_graph(&f, &mut c);
    check_minecart_wire(&f, &mut c);
    check_minecart_schedule(&f, &mut c);
    check_leash(&f, &mut c);
    check_projectile_power(&f, &mut c);

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

// =========================================================================
// M77 — `move_minecart_along_track`, `set_entity_link`, `projectile_power`
// =========================================================================

/// A table with the attachment data installed and entities at **explicit**
/// ids, which the M77 witnesses need: one of them turns on an id being large
/// enough that a var-int reading of a fixed big-endian i32 lands somewhere
/// else, and [`Fixture::table`] only ever spawns `1..=n`.
fn table_at(f: &Fixture, spec: &[(i32, i32, [f64; 3])]) -> EntityTable {
    let mut t = EntityTable::default();
    t.set_attachments(f.att.clone());
    for (id, type_id, p) in spec {
        t.add(*id, EntityState::new(0, *type_id, p[0], p[1], p[2], 0.0, 0.0));
    }
    t
}

/// The L-shaped two-step segment every composition witness rides.
///
/// **A sample must sit where the mutation bites**, and this is the shape that
/// puts it there. Within one step index the schedule's `indexedPartialTick` is
/// affine in the partial tick, so a single-step segment makes the schedule and
/// the generic `xOld -> getX()` chord *identical at every alpha* and would
/// witness nothing at all. They separate only across a step boundary — hence
/// weights `3` and `1`, which put the boundary two thirds of the way through
/// the third tick, and a right-angle turn, which makes the disagreement a
/// whole 1.76 blocks rather than a rounding difference.
fn l_segment() -> [WireStep; 2] {
    [
        WireStep::at([10.0, 0.0, 0.0], 3.0),
        WireStep::at([10.0, 0.0, 10.0], 1.0),
    ]
}

fn check_minecart_wire(f: &Fixture, c: &mut Checker) {
    // ---- m1: `lerpSteps.addAll(...)` is an append. ----
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    f.send_steps(&mut t, 1, &[WireStep::at([1.0, 0.0, 0.0], 1.0)]);
    let after_one = inbox(&t, 1);
    f.send_steps(
        &mut t,
        1,
        &[
            WireStep::at([2.0, 0.0, 0.0], 1.0),
            WireStep::at([3.0, 0.0, 0.0], 1.0),
        ],
    );
    let after_three = inbox(&t, 1);
    c.record(
        "m1.a_step_list_is_appended_not_replaced",
        after_one == 1 && after_three == 3,
        format!(
            "inbox {after_one} then {after_three} (want 1 then 3). \
             `handleMinecartAlongTrack` is `lerpSteps.addAll(packet.lerpSteps())` — two \
             packets between two client ticks are ONE segment, which is what lets the \
             client miss a tick without losing a step. MUTATION PARTNER: \
             m2.a_truncated_body_applies_nothing, where the count is the same and the \
             inbox must NOT grow"
        ),
    );

    // ---- m2: a body one byte short applies nothing at all. ----
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    let mut body = minecart_body(1, &l_segment());
    let full_len = body.len();
    body.pop();
    let claimed = rewo_net::route_move_minecart_along_track(
        f.ids.cb_play_move_minecart_along_track,
        &body,
        &f.ids,
        &mut t,
        Some(&f.classes),
    );
    c.record(
        "m2.a_truncated_body_applies_nothing",
        claimed && inbox(&t, 1) == 0,
        format!(
            "claimed={claimed} inbox={} after a {}-byte body (the whole one is {full_len}). \
             The step list is positional, so a short read means the steps we DID get are \
             not the steps the server sent — half a schedule would drag the cart to a \
             place it never was. The router still claims the id: matching and applying \
             are different questions. MUTATION PARTNER: m1, where the same encoder \
             produces a whole body and the inbox does grow",
            inbox(&t, 1),
            body.len()
        ),
    );

    // ---- m3: the element stride is exactly 54 bytes of full doubles. ----
    let two = [
        WireStep::at([1.0, 2.0, 3.0], 1.0),
        WireStep::at([-4.5, 5.25, 6.125], 2.0),
    ];
    let whole = minecart_body(9, &two);
    let parsed = rewo_net::parse_move_minecart(&whole);
    let short = rewo_net::parse_move_minecart(&whole[..whole.len() - 1]);
    let exact = match &parsed {
        Ok((eid, steps)) => {
            *eid == 9
                && steps.len() == 2
                && steps[0].position == [1.0, 2.0, 3.0]
                && steps[1].position == [-4.5, 5.25, 6.125]
                && steps[1].weight == 2.0
        }
        Err(_) => false,
    };
    c.record(
        "m3.a_step_is_exactly_54_bytes_of_full_doubles",
        whole.len() == 2 + 2 * 54 && exact && short.is_err(),
        format!(
            "body={} bytes for 2 steps (want 2 header + 2x54), second step decoded \
             exactly={exact}, one byte short rejects={}. The vectors are \
             `Vec3.STREAM_CODEC` — three plain f64s, 24 bytes — and NOT the \
             `LP_STREAM_CODEC` bit-packing `set_entity_motion` uses (M68); the two \
             codecs live on the same `Vec3` class. The SECOND step is what pins it: at \
             any other element width it decodes out of the first one's tail. MUTATION \
             PARTNER: narrow the position to f32 (24 -> 12 bytes) and step[1] reads \
             garbage while step[0] still looks fine",
            whole.len(),
            short.is_err()
        ),
    );

    // ---- m4: a rotation is one SIGNED byte through `Mth.unpackDegrees`. ----
    //
    // **The sign is only observable at the DECODE.** The first version of this
    // witness read the entity's yaw after three ticks and an unsigned-read
    // mutation left it green: `Mth.rotLerp` is
    // `from + a * wrapDegrees(to - from)`, and `wrapDegrees` normalises into
    // (-180, 180], so an off-by-360 in `unpackDegrees` is *erased* by the very
    // next operation. The decoded step is where the mutation bites; the
    // entity's yaw is the second, separately-mutated clause.
    let rot_step = [WireStep {
        position: [0.0; 3],
        movement: [0.0; 3],
        y_rot: -1,
        x_rot: 64,
        weight: 1.0,
    }];
    let decoded = rewo_net::parse_move_minecart(&minecart_body(1, &rot_step))
        .ok()
        .and_then(|(_, s)| s.first().map(|s| (s.y_rot, s.x_rot)));
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    f.send_steps(&mut t, 1, &rot_step);
    for _ in 0..3 {
        t.tick_lerp();
    }
    let (yaw, pitch) = t.get(1).map(|e| (e.yaw, e.pitch)).unwrap_or((999.0, 999.0));
    c.record(
        "m4.a_rotation_is_one_signed_byte_written_onto_the_entity",
        decoded == Some((-1.406_25, 90.0))
            && (yaw + 1.406_25).abs() < 1e-6
            && (pitch - 90.0).abs() < 1e-6,
        format!(
            "decoded={decoded:?} then entity yaw={yaw} pitch={pitch}, from bytes \
             (-1, 64) — want -1.40625 and 90.0 throughout, i.e. `rot * 360 / 256f`. The \
             yaw sample is deliberately NEGATIVE: read unsigned, -1 is 358.59375. \
             MUTATION PARTNERS, both run: (1) read the byte unsigned — this reddens ONLY \
             through the decoded clause, because `Mth.rotLerp`'s `wrapDegrees` erases the \
             360 before it ever reaches the entity, which is why this witness had to be \
             moved to the decode; (2) drop the `e.yaw = sample.y_rot` write in \
             `tick_minecarts` and the entity clause reddens instead"
        ),
    );

    // ---- m5: the router claims its own id and no other. ----
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    let wrong = rewo_net::route_move_minecart_along_track(
        f.ids.cb_play_set_entity_link,
        &minecart_body(1, &l_segment()),
        &f.ids,
        &mut t,
        Some(&f.classes),
    );
    let stored_after_wrong = inbox(&t, 1);
    let right = f.send_steps(&mut t, 1, &l_segment());
    c.record(
        "m5.the_router_claims_only_its_own_id",
        !wrong && stored_after_wrong == 0 && right && inbox(&t, 1) == 2,
        format!(
            "under set_entity_link's id: claimed={wrong} inbox={stored_after_wrong}; \
             under its own: claimed={right} inbox={}. The three M77 ids sit in one \
             `else if` ladder, so a router that claimed a neighbour's body would silently \
             swallow it. MUTATION PARTNER: m6.an_untracked_id_is_inert, where the id \
             matches and the body is well-formed and STILL nothing is stored",
            inbox(&t, 1)
        ),
    );

    // ---- m6: `packet.getEntity(level)` — an untracked id is inert. ----
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    let claimed = f.send_steps(&mut t, 4242, &l_segment());
    c.record(
        "m6.an_untracked_id_is_inert",
        claimed && t.minecart_lerp(4242).is_none() && inbox(&t, 1) == 0,
        format!(
            "claimed={claimed}, schedule for the absent id 4242 = {:?}, and the tracked \
             cart is untouched (inbox {}). `getEntity(id) == null` ends the handler, and \
             a schedule must never be created for an id the table does not hold — it \
             would be ticked against a position that does not exist. MUTATION PARTNER: \
             the net-side lookup is the load-bearing one, because it is also where the \
             type id for the class gate comes from, so the mutation has to push the \
             steps from its `else` arm AND drop `push_minecart_steps`' own \
             `contains_key` — run, and this goes red. Dropping that world-side guard \
             ALONE leaves the gate green: it is a belt, exactly like \
             `position_riders`' per-rider `vehicle_of` re-check above",
            t.minecart_lerp(4242).map(|l| l.inbox_len()),
            inbox(&t, 1)
        ),
    );

    // ---- m7: `instanceof AbstractMinecart`. ----
    let mut t = table_at(
        f,
        &[
            (1, f.t.minecart, [0.0; 3]),
            (2, f.t.pig, [0.0; 3]),
            (3, f.t.oak_boat, [0.0; 3]),
        ],
    );
    f.send_steps(&mut t, 1, &l_segment());
    f.send_steps(&mut t, 2, &l_segment());
    f.send_steps(&mut t, 3, &l_segment());
    c.record(
        "m7.only_a_minecart_takes_a_step_list",
        inbox(&t, 1) == 2 && inbox(&t, 2) == 0 && inbox(&t, 3) == 0,
        format!(
            "minecart inbox={}, pig={}, boat={} (want 2, 0, 0). The handler's first guard \
             is `instanceof AbstractMinecart`; a boat is the interesting negative because \
             it is the OTHER rideable vehicle and shares the leash gate below, so a class \
             table that collapsed 'vehicle' into one set would pass every other witness \
             here and fail this. MUTATION PARTNER: m6, where the class is right and the \
             ENTITY is missing",
            inbox(&t, 1),
            inbox(&t, 2),
            inbox(&t, 3)
        ),
    );
}

fn check_minecart_schedule(f: &Fixture, c: &mut Checker) {
    // ---- m8: one segment, three ticks, converging exactly. ----
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    f.send_steps(&mut t, 1, &[WireStep::at([30.0, 0.0, 0.0], 1.0)]);
    let mut walk = Vec::new();
    for _ in 0..3 {
        t.tick_lerp();
        walk.push(pos(&t, 1, 1.0)[0]);
    }
    let thirds = walk
        .iter()
        .zip([10.0, 20.0, 30.0])
        .all(|(got, want)| (got - want).abs() < EPS);
    c.record(
        "m8.one_segment_is_traversed_over_exactly_three_ticks",
        thirds,
        format!(
            "x per tick = {walk:?} (want 10, 20, 30). `lerpDelay` is set to \
             `POS_ROT_LERP_TICKS` = 3 at ingest and pre-decremented each tick, so \
             `alpha = (3 - lerpDelay + 1) / 3` walks 1/3, 2/3, 1 — the segment is \
             consumed in exactly three ticks and lands ON the last step, not near it. \
             MUTATION PARTNER: m9.the_segment_expires..., which proves a FOURTH tick \
             stops writing rather than overshooting"
        ),
    );

    // ---- m9: the segment expires; the generic position stands. ----
    t.tick_lerp();
    let after = pos(&t, 1, 1.0);
    c.record(
        "m9.the_segment_expires_and_the_generic_position_stands",
        (after[0] - 30.0).abs() < EPS && t.minecart_render(1, 1.0).is_none(),
        format!(
            "after a fourth tick x={} and minecart_render={:?} (want 30 and None). \
             `lerpClientPositionAndRotation` clears `currentLerpSteps` when the countdown \
             expires with an empty inbox, and `newExtractState`'s \
             `if (behavior.cartHasPosRotLerp())` then falls through to the entity's own \
             position. MUTATION PARTNER: m8, where the segment is live and every tick \
             writes",
            after[0],
            t.minecart_render(1, 1.0).map(|s| s.position)
        ),
    );

    // ---- m10: a zero-weight segment snaps rather than traversing. ----
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    f.send_steps(&mut t, 1, &[WireStep::at([7.0, 0.0, 0.0], 0.0)]);
    t.tick_lerp();
    let snapped = pos(&t, 1, 1.0)[0];
    c.record(
        "m10.a_zero_weight_segment_snaps_on_the_first_tick",
        (snapped - 7.0).abs() < EPS,
        format!(
            "x={snapped} after ONE tick (want 7, not 7/3). A weight of 0 is not 'no \
             movement': `adjustToRails(..., instant = true)` emits one, every weighted \
             step is skipped by the index search, and the `!foundIndex` fallback selects \
             the LAST step at `indexedPartialTick = 1.0`. MUTATION PARTNER: m8, whose \
             identical shape at weight 1 lands a third of the way instead"
        ),
    );

    // ---- m11 + m12: THE composition pair. ----
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    f.send_steps(&mut t, 1, &l_segment());
    for _ in 0..3 {
        t.tick_lerp();
    }
    let generic_1 = pos(&t, 1, 1.0);
    let schedule_1 = t.minecart_render(1, 1.0).map(|s| s.position);
    c.record(
        "m11.the_schedule_and_the_generic_lerp_coincide_at_alpha_1",
        schedule_1.is_some_and(|s| dist(s, generic_1) < 1e-12),
        format!(
            "generic render_pos(1.0)={generic_1:?} schedule sample(1.0)={schedule_1:?} — \
             the SAME point, to 1e-12. This is what 'the schedule does not replace the \
             generic lerp' means concretely: the tick writes `sample(1.0)` into the \
             entity through `set_derived_pos`, so `prev -> cur` is the tick-quantised \
             chord between two consecutive schedule samples and its endpoint IS a \
             schedule sample. MUTATION PARTNER: \
             m12.and_diverge_inside_a_tick..., the same pair sampled mid-tick"
        ),
    );

    let generic_half = pos(&t, 1, 0.5);
    let schedule_half = t.minecart_render(1, 0.5).map(|s| s.position);
    // Vanilla's `state.passengerOffset` is exactly this subtraction.
    let offset = schedule_half.map(|s| dist(s, generic_half)).unwrap_or(0.0);
    let want_schedule = [10.0, 0.0, 10.0 / 3.0];
    let want_generic = [(8.888_888_888_888_89 + 10.0) / 2.0, 0.0, 5.0];
    c.record(
        "m12.and_diverge_inside_a_tick_that_crosses_a_step_boundary",
        schedule_half.is_some_and(|s| dist(s, want_schedule) < EPS)
            && dist(generic_half, want_generic) < EPS
            && offset > 1.7,
        format!(
            "at alpha 0.5: schedule={schedule_half:?} (want {want_schedule:?}), generic \
             render_pos={generic_half:?} (want {want_generic:?}), |difference|={offset:.4} \
             blocks. That difference IS vanilla's `state.passengerOffset`, which \
             `EntityRenderer.extractRenderState` computes as \
             `getCartLerpPosition(partialTicks) - lerp(partialTicks, xOld, getX())` — so \
             vanilla itself measures one against the other and both must be live. They \
             separate only where the segment crosses a step boundary mid-tick, which is \
             why the fixture is L-shaped with weights 3 and 1. MUTATION PARTNER: m11, \
             the same pair at alpha 1 where they must agree exactly"
        ),
    );

    // ---- m13: the schedule never arms the generic 3-tick lerp. ----
    let synced = t.get(1).map(|e| [e.x, e.y, e.z]).unwrap_or([9.0; 3]);
    c.record(
        "m13.the_schedule_never_arms_the_generic_3_tick_lerp",
        dist(synced, [0.0; 3]) < 1e-12 && dist(generic_1, [10.0, 0.0, 10.0]) < EPS,
        format!(
            "synced target={synced:?} (want the spawn point, untouched) while the \
             rendered position reached {generic_1:?}. `setPos` writes the position, not a \
             lerp target — which is why a `NewMinecartBehavior` cart needs no \
             `InterpolationHandler` (`getInterpolation()` returns null for it; \
             `OldMinecartBehavior` is the half that owns one) and why the server sends it \
             no `move_entity_pos` / `teleport_entity` at all. MUTATION PARTNER: route the \
             sample through `set_target` instead of `set_derived_pos` — the target moves, \
             and the cart then converges on each sample three ticks late"
        ),
    );

    // ---- m14: a cart is positioned before its passengers. ----
    let mut t = table_at(
        f,
        &[(1, f.t.minecart, [0.0; 3]), (2, f.t.player, [0.0; 3])],
    );
    f.mount(&mut t, 1, &[2]);
    f.send_steps(&mut t, 1, &l_segment());
    for _ in 0..3 {
        t.tick_lerp();
    }
    let cart = pos(&t, 1, 1.0);
    let s = seat(&t, 1, 2, 1.0);
    let seat_len = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
    c.record(
        "m14.a_cart_is_positioned_before_its_passengers",
        dist(cart, [10.0, 0.0, 10.0]) < EPS && seat_len < 1.0,
        format!(
            "cart at {cart:?}, rider seat offset {s:?} (|{seat_len:.4}| — a seat, not a \
             lag). `ClientLevel.tickNonPassenger` runs `entity.tick()` — which is where \
             the schedule writes — and only then loops into `tickPassenger`, so \
             `tick_minecarts` must run BEFORE `position_riders`. MUTATION PARTNER: swap \
             the two calls in `tick_lerp` and the rider is placed off the PREVIOUS tick's \
             cart position, which on this segment is a whole 10 blocks away"
        ),
    );

    // ---- m15: a recycled entity id inherits no schedule. ----
    //
    // **Three clauses, because TWO writers clear the schedule and each needs a
    // sample where the other cannot cover for it.** The first version tested
    // only `remove` followed by `add`, and a mutation dropping either clear
    // left the gate green — the other one still ran. So: `remove` alone,
    // `remove` then `add`, and `add` on top of a LIVE schedule with no
    // `remove` at all (which is the dropped-`remove_entities` case the `add`
    // clear exists for).
    let mut t = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    f.send_steps(&mut t, 1, &l_segment());
    t.tick_lerp();
    let moved = pos(&t, 1, 1.0)[0] > 1.0;
    t.remove(1);
    let cleared_by_remove = t.minecart_lerp(1).is_none();
    t.add(
        1,
        EntityState::new(0, f.t.minecart, 50.0, 0.0, 0.0, 0.0, 0.0),
    );
    t.tick_lerp();
    let after_remove = pos(&t, 1, 1.0);

    let mut t2 = table_at(f, &[(1, f.t.minecart, [0.0; 3])]);
    f.send_steps(&mut t2, 1, &l_segment());
    t2.tick_lerp();
    t2.add(
        1,
        EntityState::new(0, f.t.minecart, 50.0, 0.0, 0.0, 0.0, 0.0),
    );
    let cleared_by_add = t2.minecart_lerp(1).is_none();
    t2.tick_lerp();
    let after_add = pos(&t2, 1, 1.0);
    c.record(
        "m15.a_recycled_entity_id_inherits_no_schedule",
        moved
            && cleared_by_remove
            && dist(after_remove, [50.0, 0.0, 0.0]) < EPS
            && cleared_by_add
            && dist(after_add, [50.0, 0.0, 0.0]) < EPS,
        format!(
            "the first cart moved={moved}; `remove` alone cleared={cleared_by_remove}; \
             after remove + re-add the cart sits at {after_remove:?}; a bare `add` over a \
             live schedule cleared={cleared_by_add} and leaves it at {after_add:?} (want \
             [50,0,0] both). A schedule is a clock, and every clock in this table dies \
             with its entity — otherwise a replacement is dragged toward the previous \
             occupant's rail. MUTATION PARTNERS, both run and each caught by exactly one \
             clause: drop `minecarts.remove` from `EntityTable::remove` (only the \
             `remove`-alone clause reddens — the later `add` would otherwise cover for \
             it), and drop it from `EntityTable::add` (only the bare-`add` clause does)"
        ),
    );
}

fn check_leash(f: &Fixture, c: &mut Checker) {
    // ---- m16: two fixed big-endian i32s, not var-ints. ----
    // 300 is the sample: its big-endian encoding starts `00`, which a var-int
    // reader consumes as the whole id 0 — an untracked entity, hence inert.
    let mut t = table_at(f, &[(300, f.t.pig, [0.0; 3]), (7, f.t.player, [0.0; 3])]);
    let claimed = f.send_link(&mut t, 300, 7);
    c.record(
        "m16.the_link_ids_are_fixed_big_endian_i32s",
        claimed && t.leash_data(300) == Some(7) && t.leash_holder(300) == Some(7),
        format!(
            "claimed={claimed} leash_data(300)={:?} leash_holder(300)={:?} (want Some(7) \
             both). `ClientboundSetEntityLinkPacket`'s reader is `readInt()` twice — the \
             same fixed-width shape `entity_event`'s id has (M17) and the opposite of \
             nearly every other entity-addressed packet. The id 300 is chosen because its \
             BE encoding leads with `00`: a var-int reading decodes entity 0, which is \
             untracked, so the whole packet goes silently inert instead of failing. \
             MUTATION PARTNER: m19.only_a_leashable_entity_takes_a_link, where the ids \
             decode correctly and the CLASS refuses",
            t.leash_data(300),
            t.leash_holder(300)
        ),
    );

    // ---- m17: dest 0 is a leash record holding nothing. ----
    // **The sample must sit where the mutation bites.** `getLeashHolder`'s
    // `delayedLeashHolderId != 0` test is only load-bearing when an entity
    // ACTUALLY EXISTS at id 0 — otherwise the lookup fails anyway and dropping
    // the zero test changes nothing. So the fixture spawns one.
    let mut t = table_at(f, &[(0, f.t.player, [0.0; 3]), (1, f.t.pig, [0.0; 3])]);
    f.send_link(&mut t, 1, 0);
    c.record(
        "m17.dest_zero_is_a_record_with_no_holder",
        t.leash_data(1) == Some(0) && t.leash_holder(1).is_none(),
        format!(
            "leash_data={:?} leash_holder={:?} (want Some(0) and None) — with a real \
             entity present at id 0, so the zero test is the only thing refusing it. \
             `setDelayedLeashHolderId(0)` still installs a `LeashData`: the sending \
             constructor writes `destEntity != null ? getId() : 0`, so 0 is the wire's \
             null and `getLeashHolder`'s `delayedLeashHolderId != 0` is what turns it \
             into no holder. Two distinct states — 'never linked' is `None` from \
             `leash_data`. MUTATION PARTNER: drop the `dest == 0` test and the holder \
             resolves to the entity at id 0; without an entity there the mutation would \
             leave this green",
            t.leash_data(1),
            t.leash_holder(1)
        ),
    );

    // ---- m18: an unresolved holder resolves when it arrives. ----
    let mut t = table_at(f, &[(1, f.t.pig, [0.0; 3])]);
    f.send_link(&mut t, 1, 55);
    let before = (t.leash_data(1), t.leash_holder(1));
    t.add(55, EntityState::new(0, f.t.player, 0.0, 0.0, 0.0, 0.0, 0.0));
    let after = (t.leash_data(1), t.leash_holder(1));
    c.record(
        "m18.an_unresolved_holder_resolves_when_it_arrives",
        before == (Some(55), None) && after == (Some(55), Some(55)),
        format!(
            "before the holder spawns {before:?}, after {after:?}. `getLeashHolder` is a \
             LAZY two-step: the packet stores a delayed id and only a later \
             `level.getEntity(id) != null` promotes it, which is exactly what makes a \
             leash survive its holder arriving in a later chunk batch. MUTATION PARTNER: \
             m17, where the stored id is 0 and no arrival can ever resolve it"
        ),
    );

    // ---- m19: `instanceof Leashable` — an interface, so a UNION. ----
    let mut t = table_at(
        f,
        &[
            (1, f.t.pig, [0.0; 3]),
            (2, f.t.oak_boat, [0.0; 3]),
            (3, f.t.armor_stand, [0.0; 3]),
            (4, f.t.minecart, [0.0; 3]),
            (9, f.t.player, [0.0; 3]),
        ],
    );
    for src in [1, 2, 3, 4] {
        f.send_link(&mut t, src, 9);
    }
    let got = [
        t.leash_data(1).is_some(),
        t.leash_data(2).is_some(),
        t.leash_data(3).is_some(),
        t.leash_data(4).is_some(),
    ];
    c.record(
        "m19.only_a_leashable_entity_takes_a_link",
        got == [true, true, false, false],
        format!(
            "pig={} boat={} armour stand={} minecart={} (want true, true, false, false). \
             `Leashable` is an INTERFACE, declared by exactly `Mob` and `AbstractBoat`, \
             so the gate is the union of two subtrees rather than one ancestry walk. \
             Both negatives sit on a real boundary: an armour stand is a `LivingEntity` \
             that is not a `Mob`, and a minecart is the other rideable vehicle. MUTATION \
             PARTNERS, both run: swap the gate for `is_living` and the armour stand takes \
             a leash; swap it for `is_mob` and the boat stops taking one",
            got[0], got[1], got[2], got[3]
        ),
    );
}

fn check_projectile_power(f: &Fixture, c: &mut Checker) {
    // ---- m20: the power is an f64, bit-exact. ----
    let mut t = table_at(f, &[(1, f.t.fireball, [0.0; 3])]);
    let power = 1.0f64 / 3.0;
    let claimed = f.send_power(&mut t, 1, power);
    let got = t.projectile_power(1);
    c.record(
        "m20.the_power_is_an_f64_not_a_narrowed_f32",
        claimed && got.is_some_and(|g| g.to_bits() == power.to_bits()),
        format!(
            "claimed={claimed} stored={got:?} want {power} — compared on the BITS, not \
             within a tolerance. `readDouble()` is the whole tail of the packet, and 1/3 \
             is chosen because it is the cheapest value no f32 can hold: a narrowed read \
             would land within any sane epsilon and fail here. MUTATION PARTNER: \
             m21.an_arrow_is_not_a_hurting_projectile, where the value is fine and the \
             CLASS refuses"
        ),
    );

    // ---- m21: an arrow is not an `AbstractHurtingProjectile`. ----
    let mut t = table_at(
        f,
        &[
            (1, f.t.fireball, [0.0; 3]),
            (2, f.t.arrow, [0.0; 3]),
            (3, f.t.pig, [0.0; 3]),
        ],
    );
    for id in [1, 2, 3] {
        f.send_power(&mut t, id, 5.0);
    }
    let got = [
        t.projectile_power(1),
        t.projectile_power(2),
        t.projectile_power(3),
    ];
    c.record(
        "m21.an_arrow_is_not_a_hurting_projectile",
        got == [Some(5.0), None, None],
        format!(
            "fireball={:?} arrow={:?} pig={:?} (want Some(5), None, None). \
             `handleProjectilePowerPacket` casts to `AbstractHurtingProjectile`, and an \
             arrow is an `AbstractArrow` — a SIBLING branch that is a projectile in every \
             English sense and fails the cast. Only six types pass: the fireball family, \
             the wither skull and the two wind charges. MUTATION PARTNER: m20, where the \
             class is right and the assertion is about the value's width",
            got[0], got[1], got[2]
        ),
    );
}
