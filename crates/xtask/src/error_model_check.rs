//! Architectural guard for operational errors and structured diagnostics.

use super::*;

const CHECK_SOURCE: &str = "crates/xtask/src/error_model_check.rs";
const POLICY_DOCUMENT: &str =
    "porting/error-diagnostics-separation-analysis-and-porting-plan-2026-07-28-en.md";

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
struct Dependency {
    name: String,
    rename: Option<String>,
}

/// Enforces the workspace ownership split between typed operational errors and
/// structured diagnostic reports.
pub(crate) fn error_model_check() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let metadata = load_metadata(&root)?;
    let members = metadata.workspace_members.iter().collect::<BTreeSet<_>>();
    let packages = metadata
        .packages
        .iter()
        .filter(|package| members.contains(&package.id))
        .collect::<Vec<_>>();
    let names = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut findings = Vec::new();

    if !names.contains("diagnostics") {
        findings.push("workspace package `diagnostics` is missing".to_owned());
    }
    if names.contains("errors") {
        findings.push("legacy workspace package `errors` is present".to_owned());
    }
    for package in &packages {
        for dependency in &package.dependencies {
            if dependency.name == "errors" || dependency.rename.as_deref() == Some("errors") {
                findings.push(format!(
                    "{}: legacy dependency name or alias `errors` is present",
                    package.name
                ));
            }
        }
    }

    let mut rust_files = Vec::new();
    let mut manifest_files = vec![root.join("Cargo.toml")];
    for package in &packages {
        let package_root = package
            .manifest_path
            .parent()
            .ok_or("workspace package manifest has no parent")?;
        collect_files(package_root, "rs", &mut rust_files)?;
        manifest_files.push(package.manifest_path.clone());
    }
    rust_files.sort();
    rust_files.dedup();
    manifest_files.sort();
    manifest_files.dedup();

    for file in &rust_files {
        let relative = workspace_relative_path(file);
        if relative == CHECK_SOURCE {
            continue;
        }
        let source = fs::read_to_string(file)?;
        findings.extend(source_findings(&relative, &source));
    }
    for file in &manifest_files {
        let relative = workspace_relative_path(file);
        let source = fs::read_to_string(file)?;
        findings.extend(manifest_findings(&relative, &source));
    }

    require_markers(
        &root.join("crates/diagnostics/src/lib.rs"),
        &["pub trait ToDiagnostic", "fn to_diagnostic(&self)"],
        &mut findings,
    )?;
    reject_markers(
        &root.join("crates/diagnostics/src/lib.rs"),
        &["pub const CRATE_NAME", "pub fn crate_id"],
        &mut findings,
    )?;
    require_markers(
        &root.join("crates/compiler/src/lib.rs"),
        &[
            "fn source(&self)",
            "pub fn diagnostic_bundle(&self)",
            "error: Box<InferenceError>",
        ],
        &mut findings,
    )?;

    if !findings.is_empty() {
        findings.sort();
        findings.dedup();
        eprintln!("error-model-check: {} finding(s)", findings.len());
        for finding in findings {
            eprintln!("- {finding}");
        }
        return Err(
            format!("error/diagnostics ownership policy failed; see {POLICY_DOCUMENT}").into(),
        );
    }

    println!(
        "error-model-check: OK ({} workspace packages, {} Rust files, {} manifests)",
        packages.len(),
        rust_files.len(),
        manifest_files.len()
    );
    Ok(())
}

fn load_metadata(root: &Path) -> Result<Metadata, Box<dyn std::error::Error>> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn collect_files(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, extension, files)?;
        } else if path
            .extension()
            .is_some_and(|candidate| candidate == extension)
        {
            files.push(path);
        }
    }
    Ok(())
}

fn source_findings(path: &str, source: &str) -> Vec<String> {
    [
        ("errors::", "legacy `errors` import path"),
        ("IntoDiagnostic", "consuming diagnostic conversion"),
        ("into_diagnostic(", "consuming diagnostic conversion call"),
        ("DiagnosticSeverity", "parser-local diagnostic severity"),
        (
            "parser_code_for_message",
            "message-text diagnostic classification",
        ),
    ]
    .into_iter()
    .filter(|(needle, _)| source.contains(needle))
    .map(|(_, description)| format!("{path}: {description} detected"))
    .collect()
}

fn manifest_findings(path: &str, source: &str) -> Vec<String> {
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    [
        ("name = \"errors\"", "legacy package name"),
        ("errors = {", "legacy dependency key"),
        ("errors = \"", "legacy dependency key"),
        ("../errors", "legacy dependency path"),
        ("crates/errors", "legacy workspace path"),
    ]
    .into_iter()
    .filter(|(needle, _)| normalized.contains(needle))
    .map(|(_, description)| format!("{path}: {description} detected"))
    .collect()
}

fn require_markers(
    path: &Path,
    markers: &[&str],
    findings: &mut Vec<String>,
) -> Result<(), io::Error> {
    let source = fs::read_to_string(path)?;
    let relative = workspace_relative_path(path);
    for marker in markers {
        if !source.contains(marker) {
            findings.push(format!("{relative}: required marker {marker:?} is missing"));
        }
    }
    Ok(())
}

fn reject_markers(
    path: &Path,
    markers: &[&str],
    findings: &mut Vec<String>,
) -> Result<(), io::Error> {
    let source = fs::read_to_string(path)?;
    let relative = workspace_relative_path(path);
    for marker in markers {
        if source.contains(marker) {
            findings.push(format!(
                "{relative}: forbidden compatibility scaffold {marker:?} is present"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{manifest_findings, reject_markers, source_findings};
    use std::fs;

    #[test]
    fn source_scan_detects_legacy_conversion_and_parser_taxonomy() {
        let source = r#"
            use errors::Diagnostic;
            impl IntoDiagnostic for PhaseError {
                fn into_diagnostic(self) {}
            }
            enum DiagnosticSeverity {}
            fn parser_code_for_message() {}
        "#;
        let findings = source_findings("sample.rs", source);
        assert_eq!(findings.len(), 5);
    }

    #[test]
    fn manifest_scan_detects_legacy_package_dependency_and_paths() {
        let source = r#"
            [package]
            name = "errors"
            [dependencies]
            errors = { path = "../errors" }
            members = ["crates/errors"]
        "#;
        let findings = manifest_findings("Cargo.toml", source);
        assert_eq!(findings.len(), 4);
    }

    #[test]
    fn canonical_source_and_manifest_are_accepted() {
        assert!(source_findings("lib.rs", "impl ToDiagnostic for PhaseError {}").is_empty());
        assert!(
            manifest_findings(
                "Cargo.toml",
                "name = \"diagnostics\"\ndiagnostics = { path = \"../diagnostics\" }"
            )
            .is_empty()
        );
    }

    #[test]
    fn diagnostics_scaffold_markers_are_rejected() {
        let path = std::env::temp_dir().join(format!(
            "faust-rs-error-model-check-{}-{}.rs",
            std::process::id(),
            line!()
        ));
        fs::write(
            &path,
            "pub const CRATE_NAME: &str = \"diagnostics\";\npub fn crate_id() {}",
        )
        .expect("write test source");
        let mut findings = Vec::new();
        reject_markers(
            &path,
            &["pub const CRATE_NAME", "pub fn crate_id"],
            &mut findings,
        )
        .expect("scan test source");
        fs::remove_file(path).expect("remove test source");
        assert_eq!(findings.len(), 2);
    }
}
