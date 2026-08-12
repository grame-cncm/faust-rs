//! Artifact delivery for the `generateAuxFiles*` entry points.
//!
//! The compiler facade returns auxiliary outputs as owned in-memory artifacts
//! so hosts without a writable filesystem can consume them. The C API predates
//! that choice and has two delivery shapes, which this module supplies:
//! writing to disk, and returning the single result as a string.

use std::ffi::c_char;
use std::path::PathBuf;

use compiler::AuxFileArtifact;
use ffi_common::write_error_4096;

/// Writes every artifact under the directory selected by `-O <path>`.
///
/// Mirrors what C++ does implicitly by having its backends write files
/// directly. Returns `false` after reporting the first failure, because a
/// partially written output set is worse than none: the caller cannot tell
/// which files are current.
pub(crate) fn write_artifacts_to_disk(
    artifacts: &[AuxFileArtifact],
    argv: &[String],
    error_msg: *mut c_char,
) -> bool {
    let out_dir = output_dir(argv);
    if let Err(error) = std::fs::create_dir_all(&out_dir) {
        unsafe {
            write_error_4096(
                error_msg,
                &format!("cannot create output dir {}: {error}", out_dir.display()),
            );
        }
        return false;
    }
    for artifact in artifacts {
        let destination = out_dir.join(&artifact.path);
        if let Err(error) = std::fs::write(&destination, &artifact.content) {
            unsafe {
                write_error_4096(
                    error_msg,
                    &format!("cannot write {}: {error}", destination.display()),
                );
            }
            return false;
        }
    }
    true
}

/// Returns the single textual artifact produced by a `*2` call.
///
/// The `*2` entry points return one string, so a request that selected several
/// outputs has no well-defined answer; saying so is better than silently
/// returning whichever one came first.
pub(crate) fn single_artifact_text(artifacts: &[AuxFileArtifact]) -> Result<String, String> {
    match artifacts {
        [] => Err("no output was requested: pass one of -cpp, -c, -wasm, -json or -svg".to_owned()),
        [artifact] if artifact.binary => Err(format!(
            "{} is binary and cannot be returned as a string; \
             use generateCAuxFilesFrom{{File,String}} to write it to disk",
            artifact.path
        )),
        [artifact] => String::from_utf8(artifact.content.clone())
            .map_err(|_| format!("{} is not valid UTF-8", artifact.path)),
        many => Err(format!(
            "{} outputs were requested ({}); the string-returning entry points deliver one",
            many.len(),
            many.iter()
                .map(|artifact| artifact.path.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Extracts the value of `-O <path>` from `argv`, defaulting to `.`.
fn output_dir(argv: &[String]) -> PathBuf {
    argv.iter()
        .position(|argument| argument == "-O")
        .and_then(|position| argv.get(position + 1))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::{output_dir, single_artifact_text};
    use compiler::AuxFileArtifact;

    /// Builds one text artifact without going through the compiler facade,
    /// whose constructors are private to it.
    fn text_artifact(path: &str, content: &str) -> AuxFileArtifact {
        AuxFileArtifact {
            path: path.to_owned(),
            content: content.as_bytes().to_vec(),
            binary: false,
        }
    }

    #[test]
    fn output_dir_defaults_to_the_working_directory() {
        assert_eq!(output_dir(&[]).to_string_lossy(), ".");
        assert_eq!(
            output_dir(&["-O".to_owned(), "build".to_owned()]).to_string_lossy(),
            "build"
        );
        // A dangling `-O` must not be read as a directory named by the next
        // flag, nor panic.
        assert_eq!(output_dir(&["-O".to_owned()]).to_string_lossy(), ".");
    }

    #[test]
    fn a_single_text_artifact_is_returned() {
        let artifacts = vec![text_artifact("probe.cpp", "class mydsp {};")];
        assert_eq!(
            single_artifact_text(&artifacts).expect("one text artifact"),
            "class mydsp {};"
        );
    }

    #[test]
    fn a_binary_artifact_is_refused_rather_than_mangled() {
        let artifacts = vec![AuxFileArtifact {
            path: "probe.wasm".to_owned(),
            content: vec![0x00, 0x61, 0x73, 0x6d],
            binary: true,
        }];
        let error = single_artifact_text(&artifacts).expect_err("a wasm module is not a string");
        assert!(error.contains("probe.wasm"), "{error}");
    }

    #[test]
    fn ambiguous_and_empty_requests_are_reported() {
        assert!(single_artifact_text(&[]).is_err());
        let artifacts = vec![
            text_artifact("probe.cpp", ""),
            text_artifact("probe.json", ""),
        ];
        let error = single_artifact_text(&artifacts).expect_err("two outputs are ambiguous");
        assert!(error.contains("probe.cpp"), "{error}");
        assert!(error.contains("probe.json"), "{error}");
    }
}
