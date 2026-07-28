//! EwoClient modules in Rewo (M52) — the port of the legit module set.
//!
//! The catalog is **not** redefined here. `ewo_core::modules::REGISTRY` is the
//! single source of truth its own docs describe ("the launcher and the in-game
//! overlay both read it"); this makes Rewo the third reader. Only `rewo-app`
//! depends on `ewo-core` — the renderer keeps taking plain floats and bools, so
//! `rewo-gpu` stays a pure Vulkan crate with no launcher coupling.
//!
//! State comes from the **same file the launcher already writes**,
//! `<config>/EwoClient/profiles/<active>/modules.toml`, so a module toggled in
//! Settings → Modules applies to a Native instance with no new contract. The
//! parser is duplicated rather than shared because `profile::load_modules`
//! lives in the launcher *binary*; the format is a dozen lines of TOML subset
//! and importing a binary crate to avoid retyping it would be the worse trade.
//!
//! ## What the survey said, and what auditing the crates corrected
//!
//! `REWO_FEATURE_SURVEY.md` ranks this port as one milestone worth ~418 mods
//! and 242M downloads of demand. Auditing the modules against the `rewo-*`
//! crates found two of the twelve are **vacuous here**:
//!
//! * `no_view_bob` — Rewo has never implemented view bobbing.
//! * `no_damage_tilt` — nor `bobHurt`.
//!
//! You cannot disable what was never built, so those two are deliberately
//! absent from [`Modules::render`] rather than wired to a no-op. They become
//! real only if Rewo first ships the vanilla behaviour they suppress. Recorded
//! because a module list that silently contains dead toggles is worse than one
//! that is honestly short.

use ewo_core::modules as catalog;

/// The per-frame module inputs the render path actually consumes.
///
/// Deliberately plain data: `rewo-gpu` never sees `Modules`, only these.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderModules {
    /// Vertical field of view in **degrees**, already including any zoom.
    pub fov_degrees: f32,
    /// Gamma handed to `resolve_lightmap`. Full Bright forces vanilla's
    /// maximum rather than bypassing the lightmap, so the result still goes
    /// through the exact `darkness_lightmap` path M13 transcribed.
    pub gamma: f32,
    /// Suppress the fire overlay (`no_fire_overlay`).
    pub hide_fire_overlay: bool,
    /// Suppress the pumpkin overlay (`no_pumpkin_overlay`).
    pub hide_pumpkin_overlay: bool,
}

impl Default for RenderModules {
    fn default() -> Self {
        Self {
            fov_degrees: VANILLA_FOV,
            gamma: 0.0,
            hide_fire_overlay: false,
            hide_pumpkin_overlay: false,
        }
    }
}

/// Vanilla's default vertical FOV, and what `eye_view_proj` hard-coded before
/// this module existed.
pub const VANILLA_FOV: f32 = 70.0;

/// Vanilla's maximum brightness option. Full Bright pins gamma here.
///
/// Mods in this cluster (Gamma Utils, GJEB) push far past 1.0 — "gamma up to
/// 1000%" — which is beyond-vanilla territory. Rewo starts at parity; going
/// further is a setting, not a different mechanism.
pub const MAX_GAMMA: f32 = 1.0;

/// Live module state: the persisted toggles plus the transient ones a key
/// holds down.
#[derive(Debug, Clone)]
pub struct Modules {
    /// Indexed by `catalog::REGISTRY` position.
    enabled: Vec<bool>,
    /// Per-module settings, same indexing, `catalog::MAX_SETTINGS` wide.
    settings: Vec<[f32; catalog::MAX_SETTINGS]>,
    /// Zoom is a *held* key, not a persisted toggle, so it lives outside the
    /// config: `Zoomify` and every one of the 54 zoom mods bind it to a hold.
    zoom_held: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Self::from_defaults()
    }
}

impl Modules {
    /// Catalog defaults, no file read. Used by the headless gates so a stray
    /// `modules.toml` on the dev box cannot change a golden image.
    pub fn from_defaults() -> Self {
        Self {
            enabled: catalog::REGISTRY.iter().map(|m| m.default_enabled).collect(),
            settings: catalog::REGISTRY
                .iter()
                .map(|m| {
                    let mut s = [0.0; catalog::MAX_SETTINGS];
                    for (i, def) in m.settings.iter().enumerate() {
                        s[i] = def.default;
                    }
                    s
                })
                .collect(),
            zoom_held: false,
        }
    }

    /// Load the active profile's `modules.toml`, falling back to catalog
    /// defaults for anything absent, unparseable, or unknown.
    ///
    /// Unknown ids are **skipped, not an error** — the launcher's own loader
    /// leaves inert sections behind for deleted modules (the post-ban refactor
    /// dropped three), and a stale section must not stop the client booting.
    pub fn load() -> Self {
        let mut me = Self::from_defaults();
        let Some(text) = modules_toml_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return me;
        };
        let mut current: Option<usize> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                current = catalog::index_of(section.trim());
                continue;
            }
            let Some(idx) = current else { continue };
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if key == "enabled" {
                me.enabled[idx] = value == "true";
            } else if let Ok(v) = value.parse::<f32>() {
                if let Some(si) = catalog::REGISTRY[idx]
                    .settings
                    .iter()
                    .position(|s| s.id == key)
                {
                    me.settings[idx][si] = v;
                }
            }
        }
        me
    }

    /// Is a module enabled? Unknown id → `false`, never a panic: this is called
    /// with string literals from the render path and a typo must degrade to
    /// "off" rather than take the client down.
    pub fn is_on(&self, id: &str) -> bool {
        catalog::index_of(id).is_some_and(|i| self.enabled[i])
    }

    /// A module's numeric setting, or the catalog default if absent.
    pub fn setting(&self, id: &str, index: usize) -> f32 {
        catalog::index_of(id)
            .and_then(|i| self.settings[i].get(index).copied())
            .unwrap_or(0.0)
    }

    /// Flip a module. Returns the new state (`false` for an unknown id).
    pub fn toggle(&mut self, id: &str) -> bool {
        match catalog::index_of(id) {
            Some(i) => {
                self.enabled[i] = !self.enabled[i];
                self.enabled[i]
            }
            None => false,
        }
    }

    /// Zoom is held, not toggled.
    pub fn set_zoom_held(&mut self, held: bool) {
        self.zoom_held = held;
    }

    pub fn zoom_held(&self) -> bool {
        self.zoom_held
    }

    /// Resolve everything the render path needs for this frame.
    ///
    /// **FOV order matters.** Zoom divides whatever FOV is in effect, so it
    /// composes with FOV Control rather than overriding it — hold zoom with a
    /// 110° FOV and you get 110/4, not 70/4. That is what every zoom mod does
    /// (`Zoomify` scales `options.fov()`), and getting it backwards is only
    /// visible when both are active at once.
    pub fn render(&self) -> RenderModules {
        let base = if self.is_on("fov") {
            self.setting("fov", 0)
        } else {
            VANILLA_FOV
        };
        let fov = if self.zoom_held { base / ZOOM_DIVISOR } else { base };
        RenderModules {
            fov_degrees: fov.clamp(1.0, 179.0),
            gamma: if self.is_on("fullbright") { MAX_GAMMA } else { 0.0 },
            hide_fire_overlay: self.is_on("no_fire_overlay"),
            hide_pumpkin_overlay: self.is_on("no_pumpkin_overlay"),
        }
    }
}

/// Zoomify's default. A divisor, not a fixed FOV, so it composes (see
/// [`Modules::render`]).
pub const ZOOM_DIVISOR: f32 = 4.0;

/// `<config>/EwoClient/profiles/<active>/modules.toml` — the launcher's path,
/// reproduced so a Native instance picks up Settings → Modules for free.
fn modules_toml_path() -> Option<std::path::PathBuf> {
    let root = dirs::config_dir()?.join("EwoClient");
    let registry = std::fs::read_to_string(root.join("profiles.toml")).ok()?;
    let active = registry
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("active")?
                .trim_start()
                .strip_prefix('=')?
                .trim()
                .trim_matches('"')
                .to_owned()
                .into()
        })
        .unwrap_or_else(|| "Default".to_string());
    Some(root.join("profiles").join(active).join("modules.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_catalog() {
        let m = Modules::from_defaults();
        for def in catalog::REGISTRY {
            assert_eq!(m.is_on(def.id), def.default_enabled, "{}", def.id);
        }
    }

    #[test]
    fn vanilla_render_state_is_untouched() {
        // Every legit module defaults off, so a fresh client must render at
        // exactly the constants `eye_view_proj` and `resolve_lightmap` used
        // before this module existed. This is what keeps the golden PNGs
        // byte-identical.
        let r = Modules::from_defaults().render();
        assert_eq!(r.fov_degrees, VANILLA_FOV);
        assert_eq!(r.gamma, 0.0);
        assert!(!r.hide_fire_overlay && !r.hide_pumpkin_overlay);
    }

    #[test]
    fn fullbright_goes_through_vanilla_gamma() {
        let mut m = Modules::from_defaults();
        m.toggle("fullbright");
        assert_eq!(m.render().gamma, MAX_GAMMA);
    }

    #[test]
    fn zoom_composes_with_fov_rather_than_overriding_it() {
        let mut m = Modules::from_defaults();
        m.toggle("fov");
        // catalog default for the fov setting is 90.
        assert_eq!(m.render().fov_degrees, 90.0);
        m.set_zoom_held(true);
        assert_eq!(m.render().fov_degrees, 90.0 / ZOOM_DIVISOR);
        // ...and with FOV Control off, zoom divides vanilla's 70.
        m.toggle("fov");
        assert_eq!(m.render().fov_degrees, VANILLA_FOV / ZOOM_DIVISOR);
    }

    #[test]
    fn unknown_ids_degrade_to_off_rather_than_panicking() {
        let mut m = Modules::from_defaults();
        assert!(!m.is_on("no_such_module"));
        assert!(!m.toggle("no_such_module"));
        assert_eq!(m.setting("no_such_module", 0), 0.0);
    }

    #[test]
    fn the_two_vacuous_modules_are_not_in_the_render_state() {
        // `no_view_bob` and `no_damage_tilt` exist in the catalog but Rewo has
        // no view bobbing and no hurt-cam to suppress. If either ever gains a
        // field in RenderModules, the behaviour must have been built first.
        let mut m = Modules::from_defaults();
        m.toggle("no_view_bob");
        m.toggle("no_damage_tilt");
        assert_eq!(m.render(), Modules::from_defaults().render());
    }
}
