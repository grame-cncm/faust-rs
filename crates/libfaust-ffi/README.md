# libfaust-ffi

Backend-agnostic libfaust C API: `generateSHA1`, `expandDSP*`,
`generateAuxFiles*`.

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

## Verification

```bash
cargo run -p xtask -- libfaust-export-check
```

Checks that every header-declared symbol is exported, diffs the export set
against `porting/generated/libfaust-rs-exported-symbols.txt`, syntax-checks C
and C++ clients, and links and runs a C++ client that calls
`expandDSPFromString` and `generateSHA1` against the real library.
