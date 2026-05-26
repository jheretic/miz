mod common;

use predicates::prelude::*;

use common::{install_fake_pkg, make_test_root, miz, FakePkg};

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qk_reports_zero_missing_when_all_files_present() {
    let root = make_test_root();
    let pkg = FakePkg {
        files: &[
            ("usr/bin/foo", b"#!/bin/sh\necho foo\n" as &[u8]),
            ("usr/share/foo/data.txt", b"hello\n" as &[u8]),
        ],
        ..FakePkg::minimal("foo", "1.0-1")
    };
    install_fake_pkg(&root, &pkg);

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-Qk",
            "foo",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "foo: 2 total files, 0 missing files",
        ));
}

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qk_counts_missing_files_and_exits_nonzero() {
    let root = make_test_root();
    let pkg = FakePkg {
        files: &[
            ("usr/bin/bar", b"present\n" as &[u8]),
            ("usr/bin/baz", b"will-be-deleted\n" as &[u8]),
            ("usr/share/bar/data.txt", b"also-deleted\n" as &[u8]),
        ],
        ..FakePkg::minimal("bar", "1.0-1")
    };
    install_fake_pkg(&root, &pkg);

    std::fs::remove_file(root.path.join("usr/bin/baz")).unwrap();
    std::fs::remove_file(root.path.join("usr/share/bar/data.txt")).unwrap();

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-Qk",
            "bar",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "bar: 3 total files, 2 missing files",
        ))
        .stderr(predicate::str::contains("bar: /usr/bin/baz (Missing file)"))
        .stderr(predicate::str::contains(
            "bar: /usr/share/bar/data.txt (Missing file)",
        ));
}

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qkk_appends_altered_count_on_summary_line() {
    let root = make_test_root();
    let pkg = FakePkg {
        files: &[("usr/bin/qux", b"original\n" as &[u8])],
        ..FakePkg::minimal("qux", "1.0-1")
    };
    install_fake_pkg(&root, &pkg);

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-Qkk",
            "qux",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "qux: 1 total files, 0 missing files, 0 altered files",
        ));
}

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qk_unknown_package_errors() {
    let root = make_test_root();

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-Qk",
            "nonexistent",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "package 'nonexistent' was not found",
        ));
}
