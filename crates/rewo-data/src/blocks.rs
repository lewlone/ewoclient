//! blocks.json → state-id table.
//!
//! Shape (per the datagen report):
//! ```json
//! "minecraft:air": {
//!   "definition": { "type": "minecraft:air", "properties": {} },
//!   "states": [ { "default": true, "id": 0 } ]
//! }
//! ```
//! For M1 we only need: state-id → block name (for queries/logging) and the
//! total state count (for the chunk global-palette bit width). Per-state
//! property maps come with the mesher in M4.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

pub struct Blocks {
    /// state id → block resource name (e.g. "minecraft:grass_block").
    state_to_block: Vec<String>,
    /// block name → its default state id.
    default_state: HashMap<String, u32>,
    /// Bits needed to index any state in the global palette:
    /// `ceil(log2(state_count))`. Used by the chunk decoder's direct path.
    pub global_palette_bits: u32,
}

impl Blocks {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let obj = json
            .as_object()
            .ok_or("blocks.json: root is not an object")?;

        let mut max_id: i64 = -1;
        let mut entries: Vec<(u32, String, bool)> = Vec::new();
        for (block_name, def) in obj {
            let states = def
                .get("states")
                .and_then(|s| s.as_array())
                .ok_or_else(|| format!("blocks.json: {block_name} has no states array"))?;
            for state in states {
                let id = state
                    .get("id")
                    .and_then(|i| i.as_u64())
                    .ok_or_else(|| format!("blocks.json: {block_name} state missing id"))?
                    as u32;
                let is_default = state.get("default").and_then(|d| d.as_bool()).unwrap_or(false);
                max_id = max_id.max(id as i64);
                entries.push((id, block_name.clone(), is_default));
            }
        }
        if max_id < 0 {
            return Err("blocks.json: no block states found".into());
        }

        let count = (max_id + 1) as usize;
        let mut state_to_block = vec![String::new(); count];
        let mut default_state = HashMap::new();
        for (id, name, is_default) in entries {
            if is_default {
                default_state.insert(name.clone(), id);
            }
            state_to_block[id as usize] = name;
        }

        // Air must be state 0 — a cheap correctness assertion on the whole
        // table (the chunk decoder treats 0 as empty).
        if state_to_block.first().map(|s| s.as_str()) != Some("minecraft:air") {
            return Err(format!(
                "blocks.json: state 0 is {:?}, expected minecraft:air",
                state_to_block.first()
            ));
        }

        let global_palette_bits = ceil_log2(count as u32);
        log::info!(
            "rewo-data: {} block states, global palette {} bits",
            count,
            global_palette_bits
        );
        Ok(Self {
            state_to_block,
            default_state,
            global_palette_bits,
        })
    }

    pub fn state_count(&self) -> usize {
        self.state_to_block.len()
    }

    pub fn block_name(&self, state_id: u32) -> Option<&str> {
        self.state_to_block.get(state_id as usize).map(|s| s.as_str())
    }

    pub fn default_state(&self, block_name: &str) -> Option<u32> {
        self.default_state.get(block_name).copied()
    }
}

/// Smallest `n` with `2^n >= v` (bits to represent ids `0..v`).
fn ceil_log2(v: u32) -> u32 {
    if v <= 1 {
        return 0;
    }
    32 - (v - 1).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::ceil_log2;

    #[test]
    fn ceil_log2_matches_palette_widths() {
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(16), 4);
        assert_eq!(ceil_log2(17), 5);
        // 32366 block states → 15-bit global palette (confirmed vs decompile).
        assert_eq!(ceil_log2(32366), 15);
        assert_eq!(ceil_log2(32768), 15);
        assert_eq!(ceil_log2(32769), 16);
    }
}
