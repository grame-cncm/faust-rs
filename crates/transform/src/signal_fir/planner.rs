//! Contract gate at the signal→FIR entry point.
//!
//! The planner is minimal and deterministic: it only validates static
//! contract inputs and records basic shape metadata. Resource planning
//! (delay lines, tables, UI) lives in the lowering stages.

use super::SignalFirOptions;
use super::error::{SignalFirError, SignalFirErrorCode};
use signals::SigId;

/// Minimal deterministic planning output.
///
/// Records only top-level facts that are cheap to validate and stable across
/// lowering strategies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SignalFirPlan {
    /// Number of output signals requested for compilation.
    pub signal_count: usize,
    /// Number of DSP input channels.
    pub num_inputs: usize,
    /// Number of DSP output channels.
    pub num_outputs: usize,
}

/// Validates the lowering inputs and produces a minimal planning snapshot.
///
/// Invariants guaranteed on success:
/// - `signal_count == signals.len()`
/// - `num_outputs == signals.len()` (strict top-level output contract)
/// - `options.module_name` is non-empty after trim
///
/// In other words, this planner is the contract gate for lowering, not an
/// optimizer: it rejects malformed entry conditions early and leaves all
/// lowering choices to subsequent stages.
pub(super) fn plan_signals(
    signals: &[SigId],
    num_inputs: usize,
    num_outputs: usize,
    options: &SignalFirOptions,
) -> Result<SignalFirPlan, SignalFirError> {
    if options.module_name.trim().is_empty() {
        return Err(SignalFirError::new(
            SignalFirErrorCode::InvalidOptions,
            "module_name must not be empty",
        ));
    }
    if signals.is_empty() {
        return Err(SignalFirError::new(
            SignalFirErrorCode::EmptySignalList,
            "at least one signal is required",
        ));
    }
    if num_outputs != signals.len() {
        return Err(SignalFirError::new(
            SignalFirErrorCode::OutputArityMismatch,
            format!(
                "num_outputs ({num_outputs}) must equal signal count ({}) in Step 1A",
                signals.len()
            ),
        ));
    }

    Ok(SignalFirPlan {
        signal_count: signals.len(),
        num_inputs,
        num_outputs,
    })
}
