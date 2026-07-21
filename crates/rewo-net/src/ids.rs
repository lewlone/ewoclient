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
    pub cb_play_chunk_batch_finished: i32,
    pub cb_play_level_chunk: i32,
    pub cb_play_forget_chunk: i32,
    pub cb_play_block_update: i32,
    pub cb_play_add_entity: i32,
    pub cb_play_remove_entities: i32,
    pub cb_play_start_configuration: Option<i32>,
    pub cb_play_cookie_request: Option<i32>,
    pub cb_play_disconnect: i32,
}

impl Ids {
    pub fn resolve(p: &Packets) -> Result<Self, String> {
        use Dir::{Clientbound as C, Serverbound as S};
        use State::{Configuration as Cfg, Login as L, Play as P};
        Ok(Self {
            sb_handshake_intention: req!(p, State::Handshake, S, "intention"),
            sb_login_hello: req!(p, L, S, "hello"),
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
            cb_play_chunk_batch_finished: req!(p, P, C, "chunk_batch_finished"),
            cb_play_level_chunk: req!(p, P, C, "level_chunk_with_light"),
            cb_play_forget_chunk: req!(p, P, C, "forget_level_chunk"),
            cb_play_block_update: req!(p, P, C, "block_update"),
            cb_play_add_entity: req!(p, P, C, "add_entity"),
            cb_play_remove_entities: req!(p, P, C, "remove_entities"),
            cb_play_start_configuration: opt!(p, P, C, "start_configuration"),
            cb_play_cookie_request: opt!(p, P, C, "cookie_request"),
            cb_play_disconnect: req!(p, P, C, "disconnect"),
        })
    }
}
