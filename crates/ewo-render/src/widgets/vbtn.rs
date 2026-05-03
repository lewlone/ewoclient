//! `vbtn` — the prototype's velvet button.
//!
//! Renders four animated layers under the label:
//!   1. **Tint** — 135° linear gradient (rose → berry → lavender), low alpha
//!   2. **Rim** — 1px stroked iridescent gradient, opacity pulse 7s (or 2s on hover)
//!   3. **Sheen** — skewed sweep gradient, 7s linear (or 2.5s on hover)
//!   4. **Label** — Newsreader 16/17 with 0.02em tracking, pearl color
//!
//! Plus the click + cursor particle layers (above the label):
//!   5. **Ripples** — pearl radial halos that expand from the click position,
//!      6px → 320px diameter, 0.9 → 0 opacity, 620ms silk easing
//!   6. **Specks** — small pearl dots emitted while the cursor moves over the
//!      button, drift up + fade out over 700ms silk easing
//!
//! Plus state-driven transforms:
//!   - hover  → translateY(-0.5)
//!   - press  → scale(0.96)
//!
//! State is owned by the caller (`VbtnState`); the `update` helper consumes a
//! mouse position + button-down flag and returns `true` once on the click
//! release frame.

use crate::text::{self, FontStore};
use skia_safe::{
    gradient_shader, BlendMode, BlurStyle, Canvas, Color, Color4f, Contains, MaskFilter, Matrix,
    Paint, PaintStyle, Point, RRect, Rect, TileMode,
};

const RIPPLE_LIFE_S: f32 = 0.62;
const RIPPLE_MAX_DIAM: f32 = 320.0;
const SPECK_LIFE_S: f32 = 0.70;
const SPECK_INTERVAL_S: f32 = 0.04;
const MAX_RIPPLES: usize = 4;
const MAX_SPECKS: usize = 8;

/// One press ripple — `pos` is in button-local pixels (origin at `bounds.left,top`).
/// `age` is seconds since spawn; expired when `age >= RIPPLE_LIFE_S`.
#[derive(Default, Debug, Copy, Clone)]
pub struct Ripple {
    pub x: f32,
    pub y: f32,
    pub age: f32,
    pub active: bool,
}

/// One cursor speck — `pos` in button-local pixels. `hue` is the oklch H
/// value the prototype's `--speck-hue` rolls (340 + rand 30 → [340, 370)°).
/// We approximate with a rose ↔ champagne lerp on a `hue_t` 0..1.
#[derive(Default, Debug, Copy, Clone)]
pub struct Speck {
    pub x: f32,
    pub y: f32,
    pub age: f32,
    pub hue_t: f32,
    pub active: bool,
}

#[derive(Default, Debug, Copy, Clone)]
pub struct VbtnState {
    pub hover: bool,
    pub pressing: bool,
    /// Wall-clock time (seconds) when the press started — feeds the
    /// scale-back transition. `None` while not pressed.
    pub press_t: Option<f32>,
    /// 0..1, smoothly tracks `hover` over CSS `transition-duration: 400ms`.
    /// Tick via `VbtnState::tick(dt)` once per frame for smooth in/out.
    pub hover_anim: f32,
    /// Live press-ripples. Slot reuse — `active=false` means free.
    pub ripples: [Ripple; MAX_RIPPLES],
    /// Live cursor specks (max 8 trailing per the prototype's `slice(-8)`).
    pub specks: [Speck; MAX_SPECKS],
    /// Last cursor position seen by `update` — used to detect movement so
    /// specks only spawn when the cursor actually moves (matches the
    /// `onPointerMove` semantics, not mere hover).
    pub last_cursor: (f32, f32),
    /// Wall-clock seconds of the last speck spawn. Throttles spawn rate
    /// to 1-per-`SPECK_INTERVAL_S` (40ms) per the prototype's `lastSpeckRef`.
    pub last_speck_t: f32,
    /// PRNG seed for hue jitter. Cheap LCG, no external deps. Bumped every
    /// time we spawn a speck so successive specks get different hues.
    pub rng_state: u32,
}

impl VbtnState {
    /// Update state from current input. `mouse` is in the same coordinate
    /// space as `bounds`. Returns `true` exactly on the frame the button is
    /// "clicked" (mouse released while still in bounds, with prior press).
    ///
    /// Also: spawns a ripple on the press-edge inside bounds, and emits
    /// cursor specks while the cursor is moving inside bounds (throttled
    /// to one per ~40ms, matching the prototype's `lastSpeckRef`).
    pub fn update(
        &mut self,
        mouse: (f32, f32),
        bounds: Rect,
        mouse_down: bool,
        time: f32,
    ) -> bool {
        let in_bounds = bounds.contains(Point::new(mouse.0, mouse.1));
        let was_pressing = self.pressing;
        self.hover = in_bounds;
        self.pressing = in_bounds && mouse_down;
        if !was_pressing && self.pressing {
            self.press_t = Some(time);
            // Spawn a ripple at the press location (button-local coords).
            self.spawn_ripple(mouse.0 - bounds.left, mouse.1 - bounds.top);
        }
        if !mouse_down {
            self.press_t = None;
        }

        // Cursor speck trail — only spawn while in bounds, when the cursor
        // actually moved since last frame, throttled to SPECK_INTERVAL_S.
        let moved = (mouse.0 - self.last_cursor.0).abs() > 0.5
            || (mouse.1 - self.last_cursor.1).abs() > 0.5;
        if in_bounds && moved && time - self.last_speck_t >= SPECK_INTERVAL_S {
            self.spawn_speck(mouse.0 - bounds.left, mouse.1 - bounds.top);
            self.last_speck_t = time;
        }
        self.last_cursor = mouse;

        // Click event: was pressing, button released, still in bounds
        was_pressing && !mouse_down && in_bounds
    }

    /// Per-frame animation tick. Drives `hover_anim` toward the target
    /// (0 or 1) at the CSS-equivalent 400ms rate, and ages all live
    /// ripples + specks. Expired particles deactivate so the slot is
    /// reusable on the next spawn.
    pub fn tick(&mut self, dt: f32) {
        let target = if self.hover { 1.0 } else { 0.0 };
        let max_step = dt / 0.4;
        let delta: f32 = target - self.hover_anim;
        self.hover_anim += delta.clamp(-max_step, max_step);

        for r in self.ripples.iter_mut() {
            if r.active {
                r.age += dt;
                if r.age >= RIPPLE_LIFE_S {
                    r.active = false;
                }
            }
        }
        for s in self.specks.iter_mut() {
            if s.active {
                s.age += dt;
                if s.age >= SPECK_LIFE_S {
                    s.active = false;
                }
            }
        }
    }

    fn spawn_ripple(&mut self, x: f32, y: f32) {
        // Find an inactive slot, or overwrite the oldest active one.
        let mut victim = 0usize;
        let mut victim_age = -1.0_f32;
        for (i, r) in self.ripples.iter().enumerate() {
            if !r.active {
                self.ripples[i] = Ripple {
                    x,
                    y,
                    age: 0.0,
                    active: true,
                };
                return;
            }
            if r.age > victim_age {
                victim_age = r.age;
                victim = i;
            }
        }
        self.ripples[victim] = Ripple {
            x,
            y,
            age: 0.0,
            active: true,
        };
    }

    fn spawn_speck(&mut self, x: f32, y: f32) {
        // Bump LCG, derive a hue_t in [0, 1) for the rose↔champagne lerp.
        self.rng_state = self.rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
        let hue_t = ((self.rng_state >> 8) & 0xFFFF) as f32 / 65535.0;
        // Find an inactive slot, or overwrite the oldest active one.
        let mut victim = 0usize;
        let mut victim_age = -1.0_f32;
        for (i, s) in self.specks.iter().enumerate() {
            if !s.active {
                self.specks[i] = Speck {
                    x,
                    y,
                    age: 0.0,
                    hue_t,
                    active: true,
                };
                return;
            }
            if s.age > victim_age {
                victim_age = s.age;
                victim = i;
            }
        }
        self.specks[victim] = Speck {
            x,
            y,
            age: 0.0,
            hue_t,
            active: true,
        };
    }
}

/// Draw a vbtn at the given bounds.
pub fn draw_vbtn(
    canvas: &Canvas,
    bounds: Rect,
    label: &str,
    state: &VbtnState,
    time: f32,
    motion_speed: f32,
    fonts: &FontStore,
    primary: bool,
) {
    // Hover lift + press scale, applied around the button's center.
    let lift_y = if state.hover { -0.5 } else { 0.0 };
    let scale = if state.pressing { 0.96 } else { 1.0 };
    let cx = (bounds.left + bounds.right) * 0.5;
    let cy = (bounds.top + bounds.bottom) * 0.5;

    let saved = canvas.save();
    canvas.translate((cx, cy + lift_y));
    canvas.scale((scale, scale));
    canvas.translate((-cx, -cy));

    let radius = 14.0;
    let rrect = RRect::new_rect_xy(bounds, radius, radius);

    // Layer 1: tint
    draw_tint(canvas, bounds, radius, primary);

    // Layer 2: animated rim (1px iridescent stroke)
    draw_rim(canvas, bounds, radius, time, motion_speed, state.hover);

    // Layer 3: sheen sweep — clipped to the button so the skewed bar can't
    // bleed past the corners.
    {
        let saved2 = canvas.save();
        canvas.clip_rrect(rrect, None, Some(true));
        draw_sheen(canvas, bounds, time, motion_speed, state.hover);
        canvas.restore_to_count(saved2);
    }

    // Layer 4: label
    draw_label(canvas, bounds, label, primary, fonts);

    // Layer 5+6: ripples + cursor specks. Clipped to the button rrect so
    // the expanding rings/dots can't bleed past the rounded corners.
    {
        let saved2 = canvas.save();
        canvas.clip_rrect(rrect, None, Some(true));
        draw_ripples(canvas, bounds, &state.ripples);
        draw_specks(canvas, bounds, &state.specks);
        canvas.restore_to_count(saved2);
    }

    canvas.restore_to_count(saved);
}

// ────────────────────────────────────────────────────────────────────────
// Ripples + specks
// ────────────────────────────────────────────────────────────────────────

fn draw_ripples(canvas: &Canvas, bounds: Rect, ripples: &[Ripple]) {
    for r in ripples.iter() {
        if !r.active {
            continue;
        }
        let t = (r.age / RIPPLE_LIFE_S).clamp(0.0, 1.0);
        let eased = ewo_core::CubicBezier::SILK.eval(t);
        let diam = 6.0 + (RIPPLE_MAX_DIAM - 6.0) * eased;
        let opacity = 0.9 * (1.0 - eased);
        let cx = bounds.left + r.x;
        let cy = bounds.top + r.y;

        if let Some(shader) = gradient_shader::radial(
            Point::new(cx, cy),
            (diam * 0.5).max(0.5),
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new(255.0 / 255.0, 225.0 / 255.0, 230.0 / 255.0, 0.8 * opacity),
                    Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.4 * opacity),
                    Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.0),
                ],
                None,
            ),
            Some(&[0.0_f32, 0.40, 0.70][..]),
            TileMode::Clamp,
            None,
            None,
        ) {
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_shader(shader);
            p.set_blend_mode(BlendMode::Screen);
            canvas.draw_circle((cx, cy), diam * 0.5, &p);
        }
    }
}

fn draw_specks(canvas: &Canvas, bounds: Rect, specks: &[Speck]) {
    for s in specks.iter() {
        if !s.active {
            continue;
        }
        let t = (s.age / SPECK_LIFE_S).clamp(0.0, 1.0);
        let eased = ewo_core::CubicBezier::SILK.eval(t);
        // CSS @keyframes vbtn-speck:
        //   0%   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
        //   100% { opacity: 0; transform: translate(-50%, -90%) scale(0.4); }
        // i.e. drift up by 40% of size and shrink to 0.4× while fading.
        let opacity = 1.0 - eased;
        let scale = 1.0 - 0.6 * eased;
        let drift_y = -0.40 * 4.0 * eased; // 4px speck size, drift -40% of size
        let cx = bounds.left + s.x;
        let cy = bounds.top + s.y + drift_y;
        // Hue lerp rose (0xE5B8C5) ↔ champagne (0xE8D4A8). hue_t∈[0,1).
        let r = lerp(229.0, 232.0, s.hue_t) / 255.0;
        let g = lerp(184.0, 212.0, s.hue_t) / 255.0;
        let b = lerp(197.0, 168.0, s.hue_t) / 255.0;
        let radius = 2.0 * scale;
        if radius <= 0.0 {
            continue;
        }
        if let Some(shader) = gradient_shader::radial(
            Point::new(cx, cy),
            radius,
            gradient_shader::GradientShaderColors::ColorsInSpace(
                &[
                    Color4f::new(0.95, 0.91, 0.93, 0.9 * opacity),
                    Color4f::new(r, g, b, 0.4 * opacity),
                    Color4f::new(r, g, b, 0.0),
                ],
                None,
            ),
            Some(&[0.0_f32, 0.50, 0.80][..]),
            TileMode::Clamp,
            None,
            None,
        ) {
            let mut p = Paint::default();
            p.set_anti_alias(true);
            p.set_shader(shader);
            p.set_blend_mode(BlendMode::Screen);
            p.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 0.5, false));
            canvas.draw_circle((cx, cy), radius, &p);
        }
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ────────────────────────────────────────────────────────────────────────
// Tint — `.vbtn-tint` 135° gradient
// ────────────────────────────────────────────────────────────────────────

/// Used by `vdrop` so the dropdown head can share vbtn's chrome.
pub(crate) fn draw_tint_for_dropdown(canvas: &Canvas, rect: Rect, radius: f32) {
    draw_tint(canvas, rect, radius, false);
}

/// Used by `vdrop` so the dropdown head can share vbtn's chrome.
pub(crate) fn draw_rim_for_dropdown(
    canvas: &Canvas,
    rect: Rect,
    radius: f32,
    time: f32,
    motion_speed: f32,
    hover: bool,
) {
    draw_rim(canvas, rect, radius, time, motion_speed, hover);
}

/// Used by `vdrop` so the dropdown head can share vbtn's chrome.
pub(crate) fn draw_sheen_for_dropdown(
    canvas: &Canvas,
    rect: Rect,
    time: f32,
    motion_speed: f32,
    hover: bool,
) {
    draw_sheen(canvas, rect, time, motion_speed, hover);
}

fn draw_tint(canvas: &Canvas, rect: Rect, radius: f32, primary: bool) {
    // CSS — non-primary alphas 0.10 / 0.06 / 0.08, primary 0.18 / 0.14 / 0.16.
    let (a1, a2, a3) = if primary {
        (0.18, 0.14, 0.16)
    } else {
        (0.10, 0.06, 0.08)
    };
    let rrect = RRect::new_rect_xy(rect, radius, radius);
    let shader = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, a1),
                Color4f::new(180.0 / 255.0, 116.0 / 255.0, 145.0 / 255.0, a2),
                Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, a3),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.4, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    )
    .expect("vbtn tint shader");
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_shader(shader);
    canvas.draw_rrect(rrect, &p);
}

// ────────────────────────────────────────────────────────────────────────
// Rim — `.vbtn-rim` with `vbtn-rim-pulse`
// ────────────────────────────────────────────────────────────────────────

fn draw_rim(canvas: &Canvas, rect: Rect, radius: f32, time: f32, motion_speed: f32, hover: bool) {
    // CSS rim-pulse: 7s ease-in-out, opacity 0.7 ↔ 1.0. Hover shortens to 2s
    // and brightens the gradient stops.
    let period = if hover { 2.0 } else { 7.0 } / motion_speed.max(0.0001);
    let phase = (time / period).rem_euclid(2.0);
    let triangle = if phase < 1.0 { phase } else { 2.0 - phase };
    let pulse = triangle * triangle * (3.0 - 2.0 * triangle);
    let opacity = 0.7 + 0.3 * pulse;

    let (a1, a2, a3) = if hover {
        (0.80, 0.45, 0.70)
    } else {
        (0.55, 0.25, 0.45)
    };

    let rrect = RRect::new_rect_xy(rect, radius, radius);
    let shader = gradient_shader::linear(
        (
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.bottom),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, a1),
                Color4f::new(201.0 / 255.0, 165.0 / 255.0, 212.0 / 255.0, a2),
                Color4f::new(232.0 / 255.0, 212.0 / 255.0, 168.0 / 255.0, a3),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.5, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    )
    .expect("vbtn rim shader");
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_style(PaintStyle::Stroke);
    p.set_stroke_width(1.0);
    p.set_shader(shader);
    p.set_alpha_f(opacity);
    canvas.draw_rrect(rrect, &p);
}

// ────────────────────────────────────────────────────────────────────────
// Sheen — `.vbtn-sheen` with `vbtn-sheen-sweep`
// ────────────────────────────────────────────────────────────────────────

fn draw_sheen(canvas: &Canvas, rect: Rect, time: f32, motion_speed: f32, hover: bool) {
    // CSS: a 40%-wide skewed bar slides from left:-40% → 110% over 7s linear.
    // Hover speeds to 2.5s. transform: skewX(-18deg). filter: blur(2px).
    let period = if hover { 2.5 } else { 7.0 } / motion_speed.max(0.0001);
    let progress = (time / period).rem_euclid(1.0);

    let btn_w = rect.width();
    let btn_h = rect.height();
    let sheen_w = btn_w * 0.4;
    // Left edge moves from -40% to 110% of button width.
    let sheen_left = rect.left + (-0.4 + progress * 1.5) * btn_w;
    // Vertical extension prevents the skew exposing edges at top/bottom.
    let sheen_rect = Rect::from_xywh(sheen_left, rect.top - 4.0, sheen_w, btn_h + 8.0);

    let shader = gradient_shader::linear(
        (
            Point::new(sheen_rect.left, 0.0),
            Point::new(sheen_rect.right, 0.0),
        ),
        gradient_shader::GradientShaderColors::ColorsInSpace(
            &[
                Color4f::new(0.0, 0.0, 0.0, 0.0),
                Color4f::new(255.0 / 255.0, 240.0 / 255.0, 244.0 / 255.0, 0.18),
                Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.35),
                Color4f::new(255.0 / 255.0, 240.0 / 255.0, 244.0 / 255.0, 0.18),
                Color4f::new(0.0, 0.0, 0.0, 0.0),
            ],
            None,
        ),
        Some(&[0.0_f32, 0.30, 0.50, 0.70, 1.0][..]),
        TileMode::Clamp,
        None,
        None,
    )
    .expect("vbtn sheen shader");

    // skewX(-18deg) ≈ tan(-18°) = -0.3249. Apply around the button center so
    // the sheen tilts symmetrically.
    let cx = (rect.left + rect.right) * 0.5;
    let cy = (rect.top + rect.bottom) * 0.5;
    let mut m = Matrix::default();
    m.pre_translate((cx, cy));
    m.pre_skew((-0.3249, 0.0), None);
    m.pre_translate((-cx, -cy));

    let saved = canvas.save();
    canvas.concat(&m);

    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_shader(shader);
    p.set_blend_mode(BlendMode::Screen);
    p.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 2.0, false));
    canvas.draw_rect(sheen_rect, &p);

    canvas.restore_to_count(saved);
}

// ────────────────────────────────────────────────────────────────────────
// Label
// ────────────────────────────────────────────────────────────────────────

fn draw_label(canvas: &Canvas, rect: Rect, label: &str, primary: bool, fonts: &FontStore) {
    let size = if primary { 17.0 } else { 16.0 };
    let font = fonts.newsreader(size);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA)); // text-pearl

    // Center the label horizontally with 0.02em tracking applied.
    let tracking_em = 0.02;
    let total_w = text::measure_tracked_em(&font, label, tracking_em);
    let cx = (rect.left + rect.right) * 0.5;
    let (_, m) = font.metrics();
    // Approximate visual centering: place baseline so cap-mid lines up with
    // the rect's vertical center.
    let cy = (rect.top + rect.bottom) * 0.5;
    let baseline = cy + m.cap_height * 0.5;

    text::draw_tracked_em(
        canvas,
        label,
        (cx - total_w * 0.5, baseline),
        &font,
        &paint,
        tracking_em,
    );
}
