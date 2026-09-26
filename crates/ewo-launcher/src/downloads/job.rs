//! `DownloadJob` — runs the whole "make this version ready to launch"
//! sequence on a worker thread, reporting progress via `mpsc`.
//!
//! Stages:
//!   1. **PerVersion** — fetch the per-version manifest (cached).
//!   2. **Client** — download client.jar, sha1-verify.
//!   3. **Libraries** — download every applicable library + native, sha1-verify.
//!   4. **AssetIndex** — fetch the asset index JSON, sha1-verify.
//!   5. **Assets** — download every asset blob (parallel-friendly but
//!      currently sequential — Mojang's CDN is fast enough that one
//!      connection saturates a typical home line).
//!
//! For Phase B we report bytes-downloaded against bytes-expected so the
//! launching screen's progress bar can drive a real measurement (Phase
//! C wires it in). Errors fail-fast at the offending stage; partial
//! downloads stay on disk so a retry resumes from where it left off
//! (sha1-verify against existing files; skip if good).

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use sha1::{Digest, Sha1};

use crate::loaders::{self, LoaderSpec};
use crate::versions::manifest::ManifestEntry;
use crate::versions::per_version::PerVersion;
use crate::versions::per_version_fetch;

use super::asset_index::{asset_url, AssetIndex};
use super::paths;
use super::rules;

/// Stage labels surfaced to the UI for the launching screen's italic
/// status line.
#[derive(Debug, Clone, Copy)]
pub enum Stage {
    PerVersion,
    /// Loader manifest fetch + merge. Only emitted when the job was
    /// configured with a `LoaderSpec`. Sequenced between `PerVersion` and
    /// `Client` so the byte total used for the progress bar can include
    /// every loader-added library.
    LoaderManifest,
    Client,
    Libraries,
    AssetIndex,
    Assets,
    Done,
}

/// Events emitted to the UI thread.
#[derive(Debug)]
pub enum JobEvent {
    /// Stage transition. UI should swap the status label via `Stage::label`.
    StageStart(Stage),
    /// Progress update. `total` may be `None` when we don't yet know
    /// (e.g. before we've enumerated the asset index).
    Progress { downloaded: u64, total: Option<u64> },
    /// Job finished successfully — version is ready to launch.
    Done,
    /// Job aborted with an error.
    Failed(String),
}

pub struct JobConfig {
    /// Master manifest entry — used both to find the per-version URL and
    /// to sha1-verify the per-version body.
    pub entry: ManifestEntry,
    /// Optional loader to layer on top of vanilla. When set, the job
    /// fetches the loader manifest after the vanilla per-version manifest,
    /// merges them in-memory, and downloads the *merged* library set so
    /// loader libraries (EwoLoader fat jar + bundled mods) appear on the
    /// progress bar instead of being hot-downloaded synchronously at
    /// launch time.
    pub loader: Option<LoaderSpec>,
}

/// Spawn the job on a fresh worker thread and return immediately.
/// Caller polls the receiver for `JobEvent`s.
pub fn spawn(
    config: JobConfig,
    tx: Sender<JobEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("ewo-dl-{}", config.entry.id))
        .spawn(move || run_job(config, tx))
        .expect("spawn download thread")
}

fn run_job(config: JobConfig, tx: Sender<JobEvent>) {
    let entry = config.entry;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .user_agent("EwoClient/0.1 (+https://github.com/lewlone/ewoclient)")
        .build();

    // Stage 1 — per-version manifest.
    let _ = tx.send(JobEvent::StageStart(Stage::PerVersion));
    let vanilla_pv = match per_version_fetch::get_or_fetch(&entry) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(JobEvent::Failed(format!("per-version fetch: {}", e)));
            return;
        }
    };

    // Stage 1b — loader manifest fetch + merge (only when the instance has
    // a loader configured). On fetch failure we proceed with vanilla:
    // matches `try_real_launch`'s "non-fatal loader miss" policy so a flaky
    // dev manifest doesn't block the whole download. The progress bar then
    // omits loader libs, but `ensure_libraries` (called from launch) will
    // pick them up if the manifest comes back at launch time.
    let pv = match &config.loader {
        Some(spec) => {
            let _ = tx.send(JobEvent::StageStart(Stage::LoaderManifest));
            match loaders::get_or_fetch(&spec.id, &spec.url) {
                Ok(loader_manifest) => {
                    log::info!(
                        "downloads: merging loader \"{}\" manifest into {}",
                        loader_manifest.id, vanilla_pv.id
                    );
                    loaders::merge(&vanilla_pv, &loader_manifest)
                }
                Err(e) => {
                    log::warn!(
                        "downloads: loader \"{}\" fetch failed ({}) — continuing with vanilla",
                        spec.id, e
                    );
                    vanilla_pv
                }
            }
        }
        None => vanilla_pv,
    };

    // Compute total size up front so the UI bar has a denominator. The
    // merged PV's library list already includes loader-added libraries,
    // so they contribute to the byte total + show up on the progress bar.
    // `pv.asset_index.total_size` covers all assets (Mojang precomputes
    // this); client + libs are the per-version manifest's own numbers.
    let lib_bytes: u64 = pv
        .libraries
        .iter()
        .filter(|l| rules::rules_pass(&l.rules))
        .filter_map(|l| {
            let mut total = l.downloads.artifact.as_ref().map(|a| a.size).unwrap_or(0);
            // Add native classifier if present.
            if let Some(natives_key) = native_classifier_for(l) {
                if let Some(c) = l.downloads.classifiers.get(&natives_key) {
                    total += c.size;
                }
            }
            Some(total)
        })
        .sum();
    let total_bytes =
        pv.downloads.client.size + lib_bytes + pv.asset_index.total_size + pv.asset_index.size;
    let mut downloaded: u64 = 0;
    let _ = tx.send(JobEvent::Progress {
        downloaded,
        total: Some(total_bytes),
    });

    // Stage 2 — client jar.
    let _ = tx.send(JobEvent::StageStart(Stage::Client));
    let client_path = match paths::client_jar(&pv.id) {
        Some(p) => p,
        None => {
            let _ = tx.send(JobEvent::Failed("config dir unresolvable".into()));
            return;
        }
    };
    if let Err(e) = ensure_file(
        &agent,
        &pv.downloads.client.url,
        &pv.downloads.client.sha1,
        pv.downloads.client.size,
        &client_path,
    ) {
        let _ = tx.send(JobEvent::Failed(format!("client.jar: {}", e)));
        return;
    }
    downloaded += pv.downloads.client.size;
    let _ = tx.send(JobEvent::Progress {
        downloaded,
        total: Some(total_bytes),
    });

    // Stage 3 — libraries (+ native classifiers).
    let _ = tx.send(JobEvent::StageStart(Stage::Libraries));
    for lib in &pv.libraries {
        if !rules::rules_pass(&lib.rules) {
            continue;
        }
        if let Some(art) = &lib.downloads.artifact {
            let path = match paths::library_path(&art.path) {
                Some(p) => p,
                None => {
                    let _ = tx.send(JobEvent::Failed(format!("library {}: unsafe or unresolvable path {:?}", lib.name, art.path)));
                    return;
                }
            };
            if let Err(e) = ensure_file(&agent, &art.url, &art.sha1, art.size, &path) {
                let _ = tx.send(JobEvent::Failed(format!(
                    "library {}: {}",
                    lib.name, e
                )));
                return;
            }
            downloaded += art.size;
            let _ = tx.send(JobEvent::Progress {
                downloaded,
                total: Some(total_bytes),
            });
        }
        // Native classifier (if any) — same flow.
        if let Some(natives_key) = native_classifier_for(lib) {
            if let Some(art) = lib.downloads.classifiers.get(&natives_key) {
                let path = match paths::library_path(&art.path) {
                    Some(p) => p,
                    None => {
                        let _ = tx.send(JobEvent::Failed(format!("native {}: unsafe or unresolvable path {:?}", lib.name, art.path)));
                        return;
                    }
                };
                if let Err(e) = ensure_file(&agent, &art.url, &art.sha1, art.size, &path) {
                    let _ = tx.send(JobEvent::Failed(format!(
                        "native {} ({}): {}",
                        lib.name, natives_key, e
                    )));
                    return;
                }
                downloaded += art.size;
                let _ = tx.send(JobEvent::Progress {
                    downloaded,
                    total: Some(total_bytes),
                });
            }
        }
    }

    // Stage 4 — asset index.
    let _ = tx.send(JobEvent::StageStart(Stage::AssetIndex));
    let asset_index_path = match paths::asset_index_path(&pv.asset_index.id) {
        Some(p) => p,
        None => {
            let _ = tx.send(JobEvent::Failed("config dir unresolvable".into()));
            return;
        }
    };
    if let Err(e) = ensure_file(
        &agent,
        &pv.asset_index.url,
        &pv.asset_index.sha1,
        pv.asset_index.size,
        &asset_index_path,
    ) {
        let _ = tx.send(JobEvent::Failed(format!("asset index: {}", e)));
        return;
    }
    downloaded += pv.asset_index.size;
    let _ = tx.send(JobEvent::Progress {
        downloaded,
        total: Some(total_bytes),
    });

    // Parse the asset index into the object map.
    let body = match fs::read_to_string(&asset_index_path) {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(JobEvent::Failed(format!("asset index read: {}", e)));
            return;
        }
    };
    let index: AssetIndex = match serde_json::from_str(&body) {
        Ok(i) => i,
        Err(e) => {
            let _ = tx.send(JobEvent::Failed(format!("asset index parse: {}", e)));
            return;
        }
    };

    // Stage 5 — asset blobs.
    let _ = tx.send(JobEvent::StageStart(Stage::Assets));
    for (name, obj) in &index.objects {
        // The hash becomes a path + URL; anything but a sha1 is malformed.
        let (Some(url), Some(path)) = (asset_url(&obj.hash), paths::asset_object_path(&obj.hash))
        else {
            let _ = tx.send(JobEvent::Failed(format!(
                "asset {}: malformed hash {:?}",
                name, obj.hash
            )));
            return;
        };
        if let Err(e) = ensure_file(&agent, &url, &obj.hash, obj.size, &path) {
            let _ = tx.send(JobEvent::Failed(format!(
                "asset {}: {}",
                &obj.hash[..8],
                e
            )));
            return;
        }
        downloaded += obj.size;
        let _ = tx.send(JobEvent::Progress {
            downloaded,
            total: Some(total_bytes),
        });
    }

    let _ = tx.send(JobEvent::StageStart(Stage::Done));
    let _ = tx.send(JobEvent::Done);
}

/// Pick the native classifier key for this library on the host OS, if
/// one applies. Modern manifests (1.13+) usually use rule-based filtering
/// at the library level instead, so the `natives` map is empty.
/// Legacy manifests use the `natives` map to look up the classifier.
fn native_classifier_for(lib: &crate::versions::per_version::Library) -> Option<String> {
    // Legacy form: lib.natives[host_os] = "natives-<os>"
    let host = std::env::consts::OS;
    let key = match host {
        "windows" => "windows",
        "macos" => "osx",
        "linux" => "linux",
        other => other,
    };
    if let Some(classifier) = lib.natives.get(key) {
        // Some entries use template strings like `natives-windows-${arch}`.
        // Our targets don't, so we ignore the templating for now.
        return Some(classifier.clone());
    }
    // Modern form: classifier directly on `downloads.classifiers` with
    // the `natives-<os>` key. Return that if present.
    let modern_key = rules::host_natives_classifier().to_string();
    if lib.downloads.classifiers.contains_key(&modern_key) {
        return Some(modern_key);
    }
    None
}

/// Download every library + native in `pv` that isn't already on disk.
///
/// Called by the launch path after Phase D's loader merge so loader-added
/// libraries (which weren't in the vanilla `PerVersion` Phase B saw at
/// instance-setup time) get pulled before the JVM spawns. Idempotent +
/// cheap when everything's already present — `ensure_file` skips files whose
/// sha1 already verifies (hashed once per session, then size+mtime cached).
pub fn ensure_libraries(pv: &PerVersion) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .user_agent("EwoClient/0.1 (+https://github.com/lewlone/ewoclient)")
        .build();
    for lib in &pv.libraries {
        if !rules::rules_pass(&lib.rules) {
            continue;
        }
        if let Some(art) = &lib.downloads.artifact {
            let path = paths::library_path(&art.path)
                .ok_or_else(|| format!("library {}: path unresolvable", lib.name))?;
            ensure_file(&agent, &art.url, &art.sha1, art.size, &path)
                .map_err(|e| format!("library {}: {}", lib.name, e))?;
        }
        if let Some(natives_key) = native_classifier_for(lib) {
            if let Some(art) = lib.downloads.classifiers.get(&natives_key) {
                let path = paths::library_path(&art.path)
                    .ok_or_else(|| format!("native {}: path unresolvable", lib.name))?;
                ensure_file(&agent, &art.url, &art.sha1, art.size, &path)
                    .map_err(|e| format!("native {} ({}): {}", lib.name, natives_key, e))?;
            }
        }
    }
    Ok(())
}

/// Files already sha1-verified this session, keyed by path, with the size +
/// mtime they had when verified. Re-verifying every existing file on every
/// launch costs ~1 s per GB (the full 26.x asset set is several hundred MB),
/// so a file is hashed once per launcher session and trusted afterwards only
/// while its size and mtime are unchanged.
static VERIFIED: Mutex<Option<HashMap<PathBuf, (u64, SystemTime, String)>>> = Mutex::new(None);

fn verified_cached(dest: &Path, meta: &fs::Metadata, sha1: &str) -> bool {
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    let guard = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .and_then(|m| m.get(dest))
        .is_some_and(|(size, t, h)| *size == meta.len() && *t == mtime && h == sha1)
}

fn remember_verified(dest: &Path, sha1: &str) {
    let Ok(meta) = fs::metadata(dest) else {
        return;
    };
    let Ok(mtime) = meta.modified() else {
        return;
    };
    let mut guard = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(HashMap::new)
        .insert(dest.to_path_buf(), (meta.len(), mtime, sha1.to_string()));
}

fn sha1_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// `<dest>.part` — downloads land here and are renamed into place only once
/// verified, so a crash or a bad body never leaves a trusted-looking file.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Whether an existing `dest` already has the expected content.
fn existing_is_valid(dest: &Path, sha1: &str, expected_size: u64) -> bool {
    let Ok(meta) = fs::metadata(dest) else {
        return false;
    };
    if !meta.is_file() || (expected_size != 0 && meta.len() != expected_size) {
        return false;
    }
    if verified_cached(dest, &meta, sha1) {
        return true;
    }
    match sha1_file(dest) {
        Ok(h) if h == sha1 => {
            remember_verified(dest, sha1);
            true
        }
        _ => false,
    }
}

/// Download `url` to `dest`, verify the file's sha1 against `expected_sha1`,
/// and confirm size matches. Skips the network if the file exists with
/// a matching hash already. HTTP(S) downloads must carry a sha1.
fn ensure_file(
    agent: &ureq::Agent,
    url: &str,
    expected_sha1: &str,
    expected_size: u64,
    dest: &Path,
) -> Result<(), String> {
    // `file://` URLs point at the user's local file. Skip the existence
    // shortcut + sha1/size verification entirely: the file is whatever the
    // user has on disk *right now*. Caching would freeze the cached copy
    // against the source's edits; sha1-pinning would force the user to
    // bump the manifest on every rebuild. Always re-copy + trust the file.
    let local_path = crate::util::file_url_to_path(url);
    let expected_sha1 = expected_sha1.to_ascii_lowercase();
    if local_path.is_none() {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("unsupported URL scheme: {}", url));
        }
        if !paths::is_sha1_hex(&expected_sha1) {
            return Err(format!("no sha1 for {} — refusing an unverifiable download", url));
        }
        if existing_is_valid(dest, &expected_sha1, expected_size) {
            return Ok(());
        }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    let mut reader: Box<dyn Read> = if let Some(ref p) = local_path {
        let f = fs::File::open(p)
            .map_err(|e| format!("open {}: {}", p.display(), e))?;
        Box::new(f)
    } else {
        let mut req = agent.get(url);
        for (k, v) in github_auth_headers_for(url) {
            req = req.set(k, &v);
        }
        let resp = req
            .call()
            .map_err(|e| format!("GET {}: {}", url, e))?;
        Box::new(resp.into_reader())
    };
    let part = part_path(dest);
    let result = (|| {
        let mut file =
            fs::File::create(&part).map_err(|e| format!("create {}: {}", part.display(), e))?;
        let mut hasher = Sha1::new();
        let mut buf = [0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("read body: {}", e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])
                .map_err(|e| format!("write {}: {}", part.display(), e))?;
            written += n as u64;
        }
        drop(file);
        if local_path.is_none() {
            let got_hex = hex(&hasher.finalize());
            if got_hex != expected_sha1 {
                return Err(format!(
                    "sha1 mismatch (got {}, expected {})",
                    got_hex, expected_sha1
                ));
            }
            if expected_size != 0 && written != expected_size {
                return Err(format!(
                    "size mismatch (got {}, expected {})",
                    written, expected_size
                ));
            }
        }
        fs::rename(&part, dest).map_err(|e| format!("rename {}: {}", dest.display(), e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    } else if local_path.is_none() {
        remember_verified(dest, &expected_sha1);
    }
    result
}

/// Build the HTTP headers needed to download a private-repo GitHub
/// release asset. Returns an empty list for any URL that's not the
/// `api.github.com/repos/<owner>/<repo>/releases/assets/<id>` shape —
/// we never send the token to Modrinth, Mojang, or other hosts.
///
/// The token comes from `EWO_LOADER_TOKEN` (kept out of source + out of
/// settings.toml; user sets it in their shell profile). Missing env var
/// returns empty — the request goes through unauthenticated and GitHub
/// responds with 404, which the existing `GET {}: {}` error path
/// surfaces clearly.
fn github_auth_headers_for(url: &str) -> Vec<(&'static str, String)> {
    if !is_github_release_asset_url(url) {
        return Vec::new();
    }
    let Ok(token) = std::env::var("EWO_LOADER_TOKEN") else {
        return Vec::new();
    };
    if token.is_empty() {
        return Vec::new();
    }
    vec![
        ("Authorization", format!("Bearer {}", token)),
        ("Accept", "application/octet-stream".to_string()),
        // GitHub API docs recommend pinning the API version. Drift-proofs
        // against silent breaking changes on their side.
        ("X-GitHub-Api-Version", "2022-11-28".to_string()),
    ]
}

fn is_github_release_asset_url(url: &str) -> bool {
    // Match `https://api.github.com/repos/<owner>/<repo>/releases/assets/<id>`.
    // We never send auth to the regular `github.com/<owner>/<repo>/releases/download/...`
    // path because that's a redirect to a signed CDN URL — the request token
    // would just confuse GitHub's auth check and isn't strictly needed there
    // for private repos either (it falls back to session cookies, which we
    // don't have).
    url.starts_with("https://api.github.com/repos/")
        && url.contains("/releases/assets/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new().timeout(Duration::from_secs(1)).build()
    }

    #[test]
    fn http_download_without_sha1_is_refused_before_any_request() {
        let dest = std::env::temp_dir().join("ewo-nosha-test.jar");
        let err = ensure_file(&agent(), "https://example.invalid/x.jar", "", 0, &dest).unwrap_err();
        assert!(err.contains("no sha1"), "{err}");
        let err = ensure_file(&agent(), "ftp://example.invalid/x.jar", "", 0, &dest).unwrap_err();
        assert!(err.contains("unsupported"), "{err}");
    }

    #[test]
    fn existing_file_must_match_its_sha1_not_just_its_size() {
        let dir = std::env::temp_dir().join(format!("ewo-verify-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("lib.jar");
        fs::write(&dest, b"hello").unwrap();
        // sha1("hello")
        let good = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";
        let bad = "0000000000000000000000000000000000000000";
        assert!(!existing_is_valid(&dest, bad, 5), "same size, wrong hash must not pass");
        assert!(existing_is_valid(&dest, good, 5));
        assert!(!existing_is_valid(&dest, good, 6), "size mismatch");
        // A good existing file short-circuits without touching the network.
        ensure_file(&agent(), "https://example.invalid/lib.jar", good, 5, &dest).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_url_copies_through_part_file() {
        let dir = std::env::temp_dir().join(format!("ewo-fileurl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.jar");
        fs::write(&src, b"jar").unwrap();
        let url = if cfg!(windows) {
            format!("file:///{}", src.to_string_lossy().replace('\\', "/"))
        } else {
            format!("file://{}", src.to_string_lossy())
        };
        let dest = dir.join("out").join("dest.jar");
        ensure_file(&agent(), &url, "", 0, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"jar");
        assert!(!part_path(&dest).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // The token-set and token-absent cases share one test on purpose:
    // `EWO_LOADER_TOKEN` is a process-global env var and Rust runs tests
    // in parallel threads, so splitting them lets one test's `remove_var`
    // race the other's `set_var`. One sequential test can't interleave.
    #[test]
    fn github_auth_headers_gate_on_url_and_token() {
        let asset = "https://api.github.com/repos/lewlone/ewo-loader/releases/assets/12345";

        // With a token set, a release-asset API URL gets auth headers.
        std::env::set_var("EWO_LOADER_TOKEN", "ghp_test_token");
        let headers = github_auth_headers_for(asset);
        assert!(!headers.is_empty(), "GitHub asset URL should get auth headers");
        assert!(headers.iter().any(|(k, _)| *k == "Authorization"));
        assert!(headers.iter().any(|(k, v)| *k == "Accept" && v == "application/octet-stream"));

        // ...but Modrinth + Mojang + the non-API GitHub browser-download
        // URL must NOT, even with the token set.
        assert!(github_auth_headers_for("https://cdn.modrinth.com/data/AANobbMI/...").is_empty());
        assert!(github_auth_headers_for("https://launcher.mojang.com/...").is_empty());
        assert!(github_auth_headers_for(
            "https://github.com/lewlone/ewo-loader/releases/download/v0.19.2/ewo-loader-0.19.2-fat.jar"
        )
        .is_empty());

        // With no token, even the release-asset API URL gets nothing.
        std::env::remove_var("EWO_LOADER_TOKEN");
        assert!(github_auth_headers_for(asset).is_empty());
    }
}
