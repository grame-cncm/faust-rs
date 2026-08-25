//! Signal use aggregation (`SignalUseTable`): occurrence counting per
//! context with C++ occurrence semantics.

use super::AnalysisError;
use super::conditions::*;
use super::dependencies::*;
use super::effects::*;
use crate::clk_env::{ClkEnv, ClkEnvMap};
use crate::signal_prepare::VerifiedPreparedSignals;
use signals::{BinOp, SigId, SigMatch, match_sig};
use sigtype::{SigType, Variability, Vectorability};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tlib::{match_sym_rec, match_sym_ref};

/// A signal use's rate and execution condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UseContext {
    /// Rate at which this use is demanded.
    pub variability: Variability,
    /// Recursive depth of the consumer context, saturated to one by the C++
    /// extended-variability rule.
    pub recursiveness: u32,
    /// Execution condition supplied by [`ExecutionConditions`].
    pub condition: CondId,
}
/// Occurrence/use facts for one reachable prepared signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalUseInfo {
    /// Full type copied from the verified preparation boundary.
    pub sig_type: SigType,
    /// Cached `sig_type.variability()`.
    pub variability: Variability,
    /// Cached `sig_type.vectorability()`.
    pub vectorability: Vectorability,
    /// Inferred clock environment copied from [`ClkEnvMap`].
    pub clk_env: ClkEnv,
    /// Recursive depth used by C++ extended variability.
    pub recursiveness: u32,
    /// Canonical execution condition attached to this signal.
    pub execution_condition: CondId,
    /// Deterministic uses grouped by context.
    pub occurrences: OccInfo,
    /// Largest fixed delay amount of a delayed reader.
    pub max_delay: u32,
    /// Number of delayed reads of this signal.
    pub delay_reads: u32,
    /// Whether at least one use is outside a delay.
    pub has_out_delay_occurrence: bool,
    /// Whether this node is itself a general `sigDelay` read.
    pub is_delay_read: bool,
    /// Whether this node is a structural `SYMREC`/`SYMREF` tuple carrier.
    pub is_symbolic_recursion_carrier: bool,
    /// Projection facts when this signal is a symbolic recursion projection.
    pub recursive_projection: Option<RecursiveProjection>,
    /// Exactly `Int | Real | Input | FConst`.
    pub very_simple: bool,
    /// Sorted conservative compute-time effects, including non-`Gen` children.
    pub effects: Vec<EffectAtom>,
    /// Sorted effects performed by this node itself, excluding child effects.
    ///
    /// This projection lets scalar scheduling and the vector event model
    /// orient actual effect operations without paying a quadratic cost over
    /// every transitive effect carrier in the signal graph. It is always a
    /// sorted subset of `effects`.
    pub direct_effects: Vec<EffectAtom>,
}
/// Deterministic record pairing a `SigId` with its occurrence/use facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalUseRecord {
    /// Signal identity.
    pub sig: SigId,
    /// Its analysis facts.
    pub info: SignalUseInfo,
}
/// Deterministic analysis output: records by numeric `SigId`, dependencies by
/// numeric source `SigId` then source-local `edge_key`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalUseTable {
    records: Vec<SignalUseRecord>,
    dependencies: Vec<AnalysisDependency>,
    occurrence_dependencies: Vec<OccurrenceUse>,
}
impl SignalUseTable {
    /// Records in increasing numeric `SigId` order.
    #[must_use]
    pub fn records(&self) -> &[SignalUseRecord] {
        &self.records
    }

    /// Decoded dependencies in deterministic source/edge-key order.
    #[must_use]
    pub fn dependencies(&self) -> &[AnalysisDependency] {
        &self.dependencies
    }

    /// Decoded occurrence uses in deterministic source/edge-key order.
    #[must_use]
    pub fn occurrence_dependencies(&self) -> &[OccurrenceUse] {
        &self.occurrence_dependencies
    }

    /// Looks up one record without requiring `SigId: Ord` in the public API.
    #[must_use]
    pub fn get(&self, sig: SigId) -> Option<&SignalUseInfo> {
        self.records
            .binary_search_by_key(&sig.as_u32(), |record| record.sig.as_u32())
            .ok()
            .map(|index| &self.records[index].info)
    }
}
/// Builds deterministic occurrence/effect facts with an injected condition
/// provider. Production clients should prefer [`analyze_vector_signals`](super::analyze_vector_signals); this
/// lower-level entry point remains useful for rule tests and formal mutations.
pub fn analyze_signal_uses(
    prepared: &VerifiedPreparedSignals,
    clk_envs: &ClkEnvMap,
    conditions: &impl ExecutionConditions,
) -> Result<SignalUseTable, AnalysisError> {
    let context = SignalAnalysisContext::new(
        prepared.arena(),
        prepared.sig_types_map(),
        prepared.outputs(),
    )?;
    analyze_forest(
        &context,
        prepared.outputs(),
        |sig| clk_envs.env(sig),
        conditions,
    )
}
pub(super) fn analyze_forest(
    analysis: &SignalAnalysisContext<'_>,
    roots: &[SigId],
    clk_env: impl Fn(SigId) -> Option<ClkEnv>,
    conditions: &impl ExecutionConditions,
) -> Result<SignalUseTable, AnalysisError> {
    let timing_enabled = std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some();
    let mut stage_started = std::time::Instant::now();
    let mut trace_stage = |stage: &str| {
        if timing_enabled {
            eprintln!(
                "[vector-uses-stage] {stage}: {:.3}s",
                stage_started.elapsed().as_secs_f64()
            );
        }
        stage_started = std::time::Instant::now();
    };
    let (recursiveness, _) = compute_recursiveness(analysis, roots)?;
    trace_stage("recursiveness");
    let mut records = BTreeMap::<u32, SignalUseRecord>::new();
    let mut dependency_cache = BTreeMap::<u32, SignalDependencies>::new();
    let mut expanded_signals = BTreeSet::<u32>::new();
    let mut work = VecDeque::<(SigId, UseContext, u32)>::new();

    for &root in roots {
        work.push_back((
            root,
            UseContext {
                variability: Variability::Samp,
                recursiveness: 0,
                condition: conditions.root_condition(root),
            },
            0,
        ));
    }

    while let Some((sig, use_context, delay)) = work.pop_front() {
        ensure_record(
            &mut records,
            analysis,
            sig,
            &recursiveness,
            &clk_env,
            conditions,
        )?;
        increment_occurrence(
            records.get_mut(&sig.as_u32()).expect("record inserted"),
            use_context,
            delay,
        );

        // C++ OccMarkup increments every use but recursively marks children
        // only on the signal's first visit. In particular, a second context on
        // a shared signal does not leak into all of that signal's descendants.
        let first_visit = expanded_signals.insert(sig.as_u32());
        if !first_visit {
            if let SigMatch::BinOp(BinOp::Mul, left, right) = match_sig(analysis.arena, sig)
                && matches!(match_sig(analysis.arena, left), SigMatch::Int(-1))
            {
                // C++ propagates repeated `-1 * y` uses because codegen ignores
                // sharing of the negation wrapper itself.
                work.push_back((right, use_context, delay));
            }
            continue;
        }
        if let std::collections::btree_map::Entry::Vacant(entry) =
            dependency_cache.entry(sig.as_u32())
        {
            entry.insert(signal_dependencies(analysis, sig)?);
        }
        let dependencies = dependency_cache
            .get(&sig.as_u32())
            .expect("dependencies inserted")
            .clone();
        let parent = &records.get(&sig.as_u32()).expect("record inserted").info;
        let child_context = UseContext {
            // C++ OccMarkup passes the current node's inferred variability
            // and recursiveness, not those inherited by this use.
            variability: parent.variability,
            recursiveness: parent.recursiveness,
            condition: conditions.signal_condition(sig),
        };

        for occurrence in dependencies.occurrences() {
            work.push_back((occurrence.to, child_context, occurrence.delay));
        }
    }
    trace_stage("occurrences-and-dependencies");

    decorate_effects(analysis, &mut records, &dependency_cache)?;
    trace_stage("effects");

    let mut dependencies = dependency_cache
        .values()
        .flat_map(|projection| projection.scheduling.iter().copied())
        .collect::<Vec<_>>();
    dependencies.sort_by_key(|dependency| (dependency.from.as_u32(), dependency.edge_key));
    let mut occurrence_dependencies = dependency_cache
        .into_values()
        .flat_map(|projection| projection.occurrences)
        .collect::<Vec<_>>();
    occurrence_dependencies
        .sort_by_key(|dependency| (dependency.from.as_u32(), dependency.edge_key));
    for record in records.values_mut() {
        finalize_occurrences(&mut record.info);
    }
    trace_stage("canonicalization");
    Ok(SignalUseTable {
        records: records.into_values().collect(),
        dependencies,
        occurrence_dependencies,
    })
}
pub(super) fn ensure_record(
    records: &mut BTreeMap<u32, SignalUseRecord>,
    analysis: &SignalAnalysisContext<'_>,
    sig: SigId,
    recursiveness: &BTreeMap<u32, u32>,
    clk_env: &impl Fn(SigId) -> Option<ClkEnv>,
    conditions: &impl ExecutionConditions,
) -> Result<(), AnalysisError> {
    if records.contains_key(&sig.as_u32()) {
        return Ok(());
    }
    let sig_type = analysis.sig_type(sig)?.clone();
    let clk_env = clk_env(sig).ok_or(AnalysisError::MissingClock { sig })?;
    let recursive_projection = match match_sig(analysis.arena, sig) {
        SigMatch::Proj(index, group) if index < 0 => {
            return Err(AnalysisError::InvalidRecursiveProjection { sig, index, group });
        }
        SigMatch::Proj(index, group_ref) => {
            let group = analysis.resolve_rec_group(group_ref).unwrap_or(group_ref);
            Some(RecursiveProjection {
                index: usize::try_from(index).expect("nonnegative i32 fits usize"),
                group,
            })
        }
        _ => None,
    };
    let very_simple = matches!(
        match_sig(analysis.arena, sig),
        SigMatch::Int(_) | SigMatch::Real(_) | SigMatch::Input(_) | SigMatch::FConst(_, _, _)
    );
    records.insert(
        sig.as_u32(),
        SignalUseRecord {
            sig,
            info: SignalUseInfo {
                variability: sig_type.variability(),
                vectorability: sig_type.vectorability(),
                sig_type,
                clk_env,
                recursiveness: recursiveness.get(&sig.as_u32()).copied().unwrap_or(0),
                execution_condition: conditions.signal_condition(sig),
                occurrences: OccInfo::default(),
                max_delay: 0,
                delay_reads: 0,
                has_out_delay_occurrence: false,
                is_delay_read: matches!(match_sig(analysis.arena, sig), SigMatch::Delay(_, _)),
                is_symbolic_recursion_carrier: match_sym_rec(analysis.arena, sig).is_some()
                    || match_sym_ref(analysis.arena, sig).is_some(),
                recursive_projection,
                very_simple,
                effects: Vec::new(),
                direct_effects: Vec::new(),
            },
        },
    );
    Ok(())
}
pub(super) fn increment_occurrence(record: &mut SignalUseRecord, context: UseContext, delay: u32) {
    if delay == 0 {
        record.info.has_out_delay_occurrence = true;
    } else {
        record.info.max_delay = record.info.max_delay.max(delay);
        record.info.delay_reads = record.info.delay_reads.saturating_add(1);
    }
    if let Some(occurrence) = record
        .info
        .occurrences
        .per_context
        .iter_mut()
        .find(|occurrence| occurrence.context == context)
    {
        occurrence.count = occurrence.count.saturating_add(1);
        return;
    }
    record
        .info
        .occurrences
        .per_context
        .push(ContextOccurrence { context, count: 1 });
}
pub(super) fn finalize_occurrences(info: &mut SignalUseInfo) {
    info.occurrences
        .per_context
        .sort_by_key(|occurrence| occurrence.context);
    let own_variability = extended_variability(info.variability, info.recursiveness);
    let mut counts = [0_u32; 4];
    for occurrence in &info.occurrences.per_context {
        let context_variability = extended_variability(
            occurrence.context.variability,
            occurrence.context.recursiveness,
        );
        counts[usize::from(context_variability)] =
            counts[usize::from(context_variability)].saturating_add(occurrence.count);
        if context_variability > own_variability
            || occurrence.context.condition != info.execution_condition
        {
            info.occurrences.multi = true;
        }
    }
    info.occurrences.multi |= counts.into_iter().any(|count| count > 1);
}
pub(super) fn extended_variability(variability: Variability, recursiveness: u32) -> u8 {
    let variability = match variability {
        Variability::Konst => 0,
        Variability::Block => 1,
        Variability::Samp => 2,
    };
    (variability + u8::from(recursiveness > 0)).min(3)
}
