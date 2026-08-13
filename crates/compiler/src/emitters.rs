//! Backend emission entry points of the [`Compiler`] facade.
//!
//! One `impl Compiler` block holding every `compile_*_to_<backend>` entry
//! point, grouped by backend. The grouping and the convention that governs it
//! are documented at the top of the block.

use std::path::{Path, PathBuf};

use crate::*;
use codegen::backends::asc::AscOptions;
use codegen::backends::c::COptions;
use codegen::backends::cmajor::CmajorOptions;
use codegen::backends::cpp::CppOptions;
#[cfg(not(target_arch = "wasm32"))]
use codegen::backends::cranelift::CraneliftOptions;
use codegen::backends::interp::InterpOptions;
use codegen::backends::julia::JuliaOptions;
use codegen::backends::rust::RustOptions;
use codegen::backends::wasm::{WasmModule, WasmOptions, generate_wasm_module_with_context};
pub use transform::signal_fir::RealType;

impl Compiler {
    // The emitter entry points below are grouped by backend, and each group
    // follows the same order — so once you know one backend, you know them all:
    //
    //   compile_source_to_X                  in-memory source, default lane
    //   compile_source_to_X_with_lane        in-memory source, explicit lane
    //   compile_file_to_X                    file + explicit search paths
    //   compile_file_to_X_with_lane
    //   compile_file_default_to_X            file + default search paths
    //   compile_file_default_to_X_with_lane
    //
    // The no-lane forms are thin wrappers that pick the default lane and
    // delegate to their `_with_lane` twin, which holds the real work. Three
    // groups deviate, each for a stated reason: the FIR dump is lane-only (the
    // lane *is* the thing being dumped), and the WASM and JSON groups end with
    // extra variants returning richer results.

    // ── C++ backend ───────────────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits C++ text.
    pub fn compile_source_to_cpp(
        &self,
        source_name: &str,
        source: &str,
        options: &CppOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_cpp_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits C++ text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_source_to_cpp_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &CppOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_cpp(source_name, &signals, options, ctx)
            .map_err(|e| lower_cpp_error_to_compiler(source_name, &signals, e))
    }

    /// Parses + evaluates + propagates one file, then emits C++ text.
    pub fn compile_file_to_cpp(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CppOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_cpp_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits C++ text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_cpp_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CppOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_cpp(&source, &signals, options, ctx)
            .map_err(|e| lower_cpp_error_to_compiler(&source, &signals, e))
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits C++ text.
    pub fn compile_file_default_to_cpp(
        &self,
        path: &Path,
        options: &CppOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_cpp_with_lane(path, options, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits C++ text using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_cpp_with_lane(
        &self,
        path: &Path,
        options: &CppOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_cpp_with_lane(path, &[], options, lane)
    }

    // ── C backend ─────────────────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits C text.
    pub fn compile_source_to_c(
        &self,
        source_name: &str,
        source: &str,
        options: &COptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_c_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits C text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_source_to_c_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &COptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_c(source_name, &signals, options, ctx)
            .map_err(|e| lower_c_error_to_compiler(source_name, &signals, e))
    }

    /// Parses + evaluates + propagates one file, then emits C text.
    pub fn compile_file_to_c(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &COptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_c_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits C text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_c_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &COptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_c(&source, &signals, options, ctx)
            .map_err(|e| lower_c_error_to_compiler(&source, &signals, e))
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits C text.
    pub fn compile_file_default_to_c(
        &self,
        path: &Path,
        options: &COptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_c_with_lane(path, options, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits C text using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_c_with_lane(
        &self,
        path: &Path,
        options: &COptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_c_with_lane(path, &[], options, lane)
    }

    // ── Rust backend ──────────────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits Rust text.
    pub fn compile_source_to_rust(
        &self,
        source_name: &str,
        source: &str,
        options: &RustOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_rust_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits Rust text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_source_to_rust_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &RustOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_rust(source_name, &signals, options, ctx)
            .map_err(|e| lower_rust_error_to_compiler(source_name, &signals, e))
    }

    /// Parses + evaluates + propagates one file, then emits Rust text.
    pub fn compile_file_to_rust(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &RustOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_rust_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits Rust text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_rust_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &RustOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_rust(&source, &signals, options, ctx)
            .map_err(|e| lower_rust_error_to_compiler(&source, &signals, e))
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits Rust text.
    pub fn compile_file_default_to_rust(
        &self,
        path: &Path,
        options: &RustOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_rust_with_lane(path, options, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits Rust text using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_rust_with_lane(
        &self,
        path: &Path,
        options: &RustOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_rust_with_lane(path, &[], options, lane)
    }

    // ── Julia backend ─────────────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits Julia text.
    pub fn compile_source_to_julia(
        &self,
        source_name: &str,
        source: &str,
        options: &JuliaOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_julia_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits Julia text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_source_to_julia_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &JuliaOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_julia(source_name, &signals, options, ctx)
            .map_err(|e| lower_julia_error_to_compiler(source_name, &signals, e))
    }

    /// Parses + evaluates + propagates one file, then emits Julia text.
    pub fn compile_file_to_julia(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &JuliaOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_julia_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits Julia text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_julia_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &JuliaOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_julia(&source, &signals, options, ctx)
            .map_err(|e| lower_julia_error_to_compiler(&source, &signals, e))
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits Julia text.
    pub fn compile_file_default_to_julia(
        &self,
        path: &Path,
        options: &JuliaOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_julia_with_lane(
            path,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits Julia text using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_julia_with_lane(
        &self,
        path: &Path,
        options: &JuliaOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_julia_with_lane(path, &[], options, lane)
    }

    // ── AssemblyScript backend ────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits AssemblyScript.
    pub fn compile_source_to_asc(
        &self,
        source_name: &str,
        source: &str,
        options: &AscOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_asc_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits AssemblyScript
    /// using the selected signal->FIR lowering lane.
    pub fn compile_source_to_asc_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &AscOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_asc(source_name, &signals, options, ctx)
            .map_err(|e| lower_asc_error_to_compiler(source_name, &signals, e))
    }

    /// Parses + evaluates + propagates one file, then emits AssemblyScript.
    pub fn compile_file_to_asc(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &AscOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_asc_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits AssemblyScript using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_asc_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &AscOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_asc(&source, &signals, options, ctx)
            .map_err(|e| lower_asc_error_to_compiler(&source, &signals, e))
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits AssemblyScript.
    pub fn compile_file_default_to_asc(
        &self,
        path: &Path,
        options: &AscOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_asc_with_lane(path, options, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits AssemblyScript using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_asc_with_lane(
        &self,
        path: &Path,
        options: &AscOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_asc_with_lane(path, &[], options, lane)
    }

    // ── Codebox backend (RNBO) ────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits codebox text.
    ///
    /// Codebox imposes external control and the one-sample processing API;
    /// `lower_signals_to_codebox` forces both, so this backend ignores
    /// `-ec`/`-os` rather than requiring them. Vector mode is rejected.
    pub fn compile_source_to_codebox(
        &self,
        source_name: &str,
        source: &str,
        options: &CodeboxOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_codebox_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits codebox text
    /// using the selected signal->FIR lowering lane.
    pub fn compile_source_to_codebox_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &CodeboxOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_codebox(source_name, &signals, options, ctx)
            .map_err(|e| lower_codebox_error_to_compiler(source_name, &signals, e))
    }

    /// Parses + evaluates + propagates one file, then emits codebox text.
    pub fn compile_file_to_codebox(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CodeboxOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_codebox_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits codebox text using
    /// the selected signal->FIR lowering lane.
    pub fn compile_file_to_codebox_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CodeboxOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_codebox(&source, &signals, options, ctx)
            .map_err(|e| lower_codebox_error_to_compiler(&source, &signals, e))
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits codebox text.
    pub fn compile_file_default_to_codebox(
        &self,
        path: &Path,
        options: &CodeboxOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_codebox_with_lane(
            path,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits codebox text using the selected signal->FIR lowering lane.
    pub fn compile_file_default_to_codebox_with_lane(
        &self,
        path: &Path,
        options: &CodeboxOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_codebox_with_lane(path, &[], options, lane)
    }

    // ── Cmajor backend ────────────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits Cmajor text.
    ///
    /// Cmajor intrinsically uses external control and one-sample processing;
    /// those modes are forced by the lowering dispatcher and vector mode is
    /// rejected with a stable execution-capability diagnostic.
    pub fn compile_source_to_cmajor(
        &self,
        source_name: &str,
        source: &str,
        options: &CmajorOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_cmajor_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits Cmajor text using
    /// the selected signal-to-FIR lowering lane.
    pub fn compile_source_to_cmajor_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &CmajorOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_cmajor(source_name, &signals, options, ctx)
            .map_err(|error| lower_cmajor_error_to_compiler(source_name, &signals, error))
    }

    /// Parses + evaluates + propagates one file, then emits Cmajor text.
    pub fn compile_file_to_cmajor(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CmajorOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_cmajor_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits Cmajor text using
    /// the selected signal-to-FIR lowering lane.
    pub fn compile_file_to_cmajor_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CmajorOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_cmajor(&source, &signals, options, ctx)
            .map_err(|error| lower_cmajor_error_to_compiler(&source, &signals, error))
    }

    /// Parses + evaluates + propagates one file with default import paths,
    /// then emits Cmajor text.
    pub fn compile_file_default_to_cmajor(
        &self,
        path: &Path,
        options: &CmajorOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_cmajor_with_lane(
            path,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file with default import paths,
    /// then emits Cmajor text using the selected signal-to-FIR lowering lane.
    pub fn compile_file_default_to_cmajor_with_lane(
        &self,
        path: &Path,
        options: &CmajorOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_cmajor_with_lane(path, &[], options, lane)
    }

    // ── Interpreter backend (`.fbc` bytecode) ─────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits `.fbc` bytecode
    /// text via the interpreter backend using the transform fast lane.
    pub fn compile_source_to_interp(
        &self,
        source_name: &str,
        source: &str,
        options: &InterpOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_interp_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits `.fbc` bytecode
    /// text using the selected signal->FIR lowering lane.
    pub fn compile_source_to_interp_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &InterpOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_interp(source_name, &signals, options, ctx)
            .map_err(|e| lower_interp_error_to_compiler(source_name, &signals, e))
    }

    /// Parses + evaluates + propagates one file, then emits `.fbc` bytecode
    /// text via the interpreter backend using the transform fast lane.
    pub fn compile_file_to_interp(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &InterpOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_interp_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits `.fbc` bytecode
    /// text using the selected signal->FIR lowering lane.
    pub fn compile_file_to_interp_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &InterpOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_interp(&source, &signals, options, ctx)
            .map_err(|e| lower_interp_error_to_compiler(&source, &signals, e))
    }

    /// Parses + evaluates + propagates one file with default import search
    /// path, then emits `.fbc` bytecode text via the interpreter backend.
    pub fn compile_file_default_to_interp(
        &self,
        path: &Path,
        options: &InterpOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_interp_with_lane(
            path,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file with default import search
    /// path, then emits `.fbc` bytecode text using the selected lane.
    pub fn compile_file_default_to_interp_with_lane(
        &self,
        path: &Path,
        options: &InterpOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_interp_with_lane(path, &[], options, lane)
    }

    // ── Cranelift JIT backend (status report) ─────────────────────────────────────
    //
    // The odd backend out, in one specific way: `generate_cranelift_module`
    // returns a live `JitDspModule` (executable memory, entry-point
    // addresses), which cannot be handed back as `String` like every other
    // group here — the caller would have to own its runtime lifetime.
    //
    // So what these entry points return is the backend *status report*: the
    // shape of the compiled module (symbol names, entry address, `dsp` struct
    // layout), rendered as text after the JIT module is dropped. That is the
    // same split as the interpreter, whose facade entry points return
    // serialized `.fbc` text rather than a live `FbcDspFactory`.
    //
    // Callers that need to *run* the compiled code must own the module and
    // therefore bypass the facade, lowering through the FIR group below and
    // calling `generate_cranelift_module` themselves — see
    // `crates/cranelift-ffi`.

    #[cfg(not(target_arch = "wasm32"))]
    /// Parses + evaluates + propagates one source, JIT-compiles it with the
    /// Cranelift backend, and returns the backend status report using the
    /// transform fast lane.
    pub fn compile_source_to_cranelift_report(
        &self,
        source_name: &str,
        source: &str,
        options: &CraneliftOptions,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_cranelift_report_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Parses + evaluates + propagates one source, JIT-compiles it with the
    /// Cranelift backend, and returns the backend status report using the
    /// selected signal->FIR lowering lane.
    pub fn compile_source_to_cranelift_report_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &CraneliftOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_cranelift_report(source_name, &signals, options, ctx)
            .map_err(|e| lower_cranelift_error_to_compiler(source_name, &signals, e))
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Parses + evaluates + propagates one file, then returns the Cranelift
    /// backend status report using the transform fast lane.
    pub fn compile_file_to_cranelift_report(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CraneliftOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_cranelift_report_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Parses + evaluates + propagates one file, then returns the Cranelift
    /// backend status report using the selected signal->FIR lowering lane.
    pub fn compile_file_to_cranelift_report_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &CraneliftOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        let ctx = self.lowering_ctx(lane);
        lower_signals_to_cranelift_report(&source, &signals, options, ctx)
            .map_err(|e| lower_cranelift_error_to_compiler(&source, &signals, e))
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Parses + evaluates + propagates one file with default import search
    /// path, then returns the Cranelift backend status report.
    pub fn compile_file_default_to_cranelift_report(
        &self,
        path: &Path,
        options: &CraneliftOptions,
    ) -> Result<String, CompilerError> {
        self.compile_file_default_to_cranelift_report_with_lane(
            path,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Parses + evaluates + propagates one file with default import search
    /// path, then returns the Cranelift backend status report using the
    /// selected lane.
    pub fn compile_file_default_to_cranelift_report_with_lane(
        &self,
        path: &Path,
        options: &CraneliftOptions,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_cranelift_report_with_lane(path, &[], options, lane)
    }

    // ── FIR module dump ───────────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then lowers to FIR using
    /// the selected signal->FIR lane.
    pub fn compile_source_to_fir_with_lane(
        &self,
        source_name: &str,
        source: &str,
        lane: SignalFirLane,
    ) -> Result<FirCompileOutput, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        self.lower_to_fir(source_name, &signals, lane)
    }

    /// Parses + evaluates + propagates one file, then lowers to FIR using
    /// the selected signal->FIR lane.
    pub fn compile_file_to_fir_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        lane: SignalFirLane,
    ) -> Result<FirCompileOutput, CompilerError> {
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let source = path.display().to_string();
        self.lower_to_fir(&source, &signals, lane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then lowers to FIR using the selected signal->FIR lane.
    pub fn compile_file_default_to_fir_with_lane(
        &self,
        path: &Path,
        lane: SignalFirLane,
    ) -> Result<FirCompileOutput, CompilerError> {
        self.compile_file_to_fir_with_lane(path, &[], lane)
    }

    // ── WebAssembly backend ───────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits a WASM module
    /// plus its matched companion JSON.
    ///
    /// This API defaults to [`SignalFirLane::TransformFastLane`] because the
    /// WASM/JSON-facing artifact surfaces need the canonical lowered FIR module
    /// with working `metadata`/`buildUserInterface` bodies.
    pub fn compile_source_to_wasm(
        &self,
        source_name: &str,
        source: &str,
        options: &WasmOptions,
    ) -> Result<WasmModule, CompilerError> {
        self.compile_source_to_wasm_with_lane(
            source_name,
            source,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one source, then emits a WASM module
    /// through the selected signal->FIR lane.
    pub fn compile_source_to_wasm_with_lane(
        &self,
        source_name: &str,
        source: &str,
        options: &WasmOptions,
        lane: SignalFirLane,
    ) -> Result<WasmModule, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let lowered = self.lower_to_fir(source_name, &signals, lane)?;
        let json_context = wasm_json_context_for_memory_source(
            source_name,
            &signals,
            compile_options_json_string(Some("wasm"), options.double_precision),
        );
        generate_wasm_module_with_context(&lowered.store, lowered.module, options, &json_context)
            .map_err(|error| wasm_error_to_compiler(source_name, &signals, &lowered, error))
    }

    /// Parses + evaluates + propagates one file, then emits a WASM module
    /// plus its matched companion JSON through the selected signal->FIR lane.
    pub fn compile_file_to_wasm(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &WasmOptions,
    ) -> Result<WasmModule, CompilerError> {
        self.compile_file_to_wasm_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Parses + evaluates + propagates one file, then emits a WASM module
    /// through the selected signal->FIR lane.
    pub fn compile_file_to_wasm_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &WasmOptions,
        lane: SignalFirLane,
    ) -> Result<WasmModule, CompilerError> {
        let source = path.display().to_string();
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let lowered = self.lower_to_fir(&source, &signals, lane)?;
        let json_context = wasm_json_context_for_file(
            path,
            search_paths,
            &signals,
            compile_options_json_string(Some("wasm"), options.double_precision),
        );
        generate_wasm_module_with_context(&lowered.store, lowered.module, options, &json_context)
            .map_err(|error| wasm_error_to_compiler(&source, &signals, &lowered, error))
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits a WASM module scaffold.
    ///
    /// This file-backed convenience wrapper follows the same default-lane
    /// policy as [`Compiler::compile_source_to_wasm`]: artifact-oriented WASM
    /// entry points default to [`SignalFirLane::TransformFastLane`].
    pub fn compile_file_default_to_wasm(
        &self,
        path: &Path,
        options: &WasmOptions,
    ) -> Result<WasmModule, CompilerError> {
        self.compile_file_default_to_wasm_with_lane(path, options, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits a WASM module through the selected signal->FIR lane.
    pub fn compile_file_default_to_wasm_with_lane(
        &self,
        path: &Path,
        options: &WasmOptions,
        lane: SignalFirLane,
    ) -> Result<WasmModule, CompilerError> {
        self.compile_file_to_wasm_with_lane(path, &[], options, lane)
    }

    /// Compiles one in-memory DSP source into an owned artifact bundle
    /// containing both the WASM bytes and the companion JSON.
    ///
    /// This is the pure-Rust compile-service entry point behind the
    /// `faustwasm` embedded-compiler mode. The returned
    /// [`WasmArtifactBundle`] avoids any explicit compiler-side object lifetime
    /// and can be cached directly by higher-level hosts.
    ///
    /// Requests default to [`SignalFirLane::TransformFastLane`] for the same
    /// reason as [`Compiler::compile_source_to_wasm`]: JSON/WASM artifact
    /// consumers need preserved UI and metadata fidelity.
    pub fn compile_wasm_artifact(
        &self,
        request: &WasmArtifactRequest,
    ) -> Result<WasmArtifactBundle, CompilerError> {
        let compile_options =
            compile_options_json_string(Some("wasm"), request.wasm_options.double_precision);
        let signals = self.compile_source_to_signals_with_import_context(
            &request.source_name,
            &request.source,
            &request.import_dirs,
            &request.virtual_sources,
        )?;
        let warnings = signals.warnings.clone();
        let lowered = self.lower_to_fir(&request.source_name, &signals, request.lane)?;
        let mut json_context = wasm_json_context_for_memory_source(
            &request.source_name,
            &signals,
            compile_options.clone(),
        );
        json_context.include_pathnames = request
            .import_dirs
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        json_context.library_list = collect_library_list(&signals);
        let module = generate_wasm_module_with_context(
            &lowered.store,
            lowered.module,
            &request.wasm_options,
            &json_context,
        )
        .map_err(|error| wasm_error_to_compiler(&request.source_name, &signals, &lowered, error))?;
        Ok(WasmArtifactBundle::from_wasm_module(
            module,
            compile_options,
            warnings,
        ))
    }

    /// Compiles one file-backed DSP source into an owned artifact bundle using
    /// the production default signal->FIR lane.
    pub fn compile_file_to_wasm_artifact(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &WasmOptions,
    ) -> Result<WasmArtifactBundle, CompilerError> {
        self.compile_file_to_wasm_artifact_with_lane(
            path,
            search_paths,
            options,
            SignalFirLane::TransformFastLane,
        )
    }

    /// Compiles one file-backed DSP source into an owned artifact bundle.
    ///
    /// Packages the result in the artifact-centric shape expected by the
    /// `faustwasm` dual-mode integration plan, so downstream code can treat
    /// compile mode and precompiled-artifact mode uniformly. This inlines the
    /// same pipeline as [`Self::compile_file_to_wasm_with_lane`] — rather than
    /// calling it — so it can retain `signals` long enough to forward its
    /// `warnings`, the same way [`Self::compile_wasm_artifact`] does for the
    /// source-based path.
    pub fn compile_file_to_wasm_artifact_with_lane(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        options: &WasmOptions,
        lane: SignalFirLane,
    ) -> Result<WasmArtifactBundle, CompilerError> {
        let compile_options = compile_options_json_string(Some("wasm"), options.double_precision);
        let source = path.display().to_string();
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let warnings = signals.warnings.clone();
        let lowered = self.lower_to_fir(&source, &signals, lane)?;
        let json_context =
            wasm_json_context_for_file(path, search_paths, &signals, compile_options.clone());
        let module = generate_wasm_module_with_context(
            &lowered.store,
            lowered.module,
            options,
            &json_context,
        )
        .map_err(|error| wasm_error_to_compiler(&source, &signals, &lowered, error))?;
        Ok(WasmArtifactBundle::from_wasm_module(
            module,
            compile_options,
            warnings,
        ))
    }

    /// Compiles one file-backed DSP source with the default import search model
    /// into an owned artifact bundle.
    ///
    /// This is the file-backed companion to [`Compiler::compile_wasm_artifact`]
    /// and therefore also defaults to [`SignalFirLane::TransformFastLane`].
    pub fn compile_file_default_to_wasm_artifact(
        &self,
        path: &Path,
        options: &WasmOptions,
    ) -> Result<WasmArtifactBundle, CompilerError> {
        self.compile_file_to_wasm_artifact(path, &[], options)
    }

    // ── JSON description ──────────────────────────────────────────────────────────

    /// Parses + evaluates + propagates one source, then emits strict C++-style JSON.
    ///
    /// Like the WASM artifact entry points, this API defaults to
    /// [`SignalFirLane::TransformFastLane`] so the reconstructed JSON sees the
    /// canonical FIR `metadata` and `buildUserInterface` bodies.
    pub fn compile_source_to_json(
        &self,
        source_name: &str,
        source: &str,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_json_with_lane(source_name, source, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one source, then emits strict C++-style JSON
    /// through the selected signal->FIR lane.
    pub fn compile_source_to_json_with_lane(
        &self,
        source_name: &str,
        source: &str,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_json_with_lane_and_compile_options(
            source_name,
            source,
            lane,
            compile_options_json_string(None, self.real_type == RealType::Float64),
        )
    }

    /// Parses + evaluates + propagates one source, then emits strict C++-style JSON
    /// through the selected signal->FIR lane with explicit `compile_options`.
    pub fn compile_source_to_json_with_lane_and_compile_options(
        &self,
        source_name: &str,
        source: &str,
        lane: SignalFirLane,
        compile_options: String,
    ) -> Result<String, CompilerError> {
        self.compile_source_to_json_with_lane_compile_options_and_memory(
            source_name,
            source,
            lane,
            compile_options,
            None,
        )
    }

    /// Emits strict JSON for source text with an optional native `-mem0`
    /// analysis selected by its effective backend layout.
    ///
    /// Mapping status: `adapted`. C++ exposes memory metadata through its
    /// global compiler option state; this facade makes the backend flavor an
    /// explicit request value and leaves ordinary JSON byte-stable with
    /// `None`.
    pub fn compile_source_to_json_with_lane_compile_options_and_memory(
        &self,
        source_name: &str,
        source: &str,
        lane: SignalFirLane,
        compile_options: String,
        memory_flavor: Option<MemoryLayoutFlavor>,
    ) -> Result<String, CompilerError> {
        let signals = self.compile_source_to_signals(source_name, source)?;
        let lowered = self.lower_to_fir(source_name, &signals, lane)?;
        let json = build_strict_json_description(
            &lowered.store,
            lowered.module,
            StrictJsonContext {
                filename: source_name_to_filename(source_name),
                include_pathnames: Vec::new(),
                library_list: Vec::new(),
                top_level_meta: json_meta_entries_from_snapshot(&signals.compilation_metadata),
                compile_options,
                double_precision: self.real_type == RealType::Float64,
                memory_flavor,
            },
        )
        .map_err(|error| wasm_error_to_compiler(source_name, &signals, &lowered, error))?;
        Ok(json.render())
    }

    /// Parses + evaluates + propagates one file, then emits strict C++-style JSON.
    pub fn compile_file_to_json(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_json_with_compile_options(
            path,
            search_paths,
            lane,
            compile_options_json_string(None, self.real_type == RealType::Float64),
        )
    }

    /// Parses + evaluates + propagates one file, then emits strict C++-style JSON
    /// with explicit `compile_options` provenance.
    pub fn compile_file_to_json_with_compile_options(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        lane: SignalFirLane,
        compile_options: String,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_json_with_compile_options_and_memory(
            path,
            search_paths,
            lane,
            compile_options,
            None,
        )
    }

    /// Emits strict JSON with an optional effective native `-mem0` layout.
    ///
    /// The explicit flavor prevents a JSON companion from accidentally using
    /// Wasm or host-default layout semantics for C, C++, or Cranelift.
    pub fn compile_file_to_json_with_compile_options_and_memory(
        &self,
        path: &Path,
        search_paths: &[PathBuf],
        lane: SignalFirLane,
        compile_options: String,
        memory_flavor: Option<MemoryLayoutFlavor>,
    ) -> Result<String, CompilerError> {
        let source = path.display().to_string();
        let signals = self.compile_file_to_signals(path, search_paths)?;
        let lowered = self.lower_to_fir(&source, &signals, lane)?;
        let library_list = collect_library_list(&signals);
        let json = build_strict_json_description(
            &lowered.store,
            lowered.module,
            StrictJsonContext {
                filename: path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| path.to_string_lossy().into_owned()),
                include_pathnames: merge_import_search_paths(path, search_paths)
                    .into_iter()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .collect(),
                library_list,
                top_level_meta: json_meta_entries_from_snapshot(&signals.compilation_metadata),
                compile_options,
                double_precision: self.real_type == RealType::Float64,
                memory_flavor,
            },
        )
        .map_err(|error| wasm_error_to_compiler(&source, &signals, &lowered, error))?;
        Ok(json.render())
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits strict C++-style JSON.
    ///
    /// This file-backed convenience wrapper follows the same default-lane
    /// policy as [`Compiler::compile_source_to_json`].
    pub fn compile_file_default_to_json(&self, path: &Path) -> Result<String, CompilerError> {
        self.compile_file_default_to_json_with_lane(path, SignalFirLane::TransformFastLane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits strict C++-style JSON through the selected signal->FIR lane.
    pub fn compile_file_default_to_json_with_lane(
        &self,
        path: &Path,
        lane: SignalFirLane,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_json(path, &[], lane)
    }

    /// Parses + evaluates + propagates one file with default import search path,
    /// then emits strict C++-style JSON through the selected signal->FIR lane
    /// with explicit `compile_options` provenance.
    pub fn compile_file_default_to_json_with_lane_and_compile_options(
        &self,
        path: &Path,
        lane: SignalFirLane,
        compile_options: String,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_json_with_compile_options(path, &[], lane, compile_options)
    }

    /// Default-search-path companion of
    /// [`Compiler::compile_file_to_json_with_compile_options_and_memory`].
    pub fn compile_file_default_to_json_with_lane_compile_options_and_memory(
        &self,
        path: &Path,
        lane: SignalFirLane,
        compile_options: String,
        memory_flavor: Option<MemoryLayoutFlavor>,
    ) -> Result<String, CompilerError> {
        self.compile_file_to_json_with_compile_options_and_memory(
            path,
            &[],
            lane,
            compile_options,
            memory_flavor,
        )
    }
}
