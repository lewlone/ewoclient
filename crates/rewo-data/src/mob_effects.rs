//! registries.json → the `minecraft:mob_effect` id table (M92c).
//!
//! # Why this module exists: five ids that were never resolved
//!
//! Rewo captured `night_vision`, `darkness`, `haste`, `conduit_power` and
//! `mining_fatigue` **from the Configuration `registry_data` packet** — inside
//! a `registry == "minecraft:mob_effect"` branch in `rewo_net`'s config
//! handling. That branch cannot fire against a vanilla server.
//! `registry_data` carries only `RegistryDataLoader.SYNCHRONIZED_REGISTRIES`,
//! and `Registries.MOB_EFFECT` appears **zero times** in that file: it is a
//! `BuiltInRegistries` entry, baked into the jar, and the server never sends
//! it. So all five stayed `None` for the whole session, and:
//!
//! * M13's night-vision and darkness lightmap terms never engaged live, and
//! * M19's `getCurrentSwingDuration` dig-speed / mining-fatigue adjustment
//!   never engaged live.
//!
//! **No gate could see it**, and the reason is worth keeping: `lightmapshot`
//! and `swingshot` are serverless and *construct* the effect state themselves,
//! supplying the very ids the live path fails to obtain. That is M86's shape
//! exactly — a path the gates never take because the gates build the state
//! around it.
//!
//! The rule this module restores is already written down in
//! [`crate::attributes`] and [`crate::sound_events`]: a **built-in** registry
//! is resolved **by name from the report**, so a renumber between versions
//! fails loud rather than silently pointing at a neighbour. `mob_effect` is
//! one, and was the only one reading the wire for it.
//!
//! # Fail loud, and only for what is asked for
//!
//! [`MobEffects::load`] refuses to build if the registry is missing or any id
//! is malformed, but it does **not** demand a fixed set of names: the table is
//! the whole registry and callers ask for what they need. A caller that needs
//! a specific effect and gets `None` has a version mismatch worth reporting at
//! its own call site, where the consequence is known.
//!
//! Ground truth: `<data_dir>/rewo/26.2/datagen/generated/reports/registries.json`,
//! key `minecraft:mob_effect`.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

/// The `minecraft:mob_effect` registry: protocol id ↔ registry name.
#[derive(Clone, Default)]
pub struct MobEffects {
    by_id: HashMap<i32, String>,
    by_name: HashMap<String, i32>,
}

impl MobEffects {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:mob_effect")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:mob_effect registry")?;
        let mut by_id = HashMap::with_capacity(entries.len());
        let mut by_name = HashMap::with_capacity(entries.len());
        for (name, entry) in entries {
            // `protocol_id`, never the iteration index — M64's alphabetisation
            // trap. `serde_json`'s default map is sorted, so an `enumerate()`
            // here would give a different wrong id for every effect and no
            // decode gate could see it.
            let id = entry
                .get("protocol_id")
                .and_then(|i| i.as_i64())
                .ok_or_else(|| format!("registries.json: {name} has no protocol_id"))?;
            by_id.insert(id as i32, name.clone());
            by_name.insert(name.clone(), id as i32);
        }
        if by_id.is_empty() {
            return Err("registries.json: minecraft:mob_effect is empty".into());
        }
        log::info!("rewo-data: {} mob effects", by_id.len());
        Ok(Self { by_id, by_name })
    }

    /// Registry name for a protocol id.
    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(|s| s.as_str())
    }

    /// Protocol id for a registry name, e.g. `"minecraft:night_vision"`.
    ///
    /// `None` for a name this version does not register — a real state that a
    /// caller must not paper over with a default, for the same reason
    /// [`crate::sound_events::SoundEvents::name`] says so: a substituted
    /// effect is harder to notice than a missing one.
    pub fn id_of(&self, name: &str) -> Option<i32> {
        self.by_name.get(name).copied()
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

    fn report() -> Option<MobEffects> {
        let paths = crate::DataPaths::for_version("26.2")?;
        MobEffects::load(&paths.registries_json()).ok()
    }

    #[test]
    fn the_five_effects_the_live_path_needed_all_resolve() {
        // The specific regression: these are the names `rewo_net`'s config
        // handler looked for in a registry the server never sends.
        let Some(m) = report() else { return };
        for name in [
            "minecraft:night_vision",
            "minecraft:darkness",
            "minecraft:haste",
            "minecraft:conduit_power",
            "minecraft:mining_fatigue",
        ] {
            assert!(m.id_of(name).is_some(), "{name} must resolve");
        }
    }

    #[test]
    fn the_ids_are_the_reports_own_and_not_alphabetical_positions() {
        // M64's trap. `speed` is protocol id 0 in 26.2, while alphabetically
        // it sits deep in the list — so a table built by `enumerate()` would
        // give it a large id and nothing would fail.
        let Some(m) = report() else { return };
        assert_eq!(m.id_of("minecraft:speed"), Some(0));
        let alphabetical: Vec<&str> = {
            let mut v: Vec<&str> = (0..m.len() as i32).filter_map(|i| m.name(i)).collect();
            v.sort_unstable();
            v
        };
        assert_ne!(
            alphabetical.first().copied(),
            Some("minecraft:speed"),
            "if these ever agreed the witness would be vacuous"
        );
    }

    #[test]
    fn the_table_round_trips_both_ways() {
        let Some(m) = report() else { return };
        for id in 0..m.len() as i32 {
            if let Some(n) = m.name(id) {
                assert_eq!(m.id_of(n), Some(id));
            }
        }
    }

    #[test]
    fn the_six_beacon_effects_resolve() {
        // What M92's beacon needs, and the reason this module landed with it.
        let Some(m) = report() else { return };
        for name in [
            "minecraft:speed",
            "minecraft:haste",
            "minecraft:resistance",
            "minecraft:jump_boost",
            "minecraft:strength",
            "minecraft:regeneration",
        ] {
            assert!(m.id_of(name).is_some(), "{name}");
        }
    }
}
