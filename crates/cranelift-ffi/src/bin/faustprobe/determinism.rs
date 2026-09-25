//! Private subprocess protocol for `--check determinism`.
//!
//! The C API coalesces factories with equal FIR/options keys, dropping the
//! second JIT. A separate process preserves the C++-compatible cache contract
//! while ensuring the second render executes its own compilation. Samples
//! travel as integer bit patterns, including signed zero, without decimal
//! rounding. No files, cache invalidation, or public Rust API are needed.

use std::process::Command;

use cranelift_ffi::probe::compare::Samples;
use cranelift_ffi::probe::engine::{Probe, RenderSpec};

use crate::cli::Args;
use crate::setup::{build_schedule, compile_program, parse_assignment, parse_input_at};
use crate::verify::render_for_comparison;

pub(crate) fn render(args: &Args) -> Result<(String, Samples), String> {
    let executable = std::env::current_exe().map_err(|e| format!("--check determinism: {e}"))?;
    let mut command = Command::new(executable);
    command.arg("--determinism-worker");
    for (flag, value) in [
        ("--sr", args.sr.to_string()),
        ("--block", args.block.to_string()),
        ("--render", args.render.to_string()),
        ("--skip", args.skip.to_string()),
        ("--opt-level", args.opt_level.to_string()),
        ("--bra-tape", args.bra_tape.to_string()),
        ("--in", args.input.clone()),
    ] {
        command.arg(format!("{flag}={value}"));
    }
    if args.double {
        command.arg("--double");
    }
    for (flag, values) in [
        ("--import-dir", &args.import_dirs),
        ("--eval", &args.evals),
        ("--set", &args.sets),
        ("--set", &args.set_a),
    ] {
        for value in values {
            command.arg(format!("{flag}={value}"));
        }
    }
    for pair in args.ats.chunks(2) {
        command.arg("--at").args(pair);
    }
    // The compiler's spellings, which the worker's own normalization reads back.
    command.args(args.compiler.argv());
    let output = command
        .arg("--")
        .arg(&args.file)
        .output()
        .map_err(|e| format!("--check determinism: cannot run independent process: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "--check determinism: independent process failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        ));
    }
    decode(&output.stdout)
}

pub(crate) fn worker(args: &Args) -> Result<(), String> {
    let (factory, _) = compile_program(args, args.double)?;
    let key = factory.sha_key();
    let probe = Probe::instantiate(&std::rc::Rc::new(factory), args.sr)?;
    let spec = RenderSpec {
        frames: args.render,
        block: args.block,
        input: parse_input_at(&args.input, args.sr, probe.inputs())?,
        skip: args.skip,
        schedule: build_schedule(args)?,
        ..RenderSpec::default()
    };
    let sets = args
        .sets
        .iter()
        .map(|s| parse_assignment(s))
        .collect::<Result<Vec<_>, _>>()?;
    let samples = render_for_comparison(&probe, &spec, &sets, "--check determinism")?;
    serde_json::to_writer(std::io::stdout().lock(), &encode(&key, &samples))
        .map_err(|e| format!("--check determinism: cannot write render: {e}"))
}

fn encode(key: &str, samples: &Samples) -> serde_json::Value {
    let bits: Vec<Vec<u64>> = samples
        .channels
        .iter()
        .map(|channel| channel.iter().map(|value| value.to_bits()).collect())
        .collect();
    serde_json::json!({
        "version": 1,
        "pid": std::process::id(),
        "key": key,
        "start": samples.start,
        "bits": bits,
    })
}

fn decode(bytes: &[u8]) -> Result<(String, Samples), String> {
    let invalid = || "--check determinism: invalid independent render response".to_owned();
    let report: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if report["version"] != 1
        || report["pid"]
            .as_u64()
            .is_none_or(|pid| pid == 0 || pid == u64::from(std::process::id()))
    {
        return Err(invalid());
    }
    let key = report["key"].as_str().ok_or_else(invalid)?.to_owned();
    let start: usize = serde_json::from_value(report["start"].clone()).map_err(|_| invalid())?;
    let bits: Vec<Vec<u64>> =
        serde_json::from_value(report["bits"].clone()).map_err(|_| invalid())?;
    if bits
        .first()
        .is_some_and(|first| bits.iter().any(|channel| channel.len() != first.len()))
    {
        return Err(invalid());
    }
    let channels = bits
        .into_iter()
        .map(|channel| channel.into_iter().map(f64::from_bits).collect())
        .collect();
    Ok((key, Samples { start, channels }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_preserves_bits_and_requires_another_process() {
        let samples = Samples {
            start: 7,
            channels: vec![vec![0.0, -0.0, f64::MIN_POSITIVE, 1.0 / 3.0]],
        };
        let mut report = encode("key", &samples);
        assert!(decode(&serde_json::to_vec(&report).unwrap()).is_err());
        report["pid"] = serde_json::json!(u64::from(std::process::id()) + 1);
        let (key, restored) = decode(&serde_json::to_vec(&report).unwrap()).unwrap();
        assert_eq!(key, "key");
        assert_eq!(restored.start, 7);
        for (a, b) in restored.channels[0].iter().zip(&samples.channels[0]) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}
