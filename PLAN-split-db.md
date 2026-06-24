# PLAN — split-database + A/B image-update workflow (`miz -S` layering, `miz -I` reinstall)

Status: design, not yet implemented. Separate from `PLAN.md` and `PLAN-images.md`.

## Goal (restated)

Give an installed (persistent-root) Archetype system a two-database model — a
read-only **image db** (`/usr/lib/miz/db`, baked into the immutable /usr) and a
writable **layered db** (`/var/lib/miz`) — so `miz -S` layers packages from the
image's date-matched archlinux archive snapshot without redundantly reinstalling
what /usr already provides, and `miz -I` re-lays those layered packages onto the
next A/B image during an offline image update.

## Context found (verified by reading the tree + alpm-5.0.2 source)

### miz source
- `crates/miz/src/config.rs` — `build_with_dbext(cli, dbext)` constructs a single
  `Alpm` from `conf.options.{root_dir,db_path}` (CLI `--root`/`--dbpath`
  override). `set_dbext` MUST precede `register_repo` (documented there). One
  handle, one localdb, N syncdbs. `apply_config` wires every option then loops
  `conf.repos` -> `register_repo`. This is the single chokepoint for any
  multi-db change.
- `crates/miz-config/src/lib.rs` — `MizConfig { options: Options, repos:
  Vec<Repository> }`. `Options.db_path` defaults to `/var/lib/pacman/`.
  `deny_unknown_fields` is intentionally OFF (forward-compat for new sections),
  and `unknown_fields_are_silently_ignored` test already exercises a future
  `[images]` table — so a new `[archetype]` section is safe to add incrementally.
- `crates/miz/src/operations/sync.rs` — `sync_install` builds a `TransGuard`,
  `add_install_targets` resolves each target via
  `alpm.syncdbs().find_satisfier()` then `trans_add_pkg`, then
  `prepare`/`commit`. No notion of "already provided by another db." This is
  where layered-install target resolution + the assume-installed seeding hooks
  in.
- `crates/miz/src/operations/transaction.rs` — `TransGuard` (RAII trans_init/
  release + signal-safe lock teardown), `prepare`/`commit`, error reporting.
  Reusable as-is for layered installs.
- `crates/miz/src/operations/images/mod.rs` — `-I` is currently a pure
  systemd-sysupdated D-Bus client, **context-less** (`run(args)` with no
  `Alpm`). The new orchestration verb needs a `Context` (or two), so `-I` must
  either gain a context-bearing sub-path or the orchestration lives behind a new
  sub-flag that `main.rs` routes with a handle. See Phase 4.
- `crates/miz/src/main.rs` — `needs_context` excludes `Images(_)`. Adding a
  db-touching image-update verb means relaxing that exclusion for the specific
  sub-flag (or building the contexts inside the verb).
- `crates/miz/src/cli/args.rs` — `images::Args` is clap-only (build.rs imports it
  for `clap_mangen`; no zbus/alpm allowed there). A new `--reinstall-layered`
  style sub-flag goes here as a bare bool.

### Build side (archetype-build/mkosi.postinst)
- `read_dbfiles()` + the `main()` tail already snapshot the build-time pacman
  local db into `/usr/lib/miz/db/{weight}-{id}/` (weight 10 = `archetype` base,
  50 = sysext layers). Each `{weight}-{id}/` dir contains per-package
  `name-version/` subdirs in **pacman local-db on-disk format** (desc/files).
  This is the image db miz must read.
- `repo_date(version)` maps `IMAGE_VERSION` `2026.06.17-2` ->
  `2026/06/17`; `configure_archetype_repo` pins both `/etc/pacman.conf` and
  `/etc/miz.toml`'s `[archetype]` repo `Server` to
  `https://jheretic.github.io/archetype-repo/repo/2026/06/17`. The same date is
  what the layered archive snapshot must use:
  `https://archive.archlinux.org/repos/2026/06/17/`.
- `IMAGE_VERSION` is written to both `/usr/lib/os-release` and `/etc/os-release`.
  miz's existing `booted_image_version()` in images/mod.rs already parses
  `IMAGE_VERSION=` from os-release — reuse it as the date source.
- /usr is verity squashfs with A/B repart slots; made mutable at runtime via
  systemd-sysext `extensions.mutable/usr` overlay (the `systemd-sysext.service`
  `ConditionDirectoryNotEmpty=|/var/lib/extensions.mutable`). Root is btrfs
  (`70-root.conf` `Format=btrfs`).

### alpm-5.0.2 API constraints (the hard part — verified in registry source)
- `Alpm::new(root, db_path)` -> `alpm_initialize`. **Exactly one localdb**, fixed
  by `db_path`; `alpm_get_localdb` has no multi-db variant. There is no
  `register_localdb`.
- `register_syncdb`/`register_syncdb_mut` add *sync* dbs (install candidates).
  Using a syncdb to represent /usr's packages is wrong: alpm would try to
  *install* them, not treat them as already present.
- **`add_assume_installed(&Dep)` / `set_assume_installed(list)`** (handle.rs
  L329/L334) — wraps `alpm_option_{add,set}assumeinstalled`. This is the ONLY
  in-tree mechanism to mark a provision "satisfied but not owned." Pacman's
  `--assume-installed`. A `Dep` is built via `Depend::new("name=version")`
  (deps.rs L121, `alpm_dep_from_string`). Each provision needs its own entry
  (one for `pkgname=pkgver`, one per `provides` token).
- `set_dbext` ordering constraint (config.rs comment): call before registering
  repos or `%FILES%` parsing breaks.
- Limitation of assume_installed: satisfies dependency resolution ONLY. It does
  not make image packages queryable via `localdb().pkgs()`, does not feed
  file-conflict detection, and does not version-track (a `name` entry with no
  version satisfies any version constraint). This is the central risk (see
  Weakest assumption).

## The core technical decision (read this before the phases)

alpm cannot mount two local dbs. Two candidate strategies:

**Strategy A — assume_installed seeding (preferred).** Point alpm's localdb at
the layered db (`db_path = /var/lib/miz`). Before each transaction, walk the
image db dirs on disk (`/usr/lib/miz/db/{weight}-{id}/*/`), parse each package's
name+version+provides, and feed them to `set_assume_installed`. alpm then treats
/usr-provided deps as satisfied and only pulls genuinely-missing deps into the
layered db. Pure in-tree alpm-rs; no libalpm patch.

**Strategy B — merged shadow localdb.** At runtime, build a unified localdb dir
under `/run` that overlays image-db entries + layered-db entries, point alpm
there read-mostly, and write new installs only to the layered partition. Closer
to "real" db semantics (queries, file-conflict detection work), but: (1) requires
reconciling on-disk db formats and keeping the shadow in sync, (2) risks alpm
treating image packages as removable/upgradable from the layered transaction,
(3) much larger surface. Rejected as the primary path; revisit only if
assume_installed proves insufficient for dependency correctness.

The plan below implements **Strategy A**, isolating the image-db parsing behind a
small module so Strategy B (or an upstream libalpm multi-localdb feature) can
replace it without touching sync/transaction code.

---

## Phase 1 — Config schema: express the three sources

**Deliverable:** `miz.toml` can name the image db, the layered db, and the
date-pinned archive snapshot; existing single-`db_path` configs still parse.

- `crates/miz-config/src/lib.rs`: add an optional section, kept additive (no
  `deny_unknown_fields`):

  ```toml
  [archetype]                       # all keys optional; absent => classic pacman behaviour
  image_db = "/usr/lib/miz/db"      # read-only, scanned for assume_installed
  layered_db = "/var/lib/miz"       # alpm localdb (overrides options.db_path when set)
  archive_base = "https://archive.archlinux.org/repos"  # snapshot root
  # archive_date is normally DERIVED from os-release IMAGE_VERSION; override for testing
  archive_date = "2026/06/17"
  ```

  Add `pub struct Archetype { image_db: Option<PathBuf>, layered_db:
  Option<PathBuf>, archive_base: Option<String>, archive_date: Option<String> }`
  and `#[serde(default)] pub archetype: Option<Archetype>` on `MizConfig`. All
  `Option` so a bare config is unchanged.
- Why a new section, not new `[options]` keys: these are miz-specific, not
  pacman.conf directives; keeping `[options]` a faithful pacman mirror preserves
  the miz-convert round-trip contract documented in lib.rs.
- Reconciliation with `db_path`: when `[archetype].layered_db` is set, it wins
  over `options.db_path` for the localdb path. Document that `options.db_path`
  remains the fallback (and the only value classic/non-image miz uses).
- **Verify:** new unit tests in lib.rs — bare config still yields pacman
  defaults and `archetype == None`; a config with `[archetype]` deserializes the
  paths; round-trip serialize/parse preserves the section.
- **Undo:** delete the struct + field; purely additive, no migration.
- Files: `crates/miz-config/src/lib.rs`, `crates/miz/examples/miz.toml` (document
  the new section, commented out).

## Phase 2 — Image-db reader (assume_installed source)

**Deliverable:** a module that scans `/usr/lib/miz/db/{weight}-{id}/*/` and
returns the list of provisions to feed `assume_installed`.

- New `crates/miz/src/operations/imagedb.rs` (sibling of `sync.rs`). Justify new
  module: no existing code parses the on-disk local-db format outside libalpm;
  alpm-rs offers no "read a localdb dir without making it THE localdb" call, so
  this parsing is genuinely new and isolated.
- Function shape (pseudocode):

  ```
  pub fn provisions(image_db_root: &Path) -> Result<Vec<String>>
    // for each weight-id dir, for each pkg dir `name-version/`:
    //   read `desc` file -> %NAME%, %VERSION%, %PROVIDES%
    //   push "name=version"
    //   push each provides token verbatim (already "foo=1.2" or bare "foo")
    // dedupe; return strings (caller turns each into Depend::new)
  ```

  Parsing the `desc` file (simple `%KEY%\nval\n\n` blocks) is enough; we only
  need NAME/VERSION/PROVIDES, not files. Reuse nothing from libalpm here.
- **Alternative considered:** shell out to `pacman -Qp`/`expac` against the image
  db — rejected, adds a runtime dep and process overhead; the desc format is
  trivial and stable.
- **Verify:** unit test against a fixture `image_db` tree (a couple of fake
  `name-version/desc` files) asserting the returned provision strings. Add the
  fixture under `crates/miz/tests/fixtures/`.
- **Undo:** delete the module; nothing else depends on it until Phase 3 wires it.
- Files: `crates/miz/src/operations/imagedb.rs`,
  `crates/miz/src/operations/mod.rs` (register module),
  `crates/miz/tests/fixtures/image_db/...`.
- **Blocks:** Phase 3.

## Phase 3 — Layered install path (`miz -S` on an installed system)

**Deliverable:** `miz -S foo` installs into `/var/lib/miz` from the date-pinned
archive snapshot, treating /usr-provided packages as already satisfied.

- `crates/miz/src/config.rs`:
  - In `build_with_dbext`, when `conf.archetype.layered_db` is set, use it as the
    `dbpath` passed to `Alpm::new` (CLI `--dbpath` still overrides). Root stays
    `options.root_dir` (`/`), because layered package *files* land in the real
    filesystem — under the overlay-mutable /usr / the persistent root (see file
    placement below).
  - After `apply_config` (i.e. after repos + set_dbext), if
    `conf.archetype.image_db` is set, call
    `imagedb::provisions(image_db)` and seed via
    `alpm.set_assume_installed(deps)` where each string is
    `Depend::new(s)`. Must happen before any transaction. Keep `Depend` values
    alive (own a `Vec<Depend>` in `Context` or set them eagerly — verify the
    alpm-rs lifetime: `set_assume_installed` copies into libalpm, so a temporary
    Vec is fine, but confirm against `alpm_option_set_assumeinstalled`
    semantics).
  - Add the date-pinned archive repos. Two sub-options:
    - (preferred) require the shipped `/etc/miz.toml` to already list `core`/
      `extra` repos whose `servers` point at the archive snapshot URL;
      mkosi.postinst (Phase 6) writes them date-pinned, exactly as it already
      does for `[archetype]`. config.rs needs no archive logic then.
    - (fallback) synthesize archive repo servers in config.rs from
      `archive_base` + derived date if the repos are absent. More code, more
      magic; prefer the postinst approach.
- File placement under overlay-mutable /usr: layered packages whose files target
  `/usr/...` write into the `extensions.mutable/usr` overlay upper dir;
  everything else writes to the persistent root. This is transparent to alpm
  (root = `/`, the kernel routes writes through the overlay). **Verify** on a
  live system that `/usr` is writable through the overlay at the time `-S` runs
  (it is, once `systemd-sysext` has merged the mutable overlay). Document that
  `-S` on an image without the mutable overlay active will fail to write /usr
  files — acceptable, that is the immutable contract.
- Dependency resolution: with assume_installed seeded, `add_install_targets` +
  `prepare` will only add genuinely-missing deps to the layered transaction.
  No change to sync.rs logic is required IF the seeding happens in config.rs
  before the transaction; otherwise seed at the top of `sync_install`.
- **Verify:** on a test root, `miz -S <leaf-pkg>` whose deps are all in the image
  db installs only `<leaf-pkg>` into `/var/lib/miz/local`, not its deps. A pkg
  whose dep is NOT in the image db pulls that dep too. Gate the live test
  `#[ignore]` like the existing `MIZ_HAS_ALPM` tests.
- **Undo:** revert config.rs; layered db lives in a separate path so removing it
  is `rm -rf /var/lib/miz`.
- Files: `crates/miz/src/config.rs`, possibly `crates/miz/src/operations/sync.rs`
  (only if seeding can't live in config), `crates/miz/tests/`.
- **Blocked by:** Phase 1, Phase 2.

## Phase 4 — `-I` image-update orchestration (re-lay layered pkgs onto new A/B)

**Deliverable:** a new `-I` sub-verb that, after a new /usr image is staged,
reinstalls the layered packages onto the new image+root snapshot offline, then
flips the A/B + default-snapshot defaults.

This verb orchestrates external tools and ONE miz-internal alpm transaction.
Split responsibilities explicitly:

| Step | Owner | Mechanism |
|---|---|---|
| 1. Acquire/stage new /usr A/B partition | systemd | existing `-Iu` (sysupdate Acquire+Install) — already implemented |
| 2. btrfs snapshot of root | btrfs (miz shells out) | `btrfs subvolume snapshot` of the root subvol |
| 3. Mount new /usr + root snapshot under /run | miz | mount the staged /usr partition + bind/overlay the new root snapshot at e.g. `/run/miz/next` |
| 4. Set up overlay-mutable /usr for the new tree | systemd-sysext semantics, miz prepares dirs | create `extensions.mutable/usr` upper for the new root snapshot |
| 5. Reinstall layered pkgs into /run tree | **miz (alpm)** | a SECOND `Alpm` with `root = /run/miz/next`, `db_path = <new root>/var/lib/miz`, image-db = new /usr's `/usr/lib/miz/db`, archive repos pinned to the NEW image's date |
| 6. A/B switch (make new /usr default) | bootctl/sysupdate | sysupdate already marks the installed version; confirm default-entry selection |
| 7. Set new btrfs snapshot as default | btrfs (miz shells out) | `btrfs subvolume set-default` |

- The miz-internal part (step 5) is a SECOND `Context` whose paths point into
  `/run/miz/next` rather than `/`. This is why `config.rs` should grow a
  `build_for_root(root, dbpath, image_db, archive_date)` helper rather than
  hard-coding `/`. The layered package list to reinstall comes from reading the
  CURRENT layered db (`/var/lib/miz/local`) — same `imagedb::provisions`-style
  desc parser, but returning explicit-install package names (read
  `%REASON%`/explicit set) to re-add as targets.
- The archive date for step 5 is the NEW image's date: read `IMAGE_VERSION` from
  the staged new /usr's `os-release` (`/run/miz/next/usr/lib/os-release`), not
  the running system's. Reuse `repo_date`-equivalent logic in Rust (see Phase 5).
- CLI: add `--reinstall-layered` (or fold into a higher-count `-Iuu`) to
  `images::Args` in `cli/args.rs` (bare bool, clap-only). `main.rs` routes this
  sub-flag through a context-building path (relax the `Images(_)`
  `needs_context` exclusion for just this verb, or build the `/run` context
  inside the verb — prefer the latter to keep `-I`'s read-only verbs
  context-free).
- **Boundary call-out:** miz does NOT reimplement sysupdate/bootctl/btrfs; it
  shells out (or D-Bus) to them and owns ONLY the layered-package alpm
  transaction against the mounted tree. Steps 2,3,7 shell to `btrfs`/`mount`;
  step 1,6 reuse the existing sysupdate D-Bus client; step 5 is native alpm.
- **Verify:** dry-run mode that prints the mount/snapshot/btrfs commands and the
  layered-pkg reinstall target list without executing. Live verification needs a
  real A/B host — gate `#[ignore]`.
- **Undo path (critical, irreversible-step call-out):** btrfs snapshot (step 2)
  is cheap/reversible (delete the snapshot). `set-default` (step 7) is
  reversible by setting the old default back BEFORE reboot. The A/B switch (step
  6) is reversible via the bootloader's other slot. Reinstalling into the /run
  tree (step 5) writes only into the NEW snapshot, so a failure leaves the
  running system untouched — this is the key safety property and must be
  asserted in the design (never mutate the live root or live /usr during `-I`).
- Files: `crates/miz/src/cli/args.rs`, `crates/miz/src/operations/images/mod.rs`
  (+ a new `images/relay.rs` for the orchestration so mod.rs stays the D-Bus
  dispatcher), `crates/miz/src/config.rs` (`build_for_root` helper),
  `crates/miz/src/main.rs`.
- **Blocked by:** Phase 3.

## Phase 5 — Archive-snapshot date derivation

**Deliverable:** miz derives `YYYY/MM/DD` and builds the archive URL.

- Single source of truth: `IMAGE_VERSION` from os-release (running system: `/etc/
  os-release`; staged image: `<root>/usr/lib/os-release`). Reuse the existing
  `booted_image_version()` parser (move it from images/mod.rs to a shared spot,
  e.g. a small `osrelease.rs`, since both `-I` and config now need it — flag the
  duplication: it already lives in images/mod.rs).
- Port `repo_date()` logic (split on `-`, replace `.` with `/`) into Rust as
  `image_date(version: &str) -> String`. Archive URL =
  `{archive_base}/{date}/$repo/os/$arch` (libalpm does `$repo`/`$arch`
  substitution at fetch time, as the existing repos rely on).
- Precedence: explicit `[archetype].archive_date` in miz.toml (testing override)
  > derived from os-release. `[archetype].archive_base` defaults to
  `https://archive.archlinux.org/repos`.
- **Note (duplication):** `repo_date` exists in Python in mkosi.postinst and will
  now exist in Rust in miz. They must agree; consider documenting the format
  contract in both. Do not consolidate across language boundary — just keep a
  comment cross-reference.
- **Verify:** unit test `image_date("2026.06.17-2") == "2026/06/17"`.
- Files: `crates/miz/src/operations/images/mod.rs` (or new `osrelease.rs`),
  `crates/miz/src/config.rs`.
- **Blocked by:** Phase 1 (uses the `[archetype]` config).

## Phase 6 — Build-side wiring (archetype-build)

**Deliverable:** the shipped image ships a `miz.toml` with the split-db paths and
date-pinned archive `core`/`extra` repos, plus the layered-db dir exists.

- `archetype-build/mkosi.postinst` `configure_archetype_repo` (or a new helper):
  append the `[archetype]` section to `/etc/miz.toml` with `image_db`,
  `layered_db`, and rewrite/append `core`/`extra` `[[repos]]` whose `servers`
  point at `https://archive.archlinux.org/repos/{repo_date(version)}/$repo/os/$arch`
  instead of the live mirrorlist. (Currently the shipped miz.toml uses live
  `geo.mirror.pkgbuild.com`/mirrorlist — for an installed system the layered
  source must be the date-pinned archive to preserve ABI consistency.)
- Ensure `/var/lib/miz` exists in the image (tmpfiles or factory) so the first
  `-S` has a localdb dir to initialize.
- **Verify:** inspect a built image's `/etc/miz.toml` — `[archetype]` present,
  archive-pinned core/extra. (Working-tree-only check: the postinst code
  produces these lines.)
- **Undo:** revert the postinst hunk.
- Files: `archetype-build/mkosi.postinst`, possibly a tmpfiles drop-in under
  `archetype-build/mkosi.extra/`.
- **Blocked by:** Phase 1 (schema must accept the section first).

---

## Rejected alternatives

- **Merged shadow localdb (Strategy B):** correct query/file-conflict semantics
  but large surface, risks alpm treating image pkgs as removable, and needs
  on-disk db reconciliation. Kept as fallback if assume_installed proves
  dependency-incorrect.
- **Image db as a syncdb:** wrong semantics — alpm would reinstall those
  packages rather than treat them as present.
- **Patch libalpm for multi-localdb:** out of scope; would fork the C lib and
  break the "dynamically link distro libalpm.so" contract.
- **New `[options]` keys instead of `[archetype]` section:** pollutes the
  pacman.conf-mirror contract that miz-convert depends on.
- **Synthesize archive repos in config.rs:** more runtime magic than letting
  mkosi.postinst date-pin the repos at build time (it already does this for
  `[archetype]`).

## Weakest assumption — VERIFIED (spike, libalpm deps.c)

RESOLVED in favour of Strategy A. Read libalpm's `_alpm_depcmp_provides` +
`dep_vercmp` (pacman master, lib/libalpm/deps.c L428-451, L392-411):

- assume-installed entries ARE matched with full version-comparison semantics:
  `dep_vercmp` honors `=`, `>=`, `<=`, `>`, `<` via `alpm_pkg_vercmp`. So an
  entry `foo=1.5` correctly satisfies a transitive `foo>=1.2` and correctly
  fails `foo>=2.0`. Versioned constraints resolve correctly.

- HARD REQUIREMENT exposed by the spike (this tightens Phase 2): when a
  dependency carries a version constraint (`dep->mod != ANY`), the provision is
  consulted ONLY if `provision->mod == ALPM_DEP_MOD_EQ` — i.e. the
  assume-installed entry MUST be `name=version`. A bare `name` entry satisfies
  unversioned deps only and is SILENTLY IGNORED for any versioned dep.
  Therefore:
    * Phase 2's imagedb reader MUST emit `name=version` for every image package
      (already specified) — a bare name is insufficient.
    * The SAME applies to `provides`: a bare provides token (`cron`) will NOT
      satisfy a versioned `cron>=1.0` dep; only versioned provides
      (`name=version`) satisfy versioned deps. The plan's "push each provides
      token verbatim" is insufficient for the unversioned-provides-vs-versioned-
      dep case. Mitigation: emit versioned provides where the desc gives a
      version; accept that an unversioned image provides consumed by a versioned
      layered dep would force a redundant (but correct) install of that
      provider into the layered db. This is rare and safe (over-install, never
      mis-resolution).

Conclusion: Strategy A is sound; Strategy B (merged shadow localdb) is NOT
needed for dependency correctness. Build Phase 2 to emit `name=version` for both
package names and (where available) provides versions.

## Follow-ups (out of scope)

- Query/`-Q` integration so layered + image packages both show in `miz -Q`
  (assume_installed does not populate localdb queries; image-db packages won't
  appear in `-Q` without extra reader work). CONFIRMED by the Phase 3 review:
  with `layered_db` set, `localdb()` is `/var/lib/miz`, so `-Q`/`-Qi`/`-Ql`/
  `-Qo`/`-Sl` and `-T` only see layered packages — image packages are invisible
  to them. Not a regression (classic miz without `[archetype]` is unchanged),
  but the image-db-backed query/deptest layer is needed before split-db is
  user-complete. `-T` (deptest.rs) specifically should consult the image-db
  provisions, not just `localdb().find_satisfier`.
- File-conflict detection between layered packages and image /usr files
  (assume_installed does not feed conflict checks).
- GC of the layered db / orphan handling across image updates.
- Rollback UX (`miz -I` to revert to the previous A/B slot + snapshot).
- Mock/fixture harness for the `/run` mount + btrfs steps in CI.

## Critical files for implementation

- `/home/n0n/src/archetype/miz/crates/miz/src/config.rs` — single alpm-construction
  chokepoint; layered_db dbpath + assume_installed seeding + `build_for_root`.
- `/home/n0n/src/archetype/miz/crates/miz-config/src/lib.rs` — `[archetype]`
  schema section (image_db/layered_db/archive_base/archive_date).
- `/home/n0n/src/archetype/miz/crates/miz/src/operations/sync.rs` — layered
  install target resolution; fallback seeding point.
- `/home/n0n/src/archetype/miz/crates/miz/src/operations/images/mod.rs` — `-I`
  dispatch; new `relay.rs` orchestration verb + `booted_image_version` reuse.
- `/home/n0n/src/archetype/archetype-build/mkosi.postinst` — ships the split-db
  miz.toml + date-pinned archive repos (mirrors existing `configure_archetype_repo`).
