//! The blocking half of a real JVM launch, run on a worker thread so the
//! UI loop never waits on the network or the disk: per-version manifest,
//! loader manifest + merge, library downloads, mod stripping and natives
//! extraction. The UI thread finishes the launch (JRE pick, plan, spawn)
//! when the result arrives.

use std::sync::mpsc::{self, Receiver};

use ewo_render::screens::instances::{InstanceLoader, ModInfo};

use crate::versions::manifest::ManifestEntry;
use crate::versions::per_version::PerVersion;
use crate::{bundled, downloads, loaders, versions};

/// Everything the UI thread needs to finish a launch.
pub struct Prepared {
    /// Merged (vanilla + loader) profile with disabled mods stripped.
    pub pv: PerVersion,
    /// Mod ids to pass as `-Dfabric.debug.disableModIds`.
    pub disabled_mod_ids: Vec<&'static str>,
}

/// Prefix of the `Err` string when an Ewo instance's loader manifest could
/// not be resolved or fetched. The UI shows these as a launch error instead
/// of falling back to the synthetic launch animation.
pub const LOADER_ERR_PREFIX: &str = "EwoLoader: ";

/// Whether a prepare error is a loader-manifest failure.
pub fn is_loader_error(msg: &str) -> bool {
    msg.starts_with(LOADER_ERR_PREFIX)
}

/// Inputs copied out of the instance for the worker.
pub struct PrepareJob {
    pub entry: ManifestEntry,
    pub version_id: String,
    pub instance_id: String,
    pub loader: InstanceLoader,
    pub mods: Vec<ModInfo>,
}

/// Start preparing on a worker thread; the result arrives on the receiver.
pub fn spawn(job: PrepareJob) -> Receiver<Result<Prepared, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("ewo-launch-prepare".into())
        .spawn(move || {
            let _ = tx.send(run(job));
        })
        .expect("spawn launch-prepare thread");
    rx
}

fn run(job: PrepareJob) -> Result<Prepared, String> {
    let version_id = job.version_id.as_str();
    // Per-version manifest must be on disk (Phase B). If somehow it isn't,
    // refuse to launch — caller falls back.
    let vanilla_pv = versions::per_version_fetch::get_or_fetch(&job.entry)
        .map_err(|e| format!("per-version fetch failed: {}", e))?;
    // Phase D: layer the instance's loader on top of vanilla, if any.
    // An Ewo instance never degrades to a vanilla launch: if its loader
    // manifest can't be resolved or fetched the launch fails with a
    // `LOADER_ERR_PREFIX` error the launching screen shows (a silent vanilla
    // game has no loader, mods or HUD and looks like a working launch). The
    // base is resolved now, on this machine; the instance's stored value is
    // not used (see `loaders::base`).
    let mut pv = match &job.loader {
        InstanceLoader::Vanilla | InstanceLoader::Native => vanilla_pv,
        InstanceLoader::Ewo { .. } => {
            let url = loaders::base::resolve_manifest_url(version_id)
                .map_err(|e| format!("{LOADER_ERR_PREFIX}{e}"))?;
            let loader_manifest = loaders::get_or_fetch("ewo", &url)
                .map_err(|e| format!("{LOADER_ERR_PREFIX}could not load {url}: {e}"))?;
            log::info!(
                "launch: merging EwoLoader manifest \"{}\" on top of {}",
                loader_manifest.id,
                version_id
            );
            loaders::merge(&vanilla_pv, &loader_manifest)
        }
    };
    // Download any library the merge added that wasn't in the vanilla
    // `PerVersion` Phase B saw at instance-setup time. Runs against the
    // *full* merged set so disabled mods stay on disk for cheap re-enable.
    downloads::ensure_libraries(&pv).map_err(|e| format!("loader library fetch failed: {}", e))?;
    // Per-instance mod toggles: strip disabled mods from the classpath
    // (after ensure_libraries, before the plan reads pv.libraries).
    let mut disabled_mod_ids = bundled::disabled_mod_ids(&job.mods);
    if !disabled_mod_ids.is_empty() {
        // Prefix match (`maven.modrinth:iris:`) — the catalog is
        // version-agnostic across manifest lines.
        let disabled_prefixes = bundled::library_prefixes_for_disabled(&disabled_mod_ids);
        let before = pv.libraries.len();
        pv.libraries
            .retain(|l| !disabled_prefixes.iter().any(|p| l.name.starts_with(p)));
        log::info!(
            "launch: disabling {} mod(s) [{}] — stripped {} libraries from classpath",
            disabled_mod_ids.len(),
            disabled_mod_ids.join(","),
            before - pv.libraries.len()
        );
    }
    // Bundled mods this manifest line doesn't ship at all were never on the
    // classpath, but the loader's BundledMods verification still expects
    // them, so they ride the same disableModIds subtraction.
    if matches!(job.loader, InstanceLoader::Ewo { .. }) {
        let missing = bundled::missing_bundled_ids(pv.libraries.iter().map(|l| l.name.as_str()));
        for id in missing {
            if !disabled_mod_ids.contains(&id) {
                log::info!(
                    "launch: bundled mod \"{}\" absent from this manifest line — auto-disabling",
                    id
                );
                disabled_mod_ids.push(id);
            }
        }
    }
    if let Err(e) = super::extract_all(&pv, &job.instance_id) {
        // A natives file held open almost always means a zombie game
        // process (deadlocked in teardown on a previous exit). Only for
        // that error, reap *our own* recorded, windowless game processes
        // and retry once — never anything else on the machine.
        if !matches!(e, super::natives::ExtractError::InUse(_)) {
            return Err(format!("native extraction failed: {}", e));
        }
        let killed = super::reaper::reap_recorded_zombies();
        log::warn!(
            "launch: native extraction failed ({}) — reaped {} zombie game process(es), retrying",
            e,
            killed
        );
        if killed > 0 {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        super::extract_all(&pv, &job.instance_id).map_err(|e2| {
            format!(
                "native extraction still failing after reaping {} process(es): {}",
                killed, e2
            )
        })?;
    }
    Ok(Prepared {
        pv,
        disabled_mod_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_errors_are_recognised_by_prefix() {
        assert!(is_loader_error(&format!("{LOADER_ERR_PREFIX}no manifests")));
        assert!(!is_loader_error("per-version fetch failed: boom"));
    }
}
