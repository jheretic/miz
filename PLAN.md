# miz — Implementation Plan

Clone of Arch's `pacman` written in Rust, built on the `alpm` crate. Target repo: `/home/n0n/src/archetype/miz` (currently empty bar `README.md`, `.gitignore`, `.git`).

---

## 0. Verification log (so you can grill me)

| Fact | How verified |
|---|---|
| `alpm` crate latest = **5.0.2** (2026-01-08), repo `github.com/archlinux/alpm.rs` | crates.io API hit |
| `alpm-sys` 5.0.1, `alpm-utils` 5.0.0 (newer `4.0.3` is a back-patch on the 4.x line — `alpm-utils 5` depends on `alpm ^5.0.0`) | crates.io versions endpoint + dependencies endpoint |
| `alpm-utils 5.0.0` with feature `conf` pulls `pacmanconf ^3.1.0` and re-exports it as module `alpm_utils::config` | crates.io `/dependencies` endpoint + docs.rs surface check |
| `pacmanconf` 3.1.0 (2025-11-02) by Morganamilo is the standalone pacman.conf parser | crates.io API |
| `clap` 4.6.1, `anyhow` 1.0.102, `thiserror` 2.0.18, `env_logger` 0.11.10, `tracing` 0.1.44, `tracing-subscriber` 0.3.23, `config` 0.15.23 | crates.io API |
| `alpm` features: `default, checkver, generate, git, mtree, pkg-config, static, docs-rs` | crates.io version metadata |
| Chambana attempt (`/home/n0n/src/chambana/miz`) used clap 3 with subcommands that have `short_flag('S')` / `long_flag("sync")` — the mechanism that makes `-S` work like a subcommand | read `main.rs` directly |
| pacman top-level ops `-D -Q -R -S -T -U -F -V -h` and their sub-flags | pacman(8); cross-checked against chambana attempt and general knowledge — **guess**: I did not re-read the man page in this planning pass; verify when implementing each module |
| libalpm major version that `alpm 5.x` binds | **verified during Phase 1.0**: `alpm-sys 5.0.1` build.rs requires `libalpm >= 16.0.0` via pkg-config (not 15 as guessed). |

---

## 1. Crate layout

### `Cargo.toml`

```toml
[package]
name        = "miz"
version     = "0.1.0"
edition     = "2021"
description = "Package and update manager for the Archetype Linux distribution"
license     = "GPL-2.0-or-later"   # matches pacman / libalpm

[dependencies]
alpm              = "5"                                          # libalpm binding; needs system libalpm + pacman + pkg-config at build time
alpm-utils        = { version = "5", features = ["conf"] }       # config = pacmanconf re-export, plus depends/local/target helpers; one-line rationale: skip writing our own pacman.conf -> Alpm bridge
clap              = { version = "4", features = ["derive", "wrap_help"] }
anyhow            = "1"                                          # top-level error in main(); good enough for a CLI tool
thiserror         = "2"                                          # named errors per operation where they need to carry exit codes
tracing           = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }

[dev-dependencies]
assert_cmd = "2"   # CLI integration tests
predicates = "3"
```

**Rejected**: `env_logger`. Reason: we want structured fields (pkg name, op) for later debug; `tracing` covers both pretty CLI output and JSON if we add it. One log facade, not two.

**Rejected**: `config` crate. Reason: pacman uses pacman.conf, parsed by `pacmanconf` (re-exported through `alpm-utils::config`). A second config layer would duplicate what alpm-utils already gives us. **Structural-quality note: do not invent a `MizConfig` wrapper — use `pacmanconf::Config` directly.**

**System deps for build/run**:
- `pacman` (provides `libalpm.so` + headers)
- `pkg-config`
- `glibc` headers
- `gpgme` (libalpm links it for sig verification)
- `openssl` or equivalent (transitive)
- `pacman.conf` at `/etc/pacman.conf` for runtime

Build with `--features pkg-config` enabled via default. Static linking (`static` feature on alpm/alpm-sys) is out of scope for v0.1.

### `src/` tree

```
src/
  main.rs                # tracing init, anyhow::Result, dispatch to operations
  cli.rs                 # #[derive(Parser)] Cli, #[derive(Subcommand)] Operation
  config.rs              # thin: open Alpm + pacmanconf::Config from --config / --root / --dbpath
  exit.rs                # ExitCode constants matching pacman (see §4)
  error.rs               # MizError (thiserror) -> exit code
  operations/
    mod.rs               # pub mod database; pub mod query; ...
    database.rs          # -D
    query.rs             # -Q
    remove.rs            # -R
    sync.rs              # -S
    deptest.rs           # -T
    upgrade.rs           # -U
    files.rs             # -F
    images.rs            # -I   (stub, custom miz extension)
    version.rs           # -V   (no alpm needed; print banner)
```

No `lib.rs`. Binary-only crate. If we later add unit tests across modules, promote then.

---

## 2. CLI shape

Pacman's top-level ops are **mutually exclusive single-letter flags**, not subcommands. clap-derive idiom is subcommands. The chambana attempt resolved this with `#[command(short_flag = 'S', long_flag = "sync")]` on subcommand structs — clap-rs supports this on derive-mode subcommands. **That is the approach.**

```rust
// src/cli.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "miz", about = "Archetype package manager", disable_help_subcommand = true)]
// NB: do NOT add `version` here — it autogenerates --version on Cli and conflicts with the `Version` subcommand's long_flag. The -V/--version subcommand replaces it (verified Phase 1.0).
pub struct Cli {
    /// Override pacman.conf path
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<std::path::PathBuf>,

    /// Override root path (default: /)
    #[arg(short = 'r', long, global = true, value_name = "PATH")]
    pub root: Option<std::path::PathBuf>,

    /// Override database path (default: /var/lib/pacman)
    #[arg(short = 'b', long, global = true, value_name = "PATH")]
    pub dbpath: Option<std::path::PathBuf>,

    /// Increase verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub op: Operation,
}

#[derive(Subcommand)]
pub enum Operation {
    #[command(short_flag = 'D', long_flag = "database", about = "Operate on the package database")]
    Database(operations::database::Args),

    #[command(short_flag = 'Q', long_flag = "query", about = "Query the local package database")]
    Query(operations::query::Args),

    #[command(short_flag = 'R', long_flag = "remove", about = "Remove packages")]
    Remove(operations::remove::Args),

    #[command(short_flag = 'S', long_flag = "sync", about = "Synchronize packages")]
    Sync(operations::sync::Args),

    #[command(short_flag = 'T', long_flag = "deptest", about = "Check dependencies")]
    Deptest(operations::deptest::Args),

    #[command(short_flag = 'U', long_flag = "upgrade", about = "Upgrade or add a local package")]
    Upgrade(operations::upgrade::Args),

    #[command(short_flag = 'F', long_flag = "files", about = "Query the files database")]
    Files(operations::files::Args),

    #[command(short_flag = 'V', long_flag = "version", about = "Display version and exit")]
    Version,

    #[command(short_flag = 'I', long_flag = "images", about = "Operate on Archetype system images (miz extension)")]
    Images(operations::images::Args),
}
```

`-h`/`--help` is given by clap automatically.

**Rejected alternative**: model operations as `ArgGroup` of mutually-exclusive top-level bools and route by inspecting matches in `main`. Reason: loses per-op `Args` structs, fights clap-derive, and would re-derive what `short_flag`/`long_flag` already do.

**Rejected alternative**: hand-write a `clap::Command` tree (chambana's approach). Reason: user explicitly asked for derive form.

**Tension worth flagging**: clap-derive's `short_flag` on subcommands means `miz -S foo` parses, but `miz sync foo` *also* parses (the long form name becomes a subcommand keyword). Pacman does not accept the word `sync`. Decision: live with the extra accepted form in v0.1 — it is a strict superset of pacman. Document this in `--help`. Stricter parity is a follow-up.

---

## 3. Per-operation modules

Each `src/operations/<op>.rs` exports:
- `#[derive(clap::Args)] pub struct Args { ... }`
- `pub fn run(args: Args, ctx: &Context) -> crate::error::Result<()>`

`Context` (defined in `config.rs`) holds the constructed `alpm::Alpm` + `pacmanconf::Config` so each operation does not re-parse.

### `-D` database (`src/operations/database.rs`)
```rust
#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]                    pub asdeps: bool,
    #[arg(long)]                    pub asexplicit: bool,
    #[arg(short = 'k', long)]       pub check: bool,
    #[arg(short = 'q', long)]       pub quiet: bool,
    pub packages: Vec<String>,
}
```
alpm: `db.set_pkg_reason(PackageReason::Depend|Explicit)`; `db.check_conflict / check_db` for `-k`. **Verified** these exist on `alpm::Db` / `alpm::Alpm` API surface.

### `-Q` query (`src/operations/query.rs`)
```rust
#[derive(clap::Args)]
pub struct Args {
    #[arg(short = 'c', long)]       pub changelog: bool,
    #[arg(short = 'd', long)]       pub deps: bool,
    #[arg(short = 'e', long)]       pub explicit: bool,
    #[arg(short = 'g', long)]       pub groups: bool,
    #[arg(short = 'i', long, action = clap::ArgAction::Count)] pub info: u8,
    #[arg(short = 'k', long)]       pub check: bool,
    #[arg(short = 'l', long)]       pub list: bool,
    #[arg(short = 'm', long)]       pub foreign: bool,
    #[arg(short = 'n', long)]       pub native: bool,
    #[arg(short = 'o', long, value_name = "FILE")] pub owns: Option<String>,
    #[arg(short = 'p', long, value_name = "FILE")] pub file: Option<std::path::PathBuf>,
    #[arg(short = 'q', long)]       pub quiet: bool,
    #[arg(short = 's', long, value_name = "REGEX")] pub search: Option<String>,
    #[arg(short = 't', long)]       pub unrequired: bool,
    #[arg(short = 'u', long)]       pub upgrades: bool,
    pub packages: Vec<String>,
}
```
alpm: iterate `alpm.localdb().pkgs()`; filter on `reason()`, `requiredby()`, `provides()`; `localdb().pkg(name)`; for `-p` use `alpm.pkg_load()`. **Verified** these are on the `alpm 5` Db/Pkg surface.

### `-R` remove (`src/operations/remove.rs`)
```rust
#[derive(clap::Args)]
pub struct Args {
    #[arg(short = 'c', long)]       pub cascade: bool,
    #[arg(short = 'd', long, action = clap::ArgAction::Count)] pub nodeps: u8,
    #[arg(short = 'n', long)]       pub nosave: bool,
    #[arg(short = 'p', long)]       pub print: bool,
    #[arg(long, value_name = "STR")] pub print_format: Option<String>,
    #[arg(short = 's', long, action = clap::ArgAction::Count)] pub recursive: u8,
    #[arg(short = 'u', long)]       pub unneeded: bool,
    #[arg(long)]                    pub assume_installed: Vec<String>,
    #[arg(long)]                    pub dbonly: bool,
    #[arg(long)]                    pub noconfirm: bool,
    #[arg(long)]                    pub noprogressbar: bool,
    #[arg(long)]                    pub noscriptlet: bool,
    #[arg(required = true)]
    pub packages: Vec<String>,
}
```
alpm: `alpm.trans_init(TransFlag::*)`, `alpm.trans_remove_pkg(pkg)`, `trans_prepare`, `trans_commit`. Signal handlers off for SIGINT mid-trans (follow pacman convention).

### `-S` sync (`src/operations/sync.rs`)
```rust
#[derive(clap::Args)]
pub struct Args {
    #[arg(short = 'c', long, action = clap::ArgAction::Count)] pub clean: u8,
    #[arg(short = 'g', long)]       pub groups: bool,
    #[arg(short = 'i', long, action = clap::ArgAction::Count)] pub info: u8,
    #[arg(short = 'l', long)]       pub list: bool,
    #[arg(short = 'p', long)]       pub print: bool,
    #[arg(long, value_name = "STR")] pub print_format: Option<String>,
    #[arg(short = 'q', long)]       pub quiet: bool,
    #[arg(short = 's', long, value_name = "REGEX")] pub search: Option<String>,
    #[arg(short = 'u', long)]       pub sysupgrade: bool,
    #[arg(short = 'w', long)]       pub downloadonly: bool,
    #[arg(short = 'y', long, action = clap::ArgAction::Count)] pub refresh: u8,
    #[arg(long)]                    pub needed: bool,
    #[arg(long, value_name = "REPO")] pub asdeps: bool,
    #[arg(long)]                    pub asexplicit: bool,
    #[arg(long)]                    pub ignore: Vec<String>,
    #[arg(long)]                    pub ignoregroup: Vec<String>,
    #[arg(long)]                    pub overwrite: Vec<String>,
    #[arg(long)]                    pub noconfirm: bool,
    pub targets: Vec<String>,
}
```
alpm: `alpm.syncdbs()` → `SyncDb::update(force)` for `-y`; `alpm.sync_sysupgrade(downgrade)` for `-u`; `trans_add_pkg` for installs; for `-s` regex-match on `db.pkgs()`; for `-c` blow `cachedirs` per pacman semantics (1 = uninstalled, 2 = all). Use `alpm_utils::depends::satisfier` for "find a provider".

### `-T` deptest (`src/operations/deptest.rs`)
```rust
#[derive(clap::Args)]
pub struct Args {
    pub deps: Vec<String>,  // dependency specs like "foo>=1.0"
}
```
alpm: `alpm.find_satisfier(&dep)` over `localdb().pkgs()`; print missing deps; exit 127 if any missing (pacman semantics).

### `-U` upgrade (`src/operations/upgrade.rs`)
```rust
#[derive(clap::Args)]
pub struct Args {
    #[arg(short = 'p', long)]       pub print: bool,
    #[arg(long, value_name = "STR")] pub print_format: Option<String>,
    #[arg(short = 'd', long, action = clap::ArgAction::Count)] pub nodeps: u8,
    #[arg(long)]                    pub asdeps: bool,
    #[arg(long)]                    pub asexplicit: bool,
    #[arg(long)]                    pub overwrite: Vec<String>,
    #[arg(long)]                    pub needed: bool,
    #[arg(long)]                    pub noconfirm: bool,
    #[arg(required = true)]
    pub files: Vec<std::path::PathBuf>,
}
```
alpm: `alpm.pkg_load(path, full=true, level=Default)`, `trans_add_pkg`, prepare/commit. Same as sync transaction path minus repo download.

### `-F` files (`src/operations/files.rs`)
```rust
#[derive(clap::Args)]
pub struct Args {
    #[arg(short = 'y', long, action = clap::ArgAction::Count)] pub refresh: u8,
    #[arg(short = 'l', long)]       pub list: bool,
    #[arg(short = 's', long, value_name = "REGEX")] pub search: Option<String>,
    #[arg(short = 'x', long)]       pub regex: bool,
    #[arg(short = 'q', long)]       pub quiet: bool,
    #[arg(short = 'o', long)]       pub owns: bool,
    #[arg(long)]                    pub machinereadable: bool,
    pub targets: Vec<String>,
}
```
alpm: requires the files DB. Open Alpm with `register_syncdb(... SigLevel)` then `db.update(force)` against `.files` DBs — **guess**: in `alpm 5` this is `Alpm::set_use_syncfirst` + a separate handle path; verify before phase 4. If the binding does not expose files-DB iteration, fall back to shelling `pacman -Fy` for refresh and parse the local files DB directly. Mark as risk in §6.

### `-V` version (`src/operations/version.rs`)
Plain print of miz version + libalpm version (`alpm::version()`) + the pacman-style ASCII banner. No alpm session needed.

### `-I` images (`src/operations/images.rs`) — **stub**
```rust
#[derive(clap::Args)]
pub struct Args {
    pub targets: Vec<String>,
}

pub fn run(_args: Args, _ctx: &crate::config::Context) -> crate::error::Result<()> {
    eprintln!("miz: -I/--images is not yet implemented");
    Err(crate::error::MizError::NotImplemented.into())
}
```
That is the entire module. Wired into `Operation::Images` so help text and tab-completion list it, nothing more.

---

## 4. Shared infrastructure

### Config / context

`config.rs` owns one function:
```rust
pub struct Context {
    pub alpm: alpm::Alpm,
    pub conf: pacmanconf::Config,
    pub root: std::path::PathBuf,
    pub dbpath: std::path::PathBuf,
}

pub fn build(cli: &Cli) -> Result<Context> { /* ... */ }
```
Implementation: use `alpm_utils::config::Config::from_file(path)` (re-export of `pacmanconf::Config`) → feed `Alpm::new(root, dbpath)` → `register_syncdb` for each `[repo]`. `alpm-utils` has the bridging helper; **verify the exact function name** before phase 2 (`alpm_utils::configure_alpm` or similar — **guess**).

`-V` and `-h` skip context construction (no need to read pacman.conf for `--version`).

### Error type — `thiserror`, not `anyhow`

```rust
// error.rs
#[derive(thiserror::Error, Debug)]
pub enum MizError {
    #[error("alpm: {0}")]            Alpm(#[from] alpm::Error),
    #[error("pacman.conf: {0}")]     Conf(#[from] pacmanconf::Error),
    #[error("io: {0}")]              Io(#[from] std::io::Error),
    #[error("not implemented")]      NotImplemented,
    #[error("dependency check failed")] Deptest,
    #[error("operation conflict: {0}")] BadArgs(String),
    #[error("{0}")]                  Other(String),
}
pub type Result<T> = std::result::Result<T, MizError>;

impl MizError {
    pub fn exit_code(&self) -> i32 { /* see table below */ }
}
```

`main()` returns `std::process::ExitCode` — calls into op `run`, maps `MizError` to exit code. `anyhow` is unused outside test code; we want named exit codes and we want the error type to encode them. Single-line rationale: pacman has structured exit codes, so does miz, so the error must carry that.

**Rejected**: `anyhow` throughout. Reason: would lose exit-code mapping; pacman tooling depends on exit codes.

### Exit codes (mirror pacman)

| code | meaning |
|------|---------|
| 0   | success |
| 1   | generic error / bad usage |
| 2   | alpm error (transaction/database) |
| 127 | `-T` reports unsatisfied deps (pacman returns the count, capped) |

`exit.rs` is just constants; centralised so each module imports them.

### Logging

`tracing-subscriber` with `EnvFilter::from_default_env().add_directive(level)` where level is set by `-v` count. Output writer = `stderr`. No JSON in v0.1. CLI output (the human-facing package lists, progress, info) goes to **stdout via plain println!**, not through tracing — pacman scripts pipe stdout, they will break if we add log prefixes. Keep that boundary clean from day one.

### `main.rs` skeleton (illustrative only)

```rust
fn main() -> std::process::ExitCode {
    let cli = cli::Cli::parse();
    init_logging(cli.verbose);
    let result = dispatch(cli);
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::from(e.exit_code() as u8)
        }
    }
}
```

---

## 5. Phasing

Each phase ends in something runnable, testable, and revertible.

### Phase 1 — Scaffolding + read-only ops
- `Cargo.toml`, full `src/` tree, all op modules with **stubs** returning `MizError::NotImplemented`.
- Implement `-V` (prints version banner).
- Implement `-Q` no-flags (list all installed packages) and `-Q <pkg>` (single-pkg info).
- Implement `-Q -i`, `-Q -l`, `-Q -s <regex>`.
- Integration tests with `assert_cmd`: `miz -V` exits 0, `miz -Q` against a fixture root.
- **Verify**: clap-derive `short_flag`/`long_flag` actually parses `miz -S` style. If it does not, halt and design switch before continuing.
- Undo: delete files; repo is empty again.

### Phase 2 — Read-only sync ops
- `-S -s`, `-S -l`, `-S -g`, `-S -i`, `-S -p` (no transaction; queries syncdbs).
- `-Sy` (refresh dbs; requires write to `/var/lib/pacman/sync`, so test with custom `--dbpath`).
- `-T` deptest (read-only against localdb).
- `-D --check`, `-D --quiet` (read-only).
- `-F -l`, `-F -s`, `-F -o` (read-only against files DB; refresh `-Fy` here too).
- Undo: each is feature-flagged-free; revert by reverting the module.

### Phase 3 — Mutating transactions
- `-D --asdeps / --asexplicit`: smallest write; one localdb mutation, no transaction.
- `-R`: remove transaction.
- `-U`: install-from-file transaction.
- `-S <pkg>`: install from sync repos (`-Sy <pkg>`, `-Su`, `--needed`, `--ignore`, `--overwrite`).
- `-S -c` cache clean.
- Hard requirement: **all of phase 3 must support `--root` so tests do not touch the host system.** Build a small `tests/fixtures/root/` skeleton via fakeroot pattern (see §6).
- Undo: revert per op; localdb writes are irreversible on the test root, but the test root itself is throwaway.

### Phase 4 — Polish
- Sysupgrade UX: progress bars, conflict resolution prompts, download progress (use `alpm` callbacks).
- `noconfirm`, `noprogressbar`, `noscriptlet` plumbed everywhere.
- Shell completion via `clap_complete` (optional dep) generated by `miz completions <shell>`.
- Man page generation via `clap_mangen` (optional).

### Phase 5 — `-I/--images` stub becomes real (out of scope here)
Just leave the stub. Tracked separately.

---

## 6. Open questions / risks

1. ~~**clap derive `short_flag` + `long_flag` on subcommands.**~~ **RESOLVED (Phase 1.0)**: clap-4 derive supports `short_flag`/`long_flag` on subcommand variants. All five spike cases parse: `-S foo`, `--sync foo`, `-V`, `-Sy`, `-Syu`.
2. ~~**Combined short flags (`-Syu`).**~~ **RESOLVED (Phase 1.0)**: clap-4 bundles short flags across the subcommand-short-flag + Args sub-flags. No pre-parser needed. `-Syu` → `Operation::Sync { refresh: 1, sysupgrade: true }` natively.
3. **`alpm` API stability between 5.0.x patch versions.** The crate's docs.rs surface is the contract; pin to `=5.0.2` if patch upgrades start breaking.
4. **libalpm version mismatch at runtime.** alpm.rs 5.x links against the libalpm SONAME present at build time. If the user's system libalpm bumps SONAME, miz will fail to load. Document; no runtime detection in v0.1.
5. **`-F` files-DB plumbing in `alpm` 5.** I did not verify the exact API for files-DB registration in the alpm crate during planning. Phase 2 step 5 must start with 30 min of `docs.rs/alpm/5.0.2` reading before code. If the API is missing, fall back to delegating to `pacman -F` until it lands.
6. **Root-required operations on the dev host.** Never run `miz` as root during dev. All install/remove integration tests must use `--root /tmp/miz-test-root --dbpath ...` against a hand-built skeleton. Document this in `CONTRIBUTING.md` when we add one. **No test must touch `/var/lib/pacman`.**
7. **pacman.conf `Include` directives and `$repo`/`$arch` interpolation.** Handled by `pacmanconf` 3.1 — assumed. If not, miz will silently see fewer repos. Verify by running `miz -V` (which we will extend to print the repo list at `-vv`) on a multi-Include system.
8. **License.** GPL-2.0-or-later in `Cargo.toml`. Add `LICENSE` file in phase 1. If user wants something else, adjust before publishing.
9. **`alpm-utils 5.0.0` is yanked-adjacent** — newest published is `4.0.3` (back-patch on the 4.x line). Watch for a `5.0.1` or for 5.0.0 being yanked. If yanked, drop to `alpm-utils = "4"` (which depends on `alpm ^4`) and downgrade `alpm = "4"` — a coordinated bump. Not a v0.1 blocker; flagged.
10. **Comments policy.** User asked for minimal comments. Plan reflects this: error variants, op `run` functions, and CLI structs carry zero rustdoc beyond what clap's `///` derives need for `--help` text. Internal logic: no comments unless the line is genuinely non-obvious.

---

## 7. Files-changed map (for reviewers)

| File | First touched in phase | Owner module |
|------|----------------------|---------------|
| `Cargo.toml`              | 1 | — |
| `src/main.rs`             | 1 | entry |
| `src/cli.rs`              | 1 | CLI |
| `src/error.rs`            | 1 | error |
| `src/exit.rs`             | 1 | error |
| `src/config.rs`           | 2 | runtime |
| `src/operations/mod.rs`   | 1 | re-exports |
| `src/operations/version.rs` | 1 | -V |
| `src/operations/query.rs` | 1 | -Q |
| `src/operations/sync.rs`  | 2/3 | -S |
| `src/operations/database.rs` | 2/3 | -D |
| `src/operations/files.rs` | 2 | -F |
| `src/operations/deptest.rs` | 2 | -T |
| `src/operations/remove.rs` | 3 | -R |
| `src/operations/upgrade.rs` | 3 | -U |
| `src/operations/images.rs` | 1 | -I (stub) |
| `tests/cli.rs`            | 1 | integration |
| `tests/fixtures/root/`    | 3 | test scaffolding |
