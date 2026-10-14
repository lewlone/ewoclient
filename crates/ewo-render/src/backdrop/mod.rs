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
//!   5. pearl dust   — 110 airborne + 80 settled motes at density 1, screen
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
use skia_safe::{Canvas, Image};

use crate::offscreen::{self, LayerSize};
use crate::tokens;
use pearl_dust::PearlDust;
use petals::Petals;

/// How often the cached slow-layer image is recomputed. The wine/velvet/
/// caustics/bokeh layers drift on 8–60 s periods, so ~20 Hz is visually
/// indistinguishable from per-frame while running ~25× less blur work.
const CACHE_REFRESH_HZ: f32 = 20.0;

/// Cached render of the four slow backdrop layers, blitted every frame.
struct SlowCache {
    image: Image,
    /// `gl_backend::gpu_generation` the image was rendered under.
    generation: u64,
    /// Physical size of `image`: the logical card size times the canvas's
    /// device scale, so one cache pixel is one screen pixel (see `offscreen`).
    size: LayerSize,
    /// `accent_hue_shift` the image was painted with; a change repaints it.
    hue_shift: f32,
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

    /// Drop the cached slow-layer composite so the next draw rebuilds it (used
    /// when reduced motion flips and the layers change from drifting to still).
    pub fn invalidate_cache(&self) {
        *self.slow_cache.borrow_mut() = None;
    }

    pub fn update(&mut self, dt: f32, settings: &Settings) {
        // Reduced motion: dust and petals hold still (`dt = 0`). The prototype
        // drives them from `requestAnimationFrame`, which its
        // `prefers-reduced-motion` CSS cannot reach; as ambient motion they
        // are frozen here anyway (non-negotiable #6, see `ewo_core::motion`).
        // `--motion-speed` scales their clock like every other ambient loop's.
        let dt = ewo_core::motion::scaled_dt(dt, settings.motion_speed);
        self.pearl_dust.update(dt);
        self.petals.update(dt);
    }

    /// Device-pixel size of the cached slow-layer image, if one is built
    /// (tests use it to check the cache is not logical-sized under a scale).
    pub fn cached_layer_size(&self) -> Option<(i32, i32)> {
        self.slow_cache
            .borrow()
            .as_ref()
            .map(|c| (c.size.px_w, c.size.px_h))
    }

    pub fn disturb(&mut self) {
        self.pearl_dust.disturb();
    }

    pub fn celebrate(&mut self, on: bool) {
        // The petal burst is pure motion; under reduced motion it never starts.
        self.petals.celebrate(on && !ewo_core::motion::is_reduced());
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
        // Publish the frame's palette (accent hue shift included) before any
        // layer paints; `draw_frame` does this too, standalone callers rely
        // on this one.
        tokens::set_active(theme, settings);
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
        let Some(size) = offscreen::layer_size(canvas, w, h) else {
            return;
        };

        let need_refresh = match self.slow_cache.borrow().as_ref() {
            Some(c)
                if c.size == size
                    && c.hue_shift == settings.accent_hue_shift
                    && c.generation == crate::gl_backend::gpu_generation() =>
            {
                // Under reduced motion the layers are static, so the cached
                // composite never goes stale (and the three full-screen blurs
                // stop costing GPU time). Size/generation changes still rebuild.
                !ewo_core::motion::is_reduced() && time - c.last_refresh >= 1.0 / CACHE_REFRESH_HZ
            }
            _ => true,
        };

        if need_refresh {
            match offscreen::new_layer(canvas, size) {
                Some(mut surface) => {
                    {
                        let oc = surface.canvas();
                        // Match the card body fill so the opaque blit is
                        // bit-identical to drawing the layers in place.
                        oc.clear(tokens::bg_core());
                        draw_slow_layers_direct(oc, w, h, time, theme, settings);
                    }
                    let image = surface.image_snapshot();
                    *self.slow_cache.borrow_mut() = Some(SlowCache {
                        image,
                        generation: crate::gl_backend::gpu_generation(),
                        size,
                        hue_shift: settings.accent_hue_shift,
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
            // 1:1 blit: the cache is exactly the card's size in device
            // pixels, drawn with the scale undone, so nearest sampling
            // reproduces every pixel and a HiDPI display is not upscaled.
            offscreen::blit_layer(canvas, &c.image);
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
    caustics::draw(canvas, w, h, time, settings.motion_speed);
    bokeh::draw(canvas, w, h, time, settings.motion_speed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--motion-speed` scales how far the dust drifts per frame, live: it is
    /// not baked into a mote when it spawns.
    #[test]
    fn pearl_dust_drift_follows_motion_speed_per_frame() {
        // (This test binary never sets reduced motion: the flag stays off.)
        let at = |speed: f32| Settings {
            motion_speed: speed,
            ..Settings::default()
        };
        let mut b = Backdrop::new(960.0, 640.0, &at(1.0));
        let dt = 1.0 / 60.0;

        let y0 = b.pearl_dust.airborne_ys();
        b.update(dt, &at(1.0));
        let y1 = b.pearl_dust.airborne_ys();
        b.update(dt, &at(3.0));
        let y2 = b.pearl_dust.airborne_ys();

        // Spawned at y >= h, so nothing leaves the screen or respawns in two
        // frames; vertical velocity is constant per mote.
        for i in 0..y0.len() {
            let one = y0[i] - y1[i];
            let three = y1[i] - y2[i];
            assert!(one > 0.0);
            // Motes spawn at y in [h, h + 80) = [640, 720), where one f32 ulp
            // is ~6.1e-5; each difference carries up to an ulp of rounding,
            // so `three - 3*one` can be off by ~4 ulp (2.4e-4). A wrong speed
            // scale would be off by >= 2*one (~0.15), so 1e-3 still decides.
            assert!(
                (three - 3.0 * one).abs() < 1e-3,
                "mote {i}: {three} is not 3 x {one}"
            );
        }
    }
}
