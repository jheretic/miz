//! Indicatif renderer for the PROGRESS seam. `IndicatifSink` consumes the
//! neutral [`ProgressEvent`]s that core emits and reproduces the exact bars miz
//! has always drawn: the dup-bar fix (retain last_kind/last_pkg past 100%), the
//! colorized `::` status lines via `Palette`, and the op/dl/job bar styles.

use crate::common::progress::{OpKind, ProgressEvent, ProgressSink};
use crate::render::palette::Palette;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::time::Duration;

pub(crate) fn bar_style_op() -> ProgressStyle {
    // Left-align the label at a fixed 12-col margin (was `>15`, which
    // right-aligned short verbs like "loading" toward the screen centre). The
    // package name follows the bar as {msg}, so the row reads
    // "installing   [####] 42% <pkg>".
    ProgressStyle::with_template("{prefix:<12} {bar:30} {percent:>3}% {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("##-")
}

/// Style for the image-update (`-Iu`/`-I`) D-Bus job bar. Like `bar_style_op`
/// but with no trailing `{msg}` (the job has no per-package name) and an elapsed
/// timer so a long, quiet install still shows life.
pub(crate) fn bar_style_job() -> ProgressStyle {
    ProgressStyle::with_template("{prefix:<12} {bar:30} {percent:>3}% ({elapsed})")
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("##-")
}

fn bar_style_dl() -> ProgressStyle {
    // The prefix carries the "downloading <file>" descriptor (set in the dl cb),
    // left-aligned at a fixed margin so bars line up at the left edge instead of
    // being pushed toward the centre by the old `>30` right-alignment.
    ProgressStyle::with_template(
        "{prefix:<32} {bytes:>10}/{total_bytes:<10} {bar:24} {percent:>3}% {bytes_per_sec}",
    )
    .unwrap_or_else(|_| ProgressStyle::default_bar())
    .progress_chars("##-")
}

fn label_for(kind: OpKind) -> &'static str {
    match kind {
        OpKind::Install => "installing",
        OpKind::Upgrade => "upgrading",
        OpKind::Downgrade => "downgrading",
        OpKind::Reinstall => "reinstalling",
        OpKind::Remove => "removing",
        OpKind::Conflicts => "conflicts",
        OpKind::Diskspace => "diskspace",
        OpKind::Integrity => "integrity",
        OpKind::Load => "loading",
        OpKind::Keyring => "keyring",
    }
}

/// The `MultiProgress`-backed sink. Created disabled unless progress bars are
/// wanted (a TTY stderr and no `--noprogressbar`); when disabled every event is
/// a no-op so callers never branch. The `MultiProgress` is (re)created in
/// [`ProgressSink::begin`] so its cursor anchor lands after any summary/confirm
/// output (the terminal-jump fix).
pub struct IndicatifSink {
    enabled: bool,
    palette: Palette,
    mp: Option<MultiProgress>,
    // Op-bar state (dup-bar fix: last_kind/last_pkg survive 100%).
    op_bar: Option<ProgressBar>,
    last_kind: Option<OpKind>,
    last_pkg: String,
    // Download bars, keyed by filename.
    dl_bars: HashMap<String, ProgressBar>,
    // Images job bar.
    job_bar: Option<ProgressBar>,
}

impl IndicatifSink {
    /// Build a sink. `wanted` is the caller's request (e.g. `!noprogressbar`);
    /// bars are suppressed unless `wanted` and stderr is a TTY, matching the old
    /// `progress_indicatif::install` gate.
    pub fn new(wanted: bool, palette: &Palette) -> Self {
        let enabled = wanted && std::io::stderr().is_terminal();
        IndicatifSink {
            enabled,
            palette: palette.clone(),
            mp: None,
            op_bar: None,
            last_kind: None,
            last_pkg: String::new(),
            dl_bars: HashMap::new(),
            job_bar: None,
        }
    }

    fn mp(&mut self) -> Option<&MultiProgress> {
        if !self.enabled {
            return None;
        }
        if self.mp.is_none() {
            self.mp = Some(MultiProgress::new());
        }
        self.mp.as_ref()
    }

    fn on_status(&mut self, text: String) {
        if !self.enabled {
            return;
        }
        let marker = self.palette.status.apply_to("::").to_string();
        if let Some(mp) = self.mp() {
            let _ = mp.println(format!("{marker} {text}"));
        }
    }

    fn on_op(&mut self, kind: OpKind, pkg: String, percent: u64) {
        if !self.enabled {
            return;
        }
        // A genuinely new bar is needed only when the (kind, pkg) pair changes.
        // alpm fires this repeatedly for one package -- including several times
        // at 100% -- so last_kind/last_pkg must survive completion: clearing
        // them on 100% made every extra 100% call look "new" and spawn a fresh
        // (instantly-finished) bar (the duplicate-bars-per-package bug).
        let new_bar = self.last_kind != Some(kind) || self.last_pkg != pkg;
        if new_bar {
            if let Some(prev) = self.op_bar.take() {
                prev.finish();
            }
            let pb = self.mp().map(|mp| mp.add(ProgressBar::new(100)));
            if let Some(pb) = pb {
                pb.set_style(bar_style_op());
                pb.set_prefix(label_for(kind).to_string());
                pb.set_message(pkg.clone());
                self.op_bar = Some(pb);
            }
            self.last_kind = Some(kind);
            self.last_pkg = pkg;
        }
        if let Some(pb) = self.op_bar.as_ref() {
            pb.set_position(percent);
            if percent >= 100 {
                // Finish but retain last_kind/last_pkg so repeat 100% callbacks
                // are no-ops (new_bar stays false), not new bars.
                pb.finish();
                self.op_bar = None;
            }
        }
    }

    fn on_download(&mut self, file: String, downloaded: u64, total: u64) {
        if !self.enabled {
            return;
        }
        if !self.dl_bars.contains_key(&file) {
            // Init (or first Progress): a new bar with a steady tick.
            if let Some(mp) = self.mp() {
                let pb = mp.add(ProgressBar::new(0));
                pb.set_style(bar_style_dl());
                pb.set_prefix(format!("downloading {file}"));
                pb.enable_steady_tick(Duration::from_millis(120));
                self.dl_bars.insert(file.clone(), pb);
            }
        }
        if let Some(pb) = self.dl_bars.get(&file) {
            if total > 0 {
                pb.set_length(total);
            }
            pb.set_position(downloaded);
        }
    }

    fn on_download_done(&mut self, file: String, total: u64) {
        if !self.enabled {
            return;
        }
        if let Some(pb) = self.dl_bars.remove(&file) {
            if total > 0 {
                pb.set_length(total);
                pb.set_position(total);
            }
            pb.finish();
        }
    }

    fn on_job_begin(&mut self, label: String) {
        if !self.enabled {
            return;
        }
        // The images job bar does NOT go through MultiProgress (the old
        // job::wait created a standalone ProgressBar); keep it standalone so the
        // rendering is byte-identical.
        let b = ProgressBar::new(100).with_style(bar_style_job());
        b.set_prefix(label);
        b.enable_steady_tick(Duration::from_millis(120));
        self.job_bar = Some(b);
    }

    fn on_job(&mut self, percent: u64) {
        if let Some(b) = self.job_bar.as_ref() {
            b.set_position(percent);
        }
    }

    fn on_job_end(&mut self) {
        if let Some(b) = self.job_bar.take() {
            b.finish_and_clear();
        }
    }
}

impl ProgressSink for IndicatifSink {
    fn handle(&mut self, ev: ProgressEvent) {
        match ev {
            ProgressEvent::Status(text) => self.on_status(text),
            ProgressEvent::Op { kind, pkg, percent } => self.on_op(kind, pkg, percent),
            ProgressEvent::Download {
                file,
                downloaded,
                total,
            } => self.on_download(file, downloaded, total),
            ProgressEvent::DownloadDone { file, total } => self.on_download_done(file, total),
            ProgressEvent::JobBegin { label } => self.on_job_begin(label),
            ProgressEvent::Job { percent } => self.on_job(percent),
            ProgressEvent::JobEnd => self.on_job_end(),
        }
    }

    fn begin(&mut self) {
        // Re-anchor the MultiProgress after any preceding summary/confirm output
        // so bar redraws don't clear the wrong lines (terminal-jump fix).
        if self.enabled {
            self.mp = Some(MultiProgress::new());
        }
    }
}
