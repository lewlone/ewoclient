//! registries.json → item-id table. Item ids are distinct from block-state
//! ids; the network item stack carries this id. Parsed from the
//! `minecraft:item` registry in the datagen report.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

pub struct Items {
    by_name: HashMap<String, i32>,
}

impl Items {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:item")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:item registry")?;
        let mut by_name = HashMap::with_capacity(entries.len());
        for (name, entry) in entries {
            if let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) {
                by_name.insert(name.clone(), id as i32);
            }
        }
        log::info!("rewo-data: {} items", by_name.len());
        Ok(Self { by_name })
    }

    /// Item id by name; accepts the bare name ("dirt").
    pub fn id(&self, name: &str) -> Option<i32> {
        let full = if name.contains(':') {
            name.to_string()
        } else {
            format!("minecraft:{name}")
        };
        self.by_name.get(&full).copied()
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}
