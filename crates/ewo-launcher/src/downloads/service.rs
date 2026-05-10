//! `DownloadService` — owns the in-flight download jobs and the channel
//! the UI thread polls for events.
//!
//! Today supports one job at a time per version ID. The UI surfaces a
//! "downloading" status on the instance row + drives the launching
//! screen's progress bar from a real measurement.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

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

pub struct DownloadService {
    tx: Sender<(String, JobEvent)>,
    rx: Receiver<(String, JobEvent)>,
    /// Active job statuses keyed by version ID.
    statuses: HashMap<String, JobStatus>,
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
            handles: HashMap::new(),
        }
    }

    /// Start a download job for the given master-manifest entry. No-op if
    /// a job for this ID is already in flight.
    pub fn start(&mut self, entry: ManifestEntry) {
        let id = entry.id.clone();
        if self.statuses.contains_key(&id) {
            log::info!("downloads: {} already in flight — ignoring", id);
            return;
        }
        log::info!("downloads: starting job for {}", id);
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
        let handle = job::spawn(JobConfig { entry }, inner_tx);
        self.handles.insert(id.clone(), handle);
        self.statuses.insert(id, JobStatus::default());
    }

    /// Drain pending events and update statuses. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok((id, event)) = self.rx.try_recv() {
            let status = self.statuses.entry(id.clone()).or_default();
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
                    status.done = true;
                    if let Some(handle) = self.handles.remove(&id) {
                        let _ = handle.join();
                    }
                }
                JobEvent::Failed(msg) => {
                    log::warn!("downloads[{}]: FAILED {}", id, msg);
                    status.error = Some(msg);
                    if let Some(handle) = self.handles.remove(&id) {
                        let _ = handle.join();
                    }
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
