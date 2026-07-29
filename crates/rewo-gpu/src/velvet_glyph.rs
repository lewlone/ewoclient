//! Velvet glyph cache (M52b step 1) — variable-font rasterization into a
//! shelf-packed coverage atlas.
//!
//! `REWO_VELVET_UI_PLAN.md` §2 is the spec. The short version of why this
//! exists: EwoClient's HUD widgets are drawn in Fraunces / Newsreader /
//! JetBrains Mono at non-default variable axes, and Rewo's only text renderer
//! is the vanilla 8px bitmap font. Porting the widgets needs a renderer first.
//!
//! ## Why rasterize rather than SDF
//!
//! An MSDF atlas is the reflexive answer for scalable GPU text and it is wrong
//! here. The fidelity target is *pixel-faithful against the Skia originals*,
//! and SDF reconstruction approximates the outline — it would differ at every
//! antialiased edge, and the gate could never be tight. It is also weakest at
//! small sizes, and this HUD is full of 9–12 px mono labels.
//!
//! Skia itself rasterizes and caches. Glyph raster is a **cache** problem, not
//! a per-frame problem: a cache hit makes the frame pure quads.
//!
//! ## The leak this is shaped to avoid
//!
//! CLAUDE.md records what the launcher paid to learn (2026-05-31):
//!
//! > never construct a Skia shader / image-filter / variable-font `Typeface`
//! > inside a per-frame draw — they allocate foreign C++/FreeType state that
//! > Skia's tracked caches don't bound.
//!
//! ~4 MB/s at 500 fps. The same hazard exists with any rasterizer, so the key
//! is **quantized** and everything expensive hangs off it: a `swash` scaler is
//! built per distinct (family, size, axes), never per draw and never per
//! glyph. Quantization is what keeps a slider that sweeps a size from
//! allocating a scaler per frame.

use std::collections::HashMap;

use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::Format;
use swash::{FontRef, Setting};

/// The three Velvet families. Italic is a separate file, not an axis, which is
/// how they ship in `assets/fonts/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Fraunces,
    Newsreader,
    JetBrainsMono,
}

impl Family {
    /// File stem in `assets/fonts/`, upright and italic.
    pub fn file_stem(self, italic: bool) -> &'static str {
        match (self, italic) {
            (Family::Fraunces, false) => "Fraunces",
            (Family::Fraunces, true) => "Fraunces-Italic",
            (Family::Newsreader, false) => "Newsreader",
            (Family::Newsreader, true) => "Newsreader-Italic",
            (Family::JetBrainsMono, false) => "JetBrainsMono",
            (Family::JetBrainsMono, true) => "JetBrainsMono-Italic",
        }
    }
}

/// Variable-axis settings, in the units the font declares.
///
/// Stored as an ordered fixed list rather than a map so the type is `Copy` and
/// hashable — this is a cache key on the hot path. `None` means "leave at the
/// font's default", which is **not** the same as zero: Fraunces' `WONK`
/// defaults to 0 but `opsz` does not, and forcing an unset axis to 0 would
/// silently pick a different design instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axes {
    pub soft: Option<f32>,
    pub wonk: Option<f32>,
    pub opsz: Option<f32>,
    pub wght: Option<f32>,
}

impl Axes {
    pub const DEFAULT: Axes = Axes { soft: None, wonk: None, opsz: None, wght: None };

    /// `ewo_render::FontStore::fraunces_axes`'s signature, so a widget
    /// transcription reads the same on both sides.
    pub fn fraunces(soft: f32, wonk: f32, wght: f32, opsz: Option<f32>) -> Self {
        Axes { soft: Some(soft), wonk: Some(wonk), opsz, wght: Some(wght) }
    }

    fn settings(self) -> Vec<Setting<f32>> {
        let mut v = Vec::with_capacity(4);
        if let Some(x) = self.soft {
            v.push(Setting { tag: tag(b"SOFT"), value: x });
        }
        if let Some(x) = self.wonk {
            v.push(Setting { tag: tag(b"WONK"), value: x });
        }
        if let Some(x) = self.opsz {
            v.push(Setting { tag: tag(b"opsz"), value: x });
        }
        if let Some(x) = self.wght {
            v.push(Setting { tag: tag(b"wght"), value: x });
        }
        v
    }
}

const fn tag(b: &[u8; 4]) -> u32 {
    ((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | (b[3] as u32)
}

/// Quantization step for the size key, in pixels.
///
/// A widget's `scale` multiplier is continuous over 0.5..3.0, so an
/// unquantized size would mint a fresh scaler on every frame of a drag. An
/// eighth of a pixel is finer than the rasterizer's own hinting grid, so the
/// visual cost is nil and the cache actually hits.
pub const SIZE_QUANTUM: f32 = 0.125;

/// Quantization step for an axis coordinate. Axes are design-space units
/// (SOFT 0..100, wght 100..900) where a fractional step is invisible.
pub const AXIS_QUANTUM: f32 = 0.5;

fn q(v: f32, step: f32) -> i32 {
    (v / step).round() as i32
}

/// The cache key. Everything expensive hangs off this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScalerKey {
    family: Family,
    italic: bool,
    size_q: i32,
    soft_q: Option<i32>,
    wonk_q: Option<i32>,
    opsz_q: Option<i32>,
    wght_q: Option<i32>,
}

impl ScalerKey {
    pub fn new(family: Family, italic: bool, size_px: f32, axes: Axes) -> Self {
        Self {
            family,
            italic,
            size_q: q(size_px, SIZE_QUANTUM),
            soft_q: axes.soft.map(|v| q(v, AXIS_QUANTUM)),
            wonk_q: axes.wonk.map(|v| q(v, AXIS_QUANTUM)),
            opsz_q: axes.opsz.map(|v| q(v, AXIS_QUANTUM)),
            wght_q: axes.wght.map(|v| q(v, AXIS_QUANTUM)),
        }
    }

    /// The size this key actually rasterizes at (post-quantization).
    pub fn size_px(self) -> f32 {
        self.size_q as f32 * SIZE_QUANTUM
    }
}

/// A rasterized glyph's place in the atlas, plus the metrics layout needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    /// Atlas rect in pixels.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Offset from the pen position to the bitmap's top-left, in pixels.
    /// `top` is positive *up* from the baseline, matching the rasterizer's
    /// placement convention.
    pub left: i32,
    pub top: i32,
    /// Horizontal advance in pixels, before any tracking is added.
    pub advance: f32,
}

/// Per-(family, size, axes) metrics. Layout uses **cap height**, not the em
/// box — `draw_coords` computes its baseline as `top + pad_y + cap`, so a
/// wrong cap height shifts every widget vertically.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
    pub cap_height: f32,
    pub units_per_em: f32,
}

/// One loaded font file.
struct Face {
    data: Vec<u8>,
    offset: u32,
}

impl Face {
    fn font(&self) -> FontRef<'_> {
        FontRef::from_index(&self.data, self.offset as usize)
            .expect("face validated at load")
    }
}

/// A shelf packer, the same shape the entity atlas already uses in `mobs.rs`.
///
/// Glyphs arrive in wildly mixed heights, so a shelf wastes a little vertical
/// space per row. That is the right trade here: the alternative (a full
/// bin-packer with repacking) buys a few percent of a 1 MB atlas and costs a
/// class of bugs where a glyph moves after it has been referenced.
struct Shelf {
    width: u32,
    height: u32,
    pen_x: u32,
    pen_y: u32,
    row_h: u32,
}

impl Shelf {
    fn new(width: u32, height: u32) -> Self {
        Self { width, height, pen_x: 0, pen_y: 0, row_h: 0 }
    }

    /// Reserve `w × h`, returning its origin. `None` when the atlas is full —
    /// the caller grows rather than evicting (see the module docs).
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.width {
            return None;
        }
        if self.pen_x + w > self.width {
            self.pen_y += self.row_h;
            self.pen_x = 0;
            self.row_h = 0;
        }
        if self.pen_y + h > self.height {
            return None;
        }
        let at = (self.pen_x, self.pen_y);
        self.pen_x += w;
        self.row_h = self.row_h.max(h);
        Some(at)
    }
}

/// Initial atlas edge. Grows by doubling; never evicts.
pub const ATLAS_START: u32 = 1024;
/// Refuse to grow past this — a HUD that needs 16 MB of glyphs is a bug, and
/// failing loudly beats silently allocating.
pub const ATLAS_MAX: u32 = 4096;

/// The glyph cache: faces, scalers, the coverage atlas, and the glyph map.
pub struct GlyphCache {
    faces: HashMap<(Family, bool), Face>,
    ctx: ScaleContext,
    glyphs: HashMap<(ScalerKey, u16, u16), Glyph>,
    metrics: HashMap<ScalerKey, Metrics>,
    /// R8 coverage. Colour is a vertex attribute, so one atlas serves every
    /// tint and the shadow copies cost nothing extra.
    pixels: Vec<u8>,
    edge: u32,
    shelf: Shelf,
    dirty: bool,
}

impl GlyphCache {
    pub fn new() -> Self {
        Self {
            faces: HashMap::new(),
            ctx: ScaleContext::new(),
            glyphs: HashMap::new(),
            metrics: HashMap::new(),
            pixels: vec![0; (ATLAS_START * ATLAS_START) as usize],
            edge: ATLAS_START,
            shelf: Shelf::new(ATLAS_START, ATLAS_START),
            dirty: false,
        }
    }

    /// Register a font file. Returns `false` if the bytes are not a font this
    /// build can read — the caller decides whether that is fatal.
    pub fn load(&mut self, family: Family, italic: bool, data: Vec<u8>) -> bool {
        let Some(font) = FontRef::from_index(&data, 0) else {
            return false;
        };
        let offset = font.offset;
        self.faces.insert((family, italic), Face { data, offset });
        true
    }

    pub fn is_loaded(&self, family: Family, italic: bool) -> bool {
        self.faces.contains_key(&(family, italic))
    }

    /// Atlas edge length in pixels (square).
    pub fn atlas_edge(&self) -> u32 {
        self.edge
    }

    /// The coverage bytes, row-major, `edge × edge`.
    pub fn atlas(&self) -> &[u8] {
        &self.pixels
    }

    /// Has the atlas changed since `clear_dirty`? Drives re-upload.
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Map a character to a glyph id in this family.
    pub fn glyph_id(&self, family: Family, italic: bool, ch: char) -> Option<u16> {
        let face = self.faces.get(&(family, italic))?;
        let id = face.font().charmap().map(ch);
        (id != 0).then_some(id)
    }

    /// Scaled metrics for a key, cached.
    pub fn metrics(&mut self, key: ScalerKey) -> Option<Metrics> {
        if let Some(m) = self.metrics.get(&key) {
            return Some(*m);
        }
        let coords = self.coords_of(key);
        let face = self.faces.get(&(key.family, key.italic))?;
        let font = face.font();
        // `scale(ppem)` divides by units_per_em; `linear_scale` does NOT --
        // it multiplies by a raw factor, so passing a pixel size returns
        // font units (Fraunces cap height came out 25200 instead of 12.6).
        let m = font.metrics(&coords).scale(key.size_px());
        let out = Metrics {
            ascent: m.ascent,
            descent: m.descent,
            line_height: m.ascent + m.descent + m.leading,
            // Fonts may omit `sCapHeight`. Skia falls back to ~0.72 em and the
            // widget layout was calibrated against that, so match it rather
            // than inventing a different fallback.
            cap_height: if m.cap_height > 0.0 {
                m.cap_height
            } else {
                key.size_px() * 0.72
            },
            units_per_em: m.units_per_em as f32,
        };
        self.metrics.insert(key, out);
        Some(out)
    }

    fn axes_of(&self, key: ScalerKey) -> Vec<Setting<f32>> {
        Axes {
            soft: key.soft_q.map(|v| v as f32 * AXIS_QUANTUM),
            wonk: key.wonk_q.map(|v| v as f32 * AXIS_QUANTUM),
            opsz: key.opsz_q.map(|v| v as f32 * AXIS_QUANTUM),
            wght: key.wght_q.map(|v| v as f32 * AXIS_QUANTUM),
        }
        .settings()
    }

    /// `metrics`/`glyph_metrics` want **normalized** coordinates (i16 in
    /// 2.14 fixed point), not the design-space `Setting<f32>` the scaler
    /// builder takes. Mixing the two compiles nowhere useful and, worse, a
    /// silently empty coord slice would return the font's DEFAULT instance
    /// metrics -- so a widget asking for wght 500 would lay out against
    /// wght 400 while rendering at 500.
    fn coords_of(&self, key: ScalerKey) -> Vec<swash::NormalizedCoord> {
        let Some(face) = self.faces.get(&(key.family, key.italic)) else {
            return Vec::new();
        };
        let settings = self.axes_of(key);
        face.font()
            .variations()
            .normalized_coords(settings.iter().copied())
            .collect()
    }

    /// Rasterize (or fetch) one glyph.
    ///
    /// A glyph with no outline — a space — caches as a zero-area entry with a
    /// real advance. That is deliberate: it keeps the advance path uniform, so
    /// layout never special-cases whitespace.
    pub fn glyph(&mut self, key: ScalerKey, glyph_id: u16) -> Option<Glyph> {
        self.glyph_blurred(key, glyph_id, 0.0)
    }

    /// A glyph, optionally Gaussian-blurred by `sigma` pixels.
    ///
    /// The Velvet in-world text shadow is three copies -- WINE at blur 5,
    /// WINE at blur 3, and a hard copy offset +1y. A blurred glyph is still a
    /// glyph, so it belongs in the same cache under the same discipline: blur
    /// the coverage **once**, at rasterization time, keyed by a quantized
    /// sigma. Blurring per frame in a shader would need a multi-tap kernel per
    /// glyph quad and would redo identical work forever.
    ///
    /// Skia's `MaskFilter::blur(BlurStyle::Normal, r)` takes a *radius*; the
    /// conversion the launcher settled on, and CLAUDE.md records, is
    /// `sigma = radius / 2`. Take sigma here and convert at the call site, so
    /// a widget transcription can keep quoting Skia's radius verbatim.
    pub fn glyph_blurred(&mut self, key: ScalerKey, glyph_id: u16, sigma: f32) -> Option<Glyph> {
        let blur_q = (sigma / BLUR_QUANTUM).round().max(0.0) as u16;
        if let Some(g) = self.glyphs.get(&(key, glyph_id, blur_q)) {
            return Some(*g);
        }
        if blur_q > 0 {
            return self.rasterize_blurred(key, glyph_id, blur_q);
        }
        let coords = self.coords_of(key);
        let settings = self.axes_of(key);
        let face = self.faces.get(&(key.family, key.italic))?;
        let font = face.font();

        // `scale`, not `linear_scale` -- see the note in `metrics`. The old
        // call made every advance ~1400x too wide, which the original
        // `advance > 0.0` assertion happily accepted.
        let advance = font
            .glyph_metrics(&coords)
            .scale(key.size_px())
            .advance_width(glyph_id)
            .max(0.0);

        let mut scaler = self
            .ctx
            .builder(font)
            .size(key.size_px())
            .variations(settings.iter().copied())
            .hint(true)
            .build();

        let image = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .render(&mut scaler, glyph_id);

        let (w, h, left, top, data) = match image {
            Some(img) if img.placement.width > 0 && img.placement.height > 0 => (
                img.placement.width,
                img.placement.height,
                img.placement.left,
                img.placement.top,
                img.data,
            ),
            // No outline (space) or a failed render: a zero-area entry that
            // still advances.
            _ => (0, 0, 0, 0, Vec::new()),
        };

        let (x, y) = if w == 0 || h == 0 {
            (0, 0)
        } else {
            // 1px gutter so bilinear sampling of one glyph cannot bleed into
            // its neighbour.
            let at = self.alloc_or_grow(w + 1, h + 1)?;
            self.blit(at.0, at.1, w, h, &data);
            at
        };

        let g = Glyph { x, y, w, h, left, top, advance };
        self.glyphs.insert((key, glyph_id, 0), g);
        Some(g)
    }

    fn alloc_or_grow(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if let Some(at) = self.shelf.alloc(w, h) {
            return Some(at);
        }
        // Grow by doubling and re-pack from scratch. Every cached glyph rect
        // is invalidated, which is why this drops the maps rather than trying
        // to move entries: a stale rect that survives a repack is a corruption
        // bug, and re-rasterizing a few hundred glyphs is microseconds.
        let next = self.edge.saturating_mul(2);
        if next > ATLAS_MAX {
            log::error!("velvet glyph atlas exhausted at {}px", self.edge);
            return None;
        }
        log::info!("velvet glyph atlas growing {} -> {}", self.edge, next);
        self.edge = next;
        self.pixels = vec![0; (next * next) as usize];
        self.shelf = Shelf::new(next, next);
        self.glyphs.clear();
        self.dirty = true;
        self.shelf.alloc(w, h)
    }

    fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        for row in 0..h {
            let src = (row * w) as usize;
            let dst = ((y + row) * self.edge + x) as usize;
            let n = w as usize;
            if src + n <= data.len() && dst + n <= self.pixels.len() {
                self.pixels[dst..dst + n].copy_from_slice(&data[src..src + n]);
            }
        }
        self.dirty = true;
    }

    /// Total advance of a string with CSS-style `letter-spacing` in em.
    ///
    /// Mirrors `ewo_render::text::measure_tracked_em`. **Tracking is added
    /// after every glyph including the last** — that is what the Skia side
    /// does, and it is why a tracked label's chip is a hair wider than the ink.
    /// Matching the quirk matters more than fixing it: the widget layout
    /// constants were calibrated against it.
    pub fn measure_tracked(&mut self, key: ScalerKey, text: &str, tracking_em: f32) -> f32 {
        let track = tracking_em * key.size_px();
        let mut w = 0.0;
        for ch in text.chars() {
            if let Some(id) = self.glyph_id(key.family, key.italic, ch) {
                if let Some(g) = self.glyph(key, id) {
                    w += g.advance;
                }
            }
            w += track;
        }
        w
    }
}

/// One glyph placed on screen: a destination rect in pixels and its atlas UVs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub dst_x: f32,
    pub dst_y: f32,
    pub dst_w: f32,
    pub dst_h: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl GlyphCache {
    /// Lay out a run at a **baseline** origin, mirroring
    /// `ewo_render::text::draw_tracked_em` exactly:
    ///
    /// ```text
    /// spacing = letter_spacing_em * font.size()
    /// for ch: draw at (cur_x, baseline_y); cur_x += advance + spacing
    /// ```
    ///
    /// Two conventions that are easy to get backwards and produce plausible
    /// but wrong output:
    ///
    /// * `origin.1` is the **baseline**, not the top of the text. Every widget
    ///   computes it as `top + pad_y + cap_height`, so treating it as a top
    ///   edge shifts the run down by a cap height.
    /// * `Glyph::top` is positive **up** from the baseline (the rasterizer's
    ///   placement convention), so the destination top is
    ///   `baseline - top`, a subtraction. Adding it flips every glyph across
    ///   the baseline, which looks like a font-loading bug rather than a sign
    ///   error.
    ///
    /// Returns the total advance, which equals `measure_tracked` for the same
    /// arguments — the two are tested against each other so layout and
    /// measurement cannot drift.
    pub fn layout_run(
        &mut self,
        key: ScalerKey,
        text: &str,
        tracking_em: f32,
        origin: (f32, f32),
        out: &mut Vec<PositionedGlyph>,
    ) -> f32 {
        self.layout_run_blurred(key, text, tracking_em, origin, 0.0, out)
    }

    /// `layout_run` with every glyph fetched at a blur sigma -- the shadow
    /// stack's copies.
    ///
    /// The pen still advances by the **sharp** advance, because
    /// `glyph_blurred` preserves it. Advancing by the blurred box width
    /// instead would spread the shadow wider than the text it belongs to,
    /// which is subtle enough to accept by eye and wrong.
    pub fn layout_run_blurred(
        &mut self,
        key: ScalerKey,
        text: &str,
        tracking_em: f32,
        origin: (f32, f32),
        sigma: f32,
        out: &mut Vec<PositionedGlyph>,
    ) -> f32 {
        let spacing = tracking_em * key.size_px();
        let inv = 1.0 / self.edge as f32;
        let (mut pen_x, baseline_y) = origin;
        let start = pen_x;
        for ch in text.chars() {
            let Some(id) = self.glyph_id(key.family, key.italic, ch) else {
                pen_x += spacing;
                continue;
            };
            let Some(g) = self.glyph_blurred(key, id, sigma) else {
                pen_x += spacing;
                continue;
            };
            if g.w > 0 && g.h > 0 {
                out.push(PositionedGlyph {
                    dst_x: pen_x + g.left as f32,
                    dst_y: baseline_y - g.top as f32,
                    dst_w: g.w as f32,
                    dst_h: g.h as f32,
                    u0: g.x as f32 * inv,
                    v0: g.y as f32 * inv,
                    u1: (g.x + g.w) as f32 * inv,
                    v1: (g.y + g.h) as f32 * inv,
                });
            }
            pen_x += g.advance + spacing;
        }
        pen_x - start
    }
}

/// Quantization step for a blur sigma, in pixels. Coarser than the size step:
/// a shadow is diffuse by construction, so an eighth-pixel sigma difference is
/// not merely invisible, it is meaningless.
pub const BLUR_QUANTUM: f32 = 0.25;

impl GlyphCache {
    /// Rasterize the sharp glyph, grow its box by the blur support, convolve,
    /// and pack the result as its own atlas entry.
    fn rasterize_blurred(&mut self, key: ScalerKey, glyph_id: u16, blur_q: u16) -> Option<Glyph> {
        let sigma = blur_q as f32 * BLUR_QUANTUM;
        let sharp = self.glyph(key, glyph_id)?;
        if sharp.w == 0 || sharp.h == 0 {
            // Nothing to blur, but the advance must survive.
            self.glyphs.insert((key, glyph_id, blur_q), sharp);
            return Some(sharp);
        }
        // 3 sigma captures >99% of a Gaussian; past that the tail is below one
        // 8-bit step and only costs atlas area.
        let pad = (sigma * 3.0).ceil() as u32;
        let (bw, bh) = (sharp.w + pad * 2, sharp.h + pad * 2);

        let mut src = vec![0f32; (bw * bh) as usize];
        for r in 0..sharp.h {
            for c in 0..sharp.w {
                let a = self.pixels[((sharp.y + r) * self.edge + sharp.x + c) as usize];
                src[((r + pad) * bw + c + pad) as usize] = a as f32;
            }
        }
        let kernel = gaussian_kernel(sigma);
        let half = (kernel.len() / 2) as i32;
        let (iw, ih) = (bw as i32, bh as i32);
        // Separable: horizontal into `tmp`, then vertical back into `src`.
        let mut tmp = vec![0f32; src.len()];
        for r in 0..ih {
            for c in 0..iw {
                let mut acc = 0.0;
                for (i, k) in kernel.iter().enumerate() {
                    let x = (c + i as i32 - half).clamp(0, iw - 1);
                    acc += src[(r * iw + x) as usize] * k;
                }
                tmp[(r * iw + c) as usize] = acc;
            }
        }
        for c in 0..iw {
            for r in 0..ih {
                let mut acc = 0.0;
                for (i, k) in kernel.iter().enumerate() {
                    let y = (r + i as i32 - half).clamp(0, ih - 1);
                    acc += tmp[(y * iw + c) as usize] * k;
                }
                src[(r * iw + c) as usize] = acc;
            }
        }
        let bytes: Vec<u8> = src.iter().map(|v| v.clamp(0.0, 255.0) as u8).collect();

        let (x, y) = self.alloc_or_grow(bw + 1, bh + 1)?;
        self.blit(x, y, bw, bh, &bytes);
        let g = Glyph {
            x,
            y,
            w: bw,
            h: bh,
            // The box grew by `pad` on every side, so the placement shifts by
            // the same amount: left back, top up. Without this the shadow sits
            // down-right of its glyph by the blur support, which reads as a
            // deliberate offset shadow and is very easy to accept by eye.
            left: sharp.left - pad as i32,
            top: sharp.top + pad as i32,
            advance: sharp.advance,
        };
        self.glyphs.insert((key, glyph_id, blur_q), g);
        Some(g)
    }
}

/// A normalized 1D Gaussian, truncated at 3 sigma.
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius = (sigma * 3.0).ceil().max(1.0) as i32;
    let two_s2 = 2.0 * sigma * sigma;
    let mut k: Vec<f32> = (-radius..=radius)
        .map(|i| (-((i * i) as f32) / two_s2).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    for v in &mut k {
        *v /= sum;
    }
    k
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font_bytes(stem: &str) -> Option<Vec<u8>> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts")
            .join(format!("{stem}.ttf"));
        std::fs::read(p).ok()
    }

    fn loaded() -> Option<GlyphCache> {
        let mut c = GlyphCache::new();
        for (fam, ital) in [
            (Family::Fraunces, false),
            (Family::Newsreader, false),
            (Family::JetBrainsMono, false),
        ] {
            let data = font_bytes(fam.file_stem(ital))?;
            assert!(c.load(fam, ital, data), "{:?} failed to parse", fam);
        }
        Some(c)
    }

    #[test]
    fn quantization_collapses_a_drag_into_one_scaler() {
        // A widget scale slider sweeping 18.00 -> 18.04 must not mint a new
        // scaler per frame; that is the shape of the launcher's 4 MB/s leak.
        let a = ScalerKey::new(Family::Fraunces, false, 18.00, Axes::DEFAULT);
        let b = ScalerKey::new(Family::Fraunces, false, 18.04, Axes::DEFAULT);
        assert_eq!(a, b);
        // ...but a real size change still separates.
        let c = ScalerKey::new(Family::Fraunces, false, 18.5, Axes::DEFAULT);
        assert_ne!(a, c);
    }

    #[test]
    fn an_unset_axis_is_not_the_same_as_zero() {
        let unset = ScalerKey::new(Family::Fraunces, false, 18.0, Axes::DEFAULT);
        let zeroed = ScalerKey::new(
            Family::Fraunces,
            false,
            18.0,
            Axes { soft: Some(0.0), ..Axes::DEFAULT },
        );
        assert_ne!(
            unset, zeroed,
            "forcing an unset axis to 0 picks a different design instance"
        );
    }

    #[test]
    fn shelf_wraps_rows_and_reports_exhaustion() {
        let mut s = Shelf::new(10, 10);
        assert_eq!(s.alloc(6, 4), Some((0, 0)));
        // Does not fit beside it: wraps to the next shelf at the row height.
        assert_eq!(s.alloc(6, 4), Some((0, 4)));
        // The third wraps to y=8, where a 4-tall row runs past the 10px
        // bottom edge -- so it must fail rather than overflow.
        assert_eq!(s.alloc(6, 4), None, "past the bottom edge");
        assert_eq!(s.alloc(20, 1), None, "wider than the atlas");
    }

    #[test]
    fn fonts_load_and_map_characters() {
        let Some(c) = loaded() else {
            eprintln!("assets/fonts missing — skipping");
            return;
        };
        for fam in [Family::Fraunces, Family::Newsreader, Family::JetBrainsMono] {
            assert!(c.is_loaded(fam, false));
            assert!(c.glyph_id(fam, false, 'X').is_some(), "{fam:?} has no X");
        }
    }

    #[test]
    fn cap_height_is_positive_and_below_the_size() {
        // Layout computes `baseline = top + pad_y + cap`. A zero or em-sized
        // cap height shifts every widget vertically, so pin the sane band.
        let Some(mut c) = loaded() else { return };
        for fam in [Family::Fraunces, Family::Newsreader, Family::JetBrainsMono] {
            let k = ScalerKey::new(fam, false, 18.0, Axes::DEFAULT);
            let m = c.metrics(k).expect("metrics");
            assert!(m.cap_height > 0.0, "{fam:?} cap height {}", m.cap_height);
            assert!(m.cap_height < 18.0, "{fam:?} cap height {}", m.cap_height);
            assert!(m.ascent > 0.0 && m.units_per_em > 0.0);
        }
    }

    #[test]
    fn a_glyph_rasterizes_with_ink_and_an_advance() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Fraunces, false, 32.0, Axes::DEFAULT);
        let id = c.glyph_id(Family::Fraunces, false, 'X').unwrap();
        let g = c.glyph(k, id).expect("render");
        assert!(g.w > 0 && g.h > 0, "no bitmap: {g:?}");
        // Bounded on BOTH sides. `> 0.0` alone accepted an advance in font
        // units -- ~1400x too wide -- for as long as it took the cap-height
        // test to notice. An 'X' at 32px advances on the order of 20px.
        assert!(
            (5.0..64.0).contains(&g.advance),
            "advance {} is not a plausible pixel advance at 32px",
            g.advance
        );
        // The atlas must actually contain ink.
        let any = (0..g.h).any(|r| {
            (0..g.w).any(|col| c.atlas()[((g.y + r) * c.atlas_edge() + g.x + col) as usize] > 0)
        });
        assert!(any, "glyph rect is blank");
        assert!(c.dirty(), "atlas write did not mark dirty");
    }

    #[test]
    fn variable_axes_change_the_rendering() {
        // If the axes were silently ignored, Velvet's whole visual identity
        // would be wrong in a way no screenshot would obviously show. Compare
        // the extremes of Fraunces' weight axis.
        let Some(mut c) = loaded() else { return };
        let id = c.glyph_id(Family::Fraunces, false, 'X').unwrap();
        let light = ScalerKey::new(
            Family::Fraunces,
            false,
            48.0,
            Axes::fraunces(0.0, 0.0, 100.0, None),
        );
        let heavy = ScalerKey::new(
            Family::Fraunces,
            false,
            48.0,
            Axes::fraunces(0.0, 0.0, 900.0, None),
        );
        let a = c.glyph(light, id).expect("light");
        let b = c.glyph(heavy, id).expect("heavy");
        let ink = |g: Glyph, c: &GlyphCache| -> u32 {
            (0..g.h)
                .map(|r| {
                    (0..g.w)
                        .filter(|col| {
                            c.atlas()[((g.y + r) * c.atlas_edge() + g.x + col) as usize] > 128
                        })
                        .count() as u32
                })
                .sum()
        };
        assert!(
            ink(b, &c) > ink(a, &c),
            "wght 900 should lay down more ink than wght 100 ({} vs {})",
            ink(b, &c),
            ink(a, &c)
        );
    }

    #[test]
    fn tracking_widens_a_string_by_exactly_one_step_per_character() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::JetBrainsMono, false, 12.0, Axes::DEFAULT);
        let plain = c.measure_tracked(k, "XYZ", 0.0);
        let tracked = c.measure_tracked(k, "XYZ", 0.22);
        let step = 0.22 * k.size_px();
        // Three characters, and the Skia original adds tracking after the last
        // one too — matching that quirk is what keeps chip widths equal.
        assert!(
            (tracked - plain - step * 3.0).abs() < 0.01,
            "plain {plain}, tracked {tracked}, step {step}"
        );
    }

    #[test]
    fn a_space_caches_as_zero_area_with_a_real_advance() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Newsreader, false, 14.0, Axes::DEFAULT);
        let id = c.glyph_id(Family::Newsreader, false, ' ').unwrap();
        let g = c.glyph(k, id).expect("space");
        assert_eq!((g.w, g.h), (0, 0));
        assert!(g.advance > 0.0, "space must still advance");
    }

    #[test]
    fn the_second_lookup_is_a_cache_hit() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Fraunces, false, 20.0, Axes::DEFAULT);
        let id = c.glyph_id(Family::Fraunces, false, 'A').unwrap();
        let first = c.glyph(k, id).unwrap();
        c.clear_dirty();
        let second = c.glyph(k, id).unwrap();
        assert_eq!(first, second);
        assert!(!c.dirty(), "a cache hit must not touch the atlas");
    }

    #[test]
    fn layout_and_measure_cannot_drift() {
        // Same arguments must produce the same width through both paths, or a
        // chip would be sized by one rule and filled by another.
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::JetBrainsMono, false, 12.0, Axes::DEFAULT);
        let mut out = Vec::new();
        let laid = c.layout_run(k, "XYZ", 0.22, (0.0, 20.0), &mut out);
        let measured = c.measure_tracked(k, "XYZ", 0.22);
        assert!((laid - measured).abs() < 0.001, "{laid} vs {measured}");
    }

    #[test]
    fn the_origin_is_the_baseline_and_top_is_up() {
        // A capital sits ABOVE the baseline, so its destination top must be
        // less than the origin y. Adding `top` instead of subtracting would
        // put it below and still look like "text, somewhere".
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Fraunces, false, 32.0, Axes::DEFAULT);
        let mut out = Vec::new();
        c.layout_run(k, "X", 0.0, (10.0, 100.0), &mut out);
        let g = out[0];
        assert!(g.dst_y < 100.0, "glyph top {} not above baseline", g.dst_y);
        let bottom = g.dst_y + g.dst_h;
        // An 'X' has no descender: its bottom sits on the baseline, within a
        // pixel of rounding.
        assert!(
            (bottom - 100.0).abs() <= 1.5,
            "'X' bottom {bottom} should rest on the baseline 100"
        );
    }

    #[test]
    fn tracking_moves_the_second_glyph_by_exactly_one_step() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::JetBrainsMono, false, 16.0, Axes::DEFAULT);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        c.layout_run(k, "AB", 0.0, (0.0, 40.0), &mut a);
        c.layout_run(k, "AB", 0.25, (0.0, 40.0), &mut b);
        let step = 0.25 * k.size_px();
        assert!((a[0].dst_x - b[0].dst_x).abs() < 0.001, "first glyph moved");
        assert!(
            ((b[1].dst_x - a[1].dst_x) - step).abs() < 0.01,
            "second glyph moved by {} not {step}",
            b[1].dst_x - a[1].dst_x
        );
    }

    #[test]
    fn uvs_stay_inside_the_atlas() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Newsreader, false, 24.0, Axes::DEFAULT);
        let mut out = Vec::new();
        c.layout_run(k, "The quick brown fox", 0.0, (0.0, 40.0), &mut out);
        assert!(!out.is_empty());
        for g in &out {
            assert!((0.0..=1.0).contains(&g.u0) && (0.0..=1.0).contains(&g.u1));
            assert!((0.0..=1.0).contains(&g.v0) && (0.0..=1.0).contains(&g.v1));
            assert!(g.u1 > g.u0 && g.v1 > g.v0, "degenerate uv {g:?}");
        }
    }

    #[test]
    fn a_space_emits_no_quad_but_still_advances() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Newsreader, false, 18.0, Axes::DEFAULT);
        let mut out = Vec::new();
        let w = c.layout_run(k, "A A", 0.0, (0.0, 30.0), &mut out);
        assert_eq!(out.len(), 2, "the space must not emit a quad");
        assert!(out[1].dst_x > out[0].dst_x + 1.0, "the space did not advance");
        assert!(w > 0.0);
    }

    fn atlas_sum(g: Glyph, c: &GlyphCache) -> f64 {
        (0..g.h)
            .map(|r| {
                (0..g.w)
                    .map(|col| c.atlas()[((g.y + r) * c.atlas_edge() + g.x + col) as usize] as f64)
                    .sum::<f64>()
            })
            .sum()
    }

    fn atlas_peak(g: Glyph, c: &GlyphCache) -> u8 {
        (0..g.h)
            .flat_map(|r| (0..g.w).map(move |col| (r, col)))
            .map(|(r, col)| c.atlas()[((g.y + r) * c.atlas_edge() + g.x + col) as usize])
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn a_blurred_glyph_is_bigger_softer_and_conserves_energy() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Fraunces, false, 32.0, Axes::DEFAULT);
        let id = c.glyph_id(Family::Fraunces, false, 'X').unwrap();
        let sharp = c.glyph(k, id).unwrap();
        let sharp_sum = atlas_sum(sharp, &c);
        let sharp_peak = atlas_peak(sharp, &c);

        let blurred = c.glyph_blurred(k, id, 2.5).unwrap();
        assert!(blurred.w > sharp.w && blurred.h > sharp.h, "box did not grow");
        assert!(atlas_peak(blurred, &c) < sharp_peak, "blur did not soften the peak");
        // A normalized kernel conserves total coverage (edge clamping adds a
        // little). A kernel that failed to normalize would show up here as a
        // wildly wrong sum rather than as a subtly wrong-looking shadow.
        let blur_sum = atlas_sum(blurred, &c);
        assert!(
            blur_sum > sharp_sum * 0.85 && blur_sum < sharp_sum * 1.30,
            "energy {blur_sum} vs {sharp_sum}"
        );
        // A shadow copy has to sit exactly under its glyph, so the advance
        // must not move.
        assert_eq!(blurred.advance, sharp.advance);
    }

    #[test]
    fn the_blur_offset_keeps_the_shadow_centred_on_its_glyph() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Fraunces, false, 32.0, Axes::DEFAULT);
        let id = c.glyph_id(Family::Fraunces, false, 'O').unwrap();
        let sharp = c.glyph(k, id).unwrap();
        let blurred = c.glyph_blurred(k, id, 2.0).unwrap();
        let sharp_cx = sharp.left as f32 + sharp.w as f32 / 2.0;
        let blur_cx = blurred.left as f32 + blurred.w as f32 / 2.0;
        let sharp_cy = -(sharp.top as f32) + sharp.h as f32 / 2.0;
        let blur_cy = -(blurred.top as f32) + blurred.h as f32 / 2.0;
        assert!((sharp_cx - blur_cx).abs() < 0.6, "x drift {sharp_cx} vs {blur_cx}");
        assert!((sharp_cy - blur_cy).abs() < 0.6, "y drift {sharp_cy} vs {blur_cy}");
    }

    #[test]
    fn blur_sigma_is_quantized_like_everything_else() {
        let Some(mut c) = loaded() else { return };
        let k = ScalerKey::new(Family::Newsreader, false, 16.0, Axes::DEFAULT);
        let id = c.glyph_id(Family::Newsreader, false, 'm').unwrap();
        let a = c.glyph_blurred(k, id, 2.50).unwrap();
        c.clear_dirty();
        let b = c.glyph_blurred(k, id, 2.51).unwrap();
        assert_eq!(a, b);
        assert!(!c.dirty(), "a quantized-equal sigma re-rasterized");
    }
}
