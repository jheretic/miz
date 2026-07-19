# The `miz-core` seam (REALIZED)

Status: realized 2026-07-19 (crate split, Phases 1-3 of `split-miz-core.md`).
Originally recorded 2026-07-11 as a map after the structured-results refactor
(Phases 0-5). The seam it described has now been extracted into the `miz-core`
library crate; this doc records the actual topology.

See `docs/refactor-structured-results.md` for the presentation/core decoupling
that made the split mechanical, and `docs/split-miz-core.md` for the split plan.

## Crate topology

```
crates/
  miz-config    serde + toml only. NO libalpm.
  miz-core      libalpm-linked core logic + presentation-neutral abstractions.
  miz           (bin) clap CLI + render presentation layer.
  miz-convert   (bin) pacmanconf + miz-config. NO libalpm.
```

Dependency DAG: `miz -> miz-core -> miz-config`; `miz-convert -> miz-config`.
`miz-core` and `miz-convert` both sit above `miz-config`; neither depends on the
other. Only `miz-core` (and transitively `miz`) links libalpm. `miz-core` does
NOT depend on `clap`, `indicatif`, or `console` — those are bin-only.

## What lives in `miz-core` (libalpm-linked library)

Pure core logic + shared infrastructure. Links libalpm, never touches
stdin/stdout/stderr for presentation, never draws progress bars, never prompts,
never renders.

- `src/common/` — shared infra:
  - `exit.rs` — exit-code constants.
  - `transaction.rs` — `TransGuard`, prepare/commit, the SIGINT/SIGTERM/SIGHUP
    handler (via `ctrlc`). (`eprintln!` failure-point diagnostics remain inline
    here; plain, uncolored — see the invariant note below.)
  - `imagedb.rs` — shared image-db reader (three consumers: `config.rs`,
    `operations/query.rs`, `operations/images`).
  - `osrelease.rs` — os-release / archive-date helpers.
  - `progress.rs` — `ProgressEvent` enum + `ProgressSink` trait + `OpKind` +
    `register()` (the alpm-callback -> event translation). ABSTRACTION ONLY; no
    indicatif/console.
  - `report.rs` — the per-operation `*Report` types, `TransactionPlan` /
    `TransactionKind`, the `Confirmer` trait. The structured-result + confirm
    abstraction boundary.
  - `fmt.rs` — pure formatters (`format_size`/`format_date`/`format_validation`/
    `join_*`). No I/O, no color. Usable by both core and render.
- `src/operations/` — one child per CLI verb, each `run() -> Report`:
  - `database.rs query.rs remove.rs sync.rs deptest.rs upgrade.rs files.rs`
    `version.rs`
  - `images/` — `mod.rs` (verb dispatch -> `ImagesReport`), `client.rs` (D-Bus
    proxies), `describe.rs` (payload PARSING into typed `Describe`/`Feature`),
    `format.rs` (field EXTRACTION into `Vec<InfoField>`; layout/coloring is NOT
    here), `job.rs` (job wait loop, emits `ProgressEvent::Job*` into a sink),
    `relay.rs` (A/B relay, returns a structured `RelayReport`; its own fail-point
    `eprintln!` warnings stay inline).
- `src/config.rs` — libalpm `Context` construction. Takes neutral inputs
  (`params::ContextParams`, built by the bin from the clap `Cli`); it has NO
  `clap` coupling. `Palette` is NOT on `Context` (owned by `render/`).
- `src/error.rs` — `MizError` + `exit_code()` mapping.
- `src/params.rs` — neutral per-verb parameter structs (plain data, no clap).
  Operations take these; the bin converts `cli::args::<verb>::Args -> params`
  via `From` impls in `main.rs`.

## What stays in the `miz` binary (presentation front-end)

Everything that does color, prompting, progress rendering, stdout, or CLI
parsing — the ONLY place `println!`/indicatif/console/clap live. (`eprintln!` is
NOT bin-exclusive: miz-core keeps plain, uncolored `eprintln!` failure
diagnostics at hard-error points — see the carve-out below.)

- `src/main.rs` — dispatch, `From<cli::args::<verb>::Args>` -> `miz_core::params`
  conversions, wires `TtyConfirmer` + `IndicatifSink`, maps `ExitCode`.
- `src/cli/` — clap `Cli` + `args` (alpm-free; `build.rs` compiles it standalone
  for manpage generation).
- `src/render/` — the single presentation layer:
  - `palette.rs` — `Palette` (owned here, NOT on `Context`).
  - `fmt.rs` — re-exports `miz_core::common::fmt` for render call sites.
  - `confirm.rs` — `TtyConfirmer` (`Confirmer` impl: summary + `[Y/n]`).
  - `progress_indicatif.rs` — `IndicatifSink` (`ProgressSink` impl: the bars).
  - `completions.rs` — writes shell completions to stdout; needs the full clap
    `Cli::command()`, so it lives in the BIN, not core.
  - `database.rs query.rs remove.rs sync.rs deptest.rs upgrade.rs files.rs`
    `version.rs images.rs` — `render(&Report[, &Palette])`.

## The abstraction boundary a daemon reimplements — the `mizd` attach point

A future `mizd` daemon depends on `miz-core`, calls the relevant
`operations::<verb>::run(...)`, and supplies its OWN implementations of the two
seam traits, rendering the SAME structured results over D-Bus instead of to a
TTY. The exact signature is per-verb, not uniform: read-only verbs take just
`(params, &ctx)` (e.g. `database::run`), `version::run` takes nothing, and the
transactional verbs (`sync`/`remove`/`upgrade`) + `images` take a
`&mut dyn Confirmer` and a `&SharedSink`. A daemon supplies a policy `Confirmer`
(e.g. assume-yes / confirm-over-D-Bus) and a D-Bus-signal `ProgressSink` in
place of the bin's `TtyConfirmer` / `IndicatifSink`:

- `ProgressSink` (`common/progress.rs`) — the daemon sink re-emits D-Bus
  `PercentProgress` / `Finished` signals instead of drawing indicatif bars.
- `Confirmer` (`common/report.rs`) — the daemon supplies a policy confirmer
  (`AssumeYes`, or return-plan-over-D-Bus-then-commit) instead of a `[Y/n]`
  prompt. Core builds a `TransactionPlan`, calls `confirmer.confirm(&plan)`, and
  commits only on `true`.
- The `*Report` types (`common/report.rs`) — the daemon serializes these as
  D-Bus return values; the TTY front-end renders them as text. One shape, two
  presentations.

The daemon builds `params::<verb>` structs directly (no clap needed) and its own
`params::ContextParams` for `config::build_with_dbext`.

## The invariant that keeps core presentation-free

> No `println!`/`print!`/indicatif/`console`/`clap` in `miz-core`, and
> `miz-core` never depends on the `miz` bin.

Carve-out: plain, uncolored `eprintln!` FAILURE-POINT diagnostics (no palette,
no render dependency). Sites: `common/transaction.rs` (release-cleanup warning
from `Drop`), `common/imagedb.rs`, `operations/images/relay.rs` (teardown/prune
warnings), and the target-not-found / add-failure / load-failure diagnostics in
`operations/sync.rs`, `operations/remove.rs`, `operations/upgrade.rs`. These
either stay as plain stderr (acceptable for a library's hard-failure path) or
get lifted behind a logging facade — a follow-up, not a blocker.

## build.rs

`crates/miz/build.rs` STAYS in the bin and generates manpages by compiling
`src/cli/mod.rs` standalone (`#[path = "src/cli/mod.rs"] mod cli;`). The `cli`
module is alpm-free (Args structs are plain clap data; the alpm-using
`operations::*::run()` impls live in `miz-core` and are not pulled in), so
manpage generation links no libalpm.
