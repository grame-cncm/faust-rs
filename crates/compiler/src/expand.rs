//! Self-contained DSP expansion — the `-e` / `--export-dsp` document.
//!
//! # Source provenance (C++)
//! - `compiler/libcode.cpp`, `expandDSPInternalAux(...)` and the `gExportDSP`
//!   branch at `libcode.cpp:1378`
//! - `compiler/global.cpp`, `global::printDeclareHeader(...)`
//! - `compiler/generator/dsp_aux.cpp`, `reorganizeCompilationOptions(...)`
//!
//! # What expansion produces
//! One `.dsp` file that compiles to the same DSP as its input with no library
//! search path at all: every `import`, every library definition and every user
//! abstraction has been evaluated away, leaving a flat list of `ID_<n>`
//! definitions and a `process` binding.
//!
//! The document has four parts, in this order:
//!
//! 1. `declare version "...";`
//! 2. `declare compile_options "...";` — the normalized option string
//! 3. `declare library_path<i> "...";` — one per source file except the entry
//! 4. the `declare` metadata header, then the serialized box program
//!
//! # Mapping status
//! `adapted`. The text is the parity target; three values legitimately differ
//! from a C++ expansion of the same program and are listed in
//! `porting/faust-rs-vs-faust-cpp-differences-en.md` under `DIFF-BEH-006`:
//! the compiler version, the option spelling, and installation-dependent
//! library paths.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::*;
use boxes::{FloatSize, box_pp_shared};
use parser::CompilationMetadataKey;

/// Prefix C++ writes before the normalized option string.
///
/// Mirrors `COMPILATION_OPTIONS` (`compiler/dsp_factory.hh:38`), including the
/// trailing space, because [`Compiler::expand_dsp`] tests incoming sources
/// against it.
const COMPILATION_OPTIONS_PREFIX: &str = "declare compile_options ";

impl Compiler {
    /// Expands one Faust source string into a self-contained DSP document.
    ///
    /// `argv` carries the compilation options recorded in the document's
    /// `compile_options` declaration; it does not select them — the options
    /// this compiler was built with are already in effect.
    ///
    /// # Errors
    /// Returns [`CompilerError`] when the program fails to parse or evaluate,
    /// when it has no output signal, or when its evaluated box contains a
    /// shape with no Faust source syntax.
    pub fn expand_source_to_dsp(
        &self,
        source_name: &str,
        source: &str,
        search_paths: &[PathBuf],
        argv: &[String],
    ) -> Result<String, CompilerError> {
        let boxes = self.compile_source_to_boxes_with_import_context(
            source_name,
            source,
            search_paths,
            &VirtualSourceMap::default(),
        )?;
        self.render_expansion(&boxes, argv)
    }

    /// Expands one Faust source file into a self-contained DSP document.
    ///
    /// # Errors
    /// Same conditions as [`Self::expand_source_to_dsp`].
    pub fn expand_file_to_dsp(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        argv: &[String],
    ) -> Result<String, CompilerError> {
        let boxes = self.compile_file_to_boxes(path, search_paths)?;
        self.render_expansion(&boxes, argv)
    }

    /// Assembles the expansion document from one evaluated program.
    fn render_expansion(
        &self,
        boxes: &BoxCompileOutput,
        argv: &[String],
    ) -> Result<String, CompilerError> {
        // C++ rejects an output-less program before the `-e` branch
        // (`libcode.cpp:1370`), so expansion never produces a document for a
        // program a normal compilation would refuse.
        if boxes.process_arity.outputs == 0 {
            return Err(CompilerError::expand_failed(
                boxes.source_name(),
                "the Faust program has no output signal",
            ));
        }

        let mut out = String::new();
        let _ = writeln!(out, "declare version \"{}\";", Self::version());
        let _ = writeln!(
            out,
            "{COMPILATION_OPTIONS_PREFIX}\"{}\";",
            reorganize_compilation_options(argv)
        );
        for (index, path) in library_paths(boxes).iter().enumerate() {
            let _ = writeln!(out, "declare library_path{index} \"{path}\";");
        }
        out.push_str(&declare_header(
            boxes.source_name(),
            &boxes.compilation_metadata,
        ));

        let program = box_pp_shared(
            &boxes.parse.state.arena,
            boxes.process_box,
            float_size(self.real_type),
        )
        .map_err(|error| CompilerError::expand_failed(boxes.source_name(), error))?;
        out.push_str(&program.render(boxes.entrypoint_name.as_ref()));
        Ok(out)
    }
}

/// Maps the compiler's real type to the printer's literal-suffix policy.
fn float_size(real_type: RealType) -> FloatSize {
    match real_type {
        RealType::Float32 => FloatSize::Single,
        RealType::Float64 => FloatSize::Double,
    }
}

/// Returns the library files that contributed to one program, entry excluded.
///
/// Mirrors C++ `listSrcFiles()` minus its first element
/// (`libcode.cpp:1199-1202`): the entry file is not a library path, and
/// re-declaring it would make the expansion look like it imports itself.
/// Evaluator-loaded files are appended because `component(...)` and
/// `library(...)` resolve after parsing in this port, so they are absent from
/// the parser's own list.
fn library_paths(boxes: &BoxCompileOutput) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in boxes
        .parse
        .used_files
        .iter()
        .skip(1)
        .chain(boxes.loaded_files.iter())
    {
        let rendered = path.display().to_string();
        if !out.contains(&rendered) {
            out.push(rendered);
        }
    }
    out
}

/// Renders the top-level `declare` header for one compilation.
///
/// Mirrors `global::printDeclareHeader` (`compiler/global.cpp:2154`):
///
/// - `.`, `:` and `/` in a key become `_`, so the scoped
///   `basics.lib/name` prints as `basics_lib_name` — Faust identifiers cannot
///   contain those characters, and an expansion has to re-parse;
/// - the first `author` value keeps its key and every later one is re-emitted
///   as `contributor`, because `declare` keys are not repeatable in a way that
///   preserves order;
/// - `filename` and `name` are synthesized when the program did not declare
///   them, matching what C++ `initDocumentNames()` puts in the metadata set
///   before the header is printed.
fn declare_header(source_name: &str, metadata: &parser::CompilationMetadataSnapshot) -> String {
    // Library-scoped keys are rendered relative to the master DSP directory,
    // falling back to the file name. The parser records canonical absolute
    // paths, and C++ keys the same entries by library file name — so without
    // this the header would read `_usr_local_share_faust_maths_lib_name`
    // instead of `maths_lib_name`, and would differ per machine.
    let master_parent = Path::new(source_name)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let master_parent = master_parent
        .canonicalize()
        .unwrap_or_else(|_| master_parent.to_path_buf());

    let mut entries: Vec<(String, String)> = Vec::new();
    let mut has_filename = false;
    let mut has_name = false;

    for (key, values) in metadata.entries() {
        let base_key = match key {
            CompilationMetadataKey::Global { key } => {
                normalize_flat_metadata_key(&master_parent, key)
            }
            CompilationMetadataKey::Scoped { source_file, key } => format!(
                "{}/{}",
                metadata_source_path(&master_parent, Path::new(source_file.as_ref())),
                key.as_ref()
            ),
        };
        has_filename |= base_key == "filename";
        has_name |= base_key == "name";

        let mangled = mangle_declare_key(&base_key);
        let mut values = values.iter();
        let Some(first) = values.next() else { continue };
        entries.push((mangled.clone(), first.as_ref().to_owned()));
        for value in values {
            let repeated_key = if base_key == "author" {
                "contributor".to_owned()
            } else {
                mangled.clone()
            };
            entries.push((repeated_key, value.as_ref().to_owned()));
        }
    }

    if !has_filename {
        entries.push(("filename".to_owned(), source_name_to_filename(source_name)));
    }
    if !has_name {
        entries.push(("name".to_owned(), source_name_to_class(source_name)));
    }
    // C++ iterates an ordered `set<Tree>` keyed by the interned metadata name,
    // and the synthesized entries are inserted before printing rather than
    // appended, so the whole header is sorted by key.
    entries.sort();

    let mut out = String::new();
    for (key, value) in entries {
        let _ = writeln!(out, "declare {key} \"{}\";", escape_declare_value(&value));
    }
    out
}

/// Replaces the characters a `declare` key may not contain.
///
/// Mirrors C++ `replaceCharList(key, {'.', ':', '/'}, '_')`.
fn mangle_declare_key(key: &str) -> String {
    key.replace(['.', ':', '/'], "_")
}

/// Escapes a metadata value for a double-quoted Faust string.
fn escape_declare_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Normalizes an argument vector into the `compile_options` string.
///
/// Port of `reorganizeCompilationOptionsAux`
/// (`compiler/generator/dsp_aux.cpp:97`). The canonical order exists so that
/// the same program compiled with the same options yields the same string, and
/// therefore the same cache key, whatever order the caller passed them in.
/// Options the normalizer does not know are appended verbatim in their
/// original order — which is why a command-line expansion carries its own
/// input and output file names into the string.
#[must_use]
pub fn reorganize_compilation_options(argv: &[String]) -> String {
    let mut options: Vec<String> = argv.to_vec();
    let mut normalized: Vec<String> = Vec::new();

    // Step 1 — precision.
    add_key(&mut options, &mut normalized, "-double", Some("-single"));

    // Step 2 — options that imply vectorization.
    let mut vectorize = add_key(&mut options, &mut normalized, "-sch", None);
    if add_key(&mut options, &mut normalized, "-omp", None) {
        vectorize = true;
        add_key(&mut options, &mut normalized, "-pl", None);
    }
    if vectorize {
        normalized.push("-vec".to_owned());
    }

    // Step 3 — options whose meaning depends on the vector/scalar choice.
    if vectorize || add_key(&mut options, &mut normalized, "-vec", None) {
        add_key(&mut options, &mut normalized, "-dfs", None);
        add_key(&mut options, &mut normalized, "-vls", None);
        add_key(&mut options, &mut normalized, "-fun", None);
        add_key(&mut options, &mut normalized, "-g", None);
        add_key_value(&mut options, &mut normalized, "-vs", "32");
        add_key_value(&mut options, &mut normalized, "-lv", "0");
    } else {
        add_key(&mut options, &mut normalized, "-scal", Some("-scal"));
        add_key(&mut options, &mut normalized, "-inpl", None);
    }

    add_key_value(&mut options, &mut normalized, "-mcd", "16");
    add_key_value(&mut options, &mut normalized, "-cn", "");
    add_key_value(&mut options, &mut normalized, "-ftz", "0");

    // Everything else keeps its original relative order. The leading program
    // name is dropped so a libFaust-style argv and a CLI argv normalize alike.
    for option in options {
        if option != "faust" {
            normalized.push(option);
        }
    }

    normalized.join(" ")
}

/// Moves `key` from `options` to `normalized`, or pushes `default_key`.
///
/// Returns whether `key` was present. Mirrors C++ `addKeyIfExisting`.
fn add_key(
    options: &mut Vec<String>,
    normalized: &mut Vec<String>,
    key: &str,
    default_key: Option<&str>,
) -> bool {
    if let Some(position) = options.iter().position(|option| option == key) {
        normalized.push(options.remove(position));
        return true;
    }
    if let Some(default_key) = default_key {
        normalized.push(default_key.to_owned());
    }
    false
}

/// Moves `key` and its value, substituting `default_value` when absent.
///
/// A following argument counts as the value only when it does not itself start
/// with `-`, matching C++ `addKeyValueIfExisting`.
fn add_key_value(
    options: &mut Vec<String>,
    normalized: &mut Vec<String>,
    key: &str,
    default_value: &str,
) {
    let Some(position) = options.iter().position(|option| option == key) else {
        return;
    };
    normalized.push(options.remove(position));
    let takes_value = options
        .get(position)
        .is_some_and(|value| !value.starts_with('-'));
    if takes_value {
        normalized.push(options.remove(position));
    } else {
        normalized.push(default_value.to_owned());
    }
}

/// Returns the quoted option string embedded in an already-expanded source.
///
/// Mirrors C++ `extractCompilationOptions` (`dsp_aux.cpp:183`): it reads the
/// text between the first two quotes following the `compile_options` key.
#[must_use]
pub fn extract_compilation_options(source: &str) -> Option<&str> {
    let start = source.find("compile_options")?;
    let rest = &source[start..];
    let open = rest.find('"')?;
    let after_open = &rest[open + 1..];
    let close = after_open.find('"')?;
    Some(&after_open[..close])
}

/// Returns whether `source` already carries a `compile_options` declaration
/// as its very first statement.
///
/// # C++ quirk preserved
/// `expandDSPFromString` short-circuits on this test
/// (`dsp_aux.cpp:240`), but `expandDSPInternalAux` writes `declare version`
/// *first* (`libcode.cpp:1195`), so the test never matches a real expansion —
/// despite the comment at `libcode.cpp:1194` asserting the options line "has
/// to be located first in the string". The behavior is reproduced rather than
/// corrected: only hand-written option-prefixed sources take the short path,
/// in both compilers.
#[must_use]
pub fn starts_with_compilation_options(source: &str) -> bool {
    source.starts_with(COMPILATION_OPTIONS_PREFIX)
}
