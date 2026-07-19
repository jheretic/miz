//! Core logic for miz: libalpm-linked operations plus the presentation-neutral
//! abstractions (progress sink, confirmer, structured reports). No color,
//! prompting, progress rendering, or CLI parsing lives here; the `miz` bin (and
//! a future `mizd` daemon) supply those via the `ProgressSink`/`Confirmer`
//! seams. The one carve-out is plain, uncolored `eprintln!` failure diagnostics
//! at hard-error points (e.g. `common::transaction` release cleanup, target
//! not-found in the transaction verbs, relay teardown warnings) — these route
//! no palette and pull in no render code; a daemon may later lift them behind a
//! logging facade.

pub mod common;
pub mod config;
pub mod error;
pub mod operations;
pub mod params;
