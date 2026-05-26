use alpm::{Alpm, DownloadEvent, Event, Progress};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::time::Duration;

pub fn install(alpm: &Alpm, noprogressbar: bool) {
    if noprogressbar || !std::io::stderr().is_terminal() {
        return;
    }

    let mp = MultiProgress::new();
    install_event_cb(alpm, mp.clone());
    install_progress_cb(alpm, mp.clone());
    install_dl_cb(alpm, mp);
}

fn bar_style_op() -> ProgressStyle {
    ProgressStyle::with_template("{prefix:>15} {bar:30} {percent:>3}% {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("##-")
}

fn bar_style_dl() -> ProgressStyle {
    ProgressStyle::with_template(
        "{prefix:>30} {bytes:>10}/{total_bytes:<10} {bar:30} {percent:>3}% {bytes_per_sec}",
    )
    .unwrap_or_else(|_| ProgressStyle::default_bar())
    .progress_chars("##-")
}

fn install_event_cb(alpm: &Alpm, mp: MultiProgress) {
    alpm.set_event_cb(mp, |event, mp| match event.event() {
        Event::CheckDepsStart => {
            let _ = mp.println(":: checking dependencies...");
        }
        Event::ResolveDepsStart => {
            let _ = mp.println(":: resolving dependencies...");
        }
        Event::InterConflictsStart => {
            let _ = mp.println(":: looking for conflicting packages...");
        }
        Event::FileConflictsStart => {
            let _ = mp.println(":: checking for file conflicts...");
        }
        Event::IntegrityStart => {
            let _ = mp.println(":: checking package integrity...");
        }
        Event::LoadStart => {
            let _ = mp.println(":: loading package files...");
        }
        Event::KeyringStart => {
            let _ = mp.println(":: checking keyring...");
        }
        Event::TransactionStart => {
            let _ = mp.println(":: processing transaction...");
        }
        Event::TransactionDone => {
            let _ = mp.println(":: transaction done");
        }
        Event::HookRunStart(h) => {
            let name = h.name();
            if !name.is_empty() {
                let _ = mp.println(format!(":: running hook {name}"));
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
            let new_bar = st.last_kind != Some(kind) || st.last_pkg != pkg;
            if new_bar {
                if let Some(prev) = st.bar.take() {
                    prev.finish_and_clear();
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
                    pb.finish();
                    st.bar = None;
                    st.last_kind = None;
                    st.last_pkg.clear();
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
                pb.set_prefix(filename.to_string());
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
