//! Signal type inference — port of `compiler/signals/sigtyperules.cpp`.
//!
//! # Algorithm
//! Bottom-up structural recursion over the signal forest with two extensions:
//!
//! 1. **Memoisation**: already-inferred types are stored in `env` to handle
//!    DAG sharing without re-computing subtrees.
//!
//! 2. **Recursive fixed-point** for `SymRec`/`Proj` groups: recursive symbols
//!    are seeded with the C++ `TREC`-style initial tuplet type, then refined
//!    through the body until stabilisation.
//!
//! Explicit limitation: the current Rust implementation now follows the same
//! global recursive-group structure as C++ `typeAnnotation(...)`, but it still
//! uses Rust value types and memoized maps instead of the C++ pointer-based
//! `setSigType` / visited-node machinery. The stop condition is therefore
//! driven by the widened approximation re-injected into the next iteration.
//! This preserves parity on recursive typing outcomes while remaining an
//! adapted, not 1:1, port of the C++ control flow.
//!
//! # C++ source
//! `compiler/signals/sigtyperules.cpp` — `inferSigType`, `typeAnnotation`,
//! `updateRecTypes`.

use std::collections::{HashMap, HashSet};

use interval::Interval;
use signals::{BinOp, SigId, SigMatch, dump_sig_readable, match_sig};
use tlib::{NodeKind, TreeArena, match_sym_rec, match_sym_ref};
use ui::{ControlId, ControlKind, UiProgram};

use crate::enums::{Boolean, Computability, Nature, Variability, Vectorability};
use crate::factory::{make_maximal, make_simple, make_table_type, make_tuplet};
use crate::ops::{check_delay_interval, float_cast, int_cast, samp_cast, union_types};
use crate::types::SigType;

/// Typed failures returned by the type inference pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError(pub String);

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "type error: {}", self.0)
    }
}

impl std::error::Error for TypeError {}

/// One symbolic recursive group discovered before running the global recursive
/// typing fixpoint.
///
/// This mirrors the C++ `typeAnnotation(...)` preparation step, which collects
/// all recursive groups into explicit vectors before applying
/// `updateRecTypes(...)`. Rust does not use this state as the active driver
/// yet, but the scaffold is introduced first so the eventual algorithm switch
/// can happen without relying on subtree invalidation as implicit state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecGroup {
    /// The `SYMREC(var, body_list)` node representing the recursive group.
    rec_sig: SigId,
    /// The recursive variable symbol associated with `rec_sig`.
    var: SigId,
    /// The `cons(...)` body list stored inside the symbolic recursion node.
    body_list: SigId,
    /// Number of outputs/components in `body_list`.
    arity: usize,
}

/// Explicit recursive typing state prepared for a future port of the C++
/// `typeAnnotation(...)` / `updateRecTypes(...)` driver.
///
/// Design intent:
/// - `groups` is the deterministic discovery order of recursive groups,
/// - `current` will hold the current recursive approximation for each group,
/// - `upper` will hold the corresponding `TRECMAX`-style upper bound,
/// - `age_min` / `age_max` will track widening counters like the C++ vectors.
///
/// The current Rust implementation still drives recursion from `infer_sym_rec`,
/// but this state object documents and reserves the shape of the explicit
/// group-based algorithm required for parity.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecTypingState {
    groups: Vec<RecGroup>,
    current: Vec<SigType>,
    upper: Vec<SigType>,
    age_min: Vec<Vec<i32>>,
    age_max: Vec<Vec<i32>>,
}

const RECURSIVE_NARROWING_LIMIT: usize = 0;
const RECURSIVE_WIDENING_LIMIT: i32 = 0;
const MAX_SOUNDFILE_PARTS: i32 = 256;

impl RecTypingState {
    /// Builds one explicit recursive typing state from already-symbolic outputs.
    ///
    /// Discovery order is deterministic DFS over the output list with DAG
    /// sharing preserved by `visited`. This matches the kind of stable vector
    /// ordering the C++ driver expects before running `updateRecTypes(...)`.
    fn discover(arena: &TreeArena, outputs: &[SigId]) -> Result<Self, TypeError> {
        let groups = discover_recursive_groups(arena, outputs)?;
        let mut current = Vec::with_capacity(groups.len());
        let mut upper = Vec::with_capacity(groups.len());
        let mut age_min = Vec::with_capacity(groups.len());
        let mut age_max = Vec::with_capacity(groups.len());

        for group in &groups {
            current.push(initial_rec_type(arena, group.body_list)?);
            upper.push(maximal_rec_type(arena, group.body_list)?);
            age_min.push(vec![0; group.arity]);
            age_max.push(vec![0; group.arity]);
        }

        Ok(Self {
            groups,
            current,
            upper,
            age_min,
            age_max,
        })
    }
}

/// Discover all symbolic recursive groups reachable from `outputs`.
///
/// This is the Rust scaffold for the C++ `typeAnnotation(...)` pre-pass that
/// collects `vrec` / `vdef` / `vdefSizes` before running `updateRecTypes(...)`.
fn discover_recursive_groups(
    arena: &TreeArena,
    outputs: &[SigId],
) -> Result<Vec<RecGroup>, TypeError> {
    let mut groups = Vec::new();
    let mut visited = HashSet::new();
    let mut seen_groups = HashSet::new();
    for &out in outputs {
        walk_collect_rec_groups(arena, out, &mut visited, &mut seen_groups, &mut groups)?;
    }
    Ok(groups)
}

fn walk_collect_rec_groups(
    arena: &TreeArena,
    sig: SigId,
    visited: &mut HashSet<SigId>,
    seen_groups: &mut HashSet<SigId>,
    groups: &mut Vec<RecGroup>,
) -> Result<(), TypeError> {
    if !visited.insert(sig) {
        return Ok(());
    }

    if arena.is_nil(sig) {
        return Ok(());
    }

    if arena.is_list(sig) {
        let head = arena.hd(sig).ok_or_else(|| {
            TypeError("malformed list payload during recursive group discovery".to_owned())
        })?;
        let tail = arena.tl(sig).ok_or_else(|| {
            TypeError("malformed list payload during recursive group discovery".to_owned())
        })?;
        walk_collect_rec_groups(arena, head, visited, seen_groups, groups)?;
        walk_collect_rec_groups(arena, tail, visited, seen_groups, groups)?;
        return Ok(());
    }

    if let Some((var, body_list)) = match_sym_rec(arena, sig) {
        if seen_groups.insert(sig) {
            groups.push(RecGroup {
                rec_sig: sig,
                var,
                body_list,
                arity: body_list_arity(arena, body_list)?,
            });
        }
        walk_collect_rec_groups(arena, body_list, visited, seen_groups, groups)?;
        return Ok(());
    }

    if match_sym_ref(arena, sig).is_some() {
        return Ok(());
    }

    let node = arena.node(sig).ok_or_else(|| {
        TypeError(format!(
            "missing node {} during recursive group discovery",
            sig.as_u32()
        ))
    })?;
    for child in node.children.as_slice() {
        walk_collect_rec_groups(arena, *child, visited, seen_groups, groups)?;
    }
    Ok(())
}

fn body_list_arity(arena: &TreeArena, mut body_list: SigId) -> Result<usize, TypeError> {
    let mut n = 0usize;
    while !arena.is_nil(body_list) {
        let _head = arena.hd(body_list).ok_or_else(|| {
            TypeError("malformed symbolic recursion body list during discovery".to_owned())
        })?;
        body_list = arena.tl(body_list).ok_or_else(|| {
            TypeError("malformed symbolic recursion body list during discovery".to_owned())
        })?;
        n += 1;
    }
    Ok(n)
}

/// Bottom-up signal type annotator.
///
/// Carries memoised results in `env`.
///
/// Recursive parity note:
/// - scalar and tuple recursion are seeded from the C++ `TREC` approximation,
/// - the body is re-inferred after invalidating memoised subtree entries so the
///   next iteration observes the updated recursive approximation,
/// - this preserves the intended recursive semantics for the current Rust
///   architecture, but it is still an adapted implementation rather than a
///   direct port of C++ `updateRecTypes(...)`.
pub struct TypeAnnotator<'a> {
    arena: &'a TreeArena,
    ui_program: &'a UiProgram,
    /// Memoised type results.
    env: HashMap<SigId, SigType>,
    /// Nodes whose type is currently being computed (cycle guard).
    in_progress: HashSet<SigId>,
}

impl<'a> TypeAnnotator<'a> {
    /// Create a new annotator for the given signal forest.
    #[must_use]
    pub fn new(arena: &'a TreeArena, ui_program: &'a UiProgram) -> Self {
        Self {
            arena,
            ui_program,
            env: HashMap::new(),
            in_progress: HashSet::new(),
        }
    }

    /// Annotate all output roots and return the complete `SigId → SigType` map.
    ///
    /// # C++ source
    /// `typeAnnotation(Tree sig, bool causality)` entry point.
    pub fn annotate(&mut self, outputs: &[SigId]) -> Result<HashMap<SigId, SigType>, TypeError> {
        let mut rec_state = RecTypingState::discover(self.arena, outputs)?;
        if !rec_state.groups.is_empty() {
            self.solve_recursive_groups(&mut rec_state)?;
            self.env.clear();
            self.in_progress.clear();
            self.seed_rec_groups(&rec_state.groups, &rec_state.current);
            for group in &rec_state.groups {
                self.infer(group.body_list)?;
            }
        }
        let mut visited = HashSet::new();
        for &out in outputs {
            self.populate_reachable_types(out, &mut visited)?;
        }
        Ok(self.env.clone())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Core dispatch
    // ─────────────────────────────────────────────────────────────────────────

    /// Infer the type of one signal node (memoised).
    ///
    /// # C++ source
    /// `Type T(Tree term, Tree env)` / `inferSigType(Tree sig, Tree env)`
    fn infer(&mut self, sig: SigId) -> Result<SigType, TypeError> {
        // Return memoised result if available.
        if let Some(t) = self.env.get(&sig) {
            return Ok(t.clone());
        }
        // If in progress (recursive reference), return the current approximation
        // seeded by the fixed-point loop, or conservative maximal if outside loop.
        if self.in_progress.contains(&sig) {
            return Ok(self.env.get(&sig).cloned().unwrap_or_else(make_maximal));
        }

        self.in_progress.insert(sig);
        let ty = self.infer_inner(sig)?;
        self.in_progress.remove(&sig);
        self.env.insert(sig, ty.clone());
        Ok(ty)
    }

    fn infer_inner(&mut self, sig: SigId) -> Result<SigType, TypeError> {
        match match_sig(self.arena, sig) {
            // ── Literals ────────────────────────────────────────────────────
            SigMatch::Int(n) => Ok(make_simple(
                Nature::Int,
                Variability::Konst,
                Computability::Comp,
                Vectorability::Vect,
                Boolean::Num,
                interval::singleton(f64::from(n)),
            )),

            SigMatch::Real(r) => Ok(make_simple(
                Nature::Real,
                Variability::Konst,
                Computability::Comp,
                Vectorability::Vect,
                Boolean::Num,
                interval::singleton(r),
            )),

            // ── Inputs / Outputs ────────────────────────────────────────────
            // C++: TINPUT = makeSimpleType(kReal, kSamp, kExec, kVect, kNum, interval(-1, 1))
            // Audio inputs carry the normalised audio range [-1, 1], not an
            // uninformative placeholder.  The lsb=-24 matches 24-bit precision.
            SigMatch::Input(_) => Ok(make_simple(
                Nature::Real,
                Variability::Samp,
                Computability::Exec,
                Vectorability::Vect,
                Boolean::Num,
                interval::Interval::new(-1.0, 1.0, -24),
            )),

            SigMatch::Output(_, s) => {
                let st = self.infer(s)?;
                Ok(samp_cast(st))
            }

            // ── Delays ──────────────────────────────────────────────────────
            // C++: castInterval(sampCast(t), itv::reunion(t->getInterval(), interval(0)))
            // The first sample of a delay1 is 0, so the interval must include 0.
            SigMatch::Delay1(x) => {
                let tx = self.infer(x)?;
                let itv = interval::reunion(tx.interval(), interval::singleton(0.0));
                Ok(samp_cast(tx).promote_interval(itv))
            }

            SigMatch::Delay(x, n) => {
                let tn = self.infer(n)?;
                // Validate that the delay amount has a bounded non-negative interval.
                check_delay_interval(&tn).map_err(|e| TypeError(e.0))?;
                let tx = self.infer(x)?;
                // C++: castInterval(sampCast(t1), itv::reunion(t1->getInterval(), interval(0)))
                let itv = interval::reunion(tx.interval(), interval::singleton(0.0));
                Ok(samp_cast(tx).promote_interval(itv))
            }

            // ── Structured FIR/IIR carriers ────────────────────────────────
            // C++ provenance:
            // - `compiler/signals/sigFIR.cpp`
            // - `compiler/signals/sigIIR.cpp`
            //
            // These compact carrier nodes are algebraic views over delayed
            // sample expressions. Typing them structurally keeps the new filter
            // algebra usable by later passes without forcing callers to expand
            // carriers back into raw `delay/add/mul` syntax first.
            SigMatch::Fir(coefs) => self.infer_fir_carrier(coefs),
            SigMatch::Iir(coefs) => self.infer_iir_carrier(coefs),

            SigMatch::Prefix(init, s) => {
                let ti = self.infer(init)?;
                let ts = self.infer(s)?;
                // C++: castInterval(sampCast(t1|t2), reunion(t1, t2))
                // union_types already computes reunion(a.interval(), b.interval()), so this
                // matches the C++ behaviour.
                Ok(samp_cast(union_types(ti, ts)))
            }

            // ── Casts ───────────────────────────────────────────────────────
            SigMatch::IntCast(x) => {
                let tx = self.infer(x)?;
                Ok(int_cast(tx))
            }

            SigMatch::BitCast(x) => {
                let tx = self.infer(x)?;
                Ok(crate::ops::bit_cast(tx))
            }

            SigMatch::FloatCast(x) => {
                let tx = self.infer(x)?;
                Ok(float_cast(tx))
            }

            // ── Binary operators ─────────────────────────────────────────────
            SigMatch::BinOp(op, lhs, rhs) => {
                let tl = self.infer(lhs)?;
                let tr = self.infer(rhs)?;
                Ok(self.infer_binop(op, tl, tr))
            }

            // ── Pow / Min / Max ──────────────────────────────────────────────
            SigMatch::Pow(x, y) => {
                let tx = self.infer(x)?;
                let ty = self.infer(y)?;
                let itv = interval::ops::math::pow(tx.interval(), ty.interval());
                let t = union_types(tx, ty);
                Ok(float_cast(t).promote_interval(itv))
            }

            SigMatch::Min(x, y) => {
                let tx = self.infer(x)?;
                let ty = self.infer(y)?;
                let itv = interval::ops::logic::min(tx.interval(), ty.interval());
                Ok(union_types(tx, ty).promote_interval(itv))
            }

            SigMatch::Max(x, y) => {
                let tx = self.infer(x)?;
                let ty = self.infer(y)?;
                let itv = interval::ops::logic::max(tx.interval(), ty.interval());
                Ok(union_types(tx, ty).promote_interval(itv))
            }

            // ── Unary math (always Real) ─────────────────────────────────────
            SigMatch::Abs(x) => {
                let tx = self.infer(x)?;
                let itv = interval::ops::arithmetic::abs(tx.interval());
                Ok(tx.promote_interval(itv))
            }
            SigMatch::Sqrt(x) => self.infer_unary_math(x, interval::ops::math::sqrt),
            SigMatch::Exp(x) => self.infer_unary_math(x, interval::ops::math::exp),
            SigMatch::Exp10(x) => self.infer_unary_math(x, interval::ops::math::exp10),
            SigMatch::Log(x) => self.infer_unary_math(x, interval::ops::math::log),
            SigMatch::Log10(x) => self.infer_unary_math(x, interval::ops::math::log10),
            SigMatch::Floor(x) => self.infer_unary_math(x, interval::ops::math::floor),
            SigMatch::Ceil(x) => self.infer_unary_math(x, interval::ops::math::ceil),
            SigMatch::Rint(x) => self.infer_unary_math(x, interval::ops::math::rint),
            SigMatch::Round(x) => self.infer_unary_math(x, interval::ops::math::round),
            SigMatch::Sin(x) => self.infer_unary_math(x, interval::ops::trig::sin),
            SigMatch::Cos(x) => self.infer_unary_math(x, interval::ops::trig::cos),
            SigMatch::Tan(x) => self.infer_unary_math(x, interval::ops::trig::tan),
            SigMatch::Asin(x) => self.infer_unary_math(x, interval::ops::trig::asin),
            SigMatch::Acos(x) => self.infer_unary_math(x, interval::ops::trig::acos),
            SigMatch::Atan(x) => self.infer_unary_math(x, interval::ops::trig::atan),

            SigMatch::Atan2(y, x) => {
                let ty = self.infer(y)?;
                let tx = self.infer(x)?;
                let itv = interval::ops::trig::atan2(ty.interval(), tx.interval());
                // Pure math — variability follows arguments, not forced to Samp.
                let t = union_types(ty, tx);
                Ok(float_cast(t).promote_interval(itv))
            }

            SigMatch::Fmod(x, y) => {
                let tx = self.infer(x)?;
                let ty = self.infer(y)?;
                let itv = interval::ops::arithmetic::mod_interval(tx.interval(), ty.interval());
                let t = union_types(tx, ty);
                Ok(float_cast(t).promote_interval(itv))
            }

            SigMatch::Remainder(x, y) => {
                let tx = self.infer(x)?;
                let ty = self.infer(y)?;
                let t = union_types(tx, ty);
                Ok(float_cast(t))
            }

            // ── Select ───────────────────────────────────────────────────────
            // C++: makeSimpleType(st1|st2 nature, st1|st2|stsel variability,
            //                     st1|st2|stsel computability, ..., reunion(st1, st2) interval)
            // Note: NO samp_cast — if all inputs are Konst, the result can be Konst.
            SigMatch::Select2(cond, s1, s2) => {
                let tc = self.infer(cond)?;
                let t1 = self.infer(s1)?;
                let t2 = self.infer(s2)?;
                let itv = interval::reunion(t1.interval(), t2.interval());
                let t = union_types(t1, t2)
                    .promote_variability(tc.variability())
                    .promote_computability(tc.computability())
                    .promote_vectorability(tc.vectorability());
                Ok(t.promote_interval(itv))
            }

            // ── Tables ───────────────────────────────────────────────────────
            SigMatch::RdTbl(tbl, idx) => {
                let ttbl = self.infer(tbl)?;
                let tidx = self.infer(idx)?;
                infer_read_table(ttbl, tidx)
            }

            SigMatch::WrTbl(size, generator, wi, ws) => {
                let ttbl = self.infer_write_table(size, generator)?;
                let twi = self.infer(wi)?;
                let tws = self.infer(ws)?;
                infer_write_table_type(ttbl, twi, tws)
            }

            SigMatch::Gen(x) => self.infer(x),

            // ── Waveform ─────────────────────────────────────────────────────
            SigMatch::Waveform(elems) => {
                // `SIGWAVEFORM` in the propagated signal tree represents the
                // **cycling output**: each sample yields the next element of the
                // table (advancing an implicit counter modulo the table length).
                //
                // The TABLE DATA is compile-time constant (`kKonst` in C++ Faust,
                // and `make_table_type` hardcodes `Variability::Konst` to match
                // that).  The OUTPUT, however, changes every sample — so the
                // signal's variability must be `Samp`.
                //
                // If we leave it as `Konst`, the variability-driven placement pass
                // in `signal_fir` hoists every expression that depends on the
                // waveform — including `clk = reset > 0` — into `instanceConstants`.
                // Those expressions then capture `LoadTable(fTblN, iWaveN)` at
                // initialisation time, before `instanceClear` has zeroed `iWaveN`,
                // producing wrong output.
                //
                // Promoting to `Samp` propagates through all dependents and keeps
                // them in the sample loop where they belong.
                if elems.is_empty() {
                    return Ok(make_simple(
                        Nature::Real,
                        Variability::Samp,
                        Computability::Comp,
                        Vectorability::Vect,
                        Boolean::Num,
                        interval::empty(),
                    ));
                }
                let mut t = self.infer(elems[0])?;
                let mut itv = t.interval();
                for &e in &elems[1..] {
                    let te = self.infer(e)?;
                    itv = interval::reunion(itv, te.interval());
                    t = union_types(t, te);
                }
                Ok(make_table_type(t.promote_interval(itv)).promote_variability(Variability::Samp))
            }

            // ── UI controls ──────────────────────────────────────────────────
            SigMatch::Button(id) => Ok(self.infer_button(id)),
            SigMatch::Checkbox(id) => Ok(self.infer_checkbox(id)),
            SigMatch::VSlider(id) => Ok(self.infer_slider(id)),
            SigMatch::HSlider(id) => Ok(self.infer_slider(id)),
            SigMatch::NumEntry(id) => Ok(self.infer_slider(id)),
            SigMatch::VBargraph(id, x) => Ok(self.infer_bargraph(id, x)?),
            SigMatch::HBargraph(id, x) => Ok(self.infer_bargraph(id, x)?),

            // ── Soundfile ────────────────────────────────────────────────────
            // C++: makeSimpleType(kInt, kBlock, kInit, kVect, kNum, interval(0, INT32_MAX))
            SigMatch::Soundfile(_) => Ok(make_simple(
                Nature::Int,
                Variability::Block,
                Computability::Init,
                Vectorability::Vect,
                Boolean::Num,
                interval::Interval::new(0.0, i32::MAX as f64, 0),
            )),
            // C++: makeSimpleType(kInt, max(kBlock, t2->variability()), kInit, kVect, kNum, interval(0, INT32_MAX))
            SigMatch::SoundfileLength(sf, part) => {
                self.infer(sf)?;
                let t_part = self.infer(part)?;
                self.check_soundfile_part_interval(sig, &t_part)?;
                let var = Variability::Block.join(t_part.variability());
                Ok(make_simple(
                    Nature::Int,
                    var,
                    Computability::Init,
                    Vectorability::Vect,
                    Boolean::Num,
                    interval::Interval::new(0.0, i32::MAX as f64, 0),
                ))
            }
            SigMatch::SoundfileRate(sf, part) => {
                self.infer(sf)?;
                let t_part = self.infer(part)?;
                self.check_soundfile_part_interval(sig, &t_part)?;
                let var = Variability::Block.join(t_part.variability());
                Ok(make_simple(
                    Nature::Int,
                    var,
                    Computability::Init,
                    Vectorability::Vect,
                    Boolean::Num,
                    interval::Interval::new(0.0, i32::MAX as f64, 0),
                ))
            }
            // C++: makeSimpleType(kReal, kSamp, kInit, kVect, kNum, interval(-1, 1))
            SigMatch::SoundfileBuffer(sf, x, part, z) => {
                self.infer(sf)?;
                self.infer(x)?;
                let t_part = self.infer(part)?;
                self.infer(z)?;
                self.check_soundfile_part_interval(sig, &t_part)?;
                Ok(make_simple(
                    Nature::Real,
                    Variability::Samp,
                    Computability::Init,
                    Vectorability::Vect,
                    Boolean::Num,
                    interval::Interval::new(-1.0, 1.0, -24),
                ))
            }

            // ── Attach / Enable / Control ────────────────────────────────────
            SigMatch::Attach(x, _y) => self.infer(x),
            SigMatch::Enable(x, _y) => self.infer(x),
            SigMatch::Control(x, _y) => self.infer(x),

            // ── Recursion ────────────────────────────────────────────────────
            // Symbolic recursion (after de_bruijn_to_sym).
            _ if match_sym_rec(self.arena, sig).is_some() => self.infer_sym_rec(sig),
            _ if match_sym_ref(self.arena, sig).is_some() => {
                let var = match_sym_ref(self.arena, sig).expect("guard checked sym ref");
                // Reference to the recursive variable — return current approx.
                Ok(self.env.get(&var).cloned().unwrap_or_else(make_maximal))
            }

            // `Rec` (de Bruijn form, should be converted before reaching here).
            SigMatch::Rec(body) => {
                // Conservative fallback: treat as maximal.
                self.infer(body).map(|_| make_maximal())
            }
            SigMatch::ReverseTimeRec(body) => self.infer(body),

            // Block reverse-AD carrier: the projection contract addresses
            // primal outputs first (slots `0..primal_count-1`) followed by
            // per-sample seed gradients (slots `primal_count..M+N-1`).
            //
            // For type inference the carrier behaves like a tuplet of all
            // visible outputs in declaration order: each primal slot
            // adopts the type of the corresponding `body` element, and
            // each gradient slot adopts the type of the corresponding
            // `seeds` element. Variability is left as inferred — phase B0
            // does not specialise it; downstream lowering will tighten
            // when the carrier becomes addressable through `Proj`.
            //
            // Source provenance: original Rust design in
            // `porting/rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md`,
            // section "11.1 Phase B0 — Signal carrier + minimal validation".
            SigMatch::BlockReverseAD {
                body,
                seeds,
                cotangents,
                ..
            } => {
                // Walking the cotangent list keeps each `1.0` constant
                // typed, which matters for the explicit-VJP successor of
                // this rule even though phase-B0 cotangents are constants.
                let _ = self.infer(cotangents)?;
                let body_ty = self.infer(body)?;
                let seeds_ty = self.infer(seeds)?;
                let mut components: Vec<SigType> = match body_ty {
                    SigType::Tuplet(t) => t.components,
                    other => vec![other],
                };
                match seeds_ty {
                    SigType::Tuplet(t) => components.extend(t.components),
                    other => components.push(other),
                }
                Ok(make_tuplet(components))
            }

            SigMatch::Proj(idx, group) => self.infer_proj(idx, group),

            // ── Foreign ─────────────────────────────────────────────────────
            SigMatch::FFun(ff, args) => self.infer_foreign_fun_type(ff, args),
            // C++: makeSimpleType(tree2int(type), kKonst, kInit, kVect, kNum, interval())
            SigMatch::FConst(kind, _, _) => self.infer_foreign_const_type(kind),
            // C++: makeSimpleType(tree2int(type), kBlock, kExec, kVect, kNum, interval())
            SigMatch::FVar(kind, _, _) => self.infer_foreign_var_type(kind),

            // ── Misc / conservative ──────────────────────────────────────────
            // C++: isSigAssertBounds(sig, min, max, cur)
            // Returns cur's type with interval clamped to [max(cur.lo, min), min(cur.hi, max)].
            SigMatch::AssertBounds(min_sig, max_sig, cur_sig) => {
                let t_min = self.infer(min_sig)?;
                let t_max = self.infer(max_sig)?;
                let t_cur = self.infer(cur_sig)?;
                let i_cur = t_cur.interval();
                let lo_bound = t_min.interval().lo();
                let hi_bound = t_max.interval().hi();
                let iend = if !i_cur.is_empty() {
                    interval::Interval::new(lo_bound.max(i_cur.lo()), hi_bound.min(i_cur.hi()), -24)
                } else {
                    interval::Interval::new(lo_bound, hi_bound, -24)
                };
                Ok(t_cur.promote_interval(iend))
            }

            // C++: makeSimpleType(kReal, kKonst, kComp, kVect, kNum, interval(i1.lo()))
            SigMatch::Lowest(x) => {
                let tx = self.infer(x)?;
                Ok(make_simple(
                    Nature::Real,
                    Variability::Konst,
                    Computability::Comp,
                    Vectorability::Vect,
                    Boolean::Num,
                    interval::singleton(tx.interval().lo()),
                ))
            }
            // C++: makeSimpleType(kReal, kKonst, kComp, kVect, kNum, interval(i1.hi()))
            SigMatch::Highest(x) => {
                let tx = self.infer(x)?;
                Ok(make_simple(
                    Nature::Real,
                    Variability::Konst,
                    Computability::Comp,
                    Vectorability::Vect,
                    Boolean::Num,
                    interval::singleton(tx.interval().hi()),
                ))
            }

            SigMatch::TempVar(x) => self.infer(x),
            // C++: castInterval(sampCast(t1), reunion(t1.interval, interval(0)))
            SigMatch::PermVar(x) => {
                let tx = self.infer(x)?;
                let itv = interval::reunion(tx.interval(), interval::singleton(0.0));
                Ok(samp_cast(tx).promote_interval(itv))
            }
            // C++: T(x, env); return T(y, env)  — return type is y's type only
            SigMatch::Seq(x, y) => {
                self.infer(x)?;
                self.infer(y)
            }
            // C++: castInterval(sampCast(t1), reunion(t1.interval, interval(0)))
            SigMatch::ZeroPad(x, _) => {
                let tx = self.infer(x)?;
                let itv = interval::reunion(tx.interval(), interval::singleton(0.0));
                Ok(samp_cast(tx).promote_interval(itv))
            }
            SigMatch::Clocked(_, x) => self.infer(x),
            // The clock-env token is an opaque annotation (first child of
            // `Clocked`), never a signal operand: `Clocked(_, x)` above only
            // infers `x`. Reaching it here means a traversal leaked it into
            // signal position — fail loudly instead of guessing a type
            // (roadmap P0: no silent state for the clocked machinery).
            SigMatch::ClockEnvToken(id) => Err(TypeError(format!(
                "clock-env token #{id} reached type inference in signal position; \
                 it must stay an opaque annotation of Clocked(env, y)"
            ))),

            // C++: type each sub for side effects, then return
            //   makeSimpleType(kReal, kSamp, kExec, kScal, kNum, interval(-1, 1))
            // The block lacks a proper type but must not be kKonst (avoids constant propagation).
            SigMatch::OnDemand(subs)
            | SigMatch::Upsampling(subs)
            | SigMatch::Downsampling(subs) => {
                for &s in subs {
                    self.infer(s)?;
                }
                Ok(make_simple(
                    Nature::Real,
                    Variability::Samp,
                    Computability::Exec,
                    Vectorability::Scal,
                    Boolean::Num,
                    interval::Interval::new(-1.0, 1.0, -24),
                ))
            }

            // Unknown / unhandled — check for cons lists first (recursion bodies).
            //
            // C++ `T(sig, env)` handles list nodes by iterating with hd/tl and
            // building a `TupletType` from the element types.  The signal arena
            // represents recursion bodies as `cons(signal, … cons(signal, nil))`.
            SigMatch::Unknown if self.arena.is_list(sig) => {
                let mut components = Vec::new();
                let mut cur = sig;
                while !self.arena.is_nil(cur) {
                    let head = self.arena.hd(cur).ok_or_else(|| {
                        TypeError("malformed cons list during type inference".into())
                    })?;
                    components.push(self.infer(head)?);
                    cur = self.arena.tl(cur).ok_or_else(|| {
                        TypeError("malformed cons list during type inference".into())
                    })?;
                }
                Ok(make_tuplet(components))
            }

            // Unknown / unhandled — conservative.
            SigMatch::Unknown => Ok(make_maximal()),
        }
    }

    /// Mirrors C++ `checkPartInterval(sig, t)` for soundfile part selectors.
    fn check_soundfile_part_interval(&self, sig: SigId, ty: &SigType) -> Result<(), TypeError> {
        let interval = ty.interval();
        if !interval.is_valid()
            || interval.lo() < 0.0
            || interval.hi() >= f64::from(MAX_SOUNDFILE_PARTS)
        {
            return Err(TypeError(format!(
                "ERROR : out of range soundfile part number ({} instead of interval(0,{})) in expression : {}",
                interval,
                MAX_SOUNDFILE_PARTS - 1,
                dump_sig_readable(self.arena, sig)
            )));
        }
        Ok(())
    }

    fn solve_recursive_groups(&mut self, state: &mut RecTypingState) -> Result<(), TypeError> {
        for _ in std::iter::repeat_n((), RECURSIVE_NARROWING_LIMIT) {
            self.update_rec_types(&state.groups, &mut state.upper, true)?;
        }

        loop {
            let prev = state.current.clone();
            self.update_rec_types(&state.groups, &mut state.current, false)?;
            let mut finished = true;

            for (g, previous) in prev.iter().enumerate().take(state.groups.len()) {
                let widened = apply_recursive_widening(
                    previous.clone(),
                    state.current[g].clone(),
                    state.upper[g].clone(),
                    &mut state.age_min[g],
                    &mut state.age_max[g],
                )?;
                if widened != *previous {
                    finished = false;
                }
                state.current[g] = widened;
            }

            if finished {
                break;
            }
        }

        Ok(())
    }

    fn update_rec_types(
        &mut self,
        groups: &[RecGroup],
        approximations: &mut [SigType],
        inter: bool,
    ) -> Result<(), TypeError> {
        self.env.clear();
        self.in_progress.clear();
        self.seed_rec_groups(groups, approximations);

        let mut next = Vec::with_capacity(groups.len());
        for (group, old) in groups.iter().zip(approximations.iter()) {
            let old_tuplet = as_tuplet_type(old)?;
            let inferred = self.infer(group.body_list)?;
            let new_tuplet = as_tuplet_type(&inferred)?;
            let mut items = Vec::with_capacity(group.arity);
            for j in 0..group.arity {
                let old_comp = &old_tuplet.components[j];
                let mut component = new_tuplet.components[j].clone();
                // Join variability and computability with the old approximation
                // so the kSamp/kInit floor from the initial TREC seed is
                // preserved across iterations — matching C++ joinType semantics
                // in updateRecTypes.  Without this, a body that ignores its
                // recursive input could lower a component to kKonst/kComp,
                // allowing Proj nodes to be hoisted out of the sample loop.
                component = component
                    .promote_variability(old_comp.variability())
                    .promote_computability(old_comp.computability());
                let merged_i = if inter {
                    interval::intersection(component.interval(), old_comp.interval())
                } else {
                    interval::reunion(component.interval(), old_comp.interval())
                };
                component = component.promote_interval(merged_i);
                items.push(component);
            }
            next.push(make_tuplet(items));
        }

        approximations.clone_from_slice(&next);
        self.env.clear();
        self.in_progress.clear();
        Ok(())
    }

    fn seed_rec_groups(&mut self, groups: &[RecGroup], approximations: &[SigType]) {
        for (group, ty) in groups.iter().zip(approximations.iter()) {
            self.env.insert(group.rec_sig, ty.clone());
            self.env.insert(group.var, ty.clone());
        }
    }

    fn populate_reachable_types(
        &mut self,
        sig: SigId,
        visited: &mut HashSet<SigId>,
    ) -> Result<(), TypeError> {
        if !visited.insert(sig) {
            return Ok(());
        }

        // `Clocked(env, y)`: the clock-env child is an opaque annotation
        // (nil or a `SIGCLOCKENV` token), never a signal — skip it entirely
        // instead of walking its children generically (roadmap P0.1).
        if let SigMatch::Clocked(_, y) = match_sig(self.arena, sig) {
            self.infer(sig)?;
            return self.populate_reachable_types(y, visited);
        }

        self.infer(sig)?;

        if self.arena.is_nil(sig) {
            return Ok(());
        }

        if self.arena.is_list(sig) {
            let head = self.arena.hd(sig).ok_or_else(|| {
                TypeError("malformed list during reachable type population".into())
            })?;
            let tail = self.arena.tl(sig).ok_or_else(|| {
                TypeError("malformed list during reachable type population".into())
            })?;
            self.populate_reachable_types(head, visited)?;
            self.populate_reachable_types(tail, visited)?;
            return Ok(());
        }

        if let Some((_var, body_list)) = match_sym_rec(self.arena, sig) {
            self.populate_reachable_types(body_list, visited)?;
            return Ok(());
        }

        let node = self.arena.node(sig).ok_or_else(|| {
            TypeError(format!(
                "missing signal node {} during type population",
                sig.as_u32()
            ))
        })?;
        for child in node.children.as_slice() {
            self.populate_reachable_types(*child, visited)?;
        }
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    fn infer_unary_math(
        &mut self,
        x: SigId,
        f: fn(Interval) -> Interval,
    ) -> Result<SigType, TypeError> {
        let tx = self.infer(x)?;
        let itv = f(tx.interval());
        // Pure math functions preserve the variability of their argument:
        // sin(Konst) → Konst, sin(Block) → Block, sin(Samp) → Samp.
        // Only the nature changes to Real (via float_cast).
        // The previous samp_cast was overly conservative — it forced all
        // transcendental results to Samp, preventing constant-folding of
        // expressions like sin(SR) into instanceConstants.
        Ok(float_cast(tx).promote_interval(itv))
    }

    fn infer_binop(&self, op: BinOp, tl: SigType, tr: SigType) -> SigType {
        use interval::ops::{arithmetic as ar, logic as lg};

        let il = tl.interval();
        let ir = tr.interval();
        let (itv, nature) = match op {
            BinOp::Add => (ar::add(il, ir), tl.nature().join(tr.nature())),
            BinOp::Sub => (ar::sub(il, ir), tl.nature().join(tr.nature())),
            BinOp::Mul => (ar::mul(il, ir), tl.nature().join(tr.nature())),
            BinOp::Div => (ar::div(il, ir), Nature::Real),
            BinOp::Rem => (ar::mod_interval(il, ir), tl.nature().join(tr.nature())),
            BinOp::Lsh => (lg::lsh(il, ir), Nature::Int),
            BinOp::ARsh | BinOp::LRsh => (lg::rsh(il, ir), Nature::Int),
            BinOp::Gt => (lg::gt(il, ir), Nature::Int),
            BinOp::Lt => (lg::lt(il, ir), Nature::Int),
            BinOp::Ge => (lg::ge(il, ir), Nature::Int),
            BinOp::Le => (lg::le(il, ir), Nature::Int),
            BinOp::Eq => (lg::eq(il, ir), Nature::Int),
            BinOp::Ne => (lg::ne(il, ir), Nature::Int),
            BinOp::And => (lg::and(il, ir), Nature::Int),
            BinOp::Or => (lg::or(il, ir), Nature::Int),
            BinOp::Xor => (lg::xor(il, ir), Nature::Int),
        };
        let variability = tl.variability().join(tr.variability());
        let computability = tl.computability().join(tr.computability());
        let vectorability = tl.vectorability().join(tr.vectorability());
        let boolean = match op {
            BinOp::Gt | BinOp::Lt | BinOp::Ge | BinOp::Le | BinOp::Eq | BinOp::Ne => Boolean::Bool,
            _ => Boolean::Num,
        };
        make_simple(
            nature,
            variability,
            computability,
            vectorability,
            boolean,
            itv,
        )
    }

    /// Infer the type of a compact `sigFIR([base, tap0, tap1, ...])` carrier.
    ///
    /// The carrier denotes `sum_i tap_i * (base @ i)`. Delayed taps are
    /// sample-rate values whose startup interval includes zero, just like the
    /// ordinary `Delay` rule. This helper mirrors that expanded expression
    /// enough for type annotation while keeping the compact carrier intact.
    fn infer_fir_carrier(&mut self, coefs: &[SigId]) -> Result<SigType, TypeError> {
        if coefs.len() < 2 {
            return Ok(make_maximal());
        }

        let base_type = self.infer(coefs[0])?;
        let delayed_base_type = samp_cast(base_type.clone()).promote_interval(interval::reunion(
            base_type.interval(),
            interval::singleton(0.0),
        ));

        let mut result = None;
        for (idx, coef) in coefs.iter().copied().enumerate().skip(1) {
            let coef_type = self.infer(coef)?;
            let signal_type = if idx == 1 {
                base_type.clone()
            } else {
                delayed_base_type.clone()
            };
            let term = self.infer_binop(BinOp::Mul, coef_type, signal_type);
            result = Some(match result {
                Some(acc) => self.infer_binop(BinOp::Add, acc, term),
                None => term,
            });
        }

        Ok(samp_cast(result.unwrap_or_else(make_maximal)))
    }

    /// Infer the type of a compact `sigIIR([rt, input, fb0, fb1, ...])`.
    ///
    /// Index `0` is metadata identifying the recursive target and is not
    /// interpreted as a numeric coefficient. The output is necessarily a
    /// sample-rate recursive signal. In absence of the fully expanded recursive
    /// equation, the conservative structural type joins the independent input
    /// and feedback coefficient types, then raises variability to `Samp`.
    fn infer_iir_carrier(&mut self, coefs: &[SigId]) -> Result<SigType, TypeError> {
        if coefs.len() < 2 {
            return Ok(make_maximal());
        }

        let mut result = self.infer(coefs[1])?;
        for coef in coefs.iter().copied().skip(2) {
            result = union_types(result, self.infer(coef)?);
        }
        Ok(samp_cast(result))
    }

    // ── UI controls ──────────────────────────────────────────────────────────

    fn infer_button(&self, _id: ControlId) -> SigType {
        // C++: castInterval(TGUI, gAlgebra.Button(interval(0)))
        // TGUI = makeSimpleType(kReal, kBlock, kExec, kVect, kNum, interval(0,1))
        // castInterval keeps kReal/kVect/kNum — it does NOT set kBool or kScal.
        make_simple(
            Nature::Real,
            Variability::Block,
            Computability::Exec,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new(0.0, 1.0, 0),
        )
    }

    fn infer_checkbox(&self, _id: ControlId) -> SigType {
        // C++: same as Button — castInterval(TGUI, gAlgebra.Checkbox(interval(0)))
        make_simple(
            Nature::Real,
            Variability::Block,
            Computability::Exec,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new(0.0, 1.0, 0),
        )
    }

    fn infer_slider(&self, id: ControlId) -> SigType {
        let itv = self
            .ui_program
            .control(id)
            .and_then(|spec| {
                // Only slider-family controls have ranges.
                if !matches!(
                    spec.kind,
                    ControlKind::VSlider | ControlKind::HSlider | ControlKind::NumEntry
                ) {
                    return None;
                }
                spec.range
            })
            .map(|r| {
                // vslider(_name, _init, lo, hi, step)
                interval::ops::ui::vslider(
                    interval::empty(), // _name (unused)
                    interval::singleton(r.init),
                    interval::singleton(r.min),
                    interval::singleton(r.max),
                    interval::singleton(r.step),
                )
            })
            .unwrap_or_else(interval::Interval::new_default);

        // C++: castInterval(TGUI, vslider_interval(...))
        // TGUI = makeSimpleType(kReal, kBlock, kExec, kVect, kNum, interval())
        // castInterval replaces only the interval → Computability is kExec, not kInit.
        make_simple(
            Nature::Real,
            Variability::Block,
            Computability::Exec,
            Vectorability::Vect,
            Boolean::Num,
            itv,
        )
    }

    fn infer_bargraph(&mut self, _id: ControlId, x: SigId) -> Result<SigType, TypeError> {
        // C++: T(s1, env)->promoteVariability(kBlock)
        // The lo/hi bound signals are computed for side effects in C++ but their
        // values are NOT reflected in the returned type.  The returned type is the
        // input signal's type with variability promoted to at least kBlock.
        let tx = self.infer(x)?;
        Ok(tx.promote_variability(Variability::Block))
    }

    // ── Tables ───────────────────────────────────────────────────────────────

    fn infer_write_table(&mut self, size: SigId, generator: SigId) -> Result<SigType, TypeError> {
        let _tsize = self.infer(size)?;
        let tgen = self.infer(generator)?;
        Ok(make_table_type(tgen))
    }

    // ── Foreign type annotation ───────────────────────────────────────────────

    fn foreign_nature(&self, kind: SigId) -> Nature {
        // The kind node encodes the C type via a tree integer.
        // 0 = int (kInt), 1 = float/double (kReal), default = Real.
        match tlib::tree_to_int(self.arena, kind) {
            Some(0) => Nature::Int,
            _ => Nature::Real,
        }
    }

    /// `inferFConstType`: constant, evaluated at init, no static range known.
    ///
    /// # C++ source
    /// `makeSimpleType(tree2int(type), kKonst, kInit, kVect, kNum, interval())`
    ///
    /// `interval()` in C++ uses the default member initializers:
    /// `fLo = std::numeric_limits<double>::lowest()`, `fHi = std::numeric_limits<double>::max()`.
    /// This is the fully-open interval `[f64::MIN, f64::MAX]` — not NaN/empty.
    /// Rust equivalent: `Interval::new_default()`.
    fn infer_foreign_const_type(&self, kind: SigId) -> Result<SigType, TypeError> {
        Ok(make_simple(
            self.foreign_nature(kind),
            Variability::Konst,
            Computability::Init,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new_default(),
        ))
    }

    /// `inferFVarType`: varies by block like a UI element, executed each block.
    ///
    /// # C++ source
    /// `makeSimpleType(tree2int(type), kBlock, kExec, kVect, kNum, interval())`
    ///
    /// Same interval semantics as `inferFConstType` — see that method's doc.
    fn infer_foreign_var_type(&self, kind: SigId) -> Result<SigType, TypeError> {
        Ok(make_simple(
            self.foreign_nature(kind),
            Variability::Block,
            Computability::Exec,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new_default(),
        ))
    }

    fn infer_foreign_fun_type(&mut self, ff: SigId, args: SigId) -> Result<SigType, TypeError> {
        let ret_nature = self.foreign_fun_return_nature(ff);
        let arg_types = self.infer_list_items(args)?;
        let variability = arg_types
            .iter()
            .fold(Variability::Konst, |acc, ty| acc.join(ty.variability()));
        let computability = arg_types
            .iter()
            .fold(Computability::Comp, |acc, ty| acc.join(ty.computability()));
        let vectorability = arg_types
            .iter()
            .fold(Vectorability::Vect, |acc, ty| acc.join(ty.vectorability()));
        Ok(make_simple(
            ret_nature,
            variability,
            computability,
            vectorability,
            Boolean::Num,
            interval::Interval::new_default(),
        ))
    }

    // ── Recursive groups ─────────────────────────────────────────────────────

    /// Infer the type of a symbolic recursion head `(var, body)`.
    ///
    /// # C++ source
    /// `inferRecType` / `updateRecTypes` fixed-point loop in
    /// `sigtyperules.cpp`.
    ///
    /// Adaptation status:
    /// - active path now expects recursive groups to have been solved by the
    ///   global `annotate(...)` driver before ordinary subtree typing runs,
    /// - this method is therefore only a seeded lookup / conservative fallback,
    ///   not a local recursive solver anymore.
    fn infer_sym_rec(&mut self, sig: SigId) -> Result<SigType, TypeError> {
        let Some((var, body)) = match_sym_rec(self.arena, sig) else {
            return Ok(make_maximal());
        };
        Ok(self
            .env
            .get(&sig)
            .or_else(|| self.env.get(&var))
            .cloned()
            .unwrap_or_else(|| initial_rec_type(self.arena, body).expect("body already validated")))
    }

    /// Infer the type of projection from a tuplet recursion group.
    ///
    /// # C++ source
    /// `inferProjType(Type t, int i, int vec)` — called with `vec = kScal`.
    /// Each component is promoted by the group's variability/computability, and
    /// vectorability is promoted to kScal (the more conservative of the two).
    fn infer_proj(&mut self, idx: i32, group: SigId) -> Result<SigType, TypeError> {
        let tg = self.infer(group)?;
        let (gv, gc) = (tg.variability(), tg.computability());
        let comp = match &tg {
            SigType::Tuplet(tt) => {
                let i = usize::try_from(idx).unwrap_or(0);
                tt.components.get(i).cloned().unwrap_or_else(make_maximal)
            }
            other => other.clone(),
        };
        Ok(comp
            .promote_variability(gv)
            .promote_computability(gc)
            .promote_vectorability(Vectorability::Scal))
    }

    fn infer_list_items(&mut self, list: SigId) -> Result<Vec<SigType>, TypeError> {
        let mut items = Vec::new();
        let mut cursor = list;
        while !self.arena.is_nil(cursor) {
            let head = self.arena.hd(cursor).ok_or_else(|| {
                TypeError("malformed list payload during foreign function typing".to_owned())
            })?;
            let tail = self.arena.tl(cursor).ok_or_else(|| {
                TypeError("malformed list payload during foreign function typing".to_owned())
            })?;
            items.push(self.infer(head)?);
            cursor = tail;
        }
        Ok(items)
    }

    fn foreign_fun_return_nature(&self, ff: SigId) -> Nature {
        let Some((signature, _, _)) = match_ffunction_node(self.arena, ff) else {
            return Nature::Real;
        };
        let Some(ret_ty) = self.arena.hd(signature) else {
            return Nature::Real;
        };
        self.foreign_nature(ret_ty)
    }
}

fn match_ffunction_node(arena: &TreeArena, id: SigId) -> Option<(SigId, SigId, SigId)> {
    let node = arena.node(id)?;
    let NodeKind::Tag(tag_id) = node.kind else {
        return None;
    };
    if arena.tag_name(tag_id)? != "FFUN" {
        return None;
    }
    let [signature, incfile, libfile] = node.children.as_slice() else {
        return None;
    };
    Some((*signature, *incfile, *libfile))
}

/// C++ parity helper for the initial recursive-group approximation (`TREC`).
///
/// Each recursive component starts as an integer sample/init scalar with a
/// zero interval. The group carrier itself is a tuplet with one such component
/// per body slot.
fn initial_rec_type(arena: &TreeArena, body: SigId) -> Result<SigType, TypeError> {
    let mut items = Vec::new();
    let mut list = body;
    while !arena.is_nil(list) {
        let _head = arena.hd(list).ok_or_else(|| {
            TypeError("malformed symbolic recursion body list during type inference".to_owned())
        })?;
        let tail = arena.tl(list).ok_or_else(|| {
            TypeError("malformed symbolic recursion body list during type inference".to_owned())
        })?;
        items.push(make_simple(
            Nature::Int,
            Variability::Samp,
            Computability::Init,
            Vectorability::Scal,
            Boolean::Num,
            interval::singleton(0.0),
        ));
        list = tail;
    }
    Ok(make_tuplet(items))
}

/// C++ parity helper for the maximal recursive-group interval carrier
/// (`TRECMAX`).
///
/// This keeps the same lattice coordinates as `TREC` and only opens the
/// interval to the default full range. In the C++ implementation this upper
/// bound is used during recursive widening while all other type coordinates
/// still come from the freshly inferred recursive body.
fn maximal_rec_type(arena: &TreeArena, body: SigId) -> Result<SigType, TypeError> {
    let mut items = Vec::new();
    let mut list = body;
    while !arena.is_nil(list) {
        let _head = arena.hd(list).ok_or_else(|| {
            TypeError("malformed symbolic recursion body list during type inference".to_owned())
        })?;
        let tail = arena.tl(list).ok_or_else(|| {
            TypeError("malformed symbolic recursion body list during type inference".to_owned())
        })?;
        items.push(make_simple(
            Nature::Int,
            Variability::Samp,
            Computability::Init,
            Vectorability::Scal,
            Boolean::Num,
            interval::Interval::new_default(),
        ));
        list = tail;
    }
    Ok(make_tuplet(items))
}

fn as_tuplet_type(ty: &SigType) -> Result<&crate::types::TupletType, TypeError> {
    let SigType::Tuplet(tuplet) = ty else {
        return Err(TypeError(
            "recursive type update expected tuplet approximations".to_owned(),
        ));
    };
    Ok(tuplet)
}

/// Apply the C++ widening logic from `typeAnnotation(...)` to one recursive
/// group after `updateRecTypes(..., inter = false)` has produced a new
/// approximation.
fn apply_recursive_widening(
    prev: SigType,
    next: SigType,
    upper: SigType,
    age_min: &mut [i32],
    age_max: &mut [i32],
) -> Result<SigType, TypeError> {
    let prev = as_tuplet_type(&prev)?;
    let next = as_tuplet_type(&next)?;
    let upper = as_tuplet_type(&upper)?;

    let mut items = Vec::with_capacity(next.components.len());
    for (index, (new_comp, old_comp)) in next
        .components
        .iter()
        .zip(prev.components.iter())
        .enumerate()
    {
        let mut widened = new_comp.interval();
        let old_i = old_comp.interval();

        if widened.lo() != old_i.lo() {
            age_min[index] += 1;
            if age_min[index] > RECURSIVE_WIDENING_LIMIT {
                widened = interval::Interval::new(
                    upper.components[index].interval().lo(),
                    widened.hi(),
                    widened.lsb(),
                );
            }
        }
        if widened.hi() != old_i.hi() {
            age_max[index] += 1;
            if age_max[index] > RECURSIVE_WIDENING_LIMIT {
                widened = interval::Interval::new(
                    widened.lo(),
                    upper.components[index].interval().hi(),
                    widened.lsb(),
                );
            }
        }

        items.push(new_comp.clone().promote_interval(widened));
    }

    Ok(make_tuplet(items))
}

// ─────────────────────────────────────────────────────────────────────────────
// Table type helpers (free functions)
// ─────────────────────────────────────────────────────────────────────────────

/// Type of a table read: element type of `tbl`, with index variability joined.
///
/// # C++ source
/// `inferReadTableType(Type tbl, Type ri)`
fn infer_read_table(tbl: SigType, idx: SigType) -> Result<SigType, TypeError> {
    let content = match &tbl {
        SigType::Table(tt) => *tt.content.clone(),
        other => other.clone(),
    };
    // Raise variability/computability by the index type.
    let t = content
        .promote_variability(idx.variability())
        .promote_computability(idx.computability());
    Ok(t)
}

/// Type of a table write: preserve table type, assert write-type ≤ table nature.
///
/// # C++ source
/// `inferWriteTableType(Type tbl, Type wi, Type ws)`
fn infer_write_table_type(tbl: SigType, _wi: SigType, _ws: SigType) -> Result<SigType, TypeError> {
    // Return the table type unchanged — writes don't change the table's type.
    Ok(tbl)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use signals::SigBuilder;
    use tlib::{TreeArena, list_to_vec, sym_rec, sym_ref};
    use ui::UiProgram;

    use super::*;
    use crate::enums::*;

    fn empty_ui() -> UiProgram {
        UiProgram::empty()
    }

    fn annotate(arena: &TreeArena, outputs: &[SigId]) -> HashMap<SigId, SigType> {
        let ui = empty_ui();
        let mut ann = TypeAnnotator::new(arena, &ui);
        ann.annotate(outputs).expect("annotation failed")
    }

    fn annotate_err(arena: &TreeArena, outputs: &[SigId]) -> TypeError {
        let ui = empty_ui();
        let mut ann = TypeAnnotator::new(arena, &ui);
        ann.annotate(outputs).expect_err("annotation should fail")
    }

    #[test]
    fn discover_recursive_groups_keeps_dfs_order_and_arity() {
        let mut arena = TreeArena::new();
        let var0 = arena.symbol("W0");
        let var1 = arena.symbol("W1");

        let body0 = {
            let self_ref = sym_ref(&mut arena, var0);
            let value = SigBuilder::new(&mut arena).delay1(self_ref);
            arena.cons(value, arena.nil())
        };
        let rec0 = sym_rec(&mut arena, var0, body0);

        let body1 = {
            let self_ref = sym_ref(&mut arena, var1);
            let first = SigBuilder::new(&mut arena).delay1(self_ref);
            let second = SigBuilder::new(&mut arena).proj(0, rec0);
            let tail = arena.cons(second, arena.nil());
            arena.cons(first, tail)
        };
        let rec1 = sym_rec(&mut arena, var1, body1);
        let out0 = SigBuilder::new(&mut arena).proj(0, rec1);
        let out1 = SigBuilder::new(&mut arena).proj(0, rec0);

        let groups = discover_recursive_groups(&arena, &[out0, out1]).expect("discovery succeeds");

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].rec_sig, rec1);
        assert_eq!(groups[0].var, var1);
        assert_eq!(groups[0].arity, 2);
        assert_eq!(groups[1].rec_sig, rec0);
        assert_eq!(groups[1].var, var0);
        assert_eq!(groups[1].arity, 1);
    }

    #[test]
    fn rec_typing_state_scaffold_matches_discovered_group_shapes() {
        let mut arena = TreeArena::new();
        let var = arena.symbol("W0");
        let body = {
            let self_ref = sym_ref(&mut arena, var);
            let first = SigBuilder::new(&mut arena).delay1(self_ref);
            let second = SigBuilder::new(&mut arena).int(1);
            let tail = arena.cons(second, arena.nil());
            arena.cons(first, tail)
        };
        let rec = sym_rec(&mut arena, var, body);
        let output = SigBuilder::new(&mut arena).proj(0, rec);

        let state = RecTypingState::discover(&arena, &[output]).expect("state builds");

        assert_eq!(state.groups.len(), 1);
        assert_eq!(state.groups[0].rec_sig, rec);
        assert_eq!(state.groups[0].arity, 2);
        assert_eq!(state.current.len(), 1);
        assert_eq!(state.upper.len(), 1);
        assert_eq!(state.age_min, vec![vec![0, 0]]);
        assert_eq!(state.age_max, vec![vec![0, 0]]);
        let SigType::Tuplet(current) = &state.current[0] else {
            panic!("current recursion state should be a tuplet");
        };
        let SigType::Tuplet(upper) = &state.upper[0] else {
            panic!("upper recursion state should be a tuplet");
        };
        assert_eq!(current.components.len(), 2);
        assert_eq!(upper.components.len(), 2);
        assert_eq!(current.components[0].interval(), interval::singleton(0.0));
        assert!(upper.components[0].interval().lo().is_finite());
        assert!(upper.components[0].interval().hi().is_finite());
    }

    #[test]
    fn recursive_multiplication_converges_and_populates_projection_types() {
        let mut arena = TreeArena::new();
        let var = arena.symbol("W0");
        let body = {
            let self_ref = sym_ref(&mut arena, var);
            let left = SigBuilder::new(&mut arena).proj(0, self_ref);
            let right = SigBuilder::new(&mut arena).proj(0, self_ref);
            let product = SigBuilder::new(&mut arena).binop(BinOp::Mul, left, right);
            arena.cons(product, arena.nil())
        };
        let rec = sym_rec(&mut arena, var, body);
        let output = SigBuilder::new(&mut arena).proj(0, rec);

        let types = annotate(&arena, &[output]);

        assert!(
            types.contains_key(&output),
            "reachable recursive projection should receive a canonical type"
        );
        let body_items = list_to_vec(&arena, body).expect("body list");
        let product = body_items[0];
        assert!(
            types.contains_key(&product),
            "recursive body expression should also be typed"
        );
    }

    #[test]
    fn int_literal_is_int_konst() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let s = b.int(42);
        let types = annotate(&arena, &[s]);
        let ty = &types[&s];
        assert_eq!(ty.nature(), Nature::Int);
        assert_eq!(ty.variability(), Variability::Konst);
        assert_eq!(ty.computability(), Computability::Comp);
        assert_eq!(ty.interval(), interval::singleton(42.0));
    }

    #[test]
    #[allow(clippy::approx_constant)] // 3.14 is the test value, not an approximation of PI
    fn real_literal_is_real_konst() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let s = b.real(3.14);
        let types = annotate(&arena, &[s]);
        let ty = &types[&s];
        assert_eq!(ty.nature(), Nature::Real);
        assert_eq!(ty.variability(), Variability::Konst);
        assert_eq!(ty.interval(), interval::singleton(3.14));
    }

    #[test]
    fn input_is_real_samp() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let s = b.input(0);
        let types = annotate(&arena, &[s]);
        let ty = &types[&s];
        // C++: TINPUT = makeSimpleType(kReal, kSamp, kExec, kVect, kNum, interval(-1,1))
        assert_eq!(ty.nature(), Nature::Real);
        assert_eq!(ty.variability(), Variability::Samp);
        assert_eq!(ty.computability(), Computability::Exec);
        assert_eq!(ty.vectorability(), Vectorability::Vect);
        assert_eq!(ty.boolean(), Boolean::Num);
        // Interval must be the audio range [-1, 1], not the uninformative placeholder.
        let itv = ty.interval();
        assert_eq!(itv.lo(), -1.0);
        assert_eq!(itv.hi(), 1.0);
    }

    #[test]
    fn button_is_real_block_vect_num() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let s = b.button(0);
        let types = annotate(&arena, &[s]);
        let ty = &types[&s];
        // C++: castInterval(TGUI, ...) → kReal, kBlock, kExec, kVect, kNum, [0,1]
        assert_eq!(ty.nature(), Nature::Real);
        assert_eq!(ty.variability(), Variability::Block);
        assert_eq!(ty.computability(), Computability::Exec);
        assert_eq!(ty.vectorability(), Vectorability::Vect);
        assert_eq!(ty.boolean(), Boolean::Num);
        assert_eq!(ty.interval().lo(), 0.0);
        assert_eq!(ty.interval().hi(), 1.0);
    }

    #[test]
    fn select2_includes_cond_variability() {
        // If the cond is Samp, the result must be at least Samp even if branches are Konst.
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let cond = b.input(0); // Samp
        let s1 = b.int(0); // Konst
        let s2 = b.int(1); // Konst
        let s = b.select2(cond, s1, s2);
        let types = annotate(&arena, &[s]);
        // C++: variability = st1|st2|stsel = Konst|Konst|Samp = Samp
        assert_eq!(types[&s].variability(), Variability::Samp);
    }

    #[test]
    fn select2_all_konst_stays_konst() {
        // If all three (cond and both branches) are Konst, result should be Konst, NOT Samp.
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let cond = b.int(1); // Konst
        let s1 = b.int(0); // Konst
        let s2 = b.real(3.0); // Konst
        let s = b.select2(cond, s1, s2);
        let types = annotate(&arena, &[s]);
        // Without the samp_cast bug, a fully-constant select2 stays Konst.
        assert_eq!(types[&s].variability(), Variability::Konst);
    }

    #[test]
    fn slider_computability_is_exec() {
        // C++: TGUI = kExec → castInterval(TGUI, ...) → kExec
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let s = b.hslider(0);
        let ui = UiProgram::empty();
        let mut ann = TypeAnnotator::new(&arena, &ui);
        let types = ann.annotate(&[s]).expect("annotation failed");
        assert_eq!(types[&s].computability(), Computability::Exec);
    }

    #[test]
    fn seq_returns_type_of_second_signal() {
        // C++: T(x, env); return T(y, env)
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let x = b.int(42); // Konst Int
        let y = b.input(0); // Samp Real
        let s = b.seq(x, y);
        let types = annotate(&arena, &[s]);
        // Must be the type of y, not the union of x and y.
        assert_eq!(types[&s].nature(), Nature::Real);
        assert_eq!(types[&s].variability(), Variability::Samp);
    }

    #[test]
    fn checkbox_is_real_block_vect_num() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let s = b.checkbox(0);
        let types = annotate(&arena, &[s]);
        let ty = &types[&s];
        // C++: same contract as Button
        assert_eq!(ty.nature(), Nature::Real);
        assert_eq!(ty.variability(), Variability::Block);
        assert_eq!(ty.computability(), Computability::Exec);
        assert_eq!(ty.vectorability(), Vectorability::Vect);
        assert_eq!(ty.boolean(), Boolean::Num);
        assert_eq!(ty.interval().lo(), 0.0);
        assert_eq!(ty.interval().hi(), 1.0);
    }

    #[test]
    fn add_ints_stays_int() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let a = b.int(1);
        let bv = b.int(2);
        let s = b.binop(signals::BinOp::Add, a, bv);
        let types = annotate(&arena, &[s]);
        let ty = &types[&s];
        assert_eq!(ty.nature(), Nature::Int);
        assert_eq!(ty.interval(), interval::Interval::new(3.0, 3.0, 0));
    }

    #[test]
    fn add_int_real_promotes_to_real() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let a = b.int(1);
        let bv = b.real(0.5);
        let s = b.binop(signals::BinOp::Add, a, bv);
        let types = annotate(&arena, &[s]);
        assert_eq!(types[&s].nature(), Nature::Real);
    }

    #[test]
    fn delay1_is_samp_and_interval_includes_zero() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        // delay1 of a real constant 0.5: the result must include 0 (initial sample).
        let c = b.real(0.5);
        let s = b.delay1(c);
        let types = annotate(&arena, &[s]);
        let ty = &types[&s];
        assert_eq!(ty.variability(), Variability::Samp);
        // C++: castInterval(sampCast(t), reunion(t.interval, interval(0)))
        // singleton(0.5) ∪ singleton(0) = [0, 0.5]
        assert!(ty.interval().lo() <= 0.0, "interval lo must be ≤ 0");
        assert!(ty.interval().hi() >= 0.5, "interval hi must be ≥ 0.5");
    }

    #[test]
    fn delay1_of_negative_constant_interval_includes_zero() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let c = b.real(-1.0);
        let s = b.delay1(c);
        let types = annotate(&arena, &[s]);
        let ty = &types[&s];
        // singleton(-1) ∪ singleton(0) = [-1, 0]
        assert!(ty.interval().lo() <= -1.0);
        assert!(ty.interval().hi() >= 0.0);
    }

    #[test]
    fn fir_carrier_types_as_sample_numeric_expression() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let c0 = b.real(0.5);
        let c1 = b.hslider(0);
        let fir = b.fir(&[x, c0, c1]);

        let types = annotate(&arena, &[fir]);
        let ty = &types[&fir];

        assert_eq!(ty.nature(), Nature::Real);
        assert_eq!(ty.variability(), Variability::Samp);
        assert_eq!(ty.computability(), Computability::Exec);
        assert_eq!(ty.boolean(), Boolean::Num);
    }

    #[test]
    fn iir_carrier_types_as_sample_numeric_expression_without_typing_target_metadata() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let target = b.input(0);
        let input = b.real(0.25);
        let fb = b.hslider(0);
        let iir = b.iir(&[target, input, fb]);

        let types = annotate(&arena, &[iir]);
        let ty = &types[&iir];

        assert_eq!(ty.nature(), Nature::Real);
        assert_eq!(ty.variability(), Variability::Samp);
        assert_eq!(ty.computability(), Computability::Exec);
        assert_eq!(ty.boolean(), Boolean::Num);
    }

    #[test]
    fn int_cast_changes_nature() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let r = b.real(1.7);
        let s = b.int_cast(r);
        let types = annotate(&arena, &[s]);
        assert_eq!(types[&s].nature(), Nature::Int);
    }

    #[test]
    fn soundfile_part_interval_must_stay_within_cpp_bounds() {
        let mut arena = TreeArena::new();
        let mut b = SigBuilder::new(&mut arena);
        let soundfile = b.soundfile(0);
        let part = b.input(0);
        let length = b.soundfile_length(soundfile, part);

        let err = annotate_err(&arena, &[length]);

        assert!(
            err.0.contains("out of range soundfile part number"),
            "unexpected error: {}",
            err.0
        );
        assert!(
            err.0.contains("interval(0,255)"),
            "unexpected error: {}",
            err.0
        );
        assert!(
            err.0.contains("SIGSOUNDFILELENGTH"),
            "unexpected error: {}",
            err.0
        );
    }
}
