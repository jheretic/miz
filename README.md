# miz

Package and update manager for the [Archetype](https://archetype.example) Linux distribution.

A clone of Arch Linux's `pacman` built on the `alpm` crate. Same operations, same flag shape, same exit codes. Implemented in Rust with `clap`-derive CLI parsing.

## Operations

| Flag | Long | Purpose |
|---|---|---|
| `-D` | `--database` | Operate on the package database (`--check`, `--asdeps`, `--asexplicit`) |
| `-Q` | `--query` | Query the local package database (16 sub-flags) |
| `-R` | `--remove` | Remove packages from the system |
| `-S` | `--sync` | Synchronize / install packages from configured repositories |
| `-T` | `--deptest` | Check whether dependencies are satisfied |
| `-U` | `--upgrade` | Install local `.pkg.tar.zst` files |
| `-F` | `--files` | Query the files database (`-Fy` refresh, `-Fl`, `-Fs`, `-Fx`) |
| `-V` | `--version` | Print version banner |
| `-I` | `--images` | **miz extension** — Archetype system images via systemd-sysupdated (`-Il`/`-Ii`/`-Iy`/`-Iu`/`-Ic`/`-Ip`/`-Ig`/`-If`/`--reboot`/`--appstream`) |

Plus a hidden `completions <shell>` subcommand for `clap_complete`-generated shell completions.

## Examples

```sh
miz -Syu                              # refresh and full system upgrade
miz -S firefox                        # install firefox
miz -Ss '^python-'                    # regex search syncdbs
miz -Q -i bash                        # show full info for installed bash
miz -Ql linux | head                  # list files owned by linux
miz -F /usr/bin/python                # which package owns this file
miz -R --recursive vim                # remove vim and unneeded deps
miz -U ./my-pkg-1.0-1-any.pkg.tar.zst # install a local file
miz -Sc                               # clean uninstalled cached packages
miz -Il host                          # list available image versions
miz -Iu host                          # update host image to newest
miz -If host                          # list optional image features
miz -If host/myfeature                # describe one feature
miz -If --enable myfeature            # enable an optional feature
```

## Building

Requires `pacman` ≥ 7.x (libalpm SONAME ≥ 16) and `pkg-config` at build time:

```sh
sudo pacman -S --needed rust pacman
cargo build --release
```

For non-Arch dev hosts, see `tests/fixtures/README.md` for the libalpm shim workaround used by CI.

## Status

v0.1 — all nine pacman operations implemented. See [`PLAN.md`](PLAN.md) for the implementation roadmap and [`docs/alpm-api-notes.md`](docs/alpm-api-notes.md) for the alpm 5.0.2 surface reference accumulated during implementation.

Integration tests against a real `pacman.conf` are `#[ignore]`-gated and require an Arch host or container; run with `cargo test -- --ignored`. The fixture under `tests/fixtures/root/` lets transactional tests run against a tempdir-rooted libalpm so they never touch `/var/lib/pacman/`.

## License

GPL-2.0-or-later. See [`LICENSE`](LICENSE).

`miz` borrows pacman's CLI shape and operation semantics; pacman itself is © Judd Vinet, Aaron Griffin, and the Arch Linux contributors, also under GPL-2.0-or-later.
