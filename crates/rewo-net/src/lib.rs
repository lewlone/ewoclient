//! rewo-net — the vanilla-protocol connection state machine.
//!
//! M1 scope: connect to an offline-mode server, walk
//! Handshake → Login → Configuration → Play, honor the liveness contract
//! (REWO_PLAN.md §6.2), decode chunk/light/entity packets into `rewo-world`,
//! and (optionally) record every inbound packet for replay. Encryption +
//! online-mode join land in M7; this path is plaintext/offline only.
//!
//! Blocking single-threaded I/O — the render loop never lives here. In the
//! real client this runs on the net thread (REWO_PLAN.md §4); for the M1
//! soak/replay tools it runs on its own driver.

pub mod biome_parse;
pub mod chat_sign;
pub mod crypt;
pub mod dimension_parse;
pub mod effects;
pub mod ids;
pub mod metadata;
pub mod play;
pub mod record;
pub mod skins;
pub mod spawn_info;

use std::borrow::Cow;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use rewo_data::packets::State;
use rewo_data::{blocks::Blocks, GameData};
use rewo_proto::frame::FrameCodec;
use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::writer::PacketWriter;
use rewo_world::dimension::{DimensionShape, DimensionTypeDef};
use rewo_world::World;

use ids::Ids;

pub const PROTOCOL_VERSION: i32 = 776;

/// Map a decode error into the crate's `String` error channel. Kept terse
/// because it appears at every reader `?` inside a `Result<_, String>` fn.
fn de(e: rewo_proto::ProtoError) -> String {
    format!("decode: {e}")
}

/// Parse the play-login prefix (`ClientboundLoginPacket` up to and including the
/// `CommonPlayerSpawnInfo` dimension-type holder) and return the holder id.
///
/// The holder is `DimensionType.STREAM_CODEC = ByteBufCodecs.holderRegistry` =
/// an `idMapper`, so it is the **raw 0-based registry id** — there is NO
/// `0=inline`/`id+1` convention (that belongs to the different
/// `ByteBufCodecs.holder` codec). Shared by the live `Connection`, the replay
/// path, and the unit tests.
///
/// The holder is the *first* field of the embedded `CommonPlayerSpawnInfo`, so
/// this is `spawn_info::read_login_prefix` plus one VarInt — callers that need
/// the rest of the block read it with `CommonPlayerSpawnInfo::read` instead.
pub(crate) fn parse_login_dimension_holder(packet: &[u8]) -> rewo_proto::Result<i32> {
    let mut r = PacketReader::new(packet);
    spawn_info::read_login_prefix(&mut r)?;
    r.varint() // dimension-type holder (raw 0-based registry id)
}

/// Select the synced dimension-type definition a login / respawn packet names,
/// by **raw 0-based registry id** — the vector index *is* the holder id, so
/// this is a direct index and never a name lookup.
///
/// The borrowed arm is the only one that can be a real registry entry. The
/// owned arm is [`DimensionTypeDef::unresolved_holder`], reached solely when the
/// *packet* names a holder the synced registry does not contain (negative, or
/// past the end) — a registry entry that fails to parse never gets this far,
/// because `dimension_parse` fails the connection instead.
pub(crate) fn login_dimension_type(
    holder: i32,
    dim_types: &[DimensionTypeDef],
) -> Cow<'_, DimensionTypeDef> {
    if holder >= 0 {
        if let Some(def) = dim_types.get(holder as usize) {
            return Cow::Borrowed(def);
        }
    }
    log::warn!(
        "net: dimension-type holder {holder} is not in the synced registry \
         ({} entries) — falling back to an unresolved Overworld-shaped world",
        dim_types.len()
    );
    Cow::Owned(DimensionTypeDef::unresolved_holder(holder))
}

/// Outcome of a soak/replay session.
pub struct SessionStats {
    pub packets_in: u64,
    pub bytes_in: u64,
    pub chunks: u64,
    pub keepalives: u64,
    pub teleports: u64,
    pub reached_play: bool,
    pub disconnect_reason: Option<String>,
    pub world_digest: u64,
    pub loaded_columns: usize,
}

/// The connection's byte stream: plain TCP until the login `key` packet,
/// AES-128-CFB8 in both directions after. One type serves the synchronous
/// connection AND the split halves (a half simply has one cipher `None`).
pub(crate) struct NetStream {
    inner: TcpStream,
    enc: Option<crypt::Cfb8>,
    dec: Option<crypt::Cfb8>,
    /// Encrypt scratch (Write::write must not mutate the caller's buffer).
    wbuf: Vec<u8>,
}

impl NetStream {
    fn new(inner: TcpStream) -> Self {
        Self {
            inner,
            enc: None,
            dec: None,
            wbuf: Vec::new(),
        }
    }

    /// Turn on AES-128-CFB8 both ways (call right after sending `key` —
    /// every subsequent byte on the wire is ciphered).
    fn enable_encryption(&mut self, secret: &[u8; 16]) {
        self.enc = Some(crypt::Cfb8::new(secret));
        self.dec = Some(crypt::Cfb8::new(secret));
    }

    /// Split into (read half, write half) for the play-phase reader thread.
    /// Each half carries its direction's cipher state.
    fn split(self) -> std::io::Result<(NetStream, NetStream)> {
        let read_inner = self.inner.try_clone()?;
        let read_half = Self {
            inner: read_inner,
            enc: None,
            dec: self.dec,
            wbuf: Vec::new(),
        };
        let write_half = Self {
            inner: self.inner,
            enc: self.enc,
            dec: None,
            wbuf: self.wbuf,
        };
        Ok((read_half, write_half))
    }
}

impl Read for NetStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(d) = self.dec.as_mut() {
            d.decrypt(&mut buf[..n]);
        }
        Ok(n)
    }
}

impl Write for NetStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.enc.as_mut() {
            None => self.inner.write(buf),
            Some(e) => {
                self.wbuf.clear();
                self.wbuf.extend_from_slice(buf);
                e.encrypt(&mut self.wbuf);
                self.inner.write_all(&self.wbuf)?;
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub struct Connection<'a> {
    stream: NetStream,
    codec: FrameCodec,
    state: State,
    ids: Ids,
    data: &'a GameData,
    scratch: Vec<u8>,
    packet: Vec<u8>,
    pub recorder: Option<record::Recorder>,
    /// The `minecraft:dimension_type` registry in raw wire order — index *is*
    /// the holder registry id. One vector of unified definitions, not the
    /// M14-era parallel `dim_shapes` / `dim_attrs` pair that a holder id could
    /// index inconsistently.
    dim_types: Vec<DimensionTypeDef>,
    /// Registry id of the `minecraft:overworld` world clock (see
    /// `parse_registry_data`); `None` on a server that syncs no clocks.
    overworld_clock_id: Option<i32>,
    /// Raw `minecraft:mob_effect` registry ids for `night_vision` / `darkness`,
    /// captured from `registry_data` (never assumed from bootstrap order) so the
    /// M13 lightmap can match the effect packets. `None` until config syncs the
    /// registry.
    night_vision_id: Option<i32>,
    darkness_id: Option<i32>,
    /// `minecraft:worldgen/biome` registry in raw wire order (M14 biome tint).
    biome_defs: Vec<rewo_world::biome::BiomeDef>,
}

impl<'a> Connection<'a> {
    pub fn connect(host: &str, port: u16, data: &'a GameData) -> Result<Self, String> {
        let stream =
            TcpStream::connect((host, port)).map_err(|e| format!("connect {host}:{port}: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| format!("set timeout: {e}"))?;
        stream.set_nodelay(true).ok();
        let ids = Ids::resolve(&data.packets)?;
        Ok(Self {
            stream: NetStream::new(stream),
            codec: FrameCodec::default(),
            state: State::Handshake,
            ids,
            data,
            scratch: Vec::new(),
            packet: Vec::new(),
            recorder: None,
            dim_types: Vec::new(),
            overworld_clock_id: None,
            night_vision_id: None,
            darkness_id: None,
            biome_defs: Vec::new(),
        })
    }

    fn send(&mut self, packet: PacketWriter) -> Result<(), String> {
        self.codec
            .write_frame(&mut self.stream, &packet.buf)
            .map_err(|e| format!("send: {e}"))
    }

    /// Read one inbound packet into `self.packet`, returning (id, body_range).
    /// The body (post-id) is what handlers parse.
    fn recv(&mut self) -> Result<Option<(i32, usize)>, String> {
        let mut packet = std::mem::take(&mut self.packet);
        let res = self
            .codec
            .read_frame(&mut self.stream, &mut self.scratch, &mut packet);
        self.packet = packet;
        match res {
            Ok(()) => {}
            Err(rewo_proto::ProtoError::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(format!("recv: {e}")),
        }
        let mut pos = 0;
        let id = rewo_proto::varint::read_varint(&self.packet, &mut pos).map_err(de)?;
        Ok(Some((id, pos)))
    }

    // -- handshake + login -------------------------------------------------

    pub fn login_offline(&mut self, host: &str, port: u16, username: &str) -> Result<(), String> {
        self.login(host, port, username, None)
    }

    /// Handshake + Login. With `auth`, an online-mode server's encryption
    /// request is answered (session join + AES handshake); without it,
    /// online-mode servers fail loud.
    pub fn login(
        &mut self,
        host: &str,
        port: u16,
        username: &str,
        auth: Option<&crypt::OnlineAuth>,
    ) -> Result<(), String> {
        // Handshake: protocol, host, port, intent=2 (login).
        let mut hs = PacketWriter::packet(self.ids.sb_handshake_intention);
        hs.varint(PROTOCOL_VERSION).string(host).u16(port).varint(2);
        self.send(hs)?;
        self.state = State::Login;

        // Login hello: name + profile UUID (offline: zero — server assigns).
        let mut hello = PacketWriter::packet(self.ids.sb_login_hello);
        hello.string(username).uuid(auth.map_or(0, |a| a.uuid));
        self.send(hello)?;

        // Drain login until we acknowledge → Configuration.
        loop {
            let Some((id, body)) = self.recv()? else {
                return Err("connection closed during login".into());
            };
            match id {
                x if x == self.ids.cb_login_compression => {
                    let mut r = PacketReader::new(&self.packet[body..]);
                    let threshold = r.varint().map_err(de)?;
                    self.codec.compression_threshold = Some(threshold);
                    log::info!("net: compression threshold {threshold}");
                }
                x if x == self.ids.cb_login_finished => {
                    // GameProfile follows; we don't need it for M1.
                    let mut ack = PacketWriter::packet(self.ids.sb_login_acknowledged);
                    let _ = &mut ack;
                    self.send(ack)?;
                    self.state = State::Configuration;
                    log::info!("net: login finished → configuration");
                    return Ok(());
                }
                x if x == self.ids.cb_login_disconnect => {
                    let mut r = PacketReader::new(&self.packet[body..]);
                    let reason = r.string(262144).unwrap_or_else(|_| "<unparsed>".into());
                    return Err(format!("login disconnect: {reason}"));
                }
                x if x == self.ids.cb_login_hello => {
                    self.handle_encryption_request(body, auth)?;
                }
                other => {
                    log::debug!("net: ignoring login packet id {other}");
                }
            }
        }
    }

    /// Clientbound login `hello` = the encryption request. Wire (decompiled
    /// `ClientboundHelloPacket`): server-id string, pubkey byte array
    /// (X.509 DER), verify-token byte array, `should_authenticate` bool.
    fn handle_encryption_request(
        &mut self,
        body: usize,
        auth: Option<&crypt::OnlineAuth>,
    ) -> Result<(), String> {
        let (server_id, pubkey, token, should_auth) = {
            let mut r = PacketReader::new(&self.packet[body..]);
            let sid = r.string(20).map_err(de)?;
            let key = r.byte_array(4096).map_err(de)?.to_vec();
            let tok = r.byte_array(4096).map_err(de)?.to_vec();
            let sa = r.bool().map_err(de)?;
            (sid, key, tok, sa)
        };
        let mut secret = [0u8; 16];
        rand::Rng::fill(&mut rand::thread_rng(), &mut secret[..]);
        if should_auth {
            let auth = auth.ok_or(
                "server is online-mode but no account was provided \
                 (REWO_ACCESS_TOKEN / REWO_UUID / REWO_USERNAME)",
            )?;
            let hash = crypt::server_hash(&server_id, &secret, &pubkey);
            crypt::session_join(auth, &hash)?;
            log::info!("net: mojang session join ok (server hash {hash})");
        }
        let enc_secret = crypt::rsa_encrypt(&pubkey, &secret)?;
        let enc_token = crypt::rsa_encrypt(&pubkey, &token)?;
        let mut key = PacketWriter::packet(self.ids.sb_login_key);
        key.varint(enc_secret.len() as i32).raw(&enc_secret);
        key.varint(enc_token.len() as i32).raw(&enc_token);
        self.send(key)?;
        // Every byte after the key packet is ciphered, both directions.
        self.stream.enable_encryption(&secret);
        log::info!("net: encryption enabled (AES-128-CFB8)");
        Ok(())
    }

    // -- configuration -----------------------------------------------------

    /// Send the client's config-phase openers: brand + client information.
    fn send_config_openers(&mut self) -> Result<(), String> {
        // Brand plugin message.
        let mut brand_payload = PacketWriter::default();
        brand_payload.string("rewo");
        let mut brand = PacketWriter::packet(self.ids.sb_config_custom_payload);
        brand.string("minecraft:brand").raw(&brand_payload.buf);
        self.send(brand)?;

        // Client information (view distance etc.). Matches decompiled
        // ServerboundClientInformation field order.
        let mut info = PacketWriter::packet(self.ids.sb_config_client_information);
        info.string("en_us") // language
            .u8(10) // view distance
            .varint(0) // chat visibility: FULL
            .bool(true) // chat colors
            .u8(0x7f) // skin model parts
            .varint(1) // main hand: right
            .bool(false) // text filtering
            .bool(true); // allow server listings
                         // particle status enum (0 = all) — 26.x added field.
        info.varint(0);
        self.send(info)
    }

    /// Run configuration until FinishConfiguration → Play.
    fn run_configuration(&mut self, stats: &mut SessionStats) -> Result<(), String> {
        self.send_config_openers()?;
        loop {
            let Some((id, body)) = self.recv()? else {
                return Err("connection closed during configuration".into());
            };
            self.record_inbound(id, body);
            match id {
                x if x == self.ids.cb_config_keep_alive => {
                    let ka = i64::from_be_bytes(self.packet[body..body + 8].try_into().unwrap());
                    let mut resp = PacketWriter::packet(self.ids.sb_config_keep_alive);
                    resp.i64(ka);
                    self.send(resp)?;
                    stats.keepalives += 1;
                }
                x if x == self.ids.cb_config_ping => {
                    let ping = i32::from_be_bytes(self.packet[body..body + 4].try_into().unwrap());
                    let mut resp = PacketWriter::packet(self.ids.sb_config_pong);
                    resp.i32(ping);
                    self.send(resp)?;
                }
                x if x == self.ids.cb_config_select_known_packs => {
                    // Reply with an empty list = "I have none cached, send me
                    // everything" (the full RegistryData follows).
                    let mut resp = PacketWriter::packet(self.ids.sb_config_select_known_packs);
                    resp.varint(0);
                    self.send(resp)?;
                }
                x if x == self.ids.cb_config_registry_data => {
                    self.parse_registry_data(body)?;
                }
                x if x == self.ids.cb_config_finish => {
                    let ack = PacketWriter::packet(self.ids.sb_config_finish);
                    self.send(ack)?;
                    self.state = State::Play;
                    log::info!("net: configuration finished → play");
                    return Ok(());
                }
                x if Some(x) == self.ids.cb_config_cookie_request => {
                    self.answer_cookie_request(body, self.ids.sb_config_cookie_response)?;
                }
                x if x == self.ids.cb_config_disconnect => {
                    let mut r = PacketReader::new(&self.packet[body..]);
                    let reason = r.nbt().map(|n| n.to_plain_text()).unwrap_or_default();
                    stats.disconnect_reason = Some(reason.clone());
                    return Err(format!("config disconnect: {reason}"));
                }
                _ => {} // update_tags, enabled_features, code_of_conduct, etc.
            }
        }
    }

    /// Decode one Configuration `registry_data` packet.
    ///
    /// The `minecraft:dimension_type` registry is the one that can fail the
    /// connection: it is the only registry here whose entries the client
    /// *must* understand exactly (a wrong vertical shape mis-decodes every
    /// chunk, a wrong `has_skylight` invents light), so `dimension_parse`
    /// returns a `Result` and it propagates. The remaining registries are
    /// id-capture only and stay tolerant.
    fn parse_registry_data(&mut self, body: usize) -> Result<(), String> {
        let mut r = PacketReader::new(&self.packet[body..]);
        let Ok(registry) = r.identifier() else {
            return Ok(());
        };
        let Ok(count) = r.count("registry entries", 1) else {
            return Ok(());
        };
        if registry == dimension_parse::DIMENSION_TYPE_REGISTRY {
            self.dim_types = dimension_parse::parse_dimension_registry(&mut r, count)?;
            log::info!("net: {} dimension type(s) synced", self.dim_types.len());
            return Ok(());
        }
        // The day/night timeline runs on the `minecraft:overworld` world
        // clock, and `set_time` keys its clock map by raw registry id. The id
        // is capture-able here rather than assumed from bootstrap order.
        let is_clock = registry == "minecraft:world_clock";
        // The M13 camera lightmap keys night-vision / darkness off their raw
        // `mob_effect` registry ids, captured here rather than assumed from
        // bootstrap order (exactly like the world clock above).
        let is_mob_effect = registry == "minecraft:mob_effect";
        // M14: the biome registry, in raw wire order, drives per-biome tint.
        let is_biome = registry == "minecraft:worldgen/biome";
        if is_biome {
            self.biome_defs.clear();
        }
        for idx in 0..count {
            let Ok(entry_name) = r.identifier() else {
                return Ok(());
            };
            if is_clock && entry_name == "minecraft:overworld" {
                self.overworld_clock_id = Some(idx as i32);
            }
            if is_mob_effect {
                match entry_name.as_str() {
                    "minecraft:night_vision" => self.night_vision_id = Some(idx as i32),
                    "minecraft:darkness" => self.darkness_id = Some(idx as i32),
                    _ => {}
                }
            }
            let has_nbt = r.bool().unwrap_or(false);
            if !has_nbt {
                if is_biome {
                    // A biome with no NBT is degenerate; keep raw order intact
                    // with a neutral default so indices still line up.
                    self.biome_defs
                        .push(crate::biome_parse::parse_biome(&entry_name, &Nbt::End));
                }
                continue;
            }
            let Ok(nbt) = r.nbt() else {
                return Ok(());
            };
            if is_biome {
                self.biome_defs
                    .push(crate::biome_parse::parse_biome(&entry_name, &nbt));
            }
        }
        if is_biome {
            log::info!("net: {} biome(s) synced", self.biome_defs.len());
        }
        Ok(())
    }

    fn answer_cookie_request(&mut self, body: usize, resp_id: i32) -> Result<(), String> {
        let mut r = PacketReader::new(&self.packet[body..]);
        let key = r.identifier().unwrap_or_default();
        let mut resp = PacketWriter::packet(resp_id);
        resp.string(&key).bool(false); // no payload
        self.send(resp)
    }

    // -- play --------------------------------------------------------------

    /// Run the Play phase until `deadline`, applying packets to `world`.
    fn run_play(
        &mut self,
        world: &mut World,
        stats: &mut SessionStats,
        deadline: Instant,
    ) -> Result<(), String> {
        stats.reached_play = true;
        while Instant::now() < deadline {
            let Some((id, body)) = self.recv()? else {
                log::info!("net: server closed the play connection");
                return Ok(());
            };
            self.record_inbound(id, body);
            stats.packets_in += 1;
            stats.bytes_in += (self.packet.len() - body) as u64;

            match id {
                x if x == self.ids.cb_play_keep_alive => {
                    let ka = i64::from_be_bytes(self.packet[body..body + 8].try_into().unwrap());
                    let mut resp = PacketWriter::packet(self.ids.sb_play_keep_alive);
                    resp.i64(ka);
                    self.send(resp)?;
                    stats.keepalives += 1;
                }
                x if x == self.ids.cb_play_ping => {
                    let ping = i32::from_be_bytes(self.packet[body..body + 4].try_into().unwrap());
                    let mut resp = PacketWriter::packet(self.ids.sb_play_pong);
                    resp.i32(ping);
                    self.send(resp)?;
                }
                x if x == self.ids.cb_play_login => {
                    self.handle_play_login(world, body)?;
                }
                x if x == self.ids.cb_play_position => {
                    self.handle_teleport(body, stats)?;
                }
                x if x == self.ids.cb_play_chunk_batch_finished => {
                    // Ack with a desired rate so chunks keep streaming.
                    let mut resp = PacketWriter::packet(self.ids.sb_play_chunk_batch_received);
                    resp.f32(16.0);
                    self.send(resp)?;
                }
                x if x == self.ids.cb_play_level_chunk => {
                    self.handle_chunk(world, body, stats);
                }
                x if x == self.ids.cb_play_forget_chunk => {
                    let mut r = PacketReader::new(&self.packet[body..]);
                    if let Ok(v) = r.i64() {
                        let cx = v as i32;
                        let cz = (v >> 32) as i32;
                        world.forget_column(cx, cz);
                    }
                }
                x if x == self.ids.cb_play_block_update => {
                    self.handle_block_update(world, body);
                }
                x if x == self.ids.cb_play_add_entity => {
                    self.handle_add_entity(world, body);
                }
                x if x == self.ids.cb_play_remove_entities => {
                    let mut r = PacketReader::new(&self.packet[body..]);
                    if let Ok(n) = r.count("remove entities", 1) {
                        for _ in 0..n {
                            if let Ok(eid) = r.varint() {
                                world.entities.remove(eid);
                            }
                        }
                    }
                }
                x if Some(x) == self.ids.cb_play_start_configuration => {
                    // Server pulls us back to config (datapack reload etc).
                    let ack = PacketWriter::packet(self.ids.sb_play_config_acknowledged);
                    self.send(ack)?;
                    self.state = State::Configuration;
                    log::info!("net: server started configuration → re-entering config");
                    self.run_configuration(stats)?;
                    stats.reached_play = true;
                }
                x if Some(x) == self.ids.cb_play_cookie_request => {
                    if let Some(resp_id) = self.ids.sb_play_cookie_response {
                        self.answer_cookie_request(body, resp_id)?;
                    }
                }
                x if x == self.ids.cb_play_disconnect => {
                    let mut r = PacketReader::new(&self.packet[body..]);
                    let reason = r.nbt().map(|n| n.to_plain_text()).unwrap_or_default();
                    stats.disconnect_reason = Some(reason.clone());
                    log::warn!("net: play disconnect: {reason}");
                    return Ok(());
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_play_login(&mut self, world: &mut World, body: usize) -> Result<(), String> {
        let holder = parse_login_dimension_holder(&self.packet[body..]).map_err(de)?;
        let def = login_dimension_type(holder, &self.dim_types);
        // The world was created before login, so re-point it at the dimension
        // we actually joined. It holds no columns yet, which is what makes an
        // in-place `apply_dimension_type` sound here.
        world.apply_dimension_type(&def);
        log::info!(
            "net: play login — dimension {} (holder {holder}): min_y={} height={} \
             sky_light={} cardinal={}",
            def.name,
            def.shape.min_y,
            def.shape.height,
            def.has_sky_light,
            def.cardinal_light_type.name(),
        );
        // Signal we've loaded (keeps some servers from stalling).
        let loaded = PacketWriter::packet(self.ids.sb_play_player_loaded);
        self.send(loaded)?;
        Ok(())
    }

    fn handle_teleport(&mut self, body: usize, stats: &mut SessionStats) -> Result<(), String> {
        let teleport_id = {
            let mut r = PacketReader::new(&self.packet[body..]);
            r.varint().map_err(de)?
        };
        let mut ack = PacketWriter::packet(self.ids.sb_play_accept_teleport);
        ack.varint(teleport_id);
        self.send(ack)?;
        stats.teleports += 1;
        Ok(())
    }

    fn handle_chunk(&mut self, world: &mut World, body: usize, stats: &mut SessionStats) {
        let blocks: &Blocks = &self.data.blocks;
        let mut r = PacketReader::new(&self.packet[body..]);
        match rewo_world::chunk::read_level_chunk(&mut r, &world.shape, blocks) {
            Ok(column) => {
                world.insert_column(column.cx, column.cz, column);
                stats.chunks += 1;
            }
            Err(e) => {
                // A decode failure is a real bug in the wire model — surface it.
                log::error!("net: chunk decode failed: {e}");
            }
        }
    }

    fn handle_block_update(&mut self, world: &mut World, body: usize) {
        let mut r = PacketReader::new(&self.packet[body..]);
        if let (Ok((x, y, z)), Ok(state)) = (r.position(), r.varint()) {
            world.set_block(x, y, z, state as u32);
        }
    }

    fn handle_add_entity(&mut self, world: &mut World, body: usize) {
        let mut r = PacketReader::new(&self.packet[body..]);
        let _ = read_add_entity(&mut r, world);
    }

    // -- driver ------------------------------------------------------------

    /// Full session: login → config → play until `run_for`, applying to a
    /// fresh world. Returns stats + the world (for digest / queries).
    pub fn run_session(
        mut self,
        host: &str,
        port: u16,
        username: &str,
        run_for: Duration,
    ) -> Result<(SessionStats, World), String> {
        let mut stats = SessionStats {
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
        self.login_offline(host, port, username)?;
        self.run_configuration(&mut stats)?;
        let mut world = World::new(DimensionShape::OVERWORLD);
        let deadline = Instant::now() + run_for;
        self.run_play(&mut world, &mut stats, deadline)?;
        stats.world_digest = world.digest();
        stats.loaded_columns = world.loaded_columns();
        if let Some(rec) = self.recorder.take() {
            let n = rec.finish().map_err(|e| format!("finish recording: {e}"))?;
            log::info!("net: recorded {n} inbound packets");
        }
        let _ = self.stream.flush();
        Ok((stats, world))
    }

    fn record_inbound(&mut self, id: i32, body: usize) {
        if let Some(rec) = self.recorder.as_mut() {
            let _ = rec.record(self.state, id, &self.packet[body..]);
        }
    }
}

/// Decode an Add Entity packet body into the entity table. Shared by the M1
/// snapshot path and the M3 live session.
pub(crate) fn read_add_entity(r: &mut PacketReader, world: &mut World) -> rewo_proto::Result<()> {
    let id = r.varint()?;
    let uuid = r.uuid()?;
    let type_id = r.varint()?;
    let x = r.f64()?;
    let y = r.f64()?;
    let z = r.f64()?;
    skip_lpvec3(r)?; // movement (LpVec3, variable length)
                     // Wire order (decompiled read): xRot (pitch) first, THEN yRot (yaw),
                     // then yHeadRot — packed-degree bytes.
    let pitch = r.i8()? as f32 * (360.0 / 256.0);
    let yaw = r.i8()? as f32 * (360.0 / 256.0);
    let head_yaw = r.i8()? as f32 * (360.0 / 256.0);
    let mut state = rewo_world::entities::EntityState::new(uuid, type_id, x, y, z, yaw, pitch);
    state.set_head_yaw(head_yaw);
    world.entities.add(id, state);
    Ok(())
}

/// Skip an LpVec3 (entity movement). Layout (decompiled `LpVec3`):
/// byte0; if 0 → zero vector (done); else read byte1 + u32, and if
/// (byte0 & 4) continuation flag set, read a trailing VarInt.
pub(crate) fn skip_lpvec3(r: &mut PacketReader) -> rewo_proto::Result<()> {
    let lowest = r.u8()?;
    if lowest == 0 {
        return Ok(());
    }
    r.u8()?; // middle
    r.take(4)?; // highest u32
    if lowest & 4 == 4 {
        r.varint()?; // scale continuation
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a realistic `ClientboundLoginPacket` prefix up to (and including)
    /// the dimension-type holder, for the given holder id + dimension names.
    fn login_prefix(holder: i32, dims: &[&str]) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.i32(42); // player entity id
        w.bool(false); // hardcore
        w.varint(dims.len() as i32);
        for d in dims {
            w.string(d);
        }
        w.varint(20); // max players
        w.varint(12); // view dist
        w.varint(12); // sim dist
        w.bool(false); // reduced debug
        w.bool(true); // show death screen
        w.bool(false); // do limited crafting
        w.varint(holder); // raw 0-based dimension-type holder
        w.buf
    }

    #[test]
    fn login_holder_is_raw_zero_based() {
        let dims = [
            "minecraft:overworld",
            "minecraft:the_nether",
            "minecraft:the_end",
        ];
        // Holder 0 = the FIRST dimension type (overworld), NOT "inline".
        assert_eq!(
            parse_login_dimension_holder(&login_prefix(0, &dims)).unwrap(),
            0
        );
        assert_eq!(
            parse_login_dimension_holder(&login_prefix(1, &dims)).unwrap(),
            1
        );
        assert_eq!(
            parse_login_dimension_holder(&login_prefix(2, &dims)).unwrap(),
            2
        );
    }

    /// The registry the selection tests index, in a **deliberately non
    /// name-sorted** wire order: the Nether is holder 0 and the Overworld is
    /// holder 3. Any name-keyed shortcut fails immediately here.
    fn registry() -> Vec<DimensionTypeDef> {
        use crate::dimension_parse::builtin as fx;
        let body = fx::registry_packet(&[
            ("minecraft:the_nether", fx::the_nether()),
            ("minecraft:the_end", fx::the_end()),
            ("minecraft:overworld_caves", fx::overworld_caves()),
            ("minecraft:overworld", fx::overworld()),
        ]);
        crate::dimension_parse::parse_dimension_registry_packet(&body)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn login_selects_the_definition_by_raw_id_not_by_name() {
        let defs = registry();
        // Holder 0 is the FIRST synced entry — here the Nether, not a
        // special-cased default and not `dim_types[-1]`.
        let zero = login_dimension_type(0, &defs);
        assert!(matches!(zero, Cow::Borrowed(_)), "holder 0 must resolve");
        assert_eq!(zero.name, "minecraft:the_nether");
        assert_eq!(zero.shape, DimensionShape::NETHER);
        assert!(!zero.has_sky_light);
        // …and every other slot follows the wire order too.
        assert_eq!(login_dimension_type(1, &defs).name, "minecraft:the_end");
        assert_eq!(
            login_dimension_type(2, &defs).name,
            "minecraft:overworld_caves"
        );
        let three = login_dimension_type(3, &defs);
        assert_eq!(three.name, "minecraft:overworld");
        assert_eq!(three.shape, DimensionShape::OVERWORLD);
        assert!(three.has_sky_light);
    }

    /// The same registry sent in a different order must give different holders
    /// the same *definitions* — i.e. selection is order-based, not name-based.
    #[test]
    fn raw_id_selection_is_order_based_not_name_based() {
        use crate::dimension_parse::builtin as fx;
        let reversed =
            crate::dimension_parse::parse_dimension_registry_packet(&fx::registry_packet(&[
                ("minecraft:overworld", fx::overworld()),
                ("minecraft:overworld_caves", fx::overworld_caves()),
                ("minecraft:the_end", fx::the_end()),
                ("minecraft:the_nether", fx::the_nether()),
            ]))
            .unwrap()
            .unwrap();
        let defs = registry();
        // Holder 0 names the Nether in one order and the Overworld in the
        // other. A name-keyed implementation would return the same entry both
        // times; an order-keyed one cannot.
        assert_eq!(login_dimension_type(0, &defs).name, "minecraft:the_nether");
        assert_eq!(
            login_dimension_type(0, &reversed).name,
            "minecraft:overworld"
        );
        assert_ne!(
            login_dimension_type(0, &defs).shape,
            login_dimension_type(0, &reversed).shape
        );
        // The Nether is holder 0 in one and holder 3 in the other, and both
        // resolve to the identical definition.
        assert_eq!(
            login_dimension_type(0, &defs).into_owned(),
            login_dimension_type(3, &reversed).into_owned()
        );
    }

    #[test]
    fn out_of_range_holders_get_the_named_fallback_not_a_vanilla_claim() {
        let defs = registry();
        for holder in [4, 9, -1, i32::MIN] {
            let d = login_dimension_type(holder, &defs);
            assert!(
                matches!(d, Cow::Owned(_)),
                "holder {holder} must not resolve"
            );
            assert_eq!(d.name, format!("rewo:unresolved_dimension_type/{holder}"));
            // Degrades to the pre-M16 Overworld behaviour…
            assert_eq!(d.shape, DimensionShape::OVERWORLD);
            // …without ever claiming to *be* a synced entry.
            assert!(defs.iter().all(|e| e.name != d.name));
        }
    }

    #[test]
    fn login_holder_end_to_end_zero_resolves_the_first_entry() {
        // The full path: parse a realistic prefix (holder 0) then resolve. The
        // pre-fix "0 = inline → OVERWORLD default" bug would have produced the
        // Overworld shape; here holder 0 is the Nether, so the two are
        // distinguishable.
        let defs = registry();
        let holder =
            parse_login_dimension_holder(&login_prefix(0, &["minecraft:overworld"])).unwrap();
        assert_eq!(holder, 0);
        let d = login_dimension_type(holder, &defs);
        assert_eq!(d.shape, DimensionShape::NETHER);
        assert_ne!(d.shape, DimensionShape::OVERWORLD);
    }
}
