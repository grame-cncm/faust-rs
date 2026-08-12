//! CLI transcript differential for `faust-rs` (cleanup plan 2026-07-26, §3).
//!
//! Refactoring the CLI is only safe if "the output did not change" is checked
//! rather than eyeballed. This captures a transcript — stdout, stderr and exit
//! status — for a fixed matrix of inputs × modes, and compares it against a
//! stored snapshot.
//!
//! ```text
//! cargo run -p xtask -- cli-transcript-gen     # record the snapshot
//! cargo run -p xtask -- cli-transcript-check   # compare against it
//! ```
//!
//! When output changes *on purpose*, re-record with `-gen` and let the diff be
//! reviewed. The snapshot is generated, never hand-edited.
//!
//! # This is a local tool, not a CI gate
//!
//! 35 of the 148 snapshots embed machine-specific absolute paths, so the
//! snapshot only reproduces on the machine that recorded it. That is why
//! `cli-transcript-check` is absent from `.github/workflows/ci.yml`.
//!
//! Three distinct sources, none of them a defect:
//!
//! 1. **Diagnostics** (29 snapshots, all `bad_*`). `compile_file_to_signals`
//!    canonicalizes the input path to key the metadata store, and the
//!    resulting absolute path reaches the rendered diagnostic.
//! 2. **`include_pathnames` in the JSON description** (4 snapshots).
//!    `paths.rs` derives an executable-relative `../share/faust`, which under
//!    a cargo layout is `<repo>/target/share/faust`.
//! 3. **The wasm binaries** (2 snapshots). They carry that same JSON as a data
//!    segment, so the path is in the *bytes*.
//!
//! Source 3 is what rules out fixing this by rewriting the recorded text: the
//! data segment's length is LEB128-encoded, so a substitution that changes the
//! path's length invalidates the prefix, and two machines with different path
//! lengths produce genuinely different bytes that no post-hoc rule reconciles.
//!
//! Note also that `include_pathnames` lists `/usr/local/share/faust` and
//! `/usr/share/faust`, so even a repo-root rewrite would not make the snapshot
//! portable to Windows.
//!
//! # Staying local is a decision, not a pending fix (2026-07-26)
//!
//! A CI-gated variant would need a declared *portable subset*: 113 of the 148
//! modes carry no machine-specific path. That was considered and **rejected**,
//! because the 35 excluded modes are all the `bad_*` ones — the error
//! diagnostics, which is the coverage nothing else in the repo provides. CI
//! would gain the half that `golden-check` already largely covers and lose the
//! half that is unique here.
//!
//! So this is a refactor tool: run it by hand while reworking the CLI or the
//! facade, and leave it alone otherwise. Do not wire it into CI without
//! revisiting that trade-off first.
//!
//! Its real failure mode is human, and worth naming: when output changes you
//! re-record, and a re-record accepted without reading the diff *launders a
//! regression into an intentional change*. The diff is the check — the
//! snapshot is only what makes the diff possible.
//!
//! If it ever needs strengthening, the weak axis is breadth of programs (three
//! DSPs), not modes.
//!
//! # Two traps this harness is built to avoid
//!
//! 1. **Inputs must sit at a path of fixed length.** The source path is
//!    embedded in the wasm module's JSON, so a directory whose name changes
//!    length between runs shifts LEB128 length prefixes inside the binary.
//!    Normalizing the path in the *text* then hides the cause while leaving the
//!    difference visible. The DSP inputs are therefore written to one fixed
//!    directory under `target/`, never into the per-run output directory.
//! 2. **Genuinely unstable fields must be normalized, and only those.** The
//!    Cranelift report prints a runtime `compute_entry_addr`, which differs on
//!    every execution; it is replaced by a placeholder. Nothing else is
//!    rewritten, so a real change cannot hide behind a normalization rule.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the recorded transcript lives.
const SNAPSHOT_DIR: &str = "tests/cli-transcripts";

/// Fixed-length home for the generated DSP inputs (see trap 1 in the module
/// docs). Kept inside `target/` so it is never mistaken for a source fixture.
const INPUT_DIR: &str = "target/cli-transcript-inputs";

/// The DSP programs the matrix runs against: a plain UI program, a recursive
/// one (state, delay lines), and one that must fail to compile.
const INPUTS: [(&str, &str); 3] = [
    ("simple", "process = _ * hslider(\"g\",0.5,0,1,0.01);\n"),
    ("rec", "process = _ : + ~ *(0.5) : @(3);\n"),
    ("bad", "process = undefined_thing;\n"),
];

/// Backends reachable through `--lang`.
const LANGS: [&str; 12] = [
    "cpp",
    "c",
    "rust",
    "julia",
    "asc",
    "codebox",
    "codebox-test",
    "interp",
    "cranelift",
    "wasm",
    "wast",
    "fir",
];

/// Single-flag modes that do not take a value.
const DUMP_FLAGS: [&str; 13] = [
    "--export-dsp",
    "--dump-cpp",
    "--dump-c",
    "--dump-fir",
    "--dump-sig",
    "--dump-box",
    "--parse",
    "--check",
    "--golden",
    "--json",
    "--dump-fir-verify",
    "--dump-interp",
    "--dump-cranelift",
];

/// Option combinations worth pinning because they cross backend boundaries.
const OPTION_RUNS: [(&str, &[&str]); 8] = [
    ("double_cpp", &["--lang", "cpp", "-double"]),
    ("vec_cpp", &["--lang", "cpp", "-vec", "-vs", "8"]),
    ("ec_os_cpp", &["--lang", "cpp", "-ec", "-os"]),
    ("ss3_cpp", &["--lang", "cpp", "-ss", "3"]),
    ("cn_cpp", &["--lang", "cpp", "-cn", "Custom"]),
    // Codebox forces external control and one-sample, so these two must record
    // byte-identical output to the plain `--lang codebox` run above.
    ("ec_os_codebox", &["--lang", "codebox", "-ec", "-os"]),
    ("double_codebox", &["--lang", "codebox", "-double"]),
    // `codebox-test` must resolve to the `codebox` capability row: looking
    // one up under the `-test` spelling fails closed and would reject this
    // perfectly valid command line.
    ("ec_codebox_test", &["--lang", "codebox-test", "-ec"]),
];

/// Invalid command lines. Which message wins is part of the contract, because
/// the order of the checks in `validate_cli_arguments` decides it.
const ERROR_RUNS: [(&str, &[&str]); 10] = [
    // `-e` emits Faust source, not generated code, so pairing it with a
    // backend has no meaning; it must be rejected rather than have one of the
    // two silently win.
    ("err_export_with_lang", &["-e", "--lang", "cpp", "INPUT"]),
    ("err_no_input", &["--lang", "cpp"]),
    ("err_two_modes", &["--dump-cpp", "--dump-c", "INPUT"]),
    ("err_empty_cn", &["--lang", "cpp", "-cn", "", "INPUT"]),
    (
        "err_lane_with_box",
        &["--dump-box", "--signal-fir-lane", "fast", "INPUT"],
    ),
    (
        "err_fixture_input",
        &["--fir-fixture", "sine_phasor", "--lang", "cpp", "INPUT"],
    ),
    ("err_bad_ss", &["--lang", "cpp", "-ss", "abc", "INPUT"]),
    ("err_os_vec", &["--lang", "cpp", "-os", "-vec", "INPUT"]),
    // `-vec` alone, with neither `-ec` nor `-os`: the path that used to return
    // Ok before any backend lookup happened.
    ("err_vec_codebox", &["--lang", "codebox", "-vec", "INPUT"]),
    (
        "err_nofirverify",
        &["--lang", "cpp", "--no-fir-verify", "--check", "INPUT"],
    ),
];

/// Records the transcript into [`SNAPSHOT_DIR`], replacing what is there.
pub fn cli_transcript_gen() -> Result<(), Box<dyn std::error::Error>> {
    let transcript = capture()?;
    let dir = Path::new(SNAPSHOT_DIR);
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)?;
    for (name, body) in &transcript {
        fs::write(dir.join(format!("{name}.txt")), body)?;
    }
    println!(
        "cli-transcript-gen: recorded {} transcripts in {SNAPSHOT_DIR}",
        transcript.len()
    );
    Ok(())
}

/// Compares the current binary against the recorded transcript.
pub fn cli_transcript_check() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new(SNAPSHOT_DIR);
    if !dir.is_dir() {
        return Err(format!(
            "{SNAPSHOT_DIR} does not exist — run `cargo run -p xtask -- cli-transcript-gen` first"
        )
        .into());
    }
    let transcript = capture()?;
    let mut findings: Vec<String> = Vec::new();
    for (name, body) in &transcript {
        let path = dir.join(format!("{name}.txt"));
        match fs::read_to_string(&path) {
            Ok(recorded) if &recorded == body => {}
            Ok(_) => findings.push(format!(
                "{name}: output differs from the recorded transcript"
            )),
            Err(_) => findings.push(format!("{name}: no recorded transcript")),
        }
    }
    let recorded_count = fs::read_dir(dir)?.count();
    if recorded_count != transcript.len() {
        findings.push(format!(
            "recorded transcript has {recorded_count} entries, the matrix produced {}",
            transcript.len()
        ));
    }
    findings.sort();
    if findings.is_empty() {
        println!(
            "cli-transcript-check: OK ({} modes identical)",
            transcript.len()
        );
        return Ok(());
    }
    for finding in &findings {
        eprintln!("cli-transcript-check: {finding}");
    }
    Err(format!("cli-transcript-check: {} finding(s)", findings.len()).into())
}

/// Runs the whole matrix and returns `(name, normalized transcript)` pairs,
/// sorted by name so the result is deterministic.
fn capture() -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let binary = build_cli()?;
    let inputs = write_inputs()?;
    let mut out: Vec<(String, String)> = Vec::new();

    for (stem, path) in &inputs {
        for lang in LANGS {
            out.push((
                format!("{stem}_lang_{lang}"),
                run(&binary, &["--lang", lang, path]),
            ));
        }
        for flag in DUMP_FLAGS {
            let label = flag.trim_start_matches('-').replace('-', "_");
            out.push((format!("{stem}_{label}"), run(&binary, &[flag, path])));
        }
        for (label, args) in OPTION_RUNS {
            let mut argv: Vec<&str> = args.to_vec();
            argv.push(path);
            out.push((format!("{stem}_{label}"), run(&binary, &argv)));
        }
    }

    let simple = &inputs[0].1;
    for (label, args) in ERROR_RUNS {
        let argv: Vec<&str> = args
            .iter()
            .map(|a| if *a == "INPUT" { simple.as_str() } else { *a })
            .collect();
        out.push((label.to_owned(), run(&binary, &argv)));
    }
    out.push(("help".to_owned(), run(&binary, &["--help"])));
    out.push(("version".to_owned(), run(&binary, &["--version"])));
    out.push((
        "list_fixtures".to_owned(),
        run(&binary, &["--list-fir-fixtures"]),
    ));

    // The fixture ladder, over every built-in fixture.
    let listing = run(&binary, &["--list-fir-fixtures"]);
    for fixture in listing
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(str::to_owned)
        .collect::<Vec<_>>()
    {
        for lang in ["cpp", "c", "interp", "cranelift", "fir"] {
            out.push((
                format!("fixture_{fixture}_{lang}"),
                run(&binary, &["--fir-fixture", &fixture, "--lang", lang]),
            ));
        }
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Builds the CLI under test and returns its path.
fn build_cli() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--quiet", "-p", "compiler", "--bin", "faust-rs"])
        .status()?;
    if !status.success() {
        return Err("failed to build the faust-rs binary".into());
    }
    Ok(PathBuf::from("target/debug/faust-rs"))
}

/// Writes the DSP inputs to the fixed-length directory (see trap 1).
fn write_inputs() -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let dir = Path::new(INPUT_DIR);
    fs::create_dir_all(dir)?;
    let mut inputs = Vec::new();
    for (stem, body) in INPUTS {
        let path = dir.join(format!("{stem}.dsp"));
        fs::write(&path, body)?;
        inputs.push((stem.to_owned(), path.to_string_lossy().into_owned()));
    }
    Ok(inputs)
}

/// Runs the CLI once and returns its transcript: stdout, then stderr, then the
/// exit status, with the runtime-varying fields normalized.
fn run(binary: &Path, args: &[&str]) -> String {
    let output = Command::new(binary).args(args).output();
    let text = match output {
        Ok(output) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&output.stdout));
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            text.push_str(&format!(
                "EXIT={}\n",
                output.status.code().unwrap_or_default()
            ));
            text
        }
        Err(error) => format!("SPAWN_FAILED: {error}\n"),
    };
    // The Cranelift entry address is a runtime pointer: normalize it, and only
    // it, so nothing else can hide behind a rewriting rule.
    //
    // Scanned forward into a fresh string on purpose. Replacing in place and
    // re-running `find` from the start does not terminate: the replacement text
    // still contains the search key, so every pass matches it again.
    const KEY: &str = "compute_entry_addr: 0x";
    let mut normalized = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(index) = rest.find(KEY) {
        normalized.push_str(&rest[..index + KEY.len()]);
        normalized.push_str("NORMALIZED");
        let after = &rest[index + KEY.len()..];
        let hex_len = after
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(after.len());
        rest = &after[hex_len..];
    }
    normalized.push_str(rest);
    normalized
}
