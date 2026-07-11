mod cli;
mod common;
mod config;
mod error;
mod operations;
mod render;

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
    // build_with_dbext returns the raw color POLICY (config's `color` flag); the
    // Palette (presentation) is built HERE in the render layer, so the
    // libalpm-linked config module carries no render/console dependency. The
    // palette is threaded into the operations instead of living on Context.
    let (mut ctx, palette) = if needs_context {
        let (c, color) = config::build_with_dbext(&cli, dbext)?;
        (Some(c), Some(render::palette::Palette::resolve(color)))
    } else {
        (None, None)
    };
    let config_path = cli.config.clone();
    // Construct the confirm + progress seam implementations for the committing
    // verbs. The sink is shared (Rc<RefCell>) because libalpm's callback
    // registration stores 'static closures; core clones the handle into each.
    let make_seams = |noconfirm: bool, noprogressbar: bool| {
        let pal = palette.clone().unwrap_or_else(|| render::palette::Palette::resolve(true));
        let confirmer = render::confirm::TtyConfirmer::new(pal.clone(), noconfirm);
        let sink: common::progress::SharedSink =
            std::rc::Rc::new(std::cell::RefCell::new(render::progress_indicatif::IndicatifSink::new(
                !noprogressbar,
                &pal,
            )));
        (confirmer, sink)
    };
    match cli.op {
        Operation::Database(args) => {
            let report = operations::database::run(args, ctx.as_ref().unwrap())?;
            render::database::render(&report);
            report.outcome()
        }
        Operation::Query(args) => {
            let report = operations::query::run(args, ctx.as_ref().unwrap())?;
            render::query::render(&report);
            report.outcome()
        }
        Operation::Remove(args) => {
            let (mut confirmer, sink) = make_seams(args.noconfirm, args.noprogressbar);
            let report = operations::remove::run(args, ctx.as_mut().unwrap(), &mut confirmer, &sink)?;
            render::remove::render(&report);
            report.outcome()
        }
        Operation::Sync(args) => {
            let (mut confirmer, sink) = make_seams(args.noconfirm, args.noprogressbar);
            let pal = palette.clone().unwrap();
            let report = operations::sync::run(args, ctx.as_mut().unwrap(), &mut confirmer, &sink)?;
            render::sync::render(&report, &pal);
            report.outcome()
        }
        Operation::Deptest(args) => {
            let report = operations::deptest::run(args, ctx.as_ref().unwrap())?;
            render::deptest::render(&report);
            report.outcome()
        }
        Operation::Upgrade(args) => {
            let (mut confirmer, sink) = make_seams(args.noconfirm, args.noprogressbar);
            let report = operations::upgrade::run(args, ctx.as_mut().unwrap(), &mut confirmer, &sink)?;
            render::upgrade::render(&report);
            report.outcome()
        }
        Operation::Files(args) => {
            let report = operations::files::run(args, ctx.as_mut().unwrap())?;
            render::files::render(&report);
            report.outcome()
        }
        Operation::Version => {
            let report = operations::version::run()?;
            render::version::render(&report);
            Ok(())
        }
        Operation::Images(args) => {
            let (mut confirmer, sink) = make_seams(args.noconfirm, args.noprogressbar);
            let report =
                operations::images::run(args, config_path.as_deref(), &mut confirmer, &sink)?;
            render::images::render(&report);
            // Deferred reboot (`-Iu --reboot`): render printed the upgrade/relay
            // lines first, now trigger the reboot before returning outcome.
            if report.wants_reboot() {
                operations::images::reboot()?;
            }
            report.outcome()
        }
        Operation::Completions { shell } => render::completions::run(shell),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    // Install soft-interrupt handler before dispatch so any libalpm
    // transaction started during the run can be unwound cleanly on
    // SIGINT / SIGTERM / SIGHUP (prevents leaking /var/lib/pacman/db.lck).
    if let Err(e) = common::transaction::install_signal_handler() {
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
                let palette = render::palette::Palette::resolve(true);
                eprintln!("{} {e}", palette.error.apply_to("error:"));
            }
            ExitCode::from(e.exit_code() as u8)
        }
    }
}
