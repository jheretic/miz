//! Renders a [`SyncReport`] byte-for-byte like the inline sync printing. The
//! refresh header/footer are NOT here (they go through the progress sink so
//! they interleave with the download bars); this covers the deferred stdout
//! body plus the colorized error/warning diagnostics.

use crate::render::palette::Palette;
use miz_core::common::report::SyncReport;

pub fn render(report: &SyncReport, palette: &Palette) {
    match report {
        SyncReport::Search { lines } => {
            for line in lines {
                println!("{line}");
            }
        }
        SyncReport::Listing {
            lines, diagnostics, ..
        } => {
            for line in lines {
                println!("{line}");
            }
            for d in diagnostics {
                eprintln!("{} {d}", palette.error.apply_to("error:"));
            }
        }
        SyncReport::Clean { removed } => {
            eprintln!("removed {removed} package file(s) from cache");
        }
        SyncReport::Print {
            lines,
            release_warning,
        } => {
            for line in lines {
                println!("{line}");
            }
            if let Some(w) = release_warning {
                eprintln!("{} {w}", palette.warning.apply_to("warning:"));
            }
        }
        SyncReport::Done => {}
    }
}
