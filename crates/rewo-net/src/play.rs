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
use rewo_world::dimension::{CardinalLightType, DimensionShape, DimensionTypeDef, Skybox};
use rewo_world::physics::{self, PlayerState, TickInput};
use rewo_world::World;

use crate::ids::Ids;
use crate::spawn_info::{read_login_prefix, CommonPlayerSpawnInfo, RespawnInfo};
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
    /// Protocol id of `minecraft:warden` / `minecraft:armadillo`, resolved by
    /// the app from the entity-type registry. `ClientboundEntityEventPacket`
    /// bytes are polymorphic by entity class, so the model-visible events are
    /// interpreted only for entities of these kinds. `None` (the default, and
    /// what the headless protocol harnesses leave it) means "don't interpret
    /// entity events" — correct for tests that never render a warden.
    pub warden_type_id: Option<i32>,
    pub armadillo_type_id: Option<i32>,
    /// The Allay's protocol type id, resolved once at setup — the client
    /// disambiguates the polymorphic index-16 BOOLEAN (`DATA_DANCING` vs
    /// `DATA_BABY_ID`) by it. `None` before setup (nothing routes to dancing).
    pub allay_type_id: Option<i32>,
    /// The Pillager's protocol type id (M20) — disambiguates the index-17
    /// BOOLEAN (`IS_CHARGING_CROSSBOW`). `None` routes it nowhere.
    pub pillager_type_id: Option<i32>,
    /// The block-entity type ids whose `triggerEvent` this client implements
    /// (M26). Default-empty, which routes every `block_event` nowhere — the
    /// correct behaviour for a harness that never renders a chest, and the
    /// same "`None` means don't interpret" rule the entity-type ids above use.
    ///
    /// Load-bearing that this is a *type* set rather than a flag: `b0 == 1`
    /// means different things to a chest, a shulker box and a bell, so the
    /// type is what selects the body. See
    /// [`rewo_world::block_entities::BlockEventTypes`].
    pub block_event_types: rewo_world::block_entities::BlockEventTypes,
    /// Block-state ids of skulls whose `SkullBlock.POWERED` is true (M29).
    /// Empty means "do not tick skull animations", which is correct for a
    /// harness that renders none.
    pub powered_skull_states: std::collections::HashSet<u32>,
    /// Conduit block states, plus the per-state water and frame predicates its
    /// activation scan needs (M30). A conduit decides for itself whether it is
    /// active by looking at the blocks around it — the server sends nothing.
    pub conduit_states: std::collections::HashSet<u32>,
    pub water_states: Vec<bool>,
    pub conduit_frame_states: Vec<bool>,
    /// Which entity types are living, and which of them run `updateSwingTime`
    /// (M19) — the machine-extracted classification from `EntityTypes.java` plus
    /// the decompiled `extends` graph. It gates every swing input (a packet
    /// naming a boat mutates nothing) and decides whose swing clock advances.
    /// `None` (the headless protocol harnesses) interprets no swing packets.
    pub entity_classes: Option<std::sync::Arc<rewo_data::entity_types::EntityClasses>>,
    /// The entity-type registry, for turning a spawned entity's type id into
    /// the registry name `DefaultAttributes.SUPPLIERS` is keyed by (M52).
    /// `None` (the headless protocol harnesses) → attribute packets are
    /// recognised but store nothing, because nothing can filter them.
    pub entity_types: Option<std::sync::Arc<rewo_data::entity_types::EntityTypes>>,
    /// The `minecraft:attribute` registry plus the per-entity suppliers (M52).
    /// `None` → as above.
    pub attribute_registry: Option<std::sync::Arc<rewo_data::attributes::AttributeRegistry>>,
    /// Item → prototype swing animation + the data-component ids the equipment
    /// patch reader needs (M19). `None` (the headless protocol harnesses) →
    /// equipment packets are recognised but interpret no item, so every swing
    /// keeps the bare-hand `SwingAnimation.DEFAULT`.
    pub swing_data: Option<crate::item_stack::SwingWireData>,
    /// The `minecraft:enchantment` registry from configuration (M42), indexed
    /// by protocol id. Empty on a server that syncs none.
    pub enchantments: Vec<crate::enchantment_parse::EnchantmentDef>,
    /// The two trim registries, index = protocol id (M48).
    pub trim_materials: Vec<crate::trim_parse::TrimMaterialDef>,
    pub trim_patterns: Vec<crate::trim_parse::TrimPatternDef>,
    /// Raw mob-effect ids of haste / conduit power / mining fatigue, captured
    /// from `registry_data` — the three effects `getCurrentSwingDuration` reads.
    swing_effect_ids: crate::SwingEffectIds,
    /// Chunk global-palette bit width (from the blocks table).
    global_bits: u32,
    /// The `minecraft:dimension_type` registry in raw wire order — index *is*
    /// the holder registry id. `apply_login_shape` selects from it.
    dim_types: Vec<DimensionTypeDef>,
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
    /// Rain and thunder (M33), off `ClientboundGameEventPacket`. Cleared on a
    /// dimension change, the way a fresh `ClientLevel`'s are.
    pub weather: rewo_world::weather::WeatherState,
    /// The player's own inventory (M34). Unlike the weather this is **not**
    /// level state — vanilla's `Inventory` lives on the player, who survives a
    /// dimension change — so it is deliberately not cleared by the transition.
    /// The server re-sends the contents on respawn anyway.
    pub inventory: rewo_world::inventory::Inventory,
    /// The local player's entity id, from the login prefix (M38).
    ///
    /// `LocalPlayer` is an ordinary `LivingEntity` in vanilla, so giving it an
    /// id in the entity table's swing machine is not a trick — it is the same
    /// object the remote-player path already models. `Player.aiStep` calls
    /// `updateSwingTime`, which is why it ticks at all.
    pub player_id: Option<i32>,
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
    /// Particle spawn requests decoded from `level_particles` / `level_event`
    /// (M37), drained by the renderer each frame. Empty for the headless bot,
    /// which has no particle system.
    pub particle_events: Vec<rewo_world::particles::ParticleEvent>,
    /// Registry-id → name for `minecraft:particle_type`, so a version bump
    /// that renumbers the registry fails loud rather than spawning the wrong
    /// effect.
    particle_types: rewo_data::particle_types::ParticleTypes,
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
    colormaps: rewo_world::biome::Colormaps,
    /// Biome container global-palette width (`BiomeRegistry::global_bits`).
    biome_global_bits: u32,
    /// `CommonPlayerSpawnInfo.seed` — the `biomeZoomSeed` driving the fiddle.
    pub biome_zoom_seed: Option<i64>,
    /// `CommonPlayerSpawnInfo.seaLevel` — `ClientLevel.getSeaLevel()`, which
    /// M33's precipitation rule needs (the snow cutoff is `seaLevel + 17`).
    /// `None` before the first spawn info; the Overworld's 63 is the only
    /// sensible stand-in and is applied at the call site, not invented here.
    pub sea_level: Option<i32>,
    /// The `ResourceKey<Level>` identifier of the dimension we are actually in
    /// (`"minecraft:the_nether"`), taken from `CommonPlayerSpawnInfo.dimension`
    /// — the *level*, which is not the same thing as the dimension **type**
    /// entry's registry name. Two levels can share one type (a datapack world
    /// built on `minecraft:overworld`), so the level key is the only field that
    /// answers "which world am I in". `None` until login.
    pub active_dimension_key: Option<String>,
    /// The raw 0-based `minecraft:dimension_type` registry id login/respawn
    /// named, kept exactly as it arrived so a diagnostic can say *which slot*
    /// was selected rather than only what it resolved to. `None` until login.
    pub active_dimension_holder: Option<i32>,
    /// The dimension-type definition currently applied to `world` — the one
    /// selected by `active_dimension_holder`, cloned so the active shape,
    /// lighting contract and base sky/fog can be re-read (and compared against
    /// the next one) without re-indexing `dim_types`. `None` until login.
    pub active_dimension_type: Option<DimensionTypeDef>,
    /// Bumped every time the active dimension actually changes. Anything that
    /// caches per-dimension state (meshes, light, column buffers) can compare
    /// against a stored generation to know its cache is from a stale world.
    /// Login establishes generation 0; only a real change increments it.
    pub dimension_generation: u64,
    /// Every observed dimension change, oldest first — a diagnostic trail, not
    /// session state. Empty until the first change.
    pub dimension_transitions: Vec<DimensionTransition>,
    /// Chunk columns the decoder rejected. A non-zero count after a dimension
    /// change is the signature of a wrong vertical shape (the exact failure M16
    /// exists to prevent), so it is counted rather than only logged.
    pub chunk_decode_failures: u64,
}

/// One observed change of the active dimension, recorded for diagnostics.
///
/// This is deliberately a *record* and not a command: nothing here drives the
/// world: the fields are the small set that must be inspectable after the fact
/// when a dimension change goes wrong (which level we left, which we entered,
/// which registry slot named it, and the three properties whose disagreement
/// mis-decodes every subsequent chunk). `ClientboundRespawnPacket` is the only
/// thing that appends one.
#[derive(Clone, Debug, PartialEq)]
pub struct DimensionTransition {
    /// The level key we left, or `None` if this is the first dimension.
    pub old_key: Option<String>,
    /// The level key we entered (`CommonPlayerSpawnInfo.dimension`).
    pub new_key: String,
    /// The raw dimension-type holder the packet named.
    pub holder: i32,
    /// The registry name of the dimension **type** that holder resolved to —
    /// which is not the level key above, and is `rewo:unresolved_dimension_type/N`
    /// when the holder resolved to nothing.
    pub type_name: String,
    /// The new dimension's vertical shape — a change here invalidates every
    /// loaded column's section indexing.
    pub shape: DimensionShape,
    /// The new dimension's lighting contract.
    pub has_sky_light: bool,
    /// The new dimension's skybox.
    pub skybox: Skybox,
    /// The new dimension's `ambient_light` scalar floor.
    pub ambient_light: f32,
    /// The new dimension's cardinal-light selector.
    pub cardinal_light_type: CardinalLightType,
    /// Whether the new dimension's synced `timelines` holder set contains
    /// `minecraft:day`.
    pub has_day_timeline: bool,
    /// `dimension_generation` *after* this transition.
    pub generation: u64,

    // -- discard/reset witnesses -------------------------------------------
    //
    // These are the fields that answer "was the world we left actually thrown
    // away", and they are recorded here rather than inferred by an observer
    // because only the transition itself can see the *old* world. Coordinate
    // comparison cannot answer it: two dimensions load identical column
    // coordinates, so an old column that survived would look exactly like a
    // newly-streamed one.
    /// How many columns the world we left had loaded, counted before the
    /// replacement.
    pub old_columns: usize,
    /// How many coordinates this transition pushed onto the renderer's removal
    /// queue. Equal to `old_columns` — every column the old level held must be
    /// handed to the renderer to free, or its GPU buffer is orphaned.
    pub queued_for_removal: usize,
    /// The removal queue's length after the push. `>= queued_for_removal`; it
    /// is the queue the app actually drains, so this is what proves the
    /// coordinates landed in it.
    pub removal_queue_len: usize,
    /// Columns loaded in the replacement world, immediately after it was built.
    /// `0` — a fresh `ClientLevel` has no chunks, and this is the witness that
    /// the old map went away rather than being carried across.
    pub new_world_columns: usize,
    /// The re-mesh queue's length after the change. `0` — every entry named a
    /// column of the level we left.
    pub dirty_after: usize,
    /// Whether the world clock, its game time and the derived day tick all
    /// returned to their pre-`set_time` state, as a fresh level's do.
    pub clock_reset: bool,
}

/// What one decoded `CommonPlayerSpawnInfo` resolves the active dimension to.
#[derive(Clone, Debug, PartialEq)]
struct ActiveDimension {
    /// `CommonPlayerSpawnInfo.dimension` — the level key, verbatim.
    key: String,
    /// The raw registry id the packet named, verbatim (it may not resolve).
    holder: i32,
    /// The registry entry that id selected, or the named unresolved fallback.
    def: DimensionTypeDef,
}

/// Resolve a decoded spawn info and apply it to `world`, returning the active
/// dimension it establishes.
///
/// Split out of `apply_login_shape` for one reason: everything here is pure
/// world+registry state with no socket in sight, so the selection rules that
/// actually matter — raw id indexing, and the level key coming from the packet
/// rather than from the selected entry's registry name — are directly testable.
///
/// The two identifiers involved are genuinely different things and are kept
/// apart deliberately: `spawn.dimension_type` selects a *dimension type* by raw
/// 0-based registry id (`crate::login_dimension_type`), while `spawn.dimension`
/// is the `ResourceKey<Level>` of the world itself. Taking the key from the
/// selected entry's `name` would report `minecraft:overworld` for every
/// datapack level built on the overworld type.
fn apply_spawn_info(
    world: &mut World,
    dim_types: &[DimensionTypeDef],
    spawn: &CommonPlayerSpawnInfo,
) -> ActiveDimension {
    // One selection, one definition: the vertical shape, the lighting contract
    // and the biome layer's base sky/fog all come from the same registry entry,
    // so they cannot disagree about which dimension we are in.
    let def = crate::login_dimension_type(spawn.dimension_type, dim_types).into_owned();
    world.apply_dimension_type(&def);
    ActiveDimension {
        key: spawn.dimension.clone(),
        holder: spawn.dimension_type,
        def,
    }
}

/// Every piece of session state a **dimension change** re-points, borrowed as
/// one group.
///
/// It exists for the same reason `apply_spawn_info` does: the transition is
/// pure world + registry state with no socket in sight, so
/// `PlaySession::apply_respawn` builds one of these out of its own fields and
/// the tests build one out of plain locals. The field list is not incidental —
/// it *is* the answer to "what does entering a new dimension invalidate", and
/// anything missing from it is state that would survive into a world it was
/// never computed for.
///
/// What is deliberately **not** here is everything a respawn must preserve: the
/// synced registries, the baked assets, the connection and its chat signer, the
/// player-skin table and the session counters all belong to the *session*, not
/// to the level, and vanilla's `handleRespawn` leaves every one of them alone.
struct WorldTransition<'a> {
    world: &'a mut World,
    /// Columns queued for re-mesh, and columns the renderer must free. On a
    /// dimension change every loaded column is dropped, so the whole old
    /// coordinate set moves out of `world` into `removed` and `dirty` empties.
    dirty: &'a mut std::collections::HashSet<(i32, i32)>,
    removed: &'a mut Vec<(i32, i32)>,
    /// Replaced wholesale: its queues and touched-set hold coordinates in the
    /// *old* vertical shape.
    light: &'a mut rewo_world::light::LightEngine,
    day_ticks: &'a mut Option<i64>,
    overworld_clock: &'a mut Option<WorldClock>,
    game_time: &'a mut Option<i64>,
    /// Rain and thunder are `ClientLevel` state too — a fresh level starts
    /// clear, and stays clear until the new dimension's server sends its own
    /// `game_event`. Carrying rain into the Nether would be visible.
    weather: &'a mut rewo_world::weather::WeatherState,
    biome_zoom_seed: &'a mut Option<i64>,
    sea_level: &'a mut Option<i32>,
    /// The parsed biome registry, *retained* across the change — it is a synced
    /// global registry, not a per-level table — plus the colormaps, so the new
    /// level's `BiomeContext` is rebuilt from the same entries against the new
    /// dimension's base sky/fog and the new `biomeZoomSeed`.
    biome_registry: Option<&'a rewo_world::biome::BiomeRegistry>,
    colormaps: &'a rewo_world::biome::Colormaps,
    active_key: &'a mut Option<String>,
    active_holder: &'a mut Option<i32>,
    active_type: &'a mut Option<DimensionTypeDef>,
    generation: &'a mut u64,
    transitions: &'a mut Vec<DimensionTransition>,
}

impl WorldTransition<'_> {
    /// Apply a decoded respawn's spawn info, returning whether the active
    /// dimension actually changed — the `boolean dimensionChanged` of
    /// `ClientPacketListener.handleRespawn`, and every consequence vanilla hangs
    /// off it.
    fn apply_respawn(
        &mut self,
        dim_types: &[DimensionTypeDef],
        spawn: &CommonPlayerSpawnInfo,
    ) -> bool {
        // `boolean dimensionChanged = dimensionKey != oldDimensionKey`. Vanilla
        // compares interned `ResourceKey<Level>` identities; over the wire the
        // key *is* its identifier, so exact string equality is that same test.
        // No active dimension at all (a respawn that somehow precedes login)
        // counts as a change — there is no world we could be claiming to stay
        // in.
        if self.active_key.as_deref() == Some(spawn.dimension.as_str()) {
            // Same level. Vanilla builds no new `ClientLevel` here, so there is
            // nothing to invalidate: the columns, the entity table, the biome
            // context, the clock and the light engine all belong to a level that
            // did not go anywhere, and neither the generation counter nor the
            // transition history moves (this is not a transition).
            //
            // Nothing re-reads `spawn.dimension_type` either. The packet still
            // carries a holder, but applying it would re-point the *shape* and
            // the lighting contract behind chunks that were decoded against the
            // old ones — the exact mis-decode this milestone exists to prevent.
            // Vanilla's old `ClientLevel` keeps the `DimensionType` it was
            // constructed with, so `active_holder` / `active_type` keep it too.
            //
            // `spawn.seed()` is likewise retained rather than applied: the seed
            // is the level's `biomeZoomSeed`, fixed at `new ClientLevel(...)`,
            // and no new level was built. Storing the packet's seed while the
            // `BiomeContext` still holds the old one would make
            // `biome_zoom_seed` disagree with the tint actually being drawn.
            return false;
        }

        // Every loaded column belongs to the world we are leaving. The whole
        // coordinate set is collected *before* the replacement — after it the
        // old map is gone, and the renderer would hold one orphaned GPU buffer
        // per column it was never told about.
        //
        // The three counts are the transition's own witnesses: nothing outside
        // this function can see the world we are leaving, and no coordinate
        // comparison could stand in for them (both dimensions load column 0,0).
        let old_columns = self.world.loaded_columns();
        let removal_queue_before = self.removed.len();
        self.removed.extend(self.world.column_coords());
        let removal_queue_len = self.removed.len();
        let queued_for_removal = removal_queue_len - removal_queue_before;
        self.dirty.clear();

        // One selection for the whole change, exactly as at login: shape,
        // lighting contract and base sky/fog come from a single registry entry.
        let def = crate::login_dimension_type(spawn.dimension_type, dim_types).into_owned();
        // `this.level = new ClientLevel(...)`: a fresh, empty level bound to the
        // new dimension type. Replacing the whole struct rather than re-pointing
        // the old one is what drops the old columns *and* the old entity table
        // in one move — the entities we were tracking live in the world we left,
        // exactly as vanilla's discarded `ClientLevel` takes them with it.
        *self.world = World::for_dimension(&def);
        // The light engine's queues and touched-set are coordinates in the old
        // vertical shape; a fresh engine is the only correct carry-over.
        *self.light = rewo_world::light::LightEngine::new();
        // The clock is `ClientLevel` state and goes with the level: the world
        // clock, the game time it integrates against, and the day-tick derived
        // from them. The new level has not been told a time yet, so all three
        // return to their pre-`set_time` state and the renderer reads full
        // daylight until the next sync arrives.
        *self.day_ticks = None;
        *self.overworld_clock = None;
        *self.game_time = None;
        self.weather.clear();

        // `spawnInfo.seed()` is the new level's `biomeZoomSeed`.
        *self.biome_zoom_seed = Some(spawn.seed);
        *self.sea_level = Some(spawn.sea_level);
        attach_biome_context(
            self.world,
            self.biome_registry,
            self.colormaps,
            &def,
            spawn.seed,
        );

        let old_key = self.active_key.replace(spawn.dimension.clone());
        *self.active_holder = Some(spawn.dimension_type);
        // A counter, not an index: its overflow is a wrap rather than a panic.
        // A cache comparing itself against a wrapped generation is no worse off
        // than one comparing against any other stale value.
        *self.generation = self.generation.wrapping_add(1);
        self.transitions.push(DimensionTransition {
            old_key,
            new_key: spawn.dimension.clone(),
            holder: spawn.dimension_type,
            type_name: def.name.clone(),
            shape: def.shape,
            has_sky_light: def.has_sky_light,
            skybox: def.skybox,
            ambient_light: def.ambient_light,
            cardinal_light_type: def.cardinal_light_type,
            has_day_timeline: def.has_day_timeline,
            generation: *self.generation,
            old_columns,
            queued_for_removal,
            removal_queue_len,
            new_world_columns: self.world.loaded_columns(),
            dirty_after: self.dirty.len(),
            clock_reset: self.day_ticks.is_none()
                && self.overworld_clock.is_none()
                && self.game_time.is_none(),
        });
        *self.active_type = Some(def);
        true
    }
}

/// The local-player state one respawn recreates, borrowed as one group — the
/// `newPlayer` half of `handleRespawn`, kept apart from [`WorldTransition`]
/// because vanilla runs it on **both** paths, changed dimension or not.
///
/// `newPlayer` is a genuinely fresh `LocalPlayer`, so the default for every
/// field here is the constructor's value; only what `handleRespawn` explicitly
/// copies over survives. Two of vanilla's carry-overs have no Rewo
/// representation at all and are documented no-ops rather than guessed at:
///
/// * `dataToKeep` bit 1 (`KEEP_ATTRIBUTE_MODIFIERS`) chooses between
///   `assignAllValues` and `assignBaseValues` on the player's `AttributeMap`.
///   Rewo has no attribute map — movement speed and the rest are the physics
///   port's constants — so neither branch has anything to assign.
/// * bit 2's `getEntityData().assignValues(...)` copies the old player's
///   non-default `SynchedEntityData` entries. Rewo does not model a
///   `SynchedEntityData` map for the *local* player (`metadata` is parsed for
///   other entities only), but it does hold one of its entries as a plain
///   field: **health** is `LivingEntity.DATA_HEALTH_ID`, so bit 2 carries it
///   over and is honoured below. Food is not synched data — it lives in
///   `FoodData`, which `handleRespawn` never copies — so it always resets to the
///   fresh player's value. Bit 2's other three statements — delta movement and
///   both rotations — are represented exactly and are applied below.
struct LocalPlayerRespawn<'a> {
    player: &'a mut PlayerState,
    health: &'a mut f32,
    food: &'a mut i32,
    dead: &'a mut bool,
    spawned: &'a mut bool,
    // `LocalPlayer`'s own `sendPosition` bookkeeping.
    last_pos: &'a mut (f64, f64, f64),
    last_rot: &'a mut (f32, f32),
    reminder: &'a mut u32,
    last_on_ground: &'a mut bool,
    last_horiz: &'a mut bool,
    last_input_flags: &'a mut u8,
}

impl LocalPlayerRespawn<'_> {
    /// `keep_entity_data` is `packet.shouldKeep((byte)2)`.
    fn apply(&mut self, keep_entity_data: bool) {
        // Position is not copied on *either* path: the new entity is built at
        // `Vec3.ZERO`, and the no-keep path's `resetPos` only ever scans upward
        // from there. That scan (`while !noCollision`) is not reproduced — it
        // queries collision against a level that, on a dimension change, has no
        // chunks at all, where vanilla's loop breaks on its first iteration and
        // leaves exactly this origin. The `player_position` teleport the server
        // sends next is authoritative either way, and `spawned` is cleared below
        // so nothing is sent from the origin in the meantime.
        self.player.x = 0.0;
        self.player.y = 0.0;
        self.player.z = 0.0;
        // A fresh entity has not collided with anything yet.
        self.player.on_ground = false;
        self.player.horizontal_collision = false;
        if !keep_entity_data {
            // `resetPos()` → `setDeltaMovement(Vec3.ZERO)` and `setXRot(0.0F)`,
            // then `handleRespawn`'s own `setYRot(-180.0F)`.
            self.player.vx = 0.0;
            self.player.vy = 0.0;
            self.player.vz = 0.0;
            self.player.pitch = 0.0;
            self.player.yaw = -180.0;
        }
        // else: `setDeltaMovement(oldPlayer.getDeltaMovement())`, `setYRot`,
        // `setXRot` — velocity and both rotations survive verbatim, so there is
        // nothing to write.

        // `xLast`/`yLast`/`zLast`, `yRotLast`/`xRotLast`, `positionReminder`,
        // `lastOnGround` and `lastHorizontalCollision` are plain fields of the
        // new `LocalPlayer`, so they start at zero on both paths — including the
        // keep path, where the preserved rotation therefore reads as "rotated"
        // on the next tick and is re-sent. `lastSentInput` is the one that is
        // not: the constructor takes `oldPlayer.getLastSentInput()` when bit 2
        // is set and `Input.EMPTY` otherwise.
        *self.last_pos = (0.0, 0.0, 0.0);
        *self.last_rot = (0.0, 0.0);
        *self.reminder = 0;
        *self.last_on_ground = false;
        *self.last_horiz = false;
        if !keep_entity_data {
            *self.last_input_flags = 0;
        }

        // A fresh `LocalPlayer` starts at full health with a fresh `FoodData`,
        // and its `deathTime` is 0. Health is the one of those three that bit 2
        // reaches: it is a `SynchedEntityData` entry
        // (`LivingEntity.DATA_HEALTH_ID`), so `assignValues` carries the old
        // player's value across verbatim when the bit is set. `FoodData` is a
        // plain field of `Player` that `handleRespawn` never touches, so food is
        // always the fresh 20. The server's `set_health` re-states both
        // immediately either way.
        if !keep_entity_data {
            *self.health = 20.0;
        }
        *self.food = 20;
        *self.dead = false;
        // `setClientLoaded(false)` + `startWaitingForNewLevel`: the client is
        // not a live participant again until the level is ready, which for us is
        // the `player_position` teleport that follows. Holding `spawned` low
        // until then keeps physics from running against the origin above, keeps
        // movement from being sent from it, and keeps the respawn teleport out
        // of the `corrections` physics-parity meter.
        *self.spawned = false;
    }
}

/// Build the `BiomeContext` from the synced registry + one dimension's base
/// sky/fog + a `biomeZoomSeed`, and attach it to `world`. The registry is cloned
/// rather than consumed, so this is idempotent and a later dimension change can
/// rebuild against the same entries.
///
/// A free function (not a `PlaySession` method) so the respawn transition can
/// call it with no session in reach.
fn attach_biome_context(
    world: &mut World,
    registry: Option<&rewo_world::biome::BiomeRegistry>,
    colormaps: &rewo_world::biome::Colormaps,
    def: &DimensionTypeDef,
    seed: i64,
) {
    let Some(reg_template) = registry else {
        return;
    };
    let mut reg = reg_template.clone();
    // `None` stays `None`: the Nether sets neither, and the biome layer
    // must fall through to its own colour rather than to opaque black.
    reg.dimension_sky = def.sky_color;
    reg.dimension_fog = def.fog_color;
    let ctx =
        rewo_world::biome::BiomeContext::new(std::sync::Arc::new(reg), colormaps.clone(), seed);
    world.set_biome_context(std::sync::Arc::new(ctx));
    log::info!("net: biome context attached (seed={seed})");
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

        // A plain Overworld placeholder, and deliberately *only* that: the
        // dimension registry is synced by now, but the login packet has not yet
        // said which entry we joined, and registry slot 0 is not the join
        // dimension in any guaranteed sense (a vanilla server may send the
        // Nether first). Applying slot 0 here would dress a guess up as a
        // resolved dimension; the placeholder cannot, because no chunk can
        // arrive before `apply_login_shape` replaces it and the active-dimension
        // fields below stay `None` until it does.
        let world = World::new(DimensionShape::OVERWORLD);
        let dim_types = self.dim_types.clone();
        let overworld_clock_id = self.overworld_clock_id;
        let visual_effects =
            crate::effects::VisualEffects::new(self.night_vision_id, self.darkness_id);
        let swing_effect_ids = self.swing_effect_ids;
        // The enchantment registry, in wire order (M42) — the index is the
        // protocol id a component patch carries.
        let enchantments = std::mem::take(&mut self.enchantments);
        let trim_materials = std::mem::take(&mut self.trim_materials);
        let trim_patterns = std::mem::take(&mut self.trim_patterns);
        // Biome registry parsed during configuration; the `biomeZoomSeed` +
        // dimension holder arrive with the play-login packet (`apply_login_shape`).
        // Access the field directly (not a `&self` method) — `self.stream` was
        // already moved by `split()`, so `self` is partially moved here.
        let pending_biome_registry = if self.biome_defs.is_empty() {
            None
        } else {
            Some(rewo_world::biome::BiomeRegistry::new(
                self.biome_defs.clone(),
            ))
        };
        let biome_global_bits = pending_biome_registry
            .as_ref()
            .map(|r| r.global_bits)
            .unwrap_or(7);
        let mut session = PlaySession {
            writer,
            codec,
            rx,
            ids: self.ids,
            enchantments,
            trim_materials,
            trim_patterns,
            world,
            player: PlayerState::at(0.5, 80.0, 0.5),
            collide,
            entity_push: Vec::new(),
            warden_type_id: None,
            armadillo_type_id: None,
            allay_type_id: None,
            pillager_type_id: None,
            block_event_types: Default::default(),
            powered_skull_states: Default::default(),
            conduit_states: Default::default(),
            water_states: Vec::new(),
            conduit_frame_states: Vec::new(),
            entity_classes: None,
            entity_types: None,
            attribute_registry: None,
            swing_data: None,
            swing_effect_ids,
            global_bits,
            dim_types,
            overworld_clock_id,
            spawned: false,
            corrections: 0,
            teleports: 0,
            block_updates: 0,
            day_ticks: None,
            weather: rewo_world::weather::WeatherState::default(),
            inventory: rewo_world::inventory::Inventory::default(),
            player_id: None,
            overworld_clock: None,
            game_time: None,
            dirty: std::collections::HashSet::new(),
            light: rewo_world::light::LightEngine::new(),
            light_emission: Vec::new(),
            light_dampening: Vec::new(),
            light_faces: Vec::new(),
            removed: Vec::new(),
            particle_events: Vec::new(),
            particle_types: self.data.particle_types.clone(),
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
            colormaps,
            biome_global_bits,
            biome_zoom_seed: None,
            sea_level: None,
            // No dimension is active until the login packet names one.
            active_dimension_key: None,
            active_dimension_holder: None,
            active_dimension_type: None,
            dimension_generation: 0,
            dimension_transitions: Vec::new(),
            chunk_decode_failures: 0,
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

    /// The world clock's `gameTime`, or 0 before the server has sent one.
    ///
    /// This is the input every block-entity animation keys off (M29): a
    /// banner's sway hashes it with the block position, a pot's wobble is
    /// timed from the tick its event arrived on. Zero before the first
    /// `set_time` is a resting world rather than a wrong one — every formula
    /// here is well defined at t=0.
    pub fn game_time(&self) -> i64 {
        self.game_time.unwrap_or(0)
    }

    /// The synced `minecraft:dimension_type` registry in raw wire order — index
    /// *is* the holder id. Read-only: nothing outside the session may select a
    /// dimension, but a gate has to be able to check that the holder a packet
    /// named really is the slot it resolved to, **derived from this registry**
    /// rather than from an assumed numeric order.
    pub fn dimension_types(&self) -> &[DimensionTypeDef] {
        &self.dim_types
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
        // M38: publish what the local player is holding into the entity table,
        // so its swing runs through M19's machine like any other entity's.
        //
        // The server never tells us our own equipment — `set_equipment` is for
        // *other* entities — but M34's inventory knows, and the swing duration
        // is a function of the held item. This is the join between the two.
        self.publish_local_hands();
        // Step other entities' 3-tick position lerps (vanilla cadence).
        self.world.entities.tick_lerp();
        // `ChestLidController.tickLid` — the client animates the ten ticks the
        // server never sends (M25c).
        self.world.block_entities.tick_lids();
        // `SkullBlockEntity.animation` — the counter a piglin head's ears and
        // a dragon head's jaw run on, which advances only while the block
        // state is POWERED (M29).
        self.world
            .tick_skull_animations(&self.powered_skull_states);
        // `ConduitBlockEntity.clientTick` — the activation scan is the client's
        // own work, so this is the one animation whose *state* Rewo derives
        // rather than receives (M30).
        let gt = self.game_time.unwrap_or(0);
        self.world.tick_conduits(
            &self.conduit_states,
            &self.water_states,
            &self.conduit_frame_states,
            gt,
        );
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
        } else if crate::route_set_entity_data(
            id,
            body,
            ids,
            &mut self.world.entities,
            crate::MetaKinds {
                allay: self.allay_type_id,
                pillager: self.pillager_type_id,
                classes: self.entity_classes.as_deref(),
                components: self.swing_data.as_ref().map(|d| d.components),
            },
        ) {
            // Entity metadata (custom name, pose, gesture state, cube size, and
            // the polymorphic index-16 BOOLEAN → Allay dancing / baby). The
            // Allay dance counters then advance in `tick_lerp`.
        } else if crate::route_damage_event(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M21: the damage response — arms the hurt clock (red overlay) and
            // kicks the walk animation, for a tracked living entity only.
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
        } else if crate::route_inventory(
            id,
            body,
            ids,
            self.swing_data.as_ref().map(|d| d.components),
            &mut self.inventory,
        ) {
            // M34: the player's own inventory — contents, one slot, or the
            // server moving the selection.
        } else if crate::route_game_event(id, body, ids, &mut self.weather) {
            // M33: rain and thunder levels. The packet also carries a dozen
            // non-weather events; those match the id and change nothing.
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

    /// `ClientboundLoginPacket`: establish the active dimension.
    ///
    /// Fallible on purpose. Everything downstream of this packet — the vertical
    /// shape every chunk is decoded against, the lighting contract, the biome
    /// layer's base sky/fog — is derived from bytes read here, so a body we
    /// cannot decode has exactly one safe outcome: fail, rather than leave the
    /// pre-login Overworld placeholder in place while the server starts sending
    /// chunks for something else. The caller propagates the error and the
    /// `player_loaded` reply is never sent.
    fn apply_login_shape(&mut self, body: &[u8]) -> Result<(), String> {
        let mut r = PacketReader::new(body);
        // The prefix ends on the first byte of the embedded
        // `CommonPlayerSpawnInfo` and yields the local player's entity id (only
        // effects targeting it drive the camera lightmap).
        let player_id =
            read_login_prefix(&mut r).map_err(|e| format!("play login: prefix: {e}"))?;
        // One shared spawn-info decoder for login and respawn — no second,
        // partial decoder anywhere: the dimension holder is
        // `DimensionType.STREAM_CODEC = ByteBufCodecs.holderRegistry` =
        // `idMapper`, i.e. the **raw 0-based registry id** with NO inline/`id+1`
        // case (unlike `ByteBufCodecs.holder`), and `seed` is the
        // `biomeZoomSeed`.
        let spawn = CommonPlayerSpawnInfo::read(&mut r)
            .map_err(|e| format!("play login: spawn info: {e}"))?;
        self.visual_effects.set_player_id(player_id);
        self.player_id = Some(player_id);
        let active = apply_spawn_info(&mut self.world, &self.dim_types, &spawn);
        self.biome_zoom_seed = Some(spawn.seed);
        self.sea_level = Some(spawn.sea_level);
        self.build_biome_context(&active.def, spawn.seed);
        log::info!(
            "net: play login — level {} on dimension type {} (holder {}): min_y={} \
             height={} sky_light={} skybox={:?} cardinal={}",
            active.key,
            active.def.name,
            active.holder,
            active.def.shape.min_y,
            active.def.shape.height,
            active.def.has_sky_light,
            active.def.skybox,
            active.def.cardinal_light_type.name(),
        );
        // Login *establishes* the active dimension at generation 0; it is not a
        // transition, so neither the counter nor the history moves. Only a later
        // change — a respawn that names a different level — records one.
        self.active_dimension_key = Some(active.key);
        self.active_dimension_holder = Some(active.holder);
        self.active_dimension_type = Some(active.def);
        Ok(())
    }

    /// `ClientboundRespawnPacket` — the mid-session twin of `apply_login_shape`,
    /// and the only packet that changes which dimension we are in.
    ///
    /// Fallible for the same reason login is: everything downstream of this
    /// packet is derived from bytes read here. The whole body is decoded
    /// **first**, through the one shared `RespawnInfo::parse` (which rejects a
    /// short body *and* a long one), and nothing is applied until it succeeds —
    /// so a malformed respawn leaves the session in the dimension it was already
    /// in rather than half-way into a new one. The caller propagates the error.
    fn apply_respawn(&mut self, body: &[u8]) -> Result<(), String> {
        let info = RespawnInfo::parse(body).map_err(|e| format!("play respawn: {e}"))?;
        let spawn = &info.spawn;
        let changed = WorldTransition {
            world: &mut self.world,
            dirty: &mut self.dirty,
            removed: &mut self.removed,
            light: &mut self.light,
            day_ticks: &mut self.day_ticks,
            overworld_clock: &mut self.overworld_clock,
            game_time: &mut self.game_time,
            weather: &mut self.weather,
            biome_zoom_seed: &mut self.biome_zoom_seed,
            sea_level: &mut self.sea_level,
            biome_registry: self.pending_biome_registry.as_ref(),
            colormaps: &self.colormaps,
            active_key: &mut self.active_dimension_key,
            active_holder: &mut self.active_dimension_holder,
            active_type: &mut self.active_dimension_type,
            generation: &mut self.dimension_generation,
            transitions: &mut self.dimension_transitions,
        }
        .apply_respawn(&self.dim_types, spawn);

        // The local player is recreated on both paths — vanilla's `newPlayer`
        // is built before the `dimensionChanged` branch is over with.
        LocalPlayerRespawn {
            player: &mut self.player,
            health: &mut self.health,
            food: &mut self.food,
            dead: &mut self.dead,
            spawned: &mut self.spawned,
            last_pos: &mut self.last_pos,
            last_rot: &mut self.last_rot,
            reminder: &mut self.reminder,
            last_on_ground: &mut self.last_on_ground,
            last_horiz: &mut self.last_horiz,
            last_input_flags: &mut self.last_input_flags,
        }
        .apply(info.should_keep(RespawnInfo::KEEP_ENTITY_DATA));

        // Same reason, same both-paths placement: the fresh `LocalPlayer` has
        // an empty `activeEffects` and a `tickCount` of 0. Neither is
        // `SynchedEntityData`, so `dataToKeep` bit 2 cannot carry them and the
        // server re-sends whatever effects still apply. The registry ids and the
        // (unchanged) local entity id are kept.
        self.visual_effects.reset_for_respawn();

        if changed {
            let def = self
                .active_dimension_type
                .as_ref()
                .expect("a changed dimension always sets the active type");
            log::info!(
                "net: respawn — dimension change to level {} on dimension type {} \
                 (holder {}): min_y={} height={} sky_light={} skybox={:?} \
                 cardinal={} generation={} data_to_keep={}",
                spawn.dimension,
                def.name,
                spawn.dimension_type,
                def.shape.min_y,
                def.shape.height,
                def.has_sky_light,
                def.skybox,
                def.cardinal_light_type.name(),
                self.dimension_generation,
                info.data_to_keep,
            );
        } else {
            log::info!(
                "net: respawn — same level {} retained (data_to_keep={})",
                spawn.dimension,
                info.data_to_keep,
            );
        }
        // `notifyPlayerLoaded`: `setClientLoaded(false)` above means the next
        // ready level announces itself again. Rewo has no level-loading tracker,
        // so — exactly as on the login path — the announcement goes out as soon
        // as the packet is applied.
        let p = PacketWriter::packet(self.ids.sb_play_player_loaded);
        self.send(p)
    }

    /// Attach the join dimension's `BiomeContext`. A dimension change rebuilds
    /// it through the same [`attach_biome_context`], so the base sky/fog always
    /// belongs to the dimension we are actually in.
    fn build_biome_context(&mut self, def: &DimensionTypeDef, seed: i64) {
        attach_biome_context(
            &mut self.world,
            self.pending_biome_registry.as_ref(),
            &self.colormaps,
            def,
            seed,
        );
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


    /// `ServerboundContainerClickPacket` for a `ContainerInput.PICKUP` (M35).
    ///
    /// ```text
    /// ContainerId  containerId    // VarInt
    /// VarInt       stateId
    /// Short        slotNum        // signed, among var-ints
    /// Byte         buttonNum
    /// ContainerInput               // idMapper: PICKUP = 0
    /// Map<Short, HashedStack>      // the client's PREDICTION, max 128
    /// HashedStack  carriedItem
    /// ```
    ///
    /// The changed-slot map is the load-bearing part and the reason this takes
    /// a prediction rather than a slot number. The server replays the click
    /// against its own container and compares every entry with
    /// `HashedStack.matches`; one disagreement and it resynchronises the whole
    /// container. So a client that cannot predict must not click.
    ///
    /// `HashedStack` is `Optional<(item, count, HashedPatchMap)>`, and
    /// `HashedPatchMap` is a map of component type to a hash plus a set of
    /// removed types. Rewo only ever writes both empty, because it decodes
    /// *whether* a stack carried components but not what they were — see
    /// [`ItemSlot::has_components`]. A click that moves a component-bearing
    /// stack is therefore predicted honestly but hashed wrongly, and the
    /// server corrects it; the alternative, refusing to move a damaged
    /// pickaxe at all, is worse.
    pub fn container_click(
        &mut self,
        prediction: &rewo_world::inventory::ClickPrediction,
    ) -> Result<(), String> {
        self.container_click_input(prediction, CONTAINER_INPUT_PICKUP)
    }

    /// [`Self::container_click`] with the `ContainerInput` named — PICKUP for a
    /// plain click, QUICK_MOVE for a shift-click.
    pub fn container_click_input(
        &mut self,
        prediction: &rewo_world::inventory::ClickPrediction,
        input: i32,
    ) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_container_click else {
            return Err("container_click unavailable".into());
        };
        // `MAX_SLOT_COUNT` in the packet's own stream codec. A PICKUP touches
        // at most one slot, so exceeding it means the prediction is wrong
        // about something more basic.
        const MAX_SLOT_COUNT: usize = 128;
        if prediction.changed.len() > MAX_SLOT_COUNT {
            return Err(format!(
                "container_click: {} changed slots exceeds the wire's {MAX_SLOT_COUNT}",
                prediction.changed.len()
            ));
        }
        let mut p = PacketWriter::packet(id);
        p.varint(rewo_world::inventory::PLAYER_CONTAINER_ID);
        p.varint(self.inventory.state_id());
        p.u16(prediction.slot as u16);
        p.i8(prediction.button);
        p.varint(input);
        p.varint(prediction.changed.len() as i32);
        for &(slot, value) in &prediction.changed {
            p.u16(slot);
            write_hashed_stack(&mut p, value);
        }
        write_hashed_stack(&mut p, prediction.carried);
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

    /// Mirror the local player's two hands into the entity table.
    ///
    /// Cheap enough to run every tick — two slot reads and, when nothing
    /// changed, two map writes of an equal value. Doing it on change instead
    /// would need every mutation path (`set_held_slot`, a container update, a
    /// click prediction) to remember to call it.
    fn publish_local_hands(&mut self) {
        let Some(id) = self.player_id else {
            return;
        };
        use rewo_world::entities::{HandItem, InteractionHand};
        let resolve = |slot: Option<rewo_world::inventory::ItemSlot>| -> HandItem {
            let Some(stack) = slot else {
                return HandItem::Empty;
            };
            let Some(data) = self.swing_data.as_ref() else {
                // Without the prototype tables the swing duration is
                // unknowable, and an unknown hand is exactly what
                // `current_swing_duration` needs to see — it must not fall back
                // to the default and pose a spear like a fist.
                return HandItem::Unknown;
            };
            match rewo_data::swing_anim::SwingAnimations::of(&data.prototypes, stack.item_id) {
                Some(swing) => HandItem::Held(rewo_world::entities::HeldItem {
                    item_id: stack.item_id,
                    swing,
                    use_profile: data.use_profiles.of(stack.item_id).unwrap_or_default(),
                    charged: false,
                    // This is the *local* player's own inventory stack, which
                    // reaches here as `(id, count)` and carries no patch — so
                    // no foil. The hand's own glint (M44) reads the inventory
                    // directly and does not come through here.
                    glint: false,
                }),
                None => HandItem::Unknown,
            }
        };
        let main = resolve(self.inventory.held());
        let off = resolve(self.inventory.offhand());
        self.world
            .entities
            .set_hand_item(id, rewo_world::entities::InteractionHand::MainHand, main);
        self.world
            .entities
            .set_hand_item(id, rewo_world::entities::InteractionHand::OffHand, off);
    }

    /// `LocalPlayer.getAttackAnim(partialTicks)` — the first-person swing
    /// (M38), and 0 before login or when the id has no swing yet.
    pub fn local_attack_anim(&self, partial: f32) -> f32 {
        self.player_id
            .map_or(0.0, |id| self.world.entities.attack_anim(id, partial))
    }


    /// `ServerboundUseItemPacket` — using the held item at nothing in
    /// particular (M38): eating, drawing a bow, raising a shield.
    ///
    /// Distinct from `use_item_on`, which targets a block. Vanilla sends this
    /// one when the right button goes down and the pick ray hits nothing
    /// usable, and it is what starts every use-driven arm pose.
    ///
    /// ```text
    /// rewo_world::entities::InteractionHand hand    // VarInt enum: 0 main, 1 off
    /// VarInt          sequence
    /// float           yRot
    /// float           xRot
    /// ```
    ///
    /// The two rotations are the player's look at the moment of use — the
    /// server replays the interaction against them rather than against
    /// whatever the last movement packet said.
    pub fn use_item(&mut self, hand: rewo_world::entities::InteractionHand) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_use_item else {
            return Err("use_item unavailable".into());
        };
        let seq = self.next_sequence();
        let mut p = PacketWriter::packet(id);
        p.varint(match hand {
            rewo_world::entities::InteractionHand::MainHand => 0,
            rewo_world::entities::InteractionHand::OffHand => 1,
        });
        p.varint(seq);
        p.f32(self.player.yaw);
        p.f32(self.player.pitch);
        self.send(p)
    }

    /// `ServerboundPlayerActionPacket` with `RELEASE_USE_ITEM`.
    ///
    /// The position and face are ignored by the server for this action but are
    /// still part of the packet, so vanilla sends `BlockPos.ZERO` and `DOWN`.
    fn release_use_item(&mut self) -> Result<(), String> {
        let seq = self.next_sequence();
        let mut p = PacketWriter::packet(self.ids.sb_play_player_action);
        p.varint(5); // RELEASE_USE_ITEM
        write_position(&mut p, 0, 0, 0);
        p.u8(0); // DOWN
        p.varint(seq);
        self.send(p)
    }

    /// Start using the held item — the local half of `startUsingItem` (M38).
    ///
    /// Drives the **same** door the remote-entity path uses:
    /// `LivingEntity.startUsingItem` sets shared-flag bit 0 (and bit 1 for the
    /// off hand), which is exactly what `set_living_flags` decodes. So the
    /// local player's use clock is M23's machine with an id, in the way its
    /// swing is M19's — no second implementation, and every rule the flags
    /// path already encodes (a repeat does not restart, an unresolvable stack
    /// latches nothing) applies unchanged.
    ///
    /// Returns whether a use actually began: an item with no use duration —
    /// most of them — is not usable, and vanilla plays no pose for it.
    pub fn start_use(&mut self, hand: rewo_world::entities::InteractionHand) -> Result<bool, String> {
        let Some(id) = self.player_id else {
            return Ok(false);
        };
        // `Item.getUseDuration() == 0` means the item cannot be used at all,
        // which is the overwhelming majority. Checking here keeps a pickaxe
        // from sending a use packet on every right click.
        let usable = self
            .world
            .entities
            .hand_item(id, hand)
            .use_profile()
            .is_some_and(|p| p.duration > 0);
        if !usable {
            return Ok(false);
        }
        self.use_item(hand)?;
        let flags = 1 | if hand == rewo_world::entities::InteractionHand::OffHand { 2 } else { 0 };
        self.world.entities.set_living_flags(id, flags);
        Ok(true)
    }

    /// Stop using it — `releaseUsingItem`. Idempotent, so a mouse release with
    /// no use in progress costs one packet and nothing else.
    pub fn stop_use(&mut self) -> Result<(), String> {
        let Some(id) = self.player_id else {
            return Ok(());
        };
        if !self.world.entities.use_state(id).using {
            return Ok(());
        }
        self.world.entities.set_living_flags(id, 0);
        self.release_use_item()
    }

    /// `LivingEntity.getUseItemRemainingTicks()` for the local player, and the
    /// hand it is using — what the first-person use poses are keyed on.
    pub fn local_use_state(&self) -> rewo_world::entities::UseState {
        self.player_id
            .map(|id| self.world.entities.use_state(id))
            .unwrap_or_default()
    }

    pub fn swing(&mut self) -> Result<(), String> {
        // Vanilla's `LocalPlayer.swing` runs `super.swing` *and* sends the
        // packet, so the animation starts locally rather than waiting for the
        // server to echo it back — which it never does for your own swings.
        if let Some(id) = self.player_id {
            self.world.entities.swing(
                id,
                rewo_world::entities::InteractionHand::MainHand,
                true,
            );
        }
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
mod login_dimension_tests {
    use super::*;
    use crate::dimension_parse::builtin as fx;
    use crate::spawn_info::GlobalPos;

    /// A registry in a **deliberately non name-sorted** wire order: the Nether
    /// is holder 0 and the Overworld is holder 2. Any name-keyed shortcut in the
    /// selection path fails immediately here.
    fn registry() -> Vec<DimensionTypeDef> {
        crate::dimension_parse::parse_dimension_registry_packet(&fx::registry_packet(&[
            ("minecraft:the_nether", fx::the_nether()),
            ("minecraft:the_end", fx::the_end()),
            ("minecraft:overworld", fx::overworld()),
        ]))
        .expect("fixture registry must parse")
        .expect("packet is the dimension_type registry")
    }

    /// A spawn info naming `level` on dimension-type holder `holder`, with every
    /// other field filled in so nothing about the case under test depends on a
    /// default.
    fn spawn(holder: i32, level: &str) -> CommonPlayerSpawnInfo {
        CommonPlayerSpawnInfo {
            dimension_type: holder,
            dimension: level.into(),
            seed: 0x0bad_f00d_dead_beefu64 as i64,
            game_type: 1,
            previous_game_type: Some(0),
            is_debug: false,
            is_flat: false,
            last_death_location: Some(GlobalPos {
                dimension: "minecraft:overworld".into(),
                x: -3,
                y: -59,
                z: 7,
            }),
            portal_cooldown: 0,
            sea_level: 63,
        }
    }

    /// The pre-login world is a plain Overworld placeholder, so a test that
    /// lands on the Nether cannot pass by accident.
    fn placeholder_world() -> World {
        let world = World::new(DimensionShape::OVERWORLD);
        assert_eq!(world.shape, DimensionShape::OVERWORLD);
        assert!(world.has_sky_light());
        world
    }

    /// Raw holder 0 is the **first synced entry**, never "inline" and never a
    /// default: here that entry is the Nether, so the resolved shape is 0..256
    /// with no skylight rather than the placeholder's -64..320 with skylight.
    #[test]
    fn raw_holder_zero_selects_the_first_entry_even_when_it_is_the_nether() {
        let defs = registry();
        assert_eq!(defs[0].name, "minecraft:the_nether", "fixture precondition");
        let mut world = placeholder_world();
        let active = apply_spawn_info(&mut world, &defs, &spawn(0, "minecraft:the_nether"));

        assert_eq!(active.holder, 0);
        assert_eq!(active.def.name, "minecraft:the_nether");
        assert_eq!(active.def.shape, DimensionShape::NETHER);
        assert!(!active.def.has_sky_light);
        assert_eq!(active.def.skybox, Skybox::None);
        // …and the world is now decoding chunks against exactly that.
        assert_eq!(world.shape, DimensionShape::NETHER);
        assert_ne!(world.shape, DimensionShape::OVERWORLD);
        assert!(!world.has_sky_light());
        assert_eq!(
            world.cardinal_light_type(),
            rewo_world::dimension::CardinalLightType::Nether
        );
    }

    /// The active level key is `CommonPlayerSpawnInfo.dimension`, NOT the
    /// selected dimension **type**'s registry name. A datapack level built on
    /// the vanilla overworld type shares that type's name with
    /// `minecraft:overworld` — reading the key off `def.name` would report the
    /// wrong world for every such level, and would be indistinguishable from
    /// correct on a vanilla-only server.
    #[test]
    fn active_level_key_comes_from_the_spawn_info_not_the_type_name() {
        let defs = registry();
        let mut world = placeholder_world();
        let active = apply_spawn_info(&mut world, &defs, &spawn(2, "rewo:mining_world"));

        assert_eq!(active.key, "rewo:mining_world");
        assert_eq!(active.def.name, "minecraft:overworld");
        assert_ne!(
            active.key, active.def.name,
            "key is the level, not the type"
        );
        // The type still resolved normally — the two identifiers are separate,
        // not alternatives.
        assert_eq!(active.holder, 2);
        assert_eq!(world.shape, DimensionShape::OVERWORLD);
        assert!(world.has_sky_light());
    }

    /// A holder the synced registry does not contain degrades to the *named*
    /// unresolved fallback, and the packet's own level key and holder id survive
    /// verbatim — the diagnostic must still say which world the server claimed.
    #[test]
    fn an_unresolved_holder_keeps_the_packets_key_and_holder() {
        let defs = registry();
        let mut world = placeholder_world();
        let active = apply_spawn_info(&mut world, &defs, &spawn(99, "rewo:mining_world"));

        assert_eq!(active.key, "rewo:mining_world");
        assert_eq!(active.holder, 99);
        assert_eq!(active.def.name, "rewo:unresolved_dimension_type/99");
        assert!(
            defs.iter().all(|d| d.name != active.def.name),
            "the fallback must never claim to be a synced entry"
        );
        assert_eq!(active.def.shape, DimensionShape::OVERWORLD);
    }
}

#[cfg(test)]
mod respawn_tests {
    use super::*;
    use crate::dimension_parse::builtin as fx;
    use crate::spawn_info::GlobalPos;
    use rewo_world::entities::EntityState;

    /// The same deliberately non name-sorted registry the login tests use:
    /// Nether 0, the_end 1, Overworld 2. Any name-keyed shortcut fails here.
    fn registry() -> Vec<DimensionTypeDef> {
        crate::dimension_parse::parse_dimension_registry_packet(&fx::registry_packet(&[
            ("minecraft:the_nether", fx::the_nether()),
            ("minecraft:the_end", fx::the_end()),
            ("minecraft:overworld", fx::overworld()),
        ]))
        .expect("fixture registry must parse")
        .expect("packet is the dimension_type registry")
    }

    const NETHER_HOLDER: i32 = 0;
    const OVERWORLD_HOLDER: i32 = 2;

    fn spawn(holder: i32, level: &str, seed: i64) -> CommonPlayerSpawnInfo {
        CommonPlayerSpawnInfo {
            dimension_type: holder,
            dimension: level.into(),
            seed,
            game_type: 1,
            previous_game_type: Some(0),
            is_debug: false,
            is_flat: false,
            last_death_location: Some(GlobalPos {
                dimension: "minecraft:overworld".into(),
                x: -3,
                y: -59,
                z: 7,
            }),
            portal_cooldown: 0,
            sea_level: 63,
        }
    }

    /// The world-side session state as plain locals: a `PlaySession` owns a
    /// socket and cannot be built in a test, but [`WorldTransition`] borrows
    /// exactly these fields and nothing else.
    struct Harness {
        world: World,
        dirty: std::collections::HashSet<(i32, i32)>,
        removed: Vec<(i32, i32)>,
        light: rewo_world::light::LightEngine,
        day_ticks: Option<i64>,
        overworld_clock: Option<WorldClock>,
        game_time: Option<i64>,
        weather: rewo_world::weather::WeatherState,
        biome_zoom_seed: Option<i64>,
        sea_level: Option<i32>,
        colormaps: rewo_world::biome::Colormaps,
        key: Option<String>,
        holder: Option<i32>,
        ty: Option<DimensionTypeDef>,
        generation: u64,
        transitions: Vec<DimensionTransition>,
    }

    /// A live-looking Overworld session: **three** loaded columns (two of them
    /// also queued for re-mesh), an entity, a running world clock, and a
    /// generation already past 0 so an increment can't be confused with a reset.
    const OLD_COLUMNS: [(i32, i32); 3] = [(0, 0), (1, -2), (-3, 5)];

    fn overworld_session(defs: &[DimensionTypeDef], generation: u64) -> Harness {
        let mut world = World::for_dimension(&defs[OVERWORLD_HOLDER as usize]);
        for (cx, cz) in OLD_COLUMNS {
            world.ensure_column(cx, cz);
        }
        world
            .entities
            .add(7, EntityState::new(1, 42, 8.0, 70.0, 8.0, 0.0, 0.0));
        assert_eq!(world.loaded_columns(), 3, "precondition");
        assert!(world.has_sky_light(), "precondition");
        Harness {
            world,
            dirty: [(0, 0), (1, -2)].into_iter().collect(),
            removed: Vec::new(),
            light: rewo_world::light::LightEngine::new(),
            day_ticks: Some(189_121),
            overworld_clock: Some(WorldClock::from_state(138_341, 189_121, 0.25, 1.0)),
            game_time: Some(138_341),
            // Deliberately a storm: a transition that failed to clear it would
            // otherwise be invisible against a default-clear harness.
            weather: {
                let mut w = rewo_world::weather::WeatherState::default();
                w.set_rain(0.8);
                w.set_thunder(0.5);
                w
            },
            biome_zoom_seed: Some(0x0bad_f00d),
            sea_level: Some(63),
            colormaps: rewo_world::biome::Colormaps::neutral(),
            key: Some("minecraft:overworld".into()),
            holder: Some(OVERWORLD_HOLDER),
            ty: Some(defs[OVERWORLD_HOLDER as usize].clone()),
            generation,
            transitions: Vec::new(),
        }
    }

    impl Harness {
        fn respawn(&mut self, defs: &[DimensionTypeDef], spawn: &CommonPlayerSpawnInfo) -> bool {
            WorldTransition {
                world: &mut self.world,
                dirty: &mut self.dirty,
                removed: &mut self.removed,
                light: &mut self.light,
                day_ticks: &mut self.day_ticks,
                overworld_clock: &mut self.overworld_clock,
                game_time: &mut self.game_time,
                weather: &mut self.weather,
                biome_zoom_seed: &mut self.biome_zoom_seed,
                sea_level: &mut self.sea_level,
                biome_registry: None,
                colormaps: &self.colormaps,
                active_key: &mut self.key,
                active_holder: &mut self.holder,
                active_type: &mut self.ty,
                generation: &mut self.generation,
                transitions: &mut self.transitions,
            }
            .apply_respawn(defs, spawn)
        }
    }

    /// Overworld → Nether: every old column is queued for the renderer to free,
    /// the world is a *fresh* Nether (0..256, no sky light, no columns, no
    /// entities), and the per-level clock/dirty/light state is back to its
    /// pre-`set_time` values.
    #[test]
    fn a_changed_key_rebuilds_the_world_and_queues_every_old_column() {
        let defs = registry();
        let mut s = overworld_session(&defs, 4);
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", -1)));

        // Every old coordinate reached `removed` — exactly once, and only those.
        let mut removed = s.removed.clone();
        removed.sort_unstable();
        let mut expected = OLD_COLUMNS;
        expected.sort_unstable();
        assert_eq!(removed, expected, "the renderer must free all three");

        // A fresh Nether, not a re-pointed Overworld.
        assert_eq!(s.world.shape, DimensionShape::NETHER);
        assert_ne!(s.world.shape, DimensionShape::OVERWORLD);
        assert!(!s.world.has_sky_light());
        assert_eq!(s.world.loaded_columns(), 0, "old columns are gone");
        assert_eq!(s.world.entities.len(), 0, "entities go with the old world");

        // Light-relevant state: the lighting contract is the Nether's, so an
        // unloaded read is dark rather than the Overworld's impossible sky 15 —
        // and the fresh engine has nothing queued from the old shape.
        assert_eq!(s.world.light_at(0, 70, 0), (0, 0));
        assert_eq!(s.world.brightness_at(0, 70, 0), 0);
        let tables = rewo_world::light::LightTables {
            emission: &[],
            dampening: &[],
            face_occludes: &[],
        };
        assert!(
            s.light
                .on_block_change(&mut s.world, tables, 0, 70, 0, 0, 0)
                .is_empty(),
            "a fresh engine touches nothing in an empty world"
        );

        // Per-level state cleared.
        assert!(s.dirty.is_empty(), "no stale column is queued for re-mesh");
        assert_eq!(s.day_ticks, None);
        assert_eq!(s.overworld_clock, None);
        assert_eq!(s.game_time, None);

        // The new dimension, and the seed the biome layer fiddles with.
        assert_eq!(s.key.as_deref(), Some("minecraft:the_nether"));
        assert_eq!(s.holder, Some(NETHER_HOLDER));
        assert_eq!(
            s.ty.as_ref().map(|d| d.name.as_str()),
            Some("minecraft:the_nether")
        );
        assert_eq!(s.biome_zoom_seed, Some(-1));

        // Generation incremented by exactly one, and the history is exact.
        assert_eq!(s.generation, 5);
        assert_eq!(
            s.transitions,
            vec![DimensionTransition {
                old_key: Some("minecraft:overworld".into()),
                new_key: "minecraft:the_nether".into(),
                holder: NETHER_HOLDER,
                type_name: "minecraft:the_nether".into(),
                shape: DimensionShape::NETHER,
                has_sky_light: false,
                skybox: Skybox::None,
                ambient_light: 0.1,
                cardinal_light_type: CardinalLightType::Nether,
                has_day_timeline: false,
                generation: 5,
                // The witnesses: three loaded columns left, three queued for the
                // renderer to free, none carried into the replacement, no stale
                // re-mesh entry, and the whole clock back to pre-`set_time`.
                old_columns: 3,
                queued_for_removal: 3,
                removal_queue_len: 3,
                new_world_columns: 0,
                dirty_after: 0,
                clock_reset: true,
            }]
        );
    }

    /// The discard witnesses are *measurements*, not constants: with a
    /// pre-loaded removal queue the transition still reports exactly what it
    /// pushed, and the queue length grows to hold both.
    ///
    /// This is the property no observer outside the transition can check —
    /// coordinates cannot prove it, because the Nether loads column (0,0) too.
    #[test]
    fn the_discard_witnesses_count_this_transitions_own_push() {
        let defs = registry();
        let mut s = overworld_session(&defs, 0);
        // A column the app has not drained yet, from an earlier unload.
        s.removed.push((99, 99));
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 7)));
        let t = &s.transitions[0];
        assert_eq!(t.old_columns, 3, "the world we left had three columns");
        assert_eq!(t.queued_for_removal, 3, "this transition pushed three");
        assert_eq!(t.removal_queue_len, 4, "the pre-existing entry is still queued");
        assert_eq!(t.new_world_columns, 0);
        assert_eq!(t.dirty_after, 0);
        assert!(t.clock_reset);
        assert_eq!(t.type_name, "minecraft:the_nether");
        assert_eq!(t.cardinal_light_type, CardinalLightType::Nether);
        assert_eq!(t.ambient_light, 0.1);
        assert!(!t.has_day_timeline);
        assert_eq!(s.removed.len(), 4);
    }

    /// A respawn naming the level we are already in (the ordinary death
    /// respawn): the world, its columns, the clock, the generation and the
    /// history all survive untouched — and so do the dimension type and the
    /// seed, because vanilla builds no new `ClientLevel` to apply them to.
    #[test]
    fn a_same_key_respawn_retains_the_world_generation_and_history() {
        let defs = registry();
        let mut s = overworld_session(&defs, 4);
        let digest = s.world.digest();
        // A same-key packet that nonetheless names a *different* holder and
        // seed: neither may be applied behind the retained chunks.
        assert!(!s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:overworld", 999)));

        assert_eq!(s.world.loaded_columns(), 3, "columns retained");
        assert_eq!(s.world.digest(), digest, "the world is untouched");
        assert_eq!(s.world.shape, DimensionShape::OVERWORLD);
        assert!(s.world.has_sky_light());
        assert_eq!(s.world.entities.len(), 1, "entities retained");
        assert!(s.removed.is_empty(), "nothing to free");
        assert_eq!(s.dirty.len(), 2, "re-mesh queue retained");

        assert_eq!(s.day_ticks, Some(189_121));
        assert_eq!(s.game_time, Some(138_341));
        assert_eq!(s.overworld_clock.unwrap().total, 189_121);

        assert_eq!(s.holder, Some(OVERWORLD_HOLDER), "type not re-applied");
        assert_eq!(
            s.ty.as_ref().map(|d| d.name.as_str()),
            Some("minecraft:overworld")
        );
        assert_eq!(s.biome_zoom_seed, Some(0x0bad_f00d), "seed retained");
        assert_eq!(s.generation, 4, "not a transition");
        assert!(s.transitions.is_empty(), "history unmoved");
        assert_eq!(
            s.weather.rain_level(),
            0.8,
            "no new level, so the storm keeps falling"
        );
    }

    /// Weather is `ClientLevel` state: a real dimension change discards it, so
    /// walking into the Nether cannot carry the Overworld's storm along. The
    /// harness starts at rain 0.8 / thunder 0.5 so this can fail.
    #[test]
    fn a_dimension_change_clears_the_weather() {
        let defs = registry();
        let mut s = overworld_session(&defs, 4);
        assert_eq!(s.weather.rain_level(), 0.8, "precondition");
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 7)));
        assert_eq!(s.weather.rain_level(), 0.0);
        assert_eq!(s.weather.thunder_level(), 0.0);
    }

    /// The generation is a counter, not an index: at `u64::MAX` it wraps to 0
    /// rather than panicking on overflow, and the recorded transition carries
    /// the wrapped value.
    #[test]
    fn the_generation_wraps_at_u64_max() {
        let defs = registry();
        let mut s = overworld_session(&defs, u64::MAX);
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 0)));
        assert_eq!(s.generation, 0);
        assert_eq!(s.transitions[0].generation, 0);
    }

    /// Two changes in a row: the history is append-only and oldest-first, and
    /// the second transition's `old_key` is the first's `new_key`.
    #[test]
    fn successive_changes_append_an_oldest_first_history() {
        let defs = registry();
        let mut s = overworld_session(&defs, 0);
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 1)));
        assert!(s.respawn(&defs, &spawn(OVERWORLD_HOLDER, "rewo:mining_world", 2)));

        assert_eq!(s.generation, 2);
        assert_eq!(s.transitions.len(), 2);
        assert_eq!(s.transitions[0].new_key, "minecraft:the_nether");
        assert_eq!(
            s.transitions[1].old_key.as_deref(),
            Some("minecraft:the_nether")
        );
        assert_eq!(s.transitions[1].new_key, "rewo:mining_world");
        // The level key is the packet's, the type is the holder's — a datapack
        // level on the vanilla overworld type keeps both straight.
        assert_eq!(s.key.as_deref(), Some("rewo:mining_world"));
        assert_eq!(
            s.ty.as_ref().map(|d| d.name.as_str()),
            Some("minecraft:overworld")
        );
        assert_eq!(s.world.shape, DimensionShape::OVERWORLD);
        assert!(s.world.has_sky_light());
    }

    // -- the local player --------------------------------------------------

    /// A player mid-flight, so nothing below can pass by starting at a default.
    fn moving_player() -> PlayerState {
        PlayerState {
            vx: 0.25,
            vy: -0.6,
            vz: -0.125,
            yaw: 137.5,
            pitch: -22.5,
            on_ground: true,
            horizontal_collision: true,
            ..PlayerState::at(120.5, 71.0, -33.25)
        }
    }

    struct PlayerHarness {
        player: PlayerState,
        health: f32,
        food: i32,
        dead: bool,
        spawned: bool,
        last_pos: (f64, f64, f64),
        last_rot: (f32, f32),
        reminder: u32,
        last_on_ground: bool,
        last_horiz: bool,
        last_input_flags: u8,
    }

    fn player_harness() -> PlayerHarness {
        PlayerHarness {
            player: moving_player(),
            health: 3.5,
            food: 6,
            dead: true,
            spawned: true,
            last_pos: (120.5, 71.0, -33.25),
            last_rot: (137.5, -22.5),
            reminder: 13,
            last_on_ground: true,
            last_horiz: true,
            last_input_flags: 0b0101_0001,
        }
    }

    impl PlayerHarness {
        fn respawn(&mut self, keep_entity_data: bool) {
            LocalPlayerRespawn {
                player: &mut self.player,
                health: &mut self.health,
                food: &mut self.food,
                dead: &mut self.dead,
                spawned: &mut self.spawned,
                last_pos: &mut self.last_pos,
                last_rot: &mut self.last_rot,
                reminder: &mut self.reminder,
                last_on_ground: &mut self.last_on_ground,
                last_horiz: &mut self.last_horiz,
                last_input_flags: &mut self.last_input_flags,
            }
            .apply(keep_entity_data);
        }
    }

    /// `dataToKeep` bit 2 (`shouldKeep((byte)2)`): the statements Rewo can
    /// represent — `setDeltaMovement(old)`, `setYRot(old)`, `setXRot(old)`, and
    /// the `assignValues` health entry — carry over bit-exactly, and the
    /// constructor's `lastSentInput` is the old player's rather than
    /// `Input.EMPTY`.
    #[test]
    fn keeping_entity_data_preserves_velocity_and_both_rotations() {
        let mut h = player_harness();
        h.respawn(true);

        let old = moving_player();
        assert_eq!(
            (h.player.vx, h.player.vy, h.player.vz),
            (old.vx, old.vy, old.vz)
        );
        assert_eq!(h.player.yaw, old.yaw);
        assert_eq!(h.player.pitch, old.pitch);
        assert_eq!(h.last_input_flags, 0b0101_0001, "old lastSentInput kept");

        // Everything else is still the fresh entity's — bit 2 keeps entity
        // *data*, not the entity's position or its send-cadence bookkeeping.
        assert_eq!((h.player.x, h.player.y, h.player.z), (0.0, 0.0, 0.0));
        assert!(!h.player.on_ground && !h.player.horizontal_collision);
        assert_eq!(h.last_pos, (0.0, 0.0, 0.0));
        assert_eq!(h.last_rot, (0.0, 0.0));
        assert_eq!(h.reminder, 0);
        assert!(!h.last_on_ground && !h.last_horiz);
        // Health is `SynchedEntityData`, so bit 2 preserves the old player's
        // non-default value exactly; food is `FoodData` and is always fresh.
        assert_eq!(h.health, 3.5, "DATA_HEALTH_ID carried by assignValues");
        assert_eq!(h.food, 20, "FoodData is not synched data — always fresh");
        assert!(!h.dead);
        assert!(
            !h.spawned,
            "not a live participant until the teleport lands"
        );
    }

    /// `dataToKeep` 0: `resetPos()` zeroes the delta movement and the X rotation,
    /// `handleRespawn` then sets the Y rotation to -180, and the constructor
    /// supplies everything else — including `Input.EMPTY`.
    #[test]
    fn without_keep_the_player_is_the_freshly_constructed_one() {
        let mut h = player_harness();
        h.respawn(false);

        assert_eq!((h.player.vx, h.player.vy, h.player.vz), (0.0, 0.0, 0.0));
        assert_eq!(h.player.pitch, 0.0, "resetPos setXRot(0)");
        assert_eq!(h.player.yaw, -180.0, "handleRespawn setYRot(-180)");
        assert_eq!(h.last_input_flags, 0, "Input.EMPTY");
        assert_eq!((h.player.x, h.player.y, h.player.z), (0.0, 0.0, 0.0));
        assert!(!h.player.on_ground && !h.player.horizontal_collision);
        assert_eq!(h.last_pos, (0.0, 0.0, 0.0));
        assert_eq!(h.last_rot, (0.0, 0.0));
        assert_eq!(h.reminder, 0);
        assert!(!h.last_on_ground && !h.last_horiz);
        // No bit 2 → no `assignValues`, so the harness's non-default 3.5 health
        // is gone and the fresh player's 20 stands.
        assert_eq!((h.health, h.food), (20.0, 20));
        assert!(!h.dead);
        assert!(!h.spawned);
    }

    /// Bit 1 (`KEEP_ATTRIBUTE_MODIFIERS`) selects `assignAllValues` vs
    /// `assignBaseValues` on an `AttributeMap` Rewo does not model, so it is a
    /// documented no-op: the two masks that differ only in bit 1 must land on
    /// identical player state.
    #[test]
    fn the_attribute_modifier_bit_is_a_no_op() {
        for (a, b) in [
            (0u8, RespawnInfo::KEEP_ATTRIBUTE_MODIFIERS),
            (RespawnInfo::KEEP_ENTITY_DATA, RespawnInfo::KEEP_ALL_DATA),
        ] {
            let keeps_entity_data = |m: u8| m & RespawnInfo::KEEP_ENTITY_DATA != 0;
            let mut x = player_harness();
            x.respawn(keeps_entity_data(a));
            let mut y = player_harness();
            y.respawn(keeps_entity_data(b));
            assert_eq!(
                (
                    x.player.vx,
                    x.player.vy,
                    x.player.vz,
                    x.player.yaw,
                    x.player.pitch
                ),
                (
                    y.player.vx,
                    y.player.vy,
                    y.player.vz,
                    y.player.yaw,
                    y.player.pitch
                ),
                "dataToKeep {a} and {b} differ only in the attribute bit"
            );
            assert_eq!(x.last_input_flags, y.last_input_flags);
            assert_eq!(
                (x.health, x.food, x.dead, x.spawned),
                (y.health, y.food, y.dead, y.spawned)
            );
        }
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

/// `ContainerInput.PICKUP`'s wire id. The enum's codec is
/// `ByteBufCodecs.idMapper`, so the declared int is what goes on the wire.
const CONTAINER_INPUT_PICKUP: i32 = 0;

/// Write one `HashedStack` (M35).
///
/// `HashedStack.STREAM_CODEC` is `ByteBufCodecs.optional(ActualItem)`, and
/// `ActualItem` is `holderRegistry(ITEM) + VarInt count + HashedPatchMap`.
/// Two details are easy to get wrong and neither fails loudly:
///
/// - `holderRegistry` writes the registry id **raw and 0-based**. It is not
///   `ByteBufCodecs.holder`, whose inline-or-reference scheme writes `id + 1`.
///   The same distinction bit the M14 dimension holder and the M21 damage type.
/// - `HashedPatchMap` is *two* collections — a map of set components to their
///   hashes, then a set of removed ones — so an empty patch is two zeros, not
///   one.
pub(crate) fn write_hashed_stack(p: &mut PacketWriter, slot: Option<rewo_world::inventory::ItemSlot>) {
    match slot {
        None => {
            p.bool(false);
        }
        Some(s) => {
            p.bool(true);
            p.varint(s.item_id);
            p.varint(s.count);
            p.varint(0); // addedComponents
            p.varint(0); // removedComponents
        }
    }
}
