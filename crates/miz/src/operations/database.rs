use crate::common::report::DbReport;
use crate::config::Context;
use crate::error::{MizError, Result};
use alpm::{Package, PackageReason};

pub use crate::cli::args::database::Args;

pub fn run(args: Args, ctx: &Context) -> Result<DbReport> {
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
) -> Result<DbReport> {
    let db = ctx.alpm.localdb();
    let mut confirmations = Vec::new();
    for name in names {
        let pkg = match db.pkg(name.as_bytes()) {
            Ok(p) => p,
            Err(_) => {
                return Ok(DbReport::SetReason {
                    confirmations,
                    not_found: Some(name.clone()),
                    set_reason_error: None,
                });
            }
        };
        // A set_reason failure after earlier successes must not discard the
        // confirmations already gathered: carry it terminally so render prints
        // them first, matching the original per-package immediate print.
        if let Err(e) = pkg.set_reason(reason) {
            return Ok(DbReport::SetReason {
                confirmations,
                not_found: None,
                set_reason_error: Some(e),
            });
        }
        if !quiet {
            let label = match reason {
                PackageReason::Depend => "dependency",
                PackageReason::Explicit => "explicitly installed",
            };
            confirmations.push(format!("{name}: install reason has been set to '{label}'"));
        }
    }
    Ok(DbReport::SetReason {
        confirmations,
        not_found: None,
        set_reason_error: None,
    })
}

fn run_check(args: &Args, ctx: &Context) -> Result<DbReport> {
    let (problems, count) = if args.check >= 2 {
        check_sync(ctx)
    } else {
        check_local(ctx)
    };
    Ok(DbReport::Check {
        problems,
        count,
        quiet: args.quiet,
    })
}

fn check_local(ctx: &Context) -> (Vec<String>, usize) {
    let pkgs: Vec<&Package> = ctx.alpm.localdb().pkgs().iter().collect();
    let mut problems = Vec::new();
    collect_missing(ctx, &pkgs, &mut problems);
    collect_conflicts(ctx, &pkgs, &mut problems);
    let count = problems.len();
    (problems, count)
}

fn check_sync(ctx: &Context) -> (Vec<String>, usize) {
    let mut all: Vec<&Package> = Vec::new();
    for db in ctx.alpm.syncdbs() {
        for pkg in db.pkgs() {
            all.push(pkg);
        }
    }
    let mut problems = Vec::new();
    collect_missing(ctx, &all, &mut problems);
    let count = problems.len();
    (problems, count)
}

fn collect_missing(ctx: &Context, pkgs: &[&Package], problems: &mut Vec<String>) {
    let missing = ctx.alpm.check_deps(
        pkgs.iter(),
        alpm::AlpmListMut::<&alpm::Pkg>::new(),
        alpm::AlpmListMut::<&alpm::Pkg>::new(),
        false,
    );
    for m in missing.iter() {
        problems.push(format!(
            "error: missing '{}' dependency for '{}'",
            m.depend(),
            m.target()
        ));
    }
}

fn collect_conflicts(ctx: &Context, pkgs: &[&Package], problems: &mut Vec<String>) {
    let conflicts = ctx.alpm.check_conflicts(pkgs.iter());
    for c in conflicts.iter() {
        problems.push(format!(
            "error: '{}' conflicts with '{}'",
            c.package1().name(),
            c.package2().name()
        ));
    }
}
