//! rewo-data — vanilla data pipeline: parse the data-generator reports
//! (blocks.json, packets.json) into runtime tables, and ensure the server
//! jar exists for (re)running the generator.
//!
//! Reports are produced once per (MC version) under
//! `<data_dir>/rewo/<version>/datagen/generated/reports/` (REWO_PLAN.md §5).
//! For M1 the reports are generated out-of-band (a PowerShell/bash step);
//! this crate consumes them. Wiring the generator run into the launcher's
//! Native-instance setup is an M1-followon.

pub mod assets;
pub mod attributes;
pub mod be_transform;
pub mod block_entity_models;
pub mod block_entity_types;
pub mod block_light;
pub mod blocks;
pub mod cem;
pub mod chest_states;
pub mod copper_golem_poses;
pub mod components;
pub mod enchantments;
pub mod equipment;
pub mod entity_attachments;
pub mod entity_attachments_table;
pub mod entity_attributes;
pub mod entity_classes;
pub mod entity_pick;
pub mod entity_pick_table;
pub mod entity_types;
pub mod etf;
pub mod items;
pub mod held_items;
pub mod item_components_table;
pub mod item_geometry;
pub mod item_models;
pub mod item_props_table;
pub mod item_tags;
pub mod lang;
pub mod level_event_sounds;
pub mod number_formats;
pub mod mob_variants;
pub mod packets;
pub mod particle_types;
pub mod server_jar;
pub mod sign_states;
pub mod sign_text;
pub mod sound_events;
pub mod sounds_json;
pub mod stats;
pub mod swing_anim;
pub mod swing_anim_table;
pub mod use_item;
pub mod use_item_table;

use std::path::{Path, PathBuf};

/// The on-disk home for a pinned version's generated data.
pub struct DataPaths {
    pub root: PathBuf,
}

impl DataPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<config>/EwoClient/rewo/<version>` — the default layout.
    pub fn for_version(version: &str) -> Option<Self> {
        let mut p = dirs::config_dir()?;
        p.push("EwoClient");
        p.push("rewo");
        p.push(version);
        Some(Self::new(p))
    }

    pub fn reports_dir(&self) -> PathBuf {
        self.root.join("datagen/generated/reports")
    }

    pub fn blocks_json(&self) -> PathBuf {
        self.reports_dir().join("blocks.json")
    }

    pub fn packets_json(&self) -> PathBuf {
        self.reports_dir().join("packets.json")
    }

    pub fn registries_json(&self) -> PathBuf {
        self.reports_dir().join("registries.json")
    }

    pub fn server_jar(&self) -> PathBuf {
        self.root.join("server.jar")
    }
}

/// Both report tables loaded together — what the net + world layers need.
pub struct GameData {
    pub blocks: blocks::Blocks,
    pub packets: packets::Packets,
    pub items: items::Items,
    pub entity_types: entity_types::EntityTypes,
    /// Item id → prototype `minecraft:swing_animation` (M19 combat swings).
    pub swing_animations: swing_anim::SwingAnimations,
    /// Item id → `getUseDuration` / `getUseAnimation` (M23 item use).
    pub use_profiles: use_item::UseProfiles,
    /// Data-component registry ids an item-stack patch is keyed by.
    pub components: components::DataComponentIds,
    /// Every data component's name and id (M41). Separate from the handful of
    /// named ids above because the component walker is keyed by name and needs
    /// the whole registry, not the ones with a special meaning.
    pub component_registry: components::DataComponentRegistry,
    /// Which entity types are living, and which tick a combat swing (M19).
    pub entity_classes: entity_types::EntityClasses,
    /// Per-entity-type PASSENGER / VEHICLE attachment points (M72). The
    /// seat a rider sits on is entity-type *data* in 26.x, not a constant.
    pub entity_attachments: entity_attachments::Attachments,
    /// Particle-type registry ids → names (M37 particles).
    pub particle_types: particle_types::ParticleTypes,
    /// Sound-event registry ids → names (M64). Resolves the id a decoded
    /// `SoundRef::Registry` carries; a built-in registry, so the report is the
    /// authority and the server never sends this table.
    pub sound_events: sound_events::SoundEvents,
    /// The three `minecraft:number_format_type` ids a scoreboard objective or
    /// score dispatches its optional number format on (M65).
    pub number_formats: number_formats::NumberFormatTypeIds,
    /// The `minecraft:attribute` id table joined with the extracted
    /// per-attribute clamps and per-entity-type suppliers (M52).
    pub attributes: std::sync::Arc<attributes::AttributeRegistry>,
    /// `minecraft:stat_type` + `minecraft:custom_stat` + `minecraft:block`,
    /// the three registries the statistics screen needs (M84).
    pub stat_registries: stats::StatRegistries,
}

impl GameData {
    pub fn load(paths: &DataPaths) -> Result<Self, String> {
        let blocks = blocks::Blocks::load(&paths.blocks_json())?;
        let packets = packets::Packets::load(&paths.packets_json())?;
        let items = items::Items::load(&paths.registries_json())?;
        let entity_types = entity_types::EntityTypes::load(&paths.registries_json())?;
        let swing_animations = swing_anim::SwingAnimations::resolve(&items)?;
        let use_profiles = use_item::UseProfiles::resolve(&items)?;
        let components = components::DataComponentIds::load(&paths.registries_json())?;
        let component_registry =
            components::DataComponentRegistry::load(&paths.registries_json())?;
        let entity_classes = entity_types::EntityClasses::resolve(&entity_types)?;
        let entity_attachments =
            entity_attachments::Attachments::resolve(&entity_types)?;
        let particle_types = particle_types::ParticleTypes::load(&paths.registries_json())?;
        let sound_events = sound_events::SoundEvents::load(&paths.registries_json())?;
        let number_formats =
            number_formats::NumberFormatTypeIds::load(&paths.registries_json())?;
        let attributes =
            std::sync::Arc::new(attributes::AttributeRegistry::load(&paths.registries_json())?);
        let stat_registries = stats::StatRegistries::load(&paths.registries_json())?;
        Ok(Self {
            blocks,
            packets,
            items,
            entity_types,
            swing_animations,
            use_profiles,
            components,
            component_registry,
            entity_classes,
            entity_attachments,
            particle_types,
            sound_events,
            number_formats,
            attributes,
            stat_registries,
        })
    }

    pub fn load_for_version(version: &str) -> Result<Self, String> {
        let paths = DataPaths::for_version(version)
            .ok_or_else(|| "no config dir for version data".to_string())?;
        Self::load(&paths)
    }
}

fn read_json_file(path: &Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}
