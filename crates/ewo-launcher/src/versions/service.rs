//! `VersionService` — owns the manifest fetch + cache, polled each frame.
//!
//! UI flow:
//!   1. App startup: `VersionService::new()` hydrates from disk cache (if
//!      present). UI immediately renders the cached version list — even
//!      offline, the user sees something.
//!   2. If the cache is stale (>6h) or absent, a background refresh
//!      thread fetches `version_manifest_v2.json` and emits a
//!      `Refreshed(manifest)` event.
//!   3. UI polls `VersionService::poll()` each frame; refreshed manifests
//!      replace the in-memory list and persist to disk.
//!
//! Errors are non-fatal: a failed fetch leaves the cached manifest in
//! place. The UI shows a status pill ("offline — using cached versions"
//! or similar) when the most recent fetch failed. (Status surface is a
//! polish item; not wired yet.)

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use super::cache;
use super::manifest::{VersionManifest, VERSION_MANIFEST_URL};

/// Snapshot of what the UI should render right now.
#[derive(Clone, Debug)]
pub enum VersionState {
    /// First run, no cache, no fetch yet.
    Empty,
    /// Cache hydrated; manifest available. `stale = true` means a refresh
    /// is in flight or about to be triggered.
    Loaded { manifest: VersionManifest, stale: bool },
    /// Refresh failed since last successful load. Manifest may still be
    /// usable from cache; `last_error` is for diagnostic display.
    Failed { manifest: Option<VersionManifest>, last_error: String },
}

#[derive(Debug)]
pub enum VersionEvent {
    Refreshed(VersionManifest),
    Failure(String),
}

pub struct VersionService {
    state: VersionState,
    rx: Receiver<VersionEvent>,
    tx: Sender<VersionEvent>,
    busy: bool,
}

impl VersionService {
    /// Construct + hydrate from cache. Returns immediately; if a refresh
    /// is needed, caller can call `start_refresh()`.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let (state, needs_refresh) = match cache::load() {
            Some((manifest, fetched_at)) => {
                let stale = cache::is_stale(fetched_at);
                (VersionState::Loaded { manifest, stale }, stale)
            }
            None => (VersionState::Empty, true),
        };
        let mut svc = Self {
            state,
            rx,
            tx,
            busy: false,
        };
        if needs_refresh {
            svc.start_refresh();
        }
        svc
    }

    pub fn state(&self) -> &VersionState {
        &self.state
    }

    /// Convenience: borrow the in-memory manifest if any. Returns `None`
    /// for `Empty` or `Failed { manifest: None }`.
    pub fn manifest(&self) -> Option<&VersionManifest> {
        match &self.state {
            VersionState::Loaded { manifest, .. } => Some(manifest),
            VersionState::Failed { manifest: Some(m), .. } => Some(m),
            _ => None,
        }
    }

    /// Drain pending events. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                VersionEvent::Refreshed(manifest) => {
                    log::info!(
                        "versions: refreshed — {} entries (latest release={})",
                        manifest.versions.len(),
                        manifest.latest.release,
                    );
                    cache::save(&manifest);
                    self.state = VersionState::Loaded {
                        manifest,
                        stale: false,
                    };
                    self.busy = false;
                }
                VersionEvent::Failure(msg) => {
                    log::warn!("versions: refresh failed: {}", msg);
                    let manifest = match &self.state {
                        VersionState::Loaded { manifest, .. } => Some(manifest.clone()),
                        VersionState::Failed { manifest, .. } => manifest.clone(),
                        VersionState::Empty => None,
                    };
                    self.state = VersionState::Failed {
                        manifest,
                        last_error: msg,
                    };
                    self.busy = false;
                }
            }
        }
    }

    /// Start a background refresh if one isn't already in flight.
    pub fn start_refresh(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("ewo-versions-refresh".into())
            .spawn(move || run_refresh(tx))
            .expect("spawn versions refresh thread");
    }
}

fn run_refresh(tx: Sender<VersionEvent>) {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent("EwoClient/0.1 (+https://github.com/lewlone/ewoclient)")
        .build();
    let res = agent.get(VERSION_MANIFEST_URL).call();
    match res {
        Ok(r) => match r.into_json::<VersionManifest>() {
            Ok(manifest) => {
                let _ = tx.send(VersionEvent::Refreshed(manifest));
            }
            Err(e) => {
                let _ = tx.send(VersionEvent::Failure(format!("parse: {}", e)));
            }
        },
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            let _ = tx.send(VersionEvent::Failure(format!("{} HTTP {}", code, body)));
        }
        Err(e) => {
            let _ = tx.send(VersionEvent::Failure(e.to_string()));
        }
    }
}
