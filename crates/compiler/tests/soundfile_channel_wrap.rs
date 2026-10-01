//! Soundfile channel wrap: `soundfile(label, N)` reads real channel
//! `chan % fChannels` when the resource has fewer than `N` channels.
//!
//! # Source provenance (C++)
//! - `InstructionsCompiler::generateSoundfileBuffer`
//!   (`compiler/generator/instructions_compiler.cpp`): the wrap is generated
//!   code, channel 0 is left unwrapped, and the runtime only provides the
//!   `fChannels` real channel buffers (no duplication up to `MAX_CHAN`).
//!
//! The runtime fixtures below hold the real channels only, so a missing wrap
//! reads past them (silence in the interpreter) instead of passing unseen.

use std::io::Cursor;

use codegen::backends::cpp::CppOptions;
use codegen::backends::interp::{FbcDspInstance, FbcOpcode, InterpOptions, Soundfile, read_fbc};
use compiler::{Compiler, ComputeMode, SignalFirLane};

/// A soundfile read with 70 outputs: past the former 64-channel sharing limit.
const OUTPUTS: usize = 70;

/// Reads part 0, sample 0 of each of the `outputs` channels (no standard
/// library: tests are self-contained).
fn source(outputs: usize) -> String {
    let bus = vec!["_"; outputs].join(",");
    format!("process = 0, 0 : soundfile(\"sf[url:{{'a.wav'}}]\", {outputs}) : !, !, {bus};\n")
}

/// A `channels`-channel, one-part soundfile whose channel `c` is the constant
/// `c + 1`, so every output names the real channel it read.
fn constant_soundfile(channels: usize) -> Soundfile {
    const PARTS: usize = 256;
    const LENGTH: usize = 16;
    Soundfile {
        num_channels: channels,
        num_parts: 1,
        lengths: vec![LENGTH as i32; PARTS],
        sample_rates: vec![44100; PARTS],
        offsets: (0..PARTS).map(|part| (part * LENGTH) as i32).collect(),
        buffers: (0..channels)
            .map(|chan| vec![(chan + 1) as f64; PARTS * LENGTH])
            .collect(),
    }
}

fn run_interp(mode: ComputeMode, channels: usize) -> Vec<f32> {
    let fbc = Compiler::new()
        .with_compute_mode(mode)
        .compile_source_to_interp_with_lane(
            "soundfile_channel_wrap",
            &source(OUTPUTS),
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|error| panic!("interp compilation failed ({mode:?}): {error}"));
    let mut factory = read_fbc::<f32>(&mut Cursor::new(fbc)).expect("fbc parse");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(44100);
    let slot = instance
        .ui_instructions()
        .iter()
        .find(|ui| ui.opcode == FbcOpcode::AddSoundfile)
        .map(|ui| usize::try_from(ui.offset).expect("soundfile slot"))
        .expect("one soundfile");
    assert!(instance.set_soundfile(slot, constant_soundfile(channels)));

    let num_outputs = usize::try_from(instance.get_num_outputs()).expect("outputs");
    assert_eq!(num_outputs, OUTPUTS);
    let frames = 8;
    let mut outputs = vec![vec![0.0_f32; frames]; num_outputs];
    let mut slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(frames as i32, &[], &mut slices)
        .expect("compute");
    outputs.iter().map(|output| output[frames - 1]).collect()
}

/// Runs `test` on a large stack: the 70-wide output bus nests its `par`
/// instances to the right, and propagation recurses through them.
fn on_large_stack(test: fn()) {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(test)
        .expect("spawn the test thread")
        .join()
        .expect("the test thread completes");
}

#[test]
fn interp_reads_channel_modulo_real_channel_count() {
    on_large_stack(interp_reads_channel_modulo_real_channel_count_body);
}

fn interp_reads_channel_modulo_real_channel_count_body() {
    for mode in [
        ComputeMode::Scalar,
        ComputeMode::Vector {
            vec_size: 4,
            loop_variant: 0,
        },
        ComputeMode::Vector {
            vec_size: 4,
            loop_variant: 1,
        },
    ] {
        for channels in [1_usize, 2, 3] {
            let got = run_interp(mode, channels);
            for (chan, value) in got.iter().enumerate() {
                let expected = (chan % channels + 1) as f32;
                assert_eq!(
                    *value,
                    expected,
                    "{mode:?}, {channels}-channel soundfile: output {chan} must read real channel {}",
                    chan % channels
                );
            }
        }
    }
}

#[test]
fn generated_cpp_wraps_every_channel_but_zero() {
    for mode in [
        ComputeMode::Scalar,
        ComputeMode::Vector {
            vec_size: 32,
            loop_variant: 1,
        },
    ] {
        let cpp = Compiler::new()
            .with_compute_mode(mode)
            .compile_source_to_cpp("soundfile_channel_wrap", &source(4), &CppOptions::default())
            .unwrap_or_else(|error| panic!("C++ compilation failed ({mode:?}): {error}"));
        assert!(
            cpp.contains("->fBuffers)[0]["),
            "{mode:?}: channel 0 always exists and is read unwrapped:\n{cpp}"
        );
        assert!(
            !cpp.contains("(0 % "),
            "{mode:?}: channel 0 must not be wrapped:\n{cpp}"
        );
        for chan in 1..4 {
            let wrapped = format!("->fBuffers)[({chan} % fSound0->fChannels)][");
            assert!(
                cpp.contains(&wrapped),
                "{mode:?}: channel {chan} must read `{wrapped}`:\n{cpp}"
            );
        }
    }
}
