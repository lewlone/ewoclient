//! `rewo particleshot --check` — the particle oracle (M35).
//!
//! Serverless, CPU-only, fail-closed.
//!
//! # Why this gate looks different from the others
//!
//! REWO_PLAN §16 declined to propose particles because "every existing gate is
//! geometry-based; it would need a new verification approach before it could be
//! shipped honestly." Geometry gates work by rendering a known shape and
//! grading pixels. Particles resist that: they are spawned in bulk by a random
//! process and then integrated over time, so there is no single frame whose
//! contents are a stated fact.
//!
//! The way through is that **vanilla's particle system is not really
//! stochastic** — it is a deterministic function of a seed. `Particle.tick()`
//! contains no randomness at all; every draw happens in a constructor, from a
//! `LegacyRandomSource` that is `java.util.Random`'s LCG and ports to Rust
//! exactly. Fix the seed and the entire subsystem becomes an assertable
//! sequence of numbers, which is the same kind of claim every other gate here
//! makes.
//!
//! So this gate grades three layers:
//!
//! 1. **The wire** — raw `level_particles` and `level_event` bodies, built
//!    against ids resolved by name from the report, driven through the
//!    production decoders. Including the `count == 0` inversion, which is the
//!    single most misreadable thing in the packet.
//! 2. **The fan-out** — `handleParticleEvent`'s spawn loop and
//!    `addDestroyBlockEffect`'s shard grid, against independently-computed
//!    expectations.
//! 3. **The simulation** — seeded trajectories against vectors emitted by a
//!    Java harness whose class bodies are copied verbatim from the decompile
//!    (`rewo-world/src/particles_oracle_26_2.txt`). Those vectors are the only
//!    thing here that can catch a *misreading* of vanilla, as opposed to a
//!    mistranslation of it, and they already caught one: `+ 0.1` where vanilla
//!    writes `+ 0.1F`.
//!
//! Rendering is deliberately **not** graded here. A particle quad is a
//! camera-facing billboard whose geometry the existing passes already cover;
//! what was novel and unverified about particles was the simulation, and that
//! is what this gate pins. Stated so the gap is a decision rather than an
//! oversight.

use clap::Args as ClapArgs;
use rewo_data::packets::{Dir, Packets, State};
use rewo_data::{particle_types::ParticleTypes, DataPaths};
use rewo_world::particles::{
    LegacyRandom, Particle, ParticleEvent, ParticleKind, ParticleSystem, FULL_CUBE,
    LEVEL_EVENT_DESTROY_BLOCK,
};

const EXPECTED_WITNESSES: usize = 34;

#[derive(ClapArgs, Debug)]
pub struct ParticleshotArgs {
    #[arg(long, default_value_t = false)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[particleshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

/// Empty world — nothing collides. The trajectory oracle runs in one, so the
/// gate must too.
fn void_shapes(_x: i32, _y: i32, _z: i32) -> &'static [[f32; 6]] {
    &[]
}
/// Solid below y = 0.
fn floor_shapes(_x: i32, y: i32, _z: i32) -> &'static [[f32; 6]] {
    if y < 0 {
        &FULL_CUBE
    } else {
        &[]
    }
}

/// Build a `level_particles` body the way a server would.
///
/// Deliberately hand-assembled rather than produced by a Rewo encoder: if the
/// same code wrote and read the packet, a wrong field order would round-trip
/// happily. The layout here is transcribed from
/// `ClientboundLevelParticlesPacket`'s write method.
#[allow(clippy::too_many_arguments)]
fn level_particles_body(
    override_limiter: bool,
    always_show: bool,
    x: f64,
    y: f64,
    z: f64,
    x_dist: f32,
    y_dist: f32,
    z_dist: f32,
    max_speed: f32,
    count: i32,
    type_id: i32,
    block_state: Option<i32>,
) -> Vec<u8> {
    let mut b = Vec::new();
    b.push(override_limiter as u8);
    b.push(always_show as u8);
    b.extend_from_slice(&x.to_be_bytes());
    b.extend_from_slice(&y.to_be_bytes());
    b.extend_from_slice(&z.to_be_bytes());
    b.extend_from_slice(&x_dist.to_be_bytes());
    b.extend_from_slice(&y_dist.to_be_bytes());
    b.extend_from_slice(&z_dist.to_be_bytes());
    b.extend_from_slice(&max_speed.to_be_bytes());
    b.extend_from_slice(&count.to_be_bytes()); // plain i32, NOT a VarInt
    write_varint(&mut b, type_id);
    if let Some(s) = block_state {
        write_varint(&mut b, s);
    }
    b
}

/// `level_event`: i32 type, packed BlockPos, i32 data, bool global.
fn level_event_body(kind: i32, x: i32, y: i32, z: i32, data: i32, global: bool) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&kind.to_be_bytes());
    let packed: u64 = (((x as i64) & 0x3FF_FFFF) as u64) << 38
        | (((z as i64) & 0x3FF_FFFF) as u64) << 12
        | ((y as i64) & 0xFFF) as u64;
    b.extend_from_slice(&packed.to_be_bytes());
    b.extend_from_slice(&data.to_be_bytes());
    b.push(global as u8);
    b
}

fn write_varint(out: &mut Vec<u8>, v: i32) {
    let mut u = v as u32;
    loop {
        if u & !0x7F == 0 {
            out.push(u as u8);
            return;
        }
        out.push(((u & 0x7F) | 0x80) as u8);
        u >>= 7;
    }
}

pub fn run(args: ParticleshotArgs) -> Result<(), String> {
    if !args.check {
        return Err("particleshot: only --check is implemented".into());
    }
    let paths = DataPaths::for_version(&args.version)
        .ok_or("no config dir for version data — the gate fails closed")?;
    let packets = Packets::load(&paths.packets_json())?;
    let types = ParticleTypes::load(&paths.registries_json())?;
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    wire_layer(&mut c, &packets, &types);
    fanout_layer(&mut c, &types);
    simulation_layer(&mut c);

    println!(
        "[particleshot] {} witnesses, {} failures",
        c.witnessed,
        c.failures.len()
    );
    if !c.failures.is_empty() {
        return Err(format!("particleshot: {} failed: {:?}", c.failures.len(), c.failures));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "particleshot: expected {EXPECTED_WITNESSES} witnesses, ran {} — the gate \
             fails closed on a witness that silently stopped running",
            c.witnessed
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Layer 1 — the wire
// ---------------------------------------------------------------------------

fn wire_layer(c: &mut Checker, packets: &Packets, types: &ParticleTypes) {
    // Both packets must exist under the names the client resolves. A version
    // bump that renames them has to fail here rather than silently disable
    // every particle in the game.
    let lp = packets.id(State::Play, Dir::Clientbound, "level_particles");
    let le = packets.id(State::Play, Dir::Clientbound, "level_event");
    c.record(
        "w1.packet_ids_resolve_by_name",
        lp.is_some() && le.is_some(),
        format!("level_particles={lp:?} level_event={le:?} from the datagen report"),
    );

    c.record(
        "w2.particle_registry_loaded",
        types.len() > 100 && types.block_id.is_some(),
        format!(
            "{} particle types, minecraft:block = {:?}",
            types.len(),
            types.block_id
        ),
    );

    // Every kind this milestone simulates must resolve name → behaviour.
    let mapped: Vec<(&str, Option<ParticleKind>)> = [
        "minecraft:block",
        "minecraft:smoke",
        "minecraft:flame",
        "minecraft:splash",
        "minecraft:crit",
        "minecraft:poof",
    ]
    .iter()
    .map(|n| {
        (
            *n,
            types
                .id_of(n)
                .and_then(|id| types.name(id))
                .and_then(ParticleKind::from_registry_name),
        )
    })
    .collect();
    c.record(
        "w3.every_supported_kind_round_trips_name_to_behaviour",
        mapped.iter().all(|(_, k)| k.is_some()),
        format!("{mapped:?}"),
    );

    // A full decode with every field distinct, so a transposed pair shows.
    let flame_id = types.id_of("minecraft:flame").unwrap();
    let body = level_particles_body(
        true, false, 10.5, 70.25, -3.75, 0.125, 0.25, 0.5, 0.0625, 7, flame_id, None,
    );
    let ev = rewo_net::route_level_particles(&body, types);
    let ok = match ev {
        Some(ParticleEvent::Command(cmd)) => {
            cmd.kind == ParticleKind::Flame
                && cmd.x == 10.5
                && cmd.y == 70.25
                && cmd.z == -3.75
                && cmd.x_dist == 0.125
                && cmd.y_dist == 0.25
                && cmd.z_dist == 0.5
                && cmd.max_speed == 0.0625
                && cmd.count == 7
                && cmd.override_limiter
                && !cmd.always_show
        }
        _ => false,
    };
    c.record(
        "w4.level_particles_fields_decode_in_order",
        ok,
        format!("{ev:?} — every field a distinct value, so a swap cannot pass"),
    );

    // `count` is a plain big-endian i32. A VarInt read would take one byte and
    // desync the type id after it, so this doubles as a guard on that.
    let big = level_particles_body(
        false, true, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 300, flame_id, None,
    );
    let big_ok = matches!(
        rewo_net::route_level_particles(&big, types),
        Some(ParticleEvent::Command(cmd)) if cmd.count == 300 && cmd.kind == ParticleKind::Flame
    );
    c.record(
        "w5.count_is_a_plain_i32_not_a_varint",
        big_ok,
        "count 300 decodes, and the type id after it still resolves — a VarInt \
         read here would consume one byte and misread the type",
    );

    // `minecraft:block` is the one supported kind carrying an options payload.
    let block_id = types.id_of("minecraft:block").unwrap();
    let with_state = level_particles_body(
        false, false, 1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 1, block_id, Some(1234),
    );
    let state_ok = matches!(
        rewo_net::route_level_particles(&with_state, types),
        Some(ParticleEvent::Command(cmd))
            if cmd.kind == ParticleKind::Terrain && cmd.block_state == 1234
    );
    c.record(
        "w6.block_particle_options_carry_a_state_id",
        state_ok,
        "BlockParticleOption appends the block state as a VarInt after the type",
    );

    // An unsupported type must be dropped, not guessed at. Most of the 125
    // registered types carry option payloads whose codecs Rewo has not
    // transcribed, so continuing past one would be reading garbage.
    let unsupported = types
        .id_of("minecraft:dust")
        .or_else(|| types.id_of("minecraft:note"))
        .unwrap();
    let dropped = rewo_net::route_level_particles(
        &level_particles_body(false, false, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1, unsupported, None),
        types,
    );
    c.record(
        "w7.unsupported_particle_types_are_dropped",
        dropped.is_none(),
        "an unmodelled type yields no event rather than a wrong one",
    );

    // Truncated bodies must not panic or invent a value.
    let full = level_particles_body(
        false, false, 1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 1, flame_id, None,
    );
    let truncated_all_none = (0..full.len()).all(|n| {
        std::panic::catch_unwind(|| rewo_net::route_level_particles(&full[..n], types).is_none())
            .unwrap_or(false)
    });
    c.record(
        "w8.truncated_bodies_decode_to_nothing",
        truncated_all_none,
        format!("every one of the {} prefixes yields None, no panic", full.len()),
    );

    // level_event 2001 carries the broken block's state in `data`.
    let destroy = level_event_body(LEVEL_EVENT_DESTROY_BLOCK, 100, 70, -200, 4321, false);
    let d_ok = matches!(
        rewo_net::route_level_event(&destroy),
        Some(ParticleEvent::DestroyBlock { x: 100, y: 70, z: -200, block_state: 4321 })
    );
    c.record(
        "w9.level_event_2001_decodes_pos_and_state",
        d_ok,
        "PARTICLES_DESTROY_BLOCK → position + block state, with negative \
         coordinates surviving the packed BlockPos",
    );

    // Every other level event is somebody else's business (sounds, effects).
    // Distinct ids straddling 2001, including its immediate neighbours so an
    // off-by-one comparison cannot pass.
    let others = [1000, 1002, 2000, 2002, 2003, 3000];
    let ignored = others
        .iter()
        .all(|k| rewo_net::route_level_event(&level_event_body(*k, 0, 0, 0, 0, false)).is_none());
    c.record(
        "w10.other_level_events_are_ignored",
        ignored,
        format!("{others:?} produce no particle event"),
    );
}

// ---------------------------------------------------------------------------
// Layer 2 — the fan-out
// ---------------------------------------------------------------------------

fn fanout_layer(c: &mut Checker, types: &ParticleTypes) {
    let flame_id = types.id_of("minecraft:flame").unwrap();

    // The whole design rests on this: same seed, same commands, same bytes.
    let run_once = || {
        let mut sys = ParticleSystem::new(0x5EED);
        let body = level_particles_body(
            false, false, 8.5, 65.0, 8.5, 0.5, 0.5, 0.5, 0.02, 32, flame_id, None,
        );
        if let Some(ParticleEvent::Command(cmd)) = rewo_net::route_level_particles(&body, types) {
            sys.spawn_from_packet(&cmd, &floor_shapes);
        }
        for _ in 0..20 {
            sys.tick(&floor_shapes);
        }
        sys.particles
            .iter()
            .map(|p| (p.x.to_bits(), p.y.to_bits(), p.z.to_bits(), p.age))
            .collect::<Vec<_>>()
    };
    let a = run_once();
    let b = run_once();
    c.record(
        "f1.a_seeded_system_is_byte_identical_across_runs",
        !a.is_empty() && a == b,
        format!(
            "{} particles alive after 20 ticks, identical bit patterns — this is \
             the property that makes every assertion below possible",
            a.len()
        ),
    );

    // ...and a different seed must actually differ, or f1 is vacuous.
    let other = {
        let mut sys = ParticleSystem::new(0x1234);
        let body = level_particles_body(
            false, false, 8.5, 65.0, 8.5, 0.5, 0.5, 0.5, 0.02, 32, flame_id, None,
        );
        if let Some(ParticleEvent::Command(cmd)) = rewo_net::route_level_particles(&body, types) {
            sys.spawn_from_packet(&cmd, &floor_shapes);
        }
        for _ in 0..20 {
            sys.tick(&floor_shapes);
        }
        sys.particles
            .iter()
            .map(|p| (p.x.to_bits(), p.y.to_bits(), p.z.to_bits(), p.age))
            .collect::<Vec<_>>()
    };
    c.record(
        "f2.determinism_witness_is_not_vacuous",
        other != a,
        "a different master seed produces a different run, so f1 is measuring \
         determinism rather than a constant",
    );

    // `count` particles spawn, one per iteration of handleParticleEvent's loop.
    let mut sys = ParticleSystem::new(1);
    let body = level_particles_body(
        false, false, 0.0, 64.0, 0.0, 0.1, 0.1, 0.1, 0.01, 25, flame_id, None,
    );
    if let Some(ParticleEvent::Command(cmd)) = rewo_net::route_level_particles(&body, types) {
        sys.spawn_from_packet(&cmd, &void_shapes);
    }
    c.record(
        "f3.count_n_spawns_n_particles",
        sys.len() == 25,
        format!("count 25 → {} particles", sys.len()),
    );

    // The count == 0 inversion: dist becomes a direction, max_speed its
    // magnitude, and exactly one particle spawns.
    let mut zero = ParticleSystem::new(1);
    let zbody = level_particles_body(
        false, false, 0.0, 64.0, 0.0, 1.0, 0.0, 0.0, 0.5, 0, flame_id, None,
    );
    let zcmd = rewo_net::route_level_particles(&zbody, types);
    if let Some(ParticleEvent::Command(cmd)) = zcmd {
        zero.spawn_from_packet(&cmd, &void_shapes);
    }
    let directed = zero.len() == 1 && zero.particles[0].xd > 0.4;
    c.record(
        "f4.zero_count_is_one_directed_particle",
        directed,
        format!(
            "one particle with xd={:.4} — with count 0 the dist fields are a \
             DIRECTION and max_speed its magnitude, not a scatter",
            zero.particles.first().map(|p| p.xd).unwrap_or(0.0)
        ),
    );

    // Scatter must be centred on the requested point, and must actually
    // scatter. Both halves matter: a broken gaussian that returned 0 would
    // still centre correctly.
    let mut scat = ParticleSystem::new(77);
    let sbody = level_particles_body(
        false, false, 100.0, 64.0, -50.0, 2.0, 2.0, 2.0, 0.0, 400, flame_id, None,
    );
    if let Some(ParticleEvent::Command(cmd)) = rewo_net::route_level_particles(&sbody, types) {
        scat.spawn_from_packet(&cmd, &void_shapes);
    }
    let n = scat.len() as f64;
    let mean_x = scat.particles.iter().map(|p| p.x).sum::<f64>() / n;
    let mean_z = scat.particles.iter().map(|p| p.z).sum::<f64>() / n;
    let var_x = scat.particles.iter().map(|p| (p.x - mean_x).powi(2)).sum::<f64>() / n;
    // xDist 2.0 → unit gaussian scaled by 2 → variance ~4 (loose bounds: this
    // is a distributional claim, and the exact draws are pinned elsewhere).
    c.record(
        "f5.scatter_is_centred_on_the_packet_position",
        (mean_x - 100.0).abs() < 0.5 && (mean_z + 50.0).abs() < 0.5,
        format!("mean ({mean_x:.3}, {mean_z:.3}) against requested (100, -50)"),
    );
    c.record(
        "f6.scatter_width_tracks_x_dist",
        var_x > 1.0 && var_x < 9.0,
        format!("variance {var_x:.3} for xDist 2.0 (expected ~4)"),
    );

    // The block-break grid: 4x4x4 for a full cube, from
    // max(2, ceil(width / 0.25)) per axis.
    let mut brk = ParticleSystem::new(3);
    brk.spawn_destroy_block(10, 70, -4, 9, &FULL_CUBE, &void_shapes);
    let inside = brk
        .particles
        .iter()
        .all(|p| p.x > 10.0 && p.x < 11.0 && p.y > 70.0 && p.y < 71.0 && p.z > -4.0 && p.z < -3.0);
    c.record(
        "f7.destroy_block_is_a_4x4x4_grid",
        brk.len() == 64 && inside,
        format!("{} shards, all inside the block", brk.len()),
    );

    // ...and it follows the block's actual shape rather than assuming a cube.
    let slab = [[0.0f32, 0.0, 0.0, 1.0, 0.5, 1.0]];
    let mut slab_sys = ParticleSystem::new(3);
    slab_sys.spawn_destroy_block(0, 0, 0, 9, &slab, &void_shapes);
    c.record(
        "f8.destroy_block_follows_the_block_shape",
        slab_sys.len() == 32 && slab_sys.particles.iter().all(|p| p.y < 0.5),
        format!(
            "a half slab gives {} shards (count_y collapses to 2), all in the \
             lower half",
            slab_sys.len()
        ),
    );

    // A shapeless block spawns nothing — vanilla iterates the collision boxes,
    // so no boxes means no shards.
    let mut none_sys = ParticleSystem::new(3);
    none_sys.spawn_destroy_block(0, 0, 0, 9, &[], &void_shapes);
    c.record(
        "f9.a_shapeless_block_spawns_no_shards",
        none_sys.is_empty(),
        "forAllBoxes over an empty shape is a no-op",
    );

    // The engine cap is enforced at spawn rather than by evicting live ones.
    let mut capped = ParticleSystem::new(3);
    capped.limit = 20;
    capped.spawn_destroy_block(0, 0, 0, 9, &FULL_CUBE, &void_shapes);
    c.record(
        "f10.the_pool_is_capped",
        capped.len() == 20,
        format!("limit 20 held against a 64-shard burst → {}", capped.len()),
    );

    // The sprite sets, against the jar's own particle JSONs. `splash` lists
    // FOUR textures even though nothing about it looks animated, and the pick
    // is a `get(random)` on the ENGINE generator — so it consumes a draw as a
    // constructor argument, before any of the particle's own.
    let expected: [(ParticleKind, u32, bool); 6] = [
        (ParticleKind::Flame, 1, true),   // flame
        (ParticleKind::Crit, 1, true),    // critical_hit
        (ParticleKind::Splash, 4, true),  // splash_0..3
        (ParticleKind::Smoke, 8, false),  // generic_7..0, by age
        (ParticleKind::Poof, 8, false),   // generic_7..0, by age
        (ParticleKind::Terrain, 1, false),// from the block model
    ];
    let sprites_ok = expected
        .iter()
        .all(|(k, n, pick)| k.sprite_frames() == *n && k.picks_sprite_at_spawn() == *pick);
    c.record(
        "f12.sprite_sets_match_the_jar_particle_definitions",
        sprites_ok,
        "frame counts and get(random)-vs-get(age,lifetime) per          assets/minecraft/particles/*.json",
    );

    // A spawn-time sprite pick must actually move the engine stream: skipping
    // `nextInt(1)` for a single-texture set would shift every later particle.
    let mut with_pick = ParticleSystem::new(99);
    let flame_cmd = level_particles_body(
        false, false, 0.0, 64.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2, flame_id, None,
    );
    if let Some(ParticleEvent::Command(cmd)) = rewo_net::route_level_particles(&flame_cmd, types) {
        with_pick.spawn_from_packet(&cmd, &void_shapes);
    }
    // The same two spawns from a system whose stream was advanced by one draw
    // must NOT land in the same place — which is what proves the pick is real.
    let mut advanced = ParticleSystem::new(99);
    advanced.burn_one_draw_for_test();
    if let Some(ParticleEvent::Command(cmd)) = rewo_net::route_level_particles(&flame_cmd, types) {
        advanced.spawn_from_packet(&cmd, &void_shapes);
    }
    let differs = with_pick
        .particles
        .iter()
        .zip(advanced.particles.iter())
        .any(|(a, b)| a.x.to_bits() != b.x.to_bits());
    c.record(
        "f13.a_spawn_time_sprite_pick_consumes_an_engine_draw",
        differs && with_pick.len() == 2,
        "advancing the engine stream by one draw changes where the particles          land, so nextInt(1) is not a no-op that could be skipped",
    );

    // Dead particles are retired.
    let mut life = ParticleSystem::new(11);
    life.spawn_destroy_block(0, 5, 0, 9, &FULL_CUBE, &void_shapes);
    let spawned = life.len();
    for _ in 0..400 {
        life.tick(&void_shapes);
    }
    c.record(
        "f11.particles_are_retired_at_end_of_life",
        spawned == 64 && life.is_empty(),
        format!("{spawned} shards → {} after 400 ticks", life.len()),
    );
}

// ---------------------------------------------------------------------------
// Layer 3 — the simulation, against the verbatim-source oracle
// ---------------------------------------------------------------------------

/// The oracle file lives beside the implementation it grades.
const ORACLE: &str = include_str!("../../rewo-world/src/particles_oracle_26_2.txt");

struct Row {
    seed: i64,
    lifetime: i32,
    ticks: Vec<[i64; 6]>,
    removed: Vec<bool>,
}

fn rows(kind: &str) -> Vec<Row> {
    ORACLE
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split(' ');
            if it.next()? != kind {
                return None;
            }
            let seed = it.next()?.parse().ok()?;
            let lifetime = it.next()?.parse().ok()?;
            let mut ticks = Vec::new();
            let mut removed = Vec::new();
            for t in it.next()?.split(';').filter(|s| !s.is_empty()) {
                let f: Vec<&str> = t.split(',').collect();
                ticks.push([
                    f[0].parse().ok()?,
                    f[1].parse().ok()?,
                    f[2].parse().ok()?,
                    f[3].parse().ok()?,
                    f[4].parse().ok()?,
                    f[5].parse().ok()?,
                ]);
                removed.push(f[6] == "1");
            }
            Some(Row { seed, lifetime, ticks, removed })
        })
        .collect()
}

fn grade_kind(c: &mut Checker, witness: &str, kind: &str, build: impl Fn(i64) -> Particle) {
    let rows = rows(kind);
    if rows.is_empty() {
        c.record(witness, false, "no oracle rows — the gate fails closed");
        return;
    }
    let mut compared = 0usize;
    let mut bad: Option<String> = None;
    for row in &rows {
        let mut p = build(row.seed);
        if p.lifetime != row.lifetime {
            bad = Some(format!(
                "seed {}: lifetime {} != vanilla {}",
                row.seed, p.lifetime, row.lifetime
            ));
            break;
        }
        for (t, want) in row.ticks.iter().enumerate() {
            let got = [
                p.x.to_bits() as i64,
                p.y.to_bits() as i64,
                p.z.to_bits() as i64,
                p.xd.to_bits() as i64,
                p.yd.to_bits() as i64,
                p.zd.to_bits() as i64,
            ];
            if got != *want || p.removed != row.removed[t] {
                bad = Some(format!(
                    "seed {} tick {t}: {got:?} != vanilla {want:?}",
                    row.seed
                ));
                break;
            }
            compared += 6;
            p.tick(&void_shapes);
        }
        if bad.is_some() {
            break;
        }
    }
    match bad {
        Some(why) => c.record(witness, false, why),
        None => c.record(
            witness,
            true,
            format!(
                "{} seeds x {} ticks, {compared} f64s bit-identical to vanilla's own \
                 source compiled and run on a JVM",
                rows.len(),
                rows.first().map(|r| r.ticks.len()).unwrap_or(0)
            ),
        ),
    }
}

fn simulation_layer(c: &mut Checker) {
    grade_kind(c, "s1.flame_trajectory", "flame", |s| {
        Particle::flame(10.5, 70.0, -3.25, 0.0, 0.05, 0.0, LegacyRandom::new(s))
    });
    grade_kind(c, "s2.smoke_trajectory", "smoke", |s| {
        Particle::smoke(10.5, 70.0, -3.25, 0.01, 0.02, -0.01, LegacyRandom::new(s))
    });
    grade_kind(c, "s3.splash_trajectory", "splash", |s| {
        Particle::splash(10.5, 70.0, -3.25, 0.0, 0.0, 0.0, LegacyRandom::new(s))
    });
    grade_kind(c, "s4.crit_trajectory", "crit", |s| {
        Particle::crit(
            10.5, 70.0, -3.25, 0.1, 0.2, -0.1,
            LegacyRandom::new(s), &void_shapes,
        )
    });
    grade_kind(c, "s5.poof_trajectory", "poof", |s| {
        Particle::poof(10.5, 70.0, -3.25, 0.0, 0.1, 0.0, LegacyRandom::new(s))
    });
    grade_kind(c, "s6.terrain_trajectory", "terrain", |s| {
        Particle::terrain(10.5, 70.0, -3.25, 0.25, -0.125, 0.5, 1, LegacyRandom::new(s))
    });

    // The oracle runs in an empty world, so collision needs its own witness
    // with an independently-stated stop position: the floor's top face is
    // y = 0, and a particle's y IS its bounding-box minimum, so it can never
    // go below 0.
    let mut p = Particle::terrain(0.5, 3.0, 0.5, 0.0, 0.0, 0.0, 1, LegacyRandom::new(9));
    p.lifetime = 400;
    let mut landed = false;
    let mut fell_through = false;
    for _ in 0..200 {
        p.tick(&floor_shapes);
        landed |= p.on_ground;
        fell_through |= p.y < -1e-9;
    }
    c.record(
        "s7.a_falling_particle_rests_on_the_block_face",
        landed && !fell_through && p.y.abs() < 1e-9,
        format!("settled at y={:.12}, on_ground seen={landed}", p.y),
    );

    let mut q = Particle::terrain(0.5, 3.0, 0.5, 0.0, 0.0, 0.0, 1, LegacyRandom::new(9));
    q.lifetime = 400;
    for _ in 0..200 {
        q.tick(&void_shapes);
    }
    c.record(
        "s8.collision_witness_is_not_vacuous",
        q.y < -1.0 && !q.on_ground,
        format!(
            "with no floor the same particle falls to y={:.2}, so s7 is measuring \
             collision rather than a particle that never moved",
            q.y
        ),
    );

    // Flame overrides `move` to skip collision entirely — it drifts through
    // blocks by design, and a "fix" that routed it through the collider would
    // be a regression.
    let mut f = Particle::flame(0.5, 3.0, 0.5, 0.0, -0.5, 0.0, LegacyRandom::new(4));
    f.lifetime = 400;
    for _ in 0..120 {
        f.tick(&floor_shapes);
    }
    c.record(
        "s9.flame_ignores_collision_by_design",
        f.y < 0.0,
        format!(
            "FlameParticle.move bypasses collideBoundingBox, so it passes the \
             floor to y={:.3}",
            f.y
        ),
    );

    // Crit ticks once inside its own constructor — age 1 before any external
    // tick. Easy to drop when porting, and it shifts the whole colour decay.
    let crit = Particle::crit(
        0.0, 64.0, 0.0, 0.0, 0.0, 0.0,
        LegacyRandom::new(42), &void_shapes,
    );
    c.record(
        "s10.crit_ticks_inside_its_constructor",
        crit.age == 1 && crit.g_col < crit.r_col && crit.b_col < crit.g_col,
        format!(
            "age={} r={:.5} g={:.5} b={:.5} — one decay of gCol*0.96 and bCol*0.9 \
             has already run",
            crit.age, crit.r_col, crit.g_col, crit.b_col
        ),
    );

    // Sprite animation indexes age * (frames-1) / lifetime, so an 8-frame
    // smoke walks 0..7 and ends on the last frame rather than wrapping.
    let mut sm = Particle::smoke(0.0, 64.0, 0.0, 0.0, 0.0, 0.0, LegacyRandom::new(6));
    let life = sm.lifetime;
    let mut frames = Vec::new();
    for _ in 0..life {
        frames.push(sm.sprite_frame);
        sm.tick(&void_shapes);
    }
    let monotonic = frames.windows(2).all(|w| w[1] >= w[0]);
    c.record(
        "s11.sprite_frames_advance_monotonically_to_the_last",
        monotonic && *frames.first().unwrap() == 0 && *frames.last().unwrap() <= 7,
        format!("lifetime {life}: frames {:?}", frames),
    );
}
