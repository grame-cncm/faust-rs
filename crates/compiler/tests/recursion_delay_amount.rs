//! A delay whose amount is not a literal, applied to a recursion's output.
//!
//! `y = x + g * y'@d` with `d` a slider (or a signal) reads the recursion's
//! shift array at a runtime slot. The read used the sizing bound instead of
//! the amount (`fRec0[bound + 1]` whatever the slider), so the feedback of
//! `+ ~ (@(int(d)) : *(g))` never arrived: the output stayed at the input.

use std::io::Cursor;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

fn render(stem: &str, source: &str, frames: usize) -> Vec<f32> {
    let path = std::env::temp_dir().join(format!(
        "faust-rs-recursion-delay-{stem}-{}.dsp",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("write temp dsp");
    let fbc = Compiler::new()
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{stem}: compilation failed: {e}"));
    let _ = std::fs::remove_file(&path);
    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader).expect("parse fbc");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    assert_eq!(instance.get_num_inputs(), 0, "{stem}: no inputs expected");
    let mut out = vec![0.0_f32; frames];
    let inputs: [&[f32]; 0] = [];
    let mut outs: [&mut [f32]; 1] = [&mut out];
    instance
        .try_compute(frames as i32, &inputs, &mut outs)
        .unwrap_or_else(|e| panic!("{stem}: compute failed: {e}"));
    out
}

/// The slider-driven amount reads the same slot as the literal it is set to.
#[test]
fn a_slider_amount_on_the_feedback_path_reads_the_slot_of_its_value() {
    let variable = render(
        "slider",
        r#"d = hslider("d", 3, 1, 8, 1);
g = hslider("g", 0.5, -0.9, 0.9, 0.001);
process = 0.5 : + ~ (@(int(d)) : *(g));"#,
        16,
    );
    let literal = render(
        "literal",
        r#"g = hslider("g", 0.5, -0.9, 0.9, 0.001);
process = 0.5 : + ~ (@(3) : *(g));"#,
        16,
    );
    assert_eq!(variable, literal);
    // The feedback does arrive: y[4] = 0.5 + 0.5 * y[0].
    assert!((literal[4] - 0.75).abs() < 1e-6, "y[4] = {}", literal[4]);
}

/// An amount that changes every sample (3, 7, 3, ...): `y[n] = 0.5 + g *
/// y[n - 1 - d[n]]`, against the recurrence computed here.
#[test]
fn a_time_varying_amount_on_the_feedback_path_follows_the_recurrence() {
    let frames = 24;
    let out = render(
        "time-varying",
        r#"g = hslider("g", 0.5, -0.9, 0.9, 0.001);
counter = +(1) ~ _;
d = min(7, max(0, 3 + 4 * (counter % 2)));
process = 0.5 : + ~ (@(d) : *(g));"#,
        frames,
    );
    let mut expected = vec![0.0_f64; frames];
    for n in 0..frames {
        // `counter` is 1 at the first sample: d = 3 + 4 * (counter % 2).
        let d = 3 + 4 * ((n + 1) % 2);
        let past = n as i64 - 1 - d as i64;
        let fed = if past >= 0 {
            expected[past as usize]
        } else {
            0.0
        };
        expected[n] = 0.5 + 0.5 * fed;
    }
    for n in 0..frames {
        assert!(
            (f64::from(out[n]) - expected[n]).abs() < 1e-6,
            "y[{n}] = {} expected {}",
            out[n],
            expected[n]
        );
    }
}
