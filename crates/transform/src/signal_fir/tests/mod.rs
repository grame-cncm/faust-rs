//! Test suite for the `signal_fir` fast-lane lowering pass.
//!
//! # Structure
//!
//! Each `#[test]` function follows the same three-step pattern:
//!
//! 1. **Build a signal forest** — use [`SigBuilder`] and, where needed,
//!    [`de_bruijn_rec`] / [`de_bruijn_ref`] to construct the input signal tree
//!    directly in a [`TreeArena`].
//! 2. **Lower to FIR** — call [`compile_fastlane_without_ui`] (or the full
//!    entry point for UI tests) and unwrap the [`SignalFirOutput`].
//! 3. **Assert on the FIR tree** — navigate to the relevant node with
//!    [`find_compute_loop_body`] / [`find_decl_fun_body`], strip the mandatory
//!    output cast with [`unwrap_output_cast`], then pattern-match with
//!    [`match_fir`].
//!
//! The private helpers below form a minimal test DSL that keeps the
//! assertion-focused body of each test free from boilerplate traversal code.

mod fixtures;

mod contract;
mod coverage;
mod delays;
mod execution;
mod flatten;
mod placement;
mod recursion;
mod reverse_ad;
mod ui_tables;
