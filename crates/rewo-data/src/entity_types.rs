//! registries.json → entity-type table. `add_entity` carries the type as a
//! protocol id into the `minecraft:entity_type` registry; rendering wants
//! the name back (is it a player? what capsule size / color?).

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

pub struct EntityTypes {
    by_id: HashMap<i32, String>,
    pub player_id: i32,
}

impl EntityTypes {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:entity_type")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:entity_type registry")?;
        let mut by_id = HashMap::with_capacity(entries.len());
        for (name, entry) in entries {
            if let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) {
                by_id.insert(id as i32, name.clone());
            }
        }
        let player_id = by_id
            .iter()
            .find(|(_, n)| n.as_str() == "minecraft:player")
            .map(|(id, _)| *id)
            .ok_or("registries.json: no minecraft:player entity type")?;
        log::info!("rewo-data: {} entity types (player = {player_id})", by_id.len());
        Ok(Self { by_id, player_id })
    }

    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(|s| s.as_str())
    }

    /// Capsule footprint (width, height) in blocks for a type id. Exact for
    /// the common types; everything else gets the humanoid default — the v1
    /// capsule renderer doesn't need more (REWO_PLAN correction #11).
    pub fn dimensions(&self, id: i32) -> (f32, f32) {
        match self.name(id).unwrap_or("") {
            "minecraft:player" => (0.6, 1.8),
            "minecraft:zombie" | "minecraft:husk" | "minecraft:drowned" => (0.6, 1.95),
            "minecraft:skeleton" | "minecraft:stray" => (0.6, 1.99),
            "minecraft:creeper" => (0.6, 1.7),
            "minecraft:villager" => (0.6, 1.95),
            "minecraft:enderman" => (0.6, 2.9),
            "minecraft:cow" | "minecraft:mooshroom" => (0.9, 1.4),
            "minecraft:pig" => (0.9, 0.9),
            "minecraft:sheep" => (0.9, 1.3),
            "minecraft:chicken" => (0.4, 0.7),
            "minecraft:wolf" => (0.6, 0.85),
            "minecraft:armor_stand" => (0.5, 1.975),
            "minecraft:item" => (0.25, 0.25),
            _ => (0.6, 1.8),
        }
    }

    /// Whether this entity type shoves other entities out of the way.
    ///
    /// Vanilla `Entity.isPushable()` is **false** by default and only
    /// `LivingEntity` overrides it to true, so items, arrows, displays and
    /// armor stands never push. This is the same rule expressed as an
    /// exclusion list over the registry names, since the wire gives us type
    /// ids rather than class hierarchies.
    pub fn pushable(&self, id: i32) -> bool {
        let name = self.name(id).unwrap_or("");
        let short = name.strip_prefix("minecraft:").unwrap_or(name);
        // Projectiles, drops, markers and decorations — never pushable.
        const NOT_LIVING: &[&str] = &[
            "item", "experience_orb", "arrow", "spectral_arrow", "trident",
            "snowball", "egg", "ender_pearl", "eye_of_ender", "potion",
            "experience_bottle", "fireball", "small_fireball", "dragon_fireball",
            "wither_skull", "wind_charge", "breeze_wind_charge", "llama_spit",
            "shulker_bullet", "fishing_bobber", "firework_rocket", "tnt",
            "falling_block", "lightning_bolt", "area_effect_cloud", "painting",
            "item_frame", "glow_item_frame", "leash_knot", "marker",
            "interaction", "text_display", "block_display", "item_display",
            "armor_stand", "end_crystal", "evoker_fangs", "ominous_item_spawner",
        ];
        !NOT_LIVING.contains(&short)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}
