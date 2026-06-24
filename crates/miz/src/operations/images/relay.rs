//! `miz -I --reinstall-layered` — re-lay layered packages onto a freshly
//! staged A/B image + btrfs snapshot, OFFLINE, then flip the A/B and
//! default-snapshot defaults. (PLAN-split-db.md Phase 4.)
//!
//! Responsibility split (PLAN Phase 4 table): miz owns ONLY the layered-package
//! alpm transaction (step 5). Everything else (btrfs snapshot/set-default,
//! mount, bootctl/sysupdate A/B flip) is shelled out / D-Bus and is here only
//! to lay out the ordered sequence. The mutating shell/alpm steps are gated
//! behind `!dry_run` AND the `/run` containment safety assertion.
//!
//! CRITICAL SAFETY INVARIANT (enforced in code, not just intent): the only
//! mutating miz step writes EXCLUSIVELY into the new btrfs snapshot mounted
//! under `/run` (default `/run/miz/next`), NEVER the live root or live /usr.
//! Before building the second alpm Context or running any transaction, the
//! target root is canonicalized and REJECTED unless it resolves under `/run/`.
//! A failure leaves the running system untouched.

use crate::config;
use crate::error::{MizError, Result};
use crate::operations::imagedb;
use crate::operations::osrelease;
use std::path::{Path, PathBuf};

pub use crate::cli::args::images::Args;

/// Default mountpoint for the staged new root snapshot + /usr image.
const NEXT_ROOT: &str = "/run/miz/next";

/// A single external command in the orchestration sequence. `argv[0]` is the
/// program; the rest are arguments. Held as data so the dry-run path can print
/// it verbatim and tests can assert on it without executing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedCommand {
    label: &'static str,
    argv: Vec<String>,
}

impl PlannedCommand {
    fn new(label: &'static str, argv: &[&str]) -> Self {
        PlannedCommand {
            label,
            argv: argv.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn rendered(&self) -> String {
        self.argv.join(" ")
    }
}

/// The full ordered plan. `pre` runs before the alpm reinstall (snapshot,
/// mount, overlay prep); `reinstall_targets` is the explicit layered-package
/// list re-added to the transaction against the /run tree; `post` runs after
/// (A/B flip, set-default).
#[derive(Debug)]
struct Plan {
    next_root: PathBuf,
    pre: Vec<PlannedCommand>,
    reinstall_targets: Vec<String>,
    post: Vec<PlannedCommand>,
}

/// Path inputs for the relay, derived from the current config + the staged
/// new tree under /run. Kept as a struct so `build_plan` is a pure function of
/// its inputs and unit-testable against fixtures.
struct RelayPaths {
    next_root: PathBuf,
    /// Current system's layered localdb (`<layered_db>/local`); source of the
    /// explicit-install package list to re-lay.
    current_layered_local: PathBuf,
    /// btrfs subvol of the live root (snapshot source).
    root_subvol: PathBuf,
}

impl RelayPaths {
    fn staged_os_release(&self) -> PathBuf {
        self.next_root.join("usr/lib/os-release")
    }
    fn snapshot_dest(&self) -> PathBuf {
        self.next_root.clone()
    }
}

/// Build the orchestration plan from path inputs. Pure: no execution, no
/// canonicalization side effects. Reads the explicit layered-package list and
/// derives the new image's archive date from the staged os-release.
fn build_plan(paths: &RelayPaths) -> Result<Plan> {
    let reinstall_targets = imagedb::explicit_packages(&paths.current_layered_local)?;

    // Archive date for the NEW image = staged os-release IMAGE_VERSION -> date.
    // Recorded in the plan output for operator visibility; the actual repos are
    // taken from the staged miz.toml (preferred) and only repinned in
    // build_for_root if an explicit date override is configured.
    let staged_version = osrelease::image_version_from(&paths.staged_os_release());
    let _archive_date = staged_version.as_deref().map(osrelease::image_date);

    let next_root = paths.next_root.to_string_lossy().into_owned();
    let snapshot_dest = paths.snapshot_dest().to_string_lossy().into_owned();
    let root_subvol = paths.root_subvol.to_string_lossy().into_owned();

    let pre = vec![
        PlannedCommand::new(
            "btrfs snapshot of root",
            &["btrfs", "subvolume", "snapshot", &root_subvol, &snapshot_dest],
        ),
        PlannedCommand::new(
            "mount staged /usr image onto snapshot",
            &["mount", "--bind", "STAGED_USR_DEV", &format!("{next_root}/usr")],
        ),
        PlannedCommand::new(
            "prepare extensions.mutable/usr overlay upper",
            &[
                "mkdir",
                "-p",
                &format!("{next_root}/var/lib/extensions.mutable/usr"),
            ],
        ),
    ];

    let post = vec![
        PlannedCommand::new(
            "A/B switch: select new image as boot default",
            &["bootctl", "set-default", "STAGED_ENTRY"],
        ),
        PlannedCommand::new(
            "set new btrfs snapshot as default subvolume",
            &["btrfs", "subvolume", "set-default", &snapshot_dest],
        ),
    ];

    Ok(Plan {
        next_root: paths.next_root.clone(),
        pre,
        reinstall_targets,
        post,
    })
}

/// Render the plan to a human-readable, copy-pasteable script for `--dry-run`.
fn render_plan(plan: &Plan) -> String {
    let mut out = String::new();
    out.push_str("# miz -I --reinstall-layered (DRY RUN — nothing executed)\n");
    out.push_str(&format!("# target root: {}\n", plan.next_root.display()));

    let mut step = 1;
    for cmd in &plan.pre {
        out.push_str(&format!("# step {step}: {}\n{}\n", cmd.label, cmd.rendered()));
        step += 1;
    }

    out.push_str(&format!(
        "# step {step}: reinstall {} layered package(s) into {} (miz alpm transaction)\n",
        plan.reinstall_targets.len(),
        plan.next_root.display()
    ));
    if plan.reinstall_targets.is_empty() {
        out.push_str("#   (no explicit layered packages to reinstall)\n");
    } else {
        // Render the FULLY-ROOTED command. A bare `miz -S` would target the
        // live system's root/db/config, not the staged /run tree — dangerous
        // if copy-pasted. Include the staged root/dbpath/config explicitly.
        let r = plan.next_root.display();
        out.push_str(&format!(
            "miz --root {r} --dbpath {r}/var/lib/miz --config {r}/etc/miz.toml -S {}\n",
            plan.reinstall_targets.join(" ")
        ));
    }
    step += 1;

    for cmd in &plan.post {
        out.push_str(&format!("# step {step}: {}\n{}\n", cmd.label, cmd.rendered()));
        step += 1;
    }
    out
}

/// SAFETY INVARIANT enforcement. Canonicalize `root` (resolving symlinks so a
/// symlink cannot smuggle the target outside /run) and reject anything that
/// does not resolve under `/run/`. Returns the canonical path on success.
///
/// This MUST be called before constructing the /run-rooted Alpm Context or
/// running any transaction. A non-existent path also fails (canonicalize
/// errors) — by the time we mutate, the snapshot is mounted and exists.
fn assert_root_under_run(root: &Path) -> Result<PathBuf> {
    let canon = root.canonicalize().map_err(|e| {
        MizError::Other(format!(
            "refusing to reinstall: target root {} does not exist / cannot be canonicalized: {e}",
            root.display()
        ))
    })?;
    if !is_under_run(&canon) {
        return Err(MizError::Other(format!(
            "refusing to reinstall: target root {} does not resolve under /run/ \
             (got {}); aborting to protect the live system",
            root.display(),
            canon.display()
        )));
    }
    Ok(canon)
}

/// Lexical check: does `path` lie under `/run/`? Split out so it is testable
/// without a real /run path on disk. Operates on an already-canonical path.
fn is_under_run(path: &Path) -> bool {
    path.starts_with("/run/") || path == Path::new("/run")
}

pub fn run(args: Args, config_path: Option<&Path>) -> Result<()> {
    let conf = config::load_config_public(config_path)?;

    // Current layered db (source of the explicit-package list) + image db base
    // come from the running system's [archetype] config.
    let arche = conf.archetype.as_ref();
    let layered_db = arche
        .and_then(|a| a.layered_db.clone())
        .unwrap_or_else(|| PathBuf::from("/var/lib/miz"));
    let archive_date_override = arche.and_then(|a| a.archive_date.clone());

    let next_root = PathBuf::from(NEXT_ROOT);
    let paths = RelayPaths {
        next_root: next_root.clone(),
        current_layered_local: layered_db.join("local"),
        // guess: the live root subvol; a real host derives this from
        // findmnt / btrfs. Held as a placeholder path for the plan output.
        root_subvol: PathBuf::from("/"),
    };

    let plan = build_plan(&paths)?;

    if args.dry_run {
        print!("{}", render_plan(&plan));
        return Ok(());
    }

    // FAIL CLOSED: the live path is NOT implemented (the layered-reinstall alpm
    // transaction can't be developed/tested without a real A/B btrfs host), so
    // a non-dry-run invocation must do NOTHING destructive. We return before
    // any pre-step (the btrfs snapshot / mount / mkdir in plan.pre are real
    // commands) and before building any Context. Wiring the live path is
    // deferred; see live_execute() below for the required sequence and the
    // unfinished safety work it must complete first.
    let _ = (&next_root, &archive_date_override); // used by the deferred path
    Err(MizError::Other(
        "live `-I --reinstall-layered` is not implemented yet; use --dry-run \
         to preview the plan. (Requires an A/B btrfs host to develop against.)"
            .to_string(),
    ))
}

/// DEFERRED, NOT WIRED — the real execution sequence for the layered reinstall.
/// `run()` fails closed before reaching anything like this; it is kept as a
/// spec for the future implementer. Two safety items reviewed-but-UNRESOLVED
/// that MUST be done before this is enabled:
///
///   1. Re-root ALL filesystem options, not just the alpm root. `build_for_root`
///      loads the staged miz.toml whose [options] cachedir/logfile/gpgdir/
///      hookdir are absolute and libalpm does NOT prefix them with root — so a
///      staged config with `/var/cache/pacman/pkg`, `/var/log/pacman.log`,
///      `/etc/pacman.d/gnupg`, hook dirs would have the transaction write to
///      the LIVE host even though root is /run. Rebase each under the staged
///      root (or reject any option path not under the staged root) before any
///      transaction. (Reviewer blocker.)
///   2. Validate the snapshot AFTER it is mounted: assert_root_under_run must
///      be re-run on the final mounted snapshot immediately before building the
///      Context (the pre-steps are what create the mount, so an assertion taken
///      before them is stale).
///
/// Ordering: the boot-affecting flip (bootctl set-default) MUST be the LAST
/// step, only after the btrfs set-default and a successful reinstall — and old
/// defaults captured for rollback — so a mid-sequence failure never leaves the
/// machine booting a new /usr against an unpopulated/old root snapshot.
#[allow(dead_code)]
fn live_execute(
    plan: &Plan,
    next_root: &Path,
    archive_date_override: Option<&str>,
) -> Result<()> {
    // Re-validate containment on the *mounted* snapshot, immediately before use.
    let canon_root = assert_root_under_run(next_root)?;

    for cmd in &plan.pre {
        run_command(cmd)?;
    }

    let staged_config = canon_root.join("etc/miz.toml");
    let dbpath = canon_root.join("var/lib/miz");
    let image_db = canon_root.join("usr/lib/miz/db");
    let archive_date = match archive_date_override {
        Some(d) => Some(d.to_string()),
        None => osrelease::image_version_from(&canon_root.join("usr/lib/os-release"))
            .map(|v| osrelease::image_date(&v)),
    };
    // TODO(safety item 1): rebase staged-config option paths under canon_root
    // before this call, or build_for_root must reject non-staged paths.
    let _ctx = config::build_for_root(
        &staged_config,
        &canon_root,
        &dbpath,
        &image_db,
        archive_date.as_deref(),
    )?;
    // TODO: add reinstall_targets to _ctx, prepare, commit.

    // post-steps run last; bootctl flip must be the final boot-affecting action.
    {
        for cmd in &plan.post {
            run_command(cmd)?;
        }
        Ok(())
    }
}

/// Execute one planned external command. Only reached on the non-dry-run path
/// after the safety assertion has passed.
fn run_command(cmd: &PlannedCommand) -> Result<()> {
    let (prog, rest) = cmd
        .argv
        .split_first()
        .ok_or_else(|| MizError::Other(format!("empty command for step '{}'", cmd.label)))?;
    let status = std::process::Command::new(prog)
        .args(rest)
        .status()
        .map_err(|e| MizError::Other(format!("failed to run '{}': {e}", cmd.rendered())))?;
    if !status.success() {
        return Err(MizError::Other(format!(
            "step '{}' failed: {} exited with {status}",
            cmd.label,
            cmd.rendered()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
    }

    #[test]
    fn is_under_run_accepts_run_paths() {
        assert!(is_under_run(Path::new("/run/miz/next")));
        assert!(is_under_run(Path::new("/run")));
    }

    #[test]
    fn is_under_run_rejects_live_paths() {
        assert!(!is_under_run(Path::new("/")));
        assert!(!is_under_run(Path::new("/usr")));
        assert!(!is_under_run(Path::new("/var/lib/miz")));
        // a path that merely starts with the substring but isn't under /run
        assert!(!is_under_run(Path::new("/runtime/x")));
    }

    #[test]
    fn assert_root_rejects_existing_path_outside_run() {
        // A real, canonicalizable dir that is NOT under /run must be rejected.
        let dir = std::env::temp_dir();
        let err = assert_root_under_run(&dir).unwrap_err();
        assert!(
            err.to_string().contains("does not resolve under /run/"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn assert_root_rejects_nonexistent_path() {
        let err = assert_root_under_run(Path::new("/run/miz/definitely-not-mounted-xyz"))
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot be canonicalized"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn build_plan_reads_explicit_targets_and_orders_steps() {
        let paths = RelayPaths {
            next_root: PathBuf::from("/run/miz/next"),
            current_layered_local: fixture("tests/fixtures/layered_db/local"),
            root_subvol: PathBuf::from("/"),
        };
        let plan = build_plan(&paths).unwrap();
        // explicit packages only (implicitpkg reason=1 excluded).
        let mut targets = plan.reinstall_targets.clone();
        targets.sort();
        assert_eq!(targets, vec!["explicitpkg", "noreasonpkg"]);
        // snapshot is first, set-default is last.
        assert_eq!(plan.pre[0].argv[0], "btrfs");
        assert_eq!(plan.pre[0].argv[1], "subvolume");
        assert_eq!(plan.pre[0].argv[2], "snapshot");
        let last = plan.post.last().unwrap();
        assert_eq!(last.argv, vec!["btrfs", "subvolume", "set-default", "/run/miz/next"]);
    }

    #[test]
    fn render_plan_lists_commands_and_reinstall_target() {
        let paths = RelayPaths {
            next_root: PathBuf::from("/run/miz/next"),
            current_layered_local: fixture("tests/fixtures/layered_db/local"),
            root_subvol: PathBuf::from("/"),
        };
        let plan = build_plan(&paths).unwrap();
        let out = render_plan(&plan);
        assert!(out.contains("DRY RUN"));
        assert!(out.contains("btrfs subvolume snapshot / /run/miz/next"));
        assert!(out.contains("btrfs subvolume set-default /run/miz/next"));
        // the reinstall command is rendered FULLY-ROOTED at the staged tree,
        // never a bare `miz -S ` that would hit the live system if pasted.
        assert!(out.contains("miz --root /run/miz/next --dbpath /run/miz/next/var/lib/miz --config /run/miz/next/etc/miz.toml -S "));
        assert!(!out.contains("\nmiz -S "));
        assert!(out.contains("explicitpkg"));
        assert!(out.contains("noreasonpkg"));
        // dependency package must NOT appear as a reinstall target.
        assert!(!out.contains("implicitpkg"));
    }

    #[test]
    fn render_plan_handles_empty_target_list() {
        let paths = RelayPaths {
            next_root: PathBuf::from("/run/miz/next"),
            current_layered_local: PathBuf::from("/nonexistent/local"),
            root_subvol: PathBuf::from("/"),
        };
        let plan = build_plan(&paths).unwrap();
        let out = render_plan(&plan);
        assert!(out.contains("no explicit layered packages to reinstall"));
        assert!(!out.contains("miz -S "));
    }

    #[test]
    fn staged_archive_date_derived_from_staged_os_release() {
        // Reuse the os-release fixture as a stand-in staged tree.
        let paths = RelayPaths {
            next_root: fixture("tests/fixtures/os-release"),
            current_layered_local: PathBuf::from("/nonexistent/local"),
            root_subvol: PathBuf::from("/"),
        };
        let version = osrelease::image_version_from(&paths.staged_os_release());
        assert_eq!(version.as_deref(), Some("2026.06.17-2"));
        assert_eq!(
            version.as_deref().map(osrelease::image_date),
            Some("2026/06/17".to_string())
        );
    }
}
