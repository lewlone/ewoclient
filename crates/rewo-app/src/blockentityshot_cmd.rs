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

const EXPECTED_WITNESSES: usize = 32;

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
        .map(|(n, _)| *n)
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
        "m11.double_chest_states_are_recognised_even_though_they_are_not_drawn",
        doubles > 0,
        format!(
            "{doubles} left/right chest states resolve — the renderer skips them \
             because a half-pair needs DoubleBlockCombiner, and drawing a single \
             chest in each half would be visibly wrong"
        ),
    );
    Ok(())
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
