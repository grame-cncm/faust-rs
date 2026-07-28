//! Source reader for the production parser crate.
//!
//! # Source provenance (C++)
//! - `compiler/parser/sourcereader.hh`
//! - `compiler/parser/sourcereader.cpp`
//!
//! # Scope
//! - Search-path based import resolution.
//! - Recursive import expansion with cycle detection.
//! - Read cache and used-file tracking for deterministic parser runs.
//! - Local-file import policy in current parser scope:
//!   - URL/network fetch is intentionally out-of-scope in `parser` and tracked as deferred
//!     in Phase 3 porting docs (no temporary network stub in this crate).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use diagnostics::{
    Diagnostic, DiagnosticBundle, DiagnosticCode, Label, LabelRole, LabelStyle, Severity,
    SourceSpan, Stage, codes,
};

/// One source-origin marker for a line in expanded source text.
#[derive(Debug, Clone, PartialEq, Eq)]
/// Origin information for one expanded source line.
pub struct SourceLineOrigin {
    /// Canonical file path where this expanded line originates.
    pub file: PathBuf,
    /// 1-based line number in the original source file.
    pub line: u32,
}

/// Expanded source payload returned by [`SourceReader`], including per-line origin mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
/// Result of recursively expanding one Faust source file with imports.
pub struct ExpandedSource {
    /// Expanded source text after recursive import substitution.
    pub text: Box<str>,
    /// Origin for each line in `text` (same ordering, 1:1 mapping).
    pub line_origins: Vec<SourceLineOrigin>,
}

/// Read-only in-memory source bundle used to resolve `import("...")` without
/// relying on a host filesystem.
///
/// # Purpose
/// This is the Rust-side transport for embedded Faust library sources used by
/// the `faustwasm` compiler-module path. It keeps import resolution keyed by
/// stable logical paths such as `stdfaust.lib` or `music.lib` while remaining
/// usable in native tests.
///
/// # Invariants
/// - keys are normalized logical paths with `.` segments removed;
/// - relative logical paths are preserved as relative paths;
/// - values are immutable UTF-8 source strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtualSourceMap {
    entries: Arc<HashMap<PathBuf, Arc<str>>>,
}

impl VirtualSourceMap {
    /// Builds one immutable source bundle from `(logical_path, source)` pairs.
    #[must_use]
    pub fn new(entries: impl IntoIterator<Item = (PathBuf, String)>) -> Self {
        let mut out = HashMap::new();
        for (path, source) in entries {
            out.insert(normalize_logical_path(&path), Arc::<str>::from(source));
        }
        Self {
            entries: Arc::new(out),
        }
    }

    /// Returns `true` when the bundle has no registered logical sources.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the normalized source text for one logical path, if present.
    #[must_use]
    pub fn get(&self, path: &Path) -> Option<&str> {
        self.entries
            .get(&normalize_logical_path(path))
            .map(AsRef::as_ref)
    }

    /// Returns `true` when one logical path exists in the bundle.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(&normalize_logical_path(path))
    }

    /// Returns all logical source entries in deterministic path order.
    pub fn iter(&self) -> impl Iterator<Item = (&Path, &str)> {
        let mut ordered: Vec<_> = self.entries.iter().collect();
        ordered.sort_by_key(|(path, _)| *path);
        ordered
            .into_iter()
            .map(|(path, source)| (path.as_path(), source.as_ref()))
    }

    /// Returns a new bundle extended with one extra logical source.
    #[must_use]
    pub fn with_source(&self, path: impl Into<PathBuf>, source: impl Into<String>) -> Self {
        let mut entries = (*self.entries).clone();
        entries.insert(
            normalize_logical_path(&path.into()),
            Arc::<str>::from(source.into()),
        );
        Self {
            entries: Arc::new(entries),
        }
    }
}

/// Source location of an `import(...)` directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSite {
    /// 1-based line of the directive.
    pub line: u32,
    /// 1-based column of the import name within that line.
    pub col: u32,
}

/// One directed edge in a detected import cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCycleEdge {
    /// File containing the import directive.
    pub from: PathBuf,
    /// Resolved file named by the directive.
    pub to: PathBuf,
    /// Location of the import name in `from`, when recoverable.
    pub site: Option<ImportSite>,
}

impl ImportSite {
    /// Locates the first `import("<name>");` directive in `text`.
    ///
    /// Used by the box-level expansion path, which resolves imports from an
    /// already-parsed tree: box nodes carry no source location, so recovering
    /// the span means re-scanning the file. That work happens only on the error
    /// path, never during a successful compile.
    ///
    /// Reuses the same line recognizer as expansion, so a commented-out or
    /// malformed directive is not mistaken for the real one. If the same name is
    /// imported more than once, the first occurrence wins.
    #[must_use]
    pub fn locate_in(text: &str, name: &str) -> Option<Self> {
        let mut in_block_comment = false;
        for (line_index, line) in text.lines().enumerate() {
            let line_starts_in_comment = in_block_comment;
            in_block_comment = SourceReader::advance_block_comment_state(in_block_comment, line);
            if line_starts_in_comment {
                continue;
            }
            if parse_import_line(line).as_deref() != Some(name) {
                continue;
            }
            let col = line
                .find(name)
                .map_or(1, |byte_idx| line[..byte_idx].chars().count() + 1);
            return Some(Self {
                line: u32::try_from(line_index + 1).unwrap_or(u32::MAX),
                col: u32::try_from(col).unwrap_or(1),
            });
        }
        None
    }
}

/// Errors returned by [`SourceReader`] during source loading and import expansion.
///
/// Each variant maps to one stable `FRS-SRC-*` diagnostic code; see
/// [`SourceReaderError::to_diagnostics`] and `docs/faust-error-model-en.md`.
#[derive(Debug)]
pub enum SourceReaderError {
    Io {
        path: PathBuf,
        message: Box<str>,
    },
    UnresolvedImport {
        name: Box<str>,
        from: PathBuf,
        /// Location of the `import(...)` directive, when the caller knows it.
        ///
        /// `None` for the box-level expansion path, which resolves imports from
        /// an already-parsed tree and has no line information. Emitting a
        /// placeholder span there would point the user at the wrong line, so
        /// the diagnostic simply carries no label in that case.
        site: Option<ImportSite>,
        /// Directories that were searched, in order, before giving up.
        searched: Vec<PathBuf>,
    },
    ImportCycle {
        path: PathBuf,
        /// Ordered closed cycle. The last edge points back to the first file.
        cycle: Vec<ImportCycleEdge>,
    },
}

impl fmt::Display for SourceReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(f, "I/O error while reading {}: {message}", path.display())
            }
            Self::UnresolvedImport { name, from, .. } => {
                write!(f, "cannot resolve import `{name}` from {}", from.display())
            }
            Self::ImportCycle { path, cycle } => {
                write!(f, "import cycle detected at {}", path.display())?;
                if !cycle.is_empty() {
                    write!(
                        f,
                        ": {}",
                        cycle
                            .iter()
                            .map(|edge| edge.from.display().to_string())
                            .chain(cycle.last().map(|edge| edge.to.display().to_string()))
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SourceReaderError {}

impl SourceReaderError {
    /// Returns the stable diagnostic code for this failure.
    #[must_use]
    pub fn code(&self) -> DiagnosticCode {
        match self {
            Self::Io { .. } => codes::SRC_IO_ERROR,
            Self::UnresolvedImport { .. } => codes::SRC_UNRESOLVED_IMPORT,
            Self::ImportCycle { .. } => codes::SRC_IMPORT_CYCLE,
        }
    }

    /// Converts this error into a structured diagnostic bundle.
    ///
    /// Before this existed, source-loading failures reached the CLI as
    /// `CompilerError::Import`, which carried no bundle at all, so every one of
    /// them was reported through the `code: null` fallback envelope with no
    /// span and no notes — the single most common newcomer failure (an
    /// unresolved `import`) answered with an unstructured string.
    ///
    /// The reference C++ compiler reports the same condition as
    /// `ERROR : unable to open file <name>`, i.e. without a location or the
    /// searched paths; this is deliberately more informative than parity.
    #[must_use]
    pub fn to_diagnostics(&self) -> DiagnosticBundle {
        let mut bundle = DiagnosticBundle::new();
        let diag = match self {
            Self::Io { path, message } => Diagnostic::new(
                Severity::Error,
                Stage::SourceReader,
                self.code(),
                format!("cannot read {}: {message}", path.display()),
            )
            .with_note(format!("path: {}", path.display()))
            .with_help("check that the path exists and is a readable file"),

            Self::UnresolvedImport {
                name,
                from,
                site,
                searched,
            } => {
                let mut diag = Diagnostic::new(
                    Severity::Error,
                    Stage::SourceReader,
                    self.code(),
                    format!("cannot resolve import `{name}`"),
                );
                if let Some(ImportSite { line, col }) = site {
                    let end_col = col + u32::try_from(name.chars().count()).unwrap_or(0);
                    diag = diag.with_label(
                        Label::new(
                            LabelStyle::Primary,
                            SourceSpan::new(from.clone(), *line, *col, *line, end_col),
                            "unresolved import",
                        )
                        .with_role(LabelRole::ImportSite),
                    );
                }
                diag = diag
                    .with_detail_code("unresolved-import")
                    .with_fact("import_name", name.clone())
                    .with_fact("imported_from", from.display().to_string())
                    .with_note(format!("import name: {name}"))
                    .with_note(format!("imported from: {}", from.display()));
                // The importing file's own directory is usually also on the
                // search path, so de-duplicate while keeping probe order.
                let mut unique: Vec<&PathBuf> = Vec::with_capacity(searched.len());
                for dir in searched {
                    if !unique.contains(&dir) {
                        unique.push(dir);
                    }
                }
                diag = diag.with_fact(
                    "searched_directories",
                    unique
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>(),
                );
                if unique.is_empty() {
                    diag = diag.with_note("no search directories were configured");
                } else {
                    diag = diag.with_note(format!(
                        "searched {} director{}:",
                        unique.len(),
                        if unique.len() == 1 { "y" } else { "ies" }
                    ));
                    for dir in unique {
                        diag = diag.with_note(format!("  {}", dir.display()));
                    }
                }
                diag.with_help("add the directory containing the file with `-I <dir>`")
                    .with_help("or correct the import name")
            }

            Self::ImportCycle { path, cycle } => {
                let mut diag = Diagnostic::new(
                    Severity::Error,
                    Stage::SourceReader,
                    self.code(),
                    format!("import cycle detected at {}", path.display()),
                )
                .with_detail_code("import-cycle")
                .with_fact(
                    "import_cycle",
                    cycle
                        .iter()
                        .map(|edge| edge.from.display().to_string())
                        .chain(cycle.last().map(|edge| edge.to.display().to_string()))
                        .collect::<Vec<_>>(),
                )
                .with_note("a file transitively imports itself")
                .with_help("break the cycle by removing one of the `import(...)` directives");
                for (index, edge) in cycle.iter().enumerate() {
                    if let Some(site) = edge.site {
                        let style = if index + 1 == cycle.len() {
                            LabelStyle::Primary
                        } else {
                            LabelStyle::Secondary
                        };
                        diag = diag.with_label(
                            Label::new(
                                style,
                                SourceSpan::new(
                                    edge.from.clone(),
                                    site.line,
                                    site.col,
                                    site.line,
                                    site.col.saturating_add(1),
                                ),
                                format!("imports `{}`", edge.to.display()),
                            )
                            .with_role(LabelRole::ImportSite),
                        );
                    }
                }
                diag
            }
        };
        bundle.push(diag);
        bundle
    }
}

/// File-backed source reader that expands `import("...");` directives recursively.
#[derive(Debug, Default)]
pub struct SourceReader {
    file_cache: HashMap<PathBuf, ExpandedSource>,
    search_paths: Vec<PathBuf>,
    virtual_sources: VirtualSourceMap,
    used_files: Vec<PathBuf>,
    visiting: HashSet<PathBuf>,
    visit_stack: Vec<PathBuf>,
    import_edges: Vec<ImportCycleEdge>,
    expanded_files: HashSet<PathBuf>,
}

impl SourceReader {
    /// Creates a source reader using the provided import search paths.
    #[must_use]
    pub fn new(search_paths: Vec<PathBuf>) -> Self {
        Self::with_virtual_sources(search_paths, VirtualSourceMap::default())
    }

    /// Creates a source reader using the provided import search paths and
    /// logical in-memory source bundle.
    #[must_use]
    pub fn with_virtual_sources(
        search_paths: Vec<PathBuf>,
        virtual_sources: VirtualSourceMap,
    ) -> Self {
        Self {
            file_cache: HashMap::new(),
            search_paths,
            virtual_sources,
            used_files: Vec::new(),
            visiting: HashSet::new(),
            visit_stack: Vec::new(),
            import_edges: Vec::new(),
            expanded_files: HashSet::new(),
        }
    }

    /// Returns search paths used by this reader.
    #[must_use]
    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    /// Returns files used during the last/ongoing recursive read.
    #[must_use]
    pub fn used_files(&self) -> &[PathBuf] {
        &self.used_files
    }

    /// Resolves one import name using current search paths.
    #[must_use]
    pub fn resolve_import(&self, name: &str) -> Option<PathBuf> {
        self.resolve_import_from(name, None)
    }

    /// Resolves one entry path without performing recursive import expansion.
    ///
    /// This helper exists for the structural C++ parity path where parsing now
    /// loads each file as its own unit and expands `importFile` nodes from the
    /// parsed definition tree instead of from rewritten source text.
    pub(crate) fn resolve_entry_source_path(
        &self,
        path: &Path,
    ) -> Result<PathBuf, SourceReaderError> {
        self.resolve_entry_path(path)
    }

    /// Resolves one import relative to the current importing file directory.
    ///
    /// The search order matches the existing C++-style `-I`-before-local-dir`
    /// behavior used by the reader's text-expansion path.
    pub(crate) fn resolve_import_source_path(
        &self,
        name: &str,
        local_dir: Option<&Path>,
    ) -> Option<PathBuf> {
        self.resolve_import_from(name, local_dir)
    }

    /// Reads one source unit without recursively expanding imports.
    ///
    /// This is the raw file/string loading counterpart used by the parser's
    /// structural import expansion path.
    pub(crate) fn read_source_unit(&self, path: &Path) -> Result<String, SourceReaderError> {
        self.read_source_text(path)
    }

    /// Returns whether a resolved path names an immutable virtual source.
    ///
    /// The structural import expander uses this only to classify snapshots in
    /// the shared diagnostic [`diagnostics::SourceMap`].
    pub(crate) fn is_virtual_source(&self, path: &Path) -> bool {
        self.virtual_sources.contains(path)
    }

    /// Reads one logical in-memory source and recursively expands imports.
    pub fn read_memory_with_origins(
        &mut self,
        source_name: &str,
        source: &str,
    ) -> Result<ExpandedSource, SourceReaderError> {
        let entry = normalize_logical_path(Path::new(source_name));
        self.expanded_files.clear();
        let prior = self.virtual_sources.clone();
        self.virtual_sources = self.virtual_sources.with_source(&entry, source);
        let out = self.read_file_impl(&entry);
        self.virtual_sources = prior;
        out
    }

    /// Reads one source file and recursively expands imports.
    pub fn read_file(&mut self, path: &Path) -> Result<String, SourceReaderError> {
        let canonical = self.resolve_entry_path(path)?;
        self.expanded_files.clear();
        self.read_file_impl(&canonical)
            .map(|expanded| expanded.text.into())
    }

    /// Reads one source file and recursively expands imports, preserving line origins.
    pub fn read_file_with_origins(
        &mut self,
        path: &Path,
    ) -> Result<ExpandedSource, SourceReaderError> {
        let canonical = self.resolve_entry_path(path)?;
        self.expanded_files.clear();
        self.read_file_impl(&canonical)
    }

    fn read_file_impl(&mut self, path: &Path) -> Result<ExpandedSource, SourceReaderError> {
        if let Some(cached) = self.file_cache.get(path) {
            return Ok(cached.clone());
        }

        if self.visiting.contains(path) {
            return Err(SourceReaderError::ImportCycle {
                path: path.to_path_buf(),
                cycle: import_cycle_from_stack(&self.visit_stack, &self.import_edges, path, None),
            });
        }

        self.visiting.insert(path.to_path_buf());
        self.visit_stack.push(path.to_path_buf());
        if !self.used_files.iter().any(|p| p == path) {
            self.used_files.push(path.to_path_buf());
        }

        let source = match self.read_source_text(path) {
            Ok(source) => source,
            Err(error) => {
                self.visiting.remove(path);
                self.visit_stack.pop();
                return Err(error);
            }
        };

        let mut expanded = String::new();
        let mut line_origins = Vec::new();
        let mut in_block_comment = false;
        for (line_index, line) in source.lines().enumerate() {
            // Track block-comment state so that import(...) lines inside /* ... */
            // blocks are not mistaken for real imports (C++ parity: the lexer sees
            // the whole file so comments are handled transparently there).
            let line_starts_in_comment = in_block_comment;
            in_block_comment = Self::advance_block_comment_state(in_block_comment, line);

            if !line_starts_in_comment && let Some(import_name) = parse_import_line(line) {
                let from_dir = path.parent();
                let col = line
                    .find(&import_name)
                    .map_or(1, |byte_idx| line[..byte_idx].chars().count() + 1);
                let site = ImportSite {
                    line: u32::try_from(line_index + 1).unwrap_or(u32::MAX),
                    col: u32::try_from(col).unwrap_or(1),
                };
                let Some(import_path) = self.resolve_import_from(&import_name, from_dir) else {
                    self.visiting.remove(path);
                    self.visit_stack.pop();
                    // Report where the directive is and where we looked, so the
                    // diagnostic is actionable instead of just "not found".
                    let mut searched: Vec<PathBuf> = Vec::new();
                    if let Some(dir) = from_dir {
                        searched.push(dir.to_path_buf());
                    }
                    searched.extend(self.search_paths.iter().cloned());
                    return Err(SourceReaderError::UnresolvedImport {
                        name: import_name.into_boxed_str(),
                        from: path.to_path_buf(),
                        site: Some(site),
                        searched,
                    });
                };
                if !self.expanded_files.contains(&import_path) {
                    let edge = ImportCycleEdge {
                        from: path.to_path_buf(),
                        to: import_path.clone(),
                        site: Some(site),
                    };
                    if self.visiting.contains(&import_path) {
                        let cycle = import_cycle_from_stack(
                            &self.visit_stack,
                            &self.import_edges,
                            &import_path,
                            Some(edge),
                        );
                        self.visiting.remove(path);
                        self.visit_stack.pop();
                        return Err(SourceReaderError::ImportCycle {
                            path: import_path,
                            cycle,
                        });
                    }
                    self.import_edges.push(edge);
                    let imported = self.read_file_impl(&import_path);
                    self.import_edges.pop();
                    let imported = match imported {
                        Ok(imported) => imported,
                        Err(error) => {
                            self.visiting.remove(path);
                            self.visit_stack.pop();
                            return Err(error);
                        }
                    };
                    expanded.push_str(&imported.text);
                    line_origins.extend(imported.line_origins);
                    if !expanded.ends_with('\n') {
                        expanded.push('\n');
                    }
                }
                continue; // import line consumed — not appended as source text
            }
            expanded.push_str(line);
            expanded.push('\n');
            line_origins.push(SourceLineOrigin {
                file: path.to_path_buf(),
                line: u32::try_from(line_index + 1).unwrap_or(u32::MAX),
            });
        }

        self.visiting.remove(path);
        self.visit_stack.pop();

        let expanded = ExpandedSource {
            text: expanded.into_boxed_str(),
            line_origins,
        };
        self.expanded_files.insert(path.to_path_buf());
        self.file_cache.insert(path.to_path_buf(), expanded.clone());
        Ok(expanded)
    }

    /// Tracks `/* ... */` block-comment state across one line.
    ///
    /// A `//` line comment outside a block comment ends the scan: without that,
    /// an ordinary comment mentioning a glob such as `// see tests/*.dsp` reads
    /// as opening a block comment, and every following line is treated as
    /// commented out until some `*/` appears. That silently hid `import(...)`
    /// directives from this expander (the box-level expansion path still
    /// resolved them, so the visible symptom was a diagnostic losing its source
    /// span rather than a miscompile).
    ///
    /// String literals are not tracked: `"/*"` inside a string still toggles
    /// the state. Pre-existing, and not reachable from a well-formed
    /// `import("...");` line, which is all this scanner gates.
    fn advance_block_comment_state(mut in_comment: bool, line: &str) -> bool {
        let bytes = line.as_bytes();
        let mut i = 0;

        while i + 1 < bytes.len() {
            match (bytes[i], bytes[i + 1]) {
                (b'/', b'/') if !in_comment => break,
                (b'/', b'*') if !in_comment => {
                    in_comment = true;
                    i += 2;
                    continue;
                }
                (b'*', b'/') if in_comment => {
                    in_comment = false;
                    i += 2;
                    continue;
                }
                _ => {
                    i += 1;
                }
            }
        }

        in_comment
    }

    fn resolve_import_from(&self, name: &str, local_dir: Option<&Path>) -> Option<PathBuf> {
        let raw = Path::new(name);
        if raw.is_absolute() {
            let normalized = normalize_logical_path(raw);
            if self.virtual_sources.contains(&normalized) {
                return Some(normalized);
            }
            return canonicalize_path(raw).ok();
        }

        // Mirror the C++ gImportDirList search order: -I paths (embedded at the head of
        // search_paths by the compiler) are checked before the local directory of the
        // currently-importing file.  In C++, `-I` entries are inserted at the front of
        // gImportDirList via `insert(begin())`, while the importing file's directory is
        // appended dynamically by `fopenSearch` only after the file is opened — i.e. it
        // ends up at the back, after the system paths already present in the list.
        // Reproducing that order: search_paths first, local_dir last (deduplicated).
        let mut candidates: Vec<PathBuf> = self
            .search_paths
            .iter()
            .map(|base| base.join(name))
            .collect();
        if let Some(base) = local_dir {
            let local_candidate = base.join(name);
            if !candidates.iter().any(|c| c == &local_candidate) {
                candidates.push(local_candidate);
            }
        }

        for candidate in candidates {
            let normalized = normalize_logical_path(&candidate);
            if self.virtual_sources.contains(&normalized) {
                return Some(normalized);
            }
            if candidate.exists() {
                return canonicalize_path(&candidate).ok();
            }
        }
        None
    }

    fn resolve_entry_path(&self, path: &Path) -> Result<PathBuf, SourceReaderError> {
        let normalized = normalize_logical_path(path);
        if self.virtual_sources.contains(&normalized) {
            Ok(normalized)
        } else {
            canonicalize_path(path)
        }
    }

    fn read_source_text(&self, path: &Path) -> Result<String, SourceReaderError> {
        if let Some(source) = self.virtual_sources.get(path) {
            return Ok(source.to_owned());
        }
        fs::read_to_string(path).map_err(|err| SourceReaderError::Io {
            path: path.to_path_buf(),
            message: err.to_string().into_boxed_str(),
        })
    }
}

fn canonicalize_path(path: &Path) -> Result<PathBuf, SourceReaderError> {
    path.canonicalize().map_err(|err| SourceReaderError::Io {
        path: path.to_path_buf(),
        message: err.to_string().into_boxed_str(),
    })
}

pub(crate) fn import_cycle_from_stack(
    active_paths: &[PathBuf],
    active_edges: &[ImportCycleEdge],
    repeated: &Path,
    closing_edge: Option<ImportCycleEdge>,
) -> Vec<ImportCycleEdge> {
    let start = active_paths
        .iter()
        .position(|path| path == repeated)
        .unwrap_or(0);
    let mut cycle = active_edges.get(start..).unwrap_or_default().to_vec();
    if let Some(edge) = closing_edge {
        cycle.push(edge);
    }
    cycle
}

fn normalize_logical_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        out
    }
}

fn parse_import_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let suffix = trimmed.strip_prefix("import")?.trim_start();
    let suffix = suffix.strip_prefix('(')?.trim_start();
    let suffix = suffix.strip_prefix('"')?;
    let end_quote = suffix.find('"')?;
    let import_name = &suffix[..end_quote];
    let rest = suffix[end_quote + 1..].trim();
    if !matches!(rest, ");")
        && !rest.starts_with(");//")
        && !rest.starts_with("); //")
        && !rest.starts_with(");/*")
        && !rest.starts_with("); /*")
    {
        return None;
    }
    Some(import_name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{SourceReader, VirtualSourceMap, parse_import_line};
    use std::path::Path;

    /// Search paths (-I) must be checked before the local directory of the importing
    /// file, mirroring the C++ gImportDirList ordering where `-I` entries are inserted
    /// at the front via `insert(begin())` while the importing file's dir is only appended
    /// dynamically at the back by `fopenSearch`.
    #[test]
    fn search_paths_take_precedence_over_local_dir_matching_cpp_import_order() {
        // Create two directories, each containing foo.lib with different content.
        // The override directory goes into search_paths (-I equivalent).
        // The local directory simulates the importing file's parent.
        // After the fix, search_paths must win.
        use std::env;
        let tmp = env::temp_dir();
        let override_dir = tmp.join("faust_rs_order_test_override");
        let local_dir = tmp.join("faust_rs_order_test_local");
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::create_dir_all(&local_dir).unwrap();
        std::fs::write(override_dir.join("foo.lib"), "// override").unwrap();
        std::fs::write(local_dir.join("foo.lib"), "// local").unwrap();

        let reader = SourceReader::new(vec![override_dir.clone()]);
        let resolved = reader
            .resolve_import_from("foo.lib", Some(&local_dir))
            .expect("should resolve");

        // The override (search_paths) must win over local_dir.
        let expected = override_dir.join("foo.lib").canonicalize().unwrap();
        assert_eq!(
            resolved, expected,
            "search_paths (-I) must take precedence over local_dir to match C++ gImportDirList order"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&override_dir);
        let _ = std::fs::remove_dir_all(&local_dir);
    }

    #[test]
    fn parses_import_line_variants() {
        assert_eq!(
            parse_import_line(r#"import("stdfaust.lib");"#).as_deref(),
            Some("stdfaust.lib")
        );
        assert_eq!(
            parse_import_line(r#"  import( "foo/bar.lib" ); "#).as_deref(),
            Some("foo/bar.lib")
        );
        assert_eq!(
            parse_import_line(r#"import("music.lib"); // transitive dependency"#).as_deref(),
            Some("music.lib")
        );
        assert!(parse_import_line(r#"process = _;"#).is_none());
    }

    #[test]
    fn transitively_reimported_file_is_expanded_only_once() {
        use std::env;

        let tmp = env::temp_dir().join("faust_rs_source_reader_transitive_reimport");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let math = tmp.join("math.lib");
        let music = tmp.join("music.lib");
        let main = tmp.join("main.dsp");

        std::fs::write(&math, "SR = 48000;\n").unwrap();
        std::fs::write(&music, "import(\"math.lib\");\nmel = SR;\n").unwrap();
        std::fs::write(
            &main,
            "import(\"math.lib\");\nimport(\"music.lib\");\nprocess = SR;\n",
        )
        .unwrap();

        let mut reader = SourceReader::new(vec![tmp.clone()]);
        let expanded = reader.read_file_with_origins(Path::new(&main)).unwrap();

        assert_eq!(expanded.text.matches("SR = 48000;").count(), 1);
        assert_eq!(
            expanded.text,
            "SR = 48000;\nmel = SR;\nprocess = SR;\n".into(),
            "transitively re-imported files should be expanded only once, matching C++ visited-set behavior"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn virtual_sources_expand_transitive_imports_without_filesystem_reads() {
        let bundle = VirtualSourceMap::new([
            (
                Path::new("stdfaust.lib").to_path_buf(),
                "import(\"maths.lib\");\nimport(\"osc.lib\");\n".to_owned(),
            ),
            (
                Path::new("maths.lib").to_path_buf(),
                "PI = 3.14;\n".to_owned(),
            ),
            (
                Path::new("osc.lib").to_path_buf(),
                "freq = 440;\n".to_owned(),
            ),
        ]);
        let mut reader = SourceReader::with_virtual_sources(Vec::new(), bundle);
        let expanded = reader
            .read_memory_with_origins("main.dsp", "import(\"stdfaust.lib\");\nprocess = freq;\n")
            .expect("virtual source expansion should succeed");

        assert!(expanded.text.contains("PI = 3.14;"));
        assert!(expanded.text.contains("freq = 440;"));
        assert!(expanded.text.contains("process = freq;"));
        assert!(
            reader
                .used_files()
                .iter()
                .any(|path| path == Path::new("stdfaust.lib"))
        );
        assert!(
            reader
                .used_files()
                .iter()
                .any(|path| path == Path::new("osc.lib"))
        );
    }
}
