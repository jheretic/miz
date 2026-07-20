//! The single libalpm worker thread + serialized job queue.
//!
//! libalpm is synchronous, holds a process-global db lock, and uses a global
//! static handle (`miz_core` `ALPM_HANDLE`). Therefore ALL libalpm access goes
//! through this one `std::thread`, which processes requests ONE AT A TIME. Two
//! transactions can never run concurrently, so the single-handle invariant
//! holds by construction: this module is the only place that builds a Context
//! or calls a miz-core operation.
//!
//! The `SharedSink` (`Rc<RefCell<dyn ProgressSink>>`, `!Send`) is created, used,
//! and dropped ON this thread — it never crosses the channel. Only
//! `ProgressEvent` (via an `async_channel::Sender`, which is `Send`) and the
//! operation results travel back to the async side.

use crate::sink::ChannelSink;
use async_channel::{Receiver, Sender};
use miz_core::common::progress::ProgressEvent;
use miz_core::common::report::{AssumeYes, SyncReport};
use miz_core::config::{self, Context};
use miz_core::error::{MizError, Result};
use miz_core::operations::{query, sync};
use miz_core::params;
use std::cell::RefCell;
use std::rc::Rc;

/// The `PreviewInstall` reply payload: resolved targets + a summary line.
type PreviewResult = Result<(Vec<(String, String)>, String)>;

/// A request handed to the worker. Each variant carries a `reply` sender (a
/// oneshot: a bounded(1) channel) the worker writes its single result into.
/// Progress-bearing ops also carry a `progress` sender the worker's sink
/// forwards `ProgressEvent`s over.
pub enum WorkerRequest {
    ListInstalled {
        reply: Sender<Result<Vec<(String, String)>>>,
    },
    ListUpgradable {
        reply: Sender<Result<Vec<(String, String, String)>>>,
    },
    PreviewInstall {
        packages: Vec<String>,
        reply: Sender<PreviewResult>,
    },
    Refresh {
        progress: Sender<ProgressEvent>,
        reply: Sender<Result<()>>,
    },
    // Phase 4: mutating ops go through the same worker with AssumeYes. Defined
    // now so the queue shape is stable; not yet dispatched by the Manager.
    #[allow(dead_code)]
    Install {
        packages: Vec<String>,
        flags: u32,
        progress: Sender<ProgressEvent>,
        reply: Sender<Result<()>>,
    },
    #[allow(dead_code)]
    Remove {
        packages: Vec<String>,
        flags: u32,
        progress: Sender<ProgressEvent>,
        reply: Sender<Result<()>>,
    },
    #[allow(dead_code)]
    Upgrade {
        flags: u32,
        progress: Sender<ProgressEvent>,
        reply: Sender<Result<()>>,
    },
}

/// Spawn the worker thread and return the queue sender the Manager keeps.
/// Dropping every sender ends the loop (the receiver closes).
pub fn spawn() -> Sender<WorkerRequest> {
    let (tx, rx) = async_channel::unbounded::<WorkerRequest>();
    std::thread::Builder::new()
        .name("mizd-worker".into())
        .spawn(move || worker_loop(rx))
        .expect("spawn mizd worker thread");
    tx
}

/// Process requests one at a time until the queue closes. Blocking recv is
/// correct here: this is a dedicated `std::thread`, never an async executor.
fn worker_loop(rx: Receiver<WorkerRequest>) {
    while let Ok(req) = rx.recv_blocking() {
        match req {
            WorkerRequest::ListInstalled { reply } => {
                let _ = reply.send_blocking(list_installed());
            }
            WorkerRequest::ListUpgradable { reply } => {
                let _ = reply.send_blocking(list_upgradable());
            }
            WorkerRequest::PreviewInstall { packages, reply } => {
                let _ = reply.send_blocking(preview_install(packages));
            }
            WorkerRequest::Refresh { progress, reply } => {
                let _ = reply.send_blocking(refresh(progress));
            }
            WorkerRequest::Install { reply, .. }
            | WorkerRequest::Remove { reply, .. }
            | WorkerRequest::Upgrade { reply, .. } => {
                // Phase 4.
                let _ = reply.send_blocking(Err(MizError::NotImplemented));
            }
        }
    }
}

/// Default context params: the daemon uses the system config/root/dbpath (no
/// overrides), mirroring a plain CLI invocation with no `--config`/`--root`.
fn ctx_params() -> params::ContextParams {
    params::ContextParams {
        config: None,
        root: None,
        dbpath: None,
    }
}

fn build_ctx() -> Result<Context> {
    // `.files` dbext is only for `-F`; read/refresh paths pass None.
    let (ctx, _color) = config::build_with_dbext(&ctx_params(), None)?;
    Ok(ctx)
}

/// `-Q` plain: the installed union (localdb + image db), name/version pairs.
fn list_installed() -> Result<Vec<(String, String)>> {
    let ctx = build_ctx()?;
    let report = query::run(query_params(false), &ctx)?;
    Ok(list_pairs(report))
}

/// `-Qu`: upgradable layered packages as (name, installed, new).
///
/// `query::run` with `upgrades: true` yields only (name, installed_version) in
/// its `QueryBody::List` — the new sync version it computes internally is
/// discarded by the report shape, which mizd cannot change (miz-core untouched).
/// So the triple is assembled directly here: the same read-only predicate
/// query.rs uses (`Package::sync_new_version`), run ON the worker so no libalpm
/// access escapes the single-handle invariant. No transaction is initiated.
fn list_upgradable() -> Result<Vec<(String, String, String)>> {
    let ctx = build_ctx()?;
    let alpm = &ctx.alpm;
    let mut out = Vec::new();
    for pkg in alpm.localdb().pkgs() {
        if let Some(newpkg) = pkg.sync_new_version(alpm.syncdbs()) {
            out.push((
                pkg.name().to_string(),
                pkg.version().as_str().to_string(),
                newpkg.version().as_str().to_string(),
            ));
        }
    }
    Ok(out)
}

/// `-Sp` preview: resolve the transaction WITHOUT committing (the print=true /
/// NO_LOCK path), returning (targets, summary). `print_format` "%n\t%v" makes
/// each print line a clean name/version pair to split on.
fn preview_install(packages: Vec<String>) -> PreviewResult {
    let mut ctx = build_ctx()?;
    let sink = new_sink_discarding();
    let mut confirmer = AssumeYes;
    let mut params = base_sync_params();
    params.print = true;
    params.print_format = Some("%n\t%v".to_string());
    params.targets = packages;
    let report = sync::run(params, &mut ctx, &mut confirmer, &sink)?;
    match report {
        SyncReport::Print { lines, .. } => {
            let targets: Vec<(String, String)> = lines
                .iter()
                .filter_map(|l| l.split_once('\t'))
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect();
            let summary = format!("{} package(s) to install", targets.len());
            Ok((targets, summary))
        }
        // Any other variant means nothing to resolve (e.g. empty target set).
        _ => Ok((Vec::new(), "0 package(s) to install".to_string())),
    }
}

/// `-Sy`: refresh sync databases, streaming progress over `progress`. The sink
/// holding that `Sender` is built, used, and dropped here (it is `!Send`).
fn refresh(progress: Sender<ProgressEvent>) -> Result<()> {
    let mut ctx = build_ctx()?;
    let sink: miz_core::common::progress::SharedSink =
        Rc::new(RefCell::new(ChannelSink::new(progress)));
    let mut confirmer = AssumeYes;
    let mut params = base_sync_params();
    params.refresh = 1;
    sync::run(params, &mut ctx, &mut confirmer, &sink).map(|_| ())
}

/// A sink whose events go nowhere (the preview path emits no user progress but
/// `sync::run` still takes a sink). Built and dropped on the worker thread.
fn new_sink_discarding() -> miz_core::common::progress::SharedSink {
    let (tx, _rx) = async_channel::unbounded::<ProgressEvent>();
    Rc::new(RefCell::new(ChannelSink::new(tx)))
}

/// Query params for the plain installed listing (no filter/detail flags).
/// `upgrades` selects the `-Qu` variant.
fn query_params(upgrades: bool) -> params::query::Params {
    params::query::Params {
        changelog: false,
        deps: false,
        explicit: false,
        groups: false,
        info: 0,
        check: 0,
        list: false,
        foreign: false,
        native: false,
        owns: None,
        file: None,
        quiet: false,
        search: None,
        unrequired: false,
        upgrades,
        packages: Vec::new(),
    }
}

/// Extract name/version pairs from a plain `-Q` list report.
fn list_pairs(report: miz_core::common::report::QueryReport) -> Vec<(String, String)> {
    use miz_core::common::report::QueryBody;
    match report.body {
        QueryBody::List { pkgs, .. } => pkgs.into_iter().map(|p| (p.name, p.version)).collect(),
        _ => Vec::new(),
    }
}

/// Baseline sync params with everything off; callers flip the fields they need.
fn base_sync_params() -> params::sync::Params {
    params::sync::Params {
        clean: 0,
        groups: false,
        info: 0,
        list: false,
        print: false,
        print_format: None,
        quiet: false,
        search: None,
        sysupgrade: 0,
        downloadonly: false,
        refresh: 0,
        needed: false,
        asdeps: false,
        asexplicit: false,
        ignore: Vec::new(),
        ignoregroup: Vec::new(),
        overwrite: Vec::new(),
        noconfirm: false,
        noprogressbar: false,
        noscriptlet: false,
        targets: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miz_core::common::report::{PkgLine, QueryBody, QueryError, QueryReport};

    #[test]
    fn list_pairs_extracts_name_version_from_list_body() {
        let report = QueryReport {
            body: QueryBody::List {
                quiet: false,
                pkgs: vec![
                    PkgLine {
                        name: "bash".into(),
                        version: "5.2-1".into(),
                    },
                    PkgLine {
                        name: "coreutils".into(),
                        version: "9.5-1".into(),
                    },
                ],
            },
            diagnostics: Vec::new(),
            error: None,
        };
        assert_eq!(
            list_pairs(report),
            vec![
                ("bash".to_string(), "5.2-1".to_string()),
                ("coreutils".to_string(), "9.5-1".to_string()),
            ]
        );
    }

    #[test]
    fn list_pairs_non_list_body_is_empty() {
        let report = QueryReport {
            body: QueryBody::Owns(vec!["x is owned by y".into()]),
            diagnostics: Vec::new(),
            error: Some(QueryError::Other("nope".into())),
        };
        assert!(list_pairs(report).is_empty());
    }

    #[test]
    fn base_sync_params_is_all_off() {
        let p = base_sync_params();
        assert_eq!(p.refresh, 0);
        assert!(!p.print);
        assert!(p.targets.is_empty());
    }

    #[test]
    fn query_params_toggles_only_upgrades() {
        assert!(!query_params(false).upgrades);
        assert!(query_params(true).upgrades);
        assert!(query_params(true).packages.is_empty());
    }

    /// The preview print-line split is the pure mapping mizd relies on. Prove
    /// it here without libalpm (the `sync::run` call is VM-validation-only).
    #[test]
    fn preview_split_parses_tab_separated_pairs() {
        let lines = ["bash\t5.2-1".to_string(), "glibc\t2.40-1".to_string()];
        let targets: Vec<(String, String)> = lines
            .iter()
            .filter_map(|l| l.split_once('\t'))
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            targets,
            vec![
                ("bash".to_string(), "5.2-1".to_string()),
                ("glibc".to_string(), "2.40-1".to_string()),
            ]
        );
    }
}
