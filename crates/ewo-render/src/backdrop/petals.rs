//! Petals — sparse pink teardrop shapes drifting down across the backdrop.
//!
//! Port of the `Petals` React component (`EwoClient · Velvet & Pearl
//! prototype.htm` ~line 6628). Two modes:
//!
//! - **Idle**: 4 baseline petals at `density=1`. They fall slowly with a
//!   sinusoidal flutter, then respawn at the top when they exit the viewport.
//! - **Celebrate**: a wave from the upper-left every ~30ms, pushing the
//!   population up to 140 petals. Off-viewport petals are *deleted* (not
//!   recycled) so the population drains naturally back to the baseline once
//!   celebrate ends.
//!
//! Alpha-blended (no screen-blend) — petals are foreground motes, not light
//! contributors.

use ewo_core::OkLch;
use rand::Rng;
use skia_safe::{
    gradient_shader, BlendMode, Canvas, Color4f, Paint, Path, Point, TileMode,
};
use std::f32::consts::PI;

const BASE_PETALS: usize = 4;
const BURST_MAX: usize = 140;
const BURST_INTERVAL_SEC: f32 = 0.030;

#[derive(Clone, Copy)]
struct Petal {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    rot: f32,
    vrot: f32,
    size: f32,
    hue: f32,
    alpha: f32,
    flutter: f32,
}

pub struct Petals {
    petals: Vec<Petal>,
    celebrating: bool,
    cycle_time: f32,
    last_burst_t: f32,
    width: f32,
    height: f32,
    density: f32,
}

impl Petals {
    pub fn new(width: f32, height: f32, density: f32) -> Self {
        let mut rng = rand::thread_rng();
        let count = ((BASE_PETALS as f32) * density).round() as usize;
        let petals = (0..count)
            .map(|_| make_petal(&mut rng, width, height, false))
            .collect();
        Self {
            petals,
            celebrating: false,
            cycle_time: 0.0,
            last_burst_t: -1.0,
            width,
            height,
            density,
        }
    }

    pub fn resize(&mut self, width: f32, height: f32, density: f32) {
        let mut rng = rand::thread_rng();
        let count = ((BASE_PETALS as f32) * density).round() as usize;
        self.petals = (0..count)
            .map(|_| make_petal(&mut rng, width, height, false))
            .collect();
        self.width = width;
        self.height = height;
        self.density = density;
    }

    #[allow(dead_code)]
    pub fn celebrate(&mut self, on: bool) {
        self.celebrating = on;
    }

    pub fn update(&mut self, dt: f32) {
        let step = (dt * 60.0).clamp(0.0, 4.0);
        self.cycle_time += dt;

        let mut rng = rand::thread_rng();

        // Burst spawn during celebrate.
        if self.celebrating
            && self.petals.len() < BURST_MAX
            && (self.cycle_time - self.last_burst_t) > BURST_INTERVAL_SEC
        {
            self.last_burst_t = self.cycle_time;
            self.petals.push(make_petal(&mut rng, self.width, self.height, true));
        }

        // Update + cull.
        let w = self.width;
        let h = self.height;
        let celebrating = self.celebrating;
        let mut i = 0;
        while i < self.petals.len() {
            let p = &mut self.petals[i];
            p.flutter += 0.03 * step;
            p.x += (p.vx + p.flutter.sin() * 0.4) * step;
            p.y += p.vy * step;
            p.rot += p.vrot * step;

            if p.y > h + 30.0 || p.x > w + 30.0 {
                if celebrating {
                    // Drop celebrate-spawned petals so the population drains
                    // back to baseline when celebrate ends.
                    self.petals.swap_remove(i);
                    continue;
                }
                *p = make_petal(&mut rng, w, h, false);
            }
            i += 1;
        }

        // Maintain baseline when idle.
        if !self.celebrating {
            let target = ((BASE_PETALS as f32) * self.density).round() as usize;
            while self.petals.len() < target {
                self.petals.push(make_petal(&mut rng, self.width, self.height, false));
            }
        }
    }

    pub fn draw(&self, canvas: &Canvas) {
        for p in &self.petals {
            canvas.save();
            canvas.translate((p.x, p.y));
            // p.rot is in radians; Skia rotate takes degrees.
            canvas.rotate(p.rot * 180.0 / PI, None);

            // Teardrop bezier: from (0, -size) curving out and back to (0, size),
            // then mirrored back to the start.
            let mut path = Path::new();
            path.move_to((0.0, -p.size));
            path.cubic_to(
                (p.size * 0.9, -p.size * 0.6),
                (p.size * 0.6, p.size * 0.6),
                (0.0, p.size),
            );
            path.cubic_to(
                (-p.size * 0.6, p.size * 0.6),
                (-p.size * 0.9, -p.size * 0.6),
                (0.0, -p.size),
            );
            path.close();

            // 3-stop linear gradient across the petal (left→right).
            let c0 = OkLch::new(0.75, 0.09, p.hue, p.alpha).to_srgb_f32();
            let c1 = OkLch::new(0.82, 0.10, p.hue + 10.0, p.alpha).to_srgb_f32();
            let c2 = OkLch::new(0.68, 0.08, p.hue - 10.0, p.alpha * 0.6).to_srgb_f32();

            let shader = gradient_shader::linear(
                (Point::new(-p.size, 0.0), Point::new(p.size, 0.0)),
                gradient_shader::GradientShaderColors::ColorsInSpace(
                    &[
                        Color4f::new(c0[0], c0[1], c0[2], c0[3]),
                        Color4f::new(c1[0], c1[1], c1[2], c1[3]),
                        Color4f::new(c2[0], c2[1], c2[2], c2[3]),
                    ],
                    None,
                ),
                Some(&[0.0_f32, 0.5, 1.0][..]),
                TileMode::Clamp,
                None,
                None,
            )
            .expect("petal gradient");

            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_blend_mode(BlendMode::SrcOver);
            paint.set_shader(shader);
            canvas.draw_path(&path, &paint);

            canvas.restore();
        }
    }
}

fn make_petal<R: Rng + ?Sized>(rng: &mut R, w: f32, h: f32, burst: bool) -> Petal {
    Petal {
        x: if burst {
            -40.0 + rng.gen::<f32>() * w * 0.4
        } else {
            rng.gen::<f32>() * w
        },
        y: if burst {
            -40.0 - rng.gen::<f32>() * 200.0
        } else {
            -30.0 - rng.gen::<f32>() * h
        },
        vx: if burst {
            0.4 + rng.gen::<f32>() * 0.6
        } else {
            (rng.gen::<f32>() - 0.5) * 0.15
        },
        vy: if burst {
            0.35 + rng.gen::<f32>() * 0.4
        } else {
            0.18 + rng.gen::<f32>() * 0.3
        },
        rot: rng.gen::<f32>() * 2.0 * PI,
        vrot: (rng.gen::<f32>() - 0.5) * 0.02,
        size: 6.0 + rng.gen::<f32>() * 10.0,
        hue: 340.0 + rng.gen::<f32>() * 30.0,
        alpha: 0.35 + rng.gen::<f32>() * 0.45,
        flutter: rng.gen::<f32>() * 2.0 * PI,
    }
}
