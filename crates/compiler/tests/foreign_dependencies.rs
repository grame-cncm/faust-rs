//! A foreign function's header reaches the generated C and C++ (W3 of
//! `porting/foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md`).
//!
//! The header operand of `ffunction` and `fvariable` used to be dropped while
//! lowering to FIR, so the generated code called a function nothing declared
//! and did not build. It now travels as a `DeclareDependency` node in the
//! module's `globals`, from table generators too, and the C-family backends
//! print it where C++ `produceClass` does.
//!
//! The header and the programs are local to the test. The generated code is
//! compiled against the repository's minimal architecture
//! (`tests/impulse-tests/archs`) and run; without a C++ compiler only the text
//! is checked.

use std::path::{Path, PathBuf};
use std::process::Command;

use compiler::{Compiler, SignalFirLane};
use fir::{FirDependencyKind, FirMatch, match_fir};

/// A static-inline function and a variable: the header is the only definition.
const HEADER: &str = "#ifndef W3_H\n#define W3_H\n\
    static inline float w3_add_one(float x) { return x + 1.0f; }\n\
    static float w3_gain = 2.0f;\n\
    #endif\n";

/// One call per sample, one in `instanceConstants`, a `<...>` header, and the
/// same header named twice.
const MIXED: &str = "f = ffunction(float w3_add_one(float), \"w3.h\", \"\");\n\
    g = ffunction(float w3_add_one(float), \"w3.h\", \"\");\n\
    s = ffunction(float sinhf|sinh|sinhl(float), <math.h>, \"\");\n\
    process = f(2), g, s;\n";

/// A call inside a table generator, and in the program that reads the table,
/// with a library to link with.
const GENERATOR: &str = "f = ffunction(float w3_add_one(float), \"w3.h\", \"w3lib\");\n\
    t = (+(1) ~ _) - 1;\n\
    process = rdtable(4, f(float(t)), 2) + f(0);\n";

/// A foreign variable, which the header declares.
const VARIABLE: &str = "process = fvariable(float w3_gain, \"w3.h\");\n";

/// A function whose header operand is empty.
const NO_HEADER: &str = "process = ffunction(float w3_add_one(float), \"\", \"\");\n";

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_faust-rs"))
}

fn arch_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/impulse-tests/archs")
}

/// A fresh directory holding `w3.h`.
fn work_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("faust_rs_w3_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("w3.h"), HEADER).expect("write header");
    dir
}

/// Generates `lang` code for `source` with `extra` flags, into `dir`.
fn generate(dir: &Path, stem: &str, source: &str, lang: &str, extra: &[&str]) -> PathBuf {
    let dsp = dir.join(format!("{stem}.dsp"));
    std::fs::write(&dsp, source).expect("write dsp");
    let out = dir.join(format!("{stem}.{lang}"));
    let output = Command::new(bin_path())
        .arg(&dsp)
        .args(["-lang", lang])
        .args(extra)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("run faust-rs");
    assert!(
        output.status.success(),
        "{stem} -lang {lang}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    out
}

fn cxx() -> Option<String> {
    let candidate = std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
    Command::new(&candidate)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|_| candidate)
}

/// A driver around the generated class (C++) or functions (C): the impulse
/// on every input, four frames, every output printed per frame.
fn driver(lang: &str) -> &'static str {
    if lang == "cpp" {
        "#include <cstdio>\n#include <vector>\n#include \"faust_minimal.h\"\n#include GENERATED\n\
         int main() {\n  mydsp d; d.init(48000);\n\
         int ni = d.getNumInputs(), no = d.getNumOutputs(); const int n = 4;\n\
         std::vector<std::vector<FAUSTFLOAT>> in(ni, std::vector<FAUSTFLOAT>(n, 0)), out(no, std::vector<FAUSTFLOAT>(n, 0));\n\
         for (auto& c : in) c[0] = 1;\n\
         std::vector<FAUSTFLOAT*> ip, op;\n\
         for (auto& c : in) ip.push_back(c.data());\n\
         for (auto& c : out) op.push_back(c.data());\n\
         d.compute(n, ip.data(), op.data());\n\
         for (int i = 0; i < n; i++) { for (int c = 0; c < no; c++) printf(\"%g \", (double)out[c][i]); printf(\"\\n\"); }\n}\n"
    } else {
        "#include <cstdio>\n#include <vector>\n#include \"faust_minimal_cglue.h\"\n#include GENERATED\n\
         int main() {\n  mydsp* d = newmydsp(); initmydsp(d, 48000);\n\
         int ni = getNumInputsmydsp(d), no = getNumOutputsmydsp(d); const int n = 4;\n\
         std::vector<std::vector<FAUSTFLOAT>> in(ni, std::vector<FAUSTFLOAT>(n, 0)), out(no, std::vector<FAUSTFLOAT>(n, 0));\n\
         for (auto& c : in) c[0] = 1;\n\
         std::vector<FAUSTFLOAT*> ip, op;\n\
         for (auto& c : in) ip.push_back(c.data());\n\
         for (auto& c : out) op.push_back(c.data());\n\
         computemydsp(d, n, ip.data(), op.data());\n\
         for (int i = 0; i < n; i++) { for (int c = 0; c < no; c++) printf(\"%g \", (double)out[c][i]); printf(\"\\n\"); }\n\
         deletemydsp(d);\n}\n"
    }
}

/// Builds the generated file with the driver, no include added by hand, and
/// returns its output; `None` without a C++ compiler.
fn build_and_run(dir: &Path, generated: &Path, lang: &str) -> Option<String> {
    let cxx = cxx()?;
    let stem = generated
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let main = dir.join(format!("{stem}_{lang}_main.cpp"));
    std::fs::write(&main, driver(lang)).expect("write driver");
    let exe = dir.join(format!("{stem}_{lang}.bin"));
    let build = Command::new(cxx)
        .args(["-std=c++14", "-I"])
        .arg(arch_dir())
        .arg("-I")
        .arg(dir)
        .arg(format!("-DGENERATED=\"{}\"", generated.display()))
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run c++");
    assert!(
        build.status.success(),
        "{} does not build:\n{}",
        generated.display(),
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&exe).output().expect("run the program");
    assert!(run.status.success());
    Some(String::from_utf8_lossy(&run.stdout).into_owned())
}

fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

#[test]
fn the_header_of_a_foreign_function_is_included_and_the_code_builds() {
    let dir = work_dir("mixed");
    for (lang, extra) in [("cpp", &[][..]), ("cpp", &["-vec"][..]), ("c", &[][..])] {
        let generated = generate(&dir, "mixed", MIXED, lang, extra);
        let text = std::fs::read_to_string(&generated).expect("read generated");
        assert_eq!(count(&text, "#include \"w3.h\""), 1, "{lang} {extra:?}");
        // C's prelude has `<math.h>` already; C++'s has `<cmath>` only.
        assert_eq!(count(&text, "#include <math.h>"), 1, "{lang} {extra:?}");
        assert!(!text.contains("link with"), "{lang}: no library");
        let header = text.find("#include \"w3.h\"").unwrap();
        let first_use = text.find("w3_add_one(").unwrap();
        assert!(header < first_use, "{lang}: the include precedes the uses");
        // `f(2)` in `instanceConstants`, `f` per sample, `sinh` per sample.
        if let Some(out) = build_and_run(&dir, &generated, lang) {
            let rows: Vec<&str> = out.lines().map(str::trim).collect();
            assert_eq!(rows[0], "3 2 1.1752", "{lang} {extra:?}: {out}");
            assert_eq!(rows[1..], ["3 1 0"; 3], "{lang} {extra:?}: {out}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_table_generator_keeps_its_header_and_library() {
    let dir = work_dir("generator");
    for lang in ["cpp", "c"] {
        let generated = generate(&dir, "generator", GENERATOR, lang, &[]);
        let text = std::fs::read_to_string(&generated).expect("read generated");
        assert_eq!(count(&text, "#include \"w3.h\""), 1, "{lang}");
        assert_eq!(count(&text, "/* link with : \"w3lib\" */"), 1, "{lang}");
        // The table holds t + 1; index 2 reads 3, plus f(0) = 1.
        if let Some(out) = build_and_run(&dir, &generated, lang) {
            assert!(out.lines().all(|row| row.trim() == "4"), "{lang}: {out}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_foreign_variable_is_read_from_its_header() {
    let dir = work_dir("variable");
    for lang in ["cpp", "c"] {
        let generated = generate(&dir, "variable", VARIABLE, lang, &[]);
        let text = std::fs::read_to_string(&generated).expect("read generated");
        assert_eq!(count(&text, "#include \"w3.h\""), 1, "{lang}");
        // The header declares it: a member of the same name would shadow it.
        assert!(!text.contains("float w3_gain;"), "{lang}:\n{text}");
        if let Some(out) = build_and_run(&dir, &generated, lang) {
            assert!(out.lines().all(|row| row.trim() == "2"), "{lang}: {out}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_header_is_not_included() {
    let dir = work_dir("empty");
    for lang in ["cpp", "c"] {
        let generated = generate(&dir, "empty", NO_HEADER, lang, &[]);
        let text = std::fs::read_to_string(&generated).expect("read generated");
        assert!(!text.contains("#include \"\""), "{lang}");
        assert!(!text.contains("link with"), "{lang}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The dependencies are FIR: once in the module's `globals` however many
/// calls name them, in the generator's own `globals`, and once after the
/// generator is flattened into the parent, as the flat backends see it.
#[test]
fn dependencies_survive_lowering_and_flattening() {
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane("generator", GENERATOR, SignalFirLane::TransformFastLane)
        .expect("lower to FIR");
    let dependencies = |globals| -> Vec<(FirDependencyKind, String)> {
        match match_fir(&fir.store, globals) {
            FirMatch::Block(items) => items
                .into_iter()
                .filter_map(|item| match match_fir(&fir.store, item) {
                    FirMatch::DeclareDependency { kind, name } => Some((kind, name)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let expected = vec![
        (FirDependencyKind::Include, "\"w3.h\"".to_owned()),
        (FirDependencyKind::Library, "\"w3lib\"".to_owned()),
    ];
    let FirMatch::Module {
        globals,
        sub_modules,
        ..
    } = match_fir(&fir.store, fir.module)
    else {
        panic!("not a module");
    };
    assert_eq!(dependencies(globals), expected);
    let FirMatch::Block(subs) = match_fir(&fir.store, sub_modules) else {
        panic!("no sub-module block");
    };
    let [sub] = subs.as_slice() else {
        panic!("one table generator expected, got {subs:?}");
    };
    let FirMatch::SubModule { globals, .. } = match_fir(&fir.store, *sub) else {
        panic!("not a sub-module");
    };
    assert_eq!(dependencies(globals), expected);

    let (store, flat) = fir::subcontainer::flatten_sub_modules_owned(
        &fir.store,
        fir.module,
        fir::subcontainer::SubModuleStatePolicy::MergedStructFields,
    )
    .expect("flatten");
    let FirMatch::Module { globals, .. } = match_fir(&store, flat) else {
        panic!("not a module");
    };
    let FirMatch::Block(items) = match_fir(&store, globals) else {
        panic!("globals is not a block");
    };
    let names: Vec<String> = items
        .into_iter()
        .filter_map(|item| match match_fir(&store, item) {
            FirMatch::DeclareDependency { name, .. } => Some(name),
            FirMatch::DeclareFun {
                name, body: None, ..
            } if name == "w3_add_one" => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["\"w3.h\"", "\"w3lib\"", "w3_add_one"]);
    let report = fir::checker::verify_fir_module(&fir.store, fir.module);
    assert!(
        !report.has_errors(),
        "{:?}",
        report.errors().collect::<Vec<_>>()
    );
}
