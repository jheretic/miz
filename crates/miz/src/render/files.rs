//! Renders a [`FilesReport`] byte-for-byte like the inline files printing.
//! Uncolored today, so no palette threaded.

use crate::common::report::{FilePkg, FilesReport};
use std::io::Write;

pub fn render(report: &FilesReport) {
    match report {
        FilesReport::List {
            machine,
            quiet,
            pkgs,
            diagnostics,
            ..
        } => {
            for p in pkgs {
                if *machine {
                    for f in &p.files {
                        print_machinereadable(&p.db, &p.pkg, &p.version, f);
                    }
                    continue;
                }
                for f in &p.files {
                    let name = String::from_utf8_lossy(f);
                    if *quiet {
                        println!("{name}");
                    } else {
                        println!("{} {}", p.pkg, name);
                    }
                }
            }
            for line in diagnostics {
                eprintln!("{line}");
            }
        }
        FilesReport::Search {
            machine,
            quiet,
            matches,
            ..
        } => {
            for p in matches {
                print_match(*machine, *quiet, p);
            }
        }
    }
}

fn print_match(machine: bool, quiet: bool, p: &FilePkg) {
    if machine {
        for f in &p.files {
            print_machinereadable(&p.db, &p.pkg, &p.version, f);
        }
        return;
    }
    if quiet {
        println!("{}/{}", p.db, p.pkg);
        return;
    }
    if p.exact_file {
        for f in &p.files {
            let name = String::from_utf8_lossy(f);
            println!("{} is owned by {}/{} {}", name, p.db, p.pkg, p.version);
        }
        return;
    }
    println!("{}/{} {}", p.db, p.pkg, p.version);
    for f in &p.files {
        println!("    {}", String::from_utf8_lossy(f));
    }
}

fn print_machinereadable(db: &str, pkg: &str, version: &str, filename: &[u8]) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(db.as_bytes());
    let _ = out.write_all(&[0]);
    let _ = out.write_all(pkg.as_bytes());
    let _ = out.write_all(&[0]);
    let _ = out.write_all(version.as_bytes());
    let _ = out.write_all(&[0]);
    let _ = out.write_all(filename);
    let _ = out.write_all(b"\n");
}
