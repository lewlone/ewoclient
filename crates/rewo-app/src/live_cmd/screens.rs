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
                &advance,
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
            &advance,
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
                &advance,
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
                &advance,
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

/// The 46 slot rects the screen draws icons into, in screen pixels.
///
/// The same shape `hotbar_slot_rects` returns, because they feed the same
/// pass: an icon in an inventory slot is the same draw as an icon in a hotbar
/// slot, and only the rectangle differs.
/// The pass's panel description for a menu, or `None` for the player's own.
///
/// This is where `menu_screen`'s sheet-relative UVs are converted back to
/// PIXELS, which is the whole reason `PanelBlit` carries pixels: a screen's
/// UVs are normalised against the sheet size its blit declares — 256 for
/// twenty-one of them and **512 for the merchant** — while the atlas
/// normalises against its own dimensions. Handing the UVs straight across
/// would divide by the wrong number for exactly one screen.
///
/// `None` for a menu with no container screen (`lectern`, a `BookViewScreen`)
/// as well as for the player's, so an open lectern paints no panel rather than
/// some other menu's.
pub(super) fn container_panel(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
) -> Option<rewo_gpu::container::ContainerPanel> {
    if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID {
        return None;
    }
    let screen = rewo_world::menu_screen::screen_of(layout.protocol_id)?;
    let sheet = sheet_index(screen.texture)?;
    let blits = rewo_world::menu_screen::background_quads(screen)
        .into_iter()
        .map(|q| rewo_gpu::container::PanelBlit {
            dx: q.dx as f32,
            dy: q.dy as f32,
            w: q.w as f32,
            h: q.h as f32,
            sx: q.u0 * screen.sheet_w,
            sy: q.v0 * screen.sheet_h,
            // A background quad samples 1:1 — the panel sheet is drawn at its
            // own scale.
            sw: q.w as f32,
            sh: q.h as f32,
            tint: [1.0; 4],
        })
        .collect();
    Some(rewo_gpu::container::ContainerPanel {
        sheet,
        blits,
        gui_w: screen.image_w as f32,
        gui_h: screen.image_h as f32,
        overlays: {
            let mut o = menu_overlays(layout, open, player, mouse_gui);
            // M103 — the ghost's red wash goes UNDER the icons, so with the
            // screen's own overlays.
            o.extend_from_slice(player.ghost_under);
            o
        },
        front_overlays: player.ghost_over.to_vec(),
    })
}

/// A `menu_screen::ProgressBlit` in the render's own units.
pub(super) fn to_blit(b: rewo_world::menu_screen::ProgressBlit) -> rewo_gpu::container::PanelBlit {
    rewo_gpu::container::PanelBlit {
        dx: b.dx as f32,
        dy: b.dy as f32,
        w: b.w as f32,
        h: b.h as f32,
        sx: b.sx as f32,
        sy: b.sy as f32,
        // M93p — `None` resolves to the destination size, which every overlay
        // before the loom preview used.
        sw: b.source_size().0 as f32,
        sh: b.source_size().1 as f32,
        // M93q — a sprite blit is untinted; `to_fill` is the tinted form.
        tint: [1.0; 4],
    }
}

/// The scroller thumb's blit — 12x15 at `leftPos + 119`, whose y the caller
/// computes from `scrollOffs` (M93s).
pub(super) fn cut_scroller_blit(y: i32) -> rewo_world::menu_screen::ProgressBlit {
    use rewo_world::menu_screen as ms;
    rewo_world::menu_screen::ProgressBlit {
        dx: ms::CUT_SCROLLER_X,
        dy: y,
        w: ms::CUT_SCROLLER_W,
        h: ms::CUT_SCROLLER_H,
        sx: 0,
        sy: 0,
        src: None,
    }
}

/// A solid-colour overlay quad (M93q) — the loom's grey banner backing, and
/// anything else vanilla draws with `GuiGraphics.fill`.
///
/// `rgb` is an **sRGB** 0xRRGGBB, which is how vanilla's colour constants are
/// written; the shader linearises it, the same discipline every Rewo UI pass
/// follows.
pub(super) fn to_fill(
    b: rewo_world::menu_screen::ProgressBlit,
    rgb: u32,
) -> (usize, rewo_gpu::container::PanelBlit) {
    let c = |shift: u32| ((rgb >> shift) & 0xFF) as f32 / 255.0;
    (
        rewo_gpu::container::FILL_SPRITE,
        rewo_gpu::container::PanelBlit {
            tint: [c(16), c(8), c(0), 1.0],
            ..to_blit(b)
        },
    )
}

/// The open menu's enchanting rows, or `None` when it is not that menu.
///
/// **One derivation, two consumers** — the sprite overlays and the cost
/// labels. They must agree about which row is hovered and which is
/// unaffordable, because a row whose background says "available" while its
/// numeral says otherwise is not a state vanilla can produce; deriving it
/// twice gives two chances to disagree (M18's finding, in miniature).
pub(crate) fn enchant_rows_of(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
) -> Option<[rewo_world::menu_screen::EnchantRow; 3]> {
    if layout.protocol_id != 13 {
        return None;
    }
    let m = open?;
    Some(rewo_world::menu_screen::enchant_rows(
        m.enchant_costs(),
        m.enchant_lapis(),
        player.xp_level,
        player.creative,
        mouse_gui,
    ))
}

/// An enchanting-row press (M92f). Returns whether the press was taken.
///
/// Mirrors `EnchantmentScreen.mouseClicked`: the row loop runs first, and a
/// row is taken **only if `clickMenuButton` would return true**. A press on a
/// row that fails the gate is *not* consumed — vanilla falls through to
/// `super.mouseClicked`, so the slot logic still gets it.
///
/// The gate is deliberately not the render's: it additionally requires slot 0
/// to hold something, and tests the level against `row + 1` as well as the
/// cost. See `menu_screen::enchant_click_allowed`.
pub(super) fn enchant_press(
    session: &mut PlaySession,
    screen: &ScreenState,
    w: f32,
    h: f32,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != 13 {
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let Some(row) = rewo_world::menu_screen::enchant_click_row(gx, gy) else {
        return false;
    };
    let allowed = rewo_world::menu_screen::enchant_click_allowed(
        row,
        open.enchant_costs(),
        open.enchant_lapis(),
        // `enchantSlots.getItem(0)` — the item being enchanted.
        open.menu.menu_slot(0).is_some(),
        session.hud.experience.level,
        session.abilities.instabuild,
    );
    if !allowed {
        return false;
    }
    if let Err(e) = session.container_button_click(row as i32) {
        log::warn!("enchant button {row}: {e}");
    }
    true
}

/// The beacon's live choice, from its data slots (M92).
///
/// **Vanilla's screen keeps its own `primary`/`secondary` fields**, seeded
/// from the menu by a `ContainerListener` on every `dataChanged` and then
/// moved by clicks *before* the server hears about them — a click updates the
/// screen and only `ServerboundSetBeaconPacket` on confirm tells the server.
/// Rewo has no click path here yet, so this reads the data slots directly:
/// the display is correct for whatever the server last said, and the
/// unconfirmed-choice half arrives with the button clicks.
/// `BeaconScreen`'s button press (M93m).
///
/// Placed beside `enchant_press` and called from the same seam, for the same
/// reason: `BeaconScreen.mouseClicked` is `AbstractContainerScreen`'s with
/// widgets in front of it, so a press on a live button consumes the click and
/// never reaches the slot logic. Returns whether it did.
///
/// The confirm sends and then closes; vanilla's order is the same
/// (`send(...)` then `closeContainer()`), and here the send failing must not
/// close the screen over a beacon the server never heard about.
pub(super) fn beacon_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    effect_ids: &BeaconEffectIds,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::menu_screen::BeaconPress;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != BEACON_MENU_PROTOCOL_ID {
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let choice = beacon_live(screen, open, effect_ids);
    let Some(button) = rewo_world::menu_screen::beacon_buttons()
        .into_iter()
        .find(|b| rewo_world::menu_screen::beacon_button_hovered(*b, gx, gy))
    else {
        return false;
    };
    match rewo_world::menu_screen::beacon_press(button, choice) {
        // An inactive or already-selected button is NOT a consumed click:
        // `AbstractWidget.mouseClicked` only returns true when it fires, so a
        // press on a dark button falls through to the slot logic exactly as a
        // disabled enchanting row does (M92f).
        BeaconPress::None => false,
        BeaconPress::Select(next) => {
            if let Some(b) = screen.beacon.as_mut() {
                b.choice = next;
            }
            true
        }
        BeaconPress::Confirm => {
            let id = |e: Option<rewo_world::menu_screen::BeaconEffect>| {
                e.and_then(|e| effect_ids.id_of(e))
            };
            match session.set_beacon(id(choice.primary), id(choice.secondary)) {
                Ok(()) => screen.close_beacon = true,
                Err(e) => log::warn!("set_beacon: {e}"),
            }
            true
        }
        BeaconPress::Cancel => {
            screen.close_beacon = true;
            true
        }
    }
}

/// `minecraft:menu`'s `beacon` id.
pub(super) const BEACON_MENU_PROTOCOL_ID: i32 = 9;

/// What the loom's pattern grid needs, resolved by the caller (M93q).
///
/// Carried rather than derived in the overlay builder because the pattern list
/// keys off the pattern slot's item NAME, and only the caller holds the
/// registry that turns an id into one — the same reason `ItemProps` is an
/// input to the click arithmetic.
#[derive(Debug, Clone, Copy)]
pub struct LoomView {
    /// `getSelectablePatterns()`.
    pub patterns: &'static [&'static str],
    /// `LoomScreen.startRow` — the first visible row.
    ///
    /// **Always 0 today**: the scrollbar's drag is not wired, so a loom with
    /// more than 16 patterns shows the first sixteen and no more. The field
    /// exists because the index arithmetic needs it and would otherwise have
    /// a 0 baked in where a variable belongs.
    pub start_row: i32,
    /// `getSelectedBannerPatternIndex()`, from data slot 0. `-1` for none.
    pub selected: i32,
    /// `displayPatterns` — the grid is hidden entirely when false.
    pub display: bool,
}

/// `AnvilScreen.keyPressed` and `EditBox.charTyped` (M93t).
///
/// ```java
/// public boolean keyPressed(final KeyEvent event) {
///    if (event.isEscape()) { this.minecraft.player.closeContainer(); return true; }
///    return !this.name.keyPressed(event) && !this.name.canConsumeInput()
///        ? super.keyPressed(event) : true;
/// }
/// ```
///
/// **Read that return carefully.** It falls through to `super` only when the
/// box did *not* handle the key **and** cannot consume input. With the field
/// focused and editable — which it is whenever slot 0 holds something — the
/// second half is false, so **every non-escape key is swallowed**: `E` does not
/// close the anvil, a number key does not swap a hotbar slot, `Q` does not
/// drop. That reads like a bug and is exactly what typing a name requires.
///
/// With slot 0 empty the field is uneditable, `canConsumeInput` is false, and
/// the screen behaves normally again.
///
/// Returns whether the key was consumed.
pub(super) fn anvil_key(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    // M101 — for `follow_cursor`, whose width function needs the font.
    baked: Option<&assets::BakedAssets>,
    input: rewo_world::edit_box::Input,
    clipboard: &mut String,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ANVIL_MENU_PROTOCOL_ID {
        return false;
    }
    // Escape is handled by the screen's own close path, before the field.
    if input.key == 256 {
        return false;
    }
    let slot0 = open
        .menu
        .menu_slot(0)
        .and_then(|s| items.name(s.item_id).map(|n| (s, n)));
    let handled = {
        let local = anvil_local(screen, open, items);
        let handled = local.field.key_pressed(input, clipboard);
        rewo_world::anvil::key_consumed(handled, local.field.can_consume_input())
    };
    if let Some(a) = screen.anvil.as_mut() {
        follow_cursor(&mut a.field, baked, ANVIL_FIELD.2);
    }
    anvil_flush(session, screen, slot0);
    handled
}

/// A typed character — `EditBox.charTyped`, which the key path never sees
/// because winit reports text separately from the key (M93t).
pub(super) fn anvil_char(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    baked: Option<&assets::BakedAssets>,
    ch: char,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ANVIL_MENU_PROTOCOL_ID {
        return false;
    }
    let slot0 = open
        .menu
        .menu_slot(0)
        .and_then(|s| items.name(s.item_id).map(|n| (s, n)));
    let handled = anvil_local(screen, open, items).field.char_typed(ch);
    if let Some(a) = screen.anvil.as_mut() {
        follow_cursor(&mut a.field, baked, ANVIL_FIELD.2);
    }
    anvil_flush(session, screen, slot0);
    handled
}

/// Drain the field's responder into `AnvilName::on_name_changed`, and send the
/// packet it asks for.
///
/// The two-stage gate is M93n's and both stages are real: the field fires on
/// every mutation, `on_name_changed` normalises "the item's own name" to the
/// empty string, and `setItemName` refuses to re-send a name the server already
/// has.
pub(super) fn anvil_flush(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    slot0: Option<(rewo_world::inventory::ItemSlot, &str)>,
) {
    let Some(local) = screen.anvil.as_mut() else {
        return;
    };
    let Some(typed) = local.field.take_value_changed() else {
        return;
    };
    let hover = slot0.map(|(_, n)| display_name_of(n));
    let input = slot0.zip(hover.as_deref()).map(|((s, _), hover_name)| {
        rewo_world::anvil::AnvilInput {
            // A stack whose patch carried anything is the closest Rewo gets to
            // `has(CUSTOM_NAME)` without decoding the component's text; the
            // approximation is one-directional, since a patched-but-unnamed
            // stack merely skips the clear-to-default normalisation.
            has_custom_name: s.has_components,
            hover_name,
        }
    });
    if let Some(send) = local.name.on_name_changed(&typed, input) {
        if let Err(e) = session.rename_item(&send) {
            log::warn!("anvil rename {send:?}: {e}");
        }
    }
}

/// `MerchantScreen.mouseClicked` — a trade button, or a scrollbar grab (M93u).
///
/// The button's press sets `shopItem = getIndex() + scrollOff` and then
/// `postButtonClick`, which does three things in order: `setSelectionHint`,
/// `tryMoveItems` and only then the packet. The first two are LOCAL — the
/// trade's items appear in the slots before the server answers — so a click
/// that the server later rejects still moved the screen first.
///
/// The scrollbar grab, like the stonecutter's, does **not** consume the press:
/// vanilla sets `isDragging` and falls through to `super.mouseClicked`.
/// The recipe book's press (M98).
///
/// **First of all**, and with a second rule under it:
///
/// ```java
/// if (this.recipeBookComponent.mouseClicked(...)) { … return true; }
/// else return this.widthTooNarrow && this.recipeBookComponent.isVisible()
///     ? true : super.mouseClicked(...);
/// ```
///
/// So a click the book does not want is still **swallowed** when the window is
/// too narrow and the book is open — the case where the book covers the menu.
/// Letting it fall through there would click a slot the player cannot see.
///
/// Returns whether the press was consumed.
pub(super) fn book_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    // M107 — `event.hasShiftDown()`, which vanilla passes straight through to
    // `useMaxItems`: shift-clicking a recipe places as many as the ingredients
    // allow rather than one.
    shift: bool,
    // M99 — the search's inputs, so the press resolves the SAME page the render
    // did: a search narrows the page, and hit-testing against an unfiltered one
    // would place a recipe the player is not looking at.
    display: &std::collections::HashMap<String, String>,
    right: bool,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::recipe_book_screen as rb;
    // `!minecraft.player.isSpectator()` — a spectator cannot click the book at
    // all, and the guard is on the whole method rather than on any one widget.
    if session.own_game_mode().is_some_and(|g| g.is_spectator()) {
        return false;
    }
    let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(w, h);
    let bx = ((screen.mouse.0 - bl as f64) / scale as f64).floor() as i32;
    let by = ((screen.mouse.1 - bt as f64) / scale as f64).floor() as i32;
    // Bound before the overlay branch, because both placement paths need it and
    // one `let` is how they cannot disagree.
    let narrow = rb::width_too_narrow((w / scale) as i32);
    // M104 — an OPEN which-of-these overlay eats every click, wherever it
    // lands. `RecipeBookPage.mouseClicked`'s overlay branch is an unconditional
    // `return true`, so while it is up the whole screen is modal: a click on
    // the page's arrows, on the search box, on the tabs, or on the menu's own
    // slots underneath all reach the overlay and nothing else.
    if let Some(open) = screen.book_overlay.as_ref() {
        match open.click_at(bx, by, right) {
            rewo_world::recipe_overlay::Click::Select(i) => {
                let picked = open.buttons.get(i).map(|b| (b.recipe, b.craftable));
                // **The overlay STAYS OPEN.** The selecting branch does not
                // call `setVisible(false)` — only the else does — so picking a
                // variant leaves the popup up and you can pick another. That
                // reads like an oversight and is what makes the feature usable.
                if let Some((id, craftable)) = picked {
                    place_from_book(session, screen, id, craftable, shift, narrow);
                }
            }
            rewo_world::recipe_overlay::Click::Close => screen.book_overlay = None,
        }
        return true;
    }
    let query = rewo_world::recipe_search::normalize(&screen.book_search.value());
    let Some(view) =
        live_recipe_book(session, items, screen.book, Some((bx, by)), &query, display)
            .and_then(|b| b.view)
    else {
        return false;
    };
    let hit = rb::book_hit(bx, by, view, view.tabs);
    let action = screen.book.press(hit, right);
    // The `EditBox` is the ONLY owner of "is the search focused" — see
    // `focus_change`'s docs for why the duplicate flag it replaced was a bug.
    if let Some(v) = rb::focus_change(hit) {
        screen.book_search.set_focused(v);
    }
    match action {
        Some(rb::BookAction::ToggleFilter) => {
            // `toggleFiltering()` then `sendUpdateSettings()` — the local flag
            // moves first and the packet reports it, which is why this is one
            // call rather than two.
            if let Err(e) = session.toggle_recipe_book_filter(shown_book_index(session))
            {
                log::warn!("rewo: recipe book filter: {e}");
            }
            true
        }
        Some(rb::BookAction::Recipe { index, right }) => {
            let render =
                live_recipe_book(session, items, screen.book, Some((bx, by)), &query, display);
            if right {
                // M104 — open the which-of-these overlay, but only on a cell
                // holding more than one recipe: `!button.isOnlyOption()`.
                //
                // `isOnlyOption` is `size() == 1`, not `size() > 1` negated —
                // an empty collection opens an (empty) overlay in vanilla too.
                let collection = render
                    .as_ref()
                    .and_then(|b| b.slot_collections.get(index))
                    .cloned()
                    .unwrap_or_default();
                if collection.len() != 1 {
                    screen.book_overlay = Some(open_overlay(collection, index, view));
                }
            } else {
                let picked = render.as_ref().and_then(|b| {
                    let id = (*b.slot_recipes.get(index)?)?;
                    // **`isCraftable(recipe)`, not `hasCraftable()`.** The
                    // cell's `(craftable, multiple)` pair carries the
                    // COLLECTION's answer, which is true when ANY of its
                    // recipes can be made — so on a group holding one craftable
                    // and one not, using it would let the uncraftable one be
                    // clicked forever. The per-recipe flag rides on the same
                    // `Button`s the which-of-these overlay is built from.
                    let craftable = b
                        .slot_collections
                        .get(index)?
                        .iter()
                        .find(|btn| btn.recipe == id)
                        .is_some_and(|btn| btn.craftable);
                    Some((id, craftable))
                });
                if let Some((id, craftable)) = picked {
                    place_from_book(session, screen, id, craftable, shift, narrow);
                }
            }
            true
        }
        Some(rb::BookAction::Navigated) => true,
        // Missed the book — swallowed anyway on a narrow window.
        None => narrow,
    }
}

/// `RecipeBookComponent.tryPlaceRecipe` and what its caller does with the
/// answer (M107).
///
/// Both placement paths — a page cell and a which-of-these button — funnel
/// through here, because vanilla has one `tryPlaceRecipe` and the guard is
/// stateful: two copies would each hold half the history and neither would
/// suppress correctly.
///
/// Three things happen on a successful placement, and only the packet is
/// obvious:
///
/// * **The ghost is cleared first.** `ghostSlots.clear()` runs before
///   `handlePlaceRecipe`, so a failed placement leaves the screen briefly with
///   no ghost until the server's reply refills it. Keeping the old one would
///   show the previous recipe's ingredients against the new request.
/// * **The book closes on a narrow window** — `if (!isOffsetNextToMainGUI())
///   setVisible(false)`, and `xOffset` is 0 exactly when the window is too
///   narrow. There the book covers the menu you are about to look at.
/// * `lastRecipe`/`lastRecipeCollection` are recorded for the Enter-key
///   re-place. **Rewo has no key path into the book yet**, so those are not
///   modelled; what matters here is that a SUPPRESSED click must not record
///   them either, and it does not, because it never gets past the guard.
///
/// Returns whether the click placed.
pub(super) fn place_from_book(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    recipe: i32,
    craftable: bool,
    use_max_items: bool,
    narrow: bool,
) -> bool {
    let effects = rewo_world::recipe_book_screen::place_effects(
        &mut screen.place_guard,
        recipe,
        craftable,
        use_max_items,
        narrow,
    );
    let Some(use_max_items) = effects.send else {
        return false;
    };
    if effects.clear_ghost {
        session.ghost_recipe = None;
    }
    if let Err(e) = session.place_recipe(recipe, use_max_items) {
        log::warn!("rewo: place_recipe: {e}");
    }
    if effects.close_book {
        // `setVisible(false)` — which also shuts the which-of-these overlay
        // (`recipeBookPage.setInvisible()`) and tells the server.
        screen.book_overlay = None;
        let book = shown_book_index(session);
        session.set_recipe_book_open(book, false);
        if let Err(e) = session.recipe_book_change_settings(book) {
            log::warn!("rewo: recipe book settings: {e}");
        }
    }
    true
}

/// Which of the four `RecipeBookSettings` slots the shown menu reads (M98).
pub(super) fn shown_book_index(session: &PlaySession) -> usize {
    book_type_of(session.shown_menu().layout()).map_or(0, |b| b.index())
}

pub(super) fn merchant_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::merchant_screen as ms;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ms::MERCHANT_MENU_PROTOCOL_ID {
        return false;
    }
    let n = session.merchant.as_ref().map_or(0, |m| m.offers.len());
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let scroll_off = screen.merchant.map_or(0, |l| l.scroll_off);
    // The grab is tested FIRST in vanilla and does not return; the button is
    // an ordinary widget press, which happens inside `super`.
    if ms::can_scroll(n) && ms::scroller_grabbed(gx, gy) {
        if let Some(l) = screen.merchant.as_mut() {
            l.dragging = true;
        }
    }
    let Some(button) = ms::button_at(gx, gy) else {
        return false;
    };
    let offer = ms::offer_for_button(button, scroll_off);
    // A button past the end of the list is drawn but dead — vanilla hides the
    // widget (`visible = false`) rather than disabling it, so there is nothing
    // there to press.
    if offer as usize >= n {
        return false;
    }
    if let Some(l) = screen.merchant.as_mut() {
        // `postButtonClick` sets `shopItem` LOCALLY first — the trade's items
        // appear before the server answers.
        l.selected = offer;
    }
    if let Err(e) = session.select_trade(offer) {
        log::warn!("select_trade {offer}: {e}");
    }
    true
}

/// What the merchant's trade list needs, resolved by the caller (M93u).
#[derive(Debug, Clone)]
pub struct MerchantView {
    /// The offers as sent, in the order the click's index addresses.
    pub offers: Vec<rewo_net::merchant::MerchantOffer>,
    /// `MerchantScreen.scrollOff` — an **offer index**, not a fraction.
    pub scroll_off: i32,
    /// Each offer's modified cost-A count, resolved here because the clamp's
    /// ceiling is the item's own max stack size and only this side holds the
    /// item table.
    pub cost_a_counts: Vec<i32>,
    /// `MerchantScreen.shopItem` — the selected offer, whose out-of-stock X
    /// is drawn in the right-hand panel. Screen-local: the packet does not
    /// carry a selection.
    pub selected: i32,
    /// `getTraderLevel` / `getTraderXp`, straight off the packet.
    pub level: i32,
    pub xp: i32,
    /// `showProgressBar()` — false for a wandering trader, which has no level
    /// and no bar.
    pub show_progress: bool,
    /// `getFutureTraderXp` — the xp the currently-matched offer would grant.
    ///
    /// **Derived here rather than received**: no packet carries it.
    /// `MerchantContainer.updateSellItem` matches the payment slots against
    /// the offers and takes the matched offer's xp, and the client holds both
    /// halves. See `merchant_future_xp` for the one case it declines.
    pub future_xp: i32,
}

/// `MerchantContainer.updateSellItem`'s `futureXp` (M93v).
///
/// Vanilla derives it: the payment slots are matched against the offers by
/// `getRecipeFor`, and the matched offer's xp is what the bar's result segment
/// shows. Both halves are on the client, so this is a derivation and not a
/// gap — the same shape as M93u's four class-C corrections one level down.
///
/// **The one case it declines.** `ItemCost.test` is `stack.is(item) &&
/// components.test(stack)`, and the second half is a
/// `DataComponentExactPredicate` — per-component *values*, where M41 gives
/// Rewo a digest of the whole patch. So a **constrained** cost cannot be
/// evaluated, and an offer carrying one is treated as unmatched. The
/// consequence is narrow and one-directional: the result segment is missing
/// where vanilla would show it, never present where vanilla would not. Vanilla
/// villager trades are plain items, so in practice this is the enchanted-book
/// and dyed-armour tail.
///
/// The slot order is `MerchantContainer`'s own: **if slot 0 is empty, slot 1
/// becomes `buyA` and `buyB` is empty** — so paying with only the second slot
/// still matches a one-item trade.
pub(super) fn merchant_future_xp(
    m: &rewo_world::menu::OpenMenu,
    offers: &rewo_net::merchant::MerchantOffers,
    selected: i32,
    props: &dyn Fn(i32) -> i32,
) -> i32 {
    let slot = |i: usize| m.menu.menu_slot(i).map(|s| (s.item_id, s.count));
    let matches: Vec<rewo_world::merchant_screen::OfferMatch> = offers
        .offers
        .iter()
        .map(|o| rewo_world::merchant_screen::OfferMatch {
            cost_a_item: o.cost_a.item_id,
            need_a: o.modified_cost_a(props(o.cost_a.item_id)),
            cost_b: o.cost_b.as_ref().map(|c| (c.item_id, c.count)),
            constrained: o.cost_a.constrained
                || o.cost_b.as_ref().is_some_and(|c| c.constrained),
        })
        .collect();
    let satisfied = rewo_world::merchant_screen::satisfied_offers(&matches, slot(0), slot(1));
    rewo_world::merchant_screen::recipe_for(selected, &satisfied)
        .map_or(0, |i| offers.offers[i].xp)
}

/// The merchant screen's scroll, which no packet carries (M93u).
#[derive(Debug, Clone, Copy, Default)]
pub struct MerchantLocal {
    pub(super) container_id: i32,
    pub scroll_off: i32,
    pub selected: i32,
    pub dragging: bool,
}

/// Resolve the view, seeding the scroll on a new container.
pub(crate) fn merchant_view(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    offers: &rewo_net::merchant::MerchantOffers,
    props: &dyn Fn(i32) -> i32,
) -> MerchantView {
    let stale = screen.merchant.is_none_or(|l| l.container_id != m.container_id);
    if stale {
        screen.merchant = Some(MerchantLocal {
            container_id: m.container_id,
            scroll_off: 0,
            selected: 0,
            dragging: false,
        });
    }
    let local = screen.merchant.expect("just seeded");
    MerchantView {
        level: offers.villager_level,
        xp: offers.villager_xp,
        show_progress: offers.show_progress,
        future_xp: merchant_future_xp(m, offers, local.selected, props),
        cost_a_counts: offers
            .offers
            .iter()
            .map(|o| o.modified_cost_a(props(o.cost_a.item_id)))
            .collect(),
        offers: offers.offers.clone(),
        // Clamped on read rather than on write: the list can SHRINK under a
        // held scroll when the villager restocks, and vanilla's own guard is
        // `offer_visible`'s `!canScroll` short-circuit rather than a stored
        // clamp.
        selected: local.selected,
        scroll_off: local
            .scroll_off
            .min(rewo_world::merchant_screen::max_scroll_off(offers.offers.len()).max(0)),
    }
}

/// `minecraft:menu`'s `anvil` id.
pub const ANVIL_MENU_PROTOCOL_ID: i32 = 8;

/// The anvil screen's name field and the name it has sent (M93t).
#[derive(Debug, Clone, Default)]
pub struct AnvilLocal {
    pub(super) container_id: i32,
    /// Slot 0 as last seen, so `slotChanged` fires exactly when vanilla's does.
    pub(super) slot0: Option<(i32, u64)>,
    pub field: rewo_world::edit_box::EditBox,
    pub name: rewo_world::anvil::AnvilName,
}

/// `AnvilScreen.subInit` + `slotChanged` — seed the field, and re-seed it
/// whenever slot 0 changes (M93t).
///
/// ```java
/// public void slotChanged(container, slotIndex, itemStack) {
///    if (slotIndex == 0) {
///       this.name.setValue(itemStack.isEmpty() ? "" : itemStack.getHoverName().getString());
///       this.name.setEditable(!itemStack.isEmpty());
///       this.setFocused(this.name);
///    }
/// }
/// ```
///
/// So an **empty** input slot leaves an empty, UNEDITABLE field — and that
/// matters for far more than the text, because `AnvilScreen.keyPressed` falls
/// through to `super` only when the box can consume nothing. See `anvil_key`.
pub(super) fn anvil_local<'a>(
    screen: &'a mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    items: &rewo_data::items::Items,
) -> &'a mut AnvilLocal {
    let slot0 = m.menu.menu_slot(0).map(|s| (s.item_id, s.components));
    let stale = match &screen.anvil {
        Some(a) => a.container_id != m.container_id || a.slot0 != slot0,
        None => true,
    };
    if stale {
        let mut field = anvil_field_new();
        let hover = slot0
            .and_then(|(id, _)| items.name(id))
            .map(display_name_of)
            .unwrap_or_default();
        field.set_value(&hover);
        field.set_editable(slot0.is_some());
        let mut local = AnvilLocal {
            container_id: m.container_id,
            slot0,
            field,
            name: screen
                .anvil
                .as_ref()
                .filter(|a| a.container_id == m.container_id)
                .map(|a| a.name.clone())
                .unwrap_or_default(),
        };
        // The seeding `setValue` fires the responder, as vanilla's does; drain
        // it so the re-seed does not send a rename of the name we were just
        // told.
        let _ = local.field.take_value_changed();
        screen.anvil = Some(local);
    }
    screen.anvil.as_mut().expect("just seeded")
}

/// A stack's `getHoverName().getString()` for the seed — the item's display
/// name, which for an un-renamed stack is its translated name.
///
/// **An approximation, and a recorded one**: Rewo resolves the display name
/// from the item registry, so a stack carrying a `custom_name` component seeds
/// the field with its *default* name instead. M41 decodes the component's
/// bytes but not its text, which is the same wall the tooltip's name override
/// hits. The consequence is narrow — the field starts on the wrong string for
/// an already-renamed item — and it does not reach the wire, because
/// `on_name_changed` compares against what the server was told.
pub(super) fn display_name_of(id: &str) -> String {
    id.rsplit(':')
        .next()
        .unwrap_or(id)
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the stonecutter's recipe grid needs, resolved by the caller (M93s).
///
/// Carried rather than derived in the overlay builder for `LoomView`'s reason:
/// the list keys off the input slot's item NAME, and only this side holds the
/// registry that turns an id into one.
#[derive(Debug, Clone)]
pub struct CutView {
    /// `getVisibleRecipes()` — `selectByInput` of the input slot, **in master
    /// order**, because the index a click sends indexes this.
    pub recipes: Vec<&'static rewo_data::stonecutter_table::Cut>,
    /// `StonecutterScreen.startIndex` — the first visible recipe. A multiple
    /// of 4, since the grid scrolls by whole rows.
    pub start_index: i32,
    /// `getSelectedRecipeIndex()`, from data slot 0.
    pub selected: i32,
    /// `scrollOffs`, for the thumb's position.
    pub scroll_offs: f32,
    /// `displayRecipes` — the grid, the icons and the scrollbar are all hidden
    /// when false.
    pub display: bool,
}

/// The stonecutter screen's scroll, which is **screen-local**: no packet
/// carries it, and vanilla resets it in `containerChanged` (M93s).
#[derive(Debug, Clone, Copy)]
pub struct CutLocal {
    pub(super) container_id: i32,
    /// The input slot as last seen. `containerChanged` is registered as the
    /// menu's update listener and fires on **any** change to the input
    /// container — so taking one block off a stack resets the scroll even
    /// though `slotsChanged` rebuilds the recipe list only when the item TYPE
    /// changes. Two granularities on one event, and this is the screen's.
    pub(super) input: Option<(i32, i32)>,
    pub(super) scroll_offs: f32,
    /// Whether the thumb is being dragged. Cleared on any release.
    pub scrolling: bool,
}

/// `StonecutterScreen.mouseClicked` (M93s).
///
/// ```java
/// if (this.displayRecipes) {
///    for (int index = this.startIndex; index < endIndex; index++) {
///       … if (hit && this.menu.clickMenuButton(player, index)) {
///          play(UI_STONECUTTER_SELECT_RECIPE);
///          this.minecraft.gameMode.handleInventoryButtonClick(this.menu.containerId, index);
///          return true;
///       }
///    }
///    if (over the scrollbar) this.scrolling = true;   // and does NOT return
/// }
/// return super.mouseClicked(event, doubleClick);
/// ```
///
/// So a recipe consumes the press and a scrollbar grab does not — the grab
/// falls through to the slot logic, which finds nothing under the bar.
/// Returns whether the press was consumed.
pub(super) fn cut_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::menu_screen as ms;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ms::STONECUTTER_MENU_PROTOCOL_ID {
        return false;
    }
    let view = cut_view(screen, open, items);
    if !view.display {
        // The whole block is inside `if (this.displayRecipes)`, so with no
        // grid there is no recipe click AND no scrollbar grab.
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    if let Some(index) = ms::cut_cell_click_at(gx, gy, view.start_index) {
        // `isValidRecipeIndex` is the server's gate and the screen's: vanilla
        // calls `clickMenuButton` inside the hit test, so an out-of-range cell
        // does not consume the press either.
        if ms::cut_click_accepted(index, view.recipes.len()) {
            if let Err(e) = session.container_button_click(index) {
                log::warn!("stonecutter recipe {index}: {e}");
            }
            return true;
        }
    }
    if ms::cut_scroller_grabbed(gx, gy) {
        if let Some(c) = screen.cut.as_mut() {
            c.scrolling = true;
        }
    }
    false
}

/// The whole stonecutter view, resolved from the menu plus the screen's own
/// scroll — the caller's job, exactly as `beacon_live` is (M93s).
pub(crate) fn cut_view(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    items: &rewo_data::items::Items,
) -> CutView {
    // Slots: input 0, result 1, then the player's 36.
    let name = m.menu.menu_slot(0).and_then(|s| items.name(s.item_id));
    let recipes = name.map_or_else(Vec::new, rewo_data::stonecutter_table::select_by_input);
    let local = cut_local(screen, m);
    let display = rewo_world::menu_screen::cut_display_recipes(name.is_some(), recipes.len());
    CutView {
        start_index: if display {
            rewo_world::menu_screen::cut_start_index(local.scroll_offs, recipes.len())
        } else {
            0
        },
        selected: m.data(0) as i32,
        scroll_offs: local.scroll_offs,
        display,
        recipes,
    }
}

/// `StonecutterScreen`'s scroll, seeded at 0 and owned by the screen until the
/// input changes — the beacon's shape (M93m), with a different reset trigger.
pub(super) fn cut_local(screen: &mut ScreenState, m: &rewo_world::menu::OpenMenu) -> CutLocal {
    let input = m.menu.menu_slot(0).map(|s| (s.item_id, s.count));
    let stale = match screen.cut {
        Some(c) => c.container_id != m.container_id || c.input != input,
        None => true,
    };
    if stale {
        screen.cut = Some(CutLocal {
            container_id: m.container_id,
            input,
            scroll_offs: 0.0,
            scrolling: false,
        });
    }
    screen.cut.expect("just seeded")
}

/// The beacon screen's local choice and the watermarks it was seeded at.
#[derive(Debug, Clone, Copy)]
pub struct BeaconLocal {
    pub(super) container_id: i32,
    pub(super) data_writes: u64,
    pub(super) choice: rewo_world::menu_screen::BeaconChoice,
}

/// `BeaconScreen`'s live choice — seeded from the menu, then owned by the
/// screen until the menu says otherwise (M93m).
pub(super) fn beacon_live(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    effect_ids: &BeaconEffectIds,
) -> rewo_world::menu_screen::BeaconChoice {
    let stale = match screen.beacon {
        Some(b) => b.container_id != m.container_id || b.data_writes != m.data_writes,
        None => true,
    };
    if stale {
        screen.beacon = Some(BeaconLocal {
            container_id: m.container_id,
            data_writes: m.data_writes,
            choice: beacon_choice(m, effect_ids),
        });
    }
    // `levels` and `has_payment` are the MENU's on every frame, not the
    // screen's: `updateStatus(levels)` is passed the menu's value each time
    // and `hasPayment()` reads the slot directly. Only the two effects are
    // screen-owned, so a payment arriving mid-selection lights Confirm
    // without disturbing the pick.
    let mut c = screen.beacon.expect("seeded above").choice;
    c.levels = m.beacon_levels();
    c.has_payment = m.beacon_has_payment();
    c
}

pub(super) fn beacon_choice(
    m: &rewo_world::menu::OpenMenu,
    effect_ids: &BeaconEffectIds,
) -> rewo_world::menu_screen::BeaconChoice {
    rewo_world::menu_screen::BeaconChoice {
        levels: m.beacon_levels(),
        primary: m.beacon_primary().and_then(|id| effect_ids.of(id)),
        secondary: m.beacon_secondary().and_then(|id| effect_ids.of(id)),
        has_payment: m.beacon_has_payment(),
    }
}

/// The six beacon effects' `minecraft:mob_effect` registry ids.
///
/// From `rewo_data`'s report-backed table (M92c) — by NAME, never by position.
/// An unresolvable name leaves that slot `None` and the effect simply does not
/// match, which shows as a button with no icon rather than the *wrong* icon.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BeaconEffectIds(pub(super) [Option<i32>; 6]);

impl BeaconEffectIds {
    pub(crate) fn resolve(m: &rewo_data::mob_effects::MobEffects) -> Self {
        use rewo_world::menu_screen::BeaconEffect;
        Self(std::array::from_fn(|i| m.id_of(BeaconEffect::ALL[i].name())))
    }

    /// The registry id of one of the six, for `set_beacon` (M93m).
    pub(super) fn id_of(&self, e: rewo_world::menu_screen::BeaconEffect) -> Option<i32> {
        let i = rewo_world::menu_screen::BeaconEffect::ALL
            .iter()
            .position(|x| *x == e)?;
        self.0[i]
    }

    /// Which of the six a registry id is, or `None` for any other effect.
    pub(super) fn of(&self, id: i32) -> Option<rewo_world::menu_screen::BeaconEffect> {
        self.0
            .iter()
            .position(|e| *e == Some(id))
            .map(|i| rewo_world::menu_screen::BeaconEffect::ALL[i])
    }
}

/// What an enchanting-table row's state selects, as `(row sprite, numeral)`.
///
/// Kept beside the overlay builder rather than in `rewo-world` because the
/// indices are `rewo-data`'s and the state is `rewo-world`'s; this is the one
/// place both are in scope.
/// **The numeral goes through `EnchantRow::numeral()`, not a second match.**
/// This function had its own copy of that mapping, and a mutation found the
/// duplication: emptying `numeral()` changed nothing rendered, because nothing
/// rendered was reading it. That is M18's finding and M45's in one — a second
/// derivation of the same fact is a second chance to disagree, and here it was
/// also a witness grading a function the app did not call.
pub(super) fn enchant_row_sprites(
    i: usize,
    row: rewo_world::menu_screen::EnchantRow,
) -> (usize, Option<usize>) {
    use rewo_data::assets as a;
    use rewo_world::menu_screen::EnchantRow;
    let background = match row {
        // `cost == 0` blits the disabled background and RETURNS; an
        // unaffordable offer blits the SAME one and then its numeral.
        EnchantRow::Empty | EnchantRow::Unaffordable { .. } => a::ENCHANT_ROW_DISABLED,
        EnchantRow::Available { .. } => a::ENCHANT_ROW,
        EnchantRow::Hovered { .. } => a::ENCHANT_ROW_HIGHLIGHTED,
    };
    let numeral = row
        .numeral()
        .map(|greyed| if greyed { a::ENCHANT_LEVEL_DISABLED } else { a::ENCHANT_LEVEL } + i);
    (background, numeral)
}

/// Everything an open menu paints over its background sheet, in draw order
/// (M91 the furnaces, M92 the rest).
///
/// Empty for a menu with no overlays *and* for one that is not open — a
/// `containershot` panel built with no `OpenMenu` has no data slots to read,
/// and inventing a plausible-looking half-lit furnace would make the gate's
/// panel witnesses grade a state no server ever sent.
///
/// `player` carries the two inputs the enchanting table needs that are not the
/// menu's at all — the local XP level and the creative flag.
pub(super) fn menu_overlays(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
) -> Vec<(usize, rewo_gpu::container::PanelBlit)> {
    use rewo_data::assets as a;
    let mut out = Vec::new();
    let Some(m) = open else { return out };
    match layout.protocol_id {
        // The furnace family (M91): flame then arrow.
        id if a::progress_index(id).is_some() => {
            let base = a::progress_index(id).expect("guarded above");
            let (flame, arrow) = rewo_world::menu_screen::furnace_progress(
                m.furnace_is_lit(),
                m.furnace_lit_progress(),
                m.furnace_burn_progress(),
            );
            // The lit sprite is the pair's first, the burn its second.
            if let Some(f) = flame {
                out.push((base, to_blit(f)));
            }
            out.push((base + 1, to_blit(arrow)));
        }
        // loom (M93q): for each visible cell, the button chrome then the
        // banner preview — a grey fill under an untinted pattern.
        rewo_world::menu_screen::LOOM_MENU_PROTOCOL_ID => {
            let Some(l) = player.loom.filter(|l| l.display) else {
                return out;
            };
            for row in 0..rewo_world::menu_screen::LOOM_ROWS {
                for col in 0..rewo_world::menu_screen::LOOM_COLS {
                    let index = (row + l.start_row) * rewo_world::menu_screen::LOOM_COLS + col;
                    // `break label82` — the grid stops at the end of the list
                    // rather than drawing empty cells.
                    let Some(pattern) = usize::try_from(index)
                        .ok()
                        .and_then(|i| l.patterns.get(i))
                    else {
                        break;
                    };
                    let (cx, cy) = rewo_world::menu_screen::loom_cell_origin(row, col);
                    let cell = rewo_world::menu_screen::ProgressBlit {
                        dx: cx,
                        dy: cy,
                        w: rewo_world::menu_screen::LOOM_CELL,
                        h: rewo_world::menu_screen::LOOM_CELL,
                        sx: 0,
                        sy: 0,
                        src: None,
                    };
                    let hovered = mouse_gui.is_some_and(|(x, y)| {
                        rewo_world::menu_screen::loom_cell_at(x, y, l.start_row) == Some(index)
                    });
                    out.push((
                        a::LOOM_PATTERN_CHROME
                            + if index == l.selected {
                                0
                            } else if hovered {
                                1
                            } else {
                                2
                            },
                        to_blit(cell),
                    ));
                    // The grey backing FIRST, then the pattern over it. The
                    // order is the whole reason fills share the sprites' list.
                    out.push(to_fill(
                        rewo_world::menu_screen::loom_preview_backing(cx, cy),
                        rewo_world::menu_screen::LOOM_PREVIEW_BACKING,
                    ));
                    if let Some(sprite) = a::banner_pattern_overlay(pattern) {
                        out.push((
                            sprite,
                            to_blit(rewo_world::menu_screen::loom_pattern_preview(cx, cy)),
                        ));
                    }
                }
            }
        }
        // merchant (M93u): the scroller, then per visible offer a trade arrow.
        // The three ITEMS per row are icons and go through the GUI-item pass.
        rewo_world::merchant_screen::MERCHANT_MENU_PROTOCOL_ID => {
            use rewo_world::merchant_screen as ms;
            let Some(v) = player.merchant else { return out };
            let n = v.offers.len();
            if let Some(y) = ms::scroller_y(v.scroll_off, n) {
                out.push((
                    a::VILLAGER_SCROLLER + usize::from(!ms::can_scroll(n)),
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::SCROLL_X,
                        dy: y,
                        w: ms::SCROLLER_W,
                        h: ms::SCROLLER_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The trade buttons FIRST (M93x) — `addRenderableWidget` puts them
            // in the widget layer, which `extractBackground` has already run
            // under. Drawn after the arrow they would cover it.
            //
            // A row past the end of the list draws nothing: vanilla toggles
            // `visible`, not `active`, so there is no greyed button.
            for (i, _) in v.offers.iter().enumerate() {
                let i = i as i32;
                if !ms::offer_visible(i, v.scroll_off, n) {
                    continue;
                }
                let row = if ms::can_scroll(n) { i - v.scroll_off } else { i };
                let hovered = mouse_gui.is_some_and(|(x, y)| ms::button_hovered(row, x, y));
                for sl in ms::button_slices(ms::TRADE_BUTTON_W) {
                    if sl.w == 0 {
                        continue;
                    }
                    out.push((
                        a::WIDGET_BUTTON + usize::from(hovered),
                        to_blit(rewo_world::menu_screen::ProgressBlit {
                            dx: ms::TRADE_BUTTON_X + sl.dx,
                            dy: ms::button_y(row),
                            w: sl.w,
                            h: ms::TRADE_BUTTON_H,
                            sx: sl.sx,
                            sy: 0,
                            // 1:1 — the source size equals the destination, so
                            // this is a tile and not a scale.
                            src: None,
                        }),
                    ));
                }
            }
            for (i, offer) in v.offers.iter().enumerate() {
                let i = i as i32;
                if !ms::offer_visible(i, v.scroll_off, n) {
                    continue;
                }
                // The row is the offer's position in the WINDOW, which is the
                // offer index only when nothing is scrolled.
                let row = if ms::can_scroll(n) { i - v.scroll_off } else { i };
                let y = ms::row_item_y(row);
                // `xo + 5 + 35 + 20` — past cost B, not past cost A. The first
                // cut of this read `5 + 5 + 20` and put every arrow 30 px left,
                // on top of the cost-A icon.
                out.push((
                    a::VILLAGER_TRADE_ARROW + usize::from(offer.out_of_stock),
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::COST_B_X + 20,
                        dy: y + 3,
                        w: 10,
                        h: 9,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The discount strikethrough (M93w), through the FIRST number.
            for (i, offer) in v.offers.iter().enumerate() {
                let idx = i as i32;
                if !ms::offer_visible(idx, v.scroll_off, n) {
                    continue;
                }
                if !ms::cost_a_display(offer.cost_a.count, v.cost_a_counts[i]).strikethrough {
                    continue;
                }
                let row = if ms::can_scroll(n) { idx - v.scroll_off } else { idx };
                out.push((
                    a::VILLAGER_STRIKETHROUGH,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::COST_A_X + ms::STRIKETHROUGH_DX,
                        dy: ms::row_item_y(row) + ms::STRIKETHROUGH_DY,
                        w: ms::STRIKETHROUGH_W,
                        h: ms::STRIKETHROUGH_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The XP bar (M93v): background, then the fill, then the result
            // segment — which samples the sprite from `w` rather than 0, so it
            // CONTINUES the gradient where the fill stopped.
            //
            // `showProgressBar()` gates it: a wandering trader has no level.
            if v.show_progress {
                if let Some((fill, future)) = ms::xp_bar(v.level, v.xp, v.future_xp) {
                    let bar = |dx: i32, sx: i32, w: i32| rewo_world::menu_screen::ProgressBlit {
                        dx,
                        dy: ms::XP_BAR_Y,
                        w,
                        h: ms::XP_BAR_H,
                        sx,
                        sy: 0,
                        // `None` — the source size EQUALS the destination, so
                        // this is a 1:1 slice at `sx` and not a scale.
                        //
                        // `blitSprite(sprite, 102, 5, u, v, x, y, w, h)` passes
                        // the SHEET's size, not the source rect's: the rect is
                        // `w x h` at `(u, v)`. Setting `src` to (102, 5) — the
                        // obvious reading of those two arguments — squeezes the
                        // whole bar into the segment, which a mutation caught
                        // by NOT dying: the `sx` offset had no visible effect
                        // because every segment was showing the entire sprite.
                        src: None,
                    };
                    out.push((
                        a::VILLAGER_XP_BAR,
                        to_blit(bar(ms::XP_BAR_X, 0, ms::XP_BAR_W)),
                    ));
                    if fill > 0 {
                        out.push((a::VILLAGER_XP_BAR + 1, to_blit(bar(ms::XP_BAR_X, 0, fill))));
                    }
                    if future > 0 {
                        out.push((
                            a::VILLAGER_XP_BAR + 2,
                            to_blit(bar(ms::XP_BAR_X + fill, fill, future)),
                        ));
                    }
                }
            }
            // The 28x21 red X is NOT per row — `extractButtonArrows` only
            // swaps the arrow. It belongs to the SELECTED offer, in the
            // right-hand trading panel at `leftPos + 83 + 99`.
            if let Some(sel) = usize::try_from(v.selected)
                .ok()
                .and_then(|i| v.offers.get(i))
                .filter(|o| o.out_of_stock)
            {
                let _ = sel;
                out.push((
                    a::VILLAGER_OUT_OF_STOCK,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: 182,
                        dy: 35,
                        w: 28,
                        h: 21,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // anvil (M93t): the name field's cursor and selection, measured by
        // `anvil_field_render` alongside the text so the two cannot disagree
        // about where the run ends.
        ANVIL_MENU_PROTOCOL_ID => {
            // `extractBackground` blits the field's own 110x16 background at
            // (59, 20) — over a RED placeholder baked into `anvil.png`, so
            // this is chrome the screen cannot omit. The pair is chosen by the
            // same slot-0 predicate that makes the field editable.
            let has_input = m.menu.menu_slot(0).is_some();
            out.push((
                a::ANVIL_TEXT_FIELD + usize::from(!has_input),
                to_blit(rewo_world::menu_screen::ProgressBlit {
                    dx: 59,
                    dy: 20,
                    w: 110,
                    h: 16,
                    sx: 0,
                    sy: 0,
                    src: None,
                }),
            ));
            // Then the field's own cursor and selection, over it.
            out.extend_from_slice(player.anvil_fills);
            // `extractErrorIcon` — an input present and NO result, which is
            // the combination the anvil refused.
            if (has_input || m.menu.menu_slot(1).is_some()) && m.menu.menu_slot(2).is_none() {
                out.push((
                    a::ANVIL_ERROR,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: 99,
                        dy: 45,
                        w: 28,
                        h: 21,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // stonecutter (M93s): the scroller, then one button chrome per visible
        // cell. The result ICONS are not here — they are items, and go through
        // the GUI-item pass with the slots (see `screen_icons`).
        rewo_world::menu_screen::STONECUTTER_MENU_PROTOCOL_ID => {
            use rewo_world::menu_screen as ms;
            let Some(c) = player.cut.filter(|c| c.display) else {
                // `displayRecipes` hides the grid AND the scrollbar: the
                // scroller is drawn inside `extractBackground` unconditionally,
                // but `isScrollBarActive` is false, so it is the *disabled*
                // sprite that shows — and vanilla draws it even with no input.
                out.push((
                    a::CUT_SCROLLER + 1,
                    to_blit(cut_scroller_blit(ms::cut_scroller_y(0.0))),
                ));
                return out;
            };
            let active = ms::cut_scroll_active(true, c.recipes.len());
            out.push((
                a::CUT_SCROLLER + usize::from(!active),
                to_blit(cut_scroller_blit(ms::cut_scroller_y(c.scroll_offs))),
            ));
            for pos_index in 0..ms::CUT_PAGE {
                let index = c.start_index + pos_index;
                if index as usize >= c.recipes.len() {
                    break;
                }
                // `extractButtons`' three-way test, in its own order: selected
                // wins over hovered, and the hover box is the ICON's, two
                // pixels below the box a click uses.
                let hovered = mouse_gui.is_some_and(|(x, y)| {
                    ms::cut_cell_highlight_at(x, y, c.start_index, c.recipes.len()) == Some(index)
                });
                let chrome = if index == c.selected {
                    0
                } else if hovered {
                    1
                } else {
                    2
                };
                let (sx, sy) = ms::cut_cell_sprite_origin(pos_index);
                out.push((
                    a::CUT_RECIPE_CHROME + chrome,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: sx,
                        dy: sy,
                        w: ms::CUT_CELL_W,
                        h: ms::CUT_CELL_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // crafter_3x3 (M93j): the redstone arrow, then one cover per disabled
        // grid slot. Order matters only in that the covers must not be hidden
        // by the arrow, and they do not overlap it.
        rewo_world::menu::CRAFTER_MENU_PROTOCOL_ID => {
            out.push((
                a::CRAFTER_REDSTONE + usize::from(m.crafter_powered()),
                to_blit(rewo_world::menu_screen::crafter_redstone()),
            ));
            for slot in 0..rewo_world::menu::CRAFTER_GRID_SLOTS {
                if !m.crafter_slot_disabled(slot) {
                    continue;
                }
                // The slot's position comes from the LAYOUT, not re-derived
                // from `26 + x * 18`: a second copy of the grid arithmetic is
                // the drift M90's `slot_kind` bug was made of.
                let Some((sx, sy)) = layout.position(slot as usize) else {
                    continue;
                };
                out.push((
                    a::CRAFTER_DISABLED_SLOT,
                    to_blit(rewo_world::menu_screen::crafter_disabled_cover(
                        sx as i32, sy as i32,
                    )),
                ));
            }
        }
        // brewing_stand (M92): fuel, arrow, bubbles — vanilla's own order.
        11 => {
            let (fuel, brew, bubbles) =
                rewo_world::menu_screen::brewing_progress(m.brewing_fuel(), m.brewing_ticks());
            for (sprite, blit) in [
                (a::BREW_FUEL, fuel),
                (a::BREW_PROGRESS, brew),
                (a::BREW_BUBBLES, bubbles),
            ] {
                if let Some(b) = blit {
                    out.push((sprite, to_blit(b)));
                }
            }
        }
        // enchantment (M92): each row's background, then its numeral ON TOP of
        // it — which is why `overlays` is an ordered list and not a set.
        13 => {
            let rows = enchant_rows_of(layout, Some(m), player, mouse_gui)
                .expect("id 13 with an open menu");
            for (i, row) in rows.into_iter().enumerate() {
                let (bg, numeral) = enchant_row_sprites(i, row);
                out.push((bg, to_blit(rewo_world::menu_screen::enchant_row_rect(i))));
                if let Some(n) = numeral {
                    out.push((n, to_blit(rewo_world::menu_screen::enchant_level_rect(i))));
                }
            }
        }
        // beacon (M92): each button's 22x22 chrome, then its 18x18 icon.
        9 => {
            let choice = player
                .beacon_override
                .unwrap_or_else(|| beacon_choice(m, &player.beacon_effects));
            for b in rewo_world::menu_screen::beacon_buttons() {
                let hovered = mouse_gui
                    .is_some_and(|(x, y)| rewo_world::menu_screen::beacon_button_hovered(b, x, y));
                let state = rewo_world::menu_screen::beacon_button_state(b, choice, hovered);
                if state == rewo_world::menu_screen::BeaconButtonState::Hidden {
                    continue;
                }
                out.push((
                    a::BEACON_BUTTON_CHROME + beacon_chrome_index(state),
                    to_blit(rewo_world::menu_screen::beacon_button_rect(b)),
                ));
                if let Some(icon) = beacon_icon_sprite(b, choice) {
                    out.push((icon, to_blit(rewo_world::menu_screen::beacon_icon_rect(b))));
                }
            }
        }
        _ => {}
    }
    out
}

/// A button state's offset into the four chrome sprites, which are listed in
/// the order `extractContents` tests for them.
pub(super) fn beacon_chrome_index(s: rewo_world::menu_screen::BeaconButtonState) -> usize {
    use rewo_world::menu_screen::BeaconButtonState as S;
    match s {
        S::Disabled => 0,
        S::Selected => 1,
        S::Highlighted => 2,
        S::Normal => 3,
        // Never reached: a hidden button is skipped before it gets here.
        S::Hidden => 3,
    }
}

/// The 18x18 icon a beacon button draws inside its chrome.
///
/// The upgrade button **borrows the primary's effect**, so its icon changes as
/// you click elsewhere — it is the one button whose art is not a constant.
pub(super) fn beacon_icon_sprite(
    b: rewo_world::menu_screen::BeaconButton,
    choice: rewo_world::menu_screen::BeaconChoice,
) -> Option<usize> {
    use rewo_data::assets as a;
    use rewo_world::menu_screen::{beacon_upgrade_effect, BeaconButtonKind, BeaconEffect};
    let effect = match b.kind {
        BeaconButtonKind::Power { effect, .. } => effect,
        BeaconButtonKind::Upgrade => beacon_upgrade_effect(choice)?,
        BeaconButtonKind::Confirm => return Some(a::BEACON_CONFIRM),
        BeaconButtonKind::Cancel => return Some(a::BEACON_CANCEL),
    };
    let i = BeaconEffect::ALL.iter().position(|e| *e == effect)?;
    Some(a::BEACON_EFFECT_ICON + i)
}

/// The two enchanting-table inputs that are the *player's* rather than the
/// menu's (M92).
///
/// A named pair rather than two loose parameters because they are only ever
/// read together and swapping a level for a flag would compile.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EnchantPlayer<'a> {
    /// `player.experienceLevel`, from `set_experience` (M79).
    pub xp_level: i32,
    /// `player.hasInfiniteMaterials()` — `abilities.instabuild` (M75).
    pub creative: bool,
    /// The beacon's six effect ids (M92c). Carried here rather than passed
    /// separately because both screens' extra inputs travel the same seam.
    pub beacon_effects: BeaconEffectIds,
    /// The loom's pattern grid (M93q), when a loom is open.
    pub loom: Option<LoomView>,
    /// The stonecutter's recipe grid (M93s), when a stonecutter is open.
    ///
    /// A reference, not a value: the visible list is a `Vec` built per frame
    /// from `selectByInput`, and this struct is `Copy` because the enchanting
    /// rows and the panel each take it by value.
    pub cut: Option<&'a CutView>,
    /// The anvil field's cursor and selection quads (M93t), already measured.
    pub anvil_fills: &'a [(usize, rewo_gpu::container::PanelBlit)],
    /// The merchant's trade list (M93u), when a merchant is open.
    pub merchant: Option<&'a MerchantView>,
    /// The beacon SCREEN's own choice (M93m), when a screen is driving.
    ///
    /// `None` re-reads the menu's data slots, which is right for a gate with
    /// no screen state — and wrong for the live client, where a click moves
    /// the choice and only `set_beacon` on confirm tells the server. Without
    /// this the render would keep painting the server's last word and a click
    /// would light nothing, which is M93i's "correct on the wire, invisible on
    /// screen" one screen over.
    pub beacon_override: Option<rewo_world::menu_screen::BeaconChoice>,
    /// M103 — the ghost recipe's two washes, already measured. Resolved by the
    /// caller for the reason the beacon's choice and the anvil's fills are: the
    /// slot positions need the layout, and `apply_screen` holds no ScreenState.
    pub ghost_under: &'a [(usize, rewo_gpu::container::PanelBlit)],
    pub ghost_over: &'a [(usize, rewo_gpu::container::PanelBlit)],
}

/// Which of `RecipeBookSettings`' four `TypeSettings` a menu reads (M94).
///
/// **Only four menus have a book at all** — `RecipeBookMenu` is abstract and
/// exactly three concrete classes implement `getRecipeBookType`: the player's
/// own `InventoryMenu` and `CraftingMenu` (both CRAFTING) and
/// `AbstractFurnaceMenu`, which returns the type its subclass was built with.
/// Every other screen returns `None` here and draws no book, which is why a
/// chest is unaffected by any of this.
pub(super) fn book_type_of(
    layout: &rewo_world::menu_layout::MenuLayout,
) -> Option<rewo_world::recipe_book_screen::BookType> {
    use rewo_world::recipe_book_screen::BookType;
    // Keyed on the registry NAME, not the protocol id: the id is the server's
    // and a version bump renumbers it, while these four names are what the
    // decompile's class names correspond to. (M94's gate learned the hard way
    // that 13 is `enchantment`, not `crafting`.)
    if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID {
        // The player's own inventory IS a `RecipeBookMenu`: `InventoryMenu`
        // returns CRAFTING. It has no menu registry id because it is never
        // opened by `open_screen`.
        return Some(BookType::Crafting);
    }
    match layout.name {
        "crafting" => Some(BookType::Crafting),
        "furnace" => Some(BookType::Furnace),
        "blast_furnace" => Some(BookType::BlastFurnace),
        "smoker" => Some(BookType::Smoker),
        _ => None,
    }
}

/// An item's `getMaxStackSize`, for `accountStack`'s `min` (M96).
///
/// Falls back to 64, which is the default `Item.Properties` value and so the
/// right answer for the 1,242 of 1,537 items that do not override it.
pub(super) fn max_stack_of(items: &rewo_data::items::Items, id: i32) -> i32 {
    items
        .name(id)
        .map_or(rewo_data::item_props_table::DEFAULT_MAX_STACK, |n| {
            rewo_data::item_props_table::max_stack_size(n)
        })
}

/// Everything the recipe book's chrome needs for one frame (M94).
#[derive(Debug, Clone, Default)]
pub(crate) struct BookRender {
    pub view: Option<rewo_world::recipe_book_screen::BookView>,
    /// `(hasCraftable, hasMultipleRecipes)` for each collection on the page.
    pub slots: Vec<(bool, bool)>,
    pub hover: rewo_world::recipe_book_screen::BookHover,
    /// Which book this is, which decides its tab list and its filter art
    /// (M95). M94 assumed four tabs for every book; a crafting book has FIVE.
    pub book: rewo_world::recipe_book_screen::BookType,
    /// The item each visible slot shows this frame, already cycled — `None`
    /// for a collection whose result needs a context Rewo has not got.
    pub slot_items: Vec<Option<i32>>,
    /// Per visible slot: several recipes AND one shared result display, the
    /// pair of conditions that draws the shadow copy.
    pub slot_shadowed: Vec<bool>,
    /// The recipe id each visible slot would place if clicked (M98) — the one
    /// the display cycle is on, not the collection's first.
    pub slot_recipes: Vec<Option<i32>>,
    /// Per visible slot, its whole collection resolved into overlay buttons —
    /// **in the collection's own order**, not the overlay's (M104).
    ///
    /// The promotion to craftable-first happens when the overlay is opened,
    /// because that is when vanilla does it and because the overlay is a
    /// snapshot: re-promoting each frame would re-sort an open overlay under
    /// the cursor. See `rewo_world::recipe_overlay::Open`.
    pub slot_collections: Vec<Vec<rewo_world::recipe_overlay::Button>>,
}

/// The recipe book for the menu currently on screen (M94), or `None` when it
/// is shut — which is also what keeps the menu centred.
///
/// # What this does not do yet
///
/// The selected tab and the current page are **client** state that only a
/// click can change, and nothing can click the book yet, so both are pinned to
/// 0 and the tab column renders with the first tab selected. `hasCraftable` is
/// answered as of M96 — see `held` below for the one input it still misses.
/// `RecipeBookComponent.isVisible()` — whether the book is showing.
///
/// **One predicate, consulted by the render's placement and by every hover.**
/// `ScreenState::hovered` had its own conversion through `Placement::centred`
/// and so ignored the book entirely; M89 and M106b each fixed one consumer of
/// this same question and each recorded that a per-call-site choice is how
/// they come to disagree.
///
/// It is the **settings flag for the open menu's book type**, which is what
/// `initVisuals` sets `this.visible` from. [`live_recipe_book`] additionally
/// requires the display registries, so `book.is_some()` is narrower in
/// principle — and identical in practice, because those come from the bake at
/// session setup rather than off the wire, so they are present before any
/// menu can open. A menu with no book at all answers `false`.
pub(crate) fn book_visible(session: &rewo_net::play::PlaySession) -> bool {
    let layout = session
        .menus
        .open()
        .map(|m| m.menu.layout())
        .unwrap_or_else(|| session.inventory.layout());
    book_visible_for(book_type_of(layout), &session.recipe_book_settings)
}

/// [`book_visible`]'s rule, over plain values.
///
/// Lifted out because `PlaySession` owns a socket and cannot be constructed in
/// a test — M71's finding, and M97's fix. A mutation making a bookless menu
/// answer `true` survived the whole suite while this lived inside the
/// session-taking function, and a bookless menu answering `true` would
/// displace a chest's panel by 77 px and suppress its hover on a narrow
/// window.
pub(crate) fn book_visible_for(
    book: Option<rewo_world::recipe_book_screen::BookType>,
    settings: &rewo_net::recipe_book::BookSettings,
) -> bool {
    use rewo_world::recipe_book_screen as rb;
    match book {
        Some(rb::BookType::Crafting) => settings.crafting.open,
        Some(rb::BookType::Furnace) => settings.furnace.open,
        Some(rb::BookType::BlastFurnace) => settings.blast_furnace.open,
        Some(rb::BookType::Smoker) => settings.smoker.open,
        // A menu with no book — a chest, a beacon — is never displaced.
        None => false,
    }
}

pub(super) fn live_recipe_book(
    session: &rewo_net::play::PlaySession,
    items: &rewo_data::items::Items,
    state: rewo_world::recipe_book_screen::BookState,
    // M98 — the cursor in BOOK coordinates, or `None` when it is not being
    // tracked. Book space rather than screen: the conversion needs the window
    // size, and doing it once at the caller keeps the hover and the press
    // reading the same number.
    book_mouse: Option<(i32, i32)>,
    // M99 — the search field's contents, already lowercased, and the item
    // display-name map the search indexes.
    query: &str,
    display: &std::collections::HashMap<String, String>,
) -> Option<BookRender> {
    use rewo_world::recipe_book_screen as rb;
    let layout = session
        .menus
        .open()
        .map(|m| m.menu.layout())
        .unwrap_or_else(|| session.inventory.layout());
    let book = book_type_of(layout)?;
    let st = match book {
        rb::BookType::Crafting => session.recipe_book_settings.crafting,
        rb::BookType::Furnace => session.recipe_book_settings.furnace,
        rb::BookType::BlastFurnace => session.recipe_book_settings.blast_furnace,
        rb::BookType::Smoker => session.recipe_book_settings.smoker,
    };
    if !st.open {
        return None;
    }
    let names = session.recipe_display_ids.as_ref()?;
    let entries: Vec<BookEntry<'_>> = session
        .recipe_book
        .values()
        .filter_map(|e| {
            Some(BookEntry {
                id: e.id,
                group: e.group,
                category: names.category.name(e.category)?,
                results: e.display.result().items(),
                // M96 — the ingredient slots, tags resolved against the
                // server's own `update_tags` payload.
                ingredients: e.ingredients(&session.tags),
                // M99 — what the search indexes.
                search: search_entry_of(&e.display.result().items(), items, display),
                // M104 — the which-of-these overlay's ingredient grid.
                shape: e.display.overlay_shape(),
                grid_items: e.display.overlay_ingredients(),
            })
        })
        .collect();
    // What the player is holding, for `hasCraftable` — see
    // [`crafting_contents`], which is where the rules live so a test can reach
    // them (M97's lesson, fourth application).
    let mut held = crafting_contents(
        &session.inventory,
        session.menus.open().map(|m| &m.menu),
        book,
        &|id| max_stack_of(items, id),
    );
    // `Mth.floor(time / 30)` — vanilla's `time` advances by the partial tick
    // each render, so this is the session's tick clock divided by the swap
    // period. Shared by every slot, which is why a page of several-recipe
    // collections flips together rather than each on its own phase.
    let cycle = (session.ticks as f32 / rewo_net::recipe_book::TICKS_TO_SWAP_SLOT).floor() as i32;
    Some(book_render_from(
        book,
        st.filtering,
        &entries,
        &mut held,
        cycle,
        state,
        book_mouse,
        query,
    ))
}

/// The ghost recipe's two washes, split into the halves they belong in (M103).
///
/// Returns `(under, over)`. Vanilla's order per slot is fill, item, fill — so
/// the red goes with the panel's overlays and the white goes after the icons,
/// which is why they cannot be one list.
pub(crate) fn ghost_washes(
    ghosts: &[rewo_world::ghost_slots::Ghost],
    layout: &rewo_world::menu_layout::MenuLayout,
    player_inventory: bool,
) -> (
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    use rewo_world::ghost_slots as gs;
    let big = gs::big_result_slot(player_inventory);
    let argb = |v: u32| {
        [
            ((v >> 16) & 255) as f32 / 255.0,
            ((v >> 8) & 255) as f32 / 255.0,
            (v & 255) as f32 / 255.0,
            ((v >> 24) & 255) as f32 / 255.0,
        ]
    };
    let mut under = Vec::new();
    let mut over = Vec::new();
    for g in ghosts {
        let Some((sx, sy)) = layout.position(g.slot) else { continue };
        let (dx, dy, size) = gs::wash_rect(g.is_result, big);
        let fill = |tint: [f32; 4], dx: i32, dy: i32, size: i32| {
            (
                rewo_gpu::container::FILL_SPRITE,
                rewo_gpu::container::PanelBlit {
                    dx: (sx as i32 + dx) as f32,
                    dy: (sy as i32 + dy) as f32,
                    w: size as f32,
                    h: size as f32,
                    sx: 0.0,
                    sy: 0.0,
                    sw: 0.0,
                    sh: 0.0,
                    tint,
                },
            )
        };
        under.push(fill(argb(gs::WASH_UNDER), dx, dy, size));
        // The veil is always the plain 16x16 at the slot: only the wash BELOW
        // widens for a big result slot. Widening both would ring the icon in
        // white.
        over.push(fill(argb(gs::WASH_OVER), 0, 0, 16));
    }
    (under, over)
}

/// The ghost recipe for the shown menu, from `place_ghost_recipe` (M103).
///
/// M93y decoded that packet into `PlaySession::ghost_recipe` and nothing
/// consumed it — it is the only decoded-but-unrendered packet left in this
/// area. `handlePlaceRecipe` is gated on the container id matching the open
/// menu, so a ghost for a screen you have since closed is dropped rather than
/// drawn over whatever replaced it.
pub(crate) fn live_ghosts(
    session: &rewo_net::play::PlaySession,
) -> Vec<rewo_world::ghost_slots::Ghost> {
    use rewo_net::recipe_book::RecipeDisplay as D;
    use rewo_world::ghost_slots as gs;
    let Some((container, display)) = session.ghost_recipe.as_ref() else {
        return Vec::new();
    };
    // The ghost belongs to a container id; the shown menu has one too, and a
    // mismatch means the ghost is stale.
    if *container != session.shown_container_id() {
        return Vec::new();
    }
    let layout = session.shown_menu().layout();
    let book = match book_type_of(layout) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let items = |d: &rewo_net::recipe_book::SlotDisplay| d.items();
    let (menu, inputs, shape) = match display {
        D::CraftingShaped { width, height, ingredients, .. } => (
            // `getGridWidth/Height` — 2x2 for the player's own inventory, 3x3
            // for a crafting table.
            gs::crafting_menu(
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
            ),
            ingredients.iter().map(items).collect::<Vec<_>>(),
            Some((*width as usize, *height as usize)),
        ),
        D::CraftingShapeless { ingredients, .. } => (
            gs::crafting_menu(
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
            ),
            ingredients.iter().map(items).collect(),
            None,
        ),
        D::Furnace { ingredient, fuel, .. } => (
            gs::FURNACE_MENU,
            vec![items(ingredient), items(fuel)],
            None,
        ),
        // A stonecutter or smithing display: `fillGhostRecipe`'s switch has no
        // case, so the result alone is ghosted — and neither screen has a book,
        // so this is unreachable in practice and transcribed anyway.
        _ => (
            gs::GhostMenu { result: 0, grid: None, furnace: None },
            Vec::new(),
            None,
        ),
    };
    let fuel_empty = menu
        .furnace
        .is_some_and(|(_, f)| session.shown_menu().menu_slot(f).is_none());
    let _ = book;
    gs::layout(menu, items(display.result()), &inputs, shape, fuel_empty)
}

/// `fillStackedContents` + `fillCraftSlotsStackedContents` (M102).
///
/// Vanilla calls **two** fills and they are disjoint:
///
/// ```java
/// player.getInventory().fillStackedContents(contents);   // the ITEMS
/// menu.fillCraftSlotsStackedContents(contents);          // the GRID
/// ```
///
/// `Inventory.items` is armour + storage + hotbar + offhand —
/// `PLAYER_ITEM_SLOTS`, **5..46**. It contains neither the 2x2 crafting grid
/// (1..5, which belongs to `InventoryMenu`) nor the craft **result** (slot 0,
/// which belongs to nothing). Walking the whole 46-slot menu is the obvious
/// reading and it double-counts the grid *and* adds the result — so a recipe
/// would read as craftable off its own output.
///
/// The inventory fill is `accountSimpleStack`, hence gated on
/// `isUsableForCrafting`; the craft-slot fill's gating depends on the family
/// (`craft_slots`). M96 named the predicate in a comment and applied nothing.
///
/// Extracted from `live_recipe_book` because that function needs a
/// `PlaySession` and so cannot be reached from a test — a mutation deleting the
/// craft-slot half survived until this split.
pub(crate) fn crafting_contents(
    player: &rewo_world::inventory::Inventory,
    open: Option<&rewo_world::inventory::Inventory>,
    book: rewo_world::recipe_book_screen::BookType,
    // `getMaxStackSize` per item id. A closure rather than the whole registry,
    // so this stays a function of plain values and its tests do not need the
    // user's decompile on disk.
    max_stack: &dyn Fn(i32) -> i32,
) -> rewo_world::stacked_contents::StackedContents {
    use rewo_world::recipe_book_screen as rb;
    let mut held = rewo_world::stacked_contents::StackedContents::new();
    let mut account = |inv: &rewo_world::inventory::Inventory, slot: usize, gated: bool| {
        if let Some(st) = inv.menu_slot(slot) {
            if !gated || inv.is_usable_for_crafting(st) {
                held.account_stack(st.item_id, st.count, max_stack(st.item_id));
            }
        }
    };
    for slot in rb::PLAYER_ITEM_SLOTS {
        account(player, slot, true);
    }
    let shown = open.unwrap_or(player);
    if let Some(cs) = rb::craft_slots(book, open.is_none()) {
        for slot in cs.range.clone() {
            account(shown, slot, cs.gated);
        }
    }
    held
}

/// One unlocked recipe, with everything already resolved (M97).
///
/// The seam between the session and the derivation: the session half is
/// lookups, the derivation half is the grouping, paging, cycling and craftable
/// arithmetic — and only the second half is worth grading, which is why it is
/// on this side of the line.
pub(crate) struct BookEntry<'a> {
    pub id: i32,
    pub group: Option<i32>,
    pub category: &'a str,
    /// The result display's item ids, in order.
    pub results: Vec<i32>,
    /// `None` when `craftingRequirements` was absent — never craftable.
    pub ingredients: Option<Vec<rewo_world::stacked_contents::Ingredient>>,
    /// The result items' names and ids, lowercased, for the search (M99).
    pub search: rewo_world::recipe_search::SearchEntry,
    /// How the which-of-these overlay lays this recipe's ingredients out, and
    /// what each of them resolves to (M104). Read together: the shape decides
    /// where ingredient `n` goes, the list decides what it shows.
    pub shape: rewo_world::recipe_overlay::Shape,
    pub grid_items: Vec<Vec<i32>>,
}

/// What the search indexes for one recipe's result items (M99).
///
/// `getTooltipLines` over the results, plus their registry keys. For Rewo the
/// tooltip of a bare item id is its **display name** and nothing else, since
/// every other line comes from a component and a recipe's result carries none —
/// so this is exact rather than an approximation, and stops being exact only
/// if results ever arrive with components.
pub(crate) fn search_entry_of(
    results: &[i32],
    items: &rewo_data::items::Items,
    display: &std::collections::HashMap<String, String>,
) -> rewo_world::recipe_search::SearchEntry {
    let mut out = rewo_world::recipe_search::SearchEntry::default();
    for id in results {
        let Some(name) = items.name(*id) else { continue };
        // A missing translation falls back to the id's own path, prettified —
        // the same fallback the tooltip takes, so a search finds what the
        // tooltip shows.
        let shown = display
            .get(name)
            .cloned()
            .unwrap_or_else(|| display_name_of(name));
        out.names.push(shown.to_lowercase());
        let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
        out.ids.push((ns.to_lowercase(), path.to_lowercase()));
    }
    out
}

/// The book's per-frame state, from resolved inputs (M97).
///
/// Split out of [`live_recipe_book`] because a `PlaySession` owns a socket and
/// cannot be built in a test — M71's lesson: logic in a place with no test
/// module is untestable, so move it. M96 shipped this arithmetic graded only at
/// its two ends (the solver's own tests, and the chrome witness), with the
/// derivation between them untested — the M92/M93b shape.
pub(crate) fn book_render_from(
    book: rewo_world::recipe_book_screen::BookType,
    filtering: bool,
    entries: &[BookEntry<'_>],
    held: &mut rewo_world::stacked_contents::StackedContents,
    cycle: i32,
    // M98 — the tab and page a click chose. Clamped here rather than trusted:
    // a tab index survives a book-type change (a furnace book has fewer tabs
    // than a crafting one) and a page survives the list shrinking under it.
    state: rewo_world::recipe_book_screen::BookState,
    // The cursor in book coordinates, for the arrows' and filter's hover art.
    book_mouse: Option<(i32, i32)>,
    // The search field's contents, already lowercased (M99).
    query: &str,
) -> BookRender {
    use rewo_world::recipe_book_screen as rb;
    let flat: Vec<(i32, Option<i32>, &str)> =
        entries.iter().map(|e| (e.id, e.group, e.category)).collect();
    let all = rb::collections(&flat);
    let tabs = book.tabs();
    let selected_tab = state.selected_tab.min(tabs.len() - 1);
    // Stage one of `updateCollections` is the tab's own membership. Tab 0 is
    // the SEARCH tab, whose categories are the book's whole set — so with the
    // selection pinned to 0 this shows everything the book has.
    let wanted = tabs[selected_tab].categories;
    // `updateCollections`' stages, in order (M93z): the tab's membership, then
    // the SEARCH, then the filter. The search stage is skipped entirely on an
    // empty query rather than run with one — see `recipe_search::matches`.
    let mine: Vec<_> = all
        .iter()
        .filter(|c| wanted.contains(&c.category.as_str()))
        .filter(|c| {
            // A collection's searchable text is the union of its recipes',
            // which is what `flatMap` over `getRecipes()` gives.
            let mut e = rewo_world::recipe_search::SearchEntry::default();
            for id in &c.recipes {
                if let Some(src) = entries.iter().find(|x| x.id == *id) {
                    e.names.extend(src.search.names.iter().cloned());
                    e.ids.extend(src.search.ids.iter().cloned());
                }
            }
            rewo_world::recipe_search::matches(&e, query)
        })
        .collect();
    let total_pages = rb::total_pages(mine.len());
    let page = rb::clamp_page(state.page, mine.len(), false);
    let range = rb::page_range(page, mine.len());
    let find = |id: &i32| entries.iter().find(|e| e.id == *id);
    let mut slots = Vec::new();
    let mut slot_items = Vec::new();
    let mut slot_shadowed = Vec::new();
    let mut slot_recipes = Vec::new();
    let mut slot_collections = Vec::new();
    for c in &mine[range] {
        let per_entry: Vec<Vec<i32>> = c
            .recipes
            .iter()
            .map(|id| find(id).map(|e| e.results.clone()).unwrap_or_default())
            .collect();
        let multiple = per_entry.len() > 1;
        // `allRecipesHaveSameResultDisplay` — every display item across every
        // entry the same. Rewo compares item IDS, where vanilla compares whole
        // stacks with `isSameItemSameComponents`; two recipes yielding the same
        // item with different components would shadow here and not in vanilla.
        let mut seen = per_entry.iter().flatten();
        let first = seen.next().copied();
        let same_result = seen.all(|i| Some(*i) == first);
        // Per-recipe affordability, which the which-of-these overlay needs one
        // by one (M104) where the cell needs only whether ANY of them is.
        // A recipe whose `craftingRequirements` were absent is never craftable,
        // which is `canCraft`'s own opening line rather than a default.
        let per_recipe: Vec<bool> = c
            .recipes
            .iter()
            .map(|id| {
                find(id)
                    .and_then(|e| e.ingredients.as_ref())
                    .is_some_and(|ing| held.try_pick(ing, 1))
            })
            .collect();
        // `hasCraftable()` — ANY of them.
        let craftable = per_recipe.iter().any(|&c| c);
        slot_collections.push(
            c.recipes
                .iter()
                .zip(&per_recipe)
                .map(|(&id, &can)| {
                    let (shape, grid) = find(&id)
                        .map(|e| (e.shape, e.grid_items.clone()))
                        .unwrap_or((rewo_world::recipe_overlay::Shape::Other, Vec::new()));
                    rewo_world::recipe_overlay::Button {
                        recipe: id,
                        craftable: can,
                        // `if (!items.isEmpty())` — an ingredient that resolves
                        // to nothing contributes no position, and because
                        // neither placement arm derives its position from a
                        // running counter, dropping one does not shift the rest.
                        slots: rewo_world::recipe_overlay::grid_positions(
                            book.furnace_family(),
                            shape,
                        )
                        .into_iter()
                        .filter_map(|p| {
                            let items = grid.get(p.ingredient).cloned().unwrap_or_default();
                            (!items.is_empty()).then_some((p, items))
                        })
                        .collect(),
                    }
                })
                .collect(),
        );
        slots.push((craftable, multiple));
        slot_items.push(rewo_net::recipe_book::display_item(&per_entry, cycle));
        slot_shadowed.push(multiple && same_result);
        // `getCurrentRecipe` — the entry the cycle is showing, which is what a
        // left-click places. Not the collection's first: clicking while the
        // cycle is on the second of two recipes places the SECOND.
        let n = c.recipes.len() as i32;
        slot_recipes.push(if n == 0 {
            None
        } else {
            c.recipes
                .get((cycle - n * cycle.div_euclid(n)) as usize)
                .copied()
        });
    }
    let view = rb::BookView {
        tabs: tabs.len(),
        selected_tab,
        page,
        total_pages,
        shown: slots.len(),
        filtering,
        furnace_family: book.furnace_family(),
    };
    let hover = match book_mouse.and_then(|(bx, by)| rb::book_hit(bx, by, view, tabs.len())) {
        Some(rb::BookHit::PageForward) => rb::BookHover { page_forward: true, ..Default::default() },
        Some(rb::BookHit::PageBackward) => {
            rb::BookHover { page_backward: true, ..Default::default() }
        }
        Some(rb::BookHit::Filter) => rb::BookHover { filter: true, ..Default::default() },
        _ => rb::BookHover::default(),
    };
    BookRender {
        view: Some(rb::BookView {
            tabs: tabs.len(),
            selected_tab,
            page,
            total_pages,
            shown: slots.len(),
            filtering,
            furnace_family: book.furnace_family(),
        }),
        slots,
        // M98 — from the SAME `book_hit` the press uses, so what lights up is
        // what a click would take. Deriving the hover from its own rects would
        // be two chances to disagree.
        hover,
        book,
        slot_items,
        slot_shadowed,
        slot_recipes,
        slot_collections,
    }
}

/// Build the which-of-these overlay a right-click on page cell `index` opens
/// (M104).
///
/// A free function of plain values rather than a method reaching into the
/// session, so a test can drive it — M97's lesson, and M92's finding one layer
/// on: the arithmetic between "a collection" and "a placed popup" is exactly
/// what neither the model's tests nor a pixel gate can see on its own.
///
/// The `centre` it passes is the one `RecipeBookPage.mouseClicked` passes, and
/// its `+ 13` on the vertical half is *inert* — see
/// `recipe_overlay::origin`'s tests for the proof, and note the constant is
/// still written out rather than dropped.
pub(crate) fn open_overlay(
    collection: Vec<rewo_world::recipe_overlay::Button>,
    index: usize,
    view: rewo_world::recipe_book_screen::BookView,
) -> rewo_world::recipe_overlay::Open {
    use rewo_world::recipe_book_screen as rb;
    use rewo_world::recipe_overlay as ro;
    // Craftable first, and the flag rides along rather than being recomputed:
    // the overlay is a snapshot, so its ordering and its greying are fixed at
    // this moment together.
    let pairs: Vec<(ro::Button, bool)> = collection
        .into_iter()
        .map(|b| {
            let c = b.craftable;
            (b, c)
        })
        .collect();
    let buttons: Vec<ro::Button> =
        ro::promote(&pairs, view.filtering).into_iter().map(|(b, _)| b).collect();
    ro::Open {
        origin: ro::origin(
            rb::grid_slot(index),
            buttons.len(),
            (rb::IMAGE_W / 2, 13 + rb::IMAGE_H / 2),
            ro::BUTTON_PITCH as f32,
        ),
        furnace: view.furnace_family,
        buttons,
    }
}

/// The anvil's name field, as `AnvilScreen.init` builds it (M101).
///
/// `setCanLoseFocus(false)` and `setInitialFocus(this.name)` — focused from the
/// moment the screen opens and unable to lose it, so its caret blinks for as
/// long as the screen is up.
///
/// A function rather than three lines inline, because those three lines sit
/// inside a path that needs a `PlaySession` and so cannot be reached from a
/// test — M93t wrote the comment and did only the first half, and nothing
/// caught it for eight milestones. Here the claim is one call to something
/// graded below.
pub(super) fn anvil_field_new() -> rewo_world::edit_box::EditBox {
    let mut field = rewo_world::edit_box::EditBox::new(rewo_world::anvil::MAX_NAME_LENGTH);
    field.set_can_lose_focus(false);
    field.set_focused(true);
    field
}

/// `scrollTo(cursorPos)` for the book's field (M101).
///
/// Vanilla folds this into `setCursorPosition`; Rewo cannot, because the width
/// function is the caller's — so every input path that moves the cursor calls
/// it. Missing it leaves a field typed past its visible width showing the head
/// of the string with no caret at all.
pub(super) fn follow_cursor(
    field: &mut rewo_world::edit_box::EditBox,
    baked: Option<&assets::BakedAssets>,
    inner_width: i32,
) {
    let Some(font) = baked.and_then(|b| b.font.as_ref()) else {
        return;
    };
    let advance = font.advance;
    let width = move |u: &[u16]| rewo_gpu::text::width(&String::from_utf16_lossy(u), &advance);
    field.follow_cursor(inner_width, &width);
}

/// The search field's background, text, caret and selection (M100).
///
/// # The nine-slice degenerates to two blits
///
/// `widget/text_field` is 200x20 with `border: 1` — but it is a **1-bit
/// paletted** image of exactly two colours: the border 160-grey (white when
/// focused) and the interior black, both fully opaque, measured out of the PNG.
/// Every one of the nine regions is therefore uniform, and a stretched 1x1
/// source is **pixel-identical** to a tiled one.
///
/// So: one blit of the whole rect sampling a border texel, then one of the
/// interior sampling a centre texel. The 1 px the first blit still shows around
/// the second *is* the border. Nine blits would draw the same pixels; two is
/// what the measurement licenses.
pub(super) fn book_field_render(
    field: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    use rewo_data::assets as a;
    use rewo_world::recipe_book_screen as rb;
    let origin = rewo_gpu::container::recipe_book_origin(w, h);
    // `SPRITES.get(isActive(), isFocused())` — the one use of that record on
    // this screen that means what its argument names say.
    let sprite = a::BOOK_SEARCH_FIELD + usize::from(rb::search_sprite_focused(field.is_focused()));
    let quad = |dx: i32, dy: i32, w: i32, h: i32, sx: f32, sy: f32| {
        (
            sprite,
            rewo_gpu::container::PanelBlit {
                dx: dx as f32,
                dy: dy as f32,
                w: w as f32,
                h: h as f32,
                sx,
                sy,
                // A 1x1 source, stretched — exact because the region is
                // uniform. See the note above.
                sw: 1.0,
                sh: 1.0,
                tint: [1.0; 4],
            },
        )
    };
    let mut fills = vec![
        // The border colour over the whole rect…
        quad(rb::SEARCH_X, rb::SEARCH_Y, rb::SEARCH_W, rb::SEARCH_H, 0.0, 0.0),
        // …then the interior, one pixel in on every side.
        quad(
            rb::SEARCH_X + 1,
            rb::SEARCH_Y + 1,
            rb::SEARCH_W - 2,
            rb::SEARCH_H - 2,
            100.0,
            10.0,
        ),
    ];
    let (labels, text_fills, _) = edit_box_render(
        field,
        advance,
        origin,
        (rb::SEARCH_TEXT_X, rb::SEARCH_TEXT_Y, rb::SEARCH_INNER_W),
        Some((rb::SEARCH_HINT, rb::SEARCH_HINT_COLOR)),
        now_ms,
    );
    fills.extend(text_fills);
    (labels, fills)
}

/// The GUI-space top-left of the slot the cursor is over, for the highlight
/// (M106b).
///
/// **Through the same [`rewo_gpu::container::Placement`] the pass draws with.**
/// This was a bare `screen_to_gui_for`, which centres, while
/// `ContainerPass::set_state` resolves its own origin with
/// `Placement::with_book` — so with the recipe book open the cursor was
/// converted against a panel 77 GUI px left of the one the highlight was drawn
/// against, and the lit slot sat four columns right of the cursor. The
/// tooltip's conversion had the same bug; the panel, the slot icons
/// ([`menu_slot_rects`]) and the bespoke-widget hovers did not.
///
/// Extracted from `apply_screen` rather than fixed in place because that
/// function needs a `PlaySession` and no gate reaches it: every `set_container`
/// call in the whole app passes `hovered: None`, so the derivation had no
/// witness of any kind. M97's lesson again — move the logic to where a test can
/// see it.
pub(super) fn hovered_slot_position(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<(i32, i32)> {
    let slot = hovered_menu_slot(layout, mouse, w, h, book_open)?;
    layout.position(slot).map(|(x, y)| (x as i32, y as i32))
}

/// `AbstractContainerScreen.getHoveredSlot` — which menu slot the cursor is
/// over, through the book-aware placement.
///
/// **Rewo does not model `AbstractRecipeBookScreen.isHovering`'s narrow-window
/// override**, which returns false for every slot while the book is visible and
/// the window is under 379 GUI px — the case where the book covers the menu. In
/// vanilla that makes `hoveredSlot` null there, suppressing the highlight, the
/// item tooltip, the ghost tooltip and the number/Q/F keyboard actions
/// together. It is one predicate with five consumers and belongs in its own
/// change, not smuggled in with a tooltip.
pub(crate) fn hovered_menu_slot(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<usize> {
    // `AbstractRecipeBookScreen.isHovering` (M112):
    //
    //     return (!this.widthTooNarrow || !this.recipeBookComponent.isVisible())
    //            && super.isHovering(...);
    //
    // On a window under 379 GUI px the book does not sit BESIDE the menu, it
    // sits OVER it — `updateScreenPosition`'s 177 px shift collapses to 0 —
    // so vanilla answers "no slot" for every slot rather than letting a click
    // reach through the panel that is covering it. Without this, a narrow
    // window lets a click on the book land on whatever menu slot happens to be
    // underneath.
    //
    // Placed here rather than at each consumer because it is ONE predicate
    // with several: the highlight, both tooltips, the click, the double-click
    // detector, the drag and the number/Q/F actions all arrive through this
    // function.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let gui_w = (w / scale) as i32;
    if book_open && rewo_world::recipe_book_screen::width_too_narrow(gui_w) {
        return None;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_placed(
        mouse,
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            book_open,
        ),
    );
    layout.slot_at(gx, gy)
}

/// [`hovered_menu_slot`] under its own name, so a gate can assert that a
/// witness's cursor really is over the slot it thinks it is.
///
/// b22 first passed for the wrong reason: it compared a book-open cursor
/// against a book-shut conversion, so the `None` it was reading came from the
/// 77 px placement shift and not from the guard it names. A mutation deleting
/// that guard survived. This exists so the witness can say which of the two it
/// is measuring.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn hovered_menu_slot_for_gate(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<usize> {
    hovered_menu_slot(layout, mouse, w, h, book_open)
}

/// Which of a frame's three tooltip producers wins (M106c).
///
/// `setTooltipForNextFrameInternal` is
/// `if (this.deferredTooltip == null || replaceExisting)`, and `replaceExisting`
/// is false on every path any of these three takes — so the **first** tooltip
/// set in a frame wins, and the calls that follow it are discarded.
///
/// That inverts the reading of `AbstractRecipeBookScreen.extractRenderState`,
/// which calls the container's `extractTooltip` and *then* the book's, as
/// though the book overwrote it. Writing this as a named function rather than
/// an `or_else` chain at the call site is the point: the parameters say which
/// producer is which, so getting the order wrong is a named mistake instead of
/// an anonymous one, and the rule has somewhere to be tested.
///
/// The producers are closures because two of the three are only worth
/// evaluating when the earlier ones declined — the container's tooltip walks a
/// component patch and the book's measures a page.
///
/// `ctx` is threaded through them rather than captured because all three want
/// the same `&mut GlyphCache`, and three closures cannot hold it at once. The
/// reborrow per call is what makes the laziness expressible at all.
pub(super) fn frame_tooltip<T, C>(
    ctx: &mut C,
    book_widget: impl FnOnce(&mut C) -> Option<T>,
    menu: impl FnOnce(&mut C) -> Option<T>,
    book_page: impl FnOnce(&mut C) -> Option<T>,
    ghost: impl FnOnce(&mut C) -> Option<T>,
) -> Option<T> {
    // M133 — the book's WIDGET tooltips come first, and the order is read off
    // `AbstractRecipeBookScreen`: `recipeBookComponent.extractRenderState`
    // runs before `this.extractTooltip`, and every widget rendered inside it
    // calls `refreshTooltipForNextRenderPass` -> `setTooltipForNextFrame`. So
    // by the same first-wins rule M106a found, an arrow or the filter beats
    // both the container's item tooltip and the book's cell tooltip.
    if let Some(t) = book_widget(ctx) {
        return Some(t);
    }
    if let Some(t) = menu(ctx) {
        return Some(t);
    }
    if let Some(t) = book_page(ctx) {
        return Some(t);
    }
    ghost(ctx)
}

/// The tooltip of a GHOST ingredient under the cursor (M106c).
///
/// `GhostSlots.extractTooltip` — keyed on the hovered menu slot, showing the
/// item the ghost's own cycle is on.
///
/// **This is where first-wins becomes observable.** The container's tooltip is
/// set earlier in the frame and `setTooltipForNextFrameInternal` only assigns
/// when nothing has claimed the slot, so a ghost drawn over a slot that already
/// holds an item describes the REAL item, not the ghost. Ordering these
/// producers the way their calls read — the book's last, therefore winning —
/// would show the ghost's name over a filled slot, which is plausible and
/// wrong.
///
/// **Gated on the book being open**, unlike the ghost's own render:
/// `extractGhostRecipe` is called unconditionally from `extractSlots` while
/// `extractTooltip` sits inside `if (this.isVisible())`. So shutting the book
/// leaves the ghost painted and stops it describing itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ghost_tooltip(
    ghosts: &[rewo_world::ghost_slots::Ghost],
    cycle: i32,
    layout: &'static rewo_world::menu_layout::MenuLayout,
    book_open: bool,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    if !book_open {
        return None;
    }
    let slot = hovered_menu_slot(layout, mouse, w, h, book_open)?;
    // `this.ingredients.get(hoveredSlot)` — a map keyed by the slot, so a
    // hovered slot with no ghost is simply absent rather than a miss to
    // recover from.
    let ghost = ghosts.iter().find(|g| g.slot == slot)?;
    let item = ghost.item(cycle)?;
    let item_name = items.name(item)?;
    let lines = vec![vec![rewo_gpu::tooltip::Span::new(
        names.get(item_name)?.to_string(),
        rarity_color(stack_rarity(Some(item_name), None, false)),
    )]];
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// Every label and fill the open book contributes (M105).
///
/// The field's text and the page counter belong to different objects — an
/// `EditBox` on the component, and `RecipeBookPage`'s own `extractRenderState`
/// — but they share both preconditions (an open book, a font to measure with)
/// and they are drawn into one list. Composing them here rather than at the
/// call site is deliberate: `apply_screen` needs a `PlaySession` and so cannot
/// be reached from a test, and a composition step performed there would be
/// deletable with every unit test still green. That is M99's lesson — shrink
/// the untestable surface rather than pretend to cover it.
pub(super) fn book_labels(
    b: &BookRender,
    field: &rewo_world::edit_box::EditBox,
    lang: &rewo_data::lang::Language,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    let (mut labels, fills) = book_field_render(field, advance, w, h, now_ms);
    labels.extend(b.view.and_then(|v| book_page_label(v, lang, advance, w, h)));
    (labels, fills)
}

/// The `x/y` page counter under the recipe grid (M105).
///
/// The one piece of the book vanilla draws as bare text rather than through a
/// widget: `RecipeBookPage.extractRenderState` opens with it, before the
/// buttons and the arrows. Its model constants have existed since M93z with
/// nothing reading them.
///
/// Three details that a call site alone does not carry:
///
/// * **The five-argument `graphics.text` delegates to the six-argument one with
///   `dropShadow = true`.** Transcribing the visible arguments and defaulting
///   the shadow to `false` loses it with nothing to notice.
/// * **Colour `-1` is `0xFFFFFFFF`** — opaque white. `text` also early-returns
///   on `ARGB.alpha(color) == 0`, which is why the alpha is worth reading
///   rather than assuming.
/// * The x is computed **by hand** as `xo - width / 2 + 73` rather than through
///   the `centeredText` helper sitting a few lines away in the same class. The
///   two are arithmetically identical here; the hand-written form is kept
///   because it is what the class does.
pub(super) fn book_page_label(
    view: rewo_world::recipe_book_screen::BookView,
    lang: &rewo_data::lang::Language,
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    use rewo_world::recipe_book_screen as rb;
    // `Language.getOrDefault` returns the key when the map has no entry, and a
    // template with no specifiers survives `decomposeTemplate` unchanged — so
    // `or_key` is vanilla's behaviour, not a local fallback.
    let text = rb::page_label(view.page, view.total_pages, lang.or_key(rb::PAGE_LABEL_KEY))?;
    let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(w, h);
    // Measured with the SAME advances the text pass will draw with. Measuring
    // one font and drawing another is what M52b found in the tooltip when it
    // moved to Newsreader; here there is only the bitmap font, and passing the
    // table in keeps it that way by construction.
    let width = rewo_gpu::text::width(&text, advance);
    Some(rewo_gpu::world::OwnedTextLine {
        x: bl + rb::page_label_x(width) as f32 * scale,
        y: bt + rb::PAGE_LABEL_Y as f32 * scale,
        px: scale,
        color_linear: [1.0, 1.0, 1.0],
        alpha: 1.0,
        shadow: true,
        style: rewo_gpu::text::TextStyle::PLAIN,
        text,
    })
}

/// The book's chrome as blits, with its semantic sprites resolved to atlas
/// indices.
///
/// The mapping lives here rather than in the model for the reason the model's
/// doc gives: `MENU_OVERLAY_SPRITES` is append-only and its order is an atlas
/// contract, so keeping the geometry free of it lets the atlas grow without
/// touching a geometry file — and lets the model's tests name a *sprite* rather
/// than an index.
pub(crate) fn recipe_book_panel(
    b: &BookRender,
    // M100 — the search field's own quads, appended AFTER the chrome so the
    // field's background sits over the panel and its caret over that.
    field: &[(usize, rewo_gpu::container::PanelBlit)],
    // M104 — the open which-of-these overlay, if any, and the cursor in book
    // pixels for its hover. Drawn LAST, which is `graphics.nextStratum()`.
    open: Option<&rewo_world::recipe_overlay::Open>,
    book_mouse: Option<(i32, i32)>,
) -> Option<rewo_gpu::container::RecipeBookPanel> {
    use rewo_data::assets as a;
    use rewo_world::recipe_book_screen as rb;
    let view = b.view?;
    let mut blits = Vec::new();
    let mut overlays = Vec::new();
    let chrome = rb::book_chrome(view, &b.slots, b.hover)
        .into_iter()
        .chain(open.into_iter().flat_map(|o| {
            rb::overlay_chrome(
                o.origin,
                &o.craftable_flags(),
                o.furnace,
                book_mouse.and_then(|(bx, by)| o.hovered(bx, by)),
            )
        }));
    for q in chrome {
        let (sx, sy, sw, sh) = q.src.unwrap_or((0, 0, q.w, q.h));
        let blit = rewo_gpu::container::PanelBlit {
            dx: q.x as f32,
            dy: q.y as f32,
            w: q.w as f32,
            h: q.h as f32,
            sx: sx as f32,
            sy: sy as f32,
            sw: sw as f32,
            sh: sh as f32,
            tint: [1.0; 4],
        };
        match q.sprite {
            // The panel is the one quad that comes off a background SHEET
            // rather than a sprite, and the one whose source is not (0, 0).
            rb::BookSprite::Panel => blits.push(rewo_gpu::container::PanelBlit {
                sx: rb::PANEL_SOURCE.0 as f32,
                sy: rb::PANEL_SOURCE.1 as f32,
                ..blit
            }),
            rb::BookSprite::Tab { selected } => {
                overlays.push((a::BOOK_TAB + usize::from(selected), blit))
            }
            rb::BookSprite::Slot(s) => overlays.push((
                a::BOOK_SLOT
                    + match s {
                        rb::SlotSprite::Craftable => 0,
                        rb::SlotSprite::ManyCraftable => 1,
                        rb::SlotSprite::Uncraftable => 2,
                        rb::SlotSprite::ManyUncraftable => 3,
                    },
                blit,
            )),
            rb::BookSprite::PageForward { hovered } => {
                overlays.push((a::BOOK_PAGE_ARROW + usize::from(hovered), blit))
            }
            rb::BookSprite::PageBackward { hovered } => {
                overlays.push((a::BOOK_PAGE_ARROW + 2 + usize::from(hovered), blit))
            }
            rb::BookSprite::Filter { furnace, filtering, hovered } => {
                let base = if furnace { a::BOOK_FILTER_FURNACE } else { a::BOOK_FILTER };
                overlays.push((base + rb::filter_sprite_offset(filtering, hovered), blit))
            }
            // M104 — the only sprite in this list that is nine-sliced, so the
            // one whose `src` is not the whole of it.
            rb::BookSprite::OverlayPanel => overlays.push((a::BOOK_OVERLAY_PANEL, blit)),
            rb::BookSprite::OverlayButton { furnace, craftable, hovered } => overlays.push((
                a::BOOK_OVERLAY_BUTTON
                    + if furnace { 4 } else { 0 }
                    + 2 * usize::from(!craftable)
                    + usize::from(hovered),
                blit,
            )),
        }
    }
    overlays.extend_from_slice(field);
    Some(rewo_gpu::container::RecipeBookPanel { blits, overlays })
}

/// [`container_panel`] for `containershot`, which drives the production
/// builder rather than a copy of it — M45's finding: a gate that reimplements
/// a slice of the app's setup misses whatever the app adds to it.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn container_panel_for_test(
    layout: &'static rewo_world::menu_layout::MenuLayout,
) -> Option<rewo_gpu::container::ContainerPanel> {
    container_panel(layout, None, EnchantPlayer::default(), None)
}

/// [`container_panel`] for an *open* menu, so `containershot` can grade the
/// M92 overlays — which only exist when there are data slots to read.
///
/// Drives the production builder for M45's reason: a gate that reimplements a
/// slice of the app's setup misses whatever the app adds to it.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn container_panel_for_open_menu(
    open: &rewo_world::menu::OpenMenu,
    xp_level: i32,
    creative: bool,
    beacon_effects: BeaconEffectIds,
    mouse_gui: Option<(f64, f64)>,
    // M93m — the beacon screen's own choice. Carried on the SAME entry point
    // the gate already drives rather than a second one, so `containershot`
    // cannot exercise a path the live client does not take (M45).
    beacon_override: Option<rewo_world::menu_screen::BeaconChoice>,
    // M93q — the loom's grid. Carried for the same reason as the beacon's
    // choice, and needed for the same reason: with it hardcoded `None` the
    // gate could not reach `menu_overlays`' loom arm at all, so M93q's fill
    // was witnessed as a *primitive* (o19/o20) and never as a *use* — delete
    // the whole arm and both stayed green. That is M92's finding one level
    // over: a gate that cannot reach a call site does not test it.
    //
    // The view is SUPPLIED, not resolved, because resolving it needs an item
    // registry this gate has not got. What that leaves untested is the
    // resolution in `live_frame`, which `d7`–`d9` grade item-side and
    // `loom_pattern_table`'s own tests grade set-side.
    loom: Option<LoomView>,
    // M93s — the stonecutter's grid, supplied for the same reason.
    cut: Option<&CutView>,
    // M93u — and the merchant's trade list.
    merchant: Option<&MerchantView>,
) -> Option<rewo_gpu::container::ContainerPanel> {
    container_panel(
        open.layout,
        Some(open),
        EnchantPlayer {
            xp_level,
            creative,
            beacon_effects,
            beacon_override,
            loom,
            cut,
            // A gate drives no anvil field; `containershot` supplies its own
            // overlays when it wants a fill (M93q's o19/o20).
            anvil_fills: &[],
            merchant,
            // A gate drives no ghost recipe either.
            ghost_under: &[],
            ghost_over: &[],
        },
        mouse_gui,
    )
}

/// [`BeaconEffectIds::resolve`] for `containershot`.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn beacon_effect_ids_for_test(
    m: &rewo_data::mob_effects::MobEffects,
) -> BeaconEffectIds {
    BeaconEffectIds::resolve(m)
}

/// [`sheet_index`] for `containershot`.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn sheet_index_for_test(texture: &str) -> Option<usize> {
    sheet_index(texture)
}

/// A texture's index in the atlas, by the path the bake loaded it under.
///
/// `menu_screen` spells paths in vanilla's `Identifier` form and the bake in
/// the jar-relative one, so the prefix comes off here. The two lists are
/// cross-checked by a test in `rewo-world`; this returning `None` would mean
/// that check had been removed.
pub(super) fn sheet_index(texture: &str) -> Option<usize> {
    let want = texture.trim_start_matches("textures/");
    rewo_data::assets::MENU_BACKGROUND_TEXTURES
        .iter()
        .position(|t| *t == want)
}

/// Every slot's on-screen rect for a menu, in its own layout and at its own
/// panel size (M87).
///
/// Takes the menu rather than assuming the player's: a container is a
/// different slot count *and* a different panel, and the panel is what centres
/// it — a six-row chest is 176x222, so measuring its slots from a 176x166
/// origin puts every one of them 28 px low.
/// Where each of a menu's slots sits on screen.
///
/// `book_open` is not optional decoration: an open recipe book MOVES the menu
/// (M94), and this origin has to move with the panel's or every icon lands 77 px
/// left of the slot it belongs to. M94 threaded the book through the panel draw
/// and the hover and **missed this path**, which is the one-accessor rule half
/// applied — the same shape as M90's `slot_kind`, and missed for the same
/// reason: a function taking bare numbers does not look like it belongs to the
/// menu.
pub(super) fn menu_slot_rects(
    menu: &rewo_world::inventory::Inventory,
    w: f32,
    h: f32,
    book_open: bool,
) -> Vec<(f32, f32, f32)> {
    let layout = menu.layout();
    let (left, top, scale) = rewo_gpu::container::gui_origin_placed(
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            book_open,
        ),
    );
    (0..menu.slot_count())
        .map(|i| {
            let (x, y) = layout.position(i).unwrap_or((0, 0));
            (
                left + x as f32 * scale,
                top + y as f32 * scale,
                16.0 * scale,
            )
        })
        .collect()
}

/// The player inventory's slot rects — [`menu_slot_rects`] for the menu that
/// was the only one before M87.
pub(super) fn screen_slot_rects(w: f32, h: f32) -> Vec<(f32, f32, f32)> {
    menu_slot_rects(&rewo_world::inventory::Inventory::default(), w, h, false)
}

/// This frame's icons and stack counts for the open screen.
///
/// Returns the draw list plus the count labels, which go through the text pass
/// — vanilla's `itemCount` draws them at `x + 19 - 2 - width` and `y + 6 + 3`,
/// right-aligned inside the slot, and **only when the count is not one**.
/// Load the Velvet families into a glyph cache (M52b).
///
/// Returns `None` if any face is missing, and the caller falls back to the
/// bitmap pass. Partial loading is deliberately not a state: a tooltip that
/// renders its upright spans and drops its italic ones would look like a
/// styling bug rather than a missing file.
pub(super) fn load_velvet_fonts() -> Option<rewo_gpu::velvet_glyph::GlyphCache> {
    use rewo_gpu::velvet_glyph::{Family, GlyphCache};
    // Next to the executable first (a packaged build), then the workspace
    // path (cargo run) -- the same order the launcher's font resolution uses.
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("assets/fonts")));
    let dir = match beside_exe {
        Some(d) if d.is_dir() => d,
        _ => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts"),
    };
    let mut c = GlyphCache::new();
    for (fam, italic) in [
        (Family::Newsreader, false),
        (Family::Newsreader, true),
        (Family::Fraunces, false),
        (Family::JetBrainsMono, false),
    ] {
        let p = dir.join(format!("{}.ttf", fam.file_stem(italic)));
        let data = std::fs::read(&p).ok()?;
        if !c.load(fam, italic, data) {
            log::warn!("velvet: {} failed to parse", p.display());
            return None;
        }
    }
    log::info!("velvet: fonts loaded from {}", dir.display());
    Some(c)
}

/// The tooltip's Velvet body size, in **GUI pixels**.
///
/// Vanilla's tooltip text is the 8px bitmap font on a 10px line. Newsreader at
/// the same nominal size reads smaller, because a bitmap glyph fills its cell
/// and a proportional one does not -- so this is calibrated to the *cap
/// height* rather than the em, which is what makes the two look like the same
/// size next to each other.
pub const TOOLTIP_TEXT_GUI_PX: f32 = 9.0;

/// A tooltip line's Velvet key.
pub(super) fn tooltip_key(italic: bool, scale: f32) -> rewo_gpu::velvet_glyph::ScalerKey {
    rewo_gpu::velvet_glyph::ScalerKey::new(
        rewo_gpu::velvet_glyph::Family::Newsreader,
        italic,
        TOOLTIP_TEXT_GUI_PX * scale,
        rewo_gpu::velvet_glyph::Axes::DEFAULT,
    )
}

/// Width of a styled tooltip line in **GUI pixels**, measured with the same
/// font that will draw it.
///
/// This is the half of the flip that is easy to skip and would have shown up
/// as text spilling out of its box: once the tooltip renders in Newsreader,
/// sizing it with the bitmap advances measures a font it no longer uses.
pub(super) fn velvet_line_width(
    cache: &mut rewo_gpu::velvet_glyph::GlyphCache,
    line: &rewo_gpu::tooltip::Line,
    scale: f32,
) -> f32 {
    line.iter()
        .map(|sp| cache.measure_tracked(tooltip_key(sp.italic, scale), &sp.text, 0.0))
        .sum::<f32>()
        / scale.max(0.001)
}
/// The hovered slot's tooltip: the box to draw, and the text line inside it
/// (M40).
///
/// Vanilla builds the lines with `Screen.getTooltipFromItem`, which starts
/// with the stack's hover name and then appends everything its **components**
/// say — enchantments, lore, durability, attribute modifiers, the "When on
/// body" block. Rewo can see none of those: `rewo_net::item_stack` reports
/// only *whether* a patch was present. So a Rewo tooltip is the first line and
/// nothing else, which is exactly what vanilla shows for a plain stack, and
/// short of what it shows for an enchanted one. Drawing a wrong second line
/// would be worse than drawing none.
///
/// The name is also always in the common rarity's white: rarity rides on
/// `DataComponents.RARITY`, which is another component.
/// The durability bars for a set of slots (M41).
///
/// `ItemStack.isBarVisible()` is `isDamaged()`, so a pristine tool has **no
/// bar**, not a full one — which is why this returns nothing for an undamaged
/// stack rather than a 13-wide green one.
///
/// The two halves of the fraction come from different places, and that is the
/// point of the milestone: the numerator `minecraft:damage` rides on the wire
/// as a component patch, the denominator does not — every diamond pickaxe has
/// the same 1561, so it lives in the generated item table. A patch that
/// overrides `max_damage` still wins, because a plugin may.
pub(super) fn item_bars(
    slots: &[(Option<rewo_world::inventory::ItemSlot>, (f32, f32, f32))],
    items: &rewo_data::items::Items,
    unbreakable: impl Fn(rewo_world::inventory::ItemSlot) -> bool,
) -> Vec<rewo_gpu::container::ItemBar> {
    let mut out = Vec::new();
    for (stack, (x, y, size)) in slots {
        let Some(stack) = stack else { continue };
        let max = stack
            .max_damage
            .or_else(|| items.name(stack.item_id).and_then(rewo_data::item_props_table::max_damage));
        // An item whose maximum this build cannot resolve gets no bar. A bar
        // needs a denominator, and inventing one would draw a confident and
        // wrong amount of remaining life.
        let Some(max) = max.filter(|m| *m > 0) else {
            continue;
        };
        // `isBarVisible()` is `isDamaged()`, and that is
        // `has(MAX_DAMAGE) && !has(UNBREAKABLE) && has(DAMAGE) && damage > 0`
        // — so an **Unbreakable** tool never shows one however much damage it
        // carries (M66 corrected this; M41 read the damage alone). The damage
        // is also clamped to the maximum, which is what stops a server sending
        // more than the item can take from producing a negative width.
        let damage = stack.damage.unwrap_or(0).clamp(0, max);
        if damage <= 0 || unbreakable(*stack) {
            continue;
        }
        out.push(rewo_gpu::container::ItemBar {
            x: *x,
            y: *y,
            // The slot rects are in screen pixels at `size` per 16 GUI pixels.
            scale: size / 16.0,
            width: rewo_gpu::container::bar_width(damage, max),
            color: rewo_gpu::container::bar_color(damage, max),
        });
    }
    out
}

/// `ItemStack.getRarity()` — the id whose colour the hover name takes (M50).
///
/// ```java
/// Rarity baseRarity = this.getOrDefault(DataComponents.RARITY, Rarity.COMMON);
/// if (!this.isEnchanted()) return baseRarity;
/// return switch (baseRarity) {
///    case COMMON, UNCOMMON -> Rarity.RARE;
///    case RARE             -> Rarity.EPIC;
///    default               -> baseRarity;
/// };
/// ```
///
/// Two halves, and Rewo could previously see only one of them. `getOrDefault`
/// answers from the item's **prototype** when the patch says nothing, and the
/// patch is all the wire carries — so `rarity.unwrap_or(COMMON)` painted all
/// **115** of 26.2's non-common items white. The prototype half is
/// [`rewo_data::item_props_table::rarity`], generated from the datagen
/// component report.
///
/// `is_enchanted` is `minecraft:enchantments` alone — see
/// [`rewo_world::inventory::SlotText::is_enchanted`] on why an enchanted book
/// is not enchanted.
///
/// An id outside the enum passes through the `default` arm unchanged, exactly
/// as a future `Rarity` constant would.
pub(crate) fn stack_rarity(item_name: Option<&str>, patch: Option<i32>, is_enchanted: bool) -> i32 {
    let base = patch.unwrap_or_else(|| {
        item_name
            .map(rewo_data::item_props_table::rarity)
            .unwrap_or(rewo_data::item_props_table::DEFAULT_RARITY)
    });
    if !is_enchanted {
        return base;
    }
    match base {
        0 | 1 => 2,
        2 => 3,
        other => other,
    }
}

/// `Rarity.color()` — the hover name's colour, by rarity id.
///
/// An unknown id is treated as common rather than wrapping into another
/// colour: the enum is a small one today and a version that grows it should
/// not repaint every item.
pub(super) fn rarity_color(rarity: i32) -> [f32; 3] {
    let rgb = match rarity {
        1 => 0xFFFF55u32, // UNCOMMON — yellow
        2 => 0x55FFFF,    // RARE — aqua
        3 => 0xFF55FF,    // EPIC — light purple
        _ => 0xFFFFFF,    // COMMON
    };
    [
        ((rgb >> 16) & 0xFF) as f32 / 255.0,
        ((rgb >> 8) & 0xFF) as f32 / 255.0,
        (rgb & 0xFF) as f32 / 255.0,
    ]
}

/// `ItemLore`'s style — dark purple and italic. Rewo has no italic face, so
/// only the colour carries.
pub(super) const LORE_COLOR: [f32; 3] = [170.0 / 255.0, 0.0, 170.0 / 255.0];
/// `ItemStack.UNBREAKABLE_TOOLTIP`, which is blue.
pub(super) const UNBREAKABLE_COLOR: [f32; 3] = [85.0 / 255.0, 85.0 / 255.0, 1.0];

/// `ChatFormatting.GRAY` — an ordinary enchantment line.
pub(super) const ENCHANT_COLOR: [f32; 3] = [170.0 / 255.0, 170.0 / 255.0, 170.0 / 255.0];
/// `ChatFormatting.RED` — a curse's.
pub(super) const CURSE_COLOR: [f32; 3] = [1.0, 85.0 / 255.0, 85.0 / 255.0];

/// `Enchantment.getFullname` for each of a stack's enchantments, in
/// `ItemEnchantments.addToTooltip`'s order (M42).
///
/// Three rules, each of which is a way to be visibly wrong:
///
/// - **The level numeral is suppressed only when `level == 1 && maxLevel == 1`.**
///   So a level-1 Mending (max 1) reads "Mending" and a level-1 Sharpness
///   (max 5) reads "Sharpness I". Suppressing on `level == 1` alone loses the
///   numeral from every single-level enchant a player actually applies.
/// - **A curse is red**, everything else grey.
/// - **The order is the `minecraft:tooltip_order` tag first**, then whatever
///   the stack carries that the tag does not mention — appended after, in the
///   stack's own order, whatever their ids.
///
/// An id the registry does not contain yields **no line**. That case means the
/// server sent an enchantment this session never synced, and inventing a name
/// for it would be worse than the omission.
pub(crate) fn enchantment_lines(
    enchantments: &[(i32, i32)],
    registry: &[rewo_net::enchantment_parse::EnchantmentDef],
    text: &rewo_data::enchantments::EnchantmentText,
) -> Vec<rewo_gpu::tooltip::Line> {
    let mut rows: Vec<(Option<usize>, usize, String, [f32; 3])> = Vec::new();
    for (order, &(id, level)) in enchantments.iter().enumerate() {
        let Some(def) = usize::try_from(id).ok().and_then(|i| registry.get(i)) else {
            continue;
        };
        // A datapack may name an enchantment with literal text rather than a
        // translation key; translating that would find nothing.
        let name = if def.literal {
            Some(def.description_key.clone())
        } else {
            text.translate(&def.description_key).map(str::to_string)
        };
        let Some(name) = name else { continue };
        let line = if level == 1 && def.max_level == 1 {
            name
        } else {
            format!("{name} {}", text.level(level))
        };
        let color = if text.is_curse(&def.id) {
            CURSE_COLOR
        } else {
            ENCHANT_COLOR
        };
        rows.push((text.tooltip_rank(&def.id), order, line, color));
    }
    // `None` sorts after `Some` for an `Option` key, which is exactly the
    // behaviour wanted here: the tag's members first, in tag order, then the
    // rest in the order the stack listed them.
    rows.sort_by_key(|(rank, order, _, _)| (rank.is_none(), *rank, *order));
    // One span per line here -- an enchantment name is uniformly coloured. The
    // span model earns its keep on lore (italic) and leaves room for the
    // mid-line colour changes vanilla does elsewhere.
    rows.into_iter()
        .map(|(_, _, l, c)| vec![rewo_gpu::tooltip::Span::new(l, c)])
        .collect()
}

/// `ItemContainerContents.addToTooltip` (M66) — the shulker-box preview.
///
/// The line count comes from [`rewo_gpu::tooltip::container_plan`], which is
/// the loop verbatim; what happens here is the translation, and the two keys
/// it needs (`item.container.item_count`, `item.container.more_items`) exist
/// **only after M54's deprecation pass** — `en_us.json` still carries them
/// under their pre-rename `container.shulkerBox.*` names, so a raw read of the
/// language file produces no container lines at all.
///
/// A present stack whose item id this session cannot name drops the **whole**
/// block rather than one line: the remainder is computed from the stack count,
/// so omitting a line silently makes "and 2 more…" wrong as well.
pub(crate) fn container_lines(
    slots: &[Option<rewo_net::item_stack::ContainerSlot>],
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::tooltip::Line> {
    use rewo_gpu::tooltip::Span;
    // `nonEmptyItemsStream` — a gap is a real slot position, and the tooltip
    // is the one consumer that does not care. The filter belongs here rather
    // than in the decode, which is exactly why M63 keeps the gaps.
    let entries: Vec<&rewo_net::item_stack::ContainerSlot> = slots.iter().flatten().collect();
    if entries.is_empty() {
        return Vec::new();
    }
    let (Some(count_key), Some(more_key)) = (
        lang.get("item.container.item_count"),
        lang.get("item.container.more_items"),
    ) else {
        return Vec::new();
    };
    let plan = rewo_gpu::tooltip::container_plan(entries.len());
    let mut out = Vec::new();
    for e in entries.iter().take(plan.shown) {
        let Some(translated) = items.name(e.item_id).and_then(|n| names.get(n)) else {
            return Vec::new();
        };
        out.push(vec![Span::new(
            rewo_data::lang::format(
                count_key,
                &[
                    &e.hover_name(translated, Some(lang)),
                    &e.count.to_string(),
                ],
            ),
            GRAY_TEXT,
        )]);
    }
    if plan.more > 0 {
        // `.withStyle(ChatFormatting.ITALIC)` — expressible since M52b's span
        // model, and rendered as the italic face by the Velvet pass.
        out.push(vec![Span::new(
            rewo_data::lang::format(more_key, &[&plan.more.to_string()]),
            GRAY_TEXT,
        )
        .italic()]);
    }
    out
}

/// `ItemStack.getStyledHoverName()` as one tooltip line (M163).
///
/// ```java
/// MutableComponent n = Component.empty().append(getHoverName())
///                               .withStyle(getRarity().color());
/// if (this.has(DataComponents.CUSTOM_NAME)) n.withStyle(ChatFormatting.ITALIC);
/// ```
/// (`ItemStack.java:829-836`.)
///
/// **Two things here are only expressible because `SlotText` keeps
/// `custom_name` and `item_name` apart.** `getHoverName()` is
/// `getCustomName() ?? getItemName()`, so the value falls back — but the ITALIC
/// keys off `has(CUSTOM_NAME)` alone, so an item whose patch sets only
/// `minecraft:item_name` is renamed and **not** slanted. A merged
/// `custom_name.or(item_name)` field cannot say which one answered.
///
/// And the name is a *component*, resolved here against `lang` rather than at
/// the wire — see [`rewo_world::chat_style::flatten`] on why the decode cannot.
///
/// Extracted from the tooltip closure so a gate can reach it: that closure
/// needs a `PlaySession`, an `Inventory` and four registries, which is M97's
/// rule (a rule with no reachable seam is a rule no witness can grade).
pub(crate) fn styled_hover_name(
    text: Option<&rewo_world::inventory::SlotText>,
    translated: &str,
    color: [f32; 3],
    lang: &rewo_data::lang::Language,
) -> rewo_gpu::tooltip::Line {
    let custom = text.and_then(|t| t.custom_name.as_ref());
    let named = custom.or_else(|| text.and_then(|t| t.item_name.as_ref()));
    let span = rewo_gpu::tooltip::Span::new(
        match named {
            Some(tag) => rewo_world::chat_style::flatten(tag, Some(lang)),
            None => translated.to_string(),
        },
        color,
    );
    vec![if custom.is_some() { span.italic() } else { span }]
}

/// `minecraft:lore`'s lines, in `ItemLore.LORE_STYLE` (M163).
///
/// `Style.EMPTY.withColor(DARK_PURPLE).withItalic(true)`. The colour was always
/// right; the italic had nowhere to live until M52b's span model, and the TEXT
/// was the raw component's literal until M163 — so a lore line carrying a
/// `translate` showed its key and one carrying a legacy colour code showed the
/// code.
pub(crate) fn lore_lines(
    text: &rewo_world::inventory::SlotText,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::tooltip::Line> {
    text.lore
        .iter()
        .map(|line| {
            vec![rewo_gpu::tooltip::Span::new(
                rewo_world::chat_style::flatten(line, Some(lang)),
                LORE_COLOR,
            )
            .italic()]
        })
        .collect()
}

/// `ItemStack.addDetailsToTooltip`'s advanced block, translated (M66).
///
/// The order and the arguments are [`rewo_gpu::tooltip::advanced_lines`]'s;
/// this resolves the two keys and the registry id, which is a **literal** and
/// therefore never goes through the language file.
pub(crate) fn advanced_tooltip_lines(
    lines: &[rewo_gpu::tooltip::AdvancedLine],
    registry_key: &str,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::tooltip::Line> {
    use rewo_gpu::tooltip::{AdvancedLine, Span, DARK_GRAY};
    lines
        .iter()
        .filter_map(|l| match l {
            AdvancedLine::Durability { remaining, max } => {
                let key = lang.get("item.durability")?;
                Some(vec![Span::new(
                    rewo_data::lang::format(key, &[&remaining.to_string(), &max.to_string()]),
                    WHITE_TEXT,
                )])
            }
            AdvancedLine::RegistryId => {
                Some(vec![Span::new(registry_key.to_string(), DARK_GRAY)])
            }
            AdvancedLine::Components { count } => {
                let key = lang.get("item.components")?;
                Some(vec![Span::new(
                    rewo_data::lang::format(key, &[&count.to_string()]),
                    DARK_GRAY,
                )])
            }
        })
        .collect()
}

/// `PatchedDataComponentMap.has(type)` for a decoded stack (M66).
///
/// The prototype answers first and the patch overrides it, which is what
/// separates an addition from an override for
/// [`rewo_net::item_stack::StackComponents::component_count`] — and is also
/// how `isDamageableItem`'s three `has(...)` calls are resolved.
///
/// `None` means unanswerable: either the item is outside this build's
/// prototype table or the component id has no name in the runtime registry.
pub(crate) fn stack_has_component(
    item: &str,
    component: &str,
    detail: Option<&rewo_net::item_stack::StackDetail>,
    registry: Option<&rewo_data::components::DataComponentRegistry>,
) -> Option<bool> {
    let mut has = rewo_data::item_components_table::prototype_has_component(item, component)?;
    if let (Some(d), Some(reg)) = (detail, registry) {
        for &id in &d.added {
            if reg.name_of(id)? == component {
                has = true;
            }
        }
        for &id in &d.removed {
            if reg.name_of(id)? == component {
                has = false;
            }
        }
    }
    Some(has)
}

/// `ChatFormatting.GRAY` — the container's own preview lines.
pub(super) const GRAY_TEXT: [f32; 3] = [170.0 / 255.0, 170.0 / 255.0, 170.0 / 255.0];
/// An unstyled tooltip line, which `GuiGraphics.tooltip` draws in white.
pub(super) const WHITE_TEXT: [f32; 3] = [1.0, 1.0, 1.0];

#[allow(clippy::too_many_arguments)]
pub(crate) fn screen_tooltip(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
    enchant_registry: &[rewo_net::enchantment_parse::EnchantmentDef],
    enchant_text: &rewo_data::enchantments::EnchantmentText,
    details: &rewo_net::item_stack::StackDetails,
    component_registry: Option<&rewo_data::components::DataComponentRegistry>,
    flag: rewo_gpu::tooltip::TooltipFlag,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
    // M93k — the SHOWN menu's layout. Without it this hover centres a
    // 176x166 panel and scans the player's 46 slots, which is the M89 bug in
    // a fourth consumer: the highlight and the icons were made container-aware
    // and this one was not, so with a chest open the tooltip named whatever
    // the PLAYER happened to have at the same index.
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    // `player.isSpectator()` — the fifth of the hint's conditions.
    spectator: bool,
    // M106b — whether the recipe book is open, which MOVES the menu. The panel,
    // its slot icons and its hover highlight all resolve their origin through
    // `Placement::with_book`; this one resolved through the centred form, so
    // with the book up it named a slot 77 GUI px right of the cursor.
    book_open: bool,
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    // A stack on the cursor suppresses the tooltip: vanilla's guard is
    // `hoveredSlot.hasItem() && getCarried().isEmpty()`, so picking something
    // up hides the label of whatever you drag it over.
    if inv.carried().is_some() {
        return None;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_placed(
        mouse,
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            book_open,
        ),
    );
    let slot = layout.slot_at(gx, gy)?;
    // M93k — the crafter's `gui.togglable_slot` hint, which is shown on an
    // EMPTY slot and so must be resolved before the item tooltip's
    // `menu_slot(slot)?` bails out.
    //
    // Vanilla's five conditions here are exactly the preconditions of a PICKUP
    // that would DISABLE the slot, so the hint is DERIVED from the same
    // decision the click uses rather than transcribed a second time — the two
    // then cannot disagree about whether a click would do anything. The
    // string says as much: the constant is named DISABLED_SLOT_TOOLTIP and
    // reads "Click to disable slot", and it appears on an ENABLED slot.
    let crafter_hint: Option<Vec<rewo_gpu::tooltip::Line>> = open.and_then(|m| {
        if !rewo_world::menu::is_crafter_grid_slot(layout.protocol_id, slot as i32) {
            return None;
        }
        let would_disable = rewo_world::menu::crafter_toggle(
            rewo_world::inventory::CONTAINER_INPUT_PICKUP,
            m.crafter_slot_disabled(slot as i32),
            inv.menu_slot(slot).is_some(),
            spectator,
            // Already known empty — the guard at the top of this function
            // returns early otherwise — but passed rather than hard-coded, so
            // the two guards cannot drift apart.
            inv.carried().is_none(),
            false,
        ) == rewo_world::menu::CrafterToggle::Disable;
        if !would_disable {
            return None;
        }
        lang.get("gui.togglable_slot")
            .map(|t| vec![vec![rewo_gpu::tooltip::Span::new(t.to_string(), [1.0, 1.0, 1.0])]])
    });
    // The item tooltip's content. A closure so the crafter's hint can
    // select between the two and share the assembly below — the measure,
    // position and glyph-run code is the same for any tooltip, and a second
    // copy of it is how two tooltips come to sit in different places.
    let item_lines = || -> Option<Vec<rewo_gpu::tooltip::Line>> {
        let stack = inv.menu_slot(slot)?;
        let item_name = items.name(stack.item_id)?;
        let translated = names.get(item_name)?;

        // `getTooltipLines` in vanilla's order: the styled hover name, then the
        // details each component contributes. Rewo produces the three it can read
        // exactly — the name, the lore, and the `Unbreakable` marker.
        //
        let text = inv.text_of(stack);
        // A line is a sequence of styled spans (M52b), not a string and a colour.
        // Vanilla styles per run, and the old model was strictly less: it could
        // not say "italic", so `ItemLore.LORE_STYLE`'s italic was silently
        // dropped. Geometry is unchanged -- the box is still measured from the
        // plain text with the vanilla advances.
        let mut lines: Vec<rewo_gpu::tooltip::Line> = Vec::new();
        lines.push(styled_hover_name(
            text,
            translated,
            rarity_color(stack_rarity(
                Some(item_name),
                text.and_then(|t| t.rarity),
                text.is_some_and(|t| t.is_enchanted),
            )),
            lang,
        ));
        if let Some(t) = text {
            // Vanilla's order: the enchantments come before the lore.
            lines.extend(enchantment_lines(
                &t.enchantments,
                enchant_registry,
                enchant_text,
            ));
            lines.extend(lore_lines(t, lang));
            if t.unbreakable {
                lines.push(vec![rewo_gpu::tooltip::Span::new(
                    "Unbreakable".to_string(),
                    UNBREAKABLE_COLOR,
                )]);
            }
        }
        // M66 stage 4: `minecraft:container`'s preview. Vanilla's component
        // tooltips run in `DataComponents` registration order and `container`
        // comes after the lore block, which is where this sits.
        let detail = details.get(stack.components);
        if let Some(d) = detail {
            lines.extend(container_lines(&d.container, items, names, lang));
        }
        // M66 stage 3: the advanced block, last of everything a component adds.
        {
            let has = |c: &str| stack_has_component(item_name, c, detail, component_registry);
            let durability = rewo_gpu::tooltip::DurabilityState {
                damage: stack.damage.unwrap_or(0),
                max: stack
                    .max_damage
                    .or_else(|| rewo_data::item_props_table::max_damage(item_name))
                    .unwrap_or(0),
                has_max_damage: has("minecraft:max_damage").unwrap_or(false),
                has_damage: has("minecraft:damage").unwrap_or(false),
                unbreakable: has("minecraft:unbreakable").unwrap_or(false),
            };
            // `PatchedDataComponentMap.size()`. Both halves can decline — an item
            // outside the prototype table, or a component id with no name — and
            // either way the line is dropped rather than guessed.
            let count = match detail {
                Some(d) => rewo_net::item_stack::StackComponents {
                    added: d.added.clone(),
                    removed: d.removed.clone(),
                    ..Default::default()
                }
                .component_count(
                    || rewo_data::item_components_table::prototype_component_count(item_name),
                    |id| {
                        let name = component_registry?.name_of(id)?;
                        rewo_data::item_components_table::prototype_has_component(item_name, name)
                    },
                ),
                // No patch at all: the merged size is the prototype's.
                None => rewo_data::item_components_table::prototype_component_count(item_name),
            };
            // `display.shows(DataComponents.DAMAGE)` — Rewo does not interpret
            // `minecraft:tooltip_display`, so this is `TooltipDisplay.DEFAULT`,
            // which shows everything. A server that hid the damage line would
            // still see it here.
            let advanced = rewo_gpu::tooltip::advanced_lines(flag, durability, true, count);
            lines.extend(advanced_tooltip_lines(&advanced, item_name, lang));
        }

        Some(lines)
    };
    let lines = match crafter_hint {
        Some(l) => l,
        None => item_lines()?,
    };
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// The tooltip of the recipe cell under the cursor (M106).
///
/// `RecipeBookPage.extractTooltip` — `Screen.getTooltipFromItem(displayStack)`
/// plus a "Right Click for More" line when the collection holds more than one
/// recipe.
///
/// **This loses to the menu's own tooltip, and that is not the order it is
/// called in.** `AbstractRecipeBookScreen` runs `this.extractTooltip` (the
/// container's) and *then* `recipeBookComponent.extractTooltip`, which reads as
/// "the book overwrites it" — and `setTooltipForNextFrameInternal`'s body is
/// `if (this.deferredTooltip == null || replaceExisting)`, with
/// `replaceExisting` false on every path either of them takes. So the FIRST
/// tooltip of a frame wins and the book's is discarded. Hence the caller's
/// `screen_tooltip(..).or_else(..)` rather than the reverse.
///
/// For a page cell the two can never both fire — the book sits beside the menu
/// on a wide window, and on a narrow one `AbstractRecipeBookScreen.isHovering`
/// returns false for every slot, so there is no hovered slot to describe. It is
/// the GHOST tooltip that makes first-wins observable; see [`ghost_tooltip`].
///
/// The stack is Rewo's `slot_items` id, which carries no components (M95): a
/// recipe result that shipped with a custom name or lore would show its plain
/// name here. The advanced block is still produced, because F3+H adds the
/// registry id to a book cell in vanilla too.
#[allow(clippy::too_many_arguments)]
pub(crate) fn book_tooltip(
    b: &BookRender,
    overlay_open: bool,
    book_mouse: Option<(i32, i32)>,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
    flag: rewo_gpu::tooltip::TooltipFlag,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    use rewo_world::recipe_book_screen as rb;
    let view = b.view?;
    let (bx, by) = book_mouse?;
    let slot = rb::page_tooltip_slot(rb::book_hit(bx, by, view, view.tabs), overlay_open)?;
    // `getDisplayStack()` — the item the cycle is currently on, which is
    // already what `slot_items` holds. A collection whose result needs a
    // context Rewo cannot build has `None` and shows nothing, rather than a
    // tooltip for some other member of the group.
    let item = (*b.slot_items.get(slot)?)?;
    let item_name = items.name(item)?;
    let mut lines: Vec<rewo_gpu::tooltip::Line> = vec![vec![rewo_gpu::tooltip::Span::new(
        names.get(item_name)?.to_string(),
        rarity_color(stack_rarity(Some(item_name), None, false)),
    )]];
    {
        let has = |c: &str| stack_has_component(item_name, c, None, None);
        let durability = rewo_gpu::tooltip::DurabilityState {
            // A recipe result is undamaged by construction: the display stack
            // comes from the recipe, not from an inventory.
            damage: 0,
            max: rewo_data::item_props_table::max_damage(item_name).unwrap_or(0),
            has_max_damage: has("minecraft:max_damage").unwrap_or(false),
            has_damage: has("minecraft:damage").unwrap_or(false),
            unbreakable: has("minecraft:unbreakable").unwrap_or(false),
        };
        let advanced = rewo_gpu::tooltip::advanced_lines(
            flag,
            durability,
            true,
            rewo_data::item_components_table::prototype_component_count(item_name),
        );
        lines.extend(advanced_tooltip_lines(&advanced, item_name, lang));
    }
    // `if (this.hasMultipleRecipes()) texts.add(MORE_RECIPES_TOOLTIP)` — LAST,
    // after everything the item contributed, including the advanced block.
    if b.slots.get(slot).is_some_and(|&(_, multiple)| multiple) {
        lines.push(vec![rewo_gpu::tooltip::Span::new(
            lang.or_key(rb::MORE_RECIPES_KEY).to_string(),
            [1.0, 1.0, 1.0],
        )]);
    }
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// Turn a resolved set of tooltip lines into its box, position and glyph runs
/// (M106).
///
/// Extracted from [`screen_tooltip`] unchanged so the recipe book's own
/// tooltips can share it. The comment it was extracted from already said why —
/// "the measure, position and glyph-run code is the same for any tooltip, and a
/// second copy of it is how two tooltips come to sit in different places" — and
/// M106 is the second producer that makes it true rather than prospective.
pub(super) fn tooltip_layout(
    lines: Vec<rewo_gpu::tooltip::Line>,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    // `if (!lines.isEmpty())` in `setTooltipForNextFrameInternal` — an empty
    // list sets no tooltip at all, which is not the same as an empty box.
    if lines.is_empty() {
        return None;
    }
    // Measure with the font that will DRAW. Once the tooltip renders in
    // Newsreader, sizing it with the bitmap advances measures a font it no
    // longer uses -- which shows up as text spilling out of its box, and is
    // the half of this flip that is easiest to skip.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let mut glyphs = glyphs;
    let widths: Vec<i32> = match glyphs.as_deref_mut() {
        Some(cache) => lines
            .iter()
            .map(|l| velvet_line_width(cache, l, scale).ceil() as i32)
            .collect(),
        None => lines
            .iter()
            .map(|l| {
                rewo_data::sign_text::width(&rewo_gpu::tooltip::line_text(l), advance).round()
                    as i32
            })
            .collect(),
    };
    let (tw, th) = rewo_gpu::container::tooltip_size(&widths);
    // The positioner works in GUI pixels, and so does the screen size it
    // clamps against — `guiWidth()`/`guiHeight()` are the *scaled* dimensions,
    // not the framebuffer's. Passing raw pixels here would let a tooltip run
    // off the right of a small window before the flip ever triggered.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let (sw, sh) = ((w / scale) as i32, (h / scale) as i32);
    let (mx, my) = ((mouse.0 / scale as f64) as i32, (mouse.1 / scale as f64) as i32);
    let (tx, ty) = rewo_gpu::container::tooltip_position(sw, sh, mx, my, tw, th);
    // `localY += line.getHeight(font) + (i == 0 ? 2 : 0)` — the gap goes after
    // the **first** line only, which is what separates the name from the
    // details without spacing the details apart.
    let mut y = ty;
    // With the cache present the text goes through the Velvet pass, which is
    // what makes italic lore actually slant. The bitmap path stays as the
    // fallback for a build with no fonts on disk.
    if let Some(cache) = glyphs.as_deref_mut() {
        let mut runs: Vec<rewo_gpu::velvet_text::OwnedRun> = Vec::new();
        let mut ly = ty;
        for (i, spans) in lines.iter().enumerate() {
            // Vanilla's `y` is the line's TOP; Velvet lays out from the
            // BASELINE. Dropping the ascent would raise every tooltip line by
            // most of its own height.
            let ascent = cache
                .metrics(tooltip_key(false, scale))
                .map(|m| m.ascent)
                .unwrap_or(TOOLTIP_TEXT_GUI_PX * scale * 0.75);
            let baseline = ly as f32 * scale + ascent;
            let mut pen = tx as f32 * scale;
            for sp in spans {
                let key = tooltip_key(sp.italic, scale);
                let mut g = Vec::new();
                let adv = cache.layout_run(key, &sp.text, 0.0, (pen, baseline), &mut g);
                pen += adv;
                if !g.is_empty() {
                    runs.push(rewo_gpu::velvet_text::OwnedRun {
                        glyphs: g,
                        color: sp.color,
                        alpha: 1.0,
                    });
                }
            }
            ly += rewo_gpu::container::TOOLTIP_LINE_HEIGHT + if i == 0 { 2 } else { 0 };
        }
        return Some((
            rewo_gpu::container::TooltipDraw {
                pos: (tx, ty),
                size: (tw, th),
                bundle: None,
            },
            Vec::new(),
            runs,
        ));
    }
    let out = lines
        .into_iter()
        .enumerate()
        .map(|(i, spans)| {
            // The bitmap pass takes one colour per line, so the vanilla-font
            // path still draws the first span's. That is a property of THAT
            // pass, not of the model: the styled line already carries every
            // span, and `tooltip::to_velvet_spans` renders it in full through
            // the Velvet type stack. Switching which pass a tooltip uses is a
            // rendering decision, deliberately left open while the HUD's
            // visual direction settles.
            let color = spans.first().map(|s| s.color).unwrap_or([1.0; 3]);
            let line = rewo_gpu::world::OwnedTextLine {
                x: tx as f32 * scale,
                y: y as f32 * scale,
                px: scale,
                color_linear: srgb_bytes_to_linear_f(color),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: rewo_gpu::tooltip::line_text(&spans),
            };
            y += rewo_gpu::container::TOOLTIP_LINE_HEIGHT + if i == 0 { 2 } else { 0 };
            line
        })
        .collect();
    Some((
        rewo_gpu::container::TooltipDraw {
            pos: (tx, ty),
            size: (tw, th),
            // **The decode exists; the carrier does not.** M61 made the patch
            // reader *keep* `minecraft:bundle_contents` — see
            // `rewo_net::item_stack::StackComponents::bundle_contents`, which
            // returns the stacks as `ItemTemplate`s. What is still missing is
            // a way to get them from there to here.
            //
            // The tooltip reads its stack out of `rewo_world`'s `Inventory`,
            // and neither of that crate's two carriers takes a `Vec` today:
            // `ItemSlot` is `Copy` on purpose (the click arithmetic moves it
            // through a dozen struct-update expressions, and growing it would
            // make every one of those a clone), and `SlotText` — the non-`Copy`
            // side-channel keyed by the component fingerprint — is the natural
            // home but would need its `is_empty` taught the new field, or a
            // bundle carrying *only* `bundle_contents` would be recorded as
            // having no text and dropped. That is the exact bug M42's
            // enchantments hit.
            //
            // Left `None` rather than guessed: `container::bundle_chrome` and
            // `tooltip::bundle_image` are both graded by `inventoryshot`
            // against synthetic bundles, so an empty grid drawn from a full
            // bundle would be a confident wrong answer with a green gate
            // behind it.
            bundle: None,
        },
        out,
        Vec::new(),
    ))
}

pub(crate) fn screen_icons(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    w: f32,
    h: f32,
    // M93j — the open menu, for the slots whose render it REPLACES. `None` is
    // the player's own inventory, which replaces none.
    open: Option<&rewo_world::menu::OpenMenu>,
    // M93s — the stonecutter's recipe grid, whose buttons draw item icons that
    // belong to no slot.
    cut: Option<&CutView>,
    // M93u — and the merchant's trade rows, likewise.
    merchant: Option<&MerchantView>,
    // M95 — the recipe book, whose tab icons and recipe results are items.
    // Its presence also MOVES the menu, so it moves every icon measured from
    // the menu's origin too.
    book: Option<&BookRender>,
    // M103 — the ghost recipe's slots, and the cycle index its items rotate on.
    ghosts: &[rewo_world::ghost_slots::Ghost],
    cycle: i32,
    // M104 — the open which-of-these overlay, whose ingredient grids are items
    // too. Supplied rather than reached for, so a gate drives the same call
    // the live client makes (M45).
    overlay: Option<&rewo_world::recipe_overlay::Open>,
) -> (Vec<rewo_gpu::gui_item::GuiItem>, Vec<rewo_gpu::world::OwnedTextLine>) {
    let rects = menu_slot_rects(inv, w, h, book.is_some());
    let (_, _, scale) = rewo_gpu::container::gui_origin(w, h);
    let mut icons = Vec::new();
    let mut labels = Vec::new();
    for (slot, rect) in rects.iter().enumerate() {
        // A slot the screen covers draws neither its icon nor its count —
        // `extractSlot` never reaches `super`, and the count is part of what
        // super draws.
        if open.is_some_and(|m| m.slot_hides_item(slot)) {
            continue;
        }
        if let Some(stack) = inv.menu_slot(slot) {
            if let Some(icon) = icon_for(items, trim_materials, stack, rect.0, rect.1, rect.2) {
                icons.push(icon);
            }
            labels.extend(count_label(stack, rect.0, rect.1, scale));
        }
    }
    // M103 — the ghost recipe's items, in the menu's own slots. Between the two
    // washes by construction: the red is in the panel's overlays (back half) and
    // the white in its front overlays, and the icon pass runs between them.
    //
    // **No count label** except on the result, which is `itemDecorations`'
    // own rule — an input ghost never shows a number even when the recipe wants
    // several of that ingredient.
    for g in ghosts {
        let Some(id) = g.item(cycle) else { continue };
        let Some((sx, sy)) = inv.layout().position(g.slot) else { continue };
        let (left, top, _) = rewo_gpu::container::gui_origin_placed(
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                inv.layout().image_w as f32,
                inv.layout().image_h as f32,
                book.is_some(),
            ),
        );
        if let Some(icon) = icon_for(
            items,
            trim_materials,
            rewo_world::inventory::ItemSlot::plain(id, 1),
            left + sx as f32 * scale,
            top + sy as f32 * scale,
            16.0 * scale,
        ) {
            icons.push(icon);
        }
    }
    // M95 — the recipe book's items: the tab icons and the page's results.
    //
    // On the BOOK's origin, not the menu's — the book is window-anchored and
    // the gap between the two alternates with the window's parity (M94).
    //
    // **No count label**, for the stonecutter's reason one screen over: both
    // `fakeItem` and `graphics.item` draw the model alone, and
    // `itemDecorations` is a separate call neither makes. A recipe yielding 8
    // torches shows no "8".
    if let Some(b) = book.and_then(|b| b.view.map(|v| (b, v))) {
        let (bk, view) = b;
        let (bl, bt, _) = rewo_gpu::container::recipe_book_origin(w, h);
        let tabs = bk.book.tabs();
        for icon in rewo_world::recipe_book_screen::book_icons(view, tabs, &bk.slot_shadowed) {
            use rewo_world::recipe_book_screen::BookIconKind as K;
            let id = match icon.kind {
                K::TabPrimary(i) => tabs.get(i).and_then(|t| items.id(t.primary)),
                K::TabSecondary(i) => tabs.get(i).and_then(|t| t.secondary).and_then(|n| items.id(n)),
                // The SAME item for both copies: the shadow is the display
                // stack drawn twice, not a second recipe's result.
                K::Slot { index, .. } => bk.slot_items.get(index).copied().flatten(),
            };
            let Some(id) = id else { continue };
            if let Some(g) = icon_for(
                items,
                trim_materials,
                rewo_world::inventory::ItemSlot::plain(id, 1),
                bl + icon.x as f32 * scale,
                bt + icon.y as f32 * scale,
                16.0 * scale,
            ) {
                icons.push(g);
            }
        }
    }
    // M104 — the which-of-these overlay's ingredient grids. On the BOOK's
    // origin like the cells above, and drawn after them because the overlay is
    // a stratum over the whole book.
    //
    // **6 px, not 16**: `scale(0.375F)` sits between two translates, so the
    // trailing `-8, -8` is scaled with it and `Pos` is the ingredient's CENTRE.
    // `item_rect` is where that composition lives.
    //
    // No count label, for the same reason the cells have none: `graphics.item`
    // draws the model alone.
    if let Some(o) = overlay {
        let (bl, bt, _) = rewo_gpu::container::recipe_book_origin(w, h);
        for (i, b) in o.buttons.iter().enumerate() {
            let button = rewo_world::recipe_overlay::button_origin(o.origin, i, o.total());
            for (pos, choices) in &b.slots {
                // One-level cycle, on the same clock the cells use — so a cell
                // and the overlay it opened can be showing different items on
                // the same frame.
                let Some(n) = rewo_world::recipe_overlay::select_ingredient(choices.len(), cycle)
                else {
                    continue;
                };
                let (ix, iy, size) = rewo_world::recipe_overlay::item_rect(button, *pos);
                if let Some(g) = icon_for(
                    items,
                    trim_materials,
                    rewo_world::inventory::ItemSlot::plain(choices[n], 1),
                    bl + ix * scale,
                    bt + iy * scale,
                    size * scale,
                ) {
                    icons.push(g);
                }
            }
        }
    }
    // M93s — the stonecutter's recipe buttons, which are ITEMS and so belong
    // here rather than in the overlay atlas.
    //
    // **No count label.** `extractRecipes` calls `graphics.item`, which draws
    // the model alone; `itemDecorations` is a separate call it never makes. So
    // a slab recipe yielding 2 shows no "2", unlike every real slot above —
    // reusing `count_label` here would be the natural thing and would be wrong
    // on 124 of the 319 recipes.
    if let Some(c) = cut.filter(|c| c.display) {
        let (left, top, _) = rewo_gpu::container::gui_origin_for(
            w,
            h,
            open.map_or(176.0, |m| m.layout.image_w as f32),
            open.map_or(166.0, |m| m.layout.image_h as f32),
        );
        for pos_index in 0..rewo_world::menu_screen::CUT_PAGE {
            let index = c.start_index + pos_index;
            let Some(recipe) = usize::try_from(index).ok().and_then(|i| c.recipes.get(i)) else {
                break;
            };
            let Some(id) = items.id(recipe.result) else { continue };
            let (gx, gy) = rewo_world::menu_screen::cut_cell_origin(pos_index);
            if let Some(icon) = icon_for(
                items,
                trim_materials,
                rewo_world::inventory::ItemSlot::plain(id, recipe.count as i32),
                left + gx as f32 * scale,
                top + gy as f32 * scale,
                16.0 * scale,
            ) {
                icons.push(icon);
            }
        }
    }
    // M93u — the merchant's three items per visible row. Like the
    // stonecutter's grid these are items rather than sprites, and unlike it
    // they DO carry counts: `extractOffers` calls `itemDecorations` for cost B
    // and the result, and `extractAndDecorateCostA` for cost A.
    if let Some(v) = merchant {
        use rewo_world::merchant_screen as ms;
        let (left, top, scale) = rewo_gpu::container::gui_origin_for(
            w,
            h,
            open.map_or(176.0, |m| m.layout.image_w as f32),
            open.map_or(166.0, |m| m.layout.image_h as f32),
        );
        let n = v.offers.len();
        for (i, offer) in v.offers.iter().enumerate() {
            let idx = i as i32;
            if !ms::offer_visible(idx, v.scroll_off, n) {
                continue;
            }
            let row = if ms::can_scroll(n) { idx - v.scroll_off } else { idx };
            let y = ms::row_item_y(row);
            // Returns rather than pushes, so the cost-A branch below can use
            // the same geometry without a second mutable borrow.
            let at = |gx: i32, id: i32, count: i32| {
                let (px, py) = (left + gx as f32 * scale, top + y as f32 * scale);
                (
                    icon_for(
                        items,
                        trim_materials,
                        rewo_world::inventory::ItemSlot::plain(id, count),
                        px,
                        py,
                        16.0 * scale,
                    ),
                    count_label(
                        rewo_world::inventory::ItemSlot::plain(id, count),
                        px,
                        py,
                        scale,
                    ),
                )
            };
            // Cost A: ONE icon, at its MODIFIED count — `fakeItem` is called
            // once, outside `extractAndDecorateCostA`'s branch. The discounted
            // display is two NUMBERS over a single item, not two items.
            let modified = v.cost_a_counts[i];
            let disp = ms::cost_a_display(offer.cost_a.count, modified);
            //
            // Only `.0`, the icon: cost A's DIGITS take the branch below.
            // `icon_for` draws the item's model and ignores the count, so
            // which count is passed here is inert — mutating it to the base
            // cost is an equivalent mutant, and it is spelled `modified`
            // because that is the stack vanilla passes to `fakeItem`.
            icons.extend(at(ms::COST_A_X, offer.cost_a.item_id, modified).0);
            // Both digits are FORCED in the discounted branch, including a 1 —
            // `count == 1 ? "1" : null` exists to defeat `itemCount`'s own
            // "a single item shows no digit" rule.
            let forced = |n: i32| (n == 1).then(|| n.to_string());
            for (gx, count) in [
                (ms::COST_A_X, disp.at_icon),
                (ms::COST_A_X + ms::DISCOUNT_SECOND_X, disp.at_second),
            ] {
                let Some(n) = count else { continue };
                labels.extend(count_label_of(
                    n,
                    disp.strikethrough.then(|| forced(n)).flatten(),
                    left + gx as f32 * scale,
                    top + y as f32 * scale,
                    scale,
                ));
            }
            // Cost B and the result take the ordinary path: `itemDecorations`
            // with no `countText`, so a single item shows no digit there.
            for (gx, id, count) in [
                offer.cost_b.as_ref().map(|b| (ms::COST_B_X, b.item_id, b.count)),
                match &offer.result {
                    rewo_net::item_stack::WireSlot::Stack(st) => {
                        Some((ms::RESULT_X, st.item_id, st.count))
                    }
                    rewo_net::item_stack::WireSlot::Empty => None,
                },
            ]
            .into_iter()
            .flatten()
            {
                let (icon, label) = at(gx, id, count);
                icons.extend(icon);
                labels.extend(label);
            }
        }
    }
    (icons, labels)
}

pub(super) fn icon_for(
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    stack: rewo_world::inventory::ItemSlot,
    x: f32,
    y: f32,
    size: f32,
) -> Option<rewo_gpu::gui_item::GuiItem> {
    let base = items.name(stack.item_id)?;
    // M49: a trimmed stack asks for its variant. `HeldItems::any` falls back to
    // the plain item when the bake has no such variant, so composing here is
    // always safe.
    let model = match stack
        .trim_material
        .and_then(|m| trim_materials.get(m as usize))
    {
        Some(m) => format!("{base}#{}", m.id),
        None => base.to_string(),
    };
    Some(rewo_gpu::gui_item::GuiItem {
        model,
        x,
        y,
        size,
        // `ItemStack.hasFoil()` (M43). Every slot icon goes through this one
        // constructor, so the hotbar, the screen and the cursor stack all get
        // the glint from the same place.
        glint: stack.enchanted,
    })
}

/// `GuiGraphicsExtractor.itemCount` — bottom-right of the slot, and **only
/// when the count is not one**, which is why a single sword shows no label.
pub(super) fn count_label(
    stack: rewo_world::inventory::ItemSlot,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    count_label_of(stack.count, None, x, y, scale)
}

/// `itemCount` with vanilla's `countText` override (M93w).
///
/// ```java
/// if (itemStack.getCount() != 1 || countText != null) {
///    String amount = countText == null ? String.valueOf(itemStack.getCount()) : countText;
/// ```
///
/// **The override's only job is to defeat the `!= 1` rule.** A single item
/// normally shows no digit; the merchant's discounted price passes
/// `count == 1 ? "1" : null` so both halves of the comparison stay visible,
/// which matters exactly when a discount has reached 1.
pub(super) fn count_label_of(
    count: i32,
    force: Option<String>,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    if count == 1 && force.is_none() {
        return None;
    }
    let text = force.unwrap_or_else(|| count.to_string());
    // Vanilla measures the string; the digits are a uniform 6 px including
    // their one-pixel gap, and the trailing gap is not part of the width.
    let width = text.chars().count() as f32 * 6.0 - 1.0;
    Some(rewo_gpu::world::OwnedTextLine {
        // `x + 19 - 2 - width`, `y + 6 + 3`.
        x: x + (17.0 - width) * scale,
        y: y + 9.0 * scale,
        px: scale,
        color_linear: [1.0, 1.0, 1.0],
        alpha: 1.0,
        shadow: true,
        style: rewo_gpu::text::TextStyle::PLAIN,
        text,
    })
}

/// The enchanting table's three cost numerals (M92).
///
/// The first text a container screen draws that is not a stack count, and the
/// alignment is why it needs the real advance table rather than a 6-px-per-
/// digit estimate: `leftPosText + 86 - font.width(costText)` is **right**-
/// aligned, so a wrong width moves a two-digit cost and leaves a one-digit one
/// looking correct.
///
/// An empty row draws nothing at all — `cost == 0` returns before the numeral,
/// the name and the cost, so a table with no item shows three blank rows.
pub(super) fn enchant_cost_labels(
    rows: [rewo_world::menu_screen::EnchantRow; 3],
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    let layout = &rewo_world::menu_layout::REGISTRY[13]; // enchantment
    let (left, top, scale) =
        rewo_gpu::container::gui_origin_for(w, h, layout.image_w as f32, layout.image_h as f32);
    let mut out = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        let (Some(cost), Some(rgb)) = (row.cost(), row.cost_color()) else {
            continue;
        };
        let text = cost.to_string();
        let (x, y) =
            rewo_world::menu_screen::enchant_cost_pos(i, rewo_gpu::text::width(&text, advance));
        out.push(rewo_gpu::world::OwnedTextLine {
            x: left + x as f32 * scale,
            y: top + y as f32 * scale,
            px: scale,
            color_linear: srgb_bytes_to_linear(rgb),
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text,
        });
    }
    out
}

/// The anvil name field's geometry, from `AnvilScreen.subInit` (M93t).
///
/// ```java
/// this.name = new EditBox(this.font, xo + 62, yo + 24, 103, 12, …);
/// this.name.setBordered(false);
/// ```
///
/// Unbordered, so `textX = getX() + 0` and `textY = getY()` — a bordered box
/// would inset by 4 and centre vertically by `(height - 8) / 2`, and reusing
/// those here would put the name four pixels right and two down.
/// `getInnerWidth()` is likewise the full 103 rather than `width - 8`.
pub(super) const ANVIL_FIELD: (i32, i32, i32) = (62, 24, 103);

/// The name field's text, cursor and selection (M93t).
///
/// The two-piece draw in `extractWidgetRenderState` exists to *place the
/// cursor*, not to change the text: the halves are separated by `+1` and then
/// pulled back by `-1` for an insert cursor, so the run is contiguous either
/// way and one label is exact.
pub(super) fn anvil_field_render(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    let layout = &rewo_world::menu_layout::REGISTRY[ANVIL_MENU_PROTOCOL_ID as usize];
    let (left, top, scale) =
        rewo_gpu::container::gui_origin_for(w, h, layout.image_w as f32, layout.image_h as f32);
    let (fx, fy, inner) = ANVIL_FIELD;
    edit_box_render(local, advance, (left, top, scale), (fx, fy, inner), None, now_ms)
}

/// One `EditBox`'s text, caret and selection — shared by the anvil's name field
/// (M93t) and the recipe book's search (M100).
///
/// Extracted rather than copied: the caret's x is the width of the run before
/// it, the `insert` rule decides whether the caret is a bar or an underscore,
/// and the selection's rect is clamped against the inner width. A second copy
/// of that is three chances to drift, and the drift would be a caret one pixel
/// out — invisible in review and obvious in use.
///
/// `origin` is the surface's `(left, top, scale)` in screen pixels; `field` is
/// `(x, y, inner_width)` in that surface's own coordinates. `hint` is the text
/// drawn when the field is **empty and unfocused**, with its own colour.
pub(super) fn edit_box_render(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    (left, top, scale): (f32, f32, f32),
    (fx, fy, inner): (i32, i32, i32),
    hint: Option<(&str, [f32; 3])>,
    // Wall-clock milliseconds, for the caret's blink phase (M101).
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    let width = |u: &[u16]| rewo_gpu::text::width(&String::from_utf16_lossy(u), advance);
    let displayed = local.displayed(inner, &width).to_vec();
    let rel_cursor = local.cursor_position().saturating_sub(local.display_pos());
    let on_screen = rel_cursor <= displayed.len();

    let mut labels = Vec::new();
    let mut fills = Vec::new();
    let mut append = None;
    let text = String::from_utf16_lossy(&displayed);
    if !text.is_empty() {
        labels.push(rewo_gpu::world::OwnedTextLine {
            x: left + fx as f32 * scale,
            y: top + fy as f32 * scale,
            px: scale,
            // `setTextColor(-1)` AND `setTextColorUneditable(-1)` — the anvil
            // sets both to white, so an uneditable field is not greyed.
            color_linear: [1.0, 1.0, 1.0],
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text,
        });
    }

    // `if (hint != null && displayed.isEmpty() && !isFocused())` — the hint
    // goes when the field takes FOCUS, not when the first character arrives.
    // Clicking an empty search box therefore blanks "Search..." before you
    // type, which reads as a bug until you check the guard.
    if let Some((text, color)) = hint {
        if displayed.is_empty() && !local.is_focused() {
            labels.push(rewo_gpu::world::OwnedTextLine {
                x: left + fx as f32 * scale,
                y: top + fy as f32 * scale,
                px: scale,
                // The hint is a styled COMPONENT, so its own colour wins over
                // the field's — `SEARCH_HINT_STYLE` is gray, where the book
                // sets the field itself to white. The constant is vanilla's
                // byte `/255`, so it converts here.
                color_linear: srgb_bytes_to_linear_f(color),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: text.to_string(),
            });
        }
    }

    // `insert = cursorPos < value.length() || value.length() >= maxLength` —
    // so a full field shows the BAR even with the cursor at the end, which is
    // how vanilla tells you there is no room left.
    let insert = local.cursor_position() < local.len() || local.len() >= local.max_length();
    let before = if on_screen { &displayed[..rel_cursor] } else { &displayed[..] };
    let mut cursor_x = fx + width(before) + if before.is_empty() { 0 } else { 1 };
    if on_screen && insert {
        cursor_x -= 1;
    }

    // The selection: `textHighlight(min(cursorX, x+width), textY-1,
    // min(highlightX-1, x+width), textY+1+9, invert)`. The anvil sets
    // `setInvertHighlightedTextColor(false)`, so only the blue fill runs.
    let rel_highlight = local
        .highlight_position()
        .saturating_sub(local.display_pos())
        .min(displayed.len());
    if rel_highlight != rel_cursor {
        let hx = fx + width(&displayed[..rel_highlight]);
        let (x0, x1) = (cursor_x.min(fx + inner), (hx - 1).min(fx + inner));
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        fills.push((
            rewo_gpu::container::FILL_SPRITE,
            rewo_gpu::container::PanelBlit {
                dx: x0 as f32,
                dy: (fy - 1) as f32,
                w: (x1 - x0) as f32,
                h: 11.0,
                sx: 0.0,
                sy: 0.0,
                sw: 0.0,
                sh: 0.0,
                // `-16776961` = 0xFF0000FF. NOTE the pipeline is
                // `GUI_TEXT_HIGHLIGHT`, whose blend Rewo's single container
                // blend does not reproduce — the colour is exact, the
                // compositing is a plain alpha draw.
                tint: [0.0, 0.0, 1.0, 1.0],
            },
        ));
    }

    // `showCursor = isFocused() && isCursorVisible(millis - focusedTime) &&
    // cursorOnScreen` — THREE conditions. M93t had only the first, so the
    // anvil's caret was solid and drawn even when scrolled out of view.
    if local.is_focused() && local.cursor_visible(now_ms) && on_screen {
        if insert {
            // `fill(x, y - 1, x + 1, y + lineHeight)`, lineHeight 9 + 1.
            fills.push((
                rewo_gpu::container::FILL_SPRITE,
                rewo_gpu::container::PanelBlit {
                    dx: cursor_x as f32,
                    dy: (fy - 1) as f32,
                    w: 1.0,
                    h: 11.0,
                    sx: 0.0,
                    sy: 0.0,
                    sw: 0.0,
                    sh: 0.0,
                    tint: [1.0, 1.0, 1.0, 1.0],
                },
            ));
        } else {
            // The append cursor is the CHARACTER "_", not a rectangle.
            append = Some(String::from("_"));
            labels.push(rewo_gpu::world::OwnedTextLine {
                x: left + cursor_x as f32 * scale,
                y: top + fy as f32 * scale,
                px: scale,
                color_linear: [1.0, 1.0, 1.0],
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: "_".into(),
            });
        }
    }
    (labels, fills, append)
}

/// [`anvil_field_render`] for `containershot`.
///
/// The clock is fixed at **0** (M101): a caret that blinks would make the same
/// scene render two ways depending on when the gate ran, and a witness cannot
/// hold that constant. At 0 the caret is visible, which is the state the
/// existing anvil witnesses were written against.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn anvil_field_render_for_test(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    anvil_field_render(local, advance, w, h, 0)
}

/// `AbstractContainerScreen.extractCarriedItem` — the cursor stack, offset by
/// half a slot so it sits under the pointer rather than beside it.
pub(super) fn carried_icon(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    mouse: (f64, f64),
    w: f32,
    h: f32,
) -> Option<(
    rewo_gpu::gui_item::GuiItem,
    Option<rewo_gpu::world::OwnedTextLine>,
)> {
    let stack = inv.carried()?;
    let (_, _, scale) = rewo_gpu::container::gui_origin(w, h);
    let (x, y) = (
        mouse.0 as f32 - 8.0 * scale,
        mouse.1 as f32 - 8.0 * scale,
    );
    let icon = icon_for(items, trim_materials, stack, x, y, 16.0 * scale)?;
    Some((icon, count_label(stack, x, y, scale)))
}

/// Resolve an item id into the facts the click arithmetic needs.
///
/// `None` for an id the registry does not contain, which makes the whole click
/// decline rather than predicting against a guessed stack cap.
///
/// `pub(crate)` so `containershot` can grade **this** function rather than its
/// own copy. M92's finding is the reason: a gate that constructs the input
/// production must derive leaves the derivation untested by construction —
/// there, five `mob_effect` ids were read from a `registry_data` branch that
/// cannot fire, and `lightmapshot`/`swingshot` could not see it because both
/// supplied the ids themselves. Every unit test of the quick-move hand-builds
/// an `ItemProps`, so without a witness on this function the table lookups
/// below could all return the wrong thing in the live client and stay green.
/// `recipes` is the last `update_recipes` (M152), or `None` before one has
/// arrived. It supplies the **only wire-derived predicates on this struct** —
/// the three smithing sets — and every other field is jar-derived, so a caller
/// that has no session (a gate, a unit test) passes `None` and gets a smithing
/// table that refuses everything. That is not a degraded mode: it is what
/// vanilla does before the packet lands, because
/// `ClientRecipeContainer.propertySet` is
/// `getOrDefault(id, RecipePropertySet.EMPTY)`.
pub(crate) fn item_props(
    items: &rewo_data::items::Items,
    id: i32,
    smithing: Option<&rewo_net::recipe_book::SmithingSets>,
) -> Option<rewo_world::inventory::ItemProps> {
    use rewo_data::item_props_table::{equip_slot, max_stack_size, EquipSlot};
    use rewo_world::inventory::ArmorPiece;
    let name = items.name(id)?;
    // The property sets carry RAW item ids (`holderRegistry`), which is the id
    // this function was handed — so this is a direct membership test with no
    // name round-trip, unlike every jar table above.
    let in_set = |pick: fn(&rewo_net::recipe_book::SmithingSets) -> &Vec<i32>| {
        smithing.is_some_and(|s| pick(s).contains(&id))
    };
    Some(rewo_world::inventory::ItemProps {
        max_stack: max_stack_size(name),
        // M152 — the smithing table's three input tests.
        smithing_template: in_set(|s| &s.template),
        smithing_base: in_set(|s| &s.base),
        smithing_addition: in_set(|s| &s.addition),
        // M91 — the furnace quick-move's two predicates, resolved here because
        // this is where the numeric id becomes a name.
        is_fuel: rewo_data::fuel_table::is_fuel(name),
        smeltable: [
            rewo_data::smelting_table::accepts(10, name) == Some(true),
            rewo_data::smelting_table::accepts(14, name) == Some(true),
            rewo_data::smelting_table::accepts(22, name) == Some(true),
        ],
        // M93 — the beacon quick-move's one item predicate.
        beacon_payment: rewo_data::beacon_payment_table::is_beacon_payment(name),
        // M93b — the stonecutter's.
        stonecuttable: rewo_data::stonecutter_table::accepts_input(name),
        // M93e — the PROTOTYPE half of `isDamageableItem`. The patch half
        // rides on the stack. `prototype_has_component` is M56's table, which
        // already exists for the tooltip's component count.
        proto_max_damage: rewo_data::item_components_table::prototype_has_component(
            name,
            "minecraft:max_damage",
        )
        .unwrap_or(false),
        proto_damage: rewo_data::item_components_table::prototype_has_component(
            name,
            "minecraft:damage",
        )
        .unwrap_or(false),
        // M93f — `is(PAPER) || is(MAP) || is(GLASS_PANE)`. Item identity, so
        // three names rather than a table. `minecraft:map` is the EMPTY map;
        // `filled_map` is a different item and routes by its MAP_ID component.
        cartography_additional: matches!(
            name,
            "minecraft:paper" | "minecraft:map" | "minecraft:glass_pane"
        ),
        // M93g — the loom. The banner half is item identity from the tag; the
        // other two are the CONJUNCTIONS, tag AND prototype component, and a
        // tag-only test would look correct on every vanilla item.
        loom_banner: rewo_data::loom_table::is_banner(name),
        loom_dye: rewo_data::loom_table::in_loom_dyes(name)
            && rewo_data::item_components_table::prototype_has_component(
                name,
                "minecraft:dye",
            )
            .unwrap_or(false),
        loom_pattern: rewo_data::loom_table::in_loom_patterns(name)
            && rewo_data::item_components_table::prototype_has_component(
                name,
                "minecraft:provides_banner_patterns",
            )
            .unwrap_or(false),
        equips: match equip_slot(name) {
            Some(EquipSlot::Head) => Some(ArmorPiece::Head),
            Some(EquipSlot::Chest) => Some(ArmorPiece::Chest),
            Some(EquipSlot::Legs) => Some(ArmorPiece::Legs),
            Some(EquipSlot::Feet) => Some(ArmorPiece::Feet),
            // `body` and `saddle` are animal equipment and `mainhand`/`offhand`
            // are not armour slots, so none of them satisfies an `ArmorSlot`.
            _ => None,
        },
    })
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
