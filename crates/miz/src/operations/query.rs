use crate::config::Context;
use crate::error::{MizError, Result};
use alpm::{PackageReason, Pkg, SigLevel};
use std::collections::HashSet;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

pub use crate::cli::args::query::Args;

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    if let Some(path) = args.file.as_ref() {
        return query_file(&args, ctx, path);
    }
    if let Some(path) = args.owns.as_ref() {
        return query_owns(ctx, path);
    }
    if args.groups {
        return query_groups(&args, ctx);
    }
    if let Some(re) = args.search.as_ref() {
        return query_search(&args, ctx, re);
    }
    query_local(&args, ctx)
}

fn query_local(args: &Args, ctx: &Context) -> Result<()> {
    let alpm = &ctx.alpm;
    let mut missing = false;
    let mut check_failed = false;

    if !args.packages.is_empty()
        && !args.deps
        && !args.explicit
        && !args.foreign
        && !args.native
        && !args.unrequired
        && !args.upgrades
    {
        for name in &args.packages {
            match alpm.localdb().pkg(name.as_bytes()) {
                Ok(pkg) => emit_pkg(args, ctx, pkg, &mut check_failed)?,
                Err(_) => {
                    eprintln!("error: package '{name}' was not found");
                    missing = true;
                }
            }
        }
        if missing {
            return Err(MizError::PackageNotFound(args.packages.join(", ")));
        }
        if check_failed {
            return Err(MizError::Other(
                "package files are missing or altered".to_string(),
            ));
        }
        return Ok(());
    }

    let foreign_names: HashSet<String> = if args.foreign || args.native {
        let mut set = HashSet::new();
        for db in alpm.syncdbs() {
            for pkg in db.pkgs() {
                set.insert(pkg.name().to_string());
            }
        }
        set
    } else {
        HashSet::new()
    };

    let target_set: Option<HashSet<&str>> = if args.packages.is_empty() {
        None
    } else {
        Some(args.packages.iter().map(|s| s.as_str()).collect())
    };

    for pkg in alpm.localdb().pkgs() {
        if args.deps && pkg.reason() != PackageReason::Depend {
            continue;
        }
        if args.explicit && pkg.reason() != PackageReason::Explicit {
            continue;
        }
        if args.unrequired {
            if !pkg.required_by().is_empty() {
                continue;
            }
            if !pkg.optional_for().is_empty() {
                continue;
            }
        }
        if args.foreign && foreign_names.contains(pkg.name()) {
            continue;
        }
        if args.native && !foreign_names.contains(pkg.name()) {
            continue;
        }
        if args.upgrades && pkg.sync_new_version(alpm.syncdbs()).is_none() {
            continue;
        }

        if let Some(ts) = target_set.as_ref() {
            if !ts.contains(pkg.name()) {
                continue;
            }
        }

        emit_pkg(args, ctx, pkg, &mut check_failed)?;
    }

    if let Some(ts) = target_set.as_ref() {
        for name in ts {
            let found = alpm.localdb().pkg(name.as_bytes()).is_ok();
            if !found {
                eprintln!("error: package '{name}' was not found");
                missing = true;
            }
        }
    }
    if missing {
        return Err(MizError::PackageNotFound(args.packages.join(", ")));
    }
    if check_failed {
        return Err(MizError::Other(
            "package files are missing or altered".to_string(),
        ));
    }
    Ok(())
}

fn emit_pkg(args: &Args, ctx: &Context, pkg: &Pkg, check_failed: &mut bool) -> Result<()> {
    if args.changelog {
        return print_changelog(pkg);
    }
    if args.check > 0 {
        return print_check(args, ctx, pkg, check_failed);
    }
    if args.list {
        return print_files(args, pkg);
    }
    if args.info > 0 {
        return print_info(args, pkg);
    }
    print_name_version(args, pkg);
    Ok(())
}

fn print_name_version(args: &Args, pkg: &Pkg) {
    if args.quiet {
        println!("{}", pkg.name());
    } else {
        println!("{} {}", pkg.name(), pkg.version());
    }
}

fn print_files(args: &Args, pkg: &Pkg) -> Result<()> {
    if args.quiet {
        for file in pkg.files().files() {
            println!("{}", String::from_utf8_lossy(file.name()));
        }
    } else {
        for file in pkg.files().files() {
            println!("{} {}", pkg.name(), String::from_utf8_lossy(file.name()));
        }
    }
    Ok(())
}

fn print_changelog(pkg: &Pkg) -> Result<()> {
    match pkg.changelog() {
        Ok(mut cl) => {
            let mut buf = String::new();
            cl.read_to_string(&mut buf)?;
            print!("{buf}");
            if !buf.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        Err(_) => {
            eprintln!("error: no changelog available for '{}'", pkg.name());
            Err(MizError::Other(format!("no changelog for {}", pkg.name())))
        }
    }
}

fn print_check(args: &Args, ctx: &Context, pkg: &Pkg, check_failed: &mut bool) -> Result<()> {
    let files = pkg.files().files();
    let mut total: usize = 0;
    let mut missing: usize = 0;
    let mut altered: usize = 0;

    for file in files {
        total += 1;
        let rel = String::from_utf8_lossy(file.name());
        let path = resolve_file_path(&ctx.root, rel.as_ref());
        let is_dir_entry = rel.ends_with('/');
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing += 1;
                if !args.quiet {
                    eprintln!("{}: /{} (Missing file)", pkg.name(), rel);
                }
            }
            Err(e) => {
                missing += 1;
                if !args.quiet {
                    eprintln!("{}: /{} ({})", pkg.name(), rel, e);
                }
            }
            Ok(md) if args.check >= 2 => {
                let mut bad = false;
                let expected_mode = file.mode() & 0o7777;
                let actual_mode = md.permissions().mode() & 0o7777;
                if expected_mode != 0 && expected_mode != actual_mode {
                    eprintln!(
                        "{}: /{} (Permissions mismatch, expected {:04o} actual {:04o})",
                        pkg.name(),
                        rel,
                        expected_mode,
                        actual_mode
                    );
                    bad = true;
                }
                if !is_dir_entry && md.is_file() {
                    let expected_size = file.size();
                    let actual_size = md.len() as i64;
                    if expected_size > 0 && expected_size != actual_size {
                        eprintln!(
                            "{}: /{} (Size mismatch, expected {} actual {})",
                            pkg.name(),
                            rel,
                            expected_size,
                            actual_size
                        );
                        bad = true;
                    }
                }
                if bad {
                    altered += 1;
                }
            }
            Ok(_) => {}
        }
    }

    let mut line = format!(
        "{}: {} total files, {} missing files",
        pkg.name(),
        total,
        missing
    );
    if args.check >= 2 {
        line.push_str(&format!(", {altered} altered files"));
    }
    println!("{line}");

    if missing > 0 || altered > 0 {
        *check_failed = true;
    }
    Ok(())
}

fn resolve_file_path(root: &std::path::Path, rel: &str) -> PathBuf {
    let trimmed = rel.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        root.to_path_buf()
    } else {
        root.join(trimmed)
    }
}

fn print_info(args: &Args, pkg: &Pkg) -> Result<()> {
    let label = |k: &str, v: &str| println!("{:<19}: {}", k, v);
    let label_or = |k: &str, v: Option<&str>| label(k, v.unwrap_or("None"));

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
    let req = pkg.required_by();
    label(
        "Required By",
        &join_string_list(req.iter().map(|s| s.to_string()), "None"),
    );
    let opt = pkg.optional_for();
    label(
        "Optional For",
        &join_string_list(opt.iter().map(|s| s.to_string()), "None"),
    );
    label("Conflicts With", &join_dep_list(pkg.conflicts(), "None"));
    label("Replaces", &join_dep_list(pkg.replaces(), "None"));
    label("Installed Size", &format_size(pkg.isize()));
    label_or("Packager", pkg.packager());
    label("Build Date", &format_date(pkg.build_date()));
    label(
        "Install Date",
        &pkg.install_date()
            .map(format_date)
            .unwrap_or_else(|| "Unknown".into()),
    );
    label(
        "Install Reason",
        match pkg.reason() {
            PackageReason::Explicit => "Explicitly installed",
            PackageReason::Depend => "Installed as a dependency for another package",
        },
    );
    label(
        "Install Script",
        if pkg.has_scriptlet() { "Yes" } else { "No" },
    );
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
    Ok(())
}

pub(crate) fn join_list_str(list: alpm::AlpmList<&str>, none: &str) -> String {
    let items: Vec<&str> = list.iter().collect();
    if items.is_empty() {
        none.to_string()
    } else {
        items.join("  ")
    }
}

pub(crate) fn join_dep_list(list: alpm::AlpmList<&alpm::Dep>, none: &str) -> String {
    let items: Vec<String> = list.iter().map(|d| d.to_string()).collect();
    if items.is_empty() {
        none.to_string()
    } else {
        items.join("  ")
    }
}

pub(crate) fn join_string_list<I: IntoIterator<Item = String>>(it: I, none: &str) -> String {
    let items: Vec<String> = it.into_iter().collect();
    if items.is_empty() {
        none.to_string()
    } else {
        items.join("  ")
    }
}

pub(crate) fn join_optdeps(pkg: &Pkg, none: &str) -> String {
    let items: Vec<String> = pkg.optdepends().iter().map(|d| d.to_string()).collect();
    if items.is_empty() {
        none.to_string()
    } else {
        items.join("\n                     ")
    }
}

pub(crate) fn format_size(bytes: i64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    format!("{:.2} {}", v, UNITS[u])
}

pub(crate) fn format_date(secs: i64) -> String {
    chrono_like(secs)
}

fn chrono_like(secs: i64) -> String {
    let days_per_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut secs = secs;
    if secs < 0 {
        return secs.to_string();
    }
    let h = ((secs / 3600) % 24) as u32;
    let m = ((secs / 60) % 60) as u32;
    let s = (secs % 60) as u32;
    let mut days = secs / 86400;
    secs %= 86400;
    let _ = secs;
    let mut year: i64 = 1970;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let yd = if leap { 366 } else { 365 };
        if days >= yd {
            days -= yd;
            year += 1;
        } else {
            break;
        }
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let mut month = 0usize;
    while month < 12 {
        let mut dm = days_per_month[month] as i64;
        if month == 1 && leap {
            dm = 29;
        }
        if days >= dm {
            days -= dm;
            month += 1;
        } else {
            break;
        }
    }
    let day = days + 1;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        year,
        month + 1,
        day,
        h,
        m,
        s
    )
}

pub(crate) fn format_validation(v: alpm::PackageValidation) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if v.contains(alpm::PackageValidation::NONE) {
        parts.push("None");
    }
    if v.contains(alpm::PackageValidation::MD5SUM) {
        parts.push("MD5 Sum");
    }
    if v.contains(alpm::PackageValidation::SHA256SUM) {
        parts.push("SHA-256 Sum");
    }
    if v.contains(alpm::PackageValidation::SIGNATURE) {
        parts.push("Signature");
    }
    if parts.is_empty() {
        "Unknown".to_string()
    } else {
        parts.join("  ")
    }
}

fn query_search(args: &Args, ctx: &Context, pattern: &str) -> Result<()> {
    let re = regex::Regex::new(pattern)?;
    for pkg in ctx.alpm.localdb().pkgs() {
        let name_match = re.is_match(pkg.name());
        let desc_match = pkg.desc().map(|d| re.is_match(d)).unwrap_or(false);
        if !(name_match || desc_match) {
            continue;
        }
        if args.quiet {
            println!("{}", pkg.name());
        } else {
            let desc = pkg.desc().unwrap_or("");
            println!("local/{} {}", pkg.name(), pkg.version());
            println!("    {desc}");
        }
    }
    Ok(())
}

fn query_owns(ctx: &Context, path: &str) -> Result<()> {
    let needle = normalise_owns_path(path)?;
    let needle_bytes = needle.as_bytes();
    let mut found = false;
    for pkg in ctx.alpm.localdb().pkgs() {
        for file in pkg.files().files() {
            if file.name() == needle_bytes {
                println!("{} is owned by {} {}", path, pkg.name(), pkg.version());
                found = true;
            }
        }
    }
    if !found {
        eprintln!("error: No package owns {path}");
        return Err(MizError::Other(format!("no package owns {path}")));
    }
    Ok(())
}

fn normalise_owns_path(path: &str) -> Result<String> {
    let p = std::path::Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let canon = abs.canonicalize().unwrap_or(abs);
    let s = canon.to_string_lossy().to_string();
    let stripped = s.trim_start_matches('/').to_string();
    Ok(stripped)
}

fn query_file(args: &Args, ctx: &Context, path: &std::path::Path) -> Result<()> {
    let bytes = path.as_os_str().as_encoded_bytes().to_vec();
    let loaded = ctx.alpm.pkg_load(bytes, true, SigLevel::USE_DEFAULT)?;
    let pkg: &Pkg = loaded.pkg();
    if args.list {
        return print_files(args, pkg);
    }
    if args.info > 0 {
        return print_info(args, pkg);
    }
    print_name_version(args, pkg);
    Ok(())
}

fn query_groups(args: &Args, ctx: &Context) -> Result<()> {
    let alpm = &ctx.alpm;
    if args.packages.is_empty() {
        if let Ok(groups) = alpm.localdb().groups() {
            for group in groups {
                if args.quiet {
                    println!("{}", group.name());
                } else {
                    for pkg in group.packages() {
                        println!("{} {}", group.name(), pkg.name());
                    }
                }
            }
        }
        return Ok(());
    }
    let mut any_missing = false;
    for name in &args.packages {
        match alpm.localdb().group(name.as_bytes()) {
            Ok(group) => {
                for pkg in group.packages() {
                    println!("{} {}", group.name(), pkg.name());
                }
            }
            Err(_) => {
                eprintln!("error: group '{name}' was not found");
                any_missing = true;
            }
        }
    }
    if any_missing {
        return Err(MizError::PackageNotFound(args.packages.join(", ")));
    }
    Ok(())
}
