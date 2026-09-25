use super::*;

impl PlaySession {
    pub(super) fn handle_packet(&mut self, id: i32, body: &[u8]) -> Result<(), String> {
        let ids = &self.ids;
        // Resolved before the ladder because the `take_item_entity` arm below
        // borrows `self.world` mutably and cannot read `self.player` too.
        let local_collector = self.local_collector();
        if Some(id) == ids.cb_play_start_configuration {
            // A proxy's server switch (Velocity/Bungee), or a datapack reload
            // that needs new registries. Only reached outside a bundle — the
            // bundler treats it as the terminal packet it is.
            self.start_reconfiguration()?;
        } else if id == ids.cb_play_keep_alive {
            // Unanswerable means a kick in 15 s; fail now with the cause.
            let v = PacketReader::new(body)
                .i64()
                .map_err(|e| format!("play keep_alive: {e}"))?;
            let mut p = PacketWriter::packet(self.ids.sb_play_keep_alive);
            p.i64(v);
            self.send(p)?;
        } else if id == ids.cb_play_ping {
            let v = PacketReader::new(body)
                .i32()
                .map_err(|e| format!("play ping: {e}"))?;
            let mut p = PacketWriter::packet(self.ids.sb_play_pong);
            p.i32(v);
            self.send(p)?;
        } else if id == ids.cb_play_resource_pack_push {
            // M166. Unlike its configuration twin this blocks nothing — there
            // is no task queue in play — but vanilla answers it from the same
            // `ClientCommonPacketListenerImpl.handleResourcePackPush`, so Rewo
            // answers it from the same `answer_pack_push`. A server that
            // swaps packs per-world pushes here.
            let (pack_id, action) =
                crate::config_tasks::answer_pack_push(body, &mut self.config_tasks);
            self.send(crate::config_tasks::write_pack_reply(
                self.ids.sb_play_resource_pack,
                pack_id,
                action,
            ))?;
        } else if id == ids.cb_play_position {
            self.apply_teleport(body)?;
        } else if id == ids.cb_play_player_rotation || id == ids.cb_play_player_look_at {
            // M76. An arm of its own rather than a `route_*` tail call because
            // one of the two answers the server immediately and the other does
            // not — see `apply_player_rotation`.
            self.apply_player_rotation(id, body)?;
        } else if id == ids.cb_play_chunk_batch_start {
            // M74. Empty body — `handleChunkBatchStart` is one call, and it is
            // the half that makes the reply below adaptive instead of a
            // differently-wrong constant.
            let now = self.now_nanos();
            self.chunk_batch.on_batch_start(now);
        } else if id == ids.cb_play_chunk_batch_finished {
            // M74. Was `p.f32(64.0)` — an ~18x over-bid against vanilla's
            // seeded opening 3.5, on every batch of every session, never
            // adapting. `batchSize` is a VarInt, and a body that fails to
            // decode still gets a reply: vanilla always answers, and going
            // silent here would stall the server's chunk pipeline outright.
            let now = self.now_nanos();
            match crate::chunk_batch::read_chunk_batch_finished(body) {
                Ok(batch_size) => self.chunk_batch.on_batch_finished(batch_size, now),
                Err(err) => self.decode_failed("chunk_batch_finished", err),
            }
            let mut p = PacketWriter::packet(self.ids.sb_play_chunk_batch_received);
            p.f32(self.chunk_batch.desired_chunks_per_tick());
            self.send(p)?;
        } else if id == ids.cb_play_level_chunk {
            let shape = self.world.shape;
            let mut r = PacketReader::new(body);
            match rewo_world::chunk::read_level_chunk_bits2(
                &mut r,
                &shape,
                self.global_bits,
                self.biome_global_bits,
            ) {
                Ok(col) => {
                    let (cx, cz) = (col.cx, col.cz);
                    self.world.insert_column(cx, cz, col);
                    // New column changes its own + its neighbors' edge faces.
                    self.mark_dirty_around(cx, cz);
                }
                Err(e) => {
                    // Counted, not just logged: a decode failure here is what a
                    // wrong vertical shape looks like from the outside, so the
                    // count is the meter for "we are decoding chunks against
                    // the dimension we are actually in".
                    self.chunk_decode_failures = self.chunk_decode_failures.wrapping_add(1);
                    log::error!("play: chunk decode failed: {e}");
                }
            }
        } else if Some(id) == ids.cb_play_chunks_biomes {
            // Biomes changed for already-loaded chunks (`/fillbiome`, worldgen
            // re-send). Body: a list of {ChunkPos (packed long), VarInt-length
            // byte array of per-section biome containers}. Replace the loaded
            // column's biome palettes and remesh the 3×3 (tint reads neighbors).
            let shape = self.world.shape;
            let biome_bits = self.biome_global_bits;
            let mut r = PacketReader::new(body);
            if let Ok(n) = r.count("chunk biomes", 12) {
                for _ in 0..n {
                    let Ok(packed) = r.i64() else { break };
                    let (cx, cz) = (packed as i32, (packed >> 32) as i32);
                    // Cap = ClientboundChunksBiomesPacket's TWO_MEGABYTES.
                    let Ok(buf) = r.byte_array(2_097_152) else {
                        break;
                    };
                    let mut br = PacketReader::new(buf);
                    match rewo_world::chunk::read_chunk_biomes(&mut br, &shape, biome_bits) {
                        Ok(containers) => {
                            self.world.apply_chunks_biomes(cx, cz, containers);
                            self.mark_dirty_around(cx, cz);
                        }
                        Err(e) => self.decode_failed("chunks_biomes", e),
                    }
                }
            } else {
                self.decode_failed("chunks_biomes", "malformed body");
            }
        } else if Some(id) == ids.cb_play_light_update {
            // Lighting changed without a chunk resend (torch placed, cave
            // mined into). Without this the client's light is frozen at
            // chunk-load, so a freshly lit cave stays black.
            let shape = self.world.shape;
            let mut r = PacketReader::new(body);
            match (r.varint(), r.varint()) {
                (Ok(cx), Ok(cz)) => {
                    if let Some(col) = self.world.column_mut(cx, cz) {
                        if let Err(e) = rewo_world::chunk::apply_light_update(&mut r, col) {
                            log::error!("play: light_update decode failed: {e}");
                        } else {
                            // Re-light means re-mesh: vertex colours bake the
                            // light in, so the neighbourhood must remesh too.
                            self.mark_dirty_around(cx, cz);
                        }
                    }
                    let _ = shape;
                }
                _ => log::error!("play: light_update: bad chunk coords"),
            }
        } else if id == ids.cb_play_forget_chunk {
            let mut r = PacketReader::new(body);
            if let Ok(v) = r.i64() {
                let (cx, cz) = (v as i32, (v >> 32) as i32);
                self.world.forget_column(cx, cz);
                self.dirty.remove(&(cx, cz));
                self.removed.push((cx, cz));
            } else {
                self.decode_failed("forget_level_chunk", "malformed body");
            }
        } else if id == ids.cb_play_block_update {
            let mut r = PacketReader::new(body);
            if let (Ok((x, y, z)), Ok(state)) = (r.position(), r.varint()) {
                let old = self.world.block_state_at(x, y, z);
                self.world.set_block(x, y, z, state as u32);
                self.relight(x, y, z, old, state as u32);
                self.block_updates += 1;
                self.mark_dirty_block(x, z);
                log::debug!("net: block_update ({x},{y},{z}) = {state}");
            } else {
                self.decode_failed("block_update", "malformed body");
            }
        } else if crate::route_block_event(
            id,
            body,
            ids,
            self.block_event_types,
            self.game_time.unwrap_or(0),
            &mut self.world,
        ) {
            // `ClientboundBlockEventPacket` — a container's viewer count, which
            // is what drives a chest's lid and a shulker box's lid. Which of
            // the two (or neither — a bell's ring is also `b0 == 1`) is
            // selected by the block entity's own type, exactly as vanilla's
            // virtual `triggerEvent` call is.
        } else if crate::route_block_entity_data(id, body, ids, &mut self.world) {
            // `ClientboundBlockEntityDataPacket` (M25) — one block entity's
            // update tag:
            //
            //     BlockPos.STREAM_CODEC                       // packed long
            //     ByteBufCodecs.registry(BLOCK_ENTITY_TYPE)   // VarInt raw id
            //     ByteBufCodecs.TRUSTED_COMPOUND_TAG          // network NBT
            //
            // `registry(...)` writes the id **raw**, like the dimension holder
            // M16 had to correct — not the `id + 1` inline scheme.
        } else if id == ids.cb_play_section_blocks_update {
            // Multi-block change within one 16³ section — what the server
            // sends for a `/fill`, an explosion, a piston, or a growing tree.
            // Without this, any edit to an already-loaded chunk is invisible:
            // single-block edits arrive as `block_update`, everything else
            // arrives here.
            //
            // Body (ClientboundSectionBlocksUpdatePacket): a packed section
            // position long, a VarInt count, then one VarLong per change
            // holding `stateId << 12 | posInSection`.
            let mut r = PacketReader::new(body);
            if let (Ok(section), Ok(count)) = (r.u64(), r.varint()) {
                let (sx, sy, sz) = unpack_section_pos(section);
                let mut applied = 0;
                let mut changes = Vec::new();
                for _ in 0..count.max(0) {
                    let Ok(packed) = r.varlong() else { break };
                    let packed = packed as u64;
                    let state = (packed >> 12) as u32;
                    let (ox, oy, oz) = unpack_section_offset((packed & 4095) as i32);
                    let (x, y, z) = (sx * 16 + ox, sy * 16 + oy, sz * 16 + oz);
                    let old = self.world.block_state_at(x, y, z);
                    self.world.set_block(x, y, z, state);
                    changes.push((x, y, z, old, state));
                    self.mark_dirty_block(x, z);
                    applied += 1;
                }
                // One light pass for the whole section, after every state is
                // in place (a /fill or explosion relit per block was O(n) floods).
                self.relight_batch(&changes);
                self.block_updates += applied;
                log::debug!("net: section_blocks_update ({sx},{sy},{sz}) × {applied}");
            } else {
                self.decode_failed("section_blocks_update", "malformed body");
            }
        } else if id == ids.cb_play_set_time {
            // 26.x replaced the old `(worldAge, timeOfDay)` pair with a game
            // time plus a map of per-clock states — the day/night cycle is now
            // a timeline over a registered `WorldClock`, not a hard-coded
            // formula. Body: `i64 gameTime`, then a VarInt-counted map of
            // `Holder<WorldClock>` → `{VarLong totalTicks, f32 partial,
            // f32 rate}`.
            //
            // `MinecraftServer.forceGameTimeSynchronization` broadcasts
            // `SetTime(gameTime, Map.of())` every 20 ticks with an *empty* map;
            // the client is expected to run each stored clock forward itself
            // (`ClientClockManager.tick`). Only a real change (join, `/time`
            // set) carries an explicit clock state. Holding the last total on
            // an empty map — the previous behaviour — froze the celestials.
            let mut r = PacketReader::new(body);
            if let (Ok(game_time), Ok(count)) = (r.i64(), r.varint()) {
                // A vanilla server sends BOTH the overworld and the_end
                // clocks, so entries are matched by id — the key is a raw
                // registry id (`ByteBufCodecs.holderRegistry` writes it plain;
                // the `id + 1` / direct-holder scheme belongs to a different
                // codec).
                let mut entries: Vec<(i32, i64, f32, f32)> = Vec::new();
                for _ in 0..count.max(0) {
                    let (holder, total, partial, rate) =
                        (r.varint(), r.varlong(), r.f32(), r.f32());
                    let (Ok(holder), Ok(total), Ok(partial), Ok(rate)) =
                        (holder, total, partial, rate)
                    else {
                        break;
                    };
                    entries.push((holder, total, partial, rate));
                }
                // `ClientLevel.setGameTime`: the server's game time is
                // authoritative. The per-tick local increment continues from
                // here, and because this re-anchors `game_time` (and the clock's
                // `last_game_time` via the advance below) the same client tick's
                // local `+1` is not double-counted.
                self.game_time = Some(game_time);
                self.clocks.handle_updates(game_time, &entries);
                // Use the ported clock's total once it exists; before any real
                // clock state, fall back to `gameTime` (best-effort for a
                // server that never registers one).
                self.day_ticks = Some(
                    match self.overworld_clock_id.and_then(|id| self.clocks.peek(id)) {
                        Some(clock) => clock.total,
                        None => game_time,
                    },
                );
                log::debug!(
                    "net: set_time game={game_time} clocks={count} day_ticks={:?}",
                    self.day_ticks
                );
            } else {
                self.decode_failed("set_time", "malformed body");
            }
        } else if id == ids.cb_play_explode {
            // M68 decoded the physics prefix; M162 walks the tail for the
            // sound. **Two entry points on purpose**: `read_explode` cannot
            // fail on tail content, so an untranscribed particle type costs
            // the sound and never the knockback.
            match crate::motion::read_explode(body) {
                Ok((e, _used)) => {
                    // Vanilla's order, and it is the order of the RNG draws as
                    // much as of the effects: `handleExplosion` plays the
                    // sound FIRST (three draws off `Level.random`), then the
                    // particle, then the tracker, and applies the knockback
                    // LAST (`ClientPacketListener.java:1357-1375`).
                    self.queue_explosion_sound(body, &e);
                    self.apply_explode(&e);
                }
                Err(err) => self.decode_failed("explode", err),
            }
        } else if id == ids.cb_play_set_entity_motion {
            match crate::motion::read_set_entity_motion(body) {
                Ok(m) => self.apply_set_entity_motion(&m),
                Err(err) => self.decode_failed("set_entity_motion", err),
            }
        } else if id == ids.cb_play_move_vehicle {
            match crate::motion::read_move_vehicle(body) {
                Ok(v) => self.apply_move_vehicle(&v),
                Err(err) => self.decode_failed("move_vehicle", err),
            }
        } else if Some(id) == ids.cb_play_level_particles
            || Some(id) == ids.cb_play_level_event
        {
            // M37. Both packets feed the same queue; the renderer drains it
            // and owns the actual spawning, because the particle system needs
            // the block shapes and the RNG that live on that side.
            let ev = if Some(id) == ids.cb_play_level_particles {
                crate::route_level_particles(body, &self.particle_types)
            } else {
                crate::route_level_event(body)
            };
            if let Some(ev) = ev {
                log::debug!("net: particle event {ev:?}");
                self.particle_events.push(ev);
            }
            // M140 — the same packet also asks for a sound, and the two are
            // independent: 2001 (a block breaking) is both, 1000 (a dispenser)
            // is sound only, and 2000 (smoke) is particle only. Deriving one
            // from the other would lose whichever the id does not have.
            if Some(id) == ids.cb_play_level_event {
                // M162 — the camera is resolved HERE, at packet time, because
                // `handleLevelEvent` calls `globalLevelEvent` synchronously.
                // See `route_level_event_sound`'s docs for why this seam and
                // not the engine's.
                let camera = Self::camera_eye(&self.player, self.spawned);
                if let Some(s) = crate::route_level_event_sound(body, camera) {
                    self.push_sound_event(s);
                }
            }
        } else if id == ids.cb_play_sound
            || id == ids.cb_play_sound_entity
            || id == ids.cb_play_stop_sound
        {
            // M63 — decode only. The three bodies differ enough that the kind
            // has to come from the id; deriving it from the body would mean
            // guessing between a var-int entity id and a fixed i32 position.
            let kind = if id == ids.cb_play_sound {
                crate::SoundPacketKind::Positioned
            } else if id == ids.cb_play_sound_entity {
                crate::SoundPacketKind::OnEntity
            } else {
                crate::SoundPacketKind::Stop
            };
            if let Some(ev) = crate::route_sound(kind, body) {
                log::debug!("net: sound event {ev:?}");
                self.push_sound_event(ev);
            }
        } else if Some(id) == ids.cb_play_block_ack {
            // Sequence ack — server confirms our predicted change. We don't
            // predict yet (M3 applies the server's block_update), so this is
            // just observed for the parity meter.
            log::debug!("net: block_changed_ack");
        } else if id == ids.cb_play_login {
            self.apply_login_shape(body)?;
            let p = PacketWriter::packet(self.ids.sb_play_player_loaded);
            self.send(p)?;
        } else if id == ids.cb_play_respawn {
            self.apply_respawn(body)?;
        } else if id == ids.cb_play_update_mob_effect {
            self.visual_effects.apply_update(body);
            // The same packet also carries any entity's haste / conduit power /
            // mining fatigue, which change how long its swing runs (M19).
            crate::apply_swing_effect(
                body,
                &mut self.world.entities,
                self.swing_effect_ids,
                true,
                self.entity_classes.as_deref(),
            );
        } else if id == ids.cb_play_remove_mob_effect {
            self.visual_effects.apply_remove(body);
            crate::apply_swing_effect(
                body,
                &mut self.world.entities,
                self.swing_effect_ids,
                false,
                self.entity_classes.as_deref(),
            );
        } else if id == ids.cb_play_add_entity {
            let mut r = PacketReader::new(body);
            if let Ok((eid, type_id)) = crate::read_add_entity(&mut r, &mut self.world) {
                self.post_add_entity_sound_instance(eid, type_id);
            } else {
                self.decode_failed("add_entity", "malformed body");
            }
        } else if id == ids.cb_play_remove_entities {
            let mut r = PacketReader::new(body);
            if let Ok(n) = r.count("remove entities", 1) {
                for _ in 0..n {
                    if let Ok(eid) = r.varint() {
                        self.world.entities.remove(eid);
                        // M68: a removed entity cannot still be riding or be
                        // ridden. Leaving the seat behind would strand the
                        // local player "mounted" on a vehicle that no longer
                        // exists, which suppresses its physics forever.
                        self.mounts.remove_entity(eid);
                    }
                }
            } else {
                self.decode_failed("remove_entities", "malformed body");
            }
        } else if id == ids.cb_play_move_entity_pos {
            let mut r = PacketReader::new(body);
            if let Ok((eid, dx, dy, dz)) = read_move_delta(&mut r) {
                if let Some(e) = self.world.entities.get_mut(eid) {
                    e.nudge(dx, dy, dz);
                }
            } else {
                self.decode_failed("move_entity_pos", "malformed body");
            }
        } else if id == ids.cb_play_move_entity_pos_rot {
            let mut r = PacketReader::new(body);
            let parse = (|| -> rewo_proto::Result<(i32, f64, f64, f64, f32, f32)> {
                let (eid, dx, dy, dz) = read_move_delta(&mut r)?;
                let yaw = packed_degrees(r.i8()?);
                let pitch = packed_degrees(r.i8()?);
                Ok((eid, dx, dy, dz, yaw, pitch))
            })();
            if let Ok((eid, dx, dy, dz, yaw, pitch)) = parse {
                if let Some(e) = self.world.entities.get_mut(eid) {
                    e.nudge(dx, dy, dz);
                    e.set_rot(yaw, pitch);
                }
            } else {
                self.decode_failed("move_entity_pos_rot", "malformed body");
            }
        } else if id == ids.cb_play_move_entity_rot {
            let mut r = PacketReader::new(body);
            let parse = (|| -> rewo_proto::Result<(i32, f32, f32)> {
                Ok((
                    r.varint()?,
                    packed_degrees(r.i8()?),
                    packed_degrees(r.i8()?),
                ))
            })();
            if let Ok((eid, yaw, pitch)) = parse {
                if let Some(e) = self.world.entities.get_mut(eid) {
                    e.set_rot(yaw, pitch);
                }
            } else {
                self.decode_failed("move_entity_rot", "malformed body");
            }
        } else if id == ids.cb_play_entity_position_sync {
            // varint id, PositionMoveRotation {pos 3×f64, vel 3×f64, yaw
            // f32, pitch f32}, bool on_ground.
            let mut r = PacketReader::new(body);
            let parse = (|| -> rewo_proto::Result<(i32, [f64; 3], f32, f32)> {
                let eid = r.varint()?;
                let pos = [r.f64()?, r.f64()?, r.f64()?];
                let _vel = [r.f64()?, r.f64()?, r.f64()?];
                Ok((eid, pos, r.f32()?, r.f32()?))
            })();
            if let Ok((eid, pos, yaw, pitch)) = parse {
                if let Some(e) = self.world.entities.get_mut(eid) {
                    e.set_target(pos[0], pos[1], pos[2]);
                    e.set_rot(yaw, pitch);
                }
            } else {
                self.decode_failed("entity_position_sync", "malformed body");
            }
        } else if id == ids.cb_play_teleport_entity {
            // varint id, PositionMoveRotation, i32 relative-bits, bool
            // on_ground — same Relative order as the player teleport
            // (X=0 Y=1 Z=2 Y_ROT=3 X_ROT=4; velocity deltas 5..7 ignored).
            let mut r = PacketReader::new(body);
            let parse = (|| -> rewo_proto::Result<(i32, [f64; 3], f32, f32, i32)> {
                let eid = r.varint()?;
                let pos = [r.f64()?, r.f64()?, r.f64()?];
                let _vel = [r.f64()?, r.f64()?, r.f64()?];
                let yaw = r.f32()?;
                let pitch = r.f32()?;
                Ok((eid, pos, yaw, pitch, r.i32()?))
            })();
            if let Ok((eid, pos, yaw, pitch, relatives)) = parse {
                if let Some(e) = self.world.entities.get_mut(eid) {
                    let rel = |bit: i32| relatives & (1 << bit) != 0;
                    let (tx, ty, tz) = (
                        if rel(0) { e.x + pos[0] } else { pos[0] },
                        if rel(1) { e.y + pos[1] } else { pos[1] },
                        if rel(2) { e.z + pos[2] } else { pos[2] },
                    );
                    e.set_target(tx, ty, tz);
                    let yaw = if rel(3) { e.yaw + yaw } else { yaw };
                    let pitch = if rel(4) { e.pitch + pitch } else { pitch };
                    e.set_rot(yaw, pitch);
                }
            } else {
                self.decode_failed("teleport_entity", "malformed body");
            }
        } else if id == ids.cb_play_rotate_head {
            // varint id, yHeadRot (packed-degree byte). The server steers the
            // head toward nearby players, so this is what makes a mob watch you.
            let mut r = PacketReader::new(body);
            match (r.varint(), r.i8()) {
                (Ok(eid), Ok(b)) => {
                    if let Some(e) = self.world.entities.get_mut(eid) {
                        e.set_head_yaw(packed_degrees(b));
                    }
                }
                _ => self.decode_failed("rotate_head", "malformed body"),
            }
        } else if crate::route_set_entity_data(
            id,
            body,
            ids,
            &mut self.world.entities,
            crate::MetaKinds {
                allay: self.allay_type_id,
                pillager: self.pillager_type_id,
                sheep: self.sheep_type_id,
                creaking: self.creaking_type_id,
                player: self.player_type_id,
                bee: self.bee_type_id,
                guardian: self.guardian_type_id,
                elder_guardian: self.elder_guardian_type_id,
                variant_kinds: self.variant_type_ids,
                classes: self.entity_classes.as_deref(),
                components: self.swing_data.as_ref().map(|d| d.components),
                // The nametag's language table. Resolving here rather than at
                // render is what keeps `EntityDraw::name` a borrowed `&str`;
                // see `MetaKinds::lang`.
                lang: self.lang.as_deref(),
            },
        ) {
            // M141e: and the local player's own, which the router cannot store.
            // `handleSetEntityData` is `if (entity != null)` and vanilla's
            // local player IS in the level, so the server's metadata for you
            // is processed like anyone else's — but `EntityTable` has no row
            // for you, so the router returns early on your id and drops it.
            // Same asymmetry M73 hit with attributes, same fix: decode the
            // body a second time when it names the camera entity.
            self.capture_local_metadata(body);
            // Entity metadata (custom name, pose, gesture state, cube size, and
            // the polymorphic index-16 BOOLEAN → Allay dancing / baby). The
            // Allay dance counters then advance in `tick_lerp`.
            //
            // M82: and the local player's own, which the line above cannot
            // store for the reason M73 records two arms down — the entity
            // table has no row for you. `ServerEntity.sendChanges` broadcasts
            // through `sendToTrackingPlayersAndSelf`, so the packet really
            // does arrive.
            crate::apply_local_player_score(body, self.player_id, &mut self.score);
        } else if crate::route_damage_event(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M21: the damage response — arms the hurt clock (red overlay) and
            // kicks the walk animation, for a tracked living entity only.
            //
            // M168: and the local player's own `invulnerableTime = 20`
            // (`LivingEntity.handleDamageEvent`, `:2044-2048`), which the
            // line above cannot store for M73's reason — the table has no
            // row for you. The body opens with the entity id (VarInt).
            let mut r = PacketReader::new(body);
            if let (Ok(eid), Some(me)) = (r.varint(), self.player_id) {
                if eid == me {
                    self.hud.local_hurt.damage_event();
                }
            }
        } else if crate::route_hurt_animation(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
            self.player_type_id,
            self.player_id,
        ) {
            // M81: `damage_event`'s twin. It arms the same clock and, for a
            // player only, stores the yaw the camera tilt leans away from —
            // the one thing `damage_event` never carries.
        } else if crate::route_player_combat_kill(
            id,
            body,
            ids,
            self.player_id,
            &mut self.death,
        ) {
            // M82: you died. The id is always your own, so this is resolved
            // against the local-player door and never against the entity
            // table — `REWO_PLAN.md` §0.0 gotcha 13.
            //
            // `handlePlayerCombatKill`'s own branch, transcribed: with the
            // death screen suppressed the client respawns *immediately* and
            // never records a death at all. Nothing downstream then has to
            // know the rule.
            match crate::death_action(self.death.take(), self.game_state.show_death_screen()) {
                crate::DeathAction::ShowScreen(kill) => self.death = Some(kill),
                crate::DeathAction::RespawnNow => self.perform_respawn()?,
                crate::DeathAction::None => {}
            }
        } else if crate::route_award_stats(id, body, ids, &mut self.awarded_stats) {
            // M84: your own statistics, in reply to a `REQUEST_STATS` this
            // client sent when the screen opened. `setValue`, not `increment`,
            // and the map is never cleared — see `StatsCounter::apply`.
            if let Some(pairs) = self.awarded_stats.take() {
                self.stats.apply(&pairs);
            }
        } else if crate::route_block_destruction(
            id,
            body,
            ids,
            &mut self.world.destruction,
            self.game_time.unwrap_or(0),
        ) {
            // M81: somebody else's mining progress. The stage byte is
            // unsigned, and anything outside 0..10 retires the record.
        } else if crate::route_take_item_entity(
            id,
            body,
            ids,
            &mut self.world,
            crate::TakeItemKinds {
                local_player: local_collector,
                ..self.take_item_kinds
            },
        ) {
            // M81: the pickup animation, and the *client-side* removal of the
            // collected entity — this packet is not a heads-up that a
            // `remove_entities` is coming, it is the removal.
        } else if crate::route_update_attributes(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
            self.entity_types.as_deref(),
            self.attribute_registry.as_deref(),
        ) {
            // M52: entity attribute snapshots — max health and the rest. Each
            // snapshot replaces one attribute's base + modifiers, filtered by
            // the entity type's `AttributeSupplier`.
            //
            // M73: and the local player's own, which the line above cannot
            // store. `handleUpdateAttributes` looks the entity up in the level
            // and the local player is in it; Rewo's `EntityTable` holds only
            // entities the server sent an `add_entity` for, and it never sends
            // one for you. So the same body is decoded a second time and kept
            // beside the table when it names the camera entity — without it
            // `entity_interaction_range` would be permanently the registered
            // default and a creative player's crosshair would stop two blocks
            // short.
            self.capture_local_attributes(body);
        } else if crate::route_inventory(
            id,
            body,
            ids,
            self.swing_data.as_ref().map(|d| d.components),
            &mut self.inventory,
            &mut self.menus,
            Some(&mut self.stack_details),
        ) {
            // M34: the player's own inventory — contents, one slot, or the
            // server moving the selection. M87: or an open container's, since
            // the same two packet ids address either menu.
        } else if crate::route_tags(id, body, ids, &mut self.tags) {
            // M69 — a datapack reload's `update_tags`. The join-time copy
            // arrives during configuration and is applied there; this arm is
            // the mid-session one. Per-registry wholesale replacement, so a
            // body that fails to decode is dropped whole rather than
            // half-applied.
        } else if crate::route_view_area(id, body, ids, &mut self.view_area) {
            // M67 — the server's view area. Decode and state only; nothing
            // evicts a column or gates a tick on it yet.
        } else if crate::route_border(id, body, ids, &mut self.border) {
        } else if crate::route_ticking(id, body, ids, &mut self.ticking) {
            // M74 — `/tick rate`, `/tick freeze`, `/tick step`. Decode and
            // state only; the 20 Hz loop does not consult it yet.
        } else if crate::route_session(id, body, ids, &mut self.session) {
            // M78 — the brand, the MOTD, the game rules, the cookie jar, the
            // two vestigial combat packets, and disguised chat.
            //
            // The chat lines are drained here rather than written by the router
            // so `crate::session` needs no reference to this type. They join
            // the same log `system_chat` and `player_chat` push to, and at the
            // same fidelity: the *raw* message, not the decoration, because
            // decorating needs the `minecraft:chat_type` registry Rewo does not
            // parse.
            // `handleDisguisedChatMessage` adds it with `GuiMessageTag.system()`
            // and a null signature — it is not a signed player message, so it
            // can never be the target of a `delete_chat`.
            for chat in self.session.take_chat() {
                // M127: decorated, as `handleDisguisedChatMessage` does. A
                // `/say` therefore reads `[Server] hi` rather than `hi`.
                let decorated = self.decorate_chat(&chat.message, &chat.bound);
                let spans = self.chat_component_spans(&decorated);
                let line = rewo_world::chat_style::plain_text(&spans);
                if line.is_empty() {
                    continue;
                }
                self.chat_log.push(line);
                self.chat_events.push(crate::chat_wire::ChatEvent::Message {
                    text: spans,
                    signature: None,
                    tag: Some(rewo_world::chat::MessageTag::SYSTEM),
                    source: rewo_world::chat::MessageSource::Player,
                });
            }
        } else if crate::route_waypoint(id, body, ids, &mut self.waypoints) {
            // M83 — the locator bar. `handleWaypoint` is two lines: the thread
            // check and `packet.apply(this.waypointManager)`. There is no
            // gamerule check and no range check on this side; the server has
            // already decided both, and the client draws whatever it was told
            // to track. See `crate::waypoints`.
        } else if crate::route_hud_state(id, body, ids, &mut self.hud) {
            // M79 — the title overlay, the XP gauge and the item-cooldown map.
            // Every one of the seven writes state a renderer reads; none of
            // them answers the server. See `crate::hud_state`.
        } else if Some(id) == ids.cb_play_cookie_request {
            // M78 closes a hole it would otherwise have shipped around: the
            // *play-state* `cookie_request` was once answered only by a
            // separate soak harness, never by this session, so the real
            // client left it unanswered entirely. `store_cookie` fills a
            // jar whose only observable consequence is this reply, and a jar
            // nothing reads is not a feature.
            //
            // `handleRequestCookie` — `send(new ServerboundCookieResponsePacket(
            // key, serverCookies.get(key)))`. A key we hold answers with its
            // payload; one we do not answers with nothing, which is the
            // behaviour the whole client had before M78.
            let key = PacketReader::new(body).identifier().unwrap_or_default();
            if let Some(resp_id) = ids.sb_play_cookie_response {
                let payload = self.session.cookie(&key).map(<[u8]>::to_vec);
                let resp =
                    crate::session::write_cookie_response(resp_id, &key, payload.as_deref());
                self.send(resp)?;
            }
        } else if crate::route_client_state(
            id,
            body,
            ids,
            &mut self.client_state,
            &self.world.entities,
            self.player_id,
        ) {
            // M74 — difficulty, the camera's target, and the container-close
            // latch. Decode and state; the app reads the camera and the latch.
            //
            // M87 hangs the menu close off this arm rather than off
            // `route_menu`, because this chain is a sequence of `else if`s and
            // `container_close` already belongs to this seam. A second seam
            // claiming the same id would either steal it from M74's counter or
            // never see it, depending only on which arm came first.
            if id == ids.cb_play_container_close {
                self.menus.apply_close();
            }
        } else if crate::route_menu(id, body, ids, &mut self.menus) {
            // M87 — `open_screen` and `container_set_data`. State only so far:
            // the menu's own item slots arrive when `Inventory` becomes a
            // layout-driven menu, and nothing renders it yet.
        } else if id == ids.cb_play_merchant_offers {
            // M93u. The coverage doc filed this as class C; it needed nothing
            // Rewo had not already built — `ItemStack` (M34/M41) and the
            // `TypedDataComponent` walker M52e wrote for `can_place_on`.
            // The component ids ride on `swing_data`, which is where every
            // other `read_optional` caller finds them; with no registry yet
            // there is nothing to decode a stack against, so the packet is
            // dropped rather than guessed at.
            match self
                .swing_data
                .as_ref()
                .map(|d| d.components)
                .ok_or_else(|| "merchant_offers: no component registry".to_string())
                .and_then(|ids| crate::merchant::parse(body, ids))
            {
                Ok(m) => self.merchant = Some(m),
                // A short or malformed body is dropped whole rather than
                // applied in part: half a trade list is worse than none, since
                // the index a click sends addresses the list by position.
                Err(e) => log::warn!("net: {e}"),
            }
        } else if id == ids.cb_play_open_book {
            // M171. `ClientboundOpenBookPacket(InteractionHand)` — one
            // enum ordinal. The pages live on the held item's
            // `written_book_content` (captured in `StackComponents`), so
            // the app resolves the stack; here we only record the hand.
            let mut r = PacketReader::new(body);
            match r.varint() {
                Ok(hand) => self.open_book_request = Some(hand),
                Err(e) => self.decode_failed("open_book", e),
            }
        } else if id == ids.cb_play_open_sign_editor {
            // M174. `ClientboundOpenSignEditorPacket` — one packed BlockPos
            // (8 bytes) then one bool. No var-ints anywhere in the body.
            let mut r = PacketReader::new(body);
            match (|| -> rewo_proto::Result<((i32, i32, i32), bool)> {
                Ok((r.position()?, r.u8()? != 0))
            })() {
                Ok((pos, front)) => self.open_sign_editor_request = Some((pos, front)),
                Err(e) => self.decode_failed("open_sign_editor", e),
            }
        } else if id == ids.cb_play_recipe_book_add
            || id == ids.cb_play_recipe_book_remove
            || id == ids.cb_play_recipe_book_settings
            || id == ids.cb_play_place_ghost_recipe
        {
            // M93y. The BOOK is a subsystem Rewo does not have — tabs, search,
            // filtering, ghost placement — and this is the decode half only.
            // It is dispatched rather than left resolved-but-ignored because
            // that class is the one `REWO_PACKET_COVERAGE.md` keeps at zero: a
            // packet whose id resolves and whose body is dropped reads as
            // handled to every grep.
            self.apply_recipe_book(id, body);
        } else if id == ids.cb_play_update_recipes {
            // M152. Unlike the four above this one IS consumed: its
            // `smithing_base` / `smithing_template` / `smithing_addition` sets
            // are exactly what `SmithingMenu.canMoveIntoInputSlots` tests, and
            // that guard was the only reason smithing stayed
            // `QuickMove::Unimplemented` after M93 took the other seven.
            //
            // The display registries are BUILT-IN, so they come from the report
            // rather than the wire (M92's rule) and are needed for the
            // stonecutter half's `SlotDisplay`s. Their absence is a warn rather
            // than a decode error: it is a Rewo-side setup failure, not a
            // malformed packet, and the two should not report the same way.
            match self.recipe_display_ids.as_ref() {
                Some(display_ids) => {
                    match crate::recipe_book::parse_update_recipes(body, display_ids) {
                        // Replace, never merge — see the field's docs.
                        Ok(u) => self.recipes = Some(u),
                        Err(e) => log::warn!("net: {e}"),
                    }
                }
                None => log::warn!("net: update_recipes with no display registries"),
            }
        } else if id == ids.cb_play_update_advancements {
            // M177. The whole feed — reset / added / removed / progress /
            // show. Applied through `ClientAdvancements`, whose tree insertion
            // runs in parent-before-child passes because a server may send a
            // child before its parent inside one packet. A malformed body is
            // dropped whole: half a tree is worse than a stale one.
            match crate::advancements::parse_update(body) {
                Ok(u) => self.advancements.apply_update(u),
                Err(e) => log::warn!("net: {e}"),
            }
        } else if id == ids.cb_play_select_advancements_tab {
            // M177. `handleSelectAdvancementsTab` resolves the id against the
            // tree and NEVER tells the server: null clears the selection, an
            // unknown id resolves to null and ALSO clears it.
            match crate::advancements::parse_select_tab(body) {
                Ok(Some(tab)) => {
                    let resolved = self.advancements.node(&tab).map(|_| tab.clone());
                    self.advancements.select_tab(resolved.as_deref());
                }
                Ok(None) => {
                    self.advancements.select_tab(None);
                }
                Err(e) => log::warn!("net: {e}"),
            }
        } else if id == ids.cb_play_game_event {
            // M33 took the four weather ids; M71 took the other ten. One
            // decode feeds the weather levels, the client game state and the
            // local sound queue — see `apply_game_event`.
            self.apply_game_event(body);
        } else if id == ids.cb_play_player_abilities {
            // M75. `handlePlayerAbilities` is six assignments and nothing else —
            // no derived state, no packet in reply. In particular it does NOT
            // touch `may_build` (absent from the wire) and does NOT feed
            // `walkingSpeed` into the movement speed.
            match crate::abilities::PlayerAbilities::parse(body) {
                Ok(p) => p.apply_to(&mut self.abilities),
                // A short body is the one case vanilla's reader would throw on.
                // Dropping it leaves the abilities we already had, which is
                // closer to "the packet never arrived" than a partial apply.
                Err(e) => log::warn!("net: player_abilities: {e}"),
            }
        } else if crate::route_animate(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // Combat arm swings (`ClientboundAnimatePacket` actions 0 / 3) — the
            // swing clock then advances in `EntityTable::tick_lerp`.
        } else if crate::route_set_equipment(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.swing_data.as_ref(),
            self.entity_classes.as_deref(),
        ) {
            // Held items: the swing's duration + animation type come from them.
        } else if crate::route_entity_event(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.warden_type_id,
            self.armadillo_type_id,
            self.ticks as i64,
            self.entity_classes.as_deref(),
        ) {
            // Model-visible entity events (warden attack/sonic boom, armadillo
            // peek) were stamped with the current tick — the renderer measures
            // the rig's elapsed time from it. `self.ticks` is the in-progress
            // tick (it increments at the end of `tick()`, after this drain).
            //
            // M141g: and the two SOUND events in the same switch. They are
            // handled here rather than inside `route_entity_event` because
            // that seam writes the entity table and these push a sound, and
            // the body is two fixed fields either way.
            self.entity_event_sound(body);
        } else if crate::route_move_minecart_along_track(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M77. An experimental-movement minecart's ONLY movement channel —
            // `ServerEntity.sendChanges` sends it instead of `move_entity_pos`
            // / `teleport_entity` / `entity_position_sync`, so the generic
            // 3-tick lerp is never armed for one of these carts. The schedule
            // is traversed in `EntityTable::tick_lerp`, before the riders are
            // placed; see `rewo_world::minecart` for why both interpolations
            // stay live.
        } else if crate::route_set_entity_link(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M77. The leash holder id, stored and not drawn.
        } else if crate::route_projectile_power(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M77. `AbstractHurtingProjectile.accelerationPower`.
        } else if id == ids.cb_play_set_passengers {
            // Riding (M70). Consumed for `Entity.isVehicle()`, which
            // suppresses a ridden entity's floating label. It does **not** yet
            // move a passenger onto its vehicle's position — that is the
            // separate gap `REWO_PACKET_COVERAGE.md` records against this
            // packet, and this milestone does not close it.
            crate::route_set_passengers(id, body, ids, &mut self.world.entities);
            // Riding, the physics half (M68). Disjoint from the label half
            // above and deliberately a second read of the same slice: M70
            // wants the riding graph, M68 wants the local player's own mount
            // state, and folding either into the other's walk would couple two
            // milestones that have no reason to share a decode.
            match crate::motion::read_set_passengers(body) {
                Ok(p) => self.apply_set_passengers(&p),
                Err(err) => self.decode_failed("set_passengers", err),
            }
        } else if id == ids.cb_play_set_player_team {
            // Scoreboard teams (M62). A body we cannot decode is dropped
            // whole rather than half-applied: the packet's three sections are
            // positional, so a short read means the roster we did get is not
            // the roster the server sent.
            match crate::teams::parse_set_player_team(body) {
                Ok(p) => {
                    self.scoreboard.teams.apply(&p);
                }
                Err(e) => self.decode_failed("set_player_team", e),
            }
        } else if id == ids.cb_play_set_objective {
            // M65 — the scoreboard's other half. Every arm below drops a body
            // it cannot decode whole rather than half-applying it, for the
            // same reason `set_player_team` does: these packets are
            // positional, so a short read means the values we did get are not
            // the values the server sent.
            match crate::scoreboard::parse_set_objective(body, self.number_formats) {
                Ok(p) => {
                    self.scoreboard.apply_set_objective(&p);
                }
                Err(e) => self.decode_failed("set_objective", e),
            }
        } else if id == ids.cb_play_set_score {
            match crate::scoreboard::parse_set_score(body, self.number_formats) {
                Ok(p) => {
                    self.scoreboard.apply_set_score(&p);
                }
                Err(e) => self.decode_failed("set_score", e),
            }
        } else if id == ids.cb_play_reset_score {
            match crate::scoreboard::parse_reset_score(body) {
                Ok(p) => {
                    self.scoreboard.apply_reset_score(&p);
                }
                Err(e) => self.decode_failed("reset_score", e),
            }
        } else if id == ids.cb_play_set_display_objective {
            match crate::scoreboard::parse_set_display_objective(body) {
                Ok(p) => self.scoreboard.apply_set_display_objective(&p),
                Err(e) => self.decode_failed("set_display_objective", e),
            }
        } else if id == ids.cb_play_boss_event {
            match crate::boss_bar::parse_boss_event(body) {
                Ok(p) => {
                    self.boss_bars.apply(&p);
                }
                Err(e) => self.decode_failed("boss_event", e),
            }
        } else if id == ids.cb_play_tab_list {
            match crate::tab_list_text::parse_tab_list(body) {
                Ok(p) => self.tab_list_text.apply(&p),
                Err(e) => self.decode_failed("tab_list", e),
            }
        } else if id == ids.cb_play_player_info_update {
            self.apply_player_info(body);
        } else if id == ids.cb_play_player_info_remove {
            let mut r = PacketReader::new(body);
            if let Ok(n) = r.count("player info removes", 16) {
                for _ in 0..n {
                    if let Ok(uuid) = r.uuid() {
                        self.world.entities.remove_name(uuid);
                        // A departed player's ping is not stale, it is gone --
                        // keeping it would let the tab list quote a number for
                        // someone who left. Vanilla drops the whole
                        // `PlayerInfo`, so the mode and the list order go with
                        // it. The TEAM does not: `handlePlayerInfoRemove`
                        // never touches the scoreboard, and a team outlives
                        // its members leaving.
                        self.latency.remove(&uuid);
                        self.gamemodes.remove(&uuid);
                        self.tab_list_orders.remove(&uuid);
                        // M151 — `handlePlayerInfoRemove` drops the whole
                        // `PlayerInfo` *and* removes it from `listedPlayers`
                        // (`ClientPacketListener.java:1995`).
                        self.tab_players.forget(uuid);
                    }
                }
            } else {
                self.decode_failed("player_info_remove", "malformed body");
            }
        } else if Some(id) == ids.cb_play_set_health {
            let mut r = PacketReader::new(body);
            if let Ok(h) = r.f32() {
                // `handleSetHealth` is `player.hurtTo(health)` then the two
                // food setters (`ClientPacketListener.java:1235-1240`); the
                // hurt window is armed against the health we HAD (M168).
                self.hud.local_hurt.hurt_to(self.health, h);
                self.health = h;
                // food (VarInt) + saturation (f32) follow.
                if let Ok(f) = r.varint() {
                    self.food = f;
                    if let Ok(s) = r.f32() {
                        self.saturation = s;
                    }
                }
                // `Player.isDeadOrDying()`'s health half. **This used to send
                // `PERFORM_RESPAWN` from here** (M3, so the headless bot could
                // recover), and that is not what a vanilla client does:
                // `handleSetHealth` assigns the three fields and nothing else.
                // Respawning is a *screen* action, so M82 moved it to
                // `player_combat_kill` — which is where vanilla decides
                // between the death screen and an immediate respawn — and left
                // the flag here. A harness with no screen respawns by draining
                // [`Self::take_death`], which is the same branch vanilla takes
                // when `shouldShowDeathScreen()` is false.
                self.dead = h <= 0.0;
            } else {
                self.decode_failed("set_health", "malformed body");
            }
        } else if Some(id) == ids.cb_play_system_chat {
            let mut r = PacketReader::new(body);
            if let Ok(packet) = crate::chat_wire::SystemChat::read(&mut r) {
                // `handleSystemChat` branches on `overlay`: true goes to
                // `handleOverlay`, which is `gui.setOverlayMessage` — the
                // ACTION BAR, not the chat log. Reading the component and
                // dropping the bool (which is what this arm used to do) put
                // every `/title actionbar` line into chat.
                // M125: resolved here, where the language table is, rather
                // than at the wire. `handleSystemChat` renders the component,
                // and a component whose contents are a `TranslatableContents`
                // renders as its translation — so flattening before the lookup
                // put `multiplayer.player.joined` on screen where vanilla puts
                // "Steve joined the game".
                let spans = self.chat_component_spans(&packet.content);
                let content = rewo_world::chat_style::plain_text(&spans);
                if packet.overlay {
                    // The action bar draws one flat string, so the spans stop
                    // here rather than being threaded through a second render.
                    self.chat_events
                        .push(crate::chat_wire::ChatEvent::Overlay(content));
                } else if !content.is_empty() {
                    self.chat_log.push(content);
                    self.chat_events.push(crate::chat_wire::ChatEvent::Message {
                        text: spans,
                        signature: None,
                        tag: Some(rewo_world::chat::MessageTag::SYSTEM_SINGLE_PLAYER),
                        source: rewo_world::chat::MessageSource::SystemServer,
                    });
                }
            } else {
                self.decode_failed("system_chat", "malformed body");
            }
        } else if Some(id) == ids.cb_play_player_chat {
            let mut r = PacketReader::new(body);
            match crate::chat_wire::PlayerChat::read(&mut r) {
                Ok(chat) => {
                    // `MessageSignatureCache.push` runs on receipt and BEFORE
                    // anything decides whether to show the message, because a
                    // later `delete_chat` may address this signature by the
                    // index this push assigns it. Feeding the cache only from
                    // *displayed* messages would leave those indices pointing
                    // at the wrong signatures.
                    let last_seen: Vec<Box<crate::chat_wire::Signature>> = chat
                        .body
                        .last_seen
                        .iter()
                        .filter_map(|p| self.signature_cache.resolve(p))
                        .collect();
                    self.signature_cache
                        .push(&last_seen, chat.signature.as_deref());
                    let received = self.chat_clock_millis;
                    // Bound before the `if let` on purpose: the two closures
                    // borrow `self`, and an `if let` scrutinee's temporaries
                    // live for the whole block, which would collide with the
                    // `self.chat_log.push` inside it.
                    let outcome = crate::chat_wire::show_message(
                        &chat,
                        received,
                        &|content| self.decorate_chat(content, &chat.bound),
                        &|tag| self.chat_component_text(tag),
                    );
                    let was_shown =
                        matches!(outcome, crate::chat_wire::ChatOutcome::Shown { .. });
                    if let Some(sig) = chat.signature.as_deref() {
                        self.mark_message_processed(sig, was_shown)?;
                    }
                    if let crate::chat_wire::ChatOutcome::Shown { content, tag } = outcome {
                        // M127: `content` is the DECORATED component now, so
                        // the store gets `<Steve> hi` rather than `hi`.
                        //
                        // Signed chat is a plain `String` on the wire, not a
                        // component — but vanilla still renders it through
                        // `StringDecomposer.iterateFormatted`, so a server's
                        // `§e` is a colour and not two glyphs of garbage. That
                        // survives the move to a component path: the content is
                        // wrapped as `Component.literal`, and `chat_style`'s
                        // walk runs `push_legacy` over a literal's text.
                        let spans = self.chat_component_spans(&content);
                        self.chat_log
                            .push(rewo_world::chat_style::plain_text(&spans));
                        self.chat_events.push(crate::chat_wire::ChatEvent::Message {
                            text: spans,
                            signature: chat.signature,
                            tag,
                            source: rewo_world::chat::MessageSource::Player,
                        });
                    }
                }
                Err(e) => self.decode_failed("player_chat", e),
            }
        } else if id == ids.cb_play_commands {
            // The argument-type registry is a BUILT-IN one, so it comes from
            // the report rather than the wire (M92's rule) — and without it
            // the tree cannot be read past its first non-singleton argument,
            // which is why a missing table is a warn-and-drop rather than a
            // partial parse.
            match self.command_argument_types.as_ref() {
                Some(types) => {
                    match crate::commands::read_commands(body, &|i| types.name(i)) {
                        Ok(tree) => {
                            log::info!(
                                "net: command tree — {} nodes, {} top-level",
                                tree.nodes.len(),
                                tree.top_level().len()
                            );
                            self.commands = tree;
                        }
                        Err(e) => self.decode_failed("commands", e),
                    }
                }
                None => log::debug!("net: commands arrived before the argument-type table"),
            }
        } else if id == ids.cb_play_command_suggestions {
            // `handleCommandSuggestions` is one line:
            // `suggestionsProvider.completeCustomSuggestions(id, toSuggestions())`.
            // The id test is the whole of it — a reply to a superseded request
            // is dropped rather than repainting the popup with the answer to a
            // prefix already typed past.
            match crate::suggestion_wire::CommandSuggestionsReply::read(body) {
                Ok(reply) => {
                    if let Some(s) = self.suggestions.complete(&reply) {
                        self.suggestion_reply = Some(s);
                    } else {
                        log::debug!(
                            "net: command_suggestions id {} is not the outstanding request",
                            reply.id
                        );
                    }
                }
                Err(e) => self.decode_failed("command_suggestions", e),
            }
        } else if id == ids.cb_play_custom_chat_completions {
            match crate::suggestion_wire::read_custom_chat_completions(body) {
                Ok((action, entries)) => self.suggestions.apply_completions(action, &entries),
                Err(e) => self.decode_failed("custom_chat_completions", e),
            }
        } else if id == ids.cb_play_delete_chat {
            let mut r = PacketReader::new(body);
            match crate::chat_wire::read_delete_chat(&mut r) {
                // An unresolvable packed id is a no-op rather than an error:
                // vanilla's `unpack` would return null for an empty slot and
                // `deleteMessageOrDelay` then finds no message. Rewo also
                // reaches here for an out-of-range id, where vanilla throws —
                // see `chat_wire`'s module docs.
                Ok(packed) => match self.signature_cache.resolve(&packed) {
                    Some(sig) => {
                        self.last_seen.ignore_pending(&sig);
                        self.chat_events
                            .push(crate::chat_wire::ChatEvent::Delete(sig));
                    }
                    None => log::debug!("net: delete_chat named an unknown signature"),
                },
                Err(e) => self.decode_failed("delete_chat", e),
            }
        } else if id == ids.cb_play_disconnect {
            // M129 — resolved against the language table rather than
            // flattened. Every vanilla kick is a `Component.translatable`, so
            // this was the most translatable-dense component Rewo received and
            // the one that rendered as a raw key most often. The decode lives
            // in `rewo_world::disconnect_screen` so a test can reach it.
            let (reason, cause) =
                rewo_world::disconnect_screen::read_disconnect(body, self.lang.as_deref());
            self.disconnect = Some(reason);
            self.disconnect_cause = Some(cause);
        }
        Ok(())
    }
}
