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
//! - Route backend generation to C/C++ emitters with consistent options.
//!
//! # API mapping status
//! - External facade API is `adapted`: it targets behavior compatibility with
//!   C++ compile flows while using Rust structs/results and explicit lane options.
//!
//! # Current lane note
//! - The active signal->FIR lowering route is [`SignalFirLane::TransformFastLane`],
//!   owned by `crates/transform`.

pub mod enrobage;

mod box_preview;
mod diagnostics;
mod error_mapping;
mod golden;
mod json_naming;
mod paths;
mod signal_lowering;

use box_preview::*;
use diagnostics::*;
use error_mapping::*;
pub use golden::*;
pub use json_naming::*;
pub use paths::*;
use signal_lowering::*;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use boxes::{BoxId, BoxMatch, dump_box, match_box};
use codegen::backends::asc::{AscOptions, generate_asc_module};
use codegen::backends::c::{COptions, CodegenError as CCodegenError, generate_c_module};
use codegen::backends::cpp::{CodegenError, CppOptions, generate_cpp_module};
use codegen::backends::interp::{
    CodegenError as InterpCodegenError, CodegenErrorCode as InterpCodegenErrorCode, FbcDspFactory,
    FbcReal, InterpOptions, generate_interp_module, write_fbc,
};
use codegen::backends::julia::{
    CodegenError as JuliaCodegenError, JuliaOptions, JuliaRealType, generate_julia_module,
};
use codegen::backends::wasm::layout::WasmMemoryLayout;
use codegen::backends::wasm::{
    WasmBackendError, WasmJsonContext, WasmModule, WasmOptions, generate_wasm_module_with_context,
};
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
};
use parser::VirtualSourceMap;
use parser::{CompilationMetadataKey, CompilationMetadataSnapshot, ParseOutput, SourceReaderError};
use propagate::{ArityCache, BoxArity, PropagateError, PropagateUiOptions};
use signals::SigId;
use sigtype::TypeAnnotator;
use tlib::NodeKind;
pub use transform::schedule::SchedulingStrategy;
pub use transform::signal_fir::{
    ComputeMode, RealType, VectorEffectiveMode, VectorFallbackReason, VectorPipelineStatus,
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
/// planned `faustwasm` Rust integration.
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

/// Auxiliary file payload planned for future `generateAuxFiles` support in the
/// Rust `faustwasm` service surface.
///
/// Mapping status: `deferred`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxFileArtifact {
    /// Logical relative output path.
    pub path: String,
    /// Raw file contents. Text files use UTF-8 bytes.
    pub content: Vec<u8>,
    /// Whether the payload should be interpreted as binary.
    pub binary: bool,
}

/// Request payload reserved for future `expandDSP(...)` parity support.
///
/// Mapping status: `deferred`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandDspRequest {
    /// Logical source name reported in diagnostics.
    pub source_name: String,
    /// Faust DSP source text to expand.
    pub source: String,
    /// Raw argument string as passed by `faustwasm`.
    pub args: String,
}

/// Request payload for `generateAuxFiles(...)`.
///
/// Mapping status: `adapted`.
#[derive(Debug, Clone, Default)]
pub struct GenerateAuxFilesRequest {
    /// Logical source name reported in diagnostics.
    pub source_name: String,
    /// Faust DSP source text used to generate the outputs.
    pub source: String,
    /// Raw argument string as passed by `faustwasm`.
    pub args: String,
    /// Optional in-memory library sources (e.g. embedded standard library
    /// bundle from the `wasm-ffi` build).  When non-empty these take
    /// precedence over filesystem resolution so `import("stdfaust.lib")`
    /// works without a writable host filesystem.
    pub virtual_sources: VirtualSourceMap,
}

/// Structured error returned by the `faustwasm`-oriented compiler service
/// methods when the requested helper surface is not implemented yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaustwasmServiceError {
    /// Stable machine-readable reason code.
    pub code: FaustwasmServiceErrorCode,
    /// User-facing explanation intended for JS-side propagation.
    pub message: String,
}

/// Stable error codes for the `faustwasm` helper-service surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaustwasmServiceErrorCode {
    /// The requested operation/key exists conceptually but is not implemented
    /// yet in the Rust service layer.
    Unsupported,
    /// The caller passed an unknown query key.
    InvalidArgument,
}

impl FaustwasmServiceError {
    /// Builds an error tagged [`FaustwasmServiceErrorCode::Unsupported`] for a
    /// query that is recognized but not yet implemented in the Rust service.
    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: FaustwasmServiceErrorCode::Unsupported,
            message: message.into(),
        }
    }

    /// Builds an error tagged [`FaustwasmServiceErrorCode::InvalidArgument`] for
    /// an unknown query key.
    fn invalid_argument(message: impl Into<String>) -> Self {
        Self {
            code: FaustwasmServiceErrorCode::InvalidArgument,
            message: message.into(),
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
    /// (`-vec`/`-vs`/`-lv`). Roadmap P6 (V1 plumbing only — `Vector` still
    /// lowers as `Scalar` until the `LoopGraph` slices land).
    compute_mode: ComputeMode,
    /// Signal/loop dependency scheduling policy (`-ss` /
    /// `--scheduling-strategy`). Vectorization port plan phase P2: plumbing
    /// only — stored and threaded through to [`SignalFirOptions`], but no
    /// compile path invokes [`transform::schedule::schedule`] yet.
    /// Independent of [`ComputeMode`]; defaults to
    /// [`SchedulingStrategy::DepthFirst`] in scalar and vector modes alike.
    scheduling_strategy: SchedulingStrategy,
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
    /// Roadmap P6 (V1). Vector mode is currently plumbing-only: selecting it
    /// records the option but still emits scalar code until the `LoopGraph`
    /// lowering (V2+) lands.
    #[must_use]
    pub fn with_compute_mode(mut self, mode: ComputeMode) -> Self {
        self.compute_mode = mode;
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
            .map_err(CompilerError::Import)?;
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
                .map_err(CompilerError::Import)?,
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
            .map_err(CompilerError::Import)?,
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

    /// Parses + evaluates + propagates one source, then emits C++ text.
    pub fn compile_source_to_cpp(
        &self,
        source_name: &str,
        source: &str,
        options: &CppOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_cpp_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits C text.
    pub fn compile_source_to_c(
        &self,
        source_name: &str,
        source: &str,
        options: &COptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_c_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits Julia text.
    pub fn compile_source_to_julia(
        &self,
        source_name: &str,
        source: &str,
        options: &JuliaOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_julia_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits C text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_source_to_c_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &COptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_c(source_name, &signals, options, ctx)
            .map_err(|e| lower_c_error_to_compiler(source_name, e))
    }

    /// Parses + evaluates + propagates one source, then emits Julia text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_source_to_julia_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &JuliaOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_julia(source_name, &signals, options, ctx)
            .map_err(|e| lower_julia_error_to_compiler(source_name, e))
    }

    /// Parses + evaluates + propagates one source, then emits C++ text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_source_to_cpp_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &CppOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_cpp(source_name, &signals, options, ctx)
            .map_err(|e| lower_cpp_error_to_compiler(source_name, e))
    }

    /// Parses + evaluates + propagates one source, then lowers to FIR using
    /// the selected signal->FIR lane.
    pub fn compile_source_to_fir_with_lane(
        &self,
        source_name: &str,
        source: &str,
        lane: SignalFirLane,
    ) -> Result<FirCompileOutput, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        self.lower_to_fir(source_name, &signals, lane)
    }

    /// Parses + evaluates + propagates one source, then emits a WASM module
    /// plus its matched companion JSON.
    ///
    /// This API defaults to [`SignalFirLane::TransformFastLane`] because the
    /// WASM/JSON-facing artifact surfaces need the canonical lowered FIR module
    /// with working `metadata`/`buildUserInterface` bodies.
    pub fn compile_source_to_wasm(
        &self,
        source_name: &str,
        source: &str,
        options: &WasmOptions,
    ) -> Result<WasmModule, CompilerError> {
        self.compile_source_to_wasm_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits a WASM module
    /// through the selected signal->FIR lane.
    pub fn compile_source_to_wasm_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &WasmOptions,
        lane: SignalFirLane,
    ) -> Result<WasmModule, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let lowered = self.lower_to_fir(source_name, &signals, lane)?;
        let json_context = wasm_json_context_for_memory_source(
            source_name,
            &signals,
            compile_options_json_string(Some("wasm"), options.double_precision),
        );
        generate_wasm_module_with_context(&lowered.store, lowered.module, options, &json_context)
            .map_err(|error| CompilerError::CodegenWasm {
                source: source_name.into(),
                error,
            })
    }

    /// Compiles one in-memory DSP source into an owned artifact bundle
    /// containing both the WASM bytes and the companion JSON.
    ///
    /// This is the pure-Rust compile-service entry point intended for the
    /// future `faustwasm` embedded-compiler mode. The returned
    /// [`WasmArtifactBundle`] avoids any explicit compiler-side object lifetime
    /// and can be cached directly by higher-level hosts.
    ///
    /// Requests default to [`SignalFirLane::TransformFastLane`] for the same
    /// reason as [`Compiler::compile_source_to_wasm`]: JSON/WASM artifact
    /// consumers need preserved UI and metadata fidelity.
    pub fn compile_wasm_artifact(
        &self,
        request: &WasmArtifactRequest,
    ) -> Result<WasmArtifactBundle, CompilerError> {
        let compile_options =
            compile_options_json_string(Some("wasm"), request.wasm_options.double_precision);
        let signals = self.compile_source_to_signals_with_import_context(
            &request.source_name,
            &request.source,
            &request.import_dirs,
            &request.virtual_sources,
        )?;
        let lowered = self.lower_to_fir(&request.source_name, &signals, request.lane)?;
        let mut json_context = wasm_json_context_for_memory_source(
            &request.source_name,
            &signals,
            compile_options.clone(),
        );
        json_context.include_pathnames = request
            .import_dirs
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        json_context.library_list = collect_library_list(&signals);
        let module = generate_wasm_module_with_context(
            &lowered.store,
            lowered.module,
            &request.wasm_options,
            &json_context,
        )
        .map_err(|error| CompilerError::CodegenWasm {
            source: request.source_name.clone().into(),
            error,
        })?;
        Ok(WasmArtifactBundle::from_wasm_module(
            module,
            compile_options,
        ))
    }

    /// Parses + evaluates + propagates one source, then emits strict C++-style JSON.
    ///
    /// Like the WASM artifact entry points, this API defaults to
    /// [`SignalFirLane::TransformFastLane`] so the reconstructed JSON sees the
    /// canonical FIR `metadata` and `buildUserInterface` bodies.
    pub fn compile_source_to_json(
        &self,
        source_name: &str,
        source: &str,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_json_with_lane(source_name, source, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one source, then emits strict C++-style JSON
    /// through the selected signal->FIR lane.
    pub fn compile_source_to_json_with_lane(
        &self,
        source_name: &str,
        source: &str,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_json_with_lane_and_compile_options(
            source_name,
            source,
            lane,
            compile_options_json_string(None, self.real_type == RealType::Float64),
        )
    }

    /// Parses + evaluates + propagates one source, then emits strict C++-style JSON
    /// through the selected signal->FIR lane with explicit `compile_options`.
    pub fn compile_source_to_json_with_lane_and_compile_options(
        &self,
        source_name: &str,
        source: &str,
        lane: SignalFirLane,
        compile_options: String,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let lowered = self.lower_to_fir(source_name, &signals, lane)?;
        let json = build_strict_json_description(
            &lowered.store,
            lowered.module,
            StrictJsonContext {
                filename: source_name_to_filename(source_name),
                include_pathnames: Vec::new(),
                library_list: Vec::new(),
                top_level_meta: json_meta_entries_from_snapshot(&signals.compilation_metadata),
                compile_options,
                double_precision: self.real_type == RealType::Float64,
            },
        )
        .map_err(|error| CompilerError::CodegenWasm {
            source: source_name.into(),
            error,
        })?;
        Ok(json.render())
    }

    /// Parses + evaluates + propagates one file, then emits C++ text.
    pub fn compile_file_to_cpp(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CppOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_cpp_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits C text.
    pub fn compile_file_to_c(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &COptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_c_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits Julia text.
    pub fn compile_file_to_julia(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &JuliaOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_julia_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits C text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_c_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &COptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_c(&source, &signals, options, ctx)
            .map_err(|e| lower_c_error_to_compiler(&source, e))
    }

    /// Parses + evaluates + propagates one file, then emits Julia text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_julia_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &JuliaOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_julia(&source, &signals, options, ctx)
            .map_err(|e| lower_julia_error_to_compiler(&source, e))
    }

    /// Parses + evaluates + propagates one file, then emits C++ text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_cpp_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CppOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_cpp(&source, &signals, options, ctx)
            .map_err(|e| lower_cpp_error_to_compiler(&source, e))
    }

    /// Parses + evaluates + propagates one file, then lowers to FIR using
    /// the selected signal->FIR lane.
    pub fn compile_file_to_fir_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        lane: SignalFirLane,
    ) -> Result<FirCompileOutput, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        self.lower_to_fir(&source, &signals, lane)
    }

    /// Parses + evaluates + propagates one file, then emits a WASM module
    /// plus its matched companion JSON through the selected signal->FIR lane.
    pub fn compile_file_to_wasm(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &WasmOptions,
    ) -> Result<WasmModule, CompilerError> {
        self.compile_file_to_wasm_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits a WASM module
    /// through the selected signal->FIR lane.
    pub fn compile_file_to_wasm_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &WasmOptions,
        lane: SignalFirLane,
    ) -> Result<WasmModule, CompilerError> {
        let source = path.display().to_string();
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let lowered = self.lower_to_fir(&source, &signals, lane)?;
        let json_context = wasm_json_context_for_file(
            path,
            search_paths,
            &signals,
            compile_options_json_string(Some("wasm"), options.double_precision),
        );
        generate_wasm_module_with_context(&lowered.store, lowered.module, options, &json_context)
            .map_err(|error| CompilerError::CodegenWasm {
                source: source.into(),
                error,
            })
    }

    /// Compiles one file-backed DSP source into an owned artifact bundle.
    ///
    /// Compared with [`Self::compile_file_to_wasm_with_lane`], this packages the
    /// result in the artifact-centric shape expected by the `faustwasm`
    /// dual-mode integration plan, so downstream code can treat compile mode
    /// and precompiled-artifact mode uniformly.
    pub fn compile_file_to_wasm_artifact_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &WasmOptions,
        lane: SignalFirLane,
    ) -> Result<WasmArtifactBundle, CompilerError> {
        let compile_options = compile_options_json_string(Some("wasm"), options.double_precision);
        let module = self.compile_file_to_wasm_with_lane(path, search_paths, options, lane)?;
        Ok(WasmArtifactBundle::from_wasm_module(
            module,
            compile_options,
        ))
    }

    /// Compiles one file-backed DSP source into an owned artifact bundle using
    /// the production default signal->FIR lane.
    pub fn compile_file_to_wasm_artifact(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &WasmOptions,
    ) -> Result<WasmArtifactBundle, CompilerError> {
        self.compile_file_to_wasm_artifact_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits strict C++-style JSON.
    pub fn compile_file_to_json(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_json_with_compile_options(
            path,
            search_paths,
            lane,
            compile_options_json_string(None, self.real_type == RealType::Float64),
        )
    }

    /// Parses + evaluates + propagates one file, then emits strict C++-style JSON
    /// with explicit `compile_options` provenance.
    pub fn compile_file_to_json_with_compile_options(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        lane: SignalFirLane,
        compile_options: String,
    ) -> Result<String, CompilerError> {
        let source = path.display().to_string();
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let lowered = self.lower_to_fir(&source, &signals, lane)?;
        let library_list = collect_library_list(&signals);
        let json = build_strict_json_description(
            &lowered.store,
            lowered.module,
            StrictJsonContext {
                filename: path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| path.to_string_lossy().into_owned()),
                include_pathnames: merge_import_search_paths(path, search_paths)
                    .into_iter()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .collect(),
                library_list,
                top_level_meta: json_meta_entries_from_snapshot(&signals.compilation_metadata),
                compile_options,
                double_precision: self.real_type == RealType::Float64,
            },
        )
        .map_err(|error| CompilerError::CodegenWasm {
            source: source.into(),
            error,
        })?;
        Ok(json.render())
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits C++ text.
    pub fn compile_file_default_to_cpp(
        &self,
        path: &Path,
        options: &CppOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_cpp_with_lane(path, options, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits C text.
    pub fn compile_file_default_to_c(
        &self,
        path: &Path,
        options: &COptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_c_with_lane(path, options, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits Julia text.
    pub fn compile_file_default_to_julia(
        &self,
        path: &Path,
        options: &JuliaOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_julia_with_lane(
            path,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits C text using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_c_with_lane(
        &self,
        path: &Path,
        options: &COptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_c_with_lane(path, &[], options, lane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits Julia text using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_julia_with_lane(
        &self,
        path: &Path,
        options: &JuliaOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_julia_with_lane(path, &[], options, lane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits C++ text using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_cpp_with_lane(
        &self,
        path: &Path,
        options: &CppOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_cpp_with_lane(path, &[], options, lane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then lowers to FIR using the selected signal->FIR lane.
    pub fn compile_file_default_to_fir_with_lane(
        &self,
        path: &Path,
        lane: SignalFirLane,
    ) -> Result<FirCompileOutput, CompilerError> {
        self.compile_file_to_fir_with_lane(path, &[], lane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits a WASM module scaffold.
    ///
    /// This file-backed convenience wrapper follows the same default-lane
    /// policy as [`Compiler::compile_source_to_wasm`]: artifact-oriented WASM
    /// entry points default to [`SignalFirLane::TransformFastLane`].
    pub fn compile_file_default_to_wasm(
        &self,
        path: &Path,
        options: &WasmOptions,
    ) -> Result<WasmModule, CompilerError> {
        self.compile_file_default_to_wasm_with_lane(path, options, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits a WASM module through the selected signal->FIR lane.
    pub fn compile_file_default_to_wasm_with_lane(
        &self,
        path: &Path,
        options: &WasmOptions,
        lane: SignalFirLane,
    ) -> Result<WasmModule, CompilerError> {
        self.compile_file_to_wasm_with_lane(path, &[], options, lane)
    }

    /// Compiles one file-backed DSP source with the default import search model
    /// into an owned artifact bundle.
    ///
    /// This is the file-backed companion to [`Compiler::compile_wasm_artifact`]
    /// and therefore also defaults to [`SignalFirLane::TransformFastLane`].
    pub fn compile_file_default_to_wasm_artifact(
        &self,
        path: &Path,
        options: &WasmOptions,
    ) -> Result<WasmArtifactBundle, CompilerError> {
        self.compile_file_to_wasm_artifact(path, &[], options)
    }

    /// Returns one `faustwasm` helper-info string.
    ///
    /// Current compatibility policy mirrors C++ `libFaustWasm::getInfos` for
    /// `version`, `help`, `libdir`, `includedir`, `archdir`, `dspdir`, and
    /// `pathslist`; unknown keys remain invalid.
    pub fn get_faustwasm_info(&self, what: &str) -> Result<String, FaustwasmServiceError> {
        match what {
            "version" => Ok(Self::version().to_owned()),
            "help" => Ok(faustwasm_info_help_text()),
            "libdir" => Ok(FaustInstallPaths::from_environment().render_lib_dir()),
            "includedir" => Ok(FaustInstallPaths::from_environment().render_include_dir()),
            "archdir" => Ok(FaustInstallPaths::from_environment().render_arch_dir()),
            "dspdir" => Ok(FaustInstallPaths::from_environment().render_dsp_dir()),
            "pathslist" => Ok(FaustInstallPaths::from_environment().render_paths_list()),
            _ => Err(FaustwasmServiceError::invalid_argument(format!(
                "incorrect argument passed to getInfos: {what}"
            ))),
        }
    }

    /// Validate and expand one Faust DSP source.
    ///
    /// Parses and evaluates the program using any `-I` search paths carried in
    /// `request.args`.  If compilation succeeds the original source text is
    /// returned verbatim; the Rust compiler currently has no box→DSP serializer
    /// analogous to C++ `printBox`, so the expanded form equals the input.
    ///
    /// Mirrors: `expandDSPFromString` / `expandDSPFromFile` (C++ Faust API).
    pub fn expand_dsp(&self, request: &ExpandDspRequest) -> Result<String, FaustwasmServiceError> {
        let argv: Vec<String> = request.args.split_whitespace().map(str::to_owned).collect();
        let search_paths = parse_search_paths_from_argv(&argv);
        self.compile_source_to_signals_with_search_paths(
            &request.source_name,
            &request.source,
            &search_paths,
        )
        .map(|_| request.source.clone())
        .map_err(|e| FaustwasmServiceError::unsupported(e.to_string()))
    }

    /// Generate auxiliary output files from a Faust DSP source.
    ///
    /// Inspects `request.args` for output-format flags (`-cpp`, `-c`, `-wasm`,
    /// `-json`, `-svg`) and returns one [`AuxFileArtifact`] per generated file.
    /// When no output flag is present an empty list is returned (no error).
    ///
    /// SVG generation writes to a temporary directory and collects all produced
    /// `.svg` files into the result.  The other formats are emitted in memory.
    ///
    /// Mirrors: `generateAuxFilesFromString` / `generateAuxFilesFromFile`
    /// (C++ Faust API).
    ///
    /// Additionally, faustwasm-style transpile requests using `-lang asc`
    /// produce one AssemblyScript source artifact (honoring `-cn` and `-o`).
    pub fn generate_aux_files(
        &self,
        request: &GenerateAuxFilesRequest,
    ) -> Result<Vec<AuxFileArtifact>, FaustwasmServiceError> {
        let argv: Vec<String> = request.args.split_whitespace().map(str::to_owned).collect();
        let search_paths = parse_search_paths_from_argv(&argv);
        let double = argv.iter().any(|a| a == "-double");

        // faustwasm-style transpile request: `-lang asc` produces one
        // AssemblyScript source artifact instead of the flag-driven outputs.
        if let Some(position) = argv.iter().position(|arg| arg == "-lang")
            && argv.get(position + 1).map(String::as_str) == Some("asc")
        {
            return self.generate_asc_aux_file(request, &argv, &search_paths);
        }

        let wants_cpp = argv.iter().any(|a| a == "-cpp");
        let wants_c = argv.iter().any(|a| a == "-c");
        let wants_wasm = argv.iter().any(|a| a == "-wasm");
        let wants_json = argv.iter().any(|a| a == "-json");
        let wants_svg = argv.iter().any(|a| a == "-svg");

        let mut artifacts: Vec<AuxFileArtifact> = Vec::new();
        let stem = std::path::Path::new(&request.source_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("process");

        if wants_cpp {
            let cpp = self
                .compile_source_to_cpp(
                    &request.source_name,
                    &request.source,
                    &CppOptions::default(),
                )
                .map_err(|e| FaustwasmServiceError::unsupported(e.to_string()))?;
            artifacts.push(AuxFileArtifact {
                path: format!("{stem}.cpp"),
                content: cpp.into_bytes(),
                binary: false,
            });
        }

        if wants_c {
            let c = self
                .compile_source_to_c(&request.source_name, &request.source, &COptions::default())
                .map_err(|e| FaustwasmServiceError::unsupported(e.to_string()))?;
            artifacts.push(AuxFileArtifact {
                path: format!("{stem}.c"),
                content: c.into_bytes(),
                binary: false,
            });
        }

        if wants_wasm {
            let opts = WasmOptions {
                double_precision: double,
                ..Default::default()
            };
            let wasm = self
                .compile_source_to_wasm(&request.source_name, &request.source, &opts)
                .map_err(|e| FaustwasmServiceError::unsupported(e.to_string()))?;
            artifacts.push(AuxFileArtifact {
                path: format!("{stem}.wasm"),
                content: wasm.wasm_binary,
                binary: true,
            });
            artifacts.push(AuxFileArtifact {
                path: format!("{stem}.json"),
                content: wasm.dsp_json.into_bytes(),
                binary: false,
            });
        } else if wants_json {
            let json = self
                .compile_source_to_json(&request.source_name, &request.source)
                .map_err(|e| FaustwasmServiceError::unsupported(e.to_string()))?;
            artifacts.push(AuxFileArtifact {
                path: format!("{stem}.json"),
                content: json.into_bytes(),
                binary: false,
            });
        }

        if wants_svg {
            let signals = self
                .compile_source_to_signals_with_import_context(
                    &request.source_name,
                    &request.source,
                    &search_paths,
                    &request.virtual_sources,
                )
                .map_err(|e| FaustwasmServiceError::unsupported(e.to_string()))?;
            let draw_config = draw::DrawConfig::default();
            // Use "process" as the diagram name so the root SVG file is
            // always process.svg, matching the C++ compiler convention and
            // the faustwasm FaustSvgDiagrams.from(...) expectation.
            // draw_schema_to_memory avoids any filesystem access so this
            // path is safe on wasm32-unknown-unknown targets.
            let svg_pairs = draw::draw_schema_to_memory(
                &signals.parse.state.arena,
                signals.process_box,
                "process",
                &draw_config,
                &signals.def_names,
            )
            .map_err(|e| FaustwasmServiceError::unsupported(e.to_string()))?;
            artifacts.extend(
                svg_pairs
                    .into_iter()
                    .map(|(path, content)| AuxFileArtifact {
                        path,
                        content,
                        binary: false,
                    }),
            );
        }

        Ok(artifacts)
    }

    /// Generates the AssemblyScript source artifact for a `-lang asc`
    /// `generateAuxFiles` request (`-cn` selects the class name, `-o` the
    /// artifact path; `request.virtual_sources` supplies in-memory libraries).
    fn generate_asc_aux_file(
        &self,
        request: &GenerateAuxFilesRequest,
        argv: &[String],
        search_paths: &[PathBuf],
    ) -> Result<Vec<AuxFileArtifact>, FaustwasmServiceError> {
        let arg_value = |flag: &str| {
            argv.iter()
                .position(|arg| arg == flag)
                .and_then(|position| argv.get(position + 1))
                .cloned()
        };
        let signals = self
            .compile_source_to_signals_with_import_context(
                &request.source_name,
                &request.source,
                search_paths,
                &request.virtual_sources,
            )
            .map_err(|error| FaustwasmServiceError::invalid_argument(error.to_string()))?;
        let lowered = self
            .lower_to_fir(
                &request.source_name,
                &signals,
                SignalFirLane::TransformFastLane,
            )
            .map_err(|error| FaustwasmServiceError::invalid_argument(error.to_string()))?;

        let class_name = arg_value("-cn").unwrap_or_else(|| {
            sanitize_cpp_ident(source_name_to_class(&request.source_name).as_str())
        });

        // Strict C++-style JSON snapshot, embedded as getJSON() in the output —
        // downstream tooling parses it for inputs/outputs and the UI tree
        // (mirrors the C++ asc backend's getJSON()).
        let json = build_strict_json_description(
            &lowered.store,
            lowered.module,
            StrictJsonContext {
                filename: source_name_to_filename(&request.source_name),
                include_pathnames: Vec::new(),
                library_list: Vec::new(),
                top_level_meta: json_meta_entries_from_snapshot(&signals.compilation_metadata),
                compile_options: compile_options_json_string(
                    Some("asc"),
                    self.real_type == RealType::Float64,
                ),
                double_precision: self.real_type == RealType::Float64,
            },
        )
        .ok()
        .map(|json| json.render());

        let options = AscOptions {
            class_name: Some(class_name.clone()),
            double_precision: self.real_type == RealType::Float64,
            json,
            ..AscOptions::default()
        };
        let asc = generate_asc_module(&lowered.store, lowered.module, &options)
            .map_err(|error| FaustwasmServiceError::invalid_argument(error.to_string()))?;

        let path = arg_value("-o").unwrap_or_else(|| format!("{class_name}.ts"));
        Ok(vec![AuxFileArtifact {
            path,
            content: asc.into_bytes(),
            binary: false,
        }])
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits strict C++-style JSON.
    ///
    /// This file-backed convenience wrapper follows the same default-lane
    /// policy as [`Compiler::compile_source_to_json`].
    pub fn compile_file_default_to_json(&self, path: &Path) -> Result<String, CompilerError> {
        self.compile_file_default_to_json_with_lane(path, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits strict C++-style JSON through the selected signal->FIR lane.
    pub fn compile_file_default_to_json_with_lane(
        &self,
        path: &Path,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_json(path, &[], lane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits strict C++-style JSON through the selected signal->FIR lane
    /// with explicit `compile_options` provenance.
    pub fn compile_file_default_to_json_with_lane_and_compile_options(
        &self,
        path: &Path,
        lane: SignalFirLane,
        compile_options: String,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_json_with_compile_options(path, &[], lane, compile_options)
    }

    /// Parses + evaluates + propagates one source, then emits `.fbc` bytecode
    /// text via the interpreter backend using the transform fast lane.
    pub fn compile_source_to_interp(
        &self,
        source_name: &str,
        source: &str,
        options: &InterpOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_interp_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits `.fbc` bytecode
    /// text using the selected signal->FIR lowering lane.
    pub fn compile_source_to_interp_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &InterpOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_interp(source_name, &signals, options, ctx)
            .map_err(|e| lower_interp_error_to_compiler(source_name, e))
    }

    /// Parses + evaluates + propagates one file, then emits `.fbc` bytecode
    /// text via the interpreter backend using the transform fast lane.
    pub fn compile_file_to_interp(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &InterpOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_interp_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits `.fbc` bytecode
    /// text using the selected signal->FIR lowering lane.
    pub fn compile_file_to_interp_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &InterpOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_interp(&source, &signals, options, ctx)
            .map_err(|e| lower_interp_error_to_compiler(&source, e))
    }

    /// Parses + evaluates + propagates one file with default import search
    /// path, then emits `.fbc` bytecode text via the interpreter backend.
    pub fn compile_file_default_to_interp(
        &self,
        path: &Path,
        options: &InterpOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_interp_with_lane(
            path,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file with default import search
    /// path, then emits `.fbc` bytecode text using the selected lane.
    pub fn compile_file_default_to_interp_with_lane(
        &self,
        path: &Path,
        options: &InterpOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_interp_with_lane(path, &[], options, lane)
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
        let root = output.root.ok_or_else(|| CompilerError::MissingRoot {
            source: source.into(),
        })?;

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

/// Compiler facade errors for parser-stage orchestration.
/// Top-level compiler error surface aggregating all stage failures.
#[derive(Debug)]
pub enum CompilerError {
    /// Import resolution/read failure before parse completion.
    Import(SourceReaderError),
    /// Parse output did not expose a root node.
    MissingRoot { source: Box<str> },
    /// Parse failed (`errors` or `recoveries` present).
    Parse {
        source: Box<str>,
        parse_errors: usize,
        recoveries: u32,
        diagnostics: DiagnosticBundle,
    },
    /// Eval stage failed while reducing boxes.
    Eval {
        source: Box<str>,
        error: Box<eval::EvalError>,
        diagnostics: DiagnosticBundle,
    },
    /// Propagate stage failed while lowering boxes to signals.
    Propagate {
        source: Box<str>,
        error: PropagateError,
        diagnostics: DiagnosticBundle,
    },
    /// Signal type validation failed after propagation.
    Type {
        source: Box<str>,
        error: Box<str>,
        diagnostics: DiagnosticBundle,
    },
    /// Transform stage failed while lowering signals to FIR.
    Transform {
        source: Box<str>,
        error: SignalFirError,
        diagnostics: DiagnosticBundle,
    },
    /// FIR verifier rejected a lowered FIR module before backend codegen.
    FirVerify {
        source: Box<str>,
        strict: bool,
        diagnostics: DiagnosticBundle,
    },
    /// C++ backend emission failed from FIR.
    Codegen {
        source: Box<str>,
        error: CodegenError,
    },
    /// C backend emission failed from FIR.
    CodegenC {
        source: Box<str>,
        error: CCodegenError,
    },
    /// Julia backend emission failed from FIR.
    CodegenJulia {
        source: Box<str>,
        error: JuliaCodegenError,
    },
    /// Interpreter backend emission failed from FIR.
    CodegenInterp {
        source: Box<str>,
        error: InterpCodegenError,
    },
    /// WASM backend emission failed from FIR.
    CodegenWasm {
        source: Box<str>,
        error: WasmBackendError,
    },
}

impl std::fmt::Display for CompilerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Import(err) => write!(f, "{err}"),
            Self::MissingRoot { source } => write!(f, "parse returned no root for {source}"),
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
            Self::Codegen { source, error } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenC { source, error } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenJulia { source, error } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenInterp { source, error } => {
                write!(f, "code generation failed for {source}: {error}")
            }
            Self::CodegenWasm { source, error } => {
                write!(f, "code generation failed for {source}: {error}")
            }
        }
    }
}

impl std::error::Error for CompilerError {}

impl CompilerError {
    /// Returns structured diagnostics when this error variant carries them.
    #[must_use]
    pub fn diagnostics(&self) -> Option<&DiagnosticBundle> {
        match self {
            Self::Parse { diagnostics, .. } => Some(diagnostics),
            Self::Eval { diagnostics, .. } => Some(diagnostics),
            Self::Propagate { diagnostics, .. } => Some(diagnostics),
            Self::Type { diagnostics, .. } => Some(diagnostics),
            Self::Transform { diagnostics, .. } => Some(diagnostics),
            Self::FirVerify { diagnostics, .. } => Some(diagnostics),
            Self::Codegen { .. } => None,
            Self::CodegenC { .. } => None,
            Self::CodegenJulia { .. } => None,
            Self::CodegenWasm { .. } => None,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
