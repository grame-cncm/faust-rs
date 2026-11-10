//! The C++ backend with its large methods outlined (`--outline`, plan
//! `porting/cpp-outlining-plan-2026-11-10-en.md`).
//!
//! - Shape: a program under the budget is emitted byte for byte as without
//!   the option; above it, the `ondemand` blocks of the sample loop become
//!   methods called on `this`, a local stored into is passed as
//!   `T* RESTRICT` by address, a channel pointer by value without
//!   `RESTRICT` (a host may process in place).
//! - Coverage: every hermetic program of `tests/corpus` and
//!   `tests/impulse-tests/dsp` (no `import`/`library`, AGENTS.md §3) emits
//!   C++ under a policy that outlines every block it can, and under one that
//!   also cuts straight-line runs through spill arrays, whenever it emits
//!   C++ at all.
//! - Numeric: each program the two policies change is built with the
//!   in-repository minimal architecture (`tests/impulse-tests/archs`) as
//!   three classes in one translation unit — plain, every block, every run —
//!   and the three must compute bit-identical outputs on the same input and
//!   control changes. Skipped when no C++ compiler is found (`CXX`, else
//!   `c++`).

use std::path::{Path, PathBuf};
use std::process::Command;

use codegen::backends::cpp::CppOptions;
use compiler::Compiler;
use fir::outline::OutlinePolicy;

/// Every `then`/`else` block the pass can outline.
const EVERY_BLOCK: OutlinePolicy = OutlinePolicy {
    budget: 1,
    min_block: 1,
    split_budget: usize::MAX,
    pointer_params: true,
    promote_references: true,
};

/// Phase 2 too: every block over 8 statements cut into runs of at most 8.
const EVERY_RUN: OutlinePolicy = OutlinePolicy {
    budget: 8,
    min_block: 1,
    split_budget: 8,
    pointer_params: true,
    promote_references: true,
};

/// The C++ policy of the CLI (`--outline`): pointers by value, references
/// promoted to fields.
fn cli_policy() -> OutlinePolicy {
    OutlinePolicy {
        pointer_params: true,
        promote_references: true,
        ..OutlinePolicy::default()
    }
}

fn cpp(name: &str, source: &str, class: &str, outline: Option<OutlinePolicy>) -> Option<String> {
    let options = CppOptions {
        class_name: Some(class.to_owned()),
        outline,
        ..CppOptions::default()
    };
    Compiler::new()
        .compile_source_to_cpp(name, source, &options)
        .ok()
}

/// A bank of gated channels, each a delay line with feedback and a gain
/// inside `ondemand`, shaped like the user program that motivated the
/// option (`n * n` channels, each reading its input inside its block).
fn gated_bank(n: usize) -> String {
    format!(
        r#"line(i, j) = (gate, _) : ondemand((+ : @(d)) ~ *(fb) : *(g) : lp)
with {{
    k = i * {n} + j;
    gate = checkbox("gate%k");
    d = hslider("d%k", 10, 1, 60, 1);
    fb = hslider("fb%k", 0.5, 0, 0.9, 0.01);
    g = hslider("g%k", 0.8, 0, 1, 0.01);
    lp = + ~ *(0.3);
}};
process = par(i, {n}, (_ <: par(j, {n}, line(i, j)))) :> par(c, {n}, _);"#
    )
}

#[test]
fn a_program_under_the_budget_is_emitted_unchanged() {
    let source = gated_bank(2);
    let plain = cpp("bank", &source, "mydsp", None).expect("C++");
    let outlined = cpp("bank", &source, "mydsp", Some(cli_policy())).expect("C++");
    assert_eq!(plain, outlined);
    assert!(!plain.contains("Outline"));
}

#[test]
fn ondemand_blocks_become_methods_called_on_this() {
    let source = gated_bank(2);
    let text = cpp("bank", &source, "mydsp", Some(EVERY_BLOCK)).expect("C++");
    let signature = text
        .lines()
        .find(|l| l.trim_start().starts_with("void computeOutline0("))
        .unwrap_or_else(|| panic!("no outlined method:\n{text}"));
    // The input channel is read inside the block: a pointer by value, with
    // no `RESTRICT`, since a host may pass the same buffer as an output.
    assert!(
        signature.contains("FAUSTFLOAT* input0"),
        "input channel pointer by value: {signature}"
    );
    assert!(
        !signature.contains("RESTRICT input0"),
        "a channel pointer is not restricted: {signature}"
    );
    // The loop counter by value, no receiver argument.
    assert!(signature.contains("int i0"), "{signature}");
    assert!(!signature.contains("dsp"), "{signature}");
    // The call passes the input channel by value, under the gate's `if`.
    let call = text
        .lines()
        .find(|l| l.trim_start().starts_with("computeOutline0("))
        .unwrap_or_else(|| panic!("no call:\n{text}"));
    assert!(call.contains("input0"), "{call}");
    assert!(!call.contains("dsp"), "{call}");
}

#[test]
fn a_local_stored_into_is_passed_by_address_and_restricted() {
    // The block-rate computations of an `ondemand` body are done once per
    // block, into locals of `compute` the block stores into (DIFF-BEH-024).
    let source = r#"gate = checkbox("gate");
f = hslider("f", 1000, 20, 20000, 1);
c = cos(f * 0.0001) : sqrt : log;
process = (gate, _) : ondemand(*(c) : + ~ *(0.5));"#;
    let without_promotion = OutlinePolicy {
        promote_references: false,
        ..EVERY_BLOCK
    };
    let text = cpp("refs", source, "mydsp", Some(without_promotion)).expect("C++");
    let has_restricted = text
        .lines()
        .any(|l| l.trim_start().starts_with("void computeOutline") && l.contains("* RESTRICT "));
    let has_address = text
        .lines()
        .any(|l| l.trim_start().starts_with("computeOutline") && l.contains("(&"));
    assert!(
        has_restricted && has_address,
        "no local passed by address:\n{text}"
    );
}

#[test]
fn a_local_only_passed_to_its_block_becomes_a_member() {
    // As above, with references promoted (the CLI policy): the block-rate
    // value and its flag are members reset at the head of `compute`, and the
    // call passes no address.
    let source = r#"gate = checkbox("gate");
f = hslider("f", 1000, 20, 20000, 1);
c = cos(f * 0.0001) : sqrt : log;
process = (gate, _) : ondemand(*(c) : + ~ *(0.5));"#;
    let text = cpp("refs", source, "mydsp", Some(EVERY_BLOCK)).expect("C++");
    for member in ["    float fSlowOD0;", "    int iSlowODDone0;"] {
        assert!(
            text.lines().any(|l| l == member),
            "no member `{member}`:\n{text}"
        );
    }
    assert!(
        text.lines().any(|l| l.trim() == "iSlowODDone0 = 0;"),
        "the flag is reset in compute:\n{text}"
    );
    assert!(
        !text
            .lines()
            .any(|l| l.trim_start().starts_with("computeOutline") && l.contains('&')),
        "an address is still passed:\n{text}"
    );
}

/// The hermetic `.dsp` files of a repository directory, sorted.
fn hermetic(dir: &str) -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(dir);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "dsp"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(&path).ok()?;
            if source.contains("import(") || source.contains("library(") {
                return None;
            }
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some((name, source))
        })
        .collect()
}

/// `(plain, every block, every run)` C++ of one program, `None` when it does
/// not compile to C++ at all. Panics if an outlined form fails where the
/// plain one succeeds.
fn three_forms(name: &str, source: &str) -> Option<(String, String, String)> {
    let plain = cpp(name, source, "plain", None)?;
    let blocks = cpp(name, source, "blocks", Some(EVERY_BLOCK))
        .unwrap_or_else(|| panic!("{name}: emits C++ but not with every block outlined"));
    let runs = cpp(name, source, "runs", Some(EVERY_RUN))
        .unwrap_or_else(|| panic!("{name}: emits C++ but not with every run outlined"));
    Some((plain, blocks, runs))
}

fn outlined(text: &str) -> bool {
    text.contains("Outline0(")
}

#[test]
fn every_hermetic_program_emits_cpp_when_outlined() {
    let (mut emitted, mut changed) = (0, 0);
    for dir in ["tests/corpus", "tests/impulse-tests/dsp"] {
        for (name, source) in hermetic(dir) {
            if let Some((_, blocks, runs)) = three_forms(&name, &source) {
                emitted += 1;
                changed += usize::from(outlined(&blocks) || outlined(&runs));
            }
        }
    }
    eprintln!("{emitted} programs emitted, {changed} changed by outlining");
    // Kept from going vacuous: counts of 2026-11-10, rounded down.
    assert!(emitted >= 280, "{emitted} programs emitted");
    assert!(changed >= 220, "{changed} programs outlined");
}

// ------------------------------------------------------------ numeric

fn cxx() -> Option<String> {
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
    Command::new(&cxx)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
        .then_some(cxx)
}

/// Runs the three classes on the same input and control changes, and
/// returns non-zero at the first output that differs bit for bit.
const DRIVER: &str = r#"
#include <cstdio>
#include <cstring>
#include <vector>
struct Zones : public UI {
  std::vector<FAUSTFLOAT*> toggles, sliders;
  void openTabBox(const char*) {} void openHorizontalBox(const char*) {}
  void openVerticalBox(const char*) {} void closeBox() {}
  void addButton(const char*, FAUSTFLOAT* z) { toggles.push_back(z); }
  void addCheckButton(const char*, FAUSTFLOAT* z) { toggles.push_back(z); }
  void addVerticalSlider(const char*, FAUSTFLOAT* z, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT) { sliders.push_back(z); }
  void addHorizontalSlider(const char*, FAUSTFLOAT* z, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT) { sliders.push_back(z); }
  void addNumEntry(const char*, FAUSTFLOAT* z, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT) { sliders.push_back(z); }
  void addHorizontalBargraph(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT) {}
  void addVerticalBargraph(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT) {}
  void addSoundfile(const char*, const char*, Soundfile**) {}
  void declare(FAUSTFLOAT*, const char*, const char*) {}
};
template <class D> std::vector<FAUSTFLOAT> run() {
  D d; d.init(48000); Zones z; d.buildUserInterface(&z);
  const int frames = 64, blocks = 6;
  int ni = d.getNumInputs(), no = d.getNumOutputs();
  std::vector<std::vector<FAUSTFLOAT>> in(ni, std::vector<FAUSTFLOAT>(frames)), out(no, std::vector<FAUSTFLOAT>(frames));
  std::vector<FAUSTFLOAT*> ip(ni), op(no);
  for (int c = 0; c < ni; c++) ip[c] = in[c].data();
  for (int c = 0; c < no; c++) op[c] = out[c].data();
  std::vector<FAUSTFLOAT> all;
  for (int b = 0; b < blocks; b++) {
    for (size_t k = 0; k < z.toggles.size(); k++) *z.toggles[k] = FAUSTFLOAT(((b + k) % 3) != 0);
    if (b == 3) for (size_t k = 0; k < z.sliders.size(); k++) *z.sliders[k] = *z.sliders[k] * FAUSTFLOAT(0.75);
    for (int c = 0; c < ni; c++) for (int i = 0; i < frames; i++)
      in[c][i] = FAUSTFLOAT((((b * frames + i) * 7 + c * 3) % 11) - 5) * FAUSTFLOAT(0.1);
    d.compute(frames, ip.data(), op.data());
    for (int c = 0; c < no; c++) all.insert(all.end(), out[c].begin(), out[c].end());
  }
  return all;
}
static bool same(const std::vector<FAUSTFLOAT>& a, const std::vector<FAUSTFLOAT>& b) {
  return a.size() == b.size() && (a.empty() || std::memcmp(a.data(), b.data(), a.size() * sizeof(FAUSTFLOAT)) == 0);
}
int main() {
  std::vector<FAUSTFLOAT> p = run<plain>(), k = run<blocks>(), r = run<runs>();
  if (!same(p, k)) { std::printf("every block differs\n"); return 1; }
  if (!same(p, r)) { std::printf("every run differs\n"); return 2; }
  return 0;
}
"#;

#[test]
fn outlined_programs_compute_bit_identical_outputs() {
    let Some(cxx) = cxx() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    let architecture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/impulse-tests/archs");
    let dir = std::env::temp_dir().join(format!("faust-rs-cpp-outline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let mut programs: Vec<(String, String)> = hermetic("tests/impulse-tests/dsp")
        .into_iter()
        .filter(|(_, source)| !source.contains("soundfile"))
        .collect();
    programs.push(("gated_bank_3".to_owned(), gated_bank(3)));
    let mut checked = 0;
    for (name, source) in programs {
        let Some((plain, blocks, runs)) = three_forms(&name, &source) else {
            continue;
        };
        if !outlined(&blocks) && !outlined(&runs) {
            continue;
        }
        let stem = name.trim_end_matches(".dsp").replace(['-', '.'], "_");
        let unit = dir.join(format!("{stem}.cpp"));
        let text = format!(
            "#include \"faust_minimal.h\"\n{plain}\n#undef FAUSTCLASS\n{blocks}\n#undef \
             FAUSTCLASS\n{runs}\n{DRIVER}"
        );
        std::fs::write(&unit, text).expect("write unit");
        let exe = dir.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
        let build = Command::new(&cxx)
            .args(["-std=c++14", "-O1", "-I"])
            .arg(&architecture)
            .arg(&unit)
            .arg("-o")
            .arg(&exe)
            .output()
            .expect("run the C++ compiler");
        assert!(
            build.status.success(),
            "{name}: the outlined C++ does not build:\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&exe).output().expect("run the program");
        assert!(
            run.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&run.stdout)
        );
        checked += 1;
    }
    std::fs::remove_dir_all(&dir).expect("remove temp dir");
    eprintln!("{checked} outlined programs bit-identical to their plain build");
    // Kept from going vacuous: 48 on 2026-11-10.
    assert!(checked >= 45, "{checked} programs checked");
}
