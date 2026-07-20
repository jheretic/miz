//! The `org.archetype.miz1.Job` interface + the job registry.
//!
//! The registry allocates monotonic ids and tracks active (id -> path). A `Job`
//! carries a shared `Arc<AtomicU32>` progress cell its driver task updates.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use zbus::interface;
use zbus::zvariant::OwnedObjectPath;

/// Monotonic job-id allocator + active-job tracker. Ids are never reused while
/// a job is active (in fact never reused at all — a monotonic counter).
pub struct JobRegistry {
    /// Next id to hand out, or `None` once the u32 space is exhausted (so the
    /// final id `u32::MAX` is still allocatable, and no id is ever reused).
    next: Option<u32>,
    active: BTreeMap<u32, OwnedObjectPath>,
}

impl Default for JobRegistry {
    fn default() -> Self {
        JobRegistry {
            next: Some(0),
            active: BTreeMap::new(),
        }
    }
}

impl JobRegistry {
    pub fn new() -> Self {
        JobRegistry::default()
    }

    /// Allocate the next id and its object path, recording it active. Returns
    /// `None` if the monotonic id space is exhausted — the "never reused"
    /// invariant holds even at the u32 ceiling (the final id `u32::MAX` is
    /// handed out, then further calls return `None` rather than wrapping onto a
    /// live id).
    pub fn allocate(&mut self) -> Option<(u32, OwnedObjectPath)> {
        let id = self.next?;
        self.next = id.checked_add(1);
        let path = job_path(id);
        self.active.insert(id, path.clone());
        Some((id, path))
    }

    /// Remove a finished job from the active set. Called on job termination
    /// (the refresh-job task) before emitting `JobRemoved`.
    pub fn remove(&mut self, id: u32) -> Option<OwnedObjectPath> {
        self.active.remove(&id)
    }

    /// Active (id, path) pairs, for `Manager.ListJobs`.
    pub fn list(&self) -> Vec<(u32, OwnedObjectPath)> {
        self.active.iter().map(|(id, p)| (*id, p.clone())).collect()
    }
}

/// The object path for a job id: `/org/archetype/miz1/job/<id>`.
pub fn job_path(id: u32) -> OwnedObjectPath {
    OwnedObjectPath::try_from(format!("/org/archetype/miz1/job/{id}"))
        .expect("job id yields a valid object path")
}

/// A served Job object. `progress` is shared with the job's driver task (the
/// refresh loop) via an `Arc<AtomicU32>` so the served `Progress` property
/// reflects live progress; `Cancel` is a Phase-4 no-op stub.
pub struct Job {
    id: u32,
    kind: String,
    progress: Arc<AtomicU32>,
}

impl Job {
    pub fn new(id: u32, kind: impl Into<String>) -> Self {
        Job {
            id,
            kind: kind.into(),
            progress: Arc::new(AtomicU32::new(0)),
        }
    }

    /// A handle to this job's progress cell, for the driver task to update as
    /// the operation advances (so the served `Progress` property tracks it).
    pub fn progress_handle(&self) -> Arc<AtomicU32> {
        self.progress.clone()
    }
}

#[interface(name = "org.archetype.miz1.Job")]
impl Job {
    #[zbus(property)]
    fn id(&self) -> u32 {
        self.id
    }

    #[zbus(property)]
    fn kind(&self) -> String {
        self.kind.clone()
    }

    #[zbus(property)]
    fn progress(&self) -> u32 {
        self.progress.load(Ordering::SeqCst)
    }

    /// Cancel the job. Phase 4 wires this to `alpm_trans_interrupt` via the
    /// worker; this phase is a no-op stub.
    fn cancel(&self) {}

    /// `Progress(u percent, s message)` — emitted as the transaction advances.
    /// Rust name differs from the property `progress`; D-Bus member stays
    /// `Progress` (member type disambiguates property vs signal on the bus).
    #[zbus(signal, name = "Progress")]
    async fn progress_signal(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        percent: u32,
        message: &str,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic() {
        let mut reg = JobRegistry::new();
        let (a, _) = reg.allocate().unwrap();
        let (b, _) = reg.allocate().unwrap();
        let (c, _) = reg.allocate().unwrap();
        assert_eq!((a, b, c), (0, 1, 2));
    }

    #[test]
    fn removing_a_job_does_not_reuse_its_id() {
        let mut reg = JobRegistry::new();
        let (a, _) = reg.allocate().unwrap();
        reg.remove(a);
        let (b, _) = reg.allocate().unwrap();
        assert_ne!(a, b);
        assert_eq!(b, 1);
    }

    #[test]
    fn list_tracks_only_active_jobs() {
        let mut reg = JobRegistry::new();
        let (a, pa) = reg.allocate().unwrap();
        let (b, pb) = reg.allocate().unwrap();
        assert_eq!(reg.list(), vec![(a, pa), (b, pb.clone())]);
        reg.remove(a);
        assert_eq!(reg.list(), vec![(b, pb)]);
    }

    #[test]
    fn allocate_errors_at_exhaustion_rather_than_reusing() {
        let mut reg = JobRegistry::new();
        reg.next = Some(u32::MAX);
        let (id, _) = reg.allocate().unwrap(); // last valid id is handed out
        assert_eq!(id, u32::MAX);
        assert!(reg.allocate().is_none()); // exhausted -> None, not a reused id
    }

    #[test]
    fn job_path_format() {
        assert_eq!(job_path(7).as_str(), "/org/archetype/miz1/job/7");
    }
}
