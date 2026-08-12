//! `<assets>/indexes/<id>.json` — the key→hash map, and reading an object back.
//!
//! **The store is content-addressed, so an asset key is a lookup and not a
//! path.** `ResolvedSound::asset_path` gives `<namespace>/sounds/<path>.ogg`,
//! and the bytes live at `<assets>/objects/<hash[0..2]>/<hash>`. Anything that
//! treats the key as a filename finds nothing, on every asset, with a perfectly
//! good "no such file" — which is why this exists as a named facility rather
//! than as two lines at a call site.
//!
//! ## Why this is not `SoundFileSet`
//!
//! [`crate::sounds_json::load_from_asset_store`] already walks this file, and
//! keeps two things out of it: the `sounds.json` documents, and a `HashSet` of
//! every `.ogg` key for `validateSoundResource`. That set answers **existence**
//! and nothing else — it has no hashes in it — so a device holding one can tell
//! that a variant is playable and still not find a single byte of it. The two
//! readers are therefore not redundant: one is for the *bake*, this one is for
//! *playback*.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One parsed asset index.
///
/// Cheap to hold — 26.2's index is on the order of 5,000 entries — and parsed
/// once, because the alternative is re-reading a multi-megabyte JSON file for
/// every sound that is played.
#[derive(Clone, Debug, Default)]
pub struct AssetIndex {
    objects: HashMap<String, String>,
}

impl AssetIndex {
    /// Parse `<assets_root>/indexes/<index_id>.json`.
    ///
    /// `index_id` is the version manifest's `assetIndex.id` (`"32"` for 26.2),
    /// resolved by [`crate::sounds_json::asset_index_id`]. It is deliberately a
    /// parameter rather than a guess from the directory listing, for the reason
    /// that function's own doc gives: a machine with several versions installed
    /// has several indexes, and "the newest file" silently reads another
    /// version's assets.
    pub fn load(assets_root: &Path, index_id: &str) -> Result<AssetIndex, String> {
        let path = assets_root.join("indexes").join(format!("{index_id}.json"));
        let json = crate::read_json_file(&path)?;
        let objects = json
            .get("objects")
            .and_then(|o| o.as_object())
            .ok_or_else(|| format!("{}: no objects", path.display()))?;
        let mut map = HashMap::with_capacity(objects.len());
        for (key, value) in objects {
            let hash = value
                .get("hash")
                .and_then(|h| h.as_str())
                .ok_or_else(|| format!("{}: {key} has no hash", path.display()))?;
            map.insert(key.clone(), hash.to_string());
        }
        Ok(AssetIndex { objects: map })
    }

    /// [`Self::load`] for a version, resolving both paths itself.
    ///
    /// Returns the store root alongside the index because every subsequent
    /// [`Self::read`] needs it, and re-resolving it at each call site is how two
    /// call sites come to disagree about where the store is (M89).
    pub fn load_for_version(version: &str) -> Result<(PathBuf, AssetIndex), String> {
        let root = crate::sounds_json::shared_assets_dir().ok_or("no config dir")?;
        let id = crate::sounds_json::asset_index_id(version).ok_or_else(|| {
            format!("no assetIndex.id for {version} in the shared version manifest")
        })?;
        let index = AssetIndex::load(&root, &id)?;
        Ok((root, index))
    }

    /// The content hash for a key, or `None` if the store does not carry it.
    pub fn hash(&self, key: &str) -> Option<&str> {
        self.objects.get(key).map(String::as_str)
    }

    /// Read one asset's bytes.
    ///
    /// **A missing key and an unreadable object are different errors**, and both
    /// name the key: a device reporting "could not read a sound" without saying
    /// which one is a diagnostic nobody can act on, and the two cases have
    /// different causes (an index that does not list it versus a store that has
    /// lost the object).
    pub fn read(&self, assets_root: &Path, key: &str) -> Result<Vec<u8>, String> {
        let hash = self
            .hash(key)
            .ok_or_else(|| format!("{key} is not in the asset index"))?;
        let path = crate::sounds_json::object_path(assets_root, hash);
        std::fs::read(&path).map_err(|e| format!("read {key} at {}: {e}", path.display()))
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a minimal index plus its object, so the whole resolution is
    /// exercised without needing an unpacked 26.2 store.
    ///
    /// The hash is a real-shaped 40-hex string rather than something short,
    /// because `object_path`'s two-character shard is taken from it and a
    /// two-character hash would make the shard and the filename the same
    /// string — a fixture in which a dropped shard cannot be seen.
    fn store() -> (tempdir::Dir, PathBuf) {
        let dir = tempdir::Dir::new("rewo-asset-index");
        let root = dir.path().to_path_buf();
        let hash = "0123456789abcdef0123456789abcdef01234567";
        std::fs::create_dir_all(root.join("indexes")).unwrap();
        std::fs::write(
            root.join("indexes/32.json"),
            format!(r#"{{"objects":{{"minecraft/sounds/step/grass1.ogg":{{"hash":"{hash}","size":3}}}}}}"#),
        )
        .unwrap();
        let obj = crate::sounds_json::object_path(&root, hash);
        std::fs::create_dir_all(obj.parent().unwrap()).unwrap();
        std::fs::write(&obj, b"OggS").unwrap();
        (dir, root)
    }

    #[test]
    fn a_key_resolves_through_the_hash_to_the_object() {
        let (_d, root) = store();
        let idx = AssetIndex::load(&root, "32").unwrap();
        assert_eq!(idx.len(), 1);
        assert_eq!(
            idx.hash("minecraft/sounds/step/grass1.ogg"),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            idx.read(&root, "minecraft/sounds/step/grass1.ogg").unwrap(),
            b"OggS"
        );
    }

    /// **The key is not a path**, which is the whole reason this type exists.
    ///
    /// A reader that joined the key onto the store root would find nothing here
    /// — and would find nothing in production too, on every sound. Asserting the
    /// path form is absent is what makes the positive test above about the
    /// *index* rather than about a file that happens to be readable.
    #[test]
    fn the_key_is_not_a_filesystem_path() {
        let (_d, root) = store();
        assert!(
            !root.join("minecraft/sounds/step/grass1.ogg").exists(),
            "the store holds objects by hash, never by key"
        );
    }

    #[test]
    fn a_missing_key_and_a_missing_object_are_different_errors() {
        let (_d, root) = store();
        let idx = AssetIndex::load(&root, "32").unwrap();
        let absent = idx.read(&root, "minecraft/sounds/step/grass9.ogg").unwrap_err();
        assert!(absent.contains("not in the asset index"), "{absent}");
        assert!(absent.contains("grass9"), "the error names the key: {absent}");

        // The object removed from under a live index — a store that has lost a
        // file rather than one that never listed it.
        let obj = crate::sounds_json::object_path(&root, idx.hash("minecraft/sounds/step/grass1.ogg").unwrap());
        std::fs::remove_file(&obj).unwrap();
        let gone = idx.read(&root, "minecraft/sounds/step/grass1.ogg").unwrap_err();
        assert!(!gone.contains("not in the asset index"), "{gone}");
        assert!(gone.contains("grass1"), "{gone}");
    }

    #[test]
    fn a_malformed_index_is_an_error_rather_than_an_empty_map() {
        // An empty index is indistinguishable from a store with no sounds, and
        // it is the shape `build_sounds` already refuses to accept silently.
        let dir = tempdir::Dir::new("rewo-asset-index-bad");
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("indexes")).unwrap();
        std::fs::write(root.join("indexes/32.json"), r#"{"nope":{}}"#).unwrap();
        assert!(AssetIndex::load(&root, "32").is_err());
    }

    /// A tiny scoped temp directory, so these tests leave nothing behind.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new(tag: &str) -> Dir {
                // The process id and a counter, rather than a random name: two
                // tests in one binary run concurrently and must not share a
                // directory, and this crate has no rand dependency.
                use std::sync::atomic::{AtomicU32, Ordering};
                static N: AtomicU32 = AtomicU32::new(0);
                let p = std::env::temp_dir().join(format!(
                    "{tag}-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                let _ = std::fs::remove_dir_all(&p);
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
