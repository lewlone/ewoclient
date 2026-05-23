//! PvP-Utils config — Rust mirror of `EwoPvpConfig.java`.
//!
//! Lives in `ewo-core` so **both** the in-game overlay (`ewo-jni`) and the
//! launcher's Settings → PvP-Utils tab (`ewo-render` + `ewo-launcher`) can
//! load + edit + save the same file. The Java mod is the authoritative
//! consumer at runtime (it owns the trackers + plays the sounds); either
//! Rust caller writes the same `<profile>/pvp.toml` and the Java mod polls
//! the file's mtime each frame and hot-reloads on a change.
//!
//! Layout mirrors `EwoPvpConfig`: the same section names + keys. Hand-parsed,
//! no `toml` crate dep (the cdylib doesn't want one — same constraint as
//! `modules.rs` / `hud.toml`).

use std::path::PathBuf;

// ──────────────────────────────────────────────────────────────────────────
// Sound palette — wire-mirror of `EwoPvpSounds`.
// ──────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PvpSound {
    Bell,
    Pling,
    Chime,
    Harp,
    Bass,
    XpOrb,
    Anvil,
    Amethyst,
}

impl PvpSound {
    pub const ALL: [PvpSound; 8] = [
        PvpSound::Bell,
        PvpSound::Pling,
        PvpSound::Chime,
        PvpSound::Harp,
        PvpSound::Bass,
        PvpSound::XpOrb,
        PvpSound::Anvil,
        PvpSound::Amethyst,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PvpSound::Bell => "Bell",
            PvpSound::Pling => "Pling",
            PvpSound::Chime => "Chime",
            PvpSound::Harp => "Harp",
            PvpSound::Bass => "Bass",
            PvpSound::XpOrb => "XP orb",
            PvpSound::Anvil => "Anvil",
            PvpSound::Amethyst => "Amethyst",
        }
    }

    /// Token written into `pvp.toml` — must match `EwoPvpSounds.fromToken`.
    pub fn token(self) -> &'static str {
        match self {
            PvpSound::Bell => "BELL",
            PvpSound::Pling => "PLING",
            PvpSound::Chime => "CHIME",
            PvpSound::Harp => "HARP",
            PvpSound::Bass => "BASS",
            PvpSound::XpOrb => "XP_ORB",
            PvpSound::Anvil => "ANVIL",
            PvpSound::Amethyst => "AMETHYST",
        }
    }

    pub fn from_token(s: &str) -> PvpSound {
        match s.to_ascii_uppercase().as_str() {
            "BELL" => PvpSound::Bell,
            "PLING" => PvpSound::Pling,
            "CHIME" => PvpSound::Chime,
            "HARP" => PvpSound::Harp,
            "BASS" => PvpSound::Bass,
            "XP_ORB" => PvpSound::XpOrb,
            "ANVIL" => PvpSound::Anvil,
            "AMETHYST" => PvpSound::Amethyst,
            _ => PvpSound::Bell,
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Config data types — mirror of `EwoPvpConfig` / `SoundSlot` / `Zone`.
// ──────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct SoundSlot {
    pub enabled: bool,
    pub sound: PvpSound,
    pub pitch: f32,
    pub volume: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub enabled: bool,
    pub min_dist: f32,
    pub max_dist: f32,
    pub sound: PvpSound,
    pub pitch: f32,
    pub volume: f32,
    pub color: i32,
}

/// One tier — used by the per-tier-sound UI.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tier {
    Perfect,
    SlightlyLate,
    Late,
    SlightlyEarly,
    Early,
}

impl Tier {
    pub const ALL: [Tier; 5] = [
        Tier::Perfect,
        Tier::SlightlyLate,
        Tier::Late,
        Tier::SlightlyEarly,
        Tier::Early,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tier::Perfect => "Perfect",
            Tier::SlightlyLate => "Slightly late",
            Tier::Late => "Late",
            Tier::SlightlyEarly => "Slightly early",
            Tier::Early => "Early",
        }
    }

    fn section(self) -> &'static str {
        match self {
            Tier::Perfect => "sounds.perfect",
            Tier::SlightlyLate => "sounds.slightly_late",
            Tier::Late => "sounds.late",
            Tier::SlightlyEarly => "sounds.slightly_early",
            Tier::Early => "sounds.early",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PvpConfig {
    // [jump_reset]
    pub jump_reset_enabled: bool,
    pub jump_reset_bar_enabled: bool,
    pub jump_reset_proximity_window_ticks: i32,
    pub jump_reset_fade_ticks: i32,
    pub jump_reset_bar_max_range_ms: i32,
    pub jump_reset_show_all_hits: bool,

    // [hit_range]
    pub hit_range_enabled: bool,
    pub hit_range_fade_ticks: i32,

    // [indicators] — world-anchored combat indicators (Commit 3)
    pub totem_count_enabled: bool,
    pub floating_health_enabled: bool,

    // [colors]
    pub color_perfect: i32,
    pub color_slightly_late: i32,
    pub color_late: i32,
    pub color_slightly_early: i32,
    pub color_early: i32,
    pub color_no_reset: i32,

    // [sounds.*]
    pub sound_perfect: SoundSlot,
    pub sound_slightly_late: SoundSlot,
    pub sound_late: SoundSlot,
    pub sound_slightly_early: SoundSlot,
    pub sound_early: SoundSlot,

    // [zones.zone1..3]
    pub zone1: Zone,
    pub zone2: Zone,
    pub zone3: Zone,
}

impl PvpConfig {
    /// Velvet defaults — must match `EwoPvpConfig`'s field initialisers.
    pub fn defaults() -> Self {
        PvpConfig {
            jump_reset_enabled: true,
            jump_reset_bar_enabled: true,
            jump_reset_proximity_window_ticks: 6,
            jump_reset_fade_ticks: 20,
            jump_reset_bar_max_range_ms: 300,
            jump_reset_show_all_hits: false,

            hit_range_enabled: true,
            hit_range_fade_ticks: 20,

            totem_count_enabled: true,
            floating_health_enabled: true,

            color_perfect: 0xE8D4A8,
            color_slightly_late: 0xE5B8C5,
            color_late: 0xC96A7A,
            color_slightly_early: 0xC9A5D4,
            color_early: 0xB47491,
            color_no_reset: 0x9A8087,

            sound_perfect: SoundSlot {
                enabled: true,
                sound: PvpSound::Bell,
                pitch: 2.0,
                volume: 1.0,
            },
            sound_slightly_late: SoundSlot {
                enabled: true,
                sound: PvpSound::Pling,
                pitch: 1.5,
                volume: 0.8,
            },
            sound_late: SoundSlot {
                enabled: true,
                sound: PvpSound::Pling,
                pitch: 0.8,
                volume: 0.8,
            },
            sound_slightly_early: SoundSlot {
                enabled: true,
                sound: PvpSound::Bass,
                pitch: 1.5,
                volume: 0.8,
            },
            sound_early: SoundSlot {
                enabled: true,
                sound: PvpSound::Bass,
                pitch: 0.8,
                volume: 0.8,
            },

            zone1: Zone {
                enabled: false,
                min_dist: 0.0,
                max_dist: 2.0,
                sound: PvpSound::Harp,
                pitch: 1.2,
                volume: 0.6,
                color: 0xC9A5D4,
            },
            zone2: Zone {
                enabled: false,
                min_dist: 2.5,
                max_dist: 2.8,
                sound: PvpSound::Pling,
                pitch: 1.5,
                volume: 0.8,
                color: 0xE8D4A8,
            },
            zone3: Zone {
                enabled: true,
                min_dist: 2.9,
                max_dist: 3.0,
                sound: PvpSound::Bell,
                pitch: 2.0,
                volume: 1.0,
                color: 0xE5B8C5,
            },
        }
    }

    /// Get / set the per-tier sound slot by tier (handy for the UI dispatch).
    pub fn sound_for_tier(&self, tier: Tier) -> &SoundSlot {
        match tier {
            Tier::Perfect => &self.sound_perfect,
            Tier::SlightlyLate => &self.sound_slightly_late,
            Tier::Late => &self.sound_late,
            Tier::SlightlyEarly => &self.sound_slightly_early,
            Tier::Early => &self.sound_early,
        }
    }

    pub fn sound_for_tier_mut(&mut self, tier: Tier) -> &mut SoundSlot {
        match tier {
            Tier::Perfect => &mut self.sound_perfect,
            Tier::SlightlyLate => &mut self.sound_slightly_late,
            Tier::Late => &mut self.sound_late,
            Tier::SlightlyEarly => &mut self.sound_slightly_early,
            Tier::Early => &mut self.sound_early,
        }
    }

    pub fn zone(&self, i: usize) -> &Zone {
        match i {
            0 => &self.zone1,
            1 => &self.zone2,
            _ => &self.zone3,
        }
    }
    pub fn zone_mut(&mut self, i: usize) -> &mut Zone {
        match i {
            0 => &mut self.zone1,
            1 => &mut self.zone2,
            _ => &mut self.zone3,
        }
    }

    /// Load the active profile's `pvp.toml`, falling back to defaults — a hand-
    /// edited or absent file never breaks the feature.
    pub fn load() -> Self {
        let mut cfg = Self::defaults();
        let Some(text) = pvp_toml_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return cfg;
        };
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(s) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = s.trim().to_string();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            cfg.apply(&section, key, value);
        }
        cfg
    }

    fn apply(&mut self, section: &str, key: &str, value: &str) {
        match section {
            "jump_reset" => match key {
                "enabled" => self.jump_reset_enabled = parse_bool(value, self.jump_reset_enabled),
                "bar_enabled" => {
                    self.jump_reset_bar_enabled = parse_bool(value, self.jump_reset_bar_enabled)
                }
                "proximity_window_ticks" => {
                    self.jump_reset_proximity_window_ticks =
                        parse_i32(value, self.jump_reset_proximity_window_ticks)
                }
                "fade_ticks" => {
                    self.jump_reset_fade_ticks = parse_i32(value, self.jump_reset_fade_ticks)
                }
                "bar_max_range_ms" => {
                    self.jump_reset_bar_max_range_ms =
                        parse_i32(value, self.jump_reset_bar_max_range_ms)
                }
                "show_all_hits" => {
                    self.jump_reset_show_all_hits =
                        parse_bool(value, self.jump_reset_show_all_hits)
                }
                _ => {}
            },
            "hit_range" => match key {
                "enabled" => self.hit_range_enabled = parse_bool(value, self.hit_range_enabled),
                "fade_ticks" => self.hit_range_fade_ticks = parse_i32(value, self.hit_range_fade_ticks),
                _ => {}
            },
            "indicators" => match key {
                "totem_count" => {
                    self.totem_count_enabled = parse_bool(value, self.totem_count_enabled)
                }
                "floating_health" => {
                    self.floating_health_enabled =
                        parse_bool(value, self.floating_health_enabled)
                }
                _ => {}
            },
            "colors" => match key {
                "perfect" => self.color_perfect = parse_color(value, self.color_perfect),
                "slightly_late" => {
                    self.color_slightly_late = parse_color(value, self.color_slightly_late)
                }
                "late" => self.color_late = parse_color(value, self.color_late),
                "slightly_early" => {
                    self.color_slightly_early = parse_color(value, self.color_slightly_early)
                }
                "early" => self.color_early = parse_color(value, self.color_early),
                "no_reset" => self.color_no_reset = parse_color(value, self.color_no_reset),
                _ => {}
            },
            s if s.starts_with("sounds.") => {
                let tier = match s {
                    "sounds.perfect" => Tier::Perfect,
                    "sounds.slightly_late" => Tier::SlightlyLate,
                    "sounds.late" => Tier::Late,
                    "sounds.slightly_early" => Tier::SlightlyEarly,
                    "sounds.early" => Tier::Early,
                    _ => return,
                };
                let slot = self.sound_for_tier_mut(tier);
                match key {
                    "enabled" => slot.enabled = parse_bool(value, slot.enabled),
                    "sound" => slot.sound = PvpSound::from_token(value),
                    "pitch" => slot.pitch = parse_f32(value, slot.pitch),
                    "volume" => slot.volume = parse_f32(value, slot.volume),
                    _ => {}
                }
            }
            s if s.starts_with("zones.") => {
                let idx = match s {
                    "zones.zone1" => 0,
                    "zones.zone2" => 1,
                    "zones.zone3" => 2,
                    _ => return,
                };
                let z = self.zone_mut(idx);
                match key {
                    "enabled" => z.enabled = parse_bool(value, z.enabled),
                    "min_dist" => z.min_dist = parse_f32(value, z.min_dist),
                    "max_dist" => z.max_dist = parse_f32(value, z.max_dist),
                    "sound" => z.sound = PvpSound::from_token(value),
                    "pitch" => z.pitch = parse_f32(value, z.pitch),
                    "volume" => z.volume = parse_f32(value, z.volume),
                    "color" => z.color = parse_color(value, z.color),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Write the active profile's `pvp.toml`. The Java mod polls the file's
    /// mtime each frame and hot-reloads on a change.
    pub fn save(&self) {
        let Some(path) = pvp_toml_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut s = String::with_capacity(2048);
        s.push_str("# EwoClient PvP Utils config — per client profile.\n");
        s.push_str("# Schema 1, written by the in-game overlay editor.\n");

        s.push_str("\n[jump_reset]\n");
        s.push_str(&format!("enabled = {}\n", self.jump_reset_enabled));
        s.push_str(&format!("bar_enabled = {}\n", self.jump_reset_bar_enabled));
        s.push_str(&format!(
            "proximity_window_ticks = {}\n",
            self.jump_reset_proximity_window_ticks
        ));
        s.push_str(&format!("fade_ticks = {}\n", self.jump_reset_fade_ticks));
        s.push_str(&format!(
            "bar_max_range_ms = {}\n",
            self.jump_reset_bar_max_range_ms
        ));
        s.push_str(&format!("show_all_hits = {}\n", self.jump_reset_show_all_hits));

        s.push_str("\n[hit_range]\n");
        s.push_str(&format!("enabled = {}\n", self.hit_range_enabled));
        s.push_str(&format!("fade_ticks = {}\n", self.hit_range_fade_ticks));

        s.push_str("\n[indicators]\n");
        s.push_str(&format!("totem_count = {}\n", self.totem_count_enabled));
        s.push_str(&format!(
            "floating_health = {}\n",
            self.floating_health_enabled
        ));

        s.push_str("\n[colors]\n");
        push_color(&mut s, "perfect", self.color_perfect);
        push_color(&mut s, "slightly_late", self.color_slightly_late);
        push_color(&mut s, "late", self.color_late);
        push_color(&mut s, "slightly_early", self.color_slightly_early);
        push_color(&mut s, "early", self.color_early);
        push_color(&mut s, "no_reset", self.color_no_reset);

        for tier in Tier::ALL {
            let slot = self.sound_for_tier(tier);
            s.push_str(&format!("\n[{}]\n", tier.section()));
            s.push_str(&format!("enabled = {}\n", slot.enabled));
            s.push_str(&format!("sound = \"{}\"\n", slot.sound.token()));
            s.push_str(&format!("pitch = {}\n", slot.pitch));
            s.push_str(&format!("volume = {}\n", slot.volume));
        }

        for (i, label) in ["zone1", "zone2", "zone3"].iter().enumerate() {
            let z = self.zone(i);
            s.push_str(&format!("\n[zones.{}]\n", label));
            s.push_str(&format!("enabled = {}\n", z.enabled));
            s.push_str(&format!("min_dist = {}\n", z.min_dist));
            s.push_str(&format!("max_dist = {}\n", z.max_dist));
            s.push_str(&format!("sound = \"{}\"\n", z.sound.token()));
            s.push_str(&format!("pitch = {}\n", z.pitch));
            s.push_str(&format!("volume = {}\n", z.volume));
            push_color(&mut s, "color", z.color);
        }

        let _ = std::fs::write(&path, s);
    }
}

fn push_color(s: &mut String, key: &str, c: i32) {
    s.push_str(&format!("{} = \"{:06X}\"\n", key, c & 0xFFFFFF));
}

fn parse_bool(s: &str, fb: bool) -> bool {
    match s.to_ascii_lowercase().as_str() {
        "true" => true,
        "false" => false,
        _ => fb,
    }
}
fn parse_i32(s: &str, fb: i32) -> i32 {
    s.parse().unwrap_or(fb)
}
fn parse_f32(s: &str, fb: f32) -> f32 {
    s.parse().unwrap_or(fb)
}
fn parse_color(s: &str, fb: i32) -> i32 {
    let t = s.trim_start_matches('#');
    i32::from_str_radix(t, 16).map(|v| v & 0xFFFFFF).unwrap_or(fb)
}

/// `%APPDATA%/EwoClient/profiles/<active>/pvp.toml`.
pub fn pvp_toml_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("EwoClient")
            .join("profiles")
            .join(read_active_profile())
            .join("pvp.toml"),
    )
}

/// `%APPDATA%/EwoClient/profiles/<active>/pvp.toml`, with a caller-supplied
/// profile name — used by the launcher's Profiles tab where the user can edit
/// the inactive-profile config too (a rare path; the active-profile lookup is
/// the common case via [`pvp_toml_path`]).
pub fn pvp_toml_path_for(profile: &str) -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("EwoClient")
            .join("profiles")
            .join(profile)
            .join("pvp.toml"),
    )
}

/// Parse `<config>/EwoClient/profiles.toml` for the `active = "Name"` line.
/// Defaults to `"Default"` — matches the launcher's `profile.rs` fallback.
fn read_active_profile() -> String {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return "Default".to_string();
    };
    let path = PathBuf::from(appdata).join("EwoClient").join("profiles.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return "Default".to_string();
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("active") {
            if let Some(val) = rest.trim_start().strip_prefix('=') {
                return val.trim().trim_matches('"').to_string();
            }
        }
    }
    "Default".to_string()
}
