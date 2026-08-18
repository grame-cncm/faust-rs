//! Public propagation entry points.
//!
//! This module contains the typed box-to-signal APIs exposed by the crate
//! facade. Callers enter here after the `eval/a2sb` boundary has produced a
//! validated [`FlatBoxId`]; implementation details remain in `engine`,
//! `arity`, and `ui_build`.

use super::*;
use crate::context_id::{SlotEnv, UiPathContext};

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
///
/// Grouped-UI construction is configured by `ui_options`;
/// [`PropagateUiOptions::default`] is the ordinary choice.
pub fn propagate_typed_with_ui(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
    ui_options: &PropagateUiOptions,
) -> Result<PropagateOutput, PropagateError> {
    propagate_typed_with_origins_policy(
        arena,
        box_tree,
        inputs,
        cache,
        ui_options,
        SignalOrigins::default(),
    )
}

/// Propagation core, parameterized by whether Box provenance is accumulated.
///
/// Passing [`SignalOrigins::disabled`] removes the per-box provenance forest
/// walk entirely; the returned `signal_origins` is then empty by construction.
/// Only use it for callers that provably discard the table.
fn propagate_typed_with_origins_policy(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
    ui_options: &PropagateUiOptions,
    mut signal_origins: SignalOrigins,
) -> Result<PropagateOutput, PropagateError> {
    let ui = build_ui_program(arena, box_tree, ui_options);
    let mut slot_env = SlotEnv::new();
    let mut memo = PropagateMemo::default();
    memo.results
        .set_enabled(crate::result_memo::result_memo_is_safe_root(
            arena, box_tree,
        )?);
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
        ui_path: UiPathContext::new(),
        signal_origins: &mut signal_origins,
    };
    let signals = propagate_in_slot_env(arena, box_tree, inputs, &mut ctx);
    ctx.memo.profile.print();
    let signals = signals?;
    Ok(PropagateOutput {
        signals,
        signal_origins,
        ui: ui.program,
        clock_domains,
    })
}

/// Propagates input signals through one validated flat box expression (memoized arity).
///
/// Compatibility wrapper for callers that only consume DSP signal outputs. New
/// post-`eval/a2sb` callers that own grouped UI should prefer
/// [`propagate_typed_with_ui`].
///
/// Because the returned value cannot expose provenance, this entry point
/// propagates with recording disabled: accumulating a table the caller has no
/// way to read was pure cost. `eval` reaches this path for every constant fold
/// (`crates/eval/src/simplify.rs`, the C++ `boxPropagateSig` equivalent), which
/// made evaluation pay one full provenance forest walk per folded expression.
pub fn propagate_typed(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
) -> Result<Vec<SigId>, PropagateError> {
    propagate_typed_with_origins_policy(
        arena,
        box_tree,
        inputs,
        cache,
        &PropagateUiOptions::default(),
        SignalOrigins::disabled(),
    )
    .map(|output| output.signals)
}
