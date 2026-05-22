//! Module-extensible keybind registry — Phase F5c.
//!
//! A *keybind* binds an EwoClient action to a key. The registry ([`REGISTRY`])
//! lists every [`KeybindAction`]: the core `overlay.open` key plus one per
//! EwoClient **module** (Phase G), derived from [`ewo_core::modules::REGISTRY`].
//!
//! Bindings are stored per client profile (`profiles/<name>/client.toml`),
//! so each profile carries its own key layout — see [`crate::profile`].
//!
//! Keys are stored as **GLFW key codes** — the namespace Minecraft itself
//! uses — so the in-game side compares the stored integer directly with no
//! translation. The launcher captures keys via winit; [`KeyChord::from_winit`]
//! maps winit's `KeyCode` into the GLFW namespace at capture time.
//!
//! F5c captures plain keys (no modifier combos): every F keybind is a single
//! key. The `mods` field on [`KeyChord`] keeps the model honest for module
//! keybinds that may want combos later — the file format and labels already
//! carry it.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use winit::keyboard::KeyCode;

// ── GLFW key codes (subset — the ones we name or default to) ──────────────
// Values from glfw3.h. The full winit→GLFW table lives in `winit_to_glfw`.

/// `GLFW_KEY_RIGHT_SHIFT` — the default overlay-open key.
pub const GLFW_RIGHT_SHIFT: i32 = 344;

/// GLFW modifier bits, for [`KeyChord::mods`].
pub mod glfw_mod {
    pub const SHIFT: u8 = 0x01;
    pub const CONTROL: u8 = 0x02;
    pub const ALT: u8 = 0x04;
    pub const SUPER: u8 = 0x08;
}

// ── KeyChord ──────────────────────────────────────────────────────────────

/// A bound key: a GLFW key code plus a modifier bitmask ([`glfw_mod`]).
/// Phase F's one keybind uses no modifiers; the field is kept so module
/// keybinds can carry combos without a format change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyChord {
    pub key: i32,
    #[serde(default)]
    pub mods: u8,
}

impl KeyChord {
    /// An unbound chord. Key `0` is never delivered as a real GLFW press, so
    /// an action left at `UNBOUND` simply never fires. Module keybinds default
    /// to this — the user opts in by binding a key.
    pub const UNBOUND: KeyChord = KeyChord { key: 0, mods: 0 };

    /// A plain key chord — no modifiers.
    pub const fn new(key: i32) -> Self {
        Self { key, mods: 0 }
    }

    /// Whether this chord is bound to a real key.
    pub fn is_bound(self) -> bool {
        self.key != 0
    }

    /// Build a chord from a captured winit key press. Returns `None` for
    /// keys with no GLFW equivalent (they can't be bound). When the pressed
    /// key is itself a modifier, the chord carries no modifier bits.
    pub fn from_winit(code: KeyCode) -> Option<Self> {
        winit_to_glfw(code).map(KeyChord::new)
    }

    /// Human-readable, e.g. `"Right Shift"`, `"Ctrl + Z"`, or `"Unbound"`.
    pub fn label(&self) -> String {
        if !self.is_bound() {
            return "Unbound".to_string();
        }
        let mut s = String::new();
        if self.mods & glfw_mod::CONTROL != 0 {
            s.push_str("Ctrl + ");
        }
        if self.mods & glfw_mod::ALT != 0 {
            s.push_str("Alt + ");
        }
        if self.mods & glfw_mod::SHIFT != 0 {
            s.push_str("Shift + ");
        }
        if self.mods & glfw_mod::SUPER != 0 {
            s.push_str("Super + ");
        }
        s.push_str(&key_name(self.key));
        s
    }
}

// ── Registry ──────────────────────────────────────────────────────────────

/// One bindable action. `module` groups actions in the UI; `id` is the
/// stable key used in `client.toml` and the in-game keybinds file.
#[derive(Debug, Clone, Copy)]
pub struct KeybindAction {
    pub id: &'static str,
    pub label: &'static str,
    pub module: &'static str,
    pub default: KeyChord,
}

/// Every bindable action: the core overlay key, then one per EwoClient module
/// ([`ewo_core::modules`]). Phase G fills the module-extensible seam Phase F5c
/// built — deriving the module keybinds here means the Keybinds tab,
/// `client.toml`, and the in-game `ewo-keybinds.txt` all pick them up for free.
///
/// A `LazyLock`, not a `const`: the module catalog is `&'static` data but a
/// `const` can't iterate it. Built once, on first use.
pub static REGISTRY: LazyLock<Vec<KeybindAction>> = LazyLock::new(|| {
    let mut actions = vec![KeybindAction {
        id: "overlay.open",
        label: "Open the overlay",
        module: "Core",
        default: KeyChord::new(GLFW_RIGHT_SHIFT),
    }];
    // One action per module — its id matches the module id, so the chord
    // round-trips through `client.toml` and `ewo-keybinds.txt` unchanged.
    for def in ewo_core::modules::REGISTRY {
        actions.push(KeybindAction {
            id: def.id,
            label: def.name,
            module: "Modules",
            // `default_key == 0` means the catalog ships the module unbound.
            default: if def.default_key == 0 {
                KeyChord::UNBOUND
            } else {
                KeyChord::new(def.default_key)
            },
        });
    }
    actions
});

// ── Display ───────────────────────────────────────────────────────────────

/// Display name for a GLFW key code — `"A"`, `"F3"`, `"Right Shift"`, …
pub fn key_name(code: i32) -> String {
    match code {
        65..=90 => ((b'A' + (code - 65) as u8) as char).to_string(), // A–Z
        48..=57 => ((b'0' + (code - 48) as u8) as char).to_string(), // 0–9
        290..=314 => format!("F{}", code - 289),                     // F1–F25
        320..=329 => format!("Num {}", code - 320),                  // Numpad 0–9
        32 => "Space".into(),
        39 => "'".into(),
        44 => ",".into(),
        45 => "-".into(),
        46 => ".".into(),
        47 => "/".into(),
        59 => ";".into(),
        61 => "=".into(),
        91 => "[".into(),
        92 => "\\".into(),
        93 => "]".into(),
        96 => "`".into(),
        256 => "Escape".into(),
        257 => "Enter".into(),
        258 => "Tab".into(),
        259 => "Backspace".into(),
        260 => "Insert".into(),
        261 => "Delete".into(),
        262 => "Right".into(),
        263 => "Left".into(),
        264 => "Down".into(),
        265 => "Up".into(),
        266 => "Page Up".into(),
        267 => "Page Down".into(),
        268 => "Home".into(),
        269 => "End".into(),
        280 => "Caps Lock".into(),
        281 => "Scroll Lock".into(),
        282 => "Num Lock".into(),
        283 => "Print Screen".into(),
        284 => "Pause".into(),
        330 => "Num .".into(),
        331 => "Num /".into(),
        332 => "Num *".into(),
        333 => "Num -".into(),
        334 => "Num +".into(),
        335 => "Num Enter".into(),
        336 => "Num =".into(),
        340 => "Left Shift".into(),
        341 => "Left Ctrl".into(),
        342 => "Left Alt".into(),
        343 => "Left Super".into(),
        344 => "Right Shift".into(),
        345 => "Right Ctrl".into(),
        346 => "Right Alt".into(),
        347 => "Right Super".into(),
        348 => "Menu".into(),
        _ => format!("Key {code}"),
    }
}

// ── winit → GLFW ──────────────────────────────────────────────────────────

/// Map a winit physical key to its GLFW key code. `None` for keys GLFW
/// doesn't name (they can't be bound). winit's `KeyCode` is `#[non_exhaustive]`
/// — the wildcard arm handles anything exotic.
fn winit_to_glfw(code: KeyCode) -> Option<i32> {
    use KeyCode as K;
    let v = match code {
        K::KeyA => 65, K::KeyB => 66, K::KeyC => 67, K::KeyD => 68,
        K::KeyE => 69, K::KeyF => 70, K::KeyG => 71, K::KeyH => 72,
        K::KeyI => 73, K::KeyJ => 74, K::KeyK => 75, K::KeyL => 76,
        K::KeyM => 77, K::KeyN => 78, K::KeyO => 79, K::KeyP => 80,
        K::KeyQ => 81, K::KeyR => 82, K::KeyS => 83, K::KeyT => 84,
        K::KeyU => 85, K::KeyV => 86, K::KeyW => 87, K::KeyX => 88,
        K::KeyY => 89, K::KeyZ => 90,
        K::Digit0 => 48, K::Digit1 => 49, K::Digit2 => 50, K::Digit3 => 51,
        K::Digit4 => 52, K::Digit5 => 53, K::Digit6 => 54, K::Digit7 => 55,
        K::Digit8 => 56, K::Digit9 => 57,
        K::Backquote => 96,
        K::Minus => 45,
        K::Equal => 61,
        K::BracketLeft => 91,
        K::BracketRight => 93,
        K::Backslash => 92,
        K::Semicolon => 59,
        K::Quote => 39,
        K::Comma => 44,
        K::Period => 46,
        K::Slash => 47,
        K::Space => 32,
        K::Enter => 257,
        K::Tab => 258,
        K::Backspace => 259,
        K::Escape => 256,
        K::CapsLock => 280,
        K::Insert => 260,
        K::Delete => 261,
        K::Home => 268,
        K::End => 269,
        K::PageUp => 266,
        K::PageDown => 267,
        K::ArrowUp => 265,
        K::ArrowDown => 264,
        K::ArrowLeft => 263,
        K::ArrowRight => 262,
        K::ShiftLeft => 340,
        K::ControlLeft => 341,
        K::AltLeft => 342,
        K::SuperLeft => 343,
        K::ShiftRight => 344,
        K::ControlRight => 345,
        K::AltRight => 346,
        K::SuperRight => 347,
        K::ContextMenu => 348,
        K::PrintScreen => 283,
        K::ScrollLock => 281,
        K::Pause => 284,
        K::NumLock => 282,
        K::Numpad0 => 320, K::Numpad1 => 321, K::Numpad2 => 322,
        K::Numpad3 => 323, K::Numpad4 => 324, K::Numpad5 => 325,
        K::Numpad6 => 326, K::Numpad7 => 327, K::Numpad8 => 328,
        K::Numpad9 => 329,
        K::NumpadDecimal => 330,
        K::NumpadDivide => 331,
        K::NumpadMultiply => 332,
        K::NumpadSubtract => 333,
        K::NumpadAdd => 334,
        K::NumpadEnter => 335,
        K::NumpadEqual => 336,
        K::F1 => 290, K::F2 => 291, K::F3 => 292, K::F4 => 293,
        K::F5 => 294, K::F6 => 295, K::F7 => 296, K::F8 => 297,
        K::F9 => 298, K::F10 => 299, K::F11 => 300, K::F12 => 301,
        K::F13 => 302, K::F14 => 303, K::F15 => 304, K::F16 => 305,
        K::F17 => 306, K::F18 => 307, K::F19 => 308, K::F20 => 309,
        K::F21 => 310, K::F22 => 311, K::F23 => 312, K::F24 => 313,
        K::F25 => 314,
        _ => return None,
    };
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_overlay_open_defaulting_to_right_shift() {
        let a = REGISTRY
            .iter()
            .find(|a| a.id == "overlay.open")
            .expect("overlay.open registered");
        assert_eq!(a.default.key, GLFW_RIGHT_SHIFT);
        assert_eq!(a.default.mods, 0);
    }

    #[test]
    fn registry_has_one_action_per_module() {
        for m in ewo_core::modules::REGISTRY {
            let a = REGISTRY
                .iter()
                .find(|a| a.id == m.id)
                .unwrap_or_else(|| panic!("no keybind action for module {}", m.id));
            assert_eq!(a.default.key, m.default_key);
        }
    }

    #[test]
    fn module_keybinds_default_unbound() {
        // The starter modules ship with no key — the user opts in.
        let fb = REGISTRY
            .iter()
            .find(|a| a.id == "fullbright")
            .expect("fullbright action registered");
        assert!(!fb.default.is_bound());
        assert_eq!(fb.default.label(), "Unbound");
    }

    #[test]
    fn from_winit_maps_into_the_glfw_namespace() {
        assert_eq!(KeyChord::from_winit(KeyCode::KeyZ), Some(KeyChord::new(90)));
        assert_eq!(
            KeyChord::from_winit(KeyCode::ShiftRight),
            Some(KeyChord::new(GLFW_RIGHT_SHIFT)),
        );
        assert_eq!(KeyChord::from_winit(KeyCode::F3), Some(KeyChord::new(292)));
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(KeyChord::new(GLFW_RIGHT_SHIFT).label(), "Right Shift");
        assert_eq!(KeyChord::new(90).label(), "Z");
        assert_eq!(KeyChord::new(292).label(), "F3");
        assert_eq!(
            KeyChord { key: 90, mods: glfw_mod::CONTROL }.label(),
            "Ctrl + Z",
        );
    }

    #[test]
    fn chord_round_trips_through_toml() {
        let c = KeyChord { key: 292, mods: glfw_mod::CONTROL };
        let text = toml::to_string(&c).expect("serialize");
        let back: KeyChord = toml::from_str(&text).expect("deserialize");
        assert_eq!(back, c);
    }
}
