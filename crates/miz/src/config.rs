use crate::cli::Cli;
use crate::error::{MizError, Result};
use alpm::{Alpm, Depend, SigLevel, Usage};
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
    let dbpath = resolve_dbpath(
        cli.dbpath.as_deref(),
        conf.archetype.as_ref().and_then(|a| a.layered_db.as_deref()),
        &conf.options.db_path,
    );
    let image_db = conf
        .archetype
        .as_ref()
        .and_then(|a| a.image_db.clone());

    // Seed assume_installed from the read-only image db so packages already
    // provided by the immutable /usr are treated as satisfied during dependency
    // resolution, rather than being reinstalled into the layered db. The
    // seeding runs AFTER apply_config (set_dbext + repos already registered —
    // that ordering is load-bearing, see assemble_context) and before any
    // transaction is built, so the existing add_install_targets/prepare path
    // only pulls genuinely-missing deps into the layered db. No change to
    // sync.rs is needed.
    //
    // File placement: layered package files targeting /usr land in the
    // extensions.mutable/usr overlay upper dir; everything else writes to the
    // persistent root. root stays "/", so the overlay routing is transparent to
    // alpm. `-S` on an image without the mutable overlay active will fail to
    // write /usr files — that is the accepted immutable contract.
    assemble_context(&conf, root, dbpath, dbext, image_db.as_deref())
}

/// Build a Context rooted at an arbitrary tree (the `-I --reinstall-layered`
/// relay points this at the new A/B snapshot mounted under `/run`). Reuses the
/// same repo-registration + assume_installed seeding as `build_with_dbext` via
/// `assemble_context` — only the path inputs differ.
///
/// `staged_config` is the staged image's `miz.toml` (its date-pinned archive
/// repos are used as-is). `root`/`dbpath` point into the `/run` snapshot.
/// `image_db` is the NEW image's read-only db. `archive_date`, when set,
/// overrides the staged config's archive repo servers via
/// [`osrelease::archive_url`] (fallback path when the staged repos are not
/// already date-pinned); the staged config's `archive_base` is honored.
///
/// SAFETY: this does NOT itself assert the `/run` containment invariant — the
/// caller (relay) MUST verify `root` canonicalizes under `/run/` before
/// invoking this, since constructing the Alpm handle is a prerequisite to a
/// mutating transaction.
pub fn build_for_root(
    staged_config: &Path,
    root: &Path,
    dbpath: &Path,
    image_db: &Path,
    archive_date: Option<&str>,
) -> Result<Context> {
    let mut conf = load_config(Some(staged_config))?;
    if let Some(date) = archive_date {
        repin_archive_repos(&mut conf, date);
    }
    assemble_context(&conf, root.to_path_buf(), dbpath.to_path_buf(), None, Some(image_db))
}

/// Override the `servers` of the standard Arch repos (`core`/`extra`/
/// `multilib`) with the date-pinned archive snapshot URL. Other repos
/// (e.g. `archetype`) keep their configured servers. Used only by
/// `build_for_root` when an explicit `archive_date` is supplied.
fn repin_archive_repos(conf: &mut MizConfig, date: &str) {
    let base = conf
        .archetype
        .as_ref()
        .and_then(|a| a.archive_base.as_deref());
    let url = crate::operations::osrelease::archive_url(base, date);
    for repo in &mut conf.repos {
        if matches!(repo.name.as_str(), "core" | "extra" | "multilib") {
            repo.servers = vec![url.clone()];
        }
    }
}

/// Shared alpm-construction core: `Alpm::new` -> optional `set_dbext` ->
/// `apply_config` (repos) -> `seed_assume_installed`. The single chokepoint so
/// `build_with_dbext` and `build_for_root` never duplicate the ordering-
/// sensitive setup.
fn assemble_context(
    conf: &MizConfig,
    root: PathBuf,
    dbpath: PathBuf,
    dbext: Option<&str>,
    image_db: Option<&Path>,
) -> Result<Context> {
    let mut alpm = alpm::Alpm::new(
        root.as_os_str().as_encoded_bytes().to_vec(),
        dbpath.as_os_str().as_encoded_bytes().to_vec(),
    )?;
    if let Some(ext) = dbext {
        alpm.set_dbext(ext);
    }
    apply_config(&mut alpm, conf)?;
    if let Some(image_db) = image_db {
        seed_assume_installed(&mut alpm, image_db);
    }
    Ok(Context { alpm, root })
}

/// Pick the alpm localdb path. Precedence: CLI `--dbpath` wins (explicit user
/// override); else `[archetype].layered_db` (split-db installed system); else
/// `[options].db_path` (classic pacman default). Kept as a pure helper so the
/// precedence is unit-testable without constructing a real `Alpm`.
fn resolve_dbpath(
    cli_dbpath: Option<&Path>,
    archetype_layered_db: Option<&Path>,
    options_db_path: &Path,
) -> PathBuf {
    cli_dbpath
        .or(archetype_layered_db)
        .unwrap_or(options_db_path)
        .to_path_buf()
}

/// Feed the image db's provisions to libalpm's assume_installed list.
///
/// Best-effort and NON-FATAL: this runs in the shared `build_with_dbext`, so a
/// malformed image db must not abort every miz invocation (even read-only ones
/// like `-Q`/`-F`). Failures and unparseable provisions are warned and skipped;
/// the worst case is redundant-but-correct layered installs, never a crash.
///
/// Lifetime: `alpm_option_set_assumeinstalled` deep-copies each depend into the
/// handle (libalpm `alpm_dep_dup`), so the temporary `Vec<Depend>` here can be
/// dropped immediately after the call — nothing needs to outlive it or be
/// stored in `Context`. Verified against alpm-5.0.2 handle.rs `set_assume_installed`
/// + the C semantics.
///
/// Each provision is already `name=version` (an EQ-mod entry, the only kind
/// libalpm consults for a versioned dep) or a bare/versioned provides token,
/// per operations::imagedb::provisions.
fn seed_assume_installed(alpm: &mut Alpm, image_db: &Path) {
    let provisions = match crate::operations::imagedb::provisions(image_db) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: could not read image db {}: {e}", image_db.display());
            return;
        }
    };
    // Depend::new panics on an embedded NUL (CString::new().unwrap()) and on a
    // string libalpm can't parse into a dep, so guard before constructing. A
    // bad token is skipped, not fatal.
    let mut deps: Vec<Depend> = Vec::with_capacity(provisions.len());
    for p in provisions {
        if p.as_bytes().contains(&0) {
            eprintln!("warning: skipping image-db provision with NUL byte");
            continue;
        }
        deps.push(Depend::new(p));
    }
    if let Err(e) = alpm.set_assume_installed(deps.iter().map(|d| d.as_dep())) {
        eprintln!("warning: failed to seed assume_installed from image db: {e}");
    }
}

/// Public wrapper around `load_config` for callers (e.g. the `-I` relay) that
/// need the parsed config without building an Alpm handle.
pub fn load_config_public(override_path: Option<&Path>) -> Result<MizConfig> {
    load_config(override_path)
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
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(un.machine.as_ptr() as *const u8, un.machine.len()) };
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
        let out =
            resolve_architectures(&["auto".to_string(), "x86_64".to_string(), "auto".to_string()]);
        assert_eq!(out, vec![m.clone(), "x86_64".to_string(), m]);
    }

    #[test]
    fn resolve_architectures_leaves_non_auto_alone() {
        let out = resolve_architectures(&["x86_64".to_string(), "aarch64".to_string()]);
        assert_eq!(out, vec!["x86_64".to_string(), "aarch64".to_string()]);
    }

    #[test]
    fn resolve_dbpath_cli_wins_over_everything() {
        let got = resolve_dbpath(
            Some(Path::new("/cli/db")),
            Some(Path::new("/var/lib/miz")),
            Path::new("/var/lib/pacman"),
        );
        assert_eq!(got, PathBuf::from("/cli/db"));
    }

    #[test]
    fn resolve_dbpath_layered_db_when_no_cli() {
        let got = resolve_dbpath(
            None,
            Some(Path::new("/var/lib/miz")),
            Path::new("/var/lib/pacman"),
        );
        assert_eq!(got, PathBuf::from("/var/lib/miz"));
    }

    #[test]
    fn resolve_dbpath_falls_back_to_options_db_path() {
        let got = resolve_dbpath(None, None, Path::new("/var/lib/pacman"));
        assert_eq!(got, PathBuf::from("/var/lib/pacman"));
    }

    #[test]
    fn repin_archive_repos_rewrites_arch_repos_only() {
        let src = r#"
            [archetype]
            archive_base = "https://archive.archlinux.org/repos"

            [[repos]]
            name = "core"
            servers = ["https://mirror.example/core"]

            [[repos]]
            name = "archetype"
            servers = ["https://jheretic.github.io/archetype-repo/repo/2026/06/17"]
        "#;
        let mut conf: MizConfig = toml::from_str(src).unwrap();
        repin_archive_repos(&mut conf, "2026/06/17");
        let core = conf.repos.iter().find(|r| r.name == "core").unwrap();
        assert_eq!(
            core.servers,
            vec!["https://archive.archlinux.org/repos/2026/06/17/$repo/os/$arch".to_string()]
        );
        let arch = conf.repos.iter().find(|r| r.name == "archetype").unwrap();
        assert_eq!(
            arch.servers,
            vec!["https://jheretic.github.io/archetype-repo/repo/2026/06/17".to_string()]
        );
    }
}
