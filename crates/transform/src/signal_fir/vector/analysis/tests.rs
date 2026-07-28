//! Tests for `vector::analysis` (relocated from the former inline
//! `mod tests` block; test names unchanged).

use propagate::ClockDomainTable;
use signals::SigBuilder;
use tlib::{TreeArena, sym_rec, vec_to_list};

use super::effects::{direct_effects, propagate_effect_sets};
use super::uses::{analyze_forest, finalize_occurrences};
use super::*;
use crate::clk_env::annotate;
use crate::signal_prepare::prepare_signals_for_fir_verified;
use signals::{SigMatch, match_sig};
use sigtype::Variability;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tlib::NodeKind;

fn dep_targets(deps: &[AnalysisDependency]) -> Vec<(u32, DepKind)> {
    deps.iter().map(|dep| (dep.to.as_u32(), dep.kind)).collect()
}

fn occurrence_targets(deps: &[OccurrenceUse]) -> Vec<(u32, u32)> {
    deps.iter()
        .map(|dep| (dep.to.as_u32(), dep.delay))
        .collect()
}

fn decode(
    arena: &TreeArena,
    roots: &[SigId],
    sig: SigId,
) -> Result<SignalDependencies, AnalysisError> {
    let types = sigtype::TypeAnnotator::new(arena, &ui::UiProgram::empty())
        .annotate(roots)
        .unwrap();
    let context = SignalAnalysisContext::new(arena, &types, roots)?;
    signal_dependencies(&context, sig)
}

#[test]
fn delay_prefix_and_seq_have_distinct_scheduling_and_occurrence_rules() {
    let mut arena = TreeArena::new();
    let (x, one, three, delay1, fixed, bounded_zero, bounded_one, prefix, seq) = {
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let zero = b.int(0);
        let one = b.int(1);
        let three = b.int(3);
        let raw_dynamic = b.input(1);
        let dynamic = b.mul(raw_dynamic, three);
        let zero_to_three = b.assert_bounds(zero, three, dynamic);
        let one_to_three = b.assert_bounds(one, three, dynamic);
        let delay1 = b.delay1(x);
        let fixed = b.delay(x, three);
        let bounded_zero = b.delay(x, zero_to_three);
        let bounded_one = b.delay(x, one_to_three);
        let prefix = b.prefix(one, x);
        let seq = b.seq(x, dynamic);
        (
            x,
            one,
            three,
            delay1,
            fixed,
            bounded_zero,
            bounded_one,
            prefix,
            seq,
        )
    };
    let roots = [delay1, fixed, bounded_zero, bounded_one, prefix, seq];

    let delay1_deps = decode(&arena, &roots, delay1).unwrap();
    assert_eq!(
        dep_targets(delay1_deps.scheduling()),
        vec![(x.as_u32(), DepKind::Delayed { amount: 1 })]
    );
    assert_eq!(
        occurrence_targets(delay1_deps.occurrences()),
        vec![(x.as_u32(), 1)]
    );

    let fixed_deps = decode(&arena, &roots, fixed).unwrap();
    assert_eq!(
        dep_targets(fixed_deps.scheduling()),
        vec![
            (x.as_u32(), DepKind::Delayed { amount: 3 }),
            (three.as_u32(), DepKind::Immediate)
        ]
    );
    assert_eq!(
        occurrence_targets(fixed_deps.occurrences()),
        vec![(x.as_u32(), 3), (three.as_u32(), 0)]
    );

    let bounded_zero_deps = decode(&arena, &roots, bounded_zero).unwrap();
    assert_eq!(
        bounded_zero_deps.scheduling()[0].kind,
        DepKind::Immediate,
        "[0, N] may read the current sample"
    );
    assert_eq!(bounded_zero_deps.occurrences()[0].delay, 3);

    let bounded_one_deps = decode(&arena, &roots, bounded_one).unwrap();
    assert_eq!(
        bounded_one_deps.scheduling()[0].kind,
        DepKind::Delayed { amount: 3 },
        "[1, N] is causally delayed"
    );

    let prefix_deps = decode(&arena, &roots, prefix).unwrap();
    assert_eq!(
        dep_targets(prefix_deps.scheduling()),
        vec![
            (one.as_u32(), DepKind::Immediate),
            (x.as_u32(), DepKind::Immediate)
        ]
    );
    assert_eq!(
        occurrence_targets(prefix_deps.occurrences()),
        vec![(one.as_u32(), 0), (x.as_u32(), 1)]
    );

    let seq_deps = decode(&arena, &roots, seq).unwrap();
    assert_eq!(
        dep_targets(seq_deps.scheduling()),
        vec![(x.as_u32(), DepKind::Immediate)]
    );
    assert_eq!(seq_deps.occurrences().len(), 2);
}

#[test]
fn projection_schedules_its_selected_definition_and_marks_the_group() {
    let mut arena = TreeArena::new();
    let (first, second) = {
        let mut b = SigBuilder::new(&mut arena);
        (b.input(0), b.input(1))
    };
    let var = arena.symbol("r");
    let body = vec_to_list(&mut arena, &[first, second]);
    let group = sym_rec(&mut arena, var, body);
    let (projection, back_reference) = {
        let reference = tlib::sym_ref(&mut arena, var);
        let mut b = SigBuilder::new(&mut arena);
        (b.proj(1, group), b.proj(1, reference))
    };
    let empty_types = HashMap::new();
    let context =
        SignalAnalysisContext::new(&arena, &empty_types, &[projection, back_reference]).unwrap();

    let projection_deps = signal_dependencies(&context, projection).unwrap();
    assert_eq!(
        dep_targets(projection_deps.scheduling()),
        vec![(second.as_u32(), DepKind::Immediate)]
    );
    assert_eq!(
        occurrence_targets(projection_deps.occurrences()),
        vec![(group.as_u32(), 0)]
    );
    assert_eq!(
        occurrence_targets(signal_dependencies(&context, group).unwrap().occurrences()),
        vec![(first.as_u32(), 0), (second.as_u32(), 0)]
    );
    assert_eq!(
        dep_targets(
            signal_dependencies(&context, back_reference)
                .unwrap()
                .scheduling()
        ),
        vec![(second.as_u32(), DepKind::Delayed { amount: 1 })]
    );
}

#[test]
fn recursiveness_expands_shared_recursive_dag_once_per_signal() {
    let mut arena = TreeArena::new();
    let mut shared = SigBuilder::new(&mut arena).input(0);
    const LAYERS: usize = 18;

    // Each layer reaches the same lower DAG through two distinct binders.
    // Memoizing `(signal, environment)` creates 2^LAYERS states, whereas
    // C++ `recursivenessAnnotation` stores exactly one value per signal.
    for layer in 0..LAYERS {
        let left_var = arena.symbol(format!("left_{layer}"));
        let left_body = vec_to_list(&mut arena, &[shared]);
        let left_group = sym_rec(&mut arena, left_var, left_body);
        let right_var = arena.symbol(format!("right_{layer}"));
        let right_body = vec_to_list(&mut arena, &[shared]);
        let right_group = sym_rec(&mut arena, right_var, right_body);
        let mut builder = SigBuilder::new(&mut arena);
        let left = builder.proj(0, left_group);
        let right = builder.proj(0, right_group);
        shared = builder.add(left, right);
    }

    let empty_types = HashMap::new();
    let context = SignalAnalysisContext::new(&arena, &empty_types, &[shared]).unwrap();
    let (by_signal, expanded_signals) = compute_recursiveness(&context, &[shared]).unwrap();
    let expected_signals = 1 + 5 * LAYERS;
    assert_eq!(by_signal.len(), expected_signals);
    assert_eq!(expanded_signals, expected_signals);
}

#[test]
fn fir_rules_preserve_tap_delays_and_zero_coefficient_fallback() {
    let mut arena = TreeArena::new();
    let (input, zero, c1, c3, sparse, all_zero) = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let zero = b.int(0);
        let c1 = b.real(0.5);
        let c3 = b.real(0.25);
        let sparse = b.fir(&[input, zero, c1, zero, c3]);
        let all_zero = b.fir(&[input, zero, zero]);
        (input, zero, c1, c3, sparse, all_zero)
    };
    let empty_types = HashMap::new();
    let context = SignalAnalysisContext::new(&arena, &empty_types, &[sparse, all_zero]).unwrap();

    let sparse_deps = signal_dependencies(&context, sparse).unwrap();
    assert_eq!(
        dep_targets(sparse_deps.scheduling()),
        vec![
            (zero.as_u32(), DepKind::Immediate),
            (c1.as_u32(), DepKind::Immediate),
            (zero.as_u32(), DepKind::Immediate),
            (c3.as_u32(), DepKind::Immediate),
            (input.as_u32(), DepKind::Delayed { amount: 1 }),
        ]
    );
    assert_eq!(
        occurrence_targets(sparse_deps.occurrences()),
        vec![
            (zero.as_u32(), 0),
            (c1.as_u32(), 0),
            (zero.as_u32(), 0),
            (c3.as_u32(), 0),
            (input.as_u32(), 1),
            (input.as_u32(), 3),
        ]
    );

    let all_zero_deps = signal_dependencies(&context, all_zero).unwrap();
    assert_eq!(
        occurrence_targets(all_zero_deps.occurrences()).last(),
        Some(&(input.as_u32(), 0))
    );
}

#[test]
fn compact_iir_rules_ignore_state_as_a_child_and_mark_feedback_delays() {
    let mut arena = TreeArena::new();
    let (state, input, c0, c1, c2, iir) = {
        let mut b = SigBuilder::new(&mut arena);
        let state = b.input(0);
        let input = b.input(1);
        let c0 = b.int(0);
        let c1 = b.real(-0.5);
        let c2 = b.real(0.25);
        let iir = b.iir(&[state, input, c0, c1, c2]);
        (state, input, c0, c1, c2, iir)
    };
    let empty_types = HashMap::new();
    let context = SignalAnalysisContext::new(&arena, &empty_types, &[iir]).unwrap();
    let deps = signal_dependencies(&context, iir).unwrap();

    assert_eq!(
        dep_targets(deps.scheduling()),
        vec![
            (input.as_u32(), DepKind::Immediate),
            (c0.as_u32(), DepKind::Immediate),
            (c1.as_u32(), DepKind::Immediate),
            (c2.as_u32(), DepKind::Immediate),
        ]
    );
    assert_eq!(
        occurrence_targets(deps.occurrences()),
        vec![
            (input.as_u32(), 0),
            (c1.as_u32(), 0),
            (iir.as_u32(), 1),
            (c2.as_u32(), 0),
            (iir.as_u32(), 2),
        ]
    );
    assert!(!deps.occurrences().iter().any(|usage| usage.to == state));
    assert_eq!(
        deps.condition_children(),
        [state, input, c0, c1, c2],
        "condition propagation follows every non-nil structural IIR child"
    );
}

#[test]
fn table_and_clock_wrapper_rules_match_cpp_child_selection() {
    let mut arena = TreeArena::new();
    let (size, generator, write_index, write_value, read_index, table, read, wrapper) = {
        let mut b = SigBuilder::new(&mut arena);
        let size = b.int(128);
        let generator = b.input(0);
        let write_index = b.input(1);
        let write_value = b.input(2);
        let read_index = b.input(3);
        let clock = b.int(2);
        let payload = b.input(4);
        let table = b.wrtbl(size, generator, write_index, write_value);
        let read = b.rdtbl(table, read_index);
        let wrapper = b.on_demand(&[clock, payload]);
        (
            size,
            generator,
            write_index,
            write_value,
            read_index,
            table,
            read,
            wrapper,
        )
    };
    let empty_types = HashMap::new();
    let context = SignalAnalysisContext::new(&arena, &empty_types, &[read, wrapper]).unwrap();

    let table_deps = signal_dependencies(&context, table).unwrap();
    assert_eq!(table_deps.scheduling().len(), 4);
    assert_eq!(table_deps.scheduling()[0].to, size);
    assert_eq!(table_deps.scheduling()[1].to, generator);

    let read_deps = signal_dependencies(&context, read).unwrap();
    assert_eq!(
        dep_targets(read_deps.scheduling()),
        vec![
            (read_index.as_u32(), DepKind::Immediate),
            (write_index.as_u32(), DepKind::Immediate),
            (write_value.as_u32(), DepKind::Immediate),
        ]
    );
    assert_eq!(
        occurrence_targets(read_deps.occurrences()),
        vec![(table.as_u32(), 0), (read_index.as_u32(), 0)]
    );

    let wrapper_deps = signal_dependencies(&context, wrapper).unwrap();
    assert_eq!(wrapper_deps.scheduling()[0].kind, DepKind::ClockBoundary);
    assert_eq!(wrapper_deps.scheduling()[1].kind, DepKind::Immediate);
    assert_eq!(wrapper_deps.occurrences().len(), 2);
}

#[test]
fn generators_are_analysis_leaves_and_malformed_iir_is_rejected() {
    let mut arena = TreeArena::new();
    let (generator, malformed_iir) = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let generator = b.generate(input);
        let zero = b.int(0);
        let malformed_iir = b.iir(&[input, input, zero]);
        (generator, malformed_iir)
    };
    let empty_types = HashMap::new();
    let context =
        SignalAnalysisContext::new(&arena, &empty_types, &[generator, malformed_iir]).unwrap();

    let generator_deps = signal_dependencies(&context, generator).unwrap();
    assert!(generator_deps.scheduling().is_empty());
    assert!(generator_deps.occurrences().is_empty());
    assert!(generator_deps.condition_children().is_empty());
    assert!(matches!(
        signal_dependencies(&context, malformed_iir),
        Err(AnalysisError::Malformed { .. })
    ));
}

#[test]
fn repeated_minus_one_product_propagates_sharing_to_its_rhs() {
    let mut arena = TreeArena::new();
    let (input, root) = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let minus_one = b.int(-1);
        let negated = b.mul(minus_one, input);
        let root = b.fir(&[negated, negated]);
        (input, root)
    };
    let types = sigtype::TypeAnnotator::new(&arena, &ui::UiProgram::empty())
        .annotate(&[root])
        .unwrap();
    let context = SignalAnalysisContext::new(&arena, &types, &[root]).unwrap();
    let table = analyze_forest(
        &context,
        &[root],
        |_| Some(None),
        &ConstantExecutionConditions::default(),
    )
    .unwrap();

    assert_eq!(
        table.get(input).unwrap().occurrences.per_context[0].count,
        2
    );
    assert!(table.get(input).unwrap().occurrences.multi);
}

fn analyze(
    arena: &TreeArena,
    roots: &[SigId],
    conditions: &impl ExecutionConditions,
) -> (VerifiedPreparedSignals, SignalUseTable) {
    let prepared = prepare_signals_for_fir_verified(arena, roots, &ui::UiProgram::empty()).unwrap();
    let domains = ClockDomainTable::new();
    let clocks = annotate(prepared.arena(), &domains, prepared.outputs()).unwrap();
    let table = analyze_signal_uses(&prepared, &clocks, conditions).unwrap();
    (prepared, table)
}

#[test]
fn table_is_deterministic_and_marks_duplicate_same_context() {
    let mut arena = TreeArena::new();
    let root = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let shared = b.sin(input);
        b.fir(&[shared, shared])
    };
    let conditions = ConstantExecutionConditions::default();
    let (first_prepared, first) = analyze(&arena, &[root], &conditions);
    let (_, second) = analyze(&arena, &[root], &conditions);
    assert_eq!(first, second);
    let analysis = SignalAnalysisContext::new(
        first_prepared.arena(),
        first_prepared.sig_types_map(),
        first_prepared.outputs(),
    )
    .unwrap();
    let root_dependencies = signal_dependencies(&analysis, first_prepared.outputs()[0]).unwrap();
    assert_eq!(root_dependencies.occurrences().len(), 2);
    assert_eq!(
        root_dependencies.occurrences()[0].to,
        root_dependencies.occurrences()[1].to
    );
    let shared_info = first.get(root_dependencies.occurrences()[0].to).unwrap();
    assert_eq!(shared_info.occurrences.per_context[0].count, 2);
    assert!(shared_info.occurrences.multi);
}

#[test]
fn faster_context_and_distinct_conditions_mark_multi() {
    let mut arena = TreeArena::new();
    let ty = arena.symbol("float");
    let name = arena.symbol("k");
    let file = arena.symbol("f");
    let root = {
        let mut b = SigBuilder::new(&mut arena);
        let constant = b.fconst(ty, name, file);
        let shared = b.input(0);
        let left = b.add(shared, constant);
        let right = b.mul(shared, constant);
        b.fir(&[left, right])
    };
    struct Branches;
    impl ExecutionConditions for Branches {
        fn signal_condition(&self, sig: SigId) -> CondId {
            CondId(u64::from(sig.as_u32()))
        }

        fn root_condition(&self, _root: SigId) -> CondId {
            CondId(0)
        }
    }
    let (_, table) = analyze(&arena, &[root], &Branches);
    assert!(
        table
            .records()
            .iter()
            .filter(|record| record.info.occurrences.multi)
            .flat_map(|record| &record.info.occurrences.per_context)
            .any(|occ| occ.context.variability > Variability::Konst),
        "a constant-rate node used by sample-rate code must be multi"
    );
    assert!(
        table.records().iter().any(|record| {
            let conditions = record
                .info
                .occurrences
                .per_context
                .iter()
                .map(|occ| occ.context.condition)
                .collect::<BTreeSet<_>>();
            conditions.len() > 1 && record.info.occurrences.multi
        }),
        "uses under distinct execution conditions must be multi"
    );

    let mut aggregate = table.records()[0].info.clone();
    aggregate.variability = Variability::Samp;
    aggregate.recursiveness = 0;
    aggregate.execution_condition = CondId(7);
    aggregate.occurrences = OccInfo {
        per_context: vec![
            ContextOccurrence {
                context: UseContext {
                    variability: Variability::Block,
                    recursiveness: 1,
                    condition: CondId(7),
                },
                count: 1,
            },
            ContextOccurrence {
                context: UseContext {
                    variability: Variability::Samp,
                    recursiveness: 0,
                    condition: CondId(7),
                },
                count: 1,
            },
        ],
        multi: false,
    };
    finalize_occurrences(&mut aggregate);
    assert!(
        aggregate.occurrences.multi,
        "C++ aggregates both contexts in extended-variability bucket 2"
    );
}

#[test]
fn a_second_use_context_does_not_reexpand_shared_children() {
    let mut arena = TreeArena::new();
    let root = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let shared = b.sin(input);
        let left = b.cos(shared);
        let right = b.exp(shared);
        b.fir(&[left, right])
    };
    struct ParentConditions;
    impl ExecutionConditions for ParentConditions {
        fn signal_condition(&self, sig: SigId) -> CondId {
            CondId(u64::from(sig.as_u32()))
        }

        fn root_condition(&self, _root: SigId) -> CondId {
            CondId(0)
        }
    }

    let (prepared, table) = analyze(&arena, &[root], &ParentConditions);
    let analysis = SignalAnalysisContext::new(
        prepared.arena(),
        prepared.sig_types_map(),
        prepared.outputs(),
    )
    .unwrap();
    let branches = signal_dependencies(&analysis, prepared.outputs()[0]).unwrap();
    assert_eq!(branches.occurrences().len(), 2);
    let left_dependencies = signal_dependencies(&analysis, branches.occurrences()[0].to).unwrap();
    let right_dependencies = signal_dependencies(&analysis, branches.occurrences()[1].to).unwrap();
    assert_eq!(left_dependencies.occurrences().len(), 1);
    assert_eq!(right_dependencies.occurrences().len(), 1);
    assert_eq!(
        left_dependencies.occurrences()[0].to,
        right_dependencies.occurrences()[0].to
    );
    let shared = left_dependencies.occurrences()[0].to;
    let shared_dependencies = signal_dependencies(&analysis, shared).unwrap();
    assert_eq!(shared_dependencies.occurrences().len(), 1);

    assert_eq!(table.get(shared).unwrap().occurrences.per_context.len(), 2);
    assert_eq!(
        table
            .get(shared_dependencies.occurrences()[0].to)
            .unwrap()
            .occurrences
            .per_context
            .len(),
        1
    );
}

#[test]
fn delay_projection_and_very_simple_facts_are_recorded() {
    let mut arena = TreeArena::new();
    let (delayed, int, real, input, rec_input) = {
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let amount = b.int(3);
        let delayed = b.delay(x, amount);
        let int = b.int(1);
        let real = b.real(1.0);
        let input = b.input(1);
        let rec_input = b.input(2);
        (delayed, int, real, input, rec_input)
    };
    let var = arena.symbol("r");
    let body = vec_to_list(&mut arena, &[rec_input]);
    let group = sym_rec(&mut arena, var, body);
    let projection = SigBuilder::new(&mut arena).proj(0, group);
    let fconst = {
        let ty = arena.symbol("float");
        let name = arena.symbol("k");
        let file = arena.symbol("f");
        SigBuilder::new(&mut arena).fconst(ty, name, file)
    };
    let (prepared, table) = analyze(
        &arena,
        &[delayed, projection, int, real, input, fconst],
        &ConstantExecutionConditions::default(),
    );
    let prepared_delayed = prepared.outputs()[0];
    let analysis = SignalAnalysisContext::new(
        prepared.arena(),
        prepared.sig_types_map(),
        prepared.outputs(),
    )
    .unwrap();
    let delayed_dependencies = signal_dependencies(&analysis, prepared_delayed).unwrap();
    let delayed_value = delayed_dependencies
        .scheduling()
        .iter()
        .find_map(|dependency| match dependency.kind {
            DepKind::Delayed { amount: 3 } => Some(dependency.to),
            _ => None,
        })
        .expect("prepared fixed delay has one delayed value dependency");
    let x_info = table.get(delayed_value).unwrap();
    assert_eq!(
        (x_info.max_delay, x_info.delay_reads, x_info.is_delay_read),
        (3, 1, false)
    );
    assert!(table.get(prepared_delayed).unwrap().is_delay_read);
    assert_eq!(
        table
            .get(prepared.outputs()[1])
            .unwrap()
            .recursive_projection
            .unwrap()
            .index,
        0
    );
    for &sig in &prepared.outputs()[2..] {
        assert!(table.get(sig).unwrap().very_simple);
    }
    assert!(!table.get(prepared_delayed).unwrap().very_simple);
}

#[test]
fn missing_clock_type_and_invalid_delay_interval_are_typed_errors() {
    let mut arena = TreeArena::new();
    let (root, dynamic_delay, amount) = {
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let root = b.sin(x);
        let minus_three = b.int(-3);
        let minus_one = b.int(-1);
        let amount_input = b.input(1);
        let amount = b.assert_bounds(minus_three, minus_one, amount_input);
        let dynamic_delay = b.delay(x, amount);
        (root, dynamic_delay, amount)
    };
    let conditions = ConstantExecutionConditions::default();
    let empty_types = HashMap::new();
    let analysis = SignalAnalysisContext::new(&arena, &empty_types, &[root]).unwrap();
    assert_eq!(
        analyze_forest(&analysis, &[root], |_| Some(None), &conditions),
        Err(AnalysisError::MissingType { sig: root })
    );
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let prepared_root = prepared.outputs()[0];
    let analysis = SignalAnalysisContext::new(
        prepared.arena(),
        prepared.sig_types_map(),
        prepared.outputs(),
    )
    .unwrap();
    assert_eq!(
        analyze_forest(&analysis, &[prepared_root], |_| None, &conditions),
        Err(AnalysisError::MissingClock { sig: prepared_root })
    );

    let amount_types = sigtype::TypeAnnotator::new(&arena, &ui::UiProgram::empty())
        .annotate(&[amount])
        .unwrap();
    let analysis = SignalAnalysisContext::new(&arena, &amount_types, &[dynamic_delay]).unwrap();
    assert!(matches!(
        signal_dependencies(&analysis, dynamic_delay),
        Err(AnalysisError::InvalidDelayInterval { .. })
    ));
}

#[test]
fn execution_conditions_match_control_dnf_and_occurrence_multi() {
    let mut arena = TreeArena::new();
    let root = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let left_gate = b.input(1);
        let right_gate = b.input(2);
        let value = b.sin(input);
        let guarded_left = b.control(value, left_gate);
        let guarded_right = b.control(value, right_gate);
        b.add(guarded_left, guarded_right)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let clocks = annotate(
        prepared.arena(),
        &ClockDomainTable::new(),
        prepared.outputs(),
    )
    .unwrap();
    let VectorSignalAnalysis { conditions, uses } =
        analyze_vector_signals(&prepared, &clocks).unwrap();
    for control in uses.records().iter().filter(|record| {
        matches!(
            match_sig(prepared.arena(), record.sig),
            SigMatch::Control(_, _)
        )
    }) {
        let dependencies = uses
            .dependencies()
            .iter()
            .filter(|dependency| dependency.from == control.sig)
            .collect::<Vec<_>>();
        assert_eq!(dependencies.len(), 2);
        assert!(
            dependencies
                .iter()
                .any(|edge| edge.kind == DepKind::Control)
        );
        assert!(
            dependencies
                .iter()
                .any(|edge| edge.kind == DepKind::Immediate)
        );
    }
    let (value, value_info) = uses
        .records()
        .iter()
        .find_map(|record| {
            matches!(match_sig(prepared.arena(), record.sig), SigMatch::Sin(_))
                .then_some((record.sig, &record.info))
        })
        .expect("prepared graph retains the shared sine value");
    let condition = conditions
        .condition(value_info.execution_condition)
        .expect("condition id is interned");

    assert_eq!(condition.clauses().len(), 2);
    assert!(condition.clauses().iter().all(|clause| clause.len() == 1));
    assert!(value_info.occurrences.multi);
    assert_eq!(
        conditions.signal_condition(value),
        value_info.execution_condition
    );
    assert_eq!(
        ExecutionConditionTable::build(&prepared).unwrap(),
        conditions
    );
}

#[test]
fn unconditional_use_absorbs_a_guarded_condition() {
    let mut arena = TreeArena::new();
    let root = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let gate = b.input(1);
        let value = b.sin(input);
        let guarded = b.control(value, gate);
        b.add(value, guarded)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[root], &ui::UiProgram::empty()).unwrap();
    let conditions = ExecutionConditionTable::build(&prepared).unwrap();
    let value = prepared
        .sig_types_map()
        .keys()
        .copied()
        .find(|&sig| matches!(match_sig(prepared.arena(), sig), SigMatch::Sin(_)))
        .expect("prepared graph retains sine");
    assert!(
        conditions
            .condition(conditions.signal_condition(value))
            .unwrap()
            .is_unconditional()
    );
}

#[test]
fn the_readonly_table_predicate_requires_both_write_ports_nil() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let (write_index, write_value) = {
        let mut b = SigBuilder::new(&mut arena);
        (b.input(1), b.input(2))
    };
    assert!(
        wrtbl_is_readonly(&arena, nil, nil),
        "rdtable binds neither write port"
    );
    assert!(
        !wrtbl_is_readonly(&arena, write_index, nil),
        "a live write index alone keeps the table mutable"
    );
    assert!(
        !wrtbl_is_readonly(&arena, nil, write_value),
        "a live write value alone keeps the table mutable"
    );
    assert!(!wrtbl_is_readonly(&arena, write_index, write_value));
}

#[test]
fn readonly_tables_carry_no_write_effect_while_mutable_tables_keep_one() {
    let mut arena = TreeArena::new();
    let (readonly, mutable, readonly_read, mutable_read, outputs) = {
        let mut b = SigBuilder::new(&mut arena);
        let read_index = b.input(0);
        let size = b.int(8);
        let generator = b.real(0.0);
        let readonly = b.wrtbl_readonly(size, generator);
        let readonly_read = b.rdtbl(readonly, read_index);
        let mutable_size = b.int(16);
        let mutable_generator = b.real(0.0);
        let write_index = b.input(3);
        let write_value = b.input(4);
        let mutable = b.wrtbl(mutable_size, mutable_generator, write_index, write_value);
        let mutable_read = b.rdtbl(mutable, read_index);
        let first = b.output(0, readonly_read);
        let second = b.output(1, mutable_read);
        (
            readonly,
            mutable,
            readonly_read,
            mutable_read,
            vec![first, second],
        )
    };
    let types = sigtype::TypeAnnotator::new(&arena, &ui::UiProgram::empty())
        .annotate(&outputs)
        .unwrap();
    let analysis = SignalAnalysisContext::new(&arena, &types, &outputs).unwrap();
    assert!(
        direct_effects(&analysis, readonly).unwrap().is_empty(),
        "a read-only table has no live writer and so no compute-time write effect"
    );
    assert_eq!(
        direct_effects(&analysis, mutable).unwrap(),
        BTreeSet::from([EffectAtom::WriteTable(mutable.as_u32())]),
        "a table with live write ports keeps its write effect"
    );
    assert_eq!(
        direct_effects(&analysis, readonly_read).unwrap(),
        BTreeSet::from([EffectAtom::ReadTable(readonly.as_u32())]),
        "reads are unchanged by the writer classification"
    );
    assert_eq!(
        direct_effects(&analysis, mutable_read).unwrap(),
        BTreeSet::from([EffectAtom::ReadTable(mutable.as_u32())])
    );
}

#[test]
fn effects_use_stable_resources_and_propagate_to_the_root() {
    let mut arena = TreeArena::new();
    let (input, delay, delay_long, table, read, output) = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.real(1.0);
        let delay = b.delay1(input);
        let two = b.int(2);
        let delay_long = b.delay(input, two);
        let size = b.int(16);
        let write_index = b.input(1);
        let table = b.wrtbl(size, delay, write_index, delay_long);
        let read_index = b.input(2);
        let read = b.rdtbl(table, read_index);
        let output = b.output(3, read);
        (input, delay, delay_long, table, read, output)
    };
    let types = sigtype::TypeAnnotator::new(&arena, &ui::UiProgram::empty())
        .annotate(&[output])
        .unwrap();
    let analysis = SignalAnalysisContext::new(&arena, &types, &[output]).unwrap();
    let uses = analyze_forest(
        &analysis,
        &[output],
        |_| Some(None),
        &ConstantExecutionConditions::default(),
    )
    .unwrap();

    let delay_resource = StateResource::Signal {
        owner: input.as_u32(),
        cell: StateCell::Delay,
    };
    assert!(
        uses.get(delay)
            .unwrap()
            .effects
            .contains(&EffectAtom::WriteState(delay_resource.clone()))
    );
    assert_eq!(
        direct_effects(&analysis, delay).unwrap(),
        direct_effects(&analysis, delay_long).unwrap(),
        "all history readers of one signal share its delay resource"
    );
    assert_eq!(
        direct_effects(&analysis, table).unwrap(),
        BTreeSet::from([EffectAtom::WriteTable(table.as_u32())])
    );
    assert_eq!(
        direct_effects(&analysis, read).unwrap(),
        BTreeSet::from([EffectAtom::ReadTable(table.as_u32())])
    );
    let root_effects = &uses.get(output).unwrap().effects;
    for expected in [
        EffectAtom::ReadState(delay_resource.clone()),
        EffectAtom::WriteState(delay_resource),
        EffectAtom::ReadTable(table.as_u32()),
        EffectAtom::WriteTable(table.as_u32()),
        EffectAtom::WriteOutput(3),
    ] {
        assert!(root_effects.contains(&expected), "missing {expected:?}");
    }
    assert!(root_effects.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn scalar_effect_analysis_preserves_vector_direct_effect_facts() {
    let mut arena = TreeArena::new();
    let output = {
        let mut b = SigBuilder::new(&mut arena);
        let input = b.input(0);
        let delayed = b.delay1(input);
        b.output(0, delayed)
    };
    let prepared =
        prepare_signals_for_fir_verified(&arena, &[output], &ui::UiProgram::empty()).unwrap();
    let clocks = annotate(
        prepared.arena(),
        &ClockDomainTable::new(),
        prepared.outputs(),
    )
    .unwrap();

    let vector = analyze_vector_signals(&prepared, &clocks).unwrap();
    let scalar = analyze_scalar_scheduling_effects(&prepared).unwrap();
    for vector_record in vector.uses.records() {
        assert_eq!(
            vector_record.info.direct_effects.as_slice(),
            scalar.direct_effects(vector_record.sig),
            "scalar scheduling must preserve direct effect facts for signal {}",
            vector_record.sig.as_u32()
        );
    }
}

#[test]
fn effect_propagation_work_scales_linearly_on_a_deep_chain() {
    const SIGNALS: u32 = 512;
    let mut direct = (0..SIGNALS)
        .map(|signal| (signal, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    direct
        .get_mut(&(SIGNALS - 1))
        .unwrap()
        .insert(EffectAtom::WriteOutput(0));
    let parents = (1..SIGNALS)
        .map(|child| (child, BTreeSet::from([child - 1])))
        .collect::<BTreeMap<_, _>>();

    let (accumulated, updates) = propagate_effect_sets(&direct, &parents);

    assert_eq!(
        accumulated[&0],
        BTreeSet::from([EffectAtom::WriteOutput(0)])
    );
    assert_eq!(updates, usize::try_from(SIGNALS - 1).unwrap());
}

#[test]
fn foreign_identity_and_effect_conflicts_are_conservative() {
    let mut arena = TreeArena::new();
    let call = {
        let int_type = arena.int(0);
        let real_type = arena.int(1);
        let name_f32 = arena.symbol("probe_f");
        let name_f64 = arena.symbol("probe");
        let names = vec_to_list(&mut arena, &[name_f32, name_f64]);
        let signature = vec_to_list(&mut arena, &[int_type, names, real_type]);
        let include = arena.symbol("<probe.h>");
        let library = arena.symbol("");
        let tag = arena.intern_tag("FFUN");
        let descriptor = arena.intern(NodeKind::Tag(tag), &[signature, include, library]);
        let input = SigBuilder::new(&mut arena).input(0);
        let args = vec_to_list(&mut arena, &[input]);
        SigBuilder::new(&mut arena).ffun(descriptor, args)
    };
    let types = sigtype::TypeAnnotator::new(&arena, &ui::UiProgram::empty())
        .annotate(&[call])
        .unwrap();
    let analysis = SignalAnalysisContext::new(&arena, &types, &[call]).unwrap();
    let effects = direct_effects(&analysis, call).unwrap();
    let foreign = effects.iter().next().expect("one foreign effect");
    let EffectAtom::Foreign {
        resource: ForeignResource::Function(signature),
        purity: ForeignPurity::Unknown,
    } = foreign
    else {
        panic!("foreign call must remain an unknown-purity effect");
    };
    assert_eq!(signature.names, ["probe_f", "probe"]);
    assert_eq!(signature.return_type, ForeignTypeCode(0));
    assert_eq!(signature.arguments, [ForeignTypeCode(1)]);
    assert!(effects_conflict(foreign, &EffectAtom::WriteOutput(0)));

    let state = StateResource::Signal {
        owner: 10,
        cell: StateCell::Delay,
    };
    assert!(!effects_conflict(
        &EffectAtom::ReadState(state.clone()),
        &EffectAtom::ReadState(state.clone())
    ));
    assert!(effects_conflict(
        &EffectAtom::ReadState(state.clone()),
        &EffectAtom::WriteState(state)
    ));
    assert!(!effects_conflict(
        &EffectAtom::WriteTable(1),
        &EffectAtom::ReadTable(2)
    ));
    let pure = EffectAtom::Foreign {
        resource: ForeignResource::Function(signature.clone()),
        purity: ForeignPurity::Pure,
    };
    assert!(!effects_conflict(&pure, &EffectAtom::WriteOutput(0)));
}
