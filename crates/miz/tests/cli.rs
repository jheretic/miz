use predicates::prelude::*;

mod common;
use common::miz;

#[test]
fn dash_v_prints_version_and_exits_zero() {
    miz()
        .arg("-V")
        .assert()
        .success()
        .stdout(predicate::str::contains("miz"))
        .stdout(predicate::str::is_match(r"\bv\d+\.\d+\.\d+").expect("version regex"));
}

#[test]
fn dash_i_images_is_not_yet_implemented() {
    miz()
        .args(["-I", "anything"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not yet implemented"));
}

#[test]
fn dash_q_with_nonexistent_root_fails_cleanly() {
    miz()
        .args(["--root", "/nonexistent/miz/test/root", "-Q"])
        .assert()
        .failure()
        .code(predicate::ne(0))
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn dash_q_help_lists_query_flags() {
    miz()
        .args(["-Q", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--info"))
        .stdout(predicate::str::contains("--list"))
        .stdout(predicate::str::contains("--search"))
        .stdout(predicate::str::contains("--owns"));
}

#[test]
#[ignore = "requires libalpm + system pacman.conf; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_qi_nonexistent_package_exits_nonzero() {
    miz()
        .args(["-Qi", "definitely-not-a-real-package-xyz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("was not found"));
}
