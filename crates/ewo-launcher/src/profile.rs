//! Client profiles — Phase F.
//!
//! A *client profile* is a named, hot-swappable bundle of cosmetic + perf
//! settings. Multiple profiles let a user keep, say, a "PvP" look and a
//! "Cinematic" look and switch between them. Profiles are global (not
//! per-instance) and orthogonal to accounts.
//!
//! Disk layout under `<config>/EwoClient/`:
//! ```text
//! profiles.toml                profile registry { active, profiles[] }
//! profiles/<name>/client.toml  the profile-scoped config
//! settings.toml                GLOBAL config (paths, window mode, log level)
//! ```
//!
//! The **profile-scoped** slice is the cosmetic + perf-feel settings: the
//! five tweak tokens (motion / breath / density / warmth / accent-hue),
//! plus theme, vsync, max-fps and audio levels. The **global** slice — one
//! value shared by every profile — is the install paths, window mode,
//! auto-backup, log level and telemetry.
//!
//! A pre-F `settings.toml` held the whole `SettingsConfig` in one file;
//! `load` migrates it: the profile slice moves into `profiles/Default/`
//! and `settings.toml` is rewritten with just the global slice.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use ewo_core::Settings;
use ewo_render::screens::SettingsConfig;
use serde::{Deserialize, Serialize};

use crate::keybind::{self, KeyChord};

const PROFILES_DIRNAME: &str = "profiles";
const INDEX_FILENAME: &str = "profiles.toml";
const SETTINGS_FILENAME: &str = "settings.toml";
const CLIENT_FILENAME: &str = "client.toml";
const DEFAULT_PROFILE: &str = "Default";

/// The profile-scoped config — one `client.toml` per profile. Field order
/// is grouped: the five cosmetic tweak tokens, then theme / perf / audio.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ClientProfile {
    motion_speed: f32,
    breath_amp: f32,
    density: f32,
    warmth: f32,
    accent_hue_shift: f32,
    theme: usize,
    vsync: bool,
    max_fps: f32,
    master: f32,
    music: f32,
    effects: f32,
    ambient_hum: bool,
    /// Per-profile keybinds, keyed by [`keybind::KeybindAction::id`]. Any
    /// action absent here falls back to its registry default — so a fresh
    /// or older `client.toml` (no `[keybinds]` table) still binds every key.
    #[serde(default)]
    keybinds: BTreeMap<String, KeyChord>,
}

impl Default for ClientProfile {
    fn default() -> Self {
        Self {
            motion_speed: 1.0,
            breath_amp: 1.0,
            density: 1.0,
            warmth: 0.6,
            accent_hue_shift: 0.0,
            theme: 0,
            vsync: true,
            max_fps: 144.0,
            master: 0.70,
            music: 0.55,
            effects: 0.50,
            ambient_hum: true,
            keybinds: BTreeMap::new(),
        }
    }
}

impl ClientProfile {
    /// The cosmetic tweak tokens this profile carries.
    fn tokens(&self) -> Settings {
        Settings {
            motion_speed: self.motion_speed,
            breath_amp: self.breath_amp,
            density: self.density,
            warmth: self.warmth,
            accent_hue_shift: self.accent_hue_shift,
        }
    }
}

/// The global config — `settings.toml`. One value across all profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct GlobalConfig {
    window_mode: usize,
    game_dir: String,
    downloads: String,
    auto_backup: bool,
    log_level: usize,
    telemetry: bool,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            window_mode: 1, // Borderless
            game_dir: "~/Library/Application Support/EwoClient".to_string(),
            downloads: "~/Downloads/ewo".to_string(),
            auto_backup: true,
            log_level: 2, // Info
            telemetry: false,
        }
    }
}

/// The profile registry — `profiles.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ProfileIndex {
    active: String,
    profiles: Vec<String>,
}

impl Default for ProfileIndex {
    fn default() -> Self {
        Self {
            active: DEFAULT_PROFILE.to_string(),
            profiles: vec![DEFAULT_PROFILE.to_string()],
        }
    }
}

// ── path helpers ─────────────────────────────────────────────────────────

fn ewo_dir() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    Some(p)
}

fn index_path() -> Option<PathBuf> {
    Some(ewo_dir()?.join(INDEX_FILENAME))
}

fn settings_path() -> Option<PathBuf> {
    Some(ewo_dir()?.join(SETTINGS_FILENAME))
}

fn client_path(profile: &str) -> Option<PathBuf> {
    let mut p = ewo_dir()?;
    p.push(PROFILES_DIRNAME);
    p.push(profile);
    p.push(CLIENT_FILENAME);
    Some(p)
}

// ── split / merge ────────────────────────────────────────────────────────

/// Split a unified `SettingsConfig` + tokens into the on-disk pair.
fn split(config: &SettingsConfig, tokens: &Settings) -> (GlobalConfig, ClientProfile) {
    let global = GlobalConfig {
        window_mode: config.window_mode,
        game_dir: config.game_dir.clone(),
        downloads: config.downloads.clone(),
        auto_backup: config.auto_backup,
        log_level: config.log_level,
        telemetry: config.telemetry,
    };
    let profile = ClientProfile {
        motion_speed: tokens.motion_speed,
        breath_amp: tokens.breath_amp,
        density: tokens.density,
        warmth: tokens.warmth,
        accent_hue_shift: tokens.accent_hue_shift,
        theme: config.theme,
        vsync: config.vsync,
        max_fps: config.max_fps,
        master: config.master,
        music: config.music,
        effects: config.effects,
        ambient_hum: config.ambient_hum,
        // Keybinds aren't part of the unified SettingsConfig; `save` splices
        // the existing on-disk keybinds back in so they survive a settings save.
        keybinds: BTreeMap::new(),
    };
    (global, profile)
}

/// Reconstruct a unified `SettingsConfig` from the on-disk pair.
fn merge(global: &GlobalConfig, profile: &ClientProfile) -> SettingsConfig {
    SettingsConfig {
        vsync: profile.vsync,
        max_fps: profile.max_fps,
        window_mode: global.window_mode,
        theme: profile.theme,
        master: profile.master,
        music: profile.music,
        effects: profile.effects,
        ambient_hum: profile.ambient_hum,
        game_dir: global.game_dir.clone(),
        downloads: global.downloads.clone(),
        auto_backup: global.auto_backup,
        log_level: global.log_level,
        telemetry: global.telemetry,
    }
}

// ── load / save ──────────────────────────────────────────────────────────

fn read_toml<T: serde::de::DeserializeOwned + Default>(path: &PathBuf, what: &str) -> T {
    if !path.exists() {
        return T::default();
    }
    match fs::read_to_string(path) {
        Ok(s) => match toml::from_str::<T>(&s) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("profile: parse {} failed: {} — using defaults", what, e);
                T::default()
            }
        },
        Err(e) => {
            log::warn!("profile: read {} failed: {} — using defaults", what, e);
            T::default()
        }
    }
}

fn write_toml<T: Serialize>(path: &PathBuf, value: &T, what: &str) {
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            log::warn!("profile: mkdir for {} failed: {}", what, e);
            return;
        }
    }
    match toml::to_string_pretty(value) {
        Ok(s) => {
            if let Err(e) = fs::write(path, s) {
                log::warn!("profile: write {} failed: {}", what, e);
            }
        }
        Err(e) => log::warn!("profile: serialize {} failed: {}", what, e),
    }
}

// ── index helpers ────────────────────────────────────────────────────────

fn read_index() -> ProfileIndex {
    match index_path() {
        Some(p) => read_toml(&p, "profiles.toml"),
        None => ProfileIndex::default(),
    }
}

fn write_index(index: &ProfileIndex) {
    if let Some(p) = index_path() {
        write_toml(&p, index, "profiles.toml");
    }
}

/// Name of the active profile.
pub fn active_name() -> String {
    read_index().active
}

/// All profile names, in registry order.
pub fn list() -> Vec<String> {
    read_index().profiles
}

// ── load ─────────────────────────────────────────────────────────────────

/// Read one profile's `client.toml` + the global `settings.toml`, merged
/// into the unified `SettingsConfig` + cosmetic tokens.
fn load_one(name: &str) -> (SettingsConfig, Settings) {
    let global: GlobalConfig = match settings_path() {
        Some(p) => read_toml(&p, "settings.toml"),
        None => GlobalConfig::default(),
    };
    let client: ClientProfile = match client_path(name) {
        Some(p) => read_toml(&p, "client.toml"),
        None => ClientProfile::default(),
    };
    (merge(&global, &client), client.tokens())
}

/// Load the active profile + global config, reconstructing the unified
/// `SettingsConfig` the Settings screen uses plus the cosmetic `Settings`
/// tokens. Migrates a pre-F `settings.toml` on first run. Never fails —
/// any unreadable file falls back to defaults.
pub fn load() -> (SettingsConfig, Settings) {
    let (Some(index_p), Some(settings_p)) = (index_path(), settings_path()) else {
        log::warn!("profile: config dir unresolvable — using defaults");
        return (SettingsConfig::default(), Settings::default());
    };
    if !index_p.exists() {
        return migrate(&index_p, &settings_p);
    }
    let active = read_index().active;
    log::info!("profile: loaded active profile \"{}\"", active);
    load_one(&active)
}

/// First-run migration: a pre-F `settings.toml` (a full `SettingsConfig`)
/// is split into `profiles/Default/client.toml` + a global-only
/// `settings.toml`, and a fresh `profiles.toml` is written. On a truly
/// fresh install this just lays down the default layout.
fn migrate(index_p: &PathBuf, settings_p: &PathBuf) -> (SettingsConfig, Settings) {
    let had_old = settings_p.exists();
    let old: SettingsConfig = read_toml(settings_p, "settings.toml (pre-F)");
    let tokens = Settings::default();
    let (global, client) = split(&old, &tokens);

    write_toml(index_p, &ProfileIndex::default(), "profiles.toml");
    write_toml(settings_p, &global, "settings.toml");
    if let Some(client_p) = client_path(DEFAULT_PROFILE) {
        write_toml(&client_p, &client, "client.toml");
    }
    if had_old {
        log::info!(
            "profile: migrated pre-F settings.toml into profiles/{}/",
            DEFAULT_PROFILE
        );
    } else {
        log::info!("profile: initialized profiles/{}/", DEFAULT_PROFILE);
    }
    (merge(&global, &client), tokens)
}

/// Persist the unified `SettingsConfig` + tokens into the active profile's
/// `client.toml` and the global `settings.toml`. Keybinds aren't carried by
/// `SettingsConfig`, so the existing on-disk keybinds are spliced back in.
pub fn save(config: &SettingsConfig, tokens: &Settings) {
    let (global, mut client) = split(config, tokens);
    if let Some(p) = settings_path() {
        write_toml(&p, &global, "settings.toml");
    }
    if let Some(p) = client_path(&active_name()) {
        client.keybinds = read_toml::<ClientProfile>(&p, "client.toml").keybinds;
        write_toml(&p, &client, "client.toml");
    }
}

// ── keybinds ─────────────────────────────────────────────────────────────

/// The active profile's keybinds — every registered action, with the
/// profile's override where it has one and the registry default otherwise.
pub fn load_keybinds() -> BTreeMap<String, KeyChord> {
    let stored = match client_path(&active_name()) {
        Some(p) => read_toml::<ClientProfile>(&p, "client.toml").keybinds,
        None => BTreeMap::new(),
    };
    let mut out = BTreeMap::new();
    for a in keybind::REGISTRY.iter() {
        let chord = stored.get(a.id).copied().unwrap_or(a.default);
        out.insert(a.id.to_string(), chord);
    }
    out
}

/// Persist the active profile's keybinds, preserving its other settings.
pub fn save_keybinds(keybinds: &BTreeMap<String, KeyChord>) {
    let Some(p) = client_path(&active_name()) else {
        return;
    };
    let mut client: ClientProfile = read_toml(&p, "client.toml");
    client.keybinds = keybinds.clone();
    write_toml(&p, &client, "client.toml");
}

// ── management ───────────────────────────────────────────────────────────

/// Switch the active profile. Returns the now-active profile's config, or
/// `None` if `name` isn't a known profile.
pub fn switch(name: &str) -> Option<(SettingsConfig, Settings)> {
    let mut index = read_index();
    if !index.profiles.iter().any(|p| p == name) {
        return None;
    }
    index.active = name.to_string();
    write_index(&index);
    log::info!("profile: switched to \"{}\"", name);
    Some(load_one(name))
}

/// Create a fresh profile with default settings, make it active, and
/// return its name + config.
pub fn create() -> (String, SettingsConfig, Settings) {
    let mut index = read_index();
    let name = unique_name(&index.profiles, "Profile");
    write_profile(&name, &ClientProfile::default());
    index.profiles.push(name.clone());
    index.active = name.clone();
    write_index(&index);
    log::info!("profile: created \"{}\"", name);
    let (config, tokens) = load_one(&name);
    (name, config, tokens)
}

/// Duplicate `src` — copy its `client.toml` into a new profile and make
/// the copy active. Returns the new name + config, `None` if `src` is
/// unknown.
pub fn duplicate(src: &str) -> Option<(String, SettingsConfig, Settings)> {
    let mut index = read_index();
    if !index.profiles.iter().any(|p| p == src) {
        return None;
    }
    let client: ClientProfile = match client_path(src) {
        Some(p) => read_toml(&p, "client.toml"),
        None => ClientProfile::default(),
    };
    let name = unique_name(&index.profiles, &format!("{src} copy"));
    write_profile(&name, &client);
    // Carry the source profile's HUD layout + module config into the copy.
    if let (Some(src_dir), Some(dst_dir)) = (profile_dir(src), profile_dir(&name)) {
        for file in ["hud.toml", "modules.toml"] {
            let src_file = src_dir.join(file);
            if src_file.exists() {
                let _ = fs::copy(&src_file, dst_dir.join(file));
            }
        }
    }
    index.profiles.push(name.clone());
    index.active = name.clone();
    write_index(&index);
    log::info!("profile: duplicated \"{}\" -> \"{}\"", src, name);
    let (config, tokens) = load_one(&name);
    Some((name, config, tokens))
}

/// Longest a profile name may be — matches the launcher's rename field cap.
pub const MAX_NAME_LEN: usize = 48;

/// Whether `name` is usable as a profile name. It becomes a directory name
/// under `profiles/`, so it must be non-empty, within the length cap, and
/// free of path-unsafe characters.
pub fn is_valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_LEN
        && name != "."
        && name != ".."
        && !name.contains(|c: char| "/\\:*?\"<>|".contains(c))
}

/// Rename profile `old` to `new`. No-op success if the names are equal.
/// Returns `false` if `new` is invalid, already taken, or `old` is unknown.
pub fn rename(old: &str, new: &str) -> bool {
    let new = new.trim();
    if old == new {
        return true; // nothing to do — treated as success
    }
    if !is_valid_name(new) {
        log::warn!("profile: rename rejected — invalid name \"{}\"", new);
        return false;
    }
    let mut index = read_index();
    if !index.profiles.iter().any(|p| p == old) {
        return false;
    }
    if index.profiles.iter().any(|p| p == new) {
        log::warn!("profile: rename rejected — \"{}\" already exists", new);
        return false;
    }
    if let (Some(src), Some(dst)) = (profile_dir(old), profile_dir(new)) {
        if let Err(e) = fs::rename(&src, &dst) {
            log::warn!("profile: rename dir failed: {}", e);
            return false;
        }
    }
    for p in index.profiles.iter_mut() {
        if p == old {
            *p = new.to_string();
        }
    }
    if index.active == old {
        index.active = new.to_string();
    }
    write_index(&index);
    log::info!("profile: renamed \"{}\" -> \"{}\"", old, new);
    true
}

/// Delete a profile. The last remaining profile can't be deleted. Returns
/// the (possibly changed) active profile's config, or `None` if nothing
/// was deleted.
pub fn delete(name: &str) -> Option<(SettingsConfig, Settings)> {
    let mut index = read_index();
    if index.profiles.len() <= 1 || !index.profiles.iter().any(|p| p == name) {
        return None;
    }
    index.profiles.retain(|p| p != name);
    if index.active == name {
        index.active = index.profiles[0].clone();
    }
    if let Some(dir) = profile_dir(name) {
        if let Err(e) = fs::remove_dir_all(&dir) {
            log::warn!("profile: delete dir failed: {}", e);
        }
    }
    write_index(&index);
    log::info!("profile: deleted \"{}\" (active now \"{}\")", name, index.active);
    Some(load_one(&index.active))
}

// ── helpers ──────────────────────────────────────────────────────────────

fn profile_dir(name: &str) -> Option<PathBuf> {
    let mut p = ewo_dir()?;
    p.push(PROFILES_DIRNAME);
    p.push(name);
    Some(p)
}

fn write_profile(name: &str, client: &ClientProfile) {
    if let Some(p) = client_path(name) {
        write_toml(&p, client, "client.toml");
    }
}

/// A name not already in `existing` — `base`, then `base 2`, `base 3`, …
fn unique_name(existing: &[String], base: &str) -> String {
    if !existing.iter().any(|p| p == base) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base} {n}");
        if !existing.iter().any(|p| p == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_merge_round_trips_every_field() {
        let mut config = SettingsConfig::default();
        config.vsync = false;
        config.max_fps = 240.0;
        config.theme = 2;
        config.master = 0.33;
        config.music = 0.11;
        config.effects = 0.22;
        config.ambient_hum = false;
        config.window_mode = 2;
        config.game_dir = "/custom/game".to_string();
        config.downloads = "/custom/dl".to_string();
        config.auto_backup = false;
        config.log_level = 4;
        config.telemetry = true;
        let tokens = Settings {
            motion_speed: 1.5,
            breath_amp: 0.5,
            density: 2.0,
            warmth: 0.2,
            accent_hue_shift: 30.0,
        };

        let (global, profile) = split(&config, &tokens);
        let merged = merge(&global, &profile);

        // Profile-scoped fields survive the round trip.
        assert!(!merged.vsync);
        assert_eq!(merged.max_fps, 240.0);
        assert_eq!(merged.theme, 2);
        assert_eq!(merged.master, 0.33);
        assert_eq!(merged.music, 0.11);
        assert_eq!(merged.effects, 0.22);
        assert!(!merged.ambient_hum);
        // Global fields survive.
        assert_eq!(merged.window_mode, 2);
        assert_eq!(merged.game_dir, "/custom/game");
        assert_eq!(merged.downloads, "/custom/dl");
        assert!(!merged.auto_backup);
        assert_eq!(merged.log_level, 4);
        assert!(merged.telemetry);
        // Tokens survive.
        let t = profile.tokens();
        assert_eq!(t.motion_speed, 1.5);
        assert_eq!(t.breath_amp, 0.5);
        assert_eq!(t.density, 2.0);
        assert_eq!(t.warmth, 0.2);
        assert_eq!(t.accent_hue_shift, 30.0);
    }

    #[test]
    fn client_profile_round_trips_through_toml() {
        let profile = ClientProfile {
            warmth: 0.9,
            theme: 3,
            max_fps: 200.0,
            ..ClientProfile::default()
        };
        let text = toml::to_string_pretty(&profile).expect("serialize");
        let parsed: ClientProfile = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed.warmth, 0.9);
        assert_eq!(parsed.theme, 3);
        assert_eq!(parsed.max_fps, 200.0);
        assert_eq!(parsed.density, 1.0); // untouched default
    }

    #[test]
    fn profile_name_validation_rejects_unsafe_names() {
        assert!(is_valid_name("PvP"));
        assert!(is_valid_name("  Cinematic  ")); // trimmed before checks
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("   "));
        assert!(!is_valid_name("."));
        assert!(!is_valid_name(".."));
        assert!(!is_valid_name("a/b"));
        assert!(!is_valid_name("c:\\bad"));
        assert!(!is_valid_name(&"x".repeat(MAX_NAME_LEN + 1)));
    }

    #[test]
    fn keybinds_survive_the_client_toml_round_trip() {
        let mut profile = ClientProfile::default();
        profile
            .keybinds
            .insert("overlay.open".to_string(), KeyChord { key: 90, mods: 0 });
        let text = toml::to_string_pretty(&profile).expect("serialize");
        let parsed: ClientProfile = toml::from_str(&text).expect("deserialize");
        assert_eq!(
            parsed.keybinds.get("overlay.open"),
            Some(&KeyChord { key: 90, mods: 0 }),
        );
    }

    #[test]
    fn partial_client_toml_fills_missing_fields_from_default() {
        // `#[serde(default)]` on the struct means a client.toml written by
        // an older build (missing newer fields) still loads cleanly.
        let parsed: ClientProfile = toml::from_str("warmth = 0.4\n").expect("partial parse");
        assert_eq!(parsed.warmth, 0.4);
        assert_eq!(parsed.vsync, true);
        assert_eq!(parsed.theme, 0);
    }
}
