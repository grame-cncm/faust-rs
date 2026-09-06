//! Effect vocabulary (`EffectAtom`, state/foreign resources), the
//! `effects_conflict` domain axioms, and effect decoration/propagation.

use super::AnalysisError;
use super::dependencies::*;
use super::uses::*;
use crate::signal_fir::recursion::recursion_read_payload;
use crate::signal_prepare::VerifiedPreparedSignals;
use signals::{SigId, SigMatch, match_sig};
use std::collections::{BTreeMap, BTreeSet};
use tlib::{NodeKind, TreeArena, list_to_vec, match_sym_rec, tree_to_int, tree_to_str};

/// Stable cell discriminator for signal-owned persistent state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateCell {
    /// Delay-line memory of a delayed signal.
    Delay,
    /// One-sample memory of a `prefix` signal.
    Prefix,
    /// Tap history of an FIR filter signal.
    Fir,
    /// Feedback history of an IIR filter signal.
    Iir,
    /// Read-position index of a waveform signal.
    WaveformIndex,
    /// Held value of a sequenced (`Seq`) block.
    Hold,
    /// Clock counter of an on-demand/up-/down-sampling wrapper.
    Clock,
    /// Buffered history of a reverse-time recursion.
    ReverseTime,
    /// Block buffer of a block-reverse audio-domain signal.
    ReverseAd,
}
/// Stable abstract identity of one persistent state resource.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateResource {
    /// State owned by one prepared signal plus a semantic cell discriminator.
    Signal {
        /// Prepared signal id that owns the state.
        owner: u32,
        /// Which semantic state cell of the owner is meant.
        cell: StateCell,
    },
    /// State owned by one symbolic recursion projection.
    Recursion {
        /// Symbolic recursion group id.
        group: u32,
        /// Projection index within the recursion group.
        projection: u32,
    },
}
/// Raw Faust foreign type code preserved independently from backend precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ForeignTypeCode(pub i64);
/// Stable identity of one declared foreign function signature.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ForeignSignature {
    /// Declared function names (per-precision name list).
    pub names: Vec<String>,
    /// Declared return type code.
    pub return_type: ForeignTypeCode,
    /// Declared argument type codes, in order.
    pub arguments: Vec<ForeignTypeCode>,
}
/// Stable foreign resource identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ForeignResource {
    /// A declared foreign function, identified by its full signature.
    Function(ForeignSignature),
    /// A declared foreign variable.
    Variable {
        /// Declared variable name.
        name: String,
        /// Declared variable type code.
        value_type: ForeignTypeCode,
    },
}
/// Declared foreign purity. Faust currently supplies no declaration, so
/// analysis-produced foreign effects use [`ForeignPurity::Unknown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ForeignPurity {
    /// Declared side-effect free; reads and calls may be reordered.
    Pure,
    /// Declared side-effecting; must keep program order.
    Impure,
    /// No purity declaration available; treated conservatively as impure.
    Unknown,
}
/// Conservative signal-level effect atom with stable resource identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EffectAtom {
    /// Reads one persistent state resource.
    ReadState(StateResource),
    /// Writes one persistent state resource.
    WriteState(StateResource),
    /// Reads the table owned by the given signal id.
    ReadTable(u32),
    /// Writes the table owned by the given signal id.
    WriteTable(u32),
    /// Writes the UI element owned by the given signal id.
    WriteUi(u32),
    /// Writes the output channel with the given index.
    WriteOutput(u32),
    /// Calls or reads a foreign resource.
    Foreign {
        /// Which foreign function or variable is touched.
        resource: ForeignResource,
        /// Declared purity of the foreign access.
        purity: ForeignPurity,
    },
}
/// Returns whether a `WrTbl` has no live writer port.
///
/// `rdtable` lowers to `WrTbl(size, generator, nil, nil)`: its content is
/// produced once by its generator before `compute` and is immutable for the
/// whole of it. `rwtable` binds both ports and stays mutable. Every component
/// that classifies table effects or admits a read-only generator must decide
/// this from one definition: reading a read-only table as a writer costs
/// coverage, and reading a mutable table as read-only would admit an unsound
/// program.
#[must_use]
pub fn wrtbl_is_readonly(arena: &TreeArena, write_index: SigId, write_value: SigId) -> bool {
    arena.is_nil(write_index) && arena.is_nil(write_value)
}
/// Returns whether two atoms cannot be freely reordered.
#[must_use]
pub fn effects_conflict(left: &EffectAtom, right: &EffectAtom) -> bool {
    use EffectAtom::{Foreign, ReadState, ReadTable, WriteOutput, WriteState, WriteTable, WriteUi};

    let foreign_barrier = |effect: &EffectAtom| {
        matches!(
            effect,
            Foreign {
                purity: ForeignPurity::Impure | ForeignPurity::Unknown,
                ..
            }
        )
    };
    if foreign_barrier(left) || foreign_barrier(right) {
        return true;
    }
    match (left, right) {
        (ReadState(a), WriteState(b))
        | (WriteState(a), ReadState(b))
        | (WriteState(a), WriteState(b)) => a == b,
        (ReadTable(a), WriteTable(b))
        | (WriteTable(a), ReadTable(b))
        | (WriteTable(a), WriteTable(b)) => a == b,
        (WriteUi(a), WriteUi(b)) | (WriteOutput(a), WriteOutput(b)) => a == b,
        _ => false,
    }
}
/// Returns whether any pair of atoms in two effect sets conflicts.
#[must_use]
pub fn effect_sets_conflict(left: &[EffectAtom], right: &[EffectAtom]) -> bool {
    left.iter()
        .any(|a| right.iter().any(|b| effects_conflict(a, b)))
}
fn state_effects(sig: SigId, cell: StateCell) -> BTreeSet<EffectAtom> {
    let resource = StateResource::Signal {
        owner: sig.as_u32(),
        cell,
    };
    BTreeSet::from([
        EffectAtom::ReadState(resource.clone()),
        EffectAtom::WriteState(resource),
    ])
}
fn match_ffunction_descriptor(arena: &TreeArena, id: SigId) -> Option<(SigId, SigId, SigId)> {
    let node = arena.node(id)?;
    let NodeKind::Tag(tag_id) = node.kind else {
        return None;
    };
    if arena.tag_name(tag_id)? != "FFUN" {
        return None;
    }
    let [signature, include_file, library_file] = node.children.as_slice() else {
        return None;
    };
    Some((*signature, *include_file, *library_file))
}
fn decode_foreign_signature(
    arena: &TreeArena,
    owner: SigId,
    descriptor: SigId,
) -> Result<ForeignSignature, AnalysisError> {
    let Some((signature, _, _)) = match_ffunction_descriptor(arena, descriptor) else {
        return Err(AnalysisError::Malformed {
            sig: owner,
            detail: "FFUN call has a malformed foreign-function descriptor".to_owned(),
        });
    };
    let items = list_to_vec(arena, signature).ok_or_else(|| AnalysisError::Malformed {
        sig: owner,
        detail: "FFUN signature is not a list".to_owned(),
    })?;
    if items.len() < 2 {
        return Err(AnalysisError::Malformed {
            sig: owner,
            detail: "FFUN signature needs a return type and name list".to_owned(),
        });
    }
    let return_type = tree_to_int(arena, items[0]).ok_or_else(|| AnalysisError::Malformed {
        sig: owner,
        detail: "FFUN return type is not an integer code".to_owned(),
    })?;
    let names = list_to_vec(arena, items[1])
        .ok_or_else(|| AnalysisError::Malformed {
            sig: owner,
            detail: "FFUN names are not a list".to_owned(),
        })?
        .into_iter()
        .map(|name| {
            tree_to_str(arena, name)
                .map(str::to_owned)
                .ok_or_else(|| AnalysisError::Malformed {
                    sig: owner,
                    detail: "FFUN name slot is not a symbol".to_owned(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let arguments = items[2..]
        .iter()
        .map(|&item| {
            tree_to_int(arena, item)
                .map(ForeignTypeCode)
                .ok_or_else(|| AnalysisError::Malformed {
                    sig: owner,
                    detail: "FFUN argument type is not an integer code".to_owned(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ForeignSignature {
        names,
        return_type: ForeignTypeCode(return_type),
        arguments,
    })
}
pub(super) fn direct_effects(
    analysis: &SignalAnalysisContext<'_>,
    sig: SigId,
) -> Result<BTreeSet<EffectAtom>, AnalysisError> {
    let arena = analysis.arena;
    if let Some((_, body)) = match_sym_rec(arena, sig) {
        let definitions = list_to_vec(arena, body).ok_or_else(|| AnalysisError::Malformed {
            sig,
            detail: "malformed SYMREC body list while deriving effects".to_owned(),
        })?;
        let mut effects = BTreeSet::new();
        for index in 0..definitions.len() {
            let projection = u32::try_from(index).map_err(|_| AnalysisError::Malformed {
                sig,
                detail: "SYMREC projection index does not fit u32".to_owned(),
            })?;
            let resource = StateResource::Recursion {
                group: sig.as_u32(),
                projection,
            };
            effects.insert(EffectAtom::ReadState(resource.clone()));
            effects.insert(EffectAtom::WriteState(resource));
        }
        return Ok(effects);
    }

    let effects = match match_sig(arena, sig) {
        // Delay storage is allocated for the carried signal and shared by all
        // of its readers, regardless of the requested history depth.
        SigMatch::Delay1(value) | SigMatch::Delay(value, _) => {
            state_effects(recursion_read_payload(arena, value), StateCell::Delay)
        }
        SigMatch::Prefix(_, _) => state_effects(sig, StateCell::Prefix),
        SigMatch::Fir(_) => state_effects(sig, StateCell::Fir),
        SigMatch::Iir(_) => state_effects(sig, StateCell::Iir),
        SigMatch::Waveform(_) => state_effects(sig, StateCell::WaveformIndex),
        SigMatch::Seq(_, _) => state_effects(sig, StateCell::Hold),
        SigMatch::Clocked(_, _)
        | SigMatch::OnDemand(_)
        | SigMatch::Upsampling(_)
        | SigMatch::Downsampling(_) => state_effects(sig, StateCell::Clock),
        SigMatch::ReverseTimeRec(_) => state_effects(sig, StateCell::ReverseTime),
        SigMatch::BlockReverseAD { .. } => state_effects(sig, StateCell::ReverseAd),
        SigMatch::Proj(index, group_ref) if index >= 0 => {
            let group = analysis.resolve_rec_group(group_ref).unwrap_or(group_ref);
            let resource = StateResource::Recursion {
                group: group.as_u32(),
                projection: u32::try_from(index).expect("nonnegative i32 fits u32"),
            };
            BTreeSet::from([
                EffectAtom::ReadState(resource.clone()),
                EffectAtom::WriteState(resource),
            ])
        }
        // A read-only table has no live writer port, so it contributes no
        // compute-time write. Its generator subtree keeps its own effects, and
        // fill-before-read stays carried by the data edge from every `RdTbl` to
        // its table operand.
        SigMatch::WrTbl(_, _, write_index, write_value)
            if wrtbl_is_readonly(arena, write_index, write_value) =>
        {
            BTreeSet::new()
        }
        SigMatch::WrTbl(_, _, _, _) => BTreeSet::from([EffectAtom::WriteTable(sig.as_u32())]),
        SigMatch::RdTbl(table, _) => BTreeSet::from([EffectAtom::ReadTable(table.as_u32())]),
        SigMatch::VBargraph(control, _) | SigMatch::HBargraph(control, _) => {
            BTreeSet::from([EffectAtom::WriteUi(control)])
        }
        SigMatch::Output(channel, _) if channel >= 0 => BTreeSet::from([EffectAtom::WriteOutput(
            u32::try_from(channel).expect("nonnegative i32 fits u32"),
        )]),
        SigMatch::Output(channel, _) => {
            return Err(AnalysisError::Malformed {
                sig,
                detail: format!("negative output channel {channel}"),
            });
        }
        SigMatch::FFun(descriptor, _) => BTreeSet::from([EffectAtom::Foreign {
            resource: ForeignResource::Function(decode_foreign_signature(arena, sig, descriptor)?),
            purity: ForeignPurity::Unknown,
        }]),
        SigMatch::FVar(value_type, name, _) => {
            let name = tree_to_str(arena, name).ok_or_else(|| AnalysisError::Malformed {
                sig,
                detail: "foreign variable name is not a symbol".to_owned(),
            })?;
            if name == "count" {
                BTreeSet::new()
            } else {
                let value_type =
                    tree_to_int(arena, value_type).ok_or_else(|| AnalysisError::Malformed {
                        sig,
                        detail: "foreign variable type is not an integer code".to_owned(),
                    })?;
                BTreeSet::from([EffectAtom::Foreign {
                    resource: ForeignResource::Variable {
                        name: name.to_owned(),
                        value_type: ForeignTypeCode(value_type),
                    },
                    purity: ForeignPurity::Unknown,
                }])
            }
        }
        _ => BTreeSet::new(),
    };
    Ok(effects)
}
pub(super) fn decorate_effects(
    analysis: &SignalAnalysisContext<'_>,
    records: &mut BTreeMap<u32, SignalUseRecord>,
    dependencies: &BTreeMap<u32, SignalDependencies>,
) -> Result<(), AnalysisError> {
    let mut direct = BTreeMap::<u32, BTreeSet<EffectAtom>>::new();
    for record in records.values() {
        direct.insert(record.sig.as_u32(), direct_effects(analysis, record.sig)?);
    }
    // The former whole-map fixed point cloned every signal's complete effect
    // set once per graph-depth iteration. Deep UI-bearing DSPs therefore paid
    // roughly O(depth * signals * effects). Propagate only changed child sets
    // to their parents instead; this computes the same least union fixed point
    // and converges over cycles without rescanning unrelated records.
    let mut parents = BTreeMap::<u32, BTreeSet<u32>>::new();
    for (&parent, projection) in dependencies {
        for child in projection.condition_children() {
            if direct.contains_key(&child.as_u32()) {
                parents.entry(child.as_u32()).or_default().insert(parent);
            }
        }
    }
    let (accumulated, _) = propagate_effect_sets(&direct, &parents);
    for (&sig, effects) in &accumulated {
        let info = &mut records
            .get_mut(&sig)
            .expect("effect record has matching signal record")
            .info;
        info.effects = effects.iter().cloned().collect();
        info.direct_effects = direct[&sig].iter().cloned().collect();
    }
    Ok(())
}
pub(super) fn propagate_effect_sets(
    direct: &BTreeMap<u32, BTreeSet<EffectAtom>>,
    parents: &BTreeMap<u32, BTreeSet<u32>>,
) -> (BTreeMap<u32, BTreeSet<EffectAtom>>, usize) {
    let mut accumulated = direct.clone();
    let mut pending = accumulated.keys().copied().collect::<BTreeSet<_>>();
    let mut updates = 0_usize;
    while let Some(child) = pending.pop_first() {
        let child_effects = accumulated
            .remove(&child)
            .expect("pending effect child has an accumulated set");
        for &parent in parents.get(&child).into_iter().flatten() {
            if parent == child {
                continue;
            }
            let parent_effects = accumulated
                .get_mut(&parent)
                .expect("effect parent has a signal record");
            let previous_len = parent_effects.len();
            parent_effects.extend(child_effects.iter().cloned());
            if parent_effects.len() != previous_len {
                updates += 1;
                pending.insert(parent);
            }
        }
        accumulated.insert(child, child_effects);
    }
    (accumulated, updates)
}
/// Direct effect facts needed by scalar scheduling, keyed by prepared signal.
///
/// Unlike [`VectorSignalAnalysis`](super::VectorSignalAnalysis), this intentionally contains neither
/// occurrence facts nor execution conditions: scalar conflict orientation only
/// needs the effects performed by each node itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarSchedulingEffects {
    direct: BTreeMap<u32, Vec<EffectAtom>>,
}
impl ScalarSchedulingEffects {
    #[must_use]
    pub(crate) fn direct_effects(&self, sig: SigId) -> &[EffectAtom] {
        self.direct.get(&sig.as_u32()).map_or(&[], Vec::as_slice)
    }
}
/// Builds only the compute-time effect facts required by scalar scheduling.
///
/// Scalar [`crate::hgraph::orient_effect_conflicts`] consumes direct effects
/// and never inspects occurrence facts, clock environments, or execution
/// conditions. Avoiding those vector-oriented analyses keeps `-ss` from
/// paying for certification data it cannot observe.
pub fn analyze_scalar_scheduling_effects(
    prepared: &VerifiedPreparedSignals,
) -> Result<ScalarSchedulingEffects, AnalysisError> {
    let analysis = SignalAnalysisContext::new(
        prepared.arena(),
        prepared.sig_types_map(),
        prepared.outputs(),
    )?;
    let mut direct = BTreeMap::new();
    for &sig in prepared.sig_types_map().keys() {
        let effects = direct_effects(&analysis, sig)?.into_iter().collect();
        direct.insert(sig.as_u32(), effects);
    }
    Ok(ScalarSchedulingEffects { direct })
}
