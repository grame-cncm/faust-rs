//! The probe says what it did (phase F1 of
//! `porting/faustprobe-feedback-quality-analysis-and-plan-2026-09-17-en.md`).
//!
//! Four places where a run did something other than what its command line
//! asked, and printed numbers that looked like measurements either way:
//!
//! - **range**: a value outside its control's range was clamped in silence, so
//!   a sweep printed rows labelled 7 and 100 that measured 1. It is an error,
//!   or under `--clamp` a clamp that is reported, the row carrying the value
//!   used;
//! - **numbers**: nine fixed decimals kept two digits of 3.3e-8. The text now
//!   parses back to the very float, at the program's width; `--precision 9` is
//!   the old text; `--out` writes the window in binary;
//! - **failures**: a non-finite render knew its first bad frame and discarded
//!   it; `--fail-above` catches a runaway where it starts;
//! - **silence**: exact zeros came with no word about the button at 0.
//!
//! Every expected frame, value and count below follows from the definition of
//! the fixture, not from a run of the tool.

use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::render::InputMode;
use cranelift_ffi::probe::schedule::{Event, Schedule};
use std::rc::Rc;

mod common;
use common::probe_source;

/// A gain on a 0..1 slider: what a request of 7 must not silently become.
const GAIN: &str = r#"
g = hslider("gain", 0.5, 0, 1, 0.001);
process = _ * g;
"#;

/// Constants no short decimal text holds, the second one small.
const CONSTANTS: &str = "process = 1.0 / 3.0, 1.0e-7 / 3.0, 3.141592653589793;\n";

/// `n` counts 1, 2, 3, ... from frame 0, so `500 - n` is negative from frame
/// 500 on (n = 501) and its square root is NaN there, not before.
const NAN_AT_500: &str = "n = +(1) ~ _;\nprocess = (500 - n) : sqrt;\n";

/// `0.5 * (frame + 1)`: above 100 first at frame 200, where it is 100.5.
const RAMP: &str = "process = (+(1) ~ _) : *(0.5);\n";

/// A one-pole loop, stable below `g = 1` and running away above it.
const LOOP: &str = r#"
g = hslider("g", 0.5, 0, 2, 0.001);
process = + ~ *(g);
"#;

/// Exact silence until its button is pressed.
const GATED: &str = "process = button(\"gate\") * 0.5;\n";

fn probe(source: &str, double: bool) -> Probe {
    let factory =
        Factory::compile_from_string("feedback_test", source, &[], double, 0).expect("compile");
    Probe::instantiate(&Rc::new(factory), 44_100).expect("instantiate")
}

/// The samples of the first frame, from the library: the independent source
/// the binary's text is compared with.
fn first_frame(source: &str, double: bool) -> Vec<f64> {
    let mut first = Vec::new();
    let spec = RenderSpec {
        frames: 1,
        input: InputMode::Zero,
        ..RenderSpec::default()
    };
    probe(source, double).render(&spec, |_, samples| first = samples.to_vec());
    first
}

/// The numbers of the CSV row of `frame`.
fn row(stdout: &str, frame: usize) -> Vec<String> {
    let prefix = format!("{frame},");
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no row for frame {frame} in:\n{stdout}"))
        .split(',')
        .map(str::to_owned)
        .collect()
}

// ------------------------------------------------------------------ range

#[test]
fn a_write_outside_the_range_is_known_before_any_render() {
    let probe = probe(GAIN, false);
    let inside = probe.controls().check_write("gain", 0.25).unwrap();
    assert!(inside.in_range());
    assert!((inside.applied - 0.25).abs() < f64::EPSILON);
    // a bound is a value the control can hold
    assert!(
        probe
            .controls()
            .check_write("gain", 1.0)
            .unwrap()
            .in_range()
    );

    let outside = probe.controls().check_write("gain", 7.0).unwrap();
    assert!(!outside.in_range());
    assert!((outside.applied - 1.0).abs() < f64::EPSILON);
    let message = outside.range_error("gain");
    assert!(message.contains("`gain`=7"), "{message}");
    assert!(message.contains("[0, 1]"), "{message}");
    assert!(message.contains("/feedback_test/gain"), "{message}");

    assert!(
        !probe
            .controls()
            .check_write("gain", f64::NAN)
            .unwrap()
            .in_range()
    );
    assert!(probe.controls().check_write("nope", 0.5).is_err());
}

#[test]
fn set_sweep_and_at_refuse_a_value_outside_the_range() {
    for args in [
        &["--set", "gain=7", "-n", "8"][..],
        &["--sweep", "gain=0.5,1,7,100", "--reduce", "peak", "-n", "8"][..],
        &["--at", "4", "gain=-0.5", "-n", "8"][..],
    ] {
        let (ok, stdout, stderr) = probe_source("range", GAIN, args);
        assert!(!ok, "{args:?} passed");
        // before any render: not one row of a sweep that would mislead
        assert!(stdout.is_empty(), "{args:?} printed:\n{stdout}");
        assert!(
            stderr.contains("is outside the range [0, 1] of /"),
            "{stderr}"
        );
        assert!(stderr.contains("--clamp"), "{stderr}");
    }
    // the bounds themselves are in the range
    let (ok, _, stderr) = probe_source("bounds", GAIN, &["--sweep", "gain=0,1", "-n", "8"]);
    assert!(ok, "{stderr}");
}

#[test]
fn a_value_typed_on_a_decimal_bound_is_in_range_in_both_widths() {
    // The bounds reach the probe in single precision: 0.7 as 0.699999988, below
    // the 0.7 a command line types. 0 and 1, which the test above uses, are
    // exact and could not show it.
    let source = "process = hslider(\"x\", 0.4, 0.1, 0.7, 0.01);\n";
    for width in [&[][..], &["--double"][..]] {
        let args = [
            width,
            &[
                "--sweep",
                "x=0.1,0.7",
                "--reduce",
                "dc",
                "--in",
                "zero",
                "-n",
                "4",
            ],
        ]
        .concat();
        let (ok, stdout, stderr) = probe_source("decimal_bounds", source, &args);
        assert!(ok, "{width:?}: {stderr}");
        assert_eq!(stdout.lines().count(), 3, "{stdout}");
    }
    // a double-precision program receives the value typed, not the bound's float
    let (_, stdout, _) = probe_source(
        "decimal_value",
        source,
        &["--double", "--set", "x=0.7", "--in", "zero", "-n", "1"],
    );
    assert_eq!(row(&stdout, 0), ["0.7"]);
    // and a value really outside is still refused, the range printed as declared
    let (ok, _, stderr) = probe_source("decimal_outside", source, &["--set", "x=0.71", "-n", "1"]);
    assert!(!ok);
    assert!(
        stderr.contains("outside the range [0.1, 0.7] of /"),
        "{stderr}"
    );
}

#[test]
fn clamp_is_reported_and_a_sweep_row_carries_the_value_used() {
    let (ok, stdout, stderr) = probe_source(
        "clamp_sweep",
        GAIN,
        &[
            "--clamp",
            "--sweep",
            "gain=0.5,7",
            "--reduce",
            "peak",
            "--in",
            "dc",
            "-n",
            "8",
        ],
    );
    assert!(ok, "{stderr}");
    assert!(stderr.contains("# clamped /"), "{stderr}");
    assert!(stderr.contains("gain: 7 -> 1"), "{stderr}");
    let rows: Vec<&str> = stdout.lines().skip(1).collect();
    // a constant 1 through the gain: the peak is the gain that was used
    assert_eq!(rows, ["0.5,0.5", "1,1.0"], "{stdout}");

    let (ok, stdout, _) = probe_source(
        "clamp_set",
        GAIN,
        &[
            "--clamp", "--set", "gain=7", "--in", "dc", "-n", "8", "--quiet",
        ],
    );
    assert!(ok);
    assert!(stdout.contains("gain: 7 -> 1"), "{stdout}");
}

#[test]
fn clamp_is_in_the_json_of_the_runs_it_concerns() {
    let (ok, stdout, stderr) = probe_source(
        "clamp_json",
        GAIN,
        &[
            "--clamp",
            "--sweep",
            "gain=0.5,7",
            "--in",
            "dc",
            "-n",
            "8",
            "--format",
            "json",
        ],
    );
    assert!(ok, "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let runs = document["runs"].as_array().unwrap();
    assert!(runs[0].get("clamped").is_none(), "{}", runs[0]);
    let clamp = &runs[1]["clamped"][0];
    assert_eq!(clamp["requested"], 7.0);
    assert_eq!(clamp["applied"], 1.0);
    assert!(clamp["path"].as_str().unwrap().ends_with("/gain"));
    // `set` is what the render used
    assert_eq!(runs[1]["set"]["gain"], 1.0);
}

// ---------------------------------------------------------------- numbers

#[test]
fn the_text_of_a_sample_parses_back_to_the_very_float() {
    let (ok, stdout, _) = probe_source(
        "numbers_double",
        CONSTANTS,
        &["--double", "--in", "zero", "-n", "1"],
    );
    assert!(ok);
    let expected = first_frame(CONSTANTS, true);
    for (text, value) in row(&stdout, 0).iter().zip(&expected) {
        assert_eq!(
            text.parse::<f64>().unwrap().to_bits(),
            value.to_bits(),
            "{text}"
        );
    }
    // the small one keeps its digits: nine fixed decimals left it two
    assert_eq!(row(&stdout, 0)[1], "3.3333333333333334e-8");

    let (ok, stdout, _) = probe_source("numbers_single", CONSTANTS, &["--in", "zero", "-n", "1"]);
    assert!(ok);
    let expected = first_frame(CONSTANTS, false);
    for (text, value) in row(&stdout, 0).iter().zip(&expected) {
        assert_eq!(
            text.parse::<f32>().unwrap().to_bits(),
            (*value as f32).to_bits(),
            "{text}"
        );
        // and it is the f32's text, not its double's seventeen digits
        assert!(text.len() <= 13, "{text}");
    }
}

#[test]
fn precision_nine_is_the_text_the_tool_used_to_print() {
    let (ok, stdout, _) = probe_source(
        "numbers_fixed",
        CONSTANTS,
        &["--double", "--precision", "9", "--in", "zero", "-n", "1"],
    );
    assert!(ok);
    let expected: Vec<String> = first_frame(CONSTANTS, true)
        .iter()
        .map(|value| format!("{value:.9}"))
        .collect();
    assert_eq!(row(&stdout, 0), expected);
    assert_eq!(row(&stdout, 0)[1], "0.000000033");
}

#[test]
fn a_single_precision_control_is_listed_at_its_own_width() {
    let (ok, stdout, _) = probe_source("list", GAIN, &["--list-params"]);
    assert!(ok);
    let line = stdout.lines().find(|l| l.contains("/gain")).unwrap();
    // the step, 0.001, once read 0.0010000000474974513
    assert!(line.trim_end().ends_with(" 0.001"), "{line}");
}

// -------------------------------------------------------------------- out

#[test]
fn an_npy_holds_the_window_at_the_program_width() {
    let out = std::env::temp_dir().join(format!("faustprobe_feedback_{}.npy", std::process::id()));
    let (ok, stdout, stderr) = probe_source(
        "npy",
        CONSTANTS,
        &[
            "--double",
            "--in",
            "zero",
            "-n",
            "6",
            "--skip",
            "2",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(ok, "{stderr}");
    // the statistics are the output; the dump is in the file
    assert!(!stdout.contains("frame,"), "{stdout}");
    assert!(stdout.contains("window=2..6 (4 frames)"), "{stdout}");

    let bytes = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    let header_len = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
    let dict = std::str::from_utf8(&bytes[10..10 + header_len]).unwrap();
    assert!(dict.contains("'descr': '<f8'"), "{dict}");
    assert!(dict.contains("'shape': (4, 3)"), "{dict}");
    let data = &bytes[10 + header_len..];
    assert_eq!(data.len(), 4 * 3 * 8);
    let expected = first_frame(CONSTANTS, true);
    for (k, bytes) in data.as_chunks::<8>().0.iter().enumerate() {
        let value = f64::from_le_bytes(*bytes);
        assert_eq!(value.to_bits(), expected[k % 3].to_bits());
    }
}

#[test]
fn a_wav_written_by_out_is_the_excitation_in_file_reads() {
    let out = std::env::temp_dir().join(format!("faustprobe_feedback_{}.wav", std::process::id()));
    let (ok, _, stderr) = probe_source(
        "wav_write",
        RAMP,
        &[
            "--double",
            "--in",
            "zero",
            "-n",
            "16",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(ok, "{stderr}");
    let (_, direct, _) = probe_source(
        "wav_direct",
        RAMP,
        &["--double", "--in", "zero", "-n", "16"],
    );
    let input = format!("file:{}", out.display());
    let (ok, replayed, stderr) = probe_source(
        "wav_read",
        "process = _;\n",
        &["--double", "--in", &input, "-n", "16"],
    );
    let _ = std::fs::remove_file(&out);
    assert!(ok, "{stderr}");
    assert_eq!(replayed, direct);
}

#[test]
fn out_refuses_what_it_cannot_honour() {
    let out =
        std::env::temp_dir().join(format!("faustprobe_feedback_refuse_{}", std::process::id()));
    let npy = format!("{}.npy", out.display());
    let f32_path = format!("{}.f32", out.display());
    let f64_path = format!("{}.f64", out.display());
    for (source, args, expected) in [
        (
            GAIN,
            vec!["--out", &npy, "--sweep", "gain=0,1"],
            "--sweep cannot be combined",
        ),
        (
            GAIN,
            vec!["--out", &npy, "--every", "2"],
            "--every cannot be combined",
        ),
        (GAIN, vec!["--out", &f32_path, "--double"], "drop digits"),
        (CONSTANTS, vec!["--out", &f64_path], "one channel"),
    ] {
        let (ok, _, stderr) = probe_source("out_refuse", source, &args);
        assert!(!ok, "{args:?} passed");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
}

// --------------------------------------------------------------- failures

#[test]
fn a_non_finite_render_says_where_it_starts() {
    let (ok, _, stderr) =
        probe_source("nan", NAN_AT_500, &["--in", "zero", "-n", "600", "--quiet"]);
    assert!(!ok);
    assert!(
        stderr.contains("render produced non-finite samples"),
        "{stderr}"
    );
    assert!(
        stderr.contains("first: frame 500, out0 (NaN); 100 of 600 frames affected"),
        "{stderr}"
    );
    assert!(stderr.contains("all at their initial values"), "{stderr}");

    // the library has the same facts
    let spec = RenderSpec {
        frames: 600,
        input: InputMode::Zero,
        ..RenderSpec::default()
    };
    let stats = probe(NAN_AT_500, false).render(&spec, |_, _| {});
    let (channel, located) = stats.first_non_finite().unwrap();
    assert_eq!((channel, located.frame), (0, 500));
    assert!(located.value.is_nan());
    assert_eq!(stats.non_finite_frames, 100);
}

#[test]
fn a_failure_names_the_writes_before_it_and_not_those_after() {
    let (ok, _, stderr) = probe_source(
        "loop_events",
        LOOP,
        &[
            "--in", "dc", "-n", "4000", "--quiet", "--set", "g=0.25", "--at", "1000", "g=1.5",
            "--at", "3900", "g=0.1",
        ],
    );
    assert!(!ok);
    assert!(stderr.contains("(+inf)"), "{stderr}");
    assert!(stderr.contains("controls written by then: /"), "{stderr}");
    // g was 0.25, then 1.5 from frame 1000: the value at the failure
    assert!(stderr.contains("/g=1.5"), "{stderr}");
    assert!(
        stderr.contains("last scheduled write before it: frame 1000, /"),
        "{stderr}"
    );
    assert!(!stderr.contains("3900"), "{stderr}");
}

#[test]
fn fail_above_locates_the_first_sample_over_the_level() {
    let (ok, _, stderr) = probe_source(
        "ramp",
        RAMP,
        &[
            "--in",
            "zero",
            "-n",
            "400",
            "--quiet",
            "--fail-above",
            "100",
        ],
    );
    assert!(!ok);
    assert!(
        stderr.contains("a sample exceeds --fail-above 100"),
        "{stderr}"
    );
    assert!(
        stderr.contains("first: frame 200, out0 = 100.5"),
        "{stderr}"
    );

    // the window is what is measured: a transient before --skip is not
    let (ok, _, stderr) = probe_source(
        "ramp_window",
        RAMP,
        &[
            "--in",
            "zero",
            "-n",
            "400",
            "--skip",
            "300",
            "--quiet",
            "--fail-above",
            "100",
        ],
    );
    assert!(!ok);
    assert!(
        stderr.contains("first: frame 300, out0 = 150.5"),
        "{stderr}"
    );

    let (ok, stdout, _) = probe_source(
        "ramp_under",
        RAMP,
        &[
            "--in",
            "zero",
            "-n",
            "400",
            "--quiet",
            "--fail-above",
            "1e9",
        ],
    );
    assert!(ok);
    // the peak of a ramp is its last frame
    assert!(stdout.contains("peak_at=399"), "{stdout}");
}

#[test]
fn a_runaway_is_reported_where_it_starts_not_where_it_overflows() {
    let (ok, _, stderr) = probe_source(
        "runaway",
        LOOP,
        &[
            "--in",
            "dc",
            "-n",
            "4000",
            "--quiet",
            "--at",
            "1000",
            "g=1.5",
            "--fail-above",
            "1000",
        ],
    );
    assert!(!ok);
    let above = stderr
        .find("a sample exceeds --fail-above 1000")
        .expect(&stderr);
    let overflow = stderr
        .find("the render turns non-finite at frame")
        .expect(&stderr);
    assert!(above < overflow, "{stderr}");

    // the scheduled write is what the library's limit sees too
    let mut schedule = Schedule::new();
    schedule.push(
        1000,
        Event::SetParam {
            path: "g".to_owned(),
            value: 1.5,
        },
    );
    let spec = RenderSpec {
        frames: 4000,
        input: InputMode::Dc,
        schedule,
        limit: Some(1000.0),
        ..RenderSpec::default()
    };
    let stats = probe(LOOP, false).render(&spec, |_, _| {});
    let (_, above) = stats.first_above().unwrap();
    let (_, overflow) = stats.first_non_finite().unwrap();
    assert!(above.frame > 1000 && above.frame < overflow.frame);
}

// ---------------------------------------------------------------- silence

#[test]
fn exact_silence_comes_with_the_facts_that_explain_it() {
    let (ok, stdout, _) = probe_source("gated", GATED, &["-n", "64", "--quiet"]);
    assert!(ok, "silence is not an error");
    assert!(
        stdout.contains("# note: every output is exactly zero over the window"),
        "{stdout}"
    );
    assert!(
        stdout.contains("# note: buttons and checkboxes at 0: /"),
        "{stdout}"
    );
    assert!(stdout.contains("/gate"), "{stdout}");
    assert!(stdout.contains("peak_at=none"), "{stdout}");

    let (_, stdout, _) = probe_source(
        "wire",
        "process = _;\n",
        &["--in", "zero", "-n", "64", "--quiet"],
    );
    assert!(
        stdout.contains("# note: input is `zero` and the program has 1 input(s)"),
        "{stdout}"
    );
}

#[test]
fn a_program_that_sounds_gets_no_note_however_quiet() {
    let (_, stdout, _) = probe_source(
        "pressed",
        GATED,
        &["-n", "64", "--quiet", "--set", "gate=1"],
    );
    assert!(!stdout.contains("# note:"), "{stdout}");
    // quiet is not silent: the comparison is with exact zero
    let (_, stdout, _) = probe_source(
        "faint",
        "process = 1.0e-30;\n",
        &["--in", "zero", "-n", "8", "--quiet"],
    );
    assert!(!stdout.contains("# note:"), "{stdout}");
}

#[test]
fn silence_is_noted_in_json_and_once_for_a_silent_sweep() {
    let (_, stdout, _) = probe_source("gated_json", GATED, &["-n", "64", "--format", "json"]);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let notes = document["runs"][0]["notes"].as_array().unwrap();
    assert!(notes[1].as_str().unwrap().contains("/gate"), "{notes:?}");

    let source = "g = hslider(\"gain\", 0.5, 0, 1, 0.001);\nprocess = button(\"gate\") * g;\n";
    let (ok, stdout, stderr) = probe_source(
        "gated_sweep",
        source,
        &["--sweep", "gain=0.25,0.5", "--reduce", "peak", "-n", "64"],
    );
    assert!(ok);
    assert_eq!(stdout.lines().count(), 3, "{stdout}");
    assert_eq!(
        stderr.matches("every output is exactly zero").count(),
        1,
        "{stderr}"
    );
    assert!(stderr.contains("(at every sweep point)"), "{stderr}");
}

// ------------------------------------------------- training and polyphony

/// A loss and its gradient on one slider in `[-1, 1]`, written by hand so
/// that the test needs no `rad`: loss `(w - 0.5)^2`, gradient `2 (w - 0.5)`.
const HOST_LOOP: &str = r#"
w = hslider("w", 0, -1, 1, 0.0001);
process = (w - 0.5) * (w - 0.5), 2 * (w - 0.5);
"#;

#[test]
fn a_training_run_refuses_a_starting_point_outside_the_range() {
    let args = [
        "--double", "--in", "zero", "--block", "8", "--blocks", "3", "--train", "w",
    ];
    let (ok, stdout, stderr) = probe_source(
        "train_range",
        HOST_LOOP,
        &[&args[..], &["--set", "w=5"]].concat(),
    );
    assert!(!ok);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("`w`=5 is outside the range [-1, 1]"),
        "{stderr}"
    );

    let (ok, stdout, stderr) = probe_source(
        "train_clamp",
        HOST_LOOP,
        &[&args[..], &["--set", "w=5", "--clamp"]].concat(),
    );
    assert!(ok, "{stderr}");
    assert!(stdout.starts_with("# clamped /"), "{stdout}");
    assert!(stdout.contains("w: 5 -> 1"), "{stdout}");
}

#[test]
fn a_trained_value_is_printed_whole() {
    let (ok, stdout, stderr) = probe_source(
        "train_text",
        HOST_LOOP,
        &[
            "--double",
            "--in",
            "zero",
            "--block",
            "8",
            "--blocks",
            "2",
            "--optimizer",
            "sgd",
            "--lr",
            "0.125",
            "--train",
            "w",
        ],
    );
    assert!(ok, "{stderr}");
    // w: 0 -> 0 - 0.125 * 2 * (0 - 0.5) = 0.125 -> 0.125 + 0.125 * 0.75 = 0.21875
    assert!(stdout.contains("# trained /"), "{stdout}");
    assert!(
        stdout.trim_end().lines().any(|l| l.ends_with("w=0.21875")),
        "{stdout}"
    );
    // the loss of the second block, (0.125 - 0.5)^2, in scientific text
    assert!(stdout.contains("2,1.40625e-1,0.21875"), "{stdout}");
}

#[test]
fn a_polyphonic_render_with_no_note_says_why_it_is_silent() {
    let voice = r#"
freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.8, 0, 1, 0.001);
gate = button("gate");
process = gate * gain * (freq / 20000);
"#;
    let (ok, stdout, stderr) = probe_source(
        "poly_silent",
        voice,
        &["--nvoices", "2", "-n", "256", "--quiet"],
    );
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("# note: every output is exactly zero over the window"),
        "{stdout}"
    );
    assert!(
        stdout.contains("no --note or --chord is scheduled"),
        "{stdout}"
    );

    let (ok, stdout, stderr) = probe_source(
        "poly_note",
        voice,
        &["--nvoices", "2", "-n", "256", "--quiet", "--note", "69@0"],
    );
    assert!(ok, "{stderr}");
    assert!(!stdout.contains("# note:"), "{stdout}");
}

// ------------------------------------------- the polyphonic path and ranges
//
// F1 left `--nvoices` out: a write of the command line outside its control's
// range was written as it is on the voices (a state no host produces) and
// clamped in silence on the effect, and `--clamp` was refused. The rule is the
// scalar one now: an error, or under `--clamp` a clamp that is reported. What a
// note writes is another matter, and is pinned below.

/// A voice that outputs its `level` while a note is held, and an effect that
/// is a gain: what was written is read off the peak, `level * drive`.
const INSTRUMENT: &str = r#"
freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.5, 0, 1, 0.001);
gate = button("gate");
level = hslider("level", 0.5, 0, 1, 0.001);
tone = hslider("tone", 0.4, 0.1, 0.7, 0.01);
meter = level : hbargraph("meter", 0, 1);
process = gate * meter + 0 * (freq + gain + tone);
effect = _ * hslider("drive", 1, 0, 2, 0.001);
"#;

fn poly(name: &str, extra: &[&str]) -> (bool, String, String) {
    let mut args = vec![
        "--double",
        "--nvoices",
        "2",
        "--note",
        "60@0",
        "-n",
        "64",
        "--quiet",
    ];
    args.extend(extra);
    probe_source(name, INSTRUMENT, &args)
}

fn peak_of(stdout: &str) -> f64 {
    let line = stdout
        .lines()
        .find(|l| l.starts_with("# out0"))
        .unwrap_or_else(|| panic!("no statistics: {stdout}"));
    line.split("peak=")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("{line}"))
}

#[test]
fn a_polyphonic_write_outside_the_range_is_refused_on_a_voice_and_on_the_effect() {
    // in range, the two writes are what the peak shows
    let (ok, stdout, stderr) = poly("poly_in_range", &["--set", "level=1", "--set", "drive=2"]);
    assert!(ok, "{stderr}");
    assert!((peak_of(&stdout) - 2.0).abs() < 1e-12);
    assert!(!stdout.contains("# clamped"));

    for (extra, place) in [
        (vec!["--set", "level=7"], "/level on every voice"),
        (vec!["--set", "drive=9"], "/drive on the effect"),
        // a scheduled write is known before any render too
        (vec!["--at", "32", "level=7"], "/level on every voice"),
        (vec!["--at", "32", "drive=9"], "/drive on the effect"),
    ] {
        let (ok, stdout, stderr) = poly("poly_refused", &extra);
        assert!(!ok, "{extra:?} must be refused");
        assert!(
            stdout.is_empty(),
            "{extra:?}: nothing is rendered: {stdout}"
        );
        assert!(stderr.contains("is outside the range [0, "), "{stderr}");
        assert!(stderr.contains(place), "{extra:?}: {stderr}");
        assert!(stderr.contains("--clamp accepts it"), "{stderr}");
    }
}

#[test]
fn clamp_is_accepted_and_reported_under_nvoices() {
    let (ok, stdout, stderr) = poly(
        "poly_clamp",
        &["--clamp", "--set", "level=7", "--set", "drive=9"],
    );
    assert!(ok, "{stderr}");
    // 1 * 2: not 7 on the voices, as it was, nor 9 on the effect
    assert!((peak_of(&stdout) - 2.0).abs() < 1e-12, "{stdout}");
    let clamps: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("# clamped"))
        .collect();
    assert_eq!(clamps.len(), 2, "{stdout}");
    assert!(clamps[0].ends_with("/level: 7 -> 1"), "{}", clamps[0]);
    assert!(clamps[1].ends_with("/drive: 9 -> 2"), "{}", clamps[1]);

    // a scheduled write is clamped alike: 0.5 for 32 frames, then 1
    let (ok, stdout, stderr) = poly("poly_clamp_at", &["--clamp", "--at", "32", "level=7"]);
    assert!(ok, "{stderr}");
    assert!((peak_of(&stdout) - 1.0).abs() < 1e-12, "{stdout}");
    assert!(
        stdout.contains(&format!("rms={:?}", 0.625_f64.sqrt())),
        "{stdout}"
    );
    // the same clamp by --set and by --at is said once
    let (_, stdout, _) = poly(
        "poly_clamp_once",
        &["--clamp", "--set", "level=7", "--at", "32", "level=7"],
    );
    assert_eq!(stdout.matches("# clamped").count(), 1, "{stdout}");

    // and in the JSON document
    let (ok, stdout, stderr) = probe_source(
        "poly_clamp_json",
        INSTRUMENT,
        &[
            "--double",
            "--nvoices",
            "2",
            "--note",
            "60@0",
            "-n",
            "64",
            "--format",
            "json",
            "--clamp",
            "--set",
            "drive=9",
        ],
    );
    assert!(ok, "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let clamped = document["clamped"].as_array().expect("a clamped array");
    assert_eq!(clamped.len(), 1);
    assert_eq!(clamped[0]["requested"], 9.0);
    assert_eq!(clamped[0]["applied"], 2.0);
}

#[test]
fn a_polyphonic_write_on_a_decimal_bound_or_on_a_bargraph_follows_the_scalar_rule() {
    // 0.7 is the bound as declared, and above the single-precision bound the
    // host knows: in range, in both widths, as for a scalar program
    for width in [&["--double"][..], &[][..]] {
        let mut args = vec!["--nvoices", "2", "--note", "60@0", "-n", "64", "--quiet"];
        args.extend(width);
        args.extend(["--set", "tone=0.7", "--set", "tone=0.1"]);
        let (ok, stdout, stderr) = probe_source("poly_decimal", INSTRUMENT, &args);
        assert!(ok, "{width:?}: {stderr}");
        assert!(!stdout.contains("# clamped"), "{stdout}");
    }
    // a bargraph is an output: it was written without a word
    let (ok, _, stderr) = poly("poly_bargraph", &["--set", "meter=1"]);
    assert!(!ok);
    assert!(
        stderr.contains("is a bargraph") && stderr.contains("on every voice"),
        "{stderr}"
    );
    // and a path that resolves nowhere says so, as before
    let (ok, _, stderr) = poly("poly_unknown", &["--set", "nope=1"]);
    assert!(!ok);
    assert!(
        stderr.contains("no control matching `nope` on any voice or the effect"),
        "{stderr}"
    );
}

/// `poly-dsp.h` writes a voice's frequency as computed from the pitch,
/// whatever the slider declares: note 127 is 12 543.85 Hz on a slider that
/// stops at 1000. That is not a widget's write, and no range applies to it.
#[test]
fn what_a_note_writes_is_not_bound_by_the_slider() {
    let voice = r#"
freq = hslider("freq", 440, 20, 1000, 0.01);
gain = hslider("gain", 0.5, 0, 1, 0.001);
gate = button("gate");
process = gate * (freq / 20000) + 0 * gain;
"#;
    let (ok, stdout, stderr) = probe_source(
        "poly_note_freq",
        voice,
        &[
            "--double",
            "--nvoices",
            "1",
            "--note",
            "127@0",
            "-n",
            "64",
            "--quiet",
        ],
    );
    assert!(ok, "{stderr}");
    let expected = 440.0 * 2.0_f64.powf((127.0 - 69.0) / 12.0) / 20000.0;
    assert!((peak_of(&stdout) - expected).abs() < 1e-9, "{stdout}");
}

// --------------------------------- the polyphonic path: failures, statistics
//
// The polyphonic render had statistics of its own, a peak and an RMS over the
// samples that were finite: a voice that ran away to infinity printed
// `rms=inf` and the command succeeded. It has the scalar path's statistics
// now, and fails as a scalar render fails, with what an instrument's failure
// is explained with: the controls written by then, and the notes held.

/// A voice whose loop gain is its `fb` control: 2 in the steady state at 0.5,
/// a runaway above 1. One held note: `y = 1 + fb * y`.
const RUNAWAY: &str = r#"
freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.5, 0, 1, 0.001);
gate = button("gate");
fb = hslider("fb", 0.5, 0, 4, 0.001);
process = (gate + 0 * (freq + gain)) : (+ ~ *(fb));
"#;

/// The first frame at which `y = 1 + fb y` exceeds `level` (infinity for the
/// overflow), `fb` being 0.5 up to frame 500 and 4 from there.
fn runaway_frame(level: f64) -> usize {
    let mut y = 0.0_f64;
    for frame in 0.. {
        y = 1.0 + if frame < 500 { 0.5 } else { 4.0 } * y;
        if y > level || !y.is_finite() {
            return frame;
        }
    }
    unreachable!()
}

#[test]
fn a_polyphonic_render_that_runs_away_fails_and_says_where_it_starts() {
    let overflow = runaway_frame(f64::MAX);
    assert_eq!(overflow, 1011, "the replay itself");
    let base = [
        "--double",
        "--nvoices",
        "2",
        "-n",
        "2000",
        "--quiet",
        // one note held, one released long before; one write before the
        // failure and one after it
        "--note",
        "60@0",
        "--note",
        "64@0..100",
        "--at",
        "500",
        "fb=4",
        "--at",
        "1900",
        "fb=0.5",
    ];
    let (ok, stdout, stderr) = probe_source("poly_runaway", RUNAWAY, &base);
    assert!(!ok, "a render that is not finite fails: {stdout}");
    assert!(
        stderr.contains(&format!(
            "render produced non-finite samples\n  first: frame {overflow}, out0 (+inf); {} of 2000 frames affected",
            2000 - overflow
        )),
        "{stderr}"
    );
    // the write before it, not the one after it
    assert!(stderr.contains("/fb=4"), "{stderr}");
    assert!(
        stderr.contains("last scheduled write before it: frame 500, "),
        "{stderr}"
    );
    assert!(!stderr.contains("frame 1900"), "{stderr}");
    // the note that is held, not the one that was released
    assert!(
        stderr.ends_with("notes held then: 60 (on at frame 0)\n"),
        "{stderr}"
    );

    // in JSON too, a failure is a failure and not a document with a null in it
    let mut json = base.to_vec();
    json.retain(|a| *a != "--quiet");
    json.extend(["--format", "json"]);
    let (ok, stdout, _) = probe_source("poly_runaway_json", RUNAWAY, &json);
    assert!(!ok);
    assert!(stdout.is_empty(), "{stdout}");
}

#[test]
fn fail_above_catches_a_polyphonic_runaway_at_its_start() {
    let above = runaway_frame(100.0);
    assert_eq!(above, 502, "2, then 9, 37, 149");
    let (ok, _, stderr) = probe_source(
        "poly_fail_above",
        RUNAWAY,
        &[
            "--double",
            "--nvoices",
            "2",
            "-n",
            "2000",
            "--quiet",
            "--note",
            "60@0",
            "--at",
            "500",
            "fb=4",
            "--fail-above",
            "100",
        ],
    );
    assert!(!ok);
    assert!(
        stderr.contains("a sample exceeds --fail-above 100\n  first: frame 502, out0 = 149.0"),
        "{stderr}"
    );
    assert!(
        stderr.contains("the render turns non-finite at frame 1011, out0 (+inf)"),
        "{stderr}"
    );
    assert!(stderr.contains("notes held then: 60"), "{stderr}");
    // under the level nothing fails
    let (ok, _, stderr) = probe_source(
        "poly_fail_above_ok",
        RUNAWAY,
        &[
            "--double",
            "--nvoices",
            "2",
            "-n",
            "2000",
            "--quiet",
            "--note",
            "60@0",
            "--fail-above",
            "100",
        ],
    );
    assert!(ok, "{stderr}");
}

/// An impulse at the note-on, halved at every sample: `2^-k` at frame `k`,
/// subnormal in single precision from frame 127 to frame 149.
#[test]
fn a_polyphonic_render_has_the_statistics_of_a_scalar_one() {
    let decay = r#"
freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.5, 0, 1, 0.001);
gate = button("gate");
process = ((gate - gate') * (gate > 0) + 0 * (freq + gain)) : (+ ~ *(0.5));
"#;
    let args = ["--nvoices", "1", "--note", "60@0", "-n", "300", "--quiet"];
    let (ok, stdout, stderr) = probe_source("poly_stats", decay, &args);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("nvoices=1 active_voices=1 window=0..300 (300 frames)"),
        "{stdout}"
    );
    let line = stdout.lines().find(|l| l.starts_with("# out0")).unwrap();
    assert!(line.starts_with("# out0: peak=1.0 rms="), "{line}");
    assert!(
        line.ends_with(" finite=yes peak_at=0 subnormal=23 subnormal_at=127"),
        "{line}"
    );
    // at the width of the instrument: none in double precision
    let mut double = args.to_vec();
    double.push("--double");
    let (_, stdout, _) = probe_source("poly_stats_double", decay, &double);
    let line = stdout.lines().find(|l| l.starts_with("# out0")).unwrap();
    assert!(line.ends_with(" finite=yes peak_at=0"), "{line}");

    // the window is the one --skip leaves: frames 140 to 149 are subnormal
    let mut skipped = args.to_vec();
    skipped.extend(["--skip", "140"]);
    let (_, stdout, _) = probe_source("poly_stats_skip", decay, &skipped);
    assert!(stdout.contains("window=140..300 (160 frames)"), "{stdout}");
    let line = stdout.lines().find(|l| l.starts_with("# out0")).unwrap();
    assert!(line.ends_with(" subnormal=10 subnormal_at=140"), "{line}");

    // and the JSON channels carry what a scalar run's carry
    let mut json = args.to_vec();
    json.retain(|a| *a != "--quiet");
    json.extend(["--format", "json"]);
    let (ok, stdout, stderr) = probe_source("poly_stats_json", decay, &json);
    assert!(ok, "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let channel = &document["channels"][0];
    assert_eq!(channel["peak"], 1.0);
    assert_eq!(channel["peak_at"], 0);
    assert_eq!(channel["subnormal"], 23);
    assert_eq!(channel["subnormal_at"], 127);
    assert!(channel["dc"].as_f64().unwrap() > 0.0);
    assert_eq!(document["window"]["frames"], 300);
}

/// Under `--clamp` the context of a failure names the value the voices ran
/// with, not the one that was typed.
#[test]
fn a_polyphonic_failure_lists_the_values_that_were_applied() {
    let (ok, _, stderr) = probe_source(
        "poly_context_clamped",
        RUNAWAY,
        &[
            "--double",
            "--nvoices",
            "2",
            "-n",
            "2000",
            "--quiet",
            "--note",
            "60@0",
            "--clamp",
            "--at",
            "500",
            "fb=9",
        ],
    );
    assert!(!ok);
    // 9 is clamped to the slider's 4: the same runaway, the same frame
    assert!(
        stderr.contains("first: frame 1011, out0 (+inf)"),
        "{stderr}"
    );
    assert!(stderr.contains("/fb=4\n"), "{stderr}");
    assert!(!stderr.contains("fb=9"), "{stderr}");
}

// ------------------------------------------------- the polyphonic path: --in
//
// `mydsp_poly::compute` hands the host's inputs to every playing voice
// (`voice->compute(count, inputs, fMixBuffer)`). The port gave every voice
// silence, so `--in` was accepted under `--nvoices` and meant nothing.

/// A voice with an input: the input while its gate is held.
const VOICE_WITH_INPUT: &str = r#"
freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.5, 0, 1, 0.001);
gate = button("gate");
process = _ * (gate + 0 * (freq + gain));
"#;

fn poly_in(name: &str, extra: &[&str]) -> (bool, String, String) {
    let mut args = vec!["--double", "--nvoices", "2", "-n", "200", "--quiet"];
    args.extend(extra);
    probe_source(name, VOICE_WITH_INPUT, &args)
}

#[test]
fn a_polyphonic_instrument_receives_the_excitation_on_every_playing_voice() {
    // one held note: the input, as it is
    let (ok, stdout, stderr) = poly_in("poly_in_dc", &["--note", "60@0", "--in", "dc"]);
    assert!(ok, "{stderr}");
    assert!((peak_of(&stdout) - 1.0).abs() < 1e-12, "{stdout}");
    assert!(stdout.contains("rms=1.0 dc=1.0"), "{stdout}");
    // two held notes: each voice is given the same input, and they are mixed
    let (_, stdout, _) = poly_in("poly_in_chord", &["--chord", "60,64@0", "--in", "dc"]);
    assert!((peak_of(&stdout) - 2.0).abs() < 1e-12, "{stdout}");
    // no note: the input reaches no voice
    let (_, stdout, _) = poly_in("poly_in_free", &["--in", "dc"]);
    assert!(peak_of(&stdout) == 0.0, "{stdout}");

    // the default excitation is one impulse at frame 0, not one per block:
    // a mean of 1/200 over the window, at any block size
    for block in ["64", "7"] {
        let (ok, stdout, stderr) =
            poly_in("poly_in_impulse", &["--note", "60@0", "--block", block]);
        assert!(ok, "{stderr}");
        assert!(
            stdout.contains("dc=0.005 finite=yes peak_at=0"),
            "{block}: {stdout}"
        );
    }
    // and noise is addressed by frame: the same samples however it is cut
    let noise = |block: &str| {
        poly_in(
            "poly_in_noise",
            &["--note", "60@0", "--in", "white:3", "--block", block],
        )
        .1
    };
    assert_eq!(noise("64"), noise("7"));
    assert!(noise("64").contains("peak=0."), "{}", noise("64"));
}

#[test]
fn a_silent_polyphonic_render_says_that_its_voices_have_an_unfed_input() {
    let (ok, stdout, stderr) = poly_in("poly_in_zero", &["--note", "60@0", "--in", "zero"]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("# note: input is `zero` and a voice has 1 input(s)"),
        "{stdout}"
    );
}

/// An impulse on an input that does not exist excites nothing, and the
/// silence that followed did not say why: scalar or polyphonic, it is an
/// error that names the inputs there are.
#[test]
fn an_impulse_on_an_input_the_program_does_not_have_is_refused() {
    let (ok, stdout, stderr) = poly_in("poly_in_channel", &["--note", "60@0", "--in", "impulse:3"]);
    assert!(!ok);
    assert!(stdout.is_empty());
    assert!(
        stderr.contains("--in impulse:3: the program has one input, channel 0"),
        "{stderr}"
    );
    let stereo = "process = _ * 0.5, _ * 0.25;\n";
    let (ok, _, stderr) = probe_source("in_channel", stereo, &["-n", "8", "--in", "impulse:2"]);
    assert!(!ok);
    assert!(
        stderr.contains("the program has 2 inputs, channels 0 to 1"),
        "{stderr}"
    );
    // the last channel is one of them
    let (ok, stdout, stderr) = probe_source(
        "in_channel_ok",
        stereo,
        &["--double", "-n", "2", "--in", "impulse:1"],
    );
    assert!(ok, "{stderr}");
    assert!(
        stdout.starts_with("frame,out0,out1\n0,0.0,0.25\n"),
        "{stdout}"
    );
    let (ok, _, stderr) = probe_source(
        "in_channel_none",
        "process = 0.5;\n",
        &["-n", "8", "--in", "impulse:0"],
    );
    assert!(!ok);
    assert!(stderr.contains("the program has no input"), "{stderr}");
}

/// A gain the render runs with, whose `fb` control is its loop gain.
const SCALAR_RUNAWAY: &str = "fb = hslider(\"fb\", 0.5, 0, 4, 0.001);\nprocess = _ : + ~ *(fb);\n";

/// A control given twice is one control: the context of a failure lists it
/// once, with the value it held. It used to list both `--set` values, the
/// scheduled write having updated the first of them.
#[test]
fn a_control_set_twice_is_listed_once_in_a_failure_context() {
    let (ok, _, stderr) = probe_source(
        "set_twice",
        SCALAR_RUNAWAY,
        &[
            "--double",
            "--quiet",
            "-n",
            "200",
            "--in",
            "dc",
            "--set",
            "fb=1.5",
            "--set",
            "fb=2",
            "--at",
            "3",
            "fb=2.5",
            "--fail-above",
            "100",
        ],
    );
    assert!(!ok);
    assert!(
        stderr.contains("\n  controls written by then: /set_twice/fb=2.5\n"),
        "{stderr}"
    );
    assert!(
        stderr.contains("last scheduled write before it: frame 3, /set_twice/fb=2.5"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("/set_twice/fb=").count(), 2, "{stderr}");
}

/// A polyphonic dump prints nothing before its command line is validated:
/// an impulse on an input the voices do not have leaves no header behind.
#[test]
fn a_polyphonic_dump_prints_nothing_before_its_input_is_validated() {
    let (ok, stdout, stderr) = probe_source(
        "poly_header",
        VOICE_WITH_INPUT,
        &["--double", "--nvoices", "2", "-n", "8", "--in", "impulse:3"],
    );
    assert!(!ok);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("--in impulse:3: the program has one input, channel 0"),
        "{stderr}"
    );
}

/// What the command line says of a clamp is what the render runs with. A NaN
/// is held by no range: `check_write` reports the control's initial value
/// as applied, and `Probe::set` used to clamp on its own, sending the NaN to
/// the zone as it was.
#[test]
fn a_nan_written_to_a_control_is_its_initial_value_as_the_clamp_says() {
    let gain = "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.001);\n";
    let probe = probe(gain, true);
    probe.set("gain", f64::NAN).expect("a slider");
    let (_, value) = probe.control_values()[0];
    assert_eq!(value, 0.5);

    let (ok, stdout, stderr) = probe_source(
        "nan_clamp",
        gain,
        &[
            "--double", "-n", "2", "--in", "dc", "--set", "gain=nan", "--clamp",
        ],
    );
    assert!(ok, "{stderr}");
    assert_eq!(stdout, "frame,out0\n0,0.5\n1,0.5\n");
    assert!(
        stderr.contains("# clamped /nan_clamp/gain: NaN -> 0.5"),
        "{stderr}"
    );
    // and a scheduled one, and a swept one
    let (ok, stdout, _) = probe_source(
        "nan_at",
        gain,
        &[
            "--double", "-n", "4", "--in", "dc", "--at", "2", "gain=nan", "--clamp",
        ],
    );
    assert!(ok);
    assert_eq!(stdout, "frame,out0\n0,0.5\n1,0.5\n2,0.5\n3,0.5\n");
    let (ok, stdout, _) = probe_source(
        "nan_sweep",
        gain,
        &[
            "--double",
            "--quiet",
            "-n",
            "2",
            "--in",
            "dc",
            "--sweep",
            "gain=nan,0.25",
            "--clamp",
            "--reduce",
            "peak",
        ],
    );
    assert!(ok);
    assert_eq!(stdout, "0.5,0.5\n0.25,0.25\n");
}
