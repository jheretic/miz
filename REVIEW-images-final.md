# Final review — miz -I / --images (complete feature)

Reviewed task #13 vs HEAD `1606896`. (Persisted by the parent agent; the
reviewer subagent is read-only — delivered inline, reproduced verbatim.)

## Summary
**Zero blockers.** 5 findings (0 blocker, 2 major, 1 minor, 2 nits). Clippy
clean, 25 lib tests + 1 ungated integration test green, 7 integration tests
correctly gated `MIZ_HAS_SYSUPDATE=1`, all D-Bus signatures verified against the
systemd 261 man page, auth handling correct, build.rs/clap-only constraint held.
The two majors: a `cargo fmt` failure inside the images module, and an `-Ip`
semantic divergence from systemd's `pending` verb.

## Findings

### [major] cargo fmt --check NOT clean inside images/
- Where: mod.rs:245,272,279,316; client.rs:150,157; job.rs:63
- 7 rustfmt diffs inside the images module (separate from pre-existing config.rs
  drift). Phase-4 polish introduced the set_feature_enabled/appstream lines that
  drifted. Fix: `cargo fmt -p miz`.

### [major] -Ip does not implement systemd's `pending` semantics
- Where: mod.rs:240-264 (images_pending)
- systemd `pending` compares newest INSTALLED vs booted `IMAGE_VERSION=` in
  /etc/os-release ("is a reboot due?"). miz compares GetVersion (newest
  installed) vs CheckNew (newest downloadable) — that's "is a download
  available?", i.e. overlaps -Iy. GetVersion's own D-Bus doc warns it isn't the
  booted version, so miz -Ip can't detect installed-but-not-booted. This was a
  deliberate PLAN §1 choice ("compares current vs newest") but diverges from the
  verb being muscle-memory-mapped. Fix: (a) re-implement against IMAGE_VERSION,
  or (b) document the divergence.

### [minor] No download-only verb (systemd `acquire` unexposed)
- miz folds acquire+install into -Iu. systemd has distinct `acquire` (download,
  ready to install). pacman's -Sw/--downloadonly maps cleanly to -Iw. Follow-up.

### [nit] enable/disable confirmation → stdout, no-op notes → stderr
- mod.rs:274,283 (println, enable/disable) vs 226,258 (eprintln, -Iy/-Ip notes).
  Internally consistent (mutation-confirmation→stdout matches -Iu/-Ic), but the
  two note-classes are an implicit convention worth a one-line comment.

### [nit] Feature struct dropped the `extra` flatten field that Describe keeps
- Defensive contract still holds (serde ignores unknown keys; test proves it),
  but Feature can't surface unknown keys the way info_block pulls from
  Describe.extra. Fine — feature_block needs no extra keys; just an asymmetry.

## Production-ready vs experimental
- **Production-ready**: read-only verbs (-Il -Ii -Iy -Ig), -If features
  list/describe, --appstream, --json, --offline, -q. Signatures verified,
  defensive parsing, clean error mapping.
- **Experimental / needs live validation**: -Iu and -Ic (Job-progress loop +
  polkit auth never run against a real service), --reboot (logind path untested
  live), the two ""=newest TODOs. -Ip functional but semantically diverges.

## Follow-ups worth filing
1. Fix fmt drift in images/ (immediate — blocks "fmt clean").
2. -Ip semantics: align with systemd `pending` (booted IMAGE_VERSION) or document.
3. Mock D-Bus service for hermetic integration tests.
4. --json for all verbs, not just Describe/DescribeFeature.
5. Deeper feature-management UX beyond the thin -If wrapper.
6. Surface ListJobs() for concurrent jobs (proxy method defined, unused).
7. Expose download-only (acquire) as -Iw/--downloadonly.
8. Live-host: confirm ""=newest for Describe/Acquire; confirm exact polkit denial
   error name on a 257+ host.

## NOT checked
- Live D-Bus behaviour (no sysupdated on host; stub aborts real calls). ""=newest,
  exact polkit error name, key stability verified vs man page, not a live service.
