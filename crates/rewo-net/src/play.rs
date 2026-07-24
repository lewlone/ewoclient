//! M3 live play session: a 20 Hz tick loop over a split socket.
//!
//! `Connection` (M1) handles login + configuration synchronously, then
//! `into_play()` splits: a reader thread decodes frames into a channel; the
//! tick thread drains packets, runs vanilla physics (`rewo_world::physics`),
//! and sends movement with the decompiled `LocalPlayer.sendPosition`
//! cadence (Pos/PosRot/Rot/StatusOnly + 20-tick reminder + tick_end +
//! player_input on change). Corrections received from the server are THE
//! physics-parity meter (REWO_PLAN.md M3 DoD: "corrections rare").

use std::io::Write as _;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

use rewo_proto::frame::FrameCodec;
use rewo_proto::reader::PacketReader;
use rewo_proto::writer::PacketWriter;
use rewo_world::dimension::DimensionShape;
use rewo_world::physics::{self, PlayerState, TickInput};
use rewo_world::World;

use crate::ids::Ids;
use crate::Connection;

pub struct PlaySession {
    writer: crate::NetStream,
    codec: FrameCodec,
    rx: Receiver<Vec<u8>>,
    pub ids: Ids,
    pub world: World,
    pub player: PlayerState,
    /// state id → collision boxes in block-local `0..1` (`BakedAssets::
    /// collide`): empty = no collision, one unit box = a full cube, several =
    /// a stair. An id past the end falls back to "non-air is a full cube",
    /// which is what the flat test worlds want when no bake is supplied.
    pub collide: Vec<Vec<[f32; 6]>>,
    /// Per entity-type `(width, height, pushable)` for entity collision.
    /// Empty disables pushing (the harnesses that don't care about mobs).
    pub entity_push: Vec<(f32, f32, bool)>,
    /// Chunk global-palette bit width (from the blocks table).
    global_bits: u32,
    dim_shapes: Vec<DimensionShape>,
    /// Registry id of the overworld world clock — `set_time` keys its clock
    /// map by raw id, and the map also carries `the_end`'s clock.
    overworld_clock_id: Option<i32>,
    pub spawned: bool,
    pub corrections: u32,
    pub teleports: u32,
    pub block_updates: u32,
    /// World-clock tick driving the day/night cycle (`rewo_world::daylight`).
    /// `None` until the first `set_time` arrives, which the renderer reads as
    /// full daylight. Refreshed from two places, exactly as vanilla runs them:
    /// `set_time` (`handleUpdates`) and the per-tick local advance
    /// (`ClientLevel.tickTime`). Derived from `overworld_clock` once a real
    /// clock state exists; otherwise a `gameTime` best-effort.
    pub day_ticks: Option<i64>,
    /// The overworld world clock, ported from 26.2 `ClientClockManager`. It is
    /// advanced from the same two places vanilla advances it: `set_time`
    /// (`handleUpdates` — advance by the game-time delta, then any explicit
    /// clock state overwrites it) and the per-tick `ClientLevel.tickTime`
    /// (advance one game tick locally). The local advance is what keeps the
    /// day/night cycle smooth between the server's 20-tick syncs instead of
    /// jumping. `None` until the first explicit overworld clock state
    /// establishes it.
    overworld_clock: Option<WorldClock>,
    /// The client's authoritative game-time counter (`ClientLevel`'s
    /// `clientLevelData.getGameTime()`). `None` until the first `set_time`
    /// establishes it; a server sync resets it to the packet value and every
    /// running client tick increments it by one (`ClientLevel.tickTime`). This
    /// is the game time the local world-clock advance runs against — keeping it
    /// in lockstep with `overworld_clock.last_game_time` is what makes a sync at
    /// the already-predicted time a zero-delta no-op (no double count).
    game_time: Option<i64>,
    /// Columns whose mesh is stale (new chunk / block edit). The live
    /// renderer drains this to know what to re-mesh; the bot ignores it.
    dirty: std::collections::HashSet<(i32, i32)>,
    /// Client-side lighting. The server sends authoritative light on chunk
    /// load but never for individual edits, so every block change is relit
    /// here — see `rewo_world::light`. Empty tables (the default) disable it,
    /// which keeps the headless protocol tests independent of the asset bake.
    light: rewo_world::light::LightEngine,
    light_emission: Vec<u8>,
    light_dampening: Vec<u8>,
    light_faces: Vec<u8>,
    /// Columns the server dropped — the renderer frees their GPU buffers.
    removed: Vec<(i32, i32)>,
    pub chat_log: Vec<String>,
    pub health: f32,
    /// Food level 0..20 (Set Health packet), for the HUD hunger bar.
    pub food: i32,
    pub dead: bool,
    pub disconnect: Option<String>,
    // sendPosition cadence state (decompiled LocalPlayer).
    last_pos: (f64, f64, f64),
    last_rot: (f32, f32),
    reminder: u32,
    last_on_ground: bool,
    last_horiz: bool,
    last_input_flags: u8,
    sequence: i32,
    pub ticks: u64,
    /// Signed-chat signer (online-mode + a fetched player certificate).
    /// `None` → unsigned chat (offline servers, or a cert-fetch failure).
    signer: Option<crate::chat_sign::ChatSigner>,
    /// Resolved player skins by profile UUID (from the Player Info
    /// `textures` property). Online-mode only — offline servers send none.
    player_skins: std::collections::HashMap<u128, crate::skins::SkinInfo>,
    /// Newly-announced skins the app hasn't fetched yet (drained each frame).
    pending_skins: Vec<(u128, crate::skins::SkinInfo)>,
    /// Local player's night-vision / darkness effects, driving the camera
    /// lightmap (M13). Fed by `update_mob_effect` / `remove_mob_effect` and one
    /// `tick()` per client tick.
    visual_effects: crate::effects::VisualEffects,
    /// M14 biome tint. The parsed registry (raw order) waits here until the
    /// play-login packet supplies the `biomeZoomSeed` + dimension holder; the
    /// full `BiomeContext` is then built and attached to `world`.
    pending_biome_registry: Option<rewo_world::biome::BiomeRegistry>,
    dim_attrs: Vec<(Option<i32>, Option<i32>)>,
    colormaps: rewo_world::biome::Colormaps,
    /// Biome container global-palette width (`BiomeRegistry::global_bits`).
    biome_global_bits: u32,
    /// `CommonPlayerSpawnInfo.seed` — the `biomeZoomSeed` driving the fiddle.
    pub biome_zoom_seed: Option<i64>,
}

impl<'a> Connection<'a> {
    /// Login + configuration, then split into a live session. `auth`
    /// answers an online-mode server's encryption request (M7); `None`
    /// keeps the M1 offline behavior.
    pub fn into_play(
        mut self,
        host: &str,
        port: u16,
        username: &str,
        auth: Option<&crate::crypt::OnlineAuth>,
        collide: Vec<Vec<[f32; 6]>>,
        global_bits: u32,
        colormaps: rewo_world::biome::Colormaps,
    ) -> Result<PlaySession, String> {
        self.login(host, port, username, auth)?;
        let mut stats = crate::SessionStats {
            packets_in: 0,
            bytes_in: 0,
            chunks: 0,
            keepalives: 0,
            teleports: 0,
            reached_play: false,
            disconnect_reason: None,
            world_digest: 0,
            loaded_columns: 0,
        };
        self.run_configuration(&mut stats)?;

        // Split the (possibly encrypted) stream — each half carries its
        // direction's CFB8 state.
        let (reader_stream, writer) = self
            .stream
            .split()
            .map_err(|e| format!("split socket: {e}"))?;
        let codec = FrameCodec {
            compression_threshold: self.codec.compression_threshold,
        };
        let reader_codec = FrameCodec {
            compression_threshold: self.codec.compression_threshold,
        };
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("rewo-net-reader".into())
            .spawn(move || {
                let mut stream = reader_stream;
                let mut scratch = Vec::new();
                loop {
                    let mut packet = Vec::new();
                    if reader_codec
                        .read_frame(&mut stream, &mut scratch, &mut packet)
                        .is_err()
                    {
                        return; // socket closed / error → channel drops
                    }
                    if tx.send(packet).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| format!("spawn reader: {e}"))?;

        let mut world = World::new(DimensionShape::OVERWORLD);
        // Dimension shapes were collected during configuration.
        if let Some(shape) = self.dim_shapes.first() {
            world.shape = *shape;
        }
        let dim_shapes = self.dim_shapes.clone();
        let overworld_clock_id = self.overworld_clock_id;
        let visual_effects =
            crate::effects::VisualEffects::new(self.night_vision_id, self.darkness_id);
        // Biome registry parsed during configuration; the `biomeZoomSeed` +
        // dimension holder arrive with the play-login packet (`apply_login_shape`).
        // Access the field directly (not a `&self` method) — `self.stream` was
        // already moved by `split()`, so `self` is partially moved here.
        let pending_biome_registry = if self.biome_defs.is_empty() {
            None
        } else {
            Some(rewo_world::biome::BiomeRegistry::new(self.biome_defs.clone()))
        };
        let dim_attrs = self.dim_attrs.clone();
        let biome_global_bits = pending_biome_registry
            .as_ref()
            .map(|r| r.global_bits)
            .unwrap_or(7);
        let mut session = PlaySession {
            writer,
            codec,
            rx,
            ids: self.ids,
            world,
            player: PlayerState::at(0.5, 80.0, 0.5),
            collide,
            entity_push: Vec::new(),
            global_bits,
            dim_shapes,
            overworld_clock_id,
            spawned: false,
            corrections: 0,
            teleports: 0,
            block_updates: 0,
            day_ticks: None,
            overworld_clock: None,
            game_time: None,
            dirty: std::collections::HashSet::new(),
            light: rewo_world::light::LightEngine::new(),
            light_emission: Vec::new(),
            light_dampening: Vec::new(),
            light_faces: Vec::new(),
            removed: Vec::new(),
            chat_log: Vec::new(),
            health: 20.0,
            food: 20,
            dead: false,
            disconnect: None,
            last_pos: (0.0, 0.0, 0.0),
            last_rot: (0.0, 0.0),
            reminder: 0,
            last_on_ground: false,
            last_horiz: false,
            last_input_flags: 0,
            sequence: 0,
            ticks: 0,
            signer: None,
            player_skins: std::collections::HashMap::new(),
            pending_skins: Vec::new(),
            visual_effects,
            pending_biome_registry,
            dim_attrs,
            colormaps,
            biome_global_bits,
            biome_zoom_seed: None,
        };
        // Online-mode: fetch a player certificate and announce the chat
        // session so `enforce-secure-profile` servers accept our chat. A
        // fetch failure is non-fatal — chat falls back to unsigned.
        if let Some(auth) = auth {
            match crate::chat_sign::ChatSigner::fetch(auth) {
                Ok(signer) => {
                    session.signer = Some(signer);
                    if let Err(e) = session.announce_chat_session() {
                        log::warn!("net: chat_session_update failed: {e}");
                    } else {
                        log::info!("net: chat session announced (signed chat enabled)");
                    }
                }
                Err(e) => log::warn!("net: player certificate fetch failed ({e}); unsigned chat"),
            }
        }
        Ok(session)
    }
}

impl PlaySession {
    fn send(&mut self, packet: PacketWriter) -> Result<(), String> {
        self.codec
            .write_frame(&mut self.writer, &packet.buf)
            .map_err(|e| format!("send: {e}"))?;
        self.writer.flush().ok();
        Ok(())
    }

    fn next_sequence(&mut self) -> i32 {
        self.sequence += 1;
        self.sequence
    }

    /// Mark a column + its 4 orthogonal neighbors stale for re-meshing.
    fn mark_dirty_around(&mut self, cx: i32, cz: i32) {
        for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            if self.world.is_loaded((cx + dx) * 16, (cz + dz) * 16) {
                self.dirty.insert((cx + dx, cz + dz));
            }
        }
    }

    /// Drain the stale-column set (live renderer re-meshes these).
    /// Install the per-state light tables from the asset bake, enabling
    /// client-side relighting of block edits.
    pub fn set_light_tables(&mut self, emission: Vec<u8>, dampening: Vec<u8>, faces: Vec<u8>) {
        self.light_emission = emission;
        self.light_dampening = dampening;
        self.light_faces = faces;
    }

    /// Apply a block change and relight around it, marking every column whose
    /// light moved for remesh. `old` is the state before the write.
    fn relight(&mut self, x: i32, y: i32, z: i32, old: u32, new: u32) {
        if self.light_dampening.is_empty() {
            return;
        }
        let tables = rewo_world::light::LightTables {
            emission: &self.light_emission,
            dampening: &self.light_dampening,
            face_occludes: &self.light_faces,
        };
        for (cx, cz) in self
            .light
            .on_block_change(&mut self.world, tables, x, y, z, old, new)
        {
            self.dirty.insert((cx, cz));
        }
    }

    pub fn take_dirty(&mut self) -> Vec<(i32, i32)> {
        self.dirty.drain().collect()
    }

    /// Drain newly-announced player skins (UUID → skin) for the app to
    /// fetch + upload. Each skin is queued once (per value change).
    pub fn take_pending_skins(&mut self) -> Vec<(u128, crate::skins::SkinInfo)> {
        std::mem::take(&mut self.pending_skins)
    }

    /// Re-queue columns whose re-mesh was deferred (per-frame budget).
    pub fn requeue_dirty(&mut self, cols: impl IntoIterator<Item = (i32, i32)>) {
        self.dirty.extend(cols);
    }

    /// How many columns are currently queued for re-mesh (cheap peek).
    pub fn dirty_len(&self) -> usize {
        self.dirty.len()
    }

    /// Drain the dropped-column list (renderer frees their buffers).
    pub fn take_removed(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.removed)
    }

    /// One 20 Hz tick: drain inbound, run physics, send movement.
    pub fn tick(&mut self, input: &TickInput) -> Result<(), String> {
        self.drain_inbound()?;
        if self.disconnect.is_some() {
            return Ok(());
        }
        // `ClientLevel.tickTime`: once the level exists, every running client
        // tick bumps the game time by one and ticks the world clock against it.
        // This is what advances the day/night cycle smoothly between the
        // server's 20-tick `set_time` syncs — the sync-only path moved the clock
        // in visible jumps and fell short of the elapsed ticks. Runs even before
        // spawn, exactly like vanilla's `ClientLevel.tick` after the level
        // exists. A sync processed above in `drain_inbound` already re-anchored
        // `game_time`, so this is the one and only local `+1` for the tick.
        if let Some((game_time, day)) = local_tick_time(self.game_time, &mut self.overworld_clock) {
            self.game_time = Some(game_time);
            self.day_ticks = Some(day);
        }
        // Advance the local player's visual effects once per client tick.
        // Vanilla increments the player's `tickCount` in
        // `ClientLevel.tickNonPassenger` *before* `entity.tick()`, which later
        // reaches the effects via `LivingEntity.baseTick` → `tickEffects`
        // (client branch). `VisualEffects::tick` keeps that order (count first,
        // then effects). Gating is on the player *entity existing* (login has
        // set its id), NOT on `self.spawned` (movement readiness): the local
        // entity is created at login, so effects tick from then on — but
        // `VisualEffects::tick` no-ops until `set_player_id`, so calling it
        // before login (no local entity yet, as in vanilla) does nothing.
        self.visual_effects.tick();
        // Step other entities' 3-tick position lerps (vanilla cadence).
        self.world.entities.tick_lerp();
        if self.spawned {
            // Vanilla order: `LivingEntity.aiStep` pushes entities apart
            // *before* `travel`, so the shove lands in this tick's movement.
            self.push_from_entities();
            let collide = std::mem::take(&mut self.collide);
            let world = &self.world;
            let shapes = |x: i32, y: i32, z: i32| -> &[[f32; 6]] {
                let state = world.block_state_at(x, y, z);
                match collide.get(state as usize) {
                    Some(boxes) => boxes.as_slice(),
                    // No table (flat test worlds): non-air collides as a cube.
                    None if state != 0 => FULL_CUBE,
                    None => &[],
                }
            };
            physics::tick(&mut self.player, input, &shapes);
            self.collide = collide;
            self.send_movement(input)?;
        }
        self.ticks += 1;
        Ok(())
    }

    /// The local player's visual-effect tracker (night vision + darkness),
    /// read-only — the app builds a `rewo_world::lightmap::LightmapState` from
    /// this plus the day/night `SkyLighting`.
    pub fn visual_effects(&self) -> &crate::effects::VisualEffects {
        &self.visual_effects
    }

    /// A read-only snapshot of the camera lightmap effects at `partial`.
    pub fn visual_effect_snapshot(&self, partial: f32) -> crate::effects::VisualEffectSnapshot {
        self.visual_effects.snapshot(partial)
    }

    /// Decompiled `LocalPlayer.sendPosition` cadence + tick_end + input.
    fn send_movement(&mut self, input: &TickInput) -> Result<(), String> {
        // player_input on change (Input.STREAM_CODEC flag order).
        let flags = (input.forward > 0.0) as u8
            | (((input.forward < 0.0) as u8) << 1)
            | (((input.strafe > 0.0) as u8) << 2)
            | (((input.strafe < 0.0) as u8) << 3)
            | ((input.jump as u8) << 4)
            | ((input.sneak as u8) << 5)
            | ((input.sprint as u8) << 6);
        if flags != self.last_input_flags {
            if let Some(id) = self.ids.sb_play_player_input {
                let mut p = PacketWriter::packet(id);
                p.u8(flags);
                self.send(p)?;
            }
            self.last_input_flags = flags;
        }

        let (px, py, pz) = (self.player.x, self.player.y, self.player.z);
        let (yaw, pitch) = (self.player.yaw, self.player.pitch);
        let dx = px - self.last_pos.0;
        let dy = py - self.last_pos.1;
        let dz = pz - self.last_pos.2;
        self.reminder += 1;
        let moved = dx * dx + dy * dy + dz * dz > 4.0e-8 || self.reminder >= 20;
        let rotated = yaw != self.last_rot.0 || pitch != self.last_rot.1;
        let move_flags =
            self.player.on_ground as u8 | ((self.player.horizontal_collision as u8) << 1);

        if moved && rotated {
            let mut p = PacketWriter::packet(self.ids.sb_play_move_pos_rot);
            p.f64(px).f64(py).f64(pz).f32(yaw).f32(pitch).u8(move_flags);
            self.send(p)?;
        } else if moved {
            let mut p = PacketWriter::packet(self.ids.sb_play_move_pos);
            p.f64(px).f64(py).f64(pz).u8(move_flags);
            self.send(p)?;
        } else if rotated {
            let mut p = PacketWriter::packet(self.ids.sb_play_move_rot);
            p.f32(yaw).f32(pitch).u8(move_flags);
            self.send(p)?;
        } else if self.last_on_ground != self.player.on_ground
            || self.last_horiz != self.player.horizontal_collision
        {
            let mut p = PacketWriter::packet(self.ids.sb_play_move_status);
            p.u8(move_flags);
            self.send(p)?;
        }
        if moved {
            self.last_pos = (px, py, pz);
            self.reminder = 0;
        }
        if rotated {
            self.last_rot = (yaw, pitch);
        }
        self.last_on_ground = self.player.on_ground;
        self.last_horiz = self.player.horizontal_collision;

        if let Some(id) = self.ids.sb_play_client_tick_end {
            self.send(PacketWriter::packet(id))?;
        }
        Ok(())
    }

    fn drain_inbound(&mut self) -> Result<(), String> {
        loop {
            let packet = match self.rx.try_recv() {
                Ok(p) => p,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    if self.disconnect.is_none() {
                        self.disconnect = Some("connection closed".into());
                    }
                    return Ok(());
                }
            };
            let mut pos = 0;
            let Ok(id) = rewo_proto::varint::read_varint(&packet, &mut pos) else {
                continue;
            };
            self.handle_packet(id, &packet[pos..])?;
        }
    }

    fn handle_packet(&mut self, id: i32, body: &[u8]) -> Result<(), String> {
        let ids = &self.ids;
        if id == ids.cb_play_keep_alive {
            let mut r = PacketReader::new(body);
            if let Ok(v) = r.i64() {
                let mut p = PacketWriter::packet(self.ids.sb_play_keep_alive);
                p.i64(v);
                self.send(p)?;
            }
        } else if id == ids.cb_play_ping {
            let mut r = PacketReader::new(body);
            if let Ok(v) = r.i32() {
                let mut p = PacketWriter::packet(self.ids.sb_play_pong);
                p.i32(v);
                self.send(p)?;
            }
        } else if id == ids.cb_play_position {
            self.apply_teleport(body)?;
        } else if id == ids.cb_play_chunk_batch_finished {
            let mut p = PacketWriter::packet(self.ids.sb_play_chunk_batch_received);
            p.f32(64.0);
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
                Err(e) => log::error!("play: chunk decode failed: {e}"),
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
                    let Ok(buf) = r.byte_array(2_097_152) else { break };
                    let mut br = PacketReader::new(buf);
                    match rewo_world::chunk::read_chunk_biomes(&mut br, &shape, biome_bits) {
                        Ok(containers) => {
                            self.world.apply_chunks_biomes(cx, cz, containers);
                            self.mark_dirty_around(cx, cz);
                        }
                        Err(e) => log::error!("play: chunks_biomes decode failed: {e}"),
                    }
                }
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
            }
        } else if id == ids.cb_play_block_update {
            let mut r = PacketReader::new(body);
            if let (Ok((x, y, z)), Ok(state)) = (r.position(), r.varint()) {
                let old = self.world.block_state_at(x, y, z);
                self.world.set_block(x, y, z, state as u32);
                self.relight(x, y, z, old, state as u32);
                self.block_updates += 1;
                self.mark_dirty_around(x >> 4, z >> 4);
                log::debug!("net: block_update ({x},{y},{z}) = {state}");
            }
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
                for _ in 0..count.max(0) {
                    let Ok(packed) = r.varlong() else { break };
                    let packed = packed as u64;
                    let state = (packed >> 12) as u32;
                    let (ox, oy, oz) = unpack_section_offset((packed & 4095) as i32);
                    let (x, y, z) = (sx * 16 + ox, sy * 16 + oy, sz * 16 + oz);
                    let old = self.world.block_state_at(x, y, z);
                    self.world.set_block(x, y, z, state);
                    self.relight(x, y, z, old, state);
                    applied += 1;
                }
                self.block_updates += applied;
                self.mark_dirty_around(sx, sz);
                log::debug!("net: section_blocks_update ({sx},{sy},{sz}) × {applied}");
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
                apply_set_time(
                    &mut self.overworld_clock,
                    self.overworld_clock_id,
                    game_time,
                    &entries,
                );
                // Use the ported clock's total once it exists; before any real
                // clock state, fall back to `gameTime` (best-effort for a
                // server that never registers one).
                self.day_ticks = Some(match &self.overworld_clock {
                    Some(clock) => clock.total,
                    None => game_time,
                });
                log::debug!(
                    "net: set_time game={game_time} clocks={count} day_ticks={:?}",
                    self.day_ticks
                );
            }
        } else if Some(id) == ids.cb_play_block_ack {
            // Sequence ack — server confirms our predicted change. We don't
            // predict yet (M3 applies the server's block_update), so this is
            // just observed for the parity meter.
            log::debug!("net: block_changed_ack");
        } else if id == ids.cb_play_login {
            self.apply_login_shape(body);
            let p = PacketWriter::packet(self.ids.sb_play_player_loaded);
            self.send(p)?;
        } else if id == ids.cb_play_update_mob_effect {
            self.visual_effects.apply_update(body);
        } else if id == ids.cb_play_remove_mob_effect {
            self.visual_effects.apply_remove(body);
        } else if id == ids.cb_play_add_entity {
            let mut r = PacketReader::new(body);
            let _ = crate::read_add_entity(&mut r, &mut self.world);
        } else if id == ids.cb_play_remove_entities {
            let mut r = PacketReader::new(body);
            if let Ok(n) = r.count("remove entities", 1) {
                for _ in 0..n {
                    if let Ok(eid) = r.varint() {
                        self.world.entities.remove(eid);
                    }
                }
            }
        } else if id == ids.cb_play_move_entity_pos {
            let mut r = PacketReader::new(body);
            if let Ok((eid, dx, dy, dz)) = read_move_delta(&mut r) {
                if let Some(e) = self.world.entities.get_mut(eid) {
                    e.nudge(dx, dy, dz);
                }
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
            }
        } else if id == ids.cb_play_rotate_head {
            // varint id, yHeadRot (packed-degree byte). The server steers the
            // head toward nearby players, so this is what makes a mob watch you.
            let mut r = PacketReader::new(body);
            if let Ok(eid) = r.varint() {
                if let Ok(b) = r.i8() {
                    if let Some(e) = self.world.entities.get_mut(eid) {
                        e.set_head_yaw(packed_degrees(b));
                    }
                }
            }
        } else if id == ids.cb_play_set_entity_data {
            let mut r = PacketReader::new(body);
            if let Ok(eid) = r.varint() {
                let meta = crate::metadata::parse(&mut r);
                if meta.custom_name.is_some() {
                    self.world.entities.set_custom_name(eid, meta.custom_name);
                }
                if let Some(p) = meta.pose {
                    self.world.entities.set_pose(eid, p);
                }
                if let Some(s) = meta.gesture_state {
                    self.world.entities.set_gesture_state(eid, s);
                }
                if let Some(sz) = meta.size {
                    self.world.entities.set_size(eid, sz);
                }
                if let Some(baby) = meta.baby {
                    self.world.entities.set_baby(eid, baby);
                }
            }
        } else if id == ids.cb_play_player_info_update {
            self.apply_player_info(body);
        } else if id == ids.cb_play_player_info_remove {
            let mut r = PacketReader::new(body);
            if let Ok(n) = r.count("player info removes", 16) {
                for _ in 0..n {
                    if let Ok(uuid) = r.uuid() {
                        self.world.entities.remove_name(uuid);
                    }
                }
            }
        } else if Some(id) == ids.cb_play_set_health {
            let mut r = PacketReader::new(body);
            if let Ok(h) = r.f32() {
                self.health = h;
                // food (VarInt) + saturation (f32) follow.
                if let Ok(f) = r.varint() {
                    self.food = f;
                }
                if h <= 0.0 && !self.dead {
                    self.dead = true;
                    // client_command action 0 = perform respawn.
                    if let Some(cc) = self.ids.sb_play_client_command {
                        let mut p = PacketWriter::packet(cc);
                        p.varint(0);
                        self.send(p)?;
                        self.dead = false;
                    }
                }
            }
        } else if Some(id) == ids.cb_play_system_chat {
            let mut r = PacketReader::new(body);
            if let Ok(nbt) = r.nbt() {
                let text = nbt.to_plain_text();
                if !text.is_empty() {
                    self.chat_log.push(text);
                }
            }
        } else if Some(id) == ids.cb_play_player_chat {
            // Parse the useful prefix: sender uuid, index, optional
            // signature (256 bytes), then the body's message string.
            let mut r = PacketReader::new(body);
            let parse = (|| -> rewo_proto::Result<String> {
                let _global_index = r.varint()?;
                let _sender = r.uuid()?;
                let _index = r.varint()?;
                if r.bool()? {
                    r.skip(256)?;
                }
                r.string(256)
            })();
            if let Ok(msg) = parse {
                self.chat_log.push(msg);
            }
        } else if id == ids.cb_play_disconnect {
            let mut r = PacketReader::new(body);
            let reason = r.nbt().map(|n| n.to_plain_text()).unwrap_or_default();
            self.disconnect = Some(reason);
        }
        Ok(())
    }

    fn apply_teleport(&mut self, body: &[u8]) -> Result<(), String> {
        let mut r = PacketReader::new(body);
        let parse = (|| -> rewo_proto::Result<(i32, [f64; 6], f32, f32, i32)> {
            let id = r.varint()?;
            let mut vals = [0.0f64; 6];
            for v in vals.iter_mut() {
                *v = r.f64()?;
            }
            let yaw = r.f32()?;
            let pitch = r.f32()?;
            let relatives = r.i32()?;
            Ok((id, vals, yaw, pitch, relatives))
        })();
        let Ok((teleport_id, vals, yaw, pitch, relatives)) = parse else {
            return Ok(());
        };
        let rel = |bit: i32| relatives & (1 << bit) != 0;
        // Relative bits (decompiled Relative enum order): X=0 Y=1 Z=2
        // Y_ROT=3 X_ROT=4, deltas 5..7, rotate-delta 8.
        self.player.x = if rel(0) {
            self.player.x + vals[0]
        } else {
            vals[0]
        };
        self.player.y = if rel(1) {
            self.player.y + vals[1]
        } else {
            vals[1]
        };
        self.player.z = if rel(2) {
            self.player.z + vals[2]
        } else {
            vals[2]
        };
        self.player.yaw = if rel(3) { self.player.yaw + yaw } else { yaw };
        self.player.pitch = if rel(4) {
            self.player.pitch + pitch
        } else {
            pitch
        };
        self.player.vx = if rel(5) {
            self.player.vx + vals[3]
        } else {
            vals[3]
        };
        self.player.vy = if rel(6) {
            self.player.vy + vals[4]
        } else {
            vals[4]
        };
        self.player.vz = if rel(7) {
            self.player.vz + vals[5]
        } else {
            vals[5]
        };

        let mut ack = PacketWriter::packet(self.ids.sb_play_accept_teleport);
        ack.varint(teleport_id);
        self.send(ack)?;
        // Vanilla immediately reports the accepted position back.
        let mut p = PacketWriter::packet(self.ids.sb_play_move_pos_rot);
        p.f64(self.player.x)
            .f64(self.player.y)
            .f64(self.player.z)
            .f32(self.player.yaw)
            .f32(self.player.pitch)
            .u8(0);
        self.send(p)?;
        self.last_pos = (self.player.x, self.player.y, self.player.z);
        self.last_rot = (self.player.yaw, self.player.pitch);

        self.teleports += 1;
        if self.spawned {
            self.corrections += 1;
        }
        self.spawned = true;
        Ok(())
    }

    /// Player Info Update (decompiled `ClientboundPlayerInfoUpdatePacket`):
    /// a 1-byte fixed bitset over the 8 actions (LSB-first, ordinal order),
    /// then a VarInt list of entries [uuid + per-set-action fields]. We keep
    /// ADD_PLAYER's name (nametags) and skip everything else byte-exactly —
    /// a mis-skip here corrupts the rest of the packet's entries.
    fn apply_player_info(&mut self, body: &[u8]) {
        let mut r = PacketReader::new(body);
        let mut names: Vec<(u128, String)> = Vec::new();
        let mut skins: Vec<(u128, crate::skins::SkinInfo)> = Vec::new();
        let parse = (|| -> rewo_proto::Result<()> {
            let mask = r.u8()?;
            let has = |bit: u8| mask & (1u8 << bit) != 0;
            let count = r.count("player info entries", 16)?;
            for _ in 0..count {
                let uuid = r.uuid()?;
                if has(0) {
                    // ADD_PLAYER: name + profile properties (skin blobs).
                    let name = r.string(16)?.to_string();
                    let props = r.count("profile properties", 1)?;
                    for _ in 0..props {
                        let prop = r.string(64)?;
                        let value = r.string(32767)?;
                        // Decode the `textures` property → skin URL + model,
                        // so a player renders with their real skin.
                        if prop == "textures" {
                            if let Some(info) = crate::skins::decode_textures_property(&value) {
                                skins.push((uuid, info));
                            }
                        }
                        if r.bool()? {
                            let _sig = r.string(1024)?;
                        }
                    }
                    names.push((uuid, name));
                }
                if has(1) {
                    // INITIALIZE_CHAT: nullable {session uuid, expires i64,
                    // pubkey bytes ≤512, key sig bytes ≤4096}.
                    if r.bool()? {
                        let _ = r.uuid()?;
                        let _ = r.i64()?;
                        let _ = r.byte_array(512)?;
                        let _ = r.byte_array(4096)?;
                    }
                }
                if has(2) {
                    let _gamemode = r.varint()?;
                }
                if has(3) {
                    let _listed = r.bool()?;
                }
                if has(4) {
                    let _latency = r.varint()?;
                }
                if has(5) {
                    // UPDATE_DISPLAY_NAME: nullable NBT text component.
                    if r.bool()? {
                        let _ = r.nbt()?;
                    }
                }
                if has(6) {
                    let _list_order = r.varint()?;
                }
                if has(7) {
                    let _show_hat = r.bool()?;
                }
            }
            Ok(())
        })();
        if let Err(e) = parse {
            log::debug!("play: player_info_update parse: {e}");
        }
        for (uuid, name) in names {
            self.world.entities.set_name(uuid, name);
        }
        for (uuid, info) in skins {
            // Queue only genuinely-new skins so the app fetches each once.
            if self.player_skins.get(&uuid) != Some(&info) {
                self.player_skins.insert(uuid, info.clone());
                self.pending_skins.push((uuid, info));
            }
        }
    }

    fn apply_login_shape(&mut self, body: &[u8]) {
        let mut r = PacketReader::new(body);
        // The first `int` is the local player's entity id (big-endian, NOT a
        // varint). Only effects targeting it drive the camera lightmap. If it is
        // truncated the reader is already past the buffer, so there is nothing
        // to parse — bail rather than run the dimension-holder parse on garbage.
        let Ok(player_id) = r.i32() else {
            return;
        };
        self.visual_effects.set_player_id(player_id);
        // Parse the login prefix + `CommonPlayerSpawnInfo`. The dimension-type
        // holder is `DimensionType.STREAM_CODEC = ByteBufCodecs.holderRegistry`
        // = `idMapper` — it writes the **raw 0-based registry id**, with NO
        // inline/`id+1` case (unlike `ByteBufCodecs.holder`). So the id is a
        // direct index and the dimension ResourceKey + `long seed`
        // (= `biomeZoomSeed`) always follow it immediately.
        let parse = (|| -> rewo_proto::Result<(i32, i64)> {
            r.bool()?; // hardcore
            let n = r.count("dims", 1)?;
            for _ in 0..n {
                r.identifier()?;
            }
            r.varint()?; // max players
            r.varint()?; // view dist
            r.varint()?; // sim dist
            r.bool()?; // reduced debug
            r.bool()?; // show death
            r.bool()?; // limited crafting
            let holder = r.varint()?; // dimension-type raw registry id (0-based)
            r.identifier()?; // dimension ResourceKey
            let seed = r.i64()?; // biomeZoomSeed
            Ok((holder, seed))
        })();
        if let Ok((holder, seed)) = parse {
            self.world.shape = crate::login_dimension_shape(holder, &self.dim_shapes);
            self.biome_zoom_seed = Some(seed);
            self.build_biome_context(holder, seed);
        }
    }

    /// Build the `BiomeContext` from the parsed registry + this dimension's base
    /// sky/fog + the `biomeZoomSeed`, and attach it to the world. `holder` is a
    /// raw 0-based dimension-type id (see `apply_login_shape`). The registry is
    /// cloned (not consumed) so this is idempotent; respawn is not yet wired to
    /// call it, so a mid-session dimension change keeps the join dimension's
    /// base sky/fog.
    fn build_biome_context(&mut self, holder: i32, seed: i64) {
        let Some(reg_template) = self.pending_biome_registry.as_ref() else {
            return;
        };
        let mut reg = reg_template.clone();
        if holder >= 0 {
            if let Some((sky, fog)) = self.dim_attrs.get(holder as usize).copied() {
                reg.dimension_sky = sky;
                reg.dimension_fog = fog;
            }
        }
        let ctx = rewo_world::biome::BiomeContext::new(
            std::sync::Arc::new(reg),
            self.colormaps.clone(),
            seed,
        );
        self.world.set_biome_context(std::sync::Arc::new(ctx));
        log::info!("net: biome context attached (seed={seed})");
    }

    // -- gameplay actions --------------------------------------------------

    /// Announce the chat session (public certificate + session id) so
    /// signed chat is accepted. Wire (decompiled
    /// `ServerboundChatSessionUpdatePacket` → `RemoteChatSession.Data` →
    /// `ProfilePublicKey.Data`): sessionId UUID, expiry Instant, pubkey
    /// byte-array, Mojang key-signature byte-array.
    fn announce_chat_session(&mut self) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_chat_session_update else {
            return Err("chat_session_update packet unavailable".into());
        };
        let signer = self.signer.as_ref().expect("signer set before announce");
        let mut p = PacketWriter::packet(id);
        p.uuid(signer.session_id);
        p.i64(signer.expires_at_ms);
        p.varint(signer.public_key_der.len() as i32)
            .raw(&signer.public_key_der);
        p.varint(signer.key_signature.len() as i32)
            .raw(&signer.key_signature);
        self.send(p)
    }

    pub fn send_chat(&mut self, message: &str) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_chat else {
            return Err("chat packet unavailable".into());
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let millis = now.as_millis() as i64;
        // A signature commits to the seconds-precision timestamp + a random
        // salt; both must go on the wire exactly as signed.
        let (salt, signature) = match self.signer.as_mut() {
            Some(signer) => {
                let mut salt_bytes = [0u8; 8];
                rand::Rng::fill(&mut rand::thread_rng(), &mut salt_bytes);
                let salt = i64::from_be_bytes(salt_bytes);
                let sig = signer.sign(message, salt, now.as_secs() as i64, &[]);
                (salt, Some(sig))
            }
            None => (0, None),
        };
        let mut p = PacketWriter::packet(id);
        p.string(message).i64(millis).i64(salt);
        match &signature {
            Some(sig) => {
                p.bool(true).raw(sig); // MessageSignature: fixed 256 bytes
            }
            None => {
                p.bool(false);
            }
        }
        p.varint(0); // last-seen offset
        p.raw(&[0, 0, 0]); // FixedBitSet(20) acknowledged — none
        p.u8(0); // checksum 0 = skip verification
        self.send(p)
    }

    /// Creative: put `count` of item `item_id` into hotbar `slot` (0..9).
    /// Inventory slot index for hotbar N is 36 + N.
    pub fn creative_set_hotbar(
        &mut self,
        slot: u8,
        item_id: i32,
        count: i32,
    ) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_set_creative_slot else {
            return Err("set_creative_mode_slot unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.u16(36 + slot as u16); // i16 slot
        p.varint(count); // ItemStack: count
        if count > 0 {
            p.varint(item_id);
            p.varint(0); // added components
            p.varint(0); // removed components
        }
        self.send(p)
    }

    pub fn select_hotbar(&mut self, slot: u8) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_set_carried_item else {
            return Err("set_carried_item unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.u16(slot as u16);
        self.send(p)
    }

    /// Start digging (creative servers break the block on START).
    /// Run a server command (unsigned `chat_command`, the string without the
    /// leading `/`). Used for verification (`/summon …`) when the account is
    /// op; a normal client mostly sends these too.
    pub fn send_command(&mut self, command: &str) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_chat_command else {
            return Err("chat_command unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.string(command);
        self.send(p)
    }

    /// The block the eye is looking at (voxel raycast against `solid`), or
    /// `None`. `dir` need not be normalized; `reach` in blocks (~4.5 creative).
    pub fn target_block(
        &self,
        eye: [f64; 3],
        dir: [f64; 3],
        reach: f64,
    ) -> Option<rewo_world::raycast::RayHit> {
        let world = &self.world;
        let collide = &self.collide;
        rewo_world::raycast::cast(eye, dir, reach, |x, y, z| {
            let s = world.block_state_at(x, y, z);
            // Targetable = has any collision shape (so slabs/stairs are
            // mineable), with the same non-air fallback as physics.
            match collide.get(s as usize) {
                Some(boxes) => !boxes.is_empty(),
                None => s != 0,
            }
        })
    }

    pub fn start_dig(&mut self, x: i32, y: i32, z: i32, face: u8) -> Result<(), String> {
        let seq = self.next_sequence();
        let mut p = PacketWriter::packet(self.ids.sb_play_player_action);
        p.varint(0); // START_DESTROY_BLOCK
        write_position(&mut p, x, y, z);
        p.u8(face);
        p.varint(seq);
        self.send(p)?;
        self.swing()
    }

    /// Place the held item against (x,y,z)'s `face`.
    pub fn use_item_on(&mut self, x: i32, y: i32, z: i32, face: u8) -> Result<(), String> {
        let seq = self.next_sequence();
        let Some(id) = self.ids.sb_play_use_item_on else {
            return Err("use_item_on unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.varint(0); // main hand
        write_position(&mut p, x, y, z);
        p.varint(face as i32); // Direction enum
        p.f32(0.5).f32(1.0).f32(0.5); // click offset on the face
        p.bool(false); // inside
        p.bool(false); // world border hit
        p.varint(seq);
        self.send(p)?;
        self.swing()
    }

    pub fn attack_entity(&mut self, entity_id: i32) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_interact else {
            return Err("interact unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.varint(entity_id);
        p.varint(1); // ATTACK
        p.bool(false); // not sneaking
        self.send(p)?;
        self.swing()
    }

    pub fn swing(&mut self) -> Result<(), String> {
        if let Some(id) = self.ids.sb_play_swing {
            let mut p = PacketWriter::packet(id);
            p.varint(0); // main hand
            self.send(p)?;
        }
        Ok(())
    }
}

/// The overworld world clock, ported from 26.2 `ClientClockManager.WorldClock`.
/// The renderer reads `total`; `partial`/`rate`/`last_game_time` are the
/// integration state that lets an empty `set_time` map advance the clock.
///
/// `partial` is stored as `f32` to match vanilla `ClockInstance.partialTick`
/// exactly. Each `advance` widens it to `f64` for the multiply-and-floor, then
/// truncates the remainder back to `f32` (`(float)(newPartialTicks - fullTicks)`)
/// — keeping the storage width identical to vanilla means the fractional carry
/// rounds bit-for-bit the way the client does, rather than accumulating extra
/// precision the client never keeps. `total`/`last_game_time` stay exact.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WorldClock {
    total: i64,
    partial: f32,
    rate: f32,
    last_game_time: i64,
}

impl WorldClock {
    /// Establish a clock from an explicit wire state observed at `game_time`
    /// (`WorldClock.Data` → the stored clock).
    fn from_state(game_time: i64, total: i64, partial: f32, rate: f32) -> Self {
        WorldClock {
            total,
            partial,
            rate,
            last_game_time: game_time,
        }
    }

    /// Vanilla `ClientClockManager.tick`, transcribed exactly:
    ///
    /// ```text
    /// long   gameTimeDelta   = gameTime - lastTickGameTime;         // long: wraps
    /// double newPartialTicks = instance.partialTick + (double)gameTimeDelta * instance.rate;
    /// long   fullTicks       = Mth.floor(newPartialTicks);          // Mth.floor returns int, widened to long
    /// instance.partialTick   = (float)(newPartialTicks - fullTicks);
    /// instance.totalTicks   += fullTicks;                           // long: wraps
    /// ```
    ///
    /// Three primitive-semantics details are load-bearing, each with a matching
    /// Rust idiom:
    ///
    /// * `gameTime - lastTickGameTime` is `long` subtraction and wraps
    ///   two's-complement; `wrapping_sub` matches it (and dodges a debug-overflow
    ///   panic) before the delta is widened to `f64`.
    /// * `Mth.floor(double)` returns a Java **`int`** (`(int)Math.floor(v)`),
    ///   which `long fullTicks` then widens. The `double → int` narrowing
    ///   saturates to `i32::MIN`/`i32::MAX` and maps `NaN` to `0` — semantics
    ///   Rust's `as i32` cast reproduces exactly. Flooring straight to `i64`
    ///   would instead saturate to the far larger `i64` bounds, diverging for
    ///   out-of-`i32`-range or `NaN` `newPartialTicks`, so we floor to `i32`
    ///   first and widen. The `(float)(newPartialTicks - fullTicks)` remainder is
    ///   likewise taken against that `i32`-derived `fullTicks` (widened back to
    ///   `double`), not the true `f64` floor — so an out-of-range value carries
    ///   the same (possibly `±inf`) `f32` the client keeps.
    /// * `totalTicks += fullTicks` is `long` addition and wraps; `wrapping_add`
    ///   matches it.
    ///
    /// The stored `f32` `partial` is widened to `f64` for the multiply-and-floor,
    /// then the remainder is truncated back to `f32` — the widen-compute-narrow
    /// round-trip is what keeps the carry bit-identical to the client. `Mth.floor`
    /// and `f64::floor` both round toward negative infinity, so a negative `rate`
    /// borrows a whole tick from `total` and leaves a positive carry. A `rate` of
    /// 0 (paused world) leaves `total` unchanged; a fractional `rate` accumulates
    /// until a whole tick rolls over.
    fn advance(&mut self, game_time: i64) {
        // `gameTime - lastTickGameTime` is `long` arithmetic; wrap it the same
        // way (and avoid a debug-overflow panic) before widening to `f64`.
        let delta = game_time.wrapping_sub(self.last_game_time) as f64;
        let new_partial = self.partial as f64 + delta * self.rate as f64;
        // `Mth.floor` returns an `int`, so narrow to `i32` (saturating, NaN → 0)
        // before widening to the `long fullTicks`. A direct `as i64` would
        // saturate to the wrong bounds for out-of-`i32`-range / NaN inputs.
        let full_i32 = new_partial.floor() as i32;
        let full = i64::from(full_i32);
        // `(float)(newPartialTicks - fullTicks)`: the remainder is against the
        // i32-derived `fullTicks` (widened to `double`), exactly like Java.
        self.partial = (new_partial - full as f64) as f32;
        // `totalTicks += fullTicks` is wrapping `long` addition.
        self.total = self.total.wrapping_add(full);
        self.last_game_time = game_time;
    }
}

/// Apply one `set_time` to the overworld clock, in vanilla
/// `ClientClockManager.handleUpdates` order: **advance** the stored clock by the
/// game-time delta first (so an empty update still moves the day/night cycle),
/// then let any explicit overworld clock state in the packet **overwrite** it.
/// Matching this order is load-bearing: a packet that both syncs and re-sets the
/// clock must land on the explicit value, not the advanced one.
fn apply_set_time(
    clock: &mut Option<WorldClock>,
    overworld_id: Option<i32>,
    game_time: i64,
    entries: &[(i32, i64, f32, f32)],
) {
    if let Some(c) = clock.as_mut() {
        c.advance(game_time);
    }
    for &(holder, total, partial, rate) in entries {
        if Some(holder) == overworld_id {
            *clock = Some(WorldClock::from_state(game_time, total, partial, rate));
        }
    }
}

/// One `ClientLevel.tickTime`, transcribed: read the stored game-time, bump it
/// by one (`clientLevelData.getGameTime() + 1L`), advance the overworld clock
/// against the new value (`ClientClockManager.tick(gameTime)`), and hand back
/// the game-time to store plus the day-tick the renderer should read.
///
/// The `+ 1L` is `long` addition and wraps two's-complement, so `wrapping_add`
/// matches it. A `None` game-time means no `set_time` has arrived yet — vanilla
/// only runs `tickTime` once the level exists — so there is nothing to tick and
/// this is a no-op (`None`), leaving the renderer on full daylight.
///
/// This is deliberately the twin of `apply_set_time` (the `handleUpdates`
/// path), not a merge of it: vanilla runs both against the same clock on any
/// tick a sync arrives — the sync advances-then-reanchors the clock to the
/// server value, and this local `+1` then continues from it. Because each
/// `advance` re-bases its delta on `last_game_time`, running both is not
/// double-counting: an empty sync at the already-predicted game time advances
/// the clock by zero, leaving exactly this one local `+1`. If no explicit
/// overworld clock exists yet, the day-tick falls back to the raw game time
/// (best-effort), still advancing one per tick rather than only on packets.
fn local_tick_time(game_time: Option<i64>, clock: &mut Option<WorldClock>) -> Option<(i64, i64)> {
    let next = game_time?.wrapping_add(1);
    if let Some(c) = clock.as_mut() {
        c.advance(next);
    }
    let day = clock.as_ref().map(|c| c.total).unwrap_or(next);
    Some((next, day))
}

fn write_position(p: &mut PacketWriter, x: i32, y: i32, z: i32) {
    let v: u64 = (((x as i64 & 0x3ff_ffff) as u64) << 38)
        | (((z as i64 & 0x3ff_ffff) as u64) << 12)
        | ((y as i64 & 0xfff) as u64);
    p.i64(v as i64);
}

/// The move-entity delta prefix: varint id + 3 shorts, each Δpos·4096
/// (decompiled `VecDeltaCodec` — deltas accumulate on the last transmitted
/// position, which is exactly what `EntityState::nudge` targets).
fn read_move_delta(r: &mut PacketReader) -> rewo_proto::Result<(i32, f64, f64, f64)> {
    let eid = r.varint()?;
    let dx = r.i16()? as f64 / 4096.0;
    let dy = r.i16()? as f64 / 4096.0;
    let dz = r.i16()? as f64 / 4096.0;
    Ok((eid, dx, dy, dz))
}

/// `Mth.unpackDegrees`: packed byte angle → degrees.
fn packed_degrees(b: i8) -> f32 {
    b as f32 * (360.0 / 256.0)
}

/// The unit cube, for blocks with no entry in the collision table.
static FULL_CUBE: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];

impl PlaySession {
    /// Vanilla `Entity.push`: entities whose bounding boxes overlap shove each
    /// other apart horizontally. Only the *player* is moved here — the server
    /// owns every other entity's position, so pushing them client-side would
    /// just be corrected away.
    ///
    /// The math is vanilla's verbatim, including its quirk: `dd` is
    /// `absMax(xa, za)` and then **square-rooted** — the sqrt of the larger
    /// component, not the vector length. Porting it as a true normalize would
    /// give a different push strength.
    fn push_from_entities(&mut self) {
        if self.entity_push.is_empty() {
            return;
        }
        let (px, py, pz) = (self.player.x, self.player.y, self.player.z);
        let phw = rewo_world::physics::PLAYER_HALF_WIDTH;
        let ph = rewo_world::physics::PLAYER_HEIGHT;
        let (mut ax, mut az) = (0.0f64, 0.0f64);
        for (_id, e) in self.world.entities.iter() {
            let Some(&(w, h, pushable)) = self.entity_push.get(e.type_id.max(0) as usize) else {
                continue;
            };
            if !pushable {
                continue;
            }
            let hw = w as f64 * 0.5;
            // Bounding-box overlap — vanilla selects entities intersecting ours.
            if e.x + hw <= px - phw || e.x - hw >= px + phw {
                continue;
            }
            if e.z + hw <= pz - phw || e.z - hw >= pz + phw {
                continue;
            }
            if e.y + h as f64 <= py || e.y >= py + ph {
                continue;
            }
            let (xa, za) = push_delta(e.x - px, e.z - pz);
            // `this.push(-xa, 0, -za)` — we move away from them.
            ax -= xa;
            az -= za;
        }
        self.player.vx += ax;
        self.player.vz += az;
    }
}

/// The horizontal shove one entity applies to another, from vanilla
/// `Entity.push(Entity)`. `dx`/`dz` are `other − self`; the result is the
/// impulse applied to *other* (self gets its negation).
///
/// Verbatim vanilla, quirk included: `dd` is `absMax(dx, dz)` and is then
/// **square-rooted** — the sqrt of the larger component, not the vector
/// length. Writing it as a normalize (the "obvious" reading) changes the push
/// strength, so it is kept literal.
fn push_delta(dx: f64, dz: f64) -> (f64, f64) {
    /// `Entity.push` scales the unit shove by this (vanilla `0.05F`).
    const PUSH_SPEED: f64 = 0.05;
    let dd = dx.abs().max(dz.abs());
    if dd < 0.01 {
        return (0.0, 0.0);
    }
    let dd = dd.sqrt();
    let pow = (1.0 / dd).min(1.0);
    (dx / dd * pow * PUSH_SPEED, dz / dd * pow * PUSH_SPEED)
}

#[cfg(test)]
mod push_tests {
    use super::push_delta;

    /// Below vanilla's 0.01 threshold nothing happens (exactly-overlapping
    /// entities would otherwise divide by ~0).
    #[test]
    fn coincident_entities_do_not_push() {
        assert_eq!(push_delta(0.0, 0.0), (0.0, 0.0));
        assert_eq!(push_delta(0.005, -0.004), (0.0, 0.0));
    }

    /// Vanilla's math, computed by hand for a 1-block separation along +x:
    /// dd = sqrt(absMax(1,0)) = 1, pow = min(1, 1/1) = 1,
    /// so the impulse is 1/1 * 1 * 0.05 = 0.05 on x and 0 on z.
    #[test]
    fn unit_separation_matches_vanilla() {
        let (x, z) = push_delta(1.0, 0.0);
        assert!((x - 0.05).abs() < 1e-12, "x={x}");
        assert_eq!(z, 0.0);
    }

    /// The push is directional and symmetric under negation.
    #[test]
    fn push_is_antisymmetric() {
        let (ax, az) = push_delta(0.4, -0.3);
        let (bx, bz) = push_delta(-0.4, 0.3);
        assert!((ax + bx).abs() < 1e-12 && (az + bz).abs() < 1e-12);
        assert!(ax > 0.0 && az < 0.0, "points away along the separation");
    }

    /// Closer than one block, `pow = 1/dd > 1` is clamped to 1 — so the shove
    /// never exceeds PUSH_SPEED in magnitude per axis component.
    #[test]
    fn close_range_push_is_clamped() {
        let (x, z) = push_delta(0.05, 0.0);
        assert!(x <= 0.05 + 1e-12, "clamped, got {x}");
        assert!(x > 0.0 && z == 0.0);
    }
}

/// Unpack `SectionPos.asLong`: x in bits 42..63, z in 20..41, y in 0..19.
///
/// All three are signed and two are narrower than a register, so each is
/// shifted left to put its sign bit at the top before the arithmetic shift
/// right sign-extends it. Getting this wrong places edits in a different
/// chunk, which reads as "some blocks never update".
fn unpack_section_pos(packed: u64) -> (i32, i32, i32) {
    let v = packed as i64;
    (
        (v >> 42) as i32,
        ((v << 44) >> 44) as i32,
        ((v << 22) >> 42) as i32,
    )
}

/// Unpack a 12-bit in-section position: **x is bits 8..11, z is 4..7, y is
/// 0..3** (`SectionPos.sectionRelative{X,Y,Z}`). Note the order — y is the
/// low nibble, not x.
fn unpack_section_offset(pos: i32) -> (i32, i32, i32) {
    ((pos >> 8) & 15, pos & 15, (pos >> 4) & 15)
}

#[cfg(test)]
mod section_update_tests {
    use super::*;

    /// Mirrors `SectionPos.asLong`, so the test states the encoding
    /// independently of the decoder under test.
    fn as_long(x: i64, y: i64, z: i64) -> u64 {
        (((x & 0x3F_FFFF) << 42) | (y & 0xF_FFFF) | ((z & 0x3F_FFFF) << 20)) as u64
    }

    #[test]
    fn section_pos_roundtrips_including_negatives() {
        for (x, y, z) in [
            (0, 0, 0),
            (1, 2, 3),
            (-1, -1, -1),
            (-3000, -4, 2999),
            (100, 19, -100),
        ] {
            assert_eq!(
                unpack_section_pos(as_long(x as i64, y as i64, z as i64)),
                (x, y, z),
                "section ({x},{y},{z})"
            );
        }
    }

    #[test]
    fn section_offset_uses_the_x_z_y_nibble_order() {
        // Vanilla packs `x << 8 | z << 4 | y`.
        for (x, y, z) in [(0, 0, 0), (15, 15, 15), (1, 2, 3), (9, 4, 7)] {
            let packed = (x << 8) | (z << 4) | y;
            assert_eq!(
                unpack_section_offset(packed),
                (x, y, z),
                "offset ({x},{y},{z})"
            );
        }
    }

    #[test]
    fn a_change_entry_splits_into_state_and_position() {
        // Wire form: `stateId << 12 | posInSection`.
        let packed: u64 = (1234u64 << 12) | ((5 << 8) | (6 << 4) | 7);
        assert_eq!(packed >> 12, 1234);
        assert_eq!(unpack_section_offset((packed & 4095) as i32), (5, 7, 6));
    }
}

#[cfg(test)]
mod clock_tests {
    use super::{apply_set_time, local_tick_time, WorldClock};

    const OVERWORLD: Option<i32> = Some(0);

    /// The join packet establishes the clock from an explicit state — total and
    /// last-game-time come straight from the wire. (The real server session
    /// showed `game=138341` establishing `total=189121`.)
    #[test]
    fn initial_explicit_state_establishes_the_clock() {
        let mut clock = None;
        apply_set_time(&mut clock, OVERWORLD, 138341, &[(0, 189121, 0.0, 1.0)]);
        let c = clock.expect("clock established");
        assert_eq!(c.total, 189121);
        assert_eq!(c.last_game_time, 138341);
        assert_eq!(c.partial, 0.0);
        assert_eq!(c.rate, 1.0);
    }

    /// The 20-tick `forceGameTimeSynchronization` sync carries an EMPTY map; at
    /// rate 1 it must advance `total` by the exact game-time delta. This is the
    /// frozen-clock regression: the old code held the last total here.
    #[test]
    fn empty_map_advances_by_the_game_time_delta_at_rate_one() {
        let mut clock = Some(WorldClock::from_state(138341, 189121, 0.0, 1.0));
        // The real diagnostic's game times after the join, deltas 3/20/20/20.
        for (game_time, expected_total) in [
            (138344, 189124),
            (138364, 189144),
            (138384, 189164),
            (138404, 189184),
        ] {
            apply_set_time(&mut clock, OVERWORLD, game_time, &[]);
            let c = clock.unwrap();
            assert_eq!(c.total, expected_total, "at game {game_time}");
            assert_eq!(c.last_game_time, game_time);
        }
    }

    /// A paused world (`/tick freeze`, or `doDaylightCycle false` reported as
    /// rate 0) must NOT advance on empty syncs, however large the delta.
    #[test]
    fn paused_rate_zero_holds_total() {
        let mut clock = Some(WorldClock::from_state(1000, 500, 0.0, 0.0));
        apply_set_time(&mut clock, OVERWORLD, 1020, &[]);
        apply_set_time(&mut clock, OVERWORLD, 5000, &[]);
        let c = clock.unwrap();
        assert_eq!(c.total, 500, "paused clock frozen");
        assert_eq!(c.last_game_time, 5000, "still anchors to gameTime");
    }

    /// A fractional rate proves the floor + partial carry: at rate 0.5 a
    /// single-tick advance banks half a tick, and the second single tick rolls
    /// the carry over into one whole `total` tick.
    #[test]
    fn fractional_rate_floors_and_carries_the_remainder() {
        let mut clock = Some(WorldClock::from_state(0, 0, 0.0, 0.5));

        apply_set_time(&mut clock, OVERWORLD, 1, &[]); // +0.5 → floor 0, carry 0.5
        let c = clock.unwrap();
        assert_eq!(c.total, 0, "half a tick banks nothing yet");
        assert!((c.partial - 0.5).abs() < 1e-9, "carry {}", c.partial);

        apply_set_time(&mut clock, OVERWORLD, 2, &[]); // 0.5 + 0.5 = 1.0 → floor 1
        let c = clock.unwrap();
        assert_eq!(c.total, 1, "carry rolls into one whole tick");
        assert!(c.partial.abs() < 1e-9, "carry reset, got {}", c.partial);
    }

    /// A negative `rate` (a clock running backward) proves the floor rounds
    /// toward negative infinity, not toward zero: starting at partial 0.25, one
    /// tick at rate -0.5 gives newPartial -0.25, `floor(-0.25) == -1` (NOT 0), so
    /// `total` BORROWS a whole tick (10 → 9) and the remainder is a POSITIVE
    /// `-0.25 - (-1) == 0.75`. A truncate-toward-zero floor would wrongly leave
    /// `total` at 10 with a negative carry.
    #[test]
    fn negative_rate_floors_toward_negative_infinity_with_positive_carry() {
        let mut clock = Some(WorldClock::from_state(0, 10, 0.25, -0.5));

        apply_set_time(&mut clock, OVERWORLD, 1, &[]); // 0.25 - 0.5 = -0.25 → floor -1
        let c = clock.unwrap();
        assert_eq!(c.total, 9, "borrowed one whole tick from total");
        assert!(
            (c.partial - 0.75).abs() < 1e-6,
            "positive carry {}",
            c.partial
        );
        assert_eq!(c.last_game_time, 1);
    }

    /// Vanilla `handleUpdates` order: a packet that both advances (non-empty
    /// game-time delta) AND carries an explicit overworld state must land on the
    /// explicit value — the advance happens first and is then overwritten.
    #[test]
    fn explicit_state_overwrites_after_the_advance() {
        let mut clock = Some(WorldClock::from_state(100, 5000, 0.0, 1.0));
        // Delta 20 would advance to 5020, but the explicit reset wins.
        apply_set_time(&mut clock, OVERWORLD, 120, &[(0, 0, 0.0, 1.0)]);
        let c = clock.unwrap();
        assert_eq!(c.total, 0, "explicit /time set overrides the advance");
        assert_eq!(c.last_game_time, 120);
    }

    /// The_end's clock is present in the map but must not touch the overworld —
    /// entries are matched by registry id, and the overworld still advances.
    #[test]
    fn a_non_overworld_entry_only_advances_the_overworld() {
        let mut clock = Some(WorldClock::from_state(100, 5000, 0.0, 1.0));
        // id 1 is the_end; the overworld advances by the delta, id 1 ignored.
        apply_set_time(&mut clock, OVERWORLD, 120, &[(1, 999, 0.0, 1.0)]);
        assert_eq!(
            clock.unwrap().total,
            5020,
            "overworld advanced, the_end skipped"
        );
    }

    /// `Mth.floor` returns a Java `int`, so a `newPartialTicks` past `i32::MAX`
    /// must saturate `fullTicks` to `i32::MAX` — NOT the far larger `i64::MAX` a
    /// direct `f64 as i64` cast would give. At rate 1 a 5-billion-tick delta
    /// (well past `i32::MAX ≈ 2.147e9`) banks exactly `i32::MAX` whole ticks and
    /// carries the ~2.85e9 remainder against that saturated value. The buggy
    /// direct-i64 path would instead bank the full 5e9 and carry 0.
    #[test]
    fn huge_positive_partial_saturates_full_to_i32_max() {
        let mut clock = Some(WorldClock::from_state(0, 0, 0.0, 1.0));
        // delta = 5_000_000_000 (exact in f64), newPartialTicks = 5e9.
        apply_set_time(&mut clock, OVERWORLD, 5_000_000_000, &[]);
        let c = clock.unwrap();
        assert_eq!(
            c.total,
            i64::from(i32::MAX),
            "double→int saturates to i32::MAX, not i64::MAX (direct i64 would be 5e9)"
        );
        assert!(
            c.partial.is_finite() && c.partial > 2.8e9,
            "remainder taken against the i32-saturated full, not the true floor (got {})",
            c.partial
        );
        assert_eq!(c.last_game_time, 5_000_000_000);
    }

    /// A `NaN` rate poisons the arithmetic: `newPartialTicks` is `NaN`,
    /// `Mth.floor`'s `double→int` narrowing maps `NaN` to `0` (so `total` holds),
    /// and the `(float)(NaN - 0)` carry is `NaN`. This must not panic.
    #[test]
    fn nan_rate_floors_to_zero_and_poisons_the_carry() {
        let mut clock = Some(WorldClock::from_state(0, 100, 0.0, f32::NAN));
        apply_set_time(&mut clock, OVERWORLD, 1, &[]);
        let c = clock.unwrap();
        assert_eq!(
            c.total, 100,
            "NaN floors to 0 (NaN→int is 0), total unchanged"
        );
        assert!(
            c.partial.is_nan(),
            "NaN rate poisons the carry, got {}",
            c.partial
        );
        assert_eq!(c.last_game_time, 1);
    }

    /// `gameTime - lastTickGameTime` is `long` subtraction that wraps
    /// two's-complement — `i64::MAX - i64::MIN` wraps to `-1`, not a debug panic.
    /// At rate 1 that -1 delta borrows exactly one whole tick from `total`.
    #[test]
    fn delta_subtraction_wraps_rather_than_panics() {
        let mut clock = Some(WorldClock::from_state(i64::MIN, 5000, 0.0, 1.0));
        // i64::MAX.wrapping_sub(i64::MIN) == -1 → newPartialTicks = -1.0.
        apply_set_time(&mut clock, OVERWORLD, i64::MAX, &[]);
        let c = clock.unwrap();
        assert_eq!(
            c.total, 4999,
            "wrapped delta of -1 borrows one tick, no panic"
        );
        assert_eq!(c.last_game_time, i64::MAX);
    }

    /// `totalTicks += fullTicks` is `long` addition that wraps two's-complement —
    /// `i64::MAX + 1` wraps to `i64::MIN`, not a debug overflow panic.
    #[test]
    fn total_addition_wraps_rather_than_panics() {
        let mut clock = Some(WorldClock::from_state(0, i64::MAX, 0.0, 1.0));
        // delta 1 at rate 1 → fullTicks 1 → i64::MAX.wrapping_add(1).
        apply_set_time(&mut clock, OVERWORLD, 1, &[]);
        let c = clock.unwrap();
        assert_eq!(
            c.total,
            i64::MIN,
            "wrapping_add overflow wraps to i64::MIN, no panic"
        );
        assert_eq!(c.last_game_time, 1);
    }

    // -- `ClientLevel.tickTime`: the local per-tick advance -----------------

    /// Before the first `set_time` the client game-time is `None`, so
    /// `ClientLevel.tickTime` has nothing to run — the local tick is a no-op and
    /// the renderer keeps reading full daylight.
    #[test]
    fn local_tick_before_first_set_time_is_a_no_op() {
        let mut clock = None;
        assert_eq!(local_tick_time(None, &mut clock), None);
        assert!(clock.is_none());
    }

    /// After an explicit join establishes the clock, each running client tick
    /// advances it by exactly one — N local ticks add N. This is the path the
    /// sync-only clock lacked: it moved in 20-tick jumps and fell short of the
    /// elapsed ticks (the measured +75 over 80 ticks).
    #[test]
    fn join_then_n_local_ticks_advance_n() {
        let mut clock = None;
        // Join carries an explicit overworld state; `set_time` also anchors the
        // client game-time to the packet value (the `setGameTime` step).
        apply_set_time(&mut clock, OVERWORLD, 1000, &[(0, 5000, 0.0, 1.0)]);
        let mut game_time = Some(1000);
        for i in 1..=64 {
            let (gt, day) = local_tick_time(game_time, &mut clock).unwrap();
            game_time = Some(gt);
            assert_eq!(gt, 1000 + i, "game_time advances one per tick");
            assert_eq!(day, 5000 + i, "day_ticks advances one per tick at rate 1");
        }
        assert_eq!(clock.unwrap().total, 5064);
        assert_eq!(game_time, Some(1064));
    }

    /// The no-double-count invariant: the 20-tick `forceGameTimeSynchronization`
    /// sync at the game time the client already predicted contributes a
    /// zero-delta advance on its own, leaving exactly the one local `+1` for
    /// that tick. (In `PlaySession::tick` the sync is drained before the local
    /// advance, so both run against the same clock in one tick.)
    #[test]
    fn empty_sync_at_predicted_time_adds_zero_then_one_local_tick() {
        // The client has locally ticked its clock up to game 1020 (clock 5020).
        let mut clock = Some(WorldClock::from_state(1020, 5020, 0.0, 1.0));
        let game_time = Some(1020);

        // A drained empty sync carries the *already-predicted* game time 1020 →
        // `setGameTime` is a no-op and `handleUpdates` advances by delta 0.
        apply_set_time(&mut clock, OVERWORLD, 1020, &[]);
        assert_eq!(
            clock.unwrap().total,
            5020,
            "empty sync at the predicted time adds zero"
        );

        // Then this tick's single local `+1`.
        let (gt, day) = local_tick_time(game_time, &mut clock).unwrap();
        assert_eq!(gt, 1021);
        assert_eq!(day, 5021, "exactly one tick banked, no double count");
    }

    /// A server sync whose game time DIFFERS from the client's prediction
    /// re-anchors the clock to the server value (a forward jump banks the gap, a
    /// backward jump borrows it), and the same tick's local `+1` then continues
    /// from the corrected value.
    #[test]
    fn server_correction_reanchors_then_local_tick() {
        // Forward correction: the client predicted 1000, the server is at 1005.
        let mut clock = Some(WorldClock::from_state(1000, 5000, 0.0, 1.0));
        apply_set_time(&mut clock, OVERWORLD, 1005, &[]); // advance delta +5
        assert_eq!(
            clock.unwrap().total,
            5005,
            "forward re-anchor banks the 5-tick gap"
        );
        let (gt, day) = local_tick_time(Some(1005), &mut clock).unwrap();
        assert_eq!(
            (gt, day),
            (1006, 5006),
            "local +1 continues from the correction"
        );

        // Backward correction: the client predicted 2000, the server is at 1997.
        let mut clock = Some(WorldClock::from_state(2000, 9000, 0.0, 1.0));
        apply_set_time(&mut clock, OVERWORLD, 1997, &[]); // advance delta -3
        assert_eq!(
            clock.unwrap().total,
            8997,
            "backward re-anchor borrows the 3-tick gap"
        );
        let (gt, day) = local_tick_time(Some(1997), &mut clock).unwrap();
        assert_eq!(
            (gt, day),
            (1998, 8998),
            "local +1 continues from the correction"
        );
    }

    /// A paused world (rate 0) advances the client game-time counter every tick
    /// but leaves the clock `total` frozen — the day/night cycle holds while the
    /// world keeps counting ticks (and the clock still re-anchors to `gameTime`).
    #[test]
    fn rate_zero_holds_total_while_game_time_advances() {
        let mut clock = Some(WorldClock::from_state(1000, 500, 0.0, 0.0));
        let mut game_time = Some(1000);
        for i in 1..=10 {
            let (gt, day) = local_tick_time(game_time, &mut clock).unwrap();
            game_time = Some(gt);
            assert_eq!(gt, 1000 + i, "game_time still counts up");
            assert_eq!(day, 500, "paused clock frozen");
        }
        let c = clock.unwrap();
        assert_eq!(c.total, 500);
        assert_eq!(c.last_game_time, 1010, "clock still re-anchors to gameTime");
    }

    /// The local `+1` is Java `long` addition (`getGameTime() + 1L`) and wraps
    /// two's-complement: at `i64::MAX` the next tick's game-time is `i64::MIN`,
    /// no debug-overflow panic. The clock's own wrapping delta then reads +1
    /// across the boundary (`i64::MIN.wrapping_sub(i64::MAX) == 1`).
    #[test]
    fn local_game_time_wraps_at_i64_max() {
        let mut clock = Some(WorldClock::from_state(i64::MAX, 5000, 0.0, 1.0));
        let (gt, day) = local_tick_time(Some(i64::MAX), &mut clock).unwrap();
        assert_eq!(gt, i64::MIN, "game_time wraps to i64::MIN, no panic");
        assert_eq!(
            day, 5001,
            "clock's wrapping delta reads one tick across the wrap"
        );
        assert_eq!(clock.unwrap().last_game_time, i64::MIN);
    }

    /// Best-effort fallback: a server that sends `set_time` but never an
    /// overworld clock state leaves `overworld_clock` `None`; the local tick
    /// still advances the day-tick by falling back to the raw game time (one per
    /// tick), rather than only jumping on packets.
    #[test]
    fn no_explicit_clock_falls_back_to_game_time_each_tick() {
        let mut clock: Option<WorldClock> = None;
        // `set_time` with an entry for the_end (id 1) only — overworld unmatched.
        apply_set_time(&mut clock, OVERWORLD, 200, &[(1, 999, 0.0, 1.0)]);
        assert!(clock.is_none(), "no overworld clock established");
        let mut game_time = Some(200);
        for i in 1..=5 {
            let (gt, day) = local_tick_time(game_time, &mut clock).unwrap();
            game_time = Some(gt);
            assert_eq!(day, 200 + i, "fallback day-tick advances one per tick");
        }
    }
}
