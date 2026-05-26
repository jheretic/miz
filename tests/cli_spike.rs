// Spike for PLAN.md §6 risks #1 and #2.
// Verifies clap-4 derive `short_flag` / `long_flag` on subcommands and
// pacman-style bundled short flags (`-Sy`, `-Syu`).
//
// Witness: parse succeeded and dispatch reached past clap if the binary
// runs `config::build` (which fails predictably against a non-existent
// --root + missing /etc/pacman.conf) rather than aborting with a clap
// parse error. Updated for Phase 1.2 (config::build now runs before any
// op stub).

use assert_cmd::Command;
use predicates::prelude::*;

mod common;
use common::miz;

fn assert_dispatched(cmd: &mut Command) {
    cmd.args(["--root", "/nonexistent/miz/spike/root"])
        .assert()
        .failure()
        .code(predicate::ne(0))
        .stderr(predicate::str::contains("error:"))
        .stderr(predicate::str::contains("unexpected argument").not())
        .stderr(predicate::str::contains("invalid value").not())
        .stderr(predicate::str::contains("unrecognized").not());
}

#[test]
fn dash_s_target_parses() {
    assert_dispatched(miz().args(["-S", "foo"]));
}

#[test]
fn long_sync_target_parses() {
    assert_dispatched(miz().args(["--sync", "foo"]));
}

#[test]
fn dash_sy_bundled_parses() {
    assert_dispatched(miz().arg("-Sy"));
}

#[test]
fn dash_syu_bundled_parses() {
    assert_dispatched(miz().arg("-Syu"));
}
