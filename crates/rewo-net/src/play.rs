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

/// `GameType`, from `player_info_update`'s `UPDATE_GAME_MODE` (M62).
///
/// `minecraft:menu`'s `crafter_3x3` protocol id — the one menu with slot
/// toggles, and the only reason `crafter_slot_click` needs to know a menu type
/// at all.
pub const CRAFTER_MENU_PROTOCOL_ID: i32 = 7;

/// The tab list's second sort key is `getGameMode() == SPECTATOR`, and a
/// spectator's row is also grey and italic, so this has to survive decode as
/// something better than a raw int.
///
/// **An out-of-range id is `Survival`, not an error.** `GameType.byId` is
/// `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`, which answers anything
/// outside 0..=3 with `values[0]`. Rejecting it instead would desync nothing
/// (the field is a fixed var-int) but would report a state vanilla never
/// shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    pub fn by_id(id: i32) -> GameMode {
        match id {
            1 => GameMode::Creative,
            2 => GameMode::Adventure,
            3 => GameMode::Spectator,
            _ => GameMode::Survival,
        }
    }

    pub fn id(self) -> i32 {
        match self {
            GameMode::Survival => 0,
            GameMode::Creative => 1,
            GameMode::Adventure => 2,
            GameMode::Spectator => 3,
        }
    }

    pub fn is_spectator(self) -> bool {
        self == GameMode::Spectator
    }

    /// `GameType.isSurvival()` — **`SURVIVAL || ADVENTURE`**, not just the
    /// first. It is what `MultiPlayerGameMode.canHurtPlayer()` returns, which
    /// decides whether the HUD draws hearts and therefore whether the
    /// held-item name sits fourteen rows lower (M66's held-item info).
    pub fn is_survival(self) -> bool {
        self == GameMode::Survival || self == GameMode::Adventure
    }

    /// `GameType.isCreative()`.
    pub fn is_creative(self) -> bool {
        self == GameMode::Creative
    }

    /// `GameType.isBlockPlacingRestricted()` — adventure **and** spectator.
    pub fn is_block_placing_restricted(self) -> bool {
        self == GameMode::Adventure || self == GameMode::Spectator
    }

    /// `GameType.updatePlayerAbilities(Abilities)` (M75), as data.
    ///
    /// **The asymmetry is the whole point, and it runs one way only.** Creative
    /// grants `mayfly`/`instabuild`/`invulnerable` and says *nothing* about
    /// `flying`; spectator grants `mayfly`/`invulnerable` and additionally sets
    /// `flying = true`; every other mode clears all four. So:
    ///
    /// - **entering creative does not start you flying** — deriving `flying`
    ///   from `mayfly` is right for three of the four modes and wrong for
    ///   exactly the one a tester is most likely to be in. It would look like it
    ///   worked: you switch to creative, press the fly key, and never notice the
    ///   initial state was wrong.
    /// - **leaving creative actively drops flight** rather than merely ceasing
    ///   to permit it, because the `else` arm assigns `flying = false`. That
    ///   assignment is what the live gate leans on: a survival walk is
    ///   speed-checked by the server, so leaked flight state shows up as
    ///   corrections.
    ///
    /// `may_build` is assigned on every arm — it is outside the if/else.
    pub fn update_player_abilities(self) -> rewo_world::abilities::ModeAbilities {
        use rewo_world::abilities::ModeAbilities;
        let mut m = match self {
            GameMode::Creative => ModeAbilities {
                mayfly: true,
                instabuild: true,
                invulnerable: true,
                flying: None,
                may_build: true,
            },
            GameMode::Spectator => ModeAbilities {
                mayfly: true,
                instabuild: false,
                invulnerable: true,
                flying: Some(true),
                may_build: true,
            },
            GameMode::Survival | GameMode::Adventure => ModeAbilities {
                mayfly: false,
                instabuild: false,
                invulnerable: false,
                flying: Some(false),
                may_build: true,
            },
        };
        m.may_build = !self.is_block_placing_restricted();
        m
    }
}

/// The gamemode half of `handleLogin` / `handleRespawn` (M75): both end in the
/// **two-argument** `setLocalMode(gameType, previousGameType)`, which assigns
/// both fields directly and then re-derives the ability flags.
///
/// `previousGameType` is a real field with its own meaning, not a spare copy:
/// vanilla passes it straight through so `MultiPlayerGameMode` can answer "what
/// was I before" across a respawn, where the one-argument form's change-guard
/// would have derived it from the client's own history instead. `-1` on the
/// wire means absent, which [`crate::spawn_info::CommonPlayerSpawnInfo`] already
/// resolves to `None`.
///
/// A **free function** rather than a `PlaySession` method so the `abilityshot`
/// gate can drive the same code the session runs. A gate that reimplemented
/// this two-line mapping would be grading its own copy — the failure M45
/// recorded when `itemshot` called `init_entities` directly and so never
/// installed the glint.
pub fn apply_spawn_game_mode(
    state: &mut crate::game_event::ClientGameState,
    abilities: &mut rewo_world::abilities::Abilities,
    spawn: &CommonPlayerSpawnInfo,
) {
    state.set_local_mode_with_previous(
        GameMode::by_id(spawn.game_type as i32),
        spawn.previous_game_type.map(|p| GameMode::by_id(p as i32)),
        abilities,
    );
}

/// `UUID.toString()` — the dashed lowercase 8-4-4-4-12 form, which is what
/// `Entity.stringUUID` holds and therefore what a non-player entity's
/// scoreboard membership is keyed by (M70).
pub fn uuid_to_dashed(uuid: u128) -> String {
    let h = format!("{uuid:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// One entry of a `player_info_update` body.
///
/// Every field but the uuid is `Option`, and that is the whole point: the
/// packet is a *delta*. An action bit that is not set means "unchanged", which
/// is a different thing from a value, so a decoder that filled in a default
/// would tell the tab list a spectator went back to survival every time the
/// server sent a latency-only update.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerInfoEntry {
    pub uuid: u128,
    /// `ADD_PLAYER`'s profile name.
    pub name: Option<String>,
    /// The raw value of the profile's `textures` property, if it carried one.
    /// Left undecoded here so the walk stays wire-only.
    pub textures: Option<String>,
    /// `UPDATE_GAME_MODE` (action 2).
    pub gamemode: Option<GameMode>,
    /// `UPDATE_LISTED` (action 3).
    pub listed: Option<bool>,
    /// `UPDATE_LATENCY` (action 4), in milliseconds. May be negative — see
    /// `PlaySession::latency`.
    pub latency: Option<i32>,
    /// `UPDATE_LIST_ORDER` (action 6).
    pub tab_list_order: Option<i32>,
    /// `UPDATE_HAT` (action 7).
    pub show_hat: Option<bool>,
}

/// Walk one entry's set fields, in **action-bit order**.
///
/// Takes the entry by `&mut` rather than returning it so that a body which
/// runs out mid-entry still contributes the fields it did read — which is what
/// the pre-M62 decoder did, since it pushed each field as it went.
fn read_player_info_entry(
    r: &mut PacketReader,
    mask: u8,
    e: &mut PlayerInfoEntry,
) -> rewo_proto::Result<()> {
    let has = |bit: u8| mask & (1u8 << bit) != 0;
    if has(0) {
        // ADD_PLAYER: `ByteBufCodecs.GAME_PROFILE` minus the uuid — a
        // 16-char name, then the property map. The three string caps
        // (64 / 32767 / 1024) are `GAME_PROFILE_PROPERTIES`'s own.
        e.name = Some(r.string(16)?);
        let props = r.count("profile properties", 1)?;
        for _ in 0..props {
            let prop = r.string(64)?;
            let value = r.string(32767)?;
            if prop == "textures" {
                e.textures = Some(value);
            }
            if r.bool()? {
                let _sig = r.string(1024)?;
            }
        }
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
        e.gamemode = Some(GameMode::by_id(r.varint()?));
    }
    if has(3) {
        e.listed = Some(r.bool()?);
    }
    if has(4) {
        e.latency = Some(r.varint()?);
    }
    if has(5) {
        // UPDATE_DISPLAY_NAME: nullable NBT text component.
        if r.bool()? {
            let _ = r.nbt()?;
        }
    }
    if has(6) {
        e.tab_list_order = Some(r.varint()?);
    }
    if has(7) {
        e.show_hat = Some(r.bool()?);
    }
    Ok(())
}

/// Decode a `player_info_update` body: a 1-byte fixed bitset over the 8
/// actions (LSB-first, ordinal order), then a var-int list of entries.
///
/// **This is the production walk**, called by `apply_player_info` and by the
/// tests both. Keeping one copy matters more than it looks: the pre-M62 tree
/// had two, and they had already drifted — one capped a profile signature at
/// 1024 (vanilla's `GAME_PROFILE_PROPERTIES` figure) and the other at 32767.
///
/// The walk is the fragile part. Fields are read in action-bit order and only
/// when their bit is set, so a mis-sized skip does not fail: it shifts every
/// subsequent entry and reports plausible numbers for the wrong players.
///
/// Returns the entries read so far alongside the error that stopped the walk,
/// because a truncated body still tells the truth about the entries it
/// completed.
pub fn parse_player_info(body: &[u8]) -> (Vec<PlayerInfoEntry>, rewo_proto::Result<()>) {
    let mut r = PacketReader::new(body);
    let mut out: Vec<PlayerInfoEntry> = Vec::new();
    let res = (|| -> rewo_proto::Result<()> {
        let mask = r.u8()?;
        let count = r.count("player info entries", 16)?;
        for _ in 0..count {
            let mut e = PlayerInfoEntry {
                uuid: r.uuid()?,
                ..Default::default()
            };
            let res = read_player_info_entry(&mut r, mask, &mut e);
            out.push(e);
            res?;
        }
        Ok(())
    })();
    (out, res)
}

/// The latency entries carried by a `player_info_update` body (M52c).
///
/// A thin view over `parse_player_info`; kept because it names the one field
/// `ping_ms` is built on.
pub fn parse_player_info_latency(body: &[u8]) -> Vec<(u128, i32)> {
    parse_player_info(body)
        .0
        .into_iter()
        .filter_map(|e| e.latency.map(|ms| (e.uuid, ms)))
        .collect()
}

/// What M68's four packets actually did to this session.
///
/// Counters rather than booleans because the gate has to distinguish "the
/// server never sent one" from "it sent one that carried nothing" — an
/// `explode` with no `playerKnockback` is the *common* case (only a
/// non-spectator, non-flying player is recorded in `ServerExplosion.hitPlayers`),
/// so a gate that only counted explosions would pass on a run where the bot
/// was never actually pushed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionStats {
    /// `explode` packets decoded.
    pub explosions: u32,
    /// …of which carried a `playerKnockback` for us.
    pub explosion_knockbacks: u32,
    /// …of which carried a knockback that was actually **non-zero**.
    ///
    /// The distinction is load-bearing for diagnosis, not decoration.
    /// `ServerExplosion` records a player in `hitPlayers` whenever it is in
    /// range and not a flying-creative spectator — *regardless of whether the
    /// computed knockback came out zero* — and it comes out zero whenever
    /// `getSeenPercent` finds no line of sight. So a shielded player receives
    /// `Some((0,0,0))`, which is indistinguishable downstream from a knockback
    /// the client decoded and threw away. Without this counter the gate blames
    /// the decoder for what is really a fixture that blew a hole in its own
    /// line of sight.
    pub explosion_knockbacks_nonzero: u32,
    /// `set_entity_motion` packets decoded, for any entity.
    pub entity_motions: u32,
    /// …of which addressed the local player's own entity id.
    pub local_motions: u32,
    /// …of which were the one-byte zero sentinel (an entity stopping).
    pub local_motion_stops: u32,
    /// `set_passengers` packets decoded.
    pub passenger_updates: u32,
    /// Transitions where the local player *became* a passenger.
    pub local_mounts: u32,
    /// Transitions where the local player *stopped* being a passenger.
    pub local_dismounts: u32,
    /// `move_vehicle` packets decoded. Expected to stay 0 — see
    /// [`crate::motion::VehicleMove`].
    pub vehicle_moves: u32,
    /// Server position corrections received while the local player was a
    /// passenger.
    ///
    /// Tracked apart from [`PlaySession::corrections`] because vanilla's
    /// server does **not** validate a passenger's movement
    /// (`ServerGamePacketListenerImpl`: `if (this.player.isPassenger())` snaps
    /// and returns), so this number cannot rise from riding badly. It exists
    /// to keep those teleports out of the walking meter, not to grade riding.
    pub corrections_while_mounted: u32,
    /// The largest per-component change an explosion knockback actually made
    /// to the local player's velocity, **measured from the player state** —
    /// before and after — rather than read off the packet.
    ///
    /// This exists because the correction-based check alone proved too weak.
    /// A mutation that decoded the knockback and then dropped it passed a
    /// green gate, so the gate needed a witness that observes the *effect* on
    /// the client rather than a downstream consequence the server may or may
    /// not choose to report. A decoded-but-unapplied knockback leaves this at
    /// exactly 0.
    pub knockback_velocity_delta: f64,
}

pub struct PlaySession {
    writer: crate::NetStream,
    codec: FrameCodec,
    rx: Receiver<Vec<u8>>,
    pub ids: Ids,
    /// The villager trade list, when a merchant screen is open (M93u).
    pub merchant: Option<crate::merchant::MerchantOffers>,
    /// The unlocked recipes, by `RecipeDisplayId` (M93y). Decoded and held;
    /// nothing renders a recipe book yet.
    pub recipe_book: std::collections::BTreeMap<i32, crate::recipe_book::Entry>,
    /// The book's four per-type open/filter pairs.
    pub recipe_book_settings: crate::recipe_book::BookSettings,
    /// The last ghost recipe the server asked to be shown, and its container.
    pub ghost_recipe: Option<(i32, crate::recipe_book::RecipeDisplay)>,
    /// Server-reported latency per player, in milliseconds (M52c).
    ///
    /// **This is the only ping a client can know**, and the reason is worth
    /// recording because the obvious alternative does not work: `keep_alive`
    /// and `ping` are *server-initiated* probes — the server sends, the client
    /// echoes, and the SERVER times the round trip. A client cannot measure
    /// RTT from a packet it did not initiate, and the play protocol gives it
    /// nothing to initiate. So vanilla's own tab list shows exactly this
    /// number: `UPDATE_LATENCY` on the player-info packet, including for
    /// yourself.
    pub latency: std::collections::HashMap<u128, i32>,
    /// Server-reported game mode per player (`UPDATE_GAME_MODE`, M62).
    ///
    /// Only *reported* modes live here. Vanilla's `PlayerInfo` defaults the
    /// field to `SURVIVAL` at construction, but this map deliberately does
    /// not, because "the server has not said" and "the server said survival"
    /// are answers to different questions and only the map can tell them
    /// apart. A caller that wants vanilla's rendering behaviour reads
    /// `unwrap_or(GameMode::Survival)`.
    pub gamemodes: std::collections::HashMap<u128, GameMode>,
    /// Server-reported tab-list order (`UPDATE_LIST_ORDER`, M62) — the tab
    /// list's *first* sort key, negated so a higher value sorts first.
    /// Absent means the server never sent one, which vanilla renders as 0.
    pub tab_list_orders: std::collections::HashMap<u128, i32>,
    /// The client-side scoreboard: M62's teams (the tab list's third sort key,
    /// keyed by member **name** rather than uuid) plus M65's objectives,
    /// scores and display slots. One struct because vanilla's `Scoreboard` is
    /// one object and the halves touch — see [`crate::scoreboard`].
    pub scoreboard: crate::scoreboard::Scoreboard,
    /// Boss bars (`boss_event`, M65), in display order.
    pub boss_bars: crate::boss_bar::BossBars,
    /// The tab list's header and footer (`tab_list`, M65).
    pub tab_list_text: crate::tab_list_text::TabListText,
    /// The server's view area (M67) — the chunk the view is centred on, the
    /// server's render distance, and its simulation distance. Seeded by the
    /// login packet's prefix and updated by `set_chunk_cache_center` /
    /// `set_chunk_cache_radius` / `set_simulation_distance`.
    ///
    /// **Read by nothing yet.** Decoding it and acting on it are separate
    /// pieces of work: eviction policy, mesh budgeting and entity-tick gating
    /// each need a renderer and a tuning pass to grade, and none of them is
    /// improved by being written blind. See [`crate::view_area`].
    ///
    /// Deliberately *not* reset by `apply_respawn`: vanilla's `handleRespawn`
    /// rebuilds its `ClientLevel` from the existing `serverChunkRadius` /
    /// `serverSimulationDistance` fields, because the respawn packet carries
    /// neither. Clearing it on a dimension change — which is what
    /// `WorldTransition` does to nearly everything else — would silently drop
    /// back to the pre-login defaults for the rest of the session.
    pub view_area: crate::view_area::ViewArea,
    /// The chunk-batch flow-control loop (M74). Rewo used to answer
    /// `chunk_batch_finished` with the literal `64.0`; vanilla answers this
    /// calculator, whose opening bid is `3.5`. See [`crate::chunk_batch`] —
    /// this is a behaviour fix, not a new decode.
    pub chunk_batch: crate::chunk_batch::ChunkBatchSizeCalculator,
    /// The server's tick clock (M74) — `/tick rate`, `/tick freeze`,
    /// `/tick step`.
    ///
    /// **Read by nothing yet**, in M67's sense: the 20 Hz loop below does not
    /// consult it. Gating the loop would retime every existing harness (the
    /// correction meter measures movement against a 50 ms tick), so it wants
    /// its own live gate rather than a free ride on this one.
    pub ticking: crate::ticking::TickRateManager,
    /// Difficulty, the camera's target entity, and the container-close latch
    /// (M74). See [`crate::client_state`].
    ///
    /// Deliberately **not** reset by [`Self::apply_respawn`], for the same
    /// reason the view area is not: `handleRespawn` builds its replacement
    /// `ClientLevelData` from `this.levelData.getDifficulty()`, carrying the
    /// old value across a dimension change rather than re-defaulting it. The
    /// respawn packet carries no difficulty, so clearing here would silently
    /// report NORMAL for the rest of the session after one Nether portal.
    pub client_state: crate::client_state::ClientState,
    /// Session, server metadata and chat (M78): the server brand, the MOTD and
    /// icon, the game-rule map, and the cookie jar `cookie_request` answers
    /// from. See [`crate::session`].
    ///
    /// Deliberately **not** reset by [`Self::apply_respawn`], for the reason
    /// the view area and `client_state` are not: every field here belongs to
    /// vanilla's `ClientCommonPacketListenerImpl`, which outlives a dimension
    /// change entirely — a `WorldTransition` rebuilds the level, not the
    /// connection.
    pub session: crate::session::SessionState,
    /// The title overlay and the two HUD gauges (M79): the title / subtitle /
    /// action-bar clocks and durations, the experience triple, and the
    /// item-cooldown map. See [`crate::hud_state`].
    ///
    /// [`Self::apply_respawn`] resets **half** of this and keeps the other
    /// half, because vanilla splits it across two objects with opposite
    /// lifetimes: the titles live on `Minecraft.gui.hud`, which outlives a
    /// death and a dimension change, while the experience fields and the
    /// cooldown map live on the `LocalPlayer` a respawn replaces. See
    /// [`crate::hud_state::HudState::reset_for_respawn`].
    pub hud: crate::hud_state::HudState,
    /// The bundle reassembler (M78). Sits between the socket and
    /// [`Self::handle_packet`], so everything between two `bundle_delimiter`s
    /// is applied in one drain and no frame is rendered part-way through a
    /// spawn. See [`crate::bundle`].
    bundle: crate::bundle::BundleAssembler,
    /// Monotonic epoch for [`Self::now_nanos`], which stands in for vanilla's
    /// `Util.getNanos()`. Only the *interval* between two reads matters to
    /// [`crate::chunk_batch`], so an arbitrary epoch is fine — and a
    /// session-local one keeps the numbers small enough to read in a log.
    clock_epoch: std::time::Instant,
    /// The three `minecraft:number_format_type` ids a scoreboard objective or
    /// score dispatches on — resolved by name at load, carried here because
    /// the decode cannot proceed without them.
    number_formats: rewo_data::number_formats::NumberFormatTypeIds,
    /// The local player's UUID, so `own_ping_ms` knows which entry is ours.
    /// Absent in offline mode until the server names us.
    pub own_uuid: Option<u128>,
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
    /// `minecraft:sheep` — gates the index-18 wool byte (M52).
    pub sheep_type_id: Option<i32>,
    /// `minecraft:creaking` — gates the index-17 `IS_ACTIVE` boolean (M52).
    pub creaking_type_id: Option<i32>,
    /// `minecraft:bee` — gates the index-19 anger deadline, and names the kind
    /// `postAddEntitySoundInstance` starts a loop for (M141f).
    pub bee_type_id: Option<i32>,
    /// `minecraft:guardian` / `minecraft:elder_guardian` — they gate the
    /// index-17 attack target and name the kinds entity event 21 reaches
    /// (M141g). Two ids because the elder is its own registry entry.
    pub guardian_type_id: Option<i32>,
    pub elder_guardian_type_id: Option<i32>,
    /// `minecraft:sniffer` — names the kind entity event 63 reaches (M141g).
    pub sniffer_type_id: Option<i32>,
    /// `minecraft:player` — gates the index-16 skin-customisation byte whose
    /// bit 0 shows the cape (M60). `None` leaves every cape hidden, which is
    /// also what an unsent mask means, so a harness that never resolves it
    /// behaves like one whose players never sent one.
    pub player_type_id: Option<i32>,
    /// The six mobs whose texture a metadata field chooses (M64). Default-
    /// empty routes every variant nowhere, which leaves those mobs on the
    /// texture Rewo baked — the same "`None` means don't interpret" rule the
    /// type ids above use.
    pub variant_type_ids: crate::VariantKinds,
    /// `minecraft:item` and `minecraft:experience_orb` (M81) —
    /// `handleTakeItemEntity` branches on both, and the three outcomes (shrink
    /// then maybe remove / never remove / remove outright) are not
    /// interchangeable. `None` on either collapses that type into the
    /// "remove outright" arm, which is the safe reading for a harness that
    /// resolves neither.
    pub take_item_kinds: crate::TakeItemKinds,
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
    /// The three recipe-book display registries (M93y).
    ///
    /// Supplied by the app, as `swing_data` is, because they are **built-in**
    /// registries read from the datagen report — a server never sends them,
    /// and `rewo-net` holds no path to the report.
    pub recipe_display_ids: Option<rewo_data::recipe_display::RecipeDisplayIds>,
    /// The `minecraft:enchantment` registry from configuration (M42), indexed
    /// by protocol id. Empty on a server that syncs none.
    pub enchantments: Vec<crate::enchantment_parse::EnchantmentDef>,
    /// The `minecraft:chat_type` registry from configuration (M127), indexed
    /// by protocol id. Empty on a server that syncs none, which leaves every
    /// chat line undecorated rather than guessing a decoration.
    pub chat_types: Vec<crate::chat_type_parse::ChatTypeDef>,
    /// The two trim registries, index = protocol id (M48).
    pub trim_materials: Vec<crate::trim_parse::TrimMaterialDef>,
    pub trim_patterns: Vec<crate::trim_parse::TrimPatternDef>,
    /// The three metadata-variant registries (M64), index = protocol id.
    /// Empty on a server that syncs none, which leaves those mobs on their
    /// base textures rather than guessing an order.
    pub cat_variants: Vec<crate::variant_parse::MobVariantDef>,
    pub wolf_variants: Vec<crate::variant_parse::MobVariantDef>,
    pub frog_variants: Vec<crate::variant_parse::MobVariantDef>,
    /// The server's datapack tags (M69).
    ///
    /// Carried over from configuration, where a vanilla server sends them, and
    /// replaced per-registry by any `update_tags` that arrives in play (a
    /// datapack reload). **Read by nothing yet** — M19's `ItemTags.SPEARS` and
    /// M42's enchantment tags still come from the client jar, which is the
    /// divergence `REWO_PACKET_COVERAGE.md` §3 ranks first. `crate::tags` says
    /// what closing it takes and why it was not done in the same change that
    /// first decoded the packet.
    pub tags: crate::tags::TagOverrides,
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
    /// Who is riding what (M68), from `set_passengers`.
    pub mounts: crate::motion::Mounts,
    /// The **local player's** `update_attributes` snapshots (M73).
    ///
    /// Kept beside the entity table rather than in it: the server sends no
    /// `add_entity` for your own player, so `apply_update_attributes`'s
    /// `getEntity(id) == null` gate drops every snapshot addressed to it. The
    /// crosshair pick reads `block_interaction_range` and
    /// `entity_interaction_range` from here.
    local_attributes: rewo_world::attributes::EntityAttributes,
    /// The local player's own `SynchedEntityData` (M141e) — see
    /// [`crate::local_player_data`]. Beside the table for M73's reason: the
    /// table has no row for you.
    local_player_data: crate::local_player_data::LocalPlayerData,
    /// The pose of the vehicle the local player rides, from `move_vehicle`.
    ///
    /// `None` in every ordinary session: that packet is only ever the server
    /// *rejecting* a serverbound vehicle move, and Rewo never claims to drive
    /// a vehicle. Kept because the decode is real and a future controlling
    /// client would read exactly this.
    pub vehicle_pose: Option<crate::motion::VehicleMove>,
    /// M68's observation counters — what `rewo play --motion-check` grades.
    pub motion_stats: MotionStats,
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
    /// The world border (M80). Level state — vanilla's lives on `ClientLevel`
    /// and a dimension change builds a fresh one — so it is cleared by
    /// [`WorldTransition`] alongside the weather. The server re-sends
    /// `initialize_border` after a respawn, so the default is never seen for
    /// more than the packets in flight.
    pub border: rewo_world::border::WorldBorder,
    /// The player's own inventory (M34). Unlike the weather this is **not**
    /// level state — vanilla's `Inventory` lives on the player, who survives a
    /// dimension change — so it is deliberately not cleared by the transition.
    /// The server re-sends the contents on respawn anyway.
    pub inventory: rewo_world::inventory::Inventory,
    /// The open container menu, if any (M87).
    ///
    /// Vanilla's second menu: `player.containerMenu`, beside the permanent
    /// `player.inventoryMenu` that [`Self::inventory`] is. The server
    /// addresses them independently — `container_set_slot` with id 0 always
    /// writes the inventory whatever is open — so they are two fields rather
    /// than one.
    pub menus: rewo_world::menu::Menus,
    /// The tracked waypoints the locator bar draws (M83).
    ///
    /// **Connection state, not level state.** `ClientWaypointManager` is a
    /// field of `ClientPacketListener`, not of `ClientLevel`, so it is one of
    /// the few things a dimension change does *not* discard — which is why it
    /// is absent from [`WorldTransition`] beside the weather and the border.
    /// It stays correct across a change because `ServerWaypointManager` is
    /// per-level and its `removePlayer`/`addPlayer` pair sends an `UNTRACK`
    /// for every waypoint of the level being left and a `TRACK` for every one
    /// of the level being entered.
    pub waypoints: crate::waypoints::WaypointStore,
    /// The `minecraft:data_component_type` registry, kept so a *name* can be
    /// recovered from a patch's raw ids (M66).
    ///
    /// `component_count` needs it: `PatchedDataComponentMap.size()` asks
    /// whether the item's prototype carries each patched component, the
    /// prototype table is keyed by name, and the wire is keyed by id. `None`
    /// before the registry arrives, in which case the count is unanswerable
    /// and the tooltip line is dropped rather than guessed.
    pub component_names: Option<std::sync::Arc<rewo_data::components::DataComponentRegistry>>,
    /// The third slot carrier (M66) — a shulker box's contents and each
    /// patch's raw component ids, keyed by the same fingerprint the
    /// inventory's `SlotText` uses. Kept beside the inventory rather than in
    /// it, because neither of that crate's carriers can hold a `Vec` of
    /// stacks and the raw ids mean nothing without the runtime registry.
    pub stack_details: crate::item_stack::StackDetails,
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
    /// Sound requests decoded from `sound` / `sound_entity` / `stop_sound`
    /// (M63). **Nothing drains this yet** — Rewo has no audio device, and
    /// this milestone is the decode half only. One queue for all three so the
    /// server's ordering survives: a `stop_sound` that overtook the sound it
    /// cancels would leave that sound playing forever. It is capped rather
    /// than unbounded precisely because no consumer exists; see
    /// [`Self::MAX_PENDING_SOUNDS`].
    pub sound_events: Vec<crate::sounds::SoundEvent>,
    /// The ten non-weather `game_event` types (M71) — gamemode, the death-
    /// screen and limited-crafting flags, the win/demo/chunk-load markers.
    /// The four weather ids go to [`Self::weather`] instead; one packet feeds
    /// both, decoded once.
    pub game_state: crate::game_event::ClientGameState,
    /// The local player's `Abilities` (M75) — what the `player_abilities`
    /// packet writes and what `GameType.updatePlayerAbilities` re-derives on
    /// every gamemode announcement. Read by `physics::tick_with` for the flight
    /// path.
    pub abilities: rewo_world::abilities::Abilities,
    /// `LocalPlayer`'s client-side flight controller: the double-tap toggle and
    /// its `jumpTriggerTime` window.
    pub flight: rewo_world::abilities::FlightControl,
    /// Every chat line as plain text, cumulative — vanilla's
    /// `LOGGER.info("[CHAT] {}")`, which `logChatMessage` writes beside the
    /// `ChatComponent` rather than instead of it. Harnesses count it; the HUD
    /// does not read it.
    pub chat_log: Vec<String>,
    /// The structured feed the chat HUD consumes, drained by
    /// [`Self::take_chat_events`]. Separate from `chat_log` for the reason
    /// vanilla keeps both: one is an append-only log, the other is a queue
    /// whose consumer owns the font and the GUI clock the store needs and
    /// which `rewo-net` cannot see.
    chat_events: Vec<crate::chat_wire::ChatEvent>,
    /// `MessageSignatureCache` — 128 slots, fed by every `player_chat` on
    /// receipt. **`delete_chat` is unreadable without it**: the packet's
    /// signature is usually a cache index rather than 256 bytes.
    signature_cache: crate::chat_wire::MessageSignatureCache,
    /// The clock `ChatTrustLevel.evaluate` compares a message's timestamp
    /// against, in epoch milliseconds. Set by the caller each tick;
    /// `rewo-net` reads no wall clock of its own, so a harness driving raw
    /// packets gets a deterministic trust level.
    pub chat_clock_millis: i64,
    /// The Brigadier command tree (M113), empty until `commands` arrives.
    pub commands: crate::commands::CommandTree,
    /// `ClientSuggestionProvider`'s completion set and its single pending
    /// request (M114). The set is server-driven; the pending slot is what
    /// makes a stale reply inert. See [`crate::suggestion_wire`].
    pub suggestions: crate::suggestion_wire::SuggestionProviderState,
    /// The most recent accepted `command_suggestions` reply, for the UI to
    /// drain. `None` once taken — a reply that arrives for a superseded
    /// request never reaches here at all.
    pub suggestion_reply: Option<rewo_world::suggestions::Suggestions>,
    /// The `command_argument_type` registry, supplied by the caller from the
    /// datagen report — it is a **built-in** registry (M92's rule), and the
    /// tree cannot be read past its first non-singleton argument without it.
    pub command_argument_types: Option<rewo_data::command_argument_types::CommandArgumentTypes>,
    /// The language table, supplied by the caller from the client jar (M125),
    /// for resolving the `translate` components that arrive in chat.
    ///
    /// **Vanilla reaches a global here** — `TranslatableContents.decompose`
    /// calls `Language.getInstance()` — so resolving at receipt rather than at
    /// render is equivalent for a client that cannot change language
    /// mid-session, which Rewo cannot (one bundled `en_us`). It is a field
    /// rather than an argument for the same reason
    /// [`Self::command_argument_types`] is: the app owns the loaded table and
    /// hands it over once, and a packet arm has no other way to reach it.
    ///
    /// `None` leaves every key unresolved, which is exactly what this session
    /// did before M125 — see [`crate::chat_translate`].
    pub lang: Option<std::sync::Arc<rewo_data::lang::Language>>,
    /// The chat HUD's store (M108). Lives here because the events that feed
    /// it do; it is *driven* from the app, which owns the font it wraps with
    /// and the GUI clock it stamps with — see [`Self::apply_chat_events`].
    pub chat: rewo_world::chat::ChatComponent,
    /// `Gui.setOverlayMessage`'s text, from a `system_chat` with `overlay`
    /// set. `None` until one arrives.
    pub chat_overlay: Option<String>,
    pub health: f32,
    /// Food level 0..20 (Set Health packet), for the HUD hunger bar.
    pub food: i32,
    pub dead: bool,
    /// `ClientboundLoginPacket.hardcore` (M82) — the death screen's title and
    /// respawn-button labels branch on it. `handlePlayerCombatKill` reads it
    /// from `this.level.getLevelData().isHardcore()`, which is seeded from the
    /// login packet and never changes for the life of a connection.
    pub hardcore: bool,
    /// `Player.getScore()` — metadata index 18, INT, for the local player
    /// (M82). Vanilla's field initialiser is 0, and a server that never awards
    /// a kill score never sends it.
    pub score: i32,
    /// How many `respawn` packets have been applied (M82).
    ///
    /// A watermark, not a flag, for the reason `container_close`'s is one: the
    /// only consumer is "close the death screen if it is open", and vanilla's
    /// own rule is exactly that — `handleRespawn` ends with
    ///
    /// ```java
    /// if (this.minecraft.gui.screen() instanceof DeathScreen
    ///  || this.minecraft.gui.screen() instanceof DeathScreen.TitleConfirmScreen) {
    ///    this.minecraft.gui.setScreen(null);
    /// }
    /// ```
    ///
    /// **The button press does not close the screen.** `DeathScreen`'s
    /// `onPress` sends `PERFORM_RESPAWN` and sets `button.active = false`, and
    /// the screen stays up until the server's respawn arrives — which is why a
    /// laggy respawn leaves you looking at a dead "Respawn" button rather than
    /// at a black world.
    respawn_epoch: u64,
    /// The last `player_combat_kill` addressed to us, waiting to be drained
    /// (M82).
    ///
    /// Not a `bool`: the packet carries the death message, and the app needs
    /// the raw NBT so it can style it. Cleared by [`Self::take_death`], so a
    /// caller that never drains it holds one death, not a queue.
    death: Option<crate::CombatKill>,
    /// The local player's statistics (M84).
    ///
    /// `Minecraft.player.getStats()`, which is why it lives on the session and
    /// not on an entity: `handleAwardStats` writes into it with no id to
    /// address, so there is nothing to look up.
    pub stats: rewo_world::stats::StatsCounter,
    /// Scratch for the dispatch arm above — a decoded `award_stats` on its
    /// way into [`Self::stats`], and always `None` between packets.
    awarded_stats: Option<Vec<(rewo_world::stats::StatKey, i32)>>,
    pub disconnect: Option<String>,
    /// **Which** of vanilla's three producers ended the connection (M85).
    ///
    /// Set beside [`Self::disconnect`] at every site that sets it, because the
    /// reason string alone cannot say: `DisconnectionDetails` has two
    /// constructors and only `createDisconnectionInfo` fills `bugReportLink`,
    /// so a disconnect screen that inferred the cause from the text would
    /// offer the server's bug-report link on a plain kick. See
    /// [`rewo_world::disconnect_screen::DisconnectCause`].
    pub disconnect_cause: Option<rewo_world::disconnect_screen::DisconnectCause>,
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
    /// The border is `ClientLevel` state on the same argument (M80): vanilla
    /// builds a fresh `WorldBorder` with the new level, and the server sends
    /// `initialize_border` again right after the respawn. Carrying the
    /// Overworld's 60-million-block default into a 1,000-block Nether border
    /// would be a wall in the wrong place.
    border: &'a mut rewo_world::border::WorldBorder,
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
        *self.border = rewo_world::border::WorldBorder::default();

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
        // The chat-type registry, likewise in wire order (M127) — the index is
        // the id a `ChatType.Bound` names.
        let chat_types = std::mem::take(&mut self.chat_types);
        let trim_materials = std::mem::take(&mut self.trim_materials);
        let trim_patterns = std::mem::take(&mut self.trim_patterns);
        // The tags the server sent during configuration (M69). Moved rather
        // than cloned for the same reason the registries above are: this
        // connection object is finished with them.
        let tags = std::mem::take(&mut self.tags);
        // The brand and the cookie jar (M78). Both arrive during
        // *configuration* — the vanilla server sends `minecraft:brand` from its
        // configuration listener and never repeats it in play — and both are
        // fields of the common listener that outlives the state switch, so they
        // move across exactly as the tags above do.
        let session_state = std::mem::take(&mut self.session);
        // The bundle reassembler (M78). Built here rather than inside the
        // struct literal below because `ids: self.ids` moves the id table.
        let bundle = crate::bundle::BundleAssembler::new(crate::bundle::BundleIds {
            delimiter: self.ids.cb_play_bundle_delimiter,
            terminal: self.ids.cb_play_start_configuration,
        });
        let cat_variants = std::mem::take(&mut self.cat_variants);
        let wolf_variants = std::mem::take(&mut self.wolf_variants);
        let frog_variants = std::mem::take(&mut self.frog_variants);
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
            merchant: None,
            recipe_book: Default::default(),
            recipe_book_settings: Default::default(),
            ghost_recipe: None,
            writer,
            codec,
            rx,
            ids: self.ids,
            latency: std::collections::HashMap::new(),
            gamemodes: std::collections::HashMap::new(),
            tab_list_orders: std::collections::HashMap::new(),
            scoreboard: crate::scoreboard::Scoreboard::new(),
            boss_bars: crate::boss_bar::BossBars::new(),
            tab_list_text: crate::tab_list_text::TabListText::new(),
            view_area: crate::view_area::ViewArea::default(),
            // Vanilla seeds `chunkBatchStartTime` in the calculator's
            // constructor, so a `chunk_batch_finished` that somehow arrives
            // before any `chunk_batch_start` still measures a finite interval.
            chunk_batch: crate::chunk_batch::ChunkBatchSizeCalculator::new(0),
            ticking: crate::ticking::TickRateManager::default(),
            client_state: crate::client_state::ClientState::default(),
            session: session_state,
            // M79. A fresh `Hud` has already run `resetTitleTimes()`, so the
            // default carries 10 / 70 / 20 rather than three zeros — see
            // `hud_state::TitleOverlay::default`.
            hud: crate::hud_state::HudState::default(),
            bundle,
            clock_epoch: std::time::Instant::now(),
            number_formats: self.data.number_formats,
            // Filled from the authenticated profile when there is one. Offline
            // mode leaves it `None`, so `own_ping_ms` reports nothing rather
            // than guessing which tab entry is us -- a name match would pick
            // the wrong player the moment two share a prefix.
            own_uuid: auth.map(|a| a.uuid),
            enchantments,
            chat_types,
            trim_materials,
            trim_patterns,
            cat_variants,
            wolf_variants,
            frog_variants,
            tags,
            world,
            player: PlayerState::at(0.5, 80.0, 0.5),
            collide,
            entity_push: Vec::new(),
            warden_type_id: None,
            armadillo_type_id: None,
            allay_type_id: None,
            pillager_type_id: None,
            sheep_type_id: None,
            creaking_type_id: None,
            bee_type_id: None,
            guardian_type_id: None,
            elder_guardian_type_id: None,
            sniffer_type_id: None,
            player_type_id: None,
            variant_type_ids: crate::VariantKinds::default(),
            take_item_kinds: crate::TakeItemKinds::default(),
            block_event_types: Default::default(),
            powered_skull_states: Default::default(),
            conduit_states: Default::default(),
            water_states: Vec::new(),
            conduit_frame_states: Vec::new(),
            entity_classes: None,
            entity_types: None,
            attribute_registry: None,
            swing_data: None,
            recipe_display_ids: None,
            swing_effect_ids,
            global_bits,
            dim_types,
            overworld_clock_id,
            spawned: false,
            corrections: 0,
            teleports: 0,
            block_updates: 0,
            mounts: crate::motion::Mounts::new(),
            local_attributes: rewo_world::attributes::EntityAttributes::default(),
            local_player_data: crate::local_player_data::LocalPlayerData::default(),
            vehicle_pose: None,
            motion_stats: MotionStats::default(),
            day_ticks: None,
            weather: rewo_world::weather::WeatherState::default(),
            border: rewo_world::border::WorldBorder::default(),
            inventory: rewo_world::inventory::Inventory::default(),
            menus: rewo_world::menu::Menus::new(),
            waypoints: crate::waypoints::WaypointStore::default(),
            component_names: None,
            stack_details: crate::item_stack::StackDetails::default(),
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
            sound_events: Vec::new(),
            game_state: crate::game_event::ClientGameState::default(),
            abilities: rewo_world::abilities::Abilities::default(),
            flight: rewo_world::abilities::FlightControl::default(),
            chat_log: Vec::new(),
            chat_events: Vec::new(),
            signature_cache: crate::chat_wire::MessageSignatureCache::default(),
            chat_clock_millis: 0,
            chat: rewo_world::chat::ChatComponent::new(),
            commands: crate::commands::CommandTree::default(),
            suggestions: crate::suggestion_wire::SuggestionProviderState::new(),
            suggestion_reply: None,
            command_argument_types: None,
            lang: None,
            chat_overlay: None,
            health: 20.0,
            food: 20,
            dead: false,
            hardcore: false,
            score: 0,
            respawn_epoch: 0,
            death: None,
            stats: Default::default(),
            awarded_stats: None,
            disconnect: None,
            disconnect_cause: None,
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

    /// Cap on [`Self::sound_events`].
    ///
    /// `particle_events` needs no cap because the renderer drains it every
    /// frame. This queue has **no consumer at all** until playback ships, so
    /// an uncapped push would grow for the whole session — a busy server
    /// sends a few sounds per tick, which is megabytes an hour of strings
    /// nobody reads. Dropping the oldest keeps the recent ordering (the part
    /// a `stop_sound` depends on) and bounds the memory. Sized to a few
    /// seconds of a loud scene, so a real consumer draining per frame never
    /// reaches it.
    pub const MAX_PENDING_SOUNDS: usize = 256;

    fn push_sound_event(&mut self, ev: crate::sounds::SoundEvent) {
        if self.sound_events.len() >= Self::MAX_PENDING_SOUNDS {
            self.sound_events.remove(0);
        }
        self.sound_events.push(ev);
    }

    /// Drain the decoded sound queue. The seam a playback layer reads;
    /// unused today, which is why the queue is capped above.
    pub fn take_sound_events(&mut self) -> Vec<crate::sounds::SoundEvent> {
        std::mem::take(&mut self.sound_events)
    }

    /// Apply one `ClientboundGameEventPacket` body — all fourteen types (M71).
    ///
    /// Decoded **once**; the weather four go to [`Self::weather`] (which owns
    /// the counter-intuitive start/stop rule), the other ten to
    /// [`Self::game_state`], and three of them additionally queue a
    /// client-local sound at the player's position.
    ///
    /// A short body or an unregistered type id does nothing, which is
    /// vanilla's own behaviour — see [`crate::game_event::decode`].
    fn apply_game_event(&mut self, body: &[u8]) {
        // Deliberately no logic here: `PlaySession` owns a socket and has no
        // unit tests, so a fan-out written at this call site is unwitnessed —
        // a mutation battery proved it, surviving the removal of each branch
        // in turn. Everything lives in `game_event::apply`, which is tested.
        let applied = crate::game_event::apply(
            body,
            &mut self.weather,
            &mut self.game_state,
            &self.player,
            &mut self.abilities,
        );
        for s in applied.sounds {
            self.push_sound_event(crate::sounds::SoundEvent::Local(s));
        }
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

    /// `(bundles closed, largest run)` — M78's live witness that the bundle
    /// path fired at all. A run that reports `0` proves nothing about bundling
    /// beyond "it did not break the session", which is equally true of a
    /// machine that never ran.
    pub fn bundle_stats(&self) -> (u64, usize) {
        self.bundle.stats()
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

    /// The local player as a pickup collector (M81): its id, and vanilla's
    /// `(getY() + getEyeY()) / 2` chest point from the real eye height.
    ///
    /// `None` before login, which is when `minecraft.player` is null too.
    fn local_collector(&self) -> Option<(i32, [f64; 3])> {
        let p = &self.player;
        self.player_id
            .map(|pid| (pid, [p.x, (p.y + p.eye_y()) / 2.0, p.z]))
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
        // M79: the title/action-bar countdowns (`Hud.tick`), the local
        // player's `tickCount` (which the XP bar's display window is measured
        // against) and `ItemCooldowns.tick`. Three vanilla call sites, one
        // cadence — `Minecraft.tick` drives the first and `ClientLevel`'s
        // entity tick the other two, and all three run once per client tick.
        // Placed beside `visual_effects` because it is the same kind of thing:
        // a purely local clock the server never re-states.
        self.hud.tick();
        // M38: publish what the local player is holding into the entity table,
        // so its swing runs through M19's machine like any other entity's.
        //
        // The server never tells us our own equipment — `set_equipment` is for
        // *other* entities — but M34's inventory knows, and the swing duration
        // is a function of the held item. This is the join between the two.
        self.publish_local_hands();
        // `LocalPlayer.aiStep`'s `this.wasFallFlying = this.isFallFlying();`
        // (M141e). Once per tick, and that cadence is what makes the elytra
        // sound's rising edge terminate rather than fire on every packet.
        self.local_player_data.tick();
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
        // `ClientLevel.tick` advances the border before anything moves, and it
        // must stay ahead of the physics below: `getMinX()` is the *previous*
        // tick's size, so ticking after the move would clip this tick's
        // movement against a box two ticks stale. M80.
        self.border.tick();
        // `ClientLevel.removeBlockBreakingProgress` — a breaker who wandered
        // off sends no "stop", so the record is retired by silence (M81).
        self.world.destruction.tick(gt);
        // `ItemPickupParticle.tick`, after the entity lerps above so the
        // collector's `cur` is this tick's (M81). The local player is not in
        // the entity table, so its own id resolves through the same fallback
        // `handleTakeItemEntity` uses.
        {
            let local = self.local_collector();
            let (entities, pickups) = (&self.world.entities, &mut self.world.pickups);
            pickups.tick(|id| crate::collector_chest(entities, id, local));
        }
        if self.spawned && self.is_mounted() {
            // M68. A passenger does not travel under its own power: vanilla's
            // `ClientLevel.tickNonPassenger` skips passengers entirely, and
            // the vehicle places its riders through `positionRider`. Running
            // gravity and walk input here would walk the player off the boat
            // — and the server would never say so, because it does not
            // validate a passenger's movement at all
            // (`ServerGamePacketListenerImpl`: `if (this.player.isPassenger())`
            // snaps rotation and returns). So this is a divergence `CORRECTIONS`
            // is structurally unable to catch, which is exactly why it is
            // handled here rather than left for the meter to find.
            //
            // **The ride offset is not modelled.** Vanilla seats a rider at
            // the vehicle's `getPassengerAttachmentPoint`, which is per
            // vehicle type (and per pose, for a boat with two seats). Rewo
            // snaps to the vehicle's own position, so a mounted player sits
            // roughly a third of a block low. Riding *visuals* are a feature;
            // this is the minimum that keeps the player attached to its
            // vehicle instead of falling through the world.
            if let Some(vehicle) = self.local_vehicle() {
                if let Some(e) = self.world.entities.get(vehicle) {
                    self.player.x = e.x;
                    self.player.y = e.y;
                    self.player.z = e.z;
                }
            }
            self.send_movement(input)?;
        } else if self.spawned {
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
            // M75. `LocalPlayer.aiStep` runs its flight prologue *before*
            // `super.aiStep()` reaches `travel`, so the toggle and the vertical
            // impulse both land in this tick's movement. All three steps live
            // in `rewo_world::abilities`, which is unit-tested; this is the
            // adapter, deliberately with no logic of its own (M71's lesson —
            // `PlaySession` owns a socket and has no tests, so a fan-out
            // written here would be unwitnessed).
            let spectator = self.game_state.game_mode().is_some_and(|m| m.is_spectator());
            let step = self.flight.before_travel(
                &mut self.abilities,
                &mut self.player,
                input,
                spectator,
                false,
            );
            if step.jump_from_ground {
                // `jumpFromGround()` on a standing toggle. `physics::tick_with`
                // fires it too when the jump key is held on the ground, so this
                // only covers the toggle's own call.
                self.player.vy = self.player.vy.max(0.42);
            }
            let mut owes_packet = step.abilities_changed;
            let abilities = self.abilities;
            physics::tick_with(
                &mut self.player,
                input,
                &abilities,
                spectator,
                Some(self.border.collision()),
                &shapes,
            );
            owes_packet |= self
                .flight
                .after_travel(&mut self.abilities, &self.player, spectator);
            self.collide = collide;
            if owes_packet {
                self.send_abilities()?;
            }
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

    /// `LocalPlayer.onUpdateAbilities()` — tell the server we changed `flying`.
    ///
    /// Sent only when the *client* made the change (a toggle, the spectator
    /// force-on, or the landing clause); a change that arrived in a
    /// `ClientboundPlayerAbilitiesPacket` is already the server's own view and
    /// echoing it back would be noise.
    fn send_abilities(&mut self) -> Result<(), String> {
        let p =
            crate::abilities::serverbound(self.ids.sb_play_player_abilities, self.abilities.flying);
        self.send(p)
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
                        // `Connection.channelInactive` — the socket went away
                        // with no packet. Vanilla's reason is
                        // `disconnect.endOfStream` and its details carry
                        // neither a report nor a link.
                        self.disconnect = Some("connection closed".into());
                        self.disconnect_cause =
                            Some(rewo_world::disconnect_screen::DisconnectCause::EndOfStream);
                    }
                    return Ok(());
                }
            };
            let mut pos = 0;
            let Ok(id) = rewo_proto::varint::read_varint(&packet, &mut pos) else {
                continue;
            };
            // M78 — bundling, wrapped *around* the dispatch chain rather than
            // folded into it. `PacketBundlePacker` sits between the frame
            // decoder and the listener in vanilla's pipeline, and it sits in
            // the same place here: the `else if` ladder in `handle_packet`
            // stays a plain list of ids and never learns that bundles exist.
            match self.bundle.feed(id, &packet[pos..]) {
                crate::bundle::Feed::Apply => self.handle_packet(id, &packet[pos..])?,
                // The opening delimiter, or a sub-packet buffered inside an
                // open bundle. An unterminated bundle is *withheld* — the
                // buffer survives this function returning, which is the whole
                // reason bundling is worth having: a socket that hands over a
                // bundle in two reads must not apply the first half.
                crate::bundle::Feed::Buffered => {}
                crate::bundle::Feed::Flush => {
                    // `handleBundlePacket` is a plain `for` loop over the
                    // sub-packets on one scheduled task, so nothing renders
                    // between them. Here that falls out of applying the run
                    // inside a single drain.
                    for (sub_id, sub_body) in self.bundle.take() {
                        self.handle_packet(sub_id, &sub_body)?;
                    }
                }
                // Vanilla throws on the Netty pipeline and the connection dies.
                // Rewo ends the session the same way it ends one for a closed
                // socket rather than recovering: a client that carried on past
                // a malformed bundle would be applying a run the server never
                // meant to send as one.
                crate::bundle::Feed::Fatal(reason) => {
                    log::warn!("net: bundle: {reason}");
                    if self.disconnect.is_none() {
                        self.disconnect = Some(format!("bundle: {reason}"));
                        // A malformed packet stream is vanilla's
                        // `onPacketError`, which is one of the two paths that
                        // *does* offer the server's bug-report link — the whole
                        // point of the link being that it reports a server bug.
                        self.disconnect_cause =
                            Some(rewo_world::disconnect_screen::DisconnectCause::ClientError);
                    }
                    return Ok(());
                }
            }
        }
    }

    fn handle_packet(&mut self, id: i32, body: &[u8]) -> Result<(), String> {
        let ids = &self.ids;
        // Resolved before the ladder because the `take_item_entity` arm below
        // borrows `self.world` mutably and cannot read `self.player` too.
        let local_collector = self.local_collector();
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
        } else if id == ids.cb_play_player_rotation || id == ids.cb_play_player_look_at {
            // M76. An arm of its own rather than a `route_*` tail call because
            // one of the two answers the server immediately and the other does
            // not — see `apply_player_rotation`.
            self.apply_player_rotation(id, body)?;
        } else if id == ids.cb_play_chunk_batch_start {
            // M74. Empty body — `handleChunkBatchStart` is one call, and it is
            // the half that makes the reply below adaptive instead of a
            // differently-wrong constant.
            let now = self.now_nanos();
            self.chunk_batch.on_batch_start(now);
        } else if id == ids.cb_play_chunk_batch_finished {
            // M74. Was `p.f32(64.0)` — an ~18x over-bid against vanilla's
            // seeded opening 3.5, on every batch of every session, never
            // adapting. `batchSize` is a VarInt, and a body that fails to
            // decode still gets a reply: vanilla always answers, and going
            // silent here would stall the server's chunk pipeline outright.
            let now = self.now_nanos();
            match crate::chunk_batch::read_chunk_batch_finished(body) {
                Ok(batch_size) => self.chunk_batch.on_batch_finished(batch_size, now),
                Err(err) => log::debug!("net: chunk_batch_finished decode: {err}"),
            }
            let mut p = PacketWriter::packet(self.ids.sb_play_chunk_batch_received);
            p.f32(self.chunk_batch.desired_chunks_per_tick());
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
        } else if id == ids.cb_play_explode {
            // M68. Only the physics prefix is consumed — see `motion::Explosion`.
            match crate::motion::read_explode(body) {
                Ok((e, _used)) => self.apply_explode(&e),
                Err(err) => log::debug!("net: explode decode: {err}"),
            }
        } else if id == ids.cb_play_set_entity_motion {
            match crate::motion::read_set_entity_motion(body) {
                Ok(m) => self.apply_set_entity_motion(&m),
                Err(err) => log::debug!("net: set_entity_motion decode: {err}"),
            }
        } else if id == ids.cb_play_move_vehicle {
            match crate::motion::read_move_vehicle(body) {
                Ok(v) => self.apply_move_vehicle(&v),
                Err(err) => log::debug!("net: move_vehicle decode: {err}"),
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
            // M140 — the same packet also asks for a sound, and the two are
            // independent: 2001 (a block breaking) is both, 1000 (a dispenser)
            // is sound only, and 2000 (smoke) is particle only. Deriving one
            // from the other would lose whichever the id does not have.
            if Some(id) == ids.cb_play_level_event {
                if let Some(s) = crate::route_level_event_sound(body) {
                    self.push_sound_event(s);
                }
            }
        } else if id == ids.cb_play_sound
            || id == ids.cb_play_sound_entity
            || id == ids.cb_play_stop_sound
        {
            // M63 — decode only. The three bodies differ enough that the kind
            // has to come from the id; deriving it from the body would mean
            // guessing between a var-int entity id and a fixed i32 position.
            let kind = if id == ids.cb_play_sound {
                crate::SoundPacketKind::Positioned
            } else if id == ids.cb_play_sound_entity {
                crate::SoundPacketKind::OnEntity
            } else {
                crate::SoundPacketKind::Stop
            };
            if let Some(ev) = crate::route_sound(kind, body) {
                log::debug!("net: sound event {ev:?}");
                self.push_sound_event(ev);
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
            if let Ok((eid, type_id)) = crate::read_add_entity(&mut r, &mut self.world) {
                self.post_add_entity_sound_instance(eid, type_id);
            }
        } else if id == ids.cb_play_remove_entities {
            let mut r = PacketReader::new(body);
            if let Ok(n) = r.count("remove entities", 1) {
                for _ in 0..n {
                    if let Ok(eid) = r.varint() {
                        self.world.entities.remove(eid);
                        // M68: a removed entity cannot still be riding or be
                        // ridden. Leaving the seat behind would strand the
                        // local player "mounted" on a vehicle that no longer
                        // exists, which suppresses its physics forever.
                        self.mounts.remove_entity(eid);
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
                sheep: self.sheep_type_id,
                creaking: self.creaking_type_id,
                player: self.player_type_id,
                bee: self.bee_type_id,
                guardian: self.guardian_type_id,
                elder_guardian: self.elder_guardian_type_id,
                variant_kinds: self.variant_type_ids,
                classes: self.entity_classes.as_deref(),
                components: self.swing_data.as_ref().map(|d| d.components),
            },
        ) {
            // M141e: and the local player's own, which the router cannot store.
            // `handleSetEntityData` is `if (entity != null)` and vanilla's
            // local player IS in the level, so the server's metadata for you
            // is processed like anyone else's — but `EntityTable` has no row
            // for you, so the router returns early on your id and drops it.
            // Same asymmetry M73 hit with attributes, same fix: decode the
            // body a second time when it names the camera entity.
            self.capture_local_metadata(body);
            // Entity metadata (custom name, pose, gesture state, cube size, and
            // the polymorphic index-16 BOOLEAN → Allay dancing / baby). The
            // Allay dance counters then advance in `tick_lerp`.
            //
            // M82: and the local player's own, which the line above cannot
            // store for the reason M73 records two arms down — the entity
            // table has no row for you. `ServerEntity.sendChanges` broadcasts
            // through `sendToTrackingPlayersAndSelf`, so the packet really
            // does arrive.
            crate::apply_local_player_score(body, self.player_id, &mut self.score);
        } else if crate::route_damage_event(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M21: the damage response — arms the hurt clock (red overlay) and
            // kicks the walk animation, for a tracked living entity only.
        } else if crate::route_hurt_animation(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
            self.player_type_id,
            self.player_id,
        ) {
            // M81: `damage_event`'s twin. It arms the same clock and, for a
            // player only, stores the yaw the camera tilt leans away from —
            // the one thing `damage_event` never carries.
        } else if crate::route_player_combat_kill(
            id,
            body,
            ids,
            self.player_id,
            &mut self.death,
        ) {
            // M82: you died. The id is always your own, so this is resolved
            // against the local-player door and never against the entity
            // table — `REWO_PLAN.md` §0.0 gotcha 13.
            //
            // `handlePlayerCombatKill`'s own branch, transcribed: with the
            // death screen suppressed the client respawns *immediately* and
            // never records a death at all. Nothing downstream then has to
            // know the rule.
            match crate::death_action(self.death.take(), self.game_state.show_death_screen()) {
                crate::DeathAction::ShowScreen(kill) => self.death = Some(kill),
                crate::DeathAction::RespawnNow => self.perform_respawn()?,
                crate::DeathAction::None => {}
            }
        } else if crate::route_award_stats(id, body, ids, &mut self.awarded_stats) {
            // M84: your own statistics, in reply to a `REQUEST_STATS` this
            // client sent when the screen opened. `setValue`, not `increment`,
            // and the map is never cleared — see `StatsCounter::apply`.
            if let Some(pairs) = self.awarded_stats.take() {
                self.stats.apply(&pairs);
            }
        } else if crate::route_block_destruction(
            id,
            body,
            ids,
            &mut self.world.destruction,
            self.game_time.unwrap_or(0),
        ) {
            // M81: somebody else's mining progress. The stage byte is
            // unsigned, and anything outside 0..10 retires the record.
        } else if crate::route_take_item_entity(
            id,
            body,
            ids,
            &mut self.world,
            crate::TakeItemKinds {
                local_player: local_collector,
                ..self.take_item_kinds
            },
        ) {
            // M81: the pickup animation, and the *client-side* removal of the
            // collected entity — this packet is not a heads-up that a
            // `remove_entities` is coming, it is the removal.
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
            //
            // M73: and the local player's own, which the line above cannot
            // store. `handleUpdateAttributes` looks the entity up in the level
            // and the local player is in it; Rewo's `EntityTable` holds only
            // entities the server sent an `add_entity` for, and it never sends
            // one for you. So the same body is decoded a second time and kept
            // beside the table when it names the camera entity — without it
            // `entity_interaction_range` would be permanently the registered
            // default and a creative player's crosshair would stop two blocks
            // short.
            self.capture_local_attributes(body);
        } else if crate::route_inventory(
            id,
            body,
            ids,
            self.swing_data.as_ref().map(|d| d.components),
            &mut self.inventory,
            &mut self.menus,
            Some(&mut self.stack_details),
        ) {
            // M34: the player's own inventory — contents, one slot, or the
            // server moving the selection. M87: or an open container's, since
            // the same two packet ids address either menu.
        } else if crate::route_tags(id, body, ids, &mut self.tags) {
            // M69 — a datapack reload's `update_tags`. The join-time copy
            // arrives during configuration and is applied there; this arm is
            // the mid-session one. Per-registry wholesale replacement, so a
            // body that fails to decode is dropped whole rather than
            // half-applied.
        } else if crate::route_view_area(id, body, ids, &mut self.view_area) {
            // M67 — the server's view area. Decode and state only; nothing
            // evicts a column or gates a tick on it yet.
        } else if crate::route_border(id, body, ids, &mut self.border) {
        } else if crate::route_ticking(id, body, ids, &mut self.ticking) {
            // M74 — `/tick rate`, `/tick freeze`, `/tick step`. Decode and
            // state only; the 20 Hz loop does not consult it yet.
        } else if crate::route_session(id, body, ids, &mut self.session) {
            // M78 — the brand, the MOTD, the game rules, the cookie jar, the
            // two vestigial combat packets, and disguised chat.
            //
            // The chat lines are drained here rather than written by the router
            // so `crate::session` needs no reference to this type. They join
            // the same log `system_chat` and `player_chat` push to, and at the
            // same fidelity: the *raw* message, not the decoration, because
            // decorating needs the `minecraft:chat_type` registry Rewo does not
            // parse.
            // `handleDisguisedChatMessage` adds it with `GuiMessageTag.system()`
            // and a null signature — it is not a signed player message, so it
            // can never be the target of a `delete_chat`.
            for chat in self.session.take_chat() {
                // M127: decorated, as `handleDisguisedChatMessage` does. A
                // `/say` therefore reads `[Server] hi` rather than `hi`.
                let decorated = self.decorate_chat(&chat.message, &chat.bound);
                let spans = self.chat_component_spans(&decorated);
                let line = rewo_world::chat_style::plain_text(&spans);
                if line.is_empty() {
                    continue;
                }
                self.chat_log.push(line);
                self.chat_events.push(crate::chat_wire::ChatEvent::Message {
                    text: spans,
                    signature: None,
                    tag: Some(rewo_world::chat::MessageTag::SYSTEM),
                    source: rewo_world::chat::MessageSource::Player,
                });
            }
        } else if crate::route_waypoint(id, body, ids, &mut self.waypoints) {
            // M83 — the locator bar. `handleWaypoint` is two lines: the thread
            // check and `packet.apply(this.waypointManager)`. There is no
            // gamerule check and no range check on this side; the server has
            // already decided both, and the client draws whatever it was told
            // to track. See `crate::waypoints`.
        } else if crate::route_hud_state(id, body, ids, &mut self.hud) {
            // M79 — the title overlay, the XP gauge and the item-cooldown map.
            // Every one of the seven writes state a renderer reads; none of
            // them answers the server. See `crate::hud_state`.
        } else if Some(id) == ids.cb_play_cookie_request {
            // M78 closes a hole it would otherwise have shipped around: the
            // *play-state* `cookie_request` was answered only by the M1-era
            // `Connection::run_play` harness, never by this session, so the
            // real client left it unanswered entirely. `store_cookie` fills a
            // jar whose only observable consequence is this reply, and a jar
            // nothing reads is not a feature.
            //
            // `handleRequestCookie` — `send(new ServerboundCookieResponsePacket(
            // key, serverCookies.get(key)))`. A key we hold answers with its
            // payload; one we do not answers with nothing, which is the
            // behaviour the whole client had before M78.
            let key = PacketReader::new(body).identifier().unwrap_or_default();
            if let Some(resp_id) = ids.sb_play_cookie_response {
                let payload = self.session.cookie(&key).map(<[u8]>::to_vec);
                let resp =
                    crate::session::write_cookie_response(resp_id, &key, payload.as_deref());
                self.send(resp)?;
            }
        } else if crate::route_client_state(
            id,
            body,
            ids,
            &mut self.client_state,
            &self.world.entities,
            self.player_id,
        ) {
            // M74 — difficulty, the camera's target, and the container-close
            // latch. Decode and state; the app reads the camera and the latch.
            //
            // M87 hangs the menu close off this arm rather than off
            // `route_menu`, because this chain is a sequence of `else if`s and
            // `container_close` already belongs to this seam. A second seam
            // claiming the same id would either steal it from M74's counter or
            // never see it, depending only on which arm came first.
            if id == ids.cb_play_container_close {
                self.menus.apply_close();
            }
        } else if crate::route_menu(id, body, ids, &mut self.menus) {
            // M87 — `open_screen` and `container_set_data`. State only so far:
            // the menu's own item slots arrive when `Inventory` becomes a
            // layout-driven menu, and nothing renders it yet.
        } else if id == ids.cb_play_merchant_offers {
            // M93u. The coverage doc filed this as class C; it needed nothing
            // Rewo had not already built — `ItemStack` (M34/M41) and the
            // `TypedDataComponent` walker M52e wrote for `can_place_on`.
            // The component ids ride on `swing_data`, which is where every
            // other `read_optional` caller finds them; with no registry yet
            // there is nothing to decode a stack against, so the packet is
            // dropped rather than guessed at.
            match self
                .swing_data
                .as_ref()
                .map(|d| d.components)
                .ok_or_else(|| "merchant_offers: no component registry".to_string())
                .and_then(|ids| crate::merchant::parse(body, ids))
            {
                Ok(m) => self.merchant = Some(m),
                // A short or malformed body is dropped whole rather than
                // applied in part: half a trade list is worse than none, since
                // the index a click sends addresses the list by position.
                Err(e) => log::warn!("net: {e}"),
            }
        } else if id == ids.cb_play_recipe_book_add
            || id == ids.cb_play_recipe_book_remove
            || id == ids.cb_play_recipe_book_settings
            || id == ids.cb_play_place_ghost_recipe
        {
            // M93y. The BOOK is a subsystem Rewo does not have — tabs, search,
            // filtering, ghost placement — and this is the decode half only.
            // It is dispatched rather than left resolved-but-ignored because
            // that class is the one `REWO_PACKET_COVERAGE.md` keeps at zero: a
            // packet whose id resolves and whose body is dropped reads as
            // handled to every grep.
            self.apply_recipe_book(id, body);
        } else if id == ids.cb_play_game_event {
            // M33 took the four weather ids; M71 took the other ten. One
            // decode feeds the weather levels, the client game state and the
            // local sound queue — see `apply_game_event`.
            self.apply_game_event(body);
        } else if id == ids.cb_play_player_abilities {
            // M75. `handlePlayerAbilities` is six assignments and nothing else —
            // no derived state, no packet in reply. In particular it does NOT
            // touch `may_build` (absent from the wire) and does NOT feed
            // `walkingSpeed` into the movement speed.
            match crate::abilities::PlayerAbilities::parse(body) {
                Ok(p) => p.apply_to(&mut self.abilities),
                // A short body is the one case vanilla's reader would throw on.
                // Dropping it leaves the abilities we already had, which is
                // closer to "the packet never arrived" than a partial apply.
                Err(e) => log::warn!("net: player_abilities: {e}"),
            }
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
            //
            // M141g: and the two SOUND events in the same switch. They are
            // handled here rather than inside `route_entity_event` because
            // that seam writes the entity table and these push a sound, and
            // the body is two fixed fields either way.
            self.entity_event_sound(body);
        } else if crate::route_move_minecart_along_track(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M77. An experimental-movement minecart's ONLY movement channel —
            // `ServerEntity.sendChanges` sends it instead of `move_entity_pos`
            // / `teleport_entity` / `entity_position_sync`, so the generic
            // 3-tick lerp is never armed for one of these carts. The schedule
            // is traversed in `EntityTable::tick_lerp`, before the riders are
            // placed; see `rewo_world::minecart` for why both interpolations
            // stay live.
        } else if crate::route_set_entity_link(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M77. The leash holder id, stored and not drawn.
        } else if crate::route_projectile_power(
            id,
            body,
            ids,
            &mut self.world.entities,
            self.entity_classes.as_deref(),
        ) {
            // M77. `AbstractHurtingProjectile.accelerationPower`.
        } else if id == ids.cb_play_set_passengers {
            // Riding (M70). Consumed for `Entity.isVehicle()`, which
            // suppresses a ridden entity's floating label. It does **not** yet
            // move a passenger onto its vehicle's position — that is the
            // separate gap `REWO_PACKET_COVERAGE.md` records against this
            // packet, and this milestone does not close it.
            crate::route_set_passengers(id, body, ids, &mut self.world.entities);
            // Riding, the physics half (M68). Disjoint from the label half
            // above and deliberately a second read of the same slice: M70
            // wants the riding graph, M68 wants the local player's own mount
            // state, and folding either into the other's walk would couple two
            // milestones that have no reason to share a decode.
            match crate::motion::read_set_passengers(body) {
                Ok(p) => self.apply_set_passengers(&p),
                Err(err) => log::debug!("net: set_passengers decode: {err}"),
            }
        } else if id == ids.cb_play_set_player_team {
            // Scoreboard teams (M62). A body we cannot decode is dropped
            // whole rather than half-applied: the packet's three sections are
            // positional, so a short read means the roster we did get is not
            // the roster the server sent.
            match crate::teams::parse_set_player_team(body) {
                Ok(p) => {
                    self.scoreboard.teams.apply(&p);
                }
                Err(e) => log::debug!("play: set_player_team parse: {e}"),
            }
        } else if id == ids.cb_play_set_objective {
            // M65 — the scoreboard's other half. Every arm below drops a body
            // it cannot decode whole rather than half-applying it, for the
            // same reason `set_player_team` does: these packets are
            // positional, so a short read means the values we did get are not
            // the values the server sent.
            match crate::scoreboard::parse_set_objective(body, self.number_formats) {
                Ok(p) => {
                    self.scoreboard.apply_set_objective(&p);
                }
                Err(e) => log::debug!("play: set_objective parse: {e}"),
            }
        } else if id == ids.cb_play_set_score {
            match crate::scoreboard::parse_set_score(body, self.number_formats) {
                Ok(p) => {
                    self.scoreboard.apply_set_score(&p);
                }
                Err(e) => log::debug!("play: set_score parse: {e}"),
            }
        } else if id == ids.cb_play_reset_score {
            match crate::scoreboard::parse_reset_score(body) {
                Ok(p) => {
                    self.scoreboard.apply_reset_score(&p);
                }
                Err(e) => log::debug!("play: reset_score parse: {e}"),
            }
        } else if id == ids.cb_play_set_display_objective {
            match crate::scoreboard::parse_set_display_objective(body) {
                Ok(p) => self.scoreboard.apply_set_display_objective(&p),
                Err(e) => log::debug!("play: set_display_objective parse: {e}"),
            }
        } else if id == ids.cb_play_boss_event {
            match crate::boss_bar::parse_boss_event(body) {
                Ok(p) => {
                    self.boss_bars.apply(&p);
                }
                Err(e) => log::debug!("play: boss_event parse: {e}"),
            }
        } else if id == ids.cb_play_tab_list {
            match crate::tab_list_text::parse_tab_list(body) {
                Ok(p) => self.tab_list_text.apply(&p),
                Err(e) => log::debug!("play: tab_list parse: {e}"),
            }
        } else if id == ids.cb_play_player_info_update {
            self.apply_player_info(body);
        } else if id == ids.cb_play_player_info_remove {
            let mut r = PacketReader::new(body);
            if let Ok(n) = r.count("player info removes", 16) {
                for _ in 0..n {
                    if let Ok(uuid) = r.uuid() {
                        self.world.entities.remove_name(uuid);
                        // A departed player's ping is not stale, it is gone --
                        // keeping it would let the tab list quote a number for
                        // someone who left. Vanilla drops the whole
                        // `PlayerInfo`, so the mode and the list order go with
                        // it. The TEAM does not: `handlePlayerInfoRemove`
                        // never touches the scoreboard, and a team outlives
                        // its members leaving.
                        self.latency.remove(&uuid);
                        self.gamemodes.remove(&uuid);
                        self.tab_list_orders.remove(&uuid);
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
                // `Player.isDeadOrDying()`'s health half. **This used to send
                // `PERFORM_RESPAWN` from here** (M3, so the headless bot could
                // recover), and that is not what a vanilla client does:
                // `handleSetHealth` assigns the three fields and nothing else.
                // Respawning is a *screen* action, so M82 moved it to
                // `player_combat_kill` — which is where vanilla decides
                // between the death screen and an immediate respawn — and left
                // the flag here. A harness with no screen respawns by draining
                // [`Self::take_death`], which is the same branch vanilla takes
                // when `shouldShowDeathScreen()` is false.
                self.dead = h <= 0.0;
            }
        } else if Some(id) == ids.cb_play_system_chat {
            let mut r = PacketReader::new(body);
            if let Ok(packet) = crate::chat_wire::SystemChat::read(&mut r) {
                // `handleSystemChat` branches on `overlay`: true goes to
                // `handleOverlay`, which is `gui.setOverlayMessage` — the
                // ACTION BAR, not the chat log. Reading the component and
                // dropping the bool (which is what this arm used to do) put
                // every `/title actionbar` line into chat.
                // M125: resolved here, where the language table is, rather
                // than at the wire. `handleSystemChat` renders the component,
                // and a component whose contents are a `TranslatableContents`
                // renders as its translation — so flattening before the lookup
                // put `multiplayer.player.joined` on screen where vanilla puts
                // "Steve joined the game".
                let spans = self.chat_component_spans(&packet.content);
                let content = rewo_world::chat_style::plain_text(&spans);
                if packet.overlay {
                    // The action bar draws one flat string, so the spans stop
                    // here rather than being threaded through a second render.
                    self.chat_events
                        .push(crate::chat_wire::ChatEvent::Overlay(content));
                } else if !content.is_empty() {
                    self.chat_log.push(content);
                    self.chat_events.push(crate::chat_wire::ChatEvent::Message {
                        text: spans,
                        signature: None,
                        tag: Some(rewo_world::chat::MessageTag::SYSTEM_SINGLE_PLAYER),
                        source: rewo_world::chat::MessageSource::SystemServer,
                    });
                }
            }
        } else if Some(id) == ids.cb_play_player_chat {
            let mut r = PacketReader::new(body);
            match crate::chat_wire::PlayerChat::read(&mut r) {
                Ok(chat) => {
                    // `MessageSignatureCache.push` runs on receipt and BEFORE
                    // anything decides whether to show the message, because a
                    // later `delete_chat` may address this signature by the
                    // index this push assigns it. Feeding the cache only from
                    // *displayed* messages would leave those indices pointing
                    // at the wrong signatures.
                    let last_seen: Vec<Box<crate::chat_wire::Signature>> = chat
                        .body
                        .last_seen
                        .iter()
                        .filter_map(|p| self.signature_cache.resolve(p))
                        .collect();
                    self.signature_cache
                        .push(&last_seen, chat.signature.as_deref());
                    let received = self.chat_clock_millis;
                    // Bound before the `if let` on purpose: the two closures
                    // borrow `self`, and an `if let` scrutinee's temporaries
                    // live for the whole block, which would collide with the
                    // `self.chat_log.push` inside it.
                    let outcome = crate::chat_wire::show_message(
                        &chat,
                        received,
                        &|content| self.decorate_chat(content, &chat.bound),
                        &|tag| self.chat_component_text(tag),
                    );
                    if let crate::chat_wire::ChatOutcome::Shown { content, tag } = outcome {
                        // M127: `content` is the DECORATED component now, so
                        // the store gets `<Steve> hi` rather than `hi`.
                        //
                        // Signed chat is a plain `String` on the wire, not a
                        // component — but vanilla still renders it through
                        // `StringDecomposer.iterateFormatted`, so a server's
                        // `§e` is a colour and not two glyphs of garbage. That
                        // survives the move to a component path: the content is
                        // wrapped as `Component.literal`, and `chat_style`'s
                        // walk runs `push_legacy` over a literal's text.
                        let spans = self.chat_component_spans(&content);
                        self.chat_log
                            .push(rewo_world::chat_style::plain_text(&spans));
                        self.chat_events.push(crate::chat_wire::ChatEvent::Message {
                            text: spans,
                            signature: chat.signature,
                            tag,
                            source: rewo_world::chat::MessageSource::Player,
                        });
                    }
                }
                Err(e) => log::warn!("net: player_chat decode failed: {e}"),
            }
        } else if id == ids.cb_play_commands {
            // The argument-type registry is a BUILT-IN one, so it comes from
            // the report rather than the wire (M92's rule) — and without it
            // the tree cannot be read past its first non-singleton argument,
            // which is why a missing table is a warn-and-drop rather than a
            // partial parse.
            match self.command_argument_types.as_ref() {
                Some(types) => {
                    match crate::commands::read_commands(body, &|i| types.name(i)) {
                        Ok(tree) => {
                            log::info!(
                                "net: command tree — {} nodes, {} top-level",
                                tree.nodes.len(),
                                tree.top_level().len()
                            );
                            self.commands = tree;
                        }
                        Err(e) => log::warn!("net: commands decode failed: {e}"),
                    }
                }
                None => log::debug!("net: commands arrived before the argument-type table"),
            }
        } else if id == ids.cb_play_command_suggestions {
            // `handleCommandSuggestions` is one line:
            // `suggestionsProvider.completeCustomSuggestions(id, toSuggestions())`.
            // The id test is the whole of it — a reply to a superseded request
            // is dropped rather than repainting the popup with the answer to a
            // prefix already typed past.
            match crate::suggestion_wire::CommandSuggestionsReply::read(body) {
                Ok(reply) => {
                    if let Some(s) = self.suggestions.complete(&reply) {
                        self.suggestion_reply = Some(s);
                    } else {
                        log::debug!(
                            "net: command_suggestions id {} is not the outstanding request",
                            reply.id
                        );
                    }
                }
                Err(e) => log::warn!("net: command_suggestions decode failed: {e}"),
            }
        } else if id == ids.cb_play_custom_chat_completions {
            match crate::suggestion_wire::read_custom_chat_completions(body) {
                Ok((action, entries)) => self.suggestions.apply_completions(action, &entries),
                Err(e) => log::warn!("net: custom_chat_completions decode failed: {e}"),
            }
        } else if id == ids.cb_play_delete_chat {
            let mut r = PacketReader::new(body);
            match crate::chat_wire::read_delete_chat(&mut r) {
                // An unresolvable packed id is a no-op rather than an error:
                // vanilla's `unpack` would return null for an empty slot and
                // `deleteMessageOrDelay` then finds no message. Rewo also
                // reaches here for an out-of-range id, where vanilla throws —
                // see `chat_wire`'s module docs.
                Ok(packed) => match self.signature_cache.resolve(&packed) {
                    Some(sig) => self
                        .chat_events
                        .push(crate::chat_wire::ChatEvent::Delete(sig)),
                    None => log::debug!("net: delete_chat named an unknown signature"),
                },
                Err(e) => log::warn!("net: delete_chat decode failed: {e}"),
            }
        } else if id == ids.cb_play_disconnect {
            // M129 — resolved against the language table rather than
            // flattened. Every vanilla kick is a `Component.translatable`, so
            // this was the most translatable-dense component Rewo received and
            // the one that rendered as a raw key most often. The decode lives
            // in `rewo_world::disconnect_screen` so a test can reach it.
            let (reason, cause) =
                rewo_world::disconnect_screen::read_disconnect(body, self.lang.as_deref());
            self.disconnect = Some(reason);
            self.disconnect_cause = Some(cause);
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
            // M68: keep riding out of the walking meter. `ServerPlayer.startRiding`
            // *always* teleports the rider (`this.connection.teleport(...)`)
            // as part of seating it, so mounting would otherwise register as a
            // physics correction on every single mount — a false red that has
            // nothing to do with the walk simulation.
            if self.is_mounted() {
                self.motion_stats.corrections_while_mounted += 1;
            }
        }
        self.spawned = true;
        Ok(())
    }

    /// `handleRotatePlayer` (73) and `handleLookAt` (71) — M76.
    ///
    /// The two write the same two floats through the same setters and differ in
    /// exactly one observable way: `handleRotatePlayer` ends with
    /// `send(new ServerboundMovePlayerPacket.Rot(getYRot(), getXRot(), false,
    /// false))` and `handleLookAt` sends nothing, leaving the next tick's
    /// ordinary movement report to carry the new angles. That is why this is a
    /// dispatch arm rather than a `route_*` tail call: the seam owns the state
    /// change so a gate can drive it, and the socket half lives here.
    ///
    /// Unlike `handleMovePlayer`, **neither has a passenger guard**. The
    /// positional teleport skips its whole body for a rider (`if
    /// (!player.isPassenger())`); a rotational one applies while mounted.
    fn apply_player_rotation(&mut self, id: i32, body: &[u8]) -> Result<(), String> {
        // `Anchor::Eyes.apply(entity)` needs that entity's `getEyeHeight()`,
        // which is `EntityDimensions.eyeHeight` — a per-type field Rewo does
        // not model. Resolving only the `Feet` anchor is not a shortcut: an
        // unresolved target falls back to the packet's own coordinates, and
        // those are the server's snapshot of `toAnchor.apply(entity)` for this
        // very entity, so the eye case degrades to *staleness* rather than to a
        // wrong point. See `PlayerLookAt::position`.
        let entities = &self.world.entities;
        let resolve = |target: i32, anchor: crate::player_rotation::Anchor| match anchor {
            crate::player_rotation::Anchor::Feet => {
                entities.get(target).map(|e| [e.x, e.y, e.z])
            }
            crate::player_rotation::Anchor::Eyes => None,
        };
        let route = crate::player_rotation::route_player_rotation(
            id,
            body,
            &self.ids,
            crate::player_rotation::LocalRotation {
                pos: [self.player.x, self.player.y, self.player.z],
                eye_height: rewo_world::physics::EYE_HEIGHT,
                yaw: &mut self.player.yaw,
                pitch: &mut self.player.pitch,
            },
            resolve,
        );
        if route == crate::player_rotation::RotationRoute::Rotation {
            // Unconditional in vanilla — it is not gated on the rotation having
            // changed, and it does not wait for the tick.
            let mut p = PacketWriter::packet(self.ids.sb_play_move_rot);
            p.f32(self.player.yaw)
                .f32(self.player.pitch)
                // `Rot(yRot, xRot, onGround, horizontalCollision)` — the two
                // literal `false`s in `handleRotatePlayer`'s call, not the
                // player's live flags.
                .u8(0);
            self.send(p)?;
            // The report just went out, so the tick loop must not send a
            // second one for the same change.
            self.last_rot = (self.player.yaw, self.player.pitch);
        }
        Ok(())
    }

    /// The vehicle the local player is directly riding, if any (M68).
    pub fn local_vehicle(&self) -> Option<i32> {
        self.player_id.and_then(|id| self.mounts.vehicle_of(id))
    }

    /// Whether the local player is a passenger. While this is true the client
    /// does not run its own physics — see [`PlaySession::tick`].
    pub fn is_mounted(&self) -> bool {
        self.local_vehicle().is_some()
    }

    /// `handleExplosion`'s tail:
    /// `packet.playerKnockback().ifPresent(this.minecraft.player::addDeltaMovement)`.
    ///
    /// Every player within 64 blocks of the blast receives this packet
    /// (`ServerLevel`: `player.distanceToSqr(center) < 4096.0`), but the
    /// knockback is `Optional.ofNullable(explosion.getHitPlayers().get(player))`
    /// — per-recipient, and absent for anyone the explosion did not push. So
    /// "present" already means "this is *your* shove"; there is no target id
    /// to check.
    fn apply_explode(&mut self, e: &crate::motion::Explosion) {
        self.motion_stats.explosions += 1;
        let Some(k) = e.player_knockback else { return };
        self.motion_stats.explosion_knockbacks += 1;
        if k != crate::motion::Vec3::ZERO {
            self.motion_stats.explosion_knockbacks_nonzero += 1;
        }
        // `addDeltaMovement` — an ADD onto the existing velocity, and one that
        // silently drops a non-finite vector rather than storing it (rule 4).
        if !k.is_finite() {
            return;
        }
        // Bracket the mutation and measure it, rather than assuming the write
        // landed. See `MotionStats::knockback_velocity_delta`.
        let before = (self.player.vx, self.player.vy, self.player.vz);
        self.player.vx += k.x;
        self.player.vy += k.y;
        self.player.vz += k.z;
        let delta = (self.player.vx - before.0)
            .abs()
            .max((self.player.vy - before.1).abs())
            .max((self.player.vz - before.2).abs());
        if delta > self.motion_stats.knockback_velocity_delta {
            self.motion_stats.knockback_velocity_delta = delta;
        }
        log::debug!(
            "net: explode knockback ({:.4}, {:.4}, {:.4}) → v=({:.4}, {:.4}, {:.4})",
            k.x,
            k.y,
            k.z,
            self.player.vx,
            self.player.vy,
            self.player.vz
        );
    }

    /// `handleSetEntityMotion` → `entity.lerpMotion(packet.movement())`, which
    /// in 26.2 is a bare `setDeltaMovement` — a **replace**, not a blend, and
    /// not an add (rule 4).
    ///
    /// The local player's half of the sound world (M141d).
    ///
    /// One derivation with two callers — `run_headless` and `LiveApp::frame`,
    /// the two composition roots the audio plan §0.3 records as unwitnessed.
    /// Letting each build its own is how they come to disagree (M89, four
    /// times now).
    ///
    /// **`fall_flying` is the one field Rewo cannot answer yet**, and it is
    /// named here rather than defaulted quietly. The local player's
    /// `DATA_SHARED_FLAGS_ID` does arrive — the server sends you your own
    /// metadata — but `route_set_entity_data` writes into `EntityTable`, which
    /// holds no row for you (M73's asymmetry, hit again). So it answers
    /// `false`, and the cost is precise: `ElytraOnPlayerSoundInstance`'s guard
    /// is `time <= 20 || isFallFlying()`, so an elytra sound would play for
    /// exactly one second and stop. That is not a silence you would blame on
    /// this function, which is why it is written down. It is also the elytra's
    /// *trigger* (`onSyncedDataUpdated`'s rising edge), so one decode closes
    /// both ends and it belongs with the trigger milestone.
    pub fn local_player_view(&self) -> Option<crate::sound_engine::LocalPlayerView> {
        Some(crate::sound_engine::LocalPlayerView {
            id: self.player_id?,
            position: (self.player.x, self.player.y, self.player.z),
            velocity: (self.player.vx, self.player.vy, self.player.vz),
            fall_flying: self.local_player_data.is_fall_flying(),
        })
    }

    /// `LocalPlayer.onSyncedDataUpdated` (M141e) — the half
    /// `route_set_entity_data` cannot take.
    ///
    /// Runs on **every** `set_entity_data`, exactly as
    /// `capture_local_attributes` does: a body naming anything but the camera
    /// entity, or one that does not parse, changes nothing.
    ///
    /// The elytra sound is queued rather than played here, because the engine
    /// lives one layer up — and it goes in the same queue as everything else,
    /// so `stop_sound` can still silence it in order.
    fn capture_local_metadata(&mut self, body: &[u8]) {
        let out = crate::local_player_data::apply_local_metadata(
            body,
            self.player_id,
            self.swing_data.as_ref().map(|d| d.components),
            &mut self.local_player_data,
        );
        if out.start_elytra_sound {
            if let Some(player) = self.player_id {
                self.push_sound_event(crate::sounds::SoundEvent::Tickable(
                    crate::sounds::TickableSound::ElytraOnPlayer { player },
                ));
            }
        }
    }

    /// `ClientPacketListener.postAddEntitySoundInstance` — the ambient loop a
    /// spawning minecart or bee brings with it (M141f).
    ///
    /// ```java
    /// if (entity instanceof AbstractMinecart minecart) {
    ///    this.minecraft.getSoundManager().play(new MinecartSoundInstance(minecart));
    /// } else if (entity instanceof Bee bee) {
    ///    boolean angry = bee.isAngry();
    ///    BeeSoundInstance soundInstance = angry ? new BeeAggressiveSoundInstance(bee)
    ///                                           : new BeeFlyingSoundInstance(bee);
    ///    this.minecraft.getSoundManager().queueTickingSound(soundInstance);
    /// }
    /// ```
    ///
    /// **The two arms use different entry points, and that is vanilla's**: the
    /// minecart goes through `play` and the bee through `queueTickingSound`,
    /// which defers to the top of the next tick and re-checks `canPlaySound()`
    /// there. So a bee spawning silent never starts, while a minecart spawning
    /// silent is refused immediately — the same outcome by two routes, and a
    /// one-tick difference in when. Tidying them into one call is the obvious
    /// simplification and loses that.
    ///
    /// **The bee's loop is chosen once, here**, from its anger at spawn. After
    /// that the ramp switches on its own (`shouldSwitchSounds`), which is why
    /// the queued spec carries *which loop* rather than the bee's id alone.
    ///
    /// Only the entity the packet just added is considered, which is what
    /// `handleAddEntity` does — it calls this with the entity it built.
    fn post_add_entity_sound_instance(&mut self, id: i32, type_id: i32) {
        let is_minecart = self
            .entity_classes
            .as_deref()
            .is_some_and(|c| c.is_minecart(type_id));
        if is_minecart {
            self.push_sound_event(crate::sounds::SoundEvent::Tickable(
                crate::sounds::TickableSound::MinecartRiding { minecart: id },
            ));
        } else if Some(type_id) == self.bee_type_id {
            // `bee.isAngry()` at this moment — a synced deadline against the
            // world clock, not a flag.
            let aggressive = crate::tickable::is_angry(
                self.world.entities.anger_end_time(id).unwrap_or(-1),
                self.game_time(),
            );
            self.push_sound_event(crate::sounds::SoundEvent::Tickable(
                crate::sounds::TickableSound::BeeLoop {
                    bee: id,
                    aggressive,
                },
            ));
        }
    }

    /// `handleEntityEvent`'s two sound cases (M141g).
    ///
    /// ```java
    /// case 21: play(new GuardianAttackSoundInstance((Guardian)entity)); break;
    /// case 63: play(new SnifferSoundInstance((Sniffer)entity)); break;
    /// ```
    ///
    /// **Both are `play`, not `queueTickingSound`** — the deferral is the
    /// bee's alone. And both casts are unchecked in vanilla, which is only
    /// safe because the server never sends those ids to another type; here the
    /// kind is checked, so a mis-addressed event is inert rather than a sound
    /// on the wrong mob.
    ///
    /// The body is `ClientboundEntityEventPacket`'s: a **fixed big-endian i32**
    /// entity id and a signed byte event id, neither a VarInt (M17).
    fn entity_event_sound(&mut self, body: &[u8]) {
        let mut r = PacketReader::new(body);
        let (Ok(eid), Ok(event)) = (r.i32(), r.i8()) else {
            return;
        };
        // `packet.getEntity(this.level)` — `if (entity != null)`.
        let Some(type_id) = self.world.entities.get(eid).map(|e| e.type_id) else {
            return;
        };
        let is_guardian = Some(type_id) == self.guardian_type_id
            || Some(type_id) == self.elder_guardian_type_id;
        let spec = match event {
            21 if is_guardian => crate::sounds::TickableSound::GuardianAttack { guardian: eid },
            63 if Some(type_id) == self.sniffer_type_id => {
                crate::sounds::TickableSound::SnifferDigging { sniffer: eid }
            }
            _ => return,
        };
        self.push_sound_event(crate::sounds::SoundEvent::Tickable(spec));
    }

    /// The local player's own synced data (M141e).
    pub fn local_player_data(&self) -> &crate::local_player_data::LocalPlayerData {
        &self.local_player_data
    }

    /// **A remote entity's velocity is stored too, since M141d.** This used to
    /// keep only the local player's, with a comment reasoning that remote
    /// entities "are never integrated client-side, so there is nothing for
    /// their velocity to drive". The first half is right and the second stopped
    /// being true at M141: four of the ten tickable sound ramps read
    /// `getDeltaMovement()` — the bee's buzz, the minecart's rumble, the riding
    /// loops and the elytra — and none of them is a position.
    ///
    /// The velocity a remote entity carries is a decaying echo of these
    /// packets and nothing else; see
    /// [`rewo_world::entities::EntityState`]'s `delta_movement`.
    fn apply_set_entity_motion(&mut self, m: &crate::motion::EntityMotion) {
        self.motion_stats.entity_motions += 1;
        if Some(m.id) != self.player_id {
            // `handleSetEntityMotion` → `entity.lerpMotion(...)`. The body is
            // in `motion::apply_remote_motion` rather than here because this
            // method is unreachable from any test — see that function's doc.
            crate::motion::apply_remote_motion(
                &mut self.world.entities,
                self.entity_classes.as_deref(),
                m,
            );
            return;
        }
        self.motion_stats.local_motions += 1;
        if m.movement == crate::motion::Vec3::ZERO {
            self.motion_stats.local_motion_stops += 1;
        }
        if !m.movement.is_finite() {
            return;
        }
        self.player.vx = m.movement.x;
        self.player.vy = m.movement.y;
        self.player.vz = m.movement.z;
        log::debug!(
            "net: set_entity_motion (local) v=({:.4}, {:.4}, {:.4})",
            m.movement.x,
            m.movement.y,
            m.movement.z
        );
    }

    /// `handleSetEntityPassengersPacket` — eject, then seat the new list.
    ///
    /// The mount/dismount *transitions* are derived by comparing before and
    /// after rather than read off the packet, because the packet has no such
    /// field: a dismount is an ordinary rider list that no longer names you.
    fn apply_set_passengers(&mut self, p: &crate::motion::Passengers) {
        self.motion_stats.passenger_updates += 1;
        let was = self.is_mounted();
        self.mounts.apply(p);
        let now = self.is_mounted();
        match (was, now) {
            (false, true) => {
                self.motion_stats.local_mounts += 1;
                log::debug!("net: mounted vehicle {:?}", self.local_vehicle());
            }
            (true, false) => {
                self.motion_stats.local_dismounts += 1;
                // The pose belonged to a vehicle we are no longer on.
                self.vehicle_pose = None;
                log::debug!("net: dismounted");
            }
            _ => {}
        }
    }

    /// `handleMoveVehicle`, minus the serverbound echo.
    ///
    /// Vanilla ends this handler by sending `ServerboundMoveVehiclePacket`
    /// back — a *controlling* client asserting where it drove the vehicle.
    /// Rewo implements no vehicle physics and never sends the serverbound
    /// half, so it never provokes this packet either (both of vanilla's send
    /// sites are inside the serverbound handler). Storing the pose is
    /// therefore the whole of the client behaviour that is honest here.
    fn apply_move_vehicle(&mut self, v: &crate::motion::VehicleMove) {
        self.motion_stats.vehicle_moves += 1;
        self.vehicle_pose = Some(*v);
    }

    /// Player Info Update — apply what [`parse_player_info`] read.
    ///
    /// The walk itself lives in that free function so the tests drive exactly
    /// the bytes production does; this half is only the state it lands in.
    /// Each field writes only when its action bit was set, because the packet
    /// is a delta and an absent action means "unchanged".
    fn apply_player_info(&mut self, body: &[u8]) {
        let (entries, parse) = parse_player_info(body);
        if let Err(e) = parse {
            log::debug!("play: player_info_update parse: {e}");
        }
        for e in entries {
            if let Some(ms) = e.latency {
                self.latency.insert(e.uuid, ms);
            }
            if let Some(gm) = e.gamemode {
                self.gamemodes.insert(e.uuid, gm);
            }
            if let Some(order) = e.tab_list_order {
                self.tab_list_orders.insert(e.uuid, order);
            }
            if let Some(name) = e.name {
                self.world.entities.set_name(e.uuid, name);
            }
            // Decode the `textures` property → skin URL + model, so a player
            // renders with their real skin. Queue only genuinely-new skins so
            // the app fetches each once.
            if let Some(info) = e
                .textures
                .as_deref()
                .and_then(crate::skins::decode_textures_property)
            {
                if self.player_skins.get(&e.uuid) != Some(&info) {
                    self.player_skins.insert(e.uuid, info.clone());
                    self.pending_skins.push((e.uuid, info));
                }
            }
        }
    }

    /// Server-reported ping for a player, in milliseconds.
    ///
    /// `None` means the server has not sent an `UPDATE_LATENCY` for them yet,
    /// which is a real and common state right after join -- distinct from a
    /// reported zero.
    pub fn ping_ms(&self, uuid: u128) -> Option<i32> {
        self.latency.get(&uuid).copied()
    }

    /// The local player's ping.
    ///
    /// `None` until both halves are known: the server has to have told us our
    /// own UUID *and* sent a latency for it.
    pub fn own_ping_ms(&self) -> Option<i32> {
        self.own_uuid.and_then(|u| self.ping_ms(u))
    }

    /// Server-reported game mode for a player (M62).
    ///
    /// `None` means no `UPDATE_GAME_MODE` has arrived for them. Vanilla's
    /// `PlayerInfo` would read `SURVIVAL` there; keeping the distinction lets
    /// a caller decide, and makes "spectators sort last" answerable without
    /// inventing a mode for someone the server never described.
    pub fn game_mode(&self, uuid: u128) -> Option<GameMode> {
        self.gamemodes.get(&uuid).copied()
    }

    /// The local player's game mode.
    pub fn own_game_mode(&self) -> Option<GameMode> {
        self.own_uuid.and_then(|u| self.game_mode(u))
    }

    /// The local player's own attribute snapshots (M73).
    pub fn local_attributes(&self) -> &rewo_world::attributes::EntityAttributes {
        &self.local_attributes
    }

    /// Store an `update_attributes` body that names the local player.
    ///
    /// Deliberately a second decode of the same bytes rather than a hook
    /// inside `apply_update_attributes`: that function's entity lookup is
    /// `handleUpdateAttributes`'s own gate and every other caller — the
    /// `attributeshot` oracle included — depends on it staying exactly that.
    /// A body that does not parse, or that names anything but the camera
    /// entity, changes nothing.
    fn capture_local_attributes(&mut self, body: &[u8]) {
        crate::attributes::apply_local_attributes(
            body,
            self.player_id,
            &mut self.local_attributes,
        );
    }


    /// `PlayerInfo.getTabListOrder()` — the tab list's first sort key (M62).
    ///
    /// `None` means unsent, which vanilla treats as 0. Distinct from
    /// `Some(0)` for the same reason as `ping_ms`.
    pub fn tab_list_order(&self, uuid: u128) -> Option<i32> {
        self.tab_list_orders.get(&uuid).copied()
    }

    /// The team a player is on, by uuid (M62).
    ///
    /// **This is a two-step lookup and cannot be anything else.** The team
    /// packet keys its members by scoreboard name, and for a player that is
    /// the profile name, which only `player_info_update`'s `ADD_PLAYER`
    /// carries — so the answer is `name_of(uuid)` then `team_of_member`.
    /// Resolving lazily on every call (rather than binding a uuid to a team
    /// when either packet arrives) is deliberate and is what vanilla does in
    /// `PlayerInfo.getTeam()`: the two packets have no ordering guarantee, so
    /// a team formed before its members' profiles arrive still answers
    /// correctly the moment the profile does.
    ///
    /// `None` covers three different situations that the caller cannot tell
    /// apart, and none of them is an error: no profile name yet, no team, or
    /// a team whose membership names an entity that is not this player.
    pub fn team_of(&self, uuid: u128) -> Option<&str> {
        let name = self.world.entities.name_of(uuid)?;
        self.scoreboard.teams.team_of_member(name)
    }

    /// The team a scoreboard name is on — the direct form, for callers that
    /// already hold a name (and the only form that works for a non-player
    /// score holder, whose scoreboard name is not a profile name at all).
    pub fn team_of_name(&self, name: &str) -> Option<&str> {
        self.scoreboard.teams.team_of_member(name)
    }

    /// `Entity.getScoreboardName()` — the key `Scoreboard` files an entity's
    /// team membership under (M70).
    ///
    /// **Two different strings, chosen by type.** `Player` overrides it to the
    /// profile name; every other entity inherits `Entity`'s, which is
    /// `this.stringUUID` — the dashed lowercase form of the entity's UUID.
    /// Using one for the other is silent: `/team join red @e[type=zombie]`
    /// would simply never match.
    pub fn scoreboard_name_of(&self, id: i32) -> Option<String> {
        let e = self.world.entities.get(id)?;
        if self.player_type_id == Some(e.type_id) {
            self.world.entities.name_of(e.uuid).map(str::to_string)
        } else {
            Some(uuid_to_dashed(e.uuid))
        }
    }

    /// `Entity.getTeam()`, reduced to what the label predicate reads (M70).
    ///
    /// The mapping itself lives in [`crate::teams::label_team`] so a gate can
    /// drive it without a live session; this only resolves the scoreboard name
    /// to look it up by.
    pub fn label_team_of(&self, id: i32) -> Option<rewo_world::label::TeamView<'_>> {
        let member = self.scoreboard_name_of(id)?;
        crate::teams::label_team(&self.scoreboard.teams, &member)
    }

    /// The viewer's own team name — `minecraft.player.getTeam()` (M70).
    pub fn own_team(&self) -> Option<&str> {
        let uuid = self.own_uuid?;
        self.team_of(uuid)
    }

    /// `ClientboundLoginPacket`: establish the active dimension.
    ///
    /// Fallible on purpose. Everything downstream of this packet — the vertical
    /// shape every chunk is decoded against, the lighting contract, the biome
    /// layer's base sky/fog — is derived from bytes read here, so a body we
    /// cannot decode has exactly one safe outcome: fail, rather than leave the
    /// pre-login Overworld placeholder in place while the server starts sending
    /// chunks for something else. The caller propagates the error and the
    /// `Util.getNanos()` — a monotonic reading in nanoseconds, measured from
    /// this session's own epoch (M74).
    ///
    /// Only intervals are ever taken from it, so the epoch is arbitrary; the
    /// session-local one just keeps the numbers legible in a log. `as i64`
    /// saturates rather than wrapping, and a session would have to run for
    /// 292 years to reach that.
    fn now_nanos(&self) -> i64 {
        self.clock_epoch.elapsed().as_nanos() as i64
    }

    /// `player_loaded` reply is never sent.
    fn apply_login_shape(&mut self, body: &[u8]) -> Result<(), String> {
        let mut r = PacketReader::new(body);
        // The prefix ends on the first byte of the embedded
        // `CommonPlayerSpawnInfo` and yields the local player's entity id (only
        // effects targeting it drive the camera lightmap).
        let prefix = read_login_prefix(&mut r).map_err(|e| format!("play login: prefix: {e}"))?;
        let player_id = prefix.player_id;
        // The login packet is where the view area starts (M67) —
        // `handleLogin` assigns both distances *and* feeds the radius to
        // `options.setServerRenderDistance`, so a server that never resends
        // either is still fully described.
        self.view_area
            .apply_login(prefix.chunk_radius, prefix.simulation_distance);
        // M82. `handleLogin` seeds the level data from `packet.hardcore()` and
        // calls `player.setShowDeathScreen(packet.showDeathScreen())`. Both
        // bytes were read into a discard before — the same shape as M52c's
        // latency and M67's two view distances, and the third time this one
        // walk turned out to be already reading something nothing consumed.
        self.hardcore = prefix.hardcore;
        self.game_state
            .set_show_death_screen(prefix.show_death_screen);
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
        // M75. `handleLogin` ends with `setLocalMode(gameType, previousGameType)`.
        // These two fields have been decoded since M16 and read by nothing:
        // without them a client that joins in creative and never switches has no
        // idea it is in creative, because `game_event`'s `CHANGE_GAME_MODE` is
        // only the *mid-session* change. This is the join-time truth.
        apply_spawn_game_mode(&mut self.game_state, &mut self.abilities, &spawn);
        let active = apply_spawn_info(&mut self.world, &self.dim_types, &spawn);
        // M76. `handleLogin` builds the first `ClientLevel`, whose constructor
        // seeds the respawn data to `(8, 64, 8)` **of that level** — not to
        // `RespawnData.DEFAULT`, which no client ever holds. The server's
        // `set_default_spawn_position` normally overwrites it moments later;
        // this is what a client believes until it arrives.
        self.client_state.enter_level(&spawn.dimension);
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
        // M82: bumped before anything else can fail, because a respawn the
        // client half-applied still ends the death screen in vanilla — the
        // `setScreen(null)` is unconditional on everything but the screen's
        // own type.
        self.respawn_epoch = self.respawn_epoch.wrapping_add(1);
        let spawn = &info.spawn;
        // M75. `handleRespawn` calls the same two-argument `setLocalMode` that
        // `handleLogin` does — so a death or a dimension change re-announces
        // the mode, and re-derives the abilities from it. Applied after the
        // parse succeeds, alongside everything else this packet establishes.
        apply_spawn_game_mode(&mut self.game_state, &mut self.abilities, spawn);
        let changed = WorldTransition {
            world: &mut self.world,
            dirty: &mut self.dirty,
            removed: &mut self.removed,
            light: &mut self.light,
            day_ticks: &mut self.day_ticks,
            overworld_clock: &mut self.overworld_clock,
            game_time: &mut self.game_time,
            weather: &mut self.weather,
            border: &mut self.border,
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

        // M76. `handleRespawn` only builds a replacement `ClientLevelData` /
        // `ClientLevel` when the dimension actually changed, so this is the
        // dimension-changing path *only* — a death in place keeps the level
        // data and with it the world spawn. The difficulty sitting on the same
        // struct behaves the opposite way round: `handleRespawn` copies it
        // across explicitly, precisely because a fresh `ClientLevelData` would
        // otherwise lose it. See `client_state`.
        if changed {
            self.client_state.enter_level(&spawn.dimension);
        }

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

        // M68: every entity id belongs to the world that just went away, so a
        // retained seat would name an entity from the old dimension — and, if
        // the local player was riding, would freeze its physics in the new one.
        // Vanilla dismounts across a dimension change for the same reason
        // (`ServerPlayer.changeDimension` removes the vehicle).
        self.mounts.clear();
        self.vehicle_pose = None;

        // Same reason, same both-paths placement: the fresh `LocalPlayer` has
        // an empty `activeEffects` and a `tickCount` of 0. Neither is
        // `SynchedEntityData`, so `dataToKeep` bit 2 cannot carry them and the
        // server re-sends whatever effects still apply. The registry ids and the
        // (unchanged) local entity id are kept.
        self.visual_effects.reset_for_respawn();

        // M79, and it runs the opposite way to the fields above it. The
        // experience triple and the item-cooldown map are `LocalPlayer` /
        // `Player` state that the fresh player does not inherit — including
        // `experienceDisplayStartTick`'s `Integer.MIN_VALUE` sentinel, so the
        // `set_experience` the server sends right after a respawn again fails
        // to prioritise the XP bar. The *titles* are untouched here on
        // purpose: they live on `Minecraft.gui.hud`, which `handleRespawn`
        // never reaches, so a title survives a death and a Nether portal and
        // is cleared only by a disconnect.
        self.hud.reset_for_respawn();

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
    /// The menu the player is looking at: the open container, or their own.
    ///
    /// One accessor rather than the choice being made at each call site (M89).
    /// Every consumer — hover, the five click actions, the click packet's
    /// container id and state id — has to agree about which menu is on screen,
    /// and M87 shipped with them disagreeing: the render drew the container
    /// while every click operated on `inventory`, so clicking a chest's slot 5
    /// picked up the player's crafting grid.
    pub fn shown_menu(&self) -> &rewo_world::inventory::Inventory {
        self.menus
            .open()
            .map(|m| &m.menu)
            .unwrap_or(&self.inventory)
    }

    /// [`Self::shown_menu`], mutably.
    pub fn shown_menu_mut(&mut self) -> &mut rewo_world::inventory::Inventory {
        match self.menus.open_mut() {
            Some(m) => &mut m.menu,
            None => &mut self.inventory,
        }
    }

    /// The container id the shown menu is addressed by — 0 for the player's.
    pub fn shown_container_id(&self) -> i32 {
        self.menus
            .open()
            .map_or(rewo_world::inventory::PLAYER_CONTAINER_ID, |m| {
                m.container_id
            })
    }

    /// `CrafterScreen.slotClicked`'s toggle half (M93i).
    ///
    /// Call this **before** the ordinary click and then send the click anyway:
    /// vanilla's override ends in an unconditional `super.slotClicked(...)`,
    /// so the toggle is additive. One method rather than a check at each click
    /// site, because vanilla has exactly one `slotClicked` override and M89's
    /// lesson was that a per-call-site choice is how paths come to disagree.
    ///
    /// Returns what it did, for the caller to log. A menu that is not a
    /// crafter, or a slot outside its grid, is [`CrafterToggle::None`] — the
    /// same answer as a click that legitimately toggles nothing, because
    /// neither does anything.
    pub fn crafter_slot_click(
        &mut self,
        slot: i32,
        button: i8,
        input: i32,
    ) -> rewo_world::menu::CrafterToggle {
        use rewo_world::menu::CrafterToggle;
        let none = CrafterToggle::None;
        let Some(open) = self.menus.open() else {
            return none;
        };
        if !rewo_world::menu::is_crafter_grid_slot(open.layout.protocol_id, slot) {
            return none;
        }
        let disabled = open.crafter_slot_disabled(slot);
        let slot_occupied = open.menu.menu_slot(slot as usize).is_some();
        // `player.getInventory().getItem(buttonNum)` — read off the PLAYER's
        // inventory, not the open menu, so the mapping is the player menu's
        // and the same one `click_swap` uses. An out-of-range button names no
        // stack: vanilla's `Inventory.getItem` answers EMPTY rather than
        // throwing, so it reads as empty and enables nothing.
        let swap_target_empty = rewo_world::inventory::swap_button_menu_slot(button as i32)
            .and_then(|i| self.inventory.menu_slot(i))
            .is_none();
        let toggle = rewo_world::menu::crafter_toggle(
            input,
            disabled,
            slot_occupied,
            self.game_state.game_mode().is_some_and(|m| m.is_spectator()),
            self.inventory.carried().is_none(),
            swap_target_empty,
        );
        let enabled = match toggle {
            CrafterToggle::Enable => true,
            CrafterToggle::Disable => false,
            CrafterToggle::None => return none,
        };
        // Send first, apply second. Vanilla is the other way round
        // (`setSlotState` then the packet), and the two differ only when the
        // send fails — where this order leaves the local view behind the
        // server's rather than ahead of it, which is the direction the rest of
        // the click path already chose.
        if self.container_slot_state_changed(slot, enabled).is_err() {
            return none;
        }
        if let Some(open) = self.menus.open_mut() {
            open.set_crafter_slot_state(slot, enabled);
        }
        toggle
    }

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
        // M89 — the SHOWN menu's id and state id, not the player's. Hard-coding
        // container 0 told the server every chest click was a click on the
        // player's own inventory, and paired it with the player's state id, so
        // the server would either apply it to the wrong menu or reject it on a
        // stale id. `stateId` is per-menu: `AbstractContainerMenu.incrementStateId`
        // is an instance counter, and the resync test is
        // `packet.stateId() != menu.getStateId()` against the menu the click
        // names.
        p.varint(self.shown_container_id());
        p.varint(self.shown_menu().state_id());
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

    /// `ServerboundContainerButtonClickPacket` (M92f) — two var-ints, the open
    /// menu's id and a button index.
    ///
    /// ```java
    /// StreamCodec.composite(ByteBufCodecs.CONTAINER_ID, ..., ByteBufCodecs.VAR_INT, ...)
    /// ```
    ///
    /// The **whole** bespoke-widget input surface for four screens: an
    /// enchanting row, a loom pattern, a stonecutter recipe and a crafter slot
    /// toggle are all this one packet with a different index. Only the beacon
    /// and the anvil need something else (`set_beacon` and `rename_item`), and
    /// the merchant its own trade-select.
    ///
    /// **The button index is a per-menu meaning, not a shared enum** — 0..=2
    /// is an enchanting offer, 0..=n a loom pattern, and 0..=8 a crafter slot.
    /// Sending the wrong screen's index is accepted by the server and does
    /// something, which is why the caller resolves it from the open menu's own
    /// layout rather than from a screen-independent id.
    ///
    /// Vanilla gates the send on `menu.clickMenuButton(player, i)` returning
    /// true — the client asks its *own* menu whether the press is legal before
    /// telling the server. Rewo has no server-side menu to ask, so the gate is
    /// the screen's own enabled/disabled state, which is the same predicate
    /// the button is drawn with.
    ///
    /// It carries **no state id**: unlike `container_click` this is not a
    /// prediction the server grades, so there is nothing to resync against.
    pub fn container_button_click(&mut self, button: i32) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_container_button_click else {
            return Err("container_button_click unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        // The SHOWN menu's id, on M89's rule — a button press belongs to
        // whatever screen is up, and container 0 is never a button screen.
        // The body comes from the tested builder rather than being written
        // twice, so the witness grades the bytes this actually sends.
        p.buf
            .extend_from_slice(&crate::container_button_click_body(
                self.shown_container_id(),
                button,
            ));
        self.send(p)
    }

    /// `RecipeBookComponent.sendUpdateSettings` (M98).
    ///
    /// **Reads both flags out of the local settings rather than taking them**,
    /// which is what vanilla does — so the caller flips the one it means and
    /// this reports the pair. Sending only the changed field would leave the
    /// server's copy of the other stale, and the server persists these across
    /// sessions.
    ///
    /// `book_type` is `RecipeBookType`'s ordinal, the same positional order
    /// `RecipeBookSettings` uses inbound (M93y).
    pub fn recipe_book_change_settings(&mut self, book_type: usize) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_recipe_book_change_settings else {
            return Err("recipe_book_change_settings unavailable".into());
        };
        let st = match book_type {
            0 => self.recipe_book_settings.crafting,
            1 => self.recipe_book_settings.furnace,
            2 => self.recipe_book_settings.blast_furnace,
            3 => self.recipe_book_settings.smoker,
            _ => return Err("recipe_book_change_settings: no such book".into()),
        };
        let mut p = PacketWriter::packet(id);
        p.buf
            .extend_from_slice(&crate::recipe_book_change_settings_body(
                book_type as i32,
                st.open,
                st.filtering,
            ));
        self.send(p)
    }

    /// `RecipeBookComponent.setVisible` — write a book's open flag locally
    /// (M107).
    ///
    /// Separate from the send, unlike [`Self::toggle_recipe_book_filter`],
    /// because the caller has other local state to settle in between (the
    /// which-of-these overlay goes too) and because vanilla's `setVisible` is
    /// itself several statements before `sendUpdateSettings()`.
    pub fn set_recipe_book_open(&mut self, book_type: usize, open: bool) {
        let st = match book_type {
            0 => &mut self.recipe_book_settings.crafting,
            1 => &mut self.recipe_book_settings.furnace,
            2 => &mut self.recipe_book_settings.blast_furnace,
            3 => &mut self.recipe_book_settings.smoker,
            _ => return,
        };
        st.open = open;
    }

    /// Flip a book's filter locally, then tell the server (M98).
    ///
    /// The order is vanilla's: `toggleFiltering()` writes the local setting and
    /// `sendUpdateSettings()` reads it back out. Reporting the value before
    /// writing it would tell the server the state it already had.
    pub fn toggle_recipe_book_filter(&mut self, book_type: usize) -> Result<(), String> {
        let st = match book_type {
            0 => &mut self.recipe_book_settings.crafting,
            1 => &mut self.recipe_book_settings.furnace,
            2 => &mut self.recipe_book_settings.blast_furnace,
            3 => &mut self.recipe_book_settings.smoker,
            _ => return Err("toggle_recipe_book_filter: no such book".into()),
        };
        st.filtering = !st.filtering;
        self.recipe_book_change_settings(book_type)
    }

    /// `ServerboundPlaceRecipePacket` (M98) — click a recipe in the book.
    ///
    /// `use_max_items` is shift-held. The container is the SHOWN menu's, on
    /// M89's rule: a book click belongs to whatever screen is up.
    pub fn place_recipe(&mut self, recipe: i32, use_max_items: bool) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_place_recipe else {
            return Err("place_recipe unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.buf.extend_from_slice(&crate::place_recipe_body(
            self.shown_container_id(),
            recipe,
            use_max_items,
        ));
        self.send(p)
    }

    /// `MultiPlayerGameMode.handleSlotStateChanged` (M93h) — a crafter toggle.
    ///
    /// Like `container_button_click` this carries **no state id**: it is not a
    /// prediction the server grades. Unlike it, the slot precedes the
    /// container in the body — see [`crate::container_slot_state_changed_body`].
    ///
    /// `CrafterScreen` sends this **in addition to** the ordinary click, not
    /// instead of it, so the caller must still send whatever the click itself
    /// resolved to.
    pub fn container_slot_state_changed(
        &mut self,
        slot_id: i32,
        enabled: bool,
    ) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_container_slot_state_changed else {
            return Err("container_slot_state_changed unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.buf
            .extend_from_slice(&crate::container_slot_state_changed_body(
                slot_id,
                // M89's rule: the SHOWN menu's id. A crafter toggle belongs to
                // whatever screen is up, and container 0 is never a crafter.
                self.shown_container_id(),
                enabled,
            ));
        self.send(p)
    }

    /// `AnvilScreen.onNameChanged` (M93n) — `rename_item`.
    ///
    /// Sent on **every accepted keystroke**, not on a confirm: the anvil has
    /// no confirm, and the server recomputes the result stack as you type,
    /// which is what makes the cost label move while you edit.
    pub fn rename_item(&mut self, name: &str) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_rename_item else {
            return Err("rename_item unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.buf.extend_from_slice(&crate::rename_item_body(name));
        self.send(p)
    }

    /// The recipe book's four clientbound packets (M93y).
    ///
    /// A malformed body is dropped whole and logged, never applied in part:
    /// `SlotDisplay`'s variants have different lengths, so a partial read has
    /// already lost its place and the rest of the list is garbage.
    /// `pub(crate)`-in-spirit but public: `--render-check` drives it directly
    /// to open the book, which no server does unprompted (M94). Injection into
    /// the production apply, not a shortcut past it — M17's rule.
    /// Inject one clientbound-play body through the production dispatcher.
    ///
    /// `--render-check` drives it to put enough chat on screen for a scrollbar
    /// to exist at all (M111): the bar's guard is
    /// `virtualHeight != chatHeight`, so it does not appear until the backlog
    /// exceeds the focused box's twenty rows, and a run's own join messages
    /// come to about six. Injection into the production apply, not a shortcut
    /// past it — M17's rule, and the same door `apply_recipe_book` opens.
    pub fn inject_packet(&mut self, id: i32, body: &[u8]) {
        if let Err(e) = self.handle_packet(id, body) {
            log::warn!("net: injected packet {id} failed: {e}");
        }
    }

    pub fn apply_recipe_book(&mut self, id: i32, body: &[u8]) {
        let ids = &self.ids;
        if id == ids.cb_play_recipe_book_settings {
            match crate::recipe_book::parse_settings(body) {
                Ok(s) => self.recipe_book_settings = s,
                Err(e) => log::warn!("net: {e}"),
            }
            return;
        }
        if id == ids.cb_play_recipe_book_remove {
            match crate::recipe_book::parse_remove(body) {
                Ok(v) => {
                    for r in v {
                        self.recipe_book.remove(&r);
                    }
                }
                Err(e) => log::warn!("net: {e}"),
            }
            return;
        }
        // The remaining two need the display registries, which are BUILT-IN
        // and so come from the report rather than the wire (M92's rule).
        let Some(display_ids) = self.recipe_display_ids.as_ref() else {
            log::warn!("net: recipe book packet with no display registries");
            return;
        };
        if id == ids.cb_play_recipe_book_add {
            match crate::recipe_book::parse_add(body, display_ids) {
                Ok(a) => {
                    // `replace` CLEARS the book first — a join sends true, an
                    // unlock sends false. Appending unconditionally leaves a
                    // stale book across a respawn.
                    if a.replace {
                        self.recipe_book.clear();
                    }
                    for e in a.entries {
                        self.recipe_book.insert(e.id, e);
                    }
                }
                Err(e) => log::warn!("net: {e}"),
            }
        } else {
            match crate::recipe_book::parse_place_ghost(body, display_ids) {
                Ok(g) => self.ghost_recipe = Some(g),
                Err(e) => log::warn!("net: {e}"),
            }
        }
    }

    /// `MerchantScreen.TradeOfferButton.onPress` — `select_trade` (M93u).
    ///
    /// One var-int, the offer's index in the list the server sent. Vanilla
    /// also calls `menu.setSelectionHint(index)` and `menu.tryMoveItems(index)`
    /// **locally first**, so the trade's items appear in the slots before the
    /// server answers; the packet is what makes it real.
    pub fn select_trade(&mut self, index: i32) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_select_trade else {
            return Err("select_trade unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.varint(index);
        self.send(p)
    }

    /// `BeaconConfirmButton.onPress` (M93l) — `set_beacon`, then close.
    ///
    /// The close is the CALLER's: vanilla sends the packet and then calls
    /// `player.closeContainer()`, and Rewo's close path is its own method, so
    /// keeping them separate here means a send failure does not leave the
    /// screen shut over a beacon the server never heard about.
    pub fn set_beacon(
        &mut self,
        primary: Option<i32>,
        secondary: Option<i32>,
    ) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_set_beacon else {
            return Err("set_beacon unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.buf
            .extend_from_slice(&crate::set_beacon_body(primary, secondary));
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

    /// `LocalPlayer.respawn()` (M82) —
    /// `ServerboundClientCommandPacket(PERFORM_RESPAWN)`, action **0**.
    ///
    /// ```java
    /// public void respawn() {
    ///    this.connection.send(new ServerboundClientCommandPacket(Action.PERFORM_RESPAWN));
    ///    KeyMapping.resetToggleKeys();
    /// }
    /// ```
    ///
    /// The `resetToggleKeys()` half is real and belongs to the app: a player
    /// who died sprinting must not respawn still sprinting. Rewo's toggle
    /// state lives in `live_cmd`'s `Keys`, so the caller clears it.
    ///
    /// The enum's other values are `REQUEST_STATS` and
    /// `REQUEST_GAMERULE_VALUES` — so the action is a VarInt `0`, and getting
    /// it wrong asks the server for the statistics screen instead of a
    /// respawn. The body is built by [`crate::client_command_body`] so a gate
    /// can grade the ordinal without a socket.
    pub fn perform_respawn(&mut self) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_client_command else {
            return Err("client_command unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.buf.extend_from_slice(&crate::client_command_body(
            crate::ClientCommand::PerformRespawn,
        ));
        self.send(p)
    }

    /// `StatsScreen.init()`'s last line —
    /// `send(new ServerboundClientCommandPacket(REQUEST_STATS))` (M84).
    ///
    /// The screen asks; the server answers with `award_stats`. **Vanilla sends
    /// this from `init()`, so it is re-sent on every window resize**, because
    /// `init()` is what `repositionElements` runs. Rewo sends it only when the
    /// screen opens, which is a deliberate deviation: a resize costs a round
    /// trip in vanilla and buys nothing the client does not already hold.
    pub fn request_stats(&mut self) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_client_command else {
            return Err("client_command unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.buf.extend_from_slice(&crate::client_command_body(
            crate::ClientCommand::RequestStats,
        ));
        self.send(p)
    }

    /// Drain the pending death, if any (M82).
    ///
    /// Draining rather than peeking, because the consumer is "open a screen" —
    /// an idempotent read would re-open it on every frame and reset its
    /// anti-misclick clock forever.
    /// Apply this frame's chat events to the store.
    ///
    /// Split from the decode because the store needs two things `rewo-net`
    /// cannot see — the font to wrap against and the GUI tick to stamp with —
    /// and both belong to the app. `gui_tick` is `Gui.getGuiTicks()`, which
    /// runs at the same 20 Hz as the session tick and is what
    /// `GuiMessage.addedTime` records.
    ///
    /// `ChatComponent.tick` runs **after** the events, so a deletion queued
    /// this frame is not also retried this frame — `processMessageDeletionQueue`
    /// is a separate `tick()` in vanilla for the same reason.
    pub fn apply_chat_events(
        &mut self,
        gui_tick: i32,
        ctx: &rewo_world::chat::WrapContext<'_>,
    ) {
        let events = std::mem::take(&mut self.chat_events);
        if let Some(overlay) =
            crate::chat_wire::apply_chat_events(&mut self.chat, events, gui_tick, ctx)
        {
            self.chat_overlay = Some(overlay);
        }
    }

    /// Drain the chat events decoded since the last call.
    ///
    /// A drain rather than a snapshot because the consumer *applies* them —
    /// a message is added to the store, an overlay replaces the action bar,
    /// a deletion mutates a message already in it — and replaying any of
    /// those would duplicate or re-delete.
    pub fn take_chat_events(&mut self) -> Vec<crate::chat_wire::ChatEvent> {
        std::mem::take(&mut self.chat_events)
    }

    /// The signature cache, for witnesses. Fed by every `player_chat`.
    pub fn signature_cache(&self) -> &crate::chat_wire::MessageSignatureCache {
        &self.signature_cache
    }

    /// [`crate::chat_translate::chat_component_text`] against this session's
    /// table — a one-line adapter, because the rule itself has to live
    /// somewhere a test can reach it (M71: this type owns a socket and has no
    /// test module anywhere in the repo).
    fn chat_component_text(&self, tag: &rewo_proto::nbt::Nbt) -> String {
        crate::chat_translate::chat_component_text(tag, self.lang.as_deref())
    }

    /// The protocol id of a named `minecraft:chat_type` entry.
    ///
    /// For witnesses. The registry is the SERVER's (M42's rule), so a gate
    /// that wants a specific decoration has to ask rather than assume an
    /// index — which is the point: the lookup is then part of what the witness
    /// grades, and an empty registry answers `None` instead of index 0.
    pub fn chat_type_id(&self, name: &str) -> Option<i32> {
        self.chat_types
            .iter()
            .position(|d| d.id == name)
            .and_then(|i| i32::try_from(i).ok())
    }

    /// `ChatType.Bound.decorate(content)` — the component vanilla renders.
    ///
    /// Falls back to the content unchanged when the bound names a chat type
    /// this session has no decoration for: a server that syncs no
    /// `minecraft:chat_type` registry, an id past its end, or an entry whose
    /// `chat` decoration could not be read. That is exactly what Rewo did
    /// before M127, and a strictly smaller lie than inventing
    /// `DEFAULT_CHAT_DECORATION` — which would render every such line as
    /// `<name> text` whatever the server asked for.
    ///
    /// Vanilla has no fallback at all: `byIdOrThrow` disconnects. Rewo declines
    /// to turn a chat line into a dropped connection.
    fn decorate_chat(
        &self,
        content: &rewo_proto::nbt::Nbt,
        bound: &crate::session::ChatTypeBound,
    ) -> rewo_proto::nbt::Nbt {
        let decoration = match &bound.chat_type {
            crate::session::ChatTypeRef::Inline(ty) => ty.chat.as_ref(),
            crate::session::ChatTypeRef::Registry(id) => usize::try_from(*id)
                .ok()
                .and_then(|i| self.chat_types.get(i))
                .and_then(|def| def.ty.chat.as_ref()),
        };
        match decoration {
            Some(d) => d.decorate(content, bound),
            None => content.clone(),
        }
    }

    /// The same walk, keeping the spans (M126b).
    ///
    /// `chat_component_text` is this followed by `plain_text`, and the two are
    /// deliberately both here: the chat HUD wants the spans, while the log line
    /// and `--render-check`'s counters want the characters.
    fn chat_component_spans(
        &self,
        tag: &rewo_proto::nbt::Nbt,
    ) -> rewo_world::chat_style::ChatLine {
        rewo_world::chat_style::parse_component(
            tag,
            rewo_world::chat_style::ChatStyle::WHITE,
            self.lang.as_deref(),
        )
    }

    pub fn take_death(&mut self) -> Option<crate::CombatKill> {
        self.death.take()
    }

    /// `LocalPlayer.shouldShowDeathScreen()` — the login flag, as amended by
    /// `game_event` id 11 (M82).
    pub fn show_death_screen(&self) -> bool {
        self.game_state.show_death_screen()
    }

    /// How many respawns have been applied (M82). See [`Self::respawn_epoch`]'s
    /// field docs for why this is a watermark.
    pub fn respawn_epoch(&self) -> u64 {
        self.respawn_epoch
    }

    /// Start digging (creative servers break the block on START).
    /// `ServerboundCommandSuggestionPacket` — ask what completes `command`.
    ///
    /// The id and the pending slot are the provider's; see
    /// [`crate::suggestion_wire`] for why there is only one outstanding
    /// request and what happens to a reply that misses it.
    pub fn request_command_suggestions(&mut self, command: &str) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_command_suggestion else {
            return Err("command_suggestion unavailable".into());
        };
        let (_req, body) = self.suggestions.begin_request(command);
        let mut p = PacketWriter::packet(id);
        p.raw(&body);
        self.send(p)
    }

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
        border: rewo_world::border::WorldBorder,
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
            // Same argument as the storm: a small off-centre border, so a
            // transition that failed to reset it would not hide behind the
            // default's own numbers.
            border: {
                let mut b = rewo_world::border::WorldBorder::default();
                b.set_center(120.0, -64.0);
                b.set_size(500.0);
                b.set_warning_blocks(11);
                b
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
                border: &mut self.border,
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
        assert_eq!(s.border.size(), 500.0, "and the same border still stands");
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

    /// The border is `ClientLevel` state on the same argument (M80). The
    /// harness starts at a 500-block border centred on (120, -64) so a
    /// transition that carried it through would be visible in all three of
    /// size, centre and warning distance.
    #[test]
    fn a_dimension_change_clears_the_world_border() {
        let defs = registry();
        let mut s = overworld_session(&defs, 4);
        assert_eq!(s.border.size(), 500.0, "precondition");
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 7)));
        assert_eq!(s.border.size(), rewo_world::border::MAX_SIZE);
        assert_eq!(s.border.center_x(), 0.0);
        assert_eq!(s.border.center_z(), 0.0);
        assert_eq!(s.border.warning_blocks(), 5);
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

#[cfg(test)]
mod ping_tests {
    //! M52c — the ping the client can actually know.
    //!
    //! These build the `player_info_update` body by hand and run it through
    //! the production `apply_player_info`, so the action bitmask, the entry
    //! walk and the latency slot are all exercised together. A local
    //! reimplementation would pass while the real decoder desynced.

    use super::*;

    /// Encode a var-int the way the wire does.
    fn varint(out: &mut Vec<u8>, mut v: i32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v = ((v as u32) >> 7) as i32;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    /// A body carrying UPDATE_LATENCY (action bit 4) for one uuid.
    fn latency_body(entries: &[(u128, i32)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(1u8 << 4); // only UPDATE_LATENCY
        varint(&mut b, entries.len() as i32);
        for (uuid, ms) in entries {
            b.extend_from_slice(&uuid.to_be_bytes());
            varint(&mut b, *ms);
        }
        b
    }

    #[test]
    fn update_latency_is_parsed_rather_than_discarded() {
        assert_eq!(parse_player_info_latency(&latency_body(&[(7, 42)])), [(7, 42)]);
    }

    #[test]
    fn several_players_are_walked_independently() {
        // The walk advances uuid-then-latency per entry; a mis-sized skip
        // corrupts every entry after it rather than failing.
        assert_eq!(
            parse_player_info_latency(&latency_body(&[(1, 10), (2, 250), (3, 0)])),
            [(1, 10), (2, 250), (3, 0)],
            "a reported zero is a value, not unknown"
        );
    }

    #[test]
    fn a_negative_latency_is_a_state_not_a_decode_error() {
        // PlayerTabOverlay buckets latency < 0 into the no-connection icon, so
        // the wire really does carry negatives; clamping at decode would erase
        // a state vanilla renders.
        assert_eq!(parse_player_info_latency(&latency_body(&[(9, -1)])), [(9, -1)]);
    }

    #[test]
    fn an_unset_latency_action_yields_nothing() {
        // Sensitivity partner: a mask without bit 4 must not invent an entry.
        // Reading the field unconditionally would fabricate a ping AND desync
        // the walk.
        let mut b = Vec::new();
        b.push(1u8 << 3);
        varint(&mut b, 1);
        b.extend_from_slice(&7u128.to_be_bytes());
        b.push(1);
        assert!(parse_player_info_latency(&b).is_empty());
    }

    #[test]
    fn an_action_before_latency_must_be_walked_first() {
        // LISTED (3) then LATENCY (4). Skipping the bool makes the walk read
        // it AS the varint and report 1ms -- a plausible number, which is
        // what makes it dangerous.
        let mut b = Vec::new();
        b.push((1u8 << 3) | (1u8 << 4));
        varint(&mut b, 1);
        b.extend_from_slice(&7u128.to_be_bytes());
        b.push(1);
        varint(&mut b, 200);
        assert_eq!(parse_player_info_latency(&b), [(7, 200)]);
    }
}

#[cfg(test)]
mod player_info_field_tests {
    //! M62 — the two `player_info_update` fields the tab list's first two
    //! sort keys come from: `UPDATE_GAME_MODE` (action 2) and
    //! `UPDATE_LIST_ORDER` (action 6). Both were read into a discard.
    //!
    //! Every body is built by hand and run through the production
    //! `parse_player_info`, so the bitmask and the entry walk are what is
    //! under test.

    use super::*;

    fn varint(out: &mut Vec<u8>, mut v: i32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v = ((v as u32) >> 7) as i32;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    /// A one-entry body carrying exactly the actions in `mask`, with each
    /// set action's payload appended by `fields` in bit order.
    fn one_entry(mask: u8, uuid: u128, fields: &[u8]) -> Vec<u8> {
        let mut b = vec![mask];
        varint(&mut b, 1);
        b.extend_from_slice(&uuid.to_be_bytes());
        b.extend_from_slice(fields);
        b
    }

    #[test]
    fn the_game_mode_action_is_kept_rather_than_discarded() {
        let (e, res) = parse_player_info(&one_entry(1 << 2, 7, &[3]));
        assert!(res.is_ok());
        assert_eq!(e[0].gamemode, Some(GameMode::Spectator));
        assert!(e[0].gamemode.unwrap().is_spectator());
    }

    #[test]
    fn the_tab_list_order_action_is_kept_rather_than_discarded() {
        let mut f = Vec::new();
        varint(&mut f, 42);
        let (e, res) = parse_player_info(&one_entry(1 << 6, 7, &f));
        assert!(res.is_ok());
        assert_eq!(e[0].tab_list_order, Some(42));
    }

    #[test]
    fn an_out_of_range_game_mode_id_is_survival_rather_than_an_error() {
        // `GameType.byId` is ByIdMap.continuous(..., ZERO), so 9 -> values[0].
        // An error here would drop a packet vanilla renders fine.
        let (e, res) = parse_player_info(&one_entry(1 << 2, 7, &[9]));
        assert!(res.is_ok());
        assert_eq!(e[0].gamemode, Some(GameMode::Survival));
    }

    #[test]
    fn an_unset_action_leaves_the_field_absent_rather_than_defaulted() {
        // The sensitivity partner for both. The packet is a DELTA: filling in
        // `Survival` / `0` here would tell the tab list a spectator had
        // switched to survival on every latency-only update, and the sort
        // would visibly reshuffle.
        let mut f = Vec::new();
        varint(&mut f, 55);
        let (e, _) = parse_player_info(&one_entry(1 << 4, 7, &f));
        assert_eq!(e[0].latency, Some(55));
        assert_eq!(e[0].gamemode, None);
        assert_eq!(e[0].tab_list_order, None);
    }

    #[test]
    fn a_mis_sized_earlier_action_would_report_a_plausible_wrong_order() {
        // GAME_MODE (2) then LIST_ORDER (6), with a two-byte var-int mode so
        // a one-byte skip is observable. Read correctly the order is 7; a
        // walk that assumed a single byte reads the mode's continuation byte
        // as the order and reports 1 -- a number nothing downstream can
        // reject.
        let mut f = Vec::new();
        varint(&mut f, 129); // two bytes: 0x81 0x01 -> mode id 129, ZERO -> Survival
        varint(&mut f, 7);
        let body = one_entry((1 << 2) | (1 << 6), 7, &f);
        let (e, res) = parse_player_info(&body);
        assert!(res.is_ok());
        assert_eq!(e[0].gamemode, Some(GameMode::Survival));
        assert_eq!(e[0].tab_list_order, Some(7));

        // The mis-sized walk, run over the same bytes.
        let mut r = PacketReader::new(&body);
        let _ = r.u8().unwrap();
        let _ = r.count("player info entries", 16).unwrap();
        let _ = r.uuid().unwrap();
        let _ = r.u8().unwrap(); // one byte where the mode is two
        assert_eq!(
            r.varint().unwrap(),
            1,
            "the mis-sized walk must report a plausible wrong order, not fail"
        );
    }

    #[test]
    fn several_entries_carry_their_own_values() {
        // Two entries under one mask, which is the shape a real join sends.
        // A walk that lost a byte in the first entry would attribute the
        // second's fields to the wrong uuid.
        let mut b = vec![(1u8 << 2) | (1u8 << 6)];
        varint(&mut b, 2);
        b.extend_from_slice(&1u128.to_be_bytes());
        b.push(3); // spectator
        varint(&mut b, 10);
        b.extend_from_slice(&2u128.to_be_bytes());
        b.push(1); // creative
        varint(&mut b, 20);

        let (e, res) = parse_player_info(&b);
        assert!(res.is_ok());
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].uuid, e[0].gamemode, e[0].tab_list_order), (1, Some(GameMode::Spectator), Some(10)));
        assert_eq!((e[1].uuid, e[1].gamemode, e[1].tab_list_order), (2, Some(GameMode::Creative), Some(20)));
    }

    #[test]
    fn a_truncated_entry_keeps_the_fields_it_completed() {
        // The body promises a mode and an order and stops after the mode.
        // The completed field must survive, because that is what the
        // pre-M62 field-at-a-time decoder did and losing it would silently
        // discard a whole packet's worth of state on one short read.
        let mut b = vec![(1u8 << 2) | (1u8 << 6)];
        varint(&mut b, 1);
        b.extend_from_slice(&7u128.to_be_bytes());
        b.push(3);
        let (e, res) = parse_player_info(&b);
        assert!(res.is_err());
        assert_eq!(e[0].gamemode, Some(GameMode::Spectator));
        assert_eq!(e[0].tab_list_order, None);
    }
}
