use predicates::prelude::*;
use std::fs;

mod common;
use common::{install_fake_pkg, make_test_root, miz, FakePkg};

#[test]
fn dash_s_help_lists_sync_flags() {
    miz()
        .args(["-S", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--search"))
        .stdout(predicate::str::contains("--list"))
        .stdout(predicate::str::contains("--groups"))
        .stdout(predicate::str::contains("--info"))
        .stdout(predicate::str::contains("--print"));
}

#[test]
fn dash_sl_parses() {
    miz()
        .args(["--root", "/nonexistent/miz/test/root", "-Sl"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn dash_ss_requires_pattern_value() {
    miz().args(["-S", "-s"]).assert().failure();
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_ss_runs_against_system_alpm() {
    miz().args(["-Ss", "^bash$"]).assert().success();
}

#[test]
#[ignore = "requires libalpm at runtime"]
fn dash_sg_no_args_lists_groups() {
    miz().args(["-Sg"]).assert().success();
}

#[test]
#[ignore = "requires libalpm at runtime"]
fn dash_si_nonexistent_package_exits_nonzero() {
    miz()
        .args(["-Si", "definitely-not-a-real-package-xyz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("was not found"));
}

#[test]
fn dash_sy_help_lists_refresh_flag() {
    miz()
        .args(["-S", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--refresh"));
}

#[test]
fn dash_sy_with_bad_root_fails_cleanly() {
    miz()
        .args(["--root", "/tmp/miz-test-root-nonexistent-xyz", "-Sy"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn dash_syy_parses() {
    miz()
        .args(["--root", "/tmp/miz-test-root-nonexistent-xyz", "-Syy"])
        .assert()
        .failure();
}

#[test]
fn dash_sys_parses_as_refresh_plus_search() {
    miz()
        .args([
            "--root",
            "/tmp/miz-test-root-nonexistent-xyz",
            "-Sys",
            "^bash$",
        ])
        .assert()
        .failure();
}

#[test]
fn dash_syl_parses_as_refresh_plus_list() {
    miz()
        .args(["--root", "/tmp/miz-test-root-nonexistent-xyz", "-Syl"])
        .assert()
        .failure();
}

#[test]
#[ignore = "requires libalpm + network; run with MIZ_HAS_ALPM=1 MIZ_ALLOW_NETWORK=1"]
fn dash_sy_refresh_against_system_alpm() {
    let _ = miz().args(["-Sy"]).assert();
}

#[test]
#[ignore = "requires libalpm + network; run with MIZ_HAS_ALPM=1 MIZ_ALLOW_NETWORK=1"]
fn dash_syy_force_refresh_against_system_alpm() {
    let _ = miz().args(["-Syy"]).assert();
}

#[test]
fn dash_s_asdeps_and_asexplicit_rejected() {
    miz()
        .args(["-S", "foo", "--asdeps", "--asexplicit"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--asdeps").and(predicate::str::contains("--asexplicit")));
}

#[test]
fn dash_sw_help_lists_downloadonly() {
    miz()
        .args(["-S", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--downloadonly"));
}

#[test]
fn dash_su_help_lists_sysupgrade() {
    miz()
        .args(["-S", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--sysupgrade"));
}

#[test]
fn dash_sc_help_lists_clean() {
    miz()
        .args(["-S", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--clean"));
}

#[test]
#[ignore = "requires libalpm at runtime"]
fn dash_sc_removes_uninstalled_cached_pkgs() {
    let root = make_test_root();
    install_fake_pkg(&root, &FakePkg::minimal("keepme", "1.0-1"));

    let cache = root.cachedir();
    let installed = cache.join("keepme-1.0-1-any.pkg.tar.zst");
    let stale = cache.join("goneby-2.5-3-any.pkg.tar.zst");
    let stale_sig = cache.join("goneby-2.5-3-any.pkg.tar.zst.sig");
    fs::write(&installed, b"x").unwrap();
    fs::write(&stale, b"x").unwrap();
    fs::write(&stale_sig, b"sig").unwrap();

    miz()
        .args([
            "--config",
            root.config_path().to_str().unwrap(),
            "--root",
            root.path.to_str().unwrap(),
            "-Sc",
            "--noconfirm",
        ])
        .assert()
        .success();

    assert!(installed.exists(), "keepme should be preserved");
    assert!(!stale.exists(), "stale pkg should be removed");
    assert!(!stale_sig.exists(), "stale sig should be removed");
}

#[test]
#[ignore = "requires libalpm at runtime"]
fn dash_scc_removes_all_cached_pkgs() {
    let root = make_test_root();
    install_fake_pkg(&root, &FakePkg::minimal("keepme", "1.0-1"));

    let cache = root.cachedir();
    let a = cache.join("keepme-1.0-1-any.pkg.tar.zst");
    let b = cache.join("other-2.0-1-any.pkg.tar.zst");
    fs::write(&a, b"x").unwrap();
    fs::write(&b, b"x").unwrap();

    miz()
        .args([
            "--config",
            root.config_path().to_str().unwrap(),
            "--root",
            root.path.to_str().unwrap(),
            "-Scc",
            "--noconfirm",
        ])
        .assert()
        .success();

    assert!(!a.exists(), "keepme cache should be removed under -Scc");
    assert!(!b.exists(), "other cache should be removed under -Scc");
}

#[test]
#[ignore = "requires libalpm at runtime with populated syncdbs"]
fn dash_sp_print_only() {
    let root = make_test_root();
    miz()
        .args([
            "--config",
            root.config_path().to_str().unwrap(),
            "--root",
            root.path.to_str().unwrap(),
            "-Sp",
            "definitely-not-in-empty-fixture",
        ])
        .assert()
        .failure();
}

#[test]
#[ignore = "requires libalpm at runtime with populated syncdbs"]
fn dash_sw_download_only() {
    let root = make_test_root();
    miz()
        .args([
            "--config",
            root.config_path().to_str().unwrap(),
            "--root",
            root.path.to_str().unwrap(),
            "-Sw",
            "--noconfirm",
            "definitely-not-in-empty-fixture",
        ])
        .assert()
        .failure();
}

#[test]
#[ignore = "requires libalpm at runtime with populated syncdbs"]
fn dash_s_install_against_test_root() {
    let root = make_test_root();
    let _ = miz()
        .args([
            "--config",
            root.config_path().to_str().unwrap(),
            "--root",
            root.path.to_str().unwrap(),
            "-S",
            "--noconfirm",
            "some-pkg-in-syncdb",
        ])
        .assert();
}
