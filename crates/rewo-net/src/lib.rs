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

pub mod music;
pub mod options;
pub mod abilities;
pub mod ambient_handlers;
pub mod attributes;
pub mod biome_parse;
pub mod border;
pub mod boss_bar;
pub mod bundle;
pub mod arg_types;
pub mod block_item;
pub mod chat_sign;
// M126a — `chat_style` and `chat_translate` moved DOWN to `rewo-world`, because
// `rewo_world::chat` has to name `ChatSpan` and the dependency runs net -> world.
// Re-exported under their old paths so no call site moved with them.
pub use rewo_world::{chat_style, chat_translate};
pub mod chat_type_parse;
pub mod chat_wire;
pub mod command_errors;
pub mod command_format;
pub mod commands;
pub mod dispatcher;
pub mod chunk_batch;
pub mod client_state;
pub mod config_tasks;
pub mod component_wire;
pub mod crypt;
pub mod dimension_parse;
pub mod enchantment_parse;
pub mod trim_parse;
pub mod variant_parse;
pub mod effects;
pub mod game_event;
pub mod hud_state;
pub mod ids;
pub mod item_stack;
pub mod merchant;
pub mod recipe_book;
pub mod menu;
pub mod jump_riding;
pub mod local_player_data;
pub mod metadata;
pub mod motion;
pub mod particle_options;
pub mod play;
pub mod player_rotation;
pub mod record;
pub mod scoreboard;
pub mod server_links;
pub mod session;
pub mod selector;
pub mod sidebar;
pub mod skins;
pub mod slot_ranges;
pub mod snbt;
pub mod snbt_grammar;
pub mod sound_engine;
pub mod sound_instance;
pub mod sounds;
pub mod suggestion_wire;
pub mod spawn_info;
pub mod tab_list_text;
pub mod tags;
pub mod tickable;
pub mod teams;
pub mod ticking;
pub mod view_area;
pub mod waypoints;

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

/// `ClientboundLoginPacket`'s `(hardcore, showDeathScreen)` (M82).
///
/// Both were `r.bool()?;` discards in [`spawn_info::read_login_prefix`] before
/// M82, and both are the *join-time* half of something the client already
/// modelled: `hardcore` chooses the death screen's title and its first
/// button's label, and `showDeathScreen` is the initial value of the flag
/// `game_event` id 11 amends mid-session.
///
/// Reads through the same walk `PlaySession` does, so a gate cannot grade a
/// second, drifting copy — the mistake M62 records finding in the tab list's
/// entry walk.
pub fn login_flags(packet: &[u8]) -> Option<(bool, bool)> {
    let mut r = PacketReader::new(packet);
    let p = spawn_info::read_login_prefix(&mut r).ok()?;
    Some((p.hardcore, p.show_death_screen))
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
    /// What the two blocking configuration tasks asked for and what Rewo
    /// answered (M166). Moved onto the `PlaySession` at `into_play`.
    config_tasks: config_tasks::ConfigTaskLog,
    /// The `minecraft:dimension_type` registry in raw wire order — index *is*
    /// the holder registry id. One vector of unified definitions, not the
    /// M14-era parallel `dim_shapes` / `dim_attrs` pair that a holder id could
    /// index inconsistently.
    dim_types: Vec<DimensionTypeDef>,
    /// Registry id of the `minecraft:overworld` world clock (see
    /// `parse_registry_data`); `None` on a server that syncs no clocks.
    overworld_clock_id: Option<i32>,
    /// The whole `minecraft:world_clock` registry **in raw wire order**, so
    /// the index *is* the holder id a `set_time` entry carries.
    ///
    /// M12 captured only the overworld's id, which is all the day/night cycle
    /// needs. M149c wants the rest because a dimension's `default_clock` names
    /// its clock by **identifier**, and the two registries arrive in the same
    /// `registry_data` batch with no ordering guarantee — so the name-to-id
    /// step has to be a lookup at use time (M62's lazy two-step) rather than a
    /// resolution at parse time.
    world_clock_ids: Vec<String>,
    /// Raw `minecraft:mob_effect` registry ids for `night_vision` / `darkness`,
    /// so the M13 lightmap can match the effect packets.
    ///
    /// **Resolved from the datagen report, not from `registry_data` (M92c).**
    /// They were read off the wire until M92c, inside a
    /// `registry == "minecraft:mob_effect"` branch that cannot fire:
    /// `registry_data` carries only `RegistryDataLoader.SYNCHRONIZED_REGISTRIES`
    /// and `Registries.MOB_EFFECT` is not one of them — it is a
    /// `BuiltInRegistries` entry the server never sends. So these stayed `None`
    /// for the whole session and night vision and darkness never engaged live,
    /// which no gate could see because `lightmapshot` is serverless and builds
    /// the effect state itself. The wire branch below is kept as an override
    /// for a server that does sync the registry; the report is the default.
    night_vision_id: Option<i32>,
    darkness_id: Option<i32>,
    /// Raw `minecraft:mob_effect` ids of the three effects that change a swing's
    /// duration (M19). Same source and same history as the two above — these
    /// were the other three ids M92c found unresolved.
    swing_effect_ids: SwingEffectIds,
    /// `minecraft:worldgen/biome` registry in raw wire order (M14 biome tint).
    biome_defs: Vec<rewo_world::biome::BiomeDef>,
    /// The `minecraft:enchantment` registry in wire order — the index is the
    /// protocol id a component patch carries (M42).
    enchantments: Vec<crate::enchantment_parse::EnchantmentDef>,
    /// The `minecraft:chat_type` registry in wire order — the index is the id
    /// a `ChatType.Bound`'s `holder` VarInt names, minus one (M127).
    chat_types: Vec<crate::chat_type_parse::ChatTypeDef>,
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
    /// The session facts that arrive in **configuration** and belong to the
    /// whole connection (M78): the server brand, and the cookie jar
    /// `cookie_request` answers from. Both are fields of vanilla's *common*
    /// listener, which is precisely the object that outlives the
    /// configuration → play switch, so this is moved into the play session by
    /// [`Connection::into_play`] exactly as `tags` is.
    session: crate::session::SessionState,
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
            config_tasks: config_tasks::ConfigTaskLog::default(),
            dim_types: Vec::new(),
            overworld_clock_id: None,
            world_clock_ids: Vec::new(),
            // M92c — from the report. `mob_effect` is a built-in registry, so
            // this is the authority and `registry_data` never carries it.
            night_vision_id: data.mob_effects.id_of("minecraft:night_vision"),
            darkness_id: data.mob_effects.id_of("minecraft:darkness"),
            swing_effect_ids: SwingEffectIds {
                haste: data.mob_effects.id_of("minecraft:haste"),
                conduit_power: data.mob_effects.id_of("minecraft:conduit_power"),
                mining_fatigue: data.mob_effects.id_of("minecraft:mining_fatigue"),
            },
            biome_defs: Vec::new(),
            enchantments: Vec::new(),
            chat_types: Vec::new(),
            trim_materials: Vec::new(),
            cat_variants: Vec::new(),
            wolf_variants: Vec::new(),
            frog_variants: Vec::new(),
            trim_patterns: Vec::new(),
            tags: crate::tags::TagOverrides::default(),
            session: crate::session::SessionState::default(),
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
                    // **This one is JSON, not NBT.**
                    // `ClientboundLoginDisconnectPacket.java:16-21` is
                    // `ByteBufCodecs.lenientJson(262144)`, so it reads as a
                    // string and prints verbatim: a whitelist kick logs
                    // `{"translate":"multiplayer.disconnect.not_whitelisted"}`.
                    //
                    // M163 left it: it needs a JSON-to-component reader Rewo has
                    // not got, `Connection` holds no language table, and this
                    // arm reaches a log line rather than a screen — `into_play`
                    // runs before a window exists. See the table on
                    // `component_wire::nbt_text`.
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
                x if x == self.ids.cb_config_code_of_conduct => {
                    // M166 — the FIRST of the two blocking tasks
                    // (`addOptionalTasks` appends it ahead of the resource
                    // pack), and the one whose name was already sitting in the
                    // comment on the ignore arm below. Until this reply exists
                    // the server's task queue never advances and
                    // `finish_configuration` never arrives.
                    match config_tasks::read_code_of_conduct(&self.packet[body..]) {
                        Ok(text) => {
                            log::info!(
                                "net: accepting the server's code of conduct ({} chars)",
                                text.chars().count()
                            );
                            self.config_tasks.codes_of_conduct.push(text);
                        }
                        Err(err) => {
                            // Answer anyway. The body is one string and the
                            // reply carries none of it, so a failed decode
                            // costs the log line above and nothing else —
                            // whereas going silent costs the whole connection.
                            log::warn!("net: code_of_conduct decode: {err} — accepting regardless");
                            self.config_tasks.codes_of_conduct.push(String::new());
                        }
                    }
                    let ack = config_tasks::write_code_of_conduct_accept(
                        self.ids.sb_config_accept_code_of_conduct,
                    );
                    self.send(ack)?;
                }
                x if x == self.ids.cb_config_resource_pack_push => {
                    // M166 — the second blocking task. See `config_tasks` for
                    // why the reply is FAILED_DOWNLOAD and not DECLINED.
                    // Disjoint-field borrow: the body is read out of
                    // `self.packet` while the log is written -- one function,
                    // two fields, no clone.
                    let (id, action) =
                        config_tasks::answer_pack_push(&self.packet[body..], &mut self.config_tasks);
                    self.send(config_tasks::write_pack_reply(
                        self.ids.sb_config_resource_pack,
                        id,
                        action,
                    ))?;
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
                x if x == self.ids.cb_config_custom_payload => {
                    // M78 — and this is the copy that actually fires: the
                    // vanilla server sends `minecraft:brand` from its
                    // configuration listener's opening burst and never sends
                    // another. `serverBrand` is a field of the *common*
                    // listener both states extend, so the two ids are one
                    // store; see `crate::session`.
                    crate::session::apply(
                        crate::session::SessionPacket::CustomPayload,
                        &self.packet[body..],
                        &mut self.session,
                    );
                }
                x if x == self.ids.cb_config_store_cookie => {
                    // M78 — the other `common` packet. A transfer-driven
                    // network sets cookies on whichever side of the state
                    // boundary it happens to be on, and the jar is one store.
                    crate::session::apply(
                        crate::session::SessionPacket::StoreCookie,
                        &self.packet[body..],
                        &mut self.session,
                    );
                }
                x if x == self.ids.cb_config_server_links => {
                    // M85 — the third `common` packet, and the state a vanilla
                    // server actually sends it in. `serverLinks` is a field of
                    // the same common listener the brand and the cookie jar
                    // are, so it crosses into play with them (`into_play`).
                    crate::session::apply(
                        crate::session::SessionPacket::ServerLinks,
                        &self.packet[body..],
                        &mut self.session,
                    );
                }
                x if x == self.ids.cb_config_disconnect => {
                    let mut r = PacketReader::new(&self.packet[body..]);
                    // On the LIVE path — `PlaySession::into_play` calls
                    // `run_configuration` — but `Connection` has no language
                    // table and `GameData` deliberately does not carry one, and
                    // this arm returns `Err(String)` to a log line rather than
                    // to a screen. M163 left it; see the table on
                    // `component_wire::nbt_text`.
                    let reason = r.nbt().map(|n| n.to_plain_text()).unwrap_or_default();
                    stats.disconnect_reason = Some(reason.clone());
                    return Err(format!("config disconnect: {reason}"));
                }
                // NOT update_tags -- that is handled ~57 lines above (M69), and NOT
                // either blocking task -- both are answered above (M166). What
                // is left is genuinely inert: enabled_features, reset_chat,
                // transfer, custom_report_details, the dialog pair, and
                // resource_pack_pop (deliberately unresolved -- see
                // `config_tasks`). None of them blocks the server's task queue.
                //
                // This comment named `code_of_conduct` and `update_tags` while
                // both of those hung or dropped real traffic, which is the
                // shape to watch for: an ignore arm that LISTS what it ignores
                // reads as deliberate whether or not anyone checked.
                _ => {}
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
        // M127: the chat-type registry, datapack-driven for the same reason —
        // the index is the id `ChatType.Bound`'s `holder` VarInt names.
        if registry == crate::chat_type_parse::CHAT_TYPE_REGISTRY {
            self.chat_types = crate::chat_type_parse::parse_chat_type_registry(&mut r, count);
            log::info!("net: {} chat type(s) synced", self.chat_types.len());
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
        if is_clock {
            self.world_clock_ids.clear();
        }
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
            if is_clock {
                if entry_name == "minecraft:overworld" {
                    self.overworld_clock_id = Some(idx as i32);
                }
                // Pushed in iteration order, so the position is the id. Never
                // sorted, and never derived from bootstrap order — M64's
                // alphabetisation trap.
                self.world_clock_ids.push(entry_name.clone());
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

    /// `handleRequestCookie` — `send(new ServerboundCookieResponsePacket(
    /// packet.key(), this.serverCookies.get(packet.key())))`.
    ///
    /// The reply is **whatever the jar holds**, and `Map.get` returning `null`
    /// is what makes it a `writeNullable` of nothing. Before M78 nothing ever
    /// called `store_cookie`, so this always wrote `false` and a
    /// transfer-driven network watched its session forget itself on every hop.
    /// The empty-jar path is unchanged; what changed is that the jar can now be
    /// non-empty.
    fn answer_cookie_request(&mut self, body: usize, resp_id: i32) -> Result<(), String> {
        let mut r = PacketReader::new(&self.packet[body..]);
        let key = r.identifier().unwrap_or_default();
        let payload = self.session.cookie(&key).map(<[u8]>::to_vec);
        let resp = crate::session::write_cookie_response(resp_id, &key, payload.as_deref());
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
                _ => {
                    // M78. The M1 soak/replay harness sees the same seven
                    // session packets the play session does, and routing them
                    // here keeps the brand and the cookie jar true on this path
                    // too. `route_session` returns `false` for every other id,
                    // which is what makes it safe as the fallthrough.
                    //
                    // The eighth, `bundle_delimiter`, is deliberately *not*
                    // handled here: this loop renders no frames, so
                    // reassembling a bundle would change nothing measurable.
                    // See [`bundle`].
                    route_session(id, &self.packet[body..], &self.ids, &mut self.session);
                }
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
/// Returns the `(entity id, type id)` it added — what vanilla's
/// `handleAddEntity` holds when it calls `postAddEntitySoundInstance` (M141f).
pub(crate) fn read_add_entity(
    r: &mut PacketReader,
    world: &mut World,
) -> rewo_proto::Result<(i32, i32)> {
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
    Ok((id, type_id))
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

/// `ClientboundHurtAnimationPacket` → `handleHurtAnimation` (M81).
///
/// ```text
/// VarInt id
/// f32    yaw      // writeFloat — a fixed big-endian 4 bytes, not a VarInt
/// ```
///
/// **Two fields, and one of them is dead for almost every entity.**
/// `handleHurtAnimation` calls `entity.animateHurt(yaw)`, whose base
/// `Entity` implementation is empty and whose `LivingEntity` override throws
/// the yaw away — only `Player` stores it. So this packet arms the same hurt
/// clock M21's `damage_event` does, and *additionally* steers the camera tilt,
/// for a player only.
///
/// In practice the local player is the only recipient that matters: the sole
/// send site is `ServerPlayer.indicateDamage`, which does
/// `this.connection.send(...)` — to the victim alone, never to trackers. The
/// entity id is therefore normally your own, and the yaw is already
/// **relative to your body yaw**, because the server computed
/// `atan2(zd, xd) * 180/π - getYRot()` before sending. The client subtracts
/// nothing further.
///
/// An untracked entity is inert (`if (entity != null)`), not an error.
///
/// # The local player is not in the table, and it is the only recipient
///
/// `getEntity(id)` on vanilla's `ClientLevel` resolves the **local player** for
/// its own id; Rewo's `EntityTable` holds only entities the server sent an
/// `add_entity` for, and it never sends one for you. Since the sole send site
/// is `ServerPlayer.indicateDamage` → `this.connection.send(...)`, the id on
/// this packet is *always* your own — so a table-only lookup drops every
/// packet this handler will ever see.
///
/// That is not hypothetical: it is what the first build did, and only a live
/// run caught it. The gate had constructed a table containing the id, which is
/// a world this packet never arrives in. `local_player` is the door the local
/// id comes through, exactly as M73's `local_attributes` is for the same
/// reason.
pub(crate) fn apply_hurt_animation(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
    player_type_id: Option<i32>,
    local_player: Option<i32>,
) {
    let mut r = PacketReader::new(body);
    let (Ok(eid), Ok(yaw)) = (r.varint(), r.f32()) else {
        return;
    };
    let is_player = if Some(eid) == local_player {
        // `getEntity` finds you, and you are a `Player`, so `animateHurt`
        // stores the direction.
        true
    } else {
        let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
            return; // getEntity(id) == null
        };
        if !classes.is_some_and(|c| c.is_living(type_id)) {
            // `Entity.animateHurt` is an empty method: no clock, no direction.
            return;
        }
        Some(type_id) == player_type_id
    };
    entities.animate_hurt(eid, yaw, is_player);
    log::debug!("net: hurt_animation eid={eid} yaw={yaw} player={is_player}");
}

/// The dispatch seam for [`apply_hurt_animation`].
pub fn route_hurt_animation(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
    player_type_id: Option<i32>,
    local_player: Option<i32>,
) -> bool {
    if id == ids.cb_play_hurt_animation {
        apply_hurt_animation(body, entities, classes, player_type_id, local_player);
        true
    } else {
        false
    }
}

/// `ClientboundBlockDestructionPacket` → `handleBlockDestruction` (M81).
///
/// ```text
/// VarInt        breakerEntityId
/// BlockPos      pos              // one packed big-endian i64
/// unsigned byte progress
/// ```
///
/// **`readUnsignedByte`, so there is no `-1` on the wire.** The server's
/// "stop" is `(byte) -1`, which arrives as **255**, and what retires the
/// record is `ClientLevel.destroyBlockProgress`'s range test failing — see
/// [`rewo_world::destruction`], which owns every rule about what happens next.
/// The breaker id is *not* validated against the entity table: vanilla does
/// not look the entity up at all, and a crack from a breaker outside view
/// distance is a real thing a server can send.
pub(crate) fn apply_block_destruction(
    body: &[u8],
    destruction: &mut rewo_world::destruction::DestructionProgress,
    game_time: i64,
) {
    let mut r = PacketReader::new(body);
    let (Ok(id), Ok((x, y, z)), Ok(progress)) = (r.varint(), r.position(), r.u8()) else {
        return;
    };
    destruction.set(id, [x, y, z], progress as i32, game_time);
    log::debug!("net: block_destruction breaker={id} ({x},{y},{z}) stage={progress}");
}

/// The dispatch seam for [`apply_block_destruction`].
pub fn route_block_destruction(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    destruction: &mut rewo_world::destruction::DestructionProgress,
    game_time: i64,
) -> bool {
    if id == ids.cb_play_block_destruction {
        apply_block_destruction(body, destruction, game_time);
        true
    } else {
        false
    }
}

/// What a decoded `player_combat_kill` says (M82).
///
/// The message is kept as its raw NBT tag rather than flattened, so the
/// renderer can style it — a server's death message is routinely coloured, and
/// `chat_style::parse_component` is what turns it into spans.
#[derive(Clone, Debug, PartialEq)]
pub struct CombatKill {
    /// `packet.playerId()`.
    pub player_id: i32,
    /// `packet.message()` — `ComponentSerialization.TRUSTED_STREAM_CODEC`, one
    /// NBT tag.
    pub message: rewo_proto::nbt::Nbt,
}

/// `ClientboundPlayerCombatKillPacket` → `handlePlayerCombatKill` (M82).
///
/// ```text
/// VarInt     playerId
/// Component  message      // TRUSTED_STREAM_CODEC, one NBT tag
/// ```
///
/// **`local_player` is the whole handler, not a filter on it.** Vanilla is
///
/// ```java
/// Entity player = this.level.getEntity(packet.playerId());
/// if (player == this.minecraft.player) { … }
/// ```
///
/// and `ClientLevel`'s entity storage *contains* the local player, so the
/// obvious transcription — look the id up in [`rewo_world::entities::EntityTable`]
/// — resolves to `None` every single time, because the server sends no
/// `add_entity` for your own player and Rewo's table is built from those. That
/// is `REWO_PLAN.md` §0.0 gotcha 13, and this is its third instance (M73's
/// attributes, M81's hurt animation). The id this packet carries is *always*
/// your own: `ServerPlayer.die` sends it down `this.connection`.
///
/// Returns `None` for an id that is not the local player's, and for a body
/// that does not decode. A packet naming somebody else is not an error —
/// vanilla drops it silently, and so does this.
pub(crate) fn apply_player_combat_kill(body: &[u8], local_player: Option<i32>) -> Option<CombatKill> {
    let mut r = PacketReader::new(body);
    let player_id = r.varint().ok()?;
    let message = r.nbt().ok()?;
    if local_player != Some(player_id) {
        log::debug!("net: player_combat_kill for {player_id}, not the local player");
        return None;
    }
    log::debug!(
        "net: player_combat_kill eid={player_id} message={:?}",
        message.to_plain_text()
    );
    Some(CombatKill { player_id, message })
}

/// `ServerboundClientCommandPacket.Action`, in the enum's declaration order —
/// which **is** the wire value, because the codec is
/// `output.writeEnum(action)` and `writeEnum` writes `ordinal()` as a VarInt.
///
/// Three values, and only the first is what a respawn sends. Getting the
/// ordinal wrong asks the server for the *statistics screen*
/// (`REQUEST_STATS`), which is a well-formed packet that does not respawn you
/// and produces no error anywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum ClientCommand {
    PerformRespawn = 0,
    RequestStats = 1,
    RequestGameruleValues = 2,
}

/// The body of a `ServerboundClientCommandPacket` — one VarInt.
pub fn client_command_body(action: ClientCommand) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::default();
    w.varint(action as i32);
    w.buf
}

/// The body of a `ServerboundContainerButtonClickPacket` (M92f) — the open
/// menu's id and a button index, both var-ints, and **nothing else**.
///
/// Extracted so the shape is testable without a socket. The absence is the
/// part worth pinning: its sibling `container_click` carries a **state id**
/// and a slot map because it is a prediction the server grades, and this one
/// carries neither because it is not. Adding a state id here — the natural
/// instinct, since the two packets sit beside each other and both name a
/// container — desynchronises every field after it.
pub fn container_button_click_body(container_id: i32, button: i32) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::default();
    w.varint(container_id);
    w.varint(button);
    w.buf
}

/// `ServerboundRecipeBookChangeSettingsPacket` (M98).
///
/// ```java
/// output.writeEnum(this.bookType);
/// output.writeBoolean(this.isOpen);
/// output.writeBoolean(this.isFiltering);
/// ```
///
/// `writeEnum` is `writeVarInt(ordinal)`, so the book type is
/// `RecipeBookType`'s ordinal — crafting, furnace, blast furnace, smoker — the
/// **same positional order** `RecipeBookSettings` uses on the way in (M93y). One
/// order, both directions.
///
/// **It carries `isOpen` as well as `isFiltering`**, and `sendUpdateSettings`
/// reads both out of the book's settings rather than taking them as arguments —
/// so toggling the filter also re-reports the open state, and opening the book
/// also re-reports the filter. Sending only the field that changed would leave
/// the server's copy of the other one stale.
pub fn recipe_book_change_settings_body(book_type: i32, open: bool, filtering: bool) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::default();
    w.varint(book_type);
    w.u8(u8::from(open));
    w.u8(u8::from(filtering));
    w.buf
}

/// `ServerboundPlaceRecipePacket` (M98) — `(containerId, recipe, useMaxItems)`.
///
/// `useMaxItems` is **shift-held**, from `event.hasShiftDown()`: a plain click
/// lays out one, a shift-click as many as the inventory allows. It is a single
/// boolean byte after two var-ints.
pub fn place_recipe_body(container_id: i32, recipe: i32, use_max_items: bool) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::default();
    w.varint(container_id);
    w.varint(recipe);
    w.u8(u8::from(use_max_items));
    w.buf
}

/// `ServerboundContainerSlotStateChangedPacket` (M93h) — the crafter's toggle.
///
/// ```java
/// output.writeVarInt(this.slotId);
/// output.writeContainerId(this.containerId);   // VarInt.write
/// output.writeBoolean(this.newState);
/// ```
///
/// **The slot comes FIRST and the container second** — the opposite order from
/// `container_button_click`, whose body is `(containerId, button)`. Two
/// adjacent serverbound container packets with their two var-ints transposed:
/// writing this one container-first produces a well-formed packet that toggles
/// the wrong slot of the wrong menu, and nothing on the wire says so.
///
/// `newState` is **enabled**, matching `setSlotState(slotId, isEnabled)` — not
/// the stored data value, which is its inverse (`isEnabled ? 0 : 1`).
/// `ServerboundRenameItemPacket` (M93n) — one `writeUtf` string and nothing
/// else.
///
/// The **empty string is meaningful**, not an absence: it is the request to
/// clear the custom name, which `AnvilScreen.onNameChanged` produces whenever
/// you type an unnamed item's own display name. So this has no `Option` and
/// no "skip if empty" — a caller that suppressed the empty case would make an
/// anvil unable to un-name anything.
pub fn rename_item_body(name: &str) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::default();
    w.string(name);
    w.buf
}

/// `ServerboundSetBeaconPacket` (M93l) — two `Optional<Holder<MobEffect>>`.
///
/// ```java
/// STREAM_CODEC = composite(
///    MobEffect.STREAM_CODEC.apply(ByteBufCodecs::optional), ..primary,
///    MobEffect.STREAM_CODEC.apply(ByteBufCodecs::optional), ..secondary)
/// ```
///
/// `optional` writes a **bool**, then the value only if present. And
/// `MobEffect.STREAM_CODEC` is `ByteBufCodecs.holderRegistry(MOB_EFFECT)` —
/// a **RAW 0-based id**, not `holder`'s `id + 1`. That distinction has now
/// bitten in M16 (dimension types), M21 (damage types), M55 (attributes) and
/// M92d (the beacon's own decode), and it is quiet every time: an off-by-one
/// names a real effect, so the beacon simply grants the wrong one.
pub fn set_beacon_body(primary: Option<i32>, secondary: Option<i32>) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::default();
    for id in [primary, secondary] {
        match id {
            Some(i) => {
                w.u8(1);
                w.varint(i);
            }
            None => {
                w.u8(0);
            }
        }
    }
    w.buf
}

pub fn container_slot_state_changed_body(
    slot_id: i32,
    container_id: i32,
    new_state: bool,
) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::default();
    w.varint(slot_id);
    w.varint(container_id);
    w.u8(u8::from(new_state));
    w.buf
}

/// `ClientboundAwardStatsPacket` → `handleAwardStats` (M84).
///
/// ```java
/// STAT_VALUES_STREAM_CODEC = ByteBufCodecs.map(
///    Object2IntOpenHashMap::new, Stat.STREAM_CODEC, ByteBufCodecs.VAR_INT);
/// Stat.STREAM_CODEC = ByteBufCodecs.registry(Registries.STAT_TYPE)
///    .dispatch(Stat::getType, StatType::streamCodec);
/// ```
///
/// so the body is `VarInt count` then `count × (VarInt statType, VarInt value,
/// VarInt amount)`.
///
/// # The two-level dispatch is uniform, which is why this is total
///
/// A dispatched codec is normally the `DataComponentPatch` hazard — an
/// untranscribed variant cannot be skipped, because the reader parks mid-value.
/// Here every `StatType`'s second level is built by the same one-line
/// constructor, `ByteBufCodecs.registry(registry.key())`, so **all nine are a
/// single VarInt** and the first level selects only *which registry to resolve
/// it in*. This walk therefore stays in step for a stat type it has never heard
/// of, and the resolution is deferred to display time — see
/// `rewo_world::stats::StatKey`.
///
/// # It is addressed to you, and carries no id to prove it
///
/// `handleAwardStats` writes into `minecraft.player.getStats()` unconditionally.
/// There is nothing to look up in an entity table and nothing to compare
/// against the local player, which is the *good* shape of gotcha 13: the packet
/// that cannot be got wrong that way.
///
/// `ByteBufCodecs.map`'s default `maxSize` is `Integer.MAX_VALUE`, so vanilla
/// imposes no cap; a short read ends the walk and keeps what it had, because a
/// truncated statistics list is worth more than a dropped one.
pub fn apply_award_stats(body: &[u8]) -> Option<Vec<(rewo_world::stats::StatKey, i32)>> {
    let mut r = PacketReader::new(body);
    let count = r.varint().ok()?;
    if count < 0 {
        log::warn!("net: award_stats with a negative count {count}");
        return None;
    }
    let mut out = Vec::with_capacity((count as usize).min(4096));
    for _ in 0..count {
        let Ok(type_id) = r.varint() else { break };
        let Ok(value_id) = r.varint() else { break };
        let Ok(amount) = r.varint() else { break };
        out.push((rewo_world::stats::StatKey::new(type_id, value_id), amount));
    }
    if out.len() != count as usize {
        log::warn!(
            "net: award_stats truncated at {} of {count} entries",
            out.len()
        );
    }
    Some(out)
}

/// The dispatch seam for [`apply_award_stats`].
pub fn route_award_stats(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    out: &mut Option<Vec<(rewo_world::stats::StatKey, i32)>>,
) -> bool {
    if id == ids.cb_play_award_stats {
        if let Some(stats) = apply_award_stats(body) {
            log::debug!("net: award_stats — {} entries", stats.len());
            *out = Some(stats);
        }
        true
    } else {
        false
    }
}

/// What `handlePlayerCombatKill` does once the packet is decoded.
#[derive(Clone, Debug, PartialEq)]
pub enum DeathAction {
    /// Nothing: the packet named somebody else, or did not decode.
    None,
    /// `minecraft.gui.setScreen(new DeathScreen(...))`.
    ShowScreen(CombatKill),
    /// `minecraft.player.respawn()` — with the death screen suppressed the
    /// client never sees one, and **records nothing**, so no state downstream
    /// has to know the rule.
    RespawnNow,
}

/// `handlePlayerCombatKill`'s branch, as a function so a gate can drive the
/// production one:
///
/// ```java
/// if (this.minecraft.player.shouldShowDeathScreen()) {
///    this.minecraft.gui.setScreen(new DeathScreen(...));
/// } else {
///    this.minecraft.player.respawn();
/// }
/// ```
///
/// `shouldShowDeathScreen()` is the `doImmediateRespawn` gamerule inverted —
/// it arrives on the login packet and is amended by `game_event` id 11, whose
/// parameter is itself inverted (`param == 0` **shows** the screen).
pub fn death_action(kill: Option<CombatKill>, show_death_screen: bool) -> DeathAction {
    match (kill, show_death_screen) {
        (None, _) => DeathAction::None,
        (Some(k), true) => DeathAction::ShowScreen(k),
        (Some(_), false) => DeathAction::RespawnNow,
    }
}

/// The dispatch seam for [`apply_player_combat_kill`].
///
/// `handled` and `kill` are separate answers: a packet addressed to another
/// player is *handled* (the id matched, the body was consumed) and produces no
/// kill. Collapsing the two would let a stray combat-kill fall through to the
/// rest of the dispatch chain.
pub fn route_player_combat_kill(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    local_player: Option<i32>,
    out: &mut Option<CombatKill>,
) -> bool {
    if id == ids.cb_play_player_combat_kill {
        if let Some(kill) = apply_player_combat_kill(body, local_player) {
            *out = Some(kill);
        }
        true
    } else {
        false
    }
}

/// What [`apply_take_item_entity`] needs to resolve a collection.
#[derive(Clone, Copy, Debug, Default)]
pub struct TakeItemKinds {
    /// `minecraft:item`'s registry id — the only type whose stack is shrunk.
    pub item: Option<i32>,
    /// `minecraft:experience_orb`'s — the only type this handler does **not**
    /// remove.
    pub orb: Option<i32>,
    /// The local player's entity id, and its chest position, for vanilla's
    /// `if (to == null) to = this.minecraft.player` fallback. `None` before
    /// login.
    pub local_player: Option<(i32, [f64; 3])>,
}

/// `ClientboundTakeItemEntityPacket` → `handleTakeItemEntity` (M81).
///
/// ```text
/// VarInt itemId      // the entity being collected
/// VarInt playerId    // the collector
/// VarInt amount
/// ```
///
/// Four things here invert the obvious reading:
///
/// 1. **The client removes the entity itself.** This is not a notification
///    that a `remove_entities` is coming — `handleTakeItemEntity` calls
///    `this.level.removeEntity(itemId, DISCARDED)` directly, and for an
///    `ItemEntity` it does so only once the *client's own copy* of the stack
///    has been shrunk to empty. A partial pickup leaves the entity alive with
///    a smaller stack and no further packet.
/// 2. **An experience orb is never removed here.** The branch is
///    `if (from instanceof ItemEntity) … else if (!(from instanceof
///    ExperienceOrb)) removeEntity(…)`, so an orb gets the sound and the
///    animation and keeps existing until the server says otherwise.
/// 3. **An unknown collector falls back to the local player**, rather than
///    making the packet inert: `if (to == null) to = this.minecraft.player`.
///    Only the *collected* entity being unknown drops the whole thing.
/// 4. **The animation is added unconditionally**, for an arrow and an orb as
///    much as for an item. See [`rewo_world::pickup`] for why a record with
///    nothing to draw is kept rather than skipped.
///
/// `to` is cast to `LivingEntity` in vanilla, which would throw for a
/// non-living collector. Rewo has no cast: it takes the entity's position,
/// which is all the animation wants.
pub(crate) fn apply_take_item_entity(
    body: &[u8],
    world: &mut rewo_world::World,
    kinds: TakeItemKinds,
) {
    let mut r = PacketReader::new(body);
    let (Ok(item_id), Ok(player_id), Ok(amount)) = (r.varint(), r.varint(), r.varint()) else {
        return;
    };
    let Some(from) = world.entities.get(item_id) else {
        return; // `if (from != null)` — nothing else in the handler runs
    };
    let type_id = from.type_id;
    let source = from.render_pos(1.0);
    let stack = world.entities.item_stack(item_id);

    // `LivingEntity to = (LivingEntity) getEntity(playerId); if (to == null)
    // to = this.minecraft.player;` — the substitution is on the *entity*, so
    // the animation then follows the local player for the rest of its life,
    // not the id that failed to resolve. Doing it on the id here is what makes
    // the per-tick re-resolution below agree with it.
    let collector = if world.entities.get(player_id).is_some() {
        player_id
    } else {
        kinds.local_player.map(|(id, _)| id).unwrap_or(player_id)
    };
    let target = collector_chest(&world.entities, collector, kinds.local_player).unwrap_or(source);

    log::debug!(
        "net: take_item_entity item={item_id} collector={player_id}->{collector} \
         amount={amount} stack={stack:?}"
    );
    world
        .pickups
        .add(stack, source, collector, item_id, target);

    if Some(type_id) == kinds.item {
        // `itemStack.shrink(amount)` guarded by `if (!itemStack.isEmpty())`,
        // then `if (itemStack.isEmpty()) removeEntity(...)`. An entity that
        // never sent a stack is already empty and is removed outright.
        match stack {
            Some((item, count, foil)) if count - amount > 0 => {
                world
                    .entities
                    .set_item_stack(item_id, Some((item, count - amount, foil)));
            }
            _ => world.entities.remove(item_id),
        }
    } else if Some(type_id) != kinds.orb {
        world.entities.remove(item_id);
    }
}

/// `(getY() + getEyeY()) / 2 - getY()`, i.e. **half the eye height**, for a
/// collector Rewo tracks as a table entry.
///
/// `getEyeY()` is absolute (`position.y + eyeHeight`), so vanilla's midpoint is
/// the collector's chest. Rewo keeps no per-entity eye height, so this is the
/// standing-player 1.62 halved — exact for the overwhelmingly common case (a
/// player collecting) and an approximation of a few tenths of a block for a
/// mob that picks something up. Stated rather than hidden; the local player,
/// which is the collector for every pickup the viewer cares about, does not go
/// through here at all.
pub(crate) const CHEST_OFFSET: f64 = 1.62 / 2.0;

/// `ItemPickupParticle.updatePosition` — where the item is flying to.
///
/// One function so the target the animation *starts* with and the target it
/// re-reads every tick cannot drift apart: vanilla holds one `Entity`
/// reference and calls one method on it, and two call sites deriving the same
/// point separately is exactly how they stop agreeing.
pub(crate) fn collector_chest(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    local_player: Option<(i32, [f64; 3])>,
) -> Option<[f64; 3]> {
    if let Some(e) = entities.get(id) {
        let p = e.render_pos(1.0);
        return Some([p[0], p[1] + CHEST_OFFSET, p[2]]);
    }
    // The local player is never in the table; the app supplies its chest point
    // from the real eye height rather than the standing constant above.
    local_player.filter(|(pid, _)| *pid == id).map(|(_, p)| p)
}

/// The dispatch seam for [`apply_take_item_entity`].
pub fn route_take_item_entity(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    world: &mut rewo_world::World,
    kinds: TakeItemKinds,
) -> bool {
    if id == ids.cb_play_take_item_entity {
        apply_take_item_entity(body, world, kinds);
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
                // `getHoverName()` is `getCustomName() ?? getItemName()`, and
                // `getItemName()` is the item's virtual `getName(stack)` whose
                // base is `getOrDefault(ITEM_NAME, EMPTY)` — a two-level
                // override, resolved at *render* rather than merged here.
                //
                // They stay apart because two rules ask about the first alone:
                // `getStyledHoverName`'s italic and `isUsableForCrafting`. And
                // they stay as components because this decode has no language
                // table — see `rewo_world::chat_style::flatten`.
                custom_name: c.custom_name.clone(),
                item_name: c.item_name.clone(),
                lore: c.lore.clone(),
                rarity: c.rarity,
                unbreakable: c.unbreakable,
                enchantments: c.enchantments.clone(),
                is_enchanted: c.is_enchanted,
                cooldown_group: c.use_cooldown_group.clone(),
                book_pages: c.book_pages.clone(),
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
                    // M93e — the grindstone's predicate. NOT `enchanted`
                    // above: that is `hasFoil()`, which respects
                    // ENCHANTMENT_GLINT_OVERRIDE both ways and reads only
                    // `minecraft:enchantments`. `c.enchantments` is already
                    // the union of that and `stored_enchantments`, which is
                    // exactly `hasAnyEnchantments`.
                    any_enchantments: !c.enchantments.is_empty(),
                    unbreakable: c.unbreakable,
                    damage_component_removed: c.damage_component_removed,
                    // M93f — the cartography table's map slot.
                    has_map_id: c.has_map_id,
                    // M93g — the loom's two conjunctions.
                    dye_removed: c.dye_removed,
                    provides_banner_patterns_removed: c.provides_banner_patterns_removed,
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
    // The container id `inventory` *is* — 0 for the player's own menu, or an
    // open container's. M87 made this a parameter: it was hard-coded to 0,
    // which is why every container but the player's was dropped.
    expect_container: i32,
    inventory: &mut rewo_world::inventory::Inventory,
    mut details: Option<&mut crate::item_stack::StackDetails>,
) -> bool {
    let mut r = rewo_proto::reader::PacketReader::new(body);
    let (Ok(container), Ok(state_id), Ok(count)) = (r.varint(), r.varint(), r.varint()) else {
        return false;
    };
    if container != expect_container {
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
    // See `apply_container_set_content`.
    expect_container: i32,
    inventory: &mut rewo_world::inventory::Inventory,
    details: Option<&mut crate::item_stack::StackDetails>,
) -> bool {
    let mut r = rewo_proto::reader::PacketReader::new(body);
    let (Ok(container), Ok(state_id), Ok(slot)) = (r.varint(), r.varint(), r.i16()) else {
        return false;
    };
    if container != expect_container {
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
    // The open container menu (M87). `container_set_content` and
    // `container_set_slot` are ONE packet each that can address either menu —
    // `handleContainerContent` is `if id == 0 { inventoryMenu } else if id ==
    // containerMenu.containerId { containerMenu }` — so both targets have to
    // be reachable from one dispatcher. Splitting them across two routers
    // would put two seams on one packet id, and in an `else if` chain only the
    // first would ever run.
    menus: &mut rewo_world::menu::Menus,
    // M66's third slot carrier. `None` for a caller that does not draw
    // tooltips — the decode is unchanged either way, so passing it or not
    // cannot move a byte.
    details: Option<&mut crate::item_stack::StackDetails>,
) -> bool {
    if id == ids.cb_play_container_set_content {
        if let Some(c) = components {
            if let Some((target_id, target)) = container_target(body, inventory, menus) {
                apply_container_set_content(body, c, target_id, target, details);
            }
        }
        return true;
    }
    if id == ids.cb_play_container_set_slot {
        if let Some(c) = components {
            if let Some((target_id, target)) = container_target(body, inventory, menus) {
                apply_container_set_slot(body, c, target_id, target, details);
            }
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

/// Which menu a `container_set_content` / `container_set_slot` body is
/// addressed to, and that menu's id.
///
/// Transcribed from `handleContainerContent`, whose two arms are the whole
/// rule:
///
/// ```java
/// if (packet.containerId() == 0)                                   -> inventoryMenu
/// else if (packet.containerId() == player.containerMenu.containerId) -> containerMenu
/// ```
///
/// Three things follow that are easy to get wrong in the obvious direction:
///
/// * **Id 0 goes to the player's own menu whatever is open.** It is not "the
///   open menu, which defaults to the inventory" — `inventoryMenu` and
///   `containerMenu` are separate objects and the server addresses each
///   deliberately. Routing 0 to an open chest would write the chest's slots
///   with the player's items.
/// * **A non-zero id that does not match is dropped**, not applied to whatever
///   is open. A stale id arriving after a close must not land in the next
///   container.
/// * The id is only peeked here; the applier re-reads and re-checks it, so a
///   body whose id disagrees with the target it was routed to still declines.
pub(crate) fn container_target<'a>(
    body: &[u8],
    inventory: &'a mut rewo_world::inventory::Inventory,
    menus: &'a mut rewo_world::menu::Menus,
) -> Option<(i32, &'a mut rewo_world::inventory::Inventory)> {
    let container = rewo_proto::reader::PacketReader::new(body).varint().ok()?;
    if container == rewo_world::inventory::PLAYER_CONTAINER_ID {
        return Some((container, inventory));
    }
    menus.menu_for(container).map(|m| (container, m))
}

/// The container-menu dispatch seam (M87): `open_screen` and
/// `container_set_data`.
///
/// **`container_close` is deliberately not here.** M74 already dispatches it
/// through [`route_client_state`], and the play loop's router chain is a
/// sequence of `else if`s — so exactly one seam sees any given id. Claiming it
/// here would either run before `route_client_state` and silently stop M74's
/// close counter, or run after it and never fire at all. Neither failure
/// announces itself. The menu is closed from inside that existing arm in
/// [`play::PlaySession`] instead, so there is still one dispatcher per packet.
///
/// A sibling of [`route_inventory`] rather than an extension of it, because
/// the two write different objects — vanilla keeps `player.inventoryMenu`
/// (container id 0, permanent) and `player.containerMenu` (whatever is open)
/// side by side, and the server addresses each independently. Keeping the
/// seams apart is also what lets this land without touching
/// `route_inventory`'s signature, and therefore without disturbing the
/// `inventoryshot` witnesses that drive it.
///
/// Returns whether the id matched — **not** whether anything was applied. All
/// three have their own reasons to decline: an unknown menu type opens
/// nothing, data for a non-matching container id is dropped, and a malformed
/// body is dropped whole.
pub fn route_menu(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    menus: &mut rewo_world::menu::Menus,
) -> bool {
    crate::menu::route(
        crate::menu::MenuIds {
            open_screen: ids.cb_play_open_screen,
            container_set_data: ids.cb_play_container_set_data,
        },
        id,
        body,
        menus,
    )
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
    /// `minecraft:bee` type id (M141f) — gates the index-19 LONG,
    /// `Bee.DATA_ANGER_END_TIME`.
    pub bee: Option<i32>,
    /// `minecraft:guardian` and `minecraft:elder_guardian` (M141g) — they gate
    /// the index-17 INT, `Guardian.DATA_ID_ATTACK_TARGET`.
    ///
    /// **Two ids rather than one**, because the elder is a separate registry
    /// entry with the same accessor and a different `getAttackDuration()` (60
    /// against 80). A gate naming only the base would leave every elder
    /// guardian's beam silent.
    pub guardian: Option<i32>,
    pub elder_guardian: Option<i32>,
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
    /// The language table, for `Entity.DATA_CUSTOM_NAME` (index 2).
    ///
    /// **The nametag resolves at DECODE, not at render**, and that is a
    /// performance decision rather than a convenience one:
    /// `rewo_gpu::entities::EntityDraw::name` is `Option<&'a str>` borrowed
    /// straight out of `EntityTable::custom_names`, so carrying the component
    /// forward would mean a parse and a `String` per named entity per frame at
    /// the 500 fps target. It is also what vanilla effectively does —
    /// `TranslatableContents.decompose` reaches the `Language.getInstance()`
    /// global — and [`crate::play::PlaySession::lang`]'s own doc records why
    /// receipt-time resolution is equivalent for a client that cannot change
    /// language mid-session.
    ///
    /// `None` leaves a `translate` component as its key, which is what this
    /// decode did before the table was threaded here.
    ///
    /// **Adding it was NOT fail-loud, and M163's report said it was.** The
    /// claim was that any other branch writing a `MetaKinds` literal would get
    /// `E0063`. **Five** literals end in `..Default::default()`: `labelshot`
    /// names `lang` explicitly, `local_player_data.rs`'s documents why it
    /// wants `None`, and the other three — `capeshot_cmd.rs`,
    /// `healthbarshot_cmd.rs`, `mobshot_cmd.rs` — took `None` silently. That
    /// is the right value for all three (none of them grades a nametag), but
    /// the *mechanism* is the merge-silent one §0.0's allocation table is
    /// about, so it is written down rather than left as a claim nobody
    /// checked. Deriving `Default` and offering `From<Option<i32>>` is what
    /// makes the struct-update form available at all, and both are
    /// load-bearing for the M18-shaped callers — so this is a stated property
    /// rather than a thing to fix.
    pub lang: Option<&'a rewo_data::lang::Language>,
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

/// The gate's door to [`apply_set_entity_data`] (M169): `gaugeshot` drives a
/// DASH entry through the real parser, the real kind gate and the real
/// `arm_dash_cooldown`, which `route_set_entity_data` would also do but
/// only with resolved packet ids in hand.
pub fn apply_set_entity_data_for_gate<'a>(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    kinds: impl Into<MetaKinds<'a>>,
) {
    apply_set_entity_data(body, entities, kinds)
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
    let meta = crate::metadata::parse(&mut r, kinds);
    // `Some(None)` is the `Optional<Component>`'s EMPTY arm — an explicit
    // clear. It used to be indistinguishable from "index 2 absent", so a
    // server removing a custom name never removed it.
    if let Some(name) = meta.custom_name {
        entities.set_custom_name(eid, name);
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
    // M169 — the DASH flags arm a dash cooldown on the vehicle they name,
    // and nothing else: `Camel.onSyncedDataUpdated` fires on the accessor,
    // not on a value change, so the flag's value is deliberately not read.
    if meta.bool19.is_some() && kinds.classes.is_some_and(|c| c.is_camel(type_id)) {
        entities.arm_dash_cooldown(eid, 55);
    }
    if meta.bool20.is_some() && kinds.classes.is_some_and(|c| c.is_nautilus(type_id)) {
        entities.arm_dash_cooldown(eid, 40);
    }
    if let Some(t) = meta.long20 {
        if kinds.classes.is_some_and(|c| c.is_camel(type_id)) {
            entities.set_last_pose_change_tick(eid, t);
        }
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
    // Slot 19 LONG → `Bee.DATA_ANGER_END_TIME` (M141f). Kind-gated, because
    // 19 is Bee's own accessor and another class's nineteenth could be a LONG
    // too. **Not a flag but a deadline**, so it is stored raw and compared
    // against the world clock at read time — see `tickable::is_angry`.
    if let Some(t) = meta.long19 {
        if Some(type_id) == kinds.bee {
            entities.set_anger_end_time(eid, t);
        }
    }
    // Slot 17 INT → `Guardian.DATA_ID_ATTACK_TARGET` (M141g). A **third**
    // claimant of an index the spellcaster BYTE and the pillager BOOLEAN
    // already share, and a fourth reading of the INT specifically
    // (`TropicalFish.DATA_ID_TYPE_VARIANT` is the other), so the kind gate is
    // load-bearing rather than defensive.
    //
    // `onSyncedDataUpdated` resets `clientSideAttackTime` on **every** arrival
    // of this accessor, not on a change — `assignValues` has no change guard
    // (M141e's finding) — so the reset is unconditional here too.
    if let Some(target) = meta.int17 {
        if Some(type_id) == kinds.guardian || Some(type_id) == kinds.elder_guardian {
            entities.set_guardian_attack_target(eid, target);
        }
    }
    // Slot 3 BOOLEAN → `Entity.DATA_CUSTOM_NAME_VISIBLE` (M70). No kind gate,
    // for the same reason as slot 0: `Entity` owns 0..7, so nothing else can
    // claim it. `false` is applied as eagerly as `true` — the server toggles it
    // off as well as on, and treating this as a latch would leave a nametag up
    // after `/data merge` cleared the flag.
    if let Some(visible) = meta.custom_name_visible {
        entities.set_custom_name_visible(eid, visible);
    }
    // Slot 4 BOOLEAN → `Entity.DATA_SILENT` (M138a). No kind gate, for the same
    // reason as slots 0 and 3. Applied both ways, and read by
    // `EntityTableWorld::entity_silent`, which answered a hardcoded `false`
    // until this line existed.
    if let Some(silent) = meta.silent {
        entities.set_silent(eid, silent);
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

/// The local player's own `set_entity_data`, for the one entry Rewo reads off
/// it (M82).
///
/// **[`apply_set_entity_data`] cannot do this**, and the reason is
/// `REWO_PLAN.md` §0.0 gotcha 13 again: its second statement is
/// `entities.get(eid)`, and Rewo's [`rewo_world::entities::EntityTable`] never
/// holds the local player, so every metadata packet the server addresses to
/// you is dropped. Vanilla's `handleSetEntityData` looks the id up in the
/// *level*, which does contain it. So the same body is walked a second time
/// when it names the camera entity — the shape M73 added
/// [`attributes::apply_local_attributes`] for.
///
/// The entry is `Player.DATA_SCORE_ID`, **index 18, INT**, which the death
/// screen renders as `deathScreen.score.value`. The index is counted up the
/// hierarchy: `Entity` 0..7, `LivingEntity` 8..14, `Avatar` 15 (main hand) and
/// 16 (mode customisation), `Player` 17 (absorption, FLOAT) and **18**
/// (score). The two `Avatar` slots are not a guess — M19 and M60 already read
/// them at 15 and 16 and are gated live.
///
/// Index 18 INT is polymorphic (`Axolotl`'s variant is read there too), so the
/// caller's local-player id is the kind gate; it is a stronger one than the
/// type-id gates elsewhere in this file, because there is exactly one entity
/// it can be.
///
/// Silent on a body that names anybody else, and on one that does not decode.
pub(crate) fn apply_local_player_score(body: &[u8], local_player: Option<i32>, score: &mut i32) {
    let mut r = PacketReader::new(body);
    let Ok(eid) = r.varint() else {
        return;
    };
    if local_player != Some(eid) {
        return;
    }
    // No component table: the score is an INT, and any entry this walk cannot
    // size stops it — which is the existing `parse` contract, not a new rule.
    if let Some(v) = crate::metadata::parse(&mut r, MetaKinds::default()).int18 {
        log::debug!("net: local player score = {v}");
        *score = v;
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
// M77 — three packets that write entity state
// ---------------------------------------------------------------------------

/// Decode a `move_minecart_along_track` body — `(entity id, steps)` (M77).
///
/// Wire form, from `ClientboundMoveMinecartPacket.STREAM_CODEC` composed with
/// `NewMinecartBehavior.MinecartStep.STREAM_CODEC`:
///
/// ```text
/// VarInt entityId
/// VarInt count                       (ByteBufCodecs.list(), maxSize = MAX_VALUE)
/// count × {
///   f64 f64 f64                      position — Vec3.STREAM_CODEC
///   f64 f64 f64                      movement — Vec3.STREAM_CODEC
///   i8                               yRot — ROTATION_BYTE
///   i8                               xRot — ROTATION_BYTE
///   f32                              weight
/// }
/// ```
///
/// **Two traps this codebase has paid for already, both live here.** The
/// vectors are `Vec3.STREAM_CODEC` — plain doubles, 24 bytes each — and *not*
/// the `LP_STREAM_CODEC` bit-packing `set_entity_motion` uses (M68); the two
/// codecs sit on the same `Vec3` class. And a rotation is one **signed byte**
/// through `Mth.unpackDegrees` (`rot * 360 / 256f`), not a float.
///
/// One deliberate divergence: vanilla's `readCount` accepts a **negative**
/// count and its `for` loop then produces an empty list, where
/// [`PacketReader::count`] rejects it. Both outcomes mutate nothing — an empty
/// `addAll` and a dropped packet are the same non-event — and the hardened
/// reader is what keeps a hostile count from reserving unbounded memory.
pub fn parse_move_minecart(
    body: &[u8],
) -> rewo_proto::Result<(i32, Vec<rewo_world::minecart::MinecartStep>)> {
    use rewo_world::minecart::MinecartStep;
    let mut r = PacketReader::new(body);
    let eid = r.varint()?;
    // 24 + 24 + 1 + 1 + 4 — the exact minimum, so the count bound is tight.
    let n = r.count("minecart lerp steps", 54)?;
    let mut steps = Vec::with_capacity(n);
    for _ in 0..n {
        let position = [r.f64()?, r.f64()?, r.f64()?];
        let movement = [r.f64()?, r.f64()?, r.f64()?];
        let y_rot = unpack_degrees(r.i8()?);
        let x_rot = unpack_degrees(r.i8()?);
        let weight = r.f32()?;
        steps.push(MinecartStep {
            position,
            movement,
            y_rot,
            x_rot,
            weight,
        });
    }
    Ok((eid, steps))
}

/// `Mth.unpackDegrees(byte)` = `rot * 360 / 256.0F`. The numerator is **int**
/// arithmetic (`rot * 360`) and only the divide is float, which is exact for
/// every one of the 256 inputs either way — transcribed in that order anyway.
fn unpack_degrees(rot: i8) -> f32 {
    (rot as i32 * 360) as f32 / 256.0
}

/// Apply a `move_minecart_along_track` body (M77).
///
/// `handleMinecartAlongTrack` is two nested `instanceof`s and one `addAll`:
///
/// ```text
/// if (packet.getEntity(level) instanceof AbstractMinecart minecart)
///   if (minecart.getBehavior() instanceof NewMinecartBehavior behavior)
///     behavior.lerpSteps.addAll(packet.lerpSteps());
/// ```
///
/// The first guard is a class fact and is enforced here through
/// [`rewo_data::entity_types::EntityClasses::is_minecart`], for the reason
/// [`apply_animate`]'s living gate exists: a packet naming a cow must mutate
/// nothing, and an untracked id must mutate nothing at all.
///
/// **The second guard is not enforced, deliberately.** Which behaviour a cart
/// has is not a class fact: `AbstractMinecart`'s constructor picks
/// `NewMinecartBehavior` iff `level.enabledFeatures().contains(
/// MINECART_IMPROVEMENTS)`, and Rewo does not decode the feature-flag set
/// (`update_enabled_features` is not in `ids.rs`). It is also structurally
/// unreachable: `ServerEntity.sendChanges` only reaches `handleMinecartPosRot`
/// — the sole sender of this packet — down the same `instanceof
/// NewMinecartBehavior` branch, so a server with the flag off never sends one.
/// Guessing the flag would be worse than omitting it; decoding it is the
/// follow-up, recorded in `REWO_PACKET_COVERAGE.md`.
///
/// `classes` is `None` in the headless protocol harnesses, which interpret
/// nothing — the same convention [`apply_animate`] uses.
pub(crate) fn apply_move_minecart(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) {
    // Decoded whole before anything is applied: the step list is positional,
    // so a short read means the steps we did get are not the steps the server
    // sent, and half a schedule would drag the cart to a place it never was.
    let (eid, steps) = match parse_move_minecart(body) {
        Ok(v) => v,
        Err(e) => {
            log::debug!("play: move_minecart_along_track parse: {e}");
            return;
        }
    };
    // `packet.getEntity(level)` — an untracked id is inert.
    let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
        return;
    };
    // `instanceof AbstractMinecart`.
    if !classes.is_some_and(|c| c.is_minecart(type_id)) {
        return;
    }
    entities.push_minecart_steps(eid, &steps);
}

/// The narrowest clientbound-play dispatch seam for the minecart schedule.
/// Mirrors [`route_set_passengers`] / [`route_animate`] so
/// [`play::PlaySession`] and the `rideshot` oracle drive packet-id → decoder
/// through the same production code.
pub fn route_move_minecart_along_track(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) -> bool {
    if id == ids.cb_play_move_minecart_along_track {
        apply_move_minecart(body, entities, classes);
        true
    } else {
        false
    }
}

/// Decode a `set_entity_link` body — `(sourceId, destId)` (M77).
///
/// **Both are fixed big-endian `i32`s**, not var-ints: the packet's private
/// constructor is `input.readInt()` twice. That is the same shape
/// `ClientboundEntityEventPacket`'s id has (M17) and the opposite of nearly
/// every other entity-addressed packet, and reading it as a var-int decodes a
/// small id into a plausible wrong one rather than failing.
///
/// `destId == 0` is the wire's null: the sending constructor writes
/// `destEntity != null ? destEntity.getId() : 0`.
pub fn parse_set_entity_link(body: &[u8]) -> rewo_proto::Result<(i32, i32)> {
    let mut r = PacketReader::new(body);
    let source = r.i32()?;
    let dest = r.i32()?;
    Ok((source, dest))
}

/// Apply a `set_entity_link` body (M77).
///
/// ```text
/// if (level.getEntity(packet.getSourceId()) instanceof Leashable leashable)
///   leashable.setDelayedLeashHolderId(packet.getDestId());
/// ```
///
/// `Leashable` is an **interface**, so the gate is the union of `Mob`'s and
/// `AbstractBoat`'s subtrees — see
/// [`rewo_data::entity_types::EntityClasses::is_leashable`]. A `set_entity_link`
/// naming an armour stand, a minecart or an item entity mutates nothing.
///
/// This decodes and stores the holder id; the rope itself is drawn by M170's
/// `leash::build_ribbon` + `WorldRenderer::draw_leash`, fed by `collect_leashes`.
pub(crate) fn apply_set_entity_link(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) {
    let (source, dest) = match parse_set_entity_link(body) {
        Ok(v) => v,
        Err(e) => {
            log::debug!("play: set_entity_link parse: {e}");
            return;
        }
    };
    let Some(type_id) = entities.get(source).map(|e| e.type_id) else {
        return;
    };
    if !classes.is_some_and(|c| c.is_leashable(type_id)) {
        return;
    }
    entities.set_leash_holder(source, dest);
}

/// The narrowest clientbound-play dispatch seam for the leash holder.
pub fn route_set_entity_link(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) -> bool {
    if id == ids.cb_play_set_entity_link {
        apply_set_entity_link(body, entities, classes);
        true
    } else {
        false
    }
}

/// Decode a `projectile_power` body — `(entity id, accelerationPower)` (M77).
///
/// A **VarInt** id (unlike `set_entity_link`'s fixed i32, one packet away)
/// then a big-endian `f64`.
pub fn parse_projectile_power(body: &[u8]) -> rewo_proto::Result<(i32, f64)> {
    let mut r = PacketReader::new(body);
    let eid = r.varint()?;
    let power = r.f64()?;
    Ok((eid, power))
}

/// Apply a `projectile_power` body (M77).
///
/// ```text
/// if (level.getEntity(packet.getId()) instanceof AbstractHurtingProjectile p)
///   p.accelerationPower = packet.getAccelerationPower();
/// ```
///
/// The cast is narrow and worth naming: an **arrow is not one of these**. It
/// is an `AbstractArrow`, a sibling branch, so a `projectile_power` naming one
/// mutates nothing. The six types that pass are the fireball family, the
/// wither skull and the two wind charges.
pub(crate) fn apply_projectile_power(
    body: &[u8],
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) {
    let (eid, power) = match parse_projectile_power(body) {
        Ok(v) => v,
        Err(e) => {
            log::debug!("play: projectile_power parse: {e}");
            return;
        }
    };
    let Some(type_id) = entities.get(eid).map(|e| e.type_id) else {
        return;
    };
    if !classes.is_some_and(|c| c.is_hurting_projectile(type_id)) {
        return;
    }
    entities.set_projectile_power(eid, power);
}

/// The narrowest clientbound-play dispatch seam for a projectile's
/// acceleration power.
pub fn route_projectile_power(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    entities: &mut rewo_world::entities::EntityTable,
    classes: Option<&rewo_data::entity_types::EntityClasses>,
) -> bool {
    if id == ids.cb_play_projectile_power {
        apply_projectile_power(body, entities, classes);
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
        // M169: slot 7 is SADDLE — `Mob.isSaddled()`, the whole of a horse's
        // `canJump()` and half of `getControllingPassenger()`. Read here
        // because the client has no other source for it.
        if slot_id & 127 == 7 {
            entities.set_saddled(eid, !matches!(slot, WireSlot::Empty));
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

/// `ClientLevel.playSound`'s distance delay, in TICKS (M162).
///
/// ```text
/// double distanceToSqr = camera.position().distanceToSqr(x, y, z);   // :747
/// if (distanceDelay && distanceToSqr > 100.0) {                      // :749
///    double delayInSeconds = Math.sqrt(distanceToSqr) / 40.0;        // :750
///    playDelayed(instance, (int)(delayInSeconds * 20.0));            // :751
/// }
/// ```
///
/// **The divisor is 40, not 340.** `crates/rewo-net/src/lib.rs` and
/// `REWO_PACKET_COVERAGE.md` both said 340 before this milestone, and 340
/// appears nowhere in `Level.java`, `ClientLevel.java` or `client/sounds/`.
/// The two are not close: at 100 blocks the right answer is **50 ticks** (2.5 s)
/// and 340 gives **5** (0.29 s), audible, plausible, and 8.5x wrong. Vanilla's
/// effective speed of sound is 40 blocks/s, which is a ninth of the real one —
/// the delay is a dramatic effect, not physics, so "it should be 340 m/s" is
/// exactly the intuition that produces the bug.
///
/// **Do not simplify `x / 40.0 * 20.0` to `x * 0.5`.** Neither 40 nor 20 is a
/// power of two, so both steps round, and the two expressions differ in the
/// last bit for some inputs. Both operations are transcribed.
///
/// **The gate is STRICT and on the SQUARE.** `distanceToSqr > 100.0`, so
/// exactly 10 blocks is not delayed. Writing `>=`, or comparing `dist > 10.0`
/// after a `sqrt`, moves the boundary — the second silently, because
/// `sqrt(100.0)` is exactly 10.0 and the two only disagree on inputs whose
/// square root is inexact.
///
/// The cast is Java `(int)`: truncation toward zero. Rust's `as i32` matches;
/// `.round()` does not, and the value is always non-negative so `.floor()`
/// happens to.
///
/// `None` means "not delayed" — play it now — and is returned for the
/// at-or-inside-10-blocks case only. The `distanceDelay` flag itself is the
/// caller's business, because a row that does not set it never asks.
pub fn distance_delay_ticks(camera: [f64; 3], pos: [f64; 3]) -> Option<i32> {
    // `Vec3.distanceToSqr(x, y, z)` — `(x - this.x)^2 + …`, no sqrt.
    let dx = pos[0] - camera[0];
    let dy = pos[1] - camera[1];
    let dz = pos[2] - camera[2];
    let distance_to_sqr = dx * dx + dy * dy + dz * dz;
    if distance_to_sqr <= 100.0 {
        return None;
    }
    let delay_in_seconds = distance_to_sqr.sqrt() / 40.0;
    Some((delay_in_seconds * 20.0) as i32)
}

/// `globalLevelEvent`'s bearing: two blocks from the camera, toward the block.
///
/// ```text
/// Vec3 directionToEvent = Vec3.atCenterOf(pos).subtract(camera.position()).normalize();
/// Vec3 soundPos = camera.position().add(directionToEvent.scale(2.0));
/// ```
///
/// **`subtract(vec)` is `this - vec`** (`Vec3.java:96-106`), so the vector runs
/// camera -> event. Reversing it is a perfect 180 deg inversion that puts a
/// wither's roar on the opposite side of your head, which no distance-shaped
/// witness and no compiler can see — only an ear, or a signed component.
///
/// **`normalize()` returns ZERO rather than erroring** when the length is
/// `< 1.0E-5F` (`Vec3.java:83-86`) — note the comparison is against a **float**
/// literal, which widens to `1.0000000116860974e-5` and not to `1e-5`. When it
/// fires, `soundPos = camera + ZERO * 2 = camera`, so the sound plays AT the
/// listener at full gain. That is not a skip and not a panic, and it is
/// reachable: stand inside the block a wither spawns at.
///
/// **The `+ 0.5` belongs to the bearing TARGET, not to the emitted position.**
/// `globalLevelEvent` calls the `double` overload of `playLocalSound` with an
/// already-absolute `soundPos`; only `Level.java:475`'s `BlockPos` overload adds
/// the half-block. Reusing the block path's centring here would be a second,
/// invisible half-block error on top of a correct one.
fn camera_bearing_position(camera: [f64; 3], block: (i32, i32, i32)) -> [f64; 3] {
    // `Vec3.atCenterOf(pos)` — the corner plus 0.5 on all three axes.
    let target = [
        block.0 as f64 + 0.5,
        block.1 as f64 + 0.5,
        block.2 as f64 + 0.5,
    ];
    let d = [
        target[0] - camera[0],
        target[1] - camera[1],
        target[2] - camera[2],
    ];
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    // `dist < 1.0E-5F`, widened. Written as the float literal so the constant
    // is the one vanilla compares against.
    let dir = if dist < f64::from(1.0E-5_f32) {
        [0.0, 0.0, 0.0]
    } else {
        [d[0] / dist, d[1] / dist, d[2] / dist]
    };
    [
        camera[0] + dir[0] * 2.0,
        camera[1] + dir[1] * 2.0,
        camera[2] + dir[2] * 2.0,
    ]
}

/// The sound a `level_event` packet asks for (M140; the tails, M162).
///
/// `rewo_data::level_event_sounds` has carried the whole 83-id switch since
/// M66 and had **zero production callers** until M140: most block interactions
/// — a dispenser firing, an anvil landing, a composter filling — arrive as a
/// `level_event` id rather than as a `sound` packet, so a large fraction of the
/// world's noise was silent no matter how good the device was.
///
/// M162 finishes it: the three camera-placed ids, the one listener-placed id,
/// and the nine distance-delayed ones, all of which M140 named as declined.
///
/// **The position is the block CENTRE** for a `Placement::Block` row.
/// `Level.playLocalSound(BlockPos, …)` delegates to `playLocalSound(
/// pos.getX() + 0.5, pos.getY() + 0.5, pos.getZ() + 0.5, …)` at
/// `Level.java:475`, and reading the corner instead puts every block sound half
/// a block out on all three axes — which at a 16-block attenuation radius is a
/// few percent of gain and a visible shift in the stereo image, wrong in a way
/// that looks like nothing in a log.
///
/// **The sound is named rather than numbered.** The table stores registry
/// names, so this yields [`crate::sounds::SoundRef::Inline`] — the
/// identified-by-name variant — instead of resolving through the report. That
/// costs nothing (M63's rule is that an inline event returns its own identifier
/// without consulting the table) and keeps the decode layer free of a registry
/// it would otherwise have to be handed.
///
/// # Where the camera enters, and why it is HERE
///
/// The `camera` argument is the listener's eye in world space, or `None` for
/// vanilla's `!camera.isInitialized()`.
///
/// Three seams were available: this function's parameter list, the sound
/// engine (`RampWorld::camera_position`, which already exists), or the
/// `PlaySession` call site. **The parameter, resolved by the caller at PACKET
/// time, is the one that matches vanilla and the only one that keeps
/// [`crate::sounds::SoundEvent::At`] meaning one thing.**
///
/// * `handleLevelEvent` calls `globalLevelEvent` **synchronously**
///   (`ClientPacketListener.java:1658-1665`), so vanilla's bearing is taken
///   against the camera at the moment the packet lands. Resolving it in the
///   engine defers it to the next `accept`, up to a tick later, and a tick is
///   about four blocks of walking.
/// * If the engine resolved it, a camera-placed event could not carry a
///   position at all — it would have to carry the TARGET BLOCK, and then
///   `At`'s `x`/`y`/`z` would mean "final world position" for `route_sound`
///   and "bearing target" for this one producer. A field whose meaning depends
///   on who filled it is how two readers come to disagree.
/// * `PlaySession` owns both the packet and `self.player`, so it can answer
///   without anything new being plumbed, and `RampWorld`'s seven implementors
///   stay untouched.
///
/// # The absent camera is TWO different things, one field apart
///
/// * For a **`Placement::Camera`** row it is silence. The guard is
///   `if (camera.isInitialized())` in `LevelEventHandler.java:66`, wrapping the
///   whole body, with no `else`.
/// * For a **distance-delayed block** row it is the **ORIGIN**, and the sound
///   still plays. `ClientLevel.playSound` has no `isInitialized()` check at
///   all, and `Camera.java:49` is `private Vec3 position = Vec3.ZERO;` — so an
///   uninitialised camera measures the delay against `(0, 0, 0)`.
///
/// Treating the absent camera uniformly is the natural implementation and is
/// wrong for one of the two. The asymmetry is vanilla's, not a nicety: the
/// guard lives in `LevelEventHandler` and the delay lives in `ClientLevel`.
///
/// ## What this still deliberately does not do
///
/// * **Pitch is `1.0`.** Over thirty rows randomise it off `ClientLevel.random`,
///   a generator with nothing on the wire, so there is no correct value to
///   transcribe — the jitter belongs to the playback layer, which is the same
///   argument the table's own module doc makes for not carrying it. That
///   includes 1032, whose vanilla pitch is `random.nextFloat() * 0.4F + 0.8F`
///   and whose 1.0 is that band's midpoint. What 1032 must NOT lose is which
///   argument is which: `forLocalAmbience(sound, PITCH, VOLUME)` takes pitch
///   second, so its literal `0.25F` is the VOLUME.
/// * **`seed` is `0`.** `level_event` carries none, so the playback layer
///   draws its own — which is what vanilla does, since `playLocalSound` passes
///   `this.random.nextLong()`. `SoundEngine::resolve` currently feeds 0 for
///   both `Some(0)` and `None`, so this is unobservable through behaviour and
///   pinned only by a witness that reads the field.
pub fn route_level_event_sound(
    body: &[u8],
    camera: Option<[f64; 3]>,
) -> Option<crate::sounds::SoundEvent> {
    use rewo_data::level_event_sounds::Placement;
    let mut r = PacketReader::new(body);
    let kind = r.i32().ok()?;
    let (x, y, z) = r.position().ok()?;
    let data = r.i32().ok()?;
    let global = r.bool().ok()?;

    // `global` is matched rather than ignored: `globalLevelEvent` and
    // `levelEvent` are disjoint switches, so a mismatched flag is silence in
    // vanilla too rather than a fall-through.
    let row = rewo_data::level_event_sounds::resolve(kind, data, global)?;
    let source = crate::sounds::SoundSource::from_name(row.source)?;
    // Two rows of seventy carry no literal; 1.0 is vanilla's own default for
    // them. The rest matter a great deal — a ghast fireball is 10.0 and a bat
    // taking off is 0.05, a factor of two hundred.
    let volume = row.volume.unwrap_or(1.0);

    if row.placement == Placement::Listener {
        // `SimpleSoundInstance.forLocalAmbience` — AMBIENT, `Attenuation.NONE`,
        // `relative = true`, position (0, 0, 0). It plays in your head by
        // construction, so it needs no camera and must not be given one:
        // emitting it as a positioned sound AT the camera would pan and
        // attenuate where vanilla's does neither.
        //
        // The source is not plumbed because `forLocalAmbience` **hardcodes**
        // `SoundSource.AMBIENT` and drops its caller's. The table's 1032 row
        // agrees, and the test below asserts that agreement rather than
        // pretending the field reaches anything.
        return Some(crate::sounds::SoundEvent::Instance(
            crate::sound_instance::SoundInstance::for_local_ambience(row.sound, 1.0, volume),
        ));
    }

    let sound = crate::sounds::SoundRef::Inline {
        name: row.sound.to_string(),
        // `getRange` falls back to `16 * max(volume, 1)`, which is what a level
        // event wants; a fixed range here would override it.
        fixed_range: None,
    };

    let position = match row.placement {
        // `LevelEventHandler.java:66` — the whole body is inside
        // `if (camera.isInitialized())`, with no `else`.
        Placement::Camera => camera_bearing_position(camera?, (x, y, z)),
        Placement::Block => [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5],
        // Unreachable: the listener arm returns above. A **fallback rather
        // than a panic**, because this runs inside a packet handler and a
        // panic there takes the whole session down — and the exhaustive match
        // is kept so a fourth `Placement` fails the BUILD instead of quietly
        // taking the block path.
        Placement::Listener => [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5],
    };

    let positioned = crate::sounds::PositionedSound {
        sound,
        source,
        x: position[0],
        y: position[1],
        z: position[2],
        volume,
        pitch: 1.0,
        seed: 0,
    };

    // The delay is measured to the sound's FINAL position, and every
    // camera-placed row passes `distanceDelay = false` anyway — so this can
    // only fire for a block row. Asserting the FLAG rather than the outcome is
    // what keeps the camera half of that from being a tautology: the bearing
    // position is two blocks from the camera by construction, so
    // `distanceToSqr == 4.0` and the gate could never open for it whatever the
    // flag said.
    let ticks = if row.distance_delay {
        // The absent camera is the ORIGIN here rather than a refusal — see the
        // asymmetry in this function's docs.
        distance_delay_ticks(camera.unwrap_or([0.0, 0.0, 0.0]), position)
    } else {
        None
    };

    Some(match ticks {
        Some(ticks) => crate::sounds::SoundEvent::AtDelayed {
            sound: positioned,
            ticks,
        },
        None => crate::sounds::SoundEvent::At(positioned),
    })
}

#[cfg(test)]
mod level_event_sound_tests {
    //! `level_event` asks for sounds too (M140).
    //!
    //! The table has been complete and partition-tested since M66 and had no
    //! production caller, so every one of these ids was silent — a dispenser,
    //! an anvil, a composter, a wither spawning.

    use crate::sounds::{SoundEvent, SoundRef};

    /// `kind: i32`, packed `BlockPos`, `data: i32`, `global: bool`.
    fn body(kind: i32, x: i64, y: i64, z: i64, data: i32, global: bool) -> Vec<u8> {
        let packed = ((x & 0x3FF_FFFF) << 38) | ((z & 0x3FF_FFFF) << 12) | (y & 0xFFF);
        let mut b = kind.to_be_bytes().to_vec();
        b.extend_from_slice(&(packed as i64).to_be_bytes());
        b.extend_from_slice(&data.to_be_bytes());
        b.push(u8::from(global));
        b
    }

    /// A camera **inside** ten blocks of the fixture block (10.5, 64.5, -6.5),
    /// so a delayed row is immediate unless a test moves it, and at a position
    /// with no zero or repeated component — so a transposed axis cannot pass.
    ///
    /// The first version of this constant sat 10.33 blocks out and quietly
    /// tripped the delay gate, which is the sort of thing a fixture does when
    /// its distance is chosen by eye rather than computed.
    const CAM: [f64; 3] = [7.5, 66.5, -4.5];

    /// **Option-shaped, and that is a fix rather than a style.** This used to
    /// `panic!` on any variant but `At`, which made it a witness that could
    /// silently not run (`REWO_PLAN` §0.0 gotcha 15) — and the moment 1032
    /// started yielding a `SoundEvent::Instance` it would have aborted the test
    /// binary rather than reporting.
    fn ev(kind: i32, data: i32, global: bool) -> Option<SoundEvent> {
        super::route_level_event_sound(&body(kind, 10, 64, -7, data, global), Some(CAM))
    }

    fn at(kind: i32, data: i32, global: bool) -> Option<crate::sounds::PositionedSound> {
        match ev(kind, data, global) {
            Some(SoundEvent::At(p)) => Some(p),
            Some(SoundEvent::AtDelayed { sound, .. }) => Some(sound),
            _ => None,
        }
    }

    /// **The block CENTRE, not the corner.**
    ///
    /// `Level.playLocalSound(BlockPos, …)` delegates to `pos.getX() + 0.5` on
    /// all three axes (`Level.java:475`). The corner reading puts every block
    /// sound half a block out in three axes at once, which at a 16-block radius
    /// is a few percent of gain and a visible shift in the image — and looks
    /// like nothing at all in a log.
    #[test]
    fn a_dispenser_sounds_at_the_block_centre() {
        let s = at(1000, 0, false).expect("1000 is a sound");
        assert_eq!((s.x, s.y, s.z), (10.5, 64.5, -6.5));
        match &s.sound {
            SoundRef::Inline { name, fixed_range } => {
                assert_eq!(name, "minecraft:block.dispenser.dispense");
                // `getRange` must fall back to `16 * max(volume, 1)`; a fixed
                // range here would override it.
                assert_eq!(*fixed_range, None);
            }
            other => panic!("expected a named sound, got {other:?}"),
        }
        assert_eq!(s.source, crate::sounds::SoundSource::Blocks);
    }

    /// Volume is carried, and it is not decorative.
    #[test]
    fn volume_spans_a_factor_of_two_hundred() {
        // A ghast warning against a bat taking off.
        assert_eq!(at(1015, 0, false).unwrap().volume, 10.0);
        assert_eq!(at(1025, 0, false).unwrap().volume, 0.05);
        // A mixer that assumed 1.0 everywhere would be wrong by 200x across
        // these two, which is only ever audible rather than assertable
        // elsewhere.
        assert_eq!(at(1031, 0, false).unwrap().volume, 0.3, "anvil landing");
    }

    /// **`data` gates some ids, and an ungated value is silence.**
    ///
    /// 1009 is two different sounds for `data` 0 and 1, and vanilla's
    /// `if/else if` has no `else` — so 2 is silence rather than a fall-through
    /// to the first branch, which is exactly what a `match` written from the
    /// id alone would produce.
    #[test]
    fn a_data_gated_id_is_silent_outside_its_branches() {
        let a = at(1009, 0, false).expect("data 0");
        let b = at(1009, 1, false).expect("data 1");
        assert_ne!(a.sound, b.sound, "two different sounds, not one");
        assert!(at(1009, 2, false).is_none(), "no else branch");
        assert!(at(1009, 99, false).is_none());
    }

    /// The global flag is matched, because the two switches are disjoint.
    ///
    /// **And every global row is camera-placed, so this function never emits a
    /// sound for a global packet at all** — a structural fact rather than a
    /// coincidence of the three ids, and worth pinning: if a block-placed global
    /// row is ever added, the second assertion here fails and tells whoever
    /// added it that the camera boundary now has a case it did not before.
    ///
    /// This test first contained `assert!(… || true)`, which is a tautology and
    /// passes against anything. It survived exactly as long as it took to read
    /// it back.
    #[test]
    fn a_mismatched_global_flag_is_silence() {
        use rewo_data::level_event_sounds::{Placement, SOUNDS};
        // 1000 is a `levelEvent`, so the global switch does not have it.
        assert!(at(1000, 0, false).is_some());
        assert!(at(1000, 0, true).is_none(), "the flag is matched, not ignored");

        // 1023 IS in the global switch — the table knows it — and is refused
        // here for its placement rather than its flag.
        assert!(rewo_data::level_event_sounds::resolve(1023, 0, true).is_some());
        assert!(at(1023, 0, false).is_none(), "not in the local switch");

        assert!(
            SOUNDS
                .iter()
                .filter(|s| s.global)
                .all(|s| s.placement == Placement::Camera),
            "a block-placed global row would be a new case for this function"
        );
    }

    /// **Every** camera-placed row lands two blocks from the camera, on the
    /// camera-to-event side (M162; M140 declined all of them).
    ///
    /// Driven off the table rather than off the three literal ids, so the
    /// completeness claim survives a table edit.
    ///
    /// The **sign of the dot product** is the assertion that matters. A
    /// distance-only check passes at 180 degrees, which is exactly what
    /// reversing `Vec3.subtract` produces — and that reversal is invisible to
    /// the compiler, to a count, and to every existing witness.
    #[test]
    fn every_camera_placed_row_lands_two_blocks_along_the_bearing() {
        use rewo_data::level_event_sounds::{Placement, SOUNDS};
        let mut seen = 0;
        for row in SOUNDS.iter().filter(|s| s.placement == Placement::Camera) {
            let p = at(row.id, 0, row.global).expect("camera row yields a sound");
            let to_sound = [p.x - CAM[0], p.y - CAM[1], p.z - CAM[2]];
            let len = (to_sound[0] * to_sound[0]
                + to_sound[1] * to_sound[1]
                + to_sound[2] * to_sound[2])
                .sqrt();
            assert!((len - 2.0).abs() < 1e-9, "id {} is {len} blocks out", row.id);
            // The block centre is (10.5, 64.5, -6.5); the camera is CAM.
            let to_event = [10.5 - CAM[0], 64.5 - CAM[1], -6.5 - CAM[2]];
            let dot = to_sound[0] * to_event[0]
                + to_sound[1] * to_event[1]
                + to_sound[2] * to_event[2];
            assert!(dot > 0.0, "id {} points AWAY from the event", row.id);
            // And NOT at the block: the whole point is that a global event is
            // heard as a direction rather than as a distant place.
            assert!((p.x - 10.5).abs() > 1.0, "id {} sits at the block", row.id);
            seen += 1;
        }
        assert_eq!(seen, 3, "the three globalLevelEvent ids");
    }

    /// A camera-placed row with **no camera** is silence, and a block-placed
    /// one is not (M162).
    ///
    /// The asymmetry is vanilla's and it is one field apart:
    /// `LevelEventHandler.java:66` wraps the whole `globalLevelEvent` body in
    /// `if (camera.isInitialized())`, while `ClientLevel.playSound` has no such
    /// guard and `Camera.java:49` defaults its position to `Vec3.ZERO`.
    /// Treating the absent camera uniformly — the natural implementation — is
    /// wrong for exactly one of the two.
    #[test]
    fn an_uninitialised_camera_silences_only_the_global_rows() {
        let none = |kind: i32, global: bool| {
            super::route_level_event_sound(&body(kind, 10, 64, -7, 0, global), None)
        };
        assert!(none(1023, true).is_none(), "wither spawn needs a camera");
        assert!(none(1028, true).is_none(), "dragon death needs a camera");
        assert!(none(1038, true).is_none(), "end portal needs a camera");
        // A dispenser does not, and neither does a delayed trial spawner.
        assert!(none(1000, false).is_some(), "a block row still plays");
        assert!(none(3012, false).is_some(), "…including a delayed one");
    }

    /// 1032 is a `forLocalAmbience` instance: relative, unattenuated, at the
    /// origin, AMBIENT, volume 0.25 (M162).
    ///
    /// Every one of those is separately guessable and none is observable in a
    /// count, so each is named. In particular **`forLocalAmbience(sound, PITCH,
    /// VOLUME)` takes pitch second**, so the call's literal `0.25F` is the
    /// volume — reading the argument list left to right gives a portal three
    /// times too loud at a fixed pitch.
    #[test]
    fn every_listener_placed_row_is_a_relative_unattenuated_instance() {
        use crate::sound_instance::Attenuation;
        use rewo_data::level_event_sounds::{Placement, SOUNDS};
        let mut seen = 0;
        for row in SOUNDS.iter().filter(|s| s.placement == Placement::Listener) {
            let got = ev(row.id, 0, row.global);
            let Some(SoundEvent::Instance(i)) = got else {
                panic!("id {} yielded {got:?}, not an Instance", row.id)
            };
            assert_eq!(i.identifier, row.sound);
            assert!(i.relative, "id {}", row.id);
            assert_eq!(i.attenuation, Attenuation::None);
            assert_eq!((i.x, i.y, i.z), (0.0, 0.0, 0.0));
            assert_eq!(i.volume, row.volume.unwrap_or(1.0));
            assert_eq!(i.seed, None, "forLocalAmbience uses createUnseededRandom");
            // `forLocalAmbience` HARDCODES `SoundSource.AMBIENT` and drops the
            // caller's source. The table agrees today; assert the agreement
            // rather than plumbing a field that reaches nothing.
            assert_eq!(i.source, crate::sounds::SoundSource::Ambient);
            assert_eq!(row.source, "ambient", "the table would be overridden");
            seen += 1;
        }
        assert_eq!(seen, 1, "1032 is the only listener-placed row");
    }

    /// The camera-placed rows are still refused when the GLOBAL flag is wrong,
    /// and the table still knows them — the half of M140's test that survives.
    #[test]
    fn a_camera_row_still_needs_its_global_flag() {
        assert!(at(1023, 0, false).is_none(), "1023 is not in the local switch");
        assert!(rewo_data::level_event_sounds::resolve(1023, 0, true).is_some());
        assert!(rewo_data::level_event_sounds::resolve(1032, 0, false).is_some());
    }

    /// A particle-only id asks for no sound, and an unknown one does not panic.
    #[test]
    fn particle_only_and_unknown_ids_are_silent() {
        // 2000 is smoke along a face — one of the SILENT ids.
        assert!(at(2000, 0, false).is_none());
        assert!(at(123_456, 0, false).is_none(), "unknown id");
        // A truncated body is refused rather than read past its end.
        assert!(super::route_level_event_sound(&[0, 0, 3, 232], Some(CAM)).is_none());
        assert!(super::route_level_event_sound(&[], Some(CAM)).is_none());
    }

    /// Every id the table calls a sound and places at a block comes through.
    ///
    /// The completeness claim: a hand-written `match` in the router would have
    /// drifted from the table the first time either changed, so this asserts
    /// the two agree for the whole set rather than for the handful above.
    #[test]
    fn every_block_placed_row_yields_a_sound() {
        use rewo_data::level_event_sounds::{Placement, SOUNDS};
        let mut seen = 0;
        for row in SOUNDS.iter().filter(|s| s.placement == Placement::Block) {
            // A `data` value the row's own gate accepts. Derived from the
            // gate rather than hardcoded, so a new gate variant fails the build
            // here instead of quietly making this loop test nothing.
            use rewo_data::level_event_sounds::DataGate as G;
            let data = match row.data {
                G::Always => 0,
                G::Eq(v) => v,
                G::Ne(v) => v.wrapping_add(1),
                G::Gt(v) => v + 1,
                G::Le(v) => v,
            };
            let got = at(row.id, data, row.global);
            assert!(got.is_some(), "id {} data {data} yielded nothing", row.id);
            seen += 1;
        }
        assert!(seen > 50, "only {seen} block-placed rows; the table shrank?");
    }

    // ── the distance delay (M162) ──────────────────────────────────────────

    /// **The divisor is 40, not 340**, and the 100-block row is the one that
    /// separates them: 50 ticks against 5.
    ///
    /// Two places in this tree said 340 before M162 (this file's own doc, and
    /// `REWO_PACKET_COVERAGE.md`) while `level_event_sounds.rs`'s module doc
    /// said 40. The wrong one is the plausible one — 340 m/s is the real speed
    /// of sound — which is exactly why it needs a number rather than a comment.
    #[test]
    fn the_divisor_is_forty_and_a_hundred_blocks_is_fifty_ticks() {
        let at_distance = |d: f64| super::distance_delay_ticks([0.0; 3], [d, 0.0, 0.0]);
        assert_eq!(at_distance(100.0), Some(50));
        assert_eq!(at_distance(400.0), Some(200));
        // What /340 would have produced for the same two.
        assert_ne!(at_distance(100.0), Some(5));
    }

    /// The gate is `distanceToSqr > 100.0` — **strict, and on the square**.
    ///
    /// Exactly ten blocks is NOT delayed. `>=` moves the boundary by one
    /// point; comparing `sqrt(dsq) > 10.0` moves it for every distance whose
    /// square root is inexact, which is almost all of them.
    #[test]
    fn the_delay_boundary_is_strict_and_on_the_square() {
        let at_distance = |d: f64| super::distance_delay_ticks([0.0; 3], [d, 0.0, 0.0]);
        assert_eq!(at_distance(10.0), None, "exactly 10 is not delayed");
        assert_eq!(at_distance(9.999), None);
        assert_eq!(at_distance(10.001), Some(5));
        // Measured from the camera, not from the origin: same block, camera
        // moved, different answer.
        assert_eq!(
            super::distance_delay_ticks([90.0, 0.0, 0.0], [100.0, 0.0, 0.0]),
            None
        );
    }

    /// `x / 40.0 * 20.0` is not `x * 0.5`, **and the difference reaches the
    /// TICK COUNT** — which the first version of this test did not show.
    ///
    /// Neither 40 nor 20 is a power of two, so both steps round; that much is
    /// easy to assert and nearly worthless, because the result is truncated to
    /// an integer and a one-ULP difference almost always vanishes there. The
    /// first draft asserted only the float inequality, and the mutation
    /// replacing the two steps with one multiply **survived the whole battery**
    /// — including `soundshot`, whose distances all sit far from a boundary.
    ///
    /// So it was measured instead of argued. Over the 3,995 tick boundaries
    /// from 6 to 4,000 ticks, sweeping `distanceToSqr` by ULPs, **595 carry a
    /// value at which the two disagree by a whole tick**. This is the first of
    /// them: at `distanceToSqr == 195.99999999999994` — reachable as a camera
    /// at the origin and a sound at `x = 13.999999999999998`, and comfortably
    /// past the `> 100.0` gate — vanilla answers **7** and the simplification
    /// answers **6**.
    #[test]
    fn the_two_divisions_are_not_one_multiply() {
        // The exact input, reached the way production reaches it: from a
        // camera and a position, not by handing the function a squared
        // distance it would otherwise compute itself.
        let pos = [13.999999999999998_f64, 0.0, 0.0];
        let dsq = pos[0] * pos[0];
        assert_eq!(dsq, 195.99999999999994, "the fixture's own arithmetic");
        assert!(dsq > 100.0, "and it must clear the gate to be observable");
        assert_eq!(super::distance_delay_ticks([0.0; 3], pos), Some(7));
        // What the one-multiply simplification would answer for the same input.
        assert_eq!((dsq.sqrt() * 0.5) as i32, 6);
        assert_eq!((dsq.sqrt() / 40.0 * 20.0) as i32, 7);
    }

    /// **Nine ids delay and every other row does not**, read out of the decoded
    /// EVENT rather than out of the table.
    ///
    /// The table's own partition test already covers the table; this covers the
    /// wire-to-event hop, which is where a flag gets read and dropped.
    #[test]
    fn exactly_the_delayed_rows_reach_the_engine_as_delayed() {
        use rewo_data::level_event_sounds::{DataGate as G, Placement, SOUNDS};
        // EXACTLY 300 blocks out on one axis — the block centre is
        // (10.5, 64.5, -6.5), so the camera goes at 310.5 and not at 310.
        // Far enough that every delayed row's gate opens, so a row that arrives
        // immediate did so because of its FLAG.
        let far = Some([310.5, 64.5, -6.5]);
        let mut delayed_ids = std::collections::BTreeSet::new();
        let mut immediate = 0;
        for row in SOUNDS.iter().filter(|s| s.placement == Placement::Block) {
            let data = match row.data {
                G::Always => 0,
                G::Eq(v) => v,
                G::Ne(v) => v.wrapping_add(1),
                G::Gt(v) => v + 1,
                G::Le(v) => v,
            };
            let got = super::route_level_event_sound(
                &body(row.id, 10, 64, -7, data, row.global),
                far,
            );
            match got {
                Some(SoundEvent::AtDelayed { ticks, .. }) => {
                    assert!(row.distance_delay, "id {} delayed without the flag", row.id);
                    // 300 / 40 = 7.5 s, * 20 = 150 ticks exactly. Derived
                    // from the fixture geometry, not read back from the code.
                    assert_eq!(ticks, 150, "300 blocks / 2");
                    delayed_ids.insert(row.id);
                }
                Some(SoundEvent::At(_)) => {
                    assert!(!row.distance_delay, "id {} dropped its flag", row.id);
                    immediate += 1;
                }
                other => panic!("id {} yielded {other:?}", row.id),
            }
        }
        assert_eq!(delayed_ids.len(), 9, "the trial-spawner family: {delayed_ids:?}");
        assert!(immediate > 50, "only {immediate} immediate rows?");
    }

    /// A delayed row **near** the camera is an ordinary immediate sound, and
    /// the two carry the same `PositionedSound`.
    ///
    /// `ClientLevel.java:748` constructs the instance ABOVE the branch, so a
    /// delayed sound is byte-identical to an immediate one. A design that gave
    /// delayed sounds their own construction path would pass every witness
    /// above and fail this one.
    #[test]
    fn a_delayed_row_near_the_camera_is_immediate_and_otherwise_identical() {
        let near = super::route_level_event_sound(&body(3012, 10, 64, -7, 0, false), Some(CAM));
        let far = super::route_level_event_sound(
            &body(3012, 10, 64, -7, 0, false),
            Some([310.5, 64.5, -6.5]),
        );
        let (Some(SoundEvent::At(a)), Some(SoundEvent::AtDelayed { sound: b, ticks })) =
            (near, far)
        else {
            panic!("expected one of each")
        };
        assert!(ticks > 0);
        assert_eq!(a, b, "the delay must not change the sound");
    }

    /// The camera-placed rows pass `distanceDelay = false`, and **the flag is
    /// what this asserts rather than the outcome**.
    ///
    /// Asserting "a global event is never delayed" would be a tautology: the
    /// bearing position is 2.0 blocks from the camera by construction, so
    /// `distanceToSqr == 4.0` and the gate could never open whatever the flag
    /// said. So: read the flag off the table, and check the position that makes
    /// the outcome moot.
    #[test]
    fn the_camera_rows_carry_no_delay_flag_and_could_not_use_one() {
        use rewo_data::level_event_sounds::{Placement, SOUNDS};
        for row in SOUNDS.iter().filter(|s| s.placement == Placement::Camera) {
            assert!(!row.distance_delay, "id {} sets the flag", row.id);
        }
        // Two blocks out => dsq 4.0, an order of magnitude inside the gate.
        assert_eq!(super::distance_delay_ticks([0.0; 3], [2.0, 0.0, 0.0]), None);
    }
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

/// The clientbound-play dispatch seam for the six world-border packets (M80).
/// Returns whether the id matched — **not** whether the body decoded.
///
/// The id table exists for the same reason `route_view_area`'s does, and the
/// hazard here is sharper: `set_border_warning_delay` and
/// `set_border_warning_distance` are *both* a single VarInt, and they write
/// different fields. A dispatcher that inferred the kind from the body would
/// swap the vignette's proximity threshold with its timing one and never
/// report an error.
pub fn route_border(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    border: &mut rewo_world::border::WorldBorder,
) -> bool {
    let table = border::BorderIds {
        initialize: ids.cb_play_initialize_border,
        center: ids.cb_play_set_border_center,
        lerp_size: ids.cb_play_set_border_lerp_size,
        size: ids.cb_play_set_border_size,
        warning_delay: ids.cb_play_set_border_warning_delay,
        warning_distance: ids.cb_play_set_border_warning_distance,
    };
    let Some(kind) = border::kind_for_id(id, table) else {
        return false;
    };
    if !border::apply(kind, body, border) {
        log::debug!("net: border {kind:?} decode failed ({} bytes)", body.len());
    }
    true
}

/// The clientbound-play dispatch seam for M83's locator bar. Returns whether
/// the id matched — **not** whether the body decoded.
///
/// One packet, so there is no id table to get wrong; the seam exists so the
/// waypoint map is written in exactly one place and the app can drive the same
/// entry point the session does.
///
/// **This handler deliberately does not touch [`rewo_world::entities::
/// EntityTable`]** — see REWO_PLAN §0.0 gotcha 13. A waypoint's identifier is
/// a UUID and the obvious first move is to resolve it to an entity, but the
/// two places that identifier is *compared* are both about the receiver: the
/// renderer skips the waypoint whose UUID is the **camera entity's**, which is
/// the local player, and the table never holds it. Resolving here would also
/// be wrong in the other direction — a waypoint is tracked whether or not its
/// subject is in render distance (that is the whole point of the chunk and
/// azimuth tiers), so an entity lookup would drop exactly the far-away
/// waypoints the bar exists to show.
pub fn route_waypoint(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    store: &mut waypoints::WaypointStore,
) -> bool {
    if id != ids.cb_play_waypoint {
        return false;
    }
    if !waypoints::apply(body, store) {
        log::debug!("net: waypoint decode failed ({} bytes)", body.len());
    }
    true
}

/// The clientbound-play dispatch seam for the four client switches: M74's
/// `change_difficulty`, `set_camera` and `container_close`, plus M76's
/// `set_default_spawn_position`. Returns whether the id matched — **not**
/// whether the body decoded.
///
/// `set_camera`'s target is resolved against `entities` **or** `local_player`.
/// Both are needed: vanilla resolves with `level.getEntity(id)` and its level
/// contains the local player, while Rewo's
/// [`rewo_world::entities::EntityTable`] never does — the server sends no
/// `add_entity` for you. Consulting only the table would make the packet that
/// hands the camera back at the end of a spectate a no-op, stranding the view
/// on the spectated entity for the rest of the session. See [`client_state`].
pub fn route_client_state(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    state: &mut client_state::ClientState,
    entities: &rewo_world::entities::EntityTable,
    local_player: Option<i32>,
) -> bool {
    let table = client_state::ClientStateIds {
        change_difficulty: ids.cb_play_change_difficulty,
        set_camera: ids.cb_play_set_camera,
        container_close: ids.cb_play_container_close,
        set_default_spawn_position: ids.cb_play_set_default_spawn_position,
    };
    let Some(kind) = client_state::kind_for_id(id, table) else {
        return false;
    };
    client_state::apply(kind, body, state, entities, local_player);
    true
}

/// The clientbound-play dispatch seam for M74's tick clock: `ticking_state`
/// and `ticking_step`. Returns whether the id matched — **not** whether the
/// body decoded.
///
/// The two ids are the only discriminator, exactly as with the view area's
/// radius pair: `ticking_step`'s body is a bare VarInt and says nothing about
/// which packet it belongs to.
pub fn route_ticking(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    manager: &mut ticking::TickRateManager,
) -> bool {
    let table = ticking::TickingIds {
        state: ids.cb_play_ticking_state,
        step: ids.cb_play_ticking_step,
    };
    let Some(kind) = ticking::kind_for_id(id, table) else {
        return false;
    };
    ticking::apply(kind, body, manager);
    true
}

/// The clientbound-**play** dispatch seam for M78's seven session / metadata /
/// chat packets. Returns whether the id matched — **not** whether the body
/// decoded.
///
/// `bundle_delimiter`, M78's eighth, is deliberately absent: it changes how
/// packets are *applied* rather than what one means, so it is consumed by
/// [`bundle::BundleAssembler`] before dispatch ever runs. See [`session`].
pub fn route_session(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    state: &mut session::SessionState,
) -> bool {
    let table = session::SessionIds {
        custom_payload: ids.cb_play_custom_payload,
        disguised_chat: ids.cb_play_disguised_chat,
        game_rule_values: ids.cb_play_game_rule_values,
        player_combat_end: ids.cb_play_player_combat_end,
        player_combat_enter: ids.cb_play_player_combat_enter,
        server_data: ids.cb_play_server_data,
        server_links: ids.cb_play_server_links,
        store_cookie: ids.cb_play_store_cookie,
    };
    let Some(kind) = session::kind_for_id(id, table) else {
        return false;
    };
    session::apply(kind, body, state);
    true
}

/// The clientbound-play dispatch seam for M79's title overlay and two HUD
/// gauges. Returns whether the id matched — **not** whether the body decoded.
///
/// The id table is load-bearing here in a way it is not for most of these
/// seams: `set_title_text`, `set_subtitle_text` and `set_action_bar_text` all
/// carry **exactly one NBT tag and nothing else**, so their bodies are
/// byte-for-byte indistinguishable. Nothing but the id says whether a
/// component belongs in the middle of the screen at 4× scale, under it at 2×,
/// or over the hotbar for sixty ticks.
pub fn route_hud_state(
    id: i32,
    body: &[u8],
    ids: &crate::ids::Ids,
    state: &mut hud_state::HudState,
) -> bool {
    let table = hud_state::HudIds {
        clear_titles: ids.cb_play_clear_titles,
        cooldown: ids.cb_play_cooldown,
        set_action_bar_text: ids.cb_play_set_action_bar_text,
        set_experience: ids.cb_play_set_experience,
        set_subtitle_text: ids.cb_play_set_subtitle_text,
        set_title_text: ids.cb_play_set_title_text,
        set_titles_animation: ids.cb_play_set_titles_animation,
    };
    let Some(kind) = hud_state::kind_for_id(id, table) else {
        return false;
    };
    hud_state::apply(kind, body, state);
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

/// M77's three decoders, at the level `rideshot` cannot reach: the byte
/// arithmetic and the malformed-body outcomes.
///
/// `rideshot` drives these through the router against a real `EntityTable`,
/// which is the right seam for everything it asserts — but it cannot ask for
/// a *negative* element count (no encoder produces one) and it samples two
/// rotation bytes, not all 256. These do.
#[cfg(test)]
mod m77_wire_tests {
    use super::{parse_move_minecart, parse_projectile_power, parse_set_entity_link, unpack_degrees};

    #[test]
    fn unpack_degrees_spans_the_whole_signed_byte() {
        // `rot * 360 / 256.0F`, exactly — 360/256 is 1.40625, a binary
        // fraction, so every one of the 256 answers is exact in f32.
        for (byte, want) in [
            (0i8, 0.0f32),
            (1, 1.406_25),
            (-1, -1.406_25),
            (64, 90.0),
            (-64, -90.0),
            (127, 178.593_75),
            (-128, -180.0),
        ] {
            assert_eq!(unpack_degrees(byte), want, "byte {byte}");
        }
        // The signed and unsigned readings differ by exactly 360 for every
        // negative byte — which is why nothing downstream of `Mth.rotLerp`
        // can tell them apart, and why the gate's witness reads the decode.
        for byte in i8::MIN..0 {
            let unsigned = ((byte as u8) as i32 * 360) as f32 / 256.0;
            assert_eq!(unsigned - unpack_degrees(byte), 360.0, "byte {byte}");
        }
    }

    #[test]
    fn an_empty_step_list_is_legal_and_a_negative_count_is_not() {
        // Count 0: a well-formed packet carrying no steps. Vanilla's
        // `addAll(emptyList())` is a no-op, and so is this.
        assert_eq!(parse_move_minecart(&[7, 0]).unwrap(), (7, Vec::new()));
        // Count -1 as a var-int (five bytes, all high bits set but the last).
        // Vanilla's `readCount` returns it and the `for` loop then yields an
        // empty list; `PacketReader::count` rejects it. Both mutate nothing —
        // the divergence is recorded here rather than papered over.
        let neg = [7u8, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
        assert!(parse_move_minecart(&neg).is_err());
    }

    #[test]
    fn a_step_count_cannot_outrun_the_body() {
        // 54 bytes an element, so a two-byte body claiming 100 steps is
        // rejected before anything is allocated.
        assert!(parse_move_minecart(&[7, 100]).is_err());
    }

    #[test]
    fn the_two_small_packets_need_their_whole_body() {
        // `set_entity_link` is 8 bytes of fixed i32s — seven is not enough,
        // and the eighth byte is part of `destId`, not padding.
        assert_eq!(
            parse_set_entity_link(&[0, 0, 1, 44, 0, 0, 0, 7]).unwrap(),
            (300, 7)
        );
        assert!(parse_set_entity_link(&[0, 0, 1, 44, 0, 0, 0]).is_err());
        // `projectile_power` is a VarInt then eight bytes of f64.
        let mut body = vec![5u8];
        body.extend_from_slice(&0.5f64.to_be_bytes());
        assert_eq!(parse_projectile_power(&body).unwrap(), (5, 0.5));
        assert!(parse_projectile_power(&body[..body.len() - 1]).is_err());
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
            map_id: 19,
            dye: 20,
            provides_banner_patterns: 21,
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
            use_cooldown: 18,
            written_book_content: 22,
        };
        let data = super::item_stack::SwingWireData {
            prototypes: unreachable_prototypes(),
            components: comps,
            use_profiles: unreachable_use_profiles(),
        };
        apply_set_equipment(&equipment_body(5, 0, 949), &mut t, &data, Some(&classes()));
        assert_eq!(t.hand_item(5, InteractionHand::MainHand), HandItem::Empty);
        // M169 — a non-living entity is inert for the saddle too: slot 7
        // (SADDLE) on a non-living id changes nothing, because
        // `apply_set_equipment` returns before it reads any slot.
        apply_set_equipment(&equipment_body(5, 7, 949), &mut t, &data, Some(&classes()));
        assert!(!t.saddled(5), "a non-living entity is not saddled");
        // …while a LIVING entity (the cow, id 2) DOES read slot 7 (M169):
        // a stack there is `isSaddled()`, and an empty stack un-saddles.
        assert!(!t.saddled(2));
        apply_set_equipment(&equipment_body(2, 7, 949), &mut t, &data, Some(&classes()));
        assert!(t.saddled(2), "a stack in slot 7 is isSaddled()");
        let mut empty = Vec::new();
        varint(2, &mut empty);
        empty.push(7);
        varint(0, &mut empty); // count 0 = the OPTIONAL codec's empty stack
        apply_set_equipment(&empty, &mut t, &data, Some(&classes()));
        assert!(!t.saddled(2), "an empty slot 7 un-saddles");
        // slot 6 (BODY) is still discarded and never touches the saddle bit.
        apply_set_equipment(&equipment_body(2, 6, 949), &mut t, &data, Some(&classes()));
        assert!(!t.saddled(2));
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
mod entity_silent_tests {
    //! `Entity.DATA_SILENT` — metadata index 4, BOOLEAN (M138a).
    //!
    //! The flag was parsed and discarded from M1 to M138a, and
    //! `EntityTableWorld::entity_silent` answered a hardcoded `false` with a
    //! comment saying it could not tell. These pin the decode, the toggle, the
    //! removal, and — the one that matters — that the SOUND ENGINE consults it,
    //! because everything above is inert if the world's answer is not read.

    use super::apply_set_entity_data;
    use rewo_world::entities::{EntityState, EntityTable};

    fn body(eid: u8, index: u8, serializer: u8, value: &[u8]) -> Vec<u8> {
        let mut b = vec![eid, index, serializer];
        b.extend_from_slice(value);
        b.push(0xFF);
        b
    }

    #[test]
    fn index_four_boolean_sets_and_clears_the_flag() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        // `Entity.java:322` seeds it false, so an entity nobody has spoken
        // about is audible — exact, not a fallback.
        assert!(!t.is_silent(1));

        apply_set_entity_data(&body(1, 4, 8, &[0x01]), &mut t, None);
        assert!(t.is_silent(1));

        // Both ways: a latch would leave an entity permanently muted after one
        // `/data merge ... {Silent:1b}` was undone.
        apply_set_entity_data(&body(1, 4, 8, &[0x00]), &mut t, None);
        assert!(!t.is_silent(1));
    }

    #[test]
    fn index_four_is_not_index_three() {
        // The two are adjacent, same class, same serializer, and differ only in
        // the index byte — which is exactly the pair a transposition would swap
        // while every decode still succeeded.
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&body(1, 4, 8, &[0x01]), &mut t, None);
        assert!(t.is_silent(1));
        assert!(
            !t.is_custom_name_visible(1),
            "index 4 must not land on index 3's flag"
        );

        let mut u = EntityTable::default();
        u.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&body(1, 3, 8, &[0x01]), &mut u, None);
        assert!(u.is_custom_name_visible(1));
        assert!(!u.is_silent(1), "and index 3 must not land on index 4's");
    }

    #[test]
    fn the_flag_dies_with_the_entity() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&body(1, 4, 8, &[0x01]), &mut t, None);
        t.remove(1);
        assert!(!t.is_silent(1), "a recycled id must not inherit it");
    }

    #[test]
    fn a_later_field_still_parses_past_index_four() {
        // Reading the boolean must consume exactly one byte, or the pose that
        // follows is lost — the same claim index 3's module makes, and it has
        // to be made again because this is a new arm of the same match.
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&[1u8, 4, 8, 0x01, 6, 20, 5, 0xFF], &mut t, None);
        assert!(t.is_silent(1));
        assert_eq!(t.pose(1), 5, "a one-byte over-read would lose the pose");
    }

    #[test]
    fn the_sound_world_reads_the_flag_rather_than_answering_false() {
        // **The load-bearing one.** Everything above passes against a decode
        // that stores the flag where nothing looks at it, which is precisely
        // the state this milestone found: parsed since M1, discarded, and the
        // consumer returning a hardcoded `false`.
        use crate::sound_engine::{EntityTableWorld, SoundWorld};
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.add(2, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        apply_set_entity_data(&body(1, 4, 8, &[0x01]), &mut t, None);

        let w = EntityTableWorld { table: &t, local: None, game_time: 0, music_volume: 1.0 };
        assert!(w.entity_silent(1), "the silenced entity");
        assert!(!w.entity_silent(2), "and only it");
        // An id the table never saw is audible, because vanilla seeds false.
        assert!(!w.entity_silent(99));
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

#[cfg(test)]
mod award_stats_tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;
    use rewo_world::stats::StatKey;

    /// `VarInt count` then `count x (statType, value, amount)`.
    fn body(entries: &[(i32, i32, i32)]) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.varint(entries.len() as i32);
        for (t, v, a) in entries {
            w.varint(*t);
            w.varint(*v);
            w.varint(*a);
        }
        w.buf
    }

    #[test]
    fn the_map_decodes_as_three_varints_per_entry() {
        let out = apply_award_stats(&body(&[(8, 1, 1200), (0, 9, 42)])).unwrap();
        assert_eq!(
            out,
            vec![(StatKey::new(8, 1), 1200), (StatKey::new(0, 9), 42)]
        );
    }

    #[test]
    fn an_empty_map_is_a_valid_packet_and_not_a_failure() {
        assert_eq!(apply_award_stats(&body(&[])), Some(Vec::new()));
    }

    /// The finding this milestone is built on: the second level of the
    /// dispatch is one VarInt **whatever the first level said**, so a stat type
    /// this client has never heard of leaves the walk in step and the entries
    /// after it decode correctly.
    #[test]
    fn an_unknown_stat_type_does_not_desync_the_walk() {
        let out = apply_award_stats(&body(&[(9999, 7, 1), (8, 2, 5)])).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (StatKey::new(9999, 7), 1));
        assert_eq!(
            out[1],
            (StatKey::new(8, 2), 5),
            "the entry *after* the unknown type is the witness - a \
             DataComponentPatch-style dispatch would have parked mid-value"
        );
    }

    /// A short read keeps what it had rather than dropping the packet: a
    /// truncated statistics list is worth more than none.
    #[test]
    fn a_truncated_body_yields_the_entries_that_did_decode() {
        let full = body(&[(8, 1, 1200), (8, 2, 42)]);
        let out = apply_award_stats(&full[..full.len() - 1]).unwrap();
        assert_eq!(out, vec![(StatKey::new(8, 1), 1200)]);
        // A body with only the count is not a panic either.
        assert_eq!(apply_award_stats(&[5u8]), Some(Vec::new()));
        assert_eq!(apply_award_stats(&[]), None, "no count at all");
    }

    /// A negative count is `readCount`'s uncovered case - vanilla only tests
    /// `count > maxSize`, so a negative one falls into `for (i = 0; i < count)`
    /// and yields an empty map. Rewo refuses it instead of allocating on it.
    #[test]
    fn a_negative_count_is_refused_rather_than_allocated_on() {
        let mut w = PacketWriter::default();
        w.varint(-1);
        assert_eq!(apply_award_stats(&w.buf), None);
    }

    /// Two var-ints and nothing else — no state id, unlike its sibling.
    #[test]
    fn the_button_click_body_is_two_varints() {
        assert_eq!(container_button_click_body(3, 1), vec![3u8, 1]);
    }

    /// The book type is an ORDINAL, and both booleans always ride along.
    #[test]
    fn the_change_settings_body_is_an_ordinal_and_two_flags() {
        // Smoker (ordinal 3), open, not filtering.
        assert_eq!(recipe_book_change_settings_body(3, true, false), vec![3u8, 1, 0]);
        // Crafting (0), shut, filtering — proving both flags are independent
        // and neither is derived from the other.
        assert_eq!(recipe_book_change_settings_body(0, false, true), vec![0u8, 0, 1]);
        assert_eq!(recipe_book_change_settings_body(0, false, false), vec![0u8, 0, 0]);
        assert_eq!(recipe_book_change_settings_body(0, true, true), vec![0u8, 1, 1]);
    }

    /// `useMaxItems` is shift-held, and it is the LAST field — after both
    /// var-ints.
    #[test]
    fn the_place_recipe_body_is_two_varints_then_shift() {
        assert_eq!(place_recipe_body(1, 42, false), vec![1u8, 42, 0]);
        assert_eq!(place_recipe_body(1, 42, true), vec![1u8, 42, 1]);
        // A recipe id past 127 takes two var-int bytes, which is what keeps the
        // trailing flag from being read as part of it.
        assert_eq!(place_recipe_body(0, 300, true), vec![0u8, 0xAC, 0x02, 1]);
        assert_eq!(container_button_click_body(0, 0), vec![0u8, 0]);
        // A two-byte body is the whole packet: anything longer means a state
        // id or a slot map crept in from `container_click`, which sits beside
        // it and names a container the same way.
        assert_eq!(container_button_click_body(127, 2).len(), 2);
        // ...and a container id past 127 is a two-byte varint, so the total
        // grows by exactly one.
        assert_eq!(container_button_click_body(128, 2).len(), 3);
    }

    /// M93n — one length-prefixed string, and the EMPTY one is meaningful.
    #[test]
    fn the_rename_body_is_one_string_and_empty_is_a_real_request() {
        // `writeUtf` is a VarInt byte length then the UTF-8 bytes.
        assert_eq!(rename_item_body("Sting"), vec![5u8, b'S', b't', b'i', b'n', b'g']);
        // The empty string is the request to CLEAR the name, so it is a
        // one-byte body and not an omission. A sender that skipped it would
        // make an anvil unable to un-name anything.
        assert_eq!(rename_item_body(""), vec![0u8]);
        // The length is in BYTES, not characters — two bytes for é.
        assert_eq!(rename_item_body("é"), vec![2u8, 0xC3, 0xA9]);
    }

    /// M93l — two optionals, each a bool then a RAW registry id.
    #[test]
    fn the_set_beacon_body_is_two_optional_raw_ids() {
        // Both present.
        assert_eq!(set_beacon_body(Some(1), Some(11)), vec![1u8, 1, 1, 11]);
        // Neither: two bare `false`s and nothing else. A codec that wrote the
        // id anyway would make this four bytes.
        assert_eq!(set_beacon_body(None, None), vec![0u8, 0]);
        // One each way, and NOT symmetric — primary first.
        assert_eq!(set_beacon_body(Some(3), None), vec![1u8, 3, 0]);
        assert_eq!(set_beacon_body(None, Some(3)), vec![0u8, 1, 3]);
        // `holderRegistry` is RAW: effect id 0 is a legal id and writes 0, not
        // 1. Under `holder`'s `id + 1` convention this would be `[1, 1]`, and
        // an off-by-one here names a real effect — the beacon would simply
        // grant the wrong one, with nothing on the wire to say so.
        assert_eq!(set_beacon_body(Some(0), None), vec![1u8, 0, 0]);
    }

    /// M93h — and the field order is the OPPOSITE of the one above.
    ///
    /// `container_button_click` writes `(containerId, button)`;
    /// `container_slot_state_changed` writes `(slotId, containerId, newState)`.
    /// Two adjacent serverbound container packets with their two var-ints
    /// transposed. Writing this one container-first produces a well-formed
    /// packet that toggles the wrong slot of the wrong menu, and nothing on
    /// the wire says so — which is why the witness uses two DIFFERENT numbers
    /// and asserts which is which, rather than a round trip.
    #[test]
    fn the_slot_state_body_is_slot_then_container_then_a_bool() {
        assert_eq!(
            container_slot_state_changed_body(4, 9, true),
            vec![4u8, 9, 1],
            "slot 4 of container 9 — not slot 9 of container 4"
        );
        assert_eq!(container_slot_state_changed_body(9, 4, true), vec![9u8, 4, 1]);
        // The bool is one byte, 0 or 1, and it is `enabled` — the opposite of
        // the value `setSlotState` stores (`isEnabled ? 0 : 1`).
        assert_eq!(container_slot_state_changed_body(0, 0, false), vec![0u8, 0, 0]);
        assert_eq!(container_slot_state_changed_body(0, 0, true), vec![0u8, 0, 1]);
        // Three bytes is the whole packet at small ids; a state id creeping in
        // from `container_click` would show here.
        assert_eq!(container_slot_state_changed_body(8, 127, true).len(), 3);
        assert_eq!(container_slot_state_changed_body(8, 128, true).len(), 4);
    }

    /// The `minecraft:mob_effect` registry is **not** network-synchronised, so
    /// nothing arriving on the wire can supply these ids (M92c).
    ///
    /// This is the witness the original bug needed and did not have. Five ids
    /// — night vision, darkness, haste, conduit power, mining fatigue — were
    /// read only from a `registry_data` branch keyed on a registry name the
    /// server never sends, so they stayed `None` for the whole session and two
    /// shipped features silently did nothing live. `lightmapshot` and
    /// `swingshot` could not see it because both are serverless and construct
    /// the effect state themselves, supplying the very ids the live path
    /// failed to obtain.
    ///
    /// It asserts the *source*, not the values: a witness on the numbers would
    /// pass just as well if they came back from a wire branch that never runs.
    #[test]
    fn the_effect_ids_come_from_the_report_because_the_wire_never_carries_them() {
        let Some(paths) = rewo_data::DataPaths::for_version("26.2") else {
            return; // no local datagen -- nothing to grade against
        };
        let m = rewo_data::mob_effects::MobEffects::load(&paths.registries_json())
            .expect("the report must carry minecraft:mob_effect");
        for name in [
            "minecraft:night_vision",
            "minecraft:darkness",
            "minecraft:haste",
            "minecraft:conduit_power",
            "minecraft:mining_fatigue",
        ] {
            assert!(
                m.id_of(name).is_some(),
                "{name} must resolve WITHOUT a server connection"
            );
        }
    }

    /// `writeEnum` is the ordinal as a VarInt, and asking for the *wrong* one
    /// respawns you instead of fetching statistics.
    #[test]
    fn the_statistics_request_is_ordinal_one() {
        assert_eq!(client_command_body(ClientCommand::RequestStats), vec![1u8]);
        assert_eq!(
            client_command_body(ClientCommand::PerformRespawn),
            vec![0u8]
        );
    }
}
