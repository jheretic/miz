# alpm 5.0.2 API notes (cumulative — Phase 1 through Phase 4)

Reference for future workers. Captures every place PLAN.md said "guess:" or a
prereq prompt was wrong about the actual `alpm` / `alpm-utils` / `alpm-sys`
5.x surface. Source: source-spelunking + commit-by-commit findings from
Phase 1.0 through Phase 4.

---

## Versioning & build

| Fact | Value |
|---|---|
| `alpm` crate | `5.0.2` (2026-01-08) |
| `alpm-sys` | `5.0.1` |
| `alpm-utils` | `5.0.0` (newer `4.0.3` is a back-patch on the 4.x line) |
| Minimum libalpm SONAME | `>= 16.0.0` (NOT 15.x, despite the "alpm 5" name) |
| Detected by | `alpm-sys`'s `build.rs` via `pkg-config libalpm >= 16.0.0` |

`alpm 5.x` will not build/link against Arch's libalpm 15.x. The fake-libalpm
shim used in dev (`/tmp/fake-alpm`) sets soname/version to 16.x to satisfy this.

---

## Pkg / Package / LoadedPackage deref chain

```
alpm::Pkg          -- concrete unowned reference (libalpm_pkg_t*)
alpm::Package      -- alias / trait surface used by AlpmList<Package>
alpm::LoadedPackage<'a>  -- what Alpm::pkg_load() returns; owns a handle slot
```

- `alpm.localdb().pkg(name.as_bytes())` returns `Result<&Pkg, alpm::Error>`
  -- takes `&[u8]`, NOT `&str`.
- `alpm.localdb().pkgs()` yields `AlpmList<&Package>`.
- `Alpm::pkg_load(raw_path_bytes, full=true, SigLevel)` returns
  `LoadedPackage<'a>`. Pass directly into `trans_add_pkg` (bound is
  `IntoPkgAdd`, not `Package`).
- `Pkg::name()` returns `&str`. `Pkg::version()` returns `Ver` (call
  `.as_str()` for the string form).
- `Pkg::desc()` returns `Option<&str>`.
- `Pkg::install_date()` returns `Option<i64>` (epoch seconds).
- `Pkg::reason()` returns `PackageReason` (`Explicit | Depend`).

PLAN.md guesses that were wrong:

- `-Q` plan used `pkg.requiredby()`. Real name: `Pkg::required_by()` (snake
  case). Same for `Pkg::optional_for()`.
- `-Q -p` plan: `pkg_load(path)` — real signature is
  `pkg_load(filename: Vec<u8>, full: bool, level: SigLevel)`. Filename is
  bytes, not `&str` / `&Path`.

---

## AlpmList variants

Three iterator surfaces; not interchangeable.

| Type | From | Mutability |
|---|---|---|
| `AlpmList<'_, &T>` | `db.pkgs()`, `pkg.depends()`, `pkg.files().files()` | immut view |
| `AlpmListMut<T>` of `DbMut` | `alpm.syncdbs_mut()` | per-element mut |

- `syncdbs()` immut for reads (`-Ss`, `-Si`).
- `syncdbs_mut()` for `-Sy` / `-Fy` because you call `.update(force)` on
  each `DbMut`. You **cannot** call `.update()` on the immut list.
- `find_satisfier(&[u8])` lives on `AlpmList<&Package>` (immut) and
  returns `Option<&Package>`.
- `pkg.files()` returns a `FileList` (not `AlpmList`); `.files()` on it
  yields entries with `&[u8]` names.

PLAN.md guess that was wrong: `-T` plan said `alpm.find_satisfier(&dep)`.
There is no top-level `find_satisfier` on `Alpm` — call it on the list from
`localdb().pkgs()` or `syncdbs()`.

---

## TransFlag values used in miz

```
TransFlag::NONE                base
TransFlag::DB_ONLY             -R --dbonly, -U --dbonly
TransFlag::NO_SCRIPTLET        -R/--noscriptlet, -S, -U
TransFlag::NO_DEPS             -R -d, -U -d
TransFlag::NO_DEP_VERSION      -R -dd, -U -dd
TransFlag::CASCADE             -R -c
TransFlag::RECURSE             -R -s
TransFlag::RECURSE_ALL         -R -ss
TransFlag::UNNEEDED            -R -u
TransFlag::NO_SAVE             -R -n
TransFlag::DOWNLOAD_ONLY       -S -w
TransFlag::NEEDED              -S --needed, -U --needed
TransFlag::ALL_DEPS            -S --asdeps, -U --asdeps
TransFlag::ALL_EXPLICIT        -S --asexplicit, -U --asexplicit
```

`TransFlag` is a `bitflags!` type; combine with `|`. alpm-rs flattens the
add/remove flag split that pacman keeps internally in C.

---

## IntoPkgAdd / IntoPkgRemove caveats

- `Alpm::trans_add_pkg<P: IntoPkgAdd>(p)` accepts `&Package` (sync-db
  package) AND `LoadedPackage<'a>`. Each call consumes a `LoadedPackage`.
- `Alpm::trans_remove_pkg(pkg: &Package)` takes a reference into a Db-backed
  package. Cannot remove a `LoadedPackage`.
- `trans_add_pkg` returns `Result<(), alpm::AddError>` — `AddError` is a
  struct with an `.error` field of type `alpm::Error`. Map via
  `MizError::Alpm(e.error)`.

Phase 2.1.1 fix (commit `b2234dd`): `-Sp` print mode needed `&Package`
because the value was moved on first call.

---

## CommitError::FileConflict opaque-binding bug (upstream)

`alpm 5.0.2`'s `CommitData::FileConflict(_)` binds an opaque inner with no
public getter for the conflict list. miz cannot enumerate conflicting files
the way pacman does. Workaround in `transaction.rs::report_commit_error`
prints a single "file conflicts detected; aborting" line.

Upstream PR target: `archlinux/alpm.rs` expose iteration. Tracked in
scratchpad. Not blocking v0.1.

---

## pacmanconf shell-out behavior

`alpm_utils::config::Config::with_opts(_, config_path, root)` re-exports
`pacmanconf::Config::with_opts`. It **shells out to `pacman-conf`** to
resolve `Include`, `$repo`, `$arch`. No in-process parser.

Any test that constructs a `Context` requires `pacman-conf` on PATH. PLAN
§6 risk 7 assumed in-process parsing — it isn't, but behavior is correct.

---

## alpm_utils::configure_alpm — the one-stop helper

PLAN §4 marked as "guess — verify". Verified:

```
pub fn configure_alpm(alpm: &mut alpm::Alpm, conf: &pacmanconf::Config)
    -> Result<(), alpm::Error>;
```

Sets cachedirs, hookdirs, logfile, gpgdir, arch, noupgrades, noextracts,
ignorepkgs, ignoregroups; registers each `[repo]` as a syncdb with its
SigLevel and servers; sets default SigLevel, local-file SigLevel, useragent.

Used in `src/config.rs::build`. Do not roll your own.

---

## set_dbext for -F mode

```
ctx.alpm.set_dbext(".files");
```

Must be called before iterating `syncdbs()` in files mode. After this call,
all `db.pkgs()`/`db.update()` operate on `.files` databases. No per-database
extension switch.

`-Fy` flow (see `src/operations/files.rs`):

```
ctx.alpm.set_dbext(".files");
if args.refresh > 0 {
    let force = args.refresh >= 2;
    let _ = ctx.alpm.syncdbs_mut().update(force)?;
}
```

PLAN §3 said: "if the binding does not expose files-DB iteration, fall back
to pacman -F". It DOES, via `set_dbext`. No fallback needed (Phase 2.5).

---

## Progress / event / download callbacks

Three independent setters on `Alpm`:

```
alpm.set_event_cb(state, |event, st| ... );
alpm.set_progress_cb(state, |kind: Progress, pkg: &str, percent: u8,
                              n: usize, current: usize, st| ... );
alpm.set_dl_cb(state, |filename: &str, event: AnyDownloadEvent, st| ... );
```

- `state` is moved into alpm; re-yielded as `&mut S` on every invocation.
- `Event::HookRunStart(h).name()` returns `&str` directly — NOT
  `Option<&str>`. Original Phase 4.1 wrote `if let Some(name) = h.name()`
  and didn't compile.
- `Progress` enum: `AddStart`, `UpgradeStart`, `RemoveStart`,
  `ConflictsStart`, `DiskspaceStart`, `IntegrityStart`, `LoadStart`,
  `KeyringStart`. No "Done" variants — completion = `percent == 100`.
- `DownloadEvent`: `Init | Progress(p) | Retry(_) | Completed(c)`. `p.total`
  and `p.downloaded` are `i64` (can be `-1` for unknown size).

---

## Error data shapes that bit us

```
PrepareError<'a> {
    error: alpm::Error,                  // .error()
    data: Option<PrepareData<'a>>,       // .data()
}

PrepareData::UnsatisfiedDeps(Vec<DependMissing>)
  DependMissing { target() -> &str, depend() -> &Depend, causing_pkg() -> Option<&str> }
PrepareData::ConflictingDeps(Vec<Conflict>)
  Conflict { package1() -> &Package, package2() -> &Package }
PrepareData::PkgInvalidArch(Vec<&Package>)
```

```
CommitError {
    error: alpm::Error,
    data: Option<CommitData>,
}
CommitData::FileConflict(_)       // opaque; see bug section
CommitData::PkgInvalid(Vec<String>)
```

`PrepareError::error()` and `CommitError::error()` return `alpm::Error`
(an enum). Map to `MizError::Alpm(e)` after rendering a message.

---

## Misc

- `alpm.cachedirs()` returns `AlpmList<&str>` — collect to `Vec<PathBuf>`
  yourself; no `.iter_paths()` helper.
- `db.servers()` returns `AlpmList<&str>`.
- `pkg.filename()` returns `Option<&str>` — `None` for local-DB entries.
- `pkg.db()` returns `Option<&Db>` — `None` for `LoadedPackage` before it
  joins a transaction.
- `alpm.add_ignorepkg(&[u8])` — byte-slice, not `&str`.
- `alpm.add_overwrite_file(&[u8])` — byte-slice, single glob; loop
  externally for multiple.
- `alpm.set_use_syncfirst(...)` (guessed in PLAN §3 for `-F`) does NOT
  exist; `set_dbext` is the real mechanism.

---

## Workflow tips

1. `cargo doc --open` against `alpm = "5.0.2"`. docs.rs HTML is the contract.
2. For "a name" parameters, check `&str` vs `&[u8]` first. Most DB/package
   lookups are bytes.
3. For "a list" return types, identify `AlpmList<T>` vs `AlpmListMut<T>` vs
   plain `Vec<T>`. Three disjoint APIs.
4. Wrap transaction work in `transaction::TransGuard` so `trans_release` is
   called on error paths; Drop handles panics.

---

## Out of scope / unverified

- `alpm-utils 4.0.3` back-patch line. miz uses 5.0.0; 4.x untested.
- Static linking (`static` feature). Not in Cargo.toml.
- `alpm::checkver` / `alpm::generate` / `mtree` features. Not enabled.
  Phase 1.3 `-Qk` is existence-check only; full mtree verification = task #21.
