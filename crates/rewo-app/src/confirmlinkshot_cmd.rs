//! `rewo confirmlinkshot --check` — the oracle for the link prompt
//! (`ConfirmLinkScreen`) a clicked chat `open_url` now goes through.
//!
//! The path under test:
//!
//! ```text
//! the jar's own en_us strings      (rewo_data lang, baked from the user's jar)
//!   -> rewo_ui::confirm_link_screen::{ConfirmLinkScreen::new, build, key_pressed}
//!   -> rewo_ui::layout                 (LinearLayout / FrameLayout)
//!   -> live_cmd::{screen_chrome, screen_text_lines}   (the builders the frame calls)
//!   -> WorldRenderer::{set_screen, set_text} -> ScreenPass / TextPass
//!   -> Offscreen::read_rgba            (real pixels)
//! ```
//!
//! **What this gate does not touch: the browser.** Nothing here spawns an
//! opener; `uri_open`'s own tests cover argv and the environment scrub. This
//! gate grades the screen (model and pixels) and the two options that decide
//! whether it appears.
//!
//! ## The rules, borrowed from `serverlinkshot` because they were each earned
//!
//! * **The detector must not share a colour with its subject.** Every pixel
//!   witness renders over a **pure magenta** clear (`255, 0, 255`). A button
//!   texel is greyscale (`r == g == b`), the title and message are white, the
//!   warning is `0xFFCCCC` (`r != b`, `g == b`), and its shadow is dark.
//!   Nothing the screen draws has `r == b` with `g == 0`, so the clear is
//!   unmistakable, and `p1` asserts a screenless frame is uniformly clear.
//! * **Expectations come from the decompile, not from the constants under
//!   test.** The stack height, the 8-px row gap and the 16-px button padding
//!   below are typed here from `ConfirmScreen.java` / `ConfirmLinkScreen.java`,
//!   and the text widths come from the jar's own font.
//! * **A control for every detector.** The warning-colour detector is run on
//!   the trusted variant too, where it must find nothing.
//! * **No witness may panic**; every lookup is `Option`-shaped, and the count
//!   is fail-closed against [`EXPECTED_WITNESSES`].

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::DataPaths;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_ui::confirm_link_screen as cl;
use rewo_ui::screen::{KeyResult, MouseResult, Screen, Widget};

/// Total named properties this gate asserts.
const EXPECTED_WITNESSES: usize = 14;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = 2 — asserted by `p0`, not assumed.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

#[derive(ClapArgs)]
pub struct ConfirmLinkshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally; the
    /// flag only labels the run, as on every other `*shot`.
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
            "[confirmlinkshot] {}  {name}: {detail}",
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

/// A URL that wraps to three rows at the 270-px message width (a URL has no
/// spaces, so the wrap is a hard break).
const URL: &str = "https://example.com/a/rather/long/path/that/keeps/going/and/going/\
                   until/it/is/long/enough/to/take/three/rows/on/screen/index.html";

pub fn run(args: ConfirmLinkshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[confirmlinkshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Model half against ConfirmScreen/ConfirmLinkScreen; \
         pixel half against a pure-magenta clear no part of the screen can produce."
    );
    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_model(&mut c, &baked)?;
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[confirmlinkshot] witnesses observed: {} / {}",
        c.witnessed, EXPECTED_WITNESSES
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "{} propert{} failed: {}",
            c.failures.len(),
            if c.failures.len() == 1 { "y" } else { "ies" },
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "witness count {} != expected {EXPECTED_WITNESSES} — a named \
             property was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[confirmlinkshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

fn untrusted(baked: &assets::BakedAssets, url: &str) -> cl::ConfirmLinkScreen {
    cl::ConfirmLinkScreen::new(&baked.lang, url, false)
}

fn rect(w: Option<&Widget>) -> (i32, i32, i32, i32) {
    w.map(|w| (w.x, w.y, w.width, w.height))
        .unwrap_or((-1, -1, 0, 0))
}

fn check_model(c: &mut Checker, baked: &assets::BakedAssets) -> Result<(), String> {
    let advance = baked.font.as_ref().ok_or("no baked font")?.advance;
    let width_of = |t: &str| rewo_gpu::text::width(t, &advance);
    let prompt = untrusted(baked, URL);
    let screen = cl::build(&prompt, GUI_W, GUI_H, &width_of);

    // m1 — the strings are the jar's own, for the path a chat click takes
    // (`trusted == false`): "Yes" / "Copy to Clipboard" / "No" and the
    // warning. Typed from en_us.json, not read back through the module's keys.
    let msg = |id| screen.widget(id).map(|w| w.message.clone());
    c.record(
        "m1.an_untrusted_link_shows_the_jars_own_captions_and_the_warning",
        msg(cl::TITLE).as_deref() == Some("Are you sure you want to open the following website?")
            && msg(cl::YES).as_deref() == Some("Yes")
            && msg(cl::COPY).as_deref() == Some("Copy to Clipboard")
            && msg(cl::NO).as_deref() == Some("No")
            && msg(cl::WARNING).as_deref()
                == Some("Never open links from people that you don't trust!"),
        format!(
            "{:?} / {:?} / {:?} / {:?}",
            msg(cl::TITLE),
            msg(cl::YES),
            msg(cl::COPY),
            msg(cl::NO)
        ),
    );

    // m2 — the vertical rhythm, from `ConfirmScreen`'s literals: spacing(8)
    // between rows, and the button cells' paddingTop(16) INSIDE the last row.
    // The whole stack is centred, so the title's y is predictable.
    let rows = match screen.widget(cl::MESSAGE).map(|w| &w.kind) {
        Some(rewo_ui::screen::WidgetKind::MultiLabel { lines, .. }) => lines.len() as i32,
        _ => -1,
    };
    let total = 9 + 8 + 9 * rows + 8 + 9 + 8 + 16 + 20;
    let top = (GUI_H - total) / 2;
    let (_, ty, _, _) = rect(screen.widget(cl::TITLE));
    let (_, my, _, _) = rect(screen.widget(cl::MESSAGE));
    let (_, wy, _, _) = rect(screen.widget(cl::WARNING));
    let (_, by, _, bh) = rect(screen.widget(cl::YES));
    c.record(
        "m2.the_stack_is_spaced_eight_apart_with_sixteen_above_the_buttons_and_centred",
        rows >= 2
            && ty == top
            && my == top + 9 + 8
            && wy == my + 9 * rows + 8
            && by == wy + 9 + 8 + 16
            && bh == 20,
        format!(
            "{rows} message rows, total {total}: title {ty} (want {top}), \
             message {my}, warning {wy}, buttons {by}"
        ),
    );

    // m3 — three 100-wide buttons, 4 apart, the 308-px row centred.
    let (yx, yy, yw, _) = rect(screen.widget(cl::YES));
    let (cx, cy, cw, _) = rect(screen.widget(cl::COPY));
    let (nx, ny, nw, _) = rect(screen.widget(cl::NO));
    c.record(
        "m3.the_three_buttons_are_a_centred_row_of_hundreds_four_apart",
        (yw, cw, nw) == (100, 100, 100)
            && yx == (GUI_W - 308) / 2
            && cx == yx + 104
            && nx == cx + 104
            && yy == cy
            && cy == ny,
        format!("x = {yx}, {cx}, {nx}; y = {yy}, {cy}, {ny}"),
    );

    // m4 — the message wraps at `width - 50` and is capped at fifteen rows
    // with a "..." on the last. A 3000-character link is 15 rows and the
    // ellipsis; every row fits.
    let huge = "a".repeat(3000);
    let rows15 = cl::wrap_message(&huge, GUI_W - 50, 15, &width_of);
    let few = cl::wrap_message(URL, GUI_W - 50, 15, &width_of);
    c.record(
        "m4.the_message_wraps_at_width_minus_fifty_and_is_cut_at_fifteen_rows",
        rows15.len() == 15
            && rows15[14].ends_with("...")
            && rows15
                .iter()
                .all(|r| rewo_gpu::text::width(r, &advance) <= GUI_W - 50)
            && !rows15[13].ends_with("...")
            && few.len() == 3
            && few.concat() == URL,
        format!(
            "huge: {} rows, last {:?}; ordinary: {} rows, nothing lost",
            rows15.len(),
            rows15.last(),
            few.len()
        ),
    );

    // m5 — each button's centre presses that button and maps to its answer;
    // the text is click-through; Esc is the "No" answer.
    let mut s = screen.clone();
    let press = |s: &mut Screen, id| {
        let (x, y, w, h) = rect(s.widget(id));
        s.mouse_clicked((x + w / 2) as f64, (y + h / 2) as f64, 0)
    };
    let yes = press(&mut s, cl::YES);
    let copy = press(&mut s, cl::COPY);
    let no = press(&mut s, cl::NO);
    let (mx, my, _, _) = rect(s.widget(cl::MESSAGE));
    let text = s.mouse_clicked((mx + 2) as f64, (my + 2) as f64, 0);
    let esc = cl::key_pressed(&mut s, 256, false);
    c.record(
        "m5.each_button_presses_its_own_answer_and_escape_is_no",
        yes == MouseResult::Pressed(cl::YES)
            && copy == MouseResult::Pressed(cl::COPY)
            && no == MouseResult::Pressed(cl::NO)
            && text == MouseResult::Ignored
            && esc == KeyResult::Pressed(cl::NO)
            && cl::decision_for(cl::YES) == Some(cl::LinkDecision::Open)
            && cl::decision_for(cl::COPY) == Some(cl::LinkDecision::CopyThenCancel)
            && cl::decision_for(cl::NO) == Some(cl::LinkDecision::Cancel),
        format!("{yes:?} {copy:?} {no:?} text {text:?} esc {esc:?}"),
    );

    // m6 — the two options that decide whether the prompt appears, read from
    // an `options.txt` as the live client reads it. `chatLinks` vetoes.
    let read = |text: &str| {
        let o = rewo_net::options::Options::parse(text);
        cl::click_url_action(o.chat_links, o.chat_links_prompt)
    };
    c.record(
        "m6.options_txt_decides_between_prompt_direct_open_and_nothing",
        read("") == cl::UrlClick::Prompt
            && read("chatLinksPrompt:false") == cl::UrlClick::OpenDirect
            && read("chatLinks:false") == cl::UrlClick::Refused
            && read("chatLinks:false\nchatLinksPrompt:true") == cl::UrlClick::Refused,
        "an absent file prompts; the prompt option off opens directly; \
         `chatLinks` off ignores the click whatever the prompt option says",
    );
    Ok(())
}

fn px(buf: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * W + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2]]
}

fn is_clear(p: [u8; 3]) -> bool {
    p[0] == 255 && p[1] == 0 && p[2] == 255
}

/// A button texel: greyscale.
fn is_button(p: [u8; 3]) -> bool {
    p[0] == p[1] && p[1] == p[2]
}

/// The warning's `0xFFCCCC`, within two levels either way.
fn is_warning(p: [u8; 3]) -> bool {
    p[0] >= 253 && p[1].abs_diff(204) <= 2 && p[2].abs_diff(204) <= 2
}

fn is_white(p: [u8; 3]) -> bool {
    p == [255, 255, 255]
}

/// Count pixels in a GUI-space rect (scaled to screen pixels) that satisfy `f`.
fn count_in(buf: &[u8], r: (i32, i32, i32, i32), f: impl Fn([u8; 3]) -> bool) -> usize {
    let (x, y, w, h) = r;
    let mut n = 0;
    for yy in (y * SCALE).max(0)..((y + h) * SCALE).min(H as i32) {
        for xx in (x * SCALE).max(0)..((x + w) * SCALE).min(W as i32) {
            if f(px(buf, xx as u32, yy as u32)) {
                n += 1;
            }
        }
    }
    n
}

fn check_pixels(
    c: &mut Checker,
    args: &ConfirmLinkshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[confirmlinkshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("confirmlinkshot: Vulkan validation requested but not active".into());
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
    let width_of = |t: &str| rewo_gpu::text::width(t, &advance);

    c.record(
        "p0.the_gui_scale_is_the_one_the_predictions_assume",
        rewo_gpu::hud::gui_scale(W as f32, H as f32) == SCALE as f32,
        format!("{W}x{H} -> scale {SCALE}, GUI space {GUI_W}x{GUI_H}"),
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
    let clear = [1.0, 0.0, 1.0, 1.0];

    let shot = |gpu: &mut Gpu,
                off: &mut Offscreen,
                wr: &mut WorldRenderer,
                chrome: rewo_gpu::screen::ScreenDraw,
                lines: Vec<rewo_gpu::world::OwnedTextLine>|
     -> Result<Vec<u8>, String> {
        wr.set_screen(chrome);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        off.read_rgba(gpu)
    };

    let empty = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        rewo_gpu::screen::ScreenDraw::default(),
        Vec::new(),
    )?;
    c.record(
        "p1.a_frame_with_no_screen_is_uniformly_the_clear_colour",
        (0..W * H).all(|i| is_clear(px(&empty, i % W, i / W))),
        "so every count below is the screen and nothing else",
    );

    let screen = cl::build(&untrusted(baked, URL), GUI_W, GUI_H, &width_of);
    let chrome = crate::live_cmd::screen_chrome(&screen, None);
    c.record(
        "p2.the_prompt_asks_for_the_in_world_menu_background_and_no_gradient",
        chrome.backdrop.is_none()
            && chrome.menu_background.is_some_and(|b| b.in_world)
            && chrome.buttons.len() == 3,
        format!(
            "`Screen.extractBackground` with a level behind it; {} buttons \
             (the title, message and warning are text, not buttons)",
            chrome.buttons.len()
        ),
    );

    // From here the background is suppressed so the geometry is measured
    // against the clear; `p2` already proved the wire.
    let mut flat = chrome;
    flat.menu_background = None;
    let frame = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        flat,
        crate::live_cmd::screen_text_lines(&screen, &advance, SCALE as f32),
    )?;

    // p3 — each button is greyscale on its exact rect and clear one pixel
    // outside every edge; the 4-GUI-px gap between neighbours is clear.
    let rects: Vec<(i32, i32, i32, i32)> = [cl::YES, cl::COPY, cl::NO]
        .iter()
        .map(|id| rect(screen.widget(*id)))
        .collect();
    let button_ok = |r: &(i32, i32, i32, i32)| {
        let (x0, y0) = ((r.0 * SCALE) as u32, (r.1 * SCALE) as u32);
        let (x1, y1) = (((r.0 + r.2) * SCALE) as u32, ((r.1 + r.3) * SCALE) as u32);
        x0 >= 1
            && y0 >= 1
            && is_button(px(&frame, x0, y0))
            && is_button(px(&frame, x1 - 1, y1 - 1))
            && is_clear(px(&frame, x0 - 1, y0 + 4))
            && is_clear(px(&frame, x1, y0 + 4))
            && is_clear(px(&frame, x0 + 4, y0 - 1))
            && is_clear(px(&frame, x0 + 4, y1))
    };
    let gap_ok = rects.windows(2).all(|p| {
        let gx = ((p[0].0 + p[0].2) * SCALE + 1) as u32; // inside the 8-px gap
        let gy = ((p[0].1 + 10) * SCALE) as u32;
        is_clear(px(&frame, gx, gy)) && is_clear(px(&frame, gx + 5, gy))
    });
    c.record(
        "p3.three_separate_hundred_wide_buttons_cover_exactly_their_rects",
        rects.iter().all(button_ok) && gap_ok,
        format!("rects {rects:?} (GUI px); greyscale inside, clear around and between"),
    );

    // p4 — the warning is drawn in 0xFFCCCC inside its own rect, and nowhere
    // in the title (white). The same detector is the control for p5.
    let warn_rect = rect(screen.widget(cl::WARNING));
    let title_rect = rect(screen.widget(cl::TITLE));
    let warn_in_warning = count_in(&frame, warn_rect, is_warning);
    let warn_in_title = count_in(&frame, title_rect, is_warning);
    let white_in_title = count_in(&frame, title_rect, is_white);
    c.record(
        "p4.the_warning_is_drawn_in_its_own_colour_and_the_title_is_white",
        warn_in_warning > 50 && warn_in_title == 0 && white_in_title > 50,
        format!(
            "warning-coloured texels: {warn_in_warning} in the warning rect, \
             {warn_in_title} in the title; white in the title: {white_in_title}"
        ),
    );

    // p5 — the control: the TRUSTED variant has no warning widget, and the
    // same detector finds no warning-coloured texel anywhere on the frame.
    let trusted = cl::build(
        &cl::ConfirmLinkScreen::new(&baked.lang, URL, true),
        GUI_W,
        GUI_H,
        &width_of,
    );
    let mut tchrome = crate::live_cmd::screen_chrome(&trusted, None);
    tchrome.menu_background = None;
    let tframe = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        tchrome,
        crate::live_cmd::screen_text_lines(&trusted, &advance, SCALE as f32),
    )?;
    let anywhere = (0..W * H)
        .filter(|i| is_warning(px(&tframe, i % W, i / W)))
        .count();
    c.record(
        "p5.a_trusted_link_draws_no_warning_anywhere",
        trusted.widget(cl::WARNING).is_none() && anywhere == 0,
        format!("{anywhere} warning-coloured texels on the trusted frame"),
    );

    // p6 — every wrapped message row has text on it, within the message rect,
    // and the rows reach down to the last one (a row count that drifted from
    // the drawn rows would leave one empty or one unrendered).
    let (mx, my, mw, _) = rect(screen.widget(cl::MESSAGE));
    let rows = match screen.widget(cl::MESSAGE).map(|w| &w.kind) {
        Some(rewo_ui::screen::WidgetKind::MultiLabel { lines, .. }) => lines.len() as i32,
        _ => 0,
    };
    let per_row: Vec<usize> = (0..rows)
        .map(|i| count_in(&frame, (mx, my + 9 * i, mw, 9), is_white))
        .collect();
    let beyond = count_in(&frame, (mx, my + 9 * rows, mw, 8), is_white);
    c.record(
        "p6.every_wrapped_row_is_drawn_and_nothing_spills_below_the_last",
        rows == 3 && per_row.iter().all(|n| *n > 20) && beyond == 0,
        format!("white texels per row {per_row:?}; {beyond} in the gap below"),
    );

    // p7 — a frame with the screen is not the empty frame, and a button
    // press in the model leaves the pixels alone (the screen is data, not
    // animation): a second render of the same inputs is byte-identical.
    let again = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        {
            let mut ch = crate::live_cmd::screen_chrome(&screen, None);
            ch.menu_background = None;
            ch
        },
        crate::live_cmd::screen_text_lines(&screen, &advance, SCALE as f32),
    )?;
    c.record(
        "p7.the_prompt_renders_deterministically_and_differs_from_the_empty_frame",
        again == frame && frame != empty,
        "same inputs, same bytes; and not the clear",
    );

    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {dir:?}: {e}"))?;
        write_png(&dir.join("confirm_link.png"), &frame)?;
        write_png(&dir.join("confirm_link_trusted.png"), &tframe)?;
    }
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

/// Eyeball output only — never read back.
fn write_png(path: &std::path::Path, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("create {path:?}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .map_err(|e| format!("write {path:?}: {e}"))
}
