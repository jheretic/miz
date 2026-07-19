use crate::common::imagedb::ImagePackage;
use crate::common::report::{
    CheckResult, FileLine, InfoBlock, InfoField, PkgLine, QueryBody, QueryError, QueryReport,
    SearchHit, SearchSource,
};
use crate::config::Context;
use crate::error::Result;
use crate::common::fmt::{
    format_date, format_size, format_validation, join_dep_list, join_list_str, join_optdeps,
    join_string_list,
};
use alpm::{PackageReason, Pkg, SigLevel};
use std::collections::HashSet;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::params::query::Params as Args;

/// Accumulators for one query invocation. Exactly one is populated per run
/// (the detail mode is fixed by args), mirroring the old emit_pkg dispatch.
#[derive(Default)]
struct Body {
    list: Vec<PkgLine>,
    info: Vec<InfoBlock>,
    files: Vec<FileLine>,
    check: Vec<CheckResult>,
    changelog: Vec<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<QueryReport> {
    if let Some(path) = args.file.as_ref() {
        return query_file(&args, ctx, path);
    }
    if let Some(path) = args.owns.as_ref() {
        return Ok(query_owns(ctx, path));
    }
    if args.groups {
        return Ok(query_groups(&args, ctx));
    }
    if let Some(re) = args.search.as_ref() {
        return query_search(&args, ctx, re);
    }
    query_local(&args, ctx)
}

fn image_packages(ctx: &Context) -> Vec<ImagePackage> {
    ctx.image_db
        .as_deref()
        .map(crate::common::imagedb::all_packages)
        .unwrap_or_default()
}

fn image_packages_unshadowed(ctx: &Context) -> Vec<ImagePackage> {
    image_packages(ctx)
        .into_iter()
        .filter(|p| ctx.alpm.localdb().pkg(p.name.as_bytes()).is_err())
        .collect()
}

/// Build a package-info block for a baked-in /usr (image-db) package. Layout
/// mirrors [`info_block`]. Fields the image db genuinely lacks (reverse-deps,
/// validation) are omitted rather than faked.
fn label(fields: &mut Vec<InfoField>, k: &str, v: &str) {
    fields.push(InfoField::Label {
        key: k.to_string(),
        value: v.to_string(),
    });
}

fn image_info_block(args: &Args, pkg: &ImagePackage) -> InfoBlock {
    let mut fields: Vec<InfoField> = Vec::new();
    macro_rules! label {
        ($k:expr, $v:expr) => {
            label(&mut fields, $k, $v)
        };
    }
    let none = |v: &[String]| {
        if v.is_empty() {
            "None".to_string()
        } else {
            v.join("  ")
        }
    };
    label!("Name", &pkg.name);
    label!("Version", &pkg.version);
    label!("Description", pkg.desc.as_deref().unwrap_or("None"));
    label!("Architecture", pkg.arch.as_deref().unwrap_or("None"));
    label!("URL", pkg.url.as_deref().unwrap_or("None"));
    label!("Licenses", &none(&pkg.licenses));
    label!("Groups", &none(&pkg.groups));
    label!("Provides", &none(&pkg.provides));
    label!("Depends On", &none(&pkg.depends));
    if pkg.optdepends.is_empty() {
        label!("Optional Deps", "None");
    } else {
        label!("Optional Deps", &pkg.optdepends.join("\n                     "));
    }
    label!("Conflicts With", &none(&pkg.conflicts));
    label!("Replaces", &none(&pkg.replaces));
    if let Some(sz) = pkg.isize {
        label!("Installed Size", &format_size(sz));
    }
    label!("Packager", pkg.packager.as_deref().unwrap_or("None"));
    if let Some(bd) = pkg.build_date {
        label!("Build Date", &format_date(bd));
    }
    if let Some(id) = pkg.install_date {
        label!("Install Date", &format_date(id));
    }
    label!(
        "Install Reason",
        if pkg.explicit {
            "Explicitly installed (base image /usr)"
        } else {
            "Installed as a dependency (base image /usr)"
        }
    );
    if args.info >= 2 {
        if pkg.backup.is_empty() {
            label!("Backup Files", "None");
        } else {
            let lines = pkg
                .backup
                .iter()
                .map(|(path, hash)| format!("{path}\t{hash}"))
                .collect();
            fields.push(InfoField::Backup(lines));
        }
    }
    InfoBlock { fields }
}

fn query_local(args: &Args, ctx: &Context) -> Result<QueryReport> {
    let alpm = &ctx.alpm;
    let mut missing = false;
    let mut check_failed = false;
    let mut body = Body::default();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut changelog_err: Option<QueryError> = None;

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

    let image_pkgs = image_packages_unshadowed(ctx);
    let required_names: HashSet<String> = if args.unrequired {
        merged_required_names(ctx, &image_pkgs)
    } else {
        HashSet::new()
    };

    'outer: {
        for pkg in alpm.localdb().pkgs() {
            if args.deps && pkg.reason() != PackageReason::Depend {
                continue;
            }
            if args.explicit && pkg.reason() != PackageReason::Explicit {
                continue;
            }
            if args.unrequired {
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
            if let Err(e) = emit_pkg(
                args,
                ctx,
                pkg,
                &mut body,
                &mut diagnostics,
                &mut check_failed,
            ) {
                changelog_err = Some(e);
                break 'outer;
            }
        }

        for pkg in &image_pkgs {
            if args.upgrades {
                continue;
            }
            if args.deps && pkg.explicit {
                continue;
            }
            if args.explicit && !pkg.explicit {
                continue;
            }
            if args.unrequired
                && name_or_provides_required(&pkg.name, &pkg.provides, &required_names)
            {
                continue;
            }
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
            if let Err(e) = emit_image_pkg(
                args,
                ctx,
                pkg,
                &mut body,
                &mut diagnostics,
                &mut check_failed,
            ) {
                changelog_err = Some(e);
                break 'outer;
            }
        }
    }

    // A changelog error propagated immediately in the old `?` flow, before the
    // not-found sweep ran; preserve that by skipping the sweep in that case.
    if changelog_err.is_none() {
        if let Some(ts) = target_set.as_ref() {
            for name in ts {
                let in_local = alpm.localdb().pkg(name.as_bytes()).is_ok();
                let in_image = image_pkgs.iter().any(|p| p.name == *name);
                if !in_local && !in_image {
                    diagnostics.push(format!("error: package '{name}' was not found"));
                    missing = true;
                }
            }
        }
    }

    let error = if let Some(e) = changelog_err {
        Some(e)
    } else if missing {
        Some(QueryError::NotFound(args.packages.join(", ")))
    } else if check_failed {
        Some(QueryError::Other(
            "package files are missing or altered".to_string(),
        ))
    } else {
        None
    };

    Ok(QueryReport {
        body: body.into_body(args),
        diagnostics,
        error,
    })
}

impl Body {
    /// Pick the populated accumulator for the fixed detail mode.
    fn into_body(self, args: &Args) -> QueryBody {
        if args.changelog {
            QueryBody::Changelog(self.changelog)
        } else if args.check > 0 {
            QueryBody::Check(self.check)
        } else if args.list {
            QueryBody::Files {
                quiet: args.quiet,
                lines: self.files,
            }
        } else if args.info > 0 {
            QueryBody::Info(self.info)
        } else {
            QueryBody::List {
                quiet: args.quiet,
                pkgs: self.list,
            }
        }
    }
}

fn emit_pkg(
    args: &Args,
    ctx: &Context,
    pkg: &Pkg,
    body: &mut Body,
    diagnostics: &mut Vec<String>,
    check_failed: &mut bool,
) -> std::result::Result<(), QueryError> {
    if args.changelog {
        return push_changelog(pkg, body, diagnostics);
    }
    if args.check > 0 {
        body.check.push(build_check(args, ctx, pkg, check_failed));
        return Ok(());
    }
    if args.list {
        push_files(args, pkg, body);
        return Ok(());
    }
    if args.info > 0 {
        body.info.push(info_block(args, pkg));
        return Ok(());
    }
    body.list.push(PkgLine {
        name: pkg.name().to_string(),
        version: pkg.version().to_string(),
    });
    Ok(())
}

fn emit_image_pkg(
    args: &Args,
    ctx: &Context,
    pkg: &ImagePackage,
    body: &mut Body,
    diagnostics: &mut Vec<String>,
    check_failed: &mut bool,
) -> std::result::Result<(), QueryError> {
    if args.changelog {
        diagnostics.push(format!("error: no changelog available for '{}'", pkg.name));
        return Err(QueryError::Other(format!("no changelog for {}", pkg.name)));
    }
    if args.check > 0 {
        body.check.push(build_image_check(args, ctx, pkg, check_failed));
        return Ok(());
    }
    if args.list {
        for file in &pkg.files {
            body.files.push(FileLine {
                pkg: pkg.name.clone(),
                file: file.clone(),
            });
        }
        return Ok(());
    }
    if args.info > 0 {
        body.info.push(image_info_block(args, pkg));
        return Ok(());
    }
    body.list.push(PkgLine {
        name: pkg.name.clone(),
        version: pkg.version.clone(),
    });
    Ok(())
}

fn merged_required_names(ctx: &Context, image_pkgs: &[ImagePackage]) -> HashSet<String> {
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

fn dep_bare_name(token: &str) -> String {
    let no_desc = token.split(':').next().unwrap_or(token);
    no_desc
        .split(['<', '>', '='])
        .next()
        .unwrap_or(no_desc)
        .trim()
        .to_string()
}

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

/// `-Qk` for an image package, mirroring [`build_check`].
fn build_image_check(
    args: &Args,
    ctx: &Context,
    pkg: &ImagePackage,
    check_failed: &mut bool,
) -> CheckResult {
    let mut problems: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut missing = 0usize;
    for rel in &pkg.files {
        total += 1;
        let path = resolve_file_path(&ctx.root, rel);
        if std::fs::symlink_metadata(&path).is_err() {
            missing += 1;
            if !args.quiet {
                problems.push(format!("{}: /{} (Missing file)", pkg.name, rel));
            }
        }
    }
    let mut summary = format!("{}: {total} total files, {missing} missing files", pkg.name);
    if args.check >= 2 {
        summary.push_str(", 0 altered files");
    }
    if missing > 0 {
        *check_failed = true;
    }
    CheckResult { problems, summary }
}

fn push_files(args: &Args, pkg: &Pkg, body: &mut Body) {
    let _ = args;
    for file in pkg.files().files() {
        body.files.push(FileLine {
            pkg: pkg.name().to_string(),
            file: String::from_utf8_lossy(file.name()).into_owned(),
        });
    }
}

fn push_changelog(
    pkg: &Pkg,
    body: &mut Body,
    diagnostics: &mut Vec<String>,
) -> std::result::Result<(), QueryError> {
    match pkg.changelog() {
        Ok(mut cl) => {
            let mut buf = String::new();
            if let Err(e) = cl.read_to_string(&mut buf) {
                // read failure surfaced the std::io::Error via `?` in the old
                // code, before any print: no "no changelog available" line.
                return Err(QueryError::Io(e.to_string()));
            }
            if !buf.ends_with('\n') {
                buf.push('\n');
            }
            body.changelog.push(buf);
            Ok(())
        }
        Err(_) => {
            diagnostics.push(format!("error: no changelog available for '{}'", pkg.name()));
            Err(QueryError::Other(format!("no changelog for {}", pkg.name())))
        }
    }
}

fn build_check(args: &Args, ctx: &Context, pkg: &Pkg, check_failed: &mut bool) -> CheckResult {
    let files = pkg.files().files();
    let mut problems: Vec<String> = Vec::new();
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
                    problems.push(format!("{}: /{} (Missing file)", pkg.name(), rel));
                }
            }
            Err(e) => {
                missing += 1;
                if !args.quiet {
                    problems.push(format!("{}: /{} ({})", pkg.name(), rel, e));
                }
            }
            Ok(md) if args.check >= 2 => {
                let mut bad = false;
                let expected_mode = file.mode() & 0o7777;
                let actual_mode = md.permissions().mode() & 0o7777;
                if expected_mode != 0 && expected_mode != actual_mode {
                    problems.push(format!(
                        "{}: /{} (Permissions mismatch, expected {:04o} actual {:04o})",
                        pkg.name(),
                        rel,
                        expected_mode,
                        actual_mode
                    ));
                    bad = true;
                }
                if !is_dir_entry && md.is_file() {
                    let expected_size = file.size();
                    let actual_size = md.len() as i64;
                    if expected_size > 0 && expected_size != actual_size {
                        problems.push(format!(
                            "{}: /{} (Size mismatch, expected {} actual {})",
                            pkg.name(),
                            rel,
                            expected_size,
                            actual_size
                        ));
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

    let mut summary = format!(
        "{}: {} total files, {} missing files",
        pkg.name(),
        total,
        missing
    );
    if args.check >= 2 {
        summary.push_str(&format!(", {altered} altered files"));
    }

    if missing > 0 || altered > 0 {
        *check_failed = true;
    }
    CheckResult { problems, summary }
}

fn resolve_file_path(root: &std::path::Path, rel: &str) -> PathBuf {
    let trimmed = rel.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        root.to_path_buf()
    } else {
        root.join(trimmed)
    }
}

fn info_block(args: &Args, pkg: &Pkg) -> InfoBlock {
    let mut fields: Vec<InfoField> = Vec::new();
    macro_rules! label {
        ($k:expr, $v:expr) => {
            label(&mut fields, $k, $v)
        };
    }
    macro_rules! label_or {
        ($k:expr, $v:expr) => {
            label(&mut fields, $k, $v.unwrap_or("None"))
        };
    }

    label!("Name", pkg.name());
    label!("Version", pkg.version().as_str());
    label_or!("Description", pkg.desc());
    label_or!("Architecture", pkg.arch());
    label_or!("URL", pkg.url());
    label!("Licenses", &join_list_str(pkg.licenses(), "None"));
    label!("Groups", &join_list_str(pkg.groups(), "None"));
    label!("Provides", &join_dep_list(pkg.provides(), "None"));
    label!("Depends On", &join_dep_list(pkg.depends(), "None"));
    label!("Optional Deps", &join_optdeps(pkg, "None"));
    let req = pkg.required_by();
    label!(
        "Required By",
        &join_string_list(req.iter().map(|s| s.to_string()), "None")
    );
    let opt = pkg.optional_for();
    label!(
        "Optional For",
        &join_string_list(opt.iter().map(|s| s.to_string()), "None")
    );
    label!("Conflicts With", &join_dep_list(pkg.conflicts(), "None"));
    label!("Replaces", &join_dep_list(pkg.replaces(), "None"));
    label!("Installed Size", &format_size(pkg.isize()));
    label_or!("Packager", pkg.packager());
    label!("Build Date", &format_date(pkg.build_date()));
    label!(
        "Install Date",
        &pkg.install_date()
            .map(format_date)
            .unwrap_or_else(|| "Unknown".into())
    );
    label!(
        "Install Reason",
        match pkg.reason() {
            PackageReason::Explicit => "Explicitly installed",
            PackageReason::Depend => "Installed as a dependency for another package",
        }
    );
    label!(
        "Install Script",
        if pkg.has_scriptlet() { "Yes" } else { "No" }
    );
    label!("Validated By", &format_validation(pkg.validation()));

    if args.info >= 2 {
        let backups: Vec<String> = pkg
            .backup()
            .iter()
            .map(|b| format!("{}\t{}", b.name(), b.hash()))
            .collect();
        if backups.is_empty() {
            label!("Backup Files", "None");
        } else {
            fields.push(InfoField::Backup(backups));
        }
    }

    InfoBlock { fields }
}

fn query_search(args: &Args, ctx: &Context, pattern: &str) -> Result<QueryReport> {
    let re = regex::Regex::new(pattern)?;
    let mut hits: Vec<SearchHit> = Vec::new();
    for pkg in ctx.alpm.localdb().pkgs() {
        let name_match = re.is_match(pkg.name());
        let desc_match = pkg.desc().map(|d| re.is_match(d)).unwrap_or(false);
        if !(name_match || desc_match) {
            continue;
        }
        hits.push(SearchHit {
            source: SearchSource::Local,
            name: pkg.name().to_string(),
            version: pkg.version().to_string(),
            desc: pkg.desc().unwrap_or("").to_string(),
        });
    }
    for pkg in image_packages_unshadowed(ctx) {
        let desc = pkg.desc.as_deref().unwrap_or("");
        let name_match = re.is_match(&pkg.name);
        let desc_match = re.is_match(desc);
        if !(name_match || desc_match) {
            continue;
        }
        hits.push(SearchHit {
            source: SearchSource::Image,
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            desc: desc.to_string(),
        });
    }
    Ok(QueryReport {
        body: QueryBody::Search {
            quiet: args.quiet,
            hits,
        },
        diagnostics: Vec::new(),
        error: None,
    })
}

fn query_owns(ctx: &Context, path: &str) -> QueryReport {
    let mut lines: Vec<String> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut error = None;
    let needle = match normalise_owns_path(path) {
        Ok(n) => n,
        Err(e) => {
            // io error resolving cwd: no output produced, surface as Other so
            // the top-level handler prints it (parity with the old `?`).
            return QueryReport {
                body: QueryBody::Owns(Vec::new()),
                diagnostics: Vec::new(),
                error: Some(QueryError::Other(e.to_string())),
            };
        }
    };
    let needle_bytes = needle.as_bytes();
    let mut found = false;
    for pkg in ctx.alpm.localdb().pkgs() {
        for file in pkg.files().files() {
            if file.name() == needle_bytes {
                lines.push(format!(
                    "{} is owned by {} {}",
                    path,
                    pkg.name(),
                    pkg.version()
                ));
                found = true;
            }
        }
    }
    for pkg in image_packages(ctx) {
        if ctx.alpm.localdb().pkg(pkg.name.as_bytes()).is_ok() {
            continue;
        }
        if pkg
            .files
            .iter()
            .any(|f| f.trim_end_matches('/').as_bytes() == needle_bytes)
        {
            lines.push(format!("{} is owned by {} {}", path, pkg.name, pkg.version));
            found = true;
        }
    }
    if !found {
        diagnostics.push(format!("error: No package owns {path}"));
        error = Some(QueryError::Other(format!("no package owns {path}")));
    }
    QueryReport {
        body: QueryBody::Owns(lines),
        diagnostics,
        error,
    }
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

fn query_file(args: &Args, ctx: &Context, path: &std::path::Path) -> Result<QueryReport> {
    let bytes = path.as_os_str().as_encoded_bytes().to_vec();
    let loaded = ctx.alpm.pkg_load(bytes, true, SigLevel::USE_DEFAULT)?;
    let pkg: &Pkg = loaded.pkg();
    let body = if args.list {
        let mut lines = Vec::new();
        for file in pkg.files().files() {
            lines.push(FileLine {
                pkg: pkg.name().to_string(),
                file: String::from_utf8_lossy(file.name()).into_owned(),
            });
        }
        QueryBody::Files {
            quiet: args.quiet,
            lines,
        }
    } else if args.info > 0 {
        QueryBody::Info(vec![info_block(args, pkg)])
    } else {
        QueryBody::List {
            quiet: args.quiet,
            pkgs: vec![PkgLine {
                name: pkg.name().to_string(),
                version: pkg.version().to_string(),
            }],
        }
    };
    Ok(QueryReport {
        body,
        diagnostics: Vec::new(),
        error: None,
    })
}

fn query_groups(args: &Args, ctx: &Context) -> QueryReport {
    let alpm = &ctx.alpm;
    let mut lines: Vec<String> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut error = None;

    if args.packages.is_empty() {
        if let Ok(groups) = alpm.localdb().groups() {
            for group in groups {
                if args.quiet {
                    lines.push(group.name().to_string());
                } else {
                    for pkg in group.packages() {
                        lines.push(format!("{} {}", group.name(), pkg.name()));
                    }
                }
            }
        }
        return QueryReport {
            body: QueryBody::Groups(lines),
            diagnostics,
            error,
        };
    }

    let mut any_missing = false;
    for name in &args.packages {
        match alpm.localdb().group(name.as_bytes()) {
            Ok(group) => {
                for pkg in group.packages() {
                    lines.push(format!("{} {}", group.name(), pkg.name()));
                }
            }
            Err(_) => {
                diagnostics.push(format!("error: group '{name}' was not found"));
                any_missing = true;
            }
        }
    }
    if any_missing {
        error = Some(QueryError::NotFound(args.packages.join(", ")));
    }
    QueryReport {
        body: QueryBody::Groups(lines),
        diagnostics,
        error,
    }
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
        assert!(name_or_provides_required("wanted", &[], &req));
        assert!(name_or_provides_required(
            "other",
            &["libz.so=1-64".to_string()],
            &req
        ));
        assert!(!name_or_provides_required(
            "orphan",
            &["unrelated".to_string()],
            &req
        ));
    }
}
