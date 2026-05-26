use crate::config::Context;
use crate::error::{MizError, Result};
use alpm::{Db, Pkg};

pub use crate::cli::args::files::Args;

pub fn run(args: Args, ctx: &mut Context) -> Result<()> {
    if args.list && args.regex {
        return Err(MizError::BadArgs(
            "--regex cannot be used with --list".into(),
        ));
    }

    ctx.alpm.set_dbext(".files");

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
            return Ok(());
        }
        return Err(MizError::BadArgs("no targets specified".into()));
    }

    files_search(&args, ctx)
}

fn files_list(args: &Args, ctx: &Context) -> Result<()> {
    if args.targets.is_empty() {
        for db in ctx.alpm.syncdbs() {
            for pkg in db.pkgs() {
                dump_pkg_files(args, db, pkg);
            }
        }
        return Ok(());
    }

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
                dump_pkg_files(args, db, pkg);
                found = true;
                break;
            }
        }
        if !found {
            eprintln!("error: package '{target}' was not found");
            missing = true;
        }
    }
    if missing {
        return Err(MizError::PackageNotFound(args.targets.join(", ")));
    }
    Ok(())
}

fn dump_pkg_files(args: &Args, db: &Db, pkg: &Pkg) {
    if args.machinereadable {
        for file in pkg.files().files() {
            print_machinereadable(db, pkg, file.name());
        }
        return;
    }
    for file in pkg.files().files() {
        let name = String::from_utf8_lossy(file.name());
        if args.quiet {
            println!("{name}");
        } else {
            println!("{} {}", pkg.name(), name);
        }
    }
}

fn files_search(args: &Args, ctx: &Context) -> Result<()> {
    let mut not_found = Vec::new();

    for target in &args.targets {
        let mut needle = target.as_str();
        let exact_file = needle.contains('/');
        if exact_file {
            while needle.starts_with('/') {
                needle = &needle[1..];
            }
        }

        let re = if args.regex {
            Some(regex::Regex::new(needle)?)
        } else {
            None
        };

        let mut found = false;
        for db in ctx.alpm.syncdbs() {
            for pkg in db.pkgs() {
                let mut matches: Vec<Vec<u8>> = Vec::new();
                let files = pkg.files();

                if exact_file {
                    if let Some(re) = re.as_ref() {
                        for f in files.files() {
                            if re.is_match(&String::from_utf8_lossy(f.name())) {
                                matches.push(f.name().to_vec());
                            }
                        }
                    } else if files.contains(needle.as_bytes()).is_some() {
                        matches.push(needle.as_bytes().to_vec());
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
                            matches.push(f.name().to_vec());
                        }
                    }
                }

                if !matches.is_empty() {
                    print_match(args, db, pkg, &matches, exact_file);
                    found = true;
                }
            }
        }

        if !found {
            not_found.push(target.clone());
        }
    }

    if !not_found.is_empty() {
        return Err(MizError::PackageNotFound(not_found.join(", ")));
    }
    Ok(())
}

fn print_match(args: &Args, db: &Db, pkg: &Pkg, matches: &[Vec<u8>], exact_file: bool) {
    if args.machinereadable {
        for m in matches {
            print_machinereadable(db, pkg, m);
        }
        return;
    }
    if args.quiet {
        println!("{}/{}", db.name(), pkg.name());
        return;
    }
    if exact_file {
        for m in matches {
            let name = String::from_utf8_lossy(m);
            println!(
                "{} is owned by {}/{} {}",
                name,
                db.name(),
                pkg.name(),
                pkg.version()
            );
        }
        return;
    }
    println!("{}/{} {}", db.name(), pkg.name(), pkg.version());
    for m in matches {
        println!("    {}", String::from_utf8_lossy(m));
    }
}

fn print_machinereadable(db: &Db, pkg: &Pkg, filename: &[u8]) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(db.name().as_bytes());
    let _ = out.write_all(&[0]);
    let _ = out.write_all(pkg.name().as_bytes());
    let _ = out.write_all(&[0]);
    let _ = out.write_all(pkg.version().as_str().as_bytes());
    let _ = out.write_all(&[0]);
    let _ = out.write_all(filename);
    let _ = out.write_all(b"\n");
}

fn split_repo_target(target: &str) -> (Option<&str>, &str) {
    match target.split_once('/') {
        Some((repo, name)) => (Some(repo), name),
        None => (None, target),
    }
}
