//! registries.json → item-id table. Item ids are distinct from block-state
//! ids; the network item stack carries this id. Parsed from the
//! `minecraft:item` registry in the datagen report.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

#[derive(Clone)]
pub struct Items {
    by_name: HashMap<String, i32>,
    /// Every registered protocol id. A held-item id outside this set is not a
    /// vanilla item, and nothing may claim to know its components.
    ids: std::collections::HashSet<i32>,
    /// Lazily-built reverse map (see [`Items::name`]).
    by_id: std::sync::OnceLock<HashMap<i32, String>>,
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
        let mut ids = std::collections::HashSet::with_capacity(entries.len());
        for (name, entry) in entries {
            if let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) {
                by_name.insert(name.clone(), id as i32);
                ids.insert(id as i32);
            }
        }
        log::info!("rewo-data: {} items", by_name.len());
        Ok(Self { by_name, ids, by_id: Default::default() })
    }

    /// Whether `id` is a registered item. The item-stack decoder needs this:
    /// an id outside the registry is not a vanilla item, so its default
    /// components — including `minecraft:swing_animation` — are unknowable, and
    /// nothing may substitute the bare default for it.
    /// Every item name, sorted (M119).
    ///
    /// Built on demand rather than stored: the only caller is the command
    /// suggester, which runs on a keystroke and not per frame, and 1,537
    /// strings is not worth a second copy of the registry.
    pub fn names(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.by_name.keys().map(String::as_str).collect();
        out.sort_unstable();
        out
    }

    pub fn has_id(&self, id: i32) -> bool {
        self.ids.contains(&id)
    }

    /// A copy of the registered-id set, for tables that must answer
    /// "is this a vanilla item" without holding a borrow.
    pub fn id_set(&self) -> std::collections::HashSet<i32> {
        self.ids.clone()
    }

    /// Item id by name; accepts the bare name ("dirt").
    /// Registry name of `id` (M22 — held-item models are keyed by name).
    /// Built lazily on first use: the reverse map is only needed by the render
    /// path, and the vast majority of runs never hold anything.
    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id
            .get_or_init(|| {
                self.by_name
                    .iter()
                    .map(|(n, i)| (*i, n.clone()))
                    .collect()
            })
            .get(&id)
            .map(String::as_str)
    }

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
