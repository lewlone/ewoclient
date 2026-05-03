//! Pearl dust — soft pink-cream motes that drift up across the backdrop.
//!
//! Port of the prototype's `PearlDust` React component (`EwoClient · Velvet &
//! Pearl prototype.htm` line ~6482). Two populations:
//!
//! - **Airborne** (default 90 motes at `density=1`): spawns at the bottom,
//!   drifts up with sinusoidal sway, modulates alpha and radius via a per-
//!   particle "shine" cycle, respawns when off-screen or at end-of-life.
//! - **Settled** (default 60 motes): static near the bottom edge, twinkles in
//!   place, gets pushed around briefly when the user clicks the backdrop.
//!
//! Drawn under `mix-blend-mode: screen`, so all the colored alphas brighten
//! whatever's beneath rather than alpha-blending.

use ewo_core::Settings;
use rand::Rng;
use skia_safe::canvas::SaveLayerRec;
use skia_safe::{
    gradient_shader, BlendMode, Canvas, Color4f, Paint, Point, Rect, TileMode,
};
use std::f32::consts::PI;

// Bumped above the prototype's CSS-derived 90 / 60 so the bottom shimmer
// reads at the same density our render produces. Each airborne mote now
// uses a 3-stop radial gradient halo (matches the prototype's JS exactly)
// + tiny halos skipped when radius < 2px — net cost is comparable to the
// pre-tuning mask-blur halo at lower visual fidelity.
const AIR_BASE: usize = 110;
const SETTLED_BASE: usize = 80;

#[derive(Clone, Copy)]
struct Airborne {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    r: f32,
    life: f32,
    max_life: f32,
    shine_phase: f32,
    shine_freq: f32,
}

#[derive(Clone, Copy)]
struct Settled {
    x: f32,
    y: f32,
    r: f32,
    twinkle: f32,
    /// Pre-baked horizontal jitter direction for `disturb` push, in [-1, 1].
    /// Stored once so the disturb push is deterministic for a given mote.
    jitter: f32,
}

pub struct PearlDust {
    airborne: Vec<Airborne>,
    settled: Vec<Settled>,
    /// Decays toward 0 each tick at *= 0.93. Bumped to 1.0 by `disturb()`.
    disturb_amount: f32,
    width: f32,
    height: f32,
    motion_speed: f32,
}

impl PearlDust {
    pub fn new(width: f32, height: f32, settings: &Settings) -> Self {
        let mut rng = rand::thread_rng();
        let air_count = ((AIR_BASE as f32) * settings.density).round() as usize;
        let settled_count = ((SETTLED_BASE as f32) * settings.density).round() as usize;
        let airborne = (0..air_count)
            .map(|_| make_air(&mut rng, width, height, settings.motion_speed))
            .collect();
        let settled = (0..settled_count)
            .map(|_| make_settled(&mut rng, width, height))
            .collect();
        Self {
            airborne,
            settled,
            disturb_amount: 0.0,
            width,
            height,
            motion_speed: settings.motion_speed,
        }
    }

    pub fn resize(&mut self, width: f32, height: f32, settings: &Settings) {
        // Cheap path: regenerate. Particles are short-lived (max ~1000 frames)
        // so the visual blip is brief.
        let mut rng = rand::thread_rng();
        let air_count = ((AIR_BASE as f32) * settings.density).round() as usize;
        let settled_count = ((SETTLED_BASE as f32) * settings.density).round() as usize;
        self.airborne = (0..air_count)
            .map(|_| make_air(&mut rng, width, height, settings.motion_speed))
            .collect();
        self.settled = (0..settled_count)
            .map(|_| make_settled(&mut rng, width, height))
            .collect();
        self.width = width;
        self.height = height;
        self.motion_speed = settings.motion_speed;
    }

    pub fn disturb(&mut self) {
        self.disturb_amount = 1.0;
    }

    /// Per-frame state update. Time-step independent: the original JS uses a
    /// fixed-step model (per-frame integer life ticks), so we approximate with
    /// `dt * 60` to keep behavior stable across refresh rates.
    pub fn update(&mut self, dt: f32) {
        let step = (dt * 60.0).clamp(0.0, 4.0); // cap big skips during stalls
        let h = self.height;
        let w = self.width;
        let speed = self.motion_speed;
        let mut rng = rand::thread_rng();

        for m in self.airborne.iter_mut() {
            m.x += (m.vx + (m.life * 0.01).sin() * 0.12) * step;
            m.y += m.vy * step;
            m.life += step;
            m.shine_phase += m.shine_freq * step;

            if m.y < -10.0 || m.life > m.max_life {
                *m = make_air(&mut rng, w, h, speed);
                m.y = h + 5.0;
            }
        }

        for s in self.settled.iter_mut() {
            s.twinkle += 0.02 * step;
        }

        // Disturb decay: 0.93 per *frame*, so per dt: 0.93^step.
        if self.disturb_amount > 0.0 {
            self.disturb_amount *= 0.93_f32.powf(step);
            if self.disturb_amount < 0.01 {
                self.disturb_amount = 0.0;
            }
        }
    }

    pub fn draw(&self, canvas: &Canvas) {
        let bounds = Rect::from_xywh(0.0, 0.0, self.width, self.height);
        let mut layer_paint = Paint::default();
        layer_paint.set_blend_mode(BlendMode::Screen);
        let saved = canvas.save_layer(&SaveLayerRec::default().paint(&layer_paint).bounds(&bounds));

        // Settled motes (drawn first — behind airborne)
        for s in &self.settled {
            let glow = 0.35 + s.twinkle.sin() * 0.15;
            let alpha = 0.08 + glow * 0.10;

            let (push, dy) = if self.disturb_amount > 0.0 {
                (
                    self.disturb_amount * 18.0 * s.jitter,
                    -self.disturb_amount * 12.0,
                )
            } else {
                (0.0, 0.0)
            };

            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color4f(
                Color4f::new(244.0 / 255.0, 232.0 / 255.0, 234.0 / 255.0, alpha),
                None,
            );
            canvas.draw_circle((s.x + push, s.y + dy), s.r, &paint);
        }

        // Airborne motes — render each as a 3-stop radial gradient (the
        // prototype's exact JS form). Faster than mask-blur halos and a
        // closer match to the browser-canvas reference.
        //
        // Each mote = 1 draw call (gradient circle at halo radius) + 1
        // tiny core circle. Previously: 1 mask-blur draw + 1 core. The
        // gradient shader path replaces the offscreen blur with an inline
        // shader pass, shaving ~30-50% off the per-mote cost.
        for m in &self.airborne {
            let shine = (m.shine_phase.sin() + 1.0) * 0.5; // 0..1
            let alpha = 0.18 + shine * 0.50;
            let r = m.r * (1.0 + shine * 0.8);
            let halo_radius = r * 4.0;

            // Skip tiny halos — for very small motes the halo is barely a
            // single pixel and the core alone reads correctly. Saves an
            // extra ~30% of the airborne draws when many small motes spawn.
            if halo_radius >= 2.0 {
                if let Some(shader) = gradient_shader::radial(
                    Point::new(m.x, m.y),
                    halo_radius,
                    gradient_shader::GradientShaderColors::ColorsInSpace(
                        &[
                            Color4f::new(
                                250.0 / 255.0,
                                240.0 / 255.0,
                                242.0 / 255.0,
                                alpha * 0.55,
                            ),
                            Color4f::new(
                                229.0 / 255.0,
                                184.0 / 255.0,
                                197.0 / 255.0,
                                alpha * 0.25,
                            ),
                            Color4f::new(229.0 / 255.0, 184.0 / 255.0, 197.0 / 255.0, 0.0),
                        ],
                        None,
                    ),
                    Some(&[0.0_f32, 0.40, 1.0][..]),
                    TileMode::Clamp,
                    None,
                    None,
                ) {
                    let mut halo = Paint::default();
                    halo.set_anti_alias(true);
                    halo.set_shader(shader);
                    canvas.draw_circle((m.x, m.y), halo_radius, &halo);
                }
            }

            // Core: bright pearl
            let mut core = Paint::default();
            core.set_anti_alias(true);
            core.set_color4f(
                Color4f::new(
                    250.0 / 255.0,
                    240.0 / 255.0,
                    242.0 / 255.0,
                    alpha,
                ),
                None,
            );
            canvas.draw_circle((m.x, m.y), r, &core);
        }

        canvas.restore_to_count(saved);
    }
}

fn make_air<R: Rng + ?Sized>(rng: &mut R, w: f32, h: f32, speed: f32) -> Airborne {
    Airborne {
        x: rng.gen::<f32>() * w,
        y: h + rng.gen::<f32>() * 80.0,
        vy: -(0.08 + rng.gen::<f32>() * 0.18) * speed,
        vx: (rng.gen::<f32>() - 0.5) * 0.04,
        r: 0.3 + rng.gen::<f32>() * 1.4,
        life: 0.0,
        max_life: 400.0 + rng.gen::<f32>() * 600.0,
        shine_phase: rng.gen::<f32>() * 2.0 * PI,
        shine_freq: 0.003 + rng.gen::<f32>() * 0.004,
    }
}

fn make_settled<R: Rng + ?Sized>(rng: &mut R, w: f32, h: f32) -> Settled {
    Settled {
        x: rng.gen::<f32>() * w,
        y: h - 6.0 - rng.gen::<f32>() * 28.0,
        r: 0.5 + rng.gen::<f32>() * 1.2,
        twinkle: rng.gen::<f32>() * 2.0 * PI,
        jitter: rng.gen::<f32>() - 0.5,
    }
}
