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
use crate::polkit::{self, Action};
use crate::sink::progress_signal;
use crate::worker::{self, WorkerRequest};
use async_channel::{Receiver, Sender};
use miz_core::common::progress::ProgressEvent;
use miz_core::error::MizError;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedObjectPath;
use zbus::Connection;

use crate::MANAGER_PATH;

/// Cap on concurrent active+queued jobs (admission control / DoS hardening).
/// Read-only methods create no Job and are bounded by the single serializing
/// worker, so they are not counted here.
const MAX_JOBS: usize = 16;

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

    /// Allocate a job id + path. Enforces the admission cap BEFORE allocating
    /// (no partial state on rejection), then errors if the id space is
    /// exhausted. The registry lock brackets the cap check + allocation so two
    /// concurrent starts cannot both pass the cap.
    fn allocate_job(&self) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        let mut reg = self.jobs.lock().expect("job registry poisoned");
        if reg.list().len() >= MAX_JOBS {
            return Err(zbus::fdo::Error::LimitsExceeded(
                "too many active jobs".into(),
            ));
        }
        reg.allocate()
            .ok_or_else(|| zbus::fdo::Error::Failed("job id space exhausted".into()))
    }

    /// Send a request to the worker, mapping a closed queue to a D-Bus error.
    async fn dispatch(&self, req: WorkerRequest) -> zbus::fdo::Result<()> {
        self.worker
            .send(req)
            .await
            .map_err(|_| zbus::fdo::Error::Failed("worker queue closed".into()))
    }

    /// polkit-gate a method: resolve the caller's unique bus name from the
    /// message header and check `action`. Returns `AccessDenied` on denial (or
    /// a missing sender). The `CheckAuthorization` round-trip is VM-only.
    async fn authorize(
        &self,
        conn: &Connection,
        header: &zbus::message::Header<'_>,
        action: Action,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::AccessDenied("missing caller bus name".into()))?;
        polkit::check(conn, sender.as_str(), action).await
    }

    /// Start a Job: allocate an id, serve the per-job object, dispatch the
    /// worker request (built by `make_req` from the progress + reply senders),
    /// and spawn the driver that streams `Job.Progress` and emits `JobRemoved`.
    /// Shared by refresh + the mutating methods (same lifecycle shape).
    ///
    /// On any setup failure before the driver is spawned, roll back so neither
    /// the registry entry nor the served object leaks (no terminal task exists
    /// yet to clean them up / emit JobRemoved).
    async fn start_job<F>(
        &self,
        conn: &Connection,
        kind: &str,
        make_req: F,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)>
    where
        F: FnOnce(Sender<ProgressEvent>, Sender<miz_core::error::Result<()>>) -> WorkerRequest,
    {
        let (id, path) = self.allocate_job()?;

        let job = Job::new(id, kind);
        let progress_state = job.progress_handle();
        if let Err(e) = conn.object_server().at(&path, job).await {
            self.jobs.lock().expect("job registry poisoned").remove(id);
            return Err(zbus::fdo::Error::Failed(format!("serve job object: {e}")));
        }

        let (progress, prog_rx) = async_channel::unbounded();
        let (reply, reply_rx) = async_channel::bounded(1);
        if let Err(e) = self.dispatch(make_req(progress, reply)).await {
            let _ = conn.object_server().remove::<Job, _>(&path).await;
            self.jobs.lock().expect("job registry poisoned").remove(id);
            return Err(e);
        }

        let jobs = self.jobs.clone();
        let conn = conn.clone();
        let job_p = path.clone();
        conn.executor()
            .spawn(
                run_job(
                    conn.clone(),
                    jobs,
                    id,
                    job_p,
                    prog_rx,
                    reply_rx,
                    progress_state,
                ),
                "mizd-job",
            )
            .detach();

        Ok((id, path))
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

/// Reject nonzero reserved flags: no flag bits are defined yet, so any set bit
/// is unknown. `flags == 0` proceeds. Pure so it is unit-testable without a bus.
fn check_flags(flags: u32) -> zbus::fdo::Result<()> {
    if flags != 0 {
        Err(zbus::fdo::Error::InvalidArgs("unknown flags".into()))
    } else {
        Ok(())
    }
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

    /// `Install(in as, in u flags) -> (u job_id, o job_path)`.
    async fn install(
        &self,
        packages: Vec<String>,
        flags: u32,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        check_flags(flags)?;
        self.authorize(conn, &header, Action::Install).await?;
        self.start_job(conn, "install", move |progress, reply| {
            WorkerRequest::Install {
                packages,
                flags,
                progress,
                reply,
            }
        })
        .await
    }

    /// `Remove(in as, in u flags) -> (u job_id, o job_path)` — layered removal.
    async fn remove(
        &self,
        packages: Vec<String>,
        flags: u32,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        check_flags(flags)?;
        self.authorize(conn, &header, Action::Install).await?;
        self.start_job(conn, "remove", move |progress, reply| {
            WorkerRequest::Remove {
                packages,
                flags,
                progress,
                reply,
            }
        })
        .await
    }

    /// `Upgrade(in u flags) -> (u job_id, o job_path)` — `-Syu` of layered pkgs.
    async fn upgrade(
        &self,
        flags: u32,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        check_flags(flags)?;
        self.authorize(conn, &header, Action::Install).await?;
        self.start_job(conn, "upgrade", move |progress, reply| {
            WorkerRequest::Upgrade {
                flags,
                progress,
                reply,
            }
        })
        .await
    }

    /// `RefreshDatabases() -> (u job_id, o job_path)` — `-Sy`. Returns a Job
    /// immediately; a spawned task forwards progress and emits `JobRemoved`.
    /// Gated on the lighter `refresh` action (it mutates the sync db cache).
    async fn refresh_databases(
        &self,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<(u32, OwnedObjectPath)> {
        self.authorize(conn, &header, Action::Refresh).await?;
        self.start_job(conn, "refresh", |progress, reply| WorkerRequest::Refresh {
            progress,
            reply,
        })
        .await
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
/// sink stays on the worker thread). Shared by every job kind (refresh +
/// install/remove/upgrade) — they differ only in the WorkerRequest enqueued.
async fn run_job(
    conn: Connection,
    jobs: Arc<Mutex<JobRegistry>>,
    id: u32,
    path: OwnedObjectPath,
    prog_rx: Receiver<ProgressEvent>,
    reply_rx: Receiver<miz_core::error::Result<()>>,
    progress_state: Arc<AtomicU32>,
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

    #[test]
    fn zero_flags_ok_nonzero_rejected() {
        assert!(check_flags(0).is_ok());
        for bits in [1u32, 2, 0x8000_0000, u32::MAX] {
            match check_flags(bits) {
                Err(zbus::fdo::Error::InvalidArgs(_)) => {}
                other => panic!("expected InvalidArgs, got {other:?}"),
            }
        }
    }

    #[test]
    fn admission_cap_rejects_beyond_max_jobs() {
        let mgr = Manager::new();
        for _ in 0..MAX_JOBS {
            mgr.allocate_job().unwrap();
        }
        match mgr.allocate_job() {
            Err(zbus::fdo::Error::LimitsExceeded(_)) => {}
            other => panic!("expected LimitsExceeded at cap, got {other:?}"),
        }
    }
}
