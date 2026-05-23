//! EwoClient modules — the catalog of legit-client features (Phase G).
//!
//! A *module* is an in-game, legit-client feature with an on/off state,
//! optional numeric settings, and an optional keybind: Full Bright, FOV
//! control, Toggle Sprint, FreeLook, and so on. The catalog ([`REGISTRY`]) is
//! the single source of truth — the launcher (Settings + keybind registry) and
//! the in-game overlay (`ewo-jni`) both read it.
//!
//! This catalog is plain `&'static` data on purpose, so `ewo-core` stays
//! dependency-light. Keybind defaults are raw GLFW key codes (`0` = unbound) —
//! `ewo-core` needn't know about winit; the launcher's `keybind` module wraps
//! them into `KeyChord`s.
//!
//! **Constraint (Phase E #4): legit-client features only.** No hacked-client
//! modules, ever. See `PHASE_G_PLAN.md`.

/// What part of the game a module touches — drives the MODULES-tab grouping
/// and accent colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleCategory {
    /// Brightness, field of view — what the world looks like.
    Visual,
    /// View bob, damage tilt, free camera — how the camera behaves.
    Camera,
    /// Sprint / sneak assistance — how the player moves.
    Movement,
}

impl ModuleCategory {
    /// Uppercase label for the overlay + launcher UIs.
    pub fn label(self) -> &'static str {
        match self {
            ModuleCategory::Visual => "VISUAL",
            ModuleCategory::Camera => "CAMERA",
            ModuleCategory::Movement => "MOVEMENT",
        }
    }
}

/// One numeric setting on a module — rendered as a slider, stored as an `f32`.
#[derive(Debug, Clone, Copy)]
pub struct ModuleSetting {
    /// Stable key — the `modules.toml` field name under the module's section.
    pub id: &'static str,
    /// Human label for the slider.
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    /// Slider snap increment.
    pub step: f32,
    pub default: f32,
}

/// One module definition.
#[derive(Debug, Clone, Copy)]
pub struct ModuleDef {
    /// Stable id — the `modules.toml` section name and the keybind-action id.
    /// The module's index in [`REGISTRY`] is its slot in the shared buffer, so
    /// neither the id nor the registry order may change without a buffer
    /// `SCHEMA_VERSION` bump.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// One-line description for the MODULES tab.
    pub description: &'static str,
    pub category: ModuleCategory,
    /// Whether a fresh profile starts with the module on.
    pub default_enabled: bool,
    /// Default keybind as a raw GLFW key code; `0` = unbound.
    pub default_key: i32,
    /// `true` if the keybind is momentary (hold-to-activate, e.g. FreeLook)
    /// rather than press-to-toggle. Hold-key modules still carry an enable
    /// toggle in the UI — the key only does anything while the module is on.
    pub hold_key: bool,
    /// 0..[`MAX_SETTINGS`] sliders, in buffer-slot order.
    pub settings: &'static [ModuleSetting],
}

impl ModuleDef {
    /// The default value of setting slot `slot`, or `0.0` past this module's
    /// setting count.
    pub fn setting_default(&self, slot: usize) -> f32 {
        self.settings.get(slot).map(|s| s.default).unwrap_or(0.0)
    }
}

/// Most settings any one module carries — fixes the shared-buffer record size.
pub const MAX_SETTINGS: usize = 2;

/// Every EwoClient module, in a stable order. A module's index here is its
/// slot in the `EwoModuleData` buffer — never reorder without bumping the
/// buffer schema version (see `PHASE_G_PLAN.md`).
pub const REGISTRY: &[ModuleDef] = &[
    ModuleDef {
        id: "fullbright",
        name: "Full Bright",
        description: "Lifts world brightness so caves and night read fully lit.",
        category: ModuleCategory::Visual,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "fov",
        name: "FOV Control",
        description: "Overrides field of view — past the vanilla 110° cap if you want.",
        category: ModuleCategory::Visual,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[ModuleSetting {
            id: "fov",
            label: "Field of view",
            min: 30.0,
            max: 150.0,
            step: 1.0,
            default: 90.0,
        }],
    },
    ModuleDef {
        id: "toggle_sprint",
        name: "Toggle Sprint",
        description: "Holds sprint for you — no need to keep the key pressed.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "toggle_sneak",
        name: "Toggle Sneak",
        description: "Holds sneak for you until you toggle it off.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "no_damage_tilt",
        name: "No Damage Tilt",
        description: "Removes the camera lurch when you take a hit.",
        category: ModuleCategory::Camera,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "no_view_bob",
        name: "No View Bob",
        description: "Stops the walk view-bob without touching vanilla options.",
        category: ModuleCategory::Camera,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "freelook",
        name: "FreeLook",
        description: "Hold the key to pan the camera freely without turning your body.",
        category: ModuleCategory::Camera,
        default_enabled: false,
        default_key: 0,
        hold_key: true,
        settings: &[],
    },
    ModuleDef {
        id: "no_fire_overlay",
        name: "No Fire Overlay",
        description: "Hides the screen-filling fire texture when you catch alight.",
        category: ModuleCategory::Visual,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "crosshair_on_reach",
        name: "Crosshair on Reach",
        description: "Tints the crosshair when the entity under it is within attack reach.",
        category: ModuleCategory::Visual,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[ModuleSetting {
            id: "reach",
            label: "Reach (blocks)",
            min: 2.0,
            max: 6.0,
            step: 0.05,
            default: 3.0,
        }],
    },
    ModuleDef {
        id: "auto_tool",
        name: "Auto Tool",
        description: "Swaps to the best hotbar tool while you mine a block.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "auto_totem",
        name: "Auto Totem",
        description: "Re-equips a totem to your offhand after one pops — real inventory click, with realistic timing.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
];

/// The module's index in [`REGISTRY`] — its shared-buffer slot — by id.
pub fn index_of(id: &str) -> Option<usize> {
    REGISTRY.iter().position(|m| m.id == id)
}

/// A module definition by id.
pub fn get(id: &str) -> Option<&'static ModuleDef> {
    REGISTRY.iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_ids_are_unique() {
        for (i, a) in REGISTRY.iter().enumerate() {
            for b in &REGISTRY[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate module id {}", a.id);
            }
        }
    }

    #[test]
    fn no_module_exceeds_the_buffer_record() {
        for m in REGISTRY {
            assert!(
                m.settings.len() <= MAX_SETTINGS,
                "module {} has {} settings, MAX_SETTINGS is {}",
                m.id,
                m.settings.len(),
                MAX_SETTINGS,
            );
        }
    }

    #[test]
    fn index_of_round_trips() {
        for (i, m) in REGISTRY.iter().enumerate() {
            assert_eq!(index_of(m.id), Some(i));
        }
        assert_eq!(index_of("not-a-module"), None);
    }

    #[test]
    fn the_shipped_modules_are_present() {
        for id in [
            "fullbright",
            "fov",
            "toggle_sprint",
            "toggle_sneak",
            "no_damage_tilt",
            "no_view_bob",
            "freelook",
            "no_fire_overlay",
            "crosshair_on_reach",
            "auto_tool",
            "auto_totem",
        ] {
            assert!(get(id).is_some(), "missing module {id}");
        }
    }

    #[test]
    fn setting_ids_are_unique_within_a_module() {
        for m in REGISTRY {
            for (i, a) in m.settings.iter().enumerate() {
                for b in &m.settings[i + 1..] {
                    assert_ne!(a.id, b.id, "duplicate setting {} in module {}", a.id, m.id);
                }
            }
        }
    }

    #[test]
    fn freelook_is_the_only_hold_key_module() {
        for m in REGISTRY {
            assert_eq!(
                m.hold_key,
                m.id == "freelook",
                "unexpected hold_key on module {}",
                m.id,
            );
        }
    }
}
