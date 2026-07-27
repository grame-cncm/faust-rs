//! Workspace-layer and unsafe-lint checks for the FFI crates.
//!
//! The intended dependency direction is:
//!
//! ```text
//! compiler core <- FFI adapters <- distribution crates
//! ```
//!
//! A crate may depend on crates in its own layer or a layer to its left, but
//! never on a layer to its right. `foreign-call` is a core runtime bridge and
//! is the only non-FFI crate allowed to opt into unsafe code.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const ADAPTER_CRATES: [&str; 6] = [
    "ffi-common",
    "tree-ffi",
    "box-ffi",
    "signal-ffi",
    "interp-ffi",
    "cranelift-ffi",
];

const DISTRIBUTION_CRATES: [&str; 2] = ["faust-ffi", "wasm-ffi"];

const UNSAFE_ALLOWLIST: [&str; 7] = [
    "ffi-common",
    "tree-ffi",
    "box-ffi",
    "signal-ffi",
    "interp-ffi",
    "cranelift-ffi",
    "wasm-ffi",
];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Layer {
    Core,
    Adapter,
    Distribution,
}

impl Layer {
    fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Adapter => "adapter",
            Self::Distribution => "distribution",
        }
    }
}

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
    path: Option<PathBuf>,
}

/// Checks workspace FFI dependency direction and unsafe-code opt-ins.
pub fn ffi_boundary_check() -> Result<(), Box<dyn std::error::Error>> {
    if !PathBuf::from("Cargo.toml").is_file() || !PathBuf::from("crates").is_dir() {
        return Err("ffi-boundary-check must run from the repository root".into());
    }

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let metadata: Metadata = serde_json::from_slice(&output.stdout)?;
    let findings = collect_findings(&metadata)?;
    if findings.is_empty() {
        println!(
            "ffi-boundary-check: OK ({} workspace crates; {} adapters; {} distributions; {} unsafe opt-ins)",
            metadata.workspace_members.len(),
            ADAPTER_CRATES.len(),
            DISTRIBUTION_CRATES.len(),
            UNSAFE_ALLOWLIST.len() + 1,
        );
        Ok(())
    } else {
        for finding in &findings {
            eprintln!("ffi-boundary-check: {finding}");
        }
        Err(format!("ffi-boundary-check: {} finding(s)", findings.len()).into())
    }
}

fn collect_findings(metadata: &Metadata) -> Result<Vec<String>, std::io::Error> {
    let workspace_ids: BTreeSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let packages: Vec<&Package> = metadata
        .packages
        .iter()
        .filter(|package| workspace_ids.contains(package.id.as_str()))
        .collect();
    let workspace_names: BTreeSet<&str> = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let mut findings = Vec::new();

    for expected in ADAPTER_CRATES.into_iter().chain(DISTRIBUTION_CRATES) {
        if !workspace_names.contains(expected) {
            findings.push(format!("classified FFI crate `{expected}` is absent"));
        }
    }

    for package in packages {
        let source_layer = layer_for(&package.name);
        if package.name.ends_with("-ffi")
            && source_layer == Layer::Core
            && package.name != "foreign-call"
        {
            findings.push(format!(
                "`{}` has an FFI package name but no explicit layer classification",
                package.name
            ));
        }

        let mut workspace_dependencies = Vec::new();
        for dependency in &package.dependencies {
            if dependency.path.is_none() || !workspace_names.contains(dependency.name.as_str()) {
                continue;
            }
            workspace_dependencies.push(dependency.name.as_str());
            let target_layer = layer_for(&dependency.name);
            if target_layer > source_layer {
                findings.push(format!(
                    "{} crate `{}` depends rightward on {} crate `{}`",
                    source_layer.name(),
                    package.name,
                    target_layer.name(),
                    dependency.name
                ));
            }
        }

        if package.name == "ffi-common" && !workspace_dependencies.is_empty() {
            workspace_dependencies.sort_unstable();
            findings.push(format!(
                "`ffi-common` must not depend on workspace crates (found: {})",
                workspace_dependencies.join(", ")
            ));
        }

        let manifest = fs::read_to_string(&package.manifest_path)?;
        if let Some(reason) = manifest_unsafe_allow_reason(&manifest) {
            if !unsafe_allowed_for(&package.name) {
                findings.push(format!(
                    "`{}` opts into unsafe code outside the explicit FFI/foreign-call boundary",
                    package.name
                ));
            }
            if reason.is_empty() {
                findings.push(format!(
                    "`{}` opts into unsafe code without an inline foreign-boundary reason",
                    package.name
                ));
            }
        }
    }

    findings.sort();
    findings.dedup();
    Ok(findings)
}

fn layer_for(package: &str) -> Layer {
    if DISTRIBUTION_CRATES.contains(&package) {
        Layer::Distribution
    } else if ADAPTER_CRATES.contains(&package) {
        Layer::Adapter
    } else {
        Layer::Core
    }
}

fn unsafe_allowed_for(package: &str) -> bool {
    package == "foreign-call" || UNSAFE_ALLOWLIST.contains(&package)
}

fn manifest_unsafe_allow_reason(manifest: &str) -> Option<&str> {
    manifest.lines().find_map(|line| {
        let (code, comment) = line
            .split_once('#')
            .map_or((line, ""), |(code, comment)| (code, comment));
        let code = code.trim();
        let (key, value) = code.split_once('=')?;
        (key.trim() == "unsafe_code" && value.trim() == "\"allow\"").then_some(comment.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_encode_the_one_way_dependency_rule() {
        assert!(layer_for("compiler") < layer_for("box-ffi"));
        assert!(layer_for("box-ffi") < layer_for("faust-ffi"));
        assert_eq!(layer_for("ffi-common"), Layer::Adapter);
    }

    #[test]
    fn unsafe_allowlist_keeps_foreign_call_as_the_only_core_exception() {
        assert!(unsafe_allowed_for("foreign-call"));
        assert!(unsafe_allowed_for("ffi-common"));
        assert!(unsafe_allowed_for("wasm-ffi"));
        assert!(!unsafe_allowed_for("compiler"));
        assert!(!unsafe_allowed_for("faust-ffi"));
    }

    #[test]
    fn manifest_scan_ignores_comments_and_requires_an_exact_allow_value() {
        assert_eq!(
            manifest_unsafe_allow_reason("[lints.rust]\nunsafe_code = \"allow\" # raw C callbacks"),
            Some("raw C callbacks")
        );
        assert_eq!(
            manifest_unsafe_allow_reason("# unsafe_code = \"allow\"\nunsafe_code = \"forbid\""),
            None
        );
        assert_eq!(
            manifest_unsafe_allow_reason("unsafe_code = \"allow\""),
            Some("")
        );
    }
}
