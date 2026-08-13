# Faust-rs `-mem0` Custom Memory Manager Guide (User)

This guide is for host/plugin developers who want Faust-generated DSP code to
get its memory from *their* allocator instead of the ordinary
`new`/`delete`/`malloc` — a real-time-safe arena with no heap traffic after
startup, a pool reused across polyphonic voices, SIMD-aligned buffers, or a
fixed budget on an embedded target.

For the full design rationale and the frozen JSON schema rules, see
[`custom-memory-manager-mem0-analysis-and-porting-plan-2026-08-13-en.md`](../porting/custom-memory-manager-mem0-analysis-and-porting-plan-2026-08-13-en.md)
§6.8. For the exact C++ compatibility contract described in §7 below, see
`DIFF-API-005` in
[`faust-rs-vs-faust-cpp-differences-en.md`](../porting/faust-rs-vs-faust-cpp-differences-en.md).

## 1. Quick start

```bash
# C++, with the layout/cost JSON next to the source
faust-rs -lang cpp -mem0 -json foo.dsp -o foo.cpp

# C
faust-rs -lang c -mem0 foo.dsp -o foo.c

# Native Cranelift JIT (the JSON describes the JIT's own layout)
faust-rs -lang cranelift -mem0 -json foo.dsp -o foo.cranelift
```

Only mode zero exists (`-mem`, `-mem0`, `--memory-manager`, and
`--memory-manager0` are equivalent spellings); `mem1`–`mem3` and vector mode
fail closed.

## 2. The three manager surfaces

| Backend | Contract | Header | Notes |
|---|---|---|---|
| C++ | `dsp_memory_manager` (C++ virtual class) | your own copy of `architecture/faust/dsp/dsp.h` | Source-compatible with the upstream Faust interface, plus an additive alignment-aware extension (§7). |
| C | `faust_memory_manager` (plain struct of function pointers) | [`crates/ffi-common/include/faust-memory-manager.h`](../crates/ffi-common/include/faust-memory-manager.h) | Rust-only, versioned (`abi_version`, `struct_size`), always alignment-aware. |
| Cranelift | same `faust_memory_manager` struct | same header | Bound at runtime through `crates/cranelift-ffi` (`setCCraneliftMemoryManager`). |

The C ABI, reproduced from the header:

```c
typedef struct faust_memory_manager {
    uint32_t abi_version;
    size_t struct_size;
    void* context;
    void (*begin)(void* context, size_t count);
    void (*info)(void* context, const char* name, faust_memory_type type,
                 size_t element_count, size_t size_bytes, size_t alignment,
                 uint64_t reads, uint64_t writes);
    void (*end)(void* context);
    void* (*allocate)(void* context, size_t size_bytes, size_t alignment);
    void (*destroy)(void* context, void* address, size_t size_bytes, size_t alignment);
} faust_memory_manager;
```

The C++ interface, exactly as documented upstream plus the additive overloads:

```cpp
struct dsp_memory_manager {
    enum MemType { kInt32, kInt32_ptr, kFloat, kFloat_ptr, kDouble, kDouble_ptr,
                   kQuad, kQuad_ptr, kFixedPoint, kFixedPoint_ptr,
                   kObj, kObj_ptr, kSound, kSound_ptr, kInt64, kInt64_ptr,
                   kBool, kBool_ptr };
    virtual ~dsp_memory_manager() {}
    virtual void begin(size_t count) {}
    virtual void info(const char* name, MemType type, size_t element_count,
                       size_t size_bytes, size_t reads, size_t writes) {}
    virtual void end() {}
    // Legacy overloads from the upstream architecture/faust/dsp/dsp.h.
    virtual void* allocate(size_t size) = 0;
    virtual void destroy(void* ptr) = 0;
    // faust-rs mem0 extension (see §7).
    virtual void* allocate(size_t size, size_t /*alignment*/) { return allocate(size); }
    virtual void destroy(void* ptr, size_t /*size*/, size_t /*alignment*/) { destroy(ptr); }
};
```

A worked, tested implementation of this header lives at
[`tests/impulse-tests/archs/faust_mem0.h`](../tests/impulse-tests/archs/faust_mem0.h)
(`AuditCppMemoryManager`) — read it alongside this guide; every pattern below
is a simplified variant of what it already does.

## 3. Reading `-json -mem0`: the schema

`-json` adds two things absent from ordinary Faust JSON: a `memory_manager`
object, and eight new fields on every `memory_layout` item (the pinned C++
reference only ever emits six: `name`, `type`, `size`, `size_bytes`, `read`,
`write`).

### 3.1 `memory_manager`

```json
"memory_manager": {
  "mode": "mem0",
  "backend": "cpp",
  "manager_abi": "dsp_memory_manager_v1",
  "abi": {
    "target": "native",
    "pointer_size": 8,
    "pointer_alignment": 8,
    "maximum_allocation_alignment": 16
  },
  "access_metric": "static_accesses_per_scalar_frame"
}
```

| Field | Meaning |
|---|---|
| `backend` | `c`, `cpp`, or `cranelift` — which emitter produced this description. |
| `manager_abi` | `dsp_memory_manager_v1` for C++, `faust_memory_manager_v1` for C and Cranelift — which of the two contracts in §2 applies. |
| `abi.pointer_size` / `abi.pointer_alignment` | Pointer size/alignment on the target this layout was computed for. |
| `abi.maximum_allocation_alignment` | A fixed per-target ceiling (16 on native 64-bit), not a per-DSP maximum. Layout construction rejects any zone whose alignment would exceed it, so every zone's `alignment` below is guaranteed to be at most this value — implement up to this bound and you cover any DSP on this target. |
| `access_metric` | Always `static_accesses_per_scalar_frame` today: `read`/`write` below count syntactic accesses in one occurrence of the scalar sample-loop body, not multiplied by loop trip count or vector lanes. |

### 3.2 `memory_layout` items

```json
{
  "name": "fVec2", "type": "kFloat_ptr", "size": 11, "size_bytes": 44,
  "read": 2, "write": 2,
  "scope": "instance", "role": "instance_buffer", "alignment": 4,
  "runtime_allocated": true, "allocation_phase": "instance_create",
  "allocation_order": 2, "size_exact": true, "size_source": "computed"
}
```

| Field | Values | Meaning |
|---|---|---|
| `scope` | `temporary`, `class`, `instance` | `class` zones are shared by every instance (allocated once, on the first `classInit`); `instance` zones are per-DSP-instance. |
| `role` | `subcontainer`, `static_table`, `dsp_object`, `instance_buffer`, `embedded_scalar` | `dsp_object` is the main DSP block itself; `instance_buffer` is an externalized array (a delay line, a `rec` recursion buffer); `static_table` is a generated class-scope table (e.g. a waveform); `subcontainer` is the small helper object that fills a `static_table` during `classInit`; `embedded_scalar` is a plain field with no separate allocation (`fSampleRate`). |
| `alignment` | bytes | What this zone's own allocation must satisfy — the exact number generated code now passes to `allocate`. |
| `runtime_allocated` | bool | `false` for `embedded_scalar` fields that are only described, never separately allocated. |
| `allocation_phase` | `describe_only`, `class_create`, `class_init`, `create_object`, `instance_create` | *When* in the lifecycle the allocation happens; `create_object` is the DSP block itself, `instance_create` is `memoryCreate()`, `class_create`/`class_init` are the class-table allocation inside `classInitChecked`. |
| `allocation_order` | integer | Deterministic order allocations happen in, and the exact reverse order they are released in on rollback or teardown. |
| `size_exact` | bool | Whether `size_bytes` is a checked, exact number. |
| `size_source` | `computed`, `compiler_expression`, `estimated` | Provenance behind `size_exact`: `computed` is derived from the explicit target ABI model; `compiler_expression` means the generated language uses `sizeof`/`alignof` as the real runtime authority and this JSON number is a non-authoritative companion (the usual case for `dsp_object`/`subcontainer`); `estimated` is a best-effort, explicitly inexact number. |

### 3.3 Budgeting offline, with no host code at all

Because the JSON is produced by the compiler alone, you can get a memory
budget for a DSP without linking any manager — useful in CI, on an embedded
target with a fixed RAM budget, or before writing any allocator code:

```bash
faust-rs -lang c -mem0 --json foo.dsp | jq '[.memory_layout[] | select(.runtime_allocated)] |
    {total_bytes: (map(.size_bytes) | add),
     allocations: length,
     max_alignment: (map(.alignment) | max)}'
# {"total_bytes": 76, "allocations": 2, "max_alignment": 8}
```

## 4. The lifecycle contract

Every manager, in either language, is driven the same way:

1. **Describe.** `memoryInfoChecked`/`memoryInfo` (C++) or
   `memoryInfoChecked<Class>`/`memoryInfo<Class>` (C) call `begin(count)`,
   then `info(...)` once per zone in `allocation_order`, then `end()`. This
   happens once and describes the shape of the class tables plus *one*
   instance's worth of buffers — it is the live equivalent of reading the
   `memory_layout` JSON.
2. **Allocate.** `classInit`/`classInitChecked` allocates class-scope zones
   (once, idempotently, shared by every instance); `create`/`createChecked`
   (C++) or `create<Class>` (C) then allocates the DSP object and its
   instance buffers, calling `allocate` in the same `allocation_order` used
   to describe them.
3. **Release.** `destroy`/`classDestroy(Checked)` release everything in
   *reverse* `allocation_order`. A failed allocation mid-`create` rolls back
   everything allocated so far the same way — this is exactly the O(N)
   rollback described in the journal entry for this port.

`memoryInfo(Checked)` and `classInit(Checked)`/`classDestroy(Checked)` come
in two forms: the plain one terminates (`std::terminate`/`abort`) on a
contract violation, and the `*Checked` variant returns `false`/`0` so a host
that wants to recover — rather than crash — can; prefer `*Checked` unless a
hard failure really should be fatal. `create`/`createChecked` (C++) and
`create<Class>` (C) never terminate either way — both are already fallible,
returning `nullptr`/`NULL` on failure; `create()` differs from
`createChecked(manager)` only by using the already-bound `fManager` instead
of an explicit argument. `destroy`/`destroy<Class>` has no `*Checked` variant
and no failure mode to report — releasing memory does not fail.

The manager and its `context` must stay alive until the matching
`classDestroy` call; generated code never rebinds to a different manager
mid-lifecycle.

## 5. Use case: a real-time-safe arena (no heap traffic after startup)

The two-phase contract above — describe once, then allocate in the same
order — makes it straightforward to size one fixed block up front and hand
out slices from it, with no `malloc`/`free` once the audio thread starts.
This is the pattern for a single long-lived DSP instance (a plugin, a
standalone synth voice that is created once).

Note that the C++ `info()` callback — unlike the C ABI's — does not carry a
per-zone alignment (it never has, upstream); pad to a conservative bound, or
read the exact `alignment` field from the `-json` output ahead of time.

```cpp
class ArenaMemoryManager final : public dsp_memory_manager {
    std::vector<size_t> fSizes;
    std::unique_ptr<unsigned char[]> fArena;
    size_t fCapacity = 0, fOffset = 0;

    void begin(size_t count) override {
        fSizes.clear();
        fSizes.reserve(count);
    }
    void info(const char*, MemType, size_t, size_t sizeBytes, size_t, size_t) override {
        fSizes.push_back(sizeBytes);
    }
    void end() override {
        // Pad every zone to a conservative worst-case bound
        // (abi.maximum_allocation_alignment from the JSON, §3.1) since this
        // callback carries no per-zone alignment. For byte-exact sizing, read
        // the real per-zone `alignment` from -json instead.
        fCapacity = 0;
        for (size_t size : fSizes) fCapacity += (size + 15) & ~size_t(15);
        fArena = std::make_unique<unsigned char[]>(fCapacity);
        fOffset = 0;
    }
    void* allocate(size_t size) override {
        size_t padded = (size + 15) & ~size_t(15);
        if (fOffset + padded > fCapacity) return nullptr;  // never happens if end() sized correctly
        void* ptr = fArena.get() + fOffset;
        fOffset += padded;
        return ptr;
    }
    void destroy(void*) override { /* released all at once when the arena is freed */ }
};
```

Bind it once, create the single instance, and never call `allocate` again for
the lifetime of the plugin:

```cpp
ArenaMemoryManager arena;
mydsp::fManager = &arena;
if (!mydsp::memoryInfoChecked(&arena)) { /* handle */ }
mydsp* dsp = mydsp::createChecked(&arena);
```

For true byte-exact sizing (respecting each zone's real `alignment` instead
of a flat worst-case bound), read the `-json -mem0` output once at build time
and hard-code the computed capacity instead of relying on `begin`/`info`/`end`
at all — the arena's `end()` above then becomes a single `assert` that the
live description still matches what the JSON predicted.

## 6. Use case: a size-class pool for polyphony

A polyphonic instrument creates and destroys many DSP instances of the *same*
patch as voices start and stop. Re-`malloc`ing every voice defeats the point
of a custom manager; instead, key a free-list by `(size_bytes, alignment)` —
the C ABI hands both to `destroy` for exactly this reason — and reuse freed
voice buffers instead of returning them to the system allocator:

```c
typedef struct pool_entry { void* address; size_t size; size_t alignment; struct pool_entry* next; } pool_entry;

typedef struct voice_pool { pool_entry* free_list; } voice_pool;

static void* pool_allocate(void* context, size_t size_bytes, size_t alignment) {
    voice_pool* pool = (voice_pool*)context;
    pool_entry** slot = &pool->free_list;
    while (*slot) {
        if ((*slot)->size == size_bytes && (*slot)->alignment == alignment) {
            pool_entry* entry = *slot;
            *slot = entry->next;
            void* address = entry->address;
            free(entry);
            return address;
        }
        slot = &(*slot)->next;
    }
    void* address = aligned_alloc(alignment, (size_bytes + alignment - 1) / alignment * alignment);
    return address;  /* fresh voice: no matching freed block yet */
}

static void pool_destroy(void* context, void* address, size_t size_bytes, size_t alignment) {
    voice_pool* pool = (voice_pool*)context;
    pool_entry* entry = (pool_entry*)malloc(sizeof(pool_entry));
    entry->address = address;
    entry->size = size_bytes;
    entry->alignment = alignment;
    entry->next = pool->free_list;
    pool->free_list = entry;  /* kept warm for the next voice, not freed */
}
```

The first N voices each pay one real allocation per zone; every voice after
that reuses a same-shaped block from the free list instead of touching the
system allocator — the steady-state audio path never calls `malloc`/`free`.
(A production version would also cap the pool size and actually free cold
entries on an idle timer, outside the audio thread.)

## 7. Use case: SIMD-aligned buffers via the alignment-aware C++ overloads

The legacy `dsp_memory_manager::allocate(size_t)` cannot ask for more than
`operator new`'s default alignment. The additive overload from §2 —
`allocate(size_t, size_t)` — receives the real per-zone alignment and can
honor it exactly, which matters if you want, say, every `instance_buffer`
32-byte aligned for AVX:

```cpp
class AlignedMemoryManager final : public dsp_memory_manager {
    void* allocate(size_t) override { std::abort(); }  // legacy path never reached, see below
    void destroy(void*) override { std::abort(); }

    void* allocate(size_t size, size_t alignment) override {
        return ::operator new(size, std::align_val_t(alignment), std::nothrow);
    }
    void destroy(void* ptr, size_t, size_t alignment) override {
        ::operator delete(ptr, std::align_val_t(alignment));
    }
};
```

Overriding the additive pair directly — rather than relying on the base
class's default (which just forwards to the legacy pair) — is what makes
generated code reach this implementation: every allocation/destruction call
site compiles against whichever overload set your `dsp_memory_manager` header
declares, decided once at compile time (`faust_mem0_detail` in the generated
file). Nothing here requires touching generated code or your build flags —
a header that has never heard of the extension keeps compiling exactly as
before, unchanged. Full mechanism and the two compatibility proofs (fallback
against an unmodified upstream header, and confirmation the richer path is
actually reached when available) are `DIFF-API-005` and the
`mem0_generated_cpp_compiles_*` tests in
[`crates/codegen/src/backends/cpp/mod.rs`](../crates/codegen/src/backends/cpp/mod.rs).

## 8. Gotchas

- **Alignment is a request, not a guarantee from Faust's side.** Generated
  code always re-checks the returned pointer's actual alignment and fails
  the allocation gracefully (`createChecked` returns `nullptr`/`NULL`) if a
  manager returns a misaligned address — it never trusts the manager blindly.
  A manager that cannot honor an alignment should simply return a pointer
  that doesn't satisfy it (or fail) rather than silently truncating it.
- **`destroy`'s size/alignment always match a prior `allocate` call exactly**
  — they are not hints, they are the same numbers `allocate` received. A pool
  keyed by `(size, alignment)` (§6) can therefore match with `==`, not a
  range check.
- **Class-scope zones are shared and allocated once**; only instance-scope
  zones repeat per `create()`. Do not size an arena for `class` zones times
  the expected voice count.
- **`releaseInstance<Class>`/`classDestroyTables<Class>` (C) and
  `faust_mem0_detail` (C++)** are internal generated helpers, not a public
  contract — their names and signatures are not stable across faust-rs
  versions. Only the entry points named in §4 are the public surface.
