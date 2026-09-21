//! A closed definition holding a clocked wrapper, referenced outside and inside
//! the body of a recursion written with an unapplied function, compiles to one
//! block (2026-09-21, `propagate::flat::mentions_slot`).
//!
//! Self-contained: the counter, the noise and the clock are written inline, so
//! the test needs no Faust standard library.

use codegen::backends::cpp::CppOptions;
use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

const SOURCE: &str = r#"
    time = (+(1) ~ _) - 1;
    lcg = (*(1103515245) : +(12345)) ~ _;
    noise = lcg * 4.656612873077393e-10;
    clk = (time % 4) == 3;
    t = (clk, noise) : ondemand(exp);
    process = t + ((\(r).(t + 0.5 * r)) ~ _);
"#;

/// Frames 3 to 10 of the program above at 48 kHz, rendered before the change
/// by the release `faustprobe` in double precision: what one block and what
/// two blocks fed the same inputs both produce.
const REFERENCE: [(usize, f64); 8] = [
    (3, 1.4449978220042639),
    (4, 1.8062472775053298),
    (5, 1.9868720052558628),
    (6, 2.077184369131129),
    (7, 2.021301248116919),
    (8, 2.0186195133477747),
    (9, 2.0172786459632026),
    (10, 2.0166082122709166),
];

#[test]
fn closed_clocked_definition_is_one_block_across_a_recursion_body() {
    let cpp = Compiler::new()
        .compile_source_to_cpp_with_lane(
            "closed_clocked_box",
            SOURCE,
            &CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|error| panic!("C++ compilation failed: {error}"));
    let exp_calls = cpp.matches("exp(").count();
    assert_eq!(
        exp_calls, 1,
        "the one `ondemand(exp)` definition must be emitted once, found {exp_calls} `exp(` in:\n{cpp}"
    );
}

#[test]
fn closed_clocked_definition_renders_as_before() {
    let fbc = Compiler::new()
        .compile_source_to_interp_with_lane(
            "closed_clocked_box",
            SOURCE,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|error| panic!("interp compilation failed: {error}"));
    let mut reader = std::io::Cursor::new(fbc);
    let mut factory =
        read_fbc::<f32>(&mut reader).unwrap_or_else(|error| panic!("fbc parse failed: {error}"));
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let frames = 12;
    let mut out0 = vec![0.0_f32; frames];
    let mut outputs: [&mut [f32]; 1] = [&mut out0];
    instance
        .try_compute(frames as i32, &[], &mut outputs)
        .unwrap_or_else(|error| panic!("interp execution failed: {error}"));
    for (frame, expected) in REFERENCE {
        let got = f64::from(outputs[0][frame]);
        assert!(
            (got - expected).abs() < 1e-4,
            "frame {frame}: got {got}, expected {expected}"
        );
    }
}

/// The shape of the X-ray learner of faust-diff-demo, without its libraries:
/// a program written as `process(xin, tin)`, whose two inputs are slots the
/// whole body mentions, a descent whose loss holds a clocked model under `fad`,
/// and an output model, fed by the descent's values, written inside a `~` body
/// (the skip test of `ts808.lib`'s `stage_repeat_live`). The descent is then
/// referenced under a second environment, lifted, that binds the same two
/// slots to the same signals: keyed on the environment restricted to its free
/// slots, and memoised from its first call as a recursion holding a `fad`, it
/// is one learner. Before 2026-09-21 the output model's reference expanded it
/// again: a second twin of the model and a second step block, one more `exp(`
/// per block here, three of five clocked blocks of the real learner, and 55
/// instead of 32 ms per second of audio for it.
const XRAY_SHAPE: &str = r#"
    time = (+(1) ~ _) - 1;
    clock = (time % 256) == 255;
    clk2 = (time % 4) == 3;
    smooth(a, x) = (x * (1.0 - a) + _ * a) ~ _;
    mse(y, t) = (y - t) * (y - t);
    frame_sum(c, v) = (+(v)) ~ *(1.0 - c');
    frame_mean(c, v) = frame_sum(c, v) / frame_sum(c, 1.0);
    model(a, b, x) = ((clk2, a * x) : ondemand(\(u).(exp(u) - 1.0))) * b : smooth(0.9);
    descend(c, loss, i1, i2) = (loop ~ (_, _)) : (+(i1), +(i2))
    with {
        loop(prev1, prev2) = (c, prev1, prev2, gm1, gm2) : ondemand(step)
        with {
            p1 = i1 + prev1; p2 = i2 + prev2;
            gs = fad(loss(p1, p2), (p1, p2)) : !, _, _;
            gm1 = frame_mean(c, (gs : _, !));
            gm2 = frame_mean(c, (gs : !, _));
            step(q1, q2, h1, h2) = q1 - 0.01 * h1, q2 - 0.01 * h2;
        };
    };
    process(xin, tin) = y, tin - y, a, b
    with {
        x = 0.2 * xin;
        p = descend(clock, loss, 1.0, 1.0);
        a = p : _, !; b = p : !, _;
        loss(a, b) = mse(model(a, b, x), tin);
        y = (\(v).(model(a, b, x) + 0.0 * v)) ~ _;
    };
"#;

#[test]
fn a_descent_read_under_a_lifted_environment_is_one_learner() {
    let cpp = Compiler::new()
        .compile_source_to_cpp_with_lane(
            "xray_shape",
            XRAY_SHAPE,
            &CppOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|error| panic!("C++ compilation failed: {error}"));
    let exp_calls = cpp.matches("exp(").count();
    assert_eq!(
        exp_calls, 2,
        "the model's twin in the loss and the output model, two `exp(`, found {exp_calls} in:\n{cpp}"
    );
}
