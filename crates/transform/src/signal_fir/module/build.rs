//! FIR module assembly — `build_module` entry point.
//!
//! Defines [`RadReverseState`], the sub-state struct for RAD reverse-time
//! scheduling that is populated post-construction in `build_module`.
//!
//! Owns the single crate-visible function [`build_module`] that accepts
//! pre-validated planning data and a prepared signal forest and assembles a
//! self-contained [`SignalFirOutput`] with all Faust lifecycle sections in
//! deterministic order: `metadata`, `instanceConstants`,
//! `instanceResetUserInterface`, `instanceClear`, `buildUserInterface`,
//! and `compute`.
//!
//! All other submodules in `module/` provide `impl SignalToFirLower` methods
//! that are invoked from the orchestration logic here.

use ahash::AHashMap;

use super::*;
use crate::signal_fir::ComputeMode;
use crate::signal_fir::loop_graph::LoopKind;

/// RAD reverse-time scheduling state, populated post-construction in `build_module`.
#[derive(Default)]
pub(super) struct RadReverseState {
    /// Forward output lanes already computed before the reverse-time loop.
    ///
    /// Phase-E1 RAD uses the public bundle layout `[primals..., gradients...]`.
    /// This map lets coefficient-gradient terms in the reverse loop replay
    /// `Delay1(primal)` from the primal output buffer instead of reading the
    /// recursion carrier in reverse-time order.
    pub(super) forward_output_by_sig: HashMap<SigId, usize>,
    /// Same map as [`Self::forward_output_by_sig`], keyed by the prepared
    /// readable signal shape to survive equivalent but non-identical `SigId`s.
    pub(super) forward_output_by_sig_key: HashMap<String, usize>,
    /// True while lowering the reverse-time sample-loop slice.
    pub(super) lowering_reverse_loop: bool,
}

/// Emits one sample-loop slice's statements. Scalar mode — and any reverse-time
/// loop — stays a single `for (i0 = 0; i0 < count; i0++)`. Vector mode (`-vec`)
/// restructures a forward slice into a chunk driver (roadmap P6, V5 / S-D):
///
/// - a **`Vectorizable`** (state-free) slice is chunked whole;
/// - a **`Recursive`** slice is *split* when possible (vector doc §5 S-D): its
///   state-free tail is hoisted into a second, vectorizable inner loop fed by
///   chunk buffers, leaving only the recursive core serial — otherwise it stays
///   one plain serial loop (chunking a loop-carried body as one block is pure
///   overhead the C compiler cannot vectorize);
/// - a **`Island`** (clocked scalar domain) stays a plain serial loop.
///
/// The chunk driver has two layouts, selected by `-lv` (`loop_variant`, as Faust
/// C++), both bit-exact vs scalar (the inner loop keeps the *global* sample index
/// `i0`, no I/O rebasing):
///
/// ```text
/// -lv 1 "simple"                       -lv 0 "fastest" (default)
/// for (vindex=0; vindex<count; +=vs) { int vlimit = count - count % vs;
///   int vend = min(vindex+vs, count);  for (vindex=0; vindex<vlimit; +=vs)
///   for (i0=vindex; i0<vend; i0++)…      for (i0=vindex; i0<vindex+vs; i0++)…  // const trip
/// }                                    int vindex = vlimit;                     // remainder
///                                      for (i0=vindex; i0<count; i0++)…
/// ```
///
/// The fastest variant's constant inner trip count (`vindex + vs`) is what lets
/// the C compiler fully unroll / SIMD the (vectorizable) inner loops; the simple
/// variant's runtime `min` bound is easier to read but vectorizes less well.
/// Reverse-time loops force scalar mode (chunking would change the implicit TBPTT
/// window from `count` to `vec_size`, vector doc §5).
///
/// Returns the slice's statements: for the split path, the chunk-buffer
/// declarations followed by the chunk driver; otherwise one loop.
fn emit_sample_loop(
    store: &mut FirStore,
    exec: &[FirId],
    is_reverse: bool,
    kind: LoopKind,
    compute_mode: ComputeMode,
) -> Vec<FirId> {
    let (vec_size, variant) = match compute_mode {
        // Reverse-time and scalar mode: one plain serial loop.
        ComputeMode::Vector {
            vec_size,
            loop_variant,
        } if !is_reverse => (vec_size.max(1), loop_variant),
        _ => return vec![plain_sample_loop(store, exec, is_reverse)],
    };
    let vs = i32::try_from(vec_size).unwrap_or(i32::MAX);

    match kind {
        // State-free slice: chunk the whole body (one inner loop per chunk).
        LoopKind::Vectorizable => build_chunk_driver(store, &[exec.to_vec()], vs, variant),
        // Recursive slice: split off the state-free tail when possible.
        LoopKind::Recursive => match split_bodies(store, exec, vec_size) {
            Some((serial_body, tail_body, buf_decls)) => {
                let mut out = buf_decls;
                out.extend(build_chunk_driver(
                    store,
                    &[serial_body, tail_body],
                    vs,
                    variant,
                ));
                out
            }
            None => vec![plain_sample_loop(store, exec, false)],
        },
        // Clocked scalar island (vector doc §6 D1): plain serial loop.
        LoopKind::Island => vec![plain_sample_loop(store, exec, false)],
    }
}

/// A single `for (i0 = 0; i0 < count; i0++) { <exec> }` (scalar / reverse / the
/// non-splittable-recursive and island fallbacks).
fn plain_sample_loop(store: &mut FirStore, exec: &[FirId], is_reverse: bool) -> FirId {
    let mut b = FirBuilder::new(store);
    let upper = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let body = b.block(exec);
    b.simple_for_loop("i0", upper, body, is_reverse)
}

/// Splits a recursive slice into `(serial core + buffer stores, rewritten tail,
/// chunk-buffer declarations)` (vector doc §5 S-D), or `None` when the body is
/// not splittable (→ one plain serial loop). The serial core buffers each
/// boundary carrier into `vbufN[i0 - vindex]`; the tail reads it back.
///
/// Bit-exact: the serial core runs the whole chunk first (state evolves exactly
/// as in the fused loop), then the tail reads the buffered values at the same
/// global `i0`.
fn split_bodies(
    store: &mut FirStore,
    exec: &[FirId],
    vec_size: u32,
) -> Option<(Vec<FirId>, Vec<FirId>, Vec<FirId>)> {
    use crate::signal_fir::loop_graph::{ChunkBuffer, partition_recursive_body, rewrite_var_loads};

    let part = partition_recursive_body(store, exec)?;

    // A chunk buffer per boundary temp (deterministic vbufN numbering).
    let buffers: Vec<ChunkBuffer> = part
        .boundary
        .iter()
        .enumerate()
        .map(|(i, (_, ty))| {
            ChunkBuffer::new(u32::try_from(i).unwrap_or(u32::MAX), ty.clone(), vec_size)
        })
        .collect();
    let buf_decls: Vec<FirId> = buffers.iter().map(|buf| buf.declare(store)).collect();

    // Serial inner body = serial core + `vbufN[i0 - vindex] = temp`.
    let mut serial_body = part.serial.clone();
    for (buf, (name, ty)) in buffers.iter().zip(&part.boundary) {
        let val = FirBuilder::new(store).load_var(name.clone(), AccessType::Stack, ty.clone());
        serial_body.push(buf.store(store, val));
    }

    // Tail inner body = tail statements with boundary reads → buffer loads.
    let mut repl = AHashMap::new();
    for (buf, (name, _)) in buffers.iter().zip(&part.boundary) {
        let load = buf.load(store);
        repl.insert(name.clone(), load);
    }
    let tail_body: Vec<FirId> = part
        .vectorizable
        .iter()
        .map(|&s| rewrite_var_loads(store, s, &repl))
        .collect::<Option<_>>()?;

    Some((serial_body, tail_body, buf_decls))
}

/// Builds the chunk driver wrapping one inner loop per `body` (in order), in the
/// requested loop variant (`-lv`). Returns the driver's top-level statements.
fn build_chunk_driver(
    store: &mut FirStore,
    bodies: &[Vec<FirId>],
    vs: i32,
    loop_variant: u8,
) -> Vec<FirId> {
    if loop_variant == 1 {
        vec![build_simple_driver(store, bodies, vs)]
    } else {
        build_fastest_driver(store, bodies, vs)
    }
}

/// One `for (i0 = start; i0 < bound; i0++) { body }` per `body`, in order. Each
/// loop reads the global `i0`; `start`/`bound` are shared interned expressions.
fn chunk_inner_loops(
    store: &mut FirStore,
    bodies: &[Vec<FirId>],
    start: FirId,
    bound: FirId,
) -> Vec<FirId> {
    bodies
        .iter()
        .map(|body| {
            let mut b = FirBuilder::new(store);
            let init = b.declare_var("i0", FirType::Int32, AccessType::Loop, Some(start));
            let step = b.int32(1);
            let blk = b.block(body);
            b.for_loop("i0", init, bound, step, blk, false)
        })
        .collect()
}

/// `-lv 1` "simple": one outer loop with a runtime `vend = min(vindex+vs, count)`.
fn build_simple_driver(store: &mut FirStore, bodies: &[Vec<FirId>], vs: i32) -> FirId {
    // vend = (vindex + vs < count) ? vindex + vs : count.
    let (vend_decl, start, bound) = {
        let mut b = FirBuilder::new(store);
        let vindex_r = b.load_var("vindex", AccessType::Loop, FirType::Int32);
        let vsz = b.int32(vs);
        let next = b.binop(FirBinOp::Add, vindex_r, vsz, FirType::Int32);
        let count_hi = b.load_var("count", AccessType::FunArgs, FirType::Int32);
        let cond = b.binop(FirBinOp::Lt, next, count_hi, FirType::Int32);
        let vend_val = b.select2(cond, next, count_hi, FirType::Int32);
        let vend_decl = b.declare_var("vend", FirType::Int32, AccessType::Stack, Some(vend_val));
        let start = b.load_var("vindex", AccessType::Loop, FirType::Int32);
        let bound = b.load_var("vend", AccessType::Stack, FirType::Int32);
        (vend_decl, start, bound)
    };
    let mut outer_stmts = vec![vend_decl];
    outer_stmts.extend(chunk_inner_loops(store, bodies, start, bound));

    let mut b = FirBuilder::new(store);
    let outer_body = b.block(&outer_stmts);
    let zero = b.int32(0);
    let vindex_init = b.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(zero));
    let count_end = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let vsz_step = b.int32(vs);
    b.for_loop(
        "vindex",
        vindex_init,
        count_end,
        vsz_step,
        outer_body,
        false,
    )
}

/// `-lv 0` "fastest": a constant-trip main loop over `[0, vlimit)` (inner bound
/// `vindex + vs`, so the C compiler proves the trip count) plus a scalar remainder
/// over `[vlimit, count)`, where `vlimit = count - count % vs`.
///
/// Both loops are wrapped in an `if` guard so an *empty* range is never entered:
/// the main loop is skipped when `count < vs` (`vlimit == 0`), the remainder when
/// `vs` divides `count` (`vlimit == count`). This matters because the interpreter
/// runs a loop body once before its first condition check (a `do/while`), so an
/// empty chunk would read past the block; the C `for` would be fine, but the guard
/// keeps both backends identical.
///
/// `vlimit` is an inline reused expression, not a named variable, so multiple
/// sample-loop slices never collide, and the remainder's `int vindex = vlimit`
/// (needed for `vbufN[i0 - vindex]`) is scoped inside its `if` block.
fn build_fastest_driver(store: &mut FirStore, bodies: &[Vec<FirId>], vs: i32) -> Vec<FirId> {
    // vlimit = count - count % vs, a reusable interned expression.
    let vlimit = {
        let mut b = FirBuilder::new(store);
        let count1 = b.load_var("count", AccessType::FunArgs, FirType::Int32);
        let vsz1 = b.int32(vs);
        let rem = b.binop(FirBinOp::Rem, count1, vsz1, FirType::Int32);
        let count2 = b.load_var("count", AccessType::FunArgs, FirType::Int32);
        b.binop(FirBinOp::Sub, count2, rem, FirType::Int32)
    };

    // Main loop: for (vindex = 0; vindex < vlimit; vindex += vs), inner bound
    // vindex + vs (constant trip count), guarded by `vlimit > 0`.
    let (start_m, bound_m) = {
        let mut b = FirBuilder::new(store);
        let start_m = b.load_var("vindex", AccessType::Loop, FirType::Int32);
        let vindex_b = b.load_var("vindex", AccessType::Loop, FirType::Int32);
        let vsz_b = b.int32(vs);
        let bound_m = b.binop(FirBinOp::Add, vindex_b, vsz_b, FirType::Int32);
        (start_m, bound_m)
    };
    let main_stmts = chunk_inner_loops(store, bodies, start_m, bound_m);
    let main_guarded = {
        let mut b = FirBuilder::new(store);
        let main_body = b.block(&main_stmts);
        let zero = b.int32(0);
        let vindex_init = b.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(zero));
        let vsz_step = b.int32(vs);
        let main_loop = b.for_loop("vindex", vindex_init, vlimit, vsz_step, main_body, false);
        let zero_c = b.int32(0);
        let cond = b.binop(FirBinOp::Gt, vlimit, zero_c, FirType::Int32);
        let then_blk = b.block(&[main_loop]);
        b.if_(cond, then_blk, None)
    };

    // Remainder: if (vlimit < count) { int vindex = vlimit; for i0=vindex..count }
    let (start_r, bound_r) = {
        let mut b = FirBuilder::new(store);
        let start_r = b.load_var("vindex", AccessType::Loop, FirType::Int32);
        let bound_r = b.load_var("count", AccessType::FunArgs, FirType::Int32);
        (start_r, bound_r)
    };
    let rem_inner = chunk_inner_loops(store, bodies, start_r, bound_r);
    let rem_guarded = {
        let mut b = FirBuilder::new(store);
        let vindex_decl = b.declare_var("vindex", FirType::Int32, AccessType::Loop, Some(vlimit));
        let mut rem_stmts = vec![vindex_decl];
        rem_stmts.extend(rem_inner);
        let rem_body = b.block(&rem_stmts);
        let count_hi = b.load_var("count", AccessType::FunArgs, FirType::Int32);
        let cond = b.binop(FirBinOp::Lt, vlimit, count_hi, FirType::Int32);
        b.if_(cond, rem_body, None)
    };

    vec![main_guarded, rem_guarded]
}

/// Lowers a prepared signal forest into a complete FIR module.
///
/// Entry point for the fast-lane Step 2A–2G boundary: accepts pre-validated
/// planning data and a prepared signal forest, returns a [`SignalFirOutput`]
/// with all Faust lifecycle sections (`metadata`, `instanceConstants`,
/// `instanceResetUserInterface`, `instanceClear`, `buildUserInterface`,
/// `compute`) assembled in deterministic order.
///
/// # Promotion invariant
///
/// The `signals` forest **must** have been processed by
/// `signal_prepare::promote_signals_for_fir` (and optionally
/// `normalize::simplify`) before being passed here.  That pass guarantees:
///
/// - Every `BinOp(op, lhs, rhs)` node has operands whose signal domain
///   types are already consistent with `op`: mixed Int/Real operands are
///   wrapped in explicit `FloatCast` nodes; bitwise/shift operands in
///   `IntCast` nodes; `Div` operands are always Real.
/// - Every `Delay(_, amount)`, `RdTbl(_, index)`, `WrTbl(…, widx, _)`,
///   `Select2(selector, …)`, and `Enable(_, gate)` has its integer-context
///   operand wrapped in `IntCast`.
/// - `Delay1(x)` and `Prefix(init, x)` have `type(init) == type(x)`.
///
/// **Consequence for the lowerer**: no implicit coercion is needed inside
/// `lower_binop`, `lower_delay_state`, or `normalized_table_index`.  All
/// necessary casts appear as explicit signal-tree nodes and are handled by
/// `lower_cast` when the lowerer dispatches on `SigMatch::IntCast /
/// FloatCast`.
///
/// BRA tape lowering relies on the same invariant.  It does not run a second
/// promotion pass over synthesized `fBraTapeN` stores.  If the signal graph
/// contains an integer/discrete subgraph that feeds a real expression through a
/// `FloatCast` (for example an LCG noise recursion multiplied by a real scale),
/// the cast node is the promoted real boundary.  The integer nodes upstream of
/// that cast keep their integer semantics and are not valid real tape values.
///
/// # Recursion Boundary
///
/// Most recursion-specific mechanics now live in `recursion.rs`:
///
/// - recursion carrier/state data types
/// - active/materialized carrier resolution
/// - delayed recursion reference resolution
/// - recursive-group projection decoding/validation
/// - recursion carrier allocation helpers
/// - recursion-specific FIR helper emission
///
/// `module/` remains responsible for orchestration:
///
/// - `lower_signal(...)` dispatch
/// - deciding when a top-level recursion group must be materialized
/// - evaluating recursive body expressions
/// - integrating recursion writes/finalization into the sample phases
///
/// # Recursion and delay1 coupling
///
/// Recursion outputs can be consumed through delay chains rooted at
/// `Proj(i, group)`, not only through the immediate feedback form
/// `Delay1(Proj(i, group))`.
///
/// The lowering path now resolves `Delay1^k(Proj(...))` through
/// `resolve_recursion_delay_ref` and reuses the group's existing recursion
/// carrier instead of allocating a separate delay-state slot. For scalar
/// carriers this reads the previous-sample struct field directly. For size-2
/// carriers, this preserves the direct two-slot fast path; for larger carriers,
/// reads use the preplanned circular recursion array sized from accumulated
/// delay analysis.
///
/// This is why two separate state spaces exist:
///
/// - `state_name_by_node`: standalone non-recursive delay-state slots keyed by
///   delay node
/// - `self.recursion`: recursion carriers keyed by `(group, body index)`
///
/// They must never alias, even when the body signal of a recursion group
/// happens to be the same `SigId` as a `Delay1` node (the tf22 regression
/// pattern).
///
/// # Parameters
///
/// - `plan` – pre-checked I/O counts and signal statistics.
/// - `types` – per-signal [`SimpleSigType`] from `signal_prepare`; drives
///   integer-vs-real decisions for state/table element types.
/// - `sig_types` – full type-annotator map; used only for interval-based
///   variable delay sizing via [`sigtype::check_delay_interval`].
/// - `real_ty` – internal computation type (`Float32` or `Float64`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_module<'a>(
    plan: &SignalFirPlan,
    module_name: &str,
    arena: &'a TreeArena,
    signals: &[SigId],
    ui: &'a UiProgram,
    types: &'a HashMap<SigId, SimpleSigType>,
    sig_types: &'a HashMap<SigId, SigType>,
    real_ty: FirType,
    max_copy_delay: u32,
    delay_line_threshold: u32,
    compute_mode: ComputeMode,
    clocked: Option<clocked::ClockedPlan<'a>>,
    scalar_schedule: Option<&crate::hgraph::Hsched>,
) -> Result<SignalFirOutput, SignalFirError> {
    let delay_opts = DelayOptions {
        max_copy_delay,
        delay_line_threshold,
    };
    let (sig_ref_counts, sig_at_boundary, konst_escapes) =
        analyze_signal_sharing(arena, signals, sig_types);
    let placement = setup::PlacementInfo::new(sig_ref_counts, sig_at_boundary, konst_escapes);
    let mut lower = SignalToFirLower::new(
        arena,
        ui,
        types,
        sig_types,
        plan.num_inputs,
        real_ty,
        placement,
        delay_opts,
    );
    lower.clocked = clocked.map(clocked::ClockedState::new);
    lower.scalar_schedule = scalar_schedule.cloned();
    lower.fixed_ad_internal_signals = fixed_ad_internal_signals(lower.arena, signals);
    lower.register_symbolic_recursion_groups(signals)?;
    if lower.clocked.is_some() && lower.scalar_schedule.is_some() {
        lower.prepare_clocked_payload_schedule(signals);
    }
    lower.ensure_sample_rate_var();
    lower.prepare_delay_lines(signals)?;
    lower.assign_clocked_delay_cursors()?;
    let reverse_time_outputs = classify_reverse_time_outputs(lower.arena, signals);
    lower.rad_reverse.forward_output_by_sig = signals
        .iter()
        .enumerate()
        .filter_map(|(index, &sig)| (!reverse_time_outputs[index]).then_some((sig, index)))
        .collect();
    let dsp_arg_type = FirType::Ptr(Box::new(FirType::Obj));
    let dsp_arg = NamedType {
        name: "dsp".to_string(),
        typ: dsp_arg_type.clone(),
    };

    {
        let mut b = FirBuilder::new(&mut lower.store);
        lower
            .sections
            .control_statements
            .push(b.label("signal_fir_fastlane_step2a: executable base slice"));
        lower.sections.control_statements.push(b.label(format!(
            "io: inputs={} outputs={}",
            plan.num_inputs, plan.num_outputs
        )));
        lower
            .sections
            .control_statements
            .push(b.label(format!("signals: {}", plan.signal_count)));
    }

    let has_forward_outputs = reverse_time_outputs.iter().any(|is_reverse| !*is_reverse);
    let has_reverse_outputs = reverse_time_outputs.iter().any(|is_reverse| *is_reverse);
    if has_reverse_outputs {
        lower.scalar_schedule = None;
        // Readable structural fallback keys are only needed when the RAD
        // reverse-time loop must reconnect a delayed value to a forward output.
        lower.rad_reverse.forward_output_by_sig_key = signals
            .iter()
            .enumerate()
            .filter_map(|(index, &sig)| {
                (!reverse_time_outputs[index]).then_some((dump_sig_readable(arena, sig), index))
            })
            .collect();
    }
    let mut sample_loops = Vec::new();

    // Reverse AD owns a fixed forward/reverse epoch split and is deliberately
    // outside P3's flat same-tick Hgraph. P6 keeps that driver authoritative.
    // Every ordinary scalar forward program, including clock islands, is
    // previsited through the selected hierarchical schedule.
    if !has_reverse_outputs {
        lower.lower_scheduled_graph(crate::hgraph::GraphKey::Control)?;
        lower.lower_scheduled_graph(crate::hgraph::GraphKey::Top)?;
    }

    if has_forward_outputs {
        // Forward loop slice.  This is not necessarily "primal only": when a
        // BRA gradient projection is consumed inside a forward-time expression
        // (for example `p_next = p - lr * grad_p` inside a recursion body),
        // `lower_output_signal` can descend into that expression and call
        // `ensure_bra_backward_sweep`.  In that case the BRA adjoint statements
        // are appended to this same forward sample phase, and no separate
        // public backward loop is required unless another top-level output was
        // classified as reverse-time below.
        for (signal_index, sig) in signals.iter().enumerate() {
            if !reverse_time_outputs[signal_index] {
                lower.lower_output_signal(signal_index, *sig, plan.num_outputs)?;
            }
        }
        lower.finalize_global_cursor();
        let delay_sample_end = lower
            .delay
            .emit_sample_end_updates(&mut lower.store, lower.uses_iota);
        lower
            .regions
            .current_phases_mut()
            .sample_end
            .extend(delay_sample_end);
        sample_loops.push((false, lower.regions.current_flattened()));
        lower.reset_sample_loop_state(region::RegionKind::ReverseSampleLoop);
    }

    if has_reverse_outputs {
        // Reverse loop slice for public reverse-time outputs.  This path is
        // used when the public bundle contains gradient projections, such as
        // `process = rad(loss, params)`.  Adaptive DSPs may skip this block
        // entirely: their gradient projection can be internal to the forward
        // update and therefore scheduled by the forward slice above.
        lower.cache.clear();
        lower.rad_reverse.lowering_reverse_loop = true;
        for (signal_index, sig) in signals.iter().enumerate() {
            if reverse_time_outputs[signal_index] {
                lower.lower_output_signal(signal_index, *sig, plan.num_outputs)?;
            }
        }
        lower.rad_reverse.lowering_reverse_loop = false;
        if !has_forward_outputs {
            lower.finalize_global_cursor();
            let delay_sample_end = lower
                .delay
                .emit_sample_end_updates(&mut lower.store, lower.uses_iota);
            lower
                .regions
                .current_phases_mut()
                .sample_end
                .extend(delay_sample_end);
        }
        sample_loops.push((true, lower.regions.current_flattened()));
        lower.reset_sample_loop_state(region::RegionKind::SampleLoop);
    }
    for index in 0..plan.num_outputs {
        let mut b = FirBuilder::new(&mut lower.store);
        let chan = b.int32(i32::try_from(index).expect("validated output index fits i32"));
        let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
        let load_chan_ptr = b.load_table("outputs", AccessType::FunArgs, chan, ptr_ty.clone());
        lower.sections.control_statements.push(b.declare_var(
            format!("output{index}"),
            ptr_ty,
            AccessType::Stack,
            Some(load_chan_ptr),
        ));
    }
    if has_reverse_outputs {
        lower.emit_reverse_time_rec_compute_resets();
    }
    // Reset BRA carry variables at the start of every compute() call.
    //
    // These carries are populated by `ensure_bra_backward_sweep` regardless of
    // whether the BRA backward sweep runs in the forward or reverse sample loop.
    // Zeroing them here treats each `compute()` call as the start of a fresh
    // TBPTT block, which is the correct interpretation for both BS=BS (reverse
    // loop) and BS=1 (forward inline) TBPTT approximations.
    //
    // `emit_bra_compute_resets` is a no-op when no BRA carry variables were
    // allocated (i.e. when no `BlockReverseAD` node appears in the program).
    lower.emit_bra_compute_resets();
    // ═══════════════════════════════════════════════════════════════════════
    // ── Phase 2: CSE Materialization per Bucket ────────────────────────────
    // ═══════════════════════════════════════════════════════════════════════
    // Deduplicate multi-referenced value sub-expressions within each
    // execution tier.  Runs after variability placement (Phase 1) has
    // finalized bucket contents, so reference counts are stable.
    {
        use crate::signal_fir::cse;

        cse::materialize_shared_values(
            &mut lower.store,
            &mut lower.sections.constants_statements,
            "fConst",
            lower.name_gen.fconst_counter,
            "iConst",
            lower.name_gen.iconst_counter,
        );

        cse::materialize_shared_values(
            &mut lower.store,
            &mut lower.sections.control_statements,
            "fSlow",
            lower.name_gen.fslow_counter,
            "iSlow",
            lower.name_gen.islow_counter,
        );

        for (_, sample_loop_statements) in &mut sample_loops {
            cse::materialize_shared_values(
                &mut lower.store,
                sample_loop_statements,
                "fTemp",
                lower.name_gen.ftemp_counter,
                "iTemp",
                lower.name_gen.itemp_counter,
            );
            // The ordinary CSE pass creates the stable `fTemp*` declarations
            // that the state-aware pass may reuse. Keeping this ordering means
            // the latter only removes a proven-redundant direct table read; it
            // never changes scalar scheduling or synthesizes a new temporary.
            cse::reuse_straight_line_scalar_loads(&mut lower.store, sample_loop_statements);
        }
    }

    let metadata_body = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&[])
    };
    let metadata_args = [
        dsp_arg.clone(),
        NamedType {
            name: "m".to_string(),
            typ: FirType::Meta,
        },
    ];
    let metadata = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            "metadata",
            FirType::Fun {
                args: vec![dsp_arg_type.clone(), FirType::Meta],
                ret: Box::new(FirType::Void),
            },
            &metadata_args,
            Some(metadata_body),
            false,
        )
    };

    let constants_body = {
        let sample_rate_store = {
            let mut b = FirBuilder::new(&mut lower.store);
            let sample_rate = b.load_var("sample_rate", AccessType::FunArgs, FirType::Int32);
            b.store_var("fSampleRate", AccessType::Struct, sample_rate)
        };
        lower
            .sections
            .constants_statements
            .insert(0, sample_rate_store);
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&lower.sections.constants_statements)
    };
    let constants_args = [
        dsp_arg.clone(),
        NamedType {
            name: "sample_rate".to_string(),
            typ: FirType::Int32,
        },
    ];
    let instance_constants = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            "instanceConstants",
            FirType::Fun {
                args: vec![dsp_arg_type.clone(), FirType::Int32],
                ret: Box::new(FirType::Void),
            },
            &constants_args,
            Some(constants_body),
            false,
        )
    };

    lower.emit_ui_program()?;
    let ui_statements = lower.ui.ui_statements.clone();
    let ui_body = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&ui_statements)
    };
    let build_ui_args = [
        dsp_arg.clone(),
        NamedType {
            name: "ui_interface".to_string(),
            typ: FirType::UI,
        },
    ];
    let build_ui = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            "buildUserInterface",
            FirType::Fun {
                args: vec![dsp_arg_type.clone(), FirType::UI],
                ret: Box::new(FirType::Void),
            },
            &build_ui_args,
            Some(ui_body),
            false,
        )
    };

    let reset_body = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&lower.sections.reset_statements)
    };
    let instance_reset_ui = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            "instanceResetUserInterface",
            FirType::Fun {
                args: vec![dsp_arg_type.clone()],
                ret: Box::new(FirType::Void),
            },
            std::slice::from_ref(&dsp_arg),
            Some(reset_body),
            false,
        )
    };

    let clear_body = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&lower.sections.clear_statements)
    };
    let instance_clear = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            "instanceClear",
            FirType::Fun {
                args: vec![dsp_arg_type.clone()],
                ret: Box::new(FirType::Void),
            },
            std::slice::from_ref(&dsp_arg),
            Some(clear_body),
            false,
        )
    };

    let compute_statements = {
        use crate::signal_fir::loop_graph::{LoopGraph, LoopKind, slice_has_persistent_state};

        // Route the per-sample slices through the loop graph (roadmap P6, V4/V5b).
        // One loop node per non-empty slice, classified `Recursive` when the
        // slice writes cross-sample state (a recursion carrier / delay line) and
        // `Vectorizable` otherwise. Emitted in insertion order via
        // `topological_order`. Scalar mode ignores the kind and stays one loop
        // per slice — bit-identical to the previous inline emission (the 190
        // goldens are the guarantee). Vector mode chunks only `Vectorizable`
        // slices (a fully-recursive slice cannot auto-vectorize as one block, so
        // chunking it is pure overhead); per-statement separation of a recursive
        // core from its vectorizable pre/post parts is a later slice.
        let mut graph = LoopGraph::new();
        for (is_reverse, sample_loop_statements) in &sample_loops {
            if sample_loop_statements.is_empty() {
                continue;
            }
            let kind = if slice_has_persistent_state(&lower.store, sample_loop_statements) {
                LoopKind::Recursive
            } else {
                LoopKind::Vectorizable
            };
            let id = graph.add_loop(kind, *is_reverse);
            graph
                .node_mut(id)
                .exec
                .extend(sample_loop_statements.iter().copied());
        }
        let order = graph
            .topological_order()
            .expect("scalar sample loop graph has no dependency edges, so no cycle");

        let mut all = Vec::new();
        all.extend(lower.sections.control_statements.iter().copied());
        for id in order {
            let node = graph.node(id);
            let is_reverse = node.is_reverse;
            let kind = node.kind;
            let pre = node.pre.clone();
            let exec = node.exec.clone();
            let post = node.post.clone();
            all.extend(pre);
            if !exec.is_empty() {
                let sample_loop =
                    emit_sample_loop(&mut lower.store, &exec, is_reverse, kind, compute_mode);
                all.extend(sample_loop);
            }
            all.extend(post);
        }
        all
    };
    let compute_body = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&compute_statements)
    };
    let compute_args = [
        dsp_arg.clone(),
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            "compute",
            FirType::Fun {
                args: vec![
                    dsp_arg_type,
                    FirType::Int32,
                    FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                    FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                ],
                ret: Box::new(FirType::Void),
            },
            &compute_args,
            Some(compute_body),
            false,
        )
    };

    // Math function prototypes use the internal real type for both arguments and
    // return value: `sin`, `cos`, `pow`, etc. operate on internal-precision samples.
    let math_real_ty = lower.real_ty();
    let mut math_prototypes = Vec::new();
    for op in MATH_PROTO_ORDER {
        if !lower.used_protos.math_ops.contains(op) {
            continue;
        }
        let arity = match op {
            FirMathOp::Pow
            | FirMathOp::Min
            | FirMathOp::Max
            | FirMathOp::Atan2
            | FirMathOp::Fmod
            | FirMathOp::Remainder => 2,
            _ => 1,
        };
        let proto_args: Vec<NamedType> = (0..arity)
            .map(|i| NamedType {
                name: format!("arg{i}"),
                typ: math_real_ty.clone(),
            })
            .collect();
        let proto = {
            let mut b = FirBuilder::new(&mut lower.store);
            b.declare_fun(
                op.symbol(),
                FirType::Fun {
                    args: vec![math_real_ty.clone(); arity],
                    ret: Box::new(math_real_ty.clone()),
                },
                &proto_args,
                None,
                false,
            )
        };
        math_prototypes.push(proto);
    }
    for name in INT_FUN_PROTO_ORDER {
        if !lower.used_protos.int_fun_names.contains(name) {
            continue;
        }
        let arity = if *name == "abs" { 1 } else { 2 };
        let proto_args: Vec<NamedType> = (0..arity)
            .map(|i| NamedType {
                name: format!("arg{i}"),
                typ: FirType::Int32,
            })
            .collect();
        let proto = {
            let mut b = FirBuilder::new(&mut lower.store);
            b.declare_fun(
                *name,
                FirType::Fun {
                    args: vec![FirType::Int32; arity],
                    ret: Box::new(FirType::Int32),
                },
                &proto_args,
                None,
                false,
            )
        };
        math_prototypes.push(proto);
    }
    for proto in lower.used_protos.foreign_fun_protos.values() {
        let proto_args: Vec<NamedType> = proto
            .args
            .iter()
            .enumerate()
            .map(|(i, typ)| NamedType {
                name: format!("arg{i}"),
                typ: typ.clone(),
            })
            .collect();
        let decl = {
            let mut b = FirBuilder::new(&mut lower.store);
            b.declare_fun(
                proto.name.clone(),
                FirType::Fun {
                    args: proto.args.clone(),
                    ret: Box::new(proto.ret.clone()),
                },
                &proto_args,
                None,
                false,
            )
        };
        math_prototypes.push(decl);
    }
    math_prototypes.extend(lower.sections.global_declarations.iter().copied());
    let functions = {
        let mut b = FirBuilder::new(&mut lower.store);
        let function_items = [
            metadata,
            instance_constants,
            instance_reset_ui,
            instance_clear,
            build_ui,
            compute,
        ];
        b.block(&function_items)
    };
    let dsp_struct = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&lower.sections.struct_declarations)
    };
    let globals = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&math_prototypes)
    };
    let static_decls_block = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&lower.sections.static_declarations)
    };
    let module: FirId = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.module(
            plan.num_inputs,
            plan.num_outputs,
            module_name,
            dsp_struct,
            globals,
            functions,
            static_decls_block,
        )
    };

    Ok(SignalFirOutput {
        store: lower.store,
        module,
        emission_order: lower.emission_order,
        // Filled in by `compile_fastlane_inner`, which owns the causality
        // gate's `Hgraph`/`Hsched`; `build_module` has no schedule to
        // compare against.
        shadow_report: None,
        vector_pipeline_status: super::super::VectorPipelineStatus::NotRequested,
        vector_effective_mode: super::super::VectorEffectiveMode::Scalar,
        vector_pipeline_detail: None,
    })
}
