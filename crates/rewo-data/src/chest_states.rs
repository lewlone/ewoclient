//! Chest block states → the two properties `ChestRenderer` reads (M25b).
//!
//! `ChestRenderer.extractRenderState` reads `ChestBlock.FACING` and
//! `ChestBlock.TYPE` off the block state, and `createModelTransformation`
//! turns the facing into `rotationAround(YP(-facing.toYRot()), 0.5, 0, 0.5)`.
//!
//! Rewo's [`crate::blocks::Blocks`] keeps only state → block name, because
//! nothing before this needed a property. Rather than widen it to every
//! property of all 32k states, this reads `blocks.json` a second time and
//! retains just the chest states — a few hundred entries.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

/// `Direction`'s horizontal members, as `toYRot()` degrees.
///
/// ```text
/// NORTH 180   SOUTH 0   WEST 90   EAST 270
/// ```
///
/// Worth transcribing rather than deriving: north is 180, not 0, and getting
/// that backwards points every chest the wrong way while still looking like a
/// chest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChestFacing {
    North,
    South,
    West,
    East,
}

impl ChestFacing {
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "north" => ChestFacing::North,
            "south" => ChestFacing::South,
            "west" => ChestFacing::West,
            "east" => ChestFacing::East,
            _ => return None,
        })
    }

    /// `Direction.toYRot()`.
    pub fn to_y_rot(self) -> f32 {
        match self {
            ChestFacing::South => 0.0,
            ChestFacing::West => 90.0,
            ChestFacing::North => 180.0,
            ChestFacing::East => 270.0,
        }
    }
}

/// `ChestType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChestType {
    Single,
    Left,
    Right,
}

/// What Rewo needs to draw one chest state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChestState {
    pub facing: ChestFacing,
    pub kind: ChestType,
    /// The `rewo:be/…` model name for this chest's **material** — the SINGLE
    /// variant. A half appends [`crate::block_entity_models::LEFT_SUFFIX`] or
    /// `RIGHT_SUFFIX`, which [`Self::model_name`] does.
    pub model: &'static str,
}

/// Block name → its `rewo:be/…` model, for the chests Rewo renders.
///
/// An ender chest has no `type` property (it is always single) and a copper
/// chest's material comes from its block, which is why this is keyed by block
/// rather than derived from one name.
const CHEST_BLOCKS: &[(&str, &str)] = &[
    ("minecraft:chest", "rewo:be/chest"),
    ("minecraft:trapped_chest", "rewo:be/trapped_chest"),
    ("minecraft:ender_chest", "rewo:be/ender_chest"),
    ("minecraft:copper_chest", "rewo:be/copper_chest"),
    ("minecraft:exposed_copper_chest", "rewo:be/exposed_copper_chest"),
    ("minecraft:weathered_copper_chest", "rewo:be/weathered_copper_chest"),
    ("minecraft:oxidized_copper_chest", "rewo:be/oxidized_copper_chest"),
    // The waxed variants share their unwaxed material's texture — waxing
    // changes only whether the block oxidises further, not how it looks.
    ("minecraft:waxed_copper_chest", "rewo:be/copper_chest"),
    ("minecraft:waxed_exposed_copper_chest", "rewo:be/exposed_copper_chest"),
    ("minecraft:waxed_weathered_copper_chest", "rewo:be/weathered_copper_chest"),
    ("minecraft:waxed_oxidized_copper_chest", "rewo:be/oxidized_copper_chest"),
];

/// State id → chest state, for every chest block state.
#[derive(Default)]
pub struct ChestStates {
    by_state: HashMap<u32, ChestState>,
}

impl ChestState {
    /// The model to draw, `Sheets.chooseSprite(material, type)`'s selection
    /// expressed as a name.
    ///
    /// An ender chest is always SINGLE, so it never reaches the suffixes —
    /// which is just as well, since the jar ships no `ender_left.png`.
    pub fn model_name(self) -> String {
        match self.kind {
            ChestType::Single => self.model.to_string(),
            ChestType::Left => {
                format!("{}{}", self.model, crate::block_entity_models::LEFT_SUFFIX)
            }
            ChestType::Right => {
                format!("{}{}", self.model, crate::block_entity_models::RIGHT_SUFFIX)
            }
        }
    }
}

impl ChestStates {
    /// Read the chest states out of `blocks.json`.
    ///
    /// Fails loud when a chest block is missing or a state carries a `facing`
    /// or `type` this does not recognise: a chest whose facing silently
    /// defaulted would render turned the wrong way, which reads as a texture
    /// bug rather than a data one.
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let obj = json.as_object().ok_or("blocks.json: root is not an object")?;
        let mut by_state = HashMap::new();
        for (block, model) in CHEST_BLOCKS {
            let Some(def) = obj.get(*block) else {
                // A copper chest is a 26.x addition; a version without it is a
                // real answer, not an error. The vanilla three are required.
                if matches!(
                    *block,
                    "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:ender_chest"
                ) {
                    return Err(format!("blocks.json: no {block}"));
                }
                continue;
            };
            let states = def
                .get("states")
                .and_then(|s| s.as_array())
                .ok_or_else(|| format!("blocks.json: {block} has no states"))?;
            for st in states {
                let id = st
                    .get("id")
                    .and_then(|i| i.as_u64())
                    .ok_or_else(|| format!("blocks.json: {block} state has no id"))?
                    as u32;
                let props = st.get("properties").and_then(|p| p.as_object());
                let facing_name = props
                    .and_then(|p| p.get("facing"))
                    .and_then(|f| f.as_str())
                    .ok_or_else(|| format!("blocks.json: {block} state {id} has no facing"))?;
                let facing = ChestFacing::from_name(facing_name).ok_or_else(|| {
                    format!("blocks.json: {block} state {id} has facing {facing_name:?}")
                })?;
                // An ender chest declares no `type`; every other chest does.
                let kind = match props.and_then(|p| p.get("type")).and_then(|t| t.as_str()) {
                    None => ChestType::Single,
                    Some("single") => ChestType::Single,
                    Some("left") => ChestType::Left,
                    Some("right") => ChestType::Right,
                    Some(other) => {
                        return Err(format!(
                            "blocks.json: {block} state {id} has chest type {other:?}"
                        ))
                    }
                };
                by_state.insert(id, ChestState { facing, kind, model });
            }
        }
        if by_state.is_empty() {
            return Err("blocks.json: no chest states found".into());
        }
        log::info!("rewo-data: {} chest block state(s)", by_state.len());
        Ok(Self { by_state })
    }

    /// The chest state for a block state id, or `None` when it is not a chest.
    pub fn get(&self, state_id: u32) -> Option<ChestState> {
        self.by_state.get(&state_id).copied()
    }

    pub fn len(&self) -> usize {
        self.by_state.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_state.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn north_is_one_eighty_not_zero() {
        // `Direction.toYRot()` — south is the zero, because the model faces
        // south in its own space.
        assert_eq!(ChestFacing::South.to_y_rot(), 0.0);
        assert_eq!(ChestFacing::West.to_y_rot(), 90.0);
        assert_eq!(ChestFacing::North.to_y_rot(), 180.0);
        assert_eq!(ChestFacing::East.to_y_rot(), 270.0);
    }

    #[test]
    fn only_the_four_horizontals_parse() {
        assert!(ChestFacing::from_name("up").is_none());
        assert!(ChestFacing::from_name("down").is_none());
        assert!(ChestFacing::from_name("north").is_some());
    }
}
