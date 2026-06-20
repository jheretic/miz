//! Per-operation argument structs.
//!
//! Pure clap-derive data with no alpm dependency. Lives in its own module
//! so `build.rs` can import the full `Cli` Command tree for `clap_mangen`
//! without dragging in `alpm-sys` (which needs `libalpm` at link time).
//!
//! The `run()` implementations stay in `crate::operations::*`.

use std::path::PathBuf;

pub mod database {
    #[derive(clap::Args)]
    pub struct Args {
        /// Mark packages as installed as a dependency
        #[arg(long, conflicts_with = "asexplicit")]
        pub asdeps: bool,
        /// Mark packages as explicitly installed
        #[arg(long)]
        pub asexplicit: bool,
        /// Check the integrity of the package database (-kk for sync dbs)
        #[arg(short = 'k', long, action = clap::ArgAction::Count)]
        pub check: u8,
        /// Suppress informational messages
        #[arg(short = 'q', long)]
        pub quiet: bool,
        pub packages: Vec<String>,
    }
}

pub mod query {
    #[derive(clap::Args)]
    pub struct Args {
        /// Print the package changelog
        #[arg(short = 'c', long)]
        pub changelog: bool,
        /// List packages installed as dependencies
        #[arg(short = 'd', long)]
        pub deps: bool,
        /// List packages installed explicitly
        #[arg(short = 'e', long)]
        pub explicit: bool,
        /// View all members of a package group, or list all groups
        #[arg(short = 'g', long)]
        pub groups: bool,
        /// View package information (-ii for backup files too)
        #[arg(short = 'i', long, action = clap::ArgAction::Count)]
        pub info: u8,
        /// Check that package files are present (-kk: also verify mode/size)
        #[arg(short = 'k', long, action = clap::ArgAction::Count)]
        pub check: u8,
        /// List the files owned by the queried package
        #[arg(short = 'l', long)]
        pub list: bool,
        /// List packages not in any sync repository
        #[arg(short = 'm', long)]
        pub foreign: bool,
        /// List packages from a sync repository
        #[arg(short = 'n', long)]
        pub native: bool,
        /// Query which package owns a file
        #[arg(short = 'o', long, value_name = "FILE")]
        pub owns: Option<String>,
        /// Query a package file instead of the local database
        #[arg(short = 'p', long, value_name = "FILE")]
        pub file: Option<std::path::PathBuf>,
        /// Suppress versions / extra detail
        #[arg(short = 'q', long)]
        pub quiet: bool,
        /// Search installed packages by regex
        #[arg(short = 's', long, value_name = "REGEX")]
        pub search: Option<String>,
        /// List packages no other package depends on
        #[arg(short = 't', long)]
        pub unrequired: bool,
        /// List packages with upgrades available in sync repos
        #[arg(short = 'u', long)]
        pub upgrades: bool,
        pub packages: Vec<String>,
    }
}

pub mod remove {
    #[derive(clap::Args)]
    pub struct Args {
        #[arg(short = 'c', long)]
        pub cascade: bool,
        #[arg(short = 'd', long, action = clap::ArgAction::Count)]
        pub nodeps: u8,
        #[arg(short = 'n', long)]
        pub nosave: bool,
        #[arg(short = 'p', long)]
        pub print: bool,
        #[arg(long, value_name = "STR")]
        pub print_format: Option<String>,
        #[arg(short = 's', long, action = clap::ArgAction::Count)]
        pub recursive: u8,
        #[arg(short = 'u', long)]
        pub unneeded: bool,
        #[arg(long)]
        pub assume_installed: Vec<String>,
        #[arg(long)]
        pub dbonly: bool,
        #[arg(long)]
        pub noconfirm: bool,
        #[arg(long)]
        pub noprogressbar: bool,
        #[arg(long)]
        pub noscriptlet: bool,
        #[arg(required = true)]
        pub packages: Vec<String>,
    }
}

pub mod sync {
    #[derive(clap::Args)]
    pub struct Args {
        /// Remove old packages from cache directory (-cc for all)
        #[arg(short = 'c', long, action = clap::ArgAction::Count)]
        pub clean: u8,
        /// View all members of a package group, or list all groups
        #[arg(short = 'g', long)]
        pub groups: bool,
        /// View package information from sync databases
        #[arg(short = 'i', long, action = clap::ArgAction::Count)]
        pub info: u8,
        /// List packages in the given sync repos (all if none given)
        #[arg(short = 'l', long)]
        pub list: bool,
        /// Print the targets instead of performing the operation
        #[arg(short = 'p', long)]
        pub print: bool,
        /// Print format string for --print
        #[arg(long, value_name = "STR")]
        pub print_format: Option<String>,
        /// Suppress versions / extra detail
        #[arg(short = 'q', long)]
        pub quiet: bool,
        /// Search sync repositories by regex
        #[arg(short = 's', long, value_name = "REGEX")]
        pub search: Option<String>,
        /// Upgrade all out-of-date packages (-uu allows downgrades)
        #[arg(short = 'u', long, action = clap::ArgAction::Count)]
        pub sysupgrade: u8,
        /// Download but do not install
        #[arg(short = 'w', long)]
        pub downloadonly: bool,
        /// Refresh package databases (-yy to force)
        #[arg(short = 'y', long, action = clap::ArgAction::Count)]
        pub refresh: u8,
        /// Skip packages already up to date
        #[arg(long)]
        pub needed: bool,
        /// Install as dependency
        #[arg(long, conflicts_with = "asexplicit")]
        pub asdeps: bool,
        /// Install as explicit
        #[arg(long)]
        pub asexplicit: bool,
        /// Ignore upgrade for a package
        #[arg(long)]
        pub ignore: Vec<String>,
        /// Ignore upgrade for all packages in a group
        #[arg(long)]
        pub ignoregroup: Vec<String>,
        /// Overwrite conflicting files (glob)
        #[arg(long)]
        pub overwrite: Vec<String>,
        /// Do not ask for confirmation
        #[arg(long)]
        pub noconfirm: bool,
        /// Suppress the progress bar
        #[arg(long)]
        pub noprogressbar: bool,
        /// Do not execute install scriptlets
        #[arg(long)]
        pub noscriptlet: bool,
        pub targets: Vec<String>,
    }
}

pub mod deptest {
    #[derive(clap::Args)]
    pub struct Args {
        pub deps: Vec<String>,
    }
}

pub mod upgrade {
    use super::PathBuf;

    #[derive(clap::Args)]
    pub struct Args {
        #[arg(short = 'p', long)]
        pub print: bool,
        #[arg(long, value_name = "STR")]
        pub print_format: Option<String>,
        #[arg(short = 'd', long, action = clap::ArgAction::Count)]
        pub nodeps: u8,
        #[arg(long, conflicts_with = "asexplicit")]
        pub asdeps: bool,
        #[arg(long)]
        pub asexplicit: bool,
        #[arg(long)]
        pub overwrite: Vec<String>,
        #[arg(long)]
        pub needed: bool,
        #[arg(long)]
        pub dbonly: bool,
        #[arg(long)]
        pub noscriptlet: bool,
        #[arg(long)]
        pub noconfirm: bool,
        #[arg(long)]
        pub noprogressbar: bool,
        #[arg(required = true)]
        pub files: Vec<PathBuf>,
    }
}

pub mod files {
    #[derive(clap::Args)]
    pub struct Args {
        /// Refresh the files databases (-yy to force)
        #[arg(short = 'y', long, action = clap::ArgAction::Count)]
        pub refresh: u8,
        /// List files owned by the given package(s)
        #[arg(short = 'l', long)]
        pub list: bool,
        /// Search mode (default when targets are given)
        #[arg(short = 's', long)]
        pub search: bool,
        /// Interpret search targets as regular expressions
        #[arg(short = 'x', long)]
        pub regex: bool,
        /// Suppress versions / extra detail
        #[arg(short = 'q', long)]
        pub quiet: bool,
        /// Print results as NUL-separated repo, name, version, filename
        #[arg(long)]
        pub machinereadable: bool,
        pub targets: Vec<String>,
    }
}

pub mod images {
    #[derive(clap::Args)]
    pub struct Args {
        /// List available versions for a component (like -Sl)
        #[arg(short = 'l', long)]
        pub list: bool,
        /// Show component/version info (-ii for changelog/contents)
        #[arg(short = 'i', long, action = clap::ArgAction::Count)]
        pub info: u8,
        /// Check for a newer version without downloading
        #[arg(short = 'y', long = "check-new")]
        pub check_new: bool,
        /// Update to the newest (or pinned) version
        #[arg(short = 'u', long, action = clap::ArgAction::Count)]
        pub upgrade: u8,
        /// Remove obsolete downloaded versions (vacuum)
        #[arg(short = 'c', long, action = clap::ArgAction::Count)]
        pub clean: u8,
        /// Report whether an update is staged/pending
        #[arg(short = 'p', long)]
        pub pending: bool,
        /// Reboot into the updated image (also usable as a modifier on -Iu).
        /// Long-only: -b is the global --dbpath short flag.
        #[arg(long)]
        pub reboot: bool,
        /// List components (like package groups)
        #[arg(short = 'g', long)]
        pub components: bool,
        /// Manage optional features
        #[arg(short = 'f', long)]
        pub features: bool,
        /// Operate on installed versions only (no network)
        #[arg(long)]
        pub offline: bool,
        /// Suppress markers / extra detail
        #[arg(short = 'q', long)]
        pub quiet: bool,
        /// Do not ask for confirmation
        #[arg(long)]
        pub noconfirm: bool,
        /// Suppress the progress bar
        #[arg(long)]
        pub noprogressbar: bool,
        /// Print raw Describe JSON instead of pacman-style rendering
        #[arg(long, value_name = "MODE")]
        pub json: Option<String>,
        /// Component, or component/version
        pub targets: Vec<String>,
    }
}
