//! Session, server metadata and chat (M78): `custom_payload` (24),
//! `disguised_chat` (33), `game_rule_values` (39), `player_combat_end` (66),
//! `player_combat_enter` (67), `server_data` (86) and `store_cookie` (120).
//!
//! Seven of M78's eight packets. The eighth, `bundle_delimiter` (0), is the one
//! that changes how packets are *applied* rather than what they mean, so it
//! lives in [`crate::bundle`] — the same split [`crate::client_state`] states:
//! a packet with machinery behind it gets its own module, and the ones that are
//! a reader plus a field share one.
//!
//! All seven are `REWO_PACKET_COVERAGE.md` class **A**: each decode changes a
//! value Rewo can read back, and every property here is witnessed on the CPU
//! with no renderer.
//!
//! ## `custom_payload` — the discriminated union whose fallback consumes
//!
//! The body is an `Identifier` followed by a payload chosen **by that
//! identifier**, and 26.2's clientbound table registers exactly **one** known
//! type: `minecraft:brand`. Everything else falls through to
//! `DiscardedPayload.codec(id, 1048576)`.
//!
//! Two halves of that fallback are load-bearing, and they are the same shape as
//! M41's `DataComponentPatch` rule:
//!
//! - **An unknown identifier is not an error.** It is a `DiscardedPayload`, and
//!   `ClientCommonPacketListenerImpl.handleCustomPayload` returns *before*
//!   `ensureRunningOnSameThread` for one — it does not even log. (The
//!   `handleUnknownCustomPayload` warn in `ClientPacketListener` is reachable
//!   only for a *registered* type with no specific handler, and 26.2 registers
//!   only `BrandPayload`, which is handled. It is dead code on this path.)
//! - **The fallback consumes the remainder.** `DiscardedPayload`'s reader takes
//!   `buf.readableBytes()` and skips exactly that many, so the *packet* is
//!   fully consumed even though the payload is thrown away. It throws only when
//!   the remainder exceeds `MAX_PAYLOAD_SIZE` = 1 048 576.
//!
//! **The brand arrives during configuration, not play.**
//! `ServerConfigurationPacketListenerImpl` sends
//! `new ClientboundCustomPayloadPacket(new BrandPayload(getServerModName()))`
//! as part of its opening burst, and `serverBrand` lives on the *common*
//! listener that both states extend. Resolving only the play copy would have
//! looked like it worked against every server that never sends a second one —
//! which is all of them. This is M69's `update_tags` lesson repeating: a packet
//! that exists in two connection states needs both ids, and a play-only survey
//! names the one a vanilla server does **not** send.
//!
//! ## `disguised_chat` — what is decoded, and what cannot be rendered
//!
//! `Component` + `ChatType.Bound`, and the `Bound` is three fields:
//!
//! | field | codec | wire |
//! |---|---|---|
//! | `chatType` | `ByteBufCodecs.**holder**(Registries.CHAT_TYPE, …)` | VarInt: `0` = an inline `ChatType` follows, else `id - 1` |
//! | `name` | `ComponentSerialization.TRUSTED_STREAM_CODEC` | one NBT tag |
//! | `targetName` | `TRUSTED_OPTIONAL_STREAM_CODEC` | a bool, then one NBT tag when set |
//!
//! The first is `holder`, **not** `holderRegistry`. M16 records the
//! distinction at the dimension holder and M65 found two enum conventions one
//! field apart; here the two live in the same three-field record, because the
//! *inline* branch is real — a `ChatType` can be sent by value, and then two
//! whole `ChatTypeDecoration`s follow (a string, a VarInt-counted list of
//! VarInt parameters, and an NBT `Style`). A `holderRegistry` reading would
//! decode `0` as chat type 0 and then read the decoration bytes as the sender's
//! name, and every subsequent field of the packet with it.
//!
//! **The rendered line is the *decorated* message**, not the raw one:
//! `ChatListener.handleDisguisedChatMessage` calls `boundChatType.decorate`,
//! which is `Component.translatable(decoration.translationKey, [sender,
//! content])`. Decorating needs the **`minecraft:chat_type` registry's
//! contents**, which arrive in the configuration `registry_data` Rewo does not
//! parse for that registry, plus the language table `rewo-net` has no access
//! to. So Rewo appends the message's plain text — exactly the fidelity the
//! `player_chat` arm already has, which also pushes the raw message rather than
//! the decoration. The whole `Bound` is still decoded, because a short read
//! desynchronises nothing here (the packet ends) but a *wrong* read would
//! silently accept a malformed body as valid. The decoded `Bound` is returned
//! so a witness can grade it and a future decoration has its inputs.
//!
//! ## `server_data` — decoded, deliberately not validated
//!
//! `Component motd` (a **context-free** trusted NBT tag — no registry access)
//! then `Optional<byte[]> iconBytes` with the unbounded `BYTE_ARRAY`.
//!
//! Vanilla's handler is gated on `this.serverData != null`, i.e. it only
//! records when the session was started from the multiplayer server list, and
//! it runs the icon through `ServerData.validateIcon` (a PNG parse capped at
//! 1024×1024, where an invalid icon leaves the previous one alone because
//! `Optional.map` returning `null` yields `empty()`). Rewo has no server list
//! and no PNG decoder in `rewo-net`, so it records unconditionally and stores
//! the bytes verbatim. Both divergences are stated rather than hidden: this is
//! a decode, and the icon is bytes.
//!
//! ## `game_rule_values` — a map, and there is no vanilla store
//!
//! `ByteBufCodecs.map(HashMap::new, ResourceKey.streamCodec(GAME_RULE),
//! STRING_UTF8)` — a VarInt count then that many `Identifier` / string pairs.
//! `readCount`'s cap is `Integer.MAX_VALUE`, so the only real bound is the
//! buffer.
//!
//! `handleGameRuleValues` hands the map to an `InWorldGameRulesScreen` **if one
//! is open** and keeps nothing otherwise, and the screen itself ignores every
//! packet after the first (`if (!this.receivedServerValues)`). There is
//! therefore no vanilla store to be faithful to. Rewo keeps the last map
//! wholesale — **replacement, not merge** — because the packet is a full
//! snapshot of the server's rules and merging would strand a rule the server
//! stopped sending.
//!
//! ## The two vestigial packets, and why the witness is byte consumption
//!
//! `handlePlayerCombatEnd` and `handlePlayerCombatEnter` are both `{}` — empty
//! method bodies. They are the residue of the 1.16-era combat-timer HUD.
//!
//! **They are not the same on the wire, and the coverage table used to imply
//! they were.** `player_combat_enter` really is empty: its codec is
//! `StreamCodec.unit(INSTANCE)`, zero bytes. `player_combat_end` carries a
//! **VarInt `duration`** — `CombatTracker.getCombatDuration()`, in ticks.
//!
//! So the honest implementation is: consume the body *exactly*, and do nothing
//! with it. Rewo stores no combat duration, because vanilla stores none —
//! inventing a field would be a divergence dressed up as decode-and-state. What
//! is left to prove is the **reader position**, which is why both readers
//! return the number of bytes they used and the gate asserts it equals the body
//! length. That matters more here than anywhere else in this module: a reader
//! that consumed one byte too few would leave the packet's tail unread, and a
//! reader that consumed one too many would read into nothing — and with no
//! observable state either way, byte consumption is the *only* thing an
//! implementation can be graded on.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/network/protocol/common/ClientboundCustomPayloadPacket.java`
//! - `net/minecraft/network/protocol/common/custom/{CustomPacketPayload,
//!   BrandPayload,DiscardedPayload}.java`
//! - `net/minecraft/network/protocol/game/ClientboundDisguisedChatPacket.java`
//! - `net/minecraft/network/chat/{ChatType,ChatTypeDecoration,
//!   ComponentSerialization}.java`
//! - `net/minecraft/network/protocol/game/ClientboundGameRuleValuesPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundPlayerCombatEndPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundPlayerCombatEnterPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundServerDataPacket.java`
//! - `net/minecraft/network/protocol/common/ClientboundStoreCookiePacket.java`
//! - `net/minecraft/network/protocol/cookie/{ClientboundCookieRequestPacket,
//!   ServerboundCookieResponsePacket}.java`
//! - `net/minecraft/network/codec/ByteBufCodecs.java` — `holder`, `map`,
//!   `optional`, `byteArray`, `BYTE_ARRAY`, `readCount`
//! - `net/minecraft/network/FriendlyByteBuf.java` — `readByteArray`
//! - `net/minecraft/client/multiplayer/ClientCommonPacketListenerImpl.java` —
//!   `handleCustomPayload`, `handleRequestCookie`, `handleStoreCookie`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleDisguisedChat`, `handleGameRuleValues`, `handlePlayerCombatEnd`,
//!   `handlePlayerCombatEnter`, `handleServerData`
//! - `net/minecraft/client/multiplayer/chat/ChatListener.java` —
//!   `handleDisguisedChatMessage`
//! - `net/minecraft/client/multiplayer/ServerData.java` — `validateIcon`
//! - `net/minecraft/server/network/ServerConfigurationPacketListenerImpl.java`
//!   — where the brand is actually sent

use std::collections::BTreeMap;

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `ClientboundCustomPayloadPacket.MAX_PAYLOAD_SIZE`.
pub const MAX_CUSTOM_PAYLOAD_SIZE: usize = 1_048_576;

/// `ClientboundStoreCookiePacket.MAX_PAYLOAD_SIZE`, and the same constant the
/// serverbound `cookie_response` writes through.
pub const MAX_COOKIE_PAYLOAD_SIZE: usize = 5120;

/// `BrandPayload.TYPE` — `CustomPacketPayload.createType("brand")` resolves
/// through `Identifier.withDefaultNamespace`, so the wire form is namespaced.
pub const BRAND_PAYLOAD_ID: &str = "minecraft:brand";

/// One decoded `custom_payload` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustomPayload {
    /// `minecraft:brand` — the only type 26.2 registers clientbound.
    Brand(String),
    /// Anything else: `DiscardedPayload`. The identifier is kept because
    /// vanilla's record keeps it; the bytes are not, because vanilla skips
    /// them.
    Discarded { id: String, len: usize },
}

/// `server_data`'s two fields. The MOTD is flattened to plain text the way
/// every other `Component` in `rewo-net` is; the icon is bytes.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ServerMetadata {
    pub motd: String,
    pub icon: Option<Vec<u8>>,
}

/// A decoded `ChatType.Bound`. Nothing renders it yet — see the module docs on
/// why decoration is out of reach — but it is decoded in full so the body is
/// validated rather than assumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatTypeBound {
    /// The `holder` VarInt as it appeared: `None` for the inline form
    /// (wire `0`), else the registry id (wire minus one).
    pub chat_type: Option<i32>,
    /// The sender's display name, flattened.
    pub name: String,
    /// `/msg`'s recipient, when the decoration takes one.
    pub target_name: Option<String>,
}

/// One decoded `disguised_chat` body.
#[derive(Clone, Debug, PartialEq)]
pub struct DisguisedChat {
    /// The component, **unflattened** (M125) — resolving a `translate` in it
    /// needs the language table, which is the session's and not the wire's.
    pub message: rewo_proto::nbt::Nbt,
    pub bound: ChatTypeBound,
}

/// The session-level facts M78 decodes, plus the cookie jar.
///
/// One struct because one object carries them in vanilla too: `serverBrand` and
/// `serverCookies` are both fields of `ClientCommonPacketListenerImpl`, which
/// is what configuration and play share. Rewo's equivalent seam is
/// `Connection::into_play`, which moves this across exactly as it moves the
/// tags M69 landed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionState {
    /// `ClientCommonPacketListenerImpl.serverBrand`.
    pub server_brand: Option<String>,
    /// `ServerData.motd` / `iconBytes`, from `server_data`.
    pub server_data: Option<ServerMetadata>,
    /// The last `game_rule_values` map, whole. Ordered so a witness (and a
    /// future UI) reads the same sequence twice.
    pub game_rules: BTreeMap<String, String>,
    /// `ClientCommonPacketListenerImpl.serverLinks` (M85).
    ///
    /// Beside the brand and the cookie jar because it is beside them in the
    /// jar too — the same class, and for the same reason: it arrives in
    /// *either* connection state and belongs to the connection rather than to
    /// the world. `handleServerLinks` **assigns**, so a later packet replaces
    /// the list wholesale and an empty one retracts it.
    ///
    /// See [`crate::server_links`] for why the disconnect screen needs this to
    /// outlive the session that received it.
    pub server_links: crate::server_links::ServerLinks,
    /// `ClientCommonPacketListenerImpl.serverCookies`.
    cookies: BTreeMap<String, Vec<u8>>,
    /// Disguised-chat lines awaiting the session's chat log. Drained by
    /// [`Self::take_chat`] rather than written directly, so this module needs
    /// no reference to `PlaySession`. Components rather than strings, because
    /// the resolution that turns one into text is `PlaySession`'s (M125).
    chat: Vec<rewo_proto::nbt::Nbt>,
}

impl SessionState {
    /// `serverCookies.put` — a later `store_cookie` for the same key replaces
    /// the earlier payload, because that is what `Map.put` does.
    pub fn store_cookie(&mut self, key: String, payload: Vec<u8>) {
        self.cookies.insert(key, payload);
    }

    /// `serverCookies.get(key)` — the value `handleRequestCookie` puts straight
    /// into its reply. `None` is vanilla's `null`, which the serverbound packet
    /// writes as a present-flag of `false`.
    pub fn cookie(&self, key: &str) -> Option<&[u8]> {
        self.cookies.get(key).map(Vec::as_slice)
    }

    /// How many cookies are stored. For witnesses and logging.
    pub fn cookie_count(&self) -> usize {
        self.cookies.len()
    }

    /// Take the disguised-chat lines decoded since the last call.
    pub fn take_chat(&mut self) -> Vec<rewo_proto::nbt::Nbt> {
        std::mem::take(&mut self.chat)
    }
}

/// Which of the seven a body is. As with the view area's radius pair and the
/// ticking pair, several of these bodies are indistinguishable from one another
/// (`player_combat_enter` is empty; `custom_payload` and `store_cookie` both
/// open with an `Identifier`), so the packet id is the whole discriminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPacket {
    CustomPayload,
    DisguisedChat,
    GameRuleValues,
    PlayerCombatEnd,
    PlayerCombatEnter,
    ServerData,
    /// `ClientboundServerLinksPacket` (M85) — a `common` packet like
    /// `custom_payload` and `store_cookie`, so it too exists in both
    /// connection states and reaches this one [`apply`] from both.
    ServerLinks,
    StoreCookie,
}

/// The seven resolved ids, lifted out of [`crate::ids::Ids`] so [`kind_for_id`]
/// and [`apply`] can be driven without building a full `Ids`.
///
/// `custom_payload` and `store_cookie` are `common` packets and exist in the
/// configuration state too; the configuration ids are routed through the same
/// [`apply`] by [`crate::Connection::run_configuration`], which is why this
/// table names roles rather than a state.
#[derive(Clone, Copy, Debug)]
pub struct SessionIds {
    pub custom_payload: i32,
    pub disguised_chat: i32,
    pub game_rule_values: i32,
    pub player_combat_end: i32,
    pub player_combat_enter: i32,
    pub server_data: i32,
    pub server_links: i32,
    pub store_cookie: i32,
}

/// Map an incoming packet id onto a kind, or `None` if it is none of them.
pub fn kind_for_id(id: i32, ids: SessionIds) -> Option<SessionPacket> {
    if id == ids.custom_payload {
        Some(SessionPacket::CustomPayload)
    } else if id == ids.disguised_chat {
        Some(SessionPacket::DisguisedChat)
    } else if id == ids.game_rule_values {
        Some(SessionPacket::GameRuleValues)
    } else if id == ids.player_combat_end {
        Some(SessionPacket::PlayerCombatEnd)
    } else if id == ids.player_combat_enter {
        Some(SessionPacket::PlayerCombatEnter)
    } else if id == ids.server_data {
        Some(SessionPacket::ServerData)
    } else if id == ids.server_links {
        Some(SessionPacket::ServerLinks)
    } else if id == ids.store_cookie {
        Some(SessionPacket::StoreCookie)
    } else {
        None
    }
}

/// Decode one body and apply it. Returns whether the body decoded.
///
/// A malformed body is reported and dropped whole rather than half-applied,
/// the rule [`crate::ticking::apply`] and [`crate::teams`] already follow: each
/// reader below reads everything it needs before anything is assigned.
pub fn apply(kind: SessionPacket, body: &[u8], state: &mut SessionState) -> bool {
    match kind {
        SessionPacket::CustomPayload => match read_custom_payload(body) {
            Ok(CustomPayload::Brand(brand)) => {
                log::debug!("net: server brand {brand:?}");
                state.server_brand = Some(brand);
                true
            }
            // Vanilla's `handleCustomPayload` returns before doing anything at
            // all for a `DiscardedPayload` — not even a log line. `trace` here
            // rather than `debug` for the same reason: on a modded server this
            // fires dozens of times a join.
            Ok(CustomPayload::Discarded { id, len }) => {
                log::trace!("net: custom_payload {id} discarded ({len} bytes)");
                true
            }
            Err(err) => {
                log::debug!("net: custom_payload decode: {err}");
                false
            }
        },
        SessionPacket::DisguisedChat => match read_disguised_chat(body) {
            Ok(chat) => {
                // The "is it empty" test moved to `PlaySession`, because it
                // is a test on the *rendered text* and rendering the text now
                // needs the language table (M125). Vanilla has no such test
                // at all on either chat handler — `handleDisguisedChatMessage`
                // decorates and adds unconditionally — so this is Rewo's own
                // guard, preserved where it was rather than quietly dropped in
                // a milestone about something else.
                state.chat.push(chat.message);
                true
            }
            Err(err) => {
                log::debug!("net: disguised_chat decode: {err}");
                false
            }
        },
        SessionPacket::GameRuleValues => match read_game_rule_values(body) {
            Ok(rules) => {
                log::debug!("net: {} game rule(s)", rules.len());
                state.game_rules = rules;
                true
            }
            Err(err) => {
                log::debug!("net: game_rule_values decode: {err}");
                false
            }
        },
        // Both handlers are empty methods in vanilla. Consume the body exactly
        // and change nothing — see the module docs.
        SessionPacket::PlayerCombatEnd => match read_player_combat_end(body) {
            Ok((duration, _)) => {
                log::trace!("net: player_combat_end duration={duration}");
                true
            }
            Err(err) => {
                log::debug!("net: player_combat_end decode: {err}");
                false
            }
        },
        SessionPacket::PlayerCombatEnter => {
            let consumed = read_player_combat_enter(body);
            log::trace!("net: player_combat_enter ({consumed} bytes)");
            true
        }
        SessionPacket::ServerData => match read_server_data(body) {
            Ok(data) => {
                log::debug!(
                    "net: server_data motd={:?} icon={} bytes",
                    data.motd,
                    data.icon.as_ref().map_or(0, Vec::len)
                );
                state.server_data = Some(data);
                true
            }
            Err(err) => {
                log::debug!("net: server_data decode: {err}");
                false
            }
        },
        // M85. Vanilla's `handleServerLinks` validates **per entry** and keeps
        // the survivors, so a decode failure of the *list* is the only thing
        // that can leave the previous links in place — one bad URL cannot.
        SessionPacket::ServerLinks => match crate::server_links::read_server_links(body) {
            Ok(entries) => {
                let (links, dropped) = crate::server_links::trust(entries);
                if dropped > 0 {
                    // `LOGGER.warn("Received invalid link for type {}:{}")`,
                    // once per bad entry in vanilla; once per packet here,
                    // because the reader has already dropped the offenders.
                    log::warn!("net: {dropped} server link(s) failed URL validation and were dropped");
                }
                log::debug!(
                    "net: {} server link(s): {:?}",
                    links.len(),
                    links
                        .entries()
                        .iter()
                        .map(|e| (&e.label, &e.link))
                        .collect::<Vec<_>>()
                );
                state.server_links = links;
                true
            }
            Err(err) => {
                log::debug!("net: server_links decode: {err}");
                false
            }
        },
        SessionPacket::StoreCookie => match read_store_cookie(body) {
            Ok((key, payload)) => {
                log::debug!("net: store_cookie {key} ({} bytes)", payload.len());
                state.store_cookie(key, payload);
                true
            }
            Err(err) => {
                log::debug!("net: store_cookie decode: {err}");
                false
            }
        },
    }
}

/// `ClientboundCustomPayloadPacket` — an `Identifier`, then the payload chosen
/// by it. Unknown identifiers are *discarded*, not rejected, and the remainder
/// is consumed. See the module docs.
pub fn read_custom_payload(body: &[u8]) -> Result<CustomPayload> {
    let mut r = PacketReader::new(body);
    let id = r.identifier()?;
    if id == BRAND_PAYLOAD_ID {
        // `BrandPayload`'s reader is `input.readUtf()`, whose default cap is
        // `FriendlyByteBuf.MAX_STRING_LENGTH` = 32767.
        return Ok(CustomPayload::Brand(r.string(32767)?));
    }
    // `DiscardedPayload.codec(id, 1048576)`: `int length = buf.readableBytes()`
    // then either skip it or throw. The bound is on what is *left*, so a huge
    // packet is the error and a huge declared length does not exist — there is
    // no length prefix here at all.
    let len = r.remaining();
    if len > MAX_CUSTOM_PAYLOAD_SIZE {
        return Err(rewo_proto::ProtoError::LengthOutOfRange {
            what: "custom payload",
            len: len as i64,
            max: MAX_CUSTOM_PAYLOAD_SIZE,
        });
    }
    r.skip(len)?;
    Ok(CustomPayload::Discarded { id, len })
}

/// `ClientboundDisguisedChatPacket` — a trusted `Component` then a
/// `ChatType.Bound`.
pub fn read_disguised_chat(body: &[u8]) -> Result<DisguisedChat> {
    let mut r = PacketReader::new(body);
    let message = r.nbt()?;
    let bound = read_chat_type_bound(&mut r)?;
    Ok(DisguisedChat { message, bound })
}

/// `ChatType.Bound.STREAM_CODEC` — the `holder` VarInt, a trusted `Component`,
/// then an optional trusted `Component`.
pub fn read_chat_type_bound(r: &mut PacketReader<'_>) -> Result<ChatTypeBound> {
    let raw = r.varint()?;
    let chat_type = if raw == 0 {
        // `Holder.direct(directCodec.decode(input))` — the inline form. Two
        // whole `ChatTypeDecoration`s follow and must be walked, or the
        // sender's name is read out of the middle of the first one.
        skip_chat_type_decoration(r)?;
        skip_chat_type_decoration(r)?;
        None
    } else {
        // `byIdOrThrow(id - 1)`. Rewo does not resolve the id (the
        // `minecraft:chat_type` registry is not parsed), so it is kept raw.
        Some(raw - 1)
    };
    let name = r.nbt()?.to_plain_text();
    let target_name = if r.bool()? {
        Some(r.nbt()?.to_plain_text())
    } else {
        None
    };
    Ok(ChatTypeBound {
        chat_type,
        name,
        target_name,
    })
}

/// `ChatTypeDecoration.STREAM_CODEC` — a string, a VarInt-counted list of
/// VarInt parameters, and an NBT `Style`. Only reached by the inline branch
/// above, and only walked (nothing renders a decoration).
fn skip_chat_type_decoration(r: &mut PacketReader<'_>) -> Result<()> {
    let _translation_key = r.string(32767)?;
    // `Parameter.STREAM_CODEC.apply(ByteBufCodecs.list())` — an unbounded
    // `collection`, so the count is guarded by the buffer rather than a
    // constant. One VarInt per element, hence a minimum of one byte each.
    let n = r.count("chat type parameters", 1)?;
    for _ in 0..n {
        let _parameter = r.varint()?;
    }
    // `Style.Serializer.TRUSTED_STREAM_CODEC` is `fromCodecWithRegistriesTrusted`
    // — one NBT tag, exactly like a `Component`.
    let _style = r.nbt()?;
    Ok(())
}

/// `ClientboundGameRuleValuesPacket` — a VarInt count then that many
/// `Identifier` / string pairs.
pub fn read_game_rule_values(body: &[u8]) -> Result<BTreeMap<String, String>> {
    let mut r = PacketReader::new(body);
    // A key is at least one byte of length prefix and a value at least one, so
    // two bytes per entry is the true floor and the guard is `count`'s.
    let n = r.count("game rules", 2)?;
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let key = r.identifier()?;
        let value = r.string(32767)?;
        out.insert(key, value);
    }
    Ok(out)
}

/// `ClientboundPlayerCombatEndPacket` — one VarInt `duration`, in ticks.
///
/// Returns `(duration, bytes consumed)`. The second half is the point: the
/// handler is empty, so consumption is the only property an implementation can
/// be graded on. See the module docs.
pub fn read_player_combat_end(body: &[u8]) -> Result<(i32, usize)> {
    let mut r = PacketReader::new(body);
    let duration = r.varint()?;
    Ok((duration, r.offset()))
}

/// `ClientboundPlayerCombatEnterPacket` — `StreamCodec.unit(INSTANCE)`, which
/// reads **nothing**. Returns the bytes consumed, which is always `0`.
///
/// Infallible on purpose: `unit` cannot fail, so a body that is somehow
/// non-empty is not this packet's problem. It is a whole-packet read, and the
/// framing already told us where the packet ended.
pub fn read_player_combat_enter(_body: &[u8]) -> usize {
    0
}

/// `ClientboundServerDataPacket` — a context-free trusted `Component` then an
/// optional unbounded byte array.
pub fn read_server_data(body: &[u8]) -> Result<ServerMetadata> {
    let mut r = PacketReader::new(body);
    let motd = r.nbt()?.to_plain_text();
    // `ByteBufCodecs.BYTE_ARRAY.apply(optional)`: a bool, then
    // `FriendlyByteBuf.readByteArray(input)` = `readByteArray(input,
    // input.readableBytes())`. The cap really is the remainder, so a declared
    // length longer than the packet is the error and nothing else is.
    let icon = if r.bool()? {
        Some(r.byte_array(r.remaining())?.to_vec())
    } else {
        None
    };
    Ok(ServerMetadata { motd, icon })
}

/// `ClientboundStoreCookiePacket` — an `Identifier` then
/// `byteArray(MAX_PAYLOAD_SIZE)`.
///
/// The size limit is an **error**, not a truncation:
/// `FriendlyByteBuf.readByteArray(input, maxSize)` throws a `DecoderException`
/// when the declared length exceeds it, before a single byte is copied. A
/// client that truncated instead would store a cookie it would later hand back
/// to a server that never issued it.
pub fn read_store_cookie(body: &[u8]) -> Result<(String, Vec<u8>)> {
    let mut r = PacketReader::new(body);
    let key = r.identifier()?;
    let payload = r.byte_array(MAX_COOKIE_PAYLOAD_SIZE)?.to_vec();
    Ok((key, payload))
}

/// `ServerboundCookieResponsePacket` — an `Identifier` then
/// `writeNullable(payload, PAYLOAD_STREAM_CODEC)`.
///
/// This is the half of the cookie loop M78 actually changes. Rewo already
/// answered `cookie_request`; it answered with `payload = None` unconditionally,
/// because nothing ever stored one. `handleRequestCookie` is
/// `send(new ServerboundCookieResponsePacket(key, serverCookies.get(key)))`, so
/// the reply is a straight jar lookup and `null` is the miss.
///
/// Lives here rather than inline at the two call sites so the writer and the
/// store are graded together — the reply is the only externally visible
/// consequence of a `store_cookie`, and a jar that filled correctly behind a
/// reply that still wrote `false` would look exactly like the pre-M78 client.
pub fn write_cookie_response(
    packet_id: i32,
    key: &str,
    payload: Option<&[u8]>,
) -> rewo_proto::writer::PacketWriter {
    let mut w = rewo_proto::writer::PacketWriter::packet(packet_id);
    w.string(key);
    match payload {
        Some(bytes) => {
            w.bool(true).varint(bytes.len() as i32);
            w.raw(bytes);
        }
        None => {
            w.bool(false);
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> SessionIds {
        SessionIds {
            custom_payload: 24,
            disguised_chat: 33,
            game_rule_values: 39,
            player_combat_end: 66,
            player_combat_enter: 67,
            server_data: 86,
            server_links: 137,
            store_cookie: 120,
        }
    }

    /// A VarInt-length-prefixed UTF-8 string, as every reader above expects.
    fn wire_string(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        rewo_proto::varint::write_varint(&mut out, s.len() as i32);
        out.extend_from_slice(s.as_bytes());
        out
    }

    /// A network-NBT string tag — the shape `ComponentSerialization`'s
    /// `fromCodecTrusted` produces for a plain-text `Component`.
    fn wire_component(text: &str) -> Vec<u8> {
        let mut out = vec![0x08]; // TAG_String, unnamed root
        out.extend_from_slice(&(text.len() as u16).to_be_bytes());
        out.extend_from_slice(text.as_bytes());
        out
    }

    /// An empty NBT compound — a `Style` with nothing set.
    fn wire_empty_compound() -> Vec<u8> {
        vec![0x0a, 0x00]
    }

    /// `custom_payload` carrying `minecraft:brand`.
    ///
    /// MUTATION: compare the identifier against the bare `"brand"` rather than
    /// the namespaced form. `CustomPacketPayload.createType` runs the name
    /// through `Identifier.withDefaultNamespace`, so the wire form is
    /// `minecraft:brand` — and a bare-name comparison would send every real
    /// brand down the discard path, leaving `server_brand` permanently `None`
    /// with no error anywhere.
    #[test]
    fn a_brand_payload_decodes_to_its_string() {
        let mut body = wire_string(BRAND_PAYLOAD_ID);
        body.extend(wire_string("Paper"));
        assert_eq!(
            read_custom_payload(&body).unwrap(),
            CustomPayload::Brand("Paper".into())
        );
        // And the bare name is *not* the brand payload — it is an unknown id.
        let mut bare = wire_string("brand");
        bare.extend(wire_string("Paper"));
        assert!(matches!(
            read_custom_payload(&bare).unwrap(),
            CustomPayload::Discarded { .. }
        ));
    }

    /// An unknown identifier is discarded rather than rejected, and the reader
    /// consumes the whole remainder.
    ///
    /// MUTATION: returning an error for an unrecognised identifier. That is the
    /// natural instinct — it is what M41 records for an untranscribed
    /// `DataComponentPatch` component — and it is wrong here, because
    /// `custom_payload` *has* a fallback codec and a plugin message is the
    /// commonest packet on a modded server. The sample carries a body that
    /// decodes as nothing else, so a reader that stopped at the identifier
    /// would leave five bytes unread.
    #[test]
    fn an_unknown_payload_is_discarded_and_fully_consumed() {
        let mut body = wire_string("mymod:hello");
        body.extend_from_slice(&[0xff, 0x00, 0x13, 0x37, 0x42]);
        assert_eq!(
            read_custom_payload(&body).unwrap(),
            CustomPayload::Discarded {
                id: "mymod:hello".into(),
                len: 5,
            }
        );
        // An unknown payload with an empty tail is still a valid packet.
        assert_eq!(
            read_custom_payload(&wire_string("mymod:ping")).unwrap(),
            CustomPayload::Discarded {
                id: "mymod:ping".into(),
                len: 0,
            }
        );
    }

    /// `store_cookie`'s payload limit is an error, and it sits **on** the
    /// bound so an off-by-one in the comparison is visible.
    ///
    /// MUTATION: `>=` instead of `>` in the size check (which would reject a
    /// legal 5120-byte cookie), or clamping the length instead of erroring
    /// (which would store a truncated cookie and hand it back to a server that
    /// never issued it).
    #[test]
    fn a_cookie_payload_of_exactly_the_limit_is_legal_and_one_more_is_not() {
        let at_limit = {
            let mut b = wire_string("mynet:session");
            rewo_proto::varint::write_varint(&mut b, MAX_COOKIE_PAYLOAD_SIZE as i32);
            b.extend(std::iter::repeat_n(0xabu8, MAX_COOKIE_PAYLOAD_SIZE));
            b
        };
        let (key, payload) = read_store_cookie(&at_limit).unwrap();
        assert_eq!(key, "mynet:session");
        assert_eq!(payload.len(), MAX_COOKIE_PAYLOAD_SIZE);

        let over = {
            let mut b = wire_string("mynet:session");
            rewo_proto::varint::write_varint(&mut b, MAX_COOKIE_PAYLOAD_SIZE as i32 + 1);
            b.extend(std::iter::repeat_n(0xabu8, MAX_COOKIE_PAYLOAD_SIZE + 1));
            b
        };
        assert!(read_store_cookie(&over).is_err());
    }

    /// Storing a cookie changes what a later `cookie_request` answers with, and
    /// a second store for the same key **replaces** the first.
    ///
    /// MUTATION: `entry().or_insert()` instead of `insert` — a first-write-wins
    /// store passes every "is the cookie readable" check and silently pins a
    /// session token to whatever the first backend issued, which is exactly the
    /// state a `transfer` exists to rewrite.
    #[test]
    fn a_stored_cookie_is_what_the_reply_reads_and_a_restore_replaces_it() {
        let mut s = SessionState::default();
        assert_eq!(s.cookie("mynet:session"), None);
        s.store_cookie("mynet:session".into(), vec![1, 2, 3]);
        assert_eq!(s.cookie("mynet:session"), Some(&[1u8, 2, 3][..]));
        s.store_cookie("mynet:session".into(), vec![9]);
        assert_eq!(s.cookie("mynet:session"), Some(&[9u8][..]));
        assert_eq!(s.cookie_count(), 1);
        // An unrelated key is still absent — the store is keyed, not global.
        assert_eq!(s.cookie("mynet:other"), None);
    }

    /// `server_data` is a component then an **optional** byte array, in that
    /// order, and the absent case is one byte.
    ///
    /// MUTATION: reading the icon unconditionally (dropping the present-flag).
    /// The sample's absent case is followed by nothing, so an unconditional
    /// read errors — and the present case's icon is chosen so a flag read as
    /// part of the length would give a different array.
    #[test]
    fn server_data_is_a_motd_then_an_optional_icon() {
        let mut absent = wire_component("A Minecraft Server");
        absent.push(0x00);
        let d = read_server_data(&absent).unwrap();
        assert_eq!(d.motd, "A Minecraft Server");
        assert_eq!(d.icon, None);

        let mut present = wire_component("hi");
        present.push(0x01);
        present.push(0x03);
        present.extend_from_slice(&[0x89, 0x50, 0x4e]);
        let d = read_server_data(&present).unwrap();
        assert_eq!(d.motd, "hi");
        assert_eq!(d.icon.as_deref(), Some(&[0x89u8, 0x50, 0x4e][..]));

        // The flag is required: a body that stops after the MOTD is short.
        assert!(read_server_data(&wire_component("hi")).is_err());
    }

    /// `game_rule_values` is a counted map of identifier → string, and the map
    /// **replaces** rather than merges.
    ///
    /// MUTATION: `state.game_rules.extend(rules)` instead of assignment. A
    /// merge passes every "did the new rule arrive" check and leaves a rule the
    /// server stopped sending permanently visible.
    #[test]
    fn game_rules_decode_as_a_map_and_a_second_packet_replaces_the_first() {
        let mut body = vec![0x02];
        body.extend(wire_string("minecraft:keep_inventory"));
        body.extend(wire_string("true"));
        body.extend(wire_string("minecraft:mob_griefing"));
        body.extend(wire_string("false"));
        let rules = read_game_rule_values(&body).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules["minecraft:keep_inventory"], "true");
        assert_eq!(rules["minecraft:mob_griefing"], "false");

        let mut s = SessionState::default();
        assert!(apply(SessionPacket::GameRuleValues, &body, &mut s));
        assert_eq!(s.game_rules.len(), 2);

        let mut second = vec![0x01];
        second.extend(wire_string("minecraft:do_fire_tick"));
        second.extend(wire_string("false"));
        assert!(apply(SessionPacket::GameRuleValues, &second, &mut s));
        assert_eq!(s.game_rules.len(), 1, "the map replaces, it does not merge");
        assert!(!s.game_rules.contains_key("minecraft:keep_inventory"));

        // An empty map is a legal packet, not a decode failure.
        assert!(read_game_rule_values(&[0x00]).unwrap().is_empty());
    }

    /// The two vestigial packets consume exactly their bodies — the only
    /// property either of them has.
    ///
    /// MUTATION: reading one byte too many or too few. `player_combat_end`'s
    /// duration is written as a two-byte VarInt precisely so a one-byte reader
    /// and a fixed-`i32` reader both disagree with it; `player_combat_enter`
    /// reads nothing at all, so any read is one byte too many.
    #[test]
    fn the_vestigial_packets_consume_exactly_their_bodies() {
        // 300 = 0xAC 0x02 — two bytes, so neither a single byte nor four.
        let body = [0xac, 0x02];
        let (duration, consumed) = read_player_combat_end(&body).unwrap();
        assert_eq!(duration, 300);
        assert_eq!(consumed, body.len(), "the whole body, and no more");

        // A one-byte VarInt body is also whole.
        let (duration, consumed) = read_player_combat_end(&[0x05]).unwrap();
        assert_eq!((duration, consumed), (5, 1));
        // An empty body is not: the VarInt is required.
        assert!(read_player_combat_end(&[]).is_err());

        // `StreamCodec.unit` reads nothing.
        assert_eq!(read_player_combat_enter(&[]), 0);
    }

    /// `ChatType.Bound`'s holder is `holder`, not `holderRegistry`: the wire
    /// number is `id + 1` and `0` means an inline `ChatType` follows.
    ///
    /// MUTATION: reading the VarInt as a raw registry id. The sample's inline
    /// case is what bites — a raw reading takes `0` as chat type 0 and then
    /// reads the first decoration's translation key as the sender's name, so
    /// the sender comes out as `"chat.type.text"` and the rest of the packet
    /// is garbage. The referenced case is graded too, because `id - 1` is the
    /// half a raw reading gets off by one.
    #[test]
    fn a_chat_type_bound_reads_the_holder_convention() {
        // Referenced: wire 4 → registry id 3.
        let mut referenced = vec![0x04];
        referenced.extend(wire_component("Notch"));
        referenced.push(0x00);
        let mut r = PacketReader::new(&referenced);
        let bound = read_chat_type_bound(&mut r).unwrap();
        assert_eq!(bound.chat_type, Some(3));
        assert_eq!(bound.name, "Notch");
        assert_eq!(bound.target_name, None);
        assert_eq!(r.remaining(), 0, "the whole body is consumed");

        // Inline: wire 0, then two whole decorations, then the name.
        let decoration = |key: &str, params: &[u8]| {
            let mut d = wire_string(key);
            d.push(params.len() as u8);
            d.extend_from_slice(params);
            d.extend(wire_empty_compound());
            d
        };
        let mut inline = vec![0x00];
        inline.extend(decoration("chat.type.text", &[0, 2]));
        inline.extend(decoration("chat.type.text.narrate", &[0, 2]));
        inline.extend(wire_component("Notch"));
        inline.push(0x01);
        inline.extend(wire_component("Herobrine"));
        let mut r = PacketReader::new(&inline);
        let bound = read_chat_type_bound(&mut r).unwrap();
        assert_eq!(bound.chat_type, None, "wire 0 is the inline form");
        assert_eq!(bound.name, "Notch", "not the decoration's translation key");
        assert_eq!(bound.target_name.as_deref(), Some("Herobrine"));
        assert_eq!(r.remaining(), 0);
    }

    /// A whole `disguised_chat` reaches the chat log through the routing seam.
    ///
    /// MUTATION: `apply` pushing nothing (the packet decoded but never
    /// surfaced) — the pre-M78 behaviour, and the one the coverage document
    /// calls out as invisible: the client is not broken in any way an observer
    /// can name.
    #[test]
    fn routing_a_disguised_chat_appends_the_message() {
        let mut body = wire_component("psst");
        body.push(0x02); // holder → chat type 1
        body.extend(wire_component("Notch"));
        body.push(0x00);

        let mut s = SessionState::default();
        assert!(apply(SessionPacket::DisguisedChat, &body, &mut s));
        assert_eq!(
            s.take_chat(),
            vec![rewo_proto::nbt::Nbt::String("psst".into())]
        );
        // Drained, not duplicated.
        assert!(s.take_chat().is_empty());
    }

    /// Each id maps to its own packet, and nothing else does.
    ///
    /// MUTATION: swapping any two arms of `kind_for_id`. Several of these
    /// bodies are mutually decodable — `player_combat_enter` accepts any body
    /// at all, and `custom_payload` and `store_cookie` both open with an
    /// identifier — so a swap is silent rather than an error.
    #[test]
    fn each_session_id_maps_to_its_own_packet() {
        let t = ids();
        assert_eq!(kind_for_id(24, t), Some(SessionPacket::CustomPayload));
        assert_eq!(kind_for_id(33, t), Some(SessionPacket::DisguisedChat));
        assert_eq!(kind_for_id(39, t), Some(SessionPacket::GameRuleValues));
        assert_eq!(kind_for_id(66, t), Some(SessionPacket::PlayerCombatEnd));
        assert_eq!(kind_for_id(67, t), Some(SessionPacket::PlayerCombatEnter));
        assert_eq!(kind_for_id(86, t), Some(SessionPacket::ServerData));
        assert_eq!(kind_for_id(137, t), Some(SessionPacket::ServerLinks));
        assert_eq!(kind_for_id(120, t), Some(SessionPacket::StoreCookie));
        assert_eq!(kind_for_id(1, t), None);
    }

    /// A `server_links` body reaches [`SessionState::server_links`] through the
    /// same [`apply`] the configuration loop and `route_session` both call, and
    /// a later packet **replaces** the list.
    ///
    /// MUTATION: `state.server_links.entries` extended rather than assigned.
    /// `handleServerLinks` builds a fresh `ServerLinks` from the packet, so a
    /// merge would make an empty list — the packet a server sends to retract —
    /// do nothing at all, and the pause menu would keep a button for links the
    /// server has withdrawn.
    #[test]
    fn server_links_reach_the_session_state_and_a_second_packet_replaces_them() {
        use crate::server_links::{KnownLinkType, ServerLinkLabel};
        let mut body = vec![0x01, 0x01];
        rewo_proto::varint::write_varint(&mut body, 6); // WEBSITE
        body.extend(wire_string("https://example.test"));

        let mut s = SessionState::default();
        assert!(s.server_links.is_empty());
        assert!(apply(SessionPacket::ServerLinks, &body, &mut s));
        assert_eq!(s.server_links.len(), 1);
        assert_eq!(
            s.server_links.entries()[0].label,
            ServerLinkLabel::Known(KnownLinkType::Website)
        );

        // The retraction.
        assert!(apply(SessionPacket::ServerLinks, &[0x00], &mut s));
        assert!(
            s.server_links.is_empty(),
            "the list is assigned, not merged"
        );
    }

    /// A malformed body is reported rather than half-applied.
    ///
    /// MUTATION: `apply` returning `true` on a decode error, or assigning as it
    /// reads. Every reader above builds its value before anything is written,
    /// which is what makes "changed nothing" assertable.
    #[test]
    fn a_malformed_session_body_reports_false_and_changes_nothing() {
        let mut s = SessionState::default();
        s.game_rules.insert("keep".into(), "true".into());
        let before = s.clone();
        assert!(!apply(SessionPacket::CustomPayload, &[0x7f], &mut s));
        assert!(!apply(SessionPacket::DisguisedChat, &[], &mut s));
        assert!(!apply(SessionPacket::GameRuleValues, &[0x7f, 0x00], &mut s));
        assert!(!apply(SessionPacket::ServerData, &[], &mut s));
        assert!(!apply(SessionPacket::StoreCookie, &[], &mut s));
        assert!(!apply(SessionPacket::PlayerCombatEnd, &[], &mut s));
        // A count that outruns the buffer, so the list never starts.
        assert!(!apply(SessionPacket::ServerLinks, &[0x7f, 0x00], &mut s));
        assert_eq!(s, before);
    }
}
