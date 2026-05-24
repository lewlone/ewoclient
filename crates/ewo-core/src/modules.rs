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
/// Bumped 2 → 8 with `EwoModuleData.SCHEMA_VERSION` 1 → 2 to unblock modules
/// that need more than 2 sliders (Mace Combo: 4) and to leave headroom.
pub const MAX_SETTINGS: usize = 8;

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
    ModuleDef {
        id: "legit_elytra_swap",
        name: "Legit Elytra Swap",
        description: "Bound key swaps chestplate ↔ elytra via real inventory clicks.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "hand_restock",
        name: "Hand Restock",
        description: "Swap a fresh stack of the same item into the held slot when it runs low (or runs out).",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[ModuleSetting {
            id: "threshold",
            label: "Refill at count ≤",
            min: 0.0,
            max: 8.0,
            step: 1.0,
            default: 0.0,
        }],
    },
    ModuleDef {
        id: "no_pumpkin_overlay",
        name: "No Pumpkin Overlay",
        description: "Hides the pumpkin-blur overlay when you wear a carved pumpkin on your head.",
        category: ModuleCategory::Visual,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "hit_color",
        name: "Hit Color",
        description: "Suppresses the vanilla red hurt-flash on entities you damage. (Color picker in a follow-up.)",
        category: ModuleCategory::Visual,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "sprint_tap",
        name: "Sprint Tap",
        description: "After each attack re-engages sprint so the next hit also gets vanilla's knockback boost.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "auto_eat",
        name: "Auto Eat",
        description: "When hunger drops below the threshold, swap to a food slot and eat through a real right-click cycle.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[ModuleSetting {
            id: "threshold",
            label: "Eat when hunger ≤",
            min: 0.0,
            max: 20.0,
            step: 1.0,
            default: 16.0,
        }],
    },
    ModuleDef {
        id: "auto_mace_swap",
        name: "Auto Mace Swap",
        description: "Swap to a hotbar mace when you've fallen far enough to land a smash attack.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[ModuleSetting {
            id: "min_fall",
            label: "Min fall (blocks)",
            min: 1.5,
            max: 8.0,
            step: 0.5,
            default: 1.5,
        }],
    },
    ModuleDef {
        id: "auto_jump_reset",
        name: "Auto Jump Reset",
        description: "Auto-press jump when you take damage so vanilla converts the knockback into a vertical reset.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "auto_crit",
        name: "Auto Crit",
        description: "Hold jump while attacking so every hit lands mid-air and triggers a vanilla critical. Visible bunny-hop.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "auto_pearl",
        name: "Auto Pearl",
        description: "Bound key throws an ender pearl from a hotbar slot then swaps back — clutch escape via real swap + use + swap-back.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "riptide_boost",
        name: "Riptide Boost",
        description: "Bound key swaps to a trident, holds use long enough for Riptide to charge, then swaps back. Vanilla self-no-ops outside rain/water.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[ModuleSetting {
            id: "charge_ms",
            label: "Hold use (ms)",
            min: 400.0,
            max: 1500.0,
            step: 50.0,
            default: 700.0,
        }],
    },
    ModuleDef {
        id: "mace_combo",
        name: "Mace Combo",
        description: "Bound key fires axe → spear → mace at the configured per-hit delay. At 50 ms = tick-perfect stun-slam; dial up to 500 ms for a paced human-cadence version.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[
            ModuleSetting {
                id: "axe_slot",
                label: "Axe hotbar slot",
                min: 0.0,
                max: 8.0,
                step: 1.0,
                default: 0.0,
            },
            ModuleSetting {
                id: "spear_slot",
                label: "Spear hotbar slot",
                min: 0.0,
                max: 8.0,
                step: 1.0,
                default: 1.0,
            },
            ModuleSetting {
                id: "mace_slot",
                label: "Mace hotbar slot",
                min: 0.0,
                max: 8.0,
                step: 1.0,
                default: 2.0,
            },
            ModuleSetting {
                id: "per_hit_delay_ms",
                label: "Per-hit delay (ms)",
                min: 50.0,
                max: 500.0,
                step: 10.0,
                default: 50.0,
            },
        ],
    },
    ModuleDef {
        id: "reach_lock",
        name: "Reach Lock",
        description: "Auto-releases the forward key when a targeted entity is within the configured optimal distance. Keeps you from overshooting sword range.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[ModuleSetting {
            id: "max_distance",
            label: "Stay at distance ≥ (blocks)",
            min: 1.5,
            max: 5.0,
            step: 0.1,
            default: 2.5,
        }],
    },
    ModuleDef {
        id: "auto_hit_timing",
        name: "Auto Hit Timing",
        description: "While the attack key is held, auto-fires one attack each time the attack-strength meter hits the threshold. Replaces spam-clicking with perfect-cadence full-charge hits.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[ModuleSetting {
            id: "min_charge",
            label: "Fire at charge ≥",
            min: 0.5,
            max: 1.0,
            step: 0.05,
            default: 0.95,
        }],
    },
    ModuleDef {
        id: "knockback_max",
        name: "Knockback Maximizer",
        description: "Engages sprint at the moment of attack so the first hit of a sequence also gets vanilla's +1 knockback level (Sprint Tap covers the subsequent hits).",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[],
    },
    ModuleDef {
        id: "triggerbot",
        name: "Triggerbot",
        description: "Autonomously fires an attack when your crosshair is on a living entity within reach and your attack-strength is full. Doesn't aim — that's still your job. Target filter lets you skip villagers / pets.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[
            ModuleSetting {
                id: "reach",
                label: "Max reach (blocks)",
                min: 1.5,
                max: 6.0,
                step: 0.05,
                default: 3.0,
            },
            ModuleSetting {
                id: "min_charge",
                label: "Fire at charge ≥",
                min: 0.5,
                max: 1.0,
                step: 0.05,
                default: 0.95,
            },
            ModuleSetting {
                id: "target_filter",
                label: "Targets (0=any, 1=players, 2=hostile+players)",
                min: 0.0,
                max: 2.0,
                step: 1.0,
                default: 2.0,
            },
            ModuleSetting {
                id: "require_attack_held",
                label: "Require attack key held (0=no, 1=yes)",
                min: 0.0,
                max: 1.0,
                step: 1.0,
                default: 0.0,
            },
        ],
    },
    ModuleDef {
        id: "hit_indicator",
        name: "Hit Indicator",
        description: "Screen-edge chevron pointing back toward whoever just hit you, fading over 1 s. Pure HUD.",
        category: ModuleCategory::Visual,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[
            ModuleSetting {
                id: "radius_pct",
                label: "Chevron radius (%)",
                min: 15.0,
                max: 40.0,
                step: 1.0,
                default: 25.0,
            },
            ModuleSetting {
                id: "fade_seconds",
                label: "Fade duration (s)",
                min: 0.5,
                max: 3.0,
                step: 0.1,
                default: 1.0,
            },
        ],
    },
    ModuleDef {
        id: "wind_charge_mlg",
        name: "Wind Charge MLG",
        description: "Auto-throws a hotbar wind charge to break a fatal fall. Off-mode (default) fires only when you're already looking sufficiently downward; snap-mode briefly snaps pitch to straight-down for a reliable save.",
        category: ModuleCategory::Movement,
        default_enabled: false,
        default_key: 0,
        hold_key: false,
        settings: &[
            ModuleSetting {
                id: "min_fall",
                label: "Fire when fall ≥ (blocks)",
                min: 10.0,
                max: 50.0,
                step: 1.0,
                default: 15.0,
            },
            ModuleSetting {
                id: "min_pitch",
                label: "Min look-down (deg)",
                min: 30.0,
                max: 90.0,
                step: 5.0,
                default: 60.0,
            },
            ModuleSetting {
                id: "snap_pitch",
                label: "Snap pitch (0=off, 1=on)",
                min: 0.0,
                max: 1.0,
                step: 1.0,
                default: 0.0,
            },
        ],
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
            "legit_elytra_swap",
            "hand_restock",
            "no_pumpkin_overlay",
            "hit_color",
            "sprint_tap",
            "auto_eat",
            "auto_mace_swap",
            "auto_jump_reset",
            "auto_crit",
            "auto_pearl",
            "riptide_boost",
            "mace_combo",
            "reach_lock",
            "auto_hit_timing",
            "knockback_max",
            "triggerbot",
            "wind_charge_mlg",
            "hit_indicator",
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
