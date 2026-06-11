use predicates::prelude::*;

mod common;
use common::miz;

#[test]
fn dash_f_help_lists_files_flags() {
    miz()
        .args(["-F", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--list"))
        .stdout(predicate::str::contains("--refresh"))
        .stdout(predicate::str::contains("--search"))
        .stdout(predicate::str::contains("--regex"))
        .stdout(predicate::str::contains("--quiet"))
        .stdout(predicate::str::contains("--machinereadable"));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_f_with_no_targets_and_no_refresh_errors() {
    miz()
        .args(["-F"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no targets"));
}

#[test]
#[ignore = "requires libalpm at runtime; run with `cargo test -- --ignored` after `export MIZ_HAS_ALPM=1`"]
fn dash_fl_with_regex_is_rejected() {
    miz()
        .args(["-Flx", "anything"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--regex"));
}

#[test]
#[ignore = "requires libalpm + populated files DBs; run with MIZ_HAS_ALPM=1 and after `pacman -Fy`"]
fn dash_fy_refreshes_files_db() {
    let _ = miz().args(["-Fy"]).assert();
}

#[test]
#[ignore = "requires libalpm + populated files DBs (run `pacman -Fy` first)"]
fn dash_fs_finds_known_path() {
    // Pacman parity: -Fs uses alpm_filelist_contains (exact binary-search
    // match) when the target has a slash. The filelist stores entries
    // without a leading slash and without any transformation. Discover
    // an actual entry from bash's filelist via `-Fl bash`, then ask -Fs
    // for that exact byte sequence. Avoids guessing at /usr-merge or
    // any other path layout convention.
    let output = miz()
        .args(["-Fl", "-q", "bash"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("-Fl stdout utf-8");
    let needle = stdout
        .lines()
        .find(|l| l.contains('/') && !l.ends_with('/'))
        .expect("bash's filelist should have at least one file with a /")
        .to_string();

    miz()
        .args(["-Fs", needle.as_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("bash"));
}

#[test]
#[ignore = "requires libalpm + populated files DBs"]
fn dash_fl_lists_files_for_known_package() {
    miz().args(["-Fl", "bash"]).assert().success();
}

#[test]
#[ignore = "requires libalpm + populated files DBs"]
fn dash_fs_nonexistent_path_exits_nonzero() {
    miz()
        .args(["-Fs", "definitely/not/a/real/path/xyzzy"])
        .assert()
        .failure();
}

#[test]
#[ignore = "requires libalpm + populated files DBs"]
fn dash_fs_machinereadable_uses_null_separators() {
    let output = miz()
        .args(["-Fl", "-q", "bash"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("-Fl stdout utf-8");
    let needle = stdout
        .lines()
        .find(|l| l.contains('/') && !l.ends_with('/'))
        .expect("bash's filelist should have at least one file with a /")
        .to_string();

    miz()
        .args(["-Fs", "--machinereadable", needle.as_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\0"));
}
