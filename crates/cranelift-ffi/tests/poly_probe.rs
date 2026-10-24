//! End-to-end checks for the polyphonic probe wrapper (design phase P4).
//!
//! The unit tests in `probe::poly` cover the allocation, stealing and
//! reclamation policy against `poly-dsp.h` without a JIT. These exercise the
//! parts that only exist once real instances are rendering: that voices
//! actually sum, that a released voice is reclaimed once it decays below the
//! stop level, and that the effect DSP runs on the mix rather than per voice.
//!
//! The DSP sources are written inline rather than read from disk, so the tests
//! do not depend on a locally installed Faust or on anything outside the repo
//! (AGENTS.md section 3).

use cranelift_ffi::probe::engine::PolyProbe;

/// Minimal polyphonic instrument: a sine at `freq`, gated, scaled by `gain`.
///
/// Written from Faust primitives only, for two reasons.
///
/// AGENTS.md section 3 requires tests to be self-contained rather than lean on
/// an installed Faust. And `createCCraneliftDSPFactoryFromString` does not
/// handle the `import(...)` statement at all: any import fails with
/// "malformed definition node N", where N tracks the statement's position,
/// whether or not the named library exists and regardless of `-I`. The
/// `library(...)` form does work on that path, so this fixture could use it —
/// primitives keep the first reason satisfied too.
///
/// The envelope is a one-pole follower on the gate rather than `en.ar`: it
/// rises while the gate is held and decays exponentially after release, which
/// is what voice reclamation below the stop level needs to observe. Its time
/// constant is 500 samples, so a released voice crosses the -90 dB threshold
/// in roughly 5 200 samples.
const VOICE_DSP: &str = r#"
SR = fconstant(int fSamplingFreq, <math.h>);
fracp(x) = x - floor(x);
phasor(f) = (+(f / SR) : fracp) ~ _;
osc(f) = sin(6.283185307179586 * phasor(f));
freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.8, 0, 1, 0.001);
gate = button("gate");
level = hslider("level", 1, 0, 1, 0.001);
env = (gate * 0.002) : (+ ~ *(0.998));
process = osc(freq) * gain * level * env;
"#;

/// Peak absolute value across every channel.
fn peak(buffers: &[Vec<f64>]) -> f64 {
    buffers
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0_f64, |a, v| a.max(v.abs()))
}

/// Render `blocks` blocks of 64 frames and return the overall peak.
fn run_blocks(probe: &mut PolyProbe, blocks: usize) -> f64 {
    let mut top = 0.0_f64;
    for _ in 0..blocks {
        top = top.max(peak(&probe.compute(64)));
    }
    top
}

fn compile(nvoices: usize) -> PolyProbe {
    PolyProbe::compile_from_string(
        "poly_test",
        VOICE_DSP,
        &[],
        48_000,
        false,
        0,
        nvoices,
        None,
        cranelift_ffi::probe::poly::DEFAULT_VOICE_STOP_LEVEL,
    )
    .expect("poly compile")
}

#[test]
fn silent_until_a_key_is_pressed() {
    let mut probe = compile(4);
    assert_eq!(probe.active_voice_count(), 0);
    assert!(run_blocks(&mut probe, 8) < 1e-9);
}

#[test]
fn voices_sum_so_a_chord_is_louder_than_one_note() {
    // The point of a poly wrapper: independent voices mixed, not one voice
    // retriggered. Four distinct pitches must exceed a single note.
    let mut one = compile(4);
    one.key_on(60, 100);
    let single = run_blocks(&mut one, 40);

    let mut many = compile(4);
    for pitch in [60, 64, 67, 72] {
        many.key_on(pitch, 100);
    }
    let chord = run_blocks(&mut many, 40);

    assert!(single > 1e-4, "single note was silent: {single}");
    assert!(
        chord > single * 1.5,
        "chord {chord} not meaningfully louder than one note {single}"
    );
}

#[test]
fn note_on_allocates_and_note_off_eventually_frees() {
    let mut probe = compile(4);
    probe.key_on(60, 100);
    assert_eq!(probe.active_voice_count(), 1);

    // Let the attack settle, then release and render long enough for the
    // decay to fall below the stop level. Reclamation is observed from the
    // rendered level, so it cannot happen without rendering.
    run_blocks(&mut probe, 10);
    assert!(probe.key_off(60, false).is_some());
    run_blocks(&mut probe, 200);
    assert_eq!(
        probe.active_voice_count(),
        0,
        "voice never dropped below the stop level"
    );
}

#[test]
fn hard_note_off_frees_immediately() {
    let mut probe = compile(4);
    probe.key_on(60, 100);
    run_blocks(&mut probe, 4);
    probe.key_off(60, true);
    assert_eq!(probe.active_voice_count(), 0);
}

#[test]
fn note_off_for_an_unheld_pitch_is_a_no_op() {
    let mut probe = compile(2);
    assert!(probe.key_off(99, false).is_none());
    assert_eq!(probe.active_voice_count(), 0);
}

#[test]
fn more_notes_than_voices_steals_rather_than_dropping() {
    // Two voices, three notes: the third must still sound, by stealing.
    let mut probe = compile(2);
    probe.key_on(60, 100);
    probe.key_on(64, 100);
    let third = probe.key_on(67, 100);
    assert!(third < 2, "stolen voice index out of range: {third}");
    assert_eq!(probe.voice_count(), 2);
    assert!(run_blocks(&mut probe, 40) > 1e-4);
}

/// A voice with an input: the input while its gate is held.
const VOICE_WITH_INPUT: &str = r#"
freq = hslider("freq", 440, 20, 20000, 0.01);
gain = hslider("gain", 0.8, 0, 1, 0.001);
gate = button("gate");
process = _ * (gate + 0 * (freq + gain));
"#;

#[test]
fn every_voice_is_given_the_inputs_and_a_stolen_one_its_own_half_of_them() {
    // `mydsp_poly::compute` hands the host's inputs to every playing voice,
    // and `computeLegato` gives each half of a stolen voice's block its own
    // half of them (`computeSlice(slice, slice, inputs, outputs)`).
    let mut probe = PolyProbe::compile_from_string(
        "voice_with_input",
        VOICE_WITH_INPUT,
        &[],
        44_100,
        true,
        0,
        1,
        None,
        cranelift_ffi::probe::poly::DEFAULT_VOICE_STOP_LEVEL,
    )
    .expect("compile");
    assert_eq!(probe.inputs(), 1);
    let ramp: Vec<Vec<f64>> = vec![(0..64).map(f64::from).collect()];
    probe.key_on(60, 100);
    let block = probe.compute_with_inputs(&ramp, 64);
    assert_eq!(block[0], ramp[0], "the input, through the held voice");
    // and silence is what `compute` gives
    assert!(probe.compute(64)[0].iter().all(|v| *v == 0.0));

    // one voice, a second note: it is stolen. The outgoing half has its gate
    // down (silence here), the incoming half reads the ramp from 32 on
    probe.key_on(64, 100);
    let stolen = probe.compute_with_inputs(&ramp, 64);
    assert!(
        stolen[0][..32].iter().all(|v| *v == 0.0),
        "{:?}",
        &stolen[0][..32]
    );
    assert_eq!(stolen[0][32..], ramp[0][32..]);
}

#[test]
fn a_render_plays_its_notes_on_their_frames_and_returns_the_scalar_statistics() {
    // The render loop moved from the command line into the library, where
    // `Probe::render` lives, with its statistics: a note scheduled at frame
    // 100 sounds from frame 100, whatever the block size, and the window's
    // statistics say where the peak is.
    use cranelift_ffi::probe::engine::PolyRenderSpec;
    use cranelift_ffi::probe::schedule::{Schedule, parse_note};
    let mut schedule = Schedule::new();
    for (frame, event) in parse_note("69@100").unwrap() {
        schedule.push(frame, event);
    }
    let spec = PolyRenderSpec {
        frames: 2000,
        block: 64,
        input: cranelift_ffi::probe::render::InputMode::Zero,
        skip: 0,
        schedule,
        limit: None,
        time: false,
    };
    let mut probe = compile(2);
    let mut first_sound = None;
    let mut frames = 0;
    let stats = probe
        .render(&spec, |frame, samples| {
            frames += 1;
            if first_sound.is_none() && samples[0] != 0.0 {
                first_sound = Some(frame);
            }
        })
        .expect("render");
    assert_eq!(frames, 2000);
    // silence up to the note-on, sound from its very frame (the phasor's
    // first value is one increment, not zero): the block was cut at 100
    assert_eq!(first_sound, Some(100));
    assert_eq!(stats.window_len, 2000);
    assert!(stats.all_finite());
    let channel = &stats.channels[0];
    assert!(channel.peak > 1e-4 && channel.peak_at.is_some_and(|at| at >= 100));
    assert_eq!(probe.active_voice_count(), 1);
}

#[test]
fn a_reset_instrument_renders_what_a_fresh_one_renders() {
    // A sweep renders each point after `reset`: the voices still releasing,
    // their allocation dates and the controls written must all be gone.
    use cranelift_ffi::probe::engine::PolyRenderSpec;
    use cranelift_ffi::probe::schedule::{Schedule, parse_note};
    let mut schedule = Schedule::new();
    for note in ["60@0..300", "64@100..500", "67@200"] {
        for (frame, event) in parse_note(note).unwrap() {
            schedule.push(frame, event);
        }
    }
    let spec = PolyRenderSpec {
        frames: 1500,
        block: 64,
        input: cranelift_ffi::probe::render::InputMode::Zero,
        skip: 0,
        schedule,
        limit: None,
        time: false,
    };
    let render = |probe: &mut PolyProbe| {
        let mut samples = Vec::new();
        probe
            .render(&spec, |_, frame| samples.extend_from_slice(frame))
            .expect("render");
        samples
    };
    let fresh = render(&mut compile(2));
    let mut probe = compile(2);
    probe.set_all("level", 0.25).unwrap();
    let _ = render(&mut probe);
    assert!(probe.active_voice_count() > 0);
    probe.reset();
    assert_eq!(probe.active_voice_count(), 0);
    let again = render(&mut probe);
    assert_eq!(fresh.len(), again.len());
    assert!(
        fresh
            .iter()
            .zip(&again)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "a reset instrument renders other samples"
    );
}

#[test]
fn a_broadcast_write_is_a_widgets_and_stays_in_its_range() {
    // `level` goes from 0 to 1. A voice used to take whatever it was given,
    // while the effect clamped: 7 is now 1 on a voice too, as `Probe::set`
    // has it, and the caller is told beforehand what would be written.
    use cranelift_ffi::probe::engine::PolyTarget;
    let mut at_the_bound = compile(2);
    let mut beyond = compile(2);
    {
        let writes = beyond.check_write("level", 7.0).expect("level resolves");
        assert_eq!(writes.len(), 1, "no effect here: the voices alone");
        assert_eq!(writes[0].target, PolyTarget::Voices);
        assert!(!writes[0].write.in_range());
        assert!((writes[0].write.applied - 1.0).abs() < f64::EPSILON);
        assert!(
            beyond.check_write("level", 0.25).unwrap()[0]
                .write
                .in_range()
        );
        assert!(beyond.check_write("nope", 1.0).is_err());
    }
    at_the_bound.set_all("level", 1.0).expect("level");
    beyond.set_all("level", 7.0).expect("level");
    at_the_bound.key_on(69, 100);
    beyond.key_on(69, 100);
    let (a, b) = (
        run_blocks(&mut at_the_bound, 20),
        run_blocks(&mut beyond, 20),
    );
    assert!(a > 1e-4, "the voice sounds: {a}");
    assert!((a - b).abs() < 1e-15, "7 was written as 7: {a} against {b}");
}

#[test]
fn broadcasting_a_control_reaches_every_voice() {
    // set_all is how a caller configures a patch across voices; if it only
    // reached voice 0 a chord would be uneven and nothing else would say so.
    //
    // Broadcast `level`, not `gain`: `gain` is one of the voice controls, so
    // key_on overwrites it from the note velocity — correctly, and that is
    // exactly what a patch parameter must not be confused with.
    let mut probe = compile(3);
    probe
        .set_all("level", 0.0)
        .expect("level exists on a voice");
    for pitch in [60, 64, 67] {
        probe.key_on(pitch, 100);
    }
    assert!(
        run_blocks(&mut probe, 20) < 1e-9,
        "a voice kept sounding after level was broadcast to zero"
    );
}

/// Compile `source` written to a file, as the command line does: the inline
/// `effect` is extracted from the file (`PolyProbe::compile`), never from a
/// string.
fn compile_file(tag: &str, source: &str) -> PolyProbe {
    let path =
        std::env::temp_dir().join(format!("faustprobe_poly_{tag}_{}.dsp", std::process::id()));
    std::fs::write(&path, source).expect("write the program");
    let probe = PolyProbe::compile(
        path.to_str().expect("a UTF-8 path"),
        &[],
        44_100,
        true,
        0,
        2,
        None,
        cranelift_ffi::probe::poly::DEFAULT_VOICE_STOP_LEVEL,
    );
    let _ = std::fs::remove_file(&path);
    probe.expect("poly compile")
}

/// A constant voice, its gated gain split into `channels`, and the declared
/// `effect`.
fn constant_voice(channels: &str, effect: &str) -> String {
    format!(
        "gain = hslider(\"gain\", 0.5, 0, 1, 0.01);\n\
         gate = button(\"gate\");\n\
         process = gate * gain <: {channels};\n\
         effect = {effect};\n"
    )
}

#[test]
fn an_effect_that_changes_the_channel_count_gives_the_instrument_its_outputs() {
    // The voices are mixed on a bus of their own width and the effect makes
    // the instrument's: this used to size the render on the voices and index
    // past the effect's single output.
    let mut probe = compile_file("mixdown", &constant_voice("_, *(0.5)", "+ : *(2.0)"));
    assert!(probe.has_effect() && probe.warnings().is_empty());
    assert_eq!((probe.bus_outputs(), probe.outputs()), (2, 1));
    probe.key_on(60, 127);
    let block = probe.compute(16);
    assert_eq!(block.len(), 1);
    assert!(
        block[0].iter().all(|v| (v - 3.0).abs() < 1e-12),
        "{block:?}"
    );

    // the render and its statistics have the instrument's width
    use cranelift_ffi::probe::engine::PolyRenderSpec;
    let spec = PolyRenderSpec {
        frames: 64,
        block: 16,
        input: cranelift_ffi::probe::render::InputMode::Zero,
        skip: 0,
        schedule: cranelift_ffi::probe::schedule::Schedule::new(),
        limit: None,
        time: false,
    };
    let mut widths = Vec::new();
    let stats = probe
        .render(&spec, |_, samples| widths.push(samples.len()))
        .expect("render");
    assert_eq!(stats.channels.len(), 1);
    assert!(widths.len() == 64 && widths.iter().all(|w| *w == 1));
}

#[test]
fn an_inline_effect_goes_through_the_adaptor_and_one_it_refuses_is_left_out_with_a_warning() {
    // `getEffectCode`'s adaptor splits the voices' channels into the effect's
    // inputs when there are at least as many, and merges them otherwise:
    // three channels pass through (a piano's regions into its body) ...
    let body = compile_file(
        "body",
        &constant_voice("_, *(0.5), *(0.25)", "_, _, _ :> _ <: _, *(0.5)"),
    );
    assert!(body.has_effect() && body.warnings().is_empty());
    assert_eq!((body.bus_outputs(), body.outputs()), (3, 2));

    // ... one is split in two ...
    let mut widen = compile_file("widen", &constant_voice("_", "*(0.25), *(0.75)"));
    assert!(widen.has_effect() && widen.warnings().is_empty());
    assert_eq!((widen.bus_outputs(), widen.outputs()), (1, 2));
    widen.key_on(60, 127);
    let block = widen.compute(4);
    assert!((block[0][0] - 0.25).abs() < 1e-12 && (block[1][0] - 0.75).abs() < 1e-12);

    // ... and four are merged into two, channel i into input i mod 2
    let mut merge = compile_file(
        "merge",
        &constant_voice("_, *(0.5), *(0.25), *(0.125)", "*(1), *(10)"),
    );
    assert!(merge.has_effect() && merge.warnings().is_empty());
    assert_eq!((merge.bus_outputs(), merge.outputs()), (4, 2));
    merge.key_on(60, 127);
    let block = merge.compute(4);
    assert!((block[0][0] - 1.25).abs() < 1e-12, "{block:?}");
    assert!((block[1][0] - 6.25).abs() < 1e-12, "{block:?}");

    // 3 -> 2 inputs: neither count is a multiple of the other, the adaptor
    // refuses them, and the render that follows is not the declared
    // instrument, which the caller is told
    let mismatch = compile_file(
        "mismatch",
        &constant_voice("_, *(0.5), *(0.25)", "_, _ :> _"),
    );
    assert!(!mismatch.has_effect());
    assert_eq!((mismatch.bus_outputs(), mismatch.outputs()), (3, 3));
    assert_eq!(mismatch.warnings().len(), 1);
    assert!(
        mismatch.warnings()[0].contains("takes 2 input(s) but a voice has 3 output(s)"),
        "{:?}",
        mismatch.warnings()
    );

    // no `effect` declared: none, and nothing to say
    let dry = compile_file("dry", "gate = button(\"gate\");\nprocess = gate <: _, _;\n");
    assert!(!dry.has_effect() && dry.warnings().is_empty());
    assert_eq!(dry.outputs(), 2);
}
