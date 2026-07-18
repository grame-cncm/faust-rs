//! PV — early vertical vector execution slice.
//!
//! Plan references:
//! `vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`,
//! section "PV - Early vertical vector execution slice"; certified plan
//! `lean-rust-certified-porting-plan-2026-07-11-en.md`, section
//! "RV - Early executable vertical slice".
//!
//! This module drives one deliberately small, non-trivial DSP shape through a
//! genuine signal-level `VectorPlan`-equivalent: it consumes prepared `SigId`
//! facts (never discovers the split by re-inspecting fused FIR), decides
//! placement with the existing [`crate::signal_fir::loop_graph::needs_separate_loop`]
//! precedence rule, allocates one typed cross-loop transport, orders the two
//! loops with the shared [`crate::schedule`] scheduler at `-ss 0`, and emits
//! FIR for both vector loop variants (`-lv 0` fixed-chunk-plus-remainder,
//! `-lv 1` single variable-size loop).
//!
//! # Scope, deliberately minimal (plan: "implement only the minimal
//! Inline/Owned placement, loop edge, typed chunk transport, and
//! region-routing path needed by that DSP")
//!
//! - Exactly one hand-picked DSP: `x = input(0) * 0.5`; `y = x @ delay_amount`
//!   (persistent cross-block state, `Inline`-at-use-site per C++
//!   `needSeparateLoop` priority 3); `z = x + 1.0` (a pure tail routed through
//!   the new transport instead of being trivially inlined, so the slice
//!   actually exercises cross-loop routing). Recursion groups and clock
//!   domains are out of scope (plan instruction), and the delay amount is
//!   chosen strictly larger than every tested block size so no delay read
//!   ever needs a same-block transported value — that combination (delay
//!   history spanning the *current* chunk) is `P6` scope, not `PV`.
//! - Occurrence counting and `max_delay` extraction are a genuine (if
//!   narrow) walk of the `SigId` forest via
//!   [`crate::signal_fir::loop_graph::signal_value_children`], not hardcoded
//!   facts. Variability is asserted `Samp` for this DSP shape rather than
//!   re-run through the full type inferencer — a full context-sensitive
//!   `SignalUseInfo` pass is `P4` scope (see the port plan, section 4.3).
//! - `PvPlan` has exactly two loop nodes and one transport; there is no
//!   general `VectorPlan`/certificate schema here yet (that is `R3`/`P5`).
//!
//! Not wired into any production compile path (no CLI/API consumes this
//! module), matching the additive pattern of the `P1`/`P2` phases already
//! landed.

use std::collections::HashSet;

use ahash::AHashMap;
use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirStore, FirType};
use signals::{BinOp, SigBuilder, SigId, SigMatch, match_sig};
use sigtype::Variability;
use tlib::TreeArena;

use crate::schedule::{ScheduleDag, SchedulingStrategy, schedule, verify_schedule};
use crate::signal_fir::loop_graph::{LoopSeparation, SignalLoopProps, needs_separate_loop};

/// Builds the PV DSP signal forest and returns `(arena, y, z)`, the two
/// output roots. `x` (the shared, delayed, separated signal) is recovered
/// from `y`'s `Delay` node by [`build_pv_plan`] rather than returned
/// separately, so every consumer goes through the same `SigId`-level fact
/// extraction path.
#[must_use]
pub fn build_pv_signals(delay_amount: i32) -> (TreeArena, SigId, SigId) {
    let mut arena = TreeArena::new();
    let (y, z) = {
        let mut b = SigBuilder::new(&mut arena);
        let inp = b.input(0);
        let half = b.real(0.5);
        let x = b.binop(BinOp::Mul, inp, half);
        let amount = b.int(delay_amount);
        let y = b.delay(x, amount);
        let one = b.real(1.0);
        let z = b.binop(BinOp::Add, x, one);
        (y, z)
    };
    (arena, y, z)
}

/// `SigId`-level facts needed by [`needs_separate_loop`] for this slice:
/// how many distinct use sites reference each signal, and the largest
/// constant delay amount any reader applies to it. Computed by a genuine walk
/// of the reachable forest via [`signal_value_children`] — not hardcoded.
struct PvFacts {
    occurrences: AHashMap<SigId, u32>,
    max_delay: AHashMap<SigId, i32>,
}

fn compute_pv_facts(arena: &TreeArena, roots: &[SigId]) -> PvFacts {
    let sig_types = sigtype::TypeAnnotator::new(arena, &ui::UiProgram::empty())
        .annotate(roots)
        .expect("PV signals have valid types");
    let analysis = super::vector_analysis::SignalAnalysisContext::new(arena, &sig_types, roots)
        .expect("PV symbolic recursion index is valid");
    let mut reachable: HashSet<SigId> = HashSet::new();
    let mut stack: Vec<SigId> = roots.to_vec();
    while let Some(sig) = stack.pop() {
        if reachable.insert(sig) {
            let dependencies = super::vector_analysis::signal_dependencies(&analysis, sig)
                .expect("PV signals are canonical");
            for occurrence in dependencies.occurrences() {
                stack.push(occurrence.to);
            }
        }
    }

    let mut occurrences: AHashMap<SigId, u32> = AHashMap::new();
    let mut max_delay: AHashMap<SigId, i32> = AHashMap::new();
    for &r in roots {
        *occurrences.entry(r).or_insert(0) += 1;
    }
    for &sig in &reachable {
        for occurrence in super::vector_analysis::signal_dependencies(&analysis, sig)
            .expect("PV signals are canonical")
            .occurrences()
        {
            *occurrences.entry(occurrence.to).or_insert(0) += 1;
            if occurrence.delay > 0 {
                let amount = i32::try_from(occurrence.delay).expect("P4.2 delay fits i32");
                let entry = max_delay.entry(occurrence.to).or_insert(0);
                if amount > *entry {
                    *entry = amount;
                }
            }
        }
    }
    PvFacts {
        occurrences,
        max_delay,
    }
}

/// Identity of the two loops in this slice's minimal loop graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PvLoopId {
    /// Owns `x`: computes it, writes the delay-history ring buffer, and
    /// emits `y` (the delay read, inlined at its use site).
    OwnsX,
    /// Consumes `x`'s current-sample value through the typed transport and
    /// emits `z` (the pure tail).
    ConsumesTransport,
}

/// The one typed cross-loop transport this slice allocates: `x`'s
/// current-sample value, produced by [`PvLoopId::OwnsX`], consumed by
/// [`PvLoopId::ConsumesTransport`].
#[derive(Debug, Clone)]
pub struct PvTransport {
    pub signal: SigId,
    pub elem_type: FirType,
    /// Chunk capacity in elements. [`route_pv_vector_fir`] allocates the
    /// transport as a struct-level table (see that function's docs for why
    /// stack-local was not usable), fully overwritten for indices `0..count`
    /// on every call; only that freshly written range is ever read, so
    /// persistence between calls carries no semantic meaning.
    pub max_length: usize,
}

/// The strategy-independent plan for this slice: exactly two loops and one
/// transport. Deliberately not a general `VectorPlan` (that is `R3`/`P5`
/// scope) — see the module docs.
#[derive(Debug, Clone)]
pub struct PvPlan {
    pub x: SigId,
    pub y: SigId,
    pub z: SigId,
    pub delay_amount: i32,
    pub transport: PvTransport,
}

impl PvPlan {
    /// Projects this slice's plan into the strategy-independent
    /// [`crate::signal_fir::vector_verify::VectorPlan`] DTO and — by
    /// construction — a plan that [`crate::signal_fir::vector_verify::verify_vector_plan`]
    /// accepts. This closes the loop between the executed PV slice and the P5
    /// vector-plan verifier: the same two-loop, one-transport shape the PV
    /// test runs bit-exactly is here shown to be a *valid* vector plan, so the
    /// verifier is exercised on a real produced plan rather than only on
    /// hand-written fixtures.
    ///
    /// Loop `0` (`OwnsX`) materializes `x` and `y`; loop `1`
    /// (`ConsumesTransport`) materializes `z` and reads `x` through the
    /// transport. Both are `Vectorizable` and share the single forward epoch.
    #[must_use]
    pub fn to_vector_plan(&self) -> crate::signal_fir::vector_verify::VectorPlan {
        use crate::signal_fir::vector_verify as vv;

        let id = |s: SigId| u64::from(s.as_u32());
        let elem = value_type_of(self.transport.elem_type.clone());
        let vec_size = self.transport.max_length as u64;

        let owned = |sig: SigId, loop_id: u64| vv::SignalRecord {
            signal_id: id(sig),
            value_type: vv::ValueType::Real,
            structural: false,
            rate: vv::Rate::Samp,
            vectorability: vv::Vectorability::Vect,
            clock_id: 0,
            effects: Vec::new(),
            direct_effects: Vec::new(),
            placement: vv::Placement::Owned(loop_id),
            duplicable: true,
        };
        // signals array must be strictly ascending by signal_id.
        let mut signals = vec![owned(self.x, 0), owned(self.y, 0), owned(self.z, 1)];
        signals.sort_by_key(|s| s.signal_id);

        vv::VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size,
            signals,
            loops: vec![
                vv::LoopRecord {
                    loop_id: 0,
                    stable_name: "pv_loop_owns_x".to_owned(),
                    kind: vv::LoopKind::Vectorizable,
                    roots: vec![id(self.x), id(self.y)],
                    epoch_id: 0,
                },
                vv::LoopRecord {
                    loop_id: 1,
                    stable_name: "pv_loop_consumes".to_owned(),
                    kind: vv::LoopKind::Vectorizable,
                    roots: vec![id(self.z)],
                    epoch_id: 0,
                },
            ],
            epochs: vec![vv::EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0, 1],
            }],
            transports: vec![vv::TransportRecord {
                transport_id: 0,
                stable_name: "transportX".to_owned(),
                signal_id: id(self.transport.signal),
                producer_loop: 0,
                consumer_loop: 1,
                element_type: elem,
                length: vec_size,
                layout: crate::signal_fir::vector_verify::TransportLayout::Planar,
            }],
            data_edges: vec![vv::LoopEdge {
                consumer: 1,
                dependency: 0,
            }],
            effect_edges: Vec::new(),
            vec_safe_witnesses: vec![
                vv::VecSafeWitness {
                    loop_id: 0,
                    witness_kind: vv::WitnessKind::Pointwise,
                },
                vv::VecSafeWitness {
                    loop_id: 1,
                    witness_kind: vv::WitnessKind::Pointwise,
                },
            ],
            fused_serial_groups: Vec::new(),
        }
    }
}

/// Maps an internal FIR value type to the vector-plan DTO's value-type
/// vocabulary (`int`/`real`/`tuple`). The PV slice only produces real chunk
/// transports, but the mapping is spelled out so the bridge does not silently
/// mislabel an integer carrier if the slice grows.
fn value_type_of(ty: FirType) -> crate::signal_fir::vector_verify::ValueType {
    use crate::signal_fir::vector_verify::ValueType;
    match ty {
        FirType::Int32 | FirType::Int64 => ValueType::Int,
        _ => ValueType::Real,
    }
}

impl ScheduleDag for PvPlan {
    type Node = PvLoopId;

    fn nodes(&self) -> Vec<PvLoopId> {
        vec![PvLoopId::OwnsX, PvLoopId::ConsumesTransport]
    }

    fn dependencies(&self, n: PvLoopId) -> Vec<PvLoopId> {
        match n {
            PvLoopId::OwnsX => vec![],
            // ConsumesTransport reads x's current-sample value through the
            // transport, so OwnsX (the producer) must run first.
            PvLoopId::ConsumesTransport => vec![PvLoopId::OwnsX],
        }
    }
}

/// Builds the plan from `SigId` facts and asserts the placement precondition
/// this slice is meant to exercise: `x` must classify as a separated,
/// vectorizable loop under the existing (unmodified) `needs_separate_loop`
/// precedence — never rediscovered from FIR.
///
/// # Panics
/// If the DSP shape does not match what this slice expects (`y` must be a
/// `Delay` node), or if `x`'s classification is not
/// `LoopSeparation::SeparateVectorizable` — the plan is validated as part of
/// construction, per the certified-porting-plan L2 pattern (a malformed plan
/// cannot be built).
#[must_use]
pub fn build_pv_plan(arena: &TreeArena, y: SigId, z: SigId, max_block: usize) -> PvPlan {
    let (x, delay_amount) = match match_sig(arena, y) {
        SigMatch::Delay(value, amount) => match match_sig(arena, amount) {
            SigMatch::Int(n) => (value, n),
            _ => panic!("PV slice expects a constant integer delay amount"),
        },
        _ => panic!("PV slice expects `y` to be a Delay node"),
    };

    let facts = compute_pv_facts(arena, &[y, z]);
    let occurrences_x = facts.occurrences.get(&x).copied().unwrap_or(0);
    let max_delay_x = facts.max_delay.get(&x).copied().unwrap_or(0);
    assert_eq!(
        max_delay_x, delay_amount,
        "the walked max_delay fact must agree with y's own delay amount"
    );

    let x_props = SignalLoopProps {
        // Asserted, not inferred: see module docs on scope. `x` is
        // input-derived, hence unconditionally Samp for this DSP shape.
        variability: Variability::Samp,
        max_delay: usize::try_from(max_delay_x).expect("delay amount is non-negative"),
        is_recursive_proj: false,
        is_shared: occurrences_x >= 2,
        is_delay_read: false,
        is_very_simple: false, // x = input * 0.5 is a BinOp, not a leaf.
    };
    let verdict = needs_separate_loop(&x_props);
    assert_eq!(
        verdict,
        LoopSeparation::SeparateVectorizable,
        "PV DSP's `x` must classify as a separate vectorizable loop \
         (needs_separate_loop verdict: {verdict:?}); this placement decision \
         is the precondition the whole slice tests"
    );

    PvPlan {
        x,
        y,
        z,
        delay_amount,
        transport: PvTransport {
            signal: x,
            elem_type: FirType::FaustFloat,
            max_length: max_block,
        },
    }
}

/// Runs the shared `-ss 0` scheduler on the plan's two-loop graph and the
/// independent postcondition checker — never a bespoke ordering.
///
/// # Panics
/// If the (trivial, acyclic-by-construction) loop graph is somehow rejected,
/// or if the produced order fails the independent checker.
#[must_use]
pub fn pv_schedule(plan: &PvPlan) -> Vec<PvLoopId> {
    let order = schedule(SchedulingStrategy::DepthFirst, plan)
        .expect("PV loop graph is a trivial two-node DAG");
    verify_schedule(plan, &order).expect("scheduled order must satisfy the postcondition checker");
    order
}

/// Which of the two loop-variant FIR shapes to emit (`-lv 0` / `-lv 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PvLoopVariant {
    /// Fixed vector size (chunk width [`LV0_CHUNK`]) plus a remaining loop.
    Lv0,
    /// Simple: one loop whose size varies with `count` itself.
    Lv1,
}

/// Fixed chunk width used by the `Lv0` loop-variant shape.
const LV0_CHUNK: i32 = 4;

/// Emits one FIR `compute` body for [`PvPlan`] under the requested loop
/// variant. Both variants read/write the *same* named globals and the same
/// transport table, so their bit-for-bit output must agree with each other
/// and with the scalar reference — that agreement is the slice's pass
/// criterion, not anything asserted here.
///
/// The construction order inside each loop body is significant: the delay
/// history read (`fDelay[widx]`) is captured into an explicit temp-variable
/// statement *before* the overwriting write, because FIR expressions
/// evaluate where they are textually embedded, not at their construction
/// point — reusing the raw load expression after the write would silently
/// read the just-written (undelayed) value.
#[must_use]
pub fn route_pv_vector_fir(plan: &PvPlan, variant: PvLoopVariant) -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let max_block = plan.transport.max_length;

    let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let chan0 = b.int32(0);
    let in_ptr = b.load_table("inputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let in_alias = b.declare_var("input0", ptr_ty.clone(), AccessType::Stack, Some(in_ptr));
    let out_chan0 = b.int32(0);
    let out0_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan0, ptr_ty.clone());
    let out0_alias = b.declare_var("output0", ptr_ty.clone(), AccessType::Stack, Some(out0_ptr));
    let out_chan1 = b.int32(1);
    let out1_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan1, ptr_ty.clone());
    let out1_alias = b.declare_var("output1", ptr_ty, AccessType::Stack, Some(out1_ptr));

    let zero_f = b.float32(0.0);

    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);

    let loops: Vec<FirId> = match variant {
        PvLoopVariant::Lv1 => {
            let body_a = build_loop_a_body(&mut b, plan.delay_amount);
            let loop_a = b.simple_for_loop("i0", count, body_a, false);
            let body_b = build_loop_b_body(&mut b);
            let loop_b = b.simple_for_loop("i0", count, body_b, false);
            vec![loop_a, loop_b]
        }
        PvLoopVariant::Lv0 => {
            let zero_i = b.int32(0);
            let one_i = b.int32(1);
            let k = b.int32(LV0_CHUNK);
            let chunks = b.binop(FirBinOp::Div, count, k, FirType::Int32);
            let full_end = b.binop(FirBinOp::Mul, chunks, k, FirType::Int32);

            // The general `for_loop` (unlike `simple_for_loop`, which always
            // starts at 0 or `upper - 1` and so can synthesize its own loop
            // variable) requires `init` to be an explicit `DeclareVar`
            // seeding the loop variable at an arbitrary start value — a bare
            // constant here left "i0" unregistered and every backend
            // rejected the loop bodies' `load_var("i0", ...)` as undeclared.
            //
            // `for_loop` is a **do-while** construct (`compile_for_loop`'s own
            // doc comment: "var = init; do { body; var += step } while
            // (cond)"), so it always runs at least once even when
            // `init == end`. Both the main chunk (empty whenever
            // `count < LV0_CHUNK`) and the remainder (empty whenever `count`
            // is an exact multiple of `LV0_CHUNK`) can legitimately be empty,
            // so each must be gated by an explicit `if` — an earlier version
            // omitted this and silently read/wrote one sample past `count`
            // whenever the remainder was empty.
            let main_nonempty = b.binop(FirBinOp::Gt, full_end, zero_i, FirType::Bool);
            let rem_nonempty = b.binop(FirBinOp::Lt, full_end, count, FirType::Bool);

            let init_a_main = b.declare_var("i0", FirType::Int32, AccessType::Loop, Some(zero_i));
            let body_a_main = build_loop_a_body(&mut b, plan.delay_amount);
            let loop_a_main = b.for_loop("i0", init_a_main, full_end, one_i, body_a_main, false);
            let loop_a_main_block = b.block(&[loop_a_main]);
            let loop_a_main_guarded = b.if_(main_nonempty, loop_a_main_block, None);

            let init_a_rem = b.declare_var("i0", FirType::Int32, AccessType::Loop, Some(full_end));
            let body_a_rem = build_loop_a_body(&mut b, plan.delay_amount);
            let loop_a_rem = b.for_loop("i0", init_a_rem, count, one_i, body_a_rem, false);
            let loop_a_rem_block = b.block(&[loop_a_rem]);
            let loop_a_rem_guarded = b.if_(rem_nonempty, loop_a_rem_block, None);

            let init_b_main = b.declare_var("i0", FirType::Int32, AccessType::Loop, Some(zero_i));
            let body_b_main = build_loop_b_body(&mut b);
            let loop_b_main = b.for_loop("i0", init_b_main, full_end, one_i, body_b_main, false);
            let loop_b_main_block = b.block(&[loop_b_main]);
            let loop_b_main_guarded = b.if_(main_nonempty, loop_b_main_block, None);

            let init_b_rem = b.declare_var("i0", FirType::Int32, AccessType::Loop, Some(full_end));
            let body_b_rem = build_loop_b_body(&mut b);
            let loop_b_rem = b.for_loop("i0", init_b_rem, count, one_i, body_b_rem, false);
            let loop_b_rem_block = b.block(&[loop_b_rem]);
            let loop_b_rem_guarded = b.if_(rem_nonempty, loop_b_rem_block, None);

            vec![
                loop_a_main_guarded,
                loop_a_rem_guarded,
                loop_b_main_guarded,
                loop_b_rem_guarded,
            ]
        }
    };

    let mut compute_stmts = vec![in_alias, out0_alias, out1_alias];
    compute_stmts.extend(loops);
    let compute_body = b.block(&compute_stmts);

    let args = [
        fir::NamedType {
            name: "dsp".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        },
        fir::NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        fir::NamedType {
            name: "inputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
        fir::NamedType {
            name: "outputs".to_string(),
            typ: FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        },
    ];
    let compute_ty = FirType::Fun {
        args: vec![
            FirType::Ptr(Box::new(FirType::Obj)),
            FirType::Int32,
            FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
        ],
        ret: Box::new(FirType::Void),
    };
    let compute = b.declare_fun("compute", compute_ty, &args, Some(compute_body), false);

    let idx0 = b.int32(0);
    let ring_init: Vec<FirId> = (0..plan.delay_amount).map(|_| zero_f).collect();
    // `transportX` is a struct-level (not stack-level) table: the interp
    // backend does not support a mid-function `DeclareTable`, and a
    // persistent chunk buffer overwritten in full every call (indices
    // `0..count`, never read outside that freshly written range) matches
    // how real vectorizing C++ backends place `Vector*`/`Zec*` chunk
    // buffers as class members rather than stack locals.
    let transport_init: Vec<FirId> = (0..max_block).map(|_| zero_f).collect();
    let globals = [
        b.declare_var("fWriteIdx", FirType::Int32, AccessType::Struct, Some(idx0)),
        b.declare_table(
            "fDelay",
            AccessType::Struct,
            FirType::FaustFloat,
            &ring_init,
        ),
        b.declare_table(
            "transportX",
            AccessType::Struct,
            FirType::FaustFloat,
            &transport_init,
        ),
    ];

    let dsp_struct = b.block(&[]);
    let globals_block = b.block(&globals);
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        1,
        2,
        "pv_vector_slice",
        dsp_struct,
        globals_block,
        functions,
        static_decls,
    );
    (store, module)
}

/// `PvLoopId::OwnsX`'s per-sample body: computes `x`, writes it to the
/// transport, performs the capture-before-write delay-ring read/write, and
/// emits `y`.
fn build_loop_a_body(b: &mut FirBuilder<'_>, delay_amount: i32) -> FirId {
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let inp = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let half = b.float32(0.5);
    // Pure (depends only on unmutated input0/half): safe to reference more
    // than once without a capture step.
    let x_val = b.binop(FirBinOp::Mul, inp, half, FirType::FaustFloat);
    let store_transport = b.store_table("transportX", AccessType::Struct, i0, x_val);

    // `widx`'s only mutation in this body is `store_widx`, which comes after
    // every other use below, so the raw load expression is safe to reuse.
    let widx = b.load_var("fWriteIdx", AccessType::Struct, FirType::Int32);
    let ring_read = b.load_table("fDelay", AccessType::Struct, widx, FirType::FaustFloat);
    // Must capture before the overwrite: FIR expressions evaluate in place,
    // not at construction time.
    let capture_read = b.declare_var(
        "tmpRead",
        FirType::FaustFloat,
        AccessType::Stack,
        Some(ring_read),
    );
    let write_ring = b.store_table("fDelay", AccessType::Struct, widx, x_val);

    let one_i = b.int32(1);
    let idx_plus = b.binop(FirBinOp::Add, widx, one_i, FirType::Int32);
    let d_const = b.int32(delay_amount);
    let ge_wrap = b.binop(FirBinOp::Ge, idx_plus, d_const, FirType::Bool);
    let zero_i = b.int32(0);
    let wrap = b.select2(ge_wrap, zero_i, idx_plus, FirType::Int32);
    let store_widx = b.store_var("fWriteIdx", AccessType::Struct, wrap);

    let read_ref = b.load_var("tmpRead", AccessType::Stack, FirType::FaustFloat);
    let store_y = b.store_table("output0", AccessType::Stack, i0, read_ref);

    b.block(&[
        store_transport,
        capture_read,
        write_ring,
        store_widx,
        store_y,
    ])
}

/// `PvLoopId::ConsumesTransport`'s per-sample body: reads `x`'s
/// current-sample value through the transport and emits `z`.
fn build_loop_b_body(b: &mut FirBuilder<'_>) -> FirId {
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x_transport = b.load_table("transportX", AccessType::Struct, i0, FirType::FaustFloat);
    let one = b.float32(1.0);
    let z_val = b.binop(FirBinOp::Add, x_transport, one, FirType::FaustFloat);
    let store_z = b.store_table("output1", AccessType::Stack, i0, z_val);
    b.block(&[store_z])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_pv_plan_places_x_as_a_separate_vectorizable_loop() {
        let (arena, y, z) = build_pv_signals(20);
        let plan = build_pv_plan(&arena, y, z, 16);
        assert_eq!(plan.delay_amount, 20);
        assert_eq!(plan.transport.max_length, 16);
        assert!(matches!(plan.transport.elem_type, FirType::FaustFloat));
    }

    #[test]
    fn pv_plan_topology_is_two_loops_one_transport() {
        let (arena, y, z) = build_pv_signals(20);
        let plan = build_pv_plan(&arena, y, z, 16);
        let nodes = plan.nodes();
        assert_eq!(nodes.len(), 2, "topology assertion: exactly two loops");
        assert_eq!(plan.dependencies(PvLoopId::OwnsX).len(), 0);
        assert_eq!(
            plan.dependencies(PvLoopId::ConsumesTransport),
            vec![PvLoopId::OwnsX],
            "the consumer loop must depend on the owning loop through the transport"
        );
    }

    #[test]
    fn pv_schedule_orders_owns_x_before_consumes_transport() {
        let (arena, y, z) = build_pv_signals(20);
        let plan = build_pv_plan(&arena, y, z, 16);
        let order = pv_schedule(&plan);
        assert_eq!(order, vec![PvLoopId::OwnsX, PvLoopId::ConsumesTransport]);
    }

    #[test]
    fn pv_schedule_rejects_an_inverted_order() {
        let (arena, y, z) = build_pv_signals(20);
        let plan = build_pv_plan(&arena, y, z, 16);
        let inverted = vec![PvLoopId::ConsumesTransport, PvLoopId::OwnsX];
        assert!(
            verify_schedule(&plan, &inverted).is_err(),
            "a consumer scheduled before its transport's producer must be rejected"
        );
    }

    #[test]
    fn pv_plan_projects_to_a_valid_vector_plan() {
        use crate::signal_fir::vector_verify::verify_vector_plan;

        let (arena, y, z) = build_pv_signals(20);
        let plan = build_pv_plan(&arena, y, z, 16);
        let vplan = plan.to_vector_plan();

        // The executed PV plan is a valid strategy-independent vector plan:
        // this exercises the P5 verifier on a real produced plan, not a
        // hand-written fixture.
        verify_vector_plan(&vplan).expect("the PV slice's plan must satisfy verify_vector_plan");

        assert_eq!(vplan.vec_size, 16);
        assert_eq!(vplan.loops.len(), 2);
        assert_eq!(vplan.transports.len(), 1);
        assert_eq!(vplan.transports[0].length, vplan.vec_size);
    }
}
