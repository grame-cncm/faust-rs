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
//! own directory>` plus any `--extra-include` directories, because many
//! examples import their neighbours (or, for `--per-symbol`, import from a
//! directory above the file itself). Each is run `--repeats` times and the
//! **minimum** is kept: scheduler noise, page faults and neighbouring work can
//! only add time, so the minimum is the robust estimator for "this run met no
//! interference". This is the same convention `compile_budget` uses.
//!
//! # Per-symbol mode
//!
//! Some corpora have no `process` at all: `faustlibraries/tests/*.dsp` is a
//! flat list of one-liners per file, each an independent regression case
//! meant to be selected individually — `db2linear_test =
//! ba.db2linear(-6);`, `linear2db_test = ba.linear2db(0.5);`, and so on, one
//! per exercised library function, never combined into one `process`.
//! Compiling such a file whole either fails outright (no `process`) or,
//! worse, silently compiles nothing meaningful. `--per-symbol` instead scans
//! each file for top-level `<name><suffix>` definitions (`--symbol-suffix`,
//! default `_test`) and compiles each one on its own via `-pn
//! <name><suffix>` — the same flag Faust uses to select a non-`process`
//! entry point — turning "does this file compile" into "does every case in
//! it compile", which is what these files are actually for.
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

/// One compile to run: a file, and optionally which top-level definition to
/// select as `process` via `-pn` (`--per-symbol` mode; `None` compiles the
/// file as-is, using whatever `process` it already defines).
struct WorkItem {
    dsp: PathBuf,
    process_name: Option<String>,
}

/// Runs one compiler on one input, returning success and the best wall time.
///
/// Output goes to a scratch path rather than the input's directory so a run
/// never writes into the corpus being measured. `process_name`, when set,
/// passes `-pn <name>` so a file with no `process` (or with several
/// independent candidates) can still be compiled by picking one.
fn measure(
    bin: &Path,
    input: &Path,
    includes: &[&Path],
    process_name: Option<&str>,
    out: &Path,
    repeats: u32,
) -> (bool, u128) {
    let mut best = u128::MAX;
    let mut ok = false;
    for _ in 0..repeats.max(1) {
        let started = Instant::now();
        let mut cmd = Command::new(bin);
        cmd.arg(input);
        for include in includes {
            cmd.arg("-I").arg(include);
        }
        if let Some(name) = process_name {
            cmd.arg("-pn").arg(name);
        }
        cmd.arg("-o")
            .arg(out)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = cmd.status();
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

/// Scans one file for top-level `<name><suffix>` definitions.
///
/// Deliberately line-based rather than a real Faust parse: the corpus this
/// serves (`faustlibraries/tests/*.dsp`) writes exactly one definition per
/// line, unindented, so `<identifier><suffix> = ...` at column 0 is enough to
/// find every case without depending on the compiler under test to parse
/// correctly first — the whole point is finding cases *before* compiling
/// them. Indented lines are skipped so a same-named local inside a `with{}`
/// block is never mistaken for a top-level case, and a name is accepted only
/// when every character is alphanumeric/`_`, which rejects a parameterized
/// definition like `foo_test(x) = ...` — `-pn` needs a fully-applied signal
/// expression, not a function.
fn collect_symbols(path: &Path, suffix: &str) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with(char::is_whitespace) || line.starts_with("//") {
            continue;
        }
        let Some(eq_idx) = line.find('=') else {
            continue;
        };
        // Skip `==` (comparison), not a definition.
        if line[eq_idx..].starts_with("==") {
            continue;
        }
        let name = line[..eq_idx].trim_end();
        let is_identifier = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if is_identifier && name.ends_with(suffix) {
            names.push(name.to_owned());
        }
    }
    names
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
    // Per-symbol mode expands one file into dozens or hundreds of independent
    // compiles; a `--repeats 3` timing default meant for whole files would
    // triple an already large run for no benefit these cases are too small
    // and fast to need noise-averaging for pass/fail.
    let default_repeats = if args.per_symbol { 1 } else { 3 };
    let repeats = args.repeats.unwrap_or(default_repeats).max(1);
    let inputs = collect_inputs(&root, args.filter.as_deref())?;
    let symbol_suffix = args.symbol_suffix.as_deref().unwrap_or("_test");

    let work: Vec<WorkItem> = if args.per_symbol {
        inputs
            .iter()
            .flat_map(|input| {
                let symbols = collect_symbols(input, symbol_suffix);
                symbols.into_iter().map(move |name| WorkItem {
                    dsp: input.clone(),
                    process_name: Some(name),
                })
            })
            .collect()
    } else {
        inputs
            .iter()
            .map(|input| WorkItem {
                dsp: input.clone(),
                process_name: None,
            })
            .collect()
    };
    if work.is_empty() {
        return Err(format!(
            "no `<name>{symbol_suffix}` definitions found under {} (is --per-symbol needed, \
             or is --symbol-suffix wrong?)",
            root.display()
        )
        .into());
    }

    println!(
        "examples-compare: {} case{} under {}, {repeats} run(s) each, keeping the minimum",
        work.len(),
        if work.len() == 1 { "" } else { "s" },
        root.display()
    );
    if args.per_symbol {
        println!(
            "  per-symbol   : {} file(s), `-pn <name{symbol_suffix}>` per case",
            inputs.len()
        );
    }
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

    let mut rows = Vec::with_capacity(work.len());
    for item in &work {
        let own_dir = item.dsp.parent().unwrap_or(&root);
        let mut includes: Vec<&Path> = vec![own_dir];
        includes.extend(args.extra_include.iter().map(PathBuf::as_path));
        let process_name = item.process_name.as_deref();
        let (cpp_ok, cpp_ms) = measure(
            &cpp_bin,
            &item.dsp,
            &includes,
            process_name,
            &cpp_out,
            repeats,
        );
        let (rs_ok, rs_ms) = measure(
            &rs_bin,
            &item.dsp,
            &includes,
            process_name,
            &rs_out,
            repeats,
        );
        let dsp_label = item
            .dsp
            .strip_prefix(&root)
            .unwrap_or(&item.dsp)
            .to_string_lossy()
            .into_owned();
        rows.push(CaseRow {
            dsp: match &item.process_name {
                Some(name) => format!("{dsp_label}::{name}"),
                None => dsp_label,
            },
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

#[cfg(test)]
mod tests {
    use super::collect_symbols;
    use std::fs;

    fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "faust-rs-examples-compare-{name}-{}-{}.dsp",
            std::process::id(),
            line!()
        ));
        fs::write(&path, contents).expect("write temp fixture");
        path
    }

    #[test]
    fn finds_one_top_level_case_per_line() {
        let path = write_temp(
            "basics",
            r#"
ba = library("basics.lib");
samp2sec_test = ba.samp2sec(512);
db2linear_test = ba.db2linear(-6);
"#,
        );
        let names = collect_symbols(&path, "_test");
        fs::remove_file(&path).ok();
        assert_eq!(names, vec!["samp2sec_test", "db2linear_test"]);
    }

    #[test]
    fn skips_comments_indentation_and_comparisons() {
        let path = write_temp(
            "skip",
            r#"
// commented_out_test = 1;
    indented_test = 1;
flag_test == 1;
real_test = 1;
"#,
        );
        let names = collect_symbols(&path, "_test");
        fs::remove_file(&path).ok();
        assert_eq!(names, vec!["real_test"]);
    }

    #[test]
    fn rejects_parameterized_definitions() {
        // `-pn` needs a fully-applied signal expression; a function taking
        // arguments can't stand alone as `process`.
        let path = write_temp("param", "foo_test(x) = x + 1;\nbar_test = foo_test(1);\n");
        let names = collect_symbols(&path, "_test");
        fs::remove_file(&path).ok();
        assert_eq!(names, vec!["bar_test"]);
    }

    #[test]
    fn honors_a_custom_suffix() {
        let path = write_temp("suffix", "foo_case = 1;\nbar_test = 2;\n");
        let names = collect_symbols(&path, "_case");
        fs::remove_file(&path).ok();
        assert_eq!(names, vec!["foo_case"]);
    }
}
