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
pub mod block_light;
pub mod blocks;
pub mod cem;
pub mod components;
pub mod entity_classes;
pub mod entity_types;
pub mod items;
pub mod item_geometry;
pub mod item_models;
pub mod item_tags;
pub mod packets;
pub mod server_jar;
pub mod swing_anim;
pub mod swing_anim_table;

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
    /// Data-component registry ids an item-stack patch is keyed by.
    pub components: components::DataComponentIds,
    /// Which entity types are living, and which tick a combat swing (M19).
    pub entity_classes: entity_types::EntityClasses,
}

impl GameData {
    pub fn load(paths: &DataPaths) -> Result<Self, String> {
        let blocks = blocks::Blocks::load(&paths.blocks_json())?;
        let packets = packets::Packets::load(&paths.packets_json())?;
        let items = items::Items::load(&paths.registries_json())?;
        let entity_types = entity_types::EntityTypes::load(&paths.registries_json())?;
        let swing_animations = swing_anim::SwingAnimations::resolve(&items)?;
        let components = components::DataComponentIds::load(&paths.registries_json())?;
        let entity_classes = entity_types::EntityClasses::resolve(&entity_types)?;
        Ok(Self {
            blocks,
            packets,
            items,
            entity_types,
            swing_animations,
            components,
            entity_classes,
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
