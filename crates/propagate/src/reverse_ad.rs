//! Reverse-mode automatic differentiation for `rad(expr, seeds)`.
//!
//! # Source provenance
//! Original Rust design — there is no upstream C++ reverse-mode AD path
//! to mirror at this surface. The plan is documented in
//! `porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md`.
//!
//! # Output layout
//!
//! ```text
//! rad(expr, (s0, s1, …, s{N-1}))
//!   = [ expr_0, expr_1, …, expr_{M-1},
//!       d sum(expr_i) / d s_0,
//!       d sum(expr_i) / d s_1,
//!       …,
//!       d sum(expr_i) / d s_{N-1} ]
//! ```
//!
//! The implicit cotangent on every primal output is `1.0`. A future VJP-style
//! API can expose a custom output cotangent vector.
//!
//! # Algorithm
//! Three explicit passes on a single `ReverseADTransform` instance:
//!
//! 1. **Active subgraph collection.** Postorder DFS from each primal output
//!    through differentiable children, stopping at any `SigId` that appears
//!    in the seed list. DAG sharing is preserved by a `visited` set so each
//!    node is visited once.
//! 2. **Adjoint accumulation.** Initialize `adjoints[primal] = 1.0` for every
//!    primal, then walk the postorder in reverse and emit local transpose
//!    contributions `child_bar += y_bar * d y / d child` for each visited
//!    `y`. Existing entries are summed via [`add_adjoint`].
//! 3. **Seed extraction.** Re-emit `primals` followed by, for each seed
//!    lane, the accumulated `adjoints[seed]` (or `0.0` when the seed is not
//!    reached from any primal output, e.g. `rad(sin(x), y)`).
//!
//! # Phase B scope
//!
//! Phase B targets the feed-forward subset that overlaps the existing
//! forward-AD differentiable set:
//!
//! - constants, audio inputs, UI controls (zero contribution unless seed),
//! - arithmetic `BinOp` (Add/Sub/Mul/Div/Rem and discrete bitwise/comparison),
//! - unary trig/exp/log/sqrt/abs and inverse-trig (Acos/Asin/Atan),
//! - binary math `Pow`, `Atan2`, `Fmod`, `Remainder`,
//! - `Min`/`Max` via primal comparison,
//! - `Select2` routes adjoint to the chosen branch,
//! - `IntCast` (zero) and `FloatCast` (forward),
//! - bargraphs (zero contribution).
//!
//! Phase C extension (this revision):
//!
//! - read-only `RdTbl(T, idx)` propagates adjoint through the read index
//!   only, using the same symmetric finite-difference slope as the FAD pass
//!   `(rdtbl(T, idx + 1) - rdtbl(T, idx - 1)) / 2`.
//! - unary foreign functions (`tanhf`/`tanh`/`tanhl`,
//!   `sinhf`/`sinh`/`sinhl`, `coshf`/`cosh`/`coshl` and the inverse-trig
//!   counterparts) propagate adjoint through the same chain-rule formulas
//!   used by FAD.
//! - pass-through wrappers (`Attach`, `Enable`, `Control`) and `Output`
//!   forward adjoint through the signal-carrying operand only, matching
//!   FAD's transparency contract.
//!
//! The 2026-05-10 dispatcher change keeps this symbolic pass feed-forward
//! only. Temporal, recursive, and `SigIIR` nodes raise
//! [`PropagateError::RadUnsupportedNode`] and are routed by
//! [`generate_rad_signals`] to the `SigBlockReverseAD` fallback. The legacy
//! `ReverseTimeRec` LTI/IIR path remains as dormant helper infrastructure but
//! public RAD propagation does not produce it.
//!
//! Out of scope for the symbolic sweep (raise [`PropagateError::RadUnsupportedNode`]):
//!
//! - delay, prefix, recursion, projection,
//! - mutable tables (`WrTbl` with non-nil write ports) and waveform
//!   sources used outside `rdtbl`,
//! - non-unary or unrecognized foreign functions,
//! - soundfile accessors,
//! - representation casts and other opaque families.
//!
//! Phase D refines the strict diagnostics around temporal nodes.
//!
//! # Why temporal nodes refuse adjoint
//!
//! Forward-mode AD applies a *causal* rule for delays:
//!
//! ```text
//! d/dp delay1(x) = delay1(x')   // tangent at frame n only depends on frame n-1
//! ```
//!
//! Reverse-mode AD requires the transpose, which is *anti-causal*:
//!
//! ```text
//! adj_x[n] += adj_y[n + 1]      // adjoint at frame n depends on a future frame
//! ```
//!
//! A correct reverse pass therefore needs either
//!
//! - a finite block tape that buffers primal intermediates and a backward
//!   scan over that block (the `SigBlockReverseAD` fallback), or
//! - a causal approximation that is explicitly not exact reverse mode.
//!
//! RAD takes the block-fallback route for temporal and recursive families:
//! the symbolic sweep raises [`PropagateError::RadUnsupportedNode`] with a
//! tailored diagnostic, and [`generate_rad_signals`] converts the supported
//! temporal/recursive kinds into `SigBlockReverseAD`.
//!
//! # `SigBlockReverseAD` is a semantic carrier, not generated FIR
//!
//! The propagation crate deliberately stops at a Signal-IR carrier:
//! [`SigMatch::BlockReverseAD`].  It records only the mathematical contract:
//! body outputs, seed signals, implicit cotangents, and a tape policy.  It does
//! **not** allocate tape arrays, choose FIR storage types, or decide where the
//! block sweep runs.  Those decisions happen later in
//! `crates/transform/src/signal_fir/module.rs`, after signal preparation,
//! type reduction, recursion lowering, and backend real-type selection.
//!
//! This separation is important for two reasons:
//!
//! - `reverse_ad.rs` still works in Signal IR, where nodes are typed by the
//!   signal type system and can be rewritten by later normalization/preparation
//!   passes.  Backend-local objects such as `fBraTapeN` arrays are not signal
//!   nodes and cannot participate in `normalform.rs` / `signalPromotion`.
//! - The correct execution schedule depends on the surrounding FIR context.
//!   A top-level `process = rad(body, seeds)` usually exposes both primal and
//!   gradient projections as public outputs, so FIR lowering emits a forward
//!   loop for primals followed by a reverse loop for gradients.  In adaptive
//!   DSPs, however, a gradient projection may be consumed inside a recursive
//!   forward update, e.g. `p_next = p - lr * (rad(loss, p) : !, _)`.  In that
//!   shape the outer public output is the recursive primal, and descending into
//!   the recursion body while classifying public outputs would be wrong.  The
//!   FIR lowerer therefore emits the BRA sweep inline in the currently lowered
//!   forward loop; there is no separate public backward loop in the generated
//!   `compute()`.
//!
//! The carrier is therefore best read as “this sub-expression needs block-local
//! reverse-mode semantics when lowered,” not as “the final C++ must contain an
//! explicit backward loop.”  Whether the sweep is split or interleaved is a
//! backend scheduling decision.
//!
//! This scheduling rule is separate from the dormant LTI/IIR transpose helper
//! path below.  “A gradient projection is consumed inside a forward recursive
//! body” describes the *use site* of a `BlockReverseAD` projection and therefore
//! where FIR statements are inserted.  It does not mean the recursive sub-graph
//! was proven linear/time-invariant or replaced by an analytic transposed
//! recursion.  The LTI path would change the derivative representation itself;
//! the BRA fallback keeps the general block tape representation and only lets
//! FIR lowering choose split versus inline placement.

use ahash::{AHashMap, AHashSet};
use signals::{
    BinOp, BlockRevPolicy, SigBuilder, SigId, SigMatch,
    ad_rules::{
        RadBinaryMathRule, RadUnaryMathRule, rad_binary_contributions, rad_binary_math_rule,
        rad_binop_contributions, rad_binop_rule, rad_unary_contribution, rad_unary_math_rule,
    },
    match_sig,
};
use smallvec::SmallVec;
use tlib::{
    NodeKind, TreeArena, check_de_bruijn_coherence, is_de_bruijn_closed, list_to_vec,
    match_de_bruijn_rec, tree_to_str,
};

use crate::PropagateError;
use crate::stateful_rad::{
    RecRadMode, classify_de_bruijn_rec_rad_mode, classify_recursive_projection_rad_mode,
};
use crate::transpose_ad::{TransposeAdError, transpose_lti_de_bruijn_rec_with_cotangents};

/// Collects the active subgraph and accumulates adjoints for one
/// `rad(expr, seeds)` call.
struct ReverseADTransform<'a> {
    arena: &'a mut TreeArena,
    seed_set: AHashSet<SigId>,
    /// Postorder of the active subgraph (children before parents).
    /// Seed nodes appear as leaves.
    postorder: Vec<SigId>,
    /// DAG sharing: every signal is visited at most once.
    visited: AHashSet<SigId>,
    /// Accumulated adjoint per visited node.
    adjoints: AHashMap<SigId, SigId>,
}

impl<'a> ReverseADTransform<'a> {
    fn new(arena: &'a mut TreeArena, seeds: &[SigId]) -> Self {
        let mut seed_set = AHashSet::with_capacity(seeds.len());
        for &s in seeds {
            seed_set.insert(s);
        }
        Self {
            arena,
            seed_set,
            postorder: Vec::new(),
            visited: AHashSet::new(),
            adjoints: AHashMap::new(),
        }
    }

    /// Returns the cached children that contribute adjoint signal flow for
    /// the dispatch of node `sig`. Pure read on the arena; the same matcher
    /// drives the adjoint emission below.
    fn active_children(&self, sig: SigId) -> Result<SmallVec<[SigId; 4]>, PropagateError> {
        let mut out: SmallVec<[SigId; 4]> = SmallVec::new();
        let decoded = match_sig(self.arena, sig);
        if let Some((_rule, x)) = rad_unary_math_rule(&decoded) {
            out.push(x);
            return Ok(out);
        }
        if let Some((_rule, x, y)) = rad_binary_math_rule(&decoded) {
            out.push(x);
            out.push(y);
            return Ok(out);
        }
        match decoded {
            // Leaves — no descent.
            SigMatch::Int(_)
            | SigMatch::Real(_)
            | SigMatch::Input(_)
            | SigMatch::HSlider(_)
            | SigMatch::VSlider(_)
            | SigMatch::NumEntry(_)
            | SigMatch::Button(_)
            | SigMatch::Checkbox(_) => {}
            // Arithmetic / math.
            SigMatch::BinOp(_, x, y) => {
                out.push(x);
                out.push(y);
            }
            SigMatch::IntCast(x) | SigMatch::FloatCast(x) => {
                out.push(x);
            }
            SigMatch::Select2(cond, x, y) => {
                // The condition is a discrete branch selector; phase B does
                // not propagate adjoint through it.
                let _ = cond;
                out.push(x);
                out.push(y);
            }
            SigMatch::VBargraph(_, inner) | SigMatch::HBargraph(_, inner) => {
                // Metering sinks: no adjoint contribution but still walked
                // so a seed reachable only through a bargraph is correctly
                // recorded as unreachable (gradient zero).
                let _ = inner;
            }
            SigMatch::RdTbl(table, ridx) => {
                // Phase C: read-only tables are differentiable through the
                // read index. Mutable tables fall through to the strict
                // failure path below.
                if !is_readonly_table_source(self.arena, table) {
                    return Err(PropagateError::RadUnsupportedNode {
                        node: sig,
                        kind: "writable-table",
                    });
                }
                out.push(ridx);
            }
            SigMatch::FFun(ff, largs) => {
                // Phase C: only the unary chain-rule rules from FAD are
                // recognized. Non-unary or unknown FFuns refuse adjoint.
                let args = list_to_vec(self.arena, largs).unwrap_or_default();
                if args.len() != 1 || !ffun_unary_supported(self.arena, ff) {
                    return Err(PropagateError::RadUnsupportedNode {
                        node: sig,
                        kind: "ffun",
                    });
                }
                out.push(args[0]);
            }
            SigMatch::Attach(x, _) | SigMatch::Enable(x, _) | SigMatch::Control(x, _) => {
                // Pass-through nodes: only the signal-carrying operand
                // contributes to the adjoint flow; the side-effect /
                // control operand is ignored, mirroring FAD.
                out.push(x);
            }
            SigMatch::Output(_, inner) => {
                // `Output` is transparent to differentiation; forward the
                // adjoint to the wrapped signal.
                out.push(inner);
            }
            // Families outside the local symbolic sweep. Temporal/recursive
            // kinds are caught by `generate_rad_signals` and routed to
            // BlockReverseAD; hard unsupported kinds surface as diagnostics.
            SigMatch::Delay1(_) | SigMatch::Delay(_, _) | SigMatch::Prefix(_, _) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "delay-or-prefix",
                });
            }
            SigMatch::Iir(_) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "iir-state-space",
                });
            }
            SigMatch::Proj(_, _) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "recursive-projection",
                });
            }
            SigMatch::Rec(_) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: recursive_rad_unsupported_kind(self.arena, sig),
                });
            }
            SigMatch::WrTbl(_, _, _, _) | SigMatch::Waveform(_) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "writable-table-or-waveform-direct",
                });
            }
            SigMatch::Soundfile(_)
            | SigMatch::SoundfileLength(_, _)
            | SigMatch::SoundfileRate(_, _)
            | SigMatch::SoundfileBuffer(_, _, _, _) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "soundfile",
                });
            }
            // Clock-domain machinery (`ondemand` / `upsampling` /
            // `downsampling` wrappers and their glue): reverse mode across a
            // boundary needs the clock-aware tape (roadmap P8) or the LPTV
            // transpose for constant rates. Name the construct so users see
            // *why* the program is rejected (roadmap P0.4), instead of the
            // generic "other".
            SigMatch::Seq(_, _) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "clocked sequencing (Seq)",
                });
            }
            SigMatch::Clocked(_, _) | SigMatch::ClockEnvToken(_) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "clocked wrapper",
                });
            }
            SigMatch::TempVar(_) | SigMatch::PermVar(_) | SigMatch::ZeroPad(_, _) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "clock-domain boundary variable (TempVar/PermVar/ZeroPad)",
                });
            }
            SigMatch::OnDemand(_) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "ondemand",
                });
            }
            SigMatch::Upsampling(_) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "upsampling",
                });
            }
            SigMatch::Downsampling(_) => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "downsampling",
                });
            }
            // Catch-all: every other signal family is opaque to RAD in
            // phase B. This includes representation-level casts, integer
            // rounding, foreign constants/variables, and generators.
            // Reject loudly rather than silently dropping a gradient.
            _ => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: sig,
                    kind: "other",
                });
            }
        }
        Ok(out)
    }

    /// DFS from `root` producing a deterministic postorder, stopping descent
    /// at any seed.
    fn collect_dfs(&mut self, root: SigId) -> Result<(), PropagateError> {
        if !self.visited.insert(root) {
            return Ok(());
        }
        if self.seed_set.contains(&root) {
            self.postorder.push(root);
            return Ok(());
        }
        let children = self.active_children(root)?;
        for child in children {
            self.collect_dfs(child)?;
        }
        self.postorder.push(root);
        Ok(())
    }

    /// `target_bar = target_bar + contribution`, with cheap zero-folding so
    /// `0 + x` becomes `x` instead of materializing a redundant Add.
    fn add_adjoint(&mut self, target: SigId, contribution: SigId) {
        match self.adjoints.get(&target).copied() {
            None => {
                self.adjoints.insert(target, contribution);
            }
            Some(existing) => {
                let mut b = SigBuilder::new(self.arena);
                let summed = b.add(existing, contribution);
                self.adjoints.insert(target, summed);
            }
        }
    }

    /// Propagates `y_bar` into the adjoints of `y`'s active children using
    /// the rule table from the module docstring.
    fn propagate_adjoint(&mut self, y: SigId, y_bar: SigId) -> Result<(), PropagateError> {
        // Re-decode `y` for each call — the matcher is a cheap lookup and
        // keeps the rule arms fully local.
        let decoded = match_sig(self.arena, y);
        if let Some((rule, x)) = rad_unary_math_rule(&decoded) {
            self.propagate_unary_math(rule, x, y_bar);
            return Ok(());
        }
        if let Some((rule, x, z)) = rad_binary_math_rule(&decoded) {
            self.propagate_binary_math(rule, x, z, y_bar);
            return Ok(());
        }
        match decoded {
            SigMatch::Int(_)
            | SigMatch::Real(_)
            | SigMatch::Input(_)
            | SigMatch::HSlider(_)
            | SigMatch::VSlider(_)
            | SigMatch::NumEntry(_)
            | SigMatch::Button(_)
            | SigMatch::Checkbox(_) => {
                // Leaves; nothing downstream.
            }
            SigMatch::BinOp(op, x, z) => self.propagate_binop(op, x, z, y_bar),
            SigMatch::FloatCast(x) => {
                let mut b = SigBuilder::new(self.arena);
                let contrib = b.float_cast(y_bar);
                self.add_adjoint(x, contrib);
            }
            SigMatch::IntCast(_) => {
                // d/dx int_cast(x) is treated as zero (truncation jumps).
            }
            SigMatch::Select2(cond, x, z) => {
                // y = select2(cond, x, z): the chosen branch receives
                // `y_bar`, the other receives `0`. SigBuilder::select2(a, b)
                // returns `b` when `cond != 0` and `a` when `cond == 0`,
                // matching the convention used by the FAD pass.
                let mut b = SigBuilder::new(self.arena);
                let zero = b.real(0.0);
                // x branch: cond == 0 -> select2(cond, y_bar, 0) = y_bar.
                let x_contrib = b.select2(cond, y_bar, zero);
                // z branch: cond != 0 -> select2(cond, 0, y_bar) = y_bar.
                let z_contrib = b.select2(cond, zero, y_bar);
                self.add_adjoint(x, x_contrib);
                self.add_adjoint(z, z_contrib);
            }
            SigMatch::VBargraph(_, _) | SigMatch::HBargraph(_, _) => {
                // Sinks: no adjoint passes through.
            }
            SigMatch::RdTbl(table, ridx) => {
                // Phase C: read-only table read.
                //   y = rdtbl(T, i)
                //   slope(i) ≈ (rdtbl(T, i+1) - rdtbl(T, i-1)) / 2
                //   i_bar += y_bar * slope(i)
                let mut b = SigBuilder::new(self.arena);
                let one = b.int(1);
                let two = b.real(2.0);
                let idx_plus = b.add(ridx, one);
                let idx_minus = b.sub(ridx, one);
                let plus = b.rdtbl(table, idx_plus);
                let minus = b.rdtbl(table, idx_minus);
                let diff = b.sub(plus, minus);
                let slope = b.div(diff, two);
                let contrib = b.mul(y_bar, slope);
                self.add_adjoint(ridx, contrib);
            }
            SigMatch::FFun(ff, largs) => {
                // Resolve the FFUN family name first so the ffun_is /
                // arena-immutable lookups don't fight the SigBuilder's
                // exclusive borrow during contribution construction.
                let args = list_to_vec(self.arena, largs).unwrap_or_default();
                let arg = args[0];
                let kind = ffun_unary_kind(self.arena, ff);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.ffun(ff, largs);
                let contrib = match kind {
                    Some(FFunUnaryKind::Tanh) => {
                        let tanh_sq = b.mul(primal, primal);
                        let one = b.real(1.0);
                        let sech_sq = b.sub(one, tanh_sq);
                        b.mul(y_bar, sech_sq)
                    }
                    Some(FFunUnaryKind::Sinh) => {
                        let sinh_sq = b.mul(primal, primal);
                        let one = b.real(1.0);
                        let one_plus_sq = b.add(one, sinh_sq);
                        let cosh_x = b.sqrt(one_plus_sq);
                        b.mul(y_bar, cosh_x)
                    }
                    Some(FFunUnaryKind::Cosh) => {
                        let exp_x = b.exp(arg);
                        let minus_one = b.real(-1.0);
                        let neg_x = b.mul(minus_one, arg);
                        let exp_neg_x = b.exp(neg_x);
                        let diff = b.sub(exp_x, exp_neg_x);
                        let half = b.real(0.5);
                        let sinh_x = b.mul(half, diff);
                        b.mul(y_bar, sinh_x)
                    }
                    Some(FFunUnaryKind::Atanh) => {
                        let x_sq = b.mul(arg, arg);
                        let one = b.real(1.0);
                        let denom = b.sub(one, x_sq);
                        b.div(y_bar, denom)
                    }
                    Some(FFunUnaryKind::Asinh) => {
                        let x_sq = b.mul(arg, arg);
                        let one = b.real(1.0);
                        let sum = b.add(one, x_sq);
                        let denom = b.sqrt(sum);
                        b.div(y_bar, denom)
                    }
                    Some(FFunUnaryKind::Acosh) => {
                        let x_sq = b.mul(arg, arg);
                        let one = b.real(1.0);
                        let diff = b.sub(x_sq, one);
                        let denom = b.sqrt(diff);
                        b.div(y_bar, denom)
                    }
                    None => {
                        // Defensive: should be unreachable because
                        // `active_children` already gated on the
                        // recognized names.
                        return Err(PropagateError::RadUnsupportedNode {
                            node: y,
                            kind: "ffun-unrecognized",
                        });
                    }
                };
                self.add_adjoint(arg, contrib);
            }
            SigMatch::Attach(x, _) | SigMatch::Enable(x, _) | SigMatch::Control(x, _) => {
                // Pass-through wrappers: forward the full adjoint to the
                // signal-carrying operand only.
                self.add_adjoint(x, y_bar);
            }
            SigMatch::Output(_, inner) => {
                self.add_adjoint(inner, y_bar);
            }
            // Families that need fallback or hard diagnostics are reported in
            // `active_children` before they reach postorder. Reaching them here
            // means a direct primal output escaped classification; report it
            // rather than silently dropping the gradient.
            _ => {
                return Err(PropagateError::RadUnsupportedNode {
                    node: y,
                    kind: "other-direct-primal",
                });
            }
        }
        Ok(())
    }

    /// Propagates the local cotangent of a unary math node in pure Signal IR.
    ///
    /// This path runs after `rad_unary_math_rule` has classified the operator
    /// as a supported pointwise transpose. It owns only the local algebra:
    /// rebuilding any needed forward output as a Signal expression, asking the
    /// shared `ad_rules` helper for the contribution, then accumulating that
    /// contribution on the single operand. It intentionally has no tape or FIR
    /// state access; temporal/recursive eligibility is decided before this
    /// postorder step.
    fn propagate_unary_math(&mut self, rule: RadUnaryMathRule, x: SigId, y_bar: SigId) {
        let mut b = SigBuilder::new(self.arena);
        let primal = match rule {
            RadUnaryMathRule::Exp => b.exp(x),
            RadUnaryMathRule::Sqrt => b.sqrt(x),
            RadUnaryMathRule::Abs => b.abs(x),
            _ => x,
        };
        let contrib = rad_unary_contribution(&mut b, rule, x, primal, y_bar);
        self.add_adjoint(x, contrib);
    }

    /// Propagates the local cotangents of a binary math node in pure Signal IR.
    ///
    /// The two operands are still Signal values, so this symbolic RAD pass may
    /// rebuild the forward result when a formula needs it. Today that is only
    /// required by `pow`, whose exponent derivative uses the original output.
    /// The backend-neutral `ad_rules` helper returns both operand
    /// contributions; this method is responsible for adding them to the
    /// reverse accumulation map.
    fn propagate_binary_math(&mut self, rule: RadBinaryMathRule, x: SigId, z: SigId, y_bar: SigId) {
        let mut b = SigBuilder::new(self.arena);
        let primal = match rule {
            RadBinaryMathRule::Pow => b.pow(x, z),
            _ => x,
        };
        let (x_contrib, z_contrib) = rad_binary_contributions(&mut b, rule, x, z, primal, y_bar);
        self.add_adjoint(x, x_contrib);
        self.add_adjoint(z, z_contrib);
    }

    fn propagate_binop(&mut self, op: BinOp, x: SigId, z: SigId, y_bar: SigId) {
        let mut b = SigBuilder::new(self.arena);
        if let Some((x_contrib, z_contrib)) =
            rad_binop_contributions(&mut b, rad_binop_rule(op), x, z, y_bar)
        {
            self.add_adjoint(x, x_contrib);
            self.add_adjoint(z, z_contrib);
        }
    }

    /// Drives the three passes and returns the bundled
    /// `[primals…, adjoint(seeds)…]` output list.
    fn run(&mut self, primals: &[SigId], seeds: &[SigId]) -> Result<Vec<SigId>, PropagateError> {
        for &p in primals {
            self.collect_dfs(p)?;
        }

        // Initialize: each primal contributes 1 to the cotangent sum.
        let one = SigBuilder::new(self.arena).real(1.0);
        for &p in primals {
            self.add_adjoint(p, one);
        }

        for i in (0..self.postorder.len()).rev() {
            let y = self.postorder[i];
            if self.seed_set.contains(&y) {
                continue;
            }
            let Some(y_bar) = self.adjoints.get(&y).copied() else {
                // Unreachable in a well-formed graph: every postorder node
                // is initialized with at least one parent contribution
                // before we visit it. Skip defensively.
                continue;
            };
            self.propagate_adjoint(y, y_bar)?;
        }

        let zero = SigBuilder::new(self.arena).real(0.0);
        let mut out = Vec::with_capacity(primals.len() + seeds.len());
        out.extend_from_slice(primals);
        for &s in seeds {
            out.push(self.adjoints.get(&s).copied().unwrap_or(zero));
        }
        Ok(out)
    }
}

fn recursive_rad_unsupported_kind(arena: &TreeArena, sig: SigId) -> &'static str {
    match classify_recursive_projection_rad_mode(arena, sig) {
        Some(RecRadMode::LinearTranspose) => "recursive-linear-transpose",
        Some(RecRadMode::BlockLinearTimeVarying) => "recursive-block-linear-time-varying",
        Some(RecRadMode::BpttRequired) => "recursive-bptt-required",
        None => "recursive-projection",
    }
}

/// One of the recognized unary FFUN families; used by the FFUN arm of
/// `propagate_adjoint` to pick the correct chain rule without re-running
/// the name-matching logic per branch.
enum FFunUnaryKind {
    Tanh,
    Sinh,
    Cosh,
    Atanh,
    Asinh,
    Acosh,
}

fn ffun_unary_kind(arena: &TreeArena, ff: SigId) -> Option<FFunUnaryKind> {
    if ffun_is(arena, ff, &["tanhf", "tanh", "tanhl"]) {
        Some(FFunUnaryKind::Tanh)
    } else if ffun_is(arena, ff, &["sinhf", "sinh", "sinhl"]) {
        Some(FFunUnaryKind::Sinh)
    } else if ffun_is(arena, ff, &["coshf", "cosh", "coshl"]) {
        Some(FFunUnaryKind::Cosh)
    } else if ffun_is(arena, ff, &["atanhf", "atanh", "atanhl"]) {
        Some(FFunUnaryKind::Atanh)
    } else if ffun_is(arena, ff, &["asinhf", "asinh", "asinhl"]) {
        Some(FFunUnaryKind::Asinh)
    } else if ffun_is(arena, ff, &["acoshf", "acosh", "acoshl"]) {
        Some(FFunUnaryKind::Acosh)
    } else {
        None
    }
}

/// Returns `true` when the FFUN descriptor's name (in any precision
/// variant) matches one of the provided targets. Mirrors
/// `forward_ad::ForwardADTransform::ffun_is`.
fn ffun_is(arena: &TreeArena, ff: SigId, targets: &[&str]) -> bool {
    let Some(node) = arena.node(ff) else {
        return false;
    };
    let NodeKind::Tag(tag_id) = node.kind else {
        return false;
    };
    if arena.tag_name(tag_id) != Some("FFUN") {
        return false;
    }
    let [signature, _, _] = node.children.as_slice() else {
        return false;
    };
    let Some(sig_items) = list_to_vec(arena, *signature) else {
        return false;
    };
    let Some(names_node) = sig_items.get(1) else {
        return false;
    };
    let Some(name_ids) = list_to_vec(arena, *names_node) else {
        return false;
    };
    name_ids
        .iter()
        .any(|id| tree_to_str(arena, *id).is_some_and(|n| targets.contains(&n)))
}

/// Set of unary FFUN names whose chain-rule rule is implemented by RAD.
/// Kept aligned with the FAD pass.
const RAD_FFUN_UNARY_NAMES: &[&str] = &[
    "tanhf", "tanh", "tanhl", "sinhf", "sinh", "sinhl", "coshf", "cosh", "coshl", "atanhf",
    "atanh", "atanhl", "asinhf", "asinh", "asinhl", "acoshf", "acosh", "acoshl",
];

fn ffun_unary_supported(arena: &TreeArena, ff: SigId) -> bool {
    ffun_is(arena, ff, RAD_FFUN_UNARY_NAMES)
}

/// Same read-only-table classifier as the FAD pass: a `Waveform` is
/// always read-only, and a `WrTbl(_, _, nil, nil)` is a write-once table
/// with no live writer port and therefore safe to read-differentiate.
fn is_readonly_table_source(arena: &TreeArena, sig: SigId) -> bool {
    match match_sig(arena, sig) {
        SigMatch::Waveform(_) => true,
        SigMatch::WrTbl(_, _, widx, wsig) => arena.is_nil(widx) && arena.is_nil(wsig),
        _ => false,
    }
}

/// Builds the phase-E1 reverse-time adjoint group for one LTI recursive group.
///
/// This is the grouped counterpart of the single-projection bridge below.
/// Public `rad(...)` still needs to discover all primal projections that read
/// the same `DEBRUIJNREC`; once it has them, this helper combines all incoming
/// cotangents by output lane, replaces the transpose scaffold placeholders,
/// and returns one shared `ReverseTimeRec(DEBRUIJNREC(transposed_body))` group.
///
/// Duplicate entries for the same `slot` are accumulated with signal addition.
/// Slots without incoming cotangent receive `0.0`, preserving the recursive
/// group's original arity.
///
/// Source provenance: original Rust RAD phase-E1 design in
/// `porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md`, sections
/// 20.2 through 20.5.
#[allow(dead_code)]
pub(super) fn build_lti_recursive_adjoint_group(
    arena: &mut TreeArena,
    group: SigId,
    slot_cotangents: &[(i32, SigId)],
) -> Result<Option<SigId>, TransposeAdError> {
    if match_de_bruijn_rec(arena, group).is_none() {
        return Ok(None);
    };
    if classify_de_bruijn_rec_rad_mode(arena, group) != Some(RecRadMode::LinearTranspose) {
        return Ok(None);
    }

    let body = match_de_bruijn_rec(arena, group).ok_or(TransposeAdError::NotRecursiveGroup)?;
    let branch_count = list_to_vec(arena, body)
        .ok_or(TransposeAdError::MalformedBody)?
        .len();
    let zero = SigBuilder::new(arena).real(0.0);
    let mut cotangents = vec![zero; branch_count];
    for &(slot, cotangent) in slot_cotangents {
        let slot_index = usize::try_from(slot).map_err(|_| TransposeAdError::SlotOutOfRange)?;
        let Some(existing) = cotangents.get(slot_index).copied() else {
            return Err(TransposeAdError::SlotOutOfRange);
        };
        cotangents[slot_index] = if existing == zero {
            cotangent
        } else {
            SigBuilder::new(arena).add(existing, cotangent)
        };
    }

    let transposed =
        transpose_lti_de_bruijn_rec_with_cotangents(arena, group, cotangents.as_slice())?;
    Ok(Some(SigBuilder::new(arena).reverse_time_rec(transposed)))
}

/// Builds the phase-E1 reverse-time adjoint projection for one LTI recursive
/// primal projection.
///
/// For one projection `Proj(slot, DEBRUIJNREC(body))`, this helper injects
/// `cotangent` into that lane, zero cotangents into the other lanes, wraps the
/// transposed recursive group in `ReverseTimeRec`, and returns
/// `Proj(slot, ReverseTimeRec(transposed_group))`.
///
/// Mapping status: `adapted`, internal phase-E1 scaffold. The public
/// `rad(...)` traversal is intentionally not switched to it yet because full
/// E1 still has to group all projections of the same primal recursion and route
/// parameter/seed gradient contributions.
#[allow(dead_code)]
pub(super) fn build_lti_recursive_adjoint_projection(
    arena: &mut TreeArena,
    primal_projection: SigId,
    cotangent: SigId,
) -> Result<Option<SigId>, TransposeAdError> {
    let SigMatch::Proj(slot, group) = match_sig(arena, primal_projection) else {
        return Ok(None);
    };
    if classify_recursive_projection_rad_mode(arena, primal_projection)
        != Some(RecRadMode::LinearTranspose)
    {
        return Ok(None);
    }

    let Some(reverse_group) =
        build_lti_recursive_adjoint_group(arena, group, &[(slot, cotangent)])?
    else {
        return Ok(None);
    };
    Ok(Some(SigBuilder::new(arena).proj(slot, reverse_group)))
}

/// Groups recursive projection cotangents and returns adjoint projections.
///
/// This is the interface the public reverse sweep needs before replacing the
/// `recursive-linear-transpose` diagnostic: every pair is a primal recursive
/// projection and the cotangent accumulated for that projection. Eligible
/// projections that read the same LTI `DEBRUIJNREC` are lowered through one
/// shared `ReverseTimeRec` group, and the result preserves the input order as
/// `(primal_projection, adjoint_projection)` pairs. Non-projection or
/// non-eligible entries are ignored so callers can feed a mixed frontier and
/// keep the existing diagnostics for unsupported recursive families.
#[allow(dead_code)]
pub(super) fn build_lti_recursive_adjoint_projections(
    arena: &mut TreeArena,
    projection_cotangents: &[(SigId, SigId)],
) -> Result<Vec<(SigId, SigId)>, TransposeAdError> {
    struct PendingGroup {
        group: SigId,
        slot_cotangents: Vec<(i32, SigId)>,
        projections: Vec<(SigId, i32)>,
    }

    let mut groups = Vec::<PendingGroup>::new();
    let mut group_indices = AHashMap::<SigId, usize>::new();
    for &(projection, cotangent) in projection_cotangents {
        let SigMatch::Proj(slot, group) = match_sig(arena, projection) else {
            continue;
        };
        if classify_recursive_projection_rad_mode(arena, projection)
            != Some(RecRadMode::LinearTranspose)
        {
            continue;
        }
        let index = match group_indices.get(&group).copied() {
            Some(index) => index,
            None => {
                let index = groups.len();
                group_indices.insert(group, index);
                groups.push(PendingGroup {
                    group,
                    slot_cotangents: Vec::new(),
                    projections: Vec::new(),
                });
                index
            }
        };
        groups[index].slot_cotangents.push((slot, cotangent));
        groups[index].projections.push((projection, slot));
    }

    let mut out = Vec::new();
    for pending in groups {
        let Some(reverse_group) =
            build_lti_recursive_adjoint_group(arena, pending.group, &pending.slot_cotangents)?
        else {
            continue;
        };
        for (projection, slot) in pending.projections {
            let adjoint = SigBuilder::new(arena).proj(slot, reverse_group);
            out.push((projection, adjoint));
        }
    }
    Ok(out)
}

/// Block-mode fallback for `rad(expr, seeds)` when the symbolic sweep
/// encounters a temporal or recursive obstacle.
///
/// Builds a `SigBlockReverseAD` carrier that encodes a TBPTT(BS, BS)
/// non-overlapping backward pass.  The implicit per-output cotangent is `1.0`,
/// matching the symbolic sweep contract.
///
/// This function does not decide the final loop schedule.  It merely produces
/// projections from one carrier:
///
/// - primal projections can be lowered like normal body signals in the causal
///   forward loop;
/// - gradient projections force `signal_fir` to emit the block adjoint sweep
///   at the point where the projection is lowered.
///
/// If the gradient projection is itself a public output, `signal_fir` classifies
/// that output as reverse-time and emits a separate backward loop over the
/// block.  If the gradient projection is consumed inside another forward-time
/// expression (for example a recursive learning update), there may be no public
/// reverse-time output at all; the sweep is emitted inline in the surrounding
/// forward loop slice.
///
/// # Output layout
///
/// ```text
/// [ Proj(0,   carrier), …, Proj(M-1,   carrier),   // M primal outputs
///   Proj(M,   carrier), …, Proj(M+N-1, carrier) ]  // N seed adjoints
/// ```
///
/// where `M = primals.len()` and `N = seeds.len()`.  Slots `0..M` carry the
/// primal body values; slots `M..M+N` carry the per-seed gradients, matching
/// the [`SigBuilder::block_reverse_ad`] slot contract.
fn build_block_reverse_ad(arena: &mut TreeArena, primals: &[SigId], seeds: &[SigId]) -> Vec<SigId> {
    let one = SigBuilder::new(arena).real(1.0);
    let cotangents: Vec<SigId> = primals.iter().map(|_| one).collect();
    let carrier = SigBuilder::new(arena).block_reverse_ad(
        primals,
        seeds,
        &cotangents,
        BlockRevPolicy::TapeFull,
    );
    let m = primals.len();
    let n = seeds.len();
    (0..m + n)
        .map(|slot| SigBuilder::new(arena).proj(slot as i32, carrier))
        .collect()
}

/// Public entry point for `rad(expr, seeds)` propagation.
///
/// # Dispatch order
///
/// 1. **Symbolic sweep** ([`ReverseADTransform`]) — exact feed-forward reverse
///    mode only. The legacy `ReverseTimeRec` LTI/IIR fast path is dormant; see
///    `porting/rad-disable-reverse-time-rec-fast-path-plan-2026-05-10-en.md`.
/// 2. **Block fallback** ([`build_block_reverse_ad`]) — engaged when the
///    symbolic sweep raises [`PropagateError::RadUnsupportedNode`] with a
///    temporal or recursive kind (see table below).  The fallback emits a
///    `SigBlockReverseAD` carrier lowered by the backend using TBPTT(BS, BS)
///    semantics — no adjoint state crosses block boundaries.
///
/// | kind that triggers fallback | origin |
/// |---|---|
/// | `"delay-or-prefix"` | `Delay1`, `Delay(_, _)`, `Prefix` in the primal |
/// | `"recursive-bptt-required"` | nonlinear recursive body |
/// | `"recursive-block-linear-time-varying"` | LTV recursive coefficient |
/// | `"recursive-projection"` | raw `Proj` / `Rec` with no classifier match |
/// | `"iir-state-space"` | structured `SigIIR` carrier |
///
/// All other errors — arity mismatches, malformed foreign functions, writable
/// tables, soundfile accessors — propagate unchanged so the user receives a
/// targeted diagnostic.
///
/// The fallback boundary is intentionally semantic.  Tape layout, enforcement
/// that actual tape stores are already `real_ty` after `signalPromotion`,
/// persistent `fConst*` storage for constants reused by generated sweeps, and
/// split-vs-inline loop scheduling are all FIR-lowering responsibilities.
pub(super) fn generate_rad_signals(
    arena: &mut TreeArena,
    primals: &[SigId],
    seeds: &[SigId],
) -> Result<Vec<SigId>, PropagateError> {
    if seeds.is_empty() {
        // Mirrors the FAD identity short-circuit: with no seeds the bundle
        // collapses to the primal outputs.
        return Ok(primals.to_vec());
    }
    let mut transform = ReverseADTransform::new(arena, seeds);
    let result = match transform.run(primals, seeds) {
        Ok(r) => r,
        Err(PropagateError::RadUnsupportedNode {
            kind:
                "delay-or-prefix"
                | "recursive-bptt-required"
                | "recursive-block-linear-time-varying"
                | "recursive-projection"
                | "iir-state-space",
            ..
        }) => build_block_reverse_ad(arena, primals, seeds),
        Err(e) => return Err(e),
    };
    for &sig in &result {
        if is_de_bruijn_closed(arena, sig) {
            check_de_bruijn_coherence(arena, sig).map_err(|e| {
                PropagateError::DeBruijnCoherence {
                    pass: "RAD",
                    detail: e.to_string(),
                }
            })?;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{
        build_lti_recursive_adjoint_group, build_lti_recursive_adjoint_projection,
        build_lti_recursive_adjoint_projections, generate_rad_signals,
    };
    use signals::{SigBuilder, SigId, SigMatch, match_sig};
    use tlib::{
        TreeArena, de_bruijn_rec, de_bruijn_ref, list_to_vec, match_de_bruijn_rec, vec_to_list,
    };

    fn rec_group(arena: &mut TreeArena, branches: &[SigId]) -> SigId {
        let body = vec_to_list(arena, branches);
        de_bruijn_rec(arena, body)
    }

    fn dump_contains_input(arena: &TreeArena, sig: SigId) -> bool {
        match match_sig(arena, sig) {
            SigMatch::Input(_) => true,
            _ => arena.node(sig).is_some_and(|node| {
                node.children
                    .as_slice()
                    .iter()
                    .copied()
                    .any(|child| dump_contains_input(arena, child))
            }),
        }
    }

    fn dump_contains_real(arena: &TreeArena, sig: SigId, expected: f64) -> bool {
        match match_sig(arena, sig) {
            SigMatch::Real(value) => (value - expected).abs() < f64::EPSILON,
            _ => arena.node(sig).is_some_and(|node| {
                node.children
                    .as_slice()
                    .iter()
                    .copied()
                    .any(|child| dump_contains_real(arena, child, expected))
            }),
        }
    }

    fn assert_block_reverse_ad_bundle(arena: &TreeArena, out: &[SigId], primal_count: usize) {
        assert!(
            out.len() > primal_count,
            "RAD bundle should include primal and seed-gradient lanes"
        );
        let SigMatch::Proj(0, carrier) = match_sig(arena, out[0]) else {
            panic!("primal lane should project from BlockReverseAD carrier");
        };
        let SigMatch::BlockReverseAD {
            primal_count: actual_primal_count,
            ..
        } = match_sig(arena, carrier)
        else {
            panic!("carrier should be BlockReverseAD");
        };
        assert_eq!(actual_primal_count, primal_count as i32);
        for (slot, sig) in out.iter().copied().enumerate() {
            let SigMatch::Proj(actual_slot, actual_carrier) = match_sig(arena, sig) else {
                panic!("RAD output lane {slot} should be Proj(slot, BlockReverseAD)");
            };
            assert_eq!(actual_slot, slot as i32);
            assert_eq!(actual_carrier, carrier);
        }
    }

    #[test]
    fn lti_recursive_projection_builds_reverse_time_adjoint_projection() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let group = {
            let mut b = SigBuilder::new(&mut arena);
            let input = b.input(0);
            let prev = b.proj(0, ref1);
            let half = b.real(0.5);
            let feedback = b.mul(half, prev);
            let branch = b.add(input, feedback);
            rec_group(&mut arena, &[branch])
        };
        let primal = SigBuilder::new(&mut arena).proj(0, group);
        let cotangent = SigBuilder::new(&mut arena).real(1.0);

        let adjoint = build_lti_recursive_adjoint_projection(&mut arena, primal, cotangent)
            .expect("LTI transpose bridge should build")
            .expect("projection should be eligible");

        let SigMatch::Proj(0, reverse_group) = match_sig(&arena, adjoint) else {
            panic!("adjoint should project from a reverse-time group");
        };
        let SigMatch::ReverseTimeRec(transposed_group) = match_sig(&arena, reverse_group) else {
            panic!("adjoint group should be ReverseTimeRec");
        };
        let transposed_body =
            match_de_bruijn_rec(&arena, transposed_group).expect("transposed recursive group");
        let branches = list_to_vec(&arena, transposed_body).expect("transposed body list");
        assert_eq!(branches.len(), 1);
        assert!(
            !dump_contains_input(&arena, branches[0]),
            "reverse_ad bridge must replace scaffold input placeholders"
        );
        assert!(
            dump_contains_real(&arena, branches[0], 1.0),
            "projection cotangent should drive the transposed branch"
        );
        assert!(match_de_bruijn_rec(&arena, group).is_some());
    }

    #[test]
    fn lti_recursive_adjoint_group_combines_slot_cotangents() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let group = {
            let mut b = SigBuilder::new(&mut arena);
            let input0 = b.input(0);
            let input1 = b.input(1);
            let prev0 = b.proj(0, ref1);
            let prev1 = b.proj(1, ref1);
            let half = b.real(0.5);
            let quarter = b.real(0.25);
            let feedback0 = b.mul(half, prev0);
            let feedback1 = b.mul(quarter, prev1);
            let branch0 = b.add(input0, feedback0);
            let branch1 = b.add(input1, feedback1);
            rec_group(&mut arena, &[branch0, branch1])
        };
        let c0 = SigBuilder::new(&mut arena).real(2.0);
        let c1 = SigBuilder::new(&mut arena).real(7.0);
        let c0_extra = SigBuilder::new(&mut arena).real(3.0);

        let reverse_group = build_lti_recursive_adjoint_group(
            &mut arena,
            group,
            &[(0, c0), (1, c1), (0, c0_extra)],
        )
        .expect("grouped LTI transpose should build")
        .expect("group should be eligible");

        let SigMatch::ReverseTimeRec(transposed_group) = match_sig(&arena, reverse_group) else {
            panic!("grouped adjoint should be ReverseTimeRec");
        };
        let transposed_body =
            match_de_bruijn_rec(&arena, transposed_group).expect("transposed recursive group");
        let branches = list_to_vec(&arena, transposed_body).expect("transposed body list");
        assert_eq!(branches.len(), 2);
        assert!(
            !dump_contains_input(&arena, branches[0]) && !dump_contains_input(&arena, branches[1]),
            "grouped bridge must replace every scaffold input placeholder"
        );
        assert!(dump_contains_real(&arena, branches[0], 2.0));
        assert!(dump_contains_real(&arena, branches[0], 3.0));
        assert!(dump_contains_real(&arena, branches[1], 7.0));
    }

    #[test]
    fn lti_recursive_projection_frontier_shares_reverse_group() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let group = {
            let mut b = SigBuilder::new(&mut arena);
            let input0 = b.input(0);
            let input1 = b.input(1);
            let prev0 = b.proj(0, ref1);
            let prev1 = b.proj(1, ref1);
            let half = b.real(0.5);
            let quarter = b.real(0.25);
            let feedback0 = b.mul(half, prev0);
            let feedback1 = b.mul(quarter, prev1);
            let branch0 = b.add(input0, feedback0);
            let branch1 = b.add(input1, feedback1);
            rec_group(&mut arena, &[branch0, branch1])
        };
        let projection0 = SigBuilder::new(&mut arena).proj(0, group);
        let projection1 = SigBuilder::new(&mut arena).proj(1, group);
        let c0 = SigBuilder::new(&mut arena).real(11.0);
        let c1 = SigBuilder::new(&mut arena).real(13.0);

        let adjoints = build_lti_recursive_adjoint_projections(
            &mut arena,
            &[(projection0, c0), (projection1, c1)],
        )
        .expect("frontier should lower");

        assert_eq!(adjoints.len(), 2);
        assert_eq!(adjoints[0].0, projection0);
        assert_eq!(adjoints[1].0, projection1);
        let SigMatch::Proj(0, reverse_group0) = match_sig(&arena, adjoints[0].1) else {
            panic!("first adjoint should project slot 0");
        };
        let SigMatch::Proj(1, reverse_group1) = match_sig(&arena, adjoints[1].1) else {
            panic!("second adjoint should project slot 1");
        };
        assert_eq!(
            reverse_group0, reverse_group1,
            "frontier projections from the same recursion must share one reverse-time group"
        );
        let SigMatch::ReverseTimeRec(transposed_group) = match_sig(&arena, reverse_group0) else {
            panic!("shared group should be ReverseTimeRec");
        };
        let transposed_body =
            match_de_bruijn_rec(&arena, transposed_group).expect("transposed recursive group");
        let branches = list_to_vec(&arena, transposed_body).expect("transposed body list");
        assert!(dump_contains_real(&arena, branches[0], 11.0));
        assert!(dump_contains_real(&arena, branches[1], 13.0));
    }

    #[test]
    fn active_drive_seed_inside_lti_recursion_falls_back_to_block_reverse_ad() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let input = SigBuilder::new(&mut arena).input(0);
        let group = {
            let mut b = SigBuilder::new(&mut arena);
            let prev = b.proj(0, ref1);
            let half = b.real(0.5);
            let feedback = b.mul(half, prev);
            let branch = b.add(input, feedback);
            rec_group(&mut arena, &[branch])
        };
        let primal = SigBuilder::new(&mut arena).proj(0, group);

        let out = generate_rad_signals(&mut arena, &[primal], &[input])
            .expect("active recursive drive input should fall back to BlockReverseAD");
        assert_eq!(out.len(), 2);
        assert_block_reverse_ad_bundle(&arena, &out, 1);
    }

    #[test]
    fn active_feedback_coefficient_seed_falls_back_to_block_reverse_ad() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let drive = SigBuilder::new(&mut arena).input(0);
        let coeff = SigBuilder::new(&mut arena).real(0.5);
        let group = {
            let mut b = SigBuilder::new(&mut arena);
            let state = b.proj(0, ref1);
            let prev = b.delay1(state);
            let feedback = b.mul(coeff, prev);
            let branch = b.add(drive, feedback);
            rec_group(&mut arena, &[branch])
        };
        let primal = SigBuilder::new(&mut arena).proj(0, group);

        let out = generate_rad_signals(&mut arena, &[primal], &[coeff])
            .expect("active LTI feedback coefficient should fall back to BlockReverseAD");
        assert_eq!(out.len(), 2);
        assert_block_reverse_ad_bundle(&arena, &out, 1);
    }

    #[test]
    fn iir_carrier_input_seed_falls_back_to_block_reverse_ad() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let state = b.input(9);
        let drive = b.input(0);
        let p = b.real(-0.25);
        let q = b.real(-0.125);
        let iir = b.iir(&[state, drive, p, q]);

        let out = generate_rad_signals(&mut arena, &[iir], &[drive])
            .expect("IIR input seed should fall back to BlockReverseAD");
        assert_eq!(out.len(), 2);
        assert_block_reverse_ad_bundle(&arena, &out, 1);
    }

    #[test]
    fn iir_carrier_feedback_seed_falls_back_to_block_reverse_ad() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let state = b.input(9);
        let drive = b.input(0);
        let p = b.real(-0.25);
        let q = b.real(-0.125);
        let iir = b.iir(&[state, drive, p, q]);

        let out = generate_rad_signals(&mut arena, &[iir], &[p])
            .expect("IIR feedback seed should fall back to BlockReverseAD");
        assert_eq!(out.len(), 2);
        assert_block_reverse_ad_bundle(&arena, &out, 1);
    }

    #[test]
    fn iir_carrier_third_order_falls_back_to_block_reverse_ad() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let state = b.input(9);
        let drive = b.input(0);
        let c0 = b.real(0.1);
        let c1 = b.real(0.2);
        let c2 = b.real(0.3);
        let iir = b.iir(&[state, drive, c0, c1, c2]);

        let out = generate_rad_signals(&mut arena, &[iir], &[c0])
            .expect("direct third-order IIR should fall back to BlockReverseAD");
        assert_eq!(out.len(), 2);
        assert_block_reverse_ad_bundle(&arena, &out, 1);
    }
}
