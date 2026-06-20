// Integration tests for `miz -I` / `--images` (systemd-sysupdated over D-Bus).
//
// The read-only verbs (-Il/-Ii/-Iy/-Ig/-Ip) need no polkit auth, so they are
// safe to run unprivileged on a host with systemd 257+ and a configured
// sysupdate.d. They are gated behind MIZ_HAS_SYSUPDATE=1 because most CI/dev
// hosts lack the service. Mutating verbs (-Iu/-Ic) are NEVER exercised here.
//
// Run with: MIZ_HAS_SYSUPDATE=1 cargo test -p miz --test images -- --ignored

use predicates::prelude::*;

mod common;
use common::miz;

// --- Ungated: clap parsing only, no bus contact required for --help. ---

#[test]
fn dash_i_help_lists_image_flags() {
    miz()
        .args(["-I", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--list"))
        .stdout(predicate::str::contains("--check-new"))
        .stdout(predicate::str::contains("--components"))
        .stdout(predicate::str::contains("--pending"))
        .stdout(predicate::str::contains("--offline"));
}

// --- Gated: require a live systemd-sysupdated. Read-only verbs only. ---

#[test]
#[ignore = "requires systemd-sysupdated; run with MIZ_HAS_SYSUPDATE=1"]
fn dash_ig_lists_components() {
    // `class name` per row; host should be present on a configured system.
    miz().args(["-Ig"]).assert().success();
}

#[test]
#[ignore = "requires systemd-sysupdated; run with MIZ_HAS_SYSUPDATE=1"]
fn dash_il_lists_host_versions() {
    miz().args(["-Il", "host"]).assert().success();
}

#[test]
#[ignore = "requires systemd-sysupdated; run with MIZ_HAS_SYSUPDATE=1"]
fn dash_il_offline_is_installed_only() {
    miz().args(["-Il", "--offline", "host"]).assert().success();
}

#[test]
#[ignore = "requires systemd-sysupdated; run with MIZ_HAS_SYSUPDATE=1"]
fn dash_ii_describes_host() {
    miz()
        .args(["-Ii", "host"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Component"));
}

#[test]
#[ignore = "requires systemd-sysupdated; run with MIZ_HAS_SYSUPDATE=1"]
fn dash_iy_checks_new() {
    miz().args(["-Iy", "host"]).assert().success();
}

#[test]
#[ignore = "requires systemd-sysupdated; run with MIZ_HAS_SYSUPDATE=1"]
fn dash_ip_reports_pending() {
    miz().args(["-Ip", "host"]).assert().success();
}

#[test]
#[ignore = "requires systemd-sysupdated; run with MIZ_HAS_SYSUPDATE=1"]
fn unknown_component_is_clean_error() {
    miz()
        .args(["-Il", "definitely-not-a-real-component"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no such component"));
}
