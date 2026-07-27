//! Offscreen render of the in-game HUD + overlay to a PNG.
//!
//! The in-game UI could previously only be looked at by launching Minecraft,
//! which made every visual change a manual round-trip. `hud::draw` is a pure
//! function over a Skia `Canvas` though, so nothing about it actually needs a
//! JVM, a GL context, or a game — only a data block ([`ewo_jni::fixture`]) and
//! a surface. This is the in-game counterpart to `ewo-render`'s `dropdown_shot`.
//!
//! ```text
//! cargo run -p ewo-jni --example hudshot -- --all
//! cargo run -p ewo-jni --example hudshot -- --view modules --out modules.png
//! cargo run -p ewo-jni --example hudshot -- --view widgets --scene combat --bg shot.png
//! ```
//!
//! `--bg <png>` composites over a real Minecraft screenshot; without it a
//! synthetic scene stands in. Either way the point is the same — the HUD is
//! painted over *game pixels*, and judging it on a flat black field flatters
//! it in a way the real thing never gets.
//!
//! Widget content is read from the current working directory (`overlay-mods.toml`,
//! `ewo-skin.png`, `ewo-keybinds.txt`, the active profile's config), exactly as
//! in-game — so running this from an instance directory renders real state.

use ewo_jni::fixture::{self, Scene};
use ewo_jni::hud::{self, Editor, HudData};
use ewo_render::FontStore;
use skia_safe::{
    gradient_shader, image_filters, surfaces, BlendMode, ClipOp, Color,
    Color4f, CubicResampler, Data, EncodedImageFormat, FilterMode, Image, Paint, Point, RRect,
    Rect, TileMode,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    let view = flag(&args, "--view").unwrap_or_else(|| "home".to_string());
    let scene_name = flag(&args, "--scene").unwrap_or_else(|| "combat".to_string());
    let out_dir = flag(&args, "--out-dir").unwrap_or_else(|| "hudshots".to_string());
    let bg_path = flag(&args, "--bg");
    let all = args.iter().any(|a| a == "--all");
    // The HUD animates now, so a shot is a shot *at a time*. Fixed default so
    // two runs of the harness are byte-identical and a PNG diff means a real
    // change rather than a different phase of the ripple.
    let time: f32 = flag(&args, "--time").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let glass_strength: f32 = flag(&args, "--glass")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);

    let Some(scene) = Scene::parse(&scene_name) else {
        eprintln!("unknown --scene '{scene_name}' (combat | explore | menu)");
        std::process::exit(2);
    };

    let (w, h) = match flag(&args, "--size") {
        Some(s) => parse_size(&s).unwrap_or_else(|| {
            eprintln!("--size wants WxH, e.g. 1920x1080");
            std::process::exit(2);
        }),
        // Matches the user's display (2560x1441 in the reference screenshots)
        // halved — big enough that nothing is layout-clipped, small enough to
        // read at a glance.
        None => (1280, 720),
    };

    let background = bg_path.as_deref().and_then(load_background);
    if bg_path.is_some() && background.is_none() {
        eprintln!("could not decode --bg image; falling back to the synthetic scene");
    }

    let fonts = FontStore::new();

    // `Editor::new` reads the persisted layout, mods list, modules, crosshair,
    // and profile from disk (relative to cwd) — same as in-game.
    let mut editor = Editor::new();

    let targets: Vec<String> = if all {
        let mut t = vec!["widgets".to_string()];
        t.extend(Editor::view_names().into_iter().map(|s| s.to_lowercase()));
        t
    } else {
        vec![view]
    };

    if all {
        std::fs::create_dir_all(&out_dir).expect("create out dir");
    }

    for target in &targets {
        let overlay_open = target != "widgets";
        if overlay_open && !editor.set_view_by_name(target) {
            eprintln!(
                "unknown --view '{target}' (widgets | {})",
                Editor::view_names().join(" | ").to_lowercase()
            );
            std::process::exit(2);
        }

        let path = if all {
            format!("{out_dir}/{}-{}.png", scene.name(), target)
        } else {
            flag(&args, "--out").unwrap_or_else(|| format!("hud-{target}.png"))
        };

        render(
            &path,
            w,
            h,
            scene,
            overlay_open,
            background.as_ref(),
            &mut editor,
            &fonts,
            time,
            glass_strength,
        );
        println!("wrote {path}");
    }
}

#[allow(clippy::too_many_arguments)]
fn render(
    path: &str,
    w: i32,
    h: i32,
    scene: Scene,
    overlay_open: bool,
    background: Option<&Image>,
    editor: &mut Editor,
    fonts: &FontStore,
    time: f32,
    glass_strength: f32,
) {
    let mut surface = surfaces::raster_n32_premul((w, h)).expect("raster surface");
    let canvas = surface.canvas();
    let (wf, hf) = (w as f32, h as f32);

    match background {
        Some(img) => draw_background_image(canvas, img, wf, hf),
        None => draw_synthetic_scene(canvas, wf, hf),
    }

    // The CROSSHAIR editor's preview panes are *cutouts*: `hud::draw` leaves
    // them transparent and the composite step fills them with un-frosted live
    // game, so the crosshair is judged against the real world at 1:1. Keep a
    // pre-frost snapshot to play that role below.
    let unfrosted = surface.image_snapshot();

    // The glass sources, taken from the *unfrosted* world — matching where the
    // in-game capture happens (before the frost, so the refracting rim still
    // has structure to bend).
    let glass = glass_sources(&mut surface);

    // In-game the frosted backdrop is applied by the JNI *composite* step, not
    // by `hud::draw` — so a harness that only calls `draw` would render the
    // overlay's glass panels against a razor-sharp game and make them look far
    // more transparent than they ever are in practice. Reproduce it here.
    if overlay_open && editor.frosts_game() {
        frost_backdrop(&mut surface);
    }

    // The data block must outlive the `HudData` view over it.
    let block = fixture::block(scene, overlay_open, wf, hf);
    // SAFETY: `block` is `BLOCK_BYTES` long — exactly the span the reader
    // touches — and outlives `data` (it is dropped at the end of this fn).
    let data = unsafe { HudData::new(block.as_ptr()) };

    // Park the cursor off-screen so nothing renders a hover state by accident;
    // a hover shot is a deliberate `--cursor` follow-up, not a default.
    editor.set_cursor(-1000.0, -1000.0);

    let canvas = surface.canvas();

    hud::draw(
        canvas,
        &data,
        editor,
        fonts,
        wf,
        hf,
        hud::Frame { time, glass: Some(glass), glass_strength },
    );

    // Backfill the cutouts. In-game the un-frosted game is painted *under* the
    // HUD layer; here there is only one surface, so `DstOver` achieves the same
    // thing — it writes only where `draw` left transparency.
    if overlay_open {
        let cutouts = editor.live_game_cutouts(wf, hf);
        if !cutouts.is_empty() {
            let mut under = Paint::default();
            under.set_blend_mode(BlendMode::DstOver);
            let canvas = surface.canvas();
            for rect in &cutouts {
                let saved = canvas.save();
                canvas.clip_rrect(
                    RRect::new_rect_xy(*rect, 10.0, 10.0),
                    Some(ClipOp::Intersect),
                    Some(true),
                );
                canvas.draw_image_rect(&unfrosted, None, Rect::from_wh(wf, hf), &under);
                canvas.restore_to_count(saved);
            }
        }
    }

    let png = surface
        .image_snapshot()
        .encode(None, EncodedImageFormat::PNG, 100)
        .expect("encode png");
    std::fs::write(path, png.as_bytes()).expect("write png");
}

/// Snapshot the current surface into the two blur levels liquid glass wants.
///
/// In-game these come from a GL copy of the live framebuffer plus the frost
/// cache the overlay already maintains; offscreen the surface *is* the game, so
/// a direct snapshot is both simpler and more faithful.
fn glass_sources(surface: &mut skia_safe::Surface) -> hud::GlassSource {
    hud::GlassSource {
        rim: downscale_blur(surface, 0.5, 2.0),
        rim_scale: 0.5,
        frost: downscale_blur(surface, 0.25, 4.0),
        frost_scale: 0.25,
    }
}

fn downscale_blur(surface: &mut skia_safe::Surface, scale: f32, sigma: f32) -> Image {
    let src = surface.image_snapshot();
    let (w, h) = (
        ((surface.width() as f32 * scale) as i32).max(1),
        ((surface.height() as f32 * scale) as i32).max(1),
    );
    let mut small = surfaces::raster_n32_premul((w, h)).expect("glass source surface");
    let mut paint = Paint::default();
    paint.set_image_filter(image_filters::blur((sigma, sigma), TileMode::Clamp, None, None));
    small
        .canvas()
        .draw_image_rect(&src, None, Rect::from_wh(w as f32, h as f32), &paint);
    small.image_snapshot()
}

/// Reproduce `Hud::refresh_frost` + the frost half of `Hud::composite`.
///
/// Deliberately step-for-step identical to the real thing: a clean two-step 2×
/// downscale (each linear step averages an exact 2×2 block, so it never
/// aliases), a small gaussian on the quarter-res result, a cubic upscale back
/// to full size, then the faint Velvet wine wash. Any drift here would make
/// every judgement about panel contrast wrong, so it is worth the duplication.
fn frost_backdrop(surface: &mut skia_safe::Surface) {
    let (w, h) = (surface.width(), surface.height());
    let game = surface.image_snapshot();

    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    let mut half = surfaces::raster_n32_premul((hw, hh)).expect("half surface");
    half.canvas().draw_image_rect_with_sampling_options(
        &game,
        None,
        Rect::from_wh(hw as f32, hh as f32),
        FilterMode::Linear,
        &Paint::default(),
    );
    let half_img = half.image_snapshot();

    let (qw, qh) = ((w / 4).max(1), (h / 4).max(1));
    let mut quarter = surfaces::raster_n32_premul((qw, qh)).expect("quarter surface");
    let mut blur = Paint::default();
    blur.set_image_filter(image_filters::blur((3.0, 3.0), TileMode::Clamp, None, None));
    quarter.canvas().draw_image_rect_with_sampling_options(
        &half_img,
        None,
        Rect::from_wh(qw as f32, qh as f32),
        FilterMode::Linear,
        &blur,
    );

    let blurred = quarter.image_snapshot();
    let dst = Rect::from_wh(w as f32, h as f32);
    let canvas = surface.canvas();
    canvas.draw_image_rect_with_sampling_options(
        &blurred,
        None,
        dst,
        CubicResampler::mitchell(),
        &Paint::default(),
    );
    let mut tint = Paint::default();
    tint.set_color(Color::from_argb(70, 10, 0, 6));
    canvas.draw_rect(dst, &tint);
}

fn load_background(path: &str) -> Option<Image> {
    let bytes = std::fs::read(path).ok()?;
    Image::from_encoded(Data::new_copy(&bytes))
}

/// Draw `img` scaled to *cover* the frame (aspect preserved, overflow cropped),
/// so a 16:9 screenshot fills a 16:9 frame exactly and an odd one is centred
/// rather than stretched.
fn draw_background_image(canvas: &skia_safe::Canvas, img: &Image, w: f32, h: f32) {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let scale = (w / iw).max(h / ih);
    let (dw, dh) = (iw * scale, ih * scale);
    let dst = Rect::from_xywh((w - dw) * 0.5, (h - dh) * 0.5, dw, dh);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    canvas.draw_image_rect(img, None, dst, &paint);
}

/// A stand-in Minecraft scene.
///
/// Not trying to be pretty — trying to be *hostile* in the ways the real game
/// is. The HUD has to stay legible over a bright sky, a dark treeline, and a
/// mid-tone dappled ground, so the frame carries all three with per-tile
/// luminance jitter rather than flat fills.
fn draw_synthetic_scene(canvas: &skia_safe::Canvas, w: f32, h: f32) {
    let horizon = h * 0.46;

    // Sky — bright at the top where top-anchored widgets land.
    let sky = gradient_shader::linear(
        (Point::new(0.0, 0.0), Point::new(0.0, horizon)),
        &[
            Color4f::new(0.44, 0.63, 0.89, 1.0),
            Color4f::new(0.62, 0.78, 0.95, 1.0),
        ][..],
        None,
        TileMode::Clamp,
        None,
        None,
    );
    let mut paint = Paint::default();
    paint.set_shader(sky);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, horizon), &paint);

    // Sun glare — a genuinely blown-out region. Any HUD text that only works
    // on dark pixels will fail here, which is the point.
    let glare = gradient_shader::radial(
        Point::new(w * 0.74, h * 0.13),
        h * 0.30,
        &[
            Color4f::new(1.0, 0.99, 0.92, 1.0),
            Color4f::new(1.0, 0.99, 0.92, 0.0),
        ][..],
        None,
        TileMode::Clamp,
        None,
        None,
    );
    let mut gp = Paint::default();
    gp.set_shader(glare);
    canvas.draw_rect(Rect::from_xywh(0.0, 0.0, w, horizon), &gp);

    // Clouds.
    let mut cloud = Paint::default();
    cloud.set_anti_alias(false);
    cloud.set_color4f(Color4f::new(1.0, 1.0, 1.0, 0.72), None);
    for i in 0..7 {
        let x = ((i as f32) * 0.17 + 0.04) * w;
        let y = h * (0.05 + 0.035 * ((i % 3) as f32));
        canvas.draw_rect(Rect::from_xywh(x, y, w * 0.11, h * 0.022), &cloud);
    }

    // Terrain — 16 px blocks with hue/value jitter, dirt below a grass crust,
    // shading downward so the bottom of the frame is genuinely dark.
    let tile = 16.0;
    let cols = (w / tile).ceil() as i32;
    let rows = ((h - horizon) / tile).ceil() as i32;
    let mut block = Paint::default();
    block.set_anti_alias(false);
    for ry in 0..rows {
        for rx in 0..cols {
            let n = hash01(rx, ry);
            let depth = ry as f32 / rows.max(1) as f32;
            // Top three rows are grass; below that, dirt.
            let (base_r, base_g, base_b) = if ry < 3 {
                (0.29, 0.52, 0.24)
            } else {
                (0.42, 0.30, 0.20)
            };
            // Darken with depth (ambient occlusion-ish) and jitter per tile.
            let k = (1.0 - depth * 0.55) * (0.86 + n * 0.28);
            block.set_color4f(
                Color4f::new(base_r * k, base_g * k, base_b * k, 1.0),
                None,
            );
            canvas.draw_rect(
                Rect::from_xywh(rx as f32 * tile, horizon + ry as f32 * tile, tile, tile),
                &block,
            );
        }
    }

    // A water pool — a bright, saturated mid-frame region that competes with
    // rose/lavender HUD accents specifically.
    let mut water = Paint::default();
    water.set_anti_alias(false);
    for ry in 0..6 {
        for rx in 0..14 {
            let n = hash01(rx + 71, ry + 13);
            let k = 0.85 + n * 0.3;
            water.set_color4f(Color4f::new(0.16 * k, 0.34 * k, 0.78 * k, 1.0), None);
            canvas.draw_rect(
                Rect::from_xywh(
                    w * 0.55 + rx as f32 * tile,
                    horizon + (ry as f32 + 1.0) * tile,
                    tile,
                    tile,
                ),
                &water,
            );
        }
    }

    // Treeline silhouette straddling the horizon — a hard dark/light edge
    // right where centre-anchored widgets sit.
    let mut trunk = Paint::default();
    trunk.set_anti_alias(false);
    for i in 0..9 {
        let n = hash01(i, 5);
        let x = (i as f32 * 0.115 + 0.02) * w;
        let th = h * (0.08 + n * 0.09);
        trunk.set_color4f(Color4f::new(0.13, 0.24, 0.12, 1.0), None);
        canvas.draw_rect(Rect::from_xywh(x, horizon - th, w * 0.055, th), &trunk);
    }

    // Vanilla dims the world behind its own GUIs; without something similar
    // the comparison would be unfair to the real thing. Nothing here — the
    // overlay applies its own 22% scrim in `hud::draw`.
    let _ = Color::BLACK;
}

/// Cheap deterministic hash → `0.0..1.0`. Deterministic so two runs of the
/// harness produce byte-identical backgrounds and a PNG diff means a real
/// change in the HUD, not noise in the backdrop.
fn hash01(x: i32, y: i32) -> f32 {
    let mut n = (x as u32).wrapping_mul(374_761_393).wrapping_add((y as u32).wrapping_mul(668_265_263));
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) & 0xFFFF) as f32 / 65_535.0
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn parse_size(s: &str) -> Option<(i32, i32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

fn print_help() {
    println!(
        "\
hudshot — render the in-game HUD / overlay to a PNG, no Minecraft required.

  --view <name>     widgets | home | hud | crosshair | modules | pvp | mods | friends | settings
                    (\"widgets\" = overlay closed, in-world HUD only)   [default: home]
  --scene <name>    combat | explore | menu   [default: combat]
  --all             render every view for the scene into --out-dir
  --out <path>      output PNG for a single view   [default: hud-<view>.png]
  --out-dir <dir>   output directory for --all     [default: hudshots]
  --size <WxH>      framebuffer size               [default: 1280x720]
  --bg <png>        composite over a real screenshot instead of the synthetic scene

Widget content (mods list, skin, keybinds, profile) is read from the current
directory, exactly as in-game — run from an instance dir for real state."
    );
}
