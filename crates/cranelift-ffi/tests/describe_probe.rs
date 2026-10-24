//! `--describe`, `--points`, and sweeps of a polyphonic instrument, through
//! the binary.
//!
//! The descriptors' formulas are checked on synthetic signals by the unit
//! tests of `probe::describe`; these check what only a render shows: the
//! values of programs whose output is known in closed form, the rows of a
//! sweep against the same renders made one by one, and the refusals.

mod common;
use common::{Fixtures, json, probe, probe_source};

/// A stereo sine at 1000 Hz, the right output at half the left's level.
const STEREO_SINE: &str = r#"
SR = fconstant(int fSamplingFreq, <math.h>);
n = (+(1) ~ _) - 1;
process = sin(6.283185307179586 * 1000 * n / SR) <: _, *(0.5);
"#;

/// A one-pole decay: its impulse response loses 20 log10(1/0.999) dB per
/// sample.
const DECAY: &str = "process = + ~ *(0.999);\n";

/// A gated, decaying sine voice with a brightness control the sweep moves.
const VOICE: &str = r#"
SR = fconstant(int fSamplingFreq, <math.h>);
fracp(x) = x - floor(x);
phasor(f) = (+(f / SR) : fracp) ~ _;
freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.8, 0, 1, 0.001);
gate = button("gate");
bright = hslider("bright", 0.5, 0, 1, 0.01);
env = (gate * 0.002) : (+ ~ *(0.998));
saw = 2 * phasor(freq) - 1;
process = saw * gain * env : + ~ *(bright * 0.9);
"#;

fn number(value: &serde_json::Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {value}"))
}

#[test]
fn a_stereo_sine_has_its_level_its_frequency_and_its_width() {
    let (ok, stdout, stderr) = probe_source(
        "stereo_sine",
        STEREO_SINE,
        &[
            "--double",
            "--sr",
            "32000",
            "-n",
            "32000",
            "--in",
            "zero",
            "--format",
            "json",
            "--describe",
            "loudness,centroid,width",
        ],
    );
    assert!(ok, "{stderr}");
    let doc = json(&stdout);
    let described = &doc["runs"][0]["describe"];
    // the mid is 0.75 sin: 10 log10(0.75² / 2)
    let loudness = number(&described["loudness"]);
    assert!(
        (loudness - 10.0 * (0.5625f64 / 2.0).log10()).abs() < 1e-6,
        "{loudness}"
    );
    // 1000 Hz is bin 64 of 2048 at 32 kHz: Hann leaks symmetrically
    let centroid = number(&described["centroid"]);
    assert!((centroid - 1000.0).abs() < 1e-6, "{centroid}");
    // side 0.5 sin, mid 1.5 sin
    let width = number(&described["width"]);
    assert!(
        (width - 10.0 * (0.25f64 / 2.25).log10()).abs() < 1e-9,
        "{width}"
    );
}

#[test]
fn a_one_pole_decay_takes_its_whole_frames() {
    // 441-sample frames at 44.1 kHz: the energy of each is q = 0.999^882
    // times the last one's (3.83 dB less), so -30 dB is first reached on the
    // eighth frame after the peak, at 0.08 s, and -60 dB on the sixteenth;
    // the frames 50 to 59 hold the mean of q^50 .. q^59 against the first
    let (ok, stdout, stderr) = probe_source(
        "decay",
        DECAY,
        &[
            "--double",
            "-n",
            "44100",
            "--format",
            "json",
            "--describe",
            "attack,decay,decay:60,sustain:0.5..0.6",
        ],
    );
    assert!(ok, "{stderr}");
    let doc = json(&stdout);
    let described = &doc["runs"][0]["describe"];
    assert_eq!(number(&described["attack"]), 0.0);
    assert_eq!(number(&described["decay:30"]), 0.08);
    assert_eq!(number(&described["decay:60"]), 0.16);
    let q = 0.999f64.powi(882);
    let mean = (50..60).map(|k| q.powi(k)).sum::<f64>() / 10.0;
    let sustain = number(&described["sustain:0.5..0.6"]);
    assert!((sustain - 10.0 * mean.log10()).abs() < 1e-6, "{sustain}");
}

#[test]
fn a_decay_not_reached_is_nan_and_said() {
    let (ok, stdout, stderr) = probe_source(
        "not_reached",
        STEREO_SINE,
        &[
            "--double",
            "-n",
            "8820",
            "--in",
            "zero",
            "--quiet",
            "--describe",
            "decay",
        ],
    );
    assert!(ok, "{stderr}");
    assert!(stdout.contains("decay:30=NaN"), "{stdout}");
    assert!(
        stdout.contains("# note: decay:30 is NaN: not reached before the window ends"),
        "{stdout}"
    );
    // and null in a document
    let (ok, stdout, stderr) = probe_source(
        "not_reached_json",
        STEREO_SINE,
        &[
            "--double",
            "-n",
            "8820",
            "--in",
            "zero",
            "--format",
            "json",
            "--describe",
            "decay",
        ],
    );
    assert!(ok, "{stderr}");
    assert!(json(&stdout)["runs"][0]["describe"]["decay:30"].is_null());
}

#[test]
fn points_render_what_the_same_sweep_renders() {
    let fixtures = Fixtures::new("points_vs_sweep");
    let dsp = fixtures.write("voice.dsp", VOICE);
    let points = fixtures.write("points.csv", "# two points\nbright\n0.2\n0.7\n");
    let common = [
        "--double", "-n", "4000", "--in", "zero", "--set", "gate=1", "--reduce", "rms",
    ];
    let mut by_sweep = common.to_vec();
    by_sweep.extend(["--sweep", "bright=0.2,0.7", &dsp]);
    let mut by_points = common.to_vec();
    by_points.extend(["--points", &points, &dsp]);
    let (ok, sweep, stderr) = probe(&by_sweep);
    assert!(ok, "{stderr}");
    let (ok, listed, stderr) = probe(&by_points);
    assert!(ok, "{stderr}");
    assert_eq!(sweep, listed);
    assert_eq!(sweep.lines().count(), 3, "{sweep}");
}

/// The RMS of each run of a polyphonic JSON document.
fn poly_rms(args: &[&str], dsp: &str) -> Vec<f64> {
    let mut all = vec![
        "--double",
        "--nvoices",
        "2",
        "-n",
        "3000",
        "--note",
        "60@0..1000",
        "--format",
        "json",
    ];
    all.extend(args);
    all.push(dsp);
    let (ok, stdout, stderr) = probe(&all);
    assert!(ok, "{stderr}");
    let doc = json(&stdout);
    match doc["runs"].as_array() {
        Some(runs) => runs
            .iter()
            .map(|run| number(&run["channels"][0]["rms"]))
            .collect(),
        None => vec![number(&doc["channels"][0]["rms"])],
    }
}

#[test]
fn a_polyphonic_sweep_renders_every_point_from_a_reset_instrument() {
    let fixtures = Fixtures::new("poly_sweep");
    let dsp = fixtures.write("voice.dsp", VOICE);
    // each point alone, from a fresh compilation
    let alone_low = poly_rms(&["--set", "bright=0.1"], &dsp);
    let alone_high = poly_rms(&["--set", "bright=0.9"], &dsp);
    // the same points in one sweep, in both orders: a voice left releasing,
    // or a state left in the feedback, would change the second point
    let up = poly_rms(&["--sweep", "bright=0.1,0.9"], &dsp);
    let down = poly_rms(&["--sweep", "bright=0.9,0.1"], &dsp);
    assert_eq!(up, [alone_low[0], alone_high[0]]);
    assert_eq!(down, [alone_high[0], alone_low[0]]);
    assert_ne!(alone_low, alone_high);
}

#[test]
fn a_polyphonic_sweep_prints_rows_with_its_descriptors() {
    let fixtures = Fixtures::new("poly_rows");
    let dsp = fixtures.write("voice.dsp", VOICE);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--nvoices",
        "2",
        "-n",
        "8820",
        "--note",
        "60@0..4410",
        "--sweep",
        "bright=0.1,0.9",
        "--describe",
        "loudness,attack,decay",
        &dsp,
    ]);
    assert!(ok, "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "bright,loudness,attack,decay:30", "{stdout}");
    assert_eq!(lines.len(), 3, "{stdout}");
    assert!(
        lines[1].starts_with("0.1,") && lines[2].starts_with("0.9,"),
        "{stdout}"
    );
}

#[test]
fn what_cannot_be_measured_is_refused_before_rendering() {
    let cases: [(&str, &[&str], &str); 7] = [
        (
            DECAY,
            &["--describe", "width"],
            "compares two outputs; this program has 1",
        ),
        (
            DECAY,
            &["-n", "1000", "--describe", "sustain:0.5..1"],
            "beyond the window",
        ),
        (
            DECAY,
            &["--describe", "brightness"],
            "unknown descriptor `brightness`",
        ),
        (
            DECAY,
            &["--describe", "loudness", "--describe-fft", "1000"],
            "power of two",
        ),
        (
            DECAY,
            &["--describe", "loudness", "--freqresp", "4"],
            "--describe cannot be combined with --freqresp",
        ),
        (
            DECAY,
            &["--describe", "loudness", "--protocol", "impulse-test"],
            "remove --describe",
        ),
        (
            DECAY,
            &["--describe", "loudness", "--train", "x", "--blocks", "0"],
            "--describe cannot be combined with --train",
        ),
    ];
    for (index, (source, args, expected)) in cases.iter().enumerate() {
        let (ok, stdout, stderr) = probe_source(&format!("refused_{index}"), source, args);
        assert!(!ok, "{args:?} accepted:\n{stdout}");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
}
