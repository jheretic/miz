//! Renders an [`ImagesReport`] byte-for-byte like the old inline `-I` printing.
//! Images output has never been colorized, so (like `render::query`) no palette
//! is threaded. The `{component} {version}{marker}` list line and the `class
//! name` component line are formatted here (presentation), while the label
//! fields for `-Ii`/`-If` arrive pre-built as `InfoField`s from core.

use miz_core::common::report::{
    ImageListRow, ImageUpgradeOutcome, ImagesReport, InfoField, RelayReport,
};

/// One `-Il` row: `{component} {version}` with `[installed]`/`[newest]` markers.
/// `quiet` prints the bare version string (like `-Slq`).
fn list_line(component: &str, row: &ImageListRow, quiet: bool) -> String {
    if quiet {
        return row.version.clone();
    }
    let mut suffix = String::new();
    if row.installed {
        suffix.push_str(" [installed]");
    }
    if row.newest {
        suffix.push_str(" [newest]");
    }
    format!("{component} {}{suffix}", row.version)
}

/// One `-Ig` row: `class name` (or bare `name` when quiet).
fn component_line(class: &str, name: &str, quiet: bool) -> String {
    if quiet {
        name.to_string()
    } else {
        format!("{class} {name}")
    }
}

/// Emit the `{:<19}: {}` label block for `-Ii`/`-If`, followed by a trailing
/// blank line (the old `print!(info_block); println!()`).
fn render_fields(fields: &[InfoField]) {
    for field in fields {
        match field {
            InfoField::Label { key, value } => println!("{key:<19}: {value}"),
            InfoField::Backup(lines) => {
                println!("Backup Files       :");
                for line in lines {
                    println!("{line}");
                }
            }
        }
    }
    println!();
}

fn render_relay(relay: &RelayReport) {
    match relay {
        RelayReport::DryRun(plan) => print!("{plan}"),
        RelayReport::Relayed { subvol, quiet } => {
            if !quiet {
                println!("relayed layered packages onto {subvol}");
            }
        }
    }
}

pub fn render(report: &ImagesReport) {
    match report {
        ImagesReport::List {
            component,
            quiet,
            rows,
        } => {
            for row in rows {
                println!("{}", list_line(component, row, *quiet));
            }
        }
        ImagesReport::Json(payload) => println!("{payload}"),
        ImagesReport::Info(fields) => render_fields(fields),
        ImagesReport::CheckNew {
            name,
            quiet,
            newest,
        } => match newest {
            None => {
                if !quiet {
                    eprintln!("{name}: no newer version available");
                }
            }
            Some(v) => {
                if *quiet {
                    println!("{v}");
                } else {
                    println!("{name}: {v} available");
                }
            }
        },
        ImagesReport::Components { quiet, rows } => {
            for (class, name) in rows {
                println!("{}", component_line(class, name, *quiet));
            }
        }
        ImagesReport::Pending {
            name,
            quiet,
            installed,
            booted_label,
            installed_label,
            reboot_due,
        } => {
            if *reboot_due {
                if *quiet {
                    println!("{installed}");
                } else {
                    println!(
                        "{name}: reboot pending: booted {booted_label}, installed {installed_label}"
                    );
                }
            } else if !quiet {
                // No reboot due -> status note to stderr, matching -Iy's stream.
                eprintln!("{name}: no reboot pending (booted {booted_label})");
            }
        }
        ImagesReport::FeatureList(features) => {
            for feature in features {
                println!("{feature}");
            }
        }
        ImagesReport::FeatureToggle {
            name,
            feature,
            enabled,
            quiet,
        } => {
            if !quiet {
                if *enabled {
                    println!("{name}: enabled feature {feature} (run -Iu to apply)");
                } else {
                    println!("{name}: disabled feature {feature} (run -Iu to apply)");
                }
            }
        }
        ImagesReport::AppStream(urls) => {
            for url in urls {
                println!("{url}");
            }
        }
        ImagesReport::Upgrade(outcome) => render_upgrade(outcome),
        ImagesReport::Vacuum {
            name,
            quiet,
            instances,
            disabled,
        } => {
            if !quiet {
                println!("{name}: removed {instances} version(s), disabled {disabled} transfer(s)");
            }
        }
        ImagesReport::Relay(relay) => render_relay(relay),
        ImagesReport::Silent => {}
    }
}

fn render_upgrade(outcome: &ImageUpgradeOutcome) {
    match outcome {
        ImageUpgradeOutcome::AlreadyUpToDate { name, quiet } => {
            if !quiet {
                eprintln!("{name}: already up to date");
            }
        }
        ImageUpgradeOutcome::Declined => {}
        ImageUpgradeOutcome::Done {
            name,
            version,
            host_changed,
            quiet,
            relay,
            ..
        } => {
            if !quiet {
                if *host_changed {
                    println!("{name}: updated to {version}");
                } else {
                    // In-place completion (e.g. a newly enabled feature): the
                    // version didn't advance, so "updated to" would mislead.
                    println!("{name}: {version} completed (in place)");
                }
            }
            if let Some(relay) = relay {
                render_relay(relay);
            }
        }
    }
}
