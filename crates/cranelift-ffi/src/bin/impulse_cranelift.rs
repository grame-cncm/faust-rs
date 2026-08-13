//! Cranelift impulse runner — the Cranelift-backend counterpart of
//! `impulse-runner` (interpreter) for the `tests/impulse-tests` harness.
//!
//! It JIT-compiles one DSP through the Cranelift backend C-API and runs the
//! scalar impulse pass (SR 44100, block 64, impulse on frame 0), emitting the
//! reference `.ir` text format with the same `normalize()` zero-clamp.
//!
//! Usage: `impulse_cranelift <file.dsp> [-n <frames>] [-I <dir>]... [-ss <n>]`

use std::alloc::{Layout, alloc, dealloc};
use std::collections::HashMap;
use std::ffi::{CStr, CString, OsString, c_char, c_int, c_void};
use std::iter;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;

use clap::Parser;
use cranelift_ffi::factory::{
    createCCraneliftDSPFactoryFromFile, deleteCCraneliftDSPFactory, freeCMemory,
    getCCraneliftDSPFactoryJSON, setCCraneliftMemoryManager,
};
use cranelift_ffi::instance::{
    buildUserInterfaceCCraneliftDSPInstance, computeCCraneliftDSPInstance,
    createCCraneliftDSPInstance, deleteCCraneliftDSPInstance, getNumInputsCCraneliftDSPInstance,
    getNumOutputsCCraneliftDSPInstance, initCCraneliftDSPInstance,
};
use cranelift_ffi::types::{FaustFloat, UIGlue};
use ffi_common::abi::{FAUST_MEMORY_MANAGER_ABI_VERSION, FaustMemoryManager, FaustMemoryType};

const SAMPLE_RATE: i32 = 44100;
const BLOCK_SIZE: usize = 64;
const DEFAULT_FRAMES: usize = 15000;

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(options) => options,
        Err(error) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(exit_code as u8);
        }
    };

    // Cranelift JIT compilation plus the faust-rs front-end can recurse deeply;
    // run on a large stack like the crate's differential tests do.
    let result = thread::Builder::new()
        .name("impulse-cranelift".to_owned())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || run(options))
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

#[derive(Debug)]
struct Options {
    dsp: String,
    frames: usize,
    double: bool,
    import_dirs: Vec<String>,
    /// Compiler-mode flags forwarded to the FFI factory argv.
    compiler_argv: Vec<String>,
    /// Install and audit the shared custom-memory manager.
    mem0: bool,
    /// Optional path receiving the validated factory JSON.
    json_output: Option<PathBuf>,
    /// Cranelift optimization level passed to the factory.
    opt_level: i32,
}

#[derive(Debug, Parser)]
#[command(
    name = "impulse-cranelift",
    version,
    about = "Compile a Faust DSP with Cranelift and emit its scalar impulse response",
    args_override_self = true
)]
struct CliArgs {
    /// Faust DSP source file.
    #[arg(value_name = "FILE")]
    dsp: String,

    /// Number of impulse-response frames to emit.
    #[arg(short = 'n', long = "frames", default_value_t = DEFAULT_FRAMES)]
    frames: usize,

    /// Compile and execute with double-precision samples.
    #[arg(long, overrides_with = "single")]
    double: bool,

    /// Compile and execute with single-precision samples.
    #[arg(long, overrides_with = "double")]
    single: bool,

    /// Add a Faust library import directory.
    #[arg(short = 'I', long = "import-dir", value_name = "DIR")]
    import_dirs: Vec<String>,

    /// Enable vector compilation.
    #[arg(long = "vectorize")]
    vectorize: bool,

    /// Vector loop size.
    #[arg(long = "vector-size")]
    vector_size: Option<u32>,

    /// Vector loop variant.
    #[arg(long = "loop-variant")]
    loop_variant: Option<u8>,

    /// FIR scheduling strategy selector.
    #[arg(long = "scheduling-strategy")]
    scheduling_strategy: Option<u32>,

    /// How a `rdtable`/`rwtable` initialization signal is computed:
    /// `const` folds it at compile time, `runtime` emits a generator
    /// sub-module that fills the table at initialization.
    #[arg(long = "table-init", value_name = "MODE")]
    table_init: Option<String>,

    /// Use the mode-zero custom memory manager and reject JIT fallback stubs.
    #[arg(long = "memory-manager0")]
    mem0: bool,

    /// Write the factory JSON after semantic validation.
    #[arg(long = "json-output", value_name = "FILE")]
    json_output: Option<PathBuf>,

    /// Cranelift optimization level (`0` = none, `3` = maximum).
    #[arg(long = "opt-level", default_value_t = 1, value_parser = clap::value_parser!(i32).range(0..=3))]
    opt_level: i32,
}

fn parse_args() -> Result<Options, clap::Error> {
    parse_args_from(std::env::args_os().skip(1))
}

fn parse_args_from<I, T>(args: I) -> Result<Options, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let normalized = iter::once(OsString::from("impulse-cranelift"))
        .chain(args.into_iter().map(Into::into).map(normalize_legacy_arg));
    let args = CliArgs::try_parse_from(normalized)?;
    let mut compiler_argv = Vec::new();
    if args.vectorize {
        compiler_argv.push("-vec".to_owned());
    }
    if let Some(value) = args.vector_size {
        compiler_argv.extend(["-vs".to_owned(), value.to_string()]);
    }
    if let Some(value) = args.loop_variant {
        compiler_argv.extend(["-lv".to_owned(), value.to_string()]);
    }
    if let Some(value) = args.scheduling_strategy {
        compiler_argv.extend(["-ss".to_owned(), value.to_string()]);
    }
    if let Some(value) = args.table_init {
        compiler_argv.extend(["--table-init".to_owned(), value]);
    }
    if args.mem0 {
        compiler_argv.push("-mem0".to_owned());
    }

    Ok(Options {
        dsp: args.dsp,
        frames: args.frames,
        double: args.double,
        import_dirs: args.import_dirs,
        compiler_argv,
        mem0: args.mem0,
        json_output: args.json_output,
        opt_level: args.opt_level,
    })
}

fn normalize_legacy_arg(arg: OsString) -> OsString {
    match arg.to_str() {
        Some("-double") => OsString::from("--double"),
        Some("-single") => OsString::from("--single"),
        Some("-vec") => OsString::from("--vectorize"),
        Some("-vs") => OsString::from("--vector-size"),
        Some("-lv") => OsString::from("--loop-variant"),
        Some("-ss") => OsString::from("--scheduling-strategy"),
        Some("-mem" | "-mem0" | "--memory-manager") => OsString::from("--memory-manager0"),
        _ => arg,
    }
}

fn run(options: Options) -> Result<String, String> {
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
            options.opt_level,
        )
    };
    if factory.is_null() {
        return Err(format!(
            "Cranelift factory creation failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        ));
    }

    let mut audit = options.mem0.then(|| Box::new(AuditMemoryState::default()));
    if let Some(state) = audit.as_mut() {
        let json = match validate_mem0_factory_json(factory) {
            Ok(json) => json,
            Err(error) => {
                unsafe {
                    let _ = deleteCCraneliftDSPFactory(factory);
                }
                return Err(error);
            }
        };
        if let Some(path) = &options.json_output
            && let Err(error) = std::fs::write(path, json)
        {
            unsafe {
                let _ = deleteCCraneliftDSPFactory(factory);
            }
            return Err(format!("cannot write Cranelift factory JSON: {error}"));
        }
        let manager = FaustMemoryManager {
            abi_version: FAUST_MEMORY_MANAGER_ABI_VERSION,
            struct_size: std::mem::size_of::<FaustMemoryManager>(),
            context: (&mut **state as *mut AuditMemoryState).cast(),
            begin: Some(audit_begin),
            info: Some(audit_info),
            end: Some(audit_end),
            allocate: Some(audit_allocate),
            destroy: Some(audit_destroy),
        };
        if !unsafe { setCCraneliftMemoryManager(factory, &manager, err.as_mut_ptr()) } {
            unsafe {
                let _ = deleteCCraneliftDSPFactory(factory);
            }
            return Err(format!(
                "Cranelift memory-manager binding failed: {}",
                unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
            ));
        }
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
    if let Some(state) = audit {
        state.verify()?;
    }
    Ok(out)
}

#[derive(Debug)]
struct DescribedZone {
    name: String,
    size_bytes: usize,
    alignment: usize,
}

#[derive(Debug)]
struct LiveAllocation {
    layout: Layout,
    requested_size: usize,
    requested_alignment: usize,
}

/// Audits the shared Cranelift manager contract used by the impulse lane.
///
/// The callbacks never panic across FFI. They record the first violation and
/// let the runner report it after factory destruction, when every allocation
/// must have been released in global reverse order.
#[derive(Debug, Default)]
struct AuditMemoryState {
    expected_descriptions: usize,
    descriptions: Vec<DescribedZone>,
    next_allocation: usize,
    live: HashMap<usize, LiveAllocation>,
    allocation_stack: Vec<usize>,
    failure: Option<String>,
}

impl AuditMemoryState {
    fn fail(&mut self, message: impl Into<String>) {
        if self.failure.is_none() {
            self.failure = Some(message.into());
        }
    }

    fn verify(self) -> Result<(), String> {
        if let Some(error) = self.failure {
            return Err(format!("mem0 allocation audit failed: {error}"));
        }
        if self.descriptions.len() != self.expected_descriptions {
            return Err("mem0 description count did not close".to_owned());
        }
        if self.next_allocation != self.descriptions.len() {
            return Err(format!(
                "mem0 described {} zones but allocated {}",
                self.descriptions.len(),
                self.next_allocation
            ));
        }
        if !self.live.is_empty() || !self.allocation_stack.is_empty() {
            return Err("mem0 manager retained live allocations after factory release".to_owned());
        }
        Ok(())
    }
}

unsafe extern "C" fn audit_begin(context: *mut c_void, count: usize) {
    let state = unsafe { &mut *context.cast::<AuditMemoryState>() };
    state.expected_descriptions = count;
    state.descriptions.clear();
    state.next_allocation = 0;
}

unsafe extern "C" fn audit_info(
    context: *mut c_void,
    name: *const c_char,
    _memory_type: FaustMemoryType,
    _element_count: usize,
    size_bytes: usize,
    alignment: usize,
    _reads: u64,
    _writes: u64,
) {
    let state = unsafe { &mut *context.cast::<AuditMemoryState>() };
    let name = if name.is_null() {
        "<null>".to_owned()
    } else {
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    state.descriptions.push(DescribedZone {
        name,
        size_bytes,
        alignment,
    });
}

unsafe extern "C" fn audit_end(context: *mut c_void) {
    let state = unsafe { &mut *context.cast::<AuditMemoryState>() };
    if state.descriptions.len() != state.expected_descriptions {
        state.fail(format!(
            "begin announced {} zones but info reported {}",
            state.expected_descriptions,
            state.descriptions.len()
        ));
    }
}

unsafe extern "C" fn audit_allocate(
    context: *mut c_void,
    size_bytes: usize,
    alignment: usize,
) -> *mut c_void {
    let state = unsafe { &mut *context.cast::<AuditMemoryState>() };
    if let Some(zone) = state.descriptions.get(state.next_allocation) {
        if zone.size_bytes != size_bytes || zone.alignment != alignment {
            state.fail(format!(
                "allocation {} requested {size_bytes}/{alignment}, description `{}` says {}/{}",
                state.next_allocation, zone.name, zone.size_bytes, zone.alignment
            ));
        }
    } else {
        state.fail("allocation has no matching description");
    }
    state.next_allocation += 1;
    let Ok(layout) = Layout::from_size_align(size_bytes.max(1), alignment) else {
        state.fail(format!(
            "invalid allocation layout {size_bytes}/{alignment}"
        ));
        return std::ptr::null_mut();
    };
    let address = unsafe { alloc(layout) };
    if address.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { address.write_bytes(0xa5, layout.size()) };
    let key = address as usize;
    state.live.insert(
        key,
        LiveAllocation {
            layout,
            requested_size: size_bytes,
            requested_alignment: alignment,
        },
    );
    state.allocation_stack.push(key);
    address.cast()
}

unsafe extern "C" fn audit_destroy(
    context: *mut c_void,
    address: *mut c_void,
    size_bytes: usize,
    alignment: usize,
) {
    let state = unsafe { &mut *context.cast::<AuditMemoryState>() };
    let key = address as usize;
    if state.allocation_stack.pop() != Some(key) {
        state.fail("allocations were not destroyed in reverse order");
    }
    let Some(live) = state.live.remove(&key) else {
        state.fail("destroy received an unknown or already released pointer");
        return;
    };
    if live.requested_size != size_bytes || live.requested_alignment != alignment {
        state.fail("destroy size/alignment differs from allocate");
    }
    unsafe { dealloc(address.cast(), live.layout) };
}

fn validate_mem0_factory_json<T>(factory: *mut T) -> Result<String, String> {
    let json_ptr = unsafe { getCCraneliftDSPFactoryJSON(factory.cast()) };
    if json_ptr.is_null() {
        return Err("Cranelift mem0 factory returned no JSON".to_owned());
    }
    let json = unsafe { CStr::from_ptr(json_ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { freeCMemory(json_ptr.cast()) };
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|error| format!("invalid factory JSON: {error}"))?;
    if value["memory_layout_version"] != 2
        || value["memory_manager"]["backend"] != "cranelift"
        || value["memory_manager"]["manager_abi"] != "faust_memory_manager_v1"
        || value["compute_cost_version"] != 2
        || value["compute_body_lowered"] != true
    {
        return Err(
            "Cranelift mem0 JSON reports an invalid layout or fallback compute body".to_owned(),
        );
    }
    Ok(json)
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
    use clap::CommandFactory;

    use super::{CliArgs, TestSoundfile, parse_args_from, soundfile_part_count};

    fn parse(args: &[&str]) -> Result<super::Options, clap::Error> {
        parse_args_from(args.iter().copied())
    }

    #[test]
    fn scheduling_strategy_is_normalized_for_the_ffi_factory() {
        let options = parse(&["test.dsp", "-vec", "-lv", "1", "--scheduling-strategy", "3"])
            .expect("parse options");
        assert_eq!(options.compiler_argv, ["-vec", "-lv", "1", "-ss", "3"]);
    }

    #[test]
    fn clap_definition_is_consistent() {
        CliArgs::command().debug_assert();
    }

    #[test]
    fn malformed_scheduling_strategy_is_rejected_before_factory_creation() {
        assert!(parse(&["test.dsp", "-ss"]).is_err());
        assert!(parse(&["test.dsp", "-ss", "-1"]).is_err());
        assert!(parse(&["test.dsp", "-ss", "abc"]).is_err());
    }

    #[test]
    fn legacy_flags_work_before_the_dsp_and_last_precision_wins() {
        let options = parse(&["-double", "-I", "lib", "test.dsp", "-single", "-n", "8"])
            .expect("parse legacy options");
        assert!(!options.double);
        assert_eq!(options.frames, 8);
        assert_eq!(options.import_dirs, ["lib"]);
    }

    #[test]
    fn every_mem0_alias_is_canonicalized_for_the_factory() {
        for alias in ["-mem", "-mem0", "--memory-manager", "--memory-manager0"] {
            let options = parse(&["test.dsp", alias]).expect("parse mem0 alias");
            assert!(options.mem0);
            assert_eq!(options.compiler_argv, ["-mem0"]);
        }
    }

    #[test]
    fn json_output_is_runner_only_and_not_forwarded() {
        let options =
            parse(&["test.dsp", "--json-output", "layout.json"]).expect("parse JSON output");
        assert_eq!(
            options.json_output.as_deref(),
            Some(std::path::Path::new("layout.json"))
        );
        assert!(options.compiler_argv.is_empty());
    }

    #[test]
    fn optimization_level_is_checked_and_not_forwarded_as_a_faust_option() {
        for level in [0, 1, 2, 3] {
            let options = parse(&["test.dsp", "--opt-level", &level.to_string()])
                .expect("parse optimization level");
            assert_eq!(options.opt_level, level);
            assert!(options.compiler_argv.is_empty());
        }
        assert!(parse(&["test.dsp", "--opt-level", "4"]).is_err());
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
