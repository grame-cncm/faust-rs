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

use super::region;
use super::setup;
use super::state;
use crate::signal_fir::FirId;
use crate::signal_fir::FirStore;
use crate::signal_fir::FirType;
use crate::signal_fir::SigId;
use crate::signal_fir::SignalFirError;
use crate::signal_fir::SignalFirOutput;
use crate::signal_fir::TreeArena;
use crate::signal_fir::UiProgram;
use crate::signal_fir::module::AccessType;
use crate::signal_fir::module::DelayOptions;
use crate::signal_fir::module::FirBuilder;
use crate::signal_fir::module::FirMathOp;
use crate::signal_fir::module::HashMap;
use crate::signal_fir::module::INT_FUN_PROTO_ORDER;
use crate::signal_fir::module::MATH_PROTO_ORDER;
use crate::signal_fir::module::NamedType;
use crate::signal_fir::module::SigType;
use crate::signal_fir::module::SignalToFirLower;
use crate::signal_fir::module::classify_reverse_time_outputs;
use crate::signal_fir::module::clocked;
use crate::signal_fir::module::dump_sig_readable;
use crate::signal_fir::module::fixed_ad_internal_signals;
use crate::signal_fir::placement::analyze_signal_sharing;
use crate::signal_fir::planner::SignalFirPlan;
use crate::signal_fir::{ControlRateMode, ProcessingApi};
use crate::signal_prepare::SimpleSigType;

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

/// A single `for (i0 = 0; i0 < count; i0++) { <exec> }` (forward or
/// reverse-time). The scalar lowerer's only sample-loop shape.
fn plain_sample_loop(store: &mut FirStore, exec: &[FirId], is_reverse: bool) -> FirId {
    let mut b = FirBuilder::new(store);
    let upper = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let body = b.block(exec);
    b.simple_for_loop("i0", upper, body, is_reverse)
}

/// Names and element type of the table one generator sub-module fills.
pub(crate) struct FillSpec {
    /// Sub-module class name, `{module}SIG{k}`.
    pub(crate) name: String,
    /// Element type of the filled table.
    pub(crate) elem_ty: FirType,
}

/// Assembles a `SubModule` from a finished table-generator lowering.
///
/// C++ parity: `generateInstanceInitFun` (fInit + fResetUI + fClear) and
/// `generateFillFun` (compute block + scalar loop over `count`) in
/// `code_container.cpp`.
fn assemble_sub_module(
    lower: &mut SignalToFirLower<'_>,
    spec: &FillSpec,
    fill_statements: &[FirId],
) -> Result<FirId, SignalFirError> {
    let dsp_arg_type = FirType::Ptr(Box::new(FirType::Obj));
    let dsp_arg = NamedType {
        name: "dsp".to_string(),
        typ: dsp_arg_type.clone(),
    };

    let init_body = {
        // A sub-module has no `classInit` of its own, so anything it would
        // have put there — the fills of tables *it* generates, when the
        // generator itself reads a generated table — goes at the front of its
        // `instanceInit`. The parent calls `instanceInit` before `fill`, so a
        // nested table is populated before the loop that reads it.
        //
        // Dropping these is not a missing optimization but a wrong answer: it
        // is precisely the upstream defect where the inner table is
        // declared and never filled, and the outer table is computed from
        // zeros. Rule FIR-SM05 keeps it from coming back.
        let mut statements = lower.sections.static_init_statements.clone();
        statements.extend(lower.sections.constants_statements.iter().copied());
        statements.extend(lower.sections.reset_statements.iter().copied());
        statements.extend(lower.sections.clear_statements.iter().copied());
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(&statements)
    };
    let init_args = [
        dsp_arg.clone(),
        NamedType {
            name: "sample_rate".to_string(),
            typ: FirType::Int32,
        },
    ];
    let instance_init = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            format!("instanceInit{}", spec.name),
            FirType::Fun {
                args: vec![dsp_arg_type.clone(), FirType::Int32],
                ret: Box::new(FirType::Void),
            },
            &init_args,
            Some(init_body),
            false,
        )
    };

    let fill_body = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.block(fill_statements)
    };
    let table_arg_ty = FirType::Ptr(Box::new(spec.elem_ty.clone()));
    let fill_args = [
        dsp_arg,
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "table".to_string(),
            typ: table_arg_ty.clone(),
        },
    ];
    let fill = {
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            format!("fill{}", spec.name),
            FirType::Fun {
                args: vec![dsp_arg_type, FirType::Int32, table_arg_ty],
                ret: Box::new(FirType::Void),
            },
            &fill_args,
            Some(fill_body),
            false,
        )
    };

    // A generator that reads another generated table produced that table's
    // sub-module during this same lowering; it must travel with us (contract
    // C5). Taking it from the lowering — not from the caller's spec — is what
    // makes nesting work at arbitrary depth.
    let nested = std::mem::take(&mut lower.sub_modules);
    let mut b = FirBuilder::new(&mut lower.store);
    let functions = b.block(&[instance_init, fill]);
    let dsp_struct = b.block(&lower.sections.struct_declarations);
    let static_decls = b.block(&lower.sections.static_declarations);
    let globals = b.block(&lower.sections.global_declarations);
    Ok(b.sub_module(
        spec.name.clone(),
        spec.elem_ty.clone(),
        dsp_struct,
        static_decls,
        globals,
        functions,
        &nested,
    ))
}

/// The six Faust lifecycle functions of a scalar module, in emission order.
struct LifecycleFunctions {
    /// Optional `staticInit` (present only with file-scope generated tables).
    static_init: Option<FirId>,
    /// `metadata(m)`.
    metadata: FirId,
    /// `instanceConstants(sample_rate)`.
    instance_constants: FirId,
    /// `buildUserInterface(ui_interface)`.
    build_ui: FirId,
    /// `instanceResetUserInterface()`.
    instance_reset_ui: FirId,
    /// `instanceClear()`.
    instance_clear: FirId,
}

/// Lowers the per-sample slices: the scheduled control/top graphs first,
/// then the forward output slice, then the reverse-time slice (public
/// reverse-AD outputs), each flattened with its end-of-sample delay
/// maintenance. Returns `(is_reverse, statements)` per non-empty slice.
fn lower_sample_slices(
    lower: &mut SignalToFirLower<'_>,
    plan: &SignalFirPlan,
    signals: &[SigId],
    reverse_time_outputs: &[bool],
    has_forward_outputs: bool,
    has_reverse_outputs: bool,
) -> Result<Vec<(bool, Vec<FirId>)>, SignalFirError> {
    let mut sample_loops = Vec::new();

    // Reverse AD owns a fixed forward/reverse epoch split and is deliberately
    // outside the flat same-tick Hgraph. Vector mode keeps that driver
    // authoritative.
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
    Ok(sample_loops)
}

/// Emits the compute-entry prologue: the `outputN` channel aliases (block
/// API only), reverse-time recursion resets, and the BRA carry resets that
/// treat each `compute()` call as a fresh TBPTT block.
fn emit_compute_prologue(
    lower: &mut SignalToFirLower<'_>,
    plan: &SignalFirPlan,
    processing_api: ProcessingApi,
    fill: Option<&FillSpec>,
    has_reverse_outputs: bool,
) {
    // A table-fill module writes into its `table` argument, not into audio
    // channels: emitting the `outputN = outputs[N]` aliases here would
    // reference an `outputs` parameter its signature does not have.
    if !processing_api.is_one_sample() && fill.is_none() {
        for index in 0..plan.num_outputs {
            let mut b = FirBuilder::new(&mut lower.store);
            let chan = b.int32(i32::try_from(index).expect("validated output index fits i32"));
            let ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
            let load_chan_ptr = b.load_table("outputs", AccessType::FunArgs, chan, ptr_ty.clone());
            let decl = b.declare_var(
                format!("output{index}"),
                ptr_ty,
                AccessType::Stack,
                Some(load_chan_ptr),
            );
            lower.sections.push_compute_preamble(decl);
        }
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
}

/// CSE materialization per execution bucket (constants, control, per-slice
/// sample statements), re-deriving control ownership tags afterwards.
fn run_bucket_cse(lower: &mut SignalToFirLower<'_>, sample_loops: &mut [(bool, Vec<FirId>)]) {
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

        // CSE operates on the flat statement list; re-derive the ownership
        // tags afterwards. Statements that survive keep their tag; newly
        // inserted declarations are shared fSlow/iSlow values, which are
        // control-rate by construction, hence externalizable.
        let prior_ownership: HashMap<FirId, state::ControlOwnership> = lower
            .sections
            .control_statements
            .iter()
            .map(|entry| (entry.statement, entry.ownership))
            .collect();
        let mut flat: Vec<FirId> = lower
            .sections
            .control_statements
            .iter()
            .map(|entry| entry.statement)
            .collect();
        cse::materialize_shared_values(
            &mut lower.store,
            &mut flat,
            "fSlow",
            lower.name_gen.fslow_counter,
            "iSlow",
            lower.name_gen.islow_counter,
        );
        lower.sections.control_statements = flat
            .into_iter()
            .map(|statement| state::ControlStatement {
                ownership: prior_ownership
                    .get(&statement)
                    .copied()
                    .unwrap_or(state::ControlOwnership::Externalizable),
                statement,
            })
            .collect();

        for (_, sample_loop_statements) in sample_loops.iter_mut() {
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
}

/// Emits the Faust lifecycle functions (`metadata`, optional `staticInit`,
/// `instanceConstants`, `buildUserInterface`, `instanceResetUserInterface`,
/// `instanceClear`) from the finished section statement lists.
fn emit_lifecycle_functions(
    lower: &mut SignalToFirLower<'_>,
    dsp_arg: &NamedType,
    dsp_arg_type: &FirType,
) -> Result<LifecycleFunctions, SignalFirError> {
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

    // `staticInit` carries the fills of file-scope generated tables and is
    // rendered as `classInit` by the backends. It is emitted only when such a
    // table exists, so every program without one keeps its current shape.
    let static_init = (!lower.sections.static_init_statements.is_empty()).then(|| {
        let body = {
            let mut b = FirBuilder::new(&mut lower.store);
            b.block(&lower.sections.static_init_statements)
        };
        let args = [
            dsp_arg.clone(),
            NamedType {
                name: "sample_rate".to_string(),
                typ: FirType::Int32,
            },
        ];
        let mut b = FirBuilder::new(&mut lower.store);
        b.declare_fun(
            "staticInit",
            FirType::Fun {
                args: vec![dsp_arg_type.clone(), FirType::Int32],
                ret: Box::new(FirType::Void),
            },
            &args,
            Some(body),
            false,
        )
    });

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
            std::slice::from_ref(dsp_arg),
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
            std::slice::from_ref(dsp_arg),
            Some(clear_body),
            false,
        )
    };

    Ok(LifecycleFunctions {
        static_init,
        metadata,
        instance_constants,
        build_ui,
        instance_reset_ui,
        instance_clear,
    })
}

/// Declares every used math / integer-helper / foreign prototype plus the
/// module's global declarations, in stable order.
fn collect_prototype_declarations(lower: &mut SignalToFirLower<'_>) -> Vec<FirId> {
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
    math_prototypes
}

/// Splits the tagged compute-preamble statements by ownership: classic mode
/// keeps everything in the block entry point in original order; under
/// external control the externalizable statements move, in order, into the
/// separate `control` function. Returns `(control_fn, entry_preamble)`.
fn split_control_statements(
    lower: &SignalToFirLower<'_>,
    external_control: bool,
) -> (Vec<FirId>, Vec<FirId>) {
    let control_fn_statements: Vec<FirId> = if external_control {
        lower
            .sections
            .control_statements
            .iter()
            .filter(|entry| entry.ownership == state::ControlOwnership::Externalizable)
            .map(|entry| entry.statement)
            .collect()
    } else {
        Vec::new()
    };
    let entry_preamble: Vec<FirId> = lower
        .sections
        .control_statements
        .iter()
        .filter(|entry| {
            !external_control || entry.ownership == state::ControlOwnership::ComputePreamble
        })
        .map(|entry| entry.statement)
        .collect();
    (control_fn_statements, entry_preamble)
}

/// Assembles the flat `compute` statement list: the entry preamble followed
/// by every per-sample slice routed through the loop graph, each emitted as
/// one plain sample loop (or inline, in one-sample mode).
fn assemble_compute_statements(
    lower: &mut SignalToFirLower<'_>,
    sample_loops: &[(bool, Vec<FirId>)],
    entry_preamble: &[FirId],
    one_sample: bool,
) -> Vec<FirId> {
    use crate::signal_fir::loop_graph::LoopGraph;

    // Route the per-sample slices through the loop graph: one loop node
    // per non-empty slice, emitted in insertion order via
    // `topological_order` — bit-identical to the previous inline
    // emission (the goldens are the guarantee). This lowerer is
    // scalar-only: accepted `-vec` compiles are built by the checked
    // vector pipeline, and a rejected one falls back here in scalar
    // shape, so every slice becomes one plain sample loop.
    let mut graph = LoopGraph::new();
    for (is_reverse, sample_loop_statements) in sample_loops {
        if sample_loop_statements.is_empty() {
            continue;
        }
        let id = graph.add_loop(*is_reverse);
        graph
            .node_mut(id)
            .exec
            .extend(sample_loop_statements.iter().copied());
    }
    let order = graph
        .topological_order()
        .expect("scalar sample loop graph has no dependency edges, so no cycle");

    let mut all = Vec::new();
    all.extend(entry_preamble.iter().copied());
    for id in order {
        let node = graph.node(id);
        let is_reverse = node.is_reverse;
        let pre = node.pre.clone();
        let exec = node.exec.clone();
        let post = node.post.clone();
        all.extend(pre);
        if !exec.is_empty() {
            if one_sample {
                // One-sample mode: `frame` processes exactly one sample — the slice
                // body is emitted directly, with no enclosing loop and no
                // `count`. I/O accesses were lowered as direct channel
                // loads/stores above.
                all.extend(exec.iter().copied());
            } else {
                all.push(plain_sample_loop(&mut lower.store, &exec, is_reverse));
            }
        }
        all.extend(post);
    }
    all
}

/// Declares the public entry points: the canonical `compute`, plus `frame`
/// (one-sample mode) and `control` (external control) when requested.
fn emit_entry_points(
    lower: &mut SignalToFirLower<'_>,
    dsp_arg: &NamedType,
    dsp_arg_type: FirType,
    compute_statements: &[FirId],
    control_fn_statements: &[FirId],
    one_sample: bool,
    external_control: bool,
) -> (FirId, Option<FirId>, Option<FirId>) {
    // In one-sample mode the canonical `compute` is kept but emitted
    // empty — it never delegates to `frame`.
    let compute_body = {
        let mut b = FirBuilder::new(&mut lower.store);
        if one_sample {
            b.block(&[])
        } else {
            b.block(compute_statements)
        }
    };
    let frame = one_sample.then(|| {
        let mut b = FirBuilder::new(&mut lower.store);
        let frame_body = b.block(compute_statements);
        let flat_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
        let frame_args = [
            dsp_arg.clone(),
            NamedType {
                name: "inputs".to_string(),
                typ: flat_ty.clone(),
            },
            NamedType {
                name: "outputs".to_string(),
                typ: flat_ty.clone(),
            },
        ];
        b.declare_fun(
            "frame",
            FirType::Fun {
                args: vec![
                    FirType::Ptr(Box::new(FirType::Obj)),
                    flat_ty.clone(),
                    flat_ty,
                ],
                ret: Box::new(FirType::Void),
            },
            &frame_args,
            Some(frame_body),
            false,
        )
    });
    let control = external_control.then(|| {
        let mut b = FirBuilder::new(&mut lower.store);
        let control_body = b.block(control_fn_statements);
        b.declare_fun(
            "control",
            FirType::Fun {
                args: vec![FirType::Ptr(Box::new(FirType::Obj))],
                ret: Box::new(FirType::Void),
            },
            std::slice::from_ref(dsp_arg),
            Some(control_body),
            false,
        )
    });
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

    (compute, frame, control)
}

/// Assembles the final output: the `functions`/struct/globals/static
/// blocks, then either the table-generator `SubModule` (fill mode) or the
/// complete scalar `Module`, with origins derived over the reachable FIR.
#[allow(clippy::too_many_arguments)]
fn assemble_module_output(
    mut lower: SignalToFirLower<'_>,
    plan: &SignalFirPlan,
    module_name: &str,
    lifecycle: LifecycleFunctions,
    entry_points: (FirId, Option<FirId>, Option<FirId>),
    math_prototypes: Vec<FirId>,
    compute_statements: &[FirId],
    fill: Option<&FillSpec>,
) -> Result<SignalFirOutput, SignalFirError> {
    let LifecycleFunctions {
        static_init,
        metadata,
        instance_constants,
        build_ui,
        instance_reset_ui,
        instance_clear,
    } = lifecycle;
    let (compute, frame, control) = entry_points;
    let functions = {
        let mut b = FirBuilder::new(&mut lower.store);
        let mut function_items = Vec::new();
        function_items.extend(static_init);
        function_items.extend([
            metadata,
            instance_constants,
            instance_reset_ui,
            instance_clear,
            build_ui,
        ]);
        // C++ emission order: `control` then `frame` precede the
        // canonical `compute`.
        function_items.extend(control);
        function_items.extend(frame);
        function_items.push(compute);
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
    // Table-generator lowering returns a `SubModule` built from the same
    // sections: `instanceInit` is the C++ sub-container's fInit + fResetUI +
    // fClear, and `fill` is its compute block plus the scalar loop over
    // `count` — which is exactly `compute_statements`, since the output sink
    // already redirected the single output to `table[i0]`.
    if let Some(spec) = fill {
        let module = assemble_sub_module(&mut lower, spec, compute_statements)?;
        lower.fir_origins.derive_reachable(&lower.store, module);
        return Ok(SignalFirOutput {
            store: lower.store,
            module,
            origins: lower.fir_origins,
            emission_order: lower.emission_order,
            shadow_report: None,
            vector_pipeline_status: super::super::VectorPipelineStatus::NotRequested,
            vector_effective_mode: super::super::VectorEffectiveMode::Scalar,
            vector_pipeline_detail: None,
            table_warnings: Vec::new(),
        });
    }

    let sub_modules = std::mem::take(&mut lower.sub_modules);
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
            &sub_modules,
        )
    };

    lower.fir_origins.derive_reachable(&lower.store, module);
    Ok(SignalFirOutput {
        store: lower.store,
        module,
        origins: lower.fir_origins,
        emission_order: lower.emission_order,
        // Filled in by `compile_fastlane_inner`, which owns the causality
        // gate's `Hgraph`/`Hsched`; `build_module` has no schedule to
        // compare against.
        shadow_report: None,
        vector_pipeline_status: super::super::VectorPipelineStatus::NotRequested,
        vector_effective_mode: super::super::VectorEffectiveMode::Scalar,
        vector_pipeline_detail: None,
        table_warnings: Vec::new(),
    })
}

/// Lowers a prepared signal forest into a complete FIR module.
///
/// Entry point for the fast-lane lowering boundary: accepts pre-validated
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
    signal_origins: &'a propagate::SignalOrigins,
    real_ty: FirType,
    max_copy_delay: u32,
    delay_line_threshold: u32,
    control_rate_mode: ControlRateMode,
    processing_api: ProcessingApi,
    table_init_mode: crate::signal_fir::TableInitMode,
    table_init_sample_rate: Option<i32>,
    check_table: bool,
    scheduling_strategy: crate::schedule::SchedulingStrategy,
    clocked: Option<clocked::ClockedPlan<'a>>,
    scalar_schedule: Option<&crate::hgraph::Hsched>,
    // `fill`: when set, lower a table generator instead of a DSP — the single
    // output is stored into the `table` argument and the result is a
    // `SubModule` rather than a `Module` (C++ `signal2Container`).
    fill: Option<&FillSpec>,
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
        module_name,
        ui,
        types,
        sig_types,
        signal_origins,
        plan.num_inputs,
        real_ty,
        placement,
        delay_opts,
    );
    lower.control_rate_mode = control_rate_mode;
    lower.processing_api = processing_api;
    lower.table_fill_sink = fill.map(|spec| spec.elem_ty.clone());
    lower.table_init_mode = table_init_mode;
    lower.table_init_sample_rate = table_init_sample_rate;
    lower.check_table = check_table;
    lower.scheduling_strategy = scheduling_strategy;
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
        let label = b.label("signal_fir_fastlane_step2a: executable base slice");
        lower.sections.push_compute_preamble(label);
        let label = b.label(format!(
            "io: inputs={} outputs={}",
            plan.num_inputs, plan.num_outputs
        ));
        lower.sections.push_compute_preamble(label);
        let label = b.label(format!("signals: {}", plan.signal_count));
        lower.sections.push_compute_preamble(label);
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
    let sample_loops = {
        let mut sample_loops = lower_sample_slices(
            &mut lower,
            plan,
            signals,
            &reverse_time_outputs,
            has_forward_outputs,
            has_reverse_outputs,
        )?;
        emit_compute_prologue(&mut lower, plan, processing_api, fill, has_reverse_outputs);
        run_bucket_cse(&mut lower, &mut sample_loops);
        sample_loops
    };

    let lifecycle = emit_lifecycle_functions(&mut lower, &dsp_arg, &dsp_arg_type)?;

    let external_control = control_rate_mode.is_external();
    let one_sample = processing_api.is_one_sample();
    debug_assert!(
        !(one_sample && has_reverse_outputs),
        "D2 rejects -os for block-sensitive reverse-AD programs before lowering"
    );
    let (control_fn_statements, entry_preamble) =
        split_control_statements(&lower, external_control);
    let compute_statements =
        assemble_compute_statements(&mut lower, &sample_loops, &entry_preamble, one_sample);
    let (compute, frame, control) = emit_entry_points(
        &mut lower,
        &dsp_arg,
        dsp_arg_type,
        &compute_statements,
        &control_fn_statements,
        one_sample,
        external_control,
    );
    let math_prototypes = collect_prototype_declarations(&mut lower);
    assemble_module_output(
        lower,
        plan,
        module_name,
        lifecycle,
        (compute, frame, control),
        math_prototypes,
        &compute_statements,
        fill,
    )
}
