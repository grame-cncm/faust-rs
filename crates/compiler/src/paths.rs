//! Import search path construction and merge helpers.
//!
//! Mirrors the C++ `global::initDocumentNames()` / `initDirectories()` model:
//! - `default_import_search_paths` — builds the ordered path list for a
//!   file-backed session (current dir, `FAUST_LIB_PATH`, exe-relative, system);
//! - `merge_import_search_paths` / `build_import_search_paths` — utilities for
//!   combining caller-supplied paths with the defaults;
//! - `ensure_parse_success` — converts a parse result into a `CompilerError`
//!   with consistent source attribution.

use super::*;

// ─── Helpers: path resolution ─────────────────────────────────────────────────

/// Installed Faust directory information exposed by C++ Faust through
/// `-libdir`, `-includedir`, `-archdir`, `-dspdir`, and `-pathslist`.
///
/// # Source provenance (C++)
/// - `global::initDirectories()`
/// - `global::printLibDir()` / `printIncludeDir()` / `printArchDir()` /
///   `printDspDir()` / `printPaths()` in `compiler/global.cpp`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaustInstallPaths {
    /// Root directory inferred from the running executable.
    pub root_dir: PathBuf,
    /// Directory containing libfaust libraries.
    pub lib_dir: PathBuf,
    /// Directory containing Faust C++ headers.
    pub include_dir: PathBuf,
    /// Directory containing Faust architecture files.
    pub arch_dir: PathBuf,
    /// Directory containing Faust DSP libraries.
    pub dsp_dir: PathBuf,
    /// Default DSP import search paths.
    pub import_dirs: Vec<PathBuf>,
    /// Default architecture search paths.
    pub architecture_dirs: Vec<PathBuf>,
}

impl FaustInstallPaths {
    /// Builds the installed-path view from explicit inputs.
    ///
    /// The C++ compiler infers `gFaustRootDir` from `argv[0]` by taking the
    /// parent of the executable directory. The Rust CLI mirrors that rule when
    /// `current_exe` is available and falls back to `/usr/local`.
    #[must_use]
    pub fn from_parts(
        current_exe: Option<PathBuf>,
        faust_lib_path: Option<OsString>,
        faust_arch_path: Option<OsString>,
    ) -> Self {
        fn push_unique(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
            if !paths.iter().any(|existing| existing == &candidate) {
                paths.push(candidate);
            }
        }

        let root_dir = current_exe
            .as_deref()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/usr/local"));

        let lib_dir = root_dir.join("lib");
        let include_dir = root_dir.join("include");
        let dsp_dir = root_dir.join("share").join("faust");
        let arch_dir = dsp_dir.clone();

        let mut import_dirs = Vec::new();
        if let Some(env_path) = faust_lib_path {
            for path in std::env::split_paths(&env_path) {
                push_unique(&mut import_dirs, path);
            }
        }
        push_unique(&mut import_dirs, dsp_dir.clone());
        push_unique(&mut import_dirs, PathBuf::from("/usr/local/share/faust"));
        push_unique(&mut import_dirs, PathBuf::from("/usr/share/faust"));

        let mut architecture_dirs = Vec::new();
        if let Some(env_path) = faust_arch_path {
            push_unique(&mut architecture_dirs, PathBuf::from(env_path));
        }
        push_unique(&mut architecture_dirs, arch_dir.clone());
        push_unique(&mut architecture_dirs, include_dir.clone());
        push_unique(
            &mut architecture_dirs,
            PathBuf::from("/usr/local/share/faust"),
        );
        push_unique(&mut architecture_dirs, PathBuf::from("/usr/share/faust"));
        push_unique(&mut architecture_dirs, PathBuf::from("/usr/local/include"));
        push_unique(&mut architecture_dirs, PathBuf::from("/usr/include"));

        Self {
            root_dir,
            lib_dir,
            include_dir,
            arch_dir,
            dsp_dir,
            import_dirs,
            architecture_dirs,
        }
    }

    /// Builds the installed-path view from the process environment.
    #[must_use]
    pub fn from_environment() -> Self {
        Self::from_parts(
            std::env::current_exe().ok(),
            std::env::var_os("FAUST_LIB_PATH"),
            std::env::var_os("FAUST_ARCH_PATH"),
        )
    }

    /// Renders one path with C++ Faust's trailing newline convention.
    #[must_use]
    pub fn render_path(path: &Path) -> String {
        format!("{}\n", path.display())
    }

    /// Renders `-libdir` output.
    #[must_use]
    pub fn render_lib_dir(&self) -> String {
        Self::render_path(&self.lib_dir)
    }

    /// Renders `-includedir` output.
    #[must_use]
    pub fn render_include_dir(&self) -> String {
        Self::render_path(&self.include_dir)
    }

    /// Renders `-archdir` output.
    #[must_use]
    pub fn render_arch_dir(&self) -> String {
        Self::render_path(&self.arch_dir)
    }

    /// Renders `-dspdir` output.
    #[must_use]
    pub fn render_dsp_dir(&self) -> String {
        Self::render_path(&self.dsp_dir)
    }

    /// Renders `-pathslist` output.
    #[must_use]
    pub fn render_paths_list(&self) -> String {
        let mut out = String::new();
        out.push_str("FAUST dsp library paths:\n");
        for path in &self.import_dirs {
            out.push_str(&format!("{}\n", path.display()));
        }
        out.push_str("\nFAUST architectures paths:\n");
        for path in &self.architecture_dirs {
            out.push_str(&format!("{}\n", path.display()));
        }
        out.push('\n');
        out
    }
}

/// Resolves the default built-in import search paths for one file-backed
/// compilation session.
///
/// # Source provenance (C++)
/// - `global::initDocumentNames()` / `global::initDirectories()` in
///   `compiler/global.cpp`
///
/// # Effective order
/// 1. current file parent directory (or `"."` for a bare filename)
/// 2. `FAUST_LIB_PATH` when present
/// 3. executable-relative `../share/faust`
/// 4. `/usr/local/share/faust`
/// 5. `/usr/share/faust`
///
/// This mirrors the C++ hardcoded library-search model as closely as possible
/// in a standalone Rust binary.
#[must_use]
/// Returns the default Faust import search paths for `path`.
pub fn default_import_search_paths(path: &Path) -> Vec<PathBuf> {
    build_import_search_paths(
        path,
        &[],
        std::env::var_os("FAUST_LIB_PATH"),
        std::env::current_exe().ok(),
    )
}

/// Builds the import search path list for a given source file, merging user-supplied
/// extra paths with the built-in defaults discovered from the environment.
///
/// This is a convenience wrapper over [`build_import_search_paths`] that reads
/// `FAUST_LIB_PATH` and the current executable location automatically.
pub(crate) fn merge_import_search_paths(path: &Path, extra_paths: &[PathBuf]) -> Vec<PathBuf> {
    build_import_search_paths(
        path,
        extra_paths,
        std::env::var_os("FAUST_LIB_PATH"),
        std::env::current_exe().ok(),
    )
}

/// Core implementation of the import search path algorithm.
///
/// Produces an ordered, deduplicated list following the same priority rules as
/// the C++ Faust compiler:
///
/// 1. User-supplied `extra_paths` (highest priority).
/// 2. Directory containing the source file.
/// 3. Paths from the `FAUST_LIB_PATH` environment variable, separated with
///    the host platform's native path separator.
/// 4. Standard library locations relative to the running executable.
///
/// Parameters are explicit so the function is pure and fully testable without
/// touching the environment.
pub(crate) fn build_import_search_paths(
    path: &Path,
    extra_paths: &[PathBuf],
    faust_lib_path: Option<OsString>,
    current_exe: Option<PathBuf>,
) -> Vec<PathBuf> {
    /// Appends `candidate` only if it is not already present in `paths`.
    fn push_unique(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
        if !paths.iter().any(|existing| existing == &candidate) {
            paths.push(candidate);
        }
    }

    let mut ordered = Vec::with_capacity(extra_paths.len() + 5);
    for path in extra_paths {
        push_unique(&mut ordered, path.clone());
    }

    push_unique(
        &mut ordered,
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    );

    if let Some(env_path) = faust_lib_path {
        for path in std::env::split_paths(&env_path) {
            push_unique(&mut ordered, path);
        }
    }

    if let Some(share_root) = current_exe
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|root| root.join("share").join("faust"))
    {
        push_unique(&mut ordered, share_root);
    }

    push_unique(&mut ordered, PathBuf::from("/usr/local/share/faust"));
    push_unique(&mut ordered, PathBuf::from("/usr/share/faust"));
    ordered
}

// ─── Helpers: parse validation ────────────────────────────────────────────────

/// Converts raw parser output into the facade-level success/error contract.
///
/// The parser may return a root node even when recoveries or hard errors were
/// recorded. The compiler facade treats any non-zero parse error or recovery
/// count as a stage failure, matching the stricter "ready for later phases"
/// contract expected by `eval` and `propagate`.
pub(crate) fn ensure_parse_success(
    source: &str,
    output: ParseOutput,
) -> Result<ParseOutput, CompilerError> {
    let parse_errors = usize::try_from(output.state.ctx.parse_error_count()).unwrap_or(usize::MAX);
    let recoveries = output.state.ctx.recovery_count();
    let has_root = output.root.is_some();
    if has_root && parse_errors == 0 && recoveries == 0 {
        Ok(output)
    } else {
        Err(CompilerError::Parse {
            source: source.into(),
            parse_errors,
            recoveries,
            diagnostics: output.diagnostics,
        })
    }
}
