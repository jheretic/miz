//! `miz -I` / `--images` — manage OS image updates via systemd-sysupdated
//! over D-Bus. Dispatch is priority-ordered (read-only verbs first, mutating
//! verbs last — deliberately NOT a mirror of `sync::run`, which checks clean
//! first). Context-less (no alpm handle).
//!
//! All verbs are wired: read-only (-Il/-Ii/-Iy/-Ig/-Ip), mutating
//! (-Iu/-Ic/--reboot), and feature/appstream polish (-If/--enable/--disable,
//! --appstream).

mod client;
pub(crate) mod describe;
mod format;
mod job;
mod relay;

use crate::error::{MizError, Result};
use crate::operations::transaction::{confirm, should_prompt};
use client::{
    map_call_error, Login1ProxyBlocking, ManagerProxyBlocking, TargetProxyBlocking, FLAG_OFFLINE,
};
use describe::Describe;

pub use crate::cli::args::images::Args;

/// A `Manager.ListTargets` row: (class, name, object path).
type TargetEntry = (String, String, zbus::zvariant::OwnedObjectPath);

pub fn run(args: Args, config_path: Option<&std::path::Path>) -> Result<()> {
    // Split-db image update: re-lay layered packages onto the new A/B image +
    // snapshot. Context-bearing (relay builds its own /run-rooted handle), so
    // it stays out of main.rs's needs_context path. Handled first so it never
    // falls through to the D-Bus verbs.
    if args.reinstall_layered {
        return relay::run(args, config_path);
    }
    if args.list {
        return images_list(&args);
    }
    if args.info > 0 {
        return images_info(&args);
    }
    if args.check_new {
        return images_check_new(&args);
    }
    if args.components {
        return images_components(&args);
    }
    if args.pending {
        return images_pending(&args);
    }
    if args.appstream {
        return images_appstream(&args);
    }
    if args.features || args.enable.is_some() || args.disable.is_some() {
        return images_features(&args);
    }
    if args.upgrade > 0 {
        return images_upgrade(&args);
    }
    if args.clean > 0 {
        return images_vacuum(&args);
    }
    if args.reboot {
        return images_reboot(&args);
    }

    // No sub-verb given. -I is an operation family, not a standalone action
    // (like pacman's bare -S), so this is a usage error rather than a default.
    Err(MizError::Other(
        "no image operation specified (use -Il to list, -Iu to update; -h for help)".to_string(),
    ))
}

/// Split a positional target into `(component, Option<version>)`, mirroring
/// `sync::split_repo_target`'s `repo/pkg` idiom. Defaults to component `"host"`.
fn split_component(target: Option<&str>) -> (&str, Option<&str>) {
    match target {
        Some(t) => match t.split_once('/') {
            Some((comp, ver)) => (comp, Some(ver)),
            None => (t, None),
        },
        None => ("host", None),
    }
}

/// Open the system bus and probe service availability. The probe (ListTargets)
/// is where the "requires systemd 257+" message belongs: `system_connection`
/// only reports raw bus-connect failures, but a masked/absent sysupdated
/// surfaces here at first method call. Returns the connection plus the probed
/// target list so callers reuse it for resolution.
fn connect() -> Result<(zbus::blocking::Connection, Vec<TargetEntry>)> {
    let conn = client::system_connection()?;
    let manager = ManagerProxyBlocking::new(&conn)?;
    let targets = manager.list_targets().map_err(|e| {
        MizError::Sysupdate(format!(
            "systemd-sysupdated is not available (requires systemd 257+): {e}"
        ))
    })?;
    Ok((conn, targets))
}

/// Resolve a component name to its Target proxy via the probed target list.
/// Unknown component -> `MizError::Sysupdate("no such component: ...")`.
fn resolve_target<'a>(
    conn: &zbus::blocking::Connection,
    targets: &[TargetEntry],
    component: &str,
) -> Result<(String, TargetProxyBlocking<'a>)> {
    let entry = targets
        .iter()
        .find(|(_, name, _)| name == component)
        .ok_or_else(|| MizError::Sysupdate(format!("no such component: {component}")))?;
    let proxy = TargetProxyBlocking::builder(conn)
        .path(entry.2.clone())?
        .build()?;
    Ok((entry.1.clone(), proxy))
}

fn list_flags(offline: bool) -> u64 {
    if offline {
        FLAG_OFFLINE
    } else {
        0
    }
}

/// `--json=MODE` outcome for the Describe/DescribeFeature passthrough.
/// Mirrors systemd-sysupdate's `--json=` (short/pretty/off).
enum JsonMode {
    Off,
    Short,
    Pretty,
}

/// Parse `--json=MODE`. Absent or `off` -> Off (pacman rendering). `short`
/// (default when given without an explicit accepted value isn't possible,
/// clap requires a value) -> raw bytes. `pretty` -> re-indented.
fn json_mode(arg: Option<&str>) -> Result<JsonMode> {
    match arg {
        None | Some("off") => Ok(JsonMode::Off),
        Some("short") => Ok(JsonMode::Short),
        Some("pretty") => Ok(JsonMode::Pretty),
        Some(other) => Err(MizError::Sysupdate(format!(
            "invalid --json mode: {other} (expected short, pretty, or off)"
        ))),
    }
}

/// Print `json` per `mode` (caller already excluded Off). Short prints the raw
/// bytes as received; Pretty re-serializes via serde_json. A malformed payload
/// under Pretty falls back to raw so the user still sees something.
fn print_json(json: &str, mode: JsonMode) {
    match mode {
        JsonMode::Off => unreachable!("caller handles Off before rendering"),
        JsonMode::Short => println!("{json}"),
        JsonMode::Pretty => match serde_json::from_str::<serde_json::Value>(json) {
            Ok(v) => println!(
                "{}",
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| json.to_string())
            ),
            Err(_) => println!("{json}"),
        },
    }
}

fn images_list(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    let (component, _version) = split_component(args.targets.first().map(String::as_str));
    let (name, proxy) = resolve_target(&conn, &targets, component)?;

    let flags = list_flags(args.offline);
    let versions = proxy.list(flags)?;
    let installed = proxy.get_version().unwrap_or_default();

    for version in &versions {
        // Per-version Describe marks [installed]/[newest]. Describe failures
        // (e.g. transient) degrade gracefully to the GetVersion comparison.
        let (is_installed, is_newest) = match proxy.describe(version, flags) {
            Ok(json) => match Describe::parse(&json) {
                Ok(d) => (
                    d.installed.unwrap_or(version == &installed),
                    d.newest.unwrap_or(false),
                ),
                Err(_) => (version == &installed, false),
            },
            Err(_) => (version == &installed, false),
        };
        println!(
            "{}",
            format::list_line(&name, version, is_installed, is_newest, args.quiet)
        );
    }
    Ok(())
}

fn images_info(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    let (component, version) = split_component(args.targets.first().map(String::as_str));
    let (name, proxy) = resolve_target(&conn, &targets, component)?;

    let flags = list_flags(args.offline);
    // sysupdated's Describe requires a CONCRETE, valid version: it calls
    // version_is_valid() and rejects "" with org.freedesktop.DBus.Error.
    // InvalidArgs "Invalid version" (sysupdated.c target_method_describe). So an
    // empty version is never "newest" -- resolve one first. Explicit arg wins;
    // else the installed version (GetVersion, like `-Qi` describes what's
    // installed, and works offline); else fall back to the newest in List.
    let resolved;
    let version = match version {
        Some(v) => v,
        None => {
            // Installed version first (works offline, matches `-Qi`); else the
            // newest available via CheckNew (purpose-built "newest" -- avoids
            // assuming List's ordering).
            let installed = proxy.get_version().unwrap_or_default();
            resolved = if !installed.is_empty() {
                installed
            } else {
                proxy.check_new().unwrap_or_default()
            };
            if resolved.is_empty() {
                return Err(MizError::Sysupdate(format!(
                    "no installed or available version to describe for '{name}'"
                )));
            }
            resolved.as_str()
        }
    };
    let json = proxy.describe(version, flags)?;

    // --json=short/pretty is a passthrough: emit the payload BEFORE any parse,
    // so a malformed body still prints rather than erroring. =off falls through.
    let mode = json_mode(args.json.as_deref())?;
    if !matches!(mode, JsonMode::Off) {
        print_json(&json, mode);
        return Ok(());
    }

    let d = Describe::parse(&json)
        .map_err(|e| MizError::Sysupdate(format!("could not parse Describe JSON: {e}")))?;
    let verbose = args.info >= 2 && !args.quiet;
    print!("{}", format::info_block(&name, &d, verbose));
    println!();
    Ok(())
}

fn images_check_new(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    let (component, _version) = split_component(args.targets.first().map(String::as_str));
    let (name, proxy) = resolve_target(&conn, &targets, component)?;

    let newest = proxy.check_new()?;
    if newest.is_empty() {
        if !args.quiet {
            eprintln!("{name}: no newer version available");
        }
        return Ok(());
    }
    if args.quiet {
        println!("{newest}");
    } else {
        println!("{name}: {newest} available");
    }
    Ok(())
}

fn images_components(args: &Args) -> Result<()> {
    let (_conn, targets) = connect()?;
    for (class, name, _path) in &targets {
        println!("{}", format::component_line(class, name, args.quiet));
    }
    Ok(())
}

fn images_pending(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    let (component, _version) = split_component(args.targets.first().map(String::as_str));
    let (name, proxy) = resolve_target(&conn, &targets, component)?;

    // systemd `pending`: is the newest INSTALLED version newer than the BOOTED
    // one (IMAGE_VERSION in os-release), i.e. is a reboot due? This is distinct
    // from -Iy/check-new ("is a download available"). GetVersion is the newest
    // installed; booted comes from os-release under --root.
    let installed = proxy.get_version().unwrap_or_default();
    let booted = crate::operations::osrelease::booted_image_version();
    let installed_label = if installed.is_empty() {
        "(none)"
    } else {
        &installed
    };
    let booted_label = match &booted {
        Some(b) if !b.is_empty() => b.as_str(),
        _ => "(unknown)",
    };

    let reboot_due = match &booted {
        Some(b) => !installed.is_empty() && &installed != b,
        None => false,
    };

    if reboot_due {
        if args.quiet {
            println!("{installed}");
        } else {
            println!("{name}: reboot pending: booted {booted_label}, installed {installed_label}");
        }
    } else if !args.quiet {
        // No reboot due -> status note to stderr, matching -Iy's note stream.
        eprintln!("{name}: no reboot pending (booted {booted_label})");
    }
    Ok(())
}

fn images_features(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    let (component, _version) = split_component(args.targets.first().map(String::as_str));
    let (name, proxy) = resolve_target(&conn, &targets, component)?;

    // Enable/disable mutate config (manage-features polkit action, admin auth).
    // enabled: >0 enable, 0 disable (man page). flags must be 0.
    if let Some(feature) = &args.enable {
        proxy
            .set_feature_enabled(feature, 1, 0)
            .map_err(map_call_error)?;
        if !args.quiet {
            println!("{name}: enabled feature {feature} (run -Iu to apply)");
        }
        return Ok(());
    }
    if let Some(feature) = &args.disable {
        proxy
            .set_feature_enabled(feature, 0, 0)
            .map_err(map_call_error)?;
        if !args.quiet {
            println!("{name}: disabled feature {feature} (run -Iu to apply)");
        }
        return Ok(());
    }

    // component/<feature> -> describe that feature; bare component -> list.
    match split_component(args.targets.first().map(String::as_str)).1 {
        Some(feature) => {
            let json = proxy.describe_feature(feature, 0)?;
            // --json passthrough before any parse (mirrors -Ii).
            let mode = json_mode(args.json.as_deref())?;
            if !matches!(mode, JsonMode::Off) {
                print_json(&json, mode);
                return Ok(());
            }
            let f = describe::Feature::parse(&json).map_err(|e| {
                MizError::Sysupdate(format!("could not parse DescribeFeature JSON: {e}"))
            })?;
            print!("{}", format::feature_block(feature, &f));
            println!();
        }
        None => {
            for feature in proxy.list_features(0)? {
                println!("{feature}");
            }
        }
    }
    Ok(())
}

fn images_appstream(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    // With a target, query that component's catalogs; bare -> all known URLs.
    let urls = match args.targets.first() {
        Some(_) => {
            let (component, _version) = split_component(args.targets.first().map(String::as_str));
            let (_name, proxy) = resolve_target(&conn, &targets, component)?;
            proxy.get_app_stream()?
        }
        None => {
            let manager = ManagerProxyBlocking::new(&conn)?;
            manager.list_app_stream()?
        }
    };
    for url in urls {
        println!("{url}");
    }
    Ok(())
}

fn images_upgrade(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    let (component, version) = split_component(args.targets.first().map(String::as_str));
    let (name, proxy) = resolve_target(&conn, &targets, component)?;

    // Resolve what to acquire, and how.
    //
    // The version argument selects the polkit action (org.freedesktop.
    // sysupdate1(5)): an EMPTY version uses `update` (permitted without admin
    // auth); a SPECIFIC version uses `update-to-version` (admin auth). So the
    // routine newest-upgrade path MUST pass "" to stay auth-free.
    //
    // A newly enabled OPTIONAL FEATURE, however, only installs when we acquire a
    // concrete version: feature transfers update "in lock-step with the rest of
    // their target" (sysupdate.features(5)) and are not independently
    // installable, so a bare Acquire("") when the host is already newest is a
    // no-op and the feature never downloads. The documented remedy (GetVersion)
    // is to "extend the newest existing installation in-place" by acquiring the
    // installed version, which completes that now-`incomplete` version and pulls
    // the feature's instance. That necessarily uses update-to-version (admin
    // auth) -- acceptable, since enabling the feature was itself admin-gated.
    let plan = match version {
        Some(v) => UpgradePlan::ToVersion(v.to_string()),
        None => resolve_upgrade_plan(&proxy)?,
    };
    if matches!(plan, UpgradePlan::UpToDate) {
        if !args.quiet {
            eprintln!("{name}: already up to date");
        }
        return Ok(());
    }
    let (acquire_arg, host_changed) = acquire_args(&plan);

    // Subscribe to JobRemoved BEFORE Acquire so a fast job can't finish in the
    // gap. Adapt the raw signal iterator into (id, status) for job::wait.
    let manager = ManagerProxyBlocking::new(&conn)?;
    let removed = manager
        .receive_job_removed()
        .map_err(map_call_error)?
        .filter_map(|sig| sig.args().ok().map(|a| (a.id, a.status)));

    let (acq_ver, acq_id, acq_path) = proxy.acquire(&acquire_arg, 0).map_err(map_call_error)?;
    job::wait(&conn, &acq_path, acq_id, args.noprogressbar, removed, "acquiring")?;

    if should_prompt(args.noconfirm) && !confirm("Proceed with installation? [Y/n] ") {
        return Ok(());
    }

    // Install needs a fresh subscription (the prior iterator was consumed).
    let removed = manager
        .receive_job_removed()
        .map_err(map_call_error)?
        .filter_map(|sig| sig.args().ok().map(|a| (a.id, a.status)));
    let (_iv, ins_id, ins_path) = proxy.install(&acq_ver, 0).map_err(map_call_error)?;
    job::wait(&conn, &ins_path, ins_id, args.noprogressbar, removed, "installing")?;

    if !args.quiet {
        if host_changed {
            println!("{name}: updated to {acq_ver}");
        } else {
            // In-place completion (e.g. a newly enabled feature): the version
            // didn't advance, so "updated to" would be misleading.
            println!("{name}: {acq_ver} completed (in place)");
        }
    }

    // Default named-subvolume relay: sysupdate has written the new /usr + UKI to
    // the inactive slot; now snapshot the root per version and upgrade layered
    // packages against the new image so a /usr rollback keeps them consistent
    // (see relay module docs). Only for the host component (the /usr image), and
    // only when the host version ACTUALLY advanced -- an in-place completion of
    // the installed version (feature download) touched only /var/lib/extensions,
    // has no root-snapshot coupling, and would make the relay try to re-create
    // the already-existing @archetype_<version> subvol (fail-closed).
    if component == "host" && host_changed {
        relay::relay_after_upgrade(&acq_ver, args.dry_run, args.quiet)?;
    }

    if args.reboot {
        return do_reboot(&conn);
    }
    Ok(())
}

/// Outcome of resolving a bare `-Iu` (no explicit version).
enum UpgradePlan {
    /// A newer version is available; acquire newest (empty arg, no-auth).
    Newest,
    /// No newer version, but the installed one is incomplete (e.g. a newly
    /// enabled feature); re-acquire this exact version to complete it in place.
    Complete(String),
    /// A user-supplied explicit version (update-to-version).
    ToVersion(String),
    /// Nothing to do.
    UpToDate,
}

/// Map an [`UpgradePlan`] to the `Acquire`/`Install` version argument and
/// whether the host version advances. Pure, so the polkit-critical invariant is
/// testable: `Newest` MUST pass an empty arg (org.freedesktop.sysupdate1.update,
/// no admin auth); a concrete version triggers update-to-version (admin auth).
/// `host_changed` gates the root-snapshot relay -- true only when the host `/usr`
/// actually advances, false for an in-place completion (feature download).
/// `UpToDate` is handled by the caller before this is reached.
fn acquire_args(plan: &UpgradePlan) -> (String, bool) {
    match plan {
        UpgradePlan::Newest => (String::new(), true),
        UpgradePlan::Complete(v) => (v.clone(), false),
        UpgradePlan::ToVersion(v) => (v.clone(), true),
        UpgradePlan::UpToDate => (String::new(), false),
    }
}

/// Decide what a bare `-Iu` should do:
///
/// * `CheckNew` non-empty -> [`UpgradePlan::Newest`] (relay runs for host).
/// * else installed version is `incomplete` -> [`UpgradePlan::Complete`]
///   (a feature was enabled but its instance isn't downloaded; complete it in
///   place, no relay).
/// * else [`UpgradePlan::UpToDate`].
fn resolve_upgrade_plan(proxy: &TargetProxyBlocking<'_>) -> Result<UpgradePlan> {
    let newest = proxy.check_new().unwrap_or_default();
    if !newest.is_empty() {
        return Ok(UpgradePlan::Newest);
    }
    // No newer version. Complete the installed one only if it is incomplete, so
    // a plain no-op `-Iu` stays a no-op (never re-installs needlessly).
    let installed = proxy.get_version().unwrap_or_default();
    if installed.is_empty() {
        return Ok(UpgradePlan::UpToDate);
    }
    let incomplete = proxy
        .describe(&installed, 0)
        .ok()
        .and_then(|json| Describe::parse(&json).ok())
        .and_then(|d| d.incomplete)
        .unwrap_or(false);
    if incomplete {
        Ok(UpgradePlan::Complete(installed))
    } else {
        Ok(UpgradePlan::UpToDate)
    }
}

fn images_vacuum(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    let (component, _version) = split_component(args.targets.first().map(String::as_str));
    let (name, proxy) = resolve_target(&conn, &targets, component)?;

    // Vacuum is admin-auth; map a denial cleanly.
    let (instances, disabled) = proxy.vacuum().map_err(map_call_error)?;
    if !args.quiet {
        println!("{name}: removed {instances} version(s), disabled {disabled} transfer(s)");
    }
    Ok(())
}

fn images_reboot(_args: &Args) -> Result<()> {
    // Reboot only needs logind, NOT sysupdated — don't run connect()'s
    // ListTargets probe, or `--reboot` would wrongly require systemd 257+
    // on a host that has logind but no sysupdated.
    let conn = client::system_connection()?;
    do_reboot(&conn)
}

/// Reboot via logind (`org.freedesktop.login1`), polkit-gated and D-Bus-native
/// rather than shelling `systemctl`.
fn do_reboot(conn: &zbus::blocking::Connection) -> Result<()> {
    let login1 = Login1ProxyBlocking::new(conn).map_err(map_call_error)?;
    login1.reboot(false).map_err(map_call_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{acquire_args, list_flags, split_component, UpgradePlan, FLAG_OFFLINE};

    #[test]
    fn acquire_args_newest_uses_empty_arg_for_no_auth_update() {
        // Empty version arg -> org.freedesktop.sysupdate1.update (no admin auth).
        // A concrete version would trigger update-to-version (admin auth), so the
        // routine newest upgrade MUST pass "".
        let (arg, host_changed) = acquire_args(&UpgradePlan::Newest);
        assert_eq!(arg, "");
        assert!(host_changed);
    }

    #[test]
    fn acquire_args_complete_uses_installed_version_without_host_change() {
        // In-place completion (newly enabled feature): explicit version, and the
        // host did NOT advance -> relay must not run.
        let (arg, host_changed) = acquire_args(&UpgradePlan::Complete("2026.07.10-1".into()));
        assert_eq!(arg, "2026.07.10-1");
        assert!(!host_changed);
    }

    #[test]
    fn acquire_args_to_version_passes_version_and_advances() {
        let (arg, host_changed) = acquire_args(&UpgradePlan::ToVersion("2026.07.09-1".into()));
        assert_eq!(arg, "2026.07.09-1");
        assert!(host_changed);
    }

    #[test]
    fn list_flags_offline_sets_bit() {
        assert_eq!(list_flags(true), FLAG_OFFLINE);
        assert_eq!(list_flags(false), 0);
    }

    #[test]
    fn split_defaults_to_host() {
        assert_eq!(split_component(None), ("host", None));
    }

    #[test]
    fn split_bare_component() {
        assert_eq!(split_component(Some("foo")), ("foo", None));
    }

    #[test]
    fn split_component_version() {
        assert_eq!(split_component(Some("host/2.3")), ("host", Some("2.3")));
    }

    #[test]
    fn json_mode_parsing() {
        use super::{json_mode, JsonMode};
        assert!(matches!(json_mode(None).unwrap(), JsonMode::Off));
        assert!(matches!(json_mode(Some("off")).unwrap(), JsonMode::Off));
        assert!(matches!(json_mode(Some("short")).unwrap(), JsonMode::Short));
        assert!(matches!(
            json_mode(Some("pretty")).unwrap(),
            JsonMode::Pretty
        ));
        assert!(json_mode(Some("bogus")).is_err());
    }
}
