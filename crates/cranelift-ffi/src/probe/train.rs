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

use std::rc::Rc;

use crate::probe::engine::{Factory, Probe};
use crate::probe::params::Resolution;
use crate::probe::render::InputMode;

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
}

/// One step of the training, reported as it happens.
#[derive(Clone, Debug)]
pub struct TrainStep {
    /// 1-based block index.
    pub block: usize,
    /// The block's mean loss.
    pub loss: f64,
    /// The controls after the step.
    pub params: Vec<f64>,
}

/// The outcome of a training.
#[derive(Clone, Debug)]
pub struct Trained {
    pub paths: Vec<String>,
    pub values: Vec<f64>,
    pub first_loss: f64,
    pub last_loss: f64,
}

/// One control's gradient lane against finite differences, on one block.
#[derive(Clone, Debug)]
pub struct FdCheck {
    pub path: String,
    /// The gradient lane summed over the block.
    pub rad: f64,
    /// The central finite difference of the summed loss lane.
    pub fd: f64,
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

fn resolve_params(probe: &Probe, queries: &[String]) -> Result<Vec<Param>, String> {
    if queries.is_empty() {
        return Err("--train needs at least one control".to_owned());
    }
    queries
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
        .collect()
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
    let params = resolve_params(&probe, &spec.params)?;
    check_lanes(&probe, spec)?;
    let mut values: Vec<f64> = params.iter().map(|p| p.init).collect();
    let (mut m, mut v) = (vec![0.0_f64; params.len()], vec![0.0_f64; params.len()]);
    let (mut first_loss, mut last_loss) = (0.0, 0.0);
    let n = spec.block as f64;
    for iteration in 1..=spec.blocks {
        if spec.reset_per_block {
            // controls back to their defaults and the state cleared; the
            // trained controls are rewritten just below
            probe.reset();
        }
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
        let outs = probe.compute_raw(&x, spec.block);
        let sums = lane_sums(&outs);
        let loss = sums[spec.loss_lane] / n;
        if !loss.is_finite() {
            return Err(format!("the loss is not finite at block {iteration}"));
        }
        if iteration == 1 {
            first_loss = loss;
        }
        last_loss = loss;
        for (k, param) in params.iter().enumerate() {
            let g = sums[spec.first_grad_lane + k] / n;
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
        }
        on_step(&TrainStep {
            block: iteration,
            loss,
            params: values.clone(),
        });
    }
    Ok(Trained {
        paths: params.iter().map(|p| p.path.clone()).collect(),
        values,
        first_loss,
        last_loss,
    })
}

/// Checks every gradient lane of `spec` against central finite differences
/// of the loss lane, with step `h` on each control in turn, at the controls'
/// initial values, on the first block of the excitation. Every evaluation
/// runs on a fresh instance so that the state is the same each time.
pub fn fd_check(
    factory: &Rc<Factory>,
    sample_rate: i32,
    spec: &TrainSpec,
    h: f64,
) -> Result<Vec<FdCheck>, String> {
    if spec.block == 0 {
        return Err("--fd-check needs a block size".to_owned());
    }
    let (params, inputs) = {
        let probe = Probe::instantiate(factory, sample_rate)?;
        check_lanes(&probe, spec)?;
        (resolve_params(&probe, &spec.params)?, probe.inputs())
    };
    let x = input_block(&spec.input, inputs, 0, spec.block, f64::from(sample_rate));
    let evaluate = |values: &[f64]| -> Result<Vec<f64>, String> {
        let probe = Probe::instantiate(factory, sample_rate)?;
        for (param, &value) in params.iter().zip(values) {
            probe.set_exact(&param.path, value)?;
        }
        Ok(lane_sums(&probe.compute_raw(&x, spec.block)))
    };
    let init: Vec<f64> = params.iter().map(|p| p.init).collect();
    let base = evaluate(&init)?;
    params
        .iter()
        .enumerate()
        .map(|(k, param)| {
            let mut plus = init.clone();
            plus[k] += h;
            let mut minus = init.clone();
            minus[k] -= h;
            let fd =
                (evaluate(&plus)?[spec.loss_lane] - evaluate(&minus)?[spec.loss_lane]) / (2.0 * h);
            let rad = base[spec.first_grad_lane + k];
            Ok(FdCheck {
                path: param.path.clone(),
                rad,
                fd,
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
        }
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
            for check in fd_check(&factory, 44_100, &s, 1e-3).expect("fd-check") {
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
            for check in fd_check(&factory, 44_100, &s, 1e-3).expect("fd-check") {
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
