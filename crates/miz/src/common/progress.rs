//! Neutral progress abstraction. The indicatif renderer (in `render/`) and a
//! future daemon sink both consume these events; core translates native alpm
//! callbacks into them here, at the callback boundary, so `ProgressEvent`
//! carries no libalpm type.

use alpm::{Alpm, DownloadEvent, Event, Progress};
use std::cell::RefCell;
use std::rc::Rc;

/// miz-owned mirror of the libalpm transaction operation kinds the renderer
/// labels. Kept separate from `alpm::Progress` so `ProgressEvent` is
/// libalpm-free (a daemon sink never links alpm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Install,
    Upgrade,
    Downgrade,
    Reinstall,
    Remove,
    Conflicts,
    Diskspace,
    Integrity,
    Load,
    Keyring,
}

impl OpKind {
    /// Translate a libalpm `Progress` at the callback boundary.
    pub fn from_alpm(p: Progress) -> Self {
        match p {
            Progress::AddStart => OpKind::Install,
            Progress::UpgradeStart => OpKind::Upgrade,
            Progress::DowngradeStart => OpKind::Downgrade,
            Progress::ReinstallStart => OpKind::Reinstall,
            Progress::RemoveStart => OpKind::Remove,
            Progress::ConflictsStart => OpKind::Conflicts,
            Progress::DiskspaceStart => OpKind::Diskspace,
            Progress::IntegrityStart => OpKind::Integrity,
            Progress::LoadStart => OpKind::Load,
            Progress::KeyringStart => OpKind::Keyring,
        }
    }
}

pub enum ProgressEvent {
    Status(String),
    Op {
        kind: OpKind,
        pkg: String,
        percent: u64,
    },
    Download {
        file: String,
        downloaded: u64,
        total: u64,
    },
    DownloadDone {
        file: String,
        total: u64,
    },
    /// Start of an images D-Bus job bar; `label` is the left-margin verb
    /// ("acquiring"/"installing"). Deviation from the plan doc's bare
    /// `Job{percent}`: the job bar needs a begin (label)/progress/end lifecycle,
    /// which also matches the doc's own daemon analogy (JobRemoved + Progress).
    JobBegin {
        label: String,
    },
    Job {
        percent: u64,
    },
    JobEnd,
}

pub trait ProgressSink {
    fn handle(&mut self, ev: ProgressEvent);

    /// Start a fresh progress session. Renderers that anchor a live display
    /// (indicatif's `MultiProgress`) re-create it here so its cursor anchor is
    /// set AFTER any summary/confirm prints -- preserving the sync_install
    /// ordering (summary -> confirm -> begin -> commit) that fixed the
    /// terminal-jump bug. Default no-op for recording/daemon sinks.
    fn begin(&mut self) {}
}

/// Shared, `'static` sink handle. libalpm's callback registration stores each
/// closure with a `'static` bound, so the three transaction callbacks share the
/// one sink via `Rc<RefCell<..>>` (all fire on the main thread, never
/// re-entrantly, so `borrow_mut` never conflicts).
pub type SharedSink = Rc<RefCell<dyn ProgressSink>>;

/// Map a libalpm status event to the neutral status line text (without the
/// `::` marker or coloring, which the renderer adds). `None` for events miz
/// does not surface.
fn status_text(event: &Event<'_>) -> Option<String> {
    let s = match event {
        Event::CheckDepsStart => "checking dependencies...",
        Event::ResolveDepsStart => "resolving dependencies...",
        Event::InterConflictsStart => "looking for conflicting packages...",
        Event::FileConflictsStart => "checking for file conflicts...",
        Event::IntegrityStart => "checking package integrity...",
        Event::LoadStart => "loading package files...",
        Event::KeyringStart => "checking keyring...",
        Event::TransactionStart => "processing transaction...",
        Event::TransactionDone => "transaction done",
        Event::HookRunStart(h) => {
            let name = h.name();
            if name.is_empty() {
                return None;
            }
            return Some(format!("running hook {name}"));
        }
        _ => return None,
    };
    Some(s.to_string())
}

/// Register the alpm event/progress/download callbacks so each native callback
/// is translated into a [`ProgressEvent`] forwarded to `sink`. This is the
/// PROGRESS seam: core wires native callbacks to a sink; the renderer (or a
/// daemon) decides how to present them.
pub fn register(alpm: &Alpm, sink: SharedSink) {
    let s = sink.clone();
    alpm.set_event_cb((), move |event, _: &mut ()| {
        if let Some(text) = status_text(&event.event()) {
            s.borrow_mut().handle(ProgressEvent::Status(text));
        }
    });

    let s = sink.clone();
    alpm.set_progress_cb(
        (),
        move |kind, pkg, percent, _n, _current, _: &mut ()| {
            s.borrow_mut().handle(ProgressEvent::Op {
                kind: OpKind::from_alpm(kind),
                pkg: pkg.to_string(),
                percent: percent.clamp(0, 100) as u64,
            });
        },
    );

    alpm.set_dl_cb((), move |filename, event, _: &mut ()| {
        let ev = match event.event() {
            DownloadEvent::Init(_) => ProgressEvent::Download {
                file: filename.to_string(),
                downloaded: 0,
                total: 0,
            },
            DownloadEvent::Progress(p) => ProgressEvent::Download {
                file: filename.to_string(),
                downloaded: p.downloaded.max(0) as u64,
                total: p.total.max(0) as u64,
            },
            // Retry resets the position to 0 without changing the length.
            DownloadEvent::Retry(_) => ProgressEvent::Download {
                file: filename.to_string(),
                downloaded: 0,
                total: 0,
            },
            DownloadEvent::Completed(c) => ProgressEvent::DownloadDone {
                file: filename.to_string(),
                total: c.total.max(0) as u64,
            },
        };
        sink.borrow_mut().handle(ev);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opkind_maps_every_alpm_progress_variant() {
        assert_eq!(OpKind::from_alpm(Progress::AddStart), OpKind::Install);
        assert_eq!(OpKind::from_alpm(Progress::UpgradeStart), OpKind::Upgrade);
        assert_eq!(OpKind::from_alpm(Progress::DowngradeStart), OpKind::Downgrade);
        assert_eq!(OpKind::from_alpm(Progress::ReinstallStart), OpKind::Reinstall);
        assert_eq!(OpKind::from_alpm(Progress::RemoveStart), OpKind::Remove);
        assert_eq!(OpKind::from_alpm(Progress::ConflictsStart), OpKind::Conflicts);
        assert_eq!(OpKind::from_alpm(Progress::DiskspaceStart), OpKind::Diskspace);
        assert_eq!(OpKind::from_alpm(Progress::IntegrityStart), OpKind::Integrity);
        assert_eq!(OpKind::from_alpm(Progress::LoadStart), OpKind::Load);
        assert_eq!(OpKind::from_alpm(Progress::KeyringStart), OpKind::Keyring);
    }

    /// A fake sink capturing events, to prove the ProgressSink path without
    /// libalpm. Mirrors what a daemon sink would do (record, not render).
    #[derive(Default)]
    struct CaptureSink {
        statuses: Vec<String>,
        ops: Vec<(OpKind, String, u64)>,
        downloads: Vec<(String, u64, u64)>,
        done: Vec<String>,
        jobs: Vec<u64>,
        job_labels: Vec<String>,
        job_ends: usize,
    }

    impl ProgressSink for CaptureSink {
        fn handle(&mut self, ev: ProgressEvent) {
            match ev {
                ProgressEvent::Status(s) => self.statuses.push(s),
                ProgressEvent::Op { kind, pkg, percent } => self.ops.push((kind, pkg, percent)),
                ProgressEvent::Download {
                    file,
                    downloaded,
                    total,
                } => self.downloads.push((file, downloaded, total)),
                ProgressEvent::DownloadDone { file, .. } => self.done.push(file),
                ProgressEvent::JobBegin { label } => self.job_labels.push(label),
                ProgressEvent::Job { percent } => self.jobs.push(percent),
                ProgressEvent::JobEnd => self.job_ends += 1,
            }
        }
    }

    #[test]
    fn capture_sink_records_events() {
        let mut sink = CaptureSink::default();
        sink.handle(ProgressEvent::Status("resolving dependencies...".into()));
        sink.handle(ProgressEvent::Op {
            kind: OpKind::Install,
            pkg: "bash".into(),
            percent: 42,
        });
        sink.handle(ProgressEvent::Download {
            file: "core.db".into(),
            downloaded: 10,
            total: 100,
        });
        sink.handle(ProgressEvent::DownloadDone {
            file: "core.db".into(),
            total: 100,
        });
        sink.handle(ProgressEvent::Job { percent: 75 });

        assert_eq!(sink.statuses, vec!["resolving dependencies...".to_string()]);
        assert_eq!(sink.ops, vec![(OpKind::Install, "bash".to_string(), 42)]);
        assert_eq!(sink.downloads, vec![("core.db".to_string(), 10, 100)]);
        assert_eq!(sink.done, vec!["core.db".to_string()]);
        assert_eq!(sink.jobs, vec![75]);
    }
}
