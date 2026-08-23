//! `rewo optionshot --check` — the options screens + volume sliders gate (M173).
//!
//! Drives the PRODUCTION chain (the M45/M86 rule):
//!
//! ```text
//! Options -> live_cmd::{sound_rows, root_rows}    (the pages' rows)
//!         -> options_screen::build                (widgets on OptionsList geometry)
//!         -> live_cmd::{screen_chrome, slider_sprites, screen_text_lines}
//!         -> WorldRenderer::{set_screen, set_text} -> ScreenPass / TextPass
//!         -> Offscreen::read_rgba                  (real pixels)
//! ```
//!
//! Pixel predictions come from the bake's own slider sheets, probed
//! texel-for-texel at GUI scale 2 — a wrong sheet, a wrong handle position or
//! a dropped lowering all fail on bytes.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::options::Options;
use rewo_net::sounds::SoundSource;
use rewo_world::options_screen as os;

/// Total named properties. Locked so a skipped one fails the run.
const EXPECTED_WITNESSES: usize = 12;

const W: u32 = 640;
const H: u32 = 480;
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;
/// The list's content top the app passes (`FOOTER_HEIGHT + 3`).
const HEADER: i32 = os::FOOTER_HEIGHT + 3;

#[derive(ClapArgs)]
pub struct OptionshotArgs {
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Write the rendered frames here for eyeballing. Never read back.
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[optionshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

pub fn run(args: OptionshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = rewo_data::DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_pixels(&mut c, &args, &baked)?;
    println!(
        "[optionshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    if c.witnessed + c.failures.len() != EXPECTED_WITNESSES {
        return Err(format!(
            "optionshot: {} witnesses ran, {EXPECTED_WITNESSES} declared",
            c.witnessed + c.failures.len()
        ));
    }
    if c.failures.is_empty() {
        println!("[optionshot] PASS — {EXPECTED_WITNESSES} witnesses");
        Ok(())
    } else {
        Err(format!("optionshot: {} witnesses failed", c.failures.len()))
    }
}

fn check_pixels(
    c: &mut Checker,
    args: &OptionshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[optionshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("optionshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.set_sky_mode(SkyMode::None);
    let sprites =
        crate::live_cmd::widget_sprites(baked).ok_or("widget sprites missing from the jar")?;
    wr.init_screen(&mut gpu, &sprites)?;
    let font = crate::live_cmd::font_data(baked).ok_or("no baked font")?;
    wr.init_text(&mut gpu, &font)?;
    let advance = baked.font.as_ref().ok_or("no baked font")?.advance;
    let widgets = baked.widgets.as_ref().ok_or("no widget sprites")?;
    let lang = &baked.lang;

    c.record(
        "o0.the_gui_scale_is_the_one_the_predictions_assume",
        rewo_gpu::hud::gui_scale(W as f32, H as f32) == SCALE as f32,
        format!("{W}x{H} -> scale {SCALE}"),
    );

    let ring = crate::stats::OverlayRing::default();
    let overlay_draw = rewo_gpu::overlay::OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    let clear = [0.0, 1.0, 0.0, 1.0];

    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    opts: Option<&Options>,
                    mouse: Option<(f64, f64)>,
                    dragging: Option<rewo_world::screen::WidgetId>,
                    dump: &str|
     -> Result<Vec<u8>, String> {
        let mut chrome = rewo_gpu::screen::ScreenDraw::default();
        let mut lines = Vec::new();
        if let Some(o) = opts {
            let rows = crate::live_cmd::sound_rows(o, lang);
            let screen = os::build(os::OptionsPage::Sound, &rows, GUI_W, GUI_H, HEADER);
            chrome = crate::live_cmd::screen_chrome(&screen, mouse);
            chrome
                .sprites
                .extend(crate::live_cmd::slider_sprites(&screen, mouse, dragging));
            lines = crate::live_cmd::screen_text_lines(&screen, &advance, SCALE as f32);
        }
        wr.set_screen(chrome);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        if let Some(dir) = &args.out_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = off.save_png(gpu, &dir.join(format!("options-{dump}.png")));
        }
        off.read_rgba(gpu)
    };

    let px = |f: &[u8], x: i32, y: i32| -> [u8; 3] {
        let i = ((y as u32 * W + x as u32) * 4) as usize;
        [f[i], f[i + 1], f[i + 2]]
    };
    let texel = |s: &rewo_data::assets::HudSprite, dx: u32, dy: u32| -> [u8; 4] {
        let i = ((dy * s.w + dx) * 4) as usize;
        [s.rgba[i], s.rgba[i + 1], s.rgba[i + 2], s.rgba[i + 3]]
    };
    let close = |a: [u8; 3], b: [u8; 3], tol: i32| -> bool {
        a.iter()
            .zip(b.iter())
            .all(|(x, y)| (*x as i32 - *y as i32).abs() <= tol)
    };

    // A distinct volume per witness need: master 0.5, music 0.0 (OFF), ui 1.0.
    let mut opts = Options::default();
    opts.set_sound_volume(SoundSource::Master, 0.5);
    opts.set_sound_volume(SoundSource::Music, 0.0);

    let empty = shot(&mut gpu, &mut off, &mut wr, None, None, None, "empty")?;
    let frame = shot(&mut gpu, &mut off, &mut wr, Some(&opts), None, None, "sound")?;

    // Geometry: the master slider is row 0, addBig — x = gw/2 - 155, 310 wide.
    let mx = GUI_W / 2 - os::BAND_HALF;
    let my = HEADER;

    // o1 — the empty frame is clear at the master row.
    c.record(
        "o1.no_screen_draws_nothing",
        px(&empty, (mx + 100) * SCALE, (my + 10) * SCALE) == [0, 255, 0],
        format!("{:?}", px(&empty, (mx + 100) * SCALE, (my + 10) * SCALE)),
    );

    // o2 — the master TRACK is drawn 310 wide: its interior is the track
    // sheet's own interior texel (nine-slice border 1 stretches the middle),
    // probed near the left edge, the middle and the right edge.
    let track = &widgets.slider[0];
    let t_mid = texel(track, 100, 10);
    let mut all = true;
    for dx in [3, 150, 306] {
        let f = px(&frame, (mx + dx) * SCALE, (my + 10) * SCALE);
        if !close([t_mid[0], t_mid[1], t_mid[2]], f, 2) {
            // The handle covers value*302 = 151..159 — skip probes under it.
            if !(151..160).contains(&dx) {
                all = false;
            }
        }
    }
    c.record(
        "o2.the_master_track_spans_the_310_band",
        all,
        format!("track interior texel {t_mid:?} at dx 3/150(skip-if-handle)/306"),
    );

    // o3 — the HANDLE sits at x + (int)(0.5 * 302) = x + 151, 8 wide: the
    // handle sheet's interior texel is there, and NOT at the value-1.0 spot.
    let handle = &widgets.slider[2];
    let h_mid = texel(handle, 4, 10);
    let at_half = px(&frame, (mx + 151 + 4) * SCALE, (my + 10) * SCALE);
    let at_full = px(&frame, (mx + 302 + 4) * SCALE, (my + 10) * SCALE);
    c.record(
        "o3.the_handle_is_at_value_times_width_minus_8",
        close([h_mid[0], h_mid[1], h_mid[2]], at_half, 2)
            && !close([h_mid[0], h_mid[1], h_mid[2]], at_full, 2),
        format!("handle texel {h_mid:?}, at 0.5 {at_half:?}, at 1.0 spot {at_full:?}"),
    );

    // o4 — hover highlights the HANDLE (hovered-or-engaged), not the track.
    let hover = ((mx + 151 + 4) as f64, (my + 10) as f64);
    let hovered = shot(&mut gpu, &mut off, &mut wr, Some(&opts), Some(hover), None, "hover")?;
    let hh_mid = texel(&widgets.slider[3], 4, 10);
    c.record(
        "o4.hover_draws_the_highlighted_handle",
        close([hh_mid[0], hh_mid[1], hh_mid[2]], px(&hovered, (mx + 151 + 4) * SCALE, (my + 10) * SCALE), 2)
            && !close([hh_mid[0], hh_mid[1], hh_mid[2]], at_half, 2),
        format!("highlighted texel {hh_mid:?}"),
    );

    // o5 — DRAGGING highlights it too, even with the cursor off the widget
    // (`canChangeValue` — vanilla keeps the engaged handle lit mid-drag).
    let dragged = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        Some(&opts),
        None,
        Some(os::widget_id(0, 0)),
        "drag",
    )?;
    c.record(
        "o5.a_dragged_handle_stays_highlighted",
        close(
            [hh_mid[0], hh_mid[1], hh_mid[2]],
            px(&dragged, (mx + 151 + 4) * SCALE, (my + 10) * SCALE),
            2,
        ),
        "the engaged handle is lit with the cursor anywhere",
    );

    // o6 — the category pairs sit at the 160 pitch: MUSIC's track at column
    // 0 and RECORDS' at x + 160, row 1.
    // Probes at dx 12 — past the 8-wide handle a value-0 slider parks at the
    // LEFT edge (music is 0.0 in this fixture; the first probe at dx 3 read
    // the handle), and short of the centred label's glyphs (the probe before
    // that, at the centre, read a white glyph of the records label).
    // …and at dy 2, ABOVE the label band (glyphs run dy 6..15, and the
    // records label — "Jukebox/Note Blocks: 100%" — is wide enough that no dx
    // at glyph height is clear of it).
    let ry = HEADER + os::ROW_HEIGHT;
    let t2 = texel(track, 100, 2);
    let music_track = px(&frame, (mx + 12) * SCALE, (ry + 2) * SCALE);
    let records_track = px(&frame, (mx + os::COLUMN_PITCH + 12) * SCALE, (ry + 2) * SCALE);
    c.record(
        "o6.the_pairs_sit_at_the_160_pitch",
        // tol 4: NEAREST sampling in the track's vertical gradient lands a
        // half-texel off ([44] against the declared [41]); the failure modes
        // this witness exists for — a label glyph (255) or a parked handle
        // (112) — are dozens of bytes away.
        close([t2[0], t2[1], t2[2]], music_track, 4)
            && close([t2[0], t2[1], t2[2]], records_track, 4),
        format!("music {music_track:?}, records {records_track:?} (want dy-2 track texel {t2:?})"),
    );

    // o7 — labels: white glyphs on the master slider ("Master Volume: 50%").
    let mut label_px = 0;
    for gy in my + 4..my + 16 {
        for gx in mx..mx + os::BAND_WIDTH {
            let p = px(&frame, gx * SCALE, gy * SCALE);
            if p[0] > 220 && p[1] > 220 && p[2] > 220 {
                label_px += 1;
            }
        }
    }
    c.record(
        "o7.the_master_slider_is_labeled",
        label_px > 30,
        format!("{label_px} white glyph pixels"),
    );

    // o8 — a volume of EXACTLY 0.0 labels OFF: the music slider's label is
    // shorter than a percent one, measured by the model (the pixels already
    // prove labels draw; the OFF rule is the model's own).
    let off_label = os::percent_label("Music", 0.0, "OFF");
    c.record(
        "o8.exact_zero_labels_off",
        off_label == "Music: OFF" && os::percent_label("Music", 0.004, "OFF") == "Music: 0%",
        format!("{off_label:?} / 0.004 -> 0%"),
    );

    // o9 — the sound page's row order carries all eleven sliders + the
    // frequency button + Done: count the widgets the production rows build.
    let rows = crate::live_cmd::sound_rows(&opts, lang);
    let screen = os::build(os::OptionsPage::Sound, &rows, GUI_W, GUI_H, HEADER);
    let sliders = screen
        .widgets
        .iter()
        .filter(|w| matches!(w.kind, rewo_world::screen::WidgetKind::Slider { .. }))
        .count();
    c.record(
        "o9.eleven_sliders_one_frequency_button_one_done",
        sliders == 11 && screen.widgets.len() == 13,
        format!("{sliders} sliders in {} widgets", screen.widgets.len()),
    );

    // o10 — the Done chrome draws in the footer band.
    let dy = GUI_H - os::FOOTER_HEIGHT + (os::FOOTER_HEIGHT - 20) / 2;
    let mut diff = 0;
    for gy in dy..dy + 20 {
        for gx in (GUI_W - 200) / 2..(GUI_W + 200) / 2 {
            if px(&frame, gx * SCALE, gy * SCALE) != px(&empty, gx * SCALE, gy * SCALE) {
                diff += 1;
            }
        }
    }
    c.record(
        "o10.the_done_button_draws_in_the_footer",
        diff > 400,
        format!("{diff} changed pixels"),
    );

    // o11 — the slider MOUSE MATH through the production framework: a click
    // at the track's 3/4 point yields 0.75 and moves the widget's own value.
    let mut sc = os::build(
        os::OptionsPage::Sound,
        &crate::live_cmd::sound_rows(&opts, lang),
        GUI_W,
        GUI_H,
        HEADER,
    );
    let press_x = (mx + 4) as f64 + 0.75 * 302.0;
    let r = sc.mouse_clicked(press_x, (my + 10) as f64, 0);
    let ok = matches!(
        r,
        rewo_world::screen::MouseResult::Slider(id, v)
            if id == os::widget_id(0, 0) && (v - 0.75).abs() < 0.01
    );
    c.record(
        "o11.a_press_computes_the_value_from_the_mouse",
        ok,
        format!("(mx - (x + 4)) / (width - 8) -> {r:?}"),
    );

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}
