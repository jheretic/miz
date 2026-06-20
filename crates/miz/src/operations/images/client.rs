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

    /// `Acquire(in s new_version, in t flags) -> (s new_version, t job_id, o job_path)`.
    /// Per the systemd 261 man page the reply is a 3-tuple, not a bare path.
    /// Wired in phase 3; verify against a live service then.
    fn acquire(&self, version: &str, flags: u64) -> zbus::Result<(String, u64, OwnedObjectPath)>;

    /// `Install(in s new_version, in t flags) -> (s new_version, t job_id, o job_path)`.
    fn install(&self, version: &str, flags: u64) -> zbus::Result<(String, u64, OwnedObjectPath)>;

    /// `Vacuum() -> (u instances, u disabled_transfers)`.
    fn vacuum(&self) -> zbus::Result<(u32, u32)>;

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

/// Open the system bus. This only fails if the bus itself is unreachable; the
/// "sysupdated not running / too-old systemd" case surfaces at first method
/// call instead. Phase 2 probes service availability (e.g. ListTargets) and
/// maps THAT to the "requires systemd 257+" message.
pub fn system_connection() -> Result<zbus::blocking::Connection> {
    zbus::blocking::Connection::system()
        .map_err(|e| MizError::Sysupdate(format!("cannot connect to the system D-Bus: {e}")))
}
