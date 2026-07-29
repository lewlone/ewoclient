//! Packet ids resolved by name at connect time (REWO_PLAN.md §11). A missing
//! *required* packet fails loud here rather than mid-stream; optional ones
//! (cookies, start_configuration) resolve to `None` and their handlers no-op.

use rewo_data::packets::{Dir, Packets, State};

macro_rules! req {
    ($p:expr, $state:expr, $dir:expr, $name:literal) => {
        $p.require($state, $dir, $name)?
    };
}
macro_rules! opt {
    ($p:expr, $state:expr, $dir:expr, $name:literal) => {
        $p.id($state, $dir, $name)
    };
}

pub struct Ids {
    // handshake / login
    pub sb_handshake_intention: i32,
    pub sb_login_hello: i32,
    pub sb_login_key: i32,
    pub sb_login_acknowledged: i32,
    pub cb_login_compression: i32,
    pub cb_login_finished: i32,
    pub cb_login_disconnect: i32,
    pub cb_login_hello: i32,
    // configuration
    pub sb_config_custom_payload: i32,
    pub sb_config_client_information: i32,
    pub sb_config_keep_alive: i32,
    pub sb_config_pong: i32,
    pub sb_config_select_known_packs: i32,
    pub sb_config_finish: i32,
    pub sb_config_cookie_response: i32,
    pub cb_config_keep_alive: i32,
    pub cb_config_ping: i32,
    pub cb_config_select_known_packs: i32,
    pub cb_config_registry_data: i32,
    /// `ClientboundUpdateTagsPacket` in the **configuration** state (M69).
    ///
    /// This is the one that actually fires on a normal join: a vanilla server
    /// sends the whole tag set here, right after `registry_data`, and the play
    /// copy is only used for a datapack reload. Handling one and not the other
    /// would have looked like it worked for exactly as long as nobody reloaded.
    pub cb_config_update_tags: i32,
    pub cb_config_finish: i32,
    pub cb_config_cookie_request: Option<i32>,
    /// `ClientboundCustomPayloadPacket` in the **configuration** state (M78).
    ///
    /// This is the copy that actually fires: a vanilla server sends
    /// `new BrandPayload(getServerModName())` from
    /// `ServerConfigurationPacketListenerImpl`'s opening burst, and never sends
    /// a second one in play. `serverBrand` lives on the *common* listener both
    /// states extend, so handling only the play id would look like it worked
    /// against every server there is — M69's `update_tags` lesson, one packet
    /// over.
    pub cb_config_custom_payload: i32,
    /// `ClientboundStoreCookiePacket` in the **configuration** state (M78).
    /// A `common` packet like the payload above; a transfer-driven network
    /// stores its cookies on whichever side of the state boundary it happens to
    /// be, and the jar is one store.
    pub cb_config_store_cookie: i32,
    pub cb_config_disconnect: i32,
    // play
    pub sb_play_keep_alive: i32,
    pub sb_play_pong: i32,
    pub sb_play_chunk_batch_received: i32,
    pub sb_play_accept_teleport: i32,
    pub sb_play_player_loaded: i32,
    pub sb_play_config_acknowledged: i32,
    pub sb_play_cookie_response: Option<i32>,
    pub cb_play_keep_alive: i32,
    pub cb_play_ping: i32,
    pub cb_play_login: i32,
    pub cb_play_position: i32,
    /// `ClientboundPlayerRotationPacket` — the *rotational* teleport (M76).
    ///
    /// Required, and the reason is the asymmetry it closes: `player_position`
    /// above has been handled since M3, so a version that renamed only this one
    /// would leave a client that accepts a body teleport and silently ignores a
    /// head turn — which is exactly the failure
    /// `REWO_PACKET_COVERAGE.md` §3 ranks second, because the working half
    /// misdirects the diagnosis.
    ///
    /// Ten fixed bytes, `FLOAT yRot, BOOL relativeY, FLOAT xRot, BOOL relativeX`
    /// — **not** the packed `Relative` mask its positional twin carries. See
    /// [`crate::player_rotation`].
    pub cb_play_player_rotation: i32,
    /// `ClientboundPlayerLookAtPacket` — `/teleport … facing` (M76). Required
    /// for the same reason as its sibling above.
    pub cb_play_player_look_at: i32,
    /// `ClientboundSetDefaultSpawnPositionPacket` — `LevelData.RespawnData`,
    /// the compass target and the bedless respawn point (M76). Required: a
    /// vanilla server sends it on every join, so a missing *name* is a version
    /// mismatch rather than a quiet runtime state.
    pub cb_play_set_default_spawn_position: i32,
    /// Dimension change / death respawn (`ClientboundRespawnPacket`), carrying
    /// the same `CommonPlayerSpawnInfo` the login packet does. Required: it is
    /// the only announcement that the world's vertical shape and lighting
    /// contract just changed, so a missing name is a version mismatch that must
    /// fail loud rather than leave us decoding the Nether as the Overworld.
    /// Decoded by `spawn_info::RespawnInfo` and applied by
    /// `play::PlaySession::apply_respawn`.
    pub cb_play_respawn: i32,
    pub cb_play_chunk_batch_finished: i32,
    pub cb_play_level_chunk: i32,
    /// Lighting changed without the chunk being resent (torch placed, cave
    /// mined into). Optional: a server that never sends it is still correct.
    pub cb_play_light_update: Option<i32>,
    pub cb_play_forget_chunk: i32,
    pub cb_play_block_update: i32,
    /// `block_entity_data` — one block entity's update tag (M25).
    pub cb_play_block_entity_data: i32,
    /// `block_event` — a block's `triggerEvent` pair (chest lids, bells).
    pub cb_play_block_event: i32,
    pub cb_play_section_blocks_update: i32,
    /// Biomes changed for already-loaded chunks (`/fillbiome`, world-gen
    /// re-send). Optional: a server that never sends it is still correct.
    pub cb_play_chunks_biomes: Option<i32>,
    pub cb_play_set_time: i32,
    pub cb_play_add_entity: i32,
    pub cb_play_remove_entities: i32,
    pub cb_play_move_entity_pos: i32,
    pub cb_play_move_entity_pos_rot: i32,
    pub cb_play_move_entity_rot: i32,
    pub cb_play_entity_position_sync: i32,
    pub cb_play_teleport_entity: i32,
    pub cb_play_player_info_update: i32,
    pub cb_play_player_info_remove: i32,
    pub cb_play_set_entity_data: i32,
    /// `ClientboundSetPlayerTeamPacket` — the scoreboard-team state (M62),
    /// which is where the tab list's third sort key comes from. Required: it
    /// is a core vanilla packet, so a missing *name* is a version mismatch
    /// and should fail loud here rather than leave every player teamless.
    /// (A server that never forms a team simply never sends one — that is a
    /// quiet runtime state, not a missing name.)
    pub cb_play_set_player_team: i32,
    /// `ClientboundSetPassengersPacket` — a var-int vehicle id then a var-int
    /// array of passenger ids (M70). Required: it is a core vanilla packet, so
    /// a missing *name* is a version mismatch and should fail loud rather than
    /// leave every ridden entity reading as un-ridden. It is what answers
    /// `Entity.isVehicle()`, which suppresses a ridden entity's label.
    pub cb_play_set_passengers: i32,
    pub cb_play_rotate_head: i32,
    /// `ClientboundEntityEventPacket` — a signed BE i32 entity id + a signed
    /// byte event. Required: the model-visible events (warden attack/sonic
    /// boom, armadillo peek) ride it, so a missing name is a version mismatch
    /// that should fail loud rather than silently drop those animations.
    pub cb_play_entity_event: i32,
    /// `ClientboundAnimatePacket` — a VarInt entity id + an **unsigned byte**
    /// action. Required: the combat arm swings (actions 0 / 3) ride it, so a
    /// missing name is a version mismatch that should fail loud rather than
    /// silently drop every swing.
    pub cb_play_animate: i32,
    /// `ClientboundDamageEventPacket` — the hurt flash + hurt clock (M21).
    /// Required: without it nothing an entity takes ever shows, and a missing
    /// name is a version mismatch rather than an absent feature.
    pub cb_play_damage_event: i32,
    /// `ClientboundGameEventPacket` — an unsigned-byte event id and a float
    /// param. It carries a dozen unrelated things; M33 consumes the four
    /// weather ones (`START_RAINING` 1, `STOP_RAINING` 2, `RAIN_LEVEL_CHANGE`
    /// 7, `THUNDER_LEVEL_CHANGE` 8). Required: without it the sky never rains,
    /// and a missing name is a version mismatch rather than clear weather.
    pub cb_play_game_event: i32,
    /// `ClientboundPlayerAbilitiesPacket` — a flags byte then flying speed then
    /// walking speed (M75). Required: it is the *authoritative* announcement of
    /// whether the local player may fly, is flying, and is invulnerable, and it
    /// is sent on every join. A missing name is a version mismatch that should
    /// fail loud rather than leave a creative player unable to leave the ground.
    pub cb_play_player_abilities: i32,
    /// `ServerboundPlayerAbilitiesPacket` — **one byte**, carrying only the
    /// flying bit (M75). Required for the same reason in the other direction:
    /// the client owns the flight toggle, so without this the server never
    /// learns we are flying and its own `Abilities.flying` stays false. Note
    /// this shares its *name* with the clientbound packet above; the direction
    /// is what disambiguates them in the report.
    pub sb_play_player_abilities: i32,
    /// `ClientboundContainerSetContentPacket` — a whole container's slots
    /// (M34). Required: without it the client never learns what is in the
    /// player's own inventory.
    pub cb_play_container_set_content: i32,
    /// `ClientboundContainerSetSlotPacket` — one slot.
    pub cb_play_container_set_slot: i32,
    /// `ClientboundSetHeldSlotPacket` — the server moving the selection.
    pub cb_play_set_held_slot: i32,
    /// `ClientboundSetPlayerInventoryPacket` (M69) — one authoritative slot
    /// write, addressed by **inventory index** rather than menu slot.
    /// `container_set_slot`'s index-addressed twin; see
    /// `rewo_world::inventory::menu_slot_of_inventory_index`.
    pub cb_play_set_player_inventory: i32,
    /// `ClientboundSetCursorItemPacket` (M69) — the server's authoritative
    /// carried stack, and the only correction path M35's predicted cursor has
    /// short of a whole `container_set_content`.
    pub cb_play_set_cursor_item: i32,
    /// `ClientboundUpdateTagsPacket` in the **play** state (M69) — a datapack
    /// reload mid-session. The same packet also exists in configuration, which
    /// is where a vanilla server sends it on join; both are resolved because
    /// they are two different ids for one body.
    pub cb_play_update_tags: i32,
    /// `ClientboundSetEquipmentPacket` — the held items that decide a swing's
    /// duration and animation type. Required for the same reason: without it
    /// every entity would silently swing with the bare-hand default.
    pub cb_play_set_equipment: i32,
    /// `ClientboundUpdateAttributesPacket` — the entity attribute snapshots
    /// (M52), and the only source of a mob's *maximum* health: metadata index
    /// 9 carries the current value and nothing carries the range. Required —
    /// every living entity gets one on spawn, so a missing name is a version
    /// mismatch rather than an absent feature.
    pub cb_play_update_attributes: i32,
    /// Player visual effects (M13 lightmap). Required: the lightmap's
    /// night-vision / darkness factors depend on them, so a missing name is a
    /// version-mismatch that should fail loud rather than silently disable the
    /// camera effect.
    pub cb_play_update_mob_effect: i32,
    pub cb_play_remove_mob_effect: i32,
    pub cb_play_start_configuration: Option<i32>,
    pub cb_play_cookie_request: Option<i32>,
    pub cb_play_disconnect: i32,
    // M3 gameplay
    pub sb_play_move_pos: i32,
    pub sb_play_move_pos_rot: i32,
    pub sb_play_move_rot: i32,
    pub sb_play_move_status: i32,
    pub sb_play_player_input: Option<i32>,
    pub sb_play_client_tick_end: Option<i32>,
    pub sb_play_chat: Option<i32>,
    pub sb_play_chat_command: Option<i32>,
    pub sb_play_chat_session_update: Option<i32>,
    pub sb_play_set_creative_slot: Option<i32>,
    pub sb_play_set_carried_item: Option<i32>,
    pub sb_play_player_action: i32,
    pub sb_play_use_item_on: Option<i32>,
    pub sb_play_interact: Option<i32>,
    pub sb_play_swing: Option<i32>,
    pub sb_play_client_command: Option<i32>,
    pub sb_play_container_click: Option<i32>,
    pub sb_play_use_item: Option<i32>,
    pub cb_play_set_health: Option<i32>,
    pub cb_play_system_chat: Option<i32>,
    pub cb_play_player_chat: Option<i32>,
    pub cb_play_block_ack: Option<i32>,
    // M37 particles. Both optional: a server that never emits an effect is
    // still a correct server, and the client must not fail to connect to one.
    pub cb_play_level_particles: Option<i32>,
    pub cb_play_level_event: Option<i32>,
    /// `ClientboundSoundPacket` — a sound at a world position (M63).
    /// Required, like every other core play packet here: `require` grades the
    /// *report*, not the server, so this fails loud when a version bump
    /// renames or drops the packet rather than silently decoding nothing.
    /// (There is no `custom_sound` in 26.2 — it has not existed since the
    /// sound-event registry replaced it.)
    pub cb_play_sound: i32,
    /// `ClientboundSoundEntityPacket` — a sound that follows an entity.
    pub cb_play_sound_entity: i32,
    /// `ClientboundStopSoundPacket` — the bitmask-driven cancel.
    pub cb_play_stop_sound: i32,
    // ── M65: the server-driven display packets ────────────────────────────
    // All `req!` for the reason the M62/M63 entries give: `require` grades
    // the *report*, not the server. A vanilla server that never creates an
    // objective simply never sends one — that is a runtime state. A report
    // with no `set_objective` in it is a version mismatch, and should fail at
    // connect rather than leave every sidebar permanently blank.
    /// `ClientboundSetObjectivePacket` — objective create / remove / update.
    pub cb_play_set_objective: i32,
    /// `ClientboundSetScorePacket` — one score line's value.
    pub cb_play_set_score: i32,
    /// `ClientboundResetScorePacket` — drop one score, or all of a holder's.
    pub cb_play_reset_score: i32,
    /// `ClientboundSetDisplayObjectivePacket` — which objective shows in which
    /// slot (sidebar / tab list / below name).
    pub cb_play_set_display_objective: i32,
    /// `ClientboundBossEventPacket` — the operation-tagged boss-bar union.
    pub cb_play_boss_event: i32,
    /// `ClientboundTabListPacket` — the tab list's header and footer.
    pub cb_play_tab_list: i32,
    // ── M67: the server's view area ───────────────────────────────────────
    // All three `req!` for the reason M62/M63/M65 give: `require` grades the
    // *report*, not the server. A server that never resends its radius is a
    // runtime state (login already carried it); a report with no
    // `set_chunk_cache_radius` in it is a version mismatch.
    /// `ClientboundSetChunkCacheCenterPacket` — the chunk the view area is
    /// measured from. Two VarInts.
    pub cb_play_set_chunk_cache_center: i32,
    /// `ClientboundSetChunkCacheRadiusPacket` — the server's render distance.
    pub cb_play_set_chunk_cache_radius: i32,
    /// `ClientboundSetSimulationDistancePacket` — how far entities tick.
    /// A different quantity from the radius above and a byte-identical body,
    /// so the id is the only thing that tells them apart.
    pub cb_play_set_simulation_distance: i32,
    // ── M68: the four packets that move the local player ──────────────────
    // All four `req!`, for the reason M62/M63/M65/M67 give: `require` grades
    // the *report*, not the server. A server whose players are never knocked
    // back simply never sends `explode` — a runtime state. A report with no
    // `explode` in it is a version mismatch, and a client whose knockback,
    // velocity and mount handling silently vanished would look exactly like a
    // physics bug rather than a protocol one.
    /// `ClientboundExplodePacket` — whose `playerKnockback` is an
    /// `addDeltaMovement` straight onto the local player.
    pub cb_play_explode: i32,
    /// `ClientboundSetEntityMotionPacket` — one entity's velocity, including
    /// the local player's own knockback. The body is a VarInt id plus
    /// `Vec3.LP_STREAM_CODEC`, **not** the legacy short fixed point.
    pub cb_play_set_entity_motion: i32,
    /// `ClientboundMoveVehiclePacket` — the server repositioning the vehicle
    /// the local player rides. Carries no entity id: the client resolves
    /// `player.getRootVehicle()` itself.
    pub cb_play_move_vehicle: i32,
    // ── M74: the chunk-batch loop, the tick clock, three client switches ──
    // All six `req!` for the reason M62/M63/M65/M67/M68 give: `require` grades
    // the *report*, not the server. A server that never freezes its tick loop
    // simply never sends `ticking_state` — a runtime state. A report with no
    // `ticking_state` in it is a version mismatch.
    /// `ClientboundChunkBatchStartPacket` — **empty body**. Stamps the clock
    /// the batch-size calculator measures against. Without it the calculator
    /// has no interval and would reply a different constant rather than an
    /// adaptive number; see [`crate::chunk_batch`].
    pub cb_play_chunk_batch_start: i32,
    /// `ClientboundChangeDifficultyPacket` — a VarInt `Difficulty` id through
    /// a **WRAP** out-of-bounds map, then a bool `locked`.
    pub cb_play_change_difficulty: i32,
    /// `ClientboundContainerClosePacket` — one VarInt container id, which
    /// vanilla reads and then **ignores**: `handleContainerClose` closes
    /// whatever is open without comparing ids.
    pub cb_play_container_close: i32,
    /// `ClientboundSetCameraPacket` — one VarInt entity id. An id the client
    /// cannot resolve leaves the camera where it is.
    pub cb_play_set_camera: i32,
    /// `ClientboundTickingStatePacket` — an f32 `tickRate` then a bool
    /// `isFrozen`. `/tick rate` and `/tick freeze`.
    pub cb_play_ticking_state: i32,
    /// `ClientboundTickingStepPacket` — one VarInt `tickSteps`. `/tick step`.
    pub cb_play_ticking_step: i32,
    // ── M77: three packets that write entity state ────────────────────────
    // All three `req!` for the reason M62/M63/M65/M67/M68/M74 give: `require`
    // grades the *report*, not the server. A server with no minecart, no leash
    // and no fireball on it simply never sends any of these — a runtime state.
    // A report missing one of these names is a version mismatch.
    /// `ClientboundMoveMinecartPacket` — a VarInt entity id then a var-int
    /// counted list of `MinecartStep`s, each two **full-double** `Vec3`s (not
    /// the `LP_STREAM_CODEC` packing), two rotation bytes and an f32 weight.
    /// It is the *only* movement channel an experimental-movement cart has:
    /// `ServerEntity.sendChanges` routes such a cart down `handleMinecartPosRot`
    /// instead of the generic position branch entirely.
    pub cb_play_move_minecart_along_track: i32,
    /// `ClientboundSetEntityLinkPacket` — **two fixed big-endian i32s**, the
    /// leashed entity and its holder, with `0` meaning no holder. Consumed for
    /// the holder id only; drawing the rope is a separate, unstarted piece of
    /// work.
    pub cb_play_set_entity_link: i32,
    /// `ClientboundProjectilePowerPacket` — a VarInt id then an f64
    /// `accelerationPower`, written onto `AbstractHurtingProjectile` and
    /// nothing else (an arrow is an `AbstractArrow` and is not one).
    pub cb_play_projectile_power: i32,
    // ── M78: session, server metadata and chat ────────────────────────────
    // All eight `req!` for the reason M62/M63/M65/M67/M68/M74 give: `require`
    // grades the *report*, not the server. A server that never sets a cookie
    // simply never sends `store_cookie` — a runtime state. A report with no
    // `store_cookie` in it is a version mismatch.
    /// `ClientboundBundleDelimiterPacket` — **empty body, and not an inert
    /// packet**: `BundleDelimiterPacket.handle` throws outright, because it is
    /// a pipeline instruction rather than a message. Everything between two of
    /// them is applied in one go; see [`crate::bundle`].
    pub cb_play_bundle_delimiter: i32,
    /// `ClientboundCustomPayloadPacket` in the **play** state — an `Identifier`
    /// then a payload chosen by it. 26.2 registers one known type,
    /// `minecraft:brand`; anything else is a `DiscardedPayload` whose reader
    /// consumes the remainder. The copy a vanilla server actually sends is the
    /// configuration one above.
    pub cb_play_custom_payload: i32,
    /// `ClientboundDisguisedChatPacket` — a `Component` plus a
    /// `ChatType.Bound`, the chat line a server sends when a message should
    /// render as chat without carrying a player signature (`/msg`, plugin
    /// routing). Its `Bound` opens with `ByteBufCodecs.**holder**` — `id + 1`,
    /// `0` = inline — not `holderRegistry`.
    pub cb_play_disguised_chat: i32,
    /// `ClientboundGameRuleValuesPacket` — a counted map of
    /// `ResourceKey<GameRule>` → string.
    pub cb_play_game_rule_values: i32,
    /// `ClientboundPlayerCombatEndPacket` — a VarInt `duration`, and a vanilla
    /// handler that is an **empty method**. Decoded so the reader position is
    /// right; nothing is stored, because vanilla stores nothing.
    pub cb_play_player_combat_end: i32,
    /// `ClientboundPlayerCombatEnterPacket` — `StreamCodec.unit`, i.e. **zero
    /// bytes**, and an empty handler. Its sibling above is *not* empty on the
    /// wire; the two are vestigial in the same way and shaped differently.
    pub cb_play_player_combat_enter: i32,
    /// `ClientboundServerDataPacket` — the MOTD `Component` and an
    /// `Optional<byte[]>` icon.
    pub cb_play_server_data: i32,
    /// `ClientboundStoreCookiePacket` in the **play** state — the other half of
    /// the loop `cookie_request` already answers. Before M78 the reply was
    /// always "no payload", because nothing ever stored one.
    pub cb_play_store_cookie: i32,
}

impl Ids {
    pub fn resolve(p: &Packets) -> Result<Self, String> {
        use Dir::{Clientbound as C, Serverbound as S};
        use State::{Configuration as Cfg, Login as L, Play as P};
        Ok(Self {
            sb_handshake_intention: req!(p, State::Handshake, S, "intention"),
            sb_login_hello: req!(p, L, S, "hello"),
            sb_login_key: req!(p, L, S, "key"),
            sb_login_acknowledged: req!(p, L, S, "login_acknowledged"),
            cb_login_compression: req!(p, L, C, "login_compression"),
            cb_login_finished: req!(p, L, C, "login_finished"),
            cb_login_disconnect: req!(p, L, C, "login_disconnect"),
            cb_login_hello: req!(p, L, C, "hello"),

            sb_config_custom_payload: req!(p, Cfg, S, "custom_payload"),
            sb_config_client_information: req!(p, Cfg, S, "client_information"),
            sb_config_keep_alive: req!(p, Cfg, S, "keep_alive"),
            sb_config_pong: req!(p, Cfg, S, "pong"),
            sb_config_select_known_packs: req!(p, Cfg, S, "select_known_packs"),
            sb_config_finish: req!(p, Cfg, S, "finish_configuration"),
            sb_config_cookie_response: req!(p, Cfg, S, "cookie_response"),
            cb_config_keep_alive: req!(p, Cfg, C, "keep_alive"),
            cb_config_ping: req!(p, Cfg, C, "ping"),
            cb_config_select_known_packs: req!(p, Cfg, C, "select_known_packs"),
            cb_config_registry_data: req!(p, Cfg, C, "registry_data"),
            cb_config_update_tags: req!(p, Cfg, C, "update_tags"),
            cb_config_finish: req!(p, Cfg, C, "finish_configuration"),
            cb_config_cookie_request: opt!(p, Cfg, C, "cookie_request"),
            cb_config_custom_payload: req!(p, Cfg, C, "custom_payload"),
            cb_config_store_cookie: req!(p, Cfg, C, "store_cookie"),
            cb_config_disconnect: req!(p, Cfg, C, "disconnect"),

            sb_play_keep_alive: req!(p, P, S, "keep_alive"),
            sb_play_pong: req!(p, P, S, "pong"),
            sb_play_chunk_batch_received: req!(p, P, S, "chunk_batch_received"),
            sb_play_accept_teleport: req!(p, P, S, "accept_teleportation"),
            sb_play_player_loaded: req!(p, P, S, "player_loaded"),
            sb_play_config_acknowledged: req!(p, P, S, "configuration_acknowledged"),
            sb_play_cookie_response: opt!(p, P, S, "cookie_response"),
            cb_play_keep_alive: req!(p, P, C, "keep_alive"),
            cb_play_ping: req!(p, P, C, "ping"),
            cb_play_login: req!(p, P, C, "login"),
            cb_play_position: req!(p, P, C, "player_position"),
            cb_play_respawn: req!(p, P, C, "respawn"),
            cb_play_chunk_batch_finished: req!(p, P, C, "chunk_batch_finished"),
            cb_play_level_chunk: req!(p, P, C, "level_chunk_with_light"),
            cb_play_light_update: opt!(p, P, C, "light_update"),
            cb_play_forget_chunk: req!(p, P, C, "forget_level_chunk"),
            cb_play_block_update: req!(p, P, C, "block_update"),
            cb_play_block_entity_data: req!(p, P, C, "block_entity_data"),
            cb_play_block_event: req!(p, P, C, "block_event"),
            cb_play_section_blocks_update: req!(p, P, C, "section_blocks_update"),
            cb_play_chunks_biomes: opt!(p, P, C, "chunks_biomes"),
            cb_play_set_time: req!(p, P, C, "set_time"),
            cb_play_add_entity: req!(p, P, C, "add_entity"),
            cb_play_remove_entities: req!(p, P, C, "remove_entities"),
            cb_play_move_entity_pos: req!(p, P, C, "move_entity_pos"),
            cb_play_move_entity_pos_rot: req!(p, P, C, "move_entity_pos_rot"),
            cb_play_move_entity_rot: req!(p, P, C, "move_entity_rot"),
            cb_play_entity_position_sync: req!(p, P, C, "entity_position_sync"),
            cb_play_teleport_entity: req!(p, P, C, "teleport_entity"),
            cb_play_player_info_update: req!(p, P, C, "player_info_update"),
            cb_play_player_info_remove: req!(p, P, C, "player_info_remove"),
            cb_play_set_entity_data: req!(p, P, C, "set_entity_data"),
            cb_play_set_player_team: req!(p, P, C, "set_player_team"),
            cb_play_set_passengers: req!(p, P, C, "set_passengers"),
            cb_play_rotate_head: req!(p, P, C, "rotate_head"),
            cb_play_entity_event: req!(p, P, C, "entity_event"),
            cb_play_animate: req!(p, P, C, "animate"),
            cb_play_damage_event: req!(p, P, C, "damage_event"),
            cb_play_game_event: req!(p, P, C, "game_event"),
            cb_play_player_abilities: req!(p, P, C, "player_abilities"),
            sb_play_player_abilities: req!(p, P, S, "player_abilities"),
            cb_play_container_set_content: req!(p, P, C, "container_set_content"),
            cb_play_container_set_slot: req!(p, P, C, "container_set_slot"),
            cb_play_set_held_slot: req!(p, P, C, "set_held_slot"),
            cb_play_set_player_inventory: req!(p, P, C, "set_player_inventory"),
            cb_play_set_cursor_item: req!(p, P, C, "set_cursor_item"),
            cb_play_update_tags: req!(p, P, C, "update_tags"),
            cb_play_set_equipment: req!(p, P, C, "set_equipment"),
            cb_play_update_attributes: req!(p, P, C, "update_attributes"),
            cb_play_update_mob_effect: req!(p, P, C, "update_mob_effect"),
            cb_play_remove_mob_effect: req!(p, P, C, "remove_mob_effect"),
            cb_play_start_configuration: opt!(p, P, C, "start_configuration"),
            cb_play_cookie_request: opt!(p, P, C, "cookie_request"),
            cb_play_disconnect: req!(p, P, C, "disconnect"),

            sb_play_move_pos: req!(p, P, S, "move_player_pos"),
            sb_play_move_pos_rot: req!(p, P, S, "move_player_pos_rot"),
            sb_play_move_rot: req!(p, P, S, "move_player_rot"),
            sb_play_move_status: req!(p, P, S, "move_player_status_only"),
            sb_play_player_input: opt!(p, P, S, "player_input"),
            sb_play_client_tick_end: opt!(p, P, S, "client_tick_end"),
            sb_play_chat: opt!(p, P, S, "chat"),
            sb_play_chat_command: opt!(p, P, S, "chat_command"),
            sb_play_chat_session_update: opt!(p, P, S, "chat_session_update"),
            sb_play_set_creative_slot: opt!(p, P, S, "set_creative_mode_slot"),
            sb_play_set_carried_item: opt!(p, P, S, "set_carried_item"),
            sb_play_player_action: req!(p, P, S, "player_action"),
            sb_play_use_item_on: opt!(p, P, S, "use_item_on"),
            sb_play_interact: opt!(p, P, S, "interact"),
            sb_play_swing: opt!(p, P, S, "swing"),
            sb_play_client_command: opt!(p, P, S, "client_command"),
            sb_play_container_click: opt!(p, P, S, "container_click"),
            sb_play_use_item: opt!(p, P, S, "use_item"),
            cb_play_set_health: opt!(p, P, C, "set_health"),
            cb_play_system_chat: opt!(p, P, C, "system_chat"),
            cb_play_player_chat: opt!(p, P, C, "player_chat"),
            cb_play_block_ack: opt!(p, P, C, "block_changed_ack"),
            cb_play_level_particles: opt!(p, P, C, "level_particles"),
            cb_play_level_event: opt!(p, P, C, "level_event"),
            cb_play_sound: req!(p, P, C, "sound"),
            cb_play_sound_entity: req!(p, P, C, "sound_entity"),
            cb_play_stop_sound: req!(p, P, C, "stop_sound"),
            cb_play_set_objective: req!(p, P, C, "set_objective"),
            cb_play_set_score: req!(p, P, C, "set_score"),
            cb_play_reset_score: req!(p, P, C, "reset_score"),
            cb_play_set_display_objective: req!(p, P, C, "set_display_objective"),
            cb_play_boss_event: req!(p, P, C, "boss_event"),
            cb_play_tab_list: req!(p, P, C, "tab_list"),
            cb_play_set_chunk_cache_center: req!(p, P, C, "set_chunk_cache_center"),
            cb_play_set_chunk_cache_radius: req!(p, P, C, "set_chunk_cache_radius"),
            cb_play_set_simulation_distance: req!(p, P, C, "set_simulation_distance"),
            cb_play_explode: req!(p, P, C, "explode"),
            cb_play_set_entity_motion: req!(p, P, C, "set_entity_motion"),
            cb_play_move_vehicle: req!(p, P, C, "move_vehicle"),
            cb_play_chunk_batch_start: req!(p, P, C, "chunk_batch_start"),
            cb_play_change_difficulty: req!(p, P, C, "change_difficulty"),
            cb_play_container_close: req!(p, P, C, "container_close"),
            cb_play_set_camera: req!(p, P, C, "set_camera"),
            cb_play_ticking_state: req!(p, P, C, "ticking_state"),
            cb_play_ticking_step: req!(p, P, C, "ticking_step"),
            cb_play_move_minecart_along_track: req!(p, P, C, "move_minecart_along_track"),
            cb_play_set_entity_link: req!(p, P, C, "set_entity_link"),
            cb_play_projectile_power: req!(p, P, C, "projectile_power"),
            cb_play_bundle_delimiter: req!(p, P, C, "bundle_delimiter"),
            cb_play_custom_payload: req!(p, P, C, "custom_payload"),
            cb_play_disguised_chat: req!(p, P, C, "disguised_chat"),
            cb_play_game_rule_values: req!(p, P, C, "game_rule_values"),
            cb_play_player_combat_end: req!(p, P, C, "player_combat_end"),
            cb_play_player_combat_enter: req!(p, P, C, "player_combat_enter"),
            cb_play_server_data: req!(p, P, C, "server_data"),
            cb_play_store_cookie: req!(p, P, C, "store_cookie"),
            cb_play_player_rotation: req!(p, P, C, "player_rotation"),
            cb_play_player_look_at: req!(p, P, C, "player_look_at"),
            cb_play_set_default_spawn_position: req!(p, P, C, "set_default_spawn_position"),
        })
    }
}

/// The machine check that keeps `REWO_PACKET_COVERAGE.md` honest (M74).
///
/// ## Why this is here and not in a `*shot` gate
///
/// The document is an inventory of a live codebase, and M67's hand-maintained
/// version **went stale within hours of being written** — four packets landed
/// in this very file the day it was published, and by the time M74 re-derived
/// it ten of its 141 rows were wrong, all in the same direction. The lesson
/// recorded there is that annotating the decay does not fix it; the document
/// has to derive from the code or fail when it disagrees.
///
/// So the check fires on **the event that causes the drift** — someone editing
/// `ids.rs` — and `cargo test -p rewo-net` is what runs then. A `*shot --check`
/// command would only fire when someone remembered to run it, which is the
/// same failure mode as the grep it replaces.
///
/// ## Why `include_str!`
///
/// All three inputs are read at **compile time**, so the check has no runtime
/// dependency on the datagen report, the network, or the working directory,
/// and there is no "skip if the file is missing" branch for it to fall
/// through. A missing file is a build error.
///
/// ## What it deliberately does not check
///
/// **Correctness and completeness.** A packet that is dispatched and
/// half-decoded is `handled` to this test — that is precisely the gap §4 of
/// the document is maintained by hand to cover, and no mechanical instrument
/// distinguishes "consumed the body" from "read the first field".
#[cfg(test)]
mod coverage_table_tests {
    /// The document itself. `crates/rewo-net/src/` → three levels to the root.
    const DOC: &str = include_str!("../../../REWO_PACKET_COVERAGE.md");
    /// This file, read as text: the check parses the `resolve` block above
    /// rather than calling it, because calling it needs a `Packets` and
    /// therefore the datagen report.
    const IDS_SRC: &str = include_str!("ids.rs");
    /// The dispatch chain. `play.rs` holds `PlaySession::handle`'s `else if`
    /// ladder; `lib.rs` holds the `route_*` seams.
    const PLAY_SRC: &str = include_str!("play.rs");
    const LIB_SRC: &str = include_str!("lib.rs");

    /// Whether `hay` contains `needle` delimited by non-identifier characters.
    ///
    /// A plain `contains` is wrong here: `cb_play_sound` is a prefix of
    /// `cb_play_sound_entity`, so a substring test would report the shorter
    /// field as dispatched on a line that only mentions the longer one.
    fn contains_word(hay: &str, needle: &str) -> bool {
        let mut from = 0;
        while let Some(i) = hay[from..].find(needle) {
            let s = from + i;
            let e = s + needle.len();
            let before_ok = s == 0
                || !hay[..s]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let after_ok = hay[e..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if before_ok && after_ok {
                return true;
            }
            from = e;
        }
        false
    }

    /// `(field, packet name)` for every clientbound-**play** id the `resolve`
    /// block above names, whether `req!` or `opt!`.
    fn resolved_play_clientbound() -> Vec<(String, String)> {
        let mut out = Vec::new();
        for line in IDS_SRC.lines() {
            let t = line.trim();
            if t.starts_with("//") {
                continue;
            }
            for mac in ["req!(p, P, C, \"", "opt!(p, P, C, \""] {
                let Some(i) = t.find(mac) else { continue };
                let field = t[..i].trim_end().trim_end_matches(':').trim().to_string();
                let rest = &t[i + mac.len()..];
                let Some(q) = rest.find('"') else { continue };
                out.push((field, rest[..q].to_string()));
            }
        }
        out
    }

    /// Whether an incoming packet id is ever tested against this field inside
    /// `rewo-net` — an `==` / `!=` comparison, or a dispatch-table struct
    /// field (`center: ids.cb_play_set_chunk_cache_center`).
    ///
    /// Stricter than M67's "does the name appear anywhere under `crates/`",
    /// which counted a field a gate merely mentions. Both instruments agree
    /// that the resolved-but-ignored set is empty; this one says something
    /// closer to what the word "handled" claims.
    fn is_dispatched(field: &str) -> bool {
        let by_table_a = format!(": ids.{field}");
        let by_table_b = format!(": self.ids.{field}");
        [PLAY_SRC, LIB_SRC].iter().any(|src| {
            src.lines().any(|line| {
                let t = line.trim();
                !t.starts_with("//")
                    && contains_word(t, field)
                    && (t.contains("==")
                        || t.contains("!=")
                        || t.contains(&by_table_a)
                        || t.contains(&by_table_b))
            })
        })
    }

    /// `(id, packet, status, resolution-or-class)` for every §5 row.
    fn doc_rows() -> Vec<(i32, String, String, String)> {
        let start = DOC.find("## §5").expect("the document has no §5");
        let rest = &DOC[start..];
        let end = rest.find("## §6").expect("§5 is not terminated by §6");
        let mut out = Vec::new();
        for line in rest[..end].lines() {
            let t = line.trim();
            if !t.starts_with('|') {
                continue;
            }
            let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
            if cells.len() < 4 {
                continue;
            }
            // The header and the `|---|` separator fail this parse, which is
            // how they are skipped.
            let Ok(id) = cells[0].parse::<i32>() else {
                continue;
            };
            out.push((
                id,
                cells[1].trim_matches('`').to_string(),
                cells[2].to_string(),
                cells[3].to_string(),
            ));
        }
        out
    }

    /// The first `**<integer>**` on the line beginning with `label`.
    fn doc_count(label: &str) -> i64 {
        let line = DOC
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(label))
            .unwrap_or_else(|| panic!("no §2 row labelled {label:?}"));
        let mut rest = &line[label.len()..];
        while let Some(i) = rest.find("**") {
            let after = &rest[i + 2..];
            let Some(j) = after.find("**") else { break };
            if let Ok(n) = after[..j].trim().parse::<i64>() {
                return n;
            }
            rest = &after[j + 2..];
        }
        panic!("no bold integer on the §2 row labelled {label:?}");
    }

    /// The whole check. One test rather than several so a single failure
    /// message can show every disagreeing row at once — the useful output is
    /// the list, not the first item in it.
    #[test]
    fn the_coverage_table_matches_the_code() {
        let rows = doc_rows();

        // ── Structure: 141 rows, ids 0..=140, contiguous and unique. ───────
        let mut ids: Vec<i32> = rows.iter().map(|r| r.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            rows.len(),
            "§5 has duplicate protocol ids — {} rows, {} distinct",
            rows.len(),
            ids.len()
        );
        assert_eq!(
            rows.len(),
            141,
            "§5 must list all 141 clientbound-play packets, found {}",
            rows.len()
        );
        for (want, got) in (0..141).zip(ids.iter().copied()) {
            assert_eq!(got, want, "§5's ids are not contiguous from 0");
        }

        // ── Status: `handled` iff resolved AND dispatched. ─────────────────
        let resolved = resolved_play_clientbound();
        let mut wrong = Vec::new();
        let mut handled = 0usize;
        let mut ignored = 0usize;
        let mut absent = 0usize;
        for (id, packet, status, resolution) in &rows {
            let field = resolved
                .iter()
                .find(|(_, name)| name == packet)
                .map(|(f, _)| f.as_str());
            let truth = match field {
                None => "absent",
                Some(f) if is_dispatched(f) => "handled",
                // The negative finding §1 records. If this ever fires, the
                // document's "resolved but ignored: 0" is no longer true and
                // §1 needs rewriting, not just the count.
                Some(_) => "resolved-but-ignored",
            };
            match truth {
                "handled" => handled += 1,
                "absent" => absent += 1,
                _ => ignored += 1,
            }
            if status != truth {
                wrong.push(format!(
                    "  {id:>3} {packet:<30} table says {status:?}, code says {truth:?}"
                ));
            }
            // Every gap carries a class letter, so §2's class table has
            // something to count.
            if truth == "absent" {
                assert!(
                    ["**A**", "**B**", "**C**", "**D**"]
                        .iter()
                        .any(|c| resolution.starts_with(c)),
                    "§5 row {id} ({packet}) is absent but carries no class letter: {resolution:?}"
                );
            }
        }
        assert!(
            wrong.is_empty(),
            "REWO_PACKET_COVERAGE.md §5 disagrees with the code on {} row(s).\n\
             Flip these rows and update §2's counts:\n{}",
            wrong.len(),
            wrong.join("\n")
        );

        // ── Every resolved packet has a row. This is the case that actually
        // happened: M68 added three ids and nothing added three rows. ──────
        let listed: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        for (field, packet) in &resolved {
            assert!(
                listed.contains(&packet.as_str()),
                "`{field}` resolves \"{packet}\" but §5 has no row for it — \
                 either the packet was renamed or the table was not updated"
            );
        }

        // ── §2's counts. ───────────────────────────────────────────────────
        assert_eq!(
            doc_count("| Resolved **and** consumed |"),
            handled as i64,
            "§2's handled count is stale"
        );
        assert_eq!(
            doc_count("| Resolved but ignored |"),
            ignored as i64,
            "§2's resolved-but-ignored count is stale"
        );
        assert_eq!(
            doc_count("| Not resolved at all |"),
            absent as i64,
            "§2's absent count is stale"
        );
        assert_eq!(doc_count("| **Total clientbound-play** |"), 141);

        // ── §2's class table. ──────────────────────────────────────────────
        for (letter, label) in [
            ("**A**", "| **A** pure state, no rendering |"),
            ("**B**", "| **B** needs rendering |"),
            ("**C**", "| **C** needs a subsystem Rewo lacks |"),
            ("**D**", "| **D** not applicable |"),
        ] {
            let want = rows
                .iter()
                .filter(|(_, _, status, res)| status == "absent" && res.starts_with(letter))
                .count();
            assert_eq!(
                doc_count(label),
                want as i64,
                "§2's class-{} count is stale",
                &letter[2..3]
            );
        }
    }

    /// The parser is not vacuous: it really does find 141 rows and a
    /// three-figure resolved set, so a regex that silently matched nothing
    /// would fail here rather than passing the check above by counting zero
    /// of everything.
    ///
    /// MUTATION: breaking either parser — changing the `req!(p, P, C, "`
    /// needle, or the §5 section bounds — drops one of these to 0. Without
    /// this witness, `doc_rows()` returning an empty vec would make the
    /// contiguity loop, the status loop and the class loop all trivially
    /// true, and only the hard-coded `141` assertion would catch it.
    #[test]
    fn the_coverage_parsers_actually_parse_something() {
        assert_eq!(doc_rows().len(), 141);
        let resolved = resolved_play_clientbound();
        assert!(
            resolved.len() > 60,
            "the ids.rs parser found only {} play-clientbound entries",
            resolved.len()
        );
        // And the dispatch probe distinguishes: a resolved field is
        // dispatched, and an invented one is not.
        assert!(is_dispatched("cb_play_set_camera"));
        assert!(!is_dispatched("cb_play_not_a_real_field"));
        // `contains_word` must not match a prefix of a longer identifier —
        // the `cb_play_sound` / `cb_play_sound_entity` hazard.
        assert!(!contains_word("x == ids.cb_play_sound_entity", "cb_play_sound"));
        assert!(contains_word("x == ids.cb_play_sound;", "cb_play_sound"));
    }
}
