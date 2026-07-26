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
