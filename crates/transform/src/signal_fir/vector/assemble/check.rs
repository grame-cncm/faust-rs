//! Independent checker for assembled vector FIR.
//!
//! `verify_vector_fir_assembly` is called by BOTH the producer's terminal
//! step (`materialize.rs`) and standalone callers, so its admission
//! guards remain on both paths after the R7 split (plan §4.8). The
//! independent re-derivations here (independently_expected_clock_cursor,
//! state_cursor_advance_matches, expected_island_declarations, shape
//! matchers) must NOT be merged with their producer counterparts in
//! `materialize.rs` — the duplication IS the assurance boundary (plan §3.2).

use super::model::*;
use crate::signal_fir::vector::clock_ad::{
    ClockGuard, ClockIsland, ClockTransportMode, VerifiedVectorClockAdPlan,
};
use crate::signal_fir::vector::route::{VectorRegion, VerifiedRoutedFir};
use crate::signal_fir::vector::state::{
    DelayTransition, VectorDelayStorage, VectorStateAction, VerifiedVectorStatePlan,
    WaveformTransition,
};
use crate::signal_fir::vector::verify::Placement;
use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, match_fir};
use std::collections::{BTreeMap, BTreeSet};

/// Independently validates P6.3b coverage and the concrete FIR word shapes.
pub fn verify_vector_fir_assembly(
    routed: &VerifiedRoutedFir,
    state_plan: Option<&VerifiedVectorStatePlan>,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
    assembly: &VectorFirAssembly,
    store: &FirStore,
) -> Result<(), VectorFirAssemblyError> {
    if assembly.schema_version != VECTOR_FIR_ASSEMBLY_VERSION {
        return Err(VectorFirAssemblyError::TopLevelShape);
    }
    if !matches!(
        match_fir(store, assembly.top_level_statement),
        FirMatch::Block(_)
    ) {
        return Err(VectorFirAssemblyError::TopLevelShape);
    }
    let expected_loops = routed
        .layout()
        .loops()
        .iter()
        .map(|region| region.loop_id)
        .collect::<Vec<_>>();
    let actual_loops = assembly
        .loops
        .iter()
        .map(|region| region.loop_id)
        .collect::<Vec<_>>();
    if actual_loops != expected_loops {
        let loop_id = expected_loops
            .iter()
            .zip(&actual_loops)
            .find_map(|(expected, actual)| (expected != actual).then_some(*expected))
            .or_else(|| expected_loops.last().copied())
            .unwrap_or(0);
        return Err(VectorFirAssemblyError::LoopInputCoverage { loop_id });
    }

    for assembled in &assembly.loops {
        let expected = state_plan
            .and_then(|state| {
                state
                    .plan()
                    .loops
                    .iter()
                    .find(|phases| phases.loop_id == assembled.loop_id)
            })
            .map(|phases| {
                phases
                    .pre
                    .iter()
                    .chain(&phases.exec)
                    .chain(&phases.post)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let actual = assembled
            .pre
            .iter()
            .chain(&assembled.exec_actions)
            .chain(&assembled.post)
            .map(|action| action.action.clone())
            .collect::<Vec<_>>();
        if expected != actual {
            return Err(VectorFirAssemblyError::LoopStateCoverage {
                loop_id: assembled.loop_id,
            });
        }
        for action in assembled
            .pre
            .iter()
            .chain(&assembled.exec_actions)
            .chain(&assembled.post)
        {
            verify_action_shape(assembled.loop_id, action, state_plan, store)?;
        }
        if !matches!(
            match_fir(store, assembled.chunk_statement),
            FirMatch::Block(_)
        ) || !matches!(
            match_fir(store, assembled.iteration_statement),
            FirMatch::Block(_)
        ) {
            return Err(VectorFirAssemblyError::LoopStateCoverage {
                loop_id: assembled.loop_id,
            });
        }
    }

    let expected_islands = clock_plan
        .map(|clock| clock.plan().clock_islands.as_slice())
        .unwrap_or(&[]);
    if assembly.islands.len() != expected_islands.len() {
        return Err(VectorFirAssemblyError::IslandShape { domain_id: 0 });
    }
    let assembled_loop_by_id = assembly
        .loops
        .iter()
        .map(|loop_| (loop_.loop_id, loop_))
        .collect::<BTreeMap<_, _>>();
    let assembled_island_by_id = assembly
        .islands
        .iter()
        .map(|island| (island.domain_id, island))
        .collect::<BTreeMap<_, _>>();
    for (actual, expected) in assembly.islands.iter().zip(expected_islands) {
        let scheduled_loop_ids = scheduled_island_loop_ids(routed, expected);
        let local_declarations = expected_island_declarations(routed, expected.domain_id);
        let mut expected_body = local_declarations.clone();
        expected_body.extend(
            scheduled_loop_ids
                .iter()
                .map(|loop_id| assembled_loop_by_id[loop_id].iteration_statement),
        );
        expected_body.extend(
            expected_islands
                .iter()
                .filter(|child| child.parent_domain == Some(expected.domain_id))
                .map(|child| assembled_island_by_id[&child.domain_id].statement),
        );
        let expected_cursor = independently_expected_clock_cursor(state_plan, expected.domain_id)?;
        if let Some(advance) = actual.state_cursor_advance {
            expected_body.push(advance);
        }
        if actual.domain_id != expected.domain_id
            || actual.parent_domain != expected.parent_domain
            || actual.guard != expected.guard
            || actual.nested_loop_ids != scheduled_loop_ids
            || actual.local_declarations != local_declarations
            || !state_cursor_advance_matches(
                actual.state_cursor_advance,
                expected_cursor.as_deref(),
                store,
            )
            || !guard_shape_matches(expected, actual.statement, &expected_body, store)
        {
            return Err(VectorFirAssemblyError::IslandShape {
                domain_id: expected.domain_id,
            });
        }
    }
    verify_clock_output_stores(assembly, expected_islands, store)?;
    verify_assembled_fused_serial_groups(routed, state_plan, assembly, store)?;
    verify_assembled_lockstep_bundles(routed, state_plan, assembly, store)?;
    Ok(())
}
pub(super) fn independently_expected_clock_cursor(
    state_plan: Option<&VerifiedVectorStatePlan>,
    domain_id: u64,
) -> Result<Option<String>, VectorFirAssemblyError> {
    let Some(state_plan) = state_plan else {
        return Ok(None);
    };
    let mut expected = None;
    for delay in &state_plan.plan().delays {
        if let VectorDelayStorage::ClockRing {
            cursor_name,
            domain_id: actual_domain,
            ..
        } = &delay.storage
            && *actual_domain == domain_id
        {
            if expected.as_ref().is_some_and(|name| name != cursor_name) {
                return Err(VectorFirAssemblyError::IslandShape { domain_id });
            }
            expected = Some(cursor_name.clone());
        }
    }
    Ok(expected)
}
pub(super) fn state_cursor_advance_matches(
    statement: Option<FirId>,
    cursor_name: Option<&str>,
    store: &FirStore,
) -> bool {
    let (Some(statement), Some(cursor_name)) = (statement, cursor_name) else {
        return statement.is_none() && cursor_name.is_none();
    };
    let FirMatch::StoreVar {
        name,
        access: AccessType::Struct,
        value,
    } = match_fir(store, statement)
    else {
        return false;
    };
    if name != cursor_name {
        return false;
    }
    let FirMatch::BinOp {
        op: FirBinOp::Add,
        lhs,
        rhs,
        ..
    } = match_fir(store, value)
    else {
        return false;
    };
    matches!(
        (match_fir(store, lhs), match_fir(store, rhs)),
        (
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct,
                ..
            },
            FirMatch::Int32 { value: 1, .. }
        ) if name == cursor_name
    )
}
/// Independently proves the physical FIR envelope of each checked fused group.
///
/// Top-rate members must be the exact body of one physical sample loop, with
/// state setup/commit outside it. Nonzero-clock members must be consecutive in
/// one exact island and have no escaped pre/post action. In both forms, every
/// recorded state write and fused scalar transport store must occur inside the
/// selected body, and no recorded delayed read or transport load may occur
/// outside it.
///
/// Reads are stated as an escape rule rather than a presence rule because they
/// are pure values: when nothing consumes one, dead pure root removal deletes
/// it from the assembled FIR entirely. An eliminated read cannot escape the
/// body, so absence from the whole assembly is accepted while any surviving
/// occurrence outside the body is still rejected. Writes stay presence rules —
/// they are statements and that cleanup never removes them.
pub(super) fn verify_assembled_fused_serial_groups(
    routed: &VerifiedRoutedFir,
    state_plan: Option<&VerifiedVectorStatePlan>,
    assembly: &VectorFirAssembly,
    store: &FirStore,
) -> Result<(), VectorFirAssemblyError> {
    if routed.plan().fused_serial_groups.is_empty() {
        return Ok(());
    }
    let FirMatch::Block(top_level) = match_fir(store, assembly.top_level_statement) else {
        return Err(VectorFirAssemblyError::TopLevelShape);
    };
    let loop_by_id = assembly
        .loops
        .iter()
        .map(|loop_| (loop_.loop_id, loop_))
        .collect::<BTreeMap<_, _>>();
    let signal_by_id = routed
        .plan()
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    // Built on first use only: the escape question is asked only for delayed
    // reads that are already absent from their physical body, which is the
    // dead-read case rather than the common one.
    let mut assembled_nodes: Option<BTreeSet<FirId>> = None;

    for group in &routed.plan().fused_serial_groups {
        let reject = || VectorFirAssemblyError::FusedGroupShape {
            group_id: group.group_id,
        };
        if !group.member_loop_ids.contains(&group.owner_loop_id)
            || group
                .member_loop_ids
                .iter()
                .any(|loop_id| !loop_by_id.contains_key(loop_id))
        {
            return Err(reject());
        }
        let members = routed
            .layout()
            .loops()
            .iter()
            .filter_map(|region| {
                group
                    .member_loop_ids
                    .binary_search(&region.loop_id)
                    .is_ok()
                    .then_some(region.loop_id)
            })
            .collect::<Vec<_>>();
        if members.len() != group.member_loop_ids.len() {
            return Err(reject());
        }
        let expected_iterations = members
            .iter()
            .map(|loop_id| loop_by_id[loop_id].iteration_statement)
            .collect::<Vec<_>>();
        let group_clock = group
            .state_carrier_signal_ids
            .first()
            .and_then(|signal_id| signal_by_id.get(signal_id))
            .map(|signal| signal.clock_id)
            .ok_or_else(reject)?;
        let owning_islands = assembly
            .islands
            .iter()
            .filter(|island| {
                group
                    .member_loop_ids
                    .iter()
                    .any(|loop_id| island.nested_loop_ids.contains(loop_id))
            })
            .collect::<Vec<_>>();
        let physical_body = if group_clock == 0 {
            if !owning_islands.is_empty() {
                return Err(reject());
            }
            let physical_loops = top_level
                .iter()
                .enumerate()
                .filter_map(|(position, &statement)| match match_fir(store, statement) {
                    FirMatch::ForLoop {
                        var,
                        body,
                        is_reverse: false,
                        ..
                    } if var == "i0"
                        && matches!(match_fir(store, body), FirMatch::Block(words) if words == expected_iterations) =>
                    {
                        Some((position, body))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let [(physical_position, physical_body)] = physical_loops.as_slice() else {
                return Err(reject());
            };
            for &loop_id in &members {
                let assembled = loop_by_id[&loop_id];
                for action in &assembled.pre {
                    if !top_level[..*physical_position].contains(&action.statement) {
                        return Err(reject());
                    }
                }
                for action in &assembled.post {
                    if !top_level[*physical_position + 1..].contains(&action.statement) {
                        return Err(reject());
                    }
                }
            }
            *physical_body
        } else {
            let [island] = owning_islands.as_slice() else {
                return Err(reject());
            };
            let positions = members
                .iter()
                .map(|loop_id| island.nested_loop_ids.iter().position(|id| id == loop_id))
                .collect::<Option<Vec<_>>>();
            let Some(positions) = positions else {
                return Err(reject());
            };
            if island.domain_id + 1 != group_clock
                || positions.windows(2).any(|pair| pair[1] != pair[0] + 1)
                || members.iter().any(|loop_id| {
                    let assembled = loop_by_id[loop_id];
                    !assembled.pre.is_empty() || !assembled.post.is_empty()
                })
            {
                return Err(reject());
            }
            island.statement
        };
        let body_nodes = fir_reachable(store, physical_body);

        for &signal_id in &group.delayed_read_signal_ids {
            let definitions = routed
                .trace()
                .definitions()
                .iter()
                .filter(|definition| {
                    definition.signal_id == signal_id
                        && matches!(definition.region, VectorRegion::Loop(loop_id) if group.member_loop_ids.contains(&loop_id))
                })
                .collect::<Vec<_>>();
            let [definition] = definitions.as_slice() else {
                return Err(reject());
            };
            if !body_nodes.contains(&definition.value) {
                let assembled = assembled_nodes
                    .get_or_insert_with(|| fir_reachable(store, assembly.top_level_statement));
                if assembled.contains(&definition.value) {
                    return Err(reject());
                }
            }
        }

        let Some(state_plan) = state_plan else {
            return Err(reject());
        };
        for &signal_id in &group.state_write_signal_ids {
            let Some(signal) = routed
                .plan()
                .signals
                .iter()
                .find(|signal| signal.signal_id == signal_id)
            else {
                return Err(reject());
            };
            let Placement::Owned(owner_loop_id) = signal.placement else {
                return Err(reject());
            };
            let definitions = routed
                .trace()
                .definitions()
                .iter()
                .filter(|definition| {
                    definition.signal_id == signal_id
                        && definition.region == VectorRegion::Loop(owner_loop_id)
                })
                .collect::<Vec<_>>();
            if definitions.len() != 1
                || !body_nodes.contains(&definitions[0].value)
                || group.member_loop_ids.binary_search(&owner_loop_id).is_err()
            {
                return Err(reject());
            }
        }
        let delayed_writer_ids = state_plan
            .plan()
            .delays
            .iter()
            .filter(|delay| {
                group
                    .state_write_signal_ids
                    .binary_search(&delay.signal_id)
                    .is_ok()
            })
            .map(|delay| delay.signal_id)
            .collect::<Vec<_>>();
        if delayed_writer_ids.is_empty() {
            return Err(reject());
        }
        for signal_id in delayed_writer_ids {
            let writes = members
                .iter()
                .flat_map(|loop_id| loop_by_id[loop_id].exec_actions.iter())
                .filter(|action| action.action == (VectorStateAction::DelayWrite { signal_id }))
                .collect::<Vec<_>>();
            if writes.len() != 1 || !body_nodes.contains(&writes[0].statement) {
                return Err(reject());
            }
        }

        for &transport_id in &group.internal_transport_ids {
            let Some(transport) = routed
                .trace()
                .transports()
                .iter()
                .find(|transport| transport.transport_id == transport_id)
            else {
                return Err(reject());
            };
            if transport.mode
                != (ClockTransportMode::FusedScalar {
                    group_id: group.group_id,
                })
                || transport
                    .store
                    .is_none_or(|statement| !body_nodes.contains(&statement))
            {
                return Err(reject());
            }
            // The load is a pure value and follows the delayed-read rule: an
            // unconsumed one is removed with the dead pure roots, so only a
            // surviving load outside the physical body is an escape.
            let Some(load) = transport.load else {
                return Err(reject());
            };
            if !body_nodes.contains(&load) {
                let assembled = assembled_nodes
                    .get_or_insert_with(|| fir_reachable(store, assembly.top_level_statement));
                if assembled.contains(&load) {
                    return Err(reject());
                }
            }
        }
    }
    Ok(())
}
/// Independently checks the section 8 physical-loop adaptation. Logical lane
/// loops remain present in the certificate and routed trace, but top-rate
/// members must occur only as consecutive scalar iterations inside one `i0`
/// loop. A bundle wholly owned by one clock island is already advanced by its
/// enclosing top-rate sample loop and is checked as one canonical contiguous
/// lane sequence inside that island.
pub(super) fn verify_assembled_lockstep_bundles(
    routed: &VerifiedRoutedFir,
    state_plan: Option<&VerifiedVectorStatePlan>,
    assembly: &VectorFirAssembly,
    store: &FirStore,
) -> Result<(), VectorFirAssemblyError> {
    if routed.plan().lockstep_bundles.is_empty() {
        return Ok(());
    }
    let FirMatch::Block(top_level) = match_fir(store, assembly.top_level_statement) else {
        return Err(VectorFirAssemblyError::TopLevelShape);
    };
    let loop_by_id = assembly
        .loops
        .iter()
        .map(|loop_| (loop_.loop_id, loop_))
        .collect::<BTreeMap<_, _>>();
    let register_bundles = state_plan
        .into_iter()
        .flat_map(|state| &state.plan().lockstep_register_bundles)
        .map(|bundle| bundle.bundle_id)
        .collect::<BTreeSet<_>>();

    for bundle in &routed.plan().lockstep_bundles {
        let reject = || VectorFirAssemblyError::LockstepBundleShape {
            bundle_id: bundle.bundle_id,
        };
        if bundle
            .member_loop_ids
            .iter()
            .any(|loop_id| !loop_by_id.contains_key(loop_id))
        {
            return Err(reject());
        }

        let owning_islands = assembly
            .islands
            .iter()
            .filter(|island| {
                bundle
                    .member_loop_ids
                    .iter()
                    .any(|loop_id| island.nested_loop_ids.contains(loop_id))
            })
            .collect::<Vec<_>>();
        if !owning_islands.is_empty() {
            let [island] = owning_islands.as_slice() else {
                return Err(reject());
            };
            let positions = bundle
                .member_loop_ids
                .iter()
                .map(|loop_id| island.nested_loop_ids.iter().position(|id| id == loop_id))
                .collect::<Option<Vec<_>>>();
            let Some(positions) = positions else {
                return Err(reject());
            };
            if positions.windows(2).any(|pair| pair[1] != pair[0] + 1) {
                return Err(reject());
            }
            continue;
        }

        let expected_iterations = if !register_bundles.contains(&bundle.bundle_id) {
            bundle
                .member_loop_ids
                .iter()
                .map(|loop_id| loop_by_id[loop_id].iteration_statement)
                .collect::<Vec<_>>()
        } else {
            let Some(width) = bundle
                .member_loop_ids
                .first()
                .map(|loop_id| loop_by_id[loop_id].exec.len())
            else {
                return Err(reject());
            };
            if bundle
                .member_loop_ids
                .iter()
                .any(|loop_id| loop_by_id[loop_id].exec.len() != width)
            {
                return Err(reject());
            }
            let mut expected_iterations = Vec::with_capacity(width * bundle.member_loop_ids.len());
            for index in 0..width {
                for loop_id in &bundle.member_loop_ids {
                    expected_iterations.push(loop_by_id[loop_id].exec[index]);
                }
            }
            expected_iterations
        };
        let physical_loops = top_level
            .iter()
            .enumerate()
            .filter_map(|(position, &statement)| match match_fir(store, statement) {
                FirMatch::ForLoop {
                    var,
                    body,
                    is_reverse: false,
                    ..
                } if var == "i0"
                    && matches!(match_fir(store, body), FirMatch::Block(words) if words == expected_iterations) =>
                {
                    Some(position)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [physical_position] = physical_loops.as_slice() else {
            return Err(reject());
        };

        for (position, &statement) in top_level.iter().enumerate() {
            let FirMatch::ForLoop {
                var,
                body,
                is_reverse: false,
                ..
            } = match_fir(store, statement)
            else {
                continue;
            };
            if var == "i0" && position != *physical_position && expected_iterations.contains(&body)
            {
                return Err(reject());
            }
        }
        for &loop_id in &bundle.member_loop_ids {
            let assembled = loop_by_id[&loop_id];
            if assembled
                .pre
                .iter()
                .any(|action| !top_level[..*physical_position].contains(&action.statement))
                || assembled
                    .post
                    .iter()
                    .any(|action| !top_level[*physical_position + 1..].contains(&action.statement))
            {
                return Err(reject());
            }
        }
    }
    Ok(())
}
/// Every FIR node reachable from `root`, including `root` itself.
///
/// Callers checking several targets against one body must reuse this set: a
/// per-target traversal walks the whole body once per question.
/// Complete reachable-node set from `root`.
///
/// Iterates the exhaustive [`fir::fir_match_children`] primitive so an
/// unclassified `FirMatch` variant fails at compile time in `fir` instead of
/// silently truncating this checker's coverage inspection.
pub(in crate::signal_fir::vector) fn fir_reachable(
    store: &FirStore,
    root: FirId,
) -> BTreeSet<FirId> {
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !seen.insert(node) {
            continue;
        }
        pending.extend(fir::fir_match_children(store, node));
    }
    seen
}
pub(super) fn verify_clock_output_stores(
    assembly: &VectorFirAssembly,
    islands: &[ClockIsland],
    store: &FirStore,
) -> Result<(), VectorFirAssemblyError> {
    let owned = islands
        .iter()
        .flat_map(|island| island.nested_loop_ids.iter().copied())
        .collect::<BTreeSet<_>>();
    for output in &assembly.clock_output_stores {
        if !owned.contains(&output.owner_loop_id)
            || !matches!(
                match_fir(store, output.statement),
                FirMatch::StoreTable { name, .. } if name.starts_with("output")
            )
            || !contains_statement(store, assembly.top_level_statement, output.statement)
        {
            return Err(VectorFirAssemblyError::ClockLoopOwnership {
                loop_id: output.owner_loop_id,
            });
        }
    }
    Ok(())
}
pub(super) fn contains_statement(store: &FirStore, root: FirId, target: FirId) -> bool {
    if root == target {
        return true;
    }
    match match_fir(store, root) {
        FirMatch::Block(body) => body
            .into_iter()
            .any(|child| contains_statement(store, child, target)),
        FirMatch::If {
            then_block,
            else_block,
            ..
        } => {
            contains_statement(store, then_block, target)
                || else_block.is_some_and(|body| contains_statement(store, body, target))
        }
        FirMatch::Control { stmt, .. } => contains_statement(store, stmt, target),
        FirMatch::ForLoop { body, .. }
        | FirMatch::SimpleForLoop { body, .. }
        | FirMatch::IteratorForLoop { body, .. }
        | FirMatch::WhileLoop { body, .. } => contains_statement(store, body, target),
        FirMatch::Switch { cases, default, .. } => {
            cases
                .into_iter()
                .any(|(_, body)| contains_statement(store, body, target))
                || default.is_some_and(|body| contains_statement(store, body, target))
        }
        _ => false,
    }
}
pub(super) fn verify_action_shape(
    loop_id: u64,
    action: &VectorStateFirAction,
    state_plan: Option<&VerifiedVectorStatePlan>,
    store: &FirStore,
) -> Result<(), VectorFirAssemblyError> {
    let state = state_plan.expect("actions require a state plan");
    let valid = match &action.action {
        VectorStateAction::DelayRegisterLoad { signal_id } => {
            let delay = find_delay(state, *signal_id);
            matches!(
                (&delay.storage, match_fir(store, action.statement)),
                (
                    VectorDelayStorage::Register { local_name, .. },
                    FirMatch::StoreVar { name, access: AccessType::Stack, .. }
                ) if *local_name == name
            )
        }
        VectorStateAction::DelayCopyIn { signal_id } => {
            let delay = find_delay(state, *signal_id);
            match &delay.storage {
                VectorDelayStorage::Copy {
                    temporary_name,
                    permanent_name,
                    history_length,
                    ..
                } => simple_copy_matches(
                    action.statement,
                    temporary_name,
                    AccessType::Stack,
                    permanent_name,
                    AccessType::Struct,
                    *history_length,
                    store,
                ),
                VectorDelayStorage::Register { .. }
                | VectorDelayStorage::Ring { .. }
                | VectorDelayStorage::ClockRing { .. } => false,
            }
        }
        VectorStateAction::DelayRingAdvance { signal_id } => {
            let delay = find_delay(state, *signal_id);
            matches!(
                (&delay.storage, match_fir(store, action.statement)),
                (
                    VectorDelayStorage::Ring { index_name, .. },
                    FirMatch::StoreVar { name, access: AccessType::Struct, .. }
                ) if *index_name == name
            )
        }
        VectorStateAction::RecursionStep { group } => {
            let expected = state
                .plan()
                .recursions
                .iter()
                .find(|recursion| recursion.group == *group)
                .expect("verified recursion action has a transition");
            matches!(match_fir(store, action.statement), FirMatch::Block(body) if body.len() == expected.projections.len() && body.iter().zip(&expected.projections).all(|(id, projection)| matches!(match_fir(store, *id), FirMatch::DeclareVar { name, access: AccessType::Stack, init: Some(_), .. } if name == recursion_name(*group, projection.index))))
        }
        VectorStateAction::DelayWrite { signal_id } => {
            let delay = find_delay(state, *signal_id);
            match (&delay.storage, match_fir(store, action.statement)) {
                (
                    VectorDelayStorage::Register { local_name, .. },
                    FirMatch::StoreVar {
                        name,
                        access: AccessType::Stack,
                        ..
                    },
                ) => *local_name == name,
                (
                    VectorDelayStorage::Copy { temporary_name, .. },
                    FirMatch::StoreTable {
                        name,
                        access: AccessType::Stack,
                        ..
                    },
                ) => *temporary_name == name,
                (
                    VectorDelayStorage::Ring { buffer_name, .. },
                    FirMatch::StoreTable {
                        name,
                        access: AccessType::Struct,
                        ..
                    },
                ) => *buffer_name == name,
                (
                    VectorDelayStorage::ClockRing { buffer_name, .. },
                    FirMatch::StoreTable {
                        name,
                        access: AccessType::Struct,
                        ..
                    },
                ) => *buffer_name == name,
                _ => false,
            }
        }
        VectorStateAction::PrefixWrite { signal_id } => {
            let transition = state
                .plan()
                .prefixes
                .iter()
                .find(|transition| transition.signal_id == *signal_id)
                .expect("verified prefix action has a transition");
            matches!(
                match_fir(store, action.statement),
                FirMatch::StoreVar {
                    name,
                    access: AccessType::Struct,
                    ..
                } if name == transition.state_name
            )
        }
        VectorStateAction::WaveformAdvance { signal_id } => {
            let transition = state
                .plan()
                .waveforms
                .iter()
                .find(|transition| transition.signal_id == *signal_id)
                .expect("verified waveform action has a transition");
            waveform_advance_matches(action.statement, transition, store)
        }
        VectorStateAction::DelayRegisterStore { signal_id } => {
            let delay = find_delay(state, *signal_id);
            matches!(
                (&delay.storage, match_fir(store, action.statement)),
                (
                    VectorDelayStorage::Register { persistent_name, .. },
                    FirMatch::StoreVar { name, access: AccessType::Struct, .. }
                ) if *persistent_name == name
            )
        }
        VectorStateAction::DelayCopyOut { signal_id } => {
            let delay = find_delay(state, *signal_id);
            match &delay.storage {
                VectorDelayStorage::Copy {
                    temporary_name,
                    permanent_name,
                    history_length,
                    ..
                } => simple_copy_matches(
                    action.statement,
                    permanent_name,
                    AccessType::Struct,
                    temporary_name,
                    AccessType::Stack,
                    *history_length,
                    store,
                ),
                VectorDelayStorage::Register { .. }
                | VectorDelayStorage::Ring { .. }
                | VectorDelayStorage::ClockRing { .. } => false,
            }
        }
        VectorStateAction::DelayRingSaveAdvance { signal_id } => {
            let delay = find_delay(state, *signal_id);
            matches!(
                (&delay.storage, match_fir(store, action.statement)),
                (
                    VectorDelayStorage::Ring { index_save_name, .. },
                    FirMatch::StoreVar { name, access: AccessType::Struct, .. }
                ) if *index_save_name == name
            )
        }
    };
    let execution_valid = match &action.action {
        VectorStateAction::RecursionStep { .. } => {
            matches!(match_fir(store, action.statement), FirMatch::Block(body) if body == action.execution_statements)
        }
        _ => action.execution_statements == [action.statement],
    };
    if valid && execution_valid {
        Ok(())
    } else {
        Err(VectorFirAssemblyError::ActionShape {
            loop_id,
            action: action.action.clone(),
        })
    }
}
pub(super) fn waveform_advance_matches(
    statement: FirId,
    transition: &WaveformTransition,
    store: &FirStore,
) -> bool {
    let FirMatch::StoreVar {
        name,
        access: AccessType::Struct,
        value,
    } = match_fir(store, statement)
    else {
        return false;
    };
    if name != transition.index_name {
        return false;
    }
    let FirMatch::BinOp {
        op: FirBinOp::Rem,
        lhs,
        rhs,
        ..
    } = match_fir(store, value)
    else {
        return false;
    };
    let FirMatch::BinOp {
        op: FirBinOp::Add,
        lhs: index,
        rhs: one,
        ..
    } = match_fir(store, lhs)
    else {
        return false;
    };
    matches!(
        (match_fir(store, index), match_fir(store, one), match_fir(store, rhs)),
        (
            FirMatch::LoadVar { name, access: AccessType::Struct, .. },
            FirMatch::Int32 { value: 1, .. },
            FirMatch::Int32 { value: length, .. }
        ) if name == transition.index_name && u64::try_from(length).ok() == Some(transition.length)
    )
}
pub(super) fn simple_copy_matches(
    statement: FirId,
    target_name: &str,
    target_access: AccessType,
    source_name: &str,
    source_access: AccessType,
    history_length: u64,
    store: &FirStore,
) -> bool {
    let FirMatch::SimpleForLoop {
        upper,
        body,
        is_reverse: false,
        ..
    } = match_fir(store, statement)
    else {
        return false;
    };
    let Ok(history_length) = i32::try_from(history_length) else {
        return false;
    };
    if !matches!(match_fir(store, upper), FirMatch::Int32 { value, .. } if value == history_length)
    {
        return false;
    }
    let FirMatch::Block(body) = match_fir(store, body) else {
        return false;
    };
    let [statement] = body.as_slice() else {
        return false;
    };
    let FirMatch::StoreTable {
        name,
        access,
        value,
        ..
    } = match_fir(store, *statement)
    else {
        return false;
    };
    name == target_name
        && access == target_access
        && matches!(match_fir(store, value), FirMatch::LoadTable { name, access, .. } if name == source_name && access == source_access)
}
pub(super) fn guard_shape_matches(
    island: &ClockIsland,
    statement: FirId,
    expected_body: &[FirId],
    store: &FirStore,
) -> bool {
    let body = match island.guard {
        ClockGuard::BooleanOnDemand => match match_fir(store, statement) {
            FirMatch::If {
                cond,
                then_block,
                else_block: None,
            } if matches!(
                match_fir(store, cond),
                FirMatch::BinOp {
                    op: FirBinOp::Ne,
                    ..
                }
            ) =>
            {
                then_block
            }
            _ => return false,
        },
        ClockGuard::CountedOnDemand | ClockGuard::CountedUpsampling => {
            match match_fir(store, statement) {
                FirMatch::SimpleForLoop {
                    var,
                    body,
                    is_reverse: false,
                    ..
                } if var == format!("vclock_d{}_fire", island.domain_id) => body,
                _ => return false,
            }
        }
        ClockGuard::DownsampleModulo => match match_fir(store, statement) {
            FirMatch::Block(words) if words.len() == 2 => {
                let expected_counter = format!("vclock_d{}_counter", island.domain_id);
                if !matches!(match_fir(store, words[1]), FirMatch::StoreVar { name, access: AccessType::Struct, .. } if name == expected_counter)
                {
                    return false;
                }
                match match_fir(store, words[0]) {
                    FirMatch::If {
                        cond,
                        then_block,
                        else_block: None,
                    } if matches!(
                        match_fir(store, cond),
                        FirMatch::BinOp {
                            op: FirBinOp::Eq,
                            ..
                        }
                    ) =>
                    {
                        then_block
                    }
                    _ => return false,
                }
            }
            _ => return false,
        },
    };
    matches!(match_fir(store, body), FirMatch::Block(words) if words == expected_body)
}
pub(super) fn expected_island_declarations(
    routed: &VerifiedRoutedFir,
    domain_id: u64,
) -> Vec<FirId> {
    routed
        .trace()
        .transports()
        .iter()
        .filter_map(|transport| {
            (transport.mode == ClockTransportMode::IslandScalar { domain_id })
                .then_some(transport.declaration)
        })
        .collect()
}
pub(super) fn find_delay(state: &VerifiedVectorStatePlan, signal_id: u64) -> &DelayTransition {
    state
        .plan()
        .delays
        .iter()
        .find(|delay| delay.signal_id == signal_id)
        .expect("verified state action references a verified delay")
}
