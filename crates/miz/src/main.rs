mod cli;
mod config;
mod error;
mod exit;
mod operations;
mod style;

use clap::Parser;
use cli::{Cli, Operation};
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn dispatch(cli: Cli) -> error::Result<()> {
    let needs_context = !matches!(
        cli.op,
        Operation::Version | Operation::Images(_) | Operation::Completions { .. }
    );
    let dbext = match &cli.op {
        Operation::Files(_) => Some(".files"),
        _ => None,
    };
    let mut ctx = if needs_context {
        Some(config::build_with_dbext(&cli, dbext)?)
    } else {
        None
    };
    let config_path = cli.config.clone();
    match cli.op {
        Operation::Database(args) => operations::database::run(args, ctx.as_ref().unwrap()),
        Operation::Query(args) => operations::query::run(args, ctx.as_ref().unwrap()),
        Operation::Remove(args) => operations::remove::run(args, ctx.as_mut().unwrap()),
        Operation::Sync(args) => operations::sync::run(args, ctx.as_mut().unwrap()),
        Operation::Deptest(args) => operations::deptest::run(args, ctx.as_ref().unwrap()),
        Operation::Upgrade(args) => operations::upgrade::run(args, ctx.as_mut().unwrap()),
        Operation::Files(args) => operations::files::run(args, ctx.as_mut().unwrap()),
        Operation::Version => operations::version::run(),
        Operation::Images(args) => operations::images::run(args, config_path.as_deref()),
        Operation::Completions { shell } => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "miz", &mut std::io::stdout());
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    // Install soft-interrupt handler before dispatch so any libalpm
    // transaction started during the run can be unwound cleanly on
    // SIGINT / SIGTERM / SIGHUP (prevents leaking /var/lib/pacman/db.lck).
    if let Err(e) = operations::transaction::install_signal_handler() {
        // Non-fatal: warn and continue. Worst case we leak the lock on signal.
        eprintln!("warning: failed to install signal handler: {e}");
    }
    match dispatch(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if !matches!(
                e,
                error::MizError::Deptest | error::MizError::DatabaseErrors(_)
            ) {
                // Errors print to stderr; resolve the palette against stderr's
                // TTY-ness. Config color isn't available on the failure path
                // (the error may BE a config-load failure), so honor NO_COLOR +
                // TTY with color defaulted on -- matching the shipped default.
                let palette = style::Palette::resolve_stderr(true);
                eprintln!("{} {e}", palette.error.apply_to("error:"));
            }
            ExitCode::from(e.exit_code() as u8)
        }
    }
}
