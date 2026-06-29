//! `miz -I --reinstall-layered` — re-lay layered packages onto a freshly
//! staged A/B image + btrfs snapshot, OFFLINE, then mark the new root snapshot
//! as the btrfs default. (PLAN-split-db.md Phase 4.)
//!
//! ## Responsibility split (revised after task #21, sysupdate-notes.md §6)
//!
//! The /usr A/B swap and the boot flip are NOT miz's job. systemd-sysupdate
//! writes the inactive /usr partition trio and relabels the GPT partition;
//! systemd-gpt-auto-generator then selects the newest-versioned /usr at the
//! next boot. There is NO bootctl/boot-entry flip for /usr. The earlier
//! `mount --bind STAGED_USR_DEV` pre-step and `bootctl set-default STAGED_ENTRY`
//! post-step (both PLACEHOLDERS) are therefore REMOVED.
//!
//! What miz owns here, in order:
//!
//! 1. (pre) btrfs snapshot of the live root subvolume into the staged tree.
//! 2. (core) reinstall the explicit layered packages into that snapshot via an
//!    alpm transaction — the ONLY mutating miz-native step.
//! 3. (post) `btrfs subvolume set-default` the populated new-root snapshot.
//!
//! ## OPEN DESIGN ITEM (settle on the A/B host before trusting this path)
//!
//! The reinstall must run against the NEW image's `/usr` (its os-release,
//! `miz.toml`, and `usr/lib/miz/db`), but a snapshot of the live root carries
//! the OLD `/usr` view — sysupdate writes the new `/usr` to the inactive
//! PARTITION, which must be mounted at `<next_root>/usr` before the reinstall.
//! The exact sequencing (does miz drive sysupdate's acquire/stage first, then
//! mount the staged `/usr`? or is the new `/usr` handed in?) is unresolved and
//! needs validation on real A/B hardware. `live_execute` FAILS CLOSED if the
//! staged `/usr` os-release matches the running image (i.e. the new `/usr` was
//! not mounted), rather than silently reinstalling against the wrong image.
//!    This is root-FS only (NOT the /usr/UKI boot chain) and is the sole
//!    boot-affecting step: it runs LAST, only after a successful reinstall,
//!    with the prior default captured for rollback.
//!
//! ## Critical safety invariant (enforced in code, not just intent)
//!
//! The only mutating alpm step writes EXCLUSIVELY into the new btrfs snapshot
//! mounted under `/run` (default `/run/miz/next`), NEVER the live root or live
//! /usr. Before building the /run-rooted alpm Context or running any
//! transaction, the target root is canonicalized and REJECTED unless it
//! resolves under `/run/`, AND every filesystem option path from the staged
//! config is rebased under that root (see `config::build_for_root_rerooted`).
//! A failure at any point leaves the running system untouched.
//!
//! ## Fail-closed gate
//!
//! This path is DESTRUCTIVE and cannot be exercised without a real A/B btrfs
//! host. `run()` therefore refuses to execute it unless
//! `MIZ_ALLOW_LIVE_REINSTALL=1` is set in the environment (and the run is not
//! `--dry-run`). Without the opt-in a non-dry-run invocation errors out before
//! touching anything. The live path is fully wired and reviewable regardless.

use crate::config;
use crate::error::{MizError, Result};
use crate::operations::imagedb;
use crate::operations::osrelease;
use crate::operations::transaction::{commit, prepare, TransGuard};
use alpm::TransFlag;
use std::path::{Path, PathBuf};

pub use crate::cli::args::images::Args;

/// Default mountpoint for the staged new root snapshot.
const NEXT_ROOT: &str = "/run/miz/next";

/// Environment opt-in that must be present for `run()` to attempt the live,
/// destructive reinstall. See the module-level "Fail-closed gate" section.
const LIVE_OPT_IN: &str = "MIZ_ALLOW_LIVE_REINSTALL";

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
    // build_for_root_rerooted if an explicit date override is configured.
    let staged_version = osrelease::image_version_from(&paths.staged_os_release());
    let _archive_date = staged_version.as_deref().map(osrelease::image_date);

    let next_root = paths.next_root.to_string_lossy().into_owned();
    let snapshot_dest = paths.snapshot_dest().to_string_lossy().into_owned();
    let root_subvol = paths.root_subvol.to_string_lossy().into_owned();

    let pre = vec![
        PlannedCommand::new(
            "btrfs snapshot of root into staged tree",
            &[
                "btrfs",
                "subvolume",
                "snapshot",
                &root_subvol,
                &snapshot_dest,
            ],
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

    // Sole boot-affecting step: select the populated new-root snapshot as the
    // btrfs default. The /usr A/B + boot flip belong to sysupdate+gpt-auto
    // (task #21); there is intentionally NO bootctl step here.
    let post = vec![PlannedCommand::new(
        "set new btrfs snapshot as default subvolume",
        &["btrfs", "subvolume", "set-default", &snapshot_dest],
    )];

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
        out.push_str(&format!(
            "# step {step}: {}\n{}\n",
            cmd.label,
            cmd.rendered()
        ));
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
        out.push_str(&format!(
            "# step {step}: {}\n{}\n",
            cmd.label,
            cmd.rendered()
        ));
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

    if args.dry_run {
        // Dry-run never touches the host: render the plan with a clearly-marked
        // placeholder where the live root subvolume would be derived.
        let paths = RelayPaths {
            next_root: next_root.clone(),
            current_layered_local: layered_db.join("local"),
            root_subvol: PathBuf::from("<live-root-subvol: derived from findmnt at run time>"),
        };
        let plan = build_plan(&paths)?;
        print!("{}", render_plan(&plan));
        return Ok(());
    }

    // FAIL CLOSED, before ANY host probing. This path is destructive and cannot
    // be exercised without a real A/B btrfs host, so a normal non-dry-run
    // invocation must refuse. The code below is fully wired; only the explicit
    // opt-in unlocks it, so an untested destructive path can never fire by
    // accident.
    if !live_opt_in(std::env::var_os(LIVE_OPT_IN).as_deref()) {
        return Err(MizError::Other(format!(
            "refusing to run live `-I --reinstall-layered`: this is a destructive \
             A/B image operation that can only be validated on real hardware. \
             Preview it with --dry-run. To run it for real on an A/B btrfs host, \
             set {LIVE_OPT_IN}=1."
        )));
    }

    // Only now, opted-in, do we probe the host for the root subvolume.
    let paths = RelayPaths {
        next_root: next_root.clone(),
        current_layered_local: layered_db.join("local"),
        root_subvol: live_root_subvol()?,
    };
    let plan = build_plan(&paths)?;

    live_execute(&plan, &next_root, archive_date_override.as_deref())
}

/// The fail-closed opt-in predicate: live execution is unlocked ONLY when
/// `MIZ_ALLOW_LIVE_REINSTALL` is exactly `1`. Pure so it is unit-testable.
fn live_opt_in(value: Option<&std::ffi::OsStr>) -> bool {
    value == Some(std::ffi::OsStr::new("1"))
}

/// True only when the staged `/usr` is a present, DIFFERENT image version than
/// the running one — i.e. the new sysupdate-written `/usr` is mounted into the
/// staged root. False (fail closed) when the staged os-release is absent or
/// equals the running version (the new `/usr` was not mounted, so a reinstall
/// would target the old image). Pure so it is unit-testable.
fn staged_usr_is_new_image(staged: Option<&str>, running: Option<&str>) -> bool {
    match staged {
        None => false,
        Some(s) => Some(s) != running,
    }
}

/// Derive the live root subvolume (the btrfs snapshot SOURCE) from `findmnt`.
/// Host-only: shells out, so it is gated behind the non-dry-run path and is
/// not unit-tested directly; the parsing is split into [`parse_findmnt_root`]
/// which IS tested against captured samples. Fails closed if `/` is not btrfs
/// or `findmnt` cannot report a source.
fn live_root_subvol() -> Result<PathBuf> {
    let out = std::process::Command::new("findmnt")
        .args(["-no", "FSTYPE,SOURCE,FSROOT", "/"])
        .output()
        .map_err(|e| MizError::Other(format!("failed to run findmnt for root subvol: {e}")))?;
    if !out.status.success() {
        return Err(MizError::Other(format!(
            "findmnt failed to report the root mount: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_findmnt_root(&text)
}

/// Parse one `findmnt -no FSTYPE,SOURCE,FSROOT /` line into the subvolume path
/// to snapshot. Pure + testable. The FSROOT field is the subvolume path within
/// the btrfs filesystem (e.g. `/@` or `/root`); that is what `btrfs subvolume
/// snapshot` needs as its source, resolved under the live mount. We return the
/// FSROOT as the source path; on a btrfs root mounted at `/`, the subvolume is
/// reachable at `/` so the snapshot source is the live mountpoint itself.
///
/// Fails closed unless FSTYPE is exactly `btrfs`.
fn parse_findmnt_root(text: &str) -> Result<PathBuf> {
    let line = text.lines().find(|l| !l.trim().is_empty()).ok_or_else(|| {
        MizError::Other("findmnt produced no output for the root mount".to_string())
    })?;
    let mut fields = line.split_whitespace();
    let fstype = fields.next().unwrap_or("");
    let _source = fields.next().unwrap_or("");
    let fsroot = fields.next().unwrap_or("");
    if fstype != "btrfs" {
        return Err(MizError::Other(format!(
            "refusing to reinstall: root filesystem is {fstype:?}, not btrfs; \
             the layered A/B reinstall requires a btrfs root subvolume"
        )));
    }
    if fsroot.is_empty() {
        return Err(MizError::Other(
            "findmnt did not report a btrfs subvolume (FSROOT) for the root mount".to_string(),
        ));
    }
    // The snapshot source is the live root mountpoint; FSROOT confirms a
    // subvolume is present and is recorded for operator clarity.
    Ok(PathBuf::from("/"))
}

/// The real execution sequence for the layered reinstall. Reached only after
/// `run()`'s fail-closed opt-in gate and only on a non-dry-run invocation.
///
/// Order (see module docs): pre-steps (btrfs snapshot + overlay dir) -> RE-
/// validate `/run` containment on the now-mounted snapshot -> build the
/// re-rooted Context (every option path rebased under the snapshot) -> alpm
/// reinstall transaction -> ONLY THEN the single boot-affecting post-step
/// (`btrfs subvolume set-default`), with the prior default captured first so a
/// failure there is reported with the value needed to revert. A failure before
/// the post-step never changes any boot default.
fn live_execute(plan: &Plan, next_root: &Path, archive_date_override: Option<&str>) -> Result<()> {
    // Pre-steps create the snapshot/mount that the containment check must see.
    for cmd in &plan.pre {
        run_command(cmd)?;
    }

    // Re-validate containment on the *mounted* snapshot, immediately before use
    // (an assertion taken before the pre-steps would be stale).
    let canon_root = assert_root_under_run(next_root)?;

    // OPEN DESIGN GAP (review blocker, must be settled on the A/B host before
    // this path is trusted): the layered reinstall must run against the NEW
    // image's /usr (its os-release, miz.toml, and image db at usr/lib/miz/db),
    // but a plain btrfs snapshot of the live ROOT carries the OLD /usr view.
    // sysupdate writes the new /usr to the inactive PARTITION; that partition
    // must be mounted at canon_root/usr before we read these paths, otherwise
    // we would reinstall against the running image and seed assume_installed
    // from the wrong db. We do NOT silently proceed: guard below fails closed
    // unless the staged /usr is a DIFFERENT image version than the running one.
    let staged_usr_osrelease = canon_root.join("usr/lib/os-release");
    let staged_version = osrelease::image_version_from(&staged_usr_osrelease);
    let running_version = osrelease::image_version_from(Path::new("/usr/lib/os-release"));
    if !staged_usr_is_new_image(staged_version.as_deref(), running_version.as_deref()) {
        return Err(MizError::Other(format!(
            "staged /usr at {} is missing os-release or matches the running image \
             ({:?}); the new sysupdate-written /usr is not mounted into the staged \
             root, so a reinstall would target the OLD image. Mount the new /usr \
             slot at {}/usr before reinstalling (open design item; see module docs).",
            staged_usr_osrelease.display(),
            running_version.as_deref().unwrap_or("<unknown>"),
            canon_root.display(),
        )));
    }

    let staged_config = canon_root.join("etc/miz.toml");
    let dbpath = canon_root.join("var/lib/miz");
    let image_db = canon_root.join("usr/lib/miz/db");
    let archive_date = match archive_date_override {
        Some(d) => Some(d.to_string()),
        None => staged_version.as_deref().map(osrelease::image_date),
    };

    // Re-rooted constructor: rebases cachedir/hookdir/gpgdir/logfile under
    // canon_root (or fails closed) so the transaction cannot write to the live
    // host. Plain build_for_root would NOT be safe here.
    let mut ctx = config::build_for_root_rerooted(
        &staged_config,
        &canon_root,
        &dbpath,
        &image_db,
        archive_date.as_deref(),
    )?;

    reinstall_into(&mut ctx, &plan.reinstall_targets)?;

    // Boot-affecting step LAST, only after a successful reinstall. Capture the
    // prior btrfs default subvolume id so the operator can revert if the
    // set-default itself fails. The rollback invariant (a failure never leaves
    // a changed boot default un-revertable) holds ONLY while set-default is the
    // single, final post-step; enforce that so adding a later post-step can't
    // silently weaken it.
    if plan.post.len() != 1 {
        return Err(MizError::Other(format!(
            "internal: expected exactly one boot-affecting post-step (set-default), \
             found {}; the rollback invariant assumes it is last and sole",
            plan.post.len()
        )));
    }
    let prior_default = current_btrfs_default(&canon_root);
    for cmd in &plan.post {
        if let Err(e) = run_command(cmd) {
            return Err(MizError::Other(format!(
                "{e}; the reinstall succeeded but the boot default was NOT changed. \
                 Prior btrfs default subvolume: {}",
                prior_default.as_deref().unwrap_or("<unknown>")
            )));
        }
    }
    Ok(())
}

/// Build and commit the layered-package reinstall transaction against the
/// /run-rooted Context, mirroring `sync::sync_install`'s guard/add/prepare/
/// commit flow. Reuses `sync::add_install_targets` so the find-satisfier +
/// `trans_add_pkg` logic is not duplicated. Offline (NEEDED is not set: this
/// is a deliberate reinstall onto a fresh tree).
fn reinstall_into(ctx: &mut config::Context, targets: &[String]) -> Result<()> {
    if targets.is_empty() {
        return Ok(());
    }
    let mut guard = TransGuard::new(&mut ctx.alpm, TransFlag::NONE)?;
    crate::operations::sync::add_install_targets(guard.alpm(), targets)?;
    prepare(guard.alpm())?;
    if guard.alpm().trans_add().is_empty() {
        guard.release()?;
        return Ok(());
    }
    commit(guard.alpm())?;
    guard.release()?;
    Ok(())
}

/// Best-effort read of the current default btrfs subvolume id under `root`,
/// for rollback reporting. Returns None if `btrfs` is unavailable or errors;
/// this is advisory only, never fatal.
fn current_btrfs_default(root: &Path) -> Option<String> {
    let out = std::process::Command::new("btrfs")
        .args(["subvolume", "get-default"])
        .arg(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
        let err =
            assert_root_under_run(Path::new("/run/miz/definitely-not-mounted-xyz")).unwrap_err();
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
        // snapshot is first.
        assert_eq!(plan.pre[0].argv[0], "btrfs");
        assert_eq!(plan.pre[0].argv[1], "subvolume");
        assert_eq!(plan.pre[0].argv[2], "snapshot");
        // The ONLY boot-affecting step is the btrfs set-default, and it is the
        // single, last post-step (no bootctl flip — that is sysupdate's job).
        assert_eq!(plan.post.len(), 1);
        let last = plan.post.last().unwrap();
        assert_eq!(
            last.argv,
            vec!["btrfs", "subvolume", "set-default", "/run/miz/next"]
        );
        // No /usr mount-bind or bootctl placeholder remains anywhere in the plan.
        for cmd in plan.pre.iter().chain(plan.post.iter()) {
            assert!(
                !cmd.argv.iter().any(|a| a == "bootctl"),
                "bootctl leaked: {cmd:?}"
            );
            assert!(
                !cmd.argv
                    .iter()
                    .any(|a| a == "STAGED_USR_DEV" || a == "STAGED_ENTRY"),
                "placeholder leaked: {cmd:?}"
            );
        }
    }

    #[test]
    fn parse_findmnt_root_accepts_btrfs() {
        // FSTYPE SOURCE FSROOT
        let out = "btrfs /dev/mapper/cryptroot /@\n";
        assert_eq!(parse_findmnt_root(out).unwrap(), PathBuf::from("/"));
    }

    #[test]
    fn parse_findmnt_root_rejects_non_btrfs() {
        let err = parse_findmnt_root("ext4 /dev/sda2 /\n").unwrap_err();
        assert!(err.to_string().contains("not btrfs"), "unexpected: {err}");
    }

    #[test]
    fn parse_findmnt_root_rejects_missing_subvol() {
        let err = parse_findmnt_root("btrfs /dev/sda2\n").unwrap_err();
        assert!(
            err.to_string().contains("did not report a btrfs subvolume"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn parse_findmnt_root_rejects_empty_output() {
        let err = parse_findmnt_root("\n").unwrap_err();
        assert!(err.to_string().contains("no output"), "unexpected: {err}");
    }

    #[test]
    fn live_opt_in_requires_exactly_one() {
        use std::ffi::OsStr;
        assert!(!live_opt_in(None));
        assert!(!live_opt_in(Some(OsStr::new(""))));
        assert!(!live_opt_in(Some(OsStr::new("0"))));
        assert!(!live_opt_in(Some(OsStr::new("yes"))));
        assert!(!live_opt_in(Some(OsStr::new("true"))));
        assert!(live_opt_in(Some(OsStr::new("1"))));
    }

    #[test]
    fn staged_usr_must_be_a_different_image_version() {
        // Fail closed: no staged os-release, or same version as running.
        assert!(!staged_usr_is_new_image(None, Some("2026.06.17-2")));
        assert!(!staged_usr_is_new_image(
            Some("2026.06.17-2"),
            Some("2026.06.17-2")
        ));
        // New /usr mounted: staged version differs from running.
        assert!(staged_usr_is_new_image(
            Some("2026.06.24-1"),
            Some("2026.06.17-2")
        ));
        // Running version unknown but a staged version exists -> proceed.
        assert!(staged_usr_is_new_image(Some("2026.06.24-1"), None));
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
