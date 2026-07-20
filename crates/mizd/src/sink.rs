//! The daemon's `ProgressSink`. It forwards each `ProgressEvent` over an mpsc
//! channel; the async side receives events and emits `Job.Progress` signals.
//!
//! `SharedSink` is `Rc<RefCell<dyn ProgressSink>>` (`!Send`), so the sink is
//! created and dropped on the worker thread (a later phase). It only holds an
//! `mpsc::Sender<ProgressEvent>`, which IS `Send` — the `!Send` sink itself
//! never crosses a thread boundary.

// Wired into the worker + async receiver in Phase 3; Phase 2 defines the shape
// and unit-tests the pure mapping.
#![allow(dead_code)]

use miz_core::common::progress::{ProgressEvent, ProgressSink};
use std::sync::mpsc::Sender;

/// A `ProgressSink` that pushes each event onto a channel for the async task.
pub struct ChannelSink {
    tx: Sender<ProgressEvent>,
}

impl ChannelSink {
    pub fn new(tx: Sender<ProgressEvent>) -> Self {
        ChannelSink { tx }
    }
}

impl ProgressSink for ChannelSink {
    fn handle(&mut self, ev: ProgressEvent) {
        // A closed receiver means the job is gone; dropping the event is fine.
        let _ = self.tx.send(ev);
    }
}

/// Map a `ProgressEvent` to the `Job.Progress(u percent, s message)` payload.
/// Pure — no D-Bus, no libalpm. `None` for events that carry no signal-worthy
/// progress on their own (e.g. `JobBegin`/`JobEnd` lifecycle markers).
pub fn progress_signal(ev: &ProgressEvent) -> Option<(u32, String)> {
    // Clamp every mapped percentage into the 0..=100 the D-Bus `Progress`
    // contract promises (a GS-plugin client relies on it).
    let clamp = |p: u64| p.min(100) as u32;
    match ev {
        ProgressEvent::Status(s) => Some((0, s.clone())),
        ProgressEvent::Op { kind, pkg, percent } => {
            Some((clamp(*percent), format!("{kind:?} {pkg}")))
        }
        ProgressEvent::Download {
            file,
            downloaded,
            total,
        } => {
            // u128 intermediate so downloaded*100 can't overflow; clamp guards
            // downloaded > total.
            let pct = if *total > 0 {
                clamp(((*downloaded as u128 * 100) / *total as u128) as u64)
            } else {
                0
            };
            Some((pct, format!("downloading {file}")))
        }
        ProgressEvent::DownloadDone { file, .. } => Some((100, format!("downloaded {file}"))),
        ProgressEvent::Job { percent } => Some((clamp(*percent), String::new())),
        ProgressEvent::JobBegin { .. } | ProgressEvent::JobEnd => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miz_core::common::progress::OpKind;
    use std::sync::mpsc;

    #[test]
    fn status_maps_to_zero_percent_with_text() {
        let ev = ProgressEvent::Status("resolving dependencies...".into());
        assert_eq!(
            progress_signal(&ev),
            Some((0, "resolving dependencies...".to_string()))
        );
    }

    #[test]
    fn op_maps_percent_and_labels_pkg() {
        let ev = ProgressEvent::Op {
            kind: OpKind::Install,
            pkg: "bash".into(),
            percent: 42,
        };
        assert_eq!(progress_signal(&ev), Some((42, "Install bash".to_string())));
    }

    #[test]
    fn download_computes_percent() {
        let ev = ProgressEvent::Download {
            file: "core.db".into(),
            downloaded: 25,
            total: 100,
        };
        assert_eq!(
            progress_signal(&ev),
            Some((25, "downloading core.db".to_string()))
        );
    }

    #[test]
    fn download_zero_total_is_zero_percent() {
        let ev = ProgressEvent::Download {
            file: "core.db".into(),
            downloaded: 0,
            total: 0,
        };
        assert_eq!(
            progress_signal(&ev),
            Some((0, "downloading core.db".to_string()))
        );
    }

    #[test]
    fn download_done_is_full() {
        let ev = ProgressEvent::DownloadDone {
            file: "core.db".into(),
            total: 100,
        };
        assert_eq!(
            progress_signal(&ev),
            Some((100, "downloaded core.db".to_string()))
        );
    }

    #[test]
    fn job_percent_no_message() {
        let ev = ProgressEvent::Job { percent: 75 };
        assert_eq!(progress_signal(&ev), Some((75, String::new())));
    }

    #[test]
    fn lifecycle_markers_have_no_signal() {
        assert_eq!(progress_signal(&ProgressEvent::JobEnd), None);
        assert_eq!(
            progress_signal(&ProgressEvent::JobBegin {
                label: "installing".into()
            }),
            None
        );
    }

    /// The sink forwards events over the channel; construct + drop it on this
    /// same thread (SharedSink is !Send).
    #[test]
    fn channel_sink_forwards_events() {
        let (tx, rx) = mpsc::channel();
        let mut sink = ChannelSink::new(tx);
        sink.handle(ProgressEvent::Job { percent: 10 });
        sink.handle(ProgressEvent::Status("done".into()));
        drop(sink);

        let events: Vec<_> = rx.iter().collect();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ProgressEvent::Job { percent: 10 }));
        assert!(matches!(events[1], ProgressEvent::Status(_)));
    }
}
