//! Neutral per-verb parameter structs (no clap).
//!
//! Mirror the fields of the corresponding `cli::args::<verb>::Args` exactly.
//! Operations take these instead of the clap types; the bin converts
//! `cli::args::<verb>::Args -> params::<verb>::Params` (see main.rs).
//! This module must NOT depend on clap.

use std::path::PathBuf;

/// Inputs `config::build_with_dbext` reads from the clap `Cli`.
pub struct ContextParams {
    pub config: Option<PathBuf>,
    pub root: Option<PathBuf>,
    pub dbpath: Option<PathBuf>,
}

pub mod database {
    pub struct Params {
        pub asdeps: bool,
        pub asexplicit: bool,
        pub check: u8,
        pub quiet: bool,
        pub packages: Vec<String>,
    }
}

pub mod query {
    pub struct Params {
        pub changelog: bool,
        pub deps: bool,
        pub explicit: bool,
        pub groups: bool,
        pub info: u8,
        pub check: u8,
        pub list: bool,
        pub foreign: bool,
        pub native: bool,
        pub owns: Option<String>,
        pub file: Option<std::path::PathBuf>,
        pub quiet: bool,
        pub search: Option<String>,
        pub unrequired: bool,
        pub upgrades: bool,
        pub packages: Vec<String>,
    }
}

pub mod remove {
    pub struct Params {
        pub cascade: bool,
        pub nodeps: u8,
        pub nosave: bool,
        pub print: bool,
        pub print_format: Option<String>,
        pub recursive: u8,
        pub unneeded: bool,
        pub assume_installed: Vec<String>,
        pub dbonly: bool,
        pub noconfirm: bool,
        pub noprogressbar: bool,
        pub noscriptlet: bool,
        pub packages: Vec<String>,
    }
}

pub mod sync {
    pub struct Params {
        pub clean: u8,
        pub groups: bool,
        pub info: u8,
        pub list: bool,
        pub print: bool,
        pub print_format: Option<String>,
        pub quiet: bool,
        pub search: Option<String>,
        pub sysupgrade: u8,
        pub downloadonly: bool,
        pub refresh: u8,
        pub needed: bool,
        pub asdeps: bool,
        pub asexplicit: bool,
        pub ignore: Vec<String>,
        pub ignoregroup: Vec<String>,
        pub overwrite: Vec<String>,
        pub noconfirm: bool,
        pub noprogressbar: bool,
        pub noscriptlet: bool,
        pub targets: Vec<String>,
    }
}

pub mod deptest {
    pub struct Params {
        pub deps: Vec<String>,
    }
}

pub mod upgrade {
    use super::PathBuf;

    pub struct Params {
        pub print: bool,
        pub print_format: Option<String>,
        pub nodeps: u8,
        pub asdeps: bool,
        pub asexplicit: bool,
        pub overwrite: Vec<String>,
        pub needed: bool,
        pub dbonly: bool,
        pub noscriptlet: bool,
        pub noconfirm: bool,
        pub noprogressbar: bool,
        pub files: Vec<PathBuf>,
    }
}

pub mod files {
    pub struct Params {
        pub refresh: u8,
        pub list: bool,
        // Parsed by clap (default-mode toggle) but not read by the op; kept for
        // field-exact parity with cli::args::files::Args.
        #[allow(dead_code)]
        pub search: bool,
        pub regex: bool,
        pub quiet: bool,
        pub machinereadable: bool,
        pub targets: Vec<String>,
    }
}

pub mod images {
    pub struct Params {
        pub list: bool,
        pub info: u8,
        pub check_new: bool,
        pub upgrade: u8,
        pub clean: u8,
        pub pending: bool,
        pub reboot: bool,
        pub components: bool,
        pub features: bool,
        pub enable: Option<String>,
        pub disable: Option<String>,
        pub appstream: bool,
        pub offline: bool,
        pub reinstall_layered: bool,
        pub dry_run: bool,
        pub quiet: bool,
        pub noconfirm: bool,
        pub noprogressbar: bool,
        pub json: Option<String>,
        pub targets: Vec<String>,
    }
}
