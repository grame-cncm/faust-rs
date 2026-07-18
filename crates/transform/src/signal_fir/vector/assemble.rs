//! Verified P6.3b assembly of vector state phases and clock islands.
//!
//! # C++ provenance and adaptation
//! State words follow `DAGInstructionsCompiler::generateDlineLoop` and
//! `generateDelayAccess` in `compiler/generator/dag_instructions_compiler.cpp`:
//! short delays copy `_perm` history into `_tmp`, execute the chunk, then copy
//! the tail back; long delays advance a masked ring index before the chunk and
//! save the chunk count afterwards. Recursive projections are captured into
//! stack temporaries before any projection storage is updated, preserving the
//! simultaneous `sigRec`/`sigProj` step.
//!
//! Clock guards follow the scalar `SignalFIRLowerer` implementation of OD, US,
//! and DS. Unlike the C++ compiler's mutable `CodeLoop` tree, Rust assembles an
//! immutable, checked artifact from the accepted P4.4/P5/P6.1/P6.2 artifacts.
//! P6.5 keeps recursion-step declarations in the enclosing sample scope and
//! places held clock outputs after the island guard, so a non-firing sample
//! observes the previous held value. Final lifecycle placement is checked by
//! `vector_module`. P6.6 adds one checked shared state cursor per clock domain;
//! it advances at the end of each guarded fire. Section 8 lockstep bundles are
//! adapted as one physical `i0` loop whose body contains the unchanged scalar
//! iteration of every lane in canonical order. This preserves each lane's IEEE
//! operation and contraction policy while exposing cross-instance SLP to FIR
//! backends without changing the planar `compute` ABI.
//!
//! General fused serial groups use the same physical sample envelope. A
//! top-rate group owns one `i0` body; a nonzero-clock group owns one contiguous
//! sequence inside exactly one guarded clock island. The independent assembly
//! checker requires delayed reads, state writes, and scalarized internal
//! transports to remain inside that envelope.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use fir::{AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

use super::vector_clock_ad::{
    ClockGuard, ClockIsland, ClockTransportMode, VerifiedVectorClockAdPlan,
};
use super::vector_route::{RoutedDefinition, VectorRegion, VerifiedRoutedFir};
use super::vector_state::{
    DelayTransition, PrefixTransition, RecursionTransition, VectorDelayStorage, VectorStateAction,
    VectorStateInitialValue, VerifiedVectorStatePlan, WaveformTransition,
};
use super::vector_verify::{Placement, ValueType, VectorPlan};

/// Current canonical P6.3b/P6.5 assembly schema.
pub const VECTOR_FIR_ASSEMBLY_VERSION: u32 = 3;

/// Already-lowered non-state statements owned by one checked P4 loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorLoopFirInput {
    pub loop_id: u64,
    pub statements: Vec<FirId>,
}

/// One top-rate output store whose value is produced by a held clock island.
///
/// This is an adapted Rust representation of the C++ post-island output write:
/// ownership is explicit rather than inferred from a mutable `CodeLoop` tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorClockOutputStore {
    pub owner_loop_id: u64,
    pub statement: FirId,
}

/// Concrete statement implementing one accepted P6.1 action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorStateFirAction {
    pub action: VectorStateAction,
    pub statement: FirId,
    /// Statements inserted into the enclosing sample scope. Recursion-step
    /// declarations are flattened so subsequent delay writes can read them.
    pub execution_statements: Vec<FirId>,
}

/// One loop body after state-phase materialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembledVectorLoop {
    pub loop_id: u64,
    pub pre: Vec<VectorStateFirAction>,
    pub exec: Vec<FirId>,
    pub exec_actions: Vec<VectorStateFirAction>,
    pub post: Vec<VectorStateFirAction>,
    /// Complete outer-chunk execution: `pre; for i0; post`.
    pub chunk_statement: FirId,
    /// One serial iteration used when this loop is nested below a clock guard.
    pub iteration_statement: FirId,
}

/// One nested serial clock domain after guard construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembledClockIsland {
    pub domain_id: u64,
    pub parent_domain: Option<u64>,
    pub guard: ClockGuard,
    pub nested_loop_ids: Vec<u64>,
    /// P6.2 `IslandScalar` declarations whose lifetime begins below this guard.
    pub local_declarations: Vec<FirId>,
    /// Optional shared P6.6 state-cursor advance inside this domain guard.
    pub state_cursor_advance: Option<FirId>,
    pub statement: FirId,
}

/// Finite FIR assembly accepted before final module placement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorFirAssembly {
    pub schema_version: u32,
    pub local_declarations: Vec<FirId>,
    pub state_declarations: Vec<FirId>,
    pub clear_statements: Vec<FirId>,
    pub loops: Vec<AssembledVectorLoop>,
    pub islands: Vec<AssembledClockIsland>,
    pub clock_output_stores: Vec<VectorClockOutputStore>,
    pub top_level_statement: FirId,
}

/// Opaque evidence that the P6.3b checker accepted an assembly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedVectorFirAssembly {
    assembly: VectorFirAssembly,
    vector_plan: VectorPlan,
}

impl VerifiedVectorFirAssembly {
    #[must_use]
    pub fn assembly(&self) -> &VectorFirAssembly {
        &self.assembly
    }

    #[must_use]
    pub fn vector_plan(&self) -> &VectorPlan {
        &self.vector_plan
    }

    #[must_use]
    pub fn into_assembly(self) -> VectorFirAssembly {
        self.assembly
    }
}

/// Typed producer/checker failure at the P6.3b boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum VectorFirAssemblyError {
    PlanMismatch {
        artifact: &'static str,
    },
    ReverseAdRequiresScalar {
        signal_id: u64,
    },
    LoopInputCoverage {
        loop_id: u64,
    },
    DuplicateLoopInput {
        loop_id: u64,
    },
    MissingDefinition {
        signal_id: u64,
        loop_id: u64,
    },
    MissingRecursionProjection {
        group: u64,
        index: u64,
    },
    LoopStateCoverage {
        loop_id: u64,
    },
    ClockLoopOwnership {
        loop_id: u64,
    },
    MissingClockValue {
        domain_id: u64,
        signal_id: u64,
    },
    MissingClockParent {
        domain_id: u64,
        parent_id: u64,
    },
    ArithmeticOverflow {
        what: &'static str,
        value: u64,
    },
    UnsupportedValueType {
        signal_id: u64,
    },
    DeclarationShape {
        name: String,
    },
    ActionShape {
        loop_id: u64,
        action: VectorStateAction,
    },
    IslandShape {
        domain_id: u64,
    },
    FusedGroupShape {
        group_id: u64,
    },
    /// A verified lockstep bundle was not emitted as one physical sample loop.
    LockstepBundleShape {
        bundle_id: u64,
    },
    TopLevelShape,
}

impl fmt::Display for VectorFirAssemblyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlanMismatch { artifact } => {
                write!(f, "{artifact} does not belong to the routed vector plan")
            }
            Self::ReverseAdRequiresScalar { signal_id } => write!(
                f,
                "signal {signal_id} requires the certified scalar reverse-AD window"
            ),
            Self::LoopInputCoverage { loop_id } => {
                write!(f, "loop FIR inputs do not cover loop {loop_id} exactly")
            }
            Self::DuplicateLoopInput { loop_id } => {
                write!(f, "loop FIR input {loop_id} is duplicated")
            }
            Self::MissingDefinition { signal_id, loop_id } => write!(
                f,
                "stateful signal {signal_id} has no routed definition in loop {loop_id}"
            ),
            Self::MissingRecursionProjection { group, index } => write!(
                f,
                "recursion group {group} projection {index} has no routed definition"
            ),
            Self::LoopStateCoverage { loop_id } => {
                write!(f, "assembled state actions disagree for loop {loop_id}")
            }
            Self::ClockLoopOwnership { loop_id } => {
                write!(f, "loop {loop_id} belongs to more than one clock island")
            }
            Self::MissingClockValue {
                domain_id,
                signal_id,
            } => write!(
                f,
                "clock island {domain_id} cannot resolve clock signal {signal_id}"
            ),
            Self::MissingClockParent {
                domain_id,
                parent_id,
            } => write!(
                f,
                "clock island {domain_id} references missing parent {parent_id}"
            ),
            Self::ArithmeticOverflow { what, value } => {
                write!(f, "{what} value {value} does not fit FIR i32 arithmetic")
            }
            Self::UnsupportedValueType { signal_id } => {
                write!(f, "signal {signal_id} has tuple-valued state storage")
            }
            Self::DeclarationShape { name } => {
                write!(f, "assembled declaration {name} has an invalid FIR shape")
            }
            Self::ActionShape { loop_id, action } => {
                write!(f, "loop {loop_id} action {action:?} has invalid FIR")
            }
            Self::IslandShape { domain_id } => {
                write!(f, "clock island {domain_id} has an invalid FIR guard")
            }
            Self::FusedGroupShape { group_id } => {
                write!(f, "fused serial group {group_id} has an invalid FIR shape")
            }
            Self::LockstepBundleShape { bundle_id } => {
                write!(f, "lockstep bundle {bundle_id} has an invalid FIR shape")
            }
            Self::TopLevelShape => write!(f, "assembled top-level FIR is not a block"),
        }
    }
}

impl std::error::Error for VectorFirAssemblyError {}

/// Materializes checked P6.1 phases and P6.2 serial islands into concrete FIR.
pub fn assemble_vector_fir(
    routed: &VerifiedRoutedFir,
    state_plan: Option<&VerifiedVectorStatePlan>,
    clock_plan: Option<&VerifiedVectorClockAdPlan>,
    inputs: &[VectorLoopFirInput],
    clock_output_stores: &[VectorClockOutputStore],
    real_type: FirType,
    store: &mut FirStore,
) -> Result<VerifiedVectorFirAssembly, VectorFirAssemblyError> {
    if let Some(state) = state_plan {
        require_same_plan(routed.plan(), state.vector_plan(), "P6.1 state plan")?;
    }
    if let Some(clock) = clock_plan {
        require_same_plan(routed.plan(), clock.vector_plan(), "P6.2 clock/AD plan")?;
        if let Some(fallback) = clock.plan().reverse_ad_fallbacks.first() {
            return Err(VectorFirAssemblyError::ReverseAdRequiresScalar {
                signal_id: fallback.signal_id,
            });
        }
    }

    let input_map = check_loop_inputs(routed, inputs)?;
    let transport_declarations = inspect_transport_declarations(routed, store)?;
    let assembly = {
        let mut builder = FirBuilder::new(store);
        let mut local_declarations = Vec::new();
        let mut state_declarations = Vec::new();
        let mut clear_statements = Vec::new();
        let mut island_declarations = BTreeMap::new();
        classify_transport_declarations(
            &transport_declarations,
            &mut builder,
            &mut local_declarations,
            &mut state_declarations,
            &mut clear_statements,
            &mut island_declarations,
        )?;

        let mut delays = BTreeMap::new();
        let mut recursions = BTreeMap::new();
        let mut prefixes = BTreeMap::new();
        let mut waveforms = BTreeMap::new();
        if let Some(state) = state_plan {
            materialize_state_storage(
                state,
                real_type.clone(),
                &mut builder,
                &mut state_declarations,
                &mut local_declarations,
                &mut clear_statements,
            )?;
            delays.extend(
                state
                    .plan()
                    .delays
                    .iter()
                    .map(|delay| (delay.signal_id, delay)),
            );
            recursions.extend(
                state
                    .plan()
                    .recursions
                    .iter()
                    .map(|recursion| (recursion.group, recursion)),
            );
            prefixes.extend(
                state
                    .plan()
                    .prefixes
                    .iter()
                    .map(|transition| (transition.signal_id, transition)),
            );
            waveforms.extend(
                state
                    .plan()
                    .waveforms
                    .iter()
                    .map(|transition| (transition.signal_id, transition)),
            );
        }

        let mut definitions = definition_map(routed.trace().definitions());
        // A state action may consume a value through an accepted cross-loop
        // route rather than a local definition (notably `prefix` writes).
        // Preserve the independently checked routed value for that consumer.
        for routed_use in routed.trace().uses() {
            definitions
                .entry((
                    VectorRegion::Loop(routed_use.consumer_loop),
                    routed_use.signal_id,
                ))
                .or_insert(routed_use.value);
        }
        let signal_types = routed
            .plan()
            .signals
            .iter()
            .map(|signal| (signal.signal_id, signal.value_type.clone()))
            .collect::<BTreeMap<_, _>>();
        let fused_members_by_loop = routed
            .plan()
            .fused_serial_groups
            .iter()
            .flat_map(|group| {
                group
                    .member_loop_ids
                    .iter()
                    .map(move |&loop_id| (loop_id, group.member_loop_ids.as_slice()))
            })
            .collect::<BTreeMap<_, _>>();
        let materialization = StateMaterializationContext {
            delays: &delays,
            recursions: &recursions,
            prefixes: &prefixes,
            waveforms: &waveforms,
            definitions: &definitions,
            fused_members_by_loop: &fused_members_by_loop,
            signal_types: &signal_types,
            real_type: real_type.clone(),
        };
        let mut loops = Vec::with_capacity(routed.layout().loops().len());
        for region in routed.layout().loops() {
            let phases = state_plan.and_then(|state| {
                state
                    .plan()
                    .loops
                    .iter()
                    .find(|phases| phases.loop_id == region.loop_id)
            });
            loops.push(materialize_loop(
                region.loop_id,
                input_map[&region.loop_id],
                phases,
                &materialization,
                &mut builder,
            )?);
        }

        let clock_materialization = ClockIslandMaterializationContext {
            routed,
            state_plan,
            clock_plan,
            loops: &loops,
            definitions: &definitions,
            island_declarations: &island_declarations,
        };
        let islands = materialize_clock_islands(
            &clock_materialization,
            &mut builder,
            &mut state_declarations,
            &mut clear_statements,
        )?;
        let top_level_statement = materialize_top_level(
            routed,
            state_plan,
            &loops,
            &islands,
            clock_output_stores,
            &mut builder,
        )?;
        VectorFirAssembly {
            schema_version: VECTOR_FIR_ASSEMBLY_VERSION,
            local_declarations,
            state_declarations,
            clear_statements,
            loops,
            islands,
            clock_output_stores: clock_output_stores.to_vec(),
            top_level_statement,
        }
    };
    verify_vector_fir_assembly(routed, state_plan, clock_plan, &assembly, store)?;
    Ok(VerifiedVectorFirAssembly {
        assembly,
        vector_plan: routed.plan().clone(),
    })
}

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

fn independently_expected_clock_cursor(
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

fn state_cursor_advance_matches(
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

fn require_same_plan(
    routed: &VectorPlan,
    artifact: &VectorPlan,
    name: &'static str,
) -> Result<(), VectorFirAssemblyError> {
    if routed == artifact {
        Ok(())
    } else {
        Err(VectorFirAssemblyError::PlanMismatch { artifact: name })
    }
}

fn check_loop_inputs<'a>(
    routed: &VerifiedRoutedFir,
    inputs: &'a [VectorLoopFirInput],
) -> Result<BTreeMap<u64, &'a [FirId]>, VectorFirAssemblyError> {
    let expected = routed
        .layout()
        .loops()
        .iter()
        .map(|region| region.loop_id)
        .collect::<BTreeSet<_>>();
    let mut result = BTreeMap::new();
    for input in inputs {
        if result
            .insert(input.loop_id, input.statements.as_slice())
            .is_some()
        {
            return Err(VectorFirAssemblyError::DuplicateLoopInput {
                loop_id: input.loop_id,
            });
        }
    }
    for loop_id in expected {
        if !result.contains_key(&loop_id) {
            return Err(VectorFirAssemblyError::LoopInputCoverage { loop_id });
        }
    }
    if let Some(extra) = result.keys().find(|loop_id| {
        !routed
            .layout()
            .loops()
            .iter()
            .any(|r| r.loop_id == **loop_id)
    }) {
        return Err(VectorFirAssemblyError::LoopInputCoverage { loop_id: *extra });
    }
    Ok(result)
}

struct TransportDeclaration {
    mode: ClockTransportMode,
    declaration: FirId,
    held: Option<(String, FirType)>,
}

fn inspect_transport_declarations(
    routed: &VerifiedRoutedFir,
    store: &FirStore,
) -> Result<Vec<TransportDeclaration>, VectorFirAssemblyError> {
    routed
        .trace()
        .transports()
        .iter()
        .map(|transport| {
            let held = if matches!(transport.mode, ClockTransportMode::HeldOutput { .. }) {
                match match_fir(store, transport.declaration) {
                    FirMatch::DeclareVar {
                        name,
                        typ,
                        access: AccessType::Struct,
                        init: None,
                    } => Some((name, typ)),
                    _ => {
                        return Err(VectorFirAssemblyError::DeclarationShape {
                            name: format!("transport_{}", transport.transport_id),
                        });
                    }
                }
            } else {
                None
            };
            Ok(TransportDeclaration {
                mode: transport.mode,
                declaration: transport.declaration,
                held,
            })
        })
        .collect()
}

fn classify_transport_declarations(
    transports: &[TransportDeclaration],
    builder: &mut FirBuilder<'_>,
    local: &mut Vec<FirId>,
    state: &mut Vec<FirId>,
    clear: &mut Vec<FirId>,
    island_declarations: &mut BTreeMap<u64, Vec<FirId>>,
) -> Result<(), VectorFirAssemblyError> {
    for transport in transports {
        match transport.mode {
            ClockTransportMode::OuterChunk => {
                local.push(transport.declaration);
            }
            ClockTransportMode::FusedScalar { .. } => {
                local.push(transport.declaration);
            }
            ClockTransportMode::IslandScalar { domain_id } => {
                island_declarations
                    .entry(domain_id)
                    .or_default()
                    .push(transport.declaration);
            }
            ClockTransportMode::HeldOutput { .. } => {
                let (name, typ) = transport
                    .held
                    .as_ref()
                    .expect("held transport was inspected before FIR construction");
                state.push(transport.declaration);
                let zero = zero_value(builder, typ);
                clear.push(builder.store_var(name, AccessType::Struct, zero));
            }
        }
    }
    Ok(())
}

fn materialize_state_storage(
    state_plan: &VerifiedVectorStatePlan,
    real_type: FirType,
    builder: &mut FirBuilder<'_>,
    state: &mut Vec<FirId>,
    local: &mut Vec<FirId>,
    clear: &mut Vec<FirId>,
) -> Result<(), VectorFirAssemblyError> {
    let mut clock_cursors = BTreeSet::new();
    for delay in &state_plan.plan().delays {
        let typ = state_fir_type(delay, real_type.clone())?;
        match &delay.storage {
            VectorDelayStorage::Register {
                local_name,
                persistent_name,
                ..
            } => {
                local.push(builder.declare_var(local_name, typ.clone(), AccessType::Stack, None));
                state.push(builder.declare_var(
                    persistent_name,
                    typ.clone(),
                    AccessType::Struct,
                    None,
                ));
                let zero = zero_value(builder, &typ);
                clear.push(builder.store_var(persistent_name, AccessType::Struct, zero));
            }
            VectorDelayStorage::Copy {
                temporary_name,
                permanent_name,
                history_length,
                temporary_length,
            } => {
                let temporary_length_u64 = *temporary_length;
                let history_length_u64 = *history_length;
                let temporary_length = usize_value("temporary delay length", temporary_length_u64)?;
                let history_length = usize_value("permanent delay length", history_length_u64)?;
                local.push(builder.declare_var(
                    temporary_name,
                    FirType::Array(Box::new(typ.clone()), temporary_length),
                    AccessType::Stack,
                    None,
                ));
                state.push(builder.declare_var(
                    permanent_name,
                    FirType::Array(Box::new(typ.clone()), history_length),
                    AccessType::Struct,
                    None,
                ));
                clear.push(clear_table(
                    builder,
                    permanent_name,
                    AccessType::Struct,
                    &typ,
                    history_length_u64,
                )?);
            }
            VectorDelayStorage::Ring {
                buffer_name,
                index_name,
                index_save_name,
                capacity,
                ..
            } => {
                let capacity_u64 = *capacity;
                let capacity = usize_value("ring capacity", capacity_u64)?;
                state.push(builder.declare_var(
                    buffer_name,
                    FirType::Array(Box::new(typ.clone()), capacity),
                    AccessType::Struct,
                    None,
                ));
                state.push(builder.declare_var(
                    index_name,
                    FirType::Int32,
                    AccessType::Struct,
                    None,
                ));
                state.push(builder.declare_var(
                    index_save_name,
                    FirType::Int32,
                    AccessType::Struct,
                    None,
                ));
                clear.push(clear_table(
                    builder,
                    buffer_name,
                    AccessType::Struct,
                    &typ,
                    capacity_u64,
                )?);
                let zero = builder.int32(0);
                clear.push(builder.store_var(index_name, AccessType::Struct, zero));
                clear.push(builder.store_var(index_save_name, AccessType::Struct, zero));
            }
            VectorDelayStorage::ClockRing {
                buffer_name,
                cursor_name,
                capacity,
                ..
            } => {
                let capacity_u64 = *capacity;
                let capacity = usize_value("clock-ring capacity", capacity_u64)?;
                state.push(builder.declare_var(
                    buffer_name,
                    FirType::Array(Box::new(typ.clone()), capacity),
                    AccessType::Struct,
                    None,
                ));
                clear.push(clear_table(
                    builder,
                    buffer_name,
                    AccessType::Struct,
                    &typ,
                    capacity_u64,
                )?);
                if clock_cursors.insert(cursor_name.clone()) {
                    state.push(builder.declare_var(
                        cursor_name,
                        FirType::Int32,
                        AccessType::Struct,
                        None,
                    ));
                    let zero = builder.int32(0);
                    clear.push(builder.store_var(cursor_name, AccessType::Struct, zero));
                }
            }
        }
    }
    for prefix in &state_plan.plan().prefixes {
        let typ = value_type_to_fir(&prefix.value_type, real_type.clone(), prefix.signal_id)?;
        state.push(builder.declare_var(&prefix.state_name, typ.clone(), AccessType::Struct, None));
        let initial = match prefix.initial {
            VectorStateInitialValue::Int(value) => builder.int32(value),
            VectorStateInitialValue::RealBits(bits) => match typ {
                FirType::Float64 => builder.float64(f64::from_bits(bits)),
                _ => builder.float32(f64::from_bits(bits) as f32),
            },
            VectorStateInitialValue::Zero => zero_value(builder, &typ),
        };
        clear.push(builder.store_var(&prefix.state_name, AccessType::Struct, initial));
    }
    for waveform in &state_plan.plan().waveforms {
        state.push(builder.declare_var(
            &waveform.index_name,
            FirType::Int32,
            AccessType::Struct,
            None,
        ));
        let zero = builder.int32(0);
        clear.push(builder.store_var(&waveform.index_name, AccessType::Struct, zero));
    }
    Ok(())
}

struct StateMaterializationContext<'a> {
    delays: &'a BTreeMap<u64, &'a DelayTransition>,
    recursions: &'a BTreeMap<u64, &'a RecursionTransition>,
    prefixes: &'a BTreeMap<u64, &'a PrefixTransition>,
    waveforms: &'a BTreeMap<u64, &'a WaveformTransition>,
    definitions: &'a BTreeMap<(VectorRegion, u64), FirId>,
    fused_members_by_loop: &'a BTreeMap<u64, &'a [u64]>,
    signal_types: &'a BTreeMap<u64, ValueType>,
    real_type: FirType,
}

fn fused_member_definition(
    loop_id: u64,
    signal_id: u64,
    context: &StateMaterializationContext<'_>,
) -> Option<FirId> {
    if let Some(value) = context
        .definitions
        .get(&(VectorRegion::Loop(loop_id), signal_id))
        .copied()
    {
        return Some(value);
    }
    let members = context.fused_members_by_loop.get(&loop_id)?;
    let mut values = context
        .definitions
        .iter()
        .filter_map(|((region, id), &value)| {
            (*id == signal_id
            && matches!(region, VectorRegion::Loop(owner) if members.binary_search(owner).is_ok()))
        .then_some(value)
        });
    let value = values.next()?;
    values.all(|candidate| candidate == value).then_some(value)
}

fn materialize_loop(
    loop_id: u64,
    inputs: &[FirId],
    phases: Option<&super::vector_state::LoopStatePhases>,
    context: &StateMaterializationContext<'_>,
    builder: &mut FirBuilder<'_>,
) -> Result<AssembledVectorLoop, VectorFirAssemblyError> {
    let mut recursion_values = BTreeMap::new();
    let mut pre = Vec::new();
    let mut exec_actions = Vec::new();
    let mut post = Vec::new();
    if let Some(phases) = phases {
        for action in &phases.pre {
            pre.push(materialize_action(
                loop_id,
                action,
                context,
                &mut recursion_values,
                builder,
            )?);
        }
        for action in &phases.exec {
            exec_actions.push(materialize_action(
                loop_id,
                action,
                context,
                &mut recursion_values,
                builder,
            )?);
        }
        for action in &phases.post {
            post.push(materialize_action(
                loop_id,
                action,
                context,
                &mut recursion_values,
                builder,
            )?);
        }
    }
    let mut exec = inputs.to_vec();
    for action in &exec_actions {
        exec.extend(action.execution_statements.iter().copied());
    }
    let iteration_statement = builder.block(&exec);
    let sample_loop = sample_loop(builder, iteration_statement);
    let mut chunk = pre
        .iter()
        .map(|action| action.statement)
        .collect::<Vec<_>>();
    chunk.push(sample_loop);
    chunk.extend(post.iter().map(|action| action.statement));
    let chunk_statement = builder.block(&chunk);
    Ok(AssembledVectorLoop {
        loop_id,
        pre,
        exec,
        exec_actions,
        post,
        chunk_statement,
        iteration_statement,
    })
}

fn materialize_action(
    loop_id: u64,
    action: &VectorStateAction,
    context: &StateMaterializationContext<'_>,
    recursion_values: &mut BTreeMap<u64, FirId>,
    builder: &mut FirBuilder<'_>,
) -> Result<VectorStateFirAction, VectorFirAssemblyError> {
    let delays = context.delays;
    let recursions = context.recursions;
    let prefixes = context.prefixes;
    let waveforms = context.waveforms;
    let definitions = context.definitions;
    let signal_types = context.signal_types;
    let real_type = context.real_type.clone();
    let mut execution_statements = None;
    let statement = match action {
        VectorStateAction::DelayRegisterLoad { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Register {
                local_name,
                persistent_name,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let typ = state_fir_type(delay, real_type)?;
            let value = builder.load_var(persistent_name, AccessType::Struct, typ);
            builder.store_var(local_name, AccessType::Stack, value)
        }
        VectorStateAction::DelayCopyIn { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Copy {
                temporary_name,
                permanent_name,
                history_length,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let typ = state_fir_type(delay, real_type)?;
            let index = builder.load_var("vdelay_copy", AccessType::Loop, FirType::Int32);
            let value = builder.load_table(permanent_name, AccessType::Struct, index, typ);
            let store = builder.store_table(temporary_name, AccessType::Stack, index, value);
            let upper = fir_i32(builder, "copy history", *history_length)?;
            let body = builder.block(&[store]);
            builder.simple_for_loop("vdelay_copy", upper, body, false)
        }
        VectorStateAction::DelayRingAdvance { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Ring {
                index_name,
                index_save_name,
                mask,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let index = builder.load_var(index_name, AccessType::Struct, FirType::Int32);
            let saved = builder.load_var(index_save_name, AccessType::Struct, FirType::Int32);
            let added = builder.binop(FirBinOp::Add, index, saved, FirType::Int32);
            let mask = fir_i32(builder, "ring mask", *mask)?;
            let masked = builder.binop(FirBinOp::And, added, mask, FirType::Int32);
            builder.store_var(index_name, AccessType::Struct, masked)
        }
        VectorStateAction::RecursionStep { group } => {
            let recursion = recursions[group];
            let mut declarations = Vec::with_capacity(recursion.projections.len());
            for projection in &recursion.projections {
                let value = projection
                    .signal_ids
                    .iter()
                    .find_map(|signal_id| fused_member_definition(loop_id, *signal_id, context))
                    .or_else(|| {
                        fused_member_definition(loop_id, projection.value_signal_id, context)
                    });
                let value = value.ok_or(VectorFirAssemblyError::MissingRecursionProjection {
                    group: *group,
                    index: projection.index,
                })?;
                let signal_id = projection
                    .signal_ids
                    .first()
                    .copied()
                    .unwrap_or(projection.value_signal_id);
                let typ = value_type_to_fir(
                    signal_types.get(&signal_id).ok_or(
                        VectorFirAssemblyError::MissingRecursionProjection {
                            group: *group,
                            index: projection.index,
                        },
                    )?,
                    real_type.clone(),
                    signal_id,
                )
                .map_err(|_| {
                    VectorFirAssemblyError::MissingRecursionProjection {
                        group: *group,
                        index: projection.index,
                    }
                })?;
                let name = recursion_name(*group, projection.index);
                declarations.push(builder.declare_var(
                    &name,
                    typ.clone(),
                    AccessType::Stack,
                    Some(value),
                ));
                let load = builder.load_var(name, AccessType::Stack, typ);
                for signal_id in &projection.signal_ids {
                    recursion_values.insert(*signal_id, load);
                }
            }
            execution_statements = Some(declarations.clone());
            builder.block(&declarations)
        }
        VectorStateAction::DelayWrite { signal_id } => {
            let delay = delays[signal_id];
            let value = recursion_values.get(signal_id).copied().or_else(|| {
                definitions
                    .get(&(VectorRegion::Loop(loop_id), *signal_id))
                    .copied()
            });
            let value = value.ok_or(VectorFirAssemblyError::MissingDefinition {
                signal_id: *signal_id,
                loop_id,
            })?;
            let local = local_index(builder);
            match &delay.storage {
                VectorDelayStorage::Register { local_name, .. } => {
                    builder.store_var(local_name, AccessType::Stack, value)
                }
                VectorDelayStorage::Copy {
                    temporary_name,
                    history_length,
                    ..
                } => {
                    let history = fir_i32(builder, "copy history", *history_length)?;
                    let index = builder.binop(FirBinOp::Add, history, local, FirType::Int32);
                    builder.store_table(temporary_name, AccessType::Stack, index, value)
                }
                VectorDelayStorage::Ring {
                    buffer_name,
                    index_name,
                    mask,
                    ..
                } => {
                    let index = builder.load_var(index_name, AccessType::Struct, FirType::Int32);
                    let added = builder.binop(FirBinOp::Add, index, local, FirType::Int32);
                    let mask = fir_i32(builder, "ring mask", *mask)?;
                    let masked = builder.binop(FirBinOp::And, added, mask, FirType::Int32);
                    builder.store_table(buffer_name, AccessType::Struct, masked, value)
                }
                VectorDelayStorage::ClockRing {
                    buffer_name,
                    cursor_name,
                    mask,
                    ..
                } => {
                    let cursor = builder.load_var(cursor_name, AccessType::Struct, FirType::Int32);
                    let mask = fir_i32(builder, "clock-ring mask", *mask)?;
                    let index = builder.binop(FirBinOp::And, cursor, mask, FirType::Int32);
                    builder.store_table(buffer_name, AccessType::Struct, index, value)
                }
            }
        }
        VectorStateAction::PrefixWrite { signal_id } => {
            let transition = prefixes[signal_id];
            let value = definitions
                .get(&(VectorRegion::Loop(loop_id), transition.value_signal_id))
                .copied()
                .ok_or(VectorFirAssemblyError::MissingDefinition {
                    signal_id: transition.value_signal_id,
                    loop_id,
                })?;
            builder.store_var(&transition.state_name, AccessType::Struct, value)
        }
        VectorStateAction::WaveformAdvance { signal_id } => {
            let transition = waveforms[signal_id];
            let index =
                builder.load_var(&transition.index_name, AccessType::Struct, FirType::Int32);
            let one = builder.int32(1);
            let next = builder.binop(FirBinOp::Add, index, one, FirType::Int32);
            let length = fir_i32(builder, "waveform length", transition.length)?;
            let wrapped = builder.binop(FirBinOp::Rem, next, length, FirType::Int32);
            builder.store_var(&transition.index_name, AccessType::Struct, wrapped)
        }
        VectorStateAction::DelayRegisterStore { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Register {
                local_name,
                persistent_name,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let typ = state_fir_type(delay, real_type)?;
            let value = builder.load_var(local_name, AccessType::Stack, typ);
            builder.store_var(persistent_name, AccessType::Struct, value)
        }
        VectorStateAction::DelayCopyOut { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Copy {
                temporary_name,
                permanent_name,
                history_length,
                ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let typ = state_fir_type(delay, real_type)?;
            let index = builder.load_var("vdelay_copy", AccessType::Loop, FirType::Int32);
            let count = builder.load_var("vcount", AccessType::Stack, FirType::Int32);
            let source_index = builder.binop(FirBinOp::Add, count, index, FirType::Int32);
            let value = builder.load_table(temporary_name, AccessType::Stack, source_index, typ);
            let store = builder.store_table(permanent_name, AccessType::Struct, index, value);
            let upper = fir_i32(builder, "copy history", *history_length)?;
            let body = builder.block(&[store]);
            builder.simple_for_loop("vdelay_copy", upper, body, false)
        }
        VectorStateAction::DelayRingSaveAdvance { signal_id } => {
            let delay = delays[signal_id];
            let VectorDelayStorage::Ring {
                index_save_name, ..
            } = &delay.storage
            else {
                return Err(VectorFirAssemblyError::ActionShape {
                    loop_id,
                    action: action.clone(),
                });
            };
            let count = builder.load_var("vcount", AccessType::Stack, FirType::Int32);
            builder.store_var(index_save_name, AccessType::Struct, count)
        }
    };
    Ok(VectorStateFirAction {
        action: action.clone(),
        statement,
        execution_statements: execution_statements.unwrap_or_else(|| vec![statement]),
    })
}

struct ClockIslandMaterializationContext<'a> {
    routed: &'a VerifiedRoutedFir,
    state_plan: Option<&'a VerifiedVectorStatePlan>,
    clock_plan: Option<&'a VerifiedVectorClockAdPlan>,
    loops: &'a [AssembledVectorLoop],
    definitions: &'a BTreeMap<(VectorRegion, u64), FirId>,
    island_declarations: &'a BTreeMap<u64, Vec<FirId>>,
}

fn materialize_clock_islands(
    context: &ClockIslandMaterializationContext<'_>,
    builder: &mut FirBuilder<'_>,
    state_declarations: &mut Vec<FirId>,
    clear_statements: &mut Vec<FirId>,
) -> Result<Vec<AssembledClockIsland>, VectorFirAssemblyError> {
    let routed = context.routed;
    let state_plan = context.state_plan;
    let clock_plan = context.clock_plan;
    let loops = context.loops;
    let definitions = context.definitions;
    let island_declarations = context.island_declarations;
    let Some(clock_plan) = clock_plan else {
        return Ok(Vec::new());
    };
    let mut owner = BTreeMap::new();
    for island in &clock_plan.plan().clock_islands {
        for loop_id in &island.nested_loop_ids {
            if owner.insert(*loop_id, island.domain_id).is_some() {
                return Err(VectorFirAssemblyError::ClockLoopOwnership { loop_id: *loop_id });
            }
        }
    }
    let loop_by_id = loops
        .iter()
        .map(|assembled| (assembled.loop_id, assembled))
        .collect::<BTreeMap<_, _>>();
    let island_by_id = clock_plan
        .plan()
        .clock_islands
        .iter()
        .map(|island| (island.domain_id, island))
        .collect::<BTreeMap<_, _>>();
    for island in &clock_plan.plan().clock_islands {
        if let Some(parent) = island.parent_domain
            && !island_by_id.contains_key(&parent)
        {
            return Err(VectorFirAssemblyError::MissingClockParent {
                domain_id: island.domain_id,
                parent_id: parent,
            });
        }
    }
    let mut statements = BTreeMap::new();
    let mut state_cursor_advances = BTreeMap::new();
    let mut pending = clock_plan.plan().clock_islands.len();
    while statements.len() < pending {
        let before = statements.len();
        for island in &clock_plan.plan().clock_islands {
            if statements.contains_key(&island.domain_id) {
                continue;
            }
            let children = clock_plan
                .plan()
                .clock_islands
                .iter()
                .filter(|child| child.parent_domain == Some(island.domain_id))
                .collect::<Vec<_>>();
            if children
                .iter()
                .any(|child| !statements.contains_key(&child.domain_id))
            {
                continue;
            }
            let local_declarations = island_declarations
                .get(&island.domain_id)
                .cloned()
                .unwrap_or_default();
            let scheduled_loop_ids = scheduled_island_loop_ids(routed, island);
            let mut body = local_declarations.clone();
            body.extend(
                scheduled_loop_ids
                    .iter()
                    .map(|loop_id| loop_by_id[loop_id].iteration_statement),
            );
            body.extend(children.iter().map(|child| statements[&child.domain_id]));
            if let Some(cursor_name) = clock_cursor_for_domain(state_plan, island.domain_id)? {
                let cursor = builder.load_var(&cursor_name, AccessType::Struct, FirType::Int32);
                let one = builder.int32(1);
                let next = builder.binop(FirBinOp::Add, cursor, one, FirType::Int32);
                let advance = builder.store_var(&cursor_name, AccessType::Struct, next);
                body.push(advance);
                state_cursor_advances.insert(island.domain_id, advance);
            }
            let body = builder.block(&body);
            let clock = resolve_clock_value(routed, definitions, island)?;
            let guarded = build_guard(
                island,
                clock,
                body,
                builder,
                state_declarations,
                clear_statements,
            );
            statements.insert(island.domain_id, guarded);
        }
        if statements.len() == before {
            let island = clock_plan
                .plan()
                .clock_islands
                .iter()
                .find(|island| !statements.contains_key(&island.domain_id))
                .expect("pending island count is nonzero");
            return Err(VectorFirAssemblyError::MissingClockParent {
                domain_id: island.domain_id,
                parent_id: island.parent_domain.unwrap_or(island.domain_id),
            });
        }
        pending = clock_plan.plan().clock_islands.len();
    }
    Ok(clock_plan
        .plan()
        .clock_islands
        .iter()
        .map(|island| AssembledClockIsland {
            domain_id: island.domain_id,
            parent_domain: island.parent_domain,
            guard: island.guard,
            nested_loop_ids: scheduled_island_loop_ids(routed, island),
            local_declarations: island_declarations
                .get(&island.domain_id)
                .cloned()
                .unwrap_or_default(),
            state_cursor_advance: state_cursor_advances.get(&island.domain_id).copied(),
            statement: statements[&island.domain_id],
        })
        .collect())
}

fn scheduled_island_loop_ids(routed: &VerifiedRoutedFir, island: &ClockIsland) -> Vec<u64> {
    let members = island
        .nested_loop_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    routed
        .layout()
        .loops()
        .iter()
        .filter_map(|region| members.contains(&region.loop_id).then_some(region.loop_id))
        .collect()
}

fn clock_cursor_for_domain(
    state_plan: Option<&VerifiedVectorStatePlan>,
    domain_id: u64,
) -> Result<Option<String>, VectorFirAssemblyError> {
    let Some(state_plan) = state_plan else {
        return Ok(None);
    };
    let names = state_plan
        .plan()
        .delays
        .iter()
        .filter_map(|delay| match &delay.storage {
            VectorDelayStorage::ClockRing {
                cursor_name,
                domain_id: delay_domain,
                ..
            } if *delay_domain == domain_id => Some(cursor_name.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if names.len() > 1 {
        return Err(VectorFirAssemblyError::IslandShape { domain_id });
    }
    Ok(names.into_iter().next())
}

fn materialize_top_level(
    routed: &VerifiedRoutedFir,
    state_plan: Option<&VerifiedVectorStatePlan>,
    loops: &[AssembledVectorLoop],
    islands: &[AssembledClockIsland],
    clock_output_stores: &[VectorClockOutputStore],
    builder: &mut FirBuilder<'_>,
) -> Result<FirId, VectorFirAssemblyError> {
    let owned = islands
        .iter()
        .flat_map(|island| island.nested_loop_ids.iter().copied())
        .collect::<BTreeSet<_>>();
    let direct_owner = islands
        .iter()
        .flat_map(|island| {
            island
                .nested_loop_ids
                .iter()
                .map(move |loop_id| (*loop_id, island.domain_id))
        })
        .collect::<BTreeMap<_, _>>();
    let island_by_id = islands
        .iter()
        .map(|island| (island.domain_id, island))
        .collect::<BTreeMap<_, _>>();
    let mut stores_by_root = BTreeMap::<u64, Vec<FirId>>::new();
    for output in clock_output_stores {
        let mut domain = *direct_owner.get(&output.owner_loop_id).ok_or(
            VectorFirAssemblyError::ClockLoopOwnership {
                loop_id: output.owner_loop_id,
            },
        )?;
        while let Some(parent) = island_by_id[&domain].parent_domain {
            domain = parent;
        }
        stores_by_root
            .entry(domain)
            .or_default()
            .push(output.statement);
    }
    let mut roots = Vec::new();
    for island in islands
        .iter()
        .filter(|island| island.parent_domain.is_none())
    {
        let mut sample_body = vec![island.statement];
        sample_body.extend(stores_by_root.remove(&island.domain_id).unwrap_or_default());
        let statement = builder.block(&sample_body);
        roots.push((island.nested_loop_ids.first().copied(), statement));
    }
    let loop_by_id = loops
        .iter()
        .map(|assembled| (assembled.loop_id, assembled))
        .collect::<BTreeMap<_, _>>();
    let fused_group_by_member = routed
        .plan()
        .fused_serial_groups
        .iter()
        .flat_map(|group| {
            group
                .member_loop_ids
                .iter()
                .map(move |&loop_id| (loop_id, group.group_id))
        })
        .collect::<BTreeMap<_, _>>();
    let fused_members_by_group = routed
        .plan()
        .fused_serial_groups
        .iter()
        .map(|group| {
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
            (group.group_id, members)
        })
        .collect::<BTreeMap<_, _>>();
    let lockstep_bundle_by_member = routed
        .plan()
        .lockstep_bundles
        .iter()
        .flat_map(|bundle| {
            bundle
                .member_loop_ids
                .iter()
                .map(move |&loop_id| (loop_id, bundle.bundle_id))
        })
        .collect::<BTreeMap<_, _>>();
    let lockstep_members_by_bundle = routed
        .plan()
        .lockstep_bundles
        .iter()
        .map(|bundle| (bundle.bundle_id, bundle.member_loop_ids.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let register_bundles = state_plan
        .into_iter()
        .flat_map(|state| &state.plan().lockstep_register_bundles)
        .map(|bundle| bundle.bundle_id)
        .collect::<BTreeSet<_>>();
    let mut body = Vec::new();
    for region in routed.layout().loops() {
        if !owned.contains(&region.loop_id) {
            body.extend(
                loop_by_id[&region.loop_id]
                    .pre
                    .iter()
                    .map(|action| action.statement),
            );
        }
    }
    for region in routed.layout().loops() {
        if !owned.contains(&region.loop_id) {
            if let Some(group_id) = fused_group_by_member.get(&region.loop_id) {
                let members = &fused_members_by_group[group_id];
                if members.first() == Some(&region.loop_id) {
                    let iterations = members
                        .iter()
                        .map(|loop_id| loop_by_id[loop_id].iteration_statement)
                        .collect::<Vec<_>>();
                    let fused_body = builder.block(&iterations);
                    body.push(sample_loop(builder, fused_body));
                }
            } else if let Some(bundle_id) = lockstep_bundle_by_member.get(&region.loop_id) {
                let members = lockstep_members_by_bundle[bundle_id];
                if members.first() == Some(&region.loop_id) {
                    let statements = if !register_bundles.contains(bundle_id) {
                        members
                            .iter()
                            .map(|loop_id| loop_by_id[loop_id].iteration_statement)
                            .collect::<Vec<_>>()
                    } else {
                        let width = members
                            .first()
                            .map(|loop_id| loop_by_id[loop_id].exec.len())
                            .unwrap_or(0);
                        if members
                            .iter()
                            .all(|loop_id| loop_by_id[loop_id].exec.len() == width)
                        {
                            let mut transposed = Vec::with_capacity(width * members.len());
                            for index in 0..width {
                                for loop_id in members {
                                    transposed.push(loop_by_id[loop_id].exec[index]);
                                }
                            }
                            transposed
                        } else {
                            members
                                .iter()
                                .map(|loop_id| loop_by_id[loop_id].iteration_statement)
                                .collect()
                        }
                    };
                    let lockstep_body = builder.block(&statements);
                    body.push(sample_loop(builder, lockstep_body));
                }
            } else {
                body.push(sample_loop(
                    builder,
                    loop_by_id[&region.loop_id].iteration_statement,
                ));
            }
        }
        for (first_loop, statement) in &roots {
            if *first_loop == Some(region.loop_id) {
                body.push(sample_loop(builder, *statement));
            }
        }
    }
    for region in routed.layout().loops() {
        if !owned.contains(&region.loop_id) {
            body.extend(
                loop_by_id[&region.loop_id]
                    .post
                    .iter()
                    .map(|action| action.statement),
            );
        }
    }
    if body.is_empty() && !roots.is_empty() {
        body.extend(
            roots
                .into_iter()
                .map(|(_, statement)| sample_loop(builder, statement)),
        );
    }
    Ok(builder.block(&body))
}

/// Independently proves the physical FIR envelope of each checked fused group.
///
/// Top-rate members must be the exact body of one physical sample loop, with
/// state setup/commit outside it. Nonzero-clock members must be consecutive in
/// one exact island and have no escaped pre/post action. In both forms, every
/// recorded delayed read, state write, and fused scalar transport must occur
/// inside the selected body.
fn verify_assembled_fused_serial_groups(
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
            if definitions.len() != 1 || !body_nodes.contains(&definitions[0].value) {
                return Err(reject());
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
                || transport
                    .load
                    .is_none_or(|value| !body_nodes.contains(&value))
            {
                return Err(reject());
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
fn verify_assembled_lockstep_bundles(
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
pub(super) fn fir_reachable(store: &FirStore, root: FirId) -> BTreeSet<FirId> {
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !seen.insert(node) {
            continue;
        }
        match match_fir(store, node) {
            FirMatch::Block(words) | FirMatch::FunCall { args: words, .. } => pending.extend(words),
            FirMatch::BinOp { lhs, rhs, .. } => pending.extend([lhs, rhs]),
            FirMatch::Neg { value, .. }
            | FirMatch::Cast { value, .. }
            | FirMatch::Bitcast { value, .. }
            | FirMatch::Drop(value) => pending.push(value),
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                ..
            } => pending.extend([cond, then_value, else_value]),
            FirMatch::DeclareVar { init, .. } => pending.extend(init),
            FirMatch::StoreVar { value, .. } => pending.push(value),
            FirMatch::LoadTable { index, .. } => pending.push(index),
            FirMatch::LoadSoundfileLength { part, .. }
            | FirMatch::LoadSoundfileRate { part, .. } => pending.push(part),
            FirMatch::LoadSoundfileBuffer {
                chan, part, idx, ..
            } => pending.extend([chan, part, idx]),
            FirMatch::StoreTable { index, value, .. } => pending.extend([index, value]),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                pending.extend([cond, then_block]);
                pending.extend(else_block);
            }
            FirMatch::Control { cond, stmt } => pending.extend([cond, stmt]),
            FirMatch::ForLoop {
                init,
                end,
                step,
                body,
                ..
            } => pending.extend([init, end, step, body]),
            FirMatch::SimpleForLoop { upper, body, .. } => pending.extend([upper, body]),
            FirMatch::IteratorForLoop { body, .. } => pending.push(body),
            FirMatch::WhileLoop { cond, body } => pending.extend([cond, body]),
            _ => {}
        }
    }
    seen
}

fn verify_clock_output_stores(
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

fn contains_statement(store: &FirStore, root: FirId, target: FirId) -> bool {
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

fn resolve_clock_value(
    routed: &VerifiedRoutedFir,
    definitions: &BTreeMap<(VectorRegion, u64), FirId>,
    island: &ClockIsland,
) -> Result<FirId, VectorFirAssemblyError> {
    routed
        .trace()
        .uses()
        .iter()
        .find(|use_| {
            use_.signal_id == island.clock_signal_id
                && use_.consumer_loop == island.boundary_loop_id
        })
        .map(|use_| use_.value)
        .or_else(|| {
            definitions
                .get(&(VectorRegion::Control, island.clock_signal_id))
                .copied()
        })
        .or_else(|| {
            definitions
                .get(&(
                    VectorRegion::Loop(island.boundary_loop_id),
                    island.clock_signal_id,
                ))
                .copied()
        })
        .ok_or(VectorFirAssemblyError::MissingClockValue {
            domain_id: island.domain_id,
            signal_id: island.clock_signal_id,
        })
}

fn build_guard(
    island: &ClockIsland,
    clock: FirId,
    body: FirId,
    builder: &mut FirBuilder<'_>,
    state_declarations: &mut Vec<FirId>,
    clear_statements: &mut Vec<FirId>,
) -> FirId {
    match island.guard {
        ClockGuard::BooleanOnDemand => {
            let zero = builder.int32(0);
            let cond = builder.binop(FirBinOp::Ne, clock, zero, FirType::Bool);
            builder.if_(cond, body, None)
        }
        ClockGuard::CountedOnDemand | ClockGuard::CountedUpsampling => builder.simple_for_loop(
            format!("vclock_d{}_fire", island.domain_id),
            clock,
            body,
            false,
        ),
        ClockGuard::DownsampleModulo => {
            let name = format!("vclock_d{}_counter", island.domain_id);
            state_declarations.push(builder.declare_var(
                &name,
                FirType::Int32,
                AccessType::Struct,
                None,
            ));
            let zero = builder.int32(0);
            clear_statements.push(builder.store_var(&name, AccessType::Struct, zero));
            let counter = builder.load_var(&name, AccessType::Struct, FirType::Int32);
            let zero = builder.int32(0);
            let cond = builder.binop(FirBinOp::Eq, counter, zero, FirType::Bool);
            let guarded = builder.if_(cond, body, None);
            let counter = builder.load_var(&name, AccessType::Struct, FirType::Int32);
            let one = builder.int32(1);
            let next = builder.binop(FirBinOp::Add, counter, one, FirType::Int32);
            let modulo = builder.binop(FirBinOp::Rem, next, clock, FirType::Int32);
            let update = builder.store_var(name, AccessType::Struct, modulo);
            builder.block(&[guarded, update])
        }
    }
}

fn verify_action_shape(
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

fn waveform_advance_matches(
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

fn simple_copy_matches(
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

fn guard_shape_matches(
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

fn expected_island_declarations(routed: &VerifiedRoutedFir, domain_id: u64) -> Vec<FirId> {
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

fn find_delay(state: &VerifiedVectorStatePlan, signal_id: u64) -> &DelayTransition {
    state
        .plan()
        .delays
        .iter()
        .find(|delay| delay.signal_id == signal_id)
        .expect("verified state action references a verified delay")
}

fn definition_map(definitions: &[RoutedDefinition]) -> BTreeMap<(VectorRegion, u64), FirId> {
    definitions
        .iter()
        .map(|definition| ((definition.region, definition.signal_id), definition.value))
        .collect()
}

fn state_fir_type(
    delay: &DelayTransition,
    real_type: FirType,
) -> Result<FirType, VectorFirAssemblyError> {
    match delay.value_type {
        ValueType::Int => Ok(FirType::Int32),
        ValueType::Real => Ok(real_type),
        ValueType::Sound | ValueType::Tuple(_) => {
            Err(VectorFirAssemblyError::UnsupportedValueType {
                signal_id: delay.signal_id,
            })
        }
    }
}

fn value_type_to_fir(
    value_type: &ValueType,
    real_type: FirType,
    signal_id: u64,
) -> Result<FirType, VectorFirAssemblyError> {
    match value_type {
        ValueType::Int => Ok(FirType::Int32),
        ValueType::Real => Ok(real_type),
        ValueType::Sound | ValueType::Tuple(_) => {
            Err(VectorFirAssemblyError::UnsupportedValueType { signal_id })
        }
    }
}

fn zero_value(builder: &mut FirBuilder<'_>, typ: &FirType) -> FirId {
    match typ {
        FirType::Float32 | FirType::FaustFloat => builder.float32(0.0),
        FirType::Float64 => builder.float64(0.0),
        FirType::Int64 => builder.int64(0),
        FirType::Bool => builder.bool_(false),
        _ => builder.int32(0),
    }
}

fn clear_table(
    builder: &mut FirBuilder<'_>,
    name: &str,
    access: AccessType,
    typ: &FirType,
    length: u64,
) -> Result<FirId, VectorFirAssemblyError> {
    let index = builder.load_var("vclear", AccessType::Loop, FirType::Int32);
    let zero = zero_value(builder, typ);
    let store = builder.store_table(name, access, index, zero);
    let upper = fir_i32(builder, "clear length", length)?;
    let body = builder.block(&[store]);
    Ok(builder.simple_for_loop("vclear", upper, body, false))
}

fn sample_loop(builder: &mut FirBuilder<'_>, body: FirId) -> FirId {
    let start = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    let init = builder.declare_var("i0", FirType::Int32, AccessType::Loop, Some(start));
    let count = builder.load_var("vcount", AccessType::Stack, FirType::Int32);
    let end = builder.binop(FirBinOp::Add, start, count, FirType::Int32);
    let one = builder.int32(1);
    builder.for_loop("i0", init, end, one, body, false)
}

fn local_index(builder: &mut FirBuilder<'_>) -> FirId {
    let index = builder.load_var("i0", AccessType::Loop, FirType::Int32);
    let base = builder.load_var("vindex", AccessType::Loop, FirType::Int32);
    builder.binop(FirBinOp::Sub, index, base, FirType::Int32)
}

fn recursion_name(group: u64, index: u64) -> String {
    format!("vrec_g{group}_p{index}_next")
}

fn fir_i32(
    builder: &mut FirBuilder<'_>,
    what: &'static str,
    value: u64,
) -> Result<FirId, VectorFirAssemblyError> {
    let value = i32::try_from(value)
        .map_err(|_| VectorFirAssemblyError::ArithmeticOverflow { what, value })?;
    Ok(builder.int32(value))
}

fn usize_value(what: &'static str, value: u64) -> Result<usize, VectorFirAssemblyError> {
    usize::try_from(value).map_err(|_| VectorFirAssemblyError::ArithmeticOverflow { what, value })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::SchedulingStrategy;
    use crate::signal_fir::vector_clock_ad::{
        ForwardAdPolicy, VECTOR_CLOCK_AD_PLAN_VERSION, VectorClockAdPlan,
        verified_vector_clock_ad_plan_for_test,
    };
    use crate::signal_fir::vector_plan::verified_vector_plan_for_test;
    use crate::signal_fir::vector_route::{RouteResolution, VectorRouteSession};
    use crate::signal_fir::vector_state::{
        DelayTransition, LoopStatePhases, RecursionProjectionTransition, RecursionTransition,
        VECTOR_STATE_PLAN_VERSION, VectorStatePlan, verified_vector_state_plan_for_test,
    };
    use crate::signal_fir::vector_verify::{
        EpochRecord, FusedSerialGroupRecord, IsoLeafMapping, IsoRootWitness, LockstepBundleRecord,
        LockstepLaneRecord, LoopEdge, LoopKind, LoopRecord, Placement, Rate, SignalRecord,
        TransportRecord, VecSafeWitness, VectorPlan, Vectorability, WitnessKind,
    };

    fn lockstep_vector_plan() -> super::super::vector_plan::VerifiedVectorPlan {
        verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            vec_size: 8,
            signals: (0..2)
                .map(|lane| SignalRecord {
                    signal_id: 10 + lane,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Scal,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Owned(lane),
                    duplicable: true,
                })
                .collect(),
            loops: (0..2)
                .map(|lane| LoopRecord {
                    loop_id: lane,
                    stable_name: format!("lockstep_lane_{lane}"),
                    kind: LoopKind::Lockstep { width: 2 },
                    roots: vec![10 + lane],
                    epoch_id: 0,
                })
                .collect(),
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0, 1],
            }],
            transports: vec![],
            data_edges: vec![],
            effect_edges: vec![],
            vec_safe_witnesses: (0..2)
                .map(|loop_id| VecSafeWitness {
                    loop_id,
                    witness_kind: WitnessKind::SerialStateInternal,
                })
                .collect(),
            fused_serial_groups: vec![],
            lockstep_bundles: vec![LockstepBundleRecord {
                bundle_id: 0,
                representative_loop_id: 0,
                member_loop_ids: vec![0, 1],
                lanes: (0..2)
                    .map(|lane| LockstepLaneRecord {
                        loop_id: lane,
                        recursion_group: 20 + lane,
                        roots: vec![IsoRootWitness {
                            representative_root: 10,
                            lane_root: 10 + lane,
                            shape_hash: 0x55,
                            leaf_mapping: vec![IsoLeafMapping {
                                representative_signal_id: 10,
                                lane_signal_id: 10 + lane,
                            }],
                        }],
                    })
                    .collect(),
            }],
        })
    }

    #[test]
    fn lockstep_lanes_share_one_physical_sample_loop() {
        let vector = lockstep_vector_plan();
        let mut store = FirStore::new();
        let values = [
            FirBuilder::new(&mut store).float32(0.25),
            FirBuilder::new(&mut store).float32(0.5),
        ];
        let (mut route, _) = VectorRouteSession::new(
            &vector,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .expect("lockstep route");
        for (loop_id, value) in values.into_iter().enumerate() {
            route
                .define_in_loop(loop_id as u64, 10 + loop_id as u64, value, &mut store)
                .expect("lane definition");
        }
        let routed = route.finish(&store).expect("finish lockstep route");
        let inputs = values
            .into_iter()
            .enumerate()
            .map(|(loop_id, value)| VectorLoopFirInput {
                loop_id: loop_id as u64,
                statements: vec![FirBuilder::new(&mut store).drop_(value)],
            })
            .collect::<Vec<_>>();
        let verified = assemble_vector_fir(
            &routed,
            None,
            None,
            &inputs,
            &[],
            FirType::Float32,
            &mut store,
        )
        .expect("assemble lockstep lanes");
        let assembly = verified.assembly();
        let FirMatch::Block(top_level) = match_fir(&store, assembly.top_level_statement) else {
            panic!("top-level block");
        };
        let [physical_loop] = top_level.as_slice() else {
            panic!("exactly one physical loop");
        };
        let FirMatch::ForLoop { body, .. } = match_fir(&store, *physical_loop) else {
            panic!("physical sample loop");
        };
        assert_eq!(
            match_fir(&store, body),
            FirMatch::Block(
                assembly
                    .loops
                    .iter()
                    .map(|loop_| loop_.iteration_statement)
                    .collect()
            )
        );

        let mut forged = assembly.clone();
        let separate = assembly
            .loops
            .iter()
            .map(|loop_| sample_loop(&mut FirBuilder::new(&mut store), loop_.iteration_statement))
            .collect::<Vec<_>>();
        forged.top_level_statement = FirBuilder::new(&mut store).block(&separate);
        assert_eq!(
            verify_vector_fir_assembly(&routed, None, None, &forged, &store),
            Err(VectorFirAssemblyError::LockstepBundleShape { bundle_id: 0 })
        );
    }

    fn state_vector_plan() -> super::super::vector_plan::VerifiedVectorPlan {
        verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size: 8,
            signals: vec![
                SignalRecord {
                    signal_id: 11,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Scal,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Owned(0),
                    duplicable: true,
                },
                SignalRecord {
                    signal_id: 12,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Scal,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Owned(0),
                    duplicable: true,
                },
            ],
            loops: vec![LoopRecord {
                loop_id: 0,
                stable_name: "recursive_1".to_owned(),
                kind: LoopKind::Recursive(1),
                roots: vec![11, 12],
                epoch_id: 0,
            }],
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0],
            }],
            transports: vec![],
            data_edges: vec![],
            effect_edges: vec![],
            vec_safe_witnesses: vec![],
            fused_serial_groups: vec![],
        })
    }

    fn state_plan(
        vector: &super::super::vector_plan::VerifiedVectorPlan,
    ) -> VerifiedVectorStatePlan {
        verified_vector_state_plan_for_test(
            VectorStatePlan {
                schema_version: VECTOR_STATE_PLAN_VERSION,
                vec_size: 8,
                max_copy_delay: 4,
                loops: vec![LoopStatePhases {
                    loop_id: 0,
                    pre: vec![
                        VectorStateAction::DelayCopyIn { signal_id: 11 },
                        VectorStateAction::DelayRingAdvance { signal_id: 12 },
                    ],
                    exec: vec![
                        VectorStateAction::RecursionStep { group: 1 },
                        VectorStateAction::DelayWrite { signal_id: 11 },
                        VectorStateAction::DelayWrite { signal_id: 12 },
                    ],
                    post: vec![
                        VectorStateAction::DelayCopyOut { signal_id: 11 },
                        VectorStateAction::DelayRingSaveAdvance { signal_id: 12 },
                    ],
                }],
                delays: vec![
                    DelayTransition {
                        signal_id: 11,
                        loop_id: 0,
                        clock_domain: None,
                        value_type: ValueType::Real,
                        max_delay: 3,
                        storage: VectorDelayStorage::Copy {
                            temporary_name: "fVec11_tmp".to_owned(),
                            permanent_name: "fVec11_perm".to_owned(),
                            history_length: 4,
                            temporary_length: 12,
                        },
                    },
                    DelayTransition {
                        signal_id: 12,
                        loop_id: 0,
                        clock_domain: None,
                        value_type: ValueType::Real,
                        max_delay: 5,
                        storage: VectorDelayStorage::Ring {
                            buffer_name: "fVec12".to_owned(),
                            index_name: "fVec12_idx".to_owned(),
                            index_save_name: "fVec12_idx_save".to_owned(),
                            capacity: 16,
                            mask: 15,
                        },
                    },
                ],
                recursions: vec![RecursionTransition {
                    group: 1,
                    loop_id: 0,
                    projections: vec![RecursionProjectionTransition {
                        index: 0,
                        signal_ids: vec![11],
                        value_signal_id: 11,
                    }],
                }],
                lockstep_register_bundles: vec![],
                prefixes: vec![],
                waveforms: vec![],
                no_op_resources: vec![],
            },
            vector,
        )
    }

    #[test]
    fn materializes_copy_ring_and_simultaneous_recursion_words() {
        let vector = state_vector_plan();
        let state = state_plan(&vector);
        let mut store = FirStore::new();
        let value11 = FirBuilder::new(&mut store).float32(0.25);
        let value12 = FirBuilder::new(&mut store).float32(0.5);
        let (mut route, _) = VectorRouteSession::new(
            &vector,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .expect("route");
        route
            .define_in_loop(0, 11, value11, &mut store)
            .expect("define 11");
        route
            .define_in_loop(0, 12, value12, &mut store)
            .expect("define 12");
        let routed = route.finish(&store).expect("finish route");
        let input = VectorLoopFirInput {
            loop_id: 0,
            statements: vec![],
        };
        let verified = assemble_vector_fir(
            &routed,
            Some(&state),
            None,
            &[input],
            &[],
            FirType::Float32,
            &mut store,
        )
        .expect("assemble state");
        let assembled = verified.assembly();
        assert_eq!(assembled.loops[0].pre.len(), 2);
        assert_eq!(assembled.loops[0].exec_actions.len(), 3);
        assert_eq!(assembled.loops[0].post.len(), 2);
        assert!(assembled.state_declarations.len() >= 4);
        assert!(matches!(
            match_fir(&store, assembled.loops[0].exec_actions[0].statement),
            FirMatch::Block(body) if body.len() == 1
        ));
        for action in [&assembled.loops[0].pre[0], &assembled.loops[0].post[0]] {
            let FirMatch::SimpleForLoop { body, .. } = match_fir(&store, action.statement) else {
                panic!("copy transition must materialize as a simple loop");
            };
            assert!(matches!(match_fir(&store, body), FirMatch::Block(words) if words.len() == 1));
        }
        assert!(assembled.clear_statements.iter().all(|statement| {
            let FirMatch::SimpleForLoop { body, .. } = match_fir(&store, *statement) else {
                return true;
            };
            matches!(match_fir(&store, body), FirMatch::Block(words) if words.len() == 1)
        }));

        let mut forged = assembled.clone();
        let FirMatch::SimpleForLoop {
            var,
            upper,
            body,
            is_reverse,
        } = match_fir(&store, forged.loops[0].pre[0].statement)
        else {
            panic!("copy-in loop");
        };
        let FirMatch::Block(words) = match_fir(&store, body) else {
            panic!("canonical copy-in body");
        };
        forged.loops[0].pre[0].statement =
            FirBuilder::new(&mut store).simple_for_loop(var, upper, words[0], is_reverse);
        assert!(matches!(
            verify_vector_fir_assembly(&routed, Some(&state), None, &forged, &store),
            Err(VectorFirAssemblyError::ActionShape {
                action: VectorStateAction::DelayCopyIn { signal_id: 11 },
                ..
            })
        ));

        let mut forged = assembled.clone();
        forged.loops[0].exec_actions[1].statement = FirBuilder::new(&mut store).int32(0);
        assert!(matches!(
            verify_vector_fir_assembly(&routed, Some(&state), None, &forged, &store),
            Err(VectorFirAssemblyError::ActionShape {
                action: VectorStateAction::DelayWrite { signal_id: 11 },
                ..
            })
        ));
    }

    fn clock_vector_plan() -> super::super::vector_plan::VerifiedVectorPlan {
        verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size: 8,
            signals: vec![
                SignalRecord {
                    signal_id: 1,
                    value_type: ValueType::Int,
                    structural: false,
                    rate: Rate::Block,
                    vectorability: Vectorability::Vect,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Control,
                    duplicable: true,
                },
                SignalRecord {
                    signal_id: 10,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Scal,
                    clock_id: 8,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Owned(0),
                    duplicable: true,
                },
                SignalRecord {
                    signal_id: 11,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Vect,
                    clock_id: 0,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Owned(1),
                    duplicable: true,
                },
                SignalRecord {
                    signal_id: 12,
                    value_type: ValueType::Real,
                    structural: false,
                    rate: Rate::Samp,
                    vectorability: Vectorability::Scal,
                    clock_id: 8,
                    effects: vec![],
                    direct_effects: vec![],
                    placement: Placement::Owned(2),
                    duplicable: true,
                },
            ],
            loops: vec![
                LoopRecord {
                    loop_id: 0,
                    stable_name: "island_7".to_owned(),
                    kind: LoopKind::Island(7),
                    roots: vec![10],
                    epoch_id: 0,
                },
                LoopRecord {
                    loop_id: 1,
                    stable_name: "outer".to_owned(),
                    kind: LoopKind::Vectorizable,
                    roots: vec![11],
                    epoch_id: 0,
                },
                LoopRecord {
                    loop_id: 2,
                    stable_name: "island_7_consumer".to_owned(),
                    kind: LoopKind::Island(7),
                    roots: vec![12],
                    epoch_id: 0,
                },
            ],
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0, 1, 2],
            }],
            transports: vec![
                TransportRecord {
                    transport_id: 0,
                    stable_name: "island_s10".to_owned(),
                    signal_id: 10,
                    producer_loop: 0,
                    consumer_loop: 2,
                    element_type: ValueType::Real,
                    length: 8,
                    layout: crate::signal_fir::vector_verify::TransportLayout::Planar,
                },
                TransportRecord {
                    transport_id: 1,
                    stable_name: "held_s12".to_owned(),
                    signal_id: 12,
                    producer_loop: 2,
                    consumer_loop: 1,
                    element_type: ValueType::Real,
                    length: 8,
                    layout: crate::signal_fir::vector_verify::TransportLayout::Planar,
                },
            ],
            data_edges: vec![
                LoopEdge {
                    consumer: 1,
                    dependency: 2,
                },
                LoopEdge {
                    consumer: 2,
                    dependency: 0,
                },
            ],
            effect_edges: vec![],
            vec_safe_witnesses: vec![VecSafeWitness {
                loop_id: 1,
                witness_kind: WitnessKind::Pointwise,
            }],
            fused_serial_groups: vec![FusedSerialGroupRecord {
                group_id: 0,
                owner_loop_id: 2,
                member_loop_ids: vec![0, 2],
                state_carrier_signal_ids: vec![12],
                delayed_read_signal_ids: vec![10],
                state_write_signal_ids: vec![12],
                internal_transport_ids: vec![0],
                output_or_transport_roots: vec![10, 12],
            }],
        })
    }

    #[test]
    fn nests_clock_loop_and_materializes_held_transport_lifetime() {
        let vector = clock_vector_plan();
        let clock = verified_vector_clock_ad_plan_for_test(
            VectorClockAdPlan {
                schema_version: VECTOR_CLOCK_AD_PLAN_VERSION,
                vec_size: 8,
                clock_islands: vec![ClockIsland {
                    domain_id: 7,
                    parent_domain: None,
                    kind: propagate::ClockDomainKind::OnDemand,
                    clock_signal_id: 1,
                    wrapper_signal_id: 10,
                    boundary_loop_id: 0,
                    guard: ClockGuard::CountedOnDemand,
                    signal_ids: vec![10],
                    clock_state_signal_ids: vec![],
                    nested_loop_ids: vec![0, 2],
                }],
                transports: vec![
                    super::super::vector_clock_ad::ClockTransportPolicy {
                        transport_id: 0,
                        mode: ClockTransportMode::FusedScalar { group_id: 0 },
                    },
                    super::super::vector_clock_ad::ClockTransportPolicy {
                        transport_id: 1,
                        mode: ClockTransportMode::HeldOutput { domain_id: 7 },
                    },
                ],
                forward_ad: ForwardAdPolicy::ExpandedSignalGraph,
                reverse_ad_fallbacks: vec![],
            },
            &vector,
        );
        let state = verified_vector_state_plan_for_test(
            VectorStatePlan {
                schema_version: VECTOR_STATE_PLAN_VERSION,
                vec_size: 8,
                max_copy_delay: 16,
                loops: vec![LoopStatePhases {
                    loop_id: 2,
                    pre: vec![],
                    exec: vec![VectorStateAction::DelayWrite { signal_id: 12 }],
                    post: vec![],
                }],
                delays: vec![DelayTransition {
                    signal_id: 12,
                    loop_id: 2,
                    value_type: ValueType::Real,
                    max_delay: 3,
                    clock_domain: Some(7),
                    storage: VectorDelayStorage::ClockRing {
                        buffer_name: "vstate_s12".to_owned(),
                        cursor_name: "vclock_d7_iota".to_owned(),
                        domain_id: 7,
                        capacity: 4,
                        mask: 3,
                    },
                }],
                recursions: vec![],
                lockstep_register_bundles: vec![],
                prefixes: vec![],
                waveforms: vec![],
                no_op_resources: vec![],
            },
            &vector,
        );
        let mut store = FirStore::new();
        let clock_value = FirBuilder::new(&mut store).int32(2);
        let value = FirBuilder::new(&mut store).float32(0.5);
        let (mut route, _) = VectorRouteSession::new_with_clock_plan(
            &vector,
            &clock,
            SchedulingStrategy::DepthFirst,
            FirType::Float32,
            &mut store,
        )
        .expect("clock route");
        route
            .define_control(1, clock_value, &store)
            .expect("clock definition");
        let island_stores = route
            .define_in_loop(0, 10, value, &mut store)
            .expect("island definition");
        let island_value = match route
            .resolve_in_loop(2, 10, &mut store)
            .expect("island scalar load")
        {
            RouteResolution::Value(value) => value,
            RouteResolution::NeedsInlineLowering => panic!("unexpected inline"),
        };
        let held_stores = route
            .define_in_loop(2, 12, island_value, &mut store)
            .expect("held definition");
        let loaded = match route.resolve_in_loop(1, 12, &mut store).expect("held load") {
            RouteResolution::Value(value) => value,
            RouteResolution::NeedsInlineLowering => panic!("unexpected inline"),
        };
        route
            .define_in_loop(1, 11, loaded, &mut store)
            .expect("outer definition");
        let routed = route.finish(&store).expect("finish route");
        let inputs = vec![
            VectorLoopFirInput {
                loop_id: 0,
                statements: island_stores,
            },
            VectorLoopFirInput {
                loop_id: 1,
                statements: vec![FirBuilder::new(&mut store).drop_(loaded)],
            },
            VectorLoopFirInput {
                loop_id: 2,
                statements: held_stores,
            },
        ];
        let clock_output = {
            let mut builder = FirBuilder::new(&mut store);
            let index = builder.load_var("i0", AccessType::Loop, FirType::Int32);
            VectorClockOutputStore {
                owner_loop_id: 2,
                statement: builder.store_table("output0", AccessType::Stack, index, loaded),
            }
        };
        let verified = assemble_vector_fir(
            &routed,
            Some(&state),
            Some(&clock),
            &inputs,
            std::slice::from_ref(&clock_output),
            FirType::Float32,
            &mut store,
        )
        .expect("assemble clock");
        let assembly = verified.assembly();
        assert_eq!(assembly.islands.len(), 1);
        assert!(assembly.islands[0].state_cursor_advance.is_some());
        assert!(assembly.islands[0].local_declarations.is_empty());
        assert_eq!(assembly.local_declarations.len(), 1);
        assert!(matches!(
            match_fir(&store, assembly.islands[0].statement),
            FirMatch::SimpleForLoop { .. }
        ));
        assert_eq!(assembly.state_declarations.len(), 3);
        assert_eq!(assembly.clear_statements.len(), 3);
        assert_eq!(assembly.clock_output_stores, [clock_output]);
        assert!(matches!(
            match_fir(&store, assembly.state_declarations[0]),
            FirMatch::DeclareVar {
                access: AccessType::Struct,
                ..
            }
        ));

        let mut forged = assembly.clone();
        forged.islands[0].statement = FirBuilder::new(&mut store).int32(0);
        assert!(matches!(
            verify_vector_fir_assembly(&routed, Some(&state), Some(&clock), &forged, &store),
            Err(VectorFirAssemblyError::IslandShape { domain_id: 7 })
        ));

        let mut forged = assembly.clone();
        let mut second_island = forged.islands[0].clone();
        second_island.nested_loop_ids = vec![2];
        forged.islands.push(second_island);
        assert!(matches!(
            verify_assembled_fused_serial_groups(&routed, Some(&state), &forged, &store),
            Err(VectorFirAssemblyError::FusedGroupShape { group_id: 0 })
        ));

        let mut forged = assembly.clone();
        forged.clock_output_stores[0].owner_loop_id = 99;
        assert!(matches!(
            verify_vector_fir_assembly(&routed, Some(&state), Some(&clock), &forged, &store),
            Err(VectorFirAssemblyError::ClockLoopOwnership { loop_id: 99 })
        ));

        let mut forged = assembly.clone();
        forged.islands[0].state_cursor_advance = None;
        assert_eq!(
            verify_vector_fir_assembly(&routed, Some(&state), Some(&clock), &forged, &store),
            Err(VectorFirAssemblyError::IslandShape { domain_id: 7 })
        );
    }
}
