//! Reference BPTT executor for [`SigMatch::BlockReverseAD`] + finite-difference oracle.
#![allow(
    clippy::collapsible_if,
    clippy::doc_overindented_list_items,
    clippy::needless_range_loop,
    clippy::too_many_arguments
)]
//!
//! # Purpose
//!
//! This module implements a self-contained, tape-based Truncated Back-Propagation
//! Through Time (TBPTT) executor that operates directly on the Signal IR.  It is
//! **intentionally independent of the FIR lowering pipeline** so that Phase B3
//! (backend lowering) can be verified against this oracle without circularity.
//!
//! # TBPTT(BS, BS) semantics
//!
//! Each call to [`block_bptt_eval`] processes one non-overlapping block of
//! `block_size` samples.  No adjoint state carries across block boundaries
//! (Williams & Peng 1990 truncated variant).
//!
//! # Anti-causal carry for `Delay1`
//!
//! `Delay1(x)` at sample *n* reads *x*[*n*−1].  In the backward pass the
//! adjoint of `x` at sample *n*−1 equals the adjoint of `Delay1(x)` at sample
//! *n*.  This is implemented via a per-step `adj_carry` map: when the backward
//! step for sample *n* encounters a `Delay1` node it adds `ȳ` to
//! `adj_carry[x]`, which is then injected into `adj[x]` at the start of
//! sample *n*−1's backward step.

use std::collections::{HashMap, HashSet};

use signals::{BinOp, BlockRevPolicy, SigBuilder, SigId, SigMatch, match_sig};
use tlib::{TreeArena, list_to_vec};
use ui::ControlId;

// ── postorder traversal ──────────────────────────────────────────────────────

/// Appends the nodes reachable from `root` to `order` in postorder (children
/// before parents).  Each node appears exactly once; the `visited` set tracks
/// already-processed nodes across multiple calls for multi-output bodies.
fn collect_postorder(
    arena: &TreeArena,
    root: SigId,
    visited: &mut HashSet<SigId>,
    order: &mut Vec<SigId>,
) {
    if !visited.insert(root) {
        return;
    }
    match match_sig(arena, root) {
        SigMatch::BinOp(_, a, b)
        | SigMatch::Pow(a, b)
        | SigMatch::Min(a, b)
        | SigMatch::Max(a, b)
        | SigMatch::Atan2(a, b)
        | SigMatch::Fmod(a, b)
        | SigMatch::Remainder(a, b) => {
            collect_postorder(arena, a, visited, order);
            collect_postorder(arena, b, visited, order);
        }
        SigMatch::Sin(x)
        | SigMatch::Cos(x)
        | SigMatch::Tan(x)
        | SigMatch::Asin(x)
        | SigMatch::Acos(x)
        | SigMatch::Atan(x)
        | SigMatch::Exp(x)
        | SigMatch::Log(x)
        | SigMatch::Log10(x)
        | SigMatch::Sqrt(x)
        | SigMatch::Abs(x)
        | SigMatch::Floor(x)
        | SigMatch::Ceil(x)
        | SigMatch::Rint(x)
        | SigMatch::Round(x)
        | SigMatch::IntCast(x)
        | SigMatch::FloatCast(x)
        | SigMatch::Delay1(x) => {
            collect_postorder(arena, x, visited, order);
        }
        SigMatch::Select2(sel, a, b) => {
            collect_postorder(arena, sel, visited, order);
            collect_postorder(arena, a, visited, order);
            collect_postorder(arena, b, visited, order);
        }
        // Proj(slot, Rec(bodies)): recurse into the corresponding body expression.
        // The body contains Delay1(Proj(...)) which back-references the same group;
        // the visited guard prevents infinite recursion.
        SigMatch::Proj(slot, group_sig) => {
            if let SigMatch::Rec(body_list) = match_sig(arena, group_sig) {
                if let Some(bodies) = list_to_vec(arena, body_list) {
                    if let Some(&body) = bodies.get(slot as usize) {
                        collect_postorder(arena, body, visited, order);
                    }
                }
            }
        }
        _ => {}
    }
    order.push(root);
}

// ── forward evaluation ───────────────────────────────────────────────────────

/// Evaluates one signal node given already-computed child values `vals`,
/// per-sample audio `inputs`, UI control scalars `ui_vals`, and the
/// anti-causal `delay1_state` map (keyed by the `Delay1` node's own `SigId`).
///
/// Nodes whose kind is not listed (e.g. unknown composite nodes) evaluate
/// to `0.0`.
fn eval_sig(
    arena: &TreeArena,
    sig: SigId,
    vals: &HashMap<SigId, f64>,
    inputs: &[f64],
    ui_vals: &HashMap<ControlId, f64>,
    delay1_state: &HashMap<SigId, f64>,
) -> f64 {
    match match_sig(arena, sig) {
        SigMatch::Real(v) => v,
        SigMatch::Int(v) => f64::from(v),
        SigMatch::Input(ch) => inputs.get(ch as usize).copied().unwrap_or(0.0),
        SigMatch::HSlider(id)
        | SigMatch::VSlider(id)
        | SigMatch::NumEntry(id)
        | SigMatch::Button(id)
        | SigMatch::Checkbox(id) => ui_vals.get(&id).copied().unwrap_or(0.0),
        SigMatch::BinOp(op, a, b) => {
            let va = vals[&a];
            let vb = vals[&b];
            match op {
                BinOp::Add => va + vb,
                BinOp::Sub => va - vb,
                BinOp::Mul => va * vb,
                BinOp::Div => {
                    if vb == 0.0 {
                        0.0
                    } else {
                        va / vb
                    }
                }
                BinOp::Rem => {
                    if vb == 0.0 {
                        0.0
                    } else {
                        va % vb
                    }
                }
                BinOp::Gt => (va > vb) as i32 as f64,
                BinOp::Lt => (va < vb) as i32 as f64,
                BinOp::Ge => (va >= vb) as i32 as f64,
                BinOp::Le => (va <= vb) as i32 as f64,
                BinOp::Eq => ((va - vb).abs() < f64::EPSILON) as i32 as f64,
                BinOp::Ne => ((va - vb).abs() >= f64::EPSILON) as i32 as f64,
                _ => 0.0,
            }
        }
        SigMatch::Pow(a, b) => vals[&a].powf(vals[&b]),
        SigMatch::Min(a, b) => vals[&a].min(vals[&b]),
        SigMatch::Max(a, b) => vals[&a].max(vals[&b]),
        SigMatch::Sin(x) => vals[&x].sin(),
        SigMatch::Cos(x) => vals[&x].cos(),
        SigMatch::Tan(x) => vals[&x].tan(),
        SigMatch::Asin(x) => vals[&x].asin(),
        SigMatch::Acos(x) => vals[&x].acos(),
        SigMatch::Atan(x) => vals[&x].atan(),
        SigMatch::Atan2(a, b) => vals[&a].atan2(vals[&b]),
        SigMatch::Exp(x) => vals[&x].exp(),
        SigMatch::Log(x) => {
            let vx = vals[&x];
            if vx > 0.0 { vx.ln() } else { 0.0 }
        }
        SigMatch::Log10(x) => {
            let vx = vals[&x];
            if vx > 0.0 { vx.log10() } else { 0.0 }
        }
        SigMatch::Sqrt(x) => {
            let vx = vals[&x];
            if vx >= 0.0 { vx.sqrt() } else { 0.0 }
        }
        SigMatch::Abs(x) => vals[&x].abs(),
        SigMatch::Floor(x) => vals[&x].floor(),
        SigMatch::Ceil(x) => vals[&x].ceil(),
        SigMatch::Rint(x) | SigMatch::Round(x) => vals[&x].round(),
        SigMatch::Fmod(a, b) | SigMatch::Remainder(a, b) => {
            let vb = vals[&b];
            if vb == 0.0 { 0.0 } else { vals[&a] % vb }
        }
        SigMatch::IntCast(x) => vals[&x].trunc() as i64 as f64,
        SigMatch::FloatCast(x) => vals[&x],
        SigMatch::Select2(sel, a, b) => {
            if vals[&sel] == 0.0 {
                vals[&a]
            } else {
                vals[&b]
            }
        }
        SigMatch::Delay1(_) => delay1_state.get(&sig).copied().unwrap_or(0.0),
        // Proj(slot, Rec(bodies)): the primal value is the body expression's value.
        // The body was evaluated before this node in postorder, so vals[body] is ready.
        SigMatch::Proj(slot, group_sig) => {
            if let SigMatch::Rec(body_list) = match_sig(arena, group_sig) {
                if let Some(bodies) = list_to_vec(arena, body_list) {
                    if let Some(&body) = bodies.get(slot as usize) {
                        return vals.get(&body).copied().unwrap_or(0.0);
                    }
                }
            }
            0.0
        }
        _ => 0.0,
    }
}

// ── adjoint accumulation ─────────────────────────────────────────────────────

#[inline]
fn acc(adj: &mut HashMap<SigId, f64>, id: SigId, delta: f64) {
    *adj.entry(id).or_insert(0.0) += delta;
}

/// Propagates the adjoint `y_bar` of `sig` into its children.
///
/// Differentiable rules follow standard chain-rule identities.
/// `Delay1(x)` is anti-causal: `y_bar` is deposited into `adj_carry[x]` to
/// be injected into the adjoint of `x` at the *previous* sample's backward
/// step.
fn propagate_adj(
    arena: &TreeArena,
    sig: SigId,
    y_bar: f64,
    vals: &HashMap<SigId, f64>,
    adj: &mut HashMap<SigId, f64>,
    adj_carry: &mut HashMap<SigId, f64>,
) {
    match match_sig(arena, sig) {
        SigMatch::BinOp(BinOp::Add, a, b) => {
            acc(adj, a, y_bar);
            acc(adj, b, y_bar);
        }
        SigMatch::BinOp(BinOp::Sub, a, b) => {
            acc(adj, a, y_bar);
            acc(adj, b, -y_bar);
        }
        SigMatch::BinOp(BinOp::Mul, a, b) => {
            acc(adj, a, y_bar * vals[&b]);
            acc(adj, b, y_bar * vals[&a]);
        }
        SigMatch::BinOp(BinOp::Div, a, b) => {
            let vb = vals[&b];
            if vb != 0.0 {
                acc(adj, a, y_bar / vb);
                acc(adj, b, -y_bar * vals[&a] / (vb * vb));
            }
        }
        SigMatch::Pow(a, b) => {
            let va = vals[&a];
            let vb = vals[&b];
            if va > 0.0 {
                acc(adj, a, y_bar * vb * va.powf(vb - 1.0));
                acc(adj, b, y_bar * va.powf(vb) * va.ln());
            }
        }
        SigMatch::Min(a, b) => {
            if vals[&a] <= vals[&b] {
                acc(adj, a, y_bar);
            } else {
                acc(adj, b, y_bar);
            }
        }
        SigMatch::Max(a, b) => {
            if vals[&a] >= vals[&b] {
                acc(adj, a, y_bar);
            } else {
                acc(adj, b, y_bar);
            }
        }
        SigMatch::Sin(x) => acc(adj, x, y_bar * vals[&x].cos()),
        SigMatch::Cos(x) => acc(adj, x, -y_bar * vals[&x].sin()),
        SigMatch::Tan(x) => {
            let c = vals[&x].cos();
            acc(adj, x, y_bar / (c * c));
        }
        SigMatch::Asin(x) => {
            let vx = vals[&x];
            let d = 1.0 - vx * vx;
            if d > 0.0 {
                acc(adj, x, y_bar / d.sqrt());
            }
        }
        SigMatch::Acos(x) => {
            let vx = vals[&x];
            let d = 1.0 - vx * vx;
            if d > 0.0 {
                acc(adj, x, -y_bar / d.sqrt());
            }
        }
        SigMatch::Atan(x) => {
            let vx = vals[&x];
            acc(adj, x, y_bar / (1.0 + vx * vx));
        }
        SigMatch::Atan2(a, b) => {
            let va = vals[&a];
            let vb = vals[&b];
            let d = va * va + vb * vb;
            if d > 0.0 {
                acc(adj, a, y_bar * vb / d);
                acc(adj, b, -y_bar * va / d);
            }
        }
        SigMatch::Exp(x) => {
            acc(adj, x, y_bar * vals[&sig]);
        }
        SigMatch::Log(x) => {
            let vx = vals[&x];
            if vx > 0.0 {
                acc(adj, x, y_bar / vx);
            }
        }
        SigMatch::Log10(x) => {
            let vx = vals[&x];
            if vx > 0.0 {
                acc(adj, x, y_bar / (vx * std::f64::consts::LN_10));
            }
        }
        SigMatch::Sqrt(x) => {
            let vy = vals[&sig];
            if vy > 0.0 {
                acc(adj, x, y_bar / (2.0 * vy));
            }
        }
        SigMatch::Abs(x) => {
            let vx = vals[&x];
            if vx != 0.0 {
                acc(adj, x, y_bar * vx.signum());
            }
        }
        SigMatch::Fmod(a, _) | SigMatch::Remainder(a, _) => {
            acc(adj, a, y_bar);
        }
        SigMatch::FloatCast(x) => acc(adj, x, y_bar),
        SigMatch::Delay1(x) => {
            // Anti-causal: deposit into carry for the previous sample's backward.
            *adj_carry.entry(x).or_insert(0.0) += y_bar;
        }
        // Proj(slot, Rec(bodies)): the adjoint flows directly to bodies[slot].
        // This is correct because Proj(slot, group)[n] = bodies[slot][n] by definition,
        // so d(Proj)/d(body) = 1. The rest of the backward sweep (including the
        // Delay1 anti-causal carry inside the body) propagates the recursion adjoint.
        SigMatch::Proj(slot, group_sig) => {
            if let SigMatch::Rec(body_list) = match_sig(arena, group_sig) {
                if let Some(bodies) = list_to_vec(arena, body_list) {
                    if let Some(&body) = bodies.get(slot as usize) {
                        acc(adj, body, y_bar);
                    }
                }
            }
        }
        // Leaves and piecewise-constant nodes: no children to propagate to.
        _ => {}
    }
}

// ── BPTT result ──────────────────────────────────────────────────────────────

/// Output of [`block_bptt_eval`].
pub struct BpttResult {
    /// Primal output values `primals[primal_idx][sample]`.
    pub primals: Vec<Vec<f64>>,
    /// Per-sample adjoint of each seed signal `grads[seed_idx][sample]`.
    ///
    /// For constant seeds (sliders) the meaningful quantity is
    /// [`Self::grads_total`]; per-sample values reflect how the block
    /// objective changes when that constant is perturbed "at" that step,
    /// which equals the total when the seed appears in every sample.
    ///
    /// For audio-rate seeds (`Input(ch)`) each `grads[k][j]` is the
    /// gradient of the total block objective w.r.t. `input[ch][j]`.
    pub grads: Vec<Vec<f64>>,
    /// Sum of `grads[seed_idx]` across the block — the gradient of
    /// `Σ_n Σ_m cotangent_m * primal_m[n]` w.r.t. the k-th seed.
    pub grads_total: Vec<f64>,
}

// ── BPTT main loop ───────────────────────────────────────────────────────────

/// Runs one non-overlapping TBPTT(BS, BS) block on the given body/seed signals.
///
/// # Arguments
///
/// * `arena`        — arena owning all `SigId` nodes.
/// * `body_sigs`    — primal output signals (one per output channel).
/// * `seed_sigs`    — gradient targets; leaves of the body graph
///                    (e.g. `HSlider`, `Input`).
/// * `cotangents`   — per-primal loss weights (usually all `1.0`).
/// * `input_block`  — audio inputs `[channel][sample]`, length `block_size`.
/// * `ui_vals`      — control-id → scalar value map for slider/button nodes.
/// * `block_size`   — number of samples to evaluate.
///
/// # Panics
///
/// Panics if `body_sigs.len() != cotangents.len()` or if `input_block` lanes
/// have fewer than `block_size` samples.
pub fn block_bptt_eval(
    arena: &TreeArena,
    body_sigs: &[SigId],
    seed_sigs: &[SigId],
    cotangents: &[f64],
    input_block: &[Vec<f64>],
    ui_vals: &HashMap<ControlId, f64>,
    block_size: usize,
) -> BpttResult {
    assert_eq!(
        body_sigs.len(),
        cotangents.len(),
        "body and cotangent lengths must match"
    );
    let m = body_sigs.len();
    let n = seed_sigs.len();

    // Build a single postorder that covers all primal outputs.
    let mut visited = HashSet::new();
    let mut postorder: Vec<SigId> = Vec::new();
    for &body in body_sigs {
        collect_postorder(arena, body, &mut visited, &mut postorder);
    }

    // ── forward pass ─────────────────────────────────────────────────────────
    let mut tape: Vec<HashMap<SigId, f64>> = Vec::with_capacity(block_size);
    let mut delay1_state: HashMap<SigId, f64> = HashMap::new();
    let mut primals = vec![vec![0.0_f64; block_size]; m];

    for sample in 0..block_size {
        let sample_inputs: Vec<f64> = input_block
            .iter()
            .map(|ch| ch.get(sample).copied().unwrap_or(0.0))
            .collect();

        let mut vals: HashMap<SigId, f64> = HashMap::new();
        for &sig in &postorder {
            let v = eval_sig(arena, sig, &vals, &sample_inputs, ui_vals, &delay1_state);
            vals.insert(sig, v);
        }

        // Update Delay1 states: state[node] = x[current_sample] for next step.
        for &sig in &postorder {
            if let SigMatch::Delay1(x) = match_sig(arena, sig) {
                delay1_state.insert(sig, vals[&x]);
            }
        }

        for (i, &body) in body_sigs.iter().enumerate() {
            primals[i][sample] = vals[&body];
        }
        tape.push(vals);
    }

    // ── backward pass ────────────────────────────────────────────────────────
    let mut grads: Vec<Vec<f64>> = vec![vec![0.0_f64; block_size]; n];
    let mut adj_carry: HashMap<SigId, f64> = HashMap::new();

    for sample in (0..block_size).rev() {
        let vals = &tape[sample];

        // Collect carry from sample+1 (delay1 anti-causal flow).
        let mut adj: HashMap<SigId, f64> = std::mem::take(&mut adj_carry);

        // Cotangent injection into primal outputs.
        for (i, &body) in body_sigs.iter().enumerate() {
            *adj.entry(body).or_insert(0.0) += cotangents[i];
        }

        // Backprop through the signal graph in reverse postorder.
        for &sig in postorder.iter().rev() {
            let y_bar = adj.get(&sig).copied().unwrap_or(0.0);
            if y_bar != 0.0 {
                propagate_adj(arena, sig, y_bar, vals, &mut adj, &mut adj_carry);
            }
        }

        for (k, &seed) in seed_sigs.iter().enumerate() {
            grads[k][sample] = adj.get(&seed).copied().unwrap_or(0.0);
        }
    }

    let grads_total: Vec<f64> = grads
        .iter()
        .map(|per_sample| per_sample.iter().sum())
        .collect();

    BpttResult {
        primals,
        grads,
        grads_total,
    }
}

// ── finite-difference oracle ─────────────────────────────────────────────────

/// Runs forward-only evaluation for one block and returns the sum of all
/// primal outputs over the block (the scalar block objective).
fn forward_block_sum(
    arena: &TreeArena,
    body_sigs: &[SigId],
    cotangents: &[f64],
    input_block: &[Vec<f64>],
    ui_vals: &HashMap<ControlId, f64>,
    block_size: usize,
) -> f64 {
    let mut visited = HashSet::new();
    let mut postorder: Vec<SigId> = Vec::new();
    for &body in body_sigs {
        collect_postorder(arena, body, &mut visited, &mut postorder);
    }

    let mut total = 0.0_f64;
    let mut delay1_state: HashMap<SigId, f64> = HashMap::new();

    for sample in 0..block_size {
        let sample_inputs: Vec<f64> = input_block
            .iter()
            .map(|ch| ch.get(sample).copied().unwrap_or(0.0))
            .collect();
        let mut vals: HashMap<SigId, f64> = HashMap::new();
        for &sig in &postorder {
            let v = eval_sig(arena, sig, &vals, &sample_inputs, ui_vals, &delay1_state);
            vals.insert(sig, v);
        }
        for &sig in &postorder {
            if let SigMatch::Delay1(x) = match_sig(arena, sig) {
                delay1_state.insert(sig, vals[&x]);
            }
        }
        for (i, &body) in body_sigs.iter().enumerate() {
            total += cotangents[i] * vals[&body];
        }
    }
    total
}

fn assert_close_f64(actual: f64, expected: f64, tol: f64, label: &str) {
    let diff = (actual - expected).abs();
    let allowed = tol.max(1.0e-10_f64 * expected.abs().max(actual.abs()));
    assert!(
        diff <= allowed,
        "{label}: expected {expected:.9}, got {actual:.9}, diff {diff:.2e} > {allowed:.2e}"
    );
}

// ── tests ────────────────────────────────────────────────────────────────────

const BS: usize = 8;
const TOL: f64 = 1.0e-9;
const FD_EPS: f64 = 1.0e-5;

/// Computes a central-difference FD estimate for the block objective gradient
/// w.r.t. a constant (UI control) seed identified by `control_id`.
fn fd_const_seed(
    arena: &TreeArena,
    body_sigs: &[SigId],
    cotangents: &[f64],
    input_block: &[Vec<f64>],
    ui_vals: &HashMap<ControlId, f64>,
    block_size: usize,
    control_id: ControlId,
    eps: f64,
) -> f64 {
    let mut up = ui_vals.clone();
    let mut dn = ui_vals.clone();
    *up.entry(control_id).or_insert(0.0) += eps;
    *dn.entry(control_id).or_insert(0.0) -= eps;
    let f_up = forward_block_sum(arena, body_sigs, cotangents, input_block, &up, block_size);
    let f_dn = forward_block_sum(arena, body_sigs, cotangents, input_block, &dn, block_size);
    (f_up - f_dn) / (2.0 * eps)
}

/// Computes a forward-difference FD estimate for the block objective gradient
/// w.r.t. audio input lane `channel` at sample `sample_idx`.
///
/// This is a forward difference (not central) because perturbing a single
/// input sample only affects outputs at that sample and later, which means
/// the FD estimate via f(x+ε)−f(x) is already first-order accurate; the
/// block sum is linear in any single sample.
fn fd_audio_seed(
    arena: &TreeArena,
    body_sigs: &[SigId],
    cotangents: &[f64],
    input_block: &[Vec<f64>],
    ui_vals: &HashMap<ControlId, f64>,
    block_size: usize,
    channel: usize,
    sample_idx: usize,
    eps: f64,
) -> f64 {
    let f_base = forward_block_sum(
        arena,
        body_sigs,
        cotangents,
        input_block,
        ui_vals,
        block_size,
    );
    let mut perturbed = input_block.to_vec();
    perturbed[channel][sample_idx] += eps;
    let f_up = forward_block_sum(
        arena, body_sigs, cotangents, &perturbed, ui_vals, block_size,
    );
    (f_up - f_base) / eps
}

// ── test: constant seed, linear body ─────────────────────────────────────────

/// `body = [2*x]`, constant seed `x = 0.5`.
///
/// BPTT total gradient = 2 * BS.  FD matches.
#[test]
fn block_ref_const_seed_linear() {
    let mut arena = TreeArena::new();
    let x = SigBuilder::new(&mut arena).hslider(0);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body = SigBuilder::new(&mut arena).binop(signals::BinOp::Mul, two, x);
    let body_sigs = [body];
    let seed_sigs = [x];
    let cotangents = [1.0_f64];
    let ui_vals: HashMap<ControlId, f64> = [(0, 0.5)].into_iter().collect();
    let inputs = vec![];

    let result = block_bptt_eval(
        &arena,
        &body_sigs,
        &seed_sigs,
        &cotangents,
        &inputs,
        &ui_vals,
        BS,
    );

    // Every sample: primal = 2 * 0.5 = 1.0, per-sample grad = 2.0.
    for sample in 0..BS {
        assert_close_f64(result.primals[0][sample], 1.0, TOL, "primal");
        assert_close_f64(result.grads[0][sample], 2.0, TOL, "per-sample grad");
    }

    let fd = fd_const_seed(
        &arena,
        &body_sigs,
        &cotangents,
        &inputs,
        &ui_vals,
        BS,
        0,
        FD_EPS,
    );
    assert_close_f64(result.grads_total[0], fd, 1.0e-7, "total grad vs FD");
    assert_close_f64(
        result.grads_total[0],
        2.0 * BS as f64,
        TOL,
        "total grad exact",
    );
}

// ── test: constant seed, sin body ────────────────────────────────────────────

/// `body = [sin(x)]`, constant seed `x = 0.5`.
///
/// Per-sample grad = cos(0.5).  Total = BS * cos(0.5).
#[test]
fn block_ref_const_seed_sin() {
    let mut arena = TreeArena::new();
    let x = SigBuilder::new(&mut arena).hslider(0);
    let body = SigBuilder::new(&mut arena).sin(x);
    let body_sigs = [body];
    let seed_sigs = [x];
    let cotangents = [1.0_f64];
    let ui_vals: HashMap<ControlId, f64> = [(0, 0.5)].into_iter().collect();
    let inputs = vec![];

    let result = block_bptt_eval(
        &arena,
        &body_sigs,
        &seed_sigs,
        &cotangents,
        &inputs,
        &ui_vals,
        BS,
    );

    let expected_grad_per_sample = 0.5_f64.cos();
    for sample in 0..BS {
        assert_close_f64(result.primals[0][sample], 0.5_f64.sin(), TOL, "primal");
        assert_close_f64(
            result.grads[0][sample],
            expected_grad_per_sample,
            TOL,
            "per-sample grad",
        );
    }

    let fd = fd_const_seed(
        &arena,
        &body_sigs,
        &cotangents,
        &inputs,
        &ui_vals,
        BS,
        0,
        FD_EPS,
    );
    assert_close_f64(result.grads_total[0], fd, 1.0e-7, "total grad vs FD");
    assert_close_f64(
        result.grads_total[0],
        BS as f64 * expected_grad_per_sample,
        TOL,
        "total grad exact",
    );
}

// ── test: constant seed, delay1 body ─────────────────────────────────────────

/// `body = [delay1(x)]`, constant seed `x = 0.5`.
///
/// `delay1(x)[0] = 0`, `delay1(x)[n] = x` for n ≥ 1.  Sum = (BS-1)*x.
/// BPTT total gradient = BS-1 (anti-causal carry).  FD matches.
#[test]
fn block_ref_const_seed_delay1() {
    let mut arena = TreeArena::new();
    let x = SigBuilder::new(&mut arena).hslider(0);
    let body = SigBuilder::new(&mut arena).delay1(x);
    let body_sigs = [body];
    let seed_sigs = [x];
    let cotangents = [1.0_f64];
    let ui_vals: HashMap<ControlId, f64> = [(0, 0.5)].into_iter().collect();
    let inputs = vec![];

    let result = block_bptt_eval(
        &arena,
        &body_sigs,
        &seed_sigs,
        &cotangents,
        &inputs,
        &ui_vals,
        BS,
    );

    // Forward: sample 0 → 0.0 (initial state), samples 1..BS-1 → 0.5.
    assert_close_f64(result.primals[0][0], 0.0, TOL, "primal[0]");
    for sample in 1..BS {
        assert_close_f64(
            result.primals[0][sample],
            0.5,
            TOL,
            &format!("primal[{sample}]"),
        );
    }

    // Total gradient = BS-1 (the initial-state slot does not depend on x).
    let fd = fd_const_seed(
        &arena,
        &body_sigs,
        &cotangents,
        &inputs,
        &ui_vals,
        BS,
        0,
        FD_EPS,
    );
    assert_close_f64(result.grads_total[0], fd, 1.0e-7, "total grad vs FD");
    assert_close_f64(
        result.grads_total[0],
        (BS - 1) as f64,
        TOL,
        "total grad exact",
    );
}

// ── test: constant seed, quadratic body ──────────────────────────────────────

/// `body = [x*x]`, constant seed `x = 2.0`.
///
/// Per-sample grad = 2*x = 4.0.  Total = BS * 4.0.
#[test]
fn block_ref_const_seed_square() {
    let mut arena = TreeArena::new();
    let x = SigBuilder::new(&mut arena).hslider(0);
    let body = SigBuilder::new(&mut arena).binop(BinOp::Mul, x, x);
    let body_sigs = [body];
    let seed_sigs = [x];
    let cotangents = [1.0_f64];
    let ui_vals: HashMap<ControlId, f64> = [(0, 2.0)].into_iter().collect();
    let inputs = vec![];

    let result = block_bptt_eval(
        &arena,
        &body_sigs,
        &seed_sigs,
        &cotangents,
        &inputs,
        &ui_vals,
        BS,
    );

    for sample in 0..BS {
        assert_close_f64(result.primals[0][sample], 4.0, TOL, "primal");
        assert_close_f64(result.grads[0][sample], 4.0, TOL, "per-sample grad");
    }
    let fd = fd_const_seed(
        &arena,
        &body_sigs,
        &cotangents,
        &inputs,
        &ui_vals,
        BS,
        0,
        FD_EPS,
    );
    assert_close_f64(result.grads_total[0], fd, 1.0e-7, "total grad vs FD");
}

// ── test: audio seed, linear body ────────────────────────────────────────────

/// `body = [2 * input(0)]`, seed = `input(0)`.
///
/// For each sample j: primal = 2*inp[j], per-sample grad = 2.0.
/// FD by perturbing inp[j] also gives 2.0.
#[test]
fn block_ref_audio_seed_linear() {
    let mut arena = TreeArena::new();
    let inp = SigBuilder::new(&mut arena).input(0);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body = SigBuilder::new(&mut arena).binop(BinOp::Mul, two, inp);
    let body_sigs = [body];
    let seed_sigs = [inp];
    let cotangents = [1.0_f64];
    let ui_vals: HashMap<ControlId, f64> = HashMap::new();
    let input_vals: Vec<f64> = (0..BS).map(|i| 0.1 * i as f64).collect();
    let input_block = vec![input_vals.clone()];

    let result = block_bptt_eval(
        &arena,
        &body_sigs,
        &seed_sigs,
        &cotangents,
        &input_block,
        &ui_vals,
        BS,
    );

    for sample in 0..BS {
        assert_close_f64(
            result.primals[0][sample],
            2.0 * input_vals[sample],
            TOL,
            &format!("primal[{sample}]"),
        );
        // Per-sample grad w.r.t. input[0][sample] = 2.0.
        assert_close_f64(
            result.grads[0][sample],
            2.0,
            TOL,
            &format!("grad[{sample}]"),
        );
        // Verify against FD.
        let fd = fd_audio_seed(
            &arena,
            &body_sigs,
            &cotangents,
            &input_block,
            &ui_vals,
            BS,
            0,
            sample,
            FD_EPS,
        );
        assert_close_f64(
            result.grads[0][sample],
            fd,
            1.0e-7,
            &format!("grad[{sample}] vs FD"),
        );
    }
}

// ── test: audio seed, delay1 body ────────────────────────────────────────────

/// `body = [delay1(input(0))]`, seed = `input(0)`.
///
/// `delay1(inp)[n] = inp[n-1]`, so the gradient of the total block sum
/// w.r.t. `inp[j]` is 1 for j < BS-1 (because `delay1[j+1] = inp[j]`)
/// and 0 for j = BS-1 (its value would affect delay1[BS], outside block).
///
/// Per-sample BPTT grad[j] = 1 for j < BS-1, 0 for j = BS-1.
/// FD forward-difference confirms.
#[test]
fn block_ref_audio_seed_delay1() {
    let mut arena = TreeArena::new();
    let inp = SigBuilder::new(&mut arena).input(0);
    let body = SigBuilder::new(&mut arena).delay1(inp);
    let body_sigs = [body];
    let seed_sigs = [inp];
    let cotangents = [1.0_f64];
    let ui_vals: HashMap<ControlId, f64> = HashMap::new();
    let input_vals: Vec<f64> = (0..BS).map(|i| 0.1 * (i + 1) as f64).collect();
    let input_block = vec![input_vals.clone()];

    let result = block_bptt_eval(
        &arena,
        &body_sigs,
        &seed_sigs,
        &cotangents,
        &input_block,
        &ui_vals,
        BS,
    );

    // Primals: delay1[0] = 0 (initial state), delay1[n] = input_vals[n-1].
    assert_close_f64(result.primals[0][0], 0.0, TOL, "primal[0]");
    for sample in 1..BS {
        assert_close_f64(
            result.primals[0][sample],
            input_vals[sample - 1],
            TOL,
            &format!("primal[{sample}]"),
        );
    }

    // BPTT per-sample grads.
    for sample in 0..BS - 1 {
        assert_close_f64(
            result.grads[0][sample],
            1.0,
            TOL,
            &format!("grad[{sample}]"),
        );
        let fd = fd_audio_seed(
            &arena,
            &body_sigs,
            &cotangents,
            &input_block,
            &ui_vals,
            BS,
            0,
            sample,
            FD_EPS,
        );
        assert_close_f64(
            result.grads[0][sample],
            fd,
            1.0e-7,
            &format!("grad[{sample}] vs FD"),
        );
    }
    // Last sample: inp[BS-1] only affects delay1[BS] (outside the block).
    assert_close_f64(result.grads[0][BS - 1], 0.0, TOL, "grad[BS-1]");

    assert_close_f64(result.grads_total[0], (BS - 1) as f64, TOL, "total grad");
}

// ── test: carrier round-trip ──────────────────────────────────────────────────

/// Builds a full `SigBlockReverseAD` carrier via [`SigBuilder::block_reverse_ad`],
/// extracts body/seed/cotangent lists via [`match_sig`] + [`list_to_vec`], and
/// confirms the reference executor produces the same result as building the
/// lists manually (regression against the B0 carrier encoding).
#[test]
fn block_ref_carrier_round_trip() {
    let mut arena = TreeArena::new();

    let x = SigBuilder::new(&mut arena).hslider(0);
    let two = SigBuilder::new(&mut arena).real(2.0);
    let body_sig = SigBuilder::new(&mut arena).binop(BinOp::Mul, two, x);
    let cot_sig = SigBuilder::new(&mut arena).real(1.0);

    let carrier = SigBuilder::new(&mut arena).block_reverse_ad(
        &[body_sig],
        &[x],
        &[cot_sig],
        BlockRevPolicy::TapeFull,
    );

    let SigMatch::BlockReverseAD {
        body,
        seeds,
        cotangents,
        ..
    } = match_sig(&arena, carrier)
    else {
        panic!("carrier must decode as BlockReverseAD");
    };

    let body_sigs = list_to_vec(&arena, body).expect("body list");
    let seed_sigs = list_to_vec(&arena, seeds).expect("seed list");
    let cot_sigs = list_to_vec(&arena, cotangents).expect("cotangent list");
    let cot_vals: Vec<f64> = cot_sigs
        .iter()
        .map(|&s| match match_sig(&arena, s) {
            SigMatch::Real(v) => v,
            _ => 0.0,
        })
        .collect();

    let ui_vals: HashMap<ControlId, f64> = [(0, 0.5)].into_iter().collect();
    let result = block_bptt_eval(&arena, &body_sigs, &seed_sigs, &cot_vals, &[], &ui_vals, BS);

    // body = 2*x = 1.0; grad = 2.0 per sample; total = 2*BS.
    for sample in 0..BS {
        assert_close_f64(result.primals[0][sample], 1.0, TOL, "primal");
        assert_close_f64(result.grads[0][sample], 2.0, TOL, "grad");
    }
    assert_close_f64(result.grads_total[0], 2.0 * BS as f64, TOL, "total");
}

// ── Phase B3 FIR integration tests ───────────────────────────────────────────
//
// These tests exercise the full compiler pipeline (propagate → transform FIR
// lowering → interp backend) for `SigBlockReverseAD` programs and verify
// that the compiled gradient outputs match the B2 reference BPTT oracle.

use std::io::Cursor;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

/// Compiles `source` through the TransformFastLane and runs `frame_count`
/// frames through the interpreter.  Returns `output[channel][frame]`.
fn run_bra_source(stem: &str, source: &str, frame_count: usize) -> Vec<Vec<f32>> {
    let stem = stem.to_owned();
    let source = source.to_owned();
    std::thread::Builder::new()
        .name(format!("bra-fir-{stem}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let path = std::env::temp_dir()
                .join(format!("faust-rs-bra-{stem}-{}.dsp", std::process::id()));
            std::fs::write(&path, &source)
                .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
            let compiler = Compiler::new();
            let fbc = compiler
                .compile_file_default_to_interp_with_lane(
                    &path,
                    &InterpOptions::default(),
                    SignalFirLane::TransformFastLane,
                )
                .unwrap_or_else(|e| panic!("{} compile failed: {e}", path.display()));
            let _ = std::fs::remove_file(&path);
            let mut reader = Cursor::new(fbc);
            let mut factory = read_fbc::<f32>(&mut reader)
                .unwrap_or_else(|e| panic!("{stem} bytecode parse failed: {e}"));
            let mut instance = FbcDspInstance::new(&mut factory);
            instance.init(48_000);
            let num_outputs =
                usize::try_from(instance.get_num_outputs()).expect("non-negative outputs");
            let mut outputs = vec![vec![0.0_f32; frame_count]; num_outputs];
            let mut output_slices: Vec<&mut [f32]> =
                outputs.iter_mut().map(Vec::as_mut_slice).collect();
            instance
                .try_compute(frame_count as i32, &[], &mut output_slices)
                .unwrap_or_else(|e| panic!("{stem} execution failed: {e}"));
            outputs
        })
        .expect("spawn bra-fir worker")
        .join()
        .expect("bra-fir worker finished")
}

fn assert_close_f32(actual: f32, expected: f32, tol: f32, label: &str) {
    let diff = (actual - expected).abs();
    let allowed = tol.max(1.0e-5_f32 * expected.abs().max(actual.abs()));
    assert!(
        diff <= allowed,
        "{label}: expected {expected:.6}, got {actual:.6}, diff {diff:.2e}"
    );
}

/// `process = rad(2*x, x)` with `x = hslider("x", 0.5, …)`.
///
/// Layout: `[primal, grad_x]` per frame.
/// Expected: primal = 1.0, grad = 2.0 every frame.
///
/// This case is feed-forward with a constant seed and goes through the
/// symbolic path (not BlockReverseAD), serving as a regression guard.
#[test]
fn fir_bra_linear_const_seed_feedforward() {
    // rad(2*x, x) is purely feed-forward → symbolic RAD path, not BRA.
    // We include it as a baseline: proves the forward projection works.
    let frame_count = BS;
    let source = r#"
x = hslider("x", 0.5, 0.0, 1.0, 0.01);
process = rad(2.0 * x, x);
"#;
    let outputs = run_bra_source("fir-bra-linear", source, frame_count);
    assert_eq!(outputs.len(), 2, "layout: [primal, grad]");
    for n in 0..frame_count {
        assert_close_f32(outputs[0][n], 1.0, 1.0e-5, &format!("primal[{n}]"));
        assert_close_f32(outputs[1][n], 2.0, 1.0e-5, &format!("grad[{n}]"));
    }
}

/// An `attach` inside a temporal RAD body must preserve its metering side
/// effect in the primal pass while remaining transparent to differentiation.
///
/// The attached branch deliberately depends on the seed. If BRA propagated an
/// adjoint into that branch, the reported gradient would contain a spurious
/// `+1`. The one-sample delay forces the BlockReverseAD fallback and gives the
/// canonical TBPTT gradient `[1, ..., 1, 0]`.
#[test]
fn fir_bra_attach_ignores_attached_branch_gradient() {
    let frame_count = BS;
    let source = r#"
x = hslider("x", 0.5, 0.0, 1.0, 0.01);
meter = x : hbargraph("probe", 0.0, 1.0);
process = rad(attach(x', meter), x);
"#;
    let outputs = run_bra_source("fir-bra-attach", source, frame_count);
    assert_eq!(outputs.len(), 2, "layout: [primal, grad]");
    for n in 0..frame_count {
        let expected_primal = if n == 0 { 0.0 } else { 0.5 };
        let expected_grad = if n + 1 < frame_count { 1.0 } else { 0.0 };
        assert_close_f32(
            outputs[0][n],
            expected_primal,
            1.0e-5,
            &format!("primal[{n}]"),
        );
        assert_close_f32(outputs[1][n], expected_grad, 1.0e-5, &format!("grad[{n}]"));
    }
}

/// `process = rad(x', x)` with `x = hslider("x", 0.5, …)`.
///
/// `x'` is Faust's one-sample delay (`Delay1`).
/// Layout: `[primal, grad_x]` per frame.
/// - primal[0] = 0 (initial state), primal[n>0] = 0.5.
/// - grad[n] = 1 for n < BS-1, grad[BS-1] = 0.
///
/// The last sample's gradient is zero because `x[BS-1]` would only affect
/// `delay1(x)[BS]`, which is outside the current block (TBPTT truncation).
/// This is the canonical TBPTT anti-causal carry result confirmed by the B2
/// reference executor in [`block_ref_const_seed_delay1`].
#[test]
fn fir_bra_delay1_const_seed() {
    let frame_count = BS;
    let source = r#"
x = hslider("x", 0.5, 0.0, 1.0, 0.01);
process = rad(x', x);
"#;
    let outputs = run_bra_source("fir-bra-delay1-const", source, frame_count);
    assert_eq!(outputs.len(), 2, "layout: [primal, grad]");

    // Primal: delay1 initial state is 0; then settles to x = 0.5.
    assert_close_f32(outputs[0][0], 0.0, 1.0e-5, "primal[0]");
    for n in 1..frame_count {
        assert_close_f32(outputs[0][n], 0.5, 1.0e-5, &format!("primal[{n}]"));
    }

    // Gradient: anti-causal carry — 1.0 for n < BS-1, 0.0 for n = BS-1.
    for n in 0..frame_count - 1 {
        assert_close_f32(outputs[1][n], 1.0, 1.0e-5, &format!("grad[{n}]"));
    }
    assert_close_f32(outputs[1][frame_count - 1], 0.0, 1.0e-5, "grad[BS-1]");
}

/// `process = rad(x * x, x)` with `x = hslider("x", 2.0, …)`.
///
/// Layout: `[primal, grad_x]` per frame.
/// - primal[n] = 4.0 every frame.
/// - grad[n] = 2*x = 4.0 every frame.
///
/// The shared `x` node exercises the adjoint accumulation path: both the `lhs`
/// and `rhs` of `Mul` propagate to the same `HSlider` seed, so
/// `add_to_adjoint` builds an `Add` node for the total.
#[test]
fn fir_bra_square_const_seed() {
    let frame_count = BS;
    let source = r#"
x = hslider("x", 2.0, 0.0, 4.0, 0.01);
process = rad(x * x, x);
"#;
    let outputs = run_bra_source("fir-bra-square-const", source, frame_count);
    assert_eq!(outputs.len(), 2, "layout: [primal, grad]");
    for n in 0..frame_count {
        assert_close_f32(outputs[0][n], 4.0, 1.0e-5, &format!("primal[{n}]"));
        assert_close_f32(outputs[1][n], 4.0, 1.0e-5, &format!("grad[{n}]"));
    }
}

/// `process = rad(x' * x, x)` with `x = hslider("x", 2.0, …)`.
///
/// This is the canonical **Phase B4 tape** test: `x'` (Delay1) is not
/// trivially reverse-evaluable, so its forward value must be stored on a
/// tape during the forward loop and loaded during the backward sweep.
///
/// Layout: `[primal, grad_x]` per frame.
///
/// Forward values: `y[n] = x[n-1] * x[n] = delay1(x)[n] * x`.
/// - `y[0] = 0 * 2 = 0`   (delay initial state = 0)
/// - `y[n>0] = 2 * 2 = 4`
///
/// Backward (adjoint of `x` given cotangent 1.0 at output `y`):
/// - By the product rule:  `adj[x][n] += y_bar[n] * delay1(x)[n]`
///                                     (from the rhs position)
///                        `+ y_bar[n+1] * x[n+1]` (from the Delay1 carry at n+1)
///
/// Concretely at each reverse step n:
/// - `n = BS-1`: rhs contrib = `tape[Delay1][BS-1]` = `x[BS-2]` = 2.0;
///               lhs contrib from carry (n+1 doesn't exist) = 0.
///               → `grad[BS-1]` = 2.0
/// - `n = 1..BS-2`: rhs = tape[Delay1][n] = 2.0; lhs from carry = x[n+1] = 2.0
///               → `grad[n]` = 4.0
/// - `n = 0`: rhs = tape[Delay1][0] = 0.0 (delay initial state);
///               lhs from carry = x[1] = 2.0
///               → `grad[0]` = 2.0
///
/// Sum of gradients = 2 + 4*(BS-2) + 2 = 4*(BS-1) = 28 for BS=8.
/// This matches the finite-difference check of `L = sum_n y[n]` with
/// `dL/dx = sum_n d(y[n])/dx`.
#[test]
fn fir_bra_delay1_mul_tape() {
    let frame_count = BS;
    let source = r#"
x = hslider("x", 2.0, 0.0, 4.0, 0.01);
process = rad(x' * x, x);
"#;
    let outputs = run_bra_source("fir-bra-delay1-mul-tape", source, frame_count);
    assert_eq!(outputs.len(), 2, "layout: [primal, grad]");

    // Primal: delay1(x)[n] * x.
    assert_close_f32(outputs[0][0], 0.0, 1.0e-5, "primal[0]");
    for n in 1..frame_count {
        assert_close_f32(outputs[0][n], 4.0, 1.0e-5, &format!("primal[{n}]"));
    }

    // Gradient: per-sample adjoint of x.
    assert_close_f32(outputs[1][0], 2.0, 1.0e-5, "grad[0]");
    for n in 1..frame_count - 1 {
        assert_close_f32(outputs[1][n], 4.0, 1.0e-5, &format!("grad[{n}]"));
    }
    assert_close_f32(outputs[1][frame_count - 1], 2.0, 1.0e-5, "grad[BS-1]");
}

/// `process = rad(x@2 * x, x)` with `x = hslider("x", 2.0, …)`.
///
/// **Phase B5 tape test**: `x@2` (Delay-by-2) is not trivially reverse-evaluable,
/// so its forward value must be stored on a tape during the forward loop.
/// The backward sweep loads from the tape via `load_bra_fwd_value`, and the
/// circular carry buffer (size 2) propagates the Delay adjoint back 2 steps.
///
/// Layout: `[primal, grad_x]` per frame (BS = 8).
///
/// Forward values: `y[n] = x[n-2] * x[n]` (initial delay state = 0).
/// - `y[0] = 0 * 2 = 0`
/// - `y[1] = 0 * 2 = 0`
/// - `y[n≥2] = 2 * 2 = 4`
///
/// Gradient `dL/dx[k]` where `L = Σ_n y[n]`:
/// - `k = 0, 1`: only contributes via `x[n]` factor (rhs) at n=0,1 where
///   `delay2(x)[n] = 0`; plus via `delay2(x)[n]` factor (lhs) at n=k+2
///   where `x[k+2] = 2` → total = 0 + 2 = **2**
/// - `k = 2..5`: contributes via rhs at n=k (`delay2(x)[k] = 2`)
///   plus via lhs at n=k+2 (`x[k+2] = 2`) → total = 2 + 2 = **4**
/// - `k = 6, 7`: via rhs only (`delay2(x)[k] = 2`); n=k+2 is out of block → **2**
#[test]
fn fir_bra_delay2_mul_tape() {
    let frame_count = BS;
    let source = r#"
x = hslider("x", 2.0, 0.0, 4.0, 0.01);
process = rad(x@2 * x, x);
"#;
    let outputs = run_bra_source("fir-bra-delay2-mul-tape", source, frame_count);
    assert_eq!(outputs.len(), 2, "layout: [primal, grad]");

    // Primal: delay2(x)[n] * x[n].
    assert_close_f32(outputs[0][0], 0.0, 1.0e-5, "primal[0]");
    assert_close_f32(outputs[0][1], 0.0, 1.0e-5, "primal[1]");
    for n in 2..frame_count {
        assert_close_f32(outputs[0][n], 4.0, 1.0e-5, &format!("primal[{n}]"));
    }

    // Gradient: per-sample adjoint of x.
    assert_close_f32(outputs[1][0], 2.0, 1.0e-5, "grad[0]");
    assert_close_f32(outputs[1][1], 2.0, 1.0e-5, "grad[1]");
    for n in 2..frame_count - 2 {
        assert_close_f32(outputs[1][n], 4.0, 1.0e-5, &format!("grad[{n}]"));
    }
    assert_close_f32(outputs[1][frame_count - 2], 2.0, 1.0e-5, "grad[BS-2]");
    assert_close_f32(outputs[1][frame_count - 1], 2.0, 1.0e-5, "grad[BS-1]");
}
