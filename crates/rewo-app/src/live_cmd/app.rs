use super::*;

impl ApplicationHandler for LiveApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Rewo · live")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.init_error = Some(format!("create window: {e}"));
                event_loop.exit();
                return;
            }
        };
        // Returns the bake **alongside** the state, and the `Ok` arm below puts
        // it back in `self.baked`.
        //
        // This closure used to return `LiveState` alone, which meant the
        // `self.baked.take()` below dropped the bake at the closing brace and
        // left `self.baked` as `None` for the entire windowed session. Every
        // `if let Some(baked) = self.baked.as_ref()` in `frame` was therefore
        // dead code in `rewo live` — the item icons, the inventory screen, the
        // first-person hand, the cloud deck, the precipitation, the rain-fog
        // band, the particles, the world border and the block-breaking decals,
        // none of which had ever rendered in the windowed client since M3. See
        // the M86 entry in `REWO_PLAN.md` §15.
        let init = (|| -> Result<(LiveState, assets::BakedAssets), String> {
            let rdh = window
                .display_handle()
                .map_err(|e| format!("dh: {e}"))?
                .as_raw();
            let rwh = window
                .window_handle()
                .map_err(|e| format!("wh: {e}"))?
                .as_raw();
            let mut gpu = Gpu::new(Some((rdh, rwh)), self.want_validation)?;
            let size = window.inner_size();
            let mut renderer = Renderer::with_frames_in_flight(
                &mut gpu,
                size.width.max(1),
                size.height.max(1),
                vk::PresentModeKHR::MAILBOX,
                self.fif,
            )?;
            renderer.ensure_depth(&mut gpu)?;
            let baked = self.baked.take().ok_or("assets consumed")?;
            let mut world_renderer = WorldRenderer::new(
                &mut gpu,
                renderer.swapchain.format,
                assets::TEX_SIZE,
                &baked.layers,
            )?;
            self.etf = init_entities_maybe_cem(&mut world_renderer, &mut gpu, &baked, &self.pack)?;
            world_renderer.set_held_items(to_gpu_held_items(&baked.held_items));
            init_celestial_if_present(&mut world_renderer, &mut gpu, &baked)?;
            init_weather_if_present(&mut world_renderer, &mut gpu, &baked)?;
            init_particles_if_present(&mut world_renderer, &mut gpu, &baked)?;
            init_crumbling_if_present(&mut world_renderer, &mut gpu, &baked)?;
            install_animations(&mut world_renderer, &mut gpu, &baked)?;
            if let Some(l) = locator_sprites(&baked) {
                self.locator_styles = l.styles.clone();
                world_renderer.init_locator_bar(&mut gpu, &l)?;
            }
            if let Some(w) = widget_sprites(&baked) {
                world_renderer.init_screen(&mut gpu, &w)?;
            }
            // The vanilla-font text pass (chat, the command line, screen
            // labels) and the container-screen pass (the inventory and every
            // menu's panel). The windowed client never created either: the
            // gates and the headless path call these on their own renderers,
            // so all of them drew text and panels while the real window drew
            // neither. `live --render-check`'s r65/r66 now ask this renderer.
            if let Some(font) = font_data(&baked) {
                world_renderer.init_text(&mut gpu, &font)?;
            }
            if let Some(c) = container_sprites(&baked) {
                world_renderer.init_container(&mut gpu, &c)?;
            }
            if let Some(hud) = hud_sprites(&baked) {
                world_renderer.init_hud(&mut gpu, &hud)?;
                // M52b: the Velvet type stack, windowed only. A build with no
                // fonts on disk gets `None` and the tooltip falls back to the
                // bitmap pass -- losing tooltips over a missing font file
                // would be worse than drawing them plainly.
                if let Some(cache) = self.glyphs.as_ref() {
                    if let Err(e) = world_renderer.init_velvet_text(&mut gpu, cache) {
                        log::warn!("velvet text unavailable: {e}");
                        self.glyphs = None;
                    }
                }
            }
            Ok((
                LiveState {
                    window: window.clone(),
                    gpu,
                    renderer,
                    world_renderer,
                },
                baked,
            ))
        })();
        match init {
            Ok((state, baked)) => {
                let _ = state.window.set_cursor_grab(CursorGrabMode::Confined);
                state.window.set_cursor_visible(false);
                self.started = Instant::now();
                self.state = Some(state);
                // The half M3 forgot. Without it every baked-gated branch in
                // `frame` is unreachable — see the closure's doc above.
                self.baked = Some(baked);
            }
            Err(e) => {
                self.init_error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(LiveState { gpu, renderer, .. }) = self.state.as_mut() {
                    if size.width > 0 && size.height > 0 {
                        // A failure here is retried: `render` reports a
                        // depth/swapchain mismatch as NeedsRecreate.
                        if let Err(e) = renderer
                            .resize(gpu, size.width, size.height)
                            .and_then(|()| renderer.ensure_depth(gpu))
                        {
                            log::error!("live: resize failed: {e}");
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let p = event.state == ElementState::Pressed;
                // While the screen is open the player stands still: only the
                // two keys that close it are read, and every movement key is
                // released so a key held at the moment of opening does not
                // stick down behind it.
                // With the screen open the keys that reach the world are
                // swallowed, but the screen has keys of its own: a number key
                // or F swaps the hovered slot with a hotbar slot, Q drops from
                // it. Ctrl is tracked either way, because Ctrl+Q drops the
                // whole stack.
                if matches!(event.physical_key, PhysicalKey::Code(KeyCode::ControlLeft)) {
                    self.ctrl = p;
                }
                if matches!(event.physical_key, PhysicalKey::Code(KeyCode::AltLeft)) {
                    self.alt = p;
                }
                // Ctrl+V pastes the SYSTEM clipboard, as vanilla's edit boxes do
                // (`KeyboardHandler.getClipboard`): refresh the buffer every
                // edit box pastes from before any of them sees the key.
                if p && self.ctrl && matches!(event.physical_key, PhysicalKey::Code(KeyCode::KeyV))
                {
                    if let Some(text) = crate::os_clipboard::read() {
                        self.clipboard_synced = text.clone();
                        self.clipboard = text;
                    }
                }
                // M151 — the tab-list hold's RELEASE, ahead of both screen
                // gates below, because `KeyboardHandler.keyPress` is
                // asymmetric about exactly this:
                //
                //   if (action == 0)      KeyMapping.set(key, false);   // always
                //   else if (handlesGameInput) KeyMapping.set(key, true);
                //
                // (`KeyboardHandler.java:519-552`.) The press is gated on there
                // being no screen — so Tab with the inventory open does not
                // open the list, which is why the press arm lives in the match
                // below — while the release is unconditional. Without this,
                // holding Tab, opening the inventory and letting go would leave
                // the list on screen behind it with nothing able to take it
                // down. The movement keys have the same asymmetry and Rewo does
                // not reproduce it for them; that predates this and is not the
                // subject here.
                if !p && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Tab)) {
                    self.keys.tab_list = false;
                }
                // M110 — the chat screen owns the keyboard entirely while it
                // is open, and it goes ahead of every other screen because
                // `Gui.screen` is ONE slot: with a chat screen in it there is
                // no inventory to route to. Opening it is handled after this
                // block, so `T` cannot both open the screen and be typed into
                // it on the same event.
                if self.chat_screen.is_some() {
                    if p {
                        if let Some(key) = glfw_key(event.physical_key) {
                            let mods = (i32::from(self.shift))
                                | (i32::from(self.ctrl) << 1)
                                | (i32::from(self.alt) << 2);
                            self.chat_key(key, mods);
                        }
                        if let Some(text) = event.text.as_ref() {
                            let chars: Vec<char> = text.chars().collect();
                            for ch in chars {
                                self.chat_char(ch);
                            }
                        }
                    }
                    // Shift is tracked either way: it holds the wheel to one
                    // line and is read by `mouse_scrolled`.
                    if !matches!(
                        event.physical_key,
                        PhysicalKey::Code(KeyCode::ShiftLeft)
                            | PhysicalKey::Code(KeyCode::ShiftRight)
                    ) {
                        return;
                    }
                }
                // M174 — the sign editor owns the keyboard entirely while it
                // is open (`Screens` is ONE slot; there is no inventory behind
                // it to route to). Its own bindings run first, then the field,
                // then Esc → `onClose()`; a key nothing wanted is swallowed,
                // which is the anvil's arrangement and vanilla's.
                if self.sign_edit.is_some() {
                    if p {
                        let advance = self.advance();
                        if let (Some(key), Some(advance)) =
                            (glfw_key(event.physical_key), advance.as_ref())
                        {
                            let mods = (i32::from(self.shift))
                                | (i32::from(self.ctrl) << 1)
                                | (i32::from(self.alt) << 2);
                            let width_fn = |t: &str| rewo_gpu::text::width(t, advance);
                            let mut clip = std::mem::take(&mut self.clipboard);
                            let outcome = self.sign_edit.as_mut().map(|v| {
                                v.state.key_pressed(
                                    rewo_ui::edit_box::Input::new(key, mods),
                                    &width_fn,
                                    &mut clip,
                                )
                            });
                            self.clipboard = clip;
                            match outcome {
                                Some(rewo_ui::sign_edit_screen::SignKey::Handled) => {
                                    self.echo_sign_edit();
                                    return;
                                }
                                Some(rewo_ui::sign_edit_screen::SignKey::Close) => {
                                    self.close_sign_edit();
                                    return;
                                }
                                _ => {}
                            }
                        }
                        // `charTyped` — the screen returns true EITHER WAY, so
                        // every typed character is consumed whether or not it
                        // was an allowed chat character (the insert itself is
                        // gated inside the model).
                        if let Some(text) = event.text.as_ref() {
                            if let Some(advance) = self.advance() {
                                let width_fn = |t: &str| rewo_gpu::text::width(t, &advance);
                                for ch in text.chars() {
                                    if let Some(v) = self.sign_edit.as_mut() {
                                        v.state.char_typed(ch, &width_fn);
                                    }
                                }
                                self.echo_sign_edit();
                            }
                        }
                    }
                    // Shift is tracked either way (Ctrl/Alt above).
                    if !matches!(
                        event.physical_key,
                        PhysicalKey::Code(KeyCode::ShiftLeft)
                            | PhysicalKey::Code(KeyCode::ShiftRight)
                    ) {
                        return;
                    }
                }
                if self.screen.inventory_open() {
                    if p {
                        // M93t — the anvil's name field runs FIRST and, while
                        // it can consume input, swallows everything but Escape.
                        // Vanilla's `AnvilScreen.keyPressed` reaches `super`
                        // only when the box neither handled the key nor could
                        // have, so with an item in slot 0 no screen shortcut
                        // fires at all.
                        let items = self.items.clone();
                        let mods = (i32::from(self.shift))
                            | (i32::from(self.ctrl) << 1)
                            | (i32::from(self.alt) << 2);
                        if let (Some(session), Some(key)) =
                            (self.session.as_mut(), glfw_key(event.physical_key))
                        {
                            let mut clip = std::mem::take(&mut self.clipboard);
                            // M99 — the book's search field first, when it has
                            // focus. Only one field can be focused at a time
                            // (a click on the book unfocuses nothing else, but
                            // the anvil's is only reachable while an anvil is
                            // open and the book's only while the book is), so
                            // the order is belt-and-braces rather than a
                            // contract.
                            if self.screen.book_search.is_focused() {
                                let input = rewo_ui::edit_box::Input::new(key, mods);
                                let consumed =
                                    self.screen.book_search.key_pressed(input, &mut clip);
                                self.clipboard = clip;
                                if consumed {
                                    follow_cursor(
                                        &mut self.screen.book_search,
                                        self.baked.as_ref(),
                                        rewo_ui::recipe_book_screen::SEARCH_INNER_W,
                                    );
                                    return;
                                }
                                clip = self.clipboard.clone();
                            }
                            let consumed = anvil_key(
                                session,
                                &mut self.screen,
                                &items,
                                self.baked.as_ref(),
                                rewo_ui::edit_box::Input::new(key, mods),
                                &mut clip,
                            );
                            self.clipboard = clip;
                            if consumed {
                                return;
                            }
                        }
                        // A typed character. winit reports it separately from
                        // the key, which is exactly the seam Rewo never read —
                        // `KeyEvent.text`, not `PhysicalKey`.
                        if let (Some(session), Some(text)) =
                            (self.session.as_mut(), event.text.as_ref())
                        {
                            let mut any = false;
                            for ch in text.chars() {
                                if self.screen.book_search.is_focused()
                                    && self.screen.book_search.char_typed(ch)
                                {
                                    any = true;
                                    follow_cursor(
                                        &mut self.screen.book_search,
                                        self.baked.as_ref(),
                                        rewo_ui::recipe_book_screen::SEARCH_INNER_W,
                                    );
                                }
                                any |= anvil_char(
                                    session,
                                    &mut self.screen,
                                    &items,
                                    self.baked.as_ref(),
                                    ch,
                                );
                            }
                            if any {
                                return;
                            }
                        }
                        if let Some(action) = screen_key_action(event.physical_key, self.ctrl) {
                            let ext = self.state.as_ref().map(|s| s.window.inner_size());
                            let items = self.items.clone();
                            if let (Some(session), Some(ext)) = (self.session.as_mut(), ext) {
                                click_screen(
                                    session,
                                    &items,
                                    &mut self.screen,
                                    action,
                                    ext.width as f32,
                                    ext.height as f32,
                                );
                            }
                            return;
                        }
                    }
                    if !matches!(
                        event.physical_key,
                        PhysicalKey::Code(KeyCode::Escape)
                            | PhysicalKey::Code(KeyCode::KeyE)
                            | PhysicalKey::Code(KeyCode::ShiftLeft)
                            | PhysicalKey::Code(KeyCode::ShiftRight)
                    ) {
                        return;
                    }
                }
                // M82: a non-inventory screen owns the keyboard.
                //
                // `Screen.keyPressed`'s order is Esc → the focused widget →
                // Tab; the inventory is exempt above only because its own
                // `keyPressed` override runs first and it has no widgets to
                // focus. The three debug keys still get through, which is
                // vanilla's arrangement too — `KeyboardHandler.keyPress`
                // handles the screenshot and debug keys before it hands the
                // event to `minecraft.gui.screen()`. **Esc is not on that
                // list**, so a death screen (`shouldCloseOnEsc() == false`)
                // swallows it and `rewo live` does not quit.
                if self.screen.any_open() && !self.screen.inventory_open() {
                    // M173: Left/Right on a FOCUSED options slider steps by
                    // one handle-pixel (`1/(width-8)` — the 310- and 150-wide
                    // sliders step differently by design). Before the generic
                    // dispatch, which never consumes the arrows.
                    if p && self.options_view.is_some() {
                        if let Some(right) = match glfw_key(event.physical_key) {
                            Some(263) => Some(false),
                            Some(262) => Some(true),
                            _ => None,
                        } {
                            let changed = self
                                .screen
                                .screens
                                .current_mut()
                                .and_then(|sc| sc.slider_key(right));
                            if let Some((id, v)) = changed {
                                self.options_slider_changed(id, v);
                            }
                            return;
                        }
                    }
                    // M172: the book's page keys — GLFW 266 PageUp -> BACK,
                    // 267 PageDown -> FORWARD (the pairing reads inverted and
                    // is vanilla's). Only these two; the arrows are NOT bound.
                    // Checked before the generic dispatch, which never
                    // consumes them (it handles Esc / Enter / Space / Tab).
                    if p && self.book.is_some() {
                        if let Some(k @ (266 | 267)) = glfw_key(event.physical_key) {
                            // M184: the lectern's page keys are sends too —
                            // the same buttons the arrows send.
                            if self.lectern_shown.is_some() {
                                let button = if k == 266 {
                                    rewo_ui::book_view_screen::LECTERN_PAGE_BACK
                                } else {
                                    rewo_ui::book_view_screen::LECTERN_PAGE_FORWARD
                                };
                                if let Some(session) = self.session.as_mut() {
                                    if let Err(e) = session.container_button_click(button) {
                                        log::warn!("live: lectern page key: {e}");
                                    }
                                }
                            } else if let Some(b) = self.book.as_mut() {
                                b.key(k);
                            }
                            return;
                        }
                    }
                    // M178: the advancements key closes the screen while it is
                    // up — `keyAdvancements.matches(event)` → `setScreen(null)`
                    // + `grabMouse()`. Vanilla default IS L.
                    if p && self.advancements.is_some() {
                        if let Some(76) = glfw_key(event.physical_key) {
                            self.close_advancements();
                            return;
                        }
                    }
                    if p {
                        let shift = self.shift;
                        let result = glfw_key(event.physical_key).and_then(|k| {
                            self.screen.screens.current_mut().map(|s| {
                                // `ConfirmScreen.keyPressed`: Esc is the "No"
                                // answer, ahead of `Screen.keyPressed`.
                                if s.kind == rewo_ui::screen::ScreenKind::ConfirmLink {
                                    (
                                        s.kind,
                                        rewo_ui::confirm_link_screen::key_pressed(s, k, shift),
                                    )
                                } else {
                                    (s.kind, s.key_pressed(k, shift))
                                }
                            })
                        });
                        match result {
                            Some((kind, rewo_ui::screen::KeyResult::Pressed(id))) => {
                                // M186 — `AbstractButton.keyPressed` plays the
                                // click before `onPress`. Not for the link
                                // prompt's Esc: `ConfirmScreen.keyPressed`
                                // answers "No" through the callback, never
                                // through the button.
                                let esc_answer = kind == rewo_ui::screen::ScreenKind::ConfirmLink
                                    && glfw_key(event.physical_key) == Some(256);
                                if !esc_answer {
                                    self.queue_down_sound(id);
                                }
                                self.press_widget(kind, id);
                                return;
                            }
                            Some((kind, rewo_ui::screen::KeyResult::Close)) => {
                                // `onClose()`. For a dialog that is
                                // `DialogAction.CLOSE` → the previous screen,
                                // which is the pause screen it was opened
                                // from; for everything else it is
                                // `setScreen(null)`.
                                if kind == rewo_ui::screen::ScreenKind::ServerLinks {
                                    self.open_pause_screen();
                                } else if kind == rewo_ui::screen::ScreenKind::Stats {
                                    // M84: a screen with per-screen state must
                                    // drop it here too, or `pump_stats_screen`
                                    // re-opens the screen Esc just closed.
                                    self.close_stats();
                                } else if kind == rewo_ui::screen::ScreenKind::BookView {
                                    // M172: same rule — the render arm keys on
                                    // `self.book`, so it drops with the screen.
                                    self.close_book();
                                } else if kind == rewo_ui::screen::ScreenKind::SignEdit {
                                    // M174: Esc COMMITS (belt-and-braces — the
                                    // sign arm consumes Esc before this
                                    // dispatch can see it).
                                    self.close_sign_edit();
                                } else if kind == rewo_ui::screen::ScreenKind::Options
                                    && self.options_view.is_some()
                                {
                                    // M173: Esc = onClose — a sub-page returns
                                    // to the root, the root to the pause
                                    // screen, and leaving a sub-page saves.
                                    self.close_options();
                                } else {
                                    self.close_view_screen();
                                }
                                return;
                            }
                            Some((_, rewo_ui::screen::KeyResult::Handled)) => return,
                            _ => {}
                        }
                    }
                    if !matches!(
                        event.physical_key,
                        PhysicalKey::Code(KeyCode::F1)
                            | PhysicalKey::Code(KeyCode::F2)
                            | PhysicalKey::Code(KeyCode::F3)
                    ) {
                        return;
                    }
                }
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyW) => self.keys.w = p,
                    PhysicalKey::Code(KeyCode::KeyA) => self.keys.a = p,
                    PhysicalKey::Code(KeyCode::KeyS) => self.keys.s = p,
                    PhysicalKey::Code(KeyCode::KeyD) => self.keys.d = p,
                    PhysicalKey::Code(KeyCode::Space) => self.keys.jump = p,
                    // M151 — Tab holds the player list up. Only the press
                    // reaches here (the release is handled above, before the
                    // screen gates); assigning `p` rather than `true` is
                    // harmless and keeps the arm reading like its neighbours.
                    PhysicalKey::Code(KeyCode::Tab) => self.keys.tab_list = p,
                    // M52 Toggle Sneak: with the module on, Shift *flips*
                    // sneak instead of holding it. `!event.repeat` is load
                    // bearing -- the OS auto-repeats a held key, and without
                    // the guard holding Shift would flip sneak dozens of times
                    // a second rather than once.
                    PhysicalKey::Code(KeyCode::ShiftLeft) => {
                        if self.modules.is_on("toggle_sneak") {
                            if p && !event.repeat {
                                self.keys.sneak = !self.keys.sneak;
                            }
                        } else {
                            self.keys.sneak = p;
                        }
                        self.shift = p;
                    }
                    PhysicalKey::Code(KeyCode::ShiftRight) => self.shift = p,
                    // M52 Toggle Sprint, same shape as Toggle Sneak.
                    PhysicalKey::Code(KeyCode::ControlLeft) => {
                        if self.modules.is_on("toggle_sprint") {
                            if p && !event.repeat {
                                self.keys.sprint = !self.keys.sprint;
                            }
                        } else {
                            self.keys.sprint = p;
                        }
                    }
                    // M52 Zoom: a HELD key, not a toggle -- every one of the
                    // 54 zoom mods in the survey binds it that way. C is
                    // Zoomify's default. The divide happens in
                    // `Modules::render` so it composes with FOV Control.
                    PhysicalKey::Code(KeyCode::KeyC) => self.modules.set_zoom_held(p),
                    // Esc closes the inventory if it is open, opens the
                    // **pause screen** if a session is running, and only quits
                    // otherwise (M85 — before it, Esc quit outright).
                    //
                    // A non-inventory screen never reaches this arm: the block
                    // above hands Esc to `Screen.keyPressed`, whose
                    // `shouldCloseOnEsc()` decides. That is why the pause
                    // screen closes on Esc and the death and disconnect
                    // screens do not.
                    PhysicalKey::Code(KeyCode::Escape) if p => {
                        if self.screen.inventory_open() {
                            // `AbstractContainerScreen.onClose` →
                            // `player.closeContainer()`.
                            self.close_container_screen();
                        } else if self.session.is_some() {
                            self.open_pause_screen();
                        } else {
                            event_loop.exit();
                        }
                    }
                    PhysicalKey::Code(KeyCode::Escape) => {}
                    // F6 opens and closes the statistics screen (M84).
                    //
                    // **A Rewo-specific binding.** Vanilla reaches this screen
                    // from the pause menu's `Statistics` button; M85's pause
                    // screen transcribes `PauseScreen`'s own grid, which does
                    // not carry one (`StatsScreen` is reached from the
                    // *singleplayer* pause menu's second row, which M85's
                    // multiplayer transcription omits). A key is the interim
                    // route rather than a claim about vanilla's input.
                    PhysicalKey::Code(KeyCode::F6) if p && !event.repeat => {
                        if self.stats.is_some() {
                            self.close_stats();
                        } else {
                            self.open_stats();
                        }
                    }
                    // M178: L opens the advancements screen — this one IS
                    // vanilla's binding (`keyAdvancements`, default L), so no
                    // Rewo-specific caveat is needed, unlike F6 above.
                    PhysicalKey::Code(KeyCode::KeyL) if p && !event.repeat => {
                        if self.advancements.is_some() {
                            self.close_advancements();
                        } else if self.session.is_some() {
                            self.open_advancements();
                        }
                    }
                    // T and `/` open the chat screen (M110).
                    //
                    // Two keys, one screen, differing only in the prefix the
                    // field starts with — and in which drafts they will
                    // restore, which is `ChatMethod.isDraftRestorable` and not
                    // symmetric. The routing branch above returns before this
                    // whenever a chat screen is already open, so `T` cannot
                    // both open the screen and be typed into it.
                    PhysicalKey::Code(KeyCode::KeyT) if p => {
                        self.open_chat_screen(rewo_ui::chat_screen::ChatMethod::Message);
                    }
                    PhysicalKey::Code(KeyCode::Slash) if p => {
                        self.open_chat_screen(rewo_ui::chat_screen::ChatMethod::Command);
                    }
                    // E opens and closes the inventory (M35).
                    PhysicalKey::Code(KeyCode::KeyE) if p => self.inventory_key(),
                    // F3 is a **modifier** whose release toggles the overlay
                    // (M66). `keyDebugModifier` and `keyDebugOverlay` are the
                    // same key, so `KeyboardHandler` defers the toggle to
                    // `action == 0` and skips it when a chord already fired:
                    //
                    //   if (usedDebugKeyAsModifier) usedDebugKeyAsModifier = false;
                    //   else                        toggleDebugOverlay();
                    //
                    // Toggling on press instead would flip the overlay every
                    // time you pressed F3+H.
                    // F1 — `Hud.toggle()` (M70). On **press**, and with no
                    // modifier dance: F1 is not also a chord prefix, so unlike
                    // F3 there is nothing to disambiguate. `!event.repeat`
                    // guards the OS auto-repeat, which would otherwise flip it
                    // dozens of times a second while held.
                    PhysicalKey::Code(KeyCode::F1) if p && !event.repeat => {
                        self.hud_hidden = !self.hud_hidden;
                        log::info!("hud.{}", if self.hud_hidden { "hidden" } else { "shown" });
                    }
                    PhysicalKey::Code(KeyCode::F3) => {
                        self.f3_down = p;
                        if !p {
                            if self.f3_used_as_modifier {
                                self.f3_used_as_modifier = false;
                            } else {
                                self.debug = !self.debug;
                            }
                        }
                    }
                    // F3+H — `keyDebugShowAdvancedTooltips`.
                    PhysicalKey::Code(KeyCode::KeyH) if p && self.f3_down => {
                        self.advanced_tooltips = !self.advanced_tooltips;
                        self.f3_used_as_modifier = true;
                        log::info!(
                            "debug.advanced_tooltips.{}",
                            if self.advanced_tooltips { "on" } else { "off" }
                        );
                    }
                    // F2 captures a screenshot — vanilla's `keyScreenshot`,
                    // GLFW key 291. Only the request is recorded here; the
                    // capture itself happens after the frame.
                    PhysicalKey::Code(KeyCode::F2) if p => self.capture_pending = true,
                    // M218 — `keyDrop` (Q; with Ctrl the whole stack) and
                    // `keySwapOffhand` (F), both refused to a spectator.
                    PhysicalKey::Code(KeyCode::KeyQ) if p => self.drop_key(),
                    PhysicalKey::Code(KeyCode::KeyF) if p => self.swap_hands_key(),
                    // Number keys 1..9 select the hotbar slot (HUD frame +
                    // sent to the server so the held item matches).
                    PhysicalKey::Code(code) if p => {
                        if let Some(n) = digit_key(code) {
                            if let Some(s) = self.session.as_mut() {
                                let _ = s.select_hotbar(n);
                            }
                        }
                    }
                    _ => {}
                }
            }
            // M128 — a click on chat text, while the chat screen is open.
            // Ahead of every other mouse arm for the reason the keyboard block
            // above gives: `Gui.screen` is ONE slot, so with a `ChatScreen` in
            // it there is no other screen to route to and no world to dig.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if self.chat_screen.is_some() => {
                let b = match button {
                    MouseButton::Left => 0u8,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                let click = self.chat_mouse_pressed(b);
                // M219 — `super.mouseClicked`, after the chat's own text:
                // the in-bed screen's Leave Bed button.
                if b == 0 && click == rewo_ui::chat_screen::ChatClick::NotHandled {
                    self.in_bed_click();
                }
            }
            // M82: a click on a widget-bearing screen. Before the inventory's
            // arm and before the world's, because
            // `ContainerEventHandler.mouseClicked` returns **true whenever
            // `getChildAt` found something** — the child's own answer only
            // decides whether it was *pressed*. So a right-click on a button
            // is eaten by the screen and never digs.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if self.screen.any_open() && !self.screen.inventory_open() => {
                let (mx, my) = self.mouse_gui();
                let b = match button {
                    MouseButton::Left => 0u8,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                let pressed = self
                    .screen
                    .screens
                    .current_mut()
                    .map(|s| (s.kind, s.mouse_clicked(mx, my, b)));
                // M179 — the advancements drag arms on ANY press over the
                // open screen (`activeButton` is set before the screen sees
                // it), whatever the click went on to do.
                if self.advancements.is_some() {
                    self.adv_drag.press(b);
                }
                if let Some((kind, rewo_ui::screen::MouseResult::Pressed(id))) = pressed {
                    // M186 — `AbstractWidget.mouseClicked`: `playDownSound`,
                    // then `onClick`.
                    self.queue_down_sound(id);
                    self.press_widget(kind, id);
                } else if let Some((_, rewo_ui::screen::MouseResult::Slider(id, v))) = pressed {
                    // M173: `onClick` -> `setValueFromMouse` — the press IS
                    // the first value change, and it arms the drag.
                    self.options_slider_changed(id, v);
                    self.options_drag = Some(id);
                } else if b == 0
                    && matches!(pressed, Some((rewo_ui::screen::ScreenKind::BookView, _)))
                {
                    // M180: the page-text click events run FIRST — vanilla's
                    // `BookViewScreen.mouseClicked` walks the
                    // `ClickableStyleFinder` ahead of super's widget pass
                    // (`:215-226`). The two never overlap geometrically (text
                    // sits above the arrow strip), so ordering is invisible,
                    // but it is vanilla's order.
                    let gw = self.gui_size().0;
                    if !self.book_page_click(mx, my, gw) {
                        // M172: the page arrows are not framework widgets (a
                        // `PageButton` draws its own sprites, not button
                        // chrome), so a left click that is neither a text
                        // event nor a framework press lands here.
                        // `BookViewScreen::click` only turns on a VISIBLE
                        // arrow.
                        //
                        // M184: the lectern's arrows are SENDS
                        // (`sendButtonClick(1/2)`), not local turns — the
                        // server's data echo moves the page.
                        if self.lectern_shown.is_some() {
                            self.lectern_arrow_click(mx, my, gw);
                        } else if let Some(book) = self.book.as_mut() {
                            // M186 — a held book's `PageButton` has
                            // `playTurnSound`; the lectern's (above) does not,
                            // because `LecternScreen` builds its reader with
                            // the no-argument `BookViewScreen()` and the
                            // server plays the turn instead.
                            if book.click(mx as i32, my as i32, gw) {
                                self.screen
                                    .ui_sounds
                                    .play(rewo_ui::ui_sound::UiSound::PageTurn);
                            }
                        }
                    }
                } else if b == 0
                    && matches!(
                        pressed,
                        Some((rewo_ui::screen::ScreenKind::Advancements, _))
                    )
                {
                    // M179: the tab strip. Not framework widgets (vanilla's
                    // loop runs in the SCREEN's mouseClicked, ahead of
                    // super's widget walk — `AdvancementsScreen.java:113-127`),
                    // so a left click the framework did not press lands here,
                    // exactly the book-arrow seam. Vanilla falls through to
                    // super afterwards; the two never overlap (tabs sit
                    // outside the window, Done inside its footer).
                    self.advancements_tab_click(mx, my);
                }
            }
            WindowEvent::MouseInput {
                state: btn, button, ..
            } if btn == ElementState::Pressed && self.screen.inventory_open() => {
                // A click on the screen moves items; it never digs or places.
                let ext = self.state.as_ref().map(|s| s.window.inner_size());
                let items = self.items.clone();
                if let (Some(session), Some(ext)) = (self.session.as_mut(), ext) {
                    let b = match button {
                        MouseButton::Left => 0,
                        MouseButton::Right => 1,
                        _ => return,
                    };
                    // M98 — the recipe book is pressed before EVERYTHING:
                    // `AbstractRecipeBookScreen.mouseClicked` runs the book and
                    // only calls `super` when the book declines.
                    // The bake's display names, or an empty map when there is
                    // no bake — in which case `search_entry_of` falls back to
                    // the id's prettified path, which is the same fallback the
                    // tooltip takes. Degrading rather than disabling the book's
                    // clicks.
                    static NO_NAMES: std::sync::OnceLock<
                        std::collections::HashMap<String, String>,
                    > = std::sync::OnceLock::new();
                    let display = self
                        .baked
                        .as_ref()
                        .map(|b| &b.item_names)
                        .unwrap_or_else(|| NO_NAMES.get_or_init(Default::default));
                    if book_press(
                        session,
                        &mut self.screen,
                        &items,
                        // M107 — `event.hasShiftDown()`, straight through to
                        // `useMaxItems`.
                        self.shift,
                        display,
                        b == 1,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // M187 — the recipe-book button: a screen widget, so it is
                    // `super.mouseClicked`'s first act once the book declined.
                    if book_button_press(
                        session,
                        &mut self.screen,
                        b == 0,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // M92f — an enchanting row is pressed BEFORE the slot
                    // logic, and only then. `EnchantmentScreen.mouseClicked`
                    // runs its three-row loop first and calls
                    // `super.mouseClicked` only when no row took the press, so
                    // a click on a *disabled* row still falls through to the
                    // normal slot handling (which finds nothing there).
                    if enchant_press(session, &self.screen, ext.width as f32, ext.height as f32) {
                        return;
                    }
                    // M93m — the beacon's buttons, on the same seam and for
                    // the same reason: a live widget consumes the click and
                    // it never reaches the slot logic, while a DARK one falls
                    // through exactly as a disabled enchanting row does.
                    if beacon_press(
                        session,
                        &mut self.screen,
                        &self.beacon_effects,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // M93s — the stonecutter's grid. Same seam, and vanilla's
                    // own order: the recipe loop runs first and returns true
                    // on a hit, then the scrollbar's grab box sets `scrolling`
                    // and DOES NOT consume the press — it falls through to
                    // `super.mouseClicked`, so a grab still reaches the slots.
                    if cut_press(
                        session,
                        &mut self.screen,
                        &items,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // M186 — the loom's pattern grid, the stonecutter's
                    // shape: a pattern consumes the press.
                    if loom_press(
                        session,
                        &mut self.screen,
                        &items,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // M93u — the merchant's trade buttons, on the same seam.
                    if merchant_press(
                        session,
                        &mut self.screen,
                        ext.width as f32,
                        ext.height as f32,
                    ) {
                        return;
                    }
                    // `AbstractContainerScreen.mouseClicked`'s double click:
                    // the **same slot**, the **left** button, and under 250 ms
                    // since the last one. Not "two clicks anywhere in
                    // 250 ms" — moving to a neighbouring slot resets it.
                    let layout = session.shown_menu().layout();
                    let slot = self.screen.hovered(
                        layout,
                        ext.width as f32,
                        ext.height as f32,
                        book_visible(session),
                    );
                    let now = std::time::Instant::now();
                    let doubled = b == 0
                        && slot.is_some()
                        && self.last_click == slot
                        && now.duration_since(self.last_click_at).as_millis() < 250;
                    self.last_click = slot;
                    self.last_click_at = now;
                    // With a stack already on the cursor a press starts a
                    // drag rather than a click. The two are told apart at
                    // *release*: a drag that never left its slot collapses
                    // back into the click it looks like, which is exactly what
                    // vanilla's one-slot special case does.
                    if session.inventory.carried().is_some() && !self.shift && !doubled {
                        self.drag.begin(b);
                        if let Some(slot) = slot {
                            self.drag.add(slot);
                        }
                        return;
                    }
                    let action = if doubled {
                        SlotAction::PickupAll
                    } else if self.shift {
                        SlotAction::QuickMove
                    } else {
                        SlotAction::Pickup(b)
                    };
                    click_screen(
                        session,
                        &items,
                        &mut self.screen,
                        action,
                        ext.width as f32,
                        ext.height as f32,
                    );
                }
            }
            // M173 — releasing ends a slider drag, unconditionally (the
            // M93s rule: a screen that vanished mid-drag must not strand the
            // grab). M186: and `AbstractSliderButton.onRelease` plays the
            // click here — a slider is the one widget that clicks on release
            // rather than on press.
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } if self.options_drag.is_some() => {
                self.options_drag = None;
                self.screen
                    .ui_sounds
                    .play(rewo_ui::ui_sound::UiSound::ButtonClick);
            }
            // M179 — any release ends the advancements drag
            // (`AdvancementsScreen.mouseReleased`: `isScrolling = false`,
            // unconditionally). Ahead of the right-button arm on purpose: a
            // right-release with the screen up must still clear the state,
            // and no `use` can be in flight because every press over an open
            // screen is consumed before the world's arms are reached.
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } if self.advancements.is_some() => {
                self.adv_drag.release();
            }
            // Releasing a button over the open screen ends any drag (M40).
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } if self.screen.inventory_open() => {
                let items = self.items.clone();
                // `StonecutterScreen.mouseReleased` clears `scrolling`
                // unconditionally — not gated on `displayRecipes`, so a list
                // that vanishes mid-drag does not strand the grab (M93s).
                if let Some(c) = self.screen.cut.as_mut() {
                    c.scrolling = false;
                }
                // M189 — `LoomScreen.mouseReleased`, the same unconditional
                // clear.
                if let Some(l) = self.screen.loom.as_mut() {
                    l.scrolling = false;
                }
                if let Some(m) = self.screen.merchant.as_mut() {
                    m.dragging = false;
                }
                if let Some(session) = self.session.as_mut() {
                    finish_drag(session, &items, &mut self.drag);
                }
            }
            // M217 — releasing the attack key: `continueAttack(false)` →
            // `stopDestroyBlock`, which aborts a block in progress.
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                self.attack_held = false;
                if let Some(a) = self.mining.stop() {
                    self.apply_mining(vec![a]);
                }
            }
            // Releasing the right button: `keyUse` is up, so the next tick's
            // `handleKeybinds` releases a use in progress (M229's
            // `use_tick`) — eating stops, a bow fires, a shield drops.
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Right,
                ..
            } => {
                self.use_down = false;
                if let Some(session) = self.session.as_mut() {
                    let _ = session.stop_use();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                // Left-click digs the targeted block; right-click places
                // against its hit face, or — with nothing to place against —
                // starts *using* the held item (M38).
                // M218 — the pick key (middle button): `pickBlockOrEntity`.
                if button == MouseButton::Middle {
                    self.pick_key();
                    return;
                }
                // M216 — `GameRenderer.pick`'s ENTITY result wins over the
                // block when it is nearer (the M73 pick already reconciles
                // the two), and the clicks take `startAttack` /
                // `startUseItem`'s ENTITY arms.
                if self.entity_click(button) {
                    return;
                }
                // M229 — `keyUse.consumeClick()` -> `startUseItem`, unless an
                // item is already in use (`handleKeybinds` drains the clicks
                // then). Held, `use_tick` repeats it every 4 ticks.
                if button == MouseButton::Right {
                    self.use_key_press();
                }
                // M217 — the attack key on a block: `startAttack`'s BLOCK arm
                // (`startDestroyBlock`), and the key stays held for
                // `continueAttack` each tick.
                if button == MouseButton::Left {
                    self.attack_held = true;
                    self.mining_press();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.screen.mouse = (position.x, position.y);
                // M173 — `onDrag` -> `setValueFromMouse`, continuous: every
                // moved pixel applies (vanilla's `applyValueImmediately` is
                // true for a volume slider; release does NOT re-apply).
                if let Some(id) = self.options_drag {
                    let (mx, _) = self.mouse_gui();
                    let changed = self
                        .screen
                        .screens
                        .current_mut()
                        .and_then(|sc| sc.slider_drag(id, mx));
                    if let Some(v) = changed {
                        self.options_slider_changed(id, v);
                    }
                }
                // M93s — `mouseDragged` while the stonecutter's thumb is held.
                // Guarded on `isScrollBarActive` as well as `scrolling`, so a
                // list that shrinks under a held thumb stops moving.
                self.cut_drag();
                self.loom_drag();
                self.merchant_drag();
                // M179 — the advancements drag. Every move feeds the machine
                // (it tracks the previous sample whether or not it delivers,
                // like vanilla's per-frame accumulator), and the delta it
                // returns is already in GUI px.
                if self.advancements.is_some() {
                    let scale = self
                        .state
                        .as_ref()
                        .map(|s| s.window.inner_size())
                        .map(|sz| {
                            rewo_gpu::hud::gui_scale(sz.width as f32, sz.height as f32) as f64
                        })
                        .unwrap_or(1.0);
                    if let Some((dx, dy)) =
                        self.adv_drag.cursor_moved((position.x, position.y), scale)
                    {
                        if let Some(v) = self.advancements.as_mut() {
                            // Raw deltas — `mouseScrolled` is the path that
                            // scales by SCROLL_SPEED; `mouseDragged` does not.
                            v.drag_scroll(dx, dy);
                        }
                    }
                }
                // Crossing a slot with the button down extends a drag. The
                // slot only *joins* it if the server would accept it, and that
                // is decided at release — this records the path.
                if self.screen.inventory_open() {
                    if let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) {
                        let layout = self.shown_layout();
                        let book_open = self.session.as_ref().is_some_and(book_visible);
                        if let Some(slot) = self.screen.hovered(
                            layout,
                            ext.width as f32,
                            ext.height as f32,
                            book_open,
                        ) {
                            self.drag.add(slot);
                        }
                    }
                }
            }
            // M84: the scroll wheel drives whichever list is up.
            //
            // `AbstractScrollArea.mouseScrolled` is
            // `setScrollAmount(scrollAmount() - scrollY * scrollRate())`, and
            // the **minus** is the whole of it: a positive `scrollY` (wheel
            // away from you) moves the list toward row 0. winit reports a notch
            // as `LineDelta(_, 1.0)`, which is GLFW's own unit, so the two need
            // no conversion; a trackpad's `PixelDelta` is divided by a line's
            // height first.
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x as f64, y as f64),
                    winit::event::MouseScrollDelta::PixelDelta(p) => (p.x / 16.0, p.y / 16.0),
                };
                if let Some(view) = self.stats.as_mut() {
                    view.model.list_mut().mouse_scrolled(dy);
                }
                // M93s — and the stonecutter's grid. `mouseScrolled` returns
                // true whether or not the bar is active, so the screen
                // swallows every notch; only an active bar moves.
                self.cut_wheel(dy);
                self.loom_wheel(dy);
                self.merchant_wheel(dy);
                // M179 — `AdvancementsScreen.mouseScrolled`: both axes scale
                // by SCROLL_SPEED inside `wheel`, and the screen consumes the
                // notch whenever a tab is selected.
                if let Some(v) = self.advancements.as_mut() {
                    v.wheel(dx, dy);
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if self.screen.any_open() || self.chat_screen.is_some() {
            // The cursor is free and driving the screen (the chat screen is
            // not in `self.screen`, so it is checked on its own); the camera
            // holds still. `CursorMoved` supplies the position, so this raw
            // delta is simply dropped rather than accumulated.
            return;
        }
        if let (DeviceEvent::MouseMotion { delta }, Some(session)) = (&event, self.session.as_mut())
        {
            // Mouse drives the player's look (what we render AND send).
            session.player.yaw += delta.0 as f32 * 0.15;
            session.player.pitch =
                (session.player.pitch - delta.1 as f32 * 0.15).clamp(-89.0, 89.0);
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

impl LiveApp {
    pub(super) fn merchant_wheel(&mut self, dy: f64) {
        use rewo_ui::merchant_screen as ms;
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.shown_open() else {
            return;
        };
        if open.layout.protocol_id != ms::MERCHANT_MENU_PROTOCOL_ID {
            return;
        }
        let n = session.merchant.as_ref().map_or(0, |m| m.offers.len());
        if !ms::can_scroll(n) {
            return;
        }
        if let Some(l) = self.screen.merchant.as_mut() {
            l.scroll_off = ms::scroll_off_from_wheel(l.scroll_off, dy, n);
        }
    }

    /// `StonecutterScreen.mouseScrolled` (M93s) — one notch is one ROW, so a
    /// long list scrolls proportionally slower per notch.
    ///
    /// The sign is vanilla's `scrollOffs - scrollY / offscreenRows`, the same
    /// minus M84 records for `AbstractScrollArea`.
    pub(super) fn cut_wheel(&mut self, dy: f64) {
        use rewo_ui::menu_screen as ms;
        if self.screen.cut.is_none() {
            return;
        }
        let items = self.items.clone();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.shown_open() else {
            return;
        };
        if open.layout.protocol_id != ms::STONECUTTER_MENU_PROTOCOL_ID {
            return;
        }
        let name = open.menu.menu_slot(0).and_then(|s| items.name(s.item_id));
        let visible = name.map_or(0, |n| {
            rewo_data::stonecutter_table::select_by_input(n).len()
        });
        if !ms::cut_scroll_active(ms::cut_display_recipes(name.is_some(), visible), visible) {
            return;
        }
        if let Some(c) = self.screen.cut.as_mut() {
            c.scroll_offs = ms::cut_scroll_offs_from_wheel(c.scroll_offs, dy, visible);
        }
    }

    /// `LoomScreen.mouseScrolled` (M189): one notch is one row, only while
    /// `displayPatterns && offscreenRows > 0`.
    pub(super) fn loom_wheel(&mut self, dy: f64) {
        let items = self.items.clone();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.shown_open() else {
            return;
        };
        let Some(view) = loom_view_scrolled(&mut self.screen, open, &items, &session.stack_details)
        else {
            return;
        };
        if !rewo_ui::menu_screen::loom_can_scroll(view.display, view.patterns.len()) {
            return;
        }
        if let Some(l) = self.screen.loom.as_mut() {
            let (offs, row) = rewo_ui::menu_screen::loom_scroll_from_wheel(
                l.scroll_offs,
                dy,
                view.patterns.len(),
            );
            l.scroll_offs = offs;
            l.start_row = row;
        }
    }

    /// `LoomScreen.mouseDragged` while the thumb is held (M189), guarded on
    /// `scrolling && displayPatterns && offscreenRows > 0` like vanilla.
    pub(super) fn loom_drag(&mut self) {
        if !self.screen.loom.is_some_and(|l| l.scrolling) {
            return;
        }
        let Some(ext) = self.state.as_ref().map(|s| s.window.inner_size()) else {
            return;
        };
        let items = self.items.clone();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(open) = session.menus.shown_open() else {
            return;
        };
        let Some(view) = loom_view_scrolled(&mut self.screen, open, &items, &session.stack_details)
        else {
            return;
        };
        if !rewo_ui::menu_screen::loom_can_scroll(view.display, view.patterns.len()) {
            return;
        }
        let (_, gy) = rewo_gpu::container::screen_to_gui_for(
            self.screen.mouse,
            ext.width as f32,
            ext.height as f32,
            open.layout.image_w as f32,
            open.layout.image_h as f32,
        );
        if let Some(l) = self.screen.loom.as_mut() {
            let (offs, row) = rewo_ui::menu_screen::loom_scroll_from_drag(gy, view.patterns.len());
            l.scroll_offs = offs;
            l.start_row = row;
        }
    }

    /// Falls back to `PLAYER` with no session, which is what the screen shows
    /// before a connection anyway.
    pub(super) fn shown_layout(&self) -> &'static rewo_world::menu_layout::MenuLayout {
        self.session
            .as_ref()
            .map(|s| s.shown_menu().layout())
            .unwrap_or(&rewo_world::menu_layout::PLAYER)
    }

    /// A USER-initiated close of the inventory / container screen
    /// (`AbstractContainerScreen.onClose` → `LocalPlayer.closeContainer()`,
    /// `LocalPlayer.java:333-341`): sends `container_close` for an open
    /// container, drops the menu so the next inventory open shows the
    /// player's own layout, then closes the screen.
    ///
    /// **Not for the server's close** (`handleContainerClose`, which is the
    /// client half alone): that path, in `pump_screens`, calls
    /// `set_screen_open(false)` and sends nothing.
    ///
    /// `container_shown` is reset with the menu: it is a watermark on
    /// `menus.open()`, and left at `Some` it would read the next frame as a
    /// server close and shut whatever screen the user opened in between.
    pub(super) fn close_container_screen(&mut self) {
        if let Some(session) = self.session.as_mut() {
            if let Err(e) = session.container_close() {
                log::warn!("live: container_close: {e}");
            }
        }
        self.screen.container_shown = None;
        self.set_screen_open(false);
    }

    pub(super) fn set_screen_open(&mut self, open: bool) {
        // M104 — the which-of-these overlay does not survive the screen.
        // `RecipeBookComponent.setVisible(false)` calls
        // `recipeBookPage.setInvisible()`, and the component's `init` rebuilds
        // the page from scratch — so neither closing a screen nor opening one
        // can leave an overlay stranded over a book that has moved on. Cleared
        // on OPEN as well as close, because it is a snapshot of a page that
        // may no longer exist.
        self.screen.book_overlay = None;
        if open {
            let (gw, gh) = self.gui_size();
            self.screen.screens.open(rewo_ui::screen::Screen::new(
                rewo_ui::screen::ScreenKind::Inventory,
                gw,
                gh,
            ));
        } else {
            self.screen.screens.close();
        }
        self.grab_for_screen(open);
    }

    /// The window in GUI pixels — the space every screen lays its widgets out
    /// in. `(0, 0)` before the window exists, which no screen is ever built
    /// against.
    /// The book reader's framework screen at this GUI size. A lectern's
    /// reader gets the two-button footer (Done + Take Book) when the player
    /// may build — vanilla's `LecternScreen` sets `mayBuild` from
    /// `!isSpectator()`, it is not on the wire; a held book gets Done only.
    /// The open and the resize rebuild both come through here, so a resize
    /// cannot drop the Take Book button.
    pub(super) fn book_view_screen_for(
        &self,
        gw: i32,
        gh: i32,
        lectern: bool,
    ) -> rewo_ui::screen::Screen {
        let done = self.lang.get_or_default("gui.done", "Done").to_string();
        let may_build = !self
            .session
            .as_ref()
            .and_then(|s| s.own_game_mode())
            .is_some_and(rewo_net::play::GameMode::is_spectator);
        if lectern && may_build {
            let take = self
                .lang
                .get_or_default("lectern.take_book", "Take Book")
                .to_string();
            rewo_ui::book_view_screen::build_lectern_screen(gw, gh, &done, &take)
        } else {
            rewo_ui::book_view_screen::build_screen(gw, gh, &done)
        }
    }

    pub(super) fn gui_size(&self) -> (i32, i32) {
        let Some(state) = self.state.as_ref() else {
            return (0, 0);
        };
        let size = state.window.inner_size();
        let px = gui_px(size.width, size.height);
        (
            (size.width as f32 / px) as i32,
            (size.height as f32 / px) as i32,
        )
    }

    /// `Gui.setScreen`'s cursor half — `mouseHandler.releaseMouse()` +
    /// `KeyMapping.releaseAll()` on the way in, `grabMouse()` on the way out.
    ///
    /// Split out of [`Self::set_screen_open`] so a screen that is *not* the
    /// inventory gets exactly the same treatment: the death screen frees the
    /// cursor for the same reason and by the same code.
    pub(super) fn grab_for_screen(&mut self, open: bool) {
        // Every movement key is released, so one held while opening does not
        // stay pressed behind the screen.
        self.keys = Keys::default();
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if open {
            let size = state.window.inner_size();
            self.screen.mouse = (size.width as f64 / 2.0, size.height as f64 / 2.0);
            let _ = state.window.set_cursor_grab(CursorGrabMode::None);
            state.window.set_cursor_visible(true);
        } else {
            let _ = state.window.set_cursor_grab(CursorGrabMode::Confined);
            state.window.set_cursor_visible(false);
        }
    }

    /// The vanilla bitmap font's advance table, when the renderer has one.
    ///
    /// M85's three screens are *laid out* from text widths (a title's
    /// `StringWidget` width, a disconnect reason's wrap), so the builder needs
    /// this before anything is drawn. Windowed, it lives on the entity pass —
    /// `self.baked` was `take()`n in `resumed` and has been `None` ever since
    /// (see the `lang` field's docs).
    pub(super) fn advance(&self) -> Option<[u8; 256]> {
        self.state
            .as_ref()
            .and_then(|s| s.world_renderer.font_advance())
            .copied()
    }

    /// `getCustomTabSuggestions()` — the online players unioned with whatever
    /// `custom_chat_completions` has set, which is what plain chat completes
    /// from. Empty with no session, so an offline harness offers nothing
    /// rather than panicking.
    pub(super) fn tab_words(&self) -> Vec<String> {
        match self.session.as_ref() {
            Some(session) => session
                .suggestions
                .tab_suggestions(session.world.entities.all_names()),
            None => Vec::new(),
        }
    }

    pub(super) fn text_width(&self, text: &str) -> i32 {
        self.advance()
            .map(|a| rewo_gpu::text::width(text, &a))
            .unwrap_or(0)
    }

    /// `PauseScreen(true)` — Esc with a session behind it (M85).
    pub(super) fn open_pause_screen(&mut self) {
        let labels = rewo_ui::pause_screen::PauseLabels::resolve(&self.lang);
        // `!connection.serverLinks().isEmpty()` — the packet's whole effect on
        // this screen. Read from the durable mirror, which is the same value
        // the session holds while it is alive.
        let has_links = !self.server_links.is_empty();
        self.view = ScreenView::Pause(labels, has_links);
        self.rebuild_view_screen();
        self.grab_for_screen(true);
        log::info!("live: pause screen (server links: {has_links})");
    }

    /// The button on the pause screen — `showDialog(Dialogs.SERVER_LINKS)`.
    pub(super) fn open_links_screen(&mut self) {
        let labels = rewo_ui::server_links_screen::ServerLinksLabels {
            title: self
                .lang
                .or_key(rewo_ui::server_links_screen::KEY_TITLE)
                .to_string(),
            back: self
                .lang
                .or_key(rewo_ui::server_links_screen::KEY_BACK)
                .to_string(),
            // `ServerLinks.Entry.displayName()` —
            // `type.map(KnownLinkType::displayName, r -> r)`: the lang map for
            // a known type, the server's own component for a custom one.
            links: self
                .server_links
                .entries()
                .iter()
                .map(|e| match &e.label {
                    rewo_net::server_links::ServerLinkLabel::Known(t) => {
                        self.lang.or_key(&t.lang_key()).to_string()
                    }
                    // M129 — resolved here rather than flattened at the
                    // wire, so a custom label honours the player's language
                    // exactly as the `Known` arm above already does. The two
                    // arms disagreeing was the visible half of the gap.
                    rewo_net::server_links::ServerLinkLabel::Custom(tag) => {
                        rewo_world::chat_translate::chat_component_text(tag, Some(&self.lang))
                    }
                })
                .collect(),
        };
        log::info!("live: server-links dialog ({} link(s))", labels.links.len());
        self.view = ScreenView::Links(labels);
        self.rebuild_view_screen();
        self.grab_for_screen(true);
    }

    /// `createDisconnectScreen` — **the screen with no session behind it.**
    pub(super) fn open_disconnect_screen(
        &mut self,
        cause: rewo_ui::disconnect_screen::DisconnectCause,
        reason: String,
    ) {
        use rewo_net::server_links::KnownLinkType;
        // `serverLinks.findKnownType(BUG_REPORT).map(Entry::link)`, off the
        // durable mirror — the session is about to be dropped, and on the
        // `ClientError` path it may already be unusable.
        let candidate = self
            .server_links
            .find_known_type(KnownLinkType::BugReport)
            .map(|e| e.link.clone());
        let details =
            rewo_ui::disconnect_screen::DisconnectDetails::new(cause, reason, candidate.as_deref());
        let labels = rewo_ui::disconnect_screen::DisconnectLabels::resolve(&self.lang);
        log::warn!(
            "live: disconnected ({cause:?}): {} — bug report link: {:?}",
            details.reason,
            details.bug_report_link
        );
        self.view = ScreenView::Disconnected(labels, details);
        self.session = None;
        self.rebuild_view_screen();
        self.grab_for_screen(true);
    }

    /// `Screen.resize` → `repositionElements` → `rebuildWidgets` → `init()`,
    /// for whichever of M85's screens is up. Also the opener: building a
    /// screen and rebuilding it are the same call, which is exactly vanilla's
    /// arrangement and the reason `ScreenView` carries what it does.
    pub(super) fn rebuild_view_screen(&mut self) {
        let (gw, gh) = self.gui_size();
        if gw <= 0 || gh <= 0 {
            return;
        }
        let advance = self.advance();
        let screen = match &self.view {
            ScreenView::None => return,
            ScreenView::Pause(labels, has_links) => {
                let tw = self.text_width(&labels.title);
                rewo_ui::pause_screen::build(labels, *has_links, tw, gw, gh)
            }
            ScreenView::Links(labels) => {
                let tw = self.text_width(&labels.title);
                rewo_ui::server_links_screen::build(labels, tw, gw, gh)
            }
            ScreenView::Disconnected(labels, details) => {
                let width_of = move |t: &str| match &advance {
                    Some(a) => rewo_gpu::text::width(t, a),
                    None => 0,
                };
                rewo_ui::disconnect_screen::build(labels, details, gw, gh, &width_of)
            }
            ScreenView::ConfirmLink(prompt) => {
                let width_of = move |t: &str| match &advance {
                    Some(a) => rewo_gpu::text::width(t, a),
                    None => 0,
                };
                rewo_ui::confirm_link_screen::build(prompt, gw, gh, &width_of)
            }
        };
        self.screen.screens.open(screen);
    }

    /// `Screen.clickUrlAction` for a link clicked on the chat screen.
    ///
    /// The two options are read from `options.txt` (`chatLinks`,
    /// `chatLinksPrompt`, both default on). Off, the click does nothing and the
    /// chat stays as it was; prompt off, the link opens at once; otherwise a
    /// [`rewo_ui::confirm_link_screen`] REPLACES the chat screen.
    pub(super) fn chat_click_url(&mut self, uri: &str) {
        use rewo_ui::confirm_link_screen::{click_url_action, UrlClick};
        match click_url_action(self.options.chat_links, self.options.chat_links_prompt) {
            UrlClick::Refused => log::info!("chat: link ignored (chatLinks is off)"),
            UrlClick::OpenDirect => {
                crate::uri_open::open_uri(uri);
            }
            UrlClick::Prompt => {
                // `Gui.setScreen(new ConfirmLinkScreen(..))` calls the chat
                // screen's `removed()` on the way out — draft, scroll reset —
                // and the callback gives the same instance back.
                self.link_return_chat = self.take_chat_screen();
                self.open_confirm_link(uri);
            }
        }
    }

    /// `new ConfirmLinkScreen(callback, uri.toString(), false)`.
    pub(super) fn open_confirm_link(&mut self, uri: &str) {
        let prompt = rewo_ui::confirm_link_screen::ConfirmLinkScreen::new(&self.lang, uri, false);
        log::info!("live: link prompt (chat link)");
        self.view = ScreenView::ConfirmLink(prompt);
        self.rebuild_view_screen();
        self.grab_for_screen(true);
    }

    /// The prompt's `BooleanConsumer`, and what follows it in
    /// `clickUrlAction`'s lambda: open on yes, then `setScreen(screen)`.
    ///
    /// The copy button runs `copyToClipboard()` (the system clipboard, through
    /// `KeyboardHandler.setClipboard`) and then `callback.accept(false)`.
    pub(super) fn confirm_link_decide(
        &mut self,
        decision: rewo_ui::confirm_link_screen::LinkDecision,
    ) {
        use rewo_ui::confirm_link_screen::LinkDecision;
        let ScreenView::ConfirmLink(prompt) = &self.view else {
            return;
        };
        let url = prompt.url.clone();
        match decision {
            LinkDecision::Open => {
                crate::uri_open::open_uri(&url);
            }
            // Through the edit boxes' buffer: `pump_screens` pushes a changed
            // buffer to the system clipboard next frame.
            LinkDecision::CopyThenCancel => self.clipboard = url,
            LinkDecision::Cancel => {}
        }
        self.view = ScreenView::None;
        self.screen.screens.close();
        match self.link_return_chat.take() {
            // `setScreen(screen)` on a screen that was shown before: `init()`
            // again, with `initial` = what was in the field. The cursor is
            // still free — chat never gave it back.
            Some(mut chat) => {
                let recent_len = self
                    .session
                    .as_ref()
                    .map(|s| s.chat.recent_chat().len())
                    .unwrap_or(0);
                chat.reinit(recent_len);
                self.chat_screen = Some(chat);
            }
            None => self.grab_for_screen(false),
        }
    }

    /// Close whichever of M85's screens is up and hand the cursor back.
    pub(super) fn close_view_screen(&mut self) {
        self.view = ScreenView::None;
        self.screen.screens.close();
        self.grab_for_screen(false);
    }

    /// Open the statistics screen and ask the server for the numbers (M84).
    ///
    /// `StatsScreen.init()`'s last line is the `REQUEST_STATS` client command,
    /// so the request is part of *opening*, not of rendering — a screen opened
    /// with no request sits on "Retrieving statistics…" forever.
    pub(super) fn open_stats(&mut self) {
        let (gw, gh) = self.gui_size();
        let labels = rewo_ui::stats_screen::StatsLabels::resolve(&self.lang);
        let counter = self
            .session
            .as_ref()
            .map(|s| s.stats.clone())
            .unwrap_or_default();
        let (view, screen) = crate::stats_view::StatsView::build(
            &counter,
            &self.stat_registries,
            &self.items,
            &self.etypes,
            &self.lang,
            labels,
            Default::default(),
            (None, 0),
            gw,
            gh,
        );
        log::info!(
            "live: statistics screen — {} stats held, loading={}",
            counter.len(),
            view.model.loading
        );
        self.stats = Some(view);
        self.screen.screens.open(screen);
        self.grab_for_screen(true);
        if let Some(session) = self.session.as_mut() {
            if let Err(e) = session.request_stats() {
                log::warn!("live: request_stats: {e}");
            }
        }
    }

    pub(super) fn open_book_screen(&mut self, view: rewo_ui::book_view_screen::BookViewScreen) {
        let (gw, gh) = self.gui_size();
        let done = self.lang.get_or_default("gui.done", "Done").to_string();
        // M184: a held book's reader REPLACES the lectern screen —
        // `Minecraft.setScreen` calls `removed()`, which detaches the
        // lectern's listener. The MENU stays open (no close is sent), but
        // the sync must not resurrect the reader over the held book while
        // it does; the superseded marker names the exact menu identity.
        if let Some(identity) = self.lectern_shown.take() {
            self.lectern_superseded = Some(identity);
            self.lectern_pages = None;
        }
        self.book = Some(view);
        self.screen
            .screens
            .open(rewo_ui::book_view_screen::build_screen(gw, gh, &done));
        self.grab_for_screen(true);
    }

    /// The per-screen close (M84's rule): drop the app-side view state WITH
    /// the framework screen, or the render arm keys on a book that no longer
    /// has a `Screen` behind it.
    ///
    /// M184: the LECTERN's close is `player.closeContainer()` — it SENDS
    /// `container_close` (`LecternScreen.onClose`), where the plain reader's
    /// `closeContainerOnServer` is deliberately empty (there is no menu to
    /// close). Without the send the frame-sync's watermark would re-open
    /// the menu the user just closed. The marker clears BEFORE the send so
    /// the send's own no-op guard can't loop.
    ///
    /// `container_close` also clears `menus` (`closeContainer()`'s
    /// `clientSideCloseContainer`): the lectern sync keys on the open menu, so
    /// leaving it would reopen the reader on the next frame. The
    /// `container_shown` watermark is reset with it.
    pub(super) fn close_book(&mut self) {
        let owed = super::screens::close_lectern_reader(
            &mut self.lectern_shown,
            &mut self.screen.container_shown,
            self.session.as_mut().map(|s| &mut s.menus),
        );
        self.lectern_pages = None;
        self.book = None;
        self.screen.screens.close();
        self.grab_for_screen(false);
        if let (Some(container), Some(session)) = (owed, self.session.as_mut()) {
            if let Err(e) = session.send_container_close(container) {
                log::warn!("live: lectern close: {e}");
            }
        }
    }

    /// M174 — the editor's ONE commit path. Every exit (Done, Esc, the
    /// validity tick, being REPLACED by another screen) reaches `removed()`,
    /// which sends `sign_update` unconditionally: there is no dirty check and
    /// no cancel. The live local echo has already been applied per edit, so
    /// the send is re-stated here as belt-and-braces before the view drops.
    pub(super) fn commit_sign_edit(&mut self) {
        if let Some(view) = self.sign_edit.as_ref() {
            if let Some(session) = self.session.as_mut() {
                match session.send_sign_update(view.pos, view.is_front, &view.state.lines) {
                    Ok(()) => {
                        #[cfg(feature = "gates")]
                        {
                            if let Some(c) = self.check.as_mut() {
                                c.sign_update_sent = true;
                            }
                        }
                    }
                    Err(e) => log::warn!("live: sign_update send failed: {e}"),
                }
                // Keep the echo's invariant: the committed face is what the
                // packet carried, even if a keystroke's echo was missed.
                let be_pos = rewo_world::block_entities::BlockEntityPos {
                    x: view.pos.0,
                    y: view.pos.1,
                    z: view.pos.2,
                };
                if let Some(be) = session.world.block_entities.get_mut(be_pos) {
                    be.set_sign_messages(view.is_front, &view.state.lines);
                }
            }
        }
    }

    pub(super) fn close_sign_edit(&mut self) {
        self.commit_sign_edit();
        self.sign_edit = None;
        self.screen.screens.close();
        self.grab_for_screen(false);
    }

    /// M174 — the per-keystroke local echo. Vanilla's field writes through to
    /// the block entity as you type (`TextFieldHelper`'s messager is
    /// `sign.setMessage(line, literal)`), so the world renderer shows the text
    /// updating live behind the editor. `setMessage` replaces one face's
    /// messages and preserves that face's colour + glow.
    pub(super) fn echo_sign_edit(&mut self) {
        let Some(view) = self.sign_edit.as_ref() else {
            return;
        };
        let lines = view.state.lines.clone();
        let (pos, front) = (view.pos, view.is_front);
        if let Some(session) = self.session.as_mut() {
            let be_pos = rewo_world::block_entities::BlockEntityPos {
                x: pos.0,
                y: pos.1,
                z: pos.2,
            };
            if let Some(be) = session.world.block_entities.get_mut(be_pos) {
                be.set_sign_messages(front, &lines);
            }
        }
    }

    /// M173 — open an options page. `Screens` is one slot, so this both
    /// opens and navigates; the durable values live on `self.options` and the
    /// widgets are rebuilt from them (the M82 resize rule).
    pub(super) fn open_options_screen(&mut self, page: rewo_ui::options_screen::OptionsPage) {
        self.options_view = Some(page);
        self.options_drag = None;
        self.rebuild_options_screen();
        self.grab_for_screen(true);
    }

    pub(super) fn rebuild_options_screen(&mut self) {
        use rewo_ui::options_screen as os;
        let Some(page) = self.options_view else {
            return;
        };
        let (gw, gh) = self.gui_size();
        if gw <= 0 || gh <= 0 {
            return;
        }
        let rows = match page {
            os::OptionsPage::Root => root_rows(&self.lang),
            os::OptionsPage::Sound => sound_rows(&self.options, &self.lang),
            os::OptionsPage::Video => video_rows(&self.options, &self.lang),
            os::OptionsPage::Accessibility => accessibility_rows(&self.options, &self.lang),
        };
        // The list content starts under the 33-px header; the title sits
        // centred inside the header, as `HeaderAndFooterLayout` places it.
        let mut screen = os::build(page, &rows, gw, gh, os::FOOTER_HEIGHT + 3);
        let title = match page {
            os::OptionsPage::Root => self.lang.get_or_default("options.title", "Options"),
            os::OptionsPage::Sound => self
                .lang
                .get_or_default("options.sounds.title", "Music & Sound Options"),
            os::OptionsPage::Video => self
                .lang
                .get_or_default("options.videoTitle", "Video Settings"),
            os::OptionsPage::Accessibility => self
                .lang
                .get_or_default("options.accessibility.title", "Accessibility Settings"),
        }
        .to_string();
        let tw = self.text_width(&title);
        screen.widgets.push(rewo_ui::screen::Widget::label(
            os::DONE + 1,
            (gw - tw) / 2,
            (os::FOOTER_HEIGHT - 9) / 2,
            tw,
            title,
        ));
        self.screen.screens.open(screen);
    }

    /// M173 — leaving a page. A sub-page's Done/Esc returns to the ROOT
    /// (vanilla's `lastScreen`), the root's to the pause screen — and leaving
    /// a sub-page SAVES `options.txt`, which is `OptionsSubScreen.removed()`
    /// (a slider never saves per drag; the cycle buttons saved per click
    /// already).
    pub(super) fn close_options(&mut self) {
        use rewo_ui::options_screen::OptionsPage;
        let page = self.options_view.take();
        self.options_drag = None;
        save_options(self.options);
        match page {
            Some(OptionsPage::Sound)
            | Some(OptionsPage::Video)
            | Some(OptionsPage::Accessibility) => {
                self.open_options_screen(OptionsPage::Root);
            }
            _ => {
                self.screen.screens.close();
                self.grab_for_screen(false);
                self.open_pause_screen();
            }
        }
    }

    /// M173 — a slider's value changed (press, drag, or arrow key): apply it
    /// to the option, the live engine, and the widget's own label.
    pub(super) fn options_slider_changed(&mut self, id: rewo_ui::screen::WidgetId, value: f32) {
        use rewo_ui::options_screen as os;
        if self.options_view != Some(os::OptionsPage::Sound) {
            return;
        }
        let Some(os::SoundSlot::Volume(ordinal)) = os::sound_slot(id) else {
            return;
        };
        let source = rewo_net::sounds::SoundSource::ALL[ordinal as usize];
        self.options.set_sound_volume(source, value);
        // The slider's `onValueUpdate` — store + `refreshCategoryVolume`,
        // never `gainBySource` (that channel belongs to the music fade).
        self.sounds.set_category_volume(source, value);
        // `updateMessage()` — the label tracks the LIVE drag value.
        let caption_key = format!("soundCategory.{}", source.name());
        let caption = self
            .lang
            .get_or_default(&caption_key, source.name())
            .to_string();
        let off = self.lang.get_or_default("options.off", "OFF").to_string();
        if let Some(sc) = self.screen.screens.current_mut() {
            if let Some(w) = sc.widget_mut(id) {
                w.message = os::percent_label(&caption, value, &off);
            }
        }
    }

    pub(super) fn close_stats(&mut self) {
        self.stats = None;
        self.screen.screens.close();
        self.grab_for_screen(false);
    }

    /// Open the advancements screen (M178). `AdvancementsScreen.init()`
    /// auto-selects the first tab and tells the server via
    /// `ServerboundSeenAdvancementsPacket.openedTab` — the send is part of
    /// opening, exactly as the stats screen's REQUEST_STATS is.
    pub(super) fn open_advancements(&mut self) {
        let Some(advance) = self
            .baked
            .as_ref()
            .and_then(|b| b.font.as_ref())
            .map(|f| f.advance)
        else {
            log::warn!("live: advancements screen needs the baked font");
            return;
        };
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let view = crate::advancements_view::AdvancementsView::build(
            &session.advancements,
            &self.lang,
            &advance,
        );
        if view.screen.tabs.is_empty() {
            // Vanilla opens onto an empty window with its two sad labels —
            // that IS the screen; keep it rather than refusing to open.
            log::info!("live: advancements screen — no displayed advancements");
        }
        let selected = view.selected_root_id().map(str::to_string);
        let (gw, gh) = self.gui_size();
        self.advancements = Some(view);
        // Seed the drag machine's cursor sample — a stale one would make the
        // first post-latch delta span the whole park-to-window trip.
        self.adv_drag
            .cursor_seed((self.screen.mouse.0, self.screen.mouse.1));
        let done = self.lang.or_key("gui.done").to_string();
        let screen = rewo_ui::advancements_screen::build_screen(gw, gh, &done);
        self.screen.screens.open(screen);
        self.grab_for_screen(true);
        if let Some(tab) = selected {
            if let Some(session) = self.session.as_mut() {
                if let Err(e) = session.send_seen_advancements_opened_tab(&tab) {
                    log::warn!("live: opened_tab: {e}");
                }
            }
        }
    }

    /// A left click on the lectern reader's arrows (M184): `LecternScreen`'s
    /// `pageBack` / `pageForward` are `sendButtonClick(1 / 2)` — sends, not
    /// local turns; the server's container-data echo moves the page. One
    /// method for the mouse handler and render-check r75.
    pub(super) fn lectern_arrow_click(&mut self, mx: f64, my: f64, gw: i32) {
        let send = self
            .book
            .as_ref()
            .and_then(|b| b.arrow_at(mx as i32, my as i32, gw));
        if let Some(button) = send.map(|a| match a {
            rewo_ui::book_view_screen::BookArrow::Back => {
                rewo_ui::book_view_screen::LECTERN_PAGE_BACK
            }
            rewo_ui::book_view_screen::BookArrow::Forward => {
                rewo_ui::book_view_screen::LECTERN_PAGE_FORWARD
            }
        }) {
            if let Some(session) = self.session.as_mut() {
                if let Err(e) = session.container_button_click(button) {
                    log::warn!("live: lectern page turn: {e}");
                }
            }
        }
    }

    /// The block the attack key works on this tick (M217): `GameRenderer
    /// .pick`'s BLOCK result — none when an entity is nearer (the M73 pick
    /// reconciles the two) — with its destroy progress for the held item.
    fn mining_target(&mut self) -> Option<rewo_world::mining::Target> {
        let tables = self
            .mining_tables
            .get_or_insert_with(|| {
                std::sync::Arc::new(rewo_data::mining::MiningTables::build(
                    &self.blocks,
                    &self.stat_registries.block,
                    &self.items,
                ))
            })
            .clone();
        let session = self.session.as_ref()?;
        if super::entities::frame_crosshair_hit(session, &self.etypes, 1.0).is_some() {
            return None;
        }
        let hit = session.target_block(
            eye_f64(session),
            look_dir(session.player.yaw, session.player.pitch),
            REACH,
        )?;
        let [x, y, z] = hit.block;
        let state = session.world.block_state_at(x, y, z);
        let name = self.blocks.block_name(state).unwrap_or("minecraft:air");
        let air =
            name.ends_with(":air") || name.ends_with("cave_air") || name.ends_with("void_air");
        let Some(block) = tables.block(state) else {
            // Not in the hardness table: unbreakable rather than instant.
            return Some(rewo_world::mining::Target {
                pos: hit.block,
                face: face_index(hit.face),
                air,
                progress: 0.0,
            });
        };
        let in_tag = |tag: &str, id: i32| {
            session
                .tags
                .contains("minecraft:block", tag, id)
                .unwrap_or(false)
        };
        let held = session.inventory.held();
        let tool = held.and_then(|s| tables.tool(s.item_id));
        let (tool_speed, tool_correct) = match tool {
            Some(t) => (
                t.speed(name, block.block_id, &in_tag),
                t.correct_for_drops(name, block.block_id, &in_tag),
            ),
            None => (1.0, false),
        };
        let (dig, fatigue) = session.mining_effects();
        let attr = |n: &str, d: f64| session.local_attribute(n).unwrap_or(d) as f32;
        let inputs = rewo_world::mining::MiningInputs {
            destroy_time: block.destroy_time,
            requires_correct_tool: block.requires_correct_tool,
            tool_speed,
            tool_correct,
            mining_efficiency: attr("minecraft:mining_efficiency", 0.0),
            dig_speed_amplifier: dig,
            fatigue_amplifier: fatigue,
            block_break_speed: attr("minecraft:block_break_speed", 1.0),
            submerged_mining_speed: attr("minecraft:submerged_mining_speed", 0.2),
            eye_in_water: session.local_player_view().is_some_and(|v| v.underwater),
            on_ground: session.player.on_ground,
        };
        Some(rewo_world::mining::Target {
            pos: hit.block,
            face: face_index(hit.face),
            air,
            progress: rewo_world::mining::destroy_progress(&inputs),
        })
    }

    /// The held main-hand stack's identity for `sameDestroyTarget`.
    fn mining_item(&self) -> Option<(i32, u64)> {
        let s = self.session.as_ref()?.inventory.held()?;
        Some((s.item_id, s.components))
    }

    /// `startAttack`'s BLOCK arm: `startDestroyBlock`, then the swing.
    pub(super) fn mining_press(&mut self) {
        let Some(t) = self.mining_target() else {
            return;
        };
        let creative = self
            .session
            .as_ref()
            .is_some_and(|s| s.abilities.instabuild);
        let item = self.mining_item();
        let sends = self.mining.start(t, creative, item);
        self.apply_mining(sends);
        if let Some(s) = self.session.as_mut() {
            let _ = s.swing();
        }
    }

    /// `continueAttack(true)`, once per client tick while the attack key is
    /// held: `continueDestroyBlock` on a block (and the swing when it says
    /// so), `stopDestroyBlock` when the crosshair is on anything else.
    pub(super) fn mining_tick(&mut self) {
        let Some(now) = self.session.as_ref().map(|s| s.ticks) else {
            return;
        };
        if now == self.mining_tick_at {
            return;
        }
        self.mining_tick_at = now;
        if !self.attack_held || self.screen.inventory_open() {
            return;
        }
        match self.mining_target() {
            Some(t) => {
                let creative = self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.abilities.instabuild);
                let item = self.mining_item();
                let (sends, swing) = self.mining.tick(t, creative, item);
                self.apply_mining(sends);
                if swing {
                    if let Some(s) = self.session.as_mut() {
                        let _ = s.swing();
                    }
                }
            }
            None => {
                if let Some(a) = self.mining.stop() {
                    self.apply_mining(vec![a]);
                }
            }
        }
    }

    /// Send what the machine asked for. A broken block is NOT predicted
    /// locally (vanilla's `destroyBlock` in `startPrediction`): the server's
    /// block update follows within the machine's five-tick delay on any sane
    /// connection — a recorded deviation.
    fn apply_mining(&mut self, sends: Vec<rewo_world::mining::MiningSend>) {
        use rewo_world::mining::MiningSend;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        for s in sends {
            let r = match s {
                MiningSend::Start {
                    pos: [x, y, z],
                    face,
                    ..
                } => session.block_action(0, x, y, z, face),
                MiningSend::Stop {
                    pos: [x, y, z],
                    face,
                } => session.block_action(2, x, y, z, face),
                MiningSend::Abort { pos: [x, y, z] } => session.block_action(1, x, y, z, 0),
            };
            if let Err(e) = r {
                log::warn!("live: block action: {e}");
            }
        }
    }

    /// `keyInventory` (M35, M220): close an open container screen through
    /// `onClose()`; while riding a vehicle with its own inventory
    /// (`isServerControlledInventory`) ask the server with `OPEN_INVENTORY`;
    /// otherwise open the player's inventory.
    pub(super) fn inventory_key(&mut self) {
        // The inventory key closes a container screen through `onClose()`
        // (`AbstractContainerScreen.java:489-491`) — a user close.
        if self.screen.inventory_open() {
            self.close_container_screen();
        } else if self
            .session
            .as_ref()
            .is_some_and(|s| s.server_controlled_inventory())
        {
            // M220 — riding a horse (or a nautilus, or a chest boat): the
            // key asks the server, which answers with `mount_screen_open`
            // (or `open_screen` for a chest boat).
            if let Some(s) = self.session.as_mut() {
                if let Err(e) = s.open_vehicle_inventory() {
                    log::warn!("live: open vehicle inventory: {e}");
                }
            }
        } else {
            // A menu still open here has no screen of its own (a lectern a
            // held book's reader replaced): vanilla's key opens the player's
            // inventory over it (`Minecraft.java:1887-1893`).
            if let Some(session) = self.session.as_mut() {
                session.menus.show_player_inventory();
            }
            self.set_screen_open(true);
        }
    }

    fn spectating(&self) -> bool {
        self.session
            .as_ref()
            .and_then(|s| s.own_game_mode())
            .is_some_and(|m| m == rewo_net::play::GameMode::Spectator)
    }

    /// `keyDrop` (M218): `LocalPlayer.drop(hasControlDown())`, and the
    /// main-hand swing when something was held (vanilla swings when its local
    /// prediction removed a non-empty stack).
    pub(super) fn drop_key(&mut self) {
        if self.spectating() {
            return;
        }
        let all = self.ctrl;
        if let Some(s) = self.session.as_mut() {
            match s.drop_item(all) {
                Ok(true) => {
                    let _ = s.swing();
                }
                Ok(false) => {}
                Err(e) => log::warn!("live: drop: {e}"),
            }
        }
    }

    /// `keySwapOffhand` (M218).
    pub(super) fn swap_hands_key(&mut self) {
        if self.spectating() {
            return;
        }
        if let Some(s) = self.session.as_mut() {
            if let Err(e) = s.swap_hands() {
                log::warn!("live: swap hands: {e}");
            }
        }
    }

    /// `pickBlockOrEntity` (M218): the entity under the crosshair, else the
    /// block, with `includeData` from Ctrl. The server answers (in creative it
    /// puts the item in the hotbar; in survival it selects a matching stack).
    /// M229 — the use key going down: `keyUse.consumeClick()` ->
    /// `startUseItem`, unless an item is already in use (`handleKeybinds`
    /// drains the clicks then). Held, `use_tick` repeats it.
    pub(super) fn use_key_press(&mut self) {
        self.use_down = true;
        let using = self
            .session
            .as_ref()
            .is_some_and(|s| s.local_use_state().using);
        if !using {
            self.start_use_item();
        }
    }

    /// M229 — `Minecraft.startUseItem`: for each hand, the block under the
    /// crosshair (`useItemOn`, its packet always sent), then the item itself
    /// (`useItem`, likewise), each step's LOCAL result
    /// ([`rewo_world::click_use`]) deciding whether the next is tried. An
    /// entity under the crosshair was already taken by `entity_click`.
    ///
    /// Recorded deviations: the entity arm interacts with the main hand only
    /// (`entity_click`); the hit is the collision-shape pick, not vanilla's
    /// outline shape (M230); `itemInHandRenderer.itemUsed` (the re-equip
    /// bob) is not run.
    pub(super) fn start_use_item(&mut self) {
        if self.mining.destroying() {
            return;
        }
        self.right_click_delay = 4;
        let Some(plan) = self.use_plan() else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        for step in plan {
            let r = match step {
                UseStep::On {
                    hand,
                    pos,
                    face,
                    offset,
                    swing,
                } => session.use_item_on_hit(hand, pos, face, offset, swing),
                UseStep::Use { hand, start, swing } => session.use_held(hand, start, swing),
            };
            if let Err(e) = r {
                log::warn!("live: use: {e}");
            }
        }
    }

    /// The packets `startUseItem` sends, decided against the local state
    /// before any is sent (sending changes none of what the decision reads).
    fn use_plan(&self) -> Option<Vec<UseStep>> {
        use rewo_world::click_use::{self as cu, Hand, Outcome};
        use rewo_world::entities::InteractionHand;
        let session = self.session.as_ref()?;
        // `isHandsBusy`: a boat being paddled (`LocalPlayer.rideTick`).
        if session.steered_boat.is_some()
            && (self.keys.w || self.keys.a || self.keys.s || self.keys.d)
        {
            return Some(Vec::new());
        }
        let me = session.player_id?;
        let eye = eye_f64(session);
        let dir = look_dir(session.player.yaw, session.player.pitch);
        let hit = session.target_block(eye, dir, REACH);
        let name = |slot: Option<rewo_world::inventory::ItemSlot>| {
            slot.and_then(|s| self.items.name(s.item_id))
        };
        let main = name(session.inventory.held());
        let off = name(session.inventory.offhand());
        let item_tag = |item: &str, tag: &str| {
            self.items.id(item).is_some_and(|id| {
                session
                    .tags
                    .contains("minecraft:item", tag, id)
                    .unwrap_or(false)
            })
        };
        let campfire = |item: &str| {
            let set = session
                .recipes
                .as_ref()
                .and_then(|r| r.property_set("minecraft:campfire_input"));
            self.items
                .id(item)
                .is_some_and(|id| set.is_some_and(|s| s.contains(&id)))
        };
        let exists = |b: &str| self.blocks.default_state(b).is_some();
        let mode = session.game_state.game_mode();
        let clicker = cu::Clicker {
            main,
            off,
            sneaking: self.keys.sneak,
            creative: session.abilities.instabuild,
            may_build: !matches!(
                mode,
                Some(rewo_net::play::GameMode::Adventure | rewo_net::play::GameMode::Spectator)
            ),
            food: session.food,
            item_tag: &item_tag,
            campfire_input: &campfire,
            block_exists: &exists,
        };
        let arrows = || {
            session.abilities.instabuild
                || (0..session.inventory.slot_count()).any(|i| {
                    name(session.inventory.menu_slot(i)).is_some_and(|n| {
                        matches!(
                            n,
                            "minecraft:arrow"
                                | "minecraft:spectral_arrow"
                                | "minecraft:tipped_arrow"
                        )
                    })
                })
        };
        let mut plan = Vec::new();
        for (hand, eh) in [
            (Hand::Main, InteractionHand::MainHand),
            (Hand::Off, InteractionHand::OffHand),
        ] {
            let held = match hand {
                Hand::Main => main,
                Hand::Off => off,
            };
            if let Some(h) = hit {
                let state = session
                    .world
                    .block_state_at(h.block[0], h.block[1], h.block[2]);
                let block = self.blocks.block_name(state).unwrap_or("minecraft:air");
                let prop = |k: &str| self.blocks.property(state, k).map(str::to_string);
                let face = face_index(h.face);
                let clicked = cu::Clicked {
                    block,
                    prop: &prop,
                    face,
                };
                let local = cu::use_item_on(&clicked, &clicker, hand);
                // `BlockHitResult.getLocation() - pos`, as the packet's floats.
                let at = [
                    eye[0] + dir[0] * h.distance,
                    eye[1] + dir[1] * h.distance,
                    eye[2] + dir[2] * h.distance,
                ];
                let offset = [
                    (at[0] - h.block[0] as f64).clamp(0.0, 1.0) as f32,
                    (at[1] - h.block[1] as f64).clamp(0.0, 1.0) as f32,
                    (at[2] - h.block[2] as f64).clamp(0.0, 1.0) as f32,
                ];
                plan.push(UseStep::On {
                    hand: eh,
                    pos: h.block,
                    face,
                    offset,
                    swing: local == Outcome::Consumed,
                });
                if local != Outcome::Pass {
                    return Some(plan);
                }
            }
            let Some(item) = held else {
                continue;
            };
            let hand_item = session.world.entities.hand_item(me, eh);
            let duration = hand_item.use_profile().map_or(0, |p| p.duration);
            let armor = matches!(
                rewo_data::item_props_table::equip_slot(item),
                Some(
                    rewo_data::item_props_table::EquipSlot::Head
                        | rewo_data::item_props_table::EquipSlot::Chest
                        | rewo_data::item_props_table::EquipSlot::Legs
                        | rewo_data::item_props_table::EquipSlot::Feet
                )
            );
            let mut local = cu::use_item(item, &clicker, duration, armor);
            // `BowItem.use`: FAIL without an arrow (outside creative);
            // `CrossbowItem.use`: a charged crossbow fires (SUCCESS) and
            // starts no use.
            let charged = hand_item.is_charged();
            if item == "minecraft:bow" && !arrows() {
                local = Outcome::Fail;
            }
            let start = local == Outcome::Consumed && duration > 0 && !charged;
            // An instant use (a throw, a bucket, a cast) swings; a held one
            // (eating, a draw, a shield) is a CONSUME, which does not.
            let swing = local == Outcome::Consumed && (duration == 0 || charged);
            plan.push(UseStep::Use {
                hand: eh,
                start,
                swing,
            });
            if local == Outcome::Consumed {
                return Some(plan);
            }
        }
        Some(plan)
    }

    /// M229 — `handleKeybinds`' tick half: release a use the moment the use
    /// key is up (`if isUsingItem() && !keyUse.isDown() releaseUsingItem`),
    /// and while it is held repeat `startUseItem` once `rightClickDelay`
    /// (counted down by `Minecraft.tick`) reaches 0.
    pub(super) fn use_tick(&mut self) {
        let Some(now) = self.session.as_ref().map(|s| s.ticks) else {
            return;
        };
        if now == self.use_tick_at {
            return;
        }
        self.use_tick_at = now;
        self.right_click_delay = self.right_click_delay.saturating_sub(1);
        // A screen up releases the key (`KeyMapping.releaseAll` on open).
        if self.screen.inventory_open() {
            self.use_down = false;
        }
        let using = self
            .session
            .as_ref()
            .is_some_and(|s| s.local_use_state().using);
        if using && !self.use_down {
            if let Some(s) = self.session.as_mut() {
                let _ = s.stop_use();
            }
        } else if self.use_down && self.right_click_delay == 0 && !using {
            self.start_use_item();
        }
    }

    pub(super) fn pick_key(&mut self) {
        let include = self.ctrl;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Some(hit) = super::entities::frame_crosshair_hit(session, &self.etypes, 1.0) {
            if let Err(e) = session.pick_item_from_entity(hit.id, include) {
                log::warn!("live: pick entity: {e}");
            }
            return;
        }
        let hit = session.target_block(
            eye_f64(session),
            look_dir(session.player.yaw, session.player.pitch),
            REACH,
        );
        if let Some(h) = hit {
            let [x, y, z] = h.block;
            if let Err(e) = session.pick_item_from_block(x, y, z, include) {
                log::warn!("live: pick block: {e}");
            }
        }
    }

    /// A world click on an ENTITY under the crosshair (M216). Returns whether
    /// it was one — the block/item paths then do not run.
    ///
    /// - Left: `Minecraft.startAttack`'s ENTITY arm → `MultiPlayerGameMode
    ///   .attack`, `ServerboundAttackPacket` and the main-hand swing.
    /// - Right: `startUseItem`'s ENTITY arm for the main hand →
    ///   `MultiPlayerGameMode.interact`, `ServerboundInteractPacket` with the
    ///   hit location relative to the entity and the shift key.
    ///
    /// Recorded deviations: vanilla continues to the off hand (and then to
    /// `useItem`) when the LOCAL `interactOn` does not consume; that local
    /// result is per-entity logic Rewo does not predict, so the main hand's
    /// interact always ends the click. A held `PIERCING_WEAPON` (a spear)
    /// takes `piercingAttack` in vanilla; here it attacks like any item, and
    /// `AttackRange` is not consulted. The local `player.attack` effects
    /// (crit and sweep particles) are not drawn.
    pub(super) fn entity_click(&mut self, button: MouseButton) -> bool {
        if !matches!(button, MouseButton::Left | MouseButton::Right) {
            return false;
        }
        let sneaking = self.keys.sneak;
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let Some(hit) = super::entities::frame_crosshair_hit(session, &self.etypes, 1.0) else {
            return false;
        };
        match button {
            MouseButton::Left => {
                if let Err(e) = session.attack_entity(hit.id) {
                    log::warn!("live: attack: {e}");
                }
            }
            _ => {
                let Some(e) = session.world.entities.get(hit.id) else {
                    return false;
                };
                let rel = [
                    hit.location[0] - e.x,
                    hit.location[1] - e.y,
                    hit.location[2] - e.z,
                ];
                if let Err(e) = session.interact_entity(hit.id, 0, rel, sneaking) {
                    log::warn!("live: interact: {e}");
                }
            }
        }
        true
    }

    /// Close the advancements screen. `removed()` sends CLOSED_SCREEN
    /// unconditionally — no dirty check, no cancel (M179 will reuse this for
    /// every close path).
    pub(super) fn close_advancements(&mut self) {
        if let Some(session) = self.session.as_mut() {
            if let Err(e) = session.send_seen_advancements_closed_screen() {
                log::warn!("live: closed_screen: {e}");
            }
        }
        // A close mid-drag must not strand the machine (the L key and Done
        // both land here while a button may be held).
        self.adv_drag.release();
        self.advancements = None;
        self.screen.screens.close();
        self.grab_for_screen(false);
    }

    /// `repositionElements` — rebuild from the current counter, keeping the
    /// tab, the sort and the scroll.
    pub(super) fn rebuild_stats_screen(&mut self) {
        let (gw, gh) = self.gui_size();
        let Some(view) = self.stats.as_ref() else {
            return;
        };
        let (tab, sort) = (
            view.model.tab,
            (view.model.sort_column, view.model.sort_order),
        );
        let scrolls: Vec<f64> = view.model.lists.iter().map(|l| l.scroll()).collect();
        let labels = view.labels.clone();
        let counter = self
            .session
            .as_ref()
            .map(|s| s.stats.clone())
            .unwrap_or_default();
        let (mut view, screen) = crate::stats_view::StatsView::build(
            &counter,
            &self.stat_registries,
            &self.items,
            &self.etypes,
            &self.lang,
            labels,
            tab,
            sort,
            gw,
            gh,
        );
        // The scroll survives a rebuild, re-clamped against the new content —
        // `updateSizeAndPosition` ends in `refreshScrollAmount()`, which is
        // `setScrollAmount(scrollAmount)` and therefore exactly this clamp.
        for (l, v) in view.model.lists.iter_mut().zip(scrolls) {
            l.set_scroll(v);
        }
        self.stats = Some(view);
        self.screen.screens.open(screen);
    }
}
