//! Single-pass delay planner: builds a [`DelayPlan`] from the prepared signal
//! DAG without any FIR side-effects.
//!
//! Provides [`DelayPlan`], [`DelayAnalysisEntry`], [`plan_delays`], the
//! [`DelayPlanner`] visitor, and the `is_recursion_delay_chain_static` helper.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use signals::{SigId, SigMatch, match_sig};
use sigtype::SigType;
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref};

use super::options::DelayOptions;
use super::sizing::delay_size_for_amount;
use super::{SignalFirError, SignalFirErrorCode};

// ─── DelayAnalysisEntry ───────────────────────────────────────────────────────

/// Read-only delay-analysis metadata for one signal carrier.
///
/// This is the first Rust-side equivalent of the C++ occurrence/delay analysis:
/// it records the maximum accumulated delay observed on a signal and how many
/// delayed accesses reached that carrier during the scan.
///
/// The data is intentionally kept separate from FIR resource allocation so
/// future planning steps can consume it without side effects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DelayAnalysisEntry {
    /// Largest accumulated delayed access observed on this carrier.
    pub(crate) max_delay: i32,
    /// Number of delayed accesses observed on this carrier.
    pub(crate) delay_count: u32,
}

// ─── DelayPlan ────────────────────────────────────────────────────────────────

/// The complete delay decision for one module, produced by a single DAG walk.
///
/// `DelayPlan` is a pure-data value with no FIR side-effects, collecting two maps:
///
/// - `lines` — the per-carrier maximum owned delay (the standalone delay lines
///   to allocate).
/// - `rec_outputs` — the recursion-output sizing metadata.
///
/// Produced by [`plan_delays`]; consumed by `prepare_delay_lines` and
/// `ensure_recursion_array_for_group`.
///
/// Source provenance (C++): `compiler/transform/signalFIRCompiler.hh`,
/// `SignalBuilder::visit` / `allocateDelayLine`, and
/// `compiler/transform/signalFIRCompiler.cpp`, `compileProjRec`. Rust adapts
/// tree identity with a clock-occurrence key because the prepared Rust DAG can
/// retain hash-consed payloads across sibling subgraphs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DelayPlan {
    /// Standalone delay lines to allocate:
    /// `(carried signal, clock-domain instance)` → required max delay.
    ///
    /// The context is part of the identity because one hash-consed signal can
    /// occur in sibling `ondemand` bodies. C++ allocates independent state for
    /// those occurrences; sharing one line would couple their fire-time state.
    ///
    /// Iteration order is allocation order (`prepare_delay_lines`) and hence
    /// struct-field and clear-loop emission order: it must stay canonical
    /// (ordered map), or emission becomes run-to-run nondeterministic.
    pub(crate) lines: BTreeMap<(SigId, Option<u32>), i32>,
    /// Recursion-output sizing metadata:
    /// `(rec_var_id, proj_index, clock-domain instance)` → entry.
    ///
    /// Stored into `DelayManager::rec_output_analysis` by `prepare_delay_lines`.
    pub(crate) rec_outputs: BTreeMap<(u32, usize, Option<u32>), DelayAnalysisEntry>,
}

// ─── plan_delays ──────────────────────────────────────────────────────────────

/// Unified single-pass delay planner: one traversal of the prepared signal DAG
/// producing both [`DelayPlan`] maps, with no FIR side-effects.
///
/// # Algorithm
///
/// An *accumulating* traversal tracks path-accumulated delay, memoised by signal
/// and occurrence clock context. A signal occurrence is revisited when reached
/// with a strictly larger accumulated delay, which fills `rec_outputs`. On its
/// first visit (tracked by the same contextual identity), the planner also
/// records the per-occurrence carrier maximum into `lines`, under these guards:
///
/// - zero-delay nodes are skipped,
/// - `!is_recursion_delay_chain` guard for both `Delay` and `Delay1`,
/// - `max_copy_delay >= 1` gate for `Delay1`.
///
/// This is correct because per-occurrence max-delay recording does not depend
/// on the accumulated delay — it only depends on the delay amount at the
/// `Delay` node itself and on whether the carried value is a recursion chain.
pub(crate) fn plan_delays(
    arena: &TreeArena,
    sig_types: &HashMap<SigId, SigType>,
    signals: &[SigId],
    options: &DelayOptions,
    clock_domains: Option<&propagate::ClockDomainTable>,
) -> Result<DelayPlan, SignalFirError> {
    DelayPlanner::new(arena, sig_types, options, clock_domains).run(signals)
}

/// Pure-function equivalent of `DelayManager::is_recursion_delay_chain` that
/// does not need `&self`.
fn is_recursion_delay_chain_static(arena: &TreeArena, value: SigId) -> bool {
    let mut current = value;
    while let SigMatch::Delay1(inner) = match_sig(arena, current) {
        current = inner;
    }
    let SigMatch::Proj(_, group) = match_sig(arena, current) else {
        return false;
    };
    match_sym_ref(arena, group).is_some() || match_sym_rec(arena, group).map(|_| ()).is_some()
}

// ─── DelayPlanner ─────────────────────────────────────────────────────────────

/// Single-pass visitor that builds a [`DelayPlan`] without threading 8
/// arguments through every recursive call.
///
/// The shared state (`arena`, `sig_types`, `options`, `clock_domains`, `plan`,
/// `best_seen_delay`, `scanned`) is held on the struct, so recursive calls only
/// need the signal, accumulated delay, and occurrence clock context.
struct DelayPlanner<'a> {
    arena: &'a TreeArena,
    sig_types: &'a HashMap<SigId, SigType>,
    options: &'a DelayOptions,
    clock_domains: Option<&'a propagate::ClockDomainTable>,
    plan: DelayPlan,
    best_seen_delay: BTreeMap<(SigId, Option<u32>), i32>,
    scanned: BTreeSet<(SigId, Option<u32>)>,
}

impl<'a> DelayPlanner<'a> {
    fn new(
        arena: &'a TreeArena,
        sig_types: &'a HashMap<SigId, SigType>,
        options: &'a DelayOptions,
        clock_domains: Option<&'a propagate::ClockDomainTable>,
    ) -> Self {
        Self {
            arena,
            sig_types,
            options,
            clock_domains,
            plan: DelayPlan::default(),
            best_seen_delay: BTreeMap::new(),
            scanned: BTreeSet::new(),
        }
    }

    /// Entry point: walk every root signal and return the finished plan.
    fn run(mut self, signals: &[SigId]) -> Result<DelayPlan, SignalFirError> {
        for &sig in signals {
            self.node(sig, 0, None)?;
        }
        Ok(self.plan)
    }

    /// Core recursive visitor.
    ///
    /// Combines the accumulating logic of `analyze_node` (tracking
    /// `accumulated_delay` along paths through `Delay` / `Delay1` / `Prefix`)
    /// with the first-visit scan-recording logic of `scan_node`.
    fn node(
        &mut self,
        sig: SigId,
        accumulated_delay: i32,
        clock_context: Option<u32>,
    ) -> Result<(), SignalFirError> {
        let visit_key = (sig, clock_context);
        // Accumulating-pass memoisation: skip if already visited with >= delay.
        if let Some(prev) = self.best_seen_delay.get(&visit_key)
            && *prev >= accumulated_delay
        {
            return Ok(());
        }
        self.best_seen_delay.insert(visit_key, accumulated_delay);

        // Accumulating pass: record rec-output analysis.
        if accumulated_delay > 0 {
            self.record_rec_output(sig, accumulated_delay, clock_context);
        }

        // First-visit scan pass: record per-carrier max owned delay.
        if self.scanned.insert(visit_key) {
            self.scan_once(sig, clock_context)?;
        }

        match match_sig(self.arena, sig) {
            SigMatch::OnDemand(children)
            | SigMatch::Upsampling(children)
            | SigMatch::Downsampling(children) => {
                let children = children.to_vec();
                let Some((&guard, payloads)) = children.split_first() else {
                    return Ok(());
                };
                if let SigMatch::Clocked(_, clock) = match_sig(self.arena, guard) {
                    // The wrapper's defining clock is evaluated before entering
                    // the domain it controls. Keep its state in the enclosing
                    // occurrence, matching `ensure_guarded_block`.
                    self.node(clock, 0, clock_context)?;
                } else {
                    // Preserve legacy/direct fast-lane traversal; guarded-block
                    // lowering will report the malformed wrapper shape.
                    self.node(guard, 0, clock_context)?;
                }
                for &payload in payloads {
                    self.node(payload, 0, clock_context)?;
                }
                return Ok(());
            }
            SigMatch::Clocked(env, inner) => {
                if let SigMatch::ClockEnvToken(domain) = match_sig(self.arena, env) {
                    self.node(inner, accumulated_delay, Some(domain))?;
                } else {
                    // Direct fast-lane callers can still carry the
                    // legacy `Clocked(clock, value)` shape. It has no domain
                    // instance identity, so preserve the enclosing context and
                    // let the normal lowering boundary report support status.
                    self.node(env, 0, clock_context)?;
                    self.node(inner, accumulated_delay, clock_context)?;
                }
                return Ok(());
            }
            SigMatch::TempVar(inner) => {
                // A TempVar is evaluated outside the guarded region that
                // consumes it. Follow the domain parent just like scalar
                // lowering's ancestor redirection, so its state is planned in
                // the same occurrence context where it will be emitted.
                let outer_context = clock_context.and_then(|domain| {
                    self.clock_domains
                        .and_then(|domains| domains.get(propagate::ClockDomainId::from_u32(domain)))
                        .and_then(|entry| entry.parent)
                        .map(propagate::ClockDomainId::as_u32)
                });
                self.node(inner, accumulated_delay, outer_context)?;
                return Ok(());
            }
            SigMatch::Delay(value, amount) => {
                let Some(delay) = delay_size_for_amount(self.arena, self.sig_types, amount)? else {
                    return Err(SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        "SIGDELAY requires a constant integer amount or a signal with a bounded non-negative interval",
                    ));
                };
                self.node(
                    value,
                    accumulated_delay.saturating_add(delay),
                    clock_context,
                )?;
                self.node(amount, 0, clock_context)?;
                return Ok(());
            }
            SigMatch::Delay1(value) => {
                self.node(value, accumulated_delay.saturating_add(1), clock_context)?;
                return Ok(());
            }
            SigMatch::Prefix(init, value) => {
                self.node(value, accumulated_delay.saturating_add(1), clock_context)?;
                self.node(init, 0, clock_context)?;
                return Ok(());
            }
            SigMatch::Proj(_, group) => {
                if let Some((_var, body_list)) = match_sym_rec(self.arena, group) {
                    let bodies = list_to_vec(self.arena, body_list).ok_or_else(|| {
                        SignalFirError::new(
                            SignalFirErrorCode::UnsupportedSignalNode,
                            "malformed symbolic recursion body list during delay planning",
                        )
                    })?;
                    for body in bodies {
                        self.node(body, 0, clock_context)?;
                    }
                    return Ok(());
                }
            }
            _ => {}
        }

        let node = self.arena.node(sig).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("missing prepared signal node {}", sig.as_u32()),
            )
        })?;
        let children: Vec<SigId> = node.children.as_slice().to_vec();
        for child in children {
            self.child(child, clock_context)?;
        }
        Ok(())
    }

    /// Walks a child node, handling list children the same way as `analyze_child`
    /// and `scan_child`.
    fn child(&mut self, child: SigId, clock_context: Option<u32>) -> Result<(), SignalFirError> {
        if self.arena.is_list(child) {
            let mut list = child;
            while !self.arena.is_nil(list) {
                let head = self.arena.hd(list).ok_or_else(|| {
                    SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        "malformed prepared signal list during delay planning",
                    )
                })?;
                self.node(head, 0, clock_context)?;
                list = self.arena.tl(list).ok_or_else(|| {
                    SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        "malformed prepared signal list during delay planning",
                    )
                })?;
            }
            Ok(())
        } else {
            self.node(child, 0, clock_context)
        }
    }

    /// Records per-occurrence carrier scan information on the first contextual
    /// visit to `sig`.
    ///
    /// Mirrors the body of `scan_node`, but operates on `plan.lines` instead of
    /// a local `max_delays` map.
    fn scan_once(&mut self, sig: SigId, clock_context: Option<u32>) -> Result<(), SignalFirError> {
        if let SigMatch::Delay(value, amount) = match_sig(self.arena, sig) {
            match delay_size_for_amount(self.arena, self.sig_types, amount)? {
                Some(0) => {}
                Some(delay) => {
                    if !is_recursion_delay_chain_static(self.arena, value) {
                        let entry = self.plan.lines.entry((value, clock_context)).or_insert(0);
                        if delay > *entry {
                            *entry = delay;
                        }
                    }
                }
                None => {
                    return Err(SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        "SIGDELAY requires a constant integer amount or a signal with a bounded non-negative interval",
                    ));
                }
            }
        }
        if let SigMatch::Delay1(value) = match_sig(self.arena, sig)
            && self.options.max_copy_delay >= 1
            && !is_recursion_delay_chain_static(self.arena, value)
        {
            let entry = self.plan.lines.entry((value, clock_context)).or_insert(0);
            if 1 > *entry {
                *entry = 1;
            }
        }
        Ok(())
    }

    /// Records recursion-output delay analysis for `sig` at `accumulated_delay`,
    /// mirroring `DelayManager::record_rec_output_delay_analysis`.
    fn record_rec_output(
        &mut self,
        sig: SigId,
        accumulated_delay: i32,
        clock_context: Option<u32>,
    ) {
        let SigMatch::Proj(index, group) = match_sig(self.arena, sig) else {
            return;
        };
        let rec_var = match match_sym_ref(self.arena, group) {
            Some(var) => Some(var),
            None => match_sym_rec(self.arena, group).map(|(var, _)| var),
        };
        let Some(var) = rec_var else {
            return;
        };
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let entry = self
            .plan
            .rec_outputs
            .entry((var.as_u32(), index, clock_context))
            .or_default();
        entry.max_delay = entry.max_delay.max(accumulated_delay);
        entry.delay_count = entry.delay_count.saturating_add(1);
    }
}
