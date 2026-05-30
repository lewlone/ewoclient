//! Text engine — font loading and shaping.
//!
//! Step 7+ scaffolding: minimal version sufficient to render the "EwoClient"
//! heading on the main menu. Full per-glyph control (for breathing text,
//! sheen sweeps, hover-glow stagger) is added in step 9.
//!
//! Fonts live in `assets/fonts/` at the workspace root. The store searches
//! for `Fraunces*.ttf` / `Newsreader*.ttf` / `JetBrainsMono*.ttf` filenames
//! and falls back to the system default if missing — so the app boots fine
//! before fonts are dropped in, just with non-target typography.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use skia_safe::font_arguments::variation_position::Coordinate;
use skia_safe::font_arguments::VariationPosition;
use skia_safe::{Canvas, Font, FontArguments, FontMgr, FontStyle, FourByteTag, Paint, Typeface};

pub struct FontStore {
    pub fraunces: Typeface,
    pub has_fraunces: bool,
    pub newsreader: Typeface,
    pub has_newsreader: bool,
    pub newsreader_italic: Typeface,
    pub has_newsreader_italic: bool,
    pub jetbrains_mono: Typeface,
    pub has_jetbrains_mono: bool,
    /// Cache of Fraunces typefaces by (soft, wonk, opsz, weight). Each
    /// `clone_with_arguments` produces a fresh `Typeface` that Skia's
    /// internal caches don't fully release, so without this we leak ≈4 MB/s
    /// at 500 fps with the launcher's animated breathing-text + per-screen
    /// axis-tweaked headings. The launcher uses ~20 distinct combinations
    /// across the whole UI; the cache fits them all + stays at a stable
    /// size forever.
    fraunces_cache: RefCell<HashMap<FrauncesAxisKey, Typeface>>,
}

/// Quantised cache key for a Fraunces variant — values map to fixed-point
/// integers so `f32`s with rounding noise don't multiply identical entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FrauncesAxisKey {
    soft: i32,
    wonk: i32,
    opsz: i32,
    weight: i32,
}

impl FrauncesAxisKey {
    fn from_axes(soft: f32, wonk: f32, opsz: f32, weight: f32) -> Self {
        // ×100 fixed-point — sub-axis precision the eye couldn't see anyway.
        Self {
            soft: (soft * 100.0).round() as i32,
            wonk: (wonk * 100.0).round() as i32,
            opsz: (opsz * 100.0).round() as i32,
            weight: (weight * 100.0).round() as i32,
        }
    }
}

impl FontStore {
    pub fn new() -> Self {
        let (fraunces, has_fraunces) = try_load_family("Fraunces", false);
        let (newsreader, has_newsreader) = try_load_family("Newsreader", false);
        let (newsreader_italic, has_newsreader_italic) =
            try_load_family("Newsreader-Italic", true);
        let (jetbrains_mono, has_jetbrains_mono) = try_load_family("JetBrainsMono", false);
        Self {
            fraunces,
            has_fraunces,
            newsreader,
            has_newsreader,
            newsreader_italic,
            has_newsreader_italic,
            jetbrains_mono,
            has_jetbrains_mono,
            fraunces_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Build a `Font` for Fraunces at the given pixel size, with default axes.
    #[allow(dead_code)]
    pub fn fraunces(&self, size: f32) -> Font {
        let mut f = Font::new(self.fraunces.clone(), size);
        f.set_subpixel(true);
        f
    }

    /// Build a `Font` for Fraunces with explicit variable-axis values. The
    /// axes Fraunces exposes are `SOFT` (0..100, terminal sharpness),
    /// `WONK` (0/1, swash glyphs), `opsz` (9..144, optical size), and `wght`
    /// (100..900). If the loaded typeface doesn't support these axes (e.g. the
    /// fallback system font), this silently falls back to the default cut.
    pub fn fraunces_axes(
        &self,
        size: f32,
        soft: f32,
        wonk: f32,
        weight: f32,
        opsz: Option<f32>,
    ) -> Font {
        let opsz = opsz.unwrap_or(size); // CSS `font-optical-sizing: auto` ≈ opsz tracks size
        let key = FrauncesAxisKey::from_axes(soft, wonk, opsz, weight);

        // Fast path — variant already built for this axis combination.
        if let Some(tf) = self.fraunces_cache.borrow().get(&key) {
            let mut f = Font::new(tf.clone(), size);
            f.set_subpixel(true);
            return f;
        }

        // Slow path — build the variant once and cache it. `clone_with_arguments`
        // is what was leaking when called per-frame; doing it once per
        // (soft, wonk, opsz, weight) combination is fine. Skia handles the
        // refcounted clone in `Font::new(tf.clone(), size)` cheaply.
        let coords = [
            Coordinate { axis: FourByteTag::from_chars('S', 'O', 'F', 'T'), value: soft },
            Coordinate { axis: FourByteTag::from_chars('W', 'O', 'N', 'K'), value: wonk },
            Coordinate { axis: FourByteTag::from_chars('o', 'p', 's', 'z'), value: opsz },
            Coordinate { axis: FourByteTag::from_chars('w', 'g', 'h', 't'), value: weight },
        ];
        let pos = VariationPosition { coordinates: &coords };
        let args = FontArguments::new().set_variation_design_position(pos);
        let tf = self
            .fraunces
            .clone_with_arguments(&args)
            .unwrap_or_else(|| self.fraunces.clone());
        let mut f = Font::new(tf.clone(), size);
        f.set_subpixel(true);
        self.fraunces_cache.borrow_mut().insert(key, tf);
        f
    }

    #[allow(dead_code)]
    pub fn newsreader(&self, size: f32) -> Font {
        let mut f = Font::new(self.newsreader.clone(), size);
        f.set_subpixel(true);
        f
    }

    pub fn jetbrains_mono(&self, size: f32) -> Font {
        let mut f = Font::new(self.jetbrains_mono.clone(), size);
        f.set_subpixel(true);
        f
    }

    /// Access the italic Newsreader typeface. Loaded separately because the
    /// upright cut (`newsreader`) skips italics in `try_load_family`.
    pub fn newsreader_italic_typeface(&self) -> &Typeface {
        &self.newsreader_italic
    }
}

impl Default for FontStore {
    fn default() -> Self {
        Self::new()
    }
}

// ────────────────────────────────────────────────────────────────────────
// Per-glyph text rendering — letter-spacing + breathing animation
// ────────────────────────────────────────────────────────────────────────

/// Draw a string with explicit inter-glyph letter-spacing (CSS
/// `letter-spacing` approximation). Spacing is added between each glyph as
/// `letter_spacing_em * font.size()`.
///
/// `origin` is `(left, baseline)` — Skia's draw_str takes a baseline origin.
pub fn draw_tracked_em(
    canvas: &Canvas,
    text: &str,
    origin: (f32, f32),
    font: &Font,
    paint: &Paint,
    letter_spacing_em: f32,
) {
    let spacing = letter_spacing_em * font.size();
    let (mut cur_x, baseline_y) = origin;
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        let s: &str = ch.encode_utf8(&mut buf);
        canvas.draw_str(s, (cur_x, baseline_y), font, paint);
        let (advance, _) = font.measure_str(s, Some(paint));
        cur_x += advance + spacing;
    }
}

/// Measure the rendered width of a string with the given letter-spacing.
pub fn measure_tracked_em(font: &Font, text: &str, letter_spacing_em: f32) -> f32 {
    let spacing = letter_spacing_em * font.size();
    let mut total = 0.0;
    let mut buf = [0u8; 4];
    let mut count = 0;
    for ch in text.chars() {
        let s: &str = ch.encode_utf8(&mut buf);
        let (advance, _) = font.measure_str(s, None);
        total += advance;
        count += 1;
    }
    if count > 1 {
        total += spacing * (count as f32 - 1.0);
    }
    total
}

/// Draw a string with breathing letter-spacing — the prototype's `bt-breath`
/// animation. Letter-spacing oscillates from `base_em` to `base_em + amp_em`
/// and back over `period_sec`, with a smoothstepped triangle curve (matches
/// CSS `ease-in-out` close enough for this purpose).
///
/// CSS reference (`.bt-g`):
/// ```css
/// @keyframes bt-breath {
///   0%, 100% { letter-spacing: 0em; }
///   50%      { letter-spacing: 0.02em; }
/// }
/// .bt-g { animation: bt-breath calc(8s / var(--motion-speed)) ease-in-out infinite; }
/// ```
///
/// The base letter-spacing comes from the parent (`.launcher-title-xl`'s
/// `-0.035em`); the amplitude is the bt-g animation's range.
pub fn draw_breathing(
    canvas: &Canvas,
    text: &str,
    origin: (f32, f32),
    font: &Font,
    paint: &Paint,
    base_em: f32,
    amp_em: f32,
    time_sec: f32,
    period_sec: f32,
) {
    let breath = breath_envelope(time_sec, period_sec);
    let total_em = base_em + amp_em * breath;
    draw_tracked_em(canvas, text, origin, font, paint, total_em);
}

/// Triangle wave 0 → 1 → 0 over `period_sec`, smoothstepped (≈ ease-in-out).
pub fn breath_envelope(time_sec: f32, period_sec: f32) -> f32 {
    let phase = (time_sec / period_sec).rem_euclid(1.0);
    let triangle = if phase < 0.5 { phase * 2.0 } else { (1.0 - phase) * 2.0 };
    triangle * triangle * (3.0 - 2.0 * triangle)
}

// ────────────────────────────────────────────────────────────────────────
// Hover-glow stagger — per-glyph delayed transitions
// ────────────────────────────────────────────────────────────────────────

/// CSS `:nth-child(Xn)` last-rule-wins delay table for the prototype's
/// `.bt-hover-glow .bt-g` glyphs. Returns the per-glyph transition delay
/// in seconds. Glyph index `i` is **1-based** (matches CSS).
///
/// Rules (defined in source order, so later overrides earlier):
///   2n → 30ms · 3n → 60ms · 4n → 90ms · 5n → 120ms
/// For glyph i, the largest of {2,3,4,5} that divides i wins.
fn nth_delay_seconds(i: usize) -> f32 {
    let i_i = i as i32;
    if i_i % 5 == 0 {
        0.120
    } else if i_i % 4 == 0 {
        0.090
    } else if i_i % 3 == 0 {
        0.060
    } else if i_i % 2 == 0 {
        0.030
    } else {
        0.0
    }
}

/// Persistent state for hover-glow text. Tracks the wall-clock moment the
/// hover state last flipped so per-glyph transitions can compute their own
/// progress with the right `:nth-child` delay.
#[derive(Copy, Clone, Debug, Default)]
pub struct HoverGlowState {
    pub hovering: bool,
    /// Wall-clock seconds at the most recent `hovering` flip. Used to
    /// derive `time_since_change = time - last_change_at` per frame.
    pub last_change_at: f32,
}

impl HoverGlowState {
    /// Update hover state given the current cursor and bounds. Records the
    /// current `time` if the state flipped this call.
    pub fn update(&mut self, hovering_now: bool, time: f32) {
        if hovering_now != self.hovering {
            self.hovering = hovering_now;
            self.last_change_at = time;
        }
    }
}

/// Draw breathing text with a per-glyph staggered hover-glow.
///
/// The base render matches `draw_breathing` (per-glyph letter-spacing
/// oscillation). On top, each glyph independently transitions its color +
/// glow halo toward the hovered state over 220ms silk, with the CSS
/// `:nth-child` delay table (0/30/60/90/120ms).
///
/// Glow at rest = 0 (just the base paint color, no halo).
/// Glow at peak = pearl color `#FFF0F4` + two halos:
///   - `0 0 8px rgba(229, 184, 197, 0.65)` (rose, near)
///   - `0 0 18px rgba(180, 116, 145, 0.35)` (berry, far)
#[allow(clippy::too_many_arguments)]
pub fn draw_breathing_glow(
    canvas: &Canvas,
    text: &str,
    origin: (f32, f32),
    font: &Font,
    paint: &Paint,
    base_em: f32,
    amp_em: f32,
    time_sec: f32,
    period_sec: f32,
    hover: HoverGlowState,
) {
    let breath = breath_envelope(time_sec, period_sec);
    let total_em = base_em + amp_em * breath;
    let spacing = total_em * font.size();
    let (mut cur_x, baseline_y) = origin;
    let mut buf = [0u8; 4];

    let time_since_change = (time_sec - hover.last_change_at).max(0.0);

    // Base paint color → at-rest pearl. Hover end color → warm-white #FFF0F4.
    let base_color = paint.color4f();
    let hover_color = skia_safe::Color4f::new(
        0xFF as f32 / 255.0,
        0xF0 as f32 / 255.0,
        0xF4 as f32 / 255.0,
        base_color.a,
    );

    for (idx, ch) in text.chars().enumerate() {
        let s: &str = ch.encode_utf8(&mut buf);

        // Per-glyph delayed transition toward `hover.hovering`.
        let delay = nth_delay_seconds(idx + 1);
        let raw = ((time_since_change - delay) / 0.22).clamp(0.0, 1.0);
        let eased = ewo_core::CubicBezier::SILK.eval(raw);
        let toward = if hover.hovering { eased } else { 1.0 - eased };

        // Halo passes only fire when there's some glow to render.
        if toward > 0.02 {
            // Far halo: 18px blur, berry rgba(180,116,145,0.35).
            let mut far = Paint::default();
            far.set_anti_alias(true);
            far.set_color4f(
                skia_safe::Color4f::new(
                    180.0 / 255.0,
                    116.0 / 255.0,
                    145.0 / 255.0,
                    0.35 * toward,
                ),
                None,
            );
            far.set_mask_filter(skia_safe::MaskFilter::blur(
                skia_safe::BlurStyle::Normal,
                9.0,
                false,
            ));
            canvas.draw_str(s, (cur_x, baseline_y), font, &far);

            // Near halo: 8px blur, rose rgba(229,184,197,0.65).
            let mut near = Paint::default();
            near.set_anti_alias(true);
            near.set_color4f(
                skia_safe::Color4f::new(
                    229.0 / 255.0,
                    184.0 / 255.0,
                    197.0 / 255.0,
                    0.65 * toward,
                ),
                None,
            );
            near.set_mask_filter(skia_safe::MaskFilter::blur(
                skia_safe::BlurStyle::Normal,
                4.0,
                false,
            ));
            canvas.draw_str(s, (cur_x, baseline_y), font, &near);
        }

        // Glyph body — color lerped from base → warm-white by `toward`.
        let mut body = paint.clone();
        let r = base_color.r + (hover_color.r - base_color.r) * toward;
        let g = base_color.g + (hover_color.g - base_color.g) * toward;
        let b = base_color.b + (hover_color.b - base_color.b) * toward;
        body.set_color4f(skia_safe::Color4f::new(r, g, b, base_color.a), None);
        canvas.draw_str(s, (cur_x, baseline_y), font, &body);

        let (advance, _) = font.measure_str(s, Some(paint));
        cur_x += advance + spacing;
    }
}

// ────────────────────────────────────────────────────────────────────────
// Internal — font loading
// ────────────────────────────────────────────────────────────────────────

fn workspace_assets_dir() -> PathBuf {
    // env!("CARGO_MANIFEST_DIR") points to crates/ewo-render at compile time.
    // The assets dir is two levels up from there.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut p = PathBuf::from(manifest);
    p.pop(); // crates/
    p.pop(); // workspace root
    p.push("assets");
    p.push("fonts");
    p
}

fn try_load_family(family_prefix: &str, want_italic: bool) -> (Typeface, bool) {
    let mgr = FontMgr::new();
    let fonts_dir = workspace_assets_dir();

    if let Ok(entries) = std::fs::read_dir(&fonts_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            // Match `<family>*.ttf` and `<family>*.otf` (case-insensitive prefix).
            let lower = name.to_ascii_lowercase();
            let prefix_lower = family_prefix.to_ascii_lowercase();
            if !lower.starts_with(&prefix_lower) {
                continue;
            }
            if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) {
                continue;
            }
            // Filter on italic vs upright. The italic cut's filename must
            // contain "italic"; for the upright cut we skip anything italic.
            let has_italic = lower.contains("italic");
            if want_italic != has_italic {
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => match mgr.new_from_data(&bytes, None) {
                    Some(tf) => {
                        log::info!("loaded font: {}", name);
                        return (tf, true);
                    }
                    None => log::warn!("could not parse font {}", name),
                },
                Err(e) => log::warn!("could not read {}: {}", path.display(), e),
            }
        }
    }

    log::warn!(
        "font family '{family_prefix}' not found in {}. Place a variable TTF \
         (e.g. {family_prefix}[SOFT,WONK,opsz,wght].ttf from Google Fonts) \
         there for proper visual fidelity.",
        fonts_dir.display()
    );
    let fallback = mgr
        .legacy_make_typeface(None, FontStyle::default())
        .expect("system default typeface");
    (fallback, false)
}
