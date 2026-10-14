//! Deleting an instance: the confirmation modal, then the folder's move to
//! the Recycle Bin on a worker thread, then (only when that worked) the
//! instance leaves the list. `recycle` holds the disk rules; this is the
//! `App` side: open, confirm, poll.

use super::*;

use crate::recycle::{self, OsRecycler, Removed};

/// A folder move running on a worker. The result arrives on `rx`.
pub(crate) struct PendingDelete {
    pub(crate) id: String,
    pub(crate) rx: std::sync::mpsc::Receiver<Result<Removed, String>>,
}

/// Why this delete can't go ahead right now, if it can't.
/// `busy_ids`: instances with a game running or being prepared.
pub(crate) fn delete_blocker(
    id: &str,
    instance_count: usize,
    busy_ids: &[&str],
) -> Option<&'static str> {
    if instance_count <= 1 {
        Some("the last instance can't be deleted")
    } else if busy_ids.contains(&id) {
        Some("it is running or starting; close the game first")
    } else {
        None
    }
}

impl App {
    /// The × on the Worlds list: ask first. Nothing is touched yet.
    pub(crate) fn open_delete_modal(&mut self, underlying_idx: usize) {
        let Some(inst) = self.instances.get(underlying_idx) else {
            return;
        };
        if self.instances.len() <= 1 {
            log::info!("delete: refused (the last instance can't be deleted)");
            return;
        }
        log::info!("delete: asking to remove \"{}\"", inst.name);
        let folder = format!("instances/{}", inst.id);
        self.delete_modal.open(&inst.id, &inst.name, &folder);
    }

    /// Ids of the instances with a game running or being prepared.
    fn busy_instance_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        ids.extend(self.active_launch_instance_id.clone());
        let by_idx = |idx: usize| self.instances.get(idx).map(|i| i.id.clone());
        ids.extend(self.pending_prepare.as_ref().and_then(|p| by_idx(p.idx)));
        ids.extend(
            self.pending_relaunch
                .as_ref()
                .and_then(|p| by_idx(p.instance_idx)),
        );
        ids.extend(self.pending_auth.as_ref().and_then(|p| by_idx(p.idx)));
        ids
    }

    /// The modal's confirm button: move the folder to the Recycle Bin on a
    /// worker, and leave the instance in the list until that has worked.
    pub(crate) fn confirm_delete(&mut self) {
        if self.delete_modal.busy || self.delete_job.is_some() {
            return;
        }
        let id = self.delete_modal.instance_id.clone();
        if !self.instances.iter().any(|i| i.id == id) {
            self.delete_modal.close();
            return;
        }
        let busy = self.busy_instance_ids();
        let busy: Vec<&str> = busy.iter().map(String::as_str).collect();
        if let Some(why) = delete_blocker(&id, self.instances.len(), &busy) {
            self.delete_modal.error = Some(why.to_string());
            return;
        }
        let Some(root) = downloads::paths::instances_root() else {
            self.delete_modal.error = Some("the launcher's data folder is unknown".to_string());
            return;
        };
        log::info!("delete: moving instances/{id} to the Recycle Bin");
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_id = id.clone();
        let spawned = std::thread::Builder::new()
            .name("ewo-delete-instance".into())
            .spawn(move || {
                let _ = tx.send(recycle::recycle_instance_folder(
                    &root,
                    &worker_id,
                    &OsRecycler,
                ));
            });
        match spawned {
            Ok(_) => {
                self.delete_modal.busy = true;
                self.delete_modal.error = None;
                self.delete_job = Some(PendingDelete { id, rx });
            }
            Err(e) => self.delete_modal.error = Some(format!("could not start the move: {e}")),
        }
    }

    /// Collect the worker's result. Called once per frame. Success removes
    /// the instance from the list; failure keeps the instance and its folder
    /// and shows the error in the modal.
    pub(crate) fn poll_instance_delete(&mut self) {
        let result = match self.delete_job.as_ref().map(|j| j.rx.try_recv()) {
            None | Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return,
            Some(Ok(r)) => r,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Err("the move stopped unexpectedly".to_string())
            }
        };
        let Some(job) = self.delete_job.take() else {
            return;
        };
        match result {
            Ok(removed) => {
                log::info!("delete: instances/{} -> {:?}", job.id, removed);
                if let Some(idx) = self.instances.iter().position(|i| i.id == job.id) {
                    delete_instance(
                        &mut self.instances,
                        &mut self.instance_prefs,
                        idx,
                        self.clock.elapsed,
                    );
                }
                self.delete_modal.close();
            }
            Err(msg) => {
                log::warn!("delete: instances/{} kept: {}", job.id, msg);
                self.delete_modal.busy = false;
                self.delete_modal.error = Some(msg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_instance_and_the_last_instance_cannot_be_deleted() {
        assert_eq!(delete_blocker("a", 3, &[]), None);
        assert_eq!(delete_blocker("a", 3, &["b"]), None);
        assert!(delete_blocker("a", 3, &["b", "a"]).is_some());
        assert!(delete_blocker("a", 1, &[]).is_some());
        assert!(delete_blocker("a", 0, &[]).is_some());
    }
}
