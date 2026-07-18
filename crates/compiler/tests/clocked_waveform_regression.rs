//! Regression: a stateful `waveform` generator inside an `ondemand` block must
//! advance its read index **in fire time**, not every sample.
//!
//! From `examples/ondemand/gamme.dsp`: a pulse clock drives a scale player
//! `ondemand(waveform{60,62,64,65,67,69,71,72}:!,_) : ba.midikey2hz`. The
//! waveform's `iWave*` index counter is fire-gated state (like the per-domain
//! delay/IOTA counters); emitting its advance at the top rate makes the index
//! race ahead ~20000× between fires and plays a garbled sequence instead of a
//! C-major scale. The C++ reference (`8eebea429`) emits the index advance
//! inside the guarded `if`. This is covered by the general rule that a held
//! `Clocked` payload lowers in the consuming guarded block, even when clock
//! inference gives the payload an ancestor environment.
//!
//! Needs `analyzers`/`basics`/`stdfaust` from faustlibraries; skips gracefully
//! when unavailable (`FAUST_RS_FAUSTLIBRARIES_ROOT`, else the default path).

use std::io::Cursor;
use std::path::PathBuf;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

const DEFAULT_FAUSTLIBRARIES_ROOT: &str = "/Users/letz/Developpements/faustlibraries";

fn faustlibraries_root() -> Option<PathBuf> {
    std::env::var_os("FAUST_RS_FAUSTLIBRARIES_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            let default = PathBuf::from(DEFAULT_FAUSTLIBRARIES_ROOT);
            default.exists().then_some(default)
        })
}

/// `440 · 2^((key − 69)/12)` — `ba.midikey2hz`.
fn midikey2hz(key: f32) -> f32 {
    440.0 * 2.0_f32.powf((key - 69.0) / 12.0)
}

#[test]
fn ondemand_waveform_plays_the_scale_in_fire_time() {
    let Some(libs) = faustlibraries_root() else {
        eprintln!("Skipping gamme regression: faustlibraries unavailable");
        return;
    };
    let path = std::env::temp_dir().join(format!("faust-rs-gamme-{}.dsp", std::process::id()));
    std::fs::write(
        &path,
        "import(\"stdfaust.lib\");\n\
         process = ba.pulsen(1, 20000) : \
           ondemand(waveform{60, 62, 64, 65, 67, 69, 71, 72}:!,_) : ba.midikey2hz;\n",
    )
    .expect("write dsp");
    let fbc = Compiler::new()
        .compile_file_to_interp_with_lane(
            &path,
            &[libs],
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("gamme compilation failed: {e}"));
    let _ = std::fs::remove_file(&path);

    let period = 20_000usize;
    let notes = [60.0, 62.0, 64.0, 65.0, 67.0, 69.0, 71.0, 72.0];
    let nframes = period * (notes.len() + 1); // one full scale + wrap

    let mut reader = Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader).expect("fbc parse");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let mut out = vec![0.0_f32; nframes];
    let mut slices = vec![out.as_mut_slice()];
    instance
        .try_compute(nframes as i32, &[], &mut slices)
        .expect("gamme execution");

    // Each pulse fires at the start of a period and holds until the next.
    // The k-th fire must emit the k-th scale note (wrapping after 8).
    for k in 0..notes.len() + 1 {
        let sample_mid = k * period + period / 2;
        let expected = midikey2hz(notes[k % notes.len()]);
        let got = out[sample_mid];
        assert!(
            (got - expected).abs() < 1.0e-2,
            "fire {k}: expected note {} ({expected:.2} Hz), got {got:.2} Hz \
             (the waveform index must advance in fire time, not every sample)",
            notes[k % notes.len()]
        );
    }
}
