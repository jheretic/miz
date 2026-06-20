//! `miz -I` / `--images` — manage OS image updates via systemd-sysupdated
//! over D-Bus. Dispatch is priority-ordered (read-only verbs first, mutating
//! verbs last — deliberately NOT a mirror of `sync::run`, which checks clean
//! first). Context-less (no alpm handle).
//!
//! All verbs are wired: read-only (-Il/-Ii/-Iy/-Ig/-Ip) and mutating
//! (-Iu/-Ic/--reboot).

mod client;
pub(crate) mod describe;
mod format;
mod job;

use crate::error::{MizError, Result};
use crate::operations::transaction::{confirm, should_prompt};
use client::{
    map_call_error, Login1ProxyBlocking, ManagerProxyBlocking, TargetProxyBlocking, FLAG_OFFLINE,
};
use describe::Describe;

pub use crate::cli::args::images::Args;

/// A `Manager.ListTargets` row: (class, name, object path).
type TargetEntry = (String, String, zbus::zvariant::OwnedObjectPath);

pub fn run(args: Args) -> Result<()> {
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
    if args.features {
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

    eprintln!("miz: -I/--images is not yet implemented");
    Err(MizError::NotImplemented)
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
    // No version pinned -> describe the newest available. TODO(phase3/live):
    // confirm against a real systemd 257+ host that "" selects newest; the
    // man page documents Describe(s,t) but not empty-string semantics. If
    // wrong, resolve an explicit version via CheckNew/List first.
    let version = version.unwrap_or("");
    let json = proxy.describe(version, flags)?;

    // --json is a raw passthrough: dump the bytes the user asked for BEFORE
    // any parse, so a malformed payload still prints rather than erroring.
    if args.json.is_some() {
        println!("{json}");
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

    let current = proxy.get_version().unwrap_or_default();
    let current_label = if current.is_empty() { "(none)" } else { &current };
    let newest = proxy.check_new()?;

    if newest.is_empty() || newest == current {
        // Status note to stderr, matching -Iy's "no newer version" note.
        if !args.quiet {
            eprintln!("{name}: up to date ({current_label})");
        }
        return Ok(());
    }

    if args.quiet {
        println!("{newest}");
    } else {
        println!("{name}: update pending {current_label} -> {newest}");
    }
    Ok(())
}

fn images_features(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_upgrade(args: &Args) -> Result<()> {
    let (conn, targets) = connect()?;
    let (component, version) = split_component(args.targets.first().map(String::as_str));
    let (name, proxy) = resolve_target(&conn, &targets, component)?;
    // "" lets sysupdate pick newest; a pinned version uses the
    // update-to-version polkit action (admin auth).
    let version = version.unwrap_or("");

    // Subscribe to JobRemoved BEFORE Acquire so a fast job can't finish in the
    // gap. Adapt the raw signal iterator into (id, status) for job::wait.
    let manager = ManagerProxyBlocking::new(&conn)?;
    let removed = manager
        .receive_job_removed()
        .map_err(map_call_error)?
        .filter_map(|sig| sig.args().ok().map(|a| (a.id, a.status)));

    // Acquire (download). No-version form is the no-auth `update` action.
    let (acq_ver, acq_id, acq_path) = proxy.acquire(version, 0).map_err(map_call_error)?;
    job::wait(&conn, &acq_path, acq_id, args.noprogressbar, removed)?;

    if should_prompt(args.noconfirm) && !confirm("Proceed with installation? [Y/n] ") {
        return Ok(());
    }

    // Install needs a fresh subscription (the prior iterator was consumed).
    let removed = manager
        .receive_job_removed()
        .map_err(map_call_error)?
        .filter_map(|sig| sig.args().ok().map(|a| (a.id, a.status)));
    let (_iv, ins_id, ins_path) = proxy.install(&acq_ver, 0).map_err(map_call_error)?;
    job::wait(&conn, &ins_path, ins_id, args.noprogressbar, removed)?;

    if !args.quiet {
        println!("{name}: updated to {acq_ver}");
    }
    if args.reboot {
        return do_reboot(&conn);
    }
    Ok(())
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

fn images_reboot(args: &Args) -> Result<()> {
    let (conn, _targets) = connect()?;
    let _ = args;
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
    use super::{list_flags, split_component, FLAG_OFFLINE};

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
}
