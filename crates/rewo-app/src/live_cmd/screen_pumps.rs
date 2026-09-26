use super::*;

impl LiveApp {
    /// Open, reposition and close the death screen (M82).
    ///
    /// Three separate rules, and only the first is the packet's:
    ///
    /// 1. `handlePlayerCombatKill` opens it. The session has already taken
    ///    vanilla's other branch (`!shouldShowDeathScreen()` → respawn
    ///    immediately), so a drained death always means "show the screen".
    /// 2. `Screen.resize` rebuilds it — which is `init()`, so the one-second
    ///    button guard restarts. That is vanilla's behaviour and not a bug.
    /// 3. **`handleRespawn` closes it**, not the button press. Watched through
    ///    the session's respawn watermark.
    pub(super) fn pump_death_screen(&mut self) {
        use rewo_world::screen::ScreenKind;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let (respawns, kill) = (session.respawn_epoch(), session.take_death());
        let (hardcore, score) = (session.hardcore, session.score);
        if let Some(kill) = kill {
            let (gw, gh) = self.gui_size();
            let lang = self.lang.clone();
            let (view, screen) =
                DeathView::open(&kill, hardcore, score, &lang, respawns, gw, gh);
            log::info!(
                "live: death screen — \"{}\", hardcore={hardcore}, score={score}",
                view.model.cause_of_death.as_deref().unwrap_or("")
            );
            self.death = Some(view);
            self.screen.screens.open(screen);
            self.grab_for_screen(true);
            return;
        }
        let Some(view) = self.death.as_ref() else {
            return;
        };
        if respawns != view.respawn_epoch {
            log::info!("live: respawned — closing the death screen");
            self.death = None;
            self.screen.screens.close();
            self.grab_for_screen(false);
            return;
        }
        // A resize while dead. Compared against the screen's recorded size so
        // a frame that changes nothing rebuilds nothing — a rebuild resets the
        // guard, and doing it every frame would leave the buttons dead forever.
        let (gw, gh) = self.gui_size();
        let stale = self
            .screen
            .screens
            .current()
            .is_some_and(|s| s.kind == ScreenKind::Death && (s.width != gw || s.height != gh));
        if stale {
            if let (Some(view), Some(s)) = (self.death.as_ref(), self.screen.screens.current_mut()) {
                view.reposition(s, gw, gh);
            }
        }
    }

    /// One frame with **no session** — the disconnect screen (M85).
    ///
    /// Deliberately not a cut-down copy of [`Self::frame`]: it renders the
    /// screen pass and the text pass and nothing else, because with no world
    /// there is nothing else to render. The view-projection is the identity,
    /// which is what the offscreen gates have passed since M82 and which the
    /// (empty) world pass does not read anyway.
    pub(super) fn render_screen_only(&mut self, event_loop: &ActiveEventLoop) {
        if self.exit_requested {
            event_loop.exit();
            return;
        }
        let mouse_gui = self.mouse_gui();
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let extent = state.renderer.swapchain.extent;
        let px = gui_px(extent.width, extent.height);
        let mut chrome = rewo_gpu::screen::ScreenDraw::default();
        let mut text = Vec::new();
        if let Some(screen) = self.screen.screens.current() {
            chrome = screen_chrome(screen, Some(mouse_gui));
            if let Some(advance) = state.world_renderer.font_advance() {
                text = screen_text_lines(screen, advance, px);
            }
        }
        state.world_renderer.set_screen(chrome);
        state.world_renderer.set_text(text);
        let draw = OverlayDraw {
            samples_ms: &self.ring.data,
            head: self.ring.head(),
            scale_ms: 20.0,
            // Off-screen: the strip chart measures a frame loop that is no
            // longer running.
            origin: [-4000.0, -4000.0],
            size: [8.0, 8.0],
        };
        let LiveState {
            window,
            gpu,
            renderer,
            world_renderer,
        } = state;
        let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
        // GPU half only. A minimized window has a zero extent, so acquire
        // returns OUT_OF_DATE and a zero-size recreate is refused — rendering
        // anyway spun this loop at 100% CPU. Skip the GPU work and pace the loop
        // instead; everything above (ticks, network) keeps running.
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            std::thread::sleep(std::time::Duration::from_millis(16));
        } else {
            gpu.collect_garbage();
            match renderer.render(gpu, Some((world_renderer, vp)), &draw, CLEAR_SKY) {
                Ok(RenderOutcome::Rendered) | Ok(RenderOutcome::Skipped) => {}
                Ok(RenderOutcome::NeedsRecreate) => {
                    let _ = renderer.recreate(gpu, size.width, size.height);
                    let _ = renderer.ensure_depth(gpu);
                }
                Err(e) => {
                    log::error!("live: render failed: {e}");
                    event_loop.exit();
                }
            }
        }
        window.request_redraw();
        // **The soak deadline lives here too, and its absence is the one thing
        // the live run caught that the gate could not.**
        //
        // `--run-seconds` was checked at the bottom of `frame`, past the point
        // where the session is borrowed — so a client that lost its connection
        // ran forever. That is the shape this milestone is about: everything
        // the frame loop does after acquiring a session was *implicitly*
        // session-gated, and a screen with no session behind it is what makes
        // the implicit gate observable.
        if let Some(limit) = self.run_seconds {
            if self.started.elapsed().as_secs_f32() >= limit {
                event_loop.exit();
            }
        }
    }

    /// M172 — an `open_book` arrived: resolve the held book's pages and open
    /// the reader. `ClientPacketListener.handleOpenBook` reads
    /// `player.getItemInHand(hand)`; `BookAccess.fromItem` tries the WRITTEN
    /// component FIRST (so a zero-page written book opens an empty reader),
    /// then the WRITABLE one (plain strings shown as literals — there is no
    /// WritableBookViewScreen in 26.2), and with neither opens NOTHING.
    ///
    /// Pages are resolved + wrapped ONCE here, never per frame — vanilla
    /// caches per page too, and `parse_component` walks up to 65,536 nodes.
    /// The wrap is `StringSplitter.splitLines` (`split_lines_wrapped`), NOT
    /// the chat's `wrap_components`, which would prepend an indent space to
    /// every continuation line vanilla's book pages do not have.
    pub(super) fn pump_book_screen(&mut self) {
        let Some(hand) = self
            .session
            .as_mut()
            .and_then(|s| s.open_book_request.take())
        else {
            // `Screens` is ONE slot (vanilla's `Minecraft.screen`): anything
            // that opens over the reader — the death screen, the pause menu,
            // an `open_screen` — REPLACES the book's `Screen` but not this
            // app-side state, and a stale `self.book` would hijack the render
            // arm the moment the other screen closed. Drop it when the slot
            // no longer holds the reader.
            if self.book.is_some()
                && !self
                    .screen
                    .screens
                    .current()
                    .is_some_and(|s| s.kind == rewo_world::screen::ScreenKind::BookView)
            {
                self.book = None;
            }
            // Resize: the framework screen's widgets re-centre. The model
            // itself is size-independent, so only the `Screen` rebuilds — and
            // only on a change, or `ticks`/focus reset every frame (M82).
            if self.book.is_some() {
                let (gw, gh) = self.gui_size();
                let stale = self.screen.screens.current().is_some_and(|s| {
                    s.kind == rewo_world::screen::ScreenKind::BookView
                        && (s.width != gw || s.height != gh)
                });
                if stale {
                    let done = self.lang.get_or_default("gui.done", "Done").to_string();
                    self.screen
                        .screens
                        .open(rewo_world::book_view_screen::build_screen(gw, gh, &done));
                }
            }
            return;
        };
        let advance = self.advance();
        let lang = self.lang.clone();
        let hotbar = self.hotbar_slot as usize;
        let pages: Option<Vec<Vec<rewo_world::chat_style::ChatLine>>> =
            self.session.as_ref().and_then(|session| {
                // Hand 0 reads the slot the server last heard
                // (`set_carried_item`), which is the app's local selection —
                // `Inventory.selected` lags it, because its only writer is the
                // server's own `set_held_slot` (M172's survey).
                let stack = if hand == 1 {
                    session.inventory.offhand()
                } else {
                    session.inventory.hotbar(hotbar)
                }?;
                let text = session.inventory.text_of(stack)?;
                resolve_book_pages(text, advance.as_ref(), Some(&lang))
            });
        if let Some(pages) = pages {
            self.open_book_screen(rewo_world::book_view_screen::BookViewScreen::new(pages));
        }
    }

    /// M174 — poll an `open_sign_editor` request and open the editor.
    ///
    /// Vanilla's `handleOpenSignEditor` warns and ignores unless a SIGN block
    /// entity sits at the packet's pos (26.2 does NOT construct a fresh one —
    /// that was the ≤1.19 shape), and the screen class is chosen by the
    /// block-ENTITY kind: hanging vs not. Wall-vs-ground for a standing sign
    /// comes from the block state, which is what decides whether the board
    /// blit samples the whole 24×26 sheet or only its top 12 rows.
    pub(super) fn pump_sign_editor(&mut self) {
        let Some((pos, front)) = self
            .session
            .as_mut()
            .and_then(|s| s.open_sign_editor_request.take())
        else {
            // Screens is ONE slot; drop the app-side view with it (the book's
            // stale-state rule — a stale `sign_edit` would hijack the render
            // arm when some other screen closed).
            if self.sign_edit.is_some()
                && !self
                    .screen
                    .screens
                    .current()
                    .is_some_and(|s| s.kind == rewo_world::screen::ScreenKind::SignEdit)
            {
                // Vanilla's `Minecraft.setScreen` calls `removed()` on the OLD
                // screen, and the sign editor's `removed()` IS the commit — so
                // a screen REPLACING the editor sends too (this is what r64
                // observes). The framework screen is already gone; only the
                // packet and the view state are ours to settle.
                self.commit_sign_edit();
                self.sign_edit = None;
            }
            return;
        };
        let Some(session) = self.session.as_ref() else { return };
        let be_pos = rewo_world::block_entities::BlockEntityPos { x: pos.0, y: pos.1, z: pos.2 };
        let Some(be) = session.world.block_entities.get(be_pos) else {
            log::warn!(
                "live: open_sign_editor at ({}, {}, {}): no block entity there - ignored",
                pos.0,
                pos.1,
                pos.2
            );
            return;
        };
        let state_id = session.world.block_state_at(pos.0, pos.1, pos.2);
        let Some(sign) = self.sign_states.get(state_id) else {
            log::warn!("live: open_sign_editor at {:?}: not a sign state", pos);
            return;
        };
        // Seed the field with the face's current lines — vanilla edits read
        // `SignText.getMessage(i, true)` (the plain literal), and setMessage
        // preserves colour + glow, so those are captured once at open.
        let (_face, _) = be.sign_text(session.lang.as_deref());
        use rewo_world::sign_edit_screen::SignKind;
        let kind = if sign.hanging {
            SignKind::Hanging
        } else if sign.attachment == rewo_data::sign_states::SignAttachment::Wall {
            SignKind::Wall
        } else {
            SignKind::Standing
        };
        let (face, _) = be.sign_text(session.lang.as_deref());
        let initial = face.as_ref().map(|f| f.lines.clone()).unwrap_or_default();
        let dye =
            rewo_data::sign_text::dye_text_color(face.as_ref().and_then(|f| f.color.clone()).as_deref());
        let glowing = face.map(|f| f.glowing).unwrap_or(false);
        let done = self.lang.get_or_default("gui.done", "Done").to_string();
        let (gw, gh) = self.gui_size();
        self.sign_edit = Some(SignEditView {
            state: rewo_world::sign_edit_screen::SignEditState::new(initial, kind),
            pos,
            is_front: front,
            dye,
            glowing,
            wood: sign.wood_index,
            opened: std::time::Instant::now(),
        });
        let mut screen = rewo_world::sign_edit_screen::build_screen(gw, gh, &done);
        // The heading is `centeredText(font, title, width/2, 40, -1)` with
        // `title = Component.translatable("sign.edit")`; the label widget
        // needs its width, so the app builds it like the options titles do.
        let title = self.lang.get_or_default("sign.edit", "Edit sign message").to_string();
        let tw = self.text_width(&title);
        screen.widgets.push(rewo_world::screen::Widget::label(
            rewo_world::sign_edit_screen::TITLE_LABEL,
            (gw - tw) / 2,
            rewo_world::sign_edit_screen::TITLE_Y,
            tw,
            title,
        ));
        self.screen.screens.open(screen);
        self.grab_for_screen(true);
    }

    /// Rebuild when the numbers or the window changed (M84).
    pub(super) fn pump_stats_screen(&mut self) {
        use rewo_world::screen::ScreenKind;
        if self.stats.is_none() {
            return;
        }
        let updates = self.session.as_ref().map(|s| s.stats.updates).unwrap_or(0);
        let (gw, gh) = self.gui_size();
        let stale = self
            .screen
            .screens
            .current()
            .is_some_and(|s| s.kind == ScreenKind::Stats && (s.width != gw || s.height != gh));
        let fresh = self.stats.as_ref().is_some_and(|v| v.built_from != updates);
        if stale || fresh {
            self.rebuild_stats_screen();
        }
    }
}
