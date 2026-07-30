//! `CommonPlayerSpawnInfo` — the spawn-state block that both
//! `ClientboundLoginPacket` and `ClientboundRespawnPacket` embed verbatim.
//!
//! This is the **only** spawn-info decoder in the crate: the play-login path
//! and the respawn path read the same bytes through the same function, so the
//! two can never disagree about the dimension we are in. Nothing here applies
//! anything to the world — it is pure wire decode.
//!
//! Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`), field-for-field:
//!
//! - `net/minecraft/network/protocol/game/CommonPlayerSpawnInfo.java` — the
//!   record's `RegistryFriendlyByteBuf` constructor *is* the field order below.
//! - `net/minecraft/world/level/dimension/DimensionType.java` —
//!   `STREAM_CODEC = ByteBufCodecs.holderRegistry(...)`, which
//!   `ByteBufCodecs.registry` implements as a plain `VarInt` **raw 0-based
//!   registry id**. There is no `0=inline`/`id+1` case here; that belongs to
//!   the different `ByteBufCodecs.holder` codec.
//! - `net/minecraft/world/level/GameType.java` — `byNullableId(id)` is
//!   `id == -1 ? null : byId(id)`, so the nullable game type is a **signed
//!   sentinel byte**, not the boolean-prefixed `readNullable` convention.
//! - `net/minecraft/network/FriendlyByteBuf.java` — `readOptional` *is* the
//!   boolean-prefixed convention, and `readGlobalPos` is
//!   `readResourceKey(DIMENSION)` followed by `readBlockPos` (one packed
//!   `long`, x 26 | z 26 | y 12).
//! - `net/minecraft/network/protocol/game/ClientboundLoginPacket.java` /
//!   `ClientboundRespawnPacket.java` — what surrounds the block.

use rewo_proto::reader::PacketReader;
use rewo_proto::{ProtoError, Result};

/// `GameType.getNullableId(null)` — the wire value for "no previous game type".
pub const GAME_TYPE_NOT_SET: i8 = -1;

/// `net.minecraft.core.GlobalPos` as `FriendlyByteBuf::readGlobalPos` reads it:
/// a dimension `ResourceKey` identifier followed by one packed `BlockPos` long.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalPos {
    /// `ResourceKey<Level>` identifier, e.g. `"minecraft:overworld"`.
    pub dimension: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl GlobalPos {
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        let dimension = r.identifier()?;
        let (x, y, z) = r.position()?;
        Ok(Self { dimension, x, y, z })
    }
}

/// The decoded `CommonPlayerSpawnInfo` record.
///
/// Every field the packet carries is retained, including the ones nothing
/// consumes yet: a decoder that silently stopped early is exactly how the
/// respawn path would end up re-deriving a different byte offset later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommonPlayerSpawnInfo {
    /// `Holder<DimensionType>` — the **raw 0-based** `minecraft:dimension_type`
    /// registry id, to be resolved through `crate::login_dimension_type`.
    pub dimension_type: i32,
    /// `ResourceKey<Level>` identifier of the dimension itself
    /// (`"minecraft:the_nether"`), which is *not* the same thing as the
    /// dimension-type holder above.
    pub dimension: String,
    /// The world seed hash — read as `biomeZoomSeed` by the biome layer.
    pub seed: i64,
    /// `GameType.byId(readByte())`, kept as the raw wire byte.
    pub game_type: i8,
    /// `GameType.byNullableId(readByte())`: `-1` on the wire means absent.
    pub previous_game_type: Option<i8>,
    pub is_debug: bool,
    pub is_flat: bool,
    /// `Optional<GlobalPos>` — boolean-prefixed (`readOptional`).
    pub last_death_location: Option<GlobalPos>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
}

impl CommonPlayerSpawnInfo {
    /// Decode the block in place, leaving the reader on the byte after it (the
    /// login packet's `onlineMode` flag, or the respawn packet's `dataToKeep`).
    ///
    /// The respawn path reaches this through [`RespawnInfo::parse`] and never
    /// directly, so the trailing-byte check below cannot be skipped.
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        let dimension_type = r.varint()?;
        let dimension = r.identifier()?;
        let seed = r.i64()?;
        let game_type = r.i8()?;
        let previous = r.i8()?;
        let is_debug = r.bool()?;
        let is_flat = r.bool()?;
        let last_death_location = r.option(GlobalPos::read)?;
        let portal_cooldown = r.varint()?;
        let sea_level = r.varint()?;
        Ok(Self {
            dimension_type,
            dimension,
            seed,
            game_type,
            previous_game_type: (previous != GAME_TYPE_NOT_SET).then_some(previous),
            is_debug,
            is_flat,
            last_death_location,
            portal_cooldown,
            sea_level,
        })
    }
}

/// `ClientboundRespawnPacket`: the spawn info plus one trailing `dataToKeep`
/// byte, and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RespawnInfo {
    pub spawn: CommonPlayerSpawnInfo,
    pub data_to_keep: u8,
}

impl RespawnInfo {
    pub const KEEP_ATTRIBUTE_MODIFIERS: u8 = 1;
    pub const KEEP_ENTITY_DATA: u8 = 2;
    pub const KEEP_ALL_DATA: u8 = 3;

    /// Decode a whole respawn packet body. A body that ends early *or* carries
    /// bytes past `dataToKeep` is rejected rather than half-applied: this
    /// packet re-points the world at a different dimension, so "we think we
    /// parsed it" is the one outcome that must not be silently possible.
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(body);
        let spawn = CommonPlayerSpawnInfo::read(&mut r)?;
        let data_to_keep = r.u8()?;
        if !r.is_empty() {
            return Err(ProtoError::LengthOutOfRange {
                what: "respawn trailing bytes",
                len: r.remaining() as i64,
                max: 0,
            });
        }
        Ok(Self {
            spawn,
            data_to_keep,
        })
    }

    /// `ClientboundRespawnPacket.shouldKeep`.
    pub fn should_keep(&self, mask: u8) -> bool {
        self.data_to_keep & mask != 0
    }
}

/// What [`read_login_prefix`] recovers from the login packet's prefix.
///
/// The two distances used to be read into a discard (`r.varint()?;`), which is
/// the same shape as the latency M52c found. They are the server's initial
/// view area — `handleLogin` assigns both to `ClientPacketListener` *and*
/// feeds the radius to `options.setServerRenderDistance` — so they are
/// returned from the one existing walk rather than read by a second one. That
/// distinction is not stylistic: M62's report records a real drift caused by
/// exactly that mistake, where a test copy of the player-info walk capped a
/// profile signature at 32767 while production correctly had 1024, so the
/// tests were grading a walk the client does not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoginPrefix {
    /// The local player's entity id — a big-endian `int`, not a VarInt.
    pub player_id: i32,
    /// `ClientboundLoginPacket.chunkRadius`.
    pub chunk_radius: i32,
    /// `ClientboundLoginPacket.simulationDistance`.
    pub simulation_distance: i32,
    /// `ClientboundLoginPacket.hardcore` (M82) — the third field the walk read
    /// into a discard, and the same lesson a third time. It chooses the death
    /// screen's title (`deathScreen.title.hardcore`) and its first button's
    /// label (`deathScreen.spectate`), and nothing else on the wire carries
    /// it: `handlePlayerCombatKill` reads it off
    /// `this.level.getLevelData().isHardcore()`, which `handleLogin` seeded
    /// from exactly this byte.
    pub hardcore: bool,
    /// `ClientboundLoginPacket.showDeathScreen` (M82) — the *join-time* value
    /// of the `doImmediateRespawn` game rule.
    ///
    /// M71 shipped `game_event` id 11 (`IMMEDIATE_RESPAWN`), which is only the
    /// **mid-session** change; the initial state rides here and was being
    /// dropped, so a server that starts with `doImmediateRespawn true` and
    /// never touches it again looked like a server that wanted the screen.
    pub show_death_screen: bool,
}

/// Read the `ClientboundLoginPacket` fields that come *before* its embedded
/// `CommonPlayerSpawnInfo`, leaving the reader on the spawn info's first byte.
///
/// Shared by the live `Connection`, the play session and the replay path, so
/// "where does the spawn info start" is answered in exactly one place.
pub(crate) fn read_login_prefix(r: &mut PacketReader<'_>) -> Result<LoginPrefix> {
    let player_id = r.i32()?;
    let hardcore = r.bool()?; // M82
    let levels = r.count("dimensions", 1)?;
    for _ in 0..levels {
        r.identifier()?; // ResourceKey<Level>
    }
    r.varint()?; // max players
    let chunk_radius = r.varint()?; // view distance (M67)
    let simulation_distance = r.varint()?; // M67
    r.bool()?; // reduced debug info
    let show_death_screen = r.bool()?; // M82
    r.bool()?; // do limited crafting
    Ok(LoginPrefix {
        player_id,
        chunk_radius,
        simulation_distance,
        hardcore,
        show_death_screen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    /// `BlockPos.asLong`: x 26 | z 26 | y 12, each two's-complement inside its
    /// own field width (`BlockPos.PACKED_*`, X_OFFSET 38 / Z_OFFSET 12).
    fn packed(x: i32, y: i32, z: i32) -> i64 {
        ((x as i64 & 0x3ff_ffff) << 38) | ((z as i64 & 0x3ff_ffff) << 12) | (y as i64 & 0xfff)
    }

    /// `CommonPlayerSpawnInfo.write`, byte for byte.
    fn write_spawn(w: &mut PacketWriter, info: &CommonPlayerSpawnInfo) {
        w.varint(info.dimension_type);
        w.string(&info.dimension);
        w.i64(info.seed);
        w.i8(info.game_type);
        w.i8(info.previous_game_type.unwrap_or(GAME_TYPE_NOT_SET));
        w.bool(info.is_debug);
        w.bool(info.is_flat);
        match &info.last_death_location {
            Some(g) => {
                w.bool(true);
                w.string(&g.dimension);
                w.i64(packed(g.x, g.y, g.z));
            }
            None => {
                w.bool(false);
            }
        }
        w.varint(info.portal_cooldown);
        w.varint(info.sea_level);
    }

    fn spawn_bytes(info: &CommonPlayerSpawnInfo) -> Vec<u8> {
        let mut w = PacketWriter::default();
        write_spawn(&mut w, info);
        w.buf
    }

    fn respawn_bytes(info: &CommonPlayerSpawnInfo, data_to_keep: u8) -> Vec<u8> {
        let mut w = PacketWriter::default();
        write_spawn(&mut w, info);
        w.u8(data_to_keep);
        w.buf
    }

    /// Both nullables present, and a `GlobalPos` with two negative packed
    /// coordinates (x below zero and y below the build floor) — the case a
    /// mask-without-sign-extension bug turns into a 67-million-block jump.
    fn full() -> CommonPlayerSpawnInfo {
        CommonPlayerSpawnInfo {
            dimension_type: 3,
            dimension: "minecraft:the_nether".into(),
            seed: -0x0123_4567_89ab_cdef,
            game_type: 1,
            previous_game_type: Some(0),
            is_debug: false,
            is_flat: true,
            last_death_location: Some(GlobalPos {
                dimension: "minecraft:overworld".into(),
                x: -1_234_567,
                y: -59,
                z: 42,
            }),
            portal_cooldown: 300,
            sea_level: 63,
        }
    }

    /// Both nullables absent: `previousGameType` as the `-1` sentinel byte and
    /// `lastDeathLocation` as a bare `false`.
    fn minimal() -> CommonPlayerSpawnInfo {
        CommonPlayerSpawnInfo {
            dimension_type: 0,
            dimension: "minecraft:overworld".into(),
            seed: 0,
            game_type: 0,
            previous_game_type: None,
            is_debug: false,
            is_flat: false,
            last_death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
        }
    }

    #[test]
    fn full_spawn_info_decodes_every_field() {
        let info = full();
        let bytes = spawn_bytes(&info);
        let mut r = PacketReader::new(&bytes);
        assert_eq!(CommonPlayerSpawnInfo::read(&mut r).unwrap(), info);
        assert!(r.is_empty(), "decoder must consume the block exactly");
    }

    /// The layout pinned against hand-written bytes rather than against our own
    /// writer — the two nullable conventions differ (sentinel byte vs boolean
    /// prefix) and swapping them still round-trips through a symmetric writer.
    #[test]
    fn layout_is_the_vanilla_field_order() {
        fn ident(b: &mut Vec<u8>, s: &str) {
            assert!(s.len() < 128, "test identifiers use a 1-byte varint length");
            b.push(s.len() as u8);
            b.extend_from_slice(s.as_bytes());
        }
        let mut b = Vec::new();
        b.push(2); // dimension_type: holder VarInt, raw 0-based registry id
        ident(&mut b, "minecraft:the_end"); // dimension ResourceKey
        b.extend_from_slice(&(-1i64).to_be_bytes()); // seed
        b.push(2); // gameType = adventure
        b.push(0xff); // previousGameType = -1 → absent
        b.push(1); // isDebug
        b.push(0); // isFlat
        b.push(1); // lastDeathLocation present
        ident(&mut b, "minecraft:overworld");
        b.extend_from_slice(&packed(-3, -64, 7).to_be_bytes());
        b.push(0x80); // portalCooldown VarInt 128 …
        b.push(0x01); // … two bytes
        b.push(63); // seaLevel

        let mut r = PacketReader::new(&b);
        let got = CommonPlayerSpawnInfo::read(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(got.dimension_type, 2);
        assert_eq!(got.dimension, "minecraft:the_end");
        assert_eq!(got.seed, -1);
        assert_eq!(got.game_type, 2);
        assert_eq!(got.previous_game_type, None);
        assert!(got.is_debug);
        assert!(!got.is_flat);
        assert_eq!(
            got.last_death_location,
            Some(GlobalPos {
                dimension: "minecraft:overworld".into(),
                x: -3,
                y: -64,
                z: 7,
            })
        );
        assert_eq!(got.portal_cooldown, 128);
        assert_eq!(got.sea_level, 63);
    }

    #[test]
    fn both_nullables_absent() {
        let info = minimal();
        let bytes = spawn_bytes(&info);
        // 1 (holder) + 20 (dimension) + 8 (seed) + 1 + 1 + 1 + 1 + 1 (no
        // GlobalPos) + 1 + 1: the absent optional costs exactly one byte.
        assert_eq!(bytes.len(), 36);
        assert_eq!(bytes[30], GAME_TYPE_NOT_SET as u8, "sentinel, not a flag");
        assert_eq!(bytes[33], 0, "lastDeathLocation absent flag");
        let mut r = PacketReader::new(&bytes);
        let got = CommonPlayerSpawnInfo::read(&mut r).unwrap();
        assert_eq!(got, info);
        assert_eq!(got.previous_game_type, None);
        assert_eq!(got.last_death_location, None);
        assert!(r.is_empty());
    }

    #[test]
    fn respawn_reads_the_trailing_data_to_keep_byte() {
        for (data_to_keep, keeps_attributes, keeps_entity_data) in [
            (0u8, false, false),
            (RespawnInfo::KEEP_ATTRIBUTE_MODIFIERS, true, false),
            (RespawnInfo::KEEP_ENTITY_DATA, false, true),
            (RespawnInfo::KEEP_ALL_DATA, true, true),
        ] {
            let info = full();
            let got = RespawnInfo::parse(&respawn_bytes(&info, data_to_keep)).unwrap();
            assert_eq!(got.spawn, info, "dataToKeep={data_to_keep}");
            assert_eq!(got.data_to_keep, data_to_keep);
            assert_eq!(
                got.should_keep(RespawnInfo::KEEP_ATTRIBUTE_MODIFIERS),
                keeps_attributes
            );
            assert_eq!(
                got.should_keep(RespawnInfo::KEEP_ENTITY_DATA),
                keeps_entity_data
            );
        }
    }

    #[test]
    fn every_truncation_is_rejected() {
        for info in [full(), minimal()] {
            let spawn = spawn_bytes(&info);
            for cut in 0..spawn.len() {
                let mut r = PacketReader::new(&spawn[..cut]);
                assert!(
                    CommonPlayerSpawnInfo::read(&mut r).is_err(),
                    "spawn info accepted a {cut}-byte body of {}",
                    spawn.len()
                );
            }
            let respawn = respawn_bytes(&info, RespawnInfo::KEEP_ALL_DATA);
            for cut in 0..respawn.len() {
                assert!(
                    RespawnInfo::parse(&respawn[..cut]).is_err(),
                    "respawn accepted a {cut}-byte body of {}",
                    respawn.len()
                );
            }
            assert!(RespawnInfo::parse(&respawn).is_ok());
        }
    }

    #[test]
    fn one_trailing_byte_is_rejected() {
        let mut respawn = respawn_bytes(&full(), RespawnInfo::KEEP_ENTITY_DATA);
        respawn.push(0);
        let err = RespawnInfo::parse(&respawn).unwrap_err();
        assert!(
            matches!(
                err,
                ProtoError::LengthOutOfRange {
                    what: "respawn trailing bytes",
                    len: 1,
                    max: 0
                }
            ),
            "unexpected error: {err}"
        );
    }

    /// The login prefix must stop on the spawn info's *first* byte: one byte
    /// either way and the holder id becomes the tail of `doLimitedCrafting` or
    /// the head of the dimension identifier.
    #[test]
    fn login_prefix_lands_exactly_on_the_spawn_info() {
        let info = CommonPlayerSpawnInfo {
            dimension_type: 0, // raw 0 = the FIRST synced entry, never "inline"
            seed: 0x1234_5678_9abc_def0,
            ..full()
        };
        let mut w = PacketWriter::default();
        w.i32(42); // playerId — a big-endian int, not a VarInt
        w.bool(true); // hardcore
        w.varint(3); // levels
        w.string("minecraft:overworld");
        w.string("minecraft:the_nether");
        w.string("minecraft:the_end");
        w.varint(20); // maxPlayers
        w.varint(12); // chunkRadius
        w.varint(10); // simulationDistance
        w.bool(false); // reducedDebugInfo
        w.bool(true); // showDeathScreen
        w.bool(false); // doLimitedCrafting
        let prefix_len = w.buf.len();
        write_spawn(&mut w, &info);
        w.bool(true); // onlineMode
        w.bool(true); // enforcesSecureChat
        let bytes = w.buf;

        let mut r = PacketReader::new(&bytes);
        // The two distances (M67) are recovered from this same walk rather
        // than by a second reader, so this witness pins their *position* in
        // the prefix as well as their values: swapping them, or reading them
        // either side of `maxPlayers`, still lands on the spawn info and is
        // caught only here.
        assert_eq!(
            read_login_prefix(&mut r).unwrap(),
            LoginPrefix {
                player_id: 42,
                chunk_radius: 12,
                simulation_distance: 10,
                // M82: the two booleans this walk used to discard. They are
                // written above as `true` / `true` against neighbours that are
                // `false`, so reading either off the wrong byte fails here.
                hardcore: true,
                show_death_screen: true,
            }
        );
        assert_eq!(r.remaining(), bytes.len() - prefix_len);
        let got = CommonPlayerSpawnInfo::read(&mut r).unwrap();
        assert_eq!(got, info);
        assert_eq!(got.dimension_type, 0);
        assert_eq!(got.seed, 0x1234_5678_9abc_def0);
        assert_eq!(r.remaining(), 2, "only the two trailing login flags left");

        // The holder-only path over the same bytes agrees, and holder 0 selects
        // the first synced registry entry rather than a default.
        assert_eq!(crate::parse_login_dimension_holder(&bytes).unwrap(), 0);
        use crate::dimension_parse::builtin as fx;
        let defs =
            crate::dimension_parse::parse_dimension_registry_packet(&fx::registry_packet(&[
                ("minecraft:the_nether", fx::the_nether()),
                ("minecraft:overworld", fx::overworld()),
            ]))
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::login_dimension_type(got.dimension_type, &defs).name,
            "minecraft:the_nether"
        );
    }
}
