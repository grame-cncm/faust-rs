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
