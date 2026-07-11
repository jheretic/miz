//! Renders an [`UpgradeReport`]. See `render::remove`: only `--print` defers
//! output and its warning was uncolored inline.

use crate::common::report::UpgradeReport;

pub fn render(report: &UpgradeReport) {
    match report {
        UpgradeReport::Print {
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
        UpgradeReport::Done => {}
    }
}
