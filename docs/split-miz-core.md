# Plan: split `miz-core` library crate out of the `miz` binary

Status: planned 2026-07-11. Follows the structured-results refactor (Phases 0-5,
committed through 0ef642b) which decoupled core logic from presentation and
recorded the seam in `docs/CORE-SEAM.md`. THIS is the crate split that refactor
deferred. Stays within scope of creating `miz-core` + slimming `miz`; the `mizd`
daemon is a later step (attach point noted only).

## Target workspace

```
crates/
  miz-config    (unchanged)  serde + toml only. NO libalpm. Below miz-core.
  miz-core      (NEW, lib)   libalpm-linked core logic + abstractions.
  miz           (bin)        clap CLI + render presentation layer. Depends on miz-core.
  miz-convert   (unchanged)  pacmanconf + miz-config. NO libalpm.
```

Dependency DAG: `miz -> miz-core -> miz-config`; `miz-convert -> miz-config`.
`miz-core` and `miz-convert` both sit above `miz-config`; neither depends on the
other. Only `miz-core` (and transitively `miz`) links libalpm.

### miz-core module tree (moved from crates/miz/src/)

```
crates/miz-core/
  Cargo.toml        # deps: alpm, alpm-sys, zbus, miz-config, thiserror, regex,
                    #       object, libc, serde/serde_json (as used) — NO clap,
                    #       NO indicatif, NO console.
  build.rs?         # ONLY if libalpm pkg-config probing must live here (see below)
  src/
    lib.rs          # pub mod common; pub mod operations; pub mod config; pub mod error;
    common/  { exit, transaction, imagedb, osrelease, progress, report, fmt }.rs
    operations/
      { database, query, remove, sync, deptest, upgrade, files, version }.rs
      images/ { mod, client, describe, format, job, relay }.rs
    config.rs
    error.rs
    params.rs       # NEW: neutral per-verb params structs (no clap) — see decision
```

### slimmed miz bin (crates/miz/src/)

```
  main.rs           # dispatch; clap Args -> miz_core params; wires TtyConfirmer + IndicatifSink; ExitCode
  cli/ { mod, args }.rs   # clap Cli + Args (stay here; alpm-free)
  render/ { palette, fmt, confirm, progress_indicatif, completions,
            database, query, remove, sync, deptest, upgrade, files, version, images }.rs
  build.rs          # manpage generation (STAYS in bin; see build.rs section)
```

miz bin Cargo.toml deps: `miz-core`, clap, clap_complete, clap_mangen,
indicatif, console. (alpm links transitively through miz-core.)

## Decision: how operations receive their arguments — **Option (b), neutral params in miz-core**

Today every operation does `pub use crate::cli::args::<verb>::Args;` and those 8
`Args` structs are `#[derive(clap::Args)]` with `#[arg(...)]` attributes
(`src/cli/args.rs`). Two ways to break the operation→clap dependency:

- **(a) Move the `Args` structs into miz-core** — drags a `clap` dependency into
  the core library. REJECTED.
- **(b) Define neutral params structs in `miz-core::params`** (plain structs, no
  clap), each operation `run()` takes its params type; the `miz` bin converts
  `cli::args::<verb>::Args -> miz_core::params::<verb>` in main.rs (a `From`
  impl per verb). CHOSEN.

### Why (b), decisively (not a soft preference)

`crates/miz/build.rs` generates manpages by compiling `src/cli/mod.rs`
standalone via `#[path = "src/cli/mod.rs"] mod cli;` and calling
`Cli::command()`. Its own header documents the invariant: *"The `cli` module is
alpm-free (Args structs live in `src/cli/args.rs`); the alpm-using
`operations::*::run()` impls are not pulled in here."* If `Args` moved into
alpm-linked `miz-core`, then `cli/mod.rs` would `use miz_core::…`, and
**manpage generation at build time would require linking libalpm** (the
fake-alpm stub on the dev host). That structural fact — not "is clap acceptable
in a lib" — kills (a). A daemon linking miz-core also should not pull clap.

Cost of (b): the 8 `Args` field types are ALL already plain (`bool`, `u8`,
`String`, `Vec<String>`, `Option<String>`) — the only clap-specific content is
the derive/attr lines. So each `miz_core::params::<verb>` is a near-verbatim
field copy minus attributes, plus a mechanical `From<cli::args::<verb>::Args>`
in the bin. ~8 structs + 8 `From` impls. Bounded and boilerplate-y, not clever.

`config.rs::build_with_dbext(cli: &Cli, …)` ALSO couples to clap
(`use crate::cli::Cli`) — `CORE-SEAM.md`'s "known seam nuance" missed this (it
only flagged operations). It must take neutral inputs too: pass the already-
resolved `root`/`dbpath`/`config`-path values (or a small `ContextParams`), built
in the bin from `Cli`, rather than `&Cli`.

### Weakest assumption (flag for the worker + reviewer)

That the clap `Args` encode no *behavioral* contract a `run()` silently relies on
— e.g. a `run()` assuming clap already rejected an illegal flag combo
(`conflicts_with`). Validation stays in clap (bin side); the `tests/` exit-code +
stdout assertions are the backstop. The params conversion must preserve every
field's meaning 1:1.

## Stale `CORE-SEAM.md` items to reconcile (Phase 3)

1. It lists `completions.rs` under `operations/` → core. WRONG: Phase 4+5 moved
   it to `src/render/completions.rs` (writes shell completions to stdout, needs
   the full clap `Cli::command()`). Completions stays in the BIN.
2. It does not mention `config.rs`'s `use crate::cli::Cli` coupling. Add it.

## build.rs / libalpm linkage

- **Manpage build.rs STAYS in the miz bin.** It needs the clap `Cli` (bin-side)
  and, per decision (b), compiles `cli/mod.rs` alpm-free — so it keeps working
  without pulling miz-core. Confirm it still only rerun-if-changed on the bin's
  cli files.
- **libalpm pkg-config linkage**: the `alpm`/`alpm-sys` crates carry their own
  build scripts that probe libalpm; that linkage follows the `alpm` dep into
  miz-core. The dev-host fake-alpm stub (`/tmp/fake-alpm`, `notes/miz-build-env.md`)
  satisfies the link for whichever crate depends on `alpm-sys` — after the split
  that is miz-core, and the `miz` bin links it transitively. VERIFY the stub env
  (`PKG_CONFIG_PATH`/`LD_LIBRARY_PATH`) still resolves the link for a
  whole-workspace build; note if miz-core needs the env at its build/test.
- If any hand-rolled libalpm probing exists outside the alpm crates, it moves to
  miz-core. (Current `crates/miz/build.rs` does NOT probe libalpm — it's
  manpages only — so nothing libalpm-related moves out of the bin.)

## Phases (each keeps the WHOLE WORKSPACE green)

CI-parity gate every phase (NOT `-p miz`): `cargo build --verbose`,
`cargo clippy --tests -- -D warnings`, `cargo test --verbose`, `cargo fmt --check`,
with the fake-alpm stub env exported. Unit tests move WITH their modules.

- **Phase 1 — de-clap the core seam (still one crate).** Introduce
  `params` (in `crates/miz/src/` for now), define neutral per-verb params,
  change each `operations::<verb>::run()` to take its params type instead of
  `cli::args::<verb>::Args`, add `From<Args> for Params` in the bin, and change
  `config::build_with_dbext` to take neutral inputs instead of `&Cli`. main.rs
  does the conversion. After this, NOTHING in `operations/`/`config.rs` refers
  to `crate::cli`. Verify: `grep -rn 'crate::cli' src/operations src/config.rs
  src/common` is empty. Behavior/output/exit unchanged. This is the riskiest
  phase (touches every verb's signature) but stays in one crate so it's easy to
  keep green.
- **Phase 2 — create `miz-core`, move the cluster.** Add `crates/miz-core`
  (lib) to the workspace. Move `common/`, `operations/`, `config.rs`, `error.rs`,
  `params.rs` into it; add `lib.rs` re-exporting them. Repoint the `miz` bin's
  imports `crate::{common,operations,config,error,params}` → `miz_core::…`.
  Wire miz-core deps in Cargo.toml + [workspace.dependencies]. Move each
  module's unit tests with it. The `tests/` integration tests stay in the bin
  (they invoke the `miz` binary). Verify the whole-workspace trio green with the
  stub.
- **Phase 3 — trim + document.** Remove now-unused deps from the `miz` bin
  Cargo.toml (alpm/alpm-sys/zbus become transitive — drop direct deps unless the
  bin still needs them directly; check render/ + main.rs). Update
  `docs/CORE-SEAM.md` (completions in bin, config.rs clap nuance, and mark the
  seam REALIZED as miz-core). Add a short `miz-core` crate-level doc comment.
  Note the `mizd` attach point (consumes miz-core's operations + Report +
  ProgressSink/Confirmer, supplies a D-Bus-backed sink + policy confirmer).

Each phase paired with a reviewer running the CI-parity trio + checking exact
CLI behavior preservation and the dependency-direction invariants.

## Risks / pitfalls

- **Args→params conversion must be field-exact** — a dropped/mismapped field
  is a silent behavior change. The `tests/` stdout/exit assertions are the
  backstop; the weakest-assumption note above.
- **build.rs must stay alpm-free** — if Phase 1 accidentally makes `cli/mod.rs`
  pull a params type that pulls alpm, manpage generation would need the stub.
  Keep params conversion in main.rs, not in cli/.
- **Import-path churn** (`crate::` → `miz_core::`) across the bin — large but
  mechanical; do it in Phase 2 as one sweep per module.
- **fake-alpm stub** — after the split, miz-core is the alpm-linking crate;
  confirm the stub env satisfies a whole-workspace build/test and note any
  per-crate env needs. `cargo test` for the bin may still need
  `touch crates/miz/src/main.rs` to relink against the stub.
- **miz-convert stays libalpm-free** — it must NOT gain a miz-core dep; it keeps
  depending on miz-config only. Verify after Phase 2.
- **Preserve exact CLI behavior + the `#[ignore]` integration tests** — they
  assert stdout + exit codes; the split is behavior-neutral.

## Out of scope

- The `mizd` daemon (separate later step). Attach point: a daemon crate depends
  on `miz-core`, calls `operations::<verb>::run(params, &mut Confirmer, &sink)`,
  and supplies a D-Bus-signal `ProgressSink` + a policy `Confirmer` instead of
  the bin's `IndicatifSink`/`TtyConfirmer`.
- Any behavior/output change; any new features.
