//! `DownloadService` — owns the in-flight download jobs and the channel
//! the UI thread polls for events.
//!
//! One job at a time per version ID. A finished or failed job can be
//! started again (retry, or a second instance of the same version with a
//! different loader); a request that arrives while a job for the version is
//! running with a *different* loader is queued and runs right after it, so
//! the version isn't reported done until the loader's libraries are there.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use crate::loaders::LoaderSpec;
use crate::versions::manifest::ManifestEntry;

use super::job::{self, JobConfig, JobEvent, Stage};

/// Snapshot of a download job's current state. Updated by `poll()` as
/// events arrive.
#[derive(Debug, Clone)]
pub struct JobStatus {
    pub stage: Stage,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub done: bool,
    pub error: Option<String>,
}

impl Default for JobStatus {
    fn default() -> Self {
        Self {
            stage: Stage::PerVersion,
            downloaded: 0,
            total: None,
            done: false,
            error: None,
        }
    }
}

impl JobStatus {
    /// Still running (neither done nor failed).
    pub fn in_flight(&self) -> bool {
        !self.done && self.error.is_none()
    }
}

/// What `start` decided to do with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartDecision {
    Start,
    Ignore,
    Queue,
}

/// Pure decision for a start request given the version's current job
/// (status + loader) — see the module docs.
fn decide(
    current: Option<(&JobStatus, &Option<LoaderSpec>)>,
    requested: &Option<LoaderSpec>,
) -> StartDecision {
    match current {
        None => StartDecision::Start,
        Some((status, _)) if !status.in_flight() => StartDecision::Start,
        Some((_, loader)) if loader == requested => StartDecision::Ignore,
        Some(_) => StartDecision::Queue,
    }
}

pub struct DownloadService {
    tx: Sender<(String, JobEvent)>,
    rx: Receiver<(String, JobEvent)>,
    /// Job statuses keyed by version ID.
    statuses: HashMap<String, JobStatus>,
    /// Loader layer of each version's current/last job.
    loaders: HashMap<String, Option<LoaderSpec>>,
    /// Entry + loader to run once the version's current job ends.
    queued: HashMap<String, (ManifestEntry, Option<LoaderSpec>)>,
    /// Live thread handles. Joined when `poll` sees `Done`/`Failed`.
    /// Keep them around so threads aren't detached and panics surface.
    handles: HashMap<String, JoinHandle<()>>,
}

impl DownloadService {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            statuses: HashMap::new(),
            loaders: HashMap::new(),
            queued: HashMap::new(),
            handles: HashMap::new(),
        }
    }

    /// Start a download job for the given master-manifest entry. Ignored if
    /// an identical job is in flight; queued if one with a different loader
    /// is; otherwise (none yet, finished, or failed) started now.
    ///
    /// `loader` is `Some` when the instance was created with a non-vanilla
    /// loader; the job then fetches + merges the loader manifest before
    /// counting bytes, so loader-added libraries (EwoLoader fat jar +
    /// bundled mods) show up on the progress bar instead of being
    /// hot-downloaded later at launch time.
    pub fn start(&mut self, entry: ManifestEntry, loader: Option<LoaderSpec>) {
        let id = entry.id.clone();
        let current = self
            .statuses
            .get(&id)
            .map(|s| (s, self.loaders.get(&id).unwrap_or(&None)));
        match decide(current, &loader) {
            StartDecision::Ignore => {
                log::info!("downloads: {} already in flight — ignoring", id);
            }
            StartDecision::Queue => {
                log::info!("downloads: {} busy with another loader — queued", id);
                self.queued.insert(id, (entry, loader));
            }
            StartDecision::Start => self.spawn_job(entry, loader),
        }
    }

    fn spawn_job(&mut self, entry: ManifestEntry, loader: Option<LoaderSpec>) {
        let id = entry.id.clone();
        log::info!(
            "downloads: starting job for {} (loader: {})",
            id,
            loader.as_ref().map(|s| s.id.as_str()).unwrap_or("vanilla")
        );
        let id_for_chan = id.clone();
        let outer_tx = self.tx.clone();
        let (inner_tx, inner_rx) = mpsc::channel::<JobEvent>();
        // Spawn a relay thread that tags every JobEvent with the version
        // ID before forwarding to the central channel. Avoids carrying
        // the ID through every JobEvent variant.
        std::thread::Builder::new()
            .name(format!("ewo-dl-relay-{}", id))
            .spawn(move || {
                while let Ok(event) = inner_rx.recv() {
                    if outer_tx.send((id_for_chan.clone(), event)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn relay thread");
        let handle = job::spawn(
            JobConfig {
                entry,
                loader: loader.clone(),
            },
            inner_tx,
        );
        self.handles.insert(id.clone(), handle);
        self.loaders.insert(id.clone(), loader);
        self.statuses.insert(id, JobStatus::default());
    }

    /// Drain pending events and update statuses. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok((id, event)) = self.rx.try_recv() {
            let status = self.statuses.entry(id.clone()).or_default();
            let mut ended = false;
            match event {
                JobEvent::StageStart(stage) => {
                    log::info!("downloads[{}]: stage = {:?}", id, stage);
                    status.stage = stage;
                }
                JobEvent::Progress { downloaded, total } => {
                    status.downloaded = downloaded;
                    if total.is_some() {
                        status.total = total;
                    }
                }
                JobEvent::Done => {
                    log::info!("downloads[{}]: done", id);
                    status.stage = Stage::Done;
                    ended = true;
                    // A queued follow-up job keeps the version "not done"
                    // until it finishes too.
                    status.done = !self.queued.contains_key(&id);
                }
                JobEvent::Failed(msg) => {
                    log::warn!("downloads[{}]: FAILED {}", id, msg);
                    status.error = Some(msg);
                    ended = true;
                }
            }
            if ended {
                if let Some(handle) = self.handles.remove(&id) {
                    let _ = handle.join();
                }
                if let Some((entry, loader)) = self.queued.remove(&id) {
                    self.spawn_job(entry, loader);
                }
            }
        }
    }

    pub fn status(&self, id: &str) -> Option<&JobStatus> {
        self.statuses.get(id)
    }

    /// Iterate all known statuses. Useful for the UI to render badges
    /// per instance regardless of whether each has an active job.
    pub fn iter_statuses(&self) -> impl Iterator<Item = (&String, &JobStatus)> {
        self.statuses.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ewo() -> Option<LoaderSpec> {
        Some(LoaderSpec {
            id: "ewo".into(),
            url: "file:///x.json".into(),
        })
    }

    #[test]
    fn start_decisions() {
        let running = JobStatus::default();
        let done = JobStatus {
            done: true,
            ..JobStatus::default()
        };
        let failed = JobStatus {
            error: Some("boom".into()),
            ..JobStatus::default()
        };
        assert_eq!(decide(None, &None), StartDecision::Start);
        // Failed jobs can be retried.
        assert_eq!(decide(Some((&failed, &None)), &None), StartDecision::Start);
        // A finished vanilla job doesn't swallow a later loader request.
        assert_eq!(decide(Some((&done, &None)), &ewo()), StartDecision::Start);
        assert_eq!(decide(Some((&running, &None)), &None), StartDecision::Ignore);
        assert_eq!(decide(Some((&running, &None)), &ewo()), StartDecision::Queue);
        assert_eq!(decide(Some((&running, &ewo())), &ewo()), StartDecision::Ignore);
    }
}
