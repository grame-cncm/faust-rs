//! Numeric differential of clocked programs against the C++ clock-domain
//! reference branch (`master-dev-ocpp-od-fir-2-FIR19`, commit `8eebea429`).
//!
//! Roadmap validation surface (synthesis §10): *"Clock domains (base) |
//! Differential vs branch binary `8eebea429`"*. For each reference clocked
//! program we compile the DSP with **both** the C++ branch compiler and
//! faust-rs, run each on the **same deterministic input**, and assert the
//! output matches sample-for-sample.
//!
//! To avoid replicating the C++ impulse architecture's RNG / 4-pass protocol,
//! this uses a **minimal deterministic architecture** (`DIFF_ARCH`): the input
//! `in[c][i] = ((i·7 + c·3) mod 11) − 5` is generated identically on both
//! sides (all values are produced before any modulo of a negative, so C++ and
//! Rust agree bit-for-bit on the input), fed in one `compute` call, and every
//! output sample printed.
//!
//! The test **skips gracefully** when the C++ branch binary or a C++ compiler
//! is unavailable. Point at the branch binary with `FAUST_OD_BIN` (else the
//! default build path is tried); the architecture headers are located next to
//! it.

use std::path::{Path, PathBuf};
use std::process::Command;

use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};
use compiler::{Compiler, SignalFirLane};

/// The C++ clock-domain reference branch checkout (contains the FIR19 OD work).
const DEFAULT_FAUST_ROOT: &str = "/Users/letz/Developpements/RUST/faust";

/// Minimal deterministic architecture: fixed integer input, all outputs
/// printed one frame per line. `<<includeIntrinsic>>` / `<<includeclass>>` are
/// filled by faust.
const DIFF_ARCH: &str = r#"#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif
#include <cstdio>
#include <cstdlib>
#include <cmath>
#include <vector>
#include "faust/dsp/dsp.h"
#include "faust/gui/UI.h"
#include "faust/gui/meta.h"
struct NullUI : public UI {
  void openTabBox(const char*){} void openHorizontalBox(const char*){} void openVerticalBox(const char*){}
  void closeBox(){}
  void addButton(const char*, FAUSTFLOAT*){} void addCheckButton(const char*, FAUSTFLOAT*){}
  void addVerticalSlider(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT){}
  void addHorizontalSlider(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT){}
  void addNumEntry(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT){}
  void addHorizontalBargraph(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT){}
  void addVerticalBargraph(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT){}
  void addSoundfile(const char*, const char*, Soundfile**){}
  void declare(FAUSTFLOAT*, const char*, const char*){}
};
struct NullMeta : public Meta { void declare(const char*, const char*){} };
<<includeIntrinsic>>
<<includeclass>>
int main(int argc, char** argv) {
  int nframes = (argc > 1) ? atoi(argv[1]) : 48;
  mydsp dsp; NullMeta m; dsp.metadata(&m);
  dsp.init(48000);
  NullUI ui; dsp.buildUserInterface(&ui);
  int ni = dsp.getNumInputs(), no = dsp.getNumOutputs();
  std::vector<std::vector<FAUSTFLOAT>> in(ni, std::vector<FAUSTFLOAT>(nframes));
  std::vector<std::vector<FAUSTFLOAT>> out(no, std::vector<FAUSTFLOAT>(nframes));
  for (int c = 0; c < ni; c++) for (int i = 0; i < nframes; i++)
      in[c][i] = (FAUSTFLOAT)(((i*7 + c*3) % 11) - 5);
  std::vector<FAUSTFLOAT*> ip(ni), op(no);
  for (int c=0;c<ni;c++) ip[c]=in[c].data();
  for (int c=0;c<no;c++) op[c]=out[c].data();
  dsp.compute(nframes, ip.data(), op.data());
  for (int i = 0; i < nframes; i++) { for (int c = 0; c < no; c++) printf("%.6f ", (double)out[c][i]); printf("\n"); }
  return 0;
}
"#;

/// Same deterministic input the architecture generates, for `num_inputs`
/// channels over `nframes` frames. `i*7 + c*3` is non-negative, so `% 11`
/// matches C++ exactly.
fn deterministic_inputs(num_inputs: usize, nframes: usize) -> Vec<Vec<f32>> {
    (0..num_inputs)
        .map(|c| {
            (0..nframes)
                .map(|i| ((i * 7 + c * 3) % 11) as f32 - 5.0)
                .collect()
        })
        .collect()
}

fn faust_root() -> Option<PathBuf> {
    let root = std::env::var_os("FAUST_OD_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FAUST_ROOT));
    root.exists().then_some(root)
}

fn faust_od_bin(root: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FAUST_OD_BIN") {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    let p = root.join("build/bin/faust");
    p.exists().then_some(p)
}

fn cxx() -> Option<String> {
    let candidate = std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
    Command::new(&candidate)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| candidate)
}

struct RefEnv {
    faust: PathBuf,
    arch_include: PathBuf,
    cxx: String,
}

fn reference_env() -> Option<RefEnv> {
    let root = faust_root()?;
    let faust = faust_od_bin(&root)?;
    let arch_include = root.join("architecture");
    if !arch_include.join("faust/dsp/dsp.h").exists() {
        return None;
    }
    let cxx = cxx()?;
    Some(RefEnv {
        faust,
        arch_include,
        cxx,
    })
}

/// Compiles `source` with the C++ branch compiler + the minimal architecture,
/// builds it, runs it for `nframes`, and returns the printed output matrix
/// `[output_channel][frame]`.
fn run_cpp_reference(env: &RefEnv, stem: &str, source: &str, nframes: usize) -> Vec<Vec<f32>> {
    let dir = std::env::temp_dir().join(format!("faust-rs-oddiff-{stem}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create work dir");
    let dsp_path = dir.join("prog.dsp");
    let arch_path = dir.join("arch.cpp");
    let cpp_path = dir.join("prog.cpp");
    let bin_path = dir.join("prog");
    std::fs::write(&dsp_path, source).expect("write dsp");
    std::fs::write(&arch_path, DIFF_ARCH).expect("write arch");

    let gen_out = Command::new(&env.faust)
        .args(["-lang", "cpp", "-i", "-a"])
        .arg(&arch_path)
        .arg("-I")
        .arg(&env.arch_include)
        .arg(&dsp_path)
        .arg("-o")
        .arg(&cpp_path)
        .output()
        .expect("run faust");
    assert!(
        gen_out.status.success(),
        "{stem}: C++ branch faust failed: {}",
        String::from_utf8_lossy(&gen_out.stderr)
    );

    let build = Command::new(&env.cxx)
        .args(["-O2", "-std=c++14", "-I"])
        .arg(&env.arch_include)
        .arg(&cpp_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("run c++");
    assert!(
        build.status.success(),
        "{stem}: reference C++ build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin_path)
        .arg(nframes.to_string())
        .output()
        .expect("run reference binary");
    assert!(run.status.success(), "{stem}: reference run failed");
    let text = String::from_utf8_lossy(&run.stdout);
    let mut cols: Vec<Vec<f32>> = Vec::new();
    for line in text.lines() {
        let vals: Vec<f32> = line
            .split_whitespace()
            .map(|t| t.parse::<f32>().expect("parse output float"))
            .collect();
        if vals.is_empty() {
            continue;
        }
        if cols.is_empty() {
            cols = vec![Vec::new(); vals.len()];
        }
        for (c, v) in vals.into_iter().enumerate() {
            cols[c].push(v);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    cols
}

fn cpp_codegen_stderr(env: &RefEnv, stem: &str, source: &str) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!(
        "faust-rs-oddiff-codegen-{stem}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create work dir");
    let dsp_path = dir.join("prog.dsp");
    let arch_path = dir.join("arch.cpp");
    let cpp_path = dir.join("prog.cpp");
    std::fs::write(&dsp_path, source).expect("write dsp");
    std::fs::write(&arch_path, DIFF_ARCH).expect("write arch");

    let gen_out = Command::new(&env.faust)
        .args(["-lang", "cpp", "-i", "-a"])
        .arg(&arch_path)
        .arg("-I")
        .arg(&env.arch_include)
        .arg(&dsp_path)
        .arg("-o")
        .arg(&cpp_path)
        .output()
        .expect("run faust");
    let _ = std::fs::remove_dir_all(&dir);
    (
        gen_out.status.success(),
        String::from_utf8_lossy(&gen_out.stderr).into_owned(),
    )
}

/// Runs `source` through the faust-rs interpreter fast lane on the same
/// deterministic input.
fn run_faust_rs(stem: &str, source: &str, num_inputs: usize, nframes: usize) -> Vec<Vec<f32>> {
    let path = std::env::temp_dir().join(format!(
        "faust-rs-oddiff-rs-{stem}-{}.dsp",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("write dsp");
    let fbc = Compiler::new()
        .compile_file_default_to_interp_with_lane(
            &path,
            &InterpOptions::default(),
            SignalFirLane::TransformFastLane,
        )
        .unwrap_or_else(|e| panic!("{stem}: faust-rs compilation failed: {e}"));
    let _ = std::fs::remove_file(&path);

    let inputs = deterministic_inputs(num_inputs, nframes);
    let input_slices: Vec<&[f32]> = inputs.iter().map(Vec::as_slice).collect();
    let mut reader = std::io::Cursor::new(fbc);
    let mut factory = read_fbc::<f32>(&mut reader).expect("fbc parse");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(48_000);
    let no = usize::try_from(instance.get_num_outputs()).expect("outputs");
    let mut outputs = vec![vec![0.0_f32; nframes]; no];
    let mut slices: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    instance
        .try_compute(nframes as i32, &input_slices, &mut slices)
        .expect("faust-rs execution");
    outputs
}

fn assert_differential(stem: &str, source: &str, num_inputs: usize) {
    let Some(env) = reference_env() else {
        eprintln!("Skipping {stem}: C++ clock-domain reference branch unavailable");
        return;
    };
    let nframes = 64;
    let cpp = run_cpp_reference(&env, stem, source, nframes);
    let rs = run_faust_rs(stem, source, num_inputs, nframes);
    assert_eq!(
        cpp.len(),
        rs.len(),
        "{stem}: output channel count differs (cpp {}, rs {})",
        cpp.len(),
        rs.len()
    );
    for (c, (cc, rc)) in cpp.iter().zip(rs.iter()).enumerate() {
        for t in 0..nframes {
            let (a, b) = (cc[t], rc[t]);
            let tol = 1.0e-4_f32 * (1.0 + a.abs());
            assert!(
                (a - b).abs() <= tol,
                "{stem}: divergence at output {c} frame {t}: C++ {a} vs faust-rs {b}"
            );
        }
    }
}

#[test]
fn ondemand_accumulator_matches_cpp_reference() {
    // Boolean-clocked accumulator: fires when the (deterministic) clock is even.
    assert_differential(
        "od_accum",
        r#"process = (((_ % 2) == 0), _) : ondemand(+ ~ _);"#,
        2,
    );
}

#[test]
fn ondemand_with_inner_delay_matches_cpp_reference() {
    // Inner delay inside the block exercises per-domain fire-time state.
    assert_differential(
        "od_delay",
        r#"process = (((_ % 2) == 0), _) : ondemand(_ <: _, @(3) :> +);"#,
        2,
    );
}

#[test]
fn ondemand_domain_free_payload_matches_cpp_reference() {
    // Regression for held payloads whose state does not read a domain-internal
    // input. The state still belongs to the held Clocked payload and advances
    // only when the OD guard fires.
    assert_differential(
        "od_domain_free_payload",
        r#"process = (((_ % 2) == 0), (_ : !)) : ondemand(1 : (+ ~ _));"#,
        2,
    );
}

#[test]
fn integer_ondemand_domain_free_payload_matches_cpp_reference() {
    // Integer OD repeats the held payload in fire time. A domain-free payload
    // must not be hoisted to the outer sample loop.
    assert_differential(
        "int_od_domain_free_payload",
        r#"process = (3, (_ : !)) : ondemand(1 : (+ ~ _));"#,
        1,
    );
}

#[test]
fn ondemand_domain_free_circular_recursion_matches_cpp_reference() {
    // Long feedback delay forces circular recursion storage. The per-domain
    // cursor must advance on OD fires, not at the outer sample rate.
    assert_differential(
        "od_domain_free_circular_rec",
        r#"process = (((_ % 2) == 0), (_ : !)) : ondemand(1 : (+ ~ @(20)));"#,
        2,
    );
}

#[test]
fn unparenthesized_independent_ondemand_domains_reject_like_cpp() {
    let source = r#"process =
            (((_ % 2) == 0), (_ : !)) : ondemand(1 : (+ ~ _)),
            (((_ % 3) == 0), (_ : !)) : ondemand(10 : (+ ~ _));"#;

    if let Some(env) = reference_env() {
        let (ok, stderr) = cpp_codegen_stderr(&env, "od_unparenthesized_reject", source);
        assert!(
            !ok && stderr.contains("sequential composition"),
            "C++ reference should reject the unparenthesized form as sequential composition, stderr:\n{stderr}"
        );
    } else {
        eprintln!("Skipping C++ half of unparenthesized OD rejection: reference unavailable");
    }

    let err = Compiler::new()
        .compile_source_to_signals("od_unparenthesized_reject.dsp", source)
        .expect_err("faust-rs must reject the unparenthesized form like C++");
    let rendered = err.to_string();
    assert!(
        rendered.contains("sequential composition"),
        "faust-rs rejection should name sequential composition, got:\n{rendered}"
    );
}

#[test]
fn independent_ondemand_domains_match_cpp_reference() {
    // Two held payloads with different clocks must keep independent domains,
    // hold fields, and fire-time state.
    assert_differential(
        "od_two_independent_domains",
        r#"od2 = (((_ % 2) == 0), (_ : !)) : ondemand(1 : (+ ~ _));
           od3 = (((_ % 3) == 0), (_ : !)) : ondemand(10 : (+ ~ _));
           process = od2, od3;"#,
        4,
    );
}

#[test]
fn upsampling_matches_cpp_reference() {
    // Integer-clocked upsampling (counted inner loop + zero-padded input).
    assert_differential("us_accum", r#"process = (2, _) : upsampling(+ ~ _);"#, 1);
}

#[test]
fn upsampling_domain_free_payload_matches_cpp_reference() {
    // Counted-loop version of the held-payload rule: the payload recursion
    // advances once per inner fire even though it does not consume the outer
    // input value.
    assert_differential(
        "us_domain_free_payload",
        r#"process = (2, (_ : !)) : upsampling(1 : (+ ~ _));"#,
        1,
    );
}

#[test]
fn downsampling_matches_cpp_reference() {
    // Integer-clocked downsampling (modulo firing guard).
    assert_differential("ds_accum", r#"process = (2, _) : downsampling(+ ~ _);"#, 1);
}

#[test]
fn downsampling_domain_free_payload_matches_cpp_reference() {
    // Downsampling fires once every N outer ticks. The held payload's state is
    // still in the downsampled domain even when it ignores the outer input.
    assert_differential(
        "ds_domain_free_payload",
        r#"process = (3, (_ : !)) : downsampling(1 : (+ ~ _));"#,
        1,
    );
}
