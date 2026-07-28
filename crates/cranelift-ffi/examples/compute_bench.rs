use std::ffi::{CStr, CString};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{ArgGroup, Parser, ValueEnum};
use codegen::backends::cranelift::{CraneliftOptions, generate_cranelift_module};
use codegen::backends::interp::{InterpOptions, generate_interp_module, write_fbc};
use codegen::fixtures::{build_heavy_bench_test_module, build_sine_phasor_test_module};
use cranelift_ffi::factory::{createCCraneliftDSPFactoryFromFile, deleteCCraneliftDSPFactory};
use cranelift_ffi::instance::{
    computeCCraneliftDSPInstance, createCCraneliftDSPInstance, deleteCCraneliftDSPInstance,
    getNumInputsCCraneliftDSPInstance, getNumOutputsCCraneliftDSPInstance,
    initCCraneliftDSPInstance,
};
use cranelift_ffi::types::FaustFloat;
use interp_ffi::factory::{
    createCInterpreterDSPFactoryFromFile, deleteCInterpreterDSPFactory,
    readCInterpreterDSPFactoryFromBitcode,
};
use interp_ffi::instance::{
    computeCInterpreterDSPInstance, createCInterpreterDSPInstance, deleteCInterpreterDSPInstance,
    getNumInputsCInterpreterDSPInstance, getNumOutputsCInterpreterDSPInstance,
    initCInterpreterDSPInstance,
};

const SAMPLE_RATE: i32 = 48_000;
const BLOCK_SIZE: usize = 64;
const NUM_BLOCKS: usize = 4096;
const WARMUP_BLOCKS: usize = 256;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Fixture {
    #[value(name = "sine_phasor")]
    SinePhasor,
    #[value(name = "heavy_bench")]
    HeavyBench,
}

#[derive(Debug, Parser)]
#[command(
    name = "compute_bench",
    about = "Compare interpreter and Cranelift compute throughput",
    group(
        ArgGroup::new("input")
            .required(true)
            .multiple(false)
            .args(["dsp", "fixture"])
    )
)]
struct CliArgs {
    /// Faust DSP source file.
    #[arg(value_name = "DSP")]
    dsp: Option<PathBuf>,

    /// Built-in compiler fixture.
    #[arg(long, value_enum)]
    fixture: Option<Fixture>,
}

type ComputeFn =
    unsafe extern "C" fn(*mut std::ffi::c_void, i32, *mut *mut FaustFloat, *mut *mut FaustFloat);

fn c_int_arity_to_usize(value: i32, label: &str) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| format!("invalid negative {label}: {value}"))
}

#[derive(Debug, Clone, Copy)]
enum BenchInput<'a> {
    DspFile(&'a Path),
    FixtureSinePhasor,
    FixtureHeavyBench,
}

struct OwnedAlignedDspState {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}

impl OwnedAlignedDspState {
    fn new(size: usize, align: usize) -> Result<Self, String> {
        let layout = std::alloc::Layout::from_size_align(size.max(1), align.max(1))
            .map_err(|e| format!("invalid layout size={size} align={align}: {e}"))?;
        // SAFETY: layout is valid and non-zero sized.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err(format!(
                "allocation failed (size={}, align={})",
                layout.size(),
                layout.align()
            ));
        }
        Ok(Self { ptr, layout })
    }
}

impl Drop for OwnedAlignedDspState {
    fn drop(&mut self) {
        // SAFETY: `ptr/layout` pair came from `alloc_zeroed` with same layout.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) };
    }
}

fn prepare_impulse_block(input_channels: &mut [Vec<f32>], global_start: usize) {
    for (ch_idx, channel) in input_channels.iter_mut().enumerate() {
        channel.fill(0.0);
        if global_start == 0 && !channel.is_empty() {
            channel[0] = 1.0f32 + ch_idx as f32;
        }
    }
}

fn run_blocked_bench<F>(
    num_inputs: usize,
    num_outputs: usize,
    mut compute_block: F,
) -> (Duration, f64)
where
    F: FnMut(*mut *mut FaustFloat, *mut *mut FaustFloat),
{
    let mut input_channels = vec![vec![0.0f32; BLOCK_SIZE]; num_inputs];
    let mut output_channels = vec![vec![0.0f32; BLOCK_SIZE]; num_outputs];
    let mut input_ptrs: Vec<*mut FaustFloat> = input_channels
        .iter_mut()
        .map(|ch| ch.as_mut_ptr())
        .collect();
    let mut output_ptrs: Vec<*mut FaustFloat> = output_channels
        .iter_mut()
        .map(|ch| ch.as_mut_ptr())
        .collect();

    for block_idx in 0..WARMUP_BLOCKS {
        prepare_impulse_block(&mut input_channels, block_idx * BLOCK_SIZE);
        for ch in &mut output_channels {
            ch.fill(0.0);
        }
        compute_block(input_ptrs.as_mut_ptr(), output_ptrs.as_mut_ptr());
    }

    let t0 = Instant::now();
    let mut checksum = 0.0f64;
    for block_idx in WARMUP_BLOCKS..(WARMUP_BLOCKS + NUM_BLOCKS) {
        prepare_impulse_block(&mut input_channels, block_idx * BLOCK_SIZE);
        for ch in &mut output_channels {
            ch.fill(0.0);
        }
        compute_block(input_ptrs.as_mut_ptr(), output_ptrs.as_mut_ptr());
        checksum += output_channels
            .iter()
            .flat_map(|ch| ch.iter())
            .map(|x| f64::from(*x))
            .sum::<f64>();
    }
    (t0.elapsed(), checksum)
}

fn run_interp(input: BenchInput<'_>) -> Result<(Duration, f64), String> {
    match input {
        BenchInput::DspFile(case) => run_interp_file(case),
        BenchInput::FixtureSinePhasor => run_interp_fixture_sine_phasor(),
        BenchInput::FixtureHeavyBench => run_interp_fixture_heavy_bench(),
    }
}

fn run_interp_file(case: &Path) -> Result<(Duration, f64), String> {
    let c_path = CString::new(case.to_string_lossy().as_bytes())
        .map_err(|e| format!("CString path: {e}"))?;
    let mut err = [0_i8; 4096];
    let factory = unsafe {
        createCInterpreterDSPFactoryFromFile(c_path.as_ptr(), 0, std::ptr::null(), err.as_mut_ptr())
    };
    if factory.is_null() {
        return Err(format!(
            "createCInterpreterDSPFactoryFromFile failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        ));
    }
    let dsp = unsafe { createCInterpreterDSPInstance(factory) };
    if dsp.is_null() {
        unsafe {
            let _ = deleteCInterpreterDSPFactory(factory);
        }
        return Err("createCInterpreterDSPInstance failed".to_owned());
    }
    unsafe { initCInterpreterDSPInstance(dsp, SAMPLE_RATE) };

    let num_inputs = c_int_arity_to_usize(
        unsafe { getNumInputsCInterpreterDSPInstance(dsp) },
        "interp input arity",
    )?;
    let num_outputs = c_int_arity_to_usize(
        unsafe { getNumOutputsCInterpreterDSPInstance(dsp) },
        "interp output arity",
    )?;

    let (elapsed, checksum) =
        run_blocked_bench(num_inputs, num_outputs, |input_ptrs, output_ptrs| {
            unsafe {
                computeCInterpreterDSPInstance(dsp, BLOCK_SIZE as i32, input_ptrs, output_ptrs)
            };
        });

    unsafe {
        deleteCInterpreterDSPInstance(dsp);
        let _ = deleteCInterpreterDSPFactory(factory);
    }

    black_box(checksum);
    Ok((elapsed, checksum))
}

fn run_interp_fixture_sine_phasor() -> Result<(Duration, f64), String> {
    let (store, module) = build_sine_phasor_test_module();
    let options = InterpOptions::default();
    let factory =
        generate_interp_module::<f32>(&store, module, &options).map_err(|e| e.to_string())?;
    let mut fbc_bytes = Vec::<u8>::new();
    write_fbc(&factory, &mut fbc_bytes, false).map_err(|e| e.to_string())?;
    let fbc_text = String::from_utf8(fbc_bytes).map_err(|e| format!("utf8 FBC: {e}"))?;
    let c_fbc = CString::new(fbc_text.as_bytes()).map_err(|e| format!("CString FBC: {e}"))?;
    let mut err = [0_i8; 4096];
    let ffi_factory =
        unsafe { readCInterpreterDSPFactoryFromBitcode(c_fbc.as_ptr(), err.as_mut_ptr()) };
    if ffi_factory.is_null() {
        return Err(format!(
            "readCInterpreterDSPFactoryFromBitcode failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        ));
    }
    let dsp = unsafe { createCInterpreterDSPInstance(ffi_factory) };
    if dsp.is_null() {
        unsafe {
            let _ = deleteCInterpreterDSPFactory(ffi_factory);
        }
        return Err("createCInterpreterDSPInstance(fixture) failed".to_owned());
    }
    unsafe { initCInterpreterDSPInstance(dsp, SAMPLE_RATE) };

    let num_inputs = c_int_arity_to_usize(
        unsafe { getNumInputsCInterpreterDSPInstance(dsp) },
        "interp fixture sine_phasor input arity",
    )?;
    let num_outputs = c_int_arity_to_usize(
        unsafe { getNumOutputsCInterpreterDSPInstance(dsp) },
        "interp fixture sine_phasor output arity",
    )?;
    if num_outputs == 0 {
        unsafe {
            deleteCInterpreterDSPInstance(dsp);
            let _ = deleteCInterpreterDSPFactory(ffi_factory);
        }
        return Err("interp fixture sine_phasor reported 0 outputs".to_owned());
    }

    let (elapsed, checksum) =
        run_blocked_bench(num_inputs, num_outputs, |input_ptrs, output_ptrs| {
            unsafe {
                computeCInterpreterDSPInstance(dsp, BLOCK_SIZE as i32, input_ptrs, output_ptrs)
            };
        });

    unsafe {
        deleteCInterpreterDSPInstance(dsp);
        let _ = deleteCInterpreterDSPFactory(ffi_factory);
    }

    black_box(checksum);
    Ok((elapsed, checksum))
}

fn run_interp_fixture_heavy_bench() -> Result<(Duration, f64), String> {
    let (store, module) = build_heavy_bench_test_module();
    let options = InterpOptions::default();
    let factory =
        generate_interp_module::<f32>(&store, module, &options).map_err(|e| e.to_string())?;
    let mut fbc_bytes = Vec::<u8>::new();
    write_fbc(&factory, &mut fbc_bytes, false).map_err(|e| e.to_string())?;
    let fbc_text = String::from_utf8(fbc_bytes).map_err(|e| format!("utf8 FBC: {e}"))?;
    let c_fbc = CString::new(fbc_text.as_bytes()).map_err(|e| format!("CString FBC: {e}"))?;
    let mut err = [0_i8; 4096];
    let ffi_factory =
        unsafe { readCInterpreterDSPFactoryFromBitcode(c_fbc.as_ptr(), err.as_mut_ptr()) };
    if ffi_factory.is_null() {
        return Err(format!(
            "readCInterpreterDSPFactoryFromBitcode failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        ));
    }
    let dsp = unsafe { createCInterpreterDSPInstance(ffi_factory) };
    if dsp.is_null() {
        unsafe {
            let _ = deleteCInterpreterDSPFactory(ffi_factory);
        }
        return Err("createCInterpreterDSPInstance(fixture heavy_bench) failed".to_owned());
    }
    unsafe { initCInterpreterDSPInstance(dsp, SAMPLE_RATE) };

    let num_inputs = c_int_arity_to_usize(
        unsafe { getNumInputsCInterpreterDSPInstance(dsp) },
        "interp fixture heavy_bench input arity",
    )?;
    let num_outputs = c_int_arity_to_usize(
        unsafe { getNumOutputsCInterpreterDSPInstance(dsp) },
        "interp fixture heavy_bench output arity",
    )?;
    if num_inputs == 0 || num_outputs == 0 {
        unsafe {
            deleteCInterpreterDSPInstance(dsp);
            let _ = deleteCInterpreterDSPFactory(ffi_factory);
        }
        return Err(format!(
            "interp fixture heavy_bench reported invalid IO (in={num_inputs}, out={num_outputs})"
        ));
    }

    let (elapsed, checksum) =
        run_blocked_bench(num_inputs, num_outputs, |input_ptrs, output_ptrs| {
            unsafe {
                computeCInterpreterDSPInstance(dsp, BLOCK_SIZE as i32, input_ptrs, output_ptrs)
            };
        });

    unsafe {
        deleteCInterpreterDSPInstance(dsp);
        let _ = deleteCInterpreterDSPFactory(ffi_factory);
    }

    black_box(checksum);
    Ok((elapsed, checksum))
}

fn run_cranelift(input: BenchInput<'_>) -> Result<(Duration, f64), String> {
    match input {
        BenchInput::DspFile(case) => run_cranelift_file(case),
        BenchInput::FixtureSinePhasor => run_cranelift_fixture_sine_phasor(),
        BenchInput::FixtureHeavyBench => run_cranelift_fixture_heavy_bench(),
    }
}

fn run_cranelift_file(case: &Path) -> Result<(Duration, f64), String> {
    let c_path = CString::new(case.to_string_lossy().as_bytes())
        .map_err(|e| format!("CString path: {e}"))?;
    let mut err = [0_i8; 4096];
    let factory = unsafe {
        createCCraneliftDSPFactoryFromFile(
            c_path.as_ptr(),
            0,
            std::ptr::null(),
            err.as_mut_ptr(),
            1,
        )
    };
    if factory.is_null() {
        return Err(format!(
            "createCCraneliftDSPFactoryFromFile failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        ));
    }
    let dsp = unsafe { createCCraneliftDSPInstance(factory) };
    if dsp.is_null() {
        unsafe {
            let _ = deleteCCraneliftDSPFactory(factory);
        }
        return Err("createCCraneliftDSPInstance failed".to_owned());
    }
    unsafe { initCCraneliftDSPInstance(dsp, SAMPLE_RATE) };

    let num_inputs = c_int_arity_to_usize(
        unsafe { getNumInputsCCraneliftDSPInstance(dsp) },
        "cranelift input arity",
    )?;
    let num_outputs = c_int_arity_to_usize(
        unsafe { getNumOutputsCCraneliftDSPInstance(dsp) },
        "cranelift output arity",
    )?;
    let (elapsed, checksum) =
        run_blocked_bench(num_inputs, num_outputs, |input_ptrs, output_ptrs| {
            unsafe {
                computeCCraneliftDSPInstance(dsp, BLOCK_SIZE as i32, input_ptrs, output_ptrs)
            };
        });

    unsafe {
        deleteCCraneliftDSPInstance(dsp);
        let _ = deleteCCraneliftDSPFactory(factory);
    }

    black_box(checksum);
    Ok((elapsed, checksum))
}

fn run_cranelift_fixture_sine_phasor() -> Result<(Duration, f64), String> {
    let (store, module) = build_sine_phasor_test_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .map_err(|e| e.to_string())?;
    let compute_addr = compiled.compute_entry_addr();
    if compute_addr == 0 {
        return Err("compiled compute address is null".to_owned());
    }
    // SAFETY: `compute_addr` comes from Cranelift with the exact compute ABI.
    let compute: ComputeFn = unsafe { std::mem::transmute(compute_addr) };

    let layout = compiled.struct_layout();
    let dsp_state =
        OwnedAlignedDspState::new(layout.size_bytes() as usize, layout.align_bytes() as usize)?;

    let num_inputs = 0usize;
    let num_outputs = 1usize;
    let (elapsed, checksum) =
        run_blocked_bench(num_inputs, num_outputs, |input_ptrs, output_ptrs| {
            // SAFETY: pointer tables and dsp pointer respect compute ABI.
            unsafe {
                compute(
                    dsp_state.ptr.cast(),
                    BLOCK_SIZE as i32,
                    input_ptrs,
                    output_ptrs,
                )
            };
        });
    black_box(checksum);
    Ok((elapsed, checksum))
}

fn run_cranelift_fixture_heavy_bench() -> Result<(Duration, f64), String> {
    let (store, module) = build_heavy_bench_test_module();
    let compiled = generate_cranelift_module(&store, module, &CraneliftOptions::default())
        .map_err(|e| e.to_string())?;
    let compute_addr = compiled.compute_entry_addr();
    if compute_addr == 0 {
        return Err("compiled compute address is null".to_owned());
    }
    // SAFETY: `compute_addr` comes from Cranelift with the exact compute ABI.
    let compute: ComputeFn = unsafe { std::mem::transmute(compute_addr) };

    let layout = compiled.struct_layout();
    let dsp_state =
        OwnedAlignedDspState::new(layout.size_bytes() as usize, layout.align_bytes() as usize)?;

    let num_inputs = 1usize;
    let num_outputs = 1usize;
    let (elapsed, checksum) =
        run_blocked_bench(num_inputs, num_outputs, |input_ptrs, output_ptrs| {
            unsafe {
                compute(
                    dsp_state.ptr.cast(),
                    BLOCK_SIZE as i32,
                    input_ptrs,
                    output_ptrs,
                )
            };
        });
    black_box(checksum);
    Ok((elapsed, checksum))
}

fn main() {
    let args = CliArgs::parse();
    let input = match (args.dsp.as_deref(), args.fixture) {
        (Some(path), None) => BenchInput::DspFile(path),
        (None, Some(Fixture::SinePhasor)) => BenchInput::FixtureSinePhasor,
        (None, Some(Fixture::HeavyBench)) => BenchInput::FixtureHeavyBench,
        _ => unreachable!("Clap requires exactly one benchmark input"),
    };
    let total_samples = (BLOCK_SIZE * NUM_BLOCKS) as f64;

    let (interp_t, interp_sum) = run_interp(input).expect("interp bench");
    let (clif_t, clif_sum) = run_cranelift(input).expect("cranelift bench");

    let interp_ns_per_sample = interp_t.as_secs_f64() * 1e9 / total_samples;
    let clif_ns_per_sample = clif_t.as_secs_f64() * 1e9 / total_samples;
    let speedup = interp_ns_per_sample / clif_ns_per_sample;

    let label = match input {
        BenchInput::DspFile(path) => path.display().to_string(),
        BenchInput::FixtureSinePhasor => "fixture:sine_phasor".to_owned(),
        BenchInput::FixtureHeavyBench => "fixture:heavy_bench".to_owned(),
    };
    println!("DSP: {label}");
    println!(
        "Config: sr={} block_size={} blocks={} warmup_blocks={}",
        SAMPLE_RATE, BLOCK_SIZE, NUM_BLOCKS, WARMUP_BLOCKS
    );
    println!(
        "Interp   : {:>10.3} ms total | {:>8.3} ns/sample | checksum={:.6}",
        interp_t.as_secs_f64() * 1e3,
        interp_ns_per_sample,
        interp_sum
    );
    println!(
        "Cranelift: {:>10.3} ms total | {:>8.3} ns/sample | checksum={:.6}",
        clif_t.as_secs_f64() * 1e3,
        clif_ns_per_sample,
        clif_sum
    );
    println!("Speedup (Cranelift vs Interp): {:.2}x", speedup);
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::CliArgs;

    #[test]
    fn cli_requires_one_dsp_or_fixture() {
        CliArgs::command().debug_assert();
        assert!(CliArgs::try_parse_from(["compute_bench", "test.dsp"]).is_ok());
        assert!(CliArgs::try_parse_from(["compute_bench", "--fixture", "sine_phasor"]).is_ok());
        assert!(CliArgs::try_parse_from(["compute_bench"]).is_err());
        assert!(
            CliArgs::try_parse_from(["compute_bench", "test.dsp", "--fixture", "heavy_bench"])
                .is_err()
        );
    }
}
