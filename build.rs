//! Build-time manpage generation.
//!
//! Reuses the real `Cli` Command tree from `src/cli/` via `#[path]` so the
//! generated `miz.1` reflects every flag of every subcommand. The `cli`
//! module is alpm-free (Args structs live in `src/cli/args.rs`); the
//! alpm-using `operations::*::run()` impls are not pulled in here.

use clap::CommandFactory;
use std::env;
use std::fs;
use std::path::PathBuf;

#[path = "src/cli/mod.rs"]
mod cli;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli/mod.rs");
    println!("cargo:rerun-if-changed=src/cli/args.rs");

    let out_dir = match env::var_os("OUT_DIR") {
        Some(v) => PathBuf::from(v),
        None => return,
    };

    let cmd = cli::Cli::command();
    let man = clap_mangen::Man::new(cmd.clone());
    let mut buf = Vec::new();
    if man.render(&mut buf).is_ok() {
        let _ = fs::write(out_dir.join("miz.1"), buf);
    }

    // Also render a man page for each subcommand (man miz-sync, etc.).
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        let name: &'static str = Box::leak(format!("miz-{}", sub.get_name()).into_boxed_str());
        let sub_cmd = sub.clone().name(name);
        let man = clap_mangen::Man::new(sub_cmd);
        let mut buf = Vec::new();
        if man.render(&mut buf).is_ok() {
            let _ = fs::write(out_dir.join(format!("{name}.1")), buf);
        }
    }
}
