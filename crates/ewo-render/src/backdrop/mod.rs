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
//! `Backdrop` owns the stateful subsystems (`PearlDust`, `Petals`) plus a
//! cached render of the four *slow* layers (wine→velvet→caustics→bokeh).
//!
//! ## Why the slow layers are cached
//!
//! Layers 1–4 are dominated by three full-screen Gaussian blurs (sigma 20/15/20)
//! plus a fractal-noise displacement — by far the heaviest GPU work in the
//! launcher. Yet they animate on 8–60 s timescales: between two adjacent frames
//! at 500 fps they move a tiny fraction of a pixel. Recomputing them every frame
//! is pure waste. Instead we render them into an offscreen surface refreshed at
//! `CACHE_REFRESH_HZ` (≈20×/sec) and blit that 1:1 every frame, then draw the
//! *fast* layers (pearl dust, petals) and the cheap vignette live on top. The
//! heavy blur work then runs ~20×/sec instead of ~500×/sec — a ~25× cut to the
//! per-frame backdrop cost — with no perceptible change (the layers' motion is
//! far slower than 20 Hz). This is the same "cache the slow clock" trick the
//! in-game HUD's frosted backdrop uses (`ewo-jni::refresh_frost`).

pub mod bokeh;
pub mod caustics;
pub mod pearl_dust;
pub mod petals;
pub mod velvet_folds;
pub mod vignette;
pub mod wine;

use std::cell::RefCell;

use ewo_core::{Settings, Theme};
use skia_safe::{AlphaType, Canvas, Color, ColorType, Image, ImageInfo};

use pearl_dust::PearlDust;
use petals::Petals;

/// How often the cached slow-layer image is recomputed. The wine/velvet/
/// caustics/bokeh layers drift on 8–60 s periods, so ~20 Hz is visually
/// indistinguishable from per-frame while running ~25× less blur work.
const CACHE_REFRESH_HZ: f32 = 20.0;

/// Cached render of the four slow backdrop layers, blitted every frame.
struct SlowCache {
    image: Image,
    w: i32,
    h: i32,
    /// Wall-clock seconds at which `image` was rendered.
    last_refresh: f32,
}

pub struct Backdrop {
    pearl_dust: PearlDust,
    petals: Petals,
    /// Cached wine→velvet→caustics→bokeh composite (see module docs). Interior
    /// mutability so `draw(&self)` can refresh it on its own slow clock.
    slow_cache: RefCell<Option<SlowCache>>,
}

impl Backdrop {
    pub fn new(width: f32, height: f32, settings: &Settings) -> Self {
        Self {
            pearl_dust: PearlDust::new(width, height, settings),
            petals: Petals::new(width, height, settings.density),
            slow_cache: RefCell::new(None),
        }
    }

    pub fn resize(&mut self, width: f32, height: f32, settings: &Settings) {
        self.pearl_dust.resize(width, height, settings);
        self.petals.resize(width, height, settings.density);
        // Drop the cached slow-layer image — it's the old size now. Rebuilt
        // lazily on the next draw.
        *self.slow_cache.borrow_mut() = None;
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
        self.draw_slow_layers_cached(canvas, w, h, time, theme, settings);
        self.pearl_dust.draw(canvas);
        self.petals.draw(canvas);
        vignette::draw(canvas, w, h);
    }

    /// Blit the cached wine→bokeh composite, refreshing it at most
    /// `CACHE_REFRESH_HZ`. Falls back to drawing the layers straight onto
    /// `canvas` if an offscreen surface can't be allocated (e.g. a non-GPU
    /// canvas, or a degenerate size).
    fn draw_slow_layers_cached(
        &self,
        canvas: &Canvas,
        w: f32,
        h: f32,
        time: f32,
        theme: &Theme,
        settings: &Settings,
    ) {
        let wi = w.round() as i32;
        let hi = h.round() as i32;
        if wi <= 0 || hi <= 0 {
            return;
        }

        let need_refresh = match self.slow_cache.borrow().as_ref() {
            Some(c) if c.w == wi && c.h == hi => {
                time - c.last_refresh >= 1.0 / CACHE_REFRESH_HZ
            }
            _ => true,
        };

        if need_refresh {
            let info = ImageInfo::new((wi, hi), ColorType::RGBA8888, AlphaType::Premul, None);
            match canvas.new_surface(&info, None) {
                Some(mut surface) => {
                    {
                        let oc = surface.canvas();
                        // Match the card body fill so the opaque blit is
                        // bit-identical to drawing the layers in place.
                        oc.clear(Color::BLACK);
                        draw_slow_layers_direct(oc, w, h, time, theme, settings);
                    }
                    let image = surface.image_snapshot();
                    *self.slow_cache.borrow_mut() = Some(SlowCache {
                        image,
                        w: wi,
                        h: hi,
                        last_refresh: time,
                    });
                }
                None => {
                    // No offscreen available — draw straight to the canvas.
                    draw_slow_layers_direct(canvas, w, h, time, theme, settings);
                    return;
                }
            }
        }

        if let Some(c) = self.slow_cache.borrow().as_ref() {
            // 1:1 blit (the cache is exactly card-sized), so default nearest
            // sampling reproduces every pixel; no resampling.
            canvas.draw_image(&c.image, (0.0, 0.0), None);
        }
    }
}

/// Draw the four slow layers straight onto `canvas` in card-local coords.
fn draw_slow_layers_direct(
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
}
