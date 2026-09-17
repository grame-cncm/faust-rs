//! What a render cost, and the subnormal samples it produced (phase F4 of
//! `porting/faustprobe-feedback-quality-analysis-and-plan-2026-09-17-en.md`).
//!
//! `fad` against `rad` was timed by hand, around the whole command. `--time`
//! times the `compute` calls alone, against real time and block by block. The
//! numbers differ from one run to the next, so nothing here asserts a
//! duration: the tests check that the default output has none, that the
//! account is consistent with itself (frames, blocks, budgets), and that it
//! measures the program: one a hundred times heavier takes longer.
//!
//! Subnormals are exact: `+ ~ *(0.5)` halves an impulse, and the halvings
//! that are subnormal at each width can be counted.

use std::path::PathBuf;
use std::process::Command;

/// An impulse halved at every sample: `2^-k` at frame `k`.
const HALVING: &str = "process = + ~ *(0.5);\n";
/// A gain, for a scheduled write.
const GAIN: &str = "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01);\n";
/// One recursive one-pole, and a hundred of them.
const ONE_POLE: &str = "process = _ : + ~ *(0.5);\n";
const MANY_POLES: &str = "process = _ <: par(i, 100, + ~ *(0.5 + i / 1000)) :> _;\n";
/// A descent, for the cost of its blocks.
const DESCENT: &str =
    "x = hslider(\"x\", 1, 0, 2, 0.001);\nprocess = (x - 3) * (x - 3), 2 * (x - 3);\n";

struct Fixtures(PathBuf);

impl Fixtures {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("faustprobe_cost_{}_{name}", std::process::id()));
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

fn json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not JSON ({e}):\n{stdout}"))
}

fn without_time(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with("# time"))
        .collect::<Vec<_>>()
        .join("\n")
}

// -------------------------------------------------------------------- --time

#[test]
fn the_default_output_has_no_time_and_is_the_same_twice() {
    let fixtures = Fixtures::new("default");
    let file = fixtures.write("halving.dsp", HALVING);
    let (_, first, first_err) = probe(&["-n", "500", &file]);
    let (_, second, second_err) = probe(&["-n", "500", &file]);
    assert_eq!(first, second);
    assert_eq!(first_err, second_err);
    assert!(!first.contains("# time") && !first_err.contains("# time"));

    // and `--time` adds its lines to the statistics without touching a line
    let (ok, timed, timed_err) = probe(&["-n", "500", "--time", &file]);
    assert!(ok);
    assert_eq!(timed, first, "the samples");
    assert_eq!(without_time(&timed_err), without_time(&first_err));
    let time: Vec<&str> = timed_err
        .lines()
        .filter(|l| l.starts_with("# time"))
        .collect();
    assert_eq!(time.len(), 3, "{timed_err}");
    assert!(time[0].starts_with("# time: compile "));
    // 500 frames in blocks of 64: seven and a last one of 52
    assert!(time[1].contains("(500 frames in 8 blocks)"), "{}", time[1]);
    assert!(time[2].starts_with("# time: worst block: frame "));
}

#[test]
fn the_account_is_consistent_with_itself() {
    let fixtures = Fixtures::new("account");
    let file = fixtures.write("halving.dsp", HALVING);
    let (ok, stdout, stderr) = probe(&["-n", "1000", "--time", "--format", "json", &file]);
    assert!(ok, "{stderr}");
    let document = json(&stdout);
    assert!(document["timing"]["compile_s"].as_f64().unwrap() > 0.0);
    let timing = &document["runs"][0]["timing"];
    let number = |key: &str| timing[key].as_f64().unwrap();
    // fifteen blocks of 64 and one of 40
    assert_eq!(timing["frames"], 1000);
    assert_eq!(timing["blocks"], 16);
    assert!((number("audio_s") - 1000.0 / 44100.0).abs() < 1e-15);
    assert!(number("compute_s") > 0.0);
    // how many times faster than real time: the audio over the time it took
    let factor = number("audio_s") / number("compute_s");
    assert!((number("realtime_factor") - factor).abs() <= 1e-9 * factor);
    // the worst block is one of the blocks, and sixteen blocks take longer
    // than any one of them
    let worst = &timing["worst_block"];
    let frames = worst["frames"].as_u64().unwrap();
    assert!(frames == 64 || frames == 40, "{worst}");
    assert_eq!(worst["frame"].as_u64().unwrap() % 64, 0);
    let seconds = worst["seconds"].as_f64().unwrap();
    assert!(seconds > 0.0 && seconds < number("compute_s"), "{timing}");
    let budget = worst["budget_s"].as_f64().unwrap();
    assert!((budget - frames as f64 / 44100.0).abs() < 1e-15);
    let fraction = worst["budget_fraction"].as_f64().unwrap();
    assert!((fraction - seconds / budget).abs() <= 1e-9 * fraction);
}

#[test]
fn a_block_is_timed_against_its_own_budget() {
    let fixtures = Fixtures::new("budget");
    let file = fixtures.write("gain.dsp", GAIN);
    // one block, and a short one: 36 frames leave 36 / 44100 s
    let (ok, stdout, stderr) = probe(&["-n", "36", "--time", "--format", "json", &file]);
    assert!(ok, "{stderr}");
    let worst = &json(&stdout)["runs"][0]["timing"]["worst_block"];
    assert_eq!(worst["frames"], 36);
    assert!((worst["budget_s"].as_f64().unwrap() - 36.0 / 44100.0).abs() < 1e-15);

    // a scheduled write cuts the block it falls in: 10 frames, then 54
    let (ok, stdout, stderr) = probe(&[
        "-n", "64", "--at", "10", "gain=0.2", "--time", "--format", "json", &file,
    ]);
    assert!(ok, "{stderr}");
    let timing = &json(&stdout)["runs"][0]["timing"];
    assert_eq!(timing["blocks"], 2);
    let worst = &timing["worst_block"];
    let (frame, frames) = (
        worst["frame"].as_u64().unwrap(),
        worst["frames"].as_u64().unwrap(),
    );
    assert!(
        (frame, frames) == (0, 10) || (frame, frames) == (10, 54),
        "{worst}"
    );
}

/// The one assertion about a duration, with a wide margin: a hundred
/// one-poles against one, over enough frames that the clock's grain and a
/// scheduler's hiccup are small against either.
#[test]
fn the_time_is_that_of_the_program() {
    let fixtures = Fixtures::new("heavier");
    let (one, many) = (
        fixtures.write("one.dsp", ONE_POLE),
        fixtures.write("many.dsp", MANY_POLES),
    );
    let compute = |file: &str| {
        let (ok, stdout, stderr) = probe(&[
            "-n", "200000", "--in", "white:1", "--quiet", "--time", "--format", "json", file,
        ]);
        assert!(ok, "{stderr}");
        json(&stdout)["runs"][0]["timing"]["compute_s"]
            .as_f64()
            .unwrap()
    };
    let (light, heavy) = (compute(&one), compute(&many));
    assert!(
        heavy > 5.0 * light,
        "100 one-poles took {heavy} s, one took {light} s"
    );
}

#[test]
fn a_sweep_and_an_ir_text_get_one_account_on_stderr() {
    let fixtures = Fixtures::new("sweep");
    let file = fixtures.write("gain.dsp", GAIN);
    let (ok, stdout, stderr) = probe(&[
        "-n",
        "1000",
        "--in",
        "white:1",
        "--sweep",
        "gain=0.1,0.5,0.9",
        "--reduce",
        "rms",
        "--time",
        &file,
    ]);
    assert!(ok, "{stderr}");
    // the rows are what they are without the flag
    assert_eq!(stdout.lines().count(), 4);
    assert!(!stdout.contains("# time"));
    assert!(stderr.contains("# time: 3 renders"), "{stderr}");
    assert!(stderr.contains("(3000 frames in 48 blocks)"), "{stderr}");

    // the `.ir` text is compared byte for byte: nothing is added to it
    let (_, plain, _) = probe(&["-n", "8", "--format", "ir", &file]);
    let (ok, timed, timed_err) = probe(&["-n", "8", "--format", "ir", "--time", &file]);
    assert!(ok);
    assert_eq!(timed, plain);
    assert!(timed_err.contains("(8 frames in 1 block)"), "{timed_err}");
}

#[test]
fn a_descent_is_timed_by_the_block() {
    let fixtures = Fixtures::new("descent");
    let file = fixtures.write("descent.dsp", DESCENT);
    let base = [
        "--double",
        "--in",
        "zero",
        "--block",
        "256",
        "--train",
        "x",
        "--optimizer",
        "sgd",
        "--blocks",
        "20",
        "--reset-per-block",
        "--time",
    ];
    let mut args = base.to_vec();
    args.push(&file);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("(5120 frames in 20 blocks)"), "{stdout}");
    // the worst block is named by its number, 1 to 20, even though every
    // block replays the excitation from frame 0
    let worst = stdout
        .lines()
        .find(|l| l.starts_with("# time: worst block: block "))
        .unwrap_or_else(|| panic!("{stdout}"));
    let number: usize = worst["# time: worst block: block ".len()..]
        .split(',')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=20).contains(&number), "{worst}");

    args.extend(["--format", "json"]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let timing = &json(&stdout)["train"]["timing"];
    assert_eq!(timing["blocks"], 20);
    assert_eq!(timing["frames"], 5120);
    assert!(timing["compile_s"].as_f64().unwrap() > 0.0);
    assert_eq!(timing["worst_block"]["frames"], 256);
}

/// A voice for the polyphonic wrapper: a gated sine, no library needed.
const VOICE: &str = "SR = fconstant(int fSamplingFreq, <math.h>);\n\
phasor(f) = (+(f / SR) : (_ <: _ - floor(_))) ~ _;\n\
freq = hslider(\"freq\", 440, 20, 20000, 0.01);\n\
gain = hslider(\"gain\", 0.8, 0, 1, 0.001);\n\
gate = button(\"gate\");\n\
process = sin(6.283185307179586 * phasor(freq)) * gain * gate;\n";

/// What a host's callback runs under `--nvoices`: the voices, their mix and
/// the effect, per block, and a note cuts the block it falls in.
#[test]
fn a_polyphonic_render_is_timed_too() {
    let fixtures = Fixtures::new("poly");
    let file = fixtures.write("voice.dsp", VOICE);
    let base = [
        "--nvoices",
        "4",
        "--note",
        "60@100",
        "-n",
        "256",
        "--quiet",
        "--time",
    ];
    let mut args = base.to_vec();
    args.push(&file);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    // 100 frames up to the note (64 + 36), then 156 (64 + 64 + 28)
    assert!(stdout.contains("(256 frames in 5 blocks)"), "{stdout}");
    args.extend(["--format", "json"]);
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "{stderr}");
    let timing = &json(&stdout)["timing"];
    assert_eq!(timing["blocks"], 5);
    assert!(timing["compile_s"].as_f64().unwrap() > 0.0);
    assert!(timing["compute_s"].as_f64().unwrap() > 0.0);
}

// ---------------------------------------------------------------- subnormals

/// `2^-k` is subnormal in single precision for `k` from 127 to 149, and zero
/// after; in double precision from 1023 to 1074.
#[test]
fn subnormal_outputs_are_counted_at_the_width_of_the_program() {
    let fixtures = Fixtures::new("subnormal");
    let file = fixtures.write("halving.dsp", HALVING);
    let (ok, stdout, stderr) = probe(&["-n", "200", "--quiet", &file]);
    assert!(ok, "{stderr}");
    let line = stdout.lines().find(|l| l.starts_with("# out0")).unwrap();
    assert!(
        line.ends_with(" peak_at=0 subnormal=23 subnormal_at=127"),
        "{line}"
    );

    // the same samples are normal numbers in double precision
    let (_, stdout, _) = probe(&["-n", "200", "--quiet", "--double", &file]);
    let line = stdout.lines().find(|l| l.starts_with("# out0")).unwrap();
    assert!(line.ends_with(" peak_at=0"), "{line}");
    let (_, stdout, _) = probe(&["-n", "1200", "--quiet", "--double", &file]);
    let line = stdout.lines().find(|l| l.starts_with("# out0")).unwrap();
    assert!(line.ends_with(" subnormal=52 subnormal_at=1023"), "{line}");
}

#[test]
fn subnormals_are_those_of_the_window_and_are_in_the_json() {
    let fixtures = Fixtures::new("subnormal_window");
    let file = fixtures.write("halving.dsp", HALVING);
    // frames 140 to 149 are left
    let (ok, stdout, stderr) = probe(&["-n", "200", "--skip", "140", "--quiet", &file]);
    assert!(ok, "{stderr}");
    let line = stdout.lines().find(|l| l.starts_with("# out0")).unwrap();
    assert!(line.ends_with(" subnormal=10 subnormal_at=140"), "{line}");

    let (_, stdout, _) = probe(&["-n", "200", "--format", "json", &file]);
    let channel = &json(&stdout)["runs"][0]["channels"][0];
    assert_eq!(channel["subnormal"], 23);
    assert_eq!(channel["subnormal_at"], 127);
    // the key is always there, a reader need not test for it
    let gain = fixtures.write("gain.dsp", GAIN);
    let (_, stdout, _) = probe(&["-n", "200", "--format", "json", &gain]);
    let channel = &json(&stdout)["runs"][0]["channels"][0];
    assert_eq!(channel["subnormal"], 0);
    assert_eq!(channel["subnormal_at"], serde_json::Value::Null);
}
