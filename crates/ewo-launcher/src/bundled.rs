//! Canonical catalog of mods bundled by EwoLoader.
//!
//! This is the launcher-side mirror of EwoLoader's `BundledMods.BUNDLED_MODS`
//! list — same set of mod ids, but extended with display metadata + the
//! per-mod loader-manifest library name so the launcher can:
//!
//! 1. Seed a new Ewo instance's `Instance.mods` with the user-toggleable
//!    subset, so the Instances UI shows real toggle rows immediately.
//! 2. At launch time, map a disabled mod id back to the loader-manifest
//!    library name to strip it from the JVM classpath, and feed the same
//!    id list to the loader via `-Dfabric.debug.disableModIds=…` so the
//!    `BundledMods` verification doesn't fire on the intentionally-absent
//!    mod.
//!
//! Adding a new bundled mod is a three-place change: this catalog, the
//! EwoLoader manifest's `libraries[]` array, and `BundledMods.BUNDLED_MODS`
//! on the loader side. The three lists must agree on every mod id; the
//! verification in `FabricLoaderImpl.setup()` is the safety net that fails
//! loud when they drift.

use ewo_render::screens::instances::ModInfo;

/// One bundled-mod row.
///
/// `library_name` matches the EwoLoader manifest's `libraries[].name`
/// coordinate (the Maven-ish `group:artifact:version` string). The
/// launcher walks the merged `PerVersion.libraries` looking for entries
/// whose name equals this one to strip them from the classpath when the
/// mod is disabled.
pub struct BundledMod {
    pub display_name: &'static str,
    pub category: &'static str,
    /// Display version shown in the UI mod row. Mirrors the version in
    /// the loader manifest's library coordinate.
    pub version: &'static str,
    /// The mod's `fabric.mod.json` id — this is what `BundledMods.BUNDLED_MODS`
    /// on the loader side asserts at startup, and what
    /// `fabric.debug.disableModIds` expects in its comma-separated list.
    pub mod_id: &'static str,
    /// Coordinate matching the loader manifest's `libraries[].name` so
    /// the classpath-strip step can find this row by mod-id and remove
    /// the matching library.
    pub library_name: &'static str,
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
        version: "0.148.2",
        mod_id: "fabric-api",
        library_name: "maven.modrinth:fabric-api:0.148.2+26.1.2",
        default_on: true,
        toggleable: false,
    },
    BundledMod {
        display_name: "Fabric Language Kotlin",
        category: "library",
        version: "1.13.11",
        mod_id: "fabric-language-kotlin",
        library_name: "maven.modrinth:fabric-language-kotlin:1.13.11+kotlin.2.3.21",
        default_on: true,
        toggleable: false,
    },
    BundledMod {
        display_name: "YetAnotherConfigLib",
        category: "library",
        version: "3.9.3",
        mod_id: "yet_another_config_lib_v3",
        library_name: "maven.modrinth:yacl:3.9.3+26.1-fabric",
        default_on: true,
        toggleable: false,
    },
    BundledMod {
        display_name: "Text Placeholder API",
        category: "library",
        version: "3.0.0",
        mod_id: "placeholder-api",
        library_name: "maven.modrinth:placeholder-api:3.0.0+26.1",
        default_on: true,
        toggleable: false,
    },
    BundledMod {
        display_name: "Cloth Config",
        category: "library",
        version: "26.1.154",
        mod_id: "cloth-config",
        library_name: "maven.modrinth:cloth-config:26.1.154+fabric",
        default_on: true,
        toggleable: false,
    },
    // ── Performance ──────────────────────────────────────────────────
    BundledMod {
        display_name: "Sodium",
        category: "performance",
        version: "0.8.9",
        mod_id: "sodium",
        library_name: "maven.modrinth:sodium:mc26.1.1-0.8.9-fabric",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Lithium",
        category: "performance",
        version: "0.24.2",
        mod_id: "lithium",
        library_name: "maven.modrinth:lithium:mc26.1.2-0.24.2-fabric",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "ImmediatelyFast",
        category: "performance",
        version: "1.15.2",
        mod_id: "immediatelyfast",
        library_name: "maven.modrinth:immediatelyfast:1.15.2+26.1.2-fabric",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "FerriteCore",
        category: "performance",
        version: "9.0.0",
        mod_id: "ferritecore",
        library_name: "maven.modrinth:ferrite-core:9.0.0-fabric",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Entity Culling",
        category: "performance",
        version: "1.10.2",
        mod_id: "entityculling",
        library_name: "maven.modrinth:entityculling:1.10.2",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "More Culling",
        category: "performance",
        version: "1.7.0",
        mod_id: "moreculling",
        library_name: "maven.modrinth:moreculling:1.7.0",
        default_on: true,
        toggleable: true,
    },
    // ── Visuals ──────────────────────────────────────────────────────
    BundledMod {
        display_name: "Iris Shaders",
        category: "visuals",
        version: "1.10.9",
        mod_id: "iris",
        library_name: "maven.modrinth:iris:1.10.9+26.1-fabric",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "LambDynamicLights",
        category: "visuals",
        version: "4.10.2",
        mod_id: "lambdynlights",
        library_name: "maven.modrinth:lambdynamiclights:4.10.2+26.1.2",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Continuity",
        category: "visuals",
        version: "3.0.1",
        mod_id: "continuity",
        library_name: "maven.modrinth:continuity:3.0.1-beta.2+26.1",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Distant Horizons",
        category: "visuals",
        version: "3.0.3",
        mod_id: "distanthorizons",
        library_name: "maven.modrinth:distanthorizons:3.0.3-b-26.1.2",
        // Heavy: 30 MB jar, LOD-renders distant chunks, opinionated.
        // Default-off so the user opts in rather than out.
        default_on: false,
        toggleable: true,
    },
    // ── Utility ──────────────────────────────────────────────────────
    BundledMod {
        display_name: "Mod Menu",
        category: "utility",
        version: "18.0.0",
        mod_id: "modmenu",
        library_name: "maven.modrinth:modmenu:18.0.0-beta.1",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Reese's Sodium Options",
        category: "utility",
        version: "2.0.5",
        mod_id: "reeses-sodium-options",
        library_name: "maven.modrinth:reeses-sodium-options:mc26.1.1-2.0.5+fabric",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "BetterF3",
        category: "utility",
        version: "18.0.2",
        mod_id: "betterf3",
        library_name: "maven.modrinth:betterf3:18.0.2",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "AppleSkin",
        category: "utility",
        version: "3.0.9",
        mod_id: "appleskin",
        library_name: "maven.modrinth:appleskin:3.0.9+mc26.1",
        default_on: true,
        toggleable: true,
    },
    BundledMod {
        display_name: "Zoomify",
        category: "utility",
        version: "2.16.0",
        mod_id: "zoomify",
        library_name: "maven.modrinth:zoomify:2.16.0+26.1",
        default_on: true,
        toggleable: true,
    },
    // ── Social ───────────────────────────────────────────────────────
    BundledMod {
        display_name: "Simple Voice Chat",
        category: "social",
        version: "2.6.17",
        mod_id: "voicechat",
        library_name: "maven.modrinth:simple-voice-chat:fabric-2.6.17+26.1.2",
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

/// Map a set of disabled mod ids to the loader-manifest library names
/// that should be stripped from the JVM classpath. Lookup is by `mod_id`
/// across the full catalog (infrastructure entries included, although in
/// practice only toggleable ids appear in the disabled list).
pub fn library_names_for_disabled(disabled_ids: &[&str]) -> Vec<&'static str> {
    let set: std::collections::HashSet<&str> = disabled_ids.iter().copied().collect();
    CATALOG
        .iter()
        .filter(|m| set.contains(m.mod_id))
        .map(|m| m.library_name)
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
        let mut mods = vec![ModInfo::new("Sodium", "0.8.9", "performance", false)];
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

        let libs = library_names_for_disabled(&disabled);
        assert_eq!(libs, vec!["maven.modrinth:iris:1.10.9+26.1-fabric"]);
    }

    #[test]
    fn empty_instance_mods_treats_all_as_default_on() {
        // Pre-catalog Ewo instance with empty mods → no mods disabled
        // (everything's default_on=true).
        let disabled = disabled_mod_ids(&[]);
        assert!(disabled.is_empty());
    }
}
