# REVIEW — miz -I Phase 2 (read-only verbs) vs PLAN-images.md

Reviewed task #9 (miz -I Phase 2 review). HEAD = `ad1f24b`.
Scope: `operations/images/{mod,describe,format,client}.rs`, `cli/args.rs`
images block, `tests/images.rs`. Adversarial read-only pass; parent already
verified build/clippy/test mechanics.

## Summary

**No blockers, no majors. 6 minors, 2 nits.** All seven plan-mandated checks
PASS on correctness. Findings are consistency/claim-drift issues, not bugs.
The Describe defensive-deserialization design is sound. The one finding worth
acting on before commit is the **stdout/stderr asymmetry between -Iy and -Ip**
(MINOR-1) — two sibling "is there something newer?" verbs route their negative
answer to different streams.

---

## Plan-mandated checks (pass/fail)

| # | Check | Verdict | Evidence |
|---|---|---|---|
| 1 | Describe JSON has permissive fallback | PASS | describe.rs:15-37 all `Option` + `#[serde(default)]` + flatten `extra` |
| 2 | -Il/-Ii output resembles pacman not systemd tables | PASS | format.rs:20-31 mirrors sync_list; format.rs:42 uses `{:<19}: {}` like print_sync_info:211 |
| 3 | stdout=data / stderr=notes discipline | PASS w/ caveat | see MINOR-1 (inconsistent between verbs, but not "systemd table" output) |
| 4 | every verb routes through the availability probe | PASS | mod.rs:117,138,162,182,191 all call `connect()` first |
| 5 | resolve_target unknown-component clean error | PASS | mod.rs:101-110 `find().ok_or_else()`, no panic; matches plan text |
| 6 | --offline sets FLAG_OFFLINE on List AND Describe; -q bare | PASS w/ caveat | mod.rs:128/132 (list+describe both get flags); -q bare on 4/5 verbs, see MINOR-2 |
| 7 | formatters reuse query.rs, no duplication | PASS | format.rs:7 imports `query::{format_date, format_size}`; no copy |

---

## Findings

### [minor] MINOR-1 — -Iy and -Ip route their "nothing newer" note to different streams
- Where: /home/n0n/src/archetype/miz/crates/miz/src/operations/images/mod.rs:175 vs :205
- What: `-Iy` (check-new) prints the negative result `"{name}: no newer
  version available"` to **stderr** (eprintln, line 175). `-Ip` (pending)
  prints its negative result `"{name}: up to date ({current})"` to **stdout**
  (println, line 205). These are parallel verbs answering the same question
  ("is there a newer version?"). The negative answer should land on one stream,
  not split by verb.
- How I know: read both functions. `images_check_new` line 175 = `eprintln!`,
  `images_pending` line 205 = `println!`. Both guarded by `if !args.quiet`.
  miz's stderr-for-status precedent (database.rs:48, sync.rs:494,
  transaction.rs:241) leans toward stderr for notes; the positive data in both
  verbs (lines 177/178, 211/212) is correctly on stdout.
- Suggested fix: pick one — either move line 205 to eprintln, or line 175 to
  println — so the two verbs agree.

### [minor] MINOR-2 — `-Iiq` does not produce "bare" output; worker note overstates
- Where: /home/n0n/src/archetype/miz/crates/miz/src/operations/images/mod.rs:158
- What: The worker note claims "-q gives bare output on all five verbs." It does
  not for `-Ii`. `images_info` only uses quiet to gate verbose
  (`let verbose = args.info >= 2 && !args.quiet;`) and never passes quiet into
  `format::info_block`, so `-Iiq` still prints the full `{:<19}: {}` labelled
  block. -Il (format.rs:18), -Iy (mod.rs:177), -Ig (format.rs:62), -Ip
  (mod.rs:209) all do honor quiet with bare output; -Ii is the outlier.
- How I know: traced quiet through all five dispatch fns. `info_block` signature
  (format.rs:36) takes no quiet param.
- Suggested fix: either accept -Iiq == full block (matches pacman `-Siq`, which
  also prints full info — defensible) and correct the claim, or thread quiet
  into info_block. Behavior is plan-defensible; the *claim* is wrong.

### [minor] MINOR-3 — `-Ii --json` errors on unparseable payload instead of dumping raw
- Where: /home/n0n/src/archetype/miz/crates/miz/src/operations/images/mod.rs:153-160
- What: `images_info` calls `Describe::parse(&json).map_err(...)?` (line 153-154)
  *before* the `if args.json.is_some()` raw-passthrough branch (line 156). So a
  syntactically malformed Describe payload makes `-Ii --json host` fail with
  "could not parse Describe JSON" instead of dumping the raw bytes the user
  asked for. Passthrough's whole point is to bypass parsing.
- How I know: read lines 150-160; the parse-and-`?` happens unconditionally
  ahead of the json branch.
- Suggested fix: move the `if args.json.is_some() { println!("{json}"); return }`
  branch above the `Describe::parse` call. (Note: `--json` is a phase-4 item per
  the plan; wiring it early is fine, but the ordering bug ships with it.)

### [minor] MINOR-4 — Describe `version=""` "describe newest" is an unverified man-page assumption
- Where: /home/n0n/src/archetype/miz/crates/miz/src/operations/images/mod.rs:150-152
- What: When no version is pinned, `-Ii` passes `version.unwrap_or("")` to
  `Target.Describe("", flags)` on the assumption that empty string = "newest".
  The plan's "Verified D-Bus surface" section documents
  `Describe(s version, t flags)` but does NOT state that `""` selects newest.
  This is a behavioral guess that can only be confirmed on a live systemd 257+
  host. If `""` is rejected or means something else, `-Ii host` (no version)
  breaks at runtime.
- How I know: PLAN-images.md "Verified D-Bus surface" lists Describe args but no
  empty-string semantics; code comment at line 150 asserts it without citation.
- Suggested fix: confirm against a live host or the 261 man page before relying
  on it; otherwise fall back to CheckNew/List to obtain an explicit version
  string first.

### [minor] MINOR-5 — get_version() failure silently yields confusing empty-version output
- Where: /home/n0n/src/archetype/miz/crates/miz/src/operations/images/mod.rs:127, 199
- What: `proxy.get_version().unwrap_or_default()` swallows a GetVersion error to
  `""`. In `-Ip` (line 199), if GetVersion fails but CheckNew returns "2.3", the
  branch at line 203 is false (`"" != "2.3"`) so it prints
  `"host: update pending  -> 2.3"` with an empty current version. Same swallow
  in `-Il` line 127 affects the `[installed]` fallback. Degrades without a
  crash, but the message is misleading rather than erroring cleanly.
- How I know: traced `unwrap_or_default()` into the line 203 comparison.
- Suggested fix: distinguish "no version installed" from "GetVersion failed"; or
  render `(none)` instead of an empty string.

### [minor] MINOR-6 — post-probe D-Bus method failures surface as raw `dbus: {e}`, not friendly text
- Where: /home/n0n/src/archetype/miz/crates/miz/src/operations/images/mod.rs:130, 153, 169, 198
- What: Only `connect()`'s `list_targets()` failure (mod.rs:91-94) gets the
  friendly "requires systemd 257+" mapping. Subsequent calls — `proxy.list(flags)?`
  (130), `proxy.describe(...)?` (153), `proxy.check_new()?` (169, 198) — use bare
  `?`, converting `zbus::Error` via `MizError::Dbus(#[from])` (error.rs:24-25) to
  a raw `"dbus: <stack-ish>"` string. The plan (§4) says "never a raw zbus stack
  trace." Post-probe these are unlikely, but a transient/polkit failure on List
  or Describe would print the raw form.
- How I know: error.rs:24 `Dbus(#[from] zbus::Error)`; the `?` on those proxy
  calls hits that conversion, not the Sysupdate mapping.
- Suggested fix: wrap the per-call `?` with a `map_err` to `Sysupdate`, or accept
  it as out-of-scope for read-only verbs (the auth-mapping is a phase-3 item).

### [nit] NIT-1 — list_flags() / offline plumbing has no unit test
- Where: /home/n0n/src/archetype/miz/crates/miz/src/operations/images/mod.rs:112-118
- What: `list_flags` (offline -> FLAG_OFFLINE) and the `info>=2` verbose gate are
  pure and bus-free but untested. Unit tests cover parse (describe.rs:64-110),
  formatting (format.rs:115-200), and split_component (mod.rs:222-237), but not
  flag derivation. Plan §5 calls for unit coverage of "no bus" logic.
- How I know: read all three `#[cfg(test)]` modules; no `list_flags` test.
- Suggested fix: add a one-line test asserting `list_flags` with offline true/false.

### [nit] NIT-2 — comment density above "minimal"
- Where: describe.rs:1-10, mod.rs:62-71/82-87, format.rs:1-4 etc.
- What: The directive asks for minimal comments. Several multi-line block
  comments restate plan rationale (e.g. mod.rs:9-11, 82-87). Most are
  decision-justifying and useful, but the volume is above "minimal." Judgment
  call; not a defect.
- Suggested fix: none required; flagged per the task's comment-count check.

---

## What I checked (and how)

- **Describe defensiveness (check 1):** read describe.rs in full. Every documented
  scalar is `Option<T>` with `#[serde(default)]`; `changelog`/`contents` are raw
  `Value`; `#[serde(flatten)] extra: Map<String,Value>` is the catch-all. `parse`
  returns `Result` with no `.unwrap()`/required field. Confirmed via the 4 unit
  tests (full / missing-keys / unknown-keys / empty-object) that a
  syntactically-valid-but-unexpected payload cannot fail deserialization. The
  flatten-into-Map pattern is the standard serde_json catch-all and is safe for a
  self-describing format. `extra_i64` uses `as_i64()` which tolerates JSON
  number-as-float. **Genuinely defensive, not just claimed.**
- **Output shape (check 2):** diffed format.rs:31 `"{component} {version}{suffix}"`
  against sync.rs:127 `"{} {} {}{}"` (sync_list) — same shape, plus an extra
  `[newest]` marker (an additive extension, not table output). Diffed
  format.rs:42 `"{:<19}: {}\n"` against sync.rs:211 `"{:<19}: {}"` — identical
  width-19 label idiom. info_block trailing-newline + images_info's `println!()`
  (mod.rs:159) reproduces print_sync_info's blank-line-after-record. No systemd
  tabular output anywhere.
- **Probe routing (check 4):** confirmed all five read verbs call `connect()`
  first (mod.rs:117,138,162,182,191) and that the "requires systemd 257+" message
  is now on the `list_targets()` method call (mod.rs:91-94), not on
  `system_connection()` (client.rs:117-120, which has its own bus-connect message).
  This is the corrected location vs phase-1's finding.
- **resolve_target (check 5):** mod.rs:101-110 uses `find(...).ok_or_else(...)`,
  no indexing/unwrap; error string `"no such component: {component}"` matches the
  plan and the integration test assertion (tests/images.rs:91).
- **offline/quiet (check 6):** verified `list_flags` feeds BOTH `proxy.list` and
  `proxy.describe` in images_list (mod.rs:128,132) and `proxy.describe` in
  images_info (mod.rs:152); CheckNew correctly takes no flags (client.rs:48).
  Traced quiet through all five verbs (see MINOR-2).
- **Formatter reuse (check 7):** format.rs:7 imports and uses
  `query::{format_date, format_size}`; the size test (format.rs:194-199) asserts
  "1.00 MiB" proving the real query.rs path. New helpers `yesno`/`value_to_text`
  have no query.rs equivalent — no duplication.
- **Error/exit wiring:** error.rs:22-25 adds Sysupdate/Dbus; exit_code maps both
  to GENERIC (error.rs:49-50). main.rs:49 dispatches `Images(args) => run(args)`
  context-less; main.rs:27-29 keeps Images out of `needs_context`. Error printed
  to stderr as `"error: {e}"` (main.rs:76).
- **Tests:** describe.rs 4 + format.rs 7 + mod.rs 3 unit tests, bus-free.
  tests/images.rs: 1 ungated `--help` parse test, 7 gated
  `#[ignore = "...MIZ_HAS_SYSUPDATE=1"]` covering -Ig/-Il/-Il --offline/-Ii/-Iy/
  -Ip + unknown-component. Gating string matches plan §5.

## What I did NOT check

- **Live D-Bus behavior:** no systemd-sysupdated on this host; the 7 integration
  tests stay ignored. D-Bus signatures verified by reading client.rs against the
  plan's "Verified D-Bus surface" only, not against a running service. The
  `Describe("")` = newest assumption (MINOR-4) is the main unverified item.
- **Compile/clippy/test-green mechanics:** out of scope — parent already verified
  (cargo build clean, clippy --tests -D warnings clean, 17 unit + 1-ungated/
  7-ignored). I did not re-run them.
- **VCS artefacts:** out of review scope (worker did not commit, per contract).
- **systemd 261 man page re-fetch:** did not fetch; relied on the plan's cited
  surface, which phase-1 already cross-checked. MINOR-4 flags the one spot the
  plan's citation does not cover (empty-string Describe semantics).
- **Phase-3 surface:** `client.rs` Acquire/Install/Vacuum/Job and the `#[allow(
  dead_code)]` modules are unused phase-3 scaffold; not reviewed for correctness.
