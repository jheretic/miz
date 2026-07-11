//! Pure formatters for the render layer. The implementations live in
//! `common/fmt.rs` (usable without importing `render`); this module re-exports
//! them so render call sites keep referring to `render::fmt`.

// Re-exported for the render verbs split out in later phases; no render call
// site formats yet, so allow the currently-unused glob.
#[allow(unused_imports)]
pub use crate::common::fmt::*;
