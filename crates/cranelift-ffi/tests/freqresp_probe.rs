//! `--freqresp` (phase F5 of
//! `porting/faustprobe-feedback-quality-analysis-and-plan-2026-09-17-en.md`).
//!
//! The frequency response of a linear program from one impulse response,
//! where a sine sweep took one render per frequency (design §7.1), and the
//! refusal of a program that has no transfer function to give: "the tool
//! should not pretend otherwise".
//!
//! The responses are compared with closed forms (the design's §6.2: a
//! one-pole, and a four-pole ladder of TPT one-poles), and each refusal with
//! the property that breaks and the first frame at which it does.

use std::f64::consts::{PI, TAU};
use std::path::PathBuf;
use std::process::Command;

/// `y[n] = x[n] + 0.5 y[n-1]`: `H = 1 / (1 - 0.5 exp(-jw))`.
const ONE_POLE: &str = "process = + ~ *(0.5);\n";

/// A four-pole ladder of TPT (zero-delay feedback) one-poles, Zavalishin's
/// form, with the feedback solved for: linear for any resonance.
const LADDER: &str = r#"
SR = fconstant(int fSamplingFreq, <math.h>);
fc = hslider("cutoff", 1000, 20, 20000, 1);
k  = hslider("resonance", 0, 0, 3.9, 0.01);
g  = tan(3.141592653589793 * fc / SR);
G  = g / (1 + g);
ladder(s1, s2, s3, s4, x) = n1, n2, n3, n4, y4
with {
    S  = (G * G * G * s1 + G * G * s2 + G * s3 + s4) / (1 + g);
    u  = (x - k * S) / (1 + k * G * G * G * G);
    v1 = (u  - s1) * G;  y1 = v1 + s1;  n1 = y1 + v1;
    v2 = (y1 - s2) * G;  y2 = v2 + s2;  n2 = y2 + v2;
    v3 = (y2 - s3) * G;  y3 = v3 + s3;  n3 = y3 + v3;
    v4 = (y3 - s4) * G;  y4 = v4 + s4;  n4 = y4 + v4;
};
process = ladder ~ (_, _, _, _) : !, !, !, !, _;
"#;

/// `x - x^3 / 3`: a saturator.
const CUBIC: &str = "process = _ <: _ - (_ * _ * _) / 3;\n";
/// A full-wave rectifier: homogeneous for a positive factor only.
const RECTIFIER: &str = "process = abs;\n";
/// A one-pole under a gain that falls with time.
const TREMOLO: &str = "n = +(1) ~ _;\nprocess = (+ ~ *(0.9)) * (1 - n / 1000);\n";
/// The median of three successive samples: homogeneous, time-invariant, and
/// not additive.
const MEDIAN: &str = "med3(x) = max(min(x, x'), min(max(x, x'), x''));\nprocess = med3;\n";
/// A gain and an offset: an output that does not come from the input.
const OFFSET: &str = "process = _ * 0.5 + 0.25;\n";
/// `x + 1e-6 x^3`: a nonlinearity 120 dB down.
const NEARLY_LINEAR: &str = "process = _ <: _ + 0.000001 * (_ * _ * _);\n";
/// The one-pole under a gain smoothed as `si.smoo` does it.
const SMOOTHED: &str = "smoo = *(0.001) : + ~ *(0.999);\n\
gain = hslider(\"gain\", 0.5, 0, 1, 0.01) : smoo;\n\
process = _ * gain : + ~ *(0.5);\n";
/// A one-pole that takes ten thousand frames to fall by 87 dB.
const SLOW: &str = "process = + ~ *(0.999);\n";
/// Two inputs, two outputs, no path across: a gain, and the one-pole.
const TWO_BY_TWO: &str = "process = *(0.5), (+ ~ *(0.5));\n";
/// No input at all.
const GENERATOR: &str = "process = +(0.001) ~ _;\n";

struct Fixtures(PathBuf);

impl Fixtures {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("faustprobe_freqresp_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        Self(dir)
    }

    fn write(&self, file: &str, text: &str) -> String {
        let path = self.0.join(file);
        std::fs::write(&path, text).expect("write fixture");
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn probe(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_faustprobe"))
        .args(args)
        .output()
        .expect("run faustprobe");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The rows of a response: `hz`, then `(mag_db, phase)` per output.
fn rows(stdout: &str) -> Vec<(f64, Vec<(f64, f64)>)> {
    stdout
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("hz"))
        .map(|l| {
            let fields: Vec<f64> = l
                .split(',')
                .map(|f| f.parse().unwrap_or_else(|_| panic!("not a number: {l}")))
                .collect();
            (
                fields[0],
                fields[1..].chunks(2).map(|c| (c[0], c[1])).collect(),
            )
        })
        .collect()
}

/// A complex number, for the closed forms.
#[derive(Clone, Copy)]
struct C(f64, f64);

impl C {
    fn mul(self, o: Self) -> Self {
        Self(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn div(self, o: Self) -> Self {
        let d = o.0 * o.0 + o.1 * o.1;
        Self(
            (self.0 * o.0 + self.1 * o.1) / d,
            (self.1 * o.0 - self.0 * o.1) / d,
        )
    }
    fn db(self) -> f64 {
        20.0 * self.0.hypot(self.1).log10()
    }
    fn arg(self) -> f64 {
        self.1.atan2(self.0)
    }
}

/// `1 / (1 - a exp(-jw))`.
fn one_pole(a: f64, w: f64) -> C {
    C(1.0, 0.0).div(C(1.0 - a * w.cos(), a * w.sin()))
}

/// Two angles are the same direction: `pi` and `-pi` are.
fn same_angle(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d) < 1e-9
}

/// The distance between a printed point and a closed form, as complex
/// numbers: where a response is 170 dB down its phase is known to fewer
/// digits, and its value to just as many.
fn distance(db: f64, phase: f64, h: C) -> f64 {
    let magnitude = 10.0_f64.powf(db / 20.0);
    (magnitude * phase.cos() - h.0).hypot(magnitude * phase.sin() - h.1)
}

// ------------------------------------------------------------ closed forms

#[test]
fn a_one_pole_matches_its_closed_form_in_level_and_phase() {
    let fixtures = Fixtures::new("one_pole");
    let file = fixtures.write("one_pole.dsp", ONE_POLE);
    for sr in [44100.0, 48000.0] {
        let (ok, stdout, stderr) = probe(&[
            "--double",
            "--sr",
            &format!("{sr}"),
            "--freqresp",
            "9:20:20000",
            &file,
        ]);
        assert!(ok, "{stderr}");
        assert!(
            stdout.starts_with("hz,mag_db_out0,phase_out0\n"),
            "{stdout}"
        );
        let rows = rows(&stdout);
        assert_eq!(rows.len(), 9);
        assert_eq!((rows[0].0, rows[8].0), (20.0, 20000.0));
        for (hz, outputs) in &rows {
            let h = one_pole(0.5, TAU * hz / sr);
            let (db, phase) = outputs[0];
            assert!(
                distance(db, phase, h) < 1e-12,
                "{sr} {hz}: {db} dB {phase} rad vs {} dB {} rad",
                h.db(),
                h.arg()
            );
        }
    }
}

/// A TPT one-pole is the bilinear transform of `1 / (1 + s / wc)` with the
/// cutoff prewarped: `H1 = 1 / (1 + j tan(w / 2) / g)`. At resonance 0 the
/// ladder is `H1^4`: 12.04 dB down and half a turn late at the cutoff. With
/// the feedback `k` solved for, `H1^4 / (1 + k H1^4)`.
#[test]
fn the_tpt_ladder_matches_its_closed_form_with_and_without_resonance() {
    let fixtures = Fixtures::new("ladder");
    let file = fixtures.write("ladder.dsp", LADDER);
    let g = (PI * 1000.0 / 44100.0).tan();
    for k in [0.0, 2.0] {
        let (ok, stdout, stderr) = probe(&[
            "--double",
            "-n",
            "8000",
            "--set",
            &format!("resonance={k}"),
            "--freqresp",
            "13:50:20000",
            &file,
        ]);
        assert!(ok, "{stderr}");
        for (hz, outputs) in &rows(&stdout) {
            let h1 = C(1.0, 0.0).div(C(1.0, (PI * hz / 44100.0).tan() / g));
            let h4 = h1.mul(h1).mul(h1).mul(h1);
            let h = h4.div(C(1.0 + k * h4.0, k * h4.1));
            let (db, phase) = outputs[0];
            // to 1e-12 of the unit gain of the passband, down to the last
            // point, 170 dB below
            assert!(
                distance(db, phase, h) < 1e-12,
                "k={k} {hz}: {db} dB {phase} rad vs {} dB {} rad",
                h.db(),
                h.arg()
            );
        }
    }
    // at the cutoff itself: four times -3.01 dB, four times -pi/4
    let (_, stdout, _) = probe(&["--double", "-n", "8000", "--freqresp", "1:1000:1000", &file]);
    let (db, phase) = rows(&stdout)[0].1[0];
    assert!((db - 20.0 * 0.25_f64.log10()).abs() < 1e-9, "{db}");
    assert!(same_angle(phase, -PI), "{phase}");
}

// ---------------------------------------------------------------- refusals

/// The error of a refused program, and nothing on stdout.
fn refused(file: &str) -> String {
    let (ok, stdout, stderr) = probe(&["--double", "--freqresp", "8", file]);
    assert!(!ok, "refused programs fail the command:\n{stdout}");
    assert!(stdout.is_empty(), "no response is printed: {stdout}");
    assert!(stderr.contains("not linear and time-invariant"), "{stderr}");
    stderr
}

#[test]
fn a_saturator_is_refused_for_homogeneity() {
    let fixtures = Fixtures::new("cubic");
    let error = refused(&fixtures.write("cubic.dsp", CUBIC));
    assert!(error.contains("homogeneity:"), "{error}");
    // 1 - 1/3 to the unit impulse; -0.5 + 0.125 / 3 to the other
    assert!(
        error.contains(
            "first: frame 0, out0: -0.4583333333333333 where -0.33333333333333337 was expected"
        ),
        "{error}"
    );
    // it says nothing to silence, and the error does not claim it does
    assert!(!error.contains("with no input at all"), "{error}");
    assert!(error.contains("--in sine:HZ"), "{error}");
}

/// `|x|` doubles when `x` doubles: only the sign of the factor shows it.
#[test]
fn a_rectifier_is_refused_because_the_factor_is_negative() {
    let fixtures = Fixtures::new("rectifier");
    let error = refused(&fixtures.write("rectifier.dsp", RECTIFIER));
    assert!(error.contains("homogeneity:"), "{error}");
    assert!(
        error.contains("first: frame 0, out0: 0.5 where -0.5 was expected"),
        "{error}"
    );
}

#[test]
fn a_time_varying_gain_is_refused_for_time_invariance() {
    let fixtures = Fixtures::new("tremolo");
    let error = refused(&fixtures.write("tremolo.dsp", TREMOLO));
    assert!(
        error.contains("time invariance: the response to an impulse at frame 37"),
        "{error}"
    );
    // the gain is 1 - 1/1000 at frame 0 and 1 - 38/1000 at frame 37
    assert!(
        error.contains("first: frame 37, out0: 0.962 where 0.999 was expected"),
        "{error}"
    );
    assert!(error.contains("--settle"), "{error}");
}

#[test]
fn a_median_filter_is_refused_for_superposition_alone() {
    let fixtures = Fixtures::new("median");
    let error = refused(&fixtures.write("median.dsp", MEDIAN));
    // its response to one impulse is zero at any level and any time: only
    // two impulses inside its window show that it is not additive
    assert!(error.contains("superposition:"), "{error}");
    assert!(
        error.contains("first: frame 1, out0: 0.5 where 0.0 was expected"),
        "{error}"
    );
}

#[test]
fn an_offset_is_refused_and_the_silent_render_says_why() {
    let fixtures = Fixtures::new("offset");
    let error = refused(&fixtures.write("offset.dsp", OFFSET));
    assert!(error.contains("homogeneity:"), "{error}");
    assert!(
        error.contains("with no input at all the program outputs a signal (out0 peaks at 0.25)"),
        "{error}"
    );
}

/// A reverberator injects 1e-20 against subnormals: an output with no input,
/// and not what makes a time-varying program time-varying.
#[test]
fn a_tiny_output_with_no_input_is_not_blamed_for_the_refusal() {
    let fixtures = Fixtures::new("tiny");
    let file = fixtures.write(
        "tiny.dsp",
        "n = +(1) ~ _;\nprocess = (+ ~ *(0.9)) * (1 - n / 1000) + 1e-20;\n",
    );
    let error = refused(&file);
    assert!(error.contains("time invariance:"), "{error}");
    assert!(!error.contains("with no input at all"), "{error}");
}

#[test]
fn the_tolerance_of_the_checks_is_the_one_given() {
    let fixtures = Fixtures::new("nearly");
    let file = fixtures.write("nearly.dsp", NEARLY_LINEAR);
    // 3.75e-7 off at frame 0, for a peak of 0.5: far above 1e-9
    let error = refused(&file);
    assert!(error.contains("tolerance 1e-9 of the peak"), "{error}");
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--freqresp",
        "2",
        "--linearity-tolerance",
        "1e-5",
        "--quiet",
        &file,
    ]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("linear and time-invariant within 1e-5 of the peak (homogeneity 7.4"),
        "{stdout}"
    );
    // single precision has its own default
    let (ok, stdout, stderr) = probe(&["--freqresp", "2", "--quiet", &file]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("within 1e-4 of the peak"), "{stdout}");
}

// ------------------------------------------------------------------ --settle

#[test]
fn a_smoothed_control_is_refused_until_it_has_settled() {
    let fixtures = Fixtures::new("smoothed");
    let file = fixtures.write("smoothed.dsp", SMOOTHED);
    let error = refused(&file);
    assert!(error.contains("time invariance:"), "{error}");
    assert!(
        error.contains(
            "a smoothed control (si.smoo) is such an envelope until it has settled: --settle N"
        ),
        "{error}"
    );

    // 0.999^50000 is 2e-22: the gain is 0.5, and the response the one-pole's
    // six decibels down
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--settle",
        "50000",
        "-n",
        "2000",
        "--freqresp",
        "5:100:10000",
        &file,
    ]);
    assert!(ok, "{stderr}");
    for (hz, outputs) in &rows(&stdout) {
        let h = one_pole(0.5, TAU * hz / 44100.0);
        assert!(
            (outputs[0].0 - (h.db() + 20.0 * 0.5_f64.log10())).abs() < 1e-9,
            "{hz}"
        );
        assert!((outputs[0].1 - h.arg()).abs() < 1e-9, "{hz}");
    }
    assert!(
        stderr.contains("to an impulse on the input at frame 50000"),
        "{stderr}"
    );
    assert!(stderr.contains("peak_at=50000"), "{stderr}");
}

// ---------------------------------------------------------------- the window

/// `h[n] = a^n` over `N` frames: the last tenth holds
/// `(a^(2 * 0.9 N) - a^(2N)) / (1 - a^(2N))` of the energy.
#[test]
fn a_response_cut_while_it_rings_is_said_to_be() {
    let fixtures = Fixtures::new("slow");
    let file = fixtures.write("slow.dsp", SLOW);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "-n",
        "1000",
        "--freqresp",
        "4",
        "--quiet",
        &file,
    ]);
    assert!(ok, "{stderr}");
    let a2: f64 = 0.999 * 0.999;
    let share = (a2.powi(900) - a2.powi(1000)) / (1.0 - a2.powi(1000));
    let line = stdout
        .lines()
        .find(|l| l.starts_with("# freqresp out0"))
        .expect("the window's line");
    let printed: f64 = line
        .split("holds ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("{line}"));
    assert!((printed - share).abs() < 1e-12, "{printed} vs {share}");
    assert!(
        stdout.contains("# note: out0 is still ringing 1000 frames after the impulse"),
        "{stdout}"
    );
    assert!(stdout.contains("raise -n"), "{stdout}");

    // 0.999^40000 is 4e-18: nothing is left, and nothing is said
    let (_, stdout, _) = probe(&[
        "--double",
        "-n",
        "40000",
        "--freqresp",
        "4",
        "--quiet",
        &file,
    ]);
    assert!(!stdout.contains("# note"), "{stdout}");
}

// ------------------------------------------------------- inputs and outputs

#[test]
fn an_impulse_on_one_input_gives_the_responses_from_that_input() {
    let fixtures = Fixtures::new("two_by_two");
    let file = fixtures.write("two.dsp", TWO_BY_TWO);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--in",
        "impulse:1",
        "--freqresp",
        "3:100:10000",
        &file,
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.starts_with("hz,mag_db_out0,phase_out0,mag_db_out1,phase_out1\n"));
    for (hz, outputs) in &rows(&stdout) {
        // nothing goes from input 1 to output 0
        assert!(outputs[0].0 == f64::NEG_INFINITY && outputs[0].1 == 0.0);
        let h = one_pole(0.5, TAU * hz / 44100.0);
        assert!((outputs[1].0 - h.db()).abs() < 1e-9, "{hz}");
    }
    assert!(stderr.contains("to an impulse on input 1"), "{stderr}");
    assert!(
        stderr.contains("# freqresp out0: the response is exactly zero: nothing reaches this output from input 1"),
        "{stderr}"
    );

    // the default excites every input, and says so
    let (ok, stdout, stderr) = probe(&["--double", "--freqresp", "3:100:10000", &file]);
    assert!(ok, "{stderr}");
    assert!((rows(&stdout)[0].1[0].0 - 20.0 * 0.5_f64.log10()).abs() < 1e-12);
    assert!(stderr.contains("on all 2 inputs at once"), "{stderr}");

    let (ok, _, stderr) = probe(&["--in", "impulse:2", "--freqresp", "3", &file]);
    assert!(!ok);
    assert!(stderr.contains("the program has 2 input(s)"), "{stderr}");
}

// ----------------------------------------------------------------- formats

#[test]
fn the_json_document_holds_the_same_response() {
    let fixtures = Fixtures::new("json");
    let file = fixtures.write("one_pole.dsp", ONE_POLE);
    let base = ["--double", "--freqresp", "6:50:15000"];
    let mut csv = base.to_vec();
    csv.push(&file);
    let (_, text, _) = probe(&csv);
    let mut args = base.to_vec();
    args.extend(["--format", "json", &file]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");
    assert_eq!(document["schema_version"], 1);
    let response = &document["freqresp"];
    assert_eq!(response["input"], serde_json::Value::Null);
    assert_eq!(response["settle"], 0);
    // one run, as a render's document has one: a sweep has one per point
    let runs = response["runs"].as_array().expect("runs");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run["set"], serde_json::json!({}));
    assert_eq!(run["linearity"]["shift"], 37);
    assert_eq!(run["linearity"]["homogeneity"], 0.0);
    assert_eq!(run["linearity"]["time_invariance"], 0.0);
    assert!(run["linearity"]["superposition"].as_f64().unwrap() < 1e-12);
    let output = &run["outputs"][0];
    assert_eq!(output["tail_energy_fraction"], 0.0);
    // the same numbers, to the last place a JSON reader is sure of
    let same = |a: f64, b: f64| (a - b).abs() <= 4.0 * f64::EPSILON * b.abs();
    for (k, (hz, outputs)) in rows(&text).iter().enumerate() {
        assert!(same(response["hz"][k].as_f64().unwrap(), *hz));
        assert!(same(output["mag_db"][k].as_f64().unwrap(), outputs[0].0));
        assert!(same(output["phase"][k].as_f64().unwrap(), outputs[0].1));
    }
}

#[test]
fn quiet_keeps_the_annotations_and_two_runs_print_the_same_bytes() {
    let fixtures = Fixtures::new("quiet");
    let file = fixtures.write("one_pole.dsp", ONE_POLE);
    let (ok, stdout, stderr) = probe(&["--double", "--freqresp", "4", "--quiet", &file]);
    assert!(ok, "{stderr}");
    assert!(stdout.lines().all(|l| l.starts_with('#')), "{stdout}");
    assert!(stdout.contains("# freqresp: 4 frequencies from 20.0 to 22050.0 Hz"));
    assert!(stdout.contains(
        "# freqresp: linear and time-invariant within 1e-9 of the peak (homogeneity 0.0, time_invariance 0.0, superposition"
    ));
    let first = probe(&["--double", "--freqresp", "32", &file]);
    assert_eq!(first, probe(&["--double", "--freqresp", "32", &file]));
}

// ------------------------------------------------------------------ --sweep

/// A family of curves is one command: the closed form of the ladder at each
/// cutoff and each resonance, the last axis varying fastest, the swept
/// controls heading the rows.
#[test]
fn a_sweep_gives_one_checked_response_per_point() {
    let fixtures = Fixtures::new("sweep");
    let file = fixtures.write("ladder.dsp", LADDER);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "-n",
        "8000",
        "--freqresp",
        "5:100:10000",
        "--sweep",
        "cutoff=500,2000",
        "--sweep",
        "resonance=0,2",
        &file,
    ]);
    assert!(ok, "{stderr}");
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some("cutoff,resonance,hz,mag_db_out0,phase_out0")
    );
    let rows: Vec<Vec<f64>> = lines
        .map(|l| l.split(',').map(|f| f.parse().unwrap()).collect())
        .collect();
    assert_eq!(rows.len(), 4 * 5);
    let expected_points = [(500.0, 0.0), (500.0, 2.0), (2000.0, 0.0), (2000.0, 2.0)];
    for (index, row) in rows.iter().enumerate() {
        let (fc, k) = expected_points[index / 5];
        assert_eq!((row[0], row[1]), (fc, k), "row {index}");
        let g = (PI * fc / 44100.0).tan();
        let h1 = C(1.0, 0.0).div(C(1.0, (PI * row[2] / 44100.0).tan() / g));
        let h4 = h1.mul(h1).mul(h1).mul(h1);
        let h = h4.div(C(1.0 + k * h4.0, k * h4.1));
        assert!(distance(row[3], row[4], h) < 1e-12, "row {index}: {row:?}");
    }
    // each point is checked, and says so under its own name
    assert!(stderr.contains("at each of 4 sweep points"), "{stderr}");
    for label in ["cutoff=500 resonance=0", "cutoff=2000 resonance=2"] {
        assert!(
            stderr.contains(&format!(
                "# freqresp [{label}]: linear and time-invariant within 1e-9"
            )),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!("# freqresp [{label}] out0: peak=")),
            "{stderr}"
        );
    }
}

/// `--set` fixes the other controls at every point.
#[test]
fn a_fixed_control_holds_at_every_point_of_the_sweep() {
    let fixtures = Fixtures::new("sweep_set");
    let file = fixtures.write("ladder.dsp", LADDER);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "-n",
        "8000",
        "--set",
        "resonance=2",
        "--freqresp",
        "1:1000:1000",
        "--sweep",
        "cutoff=1000,4000",
        &file,
    ]);
    assert!(ok, "{stderr}");
    // at its cutoff the ladder with k = 2 is H1^4 / (1 + 2 H1^4) = -0.5; at
    // the second point too the resonance is 2, not the slider's 0
    let rows: Vec<Vec<f64>> = stdout
        .lines()
        .skip(1)
        .map(|l| l.split(',').map(|f| f.parse().unwrap()).collect())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], 1000.0);
    assert!(
        (rows[0][2] - 20.0 * 0.5_f64.log10()).abs() < 1e-9,
        "{:?}",
        rows[0]
    );
    for row in &rows {
        let g = (PI * row[0] / 44100.0).tan();
        let h1 = C(1.0, 0.0).div(C(1.0, (PI * row[1] / 44100.0).tan() / g));
        let h4 = h1.mul(h1).mul(h1).mul(h1);
        let h = h4.div(C(1.0 + 2.0 * h4.0, 2.0 * h4.1));
        assert!(distance(row[2], row[3], h) < 1e-12, "{row:?}");
    }
}

/// `x + drive x^3` is linear at `drive = 0` and nowhere else: a family with a
/// member that is no frequency response is refused whole, with the point.
#[test]
fn a_point_that_is_not_linear_refuses_the_sweep_and_is_named() {
    let fixtures = Fixtures::new("sweep_nonlinear");
    let file = fixtures.write(
        "drive.dsp",
        "drive = hslider(\"drive\", 0, 0, 1, 0.01);\nprocess = _ <: _ + drive * (_ * _ * _);\n",
    );
    // alone, the linear point is measured
    let (ok, _, stderr) = probe(&["--double", "--freqresp", "4", "--sweep", "drive=0", &file]);
    assert!(ok, "{stderr}");
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--freqresp",
        "4",
        "--sweep",
        "drive=0,0.5",
        &file,
    ]);
    assert!(!ok);
    assert!(
        stdout.is_empty(),
        "not even the first point's rows: {stdout}"
    );
    assert!(
        stderr.contains("--freqresp: at `drive=0.5`, the program is not linear"),
        "{stderr}"
    );
    assert!(stderr.contains("homogeneity:"), "{stderr}");
}

#[test]
fn a_swept_value_outside_the_range_follows_the_rule_of_every_write() {
    let fixtures = Fixtures::new("sweep_range");
    let file = fixtures.write("ladder.dsp", LADDER);
    let base = [
        "--double",
        "-n",
        "8000",
        "--freqresp",
        "1:1000:1000",
        "--sweep",
        "resonance=0,7",
    ];
    let mut args = base.to_vec();
    args.push(&file);
    let (ok, stdout, stderr) = probe(&args);
    assert!(!ok);
    assert!(stdout.is_empty());
    assert!(stderr.contains("outside the range [0, 3.9]"), "{stderr}");

    // under --clamp the row carries the value that was used, and the clamp
    // is said once
    args.insert(0, "--clamp");
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let last = stdout.lines().last().unwrap();
    assert!(last.starts_with("3.9"), "{last}");
    assert_eq!(stderr.matches("# clamped").count(), 1, "{stderr}");
    assert!(stderr.contains("/resonance: 7 -> 3.9"), "{stderr}");
}

/// Each point starts from a cleared instance: a smoothed control settles at
/// every point, and a response that rings is noted for its point alone.
#[test]
fn every_point_settles_and_a_ringing_point_is_named() {
    let fixtures = Fixtures::new("sweep_settle");
    let smoothed = fixtures.write("smoothed.dsp", SMOOTHED);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--settle",
        "50000",
        "-n",
        "2000",
        "--freqresp",
        "1:1000:1000",
        "--sweep",
        "gain=0.25,0.5",
        &smoothed,
    ]);
    assert!(ok, "{stderr}");
    let levels: Vec<f64> = stdout
        .lines()
        .skip(1)
        .map(|l| l.split(',').nth(2).unwrap().parse().unwrap())
        .collect();
    // twice the gain, 6.02 dB
    assert!(
        (levels[1] - levels[0] - 20.0 * 2.0_f64.log10()).abs() < 1e-9,
        "{levels:?}"
    );

    let pole = fixtures.write(
        "pole.dsp",
        "process = + ~ *(hslider(\"a\", 0.5, 0, 0.9999, 0.0001));\n",
    );
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "-n",
        "1000",
        "--freqresp",
        "2",
        "--quiet",
        "--sweep",
        "a=0.999,0.5",
        &pole,
    ]);
    assert!(ok, "{stderr}");
    let notes: Vec<&str> = stdout.lines().filter(|l| l.starts_with("# note")).collect();
    assert_eq!(notes.len(), 1, "{stdout}");
    assert!(
        notes[0].starts_with("# note: [a=0.999] out0 is still ringing"),
        "{}",
        notes[0]
    );
}

#[test]
fn a_sweep_in_json_is_one_run_per_point_and_timed_as_one_account() {
    let fixtures = Fixtures::new("sweep_json");
    let file = fixtures.write("ladder.dsp", LADDER);
    let base = [
        "--double",
        "-n",
        "8000",
        "--freqresp",
        "3:500:2000",
        "--sweep",
        "cutoff=500,1000,2000",
        "--time",
    ];
    let mut args = base.to_vec();
    args.extend(["--format", "json", &file]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let runs = document["freqresp"]["runs"].as_array().expect("runs");
    assert_eq!(runs.len(), 3);
    assert_eq!(document["freqresp"]["hz"].as_array().unwrap().len(), 3);
    for (run, cutoff) in runs.iter().zip([500.0, 1000.0, 2000.0]) {
        assert_eq!(run["set"]["cutoff"], cutoff);
        // -12.04 dB where the frequency is the cutoff
        let at_cutoff = run["outputs"][0]["mag_db"]
            .as_array()
            .unwrap()
            .iter()
            .zip(document["freqresp"]["hz"].as_array().unwrap())
            .find(|(_, hz)| hz.as_f64() == Some(cutoff))
            .map(|(db, _)| db.as_f64().unwrap())
            .expect("the cutoff is on the grid");
        assert!(
            (at_cutoff - 20.0 * 0.25_f64.log10()).abs() < 1e-9,
            "{cutoff}"
        );
        // to the bit, short of the subnormal end of the tail, where halving
        // a sample loses its last bit
        assert!(run["linearity"]["homogeneity"].as_f64().unwrap() < 1e-300);
        assert_eq!(run["timing"]["frames"], 8000);
    }
    assert!(document["timing"]["compile_s"].as_f64().unwrap() > 0.0);

    // the text has one account for the three responses
    let mut args = base.to_vec();
    args.push(&file);
    let (ok, _, stderr) = probe(&args);
    assert!(ok);
    assert!(stderr.contains("# time: 3 renders"), "{stderr}");
    assert!(stderr.contains("(24000 frames in"), "{stderr}");
}

// ------------------------------------------------------- what it is not for

#[test]
fn what_has_no_impulse_response_to_transform_is_refused() {
    let fixtures = Fixtures::new("refusals");
    let one_pole = fixtures.write("one_pole.dsp", ONE_POLE);
    let generator = fixtures.write("generator.dsp", GENERATOR);
    let (ok, _, stderr) = probe(&["--freqresp", "8", &generator]);
    assert!(!ok);
    assert!(stderr.contains("the program has none"), "{stderr}");

    for (args, needle) in [
        (vec!["--in", "white:1"], "is not `impulse` or `impulse:CH`"),
        (
            vec!["--skip", "10"],
            "--skip cannot be combined with --freqresp",
        ),
        (vec!["--every", "2"], "--every cannot be combined"),
        (vec!["--reduce", "rms"], "--reduce cannot be combined"),
        (vec!["--format", "ir"], "--format ir cannot be combined"),
        (
            vec!["--check", "reset"],
            "--compare/--ref/--check cannot be combined",
        ),
        (vec!["-n", "3"], "at least 4 frames"),
        (vec!["--nvoices", "2"], "scalar Probe only"),
        (vec!["--protocol", "impulse-test"], "remove"),
    ] {
        let mut all = vec!["--freqresp", "8"];
        all.extend(args.iter().copied());
        all.push(&one_pole);
        let (ok, stdout, stderr) = probe(&all);
        assert!(!ok, "{args:?} must be refused");
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(stderr.contains(needle), "{args:?}: {stderr}");
    }
    for spec in ["0", "8:100", "8:0:100", "8:100:30000"] {
        let (ok, _, stderr) = probe(&["--freqresp", spec, &one_pole]);
        assert!(!ok, "{spec}");
        assert!(stderr.contains("--freqresp"), "{spec}: {stderr}");
    }
    // the flags that belong to it mean nothing without it
    for flag in [["--settle", "100"], ["--linearity-tolerance", "1e-6"]] {
        let (ok, _, stderr) = probe(&[flag[0], flag[1], &one_pole]);
        assert!(!ok, "{flag:?}");
        assert!(stderr.contains("--freqresp"), "{stderr}");
    }
}

// ------------------------------------------------------------ the libraries

const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";

fn faustlibraries() -> Option<String> {
    std::env::var("FAUST_RS_FAUSTLIBRARIES_ROOT")
        .ok()
        .or_else(|| {
            std::path::Path::new(DEFAULT_FAUSTLIBRARIES_ROOT)
                .exists()
                .then(|| DEFAULT_FAUSTLIBRARIES_ROOT.to_owned())
        })
}

/// What the phase is for: the response of a library function, with no file
/// to write. A Butterworth low-pass of order 4 is 3.01 dB down at its cutoff;
/// `ef.cubicnl` is the plan's saturator.
#[test]
fn a_library_filter_is_measured_and_a_library_saturator_refused() {
    let Some(root) = faustlibraries() else {
        eprintln!("Skipping: faustlibraries unavailable");
        return;
    };
    let library = format!("{root}/stdfaust.lib");
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "-I",
        &root,
        "--eval",
        "fi.lowpass(4, 1000)",
        "--freqresp",
        "1:1000:1000",
        &library,
    ]);
    assert!(ok, "{stderr}");
    let db = rows(&stdout)[0].1[0].0;
    assert!((db - 10.0 * 0.5_f64.log10()).abs() < 1e-6, "{db}");
    assert!(
        stderr.contains("# eval out0 = fi.lowpass(4, 1000)"),
        "{stderr}"
    );

    let (ok, stdout, stderr) = probe(&[
        "--double",
        "-I",
        &root,
        "--eval",
        "ef.cubicnl(0.5, 0)",
        "--freqresp",
        "8",
        &library,
    ]);
    assert!(!ok);
    assert!(stdout.is_empty());
    assert!(stderr.contains("homogeneity:"), "{stderr}");
}
