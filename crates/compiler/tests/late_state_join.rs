//! A sum of a constant, a control and a state term joins the state last.
//!
//! Upstream Faust commit `d4d51e5ed` (2026-08-09, posterior to the pinned
//! reference) adds an "audio-recursive" order to the normalized sums: the
//! terms some state reaches join last, as the right operand. `1 - c - o` is
//! then `(1 - c) - o`, whose slow part is hoisted to block rate, where the
//! reference builds `(1 - o) - c`. The rounding changes as well: a
//! rate-limited follower of `1 - c` reaches 0 exactly, where `1 - o` rounds and
//! left faust-coopiano's damper openness stuck at 1e-8 (single precision) after
//! the pedal. Registry: DIFF-BEH-023.

use std::io::Cursor;

use codegen::backends::cpp::CppOptions;
use codegen::backends::interp::{FbcDspInstance, InterpOptions, MAX_OPT_LEVEL, read_fbc};
use compiler::Compiler;

/// The damper openness of faust-coopiano: a rate-limited follower of `1 - c`.
const FOLLOWER: &str = "st = 1.0 / (0.04 * 44100.0);\n\
c = hslider(\"contact\", 0, 0, 1, 0.01);\n\
process = (\\(o).(o + max(-st, min(st, 1.0 - c - o)))) ~ _;\n";

#[test]
fn the_slow_part_of_a_state_sum_is_hoisted() {
    let cpp = Compiler::new()
        .compile_source_to_cpp("late-join", FOLLOWER, &CppOptions::default())
        .expect("C++ codegen");
    assert!(
        cpp.contains("float fSlow0 = (1.0f - ((float)(fHslider0)));"),
        "{cpp}"
    );
    let step = cpp
        .lines()
        .find(|line| line.contains("float fRecCur0 ="))
        .expect("the recursion");
    assert!(step.contains("(fSlow0 - fRec0)"), "{step}");
}

#[test]
fn a_rate_limited_follower_reaches_zero_exactly() {
    for opt_level in [0, i32::from(MAX_OPT_LEVEL)] {
        let fbc = Compiler::new()
            .compile_source_to_interp(
                "late-join",
                FOLLOWER,
                &InterpOptions {
                    opt_level,
                    ..Default::default()
                },
            )
            .expect("interpreter codegen");
        let mut factory = read_fbc::<f32>(&mut Cursor::new(fbc)).expect("read FBC");
        let mut instance = FbcDspInstance::new(&mut factory);
        instance.init(44100);
        let contact = instance
            .ui_instructions()
            .iter()
            .find(|ui| ui.label == "contact")
            .expect("the contact slider")
            .offset;
        let mut out = vec![f32::NAN; 4096];
        // Dampers lifted: the follower climbs to 1 (1764 samples).
        instance
            .try_compute(4096, &[], &mut [out.as_mut_slice()])
            .expect("compute");
        assert_eq!(out[4095], 1.0, "opt={opt_level}");
        // Dampers seated: it must come back to 0, not to a rounding error.
        assert!(instance.set_real_zone(contact, 1.0));
        instance
            .try_compute(4096, &[], &mut [out.as_mut_slice()])
            .expect("compute");
        assert_eq!(
            out[4095].to_bits(),
            0.0_f32.to_bits(),
            "opt={opt_level}: {}",
            out[4095]
        );
    }
}
