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

pub mod abilities;
pub mod attributes;
pub mod biome_parse;
pub mod boss_bar;
pub mod chat_sign;
pub mod chat_style;
pub mod component_wire;
pub mod crypt;
pub mod dimension_parse;
pub mod enchantment_parse;
pub mod trim_parse;
pub mod variant_parse;
pub mod effects;
pub mod game_event;
pub mod ids;
pub mod item_stack;
pub mod metadata;
pub mod motion;
pub mod play;
pub mod record;
pub mod scoreboard;
pub mod skins;
pub mod sounds;
pub mod spawn_info;
pub mod tab_list_text;
pub mod tags;
pub mod teams;
pub mod view_area;

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
    /// The `minecraft:enchantment` registry in wire order — the index is the
    /// protocol id a component patch carries (M42).
    enchantments: Vec<crate::enchantment_parse::EnchantmentDef>,
    trim_materials: Vec<crate::trim_parse::TrimMaterialDef>,
    trim_patterns: Vec<crate::trim_parse::TrimPatternDef>,
    /// The three metadata-variant registries (M64), in raw wire order.
    cat_variants: Vec<crate::variant_parse::MobVariantDef>,
    wolf_variants: Vec<crate::variant_parse::MobVariantDef>,
    frog_variants: Vec<crate::variant_parse::MobVariantDef>,
    /// The server's datapack tags (M69), applied during configuration and
    /// handed to the play session. Decode and state only — nothing reads them
    /// yet; `crate::tags` says what wiring them would take.
    tags: crate::tags::TagOverrides,
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
            enchantments: Vec::new(),
            trim_materials: Vec::new(),
            cat_variants: Vec::new(),
            wolf_variants: Vec::new(),
            frog_variants: Vec::new(),
            trim_patterns: Vec::new(),
            tags: crate::tags::TagOverrides::default(),
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
                x if x == self.ids.cb_config_update_tags => {
                    // M69 — the server's datapack tags. This is where a
                    // vanilla server sends them on a normal join; the play
                    // copy (`route_tags`) is the datapack-reload case. Both
                    // reach the same walk.
                    apply_update_tags(&self.packet[body..], &mut self.tags);
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
        if registry == crate::enchantment_parse::ENCHANTMENT_REGISTRY {
            // Datapack-driven, so both the contents and the id order are the
            // server's — nothing here may be assumed from bootstrap order.
            self.enchantments = crate::enchantment_parse::parse_enchantment_registry(&mut r, count);
            log::info!("net: {} enchantment(s) synced", self.enchantments.len());
            return Ok(());
        }
        // M48: the two trim registries, datapack-driven for the same reason.
        if registry == crate::trim_parse::TRIM_MATERIAL_REGISTRY {
            self.trim_materials = crate::trim_parse::parse_trim_material_registry(&mut r, count);
            log::info!("net: {} trim material(s) synced", self.trim_materials.len());
            return Ok(());
        }
        if registry == crate::trim_parse::TRIM_PATTERN_REGISTRY {
            self.trim_patterns = crate::trim_parse::parse_trim_pattern_registry(&mut r, count);
            log::info!("net: {} trim pattern(s) synced", self.trim_patterns.len());
            return Ok(());
        }
        // M64: the three mob-variant registries, datapack-driven for the
        // same reason — the index is the raw holder id the metadata carries.
        if registry == crate::variant_parse::CAT_VARIANT_REGISTRY {
            self.cat_variants = crate::variant_parse::parse_single_asset_registry(&mut r, count);
            log::info!("net: {} cat variant(s) synced", self.cat_variants.len());
            return Ok(());
        }
        if registry == crate::variant_parse::WOLF_VARIANT_REGISTRY {
            self.wolf_variants = crate::variant_parse::parse_wolf_variant_registry(&mut r, count);
            log::info!("net: {} wolf variant(s) synced", self.wolf_variants.len());
            return Ok(());
        }
        if registry == crate::variant_parse::FROG_VARIANT_REGISTRY {
            self.frog_variants = crate::variant_parse::parse_single_asset_registry(&mut r, count);
            log::info!("net: {} frog variant(s) synced", self.frog_variants.len());
            return Ok(());
        }
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
    classes: Option<&rewo_data::entity_types::EntityClasses>,
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
    // Event 3 is the death event, and it is **kind-independent** — every
    // `LivingEntity` handles it. `LivingEntity.handleEntityEvent(3)` plays the
    // death sound and then, for anything that is not a `Player`, sets health to
    // zero and calls `die()`. The living gate matters because the switch lives
    // on `LivingEntity`: a boat or an arrow never reaches it (M24).
    if event == 3 && classes.is_some_and(|c| c.is_living(type_id)) {
        entities.kill(eid, classes.is_some_and(|c| c.is_player(type_id)));
    }
    let mapped = match event {
        4 if is(warden_type_id) => Some(EntityEvent::WardenAttack),
        62 if is(warden_type_id) => Some(EntityEvent::WardenSonicBoom),
        64 if is(armadillo_type_id) => Some(EntityEvent::ArmadilloPeek),
        // `Warden.handleEntityEvent(61)` is `tendrilAnimation = 10` — a plain
        // 10-tick countdown rather than an `AnimationState`, but it is stamped
        // the same way and read back as `max(0, 10 − elapsed) / 10` (M52).
        61 if is(warden_type_id) => Some(EntityEvent::WardenTendril),
        // Wrong kind for this id, or an unmodelled event (hurt, particles,
        // sound, …) — no model-visible effect.
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
/// The narrowest clientbound-play dispatch seam for `block_entity_data`
/// (M25): routes a single `(packet id, body)` to the world iff `id` is the
/// resolved id, and returns whether it matched.
///
/// Exists for the same reason [`route_entity_event`] does — so
/// `blockentityshot` exercises the real packet-id selection and the real
/// decode, instead of a private copy that could drift from `PlaySession`.
pub fn route_block_entity_data(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    world: &mut rewo_world::World,
) -> bool {
    if id != ids.cb_play_block_entity_data {
        return false;
    }
    let mut r = PacketReader::new(body);
    if let (Ok((x, y, z)), Ok(type_id), Ok(data)) = (r.position(), r.varint(), r.nbt()) {
        let pos = rewo_world::block_entities::BlockEntityPos { x, y, z };
        // Vanilla ignores a packet for a position with no block entity
        // (`getBlockEntity` returns null and the handler returns), so a stray
        // packet cannot paint one into thin air.
        let applied = world.set_block_entity_data(pos, type_id, data);
        log::debug!("net: block_entity_data ({x},{y},{z}) type={type_id} applied={applied}");
    }
    true
}

/// The clientbound-play dispatch seam for `block_event` (container lids).
///
/// Wire form (26.2 `ClientboundBlockEventPacket`):
///
/// ```text
/// BlockPos    pos      // the packed long
/// u8          b0       // unsigned
/// u8          b1       // unsigned
/// VarInt      block    // ByteBufCodecs.registry(BLOCK) — a raw BLOCK id
/// ```
///
/// Note the trailing id is a **block** registry id, not a block *state* id —
/// vanilla uses it only to confirm the block at the position still matches
/// before dispatching, and a mismatch drops the event. Rewo checks the
/// position has a block entity instead, which is the same guard one level
/// down: `Level.blockEvent` ends in `getBlockState(pos).triggerEvent(...)`,
/// and that forwards straight to the block entity.
///
/// **`b0 == 1` is not one meaning.** It selects a different body per block
/// entity — a chest's viewer count, a shulker box's open/close pair, a bell's
/// click *direction* — so `types` is what decides which. Before M26 this
/// routed every `b0 == 1` to the chest lid, and a bell rung from any side but
/// below (`b1 != 0`, a `Direction.from3DDataValue`) opened a lid at the bell.
pub fn route_block_event(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    types: rewo_world::block_entities::BlockEventTypes,
    game_time: i64,
    world: &mut rewo_world::World,
) -> bool {
    if id != ids.cb_play_block_event {
        return false;
    }
    let mut r = PacketReader::new(body);
    if let (Ok((x, y, z)), Ok(b0), Ok(b1)) = (r.position(), r.u8(), r.u8()) {
        let pos = rewo_world::block_entities::BlockEntityPos { x, y, z };
        // The arrival TIME is part of the payload for one of these bodies: a
        // pot's wobble is timed from the game tick its event landed on.
        let consumed = world
            .block_entities
            .trigger_block_event(types, pos, b0, b1, game_time);
        log::debug!("net: block_event ({x},{y},{z}) b0={b0} b1={b1} consumed={consumed}");
    }
    true
}

pub fn route_entity_event(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    warden_type_id: Option<i32>,
    armadillo_type_id: Option<i32>,
    tick: i64,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) -> bool {
    if id == ids.cb_play_entity_event {
        apply_entity_event(body, entities, warden_type_id, armadillo_type_id, tick, classes);
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
/// `ClientboundDamageEventPacket` → `LivingEntity.handleDamageEvent` (M21).
///
/// Body, in wire order:
///
/// ```text
/// VarInt entityId
/// VarInt damageTypeHolder     // ByteBufCodecs.holderRegistry — RAW 0-based id
/// VarInt sourceCauseId  + 1   // writeOptionalEntityId; -1 means "none"
/// VarInt sourceDirectId + 1
/// Optional<Vec3>              // bool + 3 × f64
/// ```
///
/// **The damage-type holder is `holderRegistry`, so the id is raw and 0-based**
/// — not `ByteBufCodecs.holder`'s inline / `id+1` scheme. That distinction has
/// already cost this project once (the M14 play-login dimension holder), and it
/// matters here only for staying aligned with the rest of the body: nothing
/// model-visible depends on *which* damage type it was. The trailing fields are
/// still walked in full, because a short read would leave the stream misaligned
/// for the next packet in the same buffer.
///
/// Vanilla drops the event for an entity it is not tracking
/// (`if (entity != null)`), and `handleDamageEvent` is a `LivingEntity`
/// override — a non-living entity has `Entity.handleDamageEvent`, which does
/// not touch a hurt clock it has no field for. Both gates are applied here.
pub(crate) fn apply_damage_event(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) {
    let mut r = PacketReader::new(body);
    let Ok(eid) = r.varint() else {
        return;
    };
    // Walk the rest of the body even though none of it is model-visible: a
    // decoder that stops early is a decoder that desyncs.
    if r.varint().is_err() {
        return; // damage type holder (raw registry id)
    }
    if r.varint().is_err() || r.varint().is_err() {
        return; // cause / direct entity ids, each written as id + 1
    }
    match r.bool() {
        Ok(true) => {
            if r.take(24).is_err() {
                return; // source position: 3 × f64
            }
        }
        Ok(false) => {}
        Err(_) => return,
    }
    let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
        return; // getEntity(id) == null
    };
    if !classes.is_some_and(|c| c.is_living(type_id)) {
        return; // Entity.handleDamageEvent has no hurt clock to arm
    }
    entities.hurt(eid);
}

/// The narrowest clientbound-play dispatch seam for the damage event: routes a
/// single `(packet id, body)` to [`apply_damage_event`] iff `id` is the
/// resolved `damage_event` id, returning whether it matched. Mirrors
/// [`route_animate`] so `play::PlaySession` and the `hurtshot` oracle drive
/// packet-id → hurt routing through the same production code.
pub fn route_damage_event(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) -> bool {
    if id == ids.cb_play_damage_event {
        apply_damage_event(body, entities, classes);
        true
    } else {
        false
    }
}

/// `ClientboundUpdateAttributesPacket` → `handleUpdateAttributes` (M55).
///
/// The handler is narrower than the packet: for each snapshot it calls
/// `AttributeMap.getInstance`, which returns null — logged as `"Entity {} does
/// not have attribute {}"` and **skipped** — when the entity type's
/// `AttributeSupplier` does not declare that attribute. So the supplier is a
/// filter on receipt, not merely a source of defaults, and a zombie's
/// `spawn_reinforcements` snapshot must not stick to a pig.
///
/// Three gates before anything is stored, matching the handler's own order:
///
/// * an untracked entity is inert (`getEntity(id) == null` → the whole `if`
///   body is skipped);
/// * a non-living entity is inert. Vanilla *throws* `IllegalStateException`
///   here ("Server tried to update attributes of a non-living entity"), killing
///   the packet thread; dropping the packet is the closest safe equivalent and
///   is the same choice [`apply_damage_event`] makes;
/// * an attribute id outside the registry is dropped, where vanilla's
///   `byIdOrThrow` would throw.
///
/// Storage is per attribute and wholesale: `setBaseValue` → `removeModifiers()`
/// → add each, so a snapshot replaces that attribute's state and leaves the
/// rest of the entity's attributes alone.
///
/// Not public: the packet stream reaches this only through
/// [`route_update_attributes`] (which owns the id → decoder selection).
/// External callers (the `attributeshot` oracle) go through that seam so
/// packet-id selection is exercised too.
pub(crate) fn apply_update_attributes(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
    types: Option<&rewo_data::entity_types::EntityTypes>,
    reg: Option<&rewo_data::attributes::AttributeRegistry>,
) {
    // Decode first and completely: a body that does not fully parse changes
    // nothing, so a malformed packet can never half-apply.
    let Some(packet) = crate::attributes::parse(body) else {
        return;
    };
    let Some(type_id) = entities.get(packet.entity_id).map(|e| e.type_id) else {
        return; // getEntity(id) == null
    };
    // Measured, not assumed: `is_living` and "has an `AttributeSupplier`" are
    // the same 93 types of 158 (`attributeshot` d14 asserts it over the whole
    // registry), so this gate changes no outcome the `defaults_for` lookup
    // below would not also reject. It is kept because it is the gate
    // `handleUpdateAttributes` actually has, and it is the documented reason a
    // boat is inert; if a version ever ships a non-living type with a supplier,
    // d14 fails and this stops being redundant.
    if !classes.is_some_and(|c| c.is_living(type_id)) {
        return; // not a LivingEntity — vanilla throws, we drop
    }
    let (Some(reg), Some(types)) = (reg, types) else {
        return; // without the registry nothing can be resolved or filtered
    };
    let Some(entity_name) = types.name(type_id) else {
        return;
    };
    let Some(defaults) = reg.defaults_for(entity_name) else {
        return; // no AttributeSupplier — holds no attributes at all
    };
    for snap in packet.snapshots {
        let Some(def) = reg.def(snap.attribute) else {
            continue; // byIdOrThrow would throw; drop the snapshot
        };
        if !defaults.iter().any(|(n, _)| *n == def.name) {
            continue; // "Entity {} does not have attribute {}"
        }
        entities.set_attribute(packet.entity_id, snap.attribute, snap.base, snap.modifiers);
    }
}

/// The narrowest clientbound-play dispatch seam for entity attributes: routes a
/// single `(packet id, body)` to [`apply_update_attributes`] iff `id` is the
/// resolved `update_attributes` id, returning whether it matched. Mirrors
/// [`route_damage_event`] so `play::PlaySession` and the `attributeshot` oracle
/// drive packet-id → attribute routing through the same production code.
#[allow(clippy::too_many_arguments)]
pub fn route_update_attributes(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
    types: Option<&rewo_data::entity_types::EntityTypes>,
    reg: Option<&rewo_data::attributes::AttributeRegistry>,
) -> bool {
    if id == ids.cb_play_update_attributes {
        apply_update_attributes(body, entities, classes, types, reg);
        true
    } else {
        false
    }
}

/// Apply one `ClientboundGameEventPacket` body to the weather state.
///
/// Body: an **unsigned byte** event id and an `f32` param — not a var-int pair.
/// The packet carries a dozen unrelated things (game-mode changes, the win
/// screen, demo hints); this entry point consumes only the four weather ids and
/// reports whether this one was among them. M71 handles the other ten — see
/// [`crate::game_event`] and [`play::PlaySession::game_state`].
///
/// It is a **thin view of [`game_event::apply`]**, the same function
/// `PlaySession` runs, rather than a second decode-and-route beside it. That
/// matters because `weathershot` is this entry point's only caller: a gate
/// driving a path the client itself no longer takes grades nothing, which is
/// the failure M45 recorded (`itemshot` calling `init_entities` directly and
/// so never installing the glint). Routing both through one function keeps the
/// gate pointed at the client's real behaviour. The discarded game state and
/// origin player are exactly the parts weather does not depend on.
///
/// A short body is inert: vanilla's reader would throw, and dropping the packet
/// is the closest safe equivalent to a client that never applied it. An
/// unregistered type id is likewise inert — that is vanilla's own behaviour,
/// not a tolerance added here.
pub fn apply_game_event(body: &[u8], weather: &mut rewo_world::weather::WeatherState) -> bool {
    // The bool has always meant "was it a weather event", not "did a level
    // move" — `RAIN_LEVEL_CHANGE` to the level already held is still weather.
    game_event::apply(
        body,
        weather,
        &mut game_event::ClientGameState::default(),
        &rewo_world::physics::PlayerState::at(0.0, 0.0, 0.0),
        // Weather has no ability consequences; the discarded abilities are the
        // same kind of "part weather does not depend on" as the state above.
        &mut rewo_world::abilities::Abilities::default(),
    )
    .was_weather()
}

/// The narrowest clientbound-play dispatch seam for the game event: routes a
/// single `(packet id, body)` to [`apply_game_event`] iff `id` is the resolved
/// `game_event` id, returning whether the id matched — **not** whether the
/// event was a weather one. A caller wanting the latter should use
/// [`apply_game_event`] directly, the way the `weathershot` oracle does.
///
/// Mirrors [`route_damage_event`] so `play::PlaySession` and the gate drive
/// packet-id → weather routing through the same production code.
pub fn route_game_event(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    weather: &mut rewo_world::weather::WeatherState,
) -> bool {
    if id == ids.cb_play_game_event {
        apply_game_event(body, weather);
        true
    } else {
        false
    }
}

/// Decode one `ItemStack.OPTIONAL_STREAM_CODEC` into the world's slot type.
///
/// Returns `None` for an empty stack and `Err` when the reader could not be
/// left aligned — see [`rewo_world::inventory`] on why a misaligned slot has to
/// abandon the whole packet rather than half-apply it.
///
/// The third element is M66's [`crate::item_stack::StackDetail`] — the
/// container contents and the raw patch ids, which neither of the world's slot
/// carriers can hold. Keyed by the same fingerprint as the text, so a caller
/// that wants it records both from one decode.
type SlotAndText = (
    Option<rewo_world::inventory::ItemSlot>,
    Option<(u64, rewo_world::inventory::SlotText)>,
    Option<(u64, crate::item_stack::StackDetail)>,
);

fn read_slot(
    r: &mut rewo_proto::reader::PacketReader,
    components: rewo_data::components::DataComponentIds,
) -> Result<SlotAndText, ()> {
    let slot = crate::item_stack::read_optional(r, components)?;
    if !slot.aligned() {
        return Err(());
    }
    Ok(match slot {
        crate::item_stack::WireSlot::Empty => (None, None, None),
        crate::item_stack::WireSlot::Stack(s) => {
            let c = &s.components;
            let text = rewo_world::inventory::SlotText {
                // `getItemName` is `getOrDefault(ITEM_NAME, item.getName())`
                // and `getHoverName` wraps that in `CUSTOM_NAME`, so the two
                // are a two-level override rather than alternatives.
                name: c.custom_name.clone().or_else(|| c.item_name.clone()),
                lore: c.lore.clone(),
                rarity: c.rarity,
                unbreakable: c.unbreakable,
                enchantments: c.enchantments.clone(),
                is_enchanted: c.is_enchanted,
            };
            (
                Some(rewo_world::inventory::ItemSlot {
                    item_id: s.item_id,
                    count: s.count,
                    has_components: s.patched,
                    components: c.fingerprint,
                    damage: c.damage,
                    max_damage: c.max_damage,
                    enchanted: c.has_foil(),
                    // M49: only the material — the icon's `select` is on it alone.
                    trim_material: c.trim.map(|(m, _)| m),
                }),
                Some((c.fingerprint, text)),
                Some((c.fingerprint, c.detail())),
            )
        }
    })
}

/// `ClientboundContainerSetContentPacket` (M34).
///
/// Body: `VarInt containerId`, `VarInt stateId`, a VarInt-counted list of
/// optional `ItemStack`s, then the carried stack.
///
/// Applies only to `containerId == 0`, the player's own `InventoryMenu`. Every
/// other id belongs to an open container screen, and Rewo has none — so
/// ignoring them is not a shortcut, it is the whole truth about what this
/// client can show.
pub fn apply_container_set_content(
    body: &[u8],
    components: rewo_data::components::DataComponentIds,
    inventory: &mut rewo_world::inventory::Inventory,
    mut details: Option<&mut crate::item_stack::StackDetails>,
) -> bool {
    let mut r = rewo_proto::reader::PacketReader::new(body);
    let (Ok(container), Ok(state_id), Ok(count)) = (r.varint(), r.varint(), r.varint()) else {
        return false;
    };
    if container != rewo_world::inventory::PLAYER_CONTAINER_ID {
        return false;
    }
    // A hostile or mismatched length is rejected before allocating for it.
    if !(0..=1024).contains(&count) {
        return false;
    }
    let mut slots = Vec::with_capacity(count as usize);
    for _ in 0..count {
        match read_slot(&mut r, components) {
            Ok(s) => slots.push(s),
            // Abandoned mid-list: everything after this point is garbage, so
            // the packet is dropped whole and the previous contents stand.
            Err(()) => return false,
        }
    }
    let Ok(carried) = read_slot(&mut r, components) else {
        return false;
    };
    // The tooltip text is recorded before the contents, so a slot is never
    // visible without the text its components imply.
    for (_, text, detail) in slots.iter().chain(std::iter::once(&carried)) {
        if let Some((fingerprint, text)) = text {
            inventory.record_text(*fingerprint, text.clone());
        }
        if let (Some(sink), Some((fingerprint, detail))) = (details.as_deref_mut(), detail) {
            sink.record(*fingerprint, detail.clone());
        }
    }
    let stacks: Vec<_> = slots.into_iter().map(|(s, _, _)| s).collect();
    inventory.set_content(state_id, &stacks, carried.0)
}

/// `ClientboundContainerSetSlotPacket` (M34).
///
/// Body: `VarInt containerId`, `VarInt stateId`, **`i16` slot**, optional
/// `ItemStack`. The slot is a short, not a var-int — the one field here that a
/// glance at the neighbouring packets would get wrong.
pub fn apply_container_set_slot(
    body: &[u8],
    components: rewo_data::components::DataComponentIds,
    inventory: &mut rewo_world::inventory::Inventory,
    details: Option<&mut crate::item_stack::StackDetails>,
) -> bool {
    let mut r = rewo_proto::reader::PacketReader::new(body);
    let (Ok(container), Ok(state_id), Ok(slot)) = (r.varint(), r.varint(), r.i16()) else {
        return false;
    };
    if container != rewo_world::inventory::PLAYER_CONTAINER_ID {
        return false;
    }
    let Ok((item, text, detail)) = read_slot(&mut r, components) else {
        return false;
    };
    if let Some((fingerprint, text)) = text {
        inventory.record_text(fingerprint, text);
    }
    if let (Some(sink), Some((fingerprint, detail))) = (details, detail) {
        sink.record(fingerprint, detail);
    }
    inventory.set_slot(state_id, slot as i32, item)
}

/// `ClientboundSetPlayerInventoryPacket` (M69) — the authoritative write
/// `container_set_slot` is not.
///
/// Body: `VarInt slot`, optional `ItemStack`. **Two differences from its
/// sibling, both of which a copy of `apply_container_set_slot` would get
/// wrong:**
///
/// 1. **The slot is a `VarInt`, not an `i16`.** `container_set_slot` writes
///    its index as a short among var-ints (M34's recorded trap); this one is
///    `ByteBufCodecs.VAR_INT` in the record's `STREAM_CODEC`. Reading a short
///    here consumes two bytes of a one-byte field and every stack after it is
///    garbage — and for slot values under 128 the *first* byte alone is a
///    plausible-looking index, so the failure starts one field late.
/// 2. **There is no container id and no state id.** `handleSetPlayerInventory`
///    is `player.getInventory().setItem(slot, contents)` and nothing else — it
///    does not go through the menu, so there is no state to advance and no
///    container to filter on. It always applies to the player.
///
/// And the slot is an **inventory index**, which is the whole reason this is a
/// separate function rather than a parameter: the conversion lives in
/// `rewo_world::inventory::menu_slot_of_inventory_index`.
pub fn apply_set_player_inventory(
    body: &[u8],
    components: rewo_data::components::DataComponentIds,
    inventory: &mut rewo_world::inventory::Inventory,
    details: Option<&mut crate::item_stack::StackDetails>,
) -> rewo_world::inventory::IndexWrite {
    use rewo_world::inventory::IndexWrite;
    let mut r = rewo_proto::reader::PacketReader::new(body);
    let Ok(index) = r.varint() else {
        return IndexWrite::OutOfRange;
    };
    // The stack is read before the index is judged, deliberately: a body whose
    // index is out of range is still a *well-formed* body, and its tooltip
    // text is worth recording. Judging first would also mean the two failure
    // modes ("bad index" and "bad stack") could not be told apart.
    let Ok((item, text, detail)) = read_slot(&mut r, components) else {
        return IndexWrite::OutOfRange;
    };
    if let Some((fingerprint, text)) = text {
        inventory.record_text(fingerprint, text);
    }
    if let (Some(sink), Some((fingerprint, detail))) = (details, detail) {
        sink.record(fingerprint, detail);
    }
    inventory.set_inventory_index(index, item)
}

/// `ClientboundSetCursorItemPacket` (M69) — one optional `ItemStack` and
/// nothing else.
///
/// The shortest body in the inventory family: no container id, no state id, no
/// slot. `handleSetCursorItem` is `containerMenu.setCarried(contents)`, guarded
/// only against the creative inventory screen — which Rewo has no notion of, so
/// the guard has nothing to express here.
///
/// This is what M35's predicted cursor has been missing. Its only correction
/// path was a whole `container_set_content` triggered by a state-id mismatch;
/// this is the server fixing one value without one.
pub fn apply_set_cursor_item(
    body: &[u8],
    components: rewo_data::components::DataComponentIds,
    inventory: &mut rewo_world::inventory::Inventory,
    details: Option<&mut crate::item_stack::StackDetails>,
) -> bool {
    let mut r = rewo_proto::reader::PacketReader::new(body);
    let Ok((item, text, detail)) = read_slot(&mut r, components) else {
        return false;
    };
    if let Some((fingerprint, text)) = text {
        inventory.record_text(fingerprint, text);
    }
    if let (Some(sink), Some((fingerprint, detail))) = (details, detail) {
        sink.record(fingerprint, detail);
    }
    inventory.set_carried(item);
    true
}

/// `ClientboundSetHeldSlotPacket` (M34) — one VarInt.
///
/// An out-of-range slot is **ignored**, matching
/// `handleSetHeldSlot`'s `if (Inventory.isHotbarSlot(...))`: it does not clamp
/// and it does not reset.
pub fn apply_set_held_slot(
    body: &[u8],
    inventory: &mut rewo_world::inventory::Inventory,
) -> bool {
    let mut r = rewo_proto::reader::PacketReader::new(body);
    let Ok(slot) = r.varint() else {
        return false;
    };
    inventory.set_selected(slot)
}

/// The narrowest clientbound-play dispatch seam for the three inventory
/// packets: routes a single `(packet id, body)` to whichever applier owns it,
/// returning whether the id matched — **not** whether the update was applied.
///
/// Mirrors [`route_game_event`] so `play::PlaySession` and the `inventoryshot`
/// oracle drive packet-id → inventory routing through the same production code.
pub fn route_inventory(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    // `None` before the registry has resolved the data-component ids. The two
    // container packets carry `ItemStack`s whose patches cannot be walked
    // without them, and inventing ids would misparse rather than fail — so
    // those two are matched and dropped. `set_held_slot` carries no stack and
    // is applied regardless.
    components: Option<rewo_data::components::DataComponentIds>,
    inventory: &mut rewo_world::inventory::Inventory,
    // M66's third slot carrier. `None` for a caller that does not draw
    // tooltips — the decode is unchanged either way, so passing it or not
    // cannot move a byte.
    details: Option<&mut crate::item_stack::StackDetails>,
) -> bool {
    if id == ids.cb_play_container_set_content {
        if let Some(c) = components {
            apply_container_set_content(body, c, inventory, details);
        }
        return true;
    }
    if id == ids.cb_play_container_set_slot {
        if let Some(c) = components {
            apply_container_set_slot(body, c, inventory, details);
        }
        return true;
    }
    if id == ids.cb_play_set_held_slot {
        apply_set_held_slot(body, inventory);
        return true;
    }
    // M69 — the two authoritative writes. Both carry an `ItemStack`, so both
    // are matched and dropped without the component ids, exactly as the two
    // container packets above are: inventing ids would misparse the patch
    // rather than fail, which for the cursor would put a confidently wrong
    // stack on the pointer.
    if id == ids.cb_play_set_player_inventory {
        if let Some(c) = components {
            apply_set_player_inventory(body, c, inventory, details);
        }
        return true;
    }
    if id == ids.cb_play_set_cursor_item {
        if let Some(c) = components {
            apply_set_cursor_item(body, c, inventory, details);
        }
        return true;
    }
    false
}

/// The `update_tags` dispatch seam (M69) — the play-state half.
///
/// Mirrors [`route_inventory`] so `play::PlaySession` and any oracle drive the
/// same production code. The **configuration**-state copy of this packet is
/// dispatched by `NetSession::run_configuration` against
/// `ids.cb_config_update_tags`; both call [`apply_update_tags`], which is why
/// there is one walk rather than two.
///
/// Returns whether the id matched — **not** whether the update applied. A body
/// that fails to decode is logged and dropped whole; see
/// [`crate::tags::read_update_tags`] for why a partial apply would be worse
/// than none.
pub fn route_tags(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    overrides: &mut crate::tags::TagOverrides,
) -> bool {
    if id == ids.cb_play_update_tags {
        apply_update_tags(body, overrides);
        return true;
    }
    false
}

/// Decode one `update_tags` body and apply it. Shared by both states.
///
/// Returns whether it applied.
pub fn apply_update_tags(body: &[u8], overrides: &mut crate::tags::TagOverrides) -> bool {
    match crate::tags::read_update_tags(body) {
        Ok(update) => {
            log::info!(
                "net: update_tags — {} registr(ies), {} tag(s)",
                update.registries.len(),
                update.tag_count()
            );
            overrides.apply(&update);
            true
        }
        Err(e) => {
            log::debug!("net: update_tags decode: {e}");
            false
        }
    }
}

/// The kind information metadata routing needs, because several slots are
/// polymorphic and only the entity type can separate them.
///
/// Index 16 BOOLEAN is `DATA_BABY_ID` (ageable/zombie), `DATA_DANCING` (Allay)
/// **or** `IS_CELEBRATING` (any Raider). Index 17 BYTE is
/// `DATA_SPELL_CASTING_ID` on a spellcaster illager but a gesture-state enum on
/// a sniffer/armadillo; index 17 BOOLEAN is `IS_CHARGING_CROSSBOW` on a
/// Pillager. Index 15 BYTE is `DATA_MOB_FLAGS_ID` on a `Mob` and unrelated
/// client flags on an `ArmorStand`. Index 16 BYTE is
/// `DATA_PLAYER_MODE_CUSTOMISATION` on a player (M60).
///
/// Every field is optional: absent means "this client could not resolve that
/// kind", and the corresponding slot is then left alone rather than guessed.
#[derive(Clone, Copy, Default)]
pub struct MetaKinds<'a> {
    /// `minecraft:allay` type id (M18).
    pub allay: Option<i32>,
    /// `minecraft:pillager` type id (M20).
    pub pillager: Option<i32>,
    /// `minecraft:sheep` type id (M52) — gates the index-18 BYTE.
    pub sheep: Option<i32>,
    /// `minecraft:creaking` type id (M52) — gates the index-17 BOOLEAN, which
    /// `Pillager` also claims.
    pub creaking: Option<i32>,
    /// `minecraft:player` type id (M60) — gates the index-16 BYTE, the
    /// skin-part customisation mask whose bit 0 shows the cape.
    pub player: Option<i32>,
    /// The six mobs whose texture is chosen by synched metadata (M64), in the
    /// order `[cat, wolf, frog, axolotl, horse, llama]`.
    ///
    /// Three of them (cat, wolf, frog) carry a `Holder` whose serializer is
    /// unique to them and would need no gate at all; the other three carry a
    /// plain `int` at an index other classes also claim, and those genuinely
    /// do. Both are gated, so the two read the same and a slot that moves
    /// fails loudly rather than half-silently.
    pub variant_kinds: VariantKinds,
    /// The machine-extracted ancestry sets — mob / raider / spellcaster.
    pub classes: Option<&'a rewo_data::entity_types::EntityClasses>,
    /// Data-component registry ids, needed to walk an ITEM_STACK metadata
    /// value (`ItemEntity.DATA_ITEM`, index 8 serializer 7). `None` leaves
    /// that serializer unwalkable, which ends the parse at that entry rather
    /// than desynchronising the rest.
    pub components: Option<rewo_data::components::DataComponentIds>,
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

/// The entity-type ids of the six mobs whose texture a metadata field selects
/// (M64). `None` for any the caller could not resolve, which leaves that mob
/// on its baked texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VariantKinds {
    pub cat: Option<i32>,
    pub wolf: Option<i32>,
    pub frog: Option<i32>,
    pub axolotl: Option<i32>,
    pub horse: Option<i32>,
    pub llama: Option<i32>,
    /// `minecraft:tropical_fish` (M68). Its index-17 INT is not a texture id
    /// at all — it is `TropicalFish.packVariant`'s four packed fields — but it
    /// arrives through the same setter, and the renderer unpacks it with the
    /// kind in hand, which is exactly how the other three ints are read.
    pub tropical_fish: Option<i32>,
}

impl VariantKinds {
    /// Whether this type is one of the two `TamableAnimal`s Rewo renders a
    /// variant for — the gate on the index-18 BYTE, which the sheep's wool
    /// byte shares slot *and* serializer with.
    pub fn is_tamable(&self, type_id: i32) -> bool {
        self.cat == Some(type_id) || self.wolf == Some(type_id)
    }
}

/// Decode a `set_passengers` body — `(vehicle, passengers)` (M70).
///
/// `ClientboundSetPassengersPacket`'s reader is two lines: `readVarInt()` then
/// `readVarIntArray()`, which is itself a var-int count followed by that many
/// var-ints. Standalone and pure so a test drives the real walk.
///
/// **An empty roster is meaningful and must not be confused with a decode
/// failure**: it is exactly how the server says everyone dismounted, and it is
/// the only thing that lets a vehicle's label come back.
pub fn parse_set_passengers(body: &[u8]) -> rewo_proto::Result<(i32, Vec<i32>)> {
    let mut r = PacketReader::new(body);
    let vehicle = r.varint()?;
    // A passenger id is at least one byte, which is what `count` bounds
    // against — a hostile length cannot make us reserve unbounded memory.
    let n = r.count("passengers", 1)?;
    let mut riders = Vec::with_capacity(n);
    for _ in 0..n {
        riders.push(r.varint()?);
    }
    Ok((vehicle, riders))
}

/// Apply a decoded `set_passengers` to the entity table.
///
/// Unlike `set_entity_data`, this is **not** gated on the vehicle being
/// tracked. `handleSetEntityPassengers` does log and return for an unknown
/// vehicle, but Rewo's table is also written by the chunk/entity stream in a
/// different order, and refusing here would make the rule depend on packet
/// arrival order. The roster is inert until something asks about that id, and
/// `remove` cleans both directions, so keeping it is safe and order-free.
pub(crate) fn apply_set_passengers(body: &[u8], entities: &mut rewo_world::entities::EntityTable) {
    match parse_set_passengers(body) {
        Ok((vehicle, riders)) => entities.set_passengers(vehicle, riders),
        Err(e) => log::debug!("play: set_passengers parse: {e}"),
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
    let meta = crate::metadata::parse(&mut r, kinds.components);
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
    // Slot 16 BYTE → `Avatar.DATA_PLAYER_MODE_CUSTOMISATION` (M60), the mask
    // whose bit 0 is `PlayerModelPart.CAPE`. A third reading of slot 16, and
    // the only one gated on a *single* type rather than an ancestry set:
    // `Avatar`'s only rendered subclass here is the player, and a wrong
    // reading would toggle some mob's cape off a byte that means something
    // else entirely. An unresolved player id leaves the slot alone.
    if let Some(b) = meta.byte16 {
        if kinds.player == Some(type_id) {
            entities.set_model_customisation(eid, b);
        }
    }
    // Slot 0 BYTE → `Entity.DATA_SHARED_FLAGS_ID` (M59). **No kind gate**, and
    // that is not an oversight: index 0 is `Entity`'s own first `defineId`, so
    // every entity that exists claims it with this serializer and nothing else
    // can. Parsed since M1 and discarded until now.
    if let Some(flags) = meta.flags {
        entities.set_shared_flags(eid, flags);
    }
    // Slot 3 BOOLEAN → `Entity.DATA_CUSTOM_NAME_VISIBLE` (M70). No kind gate,
    // for the same reason as slot 0: `Entity` owns 0..7, so nothing else can
    // claim it. `false` is applied as eagerly as `true` — the server toggles it
    // off as well as on, and treating this as a latch would leave a nametag up
    // after `/data merge` cleared the flag.
    if let Some(visible) = meta.custom_name_visible {
        entities.set_custom_name_visible(eid, visible);
    }
    // Slot 8 BYTE → `LivingEntity.DATA_LIVING_ENTITY_FLAGS` (M23 item use).
    // Gated on the type actually being a `LivingEntity`: `Entity` owns 0..7, so
    // slot 8 is the *first* slot any direct subclass may claim — an
    // `AbstractArrow` puts its own flags byte there with the same serializer,
    // and bit 1 there does not mean "using an item".
    if let Some(flags) = meta.living_flags {
        if kinds.classes.is_some_and(|c| c.is_living(type_id)) {
            entities.set_living_flags(eid, flags);
        }
    }
    // Slot 8 ITEM_STACK → `ItemEntity.DATA_ITEM` (M24b). No kind gate: the
    // serializer is unique to it among the classes that claim slot 8.
    if let Some(stack) = meta.item_stack {
        entities.set_item_stack(eid, stack);
    }
    // Slot 9 FLOAT → `LivingEntity.DATA_HEALTH_ID` (M24 death). Same living
    // gate as slot 8, for the same reason.
    if let Some(health) = meta.health {
        if kinds.classes.is_some_and(|c| c.is_living(type_id)) {
            entities.set_health(eid, health);
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
    // Slot 17 BOOLEAN → `Pillager.IS_CHARGING_CROSSBOW`, or `Creaking.IS_ACTIVE`
    // (M52). Two different classes, same index and same serializer — only the
    // kind separates them, the M18 rule again. (`AgeableMob.AGE_LOCKED` is a
    // third claimant at 17 BOOLEAN; it drives nothing the client renders, so it
    // simply falls through both gates.)
    if let Some(b) = meta.bool17 {
        if kinds.pillager == Some(type_id) {
            entities.set_charging_crossbow(eid, b);
        } else if kinds.creaking == Some(type_id) {
            entities.set_creaking_active(eid, b);
        }
    }
    // Slot 18 BYTE → `Sheep.DATA_WOOL_ID` (M52): low nibble the dye, 0x10 the
    // sheared flag. Kind-gated — `Creaking.IS_TEARING_DOWN` is also at 18, with
    // a different serializer, and nothing else the client models claims the
    // BYTE there.
    if let Some(wool) = meta.byte18 {
        if kinds.sheep == Some(type_id) {
            entities.set_wool(eid, wool);
        } else if kinds.variant_kinds.is_tamable(type_id) {
            // Slot 18 BYTE again, and this time it is
            // `TamableAnimal.DATA_FLAGS_ID` — bit 0x04 `isTame()`, which is
            // what `Wolf.getTexture` branches on. Same index, same
            // serializer, different class: `Sheep` and `TamableAnimal` both
            // extend `Animal`, whose own accessor count is zero, so both
            // land their first byte at 18 and only the kind separates them
            // (M18's rule). Reading a wolf's flags as a wool byte would give
            // it dye 4 — yellow — and a shorn fleece it does not have.
            entities.set_tamable_flags(eid, wool);
        }
    }
    // M64: the six metadata-driven texture variants. Three carry a `Holder`
    // whose serializer is theirs alone; three carry an `int` at an index
    // other classes claim. The value's *units* differ — a registry id for
    // the first three, an enum ordinal for the rest — and only the kind
    // says which, which is why they all funnel through one setter the
    // renderer reads back with the kind in hand.
    let vk = kinds.variant_kinds;
    for (value, kind) in [
        (meta.cat_variant, vk.cat),
        (meta.wolf_variant, vk.wolf),
        (meta.frog_variant, vk.frog),
        (meta.int18, vk.axolotl),
        (meta.int19, vk.horse),
        (meta.int21, vk.llama),
        // M68. Index 17 INT — a third serializer at an index the spellcaster
        // BYTE and the pillager/creaking BOOLEAN already claim, so the gate is
        // doing real work here even though the serializer differs.
        (meta.int17, vk.tropical_fish),
    ] {
        if let (Some(v), true) = (value, kind == Some(type_id)) {
            entities.set_variant(eid, v);
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

/// Decode + dispatch a `set_passengers` body onto the entity table (M70) — the
/// same seam `route_set_entity_data` gives the metadata path, so a gate can
/// drive the id match and the applier rather than reaching past them.
pub fn route_set_passengers(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
) -> bool {
    if id == ids.cb_play_set_passengers {
        apply_set_passengers(body, entities);
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
            _ => None, // armour handled below; body / saddle still discarded
        };
        // M46: the four armour slots. `EquipmentSlot`'s wire ids run
        // `2 feet, 3 legs, 4 chest, 5 head` — **bottom-up**, so the index into
        // a head-first array is `5 - id` and not `id - 2`.
        if (2..=5).contains(&(slot_id & 127)) {
            let index = (5 - (slot_id & 127)) as usize;
            let worn = match &slot {
                WireSlot::Empty => None,
                // M47: the dye travels with the piece. It is only ever in the
                // component patch, so this packet is the sole opportunity.
                WireSlot::Stack(s) => Some(rewo_world::entities::WornPiece {
                    item: s.item_id,
                    dye: s.components.dyed_color,
                    trim: s.components.trim,
                    // M50: the same `hasFoil` a held stack resolves, which is
                    // `ENCHANTMENT_GLINT_OVERRIDE` when present and
                    // `!enchantments.isEmpty()` otherwise.
                    foil: s.components.has_foil(),
                }),
            };
            entities.set_armor(eid, index, worn);
        }
        if let Some(hand) = hand {
            let item = match slot {
                WireSlot::Empty => HandItem::Empty,
                WireSlot::Stack(ref s) => match item_stack::resolve_swing(s, &data.prototypes) {
                    // A stack whose swing resolves has a walked patch and a
                    // registered item, which is exactly what `resolve_use`
                    // needs too — so the `None` arm below is unreachable in
                    // practice and is still written as a suppression rather
                    // than a default, because "unreachable" is not "impossible".
                    SwingResolution::Exact(swing) => {
                        match item_stack::resolve_use(&s, &data.use_profiles) {
                            Some(use_profile) => HandItem::Held(HeldItem {
                                item_id: s.item_id,
                                swing,
                                use_profile,
                                charged: s.charged.is_charged(),
                                glint: s.components.has_foil(),
                            }),
                            None => HandItem::Unknown,
                        }
                    }
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

/// Decode a `level_particles` body (M37).
///
/// Wire order, from `ClientboundLevelParticlesPacket`'s reading constructor:
/// `bool overrideLimiter`, `bool alwaysShow`, `f64 x/y/z`, `f32
/// xDist/yDist/zDist`, `f32 maxSpeed`, **`i32 count` — a plain big-endian
/// int, not a VarInt** — then `ParticleTypes.STREAM_CODEC`, which is a VarInt
/// registry id followed by that type's own options.
///
/// Returns `None` for a particle type this milestone does not simulate.
/// That is safe rather than a desync risk: packets are length-framed, so
/// abandoning a body part-way never disturbs the stream — which matters,
/// because most of the 125 registered types carry option payloads of shapes
/// we cannot skip without transcribing their codecs too.
pub fn route_level_particles(
    body: &[u8],
    types: &rewo_data::particle_types::ParticleTypes,
) -> Option<rewo_world::particles::ParticleEvent> {
    use rewo_world::particles::{ParticleCommand, ParticleEvent, ParticleKind};
    let mut r = PacketReader::new(body);
    let override_limiter = r.bool().ok()?;
    let always_show = r.bool().ok()?;
    let x = r.f64().ok()?;
    let y = r.f64().ok()?;
    let z = r.f64().ok()?;
    let x_dist = r.f32().ok()?;
    let y_dist = r.f32().ok()?;
    let z_dist = r.f32().ok()?;
    let max_speed = r.f32().ok()?;
    let count = r.i32().ok()?;
    let type_id = r.varint().ok()?;
    let kind = ParticleKind::from_registry_name(types.name(type_id)?)?;
    // `BlockParticleOption` appends the block state as a VarInt; every other
    // kind here is a `SimpleParticleType` with an empty options body.
    let block_state = if Some(type_id) == types.block_id {
        r.varint().ok()?.max(0) as u32
    } else {
        0
    };
    Some(ParticleEvent::Command(ParticleCommand {
        kind,
        x,
        y,
        z,
        x_dist,
        y_dist,
        z_dist,
        max_speed,
        count,
        override_limiter,
        always_show,
        block_state,
    }))
}

/// Decode a `level_event` body (M37).
///
/// Wire order: `i32 type`, `BlockPos pos`, `i32 data`, `bool globalEvent`.
/// Only 2001 (`PARTICLES_DESTROY_BLOCK`) is a particle effect this milestone
/// handles; its `data` is the broken block's state id. The rest of the
/// `LevelEvent` table is sound and non-particle effects, which are out of a
/// renderer's scope.
pub fn route_level_event(body: &[u8]) -> Option<rewo_world::particles::ParticleEvent> {
    use rewo_world::particles::{ParticleEvent, LEVEL_EVENT_DESTROY_BLOCK};
    let mut r = PacketReader::new(body);
    let kind = r.i32().ok()?;
    let (x, y, z) = r.position().ok()?;
    let data = r.i32().ok()?;
    let _global = r.bool().ok()?;
    if kind != LEVEL_EVENT_DESTROY_BLOCK {
        return None;
    }
    Some(ParticleEvent::DestroyBlock {
        x,
        y,
        z,
        block_state: data.max(0) as u32,
    })
}

/// Which of the three sound packets a body is, for [`route_sound`].
///
/// The dispatcher already knows the packet id; passing the kind in rather
/// than re-deriving it keeps the id table the single place a name maps to a
/// meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundPacketKind {
    /// `ClientboundSoundPacket`.
    Positioned,
    /// `ClientboundSoundEntityPacket`.
    OnEntity,
    /// `ClientboundStopSoundPacket`.
    Stop,
}

/// Decode a sound packet body into a [`sounds::SoundEvent`] (M63).
///
/// Returns `None` on a malformed body. That is safe rather than a desync
/// risk for the same reason `route_level_particles` gives: packets are
/// length-framed, so abandoning a body part-way never disturbs the stream.
/// Unlike the particle path there is no "kind we do not simulate" case —
/// every sound packet decodes in full, because none of the three has an
/// open-ended payload.
pub fn route_sound(kind: SoundPacketKind, body: &[u8]) -> Option<sounds::SoundEvent> {
    use sounds::SoundEvent;
    let mut r = PacketReader::new(body);
    match kind {
        SoundPacketKind::Positioned => sounds::PositionedSound::read(&mut r).ok().map(SoundEvent::At),
        SoundPacketKind::OnEntity => sounds::EntitySound::read(&mut r).ok().map(SoundEvent::OnEntity),
        SoundPacketKind::Stop => sounds::StopSound::read(&mut r).ok().map(SoundEvent::Stop),
    }
}

/// The narrowest clientbound-play dispatch seam for the view area (M67):
/// routes a `(packet id, body)` to [`view_area::apply`] iff `id` is one of the
/// three resolved view-area ids, returning whether the id matched — **not**
/// whether the body decoded.
///
/// The three ids map to three [`view_area::ViewAreaPacket`] kinds here and
/// nowhere else. That matters because `set_chunk_cache_radius` and
/// `set_simulation_distance` have *byte-identical* bodies — one VarInt each —
/// and name different quantities, so the id is the only discriminator that
/// exists. Deriving the kind from the body is not merely unreliable, it is
/// impossible.
pub fn route_view_area(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    area: &mut view_area::ViewArea,
) -> bool {
    let table = view_area::ViewAreaIds {
        center: ids.cb_play_set_chunk_cache_center,
        chunk_radius: ids.cb_play_set_chunk_cache_radius,
        simulation_distance: ids.cb_play_set_simulation_distance,
    };
    let Some(kind) = view_area::kind_for_id(id, table) else {
        return false;
    };
    view_area::apply(kind, body, area);
    true
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
        apply_entity_event(&body(entity, event), t, Some(WARDEN), Some(ARMADILLO), tick, None);
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
        apply_entity_event(&[0, 0, 0], &mut t, Some(WARDEN), Some(ARMADILLO), 10, None);
        // Full id but no event byte.
        apply_entity_event(&1i32.to_be_bytes(), &mut t, Some(WARDEN), Some(ARMADILLO), 10, None);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), None);
    }

    #[test]
    fn trailing_bytes_after_the_event_are_ignored() {
        // Frame length delimits the packet; extra bytes are not an error to
        // reject (same convention as every other handler here).
        let mut t = table();
        let mut b = body(1, 4);
        b.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        apply_entity_event(&b, &mut t, Some(WARDEN), Some(ARMADILLO), 33, None);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(33));
    }

    #[test]
    fn no_type_ids_configured_means_no_interpretation() {
        // The headless protocol harnesses leave the kind ids unset.
        let mut t = table();
        apply_entity_event(&body(1, 4), &mut t, None, None, 10, None);
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
            max_damage: 4,
            stored_enchantments: 12,
            enchantment_glint_override: 13,
            dyed_color: 14,
            trim: 15,
            rarity: 5,
            unbreakable: 6,
            custom_name: 8,
            item_name: 9,
            lore: 10,
            enchantments: 11,
            swing_animation: 40,
            damage: 3,
            charged_projectiles: 7,
            bundle_contents: 16,
            container: 17,
        };
        let data = super::item_stack::SwingWireData {
            prototypes: unreachable_prototypes(),
            components: comps,
            use_profiles: unreachable_use_profiles(),
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

    fn unreachable_use_profiles() -> rewo_data::use_item::UseProfiles {
        let paths = rewo_data::DataPaths::for_version("26.2").expect("config dir");
        let items = rewo_data::items::Items::load(&paths.registries_json())
            .expect("registries.json for the equipment gate test");
        rewo_data::use_item::UseProfiles::resolve(&items).expect("use profiles")
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

/// One `HashedStack`'s bytes, for the M35 gate.
///
/// The production writer, called through a `PacketWriter` whose header is
/// discarded — so the gate grades the encoder rather than a copy of it.
pub fn hashed_stack_bytes(slot: Option<rewo_world::inventory::ItemSlot>) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::packet(0);
    // `packet(0)` wrote a one-byte id; the stack's bytes are what follows.
    let before = w.buf.len();
    crate::play::write_hashed_stack(&mut w, slot);
    w.buf[before..].to_vec()
}

#[cfg(test)]
mod passenger_tests {
    //! `set_passengers` (M70) — the decode and the riding graph. Every body is
    //! built by hand and pushed through the real `parse_set_passengers` /
    //! `apply_set_passengers`, so the walk and the state machine are what is
    //! under test rather than a local copy.

    use super::{apply_set_passengers, parse_set_passengers};
    use rewo_world::entities::{EntityState, EntityTable};

    /// A `ClientboundSetPassengersPacket` body: VarInt vehicle then a VarInt
    /// array (count, then that many VarInts). Ids are kept < 128 so each is a
    /// single byte.
    fn body(vehicle: u8, riders: &[u8]) -> Vec<u8> {
        let mut b = vec![vehicle, riders.len() as u8];
        b.extend_from_slice(riders);
        b
    }

    fn table() -> EntityTable {
        let mut t = EntityTable::default();
        for id in 1..=4 {
            t.add(id, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        }
        t
    }

    #[test]
    fn the_body_is_a_vehicle_then_a_varint_array() {
        assert_eq!(
            parse_set_passengers(&body(7, &[8, 9])).unwrap(),
            (7, vec![8, 9])
        );
    }

    #[test]
    fn an_empty_roster_decodes_rather_than_erroring() {
        // The only way a server says "everyone dismounted". Reading it as a
        // truncation would leave the vehicle suppressed forever.
        assert_eq!(parse_set_passengers(&body(7, &[])).unwrap(), (7, vec![]));
    }

    #[test]
    fn a_truncated_array_is_an_error_rather_than_a_short_roster() {
        // Count says two, only one id follows.
        assert!(parse_set_passengers(&[7, 2, 8]).is_err());
    }

    #[test]
    fn is_vehicle_asks_whether_something_rides_this_entity() {
        let mut t = table();
        apply_set_passengers(&body(1, &[2]), &mut t);
        assert!(t.is_vehicle(1), "the horse is ridden");
        assert!(!t.is_vehicle(2), "the rider is not itself a vehicle");
        assert_eq!(t.vehicle_of(2), Some(1));
        assert_eq!(t.vehicle_of(1), None);
    }

    #[test]
    fn an_empty_roster_clears_the_vehicle() {
        let mut t = table();
        apply_set_passengers(&body(1, &[2]), &mut t);
        apply_set_passengers(&body(1, &[]), &mut t);
        assert!(!t.is_vehicle(1));
        assert_eq!(t.vehicle_of(2), None, "the dismounted rider is detached");
    }

    #[test]
    fn moving_a_rider_to_another_vehicle_frees_the_first() {
        // `startRiding` detaches from the previous vehicle. Without the
        // inverse index the old vehicle reads as ridden forever and silently
        // loses its label for the rest of the session.
        let mut t = table();
        apply_set_passengers(&body(1, &[3]), &mut t);
        apply_set_passengers(&body(2, &[3]), &mut t);
        assert!(!t.is_vehicle(1), "the abandoned vehicle is free again");
        assert!(t.is_vehicle(2));
        assert_eq!(t.vehicle_of(3), Some(2));
    }

    #[test]
    fn removing_a_rider_frees_its_vehicle() {
        // The despawn direction: nothing else will ever mention this rider.
        let mut t = table();
        apply_set_passengers(&body(1, &[2, 3]), &mut t);
        t.remove(2);
        assert!(t.is_vehicle(1), "3 is still aboard");
        t.remove(3);
        assert!(!t.is_vehicle(1), "the last rider left with its entity");
    }

    #[test]
    fn removing_a_vehicle_detaches_its_riders() {
        let mut t = table();
        apply_set_passengers(&body(1, &[2, 3]), &mut t);
        t.remove(1);
        assert_eq!(t.vehicle_of(2), None);
        assert_eq!(t.vehicle_of(3), None);
    }

    #[test]
    fn a_partial_roster_change_keeps_the_survivors_aboard() {
        let mut t = table();
        apply_set_passengers(&body(1, &[2, 3]), &mut t);
        apply_set_passengers(&body(1, &[3]), &mut t);
        assert_eq!(t.vehicle_of(2), None, "dropped from the roster");
        assert_eq!(t.vehicle_of(3), Some(1), "still aboard");
        assert!(t.is_vehicle(1));
    }
}

#[cfg(test)]
mod custom_name_visible_tests {
    //! `Entity.DATA_CUSTOM_NAME_VISIBLE` — metadata index 3, BOOLEAN (M70).

    use super::apply_set_entity_data;
    use rewo_world::entities::{EntityState, EntityTable};

    fn body(eid: u8, index: u8, serializer: u8, value: &[u8]) -> Vec<u8> {
        let mut b = vec![eid, index, serializer];
        b.extend_from_slice(value);
        b.push(0xFF);
        b
    }

    #[test]
    fn index_three_boolean_sets_and_clears_the_flag() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert!(!t.is_custom_name_visible(1), "the seeded default is false");

        apply_set_entity_data(&body(1, 3, 8, &[0x01]), &mut t, None);
        assert!(t.is_custom_name_visible(1));

        // Not a latch: the server toggles it off too, and a stuck `true` would
        // leave a nametag up after the flag was cleared.
        apply_set_entity_data(&body(1, 3, 8, &[0x00]), &mut t, None);
        assert!(!t.is_custom_name_visible(1));
    }

    #[test]
    fn the_flag_dies_with_the_entity() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&body(1, 3, 8, &[0x01]), &mut t, None);
        t.remove(1);
        assert!(
            !t.is_custom_name_visible(1),
            "a recycled id must not inherit it"
        );
    }

    #[test]
    fn a_later_field_still_parses_past_index_three() {
        // The skip table already handled serializer 8; this pins that *reading*
        // it consumes exactly one byte, so the pose that follows still lands.
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        let b = vec![1u8, 3, 8, 0x01, 6, 20, 5, 0xFF];
        apply_set_entity_data(&b, &mut t, None);
        assert!(t.is_custom_name_visible(1));
        assert_eq!(t.pose(1), 5, "a one-byte over-read would lose the pose");
    }
}

#[cfg(test)]
mod scoreboard_name_tests {
    #[test]
    fn a_uuid_renders_in_the_dashed_lowercase_form_entity_uses() {
        // `Entity.stringUUID` is `UUID.toString()`, and it is the scoreboard
        // key for every non-player entity.
        let uuid = 0x069a79f4_44e9_4726_a5be_fca90e38aaf5u128;
        assert_eq!(
            crate::play::uuid_to_dashed(uuid),
            "069a79f4-44e9-4726-a5be-fca90e38aaf5"
        );
    }

    #[test]
    fn leading_zeroes_are_preserved() {
        assert_eq!(
            crate::play::uuid_to_dashed(1),
            "00000000-0000-0000-0000-000000000001"
        );
    }
}
