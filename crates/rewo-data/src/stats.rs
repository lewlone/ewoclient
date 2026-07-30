//! registries.json → the statistics tables (M84).
//!
//! `award_stats` (3) is keyed by a `Stat<?>`, and `Stat.STREAM_CODEC` is a
//! **two-level** dispatch:
//!
//! ```java
//! Stat.STREAM_CODEC = ByteBufCodecs.registry(Registries.STAT_TYPE)
//!    .dispatch(Stat::getType, StatType::streamCodec);
//! // and, per type:
//! this.streamCodec = ByteBufCodecs.registry(registry.key()).map(this::get, Stat::getValue);
//! ```
//!
//! # The dispatch is uniform, and that is the load-bearing finding
//!
//! A two-level dispatch reads like the `DataComponentPatch` hazard in
//! miniature — an untranscribed variant cannot be *skipped*, because the
//! reader parks mid-value and the rest of the packet is garbage. **Here it is
//! not**, and the reason is structural rather than lucky: every `StatType`'s
//! stream codec is built by the same one-line constructor, so *all nine* second
//! levels are `ByteBufCodecs.registry(...)` — a single VarInt. What the first
//! level selects is **which registry to look the id up in**, not a different
//! wire shape.
//!
//! So a stat type Rewo did not know about would still consume exactly one
//! VarInt and the walk would stay in step. There is nothing to be unwalkable:
//! `minecraft:stat_type` is a built-in registry, its nine entries are fixed at
//! compile time, and a server cannot add to it over the wire.
//!
//! [`ValueRegistry`] is therefore the whole of the dispatch, and it is a
//! **table keyed by the type's registry name**, transcribed from `Stats.java`'s
//! nine `makeRegistryStatType` calls. An unknown name resolves to `None` rather
//! than to a guess — a wrong registry would silently rename every value under
//! it.
//!
//! # Two other things `Stats.java` is the only source for
//!
//! * **The formatter is per-*stat*, not per-type**, and only the custom type
//!   ever gets a non-default one: `StatType.get(argument)` is
//!   `get(argument, StatFormatter.DEFAULT)`, and the four-way choice is made
//!   one `makeCustomStat` call at a time. [`custom_formatter`] is that table.
//! * `minecraft:custom_stat` is its own registry of 77 identifiers, which is
//!   what the custom type's second-level VarInt indexes.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

/// Which registry a stat type's value id indexes.
///
/// `Stats.java`'s nine `makeRegistryStatType(name, registry)` calls, and
/// nothing else. Note that five distinct types share `minecraft:item` and two
/// share `minecraft:entity_type` — the *type* is not the registry.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ValueRegistry {
    /// `mined` → `BuiltInRegistries.BLOCK`.
    Block,
    /// `crafted` / `used` / `broken` / `picked_up` / `dropped` → `ITEM`.
    Item,
    /// `killed` / `killed_by` → `ENTITY_TYPE`.
    EntityType,
    /// `custom` → `CUSTOM_STAT`.
    CustomStat,
}

/// The nine rows of `Stats.java`, keyed by the registry name the report gives.
pub fn value_registry(stat_type: &str) -> Option<ValueRegistry> {
    Some(match stat_type {
        "minecraft:mined" => ValueRegistry::Block,
        "minecraft:crafted"
        | "minecraft:used"
        | "minecraft:broken"
        | "minecraft:picked_up"
        | "minecraft:dropped" => ValueRegistry::Item,
        "minecraft:killed" | "minecraft:killed_by" => ValueRegistry::EntityType,
        "minecraft:custom" => ValueRegistry::CustomStat,
        _ => return None,
    })
}

/// `StatFormatter`'s four implementations, chosen per stat.
///
/// The formatting itself lives in `rewo_world::stats` — this crate names which
/// one a stat takes, the world crate applies it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Formatter {
    /// `NumberFormat.getIntegerInstance(Locale.US)::format`.
    #[default]
    Default,
    /// `DECIMAL_FORMAT.format(value * 0.1)`.
    DivideByTen,
    /// centimetres → km / m / cm.
    Distance,
    /// ticks → y / d / h / min / s.
    Time,
}

/// The non-`DEFAULT` custom stats, from `Stats.java`'s `makeCustomStat` calls.
///
/// Listed as the exceptions rather than all 77 rows because
/// `StatFormatter.DEFAULT` is both the majority and the fall-through every
/// other stat type takes, so an omission degrades to vanilla's own default
/// instead of to nothing.
const TIME_STATS: &[&str] = &[
    "minecraft:play_time",
    "minecraft:total_world_time",
    "minecraft:time_since_death",
    "minecraft:time_since_rest",
    // `CROUCH_TIME = makeCustomStat("sneak_time", TIME)` — the constant and the
    // registry name disagree, and the registry name is the one on the wire.
    "minecraft:sneak_time",
];

const DISTANCE_STATS: &[&str] = &[
    "minecraft:walk_one_cm",
    "minecraft:crouch_one_cm",
    "minecraft:sprint_one_cm",
    "minecraft:walk_on_water_one_cm",
    "minecraft:fall_one_cm",
    "minecraft:climb_one_cm",
    "minecraft:fly_one_cm",
    "minecraft:walk_under_water_one_cm",
    "minecraft:minecart_one_cm",
    "minecraft:boat_one_cm",
    "minecraft:pig_one_cm",
    "minecraft:happy_ghast_one_cm",
    "minecraft:horse_one_cm",
    "minecraft:aviate_one_cm",
    "minecraft:swim_one_cm",
    "minecraft:strider_one_cm",
    "minecraft:nautilus_one_cm",
];

const DIVIDE_BY_TEN_STATS: &[&str] = &[
    "minecraft:damage_dealt",
    "minecraft:damage_dealt_absorbed",
    "minecraft:damage_dealt_resisted",
    "minecraft:damage_taken",
    "minecraft:damage_blocked_by_shield",
    "minecraft:damage_absorbed",
    "minecraft:damage_resisted",
];

/// Which formatter a `minecraft:custom` stat takes, by its custom-stat name.
pub fn custom_formatter(custom_stat: &str) -> Formatter {
    if TIME_STATS.contains(&custom_stat) {
        Formatter::Time
    } else if DISTANCE_STATS.contains(&custom_stat) {
        Formatter::Distance
    } else if DIVIDE_BY_TEN_STATS.contains(&custom_stat) {
        Formatter::DivideByTen
    } else {
        Formatter::Default
    }
}

/// One registry's id ↔ name table.
#[derive(Clone, Default)]
pub struct IdTable {
    by_id: HashMap<i32, String>,
}

impl IdTable {
    fn load(json: &serde_json::Value, key: &str) -> Result<Self, String> {
        let entries = json
            .get(key)
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or_else(|| format!("registries.json: no {key} registry"))?;
        let mut by_id = HashMap::with_capacity(entries.len());
        for (name, entry) in entries {
            // `protocol_id`, never the iteration order — `serde_json`'s default
            // map is sorted, so `enumerate()` here would name every entry
            // wrong and still round-trip. That is M64's alphabetisation trap.
            if let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) {
                by_id.insert(id as i32, name.clone());
            }
        }
        Ok(Self { by_id })
    }

    /// A table from literal pairs, for fixtures.
    pub fn from_pairs(pairs: &[(i32, &str)]) -> Self {
        Self {
            by_id: pairs.iter().map(|(i, n)| (*i, n.to_string())).collect(),
        }
    }

    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(|s| s.as_str())
    }

    pub fn id_of(&self, name: &str) -> Option<i32> {
        self.by_id
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(id, _)| *id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Every `(id, name)` pair, for a screen that walks a whole registry.
    pub fn iter(&self) -> impl Iterator<Item = (i32, &str)> + '_ {
        self.by_id.iter().map(|(id, n)| (*id, n.as_str()))
    }
}

/// The three registries the statistics screen needs that no other table holds.
///
/// `minecraft:item` and `minecraft:entity_type` are deliberately absent:
/// [`crate::items::Items`] and [`crate::entity_types::EntityTypes`] already
/// carry them, and a second copy is a second thing to drift.
#[derive(Clone, Default)]
pub struct StatRegistries {
    /// The nine `minecraft:stat_type` entries — the dispatch's first level.
    pub stat_type: IdTable,
    /// The 77 `minecraft:custom_stat` entries.
    pub custom_stat: IdTable,
    /// `minecraft:block`, the `mined` type's value registry. This is the
    /// **block** registry, not the block-*state* table `crate::blocks` holds.
    pub block: IdTable,
}

impl StatRegistries {
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::from_value(&read_json_file(path)?)
    }

    /// The three tables from literal pairs, for fixtures.
    pub fn from_pairs(
        stat_type: &[(i32, &str)],
        custom_stat: &[(i32, &str)],
        block: &[(i32, &str)],
    ) -> Self {
        Self {
            stat_type: IdTable::from_pairs(stat_type),
            custom_stat: IdTable::from_pairs(custom_stat),
            block: IdTable::from_pairs(block),
        }
    }

    /// The same three tables from an already-parsed report, so a gate or a
    /// test can hand over a fixture without a file.
    pub fn from_value(json: &serde_json::Value) -> Result<Self, String> {
        let out = Self {
            stat_type: IdTable::load(json, "minecraft:stat_type")?,
            custom_stat: IdTable::load(json, "minecraft:custom_stat")?,
            block: IdTable::load(json, "minecraft:block")?,
        };
        log::info!(
            "rewo-data: {} stat types, {} custom stats, {} blocks",
            out.stat_type.len(),
            out.custom_stat.len(),
            out.block.len()
        );
        Ok(out)
    }

    /// Which registry the second VarInt of a stat with this type id indexes.
    pub fn value_registry_of(&self, stat_type_id: i32) -> Option<ValueRegistry> {
        value_registry(self.stat_type.name(stat_type_id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stat_type_name_maps_to_a_value_registry() {
        // The nine, spelled out rather than read from the report — a rename
        // has to fail here and not resolve to `None` quietly.
        for (name, want) in [
            ("minecraft:mined", ValueRegistry::Block),
            ("minecraft:crafted", ValueRegistry::Item),
            ("minecraft:used", ValueRegistry::Item),
            ("minecraft:broken", ValueRegistry::Item),
            ("minecraft:picked_up", ValueRegistry::Item),
            ("minecraft:dropped", ValueRegistry::Item),
            ("minecraft:killed", ValueRegistry::EntityType),
            ("minecraft:killed_by", ValueRegistry::EntityType),
            ("minecraft:custom", ValueRegistry::CustomStat),
        ] {
            assert_eq!(value_registry(name), Some(want), "{name}");
        }
        assert_eq!(value_registry("minecraft:not_a_stat_type"), None);
        // The bare name without the namespace is not the registry name.
        assert_eq!(value_registry("mined"), None);
    }

    #[test]
    fn the_formatter_table_is_the_three_exception_lists_and_nothing_else() {
        assert_eq!(custom_formatter("minecraft:play_time"), Formatter::Time);
        assert_eq!(
            custom_formatter("minecraft:walk_one_cm"),
            Formatter::Distance
        );
        assert_eq!(
            custom_formatter("minecraft:damage_dealt"),
            Formatter::DivideByTen
        );
        assert_eq!(custom_formatter("minecraft:jump"), Formatter::Default);
        // An unknown custom stat degrades to vanilla's own default rather than
        // to nothing.
        assert_eq!(custom_formatter("minecraft:invented"), Formatter::Default);
    }

    /// `CROUCH_TIME = makeCustomStat("sneak_time", TIME)`. Reading the Java
    /// constant rather than the string literal names a stat that does not
    /// exist, and the real one silently formats as a raw tick count.
    #[test]
    fn the_crouch_time_constant_is_registered_under_sneak_time() {
        assert_eq!(custom_formatter("minecraft:sneak_time"), Formatter::Time);
        assert_eq!(custom_formatter("minecraft:crouch_time"), Formatter::Default);
        // …while `crouch_one_cm` really is spelled `crouch`.
        assert_eq!(
            custom_formatter("minecraft:crouch_one_cm"),
            Formatter::Distance
        );
    }

    #[test]
    fn the_three_exception_lists_do_not_overlap() {
        for s in TIME_STATS {
            assert!(!DISTANCE_STATS.contains(s) && !DIVIDE_BY_TEN_STATS.contains(s), "{s}");
        }
        for s in DISTANCE_STATS {
            assert!(!DIVIDE_BY_TEN_STATS.contains(s), "{s}");
        }
        assert_eq!(TIME_STATS.len(), 5);
        assert_eq!(DISTANCE_STATS.len(), 17);
        assert_eq!(DIVIDE_BY_TEN_STATS.len(), 7);
    }
}
