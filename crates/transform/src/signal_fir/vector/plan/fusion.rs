//! Fused serial-group construction (producer side): closes sample-required
//! ancestors, dangerous delayed-read relations, and conflicting effect users
//! into one serial group envelope.

use super::build::*;
use super::producer_reachability::*;
use crate::signal_fir::vector::analysis::{DepKind, EffectAtom};
use crate::signal_fir::vector::verify::{
    FusedSerialGroupRecord, LoopEdge, Placement, TransportRecord,
};
use std::collections::{BTreeMap, BTreeSet};

/// Derives fail-closed fused-serial groups directly from certified decoration
/// facts.
///
/// Every immediate delayed-state crossing and delayed-recursion chain is
/// closed into one canonical per-sample execution component. Components also
/// absorb overlapping carriers, internal transports, and conflicting effect
/// users. A component is emitted only when every member and grouped signal has
/// one exact clock id; the independent verifier rebuilds all of these facts.
/// One fused-group candidate: the carriers, member loops, delayed reads,
/// and conflicting state effects gathered so far.
#[derive(Default)]
struct Candidate {
    /// Delayed-state carrier signal ids.
    carriers: BTreeSet<u64>,
    /// Member loop ids.
    members: BTreeSet<u64>,
    /// Delayed-read signal ids.
    delayed_reads: BTreeSet<u64>,
    /// State effects the component must isolate.
    state_effects: BTreeSet<EffectAtom>,
    /// Whether conflicting effect users must be absorbed too.
    close_effect_users: bool,
}

/// Everything the fusion passes share: the inputs plus the derived closure
/// rows, per-loop effects, and carrier-by-owner index.
struct FusionContext<'a> {
    /// Accepted decoration certificate.
    certificate: &'a crate::signal_fir::decoration_verify::DecorationCertificate,
    /// Final placement state from the plan builder.
    state: &'a PlacementState<'a>,
    /// Every allocated loop id.
    loop_ids: &'a [u64],
    /// Planned chunk transports.
    transports: &'a [TransportRecord],
    /// Forward reachability closure over the ordering edges.
    reachability: PlanReachability,
    /// Transposed closure rows (`reverse_rows[to]` = loops reaching `to`).
    reverse_rows: Vec<Vec<u64>>,
    /// Effects contributed by each loop's roots.
    effects_by_loop: BTreeMap<u64, Vec<EffectAtom>>,
    /// Qualifying delayed-state carriers indexed by owning loop.
    carriers_by_owner: BTreeMap<u64, Vec<u64>>,
}

impl<'a> FusionContext<'a> {
    fn new(
        certificate: &'a crate::signal_fir::decoration_verify::DecorationCertificate,
        state: &'a PlacementState<'a>,
        loop_ids: &'a [u64],
        data_edges: &BTreeSet<LoopEdge>,
        effect_edges: &BTreeSet<LoopEdge>,
        transports: &'a [TransportRecord],
    ) -> Self {
        let mut ordering_edges = data_edges.clone();
        ordering_edges.extend(effect_edges.iter().copied());
        let reachability = PlanReachability::new(loop_ids, &ordering_edges);
        // Transposed closure: `reverse_rows[to]` holds every loop that reaches
        // `to`. Path queries below need that direction, and a column of the
        // forward closure cannot be read without scanning every row.
        let reverse_rows = {
            let words = reachability.words();
            let mut rows = vec![vec![0_u64; words]; loop_ids.len()];
            for (to, row) in rows.iter_mut().enumerate() {
                for from in 0..loop_ids.len() {
                    if reachability.bit(from, to) {
                        set_bit(row, from);
                    }
                }
            }
            rows
        };
        // `loop_effects` depends only on the immutable placement state, so the
        // component fixpoint below would otherwise recompute identical effect sets
        // once per loop per component per iteration. Loops absent from
        // `roots_by_loop` contribute no effects, so this map is exhaustive.
        let effects_by_loop = state
            .roots_by_loop
            .keys()
            .map(|&loop_id| (loop_id, loop_effects(loop_id, state)))
            .collect::<BTreeMap<_, _>>();

        // A carrier qualifies for absorption when a placed read reaches it through
        // a positive delay or a delayed immediate pair. That predicate never
        // mentions a component, so only the owning-loop membership test below
        // varies. Index the qualifying carriers by owner once instead of rescanning
        // every record/dependency pair per component per fixpoint iteration.
        let mut carrier_targets = BTreeSet::<u32>::new();
        for dependency in &certificate.dependencies {
            let placed_read = state
                .placement
                .get(&dependency.from)
                .is_some_and(|placement| matches!(placement, Placement::Owned(_)));
            let carries = matches!(dependency.kind, DepKind::Delayed { amount } if amount > 0)
                || (matches!(dependency.kind, DepKind::Immediate)
                    && state
                        .delayed_pairs
                        .contains(&(dependency.from, dependency.to)));
            if placed_read && carries {
                carrier_targets.insert(dependency.to);
            }
        }
        let mut carriers_by_owner = BTreeMap::<u64, Vec<u64>>::new();
        for record in &certificate.records {
            if record.max_delay == 0 || !carrier_targets.contains(&record.signal_id) {
                continue;
            }
            if let Some(Placement::Owned(loop_id)) = state.placement.get(&record.signal_id).copied()
            {
                carriers_by_owner
                    .entry(loop_id)
                    .or_default()
                    .push(u64::from(record.signal_id));
            }
        }

        Self {
            certificate,
            state,
            loop_ids,
            transports,
            reachability,
            reverse_rows,
            effects_by_loop,
            carriers_by_owner,
        }
    }

    /// Route 1 — delayed recursion dependencies: close every loop on the
    /// same-sample path from a delayed read to its recursive writer.
    fn seeds_from_delayed_recursion(&self) -> Vec<Candidate> {
        let Self {
            certificate,
            state,
            loop_ids,
            reachability,
            ..
        } = self;
        let mut seeds = Vec::new();
        // Delayed recursion dependencies run from the delayed read towards the
        // recursive writer. Close every loop on that same-sample path.
        for dependency in certificate.dependencies.iter().filter(
            |dependency| matches!(dependency.kind, DepKind::Delayed { amount } if amount > 0),
        ) {
            let read_id = dependency.from;
            if !state.records.contains_key(&read_id) {
                continue;
            }
            let Some(carrier_record) = state.records.get(&dependency.to).copied() else {
                continue;
            };
            if carrier_record.max_delay == 0 {
                continue;
            }
            let Some(Placement::Owned(read_loop_id)) = state.placement.get(&read_id).copied()
            else {
                continue;
            };
            let Some(Placement::Owned(owner_loop_id)) =
                state.placement.get(&dependency.to).copied()
            else {
                continue;
            };
            if read_loop_id == owner_loop_id || !reachability.reaches(read_loop_id, owner_loop_id) {
                continue;
            }
            let mut candidate = Candidate::default();
            candidate.carriers.insert(u64::from(dependency.to));
            // Include every loop on a same-sample data path from the delayed read
            // to its recursive writer. The fused body then preserves read(n),
            // write(n), read(n+1) even when no transport directly carries the
            // delayed-read node itself.
            candidate
                .members
                .extend(loop_ids.iter().copied().filter(|&loop_id| {
                    (loop_id == read_loop_id || reachability.reaches(read_loop_id, loop_id))
                        && (loop_id == owner_loop_id
                            || reachability.reaches(loop_id, owner_loop_id))
                }));
            candidate.delayed_reads.insert(u64::from(read_id));
            seeds.push(candidate);
        }

        seeds
    }

    /// Route 2 — immediate state-mediated delay crossings (an immediate
    /// scheduling dependency plus a nonzero occurrence delay on the same
    /// pair): the carrier need not be recursive; bounded delay lines carry
    /// the same per-sample write/read obligation.
    fn seeds_from_immediate_crossings(&self) -> Vec<Candidate> {
        let Self {
            certificate,
            state,
            loop_ids,
            reachability,
            ..
        } = self;
        let mut seeds = Vec::new();
        // Immediate state-mediated delay crossings are represented by an
        // immediate scheduling dependency plus a nonzero occurrence delay for the
        // same signal pair. Unlike the original slice, the carrier need not be a
        // recursive projection: ordinary bounded delay lines have the same
        // per-sample write/read obligation.
        for dependency in certificate.dependencies.iter().filter(|dependency| {
            matches!(dependency.kind, DepKind::Immediate)
                && state.records[&dependency.from].is_delay_read
                && state
                    .delayed_pairs
                    .contains(&(dependency.from, dependency.to))
        }) {
            let carrier_record = state.records[&dependency.to];
            if carrier_record.max_delay == 0 {
                continue;
            }
            let Some(Placement::Owned(writer_loop_id)) =
                state.placement.get(&dependency.to).copied()
            else {
                continue;
            };
            let mut candidate = Candidate {
                close_effect_users: true,
                ..Default::default()
            };
            candidate.carriers.insert(u64::from(dependency.to));
            candidate.delayed_reads.insert(u64::from(dependency.from));
            candidate.members.insert(writer_loop_id);
            candidate.state_effects.extend(
                carrier_record
                    .effects
                    .iter()
                    .filter(|carrier_effect| {
                        state.records[&dependency.from]
                            .effects
                            .iter()
                            .any(|read_effect| {
                                crate::signal_fir::vector::analysis::effects_conflict(
                                    carrier_effect,
                                    read_effect,
                                )
                            })
                    })
                    .cloned(),
            );
            if let Some(Placement::Owned(read_owner)) =
                state.placement.get(&dependency.from).copied()
            {
                candidate.members.insert(read_owner);
            }
            for &read_loop_id in state.contexts.get(&dependency.from).into_iter().flatten() {
                candidate.members.insert(read_loop_id);
                if reachability.reaches(writer_loop_id, read_loop_id) {
                    candidate
                        .members
                        .extend(loop_ids.iter().copied().filter(|&loop_id| {
                            (loop_id == writer_loop_id
                                || reachability.reaches(writer_loop_id, loop_id))
                                && (loop_id == read_loop_id
                                    || reachability.reaches(loop_id, read_loop_id))
                        }));
                }
            }
            seeds.push(candidate);
        }

        seeds
    }

    /// Route 3 — transported delayed reads: a read may share its recursive
    /// owner loop while a consumer transport still has to stay in that same
    /// physical sample loop.
    fn seeds_from_transported_reads(&self) -> Vec<Candidate> {
        let Self {
            certificate,
            state,
            transports,
            ..
        } = self;
        let mut seeds = Vec::new();
        // Preserve the direct transported-read slice as a second, independent
        // discovery route. A delayed read may already share its recursive owner
        // loop while a consumer transport still has to remain in that same
        // physical sample loop. Direct dependency discovery above intentionally
        // skips that local-read case.
        for transport in transports.iter() {
            let read_id = u32::try_from(transport.signal_id).expect("signal id fits u32");
            if !state.records.contains_key(&read_id) {
                continue;
            }
            for dependency in certificate.dependencies.iter().filter(|dependency| {
                dependency.from == read_id
                    && matches!(dependency.kind, DepKind::Delayed { amount } if amount > 0)
            }) {
                let Some(carrier_record) = state.records.get(&dependency.to).copied() else {
                    continue;
                };
                if carrier_record.max_delay == 0 {
                    continue;
                }
                let Some(Placement::Owned(owner_loop_id)) =
                    state.placement.get(&dependency.to).copied()
                else {
                    continue;
                };
                let mut candidate = Candidate::default();
                candidate.carriers.insert(u64::from(dependency.to));
                candidate.members.insert(transport.producer_loop);
                candidate.members.insert(transport.consumer_loop);
                candidate.members.insert(owner_loop_id);
                candidate.delayed_reads.insert(transport.signal_id);
                seeds.push(candidate);
            }
        }

        seeds
    }

    /// One fixpoint step over one component: absorb owned carriers, their
    /// delayed readers, conflicting effect users with their connecting
    /// paths, and every loop between two members. Returns whether the
    /// component grew.
    fn absorb_into_component(&self, component: &mut Candidate) -> bool {
        let Self {
            certificate,
            state,
            loop_ids,
            reachability,
            reverse_rows,
            effects_by_loop,
            carriers_by_owner,
            ..
        } = self;
        let mut changed = false;
        let previous = (
            component.carriers.len(),
            component.members.len(),
            component.delayed_reads.len(),
            component.state_effects.len(),
        );
        component.carriers.extend(
            component
                .members
                .iter()
                .filter_map(|loop_id| carriers_by_owner.get(loop_id))
                .flatten()
                .copied(),
        );
        for dependency in &certificate.dependencies {
            let carrier_id = u64::from(dependency.to);
            if !component.carriers.contains(&carrier_id) {
                continue;
            }
            let immediate = matches!(dependency.kind, DepKind::Immediate)
                && state
                    .delayed_pairs
                    .contains(&(dependency.from, dependency.to));
            let delayed = matches!(dependency.kind, DepKind::Delayed { amount } if amount > 0);
            if !immediate && !delayed {
                continue;
            }
            let Some(Placement::Owned(read_owner)) = state.placement.get(&dependency.from).copied()
            else {
                continue;
            };
            component.delayed_reads.insert(u64::from(dependency.from));
            component.members.insert(read_owner);
            if immediate {
                component.close_effect_users = true;
                component.state_effects.extend(
                    state.records[&dependency.to]
                        .effects
                        .iter()
                        .filter(|carrier_effect| {
                            state.records[&dependency.from]
                                .effects
                                .iter()
                                .any(|read_effect| {
                                    crate::signal_fir::vector::analysis::effects_conflict(
                                        carrier_effect,
                                        read_effect,
                                    )
                                })
                        })
                        .cloned(),
                );
            }
        }
        component.state_effects.extend(
            component
                .members
                .iter()
                .filter_map(|loop_id| effects_by_loop.get(loop_id))
                .flatten()
                .cloned(),
        );
        component.close_effect_users |= !component.state_effects.is_empty();
        if component.close_effect_users {
            let carrier_owners = component
                .carriers
                .iter()
                .filter_map(|signal_id| u32::try_from(*signal_id).ok())
                .filter_map(|signal_id| match state.placement.get(&signal_id) {
                    Some(Placement::Owned(loop_id)) => Some(*loop_id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let effect_users = loop_ids
                .iter()
                .copied()
                .filter(|loop_id| {
                    effects_by_loop.get(loop_id).is_some_and(|effects| {
                        effects.iter().any(|effect| {
                            component.state_effects.iter().any(|carrier| {
                                crate::signal_fir::vector::analysis::effects_conflict(
                                    carrier, effect,
                                )
                            })
                        })
                    })
                })
                .collect::<Vec<_>>();
            for effect_loop in effect_users {
                component.members.insert(effect_loop);
                for &owner_loop in &carrier_owners {
                    let (start, end) = if reachability.reaches(owner_loop, effect_loop) {
                        (owner_loop, effect_loop)
                    } else if reachability.reaches(effect_loop, owner_loop) {
                        (effect_loop, owner_loop)
                    } else {
                        continue;
                    };
                    // Every loop on the start->end path is reachable from
                    // `start` (or is `start`) and reaches `end` (or is
                    // `end`). Intersecting the forward row of `start` with
                    // the reverse row of `end` yields that path in one pass
                    // over the bitset, where testing each candidate loop
                    // costs two indexed closure probes per loop instead.
                    let start_index = reachability.index[&start];
                    let end_index = reachability.index[&end];
                    let mut path = reachability.rows[start_index].clone();
                    set_bit(&mut path, start_index);
                    let mut backward = reverse_rows[end_index].clone();
                    set_bit(&mut backward, end_index);
                    and_bits(&mut path, &backward);
                    component
                        .members
                        .extend(set_bit_indices(&path).map(|index| loop_ids[index]));
                }
            }
        }
        changed |= previous
            != (
                component.carriers.len(),
                component.members.len(),
                component.delayed_reads.len(),
                component.state_effects.len(),
            );
        // A loop joins the component when some member reaches it and it
        // reaches some member. Both quantifiers are one bitset operation
        // against the closure rows: the union of the members' rows answers
        // the first for every loop at once, and intersecting a loop's own
        // row with the member set answers the second. Scanning member/loop
        // pairs instead costs `loops * members` indexed lookups per
        // component per iteration.
        let words = reachability.words();
        let mut reachable_from_members = vec![0_u64; words];
        let mut member_bits = vec![0_u64; words];
        for member_index in component
            .members
            .iter()
            .map(|member| reachability.index[member])
        {
            or_bits(
                &mut reachable_from_members,
                &reachability.rows[member_index],
            );
            set_bit(&mut member_bits, member_index);
        }
        let additions = loop_ids
            .iter()
            .copied()
            .filter(|loop_id| !component.members.contains(loop_id))
            .filter(|loop_id| {
                let loop_index = reachability.index[loop_id];
                bit_at(&reachable_from_members, loop_index)
                    && bits_intersect(&reachability.rows[loop_index], &member_bits)
            })
            .collect::<Vec<_>>();
        changed |= !additions.is_empty();
        component.members.extend(additions);
        changed
    }

    /// Merges overlapping/conflicting seeds, then closes every component to
    /// a fixpoint (absorption + pairwise merge), and returns the components
    /// in canonical order.
    fn close_components(&self, seeds: Vec<Candidate>) -> Vec<Candidate> {
        let mut components = Vec::<Candidate>::new();
        for mut candidate in seeds
            .into_iter()
            .filter(|candidate| !candidate.carriers.is_empty() && candidate.members.len() >= 2)
        {
            let mut position = 0;
            while position < components.len() {
                if !components[position].members.is_disjoint(&candidate.members)
                    || !components[position]
                        .carriers
                        .is_disjoint(&candidate.carriers)
                    || components[position].state_effects.iter().any(|left| {
                        candidate.state_effects.iter().any(|right| {
                            crate::signal_fir::vector::analysis::effects_conflict(left, right)
                        })
                    })
                {
                    let existing = components.remove(position);
                    candidate.carriers.extend(existing.carriers);
                    candidate.members.extend(existing.members);
                    candidate.delayed_reads.extend(existing.delayed_reads);
                    candidate.state_effects.extend(existing.state_effects);
                    candidate.close_effect_users |= existing.close_effect_users;
                    position = 0;
                } else {
                    position += 1;
                }
            }
            components.push(candidate);
        }
        loop {
            let mut changed = false;
            for component in &mut components {
                changed |= self.absorb_into_component(component);
            }
            let mut left = 0;
            while left < components.len() {
                let mut right = left + 1;
                while right < components.len() {
                    if components[left]
                        .members
                        .is_disjoint(&components[right].members)
                        && components[left]
                            .carriers
                            .is_disjoint(&components[right].carriers)
                        && !components[left].state_effects.iter().any(|left_effect| {
                            components[right].state_effects.iter().any(|right_effect| {
                                crate::signal_fir::vector::analysis::effects_conflict(
                                    left_effect,
                                    right_effect,
                                )
                            })
                        })
                    {
                        right += 1;
                        continue;
                    }
                    let other = components.remove(right);
                    components[left].carriers.extend(other.carriers);
                    components[left].members.extend(other.members);
                    components[left].delayed_reads.extend(other.delayed_reads);
                    components[left].state_effects.extend(other.state_effects);
                    components[left].close_effect_users |= other.close_effect_users;
                    changed = true;
                }
                left += 1;
            }
            if !changed {
                break;
            }
        }
        components.sort_by_key(|component| component.members.iter().next().copied());
        components
    }

    /// Emits one canonical group per component whose signals agree on one
    /// clock id and which owns at least one root, choosing the lowest
    /// carrier owner as the group owner.
    fn emit_groups(&self, components: Vec<Candidate>) -> Vec<FusedSerialGroupRecord> {
        let Self {
            certificate,
            state,
            transports,
            ..
        } = self;

        let mut groups = Vec::new();
        for component in components {
            let members = component.members;
            let carriers = component.carriers;
            let delayed_reads = component.delayed_reads;
            let expected_clock = carriers
                .iter()
                .next()
                .and_then(|carrier| u32::try_from(*carrier).ok())
                .and_then(|carrier| state.records.get(&carrier))
                .map(|record| record.clock_domain);
            let clocks_match = expected_clock.is_some()
                && carriers
                    .iter()
                    .chain(&delayed_reads)
                    .filter_map(|signal_id| u32::try_from(*signal_id).ok())
                    .all(|signal_id| {
                        state.records[&signal_id].clock_domain == expected_clock.flatten()
                    })
                && state.placement.iter().all(|(signal_id, placement)| {
                    !matches!(placement, Placement::Owned(loop_id) if members.contains(loop_id))
                        || state.records[signal_id].clock_domain == expected_clock.flatten()
                });
            if !clocks_match {
                continue;
            }
            let Some(owner_loop_id) = carriers
                .iter()
                .filter_map(|signal_id| {
                    let signal_id = u32::try_from(*signal_id).ok()?;
                    match state.placement.get(&signal_id) {
                        Some(Placement::Owned(loop_id)) => Some(*loop_id),
                        _ => None,
                    }
                })
                .min()
            else {
                continue;
            };
            let mut state_write_signal_ids = carriers.clone();
            state_write_signal_ids.extend(
                certificate
                    .records
                    .iter()
                    .filter(|record| {
                        record.recursive_projection.is_some()
                    && state
                        .placement
                        .get(&record.signal_id)
                        .is_some_and(|placement| {
                            matches!(placement, Placement::Owned(owner) if members.contains(owner))
                        })
                    })
                    .map(|record| u64::from(record.signal_id)),
            );
            let output_or_transport_roots = members
                .iter()
                .flat_map(|loop_id| state.roots_by_loop.get(loop_id).into_iter().flatten())
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let internal_transport_ids = transports
                .iter()
                .filter(|transport| {
                    members.contains(&transport.producer_loop)
                        && members.contains(&transport.consumer_loop)
                })
                .map(|transport| transport.transport_id)
                .collect::<Vec<_>>();
            if output_or_transport_roots.is_empty() {
                continue;
            }
            groups.push(FusedSerialGroupRecord {
                group_id: u64::try_from(groups.len()).expect("fused group count fits u64"),
                owner_loop_id,
                member_loop_ids: members.into_iter().collect(),
                state_carrier_signal_ids: carriers.into_iter().collect(),
                delayed_read_signal_ids: delayed_reads.into_iter().collect(),
                state_write_signal_ids: state_write_signal_ids.into_iter().collect(),
                internal_transport_ids,
                output_or_transport_roots,
            });
        }
        groups
    }
}

pub(super) fn build_fused_serial_groups(
    certificate: &crate::signal_fir::decoration_verify::DecorationCertificate,
    state: &PlacementState<'_>,
    loop_ids: &[u64],
    data_edges: &BTreeSet<LoopEdge>,
    effect_edges: &BTreeSet<LoopEdge>,
    transports: &[TransportRecord],
) -> Vec<FusedSerialGroupRecord> {
    let context = FusionContext::new(
        certificate,
        state,
        loop_ids,
        data_edges,
        effect_edges,
        transports,
    );
    let mut seeds = context.seeds_from_delayed_recursion();
    seeds.extend(context.seeds_from_immediate_crossings());
    seeds.extend(context.seeds_from_transported_reads());
    let components = context.close_components(seeds);
    context.emit_groups(components)
}
