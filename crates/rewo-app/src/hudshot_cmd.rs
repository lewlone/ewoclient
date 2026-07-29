//! `rewo hudshot --check` — the Velvet UI gate (M52b).
//!
//! Serverless, CPU-only, fail-closed. Follows the `*_cmd.rs` pattern the other
//! fourteen gates use.
//!
//! ## What it grades, and why it is not a screenshot comparison
//!
//! The fidelity target is **pixel-faithful against the Skia originals**, so
//! this asserts the *numbers*: the layout chain, all nine anchors, the glyph
//! metrics, and the shadow stack. Every expectation is an **independent
//! recomputation** of the formula from `ewo-jni/src/hud.rs` — this file does
//! not call `layout_coords` and compare it to itself.
//!
//! That distinction is the whole point. Several gates in this repo have been
//! caught asserting a proxy rather than the property (the M37 frame-diff, the
//! `mobshot` colour check), and one lesson from the survey work in the same
//! session applies directly: **a gate that reimplements a slice of the app's
//! setup will miss whatever the app adds to it** — `itemshot` measured zero
//! glint because it called `init_entities` directly instead of through the
//! app's helper. So the witnesses here drive the production functions and
//! compare against hand-derived arithmetic, never the other way round.

use clap::Args;

use rewo_gpu::velvet_glyph::{Axes, Family, GlyphCache, ScalerKey};
use rewo_gpu::velvet_widgets as w;

#[derive(Args, Debug)]
pub struct HudshotArgs {
    /// Grade the Velvet layout against the transcribed Skia constants.
    #[arg(long, default_value_t = false)]
    pub check: bool,
}

struct Grade {
    pass: usize,
    fail: usize,
}

impl Grade {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        if ok {
            self.pass += 1;
            println!("[hudshot] ok   {name}");
        } else {
            self.fail += 1;
            println!("[hudshot] FAIL {name}: {detail}");
        }
    }

    fn near(&mut self, name: &str, got: f32, want: f32, tol: f32) {
        self.check(
            name,
            (got - want).abs() <= tol,
            format!("got {got}, want {want} (tol {tol})"),
        );
    }
}

fn load_cache() -> Option<GlyphCache> {
    let mut c = GlyphCache::new();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
    for (fam, ital) in [
        (Family::Fraunces, false),
        (Family::Fraunces, true),
        (Family::Newsreader, false),
        (Family::JetBrainsMono, false),
    ] {
        let p = dir.join(format!("{}.ttf", fam.file_stem(ital)));
        let data = std::fs::read(&p).ok()?;
        if !c.load(fam, ital, data) {
            eprintln!("[hudshot] {} failed to parse", p.display());
            return None;
        }
    }
    Some(c)
}

pub fn run(args: HudshotArgs) -> Result<(), String> {
    if !args.check {
        return Err("hudshot: only --check is implemented".into());
    }
    // Fail closed: a missing font set is not a pass.
    let Some(mut cache) = load_cache() else {
        return Err("hudshot: assets/fonts is missing or unreadable".into());
    };
    let mut g = Grade { pass: 0, fail: 0 };

    // ── a: anchors. Independently written out rather than looped over the
    //    production `fractions()`, so a swapped pair is caught.
    {
        let (w_, h_) = (40.0f32, 20.0f32);
        let (ax, ay) = (100.0f32, 200.0f32);
        let cases: [(w::Anchor, f32, f32); 9] = [
            (w::Anchor::Tl, 100.0, 200.0),
            (w::Anchor::Tc, 80.0, 200.0),
            (w::Anchor::Tr, 60.0, 200.0),
            (w::Anchor::Ml, 100.0, 190.0),
            (w::Anchor::Mc, 80.0, 190.0),
            (w::Anchor::Mr, 60.0, 190.0),
            (w::Anchor::Bl, 100.0, 180.0),
            (w::Anchor::Bc, 80.0, 180.0),
            (w::Anchor::Br, 60.0, 180.0),
        ];
        for (a, ex, ey) in cases {
            let (x, y) = a.origin(ax, ay, w_, h_);
            g.check(
                &format!("a:{}", a.as_str()),
                (x - ex).abs() < 1e-4 && (y - ey).abs() < 1e-4,
                format!("got ({x}, {y}), want ({ex}, {ey})"),
            );
        }
    }

    // ── b: the value string. Format quirks, including the double space.
    {
        g.check(
            "b1:value_format",
            w::coords_value_text(-128.44, 64.4, -1492.01) == "-128.4  64  -1492.0",
            format!("{:?}", w::coords_value_text(-128.44, 64.4, -1492.01)),
        );
        g.check(
            "b2:y_rounds_not_truncates",
            w::coords_value_text(0.0, 63.7, 0.0) == "0.0  64  0.0",
            format!("{:?}", w::coords_value_text(0.0, 63.7, 0.0)),
        );
        g.check(
            "b3:double_space_is_deliberate",
            w::coords_value_text(1.0, 2.0, 3.0).matches("  ").count() == 2,
            "the two-space separator is transcribed, not a typo".into(),
        );
    }

    // ── c: glyph metrics sanity, per family. Cap height drives every plate's
    //    height, so a zero or em-sized value is a whole-HUD failure.
    {
        for fam in [Family::Fraunces, Family::Newsreader, Family::JetBrainsMono] {
            let k = ScalerKey::new(fam, false, 18.0, Axes::DEFAULT);
            let m = cache.metrics(k).expect("metrics");
            g.check(
                &format!("c:{fam:?}_cap"),
                m.cap_height > 4.0 && m.cap_height < 18.0,
                format!("cap {} at 18px", m.cap_height),
            );
        }
    }

    // ── d: the Coords chain, recomputed by hand from draw_coords.
    {
        let label_key = w::coords_label_key();
        let value_key = w::coords_value_key();
        let value = w::coords_value_text(-128.4, 64.0, -1492.0);

        // Recomputed here, independently of layout_coords.
        let label_w = cache.measure_tracked(label_key, "XYZ", 0.22);
        let value_w = cache.measure_tracked(value_key, &value, 0.0);
        let cap = cache.metrics(value_key).unwrap().cap_height;
        let want_w = 14.0 * 2.0 + label_w + 14.0 + value_w;
        let want_h = 8.0 * 2.0 + cap;

        let l = w::layout_coords(&mut cache, -128.4, 64.0, -1492.0, w::Anchor::Tl, 0.0, 0.0);
        g.near("d1:chip_w", l.chip[2], want_w, 0.01);
        g.near("d2:chip_h", l.chip[3], want_h, 0.01);
        g.near("d3:baseline", l.label_origin.1, 0.0 + 8.0 + cap, 0.01);
        g.near(
            "d4:value_x",
            l.value_origin.0,
            0.0 + 14.0 + label_w + 14.0,
            0.01,
        );
        g.check(
            "d5:shared_baseline",
            (l.label_origin.1 - l.value_origin.1).abs() < 1e-4,
            format!("{} vs {}", l.label_origin.1, l.value_origin.1),
        );
        g.check(
            "d6:radius",
            (l.radius - 12.0).abs() < 1e-4,
            format!("{}", l.radius),
        );
        // Sensitivity: the plate must NOT be sized by a line height.
        let m = cache.metrics(value_key).unwrap();
        g.check(
            "d7:not_line_height",
            l.chip[3] < 16.0 + m.ascent + m.descent,
            format!("chip_h {} vs 16 + line {}", l.chip[3], m.ascent + m.descent),
        );
    }

    // ── e: anchoring moves the plate without changing it.
    {
        let tl = w::layout_coords(&mut cache, 1.0, 2.0, 3.0, w::Anchor::Tl, 500.0, 400.0);
        let br = w::layout_coords(&mut cache, 1.0, 2.0, 3.0, w::Anchor::Br, 500.0, 400.0);
        g.check(
            "e1:size_anchor_invariant",
            (tl.chip[2] - br.chip[2]).abs() < 1e-4 && (tl.chip[3] - br.chip[3]).abs() < 1e-4,
            format!("{:?} vs {:?}", tl.chip, br.chip),
        );
        g.near(
            "e2:internal_offset_invariant",
            tl.label_origin.0 - tl.chip[0],
            br.label_origin.0 - br.chip[0],
            0.01,
        );
        g.near("e3:br_places_bottom_right", br.chip[0] + br.chip[2], 500.0, 0.01);
    }

    // ── f: the shadow stack. Three copies, sigmas 5/3/0, the last 1px down.
    {
        let l = w::layout_coords(&mut cache, -128.4, 64.0, -1492.0, w::Anchor::Tl, 0.0, 0.0);
        let value = w::coords_value_text(-128.4, 64.0, -1492.0);
        let (shell, runs) = w::emit_coords(&mut cache, &l, &value);
        g.check(
            "f1:shell_matches_chip",
            shell.rect == l.chip,
            format!("{:?} vs {:?}", shell.rect, l.chip),
        );
        g.check("f2:run_count", runs.len() == 8, format!("{}", runs.len()));
        g.check(
            "f3:label_is_rose",
            runs[3].color == w::ROSE && (runs[3].alpha - 0.9).abs() < 1e-4,
            format!("{:?} @{}", runs[3].color, runs[3].alpha),
        );
        g.check(
            "f4:value_is_pearl",
            runs[7].color == w::PEARL && (runs[7].alpha - 1.0).abs() < 1e-4,
            format!("{:?} @{}", runs[7].color, runs[7].alpha),
        );
        g.check(
            "f5:shadow_alphas",
            (runs[0].alpha - 0.55).abs() < 1e-4
                && (runs[1].alpha - 0.85).abs() < 1e-4
                && (runs[2].alpha - 0.95).abs() < 1e-4,
            format!("{} {} {}", runs[0].alpha, runs[1].alpha, runs[2].alpha),
        );
        g.check(
            "f6:no_empty_run",
            runs.iter().all(|r| !r.glyphs.is_empty()),
            "a silently absent shadow copy".into(),
        );
        // The hard copy is 1px below the two halos. Compare a glyph's top.
        let halo_y = runs[1].glyphs[0].dst_y;
        let hard_y = runs[2].glyphs[0].dst_y;
        // The halos are blurred so their boxes are taller; compare the
        // BASELINE-relative placement instead, via the run's own centre.
        let halo_c = halo_y + runs[1].glyphs[0].dst_h / 2.0;
        let hard_c = hard_y + runs[2].glyphs[0].dst_h / 2.0;
        g.near("f7:hard_copy_is_1px_down", hard_c - halo_c, 1.0, 0.6);
    }

    // ── g: the widest shadow must track the text pen for pen. A shadow that
    //    advanced by its blurred box width would spread wider than its text --
    //    subtle, and easy to accept by eye.
    {
        let l = w::layout_coords(&mut cache, -128.4, 64.0, -1492.0, w::Anchor::Tl, 0.0, 0.0);
        let value = w::coords_value_text(-128.4, 64.0, -1492.0);
        let (_, runs) = w::emit_coords(&mut cache, &l, &value);
        let span = |r: &w::TintedRun| {
            let a = r.glyphs.first().unwrap();
            let b = r.glyphs.last().unwrap();
            (b.dst_x + b.dst_w / 2.0) - (a.dst_x + a.dst_w / 2.0)
        };
        g.near("g1:shadow_span_tracks_text", span(&runs[4]), span(&runs[7]), 0.05);
        g.check(
            "g2:shadow_glyph_count",
            runs[4].glyphs.len() == runs[7].glyphs.len(),
            format!("{} vs {}", runs[4].glyphs.len(), runs[7].glyphs.len()),
        );
    }

    // ── h: variable axes actually reach the rasterizer. If they were dropped,
    //    every Velvet heading would silently render at the default instance.
    {
        let id = cache.glyph_id(Family::Fraunces, false, 'X').unwrap();
        let light = ScalerKey::new(Family::Fraunces, false, 48.0, Axes::fraunces(0.0, 0.0, 100.0, None));
        let heavy = ScalerKey::new(Family::Fraunces, false, 48.0, Axes::fraunces(0.0, 0.0, 900.0, None));
        let a = cache.glyph(light, id).unwrap();
        let b = cache.glyph(heavy, id).unwrap();
        let ink = |gl: rewo_gpu::velvet_glyph::Glyph, c: &GlyphCache| -> u32 {
            (0..gl.h)
                .map(|r| {
                    (0..gl.w)
                        .filter(|col| {
                            c.atlas()[((gl.y + r) * c.atlas_edge() + gl.x + col) as usize] > 128
                        })
                        .count() as u32
                })
                .sum()
        };
        g.check(
            "h1:wght_axis_reaches_the_rasterizer",
            ink(b, &cache) > ink(a, &cache),
            format!("wght900 {} vs wght100 {}", ink(b, &cache), ink(a, &cache)),
        );
        // Sensitivity partner: the Coords value key is NOT the default
        // instance, so it must differ from a plain 18px Fraunces.
        let plain = ScalerKey::new(Family::Fraunces, false, 18.0, Axes::DEFAULT);
        g.check(
            "h2:coords_key_is_not_the_default_instance",
            w::coords_value_key() != plain,
            "SOFT 30 / wght 500 / opsz 36 collapsed to the default".into(),
        );
    }

    println!("[hudshot] {} passed, {} failed", g.pass, g.fail);
    if g.fail > 0 {
        return Err(format!("hudshot: {} witnesses failed", g.fail));
    }
    // Fail closed on a suspiciously small witness count -- the same guard the
    // other gates carry, so a refactor that silently drops witnesses is loud
    // rather than a quieter green.
    if g.pass < 30 {
        return Err(format!(
            "hudshot: only {} witnesses ran, expected >= 30",
            g.pass
        ));
    }
    println!("[hudshot] CHECK OK — {} witnesses", g.pass);
    Ok(())
}
