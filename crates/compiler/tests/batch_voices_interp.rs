//! The voice layout differential (G3.4 of
//! `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`, §3.4).
//!
//! A program laid out by `fir::batch::layout_voices` for N voices, run by the
//! interpreter one voice per call, must give each voice exactly the outputs
//! of an independent scalar instance fed the same inputs and controls: the
//! layout moves state, it does not change arithmetic. Every voice gets its
//! own input signal and its own control values, so a voice reading another
//! voice's state shows. N = 1, 3 and 64; interpreter optimisation levels 0
//! and max, so the optimiser cannot hide a layout fault either.
//!
//! The programs are written inline, without libraries (AGENTS.md §3): a
//! recursion behind a slider, a variable delay (circular line and counter),
//! an integer recursion, a wrapped phase oscillator, a generated table (its
//! generator state stays shared), two channels, and a button.

use codegen::backends::interp::{
    FbcDspFactory, FbcDspInstance, FbcOpcode, InterpBatch, InterpOptions, MAX_OPT_LEVEL,
    generate_interp_batch_module, generate_interp_module,
};
use compiler::{Compiler, SignalFirLane};
use fir::{FirId, FirStore};

const SR: i32 = 44_100;
const BLOCK: usize = 64;
const BLOCKS: usize = 5;

const PROGRAMS: &[(&str, &str)] = &[
    (
        "onepole",
        r#"process = _ : + ~ *(hslider("pole", 0.5, 0, 0.99, 0.01));"#,
    ),
    (
        "vdelay",
        r#"process = _ <: _, @(int(hslider("d", 10, 0, 64, 1))) :> _;"#,
    ),
    (
        "intrec",
        r#"process = ((+(1) : %(int(hslider("m", 37, 2, 64, 1)))) ~ _) * 0.01;"#,
    ),
    (
        "phase",
        r#"wrap(x) = x - floor(x);
phase = hslider("f", 0.01, 0, 0.5, 0.001) : (+ : wrap) ~ _;
process = sin(phase * 6.2831853);"#,
    ),
    (
        "table",
        r#"time = (+(1) ~ _) - 1;
tbl = rdtable(64, sin(float(time) * 0.1), int(hslider("i", 3, 0, 63, 1)));
process = tbl * (_ + 1);"#,
    ),
    (
        "stereo",
        r#"process = (_ : + ~ *(hslider("g", 0.5, 0, 0.9, 0.01))), (_ : @(3));"#,
    ),
    ("button", r#"process = button("gate") : + ~ *(0.9);"#),
];

/// The flattened module of `source`, as `-lang mojo` and the interpreter see
/// it, in its own store.
fn flat_module(name: &str, source: &str) -> (FirStore, FirId) {
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane(name, source, SignalFirLane::TransformFastLane)
        .unwrap_or_else(|e| panic!("{name}: FIR lowering failed: {e}"));
    if fir::subcontainer::has_sub_modules(&fir.store, fir.module) {
        fir::subcontainer::flatten_sub_modules_owned(
            &fir.store,
            fir.module,
            fir::subcontainer::SubModuleStatePolicy::MergedStructFields,
        )
        .unwrap_or_else(|e| panic!("{name}: flattening failed: {e}"))
    } else {
        let mut store = FirStore::new();
        let root = store.import_from(&fir.store, fir.module);
        (store, root)
    }
}

/// The input of channel `c` of voice `v` in block `b`: an impulse scaled per
/// voice, then a sine whose frequency depends on the voice and the channel.
fn input(v: usize, c: usize, b: usize, i: usize) -> f32 {
    let t = (b * BLOCK + i) as f32;
    let impulse = if b == 0 && i == 0 {
        1.0 + v as f32 * 0.25
    } else {
        0.0
    };
    impulse + 0.1 * (t * (0.01 + 0.003 * v as f32 + 0.007 * c as f32)).sin()
}

/// The value of widget `w` for voice `v` in block `b`: a distinct point of
/// the widget's range per voice; buttons pressed in the first block only,
/// on every other voice.
fn control(min: f32, max: f32, is_button: bool, w: usize, v: usize, b: usize) -> f32 {
    if is_button {
        return if b == 0 && v.is_multiple_of(2) {
            1.0
        } else {
            0.0
        };
    }
    let frac = ((v * 7 + w * 3) % 11) as f32 / 10.0;
    min + (max - min) * frac
}

/// `(offset, min, max, is_button)` of every widget, in interface order.
fn widgets(factory: &FbcDspFactory<f32>) -> Vec<(i32, f32, f32, bool)> {
    factory
        .ui_block
        .iter()
        .filter(|u| u.offset >= 0)
        .filter_map(|u| match u.opcode {
            FbcOpcode::AddButton | FbcOpcode::AddCheckButton => Some((u.offset, 0.0, 1.0, true)),
            FbcOpcode::AddHorizontalSlider
            | FbcOpcode::AddVerticalSlider
            | FbcOpcode::AddNumEntry => Some((u.offset, u.min, u.max, false)),
            _ => None,
        })
        .collect()
}

/// Outputs `[voice][channel][frame]` of N independent scalar instances.
fn run_scalar(factory: &mut FbcDspFactory<f32>, voices: usize) -> Vec<Vec<Vec<f32>>> {
    let ws = widgets(factory);
    let (n_in, n_out) = (factory.num_inputs as usize, factory.num_outputs as usize);
    let mut out = Vec::with_capacity(voices);
    for v in 0..voices {
        let mut dsp = FbcDspInstance::new(factory);
        dsp.init(SR);
        let mut channels = vec![Vec::with_capacity(BLOCK * BLOCKS); n_out];
        for b in 0..BLOCKS {
            for (w, &(offset, min, max, button)) in ws.iter().enumerate() {
                assert!(dsp.set_real_zone(offset, control(min, max, button, w, v, b)));
            }
            let ins: Vec<Vec<f32>> = (0..n_in)
                .map(|c| (0..BLOCK).map(|i| input(v, c, b, i)).collect())
                .collect();
            let ins: Vec<&[f32]> = ins.iter().map(Vec::as_slice).collect();
            let mut outs = vec![vec![0.0f32; BLOCK]; n_out];
            let mut outs_ref: Vec<&mut [f32]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
            dsp.try_compute(BLOCK as i32, &ins, &mut outs_ref)
                .expect("scalar compute");
            for (c, o) in outs.iter().enumerate() {
                channels[c].extend_from_slice(o);
            }
        }
        out.push(channels);
    }
    out
}

/// Outputs `[voice][channel][frame]` of one batch instance, voices
/// interleaved block by block as a host would run them.
fn run_batch(factory: &mut FbcDspFactory<f32>, voices: usize) -> Vec<Vec<Vec<f32>>> {
    let ws = widgets(factory);
    let (n_in, n_out) = (factory.num_inputs as usize, factory.num_outputs as usize);
    let mut dsp = FbcDspInstance::new(factory);
    dsp.class_init(SR);
    for v in 0..voices {
        dsp.instance_init_voice(SR, v as i32);
    }
    let mut out = vec![vec![Vec::with_capacity(BLOCK * BLOCKS); n_out]; voices];
    for b in 0..BLOCKS {
        for (v, voice_out) in out.iter_mut().enumerate() {
            for (w, &(offset, min, max, button)) in ws.iter().enumerate() {
                assert!(dsp.set_voice_zone(offset, v as i32, control(min, max, button, w, v, b)));
            }
            let ins: Vec<Vec<f32>> = (0..n_in)
                .map(|c| (0..BLOCK).map(|i| input(v, c, b, i)).collect())
                .collect();
            let ins: Vec<&[f32]> = ins.iter().map(Vec::as_slice).collect();
            let mut outs = vec![vec![0.0f32; BLOCK]; n_out];
            let mut outs_ref: Vec<&mut [f32]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
            dsp.compute_voice(v as i32, BLOCK as i32, &ins, &mut outs_ref)
                .expect("batch compute");
            for (c, o) in outs.iter().enumerate() {
                voice_out[c].extend_from_slice(o);
            }
        }
    }
    out
}

fn options(opt_level: i32) -> InterpOptions {
    InterpOptions {
        opt_level,
        ..InterpOptions::default()
    }
}

/// The differential for one program: every voice of its batch layout,
/// bit-identical to an independent scalar instance.
fn check_program(name: &str, source: &str) {
    {
        let (store, module) = flat_module(name, source);
        let (bstore, broot, layout) = fir::batch::layout_voices(&store, module)
            .unwrap_or_else(|e| panic!("{name}: layout refused: {e}"));
        for opt_level in [0, i32::from(MAX_OPT_LEVEL)] {
            let mut scalar = generate_interp_module::<f32>(&store, module, &options(opt_level))
                .unwrap_or_else(|e| panic!("{name}: scalar interp: {e}"));
            for voices in [1usize, 3, 64] {
                let batch = InterpBatch::from_layout(voices, &layout);
                let mut batched = generate_interp_batch_module::<f32>(
                    &bstore,
                    broot,
                    &options(opt_level),
                    &batch,
                )
                .unwrap_or_else(|e| panic!("{name}: batch interp: {e}"));
                let want = run_scalar(&mut scalar, voices);
                let got = run_batch(&mut batched, voices);
                for v in 0..voices {
                    for (c, (w, g)) in want[v].iter().zip(&got[v]).enumerate() {
                        let first = w
                            .iter()
                            .zip(g)
                            .position(|(a, b)| a.to_bits() != b.to_bits());
                        assert!(
                            first.is_none(),
                            "{name}, opt {opt_level}, {voices} voices: voice {v} channel {c} \
                             differs at frame {first:?}: scalar {:?}, batch {:?}",
                            first.map(|i| w[i]),
                            first.map(|i| g[i]),
                        );
                    }
                }
                // The voices really differ from each other, so a voice
                // reading another's state would have shown.
                if voices > 1 {
                    assert_ne!(want[0], want[1], "{name}: voices 0 and 1 are identical");
                }
            }
        }
    }
}

#[test]
fn every_voice_of_a_batch_matches_an_independent_scalar_instance() {
    for &(name, source) in PROGRAMS {
        check_program(name, source);
    }
}

/// The GPU batch voices of the golden corpus (`tests/corpus/gpu_*.dsp`), the
/// programs the GPU measurements of
/// `porting/faust-gpu-audio-synthesis-2026-11-07-en.md` used: the same
/// differential, on the files themselves.
#[test]
fn the_corpus_gpu_voices_match_their_scalar_instances() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    for name in ["gpu_voice_saw", "gpu_partial_sine", "gpu_mode_resonator"] {
        let path = corpus.join(format!("{name}.dsp"));
        let source =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        check_program(name, &source);
    }
}

/// The shared path is exercised: the generated table's working state stays
/// out of the arenas, and the controls are laid out per voice.
#[test]
fn the_corpus_covers_shared_state_and_per_voice_controls() {
    let (store, module) = flat_module("table", PROGRAMS[4].1);
    let (_, _, layout) = fir::batch::layout_voices(&store, module).unwrap();
    assert!(
        !layout.shared.is_empty(),
        "the table generator state should stay shared"
    );
    assert_eq!(layout.controls.len(), 1);
    let (store, module) = flat_module("button", PROGRAMS[6].1);
    let (_, _, layout) = fir::batch::layout_voices(&store, module).unwrap();
    assert_eq!(layout.controls.len(), 1);
}
