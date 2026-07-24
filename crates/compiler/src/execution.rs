//! Declarative backend capability model for the execution options
//! (`-ec` / `--external-control` and `-os` / `--one-sample`).
//!
//! Single source of truth required by the execution-options port plan §4.2
//! (`porting/external-control-one-sample-port-plan-2026-07-23-en.md`): CLI
//! validation, programmatic compilation, diagnostics, and tests all consult
//! this table instead of reconstructing the matrix in scattered `match`
//! statements. Validation runs before expensive parsing/lowering and is also
//! enforced at the per-backend lowering dispatch, so no programmatic entry
//! point can bypass it.
//!
//! Reference behavior at the pinned C++ commit (`8eebea429`, plan §2.1):
//! `-os` is accepted for c/cpp/dlang/cmajor/fir/rust; `-ec` for
//! c/cpp/cmajor/rust. faust-rs additionally accepts `-ec` for the FIR text
//! backend as the approved D1 diagnostic extension. `-os` is rejected in
//! vector mode; unsupported backends must fail with a stable diagnostic
//! rather than silently ignoring a flag.

use std::fmt;

use transform::signal_fir::{ComputeMode, ControlRateMode, ProcessingApi};

/// Support level of one execution dimension for one backend (plan §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionCapability {
    /// The flag is rejected for this backend with a capability diagnostic.
    Unsupported,
    /// The flag changes the emitted output shape for this backend.
    Explicit,
    /// The backend's native contract already has a tick/control split; the
    /// flag is accepted as an output-invariant compatibility alias.
    Intrinsic,
}

impl ExecutionCapability {
    /// Whether the flag is accepted (explicitly or intrinsically).
    #[must_use]
    pub fn is_supported(self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// Execution-option capabilities of one backend (plan §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendExecutionCaps {
    /// Stable backend identifier used in diagnostics (matches `-lang`).
    pub backend: &'static str,
    /// `-ec` / `--external-control` support.
    pub external_control: ExecutionCapability,
    /// `-os` / `--one-sample` support.
    pub one_sample: ExecutionCapability,
    /// Combined `-ec -os` support.
    pub combined: ExecutionCapability,
    /// Whether the backend must keep emitting the canonical block `compute`
    /// entry point (empty in one-sample mode) so the ordinary DSP interface
    /// stays satisfied.
    pub canonical_compute_required: bool,
}

/// One row per active `-lang` backend. Scaffolded backends (cmajor, codebox,
/// …) get rows when they become active, initialized from plan §5.8.
const BACKEND_CAPS: &[BackendExecutionCaps] = &[
    BackendExecutionCaps {
        backend: "c",
        external_control: ExecutionCapability::Explicit,
        one_sample: ExecutionCapability::Explicit,
        combined: ExecutionCapability::Explicit,
        canonical_compute_required: true,
    },
    BackendExecutionCaps {
        backend: "cpp",
        external_control: ExecutionCapability::Explicit,
        one_sample: ExecutionCapability::Explicit,
        combined: ExecutionCapability::Explicit,
        canonical_compute_required: true,
    },
    BackendExecutionCaps {
        backend: "rust",
        external_control: ExecutionCapability::Explicit,
        one_sample: ExecutionCapability::Explicit,
        combined: ExecutionCapability::Explicit,
        canonical_compute_required: true,
    },
    // `-ec` for FIR text is the approved D1 diagnostic extension: FIR is the
    // verified representation every supporting source backend consumes, so
    // observing the external-control shape there is intentional (plan §5.3).
    BackendExecutionCaps {
        backend: "fir",
        external_control: ExecutionCapability::Explicit,
        one_sample: ExecutionCapability::Explicit,
        combined: ExecutionCapability::Explicit,
        canonical_compute_required: true,
    },
    BackendExecutionCaps {
        backend: "interp",
        external_control: ExecutionCapability::Unsupported,
        one_sample: ExecutionCapability::Unsupported,
        combined: ExecutionCapability::Unsupported,
        canonical_compute_required: true,
    },
    BackendExecutionCaps {
        backend: "cranelift",
        external_control: ExecutionCapability::Unsupported,
        one_sample: ExecutionCapability::Unsupported,
        combined: ExecutionCapability::Unsupported,
        canonical_compute_required: true,
    },
    BackendExecutionCaps {
        backend: "wasm",
        external_control: ExecutionCapability::Unsupported,
        one_sample: ExecutionCapability::Unsupported,
        combined: ExecutionCapability::Unsupported,
        canonical_compute_required: true,
    },
    BackendExecutionCaps {
        backend: "wast",
        external_control: ExecutionCapability::Unsupported,
        one_sample: ExecutionCapability::Unsupported,
        combined: ExecutionCapability::Unsupported,
        canonical_compute_required: true,
    },
    BackendExecutionCaps {
        backend: "asc",
        external_control: ExecutionCapability::Unsupported,
        one_sample: ExecutionCapability::Unsupported,
        combined: ExecutionCapability::Unsupported,
        canonical_compute_required: true,
    },
    BackendExecutionCaps {
        backend: "julia",
        external_control: ExecutionCapability::Unsupported,
        one_sample: ExecutionCapability::Unsupported,
        combined: ExecutionCapability::Unsupported,
        canonical_compute_required: true,
    },
];

/// Returns the capability row for a stable backend identifier.
///
/// Identifiers match the primary `-lang` value (`"c"`, `"cpp"`, `"rust"`,
/// `"fir"`, `"interp"`, `"cranelift"`, `"wasm"`, `"wast"`, `"asc"`,
/// `"julia"`). Unknown identifiers return `None` so callers fail closed.
#[must_use]
pub fn backend_execution_caps(backend: &str) -> Option<&'static BackendExecutionCaps> {
    BACKEND_CAPS.iter().find(|caps| caps.backend == backend)
}

/// Backends currently accepting `-os`, for diagnostics.
fn one_sample_backends() -> String {
    supported_list(|caps| caps.one_sample)
}

/// Backends currently accepting `-ec`, for diagnostics.
fn external_control_backends() -> String {
    supported_list(|caps| caps.external_control)
}

fn supported_list(dim: impl Fn(&BackendExecutionCaps) -> ExecutionCapability) -> String {
    let names: Vec<&str> = BACKEND_CAPS
        .iter()
        .filter(|caps| dim(caps).is_supported())
        .map(|caps| caps.backend)
        .collect();
    names.join("', '")
}

/// Typed rejection of an execution-option request (stable diagnostics).
///
/// The messages mirror the pinned C++ compiler's phrasing where a C++
/// equivalent exists, with the backend list driven by the capability table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOptionsError {
    /// `-os` requested for a backend whose capability is `Unsupported`.
    OneSampleUnsupportedBackend {
        /// The rejected backend identifier.
        backend: String,
    },
    /// `-ec` requested for a backend whose capability is `Unsupported`.
    ExternalControlUnsupportedBackend {
        /// The rejected backend identifier.
        backend: String,
    },
    /// `-os` requested together with vector mode (plan §2.1: scalar only).
    OneSampleWithVectorMode,
    /// The combination is accepted by the capability table but its lowering
    /// has not landed yet (execution-options port phases 2+). Emitting the
    /// classic block output would silently ignore the flag, which the plan
    /// forbids (Phase 1 pass criteria).
    NotYetImplemented {
        /// The backend identifier.
        backend: String,
        /// The accepted-but-pending option spelling (`-ec`, `-os`, `-ec -os`).
        options: &'static str,
    },
}

impl ExecutionOptionsError {
    /// Stable diagnostic code for this rejection.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::OneSampleUnsupportedBackend { .. } => "FRS-EXEC-OS-BACKEND",
            Self::ExternalControlUnsupportedBackend { .. } => "FRS-EXEC-EC-BACKEND",
            Self::OneSampleWithVectorMode => "FRS-EXEC-OS-VECTOR",
            Self::NotYetImplemented { .. } => "FRS-EXEC-UNIMPLEMENTED",
        }
    }
}

impl fmt::Display for ExecutionOptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OneSampleUnsupportedBackend { backend } => write!(
                f,
                "'-os' option can only be used with '{}' backends (got '{backend}')",
                one_sample_backends()
            ),
            Self::ExternalControlUnsupportedBackend { backend } => write!(
                f,
                "'-ec' option can only be used with '{}' backends (got '{backend}')",
                external_control_backends()
            ),
            Self::OneSampleWithVectorMode => {
                write!(f, "'-os' option can only be used in scalar mode")
            }
            Self::NotYetImplemented { backend, options } => write!(
                f,
                "'{options}' is accepted for the '{backend}' backend but its lowering \
                 is not implemented yet (execution-options port in progress)"
            ),
        }
    }
}

impl std::error::Error for ExecutionOptionsError {}

/// Validates one execution-option request against the capability table.
///
/// Checks, in order: backend capability for `-os`, backend capability for
/// `-ec`, the `-os`/vector exclusion, then the implementation gate for
/// accepted-but-pending combinations. Returns `Ok(())` when both options are
/// at their defaults, whatever the backend.
pub fn validate_execution_options(
    backend: &str,
    control_rate_mode: ControlRateMode,
    processing_api: ProcessingApi,
    compute_mode: ComputeMode,
) -> Result<(), ExecutionOptionsError> {
    let wants_ec = control_rate_mode.is_external();
    let wants_os = processing_api.is_one_sample();
    if !wants_ec && !wants_os {
        return Ok(());
    }
    // Unknown backend identifiers fail closed as unsupported.
    let caps = backend_execution_caps(backend);
    if wants_os && !caps.is_some_and(|caps| caps.one_sample.is_supported()) {
        return Err(ExecutionOptionsError::OneSampleUnsupportedBackend {
            backend: backend.to_owned(),
        });
    }
    if wants_ec && !caps.is_some_and(|caps| caps.external_control.is_supported()) {
        return Err(ExecutionOptionsError::ExternalControlUnsupportedBackend {
            backend: backend.to_owned(),
        });
    }
    if wants_os && compute_mode.is_vector() {
        return Err(ExecutionOptionsError::OneSampleWithVectorMode);
    }
    // Accepted by the table, but the lowering phases have not landed yet
    // for every backend: fail with a stable diagnostic instead of silently
    // emitting the classic block output. Entries are removed from this list
    // as phases 3-4 land per backend. The FIR text backend dumps the
    // verified module (complete since phase 2); the C and C++ emitters
    // landed in phase 3.
    //
    // Lowering status per backend: scalar landed in phases 2-4
    // (fir: module dump; c/cpp: phase 3; rust: phase 4); vector external
    // control landed in phase 5 with the promoted-control-event certificate
    // (`-os -vec` was already rejected above, so reaching here in vector
    // mode means `-ec -vec`).
    const LOWERING_LANDED: &[&str] = &["fir", "cpp", "c", "rust"];
    if LOWERING_LANDED.contains(&backend) {
        return Ok(());
    }
    let options = match (wants_ec, wants_os) {
        (true, true) => "-ec -os",
        (true, false) => "-ec",
        (false, true) => "-os",
        (false, false) => unreachable!("early-returned above"),
    };
    Err(ExecutionOptionsError::NotYetImplemented {
        backend: backend.to_owned(),
        options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_BACKENDS: [&str; 10] = [
        "c",
        "cpp",
        "rust",
        "fir",
        "interp",
        "cranelift",
        "wasm",
        "wast",
        "asc",
        "julia",
    ];

    #[test]
    fn every_active_backend_has_an_explicit_capability_row() {
        for backend in ALL_BACKENDS {
            assert!(
                backend_execution_caps(backend).is_some(),
                "missing capability row for backend '{backend}'"
            );
        }
        assert_eq!(BACKEND_CAPS.len(), ALL_BACKENDS.len());
    }

    #[test]
    fn defaults_pass_for_every_backend_including_unknown() {
        for backend in ALL_BACKENDS.iter().chain(std::iter::once(&"nonsense")) {
            assert_eq!(
                validate_execution_options(
                    backend,
                    ControlRateMode::InlinePerBlock,
                    ProcessingApi::Block,
                    ComputeMode::Scalar,
                ),
                Ok(())
            );
        }
    }

    #[test]
    fn unsupported_backends_reject_each_flag_with_stable_codes() {
        for backend in ["interp", "cranelift", "wasm", "wast", "asc", "julia"] {
            let os = validate_execution_options(
                backend,
                ControlRateMode::InlinePerBlock,
                ProcessingApi::OneSample,
                ComputeMode::Scalar,
            )
            .unwrap_err();
            assert_eq!(os.code(), "FRS-EXEC-OS-BACKEND", "{backend}");
            let ec = validate_execution_options(
                backend,
                ControlRateMode::External,
                ProcessingApi::Block,
                ComputeMode::Scalar,
            )
            .unwrap_err();
            assert_eq!(ec.code(), "FRS-EXEC-EC-BACKEND", "{backend}");
        }
    }

    #[test]
    fn unknown_backend_fails_closed() {
        let err = validate_execution_options(
            "cmajor",
            ControlRateMode::External,
            ProcessingApi::Block,
            ComputeMode::Scalar,
        )
        .unwrap_err();
        assert_eq!(err.code(), "FRS-EXEC-EC-BACKEND");
    }

    #[test]
    fn one_sample_is_rejected_in_vector_mode_before_the_implementation_gate() {
        let err = validate_execution_options(
            "cpp",
            ControlRateMode::InlinePerBlock,
            ProcessingApi::OneSample,
            ComputeMode::Vector {
                vec_size: 32,
                loop_variant: 0,
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "FRS-EXEC-OS-VECTOR");
        assert_eq!(
            err.to_string(),
            "'-os' option can only be used in scalar mode"
        );
    }

    #[test]
    fn scalar_landed_backends_accept_all_execution_shapes() {
        for backend in ["fir", "cpp", "c"] {
            for (control, api) in [
                (ControlRateMode::External, ProcessingApi::Block),
                (ControlRateMode::InlinePerBlock, ProcessingApi::OneSample),
                (ControlRateMode::External, ProcessingApi::OneSample),
            ] {
                assert_eq!(
                    validate_execution_options(backend, control, api, ComputeMode::Scalar),
                    Ok(()),
                    "{backend}"
                );
            }
        }
    }

    #[test]
    fn fir_backend_accepts_all_execution_shapes() {
        for (control, api) in [
            (ControlRateMode::External, ProcessingApi::Block),
            (ControlRateMode::InlinePerBlock, ProcessingApi::OneSample),
            (ControlRateMode::External, ProcessingApi::OneSample),
        ] {
            assert_eq!(
                validate_execution_options("fir", control, api, ComputeMode::Scalar),
                Ok(())
            );
        }
    }

    #[test]
    fn scalar_accepting_backends_no_longer_hit_the_gate() {
        for backend in ["c", "cpp", "rust", "fir"] {
            assert_eq!(
                validate_execution_options(
                    backend,
                    ControlRateMode::External,
                    ProcessingApi::OneSample,
                    ComputeMode::Scalar,
                ),
                Ok(()),
                "{backend}"
            );
        }
    }

    #[test]
    #[allow(clippy::never_loop)]
    fn accepted_combinations_hit_the_implementation_gate_for_now() {
        // Empty since phase 4: every capability-accepting backend has its
        // scalar lowering landed. The loop shape is kept so the next backend
        // family (e.g. a future asc one-sample target) can re-enter the gate.
        for backend in [] as [&str; 0] {
            for (control, api, options) in [
                (ControlRateMode::External, ProcessingApi::Block, "-ec"),
                (
                    ControlRateMode::InlinePerBlock,
                    ProcessingApi::OneSample,
                    "-os",
                ),
                (
                    ControlRateMode::External,
                    ProcessingApi::OneSample,
                    "-ec -os",
                ),
            ] {
                let err = validate_execution_options(backend, control, api, ComputeMode::Scalar)
                    .unwrap_err();
                assert_eq!(err.code(), "FRS-EXEC-UNIMPLEMENTED", "{backend} {options}");
                assert!(err.to_string().contains(options), "{backend} {options}");
            }
        }
    }

    #[test]
    fn external_control_is_accepted_in_vector_mode_since_phase_5() {
        for backend in ["cpp", "c", "fir", "rust"] {
            assert_eq!(
                validate_execution_options(
                    backend,
                    ControlRateMode::External,
                    ProcessingApi::Block,
                    ComputeMode::Vector {
                        vec_size: 32,
                        loop_variant: 0,
                    },
                ),
                Ok(()),
                "{backend}"
            );
        }
    }

    #[test]
    fn diagnostic_messages_list_backends_from_the_table() {
        let err = validate_execution_options(
            "asc",
            ControlRateMode::InlinePerBlock,
            ProcessingApi::OneSample,
            ComputeMode::Scalar,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "'-os' option can only be used with 'c', 'cpp', 'rust', 'fir' backends (got 'asc')"
        );
    }
}
