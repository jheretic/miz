# miz build env (Fedora host, no system libalpm)

CI-PARITY LESSON (bit us 2x): `cargo <cmd> -p miz` is NOT what CI runs. CI does
WHOLE-WORKSPACE `cargo build --verbose`, `cargo clippy --tests -- -D warnings`,
`cargo test --verbose`. Two misses that `-p miz` hid: (1) miz-convert (links no
libalpm, outside `-p miz`) failing whole-workspace build after a MizConfig field
was added but its struct literal not updated; (2) clippy warnings that only fire
under `-D warnings`. Before declaring green on miz work, run the whole-workspace
CI-equivalent trio, not just `-p miz`. (miz-convert builds without the stub; the
`miz` bin still needs /tmp/fake-alpm per below.)


miz is a BINARY crate that links libalpm. The host has no usable libalpm, so a
stub provides the symbols at /tmp/fake-alpm.

    export PATH="/home/n0n/.local/share/mise/installs/rust/stable/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
    export PKG_CONFIG_PATH=/tmp/fake-alpm/lib/pkgconfig
    export LD_LIBRARY_PATH=/tmp/fake-alpm/lib
    cargo test -p miz --bins <filter>      # miz is a binary crate

## The stub (/tmp/fake-alpm/stub.c)
Each libalpm symbol miz REFERENCES must be DEFINED in stub.c or the binary won't
link (clippy/test type-check fine; only the final link fails). Most are no-op
`void f(void){abort();}` -- real alpm calls abort, so live tests are #[ignore].
`alpm_version` returns "16.0.0" so the alpm crate's checkver build script passes.

Rebuild after adding a symbol (NOTE: --export-dynamic is REQUIRED on this host;
without it cc -shared puts the symbols only in the regular symtab, not .dynsym,
so lld still reports them undefined even though `nm` shows them):
    cc -shared -fPIC -Wl,--export-dynamic -Wl,-soname,libalpm.so.16 -o lib/libalpm.so.16.0.0 stub.c
    cd lib && ln -sf libalpm.so.16.0.0 libalpm.so.16 && ln -sf libalpm.so.16 libalpm.so

GOTCHA (cost ~4 wasted passes once): the stub MUST start with `#include <stdlib.h>`
before any `abort()` stub. Without it, `cc` FAILS to compile (implicit-declaration
error) so the `.so` is never rewritten -- the iterate-loop then sees the SAME
undefined symbols every pass and never converges. Seed the fresh stub.c with the
include line, not just `alpm_version`.

## Regenerating the stub from scratch (after a tmpfs reset)
The full symbol set is discovered by iterating: build -> collect
`undefined symbol: alpm_*` -> add to stub -> rebuild. CRITICAL: ACCUMULATE the
symbols across passes (union), never replace the list -- resolving one batch
reveals the next, and dropping the earlier batch makes them undefined again
(infinite oscillation at ~20). ~107 symbols total. pkg-config file:
/tmp/fake-alpm/lib/pkgconfig/libalpm.pc (prefix=/tmp/fake-alpm, Version: 16.0.0,
Libs: -L${libdir} -lalpm).

GOTCHA: new miz code that calls a previously-unused alpm fn needs the symbol
added to the stub. e.g. set_log_cb() -> needs alpm_option_set_logcb (+ the two
get_logcb getters); without them: 'rust-lld: error: undefined symbol:
alpm_option_set_logcb'. The /tmp stub is ephemeral (tmpfs) -- recreate it if the
sandbox was reset.
