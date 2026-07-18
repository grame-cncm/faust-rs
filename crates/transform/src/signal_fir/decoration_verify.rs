//! Canonical P4.3b signal-decoration certificate and independent verifier.
//!
//! # Trust boundary
//! [`export_decoration_certificate`] projects the P4.3a in-memory analysis into
//! a stable, planner-independent DTO. [`verify_decorations`] does not trust
//! that DTO: it reruns the canonical analysis from the verified prepared
//! forest, reads authoritative type and clock maps directly, and checks every
//! exported fact before vector placement can consume it.
//!
//! This certificate is deliberately compute-scoped. C++ `OccMarkup` and the
//! Rust P4.3a walk both stop at `Gen`; [`DecorationCertificate::lifecycle_boundaries`]
//! records those leaves explicitly. A full-lifecycle certificate remains
//! unavailable until generator initialization receives its own decoration.
//!
//! # C++ provenance and adaptation
//! Facts come from `compiler/generator/occurrences.cpp::OccMarkup`,
//! `compiler/signals/conditionAnnotation.cpp`, and
//! `compiler/Dependencies/DependenciesUtils.cpp::getSignalDependencies`.
//! Rust keeps the certificate as an internal value for P4.3b; canonical JSON
//! and hashing are deferred to the R2 boundary. [`CanonicalSigType`] uses exact
//! field snapshots because `SigType::PartialEq` intentionally ignores fixed
//! precision and aggregate qualifiers to reproduce C++ inference convergence.

use std::fmt;

use signals::{SigId, SigMatch, match_sig};
use sigtype::{Boolean, Computability, Nature, Res, SigType, Variability, Vectorability};
use tlib::TreeArena;

use crate::clk_env::ClkEnvMap;
use crate::signal_prepare::VerifiedPreparedSignals;

use super::vector_analysis::{
    AnalysisError, CondId, DepKind, EffectAtom, OccInfo, RecursiveProjection, SignalUseInfo,
    UseContext, VectorSignalAnalysis, analyze_vector_signals,
};

/// Current in-memory decoration-certificate schema.
pub const DECORATION_CERTIFICATE_VERSION: u32 = 2;

/// Semantic coverage claimed by a certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationScope {
    /// Runtime compute graph, treating `Gen` as a lifecycle boundary.
    Compute,
    /// Initialization and compute. Not produced or accepted in P4.3b.
    FullLifecycle,
}

/// Exact floating interval snapshot, including precision and IEEE bit pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanonicalInterval {
    pub lo_bits: u64,
    pub hi_bits: u64,
    pub lsb: i32,
}

/// Exact boundary representation of `SigType`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonicalSigType {
    /// Soundfile handle payload, derived from the signal shape exactly as the
    /// prepared boundary derives `SimpleSigType::Sound`: the sig-type nature
    /// says nothing about the handle.
    Sound,
    Simple {
        nature: Nature,
        variability: Variability,
        computability: Computability,
        vectorability: Vectorability,
        boolean: Boolean,
        interval: CanonicalInterval,
        resolution: Res,
    },
    Table {
        content: Box<CanonicalSigType>,
        nature: Nature,
        variability: Variability,
        computability: Computability,
        vectorability: Vectorability,
        boolean: Boolean,
        interval: CanonicalInterval,
    },
    Tuplet {
        components: Vec<CanonicalSigType>,
        nature: Nature,
        variability: Variability,
        computability: Computability,
        vectorability: Vectorability,
        boolean: Boolean,
        interval: CanonicalInterval,
    },
}

/// Canonical condition table entry. `condition_id` is the table index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionFact {
    pub condition_id: u64,
    pub clauses: Vec<Vec<u32>>,
}

/// Stable recursive-projection boundary fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecursiveProjectionFact {
    pub index: u64,
    pub group: u32,
}

/// One complete compute-time decoration record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecorationRecord {
    pub signal_id: u32,
    pub sig_type: CanonicalSigType,
    pub variability: Variability,
    pub vectorability: Vectorability,
    pub clock_domain: Option<u32>,
    pub recursiveness: u32,
    pub execution_condition: CondId,
    pub occurrences: OccInfo,
    pub max_delay: u32,
    pub delay_reads: u32,
    pub has_out_delay_occurrence: bool,
    pub is_delay_read: bool,
    pub is_symbolic_recursion_carrier: bool,
    pub recursive_projection: Option<RecursiveProjectionFact>,
    pub very_simple: bool,
    pub effects: Vec<EffectAtom>,
    /// Effects this signal performs itself, always a sorted subset of
    /// `effects`. Consumers that model actual effect operations must read this
    /// rather than the transitive set; consumers that ask whether a subtree is
    /// free of effects, such as duplicability, must keep reading `effects`.
    pub direct_effects: Vec<EffectAtom>,
}

/// Labelled scheduling dependency with source-local edge identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DependencyFact {
    pub from: u32,
    pub to: u32,
    pub kind: DepKind,
    pub edge_key: u64,
}

/// Labelled occurrence dependency. Delay is kept separate from scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OccurrenceDependencyFact {
    pub from: u32,
    pub to: u32,
    pub delay: u32,
    pub edge_key: u64,
}

/// Canonical projection of the real P4.3a prepared-signal analysis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecorationCertificate {
    pub schema_version: u32,
    pub scope: DecorationScope,
    /// Prepared roots in semantic output order.
    pub roots: Vec<u32>,
    /// Strictly increasing compute-visible `Gen` identities.
    pub lifecycle_boundaries: Vec<u32>,
    /// Conditions in contiguous `CondId` order.
    pub conditions: Vec<ConditionFact>,
    /// One record per compute-reachable signal, strictly increasing by id.
    pub records: Vec<DecorationRecord>,
    /// Scheduling edges ordered by `(from, edge_key)`.
    pub dependencies: Vec<DependencyFact>,
    /// Occurrence edges ordered by `(from, edge_key)`.
    pub occurrence_dependencies: Vec<OccurrenceDependencyFact>,
}

/// Opaque evidence that a certificate was freshly checked against its forest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedDecorationCertificate {
    certificate: DecorationCertificate,
}

impl VerifiedDecorationCertificate {
    /// Returns the accepted canonical certificate.
    #[must_use]
    pub fn certificate(&self) -> &DecorationCertificate {
        &self.certificate
    }

    /// Consumes the proof wrapper and returns its certificate.
    #[must_use]
    pub fn into_certificate(self) -> DecorationCertificate {
        self.certificate
    }
}

/// A record field checked independently against authoritative analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationField {
    Variability,
    Vectorability,
    Recursiveness,
    ExecutionCondition,
    Occurrences,
    DelayFacts,
    SymbolicRecursionCarrier,
    RecursiveProjection,
    VerySimple,
    Effects,
    DirectEffects,
}

/// Why [`verify_decorations`] rejected a certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecorationError {
    Analysis(AnalysisError),
    UnsupportedSchema {
        found: u32,
    },
    UnsupportedScope {
        found: DecorationScope,
    },
    RootsMismatch,
    NotCanonical {
        what: &'static str,
        at: usize,
    },
    SignalCoverageMismatch,
    LifecycleBoundariesMismatch,
    ConditionTableMismatch,
    UnknownCondition {
        signal_id: u32,
        condition_id: u64,
    },
    TypeMismatch {
        signal_id: u32,
    },
    ClockMismatch {
        signal_id: u32,
    },
    SignalFactMismatch {
        signal_id: u32,
        field: DecorationField,
    },
    DependencyEndpointUnknown {
        from: u32,
        to: u32,
    },
    DependenciesMismatch,
    OccurrenceDependencyEndpointUnknown {
        from: u32,
        to: u32,
    },
    OccurrenceDependenciesMismatch,
}

impl fmt::Display for DecorationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Analysis(error) => write!(f, "decoration analysis failed: {error}"),
            Self::UnsupportedSchema { found } => {
                write!(f, "unsupported decoration certificate schema {found}")
            }
            Self::UnsupportedScope { found } => {
                write!(f, "unsupported decoration certificate scope {found:?}")
            }
            Self::RootsMismatch => write!(f, "decoration roots do not match prepared outputs"),
            Self::NotCanonical { what, at } => {
                write!(f, "{what} is not canonical at index {at}")
            }
            Self::SignalCoverageMismatch => {
                write!(
                    f,
                    "decoration records do not exactly cover compute-reachable signals"
                )
            }
            Self::LifecycleBoundariesMismatch => {
                write!(
                    f,
                    "generator lifecycle boundaries do not match the prepared forest"
                )
            }
            Self::ConditionTableMismatch => write!(f, "execution-condition table mismatch"),
            Self::UnknownCondition {
                signal_id,
                condition_id,
            } => write!(
                f,
                "signal {signal_id} references unknown condition {condition_id}"
            ),
            Self::TypeMismatch { signal_id } => {
                write!(f, "canonical type mismatch for signal {signal_id}")
            }
            Self::ClockMismatch { signal_id } => {
                write!(f, "clock-domain mismatch for signal {signal_id}")
            }
            Self::SignalFactMismatch { signal_id, field } => {
                write!(f, "{field:?} mismatch for signal {signal_id}")
            }
            Self::DependencyEndpointUnknown { from, to } => {
                write!(f, "dependency {from} -> {to} has an unknown endpoint")
            }
            Self::DependenciesMismatch => write!(f, "scheduling dependencies mismatch"),
            Self::OccurrenceDependencyEndpointUnknown { from, to } => {
                write!(
                    f,
                    "occurrence dependency {from} -> {to} has an unknown endpoint"
                )
            }
            Self::OccurrenceDependenciesMismatch => {
                write!(f, "occurrence dependencies mismatch")
            }
        }
    }
}

impl std::error::Error for DecorationError {}

impl From<AnalysisError> for DecorationError {
    fn from(value: AnalysisError) -> Self {
        Self::Analysis(value)
    }
}

fn canonical_interval(lo: f64, hi: f64, lsb: i32) -> CanonicalInterval {
    CanonicalInterval {
        lo_bits: lo.to_bits(),
        hi_bits: hi.to_bits(),
        lsb,
    }
}

/// Canonical type of one signal, shape-aware for soundfile handles.
fn canonical_sig_type_of(arena: &TreeArena, sig: SigId, sig_type: &SigType) -> CanonicalSigType {
    if matches!(match_sig(arena, sig), SigMatch::Soundfile(_)) {
        return CanonicalSigType::Sound;
    }
    canonical_sig_type(sig_type)
}

fn canonical_sig_type(sig_type: &SigType) -> CanonicalSigType {
    match sig_type {
        SigType::Simple(ty) => CanonicalSigType::Simple {
            nature: ty.nature,
            variability: ty.variability,
            computability: ty.computability,
            vectorability: ty.vectorability,
            boolean: ty.boolean,
            interval: canonical_interval(ty.interval.lo(), ty.interval.hi(), ty.interval.lsb()),
            resolution: ty.res,
        },
        SigType::Table(ty) => CanonicalSigType::Table {
            content: Box::new(canonical_sig_type(&ty.content)),
            nature: ty.nature,
            variability: ty.variability,
            computability: ty.computability,
            vectorability: ty.vectorability,
            boolean: ty.boolean,
            interval: canonical_interval(ty.interval.lo(), ty.interval.hi(), ty.interval.lsb()),
        },
        SigType::Tuplet(ty) => CanonicalSigType::Tuplet {
            components: ty.components.iter().map(canonical_sig_type).collect(),
            nature: ty.nature,
            variability: ty.variability,
            computability: ty.computability,
            vectorability: ty.vectorability,
            boolean: ty.boolean,
            interval: canonical_interval(ty.interval.lo(), ty.interval.hi(), ty.interval.lsb()),
        },
    }
}

fn projection_fact(projection: Option<RecursiveProjection>) -> Option<RecursiveProjectionFact> {
    projection.map(|projection| RecursiveProjectionFact {
        index: u64::try_from(projection.index).expect("projection index fits certificate"),
        group: projection.group.as_u32(),
    })
}

fn decoration_record(arena: &TreeArena, sig: SigId, info: &SignalUseInfo) -> DecorationRecord {
    DecorationRecord {
        signal_id: sig.as_u32(),
        sig_type: canonical_sig_type_of(arena, sig, &info.sig_type),
        variability: info.variability,
        vectorability: info.vectorability,
        clock_domain: info.clk_env.map(|domain| domain.as_u32()),
        recursiveness: info.recursiveness,
        execution_condition: info.execution_condition,
        occurrences: info.occurrences.clone(),
        max_delay: info.max_delay,
        delay_reads: info.delay_reads,
        has_out_delay_occurrence: info.has_out_delay_occurrence,
        is_delay_read: info.is_delay_read,
        is_symbolic_recursion_carrier: info.is_symbolic_recursion_carrier,
        recursive_projection: projection_fact(info.recursive_projection),
        very_simple: info.very_simple,
        effects: info.effects.clone(),
        direct_effects: info.direct_effects.clone(),
    }
}

/// Exports a canonical certificate from the real P4.3a analysis.
#[must_use]
pub fn export_decoration_certificate(
    prepared: &VerifiedPreparedSignals,
    analysis: &VectorSignalAnalysis,
) -> DecorationCertificate {
    let lifecycle_boundaries = analysis
        .uses
        .records()
        .iter()
        .filter(|record| matches!(match_sig(prepared.arena(), record.sig), SigMatch::Gen(_)))
        .map(|record| record.sig.as_u32())
        .collect();
    let records = analysis
        .uses
        .records()
        .iter()
        .map(|record| decoration_record(prepared.arena(), record.sig, &record.info))
        .collect::<Vec<_>>();
    DecorationCertificate {
        schema_version: DECORATION_CERTIFICATE_VERSION,
        scope: DecorationScope::Compute,
        roots: prepared.outputs().iter().map(|sig| sig.as_u32()).collect(),
        lifecycle_boundaries,
        conditions: analysis
            .conditions
            .conditions()
            .iter()
            .enumerate()
            .map(|(condition_id, condition)| ConditionFact {
                condition_id: u64::try_from(condition_id).expect("condition index fits u64"),
                clauses: condition.clauses().to_vec(),
            })
            .collect(),
        records,
        dependencies: analysis
            .uses
            .dependencies()
            .iter()
            .map(|dependency| DependencyFact {
                from: dependency.from.as_u32(),
                to: dependency.to.as_u32(),
                kind: dependency.kind,
                edge_key: u64::try_from(dependency.edge_key).expect("edge key fits u64"),
            })
            .collect(),
        occurrence_dependencies: analysis
            .uses
            .occurrence_dependencies()
            .iter()
            .map(|dependency| OccurrenceDependencyFact {
                from: dependency.from.as_u32(),
                to: dependency.to.as_u32(),
                delay: dependency.delay,
                edge_key: u64::try_from(dependency.edge_key).expect("edge key fits u64"),
            })
            .collect(),
    }
}

fn check_strict_order<T: Ord>(values: &[T], what: &'static str) -> Result<(), DecorationError> {
    if let Some(at) = values.windows(2).position(|pair| pair[0] >= pair[1]) {
        return Err(DecorationError::NotCanonical { what, at: at + 1 });
    }
    Ok(())
}

fn verify_canonical_shape(certificate: &DecorationCertificate) -> Result<(), DecorationError> {
    check_strict_order(
        &certificate
            .records
            .iter()
            .map(|record| record.signal_id)
            .collect::<Vec<_>>(),
        "decoration records",
    )?;
    check_strict_order(&certificate.lifecycle_boundaries, "lifecycle boundaries")?;
    for (index, condition) in certificate.conditions.iter().enumerate() {
        if condition.condition_id != u64::try_from(index).expect("condition index fits u64") {
            return Err(DecorationError::NotCanonical {
                what: "condition ids",
                at: index,
            });
        }
        for (clause_index, clause) in condition.clauses.iter().enumerate() {
            if clause.is_empty() {
                return Err(DecorationError::NotCanonical {
                    what: "condition clauses",
                    at: clause_index,
                });
            }
            check_strict_order(clause, "condition atoms")?;
        }
        check_strict_order(&condition.clauses, "condition clauses")?;
    }
    for record in &certificate.records {
        check_strict_order(&record.effects, "effect atoms")?;
        check_strict_order(&record.direct_effects, "direct effect atoms")?;
        // A signal cannot perform an effect its own transitive closure omits.
        // The relation is cheap and total, and holds whatever produced either
        // set, so both checkers can assert it without sharing derivation state.
        if let Some(at) = record
            .direct_effects
            .iter()
            .position(|effect| record.effects.binary_search(effect).is_err())
        {
            return Err(DecorationError::NotCanonical {
                what: "direct effect atom absent from the transitive effects",
                at,
            });
        }
        check_strict_order(
            &record
                .occurrences
                .per_context
                .iter()
                .map(|occurrence| occurrence.context)
                .collect::<Vec<UseContext>>(),
            "occurrence contexts",
        )?;
        if let Some((at, _)) = record
            .occurrences
            .per_context
            .iter()
            .enumerate()
            .find(|(_, occurrence)| occurrence.count == 0)
        {
            return Err(DecorationError::NotCanonical {
                what: "occurrence counts",
                at,
            });
        }
    }
    check_strict_order(
        &certificate
            .dependencies
            .iter()
            .map(|edge| (edge.from, edge.edge_key))
            .collect::<Vec<_>>(),
        "scheduling dependencies",
    )?;
    check_strict_order(
        &certificate
            .occurrence_dependencies
            .iter()
            .map(|edge| (edge.from, edge.edge_key))
            .collect::<Vec<_>>(),
        "occurrence dependencies",
    )
}

fn mismatch(signal_id: u32, field: DecorationField) -> Result<(), DecorationError> {
    Err(DecorationError::SignalFactMismatch { signal_id, field })
}

fn verify_record(
    prepared: &VerifiedPreparedSignals,
    clocks: &ClkEnvMap,
    sig: signals::SigId,
    actual: &DecorationRecord,
    expected: &SignalUseInfo,
    condition_count: usize,
) -> Result<(), DecorationError> {
    let Some(authoritative_type) = prepared.sig_types_map().get(&sig) else {
        return Err(DecorationError::SignalCoverageMismatch);
    };
    if actual.sig_type != canonical_sig_type_of(prepared.arena(), sig, authoritative_type) {
        return Err(DecorationError::TypeMismatch {
            signal_id: actual.signal_id,
        });
    }
    let authoritative_clock = clocks.env(sig).ok_or(DecorationError::ClockMismatch {
        signal_id: actual.signal_id,
    })?;
    if actual.clock_domain != authoritative_clock.map(|domain| domain.as_u32()) {
        return Err(DecorationError::ClockMismatch {
            signal_id: actual.signal_id,
        });
    }
    if usize::try_from(actual.execution_condition.0)
        .ok()
        .filter(|index| *index < condition_count)
        .is_none()
    {
        return Err(DecorationError::UnknownCondition {
            signal_id: actual.signal_id,
            condition_id: actual.execution_condition.0,
        });
    }
    if let Some(condition_id) = actual
        .occurrences
        .per_context
        .iter()
        .map(|occurrence| occurrence.context.condition.0)
        .find(|condition_id| {
            usize::try_from(*condition_id)
                .ok()
                .filter(|index| *index < condition_count)
                .is_none()
        })
    {
        return Err(DecorationError::UnknownCondition {
            signal_id: actual.signal_id,
            condition_id,
        });
    }
    if actual.variability != expected.variability {
        return mismatch(actual.signal_id, DecorationField::Variability);
    }
    if actual.vectorability != expected.vectorability {
        return mismatch(actual.signal_id, DecorationField::Vectorability);
    }
    if actual.recursiveness != expected.recursiveness {
        return mismatch(actual.signal_id, DecorationField::Recursiveness);
    }
    if actual.execution_condition != expected.execution_condition {
        return mismatch(actual.signal_id, DecorationField::ExecutionCondition);
    }
    if actual.occurrences != expected.occurrences {
        return mismatch(actual.signal_id, DecorationField::Occurrences);
    }
    if (
        actual.max_delay,
        actual.delay_reads,
        actual.has_out_delay_occurrence,
        actual.is_delay_read,
    ) != (
        expected.max_delay,
        expected.delay_reads,
        expected.has_out_delay_occurrence,
        expected.is_delay_read,
    ) {
        return mismatch(actual.signal_id, DecorationField::DelayFacts);
    }
    if actual.recursive_projection != projection_fact(expected.recursive_projection) {
        return mismatch(actual.signal_id, DecorationField::RecursiveProjection);
    }
    if actual.is_symbolic_recursion_carrier != expected.is_symbolic_recursion_carrier {
        return mismatch(actual.signal_id, DecorationField::SymbolicRecursionCarrier);
    }
    if actual.very_simple != expected.very_simple {
        return mismatch(actual.signal_id, DecorationField::VerySimple);
    }
    if actual.effects != expected.effects {
        return mismatch(actual.signal_id, DecorationField::Effects);
    }
    if actual.direct_effects != expected.direct_effects {
        return mismatch(actual.signal_id, DecorationField::DirectEffects);
    }
    Ok(())
}

/// Recomputes and verifies every P4.3b fact before vector planning.
pub fn verify_decorations(
    prepared: &VerifiedPreparedSignals,
    clocks: &ClkEnvMap,
    certificate: &DecorationCertificate,
) -> Result<VerifiedDecorationCertificate, DecorationError> {
    if certificate.schema_version != DECORATION_CERTIFICATE_VERSION {
        return Err(DecorationError::UnsupportedSchema {
            found: certificate.schema_version,
        });
    }
    if certificate.scope != DecorationScope::Compute {
        return Err(DecorationError::UnsupportedScope {
            found: certificate.scope,
        });
    }
    let roots = prepared
        .outputs()
        .iter()
        .map(|sig| sig.as_u32())
        .collect::<Vec<_>>();
    if certificate.roots != roots {
        return Err(DecorationError::RootsMismatch);
    }
    verify_canonical_shape(certificate)?;

    // This is a fresh derivation from the prepared forest, not a validation of
    // producer-owned caches or a VectorPlan placement result.
    let expected = analyze_vector_signals(prepared, clocks)?;
    let expected_ids = expected
        .uses
        .records()
        .iter()
        .map(|record| record.sig.as_u32())
        .collect::<Vec<_>>();
    let actual_ids = certificate
        .records
        .iter()
        .map(|record| record.signal_id)
        .collect::<Vec<_>>();
    if actual_ids != expected_ids {
        return Err(DecorationError::SignalCoverageMismatch);
    }

    let expected_conditions = expected
        .conditions
        .conditions()
        .iter()
        .enumerate()
        .map(|(condition_id, condition)| ConditionFact {
            condition_id: u64::try_from(condition_id).expect("condition index fits u64"),
            clauses: condition.clauses().to_vec(),
        })
        .collect::<Vec<_>>();
    if certificate.conditions != expected_conditions {
        return Err(DecorationError::ConditionTableMismatch);
    }

    for (actual, expected_record) in certificate.records.iter().zip(expected.uses.records()) {
        verify_record(
            prepared,
            clocks,
            expected_record.sig,
            actual,
            &expected_record.info,
            certificate.conditions.len(),
        )?;
    }

    let expected_boundaries = expected
        .uses
        .records()
        .iter()
        .filter(|record| matches!(match_sig(prepared.arena(), record.sig), SigMatch::Gen(_)))
        .map(|record| record.sig.as_u32())
        .collect::<Vec<_>>();
    if certificate.lifecycle_boundaries != expected_boundaries {
        return Err(DecorationError::LifecycleBoundariesMismatch);
    }

    for edge in &certificate.dependencies {
        if actual_ids.binary_search(&edge.from).is_err()
            || actual_ids.binary_search(&edge.to).is_err()
        {
            return Err(DecorationError::DependencyEndpointUnknown {
                from: edge.from,
                to: edge.to,
            });
        }
    }
    let expected_dependencies = expected
        .uses
        .dependencies()
        .iter()
        .map(|edge| DependencyFact {
            from: edge.from.as_u32(),
            to: edge.to.as_u32(),
            kind: edge.kind,
            edge_key: u64::try_from(edge.edge_key).expect("edge key fits u64"),
        })
        .collect::<Vec<_>>();
    if certificate.dependencies != expected_dependencies {
        return Err(DecorationError::DependenciesMismatch);
    }

    for edge in &certificate.occurrence_dependencies {
        if actual_ids.binary_search(&edge.from).is_err()
            || actual_ids.binary_search(&edge.to).is_err()
        {
            return Err(DecorationError::OccurrenceDependencyEndpointUnknown {
                from: edge.from,
                to: edge.to,
            });
        }
    }
    let expected_occurrences = expected
        .uses
        .occurrence_dependencies()
        .iter()
        .map(|edge| OccurrenceDependencyFact {
            from: edge.from.as_u32(),
            to: edge.to.as_u32(),
            delay: edge.delay,
            edge_key: u64::try_from(edge.edge_key).expect("edge key fits u64"),
        })
        .collect::<Vec<_>>();
    if certificate.occurrence_dependencies != expected_occurrences {
        return Err(DecorationError::OccurrenceDependenciesMismatch);
    }
    Ok(VerifiedDecorationCertificate {
        certificate: certificate.clone(),
    })
}

/// Runs P4.3a, exports its DTO, and returns only independently accepted facts.
pub fn certify_decorations(
    prepared: &VerifiedPreparedSignals,
    clocks: &ClkEnvMap,
) -> Result<VerifiedDecorationCertificate, DecorationError> {
    let analysis = analyze_vector_signals(prepared, clocks)?;
    let certificate = export_decoration_certificate(prepared, &analysis);
    verify_decorations(prepared, clocks, &certificate)
}

#[cfg(test)]
mod tests {
    use propagate::ClockDomainTable;
    use signals::SigBuilder;
    use tlib::TreeArena;

    use super::*;
    use crate::clk_env::annotate;
    use crate::signal_prepare::prepare_signals_for_fir_verified;

    fn fixture() -> (VerifiedPreparedSignals, ClkEnvMap, DecorationCertificate) {
        let mut arena = TreeArena::new();
        let roots = {
            let mut builder = SigBuilder::new(&mut arena);
            let input = builder.input(0);
            let amount = builder.int(2);
            let delayed = builder.delay(input, amount);
            let output = builder.output(0, delayed);
            let generated = builder.generate(input);
            vec![output, generated]
        };
        let prepared =
            prepare_signals_for_fir_verified(&arena, &roots, &ui::UiProgram::empty()).unwrap();
        let clocks = annotate(
            prepared.arena(),
            &ClockDomainTable::new(),
            prepared.outputs(),
        )
        .unwrap();
        let analysis = analyze_vector_signals(&prepared, &clocks).unwrap();
        let certificate = export_decoration_certificate(&prepared, &analysis);
        (prepared, clocks, certificate)
    }

    #[test]
    fn canonical_certificate_is_accepted_and_marks_generator_boundary() {
        let (prepared, clocks, certificate) = fixture();
        assert_eq!(certificate.lifecycle_boundaries.len(), 1);
        let verified = verify_decorations(&prepared, &clocks, &certificate).unwrap();
        assert_eq!(verified.certificate(), &certificate);
        assert_eq!(
            certify_decorations(&prepared, &clocks)
                .unwrap()
                .into_certificate(),
            certificate
        );
    }

    #[test]
    fn schema_scope_roots_and_coverage_mutations_are_rejected() {
        let (prepared, clocks, certificate) = fixture();

        let mut mutated = certificate.clone();
        mutated.schema_version += 1;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::UnsupportedSchema { .. })
        ));

        let mut mutated = certificate.clone();
        mutated.scope = DecorationScope::FullLifecycle;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::UnsupportedScope { .. })
        ));

        let mut mutated = certificate.clone();
        mutated.roots.reverse();
        assert_eq!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::RootsMismatch)
        );

        let mut mutated = certificate.clone();
        mutated.records.pop();
        assert_eq!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalCoverageMismatch)
        );
    }

    #[test]
    fn noncanonical_records_conditions_occurrences_and_effects_are_rejected() {
        let (prepared, clocks, certificate) = fixture();

        let mut mutated = certificate.clone();
        mutated.records.swap(0, 1);
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::NotCanonical {
                what: "decoration records",
                ..
            })
        ));

        let mut mutated = certificate.clone();
        mutated.conditions[0].condition_id = 9;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::NotCanonical {
                what: "condition ids",
                ..
            })
        ));

        let occurrence_record = certificate
            .records
            .iter()
            .position(|record| record.occurrences.per_context.len() == 1)
            .unwrap();
        let mut mutated = certificate.clone();
        mutated.records[occurrence_record].occurrences.per_context[0].count = 0;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::NotCanonical {
                what: "occurrence counts",
                ..
            })
        ));

        // A signal cannot perform an effect its own transitive closure omits.
        let direct_record = certificate
            .records
            .iter()
            .position(|record| !record.direct_effects.is_empty())
            .unwrap();
        let mut mutated = certificate.clone();
        mutated.records[direct_record].direct_effects = vec![EffectAtom::WriteOutput(31)];
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::NotCanonical {
                what: "direct effect atom absent from the transitive effects",
                ..
            })
        ));

        let mut mutated = certificate.clone();
        let direct_duplicate = mutated.records[direct_record].direct_effects[0].clone();
        mutated.records[direct_record]
            .direct_effects
            .insert(0, direct_duplicate);
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::NotCanonical {
                what: "direct effect atoms",
                ..
            })
        ));

        let effect_record = certificate
            .records
            .iter()
            .position(|record| !record.effects.is_empty())
            .unwrap();
        let mut mutated = certificate.clone();
        let duplicate = mutated.records[effect_record].effects[0].clone();
        mutated.records[effect_record].effects.insert(0, duplicate);
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::NotCanonical {
                what: "effect atoms",
                ..
            })
        ));
    }

    #[test]
    fn type_clock_condition_and_lifecycle_mutations_are_rejected() {
        let (prepared, clocks, certificate) = fixture();

        let mut mutated = certificate.clone();
        if let CanonicalSigType::Simple { resolution, .. } = &mut mutated.records[0].sig_type {
            resolution.index = resolution.index.saturating_add(1);
            resolution.valid = !resolution.valid;
        }
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::TypeMismatch { .. })
        ));

        let mut mutated = certificate.clone();
        mutated.records[0].clock_domain = Some(u32::MAX);
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::ClockMismatch { .. })
        ));

        let mut mutated = certificate.clone();
        mutated.conditions[0].clauses = vec![vec![certificate.records[0].signal_id]];
        assert_eq!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::ConditionTableMismatch)
        );

        let mut mutated = certificate.clone();
        mutated.lifecycle_boundaries.clear();
        assert_eq!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::LifecycleBoundariesMismatch)
        );
    }

    #[test]
    fn occurrence_delay_and_effect_mutations_are_rejected() {
        let (prepared, clocks, certificate) = fixture();
        let delayed_use = certificate
            .records
            .iter()
            .position(|record| record.max_delay > 0)
            .unwrap();

        let mut mutated = certificate.clone();
        mutated.records[delayed_use].occurrences.multi =
            !mutated.records[delayed_use].occurrences.multi;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::Occurrences,
                ..
            })
        ));

        let mut mutated = certificate.clone();
        mutated.records[delayed_use].max_delay += 1;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::DelayFacts,
                ..
            })
        ));

        let effect_record = certificate
            .records
            .iter()
            .position(|record| !record.effects.is_empty())
            .unwrap();
        let mut mutated = certificate.clone();
        // Drop the atom from both projections so the direct-subset invariant
        // still holds and the fact comparison is what rejects the certificate;
        // dropping it from the transitive set alone is covered separately.
        let dropped = mutated.records[effect_record].effects.pop().unwrap();
        mutated.records[effect_record]
            .direct_effects
            .retain(|effect| *effect != dropped);
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::Effects,
                ..
            })
        ));
    }

    #[test]
    fn cached_rate_vectorability_and_shape_mutations_are_rejected() {
        let (prepared, clocks, certificate) = fixture();

        let mut mutated = certificate.clone();
        mutated.records[0].variability = match mutated.records[0].variability {
            Variability::Konst => Variability::Block,
            Variability::Block | Variability::Samp => Variability::Konst,
        };
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::Variability,
                ..
            })
        ));

        let mut mutated = certificate.clone();
        mutated.records[0].vectorability = match mutated.records[0].vectorability {
            Vectorability::Vect => Vectorability::Scal,
            Vectorability::Scal | Vectorability::TrueScal => Vectorability::Vect,
        };
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::Vectorability,
                ..
            })
        ));

        let mut mutated = certificate.clone();
        mutated.records[0].recursiveness += 1;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::Recursiveness,
                ..
            })
        ));

        let mut mutated = certificate.clone();
        mutated.records[0].execution_condition = CondId(u64::MAX);
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::UnknownCondition { .. })
        ));

        let mut mutated = certificate;
        mutated.records[0].very_simple = !mutated.records[0].very_simple;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::VerySimple,
                ..
            })
        ));
    }

    #[test]
    fn recursive_projection_identity_mutation_is_rejected() {
        let mut arena = TreeArena::new();
        let self_ref = tlib::de_bruijn_ref(&mut arena, 1);
        let body = {
            let mut builder = SigBuilder::new(&mut arena);
            let feedback = builder.proj(7, self_ref);
            builder.delay1(feedback)
        };
        let nil = arena.nil();
        let body_list = arena.cons(body, nil);
        let group = tlib::de_bruijn_rec(&mut arena, body_list);
        let output = {
            let mut builder = SigBuilder::new(&mut arena);
            builder.proj(7, group)
        };
        let prepared =
            prepare_signals_for_fir_verified(&arena, &[output], &ui::UiProgram::empty()).unwrap();
        let clocks = annotate(
            prepared.arena(),
            &ClockDomainTable::new(),
            prepared.outputs(),
        )
        .unwrap();
        let analysis = analyze_vector_signals(&prepared, &clocks).unwrap();
        let certificate = export_decoration_certificate(&prepared, &analysis);
        let projection = certificate
            .records
            .iter()
            .position(|record| record.recursive_projection.is_some())
            .unwrap();

        let mut mutated = certificate.clone();
        mutated.records[projection]
            .recursive_projection
            .as_mut()
            .unwrap()
            .index += 1;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::RecursiveProjection,
                ..
            })
        ));

        let carrier = certificate
            .records
            .iter()
            .position(|record| record.is_symbolic_recursion_carrier)
            .unwrap();
        let mut mutated = certificate;
        mutated.records[carrier].is_symbolic_recursion_carrier = false;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::SignalFactMismatch {
                field: DecorationField::SymbolicRecursionCarrier,
                ..
            })
        ));
    }

    #[test]
    fn dependency_mutations_and_unknown_endpoints_are_rejected() {
        let (prepared, clocks, certificate) = fixture();

        let mut mutated = certificate.clone();
        mutated.dependencies[0].kind = DepKind::Effect;
        assert_eq!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::DependenciesMismatch)
        );

        let mut mutated = certificate.clone();
        mutated.dependencies[0].to = u32::MAX;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::DependencyEndpointUnknown { .. })
        ));

        let mut mutated = certificate.clone();
        mutated.occurrence_dependencies[0].delay += 1;
        assert_eq!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::OccurrenceDependenciesMismatch)
        );

        let mut mutated = certificate;
        mutated.occurrence_dependencies[0].to = u32::MAX;
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::OccurrenceDependencyEndpointUnknown { .. })
        ));
    }

    #[test]
    fn exact_type_snapshot_catches_fields_ignored_by_sigtype_equality() {
        let (prepared, clocks, certificate) = fixture();
        let record_index = certificate
            .records
            .iter()
            .position(|record| matches!(record.sig_type, CanonicalSigType::Simple { .. }))
            .unwrap();
        let mut mutated = certificate;
        if let CanonicalSigType::Simple { interval, .. } =
            &mut mutated.records[record_index].sig_type
        {
            interval.lsb = interval.lsb.saturating_sub(1);
        }
        assert!(matches!(
            verify_decorations(&prepared, &clocks, &mutated),
            Err(DecorationError::TypeMismatch { .. })
        ));
    }
}
