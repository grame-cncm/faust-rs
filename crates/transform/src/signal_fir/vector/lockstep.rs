//! Lockstep instance-vectorization isomorphism producer and independent gate.
//!
//! Faust C++ has no corresponding instance-SIMD pass. The signal constructor
//! semantics and recursive-group decoding come from
//! `compiler/signals/signals.cpp` and `recursivness.cpp`; the adapted Rust
//! contract is section 8 of
//! `porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`.
//! A shape hash is only a candidate key: [`verify_lockstep_isomorphism`]
//! confirms every match by a second parallel traversal and checks the explicit
//! leaf mapping before lowering may consume a bundle.

use std::collections::{BTreeMap, BTreeSet};

use signals::{SigId, SigMatch, match_sig};
use tlib::match_sym_ref;

use crate::signal_prepare::VerifiedPreparedSignals;

use super::recursion::decode_symbolic_group_bodies;
use super::vector_analysis::effect_sets_conflict;
use super::vector_verify::{
    IsoLeafMapping, IsoRootWitness, LockstepBundleRecord, LockstepLaneRecord, LoopKind,
    SignalRecord, VectorPlan, VectorPlanError, verify_vector_plan,
};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

struct ShapeHasher(u64);

impl ShapeHasher {
    fn new() -> Self {
        Self(FNV_OFFSET)
    }

    fn token(&mut self, token: &str) {
        for byte in token.as_bytes().iter().copied().chain([0xff]) {
            self.byte(byte);
        }
    }

    fn number(&mut self, discriminant: u8, mut value: u64) {
        self.byte(discriminant);
        loop {
            let payload = (value & 0x7f) as u8;
            value >>= 7;
            self.byte(if value == 0 { payload } else { payload | 0x80 });
            if value == 0 {
                break;
            }
        }
    }

    fn signed_number(&mut self, discriminant: u8, value: i64) {
        let zigzag = ((value << 1) ^ (value >> 63)) as u64;
        self.number(discriminant, zigzag);
    }

    fn byte(&mut self, byte: u8) {
        self.0 ^= u64::from(byte);
        self.0 = self.0.wrapping_mul(FNV_PRIME);
    }

    fn finish(self) -> u64 {
        if self.0 == 0 { 1 } else { self.0 }
    }
}

struct ParallelShape<'a> {
    prepared: &'a VerifiedPreparedSignals,
    hasher: ShapeHasher,
    leaves: BTreeMap<u64, u64>,
    active: BTreeSet<(u64, u64)>,
    verified: BTreeSet<(u64, u64)>,
    #[cfg(test)]
    expanded_pairs: usize,
}

impl<'a> ParallelShape<'a> {
    fn new(prepared: &'a VerifiedPreparedSignals) -> Self {
        Self {
            prepared,
            hasher: ShapeHasher::new(),
            leaves: BTreeMap::new(),
            active: BTreeSet::new(),
            verified: BTreeSet::new(),
            #[cfg(test)]
            expanded_pairs: 0,
        }
    }

    fn leaf(&mut self, tag: &str, representative: SigId, lane: SigId) -> Option<()> {
        self.hasher.token(tag);
        let representative = u64::from(representative.as_u32());
        let lane = u64::from(lane.as_u32());
        match self.leaves.insert(representative, lane) {
            Some(previous) if previous != lane => None,
            _ => Some(()),
        }
    }

    fn unary(&mut self, tag: &str, left: SigId, right: SigId) -> Option<()> {
        self.hasher.token(tag);
        self.visit(left, right)
    }

    fn binary(
        &mut self,
        tag: &str,
        left_a: SigId,
        left_b: SigId,
        right_a: SigId,
        right_b: SigId,
    ) -> Option<()> {
        self.hasher.token(tag);
        self.visit(left_a, right_a)?;
        self.visit(left_b, right_b)
    }

    fn visit(&mut self, representative: SigId, lane: SigId) -> Option<()> {
        let pair = (u64::from(representative.as_u32()), u64::from(lane.as_u32()));
        if self.verified.contains(&pair) {
            return Some(());
        }
        if !self.active.insert(pair) {
            return None;
        }
        #[cfg(test)]
        {
            self.expanded_pairs += 1;
        }

        let arena = self.prepared.arena();
        let result = if match_sym_ref(arena, representative).is_some()
            || match_sym_ref(arena, lane).is_some()
        {
            if match_sym_ref(arena, representative).is_some()
                && match_sym_ref(arena, lane).is_some()
            {
                self.leaf("leaf:state", representative, lane)
            } else {
                None
            }
        } else if let Some((_, representative_bodies)) =
            decode_symbolic_group_bodies(arena, representative)
        {
            let (_, lane_bodies) = decode_symbolic_group_bodies(arena, lane)?;
            if representative_bodies.len() != lane_bodies.len() {
                None
            } else {
                self.hasher.number(
                    1,
                    u64::try_from(representative_bodies.len()).expect("recursion arity fits u64"),
                );
                representative_bodies
                    .into_iter()
                    .zip(lane_bodies)
                    .try_for_each(|(left, right)| self.visit(left, right))
            }
        } else if decode_symbolic_group_bodies(arena, lane).is_some() {
            None
        } else {
            match (match_sig(arena, representative), match_sig(arena, lane)) {
                (SigMatch::Int(_), SigMatch::Int(_)) => self.leaf("leaf:int", representative, lane),
                (SigMatch::Real(_), SigMatch::Real(_)) => {
                    self.leaf("leaf:real", representative, lane)
                }
                (SigMatch::Input(_), SigMatch::Input(_)) => {
                    self.leaf("leaf:input", representative, lane)
                }
                (SigMatch::FConst(_, _, _), SigMatch::FConst(_, _, _)) => {
                    self.leaf("leaf:fconst", representative, lane)
                }
                (SigMatch::FVar(_, _, _), SigMatch::FVar(_, _, _)) => {
                    self.leaf("leaf:fvar", representative, lane)
                }
                (SigMatch::Delay1(left), SigMatch::Delay1(right)) => {
                    self.unary("delay1", left, right)
                }
                (SigMatch::Delay(la, lb), SigMatch::Delay(ra, rb)) => {
                    self.binary("delay", la, lb, ra, rb)
                }
                (SigMatch::Prefix(la, lb), SigMatch::Prefix(ra, rb)) => {
                    self.binary("prefix", la, lb, ra, rb)
                }
                (SigMatch::IntCast(left), SigMatch::IntCast(right)) => {
                    self.unary("int_cast", left, right)
                }
                (SigMatch::BitCast(left), SigMatch::BitCast(right)) => {
                    self.unary("bit_cast", left, right)
                }
                (SigMatch::FloatCast(left), SigMatch::FloatCast(right)) => {
                    self.unary("float_cast", left, right)
                }
                (SigMatch::Proj(li, lg), SigMatch::Proj(ri, rg)) if li == ri => {
                    self.hasher.signed_number(2, i64::from(li));
                    self.visit(lg, rg)
                }
                (SigMatch::BinOp(lo, la, lb), SigMatch::BinOp(ro, ra, rb)) if lo == ro => {
                    self.hasher.number(3, lo as u64);
                    self.visit(la, ra)?;
                    self.visit(lb, rb)
                }
                (SigMatch::Select2(la, lb, lc), SigMatch::Select2(ra, rb, rc)) => {
                    self.hasher.token("select2");
                    self.visit(la, ra)?;
                    self.visit(lb, rb)?;
                    self.visit(lc, rc)
                }
                (SigMatch::Output(li, left), SigMatch::Output(ri, right)) if li == ri => {
                    self.hasher.signed_number(4, i64::from(li));
                    self.visit(left, right)
                }
                (SigMatch::Pow(la, lb), SigMatch::Pow(ra, rb)) => {
                    self.binary("pow", la, lb, ra, rb)
                }
                (SigMatch::Min(la, lb), SigMatch::Min(ra, rb)) => {
                    self.binary("min", la, lb, ra, rb)
                }
                (SigMatch::Max(la, lb), SigMatch::Max(ra, rb)) => {
                    self.binary("max", la, lb, ra, rb)
                }
                (SigMatch::Atan2(la, lb), SigMatch::Atan2(ra, rb)) => {
                    self.binary("atan2", la, lb, ra, rb)
                }
                (SigMatch::Fmod(la, lb), SigMatch::Fmod(ra, rb)) => {
                    self.binary("fmod", la, lb, ra, rb)
                }
                (SigMatch::Remainder(la, lb), SigMatch::Remainder(ra, rb)) => {
                    self.binary("remainder", la, lb, ra, rb)
                }
                (SigMatch::Acos(left), SigMatch::Acos(right)) => self.unary("acos", left, right),
                (SigMatch::Asin(left), SigMatch::Asin(right)) => self.unary("asin", left, right),
                (SigMatch::Atan(left), SigMatch::Atan(right)) => self.unary("atan", left, right),
                (SigMatch::Cos(left), SigMatch::Cos(right)) => self.unary("cos", left, right),
                (SigMatch::Sin(left), SigMatch::Sin(right)) => self.unary("sin", left, right),
                (SigMatch::Tan(left), SigMatch::Tan(right)) => self.unary("tan", left, right),
                (SigMatch::Exp(left), SigMatch::Exp(right)) => self.unary("exp", left, right),
                (SigMatch::Exp10(left), SigMatch::Exp10(right)) => self.unary("exp10", left, right),
                (SigMatch::Log(left), SigMatch::Log(right)) => self.unary("log", left, right),
                (SigMatch::Log10(left), SigMatch::Log10(right)) => self.unary("log10", left, right),
                (SigMatch::Sqrt(left), SigMatch::Sqrt(right)) => self.unary("sqrt", left, right),
                (SigMatch::Abs(left), SigMatch::Abs(right)) => self.unary("abs", left, right),
                (SigMatch::Floor(left), SigMatch::Floor(right)) => self.unary("floor", left, right),
                (SigMatch::Ceil(left), SigMatch::Ceil(right)) => self.unary("ceil", left, right),
                (SigMatch::Rint(left), SigMatch::Rint(right)) => self.unary("rint", left, right),
                (SigMatch::Round(left), SigMatch::Round(right)) => self.unary("round", left, right),
                _ => None,
            }
        };
        self.active.remove(&pair);
        if result.is_some() {
            self.verified.insert(pair);
        }
        result
    }
}

/// Produces one exact root witness after confirming structural equality by
/// parallel traversal. Hash equality alone is never sufficient.
pub(crate) fn build_iso_root_witness(
    prepared: &VerifiedPreparedSignals,
    representative_root: SigId,
    lane_root: SigId,
) -> Option<IsoRootWitness> {
    let mut shape = ParallelShape::new(prepared);
    shape.visit(representative_root, lane_root)?;
    Some(IsoRootWitness {
        representative_root: u64::from(representative_root.as_u32()),
        lane_root: u64::from(lane_root.as_u32()),
        shape_hash: shape.hasher.finish(),
        leaf_mapping: shape
            .leaves
            .into_iter()
            .map(
                |(representative_signal_id, lane_signal_id)| IsoLeafMapping {
                    representative_signal_id,
                    lane_signal_id,
                },
            )
            .collect(),
    })
}

fn reaches(plan: &VectorPlan, from: u64, to: u64) -> bool {
    let mut pending = vec![from];
    let mut visited = BTreeSet::new();
    while let Some(current) = pending.pop() {
        if !visited.insert(current) {
            continue;
        }
        for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
            if edge.dependency == current {
                if edge.consumer == to {
                    return true;
                }
                pending.push(edge.consumer);
            }
        }
    }
    false
}

fn loop_effects<'a>(
    plan: &'a VectorPlan,
    signal_by_id: &BTreeMap<u64, &'a SignalRecord>,
    loop_id: u64,
) -> Vec<super::vector_analysis::EffectAtom> {
    let mut effects = BTreeSet::new();
    let loop_record = plan
        .loops
        .iter()
        .find(|loop_record| loop_record.loop_id == loop_id)
        .expect("candidate loop belongs to plan");
    for root in &loop_record.roots {
        effects.extend(signal_by_id[root].effects.iter().cloned());
    }
    effects.into_iter().collect()
}

fn pair_is_legal(
    plan: &VectorPlan,
    signal_by_id: &BTreeMap<u64, &SignalRecord>,
    left: u64,
    right: u64,
) -> bool {
    let left_loop = plan
        .loops
        .iter()
        .find(|loop_record| loop_record.loop_id == left)
        .expect("candidate loop belongs to plan");
    let right_loop = plan
        .loops
        .iter()
        .find(|loop_record| loop_record.loop_id == right)
        .expect("candidate loop belongs to plan");
    left_loop.epoch_id == right_loop.epoch_id
        && !reaches(plan, left, right)
        && !reaches(plan, right, left)
        && !effect_sets_conflict(
            &loop_effects(plan, signal_by_id, left),
            &loop_effects(plan, signal_by_id, right),
        )
}

fn lane_witnesses(
    prepared: &VerifiedPreparedSignals,
    ids: &BTreeMap<u64, SigId>,
    representative_roots: &[u64],
    lane_roots: &[u64],
) -> Option<Vec<IsoRootWitness>> {
    if representative_roots.len() != lane_roots.len() {
        return None;
    }
    representative_roots
        .iter()
        .copied()
        .zip(lane_roots.iter().copied())
        .map(|(representative_root, lane_root)| {
            build_iso_root_witness(
                prepared,
                *ids.get(&representative_root)?,
                *ids.get(&lane_root)?,
            )
        })
        .collect()
}

fn decorations_agree(
    signal_by_id: &BTreeMap<u64, &SignalRecord>,
    representative_roots: &[u64],
    lane_roots: &[u64],
) -> bool {
    representative_roots
        .iter()
        .zip(lane_roots)
        .all(|(representative, lane)| {
            let representative = signal_by_id[representative];
            let lane = signal_by_id[lane];
            representative.value_type == lane.value_type
                && representative.rate == lane.rate
                && representative.vectorability == lane.vectorability
                && representative.clock_id == lane.clock_id
        })
}

/// Detects exact independent recursion instances and annotates them as
/// lockstep bundles. Detection is deterministic and fail-closed: unsupported
/// signal constructors, near-isomorphic roots, fused recursive slices, graph
/// reachability, or effect conflicts leave the original loops unchanged.
pub(crate) fn detect_lockstep_bundles(
    plan: &mut VectorPlan,
    prepared: &VerifiedPreparedSignals,
) -> Result<(), VectorPlanError> {
    if !plan.lockstep_bundles.is_empty() {
        verify_lockstep_isomorphism(plan, prepared)?;
        return Ok(());
    }
    let excluded = plan
        .fused_serial_groups
        .iter()
        .flat_map(|group| group.member_loop_ids.iter().copied())
        .collect::<BTreeSet<_>>();
    let candidates = plan
        .loops
        .iter()
        .filter_map(|loop_record| match loop_record.kind {
            LoopKind::Recursive(group) if !excluded.contains(&loop_record.loop_id) => {
                Some((loop_record.loop_id, group))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let ids = prepared_ids(prepared);
    let signal_by_id = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut assigned = BTreeSet::new();
    let mut bundles = Vec::new();

    for &(representative_loop_id, representative_group) in &candidates {
        if assigned.contains(&representative_loop_id) {
            continue;
        }
        let representative_roots = plan
            .loops
            .iter()
            .find(|loop_record| loop_record.loop_id == representative_loop_id)
            .expect("candidate loop belongs to plan")
            .roots
            .clone();
        let Some(representative_witnesses) =
            lane_witnesses(prepared, &ids, &representative_roots, &representative_roots)
        else {
            continue;
        };
        let mut members = vec![(
            representative_loop_id,
            representative_group,
            representative_witnesses,
        )];
        for &(lane_loop_id, lane_group) in &candidates {
            if lane_loop_id <= representative_loop_id || assigned.contains(&lane_loop_id) {
                continue;
            }
            let lane_roots = &plan
                .loops
                .iter()
                .find(|loop_record| loop_record.loop_id == lane_loop_id)
                .expect("candidate loop belongs to plan")
                .roots;
            if !decorations_agree(&signal_by_id, &representative_roots, lane_roots)
                || members.iter().any(|(member, _, _)| {
                    !pair_is_legal(plan, &signal_by_id, *member, lane_loop_id)
                })
            {
                continue;
            }
            let Some(witnesses) = lane_witnesses(prepared, &ids, &representative_roots, lane_roots)
            else {
                continue;
            };
            members.push((lane_loop_id, lane_group, witnesses));
        }
        if members.len() < 2 {
            continue;
        }
        let width = u64::try_from(members.len()).expect("lockstep width fits u64");
        let bundle_id = u64::try_from(bundles.len()).expect("bundle count fits u64");
        for &(loop_id, _, _) in &members {
            let loop_record = plan
                .loops
                .iter_mut()
                .find(|loop_record| loop_record.loop_id == loop_id)
                .expect("candidate loop belongs to plan");
            loop_record.kind = LoopKind::Lockstep { width };
            loop_record.stable_name = format!("loop_lockstep_{bundle_id}_lane_{loop_id}");
            assigned.insert(loop_id);
        }
        bundles.push(LockstepBundleRecord {
            bundle_id,
            representative_loop_id,
            member_loop_ids: members.iter().map(|(loop_id, _, _)| *loop_id).collect(),
            lanes: members
                .into_iter()
                .map(|(loop_id, recursion_group, roots)| LockstepLaneRecord {
                    loop_id,
                    recursion_group,
                    roots,
                })
                .collect(),
        });
    }
    plan.lockstep_bundles = bundles;
    verify_lockstep_isomorphism(plan, prepared)
}

fn prepared_ids(prepared: &VerifiedPreparedSignals) -> BTreeMap<u64, SigId> {
    let mut result = BTreeMap::new();
    let mut stack = prepared.outputs().to_vec();
    while let Some(signal) = stack.pop() {
        if result.insert(u64::from(signal.as_u32()), signal).is_some() {
            continue;
        }
        if let Some(children) = prepared.arena().children(signal) {
            stack.extend(children.iter().copied());
        }
    }
    result
}

/// Re-traverses every accepted lockstep lane against its representative.
///
/// This checker is intentionally separate from the producer and from
/// [`verify_vector_plan`]: the plan-local gate validates finite graph facts,
/// while this gate consumes the authoritative prepared forest and proves that
/// each stored leaf map is the exact result of constructor-by-constructor
/// comparison.
pub fn verify_lockstep_isomorphism(
    plan: &VectorPlan,
    prepared: &VerifiedPreparedSignals,
) -> Result<(), VectorPlanError> {
    verify_vector_plan(plan)?;
    let ids = prepared_ids(prepared);
    for bundle in &plan.lockstep_bundles {
        for lane in &bundle.lanes {
            for root in &lane.roots {
                let Some(&representative_root) = ids.get(&root.representative_root) else {
                    return Err(VectorPlanError::LockstepIsoWitnessMismatch {
                        bundle_id: bundle.bundle_id,
                        loop_id: lane.loop_id,
                    });
                };
                let Some(&lane_root) = ids.get(&root.lane_root) else {
                    return Err(VectorPlanError::LockstepIsoWitnessMismatch {
                        bundle_id: bundle.bundle_id,
                        loop_id: lane.loop_id,
                    });
                };
                if build_iso_root_witness(prepared, representative_root, lane_root).as_ref()
                    != Some(root)
                {
                    return Err(VectorPlanError::LockstepIsoWitnessMismatch {
                        bundle_id: bundle.bundle_id,
                        loop_id: lane.loop_id,
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use propagate::ClockDomainTable;
    use signals::SigBuilder;
    use tlib::TreeArena;

    use crate::clk_env::annotate;
    use crate::signal_fir::decoration_verify::certify_decorations;
    use crate::signal_fir::vector_plan::build_vector_plan_with_lockstep;
    use crate::signal_fir::vector_verify::LoopKind;
    use crate::signal_prepare::prepare_signals_for_fir_verified;

    use super::*;

    fn one_poles(outer_ops: [signals::BinOp; 2]) -> (VerifiedPreparedSignals, VectorPlan) {
        let mut arena = TreeArena::new();
        let mut roots = Vec::new();
        for channel in 0..2 {
            let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
            let body = {
                let mut builder = SigBuilder::new(&mut arena);
                let feedback = builder.proj(0, self_ref);
                let previous = builder.delay1(feedback);
                let input = builder.input(channel);
                let half = builder.real(0.5);
                let scaled = builder.binop(signals::BinOp::Mul, previous, half);
                builder.binop(outer_ops[channel as usize], input, scaled)
            };
            let nil = arena.nil();
            let bodies = arena.cons(body, nil);
            let recursion = tlib::de_bruijn_rec(&mut arena, bodies);
            roots.push(SigBuilder::new(&mut arena).proj(0, recursion));
        }
        let prepared =
            prepare_signals_for_fir_verified(&arena, &roots, &ui::UiProgram::empty()).unwrap();
        let clocks = annotate(
            prepared.arena(),
            &ClockDomainTable::new(),
            prepared.outputs(),
        )
        .unwrap();
        let decorations = certify_decorations(&prepared, &clocks).unwrap();
        let plan = build_vector_plan_with_lockstep(&prepared, &decorations, 8)
            .unwrap()
            .into_plan();
        (prepared, plan)
    }

    #[test]
    fn prepared_parallel_traversal_accepts_isomorphic_recursive_lanes() {
        let (prepared, plan) = one_poles([signals::BinOp::Add; 2]);
        assert_eq!(plan.lockstep_bundles.len(), 1);
        assert_eq!(plan.lockstep_bundles[0].member_loop_ids.len(), 2);
        assert!(
            plan.lockstep_bundles[0]
                .member_loop_ids
                .iter()
                .all(|loop_id| {
                    plan.loops[*loop_id as usize].kind == LoopKind::Lockstep { width: 2 }
                })
        );
        verify_lockstep_isomorphism(&plan, &prepared).unwrap();
    }

    #[test]
    fn prepared_parallel_traversal_rejects_corrupted_shape_hash() {
        let (prepared, mut plan) = one_poles([signals::BinOp::Add; 2]);
        plan.lockstep_bundles[0].lanes[1].roots[0].shape_hash ^= 1;
        assert!(matches!(
            verify_lockstep_isomorphism(&plan, &prepared),
            Err(VectorPlanError::LockstepIsoWitnessMismatch {
                bundle_id: 0,
                loop_id: 1,
            })
        ));
    }

    #[test]
    fn near_isomorphic_recursive_lanes_remain_unbundled() {
        let (_, plan) = one_poles([signals::BinOp::Add, signals::BinOp::Sub]);
        assert!(plan.lockstep_bundles.is_empty());
        assert!(
            plan.loops
                .iter()
                .filter(|loop_record| matches!(loop_record.kind, LoopKind::Recursive(_)))
                .count()
                >= 2
        );
    }

    #[test]
    fn parallel_shape_memoizes_deeply_shared_lane_pairs() {
        const LAYERS: usize = 18;

        let mut arena = TreeArena::new();
        let representative = {
            let mut builder = SigBuilder::new(&mut arena);
            let mut node = builder.input(0);
            for _ in 0..LAYERS {
                let sine = builder.sin(node);
                let cosine = builder.cos(node);
                node = builder.binop(signals::BinOp::Add, sine, cosine);
            }
            node
        };
        let lane = {
            let mut builder = SigBuilder::new(&mut arena);
            let mut node = builder.input(1);
            for _ in 0..LAYERS {
                let sine = builder.sin(node);
                let cosine = builder.cos(node);
                node = builder.binop(signals::BinOp::Add, sine, cosine);
            }
            node
        };
        let prepared = prepare_signals_for_fir_verified(
            &arena,
            &[representative, lane],
            &ui::UiProgram::empty(),
        )
        .unwrap();
        let mut shape = ParallelShape::new(&prepared);

        shape
            .visit(prepared.outputs()[0], prepared.outputs()[1])
            .expect("the two shared DAGs are isomorphic");

        assert_eq!(shape.expanded_pairs, LAYERS * 3 + 1);
        assert_eq!(shape.verified.len(), LAYERS * 3 + 1);
    }
}
