use crate::common::report::{FilePkg, FilesReport};
use crate::config::Context;
use crate::error::{MizError, Result};
use alpm::{Db, Pkg};

use crate::params::files::Params as Args;

pub fn run(args: Args, ctx: &mut Context) -> Result<FilesReport> {
    if args.list && args.regex {
        return Err(MizError::BadArgs(
            "--regex cannot be used with --list".into(),
        ));
    }

    // dbext is already set to '.files' by config::build_with_dbext (dispatched
    // from main.rs based on Operation::Files). Setting it here would be too
    // late: config::apply_config has already registered + loaded the syncdbs.

    if args.refresh > 0 {
        let force = args.refresh >= 2;
        let dbs = ctx.alpm.syncdbs_mut();
        let _ = dbs.update(force)?;
    }

    if args.list {
        return files_list(&args, ctx);
    }

    if args.targets.is_empty() {
        if args.refresh > 0 {
            // Nothing to list/search; an empty search report renders nothing.
            return Ok(FilesReport::Search {
                machine: args.machinereadable,
                quiet: args.quiet,
                matches: Vec::new(),
                error: None,
                regex_error: None,
            });
        }
        return Err(MizError::BadArgs("no targets specified".into()));
    }

    files_search(&args, ctx)
}

fn files_list(args: &Args, ctx: &Context) -> Result<FilesReport> {
    let mut pkgs = Vec::new();
    if args.targets.is_empty() {
        for db in ctx.alpm.syncdbs() {
            for pkg in db.pkgs() {
                pkgs.push(collect_pkg_files(db, pkg));
            }
        }
        return Ok(FilesReport::List {
            machine: args.machinereadable,
            quiet: args.quiet,
            pkgs,
            diagnostics: Vec::new(),
            error: None,
        });
    }

    let mut diagnostics = Vec::new();
    let mut missing = false;
    for target in &args.targets {
        let (repo, name) = split_repo_target(target);
        let mut found = false;
        for db in ctx.alpm.syncdbs() {
            if let Some(r) = repo {
                if db.name() != r {
                    continue;
                }
            }
            if let Ok(pkg) = db.pkg(name.as_bytes()) {
                pkgs.push(collect_pkg_files(db, pkg));
                found = true;
                break;
            }
        }
        if !found {
            diagnostics.push(format!("error: package '{target}' was not found"));
            missing = true;
        }
    }
    Ok(FilesReport::List {
        machine: args.machinereadable,
        quiet: args.quiet,
        pkgs,
        diagnostics,
        error: missing.then(|| args.targets.join(", ")),
    })
}

fn collect_pkg_files(db: &Db, pkg: &Pkg) -> FilePkg {
    FilePkg {
        db: db.name().to_string(),
        pkg: pkg.name().to_string(),
        version: pkg.version().to_string(),
        files: pkg
            .files()
            .files()
            .iter()
            .map(|f| f.name().to_vec())
            .collect(),
        exact_file: false,
    }
}

fn files_search(args: &Args, ctx: &Context) -> Result<FilesReport> {
    let mut not_found = Vec::new();
    let mut matches = Vec::new();
    let mut regex_error = None;

    for target in &args.targets {
        let mut needle = target.as_str();
        let exact_file = needle.contains('/');
        if exact_file {
            while needle.starts_with('/') {
                needle = &needle[1..];
            }
        }

        let re = if args.regex {
            // A compile error on a later target must not discard the matches
            // already gathered: stop scanning and carry the error terminally,
            // matching the original `Regex::new(..)?` which surfaced after the
            // earlier targets had already printed.
            match regex::Regex::new(needle) {
                Ok(re) => Some(re),
                Err(e) => {
                    regex_error = Some(e);
                    break;
                }
            }
        } else {
            None
        };

        let mut found = false;
        for db in ctx.alpm.syncdbs() {
            for pkg in db.pkgs() {
                let mut hits: Vec<Vec<u8>> = Vec::new();
                let files = pkg.files();

                if exact_file {
                    if let Some(re) = re.as_ref() {
                        for f in files.files() {
                            if re.is_match(&String::from_utf8_lossy(f.name())) {
                                hits.push(f.name().to_vec());
                            }
                        }
                    } else if files.contains(needle.as_bytes()).is_some() {
                        hits.push(needle.as_bytes().to_vec());
                    }
                } else {
                    for f in files.files() {
                        let full = String::from_utf8_lossy(f.name());
                        let basename = match full.rsplit_once('/') {
                            Some((_, b)) if !b.is_empty() => b,
                            Some(_) => continue,
                            None => full.as_ref(),
                        };
                        let m = match re.as_ref() {
                            Some(re) => re.is_match(basename),
                            None => basename == needle,
                        };
                        if m {
                            hits.push(f.name().to_vec());
                        }
                    }
                }

                if !hits.is_empty() {
                    matches.push(FilePkg {
                        db: db.name().to_string(),
                        pkg: pkg.name().to_string(),
                        version: pkg.version().to_string(),
                        files: hits,
                        exact_file,
                    });
                    found = true;
                }
            }
        }

        if !found {
            not_found.push(target.clone());
        }
    }

    Ok(FilesReport::Search {
        machine: args.machinereadable,
        quiet: args.quiet,
        matches,
        error: (!not_found.is_empty()).then(|| not_found.join(", ")),
        regex_error,
    })
}

fn split_repo_target(target: &str) -> (Option<&str>, &str) {
    match target.split_once('/') {
        Some((repo, name)) => (Some(repo), name),
        None => (None, target),
    }
}
