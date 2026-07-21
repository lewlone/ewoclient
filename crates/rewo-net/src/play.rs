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
use std::net::TcpStream;
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
    writer: TcpStream,
    codec: FrameCodec,
    rx: Receiver<Vec<u8>>,
    pub ids: Ids,
    pub world: World,
    pub player: PlayerState,
    /// state id → collides-as-full-cube. Callers build it (assets bake or a
    /// "non-air is solid" fallback for known-flat test worlds).
    pub solid: Vec<bool>,
    /// Chunk global-palette bit width (from the blocks table).
    global_bits: u32,
    dim_shapes: Vec<DimensionShape>,
    pub spawned: bool,
    pub corrections: u32,
    pub teleports: u32,
    pub block_updates: u32,
    /// Columns whose mesh is stale (new chunk / block edit). The live
    /// renderer drains this to know what to re-mesh; the bot ignores it.
    dirty: std::collections::HashSet<(i32, i32)>,
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
}

impl<'a> Connection<'a> {
    /// Login (offline) + configuration, then split into a live session.
    pub fn into_play(
        mut self,
        host: &str,
        port: u16,
        username: &str,
        solid: Vec<bool>,
        global_bits: u32,
    ) -> Result<PlaySession, String> {
        self.login_offline(host, port, username)?;
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

        let reader_stream = self
            .stream
            .try_clone()
            .map_err(|e| format!("split socket: {e}"))?;
        let writer = self.stream;
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
        let session = PlaySession {
            writer,
            codec,
            rx,
            ids: self.ids,
            world,
            player: PlayerState::at(0.5, 80.0, 0.5),
            solid,
            global_bits,
            dim_shapes,
            spawned: false,
            corrections: 0,
            teleports: 0,
            block_updates: 0,
            dirty: std::collections::HashSet::new(),
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
        };
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
    pub fn take_dirty(&mut self) -> Vec<(i32, i32)> {
        self.dirty.drain().collect()
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
        // Step other entities' 3-tick position lerps (vanilla cadence).
        self.world.entities.tick_lerp();
        if self.spawned {
            let solid = std::mem::take(&mut self.solid);
            let world = &self.world;
            let is_solid = |x: i32, y: i32, z: i32| -> bool {
                let state = world.block_state_at(x, y, z);
                solid.get(state as usize).copied().unwrap_or(state != 0)
            };
            physics::tick(&mut self.player, input, &is_solid);
            self.solid = solid;
            self.send_movement(input)?;
        }
        self.ticks += 1;
        Ok(())
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
            match rewo_world::chunk::read_level_chunk_bits(&mut r, &shape, self.global_bits) {
                Ok(col) => {
                    let (cx, cz) = (col.cx, col.cz);
                    self.world.insert_column(cx, cz, col);
                    // New column changes its own + its neighbors' edge faces.
                    self.mark_dirty_around(cx, cz);
                }
                Err(e) => log::error!("play: chunk decode failed: {e}"),
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
                self.world.set_block(x, y, z, state as u32);
                self.block_updates += 1;
                self.mark_dirty_around(x >> 4, z >> 4);
                log::debug!("net: block_update ({x},{y},{z}) = {state}");
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
                Ok((r.varint()?, packed_degrees(r.i8()?), packed_degrees(r.i8()?)))
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
        } else if id == ids.cb_play_set_entity_data {
            let mut r = PacketReader::new(body);
            if let Ok(eid) = r.varint() {
                let meta = crate::metadata::parse(&mut r);
                if meta.custom_name.is_some() {
                    self.world.entities.set_custom_name(eid, meta.custom_name);
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
        self.player.x = if rel(0) { self.player.x + vals[0] } else { vals[0] };
        self.player.y = if rel(1) { self.player.y + vals[1] } else { vals[1] };
        self.player.z = if rel(2) { self.player.z + vals[2] } else { vals[2] };
        self.player.yaw = if rel(3) { self.player.yaw + yaw } else { yaw };
        self.player.pitch = if rel(4) { self.player.pitch + pitch } else { pitch };
        self.player.vx = if rel(5) { self.player.vx + vals[3] } else { vals[3] };
        self.player.vy = if rel(6) { self.player.vy + vals[4] } else { vals[4] };
        self.player.vz = if rel(7) { self.player.vz + vals[5] } else { vals[5] };

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
                        let _name = r.string(64)?;
                        let _value = r.string(32767)?;
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
    }

    #[allow(dead_code)]
    fn apply_login_shape(&mut self, body: &[u8]) {
        let mut r = PacketReader::new(body);
        let parse = (|| -> rewo_proto::Result<i32> {
            r.i32()?;
            r.bool()?;
            let n = r.count("dims", 1)?;
            for _ in 0..n {
                r.identifier()?;
            }
            r.varint()?;
            r.varint()?;
            r.varint()?;
            r.bool()?;
            r.bool()?;
            r.bool()?;
            r.varint()
        })();
        if let Ok(holder) = parse {
            if holder > 0 {
                if let Some(shape) = self.dim_shapes.get((holder - 1) as usize) {
                    self.world.shape = *shape;
                }
            }
        }
    }

    // -- gameplay actions --------------------------------------------------

    pub fn send_chat(&mut self, message: &str) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_chat else {
            return Err("chat packet unavailable".into());
        };
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut p = PacketWriter::packet(id);
        p.string(message)
            .i64(millis) // Instant
            .i64(0) // salt
            .bool(false); // no signature (unsigned until M7)
        p.varint(0); // last-seen offset
        p.raw(&[0, 0, 0]); // FixedBitSet(20) acknowledged
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
        let solid = &self.solid;
        rewo_world::raycast::cast(eye, dir, reach, |x, y, z| {
            let s = world.block_state_at(x, y, z);
            solid.get(s as usize).copied().unwrap_or(s != 0)
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
