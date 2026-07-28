//! Architectural guard against reintroducing handwritten process CLI parsers.

use super::*;

const POLICY_DOCUMENT: &str =
    "porting/cli-parser-consolidation-analysis-and-porting-plan-2026-07-28-en.md";
const FFI_PROTOCOL_SOURCE: &str = "crates/ffi-common/src/args.rs";
const STANDALONE_ARCHITECTURE: &str = "tests/impulse-tests/archs/impulserust.rs";
const POLICY_CHECK_SOURCE: &str = "crates/xtask/src/cli_parser_check.rs";

const NORMALIZED_CLAP_ENTRY_POINTS: [(&str, &[&str]); 3] = [
    (
        "crates/compiler/src/cli/runner.rs",
        &["normalize_legacy_args", "CliArgs::parse_from"],
    ),
    (
        "crates/impulse-runner/src/main.rs",
        &["normalize_legacy_arg", "CliArgs::try_parse_from"],
    ),
    (
        "crates/cranelift-ffi/src/bin/impulse_cranelift.rs",
        &["normalize_legacy_arg", "CliArgs::try_parse_from"],
    ),
];

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Package {
    id: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
    targets: Vec<Target>,
}

#[derive(Debug, Deserialize)]
struct Dependency {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Target {
    kind: Vec<String>,
}

/// Checks every workspace package source for unclassified process argument
/// consumers and known handwritten CLI-parser shapes.
pub(crate) fn cli_parser_check() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let metadata = load_metadata(&root)?;
    let members = metadata.workspace_members.iter().collect::<BTreeSet<_>>();
    let mut packages = metadata
        .packages
        .iter()
        .filter(|package| members.contains(&package.id))
        .collect::<Vec<_>>();
    packages.sort_by(|left, right| left.manifest_path.cmp(&right.manifest_path));

    let mut findings = Vec::new();
    let mut rust_file_count = 0usize;
    let mut target_count = 0usize;
    let mut normalized_entry_count = 0usize;

    for package in &packages {
        target_count += package
            .targets
            .iter()
            .filter(|target| {
                target
                    .kind
                    .iter()
                    .any(|kind| matches!(kind.as_str(), "bin" | "example"))
            })
            .count();
        let package_root = package
            .manifest_path
            .parent()
            .ok_or("workspace package manifest has no parent")?;
        let has_clap = package
            .dependencies
            .iter()
            .any(|dependency| dependency.name == "clap");
        let mut files = Vec::new();
        collect_rust_files(package_root, &mut files)?;
        files.sort();

        for file in files {
            rust_file_count += 1;
            let relative = workspace_relative_path(&file);
            let source = fs::read_to_string(&file)?;
            if relative == POLICY_CHECK_SOURCE {
                continue;
            }
            let direct_lines = matching_lines(
                &source,
                &[
                    "std::env::args(",
                    "std::env::args_os(",
                    "env::args(",
                    "env::args_os(",
                ],
            );
            if !direct_lines.is_empty() {
                match NORMALIZED_CLAP_ENTRY_POINTS
                    .iter()
                    .find(|(allowed, _)| *allowed == relative)
                {
                    Some((_, required_markers))
                        if has_clap
                            && required_markers.iter().all(|marker| source.contains(marker)) =>
                    {
                        normalized_entry_count += 1;
                    }
                    Some((_, required_markers)) => findings.push(format!(
                        "{relative}:{}: approved legacy normalization entry no longer proves its Clap handoff (requires Clap dependency and markers: {})",
                        direct_lines[0],
                        required_markers.join(", ")
                    )),
                    None => findings.push(format!(
                        "{relative}:{}: direct process argument access is not classified; use Clap or document a narrow exception in {POLICY_DOCUMENT}",
                        direct_lines[0]
                    )),
                }
            }

            if relative.starts_with("crates/xtask/src/") && contains_parse_options_function(&source)
            {
                findings.push(format!(
                    "{relative}: handwritten parse_*_options function detected; add typed Args to XtaskCommand"
                ));
            }

            if relative != FFI_PROTOCOL_SOURCE {
                for (line, construct) in matching_manual_diagnostics(&source) {
                    findings.push(format!(
                        "{relative}:{line}: handwritten CLI diagnostic {construct:?} detected; use typed Clap validation or classify the protocol in {POLICY_DOCUMENT}"
                    ));
                }
            }
        }
    }

    let standalone = root.join(STANDALONE_ARCHITECTURE);
    let standalone_source = fs::read_to_string(&standalone)?;
    if !contains_direct_process_args(&standalone_source) {
        findings.push(format!(
            "{STANDALONE_ARCHITECTURE}: documented dependency-free exception no longer contains its process parser; update {POLICY_DOCUMENT} and this check"
        ));
    }

    if !findings.is_empty() {
        findings.sort();
        eprintln!("cli-parser-check: {} finding(s)", findings.len());
        for finding in findings {
            eprintln!("- {finding}");
        }
        return Err("CLI parser ownership policy failed".into());
    }

    println!(
        "cli-parser-check: OK ({} workspace packages, {} binary/example targets, {} Rust files, {} normalized Clap entry points, 1 standalone architecture exception, 1 embedded FFI protocol exception)",
        packages.len(),
        target_count,
        rust_file_count,
        normalized_entry_count
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

fn collect_rust_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), io::Error> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn contains_direct_process_args(source: &str) -> bool {
    [
        "std::env::args(",
        "std::env::args_os(",
        "env::args(",
        "env::args_os(",
    ]
    .iter()
    .any(|needle| source.contains(needle))
}

fn matching_lines(source: &str, needles: &[&str]) -> Vec<usize> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            needles
                .iter()
                .any(|needle| line.contains(needle))
                .then_some(index + 1)
        })
        .collect()
}

fn contains_parse_options_function(source: &str) -> bool {
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.split("fn ").skip(1).any(|tail| {
        let Some(open) = tail.find('(') else {
            return false;
        };
        let name = tail[..open].split_whitespace().next().unwrap_or_default();
        name.starts_with("parse_") && name.ends_with("_options")
    })
}

fn matching_manual_diagnostics(source: &str) -> Vec<(usize, &'static str)> {
    let constructs = ["missing value after", "unknown option"];
    source
        .lines()
        .enumerate()
        .flat_map(|(index, line)| {
            constructs
                .into_iter()
                .filter(move |construct| line.contains(construct))
                .map(move |construct| (index + 1, construct))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_direct_process_access_and_manual_diagnostics() {
        let source = r#"
            fn main() {
                let mut args = std::env::args();
                panic!("missing value after --case");
            }
        "#;
        assert!(contains_direct_process_args(source));
        assert_eq!(
            matching_manual_diagnostics(source),
            vec![(4, "missing value after")]
        );
    }

    #[test]
    fn detects_multiline_parse_options_functions() {
        let source = r#"
            fn parse_example_options(
                args: impl Iterator<Item = String>,
            ) {
            }
        "#;
        assert!(contains_parse_options_function(source));
    }

    #[test]
    fn ignores_functions_that_only_contain_the_options_suffix() {
        assert!(!contains_parse_options_function(
            "fn parse_example_options_defaults() {}"
        ));
    }
}
