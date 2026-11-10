//! `--json` / `-json` is a companion of the backend output, as C++ `-json`.
//!
//! C++ `CodeContainer::generateJSONFile` writes `subst("$0.json",
//! makeDrawPath())`: `<input>.json` next to the input, or
//! `<-O dir>/<name>.dsp.json`, while the backend code (C++ by default,
//! architecture-wrapped under `-a`) goes to `-o` or stdout as usual. The
//! `faust2*` scripts rely on both: `faust2max6` runs
//! `faust -json -a max-msp64.cpp foo.dsp -o tmp/foo.cpp` and reads
//! `foo.dsp.json`. Fixtures are test-local; no installed Faust is needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "faust-rs-json-companion-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("out")).unwrap();
        fs::write(
            root.join("probe.dsp"),
            "process = _ * hslider(\"gain\", 0.5, 0, 1, 0.01);\n",
        )
        .unwrap();
        fs::write(
            root.join("wrap.cpp"),
            "// wrapper begin\n<<includeclass>>\n",
        )
        .unwrap();
        Self(root)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_faust-rs"))
            .current_dir(&self.0)
            .env("FAUST_ARCH_PATH", &self.0)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "faust-rs failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("missing JSON companion {}: {e}", path.display()));
    serde_json::from_str(&text).expect("valid companion JSON")
}

#[test]
fn json_with_architecture_wraps_cpp_and_writes_input_companion() {
    let fixture = Fixture::new();
    let output = fixture.run(&[
        "-i",
        "-cn",
        "probe",
        "-json",
        "-a",
        "wrap.cpp",
        "probe.dsp",
        "-o",
        "out/probe.cpp",
    ]);
    assert_success(&output);
    let cpp = fs::read_to_string(fixture.path("out/probe.cpp")).unwrap();
    assert!(cpp.starts_with("// wrapper begin"), "{cpp}");
    assert!(cpp.contains("class probe"), "{cpp}");
    let json = read_json(&fixture.path("probe.dsp.json"));
    assert_eq!(json["name"], "probe");
    assert_eq!(json["inputs"], 1);
    assert!(
        json["compile_options"]
            .as_str()
            .unwrap()
            .contains("-lang cpp"),
        "{json}"
    );
    // The companion is named after the input, never after `-o`.
    assert!(!fixture.path("out/probe.json").exists());
}

#[test]
fn json_alone_emits_cpp_on_stdout_and_the_companion() {
    let fixture = Fixture::new();
    let output = fixture.run(&["-json", "probe.dsp"]);
    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("class mydsp"), "{stdout}");
    assert_eq!(read_json(&fixture.path("probe.dsp.json"))["outputs"], 1);
}

#[test]
fn json_with_lang_writes_the_same_companion_without_requiring_o() {
    let fixture = Fixture::new();
    let output = fixture.run(&["-lang", "c", "--json", "probe.dsp"]);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("computemydsp"));
    assert_eq!(read_json(&fixture.path("probe.dsp.json"))["inputs"], 1);
}

#[test]
fn output_dir_holds_the_o_file_and_the_named_companion() {
    let fixture = Fixture::new();
    let output = fixture.run(&["-json", "probe.dsp", "-O", "out", "-o", "code.cpp"]);
    assert_success(&output);
    assert!(fixture.path("out/code.cpp").is_file());
    assert_eq!(
        read_json(&fixture.path("out/probe.dsp.json"))["name"],
        "probe"
    );
    assert!(!fixture.path("probe.dsp.json").exists());
}

#[test]
fn wasm_keeps_its_o_helper_and_adds_the_companion() {
    let fixture = Fixture::new();
    let output = fixture.run(&[
        "-lang",
        "wasm",
        "-json",
        "probe.dsp",
        "-O",
        "out",
        "-o",
        "m.wasm",
    ]);
    assert_success(&output);
    assert!(fixture.path("out/m.wasm").is_file());
    assert!(fixture.path("out/m.json").is_file());
    assert!(fixture.path("out/probe.dsp.json").is_file());
}

#[test]
fn invalid_output_dir_is_rejected_like_cpp() {
    let fixture = Fixture::new();
    let output = fixture.run(&["probe.dsp", "-O", "missing-dir"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invalid directory path missing-dir"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn json_with_a_mode_that_emits_no_code_is_rejected() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--check", "--json", "probe.dsp"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!fixture.path("probe.dsp.json").exists());
}
