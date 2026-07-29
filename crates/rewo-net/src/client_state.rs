//! Four client switches: `change_difficulty` (10), `set_camera` (93) and
//! `container_close` (17) from M74, plus `set_default_spawn_position` (97)
//! from M76.
//!
//! They share a module because none of them has much machinery — each is one
//! authoritative fact with a reader and a field, and four files of ten lines
//! would obscure rather than separate. Where a packet *does* have machinery
//! behind it, it gets its own module: see [`crate::chunk_batch`],
//! [`crate::ticking`] and [`crate::player_rotation`]. In vanilla these live on
//! three different objects (`ClientLevelData`, `Minecraft`, `LocalPlayer`),
//! which is worth knowing precisely because it means nothing links them.
//!
//! All four are `REWO_PACKET_COVERAGE.md` class **A**: the decode changes a
//! value Rewo can act on, and a witness can prove it without drawing anything.
//!
//! ## `set_default_spawn_position` — the compass target, and the one field
//! ## here that a dimension change **resets**
//!
//! `handleSetSpawn` is `minecraft.level.setRespawnData(packet.respawnData())`,
//! so the value lands on `ClientLevelData` beside the difficulty. The two then
//! behave oppositely across a dimension change, and both behaviours fall out of
//! `handleRespawn` building a replacement level data:
//!
//! ```java
//! ClientLevelData levelData = new ClientLevel.ClientLevelData(
//!     this.levelData.getDifficulty(), this.levelData.isHardcore(), isFlat);
//! ```
//!
//! The difficulty is **carried across explicitly**; the respawn data is not in
//! the constructor at all. `ClientLevelData.respawnData` is a bare reference
//! field with no initialiser — so it is momentarily null, and what fills it is
//! the `ClientLevel` constructor, one line later:
//!
//! ```java
//! this.setRespawnData(LevelData.RespawnData.of(dimension, new BlockPos(8, 64, 8), 0.0F, 0.0F));
//! ```
//!
//! Two consequences worth stating, because both invert the obvious guess.
//! **`LevelData.RespawnData.DEFAULT` — overworld, `BlockPos.ZERO` — never
//! appears on a client**; the constructor's `(8, 64, 8)` *of the level being
//! entered* is the real default, and it follows you into the Nether. And a
//! same-dimension respawn (dying) keeps the level data entirely, so the spawn
//! point survives death and is discarded by travel — the reverse of the
//! intuition that a respawn packet resets respawn state. [`ClientState::
//! enter_level`] is that constructor line; nothing calls it on the
//! same-dimension path.
//!
//! **One scoped exclusion.** `setRespawnData` actually stores
//! `getWorldBorderAdjustedRespawnData(…)`, which relocates a spawn outside the
//! world border onto the border's centre column via a `MOTION_BLOCKING`
//! heightmap lookup. Rewo has no world border — `initialize_border` and the
//! five `set_border_*` packets are all class **B** in the coverage table — and
//! the default border is ±29,999,984, which contains every position a world
//! generates. The adjustment is therefore unreachable here and the value is
//! stored verbatim; landing it needs the border packets first, not a guess at
//! its bounds.
//!
//! ## `change_difficulty` — a **third** enum convention
//!
//! The project's notes record two ways vanilla decodes an enum: `readEnum`
//! (an array index, where out-of-range is an *error*) and
//! `ByIdMap.continuous(…, ZERO)` (where out-of-range is the zero value).
//! `Difficulty` is neither. It is
//! `ByIdMap.continuous(Difficulty::getId, values(), OutOfBoundsStrategy.WRAP)`
//! behind `ByteBufCodecs.idMapper`, so the id is a **VarInt** and an
//! out-of-range one **wraps**: `sortedValues[Math.floorMod(id, 4)]`.
//!
//! The three readings disagree on real inputs. For id `5`: WRAP gives `EASY`,
//! ZERO gives `PEACEFUL`, and `readEnum` rejects the packet outright. For a
//! *negative* id the gap is wider still, because `floorMod` is not `%` —
//! `floorMod(-1, 4)` is `3` (`HARD`) where Rust's `%` gives `-1` and panics on
//! the index.
//!
//! ## `set_camera` — an unknown entity is ignored, not a reset
//!
//! `handleSetCamera` is `Entity e = packet.getEntity(level); if (e != null)
//! setCameraEntity(e);`. So a `set_camera` naming an entity the client has not
//! been told about leaves the camera **where it already was**. It does not
//! fall back to the player, and it is not an error.
//!
//! The resolution is `level.getEntity(id)`, and vanilla's level contains the
//! local player. Rewo's [`rewo_world::entities::EntityTable`] does **not** —
//! the server never sends an `add_entity` for you — so the caller must treat
//! the local player's own id as resolvable or the server could never hand the
//! camera back at the end of a spectate. See [`ClientState::set_camera`].
//!
//! ## `container_close` — the container id is read and **ignored**
//!
//! `handleContainerClose` is one line: `this.minecraft.player
//! .clientSideCloseContainer()`. There is no comparison against
//! `containerMenu.containerId`. Gating on the id — the natural instinct, and
//! the shape M34/M35 use for `container_set_slot`, which *does* filter — would
//! leave the screen open on exactly the packet whose job is to close it.
//!
//! `clientSideCloseContainer` is `super.closeContainer()` (which is
//! `containerMenu = inventoryMenu`) plus `gui.setScreen(null)`. Rewo has only
//! the player's own menu by M34's documented choice, so the first half is a
//! no-op here and the observable effect is the second: the inventory screen
//! closes. That screen lives in `rewo-app`, so this module raises a latch and
//! the app drains it.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/network/protocol/game/ClientboundChangeDifficultyPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundSetCameraPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundContainerClosePacket.java`
//! - `net/minecraft/world/Difficulty.java`
//! - `net/minecraft/util/ByIdMap.java` — `continuous`, `OutOfBoundsStrategy`
//! - `net/minecraft/util/Mth.java` — `positiveModulo` is `Math.floorMod`
//! - `net/minecraft/network/codec/ByteBufCodecs.java` — `idMapper` reads a VarInt
//! - `net/minecraft/network/FriendlyByteBuf.java` — `readContainerId` is a VarInt
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleChangeDifficulty`, `handleSetCamera`, `handleContainerClose`
//! - `net/minecraft/client/player/LocalPlayer.java` — `clientSideCloseContainer`
//! - `net/minecraft/world/entity/player/Player.java` — `closeContainer`
//! - `net/minecraft/network/protocol/game/ClientboundSetDefaultSpawnPositionPacket.java`
//! - `net/minecraft/world/level/storage/LevelData.java` — `RespawnData`
//! - `net/minecraft/core/GlobalPos.java` — `STREAM_CODEC`
//! - `net/minecraft/core/BlockPos.java` + `net/minecraft/network/FriendlyByteBuf.java`
//!   — `readBlockPos` is `BlockPos.of(readLong())`
//! - `net/minecraft/client/multiplayer/ClientLevel.java` — the constructor's
//!   `(8, 64, 8)` seed, `setRespawnData`, `ClientLevelData`
//! - `net/minecraft/world/level/Level.java` — `getWorldBorderAdjustedRespawnData`

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `net.minecraft.world.Difficulty`, in registry-id order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Peaceful = 0,
    Easy = 1,
    Normal = 2,
    Hard = 3,
}

impl Difficulty {
    /// The four values in `getId` order, which is also declaration order —
    /// `ByIdMap.createSortedArray` would throw if it were not continuous.
    pub const VALUES: [Difficulty; 4] = [
        Difficulty::Peaceful,
        Difficulty::Easy,
        Difficulty::Normal,
        Difficulty::Hard,
    ];

    /// `Difficulty.byId` — `ByIdMap.continuous(…, WRAP)`, i.e.
    /// `sortedValues[Mth.positiveModulo(id, 4)]` where `positiveModulo` is
    /// `Math.floorMod`.
    ///
    /// Total: there is no id this rejects, which is the whole point of WRAP.
    pub fn by_id(id: i32) -> Difficulty {
        // `Math.floorMod` — Rust's `%` is a remainder, not a modulus, so it
        // returns a negative for a negative left operand. `rem_euclid` is the
        // one that matches, and the cast is safe because the result is in
        // 0..4 by construction.
        let idx = id.rem_euclid(Difficulty::VALUES.len() as i32) as usize;
        Difficulty::VALUES[idx]
    }

    /// `getId`.
    pub fn id(self) -> i32 {
        self as i32
    }
}

/// `LevelData.RespawnData` — the world spawn, which is both the compass target
/// and where a respawn without a bed puts you.
///
/// `GlobalPos` then two floats:
///
/// ```text
/// ResourceKey<Level>   Identifier.STREAM_CODEC  — a namespaced string
/// BlockPos             one packed big-endian i64
/// float yaw
/// float pitch
/// ```
///
/// The dimension is `Registries.DIMENSION` — the **level** key
/// (`minecraft:the_nether`), not `minecraft:dimension_type`. The two registries
/// share their vanilla names and are different things; M16 records the same
/// distinction from the other side.
///
/// **The stream codec does not normalise.** `RespawnData.of` applies
/// `Mth.wrapDegrees(yaw)` and `Mth.clamp(pitch, -90, 90)`, and `MAP_CODEC`
/// declares `floatRange` bounds — but neither is on this path:
/// `STREAM_CODEC` is a bare `composite` over the record's three accessors. So
/// the decode stores what the wire said. A reader that "helpfully" wrapped
/// would disagree with the value vanilla holds whenever a server built its
/// `RespawnData` by any route but `of`.
#[derive(Clone, Debug, PartialEq)]
pub struct RespawnData {
    /// The `ResourceKey<Level>` identifier, e.g. `minecraft:overworld`.
    pub dimension: String,
    /// `BlockPos`, unpacked.
    pub pos: (i32, i32, i32),
    pub yaw: f32,
    pub pitch: f32,
}

impl RespawnData {
    /// `ClientLevel`'s constructor seed: `RespawnData.of(dimension,
    /// BlockPos(8, 64, 8), 0, 0)`. **Not** `RespawnData.DEFAULT`, which is
    /// overworld/`BlockPos.ZERO` and is unreachable from a client.
    ///
    /// This goes through `of`, so the wrap and clamp *do* apply here — they are
    /// no-ops on `0.0`, and writing them out would imply the decode path shares
    /// them. It does not.
    pub fn level_default(dimension: &str) -> RespawnData {
        RespawnData {
            dimension: dimension.to_string(),
            pos: (8, 64, 8),
            yaw: 0.0,
            pitch: 0.0,
        }
    }
}

/// The four switches, plus the container-close latch.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientState {
    /// `ClientLevelData.difficulty`.
    ///
    /// `NORMAL` is not a guess and not a Rewo convention: `handleLogin` builds
    /// `new ClientLevelData(Difficulty.NORMAL, hardcore, isFlat)` with that
    /// constant literally in the source. The login packet carries no
    /// difficulty at all — its fields are `playerId`, `hardcore`, `levels`,
    /// `maxPlayers`, `chunkRadius`, `simulationDistance`, `reducedDebugInfo`,
    /// `showDeathScreen`, `doLimitedCrafting`, the spawn info, `onlineMode`
    /// and `enforcesSecureChat` — so `change_difficulty` is the *only* source,
    /// and a client that never receives one says NORMAL exactly as vanilla
    /// does.
    pub difficulty: Difficulty,
    /// `ClientLevelData.difficultyLocked`.
    pub difficulty_locked: bool,
    /// `Minecraft.cameraEntity`, or `None` while it is still the local player.
    ///
    /// Modelled as an `Option` rather than seeded with the player's id because
    /// the session learns its own entity id from the login packet, which
    /// arrives after this struct is built. [`Self::camera_entity_or`] resolves
    /// the two.
    camera_entity: Option<i32>,
    /// How many `container_close` packets have arrived. A counter rather than
    /// a flag so a witness can tell "closed twice" from "closed once", and so
    /// a consumer that misses an edge can still notice.
    close_container_requests: u64,
    /// `ClientLevelData.respawnData` (M76).
    ///
    /// `None` models vanilla's brief null window: the field has no initialiser
    /// and is filled by the `ClientLevel` constructor. A live client can never
    /// observe it — `getRespawnData()` would NPE — and neither can Rewo, since
    /// [`ClientState::enter_level`] runs on the login packet.
    respawn_data: Option<RespawnData>,
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            difficulty: Difficulty::Normal,
            difficulty_locked: false,
            camera_entity: None,
            close_container_requests: 0,
            respawn_data: None,
        }
    }
}

impl ClientState {
    /// `handleChangeDifficulty` — `setDifficulty` then `setDifficultyLocked`.
    pub fn apply_change_difficulty(&mut self, difficulty: Difficulty, locked: bool) {
        self.difficulty = difficulty;
        self.difficulty_locked = locked;
    }

    /// `handleSetCamera`. `resolvable` answers `level.getEntity(id) != null`;
    /// when it is false vanilla does nothing at all, so neither does this.
    ///
    /// Returns whether the camera moved, for the caller's logging.
    pub fn set_camera(&mut self, id: i32, resolvable: bool) -> bool {
        if !resolvable {
            return false;
        }
        self.camera_entity = Some(id);
        true
    }

    /// The camera's entity id, falling back to the local player's when the
    /// server has never redirected it — `Minecraft.getCameraEntity()`, which
    /// is initialised to the player and only ever reassigned.
    pub fn camera_entity_or(&self, local_player: Option<i32>) -> Option<i32> {
        self.camera_entity.or(local_player)
    }

    /// Whether the server has explicitly redirected the camera. Distinct from
    /// [`Self::camera_entity_or`] returning the player's id, which it also
    /// does when the server redirected the camera *to* the player.
    pub fn camera_redirected(&self) -> bool {
        self.camera_entity.is_some()
    }

    /// `clientSideCloseContainer`. The container id is deliberately not a
    /// parameter — see the module docs.
    pub fn close_container(&mut self) {
        self.close_container_requests = self.close_container_requests.saturating_add(1);
    }

    /// The running count of close requests, for a consumer that polls.
    pub fn close_container_requests(&self) -> u64 {
        self.close_container_requests
    }

    /// `handleSetSpawn` — `level.setRespawnData(packet.respawnData())`.
    pub fn set_respawn_data(&mut self, data: RespawnData) {
        self.respawn_data = Some(data);
    }

    /// `level.getRespawnData()`. `None` only before the first level exists.
    pub fn respawn_data(&self) -> Option<&RespawnData> {
        self.respawn_data.as_ref()
    }

    /// `new ClientLevel(…)`'s final line, for the level now being entered.
    ///
    /// Called on the login packet and on a **dimension-changing** respawn, and
    /// deliberately not on a same-dimension one: vanilla builds no new
    /// `ClientLevel` there, so the spawn point survives a death and is reset by
    /// travel. That asymmetry is the opposite way round from the difficulty
    /// sitting beside it, which `handleRespawn` copies across explicitly.
    pub fn enter_level(&mut self, dimension: &str) {
        self.respawn_data = Some(RespawnData::level_default(dimension));
    }
}

/// `level.getEntity(id) != null`, for Rewo's split world model.
///
/// A named function rather than a closure inside the routing seam, because
/// the `||` is the whole rule and a rule that lives only inside a closure has
/// no witness that can reach it. (It had none: the M74 mutation battery ran
/// "resolve only via the entity table" and it **survived**, because the
/// witnesses tested [`ClientState::set_camera`] — which takes the answer —
/// rather than the thing that computes it.)
///
/// Vanilla's level contains the local player and Rewo's `EntityTable` never
/// does, so the second clause is what lets a server hand the camera back at
/// the end of a spectate. `local_player` is `None` until the login packet
/// names us, and an unknown id is not resolvable — vanilla would find no
/// entity either.
pub fn camera_target_resolvable(
    target: i32,
    entities: &rewo_world::entities::EntityTable,
    local_player: Option<i32>,
) -> bool {
    entities.get(target).is_some() || local_player == Some(target)
}

/// Which of the four packets a body is. Nothing in any of the bodies says —
/// they are a VarInt-plus-bool, two bare VarInts and a `RespawnData` — so the
/// id is the only discriminator that exists, exactly as with the view area's
/// radius pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientStatePacket {
    ChangeDifficulty,
    SetCamera,
    ContainerClose,
    SetDefaultSpawnPosition,
}

/// The four resolved ids, lifted out of [`crate::ids::Ids`] so
/// [`kind_for_id`] and [`apply`] can be driven by a witness that has no
/// datagen report to build a full `Ids` from.
#[derive(Clone, Copy, Debug)]
pub struct ClientStateIds {
    pub change_difficulty: i32,
    pub set_camera: i32,
    pub container_close: i32,
    pub set_default_spawn_position: i32,
}

/// Map an incoming packet id onto a kind, or `None` if it is none of the three.
pub fn kind_for_id(id: i32, ids: ClientStateIds) -> Option<ClientStatePacket> {
    if id == ids.change_difficulty {
        Some(ClientStatePacket::ChangeDifficulty)
    } else if id == ids.set_camera {
        Some(ClientStatePacket::SetCamera)
    } else if id == ids.container_close {
        Some(ClientStatePacket::ContainerClose)
    } else if id == ids.set_default_spawn_position {
        Some(ClientStatePacket::SetDefaultSpawnPosition)
    } else {
        None
    }
}

/// Decode one body and apply it. Returns whether the body decoded — the
/// caller has already committed to the id, so a `false` is a malformed packet,
/// not an unrecognised one.
pub fn apply(
    kind: ClientStatePacket,
    body: &[u8],
    state: &mut ClientState,
    entities: &rewo_world::entities::EntityTable,
    local_player: Option<i32>,
) -> bool {
    match kind {
        ClientStatePacket::ChangeDifficulty => match read_change_difficulty(body) {
            Ok((d, locked)) => {
                state.apply_change_difficulty(d, locked);
                log::debug!("net: difficulty {d:?} locked={locked}");
                true
            }
            Err(err) => {
                log::debug!("net: change_difficulty decode: {err}");
                false
            }
        },
        ClientStatePacket::SetCamera => match read_set_camera(body) {
            Ok(target) => {
                // `handleSetCamera` does nothing at all for an entity it
                // cannot resolve — it does not fall back to the player.
                let ok = camera_target_resolvable(target, entities, local_player);
                let moved = state.set_camera(target, ok);
                log::debug!("net: set_camera {target} moved={moved}");
                true
            }
            Err(err) => {
                log::debug!("net: set_camera decode: {err}");
                false
            }
        },
        ClientStatePacket::ContainerClose => match read_container_close(body) {
            // The id is decoded for the log and **not** consulted:
            // `handleContainerClose` closes whatever is open.
            Ok(container) => {
                state.close_container();
                log::debug!("net: container_close id={container}");
                true
            }
            // Vanilla never reads the field at all, so strictly it would
            // close on a malformed body too. Rewo declines, which is the one
            // deviation here and is unreachable for any body a server can
            // send: the field is the whole body and a VarInt is the whole
            // field.
            Err(err) => {
                log::debug!("net: container_close decode: {err}");
                false
            }
        },
        ClientStatePacket::SetDefaultSpawnPosition => {
            match read_set_default_spawn_position(body) {
                Ok(data) => {
                    log::debug!(
                        "net: default spawn {:?} in {} yaw={} pitch={}",
                        data.pos,
                        data.dimension,
                        data.yaw,
                        data.pitch
                    );
                    state.set_respawn_data(data);
                    true
                }
                Err(err) => {
                    log::debug!("net: set_default_spawn_position decode: {err}");
                    false
                }
            }
        }
    }
}

/// `ClientboundChangeDifficultyPacket` — `Difficulty.STREAM_CODEC` (a VarInt
/// id through the WRAP map) then `ByteBufCodecs.BOOL`.
pub fn read_change_difficulty(body: &[u8]) -> Result<(Difficulty, bool)> {
    let mut r = PacketReader::new(body);
    let id = r.varint()?;
    let locked = r.bool()?;
    Ok((Difficulty::by_id(id), locked))
}

/// `ClientboundSetCameraPacket` — one VarInt entity id.
pub fn read_set_camera(body: &[u8]) -> Result<i32> {
    let mut r = PacketReader::new(body);
    r.varint()
}

/// `ClientboundContainerClosePacket` — `readContainerId()`, which is a VarInt.
///
/// Decoded and returned even though vanilla ignores it, so the dispatch arm
/// can log it and a witness can prove the value does not gate the effect.
pub fn read_container_close(body: &[u8]) -> Result<i32> {
    let mut r = PacketReader::new(body);
    r.varint()
}

/// `ClientboundSetDefaultSpawnPositionPacket` — one
/// `LevelData.RespawnData.STREAM_CODEC`, which is
/// `GlobalPos.STREAM_CODEC` + `FLOAT` + `FLOAT`.
///
/// `GlobalPos` is `ResourceKey.streamCodec(Registries.DIMENSION)` — an
/// `Identifier`, i.e. a **length-prefixed string**, not a registry id — then
/// `BlockPos.STREAM_CODEC`, which is `BlockPos.of(readLong())`, one packed
/// big-endian i64 among the string and the floats. Reading the dimension as a
/// VarInt id would consume the string's length prefix and misread everything
/// after it.
pub fn read_set_default_spawn_position(body: &[u8]) -> Result<RespawnData> {
    let mut r = PacketReader::new(body);
    let dimension = r.identifier()?;
    let pos = r.position()?;
    let yaw = r.f32()?;
    let pitch = r.f32()?;
    Ok(RespawnData {
        dimension,
        pos,
        yaw,
        pitch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four in-range ids map in declaration order.
    ///
    /// MUTATION: any reordering of `VALUES`. Peaceful and Hard are at the ends
    /// so a reversal is caught, and Easy/Normal in the middle catch a swap.
    #[test]
    fn the_four_difficulty_ids_map_in_order() {
        assert_eq!(Difficulty::by_id(0), Difficulty::Peaceful);
        assert_eq!(Difficulty::by_id(1), Difficulty::Easy);
        assert_eq!(Difficulty::by_id(2), Difficulty::Normal);
        assert_eq!(Difficulty::by_id(3), Difficulty::Hard);
        for d in Difficulty::VALUES {
            assert_eq!(Difficulty::by_id(d.id()), d, "round trip");
        }
    }

    /// The out-of-bounds strategy is **WRAP**, not ZERO and not an error.
    ///
    /// The sample sits **one past** the last valid id, which is where the
    /// three readings first differ and where an off-by-one in the modulus
    /// bites: id 4 is `PEACEFUL` under WRAP *and* under ZERO, so the witness
    /// also asserts id **5**, which is `EASY` under WRAP and `PEACEFUL` under
    /// ZERO. Without the id-5 sample a ZERO implementation passes.
    ///
    /// MUTATION: `OutOfBoundsStrategy::ZERO` (`if id < 4 { values[id] } else
    /// { Peaceful }`), or CLAMP (which gives `HARD` for both).
    #[test]
    fn an_out_of_range_difficulty_wraps_rather_than_clamping_or_erroring() {
        assert_eq!(Difficulty::by_id(4), Difficulty::Peaceful);
        assert_eq!(
            Difficulty::by_id(5),
            Difficulty::Easy,
            "WRAP gives Easy; ZERO would give Peaceful and CLAMP would give Hard"
        );
        assert_eq!(Difficulty::by_id(6), Difficulty::Normal);
        assert_eq!(Difficulty::by_id(7), Difficulty::Hard);
        assert_eq!(Difficulty::by_id(8), Difficulty::Peaceful);
    }

    /// WRAP uses `Math.floorMod`, not `%`.
    ///
    /// Rust's `%` returns a negative remainder for a negative left operand, so
    /// a `%`-based index would panic rather than wrap. `rem_euclid` is the
    /// match. The samples are the four negatives immediately below zero,
    /// which is where the two disagree maximally.
    ///
    /// MUTATION: `(id % 4) as usize`, which panics on any negative id — a
    /// server sending one would kill the connection instead of setting HARD.
    #[test]
    fn a_negative_difficulty_id_floor_mods_rather_than_panicking() {
        assert_eq!(Difficulty::by_id(-1), Difficulty::Hard);
        assert_eq!(Difficulty::by_id(-2), Difficulty::Normal);
        assert_eq!(Difficulty::by_id(-3), Difficulty::Easy);
        assert_eq!(Difficulty::by_id(-4), Difficulty::Peaceful);
        assert_eq!(Difficulty::by_id(-5), Difficulty::Hard);
        // The extremes are reachable: the field is a signed VarInt.
        assert_eq!(Difficulty::by_id(i32::MIN), Difficulty::Peaceful);
        assert_eq!(Difficulty::by_id(i32::MAX), Difficulty::Hard);
    }

    /// The body is a VarInt id then one boolean byte.
    ///
    /// MUTATION: reading the id as a single byte. That works for every id a
    /// vanilla server sends (0..3 are one byte) and desynchronises the `locked`
    /// flag the moment an id needs two — which, given WRAP, is a legal packet.
    #[test]
    fn change_difficulty_is_a_var_int_then_a_bool() {
        assert_eq!(
            read_change_difficulty(&[0x02, 0x01]).unwrap(),
            (Difficulty::Normal, true)
        );
        assert_eq!(
            read_change_difficulty(&[0x00, 0x00]).unwrap(),
            (Difficulty::Peaceful, false)
        );
        // A two-byte VarInt id (129 → floorMod 4 → 1 → Easy) followed by the
        // flag. A one-byte reader would take 0x81 as the id and 0x80 as a
        // truthy `locked`, giving (Easy, true) — the same difficulty, the
        // wrong flag.
        assert_eq!(
            read_change_difficulty(&[0x81, 0x01, 0x00]).unwrap(),
            (Difficulty::Easy, false)
        );
        assert!(read_change_difficulty(&[0x02]).is_err(), "the bool is required");
    }

    /// Applying a difficulty writes both fields.
    ///
    /// MUTATION: dropping the `locked` assignment. A server that locks the
    /// difficulty would then read as unlocked forever, and nothing else
    /// carries that bit.
    #[test]
    fn applying_a_difficulty_writes_the_lock_too() {
        let mut s = ClientState::default();
        assert_eq!(s.difficulty, Difficulty::Normal);
        assert!(!s.difficulty_locked);
        s.apply_change_difficulty(Difficulty::Hard, true);
        assert_eq!(s.difficulty, Difficulty::Hard);
        assert!(s.difficulty_locked);
        // And the lock is not sticky — it is whatever the last packet said.
        s.apply_change_difficulty(Difficulty::Peaceful, false);
        assert_eq!(s.difficulty, Difficulty::Peaceful);
        assert!(!s.difficulty_locked);
    }

    /// `set_camera` with a resolvable entity redirects the camera.
    ///
    /// MUTATION: never assigning. The label predicate would keep suppressing
    /// the player's own nametag while spectating someone else, and show the
    /// spectated entity's.
    #[test]
    fn a_resolvable_set_camera_redirects_the_camera() {
        let mut s = ClientState::default();
        assert_eq!(s.camera_entity_or(Some(7)), Some(7), "defaults to the player");
        assert!(!s.camera_redirected());
        assert!(s.set_camera(42, true));
        assert!(s.camera_redirected());
        assert_eq!(s.camera_entity_or(Some(7)), Some(42));
    }

    /// An **unresolvable** id leaves the camera where it was — it does not
    /// reset to the player and it is not an error.
    ///
    /// The sample redirects first and *then* sends the bad id, because from
    /// the default state "ignored" and "reset to the player" are the same
    /// observation. That is the only arrangement where the mutation bites.
    ///
    /// MUTATION: `self.camera_entity = resolvable.then_some(id)`, i.e.
    /// clearing on an unknown entity. Mid-spectate the camera would snap back
    /// to the player's body on any stale packet.
    #[test]
    fn an_unresolvable_set_camera_leaves_the_camera_alone() {
        let mut s = ClientState::default();
        s.set_camera(42, true);
        assert!(!s.set_camera(999, false), "reports that nothing moved");
        assert_eq!(
            s.camera_entity_or(Some(7)),
            Some(42),
            "still spectating 42, not snapped back to the player"
        );
    }

    /// The local player's own id resolves even though the entity table never
    /// contains it — which is what lets a server hand the camera back at the
    /// end of a spectate.
    ///
    /// This witness exists because an earlier one **did not catch its own
    /// named mutation**. It asserted the contract against
    /// [`ClientState::set_camera`], which *takes* the resolvability answer as
    /// an argument; the rule that computes it lived in a closure inside
    /// `route_client_state`, where nothing could reach it. Running the
    /// mutation left the suite green. The rule is now
    /// [`camera_target_resolvable`] and this samples it directly.
    ///
    /// MUTATION: dropping the `|| local_player == Some(target)` clause. The
    /// second assertion below is the only one that moves, and in the real
    /// client the camera would stick on the spectated entity for the rest of
    /// the session.
    #[test]
    fn the_local_player_resolves_though_the_entity_table_never_contains_it() {
        use rewo_world::entities::{EntityState, EntityTable};
        let mut t = EntityTable::default();
        t.add(42, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));

        assert!(camera_target_resolvable(42, &t, Some(7)), "a tracked entity");
        assert!(
            camera_target_resolvable(7, &t, Some(7)),
            "the local player, which is in vanilla's level and not in this table"
        );
        assert!(
            !camera_target_resolvable(999, &t, Some(7)),
            "an entity nobody has heard of"
        );
        assert!(
            !camera_target_resolvable(7, &t, None),
            "and not before the login packet names us"
        );

        // And end to end through the state: spectate 42, then come back.
        let mut s = ClientState::default();
        s.set_camera(42, camera_target_resolvable(42, &t, Some(7)));
        assert_eq!(s.camera_entity_or(Some(7)), Some(42));
        s.set_camera(7, camera_target_resolvable(7, &t, Some(7)));
        assert_eq!(s.camera_entity_or(Some(7)), Some(7));
    }

    /// `set_camera` is a VarInt.
    ///
    /// MUTATION: `r.i32()`, the fixed big-endian reading that the project's
    /// notes record as a real trap one field away in `explode` and
    /// `container_set_slot`. Entity ids are small, so a fixed reader would
    /// consume the next packet's bytes.
    #[test]
    fn set_camera_is_a_var_int_not_a_fixed_i32() {
        assert_eq!(read_set_camera(&[0x2a]).unwrap(), 42);
        assert_eq!(read_set_camera(&[0x80, 0x01]).unwrap(), 128);
        assert!(read_set_camera(&[]).is_err());
        // One byte is a whole body; a fixed i32 reader would demand four.
        assert!(read_set_camera(&[0x2a]).is_ok());
    }

    /// The close latch counts, and it does **not** care about the container
    /// id — the two calls below carry different ids and both land.
    ///
    /// MUTATION: gating `close_container` on the id matching the player's own
    /// menu (0). The second call would be dropped, and in the real client an
    /// open container screen would stay open on exactly the packet sent to
    /// close it.
    #[test]
    fn every_container_close_counts_whatever_the_id() {
        let mut s = ClientState::default();
        assert_eq!(s.close_container_requests(), 0);
        s.close_container();
        assert_eq!(s.close_container_requests(), 1);
        s.close_container();
        assert_eq!(s.close_container_requests(), 2);
    }

    /// The container id is a VarInt and decodes for the log, including the
    /// non-zero ids M34 drops elsewhere.
    ///
    /// MUTATION: `r.i16()`. `container_set_slot`'s index really is a signed
    /// short among var-ints, so reaching for the same reading here is the
    /// natural mistake — and `readContainerId` is a VarInt.
    #[test]
    fn container_close_reads_a_var_int_container_id() {
        assert_eq!(read_container_close(&[0x00]).unwrap(), 0);
        assert_eq!(read_container_close(&[0x07]).unwrap(), 7);
        assert_eq!(read_container_close(&[0x80, 0x01]).unwrap(), 128);
        assert!(read_container_close(&[]).is_err());
    }

    // ── The routing seam ──────────────────────────────────────────────────
    //
    // These exist because the first mutation battery ran two mutations
    // *against the routing layer* and neither could be caught: every witness
    // above tests a reader or a state method, and the routing decisions —
    // which id maps to which packet, and whether the container id gates the
    // close — lived in a closure and a match arm inside `lib.rs` that no test
    // could reach without building a whole `Ids`. `kind_for_id` / `apply`
    // are that layer lifted out, following `view_area`'s precedent.

    fn ids() -> ClientStateIds {
        ClientStateIds {
            change_difficulty: 10,
            set_camera: 93,
            container_close: 17,
            set_default_spawn_position: 97,
        }
    }

    fn table() -> rewo_world::entities::EntityTable {
        use rewo_world::entities::{EntityState, EntityTable};
        let mut t = EntityTable::default();
        t.add(42, EntityState::new(0, 10, 0.0, 0.0, 0.0, 0.0, 0.0));
        t
    }

    /// The id is the whole discriminator, and an unrelated id matches nothing.
    ///
    /// MUTATION: swapping any two arms of `kind_for_id`. The three bodies are
    /// mutually decodable — a bare VarInt parses as a container id, a camera
    /// id, *and* the first half of a difficulty — so a swap produces no error
    /// anywhere, just the wrong field being written.
    #[test]
    fn each_id_maps_to_its_own_packet_and_nothing_else_matches() {
        assert_eq!(
            kind_for_id(10, ids()),
            Some(ClientStatePacket::ChangeDifficulty)
        );
        assert_eq!(kind_for_id(93, ids()), Some(ClientStatePacket::SetCamera));
        assert_eq!(
            kind_for_id(17, ids()),
            Some(ClientStatePacket::ContainerClose)
        );
        assert_eq!(kind_for_id(99, ids()), None);
    }

    /// Through the routing seam: a `container_close` for a **non-zero**
    /// container still closes.
    ///
    /// The sample is id 7, not 0, and that is the whole point — vanilla's
    /// `handleContainerClose` never compares the id, while the sibling
    /// `container_set_slot` path correctly drops everything but container 0.
    /// A witness using id 0 passes under either reading.
    ///
    /// MUTATION: gating the `close_container()` call on `container == 0`.
    #[test]
    fn routing_a_container_close_ignores_the_container_id() {
        let (t, mut s) = (table(), ClientState::default());
        assert!(apply(
            ClientStatePacket::ContainerClose,
            &[0x07],
            &mut s,
            &t,
            Some(7)
        ));
        assert_eq!(s.close_container_requests(), 1, "id 7 must still close");
        apply(
            ClientStatePacket::ContainerClose,
            &[0x00],
            &mut s,
            &t,
            Some(7),
        );
        assert_eq!(s.close_container_requests(), 2);
    }

    /// Through the routing seam: the camera resolves against the entity table
    /// **or** the local player.
    ///
    /// MUTATION: dropping `|| local_player == Some(target)` from
    /// [`camera_target_resolvable`]. This is the mutation that survived the
    /// first battery — the rule was in a closure, so nothing reached it.
    #[test]
    fn routing_a_set_camera_resolves_the_local_player_too() {
        let t = table();
        let mut s = ClientState::default();

        // Spectate a tracked entity.
        apply(ClientStatePacket::SetCamera, &[42], &mut s, &t, Some(7));
        assert_eq!(s.camera_entity_or(Some(7)), Some(42));

        // An id nobody knows leaves it alone.
        apply(ClientStatePacket::SetCamera, &[99], &mut s, &t, Some(7));
        assert_eq!(s.camera_entity_or(Some(7)), Some(42));

        // And the server hands it back to us — id 7 is the local player and
        // is deliberately *not* in the table.
        apply(ClientStatePacket::SetCamera, &[7], &mut s, &t, Some(7));
        assert_eq!(s.camera_entity_or(Some(7)), Some(7));
    }

    /// A malformed body is reported rather than half-applied, and the state is
    /// untouched.
    ///
    /// MUTATION: `apply` returning `true` on a decode error, which would make
    /// the caller's "did this decode" answer meaningless.
    #[test]
    fn a_malformed_body_reports_false_and_changes_nothing() {
        let (t, mut s) = (table(), ClientState::default());
        // `ClientState` stopped being `Copy` when M76 gave it a `RespawnData`
        // with a `String` dimension in it.
        let before = s.clone();
        for kind in [
            ClientStatePacket::ChangeDifficulty,
            ClientStatePacket::SetCamera,
            ClientStatePacket::ContainerClose,
            ClientStatePacket::SetDefaultSpawnPosition,
        ] {
            assert!(!apply(kind, &[], &mut s, &t, Some(7)), "{kind:?}");
        }
        assert_eq!(s, before);
    }
}
