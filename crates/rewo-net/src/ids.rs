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
    pub cb_play_rotate_head: i32,
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
    pub cb_play_set_health: Option<i32>,
    pub cb_play_system_chat: Option<i32>,
    pub cb_play_player_chat: Option<i32>,
    pub cb_play_block_ack: Option<i32>,
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
            cb_play_rotate_head: req!(p, P, C, "rotate_head"),
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
            cb_play_set_health: opt!(p, P, C, "set_health"),
            cb_play_system_chat: opt!(p, P, C, "system_chat"),
            cb_play_player_chat: opt!(p, P, C, "player_chat"),
            cb_play_block_ack: opt!(p, P, C, "block_changed_ack"),
        })
    }
}
