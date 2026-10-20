use super::*;

/// What one frame's later phases read from its world pass.
struct FrameView {
    dt: f32,
    /// Entity interpolation within the tick (`tick_accum / TICK_DT`).
    alpha: f32,
    eye: Vec3,
    extent: vk::Extent2D,
    aspect: f32,
}

impl LiveApp {
    /// One frame of the windowed client, as a sequence of phases.
    ///
    /// The session and the window state are taken out of `self` for the
    /// phases that need them, so each phase can be a `&mut self` method with
    /// the two passed alongside; they are put back before this returns. No
    /// phase after the take reaches `self.session` / `self.state` (while
    /// this was one function the borrow checker enforced that; it is now a
    /// property of the phase bodies).
    pub(super) fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let dt = self.frame_clock();
        // M186 — before anything can return early: a click on the
        // disconnect screen has no session behind it and must still sound.
        self.flush_ui_sounds();
        if !self.pump_screens(event_loop) {
            return;
        }
        let Some(mut session) = self.session.take() else {
            return;
        };
        let mut state = self.state.take();
        self.frame_session(event_loop, dt, &mut session, state.as_mut());
        self.session = Some(session);
        self.state = state;
    }

    fn frame_session(
        &mut self,
        event_loop: &ActiveEventLoop,
        dt: f32,
        session: &mut PlaySession,
        state: Option<&mut LiveState>,
    ) {
        if !self.tick_session(dt, session) {
            return;
        }
        let Some(state) = state else {
            return;
        };
        if !self.upload_meshes(event_loop, session, state) {
            return;
        }
        let view = self.world_pass(dt, session, state);
        self.gui_pass(&view, session, state);
        self.hud_pass(&view, session, state);
        self.submit(event_loop, &view, session, state);
    }

    /// Frame timing: this frame's `dt` (capped at 100 ms), fed to the
    /// overlay's sample rings.
    fn frame_clock(&mut self) -> f32 {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|l| now.duration_since(l).as_secs_f32())
            .unwrap_or(0.0)
            .min(0.1);
        self.last_frame = Some(now);
        if dt > 0.0 {
            self.cpu.push(dt * 1000.0);
            self.ring.push(dt * 1000.0);
        }
        dt
    }

    /// Everything that runs before the session is borrowed: screens the
    /// server or a button opened or closed, the disconnect, resizes and the
    /// screen pumps. Returns `false` when the frame ends here (exit, or no
    /// session — the screen-only frame).
    fn pump_screens(&mut self, event_loop: &ActiveEventLoop) -> bool {
        // An in-game copy or cut changed the edit boxes' clipboard: hand it
        // to the system clipboard (`KeyboardHandler.setClipboard`). The pull
        // side is on Ctrl+V, in the key handler.
        if self.clipboard != self.clipboard_synced {
            crate::os_clipboard::write(&self.clipboard);
            self.clipboard_synced = self.clipboard.clone();
        }
        // M74: `container_close` — the server closing whatever screen is
        // open. Drained before the session borrow below, because acting on it
        // calls `set_screen_open`, which needs all of `self`.
        //
        // Compared as a watermark, not consumed as a flag: the session owns
        // the counter and this is the only reader, so two closes in one frame
        // collapse to one screen close without either being lost.
        let close_requested = match self.session.as_ref() {
            Some(s) => {
                let n = s.client_state.close_container_requests();
                let changed = n != self.screen.close_requests_seen;
                self.screen.close_requests_seen = n;
                changed
            }
            None => false,
        };
        if close_requested && self.screen.inventory_open() {
            self.set_screen_open(false);
        }
        // M93m — a beacon button asked to close. Drained here rather than
        // closed from inside the press, so every close goes through the one
        // path that owns the screen. The beacon's buttons call
        // `player.closeContainer()` (a USER close), so it sends
        // `container_close` and clears the menu like Esc does.
        if std::mem::take(&mut self.screen.close_beacon) {
            self.close_container_screen();
        }

        // M89 — a container the server opened opens the client's screen.
        //
        // M87 decoded `open_screen` into `Menus` and rendered whatever menu
        // was open, but nothing turned the screen ON, so right-clicking a
        // chest recorded the menu and showed nothing: the render only engaged
        // if the player separately pressed E while a container happened to be
        // open. `handleOpenScreen` is `MenuScreens.create`, which *is* the
        // screen opening — the two are one action in vanilla.
        //
        // Watermarked like `close_requested` above rather than compared to the
        // screen's state, so a server that re-opens the same container id (a
        // fresh menu, per `reopening_replaces_the_slots_rather_than_keeping_them`)
        // is not mistaken for the one already showing.
        let opened = self
            .session
            .as_ref()
            .and_then(|s| s.menus.open().map(|m| m.container_id));
        if opened != self.screen.container_shown {
            self.screen.container_shown = opened;
            match opened {
                Some(id) => {
                    // M184: a lectern's screen is the BOOK, not the container
                    // panel - `LecternScreen extends BookViewScreen`. The
                    // generic `set_screen_open(true)` opens the Inventory-kind
                    // panel screen, which for a lectern paints nothing (m8)
                    // and would fight the book screen for the one framework
                    // slot. The reader opens below, via the lectern sync.
                    let is_lectern = self
                        .session
                        .as_ref()
                        .and_then(|s| s.menus.open())
                        .is_some_and(|m| m.container_id == id && m.layout.name == "lectern");
                    if !is_lectern {
                        self.set_screen_open(true);
                    }
                }
                // The menu closing closes the screen with it — but only if a
                // container was what put it up. Pressing E with no container
                // open must not be closed by this.
                None if self.screen.inventory_open() => self.set_screen_open(false),
                None => {}
            }
        }

        // M217 - `continueAttack`, once per client tick while the attack key
        // is held (survival block breaking and creative's repeat).
        self.mining_tick();
        // M229 - `handleKeybinds`' use half: release, and the held repeat.
        self.use_tick();

        // M219 - `Gui.tick`'s sleep half: the in-bed chat opens into an empty
        // slot while the player sleeps and hands over when they wake.
        self.bed_tick();

        // M184 - the lectern's menu-backed reader. `LecternScreen extends
        // BookViewScreen` (`LecternScreen.java:25`): the pages are the book
        // in the menu's slot 0, the page is container-data 0, and every
        // page turn is a `container_button_click` the server answers by
        // echoing the new page - the client never turns locally
        // (`LecternScreen.pageBack/Forward/forcePage` are ALL sends).
        //
        // The watermark mirrors the `opened` block above rather than
        // comparing screen state, for the same reason that block does: a
        // server re-opening the same container id is a fresh menu, and a
        // replaced screen (a held book's `open_book` while the lectern menu
        // lives) must not resurrect.
        let lectern_menu = self
            .session
            .as_ref()
            .and_then(|s| super::screens::lectern_menu_state(&s.menus));
        match lectern_menu {
            Some((container_id, serial, page)) => {
                let identity = (container_id, serial);
                if super::screens::lectern_should_open(
                    identity,
                    self.lectern_shown,
                    self.lectern_superseded,
                ) {
                    // `handleOpenScreen` IS the screen opening. `mayBuild`
                    // is not on the wire - vanilla's own setter makes it
                    // `!isSpectator()`, which is what decides the
                    // two-button footer.
                    let (gw, gh) = self.gui_size();
                    let screen = self.book_view_screen_for(gw, gh, true);
                    self.screen.screens.open(screen);
                    self.book = Some(rewo_ui::book_view_screen::BookViewScreen::new(Vec::new()));
                    self.lectern_pages = None;
                    self.lectern_shown = Some(identity);
                    self.grab_for_screen(true);
                }
                // The listener half - only while this identity's reader is
                // the one up. Pages resolve from slot 0's book; the page
                // number is data slot 0. `force_page` is a no-op when the
                // echo agrees, so re-forcing each frame is the
                // eventual-consistency the client actually has.
                if self.lectern_shown == Some(identity) {
                    let advance = self.advance();
                    let pages: Option<Vec<Vec<rewo_world::chat_style::ChatLine>>> =
                        self.session.as_ref().and_then(|s| {
                            let m = s.menus.open()?;
                            let stack = m.menu.menu_slot(0)?;
                            let text = m.menu.text_of(stack)?;
                            resolve_book_pages(text, advance.as_ref(), Some(&self.lang))
                        });
                    if self.lectern_pages.as_ref() != pages.as_ref() {
                        if let Some(p) = pages.clone() {
                            self.book = Some(rewo_ui::book_view_screen::BookViewScreen::new(p));
                        }
                        self.lectern_pages = pages;
                    }
                    if let Some(b) = self.book.as_mut() {
                        b.force_page(page);
                    }
                }
            }
            None => {
                // The menu closed (the server's `container_close`, or an
                // `open_screen` for something else). Vanilla's screen goes
                // with its menu; no local send - the server already knows.
                // Both markers clear: with the menu gone there is nothing
                // left to not-resurrect.
                let was_shown = self.lectern_shown.take().is_some();
                let was_superseded = self.lectern_superseded.take().is_some();
                if was_shown || was_superseded {
                    self.lectern_pages = None;
                    self.book = None;
                    self.screen.screens.close();
                    self.grab_for_screen(false);
                }
            }
        }

        // `--render-check` scripted injections (no-op without `--render-check`,
        // and not compiled at all without `--features gates`).
        #[cfg(feature = "gates")]
        self.render_check_inject();

        // M85: mirror the server's links out of the session while it is
        // alive. The disconnect screen reads them *after* it is gone — see the
        // field's docs.
        if let Some(s) = self.session.as_ref() {
            if s.session.server_links != self.server_links {
                self.server_links = s.session.server_links.clone();
                log::info!("live: {} server link(s)", self.server_links.len());
            }
        }

        // M85: the connection ended. Before the session borrow below, because
        // it drops the session — and vanilla's `onDisconnect` is likewise the
        // thing that tears the level down rather than something the level does.
        let ended = self.session.as_ref().and_then(|s| {
            s.disconnect.clone().map(|reason| {
                (
                    s.disconnect_cause
                        .unwrap_or(rewo_ui::disconnect_screen::DisconnectCause::EndOfStream),
                    reason,
                )
            })
        });
        if let Some((cause, reason)) = ended {
            self.death = None;
            self.open_disconnect_screen(cause, reason);
        }

        // M85: a window resize while one of M85's screens is up. Same
        // watermark shape as `pump_death_screen`'s — compare the screen's
        // recorded size so a frame that changes nothing rebuilds nothing.
        {
            let (gw, gh) = self.gui_size();
            let stale = !matches!(self.view, ScreenView::None)
                && self
                    .screen
                    .screens
                    .current()
                    .is_some_and(|s| s.width != gw || s.height != gh);
            if stale {
                self.rebuild_view_screen();
            }
        }

        // M82: you died, or you respawned. Both before the session borrow
        // below, for the same reason `container_close` is.
        self.pump_death_screen();
        self.pump_stats_screen();
        self.pump_book_screen();
        self.pump_sign_editor();
        // M173 — the options screens rebuild on resize (the Done and the
        // band re-centre); only on a CHANGE, or ticks/focus reset every
        // frame (M82's rule).
        if self.options_view.is_some() {
            let (gw, gh) = self.gui_size();
            let stale = self.screen.screens.current().is_some_and(|s| {
                s.kind == rewo_ui::screen::ScreenKind::Options && (s.width != gw || s.height != gh)
            });
            if stale {
                self.rebuild_options_screen();
            }
            // The one-slot rule (M172): anything replacing the options
            // screen drops the app-side view state with it.
            if !self
                .screen
                .screens
                .current()
                .is_some_and(|s| s.kind == rewo_ui::screen::ScreenKind::Options)
            {
                self.options_view = None;
                self.options_drag = None;
            }
        }
        if self.exit_requested {
            event_loop.exit();
            return false;
        }

        // **M85: the frame with no session.**
        //
        // Every frame before this one needed a world: the loop below borrows
        // `self.session` and returns if there is none, which is why the client
        // used to `event_loop.exit()` on a disconnect rather than showing a
        // screen. The disconnect screen exists *because* there is no session,
        // so it gets its own arm — appended, not woven into the path below,
        // which stays exactly as it was.
        //
        // What it draws is the screen and nothing else. The world pass runs
        // with an identity view-projection over an empty world (no columns, no
        // entities), which is what `deathshot` has been doing offscreen since
        // M82; the menu background is opaque, so nothing behind it is visible
        // anyway.
        if self.session.is_none() {
            self.render_screen_only(event_loop);
            return false;
        }
        true
    }

    /// Packets and the fixed 20 Hz tick. Returns `false` when the session
    /// failed or disconnected (serviced at the top of the next frame).
    fn tick_session(&mut self, dt: f32, session: &mut PlaySession) -> bool {
        // Fixed 20 Hz tick on an accumulator.
        // Vanilla applies inbound packets every frame, independent of the
        // 20 Hz tick; the budget keeps a backlog from hitching one frame.
        if let Err(e) = session.pump(rewo_net::play::PumpBudget::FRAME) {
            log::error!("live: packet handling failed: {e}");
            session.disconnect = Some(e);
            session.disconnect_cause =
                Some(rewo_ui::disconnect_screen::DisconnectCause::ClientError);
            return false;
        }
        self.tick_accum += dt;
        let input = self.keys.input();
        let mut ran_tick = false;
        while self.tick_accum >= TICK_DT {
            self.tick_accum -= TICK_DT;
            if let Err(e) = session.tick(&input) {
                // `onPacketError` — a handler threw. Vanilla's is the path that
                // fills `bugReportLink`, so this is a `ClientError` and the
                // server's own bug-report link (if it sent one) appears on the
                // disconnect screen (M85).
                //
                // Recorded onto the session rather than acted on here: `self`
                // is mutably borrowed through `session` for this whole block,
                // and routing it through the one field the top of the frame
                // already reads keeps every disconnect on one path.
                log::error!("live: tick failed: {e}");
                session.disconnect = Some(e);
                session.disconnect_cause =
                    Some(rewo_ui::disconnect_screen::DisconnectCause::ClientError);
                return false;
            }
            // Advance the block-light flicker exactly once per successful tick.
            self.flicker.tick();
            // M131 — and drain the tick's sounds. Per *tick*, not per frame:
            // `MIN_SOURCE_LIFETIME` is 20 ticks, so a per-frame drive would tie
            // a channel's grace period to the frame rate.
            let queued = session.take_sound_events();
            self.sounds.drive(
                &queued,
                &session.world.entities,
                session.local_player_view(),
                session.game_time(),
                session.music_volume(),
            );
            // M143 — report the backend's counters when they MOVE, not on a
            // timer. Every one of them is an event ("a sound could not be
            // resolved", "the ring filled") rather than a level, so a change is
            // exactly the thing worth a line, and a periodic dump of four
            // unchanging zeroes trains a reader to ignore it.
            let audio = self.sounds.sink_diagnostics();
            if audio != self.last_audio {
                log::warn!(
                    "audio: {} unresolved, {} stream(s) failed, {} command(s) dropped, \
                     {} device error(s), {} buffer(s) cached",
                    audio.unresolved,
                    audio.streams_failed,
                    audio.dropped,
                    audio.device_errors,
                    audio.cached_buffers,
                );
                self.last_audio = audio;
            }
            // M71 — a client-generated system message (currently only
            // `NO_RESPAWN_BLOCK_AVAILABLE`) is queued as a *translation key*,
            // because vanilla builds a `Component.translatable` and resolves
            // it against the loaded language at render time. This is that
            // resolution; the key itself is the fallback, which is what
            // vanilla shows for a key the language file lacks.
            for key in session.game_state.take_system_messages() {
                let text = self
                    .baked
                    .as_ref()
                    .and_then(|b| b.lang.get(key))
                    .unwrap_or(key)
                    .to_string();
                session.chat_log.push(text);
            }
            // `Hud.tick`'s held-item label clock (M66) — once per client tick,
            // and it reads the selected stack *after* the tick that may have
            // changed it, exactly as vanilla's `Gui.tick` does.
            let label = self
                .baked
                .as_ref()
                .and_then(|b| selected_item_label(session, &self.items, &b.item_names, &b.lang));
            self.tool_highlight.tick(
                label.as_ref().map(|(id, n)| (*id, n.as_str())),
                NOTIFICATION_DISPLAY_TIME,
            );
            // M82: `Screen.tick()` — once per client tick, for whatever screen
            // is up. The death screen's is `delayTicker++`, and its buttons
            // arm at exactly 20.
            if self.death.is_some() {
                if let Some(s) = self.screen.screens.current_mut() {
                    rewo_ui::death_screen::DeathScreen::tick(s);
                }
            }
            ran_tick = true;
        }
        if session.disconnect.is_some() {
            // Serviced at the top of the *next* frame, by the block that opens
            // the disconnect screen. Returning here rather than acting is what
            // keeps that decision in one place — and this frame has already
            // borrowed the session it is about to drop.
            return false;
        }
        if ran_tick && session.spawned && !self.logged_spawn {
            self.logged_spawn = true;
            // M229 — the origin `REWO_PRECMD`'s `~` coordinates are
            // relative to, for the tail witnesses that walk back to them.
            #[cfg(feature = "gates")]
            {
                self.precmd_origin = Some([session.player.x, session.player.y, session.player.z]);
            }
            // `REWO_PRECMD`: the same semicolon-separated op-command knob the
            // headless path has had since M64, wired into the windowed one
            // (M82) — without it a windowed run cannot stage anything that
            // needs a command, and the death screen needs `/kill`.
            if let Ok(cmd) = crate::knobs::var("REWO_PRECMD") {
                for one in cmd.split(';').map(str::trim).filter(|c| !c.is_empty()) {
                    let _ = session.send_command(one);
                    log::info!("REWO_PRECMD: {one}");
                }
            }
            // M108 — `--render-check` sends its own chat line rather than
            // making this a THIRD caller requirement beside r14's hotbar and
            // r25's recipe book. The server echoing it back is what drives
            // `player_chat` through the signature cache, the trust level, the
            // wrap and the geometry, so r26 grades the whole chain on an
            // otherwise unstaged run.
            #[cfg(feature = "gates")]
            if self.check.is_some() {
                let _ = session.send_chat("rewo render-check");
                // M164 (r53) — a waterlogged block for the mesher to find.
                // ABOVE the player, not beside: `~ ~4 ~` is air wherever a
                // player is standing, while `~2 ~ ~` assumes flat ground.
                // M228: four up, not two: r72's injected horse stands on this
                // spot, and a rider seated by `positionRider` has its eye at
                // about +2.4, inside a slab at +2 (r81's pick hit the slab).
                let _ = session.send_command(
                    "setblock ~ ~4 ~ minecraft:oak_slab[type=bottom,waterlogged=true]",
                );
            }
            log::info!(
                "live: spawned at ({:.1},{:.1},{:.1})",
                session.player.x,
                session.player.y,
                session.player.z
            );
            // Test-only: put a stack of dirt in slot 0 so right-click can
            // place (the test server is creative). A creative-inventory
            // packet is item synthesis, so it is never sent by a default
            // build and, in a `gates` build, only when `REWO_GIVE_DIRT` is
            // set. `knobs::var` answers "not set" without the feature.
            if crate::knobs::var("REWO_GIVE_DIRT").is_ok() {
                if let Some(dirt) = self.dirt_item {
                    let _ = session.creative_set_hotbar(0, dirt, 64);
                    let _ = session.select_hotbar(0);
                }
            }
        }
        true
    }

    /// Finished meshes to the GPU, dirty columns to the pool. Returns
    /// `false` when meshing failed (the loop exits).
    fn upload_meshes(
        &mut self,
        event_loop: &ActiveEventLoop,
        session: &mut PlaySession,
        state: &mut LiveState,
    ) -> bool {
        // Upload finished meshes + feed the worker pool. (Uploads are
        // async slot-ring submissions — the CPU never waits on the copy;
        // same-queue FIFO ordering keeps this frame's draws safe.)
        #[cfg(feature = "gates")]
        let mut carried_cells = self.check.as_ref().map_or(0, |c| c.carried_fluid_cells);
        #[cfg(not(feature = "gates"))]
        let mut carried_cells = 0;
        let pumped = pump_meshing(
            session,
            &mut state.gpu,
            &mut state.world_renderer,
            &mut self.pool,
            UPLOAD_BUDGET,
            &mut carried_cells,
        );
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            c.carried_fluid_cells = carried_cells;
        }
        match pumped {
            Ok(n) => {
                self.uploaded_total += n;
                // Log once when the pool first idles after spawn + a settle
                // margin (the chunk stream arrives over the first seconds —
                // firing on the tiny pre-stream batch would be misleading).
                if !self.flood_logged
                    && session.spawned
                    && self.uploaded_total > 0
                    && self.pool.in_flight() == 0
                    && session.dirty_len() == 0
                    && self.started.elapsed().as_secs_f32() >= 2.0
                {
                    self.flood_logged = true;
                    log::info!(
                        "live: initial mesh flood done — {} uploads for {} columns in {:.1}s",
                        self.uploaded_total,
                        session.world.loaded_columns(),
                        self.started.elapsed().as_secs_f32()
                    );
                }
            }
            Err(e) => {
                log::error!("live: remesh failed: {e}");
                event_loop.exit();
                return false;
            }
        }
        true
    }

    /// The world pass: skins, the lightmap, entities, block entities, the
    /// sound listener, selection, leashes and the sky.
    fn world_pass(
        &mut self,
        dt: f32,
        session: &mut PlaySession,
        state: &mut LiveState,
    ) -> FrameView {
        // Player skins: request any newly-announced ones, upload any that
        // finished fetching (real skins on online-mode servers).
        for (uuid, info) in session.take_pending_skins() {
            self.skins.request(uuid, &info);
        }
        self.skins.own = session.own_uuid;
        self.skins
            .poll_uploads(&mut state.gpu, &mut state.world_renderer);
        self.skins.drop_evicted(&mut state.world_renderer);
        {
            // A player is needed again once loaded in the world (skin, cape)
            // or shown in the open tab list (face).
            let loaded: std::collections::HashSet<u128> =
                session.world.entities.iter().map(|(_, e)| e.uuid).collect();
            let tab_open = self.keys.tab_list && !self.hud_hidden;
            let listed = &session.tab_players.listed;
            self.skins
                .restore_evicted(&mut state.gpu, &mut state.world_renderer, |u| {
                    loaded.contains(&u) || (tab_open && listed.contains(&u))
                });
            // Players who left: forget their texture state (bounded by
            // whoever the session still tracks, not by how many ever joined).
            // "Tracked" is the session's own skin table: it is what decides
            // whether an unchanged skin is announced again, so pruning on any
            // other guess (listed, a gamemode or latency entry, a loaded
            // entity) would drop an NPC-style player the table still holds
            // and leave them on the default skin for good.
            if self.skins.prune_due() {
                self.skins.retain_players(|u| session.tracks_player_skin(u));
            }
        }
        // The inventory preview draws the local player from its own atlas, so
        // it takes the pixels rather than the world pass's upload.
        if let Some(own) = self.skins.take_own_preview() {
            self.preview_skin = Some(own);
        }

        // Entities: frame-interpolated snapshot + camera-billboarded tags.
        let alpha = (self.tick_accum / TICK_DT).clamp(0.0, 1.0);
        let anim_time = self.started.elapsed().as_secs_f32();
        // Resolve ONE camera lightmap for this frame at a FIXED partial of 1.0
        // — not the entity-interpolation `alpha`. Vanilla `GameRenderer` (lines
        // 376-386) calls `lightmapRenderStateExtractor.extract(lightmapRenderState,
        // 1.0F)` with a hard-coded 1.0F, so the camera lightmap is resolved at
        // the tick boundary regardless of the frame's partial tick. Entity
        // position interpolation still uses `alpha` (below); only the lightmap /
        // effect snapshot is pinned to 1.0. This matches the headless path.
        let lightmap_partial = 1.0;
        let snapshot = session.visual_effect_snapshot(lightmap_partial);
        let lightmap = resolve_lightmap(
            session.day_ticks,
            session.active_dimension_type.as_ref(),
            self.flicker.block_factor(),
            snapshot,
            // M52 Full Bright: pin vanilla's MAXIMUM gamma rather than
            // bypassing the lightmap. The value still goes through
            // `darkness_lightmap` -> `brightness_factor` -> the exact mix M13
            // transcribed, so night vision and the darkness effect keep
            // composing with it correctly. A bypass would have made Full
            // Bright silently defeat both.
            if self.modules.is_on("fullbright") {
                crate::modules::MAX_GAMMA
            } else {
                self.gamma
            },
            self.darkness_option,
            lightmap_partial,
            end_flash_for(session, lightmap_partial, &self.options),
        );
        let trim_slots = ensure_trims(
            session,
            &self.items,
            &self.trims,
            &mut state.gpu,
            &mut state.world_renderer,
        );
        // M272 — player heads wearing skins of their own: build the model of
        // each skin that arrived (the GPU gets the new model and texture, in
        // the bake's order), then pick each head's.
        if let Some(baked) = self.baked.as_mut() {
            let mut new_models = Vec::new();
            let mut new_textures = Vec::new();
            for (url, rgba) in self.head_skins.take_ready() {
                let Some(made) = baked.held_items.add_skin_head_model(rgba) else {
                    continue;
                };
                let (name, tex) = (made.block, made.tex);
                if let Some(item) = made.item {
                    if let Some(m) = baked.held_items.models.get(&item) {
                        new_models.push((item.clone(), to_gpu_held_model(m)));
                    }
                    self.head_skins.items.insert(url.clone(), item);
                }
                if let Some(m) = baked.held_items.block_entities.get(&name) {
                    new_models.push((name.clone(), to_gpu_held_model(m)));
                }
                new_textures.push(to_gpu_held_texture(&baked.held_items.textures[tex]));
                self.head_skins.models.insert(url, name);
            }
            if !new_textures.is_empty() {
                state
                    .world_renderer
                    .extend_held_items(new_models, new_textures);
                // M275 — the GUI pass keeps its own copy of the held items;
                // rebuilt whole (a skin arrives at most `MAX_HEAD_SKINS`
                // times a session), so its texture indices stay the bake's.
                if let Some(g) = self.gui_items.as_mut() {
                    g.held = to_gpu_held_items(&baked.held_items);
                }
            }
        }
        let head_models = head_skin_models(&session.world, &mut self.head_skins);
        // M273 — ask for the skins worn player heads name; M275 — and the
        // ones head items in the inventory name, whose icons take them.
        for v in session
            .world
            .entities
            .head_profiles()
            .chain(session.inventory.profile_skins())
        {
            self.head_skins.request(v);
        }
        if let Some(g) = self.gui_items.as_mut() {
            for v in session.inventory.profile_skins() {
                if !g.skinned.contains_key(v) {
                    if let Some(m) = self.head_skins.ready_item(v) {
                        g.skinned.insert(v.to_string(), m.to_string());
                    }
                }
            }
        }
        let draws = collect_entities(
            session,
            &self.etypes,
            alpha,
            &mut self.gestures,
            anim_time,
            &self.skins.registry,
            &lightmap,
            &self.spears,
            self.bow_item,
            &self.items,
            &self.equipment,
            &trim_slots,
            &self.etf,
            self.hud_hidden,
            frame_crosshair_pick(session, &self.etypes, alpha),
            Some(&self.head_skins),
        );
        // M163 — read the label OFF THE DRAW LIST, not off the entity table:
        // the claim is that the resolved string reached the renderer, and a
        // table-level read would stay green if `resolve_labels` dropped it.
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            // M166 — mirror only when it has grown. Fixed at configuration
            // time except for a mid-session `resource_pack_push`, so this is a
            // no-op on all but a handful of frames.
            if c.config_tasks.pack_replies.len() != session.config_tasks.pack_replies.len()
                || c.config_tasks.codes_of_conduct.len()
                    != session.config_tasks.codes_of_conduct.len()
            {
                c.config_tasks = session.config_tasks.clone();
            }
            let named = |want: &str| draws.iter().any(|d| d.name == Some(want));
            if named("Zombie") {
                c.nametag_resolved_frames += 1;
            }
            if named("entity.minecraft.zombie") {
                c.nametag_raw_key_frames += 1;
            }
        }
        let (cr, cu) = camera_basis(session.player.yaw, session.player.pitch);
        let eye = player_eye(session);
        // M138a — `SoundEngine.updateSource(camera)`, and it lives HERE rather
        // than beside `sounds.drive` because vanilla calls it from the render
        // path: per frame, with the camera this frame is about to use. The tick
        // loop drains sound *events*; this moves the ears.
        self.sounds
            .update_listener(eye_f64(session), session.player.yaw, session.player.pitch);
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            // Read BACK off the device rather than counting the call: deleting
            // `update_listener`'s body would leave a call-site counter green.
            c.listener_pushes = self.sounds.device.listener_pushes;
            c.listener_forward = self.sounds.device.last_listener.map(|t| t.forward);
            // Read off the MANAGER, which is the engine's, rather than off
            // anything this frame computed — the claim is that a track was
            // actually started, not that a situation was named.
            if let Some(music) = self.sounds.system.music.current() {
                c.music_started = Some(music.to_string());
            }
            // M162 — LATCHED rather than sampled at the end, because a one-shot
            // is reclaimed 20 ticks after its channel is released and the run
            // goes on for several seconds after the injection. Read off the
            // ENGINE, not off anything the injection computed.
            if c.global_event_sound.is_none() {
                c.global_event_sound = self
                    .sounds
                    .system
                    .engine
                    .live_position("minecraft:entity.wither.spawn");
            }
            if c.explosion_sound.is_none() {
                c.explosion_sound = self
                    .sounds
                    .system
                    .engine
                    .live_position("minecraft:entity.generic.explode");
            }
            c.queued_delayed = c
                .queued_delayed
                .max(self.sounds.system.stats.queued_delayed);
            c.explosion_tails = c.explosion_tails.max(session.motion_stats.explosion_tails);
            c.explosion_sounds_queued = c
                .explosion_sounds_queued
                .max(session.motion_stats.explosion_sounds);
        }
        // Before `set_entities`: the entity pass reads the eye as the CEM
        // `player_pos_*`, which FA aims mob eyes/heads with.
        state.world_renderer.set_camera(eye.to_array());
        apply_lightmap(
            &mut state.world_renderer,
            &lightmap,
            session.day_ticks,
            session.active_dimension_type.as_ref(),
            {
                let w = effective_weather(session);
                (w.rain_level(), w.thunder_level())
            },
            {
                let band = match self.baked.as_ref() {
                    Some(baked) => {
                        let w = self
                            .weather
                            .get_or_insert_with(|| WeatherAssets::new(baked));
                        rain_fog_band(session, w, Some(dt * 20.0))
                    }
                    None => [1.0e9, 1.0e9 + 1.0],
                };
                // r3: the sentinel the dead branch produced is 1e9 blocks out.
                // Counting a *finite* band rather than "we took the Some arm"
                // is what makes this a value witness — the arm could be taken
                // and still hand the renderer nonsense.
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    if band[0] < 1.0e8 {
                        c.fog_band_frames += 1;
                    }
                }
                band
            },
        );
        apply_biome_sky_fog(&mut state.world_renderer, session);
        let mut bes = collect_block_entities(
            &session.world,
            &self.chest_states,
            &head_models,
            &lightmap,
            alpha,
            session.game_time(),
            (cr, cu),
        );
        // M194: item frames — the frame model and the framed item ride the
        // block-entity emitter (static quads at a pose); a framed map samples
        // the entity pass's map band.
        let frames = collect_item_frames(
            &session.world,
            &session.maps,
            Some(&self.lang),
            &self.etypes,
            &self.items,
            &lightmap,
            alpha,
        );
        prepare_frame_maps(
            &mut state.world_renderer,
            &mut state.gpu,
            &frames,
            &session.maps,
        );
        // r70 — the injected frame's model and sword, at its position, and
        // no entity body there (it used to fall through to the capsule).
        // r71 — the server's own frame, read with the summoned values.
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            let summoned = collect_frame_inputs(
                &session.world,
                &session.maps,
                Some(&self.lang),
                &self.etypes,
                &self.items,
                alpha,
            )
            .iter()
            .any(|f| {
                f.facing == 2
                    && f.rotation == 3
                    && !f.glow
                    && f.item.as_deref() == Some("minecraft:diamond_sword")
                    && Some(f.pos) != c.item_frame_pos
            });
            if summoned {
                c.server_frame_frames += 1;
            }
            if let Some(pos) = c.item_frame_pos {
                let at = |m: &str, fixed: bool| {
                    frames
                        .models
                        .iter()
                        .any(|d| d.pos == pos && d.model == m && d.fixed_display == fixed)
                };
                if at("rewo:be/item_frame", false)
                    && at("minecraft:diamond_sword", true)
                    && !draws.iter().any(|d| d.pos == pos)
                {
                    c.item_frame_frames += 1;
                }
            }
            // r105 — the two staged armour stands, as their model (not the
            // capsule), each with its own flags off the wire: the plain one
            // with the default parts, the posed one with arms, no base plate
            // and its helmet.
            let stand = |arms: bool, plate: bool, helmet: bool| {
                draws.iter().any(|d| {
                    d.kind == rewo_gpu::mobs::EntityModelKind::ArmorStand
                        && d.stand_parts.arms == arms
                        && d.stand_parts.base_plate == plate
                        && d.armor[0].is_some() == helmet
                })
            };
            if stand(false, true, false) && stand(true, false, true) {
                c.armor_stand_frames += 1;
            }
            // M252 — r111: the summoned minecart drew as its model.
            if draws
                .iter()
                .any(|d| d.kind == rewo_gpu::mobs::EntityModelKind::Minecart)
            {
                c.minecart_frames += 1;
            }
            // r107 — r94's snowball and bobber, drawn as camera-facing items
            // (the snowball's own item; the bobber's hook sprite) rather
            // than capsules.
            // ...and the name has baked geometry to draw: the first cut
            // pointed the hook at a texture path that did not exist, so the
            // bobber reached the draw list and drew nothing.
            let baked_geometry = |item: &str| {
                self.baked
                    .as_ref()
                    .and_then(|b| b.held_items.any(item))
                    .is_some_and(|m| !m.quads.is_empty())
            };
            let thrown = |item: &str| {
                baked_geometry(item)
                    && draws
                        .iter()
                        .any(|d| d.ground_item == Some(item) && d.billboard.is_some())
            };
            if thrown("minecraft:snowball") {
                c.thrown_frames.0 += 1;
            }
            if thrown("rewo:be/fishing_hook") {
                c.thrown_frames.1 += 1;
            }
        }
        bes.extend(frames.models);
        // M250 — arrows and thrown tridents, by their models.
        bes.extend(collect_projectiles(session, &self.etypes, &lightmap, alpha));
        // M258 — wither skulls, llama spit, shulker bullets.
        bes.extend(collect_small_projectiles(
            session,
            &self.etypes,
            &lightmap,
            alpha,
        ));
        // M259 — evoker fangs.
        bes.extend(collect_evoker_fangs(
            session,
            &self.etypes,
            &lightmap,
            alpha,
        ));
        // M257 — end crystals.
        bes.extend(collect_end_crystals(
            session,
            &self.etypes,
            &lightmap,
            alpha,
        ));
        // M260 — block displays: build any state model the bake did not
        // make, hand the GPU just what is new, then draw them.
        if let Some(baked) = self.baked.as_mut() {
            let mut new_models = Vec::new();
            let mut new_textures = Vec::new();
            for st in block_display_states(session, &self.etypes, &self.blocks) {
                if let Some((name, first)) = baked.add_state_model(st, &mut self.runtime_states) {
                    if let Some(m) = baked.held_items.block_entities.get(&name) {
                        new_models.push((name, to_gpu_held_model(m)));
                    }
                    new_textures.extend(
                        baked.held_items.textures[first..]
                            .iter()
                            .map(to_gpu_held_texture),
                    );
                }
            }
            if !new_models.is_empty() || !new_textures.is_empty() {
                state
                    .world_renderer
                    .extend_held_items(new_models, new_textures);
            }
        }
        bes.extend(collect_block_displays(
            session,
            &self.etypes,
            &lightmap,
            alpha,
        ));
        // M261 — what minecarts carry.
        bes.extend(collect_minecart_contents(
            session,
            &self.etypes,
            &self.blocks,
            &self.chest_states,
            &lightmap,
            alpha,
        ));
        // M256 — item displays.
        bes.extend(collect_item_displays(
            session,
            &self.etypes,
            &self.items,
            self.baked.as_ref().map(|b| &b.held_items),
            &lightmap,
            alpha,
        ));
        // M254 — paintings.
        bes.extend(collect_paintings(session, &self.etypes, &lightmap, alpha));
        // M253 — falling blocks and primed TNT, by their block models.
        bes.extend(collect_carried_blocks(
            session,
            &self.etypes,
            &lightmap,
            alpha,
            self.blocks.default_state("minecraft:tnt"),
        ));
        let frame_maps = frames.maps;
        // A spawner's caged mob rides the ENTITY pass, mounted inside its
        // block (M31), so it joins the entity draws rather than these.
        let caged = collect_spawner_mobs(
            &session.world,
            &self.etypes,
            self.chest_states.spawner_states(),
            &lightmap,
            alpha,
        );
        let portals = collect_end_portals(
            &session.world,
            self.chest_states.end_portal_states(),
            self.chest_states.end_gateway_states(),
        );
        if let Err(e) =
            state
                .world_renderer
                .set_end_portals(&mut state.gpu, &portals, session.game_time())
        {
            log::warn!("live: end portal upload failed: {e}");
        }
        let mut draws = draws;
        draws.extend(caged.iter().map(spawner_mob_draw));
        draws.extend(collect_pickups(
            session,
            &self.items,
            &lightmap,
            alpha,
            anim_time,
        ));
        let mut held: Vec<&str> = draws.iter().flat_map(|d| d.held).flatten().collect();
        // M273 — worn skulls page their sheets in like any held model.
        held.extend(draws.iter().filter_map(|d| d.worn_head.map(|w| w.model)));
        held.extend(draws.iter().filter_map(|d| d.ground_item));
        held.extend(bes.iter().map(|b| b.model.as_str()));
        // A failed upload leaves the item simply absent (no resident slot →
        // no quads), which is preferable to killing the frame loop.
        if let Err(e) = state
            .world_renderer
            .prepare_held_items(&mut state.gpu, &held)
        {
            log::warn!("live: held-item texture upload: {e}");
        }
        // M278 — the glowing outline's targets, at the swapchain's size.
        if let Err(e) = state.world_renderer.ensure_outline(
            &mut state.gpu,
            state.renderer.swapchain.extent,
            rewo_gpu::world::unorm_of(state.renderer.swapchain.format),
        ) {
            log::warn!("live: outline targets: {e}");
        }
        let be_draws: Vec<_> = bes.iter().map(|b| b.as_draw()).collect();
        let frame_map_draws: Vec<_> = frame_maps.iter().map(|m| m.as_draw()).collect();
        let sign_lines = match state.world_renderer.font_advance() {
            Some(a) => {
                let mut v = collect_session_sign_text(session, &self.sign_states, &lightmap, a);
                // M255 — text displays ride the same world-text path.
                v.extend(collect_text_displays(
                    session,
                    &self.etypes,
                    &lightmap,
                    alpha,
                    a,
                ));
                v
            }
            None => Vec::new(),
        };
        // M163/r49 — read the sign line OFF THE DRAW LIST, for r48's reason
        // one list over: the claim is that the resolved string reached the
        // renderer. The RAW-KEY half is a prefix match because
        // `getRenderMessages` truncates each line to the board, so an
        // unresolved `block.minecraft.dirt` arrives as `block.minecraft.di`.
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            if sign_lines.iter().any(|l| l.text == SIGN_WITNESS_RESOLVED) {
                c.sign_resolved_frames += 1;
            }
            // M262 — r121: the staged text display's plate, the synced default
            // `0x40000000` (black at 64/255).
            if sign_lines.iter().any(|l| {
                l.backdrop
                    .is_some_and(|(_, c)| c[..3] == [0.0; 3] && (c[3] - 64.0 / 255.0).abs() < 1e-6)
            }) {
                c.text_display_backdrop_frames += 1;
            }
            // M268 — r126: the staged text display's drop shadow, gold at a
            // quarter (`ARGB.scaleRGB(0xFFAA00, 0.25)` = 0x3F2A00), behind.
            let shadow = super::entities::linear_rgb(0x3F, 0x2A, 0x00);
            if sign_lines.iter().any(|l| {
                l.text == "RewoHolo"
                    && l.z == super::entities::TEXT_DISPLAY_SHADOW_DEPTH
                    && (0..3).all(|k| (l.color[k] - shadow[k]).abs() < 1e-6)
            }) {
                c.text_display_shadow_frames += 1;
            }
            // M255 — r114: the staged text display's gold line.
            let gold = super::entities::linear_rgb(0xFF, 0xAA, 0x00);
            if sign_lines.iter().any(|l| {
                l.text == "RewoHolo" && (0..3).all(|k| (l.color[k] - gold[k]).abs() < 1e-3)
            }) {
                c.text_display_frames += 1;
            }
            if sign_lines
                .iter()
                .any(|l| !l.text.is_empty() && SIGN_WITNESS_KEY.starts_with(l.text.as_str()))
            {
                c.sign_raw_key_frames += 1;
            }
        }
        let sign_draws: Vec<_> = sign_lines
            .iter()
            .map(|l| rewo_gpu::entities::WorldTextDraw {
                transform: l.transform,
                text: &l.text,
                x: l.x,
                y: l.y,
                z: l.z,
                color: l.color,
                light: l.light,
                backdrop: l.backdrop,
            })
            .collect();
        // M270 — r128: the staged posed armour stand reached the entity draws
        // with its head and right-arm poses.
        #[cfg(feature = "gates")]
        if let Some(rc) = self.check.as_mut() {
            // M271 — r129: the staged hologram (an invisible armour stand
            // with a visible name) drew its name and not its body.
            if draws.iter().any(|d| {
                d.kind == rewo_gpu::mobs::EntityModelKind::ArmorStand
                    && d.body_hidden
                    && d.name.is_some_and(|n| n.contains("RewoStand"))
            }) {
                rc.hologram_stand_frames += 1;
            }
            // M273 — r131: the staged armour stand wears a player head whose
            // profile names r130's skin; it reached the draws as the
            // per-skin model, every quad of which samples the synthetic skin.
            let skinned = |m: &str| {
                self.baked.as_ref().is_some_and(|b| {
                    b.held_items.any(m).is_some_and(|mm| {
                        mm.quads.iter().all(|q| {
                            b.held_items
                                .textures
                                .get(q.tex as usize)
                                .is_some_and(super::render_check::is_synthetic_skin)
                        })
                    })
                })
            };
            if draws.iter().any(|d| {
                d.kind == rewo_gpu::mobs::EntityModelKind::ArmorStand
                    && d.worn_head.is_some_and(|w| {
                        w.model.starts_with("rewo:be/player_head#") && skinned(w.model)
                    })
            }) {
                rc.worn_skin_head_frames += 1;
            }
            // M274 — r132: the staged stand wearing a carved pumpkin reached
            // the draws through the head item branch.
            if draws.iter().any(|d| {
                d.kind == rewo_gpu::mobs::EntityModelKind::ArmorStand
                    && d.worn_head
                        .is_some_and(|w| w.item && w.model == "minecraft:carved_pumpkin")
            }) {
                rc.worn_item_head_frames += 1;
            }
            // M278 — r137: the staged glowing armour stand reached the draws
            // with its outline colour (white: no team), and the outline pass
            // ran.
            if state.world_renderer.outline_drawn()
                && draws.iter().any(|d| {
                    d.kind == rewo_gpu::mobs::EntityModelKind::ArmorStand
                        && d.outline == Some([1.0, 1.0, 1.0])
                })
            {
                rc.outline_frames += 1;
            }
            // M279 — r138: the invisible glowing stand, hidden and outlined.
            if state.world_renderer.outline_drawn()
                && draws.iter().any(|d| {
                    d.kind == rewo_gpu::mobs::EntityModelKind::ArmorStand
                        && d.body_hidden
                        && d.outline.is_some()
                })
            {
                rc.ghost_outline_frames += 1;
            }
            // M277 — r136: the ignited creeper swollen, and flashing.
            for d in draws
                .iter()
                .filter(|d| d.kind == rewo_gpu::mobs::EntityModelKind::Creeper)
            {
                if d.stretch[0] > 1.05 {
                    rc.creeper_swollen_frames += 1;
                    if d.white_overlay > 0.0 {
                        rc.creeper_flash_frames += 1;
                    }
                }
            }
            // M275 — r133: the staged head item's icon model, as the screen
            // and hotbar resolve it.
            let inv = &session.inventory;
            let icon = (inv.slot_count() == 46)
                .then(|| inv.menu_slot(35))
                .flatten()
                .zip(self.gui_items.as_ref())
                .and_then(|(stack, g)| {
                    let base = self.items.name(stack.item_id)?;
                    let m = skinned_icon_model(inv, stack, base, &g.skinned)?;
                    g.held.any(&m)?;
                    Some(m)
                });
            if icon.is_some_and(|m| {
                m.starts_with("minecraft:player_head#skin")
                    && self.baked.as_ref().is_some_and(|b| {
                        b.held_items.get(&m).is_some_and(|mm| {
                            mm.quads.iter().any(|q| {
                                b.held_items
                                    .textures
                                    .get(q.tex as usize)
                                    .is_some_and(super::render_check::is_synthetic_skin)
                            })
                        })
                    })
            }) {
                rc.skinned_icon_frames += 1;
            }
            if draws.iter().any(|d| {
                d.kind == rewo_gpu::mobs::EntityModelKind::ArmorStand
                    && d.stand_parts.pose[0] == [45.0, 0.0, 0.0]
                    && d.stand_parts.pose[3] == [-90.0, 0.0, 0.0]
            }) {
                rc.stand_pose_frames += 1;
            }
        }
        // r48 — the crowd census, taken on the list that is about to be
        // uploaded, so it cannot drift from what was drawn. Two counts because
        // they break independently: how many of the injected draws share one
        // `EntityModelKind` (the axis the M46 report is defined by), and how
        // many distinct non-capsule kinds are among them (the live
        // `etypes.name(type_id)` -> `kind_for_entity_name` chain, which every
        // serverless gate bypasses by handing `kind` in directly).
        //
        // **Restricted to `CROWD_IDS`.** `EntityDraw` carries no entity id, so
        // the join is the position: each of our three ids is resolved through
        // the very same `render_pos(alpha)` call `collect_entities` used one
        // screenful above, with the same `alpha`, so the two `[f32; 3]`s are
        // bit-identical and the match is exact rather than approximate. The
        // ids are inert on the wire (nothing else in the run can be at those
        // coordinates *and* be one of them), and the alternative — counting
        // every `Zombie` in the frame — is a claim about the world: another
        // fixture that spawns one satisfies it, so a partially decoded
        // injection would pass on somebody else's mob.
        #[cfg(feature = "gates")]
        if let Some(rc) = self.check.as_mut() {
            let mine: Vec<[f32; 3]> = CROWD_IDS
                .iter()
                .filter_map(|id| session.world.entities.get(*id))
                .map(|e| {
                    let q = e.render_pos(alpha);
                    [q[0] as f32, q[1] as f32, q[2] as f32]
                })
                .collect();
            let mut counts: Vec<(rewo_gpu::mobs::EntityModelKind, usize)> = Vec::new();
            let mut kinds: Vec<rewo_gpu::mobs::EntityModelKind> = Vec::new();
            for d in draws.iter().filter(|d| mine.contains(&d.pos)) {
                match counts.iter_mut().find(|(k, _)| *k == d.kind) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((d.kind, 1)),
                }
                if d.kind != rewo_gpu::mobs::EntityModelKind::Capsule && !kinds.contains(&d.kind) {
                    kinds.push(d.kind);
                }
            }
            let same = counts.iter().map(|(_, n)| *n).max().unwrap_or(0);
            rc.crowd_same_kind_max = rc.crowd_same_kind_max.max(same);
            rc.crowd_kinds_max = rc.crowd_kinds_max.max(kinds.len());
        }
        state.world_renderer.set_entities_and_block_entities(
            &draws,
            &be_draws,
            &frame_map_draws,
            &sign_draws,
            cr,
            cu,
            anim_time,
        );
        drop(draws);
        // M247 — r109: the staged chest (and, since M248, a bell and a
        // lectern's book) drew every quad of its baked model. Graded on what
        // the emitter EMITTED, not on the draw list: from M25b to M246 a chest
        // reached the list and drew nothing, because its 64x64 sheet never
        // paged into the entity atlas.
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            let mut complete = true;
            for (i, model) in super::render_check::RC_BLOCK_ENTITY_MODELS
                .iter()
                .enumerate()
            {
                let want = self
                    .baked
                    .as_ref()
                    .and_then(|b| b.held_items.any(model))
                    .map_or(0, |m| m.quads.len() as u32);
                let drew = be_draws
                    .iter()
                    .zip(state.world_renderer.block_entity_quads())
                    .filter(|(d, _)| d.model == *model)
                    .map(|(_, &q)| q)
                    .max()
                    .unwrap_or(0);
                complete &= want > 0 && drew == want;
                c.be_quads[i] = (c.be_quads[i].0.max(drew), want);
            }
            if complete {
                c.chest_frames += 1;
            }
            // M250 — r110: the arrow r99's released bow shot drew every quad
            // of the arrow model (it was a grey capsule until M250).
            let arrow_want = self
                .baked
                .as_ref()
                .and_then(|b| b.held_items.any("rewo:be/arrow"))
                .map_or(0, |m| m.quads.len() as u32);
            let arrow_drew = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .filter(|(d, _)| d.model == "rewo:be/arrow")
                .map(|(_, &q)| q)
                .max()
                .unwrap_or(0);
            if arrow_want > 0 && arrow_drew == arrow_want {
                c.arrow_frames += 1;
            }
            c.arrow_quads = (c.arrow_quads.0.max(arrow_drew), arrow_want);
            // M269 — r127: a primed TNT drawn swelling (its last ten fuse
            // ticks): any TNT draw scaled up.
            let tnt = self
                .blocks
                .default_state("minecraft:tnt")
                .map(rewo_data::block_entity_models::carried_block_model);
            // M277 — r135: and the TNT drawn both lit (white) and unlit.
            for d in be_draws.iter().filter(|d| Some(d.model) == tnt.as_deref()) {
                if d.white > 0.0 {
                    c.tnt_lit_frames += 1;
                } else {
                    c.tnt_unlit_frames += 1;
                }
            }
            if be_draws
                .iter()
                .any(|d| Some(d.model) == tnt.as_deref() && d.transform[0][0] > 1.01)
            {
                c.tnt_swell_frames += 1;
            }
            // M253 — r112: the staged falling gravel and primed TNT each drew
            // every quad of their block model.
            let complete: usize = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .filter(|(d, _)| d.model.starts_with("rewo:be/state/"))
                .filter(|(d, &q)| {
                    q > 0
                        && self
                            .baked
                            .as_ref()
                            .and_then(|b| b.held_items.any(d.model))
                            .is_some_and(|m| m.quads.len() as u32 == q)
                })
                .count();
            if complete >= 2 {
                c.carried_block_frames += 1;
            }
            // M254 — r113: the staged painting drew every quad of its
            // variant's model.
            let painting = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .filter(|(d, _)| d.model.starts_with("rewo:be/painting/"))
                .any(|(d, &q)| {
                    q > 0
                        && self
                            .baked
                            .as_ref()
                            .and_then(|b| b.held_items.any(d.model))
                            .is_some_and(|m| m.quads.len() as u32 == q)
                });
            if painting {
                c.painting_frames += 1;
            }
            // M256 — r115: the staged item display drew every quad of its
            // item's model, outside an item frame.
            let shard = "minecraft:amethyst_shard";
            let item_display = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .filter(|(d, _)| d.model == shard && !d.fixed_display)
                .any(|(_, &q)| {
                    q > 0
                        && self
                            .baked
                            .as_ref()
                            .and_then(|b| b.held_items.any(shard))
                            .is_some_and(|m| m.quads.len() as u32 == q)
                });
            if item_display {
                c.item_display_frames += 1;
            }
            // M267 — r125: the staged item display draws at its brightness
            // override's light, (0, 0), and not at the world's light where it
            // hangs (daylight, which the override was chosen to differ from).
            let overridden = super::sky::display_light(&session.world, 0.0, 0.0, 0.0, &lightmap, 0);
            if session.world.entities.iter().any(|(id, e)| {
                self.etypes.name(e.type_id) == Some("minecraft:item_display")
                    && session
                        .world
                        .entities
                        .display(id)
                        .is_some_and(|d| d.brightness == 0)
                    && {
                        let p = e.render_pos(alpha);
                        let world =
                            super::sky::entity_light(&session.world, p[0], p[1], p[2], &lightmap);
                        world != overridden
                            && be_draws.iter().any(|d| {
                                d.model == shard && !d.fixed_display && d.light == overridden
                            })
                    }
            }) {
                c.display_brightness_frames += 1;
            }
            // M257 — r116: the staged end crystal drew every quad of its
            // model (base included).
            let crystal = rewo_data::block_entity_models::END_CRYSTAL_MODELS[0].0;
            let drew_crystal = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .filter(|(d, _)| d.model == crystal)
                .any(|(_, &q)| {
                    q > 0
                        && self
                            .baked
                            .as_ref()
                            .and_then(|b| b.held_items.any(crystal))
                            .is_some_and(|m| m.quads.len() as u32 == q)
                });
            if drew_crystal {
                c.end_crystal_frames += 1;
            }
            // M266 — r124: the staged stone display part-way up its teleport,
            // between the height it was first seen at and one block above,
            // counted once per tick.
            let stone = self.blocks.default_state("minecraft:stone");
            let y = session.world.entities.iter().find_map(|(id, e)| {
                (self.etypes.name(e.type_id) == Some("minecraft:block_display")
                    && session.world.entities.display(id).and_then(|d| d.block) == stone)
                    .then(|| e.render_pos(0.0)[1])
            });
            let gliding = y.is_some_and(|y| {
                let y0 = *c.display_glide_y0.get_or_insert(y);
                y > y0 + 0.02 && y < y0 + 0.98
            });
            if gliding && session.ticks != c.display_glide_last {
                c.display_glide_last = session.ticks;
                c.display_glide_ticks += 1;
            }
            // M265 — r123: a block display drawn part-way through its scale-up.
            if session.world.entities.iter().any(|(id, e)| {
                self.etypes.name(e.type_id) == Some("minecraft:block_display")
                    && session.world.entities.display(id).is_some_and(|d| {
                        let s = d.render_transformation(alpha).scale[0];
                        s > 1.05 && s < 2.95
                    })
            }) {
                c.display_interp_frames += 1;
            }
            // M264 — r122: the staged bell, rung by the redstone block placed
            // beside it, drew its body swung away from rest.
            let rest =
                rewo_data::be_transform::part_at_rest(rewo_data::block_entity_models::BELL_PIVOT);
            if be_draws.iter().any(|d| {
                d.model == rewo_data::block_entity_models::BELL_MODEL.0
                    && d.part_transforms[usize::from(rewo_data::block_entity_models::BELL_PART)]
                        != rest
            }) {
                c.bell_shake_frames += 1;
            }
            // M258 — r117: the staged wither skull, llama spit and shulker
            // bullet each drew every quad of their model.
            use rewo_data::block_entity_models::{
                LLAMA_SPIT_MODEL, SHULKER_BULLET_MODEL, WITHER_SKULL_MODEL,
            };
            let all_small = [
                WITHER_SKULL_MODEL.0,
                LLAMA_SPIT_MODEL.0,
                SHULKER_BULLET_MODEL.0,
            ]
            .iter()
            .all(|name| {
                be_draws
                    .iter()
                    .zip(state.world_renderer.block_entity_quads())
                    .filter(|(d, _)| d.model == *name)
                    .any(|(_, &q)| {
                        q > 0
                            && self
                                .baked
                                .as_ref()
                                .and_then(|b| b.held_items.any(name))
                                .is_some_and(|m| m.quads.len() as u32 == q)
                    })
            });
            if all_small {
                c.small_projectile_frames += 1;
            }
            // M259 — r118: the staged evoker fangs, once their bite event
            // arrived, drew every quad of their model.
            let fangs = rewo_data::block_entity_models::EVOKER_FANGS_MODEL.0;
            let drew_fangs = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .filter(|(d, _)| d.model == fangs)
                .any(|(_, &q)| {
                    q > 0
                        && self
                            .baked
                            .as_ref()
                            .and_then(|b| b.held_items.any(fangs))
                            .is_some_and(|m| m.quads.len() as u32 == q)
                });
            if drew_fangs {
                c.evoker_fangs_frames += 1;
            }
            // M260 — r119: a block display of a state the bake never made (a
            // turned stair) drew every quad of the model built at run time,
            // and (since the M260 fix) every one of those quads samples the
            // oak planks layer's pixels: the first build drew cinnabar bricks
            // with the right quad count.
            let stair = self
                .blocks
                .state_with("minecraft:oak_stairs", &[("facing", "east")])
                .map(rewo_data::block_entity_models::carried_block_model);
            // Every quad of `m` samples the pixels of a layer whose name starts
            // with `prefix` (since the M260 fix; r120's hopper too). A layer
            // name keeps the model's spelling, with or without `minecraft:`.
            let sampled_from = |b: &rewo_data::assets::BakedAssets,
                                m: &rewo_data::held_items::HeldItemModel,
                                prefix: &str| {
                let layers: Vec<&Vec<u8>> = b
                    .layer_names
                    .iter()
                    .zip(&b.layers)
                    .filter(|(n, _)| {
                        n.strip_prefix("minecraft:")
                            .unwrap_or(n)
                            .starts_with(prefix)
                    })
                    .map(|(_, l)| l)
                    .collect();
                !layers.is_empty()
                    && m.quads.iter().all(|q| {
                        b.held_items
                            .textures
                            .get(q.tex as usize)
                            .is_some_and(|t| layers.contains(&&t.rgba))
                    })
            };
            let built = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .filter(|(d, _)| Some(d.model) == stair.as_deref())
                .any(|(d, &q)| {
                    q > 0
                        && self.baked.as_ref().is_some_and(|b| {
                            b.held_items.any(d.model).is_some_and(|m| {
                                m.quads.len() as u32 == q && sampled_from(b, m, "block/oak_planks#")
                            })
                        })
                });
            if built {
                c.block_display_frames += 1;
            }
            // M261 — r120: the staged hopper minecart's hopper (a model built
            // at run time) and the chest minecart's chest (off the block grid,
            // so not r109's placed chest) each drew every quad.
            let hopper = self
                .blocks
                .default_state("minecraft:hopper")
                .map(rewo_data::block_entity_models::carried_block_model);
            let drew_all = |d: &rewo_gpu::entities::BlockEntityDraw, q: u32| {
                q > 0
                    && self
                        .baked
                        .as_ref()
                        .and_then(|b| b.held_items.any(d.model))
                        .is_some_and(|m| m.quads.len() as u32 == q)
            };
            let drew_hopper = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .any(|(d, &q)| {
                    Some(d.model) == hopper.as_deref()
                        && drew_all(d, q)
                        && self.baked.as_ref().is_some_and(|b| {
                            b.held_items
                                .any(d.model)
                                .is_some_and(|m| sampled_from(b, m, "block/hopper"))
                        })
                });
            let drew_cart_chest = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .any(|(d, &q)| {
                    d.model.starts_with("rewo:be/chest")
                        && d.pos.iter().any(|v| v.fract() != 0.0)
                        && drew_all(d, q)
                });
            if drew_hopper && drew_cart_chest {
                c.minecart_contents_frames += 1;
            }
            // M276 — r134: the enchanting table's book drew every quad.
            for (d, &q) in be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
            {
                if d.model == "rewo:be/enchanting_book" && drew_all(d, q) {
                    c.enchant_book_frames += 1;
                    let t = d.transform[0][0];
                    c.enchant_book_turn = Some(
                        c.enchant_book_turn
                            .map_or((t, t), |(lo, hi)| (lo.min(t), hi.max(t))),
                    );
                }
            }
            // M272 — r130: the staged player head drew every quad of a
            // per-skin model, all sampling one texture whose texels are the
            // synthetic skin's `(4x, 4y, 0x80, 0xff)`.
            let drew_skin_head = be_draws
                .iter()
                .zip(state.world_renderer.block_entity_quads())
                .any(|(d, &q)| {
                    d.model.starts_with("rewo:be/player_head#")
                        && drew_all(d, q)
                        && self.baked.as_ref().is_some_and(|b| {
                            b.held_items.any(d.model).is_some_and(|m| {
                                m.quads.iter().all(|mq| {
                                    b.held_items
                                        .textures
                                        .get(mq.tex as usize)
                                        .is_some_and(super::render_check::is_synthetic_skin)
                                })
                            })
                        })
                });
            if drew_skin_head {
                c.skin_head_frames += 1;
            }
            c.carried_blocks_max = c.carried_blocks_max.max(complete);
        }

        let extent = state.renderer.swapchain.extent;
        let aspect = extent.width.max(1) as f32 / extent.height.max(1) as f32;
        // Targeted block for the selection outline.
        let hit = session.target_block(
            eye_f64(session),
            look_dir(session.player.yaw, session.player.pitch),
            REACH,
        );
        state.world_renderer.set_selection(hit.map(|h| h.block));
        let (mut leash_verts, quad_leashes) =
            collect_leashes(session, &self.etypes, alpha, &lightmap);
        // M241 — the fishing lines ride the leash pass (a black ribbon).
        let fov = session.fov_degrees(self.modules.render().fov_degrees, alpha);
        let lines = collect_fishing_lines(
            session,
            &self.etypes,
            &self.items,
            alpha,
            fov,
            (extent.width as f32, extent.height as f32),
        );
        // r108 — a line whose far end sits at the local hand (in front of
        // and close to the eye).
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            if lines.len() >= 12 {
                let eye = player_eye(session);
                let n = lines.len() - 12;
                let end = |k: usize| lines[n + k].pos;
                let (a, b) = (end(2), end(5));
                let mid = [
                    (a[0] + b[0]) / 2.0,
                    (a[1] + b[1]) / 2.0,
                    (a[2] + b[2]) / 2.0,
                ];
                let d = ((mid[0] - eye[0]).powi(2)
                    + (mid[1] - eye[1]).powi(2)
                    + (mid[2] - eye[2]).powi(2))
                .sqrt();
                c.fishing_line_frames += 1;
                c.fishing_line_hand = c.fishing_line_hand.max(d);
            }
        }
        leash_verts.extend(lines);
        state.world_renderer.set_leash(&leash_verts);
        // r72 — the injected happy ghast's horse took the quad branch.
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            if quad_leashes.contains(&(QUAD_LEASH_EIDS.0, QUAD_LEASH_EIDS.1)) {
                c.quad_leash_frames += 1;
            }
        }
        #[cfg(not(feature = "gates"))]
        let _ = quad_leashes;
        // r73 — the staged render distance, read back from what the server
        // sent: the loaded columns against the tracking set, and the fog.
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            let staged = crate::knobs::var("REWO_RC_VIEW_DISTANCE")
                .ok()
                .and_then(|v| v.parse::<i32>().ok());
            let sent = session.client_info.view_distance;
            let server = session.view_area.chunk_radius;
            let want = super::render_check::server_tracked_columns(
                (session.view_area.center_x, session.view_area.center_z),
                i32::from(sent),
            );
            let coords = session.world.column_coords();
            let fog = state.world_renderer.fog();
            let (start, end) = render_distance_fog(i32::from(sent));
            if staged == Some(i32::from(sent))
                && server > i32::from(sent)
                && coords.len() == want.len()
                && coords.iter().all(|k| want.contains(k))
                && fog == [start, end]
            {
                c.view_distance_frames += 1;
            }
            c.view_distance_seen = Some((staged, sent, server, coords.len(), want.len(), fog));
        }
        // Camera + lightmap were already set above from the same eye/state this
        // frame — don't duplicate them. Celestial still tracks the world clock.
        state.world_renderer.set_celestial({
            let mut cel = celestial_state_of(session.day_ticks);
            apply_weather_to_celestial(&mut cel, session);
            cel
        });
        state
            .world_renderer
            .set_end_flash(end_flash_render(session, lightmap_partial));
        FrameView {
            dt,
            alpha,
            eye,
            extent,
            aspect,
        }
    }

    /// GUI items and the open screen, the first-person hand, weather,
    /// border, particles and crumbling.
    fn gui_pass(&mut self, view: &FrameView, session: &mut PlaySession, state: &mut LiveState) {
        let (dt, alpha) = (view.dt, view.alpha);
        // M34/M35: the item icons, before the weather so the borrow of
        // `baked` is over by the time `apply_weather` takes its own. One pass
        // serves both the hotbar and the open screen — only the rectangles
        // differ.
        let ext = state.window.inner_size();
        let (sw, sh) = (ext.width as f32, ext.height as f32);
        let hovered = self
            .screen
            .inventory_open()
            .then(|| {
                self.screen
                    .hovered(session.shown_menu().layout(), sw, sh, book_visible(session))
            })
            .flatten();
        // M187 — every open is a fresh vanilla screen with nothing focused.
        if !self.screen.inventory_open() {
            self.screen.book_button_focus = None;
        }
        if let Some(baked) = self.baked.as_ref() {
            #[cfg(feature = "gates")]
            if let Some(c) = self.check.as_mut() {
                c.gui_item_frames += 1;
            }
            let items = self.items.clone();
            let gi = self
                .gui_items
                .get_or_insert_with(|| GuiItemState::new(baked));
            if self.screen.inventory_open() {
                #[cfg(feature = "gates")]
                let timed = self.started.elapsed().as_secs_f32()
                    < self.run_seconds.unwrap_or(super::RENDER_CHECK_SECONDS);
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    c.screen_frames += 1;
                    c.timed_screen_frames += u64::from(timed);
                }
                let beacon_override = session
                    .menus
                    .shown_open()
                    .filter(|m| m.layout.protocol_id == BEACON_MENU_PROTOCOL_ID)
                    .map(|m| beacon_live(&mut self.screen, m, &self.beacon_effects));
                // M93s — likewise, resolved before the borrow of `state`.
                let cut = session
                    .menus
                    .shown_open()
                    .filter(|m| {
                        m.layout.protocol_id == rewo_ui::menu_screen::STONECUTTER_MENU_PROTOCOL_ID
                    })
                    .map(|m| cut_view(&mut self.screen, m, &items));
                // M189 — and the loom's scroll, for the same reason.
                let loom_scroll = session
                    .menus
                    .shown_open()
                    .and_then(|m| {
                        loom_view_scrolled(&mut self.screen, m, &items, &session.stack_details)
                    })
                    .map_or((0.0, 0), |v| (v.scroll_offs, v.start_row));
                // M93t — likewise the anvil's field, seeded here so the render
                // and the key handler see one box.
                // M98 — the book's own selection, from the screen state.
                let book_state = self.screen.book;
                // M99 — and its search text, lowercased once here.
                let book_query =
                    rewo_ui::recipe_search::normalize(&self.screen.book_search.value());
                let anvil_field = session
                    .menus
                    .shown_open()
                    .filter(|m| m.layout.protocol_id == ANVIL_MENU_PROTOCOL_ID)
                    .map(|m| anvil_local(&mut self.screen, m, &items).field.clone());
                // M93u — the merchant's trade list, from the session's decoded
                // offers plus the screen's own scroll.
                let merchant = session
                    .menus
                    .shown_open()
                    .filter(|m| {
                        m.layout.protocol_id == rewo_ui::merchant_screen::MERCHANT_MENU_PROTOCOL_ID
                    })
                    .zip(session.merchant.as_ref())
                    .map(|(m, offers)| {
                        // The clamp's ceiling is the item's own max stack size,
                        // through the SAME production resolver every other
                        // consumer uses (M93b's rule).
                        let props =
                            |id: i32| item_props(&items, id, None).map_or(64, |p| p.max_stack);
                        merchant_view(&mut self.screen, m, offers, &props)
                    });
                let (labels, velvet) = apply_screen(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    &items,
                    gi,
                    baked,
                    self.preview_skin.as_mut(),
                    self.glyphs.as_mut(),
                    rewo_gpu::tooltip::TooltipFlag::of(self.advanced_tooltips),
                    self.screen.mouse,
                    (sw, sh),
                    self.beacon_effects,
                    // M93m — the SCREEN's choice, so a click lights the
                    // button it pressed instead of the render continuing to
                    // paint the server's last word.
                    beacon_override,
                    cut.as_ref(),
                    anvil_field.as_ref(),
                    merchant.as_ref(),
                    book_state,
                    &book_query,
                    &self.screen.book_search,
                    self.started.elapsed().as_millis() as u64,
                    self.screen.book_overlay.as_ref(),
                    self.screen.book_button_focus == Some(session.shown_container_id()),
                    loom_scroll,
                );
                // r102 — the anvil's cost line as drawn this frame.
                #[cfg(feature = "gates")]
                {
                    self.last_anvil_label = labels
                        .iter()
                        .find(|l| {
                            l.text.starts_with("Enchantment Cost") || l.text == "Too Expensive!"
                        })
                        .map(|l| (l.text.clone(), l.color_linear));
                }
                // M105 — the page counter is a LABEL, so it is counted here
                // rather than among the book's quads. Matched on the
                // model's own geometry (the counter is the only text the book
                // draws on that row) rather than on its content, which is a
                // translation and would tie the gate to a language.
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    let (_, bt, sc) = rewo_gpu::container::recipe_book_origin(sw, sh);
                    let row = bt + rewo_ui::recipe_book_screen::PAGE_LABEL_Y as f32 * sc;
                    if labels.iter().any(|l| (l.y - row).abs() < 0.5) {
                        c.book_page_label_frames += 1;
                    }
                }
                self.screen_labels = labels;
                // M94 — OUTSIDE the `container_panel_height` guard below: the
                // player's own inventory has no container panel and is the
                // commonest screen with a book, so counting inside that guard
                // measures zero forever. r23's first two red runs were this
                // and the field placement it forced.
                // M104 — split by whether an overlay was up, because the claim
                // one frame over is a DIFFERENCE and not a threshold.
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    let overlay_up = self.screen.book_overlay.is_some();
                    if session.shown_menu().layout().name == "crafting"
                        && state
                            .world_renderer
                            .container_widgets()
                            .iter()
                            .any(|(sp, b)| {
                                (*sp == rewo_data::assets::BOOK_BUTTON
                                    || *sp == rewo_data::assets::BOOK_BUTTON + 1)
                                    && (b.dx, b.dy) == (5.0, 34.0)
                            })
                    {
                        c.book_button_frames += 1;
                    }
                    let q = state.world_renderer.container_panel_book_quads();
                    if overlay_up {
                        c.book_overlay_quads_max = c.book_overlay_quads_max.max(q);
                    } else {
                        c.book_quads_max = c.book_quads_max.max(q);
                    }
                }
                // M88 — read the panel back OUT of the renderer, after the
                // draw path set it. Asking the open menu's layout instead
                // would answer 168 for a chest whether or not the panel
                // builder returned one.
                #[cfg(feature = "gates")]
                if let Some(h) = state.world_renderer.container_panel_height() {
                    let forced = self.screen_forced_open;
                    let chest_injected = self.container_injected;
                    if let Some(c) = self.check.as_mut() {
                        c.container_frames += 1;
                        // FIRST sighting, not the last: M92 injects a
                        // second container late (a brewing stand, 166 tall
                        // like the player's), and r20's question is about the
                        // chest that proved the panel builder runs. M224:
                        // and only once the chest is in — r76's villager
                        // (a 166-tall merchant screen) can open before 0.4.
                        if chest_injected {
                            c.container_panel_h.get_or_insert(h);
                        }
                        c.container_overlays_max = c
                            .container_overlays_max
                            .max(state.world_renderer.container_panel_overlays());
                        if !forced {
                            c.container_self_opened_frames += 1;
                        }
                    }
                    // M220 — r84/r85: the panel the frame drew for a mount
                    // menu, read back out of the renderer the same way.
                    // `session` is this frame's own binding: `self.session`
                    // is empty while the frame holds it.
                    let mount = session
                        .menus
                        .shown_open()
                        .and_then(|m| m.mount.map(|_| m.open_serial));
                    if let (Some(serial), Some(c)) = (mount, self.check.as_mut()) {
                        c.mount_panel_frames.push((
                            serial,
                            h,
                            state.world_renderer.container_panel_overlays(),
                        ));
                    }
                    // r86 — a container's frame carries no player preview.
                    if let Some(c) = self.check.as_mut() {
                        if state.world_renderer.preview_active() {
                            c.container_preview_frames += 1;
                        }
                    }
                } else if state.world_renderer.preview_active() {
                    // ...and the player's own inventory's does (r86's control).
                    if let Some(c) = self.check.as_mut() {
                        c.inventory_preview_frames += 1;
                    }
                }
                // Any glyph laid out above may be new to the atlas. Sync
                // BEFORE the runs are drawn and outside the rendering scope --
                // it records a transfer, and a run referencing a rect that has
                // not reached the GPU samples whatever was there before.
                if let Some(cache) = self.glyphs.as_mut() {
                    if let Err(e) = state
                        .world_renderer
                        .sync_velvet_atlas(&mut state.gpu, cache)
                    {
                        log::warn!("velvet atlas sync: {e}");
                    }
                }
                state.world_renderer.set_velvet_runs(velvet);
            } else {
                self.screen_labels.clear();
                state.world_renderer.set_velvet_runs(Vec::new());
                state.world_renderer.set_preview(None);
                apply_hotbar_icons(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    &items,
                    gi,
                    (sw, sh),
                );
            }
        }
        if !self.screen.inventory_open() {
            state.world_renderer.set_container(false, None);
        }
        let _ = hovered;
        // M38: the first-person hand. Suppressed while the inventory screen is
        // open — the screen owns the view, and vanilla does the same.
        if let Some(baked) = self.baked.as_ref() {
            #[cfg(feature = "gates")]
            if let Some(c) = self.check.as_mut() {
                c.hand_frames += 1;
            }
            let items = self.items.clone();
            let h = self.hand.get_or_insert_with(|| HandState::new(baked));
            h.tick(session, &items);
            if self.screen.inventory_open() {
                let _ = state
                    .world_renderer
                    .set_hand(&mut state.gpu, &[], [[0.0; 4]; 4]);
            } else {
                let drawn = apply_hand(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    Some(&self.lang),
                    &items,
                    h,
                    alpha,
                    sw / sh,
                );
                // r69 — the injected off-hand map, with its data and its
                // one decoration, reached the hand pass.
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    if drawn == Some((HELD_MAP_WITNESS_ID, true, 1)) {
                        c.held_map_frames += 1;
                    }
                }
                #[cfg(not(feature = "gates"))]
                let _ = drawn;
            }
        }
        // M33: the cloud deck and this frame's precipitation. The assets are
        // built lazily because `baked` arrives with the session.
        if let Some(baked) = self.baked.as_ref() {
            #[cfg(feature = "gates")]
            if let Some(c) = self.check.as_mut() {
                c.weather_frames += 1;
            }
            let w = self
                .weather
                .get_or_insert_with(|| WeatherAssets::new(baked));
            apply_weather(
                &mut state.world_renderer,
                &mut state.gpu,
                session,
                w,
                alpha,
                // 20 ticks per second — .
                Some(dt * 20.0),
            );
            apply_border(&mut state.world_renderer, &mut state.gpu, session, alpha);
            apply_render_distance_fog(&mut state.world_renderer, session);
            if self.particles.is_none() {
                self.particles = ParticleAssets::new(baked);
            }
            if let Some(p) = self.particles.as_mut() {
                let eye = player_eye(session);
                let view =
                    eye_view(eye, session.player.yaw, session.player.pitch).to_cols_array_2d();
                let events = std::mem::take(&mut session.particle_events);
                apply_particles(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    events,
                    p,
                    baked,
                    alpha,
                    view,
                );
            }
            apply_crumbling(
                &mut state.world_renderer,
                &mut state.gpu,
                session,
                baked,
                player_eye(session),
            );
        }
    }

    /// The HUD: survival bars, chat, sidebar, tab list, screens' chrome and
    /// text.
    fn hud_pass(&mut self, view: &FrameView, session: &mut PlaySession, state: &mut LiveState) {
        let (alpha, extent) = (view.alpha, view.extent);
        let contextual = contextual_info(session);
        let survival_inputs = resolve_survival_inputs(
            session,
            &self.etypes,
            self.started.elapsed().as_millis() as u64,
            contextual,
        );
        let survival = rewo_gpu::survival_hud::layout_for_screen(
            &survival_inputs,
            extent.width as f32,
            extent.height as f32,
        );
        // M188 — `Hud.playAirBubblePoppedSound`, from the same inputs the
        // bars were laid out from: `LocalPlayer.playSound` is
        // `level.playLocalSound(getX(), getY(), getZ(), sound, PLAYERS, ...)`.
        if let Some(pop) = self.bubble_popper.frame(&survival_inputs) {
            self.sounds.play_now(rewo_net::sounds::SoundEvent::Local(
                rewo_net::sounds::LocalSound {
                    name: rewo_gpu::survival_hud::BubblePopper::EVENT.to_string(),
                    source: rewo_net::sounds::SoundSource::Players,
                    x: session.player.x,
                    y: session.player.y,
                    z: session.player.z,
                    volume: pop.volume,
                    pitch: pop.pitch,
                },
            ));
        }
        // M227 — r58's effect count is the timed run's: riding r90's
        // nautilus in the tail gives the bot Breath of the Nautilus, a
        // second icon that r58's staged Speed is not about.
        #[cfg(feature = "gates")]
        let timed_hud = self.in_timed_run();
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            use rewo_gpu::hud::HudIcon;
            use rewo_gpu::survival_hud::ArmorSprite;
            let armor = survival
                .iter()
                .filter(|b| matches!(b.icon, HudIcon::Armor(_)))
                .count();
            let full = survival
                .iter()
                .filter(|b| b.icon == HudIcon::Armor(ArmorSprite::Full))
                .count();
            c.armor_icons_max = c.armor_icons_max.max(armor);
            c.armor_full_max = c.armor_full_max.max(full);
            let effects: Vec<&rewo_gpu::hud::HudBlit> = survival
                .iter()
                .filter(|b| matches!(b.icon, HudIcon::Effect(_)))
                .collect();
            if timed_hud {
                c.effect_icons_max = c.effect_icons_max.max(effects.len());
            }
            // The beneficial row is y = 1 + 3; the first slot is
            // `guiWidth - 25 + 3`, read off the layout's own GUI size.
            let gui_w = (extent.width as f32
                / rewo_gpu::hud::gui_scale(extent.width as f32, extent.height as f32))
                as i32;
            c.effect_first_beneficial |= effects
                .iter()
                .any(|b| b.x as i32 == gui_w - 25 + 3 && b.y as i32 == 4);
            let jump = survival
                .iter()
                .filter(|b| matches!(b.icon, HudIcon::JumpBar(_)))
                .count();
            c.jump_bar_blits_max = c.jump_bar_blits_max.max(jump);
            c.leash_verts_max = c
                .leash_verts_max
                .max(state.world_renderer.leash_vert_count());
            if self.options_view == Some(rewo_ui::options_screen::OptionsPage::Sound) {
                if let Some(sc) = self.screen.screens.current() {
                    c.options_slider_sprites_max = c
                        .options_slider_sprites_max
                        .max(slider_sprites(sc, None, None).len());
                }
            }
            if let Some(book) = self.book.as_ref() {
                if self
                    .screen
                    .screens
                    .current()
                    .is_some_and(|s| s.kind == rewo_ui::screen::ScreenKind::BookView)
                {
                    c.book_frames += 1;
                    c.book_pages_seen = c.book_pages_seen.max(book.page_count());
                }
            }
            c.jump_scale_max = c.jump_scale_max.max(session.jump_riding_scale());
            c.riding_jumps_sent = session.riding_jumps_sent();
        }
        state.world_renderer.set_hud(
            session.inventory.selected(),
            resolve_hud_gauges(
                &session.hud,
                &session.inventory,
                &self.items,
                has_experience(session)
                    && contextual == rewo_gpu::locator_bar::ContextualInfo::Experience,
                alpha,
            ),
            survival,
        );
        let px = gui_px(extent.width, extent.height);
        // Drain the frame's chat events into the store *before* building the
        // text, or a message that arrived this frame is a frame late. The
        // store needs the font and the GUI clock, which is why this cannot
        // live where the packets are decoded.
        apply_chat(session, state.world_renderer.font_advance().copied());
        // M117 — both command-line overlays, resolved once before the fills
        // stage so the box and the highlighting read the same cached parse.
        // The names come from the session that is already borrowed here, so
        // they are read directly rather than through `tab_words`, which would
        // need `&self` while the session holds `&mut self`.
        let chat_words = session
            .suggestions
            .tab_suggestions(session.world.entities.all_names());
        let chat_runs = chat_runs(
            &mut self.chat_parse,
            self.chat_screen.as_ref(),
            session,
            rewo_net::dispatcher::CommandCtx {
                names: &chat_words,
                blocks: Some(&self.blocks),
                items: Some(&self.items),
            },
        );
        let (usage_fills, usage_text) = Self::usage_box_parts(
            self.chat_screen.as_ref(),
            session,
            &self.chat_parse,
            state.world_renderer.font_advance().copied(),
            {
                let s = gui_px(extent.width, extent.height);
                (
                    (extent.width as f32 / s) as i32,
                    (extent.height as f32 / s) as i32,
                )
            },
            px,
        );
        // A `command_suggestions` reply that matched the outstanding request
        // (M114). Drained here rather than at decode time for the same reason
        // the chat events are: opening the popup needs the font to measure its
        // widest entry, which `rewo-net` does not have.
        if let Some(reply) = session.suggestion_reply.take() {
            let advance = state.world_renderer.font_advance().copied();
            let width_of = move |t: &str| match &advance {
                Some(a) => rewo_gpu::text::width(t, a),
                None => 0,
            };
            let px = gui_px(extent.width, extent.height);
            let (gui_w, gui_h) = (
                (extent.width as f32 / px) as i32,
                (extent.height as f32 / px) as i32,
            );
            let (ix, _iy, iw, _ih) = rewo_ui::chat_screen::input_rect(gui_w, gui_h);
            let env = rewo_ui::chat_screen::SuggestionEnv {
                metrics: rewo_ui::command_suggestions::InputMetrics {
                    x: ix,
                    inner_width: iw,
                    screen_height: gui_h,
                },
                width: &width_of,
                tab_words: &[],
                auto_suggestions: true,
            };
            if let Some(s) = self.chat_screen.as_mut() {
                s.accept_suggestions(reply, &env);
            }
        }
        // `ChatComponent.isChatFocused()` — `gui.screen() instanceof
        // ChatScreen`. It changes the box height, suppresses the fade, and is
        // what `scrollChat` clamps against, so the two derivations below and
        // the key handler must all read the same answer.
        let chat_focused = self.chat_screen.is_some();
        // M109 — the fills, from the same `visible_lines` the text comes from
        // so a row's backdrop and its glyphs cannot disagree about which rows
        // exist. Set every frame, including when it is empty: a stale backdrop
        // under nothing is a black bar hanging over the world.
        // M132 — the scoreboard sidebar. `Hud.extractRenderState` reaches
        // `extractScoreboardSidebar` in an earlier stratum than
        // `extractChat`, so its two bands go on the list BEFORE the chat's
        // rows and its text before the chat's glyphs. The two overlap only on
        // a narrow window, which is exactly when a wrong order would show.
        let sidebar = resolve_sidebar(
            &session.scoreboard,
            &self.username,
            self.hud_hidden,
            &self.lang,
            state.world_renderer.font_advance().copied(),
        );
        let sidebar_layout = sidebar.as_ref().map(|s| {
            rewo_net::sidebar::layout(
                s,
                (extent.width as f32 / px) as i32,
                (extent.height as f32 / px) as i32,
            )
        });
        let mut backdrops = match sidebar_layout.as_ref() {
            Some(l) => sidebar_fills(l),
            None => Vec::new(),
        };
        backdrops.extend(hud_fills(
            &session.chat,
            session.ticks as i32,
            px,
            extent.height as f32,
            &rewo_ui::chat::ChatOptions::default(),
            chat_focused,
        ));
        if chat_focused {
            // The input bar goes on the same list AFTER the rows, so it sits
            // over them — one list, and its order is the order on screen.
            let gw = (extent.width as f32 / px) as i32;
            let gh = (extent.height as f32 / px) as i32;
            backdrops.push(chat_input_backdrop(gw, gh));
            // M111 — the scrollbar, which only exists while the screen is up
            // (`isForeground`). It reads the same `visible_lines` count the
            // rows do, because vanilla passes `forEachLine`'s own return here.
            let bar = chat_scrollbar(
                &session.chat,
                session.ticks as i32,
                px,
                extent.height as f32,
                &rewo_ui::chat::ChatOptions::default(),
            );
            if !bar.is_empty() {
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    c.chat_scrollbar_frames += 1;
                }
            }
            backdrops.extend(bar);
            // M115 — the suggestion popup, last on the list so it sits over
            // the input bar and the rows, which is where
            // `ChatScreen.extractRenderState` hands off to it.
            if let Some(cs) = self.chat_screen.as_ref() {
                if let Some(list) = cs.suggestions.list() {
                    let fills = suggestion_popup_fills(list, cs.suggestions.config());
                    if !fills.is_empty() {
                        #[cfg(feature = "gates")]
                        if let Some(c) = self.check.as_mut() {
                            c.suggestion_popup_frames += 1;
                        }
                    }
                    backdrops.extend(fills);
                } else {
                    // M117 — `extractRenderState` is
                    // `if (!extractSuggestions(..)) extractUsage(..)`, so the
                    // box exists only when the popup does not. Drawing both
                    // stacks two panels over one field.
                    if !usage_fills.is_empty() {
                        #[cfg(feature = "gates")]
                        if let Some(c) = self.check.as_mut() {
                            c.usage_box_frames += 1;
                        }
                    }
                    backdrops.extend(usage_fills.clone());
                }
            }
            #[cfg(feature = "gates")]
            if let Some(c) = self.check.as_mut() {
                c.chat_screen_frames += 1;
            }
        }
        // M151 — the tab list, LAST on the fill list because
        // `Hud.extractRenderState` calls `extractTabList` (line 237) after
        // `extractChat` (236) and `extractTabList` opens its own
        // `graphics.nextStratum()`. It really does cover the chat box: the
        // panel is centred at the top and the chat sits at the bottom left, so
        // the two only meet on a short window — which is exactly when the
        // order shows.
        let tab_list = resolve_tab_list(
            session,
            self.keys.tab_list,
            self.hud_hidden,
            (extent.width as f32 / px) as i32,
            &self.lang,
            state.world_renderer.font_advance().copied(),
        );
        let tab_layout = tab_list
            .as_ref()
            .map(|v| rewo_gpu::tab_list::layout(&v.input, &v.entries));
        let mut tab_icons: Vec<rewo_gpu::hud::HudBlit> = Vec::new();
        if let (Some(v), Some(l)) = (tab_list.as_ref(), tab_layout.as_ref()) {
            backdrops.extend(crate::tab_list_view::fills(l));
            tab_icons = crate::tab_list_view::icons(v, l);
            #[cfg(feature = "gates")]
            if let Some(c) = self.check.as_mut() {
                c.tab_list_frames += 1;
                c.tab_list_rows_max = c.tab_list_rows_max.max(l.entries.len());
                // Recorded BEFORE the hearts are appended (M155). r47 asserts
                // this is exactly 3, and it is a claim about the PING icons —
                // folding hearts into the same count would move a number a
                // gate asserts without changing what it means to assert.
                c.tab_list_icons_max = c.tab_list_icons_max.max(tab_icons.len());
            }
            // M155 — the faces, then the health column, both appended after
            // the ping icons so each draws over its own row background.
            // Separate lists from `icons` because one needs the skin registry
            // and the other a mutable blink clock.
            //
            // `loaded_of` is `level.getPlayerByUUID(id) != null` — the flip
            // needs the player actually in the world, not merely listed, so a
            // Dinnerbone out of render distance is the right way up.
            {
                let reg = &self.skins.registry;
                let loaded: std::collections::HashSet<u128> =
                    session.world.entities.iter().map(|(_, e)| e.uuid).collect();
                tab_icons.extend(crate::tab_list_view::faces(
                    v,
                    l,
                    &|u| reg.get(&u).and_then(|p| p.face),
                    &|u| session.show_hat(u),
                    &|u| loaded.contains(&u),
                ));
            }
            // M155 — the health column, appended after the ping icons so a
            // heart draws over its own row background. `hearts` is separate
            // from `icons` because it needs the mutable blink clock.
            tab_icons.extend(crate::tab_list_view::hearts(
                v,
                l,
                &mut self.tab_health,
                session.ticks as i64,
            ));
        } else {
            // `PlayerTabOverlay.reset()` — the list is not being drawn, so
            // every blink is forgotten rather than resumed where it left off.
            self.tab_health.clear();
        }
        state.world_renderer.set_hud_fills(backdrops);
        // Set every frame, including empty: the list exists only while its key
        // is held, and a stale icon list would leave ping bars floating over
        // the world the moment it is let go.
        state.world_renderer.set_hud_icons(tab_icons);
        // The input bar goes on the same list, after the rows, so it sits over
        // them the way `ChatScreen.extractRenderState` draws its fill before
        // handing off to the chat component — one list, and the order in it is
        // the order on screen.

        let fps = (!self.cpu.is_empty()).then(|| 1000.0 / self.cpu.average().max(0.001));
        let (built, chat_rows, built_chat_range) = build_text(
            session,
            px,
            extent.height as f32,
            fps,
            self.debug,
            chat_focused,
            state.world_renderer.font_advance().copied(),
        );
        // M132 — the sidebar's lines lead the list, for the stratum reason
        // above. `chat_range` indexes into `text`, so it shifts with them
        // rather than the sidebar being appended into the wrong stratum to
        // keep an index valid.
        let mut text = match (sidebar.as_ref(), sidebar_layout.as_ref()) {
            (Some(s), Some(l)) => {
                sidebar_text(s, l, px, state.world_renderer.font_advance().copied())
            }
            _ => Vec::new(),
        };
        let sidebar_lines = text.len();
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            if sidebar_lines > 0 {
                c.sidebar_frames += 1;
                c.sidebar_text_max = c.sidebar_text_max.max(sidebar_lines);
            }
        }
        #[cfg_attr(not(feature = "gates"), allow(unused_variables))]
        let chat_range =
            (built_chat_range.start + sidebar_lines)..(built_chat_range.end + sidebar_lines);
        text.extend(built);
        if let Some(cs) = self.chat_screen.as_ref() {
            let (gw, gh) = (
                (extent.width as f32 / px) as i32,
                (extent.height as f32 / px) as i32,
            );
            let advance = state.world_renderer.font_advance().copied();
            let width_of = move |s: &str| match &advance {
                Some(a) => rewo_gpu::text::width(s, a),
                None => 0,
            };
            if chat_runs.is_some() {
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    c.highlighted_command_frames += 1;
                }
            }
            text.extend(chat_input_lines(
                cs,
                px,
                gw,
                gh,
                self.started.elapsed().as_millis() as u64,
                chat_runs.as_deref(),
                &width_of,
            ));
            if let Some(list) = cs.suggestions.list() {
                text.extend(suggestion_popup_text(list, cs.suggestions.config(), px));
            }
        }
        if self
            .chat_screen
            .as_ref()
            .is_some_and(|cs| cs.suggestions.list().is_none())
        {
            // M134 — counted HERE rather than in `usage_box_parts`, because
            // this `if` is the mutual exclusion with the popup: a message the
            // box computed and the popup covered never reached the screen,
            // and a counter upstream of the exclusion would say it did.
            if usage_text
                .iter()
                .any(|l| l.text.starts_with("Unknown command") && l.color_linear == [1.0, 1.0, 1.0])
            {
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    c.parse_error_frames += 1;
                }
            }
            text.extend(usage_text);
        }
        if !chat_rows.is_empty() {
            #[cfg(feature = "gates")]
            if let Some(c) = self.check.as_mut() {
                c.chat_line_frames += 1;
            }
        }
        // M125 — read off the DRAWN lines rather than the chat store, so a
        // resolution that happened and then failed to reach the frame is not
        // counted.
        //
        // **M126b split the two scans, and the reason is asymmetric.** A chat
        // row is now one text line per SPAN, and a resolved template is
        // several spans ("Gave ", "1", " ", "[", "Diamond Sword", "]", …) — so
        // scanning `text` for the whole sentence finds nothing and the witness
        // would read zero with the feature working. `chat_rows` is that same
        // drawn row re-concatenated by `chat_lines`, from the same spans it
        // emitted, so it keeps M125's "off the drawn lines" property.
        //
        // The raw-key scan does NOT need it: an unresolved translatable falls
        // back to its key as a SINGLE span, by construction, so it still lands
        // in one `text` line — and scanning the wider list keeps the check that
        // no key leaked into some other surface.
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            // M126d — over the chat lines this frame actually drew, and
            // **within one row**. Across the whole box is a much weaker claim
            // that a client with no span pipeline satisfies for free: the
            // section-sign message's first span is red while the filler rows
            // are white, so a truncate-to-one-span mutation left the
            // across-the-box version green. The battery caught that; this is
            // the corrected witness.
            //
            // `color` is an `[f32; 3]`, so distinctness is by bits — two spans
            // that resolved to the same colour ARE the same colour, and a
            // tolerance would only blur the claim.
            let drawn = &text[chat_range.clone()];
            let mut at = 0usize;
            let mut multi = false;
            let mut decorated = false;
            let mut styled_decoration = false;
            for (_, n) in &chat_rows {
                let row = &drawn[at.min(drawn.len())..(at + n).min(drawn.len())];
                let mut colors: Vec<[u32; 3]> = row
                    .iter()
                    .map(|l| {
                        [
                            l.color_linear[0].to_bits(),
                            l.color_linear[1].to_bits(),
                            l.color_linear[2].to_bits(),
                        ]
                    })
                    .collect();
                colors.sort_unstable();
                colors.dedup();
                multi |= colors.len() > 1;
                // M127 — r40/r41, on the same per-row slice, because both are
                // claims about ONE row: "the box contains these characters
                // somewhere" is satisfied by a client that draws the name and
                // the message as two unrelated lines.
                let row_text: String = row.iter().map(|l| l.text.as_str()).collect();
                if row_text.contains("<RewoDecoWitness> decorated") {
                    decorated = true;
                }
                if row_text.contains("RewoStyleWitness whispers to you: whispered")
                    && !row.is_empty()
                    && row.iter().all(|l| {
                        l.color_linear
                            .iter()
                            .all(|c| (c - GRAY_LINEAR).abs() < COLOR_EPS)
                            && l.style.italic
                    })
                {
                    styled_decoration = true;
                }
                at += n;
            }
            if multi {
                c.styled_chat_frames += 1;
            }
            if decorated {
                c.decorated_chat_frames += 1;
            }
            if styled_decoration {
                c.styled_decoration_frames += 1;
            }
            if drawn
                .iter()
                .any(|l| l.style != rewo_gpu::text::TextStyle::PLAIN)
            {
                c.flagged_chat_frames += 1;
            }
            if chat_rows
                .iter()
                .any(|(r, _)| r.contains("Gave 1 [Diamond Sword]"))
            {
                c.translated_chat_frames += 1;
            }
            // M128 — the drawn position of the clickable witness line. Taken
            // from `OwnedTextLine`, i.e. from what the renderer is about to
            // put on screen, so the click below is a cross-check of the two
            // derivations rather than a restatement of one.
            if let Some(l) = drawn.iter().find(|l| l.text.contains(CLICK_WITNESS_TEXT)) {
                c.chat_link_at = Some((l.x, l.y));
            }
            // The three keys of that one message, one per nesting level. Named
            // exactly rather than detected generally: a "looks like a key"
            // test would fire on the F3 block's coordinates and on any player
            // whose name has a dot in it.
            const RAW_KEYS: [&str; 3] = [
                "commands.give.success",
                "chat.square_brackets",
                "item.minecraft.diamond_sword",
            ];
            if text
                .iter()
                .any(|l| RAW_KEYS.iter().any(|k| l.text.contains(k)))
            {
                c.unresolved_key_frames += 1;
            }
        }
        // M66: the held-item name over the hotbar. Needs the font's advances
        // to centre itself, so it is built here rather than in `build_text`.
        if let Some(advance) = state.world_renderer.font_advance() {
            text.extend(selected_item_name_line(
                session,
                &self.items,
                &self.tool_highlight,
                advance,
                px,
                (extent.width as f32, extent.height as f32),
            ));
            // M79: the XP level number, then the title / subtitle / action
            // bar. The titles go last so they sit over everything the HUD
            // draws, which is where `nextStratum()` puts them in vanilla.
            text.extend(experience_level_lines(
                &session.hud.experience,
                has_experience(session),
                self.baked.as_ref().map(|b| &b.lang),
                advance,
                px,
                (extent.width as f32, extent.height as f32),
            ));
            text.extend(title_lines(
                &session.hud.titles,
                advance,
                px,
                (extent.width as f32, extent.height as f32),
                // `deltaTracker.getGameTimeDeltaPartialTick(false)` — this
                // frame's fraction of the way into the current tick, the same
                // `alpha` the entity lerps use.
                alpha,
                // M125 — the same handover `experience_level_lines` above
                // takes, so `/title {"translate":...}` resolves.
                self.baked.as_ref().map(|b| &b.lang),
            ));
        }
        // The stack counts are text like any other line, drawn after the icons
        // because the text pass runs last.
        text.append(&mut self.screen_labels);
        // M82: the death screen — its chrome into the screen pass, its four
        // text runs onto the end of this frame's lines. Last, so the title,
        // the cause and the button labels sit over the HUD, which is where
        // `extractRenderStateWithTooltipAndSubtitles`'s stratum order puts
        // everything a screen draws.
        {
            let mut chrome = rewo_gpu::screen::ScreenDraw::default();
            // M178 — the advancements screen's icons, when its arm runs.
            let mut adv_icons: Vec<crate::advancements_view::IconDraw> = Vec::new();
            // Every hover test wants the cursor in GUI space — see
            // `LiveApp::mouse_gui` for the M82 bug this fixes, and for why all
            // five screens share the one conversion.
            let mouse_gui = (
                self.screen.mouse.0 / px as f64,
                self.screen.mouse.1 / px as f64,
            );
            // M84: the statistics screen, on the same seam. Only one screen is
            // ever up, so the three arms are exclusive by construction rather
            // than by an ordering rule.
            if let (Some(view), Some(screen)) = (self.stats.as_ref(), self.screen.screens.current())
            {
                let advance = state.world_renderer.font_advance();
                chrome = crate::stats_view::chrome(view, screen, Some(mouse_gui), advance);
                if let Some(advance) = advance {
                    text.extend(crate::stats_view::lines(view, screen, advance, px));
                }
            } else if self.advancements.is_some()
                && self
                    .screen
                    .screens
                    .current()
                    .is_some_and(|s| s.kind == rewo_ui::screen::ScreenKind::Advancements)
            {
                // M178: the advancements window. Generic chrome carries the
                // transparent gradient + the Done button; our scissored
                // batches draw between the pass's head and its sprites
                // (vanilla: extractInside, then extractWindow), and our plain
                // sprites join `chrome.sprites` ahead of the buttons.
                let gw = (extent.width as f32 / px) as i32;
                let gh = (extent.height as f32 / px) as i32;
                let (win_x, win_y) = rewo_ui::advancements_screen::window_origin(gw, gh);
                // tick() runs here, contents-relative: the model owns the
                // hover clock and this is its only per-frame driver.
                if let Some(v) = self.advancements.as_mut() {
                    v.ensure_centered();
                    v.tick(Some((
                        mouse_gui.0 as i32 - win_x - rewo_ui::advancements_screen::INSIDE_X,
                        mouse_gui.1 as i32 - win_y - rewo_ui::advancements_screen::INSIDE_Y,
                    )));
                }
                if let Some(view) = self.advancements.as_ref() {
                    let mut adv = crate::advancements_view::chrome(view, gw, gh, gw);
                    if let Some(advance) = state.world_renderer.font_advance() {
                        text.extend(crate::advancements_view::lines(
                            view, &self.lang, gw, gh, gw, px, advance,
                        ));
                    }
                    chrome.scissored.append(&mut adv.scissored);
                    chrome.sprites.append(&mut adv.sprites);
                    // Icons ride the item pass, fed beside the container
                    // path's own icons further down.
                    adv_icons = crate::advancements_view::icon_draws(view, gw, gh, px);
                }
            } else if self.death.is_some() {
                if let (Some(view), Some(screen)) =
                    (self.death.as_ref(), self.screen.screens.current())
                {
                    // The pass itself is built in `resumed`, beside
                    // `init_hud` and `init_container` — the one place the bake
                    // is still alive.
                    chrome = screen_chrome(screen, Some(mouse_gui));
                    if let Some(advance) = state.world_renderer.font_advance() {
                        text.extend(death_screen_lines(
                            view,
                            screen,
                            advance,
                            px,
                            (extent.width as f32, extent.height as f32),
                        ));
                    }
                }
            } else if let (Some(book), Some(screen)) =
                (self.book.as_ref(), self.screen.screens.current())
            {
                // M172: the written-book reader. Generic chrome carries the
                // transparent gradient + the Done button; the book art rides
                // `ScreenDraw::sprites`, which the pass draws AFTER the
                // backdrop and BEFORE the buttons — vanilla's stratum order
                // (gradient, then book.png in the same stratum; widgets and
                // text in the next).
                chrome = screen_chrome(screen, Some(mouse_gui));
                let gw = (extent.width as f32 / px) as i32;
                let m = (mouse_gui.0 as i32, mouse_gui.1 as i32);
                for d in rewo_ui::book_view_screen::draws(book, gw, Some(m)) {
                    chrome.sprites.push(book_sprite(d));
                }
                if let Some(advance) = state.world_renderer.font_advance() {
                    // The Done button's LABEL rides the generic widget-text
                    // builder; the first cut pushed only the page text and the
                    // eyeball pass found an unlabeled button (bookshot's
                    // witnesses probed chrome and page text, not the label —
                    // p9 counts changed pixels and the chrome alone passes it).
                    text.extend(screen_text_lines(screen, advance, px));
                    text.extend(book_text_lines(book, gw, advance, px, &self.lang));
                }
            } else if let (Some(view), Some(screen)) =
                (self.sign_edit.as_ref(), self.screen.screens.current())
            {
                // M174: the sign editor. Generic chrome carries the gradient +
                // the Done button; the board is a sprite blit riding
                // `chrome.sprites` (the book art's stratum — backdrop, then
                // board, then widgets/text), then selection/caret fills, then
                // the four centred lines.
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    c.sign_edit_frames += 1;
                }
                chrome = screen_chrome(screen, Some(mouse_gui));
                let gw = (extent.width as f32 / px) as i32;
                chrome
                    .sprites
                    .push(sign_board_sprite(view.state.kind, view.wood, gw));
                if let Some(advance) = state.world_renderer.font_advance() {
                    let (mut sign_text, fills) = sign_edit_draws(view, gw, advance, px);
                    chrome.sprites.extend(fills);
                    text.append(&mut sign_text);
                    // The Done button's label rides the generic widget-text
                    // builder (M172's lesson: an unlabeled button passed a
                    // probe that only watched chrome).
                    text.extend(screen_text_lines(screen, advance, px));
                }
            } else if let (Some(_page), Some(screen)) =
                (self.options_view, self.screen.screens.current())
            {
                // M173: an options page. The buttons and labels ride the
                // generic builders; the SLIDERS need their own sprite
                // lowering, because `screen_chrome` lowers only
                // `WidgetKind::Button` and a missing branch is an invisible
                // widget with no error.
                chrome = screen_chrome(screen, Some(mouse_gui));
                chrome
                    .sprites
                    .extend(slider_sprites(screen, Some(mouse_gui), self.options_drag));
                if let Some(advance) = state.world_renderer.font_advance() {
                    text.extend(screen_text_lines(screen, advance, px));
                }
            } else if !matches!(self.view, ScreenView::None) {
                // M85's three screens. All of their text is on their widgets,
                // so one generic builder serves all three — the death screen
                // keeps its own only because its title, cause and score are
                // *not* widgets in vanilla either.
                if let Some(screen) = self.screen.screens.current() {
                    chrome = screen_chrome(screen, Some(mouse_gui));
                    if let Some(advance) = state.world_renderer.font_advance() {
                        text.extend(screen_text_lines(screen, advance, px));
                    }
                }
                // r67 — the link prompt, as drawn: three buttons, each
                // caption, and the URL, with the chat out of the slot.
                #[cfg(feature = "gates")]
                if let (ScreenView::ConfirmLink(p), Some(c)) = (&self.view, self.check.as_mut()) {
                    let drawn = |s: &str| text.iter().any(|l| l.text == s);
                    if chrome.buttons.len() == 3
                        && drawn(&p.labels.yes)
                        && drawn(&p.labels.copy)
                        && drawn(&p.labels.no)
                        && text.iter().any(|l| l.text.contains(&p.url))
                        && self.chat_screen.is_none()
                    {
                        c.link_prompt_frames += 1;
                    }
                }
            }
            // M219 — `InBedChatScreen`'s Leave Bed button. The chat half is
            // drawn by the chat path above; this is `super.extractRenderState`
            // drawing the screen's one widget.
            if self.chat_screen.as_ref().is_some_and(|c| c.in_bed())
                && !self.screen.any_open()
                && self.death.is_none()
            {
                let screen = self.in_bed_screen();
                chrome = screen_chrome(&screen, Some(mouse_gui));
                if let Some(advance) = state.world_renderer.font_advance() {
                    text.extend(screen_text_lines(&screen, advance, px));
                }
                #[cfg(feature = "gates")]
                if let Some(c) = self.check.as_mut() {
                    if chrome.buttons.len() == 1
                        && text.iter().any(|l| l.text == screen.widgets[0].message)
                    {
                        c.in_bed_frames += 1;
                    }
                }
            }
            state.world_renderer.set_screen(chrome);
            // M178 — the advancements screen's icons, when its arm collected
            // any. `apply_gui_icons` is the exact call the container path
            // makes, so atlas packing and shading cannot drift between them.
            if !adv_icons.is_empty() {
                if let Some(gui) = self.gui_items.as_mut() {
                    let items: Vec<rewo_gpu::gui_item::GuiItem> = adv_icons
                        .iter()
                        .filter_map(|d| {
                            icon_for(
                                &self.items,
                                &session.trim_materials,
                                rewo_world::inventory::ItemSlot::plain(d.item, d.count.max(1)),
                                d.x,
                                d.y,
                                d.size,
                            )
                        })
                        .collect();
                    apply_gui_icons(&mut state.world_renderer, &mut state.gpu, gui, &items);
                }
            }
        }
        // M151 — the tab list's glyphs go LAST, matching its fills. `chat_range`
        // indexes into this vector and every screen builder above extends it,
        // so appending is the one placement that cannot move an index anything
        // else already holds.
        if let (Some(v), Some(l)) = (tab_list.as_ref(), tab_layout.as_ref()) {
            let advance = state.world_renderer.font_advance().copied();
            let width_of = move |t: &str, style: rewo_world::chat_style::ChatStyle| match &advance {
                Some(a) => rewo_gpu::text::width_styled(t, a, style.bold),
                None => 0,
            };
            let lines = crate::tab_list_view::text(v, l, px, &width_of);
            #[cfg(feature = "gates")]
            if let Some(c) = self.check.as_mut() {
                c.tab_list_text_max = c.tab_list_text_max.max(lines.len());
            }
            text.extend(lines);
        }
        state.world_renderer.set_text(text);
        if let Err(e) = state
            .world_renderer
            .anim_tick(&mut state.gpu, session.ticks)
        {
            log::error!("live: texture animation: {e}");
        }
    }

    /// Modules, the locator bar, the view-projection, capture and the GPU
    /// submit.
    fn submit(
        &mut self,
        event_loop: &ActiveEventLoop,
        view: &FrameView,
        session: &mut PlaySession,
        state: &mut LiveState,
    ) {
        let (alpha, eye, extent, aspect) = (view.alpha, view.eye, view.extent, view.aspect);
        // M52: resolve the module state once per frame. Every legit module
        // defaults off, so an unconfigured client produces exactly the
        // constants this path used before -- which is what keeps the golden
        // PNGs byte-identical.
        let mut render_modules = self.modules.render();
        // M233 — vanilla's dynamic FOV (sprint, speed, flying, a drawn bow,
        // the spyglass, water at the eye) multiplies the option the modules
        // resolved; everything below reads the one result.
        render_modules.fov_degrees = session
            .fov_degrees(render_modules.fov_degrees, alpha)
            .clamp(1.0, 179.0);
        #[cfg(feature = "gates")]
        {
            self.last_frame_fov = render_modules.fov_degrees;
        }
        // M83's locator bar. After `render_modules` because its bearing window
        // is measured against the same FOV the frame is projected through --
        // the Zoom module divides it, and a dot must not drift out of a strip
        // that is still 60 degrees wide in the shader's terms.
        {
            let scale = rewo_gpu::hud::gui_scale(extent.width as f32, extent.height as f32);
            let bar = resolve_locator_bar(
                session,
                &session.world.entities,
                &self.locator_styles,
                render_modules.fov_degrees,
                (extent.width as f32 / scale) as i32,
                (extent.height as f32 / scale) as i32,
                alpha,
            );
            state.world_renderer.set_locator_bar(bar);
        }
        let vp = eye_view_proj_hurt(
            eye,
            session.player.yaw,
            session.player.pitch,
            aspect,
            render_modules.fov_degrees,
            local_hurt_tilt(session, alpha, render_modules.damage_tilt_strength),
        );
        let draw = OverlayDraw {
            samples_ms: &self.ring.data,
            head: self.ring.head(),
            scale_ms: 20.0,
            origin: [16.0, 16.0],
            size: [560.0, 140.0],
        };
        let LiveState {
            window,
            gpu,
            renderer,
            world_renderer,
        } = state;
        if std::mem::take(&mut self.capture_pending) {
            // Vanilla copies the framebuffer it just presented. Rewo's
            // swapchain images carry no `TRANSFER_SRC`, so the equivalent is to
            // render the same state again into an offscreen target — which is
            // also what makes a supersampled capture possible later.
            //
            // **At the swapchain's format**, not the gates' default: a
            // `WorldRenderer` bakes its colour format into every pipeline, so
            // the live renderer can only draw into a matching attachment.
            match crate::capture::grab(
                gpu,
                world_renderer,
                vp,
                &draw,
                renderer.swapchain.format,
                renderer.swapchain.extent,
            ) {
                Ok(path) => log::info!("saved screenshot as {}", path.display()),
                Err(e) => log::warn!("screenshot failed: {e}"),
            }
        }
        // M86's gate samples here: after every `set_*` this frame made and
        // before `render` consumes them, which is the only window in which the
        // rings' current slots are the ones about to be bound.
        #[cfg(feature = "gates")]
        let baked_live = self.baked.is_some();
        #[cfg(feature = "gates")]
        let timed = self.in_timed_run();
        #[cfg(feature = "gates")]
        if let Some(c) = self.check.as_mut() {
            c.frames += 1;
            c.timed_frames += u64::from(timed);
            c.baked_frames += u64::from(baked_live);
            c.sample_rings(world_renderer);
            c.gui_items_ready |= world_renderer.gui_items_ready();
            c.hand_ready |= world_renderer.hand_ready();
            c.text_ready |= world_renderer.text_ready();
            c.container_ready |= world_renderer.container_ready();
            c.clouds_ready |= world_renderer.clouds_ready();
            c.weather_ready |= world_renderer.weather_ready();
            c.particles_ready |= world_renderer.particles_ready();
            c.border_ready |= world_renderer.border_ready();
            c.crumbling_ready |= world_renderer.crumbling_ready();
        }
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
                Ok(RenderOutcome::Rendered) | Ok(RenderOutcome::Skipped) => {
                    self.recreate_guard.ok();
                }
                Ok(RenderOutcome::NeedsRecreate) => {
                    match renderer
                        .recreate(gpu, size.width, size.height)
                        .and_then(|()| renderer.ensure_depth(gpu))
                    {
                        Ok(()) => self.recreate_guard.ok(),
                        Err(e) => {
                            // Retried on the next frame (`render` reports the
                            // mismatch as NeedsRecreate again); the client
                            // only exits when it keeps failing.
                            let give_up = self
                                .recreate_guard
                                .failed(self.started.elapsed().as_secs_f32());
                            log::error!(
                                "live: swapchain/depth recreate failed: {e}{}",
                                if give_up {
                                    " - giving up"
                                } else {
                                    " - retrying"
                                }
                            );
                            if give_up {
                                event_loop.exit();
                            }
                        }
                    }
                }
                Err(e) => {
                    log::error!("live: render failed: {e}");
                    event_loop.exit();
                }
            }
        }

        if let Some(limit) = self.run_seconds {
            let elapsed = self.started.elapsed().as_secs_f32();
            // M220 — the render check's tail (r84/r85) runs after the timed
            // run, when nothing else holds the screen; it gets up to
            // `RENDER_CHECK_TAIL_SECONDS` more and the run ends when it is done.
            #[cfg(feature = "gates")]
            let tail_done = self.check.is_none()
                || (self.mount_stage >= 20
                    && self.boat_stage >= 20
                    && self.horse_stage >= 20
                    && self.camel_stage >= 20
                    && self.nautilus_stage >= 20
                    && self.use_stage >= 100
                    // r91's last answer can land a frame after r90's own.
                    && self
                        .check
                        .as_ref()
                        .is_none_or(|c| c.seat_checks.iter().all(|e| e.2.is_some())));
            #[cfg(not(feature = "gates"))]
            let tail_done = true;
            if elapsed >= limit && (tail_done || elapsed >= limit + RENDER_CHECK_TAIL_SECONDS) {
                event_loop.exit();
            }
        }
    }
}
