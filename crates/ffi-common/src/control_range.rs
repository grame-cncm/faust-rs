//! The declared range of a control at its program's precision, for Rust
//! callers of the backend FFI crates.

use crate::abi::FfiFaustFloat;

/// One control's `init`, `min`, `max` and `step`, as the program declares
/// them, at its compiled precision.
///
/// `buildUserInterface` passes these values through the C ABI's `float`
/// ([`FfiFaustFloat`]), which rounds a `-double` program's `0.1`. The
/// Interpreter and Cranelift crates also list them through a Rust-only
/// `control_ranges` function: exact for a double program, equal to the
/// `float` values for a single one. Not part of the C ABI.
///
/// Buttons and checkboxes, whose range is fixed, are not listed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlRange {
    /// The zone `buildUserInterface` passes for this control.
    pub zone: *mut FfiFaustFloat,
    /// Whether the control is a bargraph, written by the DSP: it declares
    /// no initial value and no step, and both are then 0.
    pub bargraph: bool,
    pub init: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}
