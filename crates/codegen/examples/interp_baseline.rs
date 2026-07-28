use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{ArgGroup, Parser, ValueEnum};
use codegen::backends::interp::{
    FbcDspFactory, FbcDspInstance, InterpOptions, generate_interp_module, read_fbc,
};
use codegen::fixtures::{build_heavy_bench_test_module, build_sine_phasor_test_module};

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
    name = "interp_baseline",
    about = "Profile the interpreter on built-in fixtures or one FBC file",
    group(ArgGroup::new("input").multiple(false).args(["fixture", "fbc"]))
)]
struct CliArgs {
    /// Run one built-in FIR fixture.
    #[arg(long, value_enum)]
    fixture: Option<Fixture>,

    /// Load and run one textual FBC file.
    #[arg(long, value_name = "PATH")]
    fbc: Option<PathBuf>,
}

fn i32_arity_to_usize(value: i32, label: &str) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| format!("invalid negative {label}: {value}"))
}

fn prepare_impulse_block(input_channels: &mut [Vec<f32>], global_start: usize) {
    for (ch_idx, channel) in input_channels.iter_mut().enumerate() {
        channel.fill(0.0);
        if global_start == 0 && !channel.is_empty() {
            channel[0] = 1.0f32 + ch_idx as f32;
        }
    }
}

fn run_profiled_baseline(name: &str, mut factory: FbcDspFactory<f32>) -> Result<(), String> {
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(SAMPLE_RATE);

    let num_inputs = i32_arity_to_usize(instance.get_num_inputs(), "input arity")?;
    let num_outputs = i32_arity_to_usize(instance.get_num_outputs(), "output arity")?;
    if num_outputs == 0 {
        return Err(format!("{name}: invalid output arity 0"));
    }

    let mut input_channels = vec![vec![0.0f32; BLOCK_SIZE]; num_inputs];
    let mut output_channels = vec![vec![0.0f32; BLOCK_SIZE]; num_outputs];

    // Warmup (not counted in timed/profiled section).
    for block_idx in 0..WARMUP_BLOCKS {
        prepare_impulse_block(&mut input_channels, block_idx * BLOCK_SIZE);
        for ch in &mut output_channels {
            ch.fill(0.0);
        }
        let input_refs: Vec<&[f32]> = input_channels.iter().map(Vec::as_slice).collect();
        let mut output_refs: Vec<&mut [f32]> =
            output_channels.iter_mut().map(Vec::as_mut_slice).collect();
        instance
            .try_compute(BLOCK_SIZE as i32, &input_refs, &mut output_refs)
            .map_err(|e| format!("{name}: warmup failed: {e}"))?;
    }

    let t0 = Instant::now();
    let mut checksum = 0.0f64;
    for block_idx in WARMUP_BLOCKS..(WARMUP_BLOCKS + NUM_BLOCKS) {
        prepare_impulse_block(&mut input_channels, block_idx * BLOCK_SIZE);
        for ch in &mut output_channels {
            ch.fill(0.0);
        }
        let input_refs: Vec<&[f32]> = input_channels.iter().map(Vec::as_slice).collect();
        let mut output_refs: Vec<&mut [f32]> =
            output_channels.iter_mut().map(Vec::as_mut_slice).collect();
        instance
            .try_compute(BLOCK_SIZE as i32, &input_refs, &mut output_refs)
            .map_err(|e| format!("{name}: compute failed: {e}"))?;

        checksum += output_channels
            .iter()
            .flat_map(|ch| ch.iter())
            .map(|x| f64::from(*x))
            .sum::<f64>();
    }
    let elapsed = t0.elapsed();

    let total_samples = (NUM_BLOCKS * BLOCK_SIZE) as f64;
    let ns_per_sample = elapsed.as_secs_f64() * 1e9 / total_samples;

    println!("=== {name} ===");
    println!(
        "sr={} block={} warmup_blocks={} blocks={}",
        SAMPLE_RATE, BLOCK_SIZE, WARMUP_BLOCKS, NUM_BLOCKS
    );
    println!(
        "io: in={} out={} | time={:.3}ms | ns/sample={:.3} | checksum={:.6}",
        num_inputs,
        num_outputs,
        elapsed.as_secs_f64() * 1e3,
        ns_per_sample,
        checksum
    );
    println!();

    Ok(())
}

fn load_fixture_factory(which: &str) -> Result<(String, FbcDspFactory<f32>), String> {
    let (store, module) = match which {
        "sine_phasor" => build_sine_phasor_test_module(),
        "heavy_bench" => build_heavy_bench_test_module(),
        _ => {
            return Err(format!(
                "unknown fixture '{which}', expected 'sine_phasor' or 'heavy_bench'"
            ));
        }
    };
    let factory = generate_interp_module::<f32>(&store, module, &InterpOptions::default())
        .map_err(|e| e.to_string())?;
    Ok((format!("fixture:{which}"), factory))
}

fn load_fbc_factory(path: &Path) -> Result<(String, FbcDspFactory<f32>), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let mut reader = BufReader::new(text.as_bytes());
    let factory = read_fbc::<f32>(&mut reader).map_err(|e| e.to_string())?;
    Ok((format!("fbc:{}", path.display()), factory))
}

fn main() -> Result<(), String> {
    let args = CliArgs::parse();
    if args.fixture.is_none() && args.fbc.is_none() {
        let (name1, f1) = load_fixture_factory("sine_phasor")?;
        run_profiled_baseline(&name1, f1)?;
        let (name2, f2) = load_fixture_factory("heavy_bench")?;
        run_profiled_baseline(&name2, f2)?;
        return Ok(());
    }

    match (args.fixture, args.fbc) {
        (Some(fixture), None) => {
            let name = match fixture {
                Fixture::SinePhasor => "sine_phasor",
                Fixture::HeavyBench => "heavy_bench",
            };
            let (id, factory) = load_fixture_factory(name)?;
            run_profiled_baseline(&id, factory)
        }
        (None, Some(path)) => {
            let (id, factory) = load_fbc_factory(&path)?;
            run_profiled_baseline(&id, factory)
        }
        (None, None) => unreachable!("handled default fixture pair"),
        (Some(_), Some(_)) => unreachable!("Clap input group rejects multiple sources"),
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::CliArgs;

    #[test]
    fn cli_accepts_default_fixture_and_fbc_modes() {
        CliArgs::command().debug_assert();
        assert!(CliArgs::try_parse_from(["interp_baseline"]).is_ok());
        assert!(CliArgs::try_parse_from(["interp_baseline", "--fixture", "sine_phasor"]).is_ok());
        assert!(CliArgs::try_parse_from(["interp_baseline", "--fbc", "test.fbc"]).is_ok());
        assert!(
            CliArgs::try_parse_from([
                "interp_baseline",
                "--fixture",
                "heavy_bench",
                "--fbc",
                "test.fbc"
            ])
            .is_err()
        );
    }
}
