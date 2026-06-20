# PLAN — `miz -I` / `--images` (systemd-sysupdated over D-Bus)

Status: design, not yet implemented. Separate from the main `PLAN.md`.

## Goal (restated)

Replace the `-I` stub with a real operation that talks to **systemd-sysupdated**
(`org.freedesktop.sysupdate1`, system bus) and exposes a pacman-style sub-flag
CLI mirroring `systemd-sysupdate`'s verbs, so an Archetype user manages OS image
updates with the same muscle memory as `-S`/`-Q` package operations.

Anchor given by the user: `systemd-sysupdate list` -> `miz -Il`.

## Context found (verified by reading the tree)

- `crates/miz/src/cli/mod.rs` — `Operation::Images(args::images::Args)` already
  registered, `short_flag = 'I'`, `long_flag = "images"`. No change to the enum.
- `crates/miz/src/cli/args.rs` — `images::Args` is `{ pub targets: Vec<String> }`.
  **Hard constraint:** `build.rs` imports `src/cli/mod.rs` (which re-exports
  `args`) via `#[path]` for `clap_mangen`, so this module must stay clap-only —
  no zbus, no serde_json, no alpm. The sub-flag struct lives here; the D-Bus
  client does not.
- `crates/miz/src/operations/images.rs` — current stub returns
  `MizError::NotImplemented`. Becomes a module directory (see Step 1).
- `crates/miz/src/operations/sync.rs` — **the dispatch reference.** `run(args, ctx)`
  checks each sub-flag field in priority order and delegates to a private
  `sync_<mode>` helper. `split_repo_target()` there splits `repo/pkg` on `/` —
  reuse that idiom for `component/version`.
- `crates/miz/src/operations/query.rs` — formatters are already `pub(crate)`:
  `format_size` (L378), `format_date` (L389), `format_validation` (L442),
  `join_list_str`/`join_dep_list`/`join_optdeps`. `-Ii` reuses `format_date`/
  `format_size`; the `{:<19}: {}` label idiom in `print_sync_info` is the model
  for `-Ii` output.
- `crates/miz/src/operations/progress.rs` — indicatif `MultiProgress` + bar
  styles, but every callback is shaped around alpm's `set_progress_cb`/`set_dl_cb`.
  Not directly reusable for D-Bus Jobs; `bar_style_dl()` template can be lifted.
- `crates/miz/src/main.rs` — `dispatch()` lists `Images(_)` in the
  `needs_context` exclusion (no alpm handle). Keep it context-less:
  `Operation::Images(args) => operations::images::run(args)`.
- `crates/miz/src/error.rs` — `MizError` (thiserror), `exit_code()` maps variants
  to `exit::{GENERIC=1, ALPM=2, DEPTEST=127}`.
- `crates/miz-config/Cargo.toml` — precedent for a split-out crate; justified
  there because the schema must be alpm-free for build.rs. That justification
  does **not** transfer here (the Args struct already carries the build.rs
  constraint; the client has no second consumer), so images is a module, not a
  crate (see Rejected alternatives).
- Test gating: `tests/*.rs` use `#[ignore = "... MIZ_HAS_ALPM=1"]`. Mirror with
  `MIZ_HAS_SYSUPDATE=1`.

## Verified D-Bus surface (systemd 261 man pages — cited, not re-hedged)

Service `org.freedesktop.sysupdate1`, **system bus**.

- Manager `/org/freedesktop/sysupdate1` iface `...Manager`:
  `ListTargets() -> a(sso)` (class, name, path); `ListJobs() -> a(tsuo)`;
  `ListAppStream() -> as`; signal `JobRemoved(t id, o path, i status)`
  (0 = success, >0 = exit code, <0 = -errno).
- Target (path from ListTargets) iface `...Target`:
  `List(t flags) -> as` (flag `SD_SYSUPDATE_OFFLINE = 1<<0` = installed-only);
  `Describe(s version, t flags) -> s` (JSON: version, newest, available,
  installed, obsolete, incomplete, changelog, contents...);
  `CheckNew() -> s` (newest available, "" if none; polkit: no auth);
  `Acquire(...)`/`Install(...)` -> Job path (polkit: `update` no-auth when no
  version; `update-to-version` **admin auth** when a version is named);
  `Vacuum() -> u` (count deleted; polkit: **admin auth**);
  `GetVersion() -> s`; `GetAppStream()`, `ListFeatures()`, `DescribeFeature()`,
  `SetFeatureEnabled()`; props `Class`, `Name`, `Path`.
- Job iface `...Job`: props `Id`(t), `Type`(list/describe/check-new/acquire/
  install/vacuum/describe-feature), `Offline`(b), `Progress`(u, 0-100, only
  meaningful for acquire/install).

## 1. Flag mapping (the core design)

**Target/version selection.** Positional arg = the **component/target**
(default `"host"` if omitted), mirroring `-Sl <repo>`. An optional version is
expressed with pacman's slash idiom `component/version` (reuse
`split_repo_target`'s logic), mirroring `-S repo/pkg`. So `miz -Iu` updates host
to newest; `miz -Iu host/2.3` pins a version; `miz -Il foo` lists component
`foo`. Rejected: a separate `-C <name>` option (sysupdate's own spelling) —
it breaks the pacman muscle-memory that positional = the thing you operate on,
which is the entire point of the feature.

| systemd-sysupdate verb | D-Bus method | miz flag | notes |
|---|---|---|---|
| `list [VERSION]` | `Target.List` (+`Describe` per version) | `-Il` | given. Renders like `-Sl`: one line per version, `[installed]`/`[newest]` markers. |
| (describe / info) | `Target.Describe` | `-Ii` | mirrors `-Si`/`-Qi`. `-ii` = verbose (changelog/contents). Needs a `version` positional or describes newest. |
| `check-new` | `Target.CheckNew` | `-Iy` | refresh-like: contacts network, reports newest. No download. |
| `update [VERSION]` | `Target.Acquire`+`Install` | `-Iu` | strongest fit (upgrade). `count` so `-Iuu` reserved for future force-redownload. |
| `vacuum` | `Target.Vacuum` | `-Ic` | clean-like. `-Ic` is free because check-new took `-Iy`. **This resolves the -Ic contention explicitly: vacuum wins -Ic, check-new is -Iy.** |
| `pending` | `Target.GetVersion` vs `CheckNew`/`List` | `-Ip` | "is an update staged/pending?". Compares current vs newest. |
| `reboot` | logind/`systemctl reboot` (not sysupdate1) | `-Ib` | `b`=boot. Also a `--reboot` **modifier** on `-Iu` (sysupdate's `update -m`). |
| `components` | `Manager.ListTargets` | `-Ig` | components are named collections of targets ~= pacman groups. Imperfect fit, noted; `-g` is the least-bad pacman analog. |
| `features [FEATURE]` | `Target.ListFeatures`/`DescribeFeature`/`SetFeatureEnabled` | `-If` | phase 4 polish. |

Modifiers (long-only, matching miz's `--needed`/`--noconfirm` idiom; they are not
verbs):

- `--offline` -> sets `SD_SYSUPDATE_OFFLINE` on `List`/`Describe` (installed-only,
  no network). Affects `-Il`/`-Ii`.
- `-Iq` / `--quiet` -> suppress markers/extra detail, like `-Slq`.
- `--noconfirm`, `--noprogressbar` -> reuse pacman semantics for `-Iu`.
- `--json=MODE` -> passthrough to print raw `Describe` JSON instead of pacman
  rendering (debugging / scripting). Phase 4.

Resulting `images::Args` (clap-only, stays in `cli/args.rs`):

```rust
pub mod images {
    #[derive(clap::Args)]
    pub struct Args {
        #[arg(short = 'l', long)] pub list: bool,
        #[arg(short = 'i', long, action = clap::ArgAction::Count)] pub info: u8,
        #[arg(short = 'y', long = "check-new")] pub check_new: bool,
        #[arg(short = 'u', long, action = clap::ArgAction::Count)] pub upgrade: u8,
        #[arg(short = 'c', long, action = clap::ArgAction::Count)] pub clean: u8, // vacuum
        #[arg(short = 'p', long)] pub pending: bool,
        #[arg(short = 'b', long)] pub reboot: bool,
        #[arg(short = 'g', long)] pub components: bool,
        #[arg(short = 'f', long)] pub features: bool,
        #[arg(long)] pub offline: bool,
        #[arg(short = 'q', long)] pub quiet: bool,
        #[arg(long)] pub noconfirm: bool,
        #[arg(long)] pub noprogressbar: bool,
        #[arg(long, value_name = "MODE")] pub json: Option<String>,
        pub targets: Vec<String>, // "component" or "component/version"
    }
}
```

Dispatch mirrors `sync::run`'s priority-ordered field checks, but with no
`Context` (signature `pub fn run(args: Args) -> Result<()>`).

## 2. D-Bus client architecture

- **Crate:** `zbus` (workspace dep), **blocking API** (`zbus::blocking`). Defence:
  pure-Rust, no libdbus C link step, `#[proxy]` codegen is ergonomic; blocking
  variant means miz stays sync and pulls **no** tokio runtime just for `-I`.
  Rejected: the `dbus` crate — adds a libdbus C dependency and hand-rolled
  message marshalling for no benefit here.
- **Location:** module directory `crates/miz/src/operations/images/` —
  `mod.rs` (dispatch, mirrors `sync::run`), `client.rs` (zbus `#[proxy]` defs for
  Manager/Target/Job + connection helper), `format.rs` (pacman-style rendering),
  `describe.rs` (serde structs for the `Describe` JSON). Rejected: a new
  `miz-sysupdate` crate — the miz-config split was justified by build.rs needing
  the schema alpm-free; that does not apply (Args already lives in the alpm-free
  `args.rs`, and the client has zero second consumer). A crate adds workspace
  ceremony for no reuse.
- **Connection:** `zbus::blocking::Connection::system()`. Wrap the
  service-not-found / sysupdated-not-running case (older systemd, service
  masked) in a clean `MizError::Sysupdate("systemd-sysupdated is not available
  (requires systemd 257+)")` — never a panic.
- **Target resolution:** `ListTargets()` -> match the positional component name
  (default `"host"`) to its object path, then build a `TargetProxy` on that
  path. Unknown component -> `MizError::Sysupdate("no such component: ...")`.
- **Job handling (Acquire/Install):** the method returns a Job object path.
  Subscribe to `Manager.JobRemoved` *before* issuing the call, then poll the
  Job's `Progress` property into an indicatif bar (lift `bar_style_dl()` from
  `progress.rs`, made `pub(crate)`), and finish/error on the `JobRemoved` signal
  (status 0 = ok, >0 exit code, <0 -errno). A small `job.rs` helper owns this
  loop — do **not** contort the alpm-callback-shaped `progress.rs`.

## 3. Output formatting

- `-Il`: one line per version string from `List`, marked `[installed]` /
  `[newest]` (cross-reference `GetVersion`/`Describe.newest`), echoing
  `sync_list`'s `{repo} {pkg} {ver} {suffix}` shape. `-Ilq` prints bare version
  strings.
- `-Ii`: parse the `Describe` JSON (serde_json) into a `describe.rs` struct and
  render with the `{:<19}: {}` label idiom from `print_sync_info`; reuse
  `format_size`/`format_date` for sizes/timestamps. `-iii`+`--json` dumps raw.
- `-Ig` (components): print `class name` per `ListTargets` row.
- Add `serde_json` (workspace dep) — not currently present. `serde` already is.

## 4. Error type + exit codes

- Add to `MizError`: `#[error("sysupdate: {0}")] Sysupdate(String)` and
  `#[error("dbus: {0}")] Dbus(#[from] zbus::Error)`. Map both to `exit::GENERIC`
  in `exit_code()` (no new exit constant — D-Bus failure is a generic runtime
  failure, consistent with Io/Toml). Rejected: a dedicated exit code — pacman has
  no analog and nothing consumes it.
- **Polkit auth boundary (explicit):** read-only verbs (`-Il -Ii -Iy -Ig -Ip`)
  need no auth. `-Iu host/<version>` (`update-to-version`) and `-Ic` (vacuum)
  need admin auth; the D-Bus call fails with an auth error when unprivileged.
  Catch that specific failure and surface
  `MizError::Sysupdate("this operation requires elevated privileges (run as root or via polkit)")`
  — never a raw zbus stack trace.

## 5. Testing strategy

- **Unit (no bus):** clap parsing of every sub-flag and `component/version`
  splitting; `Describe` JSON -> struct deserialization (fixture JSON strings);
  pacman-style formatter output (golden strings). Live in
  `operations/images/` `#[cfg(test)]` modules.
- **Integration (gated):** requires systemd 257+ with sysupdated running and
  configured `sysupdate.d` transfer files — harder to satisfy than the libalpm
  tests. Gate with `#[ignore = "requires systemd-sysupdated; run with MIZ_HAS_SYSUPDATE=1"]`
  in a new `tests/images.rs`, only the read-only verbs (never run a real
  `Acquire`/`Vacuum` in tests). A mock D-Bus service is **out of scope for v1**.

## 6. Phasing (each phase committable)

1. **Scaffold** — Args struct + full flag scheme, `MizError::{Sysupdate,Dbus}`
   + `exit_code()`, `zbus`+`serde_json` deps, `operations/images/` module
   skeleton with zbus `#[proxy]` defs and a connection helper, dispatch that
   compiles and returns `NotImplemented` for each unwired mode. Verifiable:
   `cargo build`, `miz -I --help` shows the flags, manpage regenerates.
2. **Read-only verbs** — `-Il`, `-Ii`, `-Iy`, `-Ig`, `-Ip` via List/Describe/
   CheckNew/ListTargets/GetVersion; `--offline` + `-q`; pacman formatters +
   `describe.rs`. All polkit-no-auth. Verifiable: unit tests pass; on a live
   host `miz -Il` matches `systemd-sysupdate list`.
3. **Mutating verbs** — `-Iu` (Acquire+Install + Job progress bar +
   `--reboot`), `-Ic` (Vacuum), `-Ib` (reboot); polkit auth-required handling.
   Shares the client/connection from phase 2.
4. **Polish** — `-If` features (ListFeatures/DescribeFeature/SetFeatureEnabled),
   appstream (`ListAppStream`/`GetAppStream`), `--json` passthrough, component
   grouping niceties, docs/man review.

## Rejected alternatives (summary)

- Separate `miz-sysupdate` crate — no second consumer; build.rs constraint is
  already satisfied by Args living in `args.rs`.
- `dbus` (libdbus) crate — C link dependency, worse ergonomics than zbus.
- `-C <component>` option — breaks the positional muscle memory that is the
  feature's whole point; use `component/version` instead.
- async zbus + tokio — miz is sync; blocking zbus avoids a runtime.

## Weakest assumption

That the `Describe` JSON keys (version/newest/installed/available/obsolete/
incomplete/changelog/contents) are stable enough across systemd 257->261 to
deserialize into fixed serde structs; if not, `describe.rs` must fall back to a
permissive `serde_json::Value` map. (Verified the key *names* from the 261 man
page; not verified their stability across the 257-261 range.)

## Follow-ups (out of scope for v1)

- Mock D-Bus service for hermetic integration tests.
- `--json` machine-readable output for all verbs (not just describe).
- Optional-feature management UX beyond a thin `-If` wrapper.
- Surfacing `ListJobs()` (concurrent jobs from other clients).

## Critical files for implementation

- /home/n0n/src/archetype/miz/crates/miz/src/cli/args.rs — add the `images::Args` fields (clap-only; build.rs constraint).
- /home/n0n/src/archetype/miz/crates/miz/src/operations/images.rs — becomes `images/` module dir; dispatch modeled on `sync::run`.
- /home/n0n/src/archetype/miz/crates/miz/src/operations/sync.rs — dispatch + `split_repo_target` reference.
- /home/n0n/src/archetype/miz/crates/miz/src/operations/query.rs — `pub(crate)` formatters to reuse for `-Ii`.
- /home/n0n/src/archetype/miz/crates/miz/src/error.rs — add `Sysupdate`/`Dbus` variants + `exit_code()`.
