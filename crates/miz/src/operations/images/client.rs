//! zbus `#[proxy]` definitions for systemd-sysupdated (`org.freedesktop.sysupdate1`,
//! system bus) plus a connection helper. Blocking API only; miz is sync.
//!
//! Signatures mirror the verified D-Bus surface in PLAN-images.md §"Verified
//! D-Bus surface". A wrong signature compiles but fails at runtime, so these
//! are kept literal.

use crate::error::{MizError, Result};
use zbus::proxy;
use zbus::zvariant::OwnedObjectPath;

/// Manager interface at `/org/freedesktop/sysupdate1`.
#[proxy(
    interface = "org.freedesktop.sysupdate1.Manager",
    default_service = "org.freedesktop.sysupdate1",
    default_path = "/org/freedesktop/sysupdate1"
)]
pub trait Manager {
    /// `ListTargets() -> a(sso)` — (class, name, object path).
    fn list_targets(&self) -> zbus::Result<Vec<(String, String, OwnedObjectPath)>>;

    /// `ListJobs() -> a(tsuo)` — (id, type, progress, object path).
    fn list_jobs(&self) -> zbus::Result<Vec<(u64, String, u32, OwnedObjectPath)>>;

    /// `ListAppStream() -> as`.
    fn list_app_stream(&self) -> zbus::Result<Vec<String>>;

    /// `JobRemoved(t id, o path, i status)` — status 0 = success, >0 exit code,
    /// <0 = -errno.
    #[zbus(signal)]
    fn job_removed(&self, id: u64, path: OwnedObjectPath, status: i32) -> zbus::Result<()>;
}

/// Target interface (object path discovered via `Manager.ListTargets`).
#[proxy(
    interface = "org.freedesktop.sysupdate1.Target",
    default_service = "org.freedesktop.sysupdate1"
)]
pub trait Target {
    /// `List(t flags) -> as`. flag `SD_SYSUPDATE_OFFLINE = 1<<0`.
    fn list(&self, flags: u64) -> zbus::Result<Vec<String>>;

    /// `Describe(s version, t flags) -> s` (JSON).
    fn describe(&self, version: &str, flags: u64) -> zbus::Result<String>;

    /// `CheckNew() -> s` — newest available version, "" if none.
    fn check_new(&self) -> zbus::Result<String>;

    // NOTE (phase 3): Acquire/Install return a Job object path. The exact
    // input signature is not pinned in PLAN §verified-surface beyond "-> o";
    // these are best-guess (version + flags) and MUST be confirmed against the
    // live service before wiring -Iu. Unused in phase 1.
    /// `Acquire(s version, t flags) -> o` (job path).
    fn acquire(&self, version: &str, flags: u64) -> zbus::Result<OwnedObjectPath>;

    /// `Install(s version, t flags) -> o` (job path).
    fn install(&self, version: &str, flags: u64) -> zbus::Result<OwnedObjectPath>;

    /// `Vacuum() -> u` — count of removed versions.
    fn vacuum(&self) -> zbus::Result<u32>;

    /// `GetVersion() -> s` — currently installed version.
    fn get_version(&self) -> zbus::Result<String>;

    /// `ListFeatures(t flags) -> as`.
    fn list_features(&self, flags: u64) -> zbus::Result<Vec<String>>;

    /// `DescribeFeature(s feature, t flags) -> s` (JSON).
    fn describe_feature(&self, feature: &str, flags: u64) -> zbus::Result<String>;

    /// `SetFeatureEnabled(s feature, i enabled, t flags)`.
    fn set_feature_enabled(&self, feature: &str, enabled: i32, flags: u64) -> zbus::Result<()>;

    #[zbus(property)]
    fn class(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn path(&self) -> zbus::Result<String>;
}

/// Job interface (object path returned by Acquire/Install).
#[proxy(
    interface = "org.freedesktop.sysupdate1.Job",
    default_service = "org.freedesktop.sysupdate1"
)]
pub trait Job {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<u64>;

    #[zbus(property, name = "Type")]
    fn job_type(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn offline(&self) -> zbus::Result<bool>;

    /// 0-100; only meaningful for acquire/install jobs.
    #[zbus(property)]
    fn progress(&self) -> zbus::Result<u32>;
}

/// `SD_SYSUPDATE_OFFLINE` flag for `List`/`Describe` (installed-only, no network).
pub const FLAG_OFFLINE: u64 = 1 << 0;

/// Open the system bus, mapping any failure to a clean `Sysupdate` error rather
/// than a raw zbus panic/trace.
pub fn system_connection() -> Result<zbus::blocking::Connection> {
    zbus::blocking::Connection::system().map_err(|_| {
        MizError::Sysupdate(
            "systemd-sysupdated is not available (requires systemd 257+)".to_string(),
        )
    })
}
