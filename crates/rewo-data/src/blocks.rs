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

#[derive(Clone)]
pub struct Blocks {
    /// state id → block resource name (e.g. "minecraft:grass_block").
    state_to_block: Vec<String>,
    /// block name → its default state id.
    default_state: HashMap<String, u32>,
    /// Every block name, sorted (M119). A `Vec` rather than the map's keys so
    /// a caller iterating for suggestions gets a stable order — vanilla's own
    /// is a registry order that `Suggestions.create` sorts away anyway.
    names: Vec<String>,
    /// block name → its properties, each with its legal values.
    ///
    /// From `blocks.json`'s per-block `properties` object. **serde_json's
    /// default `Map` is a sorted `BTreeMap`, so declaration order is already
    /// lost here** — M64's alphabetisation trap. It costs nothing this time
    /// because the only consumer is a suggestion list that gets sorted, but
    /// it would matter to anything that numbered them.
    properties: HashMap<String, Vec<(String, Vec<String>)>>,
    /// M229 — each state's property values, as `(index into its block's
    /// `properties` list, index into that property's values)`, from the
    /// state's own `properties` object (the block-level order is not relied
    /// on: serde's map sorts its keys).
    state_props: Vec<Box<[(u8, u8)]>>,
    /// Bits needed to index any state in the global palette:
    /// `ceil(log2(state_count))`. Used by the chunk decoder's direct path.
    pub global_palette_bits: u32,
}

impl Blocks {
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::from_json(&read_json_file(path)?)
    }

    /// [`Self::load`] over an already-parsed report.
    ///
    /// **The ids are file content, not a trusted index.** Sizing the table
    /// from the largest id (`vec![..; max_id + 1]`) let one hand-edited or
    /// corrupt `"id": 4000000000` ask for gigabytes before anything checked it.
    /// The table is sized by the NUMBER OF STATES the file actually lists, and
    /// each id must land inside it exactly once: that bounds the allocation by
    /// the file's own size and makes a gap, a duplicate or a stray id a named
    /// error instead of an empty-string hole that reads as a block with no
    /// name. (A datagen report is dense: ids `0..N`, each once.)
    pub fn from_json(json: &serde_json::Value) -> Result<Self, String> {
        let obj = json
            .as_object()
            .ok_or("blocks.json: root is not an object")?;

        let mut entries: Vec<(u32, String, bool, Box<[(u8, u8)]>)> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut properties: HashMap<String, Vec<(String, Vec<String>)>> = HashMap::new();
        for (block_name, def) in obj {
            names.push(block_name.clone());
            // A block with no properties has no `properties` key at all, so
            // its absence is the empty list rather than an error.
            let props: Vec<(String, Vec<String>)> = def
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|p| {
                    p.iter()
                        .map(|(k, v)| {
                            let values = v
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default();
                            (k.clone(), values)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let states = def
                .get("states")
                .and_then(|s| s.as_array())
                .ok_or_else(|| format!("blocks.json: {block_name} has no states array"))?;
            for state in states {
                let id = state
                    .get("id")
                    .and_then(|i| i.as_u64())
                    .ok_or_else(|| format!("blocks.json: {block_name} state missing id"))?;
                let id = u32::try_from(id)
                    .map_err(|_| format!("blocks.json: {block_name} state id {id} is not a u32"))?;
                let is_default = state
                    .get("default")
                    .and_then(|d| d.as_bool())
                    .unwrap_or(false);
                let values: Box<[(u8, u8)]> = state
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|p| {
                        p.iter()
                            .filter_map(|(k, v)| {
                                let pi = props.iter().position(|(name, _)| name == k)?;
                                let vi = props[pi]
                                    .1
                                    .iter()
                                    .position(|x| Some(x.as_str()) == v.as_str())?;
                                Some((pi as u8, vi as u8))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                entries.push((id, block_name.clone(), is_default, values));
            }
            properties.insert(block_name.clone(), props);
        }
        if entries.is_empty() {
            return Err("blocks.json: no block states found".into());
        }

        // N states, N slots: every id must land in `0..N`, once.
        let count = entries.len();
        let mut slots: Vec<Option<String>> = vec![None; count];
        let mut state_props: Vec<Box<[(u8, u8)]>> = vec![Box::default(); count];
        let mut default_state = HashMap::new();
        for (id, name, is_default, values) in entries {
            let slot = slots.get_mut(id as usize).ok_or_else(|| {
                format!(
                    "blocks.json: state id {id} ({name}) is outside 0..{count}: the ids are \
                     not dense (a gap below it, or a corrupt id)"
                )
            })?;
            if let Some(first) = slot {
                return Err(format!(
                    "blocks.json: state id {id} appears twice ({first} and {name})"
                ));
            }
            if is_default {
                default_state.insert(name.clone(), id);
            }
            *slot = Some(name);
            state_props[id as usize] = values;
        }
        // N entries into N slots with no repeat and no overflow fills every
        // slot, so there is no gap left to find.
        let state_to_block: Vec<String> = slots.into_iter().flatten().collect();
        debug_assert_eq!(state_to_block.len(), count);

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
        names.sort_unstable();
        Ok(Self {
            state_to_block,
            default_state,
            names,
            properties,
            state_props,
            global_palette_bits,
        })
    }

    /// A fixture constructor for tests in **other** crates (M119).
    ///
    /// `#[cfg(test)]` would not reach `rewo-net`, whose command-suggestion
    /// tests need a two-block registry rather than the 32,366-state real one.
    /// Not `#[doc(hidden)]`, because a hidden constructor that panics on
    /// misuse is worse than a documented one that says what it is for.
    pub fn for_tests(
        state_to_block: Vec<String>,
        properties: Vec<(String, Vec<(String, Vec<String>)>)>,
    ) -> Self {
        let mut names: Vec<String> = properties.iter().map(|(n, _)| n.clone()).collect();
        names.sort_unstable();
        let mut default_state = HashMap::new();
        for (id, name) in state_to_block.iter().enumerate() {
            default_state.entry(name.clone()).or_insert(id as u32);
        }
        Self {
            global_palette_bits: ceil_log2(state_to_block.len().max(1) as u32),
            state_to_block,
            default_state,
            names,
            properties: properties.into_iter().collect(),
            state_props: Vec::new(),
        }
    }

    /// Every block name, sorted (M119).
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// A block's properties and their legal values (M119). Empty for a block
    /// with none; `None` for a name that is not a block at all — the two are
    /// different answers and a suggester needs both.
    pub fn properties(&self, block_name: &str) -> Option<&[(String, Vec<String>)]> {
        self.properties.get(block_name).map(|v| v.as_slice())
    }

    pub fn state_count(&self) -> usize {
        self.state_to_block.len()
    }

    pub fn block_name(&self, state_id: u32) -> Option<&str> {
        self.state_to_block
            .get(state_id as usize)
            .map(|s| s.as_str())
    }

    pub fn default_state(&self, block_name: &str) -> Option<u32> {
        self.default_state.get(block_name).copied()
    }

    /// M261 — the state of `block_name` that has every `(key, value)` in
    /// `props` and otherwise the default's values, or `None`. A block's
    /// states are contiguous, so this walks out from the default.
    pub fn state_with(&self, block_name: &str, props: &[(&str, &str)]) -> Option<u32> {
        let default = self.default_state(block_name)?;
        let wanted = |s: u32| -> bool {
            self.block_name(s) == Some(block_name)
                && self
                    .properties(block_name)
                    .into_iter()
                    .flatten()
                    .all(|(k, _)| {
                        let want = props
                            .iter()
                            .find(|(pk, _)| pk == k)
                            .map(|(_, v)| *v)
                            .or_else(|| self.property(default, k));
                        self.property(s, k) == want
                    })
        };
        let mut lo = default;
        while lo > 0 && self.block_name(lo - 1) == Some(block_name) {
            lo -= 1;
        }
        (lo..)
            .take_while(|&s| self.block_name(s) == Some(block_name))
            .find(|&s| wanted(s))
    }

    /// M229 — the value of property `key` in state `state_id` (`"true"`,
    /// `"3"`, `"north"`...), or `None` when the state has no such property.
    pub fn property(&self, state_id: u32, key: &str) -> Option<&str> {
        let block = self.block_name(state_id)?;
        let props = self.properties.get(block)?;
        self.state_props
            .get(state_id as usize)?
            .iter()
            .find(|(pi, _)| props.get(*pi as usize).is_some_and(|(n, _)| n == key))
            .and_then(|(pi, vi)| props[*pi as usize].1.get(*vi as usize))
            .map(String::as_str)
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
    use super::{ceil_log2, Blocks};
    use serde_json::json;

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

    /// A tiny report: each block with the state ids it owns (the first is its
    /// default).
    fn report(states: &[(&str, &[u64])]) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (name, ids) in states {
            let list: Vec<_> = ids
                .iter()
                .enumerate()
                .map(|(i, id)| json!({ "id": id, "default": i == 0 }))
                .collect();
            m.insert((*name).to_string(), json!({ "states": list }));
        }
        serde_json::Value::Object(m)
    }

    /// M229 — a state's own property values, by key, from its `properties`
    /// object; a block-level key the state lacks, or a stateless block, is
    /// `None`.
    #[test]
    fn a_states_property_values_are_read_from_its_own_entry() {
        let json = json!({
            "minecraft:air": { "states": [ { "id": 0, "default": true } ] },
            "minecraft:jukebox": {
                "properties": { "has_record": ["true", "false"] },
                "states": [
                    { "id": 1, "properties": { "has_record": "true" } },
                    { "id": 2, "default": true, "properties": { "has_record": "false" } }
                ]
            },
            "minecraft:cake": {
                "properties": { "bites": ["0", "1", "2"] },
                "states": [
                    { "id": 3, "default": true, "properties": { "bites": "0" } },
                    { "id": 4, "properties": { "bites": "1" } },
                    { "id": 5, "properties": { "bites": "2" } }
                ]
            }
        });
        let b = Blocks::from_json(&json).unwrap();
        assert_eq!(b.property(1, "has_record"), Some("true"));
        assert_eq!(b.property(2, "has_record"), Some("false"));
        assert_eq!(b.property(5, "bites"), Some("2"));
        assert_eq!(b.property(4, "has_record"), None);
        assert_eq!(b.property(0, "bites"), None);
        assert_eq!(b.property(99, "bites"), None);
    }

    #[test]
    fn a_dense_report_loads() {
        let b = Blocks::from_json(&report(&[
            ("minecraft:air", &[0]),
            ("minecraft:stone", &[1]),
            ("minecraft:lever", &[2, 3]),
        ]))
        .unwrap();
        assert_eq!(b.state_count(), 4);
        assert_eq!(b.block_name(3), Some("minecraft:lever"));
        assert_eq!(b.default_state("minecraft:lever"), Some(2));
        assert_eq!(b.global_palette_bits, 2);
    }

    /// **A huge id is an error, not an allocation.** The old code built
    /// `vec![String::new(); max_id + 1]`: four billion `String`s is ~96 GB, so
    /// this report aborted the process (or the allocator) rather than failing
    /// the load. Sized by the entry count, it is rejected at once.
    ///
    /// MUTATION: sizing the table from `max_id + 1` again.
    #[test]
    fn a_huge_id_is_rejected_without_allocating_for_it() {
        let err = Blocks::from_json(&report(&[
            ("minecraft:air", &[0]),
            ("minecraft:stone", &[4_000_000_000]),
        ]))
        .err()
        .expect("must fail");
        assert!(
            err.contains("4000000000") && err.contains("outside"),
            "{err}"
        );
    }

    /// An id past u32 does not wrap into a small one (`as u32` turned
    /// 4294967297 into state 1).
    #[test]
    fn an_id_past_u32_is_not_truncated() {
        let err = Blocks::from_json(&report(&[
            ("minecraft:air", &[0]),
            ("minecraft:stone", &[4_294_967_297]),
        ]))
        .err()
        .expect("must fail");
        assert!(err.contains("not a u32"), "{err}");
    }

    /// A gap shows as an id outside `0..N` (N entries cannot cover a hole),
    /// and a repeated id is named.
    #[test]
    fn a_gap_and_a_duplicate_are_both_errors() {
        // ids 0, 1, 3: three states, so 3 is outside 0..3 and 2 is the hole.
        let gap = Blocks::from_json(&report(&[
            ("minecraft:air", &[0]),
            ("minecraft:stone", &[1]),
            ("minecraft:dirt", &[3]),
        ]))
        .err()
        .expect("gap");
        assert!(gap.contains("not dense"), "{gap}");

        let dup = Blocks::from_json(&report(&[
            ("minecraft:air", &[0]),
            ("minecraft:stone", &[1]),
            ("minecraft:dirt", &[1]),
        ]))
        .err()
        .expect("duplicate");
        assert!(dup.contains("appears twice"), "{dup}");
    }

    /// Air must still be state 0, and an empty report is still an error.
    #[test]
    fn air_must_be_state_zero_and_an_empty_report_fails() {
        let err = Blocks::from_json(&report(&[
            ("minecraft:stone", &[0]),
            ("minecraft:air", &[1]),
        ]))
        .err()
        .expect("air first");
        assert!(err.contains("expected minecraft:air"), "{err}");
        assert!(Blocks::from_json(&json!({})).is_err());
        assert!(Blocks::from_json(&json!([])).is_err());
    }
}
