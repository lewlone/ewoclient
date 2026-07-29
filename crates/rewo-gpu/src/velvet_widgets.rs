//! Velvet HUD widgets (M52b step 4) — transcribed from `ewo-jni/src/hud.rs`.
//!
//! Layout is a **pure function**: it takes a glyph cache and returns rects and
//! text runs, with no GPU involved. That is what lets `hudshot --check` assert
//! the geometry against the Skia constants directly rather than by squinting
//! at a screenshot — the fidelity target is pixel-faithful, so the numbers
//! have to be checkable as numbers.
//!
//! Every constant here is transcribed, not chosen. When one looks arbitrary it
//! is because it was calibrated by eye against the design; changing it to a
//! rounder number is a visual regression, not a cleanup.

use crate::velvet_chrome::Shell;
use crate::velvet_glyph::{Axes, Family, GlyphCache, PositionedGlyph, ScalerKey};

// ── Velvet tokens (CLAUDE.md "Velvet theme tokens"). sRGB bytes / 255, no
//    transfer function — matching `ewo-jni`'s `rgba()` exactly. See the
//    colour-space note in REWO_VELVET_UI_PLAN.md §3: these are composited in
//    GAMMA space, so the pass must render through a UNORM view.
pub const PEARL: [f32; 3] = [0xF4 as f32 / 255.0, 0xE8 as f32 / 255.0, 0xEA as f32 / 255.0];
pub const ROSE: [f32; 3] = [0xE5 as f32 / 255.0, 0xB8 as f32 / 255.0, 0xC5 as f32 / 255.0];
pub const WINE: [f32; 3] = [0x12 as f32 / 255.0, 0x00 as f32 / 255.0, 0x10 as f32 / 255.0];

/// Where a widget's anchor point sits within its own box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Tl,
    Tc,
    Tr,
    Ml,
    Mc,
    Mr,
    Bl,
    Bc,
    Br,
}

impl Anchor {
    pub fn fractions(self) -> (f32, f32) {
        match self {
            Anchor::Tl => (0.0, 0.0),
            Anchor::Tc => (0.5, 0.0),
            Anchor::Tr => (1.0, 0.0),
            Anchor::Ml => (0.0, 0.5),
            Anchor::Mc => (0.5, 0.5),
            Anchor::Mr => (1.0, 0.5),
            Anchor::Bl => (0.0, 1.0),
            Anchor::Bc => (0.5, 1.0),
            Anchor::Br => (1.0, 1.0),
        }
    }

    /// Top-left draw origin for a `w × h` widget anchored at `(ax, ay)`.
    pub fn origin(self, ax: f32, ay: f32, w: f32, h: f32) -> (f32, f32) {
        let (fx, fy) = self.fractions();
        (ax - w * fx, ay - h * fy)
    }

    /// `hud.toml` token — the same strings the editor writes.
    pub fn as_str(self) -> &'static str {
        match self {
            Anchor::Tl => "tl",
            Anchor::Tc => "tc",
            Anchor::Tr => "tr",
            Anchor::Ml => "ml",
            Anchor::Mc => "mc",
            Anchor::Mr => "mr",
            Anchor::Bl => "bl",
            Anchor::Bc => "bc",
            Anchor::Br => "br",
        }
    }

    pub fn from_str(s: &str) -> Option<Anchor> {
        Some(match s {
            "tl" => Anchor::Tl,
            "tc" => Anchor::Tc,
            "tr" => Anchor::Tr,
            "ml" => Anchor::Ml,
            "mc" => Anchor::Mc,
            "mr" => Anchor::Mr,
            "bl" => Anchor::Bl,
            "bc" => Anchor::Bc,
            "br" => Anchor::Br,
            _ => return None,
        })
    }
}

// ── The in-world text shadow stack ────────────────────────────────────────
//
// `draw_iw_text_shadow` / `_tracked`: three wine copies under the glyph.
// Skia's `MaskFilter::blur` takes a SIGMA, so these are sigmas already —
// no radius/2 conversion. The comments in the original quote the CSS blur
// radius (10px, 6px), which is 2x the sigma.
/// `(sigma, alpha, dy)` per shadow copy, in draw order.
pub const IW_SHADOW: [(f32, f32, f32); 3] = [
    (5.0, 0.55, 0.0), // wide halo
    (3.0, 0.85, 0.0), // tight halo
    (0.0, 0.95, 1.0), // hard underline, 1px down
];

// ── Coords ────────────────────────────────────────────────────────────────

/// `.w-coords` metrics, transcribed from `draw_coords`.
pub const COORDS_PAD_X: f32 = 14.0;
pub const COORDS_PAD_Y: f32 = 8.0;
pub const COORDS_GAP: f32 = 14.0;
pub const COORDS_RADIUS: f32 = 12.0;
pub const COORDS_LABEL: &str = "XYZ";
/// CSS `letter-spacing: .22em` on the label.
pub const COORDS_LABEL_TRACKING_EM: f32 = 0.22;
pub const COORDS_LABEL_SIZE: f32 = 12.0;
pub const COORDS_VALUE_SIZE: f32 = 18.0;

/// The value font: Fraunces 18, `SOFT 30`, `WONK 0`, `wght 500`, `opsz 36`.
///
/// `opsz` is pinned to 36 rather than tracking the size — the design's
/// calibration. Letting it track (CSS `font-optical-sizing: auto`) picks a
/// different design instance and the digits change width.
pub fn coords_value_key() -> ScalerKey {
    ScalerKey::new(
        Family::Fraunces,
        false,
        COORDS_VALUE_SIZE,
        Axes::fraunces(30.0, 0.0, 500.0, Some(36.0)),
    )
}

pub fn coords_label_key() -> ScalerKey {
    ScalerKey::new(
        Family::JetBrainsMono,
        false,
        COORDS_LABEL_SIZE,
        Axes::DEFAULT,
    )
}

/// `x/z` to one decimal, `y` rounded — the prototype's `-128.4  64  -1492.0`.
///
/// **Two spaces between fields**, not one. It reads as a typo and is not: the
/// gap is what keeps the three numbers legible as three numbers at a glance.
pub fn coords_value_text(x: f64, y: f64, z: f64) -> String {
    format!("{:.1}  {}  {:.1}", x, y.round() as i64, z)
}

/// Everything the renderer and the gate need, with no GPU involved.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordsLayout {
    /// `(x, y, w, h)` of the plate.
    pub chip: [f32; 4],
    pub radius: f32,
    /// Baseline origin of the tracked `XYZ` label.
    pub label_origin: (f32, f32),
    /// Baseline origin of the position value.
    pub value_origin: (f32, f32),
    pub label_w: f32,
    pub value_w: f32,
    /// Cap height of the *value* font — the chip's height is derived from it.
    pub cap: f32,
}

/// Lay out the Coords widget.
///
/// The chain, verbatim from `draw_coords`:
///
/// ```text
/// chip_w   = pad_x*2 + label_w + gap + value_w
/// chip_h   = pad_y*2 + cap
/// (cx, cy) = anchor.origin(ax, ay, chip_w, chip_h)
/// baseline = cy + pad_y + cap
/// label at (cx + pad_x, baseline)
/// value at (cx + pad_x + label_w + gap, baseline)
/// ```
///
/// Note the height comes from the **value** font's cap height, not from the
/// label's and not from a line height. A plate sized by ascent+descent would
/// be visibly taller and the vertical centring would drift.
pub fn layout_coords(
    cache: &mut GlyphCache,
    x: f64,
    y: f64,
    z: f64,
    anchor: Anchor,
    ax: f32,
    ay: f32,
) -> CoordsLayout {
    let label_key = coords_label_key();
    let value_key = coords_value_key();
    let value = coords_value_text(x, y, z);

    let label_w = cache.measure_tracked(label_key, COORDS_LABEL, COORDS_LABEL_TRACKING_EM);
    let value_w = cache.measure_tracked(value_key, &value, 0.0);
    let cap = cache
        .metrics(value_key)
        .map(|m| m.cap_height)
        .unwrap_or(COORDS_VALUE_SIZE * 0.72);

    let chip_w = COORDS_PAD_X * 2.0 + label_w + COORDS_GAP + value_w;
    let chip_h = COORDS_PAD_Y * 2.0 + cap;
    let (cx, cy) = anchor.origin(ax, ay, chip_w, chip_h);
    let baseline = cy + COORDS_PAD_Y + cap;

    CoordsLayout {
        chip: [cx, cy, chip_w, chip_h],
        radius: COORDS_RADIUS,
        label_origin: (cx + COORDS_PAD_X, baseline),
        value_origin: (cx + COORDS_PAD_X + label_w + COORDS_GAP, baseline),
        label_w,
        value_w,
        cap,
    }
}

/// One tinted run of positioned glyphs, ready for `velvet_text::Run`.
#[derive(Debug, Clone)]
pub struct TintedRun {
    pub glyphs: Vec<PositionedGlyph>,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// Emit the plate and every text run for a Coords widget, in draw order.
///
/// Order is load-bearing: shadow copies before their glyph, and the whole
/// label before the value only because that is how the original reads. Each
/// shadow copy is a *separate run* rather than a re-tint of the same glyphs,
/// because the blurred copies are different atlas entries with different
/// boxes (see `GlyphCache::glyph_blurred`).
pub fn emit_coords(
    cache: &mut GlyphCache,
    layout: &CoordsLayout,
    value_text: &str,
) -> (Shell, Vec<TintedRun>) {
    let shell = Shell::plain(
        layout.chip[0],
        layout.chip[1],
        layout.chip[2],
        layout.chip[3],
        layout.radius,
    );
    let label_key = coords_label_key();
    let value_key = coords_value_key();
    let mut runs = Vec::with_capacity(8);

    for (sigma, alpha, dy) in IW_SHADOW {
        let mut g = Vec::new();
        cache.layout_run_blurred(
            label_key,
            COORDS_LABEL,
            COORDS_LABEL_TRACKING_EM,
            (layout.label_origin.0, layout.label_origin.1 + dy),
            sigma,
            &mut g,
        );
        runs.push(TintedRun { glyphs: g, color: WINE, alpha });
    }
    let mut g = Vec::new();
    cache.layout_run(
        label_key,
        COORDS_LABEL,
        COORDS_LABEL_TRACKING_EM,
        layout.label_origin,
        &mut g,
    );
    runs.push(TintedRun { glyphs: g, color: ROSE, alpha: 0.9 });

    for (sigma, alpha, dy) in IW_SHADOW {
        let mut g = Vec::new();
        cache.layout_run_blurred(
            value_key,
            value_text,
            0.0,
            (layout.value_origin.0, layout.value_origin.1 + dy),
            sigma,
            &mut g,
        );
        runs.push(TintedRun { glyphs: g, color: WINE, alpha });
    }
    let mut g = Vec::new();
    cache.layout_run(value_key, value_text, 0.0, layout.value_origin, &mut g);
    runs.push(TintedRun { glyphs: g, color: PEARL, alpha: 1.0 });

    (shell, runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> Option<GlyphCache> {
        let mut c = GlyphCache::new();
        for (fam, ital) in [(Family::Fraunces, false), (Family::JetBrainsMono, false)] {
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/fonts")
                .join(format!("{}.ttf", fam.file_stem(ital)));
            let data = std::fs::read(p).ok()?;
            c.load(fam, ital, data);
        }
        Some(c)
    }

    #[test]
    fn anchor_fractions_place_all_nine_corners() {
        // Tl puts the box's top-left at the anchor; Br puts its bottom-right
        // there; Mc centres it. Getting a fraction backwards moves a widget by
        // its own size, which looks like a layout bug somewhere else entirely.
        assert_eq!(Anchor::Tl.origin(100.0, 200.0, 40.0, 20.0), (100.0, 200.0));
        assert_eq!(Anchor::Br.origin(100.0, 200.0, 40.0, 20.0), (60.0, 180.0));
        assert_eq!(Anchor::Mc.origin(100.0, 200.0, 40.0, 20.0), (80.0, 190.0));
        assert_eq!(Anchor::Tr.origin(100.0, 200.0, 40.0, 20.0), (60.0, 200.0));
        assert_eq!(Anchor::Bl.origin(100.0, 200.0, 40.0, 20.0), (100.0, 180.0));
    }

    #[test]
    fn anchor_tokens_round_trip_for_hud_toml() {
        for a in [
            Anchor::Tl, Anchor::Tc, Anchor::Tr,
            Anchor::Ml, Anchor::Mc, Anchor::Mr,
            Anchor::Bl, Anchor::Bc, Anchor::Br,
        ] {
            assert_eq!(Anchor::from_str(a.as_str()), Some(a));
        }
        assert_eq!(Anchor::from_str("nope"), None);
    }

    #[test]
    fn the_value_format_is_one_decimal_rounded_y_and_two_spaces() {
        assert_eq!(coords_value_text(-128.44, 64.4, -1492.01), "-128.4  64  -1492.0");
        // y ROUNDS, it does not truncate.
        assert_eq!(coords_value_text(0.0, 63.7, 0.0), "0.0  64  0.0");
        assert!(
            coords_value_text(1.0, 2.0, 3.0).contains("  "),
            "the double space is deliberate, not a typo"
        );
    }

    #[test]
    fn the_chip_is_built_from_the_transcribed_chain() {
        let Some(mut c) = cache() else { return };
        let l = layout_coords(&mut c, -128.4, 64.0, -1492.0, Anchor::Tl, 0.0, 0.0);
        // chip_w = pad*2 + label + gap + value
        let expect_w = COORDS_PAD_X * 2.0 + l.label_w + COORDS_GAP + l.value_w;
        assert!((l.chip[2] - expect_w).abs() < 0.001, "{} vs {expect_w}", l.chip[2]);
        // chip_h = pad*2 + cap, from the VALUE font.
        assert!((l.chip[3] - (COORDS_PAD_Y * 2.0 + l.cap)).abs() < 0.001);
        // baseline = top + pad_y + cap
        assert!((l.label_origin.1 - (l.chip[1] + COORDS_PAD_Y + l.cap)).abs() < 0.001);
        // The two runs share a baseline.
        assert_eq!(l.label_origin.1, l.value_origin.1);
        // The value starts one gap past the label.
        let expect_x = l.chip[0] + COORDS_PAD_X + l.label_w + COORDS_GAP;
        assert!((l.value_origin.0 - expect_x).abs() < 0.001);
    }

    #[test]
    fn the_plate_is_sized_by_cap_height_not_line_height() {
        // A plate sized by ascent+descent is visibly taller and the vertical
        // centring drifts. Pin that the height is the smaller quantity.
        let Some(mut c) = cache() else { return };
        let m = c.metrics(coords_value_key()).unwrap();
        let l = layout_coords(&mut c, 0.0, 0.0, 0.0, Anchor::Tl, 0.0, 0.0);
        assert!(l.cap < m.ascent + m.descent, "cap {} vs line", l.cap);
        assert!((l.chip[3] - (16.0 + l.cap)).abs() < 0.001);
    }

    #[test]
    fn anchoring_bottom_right_moves_the_chip_not_its_internals() {
        let Some(mut c) = cache() else { return };
        let tl = layout_coords(&mut c, 1.0, 2.0, 3.0, Anchor::Tl, 500.0, 400.0);
        let br = layout_coords(&mut c, 1.0, 2.0, 3.0, Anchor::Br, 500.0, 400.0);
        assert_eq!(tl.chip[2], br.chip[2], "width must not depend on anchor");
        assert_eq!(tl.chip[3], br.chip[3]);
        // The internal offsets are identical relative to the chip.
        assert!(
            ((tl.label_origin.0 - tl.chip[0]) - (br.label_origin.0 - br.chip[0])).abs() < 0.001
        );
    }

    #[test]
    fn emit_produces_a_shadow_stack_under_each_run() {
        let Some(mut c) = cache() else { return };
        let text = coords_value_text(-128.4, 64.0, -1492.0);
        let l = layout_coords(&mut c, -128.4, 64.0, -1492.0, Anchor::Tl, 0.0, 0.0);
        let (shell, runs) = emit_coords(&mut c, &l, &text);
        assert_eq!(shell.rect, l.chip);
        // 3 shadows + label, 3 shadows + value.
        assert_eq!(runs.len(), 8);
        assert_eq!(runs[3].color, ROSE, "the label is rose");
        assert_eq!(runs[7].color, PEARL, "the value is pearl");
        for i in [0, 1, 2, 4, 5, 6] {
            assert_eq!(runs[i].color, WINE, "run {i} should be a wine shadow");
        }
        // Every run must actually carry glyphs, or a shadow is silently absent.
        for (i, r) in runs.iter().enumerate() {
            assert!(!r.glyphs.is_empty(), "run {i} is empty");
        }
    }

    #[test]
    fn a_blurred_shadow_run_tracks_its_glyphs_pen_for_pen() {
        // The shadow advances by the SHARP advance, so copy N sits under
        // glyph N. Advancing by the blurred box width would spread the shadow
        // wider than the text -- subtle, and very easy to accept by eye.
        let Some(mut c) = cache() else { return };
        let text = coords_value_text(-128.4, 64.0, -1492.0);
        let l = layout_coords(&mut c, -128.4, 64.0, -1492.0, Anchor::Tl, 0.0, 0.0);
        let (_, runs) = emit_coords(&mut c, &l, &text);
        let shadow = &runs[4]; // widest value shadow
        let sharp = &runs[7];
        assert_eq!(shadow.glyphs.len(), sharp.glyphs.len());
        let span = |r: &TintedRun| {
            let first = r.glyphs.first().unwrap();
            let last = r.glyphs.last().unwrap();
            (last.dst_x + last.dst_w / 2.0) - (first.dst_x + first.dst_w / 2.0)
        };
        assert!(
            (span(shadow) - span(sharp)).abs() < 0.01,
            "shadow span {} vs text span {}",
            span(shadow),
            span(sharp)
        );
    }
}
