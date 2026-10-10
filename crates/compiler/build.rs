//! Records the Git commit this crate is built from.
//!
//! The workspace version only changes at a release, so every commit between two
//! tags reports the same `CARGO_PKG_VERSION` (grame-cncm/faust-rs#20). The
//! commit identifies the build instead: `--version` prints it after the version,
//! as `rustc` and `cargo` do, and the diagnostics-v2 `compiler` block carries it
//! in its own fields. The package version stays the only version written into
//! generated code, so the output of a program does not change from one commit
//! to the next.
//!
//! Sets `FAUST_RS_COMMIT` (the full hash of `HEAD`) and `FAUST_RS_COMMIT_DATE`
//! (its committer date, `YYYY-MM-DD`), both or neither. Neither is set when the
//! crate is not built from a Git checkout of this repository (a source archive,
//! no `git` program): the build then reports the package version alone. The
//! commit is that of the checkout; uncommitted changes are not reflected.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs `git -C dir args` and returns its trimmed standard output, or `None`
/// when `git` is missing, fails, or prints nothing.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// A path printed by `git rev-parse`, which may be relative to `dir`.
fn git_path(dir: &Path, printed: &str) -> PathBuf {
    let path = PathBuf::from(printed);
    if path.is_absolute() {
        path
    } else {
        dir.join(path)
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );

    // Only a checkout of this repository: a copy of the crate inside another
    // repository must not report that repository's commit.
    let Some(top) = git(&manifest_dir, &["rev-parse", "--show-toplevel"]) else {
        return;
    };
    let expected = Path::new(&top).join("crates").join("compiler");
    if expected.canonicalize().ok() != manifest_dir.canonicalize().ok() {
        return;
    }

    // Rerun when HEAD moves: a checkout or a commit rewrites `HEAD` or a ref.
    // Only existing paths are watched, since Cargo reruns a build script on
    // every build while a watched path is missing.
    if let Some(git_dir) = git(&manifest_dir, &["rev-parse", "--git-dir"]) {
        let git_dir = git_path(&manifest_dir, &git_dir);
        let common_dir = git(&manifest_dir, &["rev-parse", "--git-common-dir"])
            .map_or_else(|| git_dir.clone(), |dir| git_path(&manifest_dir, &dir));
        for path in [
            git_dir.join("HEAD"),
            common_dir.join("packed-refs"),
            common_dir.join("refs").join("heads"),
        ] {
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }

    let commit = git(&manifest_dir, &["rev-parse", "HEAD"]);
    let date = git(
        &manifest_dir,
        &["show", "-s", "--format=%cd", "--date=short", "HEAD"],
    );
    if let (Some(commit), Some(date)) = (commit, date) {
        println!("cargo:rustc-env=FAUST_RS_COMMIT={commit}");
        println!("cargo:rustc-env=FAUST_RS_COMMIT_DATE={date}");
    }
}
