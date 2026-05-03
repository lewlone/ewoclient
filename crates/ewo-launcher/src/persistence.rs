//! Disk persistence — load/save the launcher's instance list.
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

use ewo_render::screens::instances::{default_instances, Instance};
use ewo_render::screens::SettingsConfig;
use serde::{Deserialize, Serialize};

const INSTANCES_FILENAME: &str = "instances.toml";
const SETTINGS_FILENAME: &str = "settings.toml";

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

fn settings_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push(SETTINGS_FILENAME);
    Some(p)
}

/// Load the persisted instances list, or fall back to the built-in
/// defaults. Always returns at least one instance.
pub fn load_instances() -> Vec<Instance> {
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

/// Load the persisted Settings config, or return the bundled defaults.
pub fn load_settings() -> SettingsConfig {
    let Some(path) = settings_path() else {
        return SettingsConfig::default();
    };
    if !path.exists() {
        return SettingsConfig::default();
    }
    match fs::read_to_string(&path) {
        Ok(s) => match toml::from_str::<SettingsConfig>(&s) {
            Ok(c) => {
                log::info!("loaded settings from {}", path.display());
                c
            }
            Err(e) => {
                log::warn!("could not parse {}: {} — using defaults", path.display(), e);
                SettingsConfig::default()
            }
        },
        Err(e) => {
            log::warn!("could not read {}: {} — using defaults", path.display(), e);
            SettingsConfig::default()
        }
    }
}

/// Persist the Settings config to disk. Best-effort; errors are logged.
pub fn save_settings(config: &SettingsConfig) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            log::warn!("could not create {}: {}", parent.display(), e);
            return;
        }
    }
    match toml::to_string_pretty(config) {
        Ok(s) => {
            if let Err(e) = fs::write(&path, s) {
                log::warn!("could not write {}: {}", path.display(), e);
            }
        }
        Err(e) => log::warn!("could not serialize settings: {}", e),
    }
}
