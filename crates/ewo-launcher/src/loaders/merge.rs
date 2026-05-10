//! Merge a `LoaderManifest` on top of a vanilla `PerVersion`.
//!
//! Semantics:
//!   - **`id` stays vanilla's.** This is the on-disk identifier used by
//!     `paths::client_jar(&id)` etc. — substituting the loader's
//!     synthetic id breaks classpath resolution since the client jar
//!     lives under `versions/<vanilla_id>/<vanilla_id>.jar`. The loader's
//!     own id is retained on the `LoaderManifest` itself for display.
//!   - Loader's `mainClass` wins when present.
//!   - Loader libraries come first on the classpath so loader-shipped
//!     patches override vanilla classes.
//!   - Loader JVM + game args are prepended to vanilla's.
//!   - All other vanilla fields (assets, java_version, downloads, …)
//!     pass through untouched.

use crate::loaders::manifest::LoaderManifest;
use crate::versions::per_version::{Arguments, PerVersion};

pub fn merge(vanilla: &PerVersion, loader: &LoaderManifest) -> PerVersion {
    let main_class = loader
        .main_class
        .clone()
        .unwrap_or_else(|| vanilla.main_class.clone());

    let mut libraries = Vec::with_capacity(loader.libraries.len() + vanilla.libraries.len());
    libraries.extend(loader.libraries.iter().cloned());
    libraries.extend(vanilla.libraries.iter().cloned());

    let (arguments, minecraft_arguments) = match (&loader.arguments, &vanilla.arguments) {
        (Some(loader_args), Some(vanilla_args)) => {
            let mut jvm = loader_args.jvm.clone();
            jvm.extend(vanilla_args.jvm.iter().cloned());
            let mut game = loader_args.game.clone();
            game.extend(vanilla_args.game.iter().cloned());
            (Some(Arguments { jvm, game }), vanilla.minecraft_arguments.clone())
        }
        // Loader targets a legacy vanilla (no `arguments` block). Loaders
        // typically only target 1.13+, so this is unlikely; if it happens,
        // take the loader's args verbatim and drop the legacy string.
        (Some(loader_args), None) => (Some(loader_args.clone()), None),
        (None, _) => (vanilla.arguments.clone(), vanilla.minecraft_arguments.clone()),
    };

    PerVersion {
        id: vanilla.id.clone(),
        main_class,
        asset_index: vanilla.asset_index.clone(),
        assets: vanilla.assets.clone(),
        java_version: vanilla.java_version.clone(),
        downloads: vanilla.downloads.clone(),
        libraries,
        arguments,
        minecraft_arguments,
        release_time: vanilla.release_time.clone(),
        compliance_level: vanilla.compliance_level,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::versions::per_version::{
        Arguments, AssetIndexRef, ClientDownload, Downloads, Library,
    };

    fn lib(name: &str) -> Library {
        Library {
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn vanilla_fixture() -> PerVersion {
        PerVersion {
            id: "1.21.4".to_string(),
            main_class: "net.minecraft.client.main.Main".to_string(),
            asset_index: AssetIndexRef {
                id: "17".to_string(),
                sha1: "deadbeef".to_string(),
                size: 0,
                total_size: 0,
                url: "https://example.invalid/index.json".to_string(),
            },
            assets: "17".to_string(),
            java_version: None,
            downloads: Downloads {
                client: ClientDownload {
                    sha1: "cafebabe".to_string(),
                    size: 0,
                    url: "https://example.invalid/client.jar".to_string(),
                },
                server: None,
            },
            libraries: vec![lib("a"), lib("b")],
            arguments: Some(Arguments {
                jvm: vec![serde_json::Value::String("jvm-vanilla".to_string())],
                game: vec![serde_json::Value::String("game-vanilla".to_string())],
            }),
            minecraft_arguments: None,
            release_time: "2024-12-03T10:00:00+00:00".to_string(),
            compliance_level: 1,
        }
    }

    fn loader_fixture() -> LoaderManifest {
        LoaderManifest {
            id: "fabric-loader-0.16.10-1.21.4".to_string(),
            inherits_from: "1.21.4".to_string(),
            main_class: Some("net.fabricmc.KnotClient".to_string()),
            libraries: vec![lib("loader-lib")],
            arguments: Some(Arguments {
                jvm: vec![serde_json::Value::String("jvm-loader".to_string())],
                game: vec![serde_json::Value::String("game-loader".to_string())],
            }),
            release_time: None,
        }
    }

    #[test]
    fn loader_overrides_main_class_and_prepends_libs_and_args() {
        let vanilla = vanilla_fixture();
        let loader = loader_fixture();
        let merged = merge(&vanilla, &loader);

        // `id` stays the vanilla id so client.jar / asset paths resolve
        // against the right on-disk dir.
        assert_eq!(merged.id, "1.21.4");
        assert_eq!(merged.main_class, "net.fabricmc.KnotClient");

        let lib_names: Vec<&str> = merged.libraries.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(lib_names, vec!["loader-lib", "a", "b"]);

        let args = merged.arguments.as_ref().expect("arguments preserved");
        let jvm: Vec<&str> = args.jvm.iter().map(|v| v.as_str().unwrap()).collect();
        let game: Vec<&str> = args.game.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(jvm, vec!["jvm-loader", "jvm-vanilla"]);
        assert_eq!(game, vec!["game-loader", "game-vanilla"]);

        assert_eq!(merged.asset_index.id, "17");
    }
}
