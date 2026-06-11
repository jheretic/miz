mod common;

use predicates::prelude::*;
use std::path::PathBuf;

use common::{make_test_root, miz};

fn test_pkg_path() -> Option<PathBuf> {
    std::env::var_os("MIZ_TEST_PKG_PATH").map(PathBuf::from)
}

#[test]
fn dash_u_requires_at_least_one_file() {
    miz()
        .args(["-U"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("required"));
}

#[test]
fn dash_u_rejects_asdeps_with_asexplicit() {
    let root = make_test_root();
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-U",
            "--asdeps",
            "--asexplicit",
            "/nonexistent.pkg.tar.zst",
            "--noconfirm",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--asdeps").and(predicate::str::contains("--asexplicit")));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_u_missing_file_errors_cleanly() {
    let root = make_test_root();
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-U",
            "/definitely/not/a/real/path/foo.pkg.tar.zst",
            "--noconfirm",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not load"));
}

#[test]
#[ignore = "requires libalpm at runtime with MIZ_TEST_PKG_PATH set to a real .pkg.tar.zst"]
fn dash_u_installs_a_local_pkg_file() {
    let Some(pkg_path) = test_pkg_path() else {
        eprintln!("skipping: MIZ_TEST_PKG_PATH unset");
        return;
    };
    let root = make_test_root();

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-U",
            pkg_path.to_str().unwrap(),
            "--noconfirm",
        ])
        .assert()
        .success();
}

#[test]
#[ignore = "requires libalpm at runtime with MIZ_TEST_PKG_PATH set to a real .pkg.tar.zst"]
fn dash_up_prints_without_installing() {
    let Some(pkg_path) = test_pkg_path() else {
        eprintln!("skipping: MIZ_TEST_PKG_PATH unset");
        return;
    };
    let root = make_test_root();

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-Up",
            pkg_path.to_str().unwrap(),
            "--noconfirm",
        ])
        .assert()
        .success();

    let local = root.dbpath().join("local");
    let entries: Vec<_> = std::fs::read_dir(&local)
        .expect("read local")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n != "ALPM_DB_VERSION"
        })
        .collect();
    assert!(
        entries.is_empty(),
        "no packages should be installed under -Up"
    );
}
