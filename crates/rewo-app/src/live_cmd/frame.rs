use super::*;

impl LiveApp {
    pub(super) fn frame(&mut self, event_loop: &ActiveEventLoop) {
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
        // path that owns the screen.
        //
        // **This closes the CLIENT's screen only.** Vanilla's
        // `player.closeContainer()` also sends a serverbound
        // `container_close`, and Rewo resolves no such packet — `ids.rs` has
        // the clientbound one alone. So the server still believes the menu is
        // open. That gap is older and wider than this milestone (it affects
        // every screen close, not the beacon's), and is recorded rather than
        // half-fixed here.
        if std::mem::take(&mut self.screen.close_beacon) {
            self.set_screen_open(false);
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
                Some(_) => self.set_screen_open(true),
                // The menu closing closes the screen with it — but only if a
                // container was what put it up. Pressing E with no container
                // open must not be closed by this.
                None if self.screen.inventory_open() => self.set_screen_open(false),
                None => {}
            }
        }

        // M86's gate drives the inventory open for the second half of its run.
        //
        // The screen is one of the paths the bake fix re-enables, and it is the
        // one carrying the ninth instance of this milestone's bug —
        // `VelvetTextPass::sync_atlas` destroying its glyph image and rewriting
        // its descriptor set in place, whose own comment said "the caller is
        // expected to have idled" of a caller that does not. Nothing reaches it
        // except an open inventory, so a check that never opens one would grade
        // that fix not at all. The mouse is parked over the panel so a tooltip
        // lays out glyphs and the cache actually goes dirty.
        if let Some(c) = self.check.as_ref() {
            // Half of *this* run, not half of the default: `--render-check
            // --run-seconds 4` would otherwise never reach the trigger and
            // `r16` would fail with the screen having never opened. Caught by
            // exactly that.
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            let half = c.frames > 0 && self.started.elapsed().as_secs_f32() >= limit * 0.5;
            // M89 note: the guard is `!screen_forced_open`, not
            // `!inventory_open()`. Since M89 the injected container opens the
            // screen at 0.4, so an `inventory_open()` guard skips this whole
            // branch — INCLUDING the cursor park below, which is the only
            // thing that lays out a tooltip and therefore the only door to
            // `VelvetTextPass::sync_atlas`. `r16` would have stayed green
            // while no longer proving anything it was written for, which is
            // the same vacuity `p3` exists to catch in `containershot`.
            if half && !self.screen_forced_open {
                self.screen_forced_open = true;
                if !self.screen.inventory_open() {
                    self.set_screen_open(true);
                }
                // **After** the open, not before: `grab_for_screen` parks the
                // cursor in the middle of the window, which would overwrite
                // this. Menu slot 36 is hotbar slot 0, which the spawn handler
                // fills with a stack of dirt on the creative test server.
                //
                // The slot choice is not cosmetic. A tooltip is the only thing
                // in this client that lays out Velvet glyphs, an *empty* slot
                // produces none, and without one the glyph cache never goes
                // dirty and `VelvetTextPass::sync_atlas` never runs at all. Two
                // earlier cuts of this — a plausible-looking fraction of the
                // window, then the right slot set before the open — both left
                // the M3 mutation (deleting that path's `wait_idle`) alive
                // through the entire gate.
                if let Some(s) = self.state.as_ref() {
                    let e = s.window.inner_size();
                    let r = screen_slot_rects(e.width as f32, e.height as f32)
                        [rewo_world::inventory::HOTBAR_MENU_START];
                    self.screen.mouse = ((r.0 + r.2 * 0.5) as f64, (r.1 + r.2 * 0.5) as f64);
                }
            }
            // M88 — six-tenths through, open a CONTAINER over the inventory.
            //
            // Injected as a raw `open_screen` body through the production
            // router rather than staged by interacting with a real chest,
            // which is M17's precedent and its reasoning: raw-packet injection
            // into the production dispatcher is the deterministic proof, where
            // a live encounter depends on the server's own timing and on the
            // client aiming at the right block. What is being graded here is
            // the *render*, and this drives the whole chain that feeds it —
            // decode, layout resolution, `Menus::apply_open_screen`, and the
            // frame loop's choice of which menu to draw.
            //
            // A generic_9x3 chest: menu id 2, 63 slots, and a 168-tall panel,
            // so its geometry is distinguishable from the player's 46-slot
            // 166-tall one by more than rounding.
            // Four-tenths through — BEFORE the gate force-opens the inventory
            // at half (M89). The ordering is the witness: if the screen is up
            // between 0.4 and 0.5 it can only be because `open_screen` opened
            // it, which is the behaviour M87 was missing and M89 added. With
            // the injection after the forced open, the container would render
            // either way and `r21` could not tell.
            // M94 — open the recipe book, which no server does unprompted: a
            // fresh player's `RecipeBookSettings` are all shut, so without this
            // the windowed client never reaches the book's draw and `r23` would
            // be measuring nothing. Injected EARLY so it is live while the
            // inventory (a CRAFTING book menu) is on screen for `r16`.
            //
            // Eight booleans, four `(open, filtering)` pairs in
            // `RecipeBookSettings`' positional order — crafting first.
            if !self.book_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.92 {
                    if let Some(session) = self.session.as_mut() {
                        let id = session.ids.cb_play_recipe_book_settings;
                        let body = [1u8, 0, 0, 0, 0, 0, 0, 0];
                        session.apply_recipe_book(id, &body);
                        if session.recipe_book_settings.crafting.open {
                            self.book_injected = true;
                        }
                    }
                }
            }
            // M110 — force-open the chat screen a fifth of the way in. A
            // windowed run has no keyboard, so without this `T` reaches
            // nothing the gate can see and r27 measures a path no test drives
            // — the M86 shape. It goes in EARLY and stays open: the screen is
            // closed by nothing here, so every later frame carries its bar and
            // the count is unambiguous.
            // M132 — an objective, four scores and a display-slot assignment,
            // as raw bodies through the production router (M17's rule). No
            // server sets a sidebar up unprompted, so without this r42 would
            // be a witness over a path the gate cannot reach.
            //
            // Injected EARLY and never cleared, so the sidebar is live for
            // most of the run and its frame count is unambiguous. The fourth
            // holder is named `#hidden`, which `PlayerScoreEntry.isHidden`
            // filters out: a client that skipped that filter draws nine text
            // lines rather than seven and fails r42 on the count.
            if !self.sidebar_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.1 {
                    if let Some(session) = self.session.as_mut() {
                        let obj = "rewo_sidebar";
                        let mc_string = |b: &mut Vec<u8>, s: &str| {
                            rewo_proto::varint::write_varint(b, s.len() as i32);
                            b.extend_from_slice(s.as_bytes());
                        };
                        let nbt_string = |b: &mut Vec<u8>, s: &str| {
                            b.push(8); // TAG_String
                            b.extend_from_slice(&(s.len() as u16).to_be_bytes());
                            b.extend_from_slice(s.as_bytes());
                        };
                        // `set_objective`, METHOD_ADD.
                        let mut body: Vec<u8> = Vec::new();
                        mc_string(&mut body, obj);
                        body.push(0); // ADD
                        nbt_string(&mut body, "Rewo Sidebar");
                        rewo_proto::varint::write_varint(&mut body, 0); // INTEGER
                        body.push(0); // no number format
                        session.inject_packet(session.ids.cb_play_set_objective, &body);

                        // Four holders. The scores are deliberately out of
                        // order on the wire, so a client that drew them in
                        // arrival order rather than by value would place them
                        // differently — which the sort test pins and this
                        // keeps honest end to end.
                        for (owner, value) in
                            [("RewoAlpha", 3), ("#hidden", 999), ("RewoGamma", 9), ("RewoBeta", 6)]
                        {
                            let mut b: Vec<u8> = Vec::new();
                            mc_string(&mut b, owner);
                            mc_string(&mut b, obj);
                            rewo_proto::varint::write_varint(&mut b, value);
                            b.push(0); // no display override
                            b.push(0); // no number format
                            session.inject_packet(session.ids.cb_play_set_score, &b);
                        }

                        // `set_display_objective` — slot 1 is SIDEBAR. Sent
                        // last, because it is what makes the panel appear.
                        let mut b: Vec<u8> = Vec::new();
                        rewo_proto::varint::write_varint(
                            &mut b,
                            rewo_net::scoreboard::DisplaySlot::Sidebar.id(),
                        );
                        mc_string(&mut b, obj);
                        session.inject_packet(session.ids.cb_play_set_display_objective, &b);
                    }
                    self.sidebar_injected = true;
                }
            }
            // M151 — four players onto the tab list, and hold Tab.
            //
            // Both halves are needed and neither exists in a windowed run: a
            // fresh test server has exactly one player (this client), and
            // there is no keyboard, so without this r47 would be a witness
            // over a path the gate cannot reach — worse than no witness.
            //
            // **Two of the four are `listed: false`**, which is what makes the
            // row count a claim about `getListedOnlinePlayers()` rather than
            // about arrival: the list must come to THREE (the local player
            // plus the two listed ones), where a client that ignored the flag
            // scores five.
            //
            // The key goes down at half, not at the start, so
            // `tab_list_frames` must be strictly less than `frames` — a client
            // that drew the list unconditionally is exactly as wrong as one
            // that never drew it, and a `> 0` threshold cannot tell them
            // apart.
            {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                let elapsed = self.started.elapsed().as_secs_f32();
                if !self.tab_list_injected && elapsed >= limit * 0.45 {
                    if let Some(session) = self.session.as_mut() {
                        // ADD_PLAYER (0) | UPDATE_GAME_MODE (2) |
                        // UPDATE_LISTED (3) | UPDATE_LATENCY (4).
                        let mask = (1u8 << 0) | (1u8 << 2) | (1u8 << 3) | (1u8 << 4);
                        let mut body: Vec<u8> = vec![mask];
                        rewo_proto::varint::write_varint(&mut body, 4);
                        // `(name, listed, gamemode, latency)`. The spectator is
                        // named so that it would sort FIRST alphabetically and
                        // must not: `PLAYER_COMPARATOR` puts spectators after
                        // everyone else, one key ahead of the name.
                        for (i, (name, listed, mode, ms)) in [
                            ("RewoZulu", true, 0i32, 10i32),
                            ("RewoAlpha", true, 3, 800),
                            ("RewoHidden", false, 0, 50),
                            ("RewoGhost", false, 3, 50),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            // A uuid far from any real profile's, so a
                            // collision with the local player is impossible.
                            let uuid = 0x5157_0000_0000_0000_0000_0000_0000_0000u128 + i as u128;
                            body.extend_from_slice(&uuid.to_be_bytes());
                            rewo_proto::varint::write_varint(&mut body, name.len() as i32);
                            body.extend_from_slice(name.as_bytes());
                            rewo_proto::varint::write_varint(&mut body, 0); // no properties
                            rewo_proto::varint::write_varint(&mut body, mode);
                            body.push(u8::from(listed));
                            rewo_proto::varint::write_varint(&mut body, ms);
                        }
                        session.inject_packet(session.ids.cb_play_player_info_update, &body);
                    }
                    // M163 — a zombie whose custom name is a TRANSLATABLE, so
                    // r48 can see whether the windowed client's decode reached
                    // a language table. Injected as raw bodies through the
                    // production router (M17's rule): a live encounter would
                    // depend on a server spawning a mob and naming it, which is
                    // neither deterministic nor stageable without a third
                    // caller requirement (deliberately not added -- §0.0).
                    if let Some(session) = self.session.as_mut() {
                        if let Some(zombie) = self.etypes.id_of("minecraft:zombie") {
                            // `add_entity`: id, uuid, type, xyz, LpVec3 zero
                            // sentinel, then pitch / yaw / head-yaw bytes.
                            let mut add: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut add, NAMETAG_WITNESS_ID);
                            add.extend_from_slice(
                                &0x5257_0000_0000_0000_0000_0000_0000_0001u128.to_be_bytes(),
                            );
                            rewo_proto::varint::write_varint(&mut add, zombie);
                            // Beside the PLAYER, not at a fixed point: the
                            // label is gated on `name_tag_distance`, so a
                            // zombie at the origin scores zero for a reason
                            // that has nothing to do with the flatten. The
                            // first cut did exactly that and r48 read 0/0.
                            let p = &session.player;
                            for v in [p.x + 2.0, p.y, p.z] {
                                add.extend_from_slice(&v.to_be_bytes());
                            }
                            add.push(0); // LpVec3's zero sentinel
                            add.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                            session.inject_packet(session.ids.cb_play_add_entity, &add);
                            // `set_entity_data`: index 2, OPTIONAL_COMPONENT
                            // (serializer 6), present, `{translate: "..."}`.
                            let key = "entity.minecraft.zombie";
                            let mut meta: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut meta, NAMETAG_WITNESS_ID);
                            meta.extend_from_slice(&[2, 6, 1]);
                            meta.extend_from_slice(&[10, 8]);
                            meta.extend_from_slice(&9u16.to_be_bytes());
                            meta.extend_from_slice(b"translate");
                            meta.extend_from_slice(&(key.len() as u16).to_be_bytes());
                            meta.extend_from_slice(key.as_bytes());
                            meta.push(0); // TAG_End
                            // …and `DATA_CUSTOM_NAME_VISIBLE` (index 3,
                            // BOOLEAN) in the same body, because
                            // `Entity.shouldShowName()` IS
                            // `isCustomNameVisible()` for everything but a
                            // player — a named mob nobody has told to show its
                            // tag draws none, and `resolve_labels` returns
                            // `None` rather than the string.
                            meta.extend_from_slice(&[3, 8, 1]);
                            meta.push(0xFF); // metadata terminator
                            session.inject_packet(session.ids.cb_play_set_entity_data, &meta);
                        }
                    }
                    // M163 — a SIGN whose front text is a translatable, for
                    // r49. Written straight into the world rather than
                    // injected as two packets, and the reason is a decode rule
                    // rather than convenience: `World::set_block_entity_data`
                    // keeps vanilla's "a position with no block entity is
                    // ignored" (`lib.rs:218-221`), so a `block_entity_data`
                    // packet cannot CREATE one — a block entity arrives only
                    // in a chunk payload, and staging a real sign would mean a
                    // third caller requirement plus an SNBT form nothing here
                    // can check.
                    //
                    // What r49 grades is downstream of that anyway: the claim
                    // is that the windowed frame loop hands the language table
                    // to the sign collector, not that a block entity can be
                    // conjured mid-session.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(state) = self.blocks.default_state("minecraft:oak_sign") {
                            let (dx, dy, dz) = SIGN_WITNESS_OFFSET;
                            let (x, y, z) = (
                                session.player.x.floor() as i32 + dx,
                                session.player.y.floor() as i32 + dy,
                                session.player.z.floor() as i32 + dz,
                            );
                            session.world.set_block(x, y, z, state);
                            let face = rewo_proto::nbt::Nbt::Compound(vec![(
                                "messages".to_string(),
                                rewo_proto::nbt::Nbt::List(vec![
                                    rewo_proto::nbt::Nbt::Compound(vec![(
                                        "translate".to_string(),
                                        rewo_proto::nbt::Nbt::String(
                                            SIGN_WITNESS_KEY.to_string(),
                                        ),
                                    )]),
                                    rewo_proto::nbt::Nbt::String(String::new()),
                                    rewo_proto::nbt::Nbt::String(String::new()),
                                    rewo_proto::nbt::Nbt::String(String::new()),
                                ]),
                            )]);
                            session.world.block_entities.insert(
                                rewo_world::block_entities::BlockEntityPos { x, y, z },
                                rewo_world::block_entities::BlockEntity {
                                    // The type id is not read by the sign path
                                    // — `collect_sign_text` keys off the BLOCK
                                    // STATE through `SignStates::get` — so this
                                    // is a marker rather than a claim about the
                                    // block-entity registry.
                                    type_id: -1,
                                    data: rewo_proto::nbt::Nbt::Compound(vec![(
                                        "front_text".to_string(),
                                        face,
                                    )]),
                                },
                            );
                            log::info!("render-check: sign witness at ({x},{y},{z})");
                        }
                    }
                    self.tab_list_injected = true;
                }
                // r48 — TWO mobs of one type plus one of another, injected
                // through the production `add_entity` route (M17/M88's rule:
                // injection is the deterministic proof where a live encounter
                // depends on the server's spawn timing).
                //
                // This is the only check anywhere that drives
                // `etypes.name(type_id)` -> `kind_for_entity_name`: every
                // serverless gate, this milestone's `mobtexshot` included,
                // hands `kind` straight to `EntityDraw`. It is also the only
                // one that puts more than one mob in the WINDOWED client's
                // draw list, which is the axis §0.0's "a mob renders with
                // another mob's texture when more than one is in the scene"
                // report is defined by.
                //
                // The ids are fixed and named in `CROWD_IDS` because the census
                // below joins on them: without that, the count is a claim about
                // whatever the world contains rather than about this injection.
                if !self.crowd_injected && elapsed >= limit * 0.3 {
                    if let Some(session) = self.session.as_mut() {
                        let types = session.entity_types.clone();
                        let p = &session.player;
                        let eye = [p.x, p.y, p.z];
                        if let Some(types) = types {
                            let spawn = |eid: i32, tid: i32, dx: f64| -> Vec<u8> {
                                let mut b: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut b, eid);
                                // A uuid far from any real profile's.
                                let uuid =
                                    0xC0D0_0000_0000_0000_0000_0000_0000_0000u128 + eid as u128;
                                b.extend_from_slice(&uuid.to_be_bytes());
                                rewo_proto::varint::write_varint(&mut b, tid);
                                b.extend_from_slice(&(eye[0] + dx).to_be_bytes());
                                b.extend_from_slice(&eye[1].to_be_bytes());
                                b.extend_from_slice(&(eye[2] - 4.0).to_be_bytes());
                                // `LpVec3`'s one-byte zero sentinel: no motion.
                                b.push(0);
                                b.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                                b
                            };
                            let pid = session.ids.cb_play_add_entity;
                            for (i, (name, dx)) in [
                                ("minecraft:zombie", -1.5f64),
                                ("minecraft:zombie", 0.0),
                                ("minecraft:villager", 1.5),
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                if let Some(tid) = types.id_of(name) {
                                    let body = spawn(CROWD_IDS[i], tid, dx);
                                    session.inject_packet(pid, &body);
                                }
                            }
                        }
                    }
                    self.crowd_injected = true;
                }
                // M173 — walk the options pages through the PRODUCTION open
                // and close paths (r62): the Sound page at 0.06 (sliders
                // sampled while it is up), Done-equivalent closes at 0.12
                // (to the root), 0.14 (to the pause screen) and 0.16 (back
                // to the game), so nothing later meets a leftover screen or
                // a stale `self.view`.
                if self.options_staged == 0 && elapsed >= limit * 0.06 {
                    self.options_staged = 1;
                    self.open_options_screen(rewo_world::options_screen::OptionsPage::Sound);
                }
                if self.options_staged == 1 && elapsed >= limit * 0.12 {
                    self.options_staged = 2;
                    self.close_options(); // Sound -> Root, saving options.txt
                }
                if self.options_staged == 2 && elapsed >= limit * 0.14 {
                    self.options_staged = 3;
                    self.close_options(); // Root -> the pause screen
                }
                if self.options_staged == 3 && elapsed >= limit * 0.16 {
                    self.options_staged = 4;
                    self.close_view_screen(); // pause -> the game
                }
                // M169 — mount the bot on a saddled horse by INJECTION, the way
                // r48's crowd and r19's chest are staged (M17: injection is the
                // deterministic proof where a live encounter depends on the
                // server). The `/ride` command does not survive the render check
                // — the server never sent `set_passengers` naming the bot, so
                // `mounts` stayed empty and `jumpable_vehicle()` was always None.
                // Three packets through the production router: add the horse,
                // give it a saddle (slot 7), seat the bot on it. Before the
                // 0.32 hold so the meter has a vehicle to charge against.
                if !self.jump_injected && elapsed >= limit * 0.28 {
                    if let Some(session) = self.session.as_mut() {
                        let types = session.entity_types.clone();
                        if let (Some(types), Some(me)) = (types, session.player_id) {
                            if let Some(horse) = types.id_of("minecraft:horse") {
                                const HORSE_EID: i32 = 0x00A9_0000;
                                let p = &session.player;
                                let mut add: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut add, HORSE_EID);
                                let uuid = 0xC0D0_0000_0000_0000_0000_0000_00A9_0000u128;
                                add.extend_from_slice(&uuid.to_be_bytes());
                                rewo_proto::varint::write_varint(&mut add, horse);
                                add.extend_from_slice(&p.x.to_be_bytes());
                                add.extend_from_slice(&p.y.to_be_bytes());
                                add.extend_from_slice(&p.z.to_be_bytes());
                                add.push(0); // LpVec3 zero sentinel — no motion
                                add.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                                session.inject_packet(session.ids.cb_play_add_entity, &add);
                                // set_equipment: the saddle in slot 7. A minimal
                                // present stack (count 1, an arbitrary valid id,
                                // an empty patch); `saddled` only tests non-empty.
                                let mut eq: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut eq, HORSE_EID);
                                eq.push(7); // SADDLE ordinal, top bit clear = last
                                rewo_proto::varint::write_varint(&mut eq, 1); // count
                                rewo_proto::varint::write_varint(&mut eq, 1); // item id (any)
                                rewo_proto::varint::write_varint(&mut eq, 0); // added
                                rewo_proto::varint::write_varint(&mut eq, 0); // removed
                                session.inject_packet(session.ids.cb_play_set_equipment, &eq);
                                // set_passengers: seat the bot on the horse.
                                let mut pass: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut pass, HORSE_EID);
                                rewo_proto::varint::write_varint(&mut pass, 1);
                                rewo_proto::varint::write_varint(&mut pass, me);
                                session.inject_packet(session.ids.cb_play_set_passengers, &pass);
                            }
                        }
                    }
                    self.jump_injected = true;
                }
                // M170 — a leashed cow tied to a fence knot, injected so
                // `collect_leashes` builds a ribbon and the pass draws it.
                // The leash decode (`set_entity_link`) is graded by
                // `rideshot`; this proves the render path only a live frame
                // reaches. Same injection technique as the horse above.
                if !self.leash_injected && elapsed >= limit * 0.28 {
                    if let Some(session) = self.session.as_mut() {
                        let types = session.entity_types.clone();
                        if let Some(types) = types {
                            if let (Some(cow), Some(knot)) = (
                                types.id_of("minecraft:cow"),
                                types.id_of("minecraft:leash_knot"),
                            ) {
                                const COW_EID: i32 = 0x00C0_0000;
                                const KNOT_EID: i32 = 0x00C0_0001;
                                let p = &session.player;
                                let add = |eid: i32, tid: i32, x: f64, y: f64, z: f64| {
                                    let mut b: Vec<u8> = Vec::new();
                                    rewo_proto::varint::write_varint(&mut b, eid);
                                    b.extend_from_slice(&((eid as u128) << 8).to_be_bytes());
                                    rewo_proto::varint::write_varint(&mut b, tid);
                                    b.extend_from_slice(&x.to_be_bytes());
                                    b.extend_from_slice(&y.to_be_bytes());
                                    b.extend_from_slice(&z.to_be_bytes());
                                    b.push(0);
                                    b.extend_from_slice(&[0, 0, 0]);
                                    b
                                };
                                let cow_body = add(COW_EID, cow, p.x + 2.0, p.y, p.z);
                                let knot_body = add(KNOT_EID, knot, p.x + 3.0, p.y + 1.0, p.z);
                                session.inject_packet(session.ids.cb_play_add_entity, &cow_body);
                                session.inject_packet(session.ids.cb_play_add_entity, &knot_body);
                                // set_entity_link: source cow, dest knot —
                                // two fixed BE i32s.
                                let mut link: Vec<u8> = Vec::new();
                                link.extend_from_slice(&COW_EID.to_be_bytes());
                                link.extend_from_slice(&KNOT_EID.to_be_bytes());
                                session.inject_packet(session.ids.cb_play_set_entity_link, &link);
                            }
                        }
                    }
                    self.leash_injected = true;
                }
                // M172 — a written book into hotbar slot 0 + an `open_book`,
                // both through the production router. The window is
                // 0.465..0.5: after the jump hold (ends 0.46), before the
                // 0.5 inventory force-open — which then REPLACES the reader
                // (`Screens` is one slot) and exercises the stale-book sync,
                // so every later screen-dependent witness (r22-r25 at
                // 0.90-0.95) keeps its stage. The first cut injected at 0.93
                // and r24's overlay lost its inventory.
                if !self.book_view_injected && elapsed >= limit * 0.465 {
                    if let Some(session) = self.session.as_mut() {
                        let book_item = self.items.id("minecraft:written_book");
                        let comp = session.swing_data.as_ref().map(|d| d.components);
                        if let (Some(item), Some(comp)) = (book_item, comp) {
                            let pstr = |b: &mut Vec<u8>, t: &str| {
                                rewo_proto::varint::write_varint(b, t.len() as i32);
                                b.extend_from_slice(t.as_bytes());
                            };
                            let snbt = |b: &mut Vec<u8>, t: &str| {
                                b.push(8); // TAG_String
                                b.extend_from_slice(&(t.len() as u16).to_be_bytes());
                                b.extend_from_slice(t.as_bytes());
                            };
                            // written_book_content: Filterable<title>, author,
                            // generation, pages, resolved.
                            let mut value: Vec<u8> = Vec::new();
                            pstr(&mut value, "Gate Book");
                            value.push(0);
                            pstr(&mut value, "r61");
                            rewo_proto::varint::write_varint(&mut value, 0);
                            rewo_proto::varint::write_varint(&mut value, 2);
                            snbt(&mut value, "the first page");
                            value.push(0);
                            snbt(&mut value, "the second page");
                            value.push(0);
                            value.push(0);
                            // set_player_inventory: VarInt INVENTORY slot 0
                            // (hotbar 0 = menu 36 — the app's hotbar_slot is 0,
                            // which is the slot hand 0 resolves), then the stack.
                            let mut body: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut body, 0);
                            rewo_proto::varint::write_varint(&mut body, 1); // count
                            rewo_proto::varint::write_varint(&mut body, item);
                            rewo_proto::varint::write_varint(&mut body, 1); // added
                            rewo_proto::varint::write_varint(&mut body, 0); // removed
                            rewo_proto::varint::write_varint(
                                &mut body,
                                comp.written_book_content,
                            );
                            body.extend_from_slice(&value);
                            session.inject_packet(session.ids.cb_play_set_player_inventory, &body);
                            // open_book: one enum ordinal — hand 0.
                            session
                                .inject_packet(session.ids.cb_play_open_book, &[0]);
                        }
                    }
                    self.book_view_injected = true;
                }
                // The hold. Assigned every frame rather than latched, so it is
                // the same "read the key state fresh" the real gate does.
                self.keys.tab_list = elapsed >= limit * 0.5;
                // M169 — hold Space for a tenth of the run while mounted on
                // the staged horse: the meter climbs for ~16 ticks and the
                // release sends `START_RIDING_JUMP`. A level, like Tab.
                self.keys.jump = elapsed >= limit * 0.32 && elapsed < limit * 0.46;
            }
            // M162 — the two packet tails that carry a sound. Both call sites
            // live inside `PlaySession`, which owns a socket and cannot be
            // built in a unit test (M71), so this is the only check that can
            // reach them: `soundshot` grades the arithmetic and would stay
            // green with `queue_explosion_sound` deleted and `camera_eye`
            // returning `None` forever.
            //
            // Raw bodies through the production dispatcher (M17's rule), and
            // late enough in the run that `spawned` is true — before the
            // server's first `player_position` the camera is legitimately
            // absent and 1023 is correctly silent.
            // M166 — the PLAY-state `resource_pack_push`, injected because
            // NOTHING ELSE REACHES IT. A vanilla server pushes its pack during
            // configuration, so the play arm is only taken by `/resourcepack`
            // or a plugin, and without this the whole arm could be deleted with
            // every gate green — the M45/M158 shape, caught by re-reading this
            // milestone's own diff rather than by any check.
            //
            // Raw body through the production dispatcher (M17's rule), with a
            // DIFFERENT UUID from the configuration one so r55's list has to
            // grow rather than merely be non-empty.
            if !self.play_pack_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.55 {
                    if let Some(session) = self.session.as_mut() {
                        let mut b = PLAY_PACK_ID.to_be_bytes().to_vec();
                        // An `ftp:` URL, so this one must come back INVALID_URL
                        // and the two replies differ in their ACTION as well as
                        // their id. A second FAILED_DOWNLOAD would be satisfied
                        // by a client that ignored the url entirely.
                        for t in ["ftp://example.invalid/mid-session.zip", ""] {
                            rewo_proto::varint::write_varint(&mut b, t.len() as i32);
                            b.extend_from_slice(t.as_bytes());
                        }
                        b.push(0); // required = false
                        b.push(0); // no prompt
                        let pid = session.ids.cb_play_resource_pack_push;
                        session.inject_packet(pid, &b);
                        // Inside the `if let`, not after it: a run that reached
                        // the deadline before the session existed would
                        // otherwise mark itself injected and never inject.
                        self.play_pack_injected = true;
                    }
                }
            }
            if !self.sound_tails_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.3 {
                    let explode_holder = self
                        .sounds
                        .registry
                        .id_of("minecraft:entity.generic.explode")
                        .map(|id| id + 1);
                    let mut recorded: Option<([f64; 3], [f64; 3])> = None;
                    if let (Some(session), Some(holder)) = (self.session.as_mut(), explode_holder) {
                        let eye = [session.player.x, session.player.eye_y(), session.player.z];
                        let block = |dx: i64, dy: i64, dz: i64| {
                            let x = eye[0].floor() as i64 + dx;
                            let y = eye[1].floor() as i64 + dy;
                            let z = eye[2].floor() as i64 + dz;
                            let packed = ((x & 0x3FF_FFFF) << 38)
                                | ((z & 0x3FF_FFFF) << 12)
                                | (y & 0xFFF);
                            (x, y, z, packed as i64)
                        };
                        let level_event = |kind: i32, packed: i64, global: bool| {
                            let mut b = kind.to_be_bytes().to_vec();
                            b.extend_from_slice(&packed.to_be_bytes());
                            b.extend_from_slice(&0i32.to_be_bytes());
                            b.push(u8::from(global));
                            b
                        };

                        // 1023, a wither spawning 40 blocks north-east and 12
                        // up: far enough that "the sound is 2 blocks from the
                        // listener" cannot be confused with "the sound is at
                        // the block".
                        let (bx, by, bz, packed) = block(40, 12, 40);
                        recorded = Some((
                            eye,
                            [bx as f64 + 0.5, by as f64 + 0.5, bz as f64 + 0.5],
                        ));
                        session.inject_packet(
                            session.ids.cb_play_level_event.unwrap_or(-1),
                            &level_event(1023, packed, true),
                        );

                        // 3012, a trial spawner 200 blocks away: past the
                        // strict `distanceToSqr > 100.0` gate, so it must be
                        // QUEUED rather than played.
                        let (_, _, _, far) = block(200, 0, 0);
                        session.inject_packet(
                            session.ids.cb_play_level_event.unwrap_or(-1),
                            &level_event(3012, far, false),
                        );

                        // An `explode` whose TAIL carries the sound. The
                        // particle is named rather than numbered, through the
                        // session's own report-backed registry — a hardcoded id
                        // would fail closed for the wrong reason if the
                        // registry were ever renumbered.
                        if let Some(emitter) =
                            session.particle_types().id_of("minecraft:explosion_emitter")
                        {
                            let poof = session.particle_types().id_of("minecraft:poof");
                            let smoke = session.particle_types().id_of("minecraft:smoke");
                            if let (Some(poof), Some(smoke)) = (poof, smoke) {
                                let mut b = Vec::new();
                                for v in [eye[0], eye[1] - 1.0, eye[2]] {
                                    b.extend_from_slice(&v.to_be_bytes());
                                }
                                b.extend_from_slice(&4.0f32.to_be_bytes());
                                b.extend_from_slice(&37i32.to_be_bytes()); // fixed i32
                                b.push(0); // no playerKnockback
                                rewo_proto::varint::write_varint(&mut b, emitter);
                                rewo_proto::varint::write_varint(&mut b, holder);
                                rewo_proto::varint::write_varint(&mut b, 2);
                                for id in [poof, smoke] {
                                    rewo_proto::varint::write_varint(&mut b, id);
                                    b.extend_from_slice(&0.5f32.to_be_bytes());
                                    b.extend_from_slice(&1.0f32.to_be_bytes());
                                    rewo_proto::varint::write_varint(&mut b, 1);
                                }
                                session.inject_packet(session.ids.cb_play_explode, &b);
                            }
                        }
                    }
                    if let (Some(c), Some((eye, target))) = (self.check.as_mut(), recorded) {
                        c.global_event_eye = Some(eye);
                        c.global_event_target = Some(target);
                    }
                    self.sound_tails_injected = true;
                }
            }
            if !self.chat_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.2 {
                    // M111 — 25 lines, because the scrollbar's guard is
                    // `virtualHeight != chatHeight`: it does not exist until
                    // there is more chat than the focused box's twenty rows,
                    // and a run's own join messages come to about six. Without
                    // these r28 would be a witness over a path the gate cannot
                    // reach, which is worse than no witness.
                    //
                    // Injected as raw `system_chat` bodies through the
                    // production router (M17's rule: injection is the
                    // deterministic proof where a live trigger depends on
                    // timing nothing here controls).
                    if let Some(session) = self.session.as_mut() {
                        let id = session.ids.cb_play_system_chat;
                        for i in 0..25u8 {
                            let text = format!("scrollbar filler {i}");
                            let mut body: Vec<u8> = vec![8];
                            body.extend_from_slice(&(text.len() as u16).to_be_bytes());
                            body.extend_from_slice(text.as_bytes());
                            body.push(0); // overlay = false
                            if let Some(pid) = id {
                                session.inject_packet(pid, &body);
                            }
                        }
                    }
                    // M126d — one message carrying legacy codes, so the
                    // drawn chat has more than one colour and at least one
                    // non-plain flag. Injected as a raw `system_chat` body
                    // through the production router (M17's rule) rather than
                    // sent as chat, because a server owns what it echoes back
                    // and `§` in a player message is not something the gate
                    // can rely on surviving the round trip.
                    //
                    // Every stage has to work for this to score: the NBT
                    // string reaches `parse_component`, `push_legacy` resolves
                    // the codes into five spans, `FlatComponents` carries them
                    // through the wrap as separate parts, and `chat_lines`
                    // emits one text line each. A flatten anywhere in that
                    // chain drops both counters to zero.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(pid) = session.ids.cb_play_system_chat {
                            let text = concat!(
                                "\u{00a7}crewored \u{00a7}9blue ",
                                "\u{00a7}oital \u{00a7}nunder \u{00a7}mstrike"
                            );
                            let mut body: Vec<u8> = vec![8];
                            body.extend_from_slice(&(text.len() as u16).to_be_bytes());
                            body.extend_from_slice(text.as_bytes());
                            body.push(0); // overlay = false
                            session.inject_packet(pid, &body);
                        }
                    }
                    // M127 — two `disguised_chat` bodies, bound to the
                    // SERVER's own chat types by name. `handleDisguisedChat`
                    // is the shortest path to a decorated line: a component,
                    // a bound, and no signature machinery. Injected as raw
                    // bodies through the production router (M17's rule).
                    //
                    // The ids are looked up rather than assumed, because the
                    // registry is datapack-driven and the index is whatever
                    // the server's packs say. That also makes the lookup part
                    // of the claim: a client that never parsed
                    // `registry_data` for this registry answers `None` here
                    // and injects nothing, so r40 and r41 read zero instead of
                    // quietly decorating with entry 0.
                    if let Some(session) = self.session.as_mut() {
                        let pid = session.ids.cb_play_disguised_chat;
                        let plain = session.chat_type_id("minecraft:chat");
                        let whisper =
                            session.chat_type_id("minecraft:msg_command_incoming");
                        let mut body = |content: &str, name: &str, id: i32| {
                            let mut b: Vec<u8> = vec![8];
                            b.extend_from_slice(&(content.len() as u16).to_be_bytes());
                            b.extend_from_slice(content.as_bytes());
                            // `ByteBufCodecs.holder` — id + 1, 0 meaning inline.
                            rewo_proto::varint::write_varint(&mut b, id + 1);
                            b.push(8);
                            b.extend_from_slice(&(name.len() as u16).to_be_bytes());
                            b.extend_from_slice(name.as_bytes());
                            b.push(0); // no targetName
                            b
                        };
                        if let Some(id) = plain {
                            let b = body("decorated", "RewoDecoWitness", id);
                            session.inject_packet(pid, &b);
                        }
                        if let Some(id) = whisper {
                            let b = body("whispered", "RewoStyleWitness", id);
                            session.inject_packet(pid, &b);
                    }
                    }
                    // M128 — one message carrying a `click_event`, so the
                    // gate can click it. A COMPOUND rather than a bare string,
                    // because a `click_event` has nowhere to live on a string
                    // component; assembled as network NBT bytes by hand, which
                    // is what `system_chat`'s content is.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(pid) = session.ids.cb_play_system_chat {
                            let body = click_witness_body();
                            session.inject_packet(pid, &body);
                        }
                    }
                    // M115 — a completion word, then a keystroke, so r29
                    // measures the whole production chain rather than a
                    // hand-built `Suggestions`: the packet reaches
                    // `SuggestionProviderState`, `tab_words()` unions it with
                    // the online players, `on_edited` matches the typed prefix
                    // against it, and `auto_show` opens the list. A break
                    // anywhere in that drops the count to zero.
                    //
                    // The word is deliberately nothing a server would send, so
                    // it cannot be confused with a real player's name, and it
                    // begins with the character typed below.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(pid) = Some(session.ids.cb_play_custom_chat_completions) {
                            let words = ["rewopopupwitness", "rewopopupsecond"];
                            let mut body: Vec<u8> = Vec::new();
                            body.push(2); // Action.SET
                            body.push(words.len() as u8);
                            for w in words {
                                body.push(w.len() as u8);
                                body.extend_from_slice(w.as_bytes());
                            }
                            session.inject_packet(pid, &body);
                        }
                    }
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Message);
                    // `onEdited` is what turns suggestions on and asks for
                    // them; nothing else in a windowed run types.
                    self.chat_char('r');
                    self.chat_injected = true;
                }
            }
            // M128 — click the link, once, at the position the renderer
            // reported for it. Between the chat screen opening (0.2) and the
            // command screen replacing it (0.55), so the popup under the input
            // bar cannot be what answers.
            if self.chat_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                let due = self.started.elapsed().as_secs_f32() >= limit * 0.4;
                let at = self.check.as_ref().and_then(|c| {
                    (!c.chat_click_fired).then_some(c.chat_link_at).flatten()
                });
                if let (true, Some((lx, ly))) = (due, at) {
                    if let Some(c) = self.check.as_mut() {
                        c.chat_click_fired = true;
                    }
                    // Four pixels down and one right of the line's origin —
                    // inside the first glyph's `[x, x + advance) x [y, y + 9)`
                    // box at any GUI scale, because both are screen pixels and
                    // the box is at least that big.
                    self.screen.mouse = ((lx + 1.0) as f64, (ly + 4.0) as f64);
                    // …and close the suggestion popup first, because
                    // `ChatScreen::mouse_clicked` gives it precedence over the
                    // link (`if self.suggestions.mouse_clicked(..) { return
                    // Handled }`) and `SuggestionsList` claims ANY click inside
                    // its rect. That is vanilla's ordering, not a bug.
                    //
                    // The comment above used to argue the popup "cannot be what
                    // answers" because the click sits between the screen opening
                    // and the command screen replacing it. That held on M128's
                    // own branch and stopped holding when M127c's decoration
                    // witnesses joined the same injection block: the clickable
                    // row is the NEWEST message, so it draws nearest the input
                    // bar — which is exactly where the popup is. The click
                    // answered `Handled`, the popup's answer, not the link's.
                    //
                    // Closing it does not weaken the claim. r42 asks whether a
                    // click on a chat link runs its command; the popup is a
                    // different widget, and r29 already witnesses that it draws.
                    if let Some(cs) = self.chat_screen.as_mut() {
                        cs.suggestions.hide();
                    }
                    let outcome = self.chat_mouse_pressed(0);
                    if outcome
                        == rewo_world::chat_screen::ChatClick::RunCommand(
                            CLICK_WITNESS_COMMAND.into(),
                        )
                    {
                        if let Some(c) = self.check.as_mut() {
                            c.chat_click_ok += 1;
                        }
                    } else {
                        log::warn!("rendercheck: chat click answered {outcome:?}");
                    }
                }
            }
            // M116 — later, and as its own screen, so r29's frames stay
            // unambiguously the message popup's. Reopening with `Command`
            // seeds the field with `/`; one letter then reaches the top-level
            // literals, which the dispatcher answers without a packet.
            if self.chat_injected && !self.command_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.55 {
                    self.close_chat_screen();
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                    self.chat_char('g');
                    // …and then on to `/give `, which is where an ARGUMENT is
                    // expected: the integer child suggests nothing, so no
                    // popup opens and the USAGE box takes its place. r30 has
                    // already counted the literal completion `/g` produced;
                    // this reaches r31 and r32.
                    // …then on to `/give @s `, which is two arguments deep.
                    // M118 changed what `/give ` alone shows: its first
                    // argument is `minecraft:entity`, so a SELECTOR POPUP
                    // opens there and the usage box is suppressed by the
                    // mutual exclusion — which is what vanilla does too. The
                    // box needs an argument that suggests nothing, and the
                    // item after the targets is one.
                    // …then on to `/give @s dirt `, which is three arguments
                    // deep. **r32's precondition recedes by one word for every
                    // argument type transcribed**: M118 made `/give ` open a
                    // selector popup and M119 made `/give @s ` open an item
                    // one, each time suppressing the usage box by the mutual
                    // exclusion. The count after the item is a plain integer
                    // and suggests nothing, which is what the box needs.
                    for ch in "ive @s dirt ".chars() {
                        self.chat_char(ch);
                    }
                    self.command_injected = true;
                }
            }
            // M120 — and a coordinate. Injected LAST, at 0.8, so r32 has
            // already banked its frames against `/give @s dirt `: a
            // coordinate popup suppresses the box, which is the receding
            // precondition M118 and M119 both hit.
            if self.command_injected && !self.coords_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.8 {
                    self.close_chat_screen();
                    // **Drop the draft first.** `close_chat_screen` saves one,
                    // and `ChatMethod::Command` restores a COMMAND draft — so
                    // reopening handed the field back `/give @s dirt ` and the
                    // typing below appended to it. That is M110's
                    // `isDraftRestorable` working exactly as documented, and
                    // the gate read as "the coordinate family offers nothing"
                    // when what it had actually typed was
                    // `/give @s dirt setblock `.
                    self.chat_draft = None;
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                    for ch in "setblock ".chars() {
                        self.chat_char(ch);
                    }
                    self.coords_injected = true;
                }
            }
            // M124 — a literal table, injected LAST at 0.9 for the reason
            // every one of these has been: whichever popup is open last
            // suppresses the usage box, and r32 banks its frames earlier.
            if self.coords_injected && !self.literal_table_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.9 {
                    self.close_chat_screen();
                    // The draft drop M120 needed, for M110's reason.
                    self.chat_draft = None;
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                    for ch in "scoreboard objectives setdisplay ".chars() {
                        self.chat_char(ch);
                    }
                    self.literal_table_injected = true;
                }
            }
            // M134 — a command that does not exist, which is the shortest
            // route to `updateUsageInfo`'s THIRD branch and therefore to an
            // exception message. Injected after every other typing stage
            // because it is the one that must not have a popup over it: no
            // literal matches, so no completion is offered, so
            // `extractRenderState` falls through to the usage box. Every
            // earlier stage exists to open a popup, and a popup would hide
            // this.
            //
            // Nothing is recorded in `getExceptions()` for this input — the
            // root's children are all literals, none matched, and
            // `getRelevantNodes` therefore returned the empty argument set —
            // so this reaches the line through `Commands.getParseException`
            // rather than through the exception loop. That is the branch M117
            // omitted entirely.
            if self.literal_table_injected && !self.bad_command_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.95 {
                    self.close_chat_screen();
                    // The draft drop M120 needed, for M110's reason.
                    self.chat_draft = None;
                    self.open_chat_screen(rewo_world::chat_screen::ChatMethod::Command);
                    for ch in "rewonosuchcommand".chars() {
                        self.chat_char(ch);
                    }
                    self.bad_command_injected = true;
                }
            }
            if !self.container_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.4 {
                    if let Some(session) = self.session.as_mut() {
                        // VarInt container id, VarInt menu type (RAW, not a
                        // holder), then an NBT string title.
                        let mut body: Vec<u8> = vec![7, 2, 8];
                        let title = b"Chest";
                        body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                        body.extend_from_slice(title);
                        let id = session.ids.cb_play_open_screen;
                        let opened =
                            rewo_net::route_menu(id, &body, &session.ids, &mut session.menus)
                                && session.menus.open().is_some();
                        if opened {
                            self.container_injected = true;
                        }
                    }
                }
            }
            // M174 — open the staged oak_sign's editor through the PRODUCTION
            // decode (`inject_packet` -> the play dispatch), at 0.80: after
            // the container's r19/r20 have latched, late enough that the
            // inventory (force-opened at 0.5) keeps the >25%-of-frames
            // window r16 requires — the first cut injected at 0.55 and
            // starved it to 21% — and before the 0.90 book-menu injection
            // whose replacement exercises r64's commit.
            if !self.sign_editor_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.80 {
                    if let Some(session) = self.session.as_mut() {
                        let (dx, dy, dz) = SIGN_WITNESS_OFFSET;
                        let (x, y, z) = (
                            session.player.x.floor() as i32 + dx,
                            session.player.y.floor() as i32 + dy,
                            session.player.z.floor() as i32 + dz,
                        );
                        // Packed BlockPos: x<<38 | z<<12 | y — the packing
                        // `sign_update_body` writes and `position()` reads.
                        let packed: u64 = (((x as i64 & 0x3ff_ffff) as u64) << 38)
                            | (((z as i64 & 0x3ff_ffff) as u64) << 12)
                            | ((y as i64 & 0xfff) as u64);
                        let mut body = Vec::new();
                        body.extend_from_slice(&packed.to_be_bytes());
                        body.push(1); // isFrontText
                        session.inject_packet(session.ids.cb_play_open_sign_editor, &body);
                        self.sign_editor_injected = true;
                    }
                }
            }
            // M94 — last of all, a CRAFTING TABLE, because the book only draws
            // while a book menu is on screen and neither of the two injections
            // below is one: `book_type_of` answers for the player's own
            // inventory, `crafting`, and the three furnaces, and nothing else.
            // The chest opened at 0.4 holds the screen for the rest of the run,
            // so without this the windowed client never reaches the book's
            // builder however long it runs — the same shape of gap as M92's
            // overlay injection, one screen over.
            //
            // Last (0.90) so `r20` and `r22` have already latched: a crafting
            // table's panel is 176x166, the player's own size, and it draws no
            // overlays.
            if !self.book_menu_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.90 {
                    if let Some(session) = self.session.as_mut() {
                        // Menu type 12. NOT 13 - that is `enchantment`,
                        // which this first got wrong, and which opened a
                        // perfectly valid screen with no book.
                        let mut body: Vec<u8> = vec![11, 12, 8];
                        let title = b"Crafting";
                        body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                        body.extend_from_slice(title);
                        let open_id = session.ids.cb_play_open_screen;
                        if rewo_net::route_menu(open_id, &body, &session.ids, &mut session.menus) {
                            self.book_menu_injected = true;
                        }
                    }
                }
            }
            // M104 — and then the which-of-these overlay, through the SAME
            // `open_overlay` a right-click calls.
            //
            // Injected rather than clicked because a click needs the cursor
            // over a particular cell AND a server that has sent a multi-recipe
            // group, neither of which this gate controls — M17's rule. It must
            // come after the crafting table (0.90), because `set_screen_open`
            // clears the overlay: a snapshot of a page must not survive the
            // screen it was taken on.
            if !self.book_overlay_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.95 && self.book_menu_injected {
                    // Only `filtering` and `furnace_family` are read out of
                    // the view, and both are false for a crafting table's
                    // unfiltered book — the rest is named so the fixture reads
                    // as a whole `BookView` rather than a partial one.
                    let view = rewo_world::recipe_book_screen::BookView {
                        tabs: rewo_world::recipe_book_screen::CRAFTING_TABS.len(),
                        selected_tab: 0,
                        page: 0,
                        total_pages: 1,
                        shown: 1,
                        filtering: false,
                        furnace_family: false,
                    };
                    let collection = (0..3)
                        .map(|i| rewo_world::recipe_overlay::Button {
                            recipe: i,
                            craftable: i == 0,
                            slots: Vec::new(),
                        })
                        .collect();
                    self.screen.book_overlay = Some(open_overlay(collection, 0, view));
                    self.book_overlay_injected = true;
                }
            }
            // M92 — near the end, replace it with a BREWING STAND and give it
            // data, so the frame loop has to reach the overlay builder.
            //
            // A chest has no overlays at all, so the injection above cannot
            // exercise this path however long it runs. `containershot` grades
            // the overlays offscreen; without this, nothing says the windowed
            // client ever draws one — the gap M88 closed for the panel and M86
            // for nine features before that.
            //
            // Late (0.85) and after `r20` has latched its height, because a
            // brewing stand's panel is 166 tall — the same as the player's —
            // and so cannot serve `r20`'s discrimination.
            if !self.brewing_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.85 {
                    if let Some(session) = self.session.as_mut() {
                        let mut body: Vec<u8> = vec![9, 11, 8];
                        let title = b"Brewing Stand";
                        body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                        body.extend_from_slice(title);
                        let open_id = session.ids.cb_play_open_screen;
                        if rewo_net::route_menu(open_id, &body, &session.ids, &mut session.menus) {
                            // ...and its data: 200 ticks left of a brew, 20
                            // charges of fuel. Both packets through the
                            // production router (M17's precedent), so the
                            // decode, the id gate and the data-slot write are
                            // all the shipped ones.
                            let data_id = session.ids.cb_play_container_set_data;
                            for (slot, value) in [(0i16, 200i16), (1, 20)] {
                                // VarInt container id, then two BE i16s —
                                // fixed-width shorts among the var-ints (M87).
                                let mut d = vec![9u8];
                                d.extend_from_slice(&slot.to_be_bytes());
                                d.extend_from_slice(&value.to_be_bytes());
                                rewo_net::route_menu(data_id, &d, &session.ids, &mut session.menus);
                            }
                            self.brewing_injected = true;
                        }
                    }
                }
            }
            // Three-quarters through, turn on advanced tooltips (F3+H), which
            // adds the item's id as a second line.
            //
            // This is what makes the Velvet fix *gradeable*. The rebuild the
            // screen-open triggers happens on the first frame the pass exists,
            // before it has ever drawn — so destroying its image then is legal
            // and the M3 mutation survived it. New glyphs arriving while the
            // pass is already drawing every frame is the case the `wait_idle`
            // is actually for, and a second tooltip line is the cheapest way to
            // produce it.
            if half && self.screen.inventory_open() && !self.advanced_tooltips {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.75 {
                    self.advanced_tooltips = true;
                }
            }
        }

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
                        .unwrap_or(rewo_world::disconnect_screen::DisconnectCause::EndOfStream),
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
                s.kind == rewo_world::screen::ScreenKind::Options
                    && (s.width != gw || s.height != gh)
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
                .is_some_and(|s| s.kind == rewo_world::screen::ScreenKind::Options)
            {
                self.options_view = None;
                self.options_drag = None;
            }
        }
        if self.exit_requested {
            event_loop.exit();
            return;
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
            return;
        }

        // Fixed 20 Hz tick on an accumulator.
        let Some(session) = self.session.as_mut() else {
            return;
        };
        // Vanilla applies inbound packets every frame, independent of the
        // 20 Hz tick; the budget keeps a backlog from hitching one frame.
        if let Err(e) = session.pump(rewo_net::play::PumpBudget::FRAME) {
            log::error!("live: packet handling failed: {e}");
            session.disconnect = Some(e);
            session.disconnect_cause =
                Some(rewo_world::disconnect_screen::DisconnectCause::ClientError);
            return;
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
                    Some(rewo_world::disconnect_screen::DisconnectCause::ClientError);
                return;
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
                    rewo_world::death_screen::DeathScreen::tick(s);
                }
            }
            ran_tick = true;
        }
        if session.disconnect.is_some() {
            // Serviced at the top of the *next* frame, by the block that opens
            // the disconnect screen. Returning here rather than acting is what
            // keeps that decision in one place — and this frame has already
            // borrowed the session it is about to drop.
            return;
        }
        if ran_tick && session.spawned && !self.logged_spawn {
            self.logged_spawn = true;
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
            if self.check.is_some() {
                let _ = session.send_chat("rewo render-check");
                // M164 (r48) — a waterlogged block for the mesher to find.
                // ABOVE the player, not beside: `~ ~2 ~` is air wherever a
                // player is standing, while `~2 ~ ~` assumes flat ground.
                let _ = session
                    .send_command("setblock ~ ~2 ~ minecraft:oak_slab[type=bottom,waterlogged=true]");
            }
            log::info!(
                "live: spawned at ({:.1},{:.1},{:.1})",
                session.player.x,
                session.player.y,
                session.player.z
            );
            // Put a stack of dirt in slot 0 so right-click can place (the
            // test server is creative — a no-op elsewhere).
            if let Some(dirt) = self.dirt_item {
                let _ = session.creative_set_hotbar(0, dirt, 64);
                let _ = session.select_hotbar(0);
            }
        }

        let Some(state) = self.state.as_mut() else {
            return;
        };
        // Upload finished meshes + feed the worker pool. (Uploads are
        // async slot-ring submissions — the CPU never waits on the copy;
        // same-queue FIFO ordering keeps this frame's draws safe.)
        let mut carried_cells = self.check.as_ref().map_or(0, |c| c.carried_fluid_cells);
        let pumped = pump_meshing(
            session,
            &mut state.gpu,
            &mut state.world_renderer,
            &mut self.pool,
            UPLOAD_BUDGET,
            &mut carried_cells,
        );
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
                return;
            }
        }

        // Player skins: request any newly-announced ones, upload any that
        // finished fetching (real skins on online-mode servers).
        for (uuid, info) in session.take_pending_skins() {
            self.skins.request(uuid, &info);
        }
        self.skins
            .poll_uploads(&mut state.gpu, &mut state.world_renderer);

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
        );
        // M163 — read the label OFF THE DRAW LIST, not off the entity table:
        // the claim is that the resolved string reached the renderer, and a
        // table-level read would stay green if `resolve_labels` dropped it.
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
            let named = |want: &str| {
                draws
                    .iter()
                    .any(|d| d.name == Some(want))
            };
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
        self.sounds.update_listener(
            eye_f64(session),
            session.player.yaw,
            session.player.pitch,
        );
        // Read BACK off the device rather than counting the call: deleting
        // `update_listener`'s body would leave a call-site counter green.
        let pushes = self.sounds.device.listener_pushes;
        let fwd = self.sounds.device.last_listener.map(|t| t.forward);
        // Read off the MANAGER, which is the engine's, rather than off anything
        // this frame computed — the claim is that a track was actually started,
        // not that a situation was named.
        let music = self.sounds.system.music.current().map(str::to_string);
        // M162 — LATCHED rather than sampled at the end, because a one-shot is
        // reclaimed 20 ticks after its channel is released and the run goes on
        // for several seconds after the injection. Read off the ENGINE, not off
        // anything the injection computed.
        let wither = self
            .sounds
            .system
            .engine
            .live_position("minecraft:entity.wither.spawn");
        let boom = self
            .sounds
            .system
            .engine
            .live_position("minecraft:entity.generic.explode");
        let queued = self.sounds.system.stats.queued_delayed;
        let (tails, sounds_queued) = (
            session.motion_stats.explosion_tails,
            session.motion_stats.explosion_sounds,
        );
        if let Some(c) = self.check.as_mut() {
            c.listener_pushes = pushes;
            c.listener_forward = fwd;
            if music.is_some() {
                c.music_started = music;
            }
            if c.global_event_sound.is_none() {
                c.global_event_sound = wither;
            }
            if c.explosion_sound.is_none() {
                c.explosion_sound = boom;
            }
            c.queued_delayed = c.queued_delayed.max(queued);
            c.explosion_tails = c.explosion_tails.max(tails);
            c.explosion_sounds_queued = c.explosion_sounds_queued.max(sounds_queued);
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
                        let w = self.weather.get_or_insert_with(|| WeatherAssets::new(baked));
                        rain_fog_band(session, w, Some(dt * 20.0))
                    }
                    None => [1.0e9, 1.0e9 + 1.0],
                };
                // r3: the sentinel the dead branch produced is 1e9 blocks out.
                // Counting a *finite* band rather than "we took the Some arm"
                // is what makes this a value witness — the arm could be taken
                // and still hand the renderer nonsense.
                if let Some(c) = self.check.as_mut() {
                    if band[0] < 1.0e8 {
                        c.fog_band_frames += 1;
                    }
                }
                band
            },
        );
        apply_biome_sky_fog(&mut state.world_renderer, session);
        let bes = collect_block_entities(&session.world, &self.chest_states, &lightmap, alpha, session.game_time(), (cr, cu));
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
        if let Err(e) = state.world_renderer.set_end_portals(
            &mut state.gpu,
            &portals,
            session.game_time(),
        ) {
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
        held.extend(draws.iter().filter_map(|d| d.ground_item));
        held.extend(bes.iter().map(|b| b.model.as_str()));
        // A failed upload leaves the item simply absent (no resident slot →
        // no quads), which is preferable to killing the frame loop.
        if let Err(e) = state.world_renderer.prepare_held_items(&mut state.gpu, &held) {
            log::warn!("live: held-item texture upload: {e}");
        }
        let be_draws: Vec<_> = bes.iter().map(|b| b.as_draw()).collect();
        let sign_lines = match state.world_renderer.font_advance() {
            Some(a) => collect_session_sign_text(session, &self.sign_states, &lightmap, a),
            None => Vec::new(),
        };
        // M163/r49 — read the sign line OFF THE DRAW LIST, for r48's reason
        // one list over: the claim is that the resolved string reached the
        // renderer. The RAW-KEY half is a prefix match because
        // `getRenderMessages` truncates each line to the board, so an
        // unresolved `block.minecraft.dirt` arrives as `block.minecraft.di`.
        if let Some(c) = self.check.as_mut() {
            if sign_lines.iter().any(|l| l.text == SIGN_WITNESS_RESOLVED) {
                c.sign_resolved_frames += 1;
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
            })
            .collect();
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
                if d.kind != rewo_gpu::mobs::EntityModelKind::Capsule
                    && !kinds.contains(&d.kind)
                {
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
            &sign_draws,
            cr,
            cu,
            anim_time,
        );
        drop(draws);

        let extent = state.renderer.swapchain.extent;
        let aspect = extent.width.max(1) as f32 / extent.height.max(1) as f32;
        // Targeted block for the selection outline.
        let hit = session.target_block(
            eye_f64(session),
            look_dir(session.player.yaw, session.player.pitch),
            REACH,
        );
        state.world_renderer.set_selection(hit.map(|h| h.block));
        state
            .world_renderer
            .set_leash(&collect_leashes(session, &self.etypes, alpha, &lightmap));
        // Camera + lightmap were already set above from the same eye/state this
        // frame — don't duplicate them. Celestial still tracks the world clock.
        state
            .world_renderer
            .set_celestial({
                let mut cel = celestial_state_of(session.day_ticks);
                apply_weather_to_celestial(&mut cel, session);
                cel
            });
        state
            .world_renderer
            .set_end_flash(end_flash_render(session, lightmap_partial));
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
        if let Some(baked) = self.baked.as_ref() {
            if let Some(c) = self.check.as_mut() {
                c.gui_item_frames += 1;
            }
            let items = self.items.clone();
            let gi = self.gui_items.get_or_insert_with(|| GuiItemState::new(baked));
            if self.screen.inventory_open() {
                if let Some(c) = self.check.as_mut() {
                    c.screen_frames += 1;
                }
                let beacon_override = session
                    .menus
                    .open()
                    .filter(|m| m.layout.protocol_id == BEACON_MENU_PROTOCOL_ID)
                    .map(|m| beacon_live(&mut self.screen, m, &self.beacon_effects));
                // M93s — likewise, resolved before the borrow of `state`.
                let cut = session
                    .menus
                    .open()
                    .filter(|m| {
                        m.layout.protocol_id
                            == rewo_world::menu_screen::STONECUTTER_MENU_PROTOCOL_ID
                    })
                    .map(|m| cut_view(&mut self.screen, m, &items));
                // M93t — likewise the anvil's field, seeded here so the render
                // and the key handler see one box.
                // M98 — the book's own selection, from the screen state.
                let book_state = self.screen.book;
                // M99 — and its search text, lowercased once here.
                let book_query =
                    rewo_world::recipe_search::normalize(&self.screen.book_search.value());
                let anvil_field = session
                    .menus
                    .open()
                    .filter(|m| m.layout.protocol_id == ANVIL_MENU_PROTOCOL_ID)
                    .map(|m| anvil_local(&mut self.screen, m, &items).field.clone());
                // M93u — the merchant's trade list, from the session's decoded
                // offers plus the screen's own scroll.
                let merchant = session
                    .menus
                    .open()
                    .filter(|m| {
                        m.layout.protocol_id
                            == rewo_world::merchant_screen::MERCHANT_MENU_PROTOCOL_ID
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
                );
                // M105 — the page counter is a LABEL, so it is counted here
                // rather than among the book's quads. Matched on the
                // model's own geometry (the counter is the only text the book
                // draws on that row) rather than on its content, which is a
                // translation and would tie the gate to a language.
                if let Some(c) = self.check.as_mut() {
                    let (_, bt, sc) = rewo_gpu::container::recipe_book_origin(sw, sh);
                    let row = bt
                        + rewo_world::recipe_book_screen::PAGE_LABEL_Y as f32 * sc;
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
                let overlay_up = self.screen.book_overlay.is_some();
                if let Some(c) = self.check.as_mut() {
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
                if let Some(h) = state.world_renderer.container_panel_height() {
                    let forced = self.screen_forced_open;
                    if let Some(c) = self.check.as_mut() {
                        c.container_frames += 1;
                        // FIRST sighting, not the last: M92 injects a
                        // second container late (a brewing stand, 166 tall
                        // like the player's), and r20's question is about the
                        // chest that proved the panel builder runs.
                        c.container_panel_h.get_or_insert(h);
                        c.container_overlays_max = c
                            .container_overlays_max
                            .max(state.world_renderer.container_panel_overlays());
                        if !forced {
                            c.container_self_opened_frames += 1;
                        }
                    }
                }
                // Any glyph laid out above may be new to the atlas. Sync
                // BEFORE the runs are drawn and outside the rendering scope --
                // it records a transfer, and a run referencing a rect that has
                // not reached the GPU samples whatever was there before.
                if let Some(cache) = self.glyphs.as_mut() {
                    if let Err(e) = state.world_renderer.sync_velvet_atlas(&mut state.gpu, cache) {
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
                apply_hand(
                    &mut state.world_renderer,
                    &mut state.gpu,
                    session,
                    &items,
                    h,
                    alpha,
                    sw / sh,
                );
            }
        }
        // M33: the cloud deck and this frame's precipitation. The assets are
        // built lazily because `baked` arrives with the session.
        if let Some(baked) = self.baked.as_ref() {
            if let Some(c) = self.check.as_mut() {
                c.weather_frames += 1;
            }
            let w = self.weather.get_or_insert_with(|| WeatherAssets::new(baked));
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
        let contextual = contextual_info(session);
        let survival = rewo_gpu::survival_hud::layout_for_screen(
            &resolve_survival_inputs(
                session,
                &self.etypes,
                self.started.elapsed().as_millis() as u64,
                contextual,
            ),
            extent.width as f32,
            extent.height as f32,
        );
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
            c.effect_icons_max = c.effect_icons_max.max(effects.len());
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
            c.leash_verts_max = c.leash_verts_max.max(state.world_renderer.leash_vert_count());
            if self.options_view == Some(rewo_world::options_screen::OptionsPage::Sound) {
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
                    .is_some_and(|s| s.kind == rewo_world::screen::ScreenKind::BookView)
                {
                    c.book_frames += 1;
                    c.book_pages_seen = c.book_pages_seen.max(book.page_count());
                }
            }
            c.jump_scale_max = c.jump_scale_max.max(session.jump_riding_scale());
            c.riding_jumps_sent = session.riding_jumps_sent();
        }
        state.world_renderer.set_hud(
            self.hotbar_slot,
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
            let (ix, _iy, iw, _ih) = rewo_world::chat_screen::input_rect(gui_w, gui_h);
            let env = rewo_world::chat_screen::SuggestionEnv {
                metrics: rewo_world::command_suggestions::InputMetrics {
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
            &rewo_world::chat::ChatOptions::default(),
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
                &rewo_world::chat::ChatOptions::default(),
            );
            if !bar.is_empty() {
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
                        if let Some(c) = self.check.as_mut() {
                            c.usage_box_frames += 1;
                        }
                    }
                    backdrops.extend(usage_fills.clone());
                }
            }
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
        if let Some(c) = self.check.as_mut() {
            if sidebar_lines > 0 {
                c.sidebar_frames += 1;
                c.sidebar_text_max = c.sidebar_text_max.max(sidebar_lines);
            }
        }
        let chat_range =
            (built_chat_range.start + sidebar_lines)..(built_chat_range.end + sidebar_lines);
        text.extend(built);
        if let Some(cs) = self.chat_screen.as_ref() {
            let (gw, gh) = ((extent.width as f32 / px) as i32, (extent.height as f32 / px) as i32);
            let advance = state.world_renderer.font_advance().copied();
            let width_of = move |s: &str| match &advance {
                Some(a) => rewo_gpu::text::width(s, a),
                None => 0,
            };
            if chat_runs.is_some() {
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
            if usage_text.iter().any(|l| {
                l.text.starts_with("Unknown command") && l.color_linear == [1.0, 1.0, 1.0]
            }) {
                if let Some(c) = self.check.as_mut() {
                    c.parse_error_frames += 1;
                }
            }
            text.extend(usage_text);
        }
        if !chat_rows.is_empty() {
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
                        l.color_linear.iter().all(|c| (c - GRAY_LINEAR).abs() < COLOR_EPS)
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
                &advance,
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
                &advance,
                px,
                (extent.width as f32, extent.height as f32),
            ));
            text.extend(title_lines(
                &session.hud.titles,
                &advance,
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
        text.extend(self.screen_labels.drain(..));
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
                chrome = crate::stats_view::chrome(
                    view,
                    screen,
                    Some(mouse_gui),
                    advance.as_ref().map(|a| &**a),
                );
                if let Some(advance) = advance {
                    text.extend(crate::stats_view::lines(view, screen, &advance, px));
                }
            } else if self.advancements.is_some()
                && self.screen.screens.current().is_some_and(|s| {
                    s.kind == rewo_world::screen::ScreenKind::Advancements
                })
            {
                // M178: the advancements window. Generic chrome carries the
                // transparent gradient + the Done button; our scissored
                // batches draw between the pass's head and its sprites
                // (vanilla: extractInside, then extractWindow), and our plain
                // sprites join `chrome.sprites` ahead of the buttons.
                let gw = (extent.width as f32 / px) as i32;
                let gh = (extent.height as f32 / px) as i32;
                let (win_x, win_y) =
                    rewo_world::advancements_screen::window_origin(gw, gh);
                // tick() runs here, contents-relative: the model owns the
                // hover clock and this is its only per-frame driver.
                if let Some(v) = self.advancements.as_mut() {
                    v.ensure_centered();
                    v.tick(Some((
                        mouse_gui.0 as i32 - win_x - rewo_world::advancements_screen::INSIDE_X,
                        mouse_gui.1 as i32 - win_y - rewo_world::advancements_screen::INSIDE_Y,
                    )));
                }
                if let Some(view) = self.advancements.as_ref() {
                    let mut adv = crate::advancements_view::chrome(view, gw, gh, gw);
                    if let Some(advance) = state.world_renderer.font_advance() {
                        text.extend(crate::advancements_view::lines(
                            view,
                            &self.lang,
                            gw,
                            gh,
                            gw,
                            px,
                            &advance,
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
                            &advance,
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
                for d in rewo_world::book_view_screen::draws(book, gw, Some(m)) {
                    chrome.sprites.push(book_sprite(d));
                }
                if let Some(advance) = state.world_renderer.font_advance() {
                    // The Done button's LABEL rides the generic widget-text
                    // builder; the first cut pushed only the page text and the
                    // eyeball pass found an unlabeled button (bookshot's
                    // witnesses probed chrome and page text, not the label —
                    // p9 counts changed pixels and the chrome alone passes it).
                    text.extend(screen_text_lines(screen, &advance, px));
                    text.extend(book_text_lines(book, gw, &advance, px, &self.lang));
                }
            } else if let (Some(view), Some(screen)) =
                (self.sign_edit.as_ref(), self.screen.screens.current())
            {
                // M174: the sign editor. Generic chrome carries the gradient +
                // the Done button; the board is a sprite blit riding
                // `chrome.sprites` (the book art's stratum — backdrop, then
                // board, then widgets/text), then selection/caret fills, then
                // the four centred lines.
                if let Some(c) = self.check.as_mut() {
                    c.sign_edit_frames += 1;
                }
                chrome = screen_chrome(screen, Some(mouse_gui));
                let gw = (extent.width as f32 / px) as i32;
                chrome
                    .sprites
                    .push(sign_board_sprite(view.state.kind, view.wood, gw));
                if let Some(advance) = state.world_renderer.font_advance() {
                    let (mut sign_text, fills) = sign_edit_draws(view, gw, &advance, px);
                    chrome.sprites.extend(fills);
                    text.append(&mut sign_text);
                    // The Done button's label rides the generic widget-text
                    // builder (M172's lesson: an unlabeled button passed a
                    // probe that only watched chrome).
                    text.extend(screen_text_lines(screen, &advance, px));
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
                chrome.sprites.extend(slider_sprites(
                    screen,
                    Some(mouse_gui),
                    self.options_drag,
                ));
                if let Some(advance) = state.world_renderer.font_advance() {
                    text.extend(screen_text_lines(screen, &advance, px));
                }
            } else if !matches!(self.view, ScreenView::None) {
                // M85's three screens. All of their text is on their widgets,
                // so one generic builder serves all three — the death screen
                // keeps its own only because its title, cause and score are
                // *not* widgets in vanilla either.
                if let Some(screen) = self.screen.screens.current() {
                    chrome = screen_chrome(screen, Some(mouse_gui));
                    if let Some(advance) = state.world_renderer.font_advance() {
                        text.extend(screen_text_lines(screen, &advance, px));
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
                    apply_gui_icons(
                        &mut state.world_renderer,
                        &mut state.gpu,
                        gui,
                        &items,
                    );
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
        // M52: resolve the module state once per frame. Every legit module
        // defaults off, so an unconfigured client produces exactly the
        // constants this path used before -- which is what keeps the golden
        // PNGs byte-identical.
        let render_modules = self.modules.render();
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
        let baked_live = self.baked.is_some();
        if let Some(c) = self.check.as_mut() {
            c.frames += 1;
            c.baked_frames += u64::from(baked_live);
            c.sample_rings(world_renderer);
            c.gui_items_ready |= world_renderer.gui_items_ready();
            c.hand_ready |= world_renderer.hand_ready();
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

        if let Some(limit) = self.run_seconds {
            if self.started.elapsed().as_secs_f32() >= limit {
                event_loop.exit();
            }
        }
    }
}
