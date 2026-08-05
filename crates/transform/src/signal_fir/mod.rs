//! Signal→FIR lowering: scalar path, checked vector path, and selection.
//!
//! # Inputs and outputs
//! Entry points take the propagated signal forest, the arity contract, the
//! grouped [`UiProgram`], optionally the propagation-owned
//! `ClockDomainTable`, and [`SignalFirOptions`]. They return a
//! [`SignalFirOutput`]: an owned [`FirStore`] with the module root, plus the
//! observable vector status (`vector_pipeline_status`,
//! `vector_effective_mode`, `vector_pipeline_detail`) and diagnostics.
//!
//! # Pipeline
//! 1. **Plan** — validate options and the top-level signal/arity contract.
//! 2. **Prepare** — [`crate::signal_prepare`] clones the forest into a
//!    private staging arena, normalizes, types, and verifies it
//!    (`VerifiedPreparedSignals`).
//! 3. **Clock/dependency analysis** — [`crate::clk_env`] infers clock
//!    environments (for `ondemand`/`upsampling`/`downsampling` programs),
//!    [`crate::hgraph`] builds the hierarchical dependency graph, orients
//!    effect conflicts (scalar path), and schedules it under the selected
//!    [`SchedulingStrategy`].
//! 4. **Selection** — with [`ComputeMode::Vector`], the checked vector
//!    pipeline under [`vector`] runs first; on acceptance its verified module
//!    is returned. On a named rejection it **fails closed** to scalar
//!    lowering and reports [`VectorPipelineStatus::Fallback`].
//! 5. **Scalar lowering** — `module::build_module` lowers the prepared forest
//!    along the accepted schedule (delay strategies under `delay/`, recursion,
//!    tables, UI, clocked regions, and reverse-AD carriers included).
//!
//! # Fallback semantics
//! A vector fallback is observable, never silent: the stable reason codes are
//! [`VectorFallbackReason::code`] (`FRS-VEC-FALLBACK-*`), the returned FIR is
//! scalar-shaped, and [`VectorEffectiveMode`] stays `Scalar` so retention
//! gates never count a fallback as vector coverage. Scalar/vector selection
//! never changes numeric results: vector-certified output is bit-exact
//! against scalar output for the same program.
//!
//! # Module map
//! - `module/` — scalar lowerer (decomposed `SignalToFirLower` sub-states).
//! - `delay/` — delay-line planner/manager/strategies (`-mcd`/`-dlt`).
//! - [`vector`] — checked vector pipeline; its `mod.rs` holds the
//!   authoritative producer/checker stage map.
//! - `block_reverse_ad`/`bra` — reverse-AD (`rad`) carriers: tape-free and
//!   taped backward sweeps with C++-compatible storage.
//! - [`decoration_verify`] — certified signal decorations consumed by the
//!   vector pipeline.
//! - [`pv_slice`], [`shadow`] — diagnostic/experimental surfaces (P2 vector
//!   pre-slice; schedule-conformance shadow reports).
//! - `loop_graph`, `placement`, `planner`, `cse`, `recursion`, `siggen`,
//!   `error` — internal analysis/lowering support.
//!
//! # Known unsupported behavior
//! Delays with unbounded runtime intervals, complex table-generator forms
//! depending on runtime context, and foreign functions inside `SIGGEN`
//! interpretation are rejected with typed `FRS-SFIR-*` errors. UI-program and
//! reverse-AD graphs under `-vec` fall back to scalar with stable reasons.
//!
//! # Crate boundary contract
//! - `transform` owns signal->FIR lowering entrypoints.
//! - `fir` owns FIR node model, builder, and matcher.
//! - `codegen` consumes resulting FIR modules.
//! - `compiler` chooses whether to route requests to this fast-lane.
//!
//! Development history (Step 2A–2H, P4–P6, RAD B3–B5 slices) lives in
//! `porting/` and the daily journal, not here.

mod block_reverse_ad;
mod cse;
pub mod decoration_verify;
mod delay;
mod error;
mod loop_graph;
mod module;
mod one_sample;
mod origins;
mod placement;
mod planner;
pub mod pv_slice;
mod recursion;
pub mod shadow;
mod siggen;
pub mod vector;
pub use vector::analysis as vector_analysis;
pub use vector::assemble as vector_assemble;
pub use vector::clock_ad as vector_clock_ad;
pub use vector::events as vector_events;
pub use vector::lower as vector_lower;
pub use vector::plan as vector_plan;
pub use vector::route as vector_route;
pub use vector::schedule as vector_schedule;
pub use vector::state as vector_state;
pub use vector::verify as vector_verify;

pub use error::{SignalFirError, SignalFirErrorCode};
pub use origins::{FirOrigins, FirSignalOrigin};

use fir::{FirId, FirStore, FirType};
use signals::SigId;
use std::time::{Duration, Instant};
use tlib::TreeArena;
use ui::UiProgram;

use crate::schedule::SchedulingStrategy;
use crate::signal_prepare::{
    prepare_signals_for_fir_verified, prepare_signals_for_fir_verified_with_origins,
};

/// Internal DSP computation precision used when lowering signals to FIR.
///
/// Controls the type of internal state variables, arithmetic results, math
/// function signatures, waveform table element types, and real-constant nodes
/// in the generated FIR module.
///
/// **External interface types are not affected**: audio buffer samples
/// (`FAUSTFLOAT**` in `compute`) and UI zone variables (sliders, bargraphs,
/// buttons) always use `FirType::FaustFloat` regardless of this setting.
///
/// Corresponds to Faust's `-double` compilation flag and `gFLoatSize`:
/// - `Float32` → C++ `float` (default),
/// - `Float64` → C++ `double`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RealType {
    /// 32-bit single-precision float (`float` in C++). Default.
    #[default]
    Float32,
    /// 64-bit double-precision float (`double` in C++).
    Float64,
}

impl RealType {
    /// Returns the [`FirType`] that represents this precision in FIR lowering.
    #[must_use]
    pub fn as_fir_type(self) -> FirType {
        match self {
            Self::Float32 => FirType::Float32,
            Self::Float64 => FirType::Float64,
        }
    }
}

/// Codegen strategy for the generated `compute()` body.
///
/// `Scalar` compiles the whole signal graph into one per-sample loop. `Vector`
/// (`-vec`) restructures it into an **outer chunk loop** of `vec_size` samples
/// containing a DAG of small inner loops, so the C compiler can auto-vectorize
/// the non-recursive ones (SIMD); recursive computations stay in serial loops.
///
/// Roadmap P6, vector doc V1
/// (`porting/vector-mode-analysis-port-plan-2026-06-10-en.md`). P6.6 activates
/// the independently checked signal-level path for pure graphs, fixed and
/// bounded-variable delays, symbolic recursion, stateful clock islands, and
/// expanded FAD graphs. Other shapes fail closed to scalar lowering with an
/// observable [`VectorPipelineStatus::Fallback`] reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComputeMode {
    /// One scalar loop over the whole block (`for i in 0..count`).
    #[default]
    Scalar,
    /// Vector mode: outer chunk loop of `vec_size` samples, inner loop DAG.
    Vector {
        /// Chunk size (`-vs N`; Faust default 32).
        vec_size: u32,
        /// Chunk-driver variant (`-lv`, as Faust C++): `0` = fastest (default) —
        /// a constant-trip main loop over `count - count % vec_size` plus a scalar
        /// remainder (autovectorization-friendly); `1` = simple — one loop with a
        /// runtime `min(vindex + vec_size, count)` bound.
        loop_variant: u8,
    },
}

impl ComputeMode {
    /// The default Faust vector size (`-vs`) when `-vec` is given without `-vs`.
    pub const DEFAULT_VEC_SIZE: u32 = 32;

    /// Whether this mode requests vector-mode codegen.
    #[must_use]
    pub fn is_vector(self) -> bool {
        matches!(self, Self::Vector { .. })
    }
}

/// Control-rate evaluation scheduling (`-ec` / `--external-control`), as an
/// execution dimension orthogonal to [`ComputeMode`] and [`ProcessingApi`].
///
/// Mirrors C++ Faust `gExtControl`: with [`ControlRateMode::External`],
/// block-rate control computations move out of the block/frame entry point
/// into a separate public `control` entry point that the host schedules
/// explicitly. Their results are promoted from stack locals to DSP-owned
/// storage so sample-rate code can load them across the function boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ControlRateMode {
    /// Control-rate work runs inline at the start of each block (or of each
    /// frame in one-sample mode). This is the classic Faust contract and the
    /// default.
    #[default]
    InlinePerBlock,
    /// Control-rate work is emitted in a separate `control` entry point.
    /// Neither initialization, `compute`, nor `frame` invokes it implicitly;
    /// previously stored control values stay unchanged until the host calls
    /// `control`.
    External,
}

impl ControlRateMode {
    /// Whether control-rate evaluation is externally scheduled (`-ec`).
    #[must_use]
    pub fn is_external(self) -> bool {
        matches!(self, Self::External)
    }
}

/// Public processing-API shape (`-os` / `--one-sample`), as an execution
/// dimension orthogonal to [`ComputeMode`] and [`ControlRateMode`].
///
/// Mirrors C++ Faust `gOneSample`: with [`ProcessingApi::OneSample`], the
/// module exposes a `frame(inputs, outputs)` entry point over flat channel
/// arrays — no block count, no sample loop — and the canonical block
/// `compute` is emitted empty (it never delegates to `frame`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessingApi {
    /// Block processing through the canonical
    /// `compute(count, inputs, outputs)` entry point. The default.
    #[default]
    Block,
    /// One-sample processing through `frame(inputs, outputs)`; the canonical
    /// `compute` is kept but emitted empty. Rejected in vector mode, matching
    /// the C++ contract.
    OneSample,
}

impl ProcessingApi {
    /// Whether the one-sample `frame` API is requested (`-os`).
    #[must_use]
    pub fn is_one_sample(self) -> bool {
        matches!(self, Self::OneSample)
    }
}

/// Stable reason why a requested vector compile used scalar lowering instead
/// of the independently checked signal-level pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorFallbackReason {
    /// Building the vector UI lifecycle FIR failed.
    UiProgram,
    /// Clock-environment inference failed.
    ClockAnalysis,
    /// Building or verifying the analysis decorations failed.
    Decorations,
    /// Building or verifying the P4.4 vector plan failed.
    VectorPlan,
    /// Building or verifying the P6.1 state plan failed.
    StatePlan,
    /// Building or verifying the clocked-audio-domain plan failed.
    ClockAdPlan,
    /// A reverse-audio-domain signal requires fixed epochs, so the program
    /// is lowered as scalar.
    ReverseAd,
    /// Pure P5.2 lowering failed.
    PureLowering,
    /// Building or verifying the P5.3 event-order certificate failed.
    EventCertificate,
    /// Assembling the vector FIR bodies failed.
    FirAssembly,
    /// Assembling the output stage failed.
    OutputAssembly,
    /// Final module verification rejected the emitted module.
    ModuleVerification,
}

impl VectorFallbackReason {
    /// Stable snapshot/diagnostic code used by vectorization-retention gates.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::UiProgram => "FRS-VEC-FALLBACK-UI",
            Self::ClockAnalysis => "FRS-VEC-FALLBACK-CLOCK",
            Self::Decorations => "FRS-VEC-FALLBACK-DECORATIONS",
            Self::VectorPlan => "FRS-VEC-FALLBACK-PLAN",
            Self::StatePlan => "FRS-VEC-FALLBACK-STATE",
            Self::ClockAdPlan => "FRS-VEC-FALLBACK-CLOCK-AD",
            Self::ReverseAd => "FRS-VEC-RAD-SCALAR",
            Self::PureLowering => "FRS-VEC-FALLBACK-PURE",
            Self::EventCertificate => "FRS-VEC-FALLBACK-EVENTS",
            Self::FirAssembly => "FRS-VEC-FALLBACK-ASSEMBLY",
            Self::OutputAssembly => "FRS-VEC-FALLBACK-OUTPUT",
            Self::ModuleVerification => "FRS-VEC-FALLBACK-MODULE",
        }
    }
}

/// Which vector-module path produced a [`SignalFirOutput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VectorPipelineStatus {
    /// Vector-mode codegen was not requested; the scalar path was used.
    #[default]
    NotRequested,
    /// The P4/P5/P6 producer/checker chain accepted the emitted module.
    Certified,
    /// A named unsupported shape failed closed to scalar lowering.
    Fallback(VectorFallbackReason),
}

/// Effective compute shape emitted after vector selection and fallback.
///
/// This is intentionally distinct from [`VectorPipelineStatus`]: the status
/// records why the checked vector path was or was not accepted, while this
/// value states whether the returned FIR is actually vector-shaped. Keeping
/// both prevents a successful scalar fallback from being counted as effective
/// `-vec` coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VectorEffectiveMode {
    /// The returned module uses scalar compute lowering.
    #[default]
    Scalar,
    /// The returned module was accepted by the checked vector pipeline.
    CertifiedVector,
}

/// Configuration options for [`compile_signals_to_fir_fastlane_with_ui`].
///
/// These options describe the externally visible module contract.
/// Resource planning and lowering policies stay internal to the fast-lane until
/// more slices are promoted to stable configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalFirOptions {
    /// FIR module name to emit.
    pub module_name: String,
    /// Internal DSP computation precision (default: [`RealType::Float32`]).
    ///
    /// Controls the FIR type used for internal arithmetic, state variables,
    /// math calls, waveform table elements, and real constants.
    ///
    /// External interface types (`FaustFloat`) are **not** affected: audio
    /// buffers (`FAUSTFLOAT** inputs/outputs` in `compute`) and UI zone
    /// variables (sliders, bargraphs, buttons) always use `FaustFloat`.
    ///
    /// Implicit casts between the internal real type and `FaustFloat` are
    /// emitted automatically at the DSP boundary (input sample load and output
    /// sample store) and at UI zone reads/writes.
    pub real_type: RealType,
    /// Maximum delay threshold before switching from shift/copy to a circular
    /// ring buffer.
    ///
    /// Mirrors Faust's `-mcd N` option as observed in the C++ backend:
    /// standalone `Delay1` stays special-cased, and general shifted delay
    /// lines are used for delays `< max_copy_delay`. Delays at or above the
    /// threshold switch to ring-buffer strategies. Default: 16.
    pub max_copy_delay: u32,
    /// Delay threshold at which the if-based wrapping strategy replaces the
    /// default power-of-two circular buffer.
    ///
    /// Mirrors Faust's `-dlt N` option as observed in the C++ backend:
    /// delays `< delay_line_threshold` use the circular-pow2 strategy, while
    /// delays at or above the threshold use an exact-size buffer with a
    /// per-line counter variable. Default: `u32::MAX` (disabled; all delays at
    /// or above `max_copy_delay` use circular-pow2).
    pub delay_line_threshold: u32,
    /// Codegen strategy for `compute()`: scalar (default) or vector mode
    /// (`-vec`). Accepted P6.6 programs use the checked P4/P5/P6 path; named
    /// unsupported shapes fail closed to scalar lowering.
    pub compute_mode: ComputeMode,
    /// Signal/loop dependency scheduling policy (`-ss` /
    /// `--scheduling-strategy`). Deliberately independent of [`ComputeMode`]:
    /// scalar lowering applies it to hierarchical signal regions and the
    /// checked vector path applies it to every induced loop epoch.
    pub scheduling_strategy: SchedulingStrategy,
    /// Control-rate evaluation scheduling (`-ec`), orthogonal to
    /// [`ComputeMode`] and [`ProcessingApi`]. Default: inline per block,
    /// which reproduces the classic contract byte-for-byte.
    pub control_rate_mode: ControlRateMode,
    /// Public processing-API shape (`-os`), orthogonal to [`ComputeMode`]
    /// and [`ControlRateMode`]. Default: block `compute`, which reproduces
    /// the classic contract byte-for-byte.
    pub processing_api: ProcessingApi,
    /// How the initial content of `rdtable`/`rwtable` tables is produced
    /// (`--table-init`). See [`TableInitMode`].
    pub table_init_mode: TableInitMode,
}

/// How the initial content of a generated table is produced.
///
/// `rdtable`/`rwtable` carry a `SIGGEN` generator describing their content.
/// Two strategies exist, and both are permanently supported
/// (`porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md` §5.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableInitMode {
    /// Compile the generator into its own FIR sub-module whose `fill` function
    /// computes the content at initialization time, as the C++ reference does.
    ///
    /// This is the only mode that can express content depending on the sample
    /// rate or on a foreign function, and it keeps the emitted source small:
    /// a 65536-entry table costs a loop instead of 65536 literals.
    Runtime,
    /// Evaluate the generator at compile time and emit the content as a literal
    /// initializer list.
    ///
    /// Produces a `const` table — shareable, ROM-placeable, needing no
    /// initialization phase — but only works for generators that are fully
    /// determined at compile time; others are rejected with `FRS-SFIR-0004`.
    #[default]
    Const,
}

/// Optional observer for internal signal-to-FIR compilation stages.
///
/// The observer is diagnostic only: it receives elapsed wall-clock durations
/// after a stage completes and cannot affect planning, preparation, scheduling,
/// or lowering.
pub type SignalFirTimingSink = dyn Fn(&'static str, Duration) + Send + Sync;

impl Default for SignalFirOptions {
    fn default() -> Self {
        Self {
            module_name: "mydsp".to_owned(),
            real_type: RealType::Float32,
            max_copy_delay: 16,
            delay_line_threshold: u32::MAX,
            compute_mode: ComputeMode::Scalar,
            scheduling_strategy: SchedulingStrategy::DepthFirst,
            control_rate_mode: ControlRateMode::InlinePerBlock,
            processing_api: ProcessingApi::Block,
            // S2..S6 keep `Const` as the effective default; S7 flips it to
            // `Runtime` once every backend emits sub-modules.
            table_init_mode: TableInitMode::Const,
        }
    }
}

/// Output bundle produced by [`compile_signals_to_fir_fastlane_with_ui`].
///
/// The fast-lane returns ownership of the FIR store together with the module
/// root so downstream backends can keep using normal `fir` builder/matcher APIs
/// without relying on hidden global state.
#[derive(Debug)]
pub struct SignalFirOutput {
    /// FIR storage arena.
    pub store: FirStore,
    /// Root node id of the generated FIR module.
    pub module: FirId,
    /// Source-neutral Signal/Box derivations for reachable FIR nodes.
    pub origins: FirOrigins,
    /// First-lowering order of every distinct materialized `SigId`. On the
    /// scalar forward path this is driven by the selected `Hsched`. Recursion
    /// carrier projections are omitted because they are not ordinary cached
    /// values. The trace is exported only for conformance diagnostics via
    /// [`shadow::compare_emission_order`].
    pub emission_order: Vec<SigId>,
    /// P3 schedule-conformance report comparing [`Self::emission_order`] with
    /// the selected `Hsched`. Present only when the internal diagnostic entry
    /// point or `FAUST_RS_SHADOW_REPORT` requested it and a hierarchical graph
    /// was built. The report is observation-only; computing it never changes
    /// FIR.
    pub shadow_report: Option<shadow::ShadowReport>,
    /// Observable activation/fallback state for the signal-level vector path.
    pub vector_pipeline_status: VectorPipelineStatus,
    /// Effective compute shape of the returned FIR module.
    pub vector_effective_mode: VectorEffectiveMode,
    /// Complete first-failure diagnostic retained for a vector fallback.
    ///
    /// Stable automation should continue to group by
    /// [`Self::vector_pipeline_status`]; this detail is intended for corpus
    /// triage and may include signal or loop identifiers.
    pub vector_pipeline_detail: Option<String>,
}

/// Compiles propagated signals plus canonical grouped UI into a FIR module.
///
/// This is the grouped-UI-aware fast-lane entry point used by the compiler
/// facade once `propagate` owns explicit `UiProgram` output.
///
/// Callers that intentionally compile UI-free signal forests must pass an
/// explicit placeholder [`UiProgram`] such as [`UiProgram::empty()`] or a
/// root-only synthetic program constructed by the owning integration layer.
///
/// # Current behavior (Step 2A/2B/2C/2D/2E/2F/2G/2H)
/// - validates options and top-level signal/arity contract,
/// - builds a deterministic planning snapshot,
/// - prepares and verifies the whole output forest in a private staging arena,
/// - lowers one executable bootstrap signal slice to FIR using the prepared
///   reduced type annotations for state/table/result type selection.
///
/// # Errors
/// Returns [`SignalFirError`] when options are invalid or the top-level
/// signal/arity contract is inconsistent.
///
/// # Ownership contract
/// - `signals` must already be the propagated DSP outputs for the same source
///   program that produced `ui`,
/// - `ui` is the sole source of truth for grouped layout, labels, and UI
///   metadata,
/// - signal leaf widgets are expected to carry only stable `ControlId`
///   references back into `ui`.
pub fn compile_signals_to_fir_fastlane_with_ui(
    _arena: &TreeArena,
    signals: &[SigId],
    num_inputs: usize,
    num_outputs: usize,
    ui: &UiProgram,
    options: &SignalFirOptions,
) -> Result<SignalFirOutput, SignalFirError> {
    compile_fastlane_inner(
        _arena,
        signals,
        num_inputs,
        num_outputs,
        ui,
        options,
        None,
        None,
        None,
        std::env::var_os("FAUST_RS_SHADOW_REPORT").is_some(),
    )
}

/// Compiles with the post-lowering scheduling shadow report enabled.
///
/// This diagnostic entry point exists for scheduling conformance tests. Normal
/// compilation avoids the extra graph/report traversal; developers can request
/// the same report from regular entry points with `FAUST_RS_SHADOW_REPORT=1`.
#[doc(hidden)]
pub fn compile_signals_to_fir_fastlane_with_ui_and_shadow(
    arena: &TreeArena,
    signals: &[SigId],
    num_inputs: usize,
    num_outputs: usize,
    ui: &UiProgram,
    options: &SignalFirOptions,
) -> Result<SignalFirOutput, SignalFirError> {
    compile_fastlane_inner(
        arena,
        signals,
        num_inputs,
        num_outputs,
        ui,
        options,
        None,
        None,
        None,
        true,
    )
}

/// Clock-domain-aware variant of [`compile_signals_to_fir_fastlane_with_ui`].
///
/// `clock_domains` is the propagation-owned side table
/// ([`propagate::ClockDomainTable`], roadmap P0.2). When the program contains
/// clocked wrappers, this entry runs the clock-environment inference
/// ([`crate::clk_env`]) and the hierarchical-graph validation
/// ([`crate::hgraph`]) on the prepared forest, then lowers boolean `ondemand`
/// blocks as guarded regions (roadmap P3, first slice). Programs without
/// clocked wrappers behave exactly like the plain entry point.
pub fn compile_signals_to_fir_fastlane_clocked(
    arena: &TreeArena,
    signals: &[SigId],
    num_inputs: usize,
    num_outputs: usize,
    ui: &UiProgram,
    clock_domains: &propagate::ClockDomainTable,
    options: &SignalFirOptions,
) -> Result<SignalFirOutput, SignalFirError> {
    compile_signals_to_fir_fastlane_clocked_with_timing(
        arena,
        signals,
        num_inputs,
        num_outputs,
        ui,
        clock_domains,
        options,
        None,
    )
}

/// Clock-domain-aware fast-lane entry point with observation-only stage timing.
///
/// This is the timed counterpart of
/// [`compile_signals_to_fir_fastlane_clocked`].  It is separate so the regular
/// lowering API keeps its zero-overhead, no-timing contract.
#[allow(clippy::too_many_arguments)]
pub fn compile_signals_to_fir_fastlane_clocked_with_timing(
    arena: &TreeArena,
    signals: &[SigId],
    num_inputs: usize,
    num_outputs: usize,
    ui: &UiProgram,
    clock_domains: &propagate::ClockDomainTable,
    options: &SignalFirOptions,
    timing_sink: Option<&SignalFirTimingSink>,
) -> Result<SignalFirOutput, SignalFirError> {
    compile_signals_to_fir_fastlane_clocked_with_timing_and_origins(
        arena,
        signals,
        num_inputs,
        num_outputs,
        ui,
        clock_domains,
        options,
        timing_sink,
        None,
    )
}

/// Clocked/timed lowering with explicit propagated Signal origins.
///
/// This adapted Rust entry point keeps provenance outside the hash-consed
/// Signal and FIR arenas. Callers that do not own provenance should use
/// [`compile_signals_to_fir_fastlane_clocked_with_timing`].
#[allow(clippy::too_many_arguments)]
pub fn compile_signals_to_fir_fastlane_clocked_with_timing_and_origins(
    arena: &TreeArena,
    signals: &[SigId],
    num_inputs: usize,
    num_outputs: usize,
    ui: &UiProgram,
    clock_domains: &propagate::ClockDomainTable,
    options: &SignalFirOptions,
    timing_sink: Option<&SignalFirTimingSink>,
    signal_origins: Option<&propagate::SignalOrigins>,
) -> Result<SignalFirOutput, SignalFirError> {
    compile_fastlane_inner(
        arena,
        signals,
        num_inputs,
        num_outputs,
        ui,
        options,
        Some(clock_domains),
        timing_sink,
        signal_origins,
        std::env::var_os("FAUST_RS_SHADOW_REPORT").is_some(),
    )
}

fn time_signal_fir_phase<T>(
    timing_sink: Option<&SignalFirTimingSink>,
    name: &'static str,
    f: impl FnOnce() -> T,
) -> T {
    if let Some(sink) = timing_sink {
        let start = Instant::now();
        let result = f();
        sink(name, start.elapsed());
        result
    } else {
        f()
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_fastlane_inner(
    arena: &TreeArena,
    signals: &[SigId],
    num_inputs: usize,
    num_outputs: usize,
    ui: &UiProgram,
    options: &SignalFirOptions,
    clock_domains: Option<&propagate::ClockDomainTable>,
    timing_sink: Option<&SignalFirTimingSink>,
    signal_origins: Option<&propagate::SignalOrigins>,
    build_shadow_report: bool,
) -> Result<SignalFirOutput, SignalFirError> {
    let plan = time_signal_fir_phase(timing_sink, "fir-plan", || {
        planner::plan_signals(signals, num_inputs, num_outputs, options)
    })?;
    // Preparation owns the five retyping passes, promotions, and simplify
    // normalizations.  Keep them as one atomic timing region so the measured
    // stage matches the verified preparation boundary consumed by lowering.
    let prepared = time_signal_fir_phase(timing_sink, "fir-prepare-normalize", || {
        let result = signal_origins.map_or_else(
            || prepare_signals_for_fir_verified(arena, signals, ui),
            |origins| prepare_signals_for_fir_verified_with_origins(arena, signals, ui, origins),
        );
        result.map_err(|err| {
            let signal = err.signal();
            let box_origins = err.box_origins().to_vec();
            let error = SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("signal preparation failed: {err}"),
            )
            .with_box_origins(&box_origins);
            if let Some(signal) = signal {
                error.at_signal(signal)
            } else {
                error
            }
        })
    })?;

    // Execution-options port D2: `-os` has no meaning for block-sensitive
    // reverse-AD carriers (block-scoped tape/carry state, reverse-order
    // block traversal). Reject with a typed diagnostic instead of inventing
    // a persistent one-sample semantics (plan §3.5).
    if options.processing_api.is_one_sample()
        && one_sample::contains_block_sensitive_operation(prepared.arena(), prepared.outputs())
    {
        return Err(SignalFirError::new(
            SignalFirErrorCode::BlockSensitiveOneSample,
            "'-os' is not supported for programs containing block reverse-mode              AD (BlockReverseAD/ReverseTimeRec): their semantics require the              block boundary",
        ));
    }

    // P3: build the hierarchical dependency graph, orient conflicting effects
    // independently of strategy, and schedule every prepared scalar forest.
    // The accepted Hsched drives scalar forward lowering; vector mode instead
    // schedules its strategy-independent LoopGraph.
    let mut gate_graphs: Option<(crate::hgraph::Hgraph, crate::hgraph::Hsched)> = None;
    let clocked = time_signal_fir_phase(
        timing_sink,
        "fir-clock-analysis",
        || -> Result<Option<module::ClockedPlan<'_>>, SignalFirError> {
            match clock_domains {
                Some(domains) if !domains.is_empty() => {
                    let envs =
                        crate::clk_env::annotate(prepared.arena(), domains, prepared.outputs())
                            .map_err(|err| {
                                SignalFirError::new(
                                    SignalFirErrorCode::ClockAnalysis,
                                    format!("clock-environment inference failed: {err}"),
                                )
                            })?;
                    let mut hgraph = time_signal_fir_phase(timing_sink, "fir-hgraph", || {
                        crate::hgraph::build_hgraph(
                            prepared.arena(),
                            domains,
                            &envs,
                            prepared.outputs(),
                            prepared.sig_types_map(),
                        )
                    })
                    .map_err(|err| {
                        SignalFirError::new(
                            SignalFirErrorCode::ClockAnalysis,
                            format!("hierarchical dependency graph failed: {err}"),
                        )
                    })?;
                    if matches!(options.compute_mode, ComputeMode::Scalar) {
                        let effects =
                            time_signal_fir_phase(timing_sink, "fir-scalar-effects", || {
                                vector::analysis::analyze_scalar_scheduling_effects(&prepared)
                            })
                            .map_err(|err| {
                                SignalFirError::new(
                                    SignalFirErrorCode::ClockAnalysis,
                                    format!("scalar effect analysis failed: {err}"),
                                )
                            })?;
                        time_signal_fir_phase(timing_sink, "fir-effect-orientation", || {
                            crate::hgraph::orient_effect_conflicts(&mut hgraph, &effects)
                        })
                        .map_err(|err| {
                            SignalFirError::new(
                                SignalFirErrorCode::ClockAnalysis,
                                format!("scalar effect ordering failed: {err}"),
                            )
                        })?;
                    }
                    let hsched = time_signal_fir_phase(timing_sink, "fir-scheduling", || {
                        crate::hgraph::schedule(&hgraph, options.scheduling_strategy)
                    })
                    .map_err(|err| {
                        SignalFirError::new(
                            SignalFirErrorCode::ClockAnalysis,
                            format!("clock-domain scheduling failed: {err}"),
                        )
                    })?;
                    gate_graphs = Some((hgraph, hsched));
                    Ok(Some(module::ClockedPlan { domains, envs }))
                }
                _ => {
                    // A program reaching this branch with an actual OD/US/DS wrapper
                    // node was compiled through a clock-unaware entry point (no
                    // `ClockDomainTable` was ever supplied): `clk_env::annotate`
                    // cannot resolve a real wrapper's clock relationship from an
                    // empty table, and would report a confusing
                    // `ClockedViolation`-family error instead of letting
                    // `module::build_module`'s own, specific `FRS-SFIR-0007`
                    // ("clocked node reached without a domain table") rejection
                    // fire, exactly as before this gate existed. So skip the gate
                    // for those; ordinary wrapper-free programs run it.
                    let has_wrapper =
                        crate::hgraph::contains_wrapper(prepared.arena(), prepared.outputs())
                            .map_err(|err| {
                                SignalFirError::new(
                                    SignalFirErrorCode::ClockAnalysis,
                                    format!("wrapper scan failed: {err}"),
                                )
                            })?;
                    if !has_wrapper {
                        let empty_domains = propagate::ClockDomainTable::new();
                        let envs = crate::clk_env::annotate(
                            prepared.arena(),
                            &empty_domains,
                            prepared.outputs(),
                        )
                        .map_err(|err| {
                            SignalFirError::new(
                                SignalFirErrorCode::ClockAnalysis,
                                format!("clock-environment inference failed: {err}"),
                            )
                        })?;
                        let mut hgraph = time_signal_fir_phase(timing_sink, "fir-hgraph", || {
                            crate::hgraph::build_hgraph(
                                prepared.arena(),
                                &empty_domains,
                                &envs,
                                prepared.outputs(),
                                prepared.sig_types_map(),
                            )
                        })
                        .map_err(|err| {
                            SignalFirError::new(
                                SignalFirErrorCode::ClockAnalysis,
                                format!("hierarchical dependency graph failed: {err}"),
                            )
                        })?;
                        if matches!(options.compute_mode, ComputeMode::Scalar) {
                            let effects =
                                time_signal_fir_phase(timing_sink, "fir-scalar-effects", || {
                                    vector::analysis::analyze_scalar_scheduling_effects(&prepared)
                                })
                                .map_err(|err| {
                                    SignalFirError::new(
                                        SignalFirErrorCode::ClockAnalysis,
                                        format!("scalar effect analysis failed: {err}"),
                                    )
                                })?;
                            time_signal_fir_phase(timing_sink, "fir-effect-orientation", || {
                                crate::hgraph::orient_effect_conflicts(&mut hgraph, &effects)
                            })
                            .map_err(|err| {
                                SignalFirError::new(
                                    SignalFirErrorCode::ClockAnalysis,
                                    format!("scalar effect ordering failed: {err}"),
                                )
                            })?;
                        }
                        let hsched = time_signal_fir_phase(timing_sink, "fir-scheduling", || {
                            crate::hgraph::schedule(&hgraph, options.scheduling_strategy)
                        })
                        .map_err(|err| {
                            SignalFirError::new(
                                SignalFirErrorCode::ClockAnalysis,
                                format!("clock-domain scheduling failed: {err}"),
                            )
                        })?;
                        gate_graphs = Some((hgraph, hsched));
                    }
                    Ok(None)
                }
            }
        },
    )?;

    let mut vector_fallback = None;
    if options.compute_mode.is_vector() {
        let empty_domains = propagate::ClockDomainTable::new();
        let domains = clock_domains.unwrap_or(&empty_domains);
        match time_signal_fir_phase(timing_sink, "fir-vector-certification", || {
            vector::module::build_verified_vector_module(
                &prepared,
                &vector::module::VectorModuleContext {
                    domains,
                    ui,
                    num_inputs: plan.num_inputs,
                    num_outputs: plan.num_outputs,
                    module_name: options.module_name.as_str(),
                    real_type: options.real_type.as_fir_type(),
                    max_copy_delay: options.max_copy_delay,
                    compute_mode: options.compute_mode,
                    strategy: options.scheduling_strategy,
                    control_rate_mode: options.control_rate_mode,
                },
            )
        }) {
            Ok(output) => return Ok(output),
            Err(failure) => vector_fallback = Some(failure),
        }
    }

    let fallback_compute_mode = if vector_fallback.is_some() {
        ComputeMode::Scalar
    } else {
        options.compute_mode
    };
    let mut output = time_signal_fir_phase(timing_sink, "fir-lowering", || {
        module::build_module(
            &plan,
            options.module_name.as_str(),
            prepared.arena(),
            prepared.outputs(),
            ui,
            prepared.types_map(),
            prepared.sig_types_map(),
            prepared.origins(),
            options.real_type.as_fir_type(),
            options.max_copy_delay,
            options.delay_line_threshold,
            fallback_compute_mode,
            options.control_rate_mode,
            options.processing_api,
            options.table_init_mode,
            options.scheduling_strategy,
            clocked,
            if matches!(fallback_compute_mode, ComputeMode::Scalar) {
                gate_graphs.as_ref().map(|(_, schedule)| schedule)
            } else {
                None
            },
            None,
        )
    })
    .map_err(|mut error| {
        error.attach_origins(prepared.origins());
        error
    })?;

    // The post-activation trace is intentionally off the hot path. When
    // explicitly requested, compare the accepted schedule and actual first
    // lowering order over the same prepared arena without changing the FIR.
    let shadow_report = build_shadow_report.then(|| {
        gate_graphs
            .as_ref()
            .map(|(hgraph, hsched)| shadow::compare_emission_order(hgraph, hsched, &output))
    });

    output.shadow_report = shadow_report.flatten();
    if let Some(failure) = vector_fallback {
        output.vector_pipeline_status = VectorPipelineStatus::Fallback(failure.reason);
        output.vector_pipeline_detail = Some(failure.detail);
    } else {
        output.vector_pipeline_status = VectorPipelineStatus::NotRequested;
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
