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
        let dsp = factory.instantiate(48_000).unwrap();
        assert_eq!((dsp.num_inputs(), dsp.num_outputs()), (1, 1), "{backend}");
        assert_eq!(dsp.sample_rate(), 48_000);
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
        assert_eq!(dsp.get("/gain_stage/gain").unwrap(), 0.5);
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
        let mut dsp = factory.instantiate(48_000).unwrap();
        let input = [1.0_f32; 16];
        let mut output = [0.0_f32; 16];
        dsp.compute_f32(&[&input], &mut [&mut output]).unwrap();
        assert!(output.iter().all(|&y| y == 0.5), "{backend}: {output:?}");
        dsp.set("/gain_stage/gain", 0.25).unwrap();
        dsp.set("/gain_stage/offset", 1.0).unwrap();
        assert_eq!(dsp.get("/gain_stage/gain").unwrap(), 0.25);
        dsp.compute_f32(&[&input], &mut [&mut output]).unwrap();
        assert!(output.iter().all(|&y| y == 1.25), "{backend}: {output:?}");
        // the same through f64 buffers
        let input64 = [2.0_f64; 16];
        let mut output64 = [0.0_f64; 16];
        dsp.compute_f64(&[&input64], &mut [&mut output64]).unwrap();
        assert!(
            output64.iter().all(|&y| y == 1.5),
            "{backend}: {output64:?}"
        );
        // errors are typed
        assert_eq!(
            dsp.set("/gain_stage/nope", 1.0).unwrap_err().kind,
            ErrorKind::UnknownControl
        );
        assert_eq!(
            dsp.compute_f32(&[], &mut [&mut output]).unwrap_err().kind,
            ErrorKind::Buffers
        );
        dsp.reset_controls();
        assert_eq!(dsp.get("/gain_stage/gain").unwrap(), 0.5);
    }
}

#[test]
fn the_two_backends_produce_the_same_samples_on_a_stateful_program() {
    let mut outputs = Vec::new();
    for backend in both() {
        let factory =
            Factory::from_source("pole", ONE_POLE, &options(backend, Precision::F32)).unwrap();
        let mut dsp = factory.instantiate(44_100).unwrap();
        dsp.set("/pole/on", 1.0).unwrap();
        assert_eq!(
            dsp.set("/pole/level", 1.0).unwrap_err().kind,
            ErrorKind::ReadOnlyControl
        );
        let mut input = [0.0_f32; 32];
        input[0] = 1.0;
        let mut output = [0.0_f32; 32];
        dsp.compute_f32(&[&input], &mut [&mut output]).unwrap();
        assert_eq!(output[0], 1.0, "{backend}");
        assert_eq!(output[1], 0.5, "{backend}");
        assert_eq!(output[2], 0.25, "{backend}");
        // the bargraph holds what the DSP last wrote
        assert!(
            (dsp.get("/pole/level").unwrap() - f64::from(output[31].abs())).abs() < 1e-6,
            "{backend}"
        );
        // clear empties the recursion, the controls are kept
        dsp.clear();
        let silence = [0.0_f32; 32];
        dsp.compute_f32(&[&silence], &mut [&mut output]).unwrap();
        assert!(output.iter().all(|&y| y == 0.0), "{backend}");
        assert_eq!(dsp.get("/pole/on").unwrap(), 1.0, "{backend}");
        outputs.push(output);
    }
}

#[test]
fn the_two_backends_agree_sample_for_sample() {
    let mut results: Vec<Vec<f32>> = Vec::new();
    for backend in both() {
        let factory =
            Factory::from_source("pole", ONE_POLE, &options(backend, Precision::F32)).unwrap();
        let mut dsp = factory.instantiate(44_100).unwrap();
        dsp.set("/pole/on", 1.0).unwrap();
        let input: Vec<f32> = (0..256)
            .map(|i| ((i * 7919) % 97) as f32 / 97.0 - 0.5)
            .collect();
        let mut output = vec![0.0_f32; 256];
        dsp.compute_f32(&[&input], &mut [&mut output]).unwrap();
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
        let mut dsp = factory.instantiate(48_000).unwrap();
        dsp.set("/gain_stage/gain", 0.3).unwrap();
        assert!(
            (dsp.get("/gain_stage/gain").unwrap() - 0.3).abs() < 1e-9,
            "{backend}: the zone is an f64"
        );
        let input = [1.0_f64; 8];
        let mut output = [0.0_f64; 8];
        dsp.compute_f64(&[&input], &mut [&mut output]).unwrap();
        assert!(
            output.iter().all(|&y| (y - 0.3).abs() < 1e-6),
            "{backend}: {output:?}"
        );
        let input32 = [1.0_f32; 8];
        let mut output32 = [0.0_f32; 8];
        dsp.compute_f32(&[&input32], &mut [&mut output32]).unwrap();
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
            let dsp = other.instantiate(48_000).unwrap();
            drop(factory);
            drop(other);
            dsp
        };
        // the factory's code is still there: the instance keeps a reference
        let input = [1.0_f32; 4];
        let mut output = [0.0_f32; 4];
        dsp.compute_f32(&[&input], &mut [&mut output]).unwrap();
        assert_eq!(output, [0.5; 4], "{backend}");
        assert_eq!(dsp.factory().name(), "gain");
        // and it can move to another thread
        let handle = std::thread::spawn(move || {
            let mut output = [0.0_f32; 4];
            dsp.compute_f32(&[&input], &mut [&mut output]).unwrap();
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
        let mut dsp_b = b.instantiate(48_000).unwrap();
        drop(a); // the cache keeps the program for `b` and its instance
        let input = [1.0_f32; 4];
        let mut output = [0.0_f32; 4];
        dsp_b.compute_f32(&[&input], &mut [&mut output]).unwrap();
        assert_eq!(output, [0.5; 4], "{backend}");
        assert!(!b.json().is_empty());
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
        let mut dsp = factory.instantiate(48_000).unwrap();
        let input = [tiny; 8];
        let mut output = [0.0_f64; 8];
        dsp.compute_f64(&[&input], &mut [&mut output]).unwrap();
        assert_eq!(output, [tiny; 8], "{backend}: the input was narrowed");

        let factory = Factory::from_source(
            "constant",
            "process = 16777217.0;",
            &options(backend, Precision::F64),
        )
        .unwrap();
        let mut dsp = factory.instantiate(48_000).unwrap();
        let mut output = [0.0_f64; 8];
        dsp.compute_f64(&[], &mut [&mut output]).unwrap();
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
        let mut dsp = factory.instantiate(48_000).unwrap();
        let input = [tiny, 0.1, -3.5, 16_777_217.0];
        let mut output = [0.0_f64; 4];
        dsp.compute_f64(&[&input], &mut [&mut output]).unwrap();
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
            let mut dsp = factory.instantiate(48_000).unwrap();
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
            dsp.set("/p/x", 0.5).unwrap();
            dsp.set("/p/y", 0.5).unwrap();
            dsp.reset_controls();
            assert_eq!(dsp.get("/p/x").unwrap(), x.init, "{what}");
            assert_eq!(dsp.get("/p/y").unwrap(), y.init, "{what}");
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
        let mut dsp = factory.instantiate(44_100).unwrap();
        dsp.set("/pole/on", 1.0).unwrap();
        let input = [0.75_f32; 16];
        let mut output = [0.0_f32; 16];
        dsp.compute_f32(&[&input], &mut [&mut output]).unwrap();
        let level = dsp.get("/pole/level").unwrap();
        let metadata = dsp.metadata();
        let dsp = &dsp;
        std::thread::scope(|scope| {
            let readers: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..200 {
                            assert_eq!(dsp.get("/pole/level").unwrap(), level);
                            assert_eq!(dsp.get("/pole/on").unwrap(), 1.0);
                            assert_eq!(dsp.sample_rate(), 44_100);
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
        let dsp = factory.instantiate(48_000).unwrap();
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
        let dsp = factory.instantiate(48_000).unwrap();
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
    let err = factory.instantiate(48_000).unwrap_err();
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
        let mut running = factory.instantiate(48_000).unwrap();
        running.set("/pole/on", 1.0).unwrap();
        std::thread::scope(|scope| {
            let computing = scope.spawn(move || {
                let input = [0.5_f32; 64];
                let mut output = [0.0_f32; 64];
                for _ in 0..200 {
                    running.compute_f32(&[&input], &mut [&mut output]).unwrap();
                }
                output
            });
            for _ in 0..50 {
                let dsp = factory.instantiate(48_000).unwrap();
                assert_eq!(dsp.num_outputs(), 1);
                assert!(!factory.json().is_empty());
            }
            // the recursion has converged: y = 0.5 + 0.5 * y
            let output = computing.join().unwrap();
            assert!((output[63] - 1.0).abs() < 1e-6, "{backend}: {output:?}");
        });
    }
}
