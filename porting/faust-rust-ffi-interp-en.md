# Plan: C/C++ Export of the FBC Interpreter from Rust

> Date: 2026-02-22
> Branch: `signals-after-deBruijn2Sym`
> Scope: `interpreter_dsp` / `interpreter_dsp_factory` API only (no Signal/Box API)

---

## Overview

The goal is to compile the Rust interpreter backend (`crates/codegen/src/backends/interp`)
into both a **static library** (`.a`/`.lib`) and a **dynamic library** (`.so`/`.dylib`/`.dll`)
exposing a C-compatible API mirroring the official Faust interpreter headers
`interpreter-dsp.h` / `interpreter-dsp-c.h`.

```
┌──────────────────────────────────────────────────────┐
│                   C/C++ Application                   │
│  #include "interpreter-dsp.h"  (C++)                 │
│  #include "interpreter-dsp-c.h"  (C)                 │
└──────────────┬───────────────────────────────────────┘
               │  link
┌──────────────▼───────────────────────────────────────┐
│   libfaust.a / libfaust.dylib                         │
│              (crates/interp-ffi)                      │
│   extern "C" FFI layer (Rust)                        │
└──────────────┬───────────────────────────────────────┘
               │  depends on
┌──────────────▼───────────────────────────────────────┐
│   crates/codegen  →  backends/interp                  │
│   FbcDspFactory<f32>  /  FbcDspInstance               │
│   serial::read_fbc / write_fbc                        │
└──────────────────────────────────────────────────────┘
```

---

## Constraints and Design Decisions

### 1. `unsafe_code = "forbid"` in the workspace

The workspace forbids unsafe code. The new crate `crates/interp-ffi` must
**override** this rule locally in its own `Cargo.toml`. Since Cargo does not
allow combining `workspace = true` with lint overrides, all workspace lints
are reproduced manually with `unsafe_code = "allow"`:

```toml
[lints.rust]
unsafe_code = "allow"           # FFI requires raw pointer operations
unused_attributes = "warn"
unused_lifetimes = "warn"
[lints.clippy]
all = "warn"
```

### 2. Lifetime of `FbcDspInstance<'a, R>`

`FbcDspInstance` borrows `&'a FbcDspFactory<R>`. This is incompatible with
the C opaque pointer model where factory and instance are independent pointers.

**Chosen solution**: the FFI crate defines its own wrapper types that replicate
the execution logic directly using the public fields of `FbcDspFactory`.
This avoids modifying existing types and respects "don't change code not
directly impacted".

```
InterpreterDspFactory  =  Box<FbcDspFactory<f32>>  (owned, heap-allocated)
InterpreterDspInstance =  struct { factory: *const InterpreterDspFactory,
                                    executor: FbcExecutor<f32>,
                                    initialized: bool, cycle: usize }
```

The factory **must** outlive all its instances (same contract as the C++ Faust API).

### 3. FAUSTFLOAT type

`FAUSTFLOAT` is `float` (f32) by default in Faust. The FFI crate exports only
the `f32` variant (`FbcReal` = `f32`). The `f64` variant can be added later.

### 4. Scope of exported functions

| Group | Available | Reason |
|-------|-----------|--------|
| `readInterpreterDSPFactoryFromBitcode[File]` | ✅ | `read_fbc` available |
| `writeInterpreterDSPFactoryToBitcode[File]` | ✅ | `write_fbc` available |
| `createInterpreterDSPFactoryFromFile/String` | ❌ | Compiler pipeline incomplete |
| `createInterpreterDSPFactoryFromSignals/Boxes` | ❌ | Signal/Box API out of scope |
| Instance lifecycle (`init`, `compute`, etc.) | ✅ | `FbcDspInstance` complete |
| Global SHA cache | ✅ (partial) | `HashMap` + `Mutex` |
| `startMTDSPFactories` / `stopMTDSPFactories` | ✅ | `AtomicBool` flag |
| `buildUserInterface` (`UIGlue`) | ✅ | `ui_block` public fields |
| `metadata` (`MetaGlue`) | ✅ | `meta_block` public fields |

### 5. cbindgen limitation

cbindgen (0.27) does not support Rust edition 2024's `#[unsafe(no_mangle)]`
attribute. The `include/interpreter-dsp-c.h` header is therefore **written
manually**. The `build.rs` and `cbindgen.toml` are kept for future use.

---

## Step 1 — Create `crates/interp-ffi`

### 1.1 File structure

```
crates/interp-ffi/
├── Cargo.toml                  ← cdylib + staticlib, no workspace lints inheritance
├── build.rs                    ← creates include/ directory, rerun-if-changed
├── cbindgen.toml               ← future use (when cbindgen supports unsafe no_mangle)
├── src/
│   ├── lib.rs                  ← entry point, module declarations
│   ├── types.rs                ← opaque FFI types, UIGlue, MetaGlue, alloc helpers
│   ├── cache.rs                ← global factory cache (LazyLock<Mutex<HashMap>>)
│   ├── ui.rs                   ← UIGlue / MetaGlue dispatch helpers
│   ├── factory.rs              ← factory extern "C" functions
│   └── instance.rs             ← instance extern "C" functions
└── include/
    ├── interpreter-dsp-c.h     ← C API (written manually)
    └── interpreter-dsp.h       ← C++ wrapper classes (written manually)
```

### 1.2 Library name

```toml
[lib]
name = "faust"
crate-type = ["cdylib", "staticlib"]
```

Produces `libfaust.a` and `libfaust.dylib` (macOS) / `libfaust.so` (Linux).

---

## Step 2 — FFI opaque types (`src/types.rs`)

```rust
// Opaque factory wrapper — owned by the Rust heap
pub struct InterpreterDspFactory {
    pub(crate) inner: FbcDspFactory<f32>,
}

// Opaque instance — holds a non-owning raw pointer to the factory
pub struct InterpreterDspInstance {
    pub(crate) factory: *const InterpreterDspFactory,
    pub(crate) executor: FbcExecutor<f32>,
    pub(crate) initialized: bool,
    pub(crate) cycle: usize,
}
```

Allocation / deallocation via `Box::into_raw` / `Box::from_raw`.

`UIGlue` and `MetaGlue` are redefined in Rust as `#[repr(C)]` structs
matching the binary layout of `CInterface.h` (field names in snake_case,
same order and types).

---

## Step 3 — Global factory cache (`src/cache.rs`)

```rust
static FACTORY_CACHE: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
```

Factory pointers are stored as `usize` to avoid `*mut T: !Send` issues.
Functions: `cache_insert`, `cache_lookup`, `cache_remove_by_ptr`,
`cache_drain`, `cache_all_sha_keys`.

---

## Step 4 — Factory `extern "C"` functions (`src/factory.rs`)

| C Function | Rust Implementation |
|-----------|---------------------|
| `readCInterpreterDSPFactoryFromBitcode` | `serial::read_fbc` on `BufReader::new(str.as_bytes())` |
| `writeCInterpreterDSPFactoryToBitcode` | `serial::write_fbc` into `Vec<u8>` → `CString::into_raw` |
| `readCInterpreterDSPFactoryFromBitcodeFile` | `File::open` + `BufReader` + `read_fbc` |
| `writeCInterpreterDSPFactoryToBitcodeFile` | `File::create` + `BufWriter` + `write_fbc` |
| `getCInterpreterDSPFactoryFromSHAKey` | `cache_lookup(sha)` |
| `deleteCInterpreterDSPFactory` | `cache_remove_by_ptr` + `Box::from_raw` |
| `deleteAllCInterpreterDSPFactories` | `cache_drain` + drop each `Box` |
| `getAllCInterpreterDSPFactories` | `cache_all_sha_keys` → null-terminated `*mut *mut c_char` |
| `getCInterpreterDSPFactoryJSON` | generate JSON from `meta_block` + `ui_block` |
| `getCCompleteInterpreterDSPFactoryError` | `ffi_common::CompleteError` (per-thread): the `error_msg` text plus the rendered compiler diagnostics, not limited to 4096 bytes; no libfaust equivalent |
| `getCInterpreterDSPFactoryErrorDiagnostics` | the same `CompleteError` record: the typed form of that error, the compiler's complete diagnostics-v2 JSON report (`CompilerError::diagnostics_report_json("interpreter")`), null when the error carried no typed diagnostics; no libfaust equivalent |
| `getCInterpreterDSPFactoryLibraryList` | always returns empty null-terminated array |
| `getCLibFaustVersion` | `FAUST_VERSION` constant via `OnceLock<CString>` |
| `freeCMemory` | `CString::from_raw(ptr)` |
| `startMTDSPFactories` / `stopMTDSPFactories` | `AtomicBool` flag |

Unimplemented constructors (`createFromFile`, `createFromString`) return `null`
and write `"not implemented (full compiler pipeline not available)"` into `error_msg`.

---

## Step 5 — Instance `extern "C"` functions (`src/instance.rs`)

The instance execution logic replicates `FbcDspInstance` semantics directly
using public fields of `FbcDspFactory`, bypassing the borrow-based lifetime.

| C Function | Implementation |
|-----------|----------------|
| `createCInterpreterDSPInstance` | `factory.optimize()` + `FbcExecutor::new(...)` + `alloc_instance` |
| `deleteCInterpreterDSPInstance` | `Box::from_raw` |
| `initCInterpreterDSPInstance` | `initialized = true` + `instanceInit` |
| `instanceInitCInterpreterDSPInstance` | class_init + constants + reset + clear |
| `instanceConstantsCInterpreterDSPInstance` | store `sr` in `int_heap[sr_offset]`, execute `init_block` |
| `instanceResetUserInterfaceCInterpreterDSPInstance` | execute `reset_ui_block` |
| `instanceClearCInterpreterDSPInstance` | execute `clear_block` |
| `cloneCInterpreterDSPInstance` | new `FbcExecutor` + heap copy + same factory pointer |
| `computeCInterpreterDSPInstance` | store count, execute control block, `execute_block_io` |
| `buildUserInterfaceCInterpreterDSPInstance` | `dispatch_ui` from `ui.rs` |
| `metadataCInterpreterDSPInstance` | `dispatch_meta` from `ui.rs` |

### Audio buffer conversion

`FAUSTFLOAT**` → Rust slices:

```rust
let input_slices: Vec<&[f32]> = (0..num_in)
    .map(|i| std::slice::from_raw_parts(*inputs.add(i), count as usize))
    .collect();
let mut output_slices: Vec<&mut [f32]> = (0..num_out)
    .map(|i| std::slice::from_raw_parts_mut(*outputs.add(i), count as usize))
    .collect();
```

### Edition 2024 — `dangerous_implicit_autorefs`

Rust edition 2024 denies implicit autoref through raw pointer dereferences.
All calls to `get` / `get_mut` on `Vec` fields accessed via raw pointer
use explicit `&` / `&mut` with `#[allow(clippy::needless_borrow)]` to satisfy
both the compiler deny lint and clippy.

---

## Step 6 — UI/Meta dispatch (`src/ui.rs`)

`dispatch_ui` iterates `FbcUiInstruction<f32>` and calls the corresponding
`UIGlue` function pointer. Mapping:

| `FbcOpcode` | `UIGlue` field |
|-------------|----------------|
| `OpenTabBox` | `open_tab_box` |
| `OpenHorizontalBox` | `open_horizontal_box` |
| `OpenVerticalBox` | `open_vertical_box` |
| `CloseBox` | `close_box` |
| `AddButton` | `add_button` |
| `AddCheckButton` | `add_check_button` |
| `AddVerticalSlider` | `add_vertical_slider` |
| `AddHorizontalSlider` | `add_horizontal_slider` |
| `AddNumEntry` | `add_num_entry` |
| `AddHorizontalBargraph` | `add_horizontal_bargraph` |
| `AddVerticalBargraph` | `add_vertical_bargraph` |
| `AddSoundfile` | `add_soundfile` |
| `Declare` | `declare` |

`zone` pointers into `UIGlue` callbacks reference `executor.real_heap[instr.offset]`.

---

## Step 7 — C header (`include/interpreter-dsp-c.h`)

Written manually because cbindgen 0.27 does not handle `#[unsafe(no_mangle)]`.

Contains:
- `#ifndef FAUSTFLOAT` / `#define FAUSTFLOAT float`
- `UIGlue` and `MetaGlue` struct declarations (snake_case fields, same binary layout as `CInterface.h`)
- Opaque `typedef struct` for `interpreter_dsp_factory` and `interpreter_dsp`
- All C function declarations with complete documentation

---

## Step 8 — C++ header (`include/interpreter-dsp.h`)

Written manually. Provides `faust_interp::interpreter_dsp_factory` and
`faust_interp::interpreter_dsp` C++ classes wrapping the C API:

```cpp
namespace faust_interp {

class interpreter_dsp_factory {
    ::interpreter_dsp_factory* impl_;
public:
    std::string getJSON() const { ... }
    std::string writeToMemory() const { ... }
    interpreter_dsp* createDSPInstance() { ... }
    // ...
};

class interpreter_dsp {
    ::interpreter_dsp* impl_;
public:
    int  getNumInputs() const { ... }
    void init(int sample_rate) { ... }
    void compute(int n, FAUSTFLOAT** in, FAUSTFLOAT** out) { ... }
    // ...
};

} // namespace faust_interp
```

Free functions (`readInterpreterDSPFactoryFromBitcode`, etc.) are provided
as inline wrappers in the `faust_interp` namespace.

---

## Step 9 — Workspace and build integration

### Workspace `Cargo.toml`

```toml
members = [
  # ... existing members ...
  "crates/interp-ffi",   # ← new
]
```

### Build output

```bash
cargo build -p interp-ffi --release
# Produces:
#   target/release/libfaust.a       (staticlib, 9.0 MB)
#   target/release/libfaust.dylib   (cdylib, 635 KB, macOS)
#   crates/interp-ffi/include/interpreter-dsp-c.h  (manual C header)
#   crates/interp-ffi/include/interpreter-dsp.h    (manual C++ wrapper)
```

---

## Step 10 — Linking from C or C++

### C project

```c
#include "interpreter-dsp-c.h"

int main(void) {
    char err[4096] = {};
    interpreter_dsp_factory* f =
        readCInterpreterDSPFactoryFromBitcodeFile("dsp.fbc", err);
    interpreter_dsp* dsp = createCInterpreterDSPInstance(f);
    initCInterpreterDSPInstance(dsp, 44100);

    float in[512], out[512];
    float* inputs[]  = { in  };
    float* outputs[] = { out };
    computeCInterpreterDSPInstance(dsp, 512, inputs, outputs);

    deleteCInterpreterDSPInstance(dsp);
    deleteCInterpreterDSPFactory(f);
}
```

Compile:
```bash
clang -I crates/interp-ffi/include \
      -L target/release -lfaust \
      test.c -o test
```

### C++ project

```cpp
#include "interpreter-dsp.h"
using namespace faust_interp;

int main() {
    std::string err;
    auto* factory = readInterpreterDSPFactoryFromBitcodeFile("dsp.fbc", err);
    auto* dsp = factory->createDSPInstance();
    dsp->init(44100);

    std::vector<float> in(512, 0.f), out(512, 0.f);
    float* inputs[]  = { in.data()  };
    float* outputs[] = { out.data() };
    dsp->compute(512, inputs, outputs);

    delete dsp;
    deleteInterpreterDSPFactory(factory);
}
```

Compile:
```bash
clang++ -std=c++17 -I crates/interp-ffi/include \
        -L target/release -lfaust \
        test.cpp -o test
```

---

## Complete C → Rust mapping

| C API (official Faust) | Rust implementation (`src/factory.rs` or `instance.rs`) |
|------------------------|--------------------------------------------------------|
| `getCLibFaustVersion()` | `FAUST_VERSION` constant via `OnceLock<CString>` |
| `readCInterpreterDSPFactoryFromBitcode` | `serial::read_fbc` |
| `writeCInterpreterDSPFactoryToBitcode` | `serial::write_fbc` |
| `readCInterpreterDSPFactoryFromBitcodeFile` | `File::open` + `read_fbc` |
| `writeCInterpreterDSPFactoryToBitcodeFile` | `File::create` + `write_fbc` |
| `createCInterpreterDSPFactoryFromFile` | → returns `null` + error message |
| `createCInterpreterDSPFactoryFromString` | → returns `null` + error message |
| `getCInterpreterDSPFactoryFromSHAKey` | `cache::cache_lookup` |
| `deleteCInterpreterDSPFactory` | `types::free_factory` |
| `deleteAllCInterpreterDSPFactories` | `cache::cache_drain` |
| `getAllCInterpreterDSPFactories` | `cache::cache_all_sha_keys` |
| `getCInterpreterDSPFactoryJSON` | generate JSON from `meta_block` + `ui_block` |
| `createCInterpreterDSPInstance` | `factory.optimize()` + `FbcExecutor::new` |
| `deleteCInterpreterDSPInstance` | `Box::from_raw` |
| `initCInterpreterDSPInstance` | execute `init_block`, etc. |
| `computeCInterpreterDSPInstance` | `executor.execute_block_io` |
| `buildUserInterfaceCInterpreterDSPInstance` | iterate `ui_block`, call `UIGlue` |
| `metadataCInterpreterDSPInstance` | iterate `meta_block`, call `MetaGlue.declare` |
| `startMTDSPFactories` | set `AtomicBool::MT_MODE = true` |
| `stopMTDSPFactories` | set `AtomicBool::MT_MODE = false` |
| `freeCMemory` | `drop(CString::from_raw(ptr))` |

---

## Notes and known limitations

1. **Memory management**: strings returned by `write*` and `get*JSON` functions
   are allocated with `CString::into_raw()` and must be freed with `freeCMemory`.
   For `char**` arrays, free each element first, then the array pointer.

2. **Thread safety**: the factory cache uses `std::sync::Mutex`. DSP instances
   are not thread-safe by design (same semantics as the Faust C++ API).

3. **Version**: `getCLibFaustVersion()` returns `"2.84.5"` from `serial::FAUST_VERSION`.

4. **cbindgen**: disabled for now due to lack of `#[unsafe(no_mangle)]` support
   in cbindgen 0.27. Will be re-enabled when cbindgen supports Rust edition 2024.

5. **Missing compiler pipeline**: `createFromFile` and `createFromString` return
   `null` with an explanatory error message. They will be implemented when the
   full Faust compiler pipeline is integrated.

6. **No `f64` variant**: only `f32` (FAUSTFLOAT) is exported. The generic
   `FbcDspFactory<f64>` can be exported via a separate API suffix in the future.
