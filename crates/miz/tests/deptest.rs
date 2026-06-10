use predicates::prelude::*;

mod common;
use common::miz;

#[test]
#[ignore = "requires libalpm + system pacman.conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_t_no_args_exits_zero() {
    miz().arg("-T").assert().success();
}

#[test]
#[ignore = "requires libalpm + system pacman.conf with bash installed; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_t_installed_pkg_exits_zero() {
    miz().args(["-T", "bash"]).assert().success();
}

#[test]
#[ignore = "requires libalpm + system pacman.conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_t_missing_pkg_exits_127() {
    miz()
        .args(["-T", "definitely-not-real-pkg-xyz"])
        .assert()
        .failure()
        .code(127)
        .stdout(predicate::str::contains("definitely-not-real-pkg-xyz"));
}
