# packaging/

Distribution-specific packaging recipes. Each subdirectory holds the files
needed to build a package for one distribution. Sources are not vendored
here; the recipes fetch the upstream tarball from a tagged GitHub release.

```
packaging/
  arch/
    miz/PKGBUILD          -- pacman package for the miz binary
    miz-convert/PKGBUILD  -- pacman package for the miz-convert binary
```

## Arch (`packaging/arch/`)

Two separate packages:

- **`miz`** — the package manager itself. Depends on `pacman` (for
  `libalpm.so` at link/run time) and `gpgme` (libalpm's signature backend).
- **`miz-convert`** — the `pacman.conf` → `miz.toml` migration tool.
  Depends on `pacman` because the `pacmanconf` crate shells out to
  `pacman-conf(8)` at runtime to do the parsing.

Both PKGBUILDs share a `source=()` pointing at the same upstream tarball
(`v$pkgver` GitHub release archive); each only builds and packages its
own crate via `cargo build -p <crate>`.

### Building

Tag a release first:

```
git tag v0.1.0
git push --tags
```

Then from each package directory:

```
cd packaging/arch/miz && makepkg -si
cd packaging/arch/miz-convert && makepkg -si
```

### Status

These PKGBUILDs are not yet submitted to the AUR. The `source=()` URL
assumes a tagged release at `github.com/THRONELESS/miz/archive/refs/tags/`;
the maintainer line and URL placeholder need updating before submission.
`sha256sums=('SKIP')` is fine for local builds but must be replaced
with the real sha256 of the release tarball before AUR upload.
