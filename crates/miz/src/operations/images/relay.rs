//! Default `miz -Iu` layered-package relay onto a per-version root snapshot.
//!
//! ## What this does, and why
//!
//! An Archetype `/usr` is versioned and updated A/B by systemd-sysupdate; each
//! UKI pins its `/usr` verity hash (`usrhash=`) AND its matching root subvolume
//! (`rootflags=subvol=@archetype_<version>`, baked at image-build time). Layered
//! packages the user installs with `miz -S` live in the ROOT (via the
//! `/usr`-overlay), and can depend on `/usr` package versions. So a `/usr`
//! rollback (systemd-boot boot-assessment selects the older UKI) must also bring
//! a root that matches -- otherwise a layered package could reference a `/usr`
//! dependency version that no longer exists.
//!
//! This relay maintains that invariant. On every `-Iu`, AFTER sysupdate has
//! written the new `/usr` + UKI to the inactive slot, miz:
//!
//! 1. mounts the btrfs top-level and snapshots the running root subvolume
//!    `@archetype_<old>` -> `@archetype_<new>` (named per version, beside each
//!    other on the same btrfs -- NOT into tmpfs `/run`, which a snapshot dest
//!    cannot be),
//! 2. mounts the new snapshot under `/run/miz/next`,
//! 3. verity-opens the NEW `/usr` (located by GPT label `archetype_<new>`, root
//!    hash read from the new UKI's `.cmdline`) and mounts it at
//!    `/run/miz/next/usr` so dependency resolution sees the new image,
//! 4. runs a `-Syu` (refresh + sysupgrade) against that re-rooted tree so
//!    layered packages upgrade to the new version's repos (which now ship in the
//!    new `/usr` as the vendor config layer),
//! 5. tears down the working mounts/verity and prunes root snapshots that no
//!    longer match one of the two kept `/usr` versions.
//!
//! There is NO `btrfs subvolume set-default`: the per-UKI `rootflags=subvol=` is
//! the pinning. A global default would fight a rollback (the older UKI would
//! still boot the newest default subvol). The `/usr` A/B swap + boot flip belong
//! to sysupdate + gpt-auto, not miz.
//!
//! ## Critical safety invariant (enforced in code)
//!
//! The only mutating alpm step writes EXCLUSIVELY into the new snapshot mounted
//! under `/run/miz/next`. Before building the alpm Context or running any
//! transaction, that root is canonicalized and REJECTED unless it resolves under
//! `/run/`, AND every filesystem option path from the (new `/usr`) config is
//! rebased under it (see `config::build_for_root_rerooted`). A failure before
//! the `-Syu` commit leaves the running system untouched; the new snapshot is
//! deleted on any failure, working mounts/verity are always torn down.
//!
//! ## Fail-closed conditions (dev-stage; no in-place migration)
//!
//! - running root has no `@archetype_<old>` subvolume (a pre-feature install):
//!   refuse. Nothing is in production; reinstall to adopt versioned root.
//! - `@archetype_<new>` already exists (an interrupted prior `-Iu`): refuse and
//!   tell the operator to delete it.
//! - root is not btrfs, or the new `/usr`/UKI cannot be located: refuse.

use crate::config;
use crate::error::{MizError, Result};
use crate::operations::osrelease;
use crate::operations::transaction::{commit, prepare, TransGuard};
use alpm::TransFlag;
use object::read::pe::PeFile64;
use object::{Object, ObjectSection};
use std::fs;
use std::path::{Path, PathBuf};

pub use crate::cli::args::images::Args;

/// Working mount for the btrfs top-level (subvolid 5), where the per-version
/// root subvolumes live side by side.
const TOPLEVEL_MOUNT: &str = "/run/miz/toplevel";
/// Working mount for the new root snapshot the `-Syu` writes into.
const NEXT_ROOT: &str = "/run/miz/next";
/// Read-only mount of the new verity `/usr`, used as the overlay lowerdir (the
/// writable overlay is then mounted at `<next>/usr`).
const USR_LOWER_MOUNT: &str = "/run/miz/usr-lower";
/// dm-verity mapper name for the NEW `/usr` opened during the relay. Distinct
/// from the installer's `archetype-usr` and the running system's mapper so it
/// never collides with the active `/usr`.
const USR_VERITY_NEXT: &str = "archetype-usr-next";

/// A single external command. `argv[0]` is the program; the rest are arguments.
/// Held as data so teardown can run commands uniformly and tests can assert on
/// the rendered form without executing anything.
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

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Extract the `usrhash=` value (the new `/usr`'s dm-verity root hash) from a
/// kernel command line. The root hash is the EXTERNAL trust anchor for verity
/// (it is NOT recoverable from the data/hash partitions), so it must come from
/// the trusted, Secure-Boot-signed UKI cmdline. Pure + unit-testable.
fn parse_usrhash_from_cmdline(cmdline: &str) -> Option<String> {
    cmdline
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("usrhash="))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// The per-version root subvolume name, `@archetype_<version>`.
///
/// CROSS-REPO CONTRACT: this MUST byte-match archetype-install's
/// `root_subvol_name` (seeds the subvolume at install time) and the UKI's baked
/// `rootflags=subvol=@archetype_%v` (archetype-build/mkosi.conf). All three
/// derive from the same `IMAGE_VERSION`. Pure + unit-testable.
fn subvol_for_version(version: &str) -> String {
    format!("@archetype_{version}")
}

/// Which `@archetype_<version>` subvolumes to prune. Given every subvolume name
/// at the btrfs top-level and the `/usr` versions the updater keeps
/// (`InstancesMax=2` -> current + prior), return the top-level `@archetype_*`
/// root snapshots whose version is NOT kept.
///
/// Only SINGLE-COMPONENT names are considered: `btrfs subvolume list` reports
/// nested subvolume PATHS too (e.g. `@archetype_<kept>/var/lib/machines/x`),
/// which also start with `@archetype_` but must never be pruned as if they were
/// a stale root. Non-archetype subvolumes and kept versions are never returned.
/// Pure + unit-testable.
fn snapshots_to_prune(existing: &[String], keep_versions: &[&str]) -> Vec<String> {
    let keep: Vec<String> = keep_versions
        .iter()
        .map(|v| subvol_for_version(v))
        .collect();
    existing
        .iter()
        .filter(|name| {
            // top-level root snapshot only: no nested path component.
            !name.contains('/') && name.starts_with("@archetype_") && !keep.contains(*name)
        })
        .cloned()
        .collect()
}

/// Parse `btrfs subvolume list <path>` output into the subvolume path names
/// (the trailing `path <name>` field). Pure + unit-testable.
fn parse_subvol_list(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.split(" path ").nth(1))
        .map(|p| p.trim().to_string())
        .collect()
}

/// Parse the btrfs SOURCE device from `findmnt -no FSTYPE,SOURCE /`. Fails
/// closed unless FSTYPE is exactly `btrfs`. Pure + unit-testable.
fn parse_findmnt_source(text: &str) -> Result<String> {
    let line = text.lines().find(|l| !l.trim().is_empty()).ok_or_else(|| {
        MizError::Other("findmnt produced no output for the root mount".to_string())
    })?;
    let mut fields = line.split_whitespace();
    let fstype = fields.next().unwrap_or("");
    let source = fields.next().unwrap_or("");
    if fstype != "btrfs" {
        return Err(MizError::Other(format!(
            "refusing to relay: root filesystem is {fstype:?}, not btrfs; the \
             versioned-root layered update requires a btrfs root"
        )));
    }
    if source.is_empty() {
        return Err(MizError::Other(
            "findmnt did not report a source device for the root mount".to_string(),
        ));
    }
    // On btrfs, findmnt reports SOURCE with the subvolume fsroot in brackets,
    // e.g. `/dev/mapper/root[/@archetype_old]`. Strip it to the bare block
    // device so `mount -o subvolid=5` gets a real device path.
    let device = source.split('[').next().unwrap_or(source);
    Ok(device.to_string())
}

/// Lexical check: does `path` lie under `/run/`? Operates on an already-
/// canonical path. Pure + unit-testable.
fn is_under_run(path: &Path) -> bool {
    path.starts_with("/run/") || path == Path::new("/run")
}

/// SAFETY INVARIANT: canonicalize `root` (resolving symlinks so a symlink cannot
/// smuggle the target outside `/run`) and reject anything not under `/run/`.
/// MUST be called before constructing the alpm Context or running a transaction.
fn assert_root_under_run(root: &Path) -> Result<PathBuf> {
    let canon = root.canonicalize().map_err(|e| {
        MizError::Other(format!(
            "refusing to relay: target root {} cannot be canonicalized: {e}",
            root.display()
        ))
    })?;
    if !is_under_run(&canon) {
        return Err(MizError::Other(format!(
            "refusing to relay: target root {} does not resolve under /run/ (got \
             {}); aborting to protect the live system",
            root.display(),
            canon.display()
        )));
    }
    Ok(canon)
}

// ---------------------------------------------------------------------------
// Host-only helpers (shell out; not unit-tested, parsing split out above)
// ---------------------------------------------------------------------------

/// Read the `.cmdline` PE section of a UKI (`.efi`). Uses the `object` crate to
/// locate the section rather than hand-parsing PE headers.
fn read_uki_cmdline(uki: &Path) -> Result<String> {
    let data = fs::read(uki)
        .map_err(|e| MizError::Other(format!("cannot read UKI {}: {e}", uki.display())))?;
    let pe = PeFile64::parse(&*data)
        .map_err(|e| MizError::Other(format!("cannot parse UKI {} as PE: {e}", uki.display())))?;
    let section = pe
        .section_by_name(".cmdline")
        .ok_or_else(|| MizError::Other(format!("UKI {} has no .cmdline section", uki.display())))?;
    let bytes = section
        .data()
        .map_err(|e| MizError::Other(format!("cannot read .cmdline of {}: {e}", uki.display())))?;
    Ok(String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .trim()
        .to_string())
}

/// Does the cmdline pin the root subvolume for `version`, i.e. contain
/// `rootflags=subvol=@archetype_<version>`? The whole feature relies on this
/// UKI booting the matching root snapshot, so the relay verifies it rather than
/// trusting the build. `rootflags=` may carry comma-separated btrfs options;
/// check each `subvol=` token. Pure + unit-testable.
fn cmdline_pins_subvol(cmdline: &str, version: &str) -> bool {
    let want = subvol_for_version(version);
    cmdline
        .split_whitespace()
        .filter_map(|tok| tok.strip_prefix("rootflags="))
        .any(|flags| {
            flags
                .split(',')
                .filter_map(|o| o.strip_prefix("subvol="))
                .any(|s| s == want || s == format!("/{want}"))
        })
}

/// The new `/usr`'s verity root hash, read from the new version's UKI on the
/// ESP -- but ONLY after verifying the UKI also pins the matching root
/// subvolume (`rootflags=subvol=@archetype_<version>`). If the UKI booted a
/// different (or no) subvol, the root snapshot we build would not be the one
/// this UKI boots, breaking the rollback invariant -- so fail closed.
fn usrhash_from_uki(uki: &Path, version: &str) -> Result<String> {
    let cmdline = read_uki_cmdline(uki)?;
    if !cmdline_pins_subvol(&cmdline, version) {
        return Err(MizError::Other(format!(
            "UKI {} does not pin rootflags=subvol={} (its cmdline: {cmdline:?}); \
             refusing to relay -- the new UKI would not boot the root snapshot we \
             build, breaking version-matched rollback",
            uki.display(),
            subvol_for_version(version)
        )));
    }
    parse_usrhash_from_cmdline(&cmdline).ok_or_else(|| {
        MizError::Other(format!(
            "UKI {} .cmdline has no usrhash= (cannot open the new /usr verity): {cmdline:?}",
            uki.display()
        ))
    })
}

/// The btrfs SOURCE device backing `/`, via `findmnt`.
fn live_root_device() -> Result<String> {
    let out = std::process::Command::new("findmnt")
        .args(["-no", "FSTYPE,SOURCE", "/"])
        .output()
        .map_err(|e| MizError::Other(format!("failed to run findmnt for root device: {e}")))?;
    if !out.status.success() {
        return Err(MizError::Other(format!(
            "findmnt failed to report the root mount: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    parse_findmnt_source(&String::from_utf8_lossy(&out.stdout))
}

/// The `@archetype_*` subvolume names present at the mounted btrfs top-level.
fn existing_archetype_subvols(toplevel: &Path) -> Result<Vec<String>> {
    let out = std::process::Command::new("btrfs")
        .args(["subvolume", "list", "-o"])
        .arg(toplevel)
        .output()
        .map_err(|e| MizError::Other(format!("failed to list btrfs subvolumes: {e}")))?;
    if !out.status.success() {
        return Err(MizError::Other(format!(
            "btrfs subvolume list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(parse_subvol_list(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .filter(|n| n.starts_with("@archetype_"))
        .collect())
}

/// Locate a block device by GPT partition label via `lsblk`. Fails closed if no
/// partition (or more than one) carries the label.
fn partition_by_label(label: &str) -> Result<PathBuf> {
    let out = std::process::Command::new("lsblk")
        .args(["-rno", "PATH,PARTLABEL"])
        .output()
        .map_err(|e| MizError::Other(format!("failed to run lsblk: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let matches: Vec<&str> = text
        .lines()
        .filter_map(|l| {
            let (path, plabel) = l.split_once(' ')?;
            (plabel.trim() == label).then_some(path)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(PathBuf::from(one)),
        [] => Err(MizError::Other(format!(
            "no partition with GPT label {label:?} found (is the new /usr slot written?)"
        ))),
        _ => Err(MizError::Other(format!(
            "multiple partitions with GPT label {label:?}; refusing to guess which /usr to open"
        ))),
    }
}

/// Does `filename` name the UKI for exactly `version`, per sysupdate's boot-
/// assessment grammar: `archetype_<version>.efi`, `archetype_<version>+<n>.efi`,
/// or `archetype_<version>+<n>-<m>.efi`. Exact match on `<version>` -- a prefix
/// test would wrongly match `2026.07.01-10` for `2026.07.01-1`. Pure + testable.
fn uki_matches_version(filename: &str, version: &str) -> bool {
    let Some(stem) = filename.strip_suffix(".efi") else {
        return false;
    };
    let Some(rest) = stem.strip_prefix("archetype_") else {
        return false;
    };
    // rest is `<version>` optionally followed by `+<tries-left>[-<tries-done>]`.
    match rest.split_once('+') {
        None => rest == version,
        Some((v, tries)) => {
            v == version
                && !tries.is_empty()
                && tries.chars().all(|c| c.is_ascii_digit() || c == '-')
        }
    }
}

/// Find the new version's UKI on a mounted ESP, under `EFI/Linux/`. Fails closed
/// on zero matches or on more than one distinct path (an ambiguous ESP state).
fn find_new_uki(version: &str) -> Result<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for esp in ["/efi", "/boot", "/boot/efi"] {
        let dir = Path::new(esp).join("EFI/Linux");
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if uki_matches_version(&entry.file_name().to_string_lossy(), version) {
                found.push(entry.path());
            }
        }
    }
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(MizError::Other(format!(
            "no UKI for version {version} found on the ESP (/efi,/boot,/boot/efi); \
             the new /usr root hash cannot be read"
        ))),
        _ => Err(MizError::Other(format!(
            "multiple UKIs for version {version} found across ESP mountpoints \
             ({found:?}); refusing to guess which to trust"
        ))),
    }
}

/// ALL UKI files for `version` across ESP mountpoints (the bare name plus any
/// boot-assessment tries-suffixed variants). Used to roll the update back on a
/// relay failure by removing every UKI that could boot the new, unreconciled
/// image. Best-effort enumeration (unreadable dirs skipped); the removal itself
/// is `rm -f`, so a missing file is harmless.
fn uki_files_for_version(version: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for esp in ["/efi", "/boot", "/boot/efi"] {
        let dir = Path::new(esp).join("EFI/Linux");
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if uki_matches_version(&entry.file_name().to_string_lossy(), version) {
                out.push(entry.path());
            }
        }
    }
    out
}

/// Run one external command, mapping a non-zero exit to a fail-closed error.
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

/// One entry on the teardown stack: a command plus whether it should run only
/// on failure (e.g. deleting the half-built snapshot) or always (unmounts,
/// verity close).
struct TeardownStep {
    cmd: PlannedCommand,
    failure_only: bool,
}

/// Ordered teardown for the relay's transient host state, as a SINGLE stack so
/// drop runs the exact reverse of setup. The order is load-bearing: the
/// snapshot delete needs the snapshot UNMOUNTED but the btrfs top-level STILL
/// MOUNTED, so it must sit between the snapshot-unmount and the top-level
/// unmount. A two-list "always then on_failure" split cannot express that
/// interleaving (it would unmount the top-level before the delete, leaking the
/// snapshot). Push in setup order; drop pops in reverse.
#[derive(Default)]
struct Teardown {
    steps: Vec<TeardownStep>,
    committed: bool,
}

impl Teardown {
    fn always(&mut self, cmd: PlannedCommand) {
        self.steps.push(TeardownStep {
            cmd,
            failure_only: false,
        });
    }
    fn on_failure(&mut self, cmd: PlannedCommand) {
        self.steps.push(TeardownStep {
            cmd,
            failure_only: true,
        });
    }
    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for Teardown {
    fn drop(&mut self) {
        while let Some(step) = self.steps.pop() {
            if step.failure_only && self.committed {
                continue;
            }
            // Surface teardown failures: a failed unmount/close/delete can leak
            // a mount, the verity mapper, or the half-built snapshot, and the
            // caller only sees the primary error. Best-effort (Drop can't fail).
            if let Err(e) = run_command(&step.cmd) {
                eprintln!("warning: relay teardown step failed (possible leak): {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Dry-run plan rendering
// ---------------------------------------------------------------------------

/// Human-readable preview of the relay for `--dry-run`. Versions may be unknown
/// when previewing off a non-Archetype host; those render as placeholders.
struct RelayPlan {
    old_version: String,
    new_version: String,
}

impl RelayPlan {
    fn new(old: Option<&str>, new: Option<&str>) -> Self {
        RelayPlan {
            old_version: old.unwrap_or("<running-version>").to_string(),
            new_version: new.unwrap_or("<new-version>").to_string(),
        }
    }
}

fn render_plan(plan: &RelayPlan) -> String {
    let old = subvol_for_version(&plan.old_version);
    let new = subvol_for_version(&plan.new_version);
    format!(
        "# miz -Iu layered-package root relay (DRY RUN — nothing executed)\n\
         # old root subvolume: {old}\n\
         # new root subvolume: {new}\n\
         # step 1: mount btrfs top-level at {TOPLEVEL_MOUNT}\n\
         # step 2: fail closed if {new} exists; require {old} to exist\n\
         # step 3: btrfs subvolume snapshot {TOPLEVEL_MOUNT}/{old} {TOPLEVEL_MOUNT}/{new}\n\
         # step 4: mount subvol={new} at {NEXT_ROOT}\n\
         # step 5: open new /usr (label archetype_{nv}) with usrhash from its UKI, mount at {NEXT_ROOT}/usr\n\
         # step 6: miz --root {NEXT_ROOT} -Syu (upgrade layered packages against the new image's repos)\n\
         # step 7: tear down mounts/verity; prune root snapshots except [{new}, {old}]\n\
         # note: NO btrfs set-default — each UKI's rootflags=subvol= is the pinning\n",
        nv = plan.new_version,
    )
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// `miz -I --reinstall-layered [<version>]`: run the relay standalone (e.g. to
/// preview with `--dry-run`, or re-run after a manual sysupdate). The primary
/// path is [`relay_after_upgrade`], invoked automatically by `-Iu`.
pub fn run(args: Args, _config_path: Option<&Path>) -> Result<()> {
    let running = osrelease::booted_image_version();
    let new_version = args.targets.first().cloned();

    if args.dry_run {
        print!(
            "{}",
            render_plan(&RelayPlan::new(running.as_deref(), new_version.as_deref()))
        );
        return Ok(());
    }

    let new_version = new_version.ok_or_else(|| {
        MizError::Other(
            "provide the new image version as the target (miz -I --reinstall-layered \
             <version>), or run `miz -Iu` which relays automatically after updating"
                .to_string(),
        )
    })?;
    live_execute(&new_version, args.quiet)
}

/// Invoked by `images_upgrade` after sysupdate has installed `new_version` to
/// the inactive slot. Snapshots the root per version and upgrades layered
/// packages against the new image (see module docs).
pub fn relay_after_upgrade(new_version: &str, dry_run: bool, quiet: bool) -> Result<()> {
    if dry_run {
        let running = osrelease::booted_image_version();
        print!(
            "{}",
            render_plan(&RelayPlan::new(running.as_deref(), Some(new_version)))
        );
        return Ok(());
    }
    live_execute(new_version, quiet)
}

// ---------------------------------------------------------------------------
// Orchestration (host-only)
// ---------------------------------------------------------------------------

/// The real relay sequence. Nested so cleanup unwinds in the correct order via
/// [`Teardown`]. Fails closed at each precondition; only the final `-Syu` commit
/// mutates persistent state, and the new snapshot is deleted on any failure.
fn live_execute(new_version: &str, quiet: bool) -> Result<()> {
    let running = osrelease::booted_image_version().ok_or_else(|| {
        MizError::Other(
            "cannot determine the running IMAGE_VERSION from os-release; cannot name \
             the root subvolume to snapshot"
                .to_string(),
        )
    })?;
    if new_version == running {
        return Err(MizError::Other(format!(
            "new version {new_version} equals the running version; nothing to relay"
        )));
    }
    let old_subvol = subvol_for_version(&running);
    let new_subvol = subvol_for_version(new_version);

    let device = live_root_device()?;
    let toplevel = Path::new(TOPLEVEL_MOUNT);
    fs::create_dir_all(toplevel)
        .map_err(|e| MizError::Other(format!("cannot create {TOPLEVEL_MOUNT}: {e}")))?;

    let mut td = Teardown::default();

    // Mount the btrfs top-level (subvolid 5) where the per-version subvolumes
    // live. Snapshots must target the same btrfs filesystem -- NOT tmpfs /run.
    run_command(&PlannedCommand::new(
        "mount btrfs top-level",
        &["mount", "-o", "subvolid=5", &device, TOPLEVEL_MOUNT],
    ))?;
    td.always(PlannedCommand::new(
        "unmount btrfs top-level",
        &["umount", TOPLEVEL_MOUNT],
    ));

    let old_path = toplevel.join(&old_subvol);
    let new_path = toplevel.join(&new_subvol);

    // Fail closed: no in-place migration of a pre-feature plain root (dev-stage).
    if !old_path.exists() {
        return Err(MizError::Other(format!(
            "running root has no {old_subvol} subvolume at the btrfs top-level; this \
             system predates versioned-root. Reinstall to adopt it (no in-place \
             migration)."
        )));
    }
    // Fail closed: a leftover new subvolume from an interrupted prior -Iu.
    if new_path.exists() {
        return Err(MizError::Other(format!(
            "{new_subvol} already exists at the btrfs top-level (interrupted prior \
             update?). Delete it and retry: btrfs subvolume delete {}",
            new_path.display()
        )));
    }

    // Snapshot old -> new. Deleted on any later failure (on_failure), kept on
    // success. Must be deleted while the top-level is still mounted, so it is
    // registered on the failure stack BELOW the top-level unmount.
    run_command(&PlannedCommand::new(
        "snapshot root subvolume for the new version",
        &[
            "btrfs",
            "subvolume",
            "snapshot",
            &old_path.display().to_string(),
            &new_path.display().to_string(),
        ],
    ))?;
    td.on_failure(PlannedCommand::new(
        "delete the half-built new snapshot",
        &[
            "btrfs",
            "subvolume",
            "delete",
            &new_path.display().to_string(),
        ],
    ));

    // Mount the new snapshot as the seed root.
    let next_root = Path::new(NEXT_ROOT);
    fs::create_dir_all(next_root)
        .map_err(|e| MizError::Other(format!("cannot create {NEXT_ROOT}: {e}")))?;
    run_command(&PlannedCommand::new(
        "mount the new root snapshot",
        &[
            "mount",
            "-o",
            &format!("subvol={new_subvol}"),
            &device,
            NEXT_ROOT,
        ],
    ))?;
    td.always(PlannedCommand::new(
        "unmount the new root snapshot",
        &["umount", NEXT_ROOT],
    ));

    // Open + mount the NEW /usr into the snapshot so dependency resolution sees
    // the new image (its os-release, vendor miz.toml with the new repos, and db).
    let usr_data = partition_by_label(&format!("archetype_{new_version}"))?;
    let usr_hash_dev = partition_by_label(&format!("archetype_{new_version}_verity"))?;
    let uki = find_new_uki(new_version)?;
    let roothash = usrhash_from_uki(&uki, new_version)?;

    // ATOMICITY: sysupdate already wrote the new UKI (rootflags=subvol=<new>,
    // TriesLeft=3) to the ESP BEFORE this relay ran. If the relay now fails, the
    // half-applied update (new /usr bootable, layered packages NOT reconciled)
    // is exactly the inconsistency this feature prevents -- so roll the whole
    // update back by removing the new UKI(s). Without a matching UKI the new
    // /usr slot is inert (systemd-boot selects by UKI) and is overwritten by the
    // next update; the system keeps booting the old UKI+/usr+@archetype_<old>.
    // Armed here (once the UKI is located) so every later failure triggers it;
    // pushed BELOW the snapshot-delete so ordering is: delete snapshot, then
    // remove UKI, then unmount top-level.
    for uki_file in uki_files_for_version(new_version) {
        td.on_failure(PlannedCommand::new(
            "roll back: remove the new UKI so the failed update is not boot-selected",
            &["rm", "-f", &uki_file.display().to_string()],
        ));
    }

    // Clear any stale mapper from an aborted run before opening.
    if Path::new(&format!("/dev/mapper/{USR_VERITY_NEXT}")).exists() {
        let _ = run_command(&PlannedCommand::new(
            "close stale new-/usr verity mapper",
            &["veritysetup", "close", USR_VERITY_NEXT],
        ));
    }
    run_command(&PlannedCommand::new(
        "verity-open the new /usr",
        &[
            "veritysetup",
            "open",
            &usr_data.display().to_string(),
            USR_VERITY_NEXT,
            &usr_hash_dev.display().to_string(),
            &roothash,
        ],
    ))?;
    td.always(PlannedCommand::new(
        "close the new /usr verity mapper",
        &["veritysetup", "close", USR_VERITY_NEXT],
    ));

    // The new /usr is dm-verity -> READ-ONLY. Layered packages write files into
    // /usr, so we reproduce the running system's systemd-sysext mutable overlay:
    // lowerdir = the verity /usr (ro), upperdir = the snapshot's
    // /var/lib/extensions.mutable/usr (writable, travels with the root subvol),
    // mounted at <next>/usr. A plain ro mount would make any /usr write fail.
    let usr_lower = Path::new(USR_LOWER_MOUNT);
    fs::create_dir_all(usr_lower)
        .map_err(|e| MizError::Other(format!("cannot create {USR_LOWER_MOUNT}: {e}")))?;
    run_command(&PlannedCommand::new(
        "mount the new /usr (overlay lowerdir, ro)",
        &[
            "mount",
            "-o",
            "ro",
            &format!("/dev/mapper/{USR_VERITY_NEXT}"),
            USR_LOWER_MOUNT,
        ],
    ))?;
    td.always(PlannedCommand::new(
        "unmount the /usr overlay lowerdir",
        &["umount", USR_LOWER_MOUNT],
    ));

    let usr_upper = next_root.join("var/lib/extensions.mutable/usr");
    let usr_work = next_root.join("var/lib/extensions.mutable/.usr-work");
    fs::create_dir_all(&usr_upper)
        .map_err(|e| MizError::Other(format!("cannot create /usr overlay upper: {e}")))?;
    fs::create_dir_all(&usr_work)
        .map_err(|e| MizError::Other(format!("cannot create /usr overlay work: {e}")))?;

    let usr_mount = next_root.join("usr");
    run_command(&PlannedCommand::new(
        "mount the writable /usr overlay into the snapshot",
        &[
            "mount",
            "-t",
            "overlay",
            "archetype-usr-overlay",
            "-o",
            &format!(
                "lowerdir={USR_LOWER_MOUNT},upperdir={},workdir={}",
                usr_upper.display(),
                usr_work.display()
            ),
            &usr_mount.display().to_string(),
        ],
    ))?;
    td.always(PlannedCommand::new(
        "unmount the /usr overlay",
        &["umount", &usr_mount.display().to_string()],
    ));

    // Re-validate /run containment on the mounted snapshot, immediately before use.
    let canon_root = assert_root_under_run(next_root)?;

    // Confirm the mounted /usr really is the new image (guards a mislabeled or
    // stale slot from silently upgrading against the wrong version).
    let staged_version = osrelease::image_version_from(&canon_root.join("usr/lib/os-release"));
    if staged_version.as_deref() != Some(new_version) {
        return Err(MizError::Other(format!(
            "mounted /usr reports version {:?}, expected {new_version}; refusing to \
             upgrade against the wrong image",
            staged_version.as_deref().unwrap_or("<unknown>")
        )));
    }

    let dbpath = canon_root.join("var/lib/miz");
    let image_db = canon_root.join("usr/lib/miz/db");
    let archive_date = staged_version.as_deref().map(osrelease::image_date);

    // Layered config from the snapshot root: vendor /usr/lib/miz/miz.toml (from
    // the NEW /usr, carrying the new version's repos) + optional /etc override.
    // All filesystem option paths are rebased under canon_root (fail-closed) so
    // the transaction cannot touch the live host.
    let mut ctx =
        config::build_for_root_rerooted(&canon_root, &dbpath, &image_db, archive_date.as_deref())?;

    sysupgrade_into(&mut ctx, quiet)?;

    // Success: keep the new snapshot; prune root snapshots that no longer match
    // one of the two kept /usr versions (new + old, mirroring InstancesMax=2).
    td.commit();
    prune_old_snapshots(toplevel, &[new_version, &running], quiet);

    if !quiet {
        println!("relayed layered packages onto {new_subvol}");
    }
    Ok(())
}

/// Refresh the sync dbs and upgrade all layered packages (`-Syu`) against the
/// re-rooted Context. Mirrors `sync::sync_install`'s guard/prepare/commit flow.
/// A no-op transaction (nothing to upgrade) releases cleanly.
fn sysupgrade_into(ctx: &mut config::Context, _quiet: bool) -> Result<()> {
    {
        let dbs = ctx.alpm.syncdbs_mut();
        dbs.update(false)?;
    }
    let mut guard = TransGuard::new(&mut ctx.alpm, TransFlag::NONE)?;
    guard.alpm().sync_sysupgrade(false)?;
    prepare(guard.alpm())?;
    if guard.alpm().trans_add().is_empty() {
        guard.release()?;
        return Ok(());
    }
    commit(guard.alpm())?;
    guard.release()?;
    Ok(())
}

/// Delete `@archetype_*` root snapshots not matching one of `keep_versions`.
/// Best-effort: a failed delete is logged (when not quiet) but never aborts the
/// completed update. The top-level must be mounted at `toplevel`.
fn prune_old_snapshots(toplevel: &Path, keep_versions: &[&str], quiet: bool) {
    let existing = match existing_archetype_subvols(toplevel) {
        Ok(v) => v,
        Err(e) => {
            if !quiet {
                eprintln!("warning: could not list root snapshots to prune: {e}");
            }
            return;
        }
    };
    for name in snapshots_to_prune(&existing, keep_versions) {
        let path = toplevel.join(&name);
        let cmd = PlannedCommand::new(
            "prune old root snapshot",
            &["btrfs", "subvolume", "delete", &path.display().to_string()],
        );
        if let Err(e) = run_command(&cmd) {
            if !quiet {
                eprintln!("warning: could not prune {name}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
    }

    #[test]
    fn parse_usrhash_from_cmdline_extracts_token() {
        assert_eq!(
            parse_usrhash_from_cmdline("root=x usrhash=abc123 splash quiet").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            parse_usrhash_from_cmdline("  usrhash=deadbeef  ").as_deref(),
            Some("deadbeef")
        );
        assert_eq!(parse_usrhash_from_cmdline("root=x splash quiet"), None);
        assert_eq!(parse_usrhash_from_cmdline("usrhash="), None);
    }

    #[test]
    fn read_uki_cmdline_and_usrhash_from_fixture() {
        let uki = fixture("tests/fixtures/uki-cmdline.efi");
        let cmdline = read_uki_cmdline(&uki).unwrap();
        assert!(
            cmdline.contains("root=/dev/gpt-auto-root"),
            "unexpected cmdline: {cmdline:?}"
        );
        // The fixture pins subvol @archetype_2026.07.01-7, so the version-checked
        // extractor succeeds for that version and yields the usrhash.
        assert_eq!(
            usrhash_from_uki(&uki, "2026.07.01-7").unwrap(),
            "deadbeefcafe1234567890abcdef"
        );
        // A mismatched version fails closed (UKI would boot a different subvol).
        assert!(usrhash_from_uki(&uki, "2026.06.01-1").is_err());
    }

    #[test]
    fn cmdline_pins_subvol_matches_exact_version() {
        let cl = "root=x usrhash=abc rootflags=subvol=@archetype_2026.07.01-7 quiet";
        assert!(cmdline_pins_subvol(cl, "2026.07.01-7"));
        assert!(!cmdline_pins_subvol(cl, "2026.07.01-1"));
        // leading-slash form and comma-joined btrfs options both accepted.
        assert!(cmdline_pins_subvol(
            "rootflags=compress=zstd,subvol=/@archetype_2026.07.01-7",
            "2026.07.01-7"
        ));
        // no rootflags at all -> not pinned.
        assert!(!cmdline_pins_subvol(
            "root=x usrhash=abc quiet",
            "2026.07.01-7"
        ));
    }

    #[test]
    fn uki_matches_version_is_exact_with_tries_suffix() {
        assert!(uki_matches_version(
            "archetype_2026.07.01-1.efi",
            "2026.07.01-1"
        ));
        assert!(uki_matches_version(
            "archetype_2026.07.01-1+3.efi",
            "2026.07.01-1"
        ));
        assert!(uki_matches_version(
            "archetype_2026.07.01-1+2-1.efi",
            "2026.07.01-1"
        ));
        // must NOT match a different version that shares a prefix.
        assert!(!uki_matches_version(
            "archetype_2026.07.01-10.efi",
            "2026.07.01-1"
        ));
        assert!(!uki_matches_version(
            "archetype_2026.07.01-1.efi",
            "2026.07.01-10"
        ));
        assert!(!uki_matches_version("systemd-bootx64.efi", "2026.07.01-1"));
    }

    #[test]
    fn parse_findmnt_source_strips_btrfs_subvol_bracket() {
        assert_eq!(
            parse_findmnt_source("btrfs /dev/mapper/archetype-root[/@archetype_2026.06.01-1]\n")
                .unwrap(),
            "/dev/mapper/archetype-root"
        );
    }

    #[test]
    fn snapshots_to_prune_ignores_nested_subvol_paths() {
        let existing = vec![
            "@archetype_2026.06.01-1".to_string(),
            "@archetype_2026.07.01-7".to_string(),
            // nested subvol inside a KEPT root snapshot: must never be pruned.
            "@archetype_2026.07.01-7/var/lib/machines/vm".to_string(),
        ];
        let pruned = snapshots_to_prune(&existing, &["2026.07.01-7", "2026.06.15-2"]);
        assert_eq!(pruned, vec!["@archetype_2026.06.01-1"]);
    }

    #[test]
    fn subvol_name_matches_cross_repo_contract() {
        assert_eq!(
            subvol_for_version("2026.07.01-7"),
            "@archetype_2026.07.01-7"
        );
    }

    #[test]
    fn snapshots_to_prune_keeps_only_the_two_usr_versions() {
        let existing = vec![
            "@archetype_2026.06.01-1".to_string(),
            "@archetype_2026.06.15-2".to_string(),
            "@archetype_2026.07.01-7".to_string(),
            "@snapshots".to_string(),
            "@home".to_string(),
        ];
        let keep = ["2026.07.01-7", "2026.06.15-2"];
        let mut pruned = snapshots_to_prune(&existing, &keep);
        pruned.sort();
        assert_eq!(pruned, vec!["@archetype_2026.06.01-1"]);
    }

    #[test]
    fn snapshots_to_prune_never_touches_non_archetype_subvols() {
        let existing = vec!["@home".to_string(), "@var".to_string(), "root".to_string()];
        assert!(snapshots_to_prune(&existing, &["2026.07.01-7"]).is_empty());
    }

    #[test]
    fn parse_subvol_list_extracts_paths() {
        let text = "ID 256 gen 9 top level 5 path @archetype_2026.07.01-7\n\
                    ID 257 gen 9 top level 5 path @home\n";
        assert_eq!(
            parse_subvol_list(text),
            vec!["@archetype_2026.07.01-7", "@home"]
        );
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
        assert!(!is_under_run(Path::new("/runtime/x")));
    }

    #[test]
    fn assert_root_rejects_existing_path_outside_run() {
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
    fn parse_findmnt_source_accepts_btrfs() {
        assert_eq!(
            parse_findmnt_source("btrfs /dev/mapper/archetype-root\n").unwrap(),
            "/dev/mapper/archetype-root"
        );
    }

    #[test]
    fn parse_findmnt_source_rejects_non_btrfs() {
        let err = parse_findmnt_source("ext4 /dev/sda2\n").unwrap_err();
        assert!(err.to_string().contains("not btrfs"), "unexpected: {err}");
    }

    #[test]
    fn parse_findmnt_source_rejects_missing_device() {
        let err = parse_findmnt_source("btrfs\n").unwrap_err();
        assert!(
            err.to_string().contains("did not report a source device"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn render_plan_lists_named_subvol_sequence() {
        let plan = RelayPlan::new(Some("2026.06.15-2"), Some("2026.07.01-7"));
        let out = render_plan(&plan);
        assert!(out.contains("DRY RUN"));
        assert!(out.contains("@archetype_2026.06.15-2"));
        assert!(out.contains("@archetype_2026.07.01-7"));
        assert!(out.contains("subvolume snapshot"));
        assert!(out.contains("-Syu"));
        // Pinning is via rootflags, never a global set-default.
        assert!(out.contains("NO btrfs set-default"));
        assert!(!out.contains("set-default /"));
    }

    #[test]
    fn render_plan_uses_placeholders_when_versions_unknown() {
        let out = render_plan(&RelayPlan::new(None, None));
        assert!(out.contains("@archetype_<running-version>"));
        assert!(out.contains("@archetype_<new-version>"));
    }
}
