# Custom Memory Manager `-mem0` — Analysis and Porting Plan

Date: 2026-08-13

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`

Status: implementation active; M0 Phase 0 validation complete.

## 1. Goal and scope

Port the Faust custom-memory-manager mode to `faust-rs`, limited to the
following option aliases:

```text
-mem
-mem0
--memory-manager
--memory-manager0
```

All four spellings select one internal mode, `mem0`. The canonical spelling in
generated metadata and diagnostics is `-mem0`.

The target covers:

- C++ source generation;
- C source generation;
- the Cranelift JIT backend and its C/C++ factory APIs;
- the compiler and code-generator option path;
- the JSON description of the memory layout;
- the JSON `compute_cost` description emitted with that layout;
- unit, structural, differential, compile, and runtime tests;
- dedicated integration in `tests/impulse-tests`.

Only `mem0` is in scope. `-mem1`, `-mem2`, `-mem3`, their long aliases, and
the `iControl`/`fControl`/`iZone`/`fZone` models are explicit non-goals for this
port. They must remain rejected rather than silently behaving as `mem0`.

The parity baseline is the pinned C++ compiler's C++ backend. The pinned
compiler rejects `-mem0 -lang c`, despite dormant `gMemoryManager == 0` code in
the C container. Consequently:

- C++ is a parity-driven port with documented reference fixes;
- C is an `adapted` public surface and needs a native C ABI contract;
- Cranelift is an `adapted` Rust-native backend with no direct Faust C++
  Cranelift oracle; its storage semantics are checked against the canonical
  C/C++ allocation plan and its runtime behavior against the existing
  non-`mem0` JIT path;
- the C and Cranelift extensions must be listed in
  `porting/faust-rs-vs-faust-cpp-differences-en.md` when implemented.

This document plans the port; it does not enable the option or change generated
code yet.

## 2. User-visible contract

With `mem0`, arrays that would normally be embedded in the DSP object become
pointers. The host supplies a memory manager that:

1. receives a deterministic description of the externally allocated zones;
2. allocates the class/static zones;
3. allocates the DSP object and its instance zones;
4. releases the instance zones, DSP object, then class/static zones.

Scalar fields remain embedded in the DSP object. This is not a general-purpose
arena for every compiler temporary and it must not change the DSP's numeric
behavior.

For Cranelift, the logical DSP object is the native state block passed as
`dsp*` to finalized JIT functions. The Rust `CraneliftDspInstance` wrapper is
opaque cache/runtime bookkeeping and is not itself a described DSP allocation.
This adapted boundary is made explicit in D10.

The C++ documentation describes this host sequence:

```text
set manager
memoryInfo
classInit
create
instanceInit
compute ...
destroy
classDestroy
```

The Rust port must keep the ordinary Faust lifecycle contract as its stronger
invariant:

```text
init = classInit -> instanceInit
instanceInit = instanceConstants
             -> instanceResetUserInterface
             -> instanceClear
```

`mem0` changes where memory is stored and allocated, not the meaning or order
of lifecycle operations. The reference's empty `init` method is treated as a
known defect, not as the desired lifecycle contract; see section 4.3.

Generated C/C++ can perform class allocation and fill together in explicit
`classInit`. The existing Cranelift factory API creates an instance before its
sample-rate-bearing `init`; the recommended adapted implementation therefore
allocates shared class storage before the first instance block and fills it in
the later `classInit` step inside `init`. This split is observable in the
versioned allocation phase and is governed by D12; it does not permit compute
before successful initialization.

## 3. C++ reference analysis

### 3.1 Documentation contract

The architecture manual's custom-memory-manager section defines the C++
interface in `architecture/faust/dsp/dsp.h`:

```cpp
struct dsp_memory_manager {
    enum MemType {
        kInt32, kInt32_ptr,
        kFloat, kFloat_ptr,
        kDouble, kDouble_ptr,
        kQuad, kQuad_ptr,
        kFixedPoint, kFixedPoint_ptr,
        kObj, kObj_ptr,
        kSound, kSound_ptr
    };

    virtual void begin(size_t count) {}
    virtual void info(const char* name, MemType type,
                      size_t size, size_t size_bytes,
                      size_t reads, size_t writes) {}
    virtual void end() {}
    virtual void* allocate(size_t size) = 0;
    virtual void destroy(void* ptr) = 0;
};
```

`begin`/`info`/`end` describe the zones before allocation. `allocate` and
`destroy` perform the runtime operations. The generated C++ class exposes a
static `fManager`, `memoryInfo`, `classInit`/`classDestroy`, `create`/`destroy`,
and instance `memoryCreate`/`memoryDestroy` methods.

The documented legacy JSON item is:

```json
{
  "name": "fRec0",
  "type": "kFloat_ptr",
  "size": 16384,
  "size_bytes": 65536,
  "read": 1,
  "write": 1
}
```

In the reference serializer, a non-empty `memory_layout` also enables a
separate `compute_cost` object (`load`, `store`, `declare`, `number`, `cast`,
`select`, `loop`, `binop`, and `mathop`). The reference computes it by visiting
the generated scalar compute loop, then emits it as a one-element JSON array.
Although the source couples both objects through a serializer condition rather
than a memory-manager semantic dependency, this port deliberately emits
`compute_cost` whenever it emits the `mem0` layout.

### 3.2 Option and validation path

The reference implementation is distributed across these files:

| Source | Responsibility |
|---|---|
| `compiler/global.{hh,cpp}` | `gMemoryManager`, aliases, compatibility checks |
| `compiler/generator/code_container.{hh,cpp}` | `MemoryLayoutItem`, layout construction |
| `compiler/generator/struct_manager.hh` | field size and access summaries |
| `compiler/generator/instructions_complexity.hh` | scalar-loop `compute_cost` visitor |
| `compiler/generator/fir_to_fir.hh` | `ArrayToPointer` FIR rewrite |
| `compiler/generator/compile_scal.cpp` | generated static-table externalization |
| `compiler/generator/cpp/cpp_code_container.cpp` | C++ lifecycle and manager API emission |
| `compiler/generator/c/c_code_container.cpp` | dormant C memory-mode branches |
| `architecture/faust/dsp/dsp.h` | public C++ manager interface |
| `architecture/faust/gui/JSONUI.h` | `memory_layout` JSON serialization |
| `tests/impulse-tests/archs/impulsearch6.cpp` | C++ `-mem` impulse architecture |
| `tests/impulse-tests/archs/controlTools.h` | test memory-manager implementations |

`global.cpp` maps both `-mem` and `-mem0` to `gMemoryManager = 0`. It rejects:

- `-mem0` combined with `-it`;
- `-mem0` with the C backend;
- the other memory modes unless their own prerequisites are present.

The C rejection makes its `gMemoryManager == 0` container branches unreachable
from the supported CLI. Those branches are useful evidence about intended
array-to-pointer lowering, but they are not a stable C ABI to copy.

### 3.3 Layout construction

`CodeContainer::createMemoryLayout` runs after scalar FIR has been assembled.
It describes, in deterministic traversal order:

1. subcontainer objects used to construct static tables;
2. the associated class/static tables;
3. the main DSP object;
4. non-control scalar and array fields in the DSP object;
5. subcontainer objects used by instance initialization.

The access counts come from visiting a generated scalar compute loop. Scalar
field reads and writes are aggregated into the DSP-object entry; array counts
remain on their array entries.

Under `mem0`, `ArrayToPointer` rewrites every DSP-struct array declaration to a
pointer declaration. Ordinary accesses need no second rewrite because C/C++
indexing syntax is the same for an array and a pointer.

Only pointer-typed entries are passed to `memoryInfo`. Scalar entries still
appear in JSON, because they explain the full DSP state layout, but they are not
separate runtime allocations.

Runtime-generated static tables are also allocated through the manager. Literal
constant tables such as waveform constants that can stay compiled into the
program are not automatically made writable heap allocations.

### 3.4 Generated C++ lifecycle

The reference emits these operations:

- `memoryInfo()` calls `fManager->begin(n)`, one `info(...)` per allocation,
  then `end()`;
- `classInit()` allocates and fills generated shared tables;
- `create()` allocates the DSP object with placement `new`, then calls
  `memoryCreate()` for instance arrays;
- `instanceInit()` initializes constants, UI state, and delays;
- `destroy()` calls `memoryDestroy()`, the DSP destructor, then frees the
  object;
- `classDestroy()` releases generated shared tables.

Static tables and subcontainer objects use the same manager. Their entries are
distinguished from instance buffers by an undocumented convention: `size == 0`
for the object and static-table records, even when the table has a known number
of elements. Allocation code depends on entry order and separate code paths,
not on a complete typed allocation plan.

### 3.5 Observed baseline

The pinned `faust` binary (`2.84.3`) was exercised with small, import-free DSPs
and `-lang cpp -mem0 -json`:

| Case | Observation |
|---|---|
| waveform/static-table case | `memoryInfo` reports subcontainer objects, generated static tables, and the DSP object |
| multiple-delay case | DSP object plus three instance buffers are externally allocated |
| double precision | real buffers use `kDouble_ptr` |
| C backend | `-mem0/-mem1 cannot be used with 'c' backend` |
| `-it` | rejected together with `-mem0` |
| lifecycle | class tables are created by `classInit` and destroyed by `classDestroy` |
| `compute_cost` | emitted after `memory_layout` as one object inside an array; operation maps are lexically ordered |

The probe also confirmed that top-level JSON `size` and the DSP-object
`size_bytes` have different meanings. For one waveform program they were 8 and
24 respectively; for one delay program they were 28704 and 72. The top-level
value is therefore not `sizeof(mydsp)` and must not be reused as such.

For the waveform/static-table probe, the reference `compute_cost` contained
`load=14`, `store=5`, `declare=4`, `number=11`, `cast=2`, `select=0`, and
`loop=1`, with four integer binops and two named helper calls. These values form
one common-subset differential fixture; branch-heavy fixtures use the explicit
D6 correction policy instead of treating the reference defect as an oracle.

## 4. Defects and weaknesses in the reference

The port should preserve the useful public contract without copying the
following weaknesses.

### 4.1 Object-size heuristic

The reference estimates the C++ object size by summing fields after the
array-to-pointer rewrite, then adding 8 bytes for a presumed virtual pointer and
8 bytes for presumed alignment. This is not a target-ABI layout algorithm and
is wrong for some 32-bit ABIs, unusual alignments, inheritance configurations,
or code-generation options.

Plan:

- generated runtime `memoryInfo` uses `sizeof(DSPType)` and `sizeof(Element)`
  expressions wherever the target compiler knows the exact answer;
- JSON records the target ABI and exactness of numeric layout values;
- no `+ 8 + 8` heuristic is ported.

### 4.2 Static-table `size == 0` sentinel

The reference reports `size = 0` for a runtime-generated static table even when
its element count is known. Zero is overloaded as a lifetime/allocation marker.

Plan: report the real element count. Add explicit `scope`, `role`,
`runtime_allocated`, and allocation phase fields instead of a numeric sentinel.
This is an intentional reference fix.

### 4.3 Empty `init`

Both C++ and dormant C paths emit an empty `init` when a memory manager is
active. That contradicts the normal Faust lifecycle and makes an ordinary
architecture silently leave an allocated DSP uninitialized.

Plan: keep `init = classInit -> instanceInit`. Make class/static allocation
safe for the selected repeated-call policy, then prove the lifecycle with the
existing backend lifecycle-conformance style. This is an intentional reference
fix and must be registered as such.

### 4.4 Broken `clone`

The generated C++ `clone()` in `mem0` calls `create()` and returns the new,
uninitialized DSP. The source itself contains `TODO: deep copy would be needed
here`. Existing impulse coverage can hide the defect by initializing the clone
before use.

Plan: implement an independent clone with the same sample rate, scalar state,
UI state, and copied array contents. No pointer may alias an instance-owned
buffer in the source DSP. Static/class tables may remain shared. Add a runtime
clone-independence test.

### 4.5 Global mutable manager

`fManager` is a static mutable pointer. `create`, `destroy`, class tables, and
instance arrays consult it at the time of each call. Changing it after an
allocation can free memory through a different manager. Different factories or
threads cannot safely use distinct managers for the same generated class.

Plan: preserve the legacy C++ `fManager` entry point for source compatibility,
but bind each allocated object and class allocation set to the manager that
created it. Destruction must use the captured allocator. The exact compatible
surface is decision D2 in section 8.

### 4.6 Null and allocation-failure behavior

`memoryInfo` dereferences a null `fManager`. `create` applies placement `new` to
the allocation result without checking it. Failure while allocating a later
buffer leaks earlier allocations. `memoryDestroy` and `classDestroy` do not
clear pointers, so repeated destruction is unsafe.

Plan:

- fail generation or runtime use with an explicit contract when no manager is
  installed;
- allocate transactionally and unwind completed allocations in reverse order;
- null/reset released pointers;
- decide whether the legacy no-error C++ surface is retained with an additional
  checked API, or replaced, before implementation (D3);
- provide a deterministic failing allocator in tests.

### 4.7 Missing alignment contract

`allocate(size)` has no alignment parameter, yet placement-new and SIMD-capable
arrays may require alignment stricter than a byte allocator provides.

Plan: retain the legacy C++ callback for compatibility only if its documented
minimum alignment is made explicit. The C ABI should carry an alignment value
from its first version. A C++ opt-in aligned extension may be added without
removing the legacy virtual method. Code generation must never silently request
over-aligned storage from an interface that cannot promise it.

### 4.8 Weak access-count semantics

`read` and `write` are syntactic counts from one generated scalar-loop body.
They do not reliably weight nested loop trip counts, branch frequency, vector
lanes, or target-specific lowering.

Plan: preserve the values as a useful static estimate, but name and document
their semantics as `static_accesses_per_scalar_frame`. JSON retains legacy
`read`/`write` fields for compatibility and adds the metric definition at the
layout level. Tests verify deterministic counting, not a claim of measured
runtime traffic.

### 4.9 Weak upstream impulse checker

The reference `malloc_memory_manager_check` decrements an announced zone count
and sums bytes, but does not require the count to be zero at `end`, correlate
allocations with descriptions, detect leaks or double frees, verify reverse
destruction, poison memory, or test allocation failure.

The Rust port's impulse manager must cover all of these invariants.

### 4.10 Incomplete memory-type vocabulary

The FIR type system contains `Int64` and `Bool` array types. The reference
`Typed::gTypeString` can therefore produce `kInt64_ptr` or `kBool_ptr`, but
`dsp_memory_manager::MemType` and `MemoryLayoutItem::gStringType` contain
neither. The C++ allocation emitter also special-cases only `kInt32_ptr` and
casts every other buffer to the selected real type. An externally allocated
64-bit integer or boolean array can consequently produce an empty enum spelling
and/or a wrong pointer cast.

Plan: the memory analysis never falls through by type. The first implementation
either adds a reviewed, append-only vocabulary extension to the supplied
manager headers or rejects unsupported externalized element types before
emission with a stable diagnostic. It must not mislabel them as `kInt32_ptr` or
a real buffer. This is decision D5 in section 8.

### 4.11 Defective and incomplete `compute_cost` branch accounting

The reference `InstComplexityVisitor` intends to keep the more expensive branch
of an `IfInst`, but it has two independent defects:

- both the `then_branch` and `else_branch` visitors traverse `inst->fThen`;
- `cost()` always returns zero, so the comparison cannot select a more
  expensive branch.

It consequently counts the `then` branch regardless of the actual `else`
branch. The visitor also counts only `FloatNumInst`, `Int32NumInst`,
`BoolNumInst`, and `DoubleNumInst`; other numeric FIR variants are omitted. The
field called `mathop` counts every `FunCallInst`, not only mathematical
intrinsics. Loop bodies are visited once, so the report is a structural scalar
loop estimate rather than a dynamic instruction count.

Plan: preserve the legacy JSON field names and one-element-array shape, but use
an exhaustive, checked Rust FIR visitor with explicitly versioned semantics.
The proposed conditional rule is the component-wise maximum of both branches,
which is deterministic and needs no invented target-specific weight table.
Count every supported scalar literal kind, document `mathop` as function-call
count for compatibility, and reject unknown executable FIR nodes rather than
silently undercounting. The exact reference-fix policy is decision D6.

### 4.12 Current Cranelift storage and FFI gaps

These are not defects in the pinned C++ reference, because that compiler has no
Cranelift backend. They are current `faust-rs` limitations that block a real
`mem0` implementation:

- `CraneliftOptions` has no memory-manager mode;
- `StructLayoutPlan` stores all tables inline as
  `StructFieldKind::Table`, and lowering indexes directly into that payload;
- `DspStateBuffer` performs one `alloc_zeroed`/`dealloc` pair and its current
  clone copies only that contiguous block;
- writable runtime-generated static tables are JIT-owned data objects, not
  factory/class allocations owned by the host manager;
- the C++ wrapper implements `setMemoryManager` as a no-op and always returns
  `nullptr` from `getMemoryManager`;
- the C API has no `setCCraneliftMemoryManager` entry point, and the Cranelift
  FFI parity matrix currently marks the family deferred;
- factory JSON is a minimal hand-built status object and cannot describe a
  target layout or `compute_cost`;
- source/serialized factory rebuild and cache identity do not yet carry a
  memory layout mode;
- unsupported lowering may produce a no-op compute stub, which must never be
  mistaken for successful `mem0` runtime validation.

Plan: treat Cranelift as a third consumer of the canonical `Mem0Analysis`, not
as a separate allocator feature. The JIT layout, lowering, factory lifecycle,
FFI glue, JSON, cache/serialization policy, clone path, and impulse runner all
change together under the decisions in sections 6.7 and 8.

## 5. Current `faust-rs` boundary

### 5.1 Compiler and CLI

| Rust area | Current state | Required change |
|---|---|---|
| `crates/compiler/src/cli/args.rs` | no memory-manager option | add the four `mem0` aliases |
| `crates/compiler/src/cli/validate.rs` | no backend/memory validation | accept C, C++, and Cranelift; reject incompatible modes |
| `crates/compiler/src/cli/runner.rs` | compile-options assembly has no `-mem0` | emit canonical `-mem0` |
| `crates/compiler/src/cli/source_mode.rs` | no memory option passed to codegen | thread typed mode to C/C++/Cranelift and JSON |
| `crates/compiler/src/cli/fixture_mode.rs` | same gap | keep fixture path behavior consistent |
| `crates/compiler/src/json_naming.rs` | strict JSON has no memory or compute-cost analysis | make JSON backend- and `mem0`-aware |

The option must be explicit state passed down the pipeline. It must not become a
Rust equivalent of `gGlobal`.

### 5.2 FIR and code generation

`FirMatch::Module` already exposes the required structural inputs:

- `dsp_struct` for instance fields;
- `globals` and `static_decls` for shared/generated tables;
- `sub_modules` for table-producing helpers;
- lifecycle and compute functions for allocation phase and access analysis.

`FirType`, `AccessType`, canonical FIR builders/matchers, and the C-family
emitters provide enough information to implement `mem0` without adding a new
front-end or signal IR.

| Rust area | Current state | Required change |
|---|---|---|
| `crates/codegen/src/backends/c_family.rs` | shared expression/type helpers | add only language-neutral memory helpers |
| `crates/codegen/src/backends/cpp/mod.rs` | arrays embedded in class | pointer fields plus C++ manager/lifecycle emission |
| `crates/codegen/src/backends/c/mod.rs` | arrays embedded in struct | pointer fields plus an explicit C callback ABI |
| `crates/codegen/src/backends/cranelift/core.rs` | arrays are inline in `StructLayoutPlan` | pointer slots plus canonical zone bindings |
| `crates/codegen/src/backends/cranelift/lowering.rs` | table access assumes inline payload | load external buffer pointers before indexing |
| `crates/codegen/src/backends/cranelift/jit_data.rs` | generated writable tables are JIT data | keep literals in JIT data; move generated writable tables to class zones |
| `crates/cranelift-ffi` | one Rust-allocated state block; no manager ABI | factory/instance managed allocation sets and public glue |
| `CppOptions`, `COptions`, `CraneliftOptions` | no memory mode | carry typed `MemoryManagerMode` |
| codegen crate | no shared allocation description | add one backend-neutral memory-layout analysis |

The analysis belongs in `codegen`, close to the FIR representation consumed by
all three backends. It must not be duplicated inside C++, C, Cranelift, and
JSON.

### 5.3 JSON

`crates/codegen/src/json.rs` currently serializes name, metadata, I/O, UI, and a
single top-level `size`. `JsonBuildOptions` has no memory-layout or
instruction-complexity input.
`StrictJsonContext` currently obtains size information through a Wasm-oriented
path, which is not a C or C++ object-layout oracle.

The `mem0` work therefore needs a backend-specific layout input and a
backend-neutral scalar compute-cost input while preserving the current JSON byte
shape when `mem0` is not selected.

### 5.4 Cranelift JIT and FFI

The current native state contract is split across two crates:

- `codegen::backends::cranelift` owns JIT compilation, `StructLayoutPlan`,
  static JIT data, and field/table lowering;
- `cranelift-ffi` owns cached factories, `DspStateBuffer`, instances, clone,
  serialization, the C API, and the C++ wrapper header.

`JitDspModule` keeps finalized code and JIT data alive. Its state layout uses
`u32` offsets/sizes and distinguishes only inline scalars and inline tables.
`DspStateBuffer` then allocates one zero-filled native block. Under `mem0`, the
layout must contain pointer-sized slots for external buffers, with checked
links back to their owning `MemoryZone`; the runtime must own the main block
and the pointed-to allocations as one transactional set.

The effective Cranelift FIR may flatten submodules into merged struct fields.
Memory analysis must run on the same effective/flattened module snapshot that
the JIT lowers. It is incorrect to serialize a pre-flattening zone list while
allocating a post-flattening layout.

The existing factory cache key must include the semantic `mem0` option because
it changes machine code and state layout. It must not include an allocator
pointer or callback context: manager identity is runtime state, never compiled
code identity. Serialization persists the mode and layout contract but never
raw callback pointers, contexts, or live allocation addresses.

## 6. Target architecture

### 6.1 Typed option model

Introduce one shared public enum rather than backend-specific booleans:

```rust
pub enum MemoryManagerMode {
    None,
    Mem0,
}
```

Do not add future enum variants merely because C++ has `mem1`–`mem3`. Unknown
CLI spellings remain errors until a separate scoped port is approved.

The mode is threaded through the compiler request, C/C++/Cranelift backend
options, Cranelift FFI compile-argument parsing, and JSON build context. The
normal pipeline remains:

```text
parse -> boxes -> eval -> propagate -> normalize/type
      -> transform -> FIR -> effective-backend FIR
      -> memory-layout analysis -> C/C++/Cranelift and JSON
```

### 6.2 One canonical memory-layout analysis

Add a module such as `crates/codegen/src/memory_layout.rs` with typed data. The
exact Rust names may change, but the information must not:

```rust
pub struct MemoryLayout {
    pub version: u32,
    pub mode: MemoryManagerMode,
    pub target_abi: TargetAbi,
    pub access_metric: AccessMetric,
    pub zones: Vec<MemoryZone>,
}

pub struct MemoryZone {
    pub name: String,
    pub memory_type: MemoryType,
    pub scope: MemoryScope,
    pub role: MemoryRole,
    pub element_count: u64,
    pub element_size: u64,
    pub size_bytes: u64,
    pub alignment: u64,
    pub size_exact: bool,
    pub runtime_allocated: bool,
    pub allocation_phase: AllocationPhase,
    pub allocation_order: u32,
    pub reads: u64,
    pub writes: u64,
}
```

Required enums include:

- type: at least `Int32`, `Float32`, `Float64`, object, sound, and their pointer
  forms; `Int64`/`Bool` support follows D5 rather than an implicit fallback;
- scope/lifetime: `Temporary`, `Class`, `Instance`;
- role: `Subcontainer`, `StaticTable`, `DspObject`, `InstanceBuffer`,
  `EmbeddedScalar`;
- phase: `DescribeOnly`, `ClassCreate`, `ClassInit`, `CreateObject`,
  `InstanceCreate`. `ClassCreate` is needed for the documented Cranelift split
  between allocating class storage on first instance creation and filling it in
  the later semantic `classInit` step.

Properties:

- deterministic traversal and allocation order;
- checked `u64`/`usize` arithmetic, never C++ `int` accumulation;
- an error for unknown FIR nodes/types, overflow, unsized data, or unsupported
  over-alignment;
- scalar fields co-located with the DSP-object description rather than an
  index-based side table;
- explicit separation between all described fields and actual allocations;
- source-provenance Rustdoc pointing to `createMemoryLayout`,
  `StructInstVisitor`, and `ArrayToPointer`.

Allocation byte counts describe what is actually requested from the manager.
In particular, an empty logical Cranelift state still needs a stable non-null
`dsp*`: its main-object zone requests and reports one byte with target pointer
alignment rather than reporting zero and secretly allocating one. A zero-length
array is described with count/bytes zero but is not a runtime allocation; its
slot is null and any executable access is rejected as invalid FIR. These rules
remove allocator-dependent `allocate(0)` behavior and receive boundary tests.

Wrap the layout and the cost report in one immutable analysis snapshot, for
example:

```rust
pub struct Mem0Analysis {
    pub memory_layout: MemoryLayout,
    pub compute_cost: ComputeCost,
}
```

The same `Mem0Analysis` instance must drive:

1. pointer-vs-inline field emission;
2. `memoryInfo`/C description callbacks;
3. class and instance allocation/destruction code;
4. the Cranelift state layout, pointer-slot lowering, and factory allocation
   plan;
5. JSON `memory_layout` and `compute_cost`.

This prevents three independently implemented views from drifting.

### 6.3 Canonical `compute_cost` analysis

Add a sibling module such as `crates/codegen/src/compute_cost.rs`. The data model
keeps the legacy categories but uses checked counters and deterministic maps:

```rust
pub struct ComputeCost {
    pub version: u32,
    pub load: u64,
    pub store: u64,
    pub declare: u64,
    pub number: u64,
    pub cast: u64,
    pub select: u64,
    pub loops: u64,
    pub binop_total: u64,
    pub binops: BTreeMap<String, u64>,
    pub mathop_total: u64,
    pub mathops: BTreeMap<String, u64>,
}
```

The visitor analyzes the effective scalar `compute` loop only. It excludes
class initialization, instance initialization, UI construction, memory
allocation/destruction, and helper-function declarations unless a call to a
helper occurs in that compute loop. It traverses each FIR occurrence; it must
not deduplicate hash-consed `FirId` values.

Rust FIR loop nodes are a representation-level adaptation: `ForLoop` and
`SimpleForLoop` carry a bound/step from which the C-family emitter synthesizes
the loop-variable declaration, comparison, and update, whereas the C++ FIR
visited by `InstComplexityVisitor` contains those operations explicitly. The
cost visitor must therefore analyze the conceptual emitted scalar loop, not
merely count raw Rust nodes. For each loop form it deterministically accounts
for the synthesized declaration, loop-variable loads/store, comparison,
increment/decrement, and numeric constants exactly once, then visits the body
once. This normalization is shared by C, C++, and Cranelift and receives a
structural non-regression test; otherwise apparently valid JSON would
systematically undercount Rust loops while all runtime tests stayed green.

Required counting rules:

- `LoadVar` and `LoadTable` increment `load`; taking an address alone does not;
- `StoreVar` and `StoreTable` increment `store`;
- `TeeVar` increments `store` and visits its assigned value; taking its result
  does not add a separate load;
- variable and table declarations increment `declare`;
- every supported scalar literal increments `number`;
- conversions represented by `Cast` and the approved `Bitcast` policy
  increment `cast`;
- `Select2` and approved branching statement forms increment `select`;
- each supported loop node increments `loop`; explicit or synthesized control
  operations follow the loop-normalization rule above, and the body is visited
  once with no trip-count multiplication;
- each binary expression increments `binop.total` and one key named
  `Real(<op>)` when either operand is real, otherwise `Int(<op>)`;
- each FIR function call increments `mathop.total` and the exact callee-name
  key, preserving the legacy `mathop` name even for a non-math helper;
- conditions are counted once and conditional branches use the D6 aggregation
  policy;
- D6 takes the per-category and per-operation-key maximum across mutually
  exclusive branches, then recomputes `binop_total` and `mathop_total` from the
  merged maps; this is a conservative structural upper envelope and is not
  claimed to represent one dynamically taken path;
- all additions are checked and overflow is a typed code-generation error;
- every executable FIR variant is either counted/traversed by an explicit rule
  or rejected; `Unknown` is never ignored.

`BTreeMap` ordering reproduces the deterministic lexical order supplied by the
reference's `std::map`. For cases unaffected by the reference defects, the
legacy totals and operation keys are the differential target. Corrected branch,
literal, bitcast, or additional loop-node behavior is versioned and allowlisted
as a reference fix.

### 6.4 Target ABI and exact sizes

Array byte sizes are exact when element type and count are known. Object layout
requires target pointer width, scalar alignment, maximum alignment, and C++
object-model facts.

Add a `TargetAbi` model shared by C/C++/Cranelift layout and JSON. At minimum it
records:

- target triple or an explicit `native` marker;
- pointer size and alignment;
- integer and selected real sizes/alignments;
- maximum requested allocation alignment;
- whether a value is `computed`, `compiler_expression`, or `estimated`.

Generated source should use target-language `sizeof`/`alignof` expressions for
the runtime callbacks. JSON needs stable numbers, so the compiler-side model
must say whether those numbers are exact for the selected target. It must not
present a host estimate as an exact cross-target object size.

Decision D1 in section 8 selects whether the first implementation:

- requires/derives a concrete target ABI for `-json -mem0`; or
- permits unknown object size with an explicit non-exact marker.

For Cranelift JIT, use the effective native target configuration from
Cranelift as the authoritative pointer size/alignment source. Do not estimate it
from the host Rust process when an explicit target configuration disagrees.
Because the runtime calls finalized code in-process, reject a non-native JIT
target whose ABI cannot be executed safely. Convert canonical `u64` sizes and
offsets to Cranelift's current `u32` layout fields with checked conversions and
a typed error; truncation is forbidden.

### 6.5 C++ generated contract

Preserve the established source-level names needed by existing architectures:

```cpp
static dsp_memory_manager* fManager;
static void memoryInfo();
static void classInit(int sample_rate);
static void classDestroy();
static mydsp* create();
static void destroy(dsp* instance);
void memoryCreate();
void memoryDestroy();
```

The generated implementation changes are:

- every eligible FIR array field is emitted as an element pointer;
- runtime-generated class tables are manager allocations;
- `memoryInfo` is emitted from the canonical layout and uses exact
  `sizeof`/`alignof` where possible;
- allocation is transactional and destruction is reverse-order;
- instance and class allocations retain the manager that created them;
- pointers are reset after release;
- `init` preserves the Faust lifecycle contract;
- `instanceClear` initializes externally allocated buffers and does not rely on
  `calloc` behavior;
- `clone` deep-copies instance state and buffers.

The legacy `dsp_memory_manager` interface remains usable. Any aligned or checked
extension must be additive unless a separately approved compatibility change
says otherwise.

### 6.6 C generated contract

There is no supported pinned-C++ C ABI to copy. The recommended adapted ABI is
a plain C callback table carrying an opaque context:

```c
typedef enum {
    kMemInt32,
    kMemInt32Ptr,
    kMemFloat,
    kMemFloatPtr,
    kMemDouble,
    kMemDoublePtr,
    kMemObj,
    kMemObjPtr,
    kMemSound,
    kMemSoundPtr
} faust_memory_type;

typedef struct {
    uint32_t abi_version;
    size_t struct_size;
    void* context;
    void (*begin)(void* context, size_t count);
    void (*info)(void* context, const char* name, faust_memory_type type,
                 size_t count, size_t size_bytes, size_t alignment,
                 size_t reads, size_t writes);
    void (*end)(void* context);
    void* (*allocate)(void* context, size_t size, size_t alignment);
    void (*destroy)(void* context, void* ptr, size_t size,
                    size_t alignment);
} faust_memory_manager;
```

`abi_version = 1` and `struct_size` make this new adapted ABI additive rather
than freezing an unversioned callback-table size. Define the authoritative
`#[repr(C)]` Rust shape in `ffi-common::abi` and publish one guarded canonical
header such as `crates/ffi-common/include/faust-memory-manager.h`. Generated C
may reproduce that guarded definition when self-contained output is required;
the Cranelift C header must include or byte-for-byte mirror the same contract.
Add Rust `size_of`/`align_of`/`offset_of` checks plus C and C++ header-smoke
checks so the copies cannot drift.

Generated function names must remain class-namespaced, following the current C
backend convention. The likely surface is:

```c
void memoryInfoMyDsp(faust_memory_manager* manager);
void classInitMyDsp(faust_memory_manager* manager, int sample_rate);
void classDestroyMyDsp(faust_memory_manager* manager);
MyDsp* createMyDsp(faust_memory_manager* manager);
void destroyMyDsp(MyDsp* dsp);
```

The created DSP captures `manager`; `destroyMyDsp` does not accept a possibly
different manager. C code has no placement `new`, but follows the same object,
class-zone, and instance-zone allocation plan. `initMyDsp` retains the normal C
lifecycle behavior.

This ABI is a recommendation, not a settled parity fact. It must pass decision
D2 before public code is emitted.

### 6.7 Cranelift JIT, factory, and FFI contract

Cranelift consumes the same canonical zones but realizes them through a native
JIT state layout rather than generated source. Extend `CraneliftOptions` with
`memory_manager_mode` and retain the `Mem0Analysis` in `JitDspModule` (or an
equivalent immutable compiled plan) for the lifetime of the factory.

#### 6.7.1 State layout and lowering

Under `mem0`, replace each eligible inline table field with a pointer-sized
slot co-located with its canonical zone identity. One possible representation
is:

```rust
pub enum StructFieldKind {
    Scalar(FirType),
    Table { elem_type: FirType, len: u32 },
    ExternalBuffer {
        elem_type: FirType,
        len: u32,
        zone: MemoryZoneId,
    },
}
```

This is a representation-level adaptation and requires a structural test:
every externalized FIR array has exactly one `ExternalBuffer`, exactly one
runtime allocation zone, and no inline payload duplicate. The zone link stays
with `StructFieldLayout`; do not create a detached name/index side table.

`LoadTable` and `StoreTable` lowering for `AccessType::Struct` must distinguish
inline tables from external buffers. For an external buffer it loads the native
pointer from the state-block slot, then computes the element address. Scalar
and UI-zone offsets retain their ordinary contract. The finalized compute ABI
does not change:

```text
compute(dsp*, count, inputs, outputs)
```

The main state allocation therefore contains embedded scalar state and pointer
slots, not the external buffer payloads.

Writable `AccessType::Static` tables need synthetic, collision-free pointer
slots in that same layout because existing JIT functions receive `dsp*` but no
factory pointer. Their static load/store lowering uses those slots. Literal
immutable `AccessType::Static` tables continue to use `DataId`/`GlobalValue`.
The classification comes from the canonical zone role and initializer, not from
two independently maintained name sets.

#### 6.7.2 Instance ownership and clone

Replace the single-allocation `DspStateBuffer` path, only when `mem0` is active,
with an owned allocation set such as `ManagedDspState` containing:

- the manager-allocated main JIT state block;
- the ordered instance-buffer allocations;
- the captured callback table/context used to allocate them;
- enough canonical zone metadata to validate, unwind, clone, and destroy them.

Creation allocates transactionally, writes every external pointer into its
state-block slot, and unwinds in reverse order on failure. A manager is not
allowed to rely on zero filling: compiled `instanceConstants`,
`instanceResetUserInterface`, and `instanceClear` remain authoritative.
Destruction clears pointer slots where useful for diagnostics, releases
instance buffers in reverse order, then releases the main state block through
the captured manager.

Clone allocates a fresh main block and fresh instance buffers with the same
captured manager, copies scalar state and each buffer payload, rewrites all
pointer slots to the new addresses, and shares only approved class/static
tables. A byte copy of the old main block by itself is invalid because it would
alias all external buffers.

#### 6.7.3 Static tables and factory ownership

Literal immutable tables already represented as read-only JIT `DataId` objects
remain owned by `JITModule` and are not falsely described as manager
allocations. Writable tables whose contents are computed by `staticInit` move
to factory-owned class allocations. Their canonical zones use
`scope = class`, `role = static_table`, and `allocation_phase = class_init`.

The recommended JIT representation is one pointer slot per writable class table
in the ordinary native state layout, plus a factory-owned registry of the
allocated table addresses. To preserve the existing instance-oriented
`staticInit(dsp*, sample_rate)` ABI without a temporary hidden DSP allocation,
first instance creation transactionally allocates class storage before the main
instance block, records `allocation_phase = class_create`, and installs its
pointers into the instance slots. The first instance `init` then invokes
`staticInit` once to fill those tables before `instanceInit`. Later instances
install the same approved class addresses and do not refill them. This requires
decomposing the current `class_init_instance` helper so allocation and filling
are distinct and `staticInit` is no longer called per instance. Raw
process-global mutable pointers are not an acceptable shortcut.

Concurrent creation/first initialization uses a per-factory state machine
(`unbound/bound/allocating/allocated_uninitialized/initializing/ready/failed`,
or an equivalent checked model) so exactly one class allocation/fill is
published. Foreign callbacks and JIT initialization run outside the global
cache lock; a partial failure is unwound and never publishes half-initialized
class pointers. If the instance creation that caused class allocation later
fails, the implementation either transfers the completed class set to another
concurrent creator or releases it when no instance can own the transition. The
D4 policy decides how a later different sample rate is rejected or reinitialized
after all instances are gone.

The factory owns one `ManagedClassState`, including its captured manager. It is
allocated once under the D4 sample-rate/idempotency policy and released only
after all instances are gone. On final cache release, destruction order is:

```text
live instances -> instance zones -> class zones -> JIT module
```

Do not invoke foreign manager callbacks while holding the global factory-cache
lock. Allocation/destruction and description callbacks may re-enter host code;
the implementation must separate cache bookkeeping from callback execution or
document and test an equally safe synchronization boundary.

The initial Cranelift port does not need to add a public `classInit` or
`classDestroy` symbol solely for `mem0`: first instance creation reserves class
storage, existing instance `init` performs the idempotent semantic class-init
fill, and final factory release performs class destruction. This adapted
lifecycle choice is made explicit in D12 rather than being inferred from the
LLVM-only wrapper surface.

#### 6.7.4 Shared manager ABI

Cranelift must reuse the richer plain-C `faust_memory_manager` callback contract
from section 6.6; it must not introduce a second incompatible allocator
vocabulary. Put the ABI in `ffi-common` and a shared self-contained header when
possible. The callback table/context is copied into Rust-owned factory binding
state; no pointer to temporary stack glue is retained.

Add the C API setter locked by the existing naming matrix:

```c
bool setCCraneliftMemoryManager(
    cranelift_dsp_factory* factory,
    const faust_memory_manager* manager,
    char* error_msg);
```

The final error form may follow existing FFI conventions, but failure must be
observable. The setter validates/copies the callbacks, emits one deterministic
`begin`/`info`/`end` description sequence without allocating, and binds the
manager for later class/instance allocation. Reinstalling the same binding is
idempotent; conflicting rebinding is rejected while class allocations or live
instances exist.

`allocate` and `destroy` are mandatory; description callbacks may be optional
only if the shared ABI defines their absence precisely. No panic or C++
exception may unwind across the C/Rust boundary: Rust callback invocations use
the crate's FFI containment policy, and the C++ adapter catches exceptions and
turns them into a null allocation/binding diagnostic. Every returned pointer is
checked for the requested alignment before use; a misaligned result is released
through the same manager and treated as allocation failure.

The C++ `cranelift_dsp_factory::setMemoryManager` and `getMemoryManager` methods
must stop being stubs. The wrapper owns stable glue that adapts
`dsp_memory_manager` to the shared callbacks and remains alive with the factory.
The host manager itself must outlive every allocation it owns. The legacy
`MemoryManagerGlue` used by other dynamic APIs has only context,
`allocate(size)`, and `destroy(ptr)`; do not mutate that ABI in place. If an
adapter is offered, make it additive/versioned and reject alignments it cannot
promise rather than silently weakening the contract.

#### 6.7.5 Cache, serialization, and missing-manager policy

`-mem0` participates in compile options, SHA/cache identity, state layout, and
JIT code identity. The manager callback/context identity does not. A compiled
factory may therefore be cached before a manager exists, but it can have only
one active class-allocation binding at a time.

Factory compilation and JSON queries succeed without installing a manager,
because they need only the immutable allocation plan. Under `mem0`, class
initialization or instance creation without a bound manager fails explicitly;
there is no silent fallback to `std::alloc`. A rebuilt serialized factory
retains `-mem0`, its target/layout contract, and `compute_cost`, but starts
unbound and follows the same failure rule until a manager is installed. Raw
callbacks, contexts, and live pointers are never serialized.

The current `CraneliftDspInstance` Rust wrapper remains allocated by the
factory/cache runtime. The manager owns the logical JIT DSP state block and
every described FIR zone, not Rust's opaque wrapper bookkeeping. JSON and
description callbacks must say exactly that; see D10.

The existing Cranelift subset fallback remains independently visible through
`compute_body_lowered`. `compute_cost` always describes the requested effective
scalar FIR, never native instruction count or the no-op stub. `mem0` itself does
not silently change the general subset policy, but no `mem0` impulse/parity
success may be claimed unless `fail_on_subset_gap = true` and
`compute_body_lowered = true`; see D11.

### 6.8 JSON schema evolution

When `mem0` is absent, preserve the existing JSON shape exactly. When selected,
add a versioned description. Keep the six legacy item names so existing tools
can consume the common subset:

```json
{
  "memory_layout_version": 2,
  "memory_manager": {
    "mode": "mem0",
    "backend": "cpp",
    "manager_abi": "dsp_memory_manager_v1",
    "abi": {
      "target": "native",
      "pointer_size": 8,
      "pointer_alignment": 8
    },
    "access_metric": "static_accesses_per_scalar_frame"
  },
  "memory_layout": [
    {
      "name": "mydsp",
      "type": "kObj_ptr",
      "size": 1,
      "size_bytes": 72,
      "read": 4,
      "write": 2,
      "scope": "instance",
      "role": "dsp_object",
      "alignment": 8,
      "runtime_allocated": true,
      "allocation_phase": "create_object",
      "allocation_order": 0,
      "size_exact": true
    }
  ],
  "compute_cost_version": 2,
  "compute_cost_metric": "static_scalar_fir_structure",
  "compute_cost": [{
    "load": 14,
    "store": 5,
    "declare": 4,
    "number": 11,
    "cast": 2,
    "select": 0,
    "loop": 1,
    "binop": [{
      "total": 4,
      "Int(%)": 1,
      "Int(+)": 2,
      "Int(<)": 1
    }],
    "mathop": [{
      "total": 2,
      "max_i": 1,
      "min_i": 1
    }]
  }]
}
```

Rules:

- `memory_layout_version` is present only with a memory layout;
- `manager_abi` distinguishes generated C++'s legacy-compatible
  `dsp_memory_manager_v1` from the shared `faust_memory_manager_v1` used by C
  and Cranelift; it is not confused with the nested target-layout `abi` object;
- `name`, `type`, `size`, `size_bytes`, `read`, and `write` retain their legacy
  meaning where that meaning is sound;
- `size` consistently means an element/object count in v2, so the DSP object
  has count 1; the pinned reference's object value 0 is an allowlisted
  reference fix, like the corrected static-table count;
- a static table reports its actual element count, not zero;
- `scope` and `role` replace implicit ordering/sentinel conventions;
- `runtime_allocated` distinguishes embedded scalar descriptions;
- allocation order is stable and corresponds to generated callbacks;
- `compute_cost` is emitted whenever the `mem0` `memory_layout` is emitted and
  keeps the reference's one-element-array representation;
- its scalar counters retain the legacy names and order, while
  `compute_cost_version` and `compute_cost_metric` define the corrected static
  FIR semantics;
- `binop[0].total` and `mathop[0].total` equal the sum of their named entries,
  whose keys are serialized in lexical order;
- the same effective FIR/options produce the same `compute_cost` block for C,
  C++, and Cranelift, independently of object ABI/layout differences; if
  Cranelift flattening changes the analyzed FIR snapshot, that adaptation is
  identified rather than hidden behind a false equality claim;
- the JSON backend value is `c`, `cpp`, or `cranelift`, never a generic layout
  accidentally borrowed from Wasm;
- all integers use checked serialization and cannot wrap;
- `size_exact` plus ABI metadata exposes any target-layout limitation;
- top-level legacy `size` retains its existing meaning until a separately
  versioned migration is approved; it is not relabeled as object size.

`-json -mem0` without an explicit language uses the compiler's normal default
C++ backend. `-json -lang c -mem0` describes the C layout, and the Cranelift
factory JSON describes its native JIT layout. Selecting `mem0` for another
language is rejected before code generation.

Cranelift factory JSON must be built through the shared strict serializer, then
augmented with (or preserve) its existing backend status keys, including
`"backend":"cranelift"`, `jit_compiled`, and `compute_body_lowered`.
`memory_layout` is queryable before a manager is installed. It describes the
main state block and separately allocated class/instance zones; it does not
include the opaque Rust factory/instance wrapper or JIT code/data pages. The ABI
object additionally records the effective Cranelift target/native pointer
layout so its exact numeric sizes are auditable.

## 7. Phase 0 gate for this subsystem

The mandatory Phase 0 checks are narrow and must be recorded before emitter
implementation.

### P0.1 Effective pipeline confirmation

Confirm with one traceable fixture that the production path is:

```text
source -> FIR module -> selected effective backend FIR -> memory analysis
                                                    \-> C/C++ emitter
                                                    \-> Cranelift JIT layout
                                                    \-> strict JSON
```

The memory option must not modify parsing, evaluation, propagation,
normalization, typing, or the semantic FIR algorithm.

### P0.2 Differential baseline

Capture the pinned compiler's C++ output and JSON for the corpus in section 10. Classify
every difference as:

- formatting-only;
- representation adaptation with the same contract;
- intentional reference fix from section 4;
- unsupported and blocking.

There is no C `mem0` oracle. C tests use C++ semantic allocation plans plus
compiled runtime parity, not byte comparison against unreachable reference
code.

There is also no Faust C++ Cranelift oracle. Capture the current non-`mem0`
Cranelift layout, factory JSON, cache/serialization behavior, lifecycle, and
impulse output before changing them. Cranelift `mem0` is compared semantically
with the canonical C/C++ zones after accounting for documented effective-FIR
flattening, and numerically with the ordinary Cranelift backend.

### P0.3 `gGlobal` decomposition

Record the option-data path from CLI request to backend options. Passing
`MemoryManagerMode` and `TargetAbi` explicitly closes this item; no mutable
process global is permitted.

### P0.4 TreeArena performance

This port does not change TreeArena nodes, interning, or hash-consing. Mark the
TreeArena validation item not applicable with that evidence. Do not run a new
hash-consing experiment unless the implementation unexpectedly changes FIR or
tree representation.

### P0.5 Lifecycle, ownership, and ABI

Resolve decisions D1–D12 below, document ownership of C/C++ and Cranelift class
and instance zones, and define failure unwinding before generating a public
API. For Cranelift, include cache-lock/callback boundaries, factory rebinding,
serialized-factory rebinding, wrapper-vs-state ownership, and final-release
ordering.

Pass criterion: all five items are recorded in the implementation journal or a
small phase report, and D1–D12 have explicit answers.

## 8. Decisions required before implementation

These are parity- or ABI-sensitive and must not be selected silently during
coding.

### D1. JSON target layout

Recommended: introduce an explicit `TargetAbi` used by C, C++, Cranelift, and
JSON; default to the compiler's selected/native target and mark exactness per
field. Reject a request that needs an exact object size when the target layout
is unknown.

Alternative: allow `size_bytes` to be absent for unknown object layouts. This
is cleaner semantically but breaks consumers that assume the legacy field is
always numeric.

### D2. C and C++ manager binding

Recommended:

- C++ keeps `fManager` and no-argument legacy methods, but captures the manager
  used for each object/class allocation set;
- C always receives an explicit callback table/context and captures it in each
  DSP object;
- aligned callbacks are native in C and additive in C++.

This fixes wrong-manager destruction while preserving the established C++
source contract.

### D3. Allocation failure surface

Recommended: retain legacy C++ `create()` for compatibility and add a checked
creation path used by new architectures; make the C creation function return
`NULL` on failure. Both paths unwind partial allocations. Decide whether legacy
C++ `create()` returns `nullptr`, throws, or delegates to the checked API and
terminates according to the architecture policy.

### D4. Class lifecycle policy

Recommended: class allocation is idempotent for the same manager and sample
rate, conflicting manager replacement is rejected while class allocations are
live, and `classDestroy` is legal only after the last instance is destroyed.
The checked path tracks this precondition; the legacy path documents it and
must not turn an early call into a silent use-after-free. A successful
`classDestroy` clears all class pointers. `init` remains
`classInit -> instanceInit`.

This avoids the reference's empty `init` without allocating a second copy of
static tables for every instance. One class/static allocation set is bound to
one manager at a time; individual instance allocations may use distinct manager
contexts on the generated C/C++ checked creation path once D2 defines it.
Cranelift instead binds one manager to the cached factory and all of its live
instances under D9.

### D5. `Int64` and `Bool` memory types

Recommended for the first scalar gate: append `Int64`, `Int64Ptr`, `Bool`, and
`BoolPtr` to the versioned Rust/C memory vocabulary, but preserve the numeric
values and spellings of all legacy C++ enum members. Generated C++ that actually
uses a new member requires a faust-rs-supplied compatible header; an architecture
using the unextended upstream header receives a capability diagnostic rather
than invalid code.

The narrower alternative is to reject these externalized array types for all
`mem0` C/C++/Cranelift output. Whichever policy is selected must have generator,
header-compile, JIT-lowering, JSON, and impulse coverage.

### D6. Corrected `compute_cost` semantics

Recommended and assumed by this plan:

- emit the legacy object keys and one-element-array shape;
- identify the semantics with `compute_cost_version = 2` and
  `compute_cost_metric = "static_scalar_fir_structure"`;
- use a component-wise maximum for mutually exclusive branches after counting
  the condition once; merge operation maps by per-key maximum and recompute
  their totals so the sum invariants remain true;
- count all scalar literal variants supported by the effective C/C++/Cranelift
  FIR path;
- count both value casts and bitcasts in `cast`;
- count `Select2`, `If`, `Control`, and `Switch` as selections, with explicit
  branch aggregation for each;
- count every supported FIR loop form once, including the declaration,
  comparison, and update synthesized by the C-family emitter, and visit its body
  once;
- retain `mathop` as the compatibility name for all function calls.

The alternative is byte-value parity with the defective C++ branch/literal
behavior. It is not recommended because it makes the report silently dependent
on branch ordering and omits valid instructions. The selected corrections must
be recorded as reference fixes and covered by focused differential fixtures.

### D7. Cranelift static-table indirection

Recommended: keep literal immutable tables as JIT-owned read-only data, but
externalize every writable table filled by `staticInit` into a factory-owned
class zone. Finalized code reaches it through synthetic pointer slots in the
native DSP state. The first initialized instance fills the tables once;
subsequent instances install the same factory-owned addresses without refilling
them. The existing finalized function ABI does not change.

Alternative: import mutable process-global pointer symbols into the JIT. It is
not recommended because it recreates the C++ global-manager coupling, makes
cache reuse/context isolation harder, and obscures factory ownership.

### D8. Cranelift manager ABI and description timing

Recommended: reuse the section 6.6 aligned callback ABI through `ffi-common`,
add `setCCraneliftMemoryManager`, copy its callback table/context by value into
factory-owned state, and have a successful setter immediately emit exactly one
`begin`/`info`/`end` description sequence without allocating. The C++ wrapper
adapts `dsp_memory_manager` through factory-lived glue and implements a real
getter.

The legacy `MemoryManagerGlue` shape is not changed in place. An additive
legacy adapter may be supplied only when its alignment limits are explicit.
The alternative is a separate describe function, but it adds another ordering
state and must prove why setter-time description is insufficient.

The shared ABI does not claim that an arbitrary opaque manager context is
thread-safe. Factory state synchronization protects binding/class publication,
but callbacks execute outside global and per-factory state locks. If a host
creates or deletes instances concurrently, its callbacks/context must be
thread-safe or externally serialized. Document and exercise reentrant callbacks
so the implementation cannot accidentally rely on a lock being held.

The C++ setter has a legacy `void` return. Recommended failure mapping: a
conflicting/invalid replacement leaves the previous valid binding unchanged,
records a factory diagnostic, and never partially installs new glue;
`getMemoryManager` returns the effective binding. The C setter remains the
checked surface with an observable result.

### D9. Cranelift missing manager, cache, and rebinding

Recommended: JIT compilation, cache insertion, and JSON query succeed without a
manager. Under `mem0`, class initialization and instance creation fail with a
typed/FFI-visible error until a manager is installed; there is no Rust allocator
fallback. The semantic mode is part of the compiled SHA/cache/serialized
identity, while callback addresses and contexts are not. A deserialized factory
starts unbound. Rebinding to a different manager is rejected while class zones
or instances exist, and foreign callbacks are never made under the global cache
lock.

### D10. Cranelift DSP-object ownership boundary

Recommended: classify the main native JIT state block as the `dsp_object`
allocation. Keep `CraneliftDspInstance` and other Rust opaque bookkeeping under
the FFI/cache allocator, outside `memory_layout`. The manager therefore owns
every address visible to the JIT as DSP state, but it does not allocate or free
Rust structs whose layout and drop glue are not a public ABI.

This is an `adapted` compatibility surface and must be recorded in the
difference registry. The alternative—placing the Rust wrapper itself in host
memory—would require a new unsafe layout/drop/cache contract and is not
recommended for this phase.

### D11. Cranelift subset fallback and `compute_cost`

Recommended: keep the existing public subset policy independent from `-mem0`;
`compute_cost` describes the requested effective scalar FIR even if the backend
reports `compute_body_lowered = false`. However, every `mem0` impulse,
differential, and completion gate enables `fail_on_subset_gap` and requires
`compute_body_lowered = true`. Stub execution cannot establish allocator or
numeric parity.

The stricter alternative is for `-mem0` to force subset failure globally. It
reduces false positives but unexpectedly changes an unrelated Cranelift option;
choose it only as an explicit compatibility decision.

### D12. Cranelift public class-lifecycle surface

Recommended for this phase: do not add new public `classInit`/`classDestroy`
symbols. `setMemoryManager` describes and binds but does not allocate. First
instance creation allocates class storage before instance storage; `init` fills
that storage through an idempotent factory-owned class-init step before
`instanceInit`; final factory release destroys class zones after deleting live
instances. JSON/trace records Cranelift's `class_create` allocation phase rather
than falsely claiming the bytes were allocated during `classInit`.

This differs from the generated C++ architecture sequence and must be marked
`adapted` in the parity matrix/difference registry. The alternative is to add an
LLVM-style explicit class-init API; that expands the public ABI and requires
defined interaction with instance `init`, cache release, and repeated sample
rates before implementation.

## 9. Porting phases

### M0 — Baseline and contract freeze

Deliverables:

- complete the Phase 0 items;
- capture C++ code/JSON baselines and current Cranelift layout, JSON,
  cache/serialization, lifecycle, and impulse baselines for the test matrix;
- approve D1–D12;
- define exact accepted/rejected CLI combinations;
- add a compatibility classification table (`1:1`, `adapted`,
  `reference-fix`, `deferred`).

The table must classify the legacy `compute_cost` block shape as `1:1`, the
additive version/metric fields as `adapted`, and the D6 counting corrections as
`reference-fix`. It classifies the Cranelift manager API, JIT pointer-slot
layout, and D10 wrapper boundary as `adapted`.

Pass criterion: no public ABI, JSON schema, or lifecycle ambiguity remains.

### M1 — Typed option plumbing

Deliverables:

- add `MemoryManagerMode::{None, Mem0}`;
- parse the four aliases and normalize them to `-mem0`;
- pass mode/ABI through compiler requests and C/C++/Cranelift options;
- teach `parse_ffi_compile_args` and every Cranelift source/file/serialized
  factory path to preserve the semantic mode;
- reject `mem1`–`mem3`, `-it -mem0`, unsupported languages, and invalid mode
  combinations with stable diagnostics;
- add CLI unit and end-to-end tests.

Pass criterion: the option reaches a test backend context without changing
ordinary generated code. All four aliases produce the same effective options.

### M2 — Canonical memory and compute-cost analyses

Deliverables:

- implement the typed layout model and target-ABI calculations;
- define how analysis receives the effective FIR snapshot used by each backend,
  including Cranelift submodule flattening;
- identify embedded scalars, instance arrays, main object, runtime-generated
  static tables, and subcontainers;
- count accesses with documented scalar-frame semantics;
- implement the exhaustive scalar-FIR `ComputeCost` visitor and D6 branch
  aggregation;
- produce checked totals plus deterministic binop/function breakdown maps;
- produce deterministic allocation phases and order;
- reject overflow, unsupported types, and unknown FIR constructs;
- add Rustdoc provenance and structural non-regression tests.

Pass criterion: one immutable `Mem0Analysis` per effective backend FIR snapshot
completely determines C/C++ emission or Cranelift layout/lowering plus JSON. No
backend reconstructs its own zone list or recomputes the cost report.

### M3 — C++ emitter

Deliverables:

- pointer-field emission for eligible arrays;
- C++ manager description and allocation methods;
- static-table/subcontainer allocation;
- manager capture, transactional failure unwinding, reverse destruction, and
  pointer reset according to D2–D5;
- lifecycle-conformant `init`;
- deep `clone`;
- single and double precision generation tests.

Pass criterion: generated C++ compiles warning-free, passes lifecycle/runtime
tests, and is numerically identical to ordinary C++ output on the selected
impulse corpus.

### M4 — C emitter and ABI

Deliverables:

- publish the approved callback/context types in the self-contained generated
  C contract or its minimal architecture header;
- publish/version the authoritative `ffi-common` Rust/header ABI and add
  cross-language layout smoke tests;
- pointer-field emission using the same layout;
- namespaced describe/class/create/destroy functions;
- the same failure, lifecycle, and destruction guarantees as C++ where C can
  express them;
- C and C++ compilation tests for the header boundary.

Pass criterion: generated C is valid C, not C++ disguised as C; two concurrent
DSPs can use independent manager contexts; runtime output matches the ordinary
C backend.

### M5 — Cranelift JIT layout, runtime ownership, and FFI

Deliverables:

- add the typed mode and retained canonical analysis to the Cranelift compile
  result;
- extend `StructLayoutPlan` with pointer-sized external-buffer fields linked to
  their `MemoryZone`, using checked native target sizes/offsets;
- lower struct table loads/stores through pointer slots while preserving scalar
  and UI-zone offsets and the existing compute ABI;
- keep literal read-only JIT tables unchanged and implement D7 for writable
  generated class tables, including class-storage allocation before the first
  instance block and one-time fill during the first semantic `classInit`;
- replace the mem0 instance path with transactional managed state, captured
  callbacks, reverse destruction, poison-safe initialization, and deep clone;
- add factory-owned class allocation state and enforce D4/D9 destruction order;
- publish the shared manager ABI through `ffi-common`/headers and implement
  `setCCraneliftMemoryManager` plus real C++ `setMemoryManager`/getter behavior;
- include `mem0` in compile arguments, SHA/cache identity, and serialization,
  while excluding manager addresses and restoring factories unbound;
- keep foreign callbacks outside the global factory-cache lock;
- add Rustdoc provenance from the adapted implementation to the current
  `StructLayoutPlan`, `define_static_tables_in_jit`, `DspStateBuffer`, and
  `class_init_instance` contracts, while stating that no C++ Cranelift source
  oracle exists;
- update `porting/cranelift-dsp-ffi-parity-matrix-en.md` so memory-manager
  set/get is required for this mode rather than stale `v1-deferred` debt.

Pass criterion: a compiled factory can expose its layout without a manager,
creation fails clearly while unbound, a bound manager owns all described JIT
state/class/instance zones, multiple instances and clone are independent, final
release destroys instances then class state then the JIT module, and ordinary
non-`mem0` Cranelift behavior remains unchanged.

### M6 — JSON v2 memory and `compute_cost` description

Deliverables:

- add versioned `memory_manager` and `memory_layout` serialization;
- identify both the manager callback ABI and target layout ABI;
- retain legacy item keys;
- add scope, role, alignment, exactness, phase, and order;
- correct static-table element counts;
- emit `compute_cost_version`, `compute_cost_metric`, and the legacy-compatible
  `compute_cost` block from the M2 analysis;
- preserve scalar key order, one-element `binop`/`mathop` arrays, exact operation
  names, and deterministic breakdown order;
- select the C, C++, or native Cranelift ABI model from the effective backend;
- replace the Cranelift factory's hand-built JSON path with the shared strict
  serializer while retaining `jit_compiled` and `compute_body_lowered`;
- keep non-`mem0` JSON byte-stable;
- add schema/semantic tests rather than string grep tests.

Pass criterion: every runtime allocation maps one-to-one, in order, to one JSON
runtime zone and one generated/FFI description callback. Every `compute_cost`
total matches its breakdown; C, C++, and Cranelift emit the same cost for the
same effective FIR; and focused common-subset fixtures match the pinned C++
counts. JSON parses with `serde_json` and remains deterministic across runs.

### M7 — `tests/impulse-tests` integration

Deliverables are detailed in section 10. At minimum add:

- `Make.mem0`;
- `cpp-mem0`, `c-mem0`, `cranelift-mem0`, and `all-mem0` targets wired into
  the top Makefile;
- self-contained C/C++ manager drivers and a Cranelift audit-manager runner;
- import-free `mem0` DSP fixtures;
- runtime allocation auditing and semantic JSON checking;
- README/help documentation.

Pass criterion: all three backends run the representative supported corpus
through manager allocation and produce the same impulse output as their
non-`mem0` forms. Cranelift runs strict lowering and accepts no compute stub.

### M8 — Differential and hardening gate

Deliverables:

- pinned-C++ differential for C++ code shape and legacy JSON fields;
- exact `compute_cost` differential on the common FIR subset and focused
  allowlisted D6 differentials for branch/literal/control corrections;
- allowlist only the approved fixes from section 4;
- normal vs `mem0` numeric parity for C, C++, and Cranelift;
- Cranelift allocation-plan parity against C/C++ after documented flattening,
  plus cache/rebinding/serialization/final-release tests;
- optimized vs unoptimized generated-runtime parity on the representative
  subset;
- sanitizers where supported;
- compilation-cost gate.

Pass criterion: no unexplained differential remains, no leak/double free is
reported, and all repository quality gates pass.

### M9 — Compatibility and documentation closeout

Deliverables:

- update the compiler/backend/Cranelift FFI README and CLI help;
- update `porting/faust-rs-vs-faust-cpp-differences-en.md` for C and Cranelift
  support and every intentional reference fix;
- update `porting/cranelift-dsp-ffi-parity-matrix-en.md` and the Cranelift
  backend plan to reflect the implemented memory-manager family;
- update the relevant daily journal with exact validations;
- update `porting/HANDOFF.md` if the work spans sessions;
- document all public C/C++/Cranelift APIs as `1:1`, `adapted`, or
  `reference-fix`.

Pass criterion: no implemented behavioral difference or public ABI is missing
from the registry and documentation.

## 10. Test plan

### 10.1 Unit and structural tests

Add tests close to the owning code:

- compiler CLI: every alias, canonical compile-options string, backend
  acceptance/rejection, `-it` conflict, `mem1`–`mem3` rejection;
- memory analysis: stable order, types, roles, scopes, counts, checked sizes,
  exactness, nested subcontainers, single/double precision, empty DSP state,
  and zero-length arrays;
- compute-cost analysis: every counter category, integer/real binop
  classification, function-name breakdown, checked overflow, stable lexical
  ordering, and rejection of unknown executable nodes;
- compute-cost loop adaptation: compare a conceptual C++-style expanded loop
  with the compact Rust `ForLoop`/`SimpleForLoop` forms and require identical
  counters, including declaration, comparison, and update overhead;
- compute-cost branching: an asymmetric `if` whose expensive branch is the
  `else`, the inverse ordering, `Select2`, `Control`, and multi-case `Switch`,
  proving the D6 result does not reproduce the C++ `fThen`/zero-cost bug;
- C++ emitter: pointer declarations, exact `sizeof` callback arguments,
  allocation/unwind order, manager capture, lifecycle, clone code;
- C emitter: C-only declarations, callback context, aligned allocation,
  namespacing, failure paths;
- Cranelift layout: inline-to-pointer-slot conversion, pointer-size/alignment,
  checked `u64` to layout-offset conversion, and zone identity co-location;
- Cranelift lowering: loads/stores through external pointer slots, unchanged
  scalar/UI offsets, and literal-JIT-data versus writable-class-table routing;
- Cranelift FFI/runtime: setter description timing, missing manager, manager
  capture, Nth-allocation unwind, final-release order, clone pointer rewriting,
  multiple factories/managers, cache rebinding, and callback reentrancy outside
  the cache lock;
- Cranelift lifecycle conformance: `init` performs one class fill before
  `instanceInit`, `instanceInit` never refills class state, compiled
  `instanceConstants`/`instanceClear` remain authoritative, and no ad-hoc
  zeroing substitutes for them;
- Cranelift cache/serialization: `mem0` changes semantic SHA identity, manager
  addresses do not, serialized mode/layout survives, and rebuilt factories are
  unbound;
- JSON: old shape without `mem0`, v2 shape with `mem0`, backend-specific ABI,
  real static-table counts, `compute_cost` schema/totals/operation maps,
  deterministic ordering, and overflow errors;
- structural adaptation: every externalized FIR array has exactly one owned
  allocation zone and no embedded duplicate.

Use canonical FIR builders and matchers. Do not create an index side table that
can silently detach allocation metadata from the owning field.

### 10.2 Impulse fixtures

Add a small import-free group under `tests/impulse-tests/dsp/`. The exact names
can follow local naming convention, but the corpus must cover:

1. a DSP with scalar state and no externalizable array;
2. multiple delay buffers of different sizes;
3. integer and real arrays;
4. a runtime-generated static table and its subcontainer;
5. an instance-created table/subcontainer if the FIR path distinguishes it;
6. UI controls alongside arrays, proving control scalars are embedded;
7. single and double precision;
8. a stateful DSP whose clone can be advanced independently;
9. casts, integer and real binary operations, helper/math calls, and asymmetric
   conditional branches giving a non-trivial `compute_cost` oracle.

At least one table fixture must distinguish an immutable literal table from a
writable table filled by `staticInit`, so Cranelift proves that only the latter
moves from JIT data to a manager-owned class zone. Fixtures selected for the
Cranelift gate must be inside its real lowering subset; no fixture may pass via
the no-op fallback.

Tests must be self-contained: no installed Faust libraries and no copied
standard-library tree. Compact table/delay behavior is defined directly in the
fixture source, following the repository test rule.

### 10.3 Impulse build integration

Add `tests/impulse-tests/Make.mem0` and top-level targets:

```text
make -C tests/impulse-tests cpp-mem0
make -C tests/impulse-tests c-mem0
make -C tests/impulse-tests cranelift-mem0
make -C tests/impulse-tests all-mem0
```

The generated program uses dedicated self-contained architecture files, for
example:

```text
tests/impulse-tests/archs/faust_mem0.h
tests/impulse-tests/archs/impulsemem0.cpp
tests/impulse-tests/archs/impulsemem0.c
```

Extend `tests/impulse-tests/Make.cranelift` or include it from `Make.mem0` with
a separate `cranelift-mem0` output directory. Extend `impulse_cranelift` with
an audit-manager mode that passes canonical `-mem0` into the factory compile
arguments, installs the shared callback table before instance creation, enables
strict subset failure, and rejects `compute_body_lowered == false`. Its first
gate may use the existing scalar/double Cranelift subset; single precision is a
separate required unit/runtime case even if the historical impulse target stays
`-double`.

Reuse the minimal local Faust API headers already present where appropriate;
do not depend on `/usr/local/share/faust`.

The normal impulse `.ir` output remains the numeric oracle. Memory placement is
not allowed to change samples, UI evolution, or lifecycle-visible state.

### 10.4 Auditing memory manager

The C/C++ drivers and Rust Cranelift runner emit one canonical, portable trace
schema (prefer JSON lines with backend, event, zone, phase, size, alignment,
pointer identity token, and manager identity token). A shared Rust checker
validates it; pointer values themselves are never stored as golden data. The
impulse manager must record descriptions and live allocations. It verifies:

- `begin(count)` equals the number of following `info` calls;
- `end()` observes exactly zero unconsumed descriptions;
- descriptions, runtime allocations, sizes, alignments, phases, and order
  agree;
- class tables allocate once per approved lifecycle and release once;
- `classDestroy` cannot invalidate tables while an instance is live;
- multiple instances repeat only instance zones;
- instance destruction and partial-failure unwinding are reverse-order;
- every pointer is destroyed by the manager/context that allocated it;
- no leak, double free, unknown pointer, or use-after-destroy remains;
- a non-zero poison fill proves initialization does not rely on `calloc`;
- failure on the Nth allocation returns the approved error and leaks nothing;
- repeated lifecycle calls follow D4;
- C++ and Cranelift clone buffers do not alias and state evolves independently;
- compiling/querying a Cranelift factory performs no allocation before manager
  binding, while the setter describes without allocating;
- first Cranelift instance creation allocates class zones before its main state
  and instance zones, while first `init` fills but does not reallocate them;
- a serialized/rebuilt Cranelift factory cannot allocate until rebound;
- final Cranelift factory release destroys live instances first, class tables
  second, and only then drops the JIT module.

At program exit, the live-allocation map and live-class-zone map must both be
empty. A successful sample comparison without these checks is not a sufficient
`mem0` test.

### 10.5 JSON checks in impulse tests

Generate JSON for all three backends and parse it semantically with a small Rust
checker (preferably an `xtask` subcommand or reusable test binary using
`serde_json`). Do not validate JSON with `grep`.

For each fixture, assert:

- version, mode, backend, manager ABI, target ABI, and access metric;
- unique stable zone names/order;
- exact role/scope/runtime-allocation classification;
- element counts and byte sizes for known arrays;
- corrected nonzero size for generated static tables;
- one-to-one correspondence with the runtime manager trace;
- `compute_cost_version`, metric name, fixed scalar keys, and one-element array
  shape;
- `binop.total`/`mathop.total` equality with the sum of named entries and lexical
  entry ordering;
- equality of C, C++, and Cranelift compute-cost reports for the same effective
  FIR, with any flattening adaptation stated explicitly;
- preservation of Cranelift's `jit_compiled`/`compute_body_lowered` status keys,
  native target ABI, and exclusion of Rust wrapper/JIT code pages;
- pinned-C++ equality on fixtures unaffected by D6, plus an explicit allowlist
  for each corrected branch/literal/control case;
- unchanged legacy JSON shape when compiled without `mem0`.

For C++, compare legacy fields against the pinned compiler and maintain a small,
reviewed allowlist for the object-size, static-count, lifecycle, and other
approved fixes. For C, compare the semantic layout with the corresponding C++
FIR zones while allowing ABI-specific object size/name differences. For
Cranelift, compare canonical roles/counts/cost with C/C++ and compare exact
state-block offsets/sizes with `StructLayoutPlan`; allow only documented
flattening, native ABI, and D12 `class_create` versus C/C++ `class_init` phase
differences.

### 10.6 Optimization and platform coverage

Run a representative stateful subset with unoptimized and maximum-optimized
generated programs/JIT modules. The samples, manager trace, memory layout, and
FIR-level `compute_cost` must agree; Cranelift optimization may change native
machine code but not the reported structural metric.

Compile generated code as both C and C++ and run native Cranelift on supported
CI platforms. Path and process handling in the Make integration must remain
portable. Sanitizer targets are useful on Linux/macOS but must not make Windows
CI depend on unavailable flags. Cranelift JIT tests reject non-native target
configurations rather than executing mismatched code.

## 11. Expected file map

The implementation is expected to touch or add files in these areas; exact
placement can be adjusted to crate boundaries:

```text
crates/codegen/src/memory_layout.rs                 new shared analysis
crates/codegen/src/compute_cost.rs                  scalar FIR cost analysis
crates/codegen/src/lib.rs                           public/internal exposure
crates/codegen/src/json.rs                          v2 serialization
crates/codegen/src/backends/c_family.rs             shared memory helpers
crates/codegen/src/backends/cpp/mod.rs               C++ emission
crates/codegen/src/backends/c/mod.rs                 C emission and C ABI
crates/codegen/src/backends/cranelift/core.rs         mode/layout contract
crates/codegen/src/backends/cranelift/api.rs          retained analysis/JIT build
crates/codegen/src/backends/cranelift/jit_data.rs     static-table ownership split
crates/codegen/src/backends/cranelift/lowering.rs     pointer-slot table access
crates/codegen/src/backends/cranelift/tests.rs        structural/lowering tests
crates/ffi-common/src/abi.rs                          shared repr(C) manager ABI
crates/ffi-common/include/faust-memory-manager.h      canonical public C contract
crates/cranelift-ffi/src/types.rs                     managed class/instance state
crates/cranelift-ffi/src/factory.rs                   options/cache/JSON/manager binding
crates/cranelift-ffi/src/instance.rs                  lifecycle/clone/destruction
crates/cranelift-ffi/src/clif.rs                      serialized mem0 contract
crates/cranelift-ffi/include/cranelift-dsp-c.h        C setter/shared ABI exposure
crates/cranelift-ffi/include/cranelift-dsp.h          functional C++ manager wrapper
crates/cranelift-ffi/src/bin/impulse_cranelift.rs     audit-manager runner mode
crates/compiler/src/cli/args.rs                      aliases
crates/compiler/src/cli/validate.rs                  compatibility checks
crates/compiler/src/cli/runner.rs                    canonical options
crates/compiler/src/cli/source_mode.rs               backend threading
crates/compiler/src/cli/fixture_mode.rs              fixture threading
crates/compiler/src/json_naming.rs                   backend-aware JSON
crates/compiler/src/signal_lowering.rs               Cranelift effective-FIR analysis
crates/compiler/src/emitters.rs                      public Cranelift option/report path
tests/impulse-tests/Make.mem0                        new integration
tests/impulse-tests/Makefile                         targets/help
tests/impulse-tests/README.md                        usage
tests/impulse-tests/archs/*mem0*                     C/C++ audit drivers
tests/impulse-tests/dsp/*mem0*                       import-free fixtures
tests/impulse-tests/tools/*mem0*                     shared trace/JSON checker glue
porting/cranelift-dsp-ffi-parity-matrix-en.md         manager family status
porting/cranelift-backend-plan-en.md                  implemented contract cross-link
porting/faust-rs-vs-faust-cpp-differences-en.md      implemented differences
porting/journal/YYYY-MM-DD.md                        implementation record
```

Avoid adding memory-manager policy to `fir`: FIR describes the program; the
codegen analysis describes one target storage strategy. Add FIR metadata only
if M2 demonstrates that the owning node lacks information that cannot be
derived without duplication.

## 12. Risks and mitigations

| Risk | Mitigation |
|---|---|
| JSON and generated code disagree | derive both from one immutable `MemoryLayout` |
| wrong target object size | explicit `TargetAbi`, exactness flag, runtime `sizeof` |
| copied C++ lifetime bugs | captured manager, reverse unwind, pointer reset, audit manager |
| ordinary lifecycle regresses | backend lifecycle-conformance tests before impulse gate |
| C ABI becomes accidental C++ | compile a strict C architecture and header client |
| Cranelift inline accesses survive externalization | pointer-slot lowering tests plus poison/audit runtime |
| generated writable tables remain hidden JIT allocations | D7 ownership split and staticInit fixture |
| class pointer state leaks across cached factories | factory-owned class registry and distinct-manager cache tests |
| manager callbacks deadlock/re-enter the cache | never call foreign callbacks under the global cache lock; reentrancy test |
| cache key aliases mem0 and ordinary machine code | include semantic mode/layout in SHA and serialized compile options |
| raw manager pointers are serialized or outlive glue | serialize no runtime binding; copy callbacks into factory-owned glue |
| clone copies external pointers instead of payloads | fresh-zone allocation, slot rewrite, identity/divergent-state tests |
| Rust wrapper is falsely reported/freed by manager | D10 ownership boundary and trace/JSON exclusion tests |
| Cranelift stub gives a false green impulse result | strict subset mode and `compute_body_lowered == true` gate |
| Cranelift offset truncation on large layouts | checked canonical-to-`u32` conversion or typed rejection |
| poison allocator changes samples | require explicit `instanceClear` parity |
| static table allocated per instance | scope/phase classification plus multi-instance test |
| clone aliases source buffers | pointer-identity and divergent-state tests |
| access counts are oversold | version and name the static metric precisely |
| `compute_cost` silently misses FIR constructs | exhaustive matcher, typed error, focused category tests |
| branch costs reproduce the C++ `fThen` bug | versioned D6 semantics and asymmetric-branch differential |
| operation maps drift or become nondeterministic | checked totals plus `BTreeMap` and sum invariants |
| `Int64`/`Bool` buffer is mislabeled or miscast | exhaustive type match plus D5 header/diagnostic tests |
| `mem1`–`mem3` leak into scope | enum has only `None`/`Mem0`; CLI rejection tests |
| compiler cost silently increases | mandatory release `compile-budget-check` |
| vector lowering has different access/storage needs | baseline scalar first; qualify vector separately before claiming support |

Vector C/C++ output is not automatically covered merely because the common
emitter can print it. If the initial implementation accepts `-vec -mem0`, it
needs its own layout/access semantics and impulse qualification. Otherwise
reject that combination with a stable diagnostic and register the temporary
restriction.

The initial Cranelift `mem0` gate is the backend's effective scalar FIR/JIT
path. Any vector option that can reach Cranelift is rejected until pointer-slot
lowering, alignment, access counts, and impulse parity are qualified for that
path; scalar success must not be advertised as vector support.

## 13. Validation commands

Targeted commands are added as the phases land. The final gate includes:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p ffi-common
cargo test -p codegen cranelift
cargo test -p cranelift-ffi
cargo test --workspace --all-targets
cargo run -p xtask -- golden-check
cargo run --release -p xtask -- compile-budget-check
make -C tests/impulse-tests cpp-mem0
make -C tests/impulse-tests c-mem0
make -C tests/impulse-tests cranelift-mem0
make -C tests/impulse-tests all-mem0
```

`compile-budget-check` is mandatory because the work touches `compiler` and
`codegen`. A failure is investigated; the baseline is not raised merely to
accept the port.

When the pinned C++ binary is available, also run the differential generator
for the selected `mem0` corpus. Store repository-relative summaries and fixture
expectations, never absolute checkout paths.

## 14. Completion criteria

The `mem0` port is complete only when all of the following are true:

- exactly the four approved aliases select `MemoryManagerMode::Mem0`;
- `mem1`–`mem3` remain rejected;
- C++, C, and Cranelift externalize every eligible instance array and required
  generated writable static table;
- scalars and non-externalized constants remain embedded;
- runtime descriptions, actual allocations, and JSON form one consistent,
  deterministic plan;
- C++ keeps its compatible public names and fixes lifecycle/clone/ownership
  defects approved in M0;
- the C ABI is documented as adapted and compiles as strict C;
- Cranelift uses pointer-slot lowering, factory-owned class zones, and a shared
  aligned manager ABI; its C++ wrapper set/get methods are functional;
- its first instance creation allocates shared class storage before instance
  storage, and the first successful `init` fills that storage exactly once;
- Cranelift compilation/JSON work unbound, instance creation fails clearly
  until binding, cache identity includes `mem0`, and serialization never stores
  callbacks or pointers;
- Cranelift clone deep-copies instance zones, final factory release follows the
  required instance/class/JIT order, and no foreign callback runs under the
  global cache lock;
- allocation failure is leak-free and destruction uses the original manager;
- JSON is versioned, target-aware, and preserves the legacy common fields;
- JSON emitted for `mem0` contains the versioned, deterministic
  legacy-compatible `compute_cost` report;
- compute-cost totals match their operation maps, common-subset counts match the
  pinned compiler, and D6 corrections are explicitly allowlisted;
- JSON without `mem0` is unchanged;
- C, C++, and strict-lowered Cranelift `mem0` impulse tests pass in
  `tests/impulse-tests` using self-contained fixtures;
- ordinary vs `mem0`, and optimized vs unoptimized, samples agree;
- lifecycle, unit, structural, differential, golden, cost, and workspace gates
  are green;
- every implemented compatibility difference and public API status is recorded.

## 15. Explicit non-goals

- `-mem1`, `-mem2`, `-mem3`;
- `iControl`/`fControl`/`iZone`/`fZone` memory models;
- Java and legacy `ocpp` support;
- changing Wasm linear-memory layout;
- replacing the host memory manager with a Rust allocator API;
- promising dynamic cache/memory-placement optimization from the static access
  counts;
- extending `mem0` beyond C, C++, and Cranelift;
- changing the legacy top-level JSON `size` field without a separate migration.

## 16. References

- Faust manual, [Architecture Files — Custom Memory
  Manager](https://faustdoc.grame.fr/manual/architectures/#custom-memory-manager),
  consulted 2026-08-13;
- pinned Faust C++ source `8eebea429`, especially the source inventory in
  section 3.2;
- `porting/backend-lifecycle-contract-en.md` for the lifecycle invariant;
- `porting/phases/phase-0-validation-en.md` for the pre-implementation gate;
- `porting/wasm-json-parity-plan-2026-03-26-en.md` for the pre-existing JSON
  `memory_layout` and `compute_cost` gaps;
- `porting/cranelift-backend-plan-en.md` for the adapted native JIT design;
- `porting/cranelift-dsp-ffi-parity-matrix-en.md` for the public factory/API
  family whose memory-manager row must move out of deferred status.
