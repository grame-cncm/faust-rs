//! The programs of `libraries/optimizers-ddsp-tutorial-en.md`, run as the
//! tutorial says to run them and checked against the figures it gives.
//!
//! The tutorial quotes, after each program, what `faustprobe` prints for a
//! given `-n`, `--every`, `--skip` or `--in`; those figures are the
//! contract this file keeps true. Every ```` ```faust ```` block is
//! extracted from the Markdown at test time (no copy of the programs
//! anywhere else), compiled through the same Cranelift JIT as faustprobe in
//! double precision, and rendered with the engine faustprobe uses, at its
//! default sample rate and block size. One test per section; a first test
//! checks that the French tutorial carries the same programs, comments
//! aside, and a second that every complete program compiles and stays
//! finite, so that a block without figures is at least still a program.
//!
//! The programs import the Faust standard libraries, found through
//! `FAUST_RS_FAUSTLIBRARIES_ROOT` or the default checkout path; the tests
//! skip when neither exists, as the corpus tests do. They render on a 64 MiB
//! thread, as every corpus test does (the `fad` and `rad` expansions build
//! deep trees at compile time).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::render::InputMode;
use cranelift_ffi::probe::train::{self, Optimizer, TrainSpec};

/// faustprobe's defaults: `--sr 44100`, `--block 64`.
const SAMPLE_RATE: i32 = 44_100;
const BLOCK: usize = 64;
const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";

fn workspace_dir(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(rel)
}

fn faustlibraries_root() -> Option<PathBuf> {
    std::env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            let default = PathBuf::from(DEFAULT_FAUSTLIBRARIES_ROOT);
            default.exists().then_some(default)
        })
}

// ───────────────────────── the Markdown ─────────────────────────

/// One ```` ```faust ```` block of a tutorial.
#[derive(Clone, Debug)]
struct Block {
    /// 1-based line of the opening fence, for messages.
    line: usize,
    /// The nearest heading above the block.
    section: String,
    code: String,
}

impl Block {
    /// A complete program (a fragment such as `op = library(...)` alone or
    /// the `ondemand` signature has no `process`).
    fn is_program(&self) -> bool {
        self.code.lines().any(|l| l.starts_with("process"))
    }
}

fn read_blocks(name: &str) -> Vec<Block> {
    let path = workspace_dir("libraries").join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut blocks = Vec::new();
    let mut section = String::new();
    let mut open: Option<(usize, String)> = None;
    for (index, line) in text.lines().enumerate() {
        match &mut open {
            Some((start, code)) => {
                if line.starts_with("```") {
                    blocks.push(Block {
                        line: *start,
                        section: section.clone(),
                        code: std::mem::take(code),
                    });
                    open = None;
                } else {
                    code.push_str(line);
                    code.push('\n');
                }
            }
            None => {
                if line.starts_with('#') {
                    section = line.trim_start_matches('#').trim().to_owned();
                } else if line.trim() == "```faust" {
                    open = Some((index + 1, String::new()));
                }
            }
        }
    }
    assert!(open.is_none(), "{name}: unterminated code block");
    blocks
}

/// The English tutorial's programs.
fn programs() -> Vec<Block> {
    read_blocks("optimizers-ddsp-tutorial-en.md")
        .into_iter()
        .filter(Block::is_program)
        .collect()
}

/// The `nth` program (0-based) of the section whose heading starts with
/// `heading`, e.g. `("### 11.2", 1)`.
fn program(heading: &str, nth: usize) -> Block {
    programs()
        .into_iter()
        .filter(|b| b.section.starts_with(heading))
        .nth(nth)
        .unwrap_or_else(|| panic!("no program {nth} under a heading starting with `{heading}`"))
}

/// `code` without its `//` comments and blank lines: what the French
/// tutorial must share with the English one.
fn without_comments(code: &str) -> String {
    code.lines()
        .map(|l| l.split("//").next().unwrap_or("").trim_end())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

// ───────────────────────── the engine ─────────────────────────

/// Runs `f` on a 64 MiB thread, or skips (returns) when the standard
/// libraries are not available.
fn with_libraries(name: &str, f: impl FnOnce(PathBuf) + Send + 'static) {
    let Some(root) = faustlibraries_root() else {
        eprintln!("Skipping {name}: faustlibraries unavailable");
        return;
    };
    std::thread::Builder::new()
        .name(format!("tutorial-{name}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || f(root))
        .expect("spawn")
        .join()
        .expect("the test body should not panic");
}

/// The block written to a file (the string front end does not take
/// `import`) and JIT-compiled in double precision, as
/// `faustprobe --double -I libraries -I <faustlibraries>` does.
fn compile(block: &Block, root: &Path) -> Rc<Factory> {
    let dir = std::env::temp_dir().join(format!("faust-rs-tutorial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(format!("tutorial_{}.dsp", block.line));
    std::fs::write(&path, &block.code).expect("write program");
    let dirs = [workspace_dir("libraries"), root.to_path_buf()]
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    Rc::new(
        Factory::compile(&path.to_string_lossy(), &dirs, true, 0).unwrap_or_else(|e| {
            panic!(
                "line {} ({}): compilation failed: {e}",
                block.line, block.section
            )
        }),
    )
}

/// `frames` frames of the program, one vector per output, rendered with
/// faustprobe's engine and block size.
fn render(block: &Block, root: &Path, input: InputMode, frames: usize) -> Vec<Vec<f64>> {
    let factory = compile(block, root);
    let probe = Probe::instantiate(&factory, SAMPLE_RATE).expect("instantiate");
    let mut outs = vec![Vec::with_capacity(frames); probe.outputs()];
    let spec = RenderSpec {
        frames,
        block: BLOCK,
        input,
        skip: 0,
        ..RenderSpec::default()
    };
    let stats = probe.render(&spec, |_, samples| {
        for (out, &v) in outs.iter_mut().zip(samples) {
            out.push(v);
        }
    });
    assert!(
        stats.all_finite(),
        "line {} ({}): a non-finite output",
        block.line,
        block.section
    );
    outs
}

fn rms(samples: &[f64]) -> f64 {
    (samples.iter().map(|v| v * v).sum::<f64>() / samples.len() as f64).sqrt()
}

fn mean(samples: &[f64]) -> f64 {
    samples.iter().sum::<f64>() / samples.len() as f64
}

fn peak(samples: &[f64]) -> f64 {
    samples.iter().fold(0.0_f64, |m, v| m.max(v.abs()))
}

#[track_caller]
fn assert_near(what: &str, got: f64, want: f64, tol: f64) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: got {got}, wanted {want} ± {tol}"
    );
}

// ───────────────────────── the two tutorials ─────────────────────────

#[test]
fn french_tutorial_carries_the_same_programs() {
    let en = read_blocks("optimizers-ddsp-tutorial-en.md");
    let fr = read_blocks("optimizers-ddsp-tutorial-fr.md");
    assert_eq!(
        en.len(),
        fr.len(),
        "the tutorials have {} and {} faust blocks",
        en.len(),
        fr.len()
    );
    // the fragments are prose (the `ondemand` signature is translated); the
    // programs must be the same code
    for (e, f) in en.iter().zip(&fr).filter(|(e, _)| e.is_program()) {
        assert_eq!(
            without_comments(&e.code),
            without_comments(&f.code),
            "the block at line {} of the English tutorial ({}) differs from the one at line {} of the French",
            e.line,
            e.section,
            f.line
        );
    }
    assert_eq!(
        en.iter().filter(|b| b.is_program()).count(),
        23,
        "the tutorial's program count changed: update the tests"
    );
}

#[test]
fn every_program_compiles_and_stays_finite() {
    with_libraries("all", |root| {
        for block in programs() {
            let factory = compile(&block, &root);
            let probe = Probe::instantiate(&factory, SAMPLE_RATE).expect("instantiate");
            let input = if probe.inputs() > 0 {
                InputMode::Sine { hz: 220.0 }
            } else {
                InputMode::Zero
            };
            drop(probe);
            render(&block, &root, input, 2000);
        }
    });
}

// ───────────────────────── section by section ─────────────────────────

/// §1: both gains climb to 0.699 in about 1 000 samples, and the difference
/// between the hand-written and the `fad` derivative is exactly 0.
#[test]
fn s01_hand_written_and_fad_loops_agree_exactly() {
    with_libraries("s01", |root| {
        let outs = render(&program("1.", 0), &root, InputMode::Zero, 1200);
        assert_near("g_manual at 1000", outs[0][1000], 0.699, 0.002);
        assert_near("g_fad at 1000", outs[1][1000], 0.699, 0.002);
        assert!(
            outs[2].iter().all(|&d| d == 0.0),
            "the difference is not exactly 0"
        );
    });
}

/// §2: `-n 1` prints `6, 3, 2, 6, 3, 2`.
#[test]
fn s02_fad_and_rad_layouts() {
    with_libraries("s02", |root| {
        let outs = render(&program("2.", 0), &root, InputMode::Zero, 1);
        let first: Vec<f64> = outs.iter().map(|o| o[0]).collect();
        assert_eq!(first, [6.0, 3.0, 2.0, 6.0, 3.0, 2.0]);
    });
}

/// §3: `g` reads 0.546, 0.685, 0.6998, 0.699999, 0.700000 at 500 … 2 500.
#[test]
fn s03_descend_1d_reaches_the_gain() {
    with_libraries("s03", |root| {
        let outs = render(&program("3.", 0), &root, InputMode::Zero, 3000);
        assert_near("g at 500", outs[0][500], 0.546, 0.001);
        assert_near("g at 1000", outs[0][1000], 0.685, 0.001);
        assert_near("g at 1500", outs[0][1500], 0.6998, 2e-4);
        assert_near("g at 2000", outs[0][2000], 0.699_999, 2e-6);
        assert_near("g at 2500", outs[0][2500], 0.7, 1e-8);
        assert!(
            outs[1][2500].abs() < 1e-8,
            "the residual has not gone to zero"
        );
    });
}

/// §4: `p` is 0.600013 at 500 and 0.600000 from 2 000 on.
#[test]
fn s04_one_pole_least_squares() {
    with_libraries("s04", |root| {
        let outs = render(&program("4.", 0), &root, InputMode::Zero, 4000);
        assert_near("p at 500", outs[0][500], 0.600_013, 2e-6);
        for frame in [2000, 2500, 3000, 3500] {
            assert_near(&format!("p at {frame}"), outs[0][frame], 0.6, 1e-8);
        }
    });
}

/// §4.1, with the figures §11.4 gives for it: the residual of the
/// per-sample bus loop reads rms 0.10, 2e-7, then 0 over windows of 1 000.
#[test]
fn s04_1_sixteen_tap_bus_loop() {
    with_libraries("s04_1", |root| {
        let outs = render(&program("4.1", 0), &root, InputMode::Zero, 4000);
        assert_near(
            "residual rms over 0..1000",
            rms(&outs[0][..1000]),
            0.10,
            0.01,
        );
        assert!(
            rms(&outs[0][1000..2000]) < 1e-6,
            "second window not at 2e-7"
        );
        assert!(rms(&outs[0][2000..3000]) < 1e-12, "third window not at 0");
    });
}

/// §5.1: `q` reaches 2.0 while `f` moves 1 Hz per 1 000 samples and is
/// still at 1 090 Hz after 100 000.
#[test]
fn s05_1_one_rate_for_two_units() {
    with_libraries("s05_1", |root| {
        let outs = render(&program("5.1", 0), &root, InputMode::Zero, 100_000);
        assert_near("f at 90000", outs[0][90_000], 1090.0, 0.1);
        assert_near("q at 90000", outs[1][90_000], 2.0, 0.05);
    });
}

/// §5.2: `(1206, 2.003)` at 10 000 samples, then near `(1200, 2.0)` with
/// the jitter Lion's fixed step leaves.
#[test]
fn s05_2_log_frequency_with_lion() {
    with_libraries("s05_2", |root| {
        let outs = render(&program("5.2", 0), &root, InputMode::Zero, 30_000);
        assert_near("f at 10000", outs[0][10_000], 1206.0, 1.0);
        assert_near("q at 10000", outs[1][10_000], 2.003, 0.005);
        for frame in (11_000..30_000).step_by(1000) {
            assert_near(
                &format!("f at {frame}"),
                outs[0][frame],
                1200.0,
                0.05 * 1200.0,
            );
            assert_near(&format!("q at {frame}"), outs[1][frame], 2.0, 0.05 * 2.0);
        }
    });
}

/// §5.3: `lm_2D` reads `(1200.000000, 2.000000)` at 5 000 and stays.
#[test]
fn s05_3_gauss_newton_finds_the_scales() {
    with_libraries("s05_3", |root| {
        let outs = render(&program("5.3", 0), &root, InputMode::Zero, 30_000);
        for frame in (5000..30_000).step_by(5000) {
            assert_near(&format!("f at {frame}"), outs[0][frame], 1200.0, 1e-6);
            assert_near(&format!("q at {frame}"), outs[1][frame], 2.0, 1e-6);
        }
    });
}

/// §6: the raw `a1` is pinned at its bound 1.92; the reflection form reaches
/// `(-0.999, 0.3999)` after 60 000 samples.
#[test]
fn s06_reflection_coefficients_stay_stable() {
    with_libraries("s06", |root| {
        let outs = render(&program("6.", 0), &root, InputMode::Zero, 200_000);
        for frame in (20_000..200_000).step_by(20_000) {
            assert_near(&format!("raw a1 at {frame}"), outs[0][frame], 1.92, 1e-9);
        }
        assert_near(
            "a1 from reflection at 60000",
            outs[1][60_000],
            -0.999,
            0.001,
        );
        assert_near(
            "a2 from reflection at 60000",
            outs[2][60_000],
            0.3999,
            0.0005,
        );
    });
}

/// §7.1: the `mse` gain wanders between 0.49 and 0.88 under the spikes, the
/// `logcosh` gain stays within 0.69–0.71.
#[test]
fn s07_1_robust_loss_ignores_the_spikes() {
    with_libraries("s07_1", |root| {
        let outs = render(&program("7.1", 0), &root, InputMode::Zero, 40_000);
        for frame in (5000..40_000).step_by(5000) {
            let (mse, robust) = (outs[0][frame], outs[1][frame]);
            assert!((0.45..=0.92).contains(&mse), "mse gain at {frame}: {mse}");
            assert!(
                (0.685..=0.715).contains(&robust),
                "logcosh gain at {frame}: {robust}"
            );
        }
        let range = (5000..40_000).step_by(5000).map(|f| outs[0][f]);
        let (lo, hi) = range.fold((1.0_f64, 0.0_f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
        assert!(hi - lo > 0.2, "the mse gain should wander: {lo}..{hi}");
    });
}

/// §7.2: the cutoff comes down from 3 000 Hz and settles between 770 and
/// 850 Hz, the Polyak readout with it.
#[test]
fn s07_2_spectral_envelope_loss_finds_the_cutoff() {
    with_libraries("s07_2", |root| {
        let outs = render(&program("7.2", 0), &root, InputMode::Zero, 400_000);
        assert_near("cutoff at 0", outs[0][0], 3000.0, 10.0);
        for frame in (50_000..400_000).step_by(50_000) {
            let f = outs[0][frame];
            assert!((740.0..=880.0).contains(&f), "cutoff at {frame}: {f}");
        }
        for frame in (100_000..400_000).step_by(50_000) {
            let f = outs[1][frame];
            assert!(
                (740.0..=880.0).contains(&f),
                "polyak readout at {frame}: {f}"
            );
        }
    });
}

/// §8: `g` at 0.69994 after 10 000 samples and within ±6e-5 of 0.7
/// afterwards; the learning rate goes from 0.02 to 0.0016 (last printed row).
#[test]
fn s08_schedule_gate_and_readout() {
    with_libraries("s08", |root| {
        let outs = render(&program("8.", 0), &root, InputMode::Zero, 80_000);
        assert_near("g at 10000", outs[0][10_000], 0.699_94, 2e-5);
        for frame in (20_000..80_000).step_by(10_000) {
            assert_near(&format!("g at {frame}"), outs[0][frame], 0.7, 6e-5);
        }
        assert_near("lr at 0", outs[2][0], 0.02, 1e-12);
        assert_near("lr at 70000", outs[2][70_000], 0.0016, 1e-4);
    });
}

/// §9: Newton on the implicit saturator, a unit sine at 220 Hz: the solved
/// signal peaks at 0.33 and the residual is 0 to numerical precision.
#[test]
fn s09_newton_solves_the_implicit_saturator() {
    with_libraries("s09", |root| {
        let outs = render(
            &program("9.", 0),
            &root,
            InputMode::Sine { hz: 220.0 },
            2000,
        );
        assert_near(
            "peak of the solution over 1000..2000",
            peak(&outs[0][1000..]),
            0.33,
            0.005,
        );
        assert!(
            peak(&outs[1][1000..]) < 1e-9,
            "the residual is not zero: {}",
            peak(&outs[1][1000..])
        );
    });
}

/// §10.1: the taps read `0.4995, 0.2997, -0.1999` at 500, `0.5, 0.3, -0.2`
/// to 1e-6 at 1 000, exactly afterwards, and the residual is zero.
#[test]
fn s10_1_three_taps_one_sweep() {
    with_libraries("s10_1", |root| {
        let outs = render(&program("10.1", 0), &root, InputMode::Zero, 3000);
        let want = [0.5, 0.3, -0.2];
        let at_500 = [0.4995, 0.2997, -0.1999];
        for k in 0..3 {
            assert_near(&format!("tap {k} at 500"), outs[k][500], at_500[k], 5e-4);
            assert_near(&format!("tap {k} at 1000"), outs[k][1000], want[k], 2e-6);
            assert_near(&format!("tap {k} at 2500"), outs[k][2500], want[k], 1e-9);
        }
        assert!(
            outs[3][2500].abs() < 1e-9,
            "residual at 2500: {}",
            outs[3][2500]
        );
    });
}

/// §10.2: the echo canceller's residual, in windows of 1 000 samples: rms
/// 2.6 with a peak of 25, then 1.4e-4, 2e-8, 0.
#[test]
fn s10_2_echo_canceller_converges_in_three_windows() {
    with_libraries("s10_2", |root| {
        let outs = render(&program("10.2", 0), &root, InputMode::Zero, 4000);
        assert_near("residual rms over 0..1000", rms(&outs[0][..1000]), 2.6, 0.2);
        assert_near(
            "residual peak over 0..1000",
            peak(&outs[0][..1000]),
            25.0,
            1.0,
        );
        assert!(rms(&outs[0][1000..2000]) < 3e-4, "second window");
        assert!(rms(&outs[0][2000..3000]) < 1e-7, "third window");
        // "0" at faustprobe's nine printed decimals
        assert!(rms(&outs[0][3000..4000]) < 1e-9, "fourth window");
    });
}

/// §10.3: the network's residual falls from rms 0.105 over the first 2 000
/// samples to 0.0037 over the last 4 000 of 20 000.
#[test]
fn s10_3_small_network_in_the_loop() {
    with_libraries("s10_3", |root| {
        let outs = render(&program("10.3", 0), &root, InputMode::Zero, 20_000);
        assert_near(
            "residual rms over 0..2000",
            rms(&outs[0][..2000]),
            0.105,
            0.005,
        );
        assert_near(
            "residual rms over 16000..20000",
            rms(&outs[0][16_000..]),
            0.0037,
            0.0005,
        );
    });
}

/// §10.4, first program: `[gain * x + bias, x, 1]` on a sine at 220 Hz.
#[test]
fn s10_4_gradients_handed_to_a_host() {
    with_libraries("s10_4a", |root| {
        let outs = render(&program("10.4", 0), &root, InputMode::Sine { hz: 220.0 }, 5);
        for (frame, &out) in outs[0].iter().enumerate() {
            let x = (std::f64::consts::TAU * 220.0 * frame as f64 / f64::from(SAMPLE_RATE)).sin();
            assert_near(&format!("output at {frame}"), out, x, 1e-9);
            assert_near(&format!("d/dgain at {frame}"), outs[1][frame], x, 1e-9);
            assert_near(&format!("d/dbias at {frame}"), outs[2][frame], 1.0, 1e-12);
        }
    });
}

/// §10.4, second program: the loss and its two gradient lanes driven by
/// faustprobe's `--train` loop, the gradients checked against finite
/// differences; Adam lands within 0.01 of `(0.5, -0.25)` in 100 blocks of
/// 256, SGD exactly.
#[test]
fn s10_4_host_loop_with_train() {
    with_libraries("s10_4b", |root| {
        let block = program("10.4", 1);
        let factory = compile(&block, &root);
        let spec = |optimizer, lr| TrainSpec {
            params: vec!["gain".to_owned(), "bias".to_owned()],
            loss_lane: 0,
            first_grad_lane: 1,
            optimizer,
            lr,
            block: 256,
            blocks: 100,
            input: InputMode::Zero,
            reset_per_block: false,
        };
        let adam = spec(Optimizer::ADAM, 0.05);
        let checks = train::fd_check(&factory, SAMPLE_RATE, &adam, 1e-3).expect("fd-check");
        assert_eq!(checks.len(), 2);
        for c in &checks {
            assert!(
                c.relative_error < 1e-6,
                "{}: rad {} fd {}",
                c.path,
                c.rad,
                c.fd
            );
        }
        let trained = train::train(&factory, SAMPLE_RATE, &adam, |_| {}).expect("train");
        assert_near("first block loss", trained.first_loss, 0.15, 0.01);
        assert!(trained.last_loss < 1e-5, "last loss {}", trained.last_loss);
        assert_near("gain (adam)", trained.values[0], 0.5, 0.01);
        assert_near("bias (adam)", trained.values[1], -0.25, 0.01);
        let sgd =
            train::train(&factory, SAMPLE_RATE, &spec(Optimizer::Sgd, 0.5), |_| {}).expect("train");
        assert_near("gain (sgd)", sgd.values[0], 0.5, 1e-9);
        assert_near("bias (sgd)", sgd.values[1], -0.25, 1e-9);
    });
}

/// §11.1: the whole optimizer clocked every 64 samples: `g` at 0.630 (4 000),
/// 0.7097 (6 000), 0.70005 (12 000), 0.7000 ± 1e-6 at the end.
#[test]
fn s11_1_clocked_optimizer() {
    with_libraries("s11_1", |root| {
        let outs = render(&program("11.1", 0), &root, InputMode::Zero, 20_000);
        assert_near("g at 4000", outs[0][4000], 0.630, 0.002);
        assert_near("g at 6000", outs[0][6000], 0.7097, 0.001);
        assert_near("g at 12000", outs[0][12_000], 0.700_05, 5e-5);
        assert_near("g at the end", outs[0][19_999], 0.7, 2e-6);
    });
}

/// §11.2, both programs (by hand, then `descend_1D_clocked`): `0.700000`
/// from 4 000 samples on.
#[test]
fn s11_2_gradient_at_audio_rate_step_per_frame() {
    with_libraries("s11_2", |root| {
        for nth in 0..2 {
            let outs = render(&program("11.2", nth), &root, InputMode::Zero, 20_000);
            for frame in (4000..20_000).step_by(2000) {
                assert_near(
                    &format!("program {nth}, g at {frame}"),
                    outs[0][frame],
                    0.7,
                    1e-8,
                );
            }
        }
    });
}

/// §11.3: the spectral loss, one step per frame: `g` averages 0.3405 over
/// the second half of 40 000 samples (the hand-computed optimum is 0.340).
#[test]
fn s11_3_spectral_loss_one_step_per_frame() {
    with_libraries("s11_3", |root| {
        let outs = render(&program("11.3", 0), &root, InputMode::Zero, 40_000);
        assert_near(
            "mean of g over the second half",
            mean(&outs[0][20_000..]),
            0.3405,
            0.003,
        );
    });
}

/// §11.4: the clocked bus loop's residual reads rms 0.21, 6e-4, 1.6e-6,
/// 3e-9 over the first four windows of 1 000.
#[test]
fn s11_4_reverse_mode_clocked() {
    with_libraries("s11_4", |root| {
        let outs = render(&program("11.4", 0), &root, InputMode::Zero, 4000);
        assert_near(
            "residual rms over 0..1000",
            rms(&outs[0][..1000]),
            0.21,
            0.02,
        );
        assert_near(
            "residual rms over 1000..2000",
            rms(&outs[0][1000..2000]),
            6e-4,
            1e-4,
        );
        assert_near(
            "residual rms over 2000..3000",
            rms(&outs[0][2000..3000]),
            1.6e-6,
            4e-7,
        );
        assert!(rms(&outs[0][3000..4000]) < 1e-8, "fourth window");
    });
}
