//! `rewo attributeshot --check` — the M55 entity-attribute oracle.
//!
//! Serverless, CPU-only, fail-closed. Every witness drives a **raw packet
//! body** through the production path — `route_update_attributes` →
//! `apply_update_attributes` → `crate::attributes::parse` →
//! `EntityTable::set_attribute` → `rewo_world::attributes::resolve` — so packet
//! id selection, the holder encoding, the supplier filter and the value
//! arithmetic are all exercised by the code the client actually runs. Nothing
//! here reimplements the decoder and then grades the reimplementation.
//!
//! The expected numbers are independent: they are hand-derived from
//! `AttributeInstance.calculateValue` and `RangedAttribute.sanitizeValue` in the
//! decompiled 26.2 source and written as literals, not computed by calling the
//! production resolver a second time.
//!
//! **Where the mutation partners live.** Several properties here are invisible
//! to the obvious test, so each is paired with a case that must come out
//! *differently*:
//!
//! * `ADD_MULTIPLIED_BASE` and `ADD_MULTIPLIED_TOTAL` are indistinguishable
//!   with one modifier — `b1`/`b2` show they agree, and `b3`/`b4` separate them
//!   with two.
//! * Grouping by operation vs. applying in packet order is invisible unless the
//!   packet lists them out of order — `b6` sends them reversed.
//! * A raw `holderRegistry` id and an `id + 1` `holder` id both "work" if you
//!   only ever test one attribute — `a4`/`a5` feed 23 and 24 and require
//!   opposite outcomes.
//! * A resolved default and a fail-closed absence are both "no packet
//!   arrived" — `d1`/`d3` require `Some(20.0, Default)` and `None`.

use clap::Args as ClapArgs;

use rewo_data::attributes::AttributeRegistry;
use rewo_data::entity_types::{EntityClasses, EntityTypes};
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_net::ids::Ids;
use rewo_world::attributes::{resolve, Source};
use rewo_world::entities::{EntityState, EntityTable};

/// Total named properties this gate asserts. Locked so a skipped property fails
/// the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 43;

/// Tolerance for the resolved doubles. The arithmetic is exact for these
/// values, so this only absorbs the last-bit noise a different multiplication
/// order could introduce.
const EPS: f64 = 1e-12;

#[derive(ClapArgs)]
pub struct AttributeshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `eventshot`/`danceshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Version whose `packets.json` / `registries.json` resolve the real
    /// `update_attributes` packet id and the `minecraft:attribute` registry.
    #[arg(long, default_value = "26.2")]
    version: String,
}

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
        println!("[attributeshot] {status}  {name}: {detail}");
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }

    /// Record a `f64` property against an independently derived literal.
    fn near(&mut self, name: &str, got: Option<f64>, want: f64, why: &str) {
        let pass = got.is_some_and(|g| (g - want).abs() <= EPS);
        self.record(
            name,
            pass,
            format!("{got:?} (want {want} — {why})"),
        );
    }
}

// ---------------------------------------------------------------------------
// Body builder — independent of `rewo_net`'s decoder.
// ---------------------------------------------------------------------------

fn varint(v: i32, out: &mut Vec<u8>) {
    let mut n = v as u32;
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

/// One modifier as it appears on the wire.
struct Mod {
    id: &'static str,
    amount: f64,
    /// The raw operation id, so a witness can send one out of range.
    op: i32,
}

fn m(id: &'static str, amount: f64, op: i32) -> Mod {
    Mod { id, amount, op }
}

/// `ClientboundUpdateAttributesPacket` wire form, from the decompiled
/// `STREAM_CODEC`: VarInt entity id, a VarInt-counted snapshot list, and per
/// snapshot a VarInt attribute holder (raw registry id), a big-endian f64 base
/// and a VarInt-counted modifier list of (UTF-8 string id, f64 amount, VarInt
/// operation). Built here rather than by any writer under test.
fn body(entity: i32, snaps: &[(i32, f64, Vec<Mod>)]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(entity, &mut out);
    varint(snaps.len() as i32, &mut out);
    for (attr, base, mods) in snaps {
        varint(*attr, &mut out);
        out.extend_from_slice(&base.to_be_bytes());
        varint(mods.len() as i32, &mut out);
        for md in mods {
            varint(md.id.len() as i32, &mut out);
            out.extend_from_slice(md.id.as_bytes());
            out.extend_from_slice(&md.amount.to_be_bytes());
            varint(md.op, &mut out);
        }
    }
    out
}

/// A body whose operation field is written as a **multi-byte** VarInt with a
/// redundant continuation byte. Legal VarInt, same value; a reader that took a
/// single byte would see `0x82` and then desynchronise on the rest.
fn body_wide_operation(entity: i32, attr: i32, base: f64, id: &str, amount: f64, op: i32) -> Vec<u8> {
    let mut out = Vec::new();
    varint(entity, &mut out);
    varint(1, &mut out);
    varint(attr, &mut out);
    out.extend_from_slice(&base.to_be_bytes());
    varint(1, &mut out);
    varint(id.len() as i32, &mut out);
    out.extend_from_slice(id.as_bytes());
    out.extend_from_slice(&amount.to_be_bytes());
    out.push((op as u8) | 0x80); // continuation set…
    out.push(0x00); // …with a zero high group: still `op`.
    out
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A table holding one entity of `type_id` at id 1.
fn table_with(type_id: i32) -> EntityTable {
    let mut t = EntityTable::default();
    t.add(1, EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0));
    t
}

struct Env {
    ids: Ids,
    classes: EntityClasses,
    types: EntityTypes,
    reg: AttributeRegistry,
}

impl Env {
    /// Drive a body through the production router and return whether the id
    /// matched. Always uses the real `update_attributes` id unless told not to.
    fn route(&self, t: &mut EntityTable, id: i32, body: &[u8]) -> bool {
        rewo_net::route_update_attributes(
            id,
            body,
            &self.ids,
            t,
            Some(&self.classes),
            Some(&self.types),
            Some(&self.reg),
        )
    }

    fn send(&self, t: &mut EntityTable, body: &[u8]) {
        self.route(t, self.ids.cb_play_update_attributes, body);
    }

    /// The production resolution for entity 1.
    fn value(&self, t: &EntityTable, type_name: &str, attr: &str) -> Option<f64> {
        resolve(t.attributes(1), Some(type_name), attr, &self.reg).map(|(v, _)| v)
    }

    fn source(&self, t: &EntityTable, type_name: &str, attr: &str) -> Option<Source> {
        resolve(t.attributes(1), Some(type_name), attr, &self.reg).map(|(_, s)| s)
    }
}

pub fn run(args: AttributeshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[attributeshot] mode: {mode} (serverless, CPU-only; the oracle asserts \
         unconditionally — a failure exits nonzero with or without --check)"
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let types = EntityTypes::load(&paths.registries_json())?;
    let classes = EntityClasses::resolve(&types)?;
    let reg = AttributeRegistry::load(&paths.registries_json())?;

    let ua = ids.cb_play_update_attributes;
    let control = ids.cb_play_damage_event;
    if ua == control {
        return Err("update_attributes and damage_event resolved to the same id".into());
    }
    let zombie = types
        .id_of("minecraft:zombie")
        .ok_or("registries.json: no minecraft:zombie")?;
    let pig = types
        .id_of("minecraft:pig")
        .ok_or("registries.json: no minecraft:pig")?;
    let boat = types
        .id_of("minecraft:oak_boat")
        .ok_or("registries.json: no minecraft:oak_boat")?;
    let golem = types
        .id_of("minecraft:iron_golem")
        .ok_or("registries.json: no minecraft:iron_golem")?;

    println!(
        "[attributeshot] ids: update_attributes={ua} (control damage_event={control}); \
         types: zombie={zombie} pig={pig} boat={boat} iron_golem={golem}; \
         {} attributes registered",
        reg.len()
    );

    let env = Env {
        ids,
        classes,
        types,
        reg,
    };

    let mut c = Checker::new();
    check_wire(&mut c, &env, control, zombie);
    check_value(&mut c, &env, zombie);
    check_receipt(&mut c, &env, zombie, pig, boat);
    check_defaults(&mut c, &env, zombie, pig, golem);

    println!(
        "[attributeshot] witnesses observed: {} / {}",
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
            "witness count {} != expected {EXPECTED_WITNESSES} — a named property \
             was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[attributeshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// a — the wire: id resolution, dispatch, holder encoding, field widths.
// ---------------------------------------------------------------------------

fn check_wire(c: &mut Checker, env: &Env, control: i32, zombie: i32) {
    let mh = env.reg.id_of("max_health");
    c.record(
        "a0.max_health_id_resolves_from_the_registry",
        mh == Some(23),
        format!("{mh:?} (want Some(23) — registries.json minecraft:max_health)"),
    );

    // a1: the real id routes and applies.
    let mut t = table_with(zombie);
    let matched = env.route(
        &mut t,
        env.ids.cb_play_update_attributes,
        &body(1, &[(23, 40.0, vec![])]),
    );
    let got = env.value(&t, "minecraft:zombie", "max_health");
    c.record(
        "a1.the_real_id_routes_and_applies",
        matched && got == Some(40.0),
        format!("matched={matched} value={got:?} (want true / Some(40.0))"),
    );

    // a2: mutation partner — another real packet id must not match, and must
    // leave the attribute at its default.
    let mut t = table_with(zombie);
    let matched = env.route(&mut t, control, &body(1, &[(23, 40.0, vec![])]));
    let got = env.value(&t, "minecraft:zombie", "max_health");
    c.record(
        "a2.a_different_packet_id_does_not_route",
        !matched && got == Some(20.0),
        format!("matched={matched} value={got:?} (want false / Some(20.0) default)"),
    );

    // a3: the base is a big-endian f64. A little-endian reader would decode
    // 40.0's bytes as ~3.03e-319 and the clamp would floor it at 1.0.
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &[(23, 40.0, vec![])]));
    c.near(
        "a3.the_base_is_a_big_endian_f64",
        env.value(&t, "minecraft:zombie", "max_health"),
        40.0,
        "byte-swapped it would clamp to the 1.0 minimum",
    );

    // a4 / a5: the attribute holder is `holderRegistry` — a RAW 0-based id.
    // max_health is 23; max_absorption is 22 and mining_efficiency is 24. A
    // reader applying `holder`'s `id + 1` convention would read 23 as 22, and a
    // writer using it would have sent 24. Both must miss.
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &[(23, 40.0, vec![])]));
    c.near(
        "a4.a_raw_23_is_max_health",
        env.value(&t, "minecraft:zombie", "max_health"),
        40.0,
        "holderRegistry is a raw 0-based VarInt id",
    );
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &[(24, 40.0, vec![])]));
    let got = env.value(&t, "minecraft:zombie", "max_health");
    let src = env.source(&t, "minecraft:zombie", "max_health");
    c.record(
        "a5.a_raw_24_is_not_max_health",
        got == Some(20.0) && src == Some(Source::Default),
        format!(
            "value={got:?} source={src:?} (want Some(20.0)/Default — an id+1 \
             reader would have made this max_health=40)"
        ),
    );

    // a6: the operation is a VarInt (`ByteBufCodecs.idMapper` → `VarInt.read`),
    // not the single byte a three-valued enum invites.
    let mut t = table_with(zombie);
    env.send(
        &mut t,
        &body_wide_operation(1, 23, 20.0, "rewo:test", 0.5, 2),
    );
    c.near(
        "a6.the_operation_is_a_varint_not_a_byte",
        env.value(&t, "minecraft:zombie", "max_health"),
        30.0,
        "a 2-byte VarInt encoding of operation 2 (ADD_MULTIPLIED_TOTAL)",
    );

    // a7: an out-of-range operation id is ADD_VALUE, not a rejected packet —
    // `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`.
    let mut t = table_with(zombie);
    env.send(
        &mut t,
        &body(1, &[(23, 20.0, vec![m("rewo:oob", 5.0, 9)])]),
    );
    c.near(
        "a7.an_out_of_range_operation_is_add_value",
        env.value(&t, "minecraft:zombie", "max_health"),
        25.0,
        "operation 9 → ADD_VALUE (+5), not a dropped packet",
    );

    // a8: truncation at every length is rejected, and nothing is stored.
    let full = body(1, &[(23, 40.0, vec![m("rewo:t", 1.0, 0)])]);
    let mut bad = 0usize;
    for cut in 0..full.len() {
        let mut t = table_with(zombie);
        env.send(&mut t, &full[..cut]);
        if env.value(&t, "minecraft:zombie", "max_health") != Some(20.0) {
            bad += 1;
        }
    }
    c.record(
        "a8.every_truncation_stores_nothing",
        bad == 0,
        format!("{bad} of {} prefixes stored something (want 0)", full.len()),
    );

    // a9: the snapshot list's declared maximum is `ByteBufCodecs.list(128)`.
    let over: Vec<(i32, f64, Vec<Mod>)> = (0..129).map(|_| (23, 40.0, Vec::new())).collect();
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &over));
    let over_got = env.value(&t, "minecraft:zombie", "max_health");
    let at: Vec<(i32, f64, Vec<Mod>)> = (0..128).map(|_| (23, 40.0, Vec::new())).collect();
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &at));
    let at_got = env.value(&t, "minecraft:zombie", "max_health");
    c.record(
        "a9.the_snapshot_list_cap_is_128",
        over_got == Some(20.0) && at_got == Some(40.0),
        format!("129→{over_got:?} 128→{at_got:?} (want Some(20.0) / Some(40.0))"),
    );
}

// ---------------------------------------------------------------------------
// b — calculateValue: the operation order, and sanitizeValue.
// ---------------------------------------------------------------------------

fn check_value(c: &mut Checker, env: &Env, zombie: i32) {
    let mut send = |mods: Vec<Mod>, base: f64| -> Option<f64> {
        let mut t = table_with(zombie);
        env.send(&mut t, &body(1, &[(23, base, mods)]));
        env.value(&t, "minecraft:zombie", "max_health")
    };

    // b1 / b2: with ONE modifier the two multiplying operations agree — this is
    // exactly why a single-modifier witness cannot tell them apart, and why the
    // pair below exists.
    c.near(
        "b1.one_add_multiplied_total_of_half",
        send(vec![m("rewo:a", 0.5, 2)], 20.0),
        30.0,
        "20 * 1.5",
    );
    c.near(
        "b2.one_add_multiplied_base_of_half_agrees",
        send(vec![m("rewo:a", 0.5, 1)], 20.0),
        30.0,
        "20 + 20*0.5 — identical to b1, so neither witness pins the operation",
    );

    // b3 / b4: two of the same kind separate them. ADD_MULTIPLIED_BASE does not
    // compound (both read the same base); ADD_MULTIPLIED_TOTAL does.
    c.near(
        "b3.two_add_multiplied_base_do_not_compound",
        send(vec![m("rewo:a", 0.5, 1), m("rewo:b", 0.5, 1)], 20.0),
        40.0,
        "20 + 20*0.5 + 20*0.5 — NOT 20*1.5*1.5",
    );
    c.near(
        "b4.two_add_multiplied_total_do_compound",
        send(vec![m("rewo:a", 0.5, 2), m("rewo:b", 0.5, 2)], 20.0),
        45.0,
        "20 * 1.5 * 1.5 — the mutation partner of b3",
    );

    // b5: ADD_MULTIPLIED_BASE reads the POST-ADD_VALUE base.
    c.near(
        "b5.add_multiplied_base_reads_the_post_add_value_base",
        send(vec![m("rewo:a", 10.0, 0), m("rewo:b", 0.5, 1)], 20.0),
        45.0,
        "(20+10) + 30*0.5 — reading the pre-ADD_VALUE base would give 40",
    );

    // b6: the three groups run in a fixed order regardless of packet order.
    // Sent REVERSED (total, base, add_value); grouping must still give 90.
    // Applying in packet order would give ((20*1.5)+20*0.5)+10 = 50.
    c.near(
        "b6.operations_group_by_kind_not_packet_order",
        send(
            vec![m("rewo:t", 1.0, 2), m("rewo:b", 0.5, 1), m("rewo:v", 10.0, 0)],
            20.0,
        ),
        90.0,
        "base 30, +30*0.5 = 45, *2 = 90 — packet order would give 50",
    );

    // b7 / b8: RangedAttribute.sanitizeValue clamps to max_health's [1, 1024].
    c.near(
        "b7.the_clamp_bites_at_the_minimum",
        send(vec![], 0.0),
        1.0,
        "max_health's minimum is 1.0, not 0.0",
    );
    c.near(
        "b8.the_clamp_bites_at_the_maximum",
        send(vec![], 5000.0),
        1024.0,
        "max_health's maximum is 1024.0",
    );
    c.near(
        "b9.a_value_inside_the_range_is_untouched",
        send(vec![], 512.0),
        512.0,
        "the mutation partner of b7/b8 — the clamp must not always fire",
    );

    // b10: NaN → the MINIMUM, not the maximum and not NaN.
    c.near(
        "b10.nan_sanitizes_to_the_minimum",
        send(
            vec![m("rewo:inf", f64::INFINITY, 0), m("rewo:z", -1.0, 2)],
            20.0,
        ),
        1.0,
        "inf * 0 = NaN, and sanitizeValue sends NaN to min",
    );

    // b11: the clamp is per-attribute. movement_speed's maximum is 1024 too,
    // but its MINIMUM is 0.0 where max_health's is 1.0 — a shared clamp would
    // floor this at 1.0.
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &[(26, 0.0, vec![])]));
    c.near(
        "b11.the_clamp_is_per_attribute",
        env.value(&t, "minecraft:zombie", "movement_speed"),
        0.0,
        "movement_speed's minimum is 0.0, unlike max_health's 1.0",
    );
}

// ---------------------------------------------------------------------------
// c — handleUpdateAttributes' receipt gates.
// ---------------------------------------------------------------------------

fn check_receipt(c: &mut Checker, env: &Env, zombie: i32, pig: i32, boat: i32) {
    // c1: an untracked entity is inert — `getEntity(id) == null`.
    let mut t = EntityTable::default();
    env.send(&mut t, &body(1, &[(23, 40.0, vec![])]));
    c.record(
        "c1.an_untracked_entity_stores_nothing",
        t.attributes(1).is_none(),
        format!("attributes={:?} (want None)", t.attributes(1).map(|a| a.len())),
    );

    // c2: a non-living entity is inert. Vanilla throws IllegalStateException;
    // dropping is the safe equivalent, and either way nothing is stored.
    let mut t = table_with(boat);
    env.send(&mut t, &body(1, &[(23, 40.0, vec![])]));
    c.record(
        "c2.a_non_living_entity_stores_nothing",
        t.attributes(1).is_none(),
        format!(
            "attributes={:?} (want None — a boat is not a LivingEntity)",
            t.attributes(1).map(|a| a.len())
        ),
    );

    // c3: the supplier filters on receipt. A pig's AttributeSupplier does not
    // declare spawn_reinforcements (id 33), which a zombie's does.
    let sr = env.reg.id_of("spawn_reinforcements").unwrap_or(-1);
    let mut t = table_with(pig);
    env.send(&mut t, &body(1, &[(sr, 1.0, vec![])]));
    let pig_stored = t.attributes(1).map(|a| a.len()).unwrap_or(0);
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &[(sr, 1.0, vec![])]));
    let zombie_stored = t.attributes(1).map(|a| a.len()).unwrap_or(0);
    c.record(
        "c3.the_supplier_filters_on_receipt",
        pig_stored == 0 && zombie_stored == 1,
        format!(
            "pig stored {pig_stored}, zombie stored {zombie_stored} \
             (want 0 / 1 — spawn_reinforcements id {sr})"
        ),
    );

    // c4: a snapshot REPLACES that attribute's modifiers rather than merging
    // with the previous packet's.
    let mut t = table_with(zombie);
    env.send(
        &mut t,
        &body(1, &[(23, 20.0, vec![m("rewo:a", 10.0, 0)])]),
    );
    let first = env.value(&t, "minecraft:zombie", "max_health");
    env.send(&mut t, &body(1, &[(23, 20.0, vec![])]));
    let second = env.value(&t, "minecraft:zombie", "max_health");
    c.record(
        "c4.a_snapshot_replaces_rather_than_merging",
        first == Some(30.0) && second == Some(20.0),
        format!("{first:?} then {second:?} (want Some(30.0) then Some(20.0))"),
    );

    // c5: a snapshot leaves attributes it does not name alone.
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &[(23, 40.0, vec![]), (26, 0.5, vec![])]));
    env.send(&mut t, &body(1, &[(23, 50.0, vec![])]));
    let mh = env.value(&t, "minecraft:zombie", "max_health");
    let ms = env.value(&t, "minecraft:zombie", "movement_speed");
    c.record(
        "c5.an_unnamed_attribute_is_left_alone",
        mh == Some(50.0) && ms == Some(0.5),
        format!("max_health={mh:?} movement_speed={ms:?} (want Some(50.0) / Some(0.5))"),
    );

    // c6: removal clears, so a reused id cannot inherit the previous
    // occupant's max health.
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &[(23, 40.0, vec![])]));
    t.remove(1);
    t.add(1, EntityState::new(0, zombie, 0.0, 0.0, 0.0, 0.0, 0.0));
    let got = env.value(&t, "minecraft:zombie", "max_health");
    let src = env.source(&t, "minecraft:zombie", "max_health");
    c.record(
        "c6.removal_clears_the_attributes",
        got == Some(20.0) && src == Some(Source::Default),
        format!("value={got:?} source={src:?} (want Some(20.0)/Default, not 40.0)"),
    );

    // c7: an attribute id outside the registry is dropped, and the rest of the
    // packet still applies — the snapshot is fully parsed either way.
    let bogus = env.reg.len() as i32 + 500;
    let mut t = table_with(zombie);
    env.send(
        &mut t,
        &body(1, &[(bogus, 7.0, vec![]), (23, 40.0, vec![])]),
    );
    let got = env.value(&t, "minecraft:zombie", "max_health");
    let n = t.attributes(1).map(|a| a.len()).unwrap_or(0);
    c.record(
        "c7.an_unknown_attribute_id_drops_only_its_own_snapshot",
        got == Some(40.0) && n == 1,
        format!("max_health={got:?} stored={n} (want Some(40.0) / 1)"),
    );
}

// ---------------------------------------------------------------------------
// d — DefaultAttributes: the supplier as a value source, and fail-closed.
// ---------------------------------------------------------------------------

fn check_defaults(c: &mut Checker, env: &Env, zombie: i32, pig: i32, golem: i32) {
    let empty = EntityTable::default();

    // d1: nothing synced → the type's supplier base, tagged Default.
    let got = resolve(None, Some("minecraft:zombie"), "max_health", &env.reg);
    c.record(
        "d1.an_unsynced_attribute_resolves_from_the_supplier",
        got == Some((20.0, Source::Default)),
        format!("{got:?} (want Some((20.0, Default)))"),
    );

    // d2: a synced value is tagged Synced — the caller can tell them apart.
    let mut t = table_with(zombie);
    env.send(&mut t, &body(1, &[(23, 40.0, vec![])]));
    let got = resolve(
        t.attributes(1),
        Some("minecraft:zombie"),
        "max_health",
        &env.reg,
    );
    c.record(
        "d2.a_synced_attribute_is_tagged_synced",
        got == Some((40.0, Source::Synced)),
        format!("{got:?} (want Some((40.0, Synced)))"),
    );

    // d3: THE fail-closed witness. An entity type with no AttributeSupplier
    // resolves to None, not to the registry's 20.0 default. A health bar that
    // got 20.0 here would draw a full bar over a boat.
    let got = resolve(None, Some("minecraft:oak_boat"), "max_health", &env.reg);
    c.record(
        "d3.a_type_without_a_supplier_resolves_to_none",
        got.is_none(),
        format!("{got:?} (want None — never Some(20.0))"),
    );

    // d4: an unknown entity type is likewise None, not a default.
    let got = resolve(None, None, "max_health", &env.reg);
    let got2 = resolve(None, Some("rewo:not_an_entity"), "max_health", &env.reg);
    c.record(
        "d4.an_unknown_entity_type_resolves_to_none",
        got.is_none() && got2.is_none(),
        format!("{got:?} / {got2:?} (want None / None)"),
    );

    // d5: an attribute the supplier does not declare is None even though the
    // attribute itself is perfectly real and has a registry default of 0.0.
    let got = resolve(None, Some("minecraft:pig"), "spawn_reinforcements", &env.reg);
    let zom = resolve(
        None,
        Some("minecraft:zombie"),
        "spawn_reinforcements",
        &env.reg,
    );
    c.record(
        "d5.an_undeclared_attribute_resolves_to_none",
        got.is_none() && zom == Some((0.0, Source::Default)),
        format!("pig={got:?} zombie={zom:?} (want None / Some((0.0, Default)))"),
    );

    // d6: an unknown attribute name is None rather than any number.
    let got = resolve(None, Some("minecraft:zombie"), "rewo:nope", &env.reg);
    c.record(
        "d6.an_unknown_attribute_name_resolves_to_none",
        got.is_none(),
        format!("{got:?} (want None)"),
    );

    // d7: the supplier's base overrides the attribute's own default. An iron
    // golem is 100, not the registry's 20.
    c.near(
        "d7.the_supplier_base_overrides_the_registry_default",
        resolve(None, Some("minecraft:iron_golem"), "max_health", &env.reg).map(|(v, _)| v),
        100.0,
        "IronGolem.createAttributes adds MAX_HEALTH 100.0",
    );
    c.near(
        "d8.a_second_type_confirms_the_table_is_per_type",
        resolve(None, Some("minecraft:pig"), "max_health", &env.reg).map(|(v, _)| v),
        10.0,
        "Pig.createAttributes adds MAX_HEALTH 10.0 — the mutation partner of d7",
    );

    // d9: `add(attr, 0.23F)` is a FLOAT widened to double, not the decimal
    // 0.23. Getting this wrong is invisible in a render and wrong on the wire
    // model — the same class of error as M37's `+ 0.1F`.
    let want = 0.23f32 as f64;
    let got = resolve(None, Some("minecraft:zombie"), "movement_speed", &env.reg).map(|(v, _)| v);
    let exact = got == Some(want);
    let naive = got == Some(0.23f64);
    c.record(
        "d9.a_float_literal_base_is_widened_not_rounded",
        exact && !naive,
        format!(
            "{got:?} (want {want:?} = 0.23f32 as f64; the decimal 0.23 would be \
             {:?})",
            0.23f64
        ),
    );

    // d10: the supplier chain is inherited. `Zombie.createAttributes` never
    // mentions max_health — it comes from LivingEntity, four classes up.
    c.near(
        "d10.the_supplier_chain_is_inherited",
        resolve(None, Some("minecraft:zombie"), "max_health", &env.reg).map(|(v, _)| v),
        20.0,
        "LivingEntity.createLivingAttributes → Mob → Monster → Zombie",
    );
    // ...and the chain's own additions survive: FOLLOW_RANGE is 16 at Mob and
    // 35 at Zombie, so `buildKeepingLast` must keep the later one.
    c.near(
        "d11.a_repeated_add_keeps_the_last",
        resolve(None, Some("minecraft:zombie"), "follow_range", &env.reg).map(|(v, _)| v),
        35.0,
        "Mob adds FOLLOW_RANGE 16.0, Zombie re-adds 35.0 — buildKeepingLast",
    );

    // d12: the default path is sanitized too. Nothing in vanilla's table
    // violates its own clamp, so this asserts the invariant across all 93
    // suppliers rather than one value.
    let mut bad = Vec::new();
    for (name, pairs) in rewo_data::entity_attributes::ENTITY_DEFAULTS {
        for (attr, base) in *pairs {
            let Some(def) = env.reg.id_of(attr).and_then(|i| env.reg.def(i)) else {
                bad.push(format!("{name}/{attr}: not in the registry"));
                continue;
            };
            if *base < def.min || *base > def.max {
                bad.push(format!("{name}/{attr}: {base} outside [{}, {}]", def.min, def.max));
            }
        }
    }
    c.record(
        "d12.every_supplier_base_is_inside_its_own_clamp",
        bad.is_empty(),
        format!(
            "{} violation(s) across {} suppliers{}",
            bad.len(),
            rewo_data::entity_attributes::ENTITY_DEFAULTS.len(),
            if bad.is_empty() {
                String::new()
            } else {
                format!(": {}", bad.join("; "))
            }
        ),
    );

    // d13: the two tables agree on size — the registry join is the version
    // guard, and this states what it guarded.
    c.record(
        "d13.the_registry_and_the_extracted_table_agree",
        env.reg.len() == rewo_data::entity_attributes::ATTRIBUTES.len() && env.reg.len() == 40,
        format!(
            "registry={} extracted={} (want both 40)",
            env.reg.len(),
            rewo_data::entity_attributes::ATTRIBUTES.len()
        ),
    );

    // d14: `is_living` and "has an AttributeSupplier" are the SAME set over
    // every registered type.
    //
    // This is why `apply_update_attributes`' living gate cannot be isolated by
    // a behavioural witness: removing it changes no outcome, because every type
    // it would reject is also rejected by the supplier lookup two lines later.
    // The gate is kept because it is what `handleUpdateAttributes` does and it
    // is the *documented* reason a boat is inert — and this witness is what
    // makes the redundancy a checked fact rather than an assumption. If a
    // future version ever ships a non-living type with a supplier (or a living
    // one without), this fails and the gate stops being redundant.
    let mut disagree = Vec::new();
    for id in env.types.ids() {
        let living = env.classes.is_living(id);
        let has_supplier = env
            .types
            .name(id)
            .and_then(|n| env.reg.defaults_for(n))
            .is_some();
        if living != has_supplier {
            disagree.push(format!(
                "{}: living={living} supplier={has_supplier}",
                env.types.name(id).unwrap_or("?")
            ));
        }
    }
    let n_living = env.types.ids().filter(|&i| env.classes.is_living(i)).count();
    c.record(
        "d14.is_living_and_has_a_supplier_are_the_same_set",
        disagree.is_empty() && n_living == rewo_data::entity_attributes::ENTITY_DEFAULTS.len(),
        format!(
            "{} disagreement(s) over {} types; {n_living} living vs {} suppliers{}",
            disagree.len(),
            env.types.len(),
            rewo_data::entity_attributes::ENTITY_DEFAULTS.len(),
            if disagree.is_empty() {
                String::new()
            } else {
                format!(": {}", disagree.join("; "))
            }
        ),
    );

    // d15: the three types this gate leans on are all living with a supplier,
    // so the c-group's receipt witnesses are testing what they claim to.
    let ok = [zombie, pig, golem].iter().all(|&id| {
        env.classes.is_living(id)
            && env
                .types
                .name(id)
                .and_then(|n| env.reg.defaults_for(n))
                .is_some()
    });
    c.record(
        "d15.the_fixture_types_are_living_with_suppliers",
        ok && empty.attributes(1).is_none(),
        format!("zombie/pig/iron_golem all living with a supplier: {ok}"),
    );
}
