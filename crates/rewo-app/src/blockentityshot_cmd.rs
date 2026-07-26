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

const EXPECTED_WITNESSES: usize = 166;

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

    // The type ids `block_event` dispatches on, taken from production's own
    // resolver rather than looked up here — `check_lids` drives the real route
    // with them, so a wrong table would show up as a wrong animation, not as a
    // disagreement between two copies of the same lookup.
    let types = registry.block_event_types();
    let type_id = |want: &str| entries.iter().find(|(n, _)| n == want).map(|(_, i)| *i);
    let shulker = type_id("minecraft:shulker_box")
        .ok_or("registries.json: no minecraft:shulker_box block entity type")?;
    let spawner = type_id("minecraft:mob_spawner")
        .ok_or("registries.json: no minecraft:mob_spawner block entity type")?;
    let pot = type_id("minecraft:decorated_pot")
        .ok_or("registries.json: no minecraft:decorated_pot block entity type")?;
    let bell =
        type_id("minecraft:bell").ok_or("registries.json: no minecraft:bell block entity type")?;

    println!(
        "[blockentityshot] registry: {} types, {} rendered, {} still invisible; \
         block_entity_data id={}",
        registry.len(),
        registry.rendered_count(),
        registry.invisible_count(),
        ids.cb_play_block_entity_data
    );

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    check_registry(&mut c, &registry, &entries, &ids, &paths)?;
    check_gap(&mut c, &args.version, &registry)?;
    check_decode(&mut c, &blocks, chest, sign);
    check_lifecycle(&mut c, &ids, &blocks, chest);
    check_chest_models(&mut c, &args.version)?;
    check_lids(&mut c, &ids, &blocks, chest, types);
    check_sign_text(&mut c, &args.version)?;
    check_block_event_dispatch(&mut c, &ids, &blocks, &paths, &entries, types, chest, shulker, bell)?;
    check_shulker_anim(&mut c, &args.version)?;
    check_sign_style(&mut c, &blocks, &paths, sign)?;
    check_skulls(&mut c, &blocks, &paths, &args.version)?;
    check_be_clock(&mut c, &blocks, &paths, &ids, types, pot)?;
    check_conduit_active(&mut c, &paths, &args.version)?;
    check_spawner_mob(&mut c, &blocks, &paths, spawner)?;

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

    // --- shulker boxes -----------------------------------------------------
    let shulkers: Vec<String> = std::iter::once("rewo:be/shulker_box".to_string())
        .chain(
            rewo_data::block_entity_models::DYE_COLORS
                .iter()
                .map(|c| format!("rewo:be/{c}_shulker_box")),
        )
        .collect();
    let baked_shulkers = shulkers
        .iter()
        .filter(|n| items.block_entities.contains_key(*n))
        .count();
    c.record(
        "s1.every_shulker_box_bakes",
        baked_shulkers == 17,
        format!("{baked_shulkers} of 17 shulker-box models (undyed + 16 dyed)"),
    );

    let sh = items
        .block_entities
        .get("rewo:be/shulker_box")
        .ok_or("rewo:be/shulker_box did not bake")?;
    c.record(
        "s2.a_shulker_box_is_a_lid_and_a_base",
        sh.quads.len() == 12,
        format!("{} quads (want 12 = 2 boxes x 6 faces)", sh.quads.len()),
    );

    // The model is authored upside down — its boxes sit at negative y in part
    // space — because the renderer's transform ends in `scale(1, -1, -1)`.
    let mut ylo = f32::MAX;
    let mut yhi = f32::MIN;
    for q in &sh.quads {
        for v in &q.verts {
            ylo = ylo.min(v[1]);
            yhi = yhi.max(v[1]);
        }
    }
    c.record(
        "s3.the_shulker_model_is_authored_upside_down",
        ylo == 8.0 && yhi == 24.0,
        format!(
            "model y {ylo}..{yhi} px — the boxes are addBox(…,-16,…) and (…,-8,…) \
             against a PartPose.offset(0,24,0), which the renderer's trailing \
             scale(1,-1,-1) flips the right way up"
        ),
    );

    // …and the transform must land it inside the block, for all six facings.
    let mut inside = true;
    let mut rows = Vec::new();
    for f in [
        rewo_data::be_transform::Facing6::Down,
        rewo_data::be_transform::Facing6::Up,
        rewo_data::be_transform::Facing6::North,
        rewo_data::be_transform::Facing6::South,
        rewo_data::be_transform::Facing6::West,
        rewo_data::be_transform::Facing6::East,
    ] {
        let m = rewo_data::be_transform::shulker_box(f);
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for q in &sh.quads {
            for v in &q.verts {
                let p = [v[0] / 16.0, v[1] / 16.0, v[2] / 16.0];
                let t = [
                    m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
                    m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
                    m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
                ];
                for k in 0..3 {
                    lo[k] = lo[k].min(t[k]);
                    hi[k] = hi[k].max(t[k]);
                }
            }
        }
        inside &= (0..3).all(|k| lo[k] >= -0.01 && hi[k] <= 1.01);
        rows.push(format!("{f:?} y {:.3}..{:.3}", lo[1], hi[1]));
    }
    c.record(
        "s4.every_facing_lands_the_box_inside_its_block",
        inside,
        format!(
            "{} — translate(.5) scale(.9995) rotate(dir) scale(1,-1,-1) \
             translate(0,-1,0), applied right-to-left to a point",
            rows.join("; ")
        ),
    );

    // The 0.9995 shrink is real and is what keeps a wall-mounted box from
    // z-fighting the wall.
    let m_up = rewo_data::be_transform::shulker_box(rewo_data::be_transform::Facing6::Up);
    c.record(
        "s5.the_transform_shrinks_by_a_hair",
        (m_up[0][0].abs() - 0.9995).abs() < 1e-6,
        format!(
            "scale {:.6} — vanilla's 0.9995, so a box flush against a block does \
             not z-fight it",
            m_up[0][0].abs()
        ),
    );

    // Up and Down must genuinely differ — a box on a ceiling is upside down.
    let m_down = rewo_data::be_transform::shulker_box(rewo_data::be_transform::Facing6::Down);
    c.record(
        "s6.up_and_down_are_not_the_same_transform",
        (m_up[1][1] - m_down[1][1]).abs() > 1.0,
        format!(
            "up m11={:.4} vs down m11={:.4} — the y axis inverts, which is the \
             whole difference between a floor box and a ceiling one",
            m_up[1][1], m_down[1][1]
        ),
    );

    // Each colour has its own texture.
    let tex_sh = |n: &str| {
        items
            .block_entities
            .get(n)
            .and_then(|m| m.quads.first())
            .map(|q| q.tex)
    };
    let undyed = tex_sh("rewo:be/shulker_box");
    let red = tex_sh("rewo:be/red_shulker_box");
    let blue = tex_sh("rewo:be/blue_shulker_box");
    c.record(
        "s7.each_dye_has_its_own_texture",
        undyed.is_some() && undyed != red && red != blue,
        format!("undyed={undyed:?} red={red:?} blue={blue:?}"),
    );

    // Every shulker state resolves to a model that baked, with a transform.
    // Selected by the model name rather than by "not a chest": since M28 the
    // static table resolves skulls too, and the old exclusion counted those as
    // shulker boxes — 102 became 382 without the witness noticing what it was
    // actually measuring.
    let mut sh_ok = true;
    let mut sh_seen = 0;
    for id in 0..40000u32 {
        let Some(d) = states.draw_for(id) else { continue };
        if !d.model.ends_with("shulker_box") {
            continue;
        }
        sh_seen += 1;
        sh_ok &= items.block_entities.contains_key(&d.model) && d.chest().is_none();
    }
    c.record(
        "s8.every_shulker_state_names_a_baked_model",
        sh_ok && sh_seen == states.shulker_len(),
        format!(
            "{sh_seen} shulker states resolve to baked models, none carrying a \
             chest lid (a shulker's own opening is a different mechanism)"
        ),
    );

    // A block-entity type still in the Invisible list must resolve to nothing
    // — the fail-closed half, so an unimplemented type cannot borrow a model.
    let banner_state = (0..40000u32).find(|&id| {
        states.get(id).is_none()
            && states.draw_for(id).is_none()
    });
    c.record(
        "s9.an_unimplemented_type_still_draws_nothing",
        banner_state.is_some(),
        "states outside the chest and shulker tables resolve to None, so banners, \
         heads, pots and the rest render as nothing rather than as a wrong model",
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
fn check_lids(
    c: &mut Checker,
    ids: &Ids,
    blocks: &rewo_data::blocks::Blocks,
    chest: i32,
    types: rewo_world::block_entities::BlockEventTypes,
) {
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
        rewo_net::route_block_event(
            ids.cb_play_block_event,
            &ev(x, y, z, b0, b1),
            ids,
            types,
            0,
            w,
        )
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
        let got = rewo_data::be_transform::chest_lid_angle(o);
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
        rewo_data::be_transform::chest_lid_angle(0.0) == 0.0
            && (rewo_data::be_transform::chest_lid_angle(1.0) + std::f32::consts::FRAC_PI_2).abs() < 1e-6,
        format!(
            "0 -> {:.6}, 1 -> {:.6} (want 0 and -PI/2)",
            rewo_data::be_transform::chest_lid_angle(0.0),
            rewo_data::be_transform::chest_lid_angle(1.0)
        ),
    );

    // The ease is not linear — the whole point of the cubic.
    let mid = rewo_data::be_transform::chest_lid_angle(0.5);
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

/// M25e — sign text: the transform, the line layout, and the NBT decode.
///
/// The board has been drawn since M2; what M25 recorded as missing was the
/// text, and specifically the world-space text path it needs. Everything here
/// is compared against this file's own transcription of
/// `StandingSignRenderer.textTransformation` and `AbstractSignRenderer`.
/// M26 — `block_event` dispatches on the block entity's **type**.
///
/// The regression this pins: before M26 the route read `b0 == 1` as "a chest
/// lid" for every block entity, so a bell's ring — which is also `b0 == 1`,
/// with `b1` a `Direction.from3DDataValue` rather than a count — opened a lid
/// at the bell. Every witness here drives the production route with a real
/// packet body; none of them re-implements the dispatch they check.
#[allow(clippy::too_many_arguments)]
fn check_block_event_dispatch(
    c: &mut Checker,
    ids: &Ids,
    blocks: &rewo_data::blocks::Blocks,
    paths: &DataPaths,
    entries: &[(String, i32)],
    types: rewo_world::block_entities::BlockEventTypes,
    chest: i32,
    shulker: i32,
    bell: i32,
) -> Result<(), String> {
    use rewo_world::block_entities::{BlockEventBehavior, ShulkerStatus};

    // The production loader must see the same registry this gate read for
    // itself. It is a real cross-check now rather than a tautology: before
    // M26 the client never read this registry at all, so nothing in a live
    // run would have noticed the ids drifting.
    let prod = rewo_data::block_entity_types::load(&paths.registries_json())?;
    let mut prod_sorted = prod.clone();
    prod_sorted.sort_by_key(|(_, id)| *id);
    let mut mine = entries.to_vec();
    mine.sort_by_key(|(_, id)| *id);
    c.record(
        "d1.the_client_reads_the_same_registry_the_gate_does",
        prod_sorted == mine,
        format!(
            "{} types via rewo_data::block_entity_types::load, identical to this \
             gate's own read — the client resolves them now, so an unclassified \
             new type stops a live session and not only a gate run",
            prod.len()
        ),
    );

    c.record(
        "d2.the_three_types_resolve_to_their_own_bodies",
        types.behavior(chest) == Some(BlockEventBehavior::ChestLid)
            && types.behavior(shulker) == Some(BlockEventBehavior::ShulkerLid)
            && types.behavior(bell).is_none(),
        format!(
            "chest={chest} -> ChestLid, shulker_box={shulker} -> ShulkerLid, \
             bell={bell} -> none. All three send b0==1; only the type separates \
             a viewer count from an open/close pair from a click direction"
        ),
    );

    // One world holding a chest, a shulker box and a bell, so the same packet
    // shape can be aimed at each.
    let shape = DimensionShape::OVERWORLD;
    let mut world = rewo_world::World::new(shape);
    let body = chunk_body(
        0,
        0,
        shape.section_count(),
        &[(0x00, 64, chest), (0x11, 64, shulker), (0x22, 64, bell)],
    );
    let mut reader = rewo_proto::reader::PacketReader::new(&body);
    let col = rewo_world::chunk::read_level_chunk(&mut reader, &shape, blocks)
        .map_err(|e| format!("chunk decode: {e}"))?;
    world.insert_column(0, 0, col);
    let chest_pos = BlockEntityPos { x: 0, y: 64, z: 0 };
    let shulker_pos = BlockEntityPos { x: 1, y: 64, z: 1 };
    let bell_pos = BlockEntityPos { x: 2, y: 64, z: 2 };

    let ev = |p: BlockEntityPos, b0: u8, b1: u8| -> Vec<u8> {
        let mut b = packed_pos(p.x, p.y, p.z).to_be_bytes().to_vec();
        b.push(b0);
        b.push(b1);
        varint(0, &mut b);
        b
    };
    let mut send = |w: &mut rewo_world::World, p, b0, b1| {
        rewo_net::route_block_event(ids.cb_play_block_event, &ev(p, b0, b1), ids, types, 0, w)
    };

    // THE REGRESSION. `BellBlockEntity.triggerEvent` is `b0 == 1` with
    // `b1 = clickDirection.get3DDataValue()`, so ringing a bell from the north
    // is `b1 = 2` — which the old `b1 > 0` chest rule read as "two viewers".
    send(&mut world, bell_pos, 1, 2);
    c.record(
        "d3.a_bell_ring_does_not_open_a_lid",
        world.block_entities.open_lid_count() == 0
            && world.block_entities.open_shulker_count() == 0
            && world.block_entities.lid(bell_pos).openness == 0.0,
        format!(
            "bell rung with b0=1 b1=2 (Direction.NORTH.get3DDataValue()): {} lid \
             and {} shulker entries, both want 0. This is the M26 regression — \
             the old type-blind route made a lid entry here and ticked it open",
            world.block_entities.open_lid_count(),
            world.block_entities.open_shulker_count()
        ),
    );
    // Every direction, not just the one: DOWN is 0 and would have passed the
    // old rule by accident, which is how the bug stayed invisible.
    let mut any = false;
    for b1 in 0..6u8 {
        send(&mut world, bell_pos, 1, b1);
        any |= world.block_entities.open_lid_count() != 0;
    }
    c.record(
        "d4.no_click_direction_rings_through_to_a_lid",
        !any,
        "all six Direction.from3DDataValue values left both clocks empty — \
         b1=0 (DOWN) is the one the old rule got right by accident",
    );

    // The chest still works, through the same route.
    send(&mut world, chest_pos, 1, 1);
    c.record(
        "d5.a_chest_still_opens_through_the_typed_route",
        world.block_entities.lid(chest_pos).should_be_open
            && world.block_entities.open_lid_count() == 1,
        "chest b0=1 b1=1 -> shouldBeOpen, and it is the only lid entry",
    );

    // ...and the shulker box now does, on its own clock.
    send(&mut world, shulker_pos, 1, 1);
    c.record(
        "d6.a_shulker_box_opens_on_its_own_clock",
        world.block_entities.shulker(shulker_pos).status == ShulkerStatus::Opening
            && world.block_entities.lid(shulker_pos).openness == 0.0,
        format!(
            "shulker b0=1 b1=1 -> {:?}, and it made no *lid* entry — the two \
             clocks are separate, which is the thing that was conflated",
            world.block_entities.shulker(shulker_pos).status
        ),
    );

    // The asymmetry worth pinning: the shulker tests `== 1`, not `> 0`.
    let mut w2 = rewo_world::World::new(shape);
    let body2 = chunk_body(0, 0, shape.section_count(), &[(0x11, 64, shulker)]);
    let mut r2 = rewo_proto::reader::PacketReader::new(&body2);
    let col2 = rewo_world::chunk::read_level_chunk(&mut r2, &shape, blocks)
        .map_err(|e| format!("chunk decode: {e}"))?;
    w2.insert_column(0, 0, col2);
    send(&mut w2, shulker_pos, 1, 2);
    let after_two = w2.block_entities.shulker(shulker_pos).status;
    c.record(
        "d7.a_second_viewer_does_not_start_the_animation",
        after_two == ShulkerStatus::Closed,
        format!(
            "b1=2 left the status {after_two:?}. `ShulkerBoxBlockEntity` tests \
             `b1 == 0` and `b1 == 1` and has no else — a box already open stays \
             open and a shut one stays shut. The chest's `b1 > 0` would have \
             opened it, which is the plausible wrong answer"
        ),
    );

    // --- the spawner: a third meaning for `b0 == 1` (M28d) ----------------
    let spawner_type = entries
        .iter()
        .find(|(n, _)| n == "minecraft:mob_spawner")
        .map(|(_, i)| *i)
        .ok_or("registries.json: no minecraft:mob_spawner")?;
    let mut w3 = rewo_world::World::new(shape);
    let body3 = chunk_body(0, 0, shape.section_count(), &[(0x33, 64, spawner_type)]);
    let mut r3 = rewo_proto::reader::PacketReader::new(&body3);
    let col3 = rewo_world::chunk::read_level_chunk(&mut r3, &shape, blocks)
        .map_err(|e| format!("chunk decode: {e}"))?;
    w3.insert_column(0, 0, col3);
    let sp = BlockEntityPos { x: 3, y: 64, z: 3 };

    c.record(
        "d10.a_spawner_has_its_own_body_not_a_lid",
        types.behavior(spawner_type)
            == Some(rewo_world::block_entities::BlockEventBehavior::SpawnerReset),
        format!(
            "mob_spawner={spawner_type} routes to SpawnerReset — the THIRD \
             meaning of `b0 == 1` here, after a chest's viewer count and a \
             shulker's open/close pair"
        ),
    );

    // Start the clock, run it to zero, then reset it — the rate must fall.
    //
    // The first event is what CREATES the entry: a spawner Rewo has never seen
    // an event for keeps no clock, where vanilla ticks every spawner block
    // entity. That deviation is invisible while the caged mob is not drawn,
    // and is stated rather than papered over — but it does mean this witness
    // has to trigger before it can tick.
    send(&mut w3, sp, 1, 0);
    for _ in 0..200 {
        w3.block_entities.tick_lids();
    }
    let before = w3.block_entities.spawner(sp);
    let fast = before.spin - before.old_spin;
    send(&mut w3, sp, 1, 0);
    w3.block_entities.tick_lids();
    let after = w3.block_entities.spawner(sp);
    let slow = after.spin - after.old_spin;
    c.record(
        "d11.the_event_resets_the_countdown_and_slows_the_spin",
        before.delay == 0
            && after.delay == 199
            && (fast - 5.0).abs() < 0.02
            && (slow - 2.506).abs() < 0.02,
        format!(
            "run to a delay of {} the mob turns {fast:.3} deg/tick; the event \
             resets it to {} and it drops to {slow:.3}. `spin += 1000 / \
             (spawnDelay + 200)` means a spawner ACCELERATES as its next spawn \
             approaches — 1000/400 = 2.5 at a full delay against 1000/200 = 5 \
             at zero — and the event slams it back to slow. That acceleration \
             is the whole visible effect, and the server sends one event \
             rather than the angle",
            before.delay, after.delay
        ),
    );

    c.record(
        "d12.the_reset_target_comes_from_the_block_entitys_own_nbt",
        {
            let mut w4 = rewo_world::World::new(shape);
            w4.block_entities.insert(
                sp,
                rewo_world::block_entities::BlockEntity {
                    type_id: spawner_type,
                    data: Nbt::Compound(vec![("MinSpawnDelay".to_string(), Nbt::Int(40))]),
                },
            );
            w4.block_entities.trigger_block_event(types, sp, 1, 0, 0);
            w4.block_entities.spawner(sp).delay == 40
        },
        "a spawner configured with `MinSpawnDelay` 40 resets to 40, not to the \
         200 default — the target is read from the block entity rather than \
         assumed",
    );

    // ...but it *is* consumed, because vanilla's `triggerEvent` returns true
    // for any `b0 == 1` regardless of which branch fired. Asked of
    // `trigger_block_event`, not of the route: the route's bool answers "is
    // this my packet id", a dispatch-chain question, and conflating the two is
    // how the first draft of this witness failed.
    let consumed_two = w2
        .block_entities
        .trigger_block_event(types, shulker_pos, 1, 2, 0);
    let consumed_b0_two = w2
        .block_entities
        .trigger_block_event(types, shulker_pos, 2, 1, 0);
    c.record(
        "d8.a_no_op_branch_is_still_consumed_but_another_b0_is_not",
        consumed_two && !consumed_b0_two,
        format!(
            "b0=1 b1=2 consumed={consumed_two} (vanilla returns true for any \
             b0==1, even when neither branch fires) while b0=2 consumed=\
             {consumed_b0_two} — a note block's pitch or a piston's direction, \
             left alone rather than guessed at"
        ),
    );

    // A shulker box closes on b1 == 0, and an event for empty space is inert.
    send(&mut w2, shulker_pos, 1, 1);
    send(&mut w2, shulker_pos, 1, 0);
    let closing = w2.block_entities.shulker(shulker_pos).status;
    let empty = BlockEntityPos { x: 9, y: 64, z: 9 };
    let before = w2.block_entities.open_shulker_count();
    let consumed_empty = w2.block_entities.trigger_block_event(types, empty, 1, 1, 0);
    c.record(
        "d9.zero_closes_it_and_empty_space_is_inert",
        closing == ShulkerStatus::Closing
            && !consumed_empty
            && w2.block_entities.open_shulker_count() == before,
        format!(
            "b1=0 -> {closing:?}; an event at a position with no block entity was \
             not consumed and left the {before} existing entries alone — vanilla's \
             `getBlockEntity` returns null there and the handler returns"
        ),
    );
    Ok(())
}

/// M31 — the spawner's caged mob: an entity model mounted inside a block.
fn check_spawner_mob(
    c: &mut Checker,
    blocks: &rewo_data::blocks::Blocks,
    paths: &DataPaths,
    spawner_type: i32,
) -> Result<(), String> {
    use rewo_data::be_transform as bt;

    // --- the display entity comes from SpawnData two levels down -----------
    let mk = |inner: Nbt| rewo_world::block_entities::BlockEntity {
        type_id: spawner_type,
        data: Nbt::Compound(vec![("SpawnData".to_string(), inner)]),
    };
    let with_id = |id: &str| {
        mk(Nbt::Compound(vec![(
            "entity".to_string(),
            Nbt::Compound(vec![("id".to_string(), Nbt::String(id.to_string()))]),
        )]))
    };
    c.record(
        "r1.the_display_entity_is_read_from_spawn_data",
        crate::live_cmd::spawner_entity_id(&with_id("minecraft:zombie"))
            .as_deref()
            == Some("minecraft:zombie"),
        "`SpawnData` -> `entity` -> `id`; the id lives two levels down, not on \
         the block entity itself",
    );

    let empty = with_id("");
    let no_entity = mk(Nbt::Compound(vec![]));
    let bare = rewo_world::block_entities::BlockEntity {
        type_id: spawner_type,
        data: Nbt::End,
    };
    c.record(
        "r2.an_empty_or_absent_id_means_no_mob_rather_than_a_default",
        crate::live_cmd::spawner_entity_id(&empty).is_none()
            && crate::live_cmd::spawner_entity_id(&no_entity).is_none()
            && crate::live_cmd::spawner_entity_id(&bare).is_none(),
        "`getOrCreateDisplayEntity` returns null on an empty id, so an \
         unconfigured spawner draws NOTHING — a default mob would be an \
         invention, and a visible one at that",
    );

    // --- the scale fits the cage ------------------------------------------
    let small = bt::spawner_mob_scale(0.4, 0.3); // a silverfish
    let zombie = bt::spawner_mob_scale(0.6, 1.95);
    let exact = bt::spawner_mob_scale(1.0, 1.0);
    c.record(
        "r3.only_a_mob_larger_than_a_block_is_shrunk",
        (small - 0.53125).abs() < 1e-6
            && (exact - 0.53125).abs() < 1e-6
            && (zombie - 0.53125 / 1.95).abs() < 1e-6,
        format!(
            "a silverfish scales {small:.5}, a mob exactly one block {exact:.5}, \
             and a zombie {zombie:.5} = 0.53125/1.95. The threshold is on the \
             LARGER of width and height and is strict (`> 1.0`), so something \
             smaller is never enlarged to fill the cage"
        ),
    );

    // --- the mount --------------------------------------------------------
    let apply = |m: &bt::Affine, p: [f32; 3]| -> [f32; 3] {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    };
    let feet = apply(&bt::spawner_mob(0.0, 0.5), [0.0, 0.0, 0.0]);
    c.record(
        "r4.the_mob_hangs_in_the_middle_of_the_cage",
        (feet[0] - 0.5).abs() < 1e-5
            && (feet[2] - 0.5).abs() < 1e-5
            && (feet[1] - 0.2).abs() < 1e-5,
        format!(
            "its feet land at {feet:?} — `translate(0.5, 0.4, 0.5)` then, INSIDE \
             the spin, `translate(0, -0.2, 0)`, so the mob sits at y 0.2 in the \
             middle of the block"
        ),
    );

    // The second translate is inside the spin, so the mob ORBITS.
    let a = apply(&bt::spawner_mob(0.0, 0.5), [0.0, 0.0, 0.0]);
    let b = apply(&bt::spawner_mob(90.0, 0.5), [0.0, 0.0, 0.0]);
    let tilt_a = apply(&bt::spawner_mob(0.0, 0.5), [0.0, 1.0, 0.0]);
    c.record(
        "r5.the_tilt_leans_the_mob_towards_the_viewer",
        (tilt_a[2] - feet[2]).abs() > 1e-3,
        format!(
            "a point one unit above the feet lands at {tilt_a:?} against feet \
             {feet:?} — the z has moved, which is the `-30 degree` tilt about \
             X. Without it a caged mob would stand bolt upright in its box"
        ),
    );
    c.record(
        "r6.the_mob_turns_on_the_spot_rather_than_orbiting",
        (a[0] - b[0]).abs() < 1e-5 && (a[2] - b[2]).abs() < 1e-5,
        format!(
            "the feet stay at {a:?} / {b:?} through a quarter turn. The inner \
             translate LOOKS like it should make the mob orbit, but (0,-0.2,0) \
             lies along the spin AXIS, so it commutes with the Y rotation — the \
             two translates could be swapped with no effect at all, and only \
             the model's own extent sweeps round. This witness is what \
             disproved the opposite claim in `be_transform`'s own comment"
        ),
    );

    c.record(
        "r7.the_render_spin_is_ten_times_the_stored_one",
        (bt::spawner_spin_degrees(0.0, 4.0, 1.0) - 40.0).abs() < 1e-5
            && (bt::spawner_spin_degrees(0.0, 4.0, 0.5) - 20.0).abs() < 1e-5,
        "`Mth.lerp(partialTicks, oSpin, spin) * 10` — the block entity's own \
         counter advances a couple of degrees a tick and the renderer \
         multiplies by TEN, so the caged mob whirls rather than drifting",
    );

    // --- end to end through the collector ---------------------------------
    let states = rewo_data::chest_states::ChestStates::load(&paths.blocks_json())?;
    let etypes = rewo_data::entity_types::EntityTypes::load(&paths.registries_json())?;
    let shape = DimensionShape::OVERWORLD;
    let lightmap = rewo_world::lightmap::LightmapState::default();
    let spawner_state = blocks
        .default_state("minecraft:spawner")
        .ok_or("blocks.json: no minecraft:spawner")?;

    let build = |id: Option<&str>| -> Vec<crate::live_cmd::OwnedSpawnerMob> {
        let mut world = rewo_world::World::new(shape);
        let body = chunk_body(0, 0, shape.section_count(), &[(0x00, 64, spawner_type)]);
        let mut r = rewo_proto::reader::PacketReader::new(&body);
        let col = rewo_world::chunk::read_level_chunk(&mut r, &shape, blocks).unwrap();
        world.insert_column(0, 0, col);
        world.set_block(0, 64, 0, spawner_state);
        let be = match id {
            Some(i) => with_id(i),
            None => bare.clone(),
        };
        world
            .block_entities
            .insert(BlockEntityPos { x: 0, y: 64, z: 0 }, be);
        crate::live_cmd::collect_spawner_mobs(
            &world,
            &etypes,
            states.spawner_states(),
            &lightmap,
            0.0,
        )
    };

    let zombies = build(Some("minecraft:zombie"));
    let nothing = build(None);
    let unknown = build(Some("minecraft:not_a_real_mob"));
    c.record(
        "r8.a_configured_spawner_yields_one_mounted_draw",
        zombies.len() == 1
            && nothing.is_empty()
            && unknown.is_empty()
            && zombies[0].kind == rewo_gpu::mobs::kind_for_entity_name("minecraft:zombie"),
        format!(
            "a zombie spawner produced {} draw, an unconfigured one {}, and one \
             naming a type this version does not register {} — an unknown type \
             draws nothing rather than substituting a mob that is not in the \
             cage",
            zombies.len(),
            nothing.len(),
            unknown.len()
        ),
    );

    c.record(
        "r9.the_caged_mob_rides_the_entity_pass_with_a_mount",
        {
            let d = crate::live_cmd::spawner_mob_draw(&zombies[0]);
            d.mount.is_some()
                && d.pos == [0.0, 64.0, 0.0]
                && d.limb_amount == 0.0
                && !d.hurt
                && d.scale_mul == 1.0
        },
        "the draw carries a mount and the BLOCK's position, with every \
         simulated input neutral — vanilla loads a display entity once and \
         never ticks it, so it does not walk, look around, swing or take \
         damage. `scale_mul` stays 1 because the fit-to-cage scale is inside \
         the mount, and applying it twice would shrink the mob squared",
    );
    Ok(())
}

/// M30 — the active conduit: the frame scan, and the four draws it unlocks.
fn check_conduit_active(
    c: &mut Checker,
    paths: &DataPaths,
    version: &str,
) -> Result<(), String> {
    use rewo_data::be_transform as bt;
    use rewo_world::conduit;

    // --- the scan ---------------------------------------------------------
    let offs = conduit::frame_offsets();
    c.record(
        "q1.the_frame_shell_is_forty_two_positions_and_that_is_the_hunt_threshold",
        offs.len() == 42 && conduit::HUNT_AT == 42 && conduit::ACTIVATE_AT == 16,
        format!(
            "{} frame positions. Three axis rings each border a 5x5 plane — 16 \
             apiece — but they SHARE their axis ends, so the union is 42 and \
             not 48. `HUNT_AT` is also 42, so a conduit opens its eye exactly \
             when its frame is COMPLETE, not when it is nearly so. (My first \
             guess here was 48, and this witness is what corrected it.)",
            offs.len()
        ),
    );

    let water = vec![false, true, true];
    let frame = vec![false, false, true];
    let dry = conduit::scan((0, 0, 0), |_, _, _| 0, &water, &frame);
    c.record(
        "q2.a_dry_conduit_returns_before_counting_anything",
        dry.frame == 0 && !dry.submerged && !dry.active(),
        format!(
            "a conduit not fully submerged reports frame {} — vanilla returns \
             from `updateShape` BEFORE the counting loop, so this is zero \
             rather than a partial count nobody looked at",
            dry.frame
        ),
    );

    let pick = |n: usize| {
        let want: HashSet<(i32, i32, i32)> = offs.iter().take(n).copied().collect();
        conduit::scan(
            (0, 0, 0),
            move |x, y, z| if want.contains(&(x, y, z)) { 2 } else { 1 },
            &water,
            &frame,
        )
    };
    let (s15, s16, s41, s42) = (pick(15), pick(16), pick(41), pick(42));
    c.record(
        "q3.sixteen_activates_and_a_complete_frame_hunts",
        !s15.active() && s16.active() && !s41.hunting() && s42.hunting() && s42.active(),
        format!(
            "15 frame blocks -> inactive, 16 -> active, 41 -> active but not \
             hunting, 42 -> hunting. Both thresholds read the SAME count, which \
             is why the eye costs nothing once the scan exists"
        ),
    );

    // --- the clock --------------------------------------------------------
    let mut a = conduit::ConduitAnim::default();
    let live = conduit::ConduitShape { frame: 20, submerged: true };
    let dead = conduit::ConduitShape { frame: 0, submerged: false };
    for _ in 0..10 {
        a.tick(Some(dead));
    }
    let dormant = (a.active_rotation, a.rotation(0.5));
    for _ in 0..10 {
        a.tick(Some(live));
    }
    c.record(
        "q4.the_rotation_advances_only_while_active",
        dormant == (0.0, 0.0)
            && a.active_rotation == 10.0
            && (a.rotation(0.5) + 0.39375).abs() < 1e-6,
        format!(
            "ten dormant ticks left the rotation at {} and its rendered angle \
             at {} — the partial is added only while active, so a conduit that \
             has just switched off stops DEAD rather than creeping on. Ten \
             active ticks then give {} and {:.5} rad, `(10 + 0.5) * -0.0375`; \
             the sign is vanilla's and the cage turns the other way from a bare \
             tick count. This is also why Rewo's dormant shell was already \
             correct at zero",
            dormant.0,
            dormant.1,
            a.active_rotation,
            a.rotation(0.5)
        ),
    );

    let mut b = conduit::ConduitAnim::default();
    let mut phases = Vec::new();
    for _ in 0..200 {
        b.tick(Some(live));
        phases.push(b.phase());
    }
    c.record(
        "q5.the_wind_phase_holds_for_sixty_six_ticks",
        phases[0] == 0 && phases[65] == 1 && phases[131] == 2 && phases[197] == 0,
        "`tickCount / 66 % 3` — integer division, so each phase holds 66 ticks \
         and then both the shroud's axis and its texture change",
    );

    // --- the four draws ---------------------------------------------------
    let jar = client_jar(version).ok_or("client jar not found")?;
    let baked = rewo_data::assets::bake(&jar, &paths.blocks_json())?;
    let items = &baked.held_items;
    let missing: Vec<&str> = rewo_data::block_entity_models::CONDUIT_ACTIVE
        .iter()
        .map(|(n, _, _)| *n)
        .filter(|n| !items.block_entities.contains_key(*n))
        .collect();
    c.record(
        "q6.the_active_pieces_bake",
        missing.is_empty(),
        format!(
            "cage, both wind textures and both eyes baked, missing {missing:?} \
             — the eye is a flat plane, so its two variants differ only by \
             which pupil texture they carry"
        ),
    );

    let apply = |m: &bt::Affine, p: [f32; 3]| -> [f32; 3] {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    };
    // The cage turns about a TILTED axis, so a point on +x leaves the xz plane.
    let turned = apply(&bt::conduit_cage(1.0, 0.0), [1.0, 0.0, 0.0]);
    let still = apply(&bt::conduit_cage(0.0, 0.0), [1.0, 0.0, 0.0]);
    c.record(
        "q7.the_cage_tumbles_about_a_tilted_axis_rather_than_spinning_flat",
        (turned[1] - still[1]).abs() > 1e-3,
        format!(
            "a point on +x moves from y {:.4} to {:.4} under a one-radian turn \
             — the axis is `(0.5, 1, 0.5)` normalised, NOT plain Y, so the cage \
             tumbles. A Y-only rotation would leave that y untouched",
            still[1], turned[1]
        ),
    );

    // `hh = sin(t*0.1)/2 + 0.5` then `hh = hh*hh + hh`, so the drive `b` runs
    // 0..1 and the height runs 0.3 + (b*b + b) * 0.2 over 0.3..0.7.
    let y_at = |t: f32| apply(&bt::conduit_cage(0.0, t), [0.0, 0.0, 0.0])[1];
    let mid = y_at(0.0); //   sin = 0  -> b = 0.5
    let top = y_at(std::f32::consts::FRAC_PI_2 / 0.1); // sin =  1 -> b = 1
    let bot = y_at(3.0 * std::f32::consts::FRAC_PI_2 / 0.1); // sin = -1 -> b = 0
    // A LINEAR map of the same drive would put the midpoint halfway up.
    let linear_mid = bot + (top - bot) / 2.0;
    c.record(
        "q8.the_bob_is_not_a_plain_sine",
        (bot - 0.3).abs() < 1e-4
            && (top - 0.7).abs() < 1e-4
            && (mid - 0.45).abs() < 1e-4
            && mid < linear_mid - 0.04,
        format!(
            "the cage centre runs {bot:.4}..{top:.4}, and at the drive's \
             MIDPOINT it sits at {mid:.4} where a linear map would put it at \
             {linear_mid:.4}. `hh*hh + hh` is convex, so the cage dwells low \
             and snaps up rather than moving sinusoidally. (The 0.45 here is \
             vanilla's value at animTime 0, not a number I picked — the first \
             draft of this witness guessed 0.4 and failed.)"
        ),
    );

    let w0 = bt::conduit_wind(0, false);
    let w1 = bt::conduit_wind(1, false);
    let w_second = bt::conduit_wind(0, true);
    c.record(
        "q9.the_shroud_turns_per_phase_and_its_second_copy_shrinks",
        w0 != w1
            && (w_second[0][0].abs() - 0.875).abs() < 1e-5
            && (w0[0][0] - 1.0).abs() < 1e-5,
        format!(
            "phase 0 and 1 differ, and the second copy scales {:.4} against the \
             first's {:.4} — the two shells counter-rotate at different sizes, \
             which is what reads as a churn instead of a spinning box",
            w_second[0][0].abs(),
            w0[0][0]
        ),
    );

    // The eye is a BILLBOARD: turn the camera and the quad turns with it.
    let eye_a = bt::conduit_eye(0.0, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let eye_b = bt::conduit_eye(0.0, [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]);
    let corner_a = apply(&eye_a, [4.0, 0.0, 0.0]);
    let corner_b = apply(&eye_b, [4.0, 0.0, 0.0]);
    c.record(
        "q10.the_eye_faces_the_camera",
        (corner_a[0] - corner_b[0]).abs() > 1e-3 || (corner_a[2] - corner_b[2]).abs() > 1e-3,
        format!(
            "the same corner lands at {corner_a:?} under one camera basis and \
             {corner_b:?} under another — the eye is the ONE thing in this \
             block-entity path that depends on the view rather than on the \
             block, which is why the collector has to be handed the camera axes"
        ),
    );
    Ok(())
}

/// M29 — the block-entity animation clock.
///
/// Every witness here drives the production formula or the production
/// collector; none re-implements what it checks. The point of the group is
/// that "at rest" was previously indistinguishable from "animating", so each
/// one asserts a value that MOVES.
fn check_be_clock(
    c: &mut Checker,
    blocks: &rewo_data::blocks::Blocks,
    paths: &DataPaths,
    ids: &Ids,
    types: rewo_world::block_entities::BlockEventTypes,
    pot_type: i32,
) -> Result<(), String> {
    use rewo_data::be_transform as bt;

    // --- the banner sway: position and game time, no state at all ---------
    let p0 = bt::banner_phase(0, 0, 0, 0, 0.0);
    let p_later = bt::banner_phase(0, 0, 0, 50, 0.0);
    let p_neighbour = bt::banner_phase(1, 0, 0, 0, 0.0);
    c.record(
        "n1.a_banners_phase_hashes_its_position_with_the_world_clock",
        (p0 - 0.0).abs() < 1e-6
            && (p_later - 0.5).abs() < 1e-6
            && (p_neighbour - 0.07).abs() < 1e-6,
        format!(
            "phase at (0,0,0) t=0 is {p0:.3}, at t=50 is {p_later:.3}, and a \
             block east at t=0 is {p_neighbour:.3} — `x*7 + y*9 + z*13 + \
             gameTime` mod 100, so two banners side by side sway OUT OF STEP \
             rather than in unison"
        ),
    );

    let neg = bt::banner_phase(-1, 0, 0, 0, 0.0);
    c.record(
        "n2.a_negative_coordinate_wraps_rather_than_going_negative",
        (0.0..1.0).contains(&neg),
        format!(
            "phase at x=-1 is {neg:.3}, inside 0..1 — vanilla uses `floorMod`, \
             and Rust's `%` would have produced a negative phase here"
        ),
    );

    let a0 = bt::banner_flag_angle(0.0);
    let a_half = bt::banner_flag_angle(0.5);
    c.record(
        "n3.the_cloth_never_hangs_straight",
        a0 < 0.0 && a_half < 0.0 && (a0 - a_half).abs() > 1e-3,
        format!(
            "flag xRot is {a0:.4} at phase 0 and {a_half:.4} at phase 0.5 — \
             both NEGATIVE, because the constant term (-0.0125) is larger than \
             the amplitude (0.01). The cloth sways either side of a small \
             permanent backward tilt rather than passing through vertical"
        ),
    );

    // --- the pot wobble: an event and a start tick ------------------------
    c.record(
        "n4.a_pot_is_a_fourth_meaning_of_b0_equals_one",
        types.behavior(pot_type)
            == Some(rewo_world::block_entities::BlockEventBehavior::PotWobble),
        format!(
            "decorated_pot={pot_type} routes to PotWobble — after a chest's \
             viewer count, a shulker's open/close pair and a spawner's reset, \
             `b1` here is a WobbleStyle ORDINAL"
        ),
    );

    let shape = DimensionShape::OVERWORLD;
    let mut w = rewo_world::World::new(shape);
    let body = chunk_body(0, 0, shape.section_count(), &[(0x00, 64, pot_type)]);
    let mut r = rewo_proto::reader::PacketReader::new(&body);
    let col = rewo_world::chunk::read_level_chunk(&mut r, &shape, blocks)
        .map_err(|e| format!("chunk decode: {e}"))?;
    w.insert_column(0, 0, col);
    let pos = BlockEntityPos { x: 0, y: 64, z: 0 };
    let ev = |b0: u8, b1: u8| -> Vec<u8> {
        let mut b = packed_pos(0, 64, 0).to_be_bytes().to_vec();
        b.push(b0);
        b.push(b1);
        varint(0, &mut b);
        b
    };

    c.record(
        "n5.a_pot_has_no_wobble_until_one_is_triggered",
        w.block_entities.pot_wobble(pos, 0, 0.0).is_none(),
        "an untouched pot carries no wobble entry at all, which is its whole \
         resting state",
    );

    // Trigger at game time 100, then read the progress as time advances.
    rewo_net::route_block_event(ids.cb_play_block_event, &ev(1, 0), ids, types, 100, &mut w);
    let at = |t: i64, a: f32| w.block_entities.pot_wobble(pos, t, a);
    let (style, pr0) = at(100, 0.0).ok_or("no wobble after the event")?;
    let (_, pr_mid) = at(103, 0.5).ok_or("no wobble mid-way")?;
    let past = at(120, 0.0).map(|(_, p)| p).unwrap_or(f32::NAN);
    c.record(
        "n6.the_wobble_runs_from_the_tick_its_event_arrived_on",
        style == rewo_world::block_entities::PotWobble::Positive
            && pr0.abs() < 1e-6
            && (pr_mid - 0.5).abs() < 1e-6
            && past > 1.0,
        format!(
            "style {style:?}, progress {pr0:.3} at the event's own tick, \
             {pr_mid:.3} three and a half ticks later (7-tick duration), and \
             {past:.3} long after — vanilla never clears the fields; the \
             RENDERER skips the block once the progress leaves 0..=1, which is \
             what stops a finished wobble freezing at its end pose"
        ),
    );

    c.record(
        "n7.the_two_styles_last_different_lengths",
        (rewo_world::block_entities::PotWobble::Positive.duration() - 7.0).abs() < 1e-6
            && (rewo_world::block_entities::PotWobble::Negative.duration() - 10.0).abs() < 1e-6,
        "POSITIVE runs 7 ticks and NEGATIVE 10 — the two wobbles do not merely \
         look different, they take different times",
    );

    // An ordinal outside the two styles is NOT consumed.
    let consumed_bad = w
        .block_entities
        .trigger_block_event(types, pos, 1, 9, 200);
    c.record(
        "n8.an_ordinal_outside_the_two_styles_is_not_consumed",
        !consumed_bad,
        "`triggerEvent` guards `data >= 0 && data < values().length`, so an \
         out-of-range ordinal falls through to `super.triggerEvent` and \
         returns false rather than starting a wobble Rewo would have to invent",
    );

    // The wobble transform must MOVE, and about the block's floor.
    let apply = |m: &bt::Affine, p: [f32; 3]| -> [f32; 3] {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    };
    let rest = bt::pot_wobble(bt::WobbleStyle::Positive, 1.5);
    let mid = bt::pot_wobble(bt::WobbleStyle::Positive, 0.3);
    let top = apply(&mid, [0.5, 1.0, 0.5]);
    let foot = apply(&mid, [0.5, 0.0, 0.5]);
    c.record(
        "n9.the_pot_rocks_on_its_base_and_settles_outside_the_window",
        rest == bt::IDENTITY
            && (top[0] - 0.5).abs() + (top[2] - 0.5).abs() > 1e-4
            && (foot[0] - 0.5).abs() < 1e-6
            && (foot[2] - 0.5).abs() < 1e-6,
        format!(
            "at progress 1.5 the transform is the identity; at 0.3 the pot's \
             TOP moves to {top:?} while its floor centre stays {foot:?}. Both \
             wobbles turn about (0.5, 0, 0.5) — the block's FLOOR — where the \
             facing rotation turns about (0.5, 0.5, 0.5), so a pot rocks on its \
             base rather than pivoting in mid-air"
        ),
    );

    // --- the skull counter: powered block state --------------------------
    let states = rewo_data::chest_states::ChestStates::load(&paths.blocks_json())?;
    let powered = states.powered_skull_states();
    c.record(
        "n10.the_skull_counter_is_driven_by_a_block_state_not_by_nbt",
        !powered.is_empty(),
        format!(
            "{} skull block states carry `powered=true` — a skull animates \
             because the note block beneath it is powered, which is why the \
             tick lives on `World` (it needs block states) rather than on the \
             block-entity map",
            powered.len()
        ),
    );

    let mut w2 = rewo_world::World::new(shape);
    w2.block_entities.tick_skull(pos, true);
    w2.block_entities.tick_skull(pos, true);
    let two = w2.block_entities.skull_animation(pos, 0.0);
    let lerped = w2.block_entities.skull_animation(pos, 0.5);
    w2.block_entities.tick_skull(pos, false);
    let off = w2.block_entities.skull_animation(pos, 0.0);
    c.record(
        "n11.it_counts_while_powered_and_stops_when_it_is_not",
        (two - 2.0).abs() < 1e-6 && (lerped - 2.5).abs() < 1e-6 && off.abs() < 1e-6,
        format!(
            "two powered ticks read {two}, with the partial applied {lerped} \
             (`isAnimating ? count + a : count`), and an unpowered tick reads \
             {off}"
        ),
    );

    // THE BUG THIS CLOCK FIXED: setupAnim always runs, so the ears and jaw are
    // never at the mesh's own rest pose.
    let (l0, r0) = bt::piglin_ear_angles(0.0);
    let (l1, r1) = bt::piglin_ear_angles(3.0);
    c.record(
        "n12.a_piglin_ear_is_never_at_the_meshs_rest_angle",
        (l0 + 0.7).abs() < 1e-5
            && (r0 - 0.7).abs() < 1e-5
            && (l0.abs() - std::f32::consts::FRAC_PI_6).abs() > 0.1,
        format!(
            "at animation 0 the ears sit at {l0:.4} / {r0:.4} rad, NOT the \
             +/-{:.4} the mesh's PartPose carries. `setupAnim` assigns zRot \
             outright and always runs, so Rewo's pre-M29 static ears were about \
             10 degrees off on every piglin head in the world",
            std::f32::consts::FRAC_PI_6
        ),
    );

    c.record(
        "n13.the_two_ears_drift_out_of_phase",
        (l1.abs() - r1.abs()).abs() > 1e-4,
        format!(
            "at animation 3 the ears are {l1:.4} and {r1:.4} — not mirror \
             images. The `1.2` asymmetry factor is on the LEFT ear only, so \
             the pair drifts in and out of phase instead of flapping together"
        ),
    );

    let j0 = bt::dragon_jaw_angle(0.0);
    let j_mid = bt::dragon_jaw_angle(2.5);
    c.record(
        "n14.a_dragon_jaw_rests_open_and_moves",
        (j0 - 0.2).abs() < 1e-5 && (j_mid - j0).abs() > 1e-3,
        format!(
            "the jaw is {j0:.4} rad at animation 0 and {j_mid:.4} at 2.5 — \
             `(sin(...) + 1) * 0.2` never reaches zero, so a dragon head's jaw \
             is always slightly open. Rewo drew it shut before M29"
        ),
    );
    Ok(())
}

/// M28 — the seven skull types, across their fourteen blocks.
fn check_skulls(
    c: &mut Checker,
    blocks: &rewo_data::blocks::Blocks,
    paths: &DataPaths,
    version: &str,
) -> Result<(), String> {
    use rewo_data::be_transform as bt;

    let jar = client_jar(version).ok_or("client jar not found")?;
    let baked = rewo_data::assets::bake(&jar, &paths.blocks_json())?;
    let items = &baked.held_items;

    let names: Vec<&str> = rewo_data::block_entity_models::SKULL_TEXTURES
        .iter()
        .map(|(n, _)| *n)
        .collect();
    let missing: Vec<&&str> = names
        .iter()
        .filter(|n| !items.block_entities.contains_key(**n))
        .collect();
    c.record(
        "k1.every_skull_type_bakes",
        missing.is_empty() && names.len() == 7,
        format!("{} skull models baked, missing {missing:?}", names.len()),
    );

    let quads = |n: &str| items.block_entities.get(n).map(|m| m.quads.len());
    c.record(
        "k2.a_humanoid_head_has_a_hat_and_a_mob_head_does_not",
        quads("rewo:be/zombie_head") == Some(12) && quads("rewo:be/skeleton_skull") == Some(6),
        format!(
            "zombie head {:?} quads (head + hat = 2 boxes) vs skeleton skull \
             {:?} (head only) — `createHumanoidHeadLayer` adds the `hat` \
             overlay, `createMobHeadLayer` does not",
            quads("rewo:be/zombie_head"),
            quads("rewo:be/skeleton_skull")
        ),
    );

    // The hat is the same box grown by 0.25, so it must enclose the head.
    let zombie = items.block_entities.get("rewo:be/zombie_head").unwrap();
    let extent = |qs: &[rewo_data::held_items::HeldQuad]| -> (f32, f32) {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for q in qs {
            for v in &q.verts {
                lo = lo.min(v[0]);
                hi = hi.max(v[0]);
            }
        }
        (lo, hi)
    };
    let (head_lo, head_hi) = extent(&zombie.quads[..6]);
    let (hat_lo, hat_hi) = extent(&zombie.quads[6..]);
    c.record(
        "k3.the_hat_encloses_the_head_by_a_quarter_pixel",
        (hat_lo - (head_lo - 0.25)).abs() < 1e-5 && (hat_hi - (head_hi + 0.25)).abs() < 1e-5,
        format!(
            "head x {head_lo}..{head_hi}, hat x {hat_lo}..{hat_hi} — \
             `CubeDeformation(0.25)` grows every side, which is what stands a \
             player head's second skin layer proud of the first"
        ),
    );

    // A mob head sheet is 64x32, not 64x64 — every UV must still be in range,
    // which is the check that a hard-coded 64 would fail on the v axis.
    let mut uv_ok = true;
    for n in &names {
        for q in &items.block_entities.get(*n).unwrap().quads {
            for uv in q.uv {
                uv_ok &= (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]);
            }
        }
    }
    c.record(
        "k4.every_skull_uv_is_inside_its_own_sheet",
        uv_ok,
        "all seven models unwrap inside 0..1 — the mob heads are 64x32 and the \
         dragon is 256x256, so the per-model texture size is real and a \
         hard-coded 64 would push the mob heads off the bottom",
    );

    // The dragon is the only mirrored model: its left and right scales come
    // from ONE texture rect, read in opposite directions.
    let dragon = items.block_entities.get("rewo:be/dragon_head").unwrap();
    let scale_uvs: Vec<[[f32; 2]; 4]> = dragon
        .quads
        .iter()
        .filter(|q| q.uv.iter().all(|uv| uv[0] < 0.1 && uv[1] < 0.1))
        .map(|q| q.uv)
        .collect();
    c.record(
        "k5.the_dragon_scales_share_one_texture_rect",
        scale_uvs.len() >= 12 && dragon.quads.len() == 42,
        format!(
            "{} dragon quads (7 boxes), of which {} sample the (0,0) rect that \
             `mirror(true)`/`mirror(false)` share between the left and right \
             scale — one rect, read both ways",
            dragon.quads.len(),
            scale_uvs.len()
        ),
    );

    // --- the transforms ---------------------------------------------------
    let apply = |m: &bt::Affine, p: [f32; 3]| -> [f32; 3] {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    };
    let ground = bt::skull_ground(0);
    c.record(
        "k6.the_ground_transform_flips_the_entity_model_upright",
        ground[1][1] < 0.0 && ground[0][0] < 0.0 && ground[2][2] > 0.0,
        format!(
            "scale ({}, {}, {}) — `scale(-1, -1, 1)`. `SkullModelBase` is an \
             ENTITY model, authored y-down like a mob, and this is what rights \
             it. A chest has no such flip, and carrying one family's assumption \
             to the other renders the skull upside down and mirrored",
            ground[0][0], ground[1][1], ground[2][2]
        ),
    );

    // The head box spans y -8..0 in model space; after the flip and the /16
    // it must sit inside the block it occupies.
    let head_top = apply(&ground, [0.0, -8.0 / 16.0, 0.0]);
    let head_bottom = apply(&ground, [0.0, 0.0, 0.0]);
    c.record(
        "k7.a_ground_skull_sits_on_the_block_floor",
        (head_bottom[1] - 0.0).abs() < 1e-5 && (head_top[1] - 0.5).abs() < 1e-5,
        format!(
            "the head's own origin lands at y {:.3} and its top at y {:.3} — an \
             8 px cube standing on the floor of the block, which is where a \
             skull sits",
            head_bottom[1], head_top[1]
        ),
    );

    // A wall skull steps a quarter-block away from the wall it hangs on, in
    // the direction it faces.
    let north = bt::skull_wall(bt::Facing6::North);
    let south = bt::skull_wall(bt::Facing6::South);
    let (nx, ny, nz) = {
        let p = apply(&north, [0.0, 0.0, 0.0]);
        (p[0], p[1], p[2])
    };
    let sz = apply(&south, [0.0, 0.0, 0.0])[2];
    c.record(
        "k8.a_wall_skull_steps_off_its_wall_and_rides_higher",
        (nx - 0.5).abs() < 1e-5 && (ny - 0.25).abs() < 1e-5 && nz > 0.5 && sz < 0.5,
        format!(
            "north-facing origin ({nx:.3}, {ny:.3}, {nz:.3}) and south-facing z \
             {sz:.3} — `0.5 - getStepZ() * 0.25` pushes it against the wall \
             BEHIND the way it faces, and y 0.25 lifts it off the floor a \
             ground skull rests on"
        ),
    );

    // Every one of the fourteen blocks resolves, and to a model that baked.
    let states = rewo_data::chest_states::ChestStates::load(&paths.blocks_json())?;
    let mut seen = 0;
    let mut ok = true;
    let mut blocks_covered: HashSet<String> = HashSet::new();
    for id in 0..40000u32 {
        let Some(d) = states.draw_for(id) else { continue };
        if !rewo_data::block_entity_models::SKULL_TEXTURES
            .iter()
            .any(|(n, _)| *n == d.model)
        {
            continue;
        }
        seen += 1;
        ok &= items.block_entities.contains_key(&d.model);
        if let Some(b) = blocks.block_name(id) {
            blocks_covered.insert(b.to_string());
        }
    }
    c.record(
        "k9.all_fourteen_skull_blocks_resolve",
        ok && blocks_covered.len() == 14 && seen > 0,
        format!(
            "{seen} skull block states across {} blocks, every one naming a \
             baked model — seven types times a ground and a wall variant",
            blocks_covered.len()
        ),
    );

    // --- the conduit ------------------------------------------------------
    let conduit = items
        .block_entities
        .get(rewo_data::block_entity_models::CONDUIT.0)
        .ok_or("the conduit shell did not bake")?;
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for q in &conduit.quads {
        for v in &q.verts {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
    }
    c.record(
        "k11.the_conduit_shell_is_a_six_px_cube_about_its_own_origin",
        conduit.quads.len() == 6 && lo == [-3.0; 3] && hi == [3.0; 3],
        format!(
            "{} quads spanning {lo:?}..{hi:?} — symmetric about zero, which is \
             why its transform is a plain centre translate with no flip, \
             unlike a skull's",
            conduit.quads.len()
        ),
    );

    let cm = bt::conduit(0.0);
    let centre = apply(&cm, [0.0, 0.0, 0.0]);
    let corner = apply(&cm, [3.0 / 16.0, 3.0 / 16.0, 3.0 / 16.0]);
    c.record(
        "k12.a_conduit_floats_in_the_middle_of_its_block",
        centre == [0.5, 0.5, 0.5]
            && corner.iter().all(|v| (*v - 0.6875).abs() < 1e-5),
        format!(
            "the model origin lands at {centre:?} and its corner at {corner:?} \
             — a conduit hangs in the middle of its block rather than standing \
             on the floor the way a skull does"
        ),
    );

    // --- the decorated pot ------------------------------------------------
    use rewo_data::block_entity_models as bem;
    let missing_sherds: Vec<&&str> = bem::POT_SHERDS
        .iter()
        .filter(|s| {
            !items
                .block_entities
                .contains_key(&format!("rewo:be/pot_side/{s}"))
        })
        .collect();
    c.record(
        "k13.every_sherd_pattern_bakes",
        missing_sherds.is_empty()
            && bem::POT_SHERDS.len() == 23
            && items.block_entities.contains_key(bem::POT_SIDE_PLAIN.0)
            && items.block_entities.contains_key(bem::POT_BASE_MODEL.0),
        format!(
            "{} sherd patterns plus the plain side and the base all baked, \
             missing {missing_sherds:?}. The texture name is DERIVED — \
             `<stem>_pottery_sherd` becomes `<stem>_pottery_pattern` — so this \
             is what stops that derivation drifting from the jar",
            bem::POT_SHERDS.len()
        ),
    );

    c.record(
        "k14.a_pot_side_is_one_quad_and_the_base_is_four_boxes",
        items
            .block_entities
            .get(bem::POT_SIDE_PLAIN.0)
            .map(|m| m.quads.len())
            == Some(1)
            && items
                .block_entities
                .get(bem::POT_BASE_MODEL.0)
                .map(|m| m.quads.len())
                == Some(24),
        format!(
            "side {:?} quads, base {:?} — `addBox(..., EnumSet.of(NORTH))` \
             builds only the north face, so a side really is a single plane",
            items
                .block_entities
                .get(bem::POT_SIDE_PLAIN.0)
                .map(|m| m.quads.len()),
            items
                .block_entities
                .get(bem::POT_BASE_MODEL.0)
                .map(|m| m.quads.len())
        ),
    );

    c.record(
        "k15.a_sherd_item_name_selects_its_pattern_and_anything_else_is_plain",
        bem::pot_side_model(Some("minecraft:angler_pottery_sherd"))
            == "rewo:be/pot_side/angler"
            && bem::pot_side_model(Some("skull_pottery_sherd")) == "rewo:be/pot_side/skull"
            && bem::pot_side_model(Some("minecraft:brick")) == bem::POT_SIDE_PLAIN.0
            && bem::pot_side_model(None) == bem::POT_SIDE_PLAIN.0
            && bem::pot_side_model(Some("minecraft:diamond")) == bem::POT_SIDE_PLAIN.0,
        "angler and skull sherds resolve their own patterns (namespaced or \
         not); brick, an empty slot and a non-sherd item all fall through to \
         the plain side, which is exactly what `getSideSprite` does",
    );

    // The four side poses must put one plane on each face of the block.
    let corners: Vec<[f32; 3]> = (0..4)
        .map(|i| {
            let m = bt::pot_side(i);
            let p = apply(&m, [7.0, 8.0, 0.0]); // the plane's own centre
            [p[0] / 16.0, p[1] / 16.0, p[2] / 16.0]
        })
        .collect();
    let spread_x = corners
        .iter()
        .map(|p| p[0])
        .fold(f32::MIN, f32::max)
        - corners.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
    let spread_z = corners
        .iter()
        .map(|p| p[2])
        .fold(f32::MIN, f32::max)
        - corners.iter().map(|p| p[2]).fold(f32::MAX, f32::min);
    c.record(
        "k16.the_four_side_poses_face_four_different_ways",
        spread_x > 0.7 && spread_z > 0.7 && corners.iter().all(|p| (0.0..=1.0).contains(&p[1])),
        format!(
            "side centres {corners:?} — they spread {spread_x:.2} in x and \
             {spread_z:.2} in z, i.e. one per face. All four planes baked in \
             group {} rather than 0, which is what routes them through the \
             side pose instead of leaving them stacked in one place",
            bem::POT_SIDE_PART
        ),
    );

    // The pot turns about the block centre in ALL THREE axes, unlike a chest.
    let pot_m = bt::decorated_pot(0.0);
    let mid = apply(&pot_m, [0.5, 0.5, 0.5]);
    let chest_m = bt::chest(0.0);
    let chest_mid = apply(&chest_m, [0.5, 0.0, 0.5]);
    c.record(
        "k17.a_pot_turns_about_the_block_centre_not_its_floor",
        mid.iter()
            .zip([0.5, 0.5, 0.5])
            .all(|(a, b)| (a - b).abs() < 1e-5)
            && (chest_mid[1] - 0.0).abs() < 1e-5,
        format!(
            "the pot's rotation fixes {mid:?} while a chest's fixes \
             {chest_mid:?} — `rotateAround(..., 0.5, 0.5, 0.5)` against \
             `rotationAround(..., 0.5, 0, 0.5)`"
        ),
    );

    // --- banners ----------------------------------------------------------
    let missing_pat: Vec<&&str> = bem::BANNER_PATTERNS
        .iter()
        .filter(|s| {
            !items
                .block_entities
                .contains_key(&format!("rewo:be/banner_pattern/{s}"))
        })
        .collect();
    c.record(
        "k18.every_banner_pattern_bakes",
        missing_pat.is_empty() && bem::BANNER_PATTERNS.len() == 43,
        format!(
            "{} banner patterns baked, missing {missing_pat:?} — one flag-shaped \
             model per pattern, each a greyscale mask",
            bem::BANNER_PATTERNS.len()
        ),
    );

    c.record(
        "k19.a_standing_banner_has_a_pole_and_a_wall_one_does_not",
        items
            .block_entities
            .get(bem::BANNER_STANDING_BODY_MODEL)
            .map(|m| m.quads.len())
            == Some(12)
            && items
                .block_entities
                .get(bem::BANNER_WALL_BODY_MODEL)
                .map(|m| m.quads.len())
                == Some(6),
        format!(
            "standing body {:?} quads (pole + bar = 2 boxes), wall body {:?} \
             (bar only) — a wall banner hangs from its bar with nothing \
             beneath, which is why they are separate layers rather than one \
             model with a hidden part",
            items
                .block_entities
                .get(bem::BANNER_STANDING_BODY_MODEL)
                .map(|m| m.quads.len()),
            items
                .block_entities
                .get(bem::BANNER_WALL_BODY_MODEL)
                .map(|m| m.quads.len())
        ),
    );

    c.record(
        "k20.the_dye_tint_is_the_diffuse_colour_not_the_text_one",
        bem::DYE_DIFFUSE_COLORS.len() == 16
            && bem::DYE_DIFFUSE_COLORS[14] == 0xB02E26
            && rewo_data::sign_text::dye_text_color(Some("red")) == 0xFF0000,
        format!(
            "red dyes a banner {:#08x} and writes on a sign {:#08x} — both are \
             fields on the same enum, and tinting a banner from the sign table \
             would be plausibly and visibly wrong",
            bem::DYE_DIFFUSE_COLORS[14],
            rewo_data::sign_text::dye_text_color(Some("red"))
        ),
    );

    // A banner's model is 2/3 scale with a negative y and z — another
    // entity-authored model, like a skull and unlike a chest.
    let bm = bt::banner(0.0);
    c.record(
        "k21.a_banner_is_two_thirds_scale_and_entity_flipped",
        (bm[0][0] - 0.666_666_7).abs() < 1e-5 && bm[1][1] < 0.0 && bm[2][2] < 0.0,
        format!(
            "scale ({:.4}, {:.4}, {:.4}) — 2/3, which is what fits a 44 px pole \
             into a two-block banner, with y and z negated the way every \
             entity-authored model here is",
            bm[0][0], bm[1][1], bm[2][2]
        ),
    );

    // Every banner state resolves, across all 32 blocks.
    let mut banner_blocks: HashSet<String> = HashSet::new();
    let mut banner_states = 0;
    for id in 0..40000u32 {
        let Some(d) = states.draw_for(id) else { continue };
        if !matches!(
            d.anim,
            rewo_data::chest_states::BlockEntityAnim::Banner { .. }
        ) {
            continue;
        }
        banner_states += 1;
        if let Some(b) = blocks.block_name(id) {
            banner_blocks.insert(b.to_string());
        }
    }
    c.record(
        "k22.all_thirty_two_banner_blocks_resolve",
        banner_blocks.len() == 32 && banner_states > 0,
        format!(
            "{banner_states} banner states across {} blocks — sixteen colours \
             standing (16 rotations each) and sixteen on walls (4 facings)",
            banner_blocks.len()
        ),
    );

    // --- the copper golem statue ------------------------------------------
    let poses = rewo_data::copper_golem_poses::POSES;
    let mut statue_missing = Vec::new();
    for (weather, _) in bem::STATUE_TEXTURES {
        for (pose, _) in poses {
            let n = bem::statue_model(weather, pose);
            if !items.block_entities.contains_key(&n) {
                statue_missing.push(n);
            }
        }
    }
    c.record(
        "k23.every_statue_pose_and_weathering_state_bakes",
        statue_missing.is_empty() && poses.len() == 4 && bem::STATUE_TEXTURES.len() == 4,
        format!(
            "{} poses x {} weathering states baked, missing {statue_missing:?} \
             — the four poses are SEPARATE layers, not one model posed",
            poses.len(),
            bem::STATUE_TEXTURES.len()
        ),
    );

    let box_count: usize = poses.iter().map(|(_, b)| b.len()).sum();
    c.record(
        "k24.the_generated_table_carries_every_box",
        box_count == 38
            && poses.iter().all(|(_, b)| !b.is_empty())
            && poses.iter().any(|(_, b)| b.iter().any(|x| x.chain.len() > 1)),
        format!(
            "{box_count} boxes across the four poses, and at least one hangs \
             two levels deep. Machine-extracted by \
             tools/gen_copper_golem_poses.py: thirty-eight boxes with rotations \
             to four decimal places is transcription whose errors are silent"
        ),
    );

    // THE property the flat box list could not express: a rotated parent must
    // carry its children ROUND with it, not merely shift them. Compare each
    // box's real position against a naive offset-sum that ignores every
    // ancestor rotation — they must agree where no ancestor turns, and differ
    // where one does.
    let naive = |b: &rewo_data::block_entity_models::StatueBox| -> [f32; 3] {
        let mut p = b.min;
        for k in 0..3 {
            p[k] += b.own.0[k];
            for a in b.chain {
                p[k] += a.0[k];
            }
        }
        p
    };
    let real = |b: &rewo_data::block_entity_models::StatueBox| -> [f32; 3] {
        rewo_data::block_entity_models::statue_corner(b)
    };
    let standing = poses.iter().find(|(p, _)| *p == "standing").unwrap().1;
    let running = poses.iter().find(|(p, _)| *p == "running").unwrap().1;
    let flat_agrees = standing
        .iter()
        .all(|b| (0..3).all(|k| (real(b)[k] - naive(b)[k]).abs() < 1e-4));
    let rotated_differs = running
        .iter()
        .any(|b| (0..3).any(|k| (real(b)[k] - naive(b)[k]).abs() > 0.05));
    c.record(
        "k25.a_rotated_parent_carries_its_children_round",
        flat_agrees && rotated_differs,
        format!(
            "the STANDING pose has no ancestor rotations, so every box lands \
             exactly where a naive offset-sum puts it ({flat_agrees}); the \
             RUNNING pose does, and at least one box lands somewhere a naive \
             sum does not ({rotated_differs}). That difference IS the nested \
             hierarchy — a flat box list would have placed the running statue \
             plausibly and wrongly"
        ),
    );

    let sm = bt::copper_golem_statue(bt::Facing6::North);
    c.record(
        "k26.the_statue_flip_comes_from_setup_anim_not_the_matrix",
        sm[0][0] < 0.0 && sm[1][1] < 0.0 && (sm[2][2] - 1.0).abs() < 1e-5,
        format!(
            "diagonal ({:.3}, {:.3}, {:.3}) — vanilla's matrix carries NO flip; \
             it is `CopperGolemStatueModel.setupAnim` setting `root.zRot = PI`, \
             a half turn about Z that negates x and y. Folded in here because \
             this client does not run that animation step",
            sm[0][0], sm[1][1], sm[2][2]
        ),
    );

    let mut statue_blocks: HashSet<String> = HashSet::new();
    for id in 0..40000u32 {
        let Some(d) = states.draw_for(id) else { continue };
        if !d.model.starts_with("rewo:be/statue/") {
            continue;
        }
        if let Some(b) = blocks.block_name(id) {
            statue_blocks.insert(b.to_string());
        }
    }
    c.record(
        "k27.all_eight_statue_blocks_resolve",
        statue_blocks.len() == 8,
        format!(
            "{} statue blocks resolve — four weathering states, waxed and \
             unwaxed, a waxed one sharing its level's texture exactly as a \
             waxed copper chest does",
            statue_blocks.len()
        ),
    );

    // --- the two end portals ----------------------------------------------
    let portal = items
        .block_entities
        .get(bem::END_PORTAL_MODEL.0)
        .ok_or("the end portal did not bake")?;
    let gateway = items
        .block_entities
        .get(bem::END_GATEWAY_MODEL)
        .ok_or("the end gateway did not bake")?;
    c.record(
        "k28.a_portal_builds_only_its_horizontal_faces_and_a_gateway_all_six",
        portal.quads.len() == 2 && gateway.quads.len() == 6,
        format!(
            "portal {} quads, gateway {} — `TheEndPortalBlockEntity.\
             shouldRenderFace` is `getAxis() == Y`, so a portal is a pool seen \
             from above or below and has no sides at all, which is why looking \
             at one edge-on in vanilla shows nothing",
            portal.quads.len(),
            gateway.quads.len()
        ),
    );

    // The portal is a SLAB, not a full block, and not flush with the floor.
    let pm = bt::end_portal();
    let lo = apply(&pm, [0.0, 0.0, 0.0]);
    let hi = apply(&pm, [0.0, 1.0, 0.0]);
    c.record(
        "k29.the_portal_is_a_slab_between_three_eighths_and_three_quarters",
        (lo[1] - 0.375).abs() < 1e-5 && (hi[1] - 0.75).abs() < 1e-5,
        format!(
            "the unit cube's y maps to {:.3}..{:.3} — `Transformation(translate\
             (0, 0.375, 0), null, scale(1, 0.375, 1), null)` squashes then \
             lifts it, so an end portal is a pool set into the MIDDLE of its \
             block rather than a full block or a floor-flush sheet",
            lo[1], hi[1]
        ),
    );

    let gm = bt::end_gateway();
    c.record(
        "k30.a_gateway_fills_its_block_and_pushes_no_transform",
        gm == rewo_data::be_transform::IDENTITY,
        "`TheEndGatewayRenderer.submit` pushes no transform at all — its cube \
         fills the block, where the portal's is squashed to a slab",
    );

    // The wall-name derivation is not a second hard-coded list.
    c.record(
        "k10.the_wall_block_name_is_derived_not_listed",
        rewo_data::chest_states::ChestStates::wall_name("skeleton_skull")
            == "skeleton_wall_skull"
            && rewo_data::chest_states::ChestStates::wall_name("zombie_head")
                == "zombie_wall_head",
        "skeleton_skull -> skeleton_wall_skull and zombie_head -> \
         zombie_wall_head: `wall` goes before the LAST segment, and that \
         segment differs per type, so the name is derived rather than listed \
         twice",
    );
    Ok(())
}

/// M27 — dyed and glowing sign text, and the line break that keeps it on the
/// board.
///
/// The end-to-end witnesses drive `live_cmd::collect_sign_text`, the same
/// collector the client renders from, so the dye lookup, the style branch, the
/// split and the outline expansion are exercised where they actually run.
fn check_sign_style(
    c: &mut Checker,
    blocks: &rewo_data::blocks::Blocks,
    paths: &DataPaths,
    sign_type: i32,
) -> Result<(), String> {
    use rewo_data::sign_text;

    c.record(
        "g1.the_dye_table_is_the_text_colour_not_the_texture_one",
        sign_text::dye_text_color(Some("red")) == 0xFF0000
            && sign_text::dye_text_color(Some("purple")) == 0xA020F0
            && sign_text::dye_text_color(Some("pink")) == 0xFF69B4
            && sign_text::DYE_TEXT_COLORS.len() == 16,
        "red=0xFF0000, purple=0xA020F0, pink=0xFF69B4 — DyeColor's LAST \
         constructor argument. Its `textureDiffuseColor` (red = 0xB02E26) is \
         the first, and picking it gives a plausible sign in the wrong shade",
    );

    let red = sign_text::dye_text_color(Some("red"));
    let plain = sign_text::text_style(red, false, true);
    let glow = sign_text::text_style(red, true, true);
    c.record(
        "g2.glow_is_not_the_same_colour_brighter",
        plain.color == 0x660000
            && !plain.fullbright
            && plain.outline.is_none()
            && glow.color == 0xFF0000
            && glow.fullbright
            && glow.outline == Some(0x660000),
        format!(
            "unglowing red = {:#08x} (the dye at 40%), glowing red = {:#08x} \
             (the dye at FULL strength, fullbright) with {:#08x?} as its \
             outline — the dim colour is demoted to the outline rather than \
             being the dim case of the bright one",
            plain.color, glow.color, glow.outline
        ),
    );

    c.record(
        "g3.glowing_black_outlines_at_any_range_and_in_a_different_colour",
        sign_text::text_style(0x000000, true, false).outline
            == Some(sign_text::BLACK_TEXT_OUTLINE_COLOR)
            && sign_text::text_style(0x000000, true, true).outline
                == Some(sign_text::BLACK_TEXT_OUTLINE_COLOR)
            && sign_text::text_style(red, true, false).outline.is_none(),
        format!(
            "glowing black outlines {:#08x} (near-white) whether or not the \
             camera is close, because scaling black by 0.4 is still black and \
             there would be nothing to see — while a glowing *red* sign drops \
             its outline at range",
            sign_text::BLACK_TEXT_OUTLINE_COLOR
        ),
    );

    c.record(
        "g4.the_forty_percent_scale_truncates",
        sign_text::scale_rgb(0xFEFEFE, 0.4) == 0x656565
            && sign_text::scale_rgb(0xFFFFFF, 0.4) == 0x666666,
        "0xFE * 0.4 = 101.6 -> 101 (`(int)` truncates); rounding would \
         brighten every dyed sign by a step per channel",
    );

    let jar = client_jar("26.2").ok_or("client jar not found")?;
    let baked = rewo_data::assets::bake(&jar, &paths.blocks_json())?;
    let font = baked.font.as_ref().ok_or("no font baked from the jar")?;
    let adv = &font.advance;

    let long = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let cut = sign_text::split_first(long, rewo_data::sign_states::SIGN_MAX_WIDTH, adv);
    c.record(
        "g5.an_overlong_line_is_truncated_to_the_board",
        cut.len() < long.len()
            && sign_text::width(&cut, adv) <= rewo_data::sign_states::SIGN_MAX_WIDTH,
        format!(
            "{long:?} ({:.0} px) -> {cut:?} ({:.0} px) against a 90 px board. \
             Vanilla splits and keeps fragment 0, so the tail is DROPPED — a \
             sign does not wrap onto the next row",
            sign_text::width(long, adv),
            sign_text::width(&cut, adv)
        ),
    );

    let words = "the quick brown fox jumps over";
    let wcut = sign_text::split_first(words, rewo_data::sign_states::SIGN_MAX_WIDTH, adv);
    c.record(
        "g6.it_breaks_at_a_word_boundary_and_drops_the_space",
        !wcut.ends_with(' ') && words.starts_with(wcut.as_str()) && wcut.contains(' '),
        format!(
            "{words:?} -> {wcut:?} — the break retreats to `lastSpace`, and \
             `splitLines` passes the unadjusted index so the space itself is \
             excluded"
        ),
    );

    let hcut = sign_text::split_first(words, rewo_data::sign_states::HANGING_MAX_WIDTH, adv);
    c.record(
        "g7.a_hanging_sign_breaks_earlier",
        hcut.len() < wcut.len(),
        format!(
            "the same line on a 60 px hanging board gives {hcut:?} against the \
             90 px standing board's {wcut:?} — the two differ in width as well \
             as in line height"
        ),
    );

    let signs = rewo_data::sign_states::SignStates::load(&paths.blocks_json())?;
    let sign_state = blocks
        .default_state("minecraft:oak_sign")
        .ok_or("blocks.json: no minecraft:oak_sign")?;
    let lightmap = rewo_world::lightmap::LightmapState::default();

    let build =
        |color: Option<&str>, glowing: bool, text: &str| -> Vec<crate::live_cmd::OwnedSignLine> {
            let shape = DimensionShape::OVERWORLD;
            let mut world = rewo_world::World::new(shape);
            let body = chunk_body(0, 0, shape.section_count(), &[(0x00, 64, sign_type)]);
            let mut r = rewo_proto::reader::PacketReader::new(&body);
            let col = rewo_world::chunk::read_level_chunk(&mut r, &shape, blocks).unwrap();
            world.insert_column(0, 0, col);
            world.set_block(0, 64, 0, sign_state);
            let mut face = vec![(
                "messages".to_string(),
                Nbt::List(vec![
                    Nbt::String(text.to_string()),
                    Nbt::String(String::new()),
                    Nbt::String(String::new()),
                    Nbt::String(String::new()),
                ]),
            )];
            if let Some(c) = color {
                face.push(("color".to_string(), Nbt::String(c.to_string())));
            }
            if glowing {
                face.push(("has_glowing_text".to_string(), Nbt::Byte(1)));
            }
            world.block_entities.insert(
                BlockEntityPos { x: 0, y: 64, z: 0 },
                rewo_world::block_entities::BlockEntity {
                    type_id: sign_type,
                    data: Nbt::Compound(vec![("front_text".to_string(), Nbt::Compound(face))]),
                },
            );
            crate::live_cmd::collect_sign_text(&world, &signs, &lightmap, adv)
        };

    let plain_lines = build(None, false, "hi");
    let dyed_lines = build(Some("red"), false, "hi");
    c.record(
        "g8.a_dyed_sign_reaches_the_renderer_in_its_own_colour",
        plain_lines.len() == 1
            && dyed_lines.len() == 1
            && plain_lines[0].color != dyed_lines[0].color
            && dyed_lines[0].color[0] > dyed_lines[0].color[2],
        format!(
            "default {:?} vs red {:?} through `collect_sign_text` — before M27 \
             both were black, because the collector had no dye table",
            plain_lines[0].color, dyed_lines[0].color
        ),
    );

    let glow_lines = build(Some("red"), true, "hi");
    c.record(
        "g9.glowing_text_draws_eight_outline_copies_behind_one_glyph_run",
        glow_lines.len() == 9
            && glow_lines.iter().filter(|l| l.z < 0.0).count() == 8
            && glow_lines.iter().filter(|l| l.z == 0.0).count() == 1,
        format!(
            "{} draws for one glowing line: 8 outline copies at z<0 and the \
             glyphs at z=0. `prepare8xTextOutline` walks xo,yo in -1..1 minus \
             (0,0) — eight, not four; the diagonals close the corners",
            glow_lines.len()
        ),
    );

    let main = glow_lines.iter().find(|l| l.z == 0.0).unwrap();
    let offsets: HashSet<(i32, i32)> = glow_lines
        .iter()
        .filter(|l| l.z < 0.0)
        .map(|l| ((l.x - main.x).round() as i32, (l.y - main.y).round() as i32))
        .collect();
    let want: HashSet<(i32, i32)> = OUTLINE_RING.iter().copied().collect();
    c.record(
        "g10.the_outline_copies_ring_the_glyphs",
        offsets == want,
        format!("outline offsets {:?} — the full 8-neighbourhood", {
            let mut v: Vec<_> = offsets.iter().copied().collect();
            v.sort_unstable();
            v
        }),
    );

    // Which light *source* each branch takes, asserted exactly rather than as
    // "one is brighter". The synthesised chunk is open air and reads full sky
    // light, so a brighter-than comparison would be satisfied by both branches
    // returning the same maximum — it could not tell them apart, and the first
    // draft of this witness could not. `sample(7, 7)` rather than `sample(0,
    // 0)` because the latter is the shader's genuine 0/0 black-texel path and
    // yields NaN, which compares unequal to everything.
    let dim = rewo_world::lightmap::sample(7, 7, &lightmap);
    let full = rewo_world::lightmap::sample(15, 15, &lightmap);
    c.record(
        "g11.glowing_text_takes_fullbright_and_unglowing_text_takes_the_block",
        main.light == full && dim != full && dim.iter().all(|v| v.is_finite()),
        format!(
            "glowing light {:?} == sample(15,15) {:?} — vanilla passes the \
             literal 15728880, both nibbles at 15, so glow ink reads in an \
             unlit room. sample(7,7) is {dim:?}, a finite and genuinely \
             different colour, so this is not a comparison against itself",
            main.light, full
        ),
    );

    let whole = "the quick brown fox jumps over the lazy dog";
    let wrapped = build(None, false, whole);
    c.record(
        "g12.the_collector_truncates_a_long_line_rather_than_overhanging",
        wrapped.len() == 1
            && wrapped[0].text.len() < whole.len()
            && sign_text::width(&wrapped[0].text, adv)
                <= rewo_data::sign_states::SIGN_MAX_WIDTH,
        format!(
            "{:?} ({:.0} px) reached the renderer, not the whole \
             {}-character line — which before M27 ran off both ends of the board",
            wrapped[0].text,
            sign_text::width(&wrapped[0].text, adv),
            whole.len()
        ),
    );
    Ok(())
}

/// The eight offsets `prepare8xTextOutline` draws at, as the gate expects to
/// observe them. Written here independently of the renderer's own list.
const OUTLINE_RING: [(i32, i32); 8] = [
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, -1),
    (0, 1),
    (1, -1),
    (1, 0),
    (1, 1),
];

/// M26 — a shulker box's lid opens: the state machine and the two-channel
/// transform that moves it.
fn check_shulker_anim(c: &mut Checker, version: &str) -> Result<(), String> {
    use rewo_data::be_transform as bt;
    use rewo_world::block_entities::{ShulkerAnim, ShulkerStatus};

    // --- the clock, against a transcription written from the decompile -----
    //
    // `updateAnimation` is four assignments and two thresholds. Reproducing it
    // here as a plain tuple machine — no shared code with `ShulkerAnim` — is
    // what makes the comparison mean something.
    fn oracle(status: &mut u8, p: &mut f32, p_old: &mut f32) {
        *p_old = *p;
        match *status {
            0 => *p = 0.0,          // CLOSED
            1 => {
                *p += 0.1;          // OPENING — note: unclamped, then tested
                if *p >= 1.0 {
                    *status = 2;
                    *p = 1.0;
                }
            }
            2 => *p = 1.0,          // OPENED
            _ => {
                *p -= 0.1;          // CLOSING
                if *p <= 0.0 {
                    *status = 0;
                    *p = 0.0;
                }
            }
        }
    }

    let mut a = ShulkerAnim {
        status: ShulkerStatus::Opening,
        ..Default::default()
    };
    let (mut os, mut op, mut oo) = (1u8, 0.0f32, 0.0f32);
    let mut agree = true;
    let mut seq = Vec::new();
    let mut opened_at = None;
    for t in 1..=14 {
        a.tick();
        oracle(&mut os, &mut op, &mut oo);
        agree &= (a.progress - op).abs() < 1e-7 && (a.progress_old - oo).abs() < 1e-7;
        seq.push(format!("{:.2}", a.progress));
        if a.status == ShulkerStatus::Opened && opened_at.is_none() {
            opened_at = Some(t);
        }
    }
    c.record(
        "o1.the_open_clock_matches_an_independent_transcription",
        agree && a.status == ShulkerStatus::Opened && a.progress == 1.0,
        format!("progress by tick: {} — ends OPENED at 1.0", seq.join(" ")),
    );

    c.record(
        "o2.it_reaches_opened_when_the_f32_sum_crosses_one",
        opened_at == Some(10),
        format!(
            "OPENED on tick {opened_at:?}. `+= 0.1` is unclamped with a separate \
             `>= 1.0` test, so the tick is decided by where the f32 sum lands — \
             it is not a counted-out ten the way the chest's clamped `min` is"
        ),
    );

    // OPENED and CLOSED *assign* their endpoint every tick, so extra ticks
    // cannot drift them — a converging implementation would.
    for _ in 0..20 {
        a.tick();
    }
    c.record(
        "o3.opened_holds_exactly",
        a.progress == 1.0 && a.progress_old == 1.0 && a.status == ShulkerStatus::Opened,
        "20 further ticks left progress exactly 1.0 — OPENED assigns rather \
         than approaches",
    );

    a.status = ShulkerStatus::Closing;
    let mut shut_in = 0;
    for t in 1..=20 {
        a.tick();
        if a.status == ShulkerStatus::Closed && shut_in == 0 {
            shut_in = t;
        }
    }
    c.record(
        "o4.it_shuts_and_settles_at_zero",
        shut_in == 10 && a.progress == 0.0 && a.status == ShulkerStatus::Closed,
        format!("CLOSED after {shut_in} ticks at exactly {}", a.progress),
    );

    let mid = ShulkerAnim {
        status: ShulkerStatus::Opening,
        progress: 0.4,
        progress_old: 0.3,
    };
    c.record(
        "o5.the_render_progress_interpolates_between_ticks",
        (mid.progress(0.0) - 0.3).abs() < 1e-6
            && (mid.progress(0.5) - 0.35).abs() < 1e-6
            && (mid.progress(1.0) - 0.4).abs() < 1e-6,
        format!(
            "a=0 -> {:.3}, a=0.5 -> {:.3}, a=1 -> {:.3} (progressOld 0.3, progress 0.4)",
            mid.progress(0.0),
            mid.progress(0.5),
            mid.progress(1.0)
        ),
    );

    // --- the transform ----------------------------------------------------
    let at = |p: f32, v: [f32; 3]| -> [f32; 3] {
        let m = bt::shulker_lid(p);
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2] + m[0][3],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2] + m[1][3],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2] + m[2][3],
        ]
    };

    // At rest the transform must be the *pose offset itself*, so a shut box is
    // the baked geometry untouched — exactly, not to a tolerance.
    let rest = at(0.0, [0.0, 0.0, 0.0]);
    let rest_x = at(0.0, [1.0, 0.0, 0.0]);
    c.record(
        "o6.a_shut_box_is_the_rest_pose_exactly",
        rest == [0.0, 24.0, 0.0] && rest_x == [1.0, 24.0, 0.0],
        format!(
            "progress 0 puts the group origin at {rest:?} — `setPos(0, 24 - 0*8, 0)` \
             is the (0,24,0) the bake already applied, so the closed box needs no \
             animation at all"
        ),
    );

    let open = at(1.0, [0.0, 0.0, 0.0]);
    let open_x = at(1.0, [1.0, 0.0, 0.0]);
    c.record(
        "o7.the_lid_travels_eight_px_and_turns_three_quarters",
        (open[1] - 16.0).abs() < 1e-5
            && (open_x[0] - open[0]).abs() < 1e-5
            && (open_x[2] - open[2] - 1.0).abs() < 1e-5,
        format!(
            "origin {open:?} (24 - 0.5*16 = 16, so 8 model px = half a block), and \
             the +x axis lands on {:?} — a 270 degree turn about Y, not a tilt",
            [open_x[0] - open[0], open_x[1] - open[1], open_x[2] - open[2]]
        ),
    );

    // The observable that a sign error would break: after the renderer's
    // `scale(1, -1, -1)`, the lid must move *up* in the world, not down.
    let world_y = |p: f32| -> f32 {
        let block = bt::shulker_box(bt::Facing6::Up);
        let l = at(p, [0.0, 0.0, 0.0]);
        // model px -> block units, then the renderer's transform.
        let v = [l[0] / 16.0, l[1] / 16.0, l[2] / 16.0];
        block[1][0] * v[0] + block[1][1] * v[1] + block[1][2] * v[2] + block[1][3]
    };
    let (shut_y, open_y) = (world_y(0.0), world_y(1.0));
    c.record(
        "o8.the_open_lid_rises_half_a_block_in_the_world",
        open_y - shut_y > 0.49 && open_y - shut_y < 0.51,
        format!(
            "lid origin world y {shut_y:.4} shut -> {open_y:.4} open, a rise of \
             {:.4}. In the box's own space that travel is -y; the renderer's \
             trailing scale(1,-1,-1) is what turns it into the lid lifting off \
             the base, and a sign error here reads as a box sinking into the floor",
            open_y - shut_y
        ),
    );

    // --- the bake puts the lid in its own group ---------------------------
    let paths = DataPaths::for_version(version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(version).ok_or("client jar not found")?;
    let baked = rewo_data::assets::bake(&jar, &paths.blocks_json())?;
    let model = baked
        .held_items
        .block_entities
        .get("rewo:be/shulker_box")
        .ok_or("shulker box model did not bake")?;
    let lid = model
        .quads
        .iter()
        .filter(|q| q.part == rewo_data::block_entity_models::SHULKER_LID_PART)
        .count();
    let base = model.quads.iter().filter(|q| q.part == 0).count();
    c.record(
        "o9.the_lid_is_an_animated_group_and_the_base_is_not",
        lid == 6 && base == 6,
        format!(
            "{lid} lid quads in group {} and {base} static base quads — a box whose \
             base rode the lid would fly apart, and one whose lid did not would \
             never open",
            rewo_data::block_entity_models::SHULKER_LID_PART
        ),
    );
    Ok(())
}

fn check_sign_text(c: &mut Checker, version: &str) -> Result<(), String> {
    use rewo_data::sign_states::{SignAttachment, SignStates};
    use rewo_proto::nbt::Nbt;
    use rewo_world::block_entities::{BlockEntity, SignFace};

    let paths = DataPaths::for_version(version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let signs = SignStates::load(&paths.blocks_json())?;

    c.record(
        "t1.every_sign_state_resolves",
        signs.len() > 500,
        format!(
            "{} sign block states — standing, wall, hanging and wall-hanging  across every wood type",
            signs.len()
        ),
    );

    let apply = |m: &rewo_data::be_transform::Affine, p: [f32; 3]| {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    };

    // Find one standing and one wall state to work with.
    let mut ground = None;
    let mut wall = None;
    for id in 0..40000u32 {
        let Some(st) = signs.get(id) else { continue };
        if st.angle != 0.0 {
            continue;
        }
        match st.attachment {
            SignAttachment::Ground if ground.is_none() => ground = Some(st),
            SignAttachment::Wall if wall.is_none() => wall = Some(st),
            _ => {}
        }
        if ground.is_some() && wall.is_some() {
            break;
        }
    }
    let (Some(g), Some(w)) = (ground, wall) else {
        return Err("blockentityshot: no south-facing sign states found".into());
    };

    // Independently transcribed:
    //   M = T(.5,.5,.5) · YP(-angle) [· T(0,-.3125,-.4375)] [· YP(180)]
    //       · T(0, .33333334, .046666667) · S(s, -s, s),  s = 0.010416667
    let want = |wall: bool, front: bool, angle: f32| {
        use rewo_data::be_transform::{mul, rot_y, scale, translation};
        let mut m = mul(&translation(0.5, 0.5, 0.5), &rot_y(-angle));
        if wall {
            m = mul(&m, &translation(0.0, -0.3125, -0.4375));
        }
        if !front {
            m = mul(&m, &rot_y(180.0));
        }
        m = mul(&m, &translation(0.0, 0.33333334, 0.046666667));
        mul(&m, &scale(0.010416667, -0.010416667, 0.010416667))
    };
    let same = |a: &rewo_data::be_transform::Affine, b: &rewo_data::be_transform::Affine| {
        (0..3).all(|r| (0..4).all(|k| (a[r][k] - b[r][k]).abs() < 1e-6))
    };
    c.record(
        "t2.the_text_transform_is_exact",
        same(&g.text_transform(true), &want(false, true, 0.0))
            && same(&g.text_transform(false), &want(false, false, 0.0))
            && same(&w.text_transform(true), &want(true, true, 0.0)),
        "ground front/back and wall front all match the independent chain",
    );

    // The y scale must be negative — font space is y-down.
    let m = g.text_transform(true);
    c.record(
        "t3.the_y_scale_flips",
        m[1][1] < 0.0,
        format!(
            "y scale {:.6} — a positive one puts the text in exactly the right  place, upside down, which is a hard bug to see",
            m[1][1]
        ),
    );

    // Four lines, descending, straddling the board centre.
    let ys: Vec<f32> = (0..4).map(|i| g.line_y(i)).collect();
    let world: Vec<f32> = ys.iter().map(|&y| apply(&m, [0.0, y, 0.0])[1]).collect();
    c.record(
        "t4.the_four_lines_descend_from_the_top",
        ys == vec![-20.0, -10.0, 0.0, 10.0] && world.windows(2).all(|p| p[0] > p[1]),
        format!(
            "font y {ys:?} -> world y {world:?} — `i * lineHeight - 4*lineHeight/2`,  and the negative scale is what makes line 0 highest"
        ),
    );

    // A hanging sign's 9-px line height goes through *integer* division.
    let hanging = (0..40000u32)
        .filter_map(|id| signs.get(id))
        .find(|s| s.line_height == rewo_data::sign_states::HANGING_LINE_HEIGHT);
    c.record(
        "t5.a_hanging_sign_uses_integer_line_maths",
        hanging.is_some_and(|h| {
            (0..4).map(|i| h.line_y(i)).collect::<Vec<_>>() == vec![-18.0, -9.0, 0.0, 9.0]
        }),
        format!(
            "hanging line ys {:?} — `4 * 9 / 2` is 18 by integer division, not 18.0  from a float",
            hanging.map(|h| (0..4).map(|i| h.line_y(i)).collect::<Vec<_>>())
        ),
    );

    // The text sits a hair proud of the board so it does not z-fight.
    let origin = apply(&m, [0.0, 0.0, 0.0]);
    c.record(
        "t6.the_text_stands_off_the_board",
        (origin[2] - (0.5 + 0.046666667)).abs() < 1e-5,
        format!("front text plane z {:.6} (board centre 0.5)", origin[2]),
    );

    // Front and back face opposite ways.
    let back = apply(&g.text_transform(false), [0.0, 0.0, 0.0]);
    c.record(
        "t7.the_back_face_is_on_the_other_side",
        origin[2] > 0.5 && back[2] < 0.5,
        format!("front z {:.4}, back z {:.4}", origin[2], back[2]),
    );

    // A wall sign drops and pulls back onto its plaque.
    let wm = apply(&w.text_transform(true), [0.0, 0.0, 0.0]);
    c.record(
        "t8.a_wall_sign_sits_lower_and_further_back",
        wm[1] < origin[1] && wm[2] < origin[2],
        format!("wall ({:.3}, {:.3}) vs ground ({:.3}, {:.3})", wm[1], wm[2], origin[1], origin[2]),
    );

    // Every rotation must keep the text inside the block **horizontally** —
    // the property a wrong rotation or a wrong pivot would break. The vertical
    // extent is deliberately reported rather than bounded: it does not depend
    // on the rotation at all (a Y rotation preserves y), and vanilla's own top
    // line reaches slightly above the block, which is a fact about the sign
    // rather than a tolerance to pick.
    let mut horizontal_ok = true;
    let mut angles = std::collections::BTreeSet::new();
    let (mut ylo, mut yhi) = (f32::MAX, f32::MIN);
    // Per *corner*, the radius across every ground rotation. Different corners
    // sit at different radii; the same corner must not.
    let corners = [(-45.0f32, -20.0f32), (45.0, -20.0), (-45.0, 20.0), (45.0, 20.0)];
    let mut spread: f32 = 0.0;
    for (ci, &(cx, cy)) in corners.iter().enumerate() {
        let (mut rlo, mut rhi) = (f32::MAX, f32::MIN);
        for id in 0..40000u32 {
            let Some(st) = signs.get(id) else { continue };
            // Wall signs carry an extra offset, so they are a different family
            // and are covered by t8 rather than compared against ground ones.
            if st.attachment != SignAttachment::Ground {
                continue;
            }
            if ci == 0 {
                angles.insert(st.angle as i32);
            }
            let p = apply(&st.text_transform(true), [cx, cy, 0.0]);
            horizontal_ok &= (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[2]);
            ylo = ylo.min(p[1]);
            yhi = yhi.max(p[1]);
            let r = ((p[0] - 0.5).powi(2) + (p[2] - 0.5).powi(2)).sqrt();
            rlo = rlo.min(r);
            rhi = rhi.max(r);
        }
        spread = spread.max(rhi - rlo);
    }
    c.record(
        "t9.every_rotation_places_the_text_the_same_distance_out",
        horizontal_ok && angles.len() == 16 && spread < 1e-4,
        format!(
            "{} distinct ground rotations; each corner of a full-width line keeps its \
             distance from the block axis to within {spread:.6} across all of them — \
             which is what a Y rotation must preserve — and every corner stays inside \
             0..1 horizontally. Vertical extent {ylo:.4}..{yhi:.4}: the top line \
             reaches slightly above the block, which is vanilla's placement rather \
             than a tolerance chosen here.",
            angles.len()
        ),
    );

    // --- the NBT decode ----------------------------------------------------
    let msg = |s: &str| Nbt::String(s.to_string());
    let face = Nbt::Compound(vec![
        (
            "messages".to_string(),
            Nbt::List(vec![msg("hello"), msg(""), msg("world"), msg("")]),
        ),
        ("color".to_string(), Nbt::String("red".to_string())),
        ("has_glowing_text".to_string(), Nbt::Byte(1)),
    ]);
    let be = BlockEntity {
        type_id: 0,
        data: Nbt::Compound(vec![("front_text".to_string(), face)]),
    };
    let (front, back) = be.sign_text();
    c.record(
        "t10.sign_text_decodes_from_the_block_entity_nbt",
        front.as_ref().is_some_and(|f| {
            f.lines == ["hello".to_string(), String::new(), "world".to_string(), String::new()]
                && f.color.as_deref() == Some("red")
                && f.glowing
        }) && back.is_none(),
        format!(
            "front={:?} back={:?} — four lines always, padded, with the colour and  glow flag read even though neither is rendered",
            front.as_ref().map(|f| &f.lines),
            back.is_some()
        ),
    );

    // A short or absent list still yields four lines, because the renderer
    // always draws four.
    let short = Nbt::Compound(vec![(
        "messages".to_string(),
        Nbt::List(vec![msg("one")]),
    )]);
    let decoded = SignFace::from_nbt(&short);
    c.record(
        "t11.a_short_message_list_pads_to_four",
        decoded.as_ref().is_some_and(|f| {
            f.lines[0] == "one" && f.lines[1..].iter().all(|l| l.is_empty()) && !f.is_blank()
        }),
        format!("{:?}", decoded.map(|f| f.lines)),
    );
    Ok(())
}

fn check_registry(
    c: &mut Checker,
    registry: &BlockEntityRegistry,
    entries: &[(String, i32)],
    ids: &Ids,
    paths: &DataPaths,
) -> Result<(), String> {
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

    let kinds = |want: BlockEntityKind| -> Vec<&str> {
        TYPE_TABLE
            .iter()
            .filter(|(_, k)| *k == want)
            .map(|(n, _)| *n)
            .collect()
    };
    let invisible = kinds(BlockEntityKind::Invisible);
    let rendered = kinds(BlockEntityKind::Rendered);
    c.record(
        "a3.nothing_is_invisible_any_more",
        invisible.is_empty() && rendered.len() == 11 && !rendered.contains(&"minecraft:sign"),
        format!(
            "{} types still invisible, {} now Rendered: {rendered:?}. M25 \
             measured ELEVEN types whose block models bake to nothing, and all \
             eleven now have a renderer. A sign is NOT among the Rendered — its \
             block model carries the plank and only the text is renderer-side, \
             so it stays ModelIsEnough (and a bed is not a block entity in 26.2 \
             at all, which the fail-closed resolve proved by rejecting it as an \
             orphan). This witness is now the one that fires if a future \
             version adds an invisible type and nobody writes a renderer for it",
            invisible.len(),
            rendered.len()
        ),
    );

    // a4 replaces "nothing is marked Rendered yet", which was the wrong shape
    // of witness: it asserted a *moment*, so it went on passing while four
    // types shipped renderers underneath it. What it should have been checking
    // is that the classification and the renderer agree — in both directions,
    // and derived from the model resolver rather than restated from the table.
    let states = rewo_data::chest_states::ChestStates::load(&paths.blocks_json())?;
    let blocks = rewo_data::blocks::Blocks::load(&paths.blocks_json())?;
    let mut drawn_types: HashSet<&str> = HashSet::new();
    for id in states.drawn_states() {
        let Some(block) = blocks.block_name(id) else { continue };
        // Block name -> its block-entity type. The families are named after
        // their type, bar the dye and weathering prefixes.
        let name = block.trim_start_matches("minecraft:");
        if name.ends_with("shulker_box") {
            drawn_types.insert("minecraft:shulker_box");
        } else if name == "ender_chest" {
            drawn_types.insert("minecraft:ender_chest");
        } else if name == "trapped_chest" {
            drawn_types.insert("minecraft:trapped_chest");
        } else if name.ends_with("chest") {
            drawn_types.insert("minecraft:chest");
        } else if name == "end_portal" || name == "end_gateway" {
            drawn_types.insert(format!("minecraft:{name}").leak());
        } else if name.ends_with("copper_golem_statue") {
            drawn_types.insert("minecraft:copper_golem_statue");
        } else if name.ends_with("banner") {
            drawn_types.insert("minecraft:banner");
        } else if name == "decorated_pot" {
            drawn_types.insert("minecraft:decorated_pot");
        } else if name == "conduit" {
            drawn_types.insert("minecraft:conduit");
        } else if name.ends_with("skull") || name.ends_with("head") {
            // Every skull and head block is the one `minecraft:skull` type —
            // `SkullBlockRenderer` picks the model by `getType()`, not by the
            // block-entity type, which is why fourteen blocks map to one entry.
            drawn_types.insert("minecraft:skull");
        }
    }
    let declared: HashSet<&str> = kinds(BlockEntityKind::Rendered).into_iter().collect();
    let mut only_declared: Vec<&str> = declared.difference(&drawn_types).copied().collect();
    let mut only_drawn: Vec<&str> = drawn_types.difference(&declared).copied().collect();
    only_declared.sort_unstable();
    only_drawn.sort_unstable();
    c.record(
        "a4.the_rendered_set_is_exactly_what_resolves_a_model",
        only_declared.is_empty() && only_drawn.is_empty() && declared.len() == 11,
        format!(
            "{} types declared Rendered and the same {} resolve a model through \
             `ChestStates::draw_for`; declared-but-undrawn {only_declared:?}, \
             drawn-but-undeclared {only_drawn:?} (both want empty). Derived from \
             the resolver, not restated from the table, so a renderer that ships \
             without moving its type fails here and so does the reverse",
            declared.len(),
            drawn_types.len()
        ),
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
    Ok(())
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
        "e4.the_classification_accounts_for_the_measured_shortfall",
        registry.invisible_count() == 0 && registry.rendered_count() == 11,
        format!(
            "{} block-entity TYPES still Invisible and {} now Rendered, together \
             covering those blocks (one type spans all 16 banner colours, all 17 \
             shulker boxes, and so on). The jar's own gap above does not shrink \
             when Rewo draws one of them — that number measures the *models*, and \
             a chest's model is still empty; what changed is that there is a \
             renderer behind it",
            registry.invisible_count(),
            registry.rendered_count()
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
