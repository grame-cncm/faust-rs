//! Production construction of the strategy-independent vector plan:
//! placement, ordinary dependency edges, effect orientation, and the
//! type/rate conversions the producer records.

use super::fusion::*;
use super::producer_reachability::*;
use super::{EFFECT_ISLAND_TAG, VectorPlanBuildError, VerifiedVectorPlan};
use crate::signal_fir::decoration_verify::{
    CanonicalSigType, DecorationRecord, DependencyFact, VerifiedDecorationCertificate,
};
use crate::signal_fir::loop_graph::{LoopSeparation, SignalLoopProps, needs_separate_loop};
use crate::signal_fir::vector::analysis::{DepKind, EffectAtom, ForeignPurity, StateResource};
use crate::signal_fir::vector::lockstep::{detect_lockstep_bundles, verify_lockstep_isomorphism};
use crate::signal_fir::vector::verify::{
    EpochRecord, FusedSerialGroupRecord, LoopEdge, LoopKind, LoopRecord, Placement, Rate,
    SignalRecord, TransportLayout, TransportRecord, VECTOR_PLAN_SCHEMA_VERSION, ValueType,
    VecSafeWitness, VectorPlan, Vectorability, WitnessKind, effects_duplicable,
    effects_sample_reorderable, verify_fused_serial_groups_after_plan, verify_vector_plan,
};
use crate::signal_prepare::VerifiedPreparedSignals;
use sigtype::{Nature, Variability, Vectorability as SigVectorability};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) struct PlacementState<'a> {
    pub(super) records: BTreeMap<u32, &'a DecorationRecord>,
    pub(super) children: BTreeMap<u32, Vec<u32>>,
    pub(super) sample_required: BTreeSet<u32>,
    pub(super) delayed_pairs: BTreeSet<(u32, u32)>,
    pub(super) structural_carriers: BTreeSet<u32>,
    pub(super) placement: BTreeMap<u32, Placement>,
    pub(super) contexts: BTreeMap<u32, BTreeSet<u64>>,
    pub(super) roots_by_loop: BTreeMap<u64, BTreeSet<u64>>,
    pub(super) visited: BTreeSet<(u32, u64)>,
}
impl<'a> PlacementState<'a> {
    fn visit(&mut self, signal_id: u32, current_loop: u64) -> Result<(), VectorPlanBuildError> {
        let record = self
            .records
            .get(&signal_id)
            .copied()
            .ok_or(VectorPlanBuildError::MissingRecord { signal_id })?;
        if self.structural_carriers.contains(&signal_id) {
            // The carrier itself is structural, but its executable children
            // enter the sample closure in the caller's context - the module
            // contract this traversal previously broke by stopping here, which
            // left every signal reachable only through a carrier without an
            // execution context. Carrier chains are cyclic (`SYMREC` bodies
            // reference `SYMREF` of their own group), so the visited guard
            // applies to carriers too.
            self.placement.insert(signal_id, Placement::Inline);
            if !self.visited.insert((signal_id, current_loop)) {
                return Ok(());
            }
            let children = self.children.get(&signal_id).cloned().unwrap_or_default();
            for child in children {
                self.visit(child, current_loop)?;
            }
            return Ok(());
        }
        if !self.sample_required.contains(&signal_id) {
            self.placement.insert(signal_id, Placement::Control);
            return Ok(());
        }

        let execution_loop = match self.placement.get(&signal_id).copied() {
            Some(Placement::Owned(owner)) => owner,
            Some(Placement::Inline) => current_loop,
            Some(Placement::Control) => return Ok(()),
            None if effects_duplicable(&record.effects) => {
                self.placement.insert(signal_id, Placement::Inline);
                current_loop
            }
            None => {
                self.placement
                    .insert(signal_id, Placement::Owned(current_loop));
                self.roots_by_loop
                    .entry(current_loop)
                    .or_default()
                    .insert(u64::from(signal_id));
                current_loop
            }
        };
        self.contexts
            .entry(signal_id)
            .or_default()
            .insert(execution_loop);
        if !self.visited.insert((signal_id, execution_loop)) {
            return Ok(());
        }
        let children = self.children.get(&signal_id).cloned().unwrap_or_default();
        for child in children {
            self.visit(child, execution_loop)?;
        }
        Ok(())
    }
}
/// Facts derived once from the accepted decoration certificate: the delayed
/// value/pair sets, the record index, recursion groups, structural carriers,
/// the sample-required closure, and each signal's loop-separation verdict.
struct CertificateFacts<'a> {
    /// Decoration records by signal id.
    records: BTreeMap<u32, &'a crate::signal_fir::decoration_verify::DecorationRecord>,
    /// Symbolic recursion groups observed among recursive projections.
    recursion_groups: BTreeSet<u32>,
    /// Symbolic recursion carriers (structural, never materialized).
    structural_carriers: BTreeSet<u32>,
    /// Signals that must execute in sample time (transitive closure).
    sample_required: BTreeSet<u32>,
    /// `(from, to)` occurrence pairs read with a positive delay.
    delayed_pairs: BTreeSet<(u32, u32)>,
    /// Loop-separation verdict per signal.
    separations: BTreeMap<u32, LoopSeparation>,
}

impl<'a> CertificateFacts<'a> {
    fn derive(
        certificate: &'a crate::signal_fir::decoration_verify::DecorationCertificate,
    ) -> Self {
        let delayed_values = certificate
            .occurrence_dependencies
            .iter()
            .filter_map(|dependency| (dependency.delay > 0).then_some(dependency.to))
            .collect::<BTreeSet<_>>();
        let delayed_pairs = certificate
            .occurrence_dependencies
            .iter()
            .filter_map(|dependency| {
                (dependency.delay > 0).then_some((dependency.from, dependency.to))
            })
            .collect::<BTreeSet<_>>();
        let records = certificate
            .records
            .iter()
            .map(|record| (record.signal_id, record))
            .collect::<BTreeMap<_, _>>();
        let recursion_groups = certificate
            .records
            .iter()
            .filter_map(|record| {
                record
                    .recursive_projection
                    .map(|projection| projection.group)
            })
            .collect::<BTreeSet<_>>();
        let structural_carriers = certificate
            .records
            .iter()
            .filter(|record| record.is_symbolic_recursion_carrier)
            .map(|record| record.signal_id)
            .collect::<BTreeSet<_>>();
        let mut sample_required = certificate
            .records
            .iter()
            .filter(|record| {
                requires_sample_execution(
                    record,
                    delayed_values.contains(&record.signal_id),
                    structural_carriers.contains(&record.signal_id),
                )
            })
            .map(|record| record.signal_id)
            .collect::<BTreeSet<_>>();
        loop {
            let previous = sample_required.len();
            let additions = certificate
                .occurrence_dependencies
                .iter()
                .filter_map(|dependency| {
                    sample_required
                        .contains(&dependency.to)
                        .then_some(dependency.from)
                })
                .chain(certificate.dependencies.iter().filter_map(|dependency| {
                    sample_required
                        .contains(&dependency.to)
                        .then_some(dependency.from)
                }))
                .filter(|signal_id| {
                    !structural_carriers.contains(signal_id)
                        && records.get(signal_id).is_some_and(|record| {
                            !matches!(record.sig_type, CanonicalSigType::Table { .. })
                        })
                })
                .collect::<Vec<_>>();
            sample_required.extend(additions);
            if sample_required.len() == previous {
                break;
            }
        }
        let separations = certificate
            .records
            .iter()
            .map(|record| {
                let props = SignalLoopProps {
                    variability: record.variability,
                    max_delay: record.max_delay as usize,
                    is_recursive_proj: record.recursive_projection.is_some(),
                    is_shared: record.occurrences.multi,
                    is_delay_read: record.is_delay_read,
                    is_very_simple: record.very_simple,
                };
                (record.signal_id, needs_separate_loop(&props))
            })
            .collect::<BTreeMap<_, _>>();
        Self {
            records,
            recursion_groups,
            structural_carriers,
            sample_required,
            delayed_pairs,
            separations,
        }
    }
}

/// Loop identities allocated from the facts alone: the optional shared root
/// loop, one serial loop per recursion group, one vectorizable loop per
/// separated signal, and the pre-seeded owner placements.
struct LoopAllocation {
    /// Next unallocated loop id (later placement stages may allocate more).
    next_loop: u64,
    /// The shared loop for inline sample roots, when one is needed.
    root_loop: Option<u64>,
    /// Serial loop id per recursion group.
    recursion_loop: BTreeMap<u32, u64>,
    /// Pre-seeded signal-to-loop ownership.
    owner: BTreeMap<u32, u64>,
}

/// Allocates loop identities from the derived facts (no traversal).
fn allocate_loops(
    certificate: &crate::signal_fir::decoration_verify::DecorationCertificate,
    facts: &CertificateFacts<'_>,
) -> LoopAllocation {
    let CertificateFacts {
        records,
        recursion_groups,
        sample_required,
        separations,
        ..
    } = facts;
    let inline_sample_root = certificate.roots.iter().any(|root| {
        records.get(root).is_some_and(|_record| {
            sample_required.contains(root) && separations[root] == LoopSeparation::Inline
        })
    });
    let mut next_loop = 0_u64;
    let root_loop = inline_sample_root.then(|| {
        let id = next_loop;
        next_loop += 1;
        id
    });

    let mut recursion_loop = BTreeMap::new();
    for &group in recursion_groups {
        recursion_loop.insert(group, next_loop);
        next_loop += 1;
    }

    let mut owner = BTreeMap::<u32, u64>::new();
    for record in &certificate.records {
        if sample_required.contains(&record.signal_id)
            && !certificate.lifecycle_boundaries.contains(&record.signal_id)
            && separations[&record.signal_id] == LoopSeparation::SeparateSerial
        {
            let group = record
                .recursive_projection
                .expect("serial separation is a recursive projection")
                .group;
            owner.insert(record.signal_id, recursion_loop[&group]);
        }
    }
    for record in &certificate.records {
        if sample_required.contains(&record.signal_id)
            && !certificate.lifecycle_boundaries.contains(&record.signal_id)
            && separations[&record.signal_id] == LoopSeparation::SeparateVectorizable
        {
            owner.insert(record.signal_id, next_loop);
            next_loop += 1;
        }
    }

    LoopAllocation {
        next_loop,
        root_loop,
        recursion_loop,
        owner,
    }
}

/// C++ OccMarkup expands a shared node's children only on its first visit,
/// so its canonical occurrence projection can contain a compute-visible
/// effect component disconnected from the output roots (an `attach`-only
/// bargraph branch, for example). Materialize only the maximal roots of each
/// such component; visiting them assigns their complete closure without
/// duplicating descendant effects.
fn place_disconnected_effect_components(
    certificate: &crate::signal_fir::decoration_verify::DecorationCertificate,
    state: &mut PlacementState<'_>,
    next_loop: &mut u64,
) -> Result<(), VectorPlanBuildError> {
    loop {
        let unplaced = certificate
            .records
            .iter()
            .filter(|record| {
                state.sample_required.contains(&record.signal_id)
                    && !certificate.lifecycle_boundaries.contains(&record.signal_id)
                    && !state.placement.contains_key(&record.signal_id)
            })
            .map(|record| record.signal_id)
            .collect::<BTreeSet<_>>();
        if unplaced.is_empty() {
            break;
        }
        let descendants = unplaced
            .iter()
            .flat_map(|signal| state.children.get(signal).into_iter().flatten())
            .filter(|child| unplaced.contains(child))
            .copied()
            .collect::<BTreeSet<_>>();
        let component_roots = unplaced
            .difference(&descendants)
            .copied()
            .collect::<Vec<_>>();
        if component_roots.is_empty() {
            break;
        }
        let mut made_progress = false;
        for root in component_roots {
            let record = state.records[&root];
            if effects_duplicable(&record.effects) {
                state.placement.insert(root, Placement::Inline);
                made_progress = true;
                continue;
            }
            let loop_id = *next_loop;
            *next_loop += 1;
            state.placement.insert(root, Placement::Owned(loop_id));
            state
                .roots_by_loop
                .entry(loop_id)
                .or_default()
                .insert(u64::from(root));
            state.visit(root, loop_id)?;
            made_progress = true;
        }
        if !made_progress {
            break;
        }
    }
    Ok(())
}

/// Places every sample-required signal into a loop (or inline/control), by
/// visiting the certified dependency graph from the roots, then the
/// disconnected effect components, then the pre-seeded owners — and fails
/// closed if any sample signal remains unplaced.
fn place_signals<'a>(
    certificate: &'a crate::signal_fir::decoration_verify::DecorationCertificate,
    facts: CertificateFacts<'a>,
    alloc: &LoopAllocation,
    next_loop: &mut u64,
    timing_enabled: bool,
) -> Result<PlacementState<'a>, VectorPlanBuildError> {
    let CertificateFacts {
        records,
        structural_carriers,
        sample_required,
        delayed_pairs,
        ..
    } = facts;
    let owner = &alloc.owner;
    let root_loop = alloc.root_loop;
    let mut children = BTreeMap::<u32, Vec<(u64, u32)>>::new();
    for occurrence in &certificate.occurrence_dependencies {
        children
            .entry(occurrence.from)
            .or_default()
            .push((occurrence.edge_key, occurrence.to));
    }
    for dependency in &certificate.dependencies {
        children
            .entry(dependency.from)
            .or_default()
            .push((dependency.edge_key, dependency.to));
    }
    let children = children
        .into_iter()
        .map(|(signal, mut edges)| {
            edges.sort_unstable();
            edges.dedup();
            (signal, edges.into_iter().map(|(_, child)| child).collect())
        })
        .collect();
    let mut state = PlacementState {
        records,
        children,
        sample_required: sample_required.clone(),
        delayed_pairs,
        structural_carriers: structural_carriers.clone(),
        placement: BTreeMap::new(),
        contexts: BTreeMap::new(),
        roots_by_loop: BTreeMap::new(),
        visited: BTreeSet::new(),
    };
    for record in &certificate.records {
        if structural_carriers.contains(&record.signal_id) {
            state.placement.insert(record.signal_id, Placement::Inline);
        } else if !sample_required.contains(&record.signal_id) {
            state.placement.insert(record.signal_id, Placement::Control);
        }
    }
    for &boundary in &certificate.lifecycle_boundaries {
        state.placement.insert(boundary, Placement::Control);
    }
    for (&signal, &loop_id) in owner.iter() {
        state.placement.insert(signal, Placement::Owned(loop_id));
        state
            .roots_by_loop
            .entry(loop_id)
            .or_default()
            .insert(u64::from(signal));
    }
    for &root in &certificate.roots {
        let _record = state
            .records
            .get(&root)
            .copied()
            .ok_or(VectorPlanBuildError::MissingRecord { signal_id: root })?;
        if !sample_required.contains(&root) {
            continue;
        }
        let loop_id = match state.placement.get(&root).copied() {
            Some(Placement::Owned(loop_id)) => loop_id,
            Some(Placement::Inline) | None => {
                // A preceding output traversal may already have visited this
                // pure root inline. Its top-level use still needs a concrete
                // producer in the shared root loop; revisiting it records both
                // execution contexts and therefore the required transport.
                let loop_id = root_loop.expect("an inline sample root allocated the root loop");
                state.placement.insert(root, Placement::Owned(loop_id));
                state
                    .roots_by_loop
                    .entry(loop_id)
                    .or_default()
                    .insert(u64::from(root));
                loop_id
            }
            Some(Placement::Control) => {
                return Err(VectorPlanBuildError::SampleSignalUnplaced { signal_id: root });
            }
        };
        state.visit(root, loop_id)?;
    }
    place_disconnected_effect_components(certificate, &mut state, next_loop)?;
    // Pre-seeded `owner` placements bypass the traversal entirely: a signal
    // owning a separate loop that no root path reaches would keep a placement
    // with no execution context, and every later stage that reads `contexts`
    // for it would fail. Visit each one rooted at its own loop; the visited
    // guard makes this a no-op for signals the traversal already covered.
    let preseeded = state
        .placement
        .iter()
        .filter_map(|(&signal, &placement)| match placement {
            Placement::Owned(loop_id) if !state.contexts.contains_key(&signal) => {
                Some((signal, loop_id))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for (signal, loop_id) in preseeded {
        state.visit(signal, loop_id)?;
    }
    for record in &certificate.records {
        if sample_required.contains(&record.signal_id)
            && !certificate.lifecycle_boundaries.contains(&record.signal_id)
            && !state.placement.contains_key(&record.signal_id)
        {
            if timing_enabled {
                let incoming = certificate
                    .occurrence_dependencies
                    .iter()
                    .filter(|dependency| dependency.to == record.signal_id)
                    .count();
                let outgoing = certificate
                    .occurrence_dependencies
                    .iter()
                    .filter(|dependency| dependency.from == record.signal_id)
                    .count();
                let parents = certificate
                    .occurrence_dependencies
                    .iter()
                    .filter(|dependency| dependency.to == record.signal_id)
                    .map(|dependency| {
                        (
                            dependency.from,
                            state.placement.get(&dependency.from).copied(),
                        )
                    })
                    .collect::<Vec<_>>();
                eprintln!(
                    "[vector-plan-unplaced] signal={} type={:?} effects={:?} incoming={} outgoing={} parents={:?} lifecycle_boundary={}",
                    record.signal_id,
                    record.sig_type,
                    record.effects,
                    incoming,
                    outgoing,
                    parents,
                    certificate.lifecycle_boundaries.contains(&record.signal_id)
                );
            }
            return Err(VectorPlanBuildError::SampleSignalUnplaced {
                signal_id: record.signal_id,
            });
        }
    }

    Ok(state)
}

/// Collects the cross-loop uses and the delayed/immediate/effect loop edges
/// from the certified dependencies and delay-0 occurrences.
#[allow(clippy::type_complexity)]
fn derive_dependency_edges(
    certificate: &crate::signal_fir::decoration_verify::DecorationCertificate,
    state: &PlacementState<'_>,
) -> Result<
    (
        BTreeSet<(u32, u64, u64)>,
        BTreeSet<LoopEdge>,
        BTreeSet<LoopEdge>,
        BTreeSet<LoopEdge>,
    ),
    VectorPlanBuildError,
> {
    let mut cross_uses = BTreeSet::<(u32, u64, u64)>::new();
    let mut delayed_edges = BTreeSet::<LoopEdge>::new();
    let mut immediate_delay_edges = BTreeSet::<LoopEdge>::new();
    let mut effect_edges = BTreeSet::<LoopEdge>::new();
    for dependency in &certificate.dependencies {
        add_dependency_edges(
            dependency,
            &state,
            &mut cross_uses,
            &mut delayed_edges,
            &mut immediate_delay_edges,
            &mut effect_edges,
        )?;
    }
    // Pairs whose schedule edge is `Effect` are pure ordering: `attach`'s
    // forcing edge keeps its delay-0 occurrence for record coverage and scalar
    // occurrence facts, but must not plan a value transport nobody loads.
    let ordering_only_pairs = certificate
        .dependencies
        .iter()
        .filter(|dependency| matches!(dependency.kind, DepKind::Effect))
        .map(|dependency| (dependency.from, dependency.to))
        .collect::<BTreeSet<_>>();
    for occurrence in certificate
        .occurrence_dependencies
        .iter()
        .filter(|occurrence| occurrence.delay == 0)
        .filter(|occurrence| !ordering_only_pairs.contains(&(occurrence.from, occurrence.to)))
    {
        let Some(Placement::Owned(producer)) = state.placement.get(&occurrence.to).copied() else {
            continue;
        };
        for &consumer in state.contexts.get(&occurrence.from).into_iter().flatten() {
            if consumer != producer {
                cross_uses.insert((occurrence.to, producer, consumer));
            }
        }
    }
    Ok((
        cross_uses,
        delayed_edges,
        immediate_delay_edges,
        effect_edges,
    ))
}

/// Data edges: one per cross-loop use, plus each delayed edge that is not
/// already ordered through the existing data/effect edges.
fn close_delayed_edges(
    cross_uses: &BTreeSet<(u32, u64, u64)>,
    delayed_edges: BTreeSet<LoopEdge>,
    effect_edges: &BTreeSet<LoopEdge>,
) -> BTreeSet<LoopEdge> {
    let mut data_edges = cross_uses
        .iter()
        .map(|&(_, producer, consumer)| LoopEdge {
            consumer,
            dependency: producer,
        })
        .collect::<BTreeSet<_>>();
    let mut ordering_edges = data_edges.clone();
    ordering_edges.extend(effect_edges.iter().copied());
    for edge in delayed_edges {
        if !reachable(edge.consumer, edge.dependency, &ordering_edges) {
            data_edges.insert(edge);
            ordering_edges.insert(edge);
        }
    }
    data_edges
}

/// Signal records straight from the certificate plus the final placements.
fn build_signal_records(
    certificate: &crate::signal_fir::decoration_verify::DecorationCertificate,
    state: &PlacementState<'_>,
) -> Vec<SignalRecord> {
    certificate
        .records
        .iter()
        .map(|record| SignalRecord {
            signal_id: u64::from(record.signal_id),
            value_type: value_type(&record.sig_type),
            structural: record.is_symbolic_recursion_carrier,
            rate: rate(record.variability),
            vectorability: vectorability(record.vectorability),
            clock_id: record
                .clock_domain
                .map_or(0, |domain| u64::from(domain) + 1),
            effects: record.effects.clone(),
            direct_effects: record.direct_effects.clone(),
            placement: state.placement[&record.signal_id],
            duplicable: effects_duplicable(&record.effects),
        })
        .collect::<Vec<_>>()
}

/// Loop records and their `VecSafe` witnesses: serial per recursion group,
/// vectorizable when every root is provably reorderable, island otherwise.
fn build_loop_records(
    loop_ids: &[u64],
    state: &PlacementState<'_>,
    recursion_loop: &BTreeMap<u32, u64>,
) -> (Vec<LoopRecord>, Vec<VecSafeWitness>) {
    let mut loops = Vec::new();
    let mut witnesses = Vec::new();
    for loop_id in loop_ids {
        let roots = state
            .roots_by_loop
            .get(loop_id)
            .map(|roots| roots.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let recursive_group = recursion_loop
            .iter()
            .find_map(|(&group, &id)| (id == *loop_id).then_some(group));
        let kind = if let Some(group) = recursive_group {
            LoopKind::Recursive(u64::from(group))
        } else if roots.iter().all(|root| {
            let record = state.records[&u32::try_from(*root).expect("signal id fits u32")];
            record.vectorability == SigVectorability::Vect
                && effects_sample_reorderable(&record.effects)
        }) {
            LoopKind::Vectorizable
        } else {
            let clock = roots.iter().find_map(|root| {
                state.records[&u32::try_from(*root).expect("signal id fits u32")].clock_domain
            });
            LoopKind::Island(clock.map_or(EFFECT_ISLAND_TAG | loop_id, |id| u64::from(id) + 1))
        };
        let stable_name = match kind {
            LoopKind::Vectorizable => format!("loop_vec_{loop_id}"),
            LoopKind::Recursive(group) => format!("loop_rec_{group}"),
            LoopKind::Island(island) => format!("loop_island_{island}"),
            LoopKind::Lockstep { width } => format!("loop_lockstep_{width}_{loop_id}"),
        };
        loops.push(LoopRecord {
            loop_id: *loop_id,
            stable_name,
            kind,
            roots,
            epoch_id: 0,
        });
        witnesses.push(VecSafeWitness {
            loop_id: *loop_id,
            witness_kind: match kind {
                LoopKind::Vectorizable => WitnessKind::Pointwise,
                LoopKind::Recursive(_) | LoopKind::Island(_) | LoopKind::Lockstep { .. } => {
                    WitnessKind::SerialStateInternal
                }
            },
        });
    }
    (loops, witnesses)
}

/// One planar chunk transport per cross-loop use, in canonical order.
fn build_transports(
    cross_uses: &BTreeSet<(u32, u64, u64)>,
    state: &PlacementState<'_>,
    vec_size: u64,
) -> Vec<TransportRecord> {
    let mut transports = Vec::new();
    for (transport_id, &(signal_id, producer_loop, consumer_loop)) in cross_uses.iter().enumerate()
    {
        let record = state.records[&signal_id];
        transports.push(TransportRecord {
            transport_id: u64::try_from(transport_id).expect("transport count fits u64"),
            stable_name: format!("transport_s{signal_id}_l{producer_loop}_l{consumer_loop}"),
            signal_id: u64::from(signal_id),
            producer_loop,
            consumer_loop,
            element_type: value_type(&record.sig_type),
            length: vec_size,
            layout: TransportLayout::Planar,
        });
    }
    transports
}

/// Every immediate delay crossing must be covered by a fused serial group;
/// an uncovered crossing fails the whole plan closed.
fn check_immediate_delay_coverage(
    certificate: &crate::signal_fir::decoration_verify::DecorationCertificate,
    state: &PlacementState<'_>,
    fused_serial_groups: &[FusedSerialGroupRecord],
    immediate_delay_edges: &BTreeSet<LoopEdge>,
    timing_enabled: bool,
) -> Result<(), VectorPlanBuildError> {
    for edge in immediate_delay_edges.iter() {
        if !fused_serial_groups.iter().any(|group| {
            group
                .member_loop_ids
                .binary_search(&edge.dependency)
                .is_ok()
                && group.member_loop_ids.binary_search(&edge.consumer).is_ok()
        }) {
            if timing_enabled {
                eprintln!(
                    "[vector-fused-uncovered-edge] producer={} consumer={} groups={:?}",
                    edge.dependency, edge.consumer, fused_serial_groups
                );
                for dependency in certificate.dependencies.iter().filter(|dependency| {
                    matches!(dependency.kind, DepKind::Immediate)
                        && state.records[&dependency.from].is_delay_read
                        && state
                            .delayed_pairs
                            .contains(&(dependency.from, dependency.to))
                        && state.placement.get(&dependency.to)
                            == Some(&Placement::Owned(edge.dependency))
                        && state
                            .contexts
                            .get(&dependency.from)
                            .is_some_and(|contexts| contexts.contains(&edge.consumer))
                }) {
                    let read = state.records[&dependency.from];
                    let carrier = state.records[&dependency.to];
                    eprintln!(
                        "[vector-fused-uncovered-fact] read={} carrier={} read_place={:?} carrier_place={:?} read_clock={:?} carrier_clock={:?} carrier_rec={:?} carrier_delay={} read_dependencies={:?}",
                        dependency.from,
                        dependency.to,
                        state.placement.get(&dependency.from),
                        state.placement.get(&dependency.to),
                        read.clock_domain,
                        carrier.clock_domain,
                        carrier.recursive_projection,
                        carrier.max_delay,
                        certificate
                            .dependencies
                            .iter()
                            .filter(|candidate| candidate.from == dependency.from)
                            .collect::<Vec<_>>()
                    );
                }
            }
            return Err(VectorPlanBuildError::UnfusedImmediateDelayCrossing {
                producer: edge.dependency,
                consumer: edge.consumer,
            });
        }
    }
    Ok(())
}

/// Builds the production vector plan exclusively from facts of the accepted
/// decoration certificate.
///
/// Loop, epoch, transport, and stable-name identities depend only on the
/// certificate and `vec_size`; no scheduling strategy or FIR traversal is
/// consulted. Every non-duplicable sample value is materialized exactly once.
/// Recursive projections of one symbolic group share one serial loop. Other
/// non-`VecSafe` loops use a conservative serial `Island` until clock planning supplies a
/// more precise clock/state execution model.
pub fn build_vector_plan(
    verified: &VerifiedDecorationCertificate,
    vec_size: u64,
) -> Result<VerifiedVectorPlan, VectorPlanBuildError> {
    let timing_enabled = std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some();
    let mut stage_started = std::time::Instant::now();
    let mut trace_stage = |stage: &str| {
        if timing_enabled {
            eprintln!(
                "[vector-plan-stage] {stage}: {:.3}s",
                stage_started.elapsed().as_secs_f64()
            );
        }
        stage_started = std::time::Instant::now();
    };
    if vec_size == 0 {
        return Err(VectorPlanBuildError::VecSizeZero);
    }
    let certificate = verified.certificate();
    let facts = CertificateFacts::derive(certificate);
    trace_stage("records-and-separations");

    let alloc = allocate_loops(certificate, &facts);
    let mut next_loop = alloc.next_loop;
    let recursion_loop = alloc.recursion_loop.clone();
    let state = place_signals(certificate, facts, &alloc, &mut next_loop, timing_enabled)?;
    trace_stage("placement");

    let (cross_uses, delayed_edges, immediate_delay_edges, mut effect_edges) =
        derive_dependency_edges(certificate, &state)?;
    trace_stage("dependency-edges");
    let mut data_edges = close_delayed_edges(&cross_uses, delayed_edges, &effect_edges);
    trace_stage("delayed-edge-closure");

    let loop_ids = (0..next_loop).collect::<Vec<_>>();
    orient_effect_conflicts(&loop_ids, &state, &data_edges, &mut effect_edges);
    trace_stage("effect-orientation");
    data_edges.retain(|edge| edge.consumer != edge.dependency);
    effect_edges.retain(|edge| edge.consumer != edge.dependency);

    let signals = build_signal_records(certificate, &state);
    trace_stage("signal-records");

    let (loops, witnesses) = build_loop_records(&loop_ids, &state, &recursion_loop);
    trace_stage("loop-records");

    let transports = build_transports(&cross_uses, &state, vec_size);
    trace_stage("transports");

    let fused_serial_groups = build_fused_serial_groups(
        certificate,
        &state,
        &loop_ids,
        &data_edges,
        &effect_edges,
        &transports,
    );
    check_immediate_delay_coverage(
        certificate,
        &state,
        &fused_serial_groups,
        &immediate_delay_edges,
        timing_enabled,
    )?;
    trace_stage("fused-groups");
    let plan = VectorPlan {
        schema_version: VECTOR_PLAN_SCHEMA_VERSION,
        vec_size,
        signals,
        loops,
        epochs: (!loop_ids.is_empty())
            .then_some(EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: loop_ids,
            })
            .into_iter()
            .collect(),
        transports,
        data_edges: data_edges.into_iter().collect(),
        effect_edges: effect_edges.into_iter().collect(),
        vec_safe_witnesses: witnesses,
        fused_serial_groups,
        lockstep_bundles: Vec::new(),
    };
    let verification = verify_vector_plan(&plan);
    trace_stage("plan-verification");
    verification?;
    let fused_verification = verify_fused_serial_groups_after_plan(&plan, verified);
    trace_stage("fused-verification");
    fused_verification?;
    Ok(VerifiedVectorPlan { plan })
}
/// Builds the production vector plan and automatically annotates every exact,
/// independent recursive-instance family that satisfies the lockstep gate.
///
/// The ordinary [`build_vector_plan`] entry point remains available for
/// schema/planner unit tests. Production `-vec` uses this stronger boundary so
/// no public feature flag or secondary scheduling mode is introduced.
pub fn build_vector_plan_with_lockstep(
    prepared: &VerifiedPreparedSignals,
    verified: &VerifiedDecorationCertificate,
    vec_size: u64,
) -> Result<VerifiedVectorPlan, VectorPlanBuildError> {
    let mut plan = build_vector_plan(verified, vec_size)?.into_plan();
    detect_lockstep_bundles(&mut plan, prepared)?;
    verify_lockstep_isomorphism(&plan, prepared)?;
    Ok(VerifiedVectorPlan { plan })
}
pub(super) fn add_dependency_edges(
    dependency: &DependencyFact,
    state: &PlacementState<'_>,
    cross_uses: &mut BTreeSet<(u32, u64, u64)>,
    delayed_edges: &mut BTreeSet<LoopEdge>,
    immediate_delay_edges: &mut BTreeSet<LoopEdge>,
    effect_edges: &mut BTreeSet<LoopEdge>,
) -> Result<(), VectorPlanBuildError> {
    if state.placement.get(&dependency.from) == Some(&Placement::Control) {
        return Ok(());
    }
    let source_contexts =
        state
            .contexts
            .get(&dependency.from)
            .ok_or(VectorPlanBuildError::MissingContext {
                signal_id: dependency.from,
            })?;
    let target = state.placement.get(&dependency.to).copied().ok_or(
        VectorPlanBuildError::MissingContext {
            signal_id: dependency.to,
        },
    )?;
    let Placement::Owned(producer) = target else {
        return Ok(());
    };
    for &consumer in source_contexts {
        if consumer == producer {
            continue;
        }
        match dependency.kind {
            DepKind::Immediate
                if state.records[&dependency.from].is_delay_read
                    && state
                        .delayed_pairs
                        .contains(&(dependency.from, dependency.to)) =>
            {
                let edge = LoopEdge {
                    consumer,
                    dependency: producer,
                };
                delayed_edges.insert(edge);
                immediate_delay_edges.insert(edge);
            }
            DepKind::Immediate | DepKind::Control => {
                cross_uses.insert((dependency.to, producer, consumer));
            }
            DepKind::Effect => {
                effect_edges.insert(LoopEdge {
                    consumer,
                    dependency: producer,
                });
            }
            DepKind::Delayed { .. } => {
                delayed_edges.insert(LoopEdge {
                    consumer,
                    dependency: producer,
                });
            }
            DepKind::ClockBoundary => {}
        }
    }
    Ok(())
}
pub(super) fn orient_effect_conflicts(
    loops: &[u64],
    state: &PlacementState<'_>,
    data_edges: &BTreeSet<LoopEdge>,
    effect_edges: &mut BTreeSet<LoopEdge>,
) {
    if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
        eprintln!(
            "[vector-plan-size] loops={} data_edges={} effect_edges={}",
            loops.len(),
            data_edges.len(),
            effect_edges.len()
        );
    }
    let mut base_edges = data_edges.clone();
    base_edges.extend(effect_edges.iter().copied());
    let order = stable_topological_order(loops, &base_edges);
    let position = order
        .iter()
        .enumerate()
        .map(|(position, &loop_id)| (loop_id, position))
        .collect::<BTreeMap<_, _>>();
    let effects_by_loop = loops
        .iter()
        .map(|&loop_id| {
            (
                loop_id,
                EffectConflictSummary::new(&loop_effects(loop_id, state)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut reachability = PlanReachability::new(loops, &base_edges);
    for (index, &left) in loops.iter().enumerate() {
        for &right in &loops[index + 1..] {
            if !effects_by_loop[&left].conflicts(&effects_by_loop[&right])
                || reachability.reaches(left, right)
                || reachability.reaches(right, left)
            {
                continue;
            }
            let (consumer, dependency) = if position[&left] < position[&right] {
                (right, left)
            } else {
                (left, right)
            };
            let edge = LoopEdge {
                consumer,
                dependency,
            };
            effect_edges.insert(edge);
            base_edges.insert(edge);
            reachability.add_edge(dependency, consumer);
        }
    }
}
#[derive(Default)]
pub(super) struct EffectConflictSummary {
    any: bool,
    barrier: bool,
    state_reads: BTreeSet<StateResource>,
    state_writes: BTreeSet<StateResource>,
    table_reads: BTreeSet<u32>,
    table_writes: BTreeSet<u32>,
    ui_writes: BTreeSet<u32>,
    output_writes: BTreeSet<u32>,
}
impl EffectConflictSummary {
    pub(super) fn new(effects: &[EffectAtom]) -> Self {
        let mut summary = Self {
            any: !effects.is_empty(),
            ..Self::default()
        };
        for effect in effects {
            match effect {
                EffectAtom::ReadState(resource) => {
                    summary.state_reads.insert(resource.clone());
                }
                EffectAtom::WriteState(resource) => {
                    summary.state_writes.insert(resource.clone());
                }
                EffectAtom::ReadTable(table) => {
                    summary.table_reads.insert(*table);
                }
                EffectAtom::WriteTable(table) => {
                    summary.table_writes.insert(*table);
                }
                EffectAtom::WriteUi(zone) => {
                    summary.ui_writes.insert(*zone);
                }
                EffectAtom::WriteOutput(output) => {
                    summary.output_writes.insert(*output);
                }
                EffectAtom::Foreign { purity, .. } => {
                    summary.barrier |=
                        matches!(purity, ForeignPurity::Impure | ForeignPurity::Unknown);
                }
            }
        }
        summary
    }

    pub(super) fn conflicts(&self, other: &Self) -> bool {
        (self.barrier && other.any)
            || (other.barrier && self.any)
            || intersects(&self.state_writes, &other.state_reads)
            || intersects(&self.state_writes, &other.state_writes)
            || intersects(&self.state_reads, &other.state_writes)
            || intersects(&self.table_writes, &other.table_reads)
            || intersects(&self.table_writes, &other.table_writes)
            || intersects(&self.table_reads, &other.table_writes)
            || intersects(&self.ui_writes, &other.ui_writes)
            || intersects(&self.output_writes, &other.output_writes)
    }
}
pub(super) fn intersects<T: Ord>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> bool {
    let (small, large) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    small.iter().any(|item| large.contains(item))
}
pub(super) fn loop_effects(
    loop_id: u64,
    state: &PlacementState<'_>,
) -> Vec<crate::signal_fir::vector::analysis::EffectAtom> {
    let mut effects = BTreeSet::new();
    if let Some(roots) = state.roots_by_loop.get(&loop_id) {
        for root in roots {
            let signal = u32::try_from(*root).expect("signal id fits u32");
            effects.extend(state.records[&signal].effects.iter().cloned());
        }
    }
    effects.into_iter().collect()
}
pub(super) fn stable_topological_order(loops: &[u64], edges: &BTreeSet<LoopEdge>) -> Vec<u64> {
    let mut dependencies = loops
        .iter()
        .map(|&loop_id| (loop_id, 0_usize))
        .collect::<BTreeMap<_, _>>();
    let successors = successor_map(loops, edges);
    for edge in edges {
        *dependencies.entry(edge.consumer).or_default() += 1;
    }
    let mut ready = dependencies
        .iter()
        .filter_map(|(&loop_id, &count)| (count == 0).then_some(loop_id))
        .collect::<BTreeSet<_>>();
    let mut order = Vec::new();
    while let Some(next) = ready.pop_first() {
        order.push(next);
        for &consumer in &successors[&next] {
            let count = dependencies
                .get_mut(&consumer)
                .expect("successor is a known loop");
            *count -= 1;
            if *count == 0 {
                ready.insert(consumer);
            }
        }
    }
    let scheduled = order.iter().copied().collect::<BTreeSet<_>>();
    order.extend(
        loops
            .iter()
            .copied()
            .filter(|loop_id| !scheduled.contains(loop_id)),
    );
    order
}
pub(super) fn successor_map(
    loops: &[u64],
    edges: &BTreeSet<LoopEdge>,
) -> BTreeMap<u64, BTreeSet<u64>> {
    let mut successors = loops
        .iter()
        .map(|&loop_id| (loop_id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for edge in edges {
        successors
            .entry(edge.dependency)
            .or_default()
            .insert(edge.consumer);
    }
    successors
}
pub(super) fn reachable(from: u64, to: u64, edges: &BTreeSet<LoopEdge>) -> bool {
    let mut queue = VecDeque::from([from]);
    let mut seen = BTreeSet::new();
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node) {
            continue;
        }
        for edge in edges.iter().filter(|edge| edge.dependency == node) {
            if edge.consumer == to {
                return true;
            }
            queue.push_back(edge.consumer);
        }
    }
    false
}
pub(super) fn rate(variability: Variability) -> Rate {
    match variability {
        Variability::Konst => Rate::Konst,
        Variability::Block => Rate::Block,
        Variability::Samp => Rate::Samp,
    }
}
pub(super) fn vectorability(vectorability: SigVectorability) -> Vectorability {
    match vectorability {
        SigVectorability::Vect => Vectorability::Vect,
        SigVectorability::Scal => Vectorability::Scal,
        SigVectorability::TrueScal => Vectorability::TrueScal,
    }
}
pub(super) fn value_type(sig_type: &CanonicalSigType) -> ValueType {
    match sig_type {
        CanonicalSigType::Sound => ValueType::Sound,
        CanonicalSigType::Simple { nature, .. } => scalar_value_type(*nature),
        // Table wrappers carry the effective nature of the current signal.
        // This matters for casts around nested read-table types: recursively
        // unwrapping content would lose the outer real cast.
        CanonicalSigType::Table { nature, .. } => scalar_value_type(*nature),
        CanonicalSigType::Tuplet { components, .. } => {
            ValueType::Tuple(components.iter().map(value_type).collect())
        }
    }
}
pub(super) fn scalar_value_type(nature: Nature) -> ValueType {
    match nature {
        Nature::Int => ValueType::Int,
        Nature::Real | Nature::Any => ValueType::Real,
    }
}
/// Whether this signal must execute once per demanded sample even when its
/// intrinsic Faust type is `Konst` or `Block`.
///
/// C++ occurrence markup starts every output in a sample-rate use context.
/// Temporal primitives can nevertheless retain a slower intrinsic type: a
/// delayed constant, for example, still changes after the first sample because
/// its history starts cleared. The occurrence certificate identifies delay
/// carriers separately from delay amounts, while state effects retain the
/// other temporal closures. A generic sample-rate *use context* alone is
/// deliberately insufficient: the literal amount of a fixed delay is visited
/// from a sample expression but remains a pure control value.
pub(super) fn requires_sample_execution(
    record: &DecorationRecord,
    is_delayed_value: bool,
    is_structural_carrier: bool,
) -> bool {
    !is_structural_carrier
        && (record.variability == Variability::Samp
            || is_delayed_value
            || record.effects.iter().any(|effect| {
                matches!(effect, EffectAtom::ReadState(_) | EffectAtom::WriteState(_))
            }))
}
