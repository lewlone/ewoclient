//! `rewo meshshot` — serverless headless verification of the M15 greedy mesher
//! (the acceptance oracle).
//!
//! This command is a thin front-end over [`rewo_mesh::check_greedy_oracle`].
//! The grading itself lives in production code, not in `mod tests`, so the
//! release binary and the unit test run the *identical* checker and cannot
//! drift apart — this module deliberately re-derives and re-asserts nothing.
//!
//! The oracle compares the production `mesh_column` against the frozen
//! `mesh_column_reference` on one adversarial cube column. The comparison is
//! semantic, not byte-wise: each merged rectangle is expanded back into the unit
//! faces it covers, and that set — face direction, owning block, layer +
//! block/sky light, all four AO codes, tint word — must equal the reference's
//! exactly. A byte diff cannot express this, because a merged rectangle is
//! *supposed* to differ from the N unit quads it replaces.
//!
//! No jar, no network, no Vulkan: the fixture is synthetic and the run is
//! deterministic.
//!
//! What this prints is the *measured* report the checker returns only on
//! success. The numbers are the anti-vacuity proof: a checker that silently
//! graded an empty column would pass its own assertions, so the counts are what
//! show the adversarial fixture actually exercised every property.

use clap::Args as ClapArgs;
use rewo_mesh::{check_greedy_oracle, GreedyOracleReport, LegacyControlCounts};

#[derive(ClapArgs)]
pub struct MeshshotArgs {
    /// Assert the M15 greedy properties: the expanded rectangles equal the
    /// reference mesher's unit-face surface exactly, the five safe directions
    /// merge, up faces stay 1x1, and material/light/tint/AO boundaries split at
    /// the seam. The oracle asserts unconditionally and any failure exits
    /// nonzero, so this flag only labels the run.
    #[arg(long, default_value_t = false)]
    check: bool,
}

pub fn run(args: MeshshotArgs) -> Result<(), String> {
    // Always propagate: the oracle is the gate whether or not `--check` was
    // passed, so a property failure fails the command either way.
    let report = check_greedy_oracle()?;
    print_report(&args, &report);

    println!(
        "MESHSHOT: OK — the greedy mesher's rectangles expand to exactly the reference \
         mesher's unit-face surface (direction, owning block, layer + block/sky light, \
         all four AO codes, tint word); all five safe directions merged while every up \
         face stayed 1x1 (top-unit exclusion); the material, block-light, sky-light and \
         tint boundaries each split at the exact seam; non-uniform AO fell back to unit \
         quads inside a plane that still merged elsewhere; the model, water and lava \
         controls came out byte-identical to the reference; and one waterlogged block \
         produced BOTH streams from a single state, with its own DOWN face suppressed by \
         its own occlusion shape, its UP face NOT even when it occludes on all six, and an ordinary pool beside it \
         suppressing the face between them"
    );
    Ok(())
}

/// Print what the oracle measured. Every number here is observed — the checker
/// returns `Err` before building a report if any property failed.
fn print_report(args: &MeshshotArgs, r: &GreedyOracleReport) {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[meshshot] mode: {mode} (the oracle asserts unconditionally — a failure exits \
         nonzero with or without --check)"
    );

    println!(
        "[meshshot] surface: reference {} unit faces / {} quads -> optimized {} quads \
         ({} cube rects); vertices -{:.1}%, indices -{:.1}%",
        r.reference_unit_faces,
        r.reference_quads,
        r.optimized_quads,
        r.optimized_cube_rects,
        r.vertex_reduction_percent,
        r.index_reduction_percent,
    );
    println!(
        "[meshshot] greedy: visible {} = candidates {} + fallback {}; greedy_quads {} \
         (+ fallback = {} optimized quads); deterministic runs {}",
        r.visible_cube_faces,
        r.greedy_candidate_faces,
        r.unit_fallback_faces,
        r.greedy_quads,
        r.optimized_quads,
        r.deterministic_runs,
    );

    let a = r.max_rect_area_per_face;
    println!(
        "[meshshot] max rect area per face: up {}, down {}, north {}, south {}, west {}, east {}",
        a[0], a[1], a[2], a[3], a[4], a[5],
    );
    println!(
        "[meshshot] top-unit exclusion: {} up faces, max up rect area {} (1 = never merged); \
         section-crossing rects {}",
        r.up_faces, r.max_up_rect_area, r.section_crossing_rects,
    );
    println!(
        "[meshshot] AO: {} non-uniform faces fell back to unit quads; largest rect in the \
         same plane {}",
        r.nonuniform_ao_faces, r.ao_plane_max_rect_area,
    );

    println!(
        "[meshshot] material boundary: rects {:?} on shared layer {}; cutout faces {}",
        r.material_boundary_rects, r.material_boundary_layer, r.cutout_faces,
    );
    println!(
        "[meshshot] light boundary: block {:?}, sky {:?}",
        r.block_light_boundary_rects, r.sky_light_boundary_rects,
    );
    println!(
        "[meshshot] tint boundary: rects {:?} on raw layer {}, distinct words {:#010x} / {:#010x}",
        r.tint_boundary_rects,
        r.tint_boundary_layer,
        r.tint_boundary_words[0],
        r.tint_boundary_words[1],
    );

    let w = &r.waterlogged_faces;
    println!(
        "[meshshot] waterlogged: one Model state emitted {} opaque + {} translucent vertices \
         from ONE position (the dry twin: {} translucent quads); carried_fluid_cells {} / {}",
        r.waterlogged.opaque_vertices,
        r.waterlogged.translucent_vertices,
        w.dry_translucent_quads,
        w.carried_cells.0,
        w.carried_cells.1,
    );
    println!(
        "[meshshot] waterlogged faces (top, side, bottom): own-DOWN-covered {:?}, \
         own-UP-covered {:?}, ALL-SIX-covered {:?} — `renderUp` skips \
         `shouldRenderFace`, so the top survives even a double slab; pool quads in the \
         plane between it and a waterlogged / a dry neighbour: {} / {}",
        w.bottom_slab,
        w.top_slab,
        w.double_slab,
        w.pool_plane_faces.0,
        w.pool_plane_faces.1,
    );

    print_legacy("model", r.model_only_identical, r.model_only);
    print_legacy("water", r.water_only_identical, r.water_only);
    print_legacy("waterlogged", r.waterlogged_identical, r.waterlogged);
    print_legacy("lava", r.lava_only_identical, r.lava_only);
}

/// A legacy (non-cube) control: it must never enter the greedy path, so the
/// optimized mesher reproduces the reference byte for byte. The counts show the
/// control was not vacuous, and which stream it owns.
fn print_legacy(name: &str, identical: bool, c: LegacyControlCounts) {
    println!(
        "[meshshot] {name} control: byte-identical {identical}; opaque {}v/{}i, \
         translucent {}v/{}i",
        c.opaque_vertices, c.opaque_indices, c.translucent_vertices, c.translucent_indices,
    );
}
