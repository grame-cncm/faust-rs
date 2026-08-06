//! Side-by-side comparison of `faust-rs` and C++ Faust over a DSP tree.
//!
//! Answers two questions the impulse lanes do not: **what does the reference
//! compile that we do not**, and **how does compile time compare on a corpus
//! nobody tuned against**. `tests/impulse-tests/dsp` exists to be a numerical
//! gate and has been shaped by that; the reference `examples/` tree has not,
//! which is why a propagation blow-up invisible on the impulse corpus showed up
//! there immediately.
//!
//! # Method
//!
//! Both compilers run as subprocesses on the same input with `-I <the file's
//! own directory>`, because many examples import their neighbours. Each is run
//! `--repeats` times and the **minimum** is kept: scheduler noise, page faults
//! and neighbouring work can only add time, so the minimum is the robust
//! estimator for "this run met no interference". This is the same convention
//! `compile_budget` uses.
//!
//! # What the numbers do and do not support
//!
//! The totals are dominated by the expensive files and are the reliable figure.
//! The per-DSP median is not: most examples compile in a few milliseconds, and
//! at that scale process startup and the timer's resolution swamp the
//! measurement. A per-DSP ratio is only worth reading when both sides are well
//! above that floor, which is why the slow-case tables filter on it.
//!
//! # Which C++ binary you compare against changes the answer
//!
//! `resolve_cpp_faust_bin` prefers `FAUST_CPP_BIN`, then the local
//! `build/bin/faust`, then `faust` on `PATH` — and those are not
//! interchangeable. Measured 2026-08-06 over this same corpus: the local build
//! totals 98.7 s where the installed `/usr/local/bin/faust` totals 74.6 s, a
//! third slower, presumably built with different optimisation settings. A ratio
//! quoted without naming the reference binary is not reproducible, so the
//! command prints which one it used and `--faust-bin` pins it.
//!
//! This is a *comparison*, not a gate. It has no baseline and never fails on
//! timing — regressions in compile cost belong to `compile-budget-check`, and
//! per-stage attribution to `compile-profile`.

use super::*;
use std::time::Instant;

const DEFAULT_EXAMPLES_ROOT: &str = "/Users/letz/faust/examples";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CaseRow {
    dsp: String,
    cpp_ok: bool,
    cpp_ms: u128,
    rs_ok: bool,
    rs_ms: u128,
}

/// Runs one compiler on one input, returning success and the best wall time.
///
/// Output goes to a scratch path rather than the input's directory so a run
/// never writes into the corpus being measured.
fn measure(bin: &Path, input: &Path, include: &Path, out: &Path, repeats: u32) -> (bool, u128) {
    let mut best = u128::MAX;
    let mut ok = false;
    for _ in 0..repeats.max(1) {
        let started = Instant::now();
        let status = Command::new(bin)
            .arg(input)
            .arg("-I")
            .arg(include)
            .arg("-o")
            .arg(out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let elapsed = started.elapsed().as_millis();
        ok = matches!(&status, Ok(s) if s.success());
        best = best.min(elapsed);
        // A failing input fails deterministically and fast; repeating it only
        // measures the error path.
        if !ok {
            break;
        }
    }
    (ok, best)
}

fn collect_inputs(root: &Path, filter: Option<&str>) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "dsp") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    if let Some(needle) = filter {
        out.retain(|p| p.to_string_lossy().contains(needle));
    }
    if out.is_empty() {
        return Err(format!("no .dsp found under {}", root.display()));
    }
    Ok(out)
}

pub(crate) fn examples_compare(
    args: ExamplesCompareArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = args
        .root
        .unwrap_or_else(|| PathBuf::from(DEFAULT_EXAMPLES_ROOT));
    if !root.is_dir() {
        return Err(format!("examples root {} does not exist", root.display()).into());
    }
    let (cpp_bin, from_path) = match args.faust_bin {
        Some(explicit) => (explicit, false),
        None => resolve_cpp_faust_bin(),
    };
    let rs_bin = args
        .faust_rs_bin
        .unwrap_or_else(|| workspace_root().join("target/release/faust-rs"));
    if !rs_bin.exists() {
        return Err(format!(
            "{} not found; build it with `cargo build --release -p compiler --bin faust-rs`",
            rs_bin.display()
        )
        .into());
    }
    let repeats = args.repeats.unwrap_or(3).max(1);
    let inputs = collect_inputs(&root, args.filter.as_deref())?;

    println!(
        "examples-compare: {} DSP under {}, {repeats} run(s) each, keeping the minimum",
        inputs.len(),
        root.display()
    );
    println!(
        "  C++ reference: {}{}",
        cpp_bin.display(),
        if from_path { " (from PATH)" } else { "" }
    );
    println!("  faust-rs     : {}", rs_bin.display());

    let scratch = std::env::temp_dir().join("faust-rs-examples-compare");
    fs::create_dir_all(&scratch)?;
    let cpp_out = scratch.join("cpp.cpp");
    let rs_out = scratch.join("rs.cpp");

    let mut rows = Vec::with_capacity(inputs.len());
    for input in &inputs {
        let include = input.parent().unwrap_or(&root);
        let (cpp_ok, cpp_ms) = measure(&cpp_bin, input, include, &cpp_out, repeats);
        let (rs_ok, rs_ms) = measure(&rs_bin, input, include, &rs_out, repeats);
        rows.push(CaseRow {
            dsp: input
                .strip_prefix(&root)
                .unwrap_or(input)
                .to_string_lossy()
                .into_owned(),
            cpp_ok,
            cpp_ms,
            rs_ok,
            rs_ms,
        });
    }

    report(&rows, args.top.unwrap_or(10));

    if let Some(path) = &args.csv {
        let mut text = String::from("dsp,cpp_status,cpp_ms,rs_status,rs_ms\n");
        for r in &rows {
            let _ = writeln!(
                text,
                "{},{},{},{},{}",
                r.dsp,
                if r.cpp_ok { "ok" } else { "fail" },
                r.cpp_ms,
                if r.rs_ok { "ok" } else { "fail" },
                r.rs_ms
            );
        }
        fs::write(path, text)?;
        println!("\nexamples-compare: wrote {}", path.display());
    }
    Ok(())
}

fn report(rows: &[CaseRow], top: usize) {
    let both: Vec<&CaseRow> = rows.iter().filter(|r| r.cpp_ok && r.rs_ok).collect();
    let cpp_only: Vec<&CaseRow> = rows.iter().filter(|r| r.cpp_ok && !r.rs_ok).collect();
    let rs_only: Vec<&CaseRow> = rows.iter().filter(|r| !r.cpp_ok && r.rs_ok).collect();
    let neither = rows.len() - both.len() - cpp_only.len() - rs_only.len();

    println!("\ncompilation");
    println!("  both            {}", both.len());
    println!("  C++ only        {}", cpp_only.len());
    println!("  faust-rs only   {}", rs_only.len());
    println!("  neither         {neither}");
    for r in &cpp_only {
        println!("    faust-rs fails: {}", r.dsp);
    }
    for r in &rs_only {
        println!("    C++ fails:      {}", r.dsp);
    }

    let cpp_total: u128 = both.iter().map(|r| r.cpp_ms).sum();
    let rs_total: u128 = both.iter().map(|r| r.rs_ms).sum();
    if cpp_total == 0 {
        return;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "millisecond totals are far below f64's exact-integer range"
    )]
    let ratio = rs_total as f64 / cpp_total as f64;
    println!("\ncompile time over the {} that both compile", both.len());
    println!(
        "  C++ {:.2}s   faust-rs {:.2}s   ratio {ratio:.2}x",
        cpp_total as f64 / 1000.0,
        rs_total as f64 / 1000.0
    );

    // Per-DSP ratios are only meaningful above the timing floor; below it the
    // figure is process startup and timer granularity, not compilation.
    const FLOOR_MS: u128 = 100;
    let mut comparable: Vec<(&CaseRow, f64)> = both
        .iter()
        .filter(|r| r.cpp_ms >= FLOOR_MS)
        .map(|r| (*r, r.rs_ms as f64 / r.cpp_ms as f64))
        .collect();
    println!(
        "  {} of them are above the {FLOOR_MS} ms floor where a per-DSP ratio means something; \
         faust-rs is faster on {}",
        comparable.len(),
        comparable.iter().filter(|(_, x)| *x < 1.0).count()
    );

    let mut slowest: Vec<&CaseRow> = both.clone();
    slowest.sort_by_key(|r| std::cmp::Reverse(r.rs_ms));
    println!("\nslowest for faust-rs");
    for r in slowest.iter().take(top) {
        let x = if r.cpp_ms > 0 {
            format!("{:.2}x", r.rs_ms as f64 / r.cpp_ms as f64)
        } else {
            "-".to_owned()
        };
        println!(
            "  {:<52} cpp {:>7.2}s  rs {:>7.2}s  {x:>6}",
            r.dsp,
            r.cpp_ms as f64 / 1000.0,
            r.rs_ms as f64 / 1000.0
        );
    }

    comparable.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    println!("\nworst ratios above the floor");
    for (r, x) in comparable.iter().take(top) {
        println!(
            "  {:<52} cpp {:>7.2}s  rs {:>7.2}s  {x:>5.2}x",
            r.dsp,
            r.cpp_ms as f64 / 1000.0,
            r.rs_ms as f64 / 1000.0
        );
    }
}
