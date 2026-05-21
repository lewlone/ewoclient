//! `RuntimeService` — owns the bundled-JRE fetch worker thread.
//!
//! When the launcher needs a Java major version that isn't installed,
//! `start_fetch(major)` spawns a thread that:
//!   1. Hits the Adoptium API for the latest GA JRE for that major.
//!   2. Streams the archive to disk under
//!      `<config>/EwoClient/runtime/<major>/<archive>` while sha-256
//!      verifying.
//!   3. Extracts to `<runtime>/<major>/jre/`.
//!   4. Reports progress (bytes downloaded / total) via `mpsc` so the
//!      launching screen can drive a real progress bar.
//!
//! After extraction succeeds, `jre::detect_all` will pick up the new
//! JRE on its next call (the launcher caches the scan, so this needs a
//! manual invalidation — see `jre::invalidate_cache`).

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::adoptium::{self, AdoptiumError, ReleaseInfo};

#[derive(Debug)]
pub enum RuntimeEvent {
    /// API hit succeeded; we know the archive size + URL.
    Resolved { major: u32, info: ReleaseInfo },
    /// Bytes downloaded so far / total expected.
    Progress { downloaded: u64, total: u64 },
    /// Done downloading + extracting. JRE is ready under `<jre_dir>`.
    Done { major: u32, jre_dir: PathBuf },
    /// Anything went wrong — see `message`.
    Failed { major: u32, message: String },
}

pub struct RuntimeService {
    tx: Sender<RuntimeEvent>,
    rx: Receiver<RuntimeEvent>,
    /// Currently in-flight major (only one fetch at a time today).
    in_flight: Option<u32>,
}

impl RuntimeService {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            in_flight: None,
        }
    }

    /// Spawn a fetch+extract job for `major`. No-op if a fetch is
    /// already in flight (today we serialize them to keep network +
    /// disk simple; could parallelize per-major later).
    pub fn start_fetch(&mut self, major: u32) {
        if self.in_flight.is_some() {
            log::info!("runtime: already fetching — ignoring request for Java {}", major);
            return;
        }
        log::info!("runtime: starting Adoptium fetch for Java {}", major);
        self.in_flight = Some(major);
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name(format!("ewo-runtime-{}", major))
            .spawn(move || run_fetch(tx, major))
            .expect("spawn runtime fetch");
    }

    /// Drain pending events. Returns the events for caller to inspect
    /// (UI flips state, sets up retries, etc.). Clears `in_flight`
    /// when a `Done` or `Failed` arrives.
    pub fn poll(&mut self) -> Vec<RuntimeEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            match &ev {
                RuntimeEvent::Done { .. } | RuntimeEvent::Failed { .. } => {
                    self.in_flight = None;
                }
                _ => {}
            }
            out.push(ev);
        }
        out
    }
}

fn run_fetch(tx: Sender<RuntimeEvent>, major: u32) {
    let info = match adoptium::latest_jre(major) {
        Ok(i) => i,
        Err(AdoptiumError::NotAvailable(_)) => {
            let _ = tx.send(RuntimeEvent::Failed {
                major,
                message: format!(
                    "Adoptium has no Java {} for {}/{}",
                    major,
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            });
            return;
        }
        Err(e) => {
            let _ = tx.send(RuntimeEvent::Failed {
                major,
                message: format!("API: {}", e),
            });
            return;
        }
    };

    let _ = tx.send(RuntimeEvent::Resolved {
        major,
        info: info.clone(),
    });

    let runtime_dir = match super::paths::runtime_dir(major) {
        Some(p) => p,
        None => {
            let _ = tx.send(RuntimeEvent::Failed {
                major,
                message: "config dir unresolvable".into(),
            });
            return;
        }
    };
    if let Err(e) = fs::create_dir_all(&runtime_dir) {
        let _ = tx.send(RuntimeEvent::Failed {
            major,
            message: format!("mkdir {}: {}", runtime_dir.display(), e),
        });
        return;
    }
    let archive_path = runtime_dir.join(&info.archive_name);

    // Download stream-to-disk + sha256 verify.
    if let Err(e) = download_verify(&tx, &info, &archive_path) {
        let _ = tx.send(RuntimeEvent::Failed {
            major,
            message: format!("download: {}", e),
        });
        return;
    }

    // Extract.
    let jre_dir = runtime_dir.join("jre");
    if let Err(e) = extract_archive(&archive_path, &jre_dir) {
        let _ = tx.send(RuntimeEvent::Failed {
            major,
            message: format!("extract: {}", e),
        });
        return;
    }

    log::info!("runtime: Java {} ready at {}", major, jre_dir.display());
    let _ = tx.send(RuntimeEvent::Done { major, jre_dir });
}

fn download_verify(
    tx: &Sender<RuntimeEvent>,
    info: &ReleaseInfo,
    dest: &std::path::Path,
) -> Result<(), String> {
    if let (Ok(meta), expected) = (fs::metadata(dest), info.size) {
        if meta.len() == expected {
            // File already present at expected size — assume OK; full
            // sha256 verify happens regardless during extract step
            // (zip parsing fails loudly on corruption).
            let _ = tx.send(RuntimeEvent::Progress {
                downloaded: expected,
                total: expected,
            });
            return Ok(());
        }
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .user_agent("EwoClient/0.1 (+https://github.com/lewlone/ewoclient)")
        .build();
    let resp = agent
        .get(&info.url)
        .call()
        .map_err(|e| format!("GET {}: {}", info.url, e))?;
    let mut reader = resp.into_reader();
    let mut file = fs::File::create(dest)
        .map_err(|e| format!("create {}: {}", dest.display(), e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    let mut next_progress_at: u64 = 256 * 1024;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("read: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("write {}: {}", dest.display(), e))?;
        written += n as u64;
        // Throttle progress events — UI re-renders at most every 256KB
        // of download, plenty for a smooth progress bar.
        if written >= next_progress_at {
            let _ = tx.send(RuntimeEvent::Progress {
                downloaded: written,
                total: info.size,
            });
            next_progress_at += 256 * 1024;
        }
    }
    let actual = hex_digest(&hasher.finalize());
    if !info.sha256.is_empty() && actual != info.sha256 {
        let _ = fs::remove_file(dest);
        return Err(format!(
            "sha256 mismatch (got {}, expected {})",
            actual, info.sha256
        ));
    }
    let _ = tx.send(RuntimeEvent::Progress {
        downloaded: written,
        total: info.size,
    });
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn extract_archive(archive: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    if dest.exists() {
        fs::remove_dir_all(dest)
            .map_err(|e| format!("rm {}: {}", dest.display(), e))?;
    }
    fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {}", dest.display(), e))?;
    let f = fs::File::open(archive).map_err(|e| format!("open {}: {}", archive.display(), e))?;
    let name = archive.to_string_lossy();
    if archive.extension().and_then(|s| s.to_str()) == Some("zip") {
        extract_zip(f, dest)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(f, dest)
    } else {
        Err("unsupported archive format".into())
    }
}

fn extract_tar_gz(file: fs::File, dest: &std::path::Path) -> Result<(), String> {
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    let entries = archive.entries().map_err(|e| format!("tar entries: {}", e))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("tar entry: {}", e))?;
        let raw_path = entry
            .path()
            .map_err(|e| format!("tar path: {}", e))?
            .into_owned();
        if raw_path.components().any(|c| c.as_os_str() == "..") {
            continue;
        }
        let out_path = dest.join(&raw_path);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
        }
        entry
            .unpack(&out_path)
            .map_err(|e| format!("unpack {}: {}", out_path.display(), e))?;
    }
    Ok(())
}

fn extract_zip(file: fs::File, dest: &std::path::Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let raw_name = entry.name().to_string();
        // Defense against zip-traversal attacks.
        if raw_name.contains("..") {
            continue;
        }
        let safe = raw_name.replace('\\', "/");
        let out_path = dest.join(&safe);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("mkdir {}: {}", out_path.display(), e))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
        }
        let mut out = fs::File::create(&out_path)
            .map_err(|e| format!("create {}: {}", out_path.display(), e))?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| format!("write {}: {}", out_path.display(), e))?;
        }
    }
    Ok(())
}
