//! Side-by-side of the in-game widget plate: the current flat `.iw-shell`
//! against the new refracting [`liquid_glass`] plate, over a real Minecraft
//! frame.
//!
//! The point is to make the difference falsifiable rather than a matter of
//! adjectives. Panel 1 is a faithful reproduction of `hud.rs::draw_iw_shell`
//! (same six chrome layers, same alphas) so nothing is being strawmanned.
//!
//! ```text
//! cargo run -p ewo-render --example glass_shot -- --bg shot.png --crop 0,780,2560,660
//! ```

use ewo_render::skia_safe::{
    canvas::SrcRectConstraint, gradient_shader, image_filters, surfaces, BlurStyle, Canvas, Color,
    Color4f, Data, EncodedImageFormat, Font, Image, MaskFilter, Paint, PaintStyle, Point, RRect,
    Rect, TileMode,
};
use ewo_render::widgets::liquid_glass::{self, Params};
use ewo_render::FontStore;

const PEARL: (u8, u8, u8) = (0xF4, 0xE8, 0xEA);
const MAUVE: (u8, u8, u8) = (0x9A, 0x80, 0x87);
const ROSE: (u8, u8, u8) = (0xE5, 0xB8, 0xC5);
const WINE: (u8, u8, u8) = (0x12, 0x00, 0x10);

fn rgba(c: (u8, u8, u8), a: f32) -> Color4f {
    Color4f::new(c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0, a)
}

/// One plate variant to render.
struct Variant {
    label: String,
    kind: Kind,
}

enum Kind {
    /// Faithful reproduction of the current `hud.rs::draw_iw_shell`.
    Current,
    Glass(Params),
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bg_path = flag(&args, "--bg");
    let crop = flag(&args, "--crop").and_then(|s| parse_crop(&s));
    let out = flag(&args, "--out").unwrap_or_else(|| "glass_shot.png".to_string());
    let time: f32 = flag(&args, "--time").and_then(|s| s.parse().ok()).unwrap_or(0.0);

    // Blur ladder. Index 0 is the lightly-blurred rim source; index 1 is the
    // heavy frost the interior samples.
    let blur_sigmas = [2.0_f32, 9.0];

    let glass = |edge: f32, strength: f32, disperse: f32, specular: f32| {
        Kind::Glass(Params { edge, strength, disperse, specular, ..Params::WIDGET })
    };
    let variants = vec![
        Variant {
            label: "CURRENT · flat wine 0.50".into(),
            kind: Kind::Current,

        },
        Variant {
            label: "edge 16 · str 12 · subtle".into(),
            kind: glass(16.0, 12.0, 1.0, 0.14),

        },
        Variant {
            label: "edge 18 · str 20 · WIDGET preset".into(),
            kind: Kind::Glass(Params::WIDGET),

        },
        Variant {
            label: "edge 24 · str 30".into(),
            kind: glass(24.0, 30.0, 2.4, 0.24),

        },
        Variant {
            label: "edge 30 · str 44 · thick".into(),
            kind: glass(30.0, 44.0, 3.4, 0.30),

        },
        Variant {
            label: "edge 30 · str 44 · no tint (glassiest)".into(),
            kind: Kind::Glass(Params {
                edge: 30.0,
                strength: 44.0,
                disperse: 3.4,
                specular: 0.34,
                tint: ewo_render::skia_safe::Color4f::new(0.094, 0.0, 0.055, 0.34),
                ..Params::WIDGET
            }),

        },
    ];

    let (w, h) = (1000i32, 860i32);
    let mut surface = surfaces::raster_n32_premul((w, h)).expect("surface");
    let (wf, hf) = (w as f32, h as f32);

    // Background.
    let bg = bg_path.as_deref().and_then(load_image);
    match &bg {
        Some(img) => draw_bg(surface.canvas(), img, crop, wf, hf),
        None => draw_synthetic(surface.canvas(), wf, hf),
    }

    // In-game the blurred backdrop already exists as the cached frost surface,
    // so sampling it costs nothing extra there.
    let blurred: Vec<Image> = blur_sigmas
        .iter()
        .map(|s| blur_snapshot(&mut surface, 0.5, *s))
        .collect();

    let fonts = FontStore::new();
    let plate_w = 440.0;
    let plate_h = 124.0;
    let gap_x = 24.0;
    let left = (wf - (plate_w * 2.0 + gap_x)) * 0.5;

    for (i, v) in variants.iter().enumerate() {
        let (col, row) = (i % 2, i / 2);
        let x = left + col as f32 * (plate_w + gap_x);
        let y = 48.0 + row as f32 * 268.0;
        let bounds = Rect::from_xywh(x, y, plate_w, plate_h);

        // Caption above each plate.
        let cap = fonts.jetbrains_mono(11.0);
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.8), None);
        halo.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0, false));
        surface.canvas().draw_str(&v.label, (x, y - 14.0), &cap, &halo);
        let mut cp = Paint::default();
        cp.set_anti_alias(true);
        cp.set_color4f(rgba(PEARL, 0.95), None);
        surface.canvas().draw_str(&v.label, (x, y - 14.0), &cap, &cp);

        match &v.kind {
            Kind::Current => draw_iw_shell(surface.canvas(), bounds, 14.0),
            Kind::Glass(p) => {
                let ok = liquid_glass::draw_liquid_glass(
                    surface.canvas(),
                    bounds,
                    liquid_glass::Backdrop {
                        rim: &blurred[0],
                        rim_scale: 0.5,
                        frost: &blurred[1],
                        frost_scale: 0.5,
                    },
                    *p,
                    time,
                );
                if !ok {
                    eprintln!("liquid glass shader unavailable — drew nothing");
                }
            }
        }

        draw_media_content(surface.canvas(), bounds, &fonts);
    }

    let png = surface
        .image_snapshot()
        .encode(None, EncodedImageFormat::PNG, 100)
        .expect("encode");
    std::fs::write(&out, png.as_bytes()).expect("write");
    println!("wrote {out}");
}

/// Downscale + blur the current surface contents, returning the snapshot the
/// glass shader samples. `scale` is the fraction of full resolution.
fn blur_snapshot(surface: &mut skia_safe::Surface, scale: f32, sigma: f32) -> Image {
    let src = surface.image_snapshot();
    let (bw, bh) = (
        ((surface.width() as f32 * scale) as i32).max(1),
        ((surface.height() as f32 * scale) as i32).max(1),
    );
    let mut small = surfaces::raster_n32_premul((bw, bh)).expect("blur surface");
    let mut p = Paint::default();
    p.set_image_filter(image_filters::blur((sigma, sigma), TileMode::Clamp, None, None));
    small.canvas().draw_image_rect(
        &src,
        None,
        Rect::from_wh(bw as f32, bh as f32),
        &p,
    );
    small.image_snapshot()
}

/// Faithful reproduction of `ewo-jni::hud::draw_iw_shell` — the six-layer
/// chrome stack, same alphas, so the comparison is honest.
fn draw_iw_shell(canvas: &Canvas, rect: Rect, radius: f32) {
    // (1) Drop shadow.
    let shadow_rr = RRect::new_rect_xy(rect.with_offset((0.0, 6.0)), radius, radius);
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 0.55), None);
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 8.0, false));
    canvas.draw_rrect(shadow_rr, &shadow);

    // (2) Outer dark wine ring.
    let outer = Rect::new(rect.left - 1.0, rect.top - 1.0, rect.right + 1.0, rect.bottom + 1.0);
    let mut outer_paint = Paint::default();
    outer_paint.set_anti_alias(true);
    outer_paint.set_style(PaintStyle::Stroke);
    outer_paint.set_stroke_width(1.0);
    outer_paint.set_color4f(rgba(WINE, 0.55), None);
    canvas.draw_rrect(RRect::new_rect_xy(outer, radius + 1.0, radius + 1.0), &outer_paint);

    let rrect = RRect::new_rect_xy(rect, radius, radius);

    // (3) Translucent wine fill — the 0.50 that replaced the CSS 0.32.
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(rgba(WINE, 0.50), None);
    canvas.draw_rrect(rrect, &fill);

    // (4) Inset wine ring.
    let inset = Rect::new(rect.left + 1.0, rect.top + 1.0, rect.right - 1.0, rect.bottom - 1.0);
    let ir = (radius - 1.0).max(0.0);
    let mut wine_paint = Paint::default();
    wine_paint.set_anti_alias(true);
    wine_paint.set_style(PaintStyle::Stroke);
    wine_paint.set_stroke_width(1.0);
    wine_paint.set_color4f(rgba(WINE, 0.25), None);
    canvas.draw_rrect(RRect::new_rect_xy(inset, ir, ir), &wine_paint);

    // (5) Inset top pearl highlight, clipped to a 2px strip.
    canvas.save();
    canvas.clip_rect(
        Rect::from_xywh(rect.left, rect.top, rect.width(), 2.0),
        None,
        Some(true),
    );
    let top = Rect::new(rect.left + 0.5, rect.top + 0.5, rect.right - 0.5, rect.bottom - 0.5);
    let tr = (radius - 0.5).max(0.0);
    let mut top_paint = Paint::default();
    top_paint.set_anti_alias(true);
    top_paint.set_style(PaintStyle::Stroke);
    top_paint.set_stroke_width(1.0);
    top_paint.set_color4f(rgba(PEARL, 0.20), None);
    canvas.draw_rrect(RRect::new_rect_xy(top, tr, tr), &top_paint);
    canvas.restore();

    // (6) Pearl border.
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color4f(rgba(ROSE, 0.16), None);
    canvas.draw_rrect(rrect, &border);
}

/// A sketch of the media widget's contents — identical on every plate, so the
/// only variable is the plate itself.
fn draw_media_content(canvas: &Canvas, b: Rect, fonts: &FontStore) {
    let pad = 16.0;
    let art = Rect::from_xywh(b.left + pad, b.top + pad, 64.0, 64.0);
    let mut art_paint = Paint::default();
    art_paint.set_anti_alias(true);
    art_paint.set_shader(
        gradient_shader::linear(
            (Point::new(art.left, art.top), Point::new(art.right, art.bottom)),
            &[Color4f::new(0.36, 0.28, 0.42, 1.0), Color4f::new(0.62, 0.40, 0.48, 1.0)][..],
            None,
            TileMode::Clamp,
            None,
            None,
        ),
    );
    canvas.draw_rrect(RRect::new_rect_xy(art, 8.0, 8.0), &art_paint);

    let tx = art.right + 14.0;
    let title = fonts.newsreader(19.0);
    let mut tp = Paint::default();
    tp.set_anti_alias(true);
    tp.set_color4f(rgba(PEARL, 1.0), None);
    canvas.draw_str("Sofia", (tx, b.top + pad + 20.0), &title, &tp);

    let sub = fonts.newsreader(13.0);
    let mut sp = Paint::default();
    sp.set_anti_alias(true);
    sp.set_color4f(rgba(MAUVE, 1.0), None);
    canvas.draw_str("Clairo", (tx, b.top + pad + 40.0), &sub, &sp);

    let mono = fonts.jetbrains_mono(11.0);
    let mut mp = Paint::default();
    mp.set_anti_alias(true);
    mp.set_color4f(rgba(MAUVE, 1.0), None);
    canvas.draw_str("0:10 / 3:08", (b.right - 92.0, b.top + pad + 20.0), &mono, &mp);

    // Progress track + fill.
    let track = Rect::from_xywh(tx, b.top + pad + 54.0, b.right - tx - pad, 4.0);
    let mut tr_paint = Paint::default();
    tr_paint.set_anti_alias(true);
    tr_paint.set_color4f(rgba(PEARL, 0.22), None);
    canvas.draw_rrect(RRect::new_rect_xy(track, 2.0, 2.0), &tr_paint);
    let mut fill = track;
    fill.right = fill.left + track.width() * 0.06;
    let mut fp = Paint::default();
    fp.set_anti_alias(true);
    fp.set_color4f(rgba(PEARL, 0.95), None);
    canvas.draw_rrect(RRect::new_rect_xy(fill, 2.0, 2.0), &fp);

    // Transport glyphs, drawn as simple triangles/bars.
    let cy = b.bottom - 26.0;
    let mut gp = Paint::default();
    gp.set_anti_alias(true);
    gp.set_color4f(rgba(PEARL, 0.9), None);
    for (i, cx) in [tx + 40.0, tx + 78.0, tx + 116.0].into_iter().enumerate() {
        if i == 1 {
            canvas.draw_circle((cx, cy), 11.0, &gp);
        } else {
            canvas.draw_rect(Rect::from_xywh(cx - 6.0, cy - 6.0, 3.0, 12.0), &gp);
            canvas.draw_rect(Rect::from_xywh(cx - 1.0, cy - 6.0, 8.0, 12.0), &gp);
        }
    }
    let _ = (Font::default(), Data::new_empty());
}

fn draw_bg(canvas: &Canvas, img: &Image, crop: Option<Rect>, w: f32, h: f32) {
    let src = crop.unwrap_or_else(|| Rect::from_wh(img.width() as f32, img.height() as f32));
    let scale = (w / src.width()).max(h / src.height());
    let (dw, dh) = (src.width() * scale, src.height() * scale);
    let dst = Rect::from_xywh((w - dw) * 0.5, (h - dh) * 0.5, dw, dh);
    canvas.draw_image_rect(img, Some((&src, SrcRectConstraint::Fast)), dst, &Paint::default());
}

/// Fallback background with strong light/dark contrast, so the plate is judged
/// over both extremes at once.
fn draw_synthetic(canvas: &Canvas, w: f32, h: f32) {
    canvas.clear(Color::from_argb(0xFF, 0x2A, 0x3A, 0x22));
    let tile = 18.0;
    let mut p = Paint::default();
    p.set_anti_alias(false);
    let cols = (w / tile).ceil() as i32;
    let rows = (h / tile).ceil() as i32;
    for ry in 0..rows {
        for rx in 0..cols {
            let mut n = (rx as u32)
                .wrapping_mul(374_761_393)
                .wrapping_add((ry as u32).wrapping_mul(668_265_263));
            n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
            let j = ((n ^ (n >> 16)) & 0xFFFF) as f32 / 65_535.0;
            // Diagonal sweep from bright to dark across the frame.
            let sweep = ((rx as f32 / cols as f32) * 0.6 + (ry as f32 / rows as f32) * 0.4).clamp(0.0, 1.0);
            let k = (1.15 - sweep) * (0.85 + j * 0.3);
            p.set_color4f(Color4f::new(0.30 * k, 0.52 * k, 0.26 * k, 1.0), None);
            canvas.draw_rect(Rect::from_xywh(rx as f32 * tile, ry as f32 * tile, tile, tile), &p);
        }
    }
}

fn load_image(path: &str) -> Option<Image> {
    let bytes = std::fs::read(path).ok()?;
    Image::from_encoded(Data::new_copy(&bytes))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn parse_crop(s: &str) -> Option<Rect> {
    let parts: Vec<f32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if parts.len() != 4 {
        return None;
    }
    Some(Rect::from_xywh(parts[0], parts[1], parts[2], parts[3]))
}
