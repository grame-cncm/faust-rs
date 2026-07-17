// DX7 E.PIANO 1 demo for the faust-rs Rust backend.
//
// `src/dx7.rs` is generated from `dsp/dx7_alg5.dsp` (dx.algorithm(5), the
// DX7 E.Piano algorithm) with:
//
//   faust-rs -lang rust -cn Dx7Piano -I /opt/homebrew/share/faust \
//       dsp/dx7_alg5.dsp -o src/dx7.rs
//
// This host program:
//   1. instantiates the generated DSP and calls the Faust lifecycle (`init`),
//   2. programs the E.PIANO 1 (ROM1A) patch by driving the generated
//      `build_user_interface` with a path-matching UI visitor,
//   3. plays the note C5 (523.25 Hz) — gate on for 1.5 s, then release,
//   4. writes the result to `dx7-piano-c5.wav` (stereo 16-bit PCM, 44.1 kHz).

include!("dx7.rs");

// ---------------------------------------------------------------------------
// Host runtime traits expected by the generated code (same vocabulary as the
// faust-rs impulse-test architecture).
// ---------------------------------------------------------------------------

pub trait Meta {
    fn declare(&mut self, key: &str, value: &str);
}

#[allow(unused_variables)]
pub trait UI<T> {
    fn open_tab_box(&mut self, label: &str) {}
    fn open_horizontal_box(&mut self, label: &str) {}
    fn open_vertical_box(&mut self, label: &str) {}
    fn close_box(&mut self) {}
    fn add_button(&mut self, label: &str, zone: &mut T) {}
    fn add_check_button(&mut self, label: &str, zone: &mut T) {}
    fn add_horizontal_slider(
        &mut self,
        label: &str,
        zone: &mut T,
        init: T,
        min: T,
        max: T,
        step: T,
    ) {
    }
    fn add_vertical_slider(&mut self, label: &str, zone: &mut T, init: T, min: T, max: T, step: T) {
    }
    fn add_num_entry(&mut self, label: &str, zone: &mut T, init: T, min: T, max: T, step: T) {}
    fn add_horizontal_bargraph(&mut self, label: &str, zone: &mut T, min: T, max: T) {}
    fn add_vertical_bargraph(&mut self, label: &str, zone: &mut T, min: T, max: T) {}
    fn add_soundfile(&mut self, label: &str, url: &str, sf: &mut Soundfile) {}
    fn declare(&mut self, zone: Option<&mut T>, key: &str, value: &str) {}
}

/// Unused by this DSP; present because the UI trait mentions it.
#[allow(non_snake_case)]
#[derive(Default)]
pub struct Soundfile {
    pub fBuffers: Vec<Vec<FaustFloat>>,
    pub fLength: Vec<i32>,
    pub fSR: Vec<i32>,
    pub fOffset: Vec<i32>,
}

// ---------------------------------------------------------------------------
// Path-matching parameter setter: tracks the open/close box stack so
// identically-labeled widgets ("L1" in every operator) are addressed by their
// full path, e.g. "DX7/Operator 2/Amp Env Generator/Rates/R1".
// ---------------------------------------------------------------------------

struct SetParams<'a> {
    path: Vec<String>,
    values: &'a [(&'static str, FaustFloat)],
}

impl<'a> SetParams<'a> {
    fn new(values: &'a [(&'static str, FaustFloat)]) -> Self {
        Self {
            path: Vec::new(),
            values,
        }
    }

    fn apply(&mut self, label: &str, zone: &mut FaustFloat) {
        let full = if self.path.is_empty() {
            label.to_owned()
        } else {
            format!("{}/{}", self.path.join("/"), label)
        };
        if let Some((_, value)) = self.values.iter().find(|(path, _)| *path == full) {
            *zone = *value;
        }
    }
}

impl UI<FaustFloat> for SetParams<'_> {
    fn open_tab_box(&mut self, label: &str) {
        self.path.push(label.to_owned());
    }
    fn open_horizontal_box(&mut self, label: &str) {
        self.path.push(label.to_owned());
    }
    fn open_vertical_box(&mut self, label: &str) {
        self.path.push(label.to_owned());
    }
    fn close_box(&mut self) {
        self.path.pop();
    }
    fn add_button(&mut self, label: &str, zone: &mut FaustFloat) {
        self.apply(label, zone);
    }
    fn add_check_button(&mut self, label: &str, zone: &mut FaustFloat) {
        self.apply(label, zone);
    }
    fn add_horizontal_slider(
        &mut self,
        label: &str,
        zone: &mut FaustFloat,
        _init: FaustFloat,
        _min: FaustFloat,
        _max: FaustFloat,
        _step: FaustFloat,
    ) {
        self.apply(label, zone);
    }
    fn add_vertical_slider(
        &mut self,
        label: &str,
        zone: &mut FaustFloat,
        _init: FaustFloat,
        _min: FaustFloat,
        _max: FaustFloat,
        _step: FaustFloat,
    ) {
        self.apply(label, zone);
    }
    fn add_num_entry(
        &mut self,
        label: &str,
        zone: &mut FaustFloat,
        _init: FaustFloat,
        _min: FaustFloat,
        _max: FaustFloat,
        _step: FaustFloat,
    ) {
        self.apply(label, zone);
    }
}

// ---------------------------------------------------------------------------
// E.PIANO 1 (DX7 ROM1A) — Algorithm 5, native DX7 parameter values.
// Source: examples/dx7/dx7-sequence.js (javascriptmusic), channel 0.
// Parameters left at their Faust defaults (Pitch EG, LFO, breakpoints) match
// the patch already.
// ---------------------------------------------------------------------------

const C5_HZ: FaustFloat = 523.25;

const EPIANO1: &[(&str, FaustFloat)] = &[
    ("DX7/freq", C5_HZ),
    ("DX7/gain", 0.8),
    // Global
    ("DX7/Global/Main/Feedback", 6.0),
    ("DX7/Global/Main/Transpose", 0.0),
    ("DX7/Global/Main/Osc Key Sync", 1.0),
    // Op1 — carrier, tine fundamental (1:1)
    ("DX7/Operator 1/Tone/Tune", 3.0),
    ("DX7/Operator 1/Tone/Coarse", 1.0),
    ("DX7/Operator 1/Amp Env Generator/Levels/L1", 99.0),
    ("DX7/Operator 1/Amp Env Generator/Levels/L2", 75.0),
    ("DX7/Operator 1/Amp Env Generator/Levels/L3", 0.0),
    ("DX7/Operator 1/Amp Env Generator/Levels/L4", 0.0),
    ("DX7/Operator 1/Amp Env Generator/Rates/R1", 96.0),
    ("DX7/Operator 1/Amp Env Generator/Rates/R2", 25.0),
    ("DX7/Operator 1/Amp Env Generator/Rates/R3", 25.0),
    ("DX7/Operator 1/Amp Env Generator/Rates/R4", 67.0),
    ("DX7/Operator 1/Level/Level", 99.0),
    ("DX7/Operator 1/Level/Key Vel", 2.0),
    ("DX7/Operator 1/Level/Rate Scaling", 3.0),
    // Op2 — modulator of Op1, bell "ding" (14:1)
    ("DX7/Operator 2/Tone/Tune", 3.0),
    ("DX7/Operator 2/Tone/Coarse", 14.0),
    ("DX7/Operator 2/Amp Env Generator/Levels/L1", 99.0),
    ("DX7/Operator 2/Amp Env Generator/Levels/L2", 75.0),
    ("DX7/Operator 2/Amp Env Generator/Levels/L3", 0.0),
    ("DX7/Operator 2/Amp Env Generator/Levels/L4", 0.0),
    ("DX7/Operator 2/Amp Env Generator/Rates/R1", 95.0),
    ("DX7/Operator 2/Amp Env Generator/Rates/R2", 50.0),
    ("DX7/Operator 2/Amp Env Generator/Rates/R3", 35.0),
    ("DX7/Operator 2/Amp Env Generator/Rates/R4", 78.0),
    ("DX7/Operator 2/Level/Level", 82.0),
    ("DX7/Operator 2/Level/Key Vel", 4.0),
    ("DX7/Operator 2/Level/Rate Scaling", 3.0),
    // Op3 — carrier, warm body (1:1)
    ("DX7/Operator 3/Tone/Tune", 0.0),
    ("DX7/Operator 3/Tone/Coarse", 1.0),
    ("DX7/Operator 3/Amp Env Generator/Levels/L1", 99.0),
    ("DX7/Operator 3/Amp Env Generator/Levels/L2", 95.0),
    ("DX7/Operator 3/Amp Env Generator/Levels/L3", 0.0),
    ("DX7/Operator 3/Amp Env Generator/Levels/L4", 0.0),
    ("DX7/Operator 3/Amp Env Generator/Rates/R1", 95.0),
    ("DX7/Operator 3/Amp Env Generator/Rates/R2", 20.0),
    ("DX7/Operator 3/Amp Env Generator/Rates/R3", 20.0),
    ("DX7/Operator 3/Amp Env Generator/Rates/R4", 50.0),
    ("DX7/Operator 3/Level/Level", 86.0),
    ("DX7/Operator 3/Level/Key Vel", 0.0),
    ("DX7/Operator 3/Level/Rate Scaling", 1.0),
    // Op4 — modulator of Op3, harmonic richness (1:1)
    ("DX7/Operator 4/Tone/Tune", 0.0),
    ("DX7/Operator 4/Tone/Coarse", 1.0),
    ("DX7/Operator 4/Amp Env Generator/Levels/L1", 99.0),
    ("DX7/Operator 4/Amp Env Generator/Levels/L2", 58.0),
    ("DX7/Operator 4/Amp Env Generator/Levels/L3", 0.0),
    ("DX7/Operator 4/Amp Env Generator/Levels/L4", 0.0),
    ("DX7/Operator 4/Amp Env Generator/Rates/R1", 95.0),
    ("DX7/Operator 4/Amp Env Generator/Rates/R2", 29.0),
    ("DX7/Operator 4/Amp Env Generator/Rates/R3", 20.0),
    ("DX7/Operator 4/Amp Env Generator/Rates/R4", 50.0),
    ("DX7/Operator 4/Level/Level", 86.0),
    ("DX7/Operator 4/Level/Key Vel", 4.0),
    ("DX7/Operator 4/Level/Rate Scaling", 1.0),
    // Op5 — modulator, transient excitation
    ("DX7/Operator 5/Tone/Coarse", 1.0),
    ("DX7/Operator 5/Amp Env Generator/Levels/L1", 99.0),
    ("DX7/Operator 5/Amp Env Generator/Levels/L2", 0.0),
    ("DX7/Operator 5/Amp Env Generator/Levels/L3", 0.0),
    ("DX7/Operator 5/Amp Env Generator/Levels/L4", 0.0),
    ("DX7/Operator 5/Amp Env Generator/Rates/R1", 99.0),
    ("DX7/Operator 5/Amp Env Generator/Rates/R2", 95.0),
    ("DX7/Operator 5/Amp Env Generator/Rates/R3", 0.0),
    ("DX7/Operator 5/Amp Env Generator/Rates/R4", 0.0),
    ("DX7/Operator 5/Level/Level", 86.0),
    ("DX7/Operator 5/Level/Rate Scaling", 0.0),
    // Op6 — feedback modulator, attack noise
    ("DX7/Operator 6/Tone/Coarse", 1.0),
    ("DX7/Operator 6/Amp Env Generator/Levels/L1", 99.0),
    ("DX7/Operator 6/Amp Env Generator/Levels/L2", 0.0),
    ("DX7/Operator 6/Amp Env Generator/Levels/L3", 0.0),
    ("DX7/Operator 6/Amp Env Generator/Levels/L4", 0.0),
    ("DX7/Operator 6/Amp Env Generator/Rates/R1", 99.0),
    ("DX7/Operator 6/Amp Env Generator/Rates/R2", 95.0),
    ("DX7/Operator 6/Amp Env Generator/Rates/R3", 0.0),
    ("DX7/Operator 6/Amp Env Generator/Rates/R4", 0.0),
    ("DX7/Operator 6/Level/Level", 86.0),
];

// ---------------------------------------------------------------------------
// Rendering + WAV output
// ---------------------------------------------------------------------------

const SAMPLE_RATE: usize = 44100;
const BLOCK_SIZE: usize = 64;
const NOTE_SECONDS: f64 = 1.5;
const TOTAL_SECONDS: f64 = 4.0;

fn main() {
    let mut dsp = Dx7Piano::new();
    dsp.init(SAMPLE_RATE as i32);
    dsp.build_user_interface(&mut SetParams::new(EPIANO1));

    let num_outputs = dsp.get_num_outputs() as usize;
    let total_frames = (TOTAL_SECONDS * SAMPLE_RATE as f64) as usize;
    let gate_off_frame = (NOTE_SECONDS * SAMPLE_RATE as f64) as usize;

    let mut rendered: Vec<Vec<FaustFloat>> = vec![Vec::with_capacity(total_frames); num_outputs];
    let mut out_bufs = vec![[0.0 as FaustFloat; BLOCK_SIZE]; num_outputs];
    let in_bufs: Vec<&[FaustFloat]> = Vec::new(); // 0 audio inputs

    let mut written = 0usize;
    while written < total_frames {
        let n = BLOCK_SIZE.min(total_frames - written);
        let gate = if written < gate_off_frame { 1.0 } else { 0.0 };
        dsp.build_user_interface(&mut SetParams::new(&[("DX7/gate", gate)]));
        {
            let mut outputs: Vec<&mut [FaustFloat]> =
                out_bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
            dsp.compute(n as i32, &in_bufs, &mut outputs);
        }
        for (channel, buf) in out_bufs.iter().enumerate() {
            rendered[channel].extend_from_slice(&buf[..n]);
        }
        written += n;
    }

    let peak = rendered
        .iter()
        .flat_map(|channel| channel.iter())
        .fold(0.0f32, |acc, v| acc.max(v.abs()));
    println!("rendered {total_frames} frames x {num_outputs} channels, peak = {peak:.4}");
    assert!(peak > 0.001, "DSP produced silence — something is wrong");
    assert!(peak.is_finite(), "DSP produced non-finite output");

    // Normalize to -1 dBFS so the 16-bit render never clips.
    let scale = 0.891 / peak;
    for channel in rendered.iter_mut() {
        for sample in channel.iter_mut() {
            *sample *= scale;
        }
    }

    let path = "dx7-piano-c5.wav";
    write_wav_16bit(path, SAMPLE_RATE as u32, &rendered);
    println!("wrote {path} ({TOTAL_SECONDS} s, note C5 = {C5_HZ} Hz)");
}

/// Minimal 16-bit PCM WAV writer (interleaved, no dependencies).
fn write_wav_16bit(path: &str, sample_rate: u32, channels: &[Vec<FaustFloat>]) {
    let num_channels = channels.len() as u32;
    let num_frames = channels.first().map_or(0, |c| c.len()) as u32;
    let bytes_per_sample = 2u32;
    let data_len = num_frames * num_channels * bytes_per_sample;
    let byte_rate = sample_rate * num_channels * bytes_per_sample;
    let block_align = (num_channels * bytes_per_sample) as u16;

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&(num_channels as u16).to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..num_frames as usize {
        for channel in channels {
            let sample = (channel[frame].clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&sample.to_le_bytes());
        }
    }
    std::fs::write(path, out).expect("failed to write WAV file");
}
