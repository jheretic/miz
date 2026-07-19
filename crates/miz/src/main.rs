mod cli;
mod render;

use clap::Parser;
use cli::{Cli, Operation};
use miz_core::{common, config, error, operations, params};
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

// clap Args -> neutral params conversions. Live in the bin so params.rs stays
// clap-free and cli/args.rs stays params-free (build.rs compiles cli/ alpm-free).
impl From<cli::args::database::Args> for params::database::Params {
    fn from(a: cli::args::database::Args) -> Self {
        Self {
            asdeps: a.asdeps,
            asexplicit: a.asexplicit,
            check: a.check,
            quiet: a.quiet,
            packages: a.packages,
        }
    }
}

impl From<cli::args::query::Args> for params::query::Params {
    fn from(a: cli::args::query::Args) -> Self {
        Self {
            changelog: a.changelog,
            deps: a.deps,
            explicit: a.explicit,
            groups: a.groups,
            info: a.info,
            check: a.check,
            list: a.list,
            foreign: a.foreign,
            native: a.native,
            owns: a.owns,
            file: a.file,
            quiet: a.quiet,
            search: a.search,
            unrequired: a.unrequired,
            upgrades: a.upgrades,
            packages: a.packages,
        }
    }
}

impl From<cli::args::remove::Args> for params::remove::Params {
    fn from(a: cli::args::remove::Args) -> Self {
        Self {
            cascade: a.cascade,
            nodeps: a.nodeps,
            nosave: a.nosave,
            print: a.print,
            print_format: a.print_format,
            recursive: a.recursive,
            unneeded: a.unneeded,
            assume_installed: a.assume_installed,
            dbonly: a.dbonly,
            noconfirm: a.noconfirm,
            noprogressbar: a.noprogressbar,
            noscriptlet: a.noscriptlet,
            packages: a.packages,
        }
    }
}

impl From<cli::args::sync::Args> for params::sync::Params {
    fn from(a: cli::args::sync::Args) -> Self {
        Self {
            clean: a.clean,
            groups: a.groups,
            info: a.info,
            list: a.list,
            print: a.print,
            print_format: a.print_format,
            quiet: a.quiet,
            search: a.search,
            sysupgrade: a.sysupgrade,
            downloadonly: a.downloadonly,
            refresh: a.refresh,
            needed: a.needed,
            asdeps: a.asdeps,
            asexplicit: a.asexplicit,
            ignore: a.ignore,
            ignoregroup: a.ignoregroup,
            overwrite: a.overwrite,
            noconfirm: a.noconfirm,
            noprogressbar: a.noprogressbar,
            noscriptlet: a.noscriptlet,
            targets: a.targets,
        }
    }
}

impl From<cli::args::deptest::Args> for params::deptest::Params {
    fn from(a: cli::args::deptest::Args) -> Self {
        Self { deps: a.deps }
    }
}

impl From<cli::args::upgrade::Args> for params::upgrade::Params {
    fn from(a: cli::args::upgrade::Args) -> Self {
        Self {
            print: a.print,
            print_format: a.print_format,
            nodeps: a.nodeps,
            asdeps: a.asdeps,
            asexplicit: a.asexplicit,
            overwrite: a.overwrite,
            needed: a.needed,
            dbonly: a.dbonly,
            noscriptlet: a.noscriptlet,
            noconfirm: a.noconfirm,
            noprogressbar: a.noprogressbar,
            files: a.files,
        }
    }
}

impl From<cli::args::files::Args> for params::files::Params {
    fn from(a: cli::args::files::Args) -> Self {
        Self {
            refresh: a.refresh,
            list: a.list,
            search: a.search,
            regex: a.regex,
            quiet: a.quiet,
            machinereadable: a.machinereadable,
            targets: a.targets,
        }
    }
}

impl From<cli::args::images::Args> for params::images::Params {
    fn from(a: cli::args::images::Args) -> Self {
        Self {
            list: a.list,
            info: a.info,
            check_new: a.check_new,
            upgrade: a.upgrade,
            clean: a.clean,
            pending: a.pending,
            reboot: a.reboot,
            components: a.components,
            features: a.features,
            enable: a.enable,
            disable: a.disable,
            appstream: a.appstream,
            offline: a.offline,
            reinstall_layered: a.reinstall_layered,
            dry_run: a.dry_run,
            quiet: a.quiet,
            noconfirm: a.noconfirm,
            noprogressbar: a.noprogressbar,
            json: a.json,
            targets: a.targets,
        }
    }
}

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
        let ctx_params = params::ContextParams {
            config: cli.config.clone(),
            root: cli.root.clone(),
            dbpath: cli.dbpath.clone(),
        };
        let (c, color) = config::build_with_dbext(&ctx_params, dbext)?;
        (Some(c), Some(render::palette::Palette::resolve(color)))
    } else {
        (None, None)
    };
    let config_path = cli.config.clone();
    // Construct the confirm + progress seam implementations for the committing
    // verbs. The sink is shared (Rc<RefCell>) because libalpm's callback
    // registration stores 'static closures; core clones the handle into each.
    let make_seams = |noconfirm: bool, noprogressbar: bool| {
        let pal = palette
            .clone()
            .unwrap_or_else(|| render::palette::Palette::resolve(true));
        let confirmer = render::confirm::TtyConfirmer::new(pal.clone(), noconfirm);
        let sink: common::progress::SharedSink = std::rc::Rc::new(std::cell::RefCell::new(
            render::progress_indicatif::IndicatifSink::new(!noprogressbar, &pal),
        ));
        (confirmer, sink)
    };
    match cli.op {
        Operation::Database(args) => {
            let report = operations::database::run(args.into(), ctx.as_ref().unwrap())?;
            render::database::render(&report);
            report.outcome()
        }
        Operation::Query(args) => {
            let report = operations::query::run(args.into(), ctx.as_ref().unwrap())?;
            render::query::render(&report);
            report.outcome()
        }
        Operation::Remove(args) => {
            let params: params::remove::Params = args.into();
            let (mut confirmer, sink) = make_seams(params.noconfirm, params.noprogressbar);
            let report =
                operations::remove::run(params, ctx.as_mut().unwrap(), &mut confirmer, &sink)?;
            render::remove::render(&report);
            report.outcome()
        }
        Operation::Sync(args) => {
            let params: params::sync::Params = args.into();
            let (mut confirmer, sink) = make_seams(params.noconfirm, params.noprogressbar);
            let pal = palette.clone().unwrap();
            let report =
                operations::sync::run(params, ctx.as_mut().unwrap(), &mut confirmer, &sink)?;
            render::sync::render(&report, &pal);
            report.outcome()
        }
        Operation::Deptest(args) => {
            let report = operations::deptest::run(args.into(), ctx.as_ref().unwrap())?;
            render::deptest::render(&report);
            report.outcome()
        }
        Operation::Upgrade(args) => {
            let params: params::upgrade::Params = args.into();
            let (mut confirmer, sink) = make_seams(params.noconfirm, params.noprogressbar);
            let report =
                operations::upgrade::run(params, ctx.as_mut().unwrap(), &mut confirmer, &sink)?;
            render::upgrade::render(&report);
            report.outcome()
        }
        Operation::Files(args) => {
            let report = operations::files::run(args.into(), ctx.as_mut().unwrap())?;
            render::files::render(&report);
            report.outcome()
        }
        Operation::Version => {
            let report = operations::version::run()?;
            render::version::render(&report);
            Ok(())
        }
        Operation::Images(args) => {
            let params: params::images::Params = args.into();
            let (mut confirmer, sink) = make_seams(params.noconfirm, params.noprogressbar);
            let report =
                operations::images::run(params, config_path.as_deref(), &mut confirmer, &sink)?;
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
