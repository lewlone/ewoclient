//! `registries.json` → the `minecraft:block_entity_type` id table (M26).
//!
//! **Why this is not read off the wire.** `block_entity_type` is a *bootstrap*
//! registry: a vanilla server syncs only datapack registries (dimension types,
//! biomes, world clocks) in the Configuration `registry_data` packet, so the
//! ids a chunk payload's block-entity list carries are the ones the pinned
//! version's own registry assigns. That makes them exactly like
//! [`crate::entity_types`] and unlike the dimension holder ids — read from the
//! datagen report, resolved **by name**, so a renumber between versions fails
//! loud instead of silently animating the wrong block.
//!
//! Before M26 this table existed only inside `blockentityshot`, which meant
//! the fail-closed boundary
//! [`rewo_world::block_entities::BlockEntityRegistry::resolve`] documents —
//! "a registry entry the table does not classify means a new block-entity type
//! shipped and nobody decided what it looks like" — protected the *gate* and
//! not the client. It runs in the client now.

use std::path::Path;

use crate::read_json_file;

/// M247 — per block state, the block entity `LevelChunk.setBlockState`
/// creates for it on the client, as `(type protocol id, block ordinal)`;
/// `None` for a block with none. From
/// [`crate::block_entity_blocks::BLOCK_ENTITY_BLOCKS`] (each type's
/// `validBlocks`) and the registry's ids. The ordinal tells two blocks apart:
/// a DIFFERENT block drops the old entity, another state of the same block
/// (a chest turned, a furnace lit) keeps it.
pub fn block_entity_states(
    blocks: &crate::blocks::Blocks,
    types: &[(String, i32)],
) -> Vec<Option<(i32, u32)>> {
    let id_of: std::collections::HashMap<&str, i32> =
        types.iter().map(|(n, i)| (n.as_str(), *i)).collect();
    let mut of_block: std::collections::HashMap<&str, (i32, u32)> = Default::default();
    let mut ordinal = 0u32;
    for (ty, members) in crate::block_entity_blocks::BLOCK_ENTITY_BLOCKS {
        let Some(&id) = id_of.get(ty) else {
            continue;
        };
        for b in *members {
            of_block.insert(b, (id, ordinal));
            ordinal += 1;
        }
    }
    (0..blocks.state_count() as u32)
        .map(|s| blocks.block_name(s).and_then(|n| of_block.get(n).copied()))
        .collect()
}

/// Every `minecraft:block_entity_type` entry as `(name, protocol id)`.
///
/// Returned as a flat list rather than a map because that is what
/// `BlockEntityRegistry::resolve` grades, in both directions: a name it cannot
/// classify and a classification the registry has lost are both errors.
pub fn load(path: &Path) -> Result<Vec<(String, i32)>, String> {
    let json = read_json_file(path)?;
    let entries = json
        .get("minecraft:block_entity_type")
        .and_then(|r| r.get("entries"))
        .and_then(|e| e.as_object())
        .ok_or("registries.json: no minecraft:block_entity_type registry")?;
    let mut out = Vec::with_capacity(entries.len());
    for (name, entry) in entries {
        let id = entry
            .get("protocol_id")
            .and_then(|i| i.as_i64())
            .ok_or_else(|| format!("registries.json: {name} has no protocol_id"))?;
        out.push((name.clone(), id as i32));
    }
    if out.is_empty() {
        return Err("registries.json: block_entity_type registry is empty".into());
    }
    log::info!("rewo-data: {} block-entity type(s)", out.len());
    Ok(out)
}

#[cfg(test)]
mod block_entity_states_tests {
    /// M247 — on the real report: every chest state carries the chest type
    /// and one ordinal, a furnace a different one, stone none.
    #[test]
    fn chest_states_carry_the_chest_type_and_one_ordinal() {
        let Some(paths) = crate::DataPaths::for_version("26.2") else {
            crate::skip_test!("no local 26.2 datagen report");
            return;
        };
        if !paths.blocks_json().is_file() || !paths.registries_json().is_file() {
            crate::skip_test!("no local 26.2 datagen report");
            return;
        }
        let json = crate::read_json_file(&paths.blocks_json()).unwrap();
        let blocks = crate::blocks::Blocks::from_json(&json).unwrap();
        let types = super::load(&paths.registries_json()).unwrap();
        let table = super::block_entity_states(&blocks, &types);
        assert_eq!(table.len(), blocks.state_count());
        let chest_id = types
            .iter()
            .find(|(n, _)| n == "minecraft:chest")
            .unwrap()
            .1;
        let of = |name: &str| -> Vec<Option<(i32, u32)>> {
            (0..blocks.state_count() as u32)
                .filter(|&s| blocks.block_name(s) == Some(name))
                .map(|s| table[s as usize])
                .collect()
        };
        let chest = of("minecraft:chest");
        assert!(!chest.is_empty());
        assert!(chest.iter().all(|c| c.map(|c| c.0) == Some(chest_id)));
        assert!(
            chest.windows(2).all(|w| w[0] == w[1]),
            "one ordinal per block"
        );
        let furnace = of("minecraft:furnace");
        assert!(furnace
            .iter()
            .all(|f| f.is_some() && f.map(|f| f.1) != chest[0].map(|c| c.1)));
        assert!(of("minecraft:stone").iter().all(Option::is_none));
    }
}
