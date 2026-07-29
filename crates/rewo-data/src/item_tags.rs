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

    /// A directory no other fixture can be using.
    ///
    /// This was one fixed path, and that was a race with two independent
    /// reaches. `std::env::temp_dir()` is **machine-wide**, and every caller
    /// writes the fixture and then reads it back, so:
    ///
    /// * within one binary, the tests below run on separate threads, and
    /// * across binaries, every concurrent sweep on the machine — a second
    ///   worktree's `cargo test`, a re-run overlapping the last — landed on the
    ///   identical file.
    ///
    /// `fs::write` opens with `O_TRUNC`, so a reader arriving between the
    /// truncate and the bytes sees a **zero-byte file** and `Items::load` fails
    /// with `EOF while parsing a value at line 1 column 0`. Intermittent by
    /// nature: it needs a reader inside a window a few microseconds wide, which
    /// is why it surfaced as "roughly one sweep in four" rather than as a
    /// reproducible failure, and why it masked real failures instead of
    /// reporting itself.
    ///
    /// The process id makes it safe across binaries and the counter across
    /// threads; both are needed, and neither alone closes it.
    fn fixture_dir() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "rewo-item-tags-test-{}-{n}",
            std::process::id()
        ))
    }

    /// Write the two-item registry fixture, returning its directory and path.
    fn write_fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = fixture_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("registries.json");
        std::fs::write(
            &p,
            r#"{"minecraft:item":{"entries":{
                 "minecraft:iron_spear":{"protocol_id":7},
                 "minecraft:stone_sword":{"protocol_id":9}}}}"#,
        )
        .unwrap();
        (dir, p)
    }

    /// A two-item registry written through the real `Items::load`, so these
    /// tests exercise the production name→id resolution too.
    fn items() -> Items {
        let (dir, p) = write_fixture();
        let loaded = Items::load(&p);
        let _ = std::fs::remove_dir_all(&dir);
        loaded.unwrap()
    }

    /// **The race witness.** Every caller of [`items`] writes a fixture and
    /// then reads it back, so two callers sharing one path interleave as
    /// truncate / read / write: `fs::write` opens with `O_TRUNC`, and a reader
    /// landing in the gap before the bytes arrive sees a **zero-byte file**.
    ///
    /// Mutation partner: put the fixture back on one fixed path
    /// (`temp_dir().join("rewo-item-tags-test")`) and this fails with
    /// `EOF while parsing a value at line 1 column 0` — which is exactly how it
    /// failed in the wild, intermittently, masking real failures in every sweep.
    #[test]
    fn concurrent_fixtures_never_observe_a_half_written_file() {
        let failures = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::scope(|s| {
            for _ in 0..16 {
                let failures = std::sync::Arc::clone(&failures);
                s.spawn(move || {
                    for _ in 0..8 {
                        // Not `items()` — that unwraps, and a panic in a scoped
                        // thread would abort the run rather than count.
                        let (dir, p) = write_fixture();
                        let loaded = Items::load(&p);
                        let _ = std::fs::remove_dir_all(&dir);
                        if loaded.is_err() {
                            failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        assert_eq!(
            failures.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a fixture was read while another writer had it truncated"
        );
    }

    /// The property the fix turns on, asserted directly rather than through the
    /// probabilistic race above: no two fixtures share a path, and the path is
    /// process-scoped so a second test binary on the same machine — another
    /// worktree's sweep, `temp_dir()` being machine-wide — cannot collide with
    /// this one either.
    #[test]
    fn each_fixture_gets_its_own_process_scoped_directory() {
        let a = fixture_dir();
        let b = fixture_dir();
        assert_ne!(a, b, "two fixtures shared a directory");
        let pid = std::process::id().to_string();
        assert!(
            a.to_string_lossy().contains(&pid),
            "{} is not process-scoped",
            a.display()
        );
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
