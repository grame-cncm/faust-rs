//! What the probe's integration tests share: a directory of fixtures that
//! goes away with the test, and the binary run on a command line.
//!
//! Each file of `tests/` is a crate of its own and takes what it needs, hence
//! the `allow`: a helper one of them does not use is not dead.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A directory of the test's own under the system's temporary directory,
/// removed when the test ends.
///
/// `name` is unique within a test file, and the process id keeps the files
/// apart, each running in a process of its own.
pub struct Fixtures(PathBuf);

impl Fixtures {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("faustprobe_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        Self(dir)
    }

    /// Writes `text` to `file`, which may be in a sub-directory, and returns
    /// its path.
    pub fn write(&self, file: &str, text: &str) -> String {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().expect("a file in the fixture dir"))
            .expect("create dir");
        std::fs::write(&path, text).expect("write fixture");
        path.to_string_lossy().into_owned()
    }

    /// The path `file` has or would have.
    pub fn path(&self, file: &str) -> String {
        self.0.join(file).to_string_lossy().into_owned()
    }

    pub fn dir(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `faustprobe ARGS`: whether it succeeded, its stdout, its stderr.
pub fn probe(args: &[&str]) -> (bool, String, String) {
    run(Command::new(env!("CARGO_BIN_EXE_faustprobe")).args(args))
}

/// [`probe`] with `dir` as the working directory.
pub fn probe_in(dir: &Path, args: &[&str]) -> (bool, String, String) {
    run(Command::new(env!("CARGO_BIN_EXE_faustprobe"))
        .current_dir(dir)
        .args(args))
}

/// `faustprobe ARGS FILE`, `FILE` holding `source` for the time of the run.
/// `name` is unique within a test file.
pub fn probe_source(name: &str, source: &str, args: &[&str]) -> (bool, String, String) {
    let fixtures = Fixtures::new(name);
    let path = fixtures.write(&format!("{name}.dsp"), source);
    run(Command::new(env!("CARGO_BIN_EXE_faustprobe"))
        .args(args)
        .arg(path))
}

fn run(command: &mut Command) -> (bool, String, String) {
    let out = command.output().expect("run faustprobe");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The JSON document a command printed.
pub fn json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not JSON ({e}):\n{stdout}"))
}
