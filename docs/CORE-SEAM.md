# The future `miz-core` seam

Status: recorded 2026-07-11, after the structured-results refactor (Phases 0-5)
landed. This is a MAP, not a task: no crate split happens here. It records where
the seam will fall when `miz-core` / `mizd` are eventually extracted, and the
invariant that keeps that extraction mechanical.

See `docs/refactor-structured-results.md` for the why. This doc is the
Phase-6 seam audit output.

## What moves into `miz-core` (libalpm-linked library)

These directories/files are pure core logic + shared infrastructure. They link
libalpm, never touch stdin/stdout/stderr, never draw progress bars, never
prompt, and never `use crate::render`:

- `src/common/` — shared infra:
  - `exit.rs` — exit-code constants.
  - `transaction.rs` — `TransGuard`, prepare/commit, the SIGINT/SIGTERM/SIGHUP
    handler. (`eprintln!` failure-point diagnostics remain inline here; they are
    plain, uncolored, and route no palette — see the invariant note below.)
  - `imagedb.rs` — shared image-db reader (three consumers: `config.rs`,
    `query.rs`, `operations/images`).
  - `osrelease.rs` — os-release / archive-date helpers.
  - `progress.rs` — `ProgressEvent` enum + `ProgressSink` trait + `OpKind` +
    `register()` (the alpm-callback → event translation). ABSTRACTION ONLY; no
    indicatif/console.
  - `report.rs` — the per-operation `*Report` types, `TransactionPlan` /
    `TransactionKind`, the `Confirmer` trait. The structured-result + confirm
    abstraction boundary.
  - `fmt.rs` — pure formatters (`format_size`/`format_date`/`format_validation`/
    `join_*`). No I/O, no color. Usable by both core and render.
- `src/operations/` — one child per CLI verb, each `run() -> Report`:
  - `database.rs query.rs remove.rs sync.rs deptest.rs upgrade.rs files.rs`
    `version.rs completions.rs`
  - `images/` — `mod.rs` (verb dispatch → `ImagesReport`), `client.rs` (D-Bus
    proxies), `describe.rs` (payload PARSING into typed `Describe`/`Feature` —
    parsing is core), `format.rs` (field EXTRACTION into `Vec<InfoField>` — the
    layout/coloring is NOT here), `job.rs` (job wait loop, emits
    `ProgressEvent::Job*` into a sink), `relay.rs` (A/B relay, returns a
    structured `RelayReport`; its own fail-point `eprintln!` warnings stay
    inline, same rule as `transaction.rs`).
- `src/config.rs` — libalpm `Context` construction. `Palette` is NOT on
  `Context` (moved to `render/` in Phase 0).
- `src/error.rs` — `MizError` + `exit_code()` mapping.

## What stays in the `miz` binary (presentation front-end)

The bin keeps everything that does terminal I/O, color, prompting, or progress
rendering — the ONLY place `println!`/`eprintln!`/indicatif/console live:

- `src/main.rs` — dispatch, wires `TtyConfirmer` + `IndicatifSink`, maps
  `ExitCode`. The one place that calls `run() -> render::<verb>::render() ->
  report.outcome()`.
- `src/cli/` — clap `Cli` + args.
- `src/render/` — the single presentation layer:
  - `palette.rs` — `Palette` (owned here, NOT on `Context`).
  - `fmt.rs` — re-exports `common::fmt` for render call sites.
  - `confirm.rs` — `TtyConfirmer` (`Confirmer` impl: summary + `[Y/n]`).
  - `progress_indicatif.rs` — `IndicatifSink` (`ProgressSink` impl: the bars).
  - `database.rs query.rs remove.rs sync.rs deptest.rs upgrade.rs files.rs`
    `version.rs images.rs` — `render(&Report[, &Palette])`.

## The abstraction boundary a daemon reimplements

A future `mizd` daemon links `miz-core` and supplies its OWN implementations of
the two seam traits, rendering the SAME structured results over D-Bus instead of
to a TTY:

- `ProgressSink` (`common/progress.rs`) — the daemon sink re-emits D-Bus
  `PercentProgress` / `Finished` signals instead of drawing indicatif bars
  (mirroring miz's own `images/job.rs` JobRemoved + Progress shape).
- `Confirmer` (`common/report.rs`) — the daemon supplies a policy confirmer
  (`AssumeYes`, or return-plan-over-D-Bus-then-commit) instead of a `[Y/n]`
  prompt. Core builds a `TransactionPlan`, calls `confirmer.confirm(&plan)`, and
  commits only on `true`.
- The `*Report` types (`common/report.rs`) — the daemon serializes these as
  D-Bus return values; the TTY front-end renders them as text. One shape, two
  presentations.

## `miz-config` stays separate, below `miz-core`

`miz-config` is a standalone crate (already split): pure config
deserialization, links no libalpm. It sits BELOW `miz-core` in the dependency
graph (core depends on config, not vice versa). `miz-convert` also stays
libalpm-free and outside `miz-core`. Neither moves in the eventual split.

## The invariant that keeps the split mechanical

> No `println!`/`print!`/indicatif/`console` outside `src/render/` + `src/main.rs`,
> and `src/common/` + `src/operations/` never `use crate::render`.

Two carve-outs, both pre-existing and consistent with the transaction verbs:

1. Plain, uncolored `eprintln!` FAILURE-POINT diagnostics (no palette, no render
   dependency). Current sites: `common/transaction.rs` (release-cleanup warning
   from `Drop`), `common/imagedb.rs`, `operations/images/relay.rs` (teardown/prune
   warnings), and the target-not-found / add-failure / load-failure diagnostics in
   `operations/sync.rs`, `operations/remove.rs`, `operations/upgrade.rs`. The
   invariant is therefore precisely: "no presentation I/O in core EXCEPT these
   enumerated plain-stderr failure diagnostics." When `miz-core` is extracted they
   either stay as plain stderr (acceptable for a library's hard-failure path) or
   get lifted behind a logging facade — a follow-up, not a blocker.
2. `main.rs` is presentation (it IS the bin entry point), so its `eprintln!` for
   the top-level error handler and signal-handler warning are expected.

### Known seam nuance: operations depend on `cli::Args`

Each operation `run()` takes its clap-derived `Args` (`crate::cli::args::<verb>`).
Those arg structs are plain data (no presentation), so when `miz-core` is
extracted they move WITH the operations into the core crate; the `cli/` module
that builds the top-level `Cli`/clap wiring stays in the bin and constructs the
per-verb `Args`. So the split is still mechanical, but it moves the arg *type
definitions* into core alongside the operations — it is NOT the case that all of
`cli/` stays in the bin. (Alternative if that coupling is undesirable later:
introduce neutral request DTOs in core and convert clap `Args` -> DTO in main.)

Verify with:

```
grep -rn 'use crate::render\|crate::render::' src/common src/operations   # empty
grep -rn 'println!\|print!(' src/common src/operations                    # eprintln-only
```
