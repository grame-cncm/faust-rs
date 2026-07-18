//! Typed errors for the experimental signal->FIR fast-lane.
//!
//! Error codes are stable and machine-friendly so `compiler` can map them to
//! diagnostics consistently while this lane evolves.

use std::fmt::{Display, Formatter};

/// Stable error-code namespace for the signal->FIR fast-lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalFirErrorCode {
    /// Configuration is invalid for the requested compilation.
    InvalidOptions,
    /// Input signal list is empty.
    EmptySignalList,
    /// Requested output arity does not match provided signal count.
    OutputArityMismatch,
    /// Encountered one signal node family not yet supported in the fast-lane slice.
    UnsupportedSignalNode,
    /// Encountered one signal binary operator not yet supported in the fast-lane slice.
    UnsupportedBinOp,
    /// Signal input index is invalid for the declared DSP input arity.
    InputIndexOutOfRange,
    /// Encountered a clocked node (`ondemand` / `upsampling` / `downsampling`
    /// machinery) that the FIR fast-lane cannot lower yet.
    ///
    /// This is a deliberate, structured rejection (roadmap P0.1): the front
    /// half of the clock-domain port (propagation + `signal_prepare`) accepts
    /// clocked graphs, while the back half (clock inference, guarded blocks,
    /// per-domain local time — roadmap P1–P3) has not landed. Distinct from
    /// [`Self::UnsupportedSignalNode`] so callers and tests can tell "not
    /// ported yet by design" apart from "unexpected node".
    ClockedNotLowered,
    /// Clock-environment inference or hierarchical-graph validation failed
    /// on a clocked program (ill-clocked graph: incomparable domains,
    /// annotation violations, instantaneous cycles inside a domain, …).
    ClockAnalysis,
}

impl SignalFirErrorCode {
    /// Returns stable textual code for diagnostics and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidOptions => "FRS-SFIR-0001",
            Self::EmptySignalList => "FRS-SFIR-0002",
            Self::OutputArityMismatch => "FRS-SFIR-0003",
            Self::UnsupportedSignalNode => "FRS-SFIR-0004",
            Self::UnsupportedBinOp => "FRS-SFIR-0005",
            Self::InputIndexOutOfRange => "FRS-SFIR-0006",
            Self::ClockedNotLowered => "FRS-SFIR-0007",
            Self::ClockAnalysis => "FRS-SFIR-0008",
        }
    }
}

/// Typed error returned by `transform::signal_fir` APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalFirError {
    code: SignalFirErrorCode,
    /// Human-readable detail intended for logs and terminal diagnostics.
    ///
    /// This text is not a stable API contract; callers should key behavior on
    /// [`SignalFirError::code`] / [`SignalFirErrorCode::as_str`].
    message: String,
}

impl SignalFirError {
    /// Creates a typed signal->FIR fast-lane error.
    #[must_use]
    pub fn new(code: SignalFirErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Returns the stable error code.
    #[must_use]
    pub fn code(&self) -> SignalFirErrorCode {
        self.code
    }

    /// Returns the non-stable, human-readable explanation.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for SignalFirError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for SignalFirError {}
