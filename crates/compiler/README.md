# compiler

Top-level compiler facade.  Wires all pipeline stages together behind a single
`Compiler` struct and a unified `CompilerError` surface.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/libcode.cpp` | Compile entry points and orchestration |
| `compiler/global.cpp` | Session lifecycle |

## Public API

| Item | Description |
|---|---|
| `Compiler` | Session handle; all compile entry points are methods |
| `CompilerError` | Aggregated error type covering every pipeline stage |
| `SignalCompileOutput` | Parse + eval + propagate result package |
| `enrobage` | Architecture-file wrapping (Step E) |

### `Compiler` entry points

| Method group | Output |
|---|---|
| `compile_source` / `compile_file` / `compile_file_default` | Parsed `ParseOutput` |
| `compile_*_to_signals` | Evaluated and propagated `SignalCompileOutput` |
| `compile_source_to_cpp[_with_lane]` | C++ source string |
| `compile_file_to_cpp[_with_lane]` | C++ source string |
| `compile_source_to_c[_with_lane]` | C source string |
| `compile_file_to_c[_with_lane]` | C source string |
| `compile_source_to_interp[_with_lane]` | `.fbc` bytecode string |
| `compile_file_to_interp[_with_lane]` | `.fbc` bytecode string |
| `compile_file_default_to_interp[_with_lane]` | `.fbc` bytecode string |
| `compile_source_to_asc[_with_lane]` | AssemblyScript source string |
| `compile_file_to_asc[_with_lane]` | AssemblyScript source string |
| `compile_file_default_to_asc[_with_lane]` | AssemblyScript source string |
| `compile_source_to_codebox[_with_lane]` | RNBO codebox source string |
| `compile_file_to_codebox[_with_lane]` | RNBO codebox source string |
| `compile_file_default_to_codebox[_with_lane]` | RNBO codebox source string |
| `compile_source_to_rust[_with_lane]` | Rust source string |
| `compile_file_to_rust[_with_lane]` | Rust source string |
| `compile_file_default_to_rust[_with_lane]` | Rust source string |
| `compile_source_to_julia[_with_lane]` | Julia source string |
| `compile_file_to_julia[_with_lane]` | Julia source string |
| `compile_file_default_to_julia[_with_lane]` | Julia source string |
| `compile_source_to_fir_with_lane` | Owned `FirCompileOutput` |
| `compile_file_to_fir_with_lane` / `compile_file_default_to_fir_with_lane` | Owned `FirCompileOutput` |
| `compile_source_to_wasm[_with_lane]` | `WasmModule` (`.wasm` + companion JSON) |
| `compile_file_to_wasm[_with_lane]` | `WasmModule` (`.wasm` + companion JSON) |
| `compile_file_default_to_wasm[_with_lane]` | `WasmModule` (`.wasm` + companion JSON) |
| `compile_wasm_artifact` | Owned `WasmArtifactBundle` |
| `compile_file_to_wasm_artifact[_with_lane]` | Owned `WasmArtifactBundle` |
| `compile_file_default_to_wasm_artifact` | Owned `WasmArtifactBundle` |
| `compile_source_to_json[_with_lane]` | Strict Faust JSON string |
| `compile_file_to_json` / `compile_file_default_to_json[_with_lane]` | Strict Faust JSON string |
| `compile_source_to_json_with_lane_and_compile_options` / `compile_file_to_json_with_compile_options` | JSON string + explicit `compile_options` provenance |
| `compile_file_default_to_c[_with_lane]` / `compile_file_default_to_cpp[_with_lane]` | File-backed convenience wrappers without explicit search paths |
| `get_faustwasm_info` / `expand_dsp` / `generate_aux_files` | Faustwasm-compatible helper services |

### Lane defaults to know

- C / C++ file/source helpers now default to `SignalFirLane::TransformFastLane`.
- WASM / strict JSON source helpers default to `SignalFirLane::TransformFastLane`.
- Julia helpers default to `SignalFirLane::TransformFastLane`.
- Rust helpers default to `SignalFirLane::TransformFastLane`.
- AssemblyScript helpers default to `SignalFirLane::TransformFastLane`.
- Codebox helpers default to `SignalFirLane::TransformFastLane` and force the
  target's intrinsic external-control and one-sample lowering modes; vector
  mode is rejected.
- Interpreter helpers now default to `SignalFirLane::TransformFastLane`.
- `WasmArtifactRequest::new(...)` defaults to `SignalFirLane::TransformFastLane`.
- `compile_file_default_to_wasm_artifact(...)` also defaults to
  `SignalFirLane::TransformFastLane`.

## Pipeline

```
parse → eval → propagate → [optional signal→FIR] → codegen (C / C++ / Codebox / Rust / AssemblyScript / .fbc / Cranelift / WASM / Julia / JSON)
```

The public signal->FIR route is:

| Lane | Description |
|---|---|
| `SignalFirLane::TransformFastLane` | `transform::signal_prepare` + `transform::signal_fir` |

## Facade responsibilities

- Provide one orchestrator type (`Compiler`) for source- and file-based compilation.
- Aggregate typed stage errors into one top-level `CompilerError`.
- Provide test/golden-oriented helper outputs (box dump, signal dump, FIR dump).
- Route backend generation to C, C++, Codebox (RNBO), Rust, AssemblyScript,
  Julia, interpreter bytecode, Cranelift JIT, WASM/JSON artifacts, and strict
  JSON emitters with consistent options. Cranelift is currently a CLI/FFI route
  over the shared FIR lowering rather than a dedicated `Compiler::compile_*`
  method.
- Apply architecture wrapping for C, C++, and Julia output when `-a` is used.
