//! Backend-independent compilation of one `SIGGEN` payload into a sub-module.
//!
//! Split out of [`super::subcontainer`] so the checked vector lowerer can build
//! the *same* sub-module the scalar lowerer builds, rather than keeping a
//! second, folding-only implementation of table initialization
//! (`porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`, S6).
//!
//! The generator itself is never vectorized. It is a 0-input / 1-output program
//! evaluated once at initialization, so compiling it in scalar mode is correct
//! under `-vec` as well — and it is what keeps scalar and vector output
//! byte-identical in the sub-module, which the emission-determinism gate can
//! then compare.

use fir::{FirId, FirStore, FirType};
use signals::{SigId, SigMatch, match_sig};
use tlib::TreeArena;
use ui::UiProgram;

use super::build::FillSpec;
use super::{SignalFirError, SignalFirErrorCode};
use crate::schedule::SchedulingStrategy;
use crate::signal_fir::TableInitMode;

/// Everything the sub-module compiler needs that is not the generator itself.
///
/// These are all inherited from the enclosing program: a generator compiled
/// with a different real type or delay policy than its parent would produce a
/// table the parent cannot read correctly.
pub(crate) struct GeneratorSubModuleSpec<'a> {
    /// Sub-module class name, `{module}SIG{k}`, allocated by the caller so the
    /// scalar and vector lowerers keep their own counters.
    pub name: &'a str,
    /// Element type of the table being filled; becomes the `fill` signature's
    /// array type.
    pub elem_ty: FirType,
    pub real_ty: FirType,
    pub max_copy_delay: u32,
    pub delay_line_threshold: u32,
    pub table_init_mode: TableInitMode,
    pub table_init_sample_rate: Option<i32>,
    pub scheduling_strategy: SchedulingStrategy,
}

/// Compiles one `SIGGEN` payload into a sub-module interned in `store`.
///
/// Returns the `SubModule` node id, already imported into the caller's store —
/// `FirId`s are store-local, and the generator is lowered into a store of its
/// own.
pub(crate) fn compile_generator_sub_module(
    arena: &TreeArena,
    store: &mut FirStore,
    generator: SigId,
    spec: &GeneratorSubModuleSpec<'_>,
) -> Result<FirId, SignalFirError> {
    let payload = match match_sig(arena, generator) {
        SigMatch::Gen(inner) => inner,
        _ => generator,
    };

    // The generator is prepared exactly like the main program, so the
    // interpreter path and this path see the same normalized shape. This is the
    // same call `siggen::interpret_generator` already makes.
    let prepared = crate::signal_prepare::prepare_signals_for_fir_verified(
        arena,
        &[payload],
        &UiProgram::empty(),
    )
    .map_err(|err| {
        SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!("table generator preparation failed: {err}"),
        )
    })?;

    let outputs = prepared.outputs();
    if outputs.len() != 1 {
        return Err(SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!(
                "table generator must have exactly one output, got {}",
                outputs.len()
            ),
        ));
    }

    let plan = crate::signal_fir::planner::SignalFirPlan {
        num_inputs: 0,
        num_outputs: 1,
        signal_count: outputs.len(),
    };
    let fill = FillSpec {
        name: spec.name.to_owned(),
        elem_ty: spec.elem_ty.clone(),
    };

    // A generator is scheduled like any other program. Skipping this was a
    // correctness bug, not an optimization: `build_module` drives
    // recursion-group emission through `lower_scheduled_graph`, which no-ops
    // without a schedule, so a carrier read only through a delay never got its
    // update emitted and every table entry kept the initial value. Generators
    // carry no clock domains, so this is the wrapper-free branch of the main
    // gate.
    let empty_domains = propagate::ClockDomainTable::new();
    let envs =
        crate::clk_env::annotate(prepared.arena(), &empty_domains, outputs).map_err(|err| {
            SignalFirError::new(
                SignalFirErrorCode::ClockAnalysis,
                format!("table generator clock-environment inference failed: {err}"),
            )
        })?;
    let mut hgraph = crate::hgraph::build_hgraph(
        prepared.arena(),
        &empty_domains,
        &envs,
        outputs,
        prepared.sig_types_map(),
    )
    .map_err(|err| {
        SignalFirError::new(
            SignalFirErrorCode::ClockAnalysis,
            format!("table generator dependency graph failed: {err}"),
        )
    })?;
    let effects = crate::signal_fir::vector::analysis::analyze_scalar_scheduling_effects(&prepared)
        .map_err(|err| {
            SignalFirError::new(
                SignalFirErrorCode::ClockAnalysis,
                format!("table generator effect analysis failed: {err}"),
            )
        })?;
    crate::hgraph::orient_effect_conflicts(&mut hgraph, &effects).map_err(|err| {
        SignalFirError::new(
            SignalFirErrorCode::ClockAnalysis,
            format!("table generator effect ordering failed: {err}"),
        )
    })?;
    let hsched = crate::hgraph::schedule(&hgraph, spec.scheduling_strategy).map_err(|err| {
        SignalFirError::new(
            SignalFirErrorCode::ClockAnalysis,
            format!("table generator scheduling failed: {err}"),
        )
    })?;

    let empty_ui = UiProgram::empty();
    let lowered = super::build::build_module(
        &plan,
        spec.name,
        prepared.arena(),
        outputs,
        &empty_ui,
        prepared.types_map(),
        prepared.sig_types_map(),
        prepared.origins(),
        spec.real_ty.clone(),
        spec.max_copy_delay,
        spec.delay_line_threshold,
        crate::signal_fir::ComputeMode::Scalar,
        crate::signal_fir::ControlRateMode::InlinePerBlock,
        crate::signal_fir::ProcessingApi::Block,
        spec.table_init_mode,
        spec.table_init_sample_rate,
        spec.scheduling_strategy,
        None,
        Some(&hsched),
        Some(&fill),
    )?;

    Ok(store.import_from(&lowered.store, lowered.module))
}
