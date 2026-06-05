# tests/fixtures

Scaffolding for Phase 3+ integration tests. None of Phase 3's mutating
operations are allowed to touch the host's `/var/lib/pacman` — every test
that writes runs against a throwaway tempdir whose layout mirrors a real
pacman root. This directory is the canonical source for that layout.

## Layout

```
tests/fixtures/
  README.md                       <- this file
  root/
    etc/
      pacman.conf                 <- templated; see "Templating" below
    var/
      lib/pacman/
        local/.gitkeep            <- localdb (installed packages)
        sync/.gitkeep             <- sync DBs (intentionally empty)
      cache/pacman/pkg/.gitkeep   <- package cache
```

The `.gitkeep` files exist solely because git does not track empty
directories. The runtime helper does not copy them into the tempdir.

## Templating

`tests/fixtures/root/etc/pacman.conf` contains placeholder tokens:

| Token         | Replaced with                          |
|---------------|----------------------------------------|
| `@ROOTDIR@`   | `<tempdir>/`                           |
| `@DBPATH@`    | `<tempdir>/var/lib/pacman/`            |
| `@CACHEDIR@`  | `<tempdir>/var/cache/pacman/pkg/`      |
| `@LOGFILE@`   | `<tempdir>/var/log/pacman.log`         |
| `@GPGDIR@`    | `<tempdir>/etc/pacman.d/gnupg/`        |

`make_test_root()` (in `tests/common/mod.rs`) performs the substitution
and writes the result into `<tempdir>/etc/pacman.conf`. Tests then run
`miz --config <tempdir>/etc/pacman.conf --root <tempdir> ...`.

## Host requirements

`alpm_utils::config::Config::with_opts` parses pacman.conf by shelling
out to `pacman-conf(8)`. On hosts where that binary is missing (Fedora,
macOS, CI without an Arch base image), every test that calls
`make_test_root()` will fail before reaching libalpm.

For that reason **every test that uses this fixture is `#[ignore]`-gated**
and only runs under `cargo test -- --ignored` on a host where
`pacman-conf` and `libalpm.so.15` are both present. See the existing
`MIZ_HAS_ALPM=1` gate convention in `tests/cli.rs` and friends.

## libalpm localdb on-disk format

`install_fake_pkg()` writes packages into
`<root>/var/lib/pacman/local/<name>-<version>/` without going through a
transaction. The format is what libalpm's `be_local.c::local_db_read`
expects.

### `desc` file

Newline-separated blocks. Each block opens with a `%KEY%` header line,
followed by one or more value lines, terminated by a blank line. Order
of blocks is not significant; libalpm scans for headers.

Required keys (the minimum that lets `db.pkgs()` enumerate the package
and `Pkg::name()`/`version()`/`reason()` return non-empty values):

```
%NAME%
foo

%VERSION%
1.0-1

%DESC%
test package

%ARCH%
any

%BUILDDATE%
1700000000

%INSTALLDATE%
1700000000

%SIZE%
0

%REASON%
0

%VALIDATION%
none
```

`%REASON%` is `0` for explicitly-installed and `1` for installed as a
dependency. `%VALIDATION%` of `none` skips signature/hash checks on
read.

Optional keys recognised by `install_fake_pkg`:

| Key             | Notes                                              |
|-----------------|----------------------------------------------------|
| `%DEPENDS%`     | one dependency spec per value line (e.g. `glibc`)  |
| `%PROVIDES%`    | one provide per line                               |
| `%OPTDEPENDS%`  | `name: reason` per line                            |
| `%CONFLICTS%`   | one per line                                       |
| `%REPLACES%`    | one per line                                       |
| `%URL%`         | single line                                        |
| `%LICENSE%`     | one per line                                       |
| `%GROUPS%`      | one per line                                       |
| `%PACKAGER%`    | single line; defaults to `miz tests <none@none>`   |

### `files` file

```
%FILES%
usr/bin/foo
usr/share/licenses/foo/LICENSE
```

Paths are root-relative, no leading slash. Directory entries end with
`/`. `install_fake_pkg` writes the listed paths into the
`<name>-<version>/files` block and, for each `(path, bytes)` entry,
materialises the actual file under `<root>/<path>` so that
`-Qo` / `-Ql` / `-Qkk` can resolve ownership and check integrity.

### Determinism

`install_fake_pkg` never reads the wall clock. `%BUILDDATE%` and
`%INSTALLDATE%` are hard-coded to `1700000000`. Same input ⇒ identical
on-disk bytes, so tests that snapshot the localdb stay reproducible.

### Gotchas

- libalpm tolerates trailing blank lines but rejects missing terminators
  between blocks. Every block must end with exactly one blank line.
- `%NAME%` and `%VERSION%` should match the directory name
  (`<name>-<version>`); libalpm uses the `desc` values authoritatively
  but tooling that scans by directory (including miz's own `-Qo` path
  resolution) assumes they agree.
- `%ARCH% = any` is the easiest path; tests want to be host-agnostic.
- `%SIZE%` is the installed size in bytes; `0` is accepted.
- **`<dbpath>/local/ALPM_DB_VERSION` must exist** and contain the current
  schema version (currently `9`, per `ALPM_LOCAL_DB_VERSION` in
  `pacman/lib/libalpm/be_local.c`). libalpm self-heals an empty `local/`
  directory but errors with "database is incorrect version" if any
  package directory exists without the marker. `make_test_root` writes
  the marker at fixture-build time so `install_fake_pkg` always works.
- **`%REASON%` is omitted when reason is Explicit (0).** libalpm writes
  the block only for non-default reasons (be_local.c:1038 — `if(info->reason)`
  guards the fprintf). On rewrite (e.g. after `-D --asexplicit`), the
  block is dropped from `desc` entirely. Test parsers that read `desc`
  back must treat absence-of-block as Explicit, not as missing data.
  `install_fake_pkg` writes the block unconditionally for fixture
  authoring convenience; libalpm's rewrite normalises this to the
  canonical format on first transaction touch.
  See `tests/database.rs::read_reason` for the canonical reader.
