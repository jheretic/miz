use crate::common::progress::SharedSink;
use crate::common::report::{Confirmer, TransactionKind, TransactionPlan, UpgradeReport};
use crate::common::transaction::{collect_pkgs, commit, format_print_line, prepare, TransGuard};
use crate::config::Context;
use crate::error::{MizError, Result};
use alpm::{Alpm, Package, SigLevel, TransFlag};
use std::path::{Path, PathBuf};

pub use crate::cli::args::upgrade::Args;

pub fn run(
    args: Args,
    ctx: &mut Context,
    confirmer: &mut dyn Confirmer,
    sink: &SharedSink,
) -> Result<UpgradeReport> {
    apply_overwrites(&mut ctx.alpm, &args.overwrite)?;
    let flags = build_flags(&args);

    if args.print {
        return run_print(&args, ctx, flags);
    }

    let mut guard = TransGuard::new(&mut ctx.alpm, flags)?;
    load_and_add(guard.alpm(), &args.files)?;
    prepare(guard.alpm())?;

    let targets = collect_pkgs(guard.alpm().trans_add());
    if targets.is_empty() {
        guard.release()?;
        return Ok(UpgradeReport::Done);
    }

    let plan = TransactionPlan::with_targets(
        targets,
        TransactionKind::Install,
        "Proceed with installation? [Y/n] ",
    );
    if !confirmer.confirm(&plan) {
        guard.release()?;
        return Ok(UpgradeReport::Done);
    }

    // Register progress callbacks only after the summary/confirm output, so the
    // sink's live display anchors its cursor correctly (see sync::sync_install).
    sink.borrow_mut().begin();
    crate::common::progress::register(guard.alpm(), sink.clone());
    commit(guard.alpm())?;
    guard.release()?;
    Ok(UpgradeReport::Done)
}

fn build_flags(args: &Args) -> TransFlag {
    let mut flags = TransFlag::NONE;
    match args.nodeps {
        0 => {}
        1 => flags |= TransFlag::NO_DEP_VERSION,
        _ => flags |= TransFlag::NO_DEPS,
    }
    if args.dbonly {
        flags |= TransFlag::DB_ONLY;
    }
    if args.noscriptlet {
        flags |= TransFlag::NO_SCRIPTLET;
    }
    if args.needed {
        flags |= TransFlag::NEEDED;
    }
    if args.asdeps {
        flags |= TransFlag::ALL_DEPS;
    }
    if args.asexplicit {
        flags |= TransFlag::ALL_EXPLICIT;
    }
    flags
}

fn apply_overwrites(alpm: &mut Alpm, globs: &[String]) -> Result<()> {
    for glob in globs {
        alpm.add_overwrite_file(glob.as_bytes())
            .map_err(MizError::Alpm)?;
    }
    Ok(())
}

fn load_and_add(alpm: &Alpm, files: &[PathBuf]) -> Result<()> {
    for path in files {
        let loaded = load_one(alpm, path)?;
        alpm.trans_add_pkg(loaded).map_err(|e| {
            eprintln!(
                "error: failed to add '{}' to transaction: {}",
                path.display(),
                e.error
            );
            MizError::Alpm(e.error)
        })?;
    }
    Ok(())
}

fn load_one<'a>(alpm: &'a Alpm, path: &Path) -> Result<alpm::LoadedPackage<'a>> {
    let raw = path.as_os_str().as_encoded_bytes().to_vec();
    alpm.pkg_load(raw, true, SigLevel::USE_DEFAULT)
        .map_err(|e| {
            eprintln!("error: could not load '{}': {}", path.display(), e);
            MizError::Alpm(e)
        })
}

fn run_print(args: &Args, ctx: &mut Context, flags: TransFlag) -> Result<UpgradeReport> {
    let flags = flags | TransFlag::NO_LOCK;
    let mut guard = TransGuard::new(&mut ctx.alpm, flags)?;
    load_and_add(guard.alpm(), &args.files)?;
    prepare(guard.alpm())?;

    let format = args.print_format.as_deref();
    let lines: Vec<String> = guard
        .alpm()
        .trans_add()
        .iter()
        .map(|p: &Package| format_print_line(p, format))
        .collect();

    let release_warning = guard
        .release()
        .err()
        .map(|e| format!("trans_release failed after --print: {e}"));
    Ok(UpgradeReport::Print {
        lines,
        release_warning,
    })
}
