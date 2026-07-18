//! Public propagation entry points.
//!
//! This module contains the typed box-to-signal APIs exposed by the crate
//! facade. Callers enter here after the `eval/a2sb` boundary has produced a
//! validated [`FlatBoxId`]; implementation details remain in `engine`,
//! `arity`, and `ui_build`.

use super::*;

/// Propagates input signals and grouped UI through one validated flat box expression.
///
/// This is the typed entry point for callers that already crossed the
/// `eval/a2sb` flat-box boundary and want the full propagation products:
/// propagated DSP signals plus canonical grouped UI ownership.
///
/// AD parity note:
/// - when `box_tree` is `fad(expr)`, the returned `signals` list is expanded to
///   `primal outputs + one tangent bundle per enabled control`,
/// - enabled controls come from the canonical UI registry and honor
///   `[autodiff:false]`,
/// - `rad(expr)` returns [`PropagateError::RadUnsupportedNode`] for unsupported
///   signal shapes.
pub fn propagate_typed_with_ui(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
) -> Result<PropagateOutput, PropagateError> {
    propagate_typed_with_ui_options(
        arena,
        box_tree,
        inputs,
        cache,
        &PropagateUiOptions::default(),
    )
}

/// Propagates input signals and grouped UI through one validated flat box expression
/// using explicit grouped-UI construction options.
pub fn propagate_typed_with_ui_options(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
    ui_options: &PropagateUiOptions,
) -> Result<PropagateOutput, PropagateError> {
    let ui = build_ui_program(arena, box_tree, ui_options);
    let mut slot_env = SlotEnv::new();
    let mut memo = PropagateMemo::default();
    let mut clock_domains = ClockDomainTable::new();
    let mut ctx = PropagateContext {
        cache,
        control_ids: &ui.control_ids,
        slot_env: &mut slot_env,
        memo: &mut memo,
        clock_domains: &mut clock_domains,
        clock_env: arena.nil(),
        clock_domain: None,
        suppress_fad: false,
        pending_fad_seeds: Vec::new(),
        current_groups: Vec::new(),
    };
    let signals = propagate_in_slot_env(arena, box_tree, inputs, &mut ctx)?;
    Ok(PropagateOutput {
        signals,
        ui: ui.program,
        clock_domains,
    })
}

/// Propagates input signals through one validated flat box expression (memoized arity).
///
/// Compatibility wrapper for callers that only consume DSP signal outputs. New
/// post-`eval/a2sb` callers that own grouped UI should prefer
/// [`propagate_typed_with_ui`].
pub fn propagate_typed(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
) -> Result<Vec<SigId>, PropagateError> {
    propagate_typed_with_ui(arena, box_tree, inputs, cache).map(|output| output.signals)
}
