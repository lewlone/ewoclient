//! Non-negotiable #6 (reduced motion), checked end to end on the renderer:
//!
//! * every transition tick lands on its end state within one frame, and
//! * every ambient loop is frozen, so a frame drawn at any clock value is
//!   byte-identical to one drawn at clock 0.
//!
//! The reduced-motion flag is process-global, so these tests serialise on a
//! lock and always restore it. This file is its own test binary, so it cannot
//! disturb `text_not_scaled.rs`.

use std::sync::Mutex;

use ewo_core::{motion, Settings, Theme};
use ewo_render::backdrop::Backdrop;
use ewo_render::screens::about_modal::AboutModalState;
use ewo_render::skia_safe::{surfaces, Color, ImageInfo, Rect, Surface};
use ewo_render::widgets::{
    draw_glass_panel, draw_vbtn, VbtnState, VdropState, VstatusState, VtoggleState,
};
use ewo_render::FontStore;

static LOCK: Mutex<()> = Mutex::new(());

/// Holds the lock and the reduced-motion setting for one test.
struct Reduced<'a>(#[allow(dead_code)] std::sync::MutexGuard<'a, ()>);

impl<'a> Reduced<'a> {
    fn set(on: bool) -> Self {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        motion::set_reduced(on);
        Reduced(g)
    }
}

impl Drop for Reduced<'_> {
    fn drop(&mut self) {
        motion::set_reduced(false);
    }
}

const FRAME: f32 = 1.0 / 60.0;

#[test]
fn every_transition_lands_within_one_frame() {
    let _g = Reduced::set(true);

    let mut modal = AboutModalState::default();
    modal.open();
    modal.tick(FRAME);
    assert_eq!(modal.anim, 1.0, "modal entrance");

    let mut drop = VdropState {
        open: true,
        ..Default::default()
    };
    drop.tick(FRAME);
    assert_eq!(drop.anim, 1.0, "dropdown open");
    drop.open = false;
    drop.tick(FRAME);
    assert_eq!(drop.anim, 0.0, "dropdown close");

    let mut toggle = VtoggleState {
        on: true,
        ..Default::default()
    };
    toggle.tick(FRAME);
    assert_eq!(toggle.anim, 1.0, "toggle");

    let mut btn = VbtnState {
        hover: true,
        ..Default::default()
    };
    btn.tick(FRAME);
    assert_eq!(btn.hover_anim, 1.0, "button hover");

    let mut status = VstatusState::new("a");
    status.set("b");
    status.tick(FRAME);
    assert_eq!(status.anim, 1.0, "status crossfade");
}

#[test]
fn transitions_still_take_their_time_when_motion_is_on() {
    let _g = Reduced::set(false);

    let mut modal = AboutModalState::default();
    modal.open();
    modal.tick(FRAME);
    assert!(
        modal.anim > 0.0 && modal.anim < 0.5,
        "modal entrance {}",
        modal.anim
    );

    let mut toggle = VtoggleState {
        on: true,
        ..Default::default()
    };
    toggle.tick(FRAME);
    assert!(
        toggle.anim > 0.0 && toggle.anim < 0.5,
        "toggle {}",
        toggle.anim
    );
}

const W: i32 = 480;
const H: i32 = 360;

fn pixels(surface: &mut Surface) -> Vec<u8> {
    let info = ImageInfo::new_n32_premul((W, H), None);
    let stride = (W * 4) as usize;
    let mut px = vec![0u8; stride * H as usize];
    assert!(surface.read_pixels(&info, &mut px, stride, (0, 0)));
    px
}

fn fresh() -> Surface {
    let mut s = surfaces::raster_n32_premul((W, H)).expect("raster surface");
    s.canvas().clear(Color::from_argb(0xFF, 0x1A, 0x08, 0x14));
    s
}

/// One frame of every ambient loop the renderer owns, at clock value `time`:
/// the three slow backdrop layers, glass-panel breath and rims, a button's
/// rim pulse and sheen, and breathing text.
///
/// `backdrop` is shared across frames because its dust and petals are seeded
/// randomly; its slow-layer cache is dropped before every frame so the layers
/// themselves are redrawn at `time` rather than blitted from the first frame.
fn ambient_frame(backdrop: &Backdrop, fonts: &FontStore, time: f32) -> Vec<u8> {
    let settings = Settings::default();
    let theme = Theme::VELVET;
    let mut s = fresh();
    let canvas = s.canvas();

    backdrop.invalidate_cache();
    backdrop.draw(canvas, W as f32, H as f32, time, &theme, &settings);

    let panel = Rect::from_xywh(20.0, 20.0, 440.0, 160.0);
    draw_glass_panel(canvas, panel, true, time, &settings, |_| {});

    let btn = Rect::from_xywh(120.0, 220.0, 240.0, 52.0);
    draw_vbtn(
        canvas,
        btn,
        "Launch",
        &VbtnState::default(),
        time,
        1.0,
        fonts,
        true,
    );

    let font = fonts.newsreader(28.0);
    let mut paint = ewo_render::skia_safe::Paint::default();
    paint.set_color(Color::WHITE);
    ewo_render::text::draw_breathing(
        canvas,
        "Breathing",
        (40.0, 330.0),
        &font,
        &paint,
        0.0,
        0.02,
        time,
        8.0,
    );
    pixels(&mut s)
}

#[test]
fn ambient_loops_are_frozen() {
    let Some(fonts) = fonts() else { return };
    let _g = Reduced::set(true);
    let backdrop = Backdrop::new(W as f32, H as f32, &Settings::default());
    let at0 = ambient_frame(&backdrop, &fonts, 0.0);
    for t in [0.37, 5.0, 31.9, 123.4] {
        assert!(
            ambient_frame(&backdrop, &fonts, t) == at0,
            "ambient layers moved at t={t}"
        );
    }
}

#[test]
fn ambient_loops_move_when_motion_is_on() {
    let Some(fonts) = fonts() else { return };
    let _g = Reduced::set(false);
    let backdrop = Backdrop::new(W as f32, H as f32, &Settings::default());
    let at0 = ambient_frame(&backdrop, &fonts, 0.0);
    assert!(
        ambient_frame(&backdrop, &fonts, 13.7) != at0,
        "control: frames should differ with motion on"
    );
}

fn fonts() -> Option<FontStore> {
    if !std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts")).exists() {
        if std::env::var_os("REWO_REQUIRE_ASSETS").is_some() {
            panic!("assets/fonts missing");
        }
        eprintln!("SKIP: assets/fonts missing");
        return None;
    }
    Some(FontStore::new())
}
