//! `ItemTags` membership, read from the data pack the client jar ships.
//!
//! M19's arm poses need exactly one tag. `AvatarRenderer.getArmPose` ends:
//!
//! ```text
//! SwingAnimation attack = itemInHand.get(DataComponents.SWING_ANIMATION);
//! if (attack != null && attack.type() == STAB && avatar.swinging) return SPEAR;
//! else return itemInHand.is(ItemTags.SPEARS) ? SPEAR : ITEM;
//! ```
//!
//! so a spear held by an entity that is **not** swinging still poses `SPEAR`,
//! and only the tag decides it.
//!
//! **This deliberately does not reuse the swing-animation table.** "Is in
//! `minecraft:spears`" and "its `swing_animation` component is STAB" select the
//! same seven items in vanilla 26.2 and are different questions: a sword the
//! server patched to STAB is not in the tag, and a spear patched to WHACK still
//! is. Answering one with the other would be right by coincidence.
//!
//! The tag ships inside the client jar at
//! `data/minecraft/tags/item/spears.json`, so this reads the same production
//! artefact the asset bake does rather than the decompile directory.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use crate::items::Items;

/// `data/minecraft/tags/item/spears.json` inside the client jar.
const SPEARS_TAG_PATH: &str = "data/minecraft/tags/item/spears.json";

/// A resolved item-tag membership set, keyed by item protocol id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemTag {
    ids: HashSet<i32>,
}

impl ItemTag {
    /// Whether `item_id` is in this tag. An id outside the item registry is
    /// never a member — the caller has already classified it `Unknown` and must
    /// not reach a pose decision with it.
    pub fn contains(&self, item_id: i32) -> bool {
        self.ids.contains(&item_id)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Load `minecraft:spears` from the client jar and resolve every entry to a
    /// protocol id.
    ///
    /// **Fails loud on every unrecognised form** rather than dropping entries:
    /// a missing tag file, an empty value list, a `#other_tag` reference, an
    /// object-form `{"id": …, "required": …}` entry, a non-string value, or a
    /// name absent from the item registry. Any of those silently shrinking the
    /// set would put `ArmPose::Item` on a spear and call it vanilla. Neither
    /// referenced form occurs in 26.2's `spears.json`; if a version bump
    /// introduces one, this stops the client instead of guessing.
    pub fn load_spears(client_jar: &Path, items: &Items) -> Result<Self, String> {
        let file = std::fs::File::open(client_jar)
            .map_err(|e| format!("open {}: {e}", client_jar.display()))?;
        let mut jar = zip::ZipArchive::new(std::io::BufReader::new(file))
            .map_err(|e| format!("zip {}: {e}", client_jar.display()))?;
        let mut text = String::new();
        jar.by_name(SPEARS_TAG_PATH)
            .map_err(|e| {
                format!(
                    "{}: {SPEARS_TAG_PATH} missing from the client jar ({e}) — \
                     ItemTags.SPEARS decides the SPEAR arm pose and cannot be inferred",
                    client_jar.display()
                )
            })?
            .read_to_string(&mut text)
            .map_err(|e| format!("read {SPEARS_TAG_PATH}: {e}"))?;
        Self::from_json(&text, items, SPEARS_TAG_PATH)
    }

    /// The parse + resolve half of [`ItemTag::load_spears`], split out so the
    /// fail-closed branches are reachable from a test without a client jar.
    pub(crate) fn from_json(text: &str, items: &Items, source: &str) -> Result<Self, String> {
        let json: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("parse {source}: {e}"))?;
        let values = json
            .get("values")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("{source}: no `values` array"))?;
        if values.is_empty() {
            return Err(format!(
                "{source}: `values` is empty — an empty spears tag would silently \
                 pose every spear as ArmPose::Item"
            ));
        }
        let mut ids = HashSet::with_capacity(values.len());
        for v in values {
            let name = v.as_str().ok_or_else(|| {
                format!(
                    "{source}: entry {v} is not a plain string — the object form \
                     {{id, required}} is not modelled"
                )
            })?;
            if let Some(rest) = name.strip_prefix('#') {
                return Err(format!(
                    "{source}: entry references tag `{rest}` — nested tag references \
                     are not modelled"
                ));
            }
            let id = items
                .id(name)
                .ok_or_else(|| format!("{source}: `{name}` is not a registered item"))?;
            ids.insert(id);
        }
        log::info!("rewo-data: item tag {source} — {} item(s) resolved", ids.len());
        Ok(Self { ids })
    }

    /// Build a tag directly from ids. Tests and oracles only — the production
    /// path always goes through [`ItemTag::load_spears`].
    pub fn from_ids(ids: impl IntoIterator<Item = i32>) -> Self {
        Self {
            ids: ids.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-item registry written through the real `Items::load`, so these
    /// tests exercise the production name→id resolution too.
    fn items() -> Items {
        let dir = std::env::temp_dir().join("rewo-item-tags-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("registries.json");
        std::fs::write(
            &p,
            r#"{"minecraft:item":{"entries":{
                 "minecraft:iron_spear":{"protocol_id":7},
                 "minecraft:stone_sword":{"protocol_id":9}}}}"#,
        )
        .unwrap();
        Items::load(&p).unwrap()
    }

    #[test]
    fn a_plain_value_list_resolves_to_protocol_ids() {
        let t = ItemTag::from_json(r#"{"values":["minecraft:iron_spear"]}"#, &items(), "t").unwrap();
        assert!(t.contains(7));
        assert!(!t.contains(9));
        assert_eq!(t.len(), 1);
    }

    /// Each of these would silently shrink the tag and put `ArmPose::Item` on a
    /// spear. They must stop the client instead.
    #[test]
    fn every_unrecognised_form_fails_closed() {
        let i = items();
        for (name, body) in [
            // `r##` because the body itself contains the `"#` sequence.
            (
                "nested tag reference",
                r##"{"values":["#minecraft:swords"]}"##,
            ),
            (
                "object form",
                r#"{"values":[{"id":"minecraft:iron_spear","required":false}]}"#,
            ),
            ("unregistered item", r#"{"values":["minecraft:not_an_item"]}"#),
            ("empty list", r#"{"values":[]}"#),
            ("no values key", r#"{"replace":false}"#),
            ("not json", r#"{"values":"#),
        ] {
            assert!(
                ItemTag::from_json(body, &i, "t").is_err(),
                "{name} should have failed closed"
            );
        }
    }
}
