//! Disk persistence — load/save the launcher's instance list.
//!
//! Settings persistence moved to the `profile` module in Phase F (it's
//! split across `settings.toml` + the active profile's `client.toml`).
//! This module is now instances only.
//!
//! Single TOML file at `<config>/EwoClient/instances.toml` where
//! `<config>` is `%APPDATA%` on Windows / `$XDG_CONFIG_HOME` (or
//! `~/.config`) on Linux, per the `dirs` crate.
//!
//! Failure modes are non-fatal: a missing file falls back to the
//! built-in default list; a malformed file is backed up to
//! `instances.toml.bak-<ts>` and also falls back. Saves are atomic and
//! best-effort — we log failures but don't propagate them up because the
//! user shouldn't see a popup just because their disk is full.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use ewo_render::screens::instances::{default_instances, Instance, InstanceLoader};
use serde::{Deserialize, Serialize};

use crate::bundled;
use crate::downloads::paths;
use crate::util;

const INSTANCES_FILENAME: &str = "instances.toml";

/// Longest instance display name (chars).
pub const NAME_MAX_LEN: usize = 48;

#[derive(Debug, Serialize, Deserialize)]
struct InstancesFile {
    #[serde(default)]
    instances: Vec<Instance>,
}

/// Resolve the on-disk path for the instances file. `None` only when the
/// platform's config dir is unresolvable (very rare — headless server,
/// no $HOME, etc.).
fn instances_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push(INSTANCES_FILENAME);
    Some(p)
}

/// Load the persisted instances list, or fall back to the built-in
/// defaults. Always returns at least one instance.
///
/// Every instance gets a stable folder `id` (see [`assign_ids`]), and any
/// Ewo-loader instance has its `mods` list merged against the bundled-mods
/// catalog so the UI shows toggle rows for mods bundled after the instance
/// was created. Persists the list back to disk if anything changed.
pub fn load_instances() -> Vec<Instance> {
    let mut instances = load_instances_raw();
    let mut any_changed = assign_ids(&mut instances);
    for inst in instances.iter_mut() {
        if matches!(inst.loader, InstanceLoader::Ewo { .. })
            && bundled::sync_mods_with_catalog(&mut inst.mods)
        {
            log::info!(
                "instances: synced \"{}\" mod list with bundled catalog ({} entries)",
                inst.name,
                inst.mods.len()
            );
            any_changed = true;
        }
    }
    if any_changed {
        save_instances(&instances);
    }
    instances
}

fn load_instances_raw() -> Vec<Instance> {
    let Some(path) = instances_path() else {
        log::warn!("config dir unresolvable — using default instances");
        return default_instances();
    };
    if !path.exists() {
        log::info!(
            "no persisted instances at {} — using defaults",
            path.display()
        );
        return default_instances();
    }
    match fs::read_to_string(&path) {
        Ok(s) => match toml::from_str::<InstancesFile>(&s) {
            Ok(file) if !file.instances.is_empty() => {
                log::info!(
                    "loaded {} instance(s) from {}",
                    file.instances.len(),
                    path.display()
                );
                file.instances
            }
            Ok(_) => {
                log::info!("persisted instances file empty — using defaults");
                default_instances()
            }
            Err(e) => {
                // Keep the user's real file: the next save would otherwise
                // overwrite it with the defaults.
                util::backup_unparseable(&path, &e);
                default_instances()
            }
        },
        Err(e) => {
            log::warn!("could not read {}: {} — using defaults", path.display(), e);
            default_instances()
        }
    }
}

/// Persist the instance list to disk (atomically). Best-effort: any error
/// is logged but not surfaced.
pub fn save_instances(instances: &[Instance]) {
    let Some(path) = instances_path() else {
        log::warn!("config dir unresolvable — instance list not saved");
        return;
    };
    let file = InstancesFile {
        instances: instances.to_vec(),
    };
    match toml::to_string_pretty(&file) {
        Ok(s) => {
            if let Err(e) = util::atomic_write(&path, s.as_bytes()) {
                log::warn!("could not write {}: {}", path.display(), e);
            } else {
                log::info!("saved {} instance(s) to {}", instances.len(), path.display());
            }
        }
        Err(e) => log::warn!("could not serialize instances: {}", e),
    }
}

/// Validate a user-typed instance display name. Returns the trimmed name.
pub fn validate_instance_name(name: &str) -> Result<&str, util::NameError> {
    util::validate_name(name, NAME_MAX_LEN)
}

/// Give every instance a stable, unique, path-safe `id` (its folder name
/// under `instances/`). Returns `true` if anything changed.
///
/// Migration: an instance saved before ids existed keeps using its old
/// name-keyed folder — its id becomes that name when the name is path-safe
/// and `instances/<name>/` exists, so worlds stay in place. Everything else
/// (new instances, unsafe names, duplicates) gets a fresh id.
pub fn assign_ids(instances: &mut [Instance]) -> bool {
    let root = paths::instances_root();
    let folder_exists = |id: &str| root.as_ref().is_some_and(|r| r.join(id).exists());
    assign_ids_with(instances, &folder_exists)
}

fn assign_ids_with(instances: &mut [Instance], folder_exists: &dyn Fn(&str) -> bool) -> bool {
    // Case-insensitive, because NTFS is.
    let mut used: HashSet<String> = HashSet::new();
    let mut changed = false;
    // Keep ids that are already valid and unique.
    for inst in instances.iter_mut() {
        if inst.id.is_empty() {
            continue;
        }
        let ok = paths::instance_dir(&inst.id).is_some() && used.insert(inst.id.to_lowercase());
        if !ok {
            log::warn!("instances: dropping invalid/duplicate id \"{}\"", inst.id);
            inst.id.clear();
            changed = true;
        }
    }
    for inst in instances.iter_mut().filter(|i| i.id.is_empty()) {
        let legacy = util::validate_name(&inst.name, 64)
            .ok()
            .filter(|n| *n == inst.name && folder_exists(n))
            .filter(|n| !used.contains(&n.to_lowercase()));
        let id = match legacy {
            Some(n) => n.to_string(),
            None => fresh_id(&inst.name, &used, folder_exists),
        };
        log::info!("instances: \"{}\" -> folder instances/{}", inst.name, id);
        used.insert(id.to_lowercase());
        inst.id = id;
        changed = true;
    }
    changed
}

/// A new id `<slug>-<6 hex>` that no instance uses and no folder occupies.
fn fresh_id(name: &str, used: &HashSet<String>, folder_exists: &dyn Fn(&str) -> bool) -> String {
    let slug = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>();
    let slug: String = slug
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(32)
        .collect();
    let slug = if slug.is_empty() { "instance".to_string() } else { slug };
    loop {
        let id = format!("{}-{:06x}", slug, rand::random::<u32>() & 0xff_ffff);
        if paths::instance_dir(&id).is_some()
            && !used.contains(&id.to_lowercase())
            && !folder_exists(&id)
        {
            return id;
        }
    }
}

/// A fresh id for a newly created instance, unique against `instances` and
/// the real `instances/` tree — so a new instance never inherits the folder
/// of a deleted one with the same name.
pub fn new_instance_id(name: &str, instances: &[Instance]) -> String {
    let used: HashSet<String> = instances.iter().map(|i| i.id.to_lowercase()).collect();
    let root = paths::instances_root();
    let folder_exists = |id: &str| root.as_ref().is_some_and(|r| r.join(id).exists());
    fresh_id(name, &used, &folder_exists)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ewo_render::screens::instances::InstanceLoader;

    /// Pre-loader-field `instances.toml` content must still parse — the
    /// `loader` field is `#[serde(default)]` so missing entries get
    /// `InstanceLoader::Vanilla`.
    #[test]
    fn legacy_instances_file_parses_as_vanilla() {
        let legacy = r#"
[[instances]]
name = "Velvet Hours"
version = "VANILLA · 1.21"
last_played = "moments ago"
last_played_at = 0.0
ram = 8
render_distance = 16
java_runtime = 0
status = "ready"
mods = []
"#;
        let parsed: InstancesFile =
            toml::from_str(legacy).expect("legacy instances.toml must parse");
        assert_eq!(parsed.instances.len(), 1);
        assert_eq!(parsed.instances[0].loader, InstanceLoader::Vanilla);
        assert!(parsed.instances[0].id.is_empty(), "pre-id files have no id");
    }

    /// Round-trip a fresh instance with `InstanceLoader::Ewo` through
    /// TOML serialize → deserialize and confirm both variant payload
    /// and other fields survive intact.
    #[test]
    fn ewo_loader_round_trips_through_toml() {
        let mut inst = Instance::new(
            "ewo-world".into(),
            "VANILLA · 26.1".into(),
            "now".into(),
            vec![],
        );
        inst.id = "ewo-world-abc123".into();
        inst.loader = InstanceLoader::Ewo {
            manifest_url: "file:///C:/path/manifest.json".into(),
        };
        let file = InstancesFile { instances: vec![inst] };
        let s = toml::to_string_pretty(&file).expect("serialize");
        let parsed: InstancesFile = toml::from_str(&s).expect("deserialize");
        assert_eq!(parsed.instances[0].id, "ewo-world-abc123");
        match &parsed.instances[0].loader {
            InstanceLoader::Ewo { manifest_url } => {
                assert_eq!(manifest_url, "file:///C:/path/manifest.json");
            }
            other => panic!("expected Ewo, got {:?}", other),
        }
    }

    /// `InstanceLoader::Native` (Rewo) is a unit variant — confirm it
    /// TOML-round-trips like Vanilla does.
    #[test]
    fn native_loader_round_trips_through_toml() {
        let mut inst = Instance::new(
            "rewo-world".into(),
            "NATIVE · 26.2".into(),
            "now".into(),
            vec![],
        );
        inst.loader = InstanceLoader::Native;
        let file = InstancesFile { instances: vec![inst] };
        let s = toml::to_string_pretty(&file).expect("serialize");
        let parsed: InstancesFile = toml::from_str(&s).expect("deserialize");
        assert_eq!(parsed.instances[0].loader, InstanceLoader::Native);
    }

    fn named(name: &str) -> Instance {
        Instance::new(name.into(), "VANILLA · 26.2".into(), "now".into(), vec![])
    }

    #[test]
    fn legacy_instance_keeps_its_existing_name_folder() {
        let mut v = vec![named("Velvet Hours"), named("New World")];
        let exists = |id: &str| id == "Velvet Hours";
        assert!(assign_ids_with(&mut v, &exists));
        assert_eq!(v[0].id, "Velvet Hours");
        assert!(v[1].id.starts_with("new-world-"), "{}", v[1].id);
        // Idempotent once assigned.
        assert!(!assign_ids_with(&mut v, &exists));
    }

    #[test]
    fn unsafe_or_duplicate_names_get_fresh_ids() {
        let mut v = vec![named("../../evil"), named("dup"), named("DUP")];
        assign_ids_with(&mut v, &|id: &str| id == "dup" || id == "../../evil");
        assert!(!v[0].id.contains(".."), "{}", v[0].id);
        assert!(paths::instance_dir(&v[0].id).is_some());
        assert_eq!(v[1].id, "dup");
        assert_ne!(v[2].id.to_lowercase(), "dup");
    }

    #[test]
    fn invalid_stored_id_is_replaced() {
        let mut inst = named("x");
        inst.id = "../x".into();
        let mut v = vec![inst];
        assert!(assign_ids_with(&mut v, &|_: &str| false));
        assert!(paths::instance_dir(&v[0].id).is_some());
    }

    #[test]
    fn instance_names_are_validated() {
        assert_eq!(validate_instance_name("  My World "), Ok("My World"));
        assert!(validate_instance_name("a/b").is_err());
        assert!(validate_instance_name("..").is_err());
        assert!(validate_instance_name("AUX").is_err());
    }
}
