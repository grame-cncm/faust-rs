//! The two examples of "Faust Autodiff: Towards Audio Domain-Specific
//! Machine Learning" (T. Rushton, AES AIMLA 2025) as corpus fixtures: the
//! artificial neuron of its listing 4 and the differentiable IIR of its
//! listing 5, each under `fad` and under `rad`.
//!
//! The paper differentiates at the source level by pattern matching on the
//! box algebra, so its parameters enter as inputs through a wrapper, its dot
//! product is routed by hand (`route` cannot be pattern-matched) and its
//! filter cannot use `fi.iir` (`ma.sub` is an unapplied abstraction). The
//! fixtures keep the paper's routing and take the parameters as slider
//! seeds. The tests check:
//!
//! - the neuron's lanes against the closed form `y (1 - y) x_i` and
//!   `y (1 - y)`, and `rad` against `fad` lane by lane (feed-forward body);
//! - the IIR's `fad` tangents against central finite differences on every
//!   frame, and against `fad(fi.iir(bv, av), seeds)` with the library filter;
//! - the IIR's `rad` block totals (a `BlockReverseAD` body) against the
//!   finite differences of the block sum and against the `fad` totals.
//!
//! The last check found the coefficients of the paper's recursion, routed
//! through the `~` block as wires, getting a zero gradient: the reverse
//! sweep re-injected the carry of a feedback tap only into a recursion slot
//! the carrier reads outside the recursion (`collect_bra_postorder_closed`).

use std::io::Cursor;
use std::path::{Path, PathBuf};

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};
use transform::signal_fir::RealType;

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn compile_f64(path: &Path) -> String {
    Compiler::new()
        .with_real_type(RealType::Float64)
        .compile_file_to_interp_with_lane(
            path,
            &[],
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{} interp compilation failed: {e}", path.display()))
}

fn compile_corpus_f64(file: &str) -> String {
    let path = corpus_path(file);
    std::thread::Builder::new()
        .name(format!("aes-paper-{file}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || compile_f64(&path))
        .expect("spawn aes-paper worker")
        .join()
        .expect("aes-paper worker thread should finish")
}

fn compile_source_f64(stem: &str, source: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "faust-rs-aes-paper-{stem}-{}.dsp",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("write temp dsp");
    let worker_path = path.clone();
    let fbc = std::thread::Builder::new()
        .name(format!("aes-paper-{stem}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || compile_f64(&worker_path))
        .expect("spawn aes-paper worker")
        .join()
        .expect("aes-paper worker thread should finish");
    let _ = std::fs::remove_file(&path);
    fbc
}

/// Renders `frames` frames of a cleared instance with its sliders set by
/// label, and returns the output lanes.
fn render(
    fbc: &str,
    controls: &[(&str, f64)],
    inputs: &[Vec<f64>],
    frames: usize,
) -> Vec<Vec<f64>> {
    let mut reader = Cursor::new(fbc.as_bytes().to_vec());
    let mut factory = read_fbc::<f64>(&mut reader).expect("parse fbc");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    for &(label, value) in controls {
        let offset = instance
            .ui_instructions()
            .iter()
            .find(|i| i.label == label)
            .map(|i| i.offset)
            .unwrap_or_else(|| panic!("slider {label} not found"));
        instance.set_real_zone(offset, value);
    }
    let num_inputs = usize::try_from(instance.get_num_inputs()).expect("inputs");
    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("outputs");
    assert_eq!(inputs.len(), num_inputs, "input arity");
    let mut outputs = vec![vec![0.0_f64; frames]; num_outputs];
    let ins: Vec<&[f64]> = inputs.iter().map(Vec::as_slice).collect();
    let mut outs: Vec<&mut [f64]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frames as i32, &ins, &mut outs)
        .expect("compute");
    outputs
}

fn arity(fbc: &str) -> (i32, i32) {
    let mut reader = Cursor::new(fbc.as_bytes().to_vec());
    let mut factory = read_fbc::<f64>(&mut reader).expect("parse fbc");
    let instance = FbcDspInstance::new(&mut factory);
    (instance.get_num_inputs(), instance.get_num_outputs())
}

/// A deterministic signal in [-1, 1) (an LCG, as the corpus noise sources).
fn noise(seed: u32, n: usize) -> Vec<f64> {
    let mut state = seed;
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            f64::from(state as i32) / 2_147_483_648.0
        })
        .collect()
}

fn assert_close(actual: f64, expected: f64, tol: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= tol,
        "{what}: got {actual}, expected {expected}, diff {}, tolerance {tol}",
        (actual - expected).abs()
    );
}

const FRAMES: usize = 64;
const NEURON_INPUTS: usize = 3;
const NEURON_CONTROLS: [(&str, f64); 4] = [("w0", 0.5), ("w1", -0.25), ("w2", 0.75), ("b", 0.1)];
const IIR_CONTROLS: [(&str, f64); 5] = [
    ("a1", -0.5),
    ("a2", 0.25),
    ("b0", 0.3),
    ("b1", 0.2),
    ("b2", 0.1),
];

fn neuron_inputs() -> Vec<Vec<f64>> {
    (0..NEURON_INPUTS)
        .map(|i| noise(7 + i as u32, FRAMES))
        .collect()
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

#[test]
fn fixtures_have_the_paper_arity() {
    // NW input signals in, the primal and NW + 1 derivatives out; one input
    // signal in, the primal and 2N + 1 derivatives out.
    for file in ["fad_neuron_sigmoid.dsp", "rad_neuron_sigmoid.dsp"] {
        assert_eq!(arity(&compile_corpus_f64(file)), (3, 5), "{file}");
    }
    for file in ["fad_iir_transposed.dsp", "rad_iir_transposed.dsp"] {
        assert_eq!(arity(&compile_corpus_f64(file)), (1, 6), "{file}");
    }
}

#[test]
fn neuron_fad_lanes_match_the_closed_form() {
    let fbc = compile_corpus_f64("fad_neuron_sigmoid.dsp");
    let inputs = neuron_inputs();
    let lanes = render(&fbc, &NEURON_CONTROLS, &inputs, FRAMES);
    for n in 0..FRAMES {
        let pre = (0..NEURON_INPUTS)
            .map(|i| NEURON_CONTROLS[i].1 * inputs[i][n])
            .sum::<f64>()
            + NEURON_CONTROLS[3].1;
        let y = sigmoid(pre);
        assert_close(lanes[0][n], y, 1e-12, &format!("y frame {n}"));
        for i in 0..NEURON_INPUTS {
            assert_close(
                lanes[1 + i][n],
                y * (1.0 - y) * inputs[i][n],
                1e-12,
                &format!("dy/dw{i} frame {n}"),
            );
        }
        assert_close(
            lanes[4][n],
            y * (1.0 - y),
            1e-12,
            &format!("dy/db frame {n}"),
        );
    }
}

#[test]
fn neuron_rad_lanes_equal_the_fad_lanes() {
    let fad = render(
        &compile_corpus_f64("fad_neuron_sigmoid.dsp"),
        &NEURON_CONTROLS,
        &neuron_inputs(),
        FRAMES,
    );
    let rad = render(
        &compile_corpus_f64("rad_neuron_sigmoid.dsp"),
        &NEURON_CONTROLS,
        &neuron_inputs(),
        FRAMES,
    );
    // A feed-forward body: the symbolic reverse sweep, exact sample by sample.
    for (lane, (f, r)) in fad.iter().zip(&rad).enumerate() {
        for n in 0..FRAMES {
            assert_close(r[n], f[n], 1e-14, &format!("lane {lane} frame {n}"));
        }
    }
}

/// Central finite difference of every lane 0 frame with respect to the
/// control `index`, from two renders of the primal lane.
fn iir_finite_differences(fbc: &str, x: &[Vec<f64>], index: usize, eps: f64) -> Vec<f64> {
    let mut up = IIR_CONTROLS;
    up[index].1 += eps;
    let mut down = IIR_CONTROLS;
    down[index].1 -= eps;
    let y_up = render(fbc, &up, x, FRAMES);
    let y_down = render(fbc, &down, x, FRAMES);
    (0..FRAMES)
        .map(|n| (y_up[0][n] - y_down[0][n]) / (2.0 * eps))
        .collect()
}

#[test]
fn iir_fad_tangents_match_central_differences() {
    let fbc = compile_corpus_f64("fad_iir_transposed.dsp");
    let x = vec![noise(3, FRAMES)];
    let lanes = render(&fbc, &IIR_CONTROLS, &x, FRAMES);
    for (j, (label, _)) in IIR_CONTROLS.iter().enumerate() {
        let fd = iir_finite_differences(&fbc, &x, j, 1e-4);
        for n in 0..FRAMES {
            assert_close(
                lanes[1 + j][n],
                fd[n],
                1e-6,
                &format!("dy/d{label} frame {n}"),
            );
        }
    }
}

#[test]
fn iir_fad_equals_the_library_filter() {
    // The paper cannot differentiate fi.iir; fad on it gives the fixture's
    // lanes, the same sliders and the same seed order.
    let library = r#"
fi = library("filters.lib");
a(i) = hslider("a%i", 0, -2, 2, 0.001);
b(i) = hslider("b%i", 0, -2, 2, 0.001);
coeffs = a(1), a(2), b(0), b(1), b(2);
process = fad(fi.iir((b(0), b(1), b(2)), (a(1), a(2))), coeffs);
"#;
    let x = vec![noise(3, FRAMES)];
    let paper = render(
        &compile_corpus_f64("fad_iir_transposed.dsp"),
        &IIR_CONTROLS,
        &x,
        FRAMES,
    );
    let lib = render(
        &compile_source_f64("fi-iir", library),
        &IIR_CONTROLS,
        &x,
        FRAMES,
    );
    assert_eq!(lib.len(), paper.len());
    for (lane, (p, l)) in paper.iter().zip(&lib).enumerate() {
        for n in 0..FRAMES {
            assert_close(l[n], p[n], 1e-12, &format!("lane {lane} frame {n}"));
        }
    }
}

#[test]
fn iir_rad_block_totals_match_central_differences_and_the_fad_totals() {
    let x = vec![noise(3, FRAMES)];
    let fad_fbc = compile_corpus_f64("fad_iir_transposed.dsp");
    let fad = render(&fad_fbc, &IIR_CONTROLS, &x, FRAMES);
    let rad = render(
        &compile_corpus_f64("rad_iir_transposed.dsp"),
        &IIR_CONTROLS,
        &x,
        FRAMES,
    );
    for n in 0..FRAMES {
        assert_close(rad[0][n], fad[0][n], 1e-14, &format!("y frame {n}"));
    }
    // The body is recursive: the lanes are per-sample contributions to the
    // gradient of the block sum of y, exact over one block from a cleared
    // instance.
    for (j, (label, _)) in IIR_CONTROLS.iter().enumerate() {
        let rad_total: f64 = rad[1 + j].iter().sum();
        let fad_total: f64 = fad[1 + j].iter().sum();
        let fd_total: f64 = iir_finite_differences(&fad_fbc, &x, j, 1e-4).iter().sum();
        assert!(
            rad_total.abs() > 1e-3,
            "d(sum y)/d{label}: the block total should not vanish, got {rad_total}"
        );
        assert_close(
            rad_total,
            fd_total,
            1e-6,
            &format!("d(sum y)/d{label} against fd"),
        );
        assert_close(
            rad_total,
            fad_total,
            1e-9,
            &format!("d(sum y)/d{label} against fad"),
        );
    }
}
