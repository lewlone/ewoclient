//! The user-tunable `Settings` act on the whole renderer, not on the one
//! layer that happened to read them:
//!
//! * `accent_hue_shift` repaints every accent (text, rims, backdrop layers,
//!   the cached slow layers) and changes nothing at 0,
//! * `motion_speed` divides the period of every ambient loop alike (caustics,
//!   bokeh, breathing text), and reduced motion still freezes them.
//!
//! The reduced-motion flag is process-global, so tests serialise on a lock.

use std::sync::{Mutex, MutexGuard};

use ewo_core::{motion, OkLch, Settings, Srgb, Theme};
use ewo_render::backdrop::{bokeh, caustics, Backdrop};
use ewo_render::skia_safe::{surfaces, AlphaType, Color, ColorType, ImageInfo, Rect, Surface};
use ewo_render::tokens;
use ewo_render::widgets::{draw_vslider, VsliderState};

mod common;
use common::{fonts, render, scenes, H, W};

static LOCK: Mutex<()> = Mutex::new(());

fn lock(reduced: bool) -> MutexGuard<'static, ()> {
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    motion::set_reduced(reduced);
    g
}

fn settings(hue: f32, speed: f32) -> Settings {
    Settings {
        // No random motes: the frames must be reproducible.
        density: 0.0,
        accent_hue_shift: hue,
        motion_speed: speed,
        ..Settings::default()
    }
}

fn scene(name: &str) -> common::Scene {
    scenes()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scene {name}"))
}

/// Mean signed hue change (degrees, circular) over the pixels the shift
/// repainted that are bright and saturated enough to have a hue.
fn mean_hue_change(before: &[u8], after: &[u8]) -> (f32, usize) {
    let oklch = |p: &[u8]| {
        OkLch::from_srgb(Srgb {
            r: p[0],
            g: p[1],
            b: p[2],
        })
    };
    let (mut sx, mut sy, mut n) = (0.0_f64, 0.0_f64, 0);
    for (a, b) in before
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.as_chunks::<4>().0)
    {
        // Skip untouched pixels and 1-2 level ripples: only a repainted
        // pixel says anything about its hue.
        let dist: i32 = (0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).sum();
        if a[3] < 255 || dist < 12 {
            continue;
        }
        let (ca, cb) = (oklch(a), oklch(b));
        if ca.l > 0.4 && ca.c > 0.04 && cb.c > 0.04 {
            let d = ((cb.h - ca.h) as f64).to_radians();
            sx += d.cos();
            sy += d.sin();
            n += 1;
        }
    }
    (sy.atan2(sx).to_degrees().rem_euclid(360.0) as f32, n)
}

#[test]
fn accent_hue_shift_repaints_the_accents_and_zero_changes_nothing() {
    let Some(fonts) = fonts() else { return };
    let _g = lock(false);
    let theme = Theme::VELVET;
    for name in ["main_menu", "instances", "settings_graphics"] {
        let s = scene(name);
        let base = render(&fonts, &s, &theme, &settings(0.0, 1.0));
        let shifted = render(&fonts, &s, &theme, &settings(120.0, 1.0));
        assert!(base != shifted, "{name}: a 120° shift changed nothing");
        // The palette is per frame: a later frame at 0° is the original again.
        let again = render(&fonts, &s, &theme, &settings(0.0, 1.0));
        assert!(
            base == again,
            "{name}: the hue shift leaked into the next frame"
        );
    }
}

/// A widget painted straight onto black (no screen-blended backdrop mixing
/// hues): its accent gradient must rotate by the shift.
#[test]
fn widget_accents_rotate_by_the_shift() {
    let _g = lock(false);
    let theme = Theme::VELVET;
    let bar = |hue: f32| -> Vec<u8> {
        let st = settings(hue, 1.0);
        let mut surface = surfaces::raster_n32_premul((480, 60)).unwrap();
        surface.canvas().clear(Color::BLACK);
        tokens::set_active(&theme, &st);
        let mut slider = VsliderState::new(0.85, 0.0, 1.0);
        slider.dragging = true;
        draw_vslider(
            surface.canvas(),
            Rect::from_xywh(20.0, 10.0, 440.0, 40.0),
            &slider,
            1.0,
            &st,
        );
        tokens::set_active(&theme, &Settings::default());
        pixels(&mut surface)
    };
    let base = bar(0.0);
    for shift in [120.0_f32, -90.0] {
        let moved = bar(shift);
        let (mean, n) = mean_hue_change(&base, &moved);
        assert!(n > 50, "shift {shift}: only {n} repainted accent pixels");
        let want = shift.rem_euclid(360.0);
        let err = (mean - want + 540.0).rem_euclid(360.0) - 180.0;
        assert!(err.abs() < 25.0, "shift {shift}: accents moved {mean}°");
    }
    assert!(bar(0.0) == base, "the shift leaked past the frame");
}

#[test]
fn the_slow_backdrop_cache_follows_the_hue_shift() {
    let _g = lock(false);
    let theme = Theme::VELVET;
    let backdrop = Backdrop::new(W as f32, H as f32, &settings(0.0, 1.0));
    let frame = |hue: f32| -> Vec<u8> {
        let mut surface = surfaces::raster_n32_premul((W, H)).unwrap();
        // Same clock for every frame, so only the hue can invalidate the cache.
        backdrop.draw(
            surface.canvas(),
            W as f32,
            H as f32,
            3.0,
            &theme,
            &settings(hue, 1.0),
        );
        pixels(&mut surface)
    };
    let a = frame(0.0);
    let b = frame(90.0);
    let c = frame(0.0);
    assert!(a != b, "cached layers kept the old hue");
    assert!(a == c, "returning to 0° did not restore the layers");
}

fn pixels(surface: &mut Surface) -> Vec<u8> {
    // Explicit RGBA: `n32` is BGRA on Windows and the hue maths reads R first.
    let info = ImageInfo::new(
        surface.image_info().dimensions(),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let stride = info.min_row_bytes();
    let mut px = vec![0u8; stride * info.height() as usize];
    assert!(surface.read_pixels(&info, &mut px, stride, (0, 0)));
    px
}

fn layer_frame(draw: impl Fn(&ewo_render::skia_safe::Canvas)) -> Vec<u8> {
    let mut surface = surfaces::raster_n32_premul((W, H)).unwrap();
    surface
        .canvas()
        .clear(Color::from_argb(0xFF, 0x1A, 0x08, 0x14));
    draw(surface.canvas());
    pixels(&mut surface)
}

#[test]
fn motion_speed_scales_the_caustics_and_the_bokeh_clock() {
    let _g = lock(false);
    let (w, h) = (W as f32, H as f32);
    for t in [3.0_f32, 11.5, 27.0] {
        let slow = layer_frame(|c| caustics::draw(c, w, h, t, 1.0));
        let fast = layer_frame(|c| caustics::draw(c, w, h, t, 2.0));
        let twice = layer_frame(|c| caustics::draw(c, w, h, 2.0 * t, 1.0));
        assert!(fast == twice, "caustics: speed 2 at t={t} != speed 1 at 2t");
        assert!(fast != slow, "caustics ignore motion_speed at t={t}");

        let slow = layer_frame(|c| bokeh::draw(c, w, h, t, 1.0));
        let fast = layer_frame(|c| bokeh::draw(c, w, h, t, 2.0));
        let twice = layer_frame(|c| bokeh::draw(c, w, h, 2.0 * t, 1.0));
        assert!(fast == twice, "bokeh: speed 2 at t={t} != speed 1 at 2t");
        assert!(fast != slow, "bokeh ignores motion_speed at t={t}");
    }
}

#[test]
fn motion_speed_scales_the_breathing_text() {
    // CSS: `animation: bt-breath calc(8s / var(--motion-speed))`.
    let _g = lock(false);
    for t in [0.7_f32, 2.0, 3.1, 5.5] {
        let at_2x = ewo_render::text::breath_envelope(t, 8.0, 2.0);
        let at_1x_twice = ewo_render::text::breath_envelope(2.0 * t, 8.0, 1.0);
        assert!(
            (at_2x - at_1x_twice).abs() < 1e-6,
            "t={t}: {at_2x} vs {at_1x_twice}"
        );
    }
    // A quarter period in is mid-ramp at speed 1 and the peak at speed 2.
    let quarter = ewo_render::text::breath_envelope(2.0, 8.0, 1.0);
    let peak = ewo_render::text::breath_envelope(2.0, 8.0, 2.0);
    assert!(quarter > 0.4 && quarter < 0.6, "{quarter}");
    assert!(peak > 0.99, "{peak}");
}

#[test]
fn reduced_motion_still_freezes_the_loops_at_any_speed() {
    let _g = lock(true);
    let (w, h) = (W as f32, H as f32);
    let rest = layer_frame(|c| {
        caustics::draw(c, w, h, 0.0, 1.0);
        bokeh::draw(c, w, h, 0.0, 1.0);
    });
    for (t, speed) in [(9.0_f32, 3.0_f32), (31.0, 0.25)] {
        let f = layer_frame(|c| {
            caustics::draw(c, w, h, t, speed);
            bokeh::draw(c, w, h, t, speed);
        });
        assert!(f == rest, "t={t} speed={speed}: ambient layers moved");
    }
    assert_eq!(ewo_render::text::breath_envelope(5.0, 8.0, 3.0), 0.0);
    motion::set_reduced(false);
}
