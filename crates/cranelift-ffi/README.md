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
- owned, reference-counted factory and DSP-instance cache
- source factory creation through `compiler -> FIR -> codegen::cranelift`
- source-backed bitcode-family payload with deterministic factory rebuild
- FIR-derived native runtime descriptor for state/UI/meta handling
- native JIT-backed `compute` path for file/string factory constructors
- mode-zero custom memory ownership for object, instance-buffer, and generated
  class-table zones
- strict version-2 factory JSON with memory layout and `compute_cost`
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
  - global lifecycle wrappers over `ffi_common::FactoryCache<T, I>`
- `src/factory.rs`
  - Cranelift factory `extern "C"` API
  - compiler/FIR/JIT factory construction
  - temporary bitcode family
- `src/instance.rs`
  - instance lifecycle / UI / metadata / compute exports
- `src/runtime.rs`
  - FIR-derived native runtime descriptor builder shared by factories/instances
- `src/clif.rs`
  - textual `.clif` container helpers used by factory persistence

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
  `-ss`, and the four `mem0` aliases)

Cranelift-specific factory/runtime state and backend semantics remain local to
this crate.

## Factory and instance ownership

- Factory creation and SHA lookup each acquire one cache reference.
- Repeated creation with the same SHA returns the same pointer and acquires
  another reference.
- `deleteCCraneliftDSPFactory` returns `false` while references remain and
  `true` only for the final release.
- DSP instances may be deleted manually; final factory release and
  `deleteAllCCraneliftDSPFactories` delete any remaining instances.
- `deleteAllCCraneliftDSPFactories` invalidates all outstanding factory and
  instance pointers regardless of reference count.

## Factory creation paths

`createCCraneliftDSPFactoryFromFile` and `createCCraneliftDSPFactoryFromString`
share common FFI boilerplate (error handling, allocation, cache insertion), but
still keep distinct backend preflight paths:

- file path preflight preserves file-based import search semantics
- string preflight uses inline-source compilation path

This is intentional and matches the interpreter FFI refactor strategy.

## Mode-zero custom memory manager

Factories created with `-mem`, `-mem0`, `--memory-manager`, or
`--memory-manager0` retain the canonical mode in their compile options, cache
identity, JSON, and source-backed serialization. Before instance creation, the
host must call `setCCraneliftMemoryManager` with a compatible
`faust_memory_manager` ABI-v1 table. Binding copies and describes the callbacks
but performs no allocation; an unbound `mem0` factory cannot create an
instance.

The manager owns the logical DSP object, eligible instance buffers, and
writable generated class tables. Allocation is checked and transactional;
clone deep-copies instance zones; destruction uses the originally captured
callbacks in reverse order. Final factory release destroys instances, class
storage, then JIT state. Serialized factories never store callbacks or host
pointers and therefore require a fresh binding after restore.

The C++ wrapper's `setMemoryManager`/`getMemoryManager` adapt the legacy
`dsp_memory_manager` contract to this aligned C ABI. The Rust wrapper object
itself remains cache-owned and is not a manager-visible JIT zone.

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
  (target getters and LLVM IR/machine/object serialization). Cranelift
  foreign-function registration and `mem0` manager hooks are implemented
  separately.
- The bitcode API family uses a source-backed `CRANELIFT_FFI_V2_SOURCE`
  payload, not a portable native-code snapshot.
- Runtime behavior is still progressing toward full Interpreter/C++ backend
  parity across the complete Faust language and runtime surface.

## Related planning docs

- `porting/cranelift-backend-plan-en.md`
- `porting/cranelift-dsp-ffi-parity-matrix-en.md`
