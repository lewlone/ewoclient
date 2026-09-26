use super::*;

impl LiveApp {
    /// Open or close the inventory screen (M35).
    ///
    /// Opening frees the cursor and parks it in the middle of the window;
    /// closing grabs it again and hides it. The park matters — winit reports
    /// no position until the mouse moves, so without it the first frame would
    /// hover whatever slot happens to sit at the stale coordinate.
    /// The layout on screen — the open container's, or the player's.
    ///
    /// `StonecutterScreen.mouseDragged` (M93s).
    ///
    /// ```java
    /// if (this.scrolling && this.isScrollBarActive()) {
    ///    int yscr = this.topPos + 14;
    ///    this.scrollOffs = ((float)event.y() - yscr - 7.5F) / ((yscr + 54) - yscr - 15.0F);
    ///    this.scrollOffs = Mth.clamp(this.scrollOffs, 0.0F, 1.0F);
    ///    this.startIndex = (int)(this.scrollOffs * this.getOffscreenRows() + 0.5) * 4;
    /// }
    /// ```
    ///
    /// Both halves of the guard matter: `scrolling` alone would move the list
    /// whenever the cursor did, and `isScrollBarActive` alone would let a list
    /// that shrank under a held thumb keep scrolling past its end.
    pub(super) fn cut_drag(&mut self) {
        use rewo_world::menu_screen as ms;
        if !self.screen.cut.is_some_and(|c| c.scrolling) {
            return;
        }
        let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) else {
            return;
        };
        let items = self.items.clone();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.open() else {
            return;
        };
        if open.layout.protocol_id != ms::STONECUTTER_MENU_PROTOCOL_ID {
            return;
        }
        let name = open.menu.menu_slot(0).and_then(|s| items.name(s.item_id));
        let visible = name.map_or(0, |n| rewo_data::stonecutter_table::select_by_input(n).len());
        if !ms::cut_scroll_active(ms::cut_display_recipes(name.is_some(), visible), visible) {
            return;
        }
        let (_, gy) = rewo_gpu::container::screen_to_gui_for(
            self.screen.mouse,
            ext.width as f32,
            ext.height as f32,
            open.layout.image_w as f32,
            open.layout.image_h as f32,
        );
        if let Some(c) = self.screen.cut.as_mut() {
            c.scroll_offs = ms::cut_scroll_offs_from_drag(gy);
        }
    }

    /// `MerchantScreen.mouseDragged` / `mouseScrolled` (M93u).
    pub(super) fn merchant_drag(&mut self) {
        use rewo_world::merchant_screen as ms;
        if !self.screen.merchant.is_some_and(|l| l.dragging) {
            return;
        }
        let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) else {
            return;
        };
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.open() else {
            return;
        };
        if open.layout.protocol_id != ms::MERCHANT_MENU_PROTOCOL_ID {
            return;
        }
        let n = session.merchant.as_ref().map_or(0, |m| m.offers.len());
        // `mouseDragged` is NOT gated on `canScroll` — only on `isDragging` —
        // but `maxScrollOff` goes negative for a short list, so the clamp is
        // what keeps it at 0. Guarding here would be a deviation that happens
        // to agree.
        let (_, gy) = rewo_gpu::container::screen_to_gui_for(
            self.screen.mouse,
            ext.width as f32,
            ext.height as f32,
            open.layout.image_w as f32,
            open.layout.image_h as f32,
        );
        if let Some(l) = self.screen.merchant.as_mut() {
            l.scroll_off = ms::scroll_off_from_drag(gy, n);
        }
    }

    /// The cursor in **GUI** pixels — the space every widget's rect is in.
    ///
    /// **M82 passed `self.screen.mouse` straight through, and it is in screen
    /// pixels.** `deathshot` divides by the GUI scale before calling the same
    /// builders, so its hover and click witnesses passed while the live path
    /// tested a point up to four times too far right and down: at any scale
    /// above 1 the death screen's buttons could not be hovered, and could only
    /// be *clicked* where the mis-scaled point happened to land on one. M85
    /// then built three more screens against the same call, so by the time M84
    /// found it — through its own hover witness measuring a zero-pixel
    /// difference — it was in five places. Every screen goes through here now.
    ///
    /// The inventory does **not**: `rewo_gpu::container::screen_to_gui` is
    /// panel-relative, which is what `slot_at` wants, and it takes screen
    /// pixels by design.
    pub(super) fn mouse_gui(&self) -> (f64, f64) {
        let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) else {
            return (0.0, 0.0);
        };
        let scale = rewo_gpu::hud::gui_scale(ext.width as f32, ext.height as f32) as f64;
        (self.screen.mouse.0 / scale, self.screen.mouse.1 / scale)
    }

    /// `ChatScreen.mouseClicked` — the popup, then clickable chat text (M128).
    ///
    /// The hit test is passed as a closure so it runs only when the popup
    /// declines, which is vanilla's order. It needs three things this method
    /// has and [`rewo_world::chat_screen::ChatScreen`] does not: the chat
    /// store, the font's advances, and the box geometry.
    pub(super) fn chat_mouse_pressed(&mut self, button: u8) -> rewo_world::chat_screen::ChatClick {
        use rewo_world::chat_screen::ChatClick;
        let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) else {
            return ChatClick::NotHandled;
        };
        let gui_px = gui_px(ext.width, ext.height);
        let (mx, my) = (self.screen.mouse.0 as f32, self.screen.mouse.1 as f32);
        let advance = self.advance();
        let opts = rewo_world::chat::ChatOptions::default();
        let chat_px = gui_px * opts.scale as f32;
        let geom = rewo_world::chat::ChatBoxGeometry::new(ext.height as f32, chat_px, &opts);
        // `mouse_gui` in integers, which is what `(int)event.x()` is.
        let (imx, imy) = self.mouse_gui();
        let outcome = {
            let Some(session) = self.session.as_ref() else {
                return ChatClick::NotHandled;
            };
            // The same clock `build_text` hands `chat_lines`, so the hit test
            // and the draw agree about which rows have faded out — a faded
            // line is SKIPPED AND NOT COUNTED, so a stale tick would shift
            // every row's index.
            let gui_tick = session.ticks as i32;
            let chat = &session.chat;
            let width_of = |s: &str, st: rewo_world::chat_style::ChatStyle| match &advance {
                Some(a) => rewo_gpu::text::width_styled(s, a, st.bold),
                None => 0,
            };
            let hit = |shift: bool| {
                rewo_world::chat::clickable_style_at(
                    chat,
                    gui_tick,
                    // The chat screen is open, which is exactly what
                    // `ChatComponent.isChatFocused()` asks.
                    true,
                    &opts,
                    &geom,
                    gui_px,
                    chat_px,
                    (mx, my),
                    &width_of,
                    shift,
                )
            };
            let Some(s) = self.chat_screen.as_mut() else {
                return ChatClick::NotHandled;
            };
            s.mouse_clicked(imx as i32, imy as i32, button, self.shift, &hit)
        };
        match &outcome {
            ChatClick::NotHandled | ChatClick::Handled => {}
            ChatClick::OpenUrl(uri) => {
                crate::uri_open::open_uri(uri);
            }
            ChatClick::RunCommand(command) => {
                let command = command.clone();
                if let Some(session) = self.session.as_mut() {
                    if let Err(e) = session.send_command(&command) {
                        log::warn!("chat: run_command failed: {e}");
                    }
                }
            }
            ChatClick::Declined(why) => {
                log::info!("chat: click declined ({why})");
            }
        }
        // `onEdited`'s other two lines, for the one outcome that changed the
        // field: `suggest_command` calls `setValue`, whose `onValueChange`
        // reaches `setAllowSuggestions(true)` and `updateCommandInfo()` in
        // vanilla. `ChatScreen` cannot run them itself — they need the
        // `SuggestionEnv`, which lives out here.
        if matches!(outcome, ChatClick::Handled) {
            self.resolve_command_suggestions();
        }
        outcome
    }

    /// The input field's geometry, as `CommandSuggestions` measures from it.
    pub(super) fn suggestion_metrics(&self) -> rewo_world::command_suggestions::InputMetrics {
        let (gui_w, gui_h) = self.gui_size();
        let (x, _y, w, _h) = rewo_world::chat_screen::input_rect(gui_w, gui_h);
        rewo_world::command_suggestions::InputMetrics {
            x,
            // `getInnerWidth()` is `bordered ? width - 8 : width`, and the
            // chat field calls `setBordered(false)`.
            inner_width: w,
            screen_height: gui_h,
        }
    }

    /// One key press while the chat screen is up (M110).
    ///
    /// The adapter M97's lesson keeps producing: every rule lives in
    /// `rewo_world::chat_screen` and reaches a test; this turns the returned
    /// `ChatAction` into the two things only the app can do — talk to the
    /// socket and close the screen.
    pub(super) fn chat_key(&mut self, key: i32, modifiers: i32) {
        use rewo_world::chat_screen::{ChatAction, ExitReason};
        // Esc is `onClose`, and it is checked here rather than inside the
        // model because `Screen.keyPressed` handles it before the focused
        // widget — an `EditBox` with `canLoseFocus` false would otherwise
        // swallow it.
        if key == 256 {
            if let Some(s) = self.chat_screen.as_mut() {
                s.close();
            }
            self.close_chat_screen();
            return;
        }
        let (recent, per_page) = match self.session.as_ref() {
            Some(session) => (
                session.chat.recent_chat().to_vec(),
                rewo_world::chat::ChatOptions::default().lines_per_page(true),
            ),
            None => (Vec::new(), 20),
        };
        let mut clip = std::mem::take(&mut self.clipboard);
        let metrics = self.suggestion_metrics();
        let words = self.tab_words();
        let advance_for_width = self.advance();
        let width_of = move |s: &str| match &advance_for_width {
            Some(a) => rewo_gpu::text::width(s, a),
            None => 0,
        };
        let env = rewo_world::chat_screen::SuggestionEnv {
            metrics,
            width: &width_of,
            tab_words: &words,
            // `Options.autoSuggestions` defaults true, and Rewo has no
            // options screen to turn it off.
            auto_suggestions: true,
        };
        let action = match self.chat_screen.as_mut() {
            Some(s) => s.key_pressed(
                rewo_world::edit_box::Input { key, modifiers },
                &mut clip,
                &recent,
                per_page,
                &env,
            ),
            None => ChatAction::None,
        };
        self.clipboard = clip;
        self.resolve_command_suggestions();
        let advance = self.advance();
        // Which variant it was, captured before `action` is consumed.
        let is_command = matches!(action, ChatAction::Command(_));
        match action {
            ChatAction::Send(msg) | ChatAction::Command(msg) => {
                if let Some(session) = self.session.as_mut() {
                    // `addRecentChat` takes the message as TYPED, before
                    // `handleChatInput` strips the slash — the history replays
                    // what you wrote, not what went on the wire.
                    let typed = if is_command {
                        format!("/{msg}")
                    } else {
                        msg.clone()
                    };
                    session.chat.add_recent_chat(&typed);
                    let sent = if is_command {
                        session.send_command(&msg)
                    } else {
                        session.send_chat(&msg)
                    };
                    if let Err(e) = sent {
                        log::warn!("chat: send failed: {e}");
                    }
                }
            }
            ChatAction::Scroll(n) => {
                if let Some(session) = self.session.as_mut() {
                    let width_of =
                        move |s: &str, st: rewo_world::chat_style::ChatStyle| match &advance {
                            Some(a) => rewo_gpu::text::width_styled(s, a, st.bold),
                            None => 0,
                        };
                    let ctx = rewo_world::chat::WrapContext {
                        options: rewo_world::chat::ChatOptions::default(),
                        // The box is TALLER while the screen is open, and
                        // `scrollChat` clamps against `getLinesPerPage()` — so
                        // `false` here would clamp the focused view against the
                        // unfocused box's ten rows and stop the scroll short.
                        focused: true,
                        width_of: &width_of,
                        deleted_marker_text: DELETED_CHAT_MESSAGE,
                    };
                    session.chat.scroll_chat(n, &ctx);
                }
            }
            ChatAction::Close | ChatAction::None | ChatAction::NotHandled => {}
        }
        // `closeOnSubmit` is true for this screen, so a submitted message
        // closes it — the model records that as `ExitReason::Done` rather than
        // returning a `Close`, because the two are the same event in vanilla.
        if matches!(
            self.chat_screen.as_ref().map(|s| s.exit_reason()),
            Some(ExitReason::Done)
        ) {
            self.close_chat_screen();
        }
    }

    /// One printable character typed into the chat screen.
    ///
    /// Split out of the event loop because `onEdited` now needs the
    /// suggestion environment, which needs `&self` while the screen needs
    /// `&mut self`.
    pub(super) fn chat_char(&mut self, ch: char) {
        let metrics = self.suggestion_metrics();
        let words = self.tab_words();
        let advance_for_width = self.advance();
        let width_of = move |s: &str| match &advance_for_width {
            Some(a) => rewo_gpu::text::width(s, a),
            None => 0,
        };
        let env = rewo_world::chat_screen::SuggestionEnv {
            metrics,
            width: &width_of,
            tab_words: &words,
            auto_suggestions: true,
        };
        if let Some(s) = self.chat_screen.as_mut() {
            s.char_typed(ch, &env);
        }
        self.resolve_command_suggestions();
    }

    /// The coloured runs for the chat field, or `None` when there is nothing
    /// to colour (M117).
    ///
    /// `formatChat` returns null while `currentParse` is null, and
    /// `updateCommandInfo` only ever builds one for a `/`-command — so an
    /// ordinary chat message is drawn in the field's own colour, which is a
    /// state vanilla passes through too.
    /// The usage box's fills and text for this frame (M117), or two empty
    /// lists when there is nothing to show.
    ///
    /// Built from the cached parse rather than a fresh one, so it and the
    /// syntax highlighting cannot disagree about what the field says.
    ///
    /// A free-standing associated function rather than a method, for the same
    /// reason `chat_runs` is: the frame already holds the session borrowed.
    pub(super) fn usage_box_parts(
        cs: Option<&rewo_world::chat_screen::ChatScreen>,
        session: &PlaySession,
        cache: &Option<(String, rewo_net::dispatcher::ParseResults)>,
        advance: Option<[u8; 256]>,
        gui: (i32, i32),
        px: f32,
    ) -> (Vec<rewo_gpu::hud::HudFill>, Vec<rewo_gpu::world::OwnedTextLine>) {
        let empty = (Vec::new(), Vec::new());
        let Some(cs) = cs else {
            return empty;
        };
        let Some((text, parsed)) = cache.as_ref() else {
            return empty;
        };
        if text != &cs.input.value() {
            return empty;
        }
        let cursor = cs.input.cursor_position();
        let lines = rewo_net::command_format::usage_lines(
            &session.commands,
            parsed,
            cursor,
            cs.suggestions.pending().is_none_or(|s| s.is_empty()),
            // M134 — `getExceptionMessage` wraps every message in
            // `command.context.parse_error`, so the box needs the table the
            // session already carries for chat.
            session.lang.as_deref(),
        );
        if lines.is_empty() {
            return empty;
        }
        let width_of = move |s: &str| match &advance {
            Some(a) => rewo_gpu::text::width(s, a),
            None => 0,
        };
        let (gui_w, gui_h) = gui;
        let _ = gui_w;
        let (fx, _fy, fw, _fh) = rewo_world::chat_screen::input_rect(gui_w, gui_h);
        // `getScreenX(startPos)` — the field's x plus the width of everything
        // before the word being completed.
        let start = rewo_net::command_format::usage_lines_start(parsed, cursor);
        let value: Vec<u16> = cs.input.value().encode_utf16().collect();
        let prefix = String::from_utf16_lossy(&value[..start.min(value.len())]);
        let box_width = lines.iter().map(|l| width_of(&l.text)).max().unwrap_or(0);
        let position = rewo_net::command_format::usage_position(
            fx + width_of(&prefix),
            fx,
            fw,
            box_width,
        );
        usage_box(
            &lines,
            position,
            gui_h,
            px,
            cs.suggestions.config().fill_color,
            &width_of,
        )
    }

    /// Answer a `/`-command's completion locally where the dispatcher can, and
    /// ask the server only where vanilla would (M116).
    ///
    /// M114 asked about **every** command, because with no dispatcher the
    /// client could not tell a literal from an argument. Now it can:
    /// `dispatcher::parse` walks the tree M113 decodes, and
    /// `completion_suggestions` returns both what the client answered and
    /// whether any candidate child's provider is one Rewo routes to the
    /// server. `/g` therefore completes with **no packet at all**.
    ///
    /// When it does ask, the server's reply REPLACES the local set rather than
    /// merging with it — see `dispatcher`'s module docs — because
    /// `handleCustomCommandSuggestions` runs the server's own dispatcher over
    /// the whole input and returns literals too, so its answer is a superset
    /// at that position.
    pub(super) fn resolve_command_suggestions(&mut self) {
        let Some(command) = self
            .chat_screen
            .as_mut()
            .and_then(|s| s.take_command_request())
        else {
            return;
        };
        let units: Vec<u16> = command.encode_utf16().collect();
        // The text is the field up to the cursor, INCLUDING the slash, so the
        // cursor is its length and the parse starts at 1 — every range is then
        // an index into the field itself.
        // M118 — the selector parser needs the online names, exactly as
        // `EntityArgument.listSuggestions` takes them from the source.
        let words = self.tab_words();
        let cmd = rewo_net::dispatcher::CommandCtx {
            names: &words,
            blocks: Some(&self.blocks),
            items: Some(&self.items),
        };
        let completion = self.session.as_ref().map(|session| {
            let parsed = rewo_net::dispatcher::parse(&session.commands, &units, 1, cmd);
            rewo_net::dispatcher::completion_suggestions(&session.commands, &parsed, units.len(), cmd)
        });
        let Some(completion) = completion else {
            return;
        };
        if completion.ask_server {
            if let Some(session) = self.session.as_mut() {
                if let Err(e) = session.request_command_suggestions(&command) {
                    log::debug!("chat: suggestion request not sent: {e}");
                }
            }
            return;
        }
        // Counted only when the client actually ANSWERED. An empty command
        // tree parses to no children and would otherwise report a local
        // completion for every keystroke while proving nothing — the witness
        // has to name the suggestions, not the code path.
        if !completion.local.is_empty() {
            // M118 — a selector answered locally is a strictly narrower claim
            // than "a completion was", because it needs the entity argument's
            // own parser rather than a literal match.
            let selector = completion
                .local
                .list
                .iter()
                .any(|s| s.text.starts_with('@'));
            // M119 — a namespaced id. Distinctive by construction: a selector
            // starts with `@`, and neither a literal nor a selector-option
            // name contains a colon.
            let resource = completion
                .local
                .list
                .iter()
                .any(|s| s.text.contains(':'));
            // M120 — a coordinate default. `~` appears in no literal, no
            // selector and no registry id, so the test is disjoint from r33's
            // and r34's by construction.
            let coordinate = completion
                .local
                .list
                .iter()
                .any(|s| s.text.starts_with('~'));
            // M124 — a name from one of the seven literal tables. `DisplaySlot`
            // is the only source of a `sidebar.team.` prefix anywhere in the
            // protocol, so this cannot be satisfied by a literal, a selector, a
            // registry id or a coordinate.
            let literal_table = completion
                .local
                .list
                .iter()
                .any(|s| s.text.starts_with("sidebar.team."));
            if let Some(c) = self.check.as_mut() {
                c.local_command_completions += 1;
                if selector {
                    c.local_selector_completions += 1;
                }
                if resource {
                    c.local_resource_completions += 1;
                }
                if coordinate {
                    c.local_coordinate_completions += 1;
                }
                if literal_table {
                    c.local_literal_table_completions += 1;
                }
            }
        }
        let metrics = self.suggestion_metrics();
        let advance_for_width = self.advance();
        let width_of = move |s: &str| match &advance_for_width {
            Some(a) => rewo_gpu::text::width(s, a),
            None => 0,
        };
        let env = rewo_world::chat_screen::SuggestionEnv {
            metrics,
            width: &width_of,
            tab_words: &[],
            auto_suggestions: true,
        };
        if let Some(s) = self.chat_screen.as_mut() {
            s.accept_suggestions(completion.local, &env);
        }
    }

    /// `T` / `/` — `ChatComponent.openScreen`.
    pub(super) fn open_chat_screen(&mut self, method: rewo_world::chat_screen::ChatMethod) {
        let recent_len = self
            .session
            .as_ref()
            .map(|s| s.chat.recent_chat().len())
            .unwrap_or(0);
        self.chat_screen = Some(rewo_world::chat_screen::ChatScreen::open(
            method,
            self.chat_draft.as_ref(),
            recent_len,
        ));
        self.grab_for_screen(false);
    }

    /// `Gui.setScreen(null)` — and `ChatScreen.removed`, which is where the
    /// draft is decided and the chat scroll goes back to the bottom.
    pub(super) fn close_chat_screen(&mut self) {
        use rewo_world::chat_screen::DraftOutcome;
        if let Some(s) = self.chat_screen.take() {
            match s.removed(true) {
                DraftOutcome::Discard => self.chat_draft = None,
                DraftOutcome::Save(d) => self.chat_draft = Some(d),
                DraftOutcome::Keep => {}
            }
            if let Some(session) = self.session.as_mut() {
                session.chat.reset_chat_scroll();
            }
        }
        self.grab_for_screen(true);
    }

    /// One widget press on whatever screen is up (M82).
    ///
    /// This is the dispatch arm a new screen adds: the framework hands back a
    /// `(ScreenKind, WidgetId)` and the app decides what it means.
    pub(super) fn press_widget(&mut self, kind: rewo_world::screen::ScreenKind, id: rewo_world::screen::WidgetId) {
        use rewo_world::death_screen as ds;
        use rewo_world::screen::ScreenKind;
        match (kind, id) {
            // M84: the statistics screen. `Done` is `onClose()`, a tab press is
            // `selectTab`, and a sort button is `sortByColumn` — the last two
            // rebuild, because a tab change moves the six sort widgets and
            // reselects every tab's sheet.
            (ScreenKind::Stats, rewo_world::stats_screen::DONE) => {
                self.close_stats();
            }
            (ScreenKind::Stats, id) => {
                if self.stats.as_mut().is_some_and(|v| v.press(id)) {
                    self.rebuild_stats_screen();
                }
            }
            (ScreenKind::Death, ds::RESPAWN) => {
                if let Some(session) = self.session.as_mut() {
                    if let Err(e) = session.perform_respawn() {
                        log::warn!("live: respawn: {e}");
                    }
                }
                // `button.active = false` — the second half of vanilla's
                // `onPress`. The screen stays up until the server's respawn
                // arrives; this is what stops a double press.
                if let Some(s) = self.screen.screens.current_mut() {
                    if let Some(w) = s.widget_mut(ds::RESPAWN) {
                        w.active = false;
                    }
                }
                // `KeyMapping.resetToggleKeys()`.
                self.keys = Keys::default();
            }
            // M172: the book's Done button is `onClose()`.
            (ScreenKind::BookView, rewo_world::book_view_screen::DONE) => {
                self.close_book();
            }
            // M178: the advancements footer's Done is `onClose()` — which
            // lands in `removed()`'s CLOSED_SCREEN send, as every exit does.
            (ScreenKind::Advancements, rewo_world::advancements_screen::DONE) => {
                self.close_advancements();
            }
            // M174: the sign editor's Done is `onDone()` — which COMMITS
            // (the packet is in `removed()`, reached by every exit).
            (ScreenKind::SignEdit, rewo_world::sign_edit_screen::DONE) => {
                self.close_sign_edit();
            }
            // M173: the options pages.
            (ScreenKind::Options, id) if self.options_view.is_some() => {
                self.options_press(id);
            }
            // M85's three screens.
            (ScreenKind::Pause, rewo_world::pause_screen::RETURN_TO_GAME) => {
                // `this.minecraft.gui.setScreen(null); mouseHandler.grabMouse();`
                self.close_view_screen();
            }
            (ScreenKind::Pause, rewo_world::pause_screen::OPTIONS) => {
                // M173: the options are real now — the root page, whose Done
                // and Esc return here.
                self.open_options_screen(rewo_world::options_screen::OptionsPage::Root);
            }
            (ScreenKind::Pause, rewo_world::pause_screen::SERVER_LINKS) => {
                // `minecraft.player.connection.showDialog(dialog, this)`.
                self.open_links_screen();
            }
            (ScreenKind::Pause, rewo_world::pause_screen::DISCONNECT) => {
                // `minecraft.disconnectFromWorld(DEFAULT_QUIT_MESSAGE)`. Rewo
                // has no title screen to land on, so leaving the session is
                // the whole of it — the same call the death screen's second
                // button makes.
                log::info!("live: pause screen — leaving the server");
                self.exit_requested = true;
            }
            (ScreenKind::Pause, id) => {
                // Advancements, Statistics and Options. Drawn as vanilla draws
                // them and inert on press: `award_stats` is a sibling
                // milestone's screen and the other two do not exist. Logged
                // rather than silently swallowed.
                log::info!("live: pause screen — widget {id} is not implemented");
            }
            (ScreenKind::ServerLinks, rewo_world::server_links_screen::BACK) => {
                // `DialogAction.CLOSE` → `previousScreen`, which is the pause
                // screen this dialog was opened from.
                self.open_pause_screen();
            }
            (ScreenKind::ServerLinks, id) => {
                // **Rewo does not open a URL.** Vanilla's path is
                // `StaticAction(ClickEvent.OpenUrl)` → `Screen.clickUrlAction`,
                // which itself shows a `ConfirmLinkScreen` unless the player
                // turned the prompt off. Launching a browser from a string a
                // remote server chose is a decision, not a transcription — see
                // `rewo_net::server_links`.
                let i = rewo_world::server_links_screen::link_index(id).unwrap_or(0);
                match self.server_links.entries().get(i) {
                    Some(e) => log::info!(
                        "live: server link {i} selected: {} (Rewo does not open URLs)",
                        e.link
                    ),
                    None => log::warn!("live: server link {i} has no entry"),
                }
            }
            (ScreenKind::Disconnected, _) => {
                // `gui.toMenu` / `gui.toTitle` — a server list and a title
                // screen, neither of which Rewo has. Leaving is the whole of
                // it.
                log::info!("live: disconnect screen — exiting");
                self.exit_requested = true;
            }
            (ScreenKind::Death, ds::TITLE_SCREEN) => {
                // `exitToTitleScreen`: `level.disconnect(...)` then
                // `disconnectWithSavingScreen()` then a `TitleScreen`. Rewo
                // has no title screen, so leaving the session is the whole of
                // it. The `ConfirmScreen` vanilla interposes for a non-hardcore
                // world is not reproduced — see `death_screen`'s docs.
                log::info!("live: death screen — leaving the server");
                self.exit_requested = true;
            }
            _ => {}
        }
    }

    /// M173 — one press on an options page's widget.
    pub(super) fn options_press(&mut self, id: rewo_world::screen::WidgetId) {
        use rewo_world::options_screen as os;
        let Some(page) = self.options_view else { return };
        match page {
            os::OptionsPage::Root => match id {
                // `root_rows`' packing in vanilla's link order (Sounds,
                // Video, Accessibility): row-major, two per row.
                0 => self.open_options_screen(os::OptionsPage::Sound),
                1 => self.open_options_screen(os::OptionsPage::Video),
                2 => self.open_options_screen(os::OptionsPage::Accessibility),
                os::DONE => self.close_options(),
                _ => {}
            },
            os::OptionsPage::Sound => match os::sound_slot(id) {
                Some(os::SoundSlot::MusicFrequency) => {
                    // The CALLBACK path (M161): a live change re-rolls
                    // `nextSongDelay` from the CURRENT situational track —
                    // `setMinutesBetweenSongs`, not the constructor's store.
                    self.options.cycle_frequency();
                    let situational = self
                        .session
                        .as_ref()
                        .and_then(|s| s.situational_music());
                    self.sounds
                        .change_music_frequency(self.options.music_frequency, situational.as_ref());
                    // A cycle button saves on every click (vanilla's
                    // `CycleableValueSet.createButton` -> `options.save()`).
                    save_options(self.options);
                    self.rebuild_options_screen();
                }
                Some(os::SoundSlot::Done) => self.close_options(),
                _ => {}
            },
            os::OptionsPage::Video => match id {
                0 => {
                    // The button is `CycleableValueSet.createButton`, so a
                    // click runs the option's `onValueUpdate` and then
                    // `options.save()`. For `ambientOcclusion` the
                    // `onValueUpdate` is `operateOnLevelExtractor(
                    // LevelExtractor::allChanged)` (`Options.java:236-239`) —
                    // vanilla re-meshes every chunk, because the vertex light
                    // is baked into the mesh rather than applied at draw time.
                    //
                    // Rewo re-meshes through the mechanism every block edit
                    // uses: the `PlaySession` dirty set (`requeue_dirty` over
                    // the loaded columns), which `pump_meshing` turns into
                    // `pool.submit` calls. `MeshPool` reads the setting at
                    // SUBMIT time (it is moved into the job), so the re-submits
                    // all carry the new value; a job already in flight keeps
                    // the old one and is replaced by the re-submit that follows
                    // it (see `rewo_mesh::pool` — one job per
                    // `(generation, cx, cz)` at a time).
                    self.options.smooth_lighting = !self.options.smooth_lighting;
                    self.pool.set_smooth_lighting(self.options.smooth_lighting);
                    if let Some(session) = self.session.as_mut() {
                        let coords = session.world.column_coords();
                        session.requeue_dirty(coords);
                    }
                    // A cycle button saves on every click (vanilla's
                    // `CycleableValueSet.createButton` -> `options.save()`).
                    save_options(self.options);
                    self.rebuild_options_screen();
                }
                os::DONE => self.close_options(),
                _ => {}
            },
            os::OptionsPage::Accessibility => match id {
                0 => {
                    self.options.hide_lightning_flash = !self.options.hide_lightning_flash;
                    save_options(self.options);
                    self.rebuild_options_screen();
                }
                os::DONE => self.close_options(),
                _ => {}
            },
        }
    }

    /// A left click on the tab strip (M179). Selects the hit tab and sends
    /// `opened_tab` — UNCONDITIONALLY, including on the already-selected tab,
    /// because `ClientAdvancements.setSelectedTab`'s send sits BEFORE its
    /// change check (`ClientAdvancements.java:77-86`): re-clicking the open
    /// tab re-tells the server. The listener notification is what is
    /// change-gated, not the packet.
    pub(super) fn advancements_tab_click(&mut self, mx: f64, my: f64) {
        if self.advancements.is_none() {
            return;
        }
        let (gw, gh) = self.gui_size();
        let report = self
            .advancements
            .as_mut()
            .and_then(|v| v.tab_click_report(gw, gh, mx, my));
        if let Some(root_id) = report {
            if let Some(session) = self.session.as_mut() {
                if let Err(e) = session.send_seen_advancements_opened_tab(&root_id) {
                    log::warn!("live: opened_tab: {e}");
                }
            }
        }
    }

    /// A left click on the book's page text (M180) — `handleClickEvent`'s
    /// arms (`BookViewScreen.java:228-247`). Returns whether a clickable
    /// span took the click.
    pub(super) fn book_page_click(&mut self, mx: f64, my: f64, gw: i32) -> bool {
        let Some(book) = self.book.as_ref() else {
            return false;
        };
        let Some(advance) = self
            .baked
            .as_ref()
            .and_then(|b| b.font.as_ref())
            .map(|f| f.advance)
        else {
            return false;
        };
        let measure = |s: &rewo_world::chat_style::ChatSpan| {
            rewo_gpu::text::width_styled(&s.text, &advance, s.bold)
        };
        let Some(event) =
            rewo_world::book_view_screen::click_event_at(book, gw, &measure, mx as i32, my as i32)
        else {
            return false;
        };
        match event {
            // `ClickEvent.ChangePage(page)` is ONE-BASED — `forcePage(page - 1)`
            // (`:235-237`). Local page turn only; no packet.
            rewo_world::chat_events::ClickEvent::ChangePage(page) => {
                if let Some(book) = self.book.as_mut() {
                    book.force_page(page - 1);
                }
            }
            // `RunCommand`: `closeContainerOnServer()` runs first — EMPTY in
            // the plain reader (only LecternScreen overrides it, M87's
            // finding) — then the command itself, which `send_command`
            // already carries as the unsigned chat_command.
            rewo_world::chat_events::ClickEvent::RunCommand(command) => {
                if let Some(session) = self.session.as_mut() {
                    if let Err(e) = session.send_command(&command) {
                        log::warn!("live: book run_command({command}): {e}");
                    }
                }
            }
            // Everything else routes through
            // `defaultHandleGameClickEvent` — open_url and friends. Rewo
            // does not open URLs (M85's recorded rule); the click is still
            // CONSUMED (vanilla returns true for every non-null event), it
            // just does nothing.
            other => log::info!("live: book click event {other:?} declined"),
        }
        true
    }
}
