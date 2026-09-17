//! `--compare`, `--ref` and `--check` (phase F2 of
//! `porting/faustprobe-feedback-quality-analysis-and-plan-2026-09-17-en.md`).
//!
//! "Did this change a sample?" was answered in Python, by two renders, two
//! parses and the maximum of a difference. The probe answers it, and says
//! **where the renders part**, which the maximum does not. `--check` runs the
//! invariants the guide stated and nothing verified: the same samples at
//! another block size, after a reset, from a second compilation; and the
//! distance between the two widths.
//!
//! Expected frames and values follow from the fixtures' definitions.

use std::path::PathBuf;
use std::process::Command;

/// Half the input, two ways: the same samples.
const HALF: &str = "process = _ * 0.5;\n";
const HALVED: &str = "process = _ / 2;\n";

/// Half the input, then from frame 100 on (`n` counts 1, 2, ... so `n > 100`
/// first holds at frame 100) 0.50001 times the input.
const DEPARTS_AT_100: &str =
    "n = +(1) ~ _;\nprocess = _ <: select2(n > 100, _ * 0.5, _ * 0.50001);\n";

/// A gain, and the same result through two controls.
const GAIN: &str = "process = _ * hslider(\"gain\", 1, 0, 2, 0.001);\n";
const GAIN_AND_TRIM: &str =
    "process = _ * hslider(\"gain\", 1, 0, 2, 0.001) * hslider(\"trim\", 1, 0, 2, 0.001);\n";
/// The square of the gain: equal to [`GAIN`] while the gain is 1.
const GAIN_SQUARED: &str = "g = hslider(\"gain\", 1, 0, 2, 0.001);\nprocess = _ * g * g;\n";

/// Two outputs, the second departing like [`DEPARTS_AT_100`]; and its reference.
const STEREO_DEPARTS: &str =
    "n = +(1) ~ _;\nprocess = _ <: _ * 0.5, select2(n > 100, _ * 0.5, _ * 0.50001);\n";
const STEREO: &str = "process = _ <: _ * 0.5, _ * 0.5;\n";

/// State that a render leaves behind: a one-pole and a counter.
const STATEFUL: &str = "n = +(1) ~ _;\nprocess = (_ : + ~ *(0.9)) + n * 0.001;\n";

/// `0.1` accumulated: the two widths drift apart, by an amount the test replays.
const ACCUMULATOR: &str = "process = +(0.1) ~ _;\n";

struct Fixtures(PathBuf);

impl Fixtures {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("faustprobe_compare_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        Self(dir)
    }

    fn write(&self, file: &str, text: &str) -> String {
        let path = self.0.join(file);
        std::fs::write(&path, text).expect("write fixture");
        path.to_string_lossy().into_owned()
    }

    fn path(&self, file: &str) -> String {
        self.0.join(file).to_string_lossy().into_owned()
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

/// The `# TAG ...` lines of a `--quiet` run.
fn lines_of<'a>(stdout: &'a str, tag: &str) -> Vec<&'a str> {
    let prefix = format!("# {tag}");
    stdout.lines().filter(|l| l.starts_with(&prefix)).collect()
}

fn workspace(path: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
        .to_string_lossy()
        .into_owned()
}

// ---------------------------------------------------------------- --compare

#[test]
fn two_routes_to_the_same_samples_are_identical() {
    let fixtures = Fixtures::new("identical");
    let (half, halved) = (
        fixtures.write("a.dsp", HALF),
        fixtures.write("b.dsp", HALVED),
    );
    let (ok, stdout, stderr) = probe(&[
        "--in",
        "white:1",
        "-n",
        "512",
        "--quiet",
        "--compare",
        &halved,
        &half,
    ]);
    assert!(ok, "{stderr}");
    assert_eq!(
        lines_of(&stdout, "compare out0"),
        ["# compare out0: identical"]
    );
    assert!(stdout.contains("tolerance abs=0 rel=0"), "{stdout}");
}

#[test]
fn the_first_frame_that_differs_is_named_with_both_values() {
    let fixtures = Fixtures::new("departs");
    let (half, departs) = (
        fixtures.write("a.dsp", HALF),
        fixtures.write("b.dsp", DEPARTS_AT_100),
    );
    let args = [
        "--double",
        "--in",
        "dc",
        "-n",
        "512",
        "--quiet",
        "--compare",
        &departs,
        &half,
    ];
    let (ok, stdout, stderr) = probe(&args);
    assert!(!ok, "renders that differ agree under no tolerance");
    // the details are in the output, the verdict in the exit status and the error
    let line = lines_of(&stdout, "compare out0")[0];
    assert!(
        line.contains("first beyond tolerance: frame 100 (0.5 vs 0.50001)"),
        "{line}"
    );
    assert!(line.contains(" at frame 100, "), "{line}");
    assert!(stderr.contains("differs from"), "{stderr}");
    assert!(
        stderr.contains("first: frame 100, out0: 0.5 vs 0.50001"),
        "{stderr}"
    );

    // 1e-5 apart: inside an absolute tolerance of 1e-4, outside a relative
    // one of 1e-6 of the reference's peak
    let (ok, stdout, _) = probe(&[&args[..], &["--tolerance", "1e-4"]].concat());
    assert!(ok);
    assert!(
        lines_of(&stdout, "compare out0")[0].ends_with("within tolerance"),
        "{stdout}"
    );
    let (ok, _, _) = probe(&[&args[..], &["--rel-tolerance", "1e-6"]].concat());
    assert!(!ok);
    let (ok, _, _) = probe(&[&args[..], &["--rel-tolerance", "1e-4"]].concat());
    assert!(ok);
}

#[test]
fn every_output_is_compared_and_the_outputs_asked_for_only_when_asked() {
    let fixtures = Fixtures::new("stereo");
    let (stereo, departs) = (
        fixtures.write("a.dsp", STEREO),
        fixtures.write("b.dsp", STEREO_DEPARTS),
    );
    let args = [
        "--double",
        "--in",
        "dc",
        "-n",
        "256",
        "--quiet",
        "--compare",
        &departs,
        &stereo,
    ];
    let (ok, stdout, stderr) = probe(&args);
    // the difference is on the second output only
    assert!(!ok);
    assert_eq!(
        lines_of(&stdout, "compare out0"),
        ["# compare out0: identical"]
    );
    assert!(
        lines_of(&stdout, "compare out1")[0].contains("frame 100"),
        "{stdout}"
    );
    assert!(stderr.contains("first: frame 100, out1"), "{stderr}");

    let (ok, stdout, _) = probe(&[&args[..], &["--compare-outputs", "0"]].concat());
    assert!(ok);
    assert!(lines_of(&stdout, "compare out1").is_empty(), "{stdout}");
    let (ok, _, stderr) = probe(&[&args[..], &["--compare-outputs", "2"]].concat());
    assert!(!ok);
    assert!(stderr.contains("the program has 2 output(s)"), "{stderr}");
}

#[test]
fn set_addresses_both_programs_and_set_a_set_b_one() {
    let fixtures = Fixtures::new("sets");
    let (gain, trimmed) = (
        fixtures.write("a.dsp", GAIN),
        fixtures.write("b.dsp", GAIN_AND_TRIM),
    );
    let common = [
        "--in",
        "white:3",
        "-n",
        "128",
        "--quiet",
        "--compare",
        &trimmed,
    ];
    // 0.5 on one side, 0.25 * 2 on the other: the same samples
    let (ok, stdout, stderr) = probe(
        &[
            &common[..],
            &[
                "--set-a",
                "gain=0.5",
                "--set-b",
                "gain=0.25",
                "--set-b",
                "trim=2",
                &gain,
            ],
        ]
        .concat(),
    );
    assert!(ok, "{stderr}");
    assert_eq!(
        lines_of(&stdout, "compare out0"),
        ["# compare out0: identical"]
    );
    // `--set` goes to both: 0.5 against 0.5 * 1
    let (ok, _, stderr) = probe(&[&common[..], &["--set", "gain=0.5", &gain]].concat());
    assert!(ok, "{stderr}");
    // and must resolve in both
    let (ok, _, stderr) = probe(&[&common[..], &["--set", "trim=2", &gain]].concat());
    assert!(!ok);
    assert!(stderr.contains("no control matching `trim`"), "{stderr}");
    // what the second program is given is validated like the first's
    let (ok, stdout, stderr) = probe(&[&common[..], &["--set-b", "trim=9", &gain]].concat());
    assert!(!ok && stdout.is_empty());
    assert!(
        stderr.contains("--compare") && stderr.contains("outside the range [0, 2]"),
        "{stderr}"
    );
    // without --compare there is no second program to address
    let (ok, _, stderr) = probe(&["--set-b", "gain=1", "-n", "8", &gain]);
    assert!(!ok);
    assert!(
        stderr.contains("--set-a and --set-b address the two programs of --compare"),
        "{stderr}"
    );
}

#[test]
fn a_failed_comparison_names_the_scheduled_write_before_it() {
    let fixtures = Fixtures::new("schedule");
    let (gain, squared) = (
        fixtures.write("a.dsp", GAIN),
        fixtures.write("b.dsp", GAIN_SQUARED),
    );
    // equal while the gain is 1; at frame 50 it becomes 0.5: 0.5 against 0.25
    let (ok, stdout, stderr) = probe(&[
        "--in",
        "dc",
        "-n",
        "128",
        "--quiet",
        "--at",
        "50",
        "gain=0.5",
        "--compare",
        &squared,
        &gain,
    ]);
    assert!(!ok);
    assert!(
        lines_of(&stdout, "compare out0")[0].contains("frame 50 (0.5 vs 0.25)"),
        "{stdout}"
    );
    assert!(
        stderr.contains("last scheduled write before it: frame 50, /a/gain=0.5"),
        "{stderr}"
    );
}

#[test]
fn programs_that_cannot_be_compared_are_refused() {
    let fixtures = Fixtures::new("arity");
    let (half, stereo) = (
        fixtures.write("a.dsp", HALF),
        fixtures.write("b.dsp", STEREO),
    );
    let (ok, stdout, stderr) = probe(&["-n", "8", "--compare", &stereo, &half]);
    assert!(!ok && stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("has 1 output(s) and") && stderr.contains("2"),
        "{stderr}"
    );
    // a second program that does not compile says so, as the second program
    let broken = fixtures.write("broken.dsp", "process = _ : *(0.5 ;\n");
    let (ok, _, stderr) = probe(&["-n", "8", "--compare", &broken, &half]);
    assert!(!ok);
    assert!(
        stderr.contains("--compare:") && stderr.contains("FRS-PARSE-0001"),
        "{stderr}"
    );
}

// -------------------------------------------------------------------- --ref

#[test]
fn a_render_saved_by_out_is_the_reference_of_a_later_one() {
    let fixtures = Fixtures::new("ref");
    let stateful = fixtures.write("s.dsp", STATEFUL);
    for file in ["saved.npy", "saved.wav"] {
        let saved = fixtures.path(file);
        let window = ["--double", "--in", "white:5", "-n", "400", "--skip", "100"];
        let (ok, _, stderr) = probe(&[&window[..], &["--out", &saved, &stateful]].concat());
        assert!(ok, "{stderr}");
        let (ok, stdout, stderr) =
            probe(&[&window[..], &["--quiet", "--ref", &saved, &stateful]].concat());
        assert!(ok, "{file}: {stderr}");
        assert_eq!(
            lines_of(&stdout, "compare out0"),
            ["# compare out0: identical"]
        );

        // another excitation: different from the window's first frame on
        let (ok, stdout, _) = probe(&[
            "--double", "--in", "white:6", "-n", "400", "--skip", "100", "--quiet", "--ref",
            &saved, &stateful,
        ]);
        assert!(!ok);
        assert!(
            lines_of(&stdout, "compare out0")[0].contains("first beyond tolerance: frame 100 "),
            "{stdout}"
        );

        // another window is not the same render
        let (ok, _, stderr) = probe(&[
            "--double", "--in", "white:5", "-n", "400", "--quiet", "--ref", &saved, &stateful,
        ]);
        assert!(!ok);
        assert!(
            stderr.contains("holds 400 frames and the reference 300"),
            "{stderr}"
        );
    }
}

#[test]
fn a_saved_render_keeps_its_outputs_apart() {
    // Two different outputs of a noise input, written by --out and read back
    // by --ref: a file read with its frames and outputs exchanged would match
    // nowhere. The second output of the other program departs at frame 100.
    let fixtures = Fixtures::new("ref_stereo");
    let pair = fixtures.write("a.dsp", "process = _ <: _ * 0.5, _ * 0.25;\n");
    let departs = fixtures.write(
        "b.dsp",
        "n = +(1) ~ _;\nprocess = _ <: _ * 0.5, select2(n > 100, _ * 0.25, _ * 0.26);\n",
    );
    for file in ["pair.npy", "pair.wav"] {
        let saved = fixtures.path(file);
        let window = ["--double", "--in", "white:4", "-n", "256"];
        let (ok, _, stderr) = probe(&[&window[..], &["--out", &saved, &pair]].concat());
        assert!(ok, "{stderr}");
        // the program itself: the file holds its render
        let (ok, stdout, stderr) =
            probe(&[&window[..], &["--quiet", "--ref", &saved, &pair]].concat());
        assert!(ok, "{file}: {stderr}");
        assert_eq!(
            lines_of(&stdout, "compare out1"),
            ["# compare out1: identical"],
            "{file}"
        );
        // the other program: same first output, second one from frame 100
        let (ok, stdout, _) =
            probe(&[&window[..], &["--quiet", "--ref", &saved, &departs]].concat());
        assert!(!ok);
        assert_eq!(
            lines_of(&stdout, "compare out0"),
            ["# compare out0: identical"],
            "{file}"
        );
        assert!(
            lines_of(&stdout, "compare out1")[0].contains("first beyond tolerance: frame 100 "),
            "{file}: {stdout}"
        );
    }
}

// ------------------------------------------------------------------ --check

#[test]
fn the_invariants_hold_on_a_program_with_state() {
    let fixtures = Fixtures::new("invariants");
    let stateful = fixtures.write("s.dsp", STATEFUL);
    let (ok, stdout, stderr) = probe(&[
        "--double",
        "--in",
        "white:2",
        "-n",
        "1000",
        "--quiet",
        "--check",
        "block=1,7,333",
        "--check",
        "reset",
        "--check",
        "determinism",
        &stateful,
    ]);
    assert!(ok, "{stderr}");
    for tag in [
        "check block=1",
        "check block=7",
        "check block=333",
        "check reset",
        "check determinism",
    ] {
        assert_eq!(
            lines_of(&stdout, &format!("{tag} out0")),
            [format!("# {tag} out0: identical")],
            "{stdout}"
        );
    }
    assert!(
        stdout.contains("# check determinism: a second compilation gives the same program key")
    );
    // the render's own block size is what the others are compared with
    let (ok, stdout, _) = probe(&["-n", "64", "--quiet", "--check", "block=64,32", &stateful]);
    assert!(ok);
    assert!(lines_of(&stdout, "check block=64").is_empty(), "{stdout}");
    assert_eq!(lines_of(&stdout, "check block=32").len(), 1);
}

#[test]
fn a_block_dependent_output_fails_the_block_check_and_a_sound_one_passes() {
    // The gradient lanes of a `rad` program are defined per block (the block
    // reverse sweep); its loss lane is not. A genuine positive, from the corpus.
    let program = workspace("tests/corpus/ddsp_rad_host_block_resonator.dsp");
    let libraries = workspace("libraries");
    let args = [
        "--double",
        "-I",
        &libraries,
        "--in",
        "white:1",
        "-n",
        "512",
        "--quiet",
        "--check",
        "block=256",
        &program,
    ];
    let (ok, stdout, stderr) = probe(&args);
    assert!(!ok, "the gradient lanes move with the block size");
    assert_eq!(
        lines_of(&stdout, "check block=256 out0"),
        ["# check block=256 out0: identical"]
    );
    for lane in ["out1", "out2"] {
        let line = lines_of(&stdout, &format!("check block=256 {lane}"))[0];
        assert!(line.contains("first beyond tolerance: frame "), "{line}");
    }
    assert!(stderr.contains("--check block=256 failed"), "{stderr}");
    // the lane that is no gradient
    let (ok, _, stderr) = probe(&[&args[..], &["--compare-outputs", "0"]].concat());
    assert!(ok, "{stderr}");
}

#[test]
fn the_width_check_measures_what_the_two_accumulations_give() {
    let fixtures = Fixtures::new("width");
    let accumulator = fixtures.write("acc.dsp", ACCUMULATOR);
    let frames = 2000;
    // replayed here: y[n] = y[n-1] + 0.1 in each width, 0.1 at frame 0
    let (mut narrow, mut wide, mut largest) = (0.0_f32, 0.0_f64, 0.0_f64);
    for _ in 0..frames {
        narrow += 0.1_f32;
        wide += 0.1_f64;
        largest = largest.max((f64::from(narrow) - wide).abs());
    }
    let args = [
        "--double",
        "--in",
        "zero",
        "-n",
        "2000",
        "--quiet",
        "--check",
        "width",
        &accumulator,
    ];
    let (ok, stdout, stderr) = probe(&args);
    assert!(ok, "a report, without a tolerance: {stderr}");
    assert!(
        stdout.contains("(a report: no tolerance given)"),
        "{stdout}"
    );
    let line = lines_of(&stdout, "check width out0")[0];
    let printed: f64 = line
        .split("max_abs=")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("no max_abs in {line}"));
    assert_eq!(printed.to_bits(), largest.to_bits(), "{line}");
    assert!(
        largest > 1e-4,
        "the fixture must drift measurably: {largest}"
    );

    // with a tolerance it is a gate
    let (ok, _, stderr) = probe(&[&args[..], &["--tolerance", "1e-9"]].concat());
    assert!(!ok);
    assert!(stderr.contains("--check width failed"), "{stderr}");
    let (ok, _, _) = probe(&[&args[..], &["--tolerance", "1"]].concat());
    assert!(ok);
}

// --------------------------------------------------------------------- JSON

#[test]
fn json_carries_the_comparison_even_when_it_fails() {
    let fixtures = Fixtures::new("json");
    let (half, departs) = (
        fixtures.write("a.dsp", HALF),
        fixtures.write("b.dsp", DEPARTS_AT_100),
    );
    let (ok, stdout, _) = probe(&[
        "--double",
        "--in",
        "dc",
        "-n",
        "256",
        "--format",
        "json",
        "--check",
        "reset",
        "--compare",
        &departs,
        &half,
    ]);
    assert!(!ok);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");
    let run = &document["runs"][0];
    assert_eq!(run["compare"]["agrees"], false);
    assert_eq!(run["compare"]["channels"][0]["first_beyond"]["frame"], 100);
    assert_eq!(
        run["compare"]["channels"][0]["first_beyond"]["reference"],
        0.50001
    );
    assert_eq!(run["checks"][0]["check"], "reset");
    assert_eq!(run["checks"][0]["agrees"], true);
    assert_eq!(run["checks"][0]["channels"][0]["identical"], true);
}

// ----------------------------------------------------------------- refusals

#[test]
fn what_looks_at_one_render_refuses_what_makes_several_or_none() {
    let fixtures = Fixtures::new("refusals");
    let (half, halved) = (
        fixtures.write("a.dsp", HALF),
        fixtures.write("b.dsp", HALVED),
    );
    let gain = fixtures.write("g.dsp", GAIN);
    for (args, expected) in [
        (
            vec![
                "--compare",
                halved.as_str(),
                "--sweep",
                "gain=0,1",
                gain.as_str(),
            ],
            "--sweep cannot be combined",
        ),
        (
            vec![
                "--check",
                "reset",
                "--protocol",
                "impulse-test",
                half.as_str(),
            ],
            "remove --compare/--ref/--check",
        ),
        (
            vec!["--check", "reset", "--nvoices", "2", half.as_str()],
            "scalar Probe only",
        ),
        (
            vec!["--check", "reset", "--train", "gain", gain.as_str()],
            "cannot be combined with --train",
        ),
        (
            vec![
                "--compare",
                halved.as_str(),
                "--ref",
                "x.npy",
                half.as_str(),
            ],
            "give one",
        ),
        (
            vec!["--check", "blocks", half.as_str()],
            "unknown check `blocks`",
        ),
        (
            vec!["--check", "block=0", half.as_str()],
            "is not a block size",
        ),
        (
            vec![
                "--compare",
                halved.as_str(),
                "--tolerance=-1",
                half.as_str(),
            ],
            "non-negative",
        ),
    ] {
        let (ok, stdout, stderr) = probe(&args);
        assert!(!ok, "{args:?} passed");
        assert!(stdout.is_empty(), "{args:?} rendered:\n{stdout}");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
}
