//! The `org.archetype.miz1.Manager` interface at `/org/archetype/miz1`.
//!
//! Phase 2: method bodies are stubs returning empty/canned data. No worker
//! thread, no libalpm. This proves the interface compiles and can be served.

use crate::job::JobRegistry;
use std::sync::Mutex;
use zbus::interface;
use zbus::zvariant::OwnedObjectPath;

pub struct Manager {
    jobs: Mutex<JobRegistry>,
}

impl Manager {
    pub fn new() -> Self {
        Manager {
            jobs: Mutex::new(JobRegistry::new()),
        }
    }

    /// Allocate a job id + path. Phase 3+ enqueues real work onto the worker.
    /// Errors (rather than reusing an id) if the id space is exhausted.
    fn enqueue(&self) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        self.jobs
            .lock()
            .expect("job registry poisoned")
            .allocate()
            .ok_or_else(|| zbus::fdo::Error::Failed("job id space exhausted".into()))
    }
}

impl Default for Manager {
    fn default() -> Self {
        Manager::new()
    }
}

#[interface(name = "org.archetype.miz1.Manager")]
impl Manager {
    /// `ListUpgradable() -> a(sss)` — (name, installed_version, new_version).
    fn list_upgradable(&self) -> Vec<(String, String, String)> {
        Vec::new()
    }

    /// `ListInstalled() -> a(ss)` — (name, version).
    fn list_installed(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// `PreviewInstall(in as) -> (a(ss) targets, s summary)`.
    fn preview_install(&self, _packages: Vec<String>) -> (Vec<(String, String)>, String) {
        (Vec::new(), String::new())
    }

    /// `Install(in as, in u flags) -> (u job_id, o job_path)`.
    fn install(
        &self,
        _packages: Vec<String>,
        _flags: u32,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        self.enqueue()
    }

    /// `Remove(in as, in u flags) -> (u job_id, o job_path)`.
    fn remove(
        &self,
        _packages: Vec<String>,
        _flags: u32,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        self.enqueue()
    }

    /// `Upgrade(in u flags) -> (u job_id, o job_path)`.
    fn upgrade(&self, _flags: u32) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        self.enqueue()
    }

    /// `RefreshDatabases() -> (u job_id, o job_path)`.
    fn refresh_databases(&self) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        self.enqueue()
    }

    /// `ListJobs() -> a(uo)` — active (id, path).
    fn list_jobs(&self) -> Vec<(u32, OwnedObjectPath)> {
        self.jobs.lock().expect("job registry poisoned").list()
    }

    /// `JobRemoved(u id, o path, i status)` — terminal outcome (0 = ok,
    /// >0 exit code, <0 -errno).
    #[zbus(signal)]
    async fn job_removed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        path: OwnedObjectPath,
        status: i32,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_allocates_monotonic_ids() {
        let mgr = Manager::new();
        let (a, _) = mgr.enqueue().unwrap();
        let (b, _) = mgr.enqueue().unwrap();
        assert_eq!((a, b), (0, 1));
        assert_eq!(mgr.list_jobs().len(), 2);
    }

    #[test]
    fn read_only_stubs_return_empty() {
        let mgr = Manager::new();
        assert!(mgr.list_upgradable().is_empty());
        assert!(mgr.list_installed().is_empty());
        let (targets, summary) = mgr.preview_install(vec!["bash".into()]);
        assert!(targets.is_empty());
        assert!(summary.is_empty());
    }
}
