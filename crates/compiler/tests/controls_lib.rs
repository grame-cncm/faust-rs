//! Runtime checks for the project-local `libraries/controls.lib`.
//!
//! The library works on the control inputs of a whole program with the
//! faust-rs primitives `cinputs`/`cinput` and the wildcard modulation target
//! `"*"`: reading them, rebinding them, rebuilding the interface, exploring
//! settings, sweeping them for a test, and differentiating with respect to
//! all of them. It imports nothing, so its fixtures compile with
//! `tests/corpus` and `libraries` alone and these tests never skip.
//!
//! The programs are the fixtures `tests/corpus/ctl_*.dsp`, each stating its
//! expected outputs in its header; `ctl_all_functions.dsp` instantiates the
//! `#### Test` entry of every documented function, so the documentation
//! examples are compiled too. They run through the interpreter fast lane.
//! There is no C++ oracle (the reference has none of the primitives): the
//! expected values are derived by hand in each header.

use std::io::Cursor;
use std::path::PathBuf;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};
use serde_json::Value;

const SAMPLE_RATE: i32 = 48_000;

fn workspace_dir(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(rel)
}

fn fixture(stem: &str) -> PathBuf {
    workspace_dir("tests/corpus").join(format!("{stem}.dsp"))
}

fn search_paths() -> [PathBuf; 2] {
    [workspace_dir("tests/corpus"), workspace_dir("libraries")]
}

/// Runs fixture `stem` for `frames` samples with constant inputs `inputs`.
fn run(stem: &str, inputs: &[f32], frames: usize) -> Vec<Vec<f32>> {
    let path = fixture(stem);
    let fbc = Compiler::new()
        .compile_file_to_interp(&path, &search_paths(), &InterpOptions::default())
        .unwrap_or_else(|e| panic!("{stem}: compilation failed: {e}"));
    let mut factory = read_fbc::<f32>(&mut Cursor::new(fbc)).expect("parse the bytecode");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(SAMPLE_RATE);
    assert_eq!(
        usize::try_from(instance.get_num_inputs()).expect("non-negative"),
        inputs.len(),
        "input count of {stem}"
    );
    let input_buffers: Vec<Vec<f32>> = inputs.iter().map(|&v| vec![v; frames]).collect();
    let input_slices: Vec<&[f32]> = input_buffers.iter().map(Vec::as_slice).collect();
    let outputs = usize::try_from(instance.get_num_outputs()).expect("non-negative");
    let mut buffers = vec![vec![0.0_f32; frames]; outputs];
    let mut slices: Vec<&mut [f32]> = buffers.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(
            i32::try_from(frames).expect("frames"),
            &input_slices,
            &mut slices,
        )
        .unwrap_or_else(|e| panic!("{stem}: run failed: {e}"));
    buffers
}

fn first_samples(stem: &str, inputs: &[f32]) -> Vec<f32> {
    run(stem, inputs, 1).iter().map(|out| out[0]).collect()
}

fn assert_close(got: &[f32], expected: &[f32], what: &str) {
    assert_eq!(got.len(), expected.len(), "{what}: {got:?}");
    for (g, e) in got.iter().zip(expected) {
        assert!((g - e).abs() <= 1e-5 * e.abs().max(1.0), "{what}: {got:?}");
    }
}

/// One input widget of an interface.
#[derive(Debug)]
struct Widget {
    label: String,
    kind: String,
    init: f64,
    min: f64,
    max: f64,
    step: f64,
    style: Option<String>,
}

/// The input widgets of fixture `stem`'s interface, groups flattened.
fn interface(stem: &str) -> Vec<Widget> {
    fn walk(items: &[Value], out: &mut Vec<Widget>) {
        for item in items {
            let kind = item["type"].as_str().unwrap_or_default();
            if let Some(children) = item["items"].as_array() {
                walk(children, out);
            } else if matches!(kind, "hslider" | "vslider" | "nentry") {
                let style = item["meta"].as_array().and_then(|meta| {
                    meta.iter()
                        .find_map(|m| m["style"].as_str().map(str::to_owned))
                });
                let number = |key: &str| item[key].as_f64().expect("a number");
                out.push(Widget {
                    label: item["label"].as_str().expect("label").to_owned(),
                    kind: kind.to_owned(),
                    init: number("init"),
                    min: number("min"),
                    max: number("max"),
                    step: number("step"),
                    style,
                });
            }
        }
    }
    let json = Compiler::new()
        .compile_file_to_json(
            &fixture(stem),
            &search_paths(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{stem}: json failed: {e}"));
    let value: Value = serde_json::from_str(&json).expect("valid json");
    let mut out = Vec::new();
    walk(value["ui"].as_array().expect("ui"), &mut out);
    out
}

#[test]
fn the_readers_give_the_count_the_widget_and_its_parameters() {
    assert_eq!(
        first_samples("ctl_01_reading", &[]),
        vec![2.0, 0.25, 0.5, 0.0, 1.0, 0.1, 0.5, 0.25]
    );
}

#[test]
fn map_external_normalized_and_cv_rebind_every_control() {
    assert_close(
        &first_samples("ctl_02_rebinding", &[0.3, 0.4, 0.2, 0.5, 0.1, -0.1]),
        &[5.5, 3.4, 2.5, 6.15],
        "ctl_02_rebinding",
    );
}

#[test]
fn smooth_starts_at_the_defaults_and_smoother_is_a_one_pole() {
    let outs = run("ctl_03_smoothing", &[], 64);
    assert!(outs[0].iter().all(|&v| v == 5.25), "{:?}", outs[0]);
    let pole = (-1.0_f64 / 48.0).exp();
    for (n, &v) in outs[1].iter().enumerate() {
        let expected = 1.0 - pole.powi(i32::try_from(n).expect("frame") + 1);
        assert!((f64::from(v) - expected).abs() < 1e-5, "frame {n}: {v}");
    }
}

#[test]
fn knobs_and_relabel_rebuild_the_interface_with_the_same_parameters() {
    assert_eq!(first_samples("ctl_04_interface", &[]), vec![5.25, 5.25]);
    let widgets = interface("ctl_04_interface");
    let expected = [
        ("P0", "hslider", 0.5, Some("knob")),
        ("P1", "hslider", 0.25, Some("knob")),
        ("p0", "nentry", 0.5, None),
        ("p1", "nentry", 0.25, None),
    ];
    assert_eq!(widgets.len(), expected.len(), "{widgets:?}");
    for (widget, (label, kind, init, style)) in widgets.iter().zip(expected) {
        assert_eq!((widget.label.as_str(), widget.kind.as_str()), (label, kind));
        assert!((widget.init - init).abs() < 1e-6, "{widgets:?}");
        assert_eq!((widget.min, widget.max), (0.0, 1.0), "{widgets:?}");
        assert!((widget.step - 0.1).abs() < 1e-6, "{widgets:?}");
        assert_eq!(widget.style.as_deref(), style, "{widgets:?}");
    }
}

#[test]
fn morph_offset_and_randomize_explore_around_the_knobs() {
    let frames = 256;
    let outs = run("ctl_05_exploring", &[], frames);
    assert_close(&[outs[0][0], outs[1][0]], &[7.625, 6.35], "morph, offset");
    let (x, n, c) = (&outs[2], &outs[3], &outs[4]);
    // the defaults until the first draw, at sample 3
    for frame in 0..3 {
        assert_eq!((x[frame], n[frame], c[frame]), (0.5, 3.0, 0.0));
    }
    let mut draws_of_x = Vec::new();
    for frame in 0..frames {
        // in each range, on each grid
        assert!((0.0..=1.0).contains(&x[frame]), "x = {}", x[frame]);
        assert!((x[frame] * 10.0 - (x[frame] * 10.0).round()).abs() < 1e-4);
        assert!((0.0..=8.0).contains(&n[frame]) && n[frame].fract() == 0.0);
        assert!(c[frame] == 0.0 || c[frame] == 1.0);
        // held between the draws, one every 4 samples
        if frame > 0 && frame % 4 != 3 {
            assert_eq!(x[frame], x[frame - 1], "frame {frame}");
        }
        if frame % 4 == 3 {
            draws_of_x.push(x[frame]);
        }
    }
    let mut distinct = draws_of_x.clone();
    distinct.sort_by(f32::total_cmp);
    distinct.dedup();
    assert!(distinct.len() >= 6, "the draws vary: {draws_of_x:?}");
    assert!(n[3..].iter().any(|&v| v != n[3]) && c.contains(&1.0));
}

#[test]
fn sweep_covers_every_range_and_nonfinite_finds_the_pole() {
    let frames = 200;
    let outs = run("ctl_06_testing", &[], frames);
    let (both, one, bad) = (&outs[0], &outs[1], &outs[2]);
    let (lowest, highest) = both
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    assert!((lowest - 1.0).abs() < 1e-3 && (highest - 10.0).abs() < 1e-3);
    assert!(
        both.iter()
            .all(|&v| (1.0 - 1e-3..=10.0 + 1e-3).contains(&v))
    );
    // `a` alone from 0 to 1 and back, `b` on its knob
    assert!((one[0] - 0.25).abs() < 1e-6 && (one[50] - 10.25).abs() < 1e-4);
    let poles: Vec<usize> = (0..frames).filter(|&f| bad[f] != 0.0).collect();
    assert_eq!(poles, [25, 75, 125, 175]);
    assert!(poles.iter().all(|&f| bad[f] == 1.0));
}

#[test]
fn the_gradients_follow_the_fad_and_rad_layouts() {
    assert_eq!(
        first_samples("ctl_07_sensitivity", &[]),
        vec![2.0, 1.0, 0.0, 30.0, 0.0, 10.0, 2.0, 30.0, 1.0, 10.0]
    );
}

#[test]
fn every_documented_function_compiles_and_runs() {
    // 23 Test entries, 26 outputs (inits and the two gradients give two), one
    // input (nonfinite_test)
    let outs = run("ctl_all_functions", &[0.0], 256);
    assert_eq!(outs.len(), 26, "the outputs of every Test entry");
    for (channel, samples) in outs.iter().enumerate() {
        assert!(
            samples.iter().all(|v| v.is_finite()),
            "output {channel} is not finite"
        );
    }
}
