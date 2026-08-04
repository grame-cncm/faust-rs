# codegen

Backend code-generation from FIR (Faust Intermediate Representation).

Consumes a `FirStore` produced by the `transform` / `compiler` pipeline and
emits target-language source text, bytecode, or JIT-compiled machine code.
All backend option structs, typed errors, and signature-validation helpers
live here.

## Position in the pipeline

```text
parser → boxes → eval → propagate → signals → transform → fir → [codegen]
                                                                → AssemblyScript source
                                                                → C source
                                                                → C++ source
                                                                → Codebox (RNBO) source
                                                                → Rust source
                                                                → .fbc bytecode
                                                                → native C++ (AOT from .fbc)
                                                                → Cranelift JIT
                                                                → WASM binary/WAT + JSON
                                                                → Julia source
                                                                → … (scaffolded)
```

## C++ provenance

| Rust path | C++ origin |
|---|---|
| `backends::asc` | `compiler/generator/asc/` |
| `backends::c` | `compiler/generator/c/` |
| `backends::cpp` | `compiler/generator/cpp/` |
| `backends::codebox` | `compiler/generator/codebox/` |
| `backends::cranelift` | *(new — no C++ equivalent)* |
| `backends::interp` | `compiler/generator/interpreter/` |
| `backends::julia` | `compiler/generator/julia/` |
| `backends::rust` | `compiler/generator/rust/` |
| `backends::wasm` | `compiler/generator/wasm/` + `code_container.hh` JSON path |
| Other backends | `compiler/generator/<backend>/` *(planned)* |

---

## Backend status

| Backend | Status | Entry point |
|---|---|---|
| `asc` | ✅ Implemented | `generate_asc_module` |
| `c` | ✅ Implemented | `generate_c_module` |
| `cpp` | ✅ Implemented | `generate_cpp_module` |
| `cranelift` | 🔧 Bring-up | `generate_cranelift_module` |
| `interp` | ✅ Implemented | `generate_interp_module` |
| `julia` | 🔧 Bring-up | `generate_julia_module` |
| `rust` | ✅ Implemented | `generate_rust_module` |
| `interp::fbc_to_cpp` | ✅ Implemented | `generate_cpp_from_fbc` |
| `wasm` | 🔧 Bring-up | `generate_wasm_module` |
| `codebox` | 🔧 Implemented; RNBO validation pending | `generate_codebox_module` |
| `cmajor` | 🔧 Scalar bring-up | `generate_cmajor_module` |
| `csharp` | 🗂 Scaffolded | — |
| `dlang` | 🗂 Scaffolded | — |
| `jax` | 🗂 Scaffolded | — |
| `jsfx` | 🗂 Scaffolded | — |
| `llvm` | 🗂 Scaffolded | — |
| `sdf3` | 🗂 Scaffolded | — |
| `vhdl` | 🗂 Scaffolded | — |

---

## Public API

### AssemblyScript backend — `backends::asc`

Emits an `export class <name>` TypeScript/AssemblyScript module with the full
Faust DSP lifecycle (`instanceInit`, `instanceResetUserInterface`,
`instanceClear`, `compute`). Instance state is addressed as `this.<field>`;
static struct fields as `<ClassName>.<field>`. Arrays are `StaticArray<T>`,
numeric literals are cast-wrapped (`<i32>(n)`, `<f32>(n)`, `<f64>(n)`), and
math routes through `Math.*` / `Mathf.*`. UI/soundfile nodes are lowered to
comments (parity with the C++ `asc` backend). An optional embedded
`getJSON(): string` method is emitted when `AscOptions::json` is provided.

```rust
use codegen::backends::asc::{AscOptions, generate_asc_module};

let opts = AscOptions {
    class_name: Some("mydsp".to_owned()),
    json: Some(dsp_json_string),
    ..Default::default()
};
let asc_source = generate_asc_module(&store, root_id, &opts)?;
```

| Item | Description |
|---|---|
| `AscOptions` | `class_name`, `double_precision`, `quad_type_name`, `fixed_type_name`, `json` |
| `generate_asc_module` | `(&FirStore, FirId, &AscOptions) → Result<String, CodegenError>` |
| `CodegenError` | Codes `FRS-CGEN-ASC-0001..0003` |

CLI entry point lives in `compiler`:

```sh
cargo run -p compiler -- --lang asc my.dsp -o mydsp.ts
```

---

### C backend — `backends::c`

Emits a C header with a `typedef struct` DSP state container and the full
Faust C-style functional API (`new*`, `delete*`, `init*`, `buildUserInterface*`,
`compute*`, `metadata*`).

```rust
use codegen::backends::c::{COptions, generate_c_module};

let opts = COptions {
    class_name: Some("mydsp".to_owned()),
    ..Default::default()
};
let c_source = generate_c_module(&store, root_id, &opts)?;
```

| Item | Description |
|---|---|
| `COptions` | `class_name`, `quad_type_name`, `fixed_type_name` |
| `generate_c_module` | `(&FirStore, FirId, &COptions) → Result<String, CodegenError>` |
| `CodegenError` | Codes `FRS-CGEN-C-0001..0003` |

---

### C++ backend — `backends::cpp`

Emits a C++ class (`class <name> : public dsp`) with the full Faust
object-oriented lifecycle.

```rust
use codegen::backends::cpp::{CppOptions, generate_cpp_module};

let opts = CppOptions {
    class_name: Some("MySynth".to_owned()),
    namespace: Some("faust".to_owned()),
    ..Default::default()
};
let cpp_source = generate_cpp_module(&store, root_id, &opts)?;
```

| Item | Description |
|---|---|
| `CppOptions` | `class_name`, `namespace`, `super_class_name`, `quad_type_name`, `fixed_type_name` |
| `generate_cpp_module` | `(&FirStore, FirId, &CppOptions) → Result<String, CodegenError>` |
| `CodegenError` | Codes `FRS-CGEN-CPP-0001..0003` |

---

### Cranelift backend — `backends::cranelift`

JIT-compiles a FIR module to native machine code via Cranelift. Prioritizes
compile-path coverage and diagnosability; falls back to a no-op `compute` stub
for FIR nodes outside the current lowering subset. This backend is available
only on native targets; a WebAssembly-hosted compiler cannot create a native
JIT.

```rust
use codegen::backends::cranelift::{CraneliftOptions, generate_cranelift_module};

let opts = CraneliftOptions::default(); // opt_level: Speed
let jit = generate_cranelift_module(&store, root_id, &opts)?;
```

| Item | Description |
|---|---|
| `CraneliftOptions` | Optimization, target/debug, strict-subset, external data/function symbol, and precision settings |
| `CraneliftOptLevel` | `None`, `Speed` (default), `SpeedAndSize` |
| `generate_cranelift_module` | Main entry point; returns compiled JIT module |
| `diagnose_cranelift_compute_subset_gap` | Reports unsupported FIR nodes |

---

### Interpreter backend — `backends::interp`

Compiles FIR to Faust Bytecode (FBC), runs it in a stack-machine interpreter,
and serializes/deserializes `.fbc` text files. Also includes an AOT C++
emitter (see below).

#### FIR → FBC pipeline

1. `FirToFbcCompiler<R>` — compiles each FIR function body into a shared
   `FbcBlockArena`.
2. `generate_interp_module` — maps the FIR DSP lifecycle functions into
   `FbcDspFactory` code blocks, splitting `compute` into `compute_block` and
   `compute_dsp_block` when possible.
3. `FbcDspFactory::optimize(level)` — runs peephole bytecode optimizer
   (levels 0–6; `MAX_OPT_LEVEL = 6`).
4. `write_fbc` / `read_fbc` — serialize/deserialize to/from `.fbc` text.
5. `FbcDspInstance` — in-process DSP runtime (`init`, `compute`).

```rust
use codegen::backends::interp::{InterpOptions, generate_interp_module, write_fbc};

let opts = InterpOptions { opt_level: 4, module_name: None };
let factory = generate_interp_module::<f32>(&store, root_id, &opts)?;
let mut buf = Vec::new();
write_fbc(&factory, &mut buf)?;
```

#### Function-to-block mapping

| FIR function name | `FbcDspFactory` block slot |
|---|---|
| `"staticInit"` | `static_init_block` |
| `"instanceConstants"` | `init_block` |
| `"instanceResetUserInterface"` | `reset_ui_block` |
| `"instanceClear"` | `clear_block` |
| `"compute"` | `compute_dsp_block` or `compute_block` + `compute_dsp_block` |

#### Key re-exports

| Item | Description |
|---|---|
| `InterpOptions` | `opt_level: i32`, `module_name: Option<String>` |
| `generate_interp_module` | `(&FirStore, FirId, &InterpOptions) → Result<FbcDspFactory<f32>, CodegenError>` |
| `FbcDspFactory<R>` | Compiled bytecode program with lifecycle/data blocks |
| `FbcDspInstance` | Runtime DSP state; provides `init` and `compute` |
| `FbcBlockArena` | Arena of `FbcBlock`s indexed by `BlockId` |
| `FbcInstruction<R>` | Single FBC instruction (`opcode + offsets + branches`) |
| `FbcOpcode` | 298-variant enum of all interpreter opcodes |
| `FbcReal` | Trait for `f32`/`f64` dispatch |
| `write_fbc` / `read_fbc` | `.fbc` text serialization |
| `optimize_block` | Peephole optimizer |
| `MAX_OPT_LEVEL` | Maximum optimizer level (`6`) |
| `INTERP_FILE_VERSION` | Current `.fbc` format version |
| `FbcCppOptions` | Options for the AOT C++ generator |
| `generate_cpp_from_fbc` | AOT C++ generator entry point |

---

### AOT C++ generator — `backends::interp::fbc_to_cpp`

Reads an `FbcDspFactory<R>` (from `generate_interp_module` or `read_fbc`)
and emits a **self-contained C++ header** — no interpreter runtime dependency
at the output side.

The generator performs a single pass over each of the 6 code blocks,
maintaining a **virtual stack** of named C++ temporaries (`fRN` for reals,
`iIN` for integers). All 298 FBC opcodes are covered.

#### Control-flow translation

| FBC instruction | Generated C++ |
|---|---|
| `Loop(init, body)` | `{ <init>; while (true) { <body> } }` |
| `CondBranch` | `if (!<cond>) { break; }` |
| `If(b1, b2)` | `if (<cond>) { <b1> } else { <b2> }` |
| `SelectReal/Int(b1, b2)` | pre-declared merge variable + `if/else` |
| `Return` | end of block (no explicit `return` emitted) |

#### Generated class structure

```cpp
class MySynth final : public dsp {
    int   iVec[<int_heap_size>];
    float fVec[<real_heap_size>];
    int   fSampleRate;
public:
    void classInit(int sample_rate);             // static_init_block
    void instanceConstants(int sample_rate) override; // init_block
    void instanceResetUserInterface() override;       // reset_ui_block
    void instanceClear() override;                    // clear_block
    void instanceInit(int sample_rate) override;      // orchestrates the above
    void init(int sample_rate) override;
    void buildUserInterface(UI* ui_interface) override;
    void metadata(Meta* m) override;
    void compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs) override;
    dsp* clone() override;
    int  getNumInputs() override;
    int  getNumOutputs() override;
    int  getSampleRate() override;
};
```

#### Usage

```rust
use codegen::backends::interp::{
    FbcCppOptions, generate_cpp_from_fbc, read_fbc,
};
use std::io::BufReader;

// From a .fbc file:
let text = std::fs::read_to_string("my.fbc")?;
let factory = read_fbc::<f32>(&mut BufReader::new(text.as_bytes()))?;

let opts = FbcCppOptions {
    class_name: Some("MySynth".to_owned()),
    pragma_once: true,
    namespace: Some("faust".to_owned()),
};
let header = generate_cpp_from_fbc(&factory, &opts)?;
std::fs::write("my.h", header)?;
```

Or directly from the CLI:

```sh
# Step 1 — compile .dsp to .fbc
cargo run -p compiler -- --lang interp my.dsp -o my.fbc

# Step 2 — emit native C++ from .fbc
cargo run -p compiler -- --dump-cpp-from-fbc my.fbc -o my.h
```

| Item | Description |
|---|---|
| `FbcCppOptions` | `class_name`, `pragma_once`, `namespace` |
| `FbcCppError` | `MissingBranchTarget`, `InvalidBlockId`, `Unsupported` |
| `generate_cpp_from_fbc` | `(&FbcDspFactory<R>, &FbcCppOptions) → Result<String, FbcCppError>` |

---

### Julia backend — `backends::julia`

Lowers a FIR module to Faust-style Julia source. The current backend slice
emits the standard Julia DSP shell (`mutable struct mydsp{T} <: dsp`),
lifecycle/API methods, UI/metadata calls, and `compute!` over
`Matrix{FAUSTFLOAT}` input/output buffers.

```rust
use codegen::backends::julia::{JuliaOptions, JuliaRealType, generate_julia_module};

let opts = JuliaOptions {
    class_name: Some("mydsp".to_owned()),
    real_type: JuliaRealType::Float64,
};
let julia = generate_julia_module(&store, root_id, &opts)?;
std::fs::write("mydsp.jl", julia)?;
```

Important emitter rules:

- Julia table/vector indexing is one-based only at the final access boundary;
  FIR loop variables and offsets remain Faust/C-style zero-based internally.
- Real casts inside parametric DSP methods emit `T(...)`.
- `Int32` casts use `faust_wrap_int32(...)` to preserve C++-style wrapping
  instead of Julia `InexactError`.
- The generated source assumes the host provides the Faust Julia runtime names
  (`dsp`, `UI`, `FMeta`, `FAUSTFLOAT`, and UI callback functions).

| Item | Description |
|---|---|
| `JuliaOptions` | `class_name`, `real_type` |
| `JuliaRealType` | `Float32` (default) or `Float64` |
| `generate_julia_module` | `(&FirStore, FirId, &JuliaOptions) -> Result<String, CodegenError>` |
| `CodegenError` | Codes `FRS-CGEN-JULIA-0001..0003` |

CLI entry point lives in `compiler`:

```sh
cargo run -p compiler -- --lang julia my.dsp -o mydsp.jl
cargo run -p compiler -- --lang julia -double my.dsp -o mydsp.jl
```

---

### Rust backend — `backends::rust`

Emits Faust-compatible Rust source using the host-provided `F32`, `F64`,
`FaustFloat`, `ParamIndex`, `UI`, `Meta`, and `FaustDsp` contracts. The output
is intended for inclusion in a Faust Rust architecture rather than as a
standalone private runtime.

```rust
use codegen::backends::rust::{RustOptions, RustRealType, generate_rust_module};

let opts = RustOptions {
    class_name: Some("mydsp".to_owned()),
    faust_float_type: RustRealType::Float32,
};
let rust_source = generate_rust_module(&store, root_id, &opts)?;
```

| Item | Description |
|---|---|
| `RustOptions` | `class_name`, `faust_float_type` |
| `RustRealType` | `Float32` (default) or `Float64` |
| `generate_rust_module` | `(&FirStore, FirId, &RustOptions) -> Result<String, CodegenError>` |
| `CodegenError` | Codes `FRS-CGEN-RUST-0001..0003` |

CLI entry point:

```sh
cargo run -p compiler -- --lang rust my.dsp -o mydsp.rs
```

---

### WASM backend — `backends::wasm`

Lowers a FIR module to a binary `.wasm` artifact plus the matched companion
Faust JSON description consumed by web-facing runtimes. The backend preserves
the canonical exported DSP entry points (`init`, `compute`, `instance*`,
`getNumInputs`, `getNumOutputs`, `getSampleRate`, `getParamValue`,
`setParamValue`) and threads UI metadata through the same runtime memory
layout used by the generated module.

```rust
use codegen::backends::wasm::{WasmOptions, generate_wasm_module};

let opts = WasmOptions {
    double_precision: false,
    ..Default::default()
};
let wasm = generate_wasm_module(&store, root_id, &opts)?;

std::fs::write("mydsp.wasm", &wasm.wasm_binary)?;
std::fs::write("mydsp.json", &wasm.dsp_json)?;
```

For callers that already know source-level provenance to embed in the JSON
companion:

```rust
use codegen::backends::wasm::{
    WasmJsonContext, WasmOptions, generate_wasm_module_with_context,
};

let wasm = generate_wasm_module_with_context(
    &store,
    root_id,
    &WasmOptions::default(),
    &WasmJsonContext {
        filename: Some("mydsp.dsp".to_owned()),
        compile_options: Some("-lang wasm".to_owned()),
        ..Default::default()
    },
)?;
```

#### Runtime contract

`WasmModule::wasm_binary`, `WasmModule::dsp_json`, and
`WasmModule::memory_layout` describe the same module instance and must be kept
together. In particular:

- JSON widget `index` values are raw byte offsets into the runtime prefix.
- `getParamValue(dsp, index)` / `setParamValue(dsp, index, value)` consume
  those exact offsets.
- JSON `size` matches the runtime prefix size before the audio I/O zone.
- When persisting the backend output, write the `.wasm` and `.json` from the
  same compilation result.

#### Memory layout

`backends::wasm::layout::WasmMemoryLayout` exposes the current linear-memory
contract shared by code generation and companion JSON:

- static tables first,
- mutable DSP/global fields next,
- then the I/O zone / audio heap start,
- then the embedded JSON segment.

This is the source of truth for exported UI offsets and host-side parameter
access.

| Item | Description |
|---|---|
| `WasmOptions` | `double_precision`, `emit_wat`, `memory_pages`, `internal_memory` |
| `WasmJsonContext` | JSON-only provenance: `filename`, `version`, `compile_options`, include/library lists, top-level metadata |
| `WasmModule` | `wasm_binary`, `wat_text`, `dsp_json`, `memory_layout` |
| `generate_wasm_module` | `(&FirStore, FirId, &WasmOptions) -> Result<WasmModule, WasmBackendError>` |
| `generate_wasm_module_with_context` | Same as above, plus `&WasmJsonContext` |
| `WasmBackendError` | Codes `FRS-CGEN-WASM-0001..0005` |
| `WasmMemoryLayout` | Runtime prefix / I/O zone / JSON placement descriptor |

`emit_wat` and `WasmModule::wat_text` are retained in the Rust API but are not
populated by the binary emitter yet. The compiler CLI's `-lang wast` path
converts the emitted binary module to text separately.

CLI entry points live in `compiler`:

```sh
# Emit binary WASM (+ companion JSON next to the output path)
cargo run -p compiler -- --lang wasm my.dsp -o mydsp.wasm

# Emit WAST text from the same backend
cargo run -p compiler -- --lang wast my.dsp -o mydsp.wat
```

---

### Codebox backend — `backends::codebox`

Emits a flat RNBO `codebox~` source file from a FIR module. The output uses
`dspsetup`, `control`, `update`, and a per-sample `compute` function; bargraphs
are appended as extra output channels because codebox cannot report them as
controls. The compiler facade forces the external-control and one-sample
lowering modes this target requires, while vector mode is unsupported.

`CodeboxOptions::test_labels` selects the `RB_` parameter-name convention used
by `-lang codebox-test` and Faust's `rnbo-dsp.h` wrapper. It is intended for
manual RNBO round-trip validation; RNBO itself is not bundled with the
workspace.

```rust
use codegen::backends::codebox::{CodeboxOptions, generate_codebox_module};

let source = generate_codebox_module(&store, root_id, &CodeboxOptions::default())?;
```

| Item | Description |
|---|---|
| `CodeboxOptions` | `double_precision`, `test_labels` |
| `generate_codebox_module` | `(&FirStore, FirId, &CodeboxOptions) -> Result<String, CodegenError>` |
| `CodegenError` | Codes `FRS-CGEN-CBOX-0001..0002` |
| `eval::Program` | Parser/evaluator for the emitted subset, used by numeric backend tests |

CLI entry points live in `compiler`:

```sh
cargo run -p compiler -- --lang codebox my.dsp -o mydsp.codebox
cargo run -p compiler -- --lang codebox-test my.dsp -o mydsp.codebox
```

---

### Cmajor backend — `backends::cmajor`

Emits a Cmajor `processor` from FIR lowered for external control and the
one-sample API. The scalar core owns Cmajor streams, fields, lifecycle methods,
math spelling, scalar control flow, delay arrays, and the forever-running
`main` loop. Its lifecycle follows the shared faust-rs contract:
`init = classInit -> instanceInit`; direct `instanceInit` does not call
`classInit`.

The backend currently exposes its codegen API while compiler-facade/CLI, UI
events, bargraphs, table specialization, and external Cmajor validation are
completed in the staged plan. Unsupported types and nodes return stable typed
errors rather than partial source.

```rust
use codegen::backends::cmajor::{CmajorOptions, generate_cmajor_module};

let source = generate_cmajor_module(&store, root_id, &CmajorOptions::default())?;
```

| Item | Description |
|---|---|
| `CmajorOptions` | public processor name and `CmajorRealType` |
| `generate_cmajor_module` | `(&FirStore, FirId, &CmajorOptions) -> Result<String, CodegenError>` |
| `CodegenError` | Codes `FRS-CGEN-CMAJ-0001..0005` |

Port and test contract:
`porting/cmajor-backend-port-and-test-plan-2026-08-04-en.md`.

---

### Fixtures — `fixtures`

Shared FIR modules for backend-agnostic parity testing. All backends are
exercised against the same 8 canonical fixtures, preventing test drift.

```rust
use codegen::fixtures::backend_test_fixtures;

for (name, build) in backend_test_fixtures() {
    let (store, root) = build();
    // run backend against (store, root) …
}
```

| Fixture name | What it covers |
|---|---|
| `"sine_phasor"` | Phasor-driven sine oscillator, UI controls, persistent state |
| `"heavy_bench"` | Stress test for backend coverage |
| `"passthrough"` | Minimal audio pass-through |
| `"gain_bias_ui_meta"` | Gain/bias with UI and `metadata` |
| `"table_state_delay"` | Table initialization and stateful delay |
| `"control_flow"` | Conditional branching and loops |
| `"math_intrinsics"` | Mathematical function coverage |
| `"ir_coverage"` | Low-level FIR node coverage |

---

## Scaffolded backends

The following backends expose a stable `backend_id()` identifier and are
otherwise empty. They reserve a place in the roadmap and prevent accidental
namespace collisions as parity work proceeds.

`csharp` · `dlang` · `jax` · `jsfx` · `llvm` · `sdf3` · `vhdl`
