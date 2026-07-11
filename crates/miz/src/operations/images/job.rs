//! Job progress loop for Acquire/Install.
//!
//! `Acquire`/`Install` return a job id + object path and run asynchronously.
//! Completion is announced by `Manager.JobRemoved(id, path, status)`; live
//! progress is the `Job.Progress` (0-100) property. The blocking signal
//! iterator's `next()` blocks indefinitely, so it cannot be interleaved with
//! property polls on one thread — we move the iterator to a helper thread,
//! forward `(id, status)` over a channel, and poll `Progress` on the main
//! thread until the matching id arrives.

use crate::common::progress::{ProgressEvent, SharedSink};
use crate::error::{MizError, Result};
use crate::operations::images::client::JobProxyBlocking;
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

/// Wait for job `id` (at `path`) to finish, emitting progress into `sink`.
/// Returns Ok(()) on status 0, else a `Sysupdate` error describing the failure.
/// `removed` is a pre-subscribed `JobRemoved` signal iterator (subscribed BEFORE
/// the Acquire/Install call to avoid a race window). `label` is the left-margin
/// verb shown on the bar (e.g. "acquiring", "installing"). Whether a bar is
/// actually drawn is the sink's decision (see `IndicatifSink`).
pub fn wait<I>(
    conn: &Connection,
    path: &OwnedObjectPath,
    id: u64,
    removed: I,
    label: &str,
    sink: &SharedSink,
) -> Result<()>
where
    I: Iterator<Item = (u64, i32)> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<(u64, i32)>();
    let handle = std::thread::spawn(move || {
        for (rid, status) in removed {
            // Only the matching job matters; forward and stop.
            if tx.send((rid, status)).is_err() || rid == id {
                break;
            }
        }
    });

    let job = JobProxyBlocking::builder(conn)
        .path(path.clone())?
        .build()
        .ok();

    sink.borrow_mut().handle(ProgressEvent::JobBegin {
        label: label.to_string(),
    });

    let status = loop {
        if let Some(j) = &job {
            if let Ok(p) = j.progress() {
                sink.borrow_mut()
                    .handle(ProgressEvent::Job { percent: p as u64 });
            }
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok((rid, status)) if rid == id => break status,
            Ok(_) => continue, // some other job finished
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Signal thread ended before our job's JobRemoved arrived. The
                // outcome is unknown; for an OS-mutating op, surface that rather
                // than assuming success.
                let _ = handle.join();
                sink.borrow_mut().handle(ProgressEvent::JobEnd);
                return Err(MizError::Sysupdate(format!(
                    "lost track of update job {id} before completion (no JobRemoved signal)"
                )));
            }
        }
    };

    let _ = handle.join();
    sink.borrow_mut().handle(ProgressEvent::JobEnd);

    match status {
        0 => Ok(()),
        s if s > 0 => Err(MizError::Sysupdate(format!(
            "update job {id} failed (exit status {s})"
        ))),
        s => Err(MizError::Sysupdate(format!(
            "update job {id} failed: {}",
            std::io::Error::from_raw_os_error(-s)
        ))),
    }
}
