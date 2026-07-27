# FFI crate organization analysis and improvement plan

**Date:** 2026-07-27

**Analyzed source:** `f51d5000` (`main-dev`)

**Scope:** workspace crate boundaries, shared FFI support, naming, dependency
direction, and safety contracts

**Status:** analysis and proposed migration; no implementation in this document

## 1. Question

A code review raised the following concern:

> There are a lot of "ffi" crates, and to add to the confusion, some
> not-ffi-labeled crates like the utils crate contain FFI code.

The criticism is valid, but it contains two separate issues:

1. whether the number of FFI crates reflects useful architectural boundaries;
2. whether crate names and documentation make those boundaries understandable.

The first issue is mostly structural and justified. The second issue is real:
`crates/utils` is named and introduced as a general-purpose utility crate even
though its implemented API is FFI-specific and every current consumer is an FFI
adapter.

The recommended improvement is therefore not a broad merge. It is to rename
`utils` to `ffi-common`, split its API into explicit responsibility modules,
document the FFI topology as one subsystem, and strengthen the dependency and
safety contracts around that subsystem.

## 2. Current topology

The workspace currently contains these FFI-facing crates:

| Crate | Current responsibility | Boundary assessment |
|---|---|---|
| `tree-ffi` | Shared opaque tree handles, global tree context, and C-compatible enums used by the Box and Signal APIs | Keep separate: it prevents a Box/Signal dependency cycle and owns shared handle identity |
| `box-ffi` | Faust `Cbox*` constructors, matchers, and Box-to-Signal/source entry points | Keep separate: this is a large external compatibility surface |
| `signal-ffi` | Faust `Csig*` constructors, matchers, normalization, and source entry points | Keep separate: this is a distinct external compatibility surface |
| `interp-ffi` | Interpreter factory, instance, cache, lifecycle, UI, metadata, and compute APIs | Keep separate: backend-specific runtime and lifecycle |
| `cranelift-ffi` | Cranelift factory, instance, JIT runtime, cache, lifecycle, UI, metadata, and compute APIs | Keep separate: backend-specific runtime, native target constraints, and experimental status |
| `wasm-ffi` | Raw pointer/length ABI for the compiler embedded as a WebAssembly module | Keep separate: WebAssembly has a different allocation, result-handle, and target model |
| `faust-ffi` | Final `cdylib`/`staticlib` aggregation crate for `libfaust-rs` | Keep separate: packaging and link aggregation are its only responsibility |
| `utils` | Shared ABI structs, C strings, opaque allocations, C argument decoding, FFI compile options, and raw-pointer factory cache | Rename and modularize: the current name hides its actual boundary |

This is not a collection of uniformly small crates. `box-ffi`, `signal-ffi`,
`interp-ffi`, `cranelift-ffi`, and `wasm-ffi` contain substantial,
independently testable adapter or runtime implementations. Combining them would
increase dependency fan-in, enlarge the scope in which `unsafe` is permitted,
and make backend-specific lifecycle changes affect unrelated C API tiers.

Two small crates also have deliberate roles:

- `tree-ffi` is a shared state and handle-identity boundary needed by both Box
  and Signal without making either domain crate own the other;
- `faust-ffi` is a packaging crate that causes the backend/API `rlib` crates to
  be linked into the final distribution artifact.

The number of crates is consequently not the principal defect. The defect is
that the workspace does not present them clearly as one FFI subsystem, and
`utils` appears to belong to the compiler foundation even though it does not.

## 3. Audit of `crates/utils`

The crate header describes an intended general role:

- formatting helpers;
- path helpers;
- stable ordering helpers;
- other dependency-light cross-cutting utilities.

The implemented API instead contains the following:

| Current items | Actual responsibility |
|---|---|
| `FfiFaustFloat`, `UIGlue`, `MetaGlue` | C ABI type and callback-table definitions |
| `alloc_c_string`, `free_c_string`, `free_c_memory_c_string_only` | Cross-language string ownership |
| `alloc_opaque`, `free_opaque` | Opaque C handle ownership |
| `write_error_4096` | Faust C API error-buffer convention |
| `decode_c_argv`, `required_c_str_arg`, `optional_c_str_arg`, `null_c_string_array` | C argument decoding and marshalling |
| `FfiCompileArgs`, `parse_ffi_compile_args` | CLI-like option translation at FFI entry points |
| `FactoryCache<T>` | FFI factory-pointer lookup and compatibility state |
| `CRATE_NAME`, `crate_id()` | Placeholder/scaffold identity with no runtime value |

Current consumers are `tree-ffi`, `box-ffi`, `signal-ffi`, `interp-ffi`,
`cranelift-ffi`, and `wasm-ffi`. No compiler-core crate depends on `utils`.
This is good dependency isolation hidden behind a misleading name.

The crate also opts into `unsafe_code = "allow"`, correctly for its implemented
contents but unexpectedly for a crate called `utils`. A future contributor
could reasonably place general helpers there and accidentally move a core crate
across the unsafe boundary.

## 4. Proposed target organization

### 4.1 Rename `utils` to `ffi-common`

The proposed package and directory are:

```text
crates/ffi-common/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── abi.rs
    ├── args.rs
    ├── factory_cache.rs
    ├── memory.rs
    └── strings.rs
```

The Rust crate name becomes `ffi_common`.

Suggested module ownership:

| Module | Contents |
|---|---|
| `abi` | `FfiFaustFloat`, `UIGlue`, `MetaGlue` |
| `strings` | C string decoding/allocation, empty `char**`, error-buffer writing |
| `memory` | Generic opaque allocation/free helpers |
| `args` | `FfiCompileArgs` and FFI option parsing |
| `factory_cache` | Shared factory-cache mechanism only |

`lib.rs` should describe this as an internal, dependency-light support crate for
Faust ABI adapters. It should not promise general-purpose utilities. The
placeholder-only `CRATE_NAME` and `crate_id()` API should be removed.

This produces one shared crate, not one crate per helper category. Creating
multiple new support crates would reproduce the reviewer's original complaint.
Modules are sufficient because these helpers share the same dependency and
unsafe boundary.

### 4.2 Keep the domain/backend crates

The following crates should remain separate:

```text
ffi-common
├── tree-ffi
│   ├── box-ffi
│   └── signal-ffi
├── interp-ffi
├── cranelift-ffi
└── wasm-ffi

faust-ffi
├── box-ffi
├── signal-ffi
├── interp-ffi
└── cranelift-ffi
```

This is a responsibility view, not a complete Cargo dependency graph. In
particular, Box/Signal conversion entry points and backend implementation reuse
may add domain dependencies beyond the edges shown.

No physical relocation under a new `crates/ffi/` directory is proposed in the
first migration. Many reports, headers, scripts, and documents name the current
paths. Renaming only the misleading crate provides the main clarity benefit
with substantially less mechanical churn. A later directory regrouping should
be considered only if the workspace remains difficult to navigate after the
README and naming changes.

### 4.3 Document three workspace categories

The root workspace inventory should distinguish:

1. **compiler core** — parser, IRs, analyses, transforms, FIR, and code
   generation;
2. **FFI adapters** — `ffi-common`, `tree-ffi`, Box/Signal APIs, and backend
   runtime APIs;
3. **distribution targets** — `faust-ffi` and `wasm-ffi`.

This makes the role of the otherwise tiny `faust-ffi` crate explicit and avoids
presenting every workspace member as the same kind of component.

## 5. Dependency and unsafe-code invariants

The cleanup should establish the following machine-checkable direction:

```text
compiler core  <-  FFI adapters  <-  distribution crates
```

The arrows mean "may be depended on by." The reverse direction is forbidden:

- no compiler-core crate may depend on `ffi-common` or any `*-ffi` crate;
- `ffi-common` must remain dependency-light and must not depend on compiler
  domains or backends;
- only genuine foreign-boundary crates may override the workspace
  `unsafe_code = "forbid"` policy;
- all exported `extern "C"` functions remain in domain/backend FFI crates, not
  in `ffi-common`;
- backend-specific factory and instance semantics remain in the owning backend
  crate.

`faust-ffi` currently permits unsafe code even though its source only re-exports
the component crates. It should inherit the workspace `unsafe_code = "forbid"`
lint unless future packaging code demonstrates a concrete need for unsafe.
Dependency crates being unsafe does not require the aggregation crate itself to
allow unsafe code.

The `foreign-call` crate is a documented exception to an `*-ffi` naming rule:
it invokes host-provided C functions from compiled DSP execution but does not
export a public Faust C ABI. Its name describes that runtime bridge more
accurately than an `ffi` suffix would.

## 6. Safety hardening opportunities

The rename and file split should initially preserve behavior. Three safety
changes should then follow as separate commits because they affect contracts,
not only organization.

### 6.1 Bind C string lifetimes by owning decoded values

`required_c_str_arg<'a>` and `optional_c_str_arg<'a>` return borrowed `&str`
values whose lifetime parameter is not tied to a Rust input reference. The
functions are unsafe, but this signature still lets a caller request a lifetime
that outlives the C storage.

The preferred FFI-boundary API is:

```rust
unsafe fn required_c_string_arg(
    ptr: *const c_char,
    label: &str,
) -> Result<String, String>;

unsafe fn optional_c_string_arg(
    ptr: *const c_char,
    label: &str,
) -> Result<Option<String>, String>;
```

Factory creation and source compilation are not sample-loop operations, so the
small allocation cost is preferable to exporting an unconstrained borrowed
lifetime. If a measured hot path later needs borrowing, it should use a scoped
callback or a wrapper whose lifetime is tied to an explicit borrowed owner.

### 6.2 Verify ABI layouts

`UIGlue` and `MetaGlue` are `#[repr(C)]`, but construction-only Rust tests do
not prove that their size, alignment, field order, and callback signatures stay
aligned with the maintained C headers.

Add:

- Rust size/alignment/offset assertions for the callback tables;
- C and C++ header smoke translation units that initialize every field;
- a cross-language layout probe where practical;
- explicit tests for the `FAUSTFLOAT` policy used by these exported APIs.

The ABI definitions should remain centralized in `ffi-common::abi` and be
re-exported by backend crates only where their public Rust APIs currently
require it.

### 6.3 Replace integer-erased raw pointers in `FactoryCache`

`FactoryCache<T>` stores raw pointers as `usize` inside a mutex to make the
static cache type `Send + Sync`. This is mechanically convenient but erases
pointer provenance and does not encode ownership, refcount, deletion, or
concurrent lifetime rules.

This must not be changed as part of the rename. Factory-cache semantics are an
external lifecycle compatibility decision. A dedicated follow-up should first
specify:

- whether the cache owns factories or only indexes externally owned handles;
- the reference Faust behavior for repeated creation, deletion, and cache
  clearing;
- whether a lookup pins a factory against concurrent deletion;
- what the MT compatibility flag promises;
- how Interpreter and Cranelift factory semantics may legitimately differ.

Only after those rules are decided should the representation move to an
explicit handle/ownership type, preferably retaining owned Rust values
internally and deriving raw pointers only at the ABI edge.

## 7. Migration plan and gates

### Phase F0 — Baseline and contract inventory

Deliverables:

- record every `utils::*` consumer;
- record exported symbols from all FFI crates;
- record callback-table layouts and maintained headers;
- add an FFI crate map to the porting documentation.

Pass criteria:

- no consumer or exported symbol is unaccounted for;
- the current C header smoke tests and backend lifecycle tests pass;
- the baseline unified library exports are captured for before/after
  comparison.

### Phase F1 — Behavior-preserving rename and module split

Deliverables:

- rename `crates/utils` and package `utils` to `crates/ffi-common` and
  `ffi-common`;
- split the monolithic source into the modules described above;
- update Cargo dependencies, Rust paths, README files, scripts, and versioned
  porting references that describe the current location;
- remove `CRATE_NAME` and `crate_id()`;
- make `faust-ffi` inherit the workspace unsafe lint.

Pass criteria:

- no exported C symbol or header changes;
- unified static and dynamic library artifact names remain unchanged;
- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo test --workspace --all-targets`;
- `cargo run -p xtask -- golden-check`;
- existing C/C++ header smoke tests pass.

Public API mapping: **adapted, internal Rust API only**. Cargo package and Rust
module paths change from `utils` to `ffi-common`/`ffi_common`; the external C
ABI and generated-code contracts are unchanged.

### Phase F2 — Dependency-boundary enforcement and documentation

Deliverables:

- group the root README crate inventory by compiler core, FFI adapters, and
  distribution;
- document the allowed dependency direction;
- add a lightweight workspace check that rejects core-to-FFI dependency edges;
- verify that each unsafe-lint override has a stated foreign-boundary reason.

Pass criteria:

- dependency check passes on all workspace targets;
- no core crate opts into unsafe solely through a generic utility dependency;
- FFI documentation identifies the owning crate for every public API tier.

Public API mapping: **no public API change**.

### Phase F3 — Owned C argument decoding and ABI layout tests

Deliverables:

- replace unconstrained borrowed string decoding with owned results;
- update all FFI call sites;
- add ABI layout and callback-signature tests.

Pass criteria:

- invalid UTF-8, null arguments, embedded-NUL return strings, and 4096-byte
  error-buffer truncation remain covered;
- header smoke and cross-language layout checks pass;
- external function signatures and symbols remain unchanged.

Public API mapping: **adapted, internal Rust API only**. Argument helper return
types change; the C ABI does not.

### Phase F4 — Factory-cache lifecycle hardening

Decision gate:

The factory ownership, cache, deletion, and concurrency contract must be
resolved against the maintained C/C++ API before implementation. This is a
parity-sensitive lifecycle decision and must not be inferred from the current
`usize` representation.

Deliverables and pass criteria depend on that decision, but must include:

- lifecycle tests for repeated create/lookup/delete/clear sequences;
- concurrent lookup/deletion tests if concurrency is promised;
- Interpreter and Cranelift backend-specific conformance;
- no integer-erased pointer storage unless explicitly justified and tested.

Public API mapping: **deferred** until the lifecycle contract is fixed.

## 8. Rejected alternatives

### Merge all FFI APIs into one crate

Rejected because it would combine distinct public API tiers, backend runtimes,
target constraints, and lifecycle implementations under one unsafe boundary.
It would reduce the Cargo member count while increasing architectural coupling.

### Create separate crates for ABI types, strings, arguments, and cache

Rejected because these helpers share one dependency-light FFI support boundary.
Modules give the required ownership clarity without adding more crates.

### Move all FFI crates under `crates/ffi/` immediately

Deferred because it produces broad path churn in generated reports, headers,
scripts, documentation, and test commands without changing dependency
semantics. Rename the misleading crate and improve the workspace inventory
first, then reassess navigation.

### Keep the name `utils` and only rewrite its README

Rejected because the generic name continues to invite unrelated helpers and
hides the unsafe/ABI dependency boundary in Cargo manifests and Rust imports.

## 9. Recommended outcome

The target is not "fewer crates at any cost." It is a legible FFI subsystem:

- substantial API and backend boundaries remain isolated;
- the shared crate is named `ffi-common`, contains only boundary support, and
  is organized by responsibility;
- compiler-core crates cannot depend on FFI support;
- unsafe permission is visibly tied to foreign boundaries;
- the distribution crates remain distinct from implementation adapters;
- lifetime and cache risks are handled in explicit follow-up phases rather than
  being hidden inside a mechanical rename.

This directly addresses the review concern while preserving the crate
boundaries that currently protect ABI ownership, backend independence, and the
workspace-wide unsafe-code policy.
