//! Offscreen renders of text-bearing surfaces mid-entrance / mid-press, so
//! non-negotiable #2 (never scale text) can be inspected by eye as well as by
//! `tests/text_not_scaled.rs`. Writes `modal_shot_*.png` into the cwd.
//!
//! `cargo run -p ewo-render --example modal_shot`
//! `cargo run -p ewo-render --example modal_shot -- --reduced`  (reduced motion on)

use ewo_core::motion;
use ewo_render::screens::about_modal::{self, AboutModalState};
use ewo_render::skia_safe::{surfaces, Color, EncodedImageFormat, Rect, Surface};
use ewo_render::widgets::{draw_vbtn, draw_vdrop_menu, VbtnState, VdropState};
use ewo_render::FontStore;

const W: i32 = 640;
const H: i32 = 480;

fn save(surface: &mut Surface, name: &str) {
    let data = surface
        .image_snapshot()
        .encode(None, EncodedImageFormat::PNG, 100)
        .expect("encode png");
    std::fs::write(name, data.as_bytes()).expect("write png");
    println!("wrote {name}");
}

fn main() {
    if std::env::args().any(|a| a == "--reduced") {
        motion::set_reduced(true);
        println!("reduced motion ON");
    }
    let fonts = FontStore::new();
    let backdrop = Color::from_argb(0xFF, 0x1A, 0x08, 0x14);

    // About modal: settled, and two frames mid-entrance (silk is front-loaded,
    // so linear anim 0.08 is already ~45% in).
    for (tag, anim) in [("a1.00", 1.0_f32), ("a0.08", 0.08), ("a0.04", 0.04)] {
        let mut s = surfaces::raster_n32_premul((W, H)).expect("surface");
        s.canvas().clear(backdrop);
        let state = AboutModalState {
            open: true,
            anim,
            ..Default::default()
        };
        about_modal::draw_modal(s.canvas(), &fonts, W as f32, H as f32, &state);
        save(&mut s, &format!("modal_shot_about_{tag}.png"));
    }

    // Button: rest vs pressed.
    for (tag, pressing) in [("rest", false), ("pressed", true)] {
        let mut s = surfaces::raster_n32_premul((W, 160)).expect("surface");
        s.canvas().clear(backdrop);
        let state = VbtnState {
            pressing,
            ..Default::default()
        };
        let b = Rect::from_xywh(160.0, 50.0, 320.0, 56.0);
        draw_vbtn(
            s.canvas(),
            b,
            "Launch the game",
            &state,
            0.0,
            1.0,
            &fonts,
            true,
        );
        save(&mut s, &format!("modal_shot_vbtn_{tag}.png"));
    }

    // Dropdown menu: settled and mid-open.
    for (tag, anim) in [("a1.00", 1.0_f32), ("a0.10", 0.10)] {
        let mut s = surfaces::raster_n32_premul((W, 240)).expect("surface");
        s.canvas().clear(backdrop);
        let state = VdropState {
            open: true,
            anim,
            time_open: 10.0,
            ..Default::default()
        };
        let menu = Rect::from_xywh(120.0, 40.0, 400.0, 3.0 * 38.0 + 12.0);
        let opts = ["Windowed", "Borderless fullscreen", "Fullscreen"];
        draw_vdrop_menu(s.canvas(), menu, false, &opts, &state, &fonts);
        save(&mut s, &format!("modal_shot_vdrop_{tag}.png"));
    }
}
