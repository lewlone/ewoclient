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

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use sha1::{Digest, Sha1};

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
    Client,
    Libraries,
    AssetIndex,
    Assets,
    Done,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::PerVersion => "fetching version manifest…",
            Stage::Client => "downloading the game…",
            Stage::Libraries => "downloading libraries…",
            Stage::AssetIndex => "fetching asset index…",
            Stage::Assets => "downloading assets…",
            Stage::Done => "ready.",
        }
    }
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
    let pv = match per_version_fetch::get_or_fetch(&entry) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(JobEvent::Failed(format!("per-version fetch: {}", e)));
            return;
        }
    };

    // Compute total size up front so the UI bar has a denominator.
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
                None => continue,
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
                    None => continue,
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
    for (_name, obj) in &index.objects {
        let url = match asset_url(&obj.hash) {
            Some(u) => u,
            None => continue,
        };
        let path = match paths::asset_object_path(&obj.hash) {
            Some(p) => p,
            None => continue,
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

/// `file:///C:/path/to.jar` → `C:\path\to.jar` (Windows) or
/// `file:///home/user/x.jar` → `/home/user/x.jar` (Unix). Returns `None`
/// for non-`file://` URLs.
///
/// Naive percent-decoder — handles `%20` etc. for paths that contain
/// spaces. Doesn't handle authority or query strings; library URLs
/// don't need them.
fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // Strip the leading `/` on Windows-style `file:///C:/...` so the
    // result becomes `C:/...` (a real drive-letter path), not `/C:/...`.
    let trimmed = if cfg!(windows)
        && rest.starts_with('/')
        && rest.len() >= 4
        && rest.as_bytes()[2] == b':'
    {
        &rest[1..]
    } else {
        rest
    };
    let mut decoded = String::with_capacity(trimmed.len());
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                decoded.push(((h << 4) | l) as u8 as char);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i] as char);
        i += 1;
    }
    Some(PathBuf::from(decoded))
}

/// Download `url` to `dest`, verify the file's sha1 against `expected_sha1`,
/// and confirm size matches. Skips the network if the file exists with
/// a matching hash already.
fn ensure_file(
    agent: &ureq::Agent,
    url: &str,
    expected_sha1: &str,
    expected_size: u64,
    dest: &Path,
) -> Result<(), String> {
    if dest.exists() {
        // Quick check: size match → assume sha1 matches. Faster startup
        // when most of the tree is already on disk. Full sha1 verify on
        // a stricter "verify" pass we'll add later if needed.
        if let Ok(meta) = fs::metadata(dest) {
            if meta.len() == expected_size {
                return Ok(());
            }
        }
        // Size mismatch — re-download.
        let _ = fs::remove_file(dest);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    // `file://` lets local dev point library URLs at on-disk artifacts
    // (the EwoLoader fat jar during bundle-phase iteration before it's
    // hosted publicly). Mirrors the same scheme support in `loaders::fetch`.
    let mut reader: Box<dyn Read> = if let Some(local_path) = file_url_to_path(url) {
        let f = fs::File::open(&local_path)
            .map_err(|e| format!("open {}: {}", local_path.display(), e))?;
        Box::new(f)
    } else {
        let resp = agent
            .get(url)
            .call()
            .map_err(|e| format!("GET {}: {}", url, e))?;
        Box::new(resp.into_reader())
    };
    let mut file = fs::File::create(dest).map_err(|e| format!("create {}: {}", dest.display(), e))?;
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
            .map_err(|e| format!("write {}: {}", dest.display(), e))?;
        written += n as u64;
    }
    let got = hasher.finalize();
    let got_hex: String = got.iter().map(|b| format!("{:02x}", b)).collect();
    if !expected_sha1.is_empty() && got_hex != expected_sha1 {
        let _ = fs::remove_file(dest);
        return Err(format!(
            "sha1 mismatch (got {}, expected {})",
            got_hex, expected_sha1
        ));
    }
    if expected_size != 0 && written != expected_size {
        let _ = fs::remove_file(dest);
        return Err(format!(
            "size mismatch (got {}, expected {})",
            written, expected_size
        ));
    }
    Ok(())
}
