//! HiDPI: the launcher paints in logical pixels under a `scale_factor` CTM, so
//! a cache created at the logical size and blitted under that CTM is upscaled
//! (soft backdrop and glow at 125 % / 150 % / 200 %). The caches must be the
//! physical size and blit 1:1.
//!
//! Offscreen raster surface standing in for the window, with the same CTM the
//! launcher applies.

use std::sync::Mutex;

use ewo_core::{motion, Screen, Settings, Theme};
use ewo_render::app_window::glow_cache_size;
use ewo_render::backdrop::{bokeh, caustics, vignette, wine, Backdrop};
use ewo_render::skia_safe::{surfaces, AlphaType, Color, ColorType, ImageInfo};
use ewo_render::tokens;

mod common;
use common::{fonts, physical, render_scaled, Scene, H, W};

// The reduced-motion flag is process-global and `Backdrop::draw` reads it.
static LOCK: Mutex<()> = Mutex::new(());

fn settings() -> Settings {
    Settings {
        density: 0.0,
        ..Settings::default()
    }
}

/// The cached backdrop and glow images are exactly the physical size.
#[test]
fn caches_are_physical_size_under_a_display_scale() {
    let Some(fonts) = fonts() else { return };
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    motion::set_reduced(false);

    for scale in [1.0_f32, 1.25, 1.5, 2.0] {
        let st = settings();
        let backdrop = Backdrop::new(W as f32, H as f32, &st);
        let scene = Scene::new("hidpi", Screen::MainMenu);
        let px = render_scaled(&fonts, &scene, &Theme::VELVET, &st, scale, &backdrop);

        let want = physical(scale);
        assert_eq!(
            px.len(),
            (want.0 * want.1 * 4) as usize,
            "scale {scale}: frame size"
        );
        assert_eq!(
            backdrop.cached_layer_size(),
            Some(want),
            "scale {scale}: backdrop cache is not the physical size"
        );
        assert_eq!(
            glow_cache_size(),
            Some(want),
            "scale {scale}: inner-glow cache is not the physical size"
        );
    }
}

/// Blitting the cache is a pure copy: the cached backdrop is pixel-identical
/// to drawing the same layers straight onto the scaled canvas, at any scale.
/// (Upscaling a logical-size cache fails this at every scale above 1.)
#[test]
fn cached_backdrop_equals_a_direct_draw_at_every_scale() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    motion::set_reduced(false);
    let st = settings();
    let theme = Theme::VELVET;
    let (w, h) = (W as f32, H as f32);
    let time = 4.2;

    let read = |surface: &mut ewo_render::skia_safe::Surface, pw: i32, ph: i32| {
        let info = ImageInfo::new((pw, ph), ColorType::RGBA8888, AlphaType::Premul, None);
        let stride = (pw * 4) as usize;
        let mut px = vec![0u8; stride * ph as usize];
        assert!(surface.read_pixels(&info, &mut px, stride, (0, 0)));
        px
    };

    for scale in [1.0_f32, 1.25, 2.0] {
        let (pw, ph) = physical(scale);

        let mut cached = surfaces::raster_n32_premul((pw, ph)).unwrap();
        cached.canvas().clear(Color::BLACK);
        cached.canvas().scale((scale, scale));
        Backdrop::new(w, h, &st).draw(cached.canvas(), w, h, time, &theme, &st);

        let mut direct = surfaces::raster_n32_premul((pw, ph)).unwrap();
        direct.canvas().clear(Color::BLACK);
        direct.canvas().scale((scale, scale));
        {
            let c = direct.canvas();
            tokens::set_active(&theme, &st);
            wine::draw(c, w, h, &theme);
            ewo_render::backdrop::velvet_folds::draw(c, w, h, time, &st);
            caustics::draw(c, w, h, time, st.motion_speed);
            bokeh::draw(c, w, h, time, st.motion_speed);
            vignette::draw(c, w, h);
        }

        assert!(
            read(&mut cached, pw, ph) == read(&mut direct, pw, ph),
            "scale {scale}: the cached backdrop is not a 1:1 copy of the direct draw"
        );
    }
}
