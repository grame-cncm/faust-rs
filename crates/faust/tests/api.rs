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
