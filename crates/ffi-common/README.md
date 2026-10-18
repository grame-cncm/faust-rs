# ffi-common

Internal, dependency-light support for Rust-side Faust FFI adapters.

Provides common C ABI types (`UIGlue`, `MetaGlue`), heap allocation helpers,
error buffer writing, and CLI argument parsing used by `interp-ffi`,
`cranelift-ffi`, `box-ffi`, `signal-ffi`, `tree-ffi`, `libfaust-ffi`,
`wasm-ffi`, and the `faust` Rust API.

This is not a general-purpose utility crate. Compiler-core crates must not
depend on it.

## Modules

| Module | Responsibility |
|---|---|
| `abi` | Shared `#[repr(C)]` callback tables, `FAUSTFLOAT` type, and the versioned custom memory-manager ABI (`FaustMemoryManager`, `FAUST_MEMORY_MANAGER_ABI_VERSION`; C header `include/faust-memory-manager.h`) |
| `args` | FFI host options accepted at FFI entry points |
| `complete_error` | Per-thread complete text of the last error, beyond the 4096-byte `error_msg` buffer |
| `control_range` | `ControlRange`: a control's declared range at its compiled precision, for Rust callers (not part of the C ABI) |
| `factory_cache` | Owned, reference-counted factory and DSP-instance cache |
| `memory` | Opaque Rust allocation helpers |
| `sha1` | `sha1` / `sha1_hex` digests used for factory SHA keys |
| `strings` | C strings, `argv`, error buffers, and empty `char**` support |

## Public API

### C ABI types

| Item | Description |
|---|---|
| `UIGlue` | C-ABI UI callback table (mirrors Faust `UIGlue`) |
| `MetaGlue` | C-ABI metadata callback table (mirrors Faust `MetaGlue`) |
| `FfiFaustFloat` | `FAUSTFLOAT` type alias (`f32`) used by FFI exports |

### Allocation helpers

| Function | Description |
|---|---|
| `alloc_c_string(s)` | Allocate a heap C string (NUL bytes escaped as `\\0`) |
| `alloc_opaque(value)` | Box a value and return an owning raw pointer |
| `free_c_string(ptr)` | Free a pointer returned by `alloc_c_string` |
| `free_opaque<T>(ptr)` | Free a pointer returned by `alloc_opaque` |
| `free_c_memory_c_string_only(ptr)` | Common `freeCMemory` behavior for C-string pointers |
| `null_c_string_array()` | Static null-terminated empty `char**` array pointer |

### FFI utilities

| Function | Description |
|---|---|
| `write_error_4096(buf, msg)` | Write error message into a 4096-byte Faust error buffer |
| `decode_c_argv(argc, argv)` | Decode a C `argv` array into a `Vec<String>` |
| `required_c_string_arg(ptr, label)` | Copy a required C string argument into an owned `String` |
| `optional_c_string_arg(ptr, label)` | Copy an optional C string argument into an owned `Option<String>` |

### Complete error text

`error_msg` is caller-allocated and its size is never passed, so
`write_error_4096` truncates and the buffer cannot grow. `CompleteError` holds
what each adapter returns from its `getCComplete*Error` entry point. One
`thread_local!` record per adapter:

| Method | Description |
|---|---|
| `CompleteError::new()` | Empty record, usable as a `thread_local!` const initializer |
| `attach(summary, details)` | Where a typed compiler error is flattened to its summary: keep its rendered diagnostics for the report of that summary |
| `report(message)` | In the adapter's `write_error` funnel: publish `message`, followed by the attached diagnostics when it carries their summary; they are dropped either way, so they never reach another error. The text never ends with a newline |
| `as_ptr()` | The published text or null, valid until the next `report` on this thread |
| `attach_with_diagnostics(summary, details, json)` | `attach` for a typed failure: `json` is its diagnostics-v2 report (`CompilerError::diagnostics_report_json`), published with the text by the report of `summary` |
| `diagnostics_ptr()` | The published report or null, what an adapter returns from its `get...ErrorDiagnostics` entry point. A `report` of an error that has none **clears** it: a document never outlives the failure it describes |
| `diagnostics()` | The same, as an owned `String` for a Rust caller |

### Compile arguments

| Item | Description |
|---|---|
| `FfiCompileArgs` | FFI host options: `-I`, `-cn`, `-mem0` and its aliases, and `--warn` (program options such as `-double` or `-vec` are read by `compiler::CompileOptionArgs::from_argv`) |
| `parse_ffi_compile_args(argv)` | Parse a string slice into `FfiCompileArgs` |

### Factory caching

| Item | Description |
|---|---|
| `FactoryCache<T, I>` | Thread-safe SHA-keyed owner of factories and their DSP instances |
| `FactoryHandle<T>` | Typed opaque pointer identity used at backend ABI edges |
| `FactoryRelease` | `NotFound`, `Retained`, or final `Removed` release result |

The crate re-exports the module APIs at its root to keep the FFI adapter call
sites compact. Creation and SHA lookup acquire a reference; final release
drops remaining instances before their parent factory. Backend-specific
construction and runtime behavior remain in the owning backend crate.
