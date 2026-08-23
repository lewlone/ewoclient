//! `rewo signshot --check` — the sign-editor gate (M174).
//!
//! Drives the PRODUCTION chain, never a re-derivation (the M45/M86 rule):
//!
//! ```text
//! SignEditState::{key_pressed, char_typed, insert_text}   (TextFieldHelper)
//!          -> live_cmd::SignEditView + {sign_board_sprite, sign_edit_draws}
//!          -> WorldRenderer::{set_screen, set_text} -> ScreenPass / TextPass
//!          -> Offscreen::read_rgba                  (real pixels)
//! ```
//!
//! Pixel predictions come from the BAKE's own sprites (`gui/signs/<wood>.png`,
//! `gui/hanging_signs/<wood>.png`) probed at the centre of each source texel
//! under the board's nearest-integer stretch, so a wrong sheet, wrong crop or
//! wrong placement fails on bytes rather than on "something drew". The caret's
//! wall-clock blink is pinned by constructing the view with `opened` in the
//! past, which makes both phases reachable deterministically.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_world::edit_box::Input;
use rewo_world::sign_edit_screen as se;

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 23;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = 2 — asserted by `p0`, not assumed.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;

#[derive(ClapArgs)]
pub struct SignshotArgs {
    /// Assert every owned property (labels the run; failures exit nonzero
    /// either way — the deathshot convention).
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
            "[signshot] {}  {name}: {detail}",
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

fn width_of(advance: &[u8; 256]) -> impl Fn(&str) -> i32 + '_ {
    |t: &str| rewo_gpu::text::width(t, advance)
}

pub fn run(args: SignshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = rewo_data::DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    let advance = baked.font.as_ref().ok_or("no baked font")?.advance;
    check_model(&mut c, &advance);
    check_derivation(&mut c, &paths)?;
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[signshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    if c.witnessed + c.failures.len() != EXPECTED_WITNESSES {
        return Err(format!(
            "signshot: {} witnesses ran, {EXPECTED_WITNESSES} declared — a property was skipped",
            c.witnessed + c.failures.len()
        ));
    }
    if c.failures.is_empty() {
        println!("[signshot] PASS — {EXPECTED_WITNESSES} witnesses");
        Ok(())
    } else {
        Err(format!("signshot: {} witnesses failed", c.failures.len()))
    }
}

/// The decompile literals every prediction below is checked against are named
/// where they are used; nothing here reads a constant out of the model and
/// calls it a witness (M93q's rule).
fn check_model(c: &mut Checker, advance: &[u8; 256]) {    let wf = width_of(advance);

    // m1 — `TextCursorUtils.isCursorVisible`: 300 on / 300 off, starting
    // visible. The boundaries decide which side of the division each ms lands
    // on; (ms / 300) % 2 == 0 puts 299 visible and 300 not.
    let phases = [(0u64, true), (299, true), (300, false), (599, false), (600, true)];
    let all = phases.iter().all(|(ms, want)| se::cursor_visible(*ms) == *want);
    c.record(
        "m1.the_blink_is_300_on_300_off_starting_visible",
        all,
        format!("phases {phases:?}"),
    );

    // m2/m3 — Up wraps 0→3, Down/Enter wrap 3→0, both to the new line's END,
    // and Enter NEVER closes (AbstractSignEditScreen.java:81-93).
    let mut s = se::SignEditState::new(Default::default(), se::SignKind::Standing);
    s.lines = ["a".into(), String::new(), "abc".into(), "ab".into()];
    s.key_pressed(Input::new(se::KEY_UP, 0), &wf, &mut String::new());
    let up_ok = s.line == 3 && s.cursor == s.lines[3].len();
    s.key_pressed(Input::new(se::KEY_DOWN, 0), &wf, &mut String::new()); // Down wraps 3->0
    let down_ok = s.line == 0 && s.cursor == s.lines[0].len();
    let esc_like = s.key_pressed(Input::new(se::KEY_ENTER, 0), &wf, &mut String::new());
    let enter_ok = esc_like == se::SignKey::Handled && s.line == 1;
    c.record(
        "m2.vertical_keys_wrap_and_land_at_the_lines_end",
        up_ok && down_ok && enter_ok,
        format!("up {up_ok}, down-wrap {down_ok}, enter-moves-without-closing {enter_ok}"),
    );

    // m4 — Esc is Close (Screen.java's shouldCloseOnEsc path), everything the
    // field ate before it is Handled.
    let mut s = se::SignEditState::new(["hi".into(), String::new(), String::new(), String::new()], se::SignKind::Standing);
    let r = s.key_pressed(Input::new(256, 0), &wf, &mut String::new());
    c.record("m3.escape_is_a_close_not_an_insert", r == se::SignKey::Close, format!("{r:?}"));

    // m5 — the validator tests the WHOLE candidate against
    // `font.width(s) <= getMaxTextLineWidth()` and a failing insert is a
    // silent no-op: text AND cursor unchanged (TextFieldHelper.java:120-131).
    let mut s = se::SignEditState::new(["seed".into(), String::new(), String::new(), String::new()], se::SignKind::Standing);
    s.set_cursor_to_end(false);
    let (before_text, before_cursor) = (s.lines[0].clone(), s.cursor);
    s.insert_text(&"M".repeat(40), &wf);
    c.record(
        "m4.an_over_wide_candidate_is_rejected_in_its_entirety",
        s.lines[0] == before_text && s.cursor == before_cursor,
        format!("line {:?}, cursor {}", s.lines[0], s.cursor),
    );

    // m6 — an insert over a selection that FAILS leaves the line's text
    // intact (selection included) with only the selection collapsed to the
    // span's start. Odd, transcribed, and load-bearing for paste.
    let mut s = se::SignEditState::new(["word".into(), String::new(), String::new(), String::new()], se::SignKind::Standing);
    s.selection = 0;
    s.cursor = s.lines[0].len();
    s.insert_text(&"M".repeat(40), &wf);
    c.record(
        "m5.a_failed_paste_keeps_the_text_but_collapses_the_selection",
        s.lines[0] == "word" && s.selection == s.cursor && s.cursor == 0,
        format!("line {:?}, sel {} cur {}", s.lines[0], s.selection, s.cursor),
    );

    // m7 — stripFormatting removes §-codes; a trailing bare § SURVIVES.
    let stripped = se::strip_formatting("§ahi§r there §");
    c.record(
        "m6.strip_formatting_and_the_trailing_bare_section_quirk",
        stripped == "hi there §",
        format!("{stripped:?}"),
    );

    // m7b — the validator is `<=`: a candidate of EXACTLY the max width fits
    // (AbstractSignEditScreen.java:60-66). A deterministic measure keeps the
    // boundary reachable — insert_text takes the validator as a parameter, so
    // this still drives the production path.
    let mut s = se::SignEditState::new([String::new(), String::new(), String::new(), String::new()], se::SignKind::Standing);
    let measure = |t: &str| t.chars().count() as i32 * 10;
    s.insert_text("---------", &measure); // exactly 90
    let exact_fits = s.lines[0].chars().count() == 9 && s.cursor == 9;
    s.insert_text("-", &measure); // 100 > 90
    c.record(
        "m7b.a_candidate_of_exactly_the_max_width_is_accepted",
        exact_fits && s.lines[0].chars().count() == 9,
        format!("exact-fit {exact_fits}, after-overflow len {}", s.lines[0].chars().count()),
    );

    // m8 — Delete acts AND reports unhandled (vanilla's case 261 has no
    // `return true`, so it also falls through to super).
    let mut s = se::SignEditState::new(["ab".into(), String::new(), String::new(), String::new()], se::SignKind::Standing);
    s.cursor = 1;
    s.selection = 1;
    let r = s.key_pressed(Input::new(rewo_world::edit_box::key::DELETE, 0), &wf, &mut String::new());
    c.record(
        "m7.delete_acts_and_falls_through",
        s.lines[0] == "a" && r == se::SignKey::Unhandled,
        format!("line {:?}, key {r:?}", s.lines[0]),
    );

    // m9 — charTyped consumes EVERY character but inserts only allowed ones;
    // § (167) can never be typed.
    let mut s = se::SignEditState::new([String::new(), String::new(), String::new(), String::new()], se::SignKind::Standing);
    let consumed_all = s.char_typed('§', &wf) && s.char_typed('x', &wf);
    c.record(
        "m8.char_typed_consumes_everything_but_only_inserts_allowed_chars",
        consumed_all && s.lines[0] == "x",
        format!("line {:?}", s.lines[0]),
    );

    // m10 — word motion skips separator runs in both directions.
    let fwd = se::word_position("ab  cd", 1, 0);
    let fwd_clamped = se::word_position("ab  cd", 1, 4);
    let back = se::word_position("ab  cd", -1, 6);
    c.record(
        "m9.word_motion_crosses_separator_runs_both_ways",
        fwd == 4 && fwd_clamped == 6 && back == 4,
        format!("fwd {fwd} (want 4), fwd@4 {fwd_clamped} (want 6), back {back} (want 4)"),
    );

    // m11 — the three board rects, hand-derived from the decompile literals:
    // standing/wall translate (width/2, 90+27) then scale 3.9 blit
    // (-12,-13,24, wall?12:26); hanging translates (width/2, 125-13) scale 4.5
    // blit (-8,-8,16,16). Nearest-integer rounding per edge.
    let stand = se::board_sprite(se::SignKind::Standing, GUI_W);
    let wall = se::board_sprite(se::SignKind::Wall, GUI_W);
    let hang = se::board_sprite(se::SignKind::Hanging, GUI_W);
    let ok = stand == (113, 66, 94, 101)
        && wall == (113, 66, 94, round_half_up(12.0 * 3.9))
        && hang == (GUI_W / 2 - 36, 76, 72, 72);
    c.record(
        "m10.board_rects_match_the_decompile_literals",
        ok,
        format!("standing {stand:?}, wall {wall:?}, hanging {hang:?}"),
    );

    // m12 — one line anchor: x = w/2 - ts*width/2 (centred on its OWN width),
    // y = yOffset + ts*(i*lineHeight - midpoint); standing midpoint = 20.
    let (lx, ly) = se::line_origin(se::SignKind::Standing, GUI_W, 0, 30);
    let ts = se::SIGN_TEXT_SCALE;
    let ok = (lx - (GUI_W as f32 / 2.0 - ts * 15.0)).abs() < 1e-4
        && (ly - (se::SIGN_Y_OFFSET - ts * 20.0)).abs() < 1e-4;
    c.record(
        "m11.a_line_is_centred_on_its_own_width",
        ok,
        format!("({lx:.3}, {ly:.3})"),
    );

    // m13 — the selection spans substring widths around the centre over one
    // line height (AbstractSignEditScreen.java:212-220).
    let mut s = se::SignEditState::new(["abcd".into(), String::new(), String::new(), String::new()], se::SignKind::Standing);
    s.cursor = 1;
    s.selection = 3;
    let sel = se::selection_rect(&s, GUI_W, &wf).expect("a selection exists");
    let w01 = wf("a") as f32;
    let w03 = wf("abc") as f32;
    let w04 = wf("abcd") as f32;
    let expect_x = GUI_W as f32 / 2.0 + ts * (w01 - w04 / 2.0);
    let expect_w = ts * (w03 - w01);
    let ok = (sel.0 - expect_x).abs() <= 0.5 && (sel.2 - expect_w).abs() <= 0.5 && sel.3 > 0.0;
    c.record(
        "m12.the_selection_rect_spans_substring_widths",
        ok,
        format!("rect {sel:?} vs x {expect_x:.3} w {expect_w:.3}"),
    );

    // m14 — at/past the end the caret is the "_" GLYPH; inside the line it is
    // a 1-text-px bar spanning lineHeight+1 (TextCursorUtils.java:11-17).
    let mut s = se::SignEditState::new(["abc".into(), String::new(), String::new(), String::new()], se::SignKind::Standing);
    s.cursor = 1;
    s.selection = 1;
    let bar = se::caret_draw(&s, GUI_W, &wf);
    s.cursor = 3;
    let under = se::caret_draw(&s, GUI_W, &wf);
    let ok = matches!(bar, se::CaretDraw::Bar { .. }) && matches!(under, se::CaretDraw::Underscore { .. });
    c.record(
        "m13.caret_is_a_bar_inside_the_line_and_an_underscore_at_its_end",
        ok,
        format!("bar {bar:?}, underscore {under:?}"),
    );

    // m15 — the editor's colour branch: glowing → the dye at FULL strength;
    // else the dye scaled 0.4 TRUNCATING (red 0xFF0000 → 0x660000; a rounded
    // scale gives 0x666666-ish neighbours and fails on bytes).
    let dark = crate::live_cmd::sign_edit_line_color(0xFF0000, false);
    let glow = crate::live_cmd::sign_edit_line_color(0xFF0000, true);
    c.record(
        "m14.dark_text_is_the_dye_at_forty_percent_truncating",
        dark == 0x660000 && glow == 0xFF0000,
        format!("dark {dark:#06x}, glowing {glow:#06x}"),
    );
}

/// m15 (M93b's rule — the gate must reach the PRODUCTION derivation): the
/// wood/hanging/attachment triple is what `pump_sign_editor` keys the screen
/// class and the sheet index on, and every pixel witness below passes
/// `wood: 0` by construction, so a regression in `SignStates::load`'s
/// derivation would otherwise be invisible here. The gate extracts one real
/// state id per named sign from blocks.json (fixture plumbing only) and
/// grades what production returns for it.
fn check_derivation(c: &mut Checker, paths: &rewo_data::DataPaths) -> Result<(), String> {
    let signs = rewo_data::sign_states::SignStates::load(&paths.blocks_json())?;
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(paths.blocks_json()).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    // (block name, want_wood, want_hanging, want_wall)
    let cases = [
        ("oak_sign", 0u8, false, false),
        ("spruce_hanging_sign", 1, true, false),
        ("dark_oak_wall_sign", 6, false, true),
        ("bamboo_wall_hanging_sign", 11, true, true),
    ];
    let mut all = true;
    let mut detail = String::new();
    for (name, want_wood, want_hanging, want_wall) in cases {
        let Some(states) = json[format!("minecraft:{name}")]["states"].as_array() else {
            return Err(format!("blocks.json has no states for {name}"));
        };
        let Some(state_id) = states.first().and_then(|s| s["id"].as_u64()) else {
            return Err(format!("no state id for {name}"));
        };
        match signs.get(state_id as u32) {
            Some(s) => {
                let ok = s.hanging == want_hanging
                    && s.wood_index == want_wood
                    && (s.attachment == rewo_data::sign_states::SignAttachment::Wall) == want_wall;
                if !ok {
                    all = false;
                    detail = format!(
                        "{name}: got hanging {} wood {} wall {}",
                        s.hanging,
                        s.wood_index,
                        s.attachment == rewo_data::sign_states::SignAttachment::Wall
                    );
                }
            }
            None => {
                all = false;
                detail = format!("{name}: state {state_id} not in the table");
            }
        }
    }
    c.record(
        "m15.the_production_sign_state_derivation_keys_the_editor",
        all,
        if all { "four shapes resolve to the right wood/hanging/wall".into() } else { detail },
    );
    Ok(())
}

/// f32 rounding of the sprite heights, spelled out so m11 cannot silently
/// agree through the model's own `.round()`.
fn round_half_up(v: f32) -> i32 {
    (v + 0.5).floor() as i32
}

fn check_pixels(
    c: &mut Checker,
    args: &SignshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[signshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("signshot: Vulkan validation requested but not active".into());
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

    c.record(
        "p0.the_gui_scale_is_the_one_the_predictions_assume",
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
    // A pure green clear: nothing the editor draws is green — the boards are
    // wood browns, the gradient dark, the text near-black.
    let clear = [0.0, 1.0, 0.0, 1.0];

    let make_view = |kind: se::SignKind, opened_ms_ago: u64| -> crate::live_cmd::SignEditView {
        let mut state = se::SignEditState::new(
            ["hello sign".into(), String::new(), String::new(), String::new()],
            kind,
        );
        state.cursor = 6; // inside the line → the BAR caret
        state.selection = state.cursor;
        crate::live_cmd::SignEditView {
            state,
            pos: (0, 0, 0),
            is_front: true,
            dye: 0,
            glowing: false,
            wood: 0,
            opened: std::time::Instant::now()
                - std::time::Duration::from_millis(opened_ms_ago),
        }
    };

    /// What a frame contains. `ChromeOnly` is the editor's Screen (gradient
    /// backdrop + Done button) with NO board and NO text — the control that
    /// lets p3 prove "the wall board stops at row 12" against the SAME
    /// backdrop rather than against a bare clear.
    enum Frame<'a> {
        Empty,
        ChromeOnly,
        View(&'a crate::live_cmd::SignEditView),
    }

    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    frame: Frame,
                    dump: &str|
     -> Result<Vec<u8>, String> {
        let mut chrome = rewo_gpu::screen::ScreenDraw::default();
        let mut lines = Vec::new();
        match frame {
            Frame::Empty => {}
            Frame::ChromeOnly => {
                let screen = se::build_screen(GUI_W, H as i32 / SCALE, "Done");
                chrome = crate::live_cmd::screen_chrome(&screen, None);
                lines.extend(crate::live_cmd::screen_text_lines(&screen, &advance, SCALE as f32));
            }
            Frame::View(view) => {
                let screen = se::build_screen(GUI_W, H as i32 / SCALE, "Done");
                chrome = crate::live_cmd::screen_chrome(&screen, None);
                chrome
                    .sprites
                    .push(crate::live_cmd::sign_board_sprite(view.state.kind, view.wood, GUI_W));
                let (mut stext, fills) =
                    crate::live_cmd::sign_edit_draws(view, GUI_W, &advance, SCALE as f32);
                chrome.sprites.extend(fills);
                lines.extend(crate::live_cmd::screen_text_lines(&screen, &advance, SCALE as f32));
                lines.append(&mut stext);
            }
        }
        wr.set_screen(chrome);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        if let Some(dir) = &args.out_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = off.save_png(gpu, &dir.join(format!("sign-{dump}.png")));
        }
        off.read_rgba(gpu)
    };

    // Frame pixel accessor: (fb x, fb y) -> rgb.
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
    // Frame coordinate of source texel (dx, dy)'s CENTRE under the stretch —
    // the pass samples nearest, so the centre always lands inside the source
    // texel it came from.
    let stretched = |sx: f32, sy: f32, sw: f32, sh: f32, tw: i32, th: i32, dx: f32, dy: f32| -> (i32, i32) {
        (
            sx.round() as i32 + ((dx + 0.5) / sw * tw as f32).floor() as i32,
            sy.round() as i32 + ((dy + 0.5) / sh * th as f32).floor() as i32,
        )
    };

    let empty = shot(&mut gpu, &mut off, &mut wr, Frame::Empty, "empty")?;

    // p1 — no view, no board: the empty frame keeps the clear at the board's
    // centre.
    let cx = (GUI_W / 2) * SCALE;
    let cy = (90 + 27) * SCALE;
    c.record(
        "p1.no_editor_draws_no_board",
        px(&empty, cx, cy) == [0, 255, 0],
        format!("empty frame at ({cx},{cy}) = {:?}", px(&empty, cx, cy)),
    );

    // p2 — the standing board IS gui/signs/oak.png, probed through the
    // nearest-integer 24x26 -> 94x101 stretch.
    let oak = &widgets.sign_boards[0];
    let (bx, by, bw, bh) = se::board_sprite(se::SignKind::Standing, GUI_W);
    let probes = [(3u32, 3u32), (20, 4), (12, 10), (5, 22), (18, 16)];
    let mut all = true;
    let mut detail = String::new();
    for (dx, dy) in probes {
        let (fx, fy) = stretched(bx as f32, by as f32, 24.0, 26.0, bw, bh, dx as f32, dy as f32);
        let t = texel(oak, dx, dy);
        let f = px(&shot(&mut gpu, &mut off, &mut wr, Frame::View(&make_view(se::SignKind::Standing, 100)), "standing")?, fx * SCALE, fy * SCALE);
        if t[3] == 255 && !close([t[0], t[1], t[2]], f, 8) {
            all = false;
            detail = format!("texel ({dx},{dy}) = {t:?}, frame = {f:?}");
            break;
        }
    }
    c.record(
        "p2.the_standing_board_is_the_oak_sheet_through_the_stretch",
        all,
        if all { "five centred probes match within 8".into() } else { detail },
    );

    // p3 — a WALL sign crops to the top 12 rows. Three legs: (a) the plaque
    // region still matches the sheet; (b) directly below the cropped board
    // the wall frame equals a CHROME-ONLY frame at the same point — same
    // backdrop, nothing extra drawn; (c) a STANDING board at that same point
    // DIFFERS from chrome-only, because its post rows are still being blitted
    // there — which pins the crop, not merely the wall board's absence.
    let wall_view = make_view(se::SignKind::Wall, 100);
    let wall_frame = shot(&mut gpu, &mut off, &mut wr, Frame::View(&wall_view), "wall")?;
    let chrome_only = shot(&mut gpu, &mut off, &mut wr, Frame::ChromeOnly, "chrome-only")?;
    let standing_view = make_view(se::SignKind::Standing, 100);
    let standing_frame = shot(&mut gpu, &mut off, &mut wr, Frame::View(&standing_view), "standing-crop-control")?;
    let (_, _, ww, wh) = se::board_sprite(se::SignKind::Wall, GUI_W);
    let (fx, fy) = stretched(bx as f32, by as f32, 24.0, 26.0, ww, wh, 12.0, 5.0);
    let t = texel(oak, 12, 5);
    let plaque = close([t[0], t[1], t[2]], px(&wall_frame, fx * SCALE, fy * SCALE), 8);
    let below_x = (bx + ww / 2) * SCALE;
    let below_y = (by + wh + 3) * SCALE;
    let below_eq_chrome = px(&wall_frame, below_x, below_y) == px(&chrome_only, below_x, below_y);
    let standing_draws_post =
        px(&standing_frame, below_x, below_y) != px(&chrome_only, below_x, below_y);
    c.record(
        "p3.a_wall_board_draws_only_the_top_twelve_rows",
        plaque && below_eq_chrome && standing_draws_post,
        format!(
            "plaque {plaque}, wall-below == chrome-only {below_eq_chrome}, standing-below differs {}",
            standing_draws_post
        ),
    );

    // p4 — the hanging board is gui/hanging_signs/oak.png at exactly
    // (124, 76, 72, 72): integral, so the probe is plain texel mapping.
    let hoak = &widgets.hanging_sign_boards[0];
    let (hx, hy, hw, hh) = se::board_sprite(se::SignKind::Hanging, GUI_W);
    let hang_frame = shot(&mut gpu, &mut off, &mut wr, Frame::View(&make_view(se::SignKind::Hanging, 100)), "hanging")?;
    let hprobes = [(4u32, 4u32), (11, 3), (8, 12)];
    let mut all = true;
    let mut detail = String::new();
    for (dx, dy) in hprobes {
        let fx = hx + ((dx as i32 + 1) * hw) / 17;
        let fy = hy + ((dy as i32 + 1) * hh) / 17;
        let t = texel(hoak, dx, dy);
        let f = px(&hang_frame, fx * SCALE, fy * SCALE);
        if t[3] == 255 && !close([t[0], t[1], t[2]], f, 8) {
            all = false;
            detail = format!("texel ({dx},{dy}) = {t:?}, frame = {f:?}");
            break;
        }
    }
    c.record(
        "p4.the_hanging_board_is_the_hanging_oak_sheet",
        all,
        if all { "three probes match within 8".into() } else { detail },
    );

    // p5 — the four lines render: a band around line 0 — derived from the
    // ACTUAL line width, not a zero-width origin (the first cut centred a
    // two-pixel window between the words and measured nothing) — differs from
    // a BLANK-LINE caret-hidden control in MORE than a handful of pixels.
    let mut view = make_view(se::SignKind::Standing, 350);
    view.state.lines = ["hello sign".into(), String::new(), String::new(), String::new()];
    let with_text = shot(&mut gpu, &mut off, &mut wr, Frame::View(&view), "text")?;
    let mut bare_view = make_view(se::SignKind::Standing, 350);
    bare_view.state.lines = Default::default();
    let bare_board = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        Frame::View(&bare_view),
        "bare",
    )?;
    let lw = rewo_gpu::text::width("hello sign", &advance);
    let (ox, oy) = se::line_origin(se::SignKind::Standing, GUI_W, 0, lw);
    let y0 = ((oy.round() as i32) - 1).max(0) * SCALE;
    let y1 = (oy.round() as i32 + 10) * SCALE;
    let x0 = ((ox.round() as i32) - 2).max(0) * SCALE;
    let x1 = (ox.round() as i32 + lw + 2) * SCALE;
    let mut diffs = 0usize;
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (i32::MAX, 0, i32::MAX, 0);
    for y in y0..y1.min(H as i32) {
        for x in x0..x1 {
            if px(&with_text, x, y) != px(&bare_board, x, y) {
                diffs += 1;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    let bbox = if diffs > 0 {
        format!("x {min_x}..{max_x}, y {min_y}..{max_y}")
    } else {
        "none".into()
    };
    c.record(
        "p5.the_first_line_renders_ink_over_the_board",
        diffs >= 8,
        format!("{diffs} differing pixels in the line-0 band; bbox {bbox}; band x {x0}..{x1}, y {y0}..{y1}"),
    );

    // p6 — the caret's wall-clock blink flips pixels: two frames identical
    // except for `opened`'s phase (visible at 100 ms, hidden at 350 ms).
    let on = shot(&mut gpu, &mut off, &mut wr, Frame::View(&make_view(se::SignKind::Standing, 100)), "caret-on")?;
    let off_frame = shot(&mut gpu, &mut off, &mut wr, Frame::View(&make_view(se::SignKind::Standing, 350)), "caret-off")?;
    let mut flipped = 0usize;
    for y in (by * SCALE)..((by + bh) * SCALE) {
        for x in (bx * SCALE)..((bx + bw) * SCALE) {
            if px(&on, x, y) != px(&off_frame, x, y) {
                flipped += 1;
            }
        }
    }
    c.record(
        "p6.the_caret_blink_flips_pixels_inside_the_board",
        flipped >= 2,
        format!("{flipped} pixels flip between phases"),
    );

    // Explicit teardown in child-before-device order, the bookshot rule —
    // relying on Drop here leaves every descriptor set alive at
    // vkDestroyDevice and dies with a VUID storm (and a nonzero exit) after
    // all 21 witnesses have already passed.
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}
