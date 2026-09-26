use super::*;

/// The app's screen state: the framework's one slot (M82) plus the cursor.
///
/// Before M82 this was the inventory's `open: bool`. The slot is
/// `rewo_world::screen::Screens` — vanilla's `Gui.screen`, a single field, not
/// a stack — and the two accessors below are the seam every input path routes
/// through:
///
/// * [`Self::any_open`] is "a screen owns the cursor and the keyboard", which
///   is what frees the mouse, holds the camera still and swallows world input.
/// * [`Self::inventory_open`] is the *inventory* specifically, which is what
///   the slot hover, the drag and `click_screen` mean.
///
/// Conflating the two is the mistake this split exists to prevent: a death
/// screen must free the cursor without making `slot_at` meaningful.
pub struct ScreenState {
    pub screens: rewo_world::screen::Screens,
    /// The recipe book's own selection (M98) — the tab, the page and whether
    /// the search field has focus. **Screen state, not menu state**: the
    /// server's `RecipeBookSettings` carries open and filtering and nothing
    /// else, so these three exist only here.
    pub book: rewo_world::recipe_book_screen::BookState,
    /// The book's search field (M99), beside `book` rather than inside it
    /// because an `EditBox` is not `Copy` and `BookState` is passed by value
    /// through the render path.
    ///
    /// **Constructed with the book's own maximum, not `EditBox::default()`'s.**
    /// `initVisuals` calls `setMaxLength(50)` while the default is 32, so
    /// deriving `Default` for this field would silently truncate a long search
    /// at 32 characters — and the difference is invisible until someone types
    /// past it. That is why `ScreenState`'s `Default` is written out below
    /// rather than derived.
    pub book_search: rewo_world::edit_box::EditBox,
    /// The open which-of-these overlay (M104), beside `book` for the same
    /// reason `book_search` is: it holds a `Vec` and `BookState` is `Copy`.
    ///
    /// **A snapshot taken when the right-click opened it**, not a view — see
    /// `recipe_overlay::Open`. Its lifetime is exactly "until the next click",
    /// because every click while it is up either selects in it or shuts it.
    pub book_overlay: Option<rewo_world::recipe_overlay::Open>,
    /// `RecipeBookComponent.lastPlacedRecipe` (M107) — which recipe the last
    /// click sent, so a repeat of an UNCRAFTABLE one can be suppressed.
    ///
    /// Screen state rather than session state, like `book` and `book_search`:
    /// nothing on the wire carries it, and vanilla keeps it on the component.
    pub place_guard: rewo_world::recipe_book_screen::PlaceGuard,
    /// Cursor position in screen pixels. Only tracked while a screen is
    /// open; the rest of the time the cursor is grabbed and its position is
    /// meaningless.
    pub mouse: (f64, f64),
    /// How many `container_close` packets had arrived last time the frame
    /// looked (M74). A watermark rather than a flag so a close that lands in
    /// the same frame as another cannot be swallowed, and so nothing has to
    /// reach into the session to clear state it does not own.
    pub close_requests_seen: u64,
    /// The beacon screen's own `primary` / `secondary` (M93m).
    ///
    /// **Vanilla's screen owns these, not the menu.** A click moves them
    /// locally and only `set_beacon` on confirm tells the server, so they
    /// cannot be re-read from the data slots each frame — that was M92's
    /// stated shortcut, and it is what this replaces.
    ///
    /// Seeded from the menu whenever *either* watermark moves: the container
    /// id, because a new menu is a new beacon, and `data_writes`, because
    /// `ContainerListener.dataChanged` re-reads both effects on ANY slot
    /// change — so a pyramid growing under you discards an unconfirmed pick,
    /// which is vanilla's behaviour and not a bug to design around.
    pub beacon: Option<BeaconLocal>,
    /// The stonecutter's screen-local scroll (M93s). Reset by `cut_local`
    /// whenever the container or its input slot changes, which is what
    /// `containerChanged` does.
    pub cut: Option<CutLocal>,
    /// The anvil's name field (M93t). Screen-local: no packet carries the text
    /// being typed, only the `rename_item` it produces.
    pub anvil: Option<AnvilLocal>,
    /// The merchant's screen-local scroll (M93u).
    pub merchant: Option<MerchantLocal>,
    /// Set when a beacon button asks for the screen to close (M93m). Drained
    /// by the frame loop rather than closing from inside the press, so the
    /// close goes through the one path that owns the screen.
    pub close_beacon: bool,
    /// The container id whose screen is currently up, if a container's (M89).
    ///
    /// A watermark on the *menu*, not a mirror of the screen's open flag: a
    /// server re-opening the same slot gets a fresh menu, and comparing
    /// against the screen state would miss it.
    pub container_shown: Option<i32>,
}

impl Default for ScreenState {
    fn default() -> Self {
        Self {
            // M99 — `initVisuals`' own `setMaxLength(50)`. Everything else is
            // its type's default; only this one field has a value the type
            // cannot know.
            book_search: rewo_world::edit_box::EditBox::new(
                rewo_world::recipe_book_screen::SEARCH_MAX_LENGTH,
            ),
            screens: Default::default(),
            book: Default::default(),
            book_overlay: Default::default(),
            place_guard: Default::default(),
            mouse: Default::default(),
            close_requests_seen: Default::default(),
            beacon: Default::default(),
            cut: Default::default(),
            anvil: Default::default(),
            merchant: Default::default(),
            close_beacon: Default::default(),
            container_shown: Default::default(),
        }
    }
}

impl ScreenState {
    /// Any screen at all.
    pub fn any_open(&self) -> bool {
        self.screens.is_open()
    }

    /// The inventory specifically.
    pub fn inventory_open(&self) -> bool {
        self.screens.is(rewo_world::screen::ScreenKind::Inventory)
    }

    /// The menu slot under the cursor, if any.
    ///
    /// Takes the layout on screen (M89): both halves of this are panel-sized.
    /// `screen_to_gui` centres the panel to find the origin, and `slot_at`
    /// scans that layout's own slots — so asking the player's 176x166 while a
    /// 176x222 chest is up shifts the cursor 28 px relative to the panel *and*
    /// then looks it up in the wrong slot list. The two errors do not cancel.
    pub(super) fn hovered(
        &self,
        layout: &'static rewo_world::menu_layout::MenuLayout,
        w: f32,
        h: f32,
        book_open: bool,
    ) -> Option<usize> {
        // **Through `hovered_menu_slot`, not its own conversion.** This method
        // used `screen_to_gui_for`, which is `Placement::centred` — so with
        // the recipe book open it resolved the cursor against a panel 77 GUI
        // px from the one the render drew, and it feeds the CLICK, the
        // double-click detector and the item-hover highlight. M89 and M106b
        // each fixed a consumer of the same predicate and each recorded that a
        // per-call-site choice is how they come to disagree; this is the third
        // time, and the first to reach an input path rather than a tooltip.
        hovered_menu_slot(layout, self.mouse, w, h, book_open)
    }
}

/// The per-screen state M85's three screens need across frames.
///
/// The death screen keeps its own field (`LiveApp::death`) because M82 gave it
/// one and its lifecycle is driven by the wire rather than by a key; these
/// three are opened and closed by presses, so one slot mirroring
/// [`rewo_world::screen::Screens`]' one slot is the honest shape.
///
/// What each variant carries is exactly what a **resize rebuild** needs —
/// `Screen.resize` is `init()`, so the builder has to be re-runnable from
/// state the app still holds.
#[derive(Clone, Debug, Default)]
pub(crate) enum ScreenView {
    #[default]
    None,
    Pause(rewo_world::pause_screen::PauseLabels, bool),
    Links(rewo_world::server_links_screen::ServerLinksLabels),
    Disconnected(
        rewo_world::disconnect_screen::DisconnectLabels,
        rewo_world::disconnect_screen::DisconnectDetails,
    ),
}

/// Everything the death screen needs across frames (M82).
///
/// The model and its labels are separate because the labels come from the
/// language map (an asset) and the model comes from the wire, and only the
/// model changes while the screen is up.
pub(crate) struct DeathView {
    pub model: rewo_world::death_screen::DeathScreen,
    pub labels: rewo_world::death_screen::DeathLabels,
    /// The death message, parsed into styled spans once. A server's is
    /// routinely coloured, and `TRUSTED_STREAM_CODEC` means the style is the
    /// server's to set.
    pub cause: Option<rewo_net::chat_style::ChatLine>,
    /// `PlaySession::respawn_epoch` when the screen opened. The screen closes
    /// when it moves — see the field's docs for why that, and not the button
    /// press, is what ends it.
    pub respawn_epoch: u64,
}

impl DeathView {
    /// `DeathScreen`'s constructor + `init()`, given a decoded kill and the
    /// baked language map.
    pub(crate) fn open(
        kill: &rewo_net::CombatKill,
        hardcore: bool,
        score: i32,
        lang: &rewo_data::lang::Language,
        respawn_epoch: u64,
        gui_w: i32,
        gui_h: i32,
    ) -> (Self, rewo_world::screen::Screen) {
        use rewo_net::chat_style::{self, ChatStyle};
        let model = rewo_world::death_screen::DeathScreen {
            // The message is kept even when it flattens to nothing: vanilla's
            // `causeOfDeath` is `@Nullable` and a *present but empty* component
            // still takes the non-null branch and draws an empty line.
            cause_of_death: Some(chat_style::plain_text(&chat_style::parse_component(
                &kill.message,
                ChatStyle::WHITE,
                Some(lang),
            ))),
            hardcore,
            score,
        };
        let labels = model.labels(lang);
        let cause = Some(chat_style::parse_component(
            &kill.message,
            ChatStyle::WHITE,
            Some(lang),
        ));
        let screen = model.build(&labels, gui_w, gui_h);
        (
            Self {
                model,
                labels,
                cause,
                respawn_epoch,
            },
            screen,
        )
    }

    /// `Screen.resize` → `repositionElements` → `rebuildWidgets` → `init()`.
    ///
    /// **`init()` resets `delayTicker` to 0 and disables the buttons again**,
    /// so resizing the window while dead restarts the one-second guard. That
    /// falls out of rebuilding rather than being coded, because
    /// [`rewo_world::death_screen::DeathScreen::build`] *is* `init()` and
    /// `Screen::new` starts its clock at zero.
    pub(crate) fn reposition(
        &self,
        screen: &mut rewo_world::screen::Screen,
        gui_w: i32,
        gui_h: i32,
    ) {
        *screen = self.model.build(&self.labels, gui_w, gui_h);
    }
}

/// Any screen's chrome: its background and its buttons (M82, generalised in
/// M85).
///
/// Pure, and takes the screen rather than the app, so the gate drives the same
/// builder the frame path does. Nothing here is death-screen-specific and
/// nothing ever was — M85 only had to teach it the two widget kinds that are
/// **not** buttons: a label draws through the text pass, and a
/// [`rewo_world::screen::WidgetKind::Reserved`] draws nothing at all, on
/// purpose (see `Widget::reserved`).
pub(crate) fn screen_chrome(
    screen: &rewo_world::screen::Screen,
    mouse: Option<(f64, f64)>,
) -> rewo_gpu::screen::ScreenDraw {
    use rewo_world::screen::{ButtonSprite as W, WidgetKind};
    let focused = screen.focused();
    rewo_gpu::screen::ScreenDraw {
        backdrop: screen.backdrop.map(|b| (b.top, b.bottom)),
        menu_background: screen
            .menu_background
            .map(|b| rewo_gpu::screen::MenuBackgroundDraw {
                in_world: b.in_world,
            }),
        buttons: screen
            .widgets
            .iter()
            .filter(|w| w.visible && w.kind == WidgetKind::Button)
            .map(|w| rewo_gpu::screen::ButtonDraw {
                x: w.x,
                y: w.y,
                width: w.width,
                height: w.height,
                sprite: match w.sprite(w.is_hovered(mouse), focused == Some(w.id)) {
                    W::Enabled => rewo_gpu::screen::ButtonSprite::Enabled,
                    W::Disabled => rewo_gpu::screen::ButtonSprite::Disabled,
                    W::Highlighted => rewo_gpu::screen::ButtonSprite::Highlighted,
                },
            })
            .collect(),
        // M84's statistics screen fills this; every other screen's chrome is
        // its backdrop and its buttons.
        sprites: Vec::new(),
        scissored: Vec::new(),
    }
}

/// Every widget's text, for a screen whose widgets carry all of it (M85).
///
/// A button's label is centred in its own rect by `defaultScrollingHelper` and
/// coloured by `WithInactiveMessage`; a `StringWidget` draws at its own `x`; a
/// `MultiLineTextWidget` draws one line per 9 px, centred about the widget's
/// midpoint when `setCentered(true)`. A `Reserved` widget draws nothing.
///
/// `px` is the GUI scale, the same convention `death_screen_lines` uses.
pub(crate) fn screen_text_lines(
    screen: &rewo_world::screen::Screen,
    advance: &[u8; 256],
    px: f32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_world::screen::WidgetKind;
    let mut out = Vec::new();
    // `color` arrives as vanilla's byte `/255` (`screen::INACTIVE_LABEL` is
    // `0xA0A0A0`); the pass wants linear. One conversion, in the one closure
    // every branch below pushes through.
    let mut push = |text: &str, x: i32, y: i32, color: [f32; 3]| {
        if text.is_empty() {
            return;
        }
        out.push(rewo_gpu::world::OwnedTextLine {
            x: x as f32 * px,
            y: y as f32 * px,
            px,
            color_linear: srgb_bytes_to_linear_f(color),
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: text.to_string(),
        });
    };
    for widget in screen.widgets.iter().filter(|w| w.visible) {
        match &widget.kind {
            WidgetKind::Reserved => {}
            // M84's tabs and image buttons. Only the statistics screen builds
            // them and it has its own text builder (`stats_view::lines`),
            // because a tab's label is centred by `MenuTabButton.renderLabel`
            // rather than by `defaultScrollingHelper` — the two differ by the
            // 3-px drop an unselected tab takes.
            WidgetKind::Sprites { .. } => {}
            // M173: a slider's label is the button's — centred, and the
            // inactive greying flows through `label_color` the same way
            // (`AbstractSliderButton` inherits `WithInactiveMessage`).
            WidgetKind::Button | WidgetKind::Slider { .. } => {
                let w = rewo_gpu::text::width(&widget.message, advance);
                let (anchor, top) = widget.label_anchor(w);
                push(&widget.message, anchor - w / 2, top, widget.label_color());
            }
            WidgetKind::Label { centered } => {
                // `StringWidget.visitLines`: `x = getX()`, and
                // `y = getY() + (getHeight() - 9) / 2`.
                let w = rewo_gpu::text::width(&widget.message, advance);
                let x = if *centered {
                    widget.x + widget.width / 2 - w / 2
                } else {
                    widget.x
                };
                let y = widget.y + (widget.height - 9) / 2;
                push(&widget.message, x, y, widget.label_color());
            }
            WidgetKind::MultiLabel { lines, centered } => {
                // `MultiLineLabel.visitLines(alignment, midX, y, 9, output)` —
                // `getTextY()` is the widget's own `y`, with no vertical
                // centring, because the widget's height *is* the text's.
                let mid = widget.x + widget.width / 2;
                for (i, line) in lines.iter().enumerate() {
                    let w = rewo_gpu::text::width(line, advance);
                    let x = if *centered { mid - w / 2 } else { widget.x };
                    push(line, x, widget.y + 9 * i as i32, widget.label_color());
                }
            }
        }
    }
    out
}

/// The death screen's four text runs — title, cause, score, and each button's
/// label (M82).
///
/// `px` is the GUI scale; every coordinate below is in GUI pixels and is
/// multiplied by it, which is the same convention `title_lines` uses.
pub(crate) fn death_screen_lines(
    view: &DeathView,
    screen: &rewo_world::screen::Screen,
    advance: &[u8; 256],
    px: f32,
    (screen_w, _screen_h): (f32, f32),
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_net::chat_style::ChatSpan;
    use rewo_world::death_screen as ds;
    let gui_w = (screen_w / px) as i32;
    let mut out = Vec::new();

    // A run of spans laid end to end from a GUI-space top-left, at a
    // whole-number extra scale. `scale` multiplies the *font* pixel, which is
    // how the title comes out double-size without a second font.
    let run = |out: &mut Vec<rewo_gpu::world::OwnedTextLine>,
               spans: &[ChatSpan],
               x: i32,
               y: i32,
               scale: i32| {
        let mut pen = x;
        for span in spans {
            let w = rewo_gpu::text::width_styled(&span.text, advance, span.bold);
            if !span.text.is_empty() {
                out.push(rewo_gpu::world::OwnedTextLine {
                    x: pen as f32 * px,
                    y: y as f32 * px,
                    px: px * scale as f32,
                    // `ActiveTextCollector.accept` builds its `GuiTextRenderState`
                    // with `ARGB.white(opacity)` as the BASE and lets the
                    // component's own `Style` override it per character — so
                    // the colour is the span's, in linear, and the five flags
                    // are the span's too.
                    color_linear: srgb_bytes_to_linear_f(span.color),
                    alpha: 1.0,
                    shadow: true,
                    style: text_style_of(span),
                    text: span.text.clone(),
                });
            }
            pen += w * scale;
        }
    };

    // The title, at `TITLE_SCALE`. Its anchor truncates twice — see
    // `rewo_world::death_screen`.
    let title_w = rewo_gpu::text::width(&view.labels.title, advance);
    run(
        &mut out,
        &[plain_span(&view.labels.title)],
        ds::title_left(gui_w, title_w),
        ds::title_top(),
        ds::TITLE_SCALE,
    );

    // The death message, in the server's own styling.
    if let Some(cause) = &view.cause {
        let w = styled_line_width(cause, advance);
        let (x, y) = ds::cause_pos(gui_w, w);
        run(&mut out, cause, x, y, 1);
    }

    // `deathScreen.score.value` — "Score: %s" with the value in YELLOW. Two
    // spans, not one: `Component.translatable(key, scoreValue)` nests a styled
    // literal inside an unstyled template, so the number is yellow and the
    // word is not.
    let score = score_spans(&view.labels.score_template, view.model.score);
    let w = styled_line_width(&score, advance);
    let (x, y) = ds::score_pos(gui_w, w);
    run(&mut out, &score, x, y, 1);

    // Each button's label, centred in its own rect by
    // `defaultScrollingHelper` and coloured by `WithInactiveMessage`.
    for widget in screen.widgets.iter().filter(|w| w.visible) {
        let w = rewo_gpu::text::width(&widget.message, advance);
        let (anchor, top) = widget.label_anchor(w);
        let mut span = plain_span(&widget.message);
        span.color = widget.label_color();
        run(&mut out, &[span], anchor - w / 2, top, 1);
    }
    out
}

pub(super) fn plain_span(text: &str) -> rewo_net::chat_style::ChatSpan {
    rewo_net::chat_style::ChatSpan {
        text: text.to_string(),
        color: [1.0, 1.0, 1.0],
        bold: false,
        italic: false,
        underlined: false,
        strikethrough: false,
        obfuscated: false,
        events: None,
    }
}

/// `Component.translatable("deathScreen.score.value", literal(score).withStyle(YELLOW))`.
///
/// The template is split on its one `%s`; the value takes
/// `ChatFormatting.YELLOW`'s `0xFFFF55`. A template with no `%s` — a resource
/// pack could ship one — yields the template alone, which is what
/// `decomposeTemplate` does with a pattern that consumes no argument.
pub(crate) fn score_spans(template: &str, score: i32) -> rewo_net::chat_style::ChatLine {
    const YELLOW: u32 = 0xFF_FF55;
    let mut out = Vec::new();
    let value = score.to_string();
    match template.split_once("%s") {
        Some((head, tail)) => {
            out.push(plain_span(head));
            let mut v = plain_span(&value);
            v.color = rewo_net::chat_style::rgb_f32(YELLOW);
            out.push(v);
            out.push(plain_span(tail));
        }
        None => out.push(plain_span(template)),
    }
    out.retain(|s| !s.text.is_empty());
    out
}

/// Build and hand over one frame of the open screen: icons, count labels,
/// the highlight and the player preview.
///
/// One function so the windowed and headless paths cannot drift — the headless
/// one exists to photograph exactly what the windowed one shows.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_screen(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    items: &rewo_data::items::Items,
    gui: &mut GuiItemState,
    baked: &assets::BakedAssets,
    skin: Option<&mut PreviewTextures>,
    mut glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    // `options.advancedItemTooltips` — F3+H (M66).
    flag: rewo_gpu::tooltip::TooltipFlag,
    mouse: (f64, f64),
    (w, h): (f32, f32),
    // M92 — the beacon's six effect ids, resolved once at startup.
    beacon_effects: BeaconEffectIds,
    // The beacon screen's own choice (M93m), or `None` to read the menu.
    beacon_override: Option<rewo_world::menu_screen::BeaconChoice>,
    // The stonecutter's grid (M93s). Resolved by the caller for the beacon's
    // reason: it needs the screen-local scroll, and `apply_screen` holds no
    // `ScreenState` — which is also what keeps it drivable from a gate.
    cut: Option<&CutView>,
    // M93t — the anvil's name field, resolved by the caller for the reason the
    // beacon's choice and the stonecutter's grid are: it is screen-local state
    // and `apply_screen` holds no `ScreenState`.
    anvil_field: Option<&rewo_world::edit_box::EditBox>,
    // M93u — the merchant's trade list, resolved by the caller for the same
    // reason: it needs the screen-local scroll.
    merchant: Option<&MerchantView>,
    // M98 — the recipe book's own tab and page, for the same reason as the
    // three above: `apply_screen` holds no `ScreenState`, which is what keeps
    // it drivable from a gate.
    book_state: rewo_world::recipe_book_screen::BookState,
    // M99 — and its search field's contents, already lowercased.
    book_query: &str,
    // M100 — and the field itself, for its text, caret and selection.
    book_field: &rewo_world::edit_box::EditBox,
    // M101 — wall-clock milliseconds, for both fields' caret blink.
    now_ms: u64,
    // M104 — and the open which-of-these overlay, supplied for the same reason
    // the four above are: it is screen-local state that `apply_screen` cannot
    // reach, and supplying it is what lets a gate drive the render path the
    // live client takes rather than a copy of it (M45/M93q).
    book_overlay: Option<&rewo_world::recipe_overlay::Open>,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
) {
    // M93t — the anvil's field is rendered ONCE, here, because its text and
    // its cursor share a width measurement: the cursor's x is the width of the
    // run before it. Splitting the two would measure the same string twice
    // with two chances to disagree.
    let (anvil_labels, anvil_fills) = match (baked.font.as_ref(), anvil_field) {
        (Some(f), Some(a)) => {
            let (l, fills, _) = anvil_field_render(a, &f.advance, w, h, now_ms);
            (l, fills)
        }
        _ => (Vec::new(), Vec::new()),
    };
    // M93q — the loom's grid, resolved here because it keys off the pattern
    // slot's item NAME and only this side holds the registry.
    let loom = session.menus.open().and_then(|m| {
        if m.layout.protocol_id != rewo_world::menu_screen::LOOM_MENU_PROTOCOL_ID {
            return None;
        }
        // Slots: banner 0, dye 1, pattern 2.
        let name = |slot: usize| m.menu.menu_slot(slot).and_then(|s| items.name(s.item_id));
        let patterns = rewo_data::loom_pattern_table::selectable_patterns(name(2));
        Some(LoomView {
            patterns,
            // The scrollbar's drag is not wired; see `LoomView::start_row`.
            start_row: 0,
            selected: m.loom_selected_pattern(),
            display: rewo_world::menu_screen::loom_display_patterns(
                m.menu.menu_slot(0).is_some(),
                m.menu.menu_slot(1).is_some(),
                // `hasMaxPatterns` needs the banner's own layer count, which
                // lives in a component Rewo does not read — so the grid stays
                // visible on a full banner where vanilla hides it.
                false,
                patterns.len(),
            ),
        })
    });
    // M94 — the recipe book, if the server says it is open for this menu's
    // book type. It is built here rather than inside `container_panel` because
    // its presence has to reach `screen_to_gui_placed` below as well: an open
    // book MOVES the menu, and a hover resolved against a centred panel while
    // the panel is drawn 77 px right would be wrong by more than four slots.
    // M98 — the cursor in the book's own space, computed ONCE here so the
    // hover and the press cannot read different numbers.
    let book_mouse = {
        let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(w, h);
        Some((
            ((mouse.0 - bl as f64) / scale as f64).floor() as i32,
            ((mouse.1 - bt as f64) / scale as f64).floor() as i32,
        ))
    };
    // M103 — the ghost recipe, from `place_ghost_recipe`, and its two washes.
    let ghosts = live_ghosts(session);
    let (ghost_under, ghost_over) = ghost_washes(
        &ghosts,
        session.shown_menu().layout(),
        session.menus.open().is_none(),
    );
    let book = live_recipe_book(
        session,
        items,
        book_state,
        book_mouse,
        book_query,
        &baked.item_names,
    );
    // Which menu is on screen: the open container if there is one, else the
    // player's own. Chosen ONCE and threaded everywhere, because the panel,
    // the icons, the hover and the durability bars are all measured from the
    // same origin — and that origin depends on the menu's panel size. Picking
    // it per-consumer is how a chest ends up painted at one size with its
    // icons placed at another.
    let menu = session
        .menus
        .open()
        .map(|m| &m.menu)
        .unwrap_or(&session.inventory);
    let layout = menu.layout();

    // The container's own background sheet, or `None` for the player's
    // inventory, which the pass draws from its own `inventory.png` rect.
    //
    // The cursor goes in through the SAME panel size the panel itself uses
    // (M87k's rule): the enchanting table's row highlight is measured from the
    // panel's top-left, so converting against the player's 176x166 would offset
    // it wherever the two disagree.
    // M94 — the book, beside the panel. Set unconditionally so a shut book
    // clears last frame's, and set even when `container_panel` returns `None`
    // (the player's own inventory), which is one of the four screens that has
    // one.
    // M100 — the search field's own quads and text. Built only when the book is
    // open, and only when there is a font to measure with.
    // M106b — ONE binding for "the book is open", read by the panel's origin,
    // the slot rects, the hover highlight, the tooltip and the enchanting
    // rows. Each of those used to spell `book_open` at its own call site,
    // which is how the tooltip and the highlight came to be the two that
    // never learnt about the shift (M89's rule: a per-call-site choice is how
    // they come to disagree).
    //
    // This is `apply_screen`'s composition root and no test reaches it — the
    // function needs a `PlaySession`. A mutation pinning this to `false`
    // therefore survives, and is recorded rather than papered over; what the
    // consolidation buys is that the five consumers can no longer disagree
    // with each other, which is the failure that actually happened.
    let book_open = book.is_some();
    // The ghost's items rotate on the SAME 30-tick clock the book's recipe
    // cells use (M95) — `slotSelectTime` is one object shared by the page, the
    // overlay and the ghost. Bound once so the icons and the ghost's tooltip
    // cannot be a frame apart on it.
    let ghost_cycle =
        (session.ticks as f32 / rewo_net::recipe_book::TICKS_TO_SWAP_SLOT).floor() as i32;
    // M105 — one call rather than two, so the composition of the field's text
    // with the page counter is inside a function a test can reach. This
    // function cannot be one: it needs a `PlaySession`.
    let (book_field_labels, book_field_fills) = match (book.as_ref(), baked.font.as_ref()) {
        (Some(b), Some(f)) => {
            book_labels(b, book_field, &baked.lang, &f.advance, w, h, now_ms)
        }
        _ => (Vec::new(), Vec::new()),
    };
    wr.set_recipe_book(book.as_ref().and_then(|b| {
        recipe_book_panel(b, &book_field_fills, book_overlay, book_mouse)
    }));
    wr.set_container_panel(container_panel(
        layout,
        session.menus.open(),
        EnchantPlayer {
            xp_level: session.hud.experience.level,
            creative: session.abilities.instabuild,
            beacon_effects,
            beacon_override,
            loom,
            cut,
            anvil_fills: &anvil_fills,
            merchant,
            ghost_under: &ghost_under,
            ghost_over: &ghost_over,
        },
        Some(rewo_gpu::container::screen_to_gui_placed(
            mouse,
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                layout.image_w as f32,
                layout.image_h as f32,
                book_open,
            ),
        )),
    ));

    let (mut icons, mut labels) =
        screen_icons(
            menu,
            items,
            &session.trim_materials,
            w,
            h,
            session.menus.open(),
            cut,
            merchant,
            book.as_ref(),
            &ghosts,
            ghost_cycle,
            book_overlay,
        );
    if let Some((icon, label)) = carried_icon(menu, items, &session.trim_materials, mouse, w, h) {
        icons.push(icon);
        labels.extend(label);
    }
    // M92 — the enchanting table's three cost numerals, from the SAME row
    // derivation the overlays used.
    if let (Some(rows), Some(font)) = (
        enchant_rows_of(
            layout,
            session.menus.open(),
            EnchantPlayer {
                xp_level: session.hud.experience.level,
                creative: session.abilities.instabuild,
                beacon_effects,
                beacon_override,
                loom,
                cut,
                anvil_fills: &anvil_fills,
                merchant,
                ghost_under: &ghost_under,
                ghost_over: &ghost_over,
            },
            Some(rewo_gpu::container::screen_to_gui_placed(
                mouse,
                w,
                h,
                rewo_gpu::container::Placement::with_book(
                    layout.image_w as f32,
                    layout.image_h as f32,
                    book_open,
                ),
            )),
        ),
        baked.font.as_ref(),
    ) {
        labels.extend(enchant_cost_labels(rows, &font.advance, w, h));
    }
    // The text and the append cursor are labels; the insert cursor and the
    // selection travelled with the panel as solid quads — M93q's `FILL_SPRITE`
    // doing the job it was built for, one screen over.
    labels.extend(anvil_labels);
    // M100 — the book's search field, over the book's own panel.
    labels.extend(book_field_labels);
    apply_gui_icons(wr, gpu, gui, &icons);

    wr.set_container(
        true,
        hovered_slot_position(layout, mouse, w, h, book_open),
    );

    // Every visible slot's durability bar, plus the cursor's. The screen's
    // rects and the hotbar's go through the same builder.
    {
        let rects = menu_slot_rects(menu, w, h, book_open);
        let mut stacks: Vec<_> = (0..menu.slot_count())
            .map(|i| (menu.menu_slot(i), rects[i]))
            .collect();
        if let Some(carried) = menu.carried() {
            if let Some((icon, _)) = carried_icon(menu, items, &session.trim_materials, mouse, w, h) {
                stacks.push((Some(carried), (icon.x, icon.y, icon.size)));
            }
        }
        // `!has(UNBREAKABLE)`. No item's *prototype* carries the component in
    // 26.2 — it is only ever patched on — so the patch flag is the whole
    // answer here.
    wr.set_item_bars(item_bars(&stacks, items, |s| {
        session.inventory.text_of(s).is_some_and(|t| t.unbreakable)
    }));
    }

    // The tooltip's box and its one line. Both need the font's advances, so a
    // build with no baked font simply draws no tooltip.
    let tooltip = wr.font_advance().and_then(|advance| {
        // M106c — the frame's three producers, in vanilla's precedence. Named
        // rather than chained with `or_else` here: see `frame_tooltip`.
        frame_tooltip(
        &mut glyphs,
        // M133 — the three widgets that carry an `AbstractWidget` tooltip.
        |glyphs| {
            let b = book.as_ref()?;
            let view = b.view?;
            let (bx, by) = book_mouse?;
            let hit = rewo_world::recipe_book_screen::book_hit(bx, by, view, view.tabs)?;
            let key = rewo_world::recipe_book_screen::widget_tooltip_key(
                hit,
                book_type_of(layout)?,
                view.filtering,
                view.page,
                view.total_pages,
            )?;
            tooltip_layout(
                vec![vec![rewo_gpu::tooltip::Span::new(
                    baked.lang.or_key(key).to_string(),
                    [1.0, 1.0, 1.0],
                )]],
                advance,
                glyphs.as_deref_mut(),
                mouse,
                (w, h),
            )
        },
        |glyphs| screen_tooltip(
            menu,
            items,
            &baked.item_names,
            &baked.lang,
            &session.enchantments,
            &baked.enchantment_text,
            &session.stack_details,
            session.component_names.as_deref(),
            flag,
            advance,
            glyphs.as_deref_mut(),
            mouse,
            (w, h),
            layout,
            session.menus.open(),
            session
                .game_state
                .game_mode()
                .is_some_and(|m| m.is_spectator()),
            book_open,
        ),
        // The book's cell tooltip, SECOND. Vanilla calls it AFTER the
        // container's and it still loses — first-wins, not last-wins.
        |glyphs| {
            book_tooltip(
                book.as_ref()?,
                book_overlay.is_some(),
                book_mouse,
                items,
                &baked.item_names,
                &baked.lang,
                flag,
                advance,
                glyphs.as_deref_mut(),
                mouse,
                (w, h),
            )
        },
        // And the ghost's, THIRD — `RecipeBookComponent.extractTooltip` runs
        // the page's then the ghost's.
        |glyphs| {
            ghost_tooltip(
                &ghosts,
                ghost_cycle,
                layout,
                book_open,
                items,
                &baked.item_names,
                advance,
                glyphs.as_deref_mut(),
                mouse,
                (w, h),
            )
        },
        )
    });
    wr.set_container_tooltip(tooltip.as_ref().map(|(draw, _, _)| draw.clone()));

    // The preview's pass is built the first time the screen opens, so a
    // session that never opens it never pays for the second entity atlas.
    if !wr.preview_ready() {
        if let Err(e) = wr.init_preview(gpu, font_data(baked), entity_textures(baked)) {
            log::warn!("live: inventory preview unavailable: {e}");
        }
    }
    if wr.preview_ready() {
        let held = [
            session
                .inventory
                .held()
                .and_then(|s| items.name(s.item_id)),
            session
                .inventory
                .offhand()
                .and_then(|s| items.name(s.item_id)),
        ];
        // The skin and the cape go into the preview's *own* atlas the first
        // time the screen opens, and stay there. Both have to be uploaded
        // again here rather than reusing the world pass's addresses: the two
        // passes hold separate atlases, and a cape address is an absolute
        // texel origin, so a borrowed one samples a fixed wrong rectangle.
        let (skin_uv, slim, cape_origin) = match skin {
            Some(t) => {
                if t.skin_uv.is_none() {
                    if let Some((rgba, _)) = t.skin.as_ref() {
                        t.skin_uv = wr.upload_preview_skin(gpu, rgba);
                    }
                }
                if t.cape_origin.is_none() {
                    if let Some(rgba) = t.cape.as_ref() {
                        t.cape_origin = wr.upload_preview_cape(gpu, rgba);
                    }
                }
                (
                    t.skin_uv,
                    t.skin.as_ref().is_some_and(|(_, slim)| *slim),
                    t.cape_origin,
                )
            }
            None => (None, false, None),
        };
        let (draw, vp, rect) =
            preview_draw(session, skin_uv, slim, cape_origin, held, w, h, mouse);
        if let Err(e) = wr.prepare_held_items(gpu, &held.iter().flatten().copied().collect::<Vec<_>>()) {
            log::warn!("live: preview held items: {e}");
        }
        wr.set_preview(Some((&draw, vp, rect)));
    }
    // The tooltip's text goes last so it draws over the icons and the count
    // labels, matching the order the box is drawn in.
        let velvet_runs: Vec<rewo_gpu::velvet_text::OwnedRun> = tooltip
        .as_ref()
        .map(|(_, _, r)| r.clone())
        .unwrap_or_default();
    labels.extend(tooltip.into_iter().flat_map(|(_, l, _)| l));
    (labels, velvet_runs)
}

/// Handle a click on the open screen: predict, send, then apply locally.
///
/// Sending before applying, so a send that fails cannot leave the screen
/// showing a move the server never heard about. The packet carries the
/// prediction and the *pre-click* state id, so the order is invisible on the
/// wire — it only decides what happens when the socket is broken.
///
/// A click that cannot be predicted is dropped entirely rather than sent with
/// an empty changed-slot map, which the server would reject and answer with a
/// full resynchronisation.
/// What the player did to the hovered slot (M35, M39, M40).
///
/// One enum rather than a pile of booleans because each variant is a
/// **different `ContainerInput`**, not a modifier on one — `doClick` branches
/// on the input before it ever reads the button, so shift-clicking is not
/// "a click with shift" in the protocol's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotAction {
    /// A plain click. `0` primary, `1` secondary.
    Pickup(i8),
    /// Shift-click.
    QuickMove,
    /// A number key (an **inventory** index, `0..9`) or F (`40`).
    Swap(i32),
    /// Q, or Ctrl+Q for the whole stack.
    Throw { all: bool },
    /// The second click of a double click.
    PickupAll,
}

/// The screen's own keys (M40): a number key or F swaps, Q throws.
///
/// `AbstractContainerScreen.checkHotbarKeyPressed` maps the nine hotbar
/// keys and the off-hand key to a `SWAP` whose button is the **inventory
/// index**, and `keyPressed` maps the drop key to a `THROW` whose button
/// distinguishes one item from the stack.
pub(super) fn screen_key_action(key: PhysicalKey, ctrl: bool) -> Option<SlotAction> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    if let Some(n) = digit_key(code) {
        return Some(SlotAction::Swap(n as i32));
    }
    match code {
        // `Inventory.SLOT_OFFHAND`, the one button outside `0..9`.
        KeyCode::KeyF => Some(SlotAction::Swap(
            rewo_world::inventory::SWAP_OFFHAND_BUTTON,
        )),
        KeyCode::KeyQ => Some(SlotAction::Throw { all: ctrl }),
        _ => None,
    }
}

/// A quick-craft drag in progress (M40).
///
/// The drag is **three packets**, not one: a begin, one add per slot, and an
/// end that carries the whole changed-slot map. Only the end predicts
/// anything, so this holds the slots as they are touched and does the
/// arithmetic once at release.
#[derive(Clone, Debug, Default)]
pub(crate) struct DragState {
    /// `quickcraftType` — 0 spreads the stack evenly, 1 puts one in each.
    pub(super) kind: i32,
    /// Every slot the cursor has crossed, in order. Filtered at release, not
    /// here, so the same list can be re-tested if the cursor stack changed.
    pub(super) touched: Vec<usize>,
    pub(super) active: bool,
}

impl DragState {
    /// Begin a drag with the button that started it. Returns the phase-0
    /// packet to send.
    pub(super) fn begin(&mut self, button: i8) -> i32 {
        // Left drag spreads, right drag places one each — vanilla reads the
        // button at `mouseDragged` time, not at press.
        self.kind = if button == 0 {
            rewo_world::inventory::QUICK_CRAFT_SPLIT
        } else {
            rewo_world::inventory::QUICK_CRAFT_ONE
        };
        self.touched.clear();
        self.active = true;
        self.kind
    }

    pub(super) fn add(&mut self, slot: usize) -> bool {
        if !self.active || self.touched.contains(&slot) {
            return false;
        }
        self.touched.push(slot);
        true
    }
}

/// Run a drag to its end: send the accepted adds, then the end packet.
///
/// The three phases are sent here rather than as they happen because a slot
/// only joins the drag if the server would accept it, and that test needs the
/// cursor stack — which is unchanged throughout, so testing once at release
/// gives the same answer while keeping the state machine in one place.
pub(super) fn finish_drag(
    session: &mut PlaySession,
    items: &rewo_data::items::Items,
    drag: &mut DragState,
) {
    let touched = std::mem::take(&mut drag.touched);
    let kind = drag.kind;
    drag.active = false;
    if touched.is_empty() || session.inventory.carried().is_none() {
        return;
    }
    // M152: snapshot the three wire-derived smithing sets before the
    // mutable borrows below. Owned, because a closure holding
    // `session.recipes.as_ref()` cannot coexist with `shown_menu_mut()`.
    let smithing = session
        .recipes
        .as_ref()
        .map(rewo_net::recipe_book::SmithingSets::from_packet);
    let props = |id: i32| item_props(items, id, smithing.as_ref());
    let accepted = session.inventory.quick_craft_accepts(&touched, kind, &props);
    if accepted.is_empty() {
        return;
    }
    // A one-slot drag is a plain click in disguise — vanilla resets the
    // quick-craft state and re-dispatches it as `PICKUP`, so sending it as a
    // drag would desync a prediction the server never makes.
    if let Some((slot, button)) =
        rewo_world::inventory::Inventory::quick_craft_is_pickup(&accepted, kind)
    {
        // A one-slot drag is re-dispatched as PICKUP, so it reaches
        // `slotClicked` exactly as a click does and must run the same toggle
        // (M93i). Vanilla has ONE `slotClicked` override; two call sites here
        // with only one of them toggling is how they would come to disagree.
        let toggle = session.crafter_slot_click(
            slot as i32,
            button,
            rewo_world::inventory::CONTAINER_INPUT_PICKUP,
        );
        if toggle != rewo_world::menu::CrafterToggle::None {
            println!("[rewo-m93i] CRAFTER slot {slot}: {toggle:?} (from a one-slot drag)");
        }
        if let Some(p) = session.shown_menu_mut().click_pickup(slot as i32, button, &props) {
            if session
                .container_click_input(&p, rewo_world::inventory::CONTAINER_INPUT_PICKUP)
                .is_ok()
            {
                session.shown_menu_mut().apply_prediction(&p);
            }
        }
        return;
    }
    let Some(end) = session.shown_menu_mut().click_quick_craft(&accepted, kind, &props) else {
        return;
    };
    use rewo_world::inventory::Inventory as Inv;
    let input = rewo_world::inventory::CONTAINER_INPUT_QUICK_CRAFT;
    let carried = session.inventory.carried();
    // Phase 0 and the phase-1 adds change nothing; only their button and slot
    // carry information.
    let phase = |slot: i16, header: i32| rewo_world::inventory::ClickPrediction {
        slot,
        button: Inv::quick_craft_button(kind, header),
        changed: Vec::new(),
        carried,
    };
    if session
        .container_click_input(&phase(rewo_world::inventory::QUICK_CRAFT_NO_SLOT, 0), input)
        .is_err()
    {
        return;
    }
    for &slot in &accepted {
        if session
            .container_click_input(&phase(slot as i16, 1), input)
            .is_err()
        {
            return;
        }
    }
    if session.container_click_input(&end, input).is_ok() {
        session.shown_menu_mut().apply_prediction(&end);
    }
}

pub(super) fn click_screen(
    session: &mut PlaySession,
    items: &rewo_data::items::Items,
    screen: &mut ScreenState,
    action: SlotAction,
    w: f32,
    h: f32,
) {
    let book_open = book_visible(session);
    let Some(slot) = screen.hovered(session.shown_menu().layout(), w, h, book_open) else {
        return;
    };
    // M107 — `AbstractRecipeBookScreen.slotClicked` calls
    // `recipeBookComponent.slotClicked(slot)` after `super`, and that resets
    // `lastPlacedRecipe` and clears the ghost whenever the slot is a crafting
    // one. Placed HERE rather than after the send, because vanilla's reset is
    // gated only on which slot was clicked: a click that moves nothing still
    // clears it.
    {
        let layout = session.shown_menu().layout();
        let player_inventory =
            layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID;
        if book_type_of(layout).is_some_and(|b| {
            rewo_world::recipe_book_screen::is_crafting_slot(b, player_inventory, slot)
        }) {
            screen.place_guard.crafting_slot_clicked();
            session.ghost_recipe = None;
        }
    }
    // M152: snapshot the three wire-derived smithing sets before the
    // mutable borrows below. Owned, because a closure holding
    // `session.recipes.as_ref()` cannot coexist with `shown_menu_mut()`.
    let smithing = session
        .recipes
        .as_ref()
        .map(rewo_net::recipe_book::SmithingSets::from_packet);
    let props = |id: i32| item_props(items, id, smithing.as_ref());
    use rewo_world::inventory as inv;
    let slot = slot as i32;
    let (input, button, predicted) = match action {
        SlotAction::Pickup(b) => (0, b, session.shown_menu_mut().click_pickup(slot, b, &props)),
        SlotAction::QuickMove => (
            inv::CONTAINER_INPUT_QUICK_MOVE,
            0,
            session.shown_menu_mut().click_quick_move(slot, &props),
        ),
        // The button here is an inventory index, not a menu slot — see
        // `Inventory::click_swap`.
        SlotAction::Swap(index) => (
            inv::CONTAINER_INPUT_SWAP,
            index as i8,
            session.shown_menu_mut().click_swap(slot, index, &props),
        ),
        SlotAction::Throw { all } => {
            let b = i8::from(all);
            (
                inv::CONTAINER_INPUT_THROW,
                b,
                session.shown_menu_mut().click_throw(slot, b, &props),
            )
        }
        SlotAction::PickupAll => (
            inv::CONTAINER_INPUT_PICKUP_ALL,
            0,
            session.shown_menu_mut().click_pickup_all(slot, 0, &props),
        ),
    };
    let _ = button;
    let Some(prediction) = predicted else {
        log::debug!("live: {action:?} on slot {slot} not predictable — not sent");
        return;
    };
    if prediction.changed.is_empty() && prediction.carried == session.shown_menu().carried() {
        // Nothing moved (an empty slot with an empty cursor, or a placement the
        // slot refuses). Vanilla still sends it; there is no reason to.
        return;
    }
    if let Err(e) = session.container_click_input(&prediction, input) {
        log::warn!("live: container_click: {e}");
        return;
    }
    session.shown_menu_mut().apply_prediction(&prediction);
}
