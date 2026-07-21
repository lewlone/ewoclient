//! packets.json → protocol-id tables, keyed by (state, direction, name).
//!
//! Resolving ids by *name* at runtime (rather than hardcoding integers) is
//! the version-churn strategy from REWO_PLAN.md §11: when the pin moves and
//! ids shuffle, the report changes and the net layer keeps working —
//! `require()` fails loud only if a packet we depend on vanished.
//!
//! Report shape:
//! ```json
//! "play": { "clientbound": { "minecraft:level_chunk_with_light": { "protocol_id": 45 } } }
//! ```

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum State {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

impl State {
    fn key(self) -> &'static str {
        match self {
            State::Handshake => "handshake",
            State::Status => "status",
            State::Login => "login",
            State::Configuration => "configuration",
            State::Play => "play",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Dir {
    Clientbound,
    Serverbound,
}

impl Dir {
    fn key(self) -> &'static str {
        match self {
            Dir::Clientbound => "clientbound",
            Dir::Serverbound => "serverbound",
        }
    }
}

pub struct Packets {
    // (state, dir) → { name → id } and the id → name reverse for logging.
    forward: HashMap<(State, Dir), HashMap<String, i32>>,
    reverse: HashMap<(State, Dir), HashMap<i32, String>>,
}

impl Packets {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let obj = json.as_object().ok_or("packets.json: root not object")?;
        let mut forward = HashMap::new();
        let mut reverse = HashMap::new();

        for state in [
            State::Handshake,
            State::Status,
            State::Login,
            State::Configuration,
            State::Play,
        ] {
            let Some(state_obj) = obj.get(state.key()).and_then(|v| v.as_object()) else {
                continue;
            };
            for dir in [Dir::Clientbound, Dir::Serverbound] {
                let Some(dir_obj) = state_obj.get(dir.key()).and_then(|v| v.as_object()) else {
                    continue;
                };
                let mut names = HashMap::new();
                let mut ids = HashMap::new();
                for (name, entry) in dir_obj {
                    let id = entry
                        .get("protocol_id")
                        .and_then(|i| i.as_i64())
                        .ok_or_else(|| format!("packets.json: {name} missing protocol_id"))?
                        as i32;
                    names.insert(name.clone(), id);
                    ids.insert(id, name.clone());
                }
                forward.insert((state, dir), names);
                reverse.insert((state, dir), ids);
            }
        }
        Ok(Self { forward, reverse })
    }

    /// Resolve an id, accepting the bare name ("keep_alive") — the
    /// "minecraft:" prefix is added automatically.
    pub fn id(&self, state: State, dir: Dir, name: &str) -> Option<i32> {
        let full = if name.contains(':') {
            name.to_string()
        } else {
            format!("minecraft:{name}")
        };
        self.forward.get(&(state, dir))?.get(&full).copied()
    }

    /// Like [`id`], but returns a descriptive error naming the packet — used
    /// at connection setup so a version drift fails loud, not mid-stream.
    pub fn require(&self, state: State, dir: Dir, name: &str) -> Result<i32, String> {
        self.id(state, dir, name).ok_or_else(|| {
            format!("packets.json: required packet {:?}/{:?} {name} not found", state, dir)
        })
    }

    pub fn name(&self, state: State, dir: Dir, id: i32) -> Option<&str> {
        self.reverse.get(&(state, dir))?.get(&id).map(|s| s.as_str())
    }
}
