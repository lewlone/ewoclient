//! Offscreen render of the vdrop menu panel to a PNG, so the dropdown chrome
//! (esp. the bottom-corner artifact) can be inspected directly instead of
//! guessed at. Run: `cargo run -p ewo-render --example dropdown_shot`.

use ewo_render::skia_safe::{
    surfaces, Color, Color4f, EncodedImageFormat, Paint, RRect, Rect,
};
use ewo_render::widgets::{draw_vdrop_menu, VdropState};
use ewo_render::FontStore;

fn main() {
    let (w, h) = (320i32, 240i32);
    let mut surface = surfaces::raster_n32_premul((w, h)).expect("raster surface");
    let canvas = surface.canvas();

    // Backdrop: near-black wine.
    canvas.clear(Color::from_argb(0xFF, 0x0A, 0x00, 0x08));
    // A lighter "settings glass panel" patch behind the dropdown — this is the
    // real case where the corner artifact shows: the dropdown sits over the
    // settings panel, not the bare backdrop.
    let mut panel = Paint::default();
    panel.set_anti_alias(true);
    panel.set_color4f(Color4f::new(0.32, 0.20, 0.27, 1.0), None);
    canvas.draw_rrect(
        RRect::new_rect_xy(Rect::from_xywh(20.0, 20.0, 280.0, 200.0), 18.0, 18.0),
        &panel,
    );

    let fonts = FontStore::new();
    // Settled, open menu (anim = 1, time_open large so the row stagger is done).
    let state = VdropState {
        open: true,
        selected: 1,
        anim: 1.0,
        time_open: 10.0,
        ..Default::default()
    };
    let opts = ["Windowed", "Borderless", "Fullscreen"];
    let menu = Rect::from_xywh(45.0, 40.0, 230.0, 3.0 * 38.0 + 12.0);
    draw_vdrop_menu(canvas, menu, false, &opts, &state, &fonts);

    let image = surface.image_snapshot();
    let data = image
        .encode(None, EncodedImageFormat::PNG, 100)
        .expect("encode png");
    std::fs::write("dropdown_shot.png", data.as_bytes()).expect("write png");

    // Zoom: crop the menu's bottom-left corner region and scale it up 6x so
    // the artifact is unmistakable.
    let crop = Rect::from_xywh(38.0, menu.bottom - 22.0, 60.0, 40.0);
    let (zw, zh) = (360i32, 240i32);
    let mut zoom = surfaces::raster_n32_premul((zw, zh)).expect("zoom surface");
    let zc = zoom.canvas();
    zc.clear(Color::BLACK);
    zc.scale((6.0, 6.0));
    zc.draw_image_rect(
        &image,
        Some((&crop, ewo_render::skia_safe::canvas::SrcRectConstraint::Fast)),
        Rect::from_xywh(0.0, 0.0, 60.0, 40.0),
        &Paint::default(),
    );
    let zdata = zoom
        .image_snapshot()
        .encode(None, EncodedImageFormat::PNG, 100)
        .expect("encode zoom");
    std::fs::write("dropdown_corner_zoom.png", zdata.as_bytes()).expect("write zoom");
    println!("wrote dropdown_shot.png + dropdown_corner_zoom.png");
}
