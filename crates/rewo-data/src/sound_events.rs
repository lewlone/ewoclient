//! registries.json → sound-event table (M64).
//!
//! M63 decoded the three sound packets but left
//! [`rewo_net::sounds::SoundRef::Registry`] as a bare number, because turning
//! it into `"minecraft:block.stone.break"` is a report lookup rather than a
//! wire concern. This is that lookup. It is still **data only** — nothing here
//! opens an audio device, and a name is not a sample.
//!
//! ## Why the report, and why the id is the key
//!
//! `minecraft:sound_event` is a **built-in** registry: its contents and its id
//! order are baked into the client/server jar, not sent by the server (unlike
//! `minecraft:enchantment`, whose order arrives in Configuration — see M42).
//! So the datagen report is the ground truth, and each entry's `protocol_id`
//! **is** the wire value that [`SoundRef::Registry`] carries.
//!
//! **The trap this module is written to avoid:** `serde_json`'s default `Map`
//! is a sorted `BTreeMap` (the workspace `Cargo.toml` notes this next to the
//! `indexmap` dep, which exists for exactly that reason), so iterating
//! `entries` yields the registry in *alphabetical* order. Building the table
//! by `enumerate()`ing that iteration would look completely reasonable and be
//! wrong: the real 26.2 registry opens with the seven `entity.allay.*` events
//! at ids 0..6 and only reaches `ambient.cave` at id 7, where alphabetical
//! order would put `ambient.basalt_deltas.additions` first. Every id would be
//! shifted by a different amount, so nothing would fail loudly — you would
//! just get the wrong sound name for every sound, which a decode gate cannot
//! see and only a listener would notice. This module therefore reads
//! `protocol_id` off each entry and never derives an id from position.
//!
//! Naming: `SoundEvents` here is the **registry table**;
//! `rewo_net::sounds::SoundEvent` is a decoded *request* off the wire (play
//! this / stop that). Both names come from upstream — vanilla's registry
//! element type really is `net.minecraft.sounds.SoundEvent` — so they are kept
//! rather than invented apart.
//!
//! Ground truth: `<data_dir>/rewo/26.2/datagen/generated/reports/registries.json`,
//! key `minecraft:sound_event`.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

/// The `minecraft:sound_event` registry: protocol id ↔ registry name.
#[derive(Clone, Default)]
pub struct SoundEvents {
    by_id: HashMap<i32, String>,
    /// The reverse direction, built eagerly. A linear scan over ~2,000 entries
    /// per resolved sound would be the wrong shape for a path that runs once
    /// per audible event; the map costs one extra allocation at load.
    by_name: HashMap<String, i32>,
}

impl SoundEvents {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:sound_event")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:sound_event registry")?;
        let mut by_id = HashMap::with_capacity(entries.len());
        let mut by_name = HashMap::with_capacity(entries.len());
        for (name, entry) in entries {
            // `protocol_id`, never the iteration index — see the module note.
            if let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) {
                by_id.insert(id as i32, name.clone());
                by_name.insert(name.clone(), id as i32);
            }
        }
        log::info!("rewo-data: {} sound events", by_id.len());
        Ok(Self { by_id, by_name })
    }

    /// Registry name for a protocol id, e.g. `1596` →
    /// `"minecraft:block.stone.break"`.
    ///
    /// `None` means this version does not register that id — a real state, not
    /// a decode failure, and one a caller must not paper over with a default.
    /// A server on a newer protocol can name a sound this jar has never heard
    /// of; playing *something* in its place would be a wrong sound rather than
    /// a missing one, and wrong is harder to notice than absent.
    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(|s| s.as_str())
    }

    /// Protocol id for a registry name. The reverse of [`Self::name`], for the
    /// paths that start from a name: `stop_sound` carries a plain identifier
    /// rather than a holder (M63), and a mixer keyed by id needs it back.
    pub fn id_of(&self, name: &str) -> Option<i32> {
        self.by_name.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture in the shape of the real registry's head: the `allay` block
    /// occupies ids 0..2 and `ambient.cave` sits *after* them at 3, even
    /// though it sorts first. Written to disk and read back through the
    /// production `load`, so these tests exercise the real parse.
    fn fixture(dir_name: &str) -> SoundEvents {
        let dir = std::env::temp_dir().join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("registries.json");
        std::fs::write(
            &p,
            r#"{"minecraft:sound_event":{"entries":{
                 "minecraft:entity.allay.ambient_with_item":{"protocol_id":0},
                 "minecraft:entity.allay.ambient_without_item":{"protocol_id":1},
                 "minecraft:entity.allay.death":{"protocol_id":2},
                 "minecraft:ambient.cave":{"protocol_id":3},
                 "minecraft:ambient.basalt_deltas.additions":{"protocol_id":4}}}}"#,
        )
        .unwrap();
        SoundEvents::load(&p).unwrap()
    }

    #[test]
    fn a_sound_events_id_is_the_reports_protocol_id_not_its_position() {
        let t = fixture("rewo-sound-events-order");
        // `serde_json` hands `entries` back sorted, so position order is
        // alphabetical: ambient.basalt(0), ambient.cave(1), allay.ambient_with(2),
        // allay.ambient_without(3), allay.death(4). Every one of those disagrees
        // with the registry.
        assert_eq!(t.name(0), Some("minecraft:entity.allay.ambient_with_item"));
        assert_eq!(t.name(3), Some("minecraft:ambient.cave"));
        assert_eq!(t.name(4), Some("minecraft:ambient.basalt_deltas.additions"));
    }

    /// The anti-alphabetisation witness, stated as the property rather than as
    /// a list: assign ids by sorted-name order and the result must *not* match
    /// the table. If the registry ever did happen to be alphabetical this test
    /// would be vacuous, so it asserts a disagreement exists rather than
    /// asserting one particular entry moved.
    #[test]
    fn assigning_ids_alphabetically_would_disagree_with_the_table() {
        let t = fixture("rewo-sound-events-alpha");
        let mut names: Vec<&str> = t.by_name.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        let disagreements = names
            .iter()
            .enumerate()
            .filter(|(i, n)| t.name(*i as i32) != Some(**n))
            .count();
        assert!(
            disagreements > 0,
            "alphabetical id assignment agreed with the registry everywhere — \
             the table is no longer keyed by protocol_id"
        );
    }

    #[test]
    fn an_unregistered_id_is_none_rather_than_a_substituted_default() {
        let t = fixture("rewo-sound-events-missing");
        assert_eq!(t.name(5), None);
        assert_eq!(t.name(-1), None);
        assert_eq!(t.id_of("minecraft:not.a.sound"), None);
    }

    #[test]
    fn id_of_is_the_exact_inverse_of_name_for_every_entry() {
        let t = fixture("rewo-sound-events-inverse");
        assert_eq!(t.len(), 5);
        assert!(!t.is_empty());
        for id in 0..5 {
            let n = t.name(id).unwrap();
            assert_eq!(t.id_of(n), Some(id));
        }
    }

    #[test]
    fn a_registries_json_without_the_registry_fails_rather_than_loading_empty() {
        let dir = std::env::temp_dir().join("rewo-sound-events-absent");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("registries.json");
        std::fs::write(&p, r#"{"minecraft:item":{"entries":{}}}"#).unwrap();
        assert!(SoundEvents::load(&p).is_err());
    }

    /// Pins against the **real** 26.2 report when it is present. The report is
    /// git-ignored (derived from the user's own client download, per
    /// CLAUDE.md), so a clean checkout has nothing to read and the test says so
    /// loudly instead of asserting on an empty table.
    ///
    /// These numbers are deliberately version-specific: a jar bump that
    /// renumbers the registry *should* land here rather than silently shifting
    /// every sound name.
    #[test]
    fn the_real_report_pins_these_ids_to_these_names() {
        let Some(paths) = crate::DataPaths::for_version("26.2") else {
            eprintln!("SKIP: no config dir");
            return;
        };
        let p = paths.registries_json();
        if !p.exists() {
            eprintln!("SKIP: no datagen report at {}", p.display());
            return;
        }
        let t = SoundEvents::load(&p).unwrap();
        // Ids 0 and 7 are the pair that catches an alphabetised table: 0 is an
        // `entity.*` name and 7 is `ambient.cave`, which sorts fourth.
        for (id, name) in [
            (0, "minecraft:entity.allay.ambient_with_item"),
            (7, "minecraft:ambient.cave"),
            (8, "minecraft:ambient.basalt_deltas.additions"),
            (1596, "minecraft:block.stone.break"),
            (1668, "minecraft:ui.button.click"),
            (1967, "minecraft:entity.small_sulfur_cube.eat"),
        ] {
            assert_eq!(t.name(id), Some(name), "sound id {id}");
            assert_eq!(t.id_of(name), Some(id), "sound {name}");
        }
        // Dense 0..n, so the count is also the exclusive upper bound on any
        // id the wire can legally carry.
        assert_eq!(t.len(), 1968);
        assert_eq!(t.name(1968), None);
    }
}
