# cranelift-ffi

C/C++ FFI export crate for the experimental Cranelift backend (`cranelift_dsp` family).

## Purpose

This crate hosts the Rust-side C ABI and C++ wrappers for a Faust backend
implemented with Cranelift.

It mirrors the overall strategy used by `llvm_dsp` / `interpreter_dsp`:

- factory cache and factory lifecycle
- instance lifecycle and `compute`
- UI / metadata callback dispatch (`UIGlue` / `MetaGlue`)
- C API + C++ wrapper headers

## Current status (important)

This crate is **experimental** and remains in active bring-up.

What is already implemented:

- executable C ABI
- factory/instance opaque types
- minimal factory cache
- source factory creation through `compiler -> FIR -> codegen::cranelift`
- bitcode family scaffold (temporary text format, for API-path validation)
- FIR-derived native runtime descriptor for state/UI/meta handling
- native JIT-backed `compute` path for file/string factory constructors
- header smoke tests (C and C++)

What is not final yet:

- full backend/runtime parity
- final Cranelift backend serialization format
- full C++ wrapper behavioral implementation parity
- full V1 export surface completeness (some families intentionally deferred)

## Crate outputs

`Cargo.toml` currently builds:

- `rlib` (needed for Rust examples/tests)

Library name:

- Rust lib target name: `cranelift_ffi`

Distribution note:

- final `cdylib` / `staticlib` artifacts are produced by `crates/faust-ffi`,
  which links `cranelift-ffi` alongside the other FFI backend crates.

## Headers

Headers are in `include/`:

- `include/cranelift-dsp-c.h`
- `include/cranelift-dsp.h`

Related files:

- `cpp/cranelift-dsp.cpp` (translation unit for the inline C++ wrapper)
- `tests/header-smoke/` (syntax smoke for C/C++ headers)

The headers document the currently exposed compatibility surface and explicitly
list V1-deferred families where relevant.

## Internal structure

- `src/types.rs`
  - opaque factory/instance wrappers
  - re-exports shared `UIGlue` / `MetaGlue`
- `src/cache.rs`
  - global factory cache wrappers over `ffi_common::FactoryCache<T>`
- `src/factory.rs`
  - Cranelift factory `extern "C"` API
  - compiler/FIR/JIT factory construction
  - temporary bitcode family
- `src/instance.rs`
  - instance lifecycle / UI / metadata / compute exports
- `src/runtime.rs`
  - FIR-derived native runtime descriptor builder shared by factories/instances
- `src/clif.rs`
  - textual `.clif` container helpers used by the current bitcode scaffold

## Shared FFI helpers (factorized)

This crate uses shared backend-agnostic FFI helpers from `crates/ffi-common`:

- `UIGlue` / `MetaGlue`
- C string allocation/free helpers
- `freeCMemory` string helper
- `argv` decoding
- error buffer writing (`4096` bytes)
- C-string argument decoding helpers
- empty `char**` helper
- FFI option parsing (`-I`, `-cn`, `-double`, `-vec`, `-vs`, `-lv`, and
  `-ss`)

Cranelift-specific factory/runtime state and backend semantics remain local to
this crate.

## Factory creation paths

`createCCraneliftDSPFactoryFromFile` and `createCCraneliftDSPFactoryFromString`
share common FFI boilerplate (error handling, allocation, cache insertion), but
still keep distinct backend preflight paths:

- file path preflight preserves file-based import search semantics
- string preflight uses inline-source compilation path

This is intentional and matches the interpreter FFI refactor strategy.

## Build / test

Targeted checks:

```bash
cargo clippy -p cranelift-ffi --all-targets -- -D warnings
cargo test -p cranelift-ffi -- --nocapture
```

Header smoke checks (examples):

```bash
cc -fsyntax-only -I crates/cranelift-ffi/include \
  crates/cranelift-ffi/tests/header-smoke/cranelift_dsp_c_header_smoke.c

c++ -std=c++11 -fsyntax-only -I crates/cranelift-ffi/include -I /path/to/faust/architecture \
  crates/cranelift-ffi/tests/header-smoke/cranelift_dsp_cpp_header_smoke.cpp
```

## Known limitations

- Some LLVM-specific API families are intentionally omitted/deferred in V1
  (target getters, LLVM IR/machine/object serialization, and memory-manager
  hooks). Cranelift foreign-function registration is implemented separately.
- The bitcode API family currently uses a temporary scaffold format marker
  (`CRANELIFT_FFI_SCAFFOLD_V1`) and is not the final backend serialization.
- Runtime behavior is still progressing toward full Interpreter/C++ backend
  parity across the complete Faust language and runtime surface.

## Related planning docs

- `porting/cranelift-backend-plan-en.md`
- `porting/cranelift-dsp-ffi-parity-matrix-en.md`
