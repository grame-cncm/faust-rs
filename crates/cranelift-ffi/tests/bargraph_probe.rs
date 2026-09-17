//! Bargraphs on the scalar probe: read back, never written, named in listings.
//!
//! A bargraph shares the controls' address space, so it resolves like one,
//! but it is an output: the program writes its zone on every sample and a
//! host write is overwritten by the next `compute`. Three properties are
//! held here, at the library level and through the `faustprobe` binary:
//!
//! * its value can be read, and it is what the program wrote at the last
//!   sample of the last block (so its time resolution is the block);
//! * writing to it is an error, by `--set`, `--sweep` or `--at`, instead of a
//!   render that looks like a measurement and is not;
//! * a listing says what it is.

use cranelift_ffi::probe::engine::{Factory, Probe, RenderSpec};
use cranelift_ffi::probe::params::ControlKind;
use cranelift_ffi::probe::render::InputMode;
use cranelift_ffi::probe::schedule::{Event, Schedule};
use std::process::Command;
use std::rc::Rc;

/// A gain on a slider, and a bargraph showing twice the slider: the
/// bargraph's value is known from the control alone, whatever the input.
const DSP: &str = r#"
g = hslider("g", 0.25, 0, 1, 0.001);
process = _ * g : attach(_, g * 2 : hbargraph("twice", 0, 2));
"#;

fn probe(double: bool) -> Probe {
    let factory =
        Factory::compile_from_string("bargraph_test", DSP, &[], double, 0).expect("compile");
    Probe::instantiate(&Rc::new(factory), 48_000).expect("instantiate")
}

fn spec(frames: usize, block: usize, schedule: Schedule) -> RenderSpec {
    RenderSpec {
        frames,
        block,
        input: InputMode::Dc,
        skip: 0,
        schedule,
        drive_buttons: false,
    }
}

#[test]
fn a_bargraph_reads_back_what_the_program_wrote() {
    for double in [false, true] {
        let probe = probe(double);
        probe.set("g", 0.4).expect("set g");
        probe.render(&spec(256, 64, Schedule::new()), |_, _| {});
        let shown = probe.bargraphs();
        assert_eq!(shown.len(), 1, "one bargraph: {shown:?}");
        assert!(shown[0].0.ends_with("/twice"), "its path: {}", shown[0].0);
        assert!(
            (shown[0].1 - 0.8).abs() < 1e-6,
            "twice 0.4, double={double}: {}",
            shown[0].1
        );
    }
}

#[test]
fn a_bargraph_holds_the_value_of_the_last_block() {
    // g steps from 0.25 to 0.5 at frame 100: read after every block, the
    // bargraph shows 0.5 before the step's block ends and 1.0 from then on.
    let probe = probe(true);
    let mut schedule = Schedule::new();
    schedule.push(
        100,
        Event::SetParam {
            path: "g".to_owned(),
            value: 0.5,
        },
    );
    let mut seen: Vec<(usize, f64)> = Vec::new();
    probe.render(&spec(200, 64, schedule), |frame, _| {
        seen.push((frame, probe.bargraphs()[0].1));
    });
    let at = |frame: usize| seen.iter().find(|(f, _)| *f == frame).expect("frame").1;
    assert!((at(0) - 0.5).abs() < 1e-9, "first block: {}", at(0));
    assert!((at(99) - 0.5).abs() < 1e-9, "before the step: {}", at(99));
    assert!(
        (at(100) - 1.0).abs() < 1e-9,
        "the step's block: {}",
        at(100)
    );
    assert!((at(199) - 1.0).abs() < 1e-9, "the end: {}", at(199));
}

#[test]
fn writing_a_bargraph_is_an_error() {
    let probe = probe(true);
    let err = probe
        .set("twice", 1.0)
        .expect_err("a bargraph is not settable");
    assert!(err.contains("bargraph"), "{err}");
    let err = probe.check_writable("twice").expect_err("nor writable");
    assert!(err.contains("/twice") && err.contains("bargraph"), "{err}");
    probe.check_writable("g").expect("a slider is");
    let err = probe.check_writable("nothing").expect_err("unknown");
    assert!(err.contains("no control matching"), "{err}");
    // the failed write left the program's own value in place
    probe.render(&spec(64, 64, Schedule::new()), |_, _| {});
    assert!((probe.bargraphs()[0].1 - 0.5).abs() < 1e-9);
}

#[test]
fn the_kinds_have_names() {
    let probe = probe(true);
    let kinds: Vec<(String, ControlKind)> = probe
        .controls()
        .iter()
        .map(|c| (c.path.clone(), c.kind))
        .collect();
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert!(
        kinds
            .iter()
            .any(|(p, k)| p.ends_with("/g") && k.label() == "slider")
    );
    assert!(
        kinds
            .iter()
            .any(|(p, k)| p.ends_with("/twice") && k.label() == "bargraph")
    );
}

// ---- the same through the binary

/// The test program on disk, and `faustprobe` on it with `args`.
fn faustprobe(tag: &str, args: &[&str]) -> (bool, String, String) {
    let path = std::env::temp_dir().join(format!(
        "faustprobe_bargraph_{tag}_{}.dsp",
        std::process::id()
    ));
    std::fs::write(&path, DSP).expect("write dsp");
    let out = Command::new(env!("CARGO_BIN_EXE_faustprobe"))
        .args(args)
        .arg(&path)
        .output()
        .expect("run faustprobe");
    let _ = std::fs::remove_file(&path);
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn list_params_names_the_kind() {
    let (ok, stdout, stderr) = faustprobe("list", &["--list-params"]);
    assert!(ok, "{stderr}");
    let header = stdout.lines().next().expect("a header");
    assert!(header.contains("kind"), "{header}");
    let line = |suffix: &str| {
        stdout
            .lines()
            .find(|l| {
                l.split_whitespace()
                    .next()
                    .is_some_and(|p| p.ends_with(suffix))
            })
            .unwrap_or_else(|| panic!("no line for {suffix}: {stdout}"))
            .to_owned()
    };
    assert!(line("/g").contains("slider"), "{}", line("/g"));
    assert!(line("/twice").contains("bargraph"), "{}", line("/twice"));
}

#[test]
fn set_sweep_and_at_refuse_a_bargraph() {
    for (tag, args) in [
        ("set", vec!["--quiet", "--set", "twice=1"]),
        (
            "sweep",
            vec!["--quiet", "--sweep", "twice=0,1", "--reduce", "rms"],
        ),
        ("at", vec!["--quiet", "--at", "10", "twice=1"]),
    ] {
        let (ok, stdout, stderr) = faustprobe(tag, &args);
        assert!(!ok, "{tag} should fail, printed: {stdout}");
        assert!(stderr.contains("bargraph"), "{tag}: {stderr}");
        assert!(stdout.trim().is_empty(), "{tag} rendered anyway: {stdout}");
    }
    // and an unknown path in --at, which the render loop used to ignore
    let (ok, _, stderr) = faustprobe("at_unknown", &["--quiet", "--at", "10", "nothing=1"]);
    assert!(!ok && stderr.contains("no control matching"), "{stderr}");
}

#[test]
fn the_statistics_and_the_json_report_the_bargraphs() {
    let (ok, stdout, stderr) = faustprobe("stats", &["--quiet", "--in", "dc", "--set", "g=0.4"]);
    assert!(ok, "{stderr}");
    let line = stdout
        .lines()
        .find(|l| l.starts_with("# bargraph "))
        .unwrap_or_else(|| panic!("no bargraph line: {stdout}"));
    let (path, value) = line["# bargraph ".len()..]
        .split_once('=')
        .expect("PATH=VALUE");
    assert!(path.ends_with("/twice"), "{line}");
    // single precision by default
    assert!(
        (value.parse::<f64>().expect("a number") - 0.8).abs() < 1e-6,
        "{line}"
    );

    let (ok, stdout, stderr) = faustprobe(
        "json",
        &[
            "--double", "--format", "json", "--in", "dc", "--set", "g=0.4",
        ],
    );
    assert!(ok, "{stderr}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let shown = doc["runs"][0]["bargraphs"].as_object().expect("bargraphs");
    let (path, value) = shown.iter().next().expect("one bargraph");
    assert!(path.ends_with("/twice"), "{path}");
    assert!(
        (value.as_f64().expect("a number") - 0.8).abs() < 1e-12,
        "{value}"
    );
}

#[test]
fn the_bargraphs_flag_adds_columns() {
    // per-frame dump: a column per bargraph, block-rate
    let (ok, stdout, stderr) = faustprobe(
        "csv",
        &[
            "--double",
            "--in",
            "dc",
            "-n",
            "200",
            "--block",
            "64",
            "--every",
            "50",
            "--bargraphs",
            "--at",
            "100",
            "g=0.5",
        ],
    );
    assert!(ok, "{stderr}");
    let mut lines = stdout.lines();
    let header: Vec<&str> = lines.next().expect("header").split(',').collect();
    assert_eq!(header.len(), 3, "{header:?}");
    assert!(header[2].ends_with("/twice"), "{header:?}");
    let rows: Vec<Vec<f64>> = lines
        .map(|l| l.split(',').map(|v| v.parse().expect("a number")).collect())
        .collect();
    let at = |frame: f64| rows.iter().find(|r| r[0] == frame).expect("row")[2];
    assert!(
        (at(50.0) - 0.5).abs() < 1e-9
            && (at(100.0) - 1.0).abs() < 1e-9
            && (at(150.0) - 1.0).abs() < 1e-9
    );

    // a sweep: one column per bargraph, the value at the end of each render
    let (ok, stdout, stderr) = faustprobe(
        "sweepcol",
        &[
            "--double",
            "--in",
            "dc",
            "--sweep",
            "g=0.1,0.3",
            "--reduce",
            "rms",
            "--bargraphs",
        ],
    );
    assert!(ok, "{stderr}");
    let rows: Vec<Vec<&str>> = stdout.lines().map(|l| l.split(',').collect()).collect();
    assert!(rows[0][2].ends_with("/twice"), "{:?}", rows[0]);
    assert!(
        (rows[1][2].parse::<f64>().unwrap() - 0.2).abs() < 1e-9,
        "{:?}",
        rows[1]
    );
    assert!(
        (rows[2][2].parse::<f64>().unwrap() - 0.6).abs() < 1e-9,
        "{:?}",
        rows[2]
    );

    // the flag does not combine with what cannot carry it
    for (tag, args) in [
        ("ir", vec!["--bargraphs", "--format", "ir"]),
        (
            "train",
            vec!["--bargraphs", "--train", "g", "--blocks", "1"],
        ),
    ] {
        let (ok, _, stderr) = faustprobe(tag, &args);
        assert!(!ok && stderr.contains("--bargraphs"), "{tag}: {stderr}");
    }
}
