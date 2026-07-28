//! The two static inputs an enchantment tooltip line needs (M42): the display
//! strings, and the tags that colour and order the lines.
//!
//! The *dynamic* half — which registry id is which enchantment, and each one's
//! `max_level` — cannot live here. `minecraft:enchantment` is a **datapack**
//! registry, so its contents and its id order are whatever the server's packs
//! say; that arrives at runtime in `registry_data` and is parsed by
//! [`rewo_net::enchantment_parse`]. This module is only what the *client* ships.
//!
//! Both sources are read out of the client jar, so neither can drift from the
//! assets they describe:
//!
//! - the loaded [`crate::lang::Language`] for `enchantment.minecraft.<path>`
//!   and `enchantment.level.<n>`,
//! - `data/minecraft/tags/enchantment/curse.json` and `tooltip_order.json`.
//!
//! The strings come through `Language` rather than straight out of
//! `en_us.json` because the raw file is not the language map (M50) — see
//! [`crate::lang`]. None of 26.2's 54 `enchantment.*` keys is removed or
//! renamed, so the two readings agree *today*; taking the map anyway is what
//! stops a future rename from silently blanking a tooltip line.
//!
//! The tags being under `data/` rather than `assets/` is not a mistake — the
//! client jar carries the vanilla datapack, which is also where M19 reads
//! `ItemTags.SPEARS` from.

use std::collections::HashMap;
use std::path::Path;

/// Everything the client itself knows about enchantment display.
#[derive(Clone, Debug, Default)]
pub struct EnchantmentText {
    /// `enchantment.*` translations, keyed by the full translation key so a
    /// datapack enchantment naming an unusual key still resolves.
    names: HashMap<String, String>,
    /// `minecraft:curse` — these lines are red rather than grey.
    curse: Vec<String>,
    /// `minecraft:tooltip_order`, in order. Enchantments outside it are
    /// listed after, in the order the stack carries them.
    order: Vec<String>,
}

impl EnchantmentText {
    pub fn load(client_jar: &Path, lang: &crate::lang::Language) -> Self {
        let mut out = Self::default();
        out.names = lang
            .iter()
            .filter(|(k, _)| k.starts_with("enchantment."))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        out.curse = Self::tag(client_jar, "curse");
        out.order = Self::tag(client_jar, "tooltip_order");
        log::info!(
            "rewo-data: {} enchantment string(s), {} curse(s), {} in tooltip order",
            out.names.len(),
            out.curse.len(),
            out.order.len()
        );
        out
    }

    /// One `data/minecraft/tags/enchantment/<name>.json` value list.
    ///
    /// A tag file may reference another tag with a `#` prefix; the two read
    /// here are flat lists of ids, and a `#` entry is skipped rather than
    /// followed — an unresolved reference would otherwise silently drop every
    /// id after it.
    fn tag(client_jar: &Path, name: &str) -> Vec<String> {
        let path = format!("data/minecraft/tags/enchantment/{name}.json");
        let Some(raw) = crate::assets::jar_text(client_jar, &path) else {
            return Vec::new();
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Vec::new();
        };
        json.get("values")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .filter(|s| !s.starts_with('#'))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Translate a key, or `None` if this build has no string for it.
    ///
    /// Returning `None` rather than the key keeps a missing translation out of
    /// the tooltip entirely — a line reading `enchantment.somepack.thing` is
    /// worse than no line, and the caller decides.
    pub fn translate(&self, key: &str) -> Option<&str> {
        self.names.get(key).map(String::as_str)
    }

    /// `Component.translatable("enchantment.level." + level)`.
    ///
    /// 26.2 ships I through X. **Vanilla renders the raw key past that**, so a
    /// level-11 enchantment shows `enchantment.level.11` in the real client;
    /// this returns the number instead, which is the one deliberate divergence
    /// here and is strictly more readable than the key.
    pub fn level(&self, level: i32) -> String {
        self.translate(&format!("enchantment.level.{level}"))
            .map(str::to_string)
            .unwrap_or_else(|| level.to_string())
    }

    /// `enchantment.is(EnchantmentTags.CURSE)` — a curse's line is red.
    pub fn is_curse(&self, id: &str) -> bool {
        self.curse.iter().any(|c| c == id)
    }

    /// `EnchantmentTags.TOOLTIP_ORDER`'s position, or `None` for one outside
    /// the tag.
    ///
    /// The order is not cosmetic sugar: `addToTooltip` walks the tag first and
    /// then appends whatever the stack carries that the tag does not mention,
    /// so an enchantment missing from the tag lands **after** every one that
    /// is in it, whatever its id.
    pub fn tooltip_rank(&self, id: &str) -> Option<usize> {
        self.order.iter().position(|o| o == id)
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}
