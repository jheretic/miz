//! Renders a [`RemoveReport`]. Only `--print` defers output today; the
//! `trans_release` warning was uncolored in the inline code, so no palette.

use miz_core::common::report::RemoveReport;

pub fn render(report: &RemoveReport) {
    match report {
        RemoveReport::Print {
            lines,
            release_warning,
        } => {
            for line in lines {
                println!("{line}");
            }
            if let Some(w) = release_warning {
                eprintln!("warning: {w}");
            }
        }
        RemoveReport::Done => {}
    }
}
