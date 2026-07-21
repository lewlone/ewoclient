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
//! built-in default list; a malformed file logs a warning and also
//! falls back. Saves are best-effort — we log failures but don't
//! propagate them up because the user shouldn't see a popup just
//! because their disk is full.

use std::fs;
use std::path::PathBuf;

use ewo_render::screens::instances::{default_instances, Instance, InstanceLoader};
use serde::{Deserialize, Serialize};

use crate::bundled;

const INSTANCES_FILENAME: &str = "instances.toml";

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
/// After loading, any Ewo-loader instance has its `mods` list merged
/// against the bundled-mods catalog so the UI shows toggle rows for
/// mods bundled after the instance was created. Persists the synced
/// list back to disk if anything changed.
pub fn load_instances() -> Vec<Instance> {
    let mut instances = load_instances_raw();
    let mut any_synced = false;
    for inst in instances.iter_mut() {
        if matches!(inst.loader, InstanceLoader::Ewo { .. })
            && bundled::sync_mods_with_catalog(&mut inst.mods)
        {
            log::info!(
                "instances: synced \"{}\" mod list with bundled catalog ({} entries)",
                inst.name,
                inst.mods.len()
            );
            any_synced = true;
        }
    }
    if any_synced {
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
                log::warn!("could not parse {}: {} — using defaults", path.display(), e);
                default_instances()
            }
        },
        Err(e) => {
            log::warn!("could not read {}: {} — using defaults", path.display(), e);
            default_instances()
        }
    }
}

/// Persist the instance list to disk. Best-effort: any error is logged
/// but not surfaced. Creates the parent directory if missing.
pub fn save_instances(instances: &[Instance]) {
    let Some(path) = instances_path() else {
        log::warn!("config dir unresolvable — instance list not saved");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            log::warn!("could not create {}: {}", parent.display(), e);
            return;
        }
    }
    let file = InstancesFile {
        instances: instances.to_vec(),
    };
    match toml::to_string_pretty(&file) {
        Ok(s) => {
            if let Err(e) = fs::write(&path, s) {
                log::warn!("could not write {}: {}", path.display(), e);
            } else {
                log::info!("saved {} instance(s) to {}", instances.len(), path.display());
            }
        }
        Err(e) => log::warn!("could not serialize instances: {}", e),
    }
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
        inst.loader = InstanceLoader::Ewo {
            manifest_url: "file:///C:/path/manifest.json".into(),
        };
        let file = InstancesFile { instances: vec![inst] };
        let s = toml::to_string_pretty(&file).expect("serialize");
        let parsed: InstancesFile = toml::from_str(&s).expect("deserialize");
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
}
