//! CLI architecture lookup must use installed defaults as well as explicit -A paths.
//! Fixtures are test-local; no installed Faust headers or libraries are needed.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "faust-rs-architecture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("defaults/faust")).unwrap();
        fs::create_dir_all(root.join("extra/faust")).unwrap();
        fs::write(root.join("input.dsp"), "process = _;\n").unwrap();
        Self(root)
    }

    fn run(&self, extra: bool) -> String {
        let mut command = Command::new(env!("CARGO_BIN_EXE_faust-rs"));
        command
            .current_dir(&self.0)
            .env("FAUST_ARCH_PATH", self.0.join("defaults"))
            .env_remove("FAUST_LIB_PATH")
            .args(["-i", "-a", "path-probe.cpp", "input.dsp"]);
        if extra {
            command.arg("-A").arg(self.0.join("extra"));
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "architecture wrapping failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn default_architecture_path_resolves_template_and_inline_header_without_a() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("defaults/path-probe.cpp"),
        "#include <faust/probe.h>\n<<includeclass>>\n",
    )
    .unwrap();
    fs::write(
        fixture.0.join("defaults/faust/probe.h"),
        "// default header injected\n",
    )
    .unwrap();
    let output = fixture.run(false);
    assert!(output.contains("// default header injected"));
    assert!(!output.contains("#include <faust/probe.h>"));
    assert!(output.contains("class mydsp"));
}

#[test]
fn default_architecture_paths_precede_explicit_a_like_cpp() {
    let fixture = Fixture::new();
    for (directory, marker) in [("defaults", "default"), ("extra", "extra")] {
        fs::write(
            fixture.0.join(directory).join("path-probe.cpp"),
            format!("// {marker} template\n<<includeclass>>\n"),
        )
        .unwrap();
    }
    let output = fixture.run(true);
    assert!(output.contains("// default template"));
    assert!(!output.contains("// extra template"));
}

#[test]
fn explicit_a_still_resolves_templates_and_headers_outside_defaults() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("extra/path-probe.cpp"),
        "#include <faust/probe.h>\n<<includeclass>>\n",
    )
    .unwrap();
    fs::write(
        fixture.0.join("extra/faust/probe.h"),
        "// extra header injected\n",
    )
    .unwrap();
    assert!(fixture.run(true).contains("// extra header injected"));
}
