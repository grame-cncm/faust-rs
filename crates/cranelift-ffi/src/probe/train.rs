//! A gradient-descent driver over a probe: the host of the `rad` programs
//! whose loss and gradients leave the graph, at the command line.
//!
//! Such a program (`tests/corpus/ddsp_rad_host_block_resonator.dsp`,
//! `ddsp_rad_gru_amp_host.dsp`) outputs, per sample, a loss and the
//! contributions of its gradient with respect to some controls; the block
//! reverse sweep of `rad` makes the sum of a lane over a `compute` block the
//! gradient of the block's loss. What the program cannot do is close the
//! loop: read those lanes, step the controls, run the next block. This
//! module does it with the two operations a host has, `set_exact` on the
//! controls and `compute_raw` on a block, the state of the instance carried
//! from block to block (truncated backpropagation through time for a
//! recurrent model) or, with `reset_per_block`, cleared before each block so
//! that every block replays the same response from silence (an offline
//! calibration, one epoch per block), and checks the gradient lanes against central finite
//! differences on the controls, the first thing to do when a gradient looks
//! wrong.
//!
//! # What the descent reports about itself
//!
//! A loss and the controls are not enough to read a descent. Three things were
//! learned by reading tables afterwards, and are now said by the loop:
//!
//! - a control that ends **on a bound** is stopped, not converged: keeping the
//!   controls in their range is part of the algorithm, and its having been
//!   active is information ([`BoundStats`]);
//! - a flat landscape and a vanishing step look the same in the controls: the
//!   mean gradient of each block tells them apart ([`TrainStep::grads`]);
//! - the last loss is not the best one when the descent left a minimum: the
//!   lowest block loss, its block and the controls it was computed with are
//!   kept ([`Trained::min_loss`]).
//!
//! [`grid`] evaluates the loss of one block over a grid of starting points,
//! the first half of the grid-then-gradient that a non-convex loss needs, and
//! [`fd_check`] runs at any point, the trained one included: a descent is
//! finished where the gradient is small *and* right.

use std::rc::Rc;
use std::time::Instant;

use crate::probe::engine::{Factory, Probe};
use crate::probe::params::Resolution;
use crate::probe::render::InputMode;
use crate::probe::timing::{BlockTimer, Timing};

/// The update rule applied to every trained control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Optimizer {
    /// `p <- p - lr g`.
    Sgd,
    /// Adam (Kingma & Ba 2015) with bias correction.
    Adam { b1: f64, b2: f64, eps: f64 },
}

impl Optimizer {
    /// The paper's Adam: betas 0.9 and 0.999, epsilon 1e-8.
    pub const ADAM: Self = Self::Adam {
        b1: 0.9,
        b2: 0.999,
        eps: 1e-8,
    };
}

/// What to train and how.
#[derive(Clone, Debug)]
pub struct TrainSpec {
    /// The controls, exact paths or unique suffixes (`a1` for `/x/a1`), in
    /// the order of their gradient lanes.
    pub params: Vec<String>,
    /// The output lane holding the per-sample loss.
    pub loss_lane: usize,
    /// The output lane of the first control's gradient; the others follow.
    pub first_grad_lane: usize,
    pub optimizer: Optimizer,
    pub lr: f64,
    /// Frames per block: the length of the reverse sweep, and of one step.
    pub block: usize,
    /// Number of blocks, one step each.
    pub blocks: usize,
    /// The excitation, position-addressed so that the blocks are contiguous.
    pub input: InputMode,
    /// Start every block from a cleared state and from frame 0 of the
    /// excitation: each block is then one pass over the same response (an
    /// offline calibration, one epoch per block) instead of the next stretch
    /// of a stream.
    pub reset_per_block: bool,
    /// Controls written on every instance before a block is computed, as
    /// (exact path or unique suffix, value): `--set`. For a trained control
    /// the value is the descent's starting point instead of the widget's
    /// initial value; for any other control it is a fixed value, rewritten
    /// after every reset (the reset restores the widgets' defaults).
    pub sets: Vec<(String, f64)>,
}

/// One step of the training, reported as it happens.
#[derive(Clone, Debug)]
pub struct TrainStep {
    /// 1-based block index.
    pub block: usize,
    /// The block's mean loss.
    pub loss: f64,
    /// The block's mean gradient, one per control: what the step was computed
    /// from, at the controls the block ran with.
    pub grads: Vec<f64>,
    /// The controls after the step.
    pub params: Vec<f64>,
}

/// Which end of its range a control sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    Lower,
    Upper,
}

impl Bound {
    /// `lower` or `upper`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Lower => "lower",
            Self::Upper => "upper",
        }
    }
}

/// How often the projection onto its range held a control on a bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoundStats {
    /// Blocks whose step left the control on its lower bound.
    pub on_lower: usize,
    /// Blocks whose step left the control on its upper bound.
    pub on_upper: usize,
    /// The bound the control is on after the last step.
    pub ends_on: Option<Bound>,
}

/// The outcome of a training.
#[derive(Clone, Debug)]
pub struct Trained {
    pub paths: Vec<String>,
    pub values: Vec<f64>,
    /// The range each control was kept in.
    pub ranges: Vec<(f64, f64)>,
    /// What the projection onto that range did, per control.
    pub bounds: Vec<BoundStats>,
    pub first_loss: f64,
    pub last_loss: f64,
    /// The lowest block loss, the first block that reached it (1-based), and
    /// the controls that block was computed with: the best point the descent
    /// visited, which the last one need not be.
    pub min_loss: f64,
    pub min_loss_block: usize,
    pub values_at_min: Vec<f64>,
    /// What the blocks cost: the `compute` calls, not the optimiser.
    pub timing: Timing,
}

/// The loss of one block over a grid of starting points.
#[derive(Clone, Debug)]
pub struct Grid {
    /// The exact paths of the swept controls, in the order of the axes.
    pub paths: Vec<String>,
    /// One entry per point, the last axis varying fastest: the swept
    /// controls' values and the block's mean loss there (not finite when the
    /// program diverges at that point).
    pub points: Vec<(Vec<f64>, f64)>,
    /// Index of the point with the lowest finite loss, the first on a tie.
    pub best: usize,
}

impl Grid {
    /// The swept controls at the best point, as `--set` assignments.
    #[must_use]
    pub fn best_assignments(&self) -> Vec<(String, f64)> {
        self.paths
            .iter()
            .cloned()
            .zip(self.points[self.best].0.iter().copied())
            .collect()
    }
}

/// One control's gradient lane against finite differences, on one block.
#[derive(Clone, Debug)]
pub struct FdCheck {
    pub path: String,
    /// The gradient lane summed over the block.
    pub rad: f64,
    /// The reference: the central differences of the summed loss lane at the
    /// step and at half of it, extrapolated to a zero step (Richardson),
    /// `(4 D(h/2) - D(h)) / 3`.
    ///
    /// A central difference is off by the loss's third derivative times
    /// `h^2 / 6`, which does not shrink with the gradient: at the end of a
    /// descent, where the gradient is small by construction, that error is
    /// all that is left, and the plain difference reports it as the
    /// gradient's. The extrapolation cancels the `h^2` term and leaves one in
    /// `h^4`.
    pub fd: f64,
    /// The plain central difference at the step, `D(h)`: what `fd` was before
    /// the extrapolation. By how much the two differ is the error the plain
    /// difference had.
    pub fd_plain: f64,
    /// `|rad - fd| / max(|fd|, 1)`.
    pub relative_error: f64,
}

/// A trained control: its path and the range it is kept in.
struct Param {
    path: String,
    min: f64,
    max: f64,
    init: f64,
}

/// The exact path of the control a query names.
fn resolve_path(probe: &Probe, query: &str) -> Result<String, String> {
    match probe.controls().resolve(query) {
        Resolution::Unique(control) => Ok(control.path.clone()),
        Resolution::NotFound => Err(format!("no control matching `{query}`")),
        Resolution::Ambiguous(candidates) => Err(format!(
            "`{query}` is ambiguous, matches: {}",
            candidates.join(", ")
        )),
    }
}

/// The trained controls, starting from their widgets' initial values, or
/// from the value `sets` gives them (clamped to their range).
fn resolve_params(
    probe: &Probe,
    queries: &[String],
    sets: &[(String, f64)],
) -> Result<Vec<Param>, String> {
    if queries.is_empty() {
        return Err("--train needs at least one control".to_owned());
    }
    let mut params = queries
        .iter()
        .map(|query| match probe.controls().resolve(query) {
            Resolution::Unique(control) => {
                if !control.kind.is_writable() {
                    return Err(format!(
                        "`{}` is a bargraph, not a control to train",
                        control.path
                    ));
                }
                Ok(Param {
                    path: control.path.clone(),
                    min: control.min,
                    max: control.max,
                    init: control.init,
                })
            }
            Resolution::NotFound => Err(format!("no control matching `{query}`")),
            Resolution::Ambiguous(candidates) => Err(format!(
                "`{query}` is ambiguous, matches: {}",
                candidates.join(", ")
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (query, value) in sets {
        let path = resolve_path(probe, query)?;
        if let Some(param) = params.iter_mut().find(|p| p.path == path) {
            // the control's own clamp: a value typed on a decimal bound
            // (`0.7` against a bound that reached the host as the `f32`
            // nearest to it) is in range and stays what was typed
            param.init = probe
                .controls()
                .get(&path)
                .map_or(*value, |control| control.clamp(*value));
        }
    }
    Ok(params)
}

/// Writes the `--set` controls on an instance: the fixed values of the
/// controls that are not trained (the trained ones are rewritten by the
/// caller just after).
fn apply_sets(probe: &Probe, sets: &[(String, f64)]) -> Result<(), String> {
    for (query, value) in sets {
        probe.set(query, *value)?;
    }
    Ok(())
}

fn check_lanes(probe: &Probe, spec: &TrainSpec) -> Result<(), String> {
    let needed = spec.first_grad_lane + spec.params.len();
    if spec.loss_lane >= probe.outputs() || needed > probe.outputs() {
        return Err(format!(
            "the program has {} outputs: the loss lane {} and the gradient lanes {}..{} do not fit",
            probe.outputs(),
            spec.loss_lane,
            spec.first_grad_lane,
            needed - 1
        ));
    }
    Ok(())
}

/// The excitation for frames `start .. start + frames`, one vector per input.
fn input_block(
    input: &InputMode,
    channels: usize,
    start: usize,
    frames: usize,
    sample_rate: f64,
) -> Vec<Vec<f64>> {
    (0..channels)
        .map(|ch| {
            (0..frames)
                .map(|i| input.sample(ch, start + i, sample_rate))
                .collect()
        })
        .collect()
}

fn lane_sums(outs: &[Vec<f64>]) -> Vec<f64> {
    outs.iter().map(|lane| lane.iter().sum::<f64>()).collect()
}

/// Trains `spec.params` on one instance kept running: per block, the
/// controls are written, the block computed, the loss and gradient lanes
/// averaged, and the optimiser steps the controls, clamped to their range.
pub fn train(
    factory: &Rc<Factory>,
    sample_rate: i32,
    spec: &TrainSpec,
    mut on_step: impl FnMut(&TrainStep),
) -> Result<Trained, String> {
    if spec.block == 0 || spec.blocks == 0 {
        return Err("--train needs a block size and a number of blocks".to_owned());
    }
    let probe = Probe::instantiate(factory, sample_rate)?;
    let params = resolve_params(&probe, &spec.params, &spec.sets)?;
    check_lanes(&probe, spec)?;
    let mut values: Vec<f64> = params.iter().map(|p| p.init).collect();
    let (mut m, mut v) = (vec![0.0_f64; params.len()], vec![0.0_f64; params.len()]);
    let (mut first_loss, mut last_loss) = (0.0, 0.0);
    let mut min_loss = f64::INFINITY;
    let mut min_loss_block = 0;
    let mut values_at_min = values.clone();
    let mut bounds = vec![BoundStats::default(); params.len()];
    let mut timer = BlockTimer::new(f64::from(sample_rate));
    let n = spec.block as f64;
    for iteration in 1..=spec.blocks {
        if spec.reset_per_block {
            // controls back to their defaults and the state cleared; the
            // `--set` controls and the trained ones are rewritten just below
            probe.reset();
        }
        apply_sets(&probe, &spec.sets)?;
        for (param, &value) in params.iter().zip(&values) {
            probe.set_exact(&param.path, value)?;
        }
        let start = if spec.reset_per_block {
            0
        } else {
            (iteration - 1) * spec.block
        };
        let x = input_block(
            &spec.input,
            probe.inputs(),
            start,
            spec.block,
            f64::from(sample_rate),
        );
        let started = Instant::now();
        let outs = probe.compute_raw(&x, spec.block);
        // a position in what was computed, whatever the excitation replays:
        // under `reset_per_block` every block starts at frame 0 of it
        timer.record((iteration - 1) * spec.block, spec.block, started.elapsed());
        let sums = lane_sums(&outs);
        let loss = sums[spec.loss_lane] / n;
        if !loss.is_finite() {
            let then: Vec<String> = params
                .iter()
                .zip(&values)
                .map(|(param, value)| format!("{}={value}", param.path))
                .collect();
            return Err(format!(
                "the loss is not finite at block {iteration}\n  controls of that block: {}",
                then.join(" ")
            ));
        }
        if iteration == 1 {
            first_loss = loss;
        }
        last_loss = loss;
        if loss < min_loss {
            // the controls this block ran with, not those its step leads to
            min_loss = loss;
            min_loss_block = iteration;
            values_at_min.clone_from(&values);
        }
        let mut grads = Vec::with_capacity(params.len());
        for (k, param) in params.iter().enumerate() {
            let g = sums[spec.first_grad_lane + k] / n;
            grads.push(g);
            let step = match spec.optimizer {
                Optimizer::Sgd => spec.lr * g,
                Optimizer::Adam { b1, b2, eps } => {
                    m[k] = b1 * m[k] + (1.0 - b1) * g;
                    v[k] = b2 * v[k] + (1.0 - b2) * g * g;
                    let m_hat = m[k] / (1.0 - b1.powi(iteration as i32));
                    let v_hat = v[k] / (1.0 - b2.powi(iteration as i32));
                    spec.lr * m_hat / (v_hat.sqrt() + eps)
                }
            };
            values[k] = (values[k] - step).clamp(param.min, param.max);
            // on a bound after the step: the projection held it there, or
            // the step landed on it, and either way the descent is stopped
            // in that direction
            bounds[k].ends_on = if values[k] <= param.min {
                bounds[k].on_lower += 1;
                Some(Bound::Lower)
            } else if values[k] >= param.max {
                bounds[k].on_upper += 1;
                Some(Bound::Upper)
            } else {
                None
            };
        }
        on_step(&TrainStep {
            block: iteration,
            loss,
            grads,
            params: values.clone(),
        });
    }
    Ok(Trained {
        paths: params.iter().map(|p| p.path.clone()).collect(),
        values,
        ranges: params.iter().map(|p| (p.min, p.max)).collect(),
        bounds,
        first_loss,
        last_loss,
        min_loss,
        min_loss_block,
        values_at_min,
        timing: timer.finish(),
    })
}

/// The mean loss of the first block of the excitation at every point of the
/// cartesian product of `axes` (exact or suffix path, values), the last axis
/// varying fastest: each point from a cleared instance, the `--set` controls
/// written, the trained controls at their starting values and the swept ones
/// at the point's.
///
/// Every swept control must be a trained one: a grid is where a descent
/// starts from, and the descent moves the trained controls only.
///
/// # Errors
/// An axis that names no trained control, or a loss that is finite nowhere.
pub fn grid(
    factory: &Rc<Factory>,
    sample_rate: i32,
    spec: &TrainSpec,
    axes: &[(String, Vec<f64>)],
) -> Result<Grid, String> {
    if spec.block == 0 {
        return Err("--sweep with --train needs a block size".to_owned());
    }
    let probe = Probe::instantiate(factory, sample_rate)?;
    let params = resolve_params(&probe, &spec.params, &spec.sets)?;
    check_lanes(&probe, spec)?;
    // the trained control each axis drives
    let mut driven = Vec::with_capacity(axes.len());
    for (query, values) in axes {
        let path = resolve_path(&probe, query)?;
        let Some(index) = params.iter().position(|p| p.path == path) else {
            return Err(format!(
                "--sweep `{query}`: with --train, a swept control must be a trained one ({path} is not); \
                 --set gives any other control a fixed value"
            ));
        };
        if values.is_empty() {
            return Err(format!("--sweep `{query}` has no value"));
        }
        if driven.contains(&index) {
            return Err(format!("--sweep names `{path}` twice"));
        }
        driven.push(index);
    }
    let x = input_block(
        &spec.input,
        probe.inputs(),
        0,
        spec.block,
        f64::from(sample_rate),
    );
    let total: usize = axes.iter().map(|(_, values)| values.len()).product();
    let mut points = Vec::with_capacity(total);
    for flat in 0..total {
        // the last axis varies fastest, as in a render sweep
        let mut rest = flat;
        let mut at = vec![0.0; axes.len()];
        for (axis, (_, values)) in axes.iter().enumerate().rev() {
            at[axis] = values[rest % values.len()];
            rest /= values.len();
        }
        probe.reset();
        apply_sets(&probe, &spec.sets)?;
        for (k, param) in params.iter().enumerate() {
            let value = driven
                .iter()
                .position(|&index| index == k)
                .map_or(param.init, |axis| at[axis]);
            probe.set_exact(&param.path, value)?;
        }
        let sums = lane_sums(&probe.compute_raw(&x, spec.block));
        points.push((at, sums[spec.loss_lane] / spec.block as f64));
    }
    let best = points
        .iter()
        .enumerate()
        .filter(|(_, (_, loss))| loss.is_finite())
        .min_by(|(_, (_, a)), (_, (_, b))| a.total_cmp(b))
        .map(|(index, _)| index)
        .ok_or_else(|| "the loss is finite at no point of the grid".to_owned())?;
    Ok(Grid {
        paths: driven.iter().map(|&k| params[k].path.clone()).collect(),
        points,
        best,
    })
}

/// Checks every gradient lane of `spec` against central finite differences
/// of the loss lane, with steps `h` and `h / 2` on each control in turn,
/// extrapolated (see [`FdCheck::fd`]), on the first block of the excitation: at `at`, one value per trained control, or when
/// `None` at the controls' starting values (their widgets' initial values,
/// or what `spec.sets` gives them). Every evaluation runs on a fresh instance
/// so that the state is the same each time.
///
/// The differences step across a bound when the point is on one: the program
/// computes there as anywhere, the range being the host's business.
pub fn fd_check(
    factory: &Rc<Factory>,
    sample_rate: i32,
    spec: &TrainSpec,
    h: f64,
    at: Option<&[f64]>,
) -> Result<Vec<FdCheck>, String> {
    if spec.block == 0 {
        return Err("--fd-check needs a block size".to_owned());
    }
    let (params, inputs) = {
        let probe = Probe::instantiate(factory, sample_rate)?;
        check_lanes(&probe, spec)?;
        (
            resolve_params(&probe, &spec.params, &spec.sets)?,
            probe.inputs(),
        )
    };
    let x = input_block(&spec.input, inputs, 0, spec.block, f64::from(sample_rate));
    let evaluate = |values: &[f64]| -> Result<Vec<f64>, String> {
        let probe = Probe::instantiate(factory, sample_rate)?;
        apply_sets(&probe, &spec.sets)?;
        for (param, &value) in params.iter().zip(values) {
            probe.set_exact(&param.path, value)?;
        }
        Ok(lane_sums(&probe.compute_raw(&x, spec.block)))
    };
    let init: Vec<f64> = match at {
        Some(values) if values.len() == params.len() => values.to_vec(),
        Some(values) => {
            return Err(format!(
                "--fd-check: {} values for {} trained controls",
                values.len(),
                params.len()
            ));
        }
        None => params.iter().map(|p| p.init).collect(),
    };
    let base = evaluate(&init)?;
    params
        .iter()
        .enumerate()
        .map(|(k, param)| {
            let difference = |step: f64| -> Result<f64, String> {
                let mut plus = init.clone();
                plus[k] += step;
                let mut minus = init.clone();
                minus[k] -= step;
                Ok(
                    (evaluate(&plus)?[spec.loss_lane] - evaluate(&minus)?[spec.loss_lane])
                        / (2.0 * step),
                )
            };
            let fd_plain = difference(h)?;
            let fd = (4.0 * difference(h / 2.0)? - fd_plain) / 3.0;
            let rad = base[spec.first_grad_lane + k];
            Ok(FdCheck {
                path: param.path.clone(),
                rad,
                fd,
                fd_plain,
                relative_error: (rad - fd).abs() / fd.abs().max(1.0),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";

    fn workspace_dir(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(rel)
    }

    /// The factory of a corpus program, or `None` when the standard
    /// libraries are unavailable (the test skips, as the corpus tests do).
    fn corpus_factory(stem: &str) -> Option<Rc<Factory>> {
        let root = std::env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
            .map(PathBuf::from)
            .or_else(|| {
                let default = PathBuf::from(DEFAULT_FAUSTLIBRARIES_ROOT);
                default.exists().then_some(default)
            });
        let Some(root) = root else {
            eprintln!("Skipping {stem}: faustlibraries unavailable");
            return None;
        };
        let path = workspace_dir("tests/corpus").join(format!("{stem}.dsp"));
        let dirs = [workspace_dir("libraries"), root]
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        Some(Rc::new(
            Factory::compile_with_args(&path.to_string_lossy(), &dirs, &[], true, 0)
                .expect("compile"),
        ))
    }

    /// The corpus programs expand deeply at compile time: 64 MiB of stack,
    /// as every corpus test uses.
    fn on_big_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(f)
            .expect("spawn")
            .join()
            .expect("the test body should not panic");
    }

    fn spec(params: &[&str], block: usize, blocks: usize, lr: f64, seed: u64) -> TrainSpec {
        TrainSpec {
            params: params.iter().map(|p| (*p).to_owned()).collect(),
            loss_lane: 0,
            first_grad_lane: 1,
            optimizer: Optimizer::ADAM,
            lr,
            block,
            blocks,
            input: InputMode::White { seed },
            reset_per_block: false,
            sets: vec![],
        }
    }

    /// `--set` on a trained control is the descent's starting point: from
    /// `a1 = -0.4` instead of the slider's `-0.8`, the first block's loss is
    /// another number, the finite-difference check holds there, and 600
    /// blocks still recover `(-1.2, 0.72)`.
    #[test]
    fn set_gives_a_trained_control_its_starting_point() {
        on_big_stack(|| {
            let Some(factory) = corpus_factory("ddsp_rad_host_block_resonator") else {
                return;
            };
            let s0 = spec(&["a1", "a2"], 256, 600, 0.01, 1);
            let mut s = spec(&["a1", "a2"], 256, 600, 0.01, 1);
            s.sets = vec![("a1".to_owned(), -0.4)];
            for check in fd_check(&factory, 44_100, &s, 1e-3, None).expect("fd-check") {
                assert!(check.relative_error < 2e-2, "{check:?}");
            }
            let mut first_step = None;
            let trained = train(&factory, 44_100, &s, |step| {
                if step.block == 1 {
                    first_step = Some(step.params.clone());
                }
            })
            .expect("train");
            let from_default = train(&factory, 44_100, &s0, |_| {}).expect("train");
            assert!(
                (trained.first_loss - from_default.first_loss).abs()
                    > 1e-3 * from_default.first_loss,
                "the first block should be computed from the set start: {} against {}",
                trained.first_loss,
                from_default.first_loss
            );
            let first = first_step.expect("a first step");
            assert!(
                (first[0] + 0.4).abs() < 0.02,
                "the first step should leave from -0.4, got {first:?}"
            );
            assert!(
                (trained.values[0] + 1.2).abs() < 0.02 && (trained.values[1] - 0.72).abs() < 0.02,
                "training should recover (-1.2, 0.72), got {:?}",
                trained.values
            );
        });
    }

    /// The resonator of `ddsp_rad_host_block_resonator.dsp`: the block
    /// gradients match finite differences, and 600 blocks of Adam recover
    /// the hidden `(-1.2, 0.72)` from `(-0.8, 0.5)`.
    #[test]
    fn resonator_gradients_check_and_training_recovers_the_poles() {
        on_big_stack(|| {
            let Some(factory) = corpus_factory("ddsp_rad_host_block_resonator") else {
                return;
            };
            let s = spec(&["a1", "a2"], 256, 600, 0.01, 1);
            for check in fd_check(&factory, 44_100, &s, 1e-3, None).expect("fd-check") {
                assert!(check.relative_error < 2e-2, "{check:?}");
            }
            let trained = train(&factory, 44_100, &s, |_| {}).expect("train");
            assert_eq!(
                trained.paths,
                [
                    "/ddsp_rad_host_block_resonator/a1",
                    "/ddsp_rad_host_block_resonator/a2"
                ]
            );
            assert!(
                (trained.values[0] + 1.2).abs() < 0.02 && (trained.values[1] - 0.72).abs() < 0.02,
                "training should recover (-1.2, 0.72), got {:?}",
                trained.values
            );
            assert!(
                trained.last_loss < 1e-3 * trained.first_loss,
                "block loss should fall by 30 dB: first {} last {}",
                trained.first_loss,
                trained.last_loss
            );
        });
    }

    const GRU_PARAMS: [&str; 27] = [
        "wz1", "wz2", "wr1", "wr2", "wh1", "wh2", "uz11", "uz12", "uz21", "uz22", "ur11", "ur12",
        "ur21", "ur22", "uh11", "uh12", "uh21", "uh22", "bz1", "bz2", "br1", "br2", "bh1", "bh2",
        "wo1", "wo2", "bo",
    ];

    /// The GRU of `ddsp_rad_gru_amp_host.dsp`: the 27 block gradients through
    /// the recurrent cell match finite differences, and 2 000 blocks of
    /// truncated BPTT cut the block loss tenfold.
    #[test]
    fn gru_gradients_check_and_block_bptt_cuts_the_loss() {
        on_big_stack(|| {
            let Some(factory) = corpus_factory("ddsp_rad_gru_amp_host") else {
                return;
            };
            let s = spec(&GRU_PARAMS, 128, 2000, 0.005, 3);
            for check in fd_check(&factory, 44_100, &s, 1e-3, None).expect("fd-check") {
                assert!(check.relative_error < 2e-2, "{check:?}");
            }
            let s = TrainSpec {
                block: 256,
                input: InputMode::White { seed: 11 },
                ..s
            };
            let mut losses = Vec::new();
            let trained =
                train(&factory, 44_100, &s, |step| losses.push(step.loss)).expect("train");
            let first: f64 = losses[..100].iter().sum::<f64>() / 100.0;
            let last: f64 = losses[losses.len() - 100..].iter().sum::<f64>() / 100.0;
            assert!(
                last < 0.1 * first,
                "training should cut the block loss by 10 dB: first {first} last {last}"
            );
            assert!(
                trained
                    .values
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 4.0)
            );
        });
    }

    #[test]
    fn rejects_bad_lanes_and_unknown_controls() {
        on_big_stack(|| {
            let Some(factory) = corpus_factory("ddsp_rad_host_block_resonator") else {
                return;
            };
            let mut s = spec(&["a1", "a2"], 64, 1, 0.01, 1);
            s.first_grad_lane = 2; // lanes 2..3 do not fit in 3 outputs
            assert!(
                train(&factory, 44_100, &s, |_| {})
                    .unwrap_err()
                    .contains("do not fit")
            );
            let s = spec(&["a1", "nope"], 64, 1, 0.01, 1);
            assert!(
                train(&factory, 44_100, &s, |_| {})
                    .unwrap_err()
                    .contains("nope")
            );
        });
    }
}
