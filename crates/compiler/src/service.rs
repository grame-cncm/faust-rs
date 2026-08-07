//! The helper service surface: `getInfos`, `expandDSP` and `generateAuxFiles`.
//!
//! These mirror the C++ Faust helper APIs rather than the compile entry
//! points, and they report through [`FaustwasmServiceError`] instead of
//! [`CompilerError`] because their callers are host bindings that propagate a
//! code plus a message. `faustwasm` is one such caller, not the only one.

use std::path::PathBuf;

use crate::*;
use codegen::backends::asc::{AscOptions, generate_asc_module};
use codegen::backends::c::COptions;
use codegen::backends::cpp::CppOptions;
use codegen::backends::wasm::WasmOptions;
pub use transform::schedule::SchedulingStrategy;
pub use transform::signal_fir::{ComputeMode, ControlRateMode, ProcessingApi, RealType};

impl Compiler {
    // ── Helper service surface (`faustwasm`-oriented) ─────────────────────────────

    /// Returns one `faustwasm` helper-info string.
    ///
    /// Current compatibility policy mirrors C++ `libFaustWasm::getInfos` for
    /// `version`, `help`, `libdir`, `includedir`, `archdir`, `dspdir`, and
    /// `pathslist`; unknown keys remain invalid.
    pub fn get_faustwasm_info(&self, what: &str) -> Result<String, FaustwasmServiceError> {
        match what {
            "version" => Ok(Self::version().to_owned()),
            "help" => Ok(faustwasm_info_help_text()),
            "libdir" => Ok(FaustInstallPaths::from_environment().render_lib_dir()),
            "includedir" => Ok(FaustInstallPaths::from_environment().render_include_dir()),
            "archdir" => Ok(FaustInstallPaths::from_environment().render_arch_dir()),
            "dspdir" => Ok(FaustInstallPaths::from_environment().render_dsp_dir()),
            "pathslist" => Ok(FaustInstallPaths::from_environment().render_paths_list()),
            _ => Err(FaustwasmServiceError::invalid_argument(format!(
                "incorrect argument passed to getInfos: {what}"
            ))),
        }
    }

    /// Validate and expand one Faust DSP source.
    ///
    /// Parses and evaluates the program using any `-I` search paths carried in
    /// `request.args`.  If compilation succeeds the original source text is
    /// returned verbatim; the Rust compiler currently has no box→DSP serializer
    /// analogous to C++ `printBox`, so the expanded form equals the input.
    ///
    /// Mirrors: `expandDSPFromString` / `expandDSPFromFile` (C++ Faust API).
    pub fn expand_dsp(&self, request: &ExpandDspRequest) -> Result<String, FaustwasmServiceError> {
        let argv: Vec<String> = request.args.split_whitespace().map(str::to_owned).collect();
        let search_paths = parse_search_paths_from_argv(&argv);
        self.compile_source_to_signals_with_search_paths(
            &request.source_name,
            &request.source,
            &search_paths,
        )
        .map(|_| request.source.clone())
        .map_err(FaustwasmServiceError::compile_failure)
    }

    /// Returns a copy of this compiler with the compilation options found in a
    /// raw argv string applied.
    ///
    /// Reads the same flag spellings as the `faust-rs` CLI
    /// (`crates/compiler/src/cli/args.rs`), and selects the same [`RealType`],
    /// [`ComputeMode`], [`SchedulingStrategy`], [`ControlRateMode`], and
    /// [`ProcessingApi`] that [`cli::runner::compiler_from_cli`] would build
    /// for those flags:
    ///
    /// | flag | effect |
    /// |------|--------|
    /// | `-double` | double-precision internal DSP computation |
    /// | `-mcd N` / `-dlt N` | delay-line strategy thresholds |
    /// | `-vec` (+ `-vs N`, `-lv N`) | vector `compute` shape |
    /// | `-ss N` / `--scheduling-strategy N` | statement scheduling order |
    /// | `-ec` / `--external-control` | separate `control` entry point |
    /// | `-os` / `--one-sample` | `frame` entry point |
    ///
    /// These are plain Faust CLI flags rather than a dialect belonging to any
    /// one caller, which is why decoding them here — instead of only in the
    /// CLI — keeps [`Compiler::generate_aux_files`] from silently ignoring
    /// options its callers legitimately pass.
    ///
    /// [`cli::runner::compiler_from_cli`]: ../compiler/cli/runner/fn.compiler_from_cli.html
    fn with_execution_options_from_argv(&self, argv: &[String]) -> Compiler {
        let has = |names: &[&str]| argv.iter().any(|arg| names.contains(&arg.as_str()));
        let mut compiler = self.clone();
        if has(&["-double", "--double"]) {
            compiler = compiler.with_real_type(RealType::Float64);
        }
        if let Some(n) = argv_value_parsed(argv, &["-mcd", "--mcd"]) {
            compiler = compiler.with_mcd(n);
        }
        if let Some(n) = argv_value_parsed(argv, &["-dlt", "--dlt"]) {
            compiler = compiler.with_dlt(n);
        }
        if let Some(mode) = argv_value(argv, &["--table-init", "-table-init"]) {
            // An unknown value keeps the default rather than failing here; the
            // CLI layer is where argument validation belongs.
            match mode {
                "runtime" => compiler = compiler.with_table_init_mode(TableInitMode::Runtime),
                "const" => compiler = compiler.with_table_init_mode(TableInitMode::Const),
                _ => {}
            }
        }
        if has(&["-vec", "--vec"]) {
            let vec_size =
                argv_value_parsed(argv, &["-vs", "--vs"]).unwrap_or(ComputeMode::DEFAULT_VEC_SIZE);
            let loop_variant = argv_value_parsed(argv, &["-lv", "--lv"]).unwrap_or(0u8);
            compiler = compiler.with_compute_mode(ComputeMode::Vector {
                vec_size,
                loop_variant,
            });
        }
        if let Some(n) = argv_value_parsed(argv, &["-ss", "--scheduling-strategy"]) {
            compiler = compiler.with_scheduling_strategy(SchedulingStrategy::decode(n));
        }
        if has(&["-ec", "--ec", "--external-control", "--ext-control"]) {
            compiler = compiler.with_control_rate_mode(ControlRateMode::External);
        }
        if has(&["-os", "--os", "--one-sample"]) {
            compiler = compiler.with_processing_api(ProcessingApi::OneSample);
        }
        compiler
    }

    /// Generate auxiliary output files from a Faust DSP source.
    ///
    /// Inspects `request.args` for output-format flags and returns one
    /// [`AuxFileArtifact`] per generated file, all emitted in memory. When no
    /// output flag is present an empty list is returned (no error).
    ///
    /// | flag | artifacts |
    /// |------|-----------|
    /// | `-cpp` / `-c` | `<stem>.cpp` / `<stem>.c` |
    /// | `-wasm` | `<stem>.wasm` plus its matched `<stem>.json` |
    /// | `-json` | `<stem>.json` (redundant under `-wasm`, see below) |
    /// | `-svg` | `process.svg` plus one file per folded sub-diagram |
    /// | `-lang asc` | one AssemblyScript source, via the internal ASC auxiliary-file generator |
    ///
    /// `request.args` also carries the compilation options applied to every
    /// output; the internal execution-option normalizer lists them.
    ///
    /// Mirrors: `generateAuxFilesFromString` / `generateAuxFilesFromFile`
    /// (C++ Faust API). Two deliberate deviations: the artifacts are returned
    /// instead of written to disk (so wasm32 hosts without a writable
    /// filesystem can consume them), and `-lang asc` is an extension of this
    /// surface with no C++ counterpart.
    pub fn generate_aux_files(
        &self,
        request: &GenerateAuxFilesRequest,
    ) -> Result<Vec<AuxFileArtifact>, FaustwasmServiceError> {
        let argv: Vec<String> = request.args.split_whitespace().map(str::to_owned).collect();
        let search_paths = parse_search_paths_from_argv(&argv);

        // Compilation options travel in the same argv string; a derived
        // compiler applies them so downstream compile calls validate them
        // instead of silently ignoring the flags.
        let compiler = self.with_execution_options_from_argv(&argv);

        // `-lang asc` transpile request: produces one AssemblyScript source
        // artifact instead of the flag-driven outputs.
        if argv_value(&argv, &["-lang"]) == Some("asc") {
            return compiler.generate_asc_aux_file(request, &argv, &search_paths);
        }

        let wants = |flag: &str| argv.iter().any(|arg| arg == flag);
        let name = request.source_name.as_str();
        let source = request.source.as_str();
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("process");

        let mut artifacts: Vec<AuxFileArtifact> = Vec::new();

        if wants("-cpp") {
            let cpp = compiler
                .compile_source_to_cpp(name, source, &CppOptions::default())
                .map_err(FaustwasmServiceError::compile_failure)?;
            artifacts.push(AuxFileArtifact::text(stem, "cpp", cpp));
        }

        if wants("-c") {
            let c = compiler
                .compile_source_to_c(name, source, &COptions::default())
                .map_err(FaustwasmServiceError::compile_failure)?;
            artifacts.push(AuxFileArtifact::text(stem, "c", c));
        }

        if wants("-wasm") {
            // The module and its JSON description are a matched pair (see
            // `WasmArtifactBundle`'s ABI contract), so `-wasm` emits both.
            let options = WasmOptions {
                double_precision: compiler.real_type == RealType::Float64,
                ..Default::default()
            };
            let wasm = compiler
                .compile_source_to_wasm(name, source, &options)
                .map_err(FaustwasmServiceError::compile_failure)?;
            artifacts.push(AuxFileArtifact::binary(stem, "wasm", wasm.wasm_binary));
            artifacts.push(AuxFileArtifact::text(stem, "json", wasm.dsp_json));
        }

        // Under `-wasm` the companion JSON above already occupies
        // `<stem>.json` and is the description matched to the emitted module,
        // so `-json` stands down rather than adding a second artifact at the
        // same path.
        if wants("-json") && !wants("-wasm") {
            let json = compiler
                .compile_source_to_json(name, source)
                .map_err(FaustwasmServiceError::compile_failure)?;
            artifacts.push(AuxFileArtifact::text(stem, "json", json));
        }

        if wants("-svg") {
            let signals = compiler
                .compile_source_to_signals_with_import_context(
                    name,
                    source,
                    &search_paths,
                    &request.virtual_sources,
                )
                .map_err(FaustwasmServiceError::compile_failure)?;
            // Name the diagram "process" so the root file is always
            // process.svg, matching the C++ compiler convention and the
            // faustwasm `FaustSvgDiagrams.from(...)` expectation.
            // draw_schema_to_memory avoids any filesystem access, so this
            // path is safe on wasm32-unknown-unknown targets.
            let svg_files = draw::draw_schema_to_memory(
                &signals.parse.state.arena,
                signals.process_box,
                "process",
                &draw::DrawConfig::default(),
                &signals.def_names,
            )
            .map_err(FaustwasmServiceError::unsupported)?;
            artifacts.extend(
                svg_files
                    .into_iter()
                    .map(|(path, content)| AuxFileArtifact {
                        path,
                        content,
                        binary: false,
                    }),
            );
        }

        Ok(artifacts)
    }

    /// Generates the AssemblyScript source artifact for a `-lang asc`
    /// [`Compiler::generate_aux_files`] request: `-cn` selects the class name
    /// (default: derived from the source name), `-o` the artifact path
    /// (default: `<class name>.ts`), and `request.virtual_sources` supplies
    /// in-memory libraries.
    ///
    /// Called on the derived compiler, so the compilation options from the
    /// argv are already applied. It stays a dedicated function rather than
    /// another output branch of the caller because, unlike the flag-driven
    /// cpp/c/wasm/json outputs, it does not go through a
    /// `compile_source_to_*` wrapper: it lowers to FIR itself so it can embed
    /// a hand-built strict JSON snapshot as `getJSON()` in the generated
    /// class, and it names its single output from `-cn`/`-o` instead of the
    /// shared `<stem>.<extension>` scheme.
    fn generate_asc_aux_file(
        &self,
        request: &GenerateAuxFilesRequest,
        argv: &[String],
        search_paths: &[PathBuf],
    ) -> Result<Vec<AuxFileArtifact>, FaustwasmServiceError> {
        let double_precision = self.real_type == RealType::Float64;
        let signals = self
            .compile_source_to_signals_with_import_context(
                &request.source_name,
                &request.source,
                search_paths,
                &request.virtual_sources,
            )
            .map_err(FaustwasmServiceError::compile_failure)?;
        let lowered = self
            .lower_to_fir(
                &request.source_name,
                &signals,
                SignalFirLane::TransformFastLane,
            )
            .map_err(FaustwasmServiceError::compile_failure)?;

        let class_name = argv_value(argv, &["-cn"]).map_or_else(
            || sanitize_cpp_ident(source_name_to_class(&request.source_name).as_str()),
            str::to_owned,
        );

        // Strict C++-style JSON snapshot, embedded as getJSON() in the output —
        // downstream tooling parses it for inputs/outputs and the UI tree
        // (mirrors the C++ asc backend's getJSON()).
        let compile_options = compile_options_json_string(Some("asc"), double_precision);
        let json = build_strict_json_description(
            &lowered.store,
            lowered.module,
            StrictJsonContext {
                filename: source_name_to_filename(&request.source_name),
                include_pathnames: Vec::new(),
                library_list: Vec::new(),
                top_level_meta: json_meta_entries_from_snapshot(&signals.compilation_metadata),
                compile_options: compile_options.clone(),
                double_precision,
            },
        )
        .ok()
        .map(|json| json.render());

        let options = AscOptions {
            class_name: Some(class_name.clone()),
            double_precision,
            json,
            compile_options: Some(compile_options),
            ..AscOptions::default()
        };
        let asc = generate_asc_module(&lowered.store, lowered.module, &options)
            .map_err(FaustwasmServiceError::unsupported)?;

        let path =
            argv_value(argv, &["-o"]).map_or_else(|| format!("{class_name}.ts"), str::to_owned);
        Ok(vec![AuxFileArtifact {
            path,
            content: asc.into_bytes(),
            binary: false,
        }])
    }
}
