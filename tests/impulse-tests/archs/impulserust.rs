// ---------------------------------------------------------------------------
// impulserust.rs — impulse-test architecture for the faust-rs Rust backend.
//
// Prepended to the generated `-lang rust -double -cn mydsp` output.
// Mirrors the scalar impulse pass of `tools/impulsewasm.js`, which itself
// mirrors the C++ 4-pass impulse architecture's first pass:
//   - 44.1 kHz sample rate, blocks of 64 frames;
//   - a 1.0 impulse on the first frame of every input channel;
//   - buttons (only buttons, like `FUI::setButtons`) held at 1.0 during the
//     first block, then 0.0;
//   - the shared sinusoidal soundfile fixture (2 channels, 4096-frame parts,
//     `sin(part + 2*pi*i/4096)`), installed after `init`;
//   - output lines `%6d :  %.6f ...` with |v| < 1e-6 normalized to 0.
// ---------------------------------------------------------------------------

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_parens,
    unused_variables
)]

pub type F32 = f64;
pub type F64 = f64;
pub type FaustFloat = F64;

use std::rc::Rc;

unsafe extern "C" {
    #[link_name = "remainder"]
    fn c_remainder(x: f64, y: f64) -> f64;
    #[link_name = "remainderf"]
    fn c_remainderf(x: f32, y: f32) -> f32;
}

fn remainder(x: f64, y: f64) -> f64 { unsafe { c_remainder(x, y) } }
fn remainderf(x: f32, y: f32) -> f32 { unsafe { c_remainderf(x, y) } }

#[derive(Clone, Copy)]
pub struct ParamIndex(pub i32);

pub trait Meta {
    fn declare(&mut self, key: &str, value: &str);
}

#[allow(unused_variables)]
pub trait UI<T> {
    fn open_tab_box(&mut self, label: &str) {}
    fn open_horizontal_box(&mut self, label: &str) {}
    fn open_vertical_box(&mut self, label: &str) {}
    fn close_box(&mut self) {}
    fn add_button(&mut self, label: &str, zone: ParamIndex) {}
    fn add_check_button(&mut self, label: &str, zone: ParamIndex) {}
    fn add_horizontal_slider(&mut self, label: &str, zone: ParamIndex, init: T, min: T, max: T, step: T) {
    }
    fn add_vertical_slider(&mut self, label: &str, zone: ParamIndex, init: T, min: T, max: T, step: T) {}
    fn add_num_entry(&mut self, label: &str, zone: ParamIndex, init: T, min: T, max: T, step: T) {}
    fn add_horizontal_bargraph(&mut self, label: &str, zone: ParamIndex, min: T, max: T) {}
    fn add_vertical_bargraph(&mut self, label: &str, zone: ParamIndex, min: T, max: T) {}
    /// faust-rs extension: C++ Rust architectures do not currently expose
    /// soundfile widgets, but the impulse architecture supplies their fixture
    /// through the generated `Soundfile::default` state value.
    fn add_soundfile(&mut self, label: &str, url: &str, param: ParamIndex) {}
    fn declare(&mut self, zone: Option<ParamIndex>, key: &str, value: &str) {}
}

pub trait FaustDsp {
    type T;
    fn new() -> Self where Self: Sized;
    fn metadata(&self, m: &mut dyn Meta);
    fn get_sample_rate(&self) -> i32;
    fn get_num_inputs(&self) -> i32;
    fn get_num_outputs(&self) -> i32;
    fn class_init(sample_rate: i32) where Self: Sized;
    fn instance_reset_params(&mut self);
    fn instance_clear(&mut self);
    fn instance_constants(&mut self, sample_rate: i32);
    fn instance_init(&mut self, sample_rate: i32);
    fn init(&mut self, sample_rate: i32);
    fn build_user_interface(&self, ui: &mut dyn UI<Self::T>);
    fn build_user_interface_static(ui: &mut dyn UI<Self::T>) where Self: Sized;
    fn get_param(&self, param: ParamIndex) -> Option<Self::T>;
    fn set_param(&mut self, param: ParamIndex, value: Self::T);
    fn compute(&mut self, count: i32, inputs: &[&[Self::T]], outputs: &mut [&mut [Self::T]]);
}

const SAMPLE_RATE: i32 = 44100;
const BLOCK_SIZE: usize = 64;
const DEFAULT_FRAMES: usize = 15000;
const SOUND_CHAN: usize = 2;
const SOUND_LENGTH: usize = 4096;
const SOUND_SR: i32 = 44100;
const SOUND_BUFFER_SIZE: usize = 1024;
const MAX_CHAN: usize = 64;
const MAX_SOUNDFILE_PARTS: usize = 256;

/// Host soundfile container matching the field vocabulary emitted by the Rust
/// backend (`fBuffers`/`fLength`/`fSR`/`fOffset`, C++ `Soundfile` names).
///
/// Channel buffers are `Rc`-shared so the 64-entry channel table can alias the
/// two real fixture channels without copying, like the pointer table used by
/// the WASM runner.
#[allow(non_snake_case)]
pub struct Soundfile {
    pub fBuffers: Vec<Rc<Vec<FaustFloat>>>,
    pub fLength: Vec<i32>,
    pub fSR: Vec<i32>,
    pub fOffset: Vec<i32>,
}

impl Default for Soundfile {
    fn default() -> Self {
        make_soundfile(1)
    }
}

/// Counts soundfile parts from a Faust URL literal (`{'a.wav';'b.wav'}`),
/// mirroring `soundfilePartCount` in `tools/impulsewasm.js`.
fn soundfile_part_count(url: &str) -> usize {
    let trimmed = url.trim();
    let Some(open) = trimmed.find('{') else {
        return 1;
    };
    let Some(close) = trimmed[open + 1..].find('}') else {
        return 1;
    };
    let body = &trimmed[open + 1..open + 1 + close];
    let count = body
        .split(';')
        .filter(|part| !part.trim().trim_matches('\'').is_empty())
        .count();
    count.max(1)
}

/// Builds the shared sinusoidal fixture: `real_parts` parts of 4096 frames of
/// `sin(part + 2*pi*i/4096)` on both channels, then empty 1024-frame parts up
/// to the 256-part table, all at 44.1 kHz.
fn make_soundfile(real_parts: usize) -> Soundfile {
    let real_parts = real_parts.min(MAX_SOUNDFILE_PARTS);
    let mut offsets = Vec::with_capacity(MAX_SOUNDFILE_PARTS);
    let mut lengths = Vec::with_capacity(MAX_SOUNDFILE_PARTS);
    let mut total_frames = 0usize;
    for _ in 0..real_parts {
        offsets.push(total_frames as i32);
        lengths.push(SOUND_LENGTH as i32);
        total_frames += SOUND_LENGTH;
    }
    for _ in real_parts..MAX_SOUNDFILE_PARTS {
        offsets.push(total_frames as i32);
        lengths.push(SOUND_BUFFER_SIZE as i32);
        total_frames += SOUND_BUFFER_SIZE;
    }

    let mut channels: Vec<Vec<FaustFloat>> = vec![vec![0.0 as FaustFloat; total_frames]; SOUND_CHAN];
    for part in 0..real_parts {
        let offset = part * SOUND_LENGTH;
        for sample in 0..SOUND_LENGTH {
            let value = (part as f64 + (2.0 * std::f64::consts::PI * sample as f64) / SOUND_LENGTH as f64)
                .sin();
            for channel in channels.iter_mut() {
                channel[offset + sample] = value as FaustFloat;
            }
        }
    }
    let shared: Vec<Rc<Vec<FaustFloat>>> = channels.into_iter().map(Rc::new).collect();
    let buffers = (0..MAX_CHAN)
        .map(|chan| Rc::clone(&shared[chan % SOUND_CHAN]))
        .collect();

    Soundfile {
        fBuffers: buffers,
        fLength: lengths,
        fSR: vec![SOUND_SR; MAX_SOUNDFILE_PARTS],
        fOffset: offsets,
    }
}

/// UI visitor that drives button zones only, like the C++ `FUI::setButtons`.
struct SetButtons {
    value: FaustFloat,
    params: Vec<ParamIndex>,
}

impl UI<FaustFloat> for SetButtons {
    fn add_button(&mut self, _label: &str, param: ParamIndex) {
        self.params.push(param);
    }
}

/// Clamps denormal-level output values exactly like the Node runners.
fn normalize(value: FaustFloat) -> FaustFloat {
    if (value as f64).abs() < 1e-6 { 0.0 } else { value }
}

fn main() {
    // Large DSP structs (long delay lines) are constructed by value; run the
    // harness on a thread with a generous stack like the C++ harness's heap
    // allocation headroom.
    std::thread::Builder::new()
        .stack_size(1 << 29)
        .spawn(run)
        .expect("failed to spawn impulse harness thread")
        .join()
        .expect("impulse harness thread panicked");
}

fn run() {
    let mut frames = DEFAULT_FRAMES;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-n" {
            let value = args.next().expect("-n requires a frame count");
            frames = value.parse().expect("invalid frame count");
        }
    }

    let mut dsp = mydsp::new();
    dsp.init(SAMPLE_RATE);
    let num_inputs = dsp.get_num_inputs() as usize;
    let num_outputs = dsp.get_num_outputs() as usize;

    let mut out = String::new();
    out.push_str(&format!("number_of_inputs  : {num_inputs:>3}\n"));
    out.push_str(&format!("number_of_outputs : {num_outputs:>3}\n"));
    out.push_str(&format!("number_of_frames  : {frames:>6}\n"));

    let mut in_bufs = vec![[0.0 as FaustFloat; BLOCK_SIZE]; num_inputs];
    let mut out_bufs = vec![[0.0 as FaustFloat; BLOCK_SIZE]; num_outputs];

    let mut written = 0usize;
    let mut cycle = 0usize;
    while written < frames {
        let n = BLOCK_SIZE.min(frames - written);
        for buf in in_bufs.iter_mut() {
            buf.fill(0.0);
        }
        for buf in out_bufs.iter_mut() {
            buf.fill(0.0);
        }
        if written == 0 {
            for buf in in_bufs.iter_mut() {
                buf[0] = 1.0;
            }
        }
        let button = if cycle == 0 { 1.0 } else { 0.0 };
        let mut buttons = SetButtons { value: button, params: Vec::new() };
        dsp.build_user_interface(&mut buttons);
        for param in buttons.params {
            dsp.set_param(param, button);
        }
        // Which entry points to drive depends on the execution options the DSP
        // was compiled with. Selected by `--cfg` so one architecture can host
        // all four shapes; the emitted impulse response must be identical in
        // each, which is exactly what the `rust-ec` / `rust-os` / `rust-ec-os`
        // targets check against the same reference as the classic target.
        //
        // Under `-ec` the block-rate work lives in `control()`; skipping it
        // reads uninitialized slow values. Under `-os` the canonical `compute`
        // is deliberately empty and the sample body is in `frame()`; calling
        // only `compute` would emit silence.
        #[cfg(impulse_ec)]
        dsp.control();
        #[cfg(impulse_os)]
        {
            let mut frame_in = vec![0.0 as FaustFloat; num_inputs.max(1)];
            let mut frame_out = vec![0.0 as FaustFloat; num_outputs.max(1)];
            for frame in 0..n {
                for (channel, buf) in in_bufs.iter().enumerate() {
                    frame_in[channel] = buf[frame];
                }
                dsp.frame(&frame_in, &mut frame_out);
                for (channel, buf) in out_bufs.iter_mut().enumerate() {
                    buf[frame] = frame_out[channel];
                }
            }
        }
        #[cfg(not(impulse_os))]
        {
            let inputs: Vec<&[FaustFloat]> = in_bufs.iter().map(|b| b.as_slice()).collect();
            let mut outputs: Vec<&mut [FaustFloat]> =
                out_bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
            dsp.compute(n, &inputs, &mut outputs);
        }
        for frame in 0..n {
            out.push_str(&format!("{written:>6} :"));
            for buf in out_bufs.iter() {
                out.push_str(&format!("  {:.6}", normalize(buf[frame])));
            }
            out.push('\n');
            written += 1;
        }
        cycle += 1;
    }
    print!("{out}");
}
