// ---------------------------------------------------------------------------
// impulserust.rs — impulse-test architecture for the faust-rs Rust backend.
//
// Appended after the generated `-lang rust -double -cn mydsp` output
// (top-level Rust items are order-independent, so a plain `cat` works).
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

use std::rc::Rc;

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
    fn add_horizontal_slider(&mut self, label: &str, zone: &mut T, init: T, min: T, max: T, step: T) {
    }
    fn add_vertical_slider(&mut self, label: &str, zone: &mut T, init: T, min: T, max: T, step: T) {}
    fn add_num_entry(&mut self, label: &str, zone: &mut T, init: T, min: T, max: T, step: T) {}
    fn add_horizontal_bargraph(&mut self, label: &str, zone: &mut T, min: T, max: T) {}
    fn add_vertical_bargraph(&mut self, label: &str, zone: &mut T, min: T, max: T) {}
    fn add_soundfile(&mut self, label: &str, url: &str, sf: &mut Soundfile) {}
    fn declare(&mut self, zone: Option<&mut T>, key: &str, value: &str) {}
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

/// UI visitor that installs the soundfile fixture on every soundfile widget.
struct InstallSoundfiles;

impl UI<FaustFloat> for InstallSoundfiles {
    fn add_soundfile(&mut self, _label: &str, url: &str, sf: &mut Soundfile) {
        *sf = make_soundfile(soundfile_part_count(url));
    }
}

/// UI visitor that drives button zones only, like the C++ `FUI::setButtons`.
struct SetButtons {
    value: FaustFloat,
}

impl UI<FaustFloat> for SetButtons {
    fn add_button(&mut self, _label: &str, zone: &mut FaustFloat) {
        *zone = self.value;
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
    dsp.build_user_interface(&mut InstallSoundfiles);

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
        dsp.build_user_interface(&mut SetButtons { value: button });
        {
            let inputs: Vec<&[FaustFloat]> = in_bufs.iter().map(|b| b.as_slice()).collect();
            let mut outputs: Vec<&mut [FaustFloat]> =
                out_bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
            dsp.compute(n as i32, &inputs, &mut outputs);
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
