//! Trees in normal form are not re-evaluated (`eval/src/normal_form.rs`).
//!
//! `take(n, (x, xs)) = take(n - 1, xs)` binds the tail of a list at every
//! recursion level, in a fresh environment layer; re-evaluating that tail there
//! walked it whole, so indexing every element of a list of `n` was cubic: 4 s at
//! 240 elements, more than two minutes at 480. With the tail recognised as a
//! tree in normal form the walk is skipped and the program below, 480 elements,
//! compiles in about a second.

use std::io::Cursor;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

const N: usize = 480;

#[test]
fn indexing_every_element_of_a_long_list_is_not_cubic() {
    // ba.take, written out so that the test needs no library
    let source = format!(
        r#"take(1, (x, xs)) = x;
take(1, x) = x;
take(n, (x, xs)) = take(n - 1, xs);
g = hslider("g", 0.5, 0, 1, 0.001);
cs = par(i, {N}, g * (i + 1));
process = par(j, {N}, take(j + 1, cs)) :> _;
"#
    );
    let path =
        std::env::temp_dir().join(format!("faust-rs-normal-form-{}.dsp", std::process::id()));
    std::fs::write(&path, &source).expect("write temp dsp");
    let fbc = Compiler::new()
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .expect("the list program compiles");
    let _ = std::fs::remove_file(&path);
    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader).expect("parse fbc");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let mut out = vec![0.0_f32; 4];
    let inputs: [&[f32]; 0] = [];
    let mut outs: [&mut [f32]; 1] = [&mut out];
    instance
        .try_compute(4, &inputs, &mut outs)
        .expect("compute");
    // the sum of g (i + 1) for i < N, with g = 0.5
    let expected = 0.5 * (N * (N + 1) / 2) as f32;
    assert!(
        (out[0] - expected).abs() <= expected * 1e-6,
        "sum of the list: {} expected {expected}",
        out[0]
    );
}
