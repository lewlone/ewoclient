//! `rewo bookshot --check` — the written-book reader gate (M172).
//!
//! Drives the PRODUCTION chain, never a re-derivation (the M45/M86 rule):
//!
//! ```text
//! SlotText -> live_cmd::resolve_book_pages          (fromItem + resolve + wrap)
//!          -> book_view_screen::{build_screen, draws}
//!          -> live_cmd::{screen_chrome, book_sprite, book_text_lines}
//!          -> WorldRenderer::{set_screen, set_text} -> ScreenPass / TextPass
//!          -> Offscreen::read_rgba                  (real pixels)
//! ```
//!
//! Pixel predictions come from the BAKE's own sprites — the cropped
//! `book.png` and the four `widget/page_*` arrows — probed texel-for-texel at
//! GUI scale 2, so a wrong sheet, a wrong placement or a wrong crop all fail
//! on bytes rather than on "something drew".

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_world::book_view_screen::{self as bv, BookViewScreen};
use rewo_world::chat_style::ChatSpan;
use rewo_world::inventory::SlotText;

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 21;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = 2 — asserted by `p0`, not assumed.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;

#[derive(ClapArgs)]
pub struct BookshotArgs {
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
            "[bookshot] {}  {name}: {detail}",
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

fn span(text: &str) -> ChatSpan {
    ChatSpan {
        text: text.into(),
        color: [0.0, 0.0, 0.0],
        bold: false,
        italic: false,
        underlined: false,
        strikethrough: false,
        obfuscated: false,
        events: None,
    }
}

pub fn run(args: BookshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = rewo_data::DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    check_model(&mut c, &baked);
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[bookshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    if c.witnessed + c.failures.len() != EXPECTED_WITNESSES {
        return Err(format!(
            "bookshot: {} witnesses ran, {EXPECTED_WITNESSES} declared — a property was skipped",
            c.witnessed + c.failures.len()
        ));
    }
    if c.failures.is_empty() {
        println!("[bookshot] PASS — {EXPECTED_WITNESSES} witnesses");
        Ok(())
    } else {
        Err(format!("bookshot: {} witnesses failed", c.failures.len()))
    }
}

/// The `fromItem` + resolve + wrap chain, through the production
/// `resolve_book_pages` — no GPU.
fn check_model(c: &mut Checker, baked: &assets::BakedAssets) {
    use rewo_proto::nbt::Nbt;
    let advance = baked.font.as_ref().map(|f| f.advance);
    let lang = Some(&baked.lang);

    // m0 — a written book's string page resolves to one black line.
    let written = SlotText {
        has_written_book: true,
        book_pages: vec![Nbt::String("hello reader".into())],
        ..Default::default()
    };
    let pages = crate::live_cmd::resolve_book_pages(&written, advance.as_ref(), lang);
    let ok = pages.as_ref().is_some_and(|p| {
        p.len() == 1
            && p[0].len() == 1
            && p[0][0].len() == 1
            && p[0][0][0].text == "hello reader"
            && p[0][0][0].color == [0.0, 0.0, 0.0]
    });
    c.record(
        "m0.a_written_string_page_is_one_black_line",
        ok,
        format!(
            "PAGE_TEXT_STYLE is the BASE, so an unstyled page is black — got {:?}",
            pages.as_ref().map(|p| p.len())
        ),
    );

    // m1 — a styled span keeps its own colour over the black base
    // (`mergeStyles`: component-present wins).
    let styled = SlotText {
        has_written_book: true,
        book_pages: vec![Nbt::Compound(vec![
            ("text".into(), Nbt::String("warm".into())),
            ("color".into(), Nbt::String("red".into())),
        ])],
        ..Default::default()
    };
    let pages = crate::live_cmd::resolve_book_pages(&styled, advance.as_ref(), lang);
    let red = pages
        .as_ref()
        .and_then(|p| p.first())
        .and_then(|p| p.first())
        .and_then(|l| l.first())
        .map(|s| s.color);
    // The witness was wrong before the code was (again): Minecraft `red` is
    // ChatFormatting.RED = 0xFF5555, whose GREEN channel is 0.333 — a
    // "g < 0.2" assertion rejects the correct answer. Exact match instead.
    c.record(
        "m1.a_pages_own_colour_beats_the_black_base",
        red == Some(rewo_world::chat_style::rgb_f32(0xFF5555)),
        format!("color = {red:?} (want RED 0xFF5555 — forcing black destroys styled pages)"),
    );

    // m2 — the WRITTEN component wins even with zero pages: an empty reader,
    // not the writable fallback and not nothing.
    let both = SlotText {
        has_written_book: true,
        has_writable_book: true,
        writable_pages: vec!["draft".into()],
        ..Default::default()
    };
    let pages = crate::live_cmd::resolve_book_pages(&both, advance.as_ref(), lang);
    c.record(
        "m2.a_zero_page_written_book_opens_empty_not_the_draft",
        pages.as_ref().is_some_and(|p| p.is_empty()),
        format!("fromItem tries WRITTEN_BOOK_CONTENT first — got {pages:?}"),
    );

    // m3 — a writable book falls back to its plain strings as literals.
    let writable = SlotText {
        has_writable_book: true,
        writable_pages: vec!["a draft".into()],
        ..Default::default()
    };
    let pages = crate::live_cmd::resolve_book_pages(&writable, advance.as_ref(), lang);
    c.record(
        "m3.a_writable_book_shows_its_strings_read_only",
        pages.as_ref().is_some_and(|p| {
            p.len() == 1 && p[0].len() == 1 && p[0][0][0].text == "a draft"
        }),
        format!("no WritableBookViewScreen exists in 26.2 — got {pages:?}"),
    );

    // m4 — neither component: NO screen (handleOpenBook silently does
    // nothing), never an empty book.
    let none = SlotText::default();
    c.record(
        "m4.no_book_component_opens_nothing",
        crate::live_cmd::resolve_book_pages(&none, advance.as_ref(), lang).is_none(),
        "BookAccess.fromItem returns null and no screen opens",
    );

    // m5 — the wrap is StringSplitter.splitLines, NOT the chat's
    // wrapComponents: a width-wrapped continuation line has NO leading indent
    // space. This is the discriminating witness between the two wrappers.
    let long = SlotText {
        has_written_book: true,
        book_pages: vec![Nbt::String(
            "one two three four five six seven eight nine ten eleven twelve".into(),
        )],
        ..Default::default()
    };
    let pages = crate::live_cmd::resolve_book_pages(&long, advance.as_ref(), lang);
    let ok = pages.as_ref().is_some_and(|p| {
        let lines = &p[0];
        lines.len() >= 2
            && lines[1..]
                .iter()
                .all(|l| l.first().is_none_or(|s| !s.text.starts_with(' ')))
    });
    c.record(
        "m5.a_wrapped_continuation_has_no_indent_space",
        ok,
        format!(
            "wrap_components would prepend the chat INDENT — lines: {:?}",
            pages.as_ref().map(|p| p[0].len())
        ),
    );
}

fn check_pixels(
    c: &mut Checker,
    args: &BookshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[bookshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("bookshot: Vulkan validation requested but not active".into());
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
    let baked_lang = &baked.lang;

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
    // A pure green clear: nothing the book draws is green — the parchment is
    // warm greys, the gradient dark, the text black.
    let clear = [0.0, 1.0, 0.0, 1.0];

    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    book: Option<(&BookViewScreen, Option<(i32, i32)>)>,
                    dump: &str|
     -> Result<Vec<u8>, String> {
        let mut chrome = rewo_gpu::screen::ScreenDraw::default();
        let mut lines = Vec::new();
        if let Some((view, mouse)) = book {
            let screen = bv::build_screen(GUI_W, H as i32 / SCALE, "Done");
            chrome = crate::live_cmd::screen_chrome(&screen, mouse.map(|(x, y)| (x as f64, y as f64)));
            for d in bv::draws(view, GUI_W, mouse) {
                chrome.sprites.push(crate::live_cmd::book_sprite(d));
            }
            lines = crate::live_cmd::screen_text_lines(&screen, &advance, SCALE as f32);
            lines.extend(crate::live_cmd::book_text_lines(
                view,
                GUI_W,
                &advance,
                SCALE as f32,
                &baked_lang,
            ));
        }
        wr.set_screen(chrome);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        if let Some(dir) = &args.out_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = off.save_png(gpu, &dir.join(format!("book-{dump}.png")));
        }
        off.read_rgba(gpu)
    };

    // Frame pixel accessor: (fb x, fb y) -> rgb.
    let px = |f: &[u8], x: i32, y: i32| -> [u8; 3] {
        let i = ((y as u32 * W + x as u32) * 4) as usize;
        [f[i], f[i + 1], f[i + 2]]
    };
    // A sprite texel: rgba at (dx, dy).
    let texel = |s: &rewo_data::assets::HudSprite, dx: u32, dy: u32| -> [u8; 4] {
        let i = ((dy * s.w + dx) * 4) as usize;
        [s.rgba[i], s.rgba[i + 1], s.rgba[i + 2], s.rgba[i + 3]]
    };
    let close = |a: [u8; 3], b: [u8; 3], tol: i32| -> bool {
        a.iter()
            .zip(b.iter())
            .all(|(x, y)| (*x as i32 - *y as i32).abs() <= tol)
    };

    let left = BookViewScreen::background_left(GUI_W); // (320-192)/2 = 64
    let three = BookViewScreen::new(vec![
        vec![vec![span("hello reader")]],
        vec![vec![span("middle")]],
        vec![vec![span("the end")]],
    ]);
    let empty_frame = shot(&mut gpu, &mut off, &mut wr, None, "empty")?;
    let page0 = shot(&mut gpu, &mut off, &mut wr, Some((&three, None)), "page0")?;

    // p1 — no book, no parchment: the empty frame keeps the clear at the
    // book's centre.
    let centre_fb = ((left + 96) * SCALE, (2 + 96) * SCALE);
    c.record(
        "p1.no_book_draws_no_parchment",
        px(&empty_frame, centre_fb.0, centre_fb.1) == [0, 255, 0],
        format!("empty frame at {centre_fb:?} = {:?}", px(&empty_frame, centre_fb.0, centre_fb.1)),
    );

    // p2 — the parchment is the CROPPED book.png texel-for-texel: probe five
    // spread texels of the 192x192 crop against the frame at GUI scale 2.
    let book_bg = &widgets.book_background;
    // Probes avoid the text regions the screen draws OVER the parchment:
    // the indicator row is crop dy 16..25 (the first cut probed (96,20)
    // and read back a glyph pixel of the correctly-rendered indicator).
    let probes = [(30u32, 30u32), (96, 10), (160, 60), (50, 150), (140, 170)];
    let mut all = true;
    let mut detail = String::new();
    for (dx, dy) in probes {
        let t = texel(book_bg, dx, dy);
        let f = px(
            &page0,
            (left + dx as i32) * SCALE,
            (2 + dy as i32) * SCALE,
        );
        if t[3] == 255 && !close([t[0], t[1], t[2]], f, 2) {
            all = false;
            detail = format!("texel ({dx},{dy}) = {t:?}, frame = {f:?}");
        }
    }
    c.record(
        "p2.the_parchment_is_the_cropped_book_png_byte_for_byte",
        all,
        if all { "five probes match within 2".into() } else { detail },
    );

    // p3 — page 0 shows the FORWARD arrow's own texels at (left+116, 159)...
    let fwd = &widgets.page_buttons[0];
    let (adx, ady) = opaque_texel(fwd).ok_or("page_forward has no opaque texel")?;
    let fx = (left + bv::PAGE_FORWARD_BUTTON_X + adx as i32) * SCALE;
    let fy = (bv::BACKGROUND_TOP + bv::PAGE_BUTTON_Y + ady as i32) * SCALE;
    let t = texel(fwd, adx, ady);
    c.record(
        "p3.page_zero_draws_the_forward_arrow",
        close([t[0], t[1], t[2]], px(&page0, fx, fy), 2),
        format!("arrow texel ({adx},{ady}) = {t:?}, frame = {:?}", px(&page0, fx, fy)),
    );

    // p4 — ...and NOT the back arrow: that spot shows the parchment.
    let back = &widgets.page_buttons[2];
    let (bdx, bdy) = opaque_texel(back).ok_or("page_backward has no opaque texel")?;
    let bg_under = texel(
        book_bg,
        (bv::PAGE_BACK_BUTTON_X + bdx as i32) as u32,
        (bv::PAGE_BUTTON_Y + bdy as i32 - bv::BACKGROUND_TOP + 2) as u32,
    );
    let bx = (left + bv::PAGE_BACK_BUTTON_X + bdx as i32) * SCALE;
    let by = (bv::BACKGROUND_TOP + bv::PAGE_BUTTON_Y + bdy as i32) * SCALE;
    c.record(
        "p4.a_hidden_back_arrow_draws_nothing_at_all",
        close(
            [bg_under[0], bg_under[1], bg_under[2]],
            px(&page0, bx, by),
            2,
        ),
        format!(
            "under-texel = {bg_under:?}, frame = {:?} (a PageButton with visible=false is not drawn)",
            px(&page0, bx, by)
        ),
    );

    // p5 — hover picks the HIGHLIGHTED sprite: find a texel where the two
    // sprites differ and assert the hovered frame carries the highlighted
    // bytes.
    let fwd_h = &widgets.page_buttons[1];
    let (hdx, hdy) =
        differing_opaque_texel(fwd, fwd_h).ok_or("forward and highlighted never differ")?;
    let hover = (
        left + bv::PAGE_FORWARD_BUTTON_X + 2,
        bv::BACKGROUND_TOP + bv::PAGE_BUTTON_Y + 2,
    );
    let hovered = shot(&mut gpu, &mut off, &mut wr, Some((&three, Some(hover))), "hover")?;
    let th = texel(fwd_h, hdx, hdy);
    let hx = (left + bv::PAGE_FORWARD_BUTTON_X + hdx as i32) * SCALE;
    let hy = (bv::BACKGROUND_TOP + bv::PAGE_BUTTON_Y + hdy as i32) * SCALE;
    c.record(
        "p5.hover_draws_the_highlighted_arrow",
        close([th[0], th[1], th[2]], px(&hovered, hx, hy), 2)
            && !close([th[0], th[1], th[2]], px(&page0, hx, hy), 2),
        format!(
            "highlighted texel = {th:?}, hovered = {:?}, unhovered = {:?}",
            px(&hovered, hx, hy),
            px(&page0, hx, hy)
        ),
    );

    // p6 — the LAST page reverses the arrows: back visible, forward not.
    let mut last = three.clone();
    last.page_forward();
    last.page_forward();
    let lastf = shot(&mut gpu, &mut off, &mut wr, Some((&last, None)), "last")?;
    let tb = texel(back, bdx, bdy);
    let fwd_under = texel(
        book_bg,
        (bv::PAGE_FORWARD_BUTTON_X + adx as i32) as u32,
        (bv::PAGE_BUTTON_Y + ady as i32 - bv::BACKGROUND_TOP + 2) as u32,
    );
    c.record(
        "p6.the_last_page_reverses_the_arrows",
        close([tb[0], tb[1], tb[2]], px(&lastf, bx, by), 2)
            && close(
                [fwd_under[0], fwd_under[1], fwd_under[2]],
                px(&lastf, fx, fy),
                2,
            ),
        format!(
            "back = {:?} (want {tb:?}), forward spot = {:?} (want parchment {fwd_under:?})",
            px(&lastf, bx, by),
            px(&lastf, fx, fy)
        ),
    );

    // p8 — the page indicator is RIGHT-aligned. The book art has dark
    // decorative pixels of its own across the indicator row (a raw dark scan
    // reads the art — the first cut of this witness did), so the measurement
    // diffs two frames whose indicators differ only in the LAST digit
    // ("Page 1 of 3" vs "Page 1 of 1", equal widths): every differing pixel
    // is that digit, and a right-aligned indicator puts its right edge at the
    // anchor `left + 148`.
    let (anchor, ay) = BookViewScreen::indicator_right(GUI_W);
    let ind_diff = |a: &[u8], b: &[u8]| -> (i32, i32) {
        let (mut leftmost, mut rightmost) = (i32::MAX, i32::MIN);
        for gy in ay..ay + 9 {
            for gx in left..left + bv::IMAGE_W {
                if px(a, gx * SCALE, gy * SCALE) != px(b, gx * SCALE, gy * SCALE) {
                    leftmost = leftmost.min(gx);
                    rightmost = rightmost.max(gx);
                }
            }
        }
        (leftmost, rightmost)
    };
    let empty_book = BookViewScreen::new(Vec::new());
    let blank = shot(&mut gpu, &mut off, &mut wr, Some((&empty_book, None)), "blank")?;
    let (dl, dr) = ind_diff(&page0, &blank);
    c.record(
        "p8.the_indicator_right_edge_sits_at_the_anchor",
        dl != i32::MAX && dr <= anchor && dl >= anchor - 8,
        format!(
            "differing-digit span [{dl}, {dr}] against anchor {anchor} —              TextAlignment.RIGHT is anchor - width, so the varying last digit              ends AT the anchor"
        ),
    );

    // p7 — the page text lands black inside the 114-wide text box, and an
    // empty book draws none: count near-black pixels that differ from an
    // empty-BOOK frame (not the no-screen frame — the parchment must be the
    // constant behind both).
    let text_box = |f: &[u8]| -> usize {
        let (tx, ty) = BookViewScreen::text_origin(GUI_W);
        let mut n = 0;
        for gy in ty..ty + 128 {
            for gx in tx..tx + bv::TEXT_WIDTH {
                let p = px(f, gx * SCALE, gy * SCALE);
                if p[0] < 40 && p[1] < 40 && p[2] < 40 {
                    n += 1;
                }
            }
        }
        n
    };
    let (with_text, without) = (text_box(&page0), text_box(&blank));
    c.record(
        "p7.the_page_text_is_black_glyphs_in_the_text_box",
        with_text > 20 && without <= 2,
        format!("dark pixels: with text {with_text}, empty book {without}"),
    );


    // p9 — the Done button's chrome draws at menuControlsTop: the band
    // differs from the no-screen frame in many pixels (nine-sliced chrome +
    // the label).
    let done_y = bv::MENU_CONTROLS_TOP;
    let mut diff = 0;
    for gy in done_y..done_y + 20 {
        for gx in (GUI_W - 200) / 2..(GUI_W + 200) / 2 {
            if px(&page0, gx * SCALE, gy * SCALE) != px(&empty_frame, gx * SCALE, gy * SCALE) {
                diff += 1;
            }
        }
    }
    c.record(
        "p9.the_done_button_draws_at_menu_controls_top",
        diff > 400,
        format!("{diff} changed pixels in the 200x20 band at y {done_y} (= 2 + 192 + 2)"),
    );

    // p10 — the transparent gradient dims the world outside the book: not the
    // clear green, and darker than it.
    let outside = px(&page0, 10, 10);
    c.record(
        "p10.the_backdrop_gradient_dims_outside_the_book",
        outside != [0, 255, 0] && outside[1] < 200,
        format!(
            "corner = {outside:?} (0xC0101010 over green — isInGameUi is true, so ONLY the gradient, no blur, no tile)"
        ),
    );

    // p11 — the empty-book indicator still reads "Page 1 of 1":
    // `max(numPages, 1)` never shows 0. The digit cell that differs from
    // page0's "3" must hold dark glyph pixels in the BLANK frame too, over
    // book texels that are themselves light (so the darkness is a glyph, not
    // the art).
    let mut blank_digit_dark = 0;
    for gy in ay..ay + 9 {
        for gx in (anchor - 8).max(left)..anchor {
            let f = px(&blank, gx * SCALE, gy * SCALE);
            let t = texel(book_bg, (gx - left) as u32, (gy - bv::BACKGROUND_TOP) as u32);
            if f[0] < 40 && f[1] < 40 && f[2] < 40 && t[0] > 100 {
                blank_digit_dark += 1;
            }
        }
    }
    c.record(
        "p11.an_empty_book_still_shows_page_one_of_one",
        blank_digit_dark >= 2,
        format!(
            "{blank_digit_dark} glyph pixels in the last-digit cell over light              parchment — the indicator floors at max(count, 1)"
        ),
    );

    // p12 — the Done button carries its LABEL, not bare chrome: near-white
    // glyph pixels inside the button band. The eyeball pass found the first
    // cut drew an unlabeled button — p9 counts changed pixels and chrome
    // alone passes it, so the label needs its own witness.
    let mut label_px = 0;
    for gy in bv::MENU_CONTROLS_TOP + 4..bv::MENU_CONTROLS_TOP + 16 {
        for gx in (GUI_W - 200) / 2..(GUI_W + 200) / 2 {
            let p = px(&page0, gx * SCALE, gy * SCALE);
            if p[0] > 220 && p[1] > 220 && p[2] > 220 {
                label_px += 1;
            }
        }
    }
    c.record(
        "p12.the_done_button_is_labeled",
        label_px > 15,
        format!("{label_px} near-white glyph pixels in the button band"),
    );

    // p13 — a STYLED page keeps its colour all the way to the pixels: a red
    // span resolved through the production `resolve_book_pages` renders
    // reddish glyphs, not black ones. m1 checks the resolve; without this, a
    // text path that forced `color_linear` black would pass every witness.
    let styled = SlotText {
        has_written_book: true,
        book_pages: vec![rewo_proto::nbt::Nbt::Compound(vec![
            ("text".into(), rewo_proto::nbt::Nbt::String("crimson words".into())),
            ("color".into(), rewo_proto::nbt::Nbt::String("dark_red".into())),
        ])],
        ..Default::default()
    };
    let styled_pages = crate::live_cmd::resolve_book_pages(&styled, Some(&advance), Some(baked_lang))
        .ok_or("styled book did not resolve")?;
    let styled_book = BookViewScreen::new(styled_pages);
    let styled_frame = shot(&mut gpu, &mut off, &mut wr, Some((&styled_book, None)), "styled")?;
    let mut reddish = 0;
    {
        let (tx, ty) = BookViewScreen::text_origin(GUI_W);
        for gy in ty..ty + 12 {
            for gx in tx..tx + bv::TEXT_WIDTH {
                let p = px(&styled_frame, gx * SCALE, gy * SCALE);
                // DARK_RED is 0xAA0000: strongly red, near-zero green/blue.
                if p[0] > 100 && p[1] < 40 && p[2] < 40 {
                    reddish += 1;
                }
            }
        }
    }
    c.record(
        "p13.a_styled_page_renders_its_own_colour",
        reddish > 10,
        format!("{reddish} dark-red glyph pixels — mergeStyles lets the page's colour win"),
    );

    // p14 — `withoutShadow()`: the page text draws NO shadow. Undetectable on
    // black text (a black glyph's shadow is black too, and the +1 offset stays
    // inside the digit-diff tolerance — the battery's shadow mutation survived
    // every earlier witness), but a DARK_RED glyph's shadow is the quarter
    // tone `color_linear * 0.25`, which stores at roughly byte 89 red: count
    // those and require ZERO.
    let mut quarter_tone = 0;
    {
        let (tx, ty) = BookViewScreen::text_origin(GUI_W);
        for gy in ty..ty + 13 {
            for gx in tx..tx + bv::TEXT_WIDTH + 2 {
                let p = px(&styled_frame, gx * SCALE, gy * SCALE);
                if p[0] > 60 && p[0] < 100 && p[1] < 25 && p[2] < 25 {
                    quarter_tone += 1;
                }
            }
        }
    }
    c.record(
        "p14.the_page_text_casts_no_shadow",
        quarter_tone == 0,
        format!(
            "{quarter_tone} quarter-tone red pixels (a shadow would put the              0.25-darkened copy at +1,+1 — PAGE_TEXT_STYLE is withoutShadow())"
        ),
    );

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

/// The first fully-opaque texel of a sprite, scanning row-major — the arrows
/// have transparent margins, so a fixed corner probe would land on nothing.
fn opaque_texel(s: &rewo_data::assets::HudSprite) -> Option<(u32, u32)> {
    for dy in 0..s.h {
        for dx in 0..s.w {
            if s.rgba[((dy * s.w + dx) * 4 + 3) as usize] == 255 {
                return Some((dx, dy));
            }
        }
    }
    None
}

/// A texel where BOTH sprites are opaque and their bytes differ — what makes
/// the hover witness able to tell the two apart at all.
fn differing_opaque_texel(
    a: &rewo_data::assets::HudSprite,
    b: &rewo_data::assets::HudSprite,
) -> Option<(u32, u32)> {
    for dy in 0..a.h.min(b.h) {
        for dx in 0..a.w.min(b.w) {
            let i = ((dy * a.w + dx) * 4) as usize;
            let j = ((dy * b.w + dx) * 4) as usize;
            if a.rgba[i + 3] == 255
                && b.rgba[j + 3] == 255
                && a.rgba[i..i + 3] != b.rgba[j..j + 3]
            {
                return Some((dx, dy));
            }
        }
    }
    None
}
