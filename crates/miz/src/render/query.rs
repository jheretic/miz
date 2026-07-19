//! Renders a [`QueryReport`] to byte-for-byte the same stdout/stderr the inline
//! query printing produced. Uncolored (query has never colorized), so no
//! palette is threaded.

use miz_core::common::report::{InfoField, QueryBody, QueryReport, SearchSource};

pub fn render(report: &QueryReport) {
    match &report.body {
        QueryBody::List { quiet, pkgs } => {
            for p in pkgs {
                if *quiet {
                    println!("{}", p.name);
                } else {
                    println!("{} {}", p.name, p.version);
                }
            }
        }
        QueryBody::Info(blocks) => {
            for block in blocks {
                for field in &block.fields {
                    match field {
                        InfoField::Label { key, value } => {
                            println!("{:<19}: {}", key, value);
                        }
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
        }
        QueryBody::Files { quiet, lines } => {
            for l in lines {
                if *quiet {
                    println!("{}", l.file);
                } else {
                    println!("{} {}", l.pkg, l.file);
                }
            }
        }
        QueryBody::Check(results) => {
            for r in results {
                for problem in &r.problems {
                    eprintln!("{problem}");
                }
                println!("{}", r.summary);
            }
        }
        QueryBody::Changelog(entries) => {
            for e in entries {
                print!("{e}");
            }
        }
        QueryBody::Search { quiet, hits } => {
            for hit in hits {
                if *quiet {
                    println!("{}", hit.name);
                    continue;
                }
                match hit.source {
                    SearchSource::Local => {
                        println!("local/{} {}", hit.name, hit.version);
                        println!("    {}", hit.desc);
                    }
                    SearchSource::Image => {
                        println!("image/{} {}", hit.name, hit.version);
                        if !hit.desc.is_empty() {
                            println!("    {}", hit.desc);
                        }
                    }
                }
            }
        }
        QueryBody::Owns(lines) | QueryBody::Groups(lines) => {
            for l in lines {
                println!("{l}");
            }
        }
    }

    for line in &report.diagnostics {
        eprintln!("{line}");
    }
}
