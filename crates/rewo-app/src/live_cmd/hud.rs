use super::*;

/// Drain the session's chat events into its [`rewo_world::chat::ChatComponent`].
///
/// The seam exists because the store needs two things the wire cannot supply:
/// the font to wrap against (`addMessageToDisplayQueue` calls
/// `message.splitLines(font, maxWidth)`) and the GUI tick to stamp
/// `addedTime` with. `session.ticks` is the 20 Hz session tick, which is the
/// same rate and the same purpose as vanilla's `Gui.getGuiTicks()`.
///
/// **With no font the events are still drained**, and a zero width is the
/// right measurement rather than a placeholder. `LineBreakFinder`'s
/// `hadNonZeroWidthChar` guard only lets the overflow test fire *after* a
/// character of non-zero width has been accepted, so a font that measures
/// everything at 0 never overflows and the message stays whole — one line per
/// `\n`, no wrapping. Dropping the events instead would lose them
/// permanently and queueing them would grow without bound, so keeping them
/// unwrapped is what a caller with no font can honestly render.
pub(super) fn apply_chat(session: &mut PlaySession, advance: Option<[u8; 256]>) {
    let width_of = move |t: &str, st: rewo_world::chat_style::ChatStyle| match &advance {
        Some(a) => rewo_gpu::text::width_styled(t, a, st.bold),
        None => 0,
    };
    let ctx = rewo_world::chat::WrapContext {
        options: rewo_world::chat::ChatOptions::default(),
        focused: false,
        width_of: &width_of,
        deleted_marker_text: DELETED_CHAT_MESSAGE,
    };
    let tick = session.ticks as i32;
    session.apply_chat_events(tick, &ctx);
}

/// `-2039584` — `EditBox.textColor`'s default, and `DebugScreenOverlay`'s
/// literal for every F3 line.
///
/// One constant because vanilla writes the same number in both places, not
/// because Rewo chose to share it: `EditBox` declares
/// `private int textColor = -2039584;` and `DebugScreenOverlay.renderLines`
/// calls `graphics.text(this.font, line, left, top, -2039584, false)`.
///
/// It is **not** white. Every text surface that wanted "the off-white vanilla
/// uses" and reached for a literal instead got a different wrong number.
pub(crate) const EDIT_BOX_TEXT_COLOR: u32 = 0xE0_E0E0;

/// `chat.deleted_marker`'s English value.
///
/// **The literal, not a `baked.lang` lookup**, and that is a divergence rather
/// than a shortcut: vanilla resolves the key and also styles the marker
/// `GRAY, ITALIC`, neither of which this path does. The style is unreachable
/// (the HUD's text producer takes one colour per line and no italic), and the
/// lookup is reachable — `rewo_data`'s language map is loaded — but would give
/// the same string on the English-only build Rewo ships. Named here so it is a
/// known one-line change when the HUD grows spans, not an oversight.
pub(super) const DELETED_CHAT_MESSAGE: &str = "This chat message has been deleted by the server.";

/// Build this frame's overlay text: an F3-style debug block (top-left, when
/// `debug`) and the chat box (above the hotbar). GUI scale
/// `px`; `fps` is shown in the header when known (windowed only).
pub(super) fn build_text(
    session: &PlaySession,
    px: f32,
    screen_h: f32,
    fps: Option<f32>,
    debug: bool,
    chat_focused: bool,
    advance: Option<[u8; 256]>,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(String, usize)>,
    std::ops::Range<usize>,
) {
    use rewo_gpu::world::OwnedTextLine;
    // `DebugScreenOverlay.renderLines` — `graphics.text(font, line, left, top,
    // -2039584, false)`, the same [`EDIT_BOX_TEXT_COLOR`] the chat input uses.
    //
    // It was `[0.93; 3]` from the first screen-space-text commit until M130 —
    // an invented near-white that is neither the byte (`224/255 = 0.878`) nor
    // its linear form (`0.745`). Two independent errors in one literal, which
    // is what a value nobody derived looks like.
    let white = srgb_bytes_to_linear(EDIT_BOX_TEXT_COLOR);
    let mut lines = Vec::new();
    // F3 debug block (top-left). Toggled by F3 in the windowed client;
    // always on in headless so a verification PNG shows the state.
    if debug {
        let p = &session.player;
        let (bx, by, bz) = (p.x.floor() as i64, p.y.floor() as i64, p.z.floor() as i64);
        // Chunk-relative block coords (0..15) — Rust's rem_euclid keeps them
        // non-negative in the negative hemisphere, matching vanilla's F3.
        let (rbx, rbz) = (bx.rem_euclid(16), bz.rem_euclid(16));
        let (cx, cz) = (bx.div_euclid(16), bz.div_euclid(16));
        let toward = facing_axis(p.yaw, p.pitch);
        let header = match fps {
            Some(f) => format!("Rewo 26.2   {f:.0} fps"),
            None => "Rewo 26.2".to_string(),
        };
        let f3 = [
            header,
            format!("XYZ: {:.3} / {:.3} / {:.3}", p.x, p.y, p.z),
            format!(
                "Block: {bx} {by} {bz}   [{rbx} {} {rbz}]",
                by.rem_euclid(16)
            ),
            format!("Chunk: {cx} {cz}"),
            {
                // Vanilla F3's "Client Light" — sampled at the eye, the same
                // cell entity lighting uses.
                let (bl, sl) =
                    session
                        .world
                        .light_at(bx as i32, p.eye_y().floor() as i32, bz as i32);
                format!("Light: {} ({sl} sky, {bl} block)", bl.max(sl))
            },
            format!(
                "Facing: {} {}  ({:.1} / {:.1})",
                compass(p.yaw),
                toward,
                p.yaw,
                p.pitch
            ),
            format!(
                "Loaded: {} chunks   Entities: {}   {}",
                session.world.loaded_columns(),
                session.world.entities.len(),
                if p.on_ground { "grounded" } else { "airborne" },
            ),
        ];
        for (i, text) in f3.into_iter().enumerate() {
            lines.push(OwnedTextLine {
                x: 3.0 * px,
                y: (3.0 + i as f32 * 10.0) * px,
                px,
                color_linear: white,
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text,
            });
        }
    }
    let (chat, chat_rows) = chat_lines(
        &session.chat,
        session.ticks as i32,
        px,
        screen_h,
        &rewo_world::chat::ChatOptions::default(),
        advance.as_ref(),
        chat_focused,
    );
    // The range the chat occupies, so a witness can read the lines that were
    // DRAWN rather than re-deriving `chat_lines` (M93q). Chat is appended last
    // here; the caller extends further afterwards, which is why this is a
    // range and not "the tail".
    let chat_range = lines.len()..lines.len() + chat.len();
    lines.extend(chat);
    (lines, chat_rows, chat_range)
}

/// The chat screen's input line and caret (M110), in screen pixels.
///
/// `EditBox(font, 4, height - 12, width - 4, 12)` with `setBordered(false)`,
/// so there is no widget chrome — the only fill is the screen's own bar, which
/// [`chat_input_backdrop`] emits, and the text sits at the box's own inset.
///
/// **An untouched restored draft renders grey and italic**
/// (`formatChat` returns `Style.EMPTY.withColor(GRAY).withItalic(true)` while
/// `isDraft`), which is the visual half of the rule that backspace clears the
/// whole field. Rewo's bitmap text pass carries one colour per line and no
/// slant, so the colour is reproduced and the italic is not — named here
/// rather than silently dropped.
pub(super) fn chat_input_lines(
    screen: &rewo_world::chat_screen::ChatScreen,
    px: f32,
    gui_w: i32,
    gui_h: i32,
    now_ms: u64,
    runs: Option<&[rewo_net::command_format::Run]>,
    width_of: &dyn Fn(&str) -> i32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_gpu::world::OwnedTextLine;
    let (x, y, _w, h) = rewo_world::chat_screen::input_rect(gui_w, gui_h);
    // `EditBox.renderWidget`'s unbordered text origin: the box's own x, and
    // vertically centred by `(height - 8) / 2` — 2 here, not 0 and not 3.
    let text_x = x as f32 * px;
    let text_y = (y + (h - 8) / 2) as f32 * px;
    // `ChatScreen.formatChat` — a FORMATTER, not a field colour:
    //
    // ```java
    // return this.isDraft
    //    ? FormattedCharSequence.forward(text, Style.EMPTY.withColor(ChatFormatting.GRAY).withItalic(true))
    //    : null;
    // ```
    //
    // so a draft is GRAY **and italic**, and null means "no formatter", i.e.
    // the field's own `textColor` — not white.
    //
    // **The draft grey does NOT reach the caret**, and the mechanism is why:
    // `graphics.text(font, applyFormat(half, …), drawX, textY, color, …)`
    // hands the formatter's style a chance to override `color` per character,
    // where `TextCursorUtils.extractAppendCursor` draws a bare `"_"` **String**
    // with `color` itself. So on a draft the text greys and the caret stays
    // off-white. Two bindings, not one, for exactly that reason.
    let field_color = srgb_bytes_to_linear(EDIT_BOX_TEXT_COLOR);
    let (color, style) = if screen.is_draft() {
        (
            srgb_bytes_to_linear(0xAA_AAAA),
            rewo_gpu::text::TextStyle {
                italic: true,
                ..rewo_gpu::text::TextStyle::PLAIN
            },
        )
    } else {
        (field_color, rewo_gpu::text::TextStyle::PLAIN)
    };
    let value = screen.input.value();
    // M117 — one line per coloured run, laid out with the FONT's advances
    // rather than a fixed six pixels, because the runs must butt up against
    // each other exactly: a wrong width is a visible gap or an overlap, where
    // for the caret it was only ever a pixel or two of drift.
    let mut out: Vec<OwnedTextLine> = Vec::new();
    match runs {
        Some(runs) if !runs.is_empty() => {
            let mut x = text_x;
            for run in runs {
                out.push(OwnedTextLine {
                    x,
                    y: text_y,
                    px,
                    color_linear: srgb_bytes_to_linear(run.color),
                    alpha: 1.0,
                    shadow: true,
                    style: rewo_gpu::text::TextStyle::PLAIN,
                    text: run.text.clone(),
                });
                x += width_of(&run.text) as f32 * px;
            }
        }
        _ => out.push(OwnedTextLine {
            x: text_x,
            y: text_y,
            px,
            color_linear: color,
            alpha: 1.0,
            shadow: true,
            style,
            text: value.clone(),
        }),
    }
    let before: String = value.chars().take(screen.input.cursor_position()).collect();
    // Measured, not counted: the six-pixel approximation put the caret adrift
    // on any line with a narrow glyph in it (`i` is 2 wide, `l` 3), and the
    // ghost hangs off the caret so it drifted too.
    let caret_x = text_x + width_of(&before) as f32 * px;
    // The greyed ghost after the caret (M115), gated on `!insert` —
    // `cursorPos < value.length() || value.length() >= maxLength`. So it shows
    // only with the caret at the END of an under-length field: a ghost drawn
    // mid-string would sit on top of the text after the cursor. Vanilla puts
    // it at `cursorX - 1`, one pixel left of where the caret glyph goes.
    let insert = screen.input.cursor_position() < screen.input.len()
        || screen.input.len() >= screen.input.max_length();
    if !insert {
        if let Some(ghost) = screen.input.suggestion() {
            out.push(OwnedTextLine {
                x: caret_x - px,
                y: text_y,
                px,
                // `-8355712` — 0x808080, converted rather than written out:
                // the literal it replaces WAS the right number (0.216 is
                // `srgb_to_linear(128/255)` to three places), but a magic
                // constant is indistinguishable from a `/255` at a glance,
                // which is how the two colours above it stayed wrong.
                color_linear: srgb_bytes_to_linear(0x80_8080),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: ghost.to_string(),
            });
        }
    }
    // The caret, as the anvil's field draws it: a `_` at the cursor when the
    // blink says so. `setCanLoseFocus(false)` means it never stops.
    if screen.input.cursor_visible(now_ms) {
        out.push(OwnedTextLine {
            x: caret_x,
            y: text_y,
            px,
            // `extractAppendCursor(…, color, …)` — the FIELD colour, so the
            // caret does not follow the draft's grey (see the binding above).
            color_linear: field_color,
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: "_".to_string(),
        });
    }
    out
}

/// The suggestion popup's fills (M115), in screen pixels.
///
/// `SuggestionsList.extractRenderState`, fill half. Three kinds of rect and
/// the order is the order on screen:
///
/// 1. When the list is longer than its window, a **1 px bar above and below**
///    in the popup's own fill colour. Both are drawn whenever *either* end is
///    truncated — `limited` is `hasPrevious || hasNext` and gates the pair —
///    so a list scrolled to its top still gets a bar above it.
/// 2. The **dashes**, white, one pixel wide every two, on whichever end has
///    more entries. These are per-pixel `fill` calls in vanilla and stay
///    per-pixel here; merging them into one rect would draw a solid line.
/// 3. One **row fill** per visible line.
/// **It takes no GUI scale, deliberately** — see [`chat_input_backdrop`].
/// `SuggestionsList::rect` is already in GUI pixels, which is the unit
/// [`rewo_gpu::hud::HudFill`] is in. Its screen-pixel sibling
/// [`suggestion_popup_text`] does take one, because the text pass is the pass
/// that wants screen pixels.
pub(super) fn suggestion_popup_fills(
    list: &rewo_world::command_suggestions::SuggestionsList,
    cfg: rewo_world::command_suggestions::SuggestionsConfig,
) -> Vec<rewo_gpu::hud::HudFill> {
    use rewo_world::command_suggestions::{INDICATOR_COLOR, LINE_HEIGHT};
    let rect = list.rect;
    let limit = list.shown(cfg) as i32;
    let fill = |x: i32, y: i32, w: i32, h: i32, argb: u32| rewo_gpu::hud::HudFill {
        x: x as f32,
        y: y as f32,
        w: w as f32,
        h: h as f32,
        alpha: ((argb >> 24) & 0xFF) as f32 / 255.0,
        rgb: srgb_bytes_to_linear(argb & 0x00FF_FFFF),
    };
    let mut out = Vec::new();
    let has_previous = list.offset() > 0;
    let has_next = list.entries().len() > list.offset() + limit as usize;
    if has_previous || has_next {
        out.push(fill(rect.x, rect.y - 1, rect.w, 1, cfg.fill_color));
        out.push(fill(rect.x, rect.y + rect.h, rect.w, 1, cfg.fill_color));
        for (edge, present) in [(rect.y - 1, has_previous), (rect.y + rect.h, has_next)] {
            if !present {
                continue;
            }
            let mut x = 0;
            while x < rect.w {
                out.push(fill(rect.x + x, edge, 1, 1, INDICATOR_COLOR));
                x += 2;
            }
        }
    }
    for i in 0..limit {
        out.push(fill(
            rect.x,
            rect.y + LINE_HEIGHT * i,
            rect.w,
            LINE_HEIGHT,
            cfg.fill_color,
        ));
    }
    out
}

/// The suggestion popup's text (M115), in screen pixels.
///
/// One line per visible row at `rect.x + 1`, `rect.y + 2 + 12 * i`, yellow for
/// the selected entry and grey for the rest. `graphics.text`'s five-argument
/// form drops a shadow (M105), so these do too.
pub(super) fn suggestion_popup_text(
    list: &rewo_world::command_suggestions::SuggestionsList,
    cfg: rewo_world::command_suggestions::SuggestionsConfig,
    px: f32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_world::command_suggestions::{LINE_HEIGHT, SELECTED_COLOR, UNSELECTED_COLOR};
    let rect = list.rect;
    let limit = list.shown(cfg);
    (0..limit)
        .filter_map(|i| {
            let index = i + list.offset();
            let entry = list.entries().get(index)?;
            let argb = if index == list.current() {
                SELECTED_COLOR
            } else {
                UNSELECTED_COLOR
            };
            Some(rewo_gpu::world::OwnedTextLine {
                x: (rect.x + 1) as f32 * px,
                y: (rect.y + 2 + LINE_HEIGHT * i as i32) as f32 * px,
                px,
                color_linear: srgb_bytes_to_linear(argb & 0x00FF_FFFF),
                alpha: ((argb >> 24) & 0xFF) as f32 / 255.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: entry.text.clone(),
            })
        })
        .collect()
}

/// The coloured runs for the chat field, or `None` when there is nothing to
/// colour (M117).
///
/// `formatChat` returns null while `currentParse` is null, and
/// `updateCommandInfo` only ever builds one for a `/`-command — so an ordinary
/// chat message is drawn in the field's own colour, which is a state vanilla
/// passes through too.
///
/// A free function over the three pieces it needs rather than a method,
/// because the frame already holds the session borrowed and `&mut self` here
/// would take the whole app with it.
pub(super) fn chat_runs(
    cache: &mut Option<(String, rewo_net::dispatcher::ParseResults)>,
    screen: Option<&rewo_world::chat_screen::ChatScreen>,
    session: &PlaySession,
    cmd: rewo_net::dispatcher::CommandCtx<'_>,
) -> Option<Vec<rewo_net::command_format::Run>> {
    let value = screen?.input.value();
    if !value.starts_with('/') {
        *cache = None;
        return None;
    }
    if cache.as_ref().map(|(t, _)| t.as_str()) != Some(value.as_str()) {
        let units: Vec<u16> = value.encode_utf16().collect();
        let parsed = rewo_net::dispatcher::parse(&session.commands, &units, 1, cmd);
        *cache = Some((value.clone(), parsed));
    }
    let (_, parsed) = cache.as_ref()?;
    let units: Vec<u16> = value.encode_utf16().collect();
    // `offset` is `displayPos` in vanilla, because `EditBox` renders only the
    // visible substring. Rewo's chat field draws the whole value, so the
    // visible text starts at 0 — this must become `display_pos()` the day the
    // render honours the horizontal scroll, or every colour lands one scroll
    // to the left.
    Some(rewo_net::command_format::format_text(parsed, &units, 0))
}

/// The usage box under the chat field (M117), in screen pixels.
///
/// `CommandSuggestions.extractUsage`. Two things read backwards:
///
/// * The list grows **upward**. `lineY = height - 27 - 12 * y`, so entry 0 is
///   the LOWEST line and each later one sits twelve pixels higher. Laying it
///   out downward from a top puts a two-line box over the field it belongs to.
/// * The fill is one pixel wider than the text **on each side**
///   (`position - 1` to `position + width + 1`), so the box has a hairline of
///   padding the text does not.
///
/// The box and the suggestion popup are **mutually exclusive**:
/// `extractRenderState` is `if (!extractSuggestions(..)) extractUsage(..)`.
pub(super) fn usage_box(
    lines: &[rewo_net::command_format::UsageLine],
    position: i32,
    gui_h: i32,
    px: f32,
    fill_color: u32,
    width_of: &dyn Fn(&str) -> i32,
) -> (Vec<rewo_gpu::hud::HudFill>, Vec<rewo_gpu::world::OwnedTextLine>) {
    let box_width = lines.iter().map(|l| width_of(&l.text)).max().unwrap_or(0);
    let mut fills = Vec::new();
    let mut text = Vec::new();
    for (y, line) in lines.iter().enumerate() {
        let line_y = gui_h - rewo_world::command_suggestions::USAGE_OFFSET_FROM_BOTTOM
            - rewo_world::command_suggestions::LINE_HEIGHT * y as i32;
        fills.push(rewo_gpu::hud::HudFill {
            x: (position - 1) as f32 * px,
            y: line_y as f32 * px,
            w: (box_width + 2) as f32 * px,
            h: rewo_world::command_suggestions::LINE_HEIGHT as f32 * px,
            alpha: ((fill_color >> 24) & 0xFF) as f32 / 255.0,
            rgb: srgb_bytes_to_linear(fill_color & 0x00FF_FFFF),
        });
        text.push(rewo_gpu::world::OwnedTextLine {
            x: position as f32 * px,
            y: (line_y + 2) as f32 * px,
            px,
            // **Per line, not per box.** `extractUsage` passes one `-1` for
            // the lot and `Font.getTextColor` uses it only as a default, so
            // the style baked into each `FormattedCharSequence` decides:
            // `USAGE_FORMAT`'s grey on a usage entry, nothing at all on an
            // exception message, which therefore keeps the white. This drew
            // every line white from M117 until M134, when the second kind of
            // line arrived and made the difference observable.
            //
            // The field is `color_linear` since M130, which renamed it to say
            // which space it is in. Taking M130's side of this conflict would
            // have compiled and quietly restored the constant white.
            color_linear: srgb_bytes_to_linear(line.color),
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: line.text.clone(),
        });
    }
    (fills, text)
}

/// The chat scrollbar's two rects (M111), in screen pixels.
///
/// Reachable only while the chat screen is open, so the caller gates it — and
/// the visible-line count is `forEachLine`'s own return, which is why it comes
/// from the same `visible_lines` the rows and their fills read rather than
/// from `linesPerPage`: a page that is not full gives a shorter thumb.
pub(super) fn chat_scrollbar(
    chat: &rewo_world::chat::ChatComponent,
    gui_tick: i32,
    px: f32,
    screen_h: f32,
    opts: &rewo_world::chat::ChatOptions,
) -> Vec<rewo_gpu::hud::HudFill> {
    let chat_px = px * opts.scale as f32;
    let chat_bottom = ((screen_h / chat_px) - rewo_world::chat::BOTTOM_MARGIN as f32).floor();
    let max_width = (opts.width() as f32 / opts.scale as f32).ceil();
    let visible = chat.visible_lines(gui_tick, true, opts).len() as i32;
    // GUI pixels, exactly as in `hud_fills` and for the same reason — `chat_px`
    // converts the screen height into chat space above and is not what the fill
    // list wants.
    let chat_gui = opts.scale as f32;
    chat.scrollbar(visible, chat_bottom as i32, max_width as i32, opts)
        .into_iter()
        .flatten()
        .map(|r| rewo_gpu::hud::HudFill {
            // The pose is translated by `MESSAGE_INDENT` before these, exactly
            // as it is before the rows' fills.
            x: (r.x + rewo_world::chat::MESSAGE_INDENT) as f32 * chat_gui,
            y: r.y as f32 * chat_gui,
            w: r.w as f32 * chat_gui,
            h: r.h as f32 * chat_gui,
            alpha: r.alpha as f32 / 255.0,
            rgb: srgb_bytes_to_linear(r.rgb),
        })
        .collect()
}

/// The bar behind the input, as a [`rewo_gpu::hud::HudFill`].
///
/// `fill(2, height - 14, width - 2, height - 2, getBackgroundColor(Integer.MIN_VALUE))`
/// — a fixed alpha 128, which does **not** follow the text-background slider
/// the chat rows' own fills read. See
/// [`rewo_world::chat_screen::INPUT_BACKDROP_ALPHA`].
///
/// **It takes no GUI scale, deliberately.** `input_backdrop_rect` is handed GUI
/// dimensions and returns GUI pixels, which is already the unit
/// [`rewo_gpu::hud::HudFill`] is in — so there is nothing here to convert.
/// This function used to take `px` and multiply by it, which put the bar at
/// y 2034 on a 720-tall screen; not having the scale in scope is a stronger
/// guarantee than remembering not to use it.
pub(super) fn chat_input_backdrop(gui_w: i32, gui_h: i32) -> rewo_gpu::hud::HudFill {
    let (x, y, w, h) = rewo_world::chat_screen::input_backdrop_rect(gui_w, gui_h);
    rewo_gpu::hud::HudFill {
        x: x as f32,
        y: y as f32,
        w: w as f32,
        h: h as f32,
        alpha: rewo_world::chat_screen::INPUT_BACKDROP_ALPHA,
        // `getBackgroundColor` builds it with `colorFromFloat(_, 0, 0, 0)` —
        // black, like the rows'.
        rgb: [0.0; 3],
    }
}

/// The chat box's text, from `ChatComponent.extractRenderState`.
///
/// ```java
/// float scale = (float)this.getScale();
/// int chatBottom = Mth.floor((screenHeight - 40) / scale);
/// pose.scale(scale, scale); pose.translate(4.0F, 0.0F);
/// int entryBottom = chatBottom - lineIndex * entryHeight;
/// int textTop     = entryBottom - entryBottomToMessageY;
/// ```
///
/// Lifted out of [`build_text`] rather than written inline because
/// `build_text` takes a `&PlaySession`, which owns a socket and cannot be
/// constructed in a test — M97's lesson, and the arithmetic here is exactly
/// what a witness needs to reach.
///
/// **Two things this deliberately does not draw**, named rather than quietly
/// missing:
///
/// * **The backdrop fills.** `graphics.fill(-4, entryTop, maxWidth + 4 + 4,
///   entryBottom, ARGB.black(alpha * backgroundOpacity))` needs a per-quad
///   alpha, and [`rewo_gpu::hud`]'s vertex is `vec2 pos + vec2 uv` with **no
///   colour channel** — the cooldown overlay gets its tint from a texel baked
///   into the atlas, which cannot carry a varying fade. Adding one is a
///   vertex-format change: stride 16 → 32, both `hud` shaders, and the
///   `v.len() * 16` hardcode sitting beside `VERTEX_STRIDE`, which is the
///   shape M21 found in the entity pass and which produced a silently
///   truncated upload there.
/// * **The scrollbar**, which needs the same plus a second colour, and which
///   nothing can reach anyway until there is a chat screen to scroll from.
///
/// The *text* half is complete, and the fade works today only because
/// `OwnedTextLine::alpha` already exists — its doc comment says "chat fades
/// old lines", written before there was a chat that faded.
pub(super) fn chat_lines(
    chat: &rewo_world::chat::ChatComponent,
    gui_tick: i32,
    px: f32,
    screen_h: f32,
    opts: &rewo_world::chat::ChatOptions,
    advance: Option<&[u8; 256]>,
    focused: bool,
) -> (Vec<rewo_gpu::world::OwnedTextLine>, Vec<(String, usize)>) {
    use rewo_gpu::world::OwnedTextLine;
    // `getScale()` is a second multiplier on top of the GUI scale, so a chat
    // pixel is `px * scale` screen pixels and every offset below is in chat
    // pixels.
    let chat_px = px * opts.scale as f32;
    // M128 — ONE geometry, shared with `rewo_world::chat::clickable_style_at`.
    // Vanilla's lookup is not a second derivation of the layout: it calls the
    // same private `extractRenderState` the draw does, and the four times a
    // per-call-site copy has drifted in this repo are M89, M94/M95, M106b and
    // M112.
    let geom = rewo_world::chat::ChatBoxGeometry::new(screen_h, chat_px, opts);
    let text_opacity = opts.text_opacity();
    // Focused chat is a taller box with no fade. Supplied by the caller
    // (M110) rather than hardcoded: it is a fact about whether a `ChatScreen`
    // is open, which is exactly what `ChatComponent.isChatFocused` asks.
    let mut out: Vec<OwnedTextLine> = Vec::new();
    // Per row: its characters, and how many text lines it emitted — the
    // second is what lets a witness ask about ONE row rather than about the
    // whole box, which is a different and much weaker claim.
    let mut rows: Vec<(String, usize)> = Vec::new();
    for line in chat.visible_lines(gui_tick, focused, opts) {
        let y = geom.text_top(line.index) * chat_px;
        // `pose.translate(4.0F, 0.0F)` — `MESSAGE_INDENT`.
        let mut pen = rewo_world::chat::MESSAGE_INDENT as f32 * chat_px;
        let mut row = String::new();
        let row_start = out.len();
        for span in line.text {
            let w = advance
                .map(|a| rewo_gpu::text::width_styled(&span.text, a, span.bold))
                .unwrap_or(0);
            if !span.text.is_empty() {
                out.push(OwnedTextLine {
                    x: pen,
                    y,
                    px: chat_px,
                    // The span's own colour, in the LINEAR space the pass
                    // writes into an sRGB attachment. M117's coloured command
                    // runs are the precedent; the death screen and the XP
                    // level still hand over `/255` bytes and are a hair bright
                    // (named in the plan, not fixed here).
                    color_linear: srgb_bytes_to_linear_f(span.color),
                    // `alpha * textOpacity`, where `textOpacity` is
                    // `chatOpacity * 0.9 + 0.1` and so never reaches 0.
                    alpha: line.alpha * text_opacity,
                    shadow: true,
                    style: rewo_gpu::text::TextStyle {
                        bold: span.bold,
                        italic: span.italic,
                        underlined: span.underlined,
                        strikethrough: span.strikethrough,
                        obfuscated: span.obfuscated,
                    },
                    text: span.text.clone(),
                });
            }
            row.push_str(&span.text);
            pen += w as f32 * chat_px;
        }
        rows.push((row, out.len() - row_start));
    }
    (out, rows)
}

// ── The scoreboard sidebar (M132) ─────────────────────────────────────────

/// Resolve the sidebar for this frame, or `None` when there is nothing to
/// show.
///
/// `Hud.extractRenderState` reaches `extractScoreboardSidebar` only inside
/// `if (!this.isHidden)`, so F1 suppresses the whole panel — the same gate
/// M70 already reads for floating labels.
pub(crate) fn resolve_sidebar(
    scoreboard: &rewo_net::scoreboard::Scoreboard,
    local_name: &str,
    hud_hidden: bool,
    lang: &rewo_data::lang::Language,
    advance: Option<[u8; 256]>,
) -> Option<rewo_net::sidebar::Sidebar> {
    if hud_hidden {
        return None;
    }
    let width_of = move |t: &str, style: rewo_world::chat_style::ChatStyle| match &advance {
        Some(a) => rewo_gpu::text::width_styled(t, a, style.bold),
        None => 0,
    };
    rewo_net::sidebar::resolve(
        scoreboard,
        local_name,
        &rewo_net::sidebar::SidebarInput {
            width_of: &width_of,
            lang: Some(lang),
        },
    )
}

/// The sidebar's two background bands, in **GUI pixels**.
///
/// Two fills and no more: `displayScoreboardSidebar` has exactly one for the
/// header and one for the body, and the body's is a single rect covering every
/// row rather than one per row (which is what `PlayerTabOverlay` does, one
/// class over, and is the plausible symmetry to reach for).
///
/// **No `px` multiply, and that is deliberate.** [`rewo_gpu::hud::HudFill`]'s
/// own doc comment says GUI pixels and `HudPass::draw`'s `tinted_quad` scales
/// every rect by the GUI scale itself — measured, not assumed: a fill at
/// `(100, 100, 20, 20)` lands at screen `(200, 200)-(240, 240)` at scale 2
/// (`sidebarshot`'s development probe). Every OTHER producer in this file
/// multiplies by `px` first and is therefore scaled twice; see the note on
/// [`hud_fills`]. Do not "fix" this one into agreement with them.
pub(crate) fn sidebar_fills(
    layout: &rewo_net::sidebar::SidebarLayout,
) -> Vec<rewo_gpu::hud::HudFill> {
    use rewo_net::sidebar::{BODY_BACKGROUND, HEADER_BACKGROUND};
    let fill = |r: rewo_net::sidebar::Rect, argb: u32| rewo_gpu::hud::HudFill {
        x: r.x as f32,
        y: r.y as f32,
        w: r.w as f32,
        h: r.h as f32,
        alpha: ((argb >> 24) & 0xFF) as f32 / 255.0,
        rgb: srgb_bytes_to_linear(argb & 0x00FF_FFFF),
    };
    vec![
        fill(layout.header_background, HEADER_BACKGROUND),
        fill(layout.body_background, BODY_BACKGROUND),
    ]
}

/// The sidebar's text — the title, then each row's name and score.
///
/// Every one of the three `graphics.text` calls passes an explicit `false` for
/// `dropShadow`, which is why these do too. The five-argument overload one
/// class over defaults it to `true`, so the tab list's rows *do* drop a shadow
/// from the same method — the asymmetry is deliberate in vanilla and not a
/// transcription slip here.
pub(crate) fn sidebar_text(
    sidebar: &rewo_net::sidebar::Sidebar,
    layout: &rewo_net::sidebar::SidebarLayout,
    px: f32,
    advance: Option<[u8; 256]>,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_gpu::world::OwnedTextLine;
    let mut out: Vec<OwnedTextLine> = Vec::new();
    let mut push_line = |line: &rewo_world::chat_style::ChatLine, x: i32, y: i32| {
        let mut pen = x as f32 * px;
        for span in line {
            let w = advance
                .as_ref()
                .map(|a| rewo_gpu::text::width_styled(&span.text, a, span.bold))
                .unwrap_or(0);
            if !span.text.is_empty() {
                out.push(OwnedTextLine {
                    x: pen,
                    y: y as f32 * px,
                    px,
                    color_linear: srgb_bytes_to_linear_f(span.color),
                    alpha: 1.0,
                    shadow: rewo_net::sidebar::DROP_SHADOW,
                    style: rewo_gpu::text::TextStyle {
                        bold: span.bold,
                        italic: span.italic,
                        underlined: span.underlined,
                        strikethrough: span.strikethrough,
                        obfuscated: span.obfuscated,
                    },
                    text: span.text.clone(),
                });
            }
            pen += w as f32 * px;
        }
    };
    push_line(&sidebar.title, layout.title.0, layout.title.1);
    for (row, entry) in layout.rows.iter().zip(&sidebar.entries) {
        push_line(&entry.name, row.name.0, row.name.1);
        if let Some((sx, sy)) = row.score {
            push_line(&entry.score, sx, sy);
        }
    }
    out
}

// ── The tab list (M151) ───────────────────────────────────────────────────

/// Resolve the tab list for this frame, or `None` when the key is up or F1 has
/// hidden the HUD.
///
/// The session half of M97's split: every lookup here is a `PlaySession`
/// method, and every *decision* is in [`crate::tab_list_view::resolve`], which
/// takes them as closures and is unit-tested. `PlaySession` owns a socket and
/// has no test module anywhere in the repo, so a rule left in this function
/// would be unreachable by every check but `--render-check`.
pub(crate) fn resolve_tab_list(
    session: &PlaySession,
    key_down: bool,
    hud_hidden: bool,
    screen_width: i32,
    lang: &rewo_data::lang::Language,
    advance: Option<[u8; 256]>,
) -> Option<crate::tab_list_view::TabListView> {
    if !crate::tab_list_view::visible(key_down, hud_hidden) {
        return None;
    }
    let listed = session.listed_players();
    let width_of = move |t: &str, style: rewo_world::chat_style::ChatStyle| match &advance {
        Some(a) => rewo_gpu::text::width_styled(t, a, style.bold),
        None => 0,
    };
    let name_of = |u: u128| session.world.entities.name_of(u).map(str::to_string);
    let ping_of = |u: u128| session.ping_ms(u);
    // `getGameMode()` defaults to `SURVIVAL` on a fresh `PlayerInfo`, so an
    // unsent mode is not a spectator — the map's `None` and vanilla's default
    // agree here, which is why this collapses to a bool rather than carrying
    // the `Option` through.
    let spectator_of = |u: u128| session.game_mode(u).is_some_and(|m| m.is_spectator());
    let order_of = |u: u128| session.tab_list_order(u).unwrap_or(0);
    let team_of = |u: u128| session.team_of(u).map(str::to_string);
    let display_name_of = |u: u128| session.tab_display_name(u).cloned();
    Some(crate::tab_list_view::resolve(
        &crate::tab_list_view::TabListLookups {
            listed: &listed,
            name_of: &name_of,
            ping_of: &ping_of,
            spectator_of: &spectator_of,
            order_of: &order_of,
            team_of: &team_of,
            display_name_of: &display_name_of,
            width_of: &width_of,
            lang: Some(lang),
            online_mode: session.online_mode,
            screen_width,
            header: session.tab_list_text.header.as_ref(),
            footer: session.tab_list_text.footer.as_ref(),
            scoreboard: Some(&session.scoreboard),
        },
    ))
}

/// A span's already-unpacked `[f32; 3]` (sRGB, `chat_style::rgb_f32`'s plain
/// `/255`) into linear.
///
/// Beside [`srgb_bytes_to_linear`] rather than folded into it because the
/// input is a triple that has already been divided, not a packed `u32` — and
/// both go through `rewo_gpu`'s one transfer function for M111's reason.
/// `BookAccess.fromItem` + the per-page resolve + wrap (M172), extracted from
/// `pump_book_screen` so the gate can drive the SAME code (the M97 rule —
/// logic in a place with no test seam is untestable, so move it).
///
/// The written component wins even with zero pages; the writable pages are
/// plain strings shown as literals; NEITHER present resolves to `None` and no
/// screen opens. The wrap is `StringSplitter.splitLines`
/// (`split_lines_wrapped`) at `TEXT_WIDTH` — NOT the chat's
/// `wrap_components`, which would prepend an indent space to every
/// continuation line vanilla's book pages do not have.
pub(crate) fn resolve_book_pages(
    text: &rewo_world::inventory::SlotText,
    advance: Option<&[u8; 256]>,
    lang: Option<&rewo_data::lang::Language>,
) -> Option<Vec<Vec<rewo_world::chat_style::ChatLine>>> {
    use rewo_world::book_view_screen::{PAGE_TEXT_COLOR, TEXT_WIDTH};
    use rewo_world::chat_style::{self, ChatStyle};
    let base = ChatStyle::plain(chat_style::rgb_f32(PAGE_TEXT_COLOR));
    let advance = advance.copied();
    let width_of = move |t: &str, st: ChatStyle| match &advance {
        Some(a) => rewo_gpu::text::width_styled(t, a, st.bold),
        None => 0,
    };
    let wrap = |spans: &rewo_world::chat_style::ChatLine| {
        rewo_world::string_splitter::split_lines_wrapped(spans, TEXT_WIDTH, &width_of)
            .into_iter()
            .map(|l| l.spans)
            .collect::<Vec<_>>()
    };
    if text.has_written_book {
        Some(
            text.book_pages
                .iter()
                .map(|tag| wrap(&chat_style::parse_component(tag, base.clone(), lang)))
                .collect(),
        )
    } else if text.has_writable_book {
        Some(
            text.writable_pages
                .iter()
                .map(|page| wrap(&vec![base.clone().span(page.clone())]))
                .collect(),
        )
    } else {
        None
    }
}

/// The ROOT options page's rows (M173): links to the sub-pages Rewo has, in
/// vanilla `OptionsScreen.init`'s order — `SOUNDS` (`:71`), `VIDEO` (`:72`),
/// then `ACCESSIBILITY` (`:87`), two per row as vanilla's `createRowHelper(2)`
/// packs them. The links Rewo does not have (Skin Customization, Controls,
/// Language, Chat, Resource Packs, Telemetry, Credits) are absent rather than
/// stubbed.
pub(crate) fn root_rows(lang: &rewo_data::lang::Language) -> Vec<rewo_world::options_screen::OptionRow> {
    use rewo_world::options_screen::{OptionRow, RowItem};
    vec![
        OptionRow::small(
            RowItem::Button(lang.get_or_default("options.sounds", "Music & Sounds...").to_string()),
            Some(RowItem::Button(
                lang.get_or_default("options.video", "Video Settings...").to_string(),
            )),
        ),
        OptionRow::small(
            RowItem::Button(
                lang.get_or_default("options.accessibility", "Accessibility Settings...").to_string(),
            ),
            None,
        ),
    ]
}

/// The SOUND page's rows (M173), in vanilla `SoundOptionsScreen.addOptions`
/// order: `addBig(MASTER)`, five `addSmall` pairs of the ten non-master
/// sources in `SoundSource.values()` order, then the music-frequency cycle
/// button alone in the left column. The rows vanilla has that Rewo does not
/// model — the sound DEVICE, Closed Captions, Directional Audio, the music
/// toast — are absent rather than stubbed.
pub(crate) fn sound_rows(
    options: &rewo_net::options::Options,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_world::options_screen::OptionRow> {
    use rewo_net::sounds::SoundSource;
    use rewo_world::options_screen::{percent_label, OptionRow, RowItem};
    let off = lang.get_or_default("options.off", "OFF");
    let item = |source: SoundSource| {
        let caption_key = format!("soundCategory.{}", source.name());
        let caption = lang.get_or_default(&caption_key, source.name());
        let value = options.sound_volume(source);
        RowItem::Slider {
            label: percent_label(caption, value, off),
            value,
        }
    };
    let mut rows = vec![OptionRow::big(item(SoundSource::Master))];
    let rest = &SoundSource::ALL[1..];
    for pair in rest.chunks(2) {
        rows.push(OptionRow::small(item(pair[0]), pair.get(1).map(|s| item(*s))));
    }
    let caption = lang.get_or_default("options.music_frequency", "Music Frequency");
    let value_key = format!(
        "options.music_frequency.{}",
        rewo_net::options::frequency_name(options.music_frequency).to_lowercase()
    );
    let value =
        lang.get_or_default(&value_key, rewo_net::options::frequency_name(options.music_frequency));
    rows.push(OptionRow::small(
        RowItem::Button(rewo_world::options_screen::cycle_label(caption, value)),
        None,
    ));
    rows
}

/// The VIDEO page's rows (M173): the one option Rewo models there —
/// `options.ao`'s cycle button, the fifth of vanilla's `qualityOptions`
/// (`VideoSettingsScreen.java:51`) and the whole of Rewo's page. Everything
/// else on the three sections is absent rather than stubbed (see
/// [`rewo_world::options_screen::OptionsPage::Video`]).
pub(crate) fn video_rows(
    options: &rewo_net::options::Options,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_world::options_screen::OptionRow> {
    use rewo_world::options_screen::{bool_label, cycle_label, OptionRow, RowItem};
    let caption = lang.get_or_default("options.ao", "Smooth Lighting");
    vec![OptionRow::small(
        RowItem::Button(cycle_label(caption, bool_label(options.smooth_lighting))),
        None,
    )]
}

/// The ACCESSIBILITY page's rows (M173): the one option Rewo models there.
pub(crate) fn accessibility_rows(
    options: &rewo_net::options::Options,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_world::options_screen::OptionRow> {
    use rewo_world::options_screen::{bool_label, cycle_label, OptionRow, RowItem};
    let caption = lang.get_or_default("options.hideLightningFlashes", "Hide Lightning Flashes");
    vec![OptionRow::small(
        RowItem::Button(cycle_label(caption, bool_label(options.hide_lightning_flash))),
        None,
    )]
}

/// The slider sprite lowering (M173) — `screen_chrome` lowers only buttons,
/// and a missing branch is an invisible widget with no error.
///
/// Track: nine-slice border 1, never highlighted — vanilla's highlighted
/// track shows only when focused-but-NOT-engaged, a state reachable only
/// through arrow-key list navigation this framework does not have (tab-focus
/// auto-engages, `AbstractSliderButton.setFocused`). Handle: at
/// `x + (int)(value * (width - 8))`, 8 wide, nine-slice `{2, 2, 2, 3}`
/// (ASYMMETRIC — it can never be a single border number), highlighted when
/// hovered or dragging (`canChangeValue`).
pub(crate) fn slider_sprites(
    screen: &rewo_world::screen::Screen,
    mouse: Option<(f64, f64)>,
    dragging: Option<rewo_world::screen::WidgetId>,
) -> Vec<rewo_gpu::screen::SpriteDraw> {
    use rewo_gpu::screen::{Fill, Sheet, SpriteDraw};
    use rewo_world::screen::{slider_handle_x, WidgetKind};
    let mut out = Vec::new();
    for w in screen.widgets.iter().filter(|w| w.visible) {
        let WidgetKind::Slider { value } = w.kind else {
            continue;
        };
        out.push(SpriteDraw {
            x: w.x,
            y: w.y,
            width: w.width,
            height: w.height,
            sheet: Sheet::SliderSheet(0),
            fill: Fill::NineSlice([1, 1, 1, 1]),
            color: [1.0; 4],
        });
        let engaged = dragging == Some(w.id);
        let highlighted = w.active && (w.is_hovered(mouse) || engaged);
        out.push(SpriteDraw {
            x: slider_handle_x(w.x, w.width, value),
            y: w.y,
            width: 8,
            height: w.height,
            sheet: Sheet::SliderSheet(if highlighted { 3 } else { 2 }),
            fill: Fill::NineSlice([2, 2, 2, 3]),
            color: [1.0; 4],
        });
    }
    out
}

/// M174 — the sign editor's app-side state. The model lives in
/// `rewo_world::sign_edit_screen`; this carries what the model cannot know:
/// where the sign is, which face, its dye + glowing (captured at open —
/// `setMessage` preserves both), which wood sheet, and the blink epoch
/// (vanilla stamps `cursorBlinkStartTime = Util.getMillis()` in `init()`).
pub(crate) struct SignEditView {
    pub state: rewo_world::sign_edit_screen::SignEditState,
    pub pos: (i32, i32, i32),
    pub is_front: bool,
    /// The face's dye text colour, `0xRRGGBB`.
    pub dye: u32,
    pub glowing: bool,
    pub wood: u8,
    pub opened: std::time::Instant,
}

/// The sign editor's board blit (M174): the standing board is the whole
/// 24x26 sheet; the WALL board samples only the top 12 rows (the plaque —
/// the other 14 are the post) at the same origin; the hanging board is its
/// whole 16x16. All through [`rewo_world::sign_edit_screen::board_sprite`]'s
/// nearest-integer rounding of the fractional pose rect (a stated ≤0.6-px
/// deviation — vanilla rasterises the exact rect).
pub(crate) fn sign_board_sprite(
    kind: rewo_world::sign_edit_screen::SignKind,
    wood: u8,
    gui_w: i32,
) -> rewo_gpu::screen::SpriteDraw {
    use rewo_gpu::screen::{Fill, Sheet, SpriteDraw};
    use rewo_world::sign_edit_screen::SignKind;
    let (x, y, w, h) = rewo_world::sign_edit_screen::board_sprite(kind, gui_w);
    let (sheet, fill) = match kind {
        SignKind::Standing => (Sheet::SignBoard(wood), Fill::Stretch),
        SignKind::Wall => (Sheet::SignBoard(wood), Fill::SubRect(0, 0, 24, 12)),
        SignKind::Hanging => (Sheet::HangingSignBoard(wood), Fill::Stretch),
    };
    SpriteDraw { x, y, width: w, height: h, sheet, fill, color: [1.0; 4] }
}

/// The editor's line colour (M174):
/// `hasGlowingText ? dye : getDarkColor(text)` — and since the ternary only
/// reaches `getDarkColor` when glowing is FALSE, its `BLACK && glowing →
/// 0xF0EBCC` special is DEAD in this screen (it is live in the world
/// renderer, where the dark colour is the glowing text's outline). So:
/// glowing → the dye at full strength; else the dye scaled 0.4, truncating.
pub(crate) fn sign_edit_line_color(dye: u32, glowing: bool) -> u32 {
    if glowing {
        dye
    } else {
        rewo_data::sign_text::scale_rgb(dye, 0.4)
    }
}

/// The sign editor's text + caret + selection for one frame (M174), lowered
/// to physical-px text lines and GUI-px fills. The selection is drawn UNDER
/// the text (vanilla inverts the glyphs through a GUI_INVERT pipeline this
/// pass does not have — a stated approximation: a blue box with the line's
/// own dark text instead of inverted white).
pub(crate) fn sign_edit_draws(
    view: &SignEditView,
    gui_w: i32,
    advance: &[u8; 256],
    px: f32,
) -> (Vec<rewo_gpu::world::OwnedTextLine>, Vec<rewo_gpu::screen::SpriteDraw>) {
    use rewo_gpu::screen::{Fill, Sheet, SpriteDraw};
    use rewo_world::sign_edit_screen as se;
    let width_fn = |t: &str| rewo_gpu::text::width(t, advance);
    let kind = view.state.kind;
    let ts = kind.text_scale();
    let color = sign_edit_line_color(view.dye, view.glowing);
    let color_f = srgb_bytes_to_linear(color);
    let mut text = Vec::new();
    let mut fills = Vec::new();
    // The selection first — under the text.
    if let Some((x, y, w, h)) = se::selection_rect(&view.state, gui_w, &width_fn) {
        fills.push(SpriteDraw {
            x: x.round() as i32,
            y: y.round() as i32,
            width: (w.round() as i32).max(1),
            height: h.round() as i32,
            sheet: Sheet::White,
            fill: Fill::Stretch,
            color: se::SELECTION_BLUE,
        });
    }
    // The four lines, each centred independently, `shadow: false`.
    for (i, line) in view.state.lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let (x, y) = se::line_origin(kind, gui_w, i, width_fn(line));
        text.push(rewo_gpu::world::OwnedTextLine {
            x: x * px,
            y: y * px,
            px: px * ts,
            color_linear: color_f,
            alpha: 1.0,
            shadow: false,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: line.clone(),
        });
    }
    // The caret, blink-gated on the wall clock since open.
    if se::cursor_visible(view.opened.elapsed().as_millis() as u64) {
        match se::caret_draw(&view.state, gui_w, &width_fn) {
            se::CaretDraw::Underscore { x, y } => text.push(rewo_gpu::world::OwnedTextLine {
                x: x * px,
                y: y * px,
                px: px * ts,
                color_linear: color_f,
                alpha: 1.0,
                shadow: false,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: "_".to_string(),
            }),
            se::CaretDraw::Bar { x, y, w, h } => fills.push(SpriteDraw {
                x: x.round() as i32,
                y: y.round() as i32,
                width: (w.round() as i32).max(1),
                height: h.round() as i32,
                sheet: Sheet::White,
                fill: Fill::Stretch,
                // `ARGB.opaque(color)` — the bar is the line colour, opaque.
                color: [color_f[0], color_f[1], color_f[2], 1.0],
            }),
        }
    }
    (text, fills)
}

/// sprite (M172). The `PageArrow` index order is the bake's declared one:
/// 0 forward, 1 forward_highlighted, 2 backward, 3 backward_highlighted.
pub(crate) fn book_sprite(d: rewo_world::book_view_screen::BookDraw) -> rewo_gpu::screen::SpriteDraw {
    use rewo_gpu::screen::{Fill, Sheet, SpriteDraw};
    use rewo_world::book_view_screen::BookDraw;
    match d {
        BookDraw::Background { x, y } => SpriteDraw {
            x,
            y,
            width: rewo_world::book_view_screen::IMAGE_W,
            height: rewo_world::book_view_screen::IMAGE_H,
            sheet: Sheet::BookBackground,
            // The bake cropped `book.png` to exactly the 192x192 the blit
            // samples, so whole-sheet Stretch IS the 1:1 blit.
            fill: Fill::Stretch,
            color: [1.0; 4],
        },
        BookDraw::Arrow {
            forward,
            highlighted,
            x,
            y,
        } => SpriteDraw {
            x,
            y,
            width: rewo_world::book_view_screen::PAGE_BUTTON_W,
            height: rewo_world::book_view_screen::PAGE_BUTTON_H,
            sheet: Sheet::PageArrow(match (forward, highlighted) {
                (true, false) => 0,
                (true, true) => 1,
                (false, false) => 2,
                (false, true) => 3,
            }),
            fill: Fill::Stretch,
            color: [1.0; 4],
        },
    }
}

/// The book's text for one frame (M172): the current page's styled lines,
/// left-aligned from `(left + 36, top + 30)` at the 9 px pitch, and the
/// right-aligned page indicator whose RIGHT edge sits at `left + 148` —
/// `TextAlignment.RIGHT` is `anchor - width`, so anchoring the left edge
/// there would push it into the margin. Everything draws `shadow: false`:
/// `PAGE_TEXT_STYLE` is `withoutShadow()`, and the black must NOT be painted
/// over the spans — a page span with its own colour keeps it
/// (`mergeStyles` = component-present wins), which the resolve already did.
pub(crate) fn book_text_lines(
    book: &rewo_world::book_view_screen::BookViewScreen,
    gui_w: i32,
    advance: &[u8; 256],
    px: f32,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_world::book_view_screen as bv;
    let mut out = Vec::new();
    // The spans come from THE layout walk — the same one the click hit-test
    // reads — so a drawn span and its clickable rect cannot disagree (M89's
    // rule; M180's hit test was bolted onto this exact loop's replacement).
    let measure = |s: &rewo_world::chat_style::ChatSpan| {
        rewo_gpu::text::width_styled(&s.text, advance, s.bold)
    };
    for ls in bv::layout_spans(book, gui_w, &measure) {
        out.push(rewo_gpu::world::OwnedTextLine {
            x: ls.x as f32 * px,
            y: ls.y as f32 * px,
            px,
            color_linear: srgb_bytes_to_linear_f(ls.span.color),
            alpha: 1.0,
            shadow: false,
            style: text_style_of(ls.span),
            text: ls.span.text.clone(),
        });
    }
    // `book.pageIndicator` is `Page %1$s of %2$s` — POSITIONAL specifiers,
    // so this goes through the real `decomposeTemplate` (M125) rather than a
    // sequential `%s` substitution. The first cut used the latter and
    // bookshot's p8 caught it: both indicators rendered the raw pattern,
    // identically, and the diff witness measured zero.
    let (cur, total) = book.indicator();
    let template = lang.get_or_default("book.pageIndicator", "Page %s of %s");
    let args = [cur.to_string(), total.to_string()];
    let msg = match rewo_data::lang::decompose_template(template, args.len()) {
        Some(parts) => parts
            .into_iter()
            .map(|p| match p {
                rewo_data::lang::Part::Literal(t) => t.to_string(),
                rewo_data::lang::Part::Arg(i) => args.get(i).cloned().unwrap_or_default(),
            })
            .collect::<String>(),
        // An undecomposable pattern renders literally — vanilla's
        // `insertNumber` fallback shape.
        None => template.to_string(),
    };
    let (ax, ay) = bv::BookViewScreen::indicator_right(gui_w);
    let w = rewo_gpu::text::width_styled(&msg, advance, false);
    out.push(rewo_gpu::world::OwnedTextLine {
        x: (ax - w) as f32 * px,
        y: ay as f32 * px,
        px,
        color_linear: [0.0, 0.0, 0.0],
        alpha: 1.0,
        shadow: false,
        style: rewo_gpu::text::TextStyle::PLAIN,
        text: msg,
    });
    out
}

pub(crate) fn srgb_bytes_to_linear_f(rgb: [f32; 3]) -> [f32; 3] {
    [
        srgb_to_linear(rgb[0]),
        srgb_to_linear(rgb[1]),
        srgb_to_linear(rgb[2]),
    ]
}

/// An 0xRRGGBB byte triple into the LINEAR space the HUD's vertex tint
/// multiplies in.
///
/// The attachment is SRGB and the atlas is an SRGB image, so `texture()` has
/// already decoded by the time the tint applies — a caller handing over the
/// stored byte would be a third of a stop too bright. Black hid this for two
/// milestones because it is 0 in both spaces; the scrollbar's `0x3333AA` does
/// not. Same finding as M50's glint, one pass over.
/// **Built on `rewo_gpu`'s per-channel `srgb_to_linear`** rather than a second
/// transfer function: two copies of a curve are two chances to differ by a
/// hundredth, and this one is already the renderer's.
pub(crate) fn srgb_bytes_to_linear(rgb: u32) -> [f32; 3] {
    let ch = |shift: u32| srgb_to_linear(((rgb >> shift) & 0xFF) as f32 / 255.0);
    [ch(16), ch(8), ch(0)]
}

/// The chat box's backdrop fills (M109), in GUI pixels.
///
/// ```java
/// int count = this.forEachLine(alphaCalculator, (line, lineIndex, alpha) -> {
///    int entryBottom = chatBottom - lineIndex * entryHeight;
///    int entryTop = entryBottom - entryHeight;
///    graphics.fill(-4, entryTop, maxWidth + 4 + 4, entryBottom, ARGB.black(alpha * backgroundOpacity));
/// });
/// ```
///
/// Beside [`chat_lines`] and reading the same `visible_lines`, so a row's fill
/// and its text cannot disagree about which rows exist or where they are.
///
/// Three things that are not the obvious reading:
///
/// * **The rect is asymmetric about the text.** The pose is translated by
///   `MESSAGE_INDENT` (4) before the fill, so `-4` lands at absolute 0 — four
///   pixels of padding left of the text — while `maxWidth + 4 + 4` lands at
///   `maxWidth + 12`, eight past where a full-width line can reach. Centring it
///   is the tidy reading and is not vanilla's.
/// * **`maxWidth` is `Mth.ceil(getWidth() / scale)`, a CEIL**, where
///   `addMessageToDisplayQueue`'s wrap budget is `Mth.floor` of the same
///   expression. They differ by one whenever the division is not exact, and the
///   difference is deliberate in the sense that vanilla wrote both: the box is
///   never narrower than the text it was wrapped to.
/// * **The alpha is `alpha * backgroundOpacity`, NOT `alpha * textOpacity`.**
///   The text's multiplier has a 0.1 floor (`chatOpacity * 0.9 + 0.1`) and the
///   background's has none, so at "Text Background: 0" the fill vanishes
///   entirely while the text stays faintly visible.
/// **Known wrong, and out of M132's scope: these are screen pixels where
/// [`rewo_gpu::hud::HudFill`] is documented as GUI pixels.**
///
/// `HudPass::draw`'s `tinted_quad` multiplies every fill by the GUI scale, so
/// a rect already multiplied by `px` here is scaled twice — at scale 2 a chat
/// row's backdrop is drawn at twice its intended `y`, which is off the bottom
/// of the screen, so the backdrops are simply absent rather than misplaced.
/// The same applies to [`chat_input_backdrop`], [`chat_scrollbar`] and
/// [`suggestion_popup_fills`]. No gate can see it: `inventoryshot`'s
/// chat-backdrop witnesses render at 256x256, where the GUI scale is 1 and the
/// two readings coincide, and `--render-check`'s counters count the emitted
/// LIST rather than pixels. Measured during M132 with a unit probe — a fill at
/// `(100, 100, 20, 20)` lands at screen `(200, 200)-(240, 240)`.
pub(super) fn hud_fills(
    chat: &rewo_world::chat::ChatComponent,
    gui_tick: i32,
    px: f32,
    screen_h: f32,
    opts: &rewo_world::chat::ChatOptions,
    focused: bool,
) -> Vec<rewo_gpu::hud::HudFill> {
    let chat_px = px * opts.scale as f32;
    let chat_bottom = ((screen_h / chat_px) - rewo_world::chat::BOTTOM_MARGIN as f32).floor();
    let entry_height = opts.entry_height() as f32;
    // `Mth.ceil(this.getWidth() / scale)`.
    let max_width = (opts.width() as f32 / opts.scale as f32).ceil();
    let bg = opts.text_background_opacity as f32;
    // **A chat pixel is `opts.scale` GUI pixels, not `chat_px` of them.**
    //
    // `chat_px` is one chat pixel's size on the SCREEN, and it is the right
    // conversion for `chat_bottom` above, which turns a screen height into chat
    // space. It is the wrong one here: [`rewo_gpu::hud::HudFill`] is in GUI
    // pixels and `HudPass::draw` multiplies by the GUI scale itself, so
    // emitting `chat_px` applied that scale a second time and put every row's
    // backdrop `gui_scale` times too far down — off the bottom of the screen,
    // which is why they read as missing rather than as misplaced. The sibling
    // `chat_lines` DOES want `chat_px`, because the text pass takes screen
    // pixels; the two conventions sitting one function apart is how this
    // happened.
    let chat_gui = opts.scale as f32;
    chat.visible_lines(gui_tick, focused, opts)
        .into_iter()
        .map(|line| {
            let entry_bottom = chat_bottom - line.index as f32 * entry_height;
            let entry_top = entry_bottom - entry_height;
            rewo_gpu::hud::HudFill {
                // `fill(-4, …)` under a pose translated by +4.
                x: (rewo_world::chat::MESSAGE_INDENT as f32 - 4.0) * chat_gui,
                y: entry_top * chat_gui,
                // right - left = (maxWidth + 4 + 4) - (-4).
                w: (max_width + 12.0) * chat_gui,
                h: entry_height * chat_gui,
                alpha: line.alpha * bg,
                // `ARGB.black(a)` sets only the alpha byte.
                rgb: [0.0; 3],
            }
        })
        .collect()
}

/// `options.notificationDisplayTime` — the multiplier on the 40-tick timer.
///
/// Rewo has no options file, so this is vanilla's default (`1.0`, the middle
/// of an `IntRange(5, 100)` mapped through `v / 10.0`), which makes the label
/// hold for two seconds.
pub const NOTIFICATION_DISPLAY_TIME: f64 = 1.0;

/// `ItemStack.getHoverName()` for the selected hotbar stack, as the two fields
/// `Hud.tick`'s re-trigger compares (M66).
///
/// `None` is an empty hand, which zeroes the timer.
pub(super) fn selected_item_label(
    session: &PlaySession,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
) -> Option<(i32, String)> {
    let stack = session.inventory.held()?;
    let translated = items.name(stack.item_id).and_then(|n| names.get(n))?;
    // `getHoverName()`: `custom_name` then `item_name` then the item's own
    // translated name. Resolved here, against the table, rather than at the
    // wire — see `rewo_world::chat_style::flatten`.
    let name = session
        .inventory
        .text_of(stack)
        .and_then(|t| t.custom_name.as_ref().or(t.item_name.as_ref()))
        .map(|tag| rewo_world::chat_style::flatten(tag, Some(lang)))
        .unwrap_or_else(|| translated.clone());
    Some((stack.item_id, name))
}

/// `Hud.extractSelectedItemName` (M66) — the held-item label over the hotbar.
///
/// Vanilla's call site skips it entirely in spectator mode:
///
/// ```java
/// if (this.minecraft.gameMode.getPlayerMode() != GameType.SPECTATOR) {
///    this.extractSelectedItemName(graphics);
/// }
/// ```
///
/// **The ITALIC that `CUSTOM_NAME` adds is not rendered.** The HUD's text pass
/// carries one colour per line and has no italic face, so a renamed stack's
/// label shows its name and its rarity colour but stands upright. The colour
/// and the fade do carry, which is the visible bulk of it — and the same gap
/// M42 records for the bitmap tooltip fallback.
///
/// The backdrop (`textWithBackdrop`'s `fill`) is likewise absent, and there it
/// is not a divergence: `getBackgroundColor(0.0F)` is zero at vanilla's
/// defaults, so vanilla draws no fill either. See
/// [`rewo_gpu::hud::text_backdrop_rect`].
pub(super) fn selected_item_name_line(
    session: &PlaySession,
    items: &rewo_data::items::Items,
    highlight: &rewo_gpu::hud::ToolHighlight,
    advance: &[u8; 256],
    px: f32,
    (screen_w, screen_h): (f32, f32),
) -> Option<rewo_gpu::world::OwnedTextLine> {
    if session
        .own_game_mode()
        .is_some_and(rewo_net::play::GameMode::is_spectator)
    {
        return None;
    }
    let (item_id, name) = highlight.showing()?;
    let alpha = rewo_gpu::hud::tool_highlight_alpha(highlight.timer);
    // `if (alpha > 0)` — the draw's own guard, separate from the timer's.
    if alpha <= 0 {
        return None;
    }
    let width = rewo_gpu::text::width(name, advance);
    // `canHurtPlayer()` is `localPlayerMode.isSurvival()`, and **that is
    // SURVIVAL || ADVENTURE**. A server that never sent the local player an
    // `UPDATE_GAME_MODE` leaves it unknown, and survival is the assumption the
    // rest of Rewo's HUD already makes — it draws hearts unconditionally.
    let can_hurt = session
        .own_game_mode()
        .map(rewo_net::play::GameMode::is_survival)
        .unwrap_or(true);
    let (gw, gh) = ((screen_w / px) as i32, (screen_h / px) as i32);
    let (x, y) = rewo_gpu::hud::selected_item_name_pos(gw, gh, width, can_hurt);
    // The rarity colour is read off the stack the label names. A stack that
    // changed since the last tick is a different label anyway, so the mismatch
    // is not reachable in practice; white is the fallback rather than the
    // wrong rarity's colour.
    let color = session
        .inventory
        .held()
        .filter(|s| s.item_id == item_id)
        .map(|s| {
            let text = session.inventory.text_of(s);
            rarity_color(stack_rarity(
                items.name(s.item_id),
                text.and_then(|t| t.rarity),
                text.is_some_and(|t| t.is_enchanted),
            ))
        })
        .unwrap_or([1.0, 1.0, 1.0]);
    Some(rewo_gpu::world::OwnedTextLine {
        x: x as f32 * px,
        y: y as f32 * px,
        px,
        // `rarity_color` yields vanilla's byte `/255`, and three of its five
        // callers feed the tooltip and Velvet passes rather than this one — so
        // the conversion belongs here, at the line, not inside the helper.
        color_linear: srgb_bytes_to_linear_f(color),
        alpha: alpha as f32 / 255.0,
        shadow: true,
        style: rewo_gpu::text::TextStyle::PLAIN,
        text: name.to_string(),
    })
}

/// M168 — the session-free inputs of [`survival_inputs_from`], so a gate can
/// build them by hand and grade the derivation. M97's pattern: logic that
/// lives only where a `PlaySession` is in scope has no test module, and a
/// mutation there survives every battery. What the assembler
/// ([`resolve_survival_inputs`]) adds is WHERE each field comes from; what
/// this carries is the values.
pub(crate) struct SurvivalSources<'a> {
    /// `gameMode.getPlayerMode()`; `None` when the server never said.
    pub game_mode: Option<rewo_net::play::GameMode>,
    /// `PlaySession::local_attributes` — the local player's synced snapshots.
    pub local_attributes: &'a rewo_world::attributes::EntityAttributes,
    pub attribute_registry: Option<&'a rewo_data::attributes::AttributeRegistry>,
    /// The local player's `activeEffects`.
    pub effects: &'a rewo_net::effects::VisualEffects,
    /// Air, frozen ticks, absorption.
    pub local: rewo_net::local_player_data::LocalPlayerData,
    /// `isEyeInFluid(WATER)`.
    pub underwater: bool,
    /// The DIRECT vehicle, already known to be a `LivingEntity` — the
    /// assembler applies the `instanceof` with the class table.
    pub vehicle: Option<VehicleSource<'a>>,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub hardcore: bool,
    /// `Util.getMillis()`.
    pub millis: u64,
    /// The jump bar's inputs when it is the contextual bar this frame
    /// (M169): `(getJumpRidingScale(), vehicle.getJumpCooldown())`.
    pub jump: Option<rewo_gpu::survival_hud::JumpInput>,
}

/// The living vehicle under the player, as the table knows it.
pub(crate) struct VehicleSource<'a> {
    /// `etypes.name(type_id)` — what the attribute registry's suppliers are
    /// keyed by.
    pub type_name: Option<&'a str>,
    /// Its synced attributes, if any `update_attributes` has arrived.
    pub attributes: Option<&'a rewo_world::attributes::EntityAttributes>,
    /// Metadata index 9.
    pub health: f32,
}

/// `player.getActiveEffects()` as `extractEffects` reads each entry: the
/// wire's five fields plus the two the registry report does not carry, looked
/// up in the generated table by id (`isBeneficial()` and `getColor()`). An id
/// past the table — a server-added effect on a modded server — is neither
/// beneficial nor coloured, which puts it on the harmful row last.
pub(crate) fn effect_inputs(
    effects: &rewo_net::effects::VisualEffects,
) -> Vec<rewo_gpu::survival_hud::EffectInput> {
    use rewo_data::mob_effect_table as fx;
    effects
        .active()
        .iter()
        .map(|e| {
            let def = fx::def(e.effect_id);
            rewo_gpu::survival_hud::EffectInput {
                id: e.effect_id,
                duration: e.duration,
                ambient: e.ambient,
                show_icon: e.show_icon,
                beneficial: def.is_some_and(|d| d.category.is_beneficial()),
                color: def.map_or(0, |d| d.color),
            }
        })
        .collect()
}

/// M168 — everything `extractPlayerHealth`, `extractVehicleHealth` and
/// `extractEffects` read, plus the one piece of per-frame STATE the HUD owns
/// (`Hud.displayHealth` and its blink clock), which is why `hud` is `&mut`.
///
/// * The two player attributes go through `rewo_world::attributes::resolve`
///   with `"minecraft:player"`, falling back to the supplier's default
///   (20 / 0) — the local player never receives the initial pairing packet
///   (`ServerEntity.java:286-290` goes to tracking players only), so an
///   absent ARMOR is the registered 0 and vanilla draws nothing for it.
/// * `getArmorValue()` is `Mth.floor` of the attribute
///   (`LivingEntity.java:1879`).
/// * The heart type and the HUNGER / REGENERATION tests read the effects map
///   by id, and the ids are the generated table's positions, which a
///   `rewo-data` test pins against the registry report.
/// * The vehicle's MAX_HEALTH resolves like any tracked entity's, against its
///   own type's supplier; its health is metadata index 9.
pub(crate) fn survival_inputs_from(
    src: SurvivalSources<'_>,
    hud: &mut rewo_net::hud_state::HudState,
) -> rewo_gpu::survival_hud::SurvivalInputs {
    use rewo_data::mob_effect_table as fx;
    use rewo_gpu::survival_hud as sh;
    let can_hurt = src
        .game_mode
        .map(rewo_net::play::GameMode::is_survival)
        .unwrap_or(true);
    let player_attr = |name: &str, default: f32| -> f32 {
        src.attribute_registry
            .and_then(|r| {
                rewo_world::attributes::resolve(
                    Some(src.local_attributes),
                    Some("minecraft:player"),
                    name,
                    r,
                )
            })
            .map(|(v, _)| v as f32)
            .unwrap_or(default)
    };
    let max_health_attr = player_attr("max_health", 20.0);
    let armor = player_attr("armor", 0.0).floor() as i32;
    let has = |name: &str| fx::id_of(name).is_some_and(|id| src.effects.has(id));
    let heart_type = sh::HeartKind::for_player(
        has("poison"),
        has("wither"),
        src.local.is_fully_frozen(),
    );
    let vehicle = src.vehicle.as_ref().map(|v| {
        let max_health = src
            .attribute_registry
            .and_then(|r| {
                rewo_world::attributes::resolve(v.attributes, v.type_name, "max_health", r)
            })
            .map(|(x, _)| x as f32)
            .unwrap_or(20.0);
        sh::VehicleInput {
            max_health,
            health: v.health,
        }
    });
    let tick_count = hud.gui_tick;
    let invulnerable = hud.local_hurt.is_invulnerable();
    let frame = hud.health_display.update(
        src.health.ceil() as i32,
        invulnerable,
        tick_count,
        src.millis,
    );
    sh::SurvivalInputs {
        can_hurt,
        health: src.health,
        max_health_attr,
        absorption: src.local.absorption(),
        display_health: frame.display_health,
        blink: frame.blink,
        heart_type,
        hardcore: src.hardcore,
        regeneration: has("regeneration"),
        armor,
        food: src.food,
        saturation: src.saturation,
        hunger_effect: has("hunger"),
        air_supply: src.local.air_supply(),
        max_air: sh::MAX_AIR_SUPPLY,
        eye_in_water: src.underwater,
        vehicle,
        effects: effect_inputs(src.effects),
        tick_count,
        jump: src.jump,
    }
}

/// The session adapter for [`survival_inputs_from`]: where each source
/// lives. The vehicle is `getVehicle()` — the DIRECT vehicle, not the root —
/// and counts only if its type `instanceof LivingEntity`
/// (`getPlayerVehicleWithHealth`, `Hud.java:728-742`); `isEyeInFluid(WATER)`
/// is the same eye test the ambient handlers use (`local_player_view`).
///
/// The two clones are of a handful of entries each: the session's accessors
/// borrow all of it, and `hud` has to be borrowed mutably beside them.
pub(crate) fn resolve_survival_inputs(
    session: &mut PlaySession,
    etypes: &EntityTypes,
    millis: u64,
    contextual: rewo_gpu::locator_bar::ContextualInfo,
) -> rewo_gpu::survival_hud::SurvivalInputs {
    // M169 — the jump bar draws only when it owns the slot, and then its
    // inputs are the meter's scale and the vehicle's dash cooldown.
    let jump = (contextual == rewo_gpu::locator_bar::ContextualInfo::JumpableVehicle).then(|| {
        rewo_gpu::survival_hud::JumpInput {
            scale: session.jump_riding_scale(),
            cooldown: session.jumpable_vehicle().map_or(0, |v| v.cooldown),
        }
    });
    let game_mode = session.own_game_mode();
    let local_attributes = session.local_attributes().clone();
    let registry = session.attribute_registry.clone();
    let effects = session.visual_effects().clone();
    let local = *session.local_player_data();
    let underwater = session.local_player_view().is_some_and(|v| v.underwater);
    let vehicle_id = session.local_vehicle().filter(|&vid| {
        session.world.entities.get(vid).is_some_and(|e| {
            session
                .entity_classes
                .as_deref()
                .is_some_and(|c| c.is_living(e.type_id))
        })
    });
    let (health, food, saturation, hardcore) =
        (session.health, session.food, session.saturation, session.hardcore);
    let PlaySession { hud, world, .. } = session;
    let vehicle = vehicle_id.and_then(|vid| {
        let e = world.entities.get(vid)?;
        Some(VehicleSource {
            type_name: etypes.name(e.type_id),
            attributes: world.entities.attributes(vid),
            health: world.entities.death_state(vid).health,
        })
    });
    survival_inputs_from(
        SurvivalSources {
            game_mode,
            local_attributes: &local_attributes,
            attribute_registry: registry.as_deref(),
            effects: &effects,
            local,
            underwater,
            vehicle,
            health,
            food,
            saturation,
            hardcore,
            millis,
            jump,
        },
        hud,
    )
}

/// M79's two HUD gauges, resolved from the session once per frame.
///
/// **The XP half is gated on `gameMode.hasExperience()`**, which is
/// `localPlayerMode.isSurvival()` — i.e. SURVIVAL *or* ADVENTURE, the same
/// two-value predicate M66's held-item label uses. Vanilla applies it in two
/// places with different consequences: `nextContextualInfoState` picks
/// `ContextualInfo.EMPTY` over `EXPERIENCE`, which removes the *bar*, and
/// `extractCommonHud` guards the level *number* separately (and additionally
/// on `experienceLevel > 0`, so level 0 shows no number even in survival).
/// An unknown mode falls back to survival for the same reason the hearts do.
///
/// **The cooldown half needs a group per slot**, and the group is
/// `getCooldownGroup(stack)`: the stack's `use_cooldown` override when it sets
/// one, the item's registry name otherwise. Both halves of that are here
/// because neither `rewo_net::hud_state` (which never sees a stack) nor
/// `rewo_gpu::hud` (which never sees the item table) can do it alone.
pub(crate) fn resolve_hud_gauges(
    hud: &rewo_net::hud_state::HudState,
    inventory: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    has_experience: bool,
    partial: f32,
) -> rewo_gpu::hud::HudGauges {
    let xp = &hud.experience;
    let mut cooldowns = [0.0f32; 9];
    for (i, slot) in cooldowns.iter_mut().enumerate() {
        let Some(stack) = inventory.hotbar(i) else {
            continue;
        };
        let Some(name) = items.name(stack.item_id) else {
            // An item id the table cannot name has no default group, and
            // guessing one would sweep an unrelated slot. Vanilla cannot
            // reach this: `BuiltInRegistries.ITEM.getKey` always answers.
            continue;
        };
        let group = inventory
            .text_of(stack)
            .and_then(|t| t.cooldown_group.as_deref())
            .unwrap_or(name);
        // `getCooldownPercent(item, getGameTimeDeltaPartialTick(true))`: the
        // frame's fraction into the tick, so the sweep slides rather than
        // stepping at 20 Hz.
        *slot = hud.cooldowns.percent(group, partial);
    }
    rewo_gpu::hud::HudGauges {
        experience: has_experience.then_some(xp.progress),
        xp_needed: xp.xp_needed_for_next_level(),
        cooldowns,
    }
}

pub(crate) fn locator_sprites(
    baked: &assets::BakedAssets,
) -> Option<rewo_gpu::locator_bar::LocatorSpritesData<'_>> {
    let l = baked.locator.as_ref()?;
    Some(rewo_gpu::locator_bar::LocatorSpritesData {
        background: hud_sprite(&l.background),
        arrow_up: hud_sprite(&l.arrow_up),
        arrow_down: hud_sprite(&l.arrow_down),
        dots: l.dots.iter().map(hud_sprite).collect(),
        styles: l
            .styles
            .iter()
            .map(|s| rewo_gpu::locator_bar::WaypointStyle {
                key: s.key.clone(),
                near_distance: s.near_distance,
                far_distance: s.far_distance,
                sprites: s.sprites.clone(),
            })
            .collect(),
    })
}

/// The net → gpu bridge for M83's locator bar, and the three things neither
/// side can do alone.
///
/// * **The identifier.** `rewo_gpu::locator_bar` never sees one, so the
///   `icon.color`-absent fallback (`setBrightness(color(255, hash), 0.9)`) and
///   the camera-entity skip are resolved here, where the store's keys are.
/// * **The style key → index.** The wire carries an `Identifier`; the atlas
///   carries a slot. An unknown key resolves to *no* style, which the pass
///   draws as the synthesised `MissingTextureAtlasSprite` patch — the same
///   answer `WaypointStyleManager.get`'s `getOrDefault(id, MISSING)` gives.
/// * **The entity substitution.** `Vec3iWaypoint.position` prefers the tracked
///   entity's interpolated eye position, which is a `level.getEntity(uuid)`
///   lookup — and `EntityTable` is keyed by entity *id*, so this is the O(n)
///   scan the table's only UUID map (profile names) cannot serve.
///
/// Returns `None` when the locator bar is not the contextual bar this frame.
///
/// **The observer is never in `EntityTable`** (REWO_PLAN §0.0 gotcha 13). Both
/// the camera position and the entity position come from `session.player`; a
/// version of this that reached for `entities.get(session.player_id)` would
/// find nothing and emit an empty bar on every frame, and a gate that built
/// its own table would never see it.
pub(crate) struct LocatorInputs<'a> {
    pub waypoints: &'a rewo_net::waypoints::WaypointStore,
    /// The **camera entity's** UUID. `session.own_uuid`, never a lookup in
    /// `entities` — see the doc above.
    pub own_uuid: Option<u128>,
    pub entities: &'a rewo_world::entities::EntityTable,
    pub styles: &'a [rewo_gpu::locator_bar::WaypointStyle],
    /// `camera.position()`.
    pub eye: [f64; 3],
    /// `cameraEntity.position()` — the feet.
    pub feet: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    /// `Hud.nextContextualInfoState()` for this frame (M169) — resolved once
    /// by [`contextual_info`] and handed to every consumer of the slot, so
    /// the XP bar, the locator bar and the jump bar cannot disagree about
    /// who owns it.
    pub contextual: rewo_gpu::locator_bar::ContextualInfo,
    pub ticks: u64,
}

/// The session adapter. Split from [`locator_bar_state`] so the gate drives
/// the emitter the frame drives rather than a copy of it — M45's
/// `install_shapes` failure and M41's rotted `swingshot` fixture were both
/// gates that had reimplemented a slice of the app and stopped testing their
/// subject. Same split M59 made for `resolve_health_bar`.
pub(crate) fn resolve_locator_bar(
    session: &PlaySession,
    entities: &rewo_world::entities::EntityTable,
    styles: &[rewo_gpu::locator_bar::WaypointStyle],
    fov_deg: f32,
    gui_w: i32,
    gui_h: i32,
    alpha: f32,
) -> Option<rewo_gpu::locator_bar::LocatorBarState> {
    let eye = player_eye(session);
    locator_bar_state(
        LocatorInputs {
            waypoints: &session.waypoints,
            own_uuid: session.own_uuid,
            entities,
            styles,
            eye: [eye.x as f64, eye.y as f64, eye.z as f64],
            feet: [session.player.x, session.player.y, session.player.z],
            yaw: session.player.yaw,
            pitch: session.player.pitch,
            fov: fov_deg,
            contextual: contextual_info(session),
            ticks: session.ticks,
        },
        gui_w,
        gui_h,
        alpha,
    )
}

pub(crate) fn locator_bar_state(
    input: LocatorInputs<'_>,
    gui_w: i32,
    gui_h: i32,
    alpha: f32,
) -> Option<rewo_gpu::locator_bar::LocatorBarState> {
    use rewo_gpu::locator_bar as lb;
    use rewo_net::waypoints::{WaypointContents, WaypointId};

    let LocatorInputs {
        waypoints,
        own_uuid,
        entities,
        styles,
        eye,
        feet,
        yaw,
        pitch,
        fov,
        contextual,
        ticks,
    } = input;

    if contextual != lb::ContextualInfo::Locator {
        return None;
    }

    let cam = lb::LocatorCamera {
        yaw,
        pitch,
        fov,
        camera_pos: eye,
        // `cameraEntity.position()` — the **feet**, which is the wire position.
        entity_pos: feet,
        // Rewo's own projection is infinite-far reversed-Z, so there is no
        // `far` to read; vanilla's is finite. It only scales `z_ndc`, whose
        // sole consumer is a `> 1.0` test that reduces to "closer than the
        // near plane" for any `far >> near` — so a nominal value is exact to
        // the part in `near/far` that the test cannot resolve.
        near: 0.05,
        far: 1024.0,
    };

    let mut out = Vec::new();
    for w in waypoints.iter_sorted() {
        let subject = match w.contents {
            WaypointContents::Empty => lb::WaypointSubject::Empty,
            WaypointContents::Chunk { x, z } => lb::WaypointSubject::Chunk { x, z },
            WaypointContents::Azimuth { radians } => lb::WaypointSubject::Azimuth { radians },
            WaypointContents::Vec3i { x, y, z } => {
                let entity_eye = match w.id {
                    WaypointId::Uuid(uuid) => entities
                        .iter()
                        .find(|(_, e)| e.uuid == uuid)
                        .and_then(|(_, e)| {
                            let p = e.render_pos(alpha);
                            // `e.blockPosition().distManhattan(this.vector) > 3
                            //  ? null : e.getEyePosition(partialTick)` — a
                            // staleness guard, because the waypoint packet and
                            // the entity's own movement packets arrive
                            // independently.
                            let bx = p[0].floor() as i32;
                            let by = p[1].floor() as i32;
                            let bz = p[2].floor() as i32;
                            let manhattan =
                                (bx - x).abs() + (by - y).abs() + (bz - z).abs();
                            if manhattan > 3 {
                                return None;
                            }
                            // `EntityDimensions.scalable`'s default eye height
                            // is `height * 0.85`. A handful of types override
                            // it; the approximation is confined to the pitch
                            // arrow, because `yawAngleToCamera` reads only x
                            // and z — the bearing is a purely horizontal
                            // computation and the y component never enters it.
                            let h = entities
                                .attachments()
                                .and_then(|a| a.points(e.type_id))
                                .map(|p| p.height as f64)
                                .unwrap_or(1.8);
                            Some([p[0], p[1] + h * 0.85, p[2]])
                        }),
                    WaypointId::Name(_) => None,
                };
                lb::WaypointSubject::Vec3i {
                    x,
                    y,
                    z,
                    entity_eye,
                }
            }
        };
        // `icon.color.orElseGet(() -> id.map(uuid -> …, name -> …))` — the two
        // arms differ only in which `hashCode` they call, and both go through
        // `ARGB.color(255, hash)`, the **two-argument** overload that keeps the
        // hash's low 24 bits as RGB rather than treating it as three channels.
        let color = w.icon.color.unwrap_or_else(|| {
            let hash = match &w.id {
                WaypointId::Uuid(u) => lb::java_uuid_hash(*u),
                WaypointId::Name(n) => lb::java_string_hash(n),
            };
            lb::argb_set_brightness(0xFF00_0000 | (hash as u32 & 0x00FF_FFFF), 0.9)
        });
        out.push(lb::LocatorWaypoint {
            subject,
            color,
            // `usize::MAX` is "no style resolved", which the pass draws as the
            // missing patch.
            style: styles
                .iter()
                .position(|s| s.key == w.icon.style)
                .unwrap_or(usize::MAX),
            is_camera_entity: matches!(w.id, WaypointId::Uuid(u) if Some(u) == own_uuid),
        });
    }

    Some(lb::LocatorBarState {
        markers: lb::markers(&out, styles, &cam, gui_w, gui_h),
        tick: ticks as i64,
    })
}

/// `gameMode.hasExperience()` is `localPlayerMode.isSurvival()`, which is
/// **SURVIVAL or ADVENTURE**.
///
/// An unknown mode falls back to survival, the same assumption the hearts
/// already make (M66 records the reasoning at `selected_item_name_line`).
pub(crate) fn has_experience(session: &PlaySession) -> bool {
    session
        .own_game_mode()
        .map(rewo_net::play::GameMode::is_survival)
        .unwrap_or(true)
}

/// `Hud.nextContextualInfoState()` for this frame (M169). One call per
/// frame, threaded to the XP gauge, the locator bar and the survival
/// layout's jump input, so the three can never each decide differently.
pub(crate) fn contextual_info(session: &PlaySession) -> rewo_gpu::locator_bar::ContextualInfo {
    let jumpable = session
        .jumpable_vehicle()
        .map(|v| (session.jump_riding_scale(), v.cooldown));
    rewo_gpu::locator_bar::next_contextual_info(
        !session.waypoints.is_empty(),
        jumpable,
        has_experience(session),
        session.hud.experience.will_prioritize(),
    )
}

/// The XP level number — `ContextualBar.extractExperienceLevel` (M79).
///
/// Five draws: a black copy at each of ±1 on both axes, then the green one,
/// **all with `shadow = false`**. The outline is what makes the number legible
/// over the bar it straddles; a drop shadow on top of it would thicken the
/// glyphs instead of framing them.
///
/// The string is `Component.translatable("gui.experience.level", level)`,
/// which the vanilla language file renders as the bare number — so the
/// translated form is looked up and the number substituted, with the number
/// alone as the fallback.
pub(crate) fn experience_level_lines(
    xp: &rewo_net::hud_state::ExperienceState,
    has_experience: bool,
    lang: Option<&rewo_data::lang::Language>,
    advance: &[u8; 256],
    px: f32,
    (screen_w, screen_h): (f32, f32),
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    let level = xp.level;
    if !has_experience || level <= 0 {
        return Vec::new();
    }
    let number = level.to_string();
    let text = match lang.and_then(|l| l.get("gui.experience.level")) {
        // `%s` is the one substitution the vanilla key carries.
        Some(pattern) if pattern.contains("%s") => pattern.replacen("%s", &number, 1),
        _ => number,
    };
    let width = rewo_gpu::text::width(&text, advance);
    let (gw, gh) = ((screen_w / px) as i32, (screen_h / px) as i32);
    let (x, y) = rewo_gpu::hud::experience_level_pos(gw, gh, width);
    let mut out = Vec::with_capacity(5);
    let mut push = |dx: i32, dy: i32, color: u32| {
        out.push(rewo_gpu::world::OwnedTextLine {
            x: (x + dx) as f32 * px,
            y: (y + dy) as f32 * px,
            px,
            color_linear: srgb_bytes_to_linear(color & 0x00FF_FFFF),
            alpha: 1.0,
            shadow: false,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: text.clone(),
        });
    };
    // The four black copies first, in vanilla's own order, then the green.
    push(1, 0, rewo_gpu::hud::EXPERIENCE_LEVEL_OUTLINE);
    push(-1, 0, rewo_gpu::hud::EXPERIENCE_LEVEL_OUTLINE);
    push(0, 1, rewo_gpu::hud::EXPERIENCE_LEVEL_OUTLINE);
    push(0, -1, rewo_gpu::hud::EXPERIENCE_LEVEL_OUTLINE);
    push(0, 0, rewo_gpu::hud::EXPERIENCE_LEVEL_COLOR);
    out
}

/// A parsed span's five renderable `Style` flags, for the bitmap text pass.
///
/// `Font.PreparedTextBuilder.accept` reads all five off the `Style` the
/// `FormattedCharSequence` carries per character — there is no surface at
/// which vanilla honours a component's colour and drops its bold. Rewo had
/// exactly that asymmetry on the title and the death screen until M130,
/// because both were written before `TextPass` could draw a flag (M126c) and
/// neither was revisited when it could.
pub(crate) fn text_style_of(span: &rewo_world::chat_style::ChatSpan) -> rewo_gpu::text::TextStyle {
    rewo_gpu::text::TextStyle {
        bold: span.bold,
        italic: span.italic,
        underlined: span.underlined,
        strikethrough: span.strikethrough,
        obfuscated: span.obfuscated,
    }
}

/// `Font.width(FormattedText)` over a parsed line — the per-span widths summed
/// with each span's OWN bold flag.
///
/// **Not `width(plain_text(line))`.** `getBoldOffset()` is 1.0 charged per
/// character (M126b), so flattening a line to measure it undercounts a bold
/// run by its length — and every consumer here divides the result by two to
/// centre, so the error lands as a visible half-width offset rather than as a
/// pixel. Vanilla's `font.width(this.title)` is style-aware for the same
/// reason: its splitter's width provider takes the style.
pub(crate) fn styled_line_width(
    line: &rewo_world::chat_style::ChatLine,
    advance: &[u8; 256],
) -> i32 {
    line.iter()
        .map(|s| rewo_gpu::text::width_styled(&s.text, advance, s.bold))
        .sum()
}

/// The title, the subtitle and the action bar — `Hud.extractTitle` and
/// `Hud.extractOverlayMessage` (M79).
///
/// **One text line per styled span**, penned out with the font's advances:
/// vanilla's `graphics.text(font, Component, x, y, color, shadow)` passes the
/// faded colour as a *default* that a span's own `color` replaces, and
/// `Font.StringRenderOutput.getTextColor` keeps the **caller's alpha** when it
/// does:
///
/// ```java
/// if (textColor != null) {
///    int alpha = ARGB.alpha(this.color);
///    return ARGB.color(alpha, textColor.getValue());
/// }
/// ```
///
/// So `{"text":"GO","color":"red"}` is red *and* still fades. Taking the
/// span's colour whole — the natural reading of "the style wins" — would give
/// a title that snaps in and out at full opacity.
pub(crate) fn title_lines(
    t: &rewo_net::hud_state::TitleOverlay,
    advance: &[u8; 256],
    px: f32,
    (screen_w, screen_h): (f32, f32),
    partial: f32,
    lang: Option<&rewo_data::lang::Language>,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_net::chat_style::{self, ChatStyle};
    let (gw, gh) = ((screen_w / px) as i32, (screen_h / px) as i32);
    let mut out = Vec::new();
    // A run of spans laid end to end from a top-left in GUI pixels, at a
    // whole-number scale. `scale` multiplies the *font* pixel, which is why
    // the title is 4× and the subtitle 2× rather than being pre-scaled
    // strings.
    let run = |out: &mut Vec<rewo_gpu::world::OwnedTextLine>,
               line: &chat_style::ChatLine,
               x: i32,
               y: i32,
               scale: i32,
               alpha: f32| {
        let mut pen = x;
        for span in line {
            let w = rewo_gpu::text::width_styled(&span.text, advance, span.bold);
            if !span.text.is_empty() {
                out.push(rewo_gpu::world::OwnedTextLine {
                    x: pen as f32 * px,
                    y: y as f32 * px,
                    px: px * scale as f32,
                    // The span's own colour, in the LINEAR space the pass
                    // writes into an sRGB attachment. M117's coloured
                    // command runs are the precedent.
                    color_linear: srgb_bytes_to_linear_f(span.color),
                    alpha,
                    shadow: true,
                    style: text_style_of(span),
                    text: span.text.clone(),
                });
            }
            pen += w * scale;
        }
    };

    // `if (this.title != null && this.titleTime > 0)`.
    if let Some(title) = t.title.as_ref().filter(|_| t.title_time > 0) {
        let alpha =
            rewo_gpu::hud::title_alpha(t.title_time, t.fade_in, t.stay, t.fade_out, partial);
        // `if (alpha > 0)` — the draw's own guard, so a fully-faded frame
        // emits nothing rather than a transparent quad.
        if alpha > 0 {
            let a = alpha as f32 / 255.0;
            let line = chat_style::parse_component(title, ChatStyle::WHITE, lang);
            let width = styled_line_width(&line, advance);
            let (x, y) = rewo_gpu::hud::title_pos(gw, gh, width);
            run(&mut out, &line, x, y, rewo_gpu::hud::TITLE_SCALE, a);
            // The subtitle is drawn *inside* the title's block, at the title's
            // alpha — it has no ramp of its own.
            if let Some(subtitle) = &t.subtitle {
                let line = chat_style::parse_component(subtitle, ChatStyle::WHITE, lang);
                let width = styled_line_width(&line, advance);
                let (x, y) = rewo_gpu::hud::subtitle_pos(gw, gh, width);
                run(&mut out, &line, x, y, rewo_gpu::hud::SUBTITLE_SCALE, a);
            }
        }
    }

    // `if (this.overlayMessageString != null && this.overlayMessageTime > 0)`.
    // A separate block, not an `else` — an action bar and a title show at once.
    if let Some(message) = t
        .overlay_message
        .as_ref()
        .filter(|_| t.overlay_message_time > 0)
    {
        let alpha = rewo_gpu::hud::action_bar_alpha(t.overlay_message_time, partial);
        if alpha > 0 {
            let line = chat_style::parse_component(message, ChatStyle::WHITE, lang);
            let width = styled_line_width(&line, advance);
            let (x, y) = rewo_gpu::hud::action_bar_pos(gw, gh, width);
            run(&mut out, &line, x, y, 1, alpha as f32 / 255.0);
        }
    }
    out
}
