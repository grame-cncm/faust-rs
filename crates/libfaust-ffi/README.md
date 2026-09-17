# libfaust-ffi

Backend-agnostic libfaust C API: `generateSHA1`, `expandDSP*`,
`generateAuxFiles*`, and one addition of this port, `getCCompleteDSPError`.

These entry points belong to no backend — expansion and auxiliary-file
generation run the shared front end, and the SHA key is computed from text — so
they live here rather than being duplicated per backend under names like
`expandCInterpreterDSPFromString`.

## Headers

| Header | Surface |
|---|---|
| [`include/libfaust-c.h`](include/libfaust-c.h) | the exported C ABI |
| [`include/libfaust.h`](include/libfaust.h) | `std::string` C++ wrappers, header-only |

The C++ header cannot declare the reference API's functions directly: their
symbols are mangled and `std::string` has no stable ABI. It is therefore inline
wrappers over the C ABI, the same shape `libfaust-box.h` uses for the Box API,
each wrapper releasing its returned `const char*` through `freeCMemory`.

`freeCMemory` itself is exported once for the whole distribution by
`interp-ffi`; defining it here would collide at link time.

## Compile errors

`error_msg` is 4096 bytes by the reference contract (caller-allocated, its size
never passed), so it cannot grow without overflowing existing hosts, and for a
compiler error it receives a one-line summary: "parse failed for x.dsp:
errors=1, recoveries=0, diagnostics=1". `getCCompleteDSPError()` returns the
complete text, that message followed by the rendered diagnostics (location,
source snippet, notes, fixes): per thread, owned by the library (do not free
it), null before the thread's first error, valid until its next one, not reset
by a success. The reference libfaust has no equivalent; the backends have
theirs (`getCCompleteCraneliftDSPFactoryError`,
`getCCompleteInterpreterDSPFactoryError`), and so have the Box and Signal APIs
(`getCCompleteBoxError`, `getCCompleteSignalError`), each for its own entry points. The
`std::string` wrappers of `libfaust.h` read it, so their `error_msg` holds the
complete text.

`getCDSPErrorDiagnostics()`, another addition, returns the typed form of the
same failure: the compiler's complete diagnostics-v2 JSON report (codes, byte
ranges, facts, fixes with their edits and applicability), under the same
contract and **null when the last error carried no typed diagnostics** (a
missing file), even if an earlier one did. `schema_version` is 2,
`request.backend` is `"libfaust"`; `getDSPErrorDiagnostics()` in `libfaust.h`
returns it as a `std::string`. The backends and the Box API have theirs
(`getCCraneliftDSPFactoryErrorDiagnostics`,
`getCInterpreterDSPFactoryErrorDiagnostics`, `getCBoxErrorDiagnostics`).

## Verification

```bash
cargo run -p xtask -- libfaust-export-check
```

Checks that every header-declared symbol is exported, diffs the export set
against `porting/generated/libfaust-rs-exported-symbols.txt`, syntax-checks C
and C++ clients, and links and runs a C++ client that calls
`expandDSPFromString` and `generateSHA1` against the real library, and requires
the complete diagnostic of a program that does not compile.
