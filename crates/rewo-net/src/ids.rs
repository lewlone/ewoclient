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
    /// `ServerboundResourcePackPacket` (M166) — the reply that finishes
    /// `ServerResourcePackConfigurationTask`. Without it a server with
    /// `resource-pack=` set hangs the client in configuration forever.
    pub sb_config_resource_pack: i32,
    /// `ServerboundAcceptCodeOfConductPacket` (M166) — the zero-byte reply
    /// that finishes `ServerCodeOfConductConfigurationTask`. Queued *before*
    /// the resource-pack task, so on a server with both this is the one that
    /// hangs first.
    pub sb_config_accept_code_of_conduct: i32,
    pub cb_config_keep_alive: i32,
    pub cb_config_ping: i32,
    pub cb_config_select_known_packs: i32,
    pub cb_config_registry_data: i32,
    /// `ClientboundResourcePackPushPacket` in the **configuration** state
    /// (M166). Its sibling `resource_pack_pop` is deliberately left
    /// unresolved — see `config_tasks`.
    pub cb_config_resource_pack_push: i32,
    /// `ClientboundCodeOfConductPacket` (M166). Configuration-only: there is
    /// no play-state twin, because the code of conduct is shown before you
    /// join rather than during play.
    pub cb_config_code_of_conduct: i32,
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
    /// `ClientboundServerLinksPacket` in the **configuration** state (M85).
    ///
    /// The third `common` packet to need both ids, after M69's `update_tags`
    /// and M78's `custom_payload`/`store_cookie` — and `serverLinks` is a field
    /// of the very same `ClientCommonPacketListenerImpl` those two live on. A
    /// server is free to advertise its links before the play state begins, and
    /// then a play-only resolution would leave the pause menu with no links
    /// against a server that sent them.
    pub cb_config_server_links: i32,
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
    /// `ClientboundHurtAnimationPacket` (M81) — a VarInt entity id and a
    /// **big-endian f32** yaw. Required: it is `damage_event`'s twin and the
    /// only carrier of the camera tilt's *direction*, so a missing name is a
    /// version mismatch rather than a flat camera.
    pub cb_play_hurt_animation: i32,
    /// `ClientboundBlockDestructionPacket` (M81) — a VarInt breaker id, a
    /// packed `BlockPos` and an **unsigned byte** stage. Required: it is the
    /// only announcement that somebody else is mining a block, and a missing
    /// name is a version mismatch rather than an un-cracked world.
    pub cb_play_block_destruction: i32,
    /// `ClientboundTakeItemEntityPacket` (M81) — three VarInts: the collected
    /// entity, the collector, and the count taken. Required: the client's own
    /// copy of a dropped stack is shrunk *by this packet*, so a missing name
    /// leaves picked-up items lying on the ground forever.
    pub cb_play_take_item_entity: i32,
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
    /// `ServerboundCommandSuggestionPacket` (M114) — a VarInt request id then
    /// the command being typed, capped at **32500** UTF-16 units (neither
    /// `readUtf`'s default nor the chat field's 256).
    pub sb_play_command_suggestion: Option<i32>,
    pub sb_play_chat_session_update: Option<i32>,
    pub sb_play_set_creative_slot: Option<i32>,
    pub sb_play_set_carried_item: Option<i32>,
    pub sb_play_player_action: i32,
    pub sb_play_use_item_on: Option<i32>,
    pub sb_play_interact: Option<i32>,
    pub sb_play_swing: Option<i32>,
    pub sb_play_client_command: Option<i32>,
    /// `ServerboundPlayerCommandPacket` (M169) — `START_RIDING_JUMP` is the
    /// first action Rewo sends; the others (sprint, sleep, inventory,
    /// fall-flying) are not sent yet.
    pub sb_play_player_command: Option<i32>,
    pub sb_play_container_click: Option<i32>,
    /// `ServerboundContainerButtonClickPacket` (M92f) — the one input the
    /// bespoke-widget screens share. Optional for the same reason its sibling
    /// is: a client that never opens an enchanting table never needs it, and
    /// refusing to connect over a packet you may not send is worse than
    /// declining the click.
    pub sb_play_container_button_click: Option<i32>,
    /// `ServerboundRecipeBookChangeSettingsPacket` (M98) — the book's own
    /// open/filter state, echoed back so the server persists it.
    pub sb_play_recipe_book_change_settings: Option<i32>,
    /// `ServerboundPlaceRecipePacket` (M98) — click a recipe, ask the server to
    /// lay it out on the grid.
    pub sb_play_place_recipe: Option<i32>,
    /// `ServerboundSeenAdvancementsPacket` (M178) — the advancements screen's
    /// lifecycle packet: `OPENED_TAB` carries a tab id, `CLOSED_SCREEN` is
    /// bare. Optional like its siblings: a client that never opens the screen
    /// never needs it.
    pub sb_play_seen_advancements: Option<i32>,
    /// `container_slot_state_changed` (M93h) — the crafter's slot toggles.
    ///
    /// A **different packet** from `container_button_click`, not a mode of it:
    /// `CrafterScreen` never calls `clickMenuButton`, and `CrafterMenu` has no
    /// `clickMenuButton` override at all.
    pub sb_play_container_slot_state_changed: Option<i32>,
    /// `set_beacon` (M93l) — the beacon's confirm button.
    pub sb_play_set_beacon: Option<i32>,
    /// `rename_item` (M93n) — the anvil's name field.
    pub sb_play_rename_item: Option<i32>,
    /// `sign_update` (M174) — the editor's commit, sent from `removed()` on
    /// EVERY exit path (Done, Esc, the validity tick), with no dirty check.
    pub sb_play_sign_update: Option<i32>,
    /// `select_trade` (M93u) — one var-int, the offer's index.
    pub sb_play_select_trade: Option<i32>,
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
    /// `ClientboundOpenScreenPacket` (M87) — VarInt container id, then the
    /// `minecraft:menu` id as a **raw 0-based** `ByteBufCodecs.registry`
    /// (not `holder`'s `id + 1`), then an NBT `Component` title. An
    /// unregistered type makes `MenuScreens.create` log and do nothing, so a
    /// menu Rewo cannot lay out leaves whatever was open open.
    pub cb_play_open_screen: i32,
    /// `ClientboundContainerSetDataPacket` (M87) — VarInt container id, then
    /// **two signed `readShort`s** in a mostly-var-int protocol. Applied only
    /// when the id matches the open menu.
    pub cb_play_container_set_data: i32,
    /// `merchant_offers` (M93u) — the villager trade list.
    pub cb_play_merchant_offers: i32,
    /// `open_book` (M171) — opens the written-book view for a held book.
    pub cb_play_open_book: i32,
    /// `open_sign_editor` (M174) — packed BlockPos + isFrontText bool. The
    /// client opens the editor only if a SIGN block entity sits at the pos
    /// (else warn-and-ignore — 26.2 does NOT construct a fresh one).
    pub cb_play_open_sign_editor: i32,
    /// The recipe book's four (M93y).
    pub cb_play_recipe_book_add: i32,
    pub cb_play_recipe_book_remove: i32,
    pub cb_play_recipe_book_settings: i32,
    pub cb_play_place_ghost_recipe: i32,
    /// `update_recipes` (M152) — the `RecipePropertySet` map that makes a
    /// smithing table's shift-click evaluable, plus the stonecutter's list.
    ///
    /// **Play-state only.** The `update_tags` precedent (M69) is that a packet
    /// whose handler hangs off `ClientCommonPacketListener` has a configuration
    /// twin; this one hangs off `ClientGamePacketListener`
    /// (`ClientboundUpdateRecipesPacket.java:17`) and the report lists it under
    /// clientbound-play alone, so there is no second copy to miss.
    pub cb_play_update_recipes: i32,
    /// `ClientboundSelectAdvancementsTabPacket` (M177) — one **nullable**
    /// identifier. The client resolves it against its tree: null, or an id the
    /// tree does not know, both clear the selection (`get` returning null
    /// feeds `setSelectedTab(null, false)`).
    pub cb_play_select_advancements_tab: i32,
    /// `ClientboundUpdateAdvancementsPacket` (M177) — the advancement tree's
    /// whole feed: reset bool, added holders, removed ids, a progress map and
    /// a trailing show-advancements bool. See [`crate::advancements`].
    pub cb_play_update_advancements: i32,
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
    /// `ClientboundDeleteChatPacket` (M108) — a single `MessageSignature.Packed`
    /// and nothing else. **`Packed.read` is `readVarInt() - 1`**, so wire `0`
    /// means 256 inline bytes and anything else is an index into the client's
    /// own `MessageSignatureCache`. That is why this packet could not be
    /// handled before there was a cache to index: see [`crate::chat_wire`].
    pub cb_play_delete_chat: i32,
    /// `ClientboundCommandsPacket` (M113) — the Brigadier command tree.
    ///
    /// A counted list of nodes then the root index. **An argument node's
    /// properties have no length prefix and only its own type knows their
    /// size**, so an unrecognised `command_argument_type` id makes the rest of
    /// the packet unreadable — vanilla's own reader bails there too. See
    /// [`crate::commands`].
    pub cb_play_commands: i32,
    /// `ClientboundCommandSuggestionsPacket` (M114) — the autocomplete reply
    /// to a request the client made.
    ///
    /// `toSuggestions` builds its list with the **constructor**, not
    /// `Suggestions.create`, so the server's order is displayed verbatim and
    /// duplicates are not removed. See [`crate::suggestion_wire`].
    pub cb_play_command_suggestions: i32,
    /// `ClientboundCustomChatCompletionsPacket` (M114) — the server's own
    /// completion words, merged with the online-player names into the same
    /// popup. `SET` clears before it adds; an out-of-range action ordinal is
    /// a decode error, because `readEnum` indexes an array.
    pub cb_play_custom_chat_completions: i32,
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
    /// `ClientboundPlayerCombatKillPacket` (M82) — the death screen. The third
    /// of the `player_combat_*` trio and the only one that does anything:
    /// a VarInt `playerId` and a `TRUSTED_STREAM_CODEC` death message.
    ///
    /// The id it carries is **always your own** — `ServerPlayer.die` sends it
    /// through `this.connection`, never to trackers — which is why its handler
    /// needs [`crate::PlaySession`]'s local-player door rather than the entity
    /// table (see `REWO_PLAN.md` §0.0 gotcha 13).
    pub cb_play_player_combat_kill: i32,
    /// `ClientboundAwardStatsPacket` (M84) — the statistics screen.
    ///
    /// A `ByteBufCodecs.map` of `Stat<?>` to VarInt, and the key is the
    /// project's first **two-level** dispatch: a `minecraft:stat_type` id, then
    /// a value id in *that type's own* registry. Both levels are
    /// `ByteBufCodecs.registry`, i.e. a raw VarInt — see `rewo_data::stats` for
    /// why that makes the whole packet walkable whatever the type turns out to
    /// be.
    ///
    /// Addressed to **you**: `handleAwardStats` writes straight into
    /// `minecraft.player.getStats()` and the body carries no entity id at all,
    /// so there is nothing here to look up in a table (gotcha 13).
    pub cb_play_award_stats: i32,
    /// `ClientboundServerDataPacket` — the MOTD `Component` and an
    /// `Optional<byte[]>` icon.
    pub cb_play_server_data: i32,
    /// `ClientboundStoreCookiePacket` in the **play** state — the other half of
    /// the loop `cookie_request` already answers. Before M78 the reply was
    /// always "no payload", because nothing ever stored one.
    pub cb_play_store_cookie: i32,

    // ── M80: the world border ─────────────────────────────────────────────
    // Six packets, one feature. All `req!` on the same argument as above: a
    // server with no border set simply never sends the five mutators, but it
    // always sends `initialize_border` on join, and a report missing any of the
    // six is a version mismatch rather than a runtime state.
    /// `ClientboundInitializeBorderPacket` — the whole border at once: four
    /// `f64` (centre, current size, lerp target), a **var-long** remaining lerp
    /// time, then three VarInts (absolute max size, warning blocks, warning
    /// time). Its `lerpTime > 0` guard is the one the mutator below lacks.
    pub cb_play_initialize_border: i32,
    /// `ClientboundSetBorderCenterPacket` — two `f64`. Moves the box without
    /// touching the size or a running lerp.
    pub cb_play_set_border_center: i32,
    /// `ClientboundSetBorderLerpSizePacket` — two `f64` and a var-long.
    /// `handleSetBorderLerpSize` calls `lerpSizeBetween` **unguarded**, so a
    /// zero duration here builds a moving extent where `initialize_border`
    /// would have built a static one.
    pub cb_play_set_border_lerp_size: i32,
    /// `ClientboundSetBorderSizePacket` — one `f64`, and `setSize` replaces the
    /// extent object, so this **cancels** an in-flight lerp rather than
    /// retargeting it.
    pub cb_play_set_border_size: i32,
    /// `ClientboundSetBorderWarningDelayPacket` — one VarInt, written to
    /// `warningTime`. In **ticks** since 26.x moved the field off seconds
    /// (`WorldBorderWarningTimeFix` multiplies old saves by 20).
    pub cb_play_set_border_warning_delay: i32,
    /// `ClientboundSetBorderWarningDistancePacket` — one VarInt, written to
    /// `warningBlocks`. Same body as its sibling above; only the id separates
    /// them, which is why [`crate::border`] dispatches on a kind rather than
    /// on the shape of the body.
    pub cb_play_set_border_warning_distance: i32,
    /// `ClientboundSetTitleTextPacket` — a trusted `Component`, and the **only**
    /// one of the title trio that arms `Hud.titleTime` (M79). See
    /// [`crate::hud_state`].
    pub cb_play_set_title_text: i32,
    /// `ClientboundSetSubtitleTextPacket` — the same one-NBT-tag body as the
    /// title and the action bar, so the **id is the only discriminator**.
    pub cb_play_set_subtitle_text: i32,
    /// `ClientboundSetActionBarTextPacket` — `setOverlayMessage(text, false)`,
    /// a fixed 60-tick clock unrelated to the title times.
    pub cb_play_set_action_bar_text: i32,
    /// `ClientboundSetTitlesAnimationPacket` — three **fixed big-endian i32s**,
    /// each of which is a no-op when negative.
    pub cb_play_set_titles_animation: i32,
    /// `ClientboundClearTitlesPacket` — one boolean, and it does something the
    /// clear itself does not (restore 10 / 70 / 20).
    pub cb_play_clear_titles: i32,
    /// `ClientboundSetExperiencePacket` — float progress, then **level**, then
    /// total. The declaration order is progress / total / level, which is not
    /// the wire order.
    pub cb_play_set_experience: i32,
    /// `ClientboundCooldownPacket` — an `Identifier` cooldown *group* and a
    /// VarInt duration in ticks, where `0` is a removal rather than a
    /// zero-length cooldown.
    pub cb_play_cooldown: i32,
    /// `ClientboundTrackedWaypointPacket` — the locator bar (M83). An
    /// operation whose id **wraps** rather than being rejected, then a
    /// `TrackedWaypoint` whose body shape depends on a type tag whose id is
    /// rejected. See [`crate::waypoints`].
    pub cb_play_waypoint: i32,
    /// `ClientboundServerLinksPacket` in the **play** state (M85) — the
    /// `/reload`-shaped copy, sent when a server changes its links mid-session.
    /// See [`cb_config_server_links`](Self::cb_config_server_links) for the one
    /// that actually fires on a join.
    pub cb_play_server_links: i32,
    /// `ClientboundResourcePackPushPacket` in the **play** state (M166) — a
    /// pack pushed mid-session by `/resourcepack` or a plugin.
    ///
    /// It blocks nothing (there is no configuration queue in play), so unlike
    /// its configuration twin it is not a hang. It is answered anyway because
    /// `handleResourcePackPush` is on the *common* listener — one handler, two
    /// state ids, the same shape as the brand / cookie / server-links trio
    /// `crate::session` already unifies.
    pub cb_play_resource_pack_push: i32,
    /// `ServerboundResourcePackPacket` in the **play** state (M166).
    pub sb_play_resource_pack: i32,
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
            sb_config_resource_pack: req!(p, Cfg, S, "resource_pack"),
            sb_config_accept_code_of_conduct: req!(p, Cfg, S, "accept_code_of_conduct"),
            cb_config_keep_alive: req!(p, Cfg, C, "keep_alive"),
            cb_config_ping: req!(p, Cfg, C, "ping"),
            cb_config_select_known_packs: req!(p, Cfg, C, "select_known_packs"),
            cb_config_registry_data: req!(p, Cfg, C, "registry_data"),
            cb_config_resource_pack_push: req!(p, Cfg, C, "resource_pack_push"),
            cb_config_code_of_conduct: req!(p, Cfg, C, "code_of_conduct"),
            cb_config_update_tags: req!(p, Cfg, C, "update_tags"),
            cb_config_finish: req!(p, Cfg, C, "finish_configuration"),
            cb_config_cookie_request: opt!(p, Cfg, C, "cookie_request"),
            cb_config_custom_payload: req!(p, Cfg, C, "custom_payload"),
            cb_config_store_cookie: req!(p, Cfg, C, "store_cookie"),
            cb_config_server_links: req!(p, Cfg, C, "server_links"),
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
            cb_play_hurt_animation: req!(p, P, C, "hurt_animation"),
            cb_play_block_destruction: req!(p, P, C, "block_destruction"),
            cb_play_take_item_entity: req!(p, P, C, "take_item_entity"),
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
            sb_play_command_suggestion: opt!(p, P, S, "command_suggestion"),
            sb_play_chat_session_update: opt!(p, P, S, "chat_session_update"),
            sb_play_set_creative_slot: opt!(p, P, S, "set_creative_mode_slot"),
            sb_play_set_carried_item: opt!(p, P, S, "set_carried_item"),
            sb_play_player_action: req!(p, P, S, "player_action"),
            sb_play_use_item_on: opt!(p, P, S, "use_item_on"),
            sb_play_interact: opt!(p, P, S, "interact"),
            sb_play_swing: opt!(p, P, S, "swing"),
            sb_play_client_command: opt!(p, P, S, "client_command"),
            sb_play_player_command: opt!(p, P, S, "player_command"),
            sb_play_container_click: opt!(p, P, S, "container_click"),
            sb_play_container_button_click: opt!(p, P, S, "container_button_click"),
            sb_play_recipe_book_change_settings: opt!(p, P, S, "recipe_book_change_settings"),
            sb_play_place_recipe: opt!(p, P, S, "place_recipe"),
            sb_play_seen_advancements: opt!(p, P, S, "seen_advancements"),
            sb_play_container_slot_state_changed: opt!(p, P, S, "container_slot_state_changed"),
            sb_play_set_beacon: opt!(p, P, S, "set_beacon"),
            sb_play_rename_item: opt!(p, P, S, "rename_item"),
            sb_play_sign_update: opt!(p, P, S, "sign_update"),
            sb_play_select_trade: opt!(p, P, S, "select_trade"),
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
            cb_play_open_screen: req!(p, P, C, "open_screen"),
            cb_play_container_set_data: req!(p, P, C, "container_set_data"),
            cb_play_merchant_offers: req!(p, P, C, "merchant_offers"),
            cb_play_open_book: req!(p, P, C, "open_book"),
            cb_play_open_sign_editor: req!(p, P, C, "open_sign_editor"),
            cb_play_recipe_book_add: req!(p, P, C, "recipe_book_add"),
            cb_play_recipe_book_remove: req!(p, P, C, "recipe_book_remove"),
            cb_play_recipe_book_settings: req!(p, P, C, "recipe_book_settings"),
            cb_play_place_ghost_recipe: req!(p, P, C, "place_ghost_recipe"),
            cb_play_update_recipes: req!(p, P, C, "update_recipes"),
            cb_play_select_advancements_tab: req!(p, P, C, "select_advancements_tab"),
            cb_play_update_advancements: req!(p, P, C, "update_advancements"),
            cb_play_set_camera: req!(p, P, C, "set_camera"),
            cb_play_ticking_state: req!(p, P, C, "ticking_state"),
            cb_play_ticking_step: req!(p, P, C, "ticking_step"),
            cb_play_move_minecart_along_track: req!(p, P, C, "move_minecart_along_track"),
            cb_play_set_entity_link: req!(p, P, C, "set_entity_link"),
            cb_play_projectile_power: req!(p, P, C, "projectile_power"),
            cb_play_bundle_delimiter: req!(p, P, C, "bundle_delimiter"),
            cb_play_custom_payload: req!(p, P, C, "custom_payload"),
            cb_play_disguised_chat: req!(p, P, C, "disguised_chat"),
            cb_play_delete_chat: req!(p, P, C, "delete_chat"),
            cb_play_commands: req!(p, P, C, "commands"),
            cb_play_command_suggestions: req!(p, P, C, "command_suggestions"),
            cb_play_custom_chat_completions: req!(p, P, C, "custom_chat_completions"),
            cb_play_game_rule_values: req!(p, P, C, "game_rule_values"),
            cb_play_player_combat_end: req!(p, P, C, "player_combat_end"),
            cb_play_player_combat_enter: req!(p, P, C, "player_combat_enter"),
            cb_play_player_combat_kill: req!(p, P, C, "player_combat_kill"),
            cb_play_award_stats: req!(p, P, C, "award_stats"),
            cb_play_server_data: req!(p, P, C, "server_data"),
            cb_play_store_cookie: req!(p, P, C, "store_cookie"),
            cb_play_player_rotation: req!(p, P, C, "player_rotation"),
            cb_play_player_look_at: req!(p, P, C, "player_look_at"),
            cb_play_set_default_spawn_position: req!(p, P, C, "set_default_spawn_position"),
            cb_play_initialize_border: req!(p, P, C, "initialize_border"),
            cb_play_set_border_center: req!(p, P, C, "set_border_center"),
            cb_play_set_border_lerp_size: req!(p, P, C, "set_border_lerp_size"),
            cb_play_set_border_size: req!(p, P, C, "set_border_size"),
            cb_play_set_border_warning_delay: req!(p, P, C, "set_border_warning_delay"),
            cb_play_set_border_warning_distance: req!(p, P, C, "set_border_warning_distance"),
            cb_play_set_title_text: req!(p, P, C, "set_title_text"),
            cb_play_set_subtitle_text: req!(p, P, C, "set_subtitle_text"),
            cb_play_set_action_bar_text: req!(p, P, C, "set_action_bar_text"),
            cb_play_set_titles_animation: req!(p, P, C, "set_titles_animation"),
            cb_play_clear_titles: req!(p, P, C, "clear_titles"),
            cb_play_set_experience: req!(p, P, C, "set_experience"),
            cb_play_cooldown: req!(p, P, C, "cooldown"),
            cb_play_waypoint: req!(p, P, C, "waypoint"),
            cb_play_server_links: req!(p, P, C, "server_links"),
            cb_play_resource_pack_push: req!(p, P, C, "resource_pack_push"),
            sb_play_resource_pack: req!(p, P, S, "resource_pack"),
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
