//! Canonical catalog of mods bundled by EwoLoader.
//!
//! This is the launcher-side mirror of EwoLoader's `BundledMods.BUNDLED_MODS`
//! list — same set of mod ids, but extended with display metadata + the
//! per-mod loader-manifest library prefix so the launcher can:
//!
//! 1. Seed a new Ewo instance's `Instance.mods` with the user-toggleable
//!    subset, so the Instances UI shows real toggle rows immediately.
//! 2. At launch time, map a disabled mod id back to the loader-manifest
//!    library prefix to strip it from the JVM classpath, and feed the same
//!    id list to the loader via `-Dfabric.debug.disableModIds=…` so the
//!    `BundledMods` verification doesn't fire on the intentionally-absent
//!    mod.
//! 3. Detect bundled mods a given loader manifest simply doesn't ship
//!    (e.g. BetterF3 has no MC 26.2 build, so 26.2.json omits it) and
//!    auto-disable them so the loader-side verification passes.
//!
//! The catalog is **version-agnostic**: library matching is by coordinate
//! *prefix* (`maven.modrinth:iris:`), never the full pinned version, so a
//! manifest re-pin (26.1 → 26.2 bundle) needs no launcher change. The
//! per-version truth lives in the EwoLoader manifests.
//!
//! Adding a new bundled mod is a three-place change: this catalog, the
//! EwoLoader manifest's `libraries[]` array, and `BundledMods.BUNDLED_MODS`
//! on the loader side. The three lists must agree on every mod id; the
//! verification in `FabricLoaderImpl.setup()` is the safety net that fails
//! loud when they drift.

use ewo_render::screens::instances::ModInfo;

/// One bundled-mod row.
///
/// `library_prefix` matches the EwoLoader manifest's `libraries[].name`
/// coordinate up to (and including) the version separator — i.e.
/// `group:artifact:`. The launcher walks the merged `PerVersion.libraries`
/// looking for entries whose name starts with this prefix, which keeps the
/// catalog valid across per-MC-version manifest re-pins.
pub struct BundledMod {
    pub display_name: &'static str,
    pub category: &'static str,
    /// Display version shown in the UI mod row. Tracks the *newest*
    /// bundle (currently the 26.2 manifest); instances launched against
    /// an older manifest line may actually load a slightly older build.
    /// Cosmetic only — the classpath truth is the manifest.
    pub version: &'static str,
    /// The mod's `fabric.mod.json` id — this is what `BundledMods.BUNDLED_MODS`
    /// on the loader side asserts at startup, and what
    /// `fabric.debug.disableModIds` expects in its comma-separated list.
    pub mod_id: &'static str,
    /// Version-agnostic coordinate prefix matching the loader manifest's
    /// `libraries[].name` (`group:artifact:` including the trailing colon)
    /// so the classpath-strip step can find this row's library in any
    /// manifest line.
    pub library_prefix: &'static str,
    pub default_on: bool,
    /// Whether this mod is surfaced to the user as a togglable row.
    /// Infrastructure mods (fabric-api, language-kotlin, YACL, placeholder-api)
    /// are bundled but not shown — disabling them would break almost everything.
    pub toggleable: bool,
}

/// Full bundled set. Order here drives both the UI mod-list order and the
/// classpath order (the loader manifest's order is independent but kept
/// roughly aligned for readability — what matters at launch is mod-id
/// lookups, not array index).
pub const CATALOG: &[BundledMod] = &[
    // ── Infrastructure (always loaded; not user-toggleable) ──────────
    BundledMod {
        display_name: "Fabric API",
        category: "library",
        version: "0.154.0",
        mod_id: "fabric-api",
        library_prefix: "maven.modrinth:fabric-api:",
        default_on: true,
        toggleable: false,
    },
    BundledMod {
        display_name: "Fabric Language Kotlin",
        category: "library",
        version: "1.13.12",
        mod_id: "fabric-language-kotlin",
        library_prefix: "maven.modrinth:fabric-language-kotlin:",
        default_on: true,
        toggleable: false,
    },
    BundledMod {
        display_name: "YetAnotherConfigLib",
        category: "library",
        version: "3.9.5",
        mod_id: "yet_another_config_lib_v3",
        library_prefix: "maven.modrinth:yacl:",
        default_on: true,
        toggleable: false,
    },
    BundledMod {
        display_name: "Text Placeholder API",
        category: "library",
        version: "3.1.0",
        mod_id: "placeholder-api",
        library_prefix: "maven.modrinth:placeholder-api:",
        default_on: true,
        toggleable: false,
    },
    BundledMod {
        display_name: "Cloth Config",
        category: "library",
        version: "26.2.155",
        mod_id: "cloth-config",
        library_prefix: "maven.modrinth:cloth-config:",
        default_on: true,
        toggleable: false,
    },
    // ── Performance ──────────────────────────────────────────────────
    BundledMod {
        // 0.9.0, not 0.9.1-beta: the 26.2 bundle pins the launch-day
        // stable trio (Sodium 0.9.0 + Iris 1.11.1 + RSO 2.0.5) — the
        // 0.9.1 beta line declares it breaks Iris <= 1.11.1.
        display_name: "Sodium",
        category: "performance",
        version: "0.9.0",
        mod_id: "sodium",
        library_prefix: "maven.modrinth:sodium:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Lithium",
        category: "performance",
        version: "0.25.1",
        mod_id: "lithium",
        library_prefix: "maven.modrinth:lithium:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "ImmediatelyFast",
        category: "performance",
        version: "1.16.1",
        mod_id: "immediatelyfast",
        library_prefix: "maven.modrinth:immediatelyfast:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "FerriteCore",
        category: "performance",
        version: "9.0.0",
        mod_id: "ferritecore",
        library_prefix: "maven.modrinth:ferrite-core:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Entity Culling",
        category: "performance",
        version: "1.10.5",
        mod_id: "entityculling",
        library_prefix: "maven.modrinth:entityculling:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "More Culling",
        category: "performance",
        version: "1.8.0",
        mod_id: "moreculling",
        library_prefix: "maven.modrinth:moreculling:",
        default_on: true,
        toggleable: true,
    },
    // ── Visuals ──────────────────────────────────────────────────────
    BundledMod {
        display_name: "Iris Shaders",
        category: "visuals",
        version: "1.11.1",
        mod_id: "iris",
        library_prefix: "maven.modrinth:iris:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "LambDynamicLights",
        category: "visuals",
        version: "4.12.2",
        mod_id: "lambdynlights",
        library_prefix: "maven.modrinth:lambdynamiclights:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Continuity",
        category: "visuals",
        version: "3.0.1",
        mod_id: "continuity",
        library_prefix: "maven.modrinth:continuity:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Distant Horizons",
        category: "visuals",
        version: "3.1.2",
        mod_id: "distanthorizons",
        library_prefix: "maven.modrinth:distanthorizons:",
        // Heavy: 30 MB jar, LOD-renders distant chunks, opinionated.
        // Default-off so the user opts in rather than out.
        default_on: false,
        toggleable: true,
    },
    // ── Utility ──────────────────────────────────────────────────────
    BundledMod {
        display_name: "Mod Menu",
        category: "utility",
        version: "20.0.0",
        mod_id: "modmenu",
        library_prefix: "maven.modrinth:modmenu:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Reese's Sodium Options",
        category: "utility",
        version: "2.0.5",
        mod_id: "reeses-sodium-options",
        library_prefix: "maven.modrinth:reeses-sodium-options:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        // No MC 26.2 build upstream as of 2026-07 — present in the 26.1
        // manifest only. On manifest lines that omit it,
        // `missing_bundled_ids` auto-disables it at launch.
        display_name: "BetterF3",
        category: "utility",
        version: "18.0.2",
        mod_id: "betterf3",
        library_prefix: "maven.modrinth:betterf3:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "AppleSkin",
        category: "utility",
        version: "3.0.10",
        mod_id: "appleskin",
        library_prefix: "maven.modrinth:appleskin:",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Zoomify",
        category: "utility",
        version: "2.16.1",
        mod_id: "zoomify",
        library_prefix: "maven.modrinth:zoomify:",
        default_on: true,
        toggleable: true,
    },
    // ── Social ───────────────────────────────────────────────────────
    BundledMod {
        display_name: "Simple Voice Chat",
        category: "social",
        version: "2.6.20",
        mod_id: "voicechat",
        library_prefix: "maven.modrinth:simple-voice-chat:",
        default_on: true,
        toggleable: true,
    },
];

/// Build the seeded `Vec<ModInfo>` for a freshly-created Ewo instance:
/// every toggleable catalog entry as a row, each with its `default_on`.
pub fn seed_instance_mods() -> Vec<ModInfo> {
    CATALOG
        .iter()
        .filter(|m| m.toggleable)
        .map(|m| ModInfo::new(m.display_name, m.version, m.category, m.default_on))
        .collect()
}

/// Merge an existing Ewo instance's mod list with the catalog, in place.
///
/// Adds rows the catalog has but the instance doesn't (defaulting to the
/// catalog's `default_on`), preserves the `on` flag for rows that exist
/// in both, and drops rows no longer in the catalog. Order matches the
/// catalog. Returns `true` if the list actually changed (caller persists).
///
/// Run on app startup for Ewo-loader instances to handle the "bundle set
/// grew" case smoothly — existing instances pick up new toggle rows
/// without the user having to recreate them.
pub fn sync_mods_with_catalog(mods: &mut Vec<ModInfo>) -> bool {
    use std::collections::HashMap;
    let existing: HashMap<&str, bool> = mods.iter().map(|m| (m.name.as_str(), m.on)).collect();
    let new_list: Vec<ModInfo> = CATALOG
        .iter()
        .filter(|m| m.toggleable)
        .map(|m| {
            let on = existing.get(m.display_name).copied().unwrap_or(m.default_on);
            ModInfo::new(m.display_name, m.version, m.category, on)
        })
        .collect();
    let changed = new_list.len() != mods.len()
        || new_list
            .iter()
            .zip(mods.iter())
            .any(|(a, b)| a.name != b.name || a.version != b.version || a.on != b.on);
    if changed {
        *mods = new_list;
    }
    changed
}

/// Resolve the disabled mod-id set for an instance from its `mods` list.
/// Returns the ids of toggleable catalog entries that are off in `mods`.
///
/// Looks up by `display_name` (the persisted form in `ModInfo`); a mod
/// the catalog doesn't recognize is silently skipped (legacy / hand-edited
/// `instances.toml` rows).
pub fn disabled_mod_ids(mods: &[ModInfo]) -> Vec<&'static str> {
    use std::collections::HashMap;
    let by_name: HashMap<&str, bool> = mods.iter().map(|m| (m.name.as_str(), m.on)).collect();
    CATALOG
        .iter()
        .filter(|m| m.toggleable)
        .filter(|m| {
            // Default to `on` if not in the instance's list — handles
            // pre-catalog instances that haven't been synced yet.
            !by_name.get(m.display_name).copied().unwrap_or(true)
        })
        .map(|m| m.mod_id)
        .collect()
}

/// Map a set of disabled mod ids to the loader-manifest library prefixes
/// whose matching libraries should be stripped from the JVM classpath.
/// Lookup is by `mod_id` across the full catalog (infrastructure entries
/// included, although in practice only toggleable ids appear in the
/// disabled list).
pub fn library_prefixes_for_disabled(disabled_ids: &[&str]) -> Vec<&'static str> {
    let set: std::collections::HashSet<&str> = disabled_ids.iter().copied().collect();
    CATALOG
        .iter()
        .filter(|m| set.contains(m.mod_id))
        .map(|m| m.library_prefix)
        .collect()
}

/// Catalog mod ids with no matching library in the merged `PerVersion`
/// library set — i.e. bundled mods this manifest line simply doesn't ship
/// (BetterF3 has no MC 26.2 build, so 26.2.json omits it). The caller
/// appends these to `-Dfabric.debug.disableModIds` so the loader-side
/// `BundledMods` verification subtracts them instead of failing loud on a
/// mod that was never on the classpath.
pub fn missing_bundled_ids<'a>(
    library_names: impl Iterator<Item = &'a str> + Clone,
) -> Vec<&'static str> {
    CATALOG
        .iter()
        .filter(|m| {
            !library_names
                .clone()
                .any(|name| name.starts_with(m.library_prefix))
        })
        .map(|m| m.mod_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_includes_only_toggleable() {
        let seeded = seed_instance_mods();
        let toggleable_count = CATALOG.iter().filter(|m| m.toggleable).count();
        assert_eq!(seeded.len(), toggleable_count);
        // Sodium is toggleable, Fabric API is not — verify both.
        assert!(seeded.iter().any(|m| m.name == "Sodium"));
        assert!(!seeded.iter().any(|m| m.name == "Fabric API"));
    }

    #[test]
    fn sync_adds_missing_and_preserves_flips() {
        // Start with only Sodium present and flipped off; sync should
        // grow to full toggleable list with Sodium kept as off.
        let mut mods = vec![ModInfo::new("Sodium", "0.9.1", "performance", false)];
        let changed = sync_mods_with_catalog(&mut mods);
        assert!(changed);
        let sodium = mods.iter().find(|m| m.name == "Sodium").expect("sodium");
        assert!(!sodium.on);
        let lithium = mods.iter().find(|m| m.name == "Lithium").expect("lithium");
        assert!(lithium.on); // catalog default
    }

    #[test]
    fn disabled_ids_resolves_to_strings() {
        // Seed, then force every default-off mod to on so the only
        // intentionally-disabled entry is the one we flip below. Keeps the
        // test robust against the catalog gaining new default-off entries
        // (like Distant Horizons).
        let mut mods = seed_instance_mods();
        for m in mods.iter_mut() {
            m.on = true;
        }
        // Flip iris off.
        for m in mods.iter_mut() {
            if m.name == "Iris Shaders" {
                m.on = false;
            }
        }
        let disabled = disabled_mod_ids(&mods);
        assert_eq!(disabled, vec!["iris"]);

        let libs = library_prefixes_for_disabled(&disabled);
        assert_eq!(libs, vec!["maven.modrinth:iris:"]);
    }

    #[test]
    fn empty_instance_mods_treats_all_as_default_on() {
        // Pre-catalog Ewo instance with empty mods → no mods disabled
        // (everything's default_on=true).
        let disabled = disabled_mod_ids(&[]);
        assert!(disabled.is_empty());
    }

    #[test]
    fn prefix_matching_is_version_agnostic() {
        // The same prefix must match both the 26.1 and 26.2 manifest pins.
        let iris = CATALOG.iter().find(|m| m.mod_id == "iris").expect("iris");
        assert!("maven.modrinth:iris:1.10.9+26.1-fabric".starts_with(iris.library_prefix));
        assert!("maven.modrinth:iris:1.11.1+26.2-fabric".starts_with(iris.library_prefix));
    }

    #[test]
    fn missing_bundled_ids_flags_manifest_gaps() {
        // A merged library set shaped like the 26.2 manifest (everything
        // except betterf3) → exactly betterf3 reported missing.
        let names: Vec<String> = CATALOG
            .iter()
            .filter(|m| m.mod_id != "betterf3")
            .map(|m| format!("{}some-26.2-version", m.library_prefix))
            .collect();
        let missing = missing_bundled_ids(names.iter().map(|s| s.as_str()));
        assert_eq!(missing, vec!["betterf3"]);

        // Full set → nothing missing.
        let all: Vec<String> = CATALOG
            .iter()
            .map(|m| format!("{}v", m.library_prefix))
            .collect();
        assert!(missing_bundled_ids(all.iter().map(|s| s.as_str())).is_empty());
    }
}
