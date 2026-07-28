//! The `minecraft:trim_material` and `minecraft:trim_pattern` registries, out
//! of Configuration's `registry_data` (M48).
//!
//! Both are **datapack** registries, so — exactly as for M42's enchantments —
//! their contents *and their id order* are whatever the server's packs say,
//! and the vector index is the protocol id. Deriving either from bootstrap
//! order is REWO_PLAN §0.0 gotcha 5.
//!
//! What each side contributes to a sprite name:
//!
//! ```text
//! trim_pattern:  asset_id: "minecraft:coast"    // Identifier
//!                decal:    false                // optional, default false
//!
//! trim_material: asset_name: "iron"             // MaterialAssetGroup's
//!                override_armor_assets: {       // MAP_CODEC is *inlined*,
//!                  "minecraft:iron": "iron_darker"   // so both are top-level
//!                }                                    // fields
//! ```
//!
//! and `ArmorTrim.layerAssetId` puts them together:
//!
//! ```java
//! MaterialAssetGroup.AssetInfo materialAsset = material.assets().assetId(equipmentAsset);
//! return pattern.assetId().withPath(p -> layerAssetPrefix + "/" + p + "_" + materialAsset.suffix());
//! ```
//!
//! # The override is what stops a trim disappearing
//!
//! `assetId(equipmentAsset)` is `overrides.getOrDefault(equipmentAssetId, base)`
//! — iron trim on *iron* armour resolves to `iron_darker`, and likewise for
//! gold, diamond, netherite and copper. Without it a same-material trim paints
//! the armour's own colour onto the armour and vanishes.

use rewo_proto::nbt::Nbt;

pub const TRIM_MATERIAL_REGISTRY: &str = "minecraft:trim_material";
pub const TRIM_PATTERN_REGISTRY: &str = "minecraft:trim_pattern";

/// One `trim_pattern` entry, at the index that is its protocol id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrimPatternDef {
    /// `minecraft:coast`.
    pub id: String,
    /// `asset_id` — the *sprite* name's stem, which is not always the entry id
    /// (a datapack may register `mypack:foo` with `asset_id: minecraft:coast`).
    pub asset_id: String,
    /// `decal` — selects `ARMOR_DECAL_CUTOUT_NO_CULL` over the ordinary armour
    /// pipeline. Both resolve to the same visible result here (see
    /// `entities.rs`), so it is decoded and recorded rather than acted on.
    pub decal: bool,
}

/// One `trim_material` entry, at the index that is its protocol id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrimMaterialDef {
    /// `minecraft:iron`.
    pub id: String,
    /// `asset_name` — the palette suffix, e.g. `iron`.
    pub asset_name: String,
    /// `override_armor_assets`: equipment-asset id → palette suffix.
    pub overrides: Vec<(String, String)>,
}

impl TrimMaterialDef {
    /// `MaterialAssetGroup.assetId` — `overrides.getOrDefault(asset, base)`.
    pub fn suffix_for(&self, equipment_asset: &str) -> &str {
        self.overrides
            .iter()
            .find(|(k, _)| k == equipment_asset)
            .map_or(self.asset_name.as_str(), |(_, v)| v.as_str())
    }
}

/// `pattern.assetId().withPath(p -> prefix + "/" + p + "_" + suffix)`.
///
/// The namespace is the **pattern's**, and the prefix is
/// `LayerType.trimAssetPrefix()` = `trims/entity/<layer id>`. Returned without
/// a namespace prefix when the pattern is `minecraft:`, because that is how the
/// texture path is built and how Rewo keys its atlas.
pub fn layer_asset_path(pattern_asset_id: &str, prefix: &str, suffix: &str) -> String {
    let path = pattern_asset_id
        .split_once(':')
        .map_or(pattern_asset_id, |(_, p)| p);
    format!("{prefix}/{path}_{suffix}")
}

fn read_entries<T>(
    r: &mut rewo_proto::reader::PacketReader,
    count: usize,
    mut f: impl FnMut(String, Option<Nbt>) -> T,
) -> Vec<T> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let Ok(id) = r.identifier() else {
            break;
        };
        // `Optional<NBT>` — present only because Rewo answers
        // `select_known_packs` with an empty list.
        let data = match r.u8() {
            Ok(0) => None,
            Ok(_) => r.nbt().ok(),
            Err(_) => break,
        };
        out.push(f(id, data));
    }
    out
}

pub fn parse_trim_pattern_registry(
    r: &mut rewo_proto::reader::PacketReader,
    count: usize,
) -> Vec<TrimPatternDef> {
    read_entries(r, count, |id, data| {
        let asset_id = data
            .as_ref()
            .and_then(|n| n.get("asset_id"))
            .and_then(|v| match v {
                Nbt::String(s) => Some(s.clone()),
                _ => None,
            })
            // An entry that arrived without its payload still names itself,
            // and vanilla's own patterns use their id as the asset id.
            .unwrap_or_else(|| id.clone());
        let decal = data
            .as_ref()
            .and_then(|n| n.get("decal"))
            .and_then(|v| v.as_i64())
            .is_some_and(|v| v != 0);
        TrimPatternDef {
            id,
            asset_id,
            decal,
        }
    })
}

pub fn parse_trim_material_registry(
    r: &mut rewo_proto::reader::PacketReader,
    count: usize,
) -> Vec<TrimMaterialDef> {
    read_entries(r, count, |id, data| {
        let asset_name = data
            .as_ref()
            .and_then(|n| n.get("asset_name"))
            .and_then(|v| match v {
                Nbt::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| {
                id.split_once(':').map_or(id.clone(), |(_, p)| p.to_string())
            });
        let mut overrides = Vec::new();
        if let Some(Nbt::Compound(map)) = data.as_ref().and_then(|n| n.get("override_armor_assets"))
        {
            for (k, v) in map {
                if let Nbt::String(s) = v {
                    overrides.push((k.clone(), s.clone()));
                }
            }
        }
        TrimMaterialDef {
            id,
            asset_name,
            overrides,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sprite_name_is_pattern_path_underscore_material_suffix() {
        assert_eq!(
            layer_asset_path("minecraft:coast", "trims/entity/humanoid", "iron"),
            "trims/entity/humanoid/coast_iron"
        );
        // A datapack's own namespace does not enter the path — the *path* is
        // what `withPath` rewrites.
        assert_eq!(
            layer_asset_path("mypack:swirl", "trims/entity/humanoid_leggings", "gold_darker"),
            "trims/entity/humanoid_leggings/swirl_gold_darker"
        );
    }

    /// The override is keyed by the **equipment asset**, not by the material.
    #[test]
    fn a_same_material_trim_takes_the_darker_override() {
        let m = TrimMaterialDef {
            id: "minecraft:iron".into(),
            asset_name: "iron".into(),
            overrides: vec![("minecraft:iron".into(), "iron_darker".into())],
        };
        assert_eq!(m.suffix_for("minecraft:iron"), "iron_darker");
        assert_eq!(m.suffix_for("minecraft:diamond"), "iron");
        // A material with no overrides at all is its base everywhere.
        let q = TrimMaterialDef {
            id: "minecraft:quartz".into(),
            asset_name: "quartz".into(),
            overrides: vec![],
        };
        assert_eq!(q.suffix_for("minecraft:quartz"), "quartz");
    }
}
