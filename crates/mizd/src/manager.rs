//! The `org.archetype.miz1.Manager` interface at `/org/archetype/miz1`.
//!
//! Phase 3: the read-only + refresh methods dispatch to the single worker
//! thread (the only place libalpm is touched). Read-only methods send a request
//! with a oneshot reply channel and `.await` the reply — the worker runs on its
//! own `std::thread`, so awaiting never blocks the async executor. Refresh
//! returns a Job immediately and spawns a task that forwards `ProgressEvent`s to
//! `Job.Progress` until the worker's terminal result arrives, then emits
//! `Manager.JobRemoved`. Install/Remove/Upgrade/Cancel/polkit are Phase 4.

use crate::job::{Job, JobRegistry, JobSignals};
use crate::sink::progress_signal;
use crate::worker::{self, WorkerRequest};
use async_channel::Sender;
use miz_core::error::MizError;
use std::sync::{Arc, Mutex};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedObjectPath;
use zbus::Connection;

use crate::MANAGER_PATH;

pub struct Manager {
    jobs: Arc<Mutex<JobRegistry>>,
    worker: Sender<WorkerRequest>,
}

impl Manager {
    pub fn new() -> Self {
        Manager {
            jobs: Arc::new(Mutex::new(JobRegistry::new())),
            worker: worker::spawn(),
        }
    }

    /// Allocate a job id + path. Errors (rather than reusing an id) if the id
    /// space is exhausted.
    fn allocate_job(&self) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        self.jobs
            .lock()
            .expect("job registry poisoned")
            .allocate()
            .ok_or_else(|| zbus::fdo::Error::Failed("job id space exhausted".into()))
    }

    /// Send a request to the worker, mapping a closed queue to a D-Bus error.
    async fn dispatch(&self, req: WorkerRequest) -> zbus::fdo::Result<()> {
        self.worker
            .send(req)
            .await
            .map_err(|_| zbus::fdo::Error::Failed("worker queue closed".into()))
    }
}

impl Default for Manager {
    fn default() -> Self {
        Manager::new()
    }
}

/// Map a miz-core error to a D-Bus error for the read-only reply path.
fn to_fdo(e: MizError) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(e.to_string())
}

/// Map a closed reply channel (worker dropped the sender without answering) to
/// a D-Bus error.
fn reply_closed() -> zbus::fdo::Error {
    zbus::fdo::Error::Failed("worker dropped the reply".into())
}

#[interface(name = "org.archetype.miz1.Manager")]
impl Manager {
    /// `ListUpgradable() -> a(sss)` — (name, installed_version, new_version).
    async fn list_upgradable(&self) -> zbus::fdo::Result<Vec<(String, String, String)>> {
        let (reply, rx) = async_channel::bounded(1);
        self.dispatch(WorkerRequest::ListUpgradable { reply })
            .await?;
        rx.recv().await.map_err(|_| reply_closed())?.map_err(to_fdo)
    }

    /// `ListInstalled() -> a(ss)` — (name, version).
    async fn list_installed(&self) -> zbus::fdo::Result<Vec<(String, String)>> {
        let (reply, rx) = async_channel::bounded(1);
        self.dispatch(WorkerRequest::ListInstalled { reply })
            .await?;
        rx.recv().await.map_err(|_| reply_closed())?.map_err(to_fdo)
    }

    /// `PreviewInstall(in as) -> (a(ss) targets, s summary)`.
    async fn preview_install(
        &self,
        packages: Vec<String>,
    ) -> zbus::fdo::Result<(Vec<(String, String)>, String)> {
        let (reply, rx) = async_channel::bounded(1);
        self.dispatch(WorkerRequest::PreviewInstall { packages, reply })
            .await?;
        rx.recv().await.map_err(|_| reply_closed())?.map_err(to_fdo)
    }

    /// `Install(in as, in u flags) -> (u job_id, o job_path)`. Phase 4.
    fn install(
        &self,
        _packages: Vec<String>,
        _flags: u32,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        Err(zbus::fdo::Error::NotSupported("Install: phase 4".into()))
    }

    /// `Remove(in as, in u flags) -> (u job_id, o job_path)`. Phase 4.
    fn remove(
        &self,
        _packages: Vec<String>,
        _flags: u32,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        Err(zbus::fdo::Error::NotSupported("Remove: phase 4".into()))
    }

    /// `Upgrade(in u flags) -> (u job_id, o job_path)`. Phase 4.
    fn upgrade(&self, _flags: u32) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        Err(zbus::fdo::Error::NotSupported("Upgrade: phase 4".into()))
    }

    /// `RefreshDatabases() -> (u job_id, o job_path)` — `-Sy`. Returns a Job
    /// immediately; a spawned task forwards progress and emits `JobRemoved`.
    async fn refresh_databases(
        &self,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        let (id, path) = self.allocate_job()?;

        // Serve the per-job object so a client can subscribe to Job.Progress.
        // On any setup failure below, roll back so neither the registry entry
        // nor the served object leaks (there is no terminal task yet to clean
        // them up / emit JobRemoved).
        let job = Job::new(id, "refresh");
        let progress_state = job.progress_handle();
        if let Err(e) = conn.object_server().at(&path, job).await {
            self.jobs.lock().expect("job registry poisoned").remove(id);
            return Err(zbus::fdo::Error::Failed(format!("serve job object: {e}")));
        }

        let (progress, prog_rx) = async_channel::unbounded();
        let (reply, reply_rx) = async_channel::bounded(1);
        if let Err(e) = self
            .dispatch(WorkerRequest::Refresh { progress, reply })
            .await
        {
            let _ = conn.object_server().remove::<Job, _>(&path).await;
            self.jobs.lock().expect("job registry poisoned").remove(id);
            return Err(e);
        }

        let conn = conn.clone();
        let job_p = path.clone();
        let jobs = self.jobs.clone();
        conn.executor()
            .spawn(
                run_refresh_job(
                    conn.clone(),
                    jobs,
                    id,
                    job_p,
                    prog_rx,
                    reply_rx,
                    progress_state,
                ),
                "mizd-refresh-job",
            )
            .detach();

        Ok((id, path))
    }

    /// `ListJobs() -> a(uo)` — active (id, path).
    fn list_jobs(&self) -> Vec<(u32, OwnedObjectPath)> {
        self.jobs.lock().expect("job registry poisoned").list()
    }

    /// `JobRemoved(u id, o path, i status)` — terminal outcome (0 = ok,
    /// >0 exit code, <0 -errno).
    #[zbus(signal)]
    async fn job_removed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        path: OwnedObjectPath,
        status: i32,
    ) -> zbus::Result<()>;
}

/// Drain progress events into `Job.Progress` signals, await the worker's
/// terminal result, then unregister the Job object and emit `JobRemoved`.
/// Runs on the async executor; every value it captures is `Send` (the `!Send`
/// sink stays on the worker thread).
async fn run_refresh_job(
    conn: Connection,
    jobs: Arc<Mutex<JobRegistry>>,
    id: u32,
    path: OwnedObjectPath,
    prog_rx: async_channel::Receiver<miz_core::common::progress::ProgressEvent>,
    reply_rx: async_channel::Receiver<miz_core::error::Result<()>>,
    progress_state: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    if let Ok(job_emitter) = SignalEmitter::new(&conn, &path) {
        while let Ok(ev) = prog_rx.recv().await {
            if let Some((percent, message)) = progress_signal(&ev) {
                // Update the served `Progress` property (clients may poll it)
                // AND emit the Progress signal (clients may subscribe).
                progress_state.store(percent, std::sync::atomic::Ordering::SeqCst);
                let _ = job_emitter.progress_signal(percent, &message).await;
            }
        }
    }

    let status = match reply_rx.recv().await {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => e.exit_code(),
        Err(_) => MizError::Other("worker dropped the reply".into()).exit_code(),
    };

    jobs.lock().expect("job registry poisoned").remove(id);
    let _ = conn.object_server().remove::<Job, _>(&path).await;
    if let Ok(mgr_emitter) = SignalEmitter::new(&conn, MANAGER_PATH) {
        let _ = Manager::job_removed(&mgr_emitter, id, path, status).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::job_path;

    #[test]
    fn allocate_job_is_monotonic_and_tracked() {
        let mgr = Manager::new();
        let (a, _) = mgr.allocate_job().unwrap();
        let (b, _) = mgr.allocate_job().unwrap();
        assert_eq!((a, b), (0, 1));
        assert_eq!(mgr.list_jobs().len(), 2);
    }

    #[test]
    fn job_path_matches_registry_allocation() {
        let mgr = Manager::new();
        let (id, path) = mgr.allocate_job().unwrap();
        assert_eq!(path, job_path(id));
    }
}
