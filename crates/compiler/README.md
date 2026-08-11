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
| `remote_fetch` | Optional native HTTP(S) transport and host authorization policy |

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
| `compile_*_to_cranelift_report[_with_lane]` | Cranelift JIT backend status report string (native targets only) |
| `compile_source_to_asc[_with_lane]` | AssemblyScript source string |
| `compile_file_to_asc[_with_lane]` | AssemblyScript source string |
| `compile_file_default_to_asc[_with_lane]` | AssemblyScript source string |
| `compile_source_to_codebox[_with_lane]` | RNBO codebox source string |
| `compile_file_to_codebox[_with_lane]` | RNBO codebox source string |
| `compile_file_default_to_codebox[_with_lane]` | RNBO codebox source string |
| `compile_source_to_cmajor[_with_lane]` | Cmajor processor source string |
| `compile_file_to_cmajor[_with_lane]` | Cmajor processor source string |
| `compile_file_default_to_cmajor[_with_lane]` | Cmajor processor source string |
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

## HTTP(S) sources and architectures

Native HTTP(S) loading is optional and disabled by default. It requires two
independent opt-ins:

1. build `compiler` with the `network-imports` Cargo feature;
2. enable networking for the individual CLI invocation or `Compiler` value.

CLI example:

```bash
cargo run -p compiler --features network-imports -- \
  --allow-network-imports -lang cpp https://example.test/main.dsp
```

The same option permits an explicit URL import in a local DSP and a remote main
architecture template:

```bash
cargo run -p compiler --features network-imports -- \
  --allow-network-imports -lang cpp local.dsp \
  -a https://example.test/architecture.cpp
```

The Rust facade offers two activation levels:

```rust,no_run
use std::sync::Arc;

use compiler::{
    Compiler,
    remote_fetch::{AllowAllRemoteUrls, UreqSourceFetcher},
};
use parser::RemoteFetchPolicy;

let compiler = Compiler::new().with_remote_source_fetcher(
    Arc::new(UreqSourceFetcher::new(Arc::new(AllowAllRemoteUrls))),
    RemoteFetchPolicy::default(),
);
# let _ = compiler;
```

- `Compiler::with_remote_source_fetcher(...)` accepts an application-supplied
  capability and policy. This is the preferred API for servers and embedded
  applications.
- `Compiler::with_native_network_imports()` installs the built-in native
  transport with the default limits and unrestricted HTTP(S) host policy. It
  is available only with `network-imports` on non-WASM targets and is intended
  for explicitly trusted CLI-style use.
- `remote_fetch::UreqSourceFetcher::new(...)` accepts a `RemoteUrlPolicy`.
  Initial URLs and every redirect destination are checked by that policy, so a
  server can enforce an allowlist or reject private-network destinations.
- `enrobage::wrap_cpp_with_remote_architecture(...)` uses the same injected
  fetch interface and limits; there is no second HTTP implementation.

Supported remote resolution includes direct HTTP(S) entry sources, explicit
URL imports, and relative imports within a remote source graph. Requests have
bounded time, redirect count, and response size; response text must be UTF-8;
URL credentials are rejected. A feature-off or runtime-off compiler performs
no request and reports the stable `FRS-SRC-0005` diagnostic.

Native networking remains disabled for browser-WASM and the C/C++
compatibility facades. Browser hosts can asynchronously prefetch HTTP(S)
source graphs and inject canonical URL/content entries through `wasm-ffi`;
the compiler then resolves URL-relative structural imports without performing
I/O. Remote evaluator-driven `component(...)` / `library(...)` loads and
remote inline architecture sub-includes are currently deferred.

### Lane defaults to know

- C / C++ file/source helpers now default to `SignalFirLane::TransformFastLane`.
- WASM / strict JSON source helpers default to `SignalFirLane::TransformFastLane`.
- Julia helpers default to `SignalFirLane::TransformFastLane`.
- Rust helpers default to `SignalFirLane::TransformFastLane`.
- AssemblyScript helpers default to `SignalFirLane::TransformFastLane`.
- Codebox helpers default to `SignalFirLane::TransformFastLane` and force the
  target's intrinsic external-control and one-sample lowering modes; vector
  mode is rejected.
- Cmajor helpers default to `SignalFirLane::TransformFastLane` and likewise
  force the target's intrinsic event-control and one-sample execution shape;
  vector mode is rejected.
- Interpreter helpers now default to `SignalFirLane::TransformFastLane`.
- `WasmArtifactRequest::new(...)` defaults to `SignalFirLane::TransformFastLane`.
- `compile_file_default_to_wasm_artifact(...)` also defaults to
  `SignalFirLane::TransformFastLane`.

## Pipeline

```
parse → eval → propagate → [optional signal→FIR] → codegen (C / C++ / Cmajor / Codebox / Rust / AssemblyScript / .fbc / Cranelift / WASM / Julia / JSON)
```

The public signal->FIR route is:

| Lane | Description |
|---|---|
| `SignalFirLane::TransformFastLane` | `transform::signal_prepare` + `transform::signal_fir` |

## Facade responsibilities

- Provide one orchestrator type (`Compiler`) for source- and file-based compilation.
- Aggregate typed stage errors into one top-level `CompilerError`.
- Provide test/golden-oriented helper outputs (box dump, signal dump, FIR dump).
- Route backend generation to C, C++, Cmajor, Codebox (RNBO), Rust, AssemblyScript,
  Julia, interpreter bytecode, Cranelift JIT status reports, WASM/JSON
  artifacts, and strict JSON emitters with consistent options. Cranelift
  facade methods return a status report rather than a live JIT module; callers
  that need to execute compiled code use the lower-level codegen API or the
  Cranelift FFI. Cranelift is unavailable when `compiler` itself targets
  `wasm32` because that target cannot host its native JIT.
- Apply architecture wrapping for C, C++, Cmajor, and Julia output when `-a` is used.
