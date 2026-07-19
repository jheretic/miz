mod common;

use std::path::Path;

use predicates::prelude::*;

use common::{install_fake_pkg, make_test_root, miz, set_image_db, FakePkg};

/// The committed image-db fixture tree (10-archetype/foo-1.2.3-1,
/// 50-extra/baz-2.0-3). Shared by the image-db query tests below.
fn image_db_fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image_db")
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_q_unions_image_db_packages() {
    let root = make_test_root();
    // A layered /var package plus the baked-in /usr image db.
    install_fake_pkg(&root, &FakePkg::minimal("layered", "1.0-1"));
    let conf = set_image_db(&root, &image_db_fixture());

    // Plain -Q lists BOTH the layered package and the image-db packages.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Q",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("layered 1.0-1")
                .and(predicate::str::contains("foo 1.2.3-1"))
                .and(predicate::str::contains("baz 2.0-3")),
        );
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_q_named_finds_image_only_package() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Q",
            "foo",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("foo 1.2.3-1"));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_q_localdb_shadows_image_db() {
    let root = make_test_root();
    // Install a layered 'foo' that shadows the image db's foo-1.2.3-1.
    install_fake_pkg(&root, &FakePkg::minimal("foo", "9.9-9"));
    let conf = set_image_db(&root, &image_db_fixture());
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Q",
            "foo",
        ])
        .assert()
        .success()
        // localdb version wins; the image version is not shown.
        .stdout(
            predicate::str::contains("foo 9.9-9").and(predicate::str::contains("1.2.3-1").not()),
        );
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qi_on_image_only_package_shows_info() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // -Qi renders the desc fields the image db carries: an image-only package
    // resolves to a full info block, on par with a localdb package.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Qi",
            "foo",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Name")
                .and(predicate::str::contains("foo"))
                .and(predicate::str::contains("A foo package"))
                .and(predicate::str::contains("glibc")),
        );
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_ql_lists_image_package_files() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // -Ql lists the files the image `files` db entry records.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Ql",
            "foo",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("foo usr/bin/foo"));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qo_finds_image_package_owning_file() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // -Qo resolves an owned path to its image package.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Qo",
            "/usr/bin/foo",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("owned by foo 1.2.3-1"));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qs_searches_image_db() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // -Qs now prints the description line for an image hit, matching localdb.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Qs",
            "baz",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("image/baz 2.0-3"));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qs_matches_image_description() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // Searching a term only in the DESC field matches the image package and the
    // description is printed on the following line.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Qs",
            "foo package",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("image/foo 1.2.3-1")
                .and(predicate::str::contains("A foo package")),
        );
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_q_unknown_name_still_errors_with_image_db() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Q",
            "does-not-exist",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("was not found"));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qd_lists_image_dependency_packages() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // baz is %REASON% 1 (a dependency); foo is explicit. -Qd keeps baz, drops foo.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Qd",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("baz 2.0-3").and(predicate::str::contains("foo").not()));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qe_lists_image_explicit_packages() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // -Qe keeps the explicitly-installed foo, drops the dependency baz.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Qe",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("foo 1.2.3-1").and(predicate::str::contains("baz").not()));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qt_excludes_required_image_package() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // foo depends on glibc; nothing installed depends on foo, and nothing
    // requires baz's provided libbaz.so, so both foo and baz are unrequired.
    // (glibc is not in the image db here, so it does not appear.)
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Qt",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("foo 1.2.3-1").and(predicate::str::contains("baz 2.0-3")));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qk_checks_image_package_files() {
    let root = make_test_root();
    let conf = set_image_db(&root, &image_db_fixture());
    // foo owns usr/bin/foo, usr/share/doc/foo/, usr/share/doc/foo/README under
    // the root -- none created here, so all are reported missing and -Qk fails.
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            conf.to_str().unwrap(),
            "-Qk",
            "foo",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "foo: 3 total files, 3 missing files",
        ));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
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
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
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
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
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
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
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
