mod common;

use predicates::prelude::*;

use common::{install_fake_pkg, make_test_root, miz, FakePkg, TestRoot};

fn read_reason(root: &TestRoot, name: &str, version: &str) -> Option<u8> {
    let desc = std::fs::read_to_string(root.local_pkg_dir(name, version).join("desc")).ok()?;
    let mut lines = desc.lines();
    while let Some(line) = lines.next() {
        if line == "%REASON%" {
            return lines.next()?.trim().parse().ok();
        }
    }
    None
}

#[test]
#[ignore = "requires libalpm + system pacman.conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_d_no_flags_exits_one_with_usage_hint() {
    miz()
        .arg("-D")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("no operation specified"));
}

#[test]
#[ignore = "requires libalpm + system pacman.conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_dk_runs_against_host_dbs() {
    let _ = miz().arg("-Dk").assert();
}

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_d_asdeps_marks_explicit_package_as_dependency() {
    let root = make_test_root();
    let pkg = FakePkg {
        reason: 0,
        ..FakePkg::minimal("foo", "1.0-1")
    };
    install_fake_pkg(&root, &pkg);
    assert_eq!(read_reason(&root, "foo", "1.0-1"), Some(0));

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-D",
            "--asdeps",
            "foo",
        ])
        .assert()
        .success();

    assert_eq!(read_reason(&root, "foo", "1.0-1"), Some(1));
}

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_d_asexplicit_marks_dependency_as_explicit() {
    let root = make_test_root();
    let pkg = FakePkg {
        reason: 1,
        ..FakePkg::minimal("bar", "2.0-1")
    };
    install_fake_pkg(&root, &pkg);
    assert_eq!(read_reason(&root, "bar", "2.0-1"), Some(1));

    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-D",
            "--asexplicit",
            "bar",
        ])
        .assert()
        .success();

    assert_eq!(read_reason(&root, "bar", "2.0-1"), Some(0));
}

#[test]
#[ignore = "requires libalpm + pacman-conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_d_asdeps_unknown_package_errors() {
    let root = make_test_root();
    miz()
        .args([
            "--root",
            root.path.to_str().unwrap(),
            "--config",
            root.config_path().to_str().unwrap(),
            "-D",
            "--asdeps",
            "nope",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nope"));
}

#[test]
fn dash_d_asdeps_and_asexplicit_conflict() {
    miz()
        .args(["-D", "--asdeps", "--asexplicit", "foo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--asdeps").and(predicate::str::contains("--asexplicit")));
}
