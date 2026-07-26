//! Code generation crate for backend emission from FIR.
//!
//! # Source provenance (C++)
//! - `compiler/generator/*`
//! - `compiler/generator/fir/*`
//! - backend-specific emitters under `compiler/generator/<backend>/`
//!
//! # Role in pipeline
//! - Consumes FIR (`fir::FirStore` + FIR roots) produced by compile lanes.
//! - Emits target-language source text for supported backends.
//! - Centralizes backend option structs and signature validation helpers.
//!
//! # Public surface
//! - [`backends`] exposes backend modules.
//! - [`fixtures`] provides shared FIR fixtures used by backend tests and parity
//!   checks.
//!
//! # Current status
//! - C/C++ backends are implemented for the active module-first slice.
//! - Other backend modules are scaffolded with stable identifiers and explicit
//!   placeholders for future parity work.
//!
//! # API mapping status
//! - Backend option structs and generation entry points are `adapted` APIs:
//!   they preserve C++ behavior but use Rust ownership/error types.

pub mod backend_error;
pub mod backends;
pub mod fixtures;
pub mod json;

pub const CRATE_NAME: &str = "codegen";

#[must_use]
/// Returns the stable crate identifier.
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

#[cfg(test)]
mod readme_consistency_tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    /// A backend directory holding no more than this many source lines is a
    /// scaffold (`backend_id()` and nothing else), not an implementation.
    const SCAFFOLD_MAX_LINES: usize = 40;

    fn backends_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("backends")
    }

    fn readme() -> String {
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("crates/codegen/README.md must exist")
    }

    /// Splits backend directories into (implemented, scaffolded) by source size.
    fn classify_backends() -> (BTreeSet<String>, BTreeSet<String>) {
        let (mut real, mut scaffold) = (BTreeSet::new(), BTreeSet::new());
        for entry in fs::read_dir(backends_dir()).expect("backends dir must be readable") {
            let entry = entry.expect("readable dir entry");
            if !entry.file_type().expect("file type").is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let lines: usize = fs::read_dir(entry.path())
                .expect("backend dir must be readable")
                .filter_map(Result::ok)
                .filter(|f| f.path().extension().is_some_and(|e| e == "rs"))
                .map(|f| {
                    fs::read_to_string(f.path())
                        .map(|t| t.lines().count())
                        .unwrap_or(0)
                })
                .sum();
            if lines > SCAFFOLD_MAX_LINES {
                real.insert(name);
            } else {
                scaffold.insert(name);
            }
        }
        (real, scaffold)
    }

    /// Every backend directory must appear in the README's status table.
    ///
    /// This is the guard for a drift that really happened: the Rust backend was
    /// implemented (2.9k lines) while the README still listed it as a scaffold,
    /// so readers concluded `-lang rust` did not exist.
    #[test]
    fn readme_lists_every_backend_directory() {
        let text = readme();
        let (real, scaffold) = classify_backends();
        for name in real.iter().chain(scaffold.iter()) {
            assert!(
                text.contains(&format!("`{name}`")),
                "backend `{name}` has no entry in crates/codegen/README.md"
            );
        }
    }

    /// An implemented backend must not be documented as scaffolded.
    #[test]
    fn readme_does_not_call_an_implemented_backend_scaffolded() {
        let text = readme();
        let (real, _) = classify_backends();
        for name in &real {
            let scaffold_row = format!("| `{name}` | 🗂 Scaffolded");
            assert!(
                !text.contains(&scaffold_row),
                "backend `{name}` has a real implementation but README still \
                 lists it as scaffolded"
            );
        }
    }
}
