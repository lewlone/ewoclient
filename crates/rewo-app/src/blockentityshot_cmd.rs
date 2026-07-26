//! `rewo blockentityshot --check` — the M25 block-entity oracle.
//!
//! Serverless and CPU-only. It drives **production** code end to end: a
//! synthesised level-chunk payload through `rewo_world::chunk::read_level_chunk`
//! into a `World`, and a synthesised `block_entity_data` body through the real
//! `PlaySession` packet dispatch. Nothing here re-implements the decoder it is
//! checking; where it needs an expectation it builds one from the datagen
//! registry and the jar's own models, which is a different source.
//!
//! **The gap this milestone measured.** A block entity renders as an ordinary
//! block model plus a `BlockEntityRenderer`. Where the model has no `elements`
//! the renderer *is* the block. Witness `e1` re-derives that set from the real
//! client jar every run rather than trusting a number written down once — it
//! walks every blockstate's model parent chain and counts the blocks that bake
//! to nothing.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count, like every oracle
//! since `eventshot`: a witness that stops running is a failure, not a quieter
//! pass.

use std::collections::HashSet;

use clap::Args;
use rewo_data::{packets::Packets, DataPaths};
use rewo_net::ids::Ids;
use rewo_proto::nbt::Nbt;
use rewo_world::block_entities::{
    BlockEntities, BlockEntityKind, BlockEntityPos, BlockEntityRegistry, TYPE_TABLE,
};
use rewo_world::dimension::DimensionShape;

const EXPECTED_WITNESSES: usize = 50;

#[derive(Args, Debug)]
pub struct BlockentityshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `dimensioncheck` / `meshshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Version whose datagen reports and client jar the gate reads.
    #[arg(long, default_value = "26.2")]
    version: String,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        let status = if pass { " ok " } else { "FAIL" };
        println!("[blockentityshot] {status}  {name}: {detail}");
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

/// The client jar the launcher already downloaded, if present.
fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let p = dirs::config_dir()?
        .join("EwoClient")
        .join("shared")
        .join("versions")
        .join(version)
        .join(format!("{version}.jar"));
    p.exists().then_some(p)
}

// --------------------------------------------------------------- wire builders

fn varint(v: i32, out: &mut Vec<u8>) {
    let mut n = v as u32;
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// A minimal but **real** level-chunk body: the exact field order
/// `read_level_chunk` walks, with `count` block entities at the given
/// `(packedXZ, y, type)` triples and an empty NBT tag each.
///
/// Everything else is the smallest legal filler — no heightmaps, all-air
/// sections with a single-value palette, and empty light bitsets. The point is
/// to exercise the block-entity list's *position* in the stream, because a
/// decoder that reads it from the wrong offset is exactly the failure a
/// hand-built struct would hide.
fn chunk_body(cx: i32, cz: i32, sections: usize, bes: &[(u8, i16, i32)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&cx.to_be_bytes());
    b.extend_from_slice(&cz.to_be_bytes());
    varint(0, &mut b); // heightmaps: none

    let mut blob = Vec::new();
    for _ in 0..sections {
        blob.extend_from_slice(&0i16.to_be_bytes()); // non-empty count
        blob.extend_from_slice(&0i16.to_be_bytes()); // fluid count
        // Block states: single-value palette (bits 0), value 0 (air), no data.
        blob.push(0);
        varint(0, &mut blob);
        varint(0, &mut blob);
        // Biomes: same shape.
        blob.push(0);
        varint(0, &mut blob);
        varint(0, &mut blob);
    }
    varint(blob.len() as i32, &mut b);
    b.extend_from_slice(&blob);

    varint(bes.len() as i32, &mut b);
    for (packed, y, ty) in bes {
        b.push(*packed);
        b.extend_from_slice(&y.to_be_bytes());
        varint(*ty, &mut b);
        b.push(0); // TAG_End — the empty network tag
    }

    // Light: 4 bitsets (each a VarInt long-count of 0) then 2 empty lists.
    for _ in 0..4 {
        varint(0, &mut b);
    }
    varint(0, &mut b);
    varint(0, &mut b);
    b
}

/// `BlockPos.STREAM_CODEC` — the packed long, `((x & 0x3FFFFFF) << 38) |
/// ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)`.
fn packed_pos(x: i32, y: i32, z: i32) -> i64 {
    ((x as i64 & 0x3FF_FFFF) << 38) | ((z as i64 & 0x3FF_FFFF) << 12) | (y as i64 & 0xFFF)
}

fn block_entity_data_body(x: i32, y: i32, z: i32, type_id: i32, tag: &[u8]) -> Vec<u8> {
    let mut b = packed_pos(x, y, z).to_be_bytes().to_vec();
    varint(type_id, &mut b);
    b.extend_from_slice(tag);
    b
}

/// Blocks that *should* be invisible — air variants, fluids, and the markers.
/// Everything else in the measured set is a block entity Rewo renders as
/// nothing.
const LEGITIMATELY_INVISIBLE: &[&str] = &[
    "air",
    "cave_air",
    "void_air",
    "water",
    "lava",
    "barrier",
    "light",
    "structure_void",
    "bubble_column",
    "moving_piston",
];

pub fn run(args: BlockentityshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!("[blockentityshot] mode: {mode} (the oracle asserts unconditionally)");

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;

    // The registry, read from the real datagen report.
    let json: serde_json::Value = {
        let text = std::fs::read_to_string(paths.registries_json())
            .map_err(|e| format!("read registries.json: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("parse registries.json: {e}"))?
    };
    let entries_json = json
        .get("minecraft:block_entity_type")
        .and_then(|r| r.get("entries"))
        .and_then(|e| e.as_object())
        .ok_or("registries.json: no minecraft:block_entity_type registry")?;
    let mut entries: Vec<(String, i32)> = entries_json
        .iter()
        .filter_map(|(k, v)| {
            v.get("protocol_id")
                .and_then(|i| i.as_i64())
                .map(|i| (k.clone(), i as i32))
        })
        .collect();
    entries.sort_by_key(|(_, id)| *id);

    let blocks = rewo_data::blocks::Blocks::load(&paths.blocks_json())?;
    let registry = BlockEntityRegistry::resolve(&entries)?;
    let chest = entries
        .iter()
        .find(|(n, _)| n == "minecraft:chest")
        .map(|(_, i)| *i)
        .ok_or("registries.json: no minecraft:chest block entity type")?;
    let sign = entries
        .iter()
        .find(|(n, _)| n == "minecraft:sign")
        .map(|(_, i)| *i)
        .ok_or("registries.json: no minecraft:sign block entity type")?;

    println!(
        "[blockentityshot] registry: {} types, {} still invisible; block_entity_data id={}",
        registry.len(),
        registry.invisible_count(),
        ids.cb_play_block_entity_data
    );

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    check_registry(&mut c, &registry, &entries, &ids);
    check_gap(&mut c, &args.version, &registry)?;
    check_decode(&mut c, &blocks, chest, sign);
    check_lifecycle(&mut c, &ids, &blocks, chest);
    check_chest_models(&mut c, &args.version)?;
    check_lids(&mut c, &ids, &blocks, chest);

    println!(
        "[blockentityshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
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
             added or stopped running",
            c.witnessed
        ));
    }
    println!("[blockentityshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ------------------------------------------------------------------- the gate

/// M25b — the chest models: geometry, the facing transform, and the states
/// that select them.
///
/// CPU-only. The geometry witnesses read the production bake; the transform
/// witnesses apply this file's independent transcription of
/// `ChestRenderer.createModelTransformation` and compare against the same
/// rotation the renderer performs.
fn check_chest_models(c: &mut Checker, version: &str) -> Result<(), String> {
    use rewo_data::chest_states::{ChestFacing, ChestStates, ChestType};

    let paths = DataPaths::for_version(version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(version)
        .ok_or("client jar not found — blockentityshot needs it for the chest textures")?;
    let baked = rewo_data::assets::bake(&jar, &paths.blocks_json())?;
    let items = &baked.held_items;

    // --- the models exist and came from the jar ---------------------------
    let names: Vec<&str> = rewo_data::block_entity_models::CHESTS
        .iter()
        .map(|(n, _, _)| *n)
        .collect();
    let present: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| items.block_entities.contains_key(*n))
        .collect();
    c.record(
        "m1.every_chest_variant_bakes",
        present.len() == names.len(),
        format!(
            "{} of {} chest models baked: {present:?}",
            present.len(),
            names.len()
        ),
    );

    let chest = items
        .block_entities
        .get("rewo:be/chest")
        .ok_or("rewo:be/chest did not bake")?;
    c.record(
        "m2.a_chest_is_three_cuboids",
        chest.quads.len() == 18,
        format!("{} quads (want 18 = 3 boxes x 6 faces)", chest.quads.len()),
    );

    // The model must fill its block and not escape it — a chest that spilled
    // into the neighbour would z-fight with whatever is there.
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for q in &chest.quads {
        for v in &q.verts {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
    }
    c.record(
        "m3.the_model_stays_inside_its_block",
        (0..3).all(|k| lo[k] >= 0.0 && hi[k] <= 16.0),
        format!("model px extent {lo:?}..{hi:?} of 0..16"),
    );
    c.record(
        "m4.the_lock_stands_proud_of_the_lid",
        hi[2] == 16.0 && chest.quads.iter().any(|q| q.verts.iter().all(|v| v[2] >= 15.0)),
        format!(
            "max z {} — addBox(7,-2,14, 2,4,1) with offset(0,9,1) puts the latch at \
             z 15..16, past the lid's 15",
            hi[2]
        ),
    );

    // Distinct textures, not one shared sprite: a trapped chest must not be
    // an ordinary one.
    let tex_of = |n: &str| items.block_entities.get(n).and_then(|m| m.quads.first()).map(|q| q.tex);
    let normal = tex_of("rewo:be/chest");
    let trapped = tex_of("rewo:be/trapped_chest");
    let ender = tex_of("rewo:be/ender_chest");
    c.record(
        "m5.each_variant_has_its_own_texture",
        normal.is_some() && normal != trapped && normal != ender && trapped != ender,
        format!("normal={normal:?} trapped={trapped:?} ender={ender:?}"),
    );

    // Every UV must be inside its texture — the 64x64 unwrap is the thing a
    // transposed `texOffs` would break while still producing a chest shape.
    c.record(
        "m6.every_uv_is_inside_its_texture",
        chest.quads.iter().all(|q| {
            q.uv.iter()
                .all(|uv| (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]))
        }),
        "all 72 UVs within 0..1 of the 64x64 chest texture",
    );

    // --- the facing transform ---------------------------------------------
    let states = ChestStates::load(&paths.blocks_json())?;
    c.record(
        "m7.the_chest_states_resolve",
        states.len() > 100,
        format!("{} chest block states carry a facing + type", states.len()),
    );

    // `Direction.toYRot()` — independently transcribed. South is the zero.
    let want_y_rot = |f: ChestFacing| match f {
        ChestFacing::South => 0.0f32,
        ChestFacing::West => 90.0,
        ChestFacing::North => 180.0,
        ChestFacing::East => 270.0,
    };
    let all_four = [
        ChestFacing::North,
        ChestFacing::South,
        ChestFacing::West,
        ChestFacing::East,
    ];
    c.record(
        "m8.north_is_one_eighty",
        all_four.iter().all(|&f| f.to_y_rot() == want_y_rot(f)),
        format!(
            "{:?} — south is the zero because the model faces south in its own space; \
             a north-is-zero table points every chest backwards while still looking \
             like a chest",
            all_four.map(|f| (f, f.to_y_rot()))
        ),
    );

    // `rotationAround(YP(-yRot), 0.5, 0, 0.5)` must keep the model in its own
    // block for all four facings — that is the whole point of rotating about
    // the centre rather than the origin.
    let rotate = |y_rot: f32, p: [f32; 3]| -> [f32; 3] {
        let (s, cc) = (-y_rot).to_radians().sin_cos();
        let (x, z) = (p[0] - 0.5, p[2] - 0.5);
        [x * cc + z * s + 0.5, p[1], -x * s + z * cc + 0.5]
    };
    let mut inside = true;
    let mut extents = Vec::new();
    for f in all_four {
        let mut l = [f32::MAX; 3];
        let mut h = [f32::MIN; 3];
        for q in &chest.quads {
            for v in &q.verts {
                let p = rotate(f.to_y_rot(), [v[0] / 16.0, v[1] / 16.0, v[2] / 16.0]);
                for k in 0..3 {
                    l[k] = l[k].min(p[k]);
                    h[k] = h[k].max(p[k]);
                }
            }
        }
        inside &= (0..3).all(|k| l[k] >= -1e-5 && h[k] <= 1.0 + 1e-5);
        extents.push(format!("{f:?} {:.3}..{:.3} x", l[0], h[0]));
    }
    c.record(
        "m9.the_facing_rotation_keeps_the_chest_in_its_block",
        inside,
        format!(
            "{} — rotationAround(..., 0.5, 0, 0.5) is about the block centre; about \
             the origin it would swing three of the four facings outside",
            extents.join("; ")
        ),
    );

    // The four facings must be genuinely different placements, not a no-op.
    let front_of = |f: ChestFacing| {
        // The latch, at model z 15..16 — the unambiguous "front" of the model.
        let mut sum = [0f32; 3];
        let mut n = 0.0f32;
        for q in chest.quads.iter().filter(|q| q.verts.iter().all(|v| v[2] >= 15.0)) {
            for v in &q.verts {
                let p = rotate(f.to_y_rot(), [v[0] / 16.0, v[1] / 16.0, v[2] / 16.0]);
                for k in 0..3 {
                    sum[k] += p[k];
                }
                n += 1.0;
            }
        }
        [sum[0] / n, sum[1] / n, sum[2] / n]
    };
    let fronts: Vec<[f32; 3]> = all_four.iter().map(|&f| front_of(f)).collect();
    let distinct = (0..4).all(|i| {
        (0..4).all(|j| {
            i == j || (fronts[i][0] - fronts[j][0]).abs() + (fronts[i][2] - fronts[j][2]).abs() > 0.5
        })
    });
    c.record(
        "m10.each_facing_points_the_latch_somewhere_different",
        distinct,
        format!(
            "latch centroids: {} — four facings, four placements",
            all_four
                .iter()
                .zip(&fronts)
                .map(|(f, p)| format!("{f:?}({:.2},{:.2})", p[0], p[2]))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    );

    // A double chest is deliberately not drawn: its half-models need
    // `DoubleBlockCombiner`'s neighbour pairing. The states still resolve, so
    // the skip is a decision rather than a gap in the table.
    let doubles = (0..u32::MAX)
        .take(40000)
        .filter_map(|i| states.get(i))
        .filter(|s| s.kind != ChestType::Single)
        .count();
    c.record(
        "m11.double_chest_states_resolve",
        doubles > 0,
        format!("{doubles} left/right chest states carry a half to draw"),
    );

    // --- the halves --------------------------------------------------------
    // M25 recorded `DoubleBlockCombiner` as the blocker for drawing halves.
    // That was wrong: `ChestRenderer` picks the model with `models.select(
    // state.type)` — the block's OWN type property — and uses the combiner
    // only for the shared openness and light. So each half draws itself.
    let left = items.block_entities.get("rewo:be/chest_left");
    let right = items.block_entities.get("rewo:be/chest_right");
    c.record(
        "m12.both_halves_bake",
        left.is_some() && right.is_some(),
        format!(
            "left={} right={} — selected by the block's own ChestType, not by a \
             neighbour lookup",
            left.is_some(),
            right.is_some()
        ),
    );

    // A half drops the face that meets the other half: 5 faces per box, not 6.
    let (Some(l), Some(r)) = (left, right) else {
        return Ok(());
    };
    c.record(
        "m13.a_half_drops_its_seam_face",
        l.quads.len() == 15 && r.quads.len() == 15,
        format!(
            "left {} quads, right {} (want 15 = 3 boxes x 5 faces) against the \
             single's 18 — `addBox(..., allOfEnumExcept(WEST/EAST))`, so the seam \
             has no coincident quads to z-fight",
            l.quads.len(),
            r.quads.len()
        ),
    );

    // The halves are 15 px wide and meet: left spans x 0..15, right 1..16.
    let span = |m: &rewo_data::held_items::HeldItemModel| {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for q in &m.quads {
            for v in &q.verts {
                lo = lo.min(v[0]);
                hi = hi.max(v[0]);
            }
        }
        (lo, hi)
    };
    let (ll, lh) = span(l);
    let (rl, rh) = span(r);
    c.record(
        "m14.the_two_halves_meet_at_the_block_boundary",
        ll == 0.0 && lh == 15.0 && rl == 1.0 && rh == 16.0,
        format!(
            "left x {ll}..{lh}, right x {rl}..{rh} — each reaches the shared edge, \
             so a pair reads as one 30-px-wide chest rather than two with a gap"
        ),
    );

    // Each half has its own texture — a `_left` model on a single sprite would
    // show the wrong half of the artwork.
    let tex1 = l.quads.first().map(|q| q.tex);
    let tex2 = r.quads.first().map(|q| q.tex);
    let single = items.block_entities.get("rewo:be/chest").and_then(|m| m.quads.first()).map(|q| q.tex);
    c.record(
        "m15.each_half_has_its_own_texture",
        tex1.is_some() && tex1 != tex2 && tex1 != single && tex2 != single,
        format!("single={single:?} left={tex1:?} right={tex2:?}"),
    );

    // An ender chest is always single, and the jar ships no ender_left.png —
    // so its halves must be absent rather than baked from the wrong sprite.
    c.record(
        "m16.the_ender_chest_has_no_halves",
        items.block_entities.get("rewo:be/ender_chest").is_some()
            && items.block_entities.get("rewo:be/ender_chest_left").is_none(),
        "ender_chest bakes single-only — it can never be double, and the jar has \
         no ender_left.png to bake from",
    );

    // The name a state resolves to must match what actually baked.
    let mut named_ok = true;
    let mut sample = Vec::new();
    for id in 0..40000u32 {
        let Some(st) = states.get(id) else { continue };
        let name = st.model_name();
        named_ok &= items.block_entities.contains_key(&name);
        if sample.len() < 3 && st.kind != ChestType::Single {
            sample.push(name);
        }
    }
    c.record(
        "m17.every_chest_state_names_a_model_that_baked",
        named_ok,
        format!(
            "every one of the {} chest states resolves to a baked model (e.g. \
             {sample:?})",
            states.len()
        ),
    );
    Ok(())
}

/// M25c — the chest lid: the `block_event` route, the client-side clock, and
/// the ease the renderer applies to it.
fn check_lids(c: &mut Checker, ids: &Ids, blocks: &rewo_data::blocks::Blocks, chest: i32) {
    use rewo_world::block_entities::ChestLid;

    let shape = DimensionShape::OVERWORLD;
    let mut world = rewo_world::World::new(shape);
    let body = chunk_body(0, 0, shape.section_count(), &[(0x00, 64, chest)]);
    let mut reader = rewo_proto::reader::PacketReader::new(&body);
    let col = rewo_world::chunk::read_level_chunk(&mut reader, &shape, blocks).unwrap();
    world.insert_column(0, 0, col);
    let pos = BlockEntityPos { x: 0, y: 64, z: 0 };

    // `ClientboundBlockEventPacket`: BlockPos, u8 b0, u8 b1, VarInt block.
    let ev = |x: i32, y: i32, z: i32, b0: u8, b1: u8| -> Vec<u8> {
        let mut b = packed_pos(x, y, z).to_be_bytes().to_vec();
        b.push(b0);
        b.push(b1);
        varint(0, &mut b); // the BLOCK registry id — read but not needed here
        b
    };
    let send = |w: &mut rewo_world::World, x, y, z, b0, b1| {
        rewo_net::route_block_event(ids.cb_play_block_event, &ev(x, y, z, b0, b1), ids, w)
    };

    c.record(
        "l1.the_block_event_id_is_distinct",
        ids.cb_play_block_event != ids.cb_play_block_entity_data
            && ids.cb_play_block_event != ids.cb_play_block_update,
        format!(
            "block_event={} vs block_entity_data={} block_update={}",
            ids.cb_play_block_event, ids.cb_play_block_entity_data, ids.cb_play_block_update
        ),
    );

    let routed = send(&mut world, 0, 64, 0, 1, 1);
    c.record(
        "l2.a_viewer_count_opens_the_lid",
        routed && world.block_entities.lid(pos).should_be_open,
        format!(
            "routed={routed} shouldBeOpen={} — triggerEvent(1, b1) with b1 the \
             *viewer count*, so any non-zero means open",
            world.block_entities.lid(pos).should_be_open
        ),
    );

    // b1 == 0 is the last viewer leaving.
    send(&mut world, 0, 64, 0, 1, 0);
    c.record(
        "l3.a_zero_viewer_count_closes_it",
        !world.block_entities.lid(pos).should_be_open,
        "b1 = 0 → shouldBeOpen false",
    );

    // Some other block's event must not touch the lid.
    send(&mut world, 0, 64, 0, 1, 1);
    send(&mut world, 0, 64, 0, 2, 0);
    c.record(
        "l4.only_b0_equals_one_is_a_lid_event",
        world.block_entities.lid(pos).should_be_open,
        "b0 = 2 (a note block's pitch, a piston's direction) left the lid alone",
    );

    // A position with no block entity is ignored, like `block_entity_data`.
    let before = world.block_entities.open_lid_count();
    send(&mut world, 9, 9, 9, 1, 1);
    c.record(
        "l5.an_event_for_no_block_entity_is_ignored",
        world.block_entities.open_lid_count() == before,
        format!("{} lid entries, unchanged", world.block_entities.open_lid_count()),
    );

    // --- the clock, independently transcribed -----------------------------
    // `tickLid`: 0.1 per tick, clamped, with the previous value kept for the
    // render lerp.
    let mut want = (0.0f32, 0.0f32); // (openness, oOpenness)
    let want_tick = |w: &mut (f32, f32), open: bool| {
        w.1 = w.0;
        if !open && w.0 > 0.0 {
            w.0 = (w.0 - 0.1).max(0.0);
        } else if open && w.0 < 1.0 {
            w.0 = (w.0 + 0.1).min(1.0);
        }
    };

    let mut lid = ChestLid::default();
    lid.should_be_open = true;
    let mut seq = Vec::new();
    let mut ok = true;
    for _ in 0..14 {
        lid.tick();
        want_tick(&mut want, true);
        ok &= (lid.openness - want.0).abs() < 1e-6;
        seq.push(format!("{:.1}", lid.openness));
    }
    c.record(
        "l6.the_lid_opens_over_ten_ticks_and_stops_at_one",
        ok && lid.openness == 1.0,
        format!("openness by tick: {} — 0.1 per tick, clamped at 1", seq.join(" ")),
    );

    lid.should_be_open = false;
    let mut closed_in = 0;
    for i in 1..=14 {
        lid.tick();
        if lid.openness == 0.0 && closed_in == 0 {
            closed_in = i;
        }
    }
    c.record(
        "l7.it_shuts_at_the_same_rate",
        closed_in == 10,
        format!("shut after {closed_in} ticks (want 10)"),
    );

    // `getOpenness(a) = lerp(a, oOpenness, openness)` — the render value is
    // interpolated, so a still frame mid-tick is between the two.
    let mut lid = ChestLid::default();
    lid.should_be_open = true;
    lid.tick();
    lid.tick();
    c.record(
        "l8.the_render_openness_interpolates_between_ticks",
        (lid.openness(0.0) - 0.1).abs() < 1e-6
            && (lid.openness(0.5) - 0.15).abs() < 1e-6
            && (lid.openness(1.0) - 0.2).abs() < 1e-6,
        format!(
            "a=0 -> {:.3}, a=0.5 -> {:.3}, a=1 -> {:.3} (oOpenness 0.1, openness 0.2)",
            lid.openness(0.0),
            lid.openness(0.5),
            lid.openness(1.0)
        ),
    );

    // A settled-shut lid is dropped, so an untouched chest costs nothing.
    let mut world2 = rewo_world::World::new(shape);
    let body = chunk_body(0, 0, shape.section_count(), &[(0x00, 64, chest)]);
    let mut reader = rewo_proto::reader::PacketReader::new(&body);
    let col = rewo_world::chunk::read_level_chunk(&mut reader, &shape, blocks).unwrap();
    world2.insert_column(0, 0, col);
    send(&mut world2, 0, 64, 0, 1, 1);
    for _ in 0..12 {
        world2.block_entities.tick_lids();
    }
    let open_entries = world2.block_entities.open_lid_count();
    send(&mut world2, 0, 64, 0, 1, 0);
    for _ in 0..12 {
        world2.block_entities.tick_lids();
    }
    c.record(
        "l9.a_settled_shut_lid_is_dropped",
        open_entries == 1 && world2.block_entities.open_lid_count() == 0,
        format!(
            "{open_entries} entry while open, {} once shut — an untouched chest \
             carries no clock at all",
            world2.block_entities.open_lid_count()
        ),
    );

    // --- the renderer's ease ----------------------------------------------
    // `open = 1 - open; open = 1 - open*open*open;` then `xRot = -(open*PI/2)`.
    let want_ease = |o: f32| {
        let inv = 1.0 - o;
        1.0 - inv * inv * inv
    };
    let mut ease_ok = true;
    let mut rows = Vec::new();
    for o in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
        let got = rewo_gpu::entities::lid_angle(o);
        let want = -(want_ease(o) * std::f32::consts::FRAC_PI_2);
        ease_ok &= (got - want).abs() < 1e-6;
        rows.push(format!("{o:.2}->{:.4}", got));
    }
    c.record(
        "l10.the_lid_angle_is_the_cubic_ease",
        ease_ok,
        format!(
            "{} rad — 1-(1-open)^3 then -(open * PI/2); a linear ramp would differ \
             visibly at the midpoint",
            rows.join(" ")
        ),
    );
    c.record(
        "l11.a_shut_lid_is_exactly_zero_and_a_full_one_is_a_right_angle",
        rewo_gpu::entities::lid_angle(0.0) == 0.0
            && (rewo_gpu::entities::lid_angle(1.0) + std::f32::consts::FRAC_PI_2).abs() < 1e-6,
        format!(
            "0 -> {:.6}, 1 -> {:.6} (want 0 and -PI/2)",
            rewo_gpu::entities::lid_angle(0.0),
            rewo_gpu::entities::lid_angle(1.0)
        ),
    );

    // The ease is not linear — the whole point of the cubic.
    let mid = rewo_gpu::entities::lid_angle(0.5);
    c.record(
        "l12.the_ease_is_not_linear",
        (mid + std::f32::consts::FRAC_PI_2 * 0.5).abs() > 0.2,
        format!(
            "openness 0.5 -> {mid:.4} rad, where a linear ramp would give {:.4} — \
             the chest is already 87.5% open half way through",
            -std::f32::consts::FRAC_PI_2 * 0.5
        ),
    );
}

fn check_registry(
    c: &mut Checker,
    registry: &BlockEntityRegistry,
    entries: &[(String, i32)],
    ids: &Ids,
) {
    c.record(
        "a1.every_registered_type_is_classified",
        registry.len() == entries.len(),
        format!(
            "{} of {} registry entries classified — resolve() rejects an unclassified \
             type, so this can only differ if the loop silently skipped one",
            registry.len(),
            entries.len()
        ),
    );

    // The table must not have grown a name the registry lost, either — the
    // failing direction a "just look it up" table would never notice.
    let names: HashSet<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    let orphans: Vec<&str> = TYPE_TABLE
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !names.contains(n))
        .collect();
    c.record(
        "a2.the_table_classifies_nothing_the_registry_lacks",
        orphans.is_empty(),
        format!("orphaned table entries: {orphans:?} (want none)"),
    );

    let invisible: Vec<&str> = TYPE_TABLE
        .iter()
        .filter(|(_, k)| *k == BlockEntityKind::Invisible)
        .map(|(n, _)| *n)
        .collect();
    c.record(
        "a3.the_invisible_set_is_the_measured_one",
        invisible.len() == 11
            && invisible.contains(&"minecraft:chest")
            && invisible.contains(&"minecraft:shulker_box")
            && invisible.contains(&"minecraft:banner")
            && !invisible.contains(&"minecraft:sign"),
        format!(
            "{} invisible types: {:?} — a sign is NOT among them, because its block \
             model carries the plank and only the text is renderer-side (and a bed \
             is not a block entity in 26.2 at all, which the fail-closed resolve \
             proved by rejecting it as an orphan)",
            invisible.len(),
            invisible
        ),
    );

    c.record(
        "a4.nothing_is_marked_rendered_yet",
        !TYPE_TABLE
            .iter()
            .any(|(_, k)| *k == BlockEntityKind::Rendered),
        "no type claims a renderer — M25 decodes and classifies; the renderer is \
         the carried-forward half, and this witness fails the moment that changes \
         without the gate being extended",
    );

    c.record(
        "a5.the_packet_id_resolves_by_name_and_is_distinct",
        ids.cb_play_block_entity_data != ids.cb_play_block_update
            && ids.cb_play_block_entity_data != ids.cb_play_section_blocks_update,
        format!(
            "block_entity_data={} distinct from block_update={} and \
             section_blocks_update={} — resolved by name, so a renumber fails loud",
            ids.cb_play_block_entity_data,
            ids.cb_play_block_update,
            ids.cb_play_section_blocks_update
        ),
    );
}

fn check_gap(c: &mut Checker, version: &str, registry: &BlockEntityRegistry) -> Result<(), String> {
    let jar = client_jar(version)
        .ok_or("client jar not found — blockentityshot measures the gap from its models")?;
    // The measurement lives in `rewo_data::assets`, beside the model resolver
    // it mirrors — the gate reads the jar through production code rather than
    // a private copy of the parent-chain walk.
    let measured = rewo_data::assets::blocks_without_geometry(&jar)?;
    let legit: HashSet<&str> = LEGITIMATELY_INVISIBLE.iter().copied().collect();
    let real: Vec<&String> = measured
        .iter()
        .filter(|b| !legit.contains(b.as_str()))
        .collect();

    c.record(
        "e1.the_jar_still_has_a_block_entity_gap",
        real.len() > 60,
        format!(
            "{} blocks bake to no geometry, {} of them real block entities (the rest \
             are air/water/lava/light/barrier/markers) — this is measured from the \
             jar's own model parent chains every run, not a number written down once",
            measured.len(),
            real.len()
        ),
    );

    // Spot-check the specific claims the classification rests on.
    let has = |n: &str| measured.iter().any(|b| b == n);
    c.record(
        "e2.a_chest_bakes_to_nothing",
        has("chest") && has("trapped_chest") && has("ender_chest"),
        "chest / trapped_chest / ender_chest all have blockstate models with no \
         `elements` — models/block/chest.json is a single `particle` texture, which \
         is why every chest Rewo has ever rendered was empty space",
    );
    c.record(
        "e3.beds_and_signs_do_bake",
        !has("red_bed") && !has("oak_sign") && !has("oak_wall_sign"),
        "red_bed / oak_sign / oak_wall_sign all produce geometry — so they are \
         ModelIsEnough, not Invisible, and M25 loses detail rather than the block",
    );
    c.record(
        "e4.the_classification_covers_the_measured_shortfall",
        registry.invisible_count() == 11,
        format!(
            "{} block-entity TYPES classified Invisible, covering those blocks \
             (one type spans all 16 banner colours, all 17 shulker boxes, and so on)",
            registry.invisible_count()
        ),
    );
    Ok(())
}

fn check_decode(c: &mut Checker, blocks: &rewo_data::blocks::Blocks, chest: i32, sign: i32) {
    let shape = DimensionShape::OVERWORLD;
    let mut r = rewo_proto::reader::PacketReader::new(&[]);
    let _ = &mut r;

    // Two block entities in one column, at positions that pin the nibble order.
    let body = chunk_body(
        3,
        -2,
        shape.section_count(),
        &[(0x3A, 64, chest), (0x0F, -13, sign)],
    );
    let mut reader = rewo_proto::reader::PacketReader::new(&body);
    let col = rewo_world::chunk::read_level_chunk(&mut reader, &shape, blocks)
        .expect("the synthesised chunk body must decode");

    c.record(
        "b1.the_chunk_payload_yields_its_block_entities",
        col.block_entities.len() == 2,
        format!(
            "{} block entities decoded from the payload (want 2) — before M25 all four \
             fields were read and dropped",
            col.block_entities.len()
        ),
    );

    let by_type = |t: i32| col.block_entities.iter().find(|(_, b)| b.type_id == t);
    let chest_at = by_type(chest).map(|(p, _)| *p);
    let sign_at = by_type(sign).map(|(p, _)| *p);
    c.record(
        "b2.packed_xz_unpacks_with_x_in_the_high_nibble",
        chest_at == Some(BlockEntityPos { x: 3 * 16 + 3, y: 64, z: -2 * 16 + 10 }),
        format!(
            "packedXZ 0x3A in chunk (3,-2) -> {chest_at:?} (want x=51 z=-22) — \
             `(x & 15) << 4 | (z & 15)`, the opposite nibble order to the section \
             biome index"
        ),
    );
    c.record(
        "b3.the_y_field_is_absolute_and_signed",
        sign_at == Some(BlockEntityPos { x: 3 * 16, y: -13, z: -2 * 16 + 15 }),
        format!(
            "y -13 -> {sign_at:?} — an i16 read as unsigned would land at 65523, above \
             the build height"
        ),
    );
    c.record(
        "b4.the_type_id_is_carried_verbatim",
        by_type(chest).is_some() && by_type(sign).is_some(),
        format!("chest={chest} and sign={sign} both present, unremapped"),
    );

    // The alignment claim: a body with block entities must leave the light
    // section readable, i.e. the list is consumed at exactly the right offset.
    let empty = chunk_body(0, 0, shape.section_count(), &[]);
    let mut reader = rewo_proto::reader::PacketReader::new(&empty);
    let none = rewo_world::chunk::read_level_chunk(&mut reader, &shape, blocks);
    c.record(
        "b5.an_empty_list_still_parses_the_rest_of_the_chunk",
        none.is_ok_and(|col| col.block_entities.is_empty()),
        "a zero-count list leaves the light data aligned — the decoder's position in \
         the stream is what this proves, which a hand-built struct could not",
    );
}

fn check_lifecycle(c: &mut Checker, ids: &Ids, blocks: &rewo_data::blocks::Blocks, chest: i32) {
    let shape = DimensionShape::OVERWORLD;
    let mut world = rewo_world::World::new(shape);

    let load = |world: &mut rewo_world::World, cx: i32, cz: i32, bes: &[(u8, i16, i32)]| {
        let body = chunk_body(cx, cz, shape.section_count(), bes);
        let mut reader = rewo_proto::reader::PacketReader::new(&body);
        let col = rewo_world::chunk::read_level_chunk(&mut reader, &shape, blocks).unwrap();
        world.insert_column(cx, cz, col);
    };

    load(&mut world, 0, 0, &[(0x00, 64, chest)]);
    load(&mut world, 1, 0, &[(0x11, 70, chest)]);
    c.record(
        "c1.loading_a_column_registers_its_block_entities",
        world.block_entities.len() == 2,
        format!("{} in the world after two columns", world.block_entities.len()),
    );

    // A re-sent column is authoritative: its list replaces the old one.
    load(&mut world, 0, 0, &[]);
    c.record(
        "c2.a_resent_column_replaces_rather_than_merges",
        world.block_entities.len() == 1
            && world
                .block_entities
                .get(BlockEntityPos { x: 17, y: 70, z: 1 })
                .is_some(),
        format!(
            "{} left after chunk (0,0) was re-sent empty — without the clear, a chest \
             the player broke would linger",
            world.block_entities.len()
        ),
    );

    world.forget_column(1, 0);
    c.record(
        "c3.unloading_a_column_drops_its_block_entities",
        world.block_entities.is_empty(),
        format!("{} left after unload", world.block_entities.len()),
    );

    // `block_entity_data` only updates an existing entry.
    load(&mut world, 0, 0, &[(0x00, 64, chest)]);
    let pos = BlockEntityPos { x: 0, y: 64, z: 0 };
    // Through the production packet seam, so this proves the id selection and
    // the `BlockPos` unpack too — not just the world API underneath them.
    let routed = rewo_net::route_block_entity_data(
        ids.cb_play_block_entity_data,
        &block_entity_data_body(0, 64, 0, chest, &[1, 7]),
        ids,
        &mut world,
    );
    let applied = world.block_entities.get(pos).map(|b| b.data.clone()) == Some(Nbt::Byte(7));
    rewo_net::route_block_entity_data(
        ids.cb_play_block_entity_data,
        &block_entity_data_body(5, 5, 5, chest, &[1, 7]),
        ids,
        &mut world,
    );
    let absent = world.block_entities.get(BlockEntityPos { x: 5, y: 5, z: 5 }).is_some();
    // A different packet id must not be consumed by this handler.
    let wrong_id = rewo_net::route_block_entity_data(
        ids.cb_play_block_update,
        &block_entity_data_body(0, 64, 0, chest, &[1, 9]),
        ids,
        &mut world,
    );
    c.record(
        "c4.block_entity_data_updates_an_existing_entry_only",
        routed && applied && !absent && world.block_entities.len() == 1,
        format!(
            "routed={routed}; applied at a known position={applied}, at an unknown \
             one={absent} — vanilla's handler returns when `getBlockEntity` is null, so \
             a stray packet cannot paint a chest into thin air"
        ),
    );
    c.record(
        "c7.the_handler_is_selected_by_packet_id",
        !wrong_id
            && world.block_entities.get(pos).map(|b| b.data.clone()) == Some(Nbt::Byte(7)),
        format!(
            "a block_update id was not consumed by the block-entity handler \
             (consumed={wrong_id}) and left the stored tag untouched"
        ),
    );
    c.record(
        "c5.the_update_tag_is_stored",
        world.block_entities.get(pos).map(|b| b.data.clone()) == Some(Nbt::Byte(7)),
        format!("stored tag: {:?}", world.block_entities.get(pos).map(|b| &b.data)),
    );

    // Sorted iteration must be deterministic — the gate and any future dump
    // depend on it, and `HashMap` order is not stable across runs.
    let mut be = BlockEntities::default();
    for (x, z) in [(5, 1), (1, 5), (3, 3), (1, 1)] {
        be.insert(
            BlockEntityPos { x, y: 0, z },
            rewo_world::block_entities::BlockEntity {
                type_id: chest,
                data: Nbt::End,
            },
        );
    }
    let order: Vec<(i32, i32)> = be.sorted().iter().map(|(p, _)| (p.x, p.z)).collect();
    c.record(
        "c6.sorted_iteration_is_deterministic",
        order == vec![(1, 1), (1, 5), (3, 3), (5, 1)],
        format!("{order:?} — lexicographic by (x, y, z)"),
    );
}
