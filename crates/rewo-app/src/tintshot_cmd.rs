//! `rewo tintshot` — serverless headless verification of the M14 per-biome
//! tint (the acceptance oracle).
//!
//! This gate exercises the **production** path end to end, not the pure
//! helpers: the real 26.2 jar asset bake (block → tint-source metadata +
//! texture layers), a retained biome `Container` installed through the
//! production `chunks_biomes` replacement (single / indirect / direct palettes),
//! the production `mesh_column`, and the production Vulkan terrain + sky/fog
//! draw with a pixel readback.
//!
//! The scene is deterministic and synthetic — a handful of explicit biomes with
//! DISTINCT primary override colors (grass=red, foliage=green, water=blue in
//! biome A; grass=cyan, water=yellow in biome B) and a synthetic colormap, so
//! every expectation is predictable WITHOUT calling the graded
//! `BiomeContext`/`block_tint`/`camera_sky` methods. Expectations are the
//! override constants themselves, an inline colormap index, oracle vectors for
//! the fiddle mean + grass modifiers derived from the decompiled 26.2 algorithm
//! and independently verified under Temurin 25 (see `BOUNDARY_EXACT`,
//! `DARK_FOREST_RED`, `SWAMP_LIGHT`/`SWAMP_DARK`), and a small independent
//! Gaussian reference routine (`ref_env`, shared by sky and fog).
//!
//! What a green run rejects (each has a dedicated check): constant-plains
//! output, swapped/transposed biome axes, nearest-neighbour or a wrong
//! radius/fiddle/25-sample mean at a biome boundary, a wrong dark_forest bit
//! formula or swamp noise threshold, using the block fiddle for the camera
//! sky/fog, a missing fog override or a fog override that fails to inherit the
//! dimension base, missing water tint, spruce/birch treated as foliage (they are
//! distinct constants), a GPU path that ignores the mesh tint color, and a
//! terrain fog path that ignores the fog-base uniform (or follows the sky).

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::assets::{self, BakedAssets, RenderKind, TintSource};
use rewo_data::{DataPaths, GameData};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{perspective_reverse_z, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_mesh::{mesh_column, MeshVertex};
use rewo_proto::reader::PacketReader;
use rewo_proto::writer::PacketWriter;
use rewo_world::biome::{
    argb, argb_blue, argb_green, argb_red, BiomeContext, BiomeDef, BiomeRegistry, Colormaps,
    GrassModifier,
};
use rewo_world::dimension::DimensionShape;
use rewo_world::World;

use crate::stats::OverlayRing;

const W: u32 = 128;
const H: u32 = 128;

// -- deterministic biome palette (distinct primaries → unambiguous ratios) ---
const A_GRASS: i32 = argb_c(255, 0, 0); // red
const A_FOLIAGE: i32 = argb_c(0, 255, 0); // green
const A_WATER: i32 = argb_c(0, 0, 255); // blue
const A_SKY: i32 = argb_c(255, 128, 0); // orange
const B_GRASS: i32 = argb_c(0, 255, 255); // cyan
const B_WATER: i32 = argb_c(255, 255, 0); // yellow
const B_SKY: i32 = argb_c(0, 128, 255);
const B_FOG: i32 = argb_c(255, 0, 128); // magenta — biome B's fog override
const DIM_SKY: i32 = argb_c(255, 255, 255);
const DIM_FOG: i32 = argb_c(90, 90, 90);
/// The synthetic grass colormap is uniform PURPLE except one probe index.
const CMAP_BG: i32 = argb_c(128, 0, 128);
/// Biome C (no override) samples the colormap at temp=0,downfall=0 → this cell.
const C_GRASS_CELL: i32 = argb_c(200, 100, 50);
/// `FoliageColor.FOLIAGE_EVERGREEN` — the spruce constant the metadata pins.
const SPRUCE_RGB: [u8; 3] = rgb_of(-10380959);
/// `FoliageColor.FOLIAGE_BIRCH` — the birch constant (distinct from spruce).
const BIRCH_RGB: [u8; 3] = rgb_of(-8345771);

// -- Independent oracle vectors, each the exact 25-sample integer mean derived
//    from the decompiled 26.2 algorithm (NOT observed from production). Ground
//    truth: `BiomeManager.getBiome` / `getFiddledDistance` (8-corner LCG fiddle),
//    `ClientLevel.calculateBlockTint` (the `(2r+1)²` mean), `GrassColorModifier`
//    (dark_forest bit formula / swamp threshold), `Biome.BIOME_INFO_NOISE`
//    (`BitRandomSource` + `SimplexNoise`, seed 2345), and `FoliageColor`. The
//    values were re-derived by an independent port run under Temurin 25 and agree
//    with production; the full derivation belongs in the M14 milestone docs. --
/// `calculateBlockTint(GRASS)` at seed=0, pos (8,8,5), layout qx<2→A(red) else
/// B(cyan) — the exact 5×5 BiomeManager-fiddle mean.
const BOUNDARY_EXACT: [u8; 3] = [91, 163, 163];
/// `GrassColorModifier.DARK_FOREST` on a RED (0xFF0000) base:
/// `opaque(((base&0xFEFEFE)+2634762)>>1)` = 0xFF931A05 (position-independent).
const DARK_FOREST_RED: [u8; 3] = [147, 26, 5];
/// `GrassColorModifier.SWAMP` radius-2 mean at (8,8,5): all 25 samples have
/// `BIOME_INFO_NOISE >= -0.1` → the LIGHT color (-9801671).
const SWAMP_LIGHT: [u8; 3] = [106, 112, 57];
/// `GrassColorModifier.SWAMP` radius-2 mean at (-40,8,-60): all 25 samples
/// `< -0.1` → the DARK color (-11766212).
const SWAMP_DARK: [u8; 3] = [76, 118, 60];

const fn argb_c(r: i32, g: i32, b: i32) -> i32 {
    (0xFFu32 << 24) as i32 | (r << 16) | (g << 8) | b
}

const fn rgb_of(argb: i32) -> [u8; 3] {
    [
        ((argb >> 16) & 0xFF) as u8,
        ((argb >> 8) & 0xFF) as u8,
        (argb & 0xFF) as u8,
    ]
}

#[derive(ClapArgs)]
pub struct TintshotArgs {
    /// Assert every property and exit nonzero on any failure.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Also dump the rendered scene PNGs here (eyeball artifacts).
    #[arg(long)]
    out_dir: Option<PathBuf>,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Bypass Vulkan validation. Otherwise the gate requests it and, under
    /// `--check`, fails if it does not activate.
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

/// One resolved block state we drive the scene with.
struct States {
    grass_block: u32,
    oak_leaves: u32,
    spruce_leaves: u32,
    birch_leaves: u32,
    water: u32,
    stone: u32,
}

pub fn run(args: TintshotArgs) -> Result<(), String> {
    let data = GameData::load_for_version(&args.version)?;
    let jar = jar_path(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let st = States {
        grass_block: data
            .blocks
            .default_state("minecraft:grass_block")
            .ok_or("no grass_block")?,
        oak_leaves: data
            .blocks
            .default_state("minecraft:oak_leaves")
            .ok_or("no oak_leaves")?,
        spruce_leaves: data
            .blocks
            .default_state("minecraft:spruce_leaves")
            .ok_or("no spruce_leaves")?,
        birch_leaves: data
            .blocks
            .default_state("minecraft:birch_leaves")
            .ok_or("no birch_leaves")?,
        water: data
            .blocks
            .default_state("minecraft:water")
            .ok_or("no water")?,
        stone: data
            .blocks
            .default_state("minecraft:stone")
            .ok_or("no stone")?,
    };

    let mut failures: Vec<String> = Vec::new();

    // -- CPU: production mesh_column tint (independent expectations) ----------
    cpu_checks(&data, &baked, &paths, &st, &mut failures);

    // -- Camera sky/fog: independent Gaussian reference ----------------------
    sky_fog_checks(&mut failures);

    // -- GPU: production Vulkan terrain + sky readback -----------------------
    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    let status = if gpu.validation_active {
        "ON"
    } else if args.no_validation {
        "off (--no-validation)"
    } else {
        "off (VK_LAYER_KHRONOS_validation unavailable)"
    };
    println!("[tintshot] Vulkan validation: {status}");
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "tintshot check: Vulkan validation requested but not active — \
             install the Vulkan SDK (VK_LAYER_KHRONOS_validation) or pass --no-validation"
                .into(),
        );
    }
    let gpu_result = gpu_checks(&mut gpu, &data, &baked, &st, &args, &mut failures);
    gpu.wait_idle();
    gpu_result?;

    if failures.is_empty() {
        println!(
            "TINTSHOT: OK — grass/foliage/water tint; spruce+birch constants; \
             dark_forest+swamp grass modifiers (exact bit/noise formulas); radius-2 \
             boundary = EXACT 5×5 fiddle mean; raw-vs-legacy; axis orientation; \
             tall-grass-below; camera sky (deep A/B + boundary Gaussian) + fog \
             (A inherits base, B overrides, boundary Gaussian); chunks_biomes \
             single/indirect/direct; GPU mesh tint (grass/water) + sky uniform + \
             terrain fog base — all verified (validation {status})"
        );
        Ok(())
    } else {
        eprintln!("TINTSHOT: FAIL — {} mismatch(es):", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        if args.check {
            Err(format!("tintshot: {} property failure(s)", failures.len()))
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Scene construction
// ---------------------------------------------------------------------------

fn shape() -> DimensionShape {
    DimensionShape {
        min_y: 0,
        height: 16,
    }
}

/// The deterministic biome registry: A/B with distinct overrides, C with none
/// (colormap path), + the dimension base sky/fog.
fn registry() -> BiomeRegistry {
    let a = BiomeDef {
        music_volume: None,
        name: "test:a".into(),
        temperature: 0.5,
        downfall: 0.5,
        water_color: A_WATER,
        grass_override: Some(A_GRASS),
        foliage_override: Some(A_FOLIAGE),
        dry_foliage_override: None,
        grass_modifier: GrassModifier::None,
        sky_color: Some(A_SKY),
        fog_color: None,
            has_precipitation: true,
        temperature_modifier: Default::default(),
        ambient_sounds: None,
        background_music: None,
    };
    let b = BiomeDef {
        name: "test:b".into(),
        water_color: B_WATER,
        grass_override: Some(B_GRASS),
        sky_color: Some(B_SKY),
        fog_color: Some(B_FOG), // B overrides fog (A inherits the dimension base)
        ..a.clone()
    };
    let c = BiomeDef {
        name: "test:c".into(),
        // No overrides → colormap path. temp=0,downfall=0 → colormap cell 65535.
        temperature: 0.0,
        downfall: 0.0,
        grass_override: None,
        foliage_override: None,
        sky_color: None,
        ..a.clone()
    };
    let mut reg = BiomeRegistry::new(vec![a, b, c]);
    reg.dimension_sky = Some(DIM_SKY);
    reg.dimension_fog = Some(DIM_FOG);
    reg
}

/// Synthetic colormaps: grass uniform PURPLE except the biome-C probe cell.
fn colormaps() -> Colormaps {
    let mut grass = vec![CMAP_BG; 65536];
    grass[65535] = C_GRASS_CELL; // temp=0,downfall=0 lookup
    let foliage = vec![CMAP_BG; 65536];
    let dry = vec![CMAP_BG; 65536];
    Colormaps::from_pixels(grass, foliage, dry)
}

fn context() -> BiomeContext {
    // Seed 0 — deterministic fiddle. The registry + colormaps are synthetic.
    BiomeContext::new(std::sync::Arc::new(registry()), colormaps(), 0)
}

/// Build one section's biome container bytes as a DIRECT 7-bit palette (the
/// real 26.2 width), from a 64-cell array.
fn direct_section_7bit(cells: &[u32; 64]) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.u8(7); // bits > 3 → direct/global strategy
    let vpl = 64 / 7; // values per long at 7 bits
    for chunk in cells.chunks(vpl) {
        let mut word: u64 = 0;
        for (i, &v) in chunk.iter().enumerate() {
            word |= (v as u64) << (i as u32 * 7);
        }
        w.raw(&word.to_be_bytes());
    }
    w.buf
}

/// A single-value biome section (bits=0) of `id`.
fn single_section(id: u32) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.u8(0).varint(id as i32);
    w.buf
}

/// An indirect (bits=1) biome section over palette [p0, p1]; `cells` are palette
/// indices.
fn indirect_section(p0: u32, p1: u32, cells: &[u32; 64]) -> Vec<u8> {
    let mut w = PacketWriter::default();
    w.u8(1).varint(2).varint(p0 as i32).varint(p1 as i32);
    for chunk in cells.chunks(64) {
        let mut word: u64 = 0;
        for (i, &v) in chunk.iter().enumerate() {
            word |= (v as u64) << i;
        }
        w.raw(&word.to_be_bytes());
    }
    w.buf
}

fn bidx(qx: usize, qy: usize, qz: usize) -> usize {
    (qy << 2 | qz) << 2 | qx
}

/// Install a biome layout into a whole chunk NEIGHBORHOOD via the production
/// `chunks_biomes` path (DIRECT 7-bit palette), so every sampling window hits a
/// LOADED chunk — an unloaded chunk falls back to biome 0 and would contaminate
/// the average. `biome_of` maps a **global** quart-x to a biome id (the layout
/// is uniform in y/z). Loads columns cx,cz ∈ [lo, hi].
fn install_biomes(world: &mut World, lo: i32, hi: i32, biome_of: impl Fn(i32, i32) -> u16) {
    for cz in lo..=hi {
        for cx in lo..=hi {
            world.ensure_column(cx, cz);
            let mut cells = [0u32; 64];
            for qy in 0..4 {
                for qz in 0..4 {
                    for qx in 0..4 {
                        let gqx = cx * 4 + qx as i32;
                        let gqz = cz * 4 + qz as i32;
                        cells[bidx(qx, qy, qz)] = biome_of(gqx, gqz) as u32;
                    }
                }
            }
            let buf = direct_section_7bit(&cells);
            let mut r = PacketReader::new(&buf);
            let containers = rewo_world::chunk::read_chunk_biomes(&mut r, &shape(), 7).unwrap();
            world.apply_chunks_biomes(cx, cz, containers);
        }
    }
}

/// Tint scene A/B boundary at global quart 2 (block x=8): region A = x<8,
/// region B = x>=8, uniform in z. Narrow — the 5x5 tint window is small, so a
/// chunk-0 block with loaded chunks [-1,1] never reaches unloaded space.
fn biome_tint_layout(gqx: i32, _gqz: i32) -> u16 {
    if gqx < 2 {
        0
    } else {
        1
    }
}

/// Sky scene A/B boundary at global quart 8 (block x=32): the 6³ Gaussian window
/// is ±3 quarts, so the regions must be wider (loaded chunks [-2,4]).
fn biome_sky_layout(gqx: i32, _gqz: i32) -> u16 {
    if gqx < 8 {
        0
    } else {
        1
    }
}

// ---------------------------------------------------------------------------
// CPU checks — production mesh_column, independent expectations
// ---------------------------------------------------------------------------

fn cpu_checks(
    data: &GameData,
    baked: &BakedAssets,
    paths: &DataPaths,
    st: &States,
    failures: &mut Vec<String>,
) {
    // Blocks at y=8, air above (top faces mesh). Region A is x<8, region B x>=8
    // (boundary quart 2). Region-A blocks sit at x=2 (window x∈[0,4], all in the
    // loaded region-A chunks); region B at x=13; boundary at x=8.
    let mut base = World::new(shape());
    base.ensure_column(0, 0);
    base.set_block(2, 8, 2, st.grass_block); // region A
    base.set_block(2, 8, 5, st.oak_leaves); // region A foliage
    base.set_block(2, 8, 8, st.spruce_leaves); // region A constant leaves (evergreen)
    base.set_block(5, 8, 5, st.birch_leaves); // region A constant leaves (birch)
    base.set_block(2, 8, 11, st.water); // region A water
    base.set_block(2, 8, 14, st.stone); // region A untinted control
    base.set_block(13, 8, 8, st.grass_block); // region B
    base.set_block(8, 8, 5, st.grass_block); // A/B boundary at x=8

    // Legacy (no biome context) mesh of chunk 0: pre-tinted layers, white.
    let legacy = mesh_column(&base, &baked.render, &baked.models, &baked.fluid, 0, 0).expect("legacy mesh");

    // Biome-aware mesh: install the A/B layout (loaded chunks [-1,1]) + context.
    let mut world = base;
    install_biomes(&mut world, -1, 1, biome_tint_layout);
    world.set_biome_context(std::sync::Arc::new(context()));
    let mesh = mesh_column(&world, &baked.render, &baked.models, &baked.fluid, 0, 0).expect("biome mesh");

    // -- grass_block in A → RED, in B → CYAN (distinct primaries) -----------
    check_tint_ratio(failures, "grass_A", &mesh, 2, 8, 2, A_GRASS);
    check_tint_ratio(failures, "grass_B", &mesh, 13, 8, 8, B_GRASS);
    // -- oak_leaves → GREEN (foliage resolver, not grass) -------------------
    check_tint_ratio(failures, "foliage_A", &mesh, 2, 8, 5, A_FOLIAGE);
    // -- water → BLUE (water resolver; missing-water-tint check) -------------
    check_tint_ratio(failures, "water_A", &mesh, 2, 8, 11, A_WATER);
    // -- spruce_leaves → the EVERGREEN CONSTANT (97,153,97), not foliage green.
    //    Constant tint is biome-independent; the top face pins it exactly. -----
    check_exact_tint(failures, "spruce", &mesh, 2, 8, 8, SPRUCE_RGB);
    // -- birch_leaves → the BIRCH CONSTANT (128,167,85), distinct from spruce. -
    check_exact_tint(failures, "birch", &mesh, 5, 8, 5, BIRCH_RGB);
    // -- stone → NO tinted vertices (reject "everything gets tinted") --------
    if !tinted_verts(&mesh, 2, 8, 14).is_empty() {
        failures.push("stone control: an untinted block produced tinted vertices".into());
    } else {
        println!("[tintshot] stone control: no tint (correct)");
    }
    // -- indirect biome palette (production chunks_biomes decode) ------------
    check_indirect_palette(failures, baked, st);

    // -- radius-2 average (reject nearest-neighbour): boundary grass equals the
    //    EXACT 5×5 fiddle mean of RED (A) and CYAN (B), independently derived. -
    check_exact_tint(failures, "boundary", &mesh, 8, 8, 5, BOUNDARY_EXACT);

    // -- grass MODIFIERS through the production mesh (dedicated single-biome
    //    contexts). dark_forest: exact bit formula; swamp: exact noise/threshold
    //    on BOTH branches. --------------------------------------------------
    check_dark_forest(failures, baked, st);
    check_swamp(failures, baked, st);

    // -- raw-vs-legacy: tinted grass face uses the RAW layer under a biome
    //    context, the PRE-TINTED layer with none. ----------------------------
    check_raw_vs_legacy(failures, data, baked, &legacy, &mesh, st, 2, 8, 2);

    // -- axis orientation: swap the biome layout to vary by Z, and confirm a
    //    block reads its Z-region biome (not X). ------------------------------
    check_axis_orientation(failures, baked, st);

    // -- colormap path: biome C grass (no override) samples the colormap cell,
    //    computed with an inline index (not colormap_get). -------------------
    check_colormap_path(failures, baked, st);

    // -- tall-grass UPPER metadata: the baked upper state carries GrassBelow. -
    check_tall_grass_below(failures, paths, baked);
}

/// Every tinted vertex of the block at (x,y,z) must have the expected color
/// ratio (the override's r:g:b, since `color = shade·AO·override/255`). The
/// scalar shade·AO cancels — we compare each channel to `k·expected/255`.
fn check_tint_ratio(
    failures: &mut Vec<String>,
    name: &str,
    mesh: &rewo_mesh::ColumnMesh,
    x: i32,
    y: i32,
    z: i32,
    expect_argb: i32,
) {
    let verts = tinted_verts(mesh, x, y, z);
    if verts.is_empty() {
        failures.push(format!("{name}: no tinted vertices at ({x},{y},{z})"));
        return;
    }
    let exp = [
        argb_red(expect_argb) as f32 / 255.0,
        argb_green(expect_argb) as f32 / 255.0,
        argb_blue(expect_argb) as f32 / 255.0,
    ];
    // The color is `shade·AO · exp` (a scalar × the override). Recover the
    // scalar `k` from the expectation's DOMINANT channel, then require every
    // channel to match `k·exp` — this pins the r:g:b ratio and rejects
    // constant-plains (a fixed grass-green, not our red/cyan/etc.).
    let dom = (0..3).max_by(|&a, &b| exp[a].total_cmp(&exp[b])).unwrap();
    for v in &verts {
        let k = v.reconstructed_color()[dom] / exp[dom].max(1e-6);
        let pred = [exp[0] * k, exp[1] * k, exp[2] * k];
        let ok = (0..3).all(|c| approx(v.reconstructed_color()[c], pred[c]));
        if !ok {
            failures.push(format!(
                "{name}: vertex color {:?} not the {expect_argb:#08x} ratio (k={k:.3} → {pred:?})",
                v.reconstructed_color(),
            ));
            return;
        }
    }
    println!(
        "[tintshot] {name}: {} tinted verts, ratio matches {:#06x}",
        verts.len(),
        expect_argb & 0xFFFFFF
    );
}

/// Pin a block's tint to an EXACT `[r,g,b]`, independently derived. The unshaded
/// top face (dir=up, shade 1.0; model quads carry no AO and an isolated cube's
/// vertices all have AO 1.0) equals `expect/255` exactly — a sub-1/255 tolerance
/// so any ≥1-unit error fails. Every other tinted vertex must be a scalar
/// face-shade multiple of the SAME vector (rejects a per-vertex arbitrary blend).
/// Used for constants (spruce/birch), the fiddle mean (boundary), and the grass
/// modifiers (dark_forest / swamp), each with its own pinned vector + derivation.
fn check_exact_tint(
    failures: &mut Vec<String>,
    name: &str,
    mesh: &rewo_mesh::ColumnMesh,
    x: i32,
    y: i32,
    z: i32,
    expect: [u8; 3],
) {
    let verts = tinted_verts(mesh, x, y, z);
    if verts.is_empty() {
        failures.push(format!("{name}: no tinted vertices at ({x},{y},{z})"));
        return;
    }
    let exp = [
        expect[0] as f32 / 255.0,
        expect[1] as f32 / 255.0,
        expect[2] as f32 / 255.0,
    ];
    let top = verts
        .iter()
        .max_by(|a, b| {
            let (sa, sb): (f32, f32) = (
                a.reconstructed_color().iter().sum(),
                b.reconstructed_color().iter().sum(),
            );
            sa.total_cmp(&sb)
        })
        .unwrap();
    if (0..3).any(|c| (top.reconstructed_color()[c] - exp[c]).abs() > 0.5 / 255.0) {
        failures.push(format!(
            "{name}: top-face tint {:?} != exact {expect:?}/255 ({exp:?})",
            top.reconstructed_color()
        ));
        return;
    }
    let dom = (0..3).max_by(|&a, &b| exp[a].total_cmp(&exp[b])).unwrap();
    for v in &verts {
        let k = v.reconstructed_color()[dom] / exp[dom].max(1e-6);
        if (0..3).any(|c| !approx(v.reconstructed_color()[c], exp[c] * k)) {
            failures.push(format!(
                "{name}: vertex {:?} is not a face-shade of {expect:?} (k={k:.3})",
                v.reconstructed_color()
            ));
            return;
        }
    }
    println!(
        "[tintshot] {name}: exact tint {expect:?}/255 (top face {:?}); {} tinted verts, all shades",
        top.reconstructed_color(),
        verts.len()
    );
}

/// A dedicated single-biome context (only `def` at index 0), for the grass
/// modifiers. With one biome, even the unloaded-quart fallback (biome 0) is that
/// biome, so no sampling window can be contaminated.
fn single_biome_context(def: BiomeDef) -> BiomeContext {
    BiomeContext::new(
        std::sync::Arc::new(BiomeRegistry::new(vec![def])),
        colormaps(),
        0,
    )
}

/// Install a single-value biome-0 container over the 3×3 columns around (cx,cz)
/// via the production `chunks_biomes` decode, and load them.
fn install_single_biome(world: &mut World, cx: i32, cz: i32) {
    for dz in -1..=1 {
        for dx in -1..=1 {
            world.ensure_column(cx + dx, cz + dz);
            let buf = single_section(0);
            let mut r = PacketReader::new(&buf);
            let containers = rewo_world::chunk::read_chunk_biomes(&mut r, &shape(), 7).unwrap();
            world.apply_chunks_biomes(cx + dx, cz + dz, containers);
        }
    }
}

/// `dark_forest` grass modifier through the production mesh: with a RED
/// (0xFF0000) base and `GrassColorModifier.DARK_FOREST`, the modifier is
/// position-independent — `opaque(((base & 0xFEFEFE) + 2634762) >> 1)` = 0xFF931A05
/// — so the radius-2 mean of 25 identical samples is exactly [147,26,5]
/// (temurin-25 verified). A wrong bit formula (or applying the base unmodified,
/// which would be [255,0,0]) fails.
fn check_dark_forest(failures: &mut Vec<String>, baked: &BakedAssets, st: &States) {
    let def = BiomeDef {
        music_volume: None,
        name: "test:dark_forest".into(),
        temperature: 0.5,
        downfall: 0.5,
        water_color: A_WATER,
        grass_override: Some(A_GRASS), // RED base
        foliage_override: Some(A_FOLIAGE),
        dry_foliage_override: None,
        grass_modifier: GrassModifier::DarkForest,
        sky_color: None,
        fog_color: None,
            has_precipitation: true,
        temperature_modifier: Default::default(),
        ambient_sounds: None,
        background_music: None,
    };
    let mut world = World::new(shape());
    install_single_biome(&mut world, 0, 0);
    world.set_block(2, 8, 2, st.grass_block);
    world.set_biome_context(std::sync::Arc::new(single_biome_context(def)));
    let mesh = mesh_column(&world, &baked.render, &baked.models, &baked.fluid, 0, 0).expect("dark_forest mesh");
    check_exact_tint(failures, "dark_forest", &mesh, 2, 8, 2, DARK_FOREST_RED);
}

/// `swamp` grass modifier through the production mesh, BOTH branches. The
/// modifier ignores the base and returns `BIOME_INFO_NOISE(x·0.0225,z·0.0225) <
/// -0.1 ? DARK(-11766212) : LIGHT(-9801671)` per sample; the radius-2 mean of 25
/// samples then depends on the noise. temurin-25 pins two all-one-branch windows:
/// (8,5) → all LIGHT → [106,112,57]; (-40,-60) → all DARK → [76,118,60]. A wrong
/// threshold, noise, or seed shifts the branch counts and the mean.
fn check_swamp(failures: &mut Vec<String>, baked: &BakedAssets, st: &States) {
    let def = BiomeDef {
        music_volume: None,
        name: "test:swamp".into(),
        temperature: 0.5,
        downfall: 0.5,
        water_color: A_WATER,
        grass_override: Some(A_GRASS), // base is irrelevant — the modifier ignores it
        foliage_override: Some(A_FOLIAGE),
        dry_foliage_override: None,
        grass_modifier: GrassModifier::Swamp,
        sky_color: None,
        fog_color: None,
            has_precipitation: true,
        temperature_modifier: Default::default(),
        ambient_sounds: None,
        background_music: None,
    };
    check_swamp_at(failures, baked, st, &def, 8, 5, SWAMP_LIGHT, "swamp_light");
    check_swamp_at(
        failures,
        baked,
        st,
        &def,
        -40,
        -60,
        SWAMP_DARK,
        "swamp_dark",
    );
}

#[allow(clippy::too_many_arguments)]
fn check_swamp_at(
    failures: &mut Vec<String>,
    baked: &BakedAssets,
    st: &States,
    def: &BiomeDef,
    bx: i32,
    bz: i32,
    expect: [u8; 3],
    name: &str,
) {
    let (cx, cz) = (bx >> 4, bz >> 4);
    let mut world = World::new(shape());
    install_single_biome(&mut world, cx, cz);
    world.set_block(bx, 8, bz, st.grass_block);
    world.set_biome_context(std::sync::Arc::new(single_biome_context(def.clone())));
    let mesh = mesh_column(&world, &baked.render, &baked.models, &baked.fluid, cx, cz).expect("swamp mesh");
    check_exact_tint(failures, name, &mesh, bx, 8, bz, expect);
}

fn check_raw_vs_legacy(
    failures: &mut Vec<String>,
    _data: &GameData,
    baked: &BakedAssets,
    legacy: &rewo_mesh::ColumnMesh,
    biome: &rewo_mesh::ColumnMesh,
    st: &States,
    x: i32,
    y: i32,
    z: i32,
) {
    // The grass_block model's tinted top quad: `layer` = pre-tinted,
    // `raw_layer` = raw. Find both from the bake.
    let (pretint, raw) = match &baked.render[st.grass_block as usize] {
        RenderKind::Model(idx) => {
            let q = baked.models[*idx as usize]
                .iter()
                .find(|q| q.tint != TintSource::None);
            match q {
                Some(q) => (q.layer, q.raw_layer),
                None => {
                    failures.push("raw-vs-legacy: grass_block has no tinted quad".into());
                    return;
                }
            }
        }
        _ => {
            failures.push("raw-vs-legacy: grass_block is not a Model".into());
            return;
        }
    };
    if pretint == raw {
        failures
            .push("raw-vs-legacy: pre-tinted and raw layers are identical (no raw variant)".into());
        return;
    }
    // Legacy path uses the PRE-TINTED layer + white; biome path uses RAW + tint.
    let legacy_top = top_face_layers(legacy, x, y, z);
    let biome_top = top_face_layers(biome, x, y, z);
    if !legacy_top.contains(&pretint) {
        failures.push(format!(
            "raw-vs-legacy: legacy top-face layers {legacy_top:?} do not include the pre-tinted {pretint}"
        ));
    }
    if !biome_top.contains(&raw) {
        failures.push(format!(
            "raw-vs-legacy: biome top-face layers {biome_top:?} do not include the raw {raw}"
        ));
    }
    // And the legacy color is white (untinted), biome is tinted.
    if let Some(v) = tinted_verts(legacy, x, y, z).first() {
        failures.push(format!(
            "raw-vs-legacy: legacy path is tinted ({:?}), expected white",
            v.reconstructed_color()
        ));
    }
    println!("[tintshot] raw-vs-legacy: legacy→layer {pretint} white, biome→layer {raw} tinted");
}

/// Axis orientation: a layout that varies by Z (not X) must make a block read
/// its Z-region biome. A transposed container read would return the X-region's.
fn check_axis_orientation(failures: &mut Vec<String>, baked: &BakedAssets, st: &States) {
    let mut world = World::new(shape());
    world.ensure_column(0, 0);
    // Block at x-quart 0 (region A if the layout were by X), z-quart 3.
    world.set_block(2, 8, 13, st.grass_block);
    // Layout varies by Z only: gqz<2 → A(0), else B(1). If the container read
    // transposed x/z, a block at x-quart 0, z-quart 3 would read A (red);
    // correct is B (cyan).
    install_biomes(&mut world, -1, 1, |_gqx, gqz| if gqz < 2 { 0 } else { 1 });
    world.set_biome_context(std::sync::Arc::new(context()));
    let mesh = mesh_column(&world, &baked.render, &baked.models, &baked.fluid, 0, 0).expect("axis mesh");
    check_tint_ratio(failures, "axis_z", &mesh, 2, 8, 13, B_GRASS);
}

/// Reinstall the A/B layout as an INDIRECT (bits=1) biome palette [A, B] and
/// confirm the grass tint still resolves per region — exercising the indirect
/// palette decode in the production `chunks_biomes` path.
fn check_indirect_palette(failures: &mut Vec<String>, baked: &BakedAssets, st: &States) {
    let mut world = World::new(shape());
    world.ensure_column(0, 0);
    world.set_block(2, 8, 2, st.grass_block); // region A → red
    world.set_block(13, 8, 8, st.grass_block); // region B → cyan
                                               // Loaded neighborhood with the A/B-by-x layout (direct); then OVERWRITE
                                               // chunk 0's biomes with an INDIRECT (bits=1) palette [A, B] so the indirect
                                               // decode is what drives chunk 0's tint.
    install_biomes(&mut world, -1, 1, biome_tint_layout);
    let mut cells = [0u32; 64];
    for qy in 0..4 {
        for qz in 0..4 {
            for qx in 0..4 {
                cells[bidx(qx, qy, qz)] = if qx < 2 { 0 } else { 1 };
            }
        }
    }
    let buf = indirect_section(0, 1, &cells);
    let mut r = PacketReader::new(&buf);
    let containers = rewo_world::chunk::read_chunk_biomes(&mut r, &shape(), 7).unwrap();
    world.apply_chunks_biomes(0, 0, containers);
    world.set_biome_context(std::sync::Arc::new(context()));
    let mesh = mesh_column(&world, &baked.render, &baked.models, &baked.fluid, 0, 0).expect("indirect mesh");
    check_tint_ratio(failures, "indirect_A", &mesh, 2, 8, 2, A_GRASS);
    check_tint_ratio(failures, "indirect_B", &mesh, 13, 8, 8, B_GRASS);
}

/// Biome C grass (no override) must be the colormap cell for temp=0,downfall=0,
/// which we placed at index 65535 = `C_GRASS_CELL`. The expected value is read
/// from our own array with an inline index, NOT via `colormap_get`.
fn check_colormap_path(failures: &mut Vec<String>, baked: &BakedAssets, st: &States) {
    let mut world = World::new(shape());
    world.ensure_column(0, 0);
    world.set_block(2, 8, 2, st.grass_block);
    // Single-VALUE biome C (bits=0) across a loaded neighborhood — covers the
    // single-palette decode and keeps the whole window on biome C.
    for cz in -1..=1 {
        for cx in -1..=1 {
            world.ensure_column(cx, cz);
            let buf = single_section(2);
            let mut r = PacketReader::new(&buf);
            let containers = rewo_world::chunk::read_chunk_biomes(&mut r, &shape(), 7).unwrap();
            world.apply_chunks_biomes(cx, cz, containers);
        }
    }
    world.set_biome_context(std::sync::Arc::new(context()));
    let mesh = mesh_column(&world, &baked.render, &baked.models, &baked.fluid, 0, 0).expect("cmap mesh");
    // Independent index: temp=0,downfall=0 → x=255,y=255 → 65535 → C_GRASS_CELL.
    let expect = {
        let (temp, downfall) = (0.0f64, 0.0f64);
        let rain = downfall * temp;
        let x = ((1.0 - temp) * 255.0) as i32;
        let yy = ((1.0 - rain) * 255.0) as i32;
        let index = ((yy << 8) | x) as usize;
        assert_eq!(index, 65535);
        C_GRASS_CELL
    };
    check_tint_ratio(failures, "colormap_C", &mesh, 2, 8, 2, expect);
}

/// The baked tall-grass UPPER state must carry `TintSource::GrassBelow`
/// (samples pos.below()); the LOWER carries plain `Grass`. Reads the
/// command-selected `blocks.json` (the block table doesn't expose a name→half
/// lookup, so we resolve the two half states from the report directly).
fn check_tall_grass_below(failures: &mut Vec<String>, paths: &DataPaths, baked: &BakedAssets) {
    let mut upper = None;
    let mut lower = None;
    let bj: serde_json::Value = match std::fs::read(paths.blocks_json()) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    };
    if let Some(states) = bj["minecraft:tall_grass"]["states"].as_array() {
        for s in states {
            let id = s["id"].as_u64().unwrap_or(0) as usize;
            let half = s["properties"]["half"].as_str().unwrap_or("");
            match half {
                "upper" => upper = Some(id),
                "lower" => lower = Some(id),
                _ => {}
            }
        }
    }
    let tints = |id: usize| -> Vec<TintSource> {
        match baked.render.get(id) {
            Some(RenderKind::Model(idx)) => baked.models[*idx as usize]
                .iter()
                .map(|q| q.tint)
                .filter(|t| *t != TintSource::None)
                .collect(),
            _ => vec![],
        }
    };
    match (upper, lower) {
        (Some(u), Some(l)) => {
            let ut = tints(u);
            let lt = tints(l);
            if !ut.iter().any(|t| *t == TintSource::GrassBelow) {
                failures.push(format!(
                    "tall_grass upper (state {u}) tints {ut:?} lack GrassBelow"
                ));
            }
            if !lt.iter().any(|t| *t == TintSource::Grass) {
                failures.push(format!(
                    "tall_grass lower (state {l}) tints {lt:?} lack Grass"
                ));
            }
            if ut.iter().any(|t| *t == TintSource::GrassBelow)
                && lt.iter().any(|t| *t == TintSource::Grass)
            {
                println!("[tintshot] tall_grass: upper=GrassBelow, lower=Grass");
            }
        }
        _ => failures.push("tall_grass: could not find upper/lower states".into()),
    }
}

// ---------------------------------------------------------------------------
// Camera sky/fog — independent Gaussian reference (reject fiddle-for-sky)
// ---------------------------------------------------------------------------

fn sky_fog_checks(failures: &mut Vec<String>) {
    // Wide A/B layout (boundary x=32) so a deep eye's 6³ window is pure. Loaded
    // chunks [-2,4] cover x/z ∈ [-32, 79] — every sky window hits a loaded chunk.
    let mut world = World::new(shape());
    install_biomes(&mut world, -2, 4, biome_sky_layout);
    world.set_biome_context(std::sync::Arc::new(context()));

    // Eye deep in A → sky = A_SKY exactly, fog = dimension base (A has no fog
    // override). No fiddle can perturb a uniform neighbourhood, and a
    // constant-sky bug would give the wrong biome.
    let eye_a = [8.0, 8.0, 8.0]; // x=8 → quart 2, window quarts -1..4 all < 8 = A
    let sky_a = world.camera_sky(eye_a).expect("camera sky A");
    if sky_a != A_SKY {
        failures.push(format!(
            "camera sky (deep A): {sky_a:#08x} != A_SKY {A_SKY:#08x}"
        ));
    }
    let fog_a = world.camera_fog(eye_a).expect("camera fog A");
    if fog_a != DIM_FOG {
        failures.push(format!(
            "camera fog (deep A): {fog_a:#08x} != dimension base {DIM_FOG:#08x} (fog override handling)"
        ));
    }

    // Eye deep in B → sky = B_SKY, fog = B_FOG. Proves both are biome-driven,
    // not constant, and that B's fog OVERRIDE wins over the dimension base.
    let eye_b = [48.0, 8.0, 8.0]; // x=48 → quart 12, window quarts 9..14 all >= 8 = B
    let sky_b = world.camera_sky(eye_b).expect("camera sky B");
    if sky_b != B_SKY {
        failures.push(format!(
            "camera sky (deep B): {sky_b:#08x} != B_SKY {B_SKY:#08x}"
        ));
    }
    let fog_b = world.camera_fog(eye_b).expect("camera fog B");
    if fog_b != B_FOG {
        failures.push(format!(
            "camera fog (deep B): {fog_b:#08x} != B_FOG {B_FOG:#08x} (biome fog override not applied)"
        ));
    }

    // The per-biome attribute value: its override, or the dimension base if it
    // has none. A overrides sky but NOT fog (inherits the base); B overrides both.
    let sky_value = |b: u16| match b {
        0 => A_SKY,
        1 => B_SKY,
        _ => DIM_SKY,
    };
    let fog_value = |b: u16| match b {
        0 => DIM_FOG, // A has no fog override → inherits the dimension base
        1 => B_FOG,
        _ => DIM_FOG,
    };

    // Boundary eye: the 6³ Gaussian straddles A and B → a blend. Compare each of
    // sky/fog to an INDEPENDENT raw-quart Gaussian reference. If camera_* used
    // the block fiddle (or the wrong kernel/lerp/inheritance), it would diverge.
    let eye_bnd = [32.0, 8.0, 8.0]; // x=32 = the A/B quart boundary
    let sky_bnd = world.camera_sky(eye_bnd).expect("camera sky boundary");
    let sky_ref = ref_env(eye_bnd, DIM_SKY, sky_value);
    if sky_bnd != sky_ref {
        failures.push(format!(
            "camera sky (boundary): {sky_bnd:#08x} != independent raw-quart Gaussian {sky_ref:#08x}"
        ));
    } else if sky_bnd == A_SKY || sky_bnd == B_SKY {
        failures.push(
            "camera sky (boundary) equals a single biome — no blend, check is vacuous".into(),
        );
    } else {
        println!("[tintshot] camera sky: deep-A=A_SKY, deep-B=B_SKY, boundary={sky_bnd:#08x} matches raw-quart Gaussian");
    }

    // Fog boundary: blends A's inherited dimension base (DIM_FOG) with B's fog
    // OVERRIDE (B_FOG) through the same probe. Proves inheritance + override +
    // Gaussian all interact correctly (a raw-quart reference, not the fiddle).
    let fog_bnd = world.camera_fog(eye_bnd).expect("camera fog boundary");
    let fog_ref = ref_env(eye_bnd, DIM_FOG, fog_value);
    if fog_bnd != fog_ref {
        failures.push(format!(
            "camera fog (boundary): {fog_bnd:#08x} != independent raw-quart Gaussian {fog_ref:#08x}"
        ));
    } else if fog_bnd == DIM_FOG || fog_bnd == B_FOG {
        failures.push(
            "camera fog (boundary) equals a single value — no blend, check is vacuous".into(),
        );
    } else {
        println!("[tintshot] camera fog: deep-A=DIM_FOG (inherit), deep-B=B_FOG (override), boundary={fog_bnd:#08x} matches raw-quart Gaussian");
    }
}

/// Independent re-implementation of the camera environment Gaussian (sky OR
/// fog): raw-quart biome sampling (`biome_sky_layout`), the `[0,1,4,6,4,1,0]`
/// kernel, groups by biome, weighted incremental `srgbLerp`. `value_of` maps a
/// biome id to its resolved attribute — its override, or `base` when it has none
/// (mirrors `applyModifier` returning the base value for a missing entry).
/// Deliberately separate from `camera_attr` — a re-implementation, so a bug
/// there diverges from here.
fn ref_env(eye: [f64; 3], base: i32, value_of: impl Fn(u16) -> i32) -> i32 {
    const KERNEL: [f64; 7] = [0.0, 1.0, 4.0, 6.0, 4.0, 1.0, 0.0];
    let p = [
        eye[0] * 0.25 - 0.5,
        eye[1] * 0.25 - 0.5,
        eye[2] * 0.25 - 0.5,
    ];
    let (ix, iy, iz) = (
        p[0].floor() as i32,
        p[1].floor() as i32,
        p[2].floor() as i32,
    );
    let (rx, ry, rz) = (p[0] - ix as f64, p[1] - iy as f64, p[2] - iz as f64);
    let weight = |rel: f64, k: usize| KERNEL[k + 1] + rel * (KERNEL[k] - KERNEL[k + 1]);
    // Insertion-ordered groups by biome id (matches Reference2DoubleArrayMap).
    let mut order: Vec<(u16, f64)> = Vec::new();
    for zc in 0..6 {
        let wz = weight(rz, zc);
        let sz = iz - 2 + zc as i32;
        for xc in 0..6 {
            let wx = weight(rx, xc);
            let sx = ix - 2 + xc as i32;
            for yc in 0..6 {
                let wy = weight(ry, yc);
                // The layout is uniform in y, so the y sample index does not
                // change the biome — but the y kernel weight still counts.
                let w = wx * wy * wz;
                let b = biome_sky_layout(sx, sz);
                match order.iter_mut().find(|(bb, _)| *bb == b) {
                    Some((_, acc)) => *acc += w,
                    None => order.push((b, w)),
                }
            }
        }
    }
    if order.is_empty() {
        return base;
    }
    if order.len() == 1 {
        return value_of(order[0].0);
    }
    // Weighted incremental ARGB.srgbLerp (integer channels, floor rounding).
    let lerp_int = |a: f32, p0: i32, p1: i32| p0 + (a * (p1 - p0) as f32).floor() as i32;
    let srgb_lerp = |a: f32, p0: i32, p1: i32| {
        argb(
            lerp_int(a, (p0 >> 24) & 0xFF, (p1 >> 24) & 0xFF),
            lerp_int(a, argb_red(p0), argb_red(p1)),
            lerp_int(a, argb_green(p0), argb_green(p1)),
            lerp_int(a, argb_blue(p0), argb_blue(p1)),
        )
    };
    let mut result: Option<i32> = None;
    let mut accum = 0.0f64;
    for (b, w) in &order {
        let value = value_of(*b);
        accum += w;
        result = Some(match result {
            None => value,
            Some(prev) => srgb_lerp((w / accum) as f32, prev, value),
        });
    }
    result.unwrap_or(base)
}

// ---------------------------------------------------------------------------
// GPU checks — production Vulkan terrain + sky readback
// ---------------------------------------------------------------------------

fn gpu_checks(
    gpu: &mut Gpu,
    _data: &GameData,
    baked: &BakedAssets,
    st: &States,
    args: &TintshotArgs,
    failures: &mut Vec<String>,
) -> Result<(), String> {
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }
    let mut off = Offscreen::new(gpu, W, H)?;
    let ring = OverlayRing::default();
    let draw = overlay(&ring);
    let mut wr = WorldRenderer::new(gpu, off.format, 16, &baked.layers)?;
    wr.set_fog(1.0e6, 1.0e6 + 1.0);

    // Run the render checks; destroy the GPU resources afterwards no matter what
    // (a `?` early return would otherwise leak them past `vkDestroyDevice`).
    let render: Result<(), String> =
        gpu_render(gpu, &mut off, &mut wr, &draw, baked, st, args, failures);
    wr.destroy(gpu);
    off.destroy(gpu);
    render
}

#[allow(clippy::too_many_arguments)]
fn gpu_render(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &rewo_gpu::overlay::OverlayDraw,
    baked: &BakedAssets,
    st: &States,
    args: &TintshotArgs,
    failures: &mut Vec<String>,
) -> Result<(), String> {
    // A uniform 16×16 grass plane in biome A → the whole top face is RED-tinted.
    // If the GPU ignored the mesh tint color, the readback would be the grey
    // grass texture; if it produced constant-plains, it would be green.
    let grass_px = render_plane(gpu, off, wr, draw, baked, st.grass_block, 0, args, "grass")?;
    if !(grass_px[0] as i32 > grass_px[1] as i32 + 20
        && grass_px[0] as i32 > grass_px[2] as i32 + 20)
    {
        failures.push(format!(
            "GPU grass plane readback {grass_px:?} is not red-dominant — GPU ignores mesh tint (or constant-plains)"
        ));
    } else {
        println!("[tintshot] GPU grass plane: {grass_px:?} red-dominant (mesh tint reaches the framebuffer)");
    }

    // A uniform water plane in biome A → BLUE-tinted.
    let water_px = render_plane(gpu, off, wr, draw, baked, st.water, 0, args, "water")?;
    if !(water_px[2] as i32 > water_px[0] as i32 + 20
        && water_px[2] as i32 > water_px[1] as i32 + 20)
    {
        failures.push(format!(
            "GPU water plane readback {water_px:?} is not blue-dominant — water tint missing on the GPU path"
        ));
    } else {
        println!("[tintshot] GPU water plane: {water_px:?} blue-dominant (biome water tint)");
    }

    // Sky/fog uniform: paint the biome A sky (orange) via the production base
    // setter, look at the horizon, read it back. Then repeat with the DEFAULT
    // base and require the two DIFFER — proving the uniform actually drives the
    // shader (a dropped uniform would give the same pixel both times).
    let sky_biome = render_sky(gpu, off, wr, draw, Some(A_SKY), args, "sky_biome")?;
    let sky_default = render_sky(gpu, off, wr, draw, None, args, "sky_default")?;
    // Orange horizon: r > b. The default horizon (pale blue) has b >= r.
    let sky_orange = sky_biome[0] as i32 > sky_biome[2] as i32 + 15;
    let sky_differs = max_delta(sky_biome, sky_default) > 10;
    if !sky_orange {
        failures.push(format!(
            "GPU sky readback {sky_biome:?} is not the orange biome sky — sky/fog uniform ignored"
        ));
    }
    if !sky_differs {
        failures.push(format!(
            "GPU sky: biome {sky_biome:?} == default {sky_default:?} — the sky base uniform has no effect"
        ));
    }
    if sky_orange && sky_differs {
        println!("[tintshot] GPU sky uniform: biome={sky_biome:?} vs default={sky_default:?} (differ; biome is orange)");
    }

    // -- Terrain FOG through the production shader (prove the fog_col path, not
    //    infer it from the sky). Render an UNTINTED stone plane fully fogged with
    //    a GREEN fog base while the sky base is RED (deliberately different
    //    primaries): the center terrain must follow the FOG base, not the sky.
    //    Then swap the fog base to BLUE and require the readback to change. -----
    let sky_red = argb_to_linear(argb_c(255, 0, 0));
    let fog_green = argb_to_linear(argb_c(0, 255, 0));
    let fog_blue = argb_to_linear(argb_c(0, 0, 255));
    let fog_g_px = render_terrain_fog(
        gpu,
        off,
        wr,
        draw,
        baked,
        st,
        sky_red,
        fog_green,
        args,
        "fog_green",
    )?;
    let fog_b_px = render_terrain_fog(
        gpu, off, wr, draw, baked, st, sky_red, fog_blue, args, "fog_blue",
    )?;
    // GREEN fog: center is green-dominant (follows fog base, NOT the red sky).
    let fog_g_ok = fog_g_px[1] as i32 > fog_g_px[0] as i32 + 20
        && fog_g_px[1] as i32 > fog_g_px[2] as i32 + 20;
    // BLUE fog: center is blue-dominant, and it CHANGED from the green case →
    // the fog base uniform actually reaches the shader.
    let fog_b_ok = fog_b_px[2] as i32 > fog_b_px[0] as i32 + 20
        && fog_b_px[2] as i32 > fog_b_px[1] as i32 + 20;
    let fog_changed = max_delta(fog_g_px, fog_b_px) > 10;
    if !fog_g_ok {
        failures.push(format!(
            "GPU terrain fog (green base) readback {fog_g_px:?} is not green-dominant — fog_col path not driving terrain (or it followed the red sky)"
        ));
    }
    if !fog_b_ok {
        failures.push(format!(
            "GPU terrain fog (blue base) readback {fog_b_px:?} is not blue-dominant — fog base uniform ignored"
        ));
    }
    if !fog_changed {
        failures.push(format!(
            "GPU terrain fog: green {fog_g_px:?} == blue {fog_b_px:?} — the fog base uniform has no effect"
        ));
    }
    // Success line only when ALL three properties hold (dominance both cases +
    // the base actually changed the pixel).
    if fog_g_ok && fog_b_ok && fog_changed {
        println!("[tintshot] GPU terrain fog: green-base={fog_g_px:?}, blue-base={fog_b_px:?} (follows fog base, changes with it; sky was red)");
    }

    // Restore the renderer's fog/base state so teardown (and any later pass) is
    // clean — the fog check narrowed the band and set custom bases.
    wr.set_fog(1.0e6, 1.0e6 + 1.0);
    wr.set_sky_tint([1.0; 3], [1.0; 3]);
    wr.set_sky_fog_base(rewo_gpu::world::SKY_HORIZON, rewo_gpu::world::SKY_HORIZON);
    Ok(())
}

/// Render an untinted 16×16 stone plane fully fogged: the camera is far enough
/// that every terrain fragment is past the fog end, so its output is exactly
/// `fog_col = fog_base` (fog_tint = noon identity). `sky_base` is set to a
/// DIFFERENT primary so a center that follows the sky (not the fog) is caught.
#[allow(clippy::too_many_arguments)]
fn render_terrain_fog(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &rewo_gpu::overlay::OverlayDraw,
    baked: &BakedAssets,
    st: &States,
    sky_base: [f32; 3],
    fog_base: [f32; 3],
    args: &TintshotArgs,
    name: &str,
) -> Result<[u8; 3], String> {
    let mut world = World::new(shape());
    world.ensure_column(0, 0);
    for z in 0..16 {
        for x in 0..16 {
            world.set_block(x, 8, z, st.stone); // untinted opaque grey
        }
    }
    // Biome 0 + context so the mesh path matches production (stone is untinted,
    // so no tint reaches the plane — the readback is pure fog).
    let buf = single_section(0);
    let mut r = PacketReader::new(&buf);
    let containers = rewo_world::chunk::read_chunk_biomes(&mut r, &shape(), 7).unwrap();
    world.apply_chunks_biomes(0, 0, containers);
    world.set_biome_context(std::sync::Arc::new(context()));
    let mesh = mesh_column(&world, &baked.render, &baked.models, &baked.fluid, 0, 0)
        .ok_or_else(|| format!("{name}: stone plane meshed empty"))?;

    let ov: &[u8] = bytemuck::cast_slice(&mesh.vertices);
    let tv: &[u8] = bytemuck::cast_slice(&mesh.tvertices);
    wr.upload_column(gpu, 0, 0, ov, &mesh.indices, tv, &mesh.tindices, 8.0, 10.0)?;
    gpu.wait_idle();

    let eye = Vec3::new(8.0, 40.0, 8.0);
    wr.set_camera([eye.x, eye.y, eye.z]);
    wr.set_sky_tint([1.0; 3], [1.0; 3]); // noon: fog_tint = identity
    wr.set_sky_fog_base(sky_base, fog_base); // DIFFERENT primaries
                                             // Fog band ends at 5 (< the ~32u camera→plane distance) → fog = 1.0 → the
                                             // terrain fragment output is exactly fog_col, whatever the texture was.
    wr.set_fog(0.0, 5.0);
    let view = Mat4::look_to_rh(eye, Vec3::new(0.0, -1.0, 0.0), Vec3::Z);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(60f32.to_radians(), 1.0, 0.05));
    let vp = (proj * view).to_cols_array_2d();
    off.render(gpu, Some((&mut *wr, vp)), draw, [0.0, 0.0, 0.0, 1.0])?;
    let img = off.read_rgba(gpu)?;
    if let Some(d) = &args.out_dir {
        let _ = off.save_png(gpu, &d.join(format!("{name}.png")));
    }
    wr.remove_column(gpu, 0, 0);
    Ok(center_avg(&img, W, H, 6))
}

/// Mesh a uniform 16×16 plane of `block` under biome `id`, upload through the
/// production `upload_column`, render top-down, read the centre patch.
#[allow(clippy::too_many_arguments)]
fn render_plane(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &rewo_gpu::overlay::OverlayDraw,
    baked: &BakedAssets,
    block: u32,
    biome_id: u32,
    args: &TintshotArgs,
    name: &str,
) -> Result<[u8; 3], String> {
    let mut world = World::new(shape());
    world.ensure_column(0, 0);
    for z in 0..16 {
        for x in 0..16 {
            world.set_block(x, 8, z, block);
        }
    }
    // Uniform biome `id` via chunks_biomes (single-value palette).
    let buf = single_section(biome_id);
    let mut r = PacketReader::new(&buf);
    let containers = rewo_world::chunk::read_chunk_biomes(&mut r, &shape(), 7).unwrap();
    world.apply_chunks_biomes(0, 0, containers);
    world.set_biome_context(std::sync::Arc::new(context()));
    let mesh = mesh_column(&world, &baked.render, &baked.models, &baked.fluid, 0, 0)
        .ok_or_else(|| format!("{name}: plane meshed empty"))?;

    let ov: &[u8] = bytemuck::cast_slice(&mesh.vertices);
    let tv: &[u8] = bytemuck::cast_slice(&mesh.tvertices);
    wr.upload_column(gpu, 0, 0, ov, &mesh.indices, tv, &mesh.tindices, 8.0, 10.0)?;
    gpu.wait_idle();

    // Straight-down camera over the plane (fills the frame like lightmapshot).
    let eye = Vec3::new(8.0, 40.0, 8.0);
    wr.set_camera([eye.x, eye.y, eye.z]);
    wr.set_sky_tint([1.0; 3], [1.0; 3]); // noon; the plane covers the patch
    wr.set_sky_fog_base([0.0; 3], [0.0; 3]); // black sky, geometry covers it
    let view = Mat4::look_to_rh(eye, Vec3::new(0.0, -1.0, 0.0), Vec3::Z);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(60f32.to_radians(), 1.0, 0.05));
    let vp = (proj * view).to_cols_array_2d();
    off.render(gpu, Some((&mut *wr, vp)), draw, [0.0, 0.0, 0.0, 1.0])?;
    let img = off.read_rgba(gpu)?;
    if let Some(d) = &args.out_dir {
        let _ = off.save_png(gpu, &d.join(format!("{name}.png")));
    }
    wr.remove_column(gpu, 0, 0);
    Ok(center_avg(&img, W, H, 6))
}

/// Render the sky with an optional biome base color, looking at the horizon.
fn render_sky(
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &rewo_gpu::overlay::OverlayDraw,
    base_argb: Option<i32>,
    args: &TintshotArgs,
    name: &str,
) -> Result<[u8; 3], String> {
    wr.set_sky_tint([1.0; 3], [1.0; 3]); // noon multiply = identity
    match base_argb {
        Some(c) => wr.set_sky_fog_base(argb_to_linear(c), argb_to_linear(c)),
        // Restore the renderer default (SKY_HORIZON) by asking for it directly.
        None => wr.set_sky_fog_base(rewo_gpu::world::SKY_HORIZON, rewo_gpu::world::SKY_HORIZON),
    }
    // Horizontal camera looking at the horizon (+X), up +Y — the horizon band
    // is the sky base color.
    let eye = Vec3::new(0.0, 8.0, 0.0);
    wr.set_camera([eye.x, eye.y, eye.z]);
    let view = Mat4::look_to_rh(eye, Vec3::X, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(60f32.to_radians(), 1.0, 0.05));
    let vp = (proj * view).to_cols_array_2d();
    off.render(gpu, Some((&mut *wr, vp)), draw, [0.0, 0.0, 0.0, 1.0])?;
    let img = off.read_rgba(gpu)?;
    if let Some(d) = &args.out_dir {
        let _ = off.save_png(gpu, &d.join(format!("{name}.png")));
    }
    // Sample the horizon row (vertical centre).
    Ok(center_avg(&img, W, H, 6))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Tinted vertices of the block at (x,y,z): within the unit cube + a non-white
/// color (the biome path carries the tint bytes; `reconstructed_color()`
/// mirrors the vertex shader's `shade * ao * tint/255`).
fn tinted_verts<'a>(
    mesh: &'a rewo_mesh::ColumnMesh,
    x: i32,
    y: i32,
    z: i32,
) -> Vec<&'a MeshVertex> {
    let inside = |v: &MeshVertex| {
        v.pos[0] >= x as f32 - 0.01
            && v.pos[0] <= x as f32 + 1.01
            && v.pos[1] >= y as f32 - 0.01
            && v.pos[1] <= y as f32 + 1.01
            && v.pos[2] >= z as f32 - 0.01
            && v.pos[2] <= z as f32 + 1.01
    };
    let is_tinted = |v: &MeshVertex| {
        (v.reconstructed_color()[0] - v.reconstructed_color()[1]).abs() > 1e-4
            || (v.reconstructed_color()[1] - v.reconstructed_color()[2]).abs() > 1e-4
    };
    mesh.vertices
        .iter()
        .chain(mesh.tvertices.iter())
        .filter(|v| inside(v) && is_tinted(v))
        .collect()
}

/// Texture layer indices used by the top face (y = block top) of a block.
fn top_face_layers(mesh: &rewo_mesh::ColumnMesh, x: i32, y: i32, z: i32) -> Vec<u16> {
    let top = (y + 1) as f32;
    mesh.vertices
        .iter()
        .chain(mesh.tvertices.iter())
        .filter(|v| {
            (v.pos[1] - top).abs() < 0.01
                && v.pos[0] >= x as f32 - 0.01
                && v.pos[0] <= x as f32 + 1.01
                && v.pos[2] >= z as f32 - 0.01
                && v.pos[2] <= z as f32 + 1.01
        })
        .map(|v| v.layer_index())
        .collect()
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

fn max_delta(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3)
        .map(|c| (a[c] as i32 - b[c] as i32).abs())
        .max()
        .unwrap()
}

fn argb_to_linear(c: i32) -> [f32; 3] {
    let s = |v: i32| srgb_to_linear(v as f32 / 255.0);
    [s(argb_red(c)), s(argb_green(c)), s(argb_blue(c))]
}

fn jar_path(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared");
    p.push("versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

/// An off-screen overlay draw (positioned far outside the frame so the chart
/// never overlaps the sampled patch) — `off.render` needs an `OverlayDraw`.
fn overlay(ring: &OverlayRing) -> rewo_gpu::overlay::OverlayDraw<'_> {
    rewo_gpu::overlay::OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

/// Average RGB over a centred square of half-size `r`.
fn center_avg(img: &[u8], w: u32, h: u32, r: u32) -> [u8; 3] {
    let (cx, cy) = (w / 2, h / 2);
    let (mut sum, mut n) = ([0f64; 3], 0f64);
    for py in cy.saturating_sub(r)..(cy + r).min(h) {
        for px in cx.saturating_sub(r)..(cx + r).min(w) {
            let i = ((py * w + px) * 4) as usize;
            for c in 0..3 {
                sum[c] += img[i + c] as f64;
            }
            n += 1.0;
        }
    }
    std::array::from_fn(|c| (sum[c] / n).round() as u8)
}

/// sRGB → linear decode (IEC 61966-2-1) — the space the GPU sky base wants.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
