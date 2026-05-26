use crate::config::Context;
use crate::error::{MizError, Result};
use alpm::{Package, PackageReason};

pub use crate::cli::args::database::Args;

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    if args.check > 0 {
        return run_check(&args, ctx);
    }

    if args.asdeps || args.asexplicit {
        if args.packages.is_empty() {
            return Err(MizError::BadArgs(
                "no targets specified (use -h for help)".into(),
            ));
        }
        let reason = if args.asdeps {
            PackageReason::Depend
        } else {
            PackageReason::Explicit
        };
        return set_install_reason(ctx, &args.packages, reason, args.quiet);
    }

    Err(MizError::BadArgs(
        "no operation specified (use -h for help)".into(),
    ))
}

fn set_install_reason(
    ctx: &Context,
    names: &[String],
    reason: PackageReason,
    quiet: bool,
) -> Result<()> {
    let db = ctx.alpm.localdb();
    for name in names {
        let pkg = db
            .pkg(name.as_bytes())
            .map_err(|_| MizError::PackageNotFound(name.clone()))?;
        pkg.set_reason(reason)?;
        if !quiet {
            let label = match reason {
                PackageReason::Depend => "dependency",
                PackageReason::Explicit => "explicitly installed",
            };
            eprintln!("{name}: install reason has been set to '{label}'");
        }
    }
    Ok(())
}

fn run_check(args: &Args, ctx: &Context) -> Result<()> {
    let errors = if args.check >= 2 {
        check_sync(ctx)
    } else {
        check_local(ctx)
    };

    if errors == 0 {
        if !args.quiet {
            println!("No database errors have been found!");
        }
        Ok(())
    } else {
        Err(MizError::DatabaseErrors(errors))
    }
}

fn check_local(ctx: &Context) -> usize {
    let pkgs: Vec<&Package> = ctx.alpm.localdb().pkgs().iter().collect();
    let mut errors = 0;
    errors += report_missing(ctx, &pkgs);
    errors += report_conflicts(ctx, &pkgs);
    errors
}

fn check_sync(ctx: &Context) -> usize {
    let mut all: Vec<&Package> = Vec::new();
    for db in ctx.alpm.syncdbs() {
        for pkg in db.pkgs() {
            all.push(pkg);
        }
    }
    report_missing(ctx, &all)
}

fn report_missing(ctx: &Context, pkgs: &[&Package]) -> usize {
    let missing = ctx.alpm.check_deps(
        pkgs.iter(),
        alpm::AlpmListMut::<&alpm::Pkg>::new(),
        alpm::AlpmListMut::<&alpm::Pkg>::new(),
        false,
    );
    let mut count = 0;
    for m in missing.iter() {
        eprintln!(
            "error: missing '{}' dependency for '{}'",
            m.depend(),
            m.target()
        );
        count += 1;
    }
    count
}

fn report_conflicts(ctx: &Context, pkgs: &[&Package]) -> usize {
    let conflicts = ctx.alpm.check_conflicts(pkgs.iter());
    let mut count = 0;
    for c in conflicts.iter() {
        eprintln!(
            "error: '{}' conflicts with '{}'",
            c.package1().name(),
            c.package2().name()
        );
        count += 1;
    }
    count
}
