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
pub mod item_stack;
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
    /// Raw `minecraft:mob_effect` ids of the three effects that change a swing's
    /// duration (M19), captured the same way and for the same reason.
    swing_effect_ids: SwingEffectIds,
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
            swing_effect_ids: SwingEffectIds::default(),
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
                    // M19: `getCurrentSwingDuration`'s dig-speed / fatigue terms.
                    "minecraft:haste" => self.swing_effect_ids.haste = Some(idx as i32),
                    "minecraft:conduit_power" => {
                        self.swing_effect_ids.conduit_power = Some(idx as i32)
                    }
                    "minecraft:mining_fatigue" => {
                        self.swing_effect_ids.mining_fatigue = Some(idx as i32)
                    }
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

/// Decode + dispatch a `ClientboundEntityEventPacket` body onto the entity
/// table. Shared by [`play::PlaySession`] and the `eventshot` oracle so there
/// is exactly one decoder — the oracle exercises the production path, not a
/// copy of it.
///
/// Wire form (decompiled `ClientboundEntityEventPacket`): a signed big-endian
/// `i32` entity id followed by a signed `byte` event id — neither is a VarInt.
///
/// The event byte is polymorphic: vanilla dispatches it through the concrete
/// entity's `handleEntityEvent`, so id 4 is "attack" on a warden and unrelated
/// elsewhere. This maps only the three model-visible `(kind, id)` pairs and
/// safely ignores everything else — a missing entity, an entity of the wrong
/// kind, or an unmodelled event id. `warden_type_id` / `armadillo_type_id` are
/// the resolved protocol ids for those kinds (`None` when the caller hasn't
/// supplied them, e.g. the headless protocol harnesses, in which case no event
/// is interpreted). `tick` is the receipt tick stamped as the rig's start.
///
/// Trailing bytes past the id+event are ignored, and a short body decodes to
/// nothing — the "read what the packet needs, ignore the rest" convention every
/// other clientbound handler here follows (`cb_play_ping` reads one i32 and
/// stops, etc.). The frame length already delimits the packet, so extra bytes
/// are not an error to reject.
///
/// Not public: the packet stream reaches this only through
/// [`route_entity_event`] (which owns the id → decoder selection). External
/// callers (the `eventshot` oracle) go through that seam so packet-id selection
/// is exercised too.
pub(crate) fn apply_entity_event(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    warden_type_id: Option<i32>,
    armadillo_type_id: Option<i32>,
    tick: i64,
) {
    use rewo_world::entities::EntityEvent;
    let mut r = PacketReader::new(body);
    // A short / malformed body decodes to nothing (a truncated packet is not
    // an animation).
    let (Ok(eid), Ok(event)) = (r.i32(), r.i8()) else {
        return;
    };
    // Unknown entity → ignore (it may not be tracked / already despawned).
    let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
        return;
    };
    let is = |want: Option<i32>| want == Some(type_id);
    let mapped = match event {
        4 if is(warden_type_id) => Some(EntityEvent::WardenAttack),
        62 if is(warden_type_id) => Some(EntityEvent::WardenSonicBoom),
        64 if is(armadillo_type_id) => Some(EntityEvent::ArmadilloPeek),
        // Wrong kind for this id, or an unmodelled event (hurt, particles,
        // sound, the excluded warden tendril 61, …) — no model-visible effect.
        _ => None,
    };
    if let Some(ev) = mapped {
        entities.start_event(eid, ev, tick);
    }
}

/// The narrowest clientbound-play dispatch seam for entity events: routes a
/// single `(packet id, body)` to [`apply_entity_event`] iff `id` is the
/// resolved `entity_event` id, and returns whether it matched. This is the
/// exact packet-id selection [`play::PlaySession`] uses (its `handle_packet`
/// calls this), factored so the `eventshot` oracle drives packet-id → decoder
/// through production code instead of a private copy. A non-matching id is a
/// no-op returning `false` (the caller's dispatch chain continues).
pub fn route_entity_event(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    warden_type_id: Option<i32>,
    armadillo_type_id: Option<i32>,
    tick: i64,
) -> bool {
    if id == ids.cb_play_entity_event {
        apply_entity_event(body, entities, warden_type_id, armadillo_type_id, tick);
        true
    } else {
        false
    }
}

/// Apply one `ClientboundSetEntityDataPacket` body (VarInt entity id + a
/// `SynchedEntityData` delta stream) to the entity table.
///
/// **Missing entity → whole packet dropped.** Vanilla
/// `ClientPacketListener.handleSetEntityData` does
/// `Entity e = level.getEntity(id); if (e != null) e.getEntityData().assignValues(...)`
/// — an untracked id mutates no state at all. So this looks the entity up first
/// and returns before parsing/applying anything if it's absent.
///
/// **Kind-aware disambiguation of the polymorphic index-16 BOOLEAN** lives here,
/// not in the byte parser: `Allay.DATA_DANCING` shares slot 16 + serializer 8
/// with the modeled baby path (`AgeableMob`/`Zombie.DATA_BABY_ID`), so the raw
/// bit ([`metadata::EntityMeta::bool16`]) routes to `set_dancing` iff the entity
/// is the Allay type, else to `set_baby` (the pre-existing modeled path). This
/// does not claim exhaustive ownership of slot 16 across every entity — only the
/// Allay-vs-baby split the client renders.
///
/// Not public: reached only through [`route_set_entity_data`] so packet-id
/// selection is exercised (the `danceshot` oracle drives that seam).
/// The kind information metadata routing needs, because several slots are
/// polymorphic and only the entity type can separate them.
///
/// Index 16 BOOLEAN is `DATA_BABY_ID` (ageable/zombie), `DATA_DANCING` (Allay)
/// **or** `IS_CELEBRATING` (any Raider). Index 17 BYTE is
/// `DATA_SPELL_CASTING_ID` on a spellcaster illager but a gesture-state enum on
/// a sniffer/armadillo; index 17 BOOLEAN is `IS_CHARGING_CROSSBOW` on a
/// Pillager. Index 15 BYTE is `DATA_MOB_FLAGS_ID` on a `Mob` and unrelated
/// client flags on an `ArmorStand`.
///
/// Every field is optional: absent means "this client could not resolve that
/// kind", and the corresponding slot is then left alone rather than guessed.
#[derive(Clone, Copy, Default)]
pub struct MetaKinds<'a> {
    /// `minecraft:allay` type id (M18).
    pub allay: Option<i32>,
    /// `minecraft:pillager` type id (M20).
    pub pillager: Option<i32>,
    /// The machine-extracted ancestry sets — mob / raider / spellcaster.
    pub classes: Option<&'a rewo_data::entity_types::EntityClasses>,
}

impl<'a> From<Option<i32>> for MetaKinds<'a> {
    /// The M18 shape — an Allay id and nothing else. Kept so the callers that
    /// only care about the dance read the same as before.
    fn from(allay: Option<i32>) -> Self {
        Self {
            allay,
            ..Default::default()
        }
    }
}

pub(crate) fn apply_set_entity_data<'a>(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    kinds: impl Into<MetaKinds<'a>>,
) {
    let kinds: MetaKinds = kinds.into();
    let mut r = PacketReader::new(body);
    let Ok(eid) = r.varint() else {
        return;
    };
    // Vanilla drops metadata for an entity it isn't tracking (getEntity == null).
    // The type id is also what disambiguates the index-16 BOOLEAN below.
    let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
        return;
    };
    let meta = crate::metadata::parse(&mut r);
    if meta.custom_name.is_some() {
        entities.set_custom_name(eid, meta.custom_name);
    }
    if let Some(p) = meta.pose {
        entities.set_pose(eid, p);
    }
    if let Some(s) = meta.gesture_state {
        entities.set_gesture_state(eid, s);
    }
    if let Some(sz) = meta.size {
        entities.set_size(eid, sz);
    }
    if let Some(arm) = meta.main_arm {
        // `HumanoidArm.STREAM_CODEC` is `idMapper(BY_ID, getId)` with
        // `OutOfBoundsStrategy.ZERO`, and LEFT is id 0 — so only 1 is RIGHT.
        entities.set_main_arm(
            eid,
            if arm == 1 {
                rewo_world::entities::HumanoidArm::Right
            } else {
                rewo_world::entities::HumanoidArm::Left
            },
        );
    }
    if let Some(b) = meta.bool16 {
        // Slot 16 BOOLEAN → `DATA_DANCING` on an Allay, `IS_CELEBRATING` on a
        // Raider, otherwise the modeled baby path
        // (`AgeableMob`/`Zombie.DATA_BABY_ID`). Only the kind separates them.
        if kinds.allay == Some(type_id) {
            entities.set_dancing(eid, b);
        } else if kinds.classes.is_some_and(|c| c.is_raider(type_id)) {
            entities.set_celebrating(eid, b);
        } else {
            entities.set_baby(eid, b);
        }
    }
    // Slot 15 BYTE → `Mob.DATA_MOB_FLAGS_ID`. Gated on the type actually being
    // a `Mob`: an `ArmorStand` puts unrelated client flags at the same index
    // with the same serializer, and bit 2 there does not mean left-handed.
    if let Some(flags) = meta.mob_flags {
        if kinds.classes.is_some_and(|c| c.is_mob(type_id)) {
            entities.set_mob_flags(eid, flags);
        }
    }
    // Slot 17 BYTE → `SpellcasterIllager.DATA_SPELL_CASTING_ID`. The gesture
    // enums share the index but not the serializer, and they are already read
    // above; this is the BYTE arm and only a spellcaster may claim it.
    if let Some(spell) = meta.byte17 {
        if kinds.classes.is_some_and(|c| c.is_spellcaster(type_id)) {
            entities.set_spell_casting(eid, spell);
        }
    }
    // Slot 17 BOOLEAN → `Pillager.IS_CHARGING_CROSSBOW`.
    if let Some(charging) = meta.bool17 {
        if kinds.pillager == Some(type_id) {
            entities.set_charging_crossbow(eid, charging);
        }
    }
}

/// The narrowest clientbound-play dispatch seam for entity metadata: routes a
/// single `(packet id, body)` to [`apply_set_entity_data`] iff `id` is the
/// resolved `set_entity_data` id, returning whether it matched. Mirrors
/// [`route_entity_event`] so [`play::PlaySession`] and the `danceshot` oracle
/// drive packet-id → metadata routing through the same production code.
pub fn route_set_entity_data<'a>(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    kinds: impl Into<MetaKinds<'a>>,
) -> bool {
    if id == ids.cb_play_set_entity_data {
        apply_set_entity_data(body, entities, kinds);
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// M19 — combat arm swings (`ClientboundAnimatePacket`) and their inputs
// ---------------------------------------------------------------------------

/// Decode + dispatch a `ClientboundAnimatePacket` body onto the entity table.
///
/// Wire form (decompiled `ClientboundAnimatePacket`): a **VarInt** entity id
/// followed by an **unsigned byte** action — unlike
/// `ClientboundEntityEventPacket`, whose id is a fixed BE `i32`.
///
/// `ClientPacketListener.handleAnimate` maps the action:
///
/// ```text
/// entity == null            -> nothing at all
/// 0 SWING_MAIN_HAND         -> ((LivingEntity)entity).swing(MAIN_HAND)
/// 3 SWING_OFF_HAND          -> ((LivingEntity)entity).swing(OFF_HAND)
/// 2 WAKE_UP                 -> ((Player)entity).stopSleepInBed(false, false)
/// 4 CRITICAL_HIT            -> CRIT particle emitter
/// 5 MAGIC_CRITICAL_HIT      -> ENCHANTED_HIT particle emitter
/// ```
///
/// Only 0 and 3 are model-visible combat swings, and only those touch swing
/// state here. Actions 2/4/5 are a bed exit and two particle emitters — neither
/// modelled — so they are inert, as is any other byte. That is not a
/// simplification: vanilla's own handler does nothing else to the swing.
///
/// `classes` is the machine-extracted living / swing-ticking classification
/// (`rewo_data::entity_types::EntityClasses`). It answers two separate
/// questions the wire cannot: whether the id names a `LivingEntity` at all —
/// vanilla casts, so a boat or an arrow must mutate nothing — and whether that
/// class runs `updateSwingTime`, which only `Player`, `Monster` and `Mannequin`
/// descendants do. `None` (the headless protocol harnesses) interprets nothing.
///
/// Not public: the packet stream reaches this only through [`route_animate`].
pub(crate) fn apply_animate(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) {
    use rewo_world::entities::InteractionHand;
    let mut r = PacketReader::new(body);
    // A short / malformed body decodes to nothing.
    let (Ok(eid), Ok(action)) = (r.varint(), r.u8()) else {
        return;
    };
    // `getEntity(id) == null` → the whole packet is inert.
    let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
        return;
    };
    // `(LivingEntity)entity` — a non-living target would be a ClassCastException
    // server-side, and must certainly not grow swing state here.
    let Some(classes) = classes.filter(|c| c.is_living(type_id)) else {
        return;
    };
    let hand = match action {
        0 => InteractionHand::MainHand,
        3 => InteractionHand::OffHand,
        _ => return,
    };
    entities.swing(eid, hand, classes.ticks_swing(type_id));
}

/// The narrowest clientbound-play dispatch seam for combat swings: routes a
/// single `(packet id, body)` to [`apply_animate`] iff `id` is the resolved
/// `animate` id, returning whether it matched. Mirrors [`route_entity_event`] /
/// [`route_set_entity_data`] so [`play::PlaySession`] and the `swingshot` oracle
/// drive packet-id → decoder through the same production code.
pub fn route_animate(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) -> bool {
    if id == ids.cb_play_animate {
        apply_animate(body, entities, classes);
        true
    } else {
        false
    }
}

/// Apply one `ClientboundSetEquipmentPacket` body — the held items that decide
/// a swing's duration and animation type.
///
/// Wire form (decompiled):
///
/// ```text
/// VarInt entity
/// do { byte slotId; ItemStack(OPTIONAL_STREAM_CODEC) } while (slotId & 0x80) != 0
/// ```
///
/// `EquipmentSlot.VALUES.get(slotId & 127)` is an **ordinal** lookup, not the
/// enum's `id` field: 0 MAINHAND, 1 OFFHAND, 2 FEET, 3 LEGS, 4 CHEST, 5 HEAD,
/// 6 BODY, 7 SADDLE. Only the two hands change a swing input, but every stack
/// still has to be *decoded* to reach the next slot.
///
/// `handleSetEquipment` requires `getEntity(id) instanceof LivingEntity`; an
/// untracked or non-living id mutates nothing.
///
/// **Fail-closed, twice over.** A stack whose component patch cannot be walked
/// leaves the reader mid-value, so the packet stops there — every later slot
/// would be parsed out of garbage. And a stack whose swing animation cannot be
/// resolved exactly (an unwalkable patch, or an item id the registry does not
/// contain) is stored as [`HandItem::Unknown`] rather than guessed: the caller
/// then suppresses that entity's combat pose and CEM `swing_progress` until an
/// exact update repairs it. The slots already read stay applied, exactly as
/// vanilla's per-pair `forEach` would have.
pub(crate) fn apply_set_equipment(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    data: &item_stack::SwingWireData,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) {
    use item_stack::{SwingResolution, WireSlot};
    use rewo_world::entities::{HandItem, HeldItem, InteractionHand};
    let mut r = PacketReader::new(body);
    let Ok(eid) = r.varint() else {
        return;
    };
    let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
        return;
    };
    if !classes.is_some_and(|c| c.is_living(type_id)) {
        return; // `instanceof LivingEntity` failed
    }
    loop {
        let Ok(slot_id) = r.i8() else {
            return;
        };
        let Ok(slot) = item_stack::read_optional(&mut r, data.components) else {
            return; // truncated — stop, don't guess at the rest
        };
        let hand = match slot_id & 127 {
            0 => Some(InteractionHand::MainHand),
            1 => Some(InteractionHand::OffHand),
            _ => None, // armour / body / saddle: decoded, then discarded
        };
        if let Some(hand) = hand {
            let item = match slot {
                WireSlot::Empty => HandItem::Empty,
                WireSlot::Stack(s) => match item_stack::resolve_swing(&s, &data.prototypes) {
                    SwingResolution::Exact(swing) => HandItem::Held(HeldItem {
                        item_id: s.item_id,
                        swing,
                    }),
                    SwingResolution::Unknown(why) => {
                        warn_unknown_swing(s.item_id, why);
                        HandItem::Unknown
                    }
                },
            };
            entities.set_hand_item(eid, hand, item);
        }
        // The reader is parked mid-component; there is no valid next slot.
        if !slot.aligned() || slot_id & -128i8 == 0 {
            return;
        }
    }
}

/// Warn once per (reason, item) about an unresolvable swing input.
///
/// A server that sends an unwalkable patch sends it on *every* equipment
/// update, so an unconditional log would be unbounded spam. The set is keyed by
/// the pair, so a genuinely new item is still reported.
fn warn_unknown_swing(item_id: i32, why: item_stack::UnknownSwing) {
    use std::sync::Mutex;
    static SEEN: Mutex<Option<std::collections::HashSet<(i32, u8)>>> = Mutex::new(None);
    let key = (
        item_id,
        match why {
            item_stack::UnknownSwing::UnwalkableComponent => 0u8,
            item_stack::UnknownSwing::UnregisteredItem => 1u8,
        },
    );
    let first = match SEEN.lock() {
        Ok(mut guard) => guard.get_or_insert_with(Default::default).insert(key),
        Err(_) => false,
    };
    if first {
        log::warn!(
            "net: item {item_id} has an unresolvable swing animation ({why:?}) — its holder's \
             combat pose is suppressed until an exact equipment update arrives"
        );
    }
}

/// Dispatch seam for [`apply_set_equipment`], mirroring [`route_animate`].
/// `data` is `None` on the headless protocol harnesses, which then interpret no
/// equipment (every swing keeps the bare-hand default).
pub fn route_set_equipment(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    data: Option<&item_stack::SwingWireData>,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) -> bool {
    if id == ids.cb_play_set_equipment {
        if let Some(data) = data {
            apply_set_equipment(body, entities, data, classes);
        }
        true
    } else {
        false
    }
}

/// Raw `minecraft:mob_effect` registry ids of the three effects
/// `LivingEntity.getCurrentSwingDuration()` consults. Captured from
/// `registry_data` like the M13 lightmap's night-vision / darkness ids, never
/// assumed from bootstrap order. `None` = this server didn't sync that entry,
/// so nothing can match it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwingEffectIds {
    pub haste: Option<i32>,
    pub conduit_power: Option<i32>,
    pub mining_fatigue: Option<i32>,
}

impl SwingEffectIds {
    fn effect_of(&self, id: i32) -> Option<rewo_world::entities::SwingEffect> {
        use rewo_world::entities::SwingEffect;
        if self.haste == Some(id) {
            Some(SwingEffect::Haste)
        } else if self.conduit_power == Some(id) {
            Some(SwingEffect::ConduitPower)
        } else if self.mining_fatigue == Some(id) {
            Some(SwingEffect::MiningFatigue)
        } else {
            None
        }
    }
}

/// Apply an `update_mob_effect` / `remove_mob_effect` body to the per-entity
/// swing-duration effects. `add` selects which packet this is.
///
/// `handleUpdateMobEffect` applies to whatever `level.getEntity(id)` returns,
/// so this handler is written for any living entity — but **the server's send
/// scope is much narrower than that**, and the reachability claim has to match
/// it. In 26.2 the only senders are `ServerPlayer.onEffectAdded`/`onEffectUpdated`
/// (to that player about *itself*), `PlayerList` (a player's own effects on
/// join/respawn) and `LivingEntity.sendEffectToPassengers` (to `ServerPlayer`s
/// **riding** the affected entity). There is no `sendToTrackingPlayers` for
/// these packets, so an ordinary tracked mob's haste is never transmitted: the
/// duration adjustment is exercised by the local player and by a ridden
/// vehicle, and by nothing else.
///
/// Client `hasEffect` is a plain `activeEffects.containsKey` and `tickClient`
/// never removes — an effect only leaves the map when a remove packet arrives —
/// so no expiry clock belongs here (see `rewo_world::entities::SwingEffect`).
pub fn apply_swing_effect(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    ids: SwingEffectIds,
    add: bool,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) {
    let (eid, effect_id, amplifier) = if add {
        match crate::effects::parse_update(body) {
            Some(u) => (u.entity_id, u.effect_id, Some(u.amplifier)),
            None => return,
        }
    } else {
        match crate::effects::parse_remove(body) {
            Some(rem) => (rem.entity_id, rem.effect_id, None),
            None => return,
        }
    };
    let Some(effect) = ids.effect_of(effect_id) else {
        return;
    };
    // `getEntity(id) instanceof LivingEntity`.
    let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
        return;
    };
    if !classes.is_some_and(|c| c.is_living(type_id)) {
        return;
    }
    entities.set_swing_effect(eid, effect, amplifier);
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

#[cfg(test)]
mod entity_event_tests {
    use super::apply_entity_event;
    use rewo_world::entities::{EntityEvent, EntityState, EntityTable};

    const WARDEN: i32 = 100;
    const ARMADILLO: i32 = 200;
    const PIG: i32 = 300;

    /// Build a `ClientboundEntityEventPacket` body independently of any writer
    /// under test: a big-endian signed i32 entity id then a signed byte event.
    fn body(entity: i32, event: i8) -> Vec<u8> {
        let mut b = entity.to_be_bytes().to_vec();
        b.push(event as u8);
        b
    }

    fn table() -> EntityTable {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, WARDEN, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.add(2, EntityState::new(0, ARMADILLO, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.add(3, EntityState::new(0, PIG, 0.0, 0.0, 0.0, 0.0, 0.0));
        t
    }

    fn apply(t: &mut EntityTable, entity: i32, event: i8, tick: i64) {
        apply_entity_event(&body(entity, event), t, Some(WARDEN), Some(ARMADILLO), tick);
    }

    #[test]
    fn maps_the_three_model_visible_events_by_kind() {
        let mut t = table();
        apply(&mut t, 1, 4, 10);
        apply(&mut t, 1, 62, 11);
        apply(&mut t, 2, 64, 12);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(10));
        assert_eq!(t.event_start(1, EntityEvent::WardenSonicBoom), Some(11));
        assert_eq!(t.event_start(2, EntityEvent::ArmadilloPeek), Some(12));
    }

    #[test]
    fn ignores_wrong_kind_for_an_id() {
        let mut t = table();
        // Event 4 on the armadillo and the pig is not a warden attack.
        apply(&mut t, 2, 4, 10);
        apply(&mut t, 3, 4, 10);
        // Event 64 on the warden is not an armadillo peek.
        apply(&mut t, 1, 64, 10);
        assert_eq!(t.event_start(2, EntityEvent::WardenAttack), None);
        assert_eq!(t.event_start(3, EntityEvent::WardenAttack), None);
        assert_eq!(t.event_start(1, EntityEvent::ArmadilloPeek), None);
    }

    #[test]
    fn ignores_unknown_event_ids_and_the_excluded_tendril() {
        let mut t = table();
        apply(&mut t, 1, 61, 10); // warden tendril shiver — explicitly excluded
        apply(&mut t, 1, 3, 10); // LivingEntity death
        apply(&mut t, 2, 1, 10); // arbitrary status
        for ev in [
            EntityEvent::WardenAttack,
            EntityEvent::WardenSonicBoom,
            EntityEvent::ArmadilloPeek,
        ] {
            assert_eq!(t.event_start(1, ev), None);
            assert_eq!(t.event_start(2, ev), None);
        }
    }

    #[test]
    fn ignores_a_missing_entity() {
        let mut t = table();
        apply(&mut t, 999, 4, 10); // no such entity
        // Nothing recorded anywhere.
        assert_eq!(t.event_start(999, EntityEvent::WardenAttack), None);
    }

    #[test]
    fn a_repeated_event_restarts_the_clock() {
        let mut t = table();
        apply(&mut t, 1, 4, 10);
        apply(&mut t, 1, 4, 55);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(55));
    }

    #[test]
    fn a_truncated_body_is_ignored() {
        let mut t = table();
        // Only 3 of the 4 id bytes — the decode must not panic or record.
        apply_entity_event(&[0, 0, 0], &mut t, Some(WARDEN), Some(ARMADILLO), 10);
        // Full id but no event byte.
        apply_entity_event(&1i32.to_be_bytes(), &mut t, Some(WARDEN), Some(ARMADILLO), 10);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), None);
    }

    #[test]
    fn trailing_bytes_after_the_event_are_ignored() {
        // Frame length delimits the packet; extra bytes are not an error to
        // reject (same convention as every other handler here).
        let mut t = table();
        let mut b = body(1, 4);
        b.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        apply_entity_event(&b, &mut t, Some(WARDEN), Some(ARMADILLO), 33);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(33));
    }

    #[test]
    fn no_type_ids_configured_means_no_interpretation() {
        // The headless protocol harnesses leave the kind ids unset.
        let mut t = table();
        apply_entity_event(&body(1, 4), &mut t, None, None, 10);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), None);
    }
}

#[cfg(test)]
mod animate_tests {
    use super::{apply_animate, apply_set_equipment, apply_swing_effect, SwingEffectIds};
    use rewo_data::components::DataComponentIds;
    use rewo_data::entity_types::EntityClasses;
    use rewo_world::entities::{
        EntityState, EntityTable, HandItem, HumanoidArm, InteractionHand,
    };

    /// Stand-in type ids: a player, a `Monster` descendant, a living
    /// non-`Monster` (`Cow`), a `Mannequin`, and a non-living entity (a boat).
    const PLAYER: i32 = 148;
    const ZOMBIE: i32 = 151;
    const COW: i32 = 25;
    const MANNEQUIN: i32 = 77;
    const BOAT: i32 = 9;

    /// The classification these tests run against — the same *shape* as the
    /// generated one (`Player`/`Monster`/`Mannequin` tick; the cow is living but
    /// does not; the boat is not living at all).
    fn classes() -> EntityClasses {
        EntityClasses::from_raw_ids(
            &[PLAYER, ZOMBIE, COW, MANNEQUIN],
            &[PLAYER, ZOMBIE, MANNEQUIN],
        )
    }

    /// A `ClientboundAnimatePacket` body: VarInt entity id + unsigned byte
    /// action. Built independently of any writer under test.
    fn body(eid: i32, action: u8) -> Vec<u8> {
        let mut b = Vec::new();
        varint(eid, &mut b);
        b.push(action);
        b
    }

    fn varint(v: i32, out: &mut Vec<u8>) {
        let mut n = v as u32;
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn table() -> EntityTable {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, PLAYER, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.add(2, EntityState::new(0, COW, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.add(3, EntityState::new(0, ZOMBIE, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.add(4, EntityState::new(0, MANNEQUIN, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.add(5, EntityState::new(0, BOAT, 0.0, 0.0, 0.0, 0.0, 0.0));
        t
    }

    fn swing(t: &mut EntityTable, eid: i32, action: u8) {
        apply_animate(&body(eid, action), t, Some(&classes()));
    }

    #[test]
    fn action_0_and_3_swing_the_two_hands() {
        let mut t = table();
        swing(&mut t, 1, 0);
        assert_eq!(t.swing_debug(1).unwrap().4, Some(InteractionHand::MainHand));
        assert_eq!(t.attack_arm(1), HumanoidArm::Right);
        // A second swing at swingTime = -1 is always accepted (swingTime < 0).
        swing(&mut t, 1, 3);
        assert_eq!(t.swing_debug(1).unwrap().4, Some(InteractionHand::OffHand));
        assert_eq!(t.attack_arm(1), HumanoidArm::Left);
    }

    #[test]
    fn the_other_actions_never_touch_the_swing() {
        // 2 wake-up, 4 crit particles, 5 enchanted-hit particles, plus ids the
        // handler has no branch for at all.
        for action in [1u8, 2, 4, 5, 6, 64, 255] {
            let mut t = table();
            swing(&mut t, 1, action);
            assert_eq!(t.swing_debug(1), None, "action {action} started a swing");
        }
    }

    #[test]
    fn a_missing_entity_or_truncated_body_is_inert() {
        let mut t = table();
        swing(&mut t, 99, 0); // untracked
        assert_eq!(t.swing_debug(99), None);
        apply_animate(&[], &mut t, Some(&classes()));
        apply_animate(&[1], &mut t, Some(&classes())); // id but no action
        assert_eq!(t.swing_debug(1), None);
    }

    #[test]
    fn the_entity_id_is_a_varint_not_a_fixed_int() {
        // 300 needs two VarInt bytes. Reading the same body as a big-endian
        // i32 (the *entity_event* shape) would target a different entity, so
        // this distinguishes the two wire forms.
        let mut t = EntityTable::default();
        t.add(300, EntityState::new(0, PLAYER, 0.0, 0.0, 0.0, 0.0, 0.0));
        let b = body(300, 0);
        assert_eq!(b.len(), 3, "two VarInt bytes + the action");
        apply_animate(&b, &mut t, Some(&classes()));
        assert!(t.swing_debug(300).is_some());
    }

    #[test]
    fn the_generated_ticking_set_decides_whose_clock_runs() {
        let mut t = table();
        for eid in [1, 2, 3, 4] {
            swing(&mut t, eid, 0);
        }
        for _ in 0..3 {
            t.tick_lerp();
        }
        // Player, Monster descendant and Mannequin all run updateSwingTime…
        for eid in [1, 3, 4] {
            assert_eq!(t.swing_debug(eid).unwrap().1, 2, "entity {eid} should tick");
        }
        // …a living non-Monster accepts the swing and never advances it.
        assert_eq!(t.swing_debug(2).unwrap().1, -1, "the cow must not tick");
        assert_eq!(t.attack_anim(2, 1.0), 0.0);
    }

    #[test]
    fn a_non_living_entity_is_inert_everywhere() {
        let mut t = table();
        swing(&mut t, 5, 0);
        assert_eq!(t.swing_debug(5), None, "a boat cannot swing");
        // …and equipment/effects for it are dropped too.
        let ids = SwingEffectIds {
            haste: Some(3),
            conduit_power: Some(29),
            mining_fatigue: Some(4),
        };
        apply_swing_effect(&[5, 3, 1, 100, 0], &mut t, ids, true, Some(&classes()));
        assert_eq!(t.current_swing_duration(5), Some(6), "no haste applied");
        let comps = DataComponentIds {
            swing_animation: 40,
            damage: 3,
        };
        let data = super::item_stack::SwingWireData {
            prototypes: unreachable_prototypes(),
            components: comps,
        };
        apply_set_equipment(&equipment_body(5, 0, 949), &mut t, &data, Some(&classes()));
        assert_eq!(t.hand_item(5, InteractionHand::MainHand), HandItem::Empty);
    }

    /// The equipment path needs a prototype table, which only exists with a
    /// real registry — these unit tests only need the *gate*, so the table is
    /// never consulted (the entity is rejected first). `swingshot` covers the
    /// resolved path against the live registry.
    fn unreachable_prototypes() -> rewo_data::swing_anim::SwingAnimations {
        // A registry-less table is impossible to build honestly, so borrow the
        // real one if the reports are present and skip the assertion otherwise.
        let paths = rewo_data::DataPaths::for_version("26.2").expect("config dir");
        let items = rewo_data::items::Items::load(&paths.registries_json())
            .expect("registries.json for the equipment gate test");
        rewo_data::swing_anim::SwingAnimations::resolve(&items).expect("prototypes")
    }

    /// A one-slot `ClientboundSetEquipmentPacket` body with a plain stack.
    fn equipment_body(eid: i32, ordinal: u8, item: i32) -> Vec<u8> {
        let mut b = Vec::new();
        varint(eid, &mut b);
        b.push(ordinal);
        varint(1, &mut b); // count
        varint(item, &mut b);
        varint(0, &mut b); // added
        varint(0, &mut b); // removed
        b
    }

    /// An `update_mob_effect` body: VarInt entity, effect, amplifier, duration
    /// + a flags byte. Every field is kept below 128 so its VarInt is one byte.
    fn effect_body(eid: u8, effect: u8, amp: u8, duration: u8, flags: u8) -> Vec<u8> {
        assert!([eid, effect, amp, duration].iter().all(|v| *v < 128));
        vec![eid, effect, amp, duration, flags]
    }

    #[test]
    fn swing_effects_track_living_entities_and_only_the_three_ids() {
        let ids = SwingEffectIds {
            haste: Some(3),
            conduit_power: Some(29),
            mining_fatigue: Some(4),
        };
        let c = classes();
        let mut t = table();
        // Haste II on the cow (living, even though it never ticks a swing).
        apply_swing_effect(&effect_body(2, 3, 1, 100, 0), &mut t, ids, true, Some(&c));
        assert_eq!(t.current_swing_duration(2), Some(6 - 2));
        // An unrelated effect id changes nothing.
        apply_swing_effect(&effect_body(2, 99, 4, 100, 0), &mut t, ids, true, Some(&c));
        assert_eq!(t.current_swing_duration(2), Some(4));
        // The remove packet drops it.
        apply_swing_effect(&[2, 3], &mut t, ids, false, Some(&c));
        assert_eq!(t.current_swing_duration(2), Some(6));
        // An untracked entity is inert.
        apply_swing_effect(&effect_body(9, 3, 1, 100, 0), &mut t, ids, true, Some(&c));
        assert_eq!(t.current_swing_duration(9), Some(6));
        // A server that synced no ids matches nothing.
        let mut t2 = table();
        apply_swing_effect(
            &effect_body(1, 3, 1, 100, 0),
            &mut t2,
            SwingEffectIds::default(),
            true,
            Some(&c),
        );
        assert_eq!(t2.current_swing_duration(1), Some(6));
    }
}

#[cfg(test)]
mod set_entity_data_tests {
    use super::apply_set_entity_data;
    use rewo_world::entities::{EntityState, EntityTable};

    const ALLAY: i32 = 400;
    const ZOMBIE: i32 = 500;

    /// A `ClientboundSetEntityDataPacket` body: VarInt entity id then the
    /// `SynchedEntityData` delta stream (index u8 + serializer VarInt + value)
    /// terminated by 0xFF. Built independently of any writer under test. `eid`
    /// is kept < 128 so its VarInt is a single byte.
    fn body(eid: u8, index: u8, serializer: u8, value: &[u8]) -> Vec<u8> {
        let mut b = vec![eid, index, serializer];
        b.extend_from_slice(value);
        b.push(0xFF);
        b
    }

    /// Index-16 BOOLEAN carrying `true` — polymorphic (dancing on an Allay,
    /// baby elsewhere).
    fn index16_bool_true(eid: u8) -> Vec<u8> {
        body(eid, 16, 8, &[0x01])
    }

    #[test]
    fn index16_boolean_on_an_allay_is_dancing_not_baby() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, ALLAY, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&index16_bool_true(1), &mut t, Some(ALLAY));
        // The bit drives dancing (rendered from `tick_lerp`) — NOT baby. At
        // dancing_ticks=0, isSpinning() = 0 % 55 < 15 = true (vanilla reads the
        // pre-increment counter the instant dancing flips on).
        assert_eq!(t.allay_dance_render(1, 1.0), Some((true, 0.0)));
        assert!(!t.is_baby(1), "the Allay's index-16 BOOLEAN must not set baby");
    }

    #[test]
    fn index16_boolean_on_a_non_allay_is_baby_not_dancing() {
        let mut t = EntityTable::default();
        t.add(2, EntityState::new(0, ZOMBIE, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&index16_bool_true(2), &mut t, Some(ALLAY));
        assert!(t.is_baby(2), "a zombie's index-16 BOOLEAN is DATA_BABY_ID");
        assert_eq!(t.allay_dance_render(2, 1.0), None, "a zombie never dances");
    }

    #[test]
    fn index16_int_is_size_regardless_of_kind() {
        // A slime size update: index 16, INT (serializer 1), value 4. The INT
        // serializer means size, never the polymorphic BOOLEAN.
        let mut t = EntityTable::default();
        t.add(3, EntityState::new(0, 600, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&body(3, 16, 1, &[0x04]), &mut t, Some(ALLAY));
        assert_eq!(t.size(3), Some(4));
        assert!(!t.is_baby(3));
        assert_eq!(t.allay_dance_render(3, 1.0), None);
    }

    #[test]
    fn dancing_toggles_off_and_untracked_ids_are_inert() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, ALLAY, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&index16_bool_true(1), &mut t, Some(ALLAY));
        t.tick_lerp();
        assert!(t.allay_dance_render(1, 1.0).is_some());
        // A false update stops the dance.
        apply_set_entity_data(&body(1, 16, 8, &[0x00]), &mut t, Some(ALLAY));
        assert_eq!(t.allay_dance_render(1, 1.0), None);
        // Vanilla drops metadata for an untracked id (getEntity == null): no
        // state mutation at all — NOT a baby fallback.
        apply_set_entity_data(&index16_bool_true(9), &mut t, Some(ALLAY));
        assert!(!t.is_baby(9), "untracked id must not be marked baby");
        assert_eq!(t.allay_dance_render(9, 1.0), None, "untracked id must not dance");
    }
}
