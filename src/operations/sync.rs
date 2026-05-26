use crate::config::Context;
use crate::error::{MizError, Result};
use crate::operations::query::{
    format_date, format_size, format_validation, join_dep_list, join_list_str, join_optdeps,
};
use crate::operations::transaction::{
    collect_pkgs, commit, confirm, prepare, print_summary, should_prompt, TransGuard,
};
use alpm::{Alpm, Db, Package, Pkg, TransFlag};
use std::collections::HashSet;
use std::path::PathBuf;

pub use crate::cli::args::sync::Args;

pub fn run(args: Args, ctx: &mut Context) -> Result<()> {
    if args.clean > 0 {
        return sync_clean(&args, ctx);
    }

    if args.refresh > 0 {
        let force = args.refresh >= 2;
        let dbs = ctx.alpm.syncdbs_mut();
        let _ = dbs.update(force)?;
    }

    if let Some(re) = args.search.as_ref() {
        return sync_search(&args, ctx, re);
    }
    if args.list {
        return sync_list(&args, ctx);
    }
    if args.groups {
        return sync_groups(&args, ctx);
    }
    if args.info > 0 {
        return sync_info(&args, ctx);
    }

    let do_install = !args.targets.is_empty() || args.sysupgrade > 0;

    if do_install {
        return sync_install(&args, ctx, args.print);
    }

    if args.print {
        return Ok(());
    }

    if args.refresh > 0 && args.targets.is_empty() && !args.downloadonly {
        return Ok(());
    }

    Err(MizError::NotImplemented)
}

fn sync_search(args: &Args, ctx: &Context, pattern: &str) -> Result<()> {
    let re = regex::Regex::new(pattern)?;
    let installed: HashSet<String> = ctx
        .alpm
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .collect();

    for db in ctx.alpm.syncdbs() {
        for pkg in db.pkgs() {
            let name_match = re.is_match(pkg.name());
            let desc_match = pkg.desc().map(|d| re.is_match(d)).unwrap_or(false);
            if !(name_match || desc_match) {
                continue;
            }
            if args.quiet {
                println!("{}", pkg.name());
                continue;
            }
            let suffix = if installed.contains(pkg.name()) {
                " [installed]"
            } else {
                ""
            };
            println!("{}/{} {}{}", db.name(), pkg.name(), pkg.version(), suffix);
            let desc = pkg.desc().unwrap_or("");
            println!("    {desc}");
        }
    }
    Ok(())
}

fn sync_list(args: &Args, ctx: &Context) -> Result<()> {
    let installed: HashSet<String> = ctx
        .alpm
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .collect();

    let targets: Vec<&Db> = if args.targets.is_empty() {
        ctx.alpm.syncdbs().iter().collect()
    } else {
        let mut out = Vec::with_capacity(args.targets.len());
        for name in &args.targets {
            match ctx.alpm.syncdbs().iter().find(|d| d.name() == name) {
                Some(db) => out.push(db),
                None => {
                    eprintln!("error: repository '{name}' was not found");
                    return Err(MizError::PackageNotFound(name.clone()));
                }
            }
        }
        out
    };

    for db in targets {
        for pkg in db.pkgs() {
            if args.quiet {
                println!("{}", pkg.name());
            } else {
                let suffix = if installed.contains(pkg.name()) {
                    " [installed]"
                } else {
                    ""
                };
                println!("{} {} {}{}", db.name(), pkg.name(), pkg.version(), suffix);
            }
        }
    }
    Ok(())
}

fn sync_groups(args: &Args, ctx: &Context) -> Result<()> {
    if args.targets.is_empty() {
        let mut seen: HashSet<String> = HashSet::new();
        for db in ctx.alpm.syncdbs() {
            if let Ok(groups) = db.groups() {
                for group in groups {
                    if seen.insert(group.name().to_string()) {
                        println!("{}", group.name());
                    }
                }
            }
        }
        return Ok(());
    }

    let mut any_missing = false;
    for name in &args.targets {
        let mut found = false;
        for db in ctx.alpm.syncdbs() {
            if let Ok(group) = db.group(name.as_bytes()) {
                found = true;
                for pkg in group.packages() {
                    if args.quiet {
                        println!("{}", pkg.name());
                    } else {
                        println!("{} {}", name, pkg.name());
                    }
                }
            }
        }
        if !found {
            eprintln!("error: group '{name}' was not found");
            any_missing = true;
        }
    }
    if any_missing {
        return Err(MizError::PackageNotFound(args.targets.join(", ")));
    }
    Ok(())
}

fn sync_info(args: &Args, ctx: &Context) -> Result<()> {
    if args.targets.is_empty() {
        for db in ctx.alpm.syncdbs() {
            for pkg in db.pkgs() {
                print_sync_info(args, db, pkg);
            }
        }
        return Ok(());
    }

    let mut missing = false;
    for name in &args.targets {
        let (repo, pkgname) = split_repo_target(name);
        let mut found = false;
        for db in ctx.alpm.syncdbs() {
            if let Some(r) = repo {
                if db.name() != r {
                    continue;
                }
            }
            if let Ok(pkg) = db.pkg(pkgname.as_bytes()) {
                print_sync_info(args, db, pkg);
                found = true;
                break;
            }
        }
        if !found {
            eprintln!("error: package '{name}' was not found");
            missing = true;
        }
    }
    if missing {
        return Err(MizError::PackageNotFound(args.targets.join(", ")));
    }
    Ok(())
}

fn print_sync_info(args: &Args, db: &Db, pkg: &Pkg) {
    let label = |k: &str, v: &str| println!("{:<19}: {}", k, v);
    let label_or = |k: &str, v: Option<&str>| label(k, v.unwrap_or("None"));

    label("Repository", db.name());
    label("Name", pkg.name());
    label("Version", pkg.version().as_str());
    label_or("Description", pkg.desc());
    label_or("Architecture", pkg.arch());
    label_or("URL", pkg.url());
    label("Licenses", &join_list_str(pkg.licenses(), "None"));
    label("Groups", &join_list_str(pkg.groups(), "None"));
    label("Provides", &join_dep_list(pkg.provides(), "None"));
    label("Depends On", &join_dep_list(pkg.depends(), "None"));
    label("Optional Deps", &join_optdeps(pkg, "None"));
    label("Conflicts With", &join_dep_list(pkg.conflicts(), "None"));
    label("Replaces", &join_dep_list(pkg.replaces(), "None"));
    label("Download Size", &format_size(pkg.size()));
    label("Installed Size", &format_size(pkg.isize()));
    label_or("Packager", pkg.packager());
    label("Build Date", &format_date(pkg.build_date()));
    label("Validated By", &format_validation(pkg.validation()));

    if args.info >= 2 {
        let backups: Vec<String> = pkg
            .backup()
            .iter()
            .map(|b| format!("{}\t{}", b.name(), b.hash()))
            .collect();
        if backups.is_empty() {
            label("Backup Files", "None");
        } else {
            println!("Backup Files       :");
            for line in backups {
                println!("{line}");
            }
        }
    }

    println!();
}

fn build_flags(args: &Args) -> TransFlag {
    let mut flags = TransFlag::NONE;
    if args.downloadonly {
        flags |= TransFlag::DOWNLOAD_ONLY;
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
    if args.noscriptlet {
        flags |= TransFlag::NO_SCRIPTLET;
    }
    flags
}

fn apply_ignores(alpm: &mut Alpm, pkgs: &[String], groups: &[String]) -> Result<()> {
    for entry in pkgs {
        for name in entry.split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            alpm.add_ignorepkg(name.as_bytes())
                .map_err(MizError::Alpm)?;
        }
    }
    for entry in groups {
        for name in entry.split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            alpm.add_ignoregroup(name.as_bytes())
                .map_err(MizError::Alpm)?;
        }
    }
    Ok(())
}

fn apply_overwrites(alpm: &mut Alpm, globs: &[String]) -> Result<()> {
    for glob in globs {
        alpm.add_overwrite_file(glob.as_bytes())
            .map_err(MizError::Alpm)?;
    }
    Ok(())
}

fn add_install_targets(alpm: &Alpm, targets: &[String]) -> Result<()> {
    for name in targets {
        let (repo, pkgname) = split_repo_target(name);
        let mut pkg: Option<&Package> = None;
        if let Some(r) = repo {
            for db in alpm.syncdbs() {
                if db.name() != r {
                    continue;
                }
                pkg = db.pkgs().find_satisfier(pkgname.as_bytes());
                if pkg.is_some() {
                    break;
                }
            }
        } else {
            pkg = alpm.syncdbs().find_satisfier(pkgname.as_bytes());
        }

        if let Some(p) = pkg {
            alpm.trans_add_pkg(p).map_err(|e| {
                eprintln!(
                    "error: failed to add '{}' to transaction: {}",
                    name, e.error
                );
                MizError::Alpm(e.error)
            })?;
            continue;
        }

        let group_members = expand_group(alpm, repo, pkgname);
        if !group_members.is_empty() {
            for p in group_members {
                alpm.trans_add_pkg(p).map_err(|e| MizError::Alpm(e.error))?;
            }
            continue;
        }

        eprintln!("error: target not found: {name}");
        return Err(MizError::PackageNotFound(name.clone()));
    }
    Ok(())
}

fn expand_group<'a>(alpm: &'a Alpm, repo: Option<&str>, name: &str) -> Vec<&'a Package> {
    let mut out: Vec<&Package> = Vec::new();
    for db in alpm.syncdbs() {
        if let Some(r) = repo {
            if db.name() != r {
                continue;
            }
        }
        if let Ok(group) = db.group(name.as_bytes()) {
            for p in group.packages() {
                out.push(p);
            }
        }
    }
    out
}

fn sync_install(args: &Args, ctx: &mut Context, print_only: bool) -> Result<()> {
    apply_overwrites(&mut ctx.alpm, &args.overwrite)?;
    apply_ignores(&mut ctx.alpm, &args.ignore, &args.ignoregroup)?;

    let mut flags = build_flags(args);
    if print_only {
        flags |= TransFlag::NO_LOCK;
    }

    let mut guard = TransGuard::new(&mut ctx.alpm, flags)?;

    if !print_only {
        crate::operations::progress::install(guard.alpm(), args.noprogressbar);
    }

    if !args.targets.is_empty() {
        add_install_targets(guard.alpm(), &args.targets)?;
    }

    if args.sysupgrade > 0 {
        let downgrade = args.sysupgrade >= 2;
        guard.alpm().sync_sysupgrade(downgrade)?;
    }

    prepare(guard.alpm())?;

    if print_only {
        let format = args.print_format.as_deref();
        let lines: Vec<String> = guard
            .alpm()
            .trans_add()
            .iter()
            .map(|p: &Package| match format {
                Some(fmt) => crate::operations::transaction::render_format(fmt, p),
                None => format_print_target(p),
            })
            .collect();
        for line in lines {
            println!("{line}");
        }
        if let Err(e) = guard.release() {
            eprintln!("warning: trans_release failed after --print: {e}");
        }
        return Ok(());
    }

    let targets = collect_pkgs(guard.alpm().trans_add());
    if targets.is_empty() {
        guard.release()?;
        return Ok(());
    }

    print_summary(&targets);

    let prompt = if args.downloadonly {
        "Proceed with download? [Y/n] "
    } else {
        "Proceed with installation? [Y/n] "
    };
    if should_prompt(args.noconfirm) && !confirm(prompt) {
        guard.release()?;
        return Ok(());
    }

    commit(guard.alpm())?;
    guard.release()?;
    Ok(())
}

fn sync_clean(args: &Args, ctx: &mut Context) -> Result<()> {
    let installed: HashSet<(String, String)> = ctx
        .alpm
        .localdb()
        .pkgs()
        .iter()
        .map(|p| (p.name().to_string(), p.version().as_str().to_string()))
        .collect();

    let dirs: Vec<PathBuf> = ctx.alpm.cachedirs().iter().map(PathBuf::from).collect();

    let all = args.clean >= 2;

    let prompt = if all {
        "Do you want to remove ALL files from cache? [Y/n] "
    } else {
        "Do you want to remove all other packages from cache? [Y/n] "
    };
    if should_prompt(args.noconfirm) && !confirm(prompt) {
        return Ok(());
    }

    let mut removed = 0u64;
    for dir in &dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !is_cache_artifact(name) {
                continue;
            }

            let should_remove = if all {
                true
            } else {
                match parse_pkg_filename(name) {
                    Some((n, v)) => !installed.contains(&(n, v)),
                    None => false,
                }
            };
            if !should_remove {
                continue;
            }

            let mut sig = path.clone().into_os_string();
            sig.push(".sig");
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
            let _ = std::fs::remove_file(PathBuf::from(sig));
        }
    }
    eprintln!("removed {removed} package file(s) from cache");
    Ok(())
}

fn is_cache_artifact(name: &str) -> bool {
    if name.ends_with(".part") || name.ends_with(".sig") {
        return false;
    }
    name.contains(".pkg.tar.") || name.ends_with(".pkg.tar")
}

fn parse_pkg_filename(name: &str) -> Option<(String, String)> {
    let stem = if let Some(s) = name.strip_suffix(".pkg.tar.zst") {
        s
    } else if let Some(s) = name.strip_suffix(".pkg.tar.xz") {
        s
    } else if let Some(s) = name.strip_suffix(".pkg.tar.gz") {
        s
    } else if let Some(s) = name.strip_suffix(".pkg.tar.bz2") {
        s
    } else if let Some(s) = name.strip_suffix(".pkg.tar.lz4") {
        s
    } else if let Some(s) = name.strip_suffix(".pkg.tar.lzo") {
        s
    } else if let Some(s) = name.strip_suffix(".pkg.tar.lzma") {
        s
    } else if let Some(s) = name.strip_suffix(".pkg.tar.Z") {
        s
    } else if let Some(s) = name.strip_suffix(".pkg.tar") {
        s
    } else {
        return None;
    };

    let parts: Vec<&str> = stem.rsplitn(4, '-').collect();
    if parts.len() < 4 {
        return None;
    }
    let _arch = parts[0];
    let pkgrel = parts[1];
    let pkgver = parts[2];
    let pkgname = parts[3];
    let version = format!("{pkgver}-{pkgrel}");
    Some((pkgname.to_string(), version))
}

fn format_print_target(pkg: &Pkg) -> String {
    let repo = pkg.db().map(|d| d.name()).unwrap_or("local");
    let filename = pkg.filename().unwrap_or("");
    let servers: Vec<&str> = pkg
        .db()
        .map(|d| d.servers().iter().collect())
        .unwrap_or_default();
    if let Some(server) = servers.first() {
        if filename.is_empty() {
            format!("{}/{}-{}", repo, pkg.name(), pkg.version())
        } else {
            format!("{}/{}", server.trim_end_matches('/'), filename)
        }
    } else if !filename.is_empty() {
        format!("{}/{}", repo, filename)
    } else {
        format!("{}/{} {}", repo, pkg.name(), pkg.version())
    }
}

fn split_repo_target(target: &str) -> (Option<&str>, &str) {
    match target.split_once('/') {
        Some((repo, name)) => (Some(repo), name),
        None => (None, target),
    }
}
