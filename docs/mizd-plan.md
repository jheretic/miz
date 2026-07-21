# Plan: `mizd` — a D-Bus daemon for layered-package operations

Status: IMPLEMENTED 2026-07 (v1, no Cancel). Phases 1-5 committed + reviewed;
alpm/polkit/system-bus paths are VM-validation-only (see the flags below).
Follows the structured-results refactor + the `miz-core` crate split. `mizd` is
the daemon the whole refactor was building toward.

## v1 status

- Phase 1 (miz-core AssumeYes + interrupt helper), Phase 2 (interface
  skeleton), Phase 3 (worker + read-only/refresh), Phase 4 (mutating methods +
  polkit + data files), Phase 5 (packaging) — all done.
- **Cancel is NOT in v1.** `Job.Cancel` returns `NotSupported`. Two adversarial
  reviews found that a cross-thread `alpm_trans_interrupt` races libalpm's
  non-atomic transaction state (a C data race in a root daemon); the
  signal-handler precedent does NOT apply (a signal suspends its thread, a
  D-Bus thread runs concurrently). Safe cancellation needs the interrupt to run
  ON the worker thread (a flag/self-pipe polled between libalpm phases — cancels
  at phase boundaries, which is what pacman does) or a subprocess-per-
  transaction. Planned follow-up; NOT on the GS-plugin critical path
  (Preview→Install works without cancel).
- **VM-validation-only** (cannot run against the fake-alpm stub / no polkit /
  no privileged bus on the dev host): every worker transaction, the polkit
  CheckAuthorization round-trip, D-Bus activation + the installed data files,
  the Job progress/JobRemoved lifecycle on a live bus, and CLI-vs-daemon
  db.lck contention.

The original design below is retained for reference.

## Purpose + scope

Archetype has **two** update surfaces:

1. **OS image A/B updates** via `systemd-sysupdated` (`org.freedesktop.sysupdate1`).
   `miz -Iu` is already a *client*. GNOME Software has a MERGED native plugin
   (`gs-plugin-systemd-sysupdate`, GNOME GitLab MR !2004) that talks the SAME
   service. **This surface needs no mizd** — GS and `miz -Iu` are two clients of
   one daemon that already exists.
2. **Layered packages** (the libalpm `-S`/`-Syu`/`-R` side). This is the gap.
   There is no daemon; the `miz` CLI opens libalpm directly, takes the pacman
   db lock per-invocation, commits, exits.

**`mizd` exposes the LAYERED-PACKAGE operations over D-Bus**, so a graphical
tool (a future GNOME Software plugin, à la rpm-ostree) can query + trigger
layered installs/updates, and so transactions are **serialized** by one daemon
(the lock contention a shell-out CLI plugin can't solve).

**Out of scope (do NOT build here):** the GNOME Software plugin / PackageKit
backend (mizd is the daemon it would talk to — attach point noted only);
wrapping sysupdated (the OS-image path stays the existing client); AppStream
`type=systemd` bundle metadata (an in-flight upstream spec).

Precedent (verified from GNOME Software source earlier): rpm-ostree's GS plugin
declares `GS_PLUGIN_RULE_CONFLICTS, "packagekit"` and talks its daemon's own
D-Bus API — i.e. "once your package manager is a daemon with a D-Bus API, a
bespoke GS plugin over that D-Bus is idiomatic, not PackageKit." mizd follows
that model.

## What mizd plugs into (already built by the refactor)

- `miz_core::operations::<verb>::run(params, [&ctx], [&mut dyn Confirmer], [&SharedSink]) -> Result<Report>`
  — one entry per verb; clap-free `params`; structured `*Report`.
- `miz_core::common::progress`: `ProgressEvent` (Status / Op{OpKind,pkg,percent} /
  Download / DownloadDone / Job*), `ProgressSink` trait, `SharedSink =
  Rc<RefCell<dyn ProgressSink>>`, `register()` (alpm callback → ProgressEvent).
- `miz_core::common::report`: `Confirmer` trait (`confirm(&mut self,
  &TransactionPlan) -> bool`), `TransactionPlan{targets, kind: TransactionKind,
  prompt}`, per-verb `*Report` with `outcome() -> Result<()>`.
- `miz_core::common::transaction`: `TransGuard` (init/commit/release), the global
  `static ALPM_HANDLE: AtomicPtr` (documented: the ptr is `Send`;
  `alpm_trans_interrupt` is the cross-thread interrupt primitive the SIGINT
  handler already uses), `build_flags` → `TransFlag` (incl. `DOWNLOAD_ONLY`).
- `miz_core::config::build_with_dbext(&ContextParams, dbext) -> (Context, bool)`.

The `miz` CLI is one front-end over this (TtyConfirmer + IndicatifSink). `mizd`
is a second front-end (D-Bus signals + a policy confirmer).

## D-Bus interface — `org.archetype.miz1`

Mirror the sysupdated shape miz already consumes (Manager + per-Job objects +
JobRemoved signal), so a GS plugin author sees a familiar idiom and mizd's own
progress plumbing matches what its `images` client already speaks.

### Manager object `/org/archetype/miz1`, interface `org.archetype.miz1.Manager`

Methods:
- `ListUpgradable() -> a(sss)` — (name, installed_version, new_version) for
  layered packages with a sync-newer version. Backed by `query` upgrade logic
  (read-only, no lock).
- `ListInstalled() -> a(ss)` — (name, version) union of image-db + localdb
  (the `query` union already implemented). Read-only.
- `PreviewInstall(in as packages) -> (a(ss) targets, s summary)` — resolve the
  transaction WITHOUT committing, using the lock-free preview path (the
  `--print`/`NO_LOCK`-style flags in `sync.rs`); returns the `TransactionPlan`
  target list. The inspect-before-apply step (the sysupdate #34814 / GS
  download-then-apply pattern).
- `Install(in as packages, in u flags) -> (u job_id, o job_path)` — enqueue a
  layered install. Async: returns a Job handle immediately.
- `Remove(in as packages, in u flags) -> (u job_id, o job_path)`.
- `Upgrade(in u flags) -> (u job_id, o job_path)` — `-Syu` of layered packages.
- `RefreshDatabases() -> (u job_id, o job_path)` — `-Sy` (own job: network).
- `ListJobs() -> a(uo)` — active (id, path).

Signals:
- `JobRemoved(u id, o path, i status)` — mirrors sysupdated's
  `Manager.JobRemoved`; terminal outcome (0 = ok, >0 exit code, <0 -errno).

### Job object `/org/archetype/miz1/job/<id>`, interface `org.archetype.miz1.Job`

Properties: `Id u`, `Kind s` (install/remove/upgrade/refresh), `Progress u`
(0-100). Methods: `Cancel()`. Signals: `Progress(u percent, s message)` (from
`ProgressEvent`), optional `Log(s line)` for status lines. This is the exact
shape miz's `images/job.rs` already consumes from sysupdated — reuse the mental
model.

Rejected alternative: a flat Manager-only interface with progress as Manager
signals keyed by job id. Rejected — per-Job objects give clients a natural
handle to poll `Progress` + call `Cancel`, and match sysupdated/rpm-ostree so a
GS plugin is boilerplate-similar.

## Concurrency model — the load-bearing design

libalpm is **synchronous**, holds a **process-global db lock**, and uses a
**global static handle** (`ALPM_HANDLE`). Therefore: **one worker thread owns
all libalpm access; a serialized job queue feeds it; the async zbus server only
accepts requests and dispatches.** This mirrors the rpm-ostree GS plugin, which
delegates every op to one worker thread because libostree is synchronous.

```
 [async zbus server task]          [single worker thread]
   Install() ──enqueue Job──▶  job queue (mpsc) ──▶  run one at a time:
                                                        build Context,
   emits Job.Progress  ◀──ProgressEvent(mpsc)──         operations::run(params,
   emits JobRemoved    ◀──terminal Result──             &ctx, &mut AssumeYes,
                                                         &sink)
```

- **The worker owns the `Rc<RefCell<dyn ProgressSink>>`** (SharedSink is `!Send`,
  but it never leaves the worker thread — created and dropped there). The
  daemon's sink impl holds an **`mpsc::Sender<ProgressEvent>`** (which IS `Send`);
  the async task holds the `Receiver` and turns each event into a `Job.Progress`
  signal. So the `!Send` type never crosses a thread boundary — the compiler
  enforces this; a violation won't build. (This dissolves the "!Send sink"
  risk.)
- **Serialization** = the job queue is processed one at a time by the single
  worker; two transactions can never run concurrently, so the global
  `ALPM_HANDLE` static is only ever set by one in-flight transaction. Refresh
  (`-Sy`, network) and read-only queries could run on the async side without the
  worker, BUT anything touching libalpm goes through the worker to keep the
  single-handle invariant absolute.
- **Cancel** = `Job.Cancel()` calls a small new `pub` helper in
  `common::transaction` that reads `ALPM_HANDLE` and calls
  `alpm_trans_interrupt` (the same primitive the SIGINT handler uses,
  documented cross-thread-safe). One helper, not new machinery.
- **libalpm blocking the async runtime**: it doesn't — libalpm only runs ON the
  worker thread (`std::thread`), never on an async executor thread. The zbus
  task stays responsive during a long transaction.

## Confirmer in a daemon

No TTY. Decision: **`AssumeYes`** (a new zero-state `Confirmer` impl) for the
commit path, PAIRED with `PreviewInstall` for inspect-before-apply. Rationale:
the GS model (sysupdate #34814) is "download/preview, client shows the user,
client then triggers apply" — the CLIENT owns confirmation, not the daemon.
`PreviewInstall` returns the plan; the client (GS) shows it; `Install` commits
with `AssumeYes`. This maps cleanly to what a GS plugin expects.

`AssumeYes` belongs in `miz_core::common::report` (CORE-SEAM.md already names it
as the intended daemon confirmer) — NOT duplicated in mizd. It's ~5 lines
(`fn confirm(&mut self, _: &TransactionPlan) -> bool { true }`).

Rejected: a `confirm-over-D-Bus` round-trip (daemon calls back to the client
mid-transaction). Rejected — it holds the db lock open awaiting a human, and
complicates the interface; preview-then-commit achieves the same UX without
holding the lock.

## polkit authorization

mizd runs as root (D-Bus system service); clients are unprivileged. Mirror
sysupdated's action tiers:

- `org.archetype.miz1.refresh` — RefreshDatabases / ListUpgradable /
  ListInstalled / PreviewInstall → `allow_active = yes` (no auth; read + db
  refresh, matching sysupdate's `.check`).
- `org.archetype.miz1.install` — Install / Remove / Upgrade → `auth_admin_keep`
  (privileged; matches sysupdate's `.update`-tier being auth-gated for a
  mutation on a system).

A `org.archetype.miz1.policy` file ships the actions; each mutating method calls
`polkit` (via zbus to `org.freedesktop.PolicyKit1`) with the caller's bus name,
mirroring how the ecosystem does it. (miz's sysupdated client already relies on
sysupdated's own polkit gating; mizd is the enforcing side here.)

## Crate + files layout

New `crates/mizd` (bin) in the workspace:
```
crates/mizd/
  Cargo.toml            # deps: miz-core (path), zbus (ws, needs the SERVER/
                        #   ObjectServer API — not just blocking proxy; confirm
                        #   feature set), tokio or async-io executor (match zbus
                        #   features), tracing. NO clap/indicatif/console.
  src/
    main.rs             # request system bus name org.archetype.miz1, serve
    manager.rs          # #[interface] org.archetype.miz1.Manager
    job.rs              # #[interface] org.archetype.miz1.Job + job registry
    worker.rs           # the single libalpm worker thread + job queue
    sink.rs             # ProgressSink impl that sends ProgressEvent over mpsc
    polkit.rs           # polkit check helper
data/ (or under crates/mizd/)
  org.archetype.miz1.conf        # D-Bus policy (bus access)
  org.archetype.miz1.service     # D-Bus activation -> systemd
  mizd.service                   # systemd unit (D-Bus activated, Type=dbus)
  org.archetype.miz1.policy      # polkit actions
```
Small additions to `miz-core`: `AssumeYes` confirmer (report.rs); a `pub`
cancel helper (transaction.rs); possibly a `pub` on the preview/`NO_LOCK` flag
path so mizd can build a preview transaction. Keep these minimal + tested.

Packaging (archetype-build / archetype-packages): mizd binary + the 4 data
files into the image; the service D-Bus-activated so it's not always running.
An `archetype-mizd` package or fold into the `miz` package — decide at packaging
time.

## Phases (each keeps the workspace green)

CI-parity gate every phase (whole-workspace, NOT -p): `cargo build`,
`cargo clippy --tests -- -D warnings`, `cargo fmt --check`, `cargo test`, with
the fake-alpm stub env (/tmp/fake-alpm, notes/miz-build-env.md — mizd links
libalpm transitively via miz-core). mizd's alpm-touching paths CANNOT be
functionally tested on the dev host (no real libalpm, no privileged system bus)
— those are VM-validation items, like the rest of this project's runtime paths.

- **Phase 1 — miz-core additions (small, fully unit-testable).** Add `AssumeYes`
  (report.rs) + tests; add the `pub` cancel helper (transaction.rs) wrapping
  `alpm_trans_interrupt` on `ALPM_HANDLE`; expose whatever preview-flag path
  mizd needs. No daemon yet. Green trivially.
- **Phase 2 — mizd skeleton + interface, NO libalpm.** Create `crates/mizd`,
  the `#[interface]` Manager + Job with method stubs that return canned/empty
  data, the mpsc ProgressEvent → Job.Progress plumbing, request the bus name.
  Unit-test the pure pieces: ProgressEvent→signal mapping, job-id allocation,
  params construction from D-Bus args. Builds + serves (can introspect on a
  session bus in the VM), touches no alpm.
- **Phase 3 — worker thread + wire the read-only + refresh methods.** The single
  worker thread, job queue, Context construction; wire ListInstalled /
  ListUpgradable / PreviewInstall / RefreshDatabases to miz-core (these are the
  lowest-risk libalpm paths — read + db update). Progress plumbing exercised by
  RefreshDatabases.
- **Phase 4 — wire the mutating methods + polkit.** Install / Remove / Upgrade
  through the worker with `AssumeYes`; Job.Cancel; polkit gating + the .policy
  file; the systemd unit + D-Bus service + bus policy files. This is the
  privileged, VM-only-testable core.
- **Phase 5 — packaging + docs.** Package mizd + data files into the image
  (archetype-build/archetype-packages); document the interface; update
  CORE-SEAM.md (mizd realized). Note the GS-plugin attach point.

Each phase paired with a reviewer running the CI-parity gates + checking the
concurrency invariants (single worker owns alpm; !Send sink never crosses; no
second `ALPM_HANDLE` initializer).

## Risks / pitfalls

- **`operations::*::run` called from a non-main `std::thread`** — libalpm's
  `'static` callbacks + the ctrlc handler were written for a one-shot main-thread
  CLI. This is the WEAKEST ASSUMPTION: that running a transaction on the worker
  thread (not main) is sound. First thing to prove in VM validation. (The ctrlc
  signal handler installed by the CLI is NOT installed by mizd — mizd uses
  Job.Cancel + trans_interrupt instead; confirm no double-install.)
- **The global `ALPM_HANDLE` static** — safe ONLY because the single worker
  serializes transactions. If any future code path initializes a transaction off
  the worker, the invariant breaks. Enforce by construction (all alpm access
  behind the worker queue) + a comment.
- **CLI vs daemon coexistence** — they are SEPARATE processes, each with its own
  process-global `ALPM_HANDLE`; contention is the on-disk `db.lck` file, not the
  static. A `miz -S` racing mizd gets libalpm's normal "unable to lock database"
  — no corruption, no regression. (Reasoning; confirm in VM.)
- **zbus server API + executor** — the codebase uses zbus 5 blocking *proxies*;
  the server (`#[interface]`/ObjectServer) side + an async executor is new. Pin
  the feature set early (Phase 2).
- **polkit on the image** — D-Bus activation + polkit actions must be present on
  the systemd 257 image; VM-validate the deny/allow paths + the friendly error.
- **Not regressing `miz` CLI** — mizd adds to miz-core (AssumeYes, cancel
  helper) but changes no existing path; the CLI keeps working exactly as now.

## The GNOME Software attach point (out of scope, noted)

A future GS plugin (`gs-plugin-miz`, CONFLICTS packagekit, like rpm-ostree's)
would: call `ListUpgradable`/`ListInstalled` to populate updates, `PreviewInstall`
to show the user, `Install`/`Upgrade` to commit, and subscribe to `Job.Progress`
+ `JobRemoved` for progress — plus AppStream metadata to render the entries.
mizd is the daemon it talks to. That plugin is a separate effort.
