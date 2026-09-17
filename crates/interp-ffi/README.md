# interp-ffi

C/C++ FFI export crate for the Faust interpreter backend (`interpreter_dsp` family).

## Purpose

This crate exposes a Faust-compatible C API and C++ wrapper API backed by the
Rust interpreter codegen/runtime (`codegen::backends::interp`).

It is the Rust-side equivalent of the traditional `interpreter_dsp` export
surface:

- factory creation / deletion
- factory cache (`SHA -> factory`)
- bitcode (`.fbc`) read/write
- instance lifecycle
- `compute`
- UI / metadata callback dispatch (`UIGlue` / `MetaGlue`)

## Crate outputs

`Cargo.toml` currently builds:

- `rlib`

Library name:

- Rust lib target name: `faust_interp`

Distribution note:

- final `cdylib` / `staticlib` artifacts are produced by `crates/faust-ffi`,
  which links `interp-ffi` alongside the other FFI backend crates.

## Headers

Headers are kept in `include/`:

- `include/interpreter-dsp-c.h`
- `include/interpreter-dsp.h`

Notes:

- `build.rs` exists, but currently **does not** generate headers.
- `cbindgen` generation is temporarily disabled because of Rust 2024
  `#[unsafe(no_mangle)]` support limitations.
- The headers are maintained manually for now.

## Internal structure

- `src/types.rs`
  - opaque factory/instance FFI wrappers
  - re-exports shared `UIGlue` / `MetaGlue`
- `src/cache.rs`
  - global lifecycle wrappers over `ffi_common::FactoryCache<T, I>`
- `src/factory.rs`
  - factory `extern "C"` API
  - source/bitcode creation paths
  - cache operations
- `src/instance.rs`
  - instance `extern "C"` lifecycle + compute
- `src/ui.rs`
  - UI/meta callback dispatch helpers

## Shared FFI helpers (factorized)

This crate relies on `crates/ffi-common` for backend-agnostic FFI mechanics:

- `UIGlue` / `MetaGlue`
- C string allocation/free helpers
- `freeCMemory` string helper
- `argv` decoding
- error buffer writing (`4096` bytes)
- shared FFI option parsing (`-I`, `-cn`, `-double`, `-vec`, `-vs`, `-lv`,
  and `-ss`)

Backend-specific semantics remain local to this crate (factory/runtime behavior,
`.fbc` serialization semantics, interpreter execution).

## Factory and instance ownership

- Factory creation and SHA lookup each acquire one cache reference.
- Repeated creation with the same SHA returns the same pointer and acquires
  another reference.
- `deleteCInterpreterDSPFactory` returns `false` while references remain and
  `true` only for the final release.
- DSP instances may be deleted manually; final factory release and
  `deleteAllCInterpreterDSPFactories` delete any remaining instances.
- `deleteAllCInterpreterDSPFactories` invalidates all outstanding factory and
  instance pointers regardless of reference count.

## Factory creation paths

`createCInterpreterDSPFactoryFromFile` and `createCInterpreterDSPFactoryFromString`
share common FFI boilerplate (error handling, cache insertion, allocation), but
still keep distinct compile paths:

- file path compile preserves file-based import search semantics
- string compile uses the source-string path

This distinction is intentional to preserve behavior parity.

## Compile errors

`error_msg` is 4096 bytes by the libfaust contract (caller-allocated, size never
passed) and receives the one-line summary of a compiler error.
`getCCompleteInterpreterDSPFactoryError()` returns the complete text, the
summary followed by the rendered diagnostics (location, source snippet, notes,
fixes): per thread, owned by the library (do not free it), null before the
thread's first error, valid until its next one, not reset by a success, no
trailing newline. It covers every entry point of `interpreter-dsp-c.h` that
takes an `error_msg`: factory creation from file, string and bitcode, expansion,
auxiliary files. The C++ wrapper reads it, so `std::string& error_msg` holds the
complete text, and is cleared by a success. The workspace
[C and C++ usage guide](../../README.md#compile-errors) has an example and the
functions of the other APIs.

## Build / test

Targeted checks:

```bash
cargo clippy -p interp-ffi --all-targets -- -D warnings
cargo test -p interp-ffi -- --nocapture
```

Workspace checks (recommended before commit):

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

## Current status

- `interp-ffi` is functional enough for factory/instance smoke tests and the
  interpreter backend integration path.
- The C/C++ compatibility surface is parity-driven but still evolving as the
  Rust port progresses.
