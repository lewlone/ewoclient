//! registries.json → particle-type table (M37).
//!
//! `level_particles` carries the particle as a protocol id into the
//! `minecraft:particle_type` registry, and the simulation wants the name
//! back so it can pick a behaviour. Resolving by name from the report — the
//! same discipline the packet ids use (REWO_PLAN §11) — means a version bump
//! that renumbers the registry fails loud instead of quietly spawning flames
//! where the server asked for smoke.
//!
//! `block_id` is called out separately because `minecraft:block` is the one
//! kind Rewo renders whose options carry a payload (a block-state id) after
//! the type id; every other supported kind is a `SimpleParticleType` with an
//! empty body.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

#[derive(Clone, Default)]
pub struct ParticleTypes {
    by_id: HashMap<i32, String>,
    /// Protocol id of `minecraft:block`, whose options carry a state id.
    pub block_id: Option<i32>,
}

impl ParticleTypes {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:particle_type")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:particle_type registry")?;
        let mut by_id = HashMap::with_capacity(entries.len());
        for (name, entry) in entries {
            if let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) {
                by_id.insert(id as i32, name.clone());
            }
        }
        let block_id = by_id
            .iter()
            .find(|(_, n)| n.as_str() == "minecraft:block")
            .map(|(id, _)| *id);
        log::info!(
            "rewo-data: {} particle types (block = {block_id:?})",
            by_id.len()
        );
        Ok(Self { by_id, block_id })
    }

    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Build a table from explicit (id, name) pairs, for tests and gates that
    /// synthesise a packet body without a datagen report on disk (M161).
    ///
    /// Deliberately NOT a self-skipping `DataPaths` load in the unit tests that
    /// need this: a test that returns early on a machine without the report
    /// proves nothing on exactly the machine where it would be reassuring, which
    /// is the trap `REWO_AUDIO_PLAN` section 5 records. The report-backed
    /// claims — that the registry really holds 125 names in this order — live in
    /// `soundshot`, which fails closed rather than skipping.
    pub fn from_pairs(pairs: &[(i32, &str)]) -> Self {
        let mut by_id = HashMap::with_capacity(pairs.len());
        for (id, name) in pairs {
            by_id.insert(*id, (*name).to_string());
        }
        let block_id = by_id
            .iter()
            .find(|(_, n)| n.as_str() == "minecraft:block")
            .map(|(id, _)| *id);
        Self { by_id, block_id }
    }

    /// Protocol id for a registry name, for tests and gates that need to
    /// synthesise a packet body.
    pub fn id_of(&self, name: &str) -> Option<i32> {
        self.by_id
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(id, _)| *id)
    }
}
