//! Cranelift impulse runner — the Cranelift-backend counterpart of
//! `impulse-runner` (interpreter) for the `tests/impulse-tests` harness.
//!
//! It JIT-compiles one DSP through the Cranelift backend C-API and runs the
//! scalar impulse pass (SR 44100, block 64, impulse on frame 0), emitting the
//! reference `.ir` text format with the same `normalize()` zero-clamp.
//!
//! Usage: `impulse_cranelift <file.dsp> [-n <frames>] [-I <dir>]... [-ss <n>]`

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;

use cranelift_ffi::factory::{createCCraneliftDSPFactoryFromFile, deleteCCraneliftDSPFactory};
use cranelift_ffi::instance::{
    buildUserInterfaceCCraneliftDSPInstance, computeCCraneliftDSPInstance,
    createCCraneliftDSPInstance, deleteCCraneliftDSPInstance, getNumInputsCCraneliftDSPInstance,
    getNumOutputsCCraneliftDSPInstance, initCCraneliftDSPInstance,
};
use cranelift_ffi::types::{FaustFloat, UIGlue};

const SAMPLE_RATE: i32 = 44100;
const BLOCK_SIZE: usize = 64;
const DEFAULT_FRAMES: usize = 15000;

fn main() -> ExitCode {
    // Cranelift JIT compilation plus the faust-rs front-end can recurse deeply;
    // run on a large stack like the crate's differential tests do.
    let result = thread::Builder::new()
        .name("impulse-cranelift".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(run)
        .expect("spawn worker thread")
        .join()
        .expect("join worker thread");
    match result {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("impulse-cranelift: {err}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    dsp: String,
    frames: usize,
    double: bool,
    import_dirs: Vec<String>,
    /// Compiler-mode flags forwarded to the FFI factory argv.
    compiler_argv: Vec<String>,
}

fn parse_args() -> Result<Options, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut dsp: Option<String> = None;
    let mut frames = DEFAULT_FRAMES;
    let mut double = false;
    let mut import_dirs = Vec::new();
    let mut compiler_argv = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-double" => double = true,
            "-single" => double = false,
            "-n" => {
                frames = args
                    .next()
                    .ok_or("missing value after -n")?
                    .parse::<usize>()
                    .map_err(|e| format!("bad -n value: {e}"))?;
            }
            "-I" => import_dirs.push(args.next().ok_or("missing value after -I")?),
            "-vec" => compiler_argv.push("-vec".to_owned()),
            "-vs" => {
                compiler_argv.push("-vs".to_owned());
                compiler_argv.push(args.next().ok_or("missing value after -vs")?);
            }
            "-lv" => {
                compiler_argv.push("-lv".to_owned());
                compiler_argv.push(args.next().ok_or("missing value after -lv")?);
            }
            "-ss" | "--scheduling-strategy" => {
                let value = args
                    .next()
                    .ok_or("missing value after scheduling-strategy option")?;
                value
                    .parse::<u32>()
                    .map_err(|e| format!("bad scheduling-strategy value: {e}"))?;
                compiler_argv.push("-ss".to_owned());
                compiler_argv.push(value);
            }
            other if other.starts_with('-') => return Err(format!("unknown option: {other}")),
            other => {
                if dsp.is_some() {
                    return Err(format!("unexpected extra argument: {other}"));
                }
                dsp = Some(other.to_owned());
            }
        }
    }
    Ok(Options {
        dsp: dsp.ok_or("missing <file.dsp> argument")?,
        frames,
        double,
        import_dirs,
        compiler_argv,
    })
}

fn run() -> Result<String, String> {
    let options = parse_args()?;

    // Search paths: explicit -I, then the DSP's own dir, then system libs.
    let mut search = options.import_dirs.clone();
    if let Some(parent) = PathBuf::from(&options.dsp).parent()
        && !parent.as_os_str().is_empty()
    {
        search.push(parent.to_string_lossy().into_owned());
    }
    if PathBuf::from("/usr/local/share/faust").is_dir() {
        search.push("/usr/local/share/faust".to_owned());
    }

    let mut argv_storage: Vec<CString> = Vec::new();
    if options.double {
        argv_storage.push(CString::new("-double").map_err(|e| e.to_string())?);
    }
    for opt in &options.compiler_argv {
        argv_storage.push(CString::new(opt.as_str()).map_err(|e| e.to_string())?);
    }
    for dir in &search {
        argv_storage.push(CString::new("-I").map_err(|e| e.to_string())?);
        argv_storage.push(CString::new(dir.as_str()).map_err(|e| e.to_string())?);
    }
    let argv_ptrs: Vec<*const c_char> = argv_storage.iter().map(|s| s.as_ptr()).collect();
    let c_path = CString::new(options.dsp.as_str()).map_err(|e| e.to_string())?;
    let mut err = [0_i8; 4096];

    let factory = unsafe {
        createCCraneliftDSPFactoryFromFile(
            c_path.as_ptr(),
            c_int::try_from(argv_ptrs.len()).map_err(|_| "too many -I args")?,
            if argv_ptrs.is_empty() {
                std::ptr::null()
            } else {
                argv_ptrs.as_ptr()
            },
            err.as_mut_ptr(),
            1,
        )
    };
    if factory.is_null() {
        return Err(format!(
            "Cranelift factory creation failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        ));
    }

    // The factory's concrete type is private to the crate; keep it inferred by
    // doing the run inline, and free it before returning.
    let frames = options.frames;
    let dsp = unsafe { createCCraneliftDSPInstance(factory) };
    if dsp.is_null() {
        unsafe {
            let _ = deleteCCraneliftDSPFactory(factory);
        }
        return Err("Cranelift instance creation failed".to_owned());
    }
    unsafe { initCCraneliftDSPInstance(dsp, SAMPLE_RATE) };

    let mut ui_capture = UiCapture::default();
    let mut ui = UIGlue {
        ui_interface: (&mut ui_capture as *mut UiCapture).cast::<c_void>(),
        open_tab_box: None,
        open_horizontal_box: None,
        open_vertical_box: None,
        close_box: None,
        add_button: Some(capture_button),
        add_check_button: None,
        add_vertical_slider: None,
        add_horizontal_slider: None,
        add_num_entry: None,
        add_horizontal_bargraph: None,
        add_vertical_bargraph: None,
        add_soundfile: Some(capture_soundfile),
        declare: None,
    };
    unsafe { buildUserInterfaceCCraneliftDSPInstance(dsp, &mut ui) };

    let num_inputs = usize::try_from(unsafe { getNumInputsCCraneliftDSPInstance(dsp) })
        .map_err(|_| "negative input arity".to_string())?;
    let num_outputs = usize::try_from(unsafe { getNumOutputsCCraneliftDSPInstance(dsp) })
        .map_err(|_| "negative output arity".to_string())?;

    let mut out = String::new();
    out.push_str(&format!("number_of_inputs  : {num_inputs:3}\n"));
    out.push_str(&format!("number_of_outputs : {num_outputs:3}\n"));
    out.push_str(&format!("number_of_frames  : {frames:6}\n"));

    // The JIT reads/writes I/O buffers at the compiled width (`f64` under
    // `-double`, `f32` otherwise). `computeCCraneliftDSPInstance` only forwards
    // the pointers, so the buffer element type is the caller's responsibility.
    // The element type differs but the loop is identical, hence the macro.
    macro_rules! run_pass {
        ($elem:ty) => {{
            let mut in_buffer = vec![vec![<$elem>::default(); BLOCK_SIZE]; num_inputs];
            let mut out_buffer = vec![vec![<$elem>::default(); BLOCK_SIZE]; num_outputs];
            let mut written = 0usize;
            let mut cycle = 0usize;
            while written < frames {
                let n = BLOCK_SIZE.min(frames - written);
                for channel in &mut in_buffer {
                    for sample in channel.iter_mut() {
                        *sample = <$elem>::default();
                    }
                    if written == 0 && !channel.is_empty() {
                        channel[0] = 1.0;
                    }
                }
                let button_value = if cycle == 0 { 1.0 } else { 0.0 };
                set_button_zones::<$elem>(&ui_capture.button_zones, button_value);
                let mut in_ptrs: Vec<*mut FaustFloat> = in_buffer
                    .iter_mut()
                    .map(|c| c.as_mut_ptr().cast::<FaustFloat>())
                    .collect();
                let mut out_ptrs: Vec<*mut FaustFloat> = out_buffer
                    .iter_mut()
                    .map(|c| c.as_mut_ptr().cast::<FaustFloat>())
                    .collect();
                unsafe {
                    computeCCraneliftDSPInstance(
                        dsp,
                        n as i32,
                        in_ptrs.as_mut_ptr(),
                        out_ptrs.as_mut_ptr(),
                    );
                }
                for j in 0..n {
                    out.push_str(&format!("{written:6} : "));
                    for channel in out_buffer.iter().take(num_outputs) {
                        let value = normalize(channel[j] as f64);
                        out.push_str(&format!(" {value:8.6}"));
                    }
                    out.push('\n');
                    written += 1;
                }
                cycle += 1;
            }
        }};
    }

    if options.double {
        run_pass!(f64);
    } else {
        run_pass!(f32);
    }

    unsafe {
        deleteCCraneliftDSPInstance(dsp);
        let _ = deleteCCraneliftDSPFactory(factory);
    }
    Ok(out)
}

/// Zero-clamps tiny magnitudes exactly like `controlTools.h::normalize`.
fn normalize(value: f64) -> f64 {
    if value.is_nan() || value.is_infinite() {
        value
    } else if value.abs() < 0.000_001 {
        0.0
    } else {
        value
    }
}

#[derive(Default)]
struct UiCapture {
    button_zones: Vec<*mut c_void>,
    soundfiles: Vec<TestSoundfile>,
}

unsafe extern "C" fn capture_button(
    ui_interface: *mut c_void,
    _label: *const c_char,
    zone: *mut FaustFloat,
) {
    if ui_interface.is_null() || zone.is_null() {
        return;
    }
    let capture = unsafe { &mut *ui_interface.cast::<UiCapture>() };
    capture.button_zones.push(zone.cast::<c_void>());
}

unsafe extern "C" fn capture_soundfile(
    ui_interface: *mut c_void,
    _label: *const c_char,
    url: *const c_char,
    zone: *mut *mut c_void,
) {
    if ui_interface.is_null() || zone.is_null() {
        return;
    }
    let capture = unsafe { &mut *ui_interface.cast::<UiCapture>() };
    let url = if url.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(url) }.to_str().unwrap_or("")
    };
    capture
        .soundfiles
        .push(TestSoundfile::impulse_test_memory_reader(
            soundfile_part_count(url),
        ));
    let soundfile = capture
        .soundfiles
        .last_mut()
        .expect("just pushed soundfile")
        .as_mut_ptr();
    unsafe {
        *zone = soundfile;
    }
}

fn set_button_zones<T: From<f32>>(zones: &[*mut c_void], value: f32) {
    for &zone in zones {
        if !zone.is_null() {
            unsafe {
                *zone.cast::<T>() = T::from(value);
            }
        }
    }
}

/// Counts the resource parts encoded in a Faust soundfile URL.
fn soundfile_part_count(url: &str) -> usize {
    let trimmed = url.trim();
    let Some(open) = trimmed.find('{') else {
        return usize::from(!trimmed.is_empty()).max(1);
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

#[repr(C)]
struct RawSoundfile {
    buffers: *mut c_void,
    lengths: *mut i32,
    sample_rates: *mut i32,
    offsets: *mut i32,
    channels: i32,
    parts: i32,
    is_double: bool,
}

struct TestSoundfile {
    raw: Box<RawSoundfile>,
    #[allow(dead_code)]
    lengths: Vec<i32>,
    #[allow(dead_code)]
    sample_rates: Vec<i32>,
    #[allow(dead_code)]
    offsets: Vec<i32>,
    #[allow(dead_code)]
    channel_ptrs: Vec<*mut f64>,
    #[allow(dead_code)]
    buffers: Vec<Vec<f64>>,
}

impl TestSoundfile {
    fn impulse_test_memory_reader(num_real_parts: usize) -> Self {
        const SOUND_CHAN: usize = 2;
        const SOUND_LENGTH: usize = 4096;
        const SOUND_SR: i32 = 44100;
        const BUFFER_SIZE: usize = 1024;
        const MAX_CHAN: usize = 64;
        const MAX_SOUNDFILE_PARTS: usize = 256;

        let real_parts = num_real_parts.min(MAX_SOUNDFILE_PARTS);
        let mut lengths = Vec::with_capacity(MAX_SOUNDFILE_PARTS);
        let mut sample_rates = Vec::with_capacity(MAX_SOUNDFILE_PARTS);
        let mut offsets = Vec::with_capacity(MAX_SOUNDFILE_PARTS);
        let mut offset = 0usize;

        for _part in 0..real_parts {
            lengths.push(SOUND_LENGTH as i32);
            sample_rates.push(SOUND_SR);
            offsets.push(offset as i32);
            offset += SOUND_LENGTH;
        }
        for _part in real_parts..MAX_SOUNDFILE_PARTS {
            lengths.push(BUFFER_SIZE as i32);
            sample_rates.push(SOUND_SR);
            offsets.push(offset as i32);
            offset += BUFFER_SIZE;
        }

        let mut buffers = vec![vec![0.0; offset]; SOUND_CHAN];
        for (part, part_offset) in offsets.iter().copied().enumerate().take(real_parts) {
            let part_offset = part_offset as usize;
            for sample in 0..SOUND_LENGTH {
                let value = (part as f64
                    + (2.0 * std::f64::consts::PI * sample as f64 / SOUND_LENGTH as f64))
                    .sin();
                for channel in buffers.iter_mut().take(SOUND_CHAN) {
                    channel[part_offset + sample] = value;
                }
            }
        }

        let mut channel_ptrs = Vec::with_capacity(MAX_CHAN);
        for channel in 0..MAX_CHAN {
            channel_ptrs.push(buffers[channel % SOUND_CHAN].as_mut_ptr());
        }

        let raw = Box::new(RawSoundfile {
            buffers: channel_ptrs.as_mut_ptr().cast::<c_void>(),
            lengths: lengths.as_mut_ptr(),
            sample_rates: sample_rates.as_mut_ptr(),
            offsets: offsets.as_mut_ptr(),
            channels: SOUND_CHAN as i32,
            parts: real_parts as i32,
            is_double: true,
        });

        Self {
            raw,
            lengths,
            sample_rates,
            offsets,
            channel_ptrs,
            buffers,
        }
    }

    fn as_mut_ptr(&mut self) -> *mut c_void {
        self.raw.as_mut() as *mut RawSoundfile as *mut c_void
    }
}

#[cfg(test)]
mod tests {
    use super::{TestSoundfile, parse_args_from, soundfile_part_count};

    fn parse(args: &[&str]) -> Result<super::Options, String> {
        parse_args_from(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn scheduling_strategy_is_normalized_for_the_ffi_factory() {
        let options = parse(&["test.dsp", "-vec", "-lv", "1", "--scheduling-strategy", "3"])
            .expect("parse options");
        assert_eq!(options.compiler_argv, ["-vec", "-lv", "1", "-ss", "3"]);
    }

    #[test]
    fn malformed_scheduling_strategy_is_rejected_before_factory_creation() {
        assert!(parse(&["test.dsp", "-ss"]).is_err());
        assert!(parse(&["test.dsp", "-ss", "-1"]).is_err());
        assert!(parse(&["test.dsp", "-ss", "abc"]).is_err());
    }

    #[test]
    fn soundfile_part_count_follows_sound_ui_menu_urls() {
        assert_eq!(soundfile_part_count("{'sound1';'sound2'}"), 2);
        assert_eq!(soundfile_part_count("sound1"), 1);
        assert_eq!(soundfile_part_count(""), 1);
    }

    #[test]
    fn test_soundfile_shares_channels_like_cpp_fixture() {
        let mut sf = TestSoundfile::impulse_test_memory_reader(2);
        assert_eq!(sf.lengths[0], 4096);
        assert_eq!(sf.offsets[1], 4096);
        assert_eq!(sf.channel_ptrs[0], sf.buffers[0].as_mut_ptr());
        assert_eq!(sf.channel_ptrs[2], sf.buffers[0].as_mut_ptr());
    }
}
