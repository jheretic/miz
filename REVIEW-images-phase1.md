# REVIEW — `miz -I` Phase 1 scaffold (commit `9dfbf2f`)

Adversarial review against `PLAN-images.md`. Read-only; nothing fixed.
Reviewed task: "miz -I Phase 1 review". HEAD = `9dfbf2f`.

## TL;DR — BLOCKERS (read first)

1. **`cargo test` is RED.** This commit broke the existing integration test
   `dash_i_images_is_not_yet_implemented` (crates/miz/tests/cli.rs:17). CI's
   `cargo test --verbose` step will fail on this commit. Regression.
2. **Three wrong D-Bus proxy signatures** in `operations/images/client.rs`:
   `Vacuum`, `Acquire`, `Install`. All three compile but will fail at runtime
   when phases 2-4 wire them. The PLAN's own "Verified D-Bus surface" section is
   *also* wrong for these, and the code faithfully copied the wrong plan.

Verdict: **NOT a clean foundation.** 2 blockers, 1 major, 3 minor.

---

## Findings

### [blocker] `cargo test` regression: removed the "not yet implemented" message

- Where: regression spans crates/miz/src/operations/images/mod.rs (run) vs the
  test at crates/miz/tests/cli.rs:17-23.
- What: the pre-commit stub (`bc67dab:crates/miz/src/operations/images.rs`)
  printed `eprintln!("miz: -I/--images is not yet implemented")` before returning
  `MizError::NotImplemented`. The new `run()` dropped that `eprintln!`. The only
  stderr now is `error: not implemented` (from `MizError::NotImplemented`'s
  Display + main.rs:76). The test asserts stderr contains **"not yet
  implemented"** — it no longer does.
- How I know: ran `cargo test -p miz` against the libalpm stub. Result:
  `test result: FAILED. 3 passed; 1 failed`. Failure output:
  `Unexpected stderr, failed var.contains(not yet implemented) ... stderr="error: not implemented\n"`.
  Confirmed via `git show bc67dab:.../images.rs` that the old stub emitted the
  string; `grep -rn "not yet"` finds it only in the test now. CI step
  `cargo test --verbose` (.github/workflows/ci.yml:26) runs this non-ignored
  test -> CI breaks on this commit.
- Suggested fix: either re-emit the message in `run()` (or each stub), or update
  the test assertion to "not implemented". (Don't fix here — flagging.)

### [blocker] Wrong D-Bus signature: `Target.Vacuum`

- Where: crates/miz/src/operations/images/client.rs:60 — `fn vacuum(&self) -> zbus::Result<u32>`.
- What: the real method returns **two** `u` values, not one:
  `Vacuum(out u instances, out u disabled_transfers)`. A proxy declaring a single
  `u32` return will fail to deserialize the reply at runtime (zbus body is
  `(uu)`, not `u`).
- How I know: fetched the systemd man page
  (freedesktop.org/software/systemd/man/latest/org.freedesktop.sysupdate1.html),
  stripped markup, extracted: `Vacuum(out u instances, out u disabled_transfers);`.
  PLAN-images.md §"Verified D-Bus surface" says `Vacuum() -> u (count deleted)` —
  the plan is wrong too; the code matched the wrong plan.
- Suggested fix: `fn vacuum(&self) -> zbus::Result<(u32, u32)>`.

### [blocker] Wrong D-Bus signature: `Target.Acquire` / `Target.Install`

- Where: client.rs:53 (`acquire`) and client.rs:56 (`install`), both
  `(&self, version: &str, flags: u64) -> zbus::Result<OwnedObjectPath>`.
- What: real signature is
  `Acquire(in s new_version, in t flags, out s new_version, out t job_id, out o job_path)`
  (identical shape for `Install`). Return is a **3-tuple** `(s, t, o)`
  (new_version, job_id, job_path), not a bare `o`. The proxy will fail to
  decode the reply at runtime.
- How I know: same man-page extraction:
  `Acquire(in s new_version, in t flags, out s new_version, out t job_id, out o job_path);`
  and identical for `Install`. The worker's in-code NOTE (client.rs:48-51)
  flagged the *input* signature as a phase-3 guess and "-> o" — but the actual
  miss is the *output*: it's `(s, t, o)`, and the NOTE understates it by
  asserting "-> o". The input (`s version, t flags`) happens to be correct;
  the documented return is what's wrong.
- Severity rationale: blocker as a *foundation defect* (this is exactly what a
  phase-1 review exists to catch), though note these are unused in phase 1
  (dispatch returns NotImplemented), so nothing fails at phase-1 runtime. The
  `#[allow(dead_code)]` on the client module (mod.rs:8) masks them until phase 3.
- Suggested fix: `-> zbus::Result<(String, u64, OwnedObjectPath)>` for both, and
  correct the NOTE; verify against a live service in phase 3 as planned.

### [major] `system_connection()` only catches bus-connect failure, not the service-unavailable case the PLAN names

- Where: client.rs:108-115.
- What: PLAN §2 "Connection" requires wrapping "the service-not-found /
  sysupdated-not-running case (older systemd, service masked)" in a clean
  `MizError::Sysupdate("... requires systemd 257+")`. The helper only maps a
  failure of `Connection::system()` — i.e. the *bus itself* being unreachable.
  When the bus is up but `org.freedesktop.sysupdate1` is masked/absent (the
  common "older systemd" case the message literally describes), `system()`
  succeeds; the failure surfaces only on the first method call, as a raw
  `zbus::Error` -> `MizError::Dbus(...)` with a stack-trace-y message, not the
  friendly "requires systemd 257+" string.
- How I know: read client.rs:108-115; `Connection::system()` connects to the bus,
  not to a service. Service-activation/availability is checked at method-call
  time. The error message attached here (`requires systemd 257+`) is therefore
  attached to the wrong failure mode.
- Severity: major (the clean-degradation contract in PLAN is not met for its own
  stated scenario), but mitigated: it's phase-1 scaffold and no call site exists
  yet. Acceptable to defer to phase 2 *if* tracked. Flagging because the message
  string is actively misleading where it sits.
- Suggested fix: keep a generic bus-connect error here; do the
  service-availability probe (e.g. a cheap `ListTargets`/name-owner check) in
  phase 2 and map *that* to the "requires systemd 257+" message.

### [minor] Plain `cargo build` emits 2 dead_code warnings

- Where: mod.rs:54 (`split_component` "never used") — and the build reports 2
  warnings total.
- What: `split_component` is only referenced from `#[cfg(test)]`, so a non-test
  build warns. CI's clippy gate uses `cargo clippy --tests -- -D warnings`
  (ci.yml:24), which *passes* because `--tests` brings the test refs in — I
  verified `cargo clippy --tests -p miz -- -D warnings` exits 0. But
  `cargo build --verbose` (ci.yml:20, no `-D`) prints the warnings without
  failing. So this won't break CI, but it's avoidable noise in the foundation.
- How I know: `cargo check -p miz` -> "function `split_component` is never used";
  `cargo build -p miz 2>&1 | grep -c warning` -> 2. `cargo clippy --tests` -> clean,
  exit 0.
- Suggested fix: use the helper from a non-test path (phase 2 will), or
  `#[cfg(test)]`-gate it, or `#[allow(dead_code)]` it consistently with the
  module stubs.

### [minor] Dispatch order does not mirror `sync::run` and inverts clean/upgrade priority

- Where: mod.rs:21-49 vs sync.rs:16-46.
- What: PLAN §1 says "Dispatch mirrors `sync::run`'s priority-ordered field
  checks." `sync::run` checks `clean` **first** (sync.rs:16). `images::run`
  checks `list, info, check_new, components, pending, features, upgrade, clean,
  reboot` — clean is near-last, after upgrade. The chosen order (read-only verbs
  first, mutating last) is *defensible and arguably better*, but it is not a
  mirror of sync, and for a combined invocation like `-Iuc` the two operations
  resolve a different winner than the sync analog would. Calling it out as drift
  from the stated spec, not a correctness bug (every branch returns
  NotImplemented today).
- How I know: side-by-side read of both `run` functions.
- Suggested fix: either align the comment to "priority-ordered (read-only first)"
  to stop claiming it mirrors sync, or document why the order differs.

### [minor] In-code NOTE on Acquire/Install is itself inaccurate

- Where: client.rs:48-51.
- What: the NOTE says the surface is "not pinned ... beyond '-> o'" and the guess
  is "version + flags". The man page *does* pin it, and the return is `(s,t,o)`
  not `o`. The NOTE gives false confidence that only the inputs are unverified.
- How I know: see the Acquire/Install blocker above.
- Suggested fix: correct the NOTE to reflect the `(s,t,o)` return and that the
  man page is authoritative.

---

## Checks requested by the task — pass/fail

1. **Args struct matches §1, lives in cli/args.rs, clap-only** — **PASS** (with
   the documented `--reboot` deviation, see below). args.rs:265-322 carries every
   field from §1's struct; no `zbus`/`serde_json`/`alpm` import in args.rs (it
   imports only `std::path::PathBuf`). build.rs still compiles (cargo check
   green). `--reboot` is long-only, correctly justified: `-b` is the global
   `--dbpath` (cli/mod.rs:23), so `-Ib` is impossible — verified `miz -I --help`
   shows `-b, --dbpath` and `--reboot` with no collision. **No other images
   short flag collides** with the globals `-r`/`-b`/`-v`: images uses
   `l/i/y/u/c/p/g/f/q`; globals use `r/b/v`; disjoint. `-Iq` (quiet) and `-v`
   coexist. Confirmed by a clean `--help` render. The deviation is the right
   call.
2. **zbus proxy defs match verified surface** — **FAIL.** Correct:
   `ListTargets a(sso)`, `ListJobs a(tsuo)`, `ListAppStream as`, `JobRemoved(t,o,i)`,
   `List(t)->as`, `Describe(s,t)->s`, `CheckNew()->s`, `GetVersion()->s`,
   `ListFeatures(t)->as`, `DescribeFeature(s,t)->s`,
   `SetFeatureEnabled(s,i,t)`, Target props `Class/Name/Path` (all `s`), Job props
   `Id(t)/Type(s)/Offline(b)/Progress(u)` (the `#[zbus(property, name = "Type")]`
   rename at client.rs:96 is correct). **Wrong: `Vacuum` (should be `(u,u)`),
   `Acquire`/`Install` (should return `(String,u64,OwnedObjectPath)`).**
   See blockers.
3. **Connection helper handles service-unavailable cleanly (no panic)** —
   **PARTIAL.** No panic (maps to `MizError::Sysupdate`), but only covers
   bus-connect failure, not the masked/absent-service case the message claims to
   cover. See major finding.
4. **MizError::{Sysupdate,Dbus} -> GENERIC** — **PASS.** error.rs:21-24 adds both
   (`Sysupdate(String)`, `Dbus(#[from] zbus::Error)`); exit_code() maps both to
   `exit::GENERIC` (error.rs:48-49). `Dbus`'s `#[from]` makes `?` on zbus calls
   ergonomic.
5. **Dispatch mirrors sync priority ordering; context-less** — **PARTIAL.**
   Context-less signature `pub fn run(args: Args) -> Result<()>` (mod.rs:20) ok;
   wired in main.rs:49 with `Images(_)` in the `needs_context` exclusion
   (main.rs:29) ok. But the priority order does not mirror sync (see minor).
6. **Module-dir layout per §2, not a crate** — **PASS.**
   `operations/images/{mod,client,format,describe,job}.rs` all present;
   `operations/mod.rs:4` declares `pub mod images;`. No new workspace crate
   (`zbus`/`serde_json` added as deps of the existing miz crate, Cargo.toml:15,18;
   crates/miz/Cargo.toml:28,30). Stubs describe.rs/format.rs/job.rs are
   one-line doc-comment placeholders — appropriately minimal.
7. **Comment count near zero; clippy clean** — **PARTIAL.** Comments are doc
   comments + a couple of NOTEs; reasonable, not excessive. `cargo clippy --tests
   -- -D warnings` is clean (exit 0). But plain `cargo build` emits 2 dead_code
   warnings (see minor), and `cargo test` is RED (blocker 1).

---

## What I checked

- **D-Bus signatures**: thoroughly. Cross-checked every Manager/Target/Job
  method and property in client.rs against the live systemd man page
  (org.freedesktop.sysupdate1) fetched this session, not just against the PLAN.
  Found the PLAN itself is wrong for Vacuum/Acquire.
- **Build/test/clippy**: ran `cargo check`, `cargo build`, `cargo test -p miz`,
  `cargo clippy --tests -p miz -- -D warnings`, and `miz -I --help` against the
  /tmp/fake-alpm stub (PKG_CONFIG_PATH=/tmp/fake-pc, LD_LIBRARY_PATH set). Unit
  tests (split_component x3) pass; the cli.rs integration test fails.
- **Flag collisions**: enumerated images short flags vs the three global short
  flags; confirmed disjoint and verified via `--help` render.
- **Dispatch / context wiring**: read mod.rs `run`, main.rs dispatch + needs_context,
  compared ordering to sync.rs.
- **Error mapping**: read error.rs variants + exit_code.
- **Module layout / crate-vs-module**: read operations/mod.rs, Cargo.toml deps,
  the four stub files.
- **build.rs alpm-free constraint**: confirmed args.rs imports only std; build
  compiles.
- **Regression origin**: traced the broken test to the dropped `eprintln!` via
  `git show bc67dab`.

## What I did NOT check

- **Live D-Bus behaviour**: no systemd-sysupdated on this host (and the alpm stub
  aborts on real calls). Signature correctness is verified against the man page,
  not a running service — exactly the phase-3 verification the worker deferred.
- **`cargo fmt --check`**: reports a diff, but only in `crates/miz/src/config.rs`
  (pre-existing, untouched by this commit) — out of scope for this review.
- **clap_mangen manpage regeneration output**: build.rs compiles and the PLAN's
  "manpage regenerates" claim is plausible, but I did not diff generated man
  output.
- **VCS artefacts** (commit message quality, staging): out of review scope.
