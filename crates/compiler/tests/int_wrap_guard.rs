//! The helper functions of the C and C++ backends, guarded as in C++.
//!
//! Faust master-dev (`88b5c93ae`, 2.90.x) prints the wrapping integer helpers
//! under `#ifndef FAUST_INT_WRAP` (`CPPCodeContainer::produceClass`,
//! `CCodeContainer::produceClass`), and the C backend's `faustmaxi` /
//! `faustmini` under `#ifndef FAUSTMAXI` (also in the pinned `8eebea429`),
//! so several DSPs can share one translation unit. faust-rs prints the same guard; its C++ helpers are the reference's
//! token for token, so that these external-linkage `inline` functions stay
//! one definition across translation units mixing the two compilers.

use std::path::PathBuf;
use std::process::Command;

use codegen::backends::c::COptions;
use codegen::backends::cpp::CppOptions;
use compiler::Compiler;

/// Integer arithmetic and an integer `min`, so the helpers are used.
const SOURCE: &str = "process = +(1) : int : *(3) : -(2) : min(10);";

fn cpp(class_name: &str) -> String {
    let options = CppOptions {
        class_name: Some(class_name.to_owned()),
        ..CppOptions::default()
    };
    Compiler::new()
        .compile_source_to_cpp(class_name, SOURCE, &options)
        .expect("compiles to C++")
}

#[test]
fn cpp_helpers_are_the_reference_ones_under_its_guard() {
    let text = cpp("mydsp");
    let expected = "#ifndef FAUST_INT_WRAP\n\
                    #define FAUST_INT_WRAP\n\
                    inline int faust_wrap_add(int a, int b) { return int((unsigned int)a + (unsigned int)b); }\n\
                    inline int faust_wrap_sub(int a, int b) { return int((unsigned int)a - (unsigned int)b); }\n\
                    inline int faust_wrap_mul(int a, int b) { return int((unsigned int)a * (unsigned int)b); }\n\
                    #endif\n";
    assert!(text.contains(expected), "{text}");
    assert!(
        text.contains("faust_wrap_mul("),
        "the helpers are used:\n{text}"
    );
}

fn c(class_name: &str) -> String {
    let options = COptions {
        class_name: Some(class_name.to_owned()),
        ..COptions::default()
    };
    Compiler::new()
        .compile_source_to_c(class_name, SOURCE, &options)
        .expect("compiles to C")
}

#[test]
fn c_helpers_are_guarded_like_the_reference() {
    let text = c("mydsp");
    let minmax = "#ifndef FAUSTMAXI\n\
                  #define FAUSTMAXI\n\
                  static inline FAUST_UNUSED int faustmaxi(int a, int b) { return (a > b) ? a : b; }\n\
                  static inline FAUST_UNUSED int faustmini(int a, int b) { return (a < b) ? a : b; }\n\
                  #endif\n";
    assert!(text.contains(minmax), "{text}");
    assert!(text.contains("faustmini("), "the helper is used:\n{text}");
    let expected = "#ifndef FAUST_INT_WRAP\n\
                    #define FAUST_INT_WRAP\n\
                    static inline FAUST_UNUSED int faust_wrap_add(int a, int b) { return (int)(((uint32_t)a) + ((uint32_t)b)); }\n\
                    static inline FAUST_UNUSED int faust_wrap_sub(int a, int b) { return (int)(((uint32_t)a) - ((uint32_t)b)); }\n\
                    static inline FAUST_UNUSED int faust_wrap_mul(int a, int b) { return (int)(((uint32_t)a) * ((uint32_t)b)); }\n\
                    #endif\n";
    assert!(text.contains(expected), "{text}");
}

#[test]
fn two_cpp_dsp_classes_build_in_one_translation_unit() {
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
    if !Command::new(&cxx)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipping: no C++ compiler");
        return;
    }
    let dir = std::env::temp_dir().join(format!("faust-rs-int-wrap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(dir.join("dspa.h"), cpp("dspa")).expect("write dspa");
    std::fs::write(dir.join("dspb.h"), cpp("dspb")).expect("write dspb");
    // The reference's own helpers, as a C++ Faust DSP included first would
    // print them: the shared guard leaves them the only definition.
    std::fs::write(
        dir.join("main.cpp"),
        "#include \"faust_minimal.h\"\n\
         #ifndef FAUST_INT_WRAP\n\
         #define FAUST_INT_WRAP\n\
         inline int faust_wrap_add(int a, int b) { return int((unsigned int)a + (unsigned int)b); }\n\
         inline int faust_wrap_sub(int a, int b) { return int((unsigned int)a - (unsigned int)b); }\n\
         inline int faust_wrap_mul(int a, int b) { return int((unsigned int)a * (unsigned int)b); }\n\
         #endif\n\
         #include \"dspa.h\"\n\
         #undef FAUSTCLASS\n\
         #include \"dspb.h\"\n\
         int main() {\n\
           dspa a; dspb b; a.init(48000); b.init(48000);\n\
           FAUSTFLOAT x[1] = {1}, ya[1] = {0}, yb[1] = {0};\n\
           FAUSTFLOAT* in[1] = {x}; FAUSTFLOAT* oa[1] = {ya}; FAUSTFLOAT* ob[1] = {yb};\n\
           a.compute(1, in, oa); b.compute(1, in, ob);\n\
           return (ya[0] == 4 && yb[0] == 4) ? 0 : 1;\n\
         }\n",
    )
    .expect("write main");
    let architecture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/impulse-tests/archs");
    let exe = dir.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(&cxx)
        .args(["-std=c++14", "-I"])
        .arg(&architecture)
        .arg("-I")
        .arg(&dir)
        .arg(dir.join("main.cpp"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run the C++ compiler");
    assert!(
        build.status.success(),
        "two DSP classes in one translation unit do not build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&exe).output().expect("run the program");
    assert!(run.status.success(), "both DSPs compute (1 + 1) * 3 - 2");
    std::fs::remove_dir_all(dir).expect("remove temp dir");
}

#[test]
fn two_c_dsps_build_in_one_translation_unit() {
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned());
    if !Command::new(&cxx)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipping: no C++ compiler");
        return;
    }
    let dir = std::env::temp_dir().join(format!("faust-rs-int-wrap-c-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(dir.join("dspa.h"), c("dspa")).expect("write dspa");
    std::fs::write(dir.join("dspb.h"), c("dspb")).expect("write dspb");
    // The glue of the impulse drivers is C++ (`faust_minimal_cglue.h`), so the
    // two C DSPs are built in one C++ unit, which rejects a redefined
    // function as C does.
    std::fs::write(
        dir.join("main.cpp"),
        "#include \"faust_minimal_cglue.h\"\n\
         #include \"dspa.h\"\n\
         #undef FAUSTCLASS\n\
         #include \"dspb.h\"\n\
         int main() {\n\
           dspa* a = newdspa(); dspb* b = newdspb(); initdspa(a, 48000); initdspb(b, 48000);\n\
           FAUSTFLOAT x[1] = {1}, ya[1] = {0}, yb[1] = {0};\n\
           FAUSTFLOAT* in[1] = {x}; FAUSTFLOAT* oa[1] = {ya}; FAUSTFLOAT* ob[1] = {yb};\n\
           computedspa(a, 1, in, oa); computedspb(b, 1, in, ob);\n\
           bool ok = ya[0] == 4 && yb[0] == 4;\n\
           deletedspa(a); deletedspb(b);\n\
           return ok ? 0 : 1;\n\
         }\n",
    )
    .expect("write main");
    let architecture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/impulse-tests/archs");
    let exe = dir.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    // `-O0`: no inlining, so a helper without a definition in this unit
    // would fail to link.
    let build = Command::new(&cxx)
        .args(["-std=c++14", "-O0", "-I"])
        .arg(&architecture)
        .arg("-I")
        .arg(&dir)
        .arg(dir.join("main.cpp"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run the C++ compiler");
    assert!(
        build.status.success(),
        "two C DSPs in one translation unit do not build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&exe).output().expect("run the program");
    assert!(
        run.status.success(),
        "both DSPs compute min((1 + 1) * 3 - 2, 10)"
    );
    std::fs::remove_dir_all(dir).expect("remove temp dir");
}
