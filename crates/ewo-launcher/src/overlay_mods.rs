//! The launcher↔in-game-overlay bridge — per-instance files the launcher
//! writes before launch and the in-game mod reads (Phase E6 + F5c).
//!
//! Two small files in the per-instance directory carry mod state between the
//! launcher and the in-game MODS view:
//!
//! - **`overlay-mods.toml`** — the launcher writes it before each launch, from
//!   the catalog + the instance's current state. The overlay *reads* it to
//!   populate the MODS list.
//! - **`overlay-mod-overrides.toml`** — the overlay writes it when the user
//!   toggles a mod (only the mods that changed). The launcher *consumes* it on
//!   the next launch: applies the toggles to the instance, then deletes it.
//!
//! So an in-game toggle takes effect on the next launch, and the override file
//! is a delta — it never clobbers a toggle made in the launcher UI meanwhile.

use std::path::PathBuf;

use ewo_render::screens::instances::Instance;

use crate::bundled;
use crate::downloads::paths;

fn overlay_mods_path(instance_name: &str) -> Option<PathBuf> {
    paths::instance_dir(instance_name).map(|d| d.join("overlay-mods.toml"))
}

fn overrides_path(instance_name: &str) -> Option<PathBuf> {
    paths::instance_dir(instance_name).map(|d| d.join("overlay-mod-overrides.toml"))
}

/// Consume `overlay-mod-overrides.toml` for `instances[idx]` — apply the
/// in-game mod toggles to the instance, then delete the file. Returns `true`
/// if the instance's mod state actually changed (the caller persists).
pub fn apply_overrides(instances: &mut [Instance], idx: usize) -> bool {
    let Some(inst) = instances.get(idx) else {
        return false;
    };
    let Some(path) = overrides_path(&inst.name) else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let _ = std::fs::remove_file(&path); // consume it whether or not it parses

    let mut changed = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((mod_id, value)) = line.split_once('=') else {
            continue;
        };
        let mod_id = mod_id.trim();
        let want_on = value.trim() == "true";
        // mod id → catalog display name → the instance's ModInfo row.
        let Some(cat) = bundled::CATALOG.iter().find(|m| m.mod_id == mod_id) else {
            continue;
        };
        if let Some(row) = instances[idx]
            .mods
            .iter_mut()
            .find(|m| m.name == cat.display_name)
        {
            if row.on != want_on {
                row.on = want_on;
                changed = true;
                log::info!("overlay: in-game toggle — {} {}", mod_id, want_on);
            }
        }
    }
    changed
}

/// Write `ewo-keybinds.txt` into `instance_name`'s directory — the active
/// client profile's keybinds, resolved to GLFW key codes. The in-game mod
/// reads it for the overlay-open key (Phase F5c).
///
/// Format is one `action=code` (or `action=code:mods`) line, plain enough
/// for the mod to parse with a line split — no TOML parser in the JVM.
pub fn write_keybinds(instance_name: &str) {
    let Some(dir) = paths::instance_dir(instance_name) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let mut s = String::from(
        "# EwoClient keybinds — written by the launcher for the in-game mod.\n",
    );
    for (id, chord) in crate::profile::load_keybinds() {
        if chord.mods == 0 {
            s.push_str(&format!("{}={}\n", id, chord.key));
        } else {
            s.push_str(&format!("{}={}:{}\n", id, chord.key, chord.mods));
        }
    }
    let _ = std::fs::write(dir.join("ewo-keybinds.txt"), s);
}

/// Write `overlay-mods.toml` for `instance` — every toggleable catalog mod
/// with the instance's current on/off state. The in-game MODS view reads this.
pub fn write_catalog(instance: &Instance) {
    let Some(path) = overlay_mods_path(&instance.name) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut s = String::from(
        "# EwoClient bundled mods — written by the launcher for the in-game overlay.\n",
    );
    for m in bundled::CATALOG.iter().filter(|m| m.toggleable) {
        let enabled = instance
            .mods
            .iter()
            .find(|r| r.name == m.display_name)
            .map(|r| r.on)
            .unwrap_or(m.default_on);
        s.push_str(&format!(
            "\n[{}]\nname = \"{}\"\ncategory = \"{}\"\nversion = \"{}\"\nenabled = {}\n",
            m.mod_id, m.display_name, m.category, m.version, enabled,
        ));
    }
    let _ = std::fs::write(&path, s);
}
