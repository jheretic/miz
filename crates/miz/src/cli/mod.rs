pub mod args;

use clap::{Parser, Subcommand};
use clap_complete::Shell;

#[derive(Parser)]
#[command(
    name = "miz",
    about = "Archetype package manager",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Override miz.toml path (default: /etc/miz.toml)
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<std::path::PathBuf>,

    /// Override root path (default: /)
    #[arg(short = 'r', long, global = true, value_name = "PATH")]
    pub root: Option<std::path::PathBuf>,

    /// Override database path (default: /var/lib/pacman)
    #[arg(short = 'b', long, global = true, value_name = "PATH")]
    pub dbpath: Option<std::path::PathBuf>,

    /// Increase verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub op: Operation,
}

#[derive(Subcommand)]
pub enum Operation {
    #[command(
        short_flag = 'D',
        long_flag = "database",
        about = "Operate on the package database"
    )]
    Database(args::database::Args),

    #[command(
        short_flag = 'Q',
        long_flag = "query",
        about = "Query the local package database"
    )]
    Query(args::query::Args),

    #[command(short_flag = 'R', long_flag = "remove", about = "Remove packages")]
    Remove(args::remove::Args),

    #[command(short_flag = 'S', long_flag = "sync", about = "Synchronize packages")]
    Sync(args::sync::Args),

    #[command(short_flag = 'T', long_flag = "deptest", about = "Check dependencies")]
    Deptest(args::deptest::Args),

    #[command(
        short_flag = 'U',
        long_flag = "upgrade",
        about = "Upgrade or add a local package"
    )]
    Upgrade(args::upgrade::Args),

    #[command(
        short_flag = 'F',
        long_flag = "files",
        about = "Query the files database"
    )]
    Files(args::files::Args),

    #[command(
        short_flag = 'V',
        long_flag = "version",
        about = "Display version and exit"
    )]
    Version,

    #[command(
        short_flag = 'I',
        long_flag = "images",
        about = "Operate on Archetype system images (miz extension)"
    )]
    Images(args::images::Args),

    #[command(hide = true, about = "Generate shell completions")]
    Completions { shell: Shell },
}
