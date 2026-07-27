# FFI Factory-Cache Lifecycle Contract

Date: 2026-07-27

Status: accepted for `interp-ffi` and `cranelift-ffi` phase F4

Reference baseline: Faust C++ commit `8eebea429`
(`master-dev-ocpp-od-fir-2-FIR19`)

## 1. Scope

This document fixes the ownership, reference-counting, deletion, and
concurrency contract for the factory caches exported by the Interpreter and
Cranelift C/C++ APIs. It resolves the F4 decision gate in
`porting/ffi-crate-organization-analysis-2026-07-27-en.md`.

The decision is parity-driven. The primary reference implementation is
`compiler/generator/dsp_aux.hh`, especially `smartable`, `SMARTP`,
`dsp_factory_table::getDSPFactoryFromSHAKey`,
`deleteDSPFactory`, `deleteAllDSPFactories`, `addDSP`, and `removeDSP`.
Interpreter integration is confirmed in:

- `compiler/generator/interpreter/interpreter_dsp_aux.cpp`;
- `compiler/generator/interpreter/interpreter_dynamic_dsp_aux.cpp`;
- the matching maintained public Interpreter headers.

Cranelift has no C++ Faust factory implementation to copy. Its Rust FFI uses
the same generic Faust factory lifecycle so hosts see one consistent contract
across the two Rust backend APIs.

## 2. External lifecycle contract

| Operation | Required behavior |
|---|---|
| First successful creation | Cache and own one factory; return one externally releasable reference |
| Repeated creation with the same SHA | Return the same pointer and acquire one additional reference |
| SHA lookup | Return the cached pointer and acquire one additional reference |
| Factory deletion with references remaining | Release one reference, keep the factory and its instances alive, return `false` |
| Final factory deletion | Delete remaining instances, delete the factory, remove the SHA entry, return `true` |
| Manual instance deletion | Remove and delete only that instance |
| Delete all factories | Delete every instance and factory regardless of reference counts; invalidate all outstanding pointers |

A pointer returned by creation or SHA lookup must therefore be balanced by one
factory deletion unless `deleteAll*DSPFactories` invalidates it first. A DSP
pointer is valid until manual deletion, final deletion of its parent factory,
or deletion of all factories. Callers must not use or delete an invalidated
pointer.

Factory candidates are currently built before cache coalescing. If the SHA is
already present, the newly built candidate is dropped and the existing factory
reference is returned. This may perform redundant compilation, but it does not
change the observable ownership or identity contract.

## 3. Concurrency contract

Factory lifecycle operations are serialized by a mutex:

- insertion/coalescing;
- SHA lookup and reference acquisition;
- reference release and final removal;
- instance registration/removal;
- key enumeration;
- deletion of all factories.

A successful SHA lookup increments the reference count while holding the same
mutex that protects deletion. It therefore pins the factory against concurrent
final deletion until that acquired reference is released.

The C++ `startMTDSPFactories` entry point conditionally installs a factory-table
lock. Rust retains the public start/stop functions and their success behavior,
but always synchronizes lifecycle operations. This is a stronger internal
safety guarantee with no external ABI or result change. It does not make
concurrent use of the same DSP instance safe, and it does not make a raw
pointer usable after its reference is released.

## 4. Rust representation

`ffi_common::FactoryCache<T, I>` owns each cache entry:

- `Box<T>` for the stable factory allocation;
- an explicit external reference count;
- typed `FactoryHandle<T>` identity at the ABI boundary;
- a per-factory map of owned `Box<I>` DSP instances.

Raw pointers are derived only from owned boxes and are never stored as
integers. `FactoryHandle<T>` intentionally exposes no safe dereference API.
The cache drops instance boxes before their parent factory box.

Backend crates retain the global caches and C ABI wrappers because factory
construction and runtime payloads are backend-specific. `ffi-common` owns only
the dependency-light lifecycle mechanism.

## 5. Invariants and failure modes

The implementation must preserve these invariants:

1. One SHA maps to at most one live factory allocation.
2. Every returned factory pointer corresponds to one counted external
   reference.
3. An instance belongs to exactly one live cached factory.
4. Final factory removal drops all registered instances before the factory.
5. Cache clearing leaves no owned factory or instance allocation.
6. A failed instance registration returns null and drops the candidate
   instance.
7. Reference-count overflow fails acquisition without changing the live entry.

The principal unavoidable failure mode is caller use-after-release through the
C ABI. Rust cannot validate an arbitrary stale pointer before the caller
dereferences it. The mitigation is the documented reference contract, opaque
types, lookup pinning, synchronized lifecycle operations, and tests that never
reuse invalidated pointers.

## 6. Verification

Structural shared-cache tests cover:

- repeated insertion and lookup returning one stable pointer;
- exact retained/final-release results;
- candidate, factory, and instance destruction;
- instance-before-factory destruction order;
- manual instance deletion and deletion of all entries;
- concurrent lookup versus final release over repeated schedules.

Interpreter and Cranelift tests separately exercise their public creation,
SHA-lookup, instance-creation, and factory-deletion functions. Header
documentation states the same invalidation rules.

The unified export baseline remains authoritative: F4 must preserve all 388
exported symbols and all 346 maintained-header declarations.

## 7. API mapping

- External Interpreter lifecycle: **1:1** with the maintained Faust C++ API.
- External Cranelift lifecycle: **adapted** to the generic Faust factory
  contract because no reference Cranelift factory API exists; the symbol
  signatures are unchanged.
- Internal Rust representation: **adapted** from integer-erased raw-pointer
  indexing to cache-owned boxes, explicit counts, typed identities, and
  co-located instance ownership.

Compatibility impact: no C symbol, function signature, opaque type layout, or
generated-code contract changes. Existing callers that already balance every
creation/lookup reference retain their behavior. Code that treated SHA lookup
as non-owning was inconsistent with the maintained Faust contract and must
release the acquired reference.
