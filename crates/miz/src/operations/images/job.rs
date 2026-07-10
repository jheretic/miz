//! Job progress loop for Acquire/Install.
//!
//! `Acquire`/`Install` return a job id + object path and run asynchronously.
//! Completion is announced by `Manager.JobRemoved(id, path, status)`; live
//! progress is the `Job.Progress` (0-100) property. The blocking signal
//! iterator's `next()` blocks indefinitely, so it cannot be interleaved with
//! property polls on one thread — we move the iterator to a helper thread,
//! forward `(id, status)` over a channel, and poll `Progress` on the main
//! thread until the matching id arrives.

use crate::error::{MizError, Result};
use crate::operations::images::client::JobProxyBlocking;
use crate::operations::progress::bar_style_job;
use indicatif::ProgressBar;
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

/// Wait for job `id` (at `path`) to finish, rendering a progress bar unless
/// `no_bar`. Returns Ok(()) on status 0, else a `Sysupdate` error describing
/// the failure. `removed` is a pre-subscribed `JobRemoved` signal iterator
/// (subscribed BEFORE the Acquire/Install call to avoid a race window). `label`
/// is the left-margin verb shown on the bar (e.g. "acquiring", "installing").
pub fn wait<I>(
    conn: &Connection,
    path: &OwnedObjectPath,
    id: u64,
    no_bar: bool,
    removed: I,
    label: &str,
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

    let bar = if no_bar {
        None
    } else {
        // A dedicated style whose prefix carries the verb; the transaction
        // style (`bar_style_op`) expects a per-package {msg} and left an empty
        // 12-col prefix + blank trailing text on this D-Bus job bar.
        let b = ProgressBar::new(100).with_style(bar_style_job());
        b.set_prefix(label.to_string());
        b.enable_steady_tick(Duration::from_millis(120));
        Some(b)
    };

    let status = loop {
        if let Some(b) = &bar {
            if let Some(j) = &job {
                if let Ok(p) = j.progress() {
                    b.set_position(p as u64);
                }
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
                if let Some(b) = bar {
                    b.finish_and_clear();
                }
                return Err(MizError::Sysupdate(format!(
                    "lost track of update job {id} before completion (no JobRemoved signal)"
                )));
            }
        }
    };

    let _ = handle.join();
    if let Some(b) = bar {
        b.finish_and_clear();
    }

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
