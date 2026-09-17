//! Command-line interface support for the `faust-rs` binary.
//!
//! The CLI is split by concern:
//! argument parsing and compatibility normalization live in [`args`],
//! diagnostic rendering lives in [`diagnostics`] for the machine channel and
//! [`human`] for the terminal one, global timing support lives
//! in [`timer`], and process-level orchestration lives in [`runner`], which
//! delegates to [`validate`] for the command-line checks and to
//! [`fixture_mode`] / [`source_mode`] for the two per-backend emission
//! ladders.  The
//! crate root keeps `main.rs` intentionally small so the large-stack launcher
//! contract is isolated from the command implementation.

pub mod args;
pub mod diagnostics;
pub mod fixture_mode;
pub mod runner;
pub mod source_mode;
pub mod timer;
pub mod validate;

/// The terminal renderer lives in the library, so that the FFI layers render
/// the same text as this binary.
pub use compiler::diagnostics_human as human;

#[cfg(test)]
mod tests;
