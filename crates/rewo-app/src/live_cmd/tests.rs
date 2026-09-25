// -- the ghost recipe (M103) ---------------------------------------------

// -- the chat box (M108) -------------------------------------------------

/// Six lines of chat and the geometry they land on. Shared by the
/// witnesses below so each asserts one property against one fixture.
fn chat_fixture(count: usize, added_time: i32) -> rewo_world::chat::ChatComponent {
    let w6 = |s: &str, st: rewo_world::chat_style::ChatStyle| {
        s.chars().count() as i32 * (6 + i32::from(st.bold))
    };
    let ctx = rewo_world::chat::WrapContext {
        options: rewo_world::chat::ChatOptions::default(),
        focused: false,
        width_of: &w6,
        deleted_marker_text: "deleted",
    };
    let mut c = rewo_world::chat::ChatComponent::new();
    for i in 0..count {
        c.add_message(
            rewo_world::chat::GuiMessage {
                added_time,
                content: vec![
                    rewo_world::chat_style::ChatStyle::WHITE.span(format!("m{i}")),
                ],
                signature: None,
                source: rewo_world::chat::MessageSource::SystemServer,
                tag: None,
            },
            &ctx,
        );
    }
    c
}

/// The bottom row sits `entryBottomToMessageY` above `chatBottom`, and
/// `chatBottom` is `floor((screenHeight - 40) / scale)` — measured in
/// **chat** pixels, then scaled back to screen pixels.
///
/// At the defaults (GUI px 1, chat scale 1, 720 px tall) that is
/// `720 - 40 = 680`, minus 8, so the bottom line's top edge is 672.
#[test]
fn the_bottom_chat_row_sits_forty_pixels_off_the_bottom_less_the_baseline() {
    let opts = rewo_world::chat::ChatOptions::default();
    let lines = super::chat_lines(&chat_fixture(1, 0), 0, 1.0, 720.0, &opts, None, false).0;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].y, 672.0);
    // `pose.translate(4, 0)` — not 0, and not the F3 block's 3.
    assert_eq!(lines[0].x, 4.0);
}

/// Rows are one `entryHeight` apart and stack **upward**, newest at the
/// bottom. Both ends are asserted: the top row alone cannot see a
/// reversed stack, and the count alone cannot see a wrong pitch.
#[test]
fn chat_rows_stack_upward_one_entry_height_apart() {
    let opts = rewo_world::chat::ChatOptions::default();
    let lines = super::chat_lines(&chat_fixture(3, 0), 0, 1.0, 720.0, &opts, None, false).0;
    assert_eq!(lines.len(), 3);
    // `visible_lines` emits top-first, so index 0 here is the oldest and
    // highest.
    assert_eq!((lines[0].text.as_str(), lines[0].y), ("m0", 672.0 - 18.0));
    assert_eq!((lines[1].text.as_str(), lines[1].y), ("m1", 672.0 - 9.0));
    assert_eq!((lines[2].text.as_str(), lines[2].y), ("m2", 672.0));
}

/// The baseline offset is `entryBottomToMessageY`, whose two terms move in
/// opposite directions with the line spacing.
///
/// **This needs a non-default spacing to say anything.** At spacing 0 the
/// row is 9 tall and the offset is 8, so `entryHeight - 1` — the obvious
/// wrong reading — gives the same answer; every other witness here uses
/// the defaults and a mutation to it survived them all. At spacing 1 the
/// row is 18 and the offset is `round(8*2 - 4*1) = 12`, where
/// `entryHeight - 1` would be 17.
#[test]
fn the_baseline_offset_is_not_the_row_height_less_one() {
    let mut opts = rewo_world::chat::ChatOptions::default();
    opts.line_spacing = 1.0;
    assert_eq!(opts.entry_height(), 18);
    let lines = super::chat_lines(&chat_fixture(2, 0), 0, 1.0, 720.0, &opts, None, false).0;
    // chatBottom 680, minus 12.
    assert_eq!(lines[1].y, 668.0);
    // …and the pitch is the row height, so the two readings cannot be
    // confused by the gap either.
    assert_eq!(lines[0].y, 668.0 - 18.0);
}

/// The backdrop's rect is asymmetric about the text: the pose is
/// translated by 4 before `fill(-4, …, maxWidth + 4 + 4, …)`, so the left
/// edge lands at absolute 0 and the right at `maxWidth + 12`.
#[test]
fn the_backdrop_is_asymmetric_about_the_text() {
    let opts = rewo_world::chat::ChatOptions::default();
    let b = super::hud_fills(&chat_fixture(1, 0), 0, 1.0, 720.0, &opts, false);
    assert_eq!(b.len(), 1);
    // Four pixels left of the text, which sits at 4.
    assert_eq!(b[0].x, 0.0);
    // …and eight past where a full-width line can reach, not four.
    assert_eq!(b[0].w, 320.0 + 12.0);
    // Centring it — the tidy reading — would put the left edge at -6.
    assert_ne!(b[0].x, -6.0);
}

/// A row's fill spans its whole `entryHeight` and sits directly under the
/// row's text, so the two derivations agree about where a row is.
#[test]
fn each_fill_covers_its_own_row_and_meets_the_next() {
    let opts = rewo_world::chat::ChatOptions::default();
    let c = chat_fixture(3, 0);
    let b = super::hud_fills(&c, 0, 1.0, 720.0, &opts, false);
    let l = super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0;
    assert_eq!(b.len(), l.len());
    for i in 0..b.len() {
        assert_eq!(b[i].h, 9.0);
        // The text's top edge is inside its own fill.
        assert!(b[i].y <= l[i].y && l[i].y < b[i].y + b[i].h, "row {i}");
    }
    // Rows tile with no gap: each fill's bottom is the next one's top.
    for i in 1..b.len() {
        assert_eq!(b[i - 1].y + b[i - 1].h, b[i].y);
    }
}

// ── M135: the fills are in GUI pixels, and land where their text is ─────
//
// Everything below exists because the witness one function up CANNOT see
// the bug M135 fixed, and the reason generalises.
//
// [`rewo_gpu::hud::HudFill`] is in **GUI** pixels — `HudPass::draw` puts
// every rect through `tinted_quad`, which multiplies by
// [`rewo_gpu::hud::gui_scale`]. [`rewo_gpu::world::OwnedTextLine`], one
// pass over, is already in **screen** pixels. Four producers emitted screen
// pixels into the GUI-pixel field, so their fills were drawn at `gui_scale`
// times their true depth.
//
// `each_fill_covers_its_own_row_and_meets_the_next` compares the two
// producers' RAW outputs, and both sides were then in the same wrong space
// and agreed with each other perfectly. **An agreement witness has to model
// whatever sits between the producer and the screen**, or it grades the two
// derivations against each other rather than against the frame.

/// The pass's own rule, applied to a fill so a witness can ask where it
/// LANDS rather than what was emitted. This is `tinted_quad`'s
/// `(x * scale, y * scale, qw * scale, qh * scale)`.
fn on_screen(f: &rewo_gpu::hud::HudFill, px: f32) -> (f32, f32, f32, f32) {
    (f.x * px, f.y * px, f.w * px, f.h * px)
}

/// 960x720 gives `min(720/240, 960/320) = 3`, so these run at the GUI scale
/// `live --render-check` uses and every older fixture avoids.
const PX: f32 = 3.0;
const SCREEN_W: f32 = 960.0;
const SCREEN_H: f32 = 720.0;

/// The GUI scale these fixtures assume is the one the pass will apply — not
/// a number chosen to make them pass.
#[test]
fn the_fixture_scale_is_the_scale_the_pass_computes() {
    assert_eq!(rewo_gpu::hud::gui_scale(SCREEN_W, SCREEN_H), PX);
    // …and the app's own helper is that same function, not a fourth copy.
    assert_eq!(super::gui_px(SCREEN_W as u32, SCREEN_H as u32), PX);
}

/// A chat row's backdrop lands under its own text at a GUI scale above 1.
///
/// Grounded on `chat_lines`, an independent producer that `--render-check`'s
/// r26/r38 exercise against a real server, so this is an agreement witness
/// and not a restatement of `hud_fills`' own arithmetic.
///
/// **The fixture separates the two factors on purpose.** The GUI scale is 3
/// and the chat scale is 2, which makes all three candidate readings
/// distinct: a chat pixel is 2 GUI pixels (right), 6 (multiplying by the GUI
/// scale a second time — the bug), or 1 (dropping the chat scale along with
/// it — the over-correction). Every fixture written before this one uses
/// px 1 and chat scale 1, the single point where all three agree, which is
/// why eight milestones of green tests never contradicted the bug.
#[test]
fn a_row_backdrop_lands_under_its_text_at_gui_scale_three() {
    let mut opts = rewo_world::chat::ChatOptions::default();
    opts.scale = 2.0;
    let c = chat_fixture(3, 0);
    let fills = super::hud_fills(&c, 0, PX, SCREEN_H, &opts, false);
    let lines = super::chat_lines(&c, 0, PX, SCREEN_H, &opts, None, false).0;
    assert_eq!(fills.len(), lines.len());
    assert!(!fills.is_empty(), "the fixture drew no rows");
    for i in 0..fills.len() {
        let (_, y, _, h) = on_screen(&fills[i], PX);
        assert!(
            y <= lines[i].y && lines[i].y < y + h,
            "row {i}: its text is at {} but its backdrop covers {}..{}",
            lines[i].y,
            y,
            y + h
        );
    }
}

/// …and the visible symptom: they are on the screen at all.
///
/// Three times too far down puts them off the bottom, so the fills were
/// **absent rather than misplaced** — which is why nobody reported them in
/// the eight milestones since M109. Covers the scrollbar too, which has no
/// text of its own to agree with.
#[test]
fn every_chat_fill_is_on_the_screen_at_gui_scale_three() {
    let opts = rewo_world::chat::ChatOptions::default();
    // Focused, and long enough that the backlog outruns the box — otherwise
    // the scrollbar is empty and this witnesses a path it never enters.
    let c = chat_fixture(30, 0);
    let mut all = super::hud_fills(&c, 0, PX, SCREEN_H, &opts, true);
    let rows = all.len();
    let bar = super::chat_scrollbar(&c, 0, PX, SCREEN_H, &opts);
    assert!(rows > 0, "no rows");
    assert!(!bar.is_empty(), "no scrollbar — the fixture cannot scroll");
    all.extend(bar);
    for (i, f) in all.iter().enumerate() {
        let (x, y, w, h) = on_screen(f, PX);
        assert!(
            y >= 0.0 && y + h <= SCREEN_H,
            "fill {i} spans y {y}..{} on a {SCREEN_H}-tall screen",
            y + h
        );
        // **The horizontal bound is not the screen width, and that is
        // vanilla's doing rather than a slack threshold.** `getWidth()` is
        // `chatWidth * 280 + 40` — 320 at the default, computed with no
        // reference to the screen — and the backdrop is `maxWidth + 12`
        // wide. At this fixture's GUI width of exactly 320 the box already
        // fills the screen, so the row fills genuinely run 12 GUI pixels
        // past the right edge and are clipped there. A wider fixture would
        // hide that rather than settle it, and asserting `<= SCREEN_W`
        // would assert something vanilla does not do. The bound still
        // catches a scale error, which is three times over.
        const OVERHANG_GUI: f32 = 12.0;
        assert!(
            x >= 0.0 && x + w <= SCREEN_W + OVERHANG_GUI * PX,
            "fill {i} spans x {x}..{} on a {SCREEN_W}-wide screen",
            x + w
        );
    }
}

/// The input bar sits behind the text it is a backdrop for.
///
/// `input_backdrop_rect` is handed GUI dimensions and returns GUI pixels, so
/// the fill is that rect unscaled; `chat_input_lines` is the screen-pixel
/// sibling. Multiplying the rect by the GUI scale put the bar at y 2034 on a
/// 720-tall screen.
#[test]
fn the_input_backdrop_lands_under_its_text_at_gui_scale_three() {
    use rewo_world::chat_screen::{ChatMethod, ChatScreen};
    let (gw, gh) = ((SCREEN_W / PX) as i32, (SCREEN_H / PX) as i32);
    let mut s = ChatScreen::open(ChatMethod::Message, None, 0);
    s.char_typed('h', &chat_env());
    let text = super::chat_input_lines(&s, PX, gw, gh, 0, None, &|t: &str| {
        t.chars().count() as i32 * 6
    });
    let (_, y, _, h) = on_screen(&super::chat_input_backdrop(gw, gh), PX);
    assert!(
        y <= text[0].y && text[0].y < y + h,
        "the input text is at {} but its bar covers {}..{}",
        text[0].y,
        y,
        y + h
    );
}

/// Each suggestion row's fill sits behind that row's text.
///
/// `SuggestionsList::rect` is GUI pixels — which
/// `the_popup_fills_one_rect_per_visible_row` already asserts by comparing
/// against `list.rect` directly, and could not enforce, because it runs at
/// px 1 where the scaled and unscaled readings are the same number.
#[test]
fn each_popup_row_lands_under_its_text_at_gui_scale_three() {
    let (s, _) = popup(3);
    let cs = &s.suggestions;
    let list = cs.list().expect("the popup opened");
    let fills = super::suggestion_popup_fills(list, cs.config());
    let text = super::suggestion_popup_text(list, cs.config(), PX);
    assert_eq!(fills.len(), text.len());
    assert!(!fills.is_empty());
    for i in 0..fills.len() {
        let (_, y, _, h) = on_screen(&fills[i], PX);
        assert!(
            y <= text[i].y && text[i].y < y + h,
            "row {i}: its text is at {} but its fill covers {}..{}",
            text[i].y,
            y,
            y + h
        );
    }
}

/// The fill's alpha is `alpha * backgroundOpacity`, and the text's is
/// `alpha * (chatOpacity * 0.9 + 0.1)` — **different multipliers**. At
/// "Text Background: 0" the fill vanishes while the text stays visible.
#[test]
fn the_fill_and_the_text_use_different_opacity_multipliers() {
    let mut opts = rewo_world::chat::ChatOptions::default();
    let c = chat_fixture(1, 0);
    // Default: background 0.5, text 1.0.
    assert_eq!(super::hud_fills(&c, 0, 1.0, 720.0, &opts, false)[0].alpha, 0.5);
    assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0[0].alpha, 1.0);
    opts.text_background_opacity = 0.0;
    assert_eq!(super::hud_fills(&c, 0, 1.0, 720.0, &opts, false)[0].alpha, 0.0);
    // The text is untouched by it — a shared multiplier would zero both.
    assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0[0].alpha, 1.0);
}

/// The fade reaches the fill too, so a message dims its own backdrop with
/// it rather than leaving a black bar behind.
#[test]
fn the_fade_reaches_the_backdrop() {
    let opts = rewo_world::chat::ChatOptions::default();
    let c = chat_fixture(1, 0);
    // 190 ticks: half through the fade, squared -> 0.25, times bg 0.5.
    let b = super::hud_fills(&c, 190, 1.0, 720.0, &opts, false);
    assert!((b[0].alpha - 0.125).abs() < 1e-5);
    // Fully faded rows are not emitted at all.
    assert!(super::hud_fills(&c, 200, 1.0, 720.0, &opts, false).is_empty());
}

/// `maxWidth` is a CEIL here and a FLOOR in the wrap budget. They agree at
/// scale 1 and differ the moment the division is not exact.
#[test]
fn the_backdrops_max_width_is_a_ceil_not_the_wraps_floor() {
    let mut opts = rewo_world::chat::ChatOptions::default();
    opts.scale = 0.75;
    // 320 / 0.75 = 426.66… -> ceil 427, floor 426.
    let b = super::hud_fills(&chat_fixture(1, 0), 0, 1.0, 720.0, &opts, false);
    let chat_px = 0.75_f32;
    assert!(((b[0].w / chat_px) - (427.0 + 12.0)).abs() < 1e-3, "{}", b[0].w / chat_px);
}

/// The GUI scale multiplies every offset, including the 40-px margin —
/// which is why `chatBottom` divides by the scale before the subtraction
/// rather than after. At px 2 the box is 40 *chat* pixels off the bottom,
/// i.e. 80 screen pixels, not 40.
#[test]
fn the_gui_scale_multiplies_the_whole_box() {
    let opts = rewo_world::chat::ChatOptions::default();
    let lines = super::chat_lines(&chat_fixture(1, 0), 0, 2.0, 720.0, &opts, None, false).0;
    assert_eq!(lines[0].px, 2.0);
    assert_eq!(lines[0].x, 8.0);
    // floor(720/2 - 40) = 320 chat px, minus 8, times 2.
    assert_eq!(lines[0].y, 624.0);
}

/// The fade reaches the line's alpha, and is multiplied by
/// `chatOpacity * 0.9 + 0.1` — so a fully faded line is gone but a fresh
/// one is not dimmed at the default opacity of 1.
#[test]
fn the_fade_and_the_text_opacity_both_reach_the_line() {
    let opts = rewo_world::chat::ChatOptions::default();
    let c = chat_fixture(1, 0);
    assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0[0].alpha, 1.0);
    // 190 ticks: half way through the 20-tick fade, squared -> 0.25.
    let faded = super::chat_lines(&c, 190, 1.0, 720.0, &opts, None, false).0;
    assert!((faded[0].alpha - 0.25).abs() < 1e-5);
    // Past 200 the line is not emitted at all, rather than emitted at 0.
    assert!(super::chat_lines(&c, 200, 1.0, 720.0, &opts, None, false).0.is_empty());

    let mut dim = rewo_world::chat::ChatOptions::default();
    dim.opacity = 0.0;
    let lines = super::chat_lines(&c, 0, 1.0, 720.0, &dim, None, false).0;
    assert!((lines[0].alpha - 0.1).abs() < 1e-6, "the floor is 0.1, not 0");
}

/// The unfocused box holds ten rows (90 / 9), so an eleventh message
/// pushes the oldest off the top rather than growing the box.
#[test]
fn the_unfocused_box_holds_ten_rows() {
    let opts = rewo_world::chat::ChatOptions::default();
    let lines = super::chat_lines(&chat_fixture(30, 0), 0, 1.0, 720.0, &opts, None, false).0;
    assert_eq!(lines.len(), 10);
    assert_eq!(lines.last().unwrap().text, "m29");
    assert_eq!(lines.first().unwrap().text, "m20");
}

/// The whole point of threading `focused` through (M110): with the chat
/// screen open the box is TALLER and the fade is off.
///
/// Hardcoding `false` — which is what these two derivations did until the
/// screen existed — leaves the focused view showing ten rows of twenty and
/// fading messages out from under someone who is reading them.
#[test]
fn the_focused_box_is_taller_and_does_not_fade() {
    let opts = rewo_world::chat::ChatOptions::default();
    let c = chat_fixture(30, 0);
    assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, false).0.len(), 10);
    assert_eq!(super::chat_lines(&c, 0, 1.0, 720.0, &opts, None, true).0.len(), 20);
    // …and the fills follow the text, so a taller box does not draw ten
    // rows of glyphs over twenty rows of backdrop.
    assert_eq!(super::hud_fills(&c, 0, 1.0, 720.0, &opts, false).len(), 10);
    assert_eq!(super::hud_fills(&c, 0, 1.0, 720.0, &opts, true).len(), 20);

    // At 300 ticks every message is long faded, and the focused view shows
    // them anyway — `AlphaCalculator.FULLY_VISIBLE` rather than
    // `timeBased`.
    assert!(super::chat_lines(&c, 300, 1.0, 720.0, &opts, None, false).0.is_empty());
    let focused = super::chat_lines(&c, 300, 1.0, 720.0, &opts, None, true).0;
    assert_eq!(focused.len(), 20);
    assert!(focused.iter().all(|l| l.alpha == 1.0));
}

/// **The bug M112 fixes**: `ScreenState::hovered` converted through
/// `Placement::centred` while the render converts through
/// `Placement::with_book`, so with the book open the two disagree by the
/// 77 GUI px `updateScreenPosition` moves the panel.
///
/// Written before the fix and left as the regression guard. M89 and M106b
/// each recorded "a per-call-site choice is how they come to disagree";
/// this is the third time, and it reached the CLICK rather than a tooltip.
#[test]
fn the_hover_and_the_render_agree_about_where_a_slot_is() {
    let layout = &rewo_world::menu_layout::PLAYER;
    // A window wide enough for the book to displace the panel: 1280 at GUI
    // scale 4 is 320 GUI px, which is NARROW, so pick one that is not.
    let (w, h) = (1920.0f32, 1080.0f32);
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let gui_w = w / scale;
    assert!(
        gui_w >= rewo_world::recipe_book_screen::WIDTH_TOO_NARROW_BELOW as f32,
        "the fixture must be a WIDE window or the displacement is 0 and              this test cannot see anything ({gui_w} GUI px)"
    );
    // The cursor over the centre of the panel as the book-open render
    // places it.
    let (ox, _oy, _s) = rewo_gpu::container::gui_origin_placed(
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            true,
        ),
    );
    // Slot 9's own position, converted forward to a screen point.
    let (sx, sy) = layout.position(9).unwrap();
    let mouse = (
        (ox + (sx as f32 + 8.0) * scale) as f64,
        {
            let (_, oy2, _) = rewo_gpu::container::gui_origin_placed(
                w,
                h,
                rewo_gpu::container::Placement::with_book(
                    layout.image_w as f32,
                    layout.image_h as f32,
                    true,
                ),
            );
            (oy2 + (sy as f32 + 8.0) * scale) as f64
        },
    );
    let mut screen = super::ScreenState::default();
    screen.mouse = mouse;
    assert_eq!(
        screen.hovered(layout, w, h, true),
        Some(9),
        "the hover must resolve the slot the render drew under the cursor"
    );
    // …and with the book SHUT the same screen point is a different slot,
    // which is what makes the parameter load-bearing rather than cosmetic.
    assert_ne!(screen.hovered(layout, w, h, false), Some(9));
}

/// A menu with no recipe book is never displaced and never suppressed.
///
/// The witness that did not exist while this rule lived inside a
/// `PlaySession`-taking function: a mutation answering `true` for a
/// bookless menu survived the whole suite, and it would shift a chest's
/// panel 77 px and blank its hover on a narrow window.
#[test]
fn a_menu_with_no_book_is_never_displaced() {
    use rewo_world::recipe_book_screen::BookType;
    let mut open = rewo_net::recipe_book::BookSettings::default();
    open.crafting.open = true;
    open.furnace.open = true;
    open.blast_furnace.open = true;
    open.smoker.open = true;
    assert!(!super::book_visible_for(None, &open), "a chest has no book");
    // …and every type that DOES have one reads its own flag, so this is
    // not "always false".
    for b in [
        BookType::Crafting,
        BookType::Furnace,
        BookType::BlastFurnace,
        BookType::Smoker,
    ] {
        assert!(super::book_visible_for(Some(b), &open), "{b:?}");
        assert!(
            !super::book_visible_for(
                Some(b),
                &rewo_net::recipe_book::BookSettings::default()
            ),
            "{b:?} shut"
        );
    }
}

/// Each book type reads **its own** flag, not another's — four settings
/// that a single shared bool would collapse.
#[test]
fn each_book_type_reads_its_own_flag() {
    use rewo_world::recipe_book_screen::BookType;
    let mut only_furnace = rewo_net::recipe_book::BookSettings::default();
    only_furnace.furnace.open = true;
    assert!(super::book_visible_for(Some(BookType::Furnace), &only_furnace));
    assert!(!super::book_visible_for(Some(BookType::Crafting), &only_furnace));
    assert!(!super::book_visible_for(Some(BookType::Smoker), &only_furnace));
    assert!(!super::book_visible_for(
        Some(BookType::BlastFurnace),
        &only_furnace
    ));
}

/// `isHovering`'s narrow-window override: under 379 GUI px the book covers
/// the menu, and vanilla answers "no slot" for every slot rather than
/// letting a click reach through the panel on top of it.
#[test]
fn a_narrow_window_with_the_book_open_hovers_nothing() {
    let layout = &rewo_world::menu_layout::PLAYER;
    // 1280x720 at GUI scale 3 is 426 GUI px — wide. 1280x480 gives scale 2
    // and 640, also wide. A genuinely narrow one needs a small window.
    let (w, h) = (1024.0f32, 768.0f32);
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let gui_w = (w / scale) as i32;
    assert!(
        rewo_world::recipe_book_screen::width_too_narrow(gui_w),
        "the fixture must be NARROW or this test cannot see the override              ({gui_w} GUI px against {})",
        rewo_world::recipe_book_screen::WIDTH_TOO_NARROW_BELOW
    );
    // Dead centre of the panel — unambiguously over a slot with the book
    // shut, which is the control that makes the `None` mean the override
    // and not a cursor that simply missed.
    let mouse = (w as f64 / 2.0, h as f64 / 2.0);
    assert!(
        super::hovered_menu_slot(layout, mouse, w, h, false).is_some(),
        "the control: with the book shut this point IS over a slot"
    );
    assert_eq!(
        super::hovered_menu_slot(layout, mouse, w, h, true),
        None,
        "and with it open on a narrow window, nothing is hovered"
    );
}

/// …and on a WIDE window the book displaces the panel instead of covering
/// it, so slots stay hoverable. Without this partner the override could be
/// "the book always suppresses the hover", which is a different rule.
#[test]
fn a_wide_window_with_the_book_open_still_hovers() {
    let layout = &rewo_world::menu_layout::PLAYER;
    let (w, h) = (1920.0f32, 1080.0f32);
    let scale = rewo_gpu::hud::gui_scale(w, h);
    assert!(!rewo_world::recipe_book_screen::width_too_narrow((w / scale) as i32));
    let (ox, oy, _) = rewo_gpu::container::gui_origin_placed(
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            true,
        ),
    );
    let (sx, sy) = layout.position(9).unwrap();
    let mouse = (
        (ox + (sx as f32 + 8.0) * scale) as f64,
        (oy + (sy as f32 + 8.0) * scale) as f64,
    );
    assert_eq!(super::hovered_menu_slot(layout, mouse, w, h, true), Some(9));
}

/// The scrollbar reaches the frame's fill list, in GUI pixels, and its
/// colour is converted OUT of sRGB.
///
/// It said "screen pixels" until M135, which is the unit the TEXT pass one
/// function over takes; the fill list is in GUI pixels and the pass scales
/// it. The comment was a symptom of the same confusion as the code.
///
/// The conversion is the half that hides: black is 0 in both spaces, so
/// M109 and M110's fills could pass their bytes straight through and this
/// is the first fill whose colour is not black.
#[test]
fn the_scrollbar_reaches_the_frame_in_linear_colour() {
    let opts = rewo_world::chat::ChatOptions::default();
    let c = chat_fixture(30, 0);
    let bars = super::chat_scrollbar(&c, 0, 1.0, 720.0, &opts);
    assert_eq!(bars.len(), 2, "a body and a highlight");
    // `scrollBarStartX = maxWidth + 4`, plus the pose's MESSAGE_INDENT.
    assert_eq!(bars[0].x, 328.0);
    assert_eq!((bars[0].w, bars[1].w), (2.0, 1.0));
    assert_eq!(bars[1].x, 329.0);
    // 96/255.
    assert!((bars[0].alpha - 96.0 / 255.0).abs() < 1e-6);
    // 0x3333AA's blue channel decoded: (0xAA/255 + 0.055)/1.055 ^ 2.4.
    let expect_b = ((0xAA as f32 / 255.0 + 0.055) / 1.055).powf(2.4);
    assert!((bars[0].rgb[2] - expect_b).abs() < 1e-5);
    // …and NOT the raw byte, which is what a pass-through would give.
    assert!((bars[0].rgb[2] - 0xAA as f32 / 255.0).abs() > 0.1);
    // The highlight is a light grey, brighter than the body in every
    // channel — a swap would be invisible to a single-channel check.
    for i in 0..3 {
        assert!(bars[1].rgb[i] > bars[0].rgb[i], "channel {i}");
    }
}

/// Nothing to scroll, nothing drawn — and the fills list is otherwise
/// unchanged, so a short chat does not gain a stray rect.
#[test]
fn a_chat_that_fits_draws_no_scrollbar() {
    let opts = rewo_world::chat::ChatOptions::default();
    assert!(super::chat_scrollbar(&chat_fixture(5, 0), 0, 1.0, 720.0, &opts).is_empty());
    assert!(super::chat_scrollbar(
        &rewo_world::chat::ChatComponent::new(),
        0,
        1.0,
        720.0,
        &opts
    )
    .is_empty());
}

/// A suggestion environment with no completion words, so typing into a
/// chat screen in these tests never opens a popup that would swallow the
/// next key. The popup has its own tests in `rewo_world`.
fn chat_env() -> rewo_world::chat_screen::SuggestionEnv<'static> {
    fn zero(_: &str) -> i32 {
        0
    }
    const NO_WORDS: &[String] = &[];
    rewo_world::chat_screen::SuggestionEnv {
        metrics: rewo_world::command_suggestions::InputMetrics {
            x: 4,
            inner_width: 316,
            screen_height: 240,
        },
        width: &zero,
        tab_words: NO_WORDS,
        auto_suggestions: true,
    }
}

/// A popup over `n` entries, opened through the production path.
fn popup(n: usize) -> (rewo_world::chat_screen::ChatScreen, Vec<String>) {
    use rewo_world::chat_screen::{ChatMethod, ChatScreen};
    let words: Vec<String> = (0..n).map(|i| format!("rewo{i:02}")).collect();
    let mut s = ChatScreen::open(ChatMethod::Message, None, 0);
    fn six(t: &str) -> i32 {
        t.encode_utf16().count() as i32 * 6
    }
    let env = rewo_world::chat_screen::SuggestionEnv {
        metrics: rewo_world::command_suggestions::InputMetrics {
            x: 4,
            inner_width: 316,
            screen_height: 240,
        },
        width: &six,
        tab_words: &words,
        auto_suggestions: true,
    };
    s.char_typed('r', &env);
    (s, words)
}

#[test]
fn the_popup_fills_one_rect_per_visible_row() {
    let (s, _) = popup(3);
    let cs = &s.suggestions;
    let list = cs.list().expect("the popup opened");
    let fills = super::suggestion_popup_fills(list, cs.config());
    // Three rows, no truncation bars.
    assert_eq!(fills.len(), 3);
    for (i, f) in fills.iter().enumerate() {
        assert_eq!(f.x, list.rect.x as f32);
        assert_eq!(f.y, (list.rect.y + 12 * i as i32) as f32);
        assert_eq!((f.w, f.h), (list.rect.w as f32, 12.0));
        // `-805306368` — alpha 208 over black.
        assert!((f.alpha - 208.0 / 255.0).abs() < 1e-6);
        assert_eq!(f.rgb, [0.0, 0.0, 0.0]);
    }
}

#[test]
fn a_list_longer_than_its_window_gains_two_bars_and_one_row_of_dashes() {
    // `limited` is `hasPrevious || hasNext` and gates BOTH bars, so a list
    // scrolled to its top still gets one above it — while the dashes are
    // per-end and only the bottom has any here.
    let (s, _) = popup(25);
    let cs = &s.suggestions;
    let list = cs.list().expect("the popup opened");
    assert_eq!(list.offset(), 0, "fixture precondition: at the top");
    let fills = super::suggestion_popup_fills(list, cs.config());
    let rows = 10;
    let dashes = (list.rect.w as usize).div_ceil(2);
    assert_eq!(fills.len(), 2 + dashes + rows);
    // The two bars are one pixel tall and hug the rect.
    assert_eq!((fills[0].y, fills[0].h), ((list.rect.y - 1) as f32, 1.0));
    assert_eq!(
        (fills[1].y, fills[1].h),
        ((list.rect.y + list.rect.h) as f32, 1.0)
    );
    // Every dash is white, one pixel wide, two apart.
    for (k, f) in fills[2..2 + dashes].iter().enumerate() {
        assert_eq!(f.x, (list.rect.x + 2 * k as i32) as f32);
        assert_eq!((f.w, f.h), (1.0, 1.0));
        assert_eq!(f.rgb, [1.0, 1.0, 1.0]);
        assert_eq!(f.y, (list.rect.y + list.rect.h) as f32);
    }
}

#[test]
fn a_short_list_gets_no_bars_at_all() {
    let (s, _) = popup(4);
    let cs = &s.suggestions;
    let list = cs.list().unwrap();
    let fills = super::suggestion_popup_fills(list, cs.config());
    assert!(fills.iter().all(|f| f.h == 12.0), "rows only");
}

#[test]
fn the_selected_row_is_yellow_and_the_rest_are_grey() {
    let (s, _) = popup(3);
    let cs = &s.suggestions;
    let list = cs.list().unwrap();
    let text = super::suggestion_popup_text(list, cs.config(), 1.0);
    assert_eq!(text.len(), 3);
    for (i, t) in text.iter().enumerate() {
        assert_eq!(t.x, (list.rect.x + 1) as f32);
        assert_eq!(t.y, (list.rect.y + 2 + 12 * i as i32) as f32);
        assert!(t.shadow, "the five-argument graphics.text drops one");
    }
    // `-256` is 0xFFFF00: red and green at full, blue at nothing.
    assert_eq!(text[0].color_linear[0], 1.0);
    assert_eq!(text[0].color_linear[1], 1.0);
    assert_eq!(text[0].color_linear[2], 0.0);
    // `-5592406` is 0xAAAAAA: neutral, and not full.
    assert_eq!(text[1].color_linear[0], text[1].color_linear[1]);
    assert_eq!(text[1].color_linear[1], text[1].color_linear[2]);
    assert!(text[1].color_linear[0] > 0.0 && text[1].color_linear[0] < 1.0);
}

#[test]
fn the_rows_follow_the_scroll_offset() {
    let (mut s, _) = popup(25);
    let first = super::suggestion_popup_text(
        s.suggestions.list().unwrap(),
        s.suggestions.config(),
        1.0,
    )[0]
    .text
    .clone();
    s.suggestions.mouse_scrolled(-1.0, {
        let r = s.suggestions.list().unwrap().rect;
        (r.x + 2, r.y + 2)
    });
    let scrolled = super::suggestion_popup_text(
        s.suggestions.list().unwrap(),
        s.suggestions.config(),
        1.0,
    )[0]
    .text
    .clone();
    assert_ne!(first, scrolled, "the top row moved with the offset");
}

#[test]
fn the_coloured_runs_butt_up_against_each_other_at_measured_widths() {
    // The runs must abut exactly: a wrong width is a visible gap or an
    // overlap, where for the caret a fixed six pixels was only ever a
    // pixel or two of drift. The width function here is deliberately NOT
    // six-per-character, so a run laid out on the constant lands
    // somewhere else.
    use rewo_net::command_format::{Run, ARGUMENT_COLORS, GRAY};
    use rewo_world::chat_screen::{ChatMethod, ChatScreen};
    let mut s = ChatScreen::open(ChatMethod::Command, None, 0);
    s.input.set_value("/give 5");
    let runs = vec![
        Run { text: "/give ".into(), color: GRAY },
        Run { text: "5".into(), color: ARGUMENT_COLORS[0] },
    ];
    let width_of = |t: &str| t.chars().count() as i32 * 4;
    let lines = super::chat_input_lines(&s, 1.0, 320, 240, 0, Some(&runs), &width_of);
    assert_eq!(lines[0].text, "/give ");
    assert_eq!(lines[1].text, "5");
    assert_eq!(lines[1].x - lines[0].x, 6.0 * 4.0);
    // The two runs carry different colours, which is the whole point.
    assert_ne!(lines[0].color_linear, lines[1].color_linear);
    // And the caret sits after the WHOLE value, measured the same way.
    let caret = lines.iter().find(|l| l.text == "_").expect("a caret");
    assert_eq!(caret.x - lines[0].x, 7.0 * 4.0);
}

#[test]
fn with_no_runs_the_field_is_one_flat_line() {
    // `formatChat` returns null before there is a parse, and an ordinary
    // chat message never gets one.
    use rewo_world::chat_screen::{ChatMethod, ChatScreen};
    let mut s = ChatScreen::open(ChatMethod::Message, None, 0);
    s.input.set_value("hello there");
    let lines = super::chat_input_lines(&s, 1.0, 320, 240, 0, None, &|t: &str| {
        t.chars().count() as i32 * 6
    });
    assert_eq!(lines[0].text, "hello there");
}

/// A `USAGE_FORMAT`-styled line, which is what `fillNodeUsage` produces.
fn gray(text: &str) -> rewo_net::command_format::UsageLine {
    rewo_net::command_format::UsageLine {
        text: text.to_string(),
        color: rewo_net::command_format::USAGE_COLOR,
    }
}

#[test]
fn the_usage_box_grows_upward_from_the_bottom() {
    // `lineY = height - 27 - 12 * y`, so entry 0 is the LOWEST. Laying it
    // out downward from a top puts a two-line box over the field.
    let lines = vec![gray("<count>"), gray("<flag>")];
    let (fills, text) = super::usage_box(&lines, 40, 240, 1.0, 0xD000_0000, &|t| {
        t.chars().count() as i32 * 6
    });
    assert_eq!(fills.len(), 2);
    assert_eq!(fills[0].y, (240 - 27) as f32);
    assert_eq!(fills[1].y, (240 - 27 - 12) as f32);
    assert!(fills[1].y < fills[0].y, "later lines sit HIGHER");
    // The text sits two pixels down inside its fill.
    assert_eq!(text[0].y, fills[0].y + 2.0);
    assert_eq!(text[0].x, 40.0);
}

#[test]
fn the_usage_fill_is_one_pixel_wider_than_its_text_on_each_side() {
    // `position - 1` to `position + width + 1`.
    let lines = vec![gray("<count>")];
    let (fills, text) = super::usage_box(&lines, 40, 240, 1.0, 0xD000_0000, &|t| {
        t.chars().count() as i32 * 6
    });
    assert_eq!(fills[0].x, 39.0);
    assert_eq!(fills[0].w, (7 * 6 + 2) as f32);
    assert_eq!(text[0].x - fills[0].x, 1.0);
}

#[test]
fn the_usage_box_is_as_wide_as_its_widest_line() {
    let lines = vec![gray("<a>"), gray("<a much longer one>")];
    let (fills, _) = super::usage_box(&lines, 0, 240, 1.0, 0xD000_0000, &|t| {
        t.chars().count() as i32 * 6
    });
    let widest = "<a much longer one>".chars().count() as i32 * 6 + 2;
    assert!(fills.iter().all(|f| f.w == widest as f32));
}

#[test]
fn each_usage_line_takes_its_own_colour_rather_than_the_boxs(
) {
    // M134. `extractUsage` passes ONE `-1` for the whole list and
    // `Font.getTextColor` uses it only where the style has no colour —
    // so a grey usage entry and a white exception message land in the
    // same box with different colours. Drawing the box in one colour is
    // wrong for one of the two whichever is picked, and it drew
    // everything white from M117 until the messages arrived.
    let lines = vec![
        gray("<count>"),
        rewo_net::command_format::UsageLine {
            text: "Unknown command at position 1: /<--[HERE]".to_string(),
            color: rewo_net::command_format::ERROR_COLOR,
        },
    ];
    let (_, text) = super::usage_box(&lines, 0, 240, 1.0, 0xD000_0000, &|t| {
        t.chars().count() as i32 * 6
    });
    // White is 1.0 in both spaces, which is what hid this.
    assert_eq!(text[1].color_linear, [1.0, 1.0, 1.0]);
    assert_ne!(text[0].color_linear, text[1].color_linear);
    // …and the grey is the sRGB byte decoded, not the byte itself: the
    // HUD tint multiplies in linear space (0xAA is 0.667 encoded and
    // ~0.402 linear).
    assert!(
        (text[0].color_linear[0] - 0.402).abs() < 0.01,
        "{:?}",
        text[0].color_linear
    );
}

/// The input field's caret sits after the text before the cursor, and the
/// bar behind it is the screen's own rather than a chat row's.
#[test]
fn the_input_line_carries_its_text_and_a_caret() {
    use rewo_world::chat_screen::{ChatMethod, ChatScreen};
    let mut s = ChatScreen::open(ChatMethod::Message, None, 0);
    s.char_typed('h', &chat_env());
    s.char_typed('i', &chat_env());
    // `focused_time_ms` is unset until `set_focused_at`, so the blink's
    // clock starts at 0 and the caret is visible at t = 0.
    let lines = super::chat_input_lines(&s, 1.0, 320, 240, 0, None, &|t: &str| t.chars().count() as i32 * 6);
    assert_eq!(lines[0].text, "hi");
    // `EditBox(font, 4, height - 12, …)` with `(height - 8) / 2` centring.
    assert_eq!((lines[0].x, lines[0].y), (4.0, 230.0));
    assert_eq!(lines[1].text, "_");
    // Two characters at 6 px each.
    assert_eq!(lines[1].x, 4.0 + 12.0);

    let bar = super::chat_input_backdrop(320, 240);
    assert_eq!((bar.x, bar.y, bar.w, bar.h), (2.0, 226.0, 316.0, 12.0));
    // A fixed 128/255 — NOT the text-background slider the rows read.
    assert!((bar.alpha - 128.0 / 255.0).abs() < 1e-6);
}

/// An untouched restored draft renders grey; an edited one does not.
#[test]
fn a_restored_draft_renders_grey_until_it_is_edited() {
    use rewo_world::chat_screen::{ChatMethod, ChatScreen, Draft};
    let draft = Draft::of("remembered");
    let mut s = ChatScreen::open(ChatMethod::Message, Some(&draft), 0);
    let grey = super::chat_input_lines(&s, 1.0, 320, 240, 0, None, &|t: &str| {
        t.chars().count() as i32 * 6
    })[0]
        .color_linear;
    // `ChatFormatting.GRAY` is `0xAAAAAA`, and the pass wants LINEAR.
    // 0.4020 is `((170/255 + 0.055) / 1.055)^2.4` — written out rather
    // than called, so this pins the number against the sRGB transfer
    // function itself and not against `srgb_bytes_to_linear`, which the
    // renderer also calls (M93q's self-calibration trap).
    assert!(
        grey.iter().all(|c| (c - 0.401_977_8).abs() < 1e-5),
        "ChatFormatting.GRAY in linear, got {grey:?}"
    );
    // The pre-M130 value, and the exact signature of the bug: handing the
    // byte over `/255` stores 213 where vanilla stores 170.
    assert!(
        (grey[0] - 2.0 / 3.0).abs() > 0.1,
        "the /255 byte is what M130 removed"
    );
    s.char_typed('!', &chat_env());
    let normal = super::chat_input_lines(&s, 1.0, 320, 240, 0, None, &|t: &str| {
        t.chars().count() as i32 * 6
    })[0]
        .color_linear;
    assert_ne!(normal, grey);
}

/// A restored draft is grey **and italic**, and its caret is neither.
///
/// `ChatScreen.formatChat` returns
/// `Style.EMPTY.withColor(GRAY).withItalic(true)`, and the draft field on
/// `rewo_world::chat_screen` has said "drives the grey **italic**
/// rendering" in its own doc since M110 while the renderer drew
/// `TextStyle::PLAIN` — the fifth comment in this project to describe
/// behaviour its code did not have.
///
/// The caret half is the part a plausible implementation gets wrong.
/// `graphics.text(font, applyFormat(half, …), …, color, …)` lets the
/// formatter's `Style` override `color` per character, but
/// `TextCursorUtils.extractAppendCursor` draws a bare `"_"` **String** with
/// `color` itself — so the draft's grey and italic reach the text and stop
/// at the caret.
#[test]
fn a_draft_is_italic_and_its_caret_is_not() {
    use rewo_world::chat_screen::{ChatMethod, ChatScreen, Draft};
    let draft = Draft::of("remembered");
    // t=0 is inside `isCursorVisible`'s first 300 ms window, so the caret
    // is emitted; without it this test would silently assert over one line.
    let lines = super::chat_input_lines(
        &ChatScreen::open(ChatMethod::Message, Some(&draft), 0),
        1.0,
        320,
        240,
        0,
        None,
        &|t: &str| t.chars().count() as i32 * 6,
    );
    let text = lines
        .iter()
        .find(|l| l.text == "remembered")
        .expect("the draft text");
    let caret = lines.iter().find(|l| l.text == "_").expect("the caret");
    assert!(text.style.italic, "withItalic(true)");
    assert_eq!(
        text.style,
        rewo_gpu::text::TextStyle {
            italic: true,
            ..rewo_gpu::text::TextStyle::PLAIN
        },
        "italic and ONLY italic — the style is a patch, not a preset"
    );
    assert_eq!(
        caret.style,
        rewo_gpu::text::TextStyle::PLAIN,
        "a bare String never meets the formatter"
    );
    assert_ne!(
        caret.color_linear, text.color_linear,
        "and it keeps the field's textColor rather than the draft's grey"
    );
    assert_eq!(caret.color_linear, srgb_bytes_to_linear(EDIT_BOX_TEXT_COLOR));
}

/// The two converters really are the sRGB transfer function.
///
/// Pinned against the **formula**, written out, rather than against
/// `rewo_gpu::srgb_to_linear`, which both of them call: a witness that
/// asks the implementation what to expect asserts only that the
/// implementation equals itself (M93q). `((b/255 + 0.055) / 1.055)^2.4` for
/// each byte below, computed independently.
///
/// This is what stops the whole M130 conversion decaying into an identity
/// without a single gate going red — nine of the sites it fixed are graded
/// only by the pixel gates named in the milestone message, and four
/// (the tooltip's bitmap line, the statistics rows, the enchanting costs
/// and the held-item name) by nothing else at all.
#[test]
fn the_converters_are_the_srgb_transfer_function() {
    for (byte, want) in [
        (0x00u32, 0.0f32),
        (0x55, 0.090_841_7),
        (0x80, 0.215_860_5),
        (0xA0, 0.351_532_6),
        (0xAA, 0.401_977_8),
        (0xBA, 0.491_020_8),
        (0xE0, 0.745_404_2),
        (0xFF, 1.0),
    ] {
        let packed = super::srgb_bytes_to_linear(byte << 16 | byte << 8 | byte);
        let triple = super::srgb_bytes_to_linear_f([byte as f32 / 255.0; 3]);
        for c in packed.iter().chain(triple.iter()) {
            assert!(
                (c - want).abs() < 1e-5,
                "0x{byte:02X} -> {c}, want {want} (identity would give {})",
                byte as f32 / 255.0
            );
        }
    }
}

/// `screen_text_lines`' widget labels convert too.
///
/// **Found by a surviving mutation, and it was a real gap rather than an
/// equivalent mutant.** `deathshot`'s p8 grades a button label that comes
/// through `death_screen_lines`, a different builder; `serverlinkshot`
/// drives this one but every label on the pause / disconnect / dialog
/// screens is WHITE — the titles and reasons after M130a's
/// `WithInactiveMessage` fix, and its buttons because they are active — and
/// white is 1.0 in both spaces, so no pixel witness there can see a
/// colour-space change. The one reachable case is an INACTIVE BUTTON, which
/// none of those screens builds. Hence a unit test rather than a gate.
#[test]
fn an_inactive_button_label_is_linear() {
    use rewo_world::screen::{Screen, ScreenKind, Widget};
    let mut b = Widget::button(0, 10, 10, 200, 20, "Respawn");
    b.active = false;
    let screen = Screen::new(ScreenKind::Death, 320, 240).with_widgets(vec![b]);
    let lines = screen_text_lines(&screen, &advances(), 1.0);
    assert_eq!(lines.len(), 1);
    // `defaultInactiveMessage` is `withColor(-6250336)` = `0xA0A0A0`, whose
    // linear value is 0.3515 against the byte's 0.6275 — the difference
    // between the framebuffer storing 160, as vanilla does, and 208.
    assert!(
        lines[0]
            .color_linear
            .iter()
            .all(|c| (c - 0.351_532_6).abs() < 1e-5),
        "got {:?}",
        lines[0].color_linear
    );
    assert_ne!(
        lines[0].color_linear,
        [160.0 / 255.0; 3],
        "the /255 byte is what M130 removed"
    );
}

/// The enchanting table's cost numerals convert too.
///
/// This one has **no pixel witness anywhere** — `containershot` grades the
/// rows' geometry and their three states, not the numeral's colour — so
/// without this the site is covered only by the `color_linear` rename.
#[test]
fn the_enchant_cost_numerals_are_linear() {
    use rewo_world::menu_screen::{EnchantRow, ENCHANT_COST_ENABLED};
    let rows = [
        EnchantRow::Available { cost: 3 },
        EnchantRow::Empty,
        EnchantRow::Empty,
    ];
    let lines = super::enchant_cost_labels(rows, &advances(), 1280.0, 720.0);
    assert_eq!(lines.len(), 1, "an empty row draws no numeral");
    // `0x80FF20` — green. Its red channel is 0x80, whose linear value is
    // 0.2159 against the byte's 0.5020: a difference no eye would call a
    // colour change and every pixel would.
    assert!(
        (lines[0].color_linear[0] - 0.215_860_5).abs() < 1e-5,
        "got {:?} for {ENCHANT_COST_ENABLED:#08X}",
        lines[0].color_linear
    );
    assert!(
        (lines[0].color_linear[1] - 1.0).abs() < 1e-6,
        "0xFF is 1.0 in both spaces, so it cannot witness the conversion \
         on its own — the red channel above is what does"
    );
}

/// An empty chat produces no lines at all — not one blank row.
#[test]
fn an_empty_chat_draws_nothing() {
    let opts = rewo_world::chat::ChatOptions::default();
    assert!(super::chat_lines(
        &rewo_world::chat::ChatComponent::new(),
        0,
        1.0,
        720.0,
        &opts,
        None,
        false,
    )
    .0
    .is_empty());
}


/// The washes are two lists, and the veil is NOT widened for a big result
/// slot — only the wash beneath it is.
#[test]
fn the_ghosts_two_washes_go_in_different_halves_and_only_one_widens() {
    use rewo_world::ghost_slots as gs;
    let layout = &rewo_world::menu_layout::PLAYER;
    let g = [gs::Ghost { slot: 0, items: vec![1], is_result: true }];
    // A crafting table screen: the result's wash is big.
    let (under, over) = super::ghost_washes(&g, layout, false);
    assert_eq!(under.len(), 1);
    assert_eq!(over.len(), 1, "one of each, per ghost");
    assert_eq!((under[0].1.w, under[0].1.h), (24.0, 24.0), "the big wash");
    assert_eq!((over[0].1.w, over[0].1.h), (16.0, 16.0), "the veil stays 16");
    // …and the big one starts 4 px up and left of the veil.
    assert_eq!(under[0].1.dx, over[0].1.dx - 4.0);
    assert_eq!(under[0].1.dy, over[0].1.dy - 4.0);

    // The player's own inventory: not big, so the two coincide.
    let (u2, o2) = super::ghost_washes(&g, layout, true);
    assert_eq!((u2[0].1.w, u2[0].1.h), (16.0, 16.0));
    assert_eq!((u2[0].1.dx, u2[0].1.dy), (o2[0].1.dx, o2[0].1.dy));
}

/// The two washes are DIFFERENT colours — red under, white over.
#[test]
fn the_wash_under_is_red_and_the_veil_is_white() {
    use rewo_world::ghost_slots as gs;
    let g = [gs::Ghost { slot: 1, items: vec![1], is_result: false }];
    let (under, over) = super::ghost_washes(&g, &rewo_world::menu_layout::PLAYER, false);
    // alpha 48/255 on both.
    assert!((under[0].1.tint[3] - 48.0 / 255.0).abs() < 1e-6);
    assert!((over[0].1.tint[3] - 48.0 / 255.0).abs() < 1e-6);
    // Red: full red, no green or blue.
    assert_eq!(under[0].1.tint[0], 1.0);
    assert_eq!((under[0].1.tint[1], under[0].1.tint[2]), (0.0, 0.0));
    // White: all three.
    assert_eq!(
        (over[0].1.tint[0], over[0].1.tint[1], over[0].1.tint[2]),
        (1.0, 1.0, 1.0)
    );
    assert_ne!(under[0].1.tint, over[0].1.tint);
}

/// Both washes are FILL quads, so they carry no sprite — the container pass
/// reads a negative `u` as untextured.
#[test]
fn the_washes_are_untextured_fills() {
    use rewo_world::ghost_slots as gs;
    let g = [gs::Ghost { slot: 1, items: vec![1], is_result: false }];
    let (under, over) = super::ghost_washes(&g, &rewo_world::menu_layout::PLAYER, false);
    assert_eq!(under[0].0, rewo_gpu::container::FILL_SPRITE);
    assert_eq!(over[0].0, rewo_gpu::container::FILL_SPRITE);
}

/// A ghost on a slot the layout does not have is dropped rather than drawn
/// at the origin.
#[test]
fn a_ghost_on_a_slot_that_does_not_exist_is_dropped() {
    use rewo_world::ghost_slots as gs;
    let g = [gs::Ghost { slot: 999, items: vec![1], is_result: false }];
    let (under, over) = super::ghost_washes(&g, &rewo_world::menu_layout::PLAYER, false);
    assert!(under.is_empty() && over.is_empty());
}

/// Each ghost lands on its own slot's position, not all at one place.
#[test]
fn each_ghost_lands_on_its_own_slot() {
    use rewo_world::ghost_slots as gs;
    let g = [
        gs::Ghost { slot: 1, items: vec![1], is_result: false },
        gs::Ghost { slot: 2, items: vec![2], is_result: false },
    ];
    let (under, _) = super::ghost_washes(&g, &rewo_world::menu_layout::PLAYER, false);
    assert_eq!(under.len(), 2);
    assert_ne!((under[0].1.dx, under[0].1.dy), (under[1].1.dx, under[1].1.dy));
    // And they match the layout's own positions.
    for (n, slot) in [1usize, 2].into_iter().enumerate() {
        let (sx, sy) = rewo_world::menu_layout::PLAYER.position(slot).unwrap();
        assert_eq!((under[n].1.dx, under[n].1.dy), (sx as f32, sy as f32));
    }
}

// -- the two crafting fills (M102) ---------------------------------------

/// A player menu holding one stack in a given slot.
fn inv_with(slots: &[(usize, rewo_world::inventory::ItemSlot)]) -> rewo_world::inventory::Inventory {
    let mut inv = rewo_world::inventory::Inventory::default();
    for &(slot, st) in slots {
        inv.set_slot(0, slot as i32, Some(st));
    }
    inv
}

fn dirt() -> rewo_world::inventory::ItemSlot {
    rewo_world::inventory::ItemSlot::plain(1, 1)
}

/// Both fills run, and they are disjoint: a stack on the player's 2x2 grid
/// counts EXACTLY once, through the craft-slot half.
#[test]
fn the_grid_is_counted_once_through_the_craft_slot_fill() {
    let max = |_: i32| 64;
    let ing = [Ingredient::of(&[1])];
    // Slot 1 is the player's crafting grid — outside PLAYER_ITEM_SLOTS.
    let mut c = super::crafting_contents(
        &inv_with(&[(1, dirt())]),
        None,
        rb96::BookType::Crafting,
        &max,
    );
    assert!(c.try_pick(&ing, 1), "one on the grid satisfies one ingredient");
    // …and only once: two ingredients need two items, so one is not enough.
    let mut c2 = super::crafting_contents(
        &inv_with(&[(1, dirt())]),
        None,
        rb96::BookType::Crafting,
        &max,
    );
    assert!(
        !c2.try_pick(&[Ingredient::of(&[1]), Ingredient::of(&[1])], 1),
        "double-counting the grid would make this pass"
    );
}

/// The craft RESULT (slot 0) counts for nothing — a recipe must not read as
/// craftable off its own output.
#[test]
fn the_craft_result_counts_for_nothing() {
    let max = |_: i32| 64;
    let mut c = super::crafting_contents(
        &inv_with(&[(0, dirt())]),
        None,
        rb96::BookType::Crafting,
        &max,
    );
    assert!(!c.try_pick(&[Ingredient::of(&[1])], 1));
}

/// A container's craft slots are added on top of the player's items, which
/// is what makes a part-finished craft readable — the M96 gap.
#[test]
fn an_open_menus_craft_slots_are_added_to_the_players_items() {
    let max = |_: i32| 64;
    let ing = [Ingredient::of(&[1]), Ingredient::of(&[1])];
    // One in the hotbar, one on the open table's grid.
    let player = inv_with(&[(36, dirt())]);
    let table = inv_with(&[(1, dirt())]);
    let mut both = super::crafting_contents(&player, Some(&table), rb96::BookType::Crafting, &max);
    assert!(both.try_pick(&ing, 1), "one from each half");
    // Without the table's half, one short.
    let mut alone = super::crafting_contents(&player, None, rb96::BookType::Crafting, &max);
    assert!(!alone.try_pick(&ing, 1));
}

/// A furnace contributes its whole container, RESULT INCLUDED, and
/// ungated — where a crafting grid excludes its result and is gated.
#[test]
fn a_furnaces_result_slot_counts_where_a_crafting_tables_does_not() {
    let max = |_: i32| 64;
    let ing = [Ingredient::of(&[1])];
    // Slot 2 is the furnace's result.
    let furnace = inv_with(&[(2, dirt())]);
    let mut f = super::crafting_contents(
        &rewo_world::inventory::Inventory::default(),
        Some(&furnace),
        rb96::BookType::Furnace,
        &max,
    );
    assert!(f.try_pick(&ing, 1), "a furnace's output counts");
    // The same slot under a crafting table's range (1..10) is a grid cell,
    // so it counts there too — the discriminating slot is 0, which the
    // crafting range excludes and the furnace range includes.
    let mut zero = super::crafting_contents(
        &rewo_world::inventory::Inventory::default(),
        Some(&inv_with(&[(0, dirt())])),
        rb96::BookType::Furnace,
        &max,
    );
    assert!(zero.try_pick(&ing, 1), "furnace slot 0 is its ingredient slot");
}

/// The furnace half is UNGATED: a damaged stack in a furnace counts, and the
/// same stack on a crafting grid does not.
#[test]
fn the_furnace_half_ignores_isUsableForCrafting_and_the_crafting_half_does_not() {
    let max = |_: i32| 64;
    let ing = [Ingredient::of(&[1])];
    let chipped = rewo_world::inventory::ItemSlot {
        damage: Some(3),
        max_damage: Some(59),
        ..dirt()
    };
    let mut f = super::crafting_contents(
        &rewo_world::inventory::Inventory::default(),
        Some(&inv_with(&[(1, chipped)])),
        rb96::BookType::Furnace,
        &max,
    );
    assert!(f.try_pick(&ing, 1), "a furnace takes a damaged stack");
    let mut t = super::crafting_contents(
        &rewo_world::inventory::Inventory::default(),
        Some(&inv_with(&[(1, chipped)])),
        rb96::BookType::Crafting,
        &max,
    );
    assert!(!t.try_pick(&ing, 1), "a crafting grid does not");
}

// -- the recipe book's derivation (M97) ---------------------------------
//
// M96 graded this at its two ends - the solver's own tests and the gate's
// chrome witness - and not in between. These drive `book_render_from`,
// which is the whole arithmetic: grouping, tab membership, paging, the
// display cycle, and `hasCraftable`.

use rewo_world::recipe_book_screen as rb96;
use rewo_world::stacked_contents::{Ingredient, StackedContents};

fn entry<'a>(
    id: i32,
    group: Option<i32>,
    category: &'a str,
    results: &[i32],
    ingredients: Option<&[&[i32]]>,
) -> super::BookEntry<'a> {
    super::BookEntry {
        id,
        group,
        category,
        results: results.to_vec(),
        ingredients: ingredients
            .map(|v| v.iter().map(|ids| Ingredient::of(ids)).collect()),
        // M99 — no searchable text, which every witness below relies on
        // being inert: `matches` skips the stage on an empty query.
        search: Default::default(),
        // M104 — a shapeless recipe whose ingredients are the crafting
        // requirements, so a collection built from these has a grid the
        // which-of-these overlay can draw.
        shape: rewo_world::recipe_overlay::Shape::Shapeless {
            ingredients: ingredients.map_or(0, |v| v.len()),
        },
        grid_items: ingredients
            .map(|v| v.iter().map(|ids| ids.to_vec()).collect())
            .unwrap_or_default(),
    }
}

/// [`entry`] with searchable text, for the M99 witnesses.
fn searchable<'a>(id: i32, category: &'a str, name: &str, item: &str) -> super::BookEntry<'a> {
    let (ns, path) = item.split_once(':').unwrap();
    super::BookEntry {
        search: rewo_world::recipe_search::SearchEntry {
            names: vec![name.to_lowercase()],
            ids: vec![(ns.to_string(), path.to_string())],
        },
        ..entry(id, None, category, &[70], None)
    }
}

/// `book_render_from` with the two inputs no M97 witness varies — the
/// book's own selection (default) and the cursor (absent, so no hover).
/// M98 added both; funnelling through one helper keeps those witnesses
/// about what they were written for.
fn render(
    book: rb96::BookType,
    filtering: bool,
    entries: &[super::BookEntry<'_>],
    held: &mut StackedContents,
    cycle: i32,
) -> super::BookRender {
    super::book_render_from(
        book,
        filtering,
        entries,
        held,
        cycle,
        rb96::BookState::default(),
        None,
        "",
    )
}

fn held(items: &[(i32, i32)]) -> StackedContents {
    let mut c = StackedContents::new();
    for &(item, count) in items {
        c.account(item, count);
    }
    c
}

const EQUIP: &str = "minecraft:crafting_equipment";
const MISC: &str = "minecraft:crafting_misc";

/// The whole point of M96, end to end: a slot's craftable flag follows
/// what the player is holding.
#[test]
fn a_slot_is_craftable_exactly_when_the_held_items_satisfy_it() {
    let e = [entry(1, None, EQUIP, &[7], Some(&[&[10], &[11]]))];
    let got = |items: &[(i32, i32)]| {
        render(rb96::BookType::Crafting, false, &e, &mut held(items), 0)
            .slots[0]
            .0
    };
    assert!(got(&[(10, 1), (11, 1)]), "both ingredients held");
    assert!(!got(&[(10, 1)]), "one missing");
    assert!(!got(&[]), "nothing held");
    // Holding MORE than enough is still craftable.
    assert!(got(&[(10, 64), (11, 64)]));
}

/// `canCraft` opens with `craftingRequirements.isEmpty() ? false`, so an
/// entry that carried none is never craftable however much you hold — the
/// state the solver alone cannot express, because it never sees the entry.
#[test]
fn an_entry_with_no_requirements_is_never_craftable() {
    let e = [entry(1, None, EQUIP, &[7], None)];
    let r = render(
        rb96::BookType::Crafting,
        false,
        &e,
        &mut held(&[(10, 64), (11, 64)]),
        0,
    );
    assert!(!r.slots[0].0);
    // ...while the same entry declaring an EMPTY list is craftable, since
    // there is nothing to satisfy. The two states are distinct.
    let e2 = [entry(1, None, EQUIP, &[7], Some(&[]))];
    let r2 =
        render(rb96::BookType::Crafting, false, &e2, &mut held(&[]), 0);
    assert!(r2.slots[0].0);
}

/// `hasCraftable()` is ANY of the collection's recipes, so a group with one
/// affordable member lights up.
#[test]
fn a_collection_is_craftable_if_ANY_of_its_recipes_is() {
    // Two recipes in one group: the first needs an item we lack.
    let e = [
        entry(1, Some(5), EQUIP, &[7], Some(&[&[99]])),
        entry(2, Some(5), EQUIP, &[7], Some(&[&[10]])),
    ];
    let r =
        render(rb96::BookType::Crafting, false, &e, &mut held(&[(10, 1)]), 0);
    assert_eq!(r.slots.len(), 1, "one grouped collection");
    assert!(r.slots[0].0, "the second recipe is affordable");
    assert!(r.slots[0].1, "and it has several recipes");
}

/// The solver is asked once per recipe and RESTORES what it took, so two
/// collections needing the same item do not starve each other. A solver
/// that consumed would light the first and grey the second.
#[test]
fn asking_about_one_collection_does_not_spend_anothers_items() {
    let e = [
        entry(1, None, EQUIP, &[7], Some(&[&[10]])),
        entry(2, None, EQUIP, &[8], Some(&[&[10]])),
    ];
    let r =
        render(rb96::BookType::Crafting, false, &e, &mut held(&[(10, 1)]), 0);
    assert_eq!(r.slots.len(), 2);
    assert!(r.slots[0].0 && r.slots[1].0, "both, from one item");
}

/// The search tab shows every category the book has; a category from
/// ANOTHER book does not appear at all.
#[test]
fn the_page_holds_this_books_categories_and_no_others() {
    let e = [
        entry(1, None, EQUIP, &[7], None),
        entry(2, None, MISC, &[8], None),
        entry(3, None, "minecraft:smoker_food", &[9], None),
        entry(4, None, "minecraft:stonecutter", &[9], None),
    ];
    let r = render(rb96::BookType::Crafting, false, &e, &mut held(&[]), 0);
    assert_eq!(r.slots.len(), 2, "equipment and misc, not smoker or stonecutter");
    let smoker =
        render(rb96::BookType::Smoker, false, &e, &mut held(&[]), 0);
    assert_eq!(smoker.slots.len(), 1);
}

/// The cycle reaches the rendered item, and it is the SHARED clock — every
/// slot advances together.
#[test]
fn the_display_cycle_reaches_the_slot_items() {
    let e = [
        entry(1, Some(5), EQUIP, &[70], Some(&[&[10]])),
        entry(2, Some(5), EQUIP, &[71], Some(&[&[10]])),
    ];
    let at = |cycle: i32| {
        render(rb96::BookType::Crafting, false, &e, &mut held(&[]), cycle)
            .slot_items[0]
    };
    assert_eq!(at(0), Some(70));
    assert_eq!(at(1), Some(71));
    assert_eq!(at(2), Some(70), "and it wraps");
}

/// The shadow copy needs BOTH conditions — several recipes and one shared
/// result. Two recipes yielding different items draw one icon.
#[test]
fn the_shadow_needs_several_recipes_AND_one_result() {
    let same = [
        entry(1, Some(5), EQUIP, &[70], None),
        entry(2, Some(5), EQUIP, &[70], None),
    ];
    let differ = [
        entry(1, Some(5), EQUIP, &[70], None),
        entry(2, Some(5), EQUIP, &[71], None),
    ];
    let one = [entry(1, None, EQUIP, &[70], None)];
    let shadowed = |e: &[super::BookEntry<'_>]| {
        render(rb96::BookType::Crafting, false, e, &mut held(&[]), 0)
            .slot_shadowed[0]
    };
    assert!(shadowed(&same));
    assert!(!shadowed(&differ), "several recipes, different results");
    assert!(!shadowed(&one), "one recipe");
}

/// The view's `shown` is what the page actually holds, and its
/// `total_pages` counts every collection — so a 45-recipe book pages.
#[test]
fn a_long_book_pages_and_the_first_page_is_full() {
    let cats: Vec<String> = (0..45).map(|_| EQUIP.to_string()).collect();
    let e: Vec<_> = (0..45)
        .map(|i| entry(i, None, &cats[i as usize], &[70], None))
        .collect();
    let r = render(rb96::BookType::Crafting, false, &e, &mut held(&[]), 0);
    let v = r.view.unwrap();
    assert_eq!(v.total_pages, 3);
    assert_eq!(v.shown, rb96::ITEMS_PER_PAGE);
    assert_eq!(r.slots.len(), rb96::ITEMS_PER_PAGE);
}

/// The filter flag reaches the view, which is what picks the toggle's art.
#[test]
fn the_filter_flag_reaches_the_view() {
    let e = [entry(1, None, EQUIP, &[7], None)];
    for filtering in [false, true] {
        let r =
            render(rb96::BookType::Crafting, filtering, &e, &mut held(&[]), 0);
        assert_eq!(r.view.unwrap().filtering, filtering);
    }
}

/// The book's selection reaches the view (M98) — and is CLAMPED, because a
/// tab index outlives the book it was chosen in.
#[test]
fn a_tab_index_too_big_for_this_book_clamps_rather_than_panicking() {
    let e = [entry(1, None, "minecraft:smoker_food", &[7], None)];
    // Tab 4 exists on a crafting book (five tabs) and not on a smoker (two).
    let st = rb96::BookState { selected_tab: 4, page: 0 };
    let r = super::book_render_from(
        rb96::BookType::Smoker,
        false,
        &e,
        &mut held(&[]),
        0,
        st,
        None,
        "",
    );
    assert_eq!(r.view.unwrap().selected_tab, 1, "clamped to the last tab");
}

/// A page index that outlived its list resets to the FRONT, not to the new
/// last page — `clamp_page`'s rule, reached through the real derivation.
///
/// The fixture needs more than one page left for the two readings to
/// differ, which is M93z's lesson a second time: the first draft used five
/// collections (one page), where reset-to-front and clamp-to-last are both
/// 0, and a mutation replacing the reset with a clamp survived it.
#[test]
fn a_page_index_that_outlived_its_list_resets_to_the_FRONT() {
    let three_pages: Vec<_> = (0..45)
        .map(|i| entry(i, None, EQUIP, &[7], None))
        .collect();
    let page_of = |page: usize, e: &[super::BookEntry<'_>]| {
        let st = rb96::BookState { selected_tab: 0, page };
        super::book_render_from(
            rb96::BookType::Crafting,
            false,
            e,
            &mut held(&[]),
            0,
            st,
            None,
            "",
        )
            .view
            .unwrap()
            .page
    };
    assert_eq!(page_of(2, &three_pages), 2, "still in range");
    // Out of range by one: `totalPages <= currentPage`, so page 3 of three
    // pages resets — and to 0, where a clamp would give 2.
    assert_eq!(page_of(3, &three_pages), 0);
    assert_eq!(page_of(9, &three_pages), 0, "and far out of range too");
    // A one-page list, where the two readings coincide and so prove
    // nothing on their own.
    let one_page: Vec<_> = (0..5).map(|i| entry(i, None, EQUIP, &[7], None)).collect();
    assert_eq!(page_of(3, &one_page), 0);
}

/// The hover comes from the SAME `book_hit` a press uses, so what lights up
/// is what a click would take.
#[test]
fn the_hover_lights_what_a_click_would_take() {
    use rewo_world::recipe_book_screen as rb;
    // 45 collections, so there is a forward arrow to hover.
    let e: Vec<_> = (0..45)
        .map(|i| entry(i, None, EQUIP, &[7], None))
        .collect();
    let at = |bx: i32, by: i32| {
        super::book_render_from(
            rb96::BookType::Crafting,
            false,
            &e,
            &mut held(&[]),
            0,
            rb96::BookState::default(),
            Some((bx, by)),
            "",
        )
        .hover
    };
    let f = at(rb::PAGE_FORWARD_X + 6, rb::PAGE_ARROW_Y + 8);
    assert!(f.page_forward && !f.page_backward && !f.filter);
    let fl = at(rb::FILTER_X + 5, rb::FILTER_Y + 5);
    assert!(fl.filter && !fl.page_forward);
    // On page 0 the BACK arrow is not drawn, so it cannot be hovered
    // either — one gate, both.
    let b = at(rb::PAGE_BACK_X + 6, rb::PAGE_ARROW_Y + 8);
    assert!(!b.page_backward);
    // Bare panel hovers nothing.
    let none = at(70, 29);
    assert!(!none.filter && !none.page_forward && !none.page_backward);
    // And no cursor at all hovers nothing.
    let absent = super::book_render_from(
        rb96::BookType::Crafting,
        false,
        &e,
        &mut held(&[]),
        0,
        rb96::BookState::default(),
        None,
            "",
        )
    .hover;
    assert!(!absent.filter && !absent.page_forward && !absent.page_backward);
}

/// A left-click places the recipe the CYCLE is on, not the collection's
/// first — `getCurrentRecipe`, the same index `getDisplayStack` reads.
#[test]
fn a_click_places_the_recipe_the_cycle_is_showing() {
    let e = [
        entry(11, Some(5), EQUIP, &[70], Some(&[&[1]])),
        entry(22, Some(5), EQUIP, &[71], Some(&[&[1]])),
    ];
    let at = |cycle: i32| {
        render(rb96::BookType::Crafting, false, &e, &mut held(&[]), cycle).slot_recipes[0]
    };
    assert_eq!(at(0), Some(11));
    assert_eq!(at(1), Some(22), "the second recipe, matching the shown item");
    assert_eq!(at(2), Some(11), "and it wraps with the display");
}

// -- the search (M99) ----------------------------------------------------

fn search_render(entries: &[super::BookEntry<'_>], query: &str) -> super::BookRender {
    super::book_render_from(
        rb96::BookType::Crafting,
        false,
        entries,
        &mut held(&[]),
        0,
        rb96::BookState::default(),
        None,
        query,
    )
}

/// The search narrows the page, and an EMPTY query narrows nothing —
/// vanilla skips the stage rather than running it with an empty string.
#[test]
fn the_search_narrows_the_page_and_an_empty_query_does_not() {
    let e = [
        searchable(1, EQUIP, "Diamond Sword", "minecraft:diamond_sword"),
        searchable(2, EQUIP, "Golden Apple", "minecraft:golden_apple"),
        searchable(3, MISC, "Iron Ingot", "minecraft:iron_ingot"),
    ];
    assert_eq!(search_render(&e, "").slots.len(), 3, "no query, no filter");
    assert_eq!(search_render(&e, "sword").slots.len(), 1);
    assert_eq!(search_render(&e, "gold").slots.len(), 1);
    assert_eq!(search_render(&e, "o").slots.len(), 3, "a substring of all three");
    assert_eq!(search_render(&e, "zzz").slots.len(), 0);
}

/// A colon-less query searches names only — the id is not consulted.
#[test]
fn a_query_without_a_colon_does_not_reach_the_ids() {
    // The name says Plank and the id says oak.
    let e = [searchable(1, EQUIP, "Wooden Plank", "minecraft:oak_planks")];
    assert_eq!(search_render(&e, "plank").slots.len(), 1);
    assert_eq!(search_render(&e, "oak").slots.len(), 0, "the id is not searched");
    assert_eq!(search_render(&e, "minecraft:oak").slots.len(), 1, "with a colon it is");
}

/// The page's total re-counts against the FILTERED list, so a search that
/// leaves one page removes the arrows.
#[test]
fn a_search_recounts_the_pages() {
    let mut e: Vec<_> = (0..45)
        .map(|i| searchable(i, EQUIP, "Cobblestone", "minecraft:cobblestone"))
        .collect();
    e.push(searchable(99, EQUIP, "Diamond Sword", "minecraft:diamond_sword"));
    assert_eq!(search_render(&e, "").view.unwrap().total_pages, 3);
    let one = search_render(&e, "sword");
    assert_eq!(one.view.unwrap().total_pages, 1);
    assert_eq!(one.slots.len(), 1);
}

/// A collection's searchable text is the UNION of its recipes', so a group
/// survives if any member matches — `flatMap` over `getRecipes()`.
#[test]
fn a_grouped_collection_matches_on_any_of_its_recipes() {
    let mut a = searchable(1, EQUIP, "Diamond Sword", "minecraft:diamond_sword");
    let mut b = searchable(2, EQUIP, "Golden Apple", "minecraft:golden_apple");
    a.group = Some(5);
    b.group = Some(5);
    let e = [a, b];
    assert_eq!(search_render(&e, "").slots.len(), 1, "one grouped collection");
    assert_eq!(search_render(&e, "sword").slots.len(), 1);
    assert_eq!(search_render(&e, "apple").slots.len(), 1, "the OTHER member");
    assert_eq!(search_render(&e, "iron").slots.len(), 0);
}

// -- the which-of-these overlay's derivation (M104) ---------------------
//
// Between the model (which knows the arithmetic and no recipes) and the
// pixel gate (which is handed a finished `Open`) sits the step that turns
// a page cell into a placed popup. That step is what M92 found is untested
// by construction whenever a gate supplies what production derives.

fn book_view(furnace_family: bool, filtering: bool) -> rb96::BookView {
    rb96::BookView {
        tabs: 5,
        selected_tab: 0,
        page: 0,
        total_pages: 1,
        shown: 1,
        filtering,
        furnace_family,
    }
}

fn button(recipe: i32, craftable: bool) -> rewo_world::recipe_overlay::Button {
    rewo_world::recipe_overlay::Button { recipe, craftable, slots: Vec::new() }
}

/// A collection reaches the overlay craftable-first, and the flag travels
/// with the recipe rather than being recomputed against a later inventory.
#[test]
fn opening_the_overlay_promotes_the_craftable_recipes() {
    let open = super::open_overlay(
        vec![button(7, false), button(8, true), button(9, false)],
        0,
        book_view(false, false),
    );
    assert_eq!(
        open.buttons.iter().map(|b| b.recipe).collect::<Vec<_>>(),
        vec![8, 7, 9],
        "craftable first, then the collection's own order"
    );
    assert_eq!(open.craftable_flags(), vec![true, false, false]);
}

/// Filtering drops the uncraftable half instead of greying it — so the
/// overlay a filtered book opens is SHORTER, not differently coloured.
#[test]
fn a_filtered_book_opens_an_overlay_of_craftable_recipes_only() {
    let all = vec![button(7, false), button(8, true)];
    let unfiltered = super::open_overlay(all.clone(), 0, book_view(false, false));
    let filtered = super::open_overlay(all, 0, book_view(false, true));
    assert_eq!(unfiltered.buttons.len(), 2);
    assert_eq!(filtered.buttons.len(), 1);
    assert_eq!(filtered.buttons[0].recipe, 8);
}

/// The panel is placed from the clicked CELL, through the clamps — and the
/// right-hand column is where the clamps show.
#[test]
fn the_overlay_is_placed_from_the_cell_that_opened_it() {
    let two = vec![button(1, true), button(2, true)];
    // Cell 0 (column 0) is well inside every bound, so the panel's origin
    // IS the cell's corner.
    let left = super::open_overlay(two.clone(), 0, book_view(false, false));
    assert_eq!(left.origin, rb96::grid_slot(0));
    // Cell 4 (column 4) overhangs, and the horizontal clamp moves it one
    // button width — one, not two, because that clamp truncates.
    let right = super::open_overlay(two, 4, book_view(false, false));
    assert_eq!(right.origin.0, rb96::grid_slot(4).0 - 25);
    assert_eq!(right.origin.1, rb96::grid_slot(4).1, "the row is untouched");
}

/// The button family follows the MENU, which is what `view` carries.
#[test]
fn the_overlays_family_comes_from_the_book_not_the_recipes() {
    let one = vec![button(1, true), button(2, true)];
    assert!(!super::open_overlay(one.clone(), 0, book_view(false, false)).furnace);
    assert!(super::open_overlay(one, 0, book_view(true, false)).furnace);
}

/// `slot_collections` carries per-recipe affordability, where `slots`
/// carries only whether ANY of them is — and the two must not disagree.
#[test]
fn a_collections_recipes_are_graded_one_by_one_for_the_overlay() {
    // Two recipes in one group: the first needs an item held, the second
    // does not exist in the inventory at all.
    let mut a = entry(1, Some(9), EQUIP, &[7], Some(&[&[10]]));
    let mut b = entry(2, Some(9), EQUIP, &[7], Some(&[&[99]]));
    a.group = Some(9);
    b.group = Some(9);
    let got = render(rb96::BookType::Crafting, false, &[a, b], &mut held(&[(10, 1)]), 0);
    assert_eq!(got.slots[0].0, true, "the CELL is craftable — any of them");
    let per = &got.slot_collections[0];
    assert_eq!(
        per.iter().map(|x| (x.recipe, x.craftable)).collect::<Vec<_>>(),
        vec![(1, true), (2, false)],
        "and the overlay sees them one by one, in the collection's order"
    );
}

/// An ingredient that resolves to nothing contributes no grid position,
/// and because neither placement arm counts, the rest do not shift.
#[test]
fn an_unresolvable_ingredient_drops_its_position_and_moves_no_other() {
    let full = entry(1, None, EQUIP, &[7], Some(&[&[10], &[11], &[12]]));
    let holey = entry(2, None, MISC, &[7], Some(&[&[10], &[], &[12]]));
    let got = render(rb96::BookType::Crafting, false, &[full, holey], &mut held(&[]), 0);
    let slots_of = |n: usize| got.slot_collections[n][0].slots.clone();
    let a = slots_of(0);
    let b = slots_of(1);
    assert_eq!(a.len(), 3);
    assert_eq!(b.len(), 2, "the empty one is skipped");
    // The survivors keep the positions they had, rather than closing up.
    assert_eq!(b[0].0, a[0].0);
    assert_eq!(b[1].0, a[2].0, "ingredient 2 did NOT slide into slot 1");
}

/// The field is built with the BOOK's maximum (50), not `EditBox`'s default
/// (32) — a long search would otherwise be silently truncated, and the
/// difference is invisible until someone types past 32.
#[test]
fn the_search_field_carries_the_books_own_maximum() {
    let st = super::ScreenState::default();
    let mut field = st.book_search;
    // `char_typed` is gated on `can_consume_input`, which needs focus — the
    // coupling `book_press` mirrors from `BookState::search_focused`.
    field.set_focused(true);
    for _ in 0..60 {
        field.char_typed('a');
    }
    assert_eq!(
        field.value().chars().count(),
        rewo_world::recipe_book_screen::SEARCH_MAX_LENGTH
    );
    assert_eq!(rewo_world::recipe_book_screen::SEARCH_MAX_LENGTH, 50);
    assert_ne!(rewo_world::edit_box::EditBox::default().max_length(), 50);
}

// -- the search field's render (M100) -------------------------------------

fn field_of(text: &str, focused: bool) -> rewo_world::edit_box::EditBox {
    use rewo_world::recipe_book_screen as rb;
    let mut f = rewo_world::edit_box::EditBox::new(rb::SEARCH_MAX_LENGTH);
    f.set_focused(true);
    for ch in text.chars() {
        f.char_typed(ch);
    }
    f.set_focused(focused);
    f
}

/// A stub advance table: every glyph 6 wide, which is the vanilla default
/// and enough to make the caret's x a checkable number.
fn advances() -> [u8; 256] {
    [6u8; 256]
}

/// The field's text is inset FOUR px and centred vertically — the bordered
/// case, which is the book's, and not `getY()`.
#[test]
fn the_fields_text_geometry_is_the_BORDERED_one() {
    use rewo_world::recipe_book_screen as rb;
    assert_eq!(rb::SEARCH_TEXT_X, rb::SEARCH_X + 4);
    assert_eq!(rb::SEARCH_TEXT_Y, rb::SEARCH_Y + 3, "(14 - 8) / 2");
    assert_eq!(rb::SEARCH_INNER_W, 73, "81 - 8, taken off BOTH ends");
    assert_ne!(rb::SEARCH_INNER_W, rb::SEARCH_W);
    assert_ne!(rb::SEARCH_TEXT_Y, rb::SEARCH_Y, "the unbordered case");
}

/// The background is TWO blits, and the second is inset one pixel on every
/// side — which is what leaves the border showing.
#[test]
fn the_fields_background_is_two_blits_and_the_inner_one_is_inset() {
    use rewo_world::recipe_book_screen as rb;
    let (_, fills) = super::book_field_render(&field_of("", false), &advances(), 1280.0, 720.0, 0);
    assert!(fills.len() >= 2);
    let (outer, inner) = (fills[0].1, fills[1].1);
    assert_eq!((outer.dx, outer.dy), (rb::SEARCH_X as f32, rb::SEARCH_Y as f32));
    assert_eq!((outer.w, outer.h), (rb::SEARCH_W as f32, rb::SEARCH_H as f32));
    assert_eq!((inner.dx, inner.dy), (outer.dx + 1.0, outer.dy + 1.0));
    assert_eq!((inner.w, inner.h), (outer.w - 2.0, outer.h - 2.0));
    // Both sample a 1x1 source, which is only exact because every region of
    // the sprite is uniform — a 1-bit paletted image of two colours.
    assert_eq!((outer.sw, outer.sh), (1.0, 1.0));
    assert_eq!((inner.sw, inner.sh), (1.0, 1.0));
    // …from DIFFERENT texels: the border's and the interior's.
    assert_ne!((outer.sx, outer.sy), (inner.sx, inner.sy));
}

/// Focus swaps the background sprite, and this is the one use of
/// `WidgetSprites::get` on the book that means what its names say.
#[test]
fn focus_swaps_the_fields_background_sprite() {
    let plain = super::book_field_render(&field_of("", false), &advances(), 1280.0, 720.0, 0).1;
    let lit = super::book_field_render(&field_of("", true), &advances(), 1280.0, 720.0, 0).1;
    assert_ne!(plain[0].0, lit[0].0, "a different sprite index");
    assert_eq!(lit[0].0, plain[0].0 + 1, "highlighted is the pair's second");
}

/// The hint goes when the field takes FOCUS, not when the first character
/// arrives — `displayed.isEmpty() && !isFocused()`.
#[test]
fn the_hint_goes_on_focus_not_on_the_first_character() {
    use rewo_world::recipe_book_screen as rb;
    let hint_of = |text: &str, focused: bool| {
        super::book_field_render(&field_of(text, focused), &advances(), 1280.0, 720.0, 0)
            .0
            .into_iter()
            .find(|l| l.text == rb::SEARCH_HINT)
    };
    assert!(hint_of("", false).is_some(), "empty and unfocused");
    assert!(hint_of("", true).is_none(), "FOCUSED and still empty");
    assert!(hint_of("iron", false).is_none(), "unfocused but not empty");
    assert!(hint_of("iron", true).is_none());
    // Its colour is the hint style's grey, not the field's white.
    let h = hint_of("", false).unwrap();
    // `SEARCH_HINT_STYLE` is `ChatFormatting.GRAY` — `0xAAAAAA` — and the
    // text pass takes LINEAR, so the constant converts on the way in
    // rather than being handed over as the byte.
    assert!(
        h.color_linear
            .iter()
            .all(|c| (c - 0.401_977_8).abs() < 1e-5),
        "GRAY in linear, got {:?}",
        h.color_linear
    );
    assert_ne!(h.color_linear, rb::SEARCH_HINT_COLOR, "not the raw /255");
    assert_ne!(h.color_linear, [1.0, 1.0, 1.0]);
}

/// The typed text is drawn at the field's text origin, in white.
#[test]
fn the_typed_text_is_drawn_where_the_field_says() {
    use rewo_world::recipe_book_screen as rb;
    let (labels, _) =
        super::book_field_render(&field_of("iron", true), &advances(), 1280.0, 720.0, 0);
    let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(1280.0, 720.0);
    let text = labels.iter().find(|l| l.text == "iron").expect("the text");
    assert_eq!(text.x, bl + rb::SEARCH_TEXT_X as f32 * scale);
    assert_eq!(text.y, bt + rb::SEARCH_TEXT_Y as f32 * scale);
    assert_eq!(text.color_linear, [1.0, 1.0, 1.0], "setTextColor(-1)");
}

// -- the page counter (M105) --------------------------------------------

/// A language map holding the real `en_us.json` entry, so these read what
/// the client shows.
fn page_lang() -> rewo_data::lang::Language {
    let mut m = std::collections::HashMap::new();
    m.insert(
        rewo_world::recipe_book_screen::PAGE_LABEL_KEY.to_string(),
        "%s/%s".to_string(),
    );
    rewo_data::lang::Language::from_map(m)
}

fn book_of(page: usize, total: usize) -> super::BookRender {
    use rewo_world::recipe_book_screen as rb;
    super::BookRender {
        view: Some(rb::BookView {
            tabs: rb::CRAFTING_TABS.len(),
            selected_tab: 0,
            page,
            total_pages: total,
            shown: 0,
            filtering: false,
            furnace_family: false,
        }),
        book: rb::BookType::Crafting,
        ..Default::default()
    }
}

fn counter(page: usize, total: usize) -> Option<rewo_gpu::world::OwnedTextLine> {
    let (labels, _) = super::book_labels(
        &book_of(page, total),
        &field_of("", false),
        &page_lang(),
        &advances(),
        1280.0,
        720.0,
        0,
    );
    // By text rather than by index: the field contributes a hint label on
    // this fixture, so "the last one" would name whichever happened to be
    // pushed last and would keep passing if the two swapped.
    labels.into_iter().find(|l| l.text.contains('/'))
}

/// The counter reaches the composed label list at all — the step
/// `apply_screen` cannot be asked about, since it needs a `PlaySession`.
#[test]
fn the_page_counter_reaches_the_books_labels() {
    assert_eq!(counter(0, 3).map(|l| l.text), Some("1/3".to_string()));
    // …and the field's own text is still there beside it, so composing the
    // two did not replace one with the other.
    let (labels, _) = super::book_labels(
        &book_of(0, 3),
        &field_of("iron", true),
        &page_lang(),
        &advances(),
        1280.0,
        720.0,
        0,
    );
    assert!(labels.iter().any(|l| l.text == "iron"), "the search text");
    assert!(labels.iter().any(|l| l.text == "1/3"), "the counter");
}

/// `if (this.totalPages > 1)` — a single-page book draws no counter, and a
/// shut book contributes none because it has no view.
#[test]
fn a_single_page_book_draws_no_counter() {
    assert!(counter(0, 1).is_none(), "one page");
    assert!(counter(0, 0).is_none(), "an empty book");
    assert!(counter(0, 2).is_some(), "two pages");
    let (labels, _) = super::book_labels(
        &super::BookRender::default(),
        &field_of("", false),
        &page_lang(),
        &advances(),
        1280.0,
        720.0,
        0,
    );
    assert!(
        !labels.iter().any(|l| l.text.contains('/')),
        "no view, no counter"
    );
}

/// The x is `73 - width / 2` in BOOK pixels, and the width is MEASURED.
///
/// The two fixtures differ in label length by one character, so a build
/// that centred on a constant — or that measured the wrong string — puts
/// them at the same x. With the stub table's 6 px glyphs the difference is
/// exactly 3 book pixels.
#[test]
fn the_counter_is_placed_by_its_measured_width() {
    use rewo_world::recipe_book_screen as rb;
    let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(1280.0, 720.0);
    let short = counter(0, 3).expect("1/3");
    let long = counter(0, 10).expect("1/10");
    assert_eq!(short.text, "1/3");
    assert_eq!(long.text, "1/10");
    assert_eq!(short.x, bl + rb::page_label_x(3 * 6) as f32 * scale);
    assert_eq!(long.x, bl + rb::page_label_x(4 * 6) as f32 * scale);
    assert_ne!(short.x, long.x, "a constant x would agree here");
    assert_eq!(short.x - long.x, 3.0 * scale, "half the extra glyph");
    // The row is the same for both — only the x tracks the width.
    assert_eq!(short.y, bt + rb::PAGE_LABEL_Y as f32 * scale);
    assert_eq!(long.y, short.y);
}

/// Colour `-1` is opaque white, and the five-argument `graphics.text`
/// delegates with `dropShadow = true`.
#[test]
fn the_counter_is_white_and_shadowed() {
    let c = counter(0, 3).unwrap();
    assert_eq!(c.color_linear, [1.0, 1.0, 1.0]);
    assert_eq!(c.alpha, 1.0, "ARGB.alpha(-1) — and text() skips alpha 0");
    assert!(c.shadow, "the 5-arg overload passes true");
}

/// The FIRST tooltip of a frame wins, not the last (M106c).
///
/// `setTooltipForNextFrameInternal`'s body is
/// `if (this.deferredTooltip == null || replaceExisting)` — so the
/// container's, set before the book's two, is the one that survives. The
/// call order reads the other way, which is what makes this worth pinning.
#[test]
fn the_first_tooltip_of_a_frame_wins() {
    fn pick(
        m: Option<&'static str>,
        b: Option<&'static str>,
        g: Option<&'static str>,
    ) -> Option<&'static str> {
        super::frame_tooltip(&mut (), |_| None, |_| m, |_| b, |_| g)
    }
    assert_eq!(pick(Some("menu"), Some("book"), Some("ghost")), Some("menu"));
    assert_eq!(pick(None, Some("book"), Some("ghost")), Some("book"));
    assert_eq!(pick(None, None, Some("ghost")), Some("ghost"));
    assert_eq!(pick(None, None, None), None);
    // M133 — and the book's WIDGET tooltips are ahead of all three, which
    // is the same rule read one call earlier: the widgets set theirs inside
    // `extractRenderState`, which runs before `this.extractTooltip`.
    assert_eq!(
        super::frame_tooltip(
            &mut (),
            |_| Some("widget"),
            |_| Some("menu"),
            |_| Some("book"),
            |_| Some("ghost"),
        ),
        Some("widget"),
    );
    // The menu's beats the ghost's with no page tooltip in play, which is
    // the pair that is actually reachable together: a ghost sits ON a menu
    // slot, while a page cell and a menu slot can never both be hovered.
    assert_eq!(pick(Some("menu"), None, Some("ghost")), Some("menu"));
    // And the later producers are not even evaluated once one has spoken —
    // `deferredTooltip` is assigned, not compared.
    let mut ran = 0;
    assert_eq!(
        super::frame_tooltip(
            &mut ran,
            |_| None,
            |_| Some("menu"),
            |n| {
                *n += 1;
                Some("book")
            },
            |n| {
                *n += 1;
                Some("ghost")
            },
        ),
        Some("menu")
    );
    assert_eq!(ran, 0, "neither later producer ran");
}

/// The hover highlight resolves through the placement the book MOVED
/// (M106b).
///
/// The cursor is put at the true centre of a slot in each case, so a
/// conversion that ignored the book would miss it by 77 GUI px — four
/// columns on an 18 px pitch, and often off the panel entirely.
#[test]
fn the_hover_highlight_follows_the_panel_the_book_pushed() {
    // The crafting table: one of the four menus that has a book.
    let craft = rewo_world::menu_layout::layout_of(12).unwrap();
    let (w, h) = (1280.0f32, 720.0f32);
    let at = |slot: usize, book_open: bool| {
        let (l, t, sc) = rewo_gpu::container::gui_origin_placed(
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                craft.image_w as f32,
                craft.image_h as f32,
                book_open,
            ),
        );
        let (sx, sy) = craft.position(slot).unwrap();
        super::hovered_slot_position(
            craft,
            (
                (l + (sx as f32 + 8.0) * sc) as f64,
                (t + (sy as f32 + 8.0) * sc) as f64,
            ),
            w,
            h,
            book_open,
        )
    };
    let want = craft.position(1).map(|(x, y)| (x as i32, y as i32));
    assert_eq!(at(1, false), want, "book shut");
    assert_eq!(at(1, true), want, "book OPEN — the panel moved with it");
    // And the shift is real at this size, so the two cases are not the
    // same test written twice.
    let left = |book_open: bool| {
        rewo_gpu::container::gui_origin_placed(
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                craft.image_w as f32,
                craft.image_h as f32,
                book_open,
            ),
        )
        .0
    };
    assert_ne!(left(true), left(false));
    // Off the panel entirely is `None`, not slot 0 — otherwise every miss
    // would light the top-left slot.
    assert_eq!(
        super::hovered_slot_position(craft, (0.0, 0.0), w, h, false),
        None
    );
}

/// `if (!lines.isEmpty())` in `setTooltipForNextFrameInternal` — an empty
/// list sets NO tooltip, which is not the same as an empty box (M106).
///
/// **No current caller can reach this**: `screen_tooltip`'s two producers
/// both start with a name line, and so does `book_tooltip`. A mutation
/// deleting the guard therefore survived every behavioural witness, and
/// the choice was to drop the guard or to pin it. It is pinned, because it
/// is vanilla's rule for a shared entry point rather than a property of
/// today's two producers — the third one to arrive gets it for free. This
/// test names its own unreachability so the next reader does not go looking
/// for the path that exercises it.
#[test]
fn an_empty_line_list_sets_no_tooltip_rather_than_an_empty_box() {
    assert!(
        super::tooltip_layout(Vec::new(), &advances(), None, (100.0, 100.0), (1280.0, 720.0))
            .is_none()
    );
    // …and one line still does, so the guard is not simply "never".
    assert!(super::tooltip_layout(
        vec![vec![rewo_gpu::tooltip::Span::new("x".to_string(), [1.0; 3])]],
        &advances(),
        None,
        (100.0, 100.0),
        (1280.0, 720.0),
    )
    .is_some());
}

/// A missing translation renders the bare key — `getOrDefault` returns it
/// and a template with no specifiers survives substitution unchanged.
#[test]
fn a_missing_translation_shows_the_key() {
    use rewo_world::recipe_book_screen as rb;
    let empty = rewo_data::lang::Language::from_map(Default::default());
    let (labels, _) = super::book_labels(
        &book_of(0, 3),
        &field_of("", false),
        &empty,
        &advances(),
        1280.0,
        720.0,
        0,
    );
    assert!(labels.iter().any(|l| l.text == rb::PAGE_LABEL_KEY));
}

/// The blink reaches the RENDER: the same focused field draws a caret in
/// one 300 ms window and none in the next (M101).
#[test]
fn the_caret_blinks_in_the_rendered_output() {
    use rewo_world::recipe_book_screen as rb;
    let mut f = rewo_world::edit_box::EditBox::new(rb::SEARCH_MAX_LENGTH);
    f.set_focused_at(true, 1_000);
    for ch in "iron".chars() {
        f.char_typed(ch);
    }
    let drawn = |now: u64| {
        let (labels, fills) = super::book_field_render(&f, &advances(), 1280.0, 720.0, now);
        labels.iter().any(|l| l.text == "_")
            || fills.iter().skip(2).any(|(_, b)| b.w == 1.0 && b.h == 11.0)
    };
    assert!(drawn(1_000), "on, the instant it was focused");
    assert!(drawn(1_299));
    assert!(!drawn(1_300), "off for the next 300 ms");
    assert!(drawn(1_600), "and on again");
}

/// The anvil's field is pinned focused, so a click elsewhere cannot take
/// its caret away — `setCanLoseFocus(false)`.
#[test]
fn the_anvils_field_is_pinned_focused() {
    let mut f = super::anvil_field_new();
    assert!(f.is_focused(), "focused from the moment it is built");
    f.set_focused(false);
    assert!(f.is_focused(), "and it cannot lose it");
    assert_eq!(f.max_length(), rewo_world::anvil::MAX_NAME_LENGTH);
}

/// A caret outside the visible run is NOT drawn — `cursorOnScreen`, the
/// third of `showCursor`'s conditions, which M93t's renderer omitted.
///
/// Reachable only without `follow_cursor`: with it, every input keeps the
/// cursor inside the run, which is the point of M101's other half. So the
/// fixture types without following — the state the field was permanently in
/// before M101, and the state a programmatic cursor move still produces.
#[test]
fn a_caret_outside_the_visible_run_is_not_drawn() {
    use rewo_world::recipe_book_screen as rb;
    let mut f = rewo_world::edit_box::EditBox::new(rb::SEARCH_MAX_LENGTH);
    f.set_focused_at(true, 0);
    // 26 glyphs at 6 px is 156, well past the 73 px inner width.
    for ch in "abcdefghijklmnopqrstuvwxyz".chars() {
        f.char_typed(ch);
    }
    let caret = |field: &rewo_world::edit_box::EditBox| {
        let (labels, fills) = super::book_field_render(field, &advances(), 1280.0, 720.0, 0);
        labels.iter().any(|l| l.text == "_")
            || fills.iter().skip(2).any(|(_, b)| b.w == 1.0 && b.h == 11.0)
    };
    assert!(!caret(&f), "the cursor is past the visible run");
    // Follow it, and the caret comes back — the two halves of M101 meeting.
    let advance = advances();
    let width = move |u: &[u16]| rewo_gpu::text::width(&String::from_utf16_lossy(u), &advance);
    f.follow_cursor(rb::SEARCH_INNER_W, &width);
    assert!(caret(&f), "and following the cursor restores it");
}

/// A focused field draws a caret and an unfocused one does not.
#[test]
fn only_a_focused_field_draws_a_caret() {
    let caret = |focused: bool| {
        let (labels, fills) =
            super::book_field_render(&field_of("iron", focused), &advances(), 1280.0, 720.0, 0);
        // The append caret is the character "_"; the insert caret is a
        // 1-px fill. Either counts.
        labels.iter().any(|l| l.text == "_")
            || fills.iter().skip(2).any(|(_, b)| b.w == 1.0 && b.h == 11.0)
    };
    assert!(caret(true));
    assert!(!caret(false));
}

/// `rewo-gpu` restates the recipe book's geometry rather than importing it,
/// because the renderer deliberately does not depend on `rewo-world` (M94).
/// This is the crate that sees both, so this is where the copy is paid for.
///
/// Without it a drift draws the book a pixel off, or against the wrong
/// sheet, with nothing failing anywhere.
#[test]
fn the_renderers_copy_of_the_books_geometry_matches_the_model() {
    use rewo_world::recipe_book_screen as rb;
    let (w, h, off, narrow, sheet) =
        rewo_gpu::container::book_constants_for_cross_check();
    assert_eq!(w, rb::IMAGE_W as f32);
    assert_eq!(h, rb::IMAGE_H as f32);
    assert_eq!(off, rb::OFFSET_X as f32);
    assert_eq!(narrow, rb::WIDTH_TOO_NARROW_BELOW as f32);
    assert_eq!(
        rewo_data::assets::MENU_BACKGROUND_TEXTURES[sheet],
        "gui/recipe_book.png",
        "the renderer's sheet index must name the book's own sheet"
    );
    assert_eq!(sheet, rewo_data::assets::RECIPE_BOOK_SHEET);
}

/// The draw and the hit test resolve their origin through the same
/// `Placement`, so an open book must move BOTH or neither.
#[test]
fn an_open_book_moves_the_hit_test_exactly_as_far_as_the_panel() {
    use rewo_gpu::container::{gui_origin_placed, screen_to_gui_placed, Placement};
    let (w, h) = (1920.0f32, 1080.0f32);
    let shut = Placement::with_book(176.0, 166.0, false);
    let open = Placement::with_book(176.0, 166.0, true);
    let (l0, t0, sc) = gui_origin_placed(w, h, shut);
    let (l1, t1, _) = gui_origin_placed(w, h, open);
    assert_ne!(l0, l1, "the panel moves");
    assert_eq!(t0, t1, "but only horizontally");

    let m = (900.0, 500.0);
    let g0 = screen_to_gui_placed(m, w, h, shut);
    let g1 = screen_to_gui_placed(m, w, h, open);
    // The cursor's GUI-space x shifts by exactly the panel's own shift, so
    // a slot under the cursor before the book opened is not under it after.
    let panel_shift = ((l1 - l0) / sc) as f64;
    assert!(((g0.0 - g1.0) - panel_shift).abs() < 1e-6);
    assert_eq!(g0.1, g1.1);
}

/// A window under 379 GUI px keeps the menu centred, and the book covers it.
#[test]
fn a_narrow_window_moves_neither() {
    use rewo_gpu::container::{gui_origin_placed, Placement};
    // 640x480 at the GUI scale this picks is under the threshold.
    let (w, h) = (640.0f32, 480.0f32);
    let scale = rewo_gpu::hud::gui_scale(w, h);
    assert!(w / scale < 379.0, "the fixture has to actually be narrow");
    assert_eq!(
        gui_origin_placed(w, h, Placement::with_book(176.0, 166.0, true)),
        gui_origin_placed(w, h, Placement::with_book(176.0, 166.0, false))
    );
}

use super::*;
use rewo_net::effects::VisualEffectSnapshot;

// -- M87: the container panel the screen path builds --------------------

fn layout(id: i32) -> &'static rewo_world::menu_layout::MenuLayout {
    rewo_world::menu_layout::layout_of(id).unwrap()
}

#[test]
fn the_players_own_menu_has_no_container_panel() {
    // It is drawn from the pass's own `inventory.png` rect, and returning
    // a panel here would send it through the container path instead --
    // which is the change `inventoryshot` would catch, but only because
    // this stays None.
    assert!(container_panel(&rewo_world::menu_layout::PLAYER, None, EnchantPlayer::default(), None).is_none());
}

#[test]
fn a_lectern_paints_no_panel_rather_than_someone_elses() {
    // LecternScreen is a BookViewScreen. Falling through to a default
    // would paint some other menu's sheet behind a book.
    assert!(container_panel(layout(17), None, EnchantPlayer::default(), None).is_none());
}

#[test]
fn every_other_menu_resolves_to_a_sheet_in_the_atlas() {
    for id in 0..25 {
        let l = layout(id);
        if id == 17 {
            continue;
        }
        let p = container_panel(l, None, EnchantPlayer::default(), None).unwrap_or_else(|| panic!("{} has no panel", l.name));
        assert!(
            p.sheet < rewo_data::assets::MENU_BACKGROUND_TEXTURES.len(),
            "{} indexes past the atlas",
            l.name
        );
    }
}

#[test]
fn a_chest_is_two_blits_that_take_the_right_bands() {
    // generic_9x3: the top 3*18 + 17 = 71 px from the sheet's top, then
    // 96 px from v = 126. The gap between them is the rows a three-row
    // chest does not want.
    let p = container_panel(layout(2), None, EnchantPlayer::default(), None).unwrap();
    assert_eq!(p.blits.len(), 2);
    assert_eq!((p.gui_w, p.gui_h), (176.0, 168.0));
    assert_eq!((p.blits[0].dy, p.blits[0].sy, p.blits[0].h), (0.0, 0.0, 71.0));
    assert_eq!((p.blits[1].dy, p.blits[1].sy, p.blits[1].h), (71.0, 126.0, 96.0));
}

#[test]
fn the_merchants_source_pixels_come_back_off_a_512_sheet() {
    // The conversion this function exists for. menu_screen normalises the
    // merchant against 512, so multiplying by 512 must return the pixels
    // vanilla blits -- 0, 0, 276 wide. Multiplying by 256 (the other
    // twenty-one screens' sheet) would halve them.
    let p = container_panel(layout(19), None, EnchantPlayer::default(), None).unwrap();
    assert_eq!(p.blits.len(), 1);
    assert_eq!((p.blits[0].sx, p.blits[0].sy), (0.0, 0.0));
    assert_eq!(p.blits[0].w, 276.0);
    assert_eq!(p.gui_w, 276.0);
}

#[test]
fn every_screens_sheet_index_resolves() {
    // sheet_index returning None would mean the cross-check in rewo-world
    // had been removed; this is the same claim from the consuming side.
    for id in (0..25).filter(|&i| i != 17) {
        let s = rewo_world::menu_screen::screen_of(id).unwrap();
        assert!(sheet_index(s.texture).is_some(), "{}", s.texture);
    }
}

#[test]
fn slot_rects_follow_the_menus_own_panel() {
    // A six-row chest is 176x222; measuring its slots from a 176x166
    // origin would put every one of them 28 px low. Same window, two
    // menus, and the difference is exactly half the height difference.
    let (w, h) = (1280.0f32, 720.0f32);
    let chest = rewo_world::inventory::Inventory::with_layout(layout(5));
    let player = rewo_world::inventory::Inventory::default();
    let (_, ctop, scale) =
        rewo_gpu::container::gui_origin_for(w, h, 176.0, chest.layout().image_h as f32);
    let (_, ptop, _) = rewo_gpu::container::gui_origin(w, h);
    assert!(ctop < ptop, "the taller panel starts higher");
    let cr = menu_slot_rects(&chest, w, h, false);
    let pr = menu_slot_rects(&player, w, h, false);
    assert_eq!(cr.len(), 90);
    assert_eq!(pr.len(), 46);
    // Slot 0 of each sits at its own layout's first position.
    assert_eq!(cr[0].1, ctop + 18.0 * scale, "chest grid starts at y=18");
    assert_eq!(pr[0].1, ptop + 28.0 * scale, "player's result slot at y=28");
}

#[test]
fn stale_mesh_output_is_rejected_by_generation() {
    assert!(!mesh_output_is_stale(7, 7));
    assert!(mesh_output_is_stale(6, 7));
    assert!(mesh_output_is_stale(u64::MAX, 0));
}

#[test]
fn resolve_allay_dance_gates_on_kind() {
    use rewo_world::entities::{EntityState, EntityTable};
    let mut t = EntityTable::default();
    t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
    t.set_dancing(1, true);
    t.tick_lerp();
    // The Allay kind resolves the live dance from the counters.
    assert!(resolve_allay_dance(EntityModelKind::Allay, &t, 1, 1.0).is_some());
    // A non-Allay kind is inert even though the entity carries a dance clock
    // — the kind gate must survive the extraction.
    assert!(resolve_allay_dance(EntityModelKind::Zombie, &t, 1, 1.0).is_none());
    // No dance entry at all → None regardless of kind.
    assert!(resolve_allay_dance(EntityModelKind::Allay, &t, 999, 1.0).is_none());
}

#[test]
fn resolve_attack_anim_extracts_the_armed_render_state() {
    use rewo_data::swing_anim::{SwingAnimation, SwingAnimationType};
    use rewo_gpu::mobs::SwingKind;
    use rewo_world::entities::{
        EntityState, EntityTable, HandItem, HeldItem, InteractionHand,
    };
    let mut t = EntityTable::default();
    t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
    // Nothing has happened: the neutral pose, and never `None` — vanilla's
    // render state always carries these fields.
    let idle = resolve_attack_anim(&t, 1, 1.0);
    assert_eq!(idle.attack_time, 0.0);
    assert!(!idle.left_arm);
    assert_eq!(idle.kind, SwingKind::Whack, "bare hand = SwingAnimation.DEFAULT");
    assert_eq!(idle.age_scale, 1.0);
    // A spear in the off hand + an off-hand swing: the attack arm flips and
    // the type comes from the item held by *that arm*.
    t.set_hand_item(
        1,
        InteractionHand::OffHand,
        HandItem::Held(HeldItem {
            item_id: 1329,
            swing: SwingAnimation::new(SwingAnimationType::Stab, 19),
            use_profile: rewo_data::use_item::UseProfile::UNUSABLE,
            charged: false,
            glint: false,
        }),
    );
    t.swing(1, InteractionHand::OffHand, true);
    t.tick_lerp();
    t.tick_lerp();
    let a = resolve_attack_anim(&t, 1, 1.0);
    assert!(a.left_arm, "off-hand swing → the opposite of the RIGHT main arm");
    assert_eq!(a.kind, SwingKind::Stab);
    assert!((a.attack_time - 1.0 / 19.0).abs() < 1e-6, "{}", a.attack_time);
    // A baby's `getAgeScale()` halves the arm-pivot swing.
    t.set_baby(1, true);
    assert_eq!(resolve_attack_anim(&t, 1, 1.0).age_scale, 0.5);
    assert!(resolve_attack_anim(&t, 1, 1.0).inputs_known);
    // An unresolvable hand suppresses the whole pose rather than guessing.
    t.set_hand_item(1, InteractionHand::MainHand, HandItem::Unknown);
    let sup = resolve_attack_anim(&t, 1, 1.0);
    assert!(!sup.inputs_known);
    assert_eq!(sup.attack_time, 0.0, "suppressed, not guessed");
}

/// The three built-in dimension types, with exactly the fields M16's light
/// resolution reads, transcribed from
/// `%APPDATA%/EwoClient/rewo/26.2/decompiled/data/minecraft/dimension_type/`.
/// Everything else comes from `unresolved_holder` (Overworld-shaped), so
/// each fixture states precisely what it depends on.
fn dim(
    name: &str,
    has_fixed_time: bool,
    skybox: Skybox,
    ambient: u32,
    sky_light_color: u32,
    sky_light_factor: f32,
) -> DimensionTypeDef {
    DimensionTypeDef {
        name: name.into(),
        has_fixed_time,
        has_day_timeline: !has_fixed_time,
        skybox,
        ambient_light_color: ambient as i32,
        sky_light_color: sky_light_color as i32,
        sky_light_factor,
        ..DimensionTypeDef::unresolved_holder(0)
    }
}

/// `overworld.json`: no `has_fixed_time`, no `skybox` (→ codec default
/// OVERWORLD), ambient `#0a0a0a`, and NO `sky_light_color` /
/// `sky_light_factor` attribute — so both take the codec defaults.
fn overworld() -> DimensionTypeDef {
    dim(
        "minecraft:overworld",
        false,
        Skybox::Overworld,
        0xFF0A_0A0A,
        0xFFFF_FFFF,
        1.0,
    )
}

/// `the_nether.json`: `has_fixed_time: true`, `skybox: "none"`, ambient
/// `#302821`, `sky_light_color: "#7a7aff"`, `sky_light_factor: 0.0`.
fn nether() -> DimensionTypeDef {
    dim(
        "minecraft:the_nether",
        true,
        Skybox::None,
        0xFF30_2821,
        0xFF7A_7AFF,
        0.0,
    )
}

/// `the_end.json`: `has_fixed_time: true`, `skybox: "end"`, ambient
/// `#3f473f`, `sky_light_color: "#ac60cd"`, `sky_light_factor: 0.0`.
fn the_end() -> DimensionTypeDef {
    dim(
        "minecraft:the_end",
        true,
        Skybox::End,
        0xFF3F_473F,
        0xFFAC_60CD,
        0.0,
    )
}

/// A snapshot with no active effects and a given player tick count.
fn no_effects(tick_count: i32) -> VisualEffectSnapshot {
    VisualEffectSnapshot {
        night_vision_duration: None,
        darkness_blend_factor: 0.0,
        tick_count,
    }
}

/// `ARGB.vector3fFromRGB24`, restated: a plain `/255` of the low 24 bits.
fn rgb24(argb: i32) -> [f32; 3] {
    [
        ((argb >> 16) & 0xFF) as f32 / 255.0,
        ((argb >> 8) & 0xFF) as f32 / 255.0,
        (argb & 0xFF) as f32 / 255.0,
    ]
}

/// The flash is **added** to `skyFactor`
/// (`LightmapRenderStateExtractor.java:60-64`), and that matters most in
/// the one dimension that has it: `the_end.json` sets
/// `sky_light_factor: 0.0`, so the dimension's own sky term is zero and
/// the flash is the *only* thing lighting the sky there. A multiply — the
/// natural reading of "the timeline tracks are multipliers over the
/// dimension's base", which is the comment directly above the line — gives
/// zero at every intensity, and the End never brightens.
#[test]
fn the_flash_is_added_not_multiplied_which_is_the_whole_of_the_end() {
    let mut end = DimensionTypeDef::unresolved_holder(0);
    end.skybox = Skybox::End;
    end.sky_light_factor = 0.0;
    end.has_day_timeline = false;

    let dark = resolve_lightmap(
        None,
        Some(&end),
        1.4,
        no_effects(0),
        0.5,
        1.0,
        0.0,
        EndFlash::none(),
    );
    assert_eq!(dark.sky_factor, 0.0, "the End's own sky term is zero");

    let lit = resolve_lightmap(
        None,
        Some(&end),
        1.4,
        no_effects(0),
        0.5,
        1.0,
        0.0,
        EndFlash {
            intensity: 0.75,
            boss_world_fog: false,
        },
    );
    assert_eq!(lit.sky_factor, 0.75, "so the flash is all of it");
}

/// The boss-fog arm **divides by three**; it does not suppress. A wither
/// or dragon bar carrying `FLAG_FOG` dims the flash and keeps it.
#[test]
fn boss_world_fog_thirds_the_flash_rather_than_hiding_it() {
    let plain = EndFlash {
        intensity: 0.9,
        boss_world_fog: false,
    };
    let fogged = EndFlash {
        intensity: 0.9,
        boss_world_fog: true,
    };
    assert_eq!(plain.sky_factor_bonus(), 0.9);
    assert!((fogged.sky_factor_bonus() - 0.3).abs() < 1e-6);
    assert!(fogged.sky_factor_bonus() > 0.0, "dimmed, not suppressed");
}

/// The neutral value really is neutral, at every base factor — the
/// property every other lightmap fixture in this module now depends on.
#[test]
fn no_flash_changes_nothing() {
    for base in [0.0f32, 0.24, 1.0] {
        let mut dim = DimensionTypeDef::unresolved_holder(0);
        dim.sky_light_factor = base;
        dim.has_day_timeline = false;
        let s = resolve_lightmap(
            None,
            Some(&dim),
            1.4,
            no_effects(0),
            0.5,
            1.0,
            0.0,
            EndFlash::none(),
        );
        assert_eq!(s.sky_factor, base, "base {base}");
    }
    assert_eq!(EndFlash::none().sky_factor_bonus(), 0.0);
    assert_eq!(EndFlash::hidden().sky_factor_bonus(), 0.0);
}

#[test]
fn neutral_state_with_gamma_half() {
    // No day/night clock, resting flicker (1.4), no effects, gamma 0.5:
    // full daylight sky, gamma flows straight through to brightness, and
    // every effect term is off.
    let s = resolve_lightmap(None, None, 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    assert_eq!(s.sky_factor, 1.0);
    assert_eq!(s.block_factor, 1.4);
    assert_eq!(s.sky_light_color, [1.0, 1.0, 1.0]);
    assert_eq!(s.brightness_factor, 0.5);
    assert_eq!(s.darkness_scale, 0.0);
    assert_eq!(s.night_vision_factor, 0.0);
}

#[test]
fn night_vision_duration_drives_the_factor() {
    // Absent → 0.
    assert_eq!(
        resolve_lightmap(None, None, 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none()).night_vision_factor,
        0.0
    );
    // Infinite (`-1`) and > 200 ticks both pin to the full 1.0 seed.
    let inf = VisualEffectSnapshot {
        night_vision_duration: Some(-1),
        ..no_effects(0)
    };
    assert_eq!(
        resolve_lightmap(None, None, 1.4, inf, 0.5, 1.0, 0.0, EndFlash::none()).night_vision_factor,
        1.0
    );
    let long = VisualEffectSnapshot {
        night_vision_duration: Some(400),
        ..no_effects(0)
    };
    assert_eq!(
        resolve_lightmap(None, None, 1.4, long, 0.5, 1.0, 0.0, EndFlash::none()).night_vision_factor,
        1.0
    );
    // Within the last 200 ticks it pulses below 1.0 (the fade-out).
    let ending = VisualEffectSnapshot {
        night_vision_duration: Some(200),
        ..no_effects(0)
    };
    let nv = resolve_lightmap(None, None, 1.4, ending, 0.5, 1.0, 0.0, EndFlash::none()).night_vision_factor;
    assert!(
        nv > 0.0 && nv < 1.0,
        "expected a pulsing NV factor, got {nv}"
    );
}

#[test]
fn darkness_lowers_brightness_and_raises_scale() {
    // Partial darkness (blend 0.3) at the pulse peak (tick 0, cos = 1):
    // brightness drops below gamma, darkness scale goes positive.
    let partial_dark = VisualEffectSnapshot {
        darkness_blend_factor: 0.3,
        ..no_effects(0)
    };
    let s = resolve_lightmap(None, None, 1.4, partial_dark, 0.5, 1.0, 0.0, EndFlash::none());
    assert!(
        s.brightness_factor < 0.5,
        "brightness {} should dip below gamma",
        s.brightness_factor
    );
    assert!(
        s.brightness_factor > 0.0,
        "brightness {} should stay positive",
        s.brightness_factor
    );
    assert!(
        s.darkness_scale > 0.0,
        "darkness {} should be positive",
        s.darkness_scale
    );

    // Full darkness (blend 1.0): brightness floors to 0 (gamma - 1), and
    // the darkness subtraction maxes at 0.45 * option. This pins the
    // brightness-vs-darkness ordering: as darkness climbs, brightness sinks.
    let full_dark = VisualEffectSnapshot {
        darkness_blend_factor: 1.0,
        ..no_effects(0)
    };
    let s = resolve_lightmap(None, None, 1.4, full_dark, 0.5, 1.0, 0.0, EndFlash::none());
    assert_eq!(s.brightness_factor, 0.0);
    assert!(
        (s.darkness_scale - 0.45).abs() < 1e-4,
        "darkness {} ~ 0.45",
        s.darkness_scale
    );
    assert!(s.darkness_scale > s.brightness_factor);
}

#[test]
fn day_ticks_drive_the_sky_half() {
    // Midnight (tick 18000) dims and blues the sky half, independent of
    // the block factor (a torch stays as bright).
    let s = resolve_lightmap(Some(18000), None, 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    assert_eq!(s.sky_factor, 0.24);
    assert_eq!(s.sky_light_color, [0.48, 0.48, 1.0]);
    assert_eq!(s.block_factor, 1.4);
}

#[test]
fn world_lightmap_conversion_is_field_for_field() {
    let s = resolve_lightmap(Some(18000), None, 1.7, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    let w = to_world_lightmap(&s);
    assert_eq!(w.sky_factor, s.sky_factor);
    assert_eq!(w.block_factor, s.block_factor);
    assert_eq!(w.sky_color, s.sky_light_color);
    assert_eq!(w.ambient_color, s.ambient_color);
    assert_eq!(w.brightness_factor, s.brightness_factor);
    assert_eq!(w.darkness_scale, s.darkness_scale);
    assert_eq!(w.night_vision_factor, s.night_vision_factor);
}

/// A transition-like sequence of consecutive resolves — Overworld → Nether
/// → End → Overworld — with the world clock *still running* underneath, as
/// it does across a real `respawn`. Every frame must depend only on the
/// dimension it was given: no term may carry over from the previous one.
///
/// This is the failure `resolve_lightmap` exists to make impossible. It is
/// a pure function, so the proof is that each step equals the same step
/// computed in isolation, and that returning to the Overworld reproduces
/// the Overworld's own value bit-for-bit.
#[test]
fn dimension_transitions_leave_no_stale_term() {
    let (ow, ne, en) = (overworld(), nether(), the_end());
    // The clock keeps ticking across the transitions.
    let ticks = [1000i64, 18000, 6000, 23000];
    let seq = [Some(&ow), Some(&ne), Some(&en), Some(&ow)];

    let mut states = Vec::new();
    for (t, d) in ticks.iter().zip(seq) {
        states.push(resolve_lightmap(
            Some(*t),
            d,
            1.4,
            no_effects(0),
            0.5,
            1.0,
            0.0,
            EndFlash::none(),
        ));
    }

    // 1. Each step matches the isolated computation for its own inputs.
    for (i, (t, d)) in ticks.iter().zip(seq).enumerate() {
        let isolated = resolve_lightmap(Some(*t), d, 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
        assert_eq!(states[i], isolated, "step {i} depends on history");
    }

    // 2. The two non-Overworld steps are exactly their registry values,
    //    even though they were preceded by a lit Overworld frame.
    assert_eq!(states[1].sky_factor, 0.0);
    assert_eq!(states[1].ambient_color, rgb24(0xFF30_2821u32 as i32));
    assert_eq!(states[1].sky_light_color, rgb24(0xFF7A_7AFFu32 as i32));
    assert_eq!(states[2].sky_factor, 0.0);
    assert_eq!(states[2].ambient_color, rgb24(0xFF3F_473Fu32 as i32));
    assert_eq!(states[2].sky_light_color, rgb24(0xFFAC_60CDu32 as i32));

    // 2b. Reject the specific "the timeline got multiplied in anyway"
    //     failure by name: step 1 sits at midnight, where the Overworld
    //     tracks are (0.24, [0.48, 0.48, 1.0]). A leaked multiply would not
    //     be zero or default — it would be a plausible-looking third
    //     colour, which is exactly why it needs its own assertion.
    for (i, base) in [(1usize, 0xFF7A_7AFFu32 as i32)] {
        let leaked: [f32; 3] = std::array::from_fn(|c| rgb24(base)[c] * [0.48, 0.48, 1.0][c]);
        assert_ne!(
            states[i].sky_light_color, leaked,
            "step {i} leaked the clock"
        );
    }

    // 3. Back in the Overworld at tick 23000 the timeline drives the sky
    //    again — the End's factor-0 must not have stuck.
    let fresh = resolve_lightmap(Some(23000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    assert_eq!(states[3], fresh);
    assert!(
        states[3].sky_factor > 0.0,
        "the Overworld sky came back dead: {}",
        states[3].sky_factor
    );
    assert_eq!(states[3].ambient_color, rgb24(0xFF0A_0A0Au32 as i32));

    // 4. And the sky *mode* follows the same sequence, so a transition can
    //    never leave the previous world's skybox on screen.
    let modes: Vec<SkyMode> = seq.iter().map(|d| sky_mode_of(*d)).collect();
    assert_eq!(
        modes,
        vec![
            SkyMode::Overworld,
            SkyMode::None,
            SkyMode::End,
            SkyMode::Overworld
        ]
    );
}

/// No dimension resolved yet (pre-login, or any serverless path) must give
/// exactly the pre-M16 inputs: attribute defaults, day timeline on.
#[test]
fn unresolved_dimension_is_the_pre_m16_default() {
    let d = dimension_light(None);
    assert_eq!(d, DimensionLight::UNRESOLVED);
    assert_eq!(d.ambient_color, [0.0, 0.0, 0.0]);
    assert_eq!(d.sky_light_color, [1.0, 1.0, 1.0]);
    assert_eq!(d.sky_light_factor, 1.0);
    assert!(d.day_timeline);
    assert_eq!(sky_mode_of(None), SkyMode::Overworld);
    // And the resolved lightmap is unchanged from the legacy call.
    let s = resolve_lightmap(Some(18000), None, 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    assert_eq!(s.sky_factor, 0.24);
    assert_eq!(s.sky_light_color, [0.48, 0.48, 1.0]);
    assert_eq!(s.ambient_color, [0.0, 0.0, 0.0]);
}

/// The Overworld keeps the day timeline, and its ambient is the dimension
/// attribute `#0a0a0a` — NOT the codec default black the serverless paths
/// use. The two must be distinguishable, or the field is doing nothing.
#[test]
fn overworld_keeps_the_day_timeline_and_gains_its_ambient() {
    let ow = overworld();
    let d = dimension_light(Some(&ow));
    assert!(d.day_timeline);
    assert_eq!(d.ambient_color, [10.0 / 255.0; 3]);
    assert_eq!(sky_mode_of(Some(&ow)), SkyMode::Overworld);

    // Noon: the timeline multiplier is 1.0 over the 1.0 base.
    let noon = resolve_lightmap(Some(6000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    assert_eq!(noon.sky_factor, 1.0);
    assert_eq!(noon.sky_light_color, [1.0, 1.0, 1.0]);
    // Midnight: 1.0 * 0.24, white * (0.48, 0.48, 1.0) — the legacy values.
    let mid = resolve_lightmap(Some(18000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    assert_eq!(mid.sky_factor, 0.24);
    assert_eq!(mid.sky_light_color, [0.48, 0.48, 1.0]);
    // Ambient is constant across the cycle (no timeline track keyframes it).
    assert_eq!(noon.ambient_color, mid.ambient_color);
    assert_eq!(mid.ambient_color, [10.0 / 255.0; 3]);
    assert_ne!(mid.ambient_color, [0.0; 3]);
}

/// The Nether and the End are fixed-time: the Overworld day timeline must
/// not touch them, so their exact `sky_light_factor` / `sky_light_color`
/// attributes survive at any world-clock tick.
#[test]
fn fixed_time_dimensions_ignore_the_overworld_clock() {
    for (def, ambient, sky, mode) in [
        (
            nether(),
            [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0],
            [122.0 / 255.0, 122.0 / 255.0, 1.0],
            SkyMode::None,
        ),
        (
            the_end(),
            [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0],
            [172.0 / 255.0, 96.0 / 255.0, 205.0 / 255.0],
            SkyMode::End,
        ),
    ] {
        let d = dimension_light(Some(&def));
        assert!(!d.day_timeline, "{} must be fixed-time", def.name);
        assert_eq!(sky_mode_of(Some(&def)), mode, "{} skybox", def.name);

        // Every tick of the day, and with no clock at all, resolves the same.
        let mut seen = Vec::new();
        for t in [
            None,
            Some(0),
            Some(6000),
            Some(13000),
            Some(18000),
            Some(23999),
        ] {
            let s = resolve_lightmap(t, Some(&def), 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
            assert_eq!(s.sky_factor, 0.0, "{} sky factor at {t:?}", def.name);
            assert_eq!(s.sky_light_color, sky, "{} sky colour at {t:?}", def.name);
            assert_eq!(s.ambient_color, ambient, "{} ambient at {t:?}", def.name);
            // The sky/fog gradient multiplier is gated on the same test, so
            // a midnight clock cannot black out the End's sky either.
            let tl = dimension_timeline(t, &d);
            assert_eq!(tl, rewo_world::daylight::SkyLighting::DAY);
            seen.push(s);
        }
        assert!(seen.windows(2).all(|w| w[0] == w[1]));
    }
}

/// The exact leak this guards: at midnight the Overworld resolves a dim,
/// blue sky half and a black sky gradient. Respawning into the Nether with
/// the SAME `day_ticks` must not carry any of that across.
#[test]
fn overworld_midnight_does_not_leak_across_a_respawn() {
    const MIDNIGHT: Option<i64> = Some(18000);
    let ow = overworld();
    let nether = nether();
    let before = resolve_lightmap(MIDNIGHT, Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    let after = resolve_lightmap(MIDNIGHT, Some(&nether), 1.4, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    assert_eq!(
        before.sky_light_color,
        [0.48, 0.48, 1.0],
        "the OW night tint"
    );
    // The Nether's own #7a7aff, not the night-multiplied version of it.
    assert_eq!(after.sky_light_color, [122.0 / 255.0, 122.0 / 255.0, 1.0]);
    assert_ne!(after.sky_light_color, before.sky_light_color);
    assert_eq!(after.sky_factor, 0.0);
    assert_ne!(after.ambient_color, before.ambient_color);

    // The gradient tint too: midnight blacks the Overworld sky, the Nether
    // (and End) must stay at the identity multiplier.
    let ow_tl = dimension_timeline(MIDNIGHT, &dimension_light(Some(&ow)));
    assert_eq!(ow_tl.sky_color, [0.0, 0.0, 0.0], "OW midnight sky is black");
    let nether_tl = dimension_timeline(MIDNIGHT, &dimension_light(Some(&nether)));
    assert_eq!(nether_tl.sky_color, [1.0, 1.0, 1.0]);
    assert_eq!(nether_tl.fog_color, [1.0, 1.0, 1.0]);
}

/// The ambient survives the CPU→GPU conversion, and the conversion carries
/// every field (a missed field would default-initialize to something bland).
#[test]
fn world_lightmap_conversion_carries_the_ambient() {
    let end = the_end();
    let s = resolve_lightmap(Some(18000), Some(&end), 1.7, no_effects(0), 0.5, 1.0, 0.0, EndFlash::none());
    let w = to_world_lightmap(&s);
    assert_eq!(w.ambient_color, s.ambient_color);
    assert_eq!(w.ambient_color, [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0]);
    assert_ne!(w.ambient_color, WorldLightmapState::default().ambient_color);
}

/// `DimensionType.Skybox` → the renderer's mode, both ways round.
#[test]
fn sky_mode_maps_every_skybox() {
    assert_eq!(sky_mode_of(Some(&overworld())), SkyMode::Overworld);
    assert_eq!(sky_mode_of(Some(&nether())), SkyMode::None);
    assert_eq!(sky_mode_of(Some(&the_end())), SkyMode::End);
    // An unresolved holder degrades to the codec default, not to NONE.
    let unresolved = DimensionTypeDef::unresolved_holder(9);
    assert_eq!(unresolved.skybox, Skybox::DEFAULT);
    assert_eq!(sky_mode_of(Some(&unresolved)), SkyMode::Overworld);
}

#[test]
fn validate_unit_bounds() {
    assert!(validate_unit("gamma", 0.0).is_ok());
    assert!(validate_unit("gamma", 1.0).is_ok());
    assert!(validate_unit("gamma", 0.5).is_ok());
    assert!(validate_unit("gamma", -0.01).is_err());
    assert!(validate_unit("gamma", 1.01).is_err());
    assert!(validate_unit("gamma", f32::NAN).is_err());
    assert!(validate_unit("gamma", f32::INFINITY).is_err());
}

// -- M33: weather and clouds --------------------------------------------------
