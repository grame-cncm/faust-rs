//! The facade over both backends: the same program, the same calls, the same
//! samples.

use faust::{Backend, CompileOptions, ControlKind, ErrorKind, Factory, Precision};

const GAIN: &str = r#"
declare name "gain_stage";
process = _ * hslider("gain [unit:dB]", 0.5, 0, 1, 0.01) : +(nentry("offset", 0, -1, 1, 0.1));
"#;

/// A one-pole with a button, a checkbox and a bargraph: state, two-state
/// controls and a value written by the DSP.
const ONE_POLE: &str = r#"
process = _ : *(checkbox("on")) : + ~ *(0.5) <: attach(_, abs : hbargraph("level", 0, 10));
"#;

fn both() -> [Backend; 2] {
    [Backend::Interp, Backend::Cranelift]
}

fn options(backend: Backend, precision: Precision) -> CompileOptions {
    CompileOptions {
        backend,
        precision,
        ..CompileOptions::default()
    }
}

#[test]
fn controls_have_the_paths_kinds_ranges_and_metadata_of_the_program() {
    for backend in both() {
        let factory =
            Factory::from_source("gain", GAIN, &options(backend, Precision::F32)).unwrap();
        let dsp = factory.create_dsp_instance(48_000).unwrap();
        assert_eq!(
            (dsp.get_num_inputs(), dsp.get_num_outputs()),
            (1, 1),
            "{backend}"
        );
        assert_eq!(dsp.get_sample_rate(), 48_000);
        let paths: Vec<&str> = dsp.controls().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/gain_stage/gain", "/gain_stage/offset"],
            "{backend}"
        );
        let gain = dsp.control("/gain_stage/gain").unwrap();
        assert_eq!(gain.kind, ControlKind::HorizontalSlider);
        assert_eq!((gain.init, gain.min, gain.max), (0.5, 0.0, 1.0));
        assert!((gain.step - 0.01).abs() < 1e-6);
        assert_eq!(
            gain.metadata,
            [("unit".to_owned(), "dB".to_owned())],
            "{backend}"
        );
        assert_eq!(
            dsp.control("/gain_stage/offset").unwrap().kind,
            ControlKind::NumEntry
        );
        assert_eq!(dsp.get_param_value("/gain_stage/gain").unwrap(), 0.5);
        // `metadata()` is the backend's `metadata` entry point; today the FIR
        // backends do not carry the program's `declare`s (a compiler gap, see
        // the crate documentation), so only the backend's own entries show
        let metadata = dsp.metadata();
        if backend == Backend::Cranelift {
            assert!(metadata.contains(&("backend".to_owned(), "cranelift".to_owned())));
        }
    }
}

#[test]
fn set_get_and_compute_on_both_backends() {
    for backend in both() {
        let factory =
            Factory::from_source("gain", GAIN, &options(backend, Precision::F32)).unwrap();
        let mut dsp = factory.create_dsp_instance(48_000).unwrap();
        let input = [1.0_f32; 16];
        let mut output = [0.0_f32; 16];
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        assert!(output.iter().all(|&y| y == 0.5), "{backend}: {output:?}");
        dsp.set_param_value("/gain_stage/gain", 0.25).unwrap();
        dsp.set_param_value("/gain_stage/offset", 1.0).unwrap();
        assert_eq!(dsp.get_param_value("/gain_stage/gain").unwrap(), 0.25);
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        assert!(output.iter().all(|&y| y == 1.25), "{backend}: {output:?}");
        // the same through f64 buffers
        let input64 = [2.0_f64; 16];
        let mut output64 = [0.0_f64; 16];
        dsp.compute(output64.len(), &[&input64], &mut [&mut output64])
            .unwrap();
        assert!(
            output64.iter().all(|&y| y == 1.5),
            "{backend}: {output64:?}"
        );
        // errors are typed
        assert_eq!(
            dsp.set_param_value("/gain_stage/nope", 1.0)
                .unwrap_err()
                .kind,
            ErrorKind::UnknownControl
        );
        assert_eq!(
            dsp.compute(output.len(), &[], &mut [&mut output])
                .unwrap_err()
                .kind,
            ErrorKind::Buffers
        );
        dsp.instance_reset_user_interface();
        assert_eq!(dsp.get_param_value("/gain_stage/gain").unwrap(), 0.5);
    }
}

#[test]
fn the_two_backends_produce_the_same_samples_on_a_stateful_program() {
    let mut outputs = Vec::new();
    for backend in both() {
        let factory =
            Factory::from_source("pole", ONE_POLE, &options(backend, Precision::F32)).unwrap();
        let mut dsp = factory.create_dsp_instance(44_100).unwrap();
        dsp.set_param_value("/pole/on", 1.0).unwrap();
        assert_eq!(
            dsp.set_param_value("/pole/level", 1.0).unwrap_err().kind,
            ErrorKind::ReadOnlyControl
        );
        let mut input = [0.0_f32; 32];
        input[0] = 1.0;
        let mut output = [0.0_f32; 32];
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        assert_eq!(output[0], 1.0, "{backend}");
        assert_eq!(output[1], 0.5, "{backend}");
        assert_eq!(output[2], 0.25, "{backend}");
        // the bargraph holds what the DSP last wrote
        assert!(
            (dsp.get_param_value("/pole/level").unwrap() - f64::from(output[31].abs())).abs()
                < 1e-6,
            "{backend}"
        );
        // clear empties the recursion, the controls are kept
        dsp.instance_clear();
        let silence = [0.0_f32; 32];
        dsp.compute(output.len(), &[&silence], &mut [&mut output])
            .unwrap();
        assert!(output.iter().all(|&y| y == 0.0), "{backend}");
        assert_eq!(dsp.get_param_value("/pole/on").unwrap(), 1.0, "{backend}");
        outputs.push(output);
    }
}

#[test]
fn the_two_backends_agree_sample_for_sample() {
    let mut results: Vec<Vec<f32>> = Vec::new();
    for backend in both() {
        let factory =
            Factory::from_source("pole", ONE_POLE, &options(backend, Precision::F32)).unwrap();
        let mut dsp = factory.create_dsp_instance(44_100).unwrap();
        dsp.set_param_value("/pole/on", 1.0).unwrap();
        let input: Vec<f32> = (0..256)
            .map(|i| ((i * 7919) % 97) as f32 / 97.0 - 0.5)
            .collect();
        let mut output = vec![0.0_f32; 256];
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        results.push(output);
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn double_precision_on_both_backends() {
    for backend in both() {
        let factory =
            Factory::from_source("gain", GAIN, &options(backend, Precision::F64)).unwrap();
        assert_eq!(factory.precision(), Precision::F64);
        let mut dsp = factory.create_dsp_instance(48_000).unwrap();
        dsp.set_param_value("/gain_stage/gain", 0.3).unwrap();
        assert!(
            (dsp.get_param_value("/gain_stage/gain").unwrap() - 0.3).abs() < 1e-9,
            "{backend}: the zone is an f64"
        );
        let input = [1.0_f64; 8];
        let mut output = [0.0_f64; 8];
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        assert!(
            output.iter().all(|&y| (y - 0.3).abs() < 1e-6),
            "{backend}: {output:?}"
        );
        let input32 = [1.0_f32; 8];
        let mut output32 = [0.0_f32; 8];
        dsp.compute(output32.len(), &[&input32], &mut [&mut output32])
            .unwrap();
        assert!(
            output32.iter().all(|&y| (y - 0.3).abs() < 1e-6),
            "{backend}: {output32:?}"
        );
    }
}

#[test]
fn an_instance_outlives_the_host_s_factory_handles() {
    for backend in both() {
        let mut dsp = {
            let factory =
                Factory::from_source("gain", GAIN, &options(backend, Precision::F32)).unwrap();
            let other = factory.clone();
            let dsp = other.create_dsp_instance(48_000).unwrap();
            drop(factory);
            drop(other);
            dsp
        };
        // the factory's code is still there: the instance keeps a reference
        let input = [1.0_f32; 4];
        let mut output = [0.0_f32; 4];
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        assert_eq!(output, [0.5; 4], "{backend}");
        assert_eq!(dsp.factory().get_name(), "gain");
        // and it can move to another thread
        let handle = std::thread::spawn(move || {
            let mut output = [0.0_f32; 4];
            dsp.compute(output.len(), &[&input], &mut [&mut output])
                .unwrap();
            output
        });
        assert_eq!(handle.join().unwrap(), [0.5; 4], "{backend}");
    }
}

#[test]
fn two_factories_of_the_same_program_are_independent_handles() {
    for backend in both() {
        let a = Factory::from_source("gain", GAIN, &options(backend, Precision::F32)).unwrap();
        let b = Factory::from_source("gain", GAIN, &options(backend, Precision::F32)).unwrap();
        let mut dsp_b = b.create_dsp_instance(48_000).unwrap();
        drop(a); // the cache keeps the program for `b` and its instance
        let input = [1.0_f32; 4];
        let mut output = [0.0_f32; 4];
        dsp_b
            .compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        assert_eq!(output, [0.5; 4], "{backend}");
        assert!(!b.get_json().is_empty());
    }
}

#[test]
fn a_program_that_does_not_compile_is_a_typed_error_with_the_compiler_s_message() {
    for backend in both() {
        let err = Factory::from_source(
            "bad",
            "process = undefined_thing;",
            &options(backend, Precision::F32),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Compile, "{backend}");
        assert!(
            err.message.contains("undefined_thing"),
            "{backend}: {}",
            err.message
        );
    }
}

#[test]
fn a_double_program_exchanges_f64_samples_exactly_on_both_backends() {
    // 1 + 2^-40 and 2^24 + 1 are not representable in f32: a narrowing
    // anywhere between the host's buffers and the program shows up here
    let tiny = 1.0 + 2.0_f64.powi(-40);
    for backend in both() {
        let factory =
            Factory::from_source("wire", "process = _;", &options(backend, Precision::F64))
                .unwrap();
        let mut dsp = factory.create_dsp_instance(48_000).unwrap();
        let input = [tiny; 8];
        let mut output = [0.0_f64; 8];
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        assert_eq!(output, [tiny; 8], "{backend}: the input was narrowed");

        let factory = Factory::from_source(
            "constant",
            "process = 16777217.0;",
            &options(backend, Precision::F64),
        )
        .unwrap();
        let mut dsp = factory.create_dsp_instance(48_000).unwrap();
        let mut output = [0.0_f64; 8];
        dsp.compute(output.len(), &[], &mut [&mut output]).unwrap();
        assert_eq!(
            output, [16_777_217.0; 8],
            "{backend}: the output was narrowed"
        );
    }
}

#[test]
fn f64_buffers_on_a_single_precision_program_are_converted() {
    // the other direction: an f32 program run through f64 buffers computes in
    // f32, the conversion rounding to the nearest f32 on entry
    let tiny = 1.0 + 2.0_f64.powi(-40);
    for backend in both() {
        let factory =
            Factory::from_source("wire", "process = _;", &options(backend, Precision::F32))
                .unwrap();
        let mut dsp = factory.create_dsp_instance(48_000).unwrap();
        let input = [tiny, 0.1, -3.5, 16_777_217.0];
        let mut output = [0.0_f64; 4];
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        assert_eq!(output, input.map(|x| f64::from(x as f32)), "{backend}");
    }
}

#[test]
fn control_ranges_are_those_of_the_compiled_precision() {
    // 0.1, 0.01, -1.7, ... are not f32 values: a -double program must report
    // them as written, a single one as the f32 values its zones hold
    const RANGES: &str = r#"
process = hslider("x", 0.1, 0, 1, 0.01) + nentry("y", 0.3, -1.7, 2.9, 0.001)
        : vbargraph("v", -0.3, 0.7);
"#;
    for backend in both() {
        for precision in [Precision::F32, Precision::F64] {
            let at = |v: f64| match precision {
                Precision::F32 => f64::from(v as f32),
                Precision::F64 => v,
            };
            let factory = Factory::from_source("p", RANGES, &options(backend, precision)).unwrap();
            let mut dsp = factory.create_dsp_instance(48_000).unwrap();
            let what = format!("{backend} {precision:?}");
            let x = dsp.control("/p/x").unwrap().clone();
            assert_eq!(
                (x.init, x.min, x.max, x.step),
                (at(0.1), 0.0, 1.0, at(0.01)),
                "{what}"
            );
            let y = dsp.control("/p/y").unwrap().clone();
            assert_eq!(
                (y.init, y.min, y.max, y.step),
                (at(0.3), at(-1.7), at(2.9), at(0.001)),
                "{what}"
            );
            let v = dsp.control("/p/v").unwrap().clone();
            assert_eq!((v.min, v.max), (at(-0.3), at(0.7)), "{what}");
            // the declared initial value is the one the program resets to
            dsp.set_param_value("/p/x", 0.5).unwrap();
            dsp.set_param_value("/p/y", 0.5).unwrap();
            dsp.instance_reset_user_interface();
            assert_eq!(dsp.get_param_value("/p/x").unwrap(), x.init, "{what}");
            assert_eq!(dsp.get_param_value("/p/y").unwrap(), y.init, "{what}");
        }
    }
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn dsp_and_factory_are_send_and_sync() {
    // what PyO3 asks of a `#[pyclass]` field
    assert_send_sync::<faust::Dsp>();
    assert_send_sync::<Factory>();
}

#[test]
fn one_dsp_can_be_read_from_several_threads_at_once() {
    for backend in both() {
        let factory =
            Factory::from_source("pole", ONE_POLE, &options(backend, Precision::F32)).unwrap();
        let mut dsp = factory.create_dsp_instance(44_100).unwrap();
        dsp.set_param_value("/pole/on", 1.0).unwrap();
        let input = [0.75_f32; 16];
        let mut output = [0.0_f32; 16];
        dsp.compute(output.len(), &[&input], &mut [&mut output])
            .unwrap();
        let level = dsp.get_param_value("/pole/level").unwrap();
        let metadata = dsp.metadata();
        let dsp = &dsp;
        std::thread::scope(|scope| {
            let readers: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..200 {
                            assert_eq!(dsp.get_param_value("/pole/level").unwrap(), level);
                            assert_eq!(dsp.get_param_value("/pole/on").unwrap(), 1.0);
                            assert_eq!(dsp.get_sample_rate(), 44_100);
                            assert_eq!(dsp.controls().count(), 2);
                            assert_eq!(dsp.metadata(), metadata);
                        }
                    })
                })
                .collect();
            for reader in readers {
                reader.join().unwrap();
            }
        });
    }
}

#[test]
fn a_control_keeps_its_label_as_the_program_wrote_it() {
    // the path replaces what an OSC address cannot hold, so it cannot give
    // the label back; the metadata in brackets is not part of the label
    const LABELS: &str = r#"process = hslider("my gain [unit:dB]", 0.5, 0, 1, 0.01) + nentry("a/b (x)", 0, 0, 1, 1);"#;
    for backend in both() {
        let factory =
            Factory::from_source("labels", LABELS, &options(backend, Precision::F32)).unwrap();
        let dsp = factory.create_dsp_instance(48_000).unwrap();
        let gain = dsp.control("/labels/my_gain").unwrap();
        assert_eq!(gain.label, "my gain", "{backend}");
        let entry = dsp.control("/labels/a_b__x_").unwrap();
        assert_eq!(entry.label, "a/b (x)", "{backend}");
    }
}

#[test]
fn controls_come_in_the_order_of_the_user_interface() {
    // `[n]` orders the widgets of a group, as in every Faust UI: the order
    // of `buildUserInterface`, not the alphabetical order of the paths
    const ORDERED: &str = r#"
process = hslider("[2]alpha", 0, 0, 1, 0.1), hslider("[1]beta", 0, 0, 1, 0.1),
          hgroup("[0]group", nentry("[1]zeta", 0, 0, 1, 1), nentry("[0]eta", 0, 0, 1, 1));
"#;
    for backend in both() {
        let factory =
            Factory::from_source("ui", ORDERED, &options(backend, Precision::F32)).unwrap();
        let dsp = factory.create_dsp_instance(48_000).unwrap();
        let paths: Vec<&str> = dsp.controls().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/ui/group/eta", "/ui/group/zeta", "/ui/beta", "/ui/alpha"],
            "{backend}"
        );
        // lookup by path is unchanged
        assert_eq!(dsp.control("/ui/alpha").unwrap().label, "alpha");
    }
}

#[test]
fn a_program_cranelift_cannot_lower_is_refused_at_instantiate() {
    // a foreign function with no bound symbol falls outside the Cranelift
    // lowering subset: the factory compiles with an empty `compute`, and the
    // facade refuses to instantiate what would be a silent instance; the
    // interpreter refuses the program at compile time
    const FOREIGN: &str = r#"process = _ : ffunction(float frs_unknown_fn(float), "", "");"#;
    let factory = Factory::from_source(
        "foreign",
        FOREIGN,
        &options(Backend::Cranelift, Precision::F32),
    )
    .unwrap();
    let err = factory.create_dsp_instance(48_000).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Instantiate);
    assert!(err.message.contains("did not lower"), "{}", err.message);

    let err = Factory::from_source(
        "foreign",
        FOREIGN,
        &options(Backend::Interp, Precision::F32),
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Compile);
    assert!(err.message.contains("frs_unknown_fn"), "{}", err.message);
}

#[test]
fn instances_are_created_while_another_one_computes() {
    // instantiation and the JSON touch the shared factory while an instance
    // of it computes on another thread (run it under ThreadSanitizer too)
    for backend in both() {
        let factory =
            Factory::from_source("pole", ONE_POLE, &options(backend, Precision::F32)).unwrap();
        let mut running = factory.create_dsp_instance(48_000).unwrap();
        running.set_param_value("/pole/on", 1.0).unwrap();
        std::thread::scope(|scope| {
            let computing = scope.spawn(move || {
                let input = [0.5_f32; 64];
                let mut output = [0.0_f32; 64];
                for _ in 0..200 {
                    running
                        .compute(output.len(), &[&input], &mut [&mut output])
                        .unwrap();
                }
                output
            });
            for _ in 0..50 {
                let dsp = factory.create_dsp_instance(48_000).unwrap();
                assert_eq!(dsp.get_num_outputs(), 1);
                assert!(!factory.get_json().is_empty());
            }
            // the recursion has converged: y = 0.5 + 0.5 * y
            let output = computing.join().unwrap();
            assert!((output[63] - 1.0).abs() < 1e-6, "{backend}: {output:?}");
        });
    }
}

/// A scratch directory per test, under the target directory.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The first output sample of `factory`, silent input.
fn first_sample(factory: &Factory) -> f32 {
    let mut dsp = factory.create_dsp_instance(48_000).unwrap();
    let mut out = [0.0_f32; 1];
    dsp.compute(out.len(), &[], &mut [&mut out]).unwrap();
    out[0]
}

#[test]
fn import_dirs_are_searched_in_order_and_before_the_file_s_directory() {
    // `facade_probe.lib` exists in no installed Faust: each directory gives it
    // a different value, and the value read says which one was found.
    let root = scratch("facade_import_order");
    for (dir, value) in [("first", "0.25"), ("second", "0.5"), ("program", "0.75")] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::write(
            root.join(dir).join("facade_probe.lib"),
            format!("value = {value};\n"),
        )
        .unwrap();
    }
    let program = root.join("program").join("probe.dsp");
    std::fs::write(&program, "process = library(\"facade_probe.lib\").value;\n").unwrap();
    for backend in both() {
        let with_dirs = CompileOptions {
            import_dirs: vec![root.join("first"), root.join("second")],
            ..options(backend, Precision::F32)
        };
        // the first directory of the list wins, from a source string ...
        let source = Factory::from_source(
            "probe",
            "process = library(\"facade_probe.lib\").value;",
            &with_dirs,
        )
        .unwrap();
        assert_eq!(first_sample(&source), 0.25, "{backend}");
        // ... and from a file, whose own directory comes after them
        let file = Factory::from_file(&program, &with_dirs).unwrap();
        assert_eq!(first_sample(&file), 0.25, "{backend}");
        let alone = Factory::from_file(&program, &options(backend, Precision::F32)).unwrap();
        assert_eq!(first_sample(&alone), 0.75, "{backend}");
    }
}

#[test]
fn two_state_controls_range_over_0_1_by_1_and_bargraphs_have_no_step() {
    // what `Control::step` documents
    for backend in both() {
        let factory =
            Factory::from_source("one_pole", ONE_POLE, &options(backend, Precision::F32)).unwrap();
        let dsp = factory.create_dsp_instance(48_000).unwrap();
        for control in dsp.controls() {
            let expected = match control.kind {
                ControlKind::CheckButton | ControlKind::Button => (0.0, 1.0, 1.0),
                ControlKind::HorizontalBargraph => (0.0, 10.0, 0.0),
                other => panic!("unexpected {other:?}"),
            };
            assert_eq!(
                (control.min, control.max, control.step),
                expected,
                "{backend} {}",
                control.path
            );
        }
    }
}

#[test]
fn compute_runs_count_frames_and_refuses_a_shorter_buffer() {
    for backend in both() {
        let factory =
            Factory::from_source("gain", GAIN, &options(backend, Precision::F32)).unwrap();
        let mut dsp = factory.create_dsp_instance(48_000).unwrap();
        let input = [1.0_f32; 8];
        // `count` frames written, the rest of a longer buffer left as it is
        let mut output = [-1.0_f32; 8];
        dsp.compute(4, &[&input], &mut [&mut output]).unwrap();
        assert_eq!(
            output,
            [0.5, 0.5, 0.5, 0.5, -1.0, -1.0, -1.0, -1.0],
            "{backend}"
        );
        // a buffer shorter than `count` is refused before the backend runs
        let mut short = [0.0_f32; 2];
        let err = dsp.compute(4, &[&input], &mut [&mut short]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Buffers, "{backend}");
        assert!(
            err.message.contains("fewer than the 4 frames"),
            "{}",
            err.message
        );
    }
}

#[test]
fn instance_constants_recomputes_the_rate_dependent_constants_and_keeps_the_controls() {
    // `ma.SR`, spelled out: the sample rate is an instance constant
    const RATE: &str = r#"
process = fconstant(int fSamplingFreq, <math.h>) * hslider("g", 1, 0, 2, 0.01);
"#;
    for backend in both() {
        let factory =
            Factory::from_source("rate", RATE, &options(backend, Precision::F32)).unwrap();
        let mut dsp = factory.create_dsp_instance(48_000).unwrap();
        dsp.set_param_value("/rate/g", 0.5).unwrap();
        dsp.instance_constants(44_100);
        assert_eq!(dsp.get_sample_rate(), 44_100, "{backend}");
        assert_eq!(
            dsp.get_param_value("/rate/g").unwrap(),
            0.5,
            "{backend}: the control was reset"
        );
        let mut out = [0.0_f32; 1];
        dsp.compute(1, &[], &mut [&mut out]).unwrap();
        assert_eq!(out[0], 22_050.0, "{backend}");
    }
}

/// Paths and shortnames printed by the C++ `MapUI` (`fFullPaths` and
/// `fFull2Short`) of Faust 2.89.3 for the same programs: the reference the
/// port of `PathBuilder::computeShortNames` is checked against.
#[test]
fn paths_and_shortnames_are_those_of_the_cpp_mapui() {
    const NAMES: &str = r#"
declare name "names";
osc(i) = vgroup("osc%i", hslider("freq", 440, 20, 2000, 1) * hslider("my gain [unit:dB]", 0.5, 0, 1, 0.01));
process = hgroup("synth", (par(i, 2, osc(i)) :> _) * hslider("volume", 1, 0, 1, 0.01) + vgroup("fx", hgroup("echo", hslider("mix", 0, 0, 1, 0.1)) + hgroup("reverb", hslider("mix", 0, 0, 1, 0.1)) + hgroup("[2]deep/er", checkbox("mix"))));
"#;
    const DEEP: &str = r#"
declare name "deep";
process = vgroup("a", vgroup("x", hslider("g", 0, 0, 1, 0.1))) + vgroup("b", vgroup("x", hslider("g", 0, 0, 1, 0.1))) + vgroup("c", hslider("g", 0, 0, 1, 0.1)) + hslider("solo #1", 0, 0, 1, 0.1);
"#;
    /// A program's name, its source, and its `(path, shortname)` pairs.
    type Case<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);
    let cases: [Case; 2] = [
        (
            "names",
            NAMES,
            &[
                ("/synth/fx/deep_er/mix", "deep_er_mix"),
                ("/synth/fx/echo/mix", "echo_mix"),
                ("/synth/fx/reverb/mix", "reverb_mix"),
                ("/synth/osc0/freq", "osc0_freq"),
                ("/synth/osc0/my_gain", "osc0_my_gain"),
                ("/synth/osc1/freq", "osc1_freq"),
                ("/synth/osc1/my_gain", "osc1_my_gain"),
                ("/synth/volume", "volume"),
            ],
        ),
        (
            "deep",
            DEEP,
            &[
                ("/deep/a/x/g", "a_x_g"),
                ("/deep/b/x/g", "b_x_g"),
                ("/deep/c/g", "c_g"),
                ("/deep/solo__1", "solo_1"),
            ],
        ),
    ];
    for backend in both() {
        for (name, source, expected) in cases {
            let factory =
                Factory::from_source(name, source, &options(backend, Precision::F32)).unwrap();
            let dsp = factory.create_dsp_instance(48_000).unwrap();
            let got: Vec<(&str, &str)> = dsp
                .controls()
                .map(|c| (c.path.as_str(), c.shortname.as_str()))
                .collect();
            assert_eq!(got, expected, "{backend} {name}");
        }
    }
}

/// What the C++ `MapUI` of Faust 2.89.3 does with the same program:
/// `setParamValue("a_x", 0.25)` writes `/look/a/x` (a shortname, before the
/// label of two other controls), `setParamValue("y", 0.5)` writes
/// `/look/h/y` (the last control declared with that label).
#[test]
fn a_control_is_found_by_path_then_shortname_then_label_as_in_mapui() {
    const LOOK: &str = r#"
declare name "look";
process = vgroup("a", hslider("x", 0, 0, 1, 0.01)) + vgroup("b", hslider("x", 0, 0, 1, 0.01))
        + vgroup("g", hslider("a_x", 0, 0, 1, 0.01) + hslider("y", 0, 0, 1, 0.01))
        + vgroup("h", hslider("a_x", 0, 0, 1, 0.01) + hslider("y", 0, 0, 1, 0.01));
"#;
    for backend in both() {
        let factory =
            Factory::from_source("look", LOOK, &options(backend, Precision::F64)).unwrap();
        let mut dsp = factory.create_dsp_instance(48_000).unwrap();
        dsp.set_param_value("a_x", 0.25).unwrap();
        dsp.set_param_value("y", 0.5).unwrap();
        dsp.set_param_value("/look/g/y", 0.75).unwrap();
        let values: Vec<(&str, f64)> = dsp
            .controls()
            .map(|c| (c.path.as_str(), dsp.get_param_value(&c.path).unwrap()))
            .collect();
        assert_eq!(
            values,
            [
                ("/look/a/x", 0.25),
                ("/look/b/x", 0.0),
                ("/look/g/a_x", 0.0),
                ("/look/g/y", 0.75),
                ("/look/h/a_x", 0.0),
                ("/look/h/y", 0.5),
            ],
            "{backend}"
        );
        // the three names of one control read the same value
        for name in ["/look/h/y", "h_y", "y"] {
            assert_eq!(dsp.get_param_value(name).unwrap(), 0.5, "{backend} {name}");
            assert_eq!(
                dsp.control(name).unwrap().path,
                "/look/h/y",
                "{backend} {name}"
            );
        }
        let err = dsp.set_param_value("nothing", 1.0).unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnknownControl, "{backend}");
    }
}

/// C++ `checkNullLabel` names a group with an empty label `0x00`, and the
/// C++ `MapUI` of Faust 2.89.3 reports these paths for this program; the
/// shortnames leave the unnamed group out (`PathBuilder::remove0x00`).
#[test]
fn a_group_with_an_empty_label_is_0x00_in_the_paths_as_in_cpp() {
    const EMPTY: &str = r#"
declare name "empty";
process = hgroup("", hslider("g", 0, 0, 1, 0.1)) + hgroup("", hslider("h", 0, 0, 1, 0.1)) + vgroup("0x00", hslider("k", 0, 0, 1, 0.1));
"#;
    for backend in both() {
        let factory =
            Factory::from_source("empty", EMPTY, &options(backend, Precision::F32)).unwrap();
        let dsp = factory.create_dsp_instance(48_000).unwrap();
        let names: Vec<(&str, &str)> = dsp
            .controls()
            .map(|c| (c.path.as_str(), c.shortname.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("/empty/0x00/g", "g"),
                ("/empty/0x00/h", "h"),
                ("/empty/0x00/k", "k"),
            ],
            "{backend}"
        );
    }
}
