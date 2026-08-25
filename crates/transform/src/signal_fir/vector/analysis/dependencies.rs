//! Dependency and occurrence projection over the prepared forest
//! (C++ `getSignalDependencies` / `OccMarkup` counterparts).

use super::AnalysisError;
use super::uses::*;
use signals::{SigId, SigMatch, match_sig};
use sigtype::{SigType, check_delay_interval};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref};

/// Temporal/deferred semantic class of one dependency edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepKind {
    /// Same-tick value dependency.
    Immediate,
    /// State read with a known positive delay amount.
    Delayed {
        /// The known positive delay amount in samples.
        amount: u32,
    },
    /// Reserved for the control-dependency distinction (not produced yet).
    Control,
    /// Boundary between a wrapper and its outer-domain clock.
    ClockBoundary,
    /// Reserved for effect dependencies (not produced yet).
    Effect,
}
/// One decoded dependency, keyed by source-local child order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisDependency {
    /// Signal whose decoded shape owns this edge.
    pub from: SigId,
    /// Dependency target.
    pub to: SigId,
    /// Temporal class.
    pub kind: DepKind,
    /// Stable source-local edge key (decoded child order).
    pub edge_key: usize,
}
/// One occurrence use with the C++ delay amount kept separate from scheduling
/// immediacy. A bounded delay `[0, n]`, for example, is an immediate scheduling
/// dependency and an occurrence use with delay `n`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OccurrenceUse {
    /// Signal whose decoded shape owns this use.
    pub from: SigId,
    /// Used signal.
    pub to: SigId,
    /// Maximum fixed delay attached to this use (`0` means outside a delay).
    pub delay: u32,
    /// Stable source-local use key.
    pub edge_key: usize,
}
/// The two dependency projections produced by one decoded signal shape.
///
/// Scheduling follows C++ `getSignalDependencies`; occurrences follow
/// `OccMarkup::incOcc`. Keeping both in one value preserves a single owner for
/// `SigMatch` child enumeration without conflating the two semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignalDependencies {
    pub(super) scheduling: Vec<AnalysisDependency>,
    pub(super) occurrences: Vec<OccurrenceUse>,
    pub(super) condition_children: Vec<SigId>,
}
impl SignalDependencies {
    /// Dependencies constraining Hgraph scheduling and placement.
    #[must_use]
    pub fn scheduling(&self) -> &[AnalysisDependency] {
        &self.scheduling
    }

    /// Uses consumed by C++-compatible occurrence analysis.
    #[must_use]
    pub fn occurrences(&self) -> &[OccurrenceUse] {
        &self.occurrences
    }

    /// Children receiving execution conditions through C++
    /// `conditionAnnotation`; generators intentionally have no children here.
    #[must_use]
    pub fn condition_children(&self) -> &[SigId] {
        &self.condition_children
    }
}
/// Forest-scoped inputs required by typed dependency decoding.
#[derive(Debug)]
pub struct SignalAnalysisContext<'a> {
    pub(super) arena: &'a TreeArena,
    pub(super) sig_types: &'a HashMap<SigId, SigType>,
    pub(super) rec_groups: BTreeMap<u32, SigId>,
}
impl<'a> SignalAnalysisContext<'a> {
    /// Builds the symbolic-recursion index once for the reachable forest.
    pub fn new(
        arena: &'a TreeArena,
        sig_types: &'a HashMap<SigId, SigType>,
        roots: &[SigId],
    ) -> Result<Self, AnalysisError> {
        let mut rec_groups = BTreeMap::new();
        let mut visited = BTreeSet::new();
        let mut stack = roots.to_vec();
        while let Some(sig) = stack.pop() {
            if !visited.insert(sig.as_u32()) {
                continue;
            }
            if let Some((var, _)) = match_sym_rec(arena, sig)
                && let Some(previous) = rec_groups.insert(var.as_u32(), sig)
                && previous != sig
            {
                return Err(AnalysisError::Malformed {
                    sig,
                    detail: format!(
                        "symbolic recursion variable {} names groups {} and {}",
                        var.as_u32(),
                        previous.as_u32(),
                        sig.as_u32()
                    ),
                });
            }
            if let Some(children) = arena.children(sig) {
                stack.extend(children.iter().copied());
            }
        }
        Ok(Self {
            arena,
            sig_types,
            rec_groups,
        })
    }

    /// Prepared signal arena.
    #[must_use]
    pub fn arena(&self) -> &TreeArena {
        self.arena
    }

    pub(super) fn sig_type(&self, sig: SigId) -> Result<&SigType, AnalysisError> {
        self.sig_types
            .get(&sig)
            .ok_or(AnalysisError::MissingType { sig })
    }

    pub(super) fn resolve_rec_group(&self, sig: SigId) -> Option<SigId> {
        if match_sym_rec(self.arena, sig).is_some() {
            return Some(sig);
        }
        if let Some(var) = match_sym_ref(self.arena, sig) {
            return self.rec_groups.get(&var.as_u32()).copied();
        }
        if let SigMatch::ReverseTimeRec(inner) = match_sig(self.arena, sig) {
            return self.resolve_rec_group(inner);
        }
        None
    }

    fn projection_dependency(
        &self,
        projection: SigId,
        index: i32,
        group_ref: SigId,
    ) -> Result<(SigId, DepKind), AnalysisError> {
        if index < 0 {
            return Err(AnalysisError::InvalidRecursiveProjection {
                sig: projection,
                index,
                group: group_ref,
            });
        }
        let back_reference = match_sym_ref(self.arena, group_ref).is_some();
        let group = if match_sym_rec(self.arena, group_ref).is_some() {
            Some(group_ref)
        } else if let Some(var) = match_sym_ref(self.arena, group_ref) {
            self.rec_groups.get(&var.as_u32()).copied()
        } else {
            None
        };
        let Some(group) = group else {
            // Rust-only tuple carriers such as BlockReverseAD and
            // ReverseTimeRec also use Proj. They retain the pre-analysis dependency
            // on the carrier itself; only symbolic recursion selects a body.
            return Ok((group_ref, DepKind::Immediate));
        };
        let (_, body_list) = match_sym_rec(self.arena, group).expect("resolved SYMREC group");
        let definitions =
            list_to_vec(self.arena, body_list).ok_or_else(|| AnalysisError::Malformed {
                sig: group,
                detail: "malformed SYMREC body list".to_owned(),
            })?;
        let definition = definitions
            .get(usize::try_from(index).expect("nonnegative i32 fits usize"))
            .copied()
            .ok_or_else(|| AnalysisError::Malformed {
                sig: projection,
                detail: format!(
                    "projection index {index} is outside group {} arity {}",
                    group.as_u32(),
                    definitions.len()
                ),
            })?;
        let kind = if back_reference {
            // In the acyclic Rust encoding, SYMREF denotes the implicit
            // one-sample recursion back-edge that C++ represents cyclically.
            DepKind::Delayed { amount: 1 }
        } else {
            DepKind::Immediate
        };
        Ok((definition, kind))
    }
}
/// One use count grouped by a [`UseContext`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextOccurrence {
    /// Context in which this signal is referenced.
    pub context: UseContext,
    /// Number of references in that context.
    pub count: u32,
}
/// Canonical context-sensitive counterpart of C++ `Occurrences`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OccInfo {
    /// Counts sorted by [`UseContext`].
    pub per_context: Vec<ContextOccurrence>,
    /// Whether any use prevents simple single-use treatment.
    pub multi: bool,
}
/// Checked symbolic recursive projection metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecursiveProjection {
    /// Nonnegative projection index.
    pub index: usize,
    /// Symbolic recursion group read by the projection.
    pub group: SigId,
}
/// Canonically decodes the scheduling dependencies and occurrence uses of one
/// typed signal. This is the sole `SigMatch` child enumeration for Hgraph,
/// LoopGraph, PV, and [`analyze_signal_uses`].
pub fn signal_dependencies(
    context: &SignalAnalysisContext<'_>,
    sig: SigId,
) -> Result<SignalDependencies, AnalysisError> {
    let arena = context.arena;
    let mut result = SignalDependencies::default();

    if let Some((_, body_list)) = match_sym_rec(arena, sig) {
        let definitions =
            list_to_vec(arena, body_list).ok_or_else(|| AnalysisError::Malformed {
                sig,
                detail: "malformed SYMREC body list".to_owned(),
            })?;
        for definition in definitions {
            push_occurrence(&mut result, sig, definition, 0);
            push_condition(&mut result, definition);
        }
        return Ok(result);
    }
    if match_sym_ref(arena, sig).is_some() {
        return Ok(result);
    }

    match match_sig(arena, sig) {
        SigMatch::Int(_)
        | SigMatch::Real(_)
        | SigMatch::Input(_)
        | SigMatch::Button(_)
        | SigMatch::Checkbox(_)
        | SigMatch::VSlider(_)
        | SigMatch::HSlider(_)
        | SigMatch::NumEntry(_)
        | SigMatch::Soundfile(_)
        | SigMatch::FConst(_, _, _)
        | SigMatch::FVar(_, _, _)
        | SigMatch::ClockEnvToken(_)
        | SigMatch::Unknown => {}
        SigMatch::Waveform(children) => {
            push_both_many(&mut result, sig, children.iter().copied());
        }
        SigMatch::Seq(block, held) => {
            push_schedule(&mut result, sig, block, DepKind::Immediate);
            push_occurrence(&mut result, sig, block, 0);
            push_occurrence(&mut result, sig, held, 0);
            push_condition(&mut result, block);
            push_condition(&mut result, held);
        }
        SigMatch::Delay1(value) => {
            push_schedule(&mut result, sig, value, DepKind::Delayed { amount: 1 });
            push_occurrence(&mut result, sig, value, 1);
            push_condition(&mut result, value);
        }
        SigMatch::Delay(x, amount) => {
            let amount_type = context.sig_type(amount)?;
            let max_delay = check_delay_interval(amount_type).map_err(|error| {
                AnalysisError::InvalidDelayInterval {
                    sig,
                    amount,
                    detail: error.to_string(),
                }
            })?;
            let max_delay =
                u32::try_from(max_delay).map_err(|_| AnalysisError::InvalidDelayInterval {
                    sig,
                    amount,
                    detail: format!("negative maximum delay {max_delay}"),
                })?;
            // The validated lower bound is non-negative. C++ converts it to
            // `int`, so exactly the interval [0, 1) can still schedule the
            // value as an immediate dependency.
            let schedule_kind = if amount_type.interval().lo() < 1.0 {
                DepKind::Immediate
            } else {
                DepKind::Delayed {
                    amount: max_delay.max(1),
                }
            };
            push_schedule(&mut result, sig, x, schedule_kind);
            push_schedule(&mut result, sig, amount, DepKind::Immediate);
            push_occurrence(&mut result, sig, x, max_delay);
            push_occurrence(&mut result, sig, amount, 0);
            push_condition(&mut result, x);
            push_condition(&mut result, amount);
        }
        SigMatch::Prefix(init, x) => {
            push_schedule(&mut result, sig, init, DepKind::Immediate);
            push_schedule(&mut result, sig, x, DepKind::Immediate);
            push_occurrence(&mut result, sig, init, 0);
            push_occurrence(&mut result, sig, x, 1);
            push_condition(&mut result, init);
            push_condition(&mut result, x);
        }
        SigMatch::Clocked(_, y)
        | SigMatch::TempVar(y)
        | SigMatch::PermVar(y)
        | SigMatch::Output(_, y)
        | SigMatch::IntCast(y)
        | SigMatch::BitCast(y)
        | SigMatch::FloatCast(y)
        | SigMatch::Lowest(y)
        | SigMatch::Highest(y)
        | SigMatch::Acos(y)
        | SigMatch::Asin(y)
        | SigMatch::Atan(y)
        | SigMatch::Cos(y)
        | SigMatch::Sin(y)
        | SigMatch::Tan(y)
        | SigMatch::Exp(y)
        | SigMatch::Exp10(y)
        | SigMatch::Log(y)
        | SigMatch::Log10(y)
        | SigMatch::Sqrt(y)
        | SigMatch::Abs(y)
        | SigMatch::Floor(y)
        | SigMatch::Ceil(y)
        | SigMatch::Rint(y)
        | SigMatch::Round(y)
        | SigMatch::VBargraph(_, y)
        | SigMatch::HBargraph(_, y)
        | SigMatch::ReverseTimeRec(y) => push_both(&mut result, sig, y),
        // Both `getSubSignals(..., false)` and C++ `OccMarkup` deliberately
        // stop at generators. Their contents are compiled in table context.
        SigMatch::Gen(_) => {}
        SigMatch::Proj(index, group) => {
            let (definition, kind) = context.projection_dependency(sig, index, group)?;
            push_schedule(&mut result, sig, definition, kind);
            push_occurrence(&mut result, sig, group, 0);
            push_condition(&mut result, group);
        }
        SigMatch::RdTbl(table, read_index) => {
            push_schedule(&mut result, sig, read_index, DepKind::Immediate);
            if let SigMatch::WrTbl(_, _, write_index, write_value) = match_sig(arena, table)
                && !arena.is_nil(write_index)
            {
                push_schedule(&mut result, sig, write_index, DepKind::Immediate);
                push_schedule(&mut result, sig, write_value, DepKind::Immediate);
            }
            push_occurrence(&mut result, sig, table, 0);
            push_occurrence(&mut result, sig, read_index, 0);
            push_condition(&mut result, table);
            push_condition(&mut result, read_index);
        }
        SigMatch::Control(value, gate) => {
            push_both(&mut result, sig, value);
            push_schedule(&mut result, sig, gate, DepKind::Control);
            push_occurrence(&mut result, sig, gate, 0);
            push_condition(&mut result, gate);
        }
        SigMatch::Attach(x, h) => {
            push_both(&mut result, sig, x);
            // `attach(x, h)` returns `x` and only forces `h`'s computation.
            // The `h` schedule edge is pure ordering (`Effect`), never a value
            // use: the `SIGATTACH` lowering does not load the attached value,
            // so planning a transport for it produces an orphan the body
            // checker rejects. The delay-0 occurrence stays: record coverage
            // and the scalar occurrence facts derive from occurrences, and the
            // plan suppresses occurrence-driven transports for pairs whose
            // schedule edge is `Effect`.
            push_schedule(&mut result, sig, h, DepKind::Effect);
            push_occurrence(&mut result, sig, h, 0);
            push_condition(&mut result, h);
        }
        SigMatch::ZeroPad(x, h)
        | SigMatch::Pow(x, h)
        | SigMatch::Min(x, h)
        | SigMatch::Max(x, h)
        | SigMatch::Atan2(x, h)
        | SigMatch::Fmod(x, h)
        | SigMatch::Remainder(x, h)
        | SigMatch::Enable(x, h)
        | SigMatch::SoundfileLength(x, h)
        | SigMatch::SoundfileRate(x, h)
        | SigMatch::BinOp(_, x, h) => {
            push_both(&mut result, sig, x);
            push_both(&mut result, sig, h);
        }
        SigMatch::Select2(a, b, c) | SigMatch::AssertBounds(a, b, c) => {
            push_both_many(&mut result, sig, [a, b, c]);
        }
        SigMatch::SoundfileBuffer(a, b, c, d) => {
            push_both_many(&mut result, sig, [a, b, c, d]);
        }
        SigMatch::WrTbl(size, generator, write_index, write_value) => {
            push_both(&mut result, sig, size);
            push_both(&mut result, sig, generator);
            for child in [write_index, write_value] {
                if !arena.is_nil(child) {
                    push_both(&mut result, sig, child);
                }
            }
        }
        SigMatch::OnDemand(children)
        | SigMatch::Upsampling(children)
        | SigMatch::Downsampling(children) => {
            if let Some((&clock, payload)) = children.split_first() {
                push_schedule(&mut result, sig, clock, DepKind::ClockBoundary);
                for &child in payload {
                    push_schedule(&mut result, sig, child, DepKind::Immediate);
                }
                for &child in children {
                    push_occurrence(&mut result, sig, child, 0);
                    push_condition(&mut result, child);
                }
            }
        }
        SigMatch::Fir(children) => {
            decode_fir(context, sig, children, &mut result)?;
            for &child in children {
                push_condition(&mut result, child);
            }
        }
        SigMatch::Iir(children) => {
            decode_iir(context, sig, children, &mut result)?;
            for &child in children {
                if !arena.is_nil(child) {
                    push_condition(&mut result, child);
                }
            }
        }
        SigMatch::FFun(_, args) => {
            let args = list_to_vec(arena, args).ok_or_else(|| AnalysisError::Malformed {
                sig,
                detail: "malformed FFUN argument list".to_owned(),
            })?;
            push_both_many(&mut result, sig, args);
        }
        SigMatch::BlockReverseAD {
            body,
            seeds,
            cotangents,
            ..
        } => {
            for list in [body, seeds, cotangents] {
                let items = list_to_vec(arena, list).ok_or_else(|| AnalysisError::Malformed {
                    sig,
                    detail: "malformed BlockReverseAD child list".to_owned(),
                })?;
                push_both_many(&mut result, sig, items);
            }
        }
        SigMatch::Rec(_) => {
            return Err(AnalysisError::Malformed {
                sig,
                detail: "legacy SIGREC form is not supported by hgraph".to_owned(),
            });
        }
    }
    Ok(result)
}
fn push_schedule(result: &mut SignalDependencies, from: SigId, to: SigId, kind: DepKind) {
    let edge_key = result.scheduling.len();
    result.scheduling.push(AnalysisDependency {
        from,
        to,
        kind,
        edge_key,
    });
}
fn push_occurrence(result: &mut SignalDependencies, from: SigId, to: SigId, delay: u32) {
    let edge_key = result.occurrences.len();
    result.occurrences.push(OccurrenceUse {
        from,
        to,
        delay,
        edge_key,
    });
}
fn push_both(result: &mut SignalDependencies, from: SigId, to: SigId) {
    push_schedule(result, from, to, DepKind::Immediate);
    push_occurrence(result, from, to, 0);
    push_condition(result, to);
}
fn push_both_many(
    result: &mut SignalDependencies,
    from: SigId,
    children: impl IntoIterator<Item = SigId>,
) {
    for child in children {
        push_both(result, from, child);
    }
}
fn push_condition(result: &mut SignalDependencies, child: SigId) {
    result.condition_children.push(child);
}
fn is_zero_signal(arena: &TreeArena, sig: SigId) -> bool {
    matches!(match_sig(arena, sig), SigMatch::Int(0))
        || matches!(match_sig(arena, sig), SigMatch::Real(value) if value == 0.0)
}
fn decode_fir(
    context: &SignalAnalysisContext<'_>,
    sig: SigId,
    children: &[SigId],
    result: &mut SignalDependencies,
) -> Result<(), AnalysisError> {
    let Some((&input, coefficients)) = children.split_first() else {
        return Err(AnalysisError::Malformed {
            sig,
            detail: "FIR carrier has no input".to_owned(),
        });
    };
    if coefficients.is_empty() {
        return Err(AnalysisError::Malformed {
            sig,
            detail: "FIR carrier has no coefficient".to_owned(),
        });
    }

    for &coefficient in coefficients {
        push_schedule(result, sig, coefficient, DepKind::Immediate);
        push_occurrence(result, sig, coefficient, 0);
    }
    let first_nonzero = coefficients
        .iter()
        .position(|&coefficient| !is_zero_signal(context.arena, coefficient));
    let schedule_delay = first_nonzero.unwrap_or(1);
    let schedule_kind = if schedule_delay == 0 {
        DepKind::Immediate
    } else {
        DepKind::Delayed {
            amount: u32::try_from(schedule_delay).expect("FIR tap index fits u32"),
        }
    };
    push_schedule(result, sig, input, schedule_kind);

    if first_nonzero.is_some() {
        for (delay, &coefficient) in coefficients.iter().enumerate() {
            if !is_zero_signal(context.arena, coefficient) {
                push_occurrence(
                    result,
                    sig,
                    input,
                    u32::try_from(delay).expect("FIR tap index fits u32"),
                );
            }
        }
    } else {
        push_occurrence(result, sig, input, 0);
    }
    Ok(())
}
fn decode_iir(
    _context: &SignalAnalysisContext<'_>,
    sig: SigId,
    children: &[SigId],
    result: &mut SignalDependencies,
) -> Result<(), AnalysisError> {
    if children.len() < 4 {
        return Err(AnalysisError::Malformed {
            sig,
            detail: "compact IIR carrier requires state, input, C0, and one feedback coefficient"
                .to_owned(),
        });
    }
    let input = children[1];
    for &dependency in &children[1..] {
        push_schedule(result, sig, dependency, DepKind::Immediate);
    }
    push_occurrence(result, sig, input, 0);
    // V[2] is the structural C0 term and is zero in canonical IIR carriers.
    // OccMarkup starts at V[3], whose self-use is delayed by one sample.
    for (index, &coefficient) in children.iter().enumerate().skip(3) {
        push_occurrence(result, sig, coefficient, 0);
        push_occurrence(
            result,
            sig,
            sig,
            u32::try_from(index - 2).expect("IIR tap index fits u32"),
        );
    }
    Ok(())
}
pub(super) fn compute_recursiveness(
    context: &SignalAnalysisContext<'_>,
    roots: &[SigId],
) -> Result<(BTreeMap<u32, u32>, usize), AnalysisError> {
    #[derive(Clone)]
    enum Frame {
        Enter {
            sig: SigId,
            env: Vec<SigId>,
        },
        Exit {
            sig: SigId,
            children: Vec<SigId>,
            binder: bool,
        },
    }

    // C++ `recursiveness.cpp::annotate` stores `RECURSIVNESS` directly on the
    // signal tree and returns it on every later visit, independently of the
    // current recursive environment. Keep the same first-visit memoization:
    // keying by the whole environment makes shared recursive DAGs expand once
    // per binder combination and can grow exponentially.
    let mut memo = BTreeMap::<u32, u32>::new();
    let mut expanded_signals = 0;
    let mut stack = roots
        .iter()
        .rev()
        .copied()
        .map(|sig| Frame::Enter {
            sig,
            env: Vec::new(),
        })
        .collect::<Vec<_>>();

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Enter { sig, env } => {
                let signal_key = sig.as_u32();
                if memo.contains_key(&signal_key) {
                    continue;
                }
                expanded_signals += 1;
                if let Some(var) = match_sym_ref(context.arena, sig) {
                    let depth = env
                        .iter()
                        .position(|candidate| *candidate == var)
                        .map_or(0, |position| {
                            u32::try_from(position + 1).expect("recursion depth fits u32")
                        });
                    memo.insert(signal_key, depth);
                    continue;
                }

                let dependencies = signal_dependencies(context, sig)?;
                let children = dependencies
                    .occurrences()
                    .iter()
                    .filter_map(|dependency| (dependency.to != sig).then_some(dependency.to))
                    .collect::<Vec<_>>();
                let mut child_env = env.clone();
                let binder = if let Some((var, _)) = match_sym_rec(context.arena, sig) {
                    child_env.insert(0, var);
                    true
                } else {
                    false
                };
                stack.push(Frame::Exit {
                    sig,
                    children: children.clone(),
                    binder,
                });
                for child in children.into_iter().rev() {
                    stack.push(Frame::Enter {
                        sig: child,
                        env: child_env.clone(),
                    });
                }
            }
            Frame::Exit {
                sig,
                children,
                binder,
            } => {
                let maximum = children
                    .iter()
                    .map(|&child| {
                        memo.get(&child.as_u32())
                            .copied()
                            .expect("child recursiveness computed before parent")
                    })
                    .max()
                    .unwrap_or(0);
                let value = if binder {
                    maximum.saturating_sub(1)
                } else {
                    maximum
                };
                memo.insert(sig.as_u32(), value);
            }
        }
    }
    Ok((memo, expanded_signals))
}
