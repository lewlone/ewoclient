//! Module config + the Rust→Java module-state channel (Phase G).
//!
//! The launcher and the in-game overlay share the module catalog
//! ([`ewo_core::modules`]). This file is the `ewo-jni` runtime side:
//!
//! - [`ModuleConfig`] — the live enabled/settings state, loaded from and saved
//!   to the active profile's `modules.toml` (a sibling of `hud.toml`, hand-
//!   parsed for the same reason: no `toml` crate in the cdylib).
//! - [`ModuleConfig::write_buffer`] — fills the `EwoModuleData` block, the
//!   Rust→Java half of the bridge. The `ewo-hud` mod reads it every frame to
//!   apply the module effects. The layout is mirrored in `EwoModuleData.java`;
//!   [`SCHEMA_VERSION`] guards the two sides against drift.

use std::path::PathBuf;

use ewo_core::modules as catalog;

/// Layout version of the `EwoModuleData` buffer. Bump on any layout change;
/// `EwoModuleData.SCHEMA_VERSION` on the Java side must match.
///
/// Schema 2 (2026-05): grew `MAX_SETTINGS` 2 → 8 (record stride 16 → 40 bytes)
/// and the buffer `CAPACITY` 256 → 4096 on the Java side. Schema 1 was over-
/// capacity at 17 modules (the 17th record landed past the 256-byte limit
/// causing UB in the unsafe pointer write).
///
/// Schema 3 (2026-05-26, the post-ban refactor): the catalog rebalanced —
/// legit slots 0..11 first, assist slots 12..25 second, deleted three ids,
/// renamed triggerbot → swing_cadence, and the module count became dynamic
/// (offset 4). Record geometry is unchanged from schema 2; what changed is
/// which entries exist and in what order. The Java side was bumped at the
/// time and this side was not, so `EwoModuleData.ready()` sat permanently
/// false for ~3 months (reads are not gated on `ready()`, which is why
/// nothing failed). Aligned to 3 on 2026-08-23.
pub const SCHEMA_VERSION: i32 = 3;

/// First record offset — past the `i32 schema` + `i32 count` header.
const OFF_RECORDS: usize = 8;
/// Per-module record stride: `i32 enabled`, `MAX_SETTINGS × f32`, `i32 reserved`.
/// Schema 2: 4 + 8*4 + 4 = 40 bytes.
const RECORD: usize = 8 + catalog::MAX_SETTINGS * 4;

/// One module's live state.
#[derive(Clone, Copy)]
pub struct ModuleState {
    pub enabled: bool,
    pub settings: [f32; catalog::MAX_SETTINGS],
}

/// Live module config — one [`ModuleState`] per [`ewo_core::modules::REGISTRY`]
/// entry, in registry order. Loaded from / saved to the active profile's
/// `modules.toml`.
pub struct ModuleConfig {
    states: Vec<ModuleState>,
}

impl ModuleConfig {
    /// Every module at its catalog default.
    pub fn defaults() -> Self {
        let states = catalog::REGISTRY
            .iter()
            .map(|m| {
                let mut settings = [0.0_f32; catalog::MAX_SETTINGS];
                for (slot, value) in settings.iter_mut().enumerate() {
                    *value = m.setting_default(slot);
                }
                ModuleState {
                    enabled: m.default_enabled,
                    settings,
                }
            })
            .collect();
        ModuleConfig { states }
    }

    /// Load the active profile's `modules.toml`, falling back to the catalog
    /// default for any module or setting absent or malformed — a hand-edited
    /// or missing file never breaks the modules.
    pub fn load() -> Self {
        let mut cfg = Self::defaults();
        let Some(text) = modules_toml_path().and_then(|p| std::fs::read_to_string(p).ok())
        else {
            return cfg;
        };
        let mut current: Option<usize> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                current = catalog::index_of(section);
                continue;
            }
            let (Some((key, value)), Some(idx)) = (line.split_once('='), current) else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if key == "enabled" {
                cfg.states[idx].enabled = value == "true";
            } else if let Some(slot) =
                catalog::REGISTRY[idx].settings.iter().position(|s| s.id == key)
            {
                if let Ok(v) = value.parse::<f32>() {
                    cfg.states[idx].settings[slot] = v;
                }
            }
        }
        cfg
    }

    /// Write the active profile's `modules.toml`.
    pub fn save(&self) {
        let Some(path) = modules_toml_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut s = String::from("# EwoClient modules — per client profile.\n");
        for (idx, def) in catalog::REGISTRY.iter().enumerate() {
            let st = self.states[idx];
            s.push_str(&format!("\n[{}]\nenabled = {}\n", def.id, st.enabled));
            for (slot, setting) in def.settings.iter().enumerate() {
                s.push_str(&format!("{} = {}\n", setting.id, st.settings[slot]));
            }
        }
        let _ = std::fs::write(&path, s);
    }

    /// Flip module `idx`'s enabled flag and persist. No-op out of range.
    pub fn toggle(&mut self, idx: usize) {
        if let Some(st) = self.states.get_mut(idx) {
            st.enabled = !st.enabled;
            self.save();
        }
    }

    /// State of module `idx` — a disabled default past the registry.
    pub fn get(&self, idx: usize) -> ModuleState {
        self.states.get(idx).copied().unwrap_or(ModuleState {
            enabled: false,
            settings: [0.0; catalog::MAX_SETTINGS],
        })
    }

    /// Set module `idx`'s setting `slot`. Does not persist on its own — call
    /// [`ModuleConfig::save`] when the edit completes (e.g. on slider-release),
    /// so a drag doesn't write the file on every frame.
    pub fn set_setting(&mut self, idx: usize, slot: usize, value: f32) {
        if let Some(st) = self.states.get_mut(idx) {
            if slot < catalog::MAX_SETTINGS {
                st.settings[slot] = value;
            }
        }
    }

    /// Fill the `EwoModuleData` block at `base` — the Rust→Java channel. The
    /// `ewo-hud` mod reads it every frame to apply the module effects.
    ///
    /// # Safety
    /// `base` must point to at least `EwoModuleData.CAPACITY` writable bytes.
    pub unsafe fn write_buffer(&self, base: *mut u8) {
        write_i32(base, 0, SCHEMA_VERSION);
        write_i32(base, 4, self.states.len() as i32);
        for (i, st) in self.states.iter().enumerate() {
            let rec = OFF_RECORDS + i * RECORD;
            write_i32(base, rec, st.enabled as i32);
            for (slot, value) in st.settings.iter().enumerate() {
                write_f32(base, rec + 4 + slot * 4, *value);
            }
            write_i32(base, rec + 4 + catalog::MAX_SETTINGS * 4, 0); // reserved
        }
    }
}

/// Write an `i32` at `base + off` (unaligned-safe — the direct `ByteBuffer` is
/// native byte order, so no swap is needed).
unsafe fn write_i32(base: *mut u8, off: usize, v: i32) {
    (base.add(off) as *mut i32).write_unaligned(v);
}
/// Write an `f32` at `base + off` (unaligned-safe).
unsafe fn write_f32(base: *mut u8, off: usize, v: f32) {
    (base.add(off) as *mut f32).write_unaligned(v);
}

/// `%APPDATA%/EwoClient/profiles/<active>/modules.toml` — per client profile,
/// resolved like `hud.rs`'s `hud_toml_path`.
fn modules_toml_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let profile = crate::hud::read_active_profile().unwrap_or_else(|| "Default".to_string());
    Some(
        PathBuf::from(appdata)
            .join("EwoClient")
            .join("profiles")
            .join(profile)
            .join("modules.toml"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_layout_matches_the_schema() {
        let cfg = ModuleConfig::defaults();
        // write_buffer's safety contract requires at least the Java side's
        // `EwoModuleData.CAPACITY` (4096). The old 256-byte fixture overflowed
        // twice over — 12 legit records × 40 B + header = 488 bytes written,
        // and reads up to offset 448 — which corrupted the stack and killed
        // the test process abnormally rather than failing an assert.
        const CAPACITY: usize = 4096;
        let mut buf = [0u8; CAPACITY];
        unsafe { cfg.write_buffer(buf.as_mut_ptr()) };
        let needed = OFF_RECORDS + catalog::REGISTRY.len() * RECORD;
        assert!(
            needed <= CAPACITY,
            "registry outgrew the Java-side capacity: {} > {}",
            needed,
            CAPACITY
        );
        let i32_at = |o: usize| i32::from_ne_bytes(buf[o..o + 4].try_into().unwrap());
        assert_eq!(i32_at(0), SCHEMA_VERSION);
        assert_eq!(i32_at(4), catalog::REGISTRY.len() as i32);
        for (i, m) in catalog::REGISTRY.iter().enumerate() {
            let rec = OFF_RECORDS + i * RECORD;
            assert_eq!(i32_at(rec) != 0, m.default_enabled, "module {}", m.id);
        }
    }

    /// The Java reader (`ingame-mod/src/main/java/dev/lewlone/ewohud/
    /// EwoModuleData.java`) hard-codes the matching constant in `ready()`.
    /// Bump BOTH sides together: this pin exists because the 2026-05-26
    /// post-ban bump was applied Java-side only, and nothing failed for three
    /// months — a unilateral bump here would silently do it again.
    #[test]
    fn schema_version_matches_the_java_mirror() {
        assert_eq!(SCHEMA_VERSION, 3);
    }

    #[test]
    fn defaults_carry_catalog_setting_values() {
        let cfg = ModuleConfig::defaults();
        let fov = catalog::index_of("fov").expect("fov module");
        // FOV Control's one slider defaults to 90°.
        assert_eq!(cfg.states[fov].settings[0], 90.0);
    }
}
