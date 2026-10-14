//! Non-negotiable #2 ("don't transform anything that contains text"), checked
//! on pixels: the bright-text bounding box of an entrance / press frame must
//! have exactly the size of the settled frame. A scaled text container shrinks
//! it (the old 0.97 modal scale cost ~9 px on a ~300 px line, the 0.96 button
//! press ~10 px on a label).
//!
//! Offscreen raster surface, no GPU. Fonts load from `assets/fonts/`, so each
//! test skips (loudly, and fails under `REWO_REQUIRE_ASSETS=1`) if they are
//! missing.

use ewo_render::screens::about_modal::{self, AboutModalState};
use ewo_render::skia_safe::{surfaces, Color, ImageInfo, Rect, Surface};
use ewo_render::widgets::{draw_vbtn, draw_vdrop_menu, VbtnState, VdropState};
use ewo_render::FontStore;

const W: i32 = 640;
const H: i32 = 480;
/// A pixel counts as text when it is at least this fraction of the brightest
/// pixel in the band: the frames under test are at different opacities, so an
/// absolute threshold would compare different glyph coverage. Chrome (card
/// fill, rim, tint) stays well under this share of pearl/warm-white text.
const TEXT_SHARE: f32 = 0.65;

fn surface() -> Surface {
    let mut s = surfaces::raster_n32_premul((W, H)).expect("raster surface");
    s.canvas().clear(Color::BLACK);
    s
}

/// Bounding box `(w, h)` of the text-bright pixels within rows `y0..y1`.
fn bright_bbox(surface: &mut Surface, y0: i32, y1: i32) -> (i32, i32) {
    let info = ImageInfo::new_n32_premul((W, H), None);
    let stride = (W * 4) as usize;
    let mut px = vec![0u8; stride * H as usize];
    assert!(surface.read_pixels(&info, &mut px, stride, (0, 0)));
    let lum = |x: i32, y: i32| {
        let i = y as usize * stride + x as usize * 4;
        px[i].max(px[i + 1]).max(px[i + 2])
    };
    let peak = (y0..y1)
        .flat_map(|y| (0..W).map(move |x| (x, y)))
        .map(|(x, y)| lum(x, y))
        .max()
        .unwrap();
    let min = (peak as f32 * TEXT_SHARE) as u8;
    let (mut bx0, mut by0, mut bx1, mut by1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for y in y0..y1 {
        for x in 0..W {
            if lum(x, y) >= min {
                bx0 = bx0.min(x);
                bx1 = bx1.max(x);
                by0 = by0.min(y);
                by1 = by1.max(y);
            }
        }
    }
    assert!(bx1 >= bx0, "no text pixels found");
    (bx1 - bx0 + 1, by1 - by0 + 1)
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

fn about_frame(fonts: &FontStore, anim: f32) -> Surface {
    let mut s = surface();
    let state = AboutModalState {
        open: true,
        anim,
        ..Default::default()
    };
    about_modal::draw_modal(s.canvas(), fonts, W as f32, H as f32, &state);
    s
}

#[test]
fn about_modal_text_keeps_its_size_while_entering() {
    let Some(fonts) = fonts() else { return };
    // Title band: "EwoClient" (Fraunces 48) sits at y ~ 120..165 in this layout;
    // the band is wide enough for the <= 10 px entrance translate.
    let (y0, y1) = (105, 180);
    let settled = bright_bbox(&mut about_frame(&fonts, 1.0), y0, y1);
    // `anim` is linear time; silk eases it (0.2 -> 0.67 opacity, 0.4 -> 0.92).
    // The old 0.97 -> 1 scale was 0.990 / 0.998 there: 2-3 px on the title.
    for anim in [0.2_f32, 0.3, 0.4] {
        let got = bright_bbox(&mut about_frame(&fonts, anim), y0, y1);
        assert_eq!(got, settled, "title size changed at anim={anim}");
    }
}

#[test]
fn button_label_keeps_its_size_while_pressed() {
    let Some(fonts) = fonts() else { return };
    let bounds = Rect::from_xywh(160.0, 200.0, 320.0, 56.0);
    let frame = |pressing: bool| {
        let mut s = surface();
        let state = VbtnState {
            pressing,
            ..Default::default()
        };
        draw_vbtn(
            s.canvas(),
            bounds,
            "Launch the game",
            &state,
            0.0,
            1.0,
            &fonts,
            true,
        );
        s
    };
    let rest = bright_bbox(&mut frame(false), 0, H);
    let pressed = bright_bbox(&mut frame(true), 0, H);
    assert_eq!(rest, pressed, "pressing must not scale the label");
}

#[test]
fn dropdown_rows_keep_their_size_while_opening() {
    let Some(fonts) = fonts() else { return };
    let menu = Rect::from_xywh(20.0, 80.0, 600.0, 3.0 * 38.0 + 12.0);
    // One long row so a 0.98-ish scale would move the bbox by whole pixels.
    let opts = [
        "Windowed",
        "Borderless fullscreen, with a deliberately long label to widen the row text",
        "Fullscreen",
    ];
    let frame = |anim: f32| {
        let mut s = surface();
        let state = VdropState {
            open: true,
            anim,
            time_open: 10.0,
            ..Default::default()
        };
        draw_vdrop_menu(s.canvas(), menu, false, &opts, &state, &fonts);
        s
    };
    // Band over the long middle row only (rows are 38 px, menu padded by 6).
    let (y0, y1) = (126, 160);
    let settled = bright_bbox(&mut frame(1.0), y0, y1);
    for anim in [0.2_f32, 0.3, 0.5] {
        let got = bright_bbox(&mut frame(anim), y0, y1);
        assert_eq!(got.0, settled.0, "row text width changed at anim={anim}");
    }
}
