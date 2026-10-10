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
    tlib::on_compile_stack(|| {
        propagate_typed_with_ui_step(arena, box_tree, inputs, cache, ui_options)
    })
}

fn propagate_typed_with_ui_step(
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
        fold: false,
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

/// The caches of constant folding, kept from one fold to the next.
///
/// The evaluator folds an expression to a number at every pattern dispatch
/// and every box simplification (C++ `isBoxNumeric`). Built afresh for each
/// fold, these caches made a fold cost the size of the whole expression:
/// a recursion whose argument grows by one node per level, `f(n-1, x-1)` with
/// `x` a signal compared with a number, was quadratic (35 s at 4000 levels;
/// C++ 2.84.3, which memoizes on the trees, takes 0.1 s). Kept for one
/// evaluation pass, they make a fold cost what is new in the expression.
///
/// Every entry is a function of hash-consed trees, which do not change
/// during the pass. The session is for folds only (see [`propagate_fold`]):
/// its widget signals carry no real control id, so its results must never
/// reach a real propagation.
pub struct FoldSession {
    cache: ArityCache,
    memo: PropagateMemo,
    slot_env: SlotEnv,
    ui_path: UiPathContext,
    clock_domains: ClockDomainTable,
    validated: ahash::AHashSet<FlatBoxId>,
    control_ids: ControlIds,
}

impl Default for FoldSession {
    fn default() -> Self {
        Self {
            cache: ArityCache::default(),
            memo: PropagateMemo::default(),
            slot_env: SlotEnv::default(),
            ui_path: UiPathContext::default(),
            clock_domains: ClockDomainTable::default(),
            validated: ahash::AHashSet::new(),
            control_ids: ControlIds::new(),
        }
    }
}

impl Clone for FoldSession {
    /// A clone starts with empty caches: a cache is never needed for
    /// correctness.
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for FoldSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoldSession")
            .field("validated", &self.validated.len())
            .finish_non_exhaustive()
    }
}

/// Propagates a box with no input for constant folding, with the caches of
/// `session`, and returns its output signals.
///
/// The same propagation as [`propagate_typed`] with no input, except that no
/// UI program is built and widgets get a placeholder control id: a fold only
/// asks whether the outputs are numbers. Validation, arity, free slots and
/// propagation results carry over from one call to the next within
/// `session`, so folding `x - 1` after `x` propagates one node.
///
/// # Errors
/// A malformed flat box, or a propagation failure, as [`propagate_typed`].
pub fn propagate_fold(
    arena: &mut TreeArena,
    box_tree: BoxId,
    session: &mut FoldSession,
) -> Result<Vec<SigId>, PropagateError> {
    let flat = crate::flat::try_build_flat_box_validated(arena, box_tree, &mut session.validated)?;
    // A box with inputs is no constant: refused before propagation touches
    // the caches. Box simplification asks this of every sub-box.
    let arity = box_arity_typed(arena, flat, &mut session.cache)?;
    if arity.inputs != 0 {
        return Err(PropagateError::InputArityMismatch {
            node: box_tree,
            expected: arity.inputs,
            got: 0,
        });
    }
    // The result memo is always safe for a fold: no node kind disqualifies a
    // root today (`result_memo_is_safe_root`), and the validation above has
    // reported malformed boxes.
    session.memo.results.set_enabled(true);
    let mut signal_origins = SignalOrigins::disabled();
    // the path is moved in and out: `PropagateContext` owns it
    let ui_path = std::mem::take(&mut session.ui_path);
    let mut ctx = PropagateContext {
        cache: &mut session.cache,
        control_ids: &session.control_ids,
        slot_env: &mut session.slot_env,
        memo: &mut session.memo,
        clock_domains: &mut session.clock_domains,
        clock_env: arena.nil(),
        clock_domain: None,
        suppress_fad: false,
        pending_fad_seeds: Vec::new(),
        ui_path,
        signal_origins: &mut signal_origins,
        fold: true,
    };
    let signals = propagate_in_slot_env(arena, flat, &[], &mut ctx);
    session.ui_path = std::mem::take(&mut ctx.ui_path);
    if signals.is_err() {
        // a failure can leave bindings in the slot environment and groups in
        // the path: start the next fold from empty caches
        *session = FoldSession::default();
    }
    signals
}
