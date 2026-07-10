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

/// The full baked-in /usr image packages (all desc + files metadata). Empty
/// when no image db is configured. Call sites decide whether to skip
/// localdb-shadowed names; this returns the raw set so `-Qi <name>` on an image
/// package that is ALSO layered still resolves.
fn image_packages(ctx: &Context) -> Vec<crate::operations::imagedb::ImagePackage> {
    ctx.image_db
        .as_deref()
        .map(crate::operations::imagedb::all_packages)
        .unwrap_or_default()
}

/// The image packages NOT shadowed by a localdb (overlay) package -- the set to
/// UNION into whole-system listings/filters so a layered package is reported
/// once (from the localdb, which has richer alpm metadata).
fn image_packages_unshadowed(ctx: &Context) -> Vec<crate::operations::imagedb::ImagePackage> {
    image_packages(ctx)
        .into_iter()
        .filter(|p| ctx.alpm.localdb().pkg(p.name.as_bytes()).is_err())
        .collect()
}

/// Render a package-info block for a baked-in /usr (image-db) package. Layout
/// mirrors [`print_info`]. Fields the image db genuinely lacks (reverse-deps,
/// validation) are omitted rather than faked.
fn print_image_info(args: &Args, pkg: &crate::operations::imagedb::ImagePackage) {
    let label = |k: &str, v: &str| println!("{:<19}: {}", k, v);
    let none = |v: &[String]| {
        if v.is_empty() {
            "None".to_string()
        } else {
            v.join("  ")
        }
    };
    label("Name", &pkg.name);
    label("Version", &pkg.version);
    label("Description", pkg.desc.as_deref().unwrap_or("None"));
    label("Architecture", pkg.arch.as_deref().unwrap_or("None"));
    label("URL", pkg.url.as_deref().unwrap_or("None"));
    label("Licenses", &none(&pkg.licenses));
    label("Groups", &none(&pkg.groups));
    label("Provides", &none(&pkg.provides));
    label("Depends On", &none(&pkg.depends));
    // Optional deps: one per line after the first, aligned under the value
    // column, matching print_info's layout.
    if pkg.optdepends.is_empty() {
        label("Optional Deps", "None");
    } else {
        label("Optional Deps", &pkg.optdepends.join("\n                     "));
    }
    label("Conflicts With", &none(&pkg.conflicts));
    label("Replaces", &none(&pkg.replaces));
    if let Some(sz) = pkg.isize {
        label("Installed Size", &format_size(sz));
    }
    label("Packager", pkg.packager.as_deref().unwrap_or("None"));
    if let Some(bd) = pkg.build_date {
        label("Build Date", &format_date(bd));
    }
    if let Some(id) = pkg.install_date {
        label("Install Date", &format_date(id));
    }
    label(
        "Install Reason",
        if pkg.explicit {
            "Explicitly installed (base image /usr)"
        } else {
            "Installed as a dependency (base image /usr)"
        },
    );
    if args.info >= 2 {
        // -Qii: backup files, matching print_info's `path\thash` layout.
        if pkg.backup.is_empty() {
            label("Backup Files", "None");
        } else {
            println!("Backup Files       :");
            for (path, hash) in &pkg.backup {
                println!("{path}\t{hash}");
            }
        }
    }
    println!();
}

fn query_local(args: &Args, ctx: &Context) -> Result<()> {
    let alpm = &ctx.alpm;
    let mut missing = false;
    let mut check_failed = false;

    // Sync-db package names, for the -Qm/-Qn (foreign/native) and -Qu paths.
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

    // Image packages the localdb doesn't shadow, unioned into every path so the
    // baked-in /usr packages get the SAME query support as overlay packages.
    // (-Qu is handled per-package in the loop below: image packages update via
    // the whole /usr A/B image, never as a per-package sync upgrade.)
    let image_pkgs = image_packages_unshadowed(ctx);
    // Reverse-dependency index over the MERGED set (localdb + image), by name
    // and by every provided name, for -Qt (unrequired). Unversioned (name +
    // provides), which is sufficient for the pinned base image.
    let required_names: HashSet<String> = if args.unrequired {
        merged_required_names(ctx, &image_pkgs)
    } else {
        HashSet::new()
    };

    for pkg in alpm.localdb().pkgs() {
        if args.deps && pkg.reason() != PackageReason::Depend {
            continue;
        }
        if args.explicit && pkg.reason() != PackageReason::Explicit {
            continue;
        }
        if args.unrequired {
            // Use the merged (localdb + image) reverse-dep set, not alpm's
            // required_by/optional_for, which cannot see image packages that
            // depend on this one.
            let provides: Vec<String> =
                pkg.provides().iter().map(|p| p.to_string()).collect();
            if name_or_provides_required(pkg.name(), &provides, &required_names) {
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

    // The image (/usr) packages, filtered by the same flags and emitted with the
    // same detail modes as localdb packages.
    for pkg in &image_pkgs {
        // -Qu: image packages update via the whole /usr A/B image (miz -Iu),
        // never as a per-package sync upgrade, so none is ever a candidate.
        // They still count as "installed" for the not-found check below.
        if args.upgrades {
            continue;
        }
        if args.deps && pkg.explicit {
            continue; // -Qd: dependencies only
        }
        if args.explicit && !pkg.explicit {
            continue; // -Qe: explicit only
        }
        if args.unrequired && name_or_provides_required(&pkg.name, &pkg.provides, &required_names) {
            continue;
        }
        // Foreign/native is decided by sync-db presence, exactly as for a
        // localdb package: -Qm (foreign) keeps names absent from every sync db,
        // -Qn (native) keeps names present in one.
        if args.foreign && foreign_names.contains(&pkg.name) {
            continue;
        }
        if args.native && !foreign_names.contains(&pkg.name) {
            continue;
        }
        if let Some(ts) = target_set.as_ref() {
            if !ts.contains(pkg.name.as_str()) {
                continue;
            }
        }
        emit_image_pkg(args, ctx, pkg, &mut check_failed)?;
    }

    if let Some(ts) = target_set.as_ref() {
        for name in ts {
            let in_local = alpm.localdb().pkg(name.as_bytes()).is_ok();
            let in_image = image_pkgs.iter().any(|p| p.name == *name);
            if !in_local && !in_image {
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

/// Detail-mode dispatch for a baked-in /usr (image) package, mirroring
/// [`emit_pkg`] for a localdb `Pkg`.
fn emit_image_pkg(
    args: &Args,
    ctx: &Context,
    pkg: &crate::operations::imagedb::ImagePackage,
    check_failed: &mut bool,
) -> Result<()> {
    if args.changelog {
        // pacman's localdb stores no changelog for ANY package; the image db is
        // no different. Match the localdb "no changelog" behaviour exactly.
        eprintln!("error: no changelog available for '{}'", pkg.name);
        return Err(MizError::Other(format!("no changelog for {}", pkg.name)));
    }
    if args.check > 0 {
        return print_image_check(args, ctx, pkg, check_failed);
    }
    if args.list {
        print_image_files(args, pkg);
        return Ok(());
    }
    if args.info > 0 {
        print_image_info(args, pkg);
        return Ok(());
    }
    if args.quiet {
        println!("{}", pkg.name);
    } else {
        println!("{} {}", pkg.name, pkg.version);
    }
    Ok(())
}

/// Every name any installed package (localdb + image) requires -- both hard
/// `depends` and `optdepends` -- for the -Qt (unrequired) filter. Unversioned:
/// a package is "required" if its name, or a name it provides, appears here.
/// `image_pkgs` is the unshadowed image set already computed by the caller
/// (avoids re-reading the image db).
fn merged_required_names(
    ctx: &Context,
    image_pkgs: &[crate::operations::imagedb::ImagePackage],
) -> HashSet<String> {
    let mut set = HashSet::new();
    for pkg in ctx.alpm.localdb().pkgs() {
        for dep in pkg.depends() {
            set.insert(dep_bare_name(&dep.to_string()));
        }
        for opt in pkg.optdepends() {
            set.insert(dep_bare_name(&opt.to_string()));
        }
    }
    for pkg in image_pkgs {
        for dep in &pkg.depends {
            set.insert(dep_bare_name(dep));
        }
        for opt in &pkg.optdepends {
            set.insert(dep_bare_name(opt));
        }
    }
    set
}

/// The bare package name from a dependency, optional-dependency, or provision
/// token: drop an optdepends `: description` suffix, then any version
/// constraint. "foo>=1.2" -> "foo"; "bar: needed for X" -> "bar";
/// "libfoo.so=1-64" -> "libfoo.so".
fn dep_bare_name(token: &str) -> String {
    let no_desc = token.split(':').next().unwrap_or(token);
    no_desc
        .split(['<', '>', '='])
        .next()
        .unwrap_or(no_desc)
        .trim()
        .to_string()
}

/// Whether a package (its own name or any name it provides) is required by
/// something installed, i.e. appears in `required_names`.
fn name_or_provides_required(
    name: &str,
    provides: &[String],
    required_names: &HashSet<String>,
) -> bool {
    required_names.contains(name)
        || provides
            .iter()
            .any(|p| required_names.contains(&dep_bare_name(p)))
}

/// `-Ql` for an image package: list its owned files.
fn print_image_files(args: &Args, pkg: &crate::operations::imagedb::ImagePackage) {
    for file in &pkg.files {
        if args.quiet {
            println!("{file}");
        } else {
            println!("{} {}", pkg.name, file);
        }
    }
}

/// `-Qk` for an image package: check its owned files against the live root,
/// mirroring [`print_check`]. `-Qkk` (check>=2) additionally cannot verify
/// size/mode from the image `files` list (pacman's `files` db carries only
/// paths + backup hashes, not per-file size/mode), so only existence is checked
/// -- which is also exactly what a size/mode-less localdb check would report.
fn print_image_check(
    args: &Args,
    ctx: &Context,
    pkg: &crate::operations::imagedb::ImagePackage,
    check_failed: &mut bool,
) -> Result<()> {
    let mut total = 0usize;
    let mut missing = 0usize;
    for rel in &pkg.files {
        total += 1;
        let path = resolve_file_path(&ctx.root, rel);
        if std::fs::symlink_metadata(&path).is_err() {
            missing += 1;
            if !args.quiet {
                eprintln!("{}: /{} (Missing file)", pkg.name, rel);
            }
        }
    }
    let mut line = format!("{}: {total} total files, {missing} missing files", pkg.name);
    if args.check >= 2 {
        // The image `files` db carries no per-file size/mode, so alteration
        // cannot be detected; report 0 altered, matching the localdb -Qkk line.
        line.push_str(", 0 altered files");
    }
    println!("{line}");
    if missing > 0 {
        *check_failed = true;
    }
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
    // Reject out-of-range values (negative, or beyond year 9999) rather than
    // looping ~292 billion times. Image-db timestamps are parsed from untrusted
    // text, so a bogus value must not hang -Qi.
    if !(0..=253_402_300_799).contains(&secs) {
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
    // Also search the baked-in /usr image packages (matching name OR
    // description, like the localdb arm). Shadowed names are excluded so a
    // layered package is reported once, from the localdb arm above.
    for pkg in image_packages_unshadowed(ctx) {
        let desc = pkg.desc.as_deref().unwrap_or("");
        let name_match = re.is_match(&pkg.name);
        let desc_match = re.is_match(desc);
        if !(name_match || desc_match) {
            continue;
        }
        if args.quiet {
            println!("{}", pkg.name);
        } else {
            println!("image/{} {}", pkg.name, pkg.version);
            if !desc.is_empty() {
                println!("    {desc}");
            }
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
    // Also search the baked-in /usr image packages. A localdb (overlay) package
    // shadows its image version, so skip image packages whose name is in localdb
    // to avoid a duplicate "owned by" line.
    for pkg in image_packages(ctx) {
        if ctx.alpm.localdb().pkg(pkg.name.as_bytes()).is_ok() {
            continue;
        }
        // `needle` has its trailing slash stripped; image `files` keep it on
        // directory entries, so trim both sides before comparing.
        if pkg
            .files
            .iter()
            .any(|f| f.trim_end_matches('/').as_bytes() == needle_bytes)
        {
            println!("{} is owned by {} {}", path, pkg.name, pkg.version);
            found = true;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dep_bare_name_strips_version_and_description() {
        assert_eq!(dep_bare_name("foo"), "foo");
        assert_eq!(dep_bare_name("foo>=1.2"), "foo");
        assert_eq!(dep_bare_name("foo<3"), "foo");
        assert_eq!(dep_bare_name("foo=1-1"), "foo");
        assert_eq!(dep_bare_name("bar: needed for X"), "bar");
        assert_eq!(dep_bare_name("baz: optional >= 2"), "baz");
        assert_eq!(dep_bare_name("libfoo.so=1-64"), "libfoo.so");
    }

    #[test]
    fn name_or_provides_required_matches_name_or_provision() {
        let mut req = HashSet::new();
        req.insert("wanted".to_string());
        req.insert("libz.so".to_string());
        // matched by its own name
        assert!(name_or_provides_required("wanted", &[], &req));
        // matched by a provided name (version stripped)
        assert!(name_or_provides_required(
            "other",
            &["libz.so=1-64".to_string()],
            &req
        ));
        // no match
        assert!(!name_or_provides_required(
            "orphan",
            &["unrelated".to_string()],
            &req
        ));
    }

    #[test]
    fn chrono_like_rejects_out_of_range() {
        // negative and absurd values are echoed verbatim, not looped over
        assert_eq!(chrono_like(-5), "-5");
        assert_eq!(chrono_like(i64::MAX), i64::MAX.to_string());
        // a normal value formats
        assert!(chrono_like(0).starts_with("1970-01-01"));
    }
}
