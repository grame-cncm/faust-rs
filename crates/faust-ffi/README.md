# faust-ffi

Unified C/C++ FFI distribution crate — owns the canonical `libfaust-rs` artifacts.

This crate links `interp-ffi`, `cranelift-ffi`, `box-ffi`, `signal-ffi`, and
`libfaust-ffi` as Rust libraries and distributes their exported `extern "C"` symbols through a
single top-level `staticlib` + `cdylib`.

## Public API

| Re-export | Source crate | Description |
|---|---|---|
| `box_api` | `box_ffi` (`box-ffi`) | Box manipulation C and C++ API |
| `signal_api` | `signal_ffi` (`signal-ffi`) | Signal manipulation C and C++ API |
| `cranelift` | `cranelift_ffi` (`cranelift-ffi`) | Cranelift JIT backend C and C++ API |
| `interp` | `interp_ffi` (`interp-ffi`) | Interpreter backend C and C++ API |
| `libfaust` | `libfaust_ffi` (`libfaust-ffi`) | Backend-agnostic libfaust C and C++ API (`expandDSP*`, `generateAuxFiles*`, `generateSHA1`) |

For per-backend API details, see each backend crate's README.

## Errors

Every entry point that reports through a 4096-byte `error_msg` buffer has the
complete text of its error (the message uncut, then the compiler's rendered
diagnostics when there are some) behind one function per API, all five under
the same contract: per thread, owned by the library, null before the thread's
first error, valid until its next one, not reset by a success.

| API | Complete text | Typed form (diagnostics-v2 JSON) |
|---|---|---|
| Interpreter | `getCCompleteInterpreterDSPFactoryError()` | `getCInterpreterDSPFactoryErrorDiagnostics()` |
| Cranelift | `getCCompleteCraneliftDSPFactoryError()` | `getCCraneliftDSPFactoryErrorDiagnostics()` |
| libfaust (`expandCDSP*`, `generateCAuxFiles*`) | `getCCompleteDSPError()` | `getCDSPErrorDiagnostics()` |
| Box | `getCCompleteBoxError()` | `getCBoxErrorDiagnostics()` |
| Signal | `getCCompleteSignalError()` | none: no failure of this API is typed |

The typed form is null when the last error carried no typed diagnostics, even
if an earlier one did.

The C++ wrappers that take a `std::string& error_msg` read it themselves. See
[Compile errors](../../README.md#compile-errors) in the workspace guide.

## Build

```bash
cargo run -p xtask -- build-libfaust --release
```

The packaging command produces `libfaust-rs.a` plus the platform dynamic
library (`libfaust-rs.dylib`, `libfaust-rs.so`, or `faust-rs.dll`) under
`target/release/`. The maintained C and C++ headers remain in the source FFI
crates:

- `../interp-ffi/include/interpreter-dsp-c.h` and `interpreter-dsp.h`
- `../cranelift-ffi/include/cranelift-dsp-c.h` and `cranelift-dsp.h`
- `../box-ffi/include/libfaust-box-c.h` and `libfaust-box.h`
- `../signal-ffi/include/libfaust-signal-c.h` and `libfaust-signal.h`
- `../libfaust-ffi/include/libfaust-c.h` and `libfaust.h`

See the workspace [C and C++ usage guide](../../README.md#use-libfaust-rs-from-c-and-c)
for complete Interpreter and Cranelift lifecycle examples.
