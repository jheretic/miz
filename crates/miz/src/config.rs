use crate::cli::Cli;
use crate::error::{MizError, Result};
use alpm::{Alpm, SigLevel, Usage};
use miz_config::{MizConfig, Repository};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Context {
    pub alpm: alpm::Alpm,
    pub root: PathBuf,
}

const DEFAULT_CONFIG_PATH: &str = "/etc/miz.toml";

/// Build a Context with an optional sync-db file extension override.
///
/// libalpm parses `%FILES%` blocks while reading the syncdb archive on
/// registration. `set_dbext` must therefore be called BEFORE the repos
/// are registered, otherwise repos load from `.db` and `pkg.files()`
/// returns an empty filelist forever.
///
/// `-F` operations pass `Some(".files")`; everything else passes `None`.
pub fn build_with_dbext(cli: &Cli, dbext: Option<&str>) -> Result<Context> {
    let conf = load_config(cli.config.as_deref())?;

    let root = cli
        .root
        .clone()
        .unwrap_or_else(|| conf.options.root_dir.clone());
    let dbpath = cli
        .dbpath
        .clone()
        .unwrap_or_else(|| conf.options.db_path.clone());

    let mut alpm = alpm::Alpm::new(
        root.as_os_str().as_encoded_bytes().to_vec(),
        dbpath.as_os_str().as_encoded_bytes().to_vec(),
    )?;
    if let Some(ext) = dbext {
        alpm.set_dbext(ext);
    }
    apply_config(&mut alpm, &conf)?;

    Ok(Context { alpm, root })
}

fn load_config(override_path: Option<&Path>) -> Result<MizConfig> {
    let path = override_path.unwrap_or(Path::new(DEFAULT_CONFIG_PATH));
    let bytes = fs::read_to_string(path)
        .map_err(|e| MizError::Other(format!("{}: {e}", path.display())))?;
    toml::from_str(&bytes).map_err(|source| MizError::Toml {
        path: path.to_path_buf(),
        source,
    })
}

fn apply_config(alpm: &mut Alpm, conf: &MizConfig) -> Result<()> {
    let opts = &conf.options;
    alpm.set_cachedirs(paths_to_strs(&opts.cache_dir).iter().copied())?;
    alpm.set_hookdirs(paths_to_strs(&opts.hook_dir).iter().copied())?;
    alpm.set_gpgdir(path_to_str(&opts.gpg_dir))?;
    alpm.set_logfile(path_to_str(&opts.log_file))?;
    alpm.set_ignorepkgs(opts.ignore_pkg.iter().map(String::as_str))?;
    alpm.set_ignoregroups(opts.ignore_group.iter().map(String::as_str))?;
    let architectures = resolve_architectures(&opts.architecture);
    alpm.set_architectures(architectures.iter().map(String::as_str))?;
    alpm.set_noupgrades(opts.no_upgrade.iter().map(String::as_str))?;
    alpm.set_noextracts(opts.no_extract.iter().map(String::as_str))?;
    alpm.set_default_siglevel(parse_sig_level(&opts.sig_level))?;
    alpm.set_local_file_siglevel(parse_sig_level(&opts.local_file_sig_level))?;
    alpm.set_remote_file_siglevel(parse_sig_level(&opts.remote_file_sig_level))?;
    alpm.set_use_syslog(opts.use_syslog);
    alpm.set_check_space(opts.check_space);
    alpm.set_disable_dl_timeout(opts.disable_download_timeout);
    alpm.set_parallel_downloads(opts.parallel_downloads as u32);
    let sb_fs = opts.disable_sandbox_filesystem || opts.disable_sandbox;
    let sb_sc = opts.disable_sandbox_syscalls || opts.disable_sandbox;
    alpm.set_disable_sandbox_filesystem(sb_fs);
    alpm.set_disable_sandbox_syscalls(sb_sc);
    alpm.set_sandbox_user(opts.download_user.clone())?;

    for repo in &conf.repos {
        register_repo(alpm, repo)?;
    }
    Ok(())
}

fn register_repo(alpm: &mut Alpm, repo: &Repository) -> Result<()> {
    let sig = if repo.sig_level.is_empty() {
        SigLevel::USE_DEFAULT
    } else {
        parse_sig_level(&repo.sig_level)
    };
    let db = alpm.register_syncdb_mut(repo.name.as_str(), sig)?;
    db.set_servers(repo.servers.iter().map(String::as_str))?;

    let mut usage = Usage::NONE;
    for v in &repo.usage {
        match v.as_str() {
            "Sync" => usage |= Usage::SYNC,
            "Search" => usage |= Usage::SEARCH,
            "Install" => usage |= Usage::INSTALL,
            "Upgrade" => usage |= Usage::UPGRADE,
            "All" => usage = Usage::ALL,
            _ => {}
        }
    }
    if usage == Usage::NONE {
        usage = Usage::ALL;
    }
    db.set_usage(usage)?;
    Ok(())
}

/// Parse pacman.conf-style SigLevel tokens into an alpm bitmask.
///
/// Matches pacman/src/pacman/conf.c::process_siglevel: each token can carry
/// a `Package` or `Database` prefix scoping it; the bare verb (`Optional`,
/// `Required`, `Never`, `TrustedOnly`, `TrustAll`) applies to both. Unknown
/// tokens are silently ignored (mirrors pacman; warnings go to stderr there).
fn parse_sig_level(levels: &[String]) -> SigLevel {
    let mut sig = SigLevel::NONE;
    for level in levels {
        let (verb, package, database) = if let Some(v) = level.strip_prefix("Package") {
            (v, true, false)
        } else if let Some(v) = level.strip_prefix("Database") {
            (v, false, true)
        } else {
            (level.as_str(), true, true)
        };
        match verb {
            "Never" => {
                if package {
                    sig.remove(SigLevel::PACKAGE);
                }
                if database {
                    sig.remove(SigLevel::DATABASE);
                }
            }
            "Optional" => {
                if package {
                    sig.insert(SigLevel::PACKAGE | SigLevel::PACKAGE_OPTIONAL);
                }
                if database {
                    sig.insert(SigLevel::DATABASE | SigLevel::DATABASE_OPTIONAL);
                }
            }
            "Required" => {
                if package {
                    sig.insert(SigLevel::PACKAGE);
                    sig.remove(SigLevel::PACKAGE_OPTIONAL);
                }
                if database {
                    sig.insert(SigLevel::DATABASE);
                    sig.remove(SigLevel::DATABASE_OPTIONAL);
                }
            }
            "TrustedOnly" => {
                if package {
                    sig.remove(SigLevel::PACKAGE_MARGINAL_OK | SigLevel::PACKAGE_UNKNOWN_OK);
                }
                if database {
                    sig.remove(SigLevel::DATABASE_MARGINAL_OK | SigLevel::DATABASE_UNKNOWN_OK);
                }
            }
            "TrustAll" => {
                if package {
                    sig.insert(SigLevel::PACKAGE_MARGINAL_OK | SigLevel::PACKAGE_UNKNOWN_OK);
                }
                if database {
                    sig.insert(SigLevel::DATABASE_MARGINAL_OK | SigLevel::DATABASE_UNKNOWN_OK);
                }
            }
            _ => {}
        }
    }
    sig
}

fn paths_to_strs(paths: &[PathBuf]) -> Vec<&str> {
    paths.iter().filter_map(|p| p.to_str()).collect()
}

/// Expand `"auto"` entries to the running kernel's `uname -m` value,
/// matching `pacman/src/pacman/conf.c::config_add_architecture`. Other
/// tokens pass through unchanged. Falls back to leaving `"auto"` in
/// place if the kernel call fails (libalpm will then reject it; the
/// user sees a clear error rather than a silent arch mismatch).
fn resolve_architectures(input: &[String]) -> Vec<String> {
    let machine = uname_machine();
    input
        .iter()
        .map(|a| match (a.as_str(), &machine) {
            ("auto", Some(m)) => m.clone(),
            _ => a.clone(),
        })
        .collect()
}

fn uname_machine() -> Option<String> {
    // SAFETY: utsname is POD; uname(2) writes into the buffer and returns 0
    // on success. We zero-init so a partial write still produces a valid
    // C-string in `machine`.
    let mut un: libc::utsname = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::uname(&mut un) };
    if rc != 0 {
        return None;
    }
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(un.machine.as_ptr() as *const u8, un.machine.len())
    };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).ok().map(String::from)
}

fn path_to_str(p: &Path) -> &str {
    p.to_str().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_machine_returns_a_value_on_linux() {
        let m = uname_machine().expect("uname(2) should succeed on a Linux host");
        assert!(!m.is_empty(), "uname.machine should not be empty");
    }

    #[test]
    fn resolve_architectures_substitutes_auto_per_token() {
        let m = uname_machine().unwrap();
        let out = resolve_architectures(&[
            "auto".to_string(),
            "x86_64".to_string(),
            "auto".to_string(),
        ]);
        assert_eq!(out, vec![m.clone(), "x86_64".to_string(), m]);
    }

    #[test]
    fn resolve_architectures_leaves_non_auto_alone() {
        let out = resolve_architectures(&["x86_64".to_string(), "aarch64".to_string()]);
        assert_eq!(out, vec!["x86_64".to_string(), "aarch64".to_string()]);
    }
}
