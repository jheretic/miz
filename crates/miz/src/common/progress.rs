//! Neutral progress abstraction. The indicatif renderer (in `render/`) and a
//! future daemon sink both consume these events; core translates native alpm
//! callbacks into them. Field types mirror what `operations/progress.rs`
//! currently reads from the alpm callbacks.

use alpm::Progress;

// wired in a later phase
#[allow(dead_code)]
pub enum ProgressEvent {
    Status(String),
    Op {
        kind: Progress,
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
    },
    Job {
        percent: u64,
    },
}

// wired in a later phase
#[allow(dead_code)]
pub trait ProgressSink {
    fn handle(&mut self, ev: ProgressEvent);
}
