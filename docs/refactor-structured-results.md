# Refactor: structured results + single presentation layer

Status: planned (2026-07-11). Prerequisite step before a future `miz-core`
library crate and `mizd` daemon. This refactor stays WITHIN the current `miz`
binary crate; it does NOT create `miz-core`/`mizd` (that is a later step) — it
only decouples core logic from presentation and reorganizes modules so the
crate split later is nearly mechanical.

## Goal

Operations return **structured results** and emit **progress through an
abstraction**; ALL rendering — `println!`/`eprintln!`, indicatif progress bars,
interactive `[Y/n]` confirmation, `Palette` colorization, exit-code mapping — is
consolidated into ONE presentation layer in the `miz` binary. A future daemon
front-end then renders the SAME structured results as D-Bus returns + progress
signals instead of TTY output.

Secondary goal: `src/operations/` is **structured by operation** — every direct
child is a file or directory named after a CLI `Operation` verb (database,
query, remove, sync, deptest, upgrade, files, version, images, completions).
Shared/infrastructure code moves to a top-level `src/common/` sibling.

## Naming decisions (user, 2026-07-11)

1. Shared elements live in **`src/common/`**, a top-level sibling of
   `src/operations/` — NOT `src/core/`. Rationale: `core` would conflate with
   the future `miz-core` library crate. `operations/` stays a top-level sibling
   (not nested under `common/`). The later `miz-core` extraction pulls
   `common/ + operations/ + config.rs + error.rs` together; recorded in the
   seam doc rather than forced into one directory now.
2. **`Palette` moves OFF `config::Context`** into the `render/` layer — a
   first-class goal of this refactor, not a deferred maybe. If `Palette` stays
   on the libalpm-linked `Context`, the future `miz-core` inherits a `console`
   dependency, which is exactly the presentation leak this work exists to kill.
   Operation `run()` returns uncolored structured data; `render/` colorizes.

## Target module layout

```
crates/miz/src/
  main.rs                  # bin: dispatch, wire TtyConfirmer + IndicatifSink, map ExitCode
  cli/                     # bin: clap Cli + args  (unchanged)
    mod.rs
    args.rs
  config.rs                # libalpm Context construction (Palette REMOVED from Context)
  error.rs
  common/                  # SHARED infra, libalpm-linked, NO stdout/stdin/indicatif/console
    mod.rs
    exit.rs                # exit-code constants (was src/exit.rs)
    transaction.rs         # TransGuard, prepare/commit, signal handler (confirm/summary REMOVED)
    imagedb.rs             # shared image-db reader (config + query + images consumers)
    osrelease.rs           # shared os-release / archive-date helpers
    progress.rs            # ProgressEvent enum + ProgressSink trait (abstraction only)
    report.rs              # per-operation result types + TransactionPlan + Confirmer trait
  operations/              # STRUCTURED BY OPERATION; each child is a verb, run() -> Report
    mod.rs
    database.rs query.rs remove.rs sync.rs deptest.rs
    upgrade.rs files.rs version.rs completions.rs
    images/
      mod.rs               # verb dispatch -> ImagesReport
      client.rs            # D-Bus proxies (unchanged)
      describe.rs          # payload parsing (stays; NOT presentation)
      relay.rs             # A/B relay (returns structured outcome)
      job.rs               # job wait loop -> emits ProgressEvent::Job into a sink
  render/                  # BIN-ONLY presentation: the ONLY place with I/O + console
    mod.rs
    palette.rs             # Palette (was src/style.rs); owned here, NOT on Context
    fmt.rs                 # format_size/format_date/format_validation/join_* (pure formatters)
    confirm.rs             # TtyConfirmer: summary + [Y/n] prompt
    progress_indicatif.rs  # IndicatifSink: ProgressSink impl (was operations/progress.rs)
    database.rs query.rs remove.rs sync.rs deptest.rs
    upgrade.rs files.rs version.rs images.rs   # render(&Report, &Palette)
```

### Why these homes (taxonomy tension resolved)

- **`imagedb`** has three consumers — `config.rs::seed_assume_installed`
  (`provisions()`), `query.rs` (`all_packages()`/`ImagePackage`), and `images`.
  No single owner; moving it under `images/` would force `query → images::imagedb`
  and `config → images::imagedb` (wrong direction). It is shared infra →
  `common/`.
- **`transaction`** is used by sync/remove/upgrade/images/relay + `main.rs`
  (signal handler). Machinery, not a verb → `common/`.
- **`osrelease`** is used by `images/relay.rs` and `config.rs` → `common/`.
- **`progress`** splits: the ABSTRACTION (`ProgressEvent`/`ProgressSink`) is
  neutral → `common/progress.rs`; the indicatif RENDERER is presentation →
  `render/progress_indicatif.rs` (keeping `indicatif`/`console` out of the
  future `miz-core`).
- **`describe.rs`** (images payload parsing) is core, not presentation → stays
  under `operations/images/`.

### Seam preview (later, out of scope here)

`miz-core` ← `common/` + `operations/` (run-logic) + `config.rs` + `error.rs`
(all libalpm-linked). `miz` bin keeps `render/`, `main.rs`, `cli/`, and the
`TtyConfirmer`/`IndicatifSink` implementations. `miz-config` stays a separate
crate below `miz-core`; `miz-convert` stays libalpm-free. Recorded in
`docs/CORE-SEAM.md` at the end.

## The result / presentation abstraction

**Structured results** — `common/report.rs` holds per-operation report types
capturing exactly what the current print sites emit:

- `QueryReport::{ NameVersions(Vec<PkgLine>), Info(Vec<InfoBlock>), Files(..),
  Search(Vec<SearchHit>), Check(Vec<CheckResult>), Owns(..) }` — localdb `Pkg`
  and image-db `ImagePackage` map into the SAME variants (they already print
  identically), so a daemon renders one shape.
- `SyncReport`, `RemoveReport`, `UpgradeReport` (installed/removed target lists,
  up-to-date flags, print-only lines), `FilesReport`, `DbReport`,
  deptest `Vec<String>`, `ImagesReport`.
- `TransactionPlan { targets: Vec<(String,String)>, kind, prompt }` — the value a
  caller inspects before confirming.

**Progress abstraction** — `common/progress.rs`:

```rust
enum ProgressEvent {
    Status(String),
    Op { kind, pkg, percent },
    Download { file, downloaded, total },
    DownloadDone { file },
    Job { percent },
}
trait ProgressSink { fn handle(&mut self, ev: ProgressEvent); }
```

Core registers the alpm event/progress/dl callbacks (and the images job loop) to
translate native callbacks into `ProgressEvent`s forwarded to a
`&mut dyn ProgressSink`. The bin supplies `render/progress_indicatif.rs::IndicatifSink`
(owns `MultiProgress`; today's bars verbatim). A daemon supplies a sink that
re-emits D-Bus `PercentProgress`/`Finished` — mirroring miz's own
`images/job.rs` (JobRemoved + Progress) and the rpm-ostree GS-plugin Transaction
shape.

**Confirmation inversion** — core cannot prompt. `common/report.rs`:

```rust
trait Confirmer { fn confirm(&mut self, plan: &TransactionPlan) -> bool; }
```

Core builds the `TransactionPlan`, calls `confirmer.confirm(&plan)`, commits only
on `true`. The bin supplies `render/confirm.rs::TtyConfirmer` (renders summary +
reads `[Y/n]`, honors `noconfirm`/TTY/NO_COLOR). A daemon supplies a policy
confirmer (`AssumeYes`, or return-plan-over-D-Bus-then-commit).

## Phases (each keeps the WHOLE WORKSPACE green)

CI-parity gate every phase (NOT `-p miz`): `cargo build --verbose`,
`cargo fmt --check`, `cargo clippy --tests -- -D warnings`, `cargo test --verbose`.
Dev links a stub libalpm (`/tmp/fake-alpm`, rebuild per `notes/miz-build-env.md`);
`miz-convert` builds without it. Verbs are decoupled operation-by-operation; the
transaction+confirm+progress triad is done as one cluster.

- **Phase 0** — Scaffold `common/` + `render/`. Move `style.rs`→`render/palette.rs`,
  `exit.rs`→`common/exit.rs`. Define `ProgressEvent`/`ProgressSink` and the
  `Report`/`Confirmer` skeletons (abstraction only, no wiring). **Move `Palette`
  off `config::Context`** (thread it through the bin instead) — first-class here.
  Pure relocation + empty abstractions. Blocks everything.
- **Phase 1** — Relocate shared infra: `transaction.rs`/`imagedb.rs`/`osrelease.rs`
  → `common/`; indicatif renderer → `render/progress_indicatif.rs`; extract the
  inline `completions` arm into `operations/completions.rs`. `operations/` becomes
  verb-only. Pure relocation.
- **Phase 2** — Split the READ-ONLY verbs (query, files, deptest, database,
  version): `run() -> Report` + `render/<verb>.rs`. Move pure formatters to
  `render/fmt.rs`; repoint `images/format.rs`. Safe first proof.
- **Phase 3** — Invert `confirm` (→ `Confirmer`/`TtyConfirmer`) and `progress`
  (→ `ProgressSink`/`IndicatifSink`), including the images job loop. Establishes
  the seams for the cluster. Parallel to Phase 2.
- **Phase 4** — Split the transaction cluster (sync, remove, upgrade) using the
  Phase 3 seams. Preserve `sync_install` ordering (summary → confirm → register
  progress → commit).
- **Phase 5** — Split images verbs + relay into `run() -> ImagesReport` +
  `render/images.rs`; route `-Iu` confirm/progress through the seams.
  `describe.rs` parsing stays under `images/`. Parallel to Phase 4.
- **Phase 6** — Seam audit (no I/O or `console`/`indicatif` outside
  `render/`+`main.rs`; `common`/`operations` don't import `render`); confirm
  `Palette` fully off `Context`; write `docs/CORE-SEAM.md`.

Each implementation phase is paired with a reviewer pass running the full
CI-parity triad + exact stdout/exit-code preservation check.

## Risks / pitfalls

- **Byte-for-byte output preservation** is the weakest assumption: most mutating
  output paths (transaction summaries, progress, commit errors) are only
  exercised under `#[ignore]` tests needing real libalpm. Non-ignored tests won't
  catch drift there — reviewers verify by reading, and a
  `MIZ_HAS_ALPM=1 cargo test -- --include-ignored` run against the finished
  refactor is required at least once.
- **The transaction+confirm+progress triad** (sync/remove/upgrade share it) must
  be split as one cluster (Phase 4), after its seams exist (Phase 3), never
  half-built across a green checkpoint.
- **Palette-off-Context churn**: `Palette` is currently on `Context` and read by
  print sites deep in operations; removing it touches many call sites. Do it in
  Phase 0 while those sites still print (temporary: bin passes Palette to the old
  print paths), so later phases move rendering out cleanly.
- **Exact CLI behavior**: integration tests assert stdout strings and exit codes;
  preserve them. `error::MizError::exit_code()` mapping stays intact.

## Out of scope

- Creating `miz-core`/`mizd` crates (later; this leaves the seam + doc only).
- The `sync::print_sync_info` vs `query::print_info` `{:<19}: {}` consolidation
  (flag in Phase 4, consolidate later).
- The known presentation UX bugs (progress-bar interleaving, `-Sy` no-update
  "unexpected error"): this refactor is well-positioned to fix them once output
  flows through one sink/palette, but fixing them changes asserted output → track
  separately. (Duplicate-bars + color-visibility were already fixed in ccb1221 /
  19d0f86.)
