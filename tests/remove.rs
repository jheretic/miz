mod common;

use predicates::prelude::*;

use common::{install_fake_pkg, make_test_root, miz, FakePkg};

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_r_removes_an_installed_package() {
    let root = make_test_root();
    install_fake_pkg(&root, &FakePkg::minimal("foo", "1.0-1"));
    assert!(root.local_pkg_dir("foo", "1.0-1").exists());

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-R",
            "foo",
            "--noconfirm",
        ])
        .assert()
        .success();

    assert!(!root.local_pkg_dir("foo", "1.0-1").exists());
}

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_rp_prints_without_removing() {
    let root = make_test_root();
    install_fake_pkg(&root, &FakePkg::minimal("baz", "3.0-1"));
    assert!(root.local_pkg_dir("baz", "3.0-1").exists());

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-Rp",
            "baz",
            "--noconfirm",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("baz"));

    assert!(root.local_pkg_dir("baz", "3.0-1").exists());
}

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_r_unknown_package_errors() {
    let root = make_test_root();

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-R",
            "nonexistent",
            "--noconfirm",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("target not found"));
}
