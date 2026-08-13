# `-mem0` Phase 0 Validation

Date: 2026-08-13

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`

Decision: **GO** for the scalar C, C++, and native Cranelift implementation
described by
`custom-memory-manager-mem0-analysis-and-porting-plan-2026-08-13-en.md`.

## 1. Effective production paths

The port uses the existing production compiler facade and transform fast lane:

```text
CLI / compiler facade
  -> parse -> boxes -> eval -> propagate
  -> transform::signal_fir fast lane
  -> verified FirStore + module
  -> C emitter | C++ emitter | Cranelift JIT
```

The relevant Rust dispatch is in `crates/compiler/src/signal_lowering.rs`.
`Compiler` source/file entry points in `crates/compiler/src/emitters.rs` first
produce signals and then call those lowering functions. The Cranelift FFI path
keeps the resulting `JitDspModule` alive rather than rendering only the compiler
facade's status report.

The canonical memory and cost analysis is inserted after effective FIR
construction and before target storage emission. It does not alter parsing,
evaluation, propagation, TreeArena interning, or the DSP algorithm. C and C++
analyze the common effective module. Cranelift analyzes the exact module view it
uses for `StructLayoutPlan` and JIT lowering, including any submodule flattening.

## 2. Scope and capability matrix

| Surface | `mem0` status | Compatibility class | Gate |
|---|---|---|---|
| generated C++ | included | `1:1` plus explicit reference fixes | compile/runtime/differential |
| generated C | included | `adapted` | strict-C compile/runtime |
| Cranelift JIT | included, native scalar subset | `adapted` | strict lowering/runtime/FFI |
| JSON memory layout | included, schema v2 | `adapted` additive schema | semantic parser checks |
| JSON `compute_cost` | included | `1:1` shape plus versioned reference fixes | structural/differential |
| `-mem`, `-mem0`, long aliases | included | `1:1` spelling normalization | CLI tests |
| `mem1`–`mem3` | excluded | `deferred` | stable rejection |
| vector `mem0` | excluded until separately qualified | `deferred` | stable rejection |
| Java, legacy `ocpp`, Wasm and other backends | excluded | out of scope | stable rejection |

`-it -mem0` remains rejected. Cranelift tests require
`fail_on_subset_gap = true` and `compute_body_lowered = true`; a no-op fallback
cannot satisfy a `mem0` gate.

## 3. Explicit option/state path

The implementation adds `MemoryManagerMode::{None, Mem0}` in `codegen` and
passes it by value through:

```text
CliArgs / programmatic backend options
  -> Compiler lowering request
  -> COptions | CppOptions | CraneliftOptions | JsonBuildOptions
  -> Mem0Analysis
  -> emitter/JIT/factory JSON
```

The Cranelift source/file factory argv parser also produces this enum. The
semantic mode participates in factory SHA/cache/serialization identity; manager
callback addresses and live allocations do not. No process-global equivalent
of C++ `gMemoryManager` is introduced.

## 4. Lifecycle and ownership

The invariant remains:

```text
init = classInit -> instanceInit
instanceInit = instanceConstants
             -> instanceResetUserInterface
             -> instanceClear
```

Generated C/C++ class allocation occurs in explicit class initialization.
Cranelift has no public class-init factory call today: first instance creation
allocates shared class storage before instance storage, and the first instance
`init` fills it once before `instanceInit`. Final factory release destroys live
instances, class zones, then the JIT module.

The C++ object or C/Cranelift native DSP state block and all externalized FIR
zones are owned by the manager that allocated them. The Rust
`CraneliftDspInstance` wrapper remains opaque Rust bookkeeping and is excluded
from the memory description. Allocation is transactional; destruction is
reverse-order and uses captured callbacks. Clone deep-copies instance state and
buffers while sharing only approved class tables.

## 5. Frozen decisions D1–D12

All recommended alternatives in plan section 8 are approved:

1. explicit target ABI and exactness metadata;
2. captured C/C++ manager binding with aligned additive ABI;
3. checked creation plus leak-free failure unwinding;
4. idempotent class lifecycle with live-instance protection;
5. append-only `Int64`/`Bool` vocabulary or capability rejection, never a
   wrong cast;
6. `compute_cost` v2 with exhaustive literals and component-wise branch
   maxima;
7. Cranelift writable static tables use factory-owned zones and DSP pointer
   slots, not mutable process globals;
8. Cranelift reuses the versioned shared C manager ABI and describes on setter;
9. unbound Cranelift compilation/JSON is allowed but class/instance allocation
   fails until binding; cache callbacks run outside the global lock;
10. the logical Cranelift DSP object is its native JIT state block, not its Rust
    wrapper;
11. general subset fallback remains independent, but all `mem0` gates require
    real lowering;
12. no new public Cranelift class-init symbols in this phase; allocation and
    semantic fill follow the adapted instance/factory lifecycle above.

## 6. Public API mapping

| Reference/current surface | Target | Status | Compatibility impact |
|---|---|---|---|
| C++ `dsp_memory_manager` | compatible generated C++ surface | `1:1` plus additive alignment/failure fixes | old architectures remain usable within documented alignment capability |
| dormant C generator branches | versioned `faust_memory_manager` callback table | `adapted` | new supported C ABI |
| Cranelift C API missing setter | `setCCraneliftMemoryManager` | `adapted` | closes deferred parity row |
| Cranelift C++ no-op set/get | functional factory binding | `adapted` | existing signatures acquire real behavior |
| C++ legacy memory JSON | versioned v2 common fields | `1:1` common subset plus reference fixes | additive fields and corrected counts |
| C++ `InstComplexityVisitor` JSON | `ComputeCost` v2 | `1:1` shape, `reference-fix` semantics | defective branches/literals corrected and versioned |

Implementation must update the compatibility registry and Cranelift parity
matrix in the same phase that exposes each surface.

## 7. Differential baseline and acceptance

The pinned C++ compiler is the source/code/JSON oracle for generated C++ on the
common, non-defective subset. Approved differences are limited to the defects
listed in plan section 4 and must be fixture-allowlisted. Generated C has no
supported upstream `mem0` oracle, so it uses the canonical C++ zone semantics
plus strict-C runtime parity. Cranelift has no upstream backend oracle, so it
uses canonical role/count/cost parity plus ordinary-versus-`mem0` JIT runtime
parity and exact `StructLayoutPlan` checks.

The self-contained corpus covers no-array state, several delays, integer/real
arrays, literal and runtime-generated tables, UI scalars, both precisions,
clone, failure unwinding, and branch-heavy compute cost. Numeric impulse output,
manager trace, JSON, lifecycle, and optimized/unoptimized execution are all
independent acceptance dimensions.

## 8. TreeArena, recursion, diagnostics, and stubs

- TreeArena: not affected. The implementation consumes established FIR and
  adds no tree node, interning key, property table, or traversal in the frontend.
- Recursion: canonical `sigRec`/`sigProj` and signal→FIR lowering remain
  unchanged. Memory analysis traverses finite FIR blocks iteratively or with
  existing bounded visitor conventions.
- Diagnostics: option, analysis, overflow, ABI, allocation, and subset failures
  use typed crate/backend errors and stable CLI/FFI rendering. No panic crosses
  an FFI boundary.
- Stubs: none are approved. The Cranelift no-op compute fallback is existing
  behavior but is expressly disallowed from validation and completion gates.

## 9. Rustdoc provenance convention

Every new public or parity-sensitive item records:

- the corresponding C++ file/function when one exists;
- `adapted` status when no direct source oracle exists;
- storage/lifecycle/counting invariants that would cause silent drift;
- ownership and failure behavior for unsafe/FFI boundaries.

Primary sources are `CodeContainer::createMemoryLayout`,
`StructInstVisitor`, `ArrayToPointer`, `InstComplexityVisitor`, the C++/C code
containers, and current Rust Cranelift `StructLayoutPlan`,
`define_static_tables_in_jit`, `DspStateBuffer`, and `class_init_instance`.

## 10. Quality gates and blockers

Each implementation commit runs focused unit/compile/runtime tests. Pipeline
changes additionally run the release compile-budget gate before completion.
The final phase runs formatting, workspace Clippy/tests, golden checks, all
three impulse targets, and differential/hardening checks.

No Phase 0 blocker remains. There is no deferred prototype or unowned stub debt
created by this validation.
