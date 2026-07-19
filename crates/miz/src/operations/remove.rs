use crate::common::progress::SharedSink;
use crate::common::report::{Confirmer, RemoveReport, TransactionKind, TransactionPlan};
use crate::common::transaction::{
    collect_pkgs, commit, format_print_line, prepare, TransGuard,
};
use crate::config::Context;
use crate::error::{MizError, Result};
use alpm::{Alpm, Depend, Package, TransFlag};

use crate::params::remove::Params as Args;

pub fn run(
    args: Args,
    ctx: &mut Context,
    confirmer: &mut dyn Confirmer,
    sink: &SharedSink,
) -> Result<RemoveReport> {
    let flags = build_flags(&args);
    apply_assume_installed(&mut ctx.alpm, &args.assume_installed)?;

    if args.print {
        return run_print(&args, ctx, flags);
    }

    let mut guard = TransGuard::new(&mut ctx.alpm, flags)?;
    add_targets(guard.alpm(), &args.packages)?;
    prepare(guard.alpm())?;

    let targets = collect_pkgs(guard.alpm().trans_remove());
    if targets.is_empty() {
        guard.release()?;
        return Ok(RemoveReport::Done);
    }

    let plan = TransactionPlan::with_targets(
        targets,
        TransactionKind::Remove,
        "Do you want to remove these packages? [Y/n] ",
    );
    if !confirmer.confirm(&plan) {
        guard.release()?;
        return Ok(RemoveReport::Done);
    }

    // Register progress callbacks only after the summary/confirm output, so the
    // sink's live display anchors its cursor correctly (see sync::sync_install).
    sink.borrow_mut().begin();
    crate::common::progress::register(guard.alpm(), sink.clone());
    commit(guard.alpm())?;
    guard.release()?;
    Ok(RemoveReport::Done)
}

fn build_flags(args: &Args) -> TransFlag {
    let mut flags = TransFlag::NONE;
    if args.cascade {
        flags |= TransFlag::CASCADE;
    }
    match args.nodeps {
        0 => {}
        1 => flags |= TransFlag::NO_DEP_VERSION,
        _ => flags |= TransFlag::NO_DEPS,
    }
    if args.nosave {
        flags |= TransFlag::NO_SAVE;
    }
    match args.recursive {
        0 => {}
        1 => flags |= TransFlag::RECURSE,
        _ => flags |= TransFlag::RECURSE | TransFlag::RECURSE_ALL,
    }
    if args.unneeded {
        flags |= TransFlag::UNNEEDED;
    }
    if args.dbonly {
        flags |= TransFlag::DB_ONLY;
    }
    if args.noscriptlet {
        flags |= TransFlag::NO_SCRIPTLET;
    }
    flags
}

fn apply_assume_installed(alpm: &mut Alpm, specs: &[String]) -> Result<()> {
    for spec in specs {
        let dep = Depend::new(spec.as_str());
        alpm.add_assume_installed(&dep).map_err(MizError::Alpm)?;
    }
    Ok(())
}

fn add_targets(alpm: &Alpm, names: &[String]) -> Result<()> {
    let db = alpm.localdb();
    for name in names {
        let pkg = db.pkg(name.as_bytes()).map_err(|_| {
            eprintln!("error: target not found: {name}");
            MizError::PackageNotFound(name.clone())
        })?;
        alpm.trans_remove_pkg(pkg).map_err(|e| {
            eprintln!("error: failed to add target '{}': {}", name, e);
            MizError::Alpm(e)
        })?;
    }
    Ok(())
}

fn run_print(args: &Args, ctx: &mut Context, flags: TransFlag) -> Result<RemoveReport> {
    let flags = flags | TransFlag::NO_LOCK;
    let mut guard = TransGuard::new(&mut ctx.alpm, flags)?;
    add_targets(guard.alpm(), &args.packages)?;
    prepare(guard.alpm())?;

    let format = args.print_format.as_deref();
    let lines: Vec<String> = guard
        .alpm()
        .trans_remove()
        .iter()
        .map(|p: &Package| format_print_line(p, format))
        .collect();

    let release_warning = guard
        .release()
        .err()
        .map(|e| format!("trans_release failed after --print: {e}"));
    Ok(RemoveReport::Print {
        lines,
        release_warning,
    })
}
