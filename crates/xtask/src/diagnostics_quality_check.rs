//! Architectural guard for the diagnostics v2 machine contract.
//!
//! Two properties are enforced, both of them silent-breakage risks that no
//! unit test naturally covers:
//!
//! 1. **The published schema keeps up with the model.** Every variant of a
//!    diagnostics enum that reaches JSON must appear in the matching schema
//!    enum. Adding a variant without updating the schema produces payloads
//!    that no longer validate against the contract consumers were handed.
//!
//! 2. **No new machine protocol hides in note text.** Diagnostics v2 exists so
//!    tools read typed fields; a renderer that recovers meaning by matching a
//!    note prefix reintroduces exactly the fragility v2 removed. Two legacy
//!    sites are allowed by name and must not grow.

use super::*;

const CHECK_SOURCE: &str = "crates/xtask/src/diagnostics_quality_check.rs";
const SCHEMA: &str = "docs/diagnostics-v2.schema.json";
const CODE_TABLE: &str = "docs/diagnostics-codes-en.md";

/// Rust enums whose variants are serialized as schema enum values.
///
/// The pair is `(Rust type in crates/diagnostics, JSON pointer to the schema
/// enum)`. Rust variants are compared after converting `CamelCase` to
/// `snake_case`, which is exactly what the renderer's `*_name` functions do.
const SERIALIZED_ENUMS: &[(&str, &str)] = &[
    ("Severity", "/$defs/diagnostic/properties/severity/enum"),
    ("Stage", "/$defs/diagnostic/properties/stage/enum"),
    (
        "DiagnosticCategory",
        "/$defs/diagnostic/properties/category/enum",
    ),
    ("LabelRole", "/$defs/label/properties/role/enum"),
    ("TraceKind", "/$defs/trace/properties/kind/enum"),
    ("Applicability", "/$defs/fix/properties/applicability/enum"),
    ("SourceKind", "/$defs/source/properties/kind/enum"),
];

/// Files permitted to branch on note text, with the reason.
///
/// Every entry is a *presentation* decision — what a reader sees, in what
/// order — never a fact a tool consumes:
///
/// - `human.rs` condenses the paired composition block a propagate diagnostic
///   writes for that purpose, and hides internal IR previews below
///   `--error-verbosity debug`;
/// - `diagnostics/src/lib.rs` sorts notes into the canonical
///   cause/rule/computed skeleton.
///
/// This list must not grow. A new consumer of note text is the regression this
/// gate exists to catch.
const NOTE_MATCHING_ALLOWLIST: &[&str] = &[
    "crates/compiler/src/cli/human.rs",
    "crates/diagnostics/src/lib.rs",
];

/// Enforces the diagnostics v2 machine contract.
pub(crate) fn diagnostics_quality_check() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let mut findings = Vec::new();

    let schema_text = fs::read_to_string(root.join(SCHEMA))?;
    let schema: serde_json::Value = serde_json::from_str(&schema_text)?;
    let model = fs::read_to_string(root.join("crates/diagnostics/src/lib.rs"))?;
    let model_v2 = fs::read_to_string(root.join("crates/diagnostics/src/model_v2.rs"))?;
    let source_model = fs::read_to_string(root.join("crates/diagnostics/src/source.rs"))?;
    let all_model = format!("{model}\n{model_v2}\n{source_model}");

    for (rust_enum, pointer) in SERIALIZED_ENUMS {
        check_enum_coverage(&all_model, rust_enum, &schema, pointer, &mut findings);
    }

    let codes = fs::read_to_string(root.join("crates/diagnostics/src/codes.rs"))?;
    check_code_registry(
        &codes,
        &fs::read_to_string(root.join(CODE_TABLE))?,
        &mut findings,
    );

    let mut sources = Vec::new();
    collect_files(&root.join("crates"), "rs", &mut sources)?;
    sources.sort();
    let mut scanned = 0usize;
    for path in &sources {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if NOTE_MATCHING_ALLOWLIST.contains(&relative.as_str())
            || relative == CHECK_SOURCE
            || relative.contains("/tests/")
            || relative.ends_with("/tests.rs")
        {
            continue;
        }
        scanned += 1;
        check_note_protocols(&relative, &fs::read_to_string(path)?, &mut findings);
    }

    if findings.is_empty() {
        println!(
            "diagnostics-quality-check: OK ({} serialized enums, {scanned} Rust files scanned)",
            SERIALIZED_ENUMS.len()
        );
        return Ok(());
    }
    for finding in &findings {
        println!("diagnostics-quality-check: {finding}");
    }
    Err(format!("diagnostics-quality-check: {} finding(s)", findings.len()).into())
}

/// Requires every Rust variant of one serialized enum to exist in the schema.
///
/// The reverse direction is intentionally not an error: a schema may keep a
/// retired value so old payloads still validate.
fn check_enum_coverage(
    model: &str,
    rust_enum: &str,
    schema: &serde_json::Value,
    pointer: &str,
    findings: &mut Vec<String>,
) {
    let Some(variants) = enum_variants(model, rust_enum) else {
        findings.push(format!(
            "cannot find `pub enum {rust_enum}` in crates/diagnostics; update {CHECK_SOURCE}"
        ));
        return;
    };
    let Some(schema_values) = schema
        .pointer(pointer)
        .and_then(serde_json::Value::as_array)
    else {
        findings.push(format!("schema {SCHEMA} has no enum at {pointer}"));
        return;
    };
    let declared = schema_values
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<BTreeSet<_>>();
    for variant in variants {
        let serialized = camel_to_snake(&variant);
        if !declared.contains(serialized.as_str()) {
            findings.push(format!(
                "{rust_enum}::{variant} serializes as `{serialized}` but {pointer} does not list it"
            ));
        }
    }
}

/// Extracts the variant names of one `pub enum` from Rust source.
///
/// Deliberately textual: this gate runs against the source of truth, so a
/// dependency on the compiled crate would let a mismatch build before it is
/// caught.
fn enum_variants(source: &str, name: &str) -> Option<Vec<String>> {
    let needle = format!("pub enum {name} {{");
    let start = source.find(&needle)? + needle.len();
    let body = &source[start..];
    let end = body.find("\n}")?;
    let mut variants = Vec::new();
    for line in body[..end].lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let variant = line.trim_end_matches(',').trim();
        if variant
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
            && variant.chars().all(|c| c.is_ascii_alphanumeric())
        {
            variants.push(variant.to_owned());
        }
    }
    Some(variants)
}

/// Converts `CamelCase` to the `snake_case` spelling used in JSON.
fn camel_to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.char_indices() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Requires every declared code to be listed in `all_codes()` and documented.
///
/// A code missing from the registry is invisible to tooling that enumerates
/// them; a code missing from the table has no published meaning.
fn check_code_registry(codes: &str, table: &str, findings: &mut Vec<String>) {
    let Some(registry_start) = codes.find("pub fn all_codes()") else {
        findings.push("cannot find `all_codes()` in crates/diagnostics/src/codes.rs".to_owned());
        return;
    };
    let (declarations, registry) = codes.split_at(registry_start);

    for line in declarations.lines() {
        let Some(rest) = line.trim().strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let Some(code) = value
            .split('"')
            .nth(1)
            .filter(|code| code.starts_with("FRS-"))
        else {
            continue;
        };
        if !registry.contains(name) {
            findings.push(format!("{name} ({code}) is not listed in `all_codes()`"));
        }
        if !table.contains(code) {
            findings.push(format!("{code} is not documented in {CODE_TABLE}"));
        }
    }
}

/// Rejects code that derives machine meaning from note text.
fn check_note_protocols(path: &str, source: &str, findings: &mut Vec<String>) {
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("///") {
            continue;
        }
        let mentions_note = trimmed.contains("note") || trimmed.contains("notes");
        if mentions_note
            && (trimmed.contains(".starts_with(") || trimmed.contains(".strip_prefix("))
        {
            findings.push(format!(
                "{path}:{}: derives machine meaning from note text; put it in a typed fact instead",
                index + 1
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{camel_to_snake, check_note_protocols, enum_variants};

    #[test]
    fn camel_case_variants_map_to_snake_case_json_values() {
        assert_eq!(camel_to_snake("PrimaryCause"), "primary_cause");
        assert_eq!(camel_to_snake("Fir"), "fir");
        assert_eq!(camel_to_snake("MachineApplicable"), "machine_applicable");
    }

    #[test]
    fn enum_variants_skips_docs_and_attributes() {
        let source = "pub enum Kind {\n    /// doc\n    #[default]\n    First,\n    Second,\n}\n";
        assert_eq!(
            enum_variants(source, "Kind"),
            Some(vec!["First".to_owned(), "Second".to_owned()])
        );
    }

    #[test]
    fn a_note_prefix_protocol_is_rejected() {
        let mut findings = Vec::new();
        check_note_protocols(
            "a.rs",
            "if note.starts_with(\"binding_trace=\") { }",
            &mut findings,
        );
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn ordinary_code_and_comments_are_accepted() {
        let mut findings = Vec::new();
        check_note_protocols(
            "a.rs",
            "// note.starts_with is fine in prose\nlet x = label.starts_with(\"A\");",
            &mut findings,
        );
        assert!(findings.is_empty(), "{findings:?}");
    }
}
