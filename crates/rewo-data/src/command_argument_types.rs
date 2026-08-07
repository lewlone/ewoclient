//! registries.json → the `command_argument_type` table (M113).
//!
//! `ClientboundCommandsPacket` names each argument node's type by a protocol
//! id, and then reads **that type's own properties**, whose length only the
//! type knows. So this table is not a convenience: without it the tree cannot
//! be read past its first non-singleton argument. See
//! [`rewo_net::commands`] for what happens when an id is unknown (an error —
//! and vanilla itself bails there too).
//!
//! `minecraft:command_argument_type` is a **built-in** registry: 57 entries
//! baked into the jar rather than sent by the server, so the report is the
//! ground truth and each entry's `protocol_id` **is** the wire value. The
//! alphabetisation trap M64 records applies here in full — `serde_json`'s
//! default `Map` is a sorted `BTreeMap`, and `brigadier:bool` happening to
//! sort first is a coincidence, not a rule. Every id is read off
//! `protocol_id` and none is derived from position.
//!
//! Ground truth: `<data_dir>/rewo/26.2/datagen/generated/reports/registries.json`,
//! key `minecraft:command_argument_type`.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

/// The `minecraft:command_argument_type` registry: protocol id → name.
#[derive(Clone, Default)]
pub struct CommandArgumentTypes {
    by_id: HashMap<i32, String>,
}

impl CommandArgumentTypes {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:command_argument_type")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:command_argument_type registry")?;
        let mut by_id = HashMap::with_capacity(entries.len());
        for (name, entry) in entries {
            if let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) {
                by_id.insert(id as i32, name.clone());
            }
        }
        log::info!("rewo-data: {} command argument types", by_id.len());
        Ok(Self { by_id })
    }

    /// Registry name for a protocol id.
    ///
    /// `None` is a real state — a server on a newer protocol can name a type
    /// this jar has never registered — and the caller must treat it as fatal
    /// for the packet rather than skipping the node, because the properties
    /// that follow have no length.
    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture in the shape of the real registry's head, written to disk and
    /// read back through the production `load` so these exercise the real
    /// parse. The ids and names are the ones verified against the 26.2 report.
    fn fixture(dir_name: &str) -> CommandArgumentTypes {
        let dir = std::env::temp_dir().join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("registries.json");
        std::fs::write(
            &p,
            r#"{"minecraft:command_argument_type":{"entries":{
                 "brigadier:bool":{"protocol_id":0},
                 "brigadier:float":{"protocol_id":1},
                 "brigadier:double":{"protocol_id":2},
                 "brigadier:integer":{"protocol_id":3},
                 "brigadier:long":{"protocol_id":4},
                 "brigadier:string":{"protocol_id":5},
                 "minecraft:entity":{"protocol_id":6},
                 "minecraft:angle":{"protocol_id":19},
                 "minecraft:score_holder":{"protocol_id":31},
                 "minecraft:time":{"protocol_id":43},
                 "minecraft:resource_or_tag":{"protocol_id":44},
                 "minecraft:resource_or_tag_key":{"protocol_id":45},
                 "minecraft:resource":{"protocol_id":46},
                 "minecraft:resource_key":{"protocol_id":47},
                 "minecraft:resource_selector":{"protocol_id":48}}}}"#,
        )
        .unwrap();
        CommandArgumentTypes::load(&p).unwrap()
    }

    /// The ids are the report's `protocol_id`s and **not** the alphabetical
    /// position — M64's trap, which here would read one argument type's
    /// properties as another's and desynchronise the whole tree.
    #[test]
    fn the_ids_are_protocol_ids_and_not_alphabetical() {
        let t = fixture("rewo_cat_alpha");
        // Declaration order: bool, float, double — where SORTING puts `double`
        // before `float`, so an `enumerate()`-built table would swap 1 and 2
        // and read a f64 range as a f32 one.
        assert_eq!(t.name(1), Some("brigadier:float"));
        assert_eq!(t.name(2), Some("brigadier:double"));
        // And `minecraft:angle` really does sort before `minecraft:entity`
        // while sitting thirteen ids later.
        assert_eq!(t.name(6), Some("minecraft:entity"));
        assert_eq!(t.name(19), Some("minecraft:angle"));
    }

    /// **The names are namespaced.** `ArgumentTypeInfos.register` passes a bare
    /// string and `Identifier` fills in `minecraft:`, so five types are
    /// `brigadier:*` and the rest are `minecraft:*`. A decoder matching the
    /// bare name compiles, falls through to the singleton arm, and reads zero
    /// bytes where the type has properties.
    #[test]
    fn the_names_carry_their_namespace() {
        let t = fixture("rewo_cat_ns");
        assert_eq!(t.name(6), Some("minecraft:entity"));
        assert_ne!(t.name(6), Some("entity"));
        assert_eq!(t.name(5), Some("brigadier:string"));
    }

    /// Every type the decoder special-cases must be registered, or its
    /// properties stop being read and the tree desynchronises.
    #[test]
    fn every_type_with_properties_is_registered() {
        let t = fixture("rewo_cat_props");
        let names: Vec<&str> = (0..64).filter_map(|i| t.name(i)).collect();
        for want in [
            "brigadier:float",
            "brigadier:double",
            "brigadier:integer",
            "brigadier:long",
            "brigadier:string",
            "minecraft:entity",
            "minecraft:score_holder",
            "minecraft:time",
            "minecraft:resource_or_tag",
            "minecraft:resource_or_tag_key",
            "minecraft:resource",
            "minecraft:resource_key",
            "minecraft:resource_selector",
        ] {
            assert!(names.contains(&want), "{want} is not registered");
        }
    }

    #[test]
    fn an_unregistered_id_is_none() {
        let t = fixture("rewo_cat_none");
        // `None` is fatal for the packet at the caller, not a skip — the
        // properties that follow have no length.
        assert_eq!(t.name(-1), None);
        assert_eq!(t.name(10_000), None);
    }
}
