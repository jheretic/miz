# Review — miz -I Phase 3 (mutating verbs)

Reviewed task #11 vs HEAD `d19699c`. (Persisted by the parent agent; the
reviewer subagent is read-only and has no write tool — full text was
delivered inline and is reproduced here verbatim.)

## Summary
4 findings (0 blocker, 2 major, 2 minor). **AUTH HANDLING PASSES — no zbus
stack trace on unprivileged use.** clippy `--tests -D warnings` clean; 18 unit
+ ungated integration tests green.

## AUTH (make-or-break) — PASS
Every privileged `?` routes through `map_call_error`: `mod.rs:240,244,254,256,274,290,291`.
No bare `?` bypasses it. `map_call_error` (`client.rs:117`) matches D-Bus error
NAME via `name.as_str()` (confirmed `zbus_names-4.3.2/src/utils.rs:37`;
`MethodError(OwnedErrorName,_,_)` at `zbus-5.16.0/src/error.rs:40`) → returns
`MizError::Sysupdate("...requires elevated privileges...")`, a clean message.
Names matched (AccessDenied / InteractiveAuthorizationRequired /
PolicyKit1.Error.NotAuthorized) are the standard polkit denials; the two real
unprivileged outcomes are both covered.

## Findings

**[major] -Ib reboot coupled to sysupdated availability** — `mod.rs:282`:
`images_reboot` calls `connect()`, which probes sysupdated via `list_targets`
(`mod.rs:81-85`) and maps failure to "requires systemd 257+". Reboot only needs
logind (`_targets` discarded). Plan says reboot is "not sysupdate1" (PLAN:95).
On a host without sysupdated, `miz -Ib` wrongly errors. Fix: use
`client::system_connection()?` directly.

**[major] map_call_error has no unit test** — `client.rs:117`, no `#[cfg(test)]`.
The worker said it's unit-testable; it's the riskiest logic and a dropped match
arm / renamed string would silently regress the make-or-break UX. If fabricating
a `MethodError` offline is awkward, extract `fn classify(name: &str) -> bool`
and test that.

**[minor] Job loop treats channel-disconnect as SUCCESS** — `job.rs:68-70`
`break 0`. Thread ending before our id → reported as success when outcome is
unknown. For OS-mutating verbs, prefer surfacing unknown-completion as an error
or verify via `ListJobs`. Judgment call.

**[minor] MINOR-4 unflagged on -Iu** — `mod.rs:233` `version.unwrap_or("")`
assumes ""=newest as fact; `-Ii` (`mod.rs:152`) has an explicit
`TODO(phase3/live)` for the same unverified semantics. Mirror that TODO on the
`-Iu` line.

## Passed
- bar_style_op (percent) used at `job.rs:48`, NOT byte-oriented `bar_style_dl`
  (still private, alpm callers untouched). Correction is right — Progress is
  u32 0-100 (`client.rs:103`).
- JobRemoved subscribed before Acquire (`mod.rs:239` then `:244`); Install gets
  fresh subscription (`mod.rs:253`).
- Version threading correct: `&acq_ver` (resolved) into Install (`mod.rs:256`),
  not `""`.
- status decode correct (`job.rs:79-87`: 0/+exit/-errno via from_raw_os_error).
- Non-matching job id → `continue`, no deadlock (`job.rs:65`).
- No destructive integration test runs Acquire/Install/Vacuum (`tests/images.rs`
  all ignored except --help).
- reboot = login1 `Reboot(false)` (`client.rs:142`, `mod.rs:291`).
- `-Iu host/version` pins (split_component unit-tested `mod.rs:313`).

## NOT checked
- Live polkit/sysupdated: no service on host, no network fetch tool — exact
  emitted error name and ""=newest semantics verified by knowledge, not against
  a 257+ host. Two live items (MINOR-4, live auth-name) remain for on-host pass.
- VCS artefacts: out of scope.
