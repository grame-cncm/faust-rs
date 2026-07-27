# FFI crate map and export baseline

**Date:** 2026-07-27

**Source commit:** `8a426267`

**Phase:** F0 of
`porting/ffi-crate-organization-analysis-2026-07-27-en.md`

## 1. Purpose

This document records the pre-refactor FFI topology and external symbol
baseline. It is the comparison point for the `utils` to `ffi-common` migration
and the later safety hardening phases.

F0 changes no compiler behavior, Rust crate ownership, generated code, or C
ABI. It adds a reproducible check that makes those non-changes testable.

## 2. FFI dependency map

The current FFI support consumers are:

| Consumer | Shared `utils` responsibilities used |
|---|---|
| `tree-ffi` | C string allocation/free support |
| `box-ffi` | C argument decoding, compile options, error buffers, C strings |
| `signal-ffi` | C argument decoding, error buffers, C strings |
| `interp-ffi` | callback ABI types, opaque allocation, strings, arguments, compile options, factory cache |
| `cranelift-ffi` | callback ABI types, opaque allocation, strings, arguments, compile options, factory cache |
| `wasm-ffi` | compile-option translation |

No compiler-core crate depends on `utils`. The current dependency direction is
therefore already suitable for the rename:

```text
compiler core  <-  FFI adapters  <-  distribution crates
                       ^
                       |
                    `utils`
```

Here an arrow points from a dependency to a permitted consumer.

`faust-ffi` is the native distribution aggregator. It links `box-ffi`,
`signal-ffi`, `interp-ffi`, and `cranelift-ffi` into the final
`libfaust-rs` static and dynamic libraries. `wasm-ffi` remains a separate
distribution target because it exposes a WebAssembly pointer/length and result
handle ABI.

## 3. ABI type inventory

The shared callback ABI currently lives in `crates/utils/src/lib.rs`:

| Rust type | Maintained C definitions | Structural baseline |
|---|---|---|
| `FfiFaustFloat` | `FAUSTFLOAT` in Interpreter and Cranelift C headers | `f32` |
| `UIGlue` | `UIGlue` in `interpreter-dsp-c.h` and `cranelift-dsp-c.h` | `#[repr(C)]`, one context pointer plus thirteen callback fields |
| `MetaGlue` | `MetaGlue` in both backend C headers | `#[repr(C)]`, one context pointer plus one callback field |

Before F3, tests only prove that these Rust types are constructible. They do
not yet assert offsets against a C layout probe. F3 owns that missing
cross-language layout gate.

The backend crates re-export `UIGlue` and `MetaGlue` from their `types`
modules. Those re-exports are observable Rust API paths, but the structs
themselves have one shared definition.

## 4. Header and exported-symbol baseline

The native distribution has four maintained C headers:

| API tier | Header |
|---|---|
| Box | `crates/box-ffi/include/libfaust-box-c.h` |
| Signal | `crates/signal-ffi/include/libfaust-signal-c.h` |
| Interpreter | `crates/interp-ffi/include/interpreter-dsp-c.h` |
| Cranelift | `crates/cranelift-ffi/include/cranelift-dsp-c.h` |

At source commit `8a426267`, the macOS debug dynamic library contains:

- **388** exported Faust C symbols;
- **346** distinct function declarations parsed from the four maintained C
  headers;
- **42** exported symbols not declared in those C headers.

The 42 non-header exports are still part of the observed distribution ABI and
are therefore retained in the checked-in baseline:

- 39 `Cbox*Aux` compatibility helpers used by the Box wrapper layer;
- `createCCraneliftDSPFactoryFromBoxes`;
- `createCCraneliftDSPFactoryFromSignals`;
- `getInterpreterDSPInstanceVersion`.

The complete sorted set is stored in
`porting/generated/libfaust-rs-exported-symbols.txt`.

## 5. Reproducible export gate

`cargo run -p xtask -- libfaust-export-check` now:

1. builds and packages the unified `faust-ffi` dynamic library;
2. extracts its public C symbols with the platform-native inspection tool;
3. requires all four C headers' declarations to be exported;
4. requires the complete 388-symbol checked-in baseline to match exactly;
5. syntax-checks Box/Signal, Interpreter, and Cranelift C clients separately;
6. syntax-checks the maintained Box/Signal C++ wrapper client.

The backend C headers are checked in separate translation units because both
define the same public `UIGlue` and `MetaGlue` type names. Including both
headers in one C file would create an artificial typedef collision that real
backend clients do not encounter.

An intentional ABI change refreshes the baseline explicitly:

```text
cargo run -p xtask -- libfaust-export-check --bless
```

The normal check never rewrites the baseline.

## 6. Factory lifecycle baseline

The shared `FactoryCache<T>` currently provides:

- SHA-key insertion and lookup;
- pointer-based removal;
- complete drain;
- SHA-key listing;
- a compatibility-only MT mode flag;
- unconditional mutex protection.

The Interpreter and Cranelift wrapper caches both store raw factory pointers
through this helper. Current important differences are:

| Backend | Current cache/lifecycle status |
|---|---|
| Interpreter | functional factory construction and lookup; cache comments claim parity shape but no reference-count model |
| Cranelift | explicitly documented scaffold cache; no reference-count semantics |
| C++ reference | factory table owns one smart-pointer reference, lookups add a reference, deletion decrements or erases on the last external reference |

F0 does not resolve that difference. F4 must compare create/lookup/delete/all
sequences with the C++ reference before choosing an owned Rust representation.

Existing focused coverage includes:

- shared raw-pointer cache round-trip tests;
- Cranelift cache create/lookup/list wiring;
- Interpreter and Cranelift factory creation tests;
- Cranelift lifecycle conformance tests for instance initialization;
- native export and header smoke checks.

Refcounted repeated-lookup/deletion behavior is not yet covered and remains an
F4 gate.

## 7. F0 pass result

The F0 deliverables are satisfied:

- every current `utils` consumer is recorded;
- each FFI crate has an explicit ownership role;
- the callback tables and maintained headers are identified;
- all current native exports are captured in a checked-in baseline;
- header parsing covers all four native API tiers;
- the export checker has a deliberate refresh workflow.

Validation:

```text
cargo fmt --all --check
cargo test -p xtask
cargo run -p xtask -- libfaust-export-check
git diff --check
```

Public API mapping: no API change. The new baseline and checker only observe
the existing native C surface.
