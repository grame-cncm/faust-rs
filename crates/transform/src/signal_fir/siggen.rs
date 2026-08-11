//! Compile-time interpreter for computed table generators (`SIGGEN`).
//!
//! Mirrors the C++ `signal2Container()`-style behavior used for deterministic
//! 0-input table generators: evaluate the generator at compile time instead of
//! emitting runtime init code.
//!
//! # Scope
//!
//! This interpreter is used only for table generators that are known to be
//! compile-time evaluable in the fast-lane:
//!
//! - no audio inputs,
//! - no UI/runtime context dependency,
//! - deterministic signal graph,
//! - bounded expansion requested by the caller (`size` samples).
//!
//! The input signal is first normalized through `prepare_signals_for_fir` so
//! the interpreter sees the same explicit cast/promotion shape as the lowering
//! pipeline. Integer-vs-real arithmetic decisions therefore follow the reduced
//! `SimpleSigType` map produced by signal preparation.
//!
//! # State model
//!
//! The interpreter keeps just enough step state to match generator semantics:
//!
//! - recursion groups carry current/previous output vectors,
//! - `Delay1(x)` on non-recursive signals uses per-node previous/current maps,
//! - general `Delay(x, n)` uses a simple per-node history buffer,
//! - waveform and writable-table reads are interpreted directly.
//!
//! Unsupported runtime-dependent nodes fail with typed
//! `UnsupportedSignalNode` errors so the caller can reject the generator slice
//! rather than silently emitting wrong initialization data.

use std::collections::{HashMap, HashSet};

use signals::{BinOp, SigId, SigMatch, dump_sig_readable, match_sig};
use tlib::{TreeArena, list_to_vec, match_sym_rec, match_sym_ref, tree_to_str};
use ui::UiProgram;

use crate::signal_prepare::SimpleSigType;

use super::error::{SignalFirError, SignalFirErrorCode};

/// Interprets a generator signal for `size` steps, returning one `f64` sample
/// per step.
///
/// This is the entry point used by Step 2H table lowering when handling
/// `SIGWRTBL(size, SIGGEN(...), _, _)` forms that can be evaluated fully at
/// compile time.
pub(super) fn interpret_generator(
    arena: &TreeArena,
    sig: SigId,
    size: usize,
    table_init_sample_rate: Option<i32>,
) -> Result<Vec<f64>, SignalFirError> {
    let prepared =
        crate::signal_prepare::prepare_signals_for_fir(arena, &[sig], &UiProgram::empty())
            .map_err(|err| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!("SIGGEN interpreter preparation failed: {err}"),
                )
            })?;
    let prepared_sig = prepared.outputs().first().copied().ok_or_else(|| {
        SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            "SIGGEN interpreter received empty prepared output list",
        )
    })?;
    let mut interp = GeneratorInterpreter::new(
        prepared.arena(),
        prepared.types_map(),
        table_init_sample_rate,
    );
    let mut results = Vec::with_capacity(size);
    for _ in 0..size {
        let val = interp.eval(prepared_sig)?;
        results.push(val);
        interp.advance();
    }
    Ok(results)
}

/// Test-only forwarding hook so `tests.rs` can validate generator semantics
/// without exposing the interpreter outside the `signal_fir` module boundary.
#[cfg(test)]
pub(super) fn interpret_generator_for_test(
    arena: &TreeArena,
    sig: SigId,
    size: usize,
) -> Result<Vec<f64>, SignalFirError> {
    interpret_generator(arena, sig, size, None)
}

/// Small step interpreter for deterministic 0-input generator graphs.
///
/// The interpreter intentionally mirrors only the subset needed by compile-time
/// table generation, not the full runtime DSP execution model.
struct GeneratorInterpreter<'a> {
    arena: &'a TreeArena,
    types: &'a HashMap<SigId, SimpleSigType>,
    /// Recursion group state keyed by the canonical recursion binder id:
    /// `(current_values, previous_values)`.
    rec_state: HashMap<SigId, (Vec<f64>, Vec<f64>)>,
    /// Groups already evaluated at the current step to prevent infinite
    /// self-recursive expansion while still allowing `Delay1(Proj(...))` to
    /// observe the previous vector.
    evaluating: HashSet<SigId>,
    /// Current sample index in the interpreted sequence.
    step: usize,
    /// Previous-step values for non-recursive `Delay1(x)` forms.
    delay1_prev: HashMap<SigId, f64>,
    /// Current-step values that become `delay1_prev` on `advance()`.
    delay1_current: HashMap<SigId, f64>,
    /// Explicit SR supplied to `--table-init const`, if any.
    table_init_sample_rate: Option<i32>,
    /// Per-node history for general `Delay(x, n)` evaluation.
    delay_history: HashMap<SigId, Vec<f64>>,
}

/// Returns whether a foreign-constant name is Faust's `ma.SR` runtime alias.
fn is_sampling_frequency(arena: &TreeArena, name: SigId) -> bool {
    matches!(
        tree_to_str(arena, name),
        Some("fSamplingFreq" | "fSamplingRate")
    )
}

impl<'a> GeneratorInterpreter<'a> {
    /// Creates a fresh step interpreter over an already prepared generator
    /// arena and reduced type map.
    ///
    /// All runtime-like state starts zeroed so the first interpreted sample
    /// matches the compiled path's zero-initialized delay/recursion storage.
    fn new(
        arena: &'a TreeArena,
        types: &'a HashMap<SigId, SimpleSigType>,
        table_init_sample_rate: Option<i32>,
    ) -> Self {
        Self {
            arena,
            types,
            rec_state: HashMap::new(),
            evaluating: HashSet::new(),
            step: 0,
            delay1_prev: HashMap::new(),
            delay1_current: HashMap::new(),
            table_init_sample_rate,
            delay_history: HashMap::new(),
        }
    }

    /// Advances the interpreter by one sample.
    ///
    /// Recursion current vectors become previous vectors, non-recursive
    /// `Delay1` staging becomes visible as `delay1_prev`, and the step counter
    /// increments.
    fn advance(&mut self) {
        for (cur, prev) in self.rec_state.values_mut() {
            prev.clone_from(cur);
        }
        self.evaluating.clear();
        self.delay1_prev.clone_from(&self.delay1_current);
        self.delay1_current.clear();
        self.step += 1;
    }

    /// Returns the reduced prepared type of `sig`, when known.
    ///
    /// The interpreter uses this to preserve integer arithmetic semantics for
    /// generator-local nodes that stayed integer after signal preparation.
    fn simple_type(&self, sig: SigId) -> Option<SimpleSigType> {
        self.types.get(&sig).copied()
    }

    /// Evaluates one generator signal node for the current step.
    ///
    /// This covers the compile-time-supported generator vocabulary:
    /// constants, arithmetic, casts, math intrinsics, delays, recursion, and
    /// table reads. Nodes that depend on runtime DSP context are rejected.
    fn eval(&mut self, sig: SigId) -> Result<f64, SignalFirError> {
        if let Some((var, body)) = match_sym_rec(self.arena, sig) {
            return self.eval_rec_and_project(var, Some(body), 0);
        }
        if let Some(var) = match_sym_ref(self.arena, sig) {
            return self.read_rec_current(var, 0);
        }

        match match_sig(self.arena, sig) {
            SigMatch::Int(v) => Ok(v as f64),
            SigMatch::Real(v) => Ok(v),
            SigMatch::BinOp(op, x, y) => {
                let lhs = self.eval(x)?;
                let rhs = self.eval(y)?;
                Ok(self.eval_binop(sig, op, lhs, rhs))
            }
            SigMatch::Pow(x, y) => Ok(self.eval(x)?.powf(self.eval(y)?)),
            SigMatch::Min(x, y) => Ok(self.eval(x)?.min(self.eval(y)?)),
            SigMatch::Max(x, y) => Ok(self.eval(x)?.max(self.eval(y)?)),
            SigMatch::FloatCast(x) => self.eval(x),
            SigMatch::IntCast(x) => Ok((self.eval(x)? as i32) as f64),
            SigMatch::BitCast(x) => {
                let v = self.eval(x)?;
                Ok((v as f32).to_bits() as i32 as f64)
            }
            SigMatch::Sin(x) => Ok(self.eval(x)?.sin()),
            SigMatch::Cos(x) => Ok(self.eval(x)?.cos()),
            SigMatch::Tan(x) => Ok(self.eval(x)?.tan()),
            SigMatch::Asin(x) => Ok(self.eval(x)?.asin()),
            SigMatch::Acos(x) => Ok(self.eval(x)?.acos()),
            SigMatch::Atan(x) => Ok(self.eval(x)?.atan()),
            SigMatch::Exp(x) => Ok(self.eval(x)?.exp()),
            SigMatch::Exp10(x) => Ok(10.0_f64.powf(self.eval(x)?)),
            SigMatch::Log(x) => Ok(self.eval(x)?.ln()),
            SigMatch::Log10(x) => Ok(self.eval(x)?.log10()),
            SigMatch::Sqrt(x) => Ok(self.eval(x)?.sqrt()),
            SigMatch::Abs(x) => Ok(self.eval(x)?.abs()),
            SigMatch::Floor(x) => Ok(self.eval(x)?.floor()),
            SigMatch::Ceil(x) => Ok(self.eval(x)?.ceil()),
            SigMatch::Rint(x) => Ok(self.eval(x)?.round_ties_even()),
            SigMatch::Round(x) => Ok(self.eval(x)?.round()),
            SigMatch::Atan2(x, y) => Ok(self.eval(x)?.atan2(self.eval(y)?)),
            SigMatch::Fmod(x, y) => {
                let lhs = self.eval(x)?;
                let rhs = self.eval(y)?;
                Ok(if rhs == 0.0 { 0.0 } else { lhs % rhs })
            }
            SigMatch::Remainder(x, y) => {
                let lhs = self.eval(x)?;
                let rhs = self.eval(y)?;
                Ok(if rhs == 0.0 {
                    0.0
                } else {
                    lhs - (lhs / rhs).round() * rhs
                })
            }
            SigMatch::Select2(sel, s1, s2) => {
                if self.eval(sel)? != 0.0 {
                    self.eval(s2)
                } else {
                    self.eval(s1)
                }
            }
            SigMatch::Delay1(x) => self.eval_delay1(x),
            SigMatch::Delay(value, amount) => self.eval_delay(sig, value, amount),
            SigMatch::Prefix(init, value) => {
                if self.step == 0 {
                    self.eval(init)
                } else {
                    self.eval(value)
                }
            }
            SigMatch::Proj(idx, group) => self.eval_proj(idx, group),
            SigMatch::Rec(_body) => self.eval_proj(0, sig),
            SigMatch::Gen(inner) => self.eval(inner),
            SigMatch::Output(_, inner) => self.eval(inner),
            SigMatch::Lowest(x) | SigMatch::Highest(x) => self.eval(x),
            SigMatch::Attach(x, _) => self.eval(x),
            SigMatch::Enable(x, _) => self.eval(x),
            SigMatch::Control(x, _) => self.eval(x),
            SigMatch::RdTbl(tbl, idx) => self.eval_rdtbl(tbl, idx),
            SigMatch::Waveform(values) => {
                if values.is_empty() {
                    Ok(0.0)
                } else {
                    self.eval(values[self.step % values.len()])
                }
            }
            SigMatch::Input(_) => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                "SIGGEN interpreter: Input not allowed (generators are 0-input)",
            )),
            SigMatch::Button(_)
            | SigMatch::Checkbox(_)
            | SigMatch::VSlider(_)
            | SigMatch::HSlider(_)
            | SigMatch::NumEntry(_) => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                "SIGGEN interpreter: UI controls not allowed in generators",
            )),
            SigMatch::VBargraph(_, _) | SigMatch::HBargraph(_, _) => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                "SIGGEN interpreter: bargraphs not allowed in generators",
            )),
            SigMatch::Soundfile(_)
            | SigMatch::SoundfileLength(_, _)
            | SigMatch::SoundfileRate(_, _)
            | SigMatch::SoundfileBuffer(_, _, _, _) => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                "SIGGEN interpreter: soundfile access not allowed in generators",
            )),
            SigMatch::FConst(_, name, _) if is_sampling_frequency(self.arena, name) => {
                match self.table_init_sample_rate {
                    Some(sample_rate) if sample_rate > 0 => Ok(f64::from(sample_rate)),
                    Some(_) => Err(SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        "SIGGEN interpreter: --table-init-sample-rate must be a positive integer",
                    )),
                    None => Err(SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        "SIGGEN interpreter: ma.SR requires --table-init-sample-rate when --table-init const is selected",
                    )),
                }
            }
            SigMatch::FConst(_, _, _) | SigMatch::FVar(_, _, _) | SigMatch::FFun(_, _) => {
                Err(SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    "SIGGEN interpreter: foreign functions/constants/variables not supported",
                ))
            }
            _ => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "SIGGEN interpreter: unsupported signal node (expr={})",
                    dump_sig_readable(self.arena, sig)
                ),
            )),
        }
    }

    /// Evaluates a recursive projection `Proj(idx, group)` and returns the
    /// current-step value of the requested output slot.
    fn eval_proj(&mut self, idx: i32, group: SigId) -> Result<f64, SignalFirError> {
        let i = idx as usize;
        if let Some((var, body)) = match_sym_rec(self.arena, group) {
            return self.eval_rec_and_project(var, Some(body), i);
        }
        if let Some(var) = match_sym_ref(self.arena, group) {
            return self.read_rec_current(var, i);
        }
        if let SigMatch::Rec(body) = match_sig(self.arena, group) {
            return self.eval_rec_and_project(group, Some(body), i);
        }
        Err(SignalFirError::new(
            SignalFirErrorCode::UnsupportedSignalNode,
            format!(
                "SIGGEN interpreter: Proj target is not a recursion group (expr={})",
                dump_sig_readable(self.arena, group)
            ),
        ))
    }

    /// Materializes a recursion group for the current step if needed, then
    /// returns the requested output slot.
    ///
    /// Groups are evaluated at most once per interpreted step; later reads in
    /// the same step reuse the cached current vector.
    fn eval_rec_and_project(
        &mut self,
        var: SigId,
        body: Option<SigId>,
        idx: usize,
    ) -> Result<f64, SignalFirError> {
        if !self.rec_state.contains_key(&var) {
            let n = if let Some(body) = body {
                self.collect_cons_list(body).len().max(1)
            } else {
                1
            };
            self.rec_state.insert(var, (vec![0.0; n], vec![0.0; n]));
        }

        if !self.evaluating.contains(&var)
            && let Some(body) = body
        {
            self.evaluating.insert(var);
            let body_signals = self.collect_cons_list(body);
            let mut new_values = Vec::with_capacity(body_signals.len());
            for sig in &body_signals {
                new_values.push(self.eval(*sig)?);
            }
            if let Some((cur, _)) = self.rec_state.get_mut(&var) {
                *cur = new_values;
            }
        }

        let (cur, _) = &self.rec_state[&var];
        let canonical_index = if cur.len() == 1 { 0 } else { idx };
        if canonical_index < cur.len() {
            Ok(cur[canonical_index])
        } else {
            Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "SIGGEN interpreter: Proj index {} out of range (group has {} outputs)",
                    idx,
                    cur.len()
                ),
            ))
        }
    }

    /// Evaluates `Delay1(x)` using either recursion previous-state reads or a
    /// generic per-node one-sample memory for non-recursive signals.
    /// Reads the previous-step value of `x` for `Delay1(x)`.
    ///
    /// Recursion-backed `Delay1` reads from the recursion previous vector,
    /// while non-recursive `Delay1` uses the generic one-sample staging maps.
    fn eval_delay1(&mut self, x: SigId) -> Result<f64, SignalFirError> {
        if let Some(var) = match_sym_ref(self.arena, x) {
            return self.read_rec_prev(var, 0);
        }
        if let SigMatch::Proj(idx, group) = match_sig(self.arena, x) {
            if let Some((var, body)) = match_sym_rec(self.arena, group) {
                let prev_val = self.read_rec_prev(var, idx as usize)?;
                let _ = self.eval_rec_and_project(var, Some(body), idx as usize);
                return Ok(prev_val);
            }
            if let Some(var) = match_sym_ref(self.arena, group) {
                return self.read_rec_prev(var, idx as usize);
            }
            if let SigMatch::Rec(body) = match_sig(self.arena, group) {
                let prev_val = self.read_rec_prev(group, idx as usize)?;
                let _ = self.eval_rec_and_project(group, Some(body), idx as usize);
                return Ok(prev_val);
            }
        }
        let prev = self.delay1_prev.get(&x).copied().unwrap_or(0.0);
        if !self.delay1_current.contains_key(&x) {
            let current = self.eval(x)?;
            self.delay1_current.insert(x, current);
        }
        Ok(prev)
    }

    /// Reads the current-step value of a recursion group output.
    ///
    /// Missing state defaults to `0.0`, matching the zero-initialized state
    /// convention used by the compiled path.
    fn read_rec_current(&self, var: SigId, idx: usize) -> Result<f64, SignalFirError> {
        if let Some((cur, _)) = self.rec_state.get(&var) {
            let canonical_index = if cur.len() == 1 { 0 } else { idx };
            if canonical_index < cur.len() {
                return Ok(cur[canonical_index]);
            }
        }
        Ok(0.0)
    }

    /// Reads the previous-step value of a recursion group output, defaulting to
    /// `0.0` before the group has been initialized.
    fn read_rec_prev(&self, var: SigId, idx: usize) -> Result<f64, SignalFirError> {
        if let Some((_, prev)) = self.rec_state.get(&var) {
            let canonical_index = if prev.len() == 1 { 0 } else { idx };
            if canonical_index < prev.len() {
                return Ok(prev[canonical_index]);
            }
        }
        Ok(0.0)
    }

    /// Evaluates a general multi-sample `Delay(value, amount)` using a simple
    /// per-node history vector.
    fn eval_delay(
        &mut self,
        sig: SigId,
        value: SigId,
        amount: SigId,
    ) -> Result<f64, SignalFirError> {
        let n = self.eval(amount)? as usize;
        let current = self.eval(value)?;
        let history = self.delay_history.entry(sig).or_default();
        history.push(current);
        if n == 0 {
            Ok(current)
        } else if history.len() > n {
            Ok(history[history.len() - 1 - n])
        } else {
            Ok(0.0)
        }
    }

    /// Evaluates a table read from either a generated writable table or a
    /// literal waveform, using wrapped integer indexing.
    /// Interprets `RdTbl(tbl, idx)` for compile-time-readable table sources.
    ///
    /// Generated writable tables are expanded on demand by recursively
    /// interpreting their generator expression; waveform tables are read
    /// directly from their literal element list.
    fn eval_rdtbl(&mut self, tbl: SigId, idx: SigId) -> Result<f64, SignalFirError> {
        let index = self.eval(idx)? as i32;
        match match_sig(self.arena, tbl) {
            SigMatch::WrTbl(size_sig, gen_sig, _, _) => {
                let size = self.eval(size_sig)? as usize;
                if size == 0 {
                    return Ok(0.0);
                }
                let table =
                    interpret_generator(self.arena, gen_sig, size, self.table_init_sample_rate)?;
                let i = ((index % size as i32) + size as i32) as usize % size;
                Ok(table[i])
            }
            SigMatch::Waveform(values) => {
                if values.is_empty() {
                    return Ok(0.0);
                }
                let len = values.len();
                let i = ((index % len as i32) + len as i32) as usize % len;
                self.eval(values[i])
            }
            _ => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "SIGGEN interpreter: RdTbl source not supported (expr={})",
                    dump_sig_readable(self.arena, tbl)
                ),
            )),
        }
    }

    /// Flattens a recursion-group body list into output expressions.
    fn collect_cons_list(&self, body: SigId) -> Vec<SigId> {
        if let Some(elements) = list_to_vec(self.arena, body)
            && !elements.is_empty()
        {
            return elements;
        }
        vec![body]
    }

    /// Evaluates a binary operation while preserving prepared integer
    /// arithmetic for generator-local integer nodes.
    /// Evaluates integer-preserving binary operators using the prepared result
    /// type of the enclosing signal node.
    ///
    /// This mirrors the fast-lane contract after `prepare_signals_for_fir`,
    /// where explicit casts already encode the promotion policy and remaining
    /// integer nodes must keep wrapping `i32` behavior.
    fn eval_binop(&self, sig: SigId, op: BinOp, lhs: f64, rhs: f64) -> f64 {
        let result_ty = self.simple_type(sig);
        match op {
            BinOp::Add if result_ty == Some(SimpleSigType::Int) => {
                (lhs as i32).wrapping_add(rhs as i32) as f64
            }
            BinOp::Sub if result_ty == Some(SimpleSigType::Int) => {
                (lhs as i32).wrapping_sub(rhs as i32) as f64
            }
            BinOp::Mul if result_ty == Some(SimpleSigType::Int) => {
                (lhs as i32).wrapping_mul(rhs as i32) as f64
            }
            BinOp::Rem if result_ty == Some(SimpleSigType::Int) => {
                let rhs_i = rhs as i32;
                if rhs_i == 0 {
                    0.0
                } else {
                    ((lhs as i32) % rhs_i) as f64
                }
            }
            _ => eval_binop(op, lhs, rhs),
        }
    }
}

/// Evaluates a binary operator on `f64` values using Faust signal semantics.
///
/// Division and remainder by zero return `0.0`. Bitwise and shift operators
/// truncate to integer operands and widen the result back to `f64`.
fn eval_binop(op: BinOp, lhs: f64, rhs: f64) -> f64 {
    match op {
        BinOp::Add => lhs + rhs,
        BinOp::Sub => lhs - rhs,
        BinOp::Mul => lhs * rhs,
        BinOp::Div => {
            if rhs == 0.0 {
                0.0
            } else {
                lhs / rhs
            }
        }
        BinOp::Rem => {
            if rhs == 0.0 {
                0.0
            } else {
                lhs % rhs
            }
        }
        BinOp::Lsh => ((lhs as i32) << (rhs as i32)) as f64,
        BinOp::ARsh => ((lhs as i32) >> (rhs as i32)) as f64,
        BinOp::LRsh => ((lhs as u32) >> (rhs as u32)) as f64,
        BinOp::Gt => (lhs > rhs) as i32 as f64,
        BinOp::Lt => (lhs < rhs) as i32 as f64,
        BinOp::Ge => (lhs >= rhs) as i32 as f64,
        BinOp::Le => (lhs <= rhs) as i32 as f64,
        BinOp::Eq => (lhs == rhs) as i32 as f64,
        BinOp::Ne => (lhs != rhs) as i32 as f64,
        BinOp::And => ((lhs as i32) & (rhs as i32)) as f64,
        BinOp::Or => ((lhs as i32) | (rhs as i32)) as f64,
        BinOp::Xor => ((lhs as i32) ^ (rhs as i32)) as f64,
    }
}
