use crate::config::Context;
use crate::error::{MizError, Result};
use crate::operations::transaction::{
    collect_pkgs, commit, confirm, format_print_line, prepare, print_summary, should_prompt,
    TransGuard,
};
use alpm::{Alpm, Package, SigLevel, TransFlag};
use std::path::{Path, PathBuf};

pub use crate::cli::args::upgrade::Args;

pub fn run(args: Args, ctx: &mut Context) -> Result<()> {
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
        return Ok(());
    }

    print_summary(&targets, &ctx.palette);

    if should_prompt(args.noconfirm) && !confirm("Proceed with installation? [Y/n] ") {
        guard.release()?;
        return Ok(());
    }

    // Register progress bars only after the summary/confirm output, so indicatif
    // anchors its cursor correctly (see the note in sync::sync_install).
    crate::operations::progress::install(guard.alpm(), args.noprogressbar);
    commit(guard.alpm())?;
    guard.release()?;
    Ok(())
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

fn run_print(args: &Args, ctx: &mut Context, flags: TransFlag) -> Result<()> {
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

    for line in lines {
        println!("{line}");
    }

    if let Err(e) = guard.release() {
        eprintln!("warning: trans_release failed after --print: {e}");
    }
    Ok(())
}
