//! Code graph and public source-index generation for `xtask`.
//!
//! The `code-graphs` workflow reads Cargo metadata, emits crate dependency
//! diagrams, and builds a lightweight public item index. It intentionally
//! remains source-based; rustdoc stays the authoritative API model.

use super::*;

// ---------------------------------------------------------------------------
// `code-graphs` support
// ---------------------------------------------------------------------------

/// Minimal subset of `cargo metadata --format-version 1` used by the
/// `code-graphs` workflow.
///
/// Keeping this structure narrow makes the generator resilient to unrelated
/// metadata additions while still allowing typed access to workspace members and
/// package-level dependency data.
#[derive(Debug, Deserialize)]
pub(crate) struct CargoMetadata {
    /// All packages reported by Cargo for the current workspace query.
    packages: Vec<CargoPackage>,
    /// Package IDs that belong to the active workspace.
    workspace_members: Vec<String>,
}

/// Package metadata needed to render crate nodes, dependency edges, and the
/// public API source-scan index.
#[derive(Debug, Deserialize)]
pub(crate) struct CargoPackage {
    /// Cargo package name, used as the graph label and public API section title.
    name: String,
    /// Stable package ID from Cargo metadata.
    id: String,
    /// Declared dependencies. Path dependencies are used to identify internal
    /// workspace edges.
    dependencies: Vec<CargoDependency>,
    /// Package manifest path; its parent directory is used to map path
    /// dependencies back to package names.
    manifest_path: PathBuf,
    /// Build targets. Library-like targets provide source roots for the public
    /// item index.
    targets: Vec<CargoTarget>,
}

/// Dependency metadata needed to decide whether an edge is internal to the
/// workspace.
#[derive(Debug, Deserialize)]
pub(crate) struct CargoDependency {
    /// Dependency package name as written in Cargo metadata.
    name: String,
    /// Local path for path dependencies. Registry dependencies have no path and
    /// are intentionally omitted from internal crate graphs.
    path: Option<PathBuf>,
}

/// Target metadata used to discover crate source roots.
#[derive(Debug, Deserialize)]
pub(crate) struct CargoTarget {
    /// Cargo target kinds, for example `lib`, `rlib`, `cdylib`, or `bin`.
    kind: Vec<String>,
    /// Main source file for the target.
    src_path: PathBuf,
}

/// Parsed options for `cargo run -p xtask -- code-graphs`.
#[derive(Debug)]
pub(crate) struct CodeGraphOptions {
    /// Destination directory for generated Mermaid, DOT, SVG, README, and public
    /// API index files.
    out_dir: PathBuf,
    /// Compare the checked-in artifacts instead of rewriting them.
    check: bool,
}

impl Default for CodeGraphOptions {
    fn default() -> Self {
        Self {
            out_dir: workspace_root().join("docs/code-graphs"),
            check: false,
        }
    }
}

impl From<CodeGraphArgs> for CodeGraphOptions {
    fn from(args: CodeGraphArgs) -> Self {
        let mut options = Self {
            out_dir: args
                .out_dir
                .unwrap_or_else(|| PathBuf::from("docs/code-graphs")),
            check: args.check,
        };
        if options.out_dir.is_relative() {
            options.out_dir = workspace_root().join(&options.out_dir);
        }
        options
    }
}

/// Artifacts excluded from `--check`.
///
/// `public-api-index.md` carries source line numbers, so any code motion makes
/// it differ even when no public item changed — gating it would fire on exactly
/// the commits a restructuring produces, and a gate that cries wolf gets
/// disabled. The line-number-free `public-api-baseline.txt` gates the same
/// surface, and regenerating to satisfy that baseline refreshes the index too.
///
/// The `.svg` files are excluded because rendering them shells out to Graphviz
/// `dot`, which CI runners do not install; they are derived from the `.dot`
/// artifacts that *are* checked.
const CHECK_EXEMPT_ARTIFACTS: [&str; 4] = [
    "public-api-index.md",
    "workspace-crates.svg",
    "internal-crate-deps.svg",
    "ir-overview.svg",
];

/// One public source item found by the lightweight public API scanner.
///
/// This is deliberately a source index, not a semantic model. Rustdoc remains
/// the authoritative API representation; this index is optimized for quick
/// navigation across the workspace.
#[derive(Debug)]
pub(crate) struct PublicItem {
    /// Item category such as `struct`, `enum`, `trait`, `fn`, or `use`.
    pub(crate) kind: String,
    /// Parsed item name. For re-exports, this is the first path-like token after
    /// `pub use`.
    pub(crate) name: String,
    /// Source file containing the public item.
    pub(crate) path: PathBuf,
    /// One-based source line number.
    pub(crate) line: usize,
}

/// Generates workspace/dependency graphs, IR overview graphs, and a public API
/// index for developer navigation.
///
/// Output is deterministic for stable `cargo metadata` and stable source file
/// ordering. SVG generation requires Graphviz `dot`; failing to render one SVG
/// fails the workflow so checked-in visualizations do not silently go stale.
pub(crate) fn code_graphs(args: CodeGraphArgs) -> Result<(), Box<dyn std::error::Error>> {
    let options = CodeGraphOptions::from(args);

    let metadata = load_cargo_metadata()?;
    let workspace = workspace_packages(&metadata);
    let edges = internal_dependency_edges(&workspace);

    let artifacts: Vec<(PathBuf, String)> = vec![
        (
            options.out_dir.join("workspace-crates.mmd"),
            render_workspace_mermaid(&workspace),
        ),
        (
            options.out_dir.join("workspace-crates.dot"),
            render_workspace_dot(&workspace),
        ),
        (
            options.out_dir.join("internal-crate-deps.mmd"),
            render_internal_deps_mermaid(&workspace, &edges),
        ),
        (
            options.out_dir.join("internal-crate-deps.dot"),
            render_internal_deps_dot(&workspace, &edges),
        ),
        (
            options.out_dir.join("ir-overview.mmd"),
            render_ir_overview_mermaid(),
        ),
        (
            options.out_dir.join("ir-overview.dot"),
            render_ir_overview_dot(),
        ),
        (
            options.out_dir.join("public-api-baseline.txt"),
            render_public_api_baseline(&workspace)?,
        ),
        (
            options.out_dir.join("public-api-index.md"),
            render_public_api_index(&workspace)?,
        ),
        (
            options.out_dir.join("README.md"),
            render_code_graphs_readme(),
        ),
    ];

    if options.check {
        return check_code_graphs(&options.out_dir, &artifacts);
    }

    fs::create_dir_all(&options.out_dir)?;
    for (path, text) in &artifacts {
        write_text(path, text)?;
    }
    render_svg_with_dot(
        &options.out_dir.join("workspace-crates.dot"),
        &options.out_dir.join("workspace-crates.svg"),
    )?;
    render_svg_with_dot(
        &options.out_dir.join("internal-crate-deps.dot"),
        &options.out_dir.join("internal-crate-deps.svg"),
    )?;
    render_svg_with_dot(
        &options.out_dir.join("ir-overview.dot"),
        &options.out_dir.join("ir-overview.svg"),
    )?;

    println!(
        "wrote code graph documentation to {}",
        workspace_relative_path(&options.out_dir)
    );
    Ok(())
}

/// Compares freshly rendered artifacts against the checked-in ones.
///
/// Findings are sorted and reported together so one run tells the whole story
/// rather than stopping at the first difference. Regeneration is deliberately
/// manual: `code-graphs` without `--check` is the bless path, so a moved crate
/// boundary is reviewed in a diff instead of being silently absorbed.
pub(crate) fn check_code_graphs(
    out_dir: &Path,
    artifacts: &[(PathBuf, String)],
) -> Result<(), Box<dyn std::error::Error>> {
    if !out_dir.is_dir() {
        return Err(format!(
            "{} does not exist — run `cargo run -p xtask -- code-graphs` first",
            workspace_relative_path(out_dir)
        )
        .into());
    }

    let mut findings: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for (path, expected) in artifacts {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if CHECK_EXEMPT_ARTIFACTS.contains(&name.as_str()) {
            continue;
        }
        checked += 1;
        let rel = workspace_relative_path(path);
        match fs::read_to_string(path) {
            Ok(recorded) if &recorded == expected => {}
            Ok(_) => findings.push(format!("{rel}: differs from the generated artifact")),
            Err(_) => findings.push(format!("{rel}: missing")),
        }
    }

    findings.sort();
    if findings.is_empty() {
        println!("code-graphs --check: OK ({checked} artifacts identical)");
        return Ok(());
    }

    let mut report = String::from("code-graphs --check failed:\n");
    for finding in &findings {
        let _ = writeln!(report, "  - {finding}");
    }
    report.push_str(
        "\nRegenerate with `cargo run -p xtask -- code-graphs`, review the diff, and commit it.\n",
    );
    Err(report.into())
}

/// Loads typed workspace metadata by invoking Cargo rather than parsing
/// manifests by hand.
///
/// This keeps feature resolution, package IDs, target metadata, and path
/// dependencies aligned with Cargo's own view of the workspace.
pub(crate) fn load_cargo_metadata() -> Result<CargoMetadata, Box<dyn std::error::Error>> {
    let output = Command::new("cargo")
        .current_dir(workspace_root())
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Returns workspace packages sorted by package name.
///
/// Sorting makes generated graph files stable across platforms and independent
/// of the ordering returned by Cargo.
pub(crate) fn workspace_packages(metadata: &CargoMetadata) -> Vec<&CargoPackage> {
    let members: BTreeSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let mut packages: Vec<&CargoPackage> = metadata
        .packages
        .iter()
        .filter(|package| members.contains(package.id.as_str()))
        .collect();
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    packages
}

/// Builds deterministic dependency edges between workspace packages.
///
/// Registry dependencies are ignored. Path dependencies are matched through
/// manifest parent directories so package renames in `Cargo.toml` still resolve
/// to the actual workspace package name.
pub(crate) fn internal_dependency_edges(packages: &[&CargoPackage]) -> Vec<(String, String)> {
    let manifest_parent_to_name: BTreeMap<PathBuf, String> = packages
        .iter()
        .filter_map(|package| {
            package
                .manifest_path
                .parent()
                .map(|parent| (parent.to_path_buf(), package.name.clone()))
        })
        .collect();
    let names: BTreeSet<&str> = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let mut edges = BTreeSet::new();

    for package in packages {
        for dep in &package.dependencies {
            let dep_name = dep
                .path
                .as_ref()
                .and_then(|path| manifest_parent_to_name.get(path))
                .map_or(dep.name.as_str(), String::as_str);
            if names.contains(dep_name) {
                edges.insert((package.name.clone(), dep_name.to_owned()));
            }
        }
    }

    edges.into_iter().collect()
}

/// Renders a Mermaid graph containing one node per workspace crate.
pub(crate) fn render_workspace_mermaid(packages: &[&CargoPackage]) -> String {
    let mut out = String::from("flowchart LR\n");
    for package in packages {
        let id = graph_id(&package.name);
        let label = mermaid_label(&package.name);
        let _ = writeln!(out, "    {id}[\"{label}\"]");
    }
    out
}

/// Renders a DOT graph containing one node per workspace crate.
pub(crate) fn render_workspace_dot(packages: &[&CargoPackage]) -> String {
    let mut out = String::from("digraph workspace_crates {\n");
    out.push_str("    rankdir=LR;\n");
    out.push_str("    node [shape=box, style=\"rounded\"];\n");
    for package in packages {
        let _ = writeln!(out, "    \"{}\";", dot_escape(&package.name));
    }
    out.push_str("}\n");
    out
}

/// Renders a Mermaid graph of internal workspace crate dependencies.
pub(crate) fn render_internal_deps_mermaid(
    packages: &[&CargoPackage],
    edges: &[(String, String)],
) -> String {
    let mut out = render_workspace_mermaid(packages);
    for (from, to) in edges {
        let _ = writeln!(out, "    {} --> {}", graph_id(from), graph_id(to));
    }
    out
}

/// Renders a DOT graph of internal workspace crate dependencies.
pub(crate) fn render_internal_deps_dot(
    packages: &[&CargoPackage],
    edges: &[(String, String)],
) -> String {
    let mut out = render_workspace_dot(packages);
    out.truncate(out.trim_end_matches("}\n").len());
    for (from, to) in edges {
        let _ = writeln!(
            out,
            "    \"{}\" -> \"{}\";",
            dot_escape(from),
            dot_escape(to)
        );
    }
    out.push_str("}\n");
    out
}

/// Renders a curated Mermaid overview of the main Faust IR layers.
///
/// This graph is intentionally hand-authored: it documents architectural
/// relationships that cannot be inferred reliably from Cargo dependencies
/// alone.
pub(crate) fn render_ir_overview_mermaid() -> String {
    String::from(
        "flowchart LR\n\
         \n\
             subgraph Boxes[\"boxes IR\"]\n\
                 BoxBuilder[\"BoxBuilder\"]\n\
                 BoxTree[\"BoxId / TreeArena\"]\n\
                 BoxMatch[\"match_box / BoxMatch\"]\n\
                 BoxBuilder --> BoxTree --> BoxMatch\n\
             end\n\
         \n\
             subgraph UI[\"ui IR\"]\n\
                 UiProgram[\"UiProgram\"]\n\
                 UiItems[\"groups / controls / metadata\"]\n\
                 UiProgram --> UiItems\n\
             end\n\
         \n\
             subgraph Signals[\"signals IR\"]\n\
                 SigBuilder[\"SigBuilder\"]\n\
                 SigTree[\"SigId / TreeArena\"]\n\
                 SigMatch[\"match_sig / SigMatch\"]\n\
                 AdRules[\"ad_rules\"]\n\
                 SigBuilder --> SigTree --> SigMatch\n\
                 AdRules --> SigBuilder\n\
             end\n\
         \n\
             subgraph FIR[\"fir IR\"]\n\
                 FirStore[\"FirStore\"]\n\
                 FirModule[\"FirModule\"]\n\
                 FirVerifier[\"checker\"]\n\
                 FirInliner[\"inliner\"]\n\
                 FirStore --> FirModule --> FirVerifier\n\
                 FirModule --> FirInliner\n\
             end\n\
         \n\
             boxes[\"boxes crate\"] --> eval[\"eval crate\"] --> propagate[\"propagate crate\"]\n\
             propagate --> Signals\n\
             propagate --> UI\n\
             Signals --> transform[\"transform crate\"] --> FIR --> codegen[\"codegen crate\"]\n",
    )
}

/// Renders the same curated IR overview as DOT for Graphviz/SVG output.
pub(crate) fn render_ir_overview_dot() -> String {
    String::from(
        "digraph ir_overview {\n\
             rankdir=LR;\n\
             node [shape=box, style=\"rounded\"];\n\
             \"boxes::BoxBuilder\" -> \"BoxId / TreeArena\" -> \"match_box\";\n\
             \"ui::UiProgram\" -> \"UI groups/controls/metadata\";\n\
             \"signals::SigBuilder\" -> \"SigId / TreeArena\" -> \"match_sig\";\n\
             \"signals::ad_rules\" -> \"signals::SigBuilder\";\n\
             \"fir::FirStore\" -> \"FirModule\" -> \"fir::checker\";\n\
             \"FirModule\" -> \"fir::inliner\";\n\
             \"boxes\" -> \"eval\" -> \"propagate\";\n\
             \"propagate\" -> \"signals\";\n\
             \"propagate\" -> \"ui\";\n\
             \"signals\" -> \"transform\" -> \"fir\" -> \"codegen\";\n\
         }\n",
    )
}

/// Renders a Markdown index of public source items for every workspace package.
///
/// The scan is intentionally syntactic and conservative. It is useful for
/// navigation and broad API inventory, but it does not expand macros, resolve
/// visibility through modules, or replace rustdoc.
pub(crate) fn render_public_api_index(
    packages: &[&CargoPackage],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::from(
        "# Public API Index\n\n\
         Generated by `cargo run -p xtask -- code-graphs`.\n\n\
         This is a lightweight source scan for public items. Use `cargo doc \
         --workspace --no-deps` for authoritative Rust API documentation.\n",
    );

    for package in packages {
        let items = public_items_for_package(package)?;
        let _ = writeln!(out, "\n## `{}`\n", package.name);
        if items.is_empty() {
            out.push_str("_No direct public items found by the source scan._\n");
            continue;
        }
        out.push_str("| Kind | Name | Location |\n|---|---|---|\n");
        for item in items {
            let rel = workspace_relative_path(&item.path);
            let _ = writeln!(
                out,
                "| `{}` | `{}` | `{rel}:{}` |",
                item.kind, item.name, item.line
            );
        }
    }

    Ok(out)
}

/// Renders the gated, line-number-free inventory of each crate's public surface.
///
/// This is the artifact `--check` compares. It deliberately drops the source
/// locations kept by [`render_public_api_index`] and deduplicates `(kind, name)`
/// pairs, so the file changes when a crate exposes something new — or stops
/// exposing something — and stays byte-identical when code merely moves.
///
/// The failure mode it exists to catch is the `pub` leak: relocating a function
/// out of a crowded module often means widening a `pub(crate)` helper to `pub`
/// so the move compiles. Nothing else in the workspace notices, and the crate
/// boundary a restructuring set out to clarify quietly grows instead.
pub(crate) fn render_public_api_baseline(
    packages: &[&CargoPackage],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::from(
        "# Public API baseline\n\
         #\n\
         # Generated by `cargo run -p xtask -- code-graphs`, verified by\n\
         # `cargo run -p xtask -- code-graphs --check`.\n\
         #\n\
         # One line per public item, as `<kind> <name>`, sorted and deduplicated.\n\
         # Source locations live in `public-api-index.md` instead: this file gates\n\
         # the shape of each public surface, not where the code sits.\n\
         #\n\
         # A diff here is a moved crate boundary. Regenerate to accept it.\n",
    );

    for package in packages {
        let items = public_items_for_package(package)?;
        out.push_str(&render_baseline_section(&package.name, items));
    }

    Ok(out)
}

/// Renders one crate's section of the public API baseline.
///
/// Split out from [`render_public_api_baseline`] so the two properties the gate
/// depends on — blindness to source location, sensitivity to the surface itself
/// — are unit-testable without invoking `cargo metadata`.
pub(crate) fn render_baseline_section(package_name: &str, items: Vec<PublicItem>) -> String {
    let entries: BTreeSet<(String, String)> = items
        .into_iter()
        .map(|item| (item.kind, item.name))
        .collect();
    let mut out = String::new();
    let _ = writeln!(out, "\n## {package_name}");
    for (kind, name) in entries {
        let _ = writeln!(out, "{kind} {name}");
    }
    out
}

/// Collects public source items for a single package by scanning library-like
/// targets.
///
/// FFI crates in this workspace use target kinds such as `rlib`, `staticlib`,
/// and `cdylib`, so all library-like target kinds are included.
pub(crate) fn public_items_for_package(
    package: &CargoPackage,
) -> Result<Vec<PublicItem>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    for target in &package.targets {
        if target.kind.iter().any(|kind| {
            matches!(
                kind.as_str(),
                "lib" | "rlib" | "dylib" | "staticlib" | "cdylib"
            )
        }) {
            collect_rs_files(
                target
                    .src_path
                    .parent()
                    .ok_or("library target has no source parent")?,
                &mut files,
            )?;
        }
    }
    files.sort();

    let mut items = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file)?;
        // Track the enclosing `impl` so methods are recorded as `Type::method`.
        // Without this, `SignalFirRequest::new` and every other constructor
        // collapse onto the single name `new` in the baseline, and adding a
        // method to a new type is invisible to `--check`.
        let mut depth: i32 = 0;
        let mut impl_type: Option<String> = None;
        for (idx, line) in text.lines().enumerate() {
            if depth == 0 {
                impl_type = parse_impl_header(line);
            }
            if let Some((kind, name)) = parse_public_item_line(line) {
                let name = match impl_type.as_deref() {
                    Some(ty) if depth > 0 && kind == "fn" => format!("{ty}::{name}"),
                    _ => name,
                };
                items.push(PublicItem {
                    kind,
                    name,
                    path: file.clone(),
                    line: idx + 1,
                });
            }
            depth += brace_balance(line);
            if depth <= 0 {
                depth = 0;
                impl_type = None;
            }
        }
    }
    items.sort_by(|left, right| {
        workspace_relative_path(&left.path)
            .cmp(&workspace_relative_path(&right.path))
            .then(left.line.cmp(&right.line))
    });
    Ok(items)
}

/// Recursively collects Rust source files below `dir`.
pub(crate) fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), io::Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// Extracts the implementing type from an `impl` header line.
///
/// Handles the shapes this workspace uses: `impl Type {`, `impl<'a> Type<'a> {`,
/// and `impl Trait for Type {` — the last one yielding `Type`, since that is the
/// item a reader looks up. Returns `None` for any line that does not open an
/// `impl` at the start of the line, which keeps nested or indented blocks out.
pub(crate) fn parse_impl_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix("impl")?;
    let rest = match rest.strip_prefix('<') {
        // Skip the generic parameter list introduced right after `impl`.
        Some(after) => {
            let mut depth = 1usize;
            let mut end = 0usize;
            for (offset, ch) in after.char_indices() {
                match ch {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = offset + ch.len_utf8();
                            break;
                        }
                    }
                    _ => {}
                }
            }
            &after[end..]
        }
        None => rest,
    };
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    // `impl Trait for Type` names the type after `for`.
    let target = rest.rsplit(" for ").next().unwrap_or(rest).trim_start();
    let target = target
        .split_whitespace()
        .next()?
        .split('<')
        .next()?
        .split('{')
        .next()?;
    let name = target.rsplit("::").next()?.trim();
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    Some(name.to_owned())
}

/// Net brace balance of one source line, ignoring string literals and the
/// trailing line comment.
///
/// Deliberately crude, consistent with the rest of this scanner: it is enough to
/// tell whether the current line still sits inside an `impl` body.
pub(crate) fn brace_balance(line: &str) -> i32 {
    let mut balance = 0;
    let mut in_string = false;
    let mut chars = line.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        match ch {
            '\\' if in_string => {
                chars.next();
            }
            '"' => in_string = !in_string,
            '/' if !in_string && line[idx..].starts_with("//") => break,
            '{' if !in_string => balance += 1,
            '}' if !in_string => balance -= 1,
            _ => {}
        }
    }
    balance
}

/// Parses one source line as a top-level public item declaration.
///
/// The parser deliberately ignores `pub(crate)`, `pub(super)`, and `pub(self)`
/// items because the generated index is meant to approximate externally visible
/// API surfaces. Multi-line signatures are represented by the first line only.
pub(crate) fn parse_public_item_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || !trimmed.starts_with("pub ") {
        return None;
    }
    let rest = trimmed.strip_prefix("pub ")?.trim_start();
    if rest.starts_with("crate")
        || rest.starts_with("(crate)")
        || rest.starts_with("(super)")
        || rest.starts_with("(self)")
        || rest.starts_with("super")
        || rest.starts_with("self")
    {
        return None;
    }

    let rest = rest
        .strip_prefix("unsafe ")
        .unwrap_or(rest)
        .strip_prefix("extern ")
        .unwrap_or(rest)
        .strip_prefix("async ")
        .unwrap_or(rest);
    let rest = rest.strip_prefix("\"C\" ").unwrap_or(rest);

    for kind in [
        "struct", "enum", "trait", "fn", "type", "const", "static", "mod", "use",
    ] {
        if let Some(after_kind) = rest
            .strip_prefix(kind)
            .and_then(|tail| tail.strip_prefix(' '))
        {
            return Some((kind.to_owned(), parse_item_name(after_kind)));
        }
    }
    None
}

/// Extracts the first identifier/path-like token after a recognized item kind.
pub(crate) fn parse_item_name(input: &str) -> String {
    let name: String = input
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == ':')
        .collect();
    if name.is_empty() {
        "_".to_owned()
    } else {
        name.trim_end_matches(':').to_owned()
    }
}

/// Renders the generated `docs/code-graphs/README.md` entry point.
pub(crate) fn render_code_graphs_readme() -> String {
    String::from(
        "# Code Graphs\n\n\
         Generated by:\n\n\
         ```bash\n\
         cargo run -p xtask -- code-graphs\n\
         ```\n\n\
         Verified in CI by:\n\n\
         ```bash\n\
         cargo run -p xtask -- code-graphs --check\n\
         ```\n\n\
         The check compares every artifact below except `public-api-index.md` \
         (its source line numbers make code motion look like an API change) and \
         the `.svg` renderings (they need Graphviz `dot`, which CI does not \
         install, and they derive from the checked `.dot` files). Regenerating \
         is the bless path: run the generator, review the diff, commit it.\n\n\
         Files:\n\n\
         - `workspace-crates.mmd` / `workspace-crates.dot` / `workspace-crates.svg`: workspace crate nodes from `cargo metadata`.\n\
         - `internal-crate-deps.mmd` / `internal-crate-deps.dot` / `internal-crate-deps.svg`: internal crate dependency edges from `cargo metadata`.\n\
         - `ir-overview.mmd` / `ir-overview.dot` / `ir-overview.svg`: curated overview of the main `boxes`, `signals`, `fir`, and `ui` IR relationships.\n\
         - `public-api-baseline.txt`: gated inventory of every crate's public items, without source locations. A diff here means a crate boundary moved.\n\
         - `public-api-index.md`: lightweight source-scan index of public items, with locations. Use Rustdoc as the authoritative API reference.\n\
         \n\
         ## Rendered SVG\n\n\
         ### Workspace Crates\n\n\
         ![Workspace crates](workspace-crates.svg)\n\n\
         ### Internal Crate Dependencies\n\n\
         ![Internal crate dependencies](internal-crate-deps.svg)\n\n\
         ### IR Overview\n\n\
         ![IR overview](ir-overview.svg)\n",
    )
}

/// Writes a UTF-8 text artifact.
pub(crate) fn write_text(path: &Path, text: &str) -> Result<(), io::Error> {
    fs::write(path, text)
}

/// Converts one DOT file to SVG with Graphviz `dot`.
///
/// The generated SVG is checked in so users can inspect graphs without local
/// Mermaid or Graphviz tooling. Regeneration still requires `dot`.
pub(crate) fn render_svg_with_dot(
    dot_path: &Path,
    svg_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("dot")
        .arg("-Tsvg")
        .arg(dot_path)
        .arg("-o")
        .arg(svg_path)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to render {} with Graphviz `dot`",
            workspace_relative_path(dot_path)
        )
        .into())
    }
}

/// Converts a crate name into a Mermaid-safe node identifier.
pub(crate) fn graph_id(name: &str) -> String {
    let mut out = String::from("crate_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

/// Escapes a label for Mermaid quoted-node syntax.
pub(crate) fn mermaid_label(label: &str) -> String {
    label.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Escapes a label for DOT quoted strings.
pub(crate) fn dot_escape(label: &str) -> String {
    label.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Enumerates all compile corpus `.dsp` files in deterministic order.
pub(crate) fn corpus_files() -> Result<Vec<PathBuf>, io::Error> {
    let root = workspace_root();
    let corpus_dir = root.join("tests/corpus");
    let mut files = Vec::new();

    for entry in fs::read_dir(corpus_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "dsp") {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
}

/// Enumerates all runtime corpus `.dsp` files in deterministic order.
pub(crate) fn runtime_corpus_files() -> Result<Vec<PathBuf>, io::Error> {
    let root = workspace_root();
    let dir = root.join("tests/runtime_corpus");
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "dsp") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Returns the root directory for persisted runtime trace snapshots.
pub(crate) fn runtime_trace_snapshot_root() -> PathBuf {
    workspace_root().join("tests/runtime_traces").join("rust")
}
