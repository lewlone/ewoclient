//! registries.json → the `minecraft:data_component_type` ids an item-stack
//! `DataComponentPatch` is keyed by.
//!
//! The patch encodes each entry's type as a raw registry id
//! (`DataComponentType.STREAM_CODEC = ByteBufCodecs.registry(DATA_COMPONENT_TYPE)`),
//! and those numbers move between versions — so they are resolved by name here
//! and never hard-coded, exactly like the packet ids (REWO_PLAN §0.0 gotcha 5).
//!
//! Only the components a decoder actually transcribes are listed; adding one
//! means adding its stream codec too (see `rewo_net::item_stack`).

use std::path::Path;

use crate::read_json_file;

/// Registry ids of the data components Rewo can read out of a patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataComponentIds {
    /// `minecraft:swing_animation` — `SwingAnimation.STREAM_CODEC`
    /// (type idMapper + VarInt duration). Drives M19's swing length + arm rig.
    pub swing_animation: i32,
    /// `minecraft:damage` — `ByteBufCodecs.VAR_INT`. Not read for its value:
    /// it is the component a vanilla server most often patches onto a held
    /// weapon, and being able to *skip* it is what lets the walk reach a swing
    /// override that follows it.
    pub damage: i32,
    /// `minecraft:charged_projectiles` — `ItemStackTemplate.STREAM_CODEC`
    /// under `ByteBufCodecs.list(1024)`. Read for `CrossbowItem.isCharged`,
    /// which is `!getOrDefault(CHARGED_PROJECTILES, EMPTY).isEmpty()` and is
    /// the sole gate on `ArmPose::CrossbowHold` (M23).
    ///
    /// Unlike the other two this one *must* be walked rather than merely
    /// skipped-past: a crossbow is only ever charged by a patch, so before M23
    /// every charged crossbow made its stack unresolvable and suppressed the
    /// entity's whole combat pose.
    pub charged_projectiles: i32,
}

impl DataComponentIds {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:data_component_type")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:data_component_type registry")?;
        let id = |name: &str| -> Result<i32, String> {
            entries
                .get(name)
                .and_then(|e| e.get("protocol_id"))
                .and_then(|i| i.as_i64())
                .map(|i| i as i32)
                .ok_or_else(|| format!("registries.json: no data component {name}"))
        };
        let ids = Self {
            swing_animation: id("minecraft:swing_animation")?,
            damage: id("minecraft:damage")?,
            charged_projectiles: id("minecraft:charged_projectiles")?,
        };
        log::info!(
            "rewo-data: data components — swing_animation={} damage={} charged_projectiles={}",
            ids.swing_animation,
            ids.damage,
            ids.charged_projectiles
        );
        Ok(ids)
    }
}
