//! CLI-like compile arguments shared by FFI entry points.

use std::path::PathBuf;

/// The options of a C API `argv` that concern the FFI host rather than the
/// compiled program.
///
/// Supported options: `-I <path>`, `-cn <name>`, the four mode-zero
/// memory-manager aliases, and the non-fatal diagnostic switch `--warn`. The
/// options of the program itself (`-pn`, `-double`, `-vec`, `-ss`, `-mcd`,
/// ...) are the compiler's: `compiler::CompileOptionArgs::from_argv` reads
/// them from the same `argv`, which this crate, dependency-light, cannot see.
/// Unknown options are ignored so backend FFI crates can accept broader argv
/// vectors while incrementally extending support.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FfiCompileArgs {
    /// Enable custom memory-manager mode zero.
    ///
    /// This dependency-light parsing crate stores the semantic bit; consumers
    /// map it immediately to `codegen::memory_layout::MemoryManagerMode`.
    pub memory_manager0: bool,
    /// Extra import search paths collected from `-I`, in search order: the last
    /// `-I` first, as the C++ compiler inserts each at the front of
    /// `gImportDirList` (`global::processCmdline`).
    pub search_paths: Vec<PathBuf>,
    /// Optional class/module name override from `-cn`.
    pub module_name: Option<String>,
    /// Collect non-blocking semantic warnings on successful compilations.
    ///
    /// This mirrors the compiler facade's warning policy: warnings are
    /// retained for a diagnostics query but never turn success into failure.
    pub warnings: bool,
}

/// Parses the FFI host options (`-I`, `-cn`, `-mem0` and its aliases,
/// `--warn`) from an argv vector, and refuses `-mem0` with `-vec` or `-it`.
pub fn parse_ffi_compile_args(argv: &[String]) -> Result<FfiCompileArgs, String> {
    let mut parsed = FfiCompileArgs::default();
    let mut index = 0usize;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "-I" {
            let Some(value) = argv.get(index + 1) else {
                return Err("missing path after -I".to_owned());
            };
            parsed.search_paths.push(PathBuf::from(value));
            index += 2;
            continue;
        }
        if arg == "-cn" {
            let Some(value) = argv.get(index + 1) else {
                return Err("missing class name after -cn".to_owned());
            };
            parsed.module_name = Some(value.clone());
            index += 2;
            continue;
        }
        if matches!(
            arg.as_str(),
            "-mem" | "-mem0" | "--memory-manager" | "--memory-manager0"
        ) {
            parsed.memory_manager0 = true;
            index += 1;
            continue;
        }
        if matches!(
            arg.as_str(),
            "-mem1"
                | "-mem2"
                | "-mem3"
                | "--memory-manager1"
                | "--memory-manager2"
                | "--memory-manager3"
        ) {
            return Err(format!(
                "unsupported memory-manager mode `{arg}`; only -mem0 is implemented"
            ));
        }
        if arg == "--warn" {
            parsed.warnings = true;
        }
        index += 1;
    }
    let given = |names: &[&str]| argv.iter().any(|arg| names.contains(&arg.as_str()));
    if parsed.memory_manager0 && given(&["-vec", "--vec"]) {
        return Err("-mem0 is currently supported only in scalar mode; remove -vec".to_owned());
    }
    if parsed.memory_manager0 && given(&["-it"]) {
        return Err("-mem0 cannot be combined with -it".to_owned());
    }
    parsed.search_paths.reverse();
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::parse_ffi_compile_args;

    #[test]
    fn accepts_i_and_cn_and_skips_the_compiler_s_options() {
        let argv = [
            "-I", "lib1", "-I", "lib2", "-cn", "MyDSP", "-vec", "-vs", "64", "-pn", "voice",
        ]
        .map(str::to_owned);
        let parsed = parse_ffi_compile_args(&argv).unwrap();
        // search order: the last -I first, as in C++
        assert_eq!(
            parsed.search_paths,
            [PathBuf::from("lib2"), PathBuf::from("lib1")]
        );
        assert_eq!(parsed.module_name.as_deref(), Some("MyDSP"));
        assert!(!parsed.memory_manager0 && !parsed.warnings);
    }

    #[test]
    fn accepts_non_fatal_warning_collection() {
        let parsed = parse_ffi_compile_args(&["--warn".to_owned()]).unwrap();
        assert!(parsed.warnings);
    }

    #[test]
    fn all_mem0_aliases_select_one_mode() {
        for spelling in ["-mem", "-mem0", "--memory-manager", "--memory-manager0"] {
            let parsed = parse_ffi_compile_args(&[spelling.to_owned()]).unwrap();
            assert!(parsed.memory_manager0, "{spelling}");
        }
    }

    #[test]
    fn rejects_unported_memory_modes_and_incompatible_mem0_options() {
        for spelling in ["-mem1", "-mem2", "-mem3", "--memory-manager3"] {
            let error = parse_ffi_compile_args(&[spelling.to_owned()]).unwrap_err();
            assert!(error.contains("only -mem0 is implemented"), "{error}");
        }
        let error = parse_ffi_compile_args(&["-mem0".to_owned(), "-vec".to_owned()]).unwrap_err();
        assert!(error.contains("scalar mode"), "{error}");
        let error = parse_ffi_compile_args(&["-it".to_owned(), "-mem".to_owned()]).unwrap_err();
        assert!(error.contains("cannot be combined with -it"), "{error}");
    }
}
