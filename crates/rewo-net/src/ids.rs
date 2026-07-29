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
            cb_play_rotate_head: req!(p, P, C, "rotate_head"),
            cb_play_entity_event: req!(p, P, C, "entity_event"),
            cb_play_animate: req!(p, P, C, "animate"),
            cb_play_damage_event: req!(p, P, C, "damage_event"),
            cb_play_game_event: req!(p, P, C, "game_event"),
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
        })
    }
}
