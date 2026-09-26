use super::*;

pub(super) fn run_headless(
    mut session: PlaySession,
    baked: assets::BakedAssets,
    etypes: EntityTypes,
    spears: rewo_data::item_tags::ItemTag,
    // Chest block states → facing + material, for the M25b block-entity draws.
    chest_states: rewo_data::chest_states::ChestStates,
    sign_states: rewo_data::sign_states::SignStates,
    bow_item: Option<i32>,
    items: std::sync::Arc<rewo_data::items::Items>,
    beacon_effects: BeaconEffectIds,
    want_validation: bool,
    out: &std::path::Path,
    settle_seconds: f32,
    dirt_item: Option<i32>,
    pack: Option<PathBuf>,
    gamma: f32,
    darkness_option: f32,
    // M131 — the sound model. Silent by construction; see `build_sounds`.
    mut sounds: LiveSounds,
) -> Result<(), String> {
    let _ = dirt_item;
    let mut gpu = Gpu::new(None, want_validation)?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;
    let mut world_renderer =
        WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    let etf = init_entities_maybe_cem(&mut world_renderer, &mut gpu, &baked, &pack)?;
    // M22: the baked held-item models, converted across the crate seam.
    world_renderer.set_held_items(to_gpu_held_items(&baked.held_items));
    init_celestial_if_present(&mut world_renderer, &mut gpu, &baked)?;
    init_weather_if_present(&mut world_renderer, &mut gpu, &baked)?;
    init_particles_if_present(&mut world_renderer, &mut gpu, &baked)?;
    init_crumbling_if_present(&mut world_renderer, &mut gpu, &baked)?;
    let mut weather_assets = WeatherAssets::new(&baked);
    let mut gui_items = GuiItemState::new(&baked);
    let mut particle_assets = ParticleAssets::new(&baked);
    world_renderer.set_animations(layer_animations(&baked));
    if let Some(hud) = hud_sprites(&baked) {
        world_renderer.init_hud(&mut gpu, &hud)?;
    }
    let locator_styles = match locator_sprites(&baked) {
        Some(l) => {
            let styles = l.styles.clone();
            world_renderer.init_locator_bar(&mut gpu, &l)?;
            styles
        }
        None => Vec::new(),
    };
    if let Some(c) = container_sprites(&baked) {
        world_renderer.init_container(&mut gpu, &c)?;
    }
    if let Some(font) = font_data(&baked) {
        world_renderer.init_text(&mut gpu, &font)?;
    }

    // Pump the session on a real 20 Hz clock until spawned + settled, so
    // chunks arrive and the player position is real.
    let start = Instant::now();
    let idle = TickInput::default();
    let mut tick = 0u64;
    // M35 headless click state (`REWO_CLICK`).
    let mut clicked = false;
    let mut click_resyncs_at: Option<u32> = None;
    let mut summoned = false;
    // The block-light flicker (M13): a 48-bit LCG advanced exactly once per
    // successful 20 Hz client tick, mirroring `LightmapRenderStateExtractor`.
    let mut flicker = BlockLightFlicker::random();
    while start.elapsed().as_secs_f32() < settle_seconds {
        let deadline = start + Duration::from_millis(50) * (tick as u32 + 1);
        session.tick(&idle)?;
        flicker.tick();
        // M131 — one drain per client tick, beside the flicker, because
        // `SoundEngine.tick` is a *tick* clock (`MIN_SOURCE_LIFETIME` is 20 of
        // them) and driving it per frame would tie a channel's grace period to
        // the frame rate.
        let queued = session.take_sound_events();
        sounds.drive(
            &queued,
            &session.world.entities,
            session.local_player_view(),
            session.game_time(),
            session.music_volume(),
        );
        if let Some(reason) = &session.disconnect {
            return Err(format!("disconnected: {reason}"));
        }
        // REWO_SUMMON=mob: once spawned, /summon a mob ~3 blocks in front
        // (op required) so the model can be verified without a live one.
        if !summoned && session.spawned {
            // REWO_PRECMD: run one op command before the summon (e.g. clear
            // prior test mobs with `kill @e[type=husk]`), so a re-run starts
            // from a clean scene.
            if let Ok(cmd) = crate::knobs::var("REWO_PRECMD") {
                // Semicolon-separated, so a scene that needs several commands
                // (a `clear` then a handful of `give`s) is still one knob.
                for one in cmd.split(';').map(str::trim).filter(|c| !c.is_empty()) {
                    let _ = session.send_command(one);
                    log::info!("REWO_PRECMD: {one}");
                }
                if !cmd.is_empty() {
                    std::env::remove_var("REWO_PRECMD");
                }
            }
            if let Ok(mob) = crate::knobs::var("REWO_SUMMON") {
                let dir = look_dir(session.player.yaw, 0.0);
                let dist = crate::knobs::var("REWO_SUMMON_DIST")
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(3.0);
                // Optional vertical offset — float the mob into empty sky so a
                // verification shot isn't occluded by ground clutter.
                let dy: f64 = crate::knobs::var("REWO_SUMMON_DY")
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0.0);
                let (sx, sy, sz) = (
                    session.player.x + dir[0] * dist,
                    session.player.y + dy,
                    session.player.z + dir[2] * dist,
                );
                // Optional NBT tail (e.g. REWO_SUMMON_NBT={CustomName:'"Bo"'}).
                let nbt = crate::knobs::var("REWO_SUMMON_NBT").unwrap_or_default();
                let cmd = format!("summon minecraft:{mob} {sx:.2} {sy:.2} {sz:.2} {nbt}");
                if let Err(e) = session.send_command(&cmd) {
                    log::warn!("REWO_SUMMON: {e}");
                }
                log::info!("REWO_SUMMON: {cmd}");
                summoned = true;
            }
        }
        // REWO_CHAT: send a chat line once (verifies the chat overlay).
        //
        // `--render-check` supplies its own (M108) rather than making this a
        // third caller requirement beside r14's hotbar and r25's recipe book.
        // The server echoing the line back is what drives `player_chat`
        // through the signature cache, the trust level, the wrap and the
        // geometry, so the whole chain is exercised by the run itself.
        if summoned || session.spawned {
            if let Ok(msg) = crate::knobs::var("REWO_CHAT") {
                if !msg.is_empty() {
                    let _ = session.send_chat(&msg);
                    std::env::remove_var("REWO_CHAT");
                }
            }
        }
        // M35: `REWO_CLICK=<menu slot>[,<button>]` clicks one inventory slot
        // once the contents have arrived, then keeps ticking so the server's
        // answer lands before the frame is drawn. A rejected prediction comes
        // back as a whole-container update, which `inventory.content_updates`
        // counts — so this is a real end-to-end gate, not a "the packet was
        // written" claim.
        // Not the moment the first stack arrives: `/give` sends one container
        // update per item and each advances the server's state id, so a click
        // fired mid-give would echo a stale one and be resynced. Forty ticks
        // is two seconds of quiet.
        if !clicked && session.spawned && !session.inventory.is_empty() && tick >= 40 {
            // `REWO_DUMP_INVENTORY=1`: print every occupied slot once the
            // container has settled. The components are the point — a stack
            // that decoded at all proves the walk reached the end of its
            // patch, and the values prove it read the right bytes (M41).
            if crate::knobs::var("REWO_DUMP_INVENTORY").is_ok() {
                for i in 0..rewo_world::inventory::MENU_SLOTS {
                    if let Some(s) = session.inventory.menu_slot(i) {
                        println!(
                            "[rewo-m41] slot {i:2}: item {:4} x{:<3} components {:#018x}                              damage {:?} max_damage {:?} enchanted {}",
                            s.item_id, s.count, s.components, s.damage, s.max_damage, s.enchanted
                        );
                    }
                }
                std::env::remove_var("REWO_DUMP_INVENTORY");
            }
            if let Ok(whole) = crate::knobs::var("REWO_CLICK") {
              let before_all = session.inventory.content_updates();
              // Semicolon-separated, so a run can pick a stack up and then do
              // something with it — a drag needs a stack on the cursor, and no
              // single click can leave one there and use it.
              for spec in whole.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                // `d:<slot>,<slot>,…[,one]` is a quick-craft drag over those
                // slots; the trailing `one` selects type 1 (one per slot).
                if let Some(rest) = spec.strip_prefix("d:") {
                    let one = rest.trim_end().ends_with("one");
                    let kind = if one {
                        rewo_world::inventory::QUICK_CRAFT_ONE
                    } else {
                        rewo_world::inventory::QUICK_CRAFT_SPLIT
                    };
                    let touched: Vec<usize> = rest
                        .split(',')
                        .filter_map(|v| v.trim().parse::<usize>().ok())
                        .collect();
                    // M152: snapshot the three wire-derived smithing sets before the
                    // mutable borrows below. Owned, because a closure holding
                    // `session.recipes.as_ref()` cannot coexist with `shown_menu_mut()`.
                    let smithing = session
                        .recipes
                        .as_ref()
                        .map(rewo_net::recipe_book::SmithingSets::from_packet);
                    let props = |id: i32| item_props(&items, id, smithing.as_ref());
                    let accepted = session.inventory.quick_craft_accepts(&touched, kind, &props);
                    match session.shown_menu_mut().click_quick_craft(&accepted, kind, &props) {
                        Some(end) => {
                            use rewo_world::inventory::Inventory as Inv;
                            let input = rewo_world::inventory::CONTAINER_INPUT_QUICK_CRAFT;
                            let carried = session.inventory.carried();
                            let phase = |slot: i16, header: i32| {
                                rewo_world::inventory::ClickPrediction {
                                    slot,
                                    button: Inv::quick_craft_button(kind, header),
                                    changed: Vec::new(),
                                    carried,
                                }
                            };
                            let no_slot = rewo_world::inventory::QUICK_CRAFT_NO_SLOT;
                            let mut ok = session
                                .container_click_input(&phase(no_slot, 0), input)
                                .is_ok();
                            for &sl in &accepted {
                                ok = ok
                                    && session
                                        .container_click_input(&phase(sl as i16, 1), input)
                                        .is_ok();
                            }
                            if ok && session.container_click_input(&end, input).is_ok() {
                                session.shown_menu_mut().apply_prediction(&end);
                                println!(
                                    "[rewo-m35] DRAG over {accepted:?} type {kind}: \
                                     3 + {} packet(s), {} changed slot(s), carried {:?}",
                                    accepted.len(),
                                    end.changed.len(),
                                    session.inventory.carried()
                                );
                            } else {
                                println!("[rewo-m35] DRAG send failed");
                            }
                        }
                        None => println!("[rewo-m35] DRAG over {accepted:?}: not predictable"),
                    }
                    continue;
                }
                let mut parts = spec.split(',');
                let slot: i32 = parts.next().and_then(|v| v.trim().parse().ok()).unwrap_or(-1);
                let button: i8 = parts.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                // M152: snapshot the three wire-derived smithing sets before the
                // mutable borrows below. Owned, because a closure holding
                // `session.recipes.as_ref()` cannot coexist with `shown_menu_mut()`.
                let smithing = session
                    .recipes
                    .as_ref()
                    .map(rewo_net::recipe_book::SmithingSets::from_packet);
                let props = |id: i32| item_props(&items, id, smithing.as_ref());
                // `REWO_CLICK=<slot>,<button>[,<kind>]`, where `kind` selects
                // the `ContainerInput`: `q` quick-move, `s` swap (the button
                // is then an inventory index), `t` throw, `a` pickup-all.
                let kind = spec
                    .split(',')
                    .nth(2)
                    .map(|f| f.trim().to_string())
                    .unwrap_or_default();
                let (input, predicted) = match kind.as_str() {
                    "q" => (
                        rewo_world::inventory::CONTAINER_INPUT_QUICK_MOVE,
                        session.shown_menu_mut().click_quick_move(slot, &props),
                    ),
                    "s" => (
                        rewo_world::inventory::CONTAINER_INPUT_SWAP,
                        session.shown_menu_mut().click_swap(slot, button as i32, &props),
                    ),
                    "t" => (
                        rewo_world::inventory::CONTAINER_INPUT_THROW,
                        session.shown_menu_mut().click_throw(slot, button, &props),
                    ),
                    "a" => (
                        rewo_world::inventory::CONTAINER_INPUT_PICKUP_ALL,
                        session.shown_menu_mut().click_pickup_all(slot, button, &props),
                    ),
                    _ => (0, session.shown_menu_mut().click_pickup(slot, button, &props)),
                };
                // M93i — `CrafterScreen.slotClicked` runs its toggle BEFORE
                // the ordinary click and then falls through to it, so this is
                // additive: whatever it does, the click below still happens.
                let toggle = session.crafter_slot_click(slot, button, input);
                if toggle != rewo_world::menu::CrafterToggle::None {
                    println!("[rewo-m93i] CRAFTER slot {slot}: {toggle:?}");
                }
                match predicted {
                    Some(prediction) => {
                        match session.container_click_input(&prediction, input) {
                            Ok(()) => {
                                session.shown_menu_mut().apply_prediction(&prediction);
                                println!(
                                    "[rewo-m35] CLICK slot {slot} button {button} input \
                                     {input}: predicted {} changed slot(s), carried {:?}",
                                    prediction.changed.len(),
                                    session.inventory.carried()
                                );
                            }
                            Err(e) => println!("[rewo-m35] CLICK send failed: {e}"),
                        }
                    }
                    None => println!("[rewo-m35] CLICK slot {slot}: not predictable"),
                }
              }
              // One window over the whole sequence, so a drag's three packets
              // are graded together with the click that set it up.
              click_resyncs_at = Some(before_all);
              clicked = true;
              std::env::remove_var("REWO_CLICK");
            }
        }
        tick += 1;
        let now = Instant::now();
        if now < deadline {
            std::thread::sleep(deadline - now);
        }
    }
    if let Some(before) = click_resyncs_at {
        let after = session.inventory.content_updates();
        println!(
            "[rewo-m35] CLICK result: {} container resync(s) after the click — {}",
            after - before,
            if after == before {
                "the server accepted the prediction"
            } else {
                "REJECTED, the server re-sent the whole container"
            }
        );
    }
    if !session.spawned {
        return Err("never spawned".into());
    }
    // Mesh everything loaded — one shot, parallel across the rayon pool
    // (order-preserving; nothing mutates the world while this runs).
    for (cx, cz) in session.take_removed() {
        world_renderer.remove_column(&mut gpu, cx, cz);
    }
    let _ = session.take_dirty(); // superseded — we mesh every column below
    let mut coords = session.world.column_coords();
    coords.sort_unstable();
    let t0 = Instant::now();
    let outputs = rewo_mesh::pool::mesh_all_with(
        session.dimension_generation,
        &session.world,
        rewo_mesh::MeshInputs::from_baked(&baked),
        &coords,
    );
    let mut meshed = 0usize;
    for out in outputs {
        if let Some(mesh) = out.mesh {
            world_renderer.upload_column(
                &mut gpu,
                out.cx,
                out.cz,
                bytemuck::cast_slice(&mesh.vertices),
                &mesh.indices,
                bytemuck::cast_slice(&mesh.tvertices),
                &mesh.tindices,
                mesh.y_min,
                mesh.y_max,
            )?;
            meshed += 1;
        }
    }
    log::info!(
        "live: meshed {} of {} columns in {:.1} ms (parallel one-shot)",
        meshed,
        coords.len(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    gpu.wait_idle();

    // Look slightly down from the eye toward the horizon (or a debug
    // pitch) — unless REWO_LOOK_ENTITY=1, which aims at the nearest entity
    // so the verification PNG is guaranteed to frame it.
    let eye = player_eye(&session);
    // Single-shot render: a fresh tracker sees every gesture at age 0 (use
    // REWO_FORCE_GESTURE for a specific rig time).
    let mut gestures = GestureTracker::default();
    // Headless single-frame: skins can't finish fetching in time (use
    // `mobshot --skin` for a deterministic real-skin PNG). Empty registry.
    let skins = SkinRegistry::new();
    // Resolve ONE camera lightmap for this frame at a FIXED partial of 1.0,
    // matching vanilla `GameRenderer` (lines 376-386):
    // `lightmapRenderStateExtractor.extract(lightmapRenderState, 1.0F)`. Feed
    // the identical value to both the entity-light sampler and the renderer
    // uniform, so mobs and terrain share a lightmap.
    let partial = 1.0;
    let snapshot = session.visual_effect_snapshot(partial);
    // M157 — the headless path reads the same `options.txt` the windowed one
    // does, rather than defaulting: a gate that silently used different
    // options from the client would grade a configuration nobody runs.
    let headless_options = load_options();
    let lightmap = resolve_lightmap(
        session.day_ticks,
        session.active_dimension_type.as_ref(),
        flicker.block_factor(),
        snapshot,
        gamma,
        darkness_option,
        partial,
        end_flash_for(&session, partial, &headless_options),
    );
    // Drained before `collect_entities` takes its long-lived borrow of the
    // session; the particle spawn happens further down, once the renderer is
    // ready for it.
    let particle_events = std::mem::take(&mut session.particle_events);
    // M48: permute + upload any trim sprites this frame needs, before the
    // draws take their borrow of the session.
    let trim_slots = ensure_trims(&session, &items, &baked.trims, &mut gpu, &mut world_renderer);
    let draws = collect_entities(
        &session,
        &etypes,
        1.0,
        &mut gestures,
        0.0,
        &skins,
        &lightmap,
        &spears,
        bow_item,
        &items,
        &baked.equipment,
        &trim_slots,
        &etf,
        // The headless one-shot has no key handling, so the HUD is never
        // hidden. `REWO_HUD_HIDDEN=1` is the knob that lets a gate or a
        // scripted shot exercise F1's suppression without a keyboard.
        crate::knobs::var("REWO_HUD_HIDDEN").is_ok_and(|v| v.trim() == "1"),
        frame_crosshair_pick(&session, &etypes, 1.0),
    );
    for (id, e) in session.world.entities.iter() {
        let p = e.render_pos(1.0);
        println!(
            "[rewo-entities] #{id} {} at ({:.2},{:.2},{:.2}) yaw {:.0}{}",
            etypes.name(e.type_id).unwrap_or("?"),
            p[0],
            p[1],
            p[2],
            e.yaw,
            session
                .world
                .entities
                .name_of(e.uuid)
                .map(|n| format!(" name \"{n}\""))
                .unwrap_or_default(),
        );
    }
    println!("[rewo-entities] {} tracked", draws.len());
    let mut yaw = session.player.yaw;
    let mut pitch = crate::knobs::var("REWO_PITCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    // REWO_LOOK_AT="x,y,z": aim the camera at a fixed world point, bypassing
    // the entity search — deterministic framing for a summoned target even in
    // a scene full of other entities of the same kind.
    if let Some(pt) = crate::knobs::var("REWO_LOOK_AT").ok().and_then(|s| {
        let mut it = s.split(',');
        Some(Vec3::new(
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
        ))
    }) {
        let d = pt - Vec3::new(eye.x, eye.y, eye.z);
        let len = d.length().max(1e-4);
        yaw = (-d.x).atan2(d.z).to_degrees();
        pitch = (-d.y / len).asin().to_degrees();
        log::info!(
            "live: LOOK_AT {pt:?} from eye ({:.1},{:.1},{:.1}) -> yaw {yaw:.1} pitch {pitch:.1}",
            eye.x,
            eye.y,
            eye.z
        );
    } else if crate::knobs::var("REWO_LOOK_ENTITY").is_ok() {
        // Aim at the nearest interesting model: a player, or (REWO_LOOK=slime)
        // the nearest slime; else the nearest anything.
        let d = |e: &EntityDraw| {
            (e.pos[0] - eye.x).powi(2) + (e.pos[1] - eye.y).powi(2) + (e.pos[2] - eye.z).powi(2)
        };
        let look = crate::knobs::var("REWO_LOOK").ok();
        let look_kind = look
            .as_deref()
            .map(|s| rewo_gpu::mobs::kind_for_entity_name(&format!("minecraft:{s}")));
        let pref = |e: &&EntityDraw| match look_kind {
            Some(k) if k != EntityModelKind::Capsule => e.kind == k,
            _ => e.name.is_some(),
        };
        // REWO_LOOK_HIGH: among the preferred kind, take the highest one
        // (a floated summon sits above all ground clutter — deterministic).
        let high = crate::knobs::var("REWO_LOOK_HIGH").is_ok();
        let nearest = if high {
            draws
                .iter()
                .filter(pref)
                .max_by(|a, b| a.pos[1].partial_cmp(&b.pos[1]).unwrap())
        } else {
            draws
                .iter()
                .filter(pref)
                .min_by(|a, b| d(a).partial_cmp(&d(b)).unwrap())
        }
        .or_else(|| draws.iter().min_by(|a, b| d(a).partial_cmp(&d(b)).unwrap()));
        if let Some(t) = nearest {
            let d = Vec3::new(
                t.pos[0] - eye.x,
                t.pos[1] + t.height * 0.5 - eye.y,
                t.pos[2] - eye.z,
            );
            let len = d.length().max(1e-4);
            yaw = (-d.x).atan2(d.z).to_degrees();
            pitch = (-d.y / len).asin().to_degrees();
            log::info!(
                "live: aiming at entity ({:.1},{:.1},{:.1})",
                t.pos[0],
                t.pos[1],
                t.pos[2]
            );
        }
    }
    // Block targeting: aim the raycast the same way the camera looks, so the
    // selection outline lands on whatever block the eye view frames.
    let hit = session.target_block(eye_f64(&session), look_dir(yaw, pitch), REACH);
    if let Some(h) = hit {
        log::info!("live: targeting block {:?} face {:?}", h.block, h.face);
    }
    world_renderer.set_selection(hit.map(|h| h.block));
    world_renderer.set_leash(&collect_leashes(&session, &etypes, 1.0, &lightmap));
    let (cr, cu) = camera_basis(yaw, pitch);
    // Before `set_entities`: the entity pass reads the eye as the CEM
    // `player_pos_*`, which FA aims mob eyes/heads with.
    world_renderer.set_camera(eye.to_array());
    // Day/night + effects: push the one resolved lightmap plus the sky/fog tint.
    apply_lightmap(
        &mut world_renderer,
        &lightmap,
        session.day_ticks,
        session.active_dimension_type.as_ref(),
        {
            let w = effective_weather(&session);
            (w.rain_level(), w.thunder_level())
        },
        rain_fog_band(&session, &mut weather_assets, None),
    );
    apply_biome_sky_fog(&mut world_renderer, &session);
    // M33: the sun and moon fade out as rain comes in
    // (`SkyRenderState.rainBrightness`).
    let mut cel = celestial_state_of(session.day_ticks);
    apply_weather_to_celestial(&mut cel, &session);
    world_renderer.set_celestial(cel);
    world_renderer.set_end_flash(end_flash_render(&session, partial));
    apply_weather(
        &mut world_renderer,
        &mut gpu,
        &session,
        &mut weather_assets,
        1.0,
        None,
    );
    apply_border(&mut world_renderer, &mut gpu, &session, 1.0);
    // M35: `REWO_OPEN_INVENTORY=1` opens the screen for the headless shot, so
    // a PNG can show it without a windowed session and a keypress. The cursor
    // is parked at the window centre, which is where `set_screen_open` puts it.
    let mut headless_screen_labels: Vec<rewo_gpu::world::OwnedTextLine> = Vec::new();
    let (sw, sh) = (off.extent.width as f32, off.extent.height as f32);
    let screen_open = crate::knobs::var("REWO_OPEN_INVENTORY")
        .map(|v| v != "0")
        .unwrap_or(false);
    if screen_open {
        // `REWO_MOUSE=x,y` moves the cursor for the shot, which is the only way
        // to photograph the preview turning to follow it.
        let mouse = crate::knobs::var("REWO_MOUSE")
            .ok()
            .and_then(|v| {
                let (a, b) = v.split_once(',')?;
                Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
            })
            .unwrap_or((sw as f64 / 2.0, sh as f64 / 2.0));
        // `REWO_PREVIEW_SKIN=<username|url>` fetches one profile's textures
        // for the shot. Offline test servers carry no textures property, so
        // this is the only way to photograph the preview wearing a real skin
        // — or, since M64, a real cape.
        let mut skin = crate::knobs::var("REWO_PREVIEW_SKIN").ok().and_then(|spec| {
            let info = match crate::skin_fetch::resolve(&spec) {
                Ok(i) => i,
                Err(e) => {
                    log::warn!("preview: {spec}: {e}");
                    return None;
                }
            };
            // The two are independent: a profile may carry a cape and no
            // skin, exactly as `SkinLoader::request` treats them.
            let mut t = PreviewTextures::default();
            if let Some(url) = info.url.as_ref() {
                match crate::skin_fetch::fetch_rgba64(url) {
                    Ok(rgba) => {
                        log::info!(
                            "preview: skin {spec} ({} model)",
                            if info.slim { "slim" } else { "wide" }
                        );
                        t.skin = Some((rgba, info.slim));
                    }
                    Err(e) => log::warn!("preview: skin {spec}: {e}"),
                }
            }
            if let Some(url) = info.cape.as_ref() {
                match crate::skin_fetch::fetch_cape_rgba(url) {
                    Ok(rgba) => {
                        log::info!("preview: cape {spec}");
                        t.cape = Some(rgba);
                    }
                    Err(e) => log::warn!("preview: cape {spec}: {e}"),
                }
            }
            (t.skin.is_some() || t.cape.is_some()).then_some(t)
        });
        // The headless path takes no glyph cache: the gates' golden images
        // are graded against the bitmap tooltip, and swapping the typeface
        // under them would move every one of those pixels for a reason that
        // has nothing to do with what they test.
        let (labels, _velvet) = apply_screen(
            &mut world_renderer,
            &mut gpu,
            &session,
            &items,
            &mut gui_items,
            &baked,
            skin.as_mut(),
            None,
            // The headless screen renders the normal tooltip: the gates' golden
            // images grade what a default session shows, and F3+H is not it.
            rewo_gpu::tooltip::TooltipFlag::NORMAL,
            mouse,
            (sw, sh),
            beacon_effects,
            // The headless path drives no screen, so the menu is the answer.
            None,
            // …and it holds no scroll, so a stonecutter would draw its first
            // page. `run_headless` never opens one.
            None,
            // …nor an anvil, so there is no field to draw.
            None,
            // …nor a merchant.
            None,
            // …and the book's selection is its default, since nothing can
            // click it headlessly, with an empty search field.
            Default::default(),
            "",
            &rewo_world::edit_box::EditBox::new(
                rewo_world::recipe_book_screen::SEARCH_MAX_LENGTH,
            ),
            // Headless: a fixed clock, so a caret's blink cannot make the same
            // scene render two ways between runs.
            0,
            // Headless never opens the which-of-these overlay: nothing can
            // right-click a recipe cell without a cursor.
            None,
        );
        headless_screen_labels = labels;
    } else {
        apply_hotbar_icons(
            &mut world_renderer,
            &mut gpu,
            &session,
            &items,
            &mut gui_items,
            (sw, sh),
        );
    }
    if let Some(p) = particle_assets.as_mut() {
        let view = eye_view(eye, session.player.yaw, session.player.pitch).to_cols_array_2d();
        apply_particles(
            &mut world_renderer,
            &mut gpu,
            &session,
            particle_events,
            p,
            &baked,
            1.0,
            view,
        );
    }
    apply_crumbling(&mut world_renderer, &mut gpu, &session, &baked, eye);
    // M38: the first-person hand. `REWO_HAND_SWING=<0..1>` freezes the swing
    // partway for a shot — it is a tick clock, so a headless frame would
    // otherwise always catch it at rest.
    {
        let mut hand = HandState::new(&baked);
        // Settle the equip clock, or the item is caught mid-dip on tick one.
        hand.settle(&session, &items);
        hand.forced_attack = crate::knobs::var("REWO_HAND_SWING")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .map(|v| v.clamp(0.0, 1.0));
        apply_hand(
            &mut world_renderer,
            &mut gpu,
            &session,
            &items,
            &mut hand,
            1.0,
            sw / sh,
        );
    }
    let bes = collect_block_entities(&session.world, &chest_states, &lightmap, 1.0, session.game_time(), (cr, cu));
    // A spawner's caged mob rides the ENTITY pass, mounted inside its block
    // (M31), so it joins the entity draws rather than the block-entity ones.
    let caged = collect_spawner_mobs(
        &session.world,
        &etypes,
        chest_states.spawner_states(),
        &lightmap,
        1.0,
    );
    let portals = collect_end_portals(
        &session.world,
        chest_states.end_portal_states(),
        chest_states.end_gateway_states(),
    );
    world_renderer.set_end_portals(&mut gpu, &portals, session.game_time())?;
    let mut draws = draws;
    draws.extend(caged.iter().map(spawner_mob_draw));
    // M81: the stacks in flight to whoever picked them up. Appended to the
    // same list so they go through `prepare_held_items` below — a pickup's
    // item needs an atlas slot exactly as a dropped one does, and the entity
    // it came from has already left the table.
    draws.extend(collect_pickups(
        &session,
        &items,
        &lightmap,
        1.0,
        start.elapsed().as_secs_f32(),
    ));
    // Every texture the frame samples from the entity atlas — items in hands,
    // dropped stacks, and now block-entity models, which share the pool.
    let mut held: Vec<&str> = draws.iter().flat_map(|d| d.held).flatten().collect();
    held.extend(draws.iter().filter_map(|d| d.ground_item));
    held.extend(bes.iter().map(|b| b.model.as_str()));
    world_renderer.prepare_held_items(&mut gpu, &held)?;
    let be_draws: Vec<_> = bes.iter().map(|b| b.as_draw()).collect();
    let sign_lines = match world_renderer.font_advance() {
        Some(a) => collect_session_sign_text(&session, &sign_states, &lightmap, a),
        None => Vec::new(),
    };
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
    world_renderer.set_entities_and_block_entities(
        &draws,
        &be_draws,
        &sign_draws,
        cr,
        cu,
        start.elapsed().as_secs_f32(),
    );
    let contextual = contextual_info(&session);
    let survival = rewo_gpu::survival_hud::layout_for_screen(
        &resolve_survival_inputs(&mut session, &etypes, start.elapsed().as_millis() as u64, contextual),
        off.extent.width as f32,
        off.extent.height as f32,
    );
    world_renderer.set_hud(
        0,
        resolve_hud_gauges(
            &session.hud,
            &session.inventory,
            &items,
            // M169: the XP bar draws only when it OWNS the slot; the level
            // number is gated separately, on `hasExperience()` alone.
            has_experience(&session) && contextual == rewo_gpu::locator_bar::ContextualInfo::Experience,
            0.0,
        ),
        survival,
    );
    {
        let scale = rewo_gpu::hud::gui_scale(1280.0, 720.0);
        world_renderer.set_locator_bar(resolve_locator_bar(
            &session,
            &session.world.entities,
            &locator_styles,
            crate::modules::VANILLA_FOV,
            (1280.0 / scale) as i32,
            (720.0 / scale) as i32,
            0.0,
        ));
    }
    // Same ordering as the windowed path: drain the chat events before the
    // text is built, or a headless `--out` render shows an empty chat box for
    // messages the session has already decoded.
    apply_chat(&mut session, world_renderer.font_advance().copied());
    let (mut headless_text, _, _) = build_text(
        &session,
        gui_px(1280, 720),
        720.0,
        None,
        true,
        false,
        world_renderer.font_advance().copied(),
    );
    headless_text.extend(headless_screen_labels);
    world_renderer.set_text(headless_text);
    world_renderer.anim_tick(&mut gpu, session.ticks)?;
    let vp = eye_view_proj(eye, yaw, pitch, 1280.0 / 720.0, crate::modules::VANILLA_FOV);
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [16.0, 16.0],
        size: [560.0, 140.0],
    };
    for _ in 0..3 {
        off.render(&gpu, Some((&mut world_renderer, vp)), &draw, CLEAR_SKY)?;
    }
    off.save_png(&gpu, out)?;
    // M16 diagnostic: which dimension the frame was actually resolved from, and
    // the three values that drive it. Printed once, not per frame — enough to
    // tell a Nether frame from an Overworld one in a headless log, without
    // asking anyone to look at the PNG.
    println!(
        "[rewo-m16] dimension: {} skybox {:?} (end_sky asset {}) ambient {:?} sky_light {:?} x{:.3}",
        session
            .active_dimension_type
            .as_ref()
            .map(|d| d.name.as_str())
            .unwrap_or("<unresolved>"),
        world_renderer.sky_mode(),
        if world_renderer.end_sky_ready() { "present" } else { "MISSING" },
        lightmap.ambient_color,
        lightmap.sky_light_color,
        lightmap.sky_factor,
    );
    let total = world_renderer.column_count();
    let gpu_drawn = world_renderer.read_draw_count(&mut gpu);
    println!(
        "[rewo-m3-live] headless: spawned at ({:.1},{:.1},{:.1}), {} columns loaded, GPU cull drew {} of {} ({} culled on GPU)",
        session.player.x,
        session.player.y,
        session.player.z,
        session.world.loaded_columns(),
        gpu_drawn,
        total,
        total as i64 - gpu_drawn as i64,
    );
    println!("[rewo-m3-live] wrote {}", out.display());
    world_renderer.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

// -- windowed ---------------------------------------------------------------
