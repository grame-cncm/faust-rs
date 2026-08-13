//! CLI-like compile arguments shared by FFI entry points.

use std::path::PathBuf;

/// Minimal shared subset of Faust CLI-like options accepted by Rust FFI crates.
///
/// Supported options: `-I <path>`, `-cn <name>`, `-double`, the vector-mode
/// trio `-vec` / `-vs <n>` / `-lv <n>`, the scheduling-strategy option
/// `-ss <n>`, the four mode-zero memory-manager aliases, and the non-fatal
/// diagnostic switch `--warn`.
/// Unknown options are ignored so backend FFI crates can accept broader argv
/// vectors while incrementally extending support.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FfiCompileArgs {
    /// Enable custom memory-manager mode zero.
    ///
    /// This dependency-light parsing crate stores the semantic bit; consumers
    /// map it immediately to `codegen::memory_layout::MemoryManagerMode`.
    pub memory_manager0: bool,
    /// Extra import search paths collected from `-I`.
    pub search_paths: Vec<PathBuf>,
    /// Optional class/module name override from `-cn`.
    pub module_name: Option<String>,
    /// Use double-precision (64-bit) floating-point for internal DSP arithmetic.
    ///
    /// Set by the `-double` flag in the `argv` vector passed to FFI factory
    /// constructors. Mirrors the reference Faust compiler's `-double` option.
    pub double: bool,
    /// Vector mode requested (`-vec`). When false, `vec_size`/`loop_variant` are
    /// ignored (scalar codegen).
    pub vec_mode: bool,
    /// Vector chunk size (`-vs <n>`; Faust default 32). Only meaningful when
    /// [`Self::vec_mode`] is set.
    pub vec_size: u32,
    /// Vector loop variant (`-lv <n>`; 0 = fastest/default, 1 = simple). Only
    /// meaningful when [`Self::vec_mode`] is set.
    pub loop_variant: u8,
    /// Raw signal/loop scheduling-strategy value (`-ss <n>`; Faust default
    /// `0`, depth-first).
    ///
    /// Kept as the raw non-negative integer here (rather than a decoded enum)
    /// because `ffi-common` is a dependency-light leaf crate and the
    /// `SchedulingStrategy` enum lives in `transform`. Callers that depend on
    /// `compiler`/`transform` decode it with
    /// `transform::schedule::SchedulingStrategy::decode` (re-exported as
    /// `compiler::SchedulingStrategy`): `0 -> DepthFirst`,
    /// `1 -> BreadthFirst`, `2 -> Special`, `n >= 3 -> ReverseBreadthFirst`.
    pub scheduling_strategy: u32,
    /// Collect non-blocking semantic warnings on successful compilations.
    ///
    /// This mirrors the compiler facade's warning policy: warnings are
    /// retained for a diagnostics query but never turn success into failure.
    pub warnings: bool,
    /// `runtime` when `--table-init runtime` was given, selecting a generated
    /// sub-module that fills a `rdtable`/`rwtable` at initialization instead of
    /// folding its contents at compile time.
    ///
    /// Kept as the raw string for the same reason as
    /// [`Self::scheduling_strategy`]: `TableInitMode` lives in `transform`, and
    /// `ffi-common` is a dependency-light leaf crate.
    pub table_init: Option<String>,
    /// Explicit sample rate used to fold `ma.SR` under `--table-init const`.
    pub table_init_sample_rate: Option<i32>,
}

/// Parses the shared FFI option subset (`-I`, `-cn`, `-double`,
/// `-vec`/`-vs`/`-lv`, `-ss`, `--table-init`, `--table-init-sample-rate`, `--warn`) from an argv vector. `vec_size` defaults to 32
/// when `-vec` is given without `-vs`, matching the Faust CLI.
/// `scheduling_strategy` defaults to `0` (depth-first) when `-ss` is absent,
/// mirroring the CLI's `--scheduling-strategy` default.
pub fn parse_ffi_compile_args(argv: &[String]) -> Result<FfiCompileArgs, String> {
    let mut parsed = FfiCompileArgs {
        vec_size: 32,
        ..FfiCompileArgs::default()
    };
    let mut index = 0usize;
    let mut in_place = false;
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
        if arg == "-double" {
            parsed.double = true;
            index += 1;
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
        if arg == "-it" {
            in_place = true;
            index += 1;
            continue;
        }
        if arg == "-vec" {
            parsed.vec_mode = true;
            index += 1;
            continue;
        }
        if arg == "-vs" {
            let Some(value) = argv.get(index + 1) else {
                return Err("missing value after -vs".to_owned());
            };
            parsed.vec_size = value
                .parse()
                .map_err(|error| format!("bad -vs value: {error}"))?;
            index += 2;
            continue;
        }
        if arg == "-lv" {
            let Some(value) = argv.get(index + 1) else {
                return Err("missing value after -lv".to_owned());
            };
            parsed.loop_variant = value
                .parse()
                .map_err(|error| format!("bad -lv value: {error}"))?;
            index += 2;
            continue;
        }
        if arg == "-ss" {
            let Some(value) = argv.get(index + 1) else {
                return Err("missing value after -ss".to_owned());
            };
            parsed.scheduling_strategy = value
                .parse()
                .map_err(|error| format!("bad -ss value: {error}"))?;
            index += 2;
            continue;
        }
        if arg == "--table-init" || arg == "-table-init" {
            let Some(value) = argv.get(index + 1) else {
                return Err("missing value after --table-init".to_owned());
            };
            parsed.table_init = Some(value.clone());
            index += 2;
            continue;
        }
        if arg == "--table-init-sample-rate" {
            let Some(value) = argv.get(index + 1) else {
                return Err("missing value after --table-init-sample-rate".to_owned());
            };
            parsed.table_init_sample_rate = Some(
                value
                    .parse()
                    .map_err(|error| format!("bad --table-init-sample-rate value: {error}"))?,
            );
            index += 2;
            continue;
        }
        if arg == "--warn" {
            parsed.warnings = true;
            index += 1;
            continue;
        }
        index += 1;
    }
    if parsed.memory_manager0 && parsed.vec_mode {
        return Err("-mem0 is currently supported only in scalar mode; remove -vec".to_owned());
    }
    if parsed.memory_manager0 && in_place {
        return Err("-mem0 cannot be combined with -it".to_owned());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::parse_ffi_compile_args;

    #[test]
    fn accepts_i_cn_and_vec_options() {
        let argv = vec![
            "-I".to_owned(),
            "lib1".to_owned(),
            "-I".to_owned(),
            "lib2".to_owned(),
            "-cn".to_owned(),
            "MyDSP".to_owned(),
            "-vec".to_owned(),
            "-vs".to_owned(),
            "64".to_owned(),
            "-lv".to_owned(),
            "1".to_owned(),
        ];
        let parsed = parse_ffi_compile_args(&argv).unwrap();
        assert_eq!(parsed.search_paths.len(), 2);
        assert_eq!(parsed.search_paths[0], PathBuf::from("lib1"));
        assert_eq!(parsed.search_paths[1], PathBuf::from("lib2"));
        assert_eq!(parsed.module_name.as_deref(), Some("MyDSP"));
        assert!(parsed.vec_mode);
        assert_eq!(parsed.vec_size, 64);
        assert_eq!(parsed.loop_variant, 1);
    }

    #[test]
    fn uses_default_vec_size() {
        let parsed = parse_ffi_compile_args(&["-vec".to_owned()]).unwrap();
        assert!(parsed.vec_mode);
        assert_eq!(parsed.vec_size, 32);
        assert_eq!(parsed.loop_variant, 0);
    }

    #[test]
    fn accepts_ss_option() {
        let parsed = parse_ffi_compile_args(&["-ss".to_owned(), "3".to_owned()]).unwrap();
        assert_eq!(parsed.scheduling_strategy, 3);
    }

    #[test]
    fn ss_defaults_to_zero_when_absent() {
        let parsed = parse_ffi_compile_args(&[]).unwrap();
        assert_eq!(parsed.scheduling_strategy, 0);
    }

    #[test]
    fn ss_is_independent_of_vec() {
        let parsed = parse_ffi_compile_args(&["-ss".to_owned(), "1".to_owned()]).unwrap();
        assert_eq!(parsed.scheduling_strategy, 1);
        assert!(!parsed.vec_mode);
        assert_eq!(parsed.vec_size, 32);
        assert_eq!(parsed.loop_variant, 0);
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

    #[test]
    fn rejects_missing_ss_value() {
        let error = parse_ffi_compile_args(&["-ss".to_owned()]).unwrap_err();
        assert!(error.contains("missing value after -ss"), "{error}");
    }

    #[test]
    fn rejects_non_integer_ss_value() {
        let error = parse_ffi_compile_args(&["-ss".to_owned(), "abc".to_owned()]).unwrap_err();
        assert!(error.contains("bad -ss value"), "{error}");
    }

    #[test]
    fn rejects_negative_ss_value() {
        let error = parse_ffi_compile_args(&["-ss".to_owned(), "-1".to_owned()]).unwrap_err();
        assert!(error.contains("bad -ss value"), "{error}");
    }
}
