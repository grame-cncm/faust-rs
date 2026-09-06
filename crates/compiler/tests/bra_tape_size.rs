//! The `-bra-tape N` option: the block reverse sweep of `rad` records its
//! forward values in tapes of `N` samples, so the gradients of a block longer
//! than the tape wrap and are wrong, and a host that differentiates over a
//! long block (a whole impulse response in one `compute` call) sizes the tape
//! to it. The size must be a power of two: the tape index is masked.

use std::io::Cursor;
use std::path::PathBuf;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};
use transform::signal_fir::RealType;

/// A one-pole whose gain is the seed: the loss over a block depends on the
/// whole past through the recursion, so the reverse sweep needs every forward
/// value of the block.
const SOURCE: &str = r#"
g = hslider("g", 0.7, -2.0, 2.0, 0.001);
process = rad((_ * g : + ~ *(0.5)) <: *, g);
"#;

fn write_source(stem: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "faust-rs-bra-tape-{stem}-{}.dsp",
        std::process::id()
    ));
    std::fs::write(&path, SOURCE).expect("write temp dsp");
    path
}

fn compile(stem: &str, tape: usize) -> Result<String, String> {
    let path = write_source(stem);
    let result = Compiler::new()
        .with_real_type(RealType::Float64)
        .with_bra_tape(tape)
        .compile_file_to_interp_with_lane(
            &path,
            &[],
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&path);
    result
}

/// `(sum of the loss lane, sum of the gradient lane)` over one block of
/// `frames` samples from a cleared instance.
fn block(fbc: &str, gain: f64, x: &[f64]) -> (f64, f64) {
    let mut reader = Cursor::new(fbc.as_bytes().to_vec());
    let mut factory = read_fbc::<f64>(&mut reader).expect("parse fbc");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let offset = instance
        .ui_instructions()
        .iter()
        .find(|i| i.label == "g")
        .map(|i| i.offset)
        .expect("slider g");
    instance.set_real_zone(offset, gain);
    let mut lanes = vec![vec![0.0_f64; x.len()]; 2];
    let mut outs: Vec<&mut [f64]> = lanes.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(x.len() as i32, &[x], &mut outs)
        .expect("compute");
    (lanes[0].iter().sum(), lanes[1].iter().sum())
}

fn noise(n: usize) -> Vec<f64> {
    let mut state: u32 = 12345;
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            f64::from(state >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0
        })
        .collect()
}

fn relative_gap(fbc: &str, frames: usize) -> f64 {
    let x = noise(frames);
    let (_, grad) = block(fbc, 0.7, &x);
    let h = 1e-5;
    let fd = (block(fbc, 0.7 + h, &x).0 - block(fbc, 0.7 - h, &x).0) / (2.0 * h);
    ((grad - fd) / fd).abs()
}

#[test]
fn the_default_tape_is_exact_up_to_8192_frames_and_wraps_beyond() {
    let fbc = compile("default", 8192).expect("compile");
    let within = relative_gap(&fbc, 8192);
    let beyond = relative_gap(&fbc, 12_000);
    eprintln!("default tape: relative gap {within:.2e} at 8192 frames, {beyond:.2e} at 12000");
    assert!(
        within < 1e-6,
        "gradient within the tape must match finite differences: {within}"
    );
    assert!(
        beyond > 1e-3,
        "a block longer than the tape must wrap: {beyond}"
    );
}

#[test]
fn a_larger_tape_makes_the_long_block_exact() {
    let fbc = compile("large", 16_384).expect("compile");
    let gap = relative_gap(&fbc, 12_000);
    eprintln!("16384 tape: relative gap {gap:.2e} at 12000 frames");
    assert!(
        gap < 1e-6,
        "gradient over 12000 frames with a 16384 tape: {gap}"
    );
}

#[test]
fn a_tape_that_is_not_a_power_of_two_is_rejected() {
    let err = compile("odd", 1000).expect_err("1000 is not a power of two");
    assert!(err.contains("power of two"), "unexpected error: {err}");
}
