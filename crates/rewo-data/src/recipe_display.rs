//! registries.json → the three recipe-book id tables (M93y).
//!
//! `SlotDisplay`, `RecipeDisplay` and `RecipeBookCategory` all dispatch on
//! `ByteBufCodecs.registry(...)` — a raw protocol id — and all three are
//! **`BuiltInRegistries` entries**, so the server never sends them. That makes
//! them M92's rule exactly: **a built-in registry is resolved by name from the
//! report**, never off the wire and never by iteration order.
//!
//! The iteration-order half is M64's alphabetisation trap and it would bite
//! hard here. `serde_json`'s default map is sorted, so `enumerate()` would put
//! `minecraft:any_fuel` at 0 where the real registry has `minecraft:empty`
//! there — and *every* slot display would then decode as the wrong variant,
//! with bodies of different lengths, so the reader would desync mid-packet
//! rather than merely mislabel. Read `protocol_id`.
//!
//! Ground truth: `<data_dir>/rewo/26.2/datagen/generated/reports/registries.json`,
//! keys `minecraft:slot_display`, `minecraft:recipe_display` and
//! `minecraft:recipe_book_category`.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

/// One registry's two-way id table.
#[derive(Clone, Debug, Default)]
pub struct IdTable {
    by_id: HashMap<i32, String>,
    by_name: HashMap<String, i32>,
}

impl IdTable {
    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    pub fn id(&self, name: &str) -> Option<i32> {
        self.by_name.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// The recipe book's three registries.
#[derive(Clone, Debug, Default)]
pub struct RecipeDisplayIds {
    pub slot_display: IdTable,
    pub recipe_display: IdTable,
    pub category: IdTable,
}

fn table(json: &serde_json::Value, key: &str) -> Result<IdTable, String> {
    let entries = json
        .get(key)
        .and_then(|r| r.get("entries"))
        .and_then(|e| e.as_object())
        .ok_or_else(|| format!("registries.json: no {key} registry"))?;
    let mut t = IdTable::default();
    for (name, entry) in entries {
        // `protocol_id`, never the iteration index — see the module docs.
        let id = entry
            .get("protocol_id")
            .and_then(|i| i.as_i64())
            .ok_or_else(|| format!("registries.json: {name} has no protocol_id"))?;
        t.by_id.insert(id as i32, name.clone());
        t.by_name.insert(name.clone(), id as i32);
    }
    if t.is_empty() {
        return Err(format!("registries.json: {key} is empty"));
    }
    Ok(t)
}

impl RecipeDisplayIds {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        Ok(Self {
            slot_display: table(&json, "minecraft:slot_display")?,
            recipe_display: table(&json, "minecraft:recipe_display")?,
            category: table(&json, "minecraft:recipe_book_category")?,
        })
    }

    /// The variant a `SlotDisplay`'s leading var-int names.
    pub fn slot(&self, id: i32) -> Option<SlotKind> {
        SlotKind::from_name(self.slot_display.name(id)?)
    }

    /// The variant a `RecipeDisplay`'s leading var-int names.
    pub fn recipe(&self, id: i32) -> Option<RecipeKind> {
        RecipeKind::from_name(self.recipe_display.name(id)?)
    }
}

/// `minecraft:slot_display`'s eleven variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    Empty,
    AnyFuel,
    WithAnyPotion,
    OnlyWithComponent,
    Item,
    ItemStack,
    Tag,
    Dyed,
    SmithingTrim,
    WithRemainder,
    Composite,
}

impl SlotKind {
    /// Resolved by NAME, so a renumber fails loud rather than decoding one
    /// variant's body as another's.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "minecraft:empty" => Self::Empty,
            "minecraft:any_fuel" => Self::AnyFuel,
            "minecraft:with_any_potion" => Self::WithAnyPotion,
            "minecraft:only_with_component" => Self::OnlyWithComponent,
            "minecraft:item" => Self::Item,
            "minecraft:item_stack" => Self::ItemStack,
            "minecraft:tag" => Self::Tag,
            "minecraft:dyed" => Self::Dyed,
            "minecraft:smithing_trim" => Self::SmithingTrim,
            "minecraft:with_remainder" => Self::WithRemainder,
            "minecraft:composite" => Self::Composite,
            _ => return None,
        })
    }
}

/// `minecraft:recipe_display`'s five variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeKind {
    CraftingShapeless,
    CraftingShaped,
    Furnace,
    Stonecutter,
    Smithing,
}

impl RecipeKind {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "minecraft:crafting_shapeless" => Self::CraftingShapeless,
            "minecraft:crafting_shaped" => Self::CraftingShaped,
            "minecraft:furnace" => Self::Furnace,
            "minecraft:stonecutter" => Self::Stonecutter,
            "minecraft:smithing" => Self::Smithing,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `None` when the decompile is absent, as every report-backed test in
    /// this crate does — the data is derived from the user's own download.
    fn ids() -> Option<RecipeDisplayIds> {
        let paths = crate::DataPaths::for_version("26.2")?;
        RecipeDisplayIds::load(&paths.registries_json()).ok()
    }

    #[test]
    fn the_three_registries_are_the_sizes_the_report_says() {
        let Some(i) = ids() else { return };
        assert_eq!(i.slot_display.len(), 11);
        assert_eq!(i.recipe_display.len(), 5);
        assert_eq!(i.category.len(), 13);
    }

    /// The ids come from `protocol_id`, and sorted order is NOT that order.
    ///
    /// This is the check that matters: `minecraft:empty` is id **0** while
    /// alphabetically `minecraft:any_fuel` comes first. An `enumerate()`-based
    /// table would put `AnyFuel` at 0 — and because the two have *different*
    /// body lengths, the reader would desync mid-packet rather than merely
    /// mislabel a variant.
    #[test]
    fn the_ids_are_protocol_ids_and_not_alphabetical() {
        let Some(i) = ids() else { return };
        assert_eq!(i.slot(0), Some(SlotKind::Empty));
        assert_eq!(i.slot(1), Some(SlotKind::AnyFuel));
        assert_eq!(i.slot(10), Some(SlotKind::Composite));
        // Alphabetically `any_fuel` precedes `empty`, so a sorted table would
        // have swapped these two.
        assert!("minecraft:any_fuel" < "minecraft:empty");
        assert_eq!(i.recipe(0), Some(RecipeKind::CraftingShapeless));
        assert_eq!(i.recipe(4), Some(RecipeKind::Smithing));
    }

    #[test]
    fn an_unknown_id_is_none_rather_than_a_substitute() {
        let Some(i) = ids() else { return };
        assert_eq!(i.slot(99), None);
        assert_eq!(i.recipe(99), None);
        assert_eq!(SlotKind::from_name("minecraft:not_a_display"), None);
    }

    #[test]
    fn every_variant_in_the_registry_maps_to_a_kind() {
        // If a version adds one, this fails rather than letting the decoder
        // meet an id it cannot size the body of.
        let Some(i) = ids() else { return };
        for id in 0..i.slot_display.len() as i32 {
            assert!(i.slot(id).is_some(), "slot_display {id} unmapped");
        }
        for id in 0..i.recipe_display.len() as i32 {
            assert!(i.recipe(id).is_some(), "recipe_display {id} unmapped");
        }
    }
}
