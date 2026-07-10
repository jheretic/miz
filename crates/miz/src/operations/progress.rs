use crate::style::Palette;
use alpm::{Alpm, DownloadEvent, Event, Progress};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::time::Duration;

pub fn install(alpm: &Alpm, noprogressbar: bool, palette: &Palette) {
    if noprogressbar || !std::io::stderr().is_terminal() {
        return;
    }

    let mp = MultiProgress::new();
    install_event_cb(alpm, mp.clone(), palette.clone());
    install_progress_cb(alpm, mp.clone());
    install_dl_cb(alpm, mp);
}

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

fn install_event_cb(alpm: &Alpm, mp: MultiProgress, palette: Palette) {
    // The status callback runs on the alpm side; `mp.println` interleaves the
    // line above the live bars without disturbing them. The `::` marker is
    // colorized to match the db-sync status line in sync.rs.
    let status = move |mp: &MultiProgress, rest: &str| {
        let _ = mp.println(format!("{} {rest}", palette.status.apply_to("::")));
    };
    alpm.set_event_cb(mp, move |event, mp| match event.event() {
        Event::CheckDepsStart => status(mp, "checking dependencies..."),
        Event::ResolveDepsStart => status(mp, "resolving dependencies..."),
        Event::InterConflictsStart => status(mp, "looking for conflicting packages..."),
        Event::FileConflictsStart => status(mp, "checking for file conflicts..."),
        Event::IntegrityStart => status(mp, "checking package integrity..."),
        Event::LoadStart => status(mp, "loading package files..."),
        Event::KeyringStart => status(mp, "checking keyring..."),
        Event::TransactionStart => status(mp, "processing transaction..."),
        Event::TransactionDone => status(mp, "transaction done"),
        Event::HookRunStart(h) => {
            let name = h.name();
            if !name.is_empty() {
                status(mp, &format!("running hook {name}"));
            }
        }
        _ => {}
    });
}

struct OpState {
    bar: Option<ProgressBar>,
    last_kind: Option<Progress>,
    last_pkg: String,
    mp: MultiProgress,
}

fn install_progress_cb(alpm: &Alpm, mp: MultiProgress) {
    let state = OpState {
        bar: None,
        last_kind: None,
        last_pkg: String::new(),
        mp,
    };
    alpm.set_progress_cb(
        state,
        |kind, pkg, percent, _n, _current, st: &mut OpState| {
            let percent = percent.clamp(0, 100) as u64;
            // A genuinely new bar is needed only when the (kind, pkg) pair
            // changes. alpm fires this callback repeatedly for one package --
            // including several times at 100% -- so `last_kind`/`last_pkg` must
            // survive completion: clearing them on 100% made every extra 100%
            // call look "new" and spawn a fresh (instantly-finished) bar, which
            // is the duplicate-bars-per-package bug and the display churn.
            let new_bar = st.last_kind != Some(kind) || st.last_pkg != pkg;
            if new_bar {
                if let Some(prev) = st.bar.take() {
                    prev.finish();
                }
                let pb = st.mp.add(ProgressBar::new(100));
                pb.set_style(bar_style_op());
                pb.set_prefix(label_for(kind).to_string());
                pb.set_message(pkg.to_string());
                st.bar = Some(pb);
                st.last_kind = Some(kind);
                st.last_pkg = pkg.to_string();
            }
            if let Some(pb) = st.bar.as_ref() {
                pb.set_position(percent);
                if percent >= 100 {
                    // Finish the bar but retain last_kind/last_pkg so repeat
                    // 100% callbacks for this same package are no-ops (new_bar
                    // stays false), not new bars.
                    pb.finish();
                    st.bar = None;
                }
            }
        },
    );
}

fn label_for(kind: Progress) -> &'static str {
    match kind {
        Progress::AddStart => "installing",
        Progress::UpgradeStart => "upgrading",
        Progress::DowngradeStart => "downgrading",
        Progress::ReinstallStart => "reinstalling",
        Progress::RemoveStart => "removing",
        Progress::ConflictsStart => "conflicts",
        Progress::DiskspaceStart => "diskspace",
        Progress::IntegrityStart => "integrity",
        Progress::LoadStart => "loading",
        Progress::KeyringStart => "keyring",
    }
}

struct DlState {
    bars: HashMap<String, ProgressBar>,
    mp: MultiProgress,
}

fn install_dl_cb(alpm: &Alpm, mp: MultiProgress) {
    let state = DlState {
        bars: HashMap::new(),
        mp,
    };
    alpm.set_dl_cb(state, |filename, event, st: &mut DlState| {
        match event.event() {
            DownloadEvent::Init(_) => {
                let pb = st.mp.add(ProgressBar::new(0));
                pb.set_style(bar_style_dl());
                pb.set_prefix(format!("downloading {filename}"));
                pb.enable_steady_tick(Duration::from_millis(120));
                st.bars.insert(filename.to_string(), pb);
            }
            DownloadEvent::Progress(p) => {
                if let Some(pb) = st.bars.get(filename) {
                    if p.total > 0 {
                        pb.set_length(p.total as u64);
                    }
                    pb.set_position(p.downloaded.max(0) as u64);
                }
            }
            DownloadEvent::Retry(_) => {
                if let Some(pb) = st.bars.get(filename) {
                    pb.set_position(0);
                }
            }
            DownloadEvent::Completed(c) => {
                if let Some(pb) = st.bars.remove(filename) {
                    if c.total > 0 {
                        pb.set_length(c.total as u64);
                        pb.set_position(c.total as u64);
                    }
                    pb.finish();
                }
            }
        }
    });
}
