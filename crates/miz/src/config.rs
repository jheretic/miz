use crate::cli::Cli;
use crate::error::{MizError, Result};
use crate::style::Palette;
use alpm::{Alpm, Depend, LogLevel, SigLevel, Usage};
use miz_config::{MizConfig, Options, Repository};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub struct Context {
    pub alpm: alpm::Alpm,
    pub root: PathBuf,
    /// The read-only baked-in `/usr` image db (`[archetype].image_db`), if
    /// configured. alpm's single localdb is the mutable `/var` layered db; the
    /// image db is a separate grouped tree (not an alpm localdb), so query
    /// operations read it via `operations::imagedb` and union the results.
    pub image_db: Option<PathBuf>,
    /// Resolved terminal color styling for this run (from `[options] color` +
    /// NO_COLOR + TTY detection). Read by the print sites.
    pub palette: Palette,
}

/// User override layer. Optional: absent means "vendor config as-is".
const DEFAULT_CONFIG_PATH: &str = "/etc/miz.toml";

/// Vendor base layer, shipped in the immutable `/usr` image and date-pinned at
/// build time (mkosi.postinst writes the full repo set here). Carrying the repo
/// configuration in `/usr` (beside the image db at `/usr/lib/miz/db`) instead of
/// `/etc` means an A/B update brings the correct, matching repos automatically:
/// the `-Iu` relay mounts the NEW `/usr` into the root snapshot, so `-Syu`
/// resolves against the new version's repos with no string-surgery on a
/// user-editable file. `/etc/miz.toml` is a pure, optional override layer.
const VENDOR_CONFIG_PATH: &str = "/usr/lib/miz/miz.toml";

/// Path of the vendor config relative to a root (for the rerooted relay path).
const VENDOR_CONFIG_REL: &str = "usr/lib/miz/miz.toml";
/// Path of the user override relative to a root.
const USER_CONFIG_REL: &str = "etc/miz.toml";

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
        conf.archetype
            .as_ref()
            .and_then(|a| a.layered_db.as_deref()),
        &conf.options.db_path,
    );
    let image_db = conf.archetype.as_ref().and_then(|a| a.image_db.clone());

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
/// Build a Context rooted at the `/run` snapshot for the `-I` relay, FIRST
/// rebasing every absolute filesystem option path (`cachedir`/`hookdir`/
/// `gpgdir`/`logfile`) under `root` via [`reroot_options`] and rejecting any
/// that escape it.
///
/// libalpm does NOT prefix these option paths with the alpm root, so a staged
/// `miz.toml` whose `[options]` point at `/var/cache/pacman/pkg`,
/// `/var/log/pacman.log`, `/etc/pacman.d/gnupg`, etc. would make a transaction
/// write to the LIVE host even with `root=/run/...`. Re-rooting closes that
/// hole; this is why the relay has no plain non-rerooted constructor.
///
/// The config is loaded LAYERED from `root` (`<root>/usr/lib/miz/miz.toml`
/// vendor base + `<root>/etc/miz.toml` user override), so the NEW image's
/// vendor repos (incl. `[archetype]`, mounted into the snapshot from the new
/// `/usr`) are used — no string surgery on the user file. `root`/`dbpath` point
/// into the `/run` snapshot; `image_db` is the NEW image's read-only db;
/// `archive_date`, when set, repins core/extra/multilib to the archive snapshot
/// via [`crate::operations::osrelease::archive_url`] (the date derives from the
/// new `/usr`'s os-release).
///
/// SAFETY: this does NOT itself assert the `/run` containment invariant — the
/// caller (relay) MUST verify `root` canonicalizes under `/run/` before
/// invoking this, since constructing the Alpm handle is a prerequisite to a
/// mutating transaction.
pub fn build_for_root_rerooted(
    root: &Path,
    dbpath: &Path,
    image_db: &Path,
    archive_date: Option<&str>,
) -> Result<Context> {
    let mut conf = load_layered_from_root(root)?;
    if let Some(date) = archive_date {
        repin_archive_repos(&mut conf, date);
    }
    reroot_options(&mut conf.options, root)?;
    assemble_context(
        &conf,
        root.to_path_buf(),
        dbpath.to_path_buf(),
        None,
        Some(image_db),
    )
}

/// Rebase, in place, every filesystem option path libalpm consumes verbatim
/// (cachedirs, hookdirs, gpgdir, logfile) under `root`. `db_path`/`root_dir`
/// are NOT touched here: the relay passes its own `dbpath`/`root` to
/// `assemble_context`, so `apply_config` never reads `opts.db_path`/`root_dir`.
///
/// Errors (fail-closed) if any path escapes `root` after lexical normalization.
fn reroot_options(opts: &mut Options, root: &Path) -> Result<()> {
    for dir in &mut opts.cache_dir {
        *dir = reroot_under(root, dir)?;
    }
    for dir in &mut opts.hook_dir {
        *dir = reroot_under(root, dir)?;
    }
    opts.gpg_dir = reroot_under(root, &opts.gpg_dir)?;
    opts.log_file = reroot_under(root, &opts.log_file)?;
    Ok(())
}

/// Rebase a single option `path` under `root`. Pure (no filesystem access):
///
/// * a path already under `root` is normalized and kept as-is (idempotent);
/// * an absolute path is reparented onto `root` (its leading `/` stripped);
/// * a relative path is joined onto `root`;
///
/// then the result is lexically normalized (resolving `.`/`..`) and REJECTED
/// if it no longer lies under `root` — so a `..` sequence cannot smuggle the
/// transaction back onto the live host.
fn reroot_under(root: &Path, path: &Path) -> Result<PathBuf> {
    let canon_root = lexical_normalize(root);
    let candidate = if path.starts_with(&canon_root) {
        lexical_normalize(path)
    } else if path.is_absolute() {
        let rel = path.strip_prefix("/").unwrap_or(path);
        lexical_normalize(&canon_root.join(rel))
    } else {
        lexical_normalize(&canon_root.join(path))
    };
    if !candidate.starts_with(&canon_root) {
        return Err(MizError::Other(format!(
            "refusing to reinstall: option path {} escapes staged root {} (resolved to {})",
            path.display(),
            root.display(),
            candidate.display()
        )));
    }
    Ok(candidate)
}

/// Resolve `.` and `..` components purely lexically, with NO filesystem access
/// (the staged paths need not exist yet). A `..` that would climb above the
/// path root is preserved as a literal `..` so the caller's `starts_with`
/// containment check still fails closed rather than silently clamping.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut stack: Vec<Component> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(stack.last(), Some(Component::Normal(_))) {
                    stack.pop();
                } else {
                    stack.push(comp);
                }
            }
            other => stack.push(other),
        }
    }
    stack.iter().map(|c| c.as_os_str()).collect()
}

/// Override the `servers` of the standard Arch repos (`core`/`extra`/
/// `multilib`) with the date-pinned archive snapshot URL. Other repos
/// (e.g. `archetype`) keep their configured servers. Used only by
/// `build_for_root_rerooted` when an explicit `archive_date` is supplied.
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
/// `build_with_dbext` and `build_for_root_rerooted` never duplicate the ordering-
/// sensitive setup.
/// Forward libalpm's log messages into `tracing` so its diagnostics (notably
/// the detailed download/TLS reason behind a terse error like
/// `ALPM_ERR_LIBCURL` "download library error") are actually visible. Without
/// this, alpm's ERROR line is discarded and the user only sees miz's mapped
/// error. ERROR/WARNING surface at the default log level (warn); DEBUG/FUNCTION
/// only with -v. Trailing newline (alpm lines carry one) is trimmed.
fn register_log_cb(alpm: &Alpm) {
    alpm.set_log_cb((), |level, msg, _| {
        let msg = msg.trim_end();
        if msg.is_empty() {
            return;
        }
        if level.contains(LogLevel::ERROR) {
            tracing::error!(target: "alpm", "{msg}");
        } else if level.contains(LogLevel::WARNING) {
            tracing::warn!(target: "alpm", "{msg}");
        } else if level.contains(LogLevel::DEBUG) {
            tracing::debug!(target: "alpm", "{msg}");
        } else {
            tracing::trace!(target: "alpm", "{msg}");
        }
    });
}

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
    register_log_cb(&alpm);
    if let Some(ext) = dbext {
        alpm.set_dbext(ext);
    }
    apply_config(&mut alpm, conf)?;
    if let Some(image_db) = image_db {
        seed_assume_installed(&mut alpm, image_db);
    }
    Ok(Context {
        alpm,
        root,
        image_db: image_db.map(Path::to_path_buf),
        palette: Palette::resolve(conf.options.color),
    })
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
            eprintln!(
                "warning: could not read image db {}: {e}",
                image_db.display()
            );
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

/// Load the effective config by layering the vendor base (`/usr/lib/miz/miz.toml`)
/// under the user override (`/etc/miz.toml`).
///
/// - `override_path = Some(p)`: an explicit `--config p` (or a rerooted staged
///   config). Loaded ALONE, no vendor layering — the caller named an exact file
///   and gets exactly it. (The rerooted relay layers explicitly; see
///   [`load_layered_from_root`].)
/// - `override_path = None`: the normal path. Merge vendor + user with
///   [`merge_config_tables`] (user keys win per-key; repos by name).
///
/// At least one layer must exist; if neither does, the vendor-missing error is
/// surfaced (that is the build-provided file, so its absence is the real fault).
fn load_config(override_path: Option<&Path>) -> Result<MizConfig> {
    if let Some(path) = override_path {
        return load_single(path);
    }
    load_layered(
        Path::new(VENDOR_CONFIG_PATH),
        Path::new(DEFAULT_CONFIG_PATH),
    )
}

/// Load and deserialize one TOML config file with no layering.
fn load_single(path: &Path) -> Result<MizConfig> {
    let bytes = fs::read_to_string(path)
        .map_err(|e| MizError::Other(format!("{}: {e}", path.display())))?;
    toml::from_str(&bytes).map_err(|source| MizError::Toml {
        path: path.to_path_buf(),
        source,
    })
}

/// Layer the user override over the vendor base. Either file may be absent:
/// vendor-only (no `/etc` override) is the common installed case; user-only
/// (no vendor, e.g. a plain non-image host) preserves the pre-split behaviour.
/// Merging is done on the raw TOML tables BEFORE deserialization, because
/// `Options` fields all carry `#[serde(default)]` — after deserialization a
/// defaulted value is indistinguishable from a user-set one, so a struct-level
/// merge could not honour "override only keys the user actually wrote".
fn load_layered(vendor: &Path, user: &Path) -> Result<MizConfig> {
    let vendor_tbl = read_optional_table(vendor)?;
    let user_tbl = read_optional_table(user)?;
    let merged = match (vendor_tbl, user_tbl) {
        (None, None) => {
            return Err(MizError::Other(format!(
                "no miz config found: neither {} (vendor) nor {} (user override) exists",
                vendor.display(),
                user.display()
            )))
        }
        (Some(v), None) => v,
        (None, Some(u)) => u,
        (Some(v), Some(u)) => merge_config_tables(v, u),
    };
    merged.try_into().map_err(|source| MizError::Toml {
        path: user.to_path_buf(),
        source,
    })
}

/// Rerooted variant of [`load_layered`] for the relay: layer
/// `<root>/usr/lib/miz/miz.toml` under `<root>/etc/miz.toml`.
pub fn load_layered_from_root(root: &Path) -> Result<MizConfig> {
    load_layered(&root.join(VENDOR_CONFIG_REL), &root.join(USER_CONFIG_REL))
}

/// Read a TOML file into a `toml::Table`, or `None` if it does not exist.
/// A present-but-unreadable or malformed file is a hard error (fail closed).
fn read_optional_table(path: &Path) -> Result<Option<toml::Table>> {
    match fs::read_to_string(path) {
        Ok(bytes) => {
            let tbl = bytes
                .parse::<toml::Table>()
                .map_err(|source| MizError::Toml {
                    path: path.to_path_buf(),
                    source,
                })?;
            Ok(Some(tbl))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(MizError::Other(format!("{}: {e}", path.display()))),
    }
}

/// Merge the user table over the vendor table (user wins), returning the merged
/// table. Rules, tailored to the miz schema:
/// - `[options]`: per-key override — a key present in user replaces vendor;
///   keys only in vendor survive.
/// - `[archetype]`: same per-key override.
/// - `[[repos]]`: matched by `name`. A user repo with the same name replaces the
///   vendor repo in place (preserving order); a new name is appended after the
///   vendor repos. This lets a user retarget or add a repo without restating
///   the whole vendor set.
/// - any other top-level key: user replaces vendor.
fn merge_config_tables(mut vendor: toml::Table, user: toml::Table) -> toml::Table {
    for (key, uval) in user {
        match key.as_str() {
            "options" | "archetype" => {
                let merged = match (vendor.remove(&key), uval) {
                    (Some(toml::Value::Table(v)), toml::Value::Table(u)) => {
                        toml::Value::Table(merge_sub_table(v, u))
                    }
                    (_, uval) => uval,
                };
                vendor.insert(key, merged);
            }
            "repos" => {
                let merged = match (vendor.remove(&key), uval) {
                    (Some(toml::Value::Array(v)), toml::Value::Array(u)) => {
                        toml::Value::Array(merge_repos_by_name(v, u))
                    }
                    (_, uval) => uval,
                };
                vendor.insert(key, merged);
            }
            _ => {
                vendor.insert(key, uval);
            }
        }
    }
    vendor
}

/// Per-key override of one sub-table (user key replaces vendor key).
fn merge_sub_table(mut vendor: toml::Table, user: toml::Table) -> toml::Table {
    for (k, v) in user {
        vendor.insert(k, v);
    }
    vendor
}

/// Merge `[[repos]]` arrays by the `name` field: same name replaces in place,
/// new name appends. Entries without a string `name` are appended verbatim.
fn merge_repos_by_name(vendor: Vec<toml::Value>, user: Vec<toml::Value>) -> Vec<toml::Value> {
    let repo_name = |v: &toml::Value| {
        v.as_table()
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_string)
    };
    let mut result = vendor;
    for u in user {
        match repo_name(&u) {
            Some(name) => {
                if let Some(slot) = result
                    .iter_mut()
                    .find(|v| repo_name(v).as_deref() == Some(name.as_str()))
                {
                    *slot = u;
                } else {
                    result.push(u);
                }
            }
            None => result.push(u),
        }
    }
    result
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

    // $arch for server-URL substitution: the first resolved architecture, as
    // pacman's frontend does (conf.c). None only if architecture=[] (unusual).
    let arch = architectures.first().map(String::as_str);
    for repo in &conf.repos {
        register_repo(alpm, repo, arch)?;
    }
    Ok(())
}

fn register_repo(alpm: &mut Alpm, repo: &Repository, arch: Option<&str>) -> Result<()> {
    let sig = if repo.sig_level.is_empty() {
        SigLevel::USE_DEFAULT
    } else {
        parse_sig_level(&repo.sig_level)
    };
    let db = alpm.register_syncdb_mut(repo.name.as_str(), sig)?;
    // libalpm does NOT expand $repo/$arch in server URLs -- that's done by
    // pacman's frontend (conf.c), not the library. As a libalpm client miz must
    // do it itself, or the literal "$arch"/"$repo" go to the server and 404.
    let servers: Vec<String> = repo
        .servers
        .iter()
        .map(|s| expand_server_url(s, &repo.name, arch))
        .collect();
    db.set_servers(servers.iter().map(String::as_str))?;

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
/// Expand `$repo` and `$arch` in a server URL, as pacman's frontend does
/// (conf.c: strreplace for each). libalpm does not do this. `$repo` -> the repo
/// name; `$arch` -> the primary resolved architecture. If `$arch` is present but
/// no architecture is known, it is left literal (and will fail loudly at fetch)
/// -- matching pacman, which refuses such a server.
fn expand_server_url(url: &str, repo: &str, arch: Option<&str>) -> String {
    let mut out = url.replace("$repo", repo);
    if let Some(a) = arch {
        out = out.replace("$arch", a);
    }
    out
}

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

    fn tbl(s: &str) -> toml::Table {
        s.parse().unwrap()
    }

    #[test]
    fn merge_options_overrides_per_key_not_wholesale() {
        let vendor =
            tbl("[options]\nparallel_downloads = 5\ncheck_space = true\nhold_pkg = [\"pacman\"]\n");
        let user = tbl("[options]\nparallel_downloads = 10\n");
        let merged = merge_config_tables(vendor, user);
        let opts = merged["options"].as_table().unwrap();
        // user key wins
        assert_eq!(opts["parallel_downloads"].as_integer(), Some(10));
        // vendor-only keys survive (NOT wiped by a wholesale section replace)
        assert_eq!(opts["check_space"].as_bool(), Some(true));
        assert_eq!(opts["hold_pkg"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn merge_repos_by_name_replaces_and_appends_preserving_order() {
        let vendor = tbl(
            "[[repos]]\nname = \"core\"\nservers = [\"https://vendor/core\"]\n\
             [[repos]]\nname = \"archetype\"\nservers = [\"https://vendor/arch\"]\n",
        );
        let user = tbl(
            "[[repos]]\nname = \"archetype\"\nservers = [\"https://user/arch\"]\n\
             [[repos]]\nname = \"aur\"\nservers = [\"https://user/aur\"]\n",
        );
        let merged = merge_config_tables(vendor, user);
        let repos = merged["repos"].as_array().unwrap();
        let names: Vec<&str> = repos
            .iter()
            .map(|r| r.as_table().unwrap()["name"].as_str().unwrap())
            .collect();
        // core kept, archetype replaced in place, aur appended.
        assert_eq!(names, vec!["core", "archetype", "aur"]);
        let arch = repos[1].as_table().unwrap();
        assert_eq!(
            arch["servers"].as_array().unwrap()[0].as_str(),
            Some("https://user/arch")
        );
    }

    #[test]
    fn load_layered_vendor_only_when_no_user_override() {
        let dir = std::env::temp_dir().join(format!("miz-cfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let vendor = dir.join("vendor.toml");
        let user = dir.join("user-absent.toml");
        std::fs::write(&vendor, "[options]\nparallel_downloads = 7\n").unwrap();
        let _ = std::fs::remove_file(&user);
        let conf = load_layered(&vendor, &user).unwrap();
        assert_eq!(conf.options.parallel_downloads, 7);
        let _ = std::fs::remove_file(&vendor);
    }

    #[test]
    fn load_layered_errors_when_neither_layer_exists() {
        let miss_v = Path::new("/nonexistent/vendor.toml");
        let miss_u = Path::new("/nonexistent/user.toml");
        let err = load_layered(miss_v, miss_u).unwrap_err();
        assert!(
            err.to_string().contains("no miz config found"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn uname_machine_returns_a_value_on_linux() {
        let m = uname_machine().expect("uname(2) should succeed on a Linux host");
        assert!(!m.is_empty(), "uname.machine should not be empty");
    }

    #[test]
    fn expand_server_url_substitutes_repo_and_arch() {
        assert_eq!(
            expand_server_url(
                "https://archive.archlinux.org/repos/2026/06/30/$repo/os/$arch",
                "core",
                Some("x86_64"),
            ),
            "https://archive.archlinux.org/repos/2026/06/30/core/os/x86_64"
        );
        // archetype's flat URL has neither wildcard -> unchanged.
        assert_eq!(
            expand_server_url(
                "https://repo.archetype.li/packages/2026/06/30",
                "archetype",
                Some("x86_64")
            ),
            "https://repo.archetype.li/packages/2026/06/30"
        );
        // $arch left literal when no arch known (pacman refuses such a server).
        assert_eq!(
            expand_server_url("https://x/$repo/os/$arch", "extra", None),
            "https://x/extra/os/$arch"
        );
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
    fn reroot_under_reparents_absolute_paths() {
        let root = Path::new("/run/miz/next");
        assert_eq!(
            reroot_under(root, Path::new("/var/cache/pacman/pkg")).unwrap(),
            PathBuf::from("/run/miz/next/var/cache/pacman/pkg")
        );
        assert_eq!(
            reroot_under(root, Path::new("/var/log/pacman.log")).unwrap(),
            PathBuf::from("/run/miz/next/var/log/pacman.log")
        );
    }

    #[test]
    fn reroot_under_is_idempotent_for_already_rooted_paths() {
        let root = Path::new("/run/miz/next");
        assert_eq!(
            reroot_under(root, Path::new("/run/miz/next/var/cache")).unwrap(),
            PathBuf::from("/run/miz/next/var/cache")
        );
    }

    #[test]
    fn reroot_under_joins_relative_paths() {
        let root = Path::new("/run/miz/next");
        assert_eq!(
            reroot_under(root, Path::new("var/log/pacman.log")).unwrap(),
            PathBuf::from("/run/miz/next/var/log/pacman.log")
        );
    }

    #[test]
    fn reroot_under_rejects_dotdot_escape() {
        let root = Path::new("/run/miz/next");
        // An absolute path whose normalized form climbs back out of root.
        let err = reroot_under(root, Path::new("/../../etc/pacman.d/gnupg")).unwrap_err();
        assert!(
            err.to_string().contains("escapes staged root"),
            "unexpected error: {err}"
        );
        // A path already "under" root textually but with .. climbing out.
        let err = reroot_under(root, Path::new("/run/miz/next/../../../etc")).unwrap_err();
        assert!(
            err.to_string().contains("escapes staged root"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reroot_options_rebases_every_fs_path() {
        let mut opts = Options {
            cache_dir: vec![PathBuf::from("/var/cache/pacman/pkg/")],
            hook_dir: vec![PathBuf::from("/etc/pacman.d/hooks/")],
            gpg_dir: PathBuf::from("/etc/pacman.d/gnupg/"),
            log_file: PathBuf::from("/var/log/pacman.log"),
            ..Default::default()
        };
        reroot_options(&mut opts, Path::new("/run/miz/next")).unwrap();
        assert_eq!(
            opts.cache_dir,
            vec![PathBuf::from("/run/miz/next/var/cache/pacman/pkg")]
        );
        assert_eq!(
            opts.hook_dir,
            vec![PathBuf::from("/run/miz/next/etc/pacman.d/hooks")]
        );
        assert_eq!(
            opts.gpg_dir,
            PathBuf::from("/run/miz/next/etc/pacman.d/gnupg")
        );
        assert_eq!(
            opts.log_file,
            PathBuf::from("/run/miz/next/var/log/pacman.log")
        );
    }

    #[test]
    fn reroot_options_propagates_escape_error() {
        let mut opts = Options {
            log_file: PathBuf::from("/run/miz/next/../../../var/log/pacman.log"),
            ..Default::default()
        };
        let err = reroot_options(&mut opts, Path::new("/run/miz/next")).unwrap_err();
        assert!(err.to_string().contains("escapes staged root"));
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
            servers = ["https://repo.archetype.li/packages/2026/06/17"]
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
            vec!["https://repo.archetype.li/packages/2026/06/17".to_string()]
        );
    }
}
