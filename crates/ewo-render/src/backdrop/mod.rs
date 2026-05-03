//! Backdrop — the layered velvet+caustics+bokeh+vignette+particles stack
//! inside the rounded card. Drawn on top of the card body, beneath the screen
//! content, the inset hairline rim, and the inner berry glow.
//!
//! Order (top of stack last):
//!
//!   1. wine         — static radial gradient base
//!   2. velvet folds — 3 oklch radial layers, blurred 40px, screen 85%
//!   3. caustics     — 5 oklch radials on 2 layers, blurred 30px, screen
//!   4. bokeh        — single huge soft pearl crossing every 60s, screen
//!   5. pearl dust   — 90 airborne + 60 settled motes, screen
//!   6. vignette     — radial dim at corners
//!
//! Petals (step 6) and velvet-folds turbulence/displacement (step 16 polish)
//! are pending.
//!
//! `Backdrop` owns the stateful subsystems (currently `PearlDust`; future
//! additions: `Petals`). Stateless layers stay as free draw functions.

pub mod bokeh;
pub mod caustics;
pub mod pearl_dust;
pub mod petals;
pub mod velvet_folds;
pub mod vignette;
pub mod wine;

use ewo_core::{Settings, Theme};
use skia_safe::Canvas;

use pearl_dust::PearlDust;
use petals::Petals;

pub struct Backdrop {
    pearl_dust: PearlDust,
    petals: Petals,
}

impl Backdrop {
    pub fn new(width: f32, height: f32, settings: &Settings) -> Self {
        Self {
            pearl_dust: PearlDust::new(width, height, settings),
            petals: Petals::new(width, height, settings.density),
        }
    }

    pub fn resize(&mut self, width: f32, height: f32, settings: &Settings) {
        self.pearl_dust.resize(width, height, settings);
        self.petals.resize(width, height, settings.density);
    }

    pub fn update(&mut self, dt: f32) {
        self.pearl_dust.update(dt);
        self.petals.update(dt);
    }

    pub fn disturb(&mut self) {
        self.pearl_dust.disturb();
    }

    #[allow(dead_code)]
    pub fn celebrate(&mut self, on: bool) {
        self.petals.celebrate(on);
    }

    pub fn draw(
        &self,
        canvas: &Canvas,
        w: f32,
        h: f32,
        time: f32,
        theme: &Theme,
        settings: &Settings,
    ) {
        wine::draw(canvas, w, h, theme);
        velvet_folds::draw(canvas, w, h, time, settings);
        caustics::draw(canvas, w, h, time);
        bokeh::draw(canvas, w, h, time);
        self.pearl_dust.draw(canvas);
        self.petals.draw(canvas);
        vignette::draw(canvas, w, h);
    }
}
