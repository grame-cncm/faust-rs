//! Clock-environment inference over the prepared signal forest (roadmap P1.1).
//!
//! # Source provenance (C++)
//! - `compiler/signals/clkEnvInference.cpp` (`ClkEnvInference`, 638 lines,
//!   branch `master-dev-ocpp-od-fir-2-FIR19`, commit `8eebea429`)
//! - rules document `ClockEnvironmentInferenceRulesV2.md` as summarized in
//!   `porting/ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md`
//!   §3.6 / §4.1.
//!
//! # What this module computes
//! `C⟦sig⟧ = c` — "signal `sig` is computed in clock domain `c`" — for every
//! signal reachable from the prepared outputs. This is the information the
//! hierarchical dependency graph ([`crate::hgraph`]) and, later, guarded-block
//! code generation need.
//!
//! The system is a deliberately simplified clock calculus (plan §4.1): no
//! clock polymorphism, no boolean-clock unification. Domains form a **finite
//! tree rooted at `nil`** (the audio rate, bottom element), so clock checking
//! reduces to order checking on the [`ClockDomainTable`] parent chains.
//!
//! # Rules (C++ names kept for parity audits)
//! - `R_PROJ`: for `proj(i, W)`, seed the group hypothesis, infer each
//!   definition, result = `max` of the definitions' envs.
//! - `R_CLOCKED(c, s)`: require `C⟦s⟧ ⊆ c`; result `c` (re-clocking moves a
//!   signal *deeper*, never shallower).
//! - `R_CD` (OD/US/DS): first child must be `Clocked(c_inner, h)`; require
//!   `C⟦h⟧ ⊆ parent(c_inner)` (the clock is computed outside); every other
//!   child must live *exactly* in `c_inner` (exception: literal `0`);
//!   result = `parent(c_inner)`.
//! - `R_SEQ(x, y)`: require `C⟦x⟧ ⊆ C⟦y⟧`; result `C⟦x⟧`.
//! - `R_COMPOSITE` (default, incl. tables): `max` over subsignals.
//!
//! # Fixed point
//! Recursive groups make the bottom-up synthesis circular. [`find_fixpoint`]
//! runs a Jacobi-style Kleene iteration: the hypothesis `H : group → env`
//! starts at `nil` for every group collected by [`collect_rec_groups`], and
//! each round recomputes every group against the *previous* hypothesis
//! (order-independent, deterministic). Monotonicity under `max` plus the
//! finite domain height bound termination; `MAX_ITERATIONS` is pure safety.
//!
//! # Adaptation status
//! - C++ attaches results as a tree property (`CLKENVPROPERTY`); Rust returns
//!   a side map `SigId → ClkEnv` (house style, no tree mutation).
//! - C++ walks the `(parent, slotenv, path, box, inputs...)` cons tuple;
//!   Rust decodes the opaque `SIGCLOCKENV` token (P0.2) against the
//!   propagation-owned [`ClockDomainTable`].
//! - Runs on the **prepared** forest (symbolic `SYMREC`/`SYMREF` recursion);
//!   de Bruijn recursion is not supported here, mirroring C++ where inference
//!   runs after `deBruijn2Sym`.

use std::fmt;

use ahash::AHashMap;
use propagate::{ClockDomainId, ClockDomainTable};
use signals::{SigId, SigMatch, match_sig};
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref};

/// One clock environment: `None` is `nil` (top-level audio rate, bottom of
/// the domain tree), `Some(id)` is one clocked-wrapper instance domain.
pub type ClkEnv = Option<ClockDomainId>;

/// Safety bound for the Kleene iteration (C++ `MAX_ITERATIONS = 1000`;
/// real programs converge in ≤ 2-3 rounds).
const MAX_ITERATIONS: usize = 1000;

/// Structured inference errors.
///
/// Every error names the offending signal (and the domains involved) so the
/// compiler facade can enrich the diagnostic (roadmap P1.1: "structured error
/// naming the two incomparable domains *and* the offending signal").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClkEnvError {
    /// `max_clk_env` was asked to join two domains on different branches of
    /// the domain tree — sibling domains may not exchange un-annotated
    /// signals (plan §4.1 scoping rule).
    Incomparable {
        sig: SigId,
        left: ClkEnv,
        right: ClkEnv,
    },
    /// `R_CLOCKED`: the wrapped signal lives deeper than its annotation.
    ClockedViolation {
        sig: SigId,
        annotation: ClkEnv,
        inner: ClkEnv,
    },
    /// `R_SEQ`: the sequenced block does not dominate the read value.
    SeqViolation {
        sig: SigId,
        left: ClkEnv,
        right: ClkEnv,
    },
    /// `R_CD`: the wrapper clock must be computed in the parent domain.
    ClockComputedInside {
        sig: SigId,
        expected: ClkEnv,
        got: ClkEnv,
    },
    /// `R_CD`: a wrapper output child must live exactly in the inner domain.
    WrapperChildOutsideDomain {
        sig: SigId,
        child: SigId,
        expected: ClkEnv,
        got: ClkEnv,
    },
    /// A `Clocked` env child or `SIGCLOCKENV` token is malformed, or a token
    /// id has no entry in the domain table.
    MalformedClockEnv { sig: SigId },
    /// Structural error (malformed list, de Bruijn recursion, unknown node).
    Malformed { sig: SigId, detail: String },
    /// The Kleene iteration did not stabilize within [`MAX_ITERATIONS`].
    FixpointDiverged { iterations: usize },
}

fn env_name(env: ClkEnv) -> String {
    match env {
        None => "nil (audio rate)".to_owned(),
        Some(id) => format!("domain #{}", id.as_u32()),
    }
}

impl fmt::Display for ClkEnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomparable { sig, left, right } => write!(
                f,
                "incomparable clock environments at signal {}: {} vs {} \
                 (sibling clock domains may not exchange un-annotated signals)",
                sig.as_u32(),
                env_name(*left),
                env_name(*right)
            ),
            Self::ClockedViolation {
                sig,
                annotation,
                inner,
            } => write!(
                f,
                "clocked annotation violated at signal {}: wrapped signal lives in {} \
                 which is not visible from annotation {}",
                sig.as_u32(),
                env_name(*inner),
                env_name(*annotation)
            ),
            Self::SeqViolation { sig, left, right } => write!(
                f,
                "seq ordering violated at signal {}: block env {} must be an ancestor \
                 of value env {}",
                sig.as_u32(),
                env_name(*left),
                env_name(*right)
            ),
            Self::ClockComputedInside { sig, expected, got } => write!(
                f,
                "wrapper clock at signal {} must be computed in the enclosing domain {} \
                 but lives in {}",
                sig.as_u32(),
                env_name(*expected),
                env_name(*got)
            ),
            Self::WrapperChildOutsideDomain {
                sig,
                child,
                expected,
                got,
            } => write!(
                f,
                "wrapper output {} (child {}) must live exactly in {} but lives in {}",
                sig.as_u32(),
                child.as_u32(),
                env_name(*expected),
                env_name(*got)
            ),
            Self::MalformedClockEnv { sig } => write!(
                f,
                "malformed clock environment annotation at signal {}",
                sig.as_u32()
            ),
            Self::Malformed { sig, detail } => write!(
                f,
                "malformed signal {} during clock inference: {detail}",
                sig.as_u32()
            ),
            Self::FixpointDiverged { iterations } => write!(
                f,
                "clock-environment fixpoint did not stabilize after {iterations} iterations"
            ),
        }
    }
}

impl std::error::Error for ClkEnvError {}

/// Result of one full inference run.
#[derive(Debug)]
pub struct ClkEnvMap {
    /// `C⟦sig⟧` for every signal node reachable from the outputs (opaque
    /// clock-env tokens excluded — they are annotations, not signals).
    envs: AHashMap<SigId, ClkEnv>,
    /// Converged group hypothesis `H : recursion var → env`.
    groups: AHashMap<SigId, ClkEnv>,
}

impl Default for ClkEnvMap {
    fn default() -> Self {
        Self {
            envs: AHashMap::new(),
            groups: AHashMap::new(),
        }
    }
}

impl ClkEnvMap {
    /// Returns `C⟦sig⟧` when `sig` was reachable during inference.
    #[must_use]
    pub fn env(&self, sig: SigId) -> Option<ClkEnv> {
        self.envs.get(&sig).copied()
    }

    /// Returns the converged env of one recursive group by its variable.
    #[must_use]
    pub fn group_env(&self, var: SigId) -> Option<ClkEnv> {
        self.groups.get(&var).copied()
    }

    /// Read-only view of the full side map.
    #[must_use]
    pub fn envs(&self) -> &AHashMap<SigId, ClkEnv> {
        &self.envs
    }
}

/// `isAncestorClkEnv(a, b)` — `a ⊆ b`: `a` is an ancestor of (or equal to)
/// `b` in the domain tree. `nil ⊆ c` for every `c`. O(h) per query.
#[must_use]
pub fn is_ancestor_clk_env(domains: &ClockDomainTable, a: ClkEnv, b: ClkEnv) -> bool {
    let Some(a_id) = a else {
        return true; // nil is the bottom element
    };
    let mut cursor = b;
    while let Some(cur_id) = cursor {
        if cur_id == a_id {
            return true;
        }
        cursor = domains.get(cur_id).and_then(|d| d.parent);
    }
    false
}

/// `maxClkEnv{c1, c2}` — join restricted to chains: the deeper of two
/// *comparable* domains; incomparable domains are the scoping error of the
/// system (plan §4.1).
pub fn max_clk_env(
    domains: &ClockDomainTable,
    sig: SigId,
    left: ClkEnv,
    right: ClkEnv,
) -> Result<ClkEnv, ClkEnvError> {
    if is_ancestor_clk_env(domains, left, right) {
        return Ok(right);
    }
    if is_ancestor_clk_env(domains, right, left) {
        return Ok(left);
    }
    Err(ClkEnvError::Incomparable { sig, left, right })
}

/// One collected symbolic recursion group.
#[derive(Clone, Debug)]
struct RecGroup {
    /// Recursion variable (`SYMREC` binder identity).
    var: SigId,
    /// Definition signals, in slot order.
    defs: Vec<SigId>,
}

/// Collects every `SYMREC` group reachable from `outputs`, in deterministic
/// discovery order (C++ `collectRecGroups`).
fn collect_rec_groups(arena: &TreeArena, outputs: &[SigId]) -> Result<Vec<RecGroup>, ClkEnvError> {
    let mut visited: ahash::AHashSet<SigId> = ahash::AHashSet::new();
    let mut groups = Vec::new();

    fn walk(
        arena: &TreeArena,
        sig: SigId,
        visited: &mut ahash::AHashSet<SigId>,
        groups: &mut Vec<RecGroup>,
    ) -> Result<(), ClkEnvError> {
        if !visited.insert(sig) {
            return Ok(());
        }
        if arena.is_nil(sig) {
            return Ok(());
        }
        if let Some((var, body_list)) = match_sym_rec(arena, sig) {
            let defs = list_to_vec(arena, body_list).ok_or_else(|| ClkEnvError::Malformed {
                sig,
                detail: "malformed SYMREC body list".to_owned(),
            })?;
            groups.push(RecGroup {
                var,
                defs: defs.clone(),
            });
            for def in defs {
                walk(arena, def, visited, groups)?;
            }
            return Ok(());
        }
        if match_sym_ref(arena, sig).is_some() {
            return Ok(());
        }
        // Never descend into the opaque env child of `Clocked`.
        if let SigMatch::Clocked(_, y) = match_sig(arena, sig) {
            return walk(arena, y, visited, groups);
        }
        let Some(node) = arena.node(sig) else {
            return Ok(());
        };
        for &child in node.children.as_slice() {
            if arena.is_list(child) {
                let items = list_to_vec(arena, child).ok_or_else(|| ClkEnvError::Malformed {
                    sig,
                    detail: "malformed child list".to_owned(),
                })?;
                for item in items {
                    walk(arena, item, visited, groups)?;
                }
            } else {
                walk(arena, child, visited, groups)?;
            }
        }
        Ok(())
    }

    for &out in outputs {
        walk(arena, out, &mut visited, &mut groups)?;
    }
    Ok(groups)
}

/// One inference sweep (`inferClkEnvWithHypothesis`) with a per-sweep memo
/// cache and a fixed group hypothesis `H`.
struct Inference<'a> {
    arena: &'a TreeArena,
    domains: &'a ClockDomainTable,
    /// Per-sweep memo (`M` in the plan) — reset between Kleene rounds.
    cache: AHashMap<SigId, ClkEnv>,
    /// Group hypothesis `H : recursion var → env`.
    hypothesis: &'a AHashMap<SigId, ClkEnv>,
}

impl<'a> Inference<'a> {
    fn new(
        arena: &'a TreeArena,
        domains: &'a ClockDomainTable,
        hypothesis: &'a AHashMap<SigId, ClkEnv>,
    ) -> Self {
        Self {
            arena,
            domains,
            cache: AHashMap::new(),
            hypothesis,
        }
    }

    /// Decodes the opaque env child of `Clocked(env, y)`.
    fn decode_env(&self, sig: SigId, env_child: SigId) -> Result<ClkEnv, ClkEnvError> {
        if self.arena.is_nil(env_child) {
            return Ok(None);
        }
        if let SigMatch::ClockEnvToken(id) = match_sig(self.arena, env_child) {
            let id = ClockDomainId::from_u32(id);
            if self.domains.get(id).is_none() {
                return Err(ClkEnvError::MalformedClockEnv { sig });
            }
            return Ok(Some(id));
        }
        Err(ClkEnvError::MalformedClockEnv { sig })
    }

    fn max(&self, sig: SigId, left: ClkEnv, right: ClkEnv) -> Result<ClkEnv, ClkEnvError> {
        max_clk_env(self.domains, sig, left, right)
    }

    /// `R_COMPOSITE`: `max` over the provided children.
    fn composite(&mut self, sig: SigId, children: &[SigId]) -> Result<ClkEnv, ClkEnvError> {
        let mut acc = None;
        for &child in children {
            let child_env = self.infer(child)?;
            acc = self.max(sig, acc, child_env)?;
        }
        Ok(acc)
    }

    fn infer(&mut self, sig: SigId) -> Result<ClkEnv, ClkEnvError> {
        if let Some(env) = self.cache.get(&sig) {
            return Ok(*env);
        }
        let env = self.infer_uncached(sig)?;
        self.cache.insert(sig, env);
        Ok(env)
    }

    fn infer_uncached(&mut self, sig: SigId) -> Result<ClkEnv, ClkEnvError> {
        // Symbolic recursion forms first (they are not `SigMatch` shapes).
        if let Some((var, body_list)) = match_sym_rec(self.arena, sig) {
            // `R_PROJ` core: seed the projections of this group with the
            // hypothesis, infer each definition, join.
            let defs =
                list_to_vec(self.arena, body_list).ok_or_else(|| ClkEnvError::Malformed {
                    sig,
                    detail: "malformed SYMREC body list".to_owned(),
                })?;
            let seed = self.hypothesis.get(&var).copied().unwrap_or(None);
            // Seed the group node itself so back-edges through `Proj(i, ref)`
            // resolve to the hypothesis while the definitions are inferred.
            self.cache.insert(sig, seed);
            let mut acc = seed;
            for def in defs {
                let def_env = self.infer(def)?;
                acc = self.max(sig, acc, def_env)?;
            }
            return Ok(acc);
        }
        if let Some(var) = match_sym_ref(self.arena, sig) {
            return Ok(self.hypothesis.get(&var).copied().unwrap_or(None));
        }
        if tlib::match_de_bruijn_rec(self.arena, sig).is_some()
            || tlib::match_de_bruijn_ref(self.arena, sig).is_some()
        {
            return Err(ClkEnvError::Malformed {
                sig,
                detail: "clock inference runs on symbolic recursion only (post de_bruijn_to_sym)"
                    .to_owned(),
            });
        }

        match match_sig(self.arena, sig) {
            // Leaves: audio rate (bottom).
            SigMatch::Int(_)
            | SigMatch::Real(_)
            | SigMatch::Input(_)
            | SigMatch::Button(_)
            | SigMatch::Checkbox(_)
            | SigMatch::VSlider(_)
            | SigMatch::HSlider(_)
            | SigMatch::NumEntry(_)
            | SigMatch::Soundfile(_) => Ok(None),

            // C++ reaches waveform branches through the generic composite
            // rule. Visiting every table element makes the annotation total
            // for Hgraph and retains the generic deepest-domain join.
            SigMatch::Waveform(children) => {
                let mut result = None;
                for &child in children {
                    let child_env = self.infer(child)?;
                    result = self.max(sig, result, child_env)?;
                }
                Ok(result)
            }

            // `R_CLOCKED(c, s)`: require `C⟦s⟧ ⊆ c`; result `c`.
            SigMatch::Clocked(env_child, inner) => {
                let annotation = self.decode_env(sig, env_child)?;
                let inner_env = self.infer(inner)?;
                if !is_ancestor_clk_env(self.domains, inner_env, annotation) {
                    return Err(ClkEnvError::ClockedViolation {
                        sig,
                        annotation,
                        inner: inner_env,
                    });
                }
                Ok(annotation)
            }

            // A token in signal position is structurally illegal.
            SigMatch::ClockEnvToken(_) => Err(ClkEnvError::Malformed {
                sig,
                detail: "clock-env token reached signal position".to_owned(),
            }),

            // `R_CD` (OD/US/DS).
            SigMatch::OnDemand(children)
            | SigMatch::Upsampling(children)
            | SigMatch::Downsampling(children) => {
                let children = children.to_vec();
                let Some((&first, outputs)) = children.split_first() else {
                    return Err(ClkEnvError::Malformed {
                        sig,
                        detail: "clocked wrapper without children".to_owned(),
                    });
                };
                let SigMatch::Clocked(env_child, clock) = match_sig(self.arena, first) else {
                    return Err(ClkEnvError::Malformed {
                        sig,
                        detail: "clocked wrapper first child must be Clocked(env, clock)"
                            .to_owned(),
                    });
                };
                let inner = self.decode_env(sig, env_child)?;
                let Some(inner_id) = inner else {
                    return Err(ClkEnvError::MalformedClockEnv { sig });
                };
                let outer = self
                    .domains
                    .get(inner_id)
                    .ok_or(ClkEnvError::MalformedClockEnv { sig })?
                    .parent;
                // The clock is computed outside: `C⟦h⟧ ⊆ parent(c_inner)`.
                let clock_env = self.infer(clock)?;
                if !is_ancestor_clk_env(self.domains, clock_env, outer) {
                    return Err(ClkEnvError::ClockComputedInside {
                        sig,
                        expected: outer,
                        got: clock_env,
                    });
                }
                // Mark the first child as belonging to the inner domain
                // (it is `Clocked(c_inner, clock)` by construction).
                self.cache.insert(first, inner);
                // Every other child lives exactly in `c_inner`
                // (exception: literal 0).
                for &child in outputs {
                    let child_env = self.infer(child)?;
                    if child_env != inner && !self.is_literal_zero(child) {
                        return Err(ClkEnvError::WrapperChildOutsideDomain {
                            sig,
                            child,
                            expected: inner,
                            got: child_env,
                        });
                    }
                }
                // The block as a whole belongs to the outer domain.
                Ok(outer)
            }

            // `R_SEQ(x, y)`: require `C⟦x⟧ ⊆ C⟦y⟧`; result `C⟦x⟧`.
            SigMatch::Seq(x, y) => {
                let left = self.infer(x)?;
                let right = self.infer(y)?;
                if !is_ancestor_clk_env(self.domains, left, right) {
                    return Err(ClkEnvError::SeqViolation { sig, left, right });
                }
                Ok(left)
            }

            // `R_PROJ` entry: projections resolve through the group.
            SigMatch::Proj(_, group) => self.infer(group),

            // Unary pass-throughs (R_COMPOSITE with one child).
            SigMatch::Output(_, x)
            | SigMatch::Delay1(x)
            | SigMatch::IntCast(x)
            | SigMatch::BitCast(x)
            | SigMatch::FloatCast(x)
            | SigMatch::Gen(x)
            | SigMatch::Lowest(x)
            | SigMatch::Highest(x)
            | SigMatch::Acos(x)
            | SigMatch::Asin(x)
            | SigMatch::Atan(x)
            | SigMatch::Cos(x)
            | SigMatch::Sin(x)
            | SigMatch::Tan(x)
            | SigMatch::Exp(x)
            | SigMatch::Exp10(x)
            | SigMatch::Log(x)
            | SigMatch::Log10(x)
            | SigMatch::Sqrt(x)
            | SigMatch::Abs(x)
            | SigMatch::Floor(x)
            | SigMatch::Ceil(x)
            | SigMatch::Rint(x)
            | SigMatch::Round(x)
            | SigMatch::TempVar(x)
            | SigMatch::PermVar(x)
            | SigMatch::VBargraph(_, x)
            | SigMatch::HBargraph(_, x)
            | SigMatch::ReverseTimeRec(x) => self.infer(x),

            // Binary composites.
            SigMatch::Delay(x, y)
            | SigMatch::Prefix(x, y)
            | SigMatch::RdTbl(x, y)
            | SigMatch::Pow(x, y)
            | SigMatch::Min(x, y)
            | SigMatch::Max(x, y)
            | SigMatch::Atan2(x, y)
            | SigMatch::Fmod(x, y)
            | SigMatch::Remainder(x, y)
            | SigMatch::Attach(x, y)
            | SigMatch::Enable(x, y)
            | SigMatch::Control(x, y)
            | SigMatch::ZeroPad(x, y)
            | SigMatch::SoundfileLength(x, y)
            | SigMatch::SoundfileRate(x, y) => self.composite(sig, &[x, y]),

            SigMatch::BinOp(_, x, y) => self.composite(sig, &[x, y]),

            SigMatch::Select2(a, b, c) | SigMatch::AssertBounds(a, b, c) => {
                self.composite(sig, &[a, b, c])
            }

            SigMatch::SoundfileBuffer(a, b, c, d) => self.composite(sig, &[a, b, c, d]),

            // Tables: `max` of size/generator/write envs (read index is one
            // of the RdTbl children above).
            SigMatch::WrTbl(size, generator, wi, ws) => {
                let mut children = vec![size, generator];
                if !self.arena.is_nil(wi) {
                    children.push(wi);
                }
                if !self.arena.is_nil(ws) {
                    children.push(ws);
                }
                self.composite(sig, &children)
            }

            SigMatch::FFun(_, largs) => {
                let args =
                    list_to_vec(self.arena, largs).ok_or_else(|| ClkEnvError::Malformed {
                        sig,
                        detail: "malformed FFUN argument list".to_owned(),
                    })?;
                self.composite(sig, &args)
            }

            SigMatch::FConst(_, _, _) | SigMatch::FVar(_, _, _) => Ok(None),

            SigMatch::Fir(coefs) | SigMatch::Iir(coefs) => {
                let coefs = coefs.to_vec();
                self.composite(sig, &coefs)
            }

            SigMatch::BlockReverseAD {
                body,
                seeds,
                cotangents,
                ..
            } => {
                let mut children = Vec::new();
                for list in [body, seeds, cotangents] {
                    let items =
                        list_to_vec(self.arena, list).ok_or_else(|| ClkEnvError::Malformed {
                            sig,
                            detail: "malformed BlockReverseAD child list".to_owned(),
                        })?;
                    children.extend(items);
                }
                self.composite(sig, &children)
            }

            SigMatch::Rec(_) => Err(ClkEnvError::Malformed {
                sig,
                detail: "legacy SIGREC form is not supported by clock inference".to_owned(),
            }),

            SigMatch::Unknown => Err(ClkEnvError::Malformed {
                sig,
                detail: "unknown signal shape".to_owned(),
            }),
        }
    }

    fn is_literal_zero(&self, sig: SigId) -> bool {
        match match_sig(self.arena, sig) {
            SigMatch::Int(0) => true,
            SigMatch::Real(v) => v == 0.0,
            _ => false,
        }
    }
}

/// `findFixpoint`: Jacobi-style Kleene iteration on the group hypothesis.
fn find_fixpoint(
    arena: &TreeArena,
    domains: &ClockDomainTable,
    groups: &[RecGroup],
) -> Result<AHashMap<SigId, ClkEnv>, ClkEnvError> {
    let mut hypothesis: AHashMap<SigId, ClkEnv> = groups.iter().map(|g| (g.var, None)).collect();

    for _round in 0..MAX_ITERATIONS {
        let mut next = hypothesis.clone();
        for group in groups {
            // Fresh cache per group evaluation, seeded from the *previous*
            // hypothesis (Jacobi): order-independent, deterministic.
            let mut sweep = Inference::new(arena, domains, &hypothesis);
            let mut acc = hypothesis.get(&group.var).copied().unwrap_or(None);
            for &def in &group.defs {
                let def_env = sweep.infer(def)?;
                acc = max_clk_env(domains, def, acc, def_env)?;
            }
            next.insert(group.var, acc);
        }
        if next == hypothesis {
            return Ok(hypothesis);
        }
        hypothesis = next;
    }
    Err(ClkEnvError::FixpointDiverged {
        iterations: MAX_ITERATIONS,
    })
}

/// Entry point: infers `C⟦sig⟧` for every signal reachable from `outputs`.
///
/// Returns a side map (no tree mutation). `domains` is the propagation-owned
/// clock-domain table; the prepared staging arena preserves the `SIGCLOCKENV`
/// token ids, so the same table is valid across the arena clone.
pub fn annotate(
    arena: &TreeArena,
    domains: &ClockDomainTable,
    outputs: &[SigId],
) -> Result<ClkEnvMap, ClkEnvError> {
    let groups = collect_rec_groups(arena, outputs)?;
    let hypothesis = find_fixpoint(arena, domains, &groups)?;

    // Final sweep under the converged hypothesis: outputs first, then every
    // group definition so all reachable nodes carry an env.
    let mut sweep = Inference::new(arena, domains, &hypothesis);
    for &out in outputs {
        sweep.infer(out)?;
    }
    for group in &groups {
        for &def in &group.defs {
            sweep.infer(def)?;
        }
    }

    Ok(ClkEnvMap {
        envs: sweep.cache,
        groups: hypothesis,
    })
}

#[cfg(test)]
mod tests;
