//! Top-level compiler facade crate.
//!
//! # Source provenance (C++)
//! - `compiler/libcode.cpp` (compile entry points and orchestration)
//! - `compiler/global.cpp` (session lifecycle)
//!
//! # Current scope
//! - Exposes minimal compile-session APIs.
//! - Wires parsing through production `crates/parser` APIs.
//!
//! # Canonical pipeline
//! `parse -> eval -> propagate -> normalize/type/interval (incremental) -> transform -> fir -> backend`
//!
//! The currently wired production fast path in this crate is:
//! `parse -> eval -> propagate -> (optional signal->FIR) -> codegen`.
//!
//! # Facade responsibilities
//! - Provide one orchestrator type ([`Compiler`]) for file-based compilation.
//! - Aggregate typed stage errors into one top-level [`CompilerError`] surface.
//! - Provide test/golden-oriented helper outputs (box dump, signal dump, FIR dump).
//! - Route backend generation, with consistent options, to every emitter:
//!   C++, C, Rust, Julia, AssemblyScript, interpreter `.fbc`, WASM, the JSON
//!   description, and the Cranelift JIT status report. Every entry point
//!   returns source text or bytes; a caller that needs to *run* JIT-compiled
//!   code must own the generated module itself and so lowers through the FIR
//!   entry points instead (see `crates/cranelift-ffi`).
//!
//! # API mapping status
//! - External facade API is `adapted`: it targets behavior compatibility with
//!   C++ compile flows while using Rust structs/results and explicit lane options.
//!
//! # Current lane note
//! - The active signal->FIR lowering route is [`SignalFirLane::TransformFastLane`],
//!   owned by `crates/transform`.

// Every public item carries documentation, as in `crates/transform`. The
// workspace CI gate (`cargo clippy --workspace --all-targets -- -D warnings`)
// turns this into a hard failure, so the surface cannot silently drift back to
// undocumented.
#![warn(missing_docs)]

pub mod enrobage;

mod box_preview;
mod diagnostics;
mod emitters;
mod error_mapping;
mod golden;
mod json_naming;
mod paths;
mod service;
mod signal_lowering;

pub mod execution;

use box_preview::*;
use diagnostics::*;
use error_mapping::*;
pub use golden::*;
pub use json_naming::*;
pub use paths::*;
pub use signal_lowering::render_cranelift_module_report;
use signal_lowering::*;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use boxes::{BoxId, BoxMatch, dump_box, match_box};
use codegen::backends::asc::{AscOptions, CodegenError as AscCodegenError, generate_asc_module};
use codegen::backends::c::{COptions, CodegenError as CCodegenError, generate_c_module};
use codegen::backends::codebox::{
    CodeboxOptions, CodegenError as CodeboxCodegenError, generate_codebox_module,
};
use codegen::backends::cpp::{CodegenError as CppCodegenError, CppOptions, generate_cpp_module};
use codegen::backends::cranelift::{
    CraneliftBackendError, CraneliftOptions, JitDspModule, StructFieldKind,
    diagnose_cranelift_compute_subset_gap, generate_cranelift_module,
};
use codegen::backends::interp::{
    CodegenError as InterpCodegenError, CodegenErrorCode as InterpCodegenErrorCode, FbcDspFactory,
    FbcReal, InterpOptions, generate_interp_module, write_fbc,
};
use codegen::backends::julia::{
    CodegenError as JuliaCodegenError, JuliaOptions, JuliaRealType, generate_julia_module,
};
use codegen::backends::rust::{
    CodegenError as RustCodegenError, RustOptions, RustRealType, generate_rust_module,
};
use codegen::backends::wasm::layout::WasmMemoryLayout;
use codegen::backends::wasm::{WasmBackendError, WasmJsonContext, WasmModule, WasmOptions};
use codegen::json::{
    JsonBuildOptions, JsonDescription, JsonMetaEntry, build_json_description_from_fir,
};
use errors::codes::COMP_TYPE_FAILED;
use errors::{
    Diagnostic, DiagnosticBundle, IntoDiagnostic, Label, LabelStyle, Severity, SourceSpan, Stage,
};
use fir::{
    FirId, FirStore,
    checker::{FirVerifyReport, Severity as FirVerifySeverity, verify_fir_module},
    inliner::sweep_scaffolding_drop_roots,
};
use parser::VirtualSourceMap;
use parser::{CompilationMetadataKey, CompilationMetadataSnapshot, ParseOutput, SourceReaderError};
use propagate::{ArityCache, BoxArity, PropagateError, PropagateUiOptions};
use signals::SigId;
use sigtype::TypeAnnotator;
use tlib::NodeKind;
pub use transform::schedule::SchedulingStrategy;
pub use transform::signal_fir::{
    ComputeMode, ControlRateMode, ProcessingApi, RealType, VectorEffectiveMode,
    VectorFallbackReason, VectorPipelineStatus,
};
use transform::signal_fir::{SignalFirError, SignalFirErrorCode, SignalFirOptions};
use ui::UiProgram;

/// Parse + eval + propagate output package.
///
/// This is the highest-level structural output of the box/signal pipeline
/// before any FIR lowering or backend selection happens.
///
/// Since the grouped-UI rewrite, this facade boundary owns both semantic
/// products of propagation:
/// - propagated DSP signals,
/// - canonical grouped UI layout/metadata.
#[derive(Debug)]
pub struct SignalCompileOutput {
    /// Full parser output (arena + metadata + diagnostics from parse stage).
    pub parse: ParseOutput,
    /// Aggregated top-level `declare key "value";` metadata visible after the
    /// whole parse + eval file-loading session.
    pub compilation_metadata: parser::CompilationMetadataSnapshot,
    /// Additional Faust source files loaded through evaluator-side
    /// `component(...)` / `library(...)` resolution during this session.
    pub loaded_files: Vec<PathBuf>,
    /// Evaluated `process` box expression after `eval`.
    pub process_box: BoxId,
    /// Inferred process arity (`inputs`/`outputs`) from `propagate::box_arity_typed`.
    pub process_arity: BoxArity,
    /// Final propagated output signal list.
    ///
    /// Parity note:
    /// - for ordinary Faust programs, this usually matches
    ///   `process_arity.outputs`,
    /// - for `fad(expr)`, propagation expands the signal list to
    ///   `primal outputs + tangent outputs`, so `signals.len()` may be greater
    ///   than `process_arity.outputs`.
    pub signals: Vec<SigId>,
    /// Canonical grouped UI artifact owned after the propagation boundary.
    ///
    /// Downstream FIR lowering/backends must treat this as the source of truth
    /// for `buildUserInterface`, rather than reconstructing groups from signal
    /// leaf widgets.
    pub ui: UiProgram,
    /// Evaluated `BoxId` → source definition name.
    ///
    /// Populated by the evaluator: when a named closure is forced to a concrete
    /// box, the result `BoxId` is recorded with the definition's string name.
    /// Used by the SVG draw module to label and fold named sub-diagrams.
    pub def_names: std::collections::HashMap<boxes::BoxId, String>,
    /// Clock-domain instances allocated by `ondemand` / `upsampling` /
    /// `downsampling` wrappers during propagation (roadmap P0.2).
    ///
    /// Empty for programs without clocked wrappers. In-graph `SIGCLOCKENV`
    /// tokens index into this table.
    pub clock_domains: propagate::ClockDomainTable,
}

impl SignalCompileOutput {
    /// Returns the effective propagated output arity seen by FIR/backends.
    ///
    /// This differs from [`Self::process_arity`] for forward-mode AD:
    /// `box_arity_typed(...)` intentionally keeps `fad(expr)` transparent at the
    /// box level, while propagation expands the concrete signal forest.
    #[must_use]
    pub fn propagated_output_count(&self) -> usize {
        self.signals.len()
    }
}

/// Parse + eval + propagate + FIR lowering output package.
///
/// This bundle is used by FIR-oriented backends and verifier integration.
#[derive(Debug)]
pub struct FirCompileOutput {
    /// FIR storage arena.
    pub store: FirStore,
    /// FIR module root id.
    pub module: FirId,
    /// Checked signal-level vector activation or named fallback status.
    pub vector_pipeline_status: VectorPipelineStatus,
    /// Effective scalar or checked-vector compute shape in the returned FIR.
    pub vector_effective_mode: VectorEffectiveMode,
    /// Complete first-failure diagnostic when vector selection fell back.
    pub vector_pipeline_detail: Option<String>,
}

/// Request payload for the artifact-centric WASM compile service used by the
/// `faustwasm` Rust integration.
///
/// # Role
/// This request intentionally avoids the historical C++ `cfactory` model.
/// Callers provide one self-contained compilation request and receive one owned
/// [`WasmArtifactBundle`] back. JS/host-side caches can then own the resulting
/// `{ wasm, json }` pair directly.
///
/// # Mapping status
/// `adapted` relative to the C++ `createDSPFactory(...)` entry point:
/// - preserved semantics: DSP name/source, WASM backend options, and
///   signal->FIR lane selection;
/// - intentionally omitted: factory pointer lifetime and explicit deletion.
#[derive(Debug, Clone)]
pub struct WasmArtifactRequest {
    /// Logical source name reported in diagnostics and JSON provenance.
    pub source_name: String,
    /// Faust DSP source text to compile.
    pub source: String,
    /// Extra import search directories, mirroring CLI/FFI `-I`.
    pub import_dirs: Vec<PathBuf>,
    /// Optional in-memory source bundle used to resolve `import("...")` and
    /// evaluator-side `library(...)` / `component(...)` without a host
    /// filesystem dependency.
    pub virtual_sources: VirtualSourceMap,
    /// WASM backend configuration (`-double`, memory model, etc.).
    pub wasm_options: WasmOptions,
    /// Signal->FIR lowering lane used before WASM code generation.
    pub lane: SignalFirLane,
}

impl WasmArtifactRequest {
    /// Builds a source-backed request with default import search paths, default
    /// WASM options, and the production JSON/WASM lowering lane.
    ///
    /// Mapping status: `adapted`.
    /// The artifact-oriented faustwasm service needs the full FIR module shape
    /// (`metadata`, `buildUserInterface`, lifecycle methods), so its default
    /// lane follows the transform fast lane rather than the temporary legacy
    /// summary bridge.
    #[must_use]
    pub fn new(source_name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            source_name: source_name.into(),
            source: source.into(),
            import_dirs: Vec::new(),
            virtual_sources: VirtualSourceMap::default(),
            wasm_options: WasmOptions::default(),
            lane: SignalFirLane::TransformFastLane,
        }
    }
}

/// Owned `{ wasm, json }` bundle returned by the Rust-side WASM compile service.
///
/// This is the first Phase 1 artifact contract from
/// `porting/faustwasm-dual-mode-rust-interface-plan-2026-03-26-en.md`.
/// The bundle is designed to be consumed directly by a future JS/WASM binding
/// layer or by Rust-native tests without any factory-pointer semantics.
///
/// # ABI contract
/// - [`Self::wasm_bytes`] and [`Self::dsp_json`] are a matched pair and must be
///   consumed together.
/// - [`Self::compile_options`] mirrors the JSON `compile_options` field so a
///   binding layer does not need to re-parse the JSON merely to discover the
///   emitted float/backend mode.
///
/// # Mapping status
/// `adapted` relative to the C++ `FaustWasm { data, json, ... }` result:
/// - preserved semantics: owned WASM bytes plus companion JSON;
/// - adapted: compile provenance is exposed as a dedicated field in addition to
///   the JSON payload;
/// - deferred: warnings and aux files until the corresponding Rust services are
///   ported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmArtifactBundle {
    /// Binary WebAssembly module bytes.
    pub wasm_bytes: Vec<u8>,
    /// Companion Faust JSON description for the same module.
    pub dsp_json: String,
    /// High-level compilation provenance mirrored from the JSON payload.
    pub compile_options: String,
}

/// One auxiliary file produced by [`Compiler::generate_aux_files`].
///
/// Mapping status: `adapted` — the C++ API writes the files to disk, while
/// this surface returns them in memory so wasm32 hosts without a writable
/// filesystem can consume them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxFileArtifact {
    /// Logical relative output path.
    pub path: String,
    /// Raw file contents. Text files use UTF-8 bytes.
    pub content: Vec<u8>,
    /// Whether the payload should be interpreted as binary.
    pub binary: bool,
}

impl AuxFileArtifact {
    /// One generated text file, named `<stem>.<extension>`.
    fn text(stem: &str, extension: &str, content: String) -> Self {
        Self {
            path: format!("{stem}.{extension}"),
            content: content.into_bytes(),
            binary: false,
        }
    }

    /// One generated binary file, named `<stem>.<extension>`.
    fn binary(stem: &str, extension: &str, content: Vec<u8>) -> Self {
        Self {
            path: format!("{stem}.{extension}"),
            content,
            binary: true,
        }
    }
}

/// Request payload for [`Compiler::expand_dsp`].
///
/// Mapping status: `adapted` — see that method for the one behavioral gap
/// (no box→DSP serializer yet, so the expansion returns the input verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandDspRequest {
    /// Logical source name reported in diagnostics.
    pub source_name: String,
    /// Faust DSP source text to expand.
    pub source: String,
    /// Raw Faust-CLI-style argument string; only `-I <dir>` is read here.
    pub args: String,
}

/// Request payload for [`Compiler::generate_aux_files`].
///
/// Mapping status: `adapted`.
#[derive(Debug, Clone, Default)]
pub struct GenerateAuxFilesRequest {
    /// Logical source name reported in diagnostics.
    pub source_name: String,
    /// Faust DSP source text used to generate the outputs.
    pub source: String,
    /// Raw Faust-CLI-style argument string selecting the outputs and the
    /// compilation options; see [`Compiler::generate_aux_files`] for the
    /// flags that are read.
    pub args: String,
    /// Optional in-memory library sources (e.g. embedded standard library
    /// bundle from the `wasm-ffi` build).  When non-empty these take
    /// precedence over filesystem resolution so `import("stdfaust.lib")`
    /// works without a writable host filesystem.
    pub virtual_sources: VirtualSourceMap,
}

/// Structured error returned by the helper-service methods of this facade
/// ([`Compiler::get_faustwasm_info`], [`Compiler::expand_dsp`],
/// [`Compiler::generate_aux_files`]).
///
/// These methods carry a service-shaped error instead of [`CompilerError`]
/// because their callers are host bindings (the `wasm-ffi` exports, and
/// through them `faustwasm`) that propagate a code plus a message rather
/// than a typed stage error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaustwasmServiceError {
    /// Stable machine-readable reason code.
    pub code: FaustwasmServiceErrorCode,
    /// User-facing explanation intended for JS-side propagation.
    pub message: String,
}

/// Stable error codes for the helper-service surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaustwasmServiceErrorCode {
    /// The request could not be fulfilled: either the DSP failed to compile
    /// (the common case — the message carries the rendered diagnostic), or
    /// the requested operation is recognized but not implemented yet.
    Unsupported,
    /// The caller passed an unknown query key.
    InvalidArgument,
}

impl FaustwasmServiceError {
    /// Builds an error tagged [`FaustwasmServiceErrorCode::Unsupported`].
    ///
    /// Takes any [`Display`](std::fmt::Display) value so a fallible stage can
    /// be mapped without a closure: `.map_err(FaustwasmServiceError::unsupported)?`
    /// works for [`CompilerError`], backend codegen errors, and `draw` errors
    /// alike. Every compile failure on this surface renders through here, so
    /// one rendered diagnostic shape reaches the host for all of them.
    fn unsupported(message: impl std::fmt::Display) -> Self {
        Self {
            code: FaustwasmServiceErrorCode::Unsupported,
            message: message.to_string(),
        }
    }

    /// Builds an error tagged [`FaustwasmServiceErrorCode::InvalidArgument`] for
    /// an unknown query key.
    fn invalid_argument(message: impl std::fmt::Display) -> Self {
        Self {
            code: FaustwasmServiceErrorCode::InvalidArgument,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for FaustwasmServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FaustwasmServiceError {}

/// FIR verifier configuration used at the compiler facade / CLI integration layer.
///
/// The facade keeps verifier policy explicit because different workflows need
/// different failure semantics: local exploration may allow warnings, while CI
/// or strict lane validation should fail on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FirVerifyOptions {
    /// Run FIR verifier after FIR generation and before backend codegen.
    pub enabled: bool,
    /// Treat warnings as fatal in addition to errors.
    pub strict: bool,
}

/// Main façade orchestrating the current production compilation pipeline.
///
/// Current canonical flow:
/// `parse -> eval -> propagate -> (optional signal->FIR lowering) -> codegen`.
#[derive(Clone)]
pub struct Compiler {
    fir_verify: FirVerifyOptions,
    entrypoint_name: Box<str>,
    /// Floating-point precision used for internal DSP computation in the
    /// transform fast lane. `Float32` (single precision) is the default;
    /// set to `Float64` to activate double-precision mode (`--double`).
    ///
    /// This controls the internal FIR real type only. Backend interface types
    /// such as C/C++ `FAUSTFLOAT` remain architecture-controlled.
    real_type: RealType,
    /// Maximum delay (inclusive) for which the shift/copy strategy is used.
    /// Mirrors Faust `-mcd N`. Default: 16.
    max_copy_delay: u32,
    /// Delay above which the if-based wrapping strategy is used.
    /// Mirrors Faust `-dlt N`. Default: `u32::MAX` (disabled).
    delay_line_threshold: u32,
    /// Codegen strategy for `compute()`: scalar (default) or vector mode
    /// (`-vec`/`-vs`/`-lv`).
    ///
    /// `Vector` runs the checked vector pipeline, which either certifies the
    /// chunked-loop module or fails closed to scalar lowering; the outcome is
    /// reported per compilation through [`FirCompileOutput`]'s
    /// [`VectorPipelineStatus`] / [`VectorEffectiveMode`], so a caller never
    /// has to guess which shape was emitted.
    compute_mode: ComputeMode,
    /// Signal/loop dependency scheduling policy (`-ss` /
    /// `--scheduling-strategy`), threaded through to [`SignalFirOptions`] and
    /// applied to the lowered dependency graph — the four documented values
    /// generally emit different (equivalent) statement orders.
    ///
    /// Independent of [`ComputeMode`]; defaults to
    /// [`SchedulingStrategy::DepthFirst`] in scalar and vector modes alike.
    scheduling_strategy: SchedulingStrategy,
    /// Control-rate evaluation scheduling (`-ec` / `--external-control`):
    /// [`ControlRateMode::External`] emits a separate `control` entry point.
    /// The default reproduces the classic inline-per-block contract.
    control_rate_mode: ControlRateMode,
    /// Public processing-API shape (`-os` / `--one-sample`):
    /// [`ProcessingApi::OneSample`] emits a `frame` entry point and keeps the
    /// canonical block `compute` empty. Scalar mode only, and subject to
    /// per-backend capability validation. The default reproduces the classic
    /// block contract.
    processing_api: ProcessingApi,
    /// Optional cooperative cancellation flag.
    ///
    /// When set, the evaluator checks this flag on every recursive call and
    /// returns `EvalError::Cancelled` if it has been set to `true`. The CLI
    /// uses this with a watchdog thread for `--timeout`; libfaust hosts can
    /// set it from any thread to abort compilation without killing the process.
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Optional sink for phase timings, used by the CLI `-time` flag and by
    /// embedding layers that want Faust-style internal compilation timings.
    timing_sink: Option<TimingSink>,
}

type TimingSink = Arc<dyn Fn(&str, Duration) + Send + Sync + 'static>;

/// Selects which signal->FIR lowering route is used before backend emission.
///
/// The only remaining public route is the transform-owned fast lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignalFirLane {
    /// Lowering lane owned by `crates/transform`.
    #[default]
    TransformFastLane,
}

fn parser_float_size(real_type: RealType) -> u8 {
    match real_type {
        RealType::Float32 => 1,
        RealType::Float64 => 2,
    }
}

impl WasmArtifactBundle {
    /// Repackages a compiled [`WasmModule`] into the public artifact bundle,
    /// pairing its binary and JSON with the formatted `compile_options` string.
    fn from_wasm_module(module: WasmModule, compile_options: String) -> Self {
        Self {
            wasm_bytes: module.wasm_binary,
            dsp_json: module.dsp_json,
            compile_options,
        }
    }
}

impl Compiler {
    #[must_use]
    /// Creates a new top-level compiler facade instance.
    pub fn new() -> Self {
        Self {
            fir_verify: FirVerifyOptions::default(),
            entrypoint_name: "process".into(),
            real_type: RealType::default(),
            max_copy_delay: 16,
            delay_line_threshold: u32::MAX,
            compute_mode: ComputeMode::Scalar,
            scheduling_strategy: SchedulingStrategy::DepthFirst,
            control_rate_mode: ControlRateMode::InlinePerBlock,
            processing_api: ProcessingApi::Block,
            cancel: None,
            timing_sink: None,
        }
    }

    /// Returns a compiler facade configured with FIR verifier settings.
    #[must_use]
    pub fn with_fir_verify_options(mut self, fir_verify: FirVerifyOptions) -> Self {
        self.fir_verify = fir_verify;
        self
    }

    /// Returns a compiler facade configured to use a custom top-level DSP
    /// entry-point name instead of the default `process`.
    #[must_use]
    pub fn with_process_name(mut self, entrypoint_name: impl Into<Box<str>>) -> Self {
        self.entrypoint_name = entrypoint_name.into();
        self
    }

    /// Returns a compiler facade configured to use the given floating-point
    /// precision for internal DSP computation (transform fast lane only).
    ///
    /// This mirrors Faust `-double` semantics for the C/C++ backends: the
    /// generated DSP core uses `double`, while the external `FAUSTFLOAT`
    /// interface remains controlled by the architecture layer.
    #[must_use]
    pub fn with_real_type(mut self, real_type: RealType) -> Self {
        self.real_type = real_type;
        self
    }

    /// Sets the max-copy-delay threshold (`-mcd N`).
    ///
    /// Delays ≤ `n` use the shift/copy strategy (no `fIOTA`).  Default: 16.
    #[must_use]
    pub fn with_mcd(mut self, n: u32) -> Self {
        self.max_copy_delay = n;
        self
    }

    /// Sets the delay-line threshold (`-dlt N`).
    ///
    /// Delays > `n` use the if-based wrapping strategy (per-line counter,
    /// exact buffer size).  Default: `u32::MAX` (disabled).
    #[must_use]
    pub fn with_dlt(mut self, n: u32) -> Self {
        self.delay_line_threshold = n;
        self
    }

    /// Selects the `compute()` codegen strategy (`-vec` / scalar).
    ///
    /// [`ComputeMode::Vector`] restructures `compute` into an outer chunk loop
    /// (`-vs` size, `-lv` driver variant). Selection is checked, not blind: a
    /// shape the vector pipeline cannot certify falls back to scalar lowering
    /// rather than emitting unverified code, and either way the result is
    /// bit-exact against scalar output for the same program. Inspect
    /// [`FirCompileOutput::vector_effective_mode`] to see which shape was
    /// emitted.
    #[must_use]
    pub fn with_compute_mode(mut self, mode: ComputeMode) -> Self {
        self.compute_mode = mode;
        self
    }

    /// Selects the control-rate evaluation scheduling (`-ec`).
    ///
    /// [`ControlRateMode::External`] moves block-rate control work into a
    /// separate `control` entry point that the host schedules explicitly.
    /// Subject to per-backend capability validation at compile time.
    #[must_use]
    pub fn with_control_rate_mode(mut self, mode: ControlRateMode) -> Self {
        self.control_rate_mode = mode;
        self
    }

    /// Selects the public processing-API shape (`-os`).
    ///
    /// [`ProcessingApi::OneSample`] requests the flat-array `frame` entry
    /// point with an empty canonical `compute`. Rejected in vector mode and
    /// subject to per-backend capability validation at compile time.
    #[must_use]
    pub fn with_processing_api(mut self, api: ProcessingApi) -> Self {
        self.processing_api = api;
        self
    }

    /// Selects the signal/loop dependency scheduling strategy (`-ss` /
    /// `--scheduling-strategy`).
    ///
    /// Scalar lowering applies the strategy to the hierarchical signal DAG;
    /// vector lowering applies the same strategy to each completed loop-DAG
    /// epoch. Fixed reverse-AD epochs remain outside this pluggable order.
    /// Independent of [`ComputeMode`]: selecting `-vec` does not change the
    /// default or decode a second scheduling option.
    #[must_use]
    pub fn with_scheduling_strategy(mut self, strategy: SchedulingStrategy) -> Self {
        self.scheduling_strategy = strategy;
        self
    }

    /// Returns a compiler facade with a cooperative cancellation flag.
    ///
    /// The caller retains an `Arc<AtomicBool>` clone and can set it to `true`
    /// from any thread to request cancellation. The evaluator checks the flag
    /// on every recursive call and returns a `Cancelled` error.
    ///
    /// This is the library-safe alternative to `process::exit`: the CLI uses
    /// a watchdog thread for `--timeout`; libfaust hosts can set the flag on
    /// user abort without killing the process.
    #[must_use]
    pub fn with_cancel(mut self, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Returns a compiler facade that reports internal phase timings.
    #[must_use]
    pub fn with_timing_sink<F>(mut self, sink: F) -> Self
    where
        F: Fn(&str, Duration) + Send + Sync + 'static,
    {
        self.timing_sink = Some(Arc::new(sink));
        self
    }

    /// Runs `f`, reporting its wall-clock duration as phase `name` to the
    /// configured timing sink (a no-op when [`with_timing_sink`](Self::with_timing_sink)
    /// was never called). Returns `f`'s result unchanged.
    fn time_phase<T>(&self, name: &'static str, f: impl FnOnce() -> T) -> T {
        time_phase_with_sink(self.timing_sink.as_ref(), name, f)
    }

    /// Bundles the facade-owned lowering knobs into a [`SignalLoweringContext`]
    /// for the given lane. Shared by every native-backend emitter so they all
    /// observe the same FIR verify policy, real type, delay parameters, and
    /// timing sink.
    fn lowering_ctx(&self, lane: SignalFirLane) -> SignalLoweringContext {
        SignalLoweringContext {
            lane,
            fir_verify: self.fir_verify,
            real_type: self.real_type,
            max_copy_delay: self.max_copy_delay,
            delay_line_threshold: self.delay_line_threshold,
            compute_mode: self.compute_mode,
            scheduling_strategy: self.scheduling_strategy,
            control_rate_mode: self.control_rate_mode,
            processing_api: self.processing_api,
            timing_sink: self.timing_sink.clone(),
        }
    }

    /// Lowers propagated signals to a FIR module using the facade-owned verify
    /// policy, real type, and delay parameters, mapping the lowering error into
    /// the top-level [`CompilerError`] surface.
    fn lower_to_fir(
        &self,
        source: &str,
        signals: &SignalCompileOutput,
        lane: SignalFirLane,
    ) -> Result<FirCompileOutput, CompilerError> {
        lower_signals_to_fir(
            source,
            signals,
            lane,
            self.fir_verify,
            self.real_type,
            self.max_copy_delay,
            self.delay_line_threshold,
            self.compute_mode,
            self.scheduling_strategy,
            self.control_rate_mode,
            self.processing_api,
        )
        .map_err(|error| lower_fir_error_to_compiler(source, error))
    }

    #[must_use]
    /// Returns the crate package version used by this binary/library build.
    pub fn version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Parses one source string through the production parser crate.
    ///
    /// Returns [`CompilerError::Parse`] when parser recovery/errors are present.
    pub fn compile_source(
        &self,
        source_name: &str,
        source: &str,
    ) -> Result<ParseOutput, CompilerError> {
        let output = self.time_phase("parser", || {
            parser::parse_program_with_precision_and_metadata(
                source,
                source_name,
                parser_float_size(self.real_type),
                parser::CompilationMetadataStore::new(source_name),
            )
        });
        ensure_parse_success(source_name, output)
    }

    /// Parses one source file and expands local imports using `search_paths`.
    ///
    /// `search_paths` are treated like C++ `-I/--import-dir` entries: they are
    /// searched before the built-in file-backed defaults (`master` directory,
    /// `FAUST_LIB_PATH`, executable-relative `share/faust`, and the usual
    /// system install roots).
    ///
    /// Returns [`CompilerError::Import`] for import resolution/cycle failures.
    pub fn compile_file(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
    ) -> Result<ParseOutput, CompilerError> {
        let import_search_paths = merge_import_search_paths(path, search_paths);
        let output = self
            .time_phase("parser", || {
                parser::parse_file_with_imports_and_precision(
                    path,
                    &import_search_paths,
                    parser_float_size(self.real_type),
                )
            })
            .map_err(CompilerError::import)?;
        ensure_parse_success(&path.display().to_string(), output)
    }

    /// Parses one source file using the same default library search model as the
    /// C++ compiler frontend.
    pub fn compile_file_default(&self, path: &Path) -> Result<ParseOutput, CompilerError> {
        self.compile_file(path, &[])
    }

    /// Parses, evaluates `process`, then propagates boxes to output signals.
    ///
    /// This in-memory entry point is the closest Rust equivalent of compiling a
    /// standalone Faust string in the C++ frontend. It still installs a shared
    /// top-level metadata store so parser-stage `declare` metadata and any
    /// evaluator-driven loads performed later in the same session contribute to
    /// one coherent compilation snapshot.
    pub fn compile_source_to_signals(
        &self,
        source_name: &str,
        source: &str,
    ) -> Result<SignalCompileOutput, CompilerError> {
        self.compile_source_to_signals_with_search_paths(source_name, source, &[])
    }

    /// Parses, evaluates `process`, then propagates boxes to output signals
    /// using explicit evaluator import search paths.
    ///
    /// This is the string-backed counterpart of
    /// [`Self::compile_file_to_signals`]. It exists so embedding/binding layers
    /// can compile source strings while still honoring `-I` search paths.
    pub fn compile_source_to_signals_with_search_paths(
        &self,
        source_name: &str,
        source: &str,
        search_paths: &[PathBuf],
    ) -> Result<SignalCompileOutput, CompilerError> {
        self.compile_source_to_signals_with_import_context(
            source_name,
            source,
            search_paths,
            &VirtualSourceMap::default(),
        )
    }

    /// Shared core of the `compile_*_to_signals*` family.
    ///
    /// Parses `source` (with import resolution when `search_paths` or
    /// `virtual_sources` are non-empty), evaluates the `process` entry point, and
    /// propagates the resulting boxes to output signals. `virtual_sources` lets
    /// callers supply in-memory library files instead of on-disk ones.
    fn compile_source_to_signals_with_import_context(
        &self,
        source_name: &str,
        source: &str,
        search_paths: &[PathBuf],
        virtual_sources: &VirtualSourceMap,
    ) -> Result<SignalCompileOutput, CompilerError> {
        let metadata_store = parser::CompilationMetadataStore::new(source_name);
        let output = if search_paths.is_empty() && virtual_sources.is_empty() {
            ensure_parse_success(
                source_name,
                self.time_phase("parser", || {
                    parser::parse_program_with_precision_and_metadata(
                        source,
                        source_name,
                        parser_float_size(self.real_type),
                        metadata_store.clone(),
                    )
                }),
            )?
        } else {
            ensure_parse_success(
                source_name,
                self.time_phase("parser", || {
                    parser::parse_program_with_imports_and_precision_and_metadata(
                        source,
                        source_name,
                        search_paths,
                        virtual_sources,
                        metadata_store.clone(),
                        parser_float_size(self.real_type),
                    )
                })
                .map_err(CompilerError::import)?,
            )?
        };
        let mut eval_source_context = if search_paths.is_empty() && virtual_sources.is_empty() {
            eval::EvalSourceContext::memory_with_metadata(metadata_store)
        } else {
            eval::EvalSourceContext::memory_with_search_paths_metadata_and_virtual_sources(
                search_paths,
                virtual_sources.clone(),
                metadata_store,
            )
        };
        eval_source_context.sample_precision = match self.real_type {
            RealType::Float32 => eval::SamplePrecision::Float32,
            RealType::Float64 => eval::SamplePrecision::Float64,
        };
        self.pipeline_to_signals(source_name, output, Some(eval_source_context))
    }

    /// Parses one file, evaluates `process`, then propagates boxes to output signals.
    ///
    /// Unlike [`compile_source_to_signals`](Self::compile_source_to_signals),
    /// this file-backed entry point also installs an [`eval::EvalSourceContext`]
    /// so Phase 4 can resolve `component("...")` and `library("...")` with the
    /// same relative-file/import-search semantics as the C++ compiler.
    pub fn compile_file_to_signals(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
    ) -> Result<SignalCompileOutput, CompilerError> {
        let import_search_paths = merge_import_search_paths(path, search_paths);
        let metadata_store = parser::CompilationMetadataStore::new(
            &path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .to_string_lossy(),
        );
        let output = ensure_parse_success(
            &path.display().to_string(),
            self.time_phase("parser", || {
                parser::parse_file_with_imports_and_precision_and_metadata(
                    path,
                    &import_search_paths,
                    metadata_store.clone(),
                    parser_float_size(self.real_type),
                )
            })
            .map_err(CompilerError::import)?,
        )?;
        let mut eval_source_context = eval::EvalSourceContext::for_file_with_metadata(
            path,
            &import_search_paths,
            metadata_store,
        );
        eval_source_context.sample_precision = match self.real_type {
            RealType::Float32 => eval::SamplePrecision::Float32,
            RealType::Float64 => eval::SamplePrecision::Float64,
        };
        self.pipeline_to_signals(
            &path.display().to_string(),
            output,
            Some(eval_source_context),
        )
    }

    /// Parses one file with default import search path, then runs eval+propagate.
    ///
    /// The default search set follows the C++ frontend model:
    /// current file directory, `FAUST_LIB_PATH`, executable-relative
    /// `share/faust`, then standard system install roots.
    pub fn compile_file_default_to_signals(
        &self,
        path: &Path,
    ) -> Result<SignalCompileOutput, CompilerError> {
        self.compile_file_to_signals(path, &[])
    }

    /// Runs eval+propagate on an already parsed Faust program.
    ///
    /// This is an advanced entry point used by tooling/tests that need to alter
    /// parse metadata before Phase 4 (for example diagnostics fallback checks).
    /// No file-backed evaluator source context is installed here, so nested
    /// `component(...)` / `library(...)` resolution keeps the in-memory
    /// semantics of [`eval::EvalSourceContext::memory`].
    pub fn compile_parsed_to_signals(
        &self,
        source_name: &str,
        output: ParseOutput,
    ) -> Result<SignalCompileOutput, CompilerError> {
        self.pipeline_to_signals(source_name, output, None)
    }

    /// Runs the shared `parse output -> eval -> arity -> propagate` pipeline.
    ///
    /// This is the semantic heart of the facade. All higher-level helpers
    /// (`compile_*_to_signals`, backend emitters, FIR dump paths) eventually
    /// flow through this function so they observe the same:
    /// - evaluator source-loading semantics,
    /// - top-level metadata aggregation rules,
    /// - diagnostic enrichment policy,
    /// - process arity inference and signal propagation contract.
    fn pipeline_to_signals(
        &self,
        source: &str,
        mut output: ParseOutput,
        eval_source_context: Option<eval::EvalSourceContext>,
    ) -> Result<SignalCompileOutput, CompilerError> {
        let root = output
            .root
            .ok_or_else(|| CompilerError::missing_root(source))?;

        let eval_result = self.time_phase("evaluation", || {
            match (&eval_source_context, &self.cancel) {
                (Some(source_context), Some(cancel)) => {
                    eval::eval_entrypoint_with_source_context_and_cancel(
                        &mut output.state.arena,
                        root,
                        self.entrypoint_name.as_ref(),
                        source_context.clone(),
                        std::sync::Arc::clone(cancel),
                    )
                }
                (Some(source_context), None) => {
                    eval::eval_entrypoint_with_stats_and_source_context(
                        &mut output.state.arena,
                        root,
                        self.entrypoint_name.as_ref(),
                        source_context.clone(),
                    )
                }
                (None, _) => eval::eval_entrypoint_with_stats(
                    &mut output.state.arena,
                    root,
                    self.entrypoint_name.as_ref(),
                ),
            }
        });
        let (process_box, eval_stats) = eval_result.map_err(|error| {
            let node = eval_error_node(&error);
            let owner =
                node.and_then(|n| owner_definition_name_for_node(&output.state.arena, root, n));
            let mut diagnostic = error.clone().into_diagnostic();
            if let Some(n) = node {
                diagnostic = enrich_diagnostic_with_node(
                    diagnostic,
                    &output.state.arena,
                    root,
                    n,
                    owner.as_deref(),
                    self.entrypoint_name.as_ref(),
                );
                diagnostic = maybe_add_eval_source_labels(
                    diagnostic,
                    &output.state.ctx,
                    &output.state.arena,
                    root,
                    n,
                    owner.as_deref(),
                    self.entrypoint_name.as_ref(),
                );
            }
            CompilerError::Eval {
                source: source.into(),
                error: Box::new(error),
                diagnostics: bundle_from_diagnostic(diagnostic),
            }
        })?;

        let ep = self.entrypoint_name.as_ref();
        let process_flat = self
            .time_phase("box-flatten", || {
                propagate::try_build_flat_box(&output.state.arena, process_box)
            })
            .map_err(|e| {
                make_propagate_compiler_error(
                    source,
                    e.into(),
                    &output.state.arena,
                    &output.state.ctx,
                    root,
                    ep,
                    false,
                )
            })?;

        let mut arity_cache = ArityCache::new();
        let process_arity = self
            .time_phase("arity", || {
                propagate::box_arity_typed(&output.state.arena, process_flat, &mut arity_cache)
            })
            .map_err(|e| {
                make_propagate_compiler_error(
                    source,
                    e,
                    &output.state.arena,
                    &output.state.ctx,
                    root,
                    ep,
                    true,
                )
            })?;

        let compilation_metadata = eval_source_context.as_ref().map_or_else(
            || output.compilation_metadata.clone(),
            eval::EvalSourceContext::metadata_snapshot,
        );
        let ui_options =
            PropagateUiOptions::new(resolve_ui_root_label(source, &compilation_metadata));
        let inputs = propagate::make_sig_input_list(&mut output.state.arena, process_arity.inputs);
        let propagated = self
            .time_phase("propagation", || {
                propagate::propagate_typed_with_ui_options(
                    &mut output.state.arena,
                    process_flat,
                    &inputs,
                    &mut arity_cache,
                    &ui_options,
                )
            })
            .map_err(|e| {
                make_propagate_compiler_error(
                    source,
                    e,
                    &output.state.arena,
                    &output.state.ctx,
                    root,
                    ep,
                    true,
                )
            })?;
        self.time_phase("signal-type-validation", || {
            validate_signal_types(
                source,
                &output.state.arena,
                &propagated.signals,
                &propagated.ui,
            )
        })?;

        Ok(SignalCompileOutput {
            compilation_metadata,
            parse: output,
            loaded_files: eval_source_context
                .as_ref()
                .map_or_else(Vec::new, eval::EvalSourceContext::loaded_files),
            process_box,
            process_arity,
            signals: propagated.signals,
            ui: propagated.ui,
            def_names: eval_stats.def_names,
            clock_domains: propagated.clock_domains,
        })
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Collects the JSON `library_list` for a compiled program: every used parse
/// file except the primary source (`skip(1)`), followed by evaluator-loaded
/// `component`/`library` files not already present.
fn collect_library_list(signals: &SignalCompileOutput) -> Vec<String> {
    let mut library_list: Vec<String> = signals
        .parse
        .used_files
        .iter()
        .skip(1)
        .map(|file| file.to_string_lossy().into_owned())
        .collect();
    for file in &signals.loaded_files {
        let file = file.to_string_lossy().into_owned();
        if !library_list.iter().any(|existing| existing == &file) {
            library_list.push(file);
        }
    }
    library_list
}

/// Top-level compiler error surface aggregating every stage failure.
///
/// # Shared field convention
///
/// Almost every variant carries the same three pieces, so they are documented
/// once here rather than restated at each field:
///
/// - `source` — provenance for messages only: the display path for the
///   file-based entry points, or the caller-supplied logical source name for
///   the in-memory ones. It is not a key and not guaranteed to name a file that
///   exists on disk.
/// - `error` — the typed error from the stage that failed, kept so callers can
///   inspect the failure instead of parsing a string. `FirVerify` has no such
///   field (the verifier reports through the bundle) and carries `strict`
///   instead.
/// - `diagnostics` — the rendered [`DiagnosticBundle`] for that same failure.
///
/// The invariant that matters: `diagnostics` is *derived from* `error`, so the
/// two can never describe different failures. Variants whose bundle takes real
/// work to build expose a constructor ([`CompilerError::import`],
/// [`CompilerError::missing_root`], [`CompilerError::codegen_wasm`]) — prefer
/// them over building the variant by hand, which is how the two fields drift
/// apart.
#[derive(Debug)]
pub enum CompilerError {
    /// Import resolution/read failure before parse completion.
    ///
    /// Carries the structured `FRS-SRC-*` bundle built by
    /// [`SourceReaderError::to_diagnostics`]; build it with
    /// [`CompilerError::import`] rather than constructing the variant directly,
    /// so the bundle and the error can never disagree.
    Import(SourceReaderError, DiagnosticBundle),
    /// Parse output did not expose a root node.
    ///
    /// Build with [`CompilerError::missing_root`] so the bundle is attached.
    MissingRoot {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Parse failed (`errors` or `recoveries` present).
    Parse {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Number of hard parse errors reported by the parser.
        parse_errors: usize,
        /// Number of error recoveries the parser performed.
        recoveries: u32,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Eval stage failed while reducing boxes.
    Eval {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: Box<eval::EvalError>,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Propagate stage failed while lowering boxes to signals.
    Propagate {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: PropagateError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Signal type validation failed after propagation.
    Type {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: Box<str>,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Execution-option request (`-ec`/`-os`) rejected by the backend
    /// capability model before any parsing or lowering work.
    ExecutionOptions {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: crate::execution::ExecutionOptionsError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Transform stage failed while lowering signals to FIR.
    Transform {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: SignalFirError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// FIR verifier rejected a lowered FIR module before backend codegen.
    FirVerify {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Whether warnings were fatal (`--fir-verify-strict`).
        strict: bool,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// C++ backend emission failed from FIR.
    CodegenCpp {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: CppCodegenError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// C backend emission failed from FIR.
    CodegenC {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: CCodegenError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Julia backend emission failed from FIR.
    CodegenJulia {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: JuliaCodegenError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// AssemblyScript backend emission failed from FIR.
    CodegenAsc {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: AscCodegenError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Codebox (RNBO) backend emission failed from FIR.
    CodegenCodebox {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: CodeboxCodegenError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Rust backend emission failed from FIR.
    CodegenRust {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: RustCodegenError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Interpreter backend emission failed from FIR.
    CodegenInterp {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: InterpCodegenError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// Cranelift JIT backend emission failed from FIR.
    CodegenCranelift {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: CraneliftBackendError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
    /// WASM backend emission failed from FIR.
    CodegenWasm {
        /// Program provenance; see the shared field convention.
        source: Box<str>,
        /// Typed error from the stage that failed.
        error: WasmBackendError,
        /// Rendered diagnostics for this failure.
        diagnostics: DiagnosticBundle,
    },
}

impl std::fmt::Display for CompilerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Import(err, _) => write!(f, "{err}"),
            Self::MissingRoot { source, .. } => write!(f, "parse returned no root for {source}"),
            Self::Parse {
                source,
                parse_errors,
                recoveries,
                diagnostics,
            } => write!(
                f,
                "parse failed for {source}: errors={parse_errors}, recoveries={recoveries}, diagnostics={}",
                diagnostics.len()
            ),
            Self::ExecutionOptions { source, error, .. } => {
                write!(f, "execution options rejected for {source}: {error}")
            }
            Self::Eval { source, error, .. } => {
                write!(f, "evaluation failed for {source}: {error}")
            }
            Self::Propagate { source, error, .. } => {
                write!(f, "propagation failed for {source}: {error}")
            }
            Self::Type { source, error, .. } => {
                write!(f, "type validation failed for {source}: {error}")
            }
            Self::Transform { source, error, .. } => {
                write!(f, "transform failed for {source}: {error}")
            }
            Self::FirVerify {
                source,
                strict,
                diagnostics,
            } => write!(
                f,
                "FIR verification failed for {source}{}: diagnostics={}",
                if *strict { " (strict mode)" } else { "" },
                diagnostics.len()
            ),
            Self::CodegenCpp { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenC { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenJulia { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenAsc { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenCodebox { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenRust { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenInterp { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenCranelift { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenWasm { source, error, .. } => {
                write!(f, "code generation failed for {source}: {error}")
            }
        }
    }
}

impl std::error::Error for CompilerError {}

impl CompilerError {
    /// Builds an [`CompilerError::Import`] with its structured `FRS-SRC-*`
    /// bundle attached.
    #[must_use]
    pub fn import(err: SourceReaderError) -> Self {
        let diagnostics = err.to_diagnostics();
        Self::Import(err, diagnostics)
    }

    /// Builds the `FRS-CODEGEN-0001` bundle for one backend emission failure.
    ///
    /// `backend` is the `-lang` name (`cpp`, `c`, `julia`, `interp`, `wasm`),
    /// `backend_code` the backend's own stable `FRS-CGEN-<LANG>-NNNN` code, and
    /// `message` its text without the bracketed code prefix.
    ///
    /// The backend code travels as a note rather than becoming its own `FRS-*`
    /// code, mirroring `FRS-FIR-0002` + `fir_code=...`: the backends already
    /// own that taxonomy and duplicating it here would create two competing
    /// schemes for the same events.
    fn codegen_diagnostics(
        source: &str,
        backend: &str,
        backend_code: &str,
        message: &str,
    ) -> DiagnosticBundle {
        let mut bundle = DiagnosticBundle::new();
        bundle.push(
            Diagnostic::new(
                Severity::Error,
                Stage::Codegen,
                errors::codes::CODEGEN_EMISSION_FAILED,
                format!("{backend} backend code generation failed: {message}"),
            )
            .with_note(format!("backend: {backend}"))
            .with_note(format!("codegen_code={backend_code}"))
            .with_note(format!("source: {source}"))
            .with_help("this is a backend limitation or a compiler bug, not a DSP syntax error")
            .with_help("try another `-lang` backend to confirm the front-end result is sound"),
        );
        bundle
    }

    /// Builds a [`CompilerError::CodegenWasm`] with its `FRS-CODEGEN-0001`
    /// bundle attached.
    #[must_use]
    pub fn codegen_wasm(source: &str, error: WasmBackendError) -> Self {
        let diagnostics =
            Self::codegen_diagnostics(source, "wasm", error.code().as_str(), error.message());
        Self::CodegenWasm {
            source: source.into(),
            error,
            diagnostics,
        }
    }

    /// Builds a [`CompilerError::MissingRoot`] with its bundle attached.
    ///
    /// Internal invariant guard: a parse that reports no errors always exposes
    /// a root, so reaching this means a compiler bug rather than bad DSP input
    /// (an empty file, for instance, fails later with `FRS-EVAL-0001`).
    #[must_use]
    pub fn missing_root(source: &str) -> Self {
        let mut diagnostics = DiagnosticBundle::new();
        diagnostics.push(
            Diagnostic::new(
                Severity::Error,
                Stage::Compiler,
                errors::codes::COMP_MISSING_ROOT,
                format!("parse returned no root for {source}"),
            )
            .with_note("the parser reported no errors yet exposed no root node")
            .with_note("this indicates a compiler bug, not a DSP mistake")
            .with_help("please report this with the input DSP"),
        );
        Self::MissingRoot {
            source: source.into(),
            diagnostics,
        }
    }

    /// Returns the structured diagnostics carried by this error.
    ///
    /// Every variant now carries a bundle, so this never returns `None` — the
    /// `Option` is kept for source compatibility with existing callers. The
    /// exhaustive match below is deliberate: it makes the compiler reject a new
    /// variant that forgets its bundle, which is how the `code: null` fallback
    /// crept in for import and backend failures in the first place.
    #[must_use]
    pub fn diagnostics(&self) -> Option<&DiagnosticBundle> {
        match self {
            Self::Parse { diagnostics, .. } => Some(diagnostics),
            Self::Eval { diagnostics, .. } => Some(diagnostics),
            Self::Propagate { diagnostics, .. } => Some(diagnostics),
            Self::Type { diagnostics, .. } => Some(diagnostics),
            Self::Transform { diagnostics, .. } => Some(diagnostics),
            Self::ExecutionOptions { diagnostics, .. } => Some(diagnostics),
            Self::FirVerify { diagnostics, .. } => Some(diagnostics),
            Self::Import(_, diagnostics) => Some(diagnostics),
            Self::CodegenCpp { diagnostics, .. } => Some(diagnostics),
            Self::CodegenC { diagnostics, .. } => Some(diagnostics),
            Self::CodegenJulia { diagnostics, .. } => Some(diagnostics),
            Self::CodegenAsc { diagnostics, .. } => Some(diagnostics),
            Self::CodegenCodebox { diagnostics, .. } => Some(diagnostics),
            Self::CodegenRust { diagnostics, .. } => Some(diagnostics),
            Self::CodegenInterp { diagnostics, .. } => Some(diagnostics),
            Self::CodegenCranelift { diagnostics, .. } => Some(diagnostics),
            Self::CodegenWasm { diagnostics, .. } => Some(diagnostics),
            Self::MissingRoot { diagnostics, .. } => Some(diagnostics),
        }
    }
}

#[cfg(test)]
mod tests;
