# tree-ffi

Shared support layer for Faust-style C ABI crates.

## Scope

- Owns the shared `TreeArena` handle model used by Box and Signal FFI APIs,
  held in one process-global context (`global_context`,
  `with_global_context`, `reset_global_context`); its C entry points
  `createLibContext` / `destroyLibContext` are exported by `box-ffi`.
- Holds the registry of Signal FFI UI/control descriptors
  (`FfiSignalControl`, `register_signal_control`).
- Provides stable opaque handle encoding/decoding for C callers.
- Allocates null-terminated handle arrays, registered with `ffi-common` so any
  adapter's `freeCMemory` releases them, also after a context reset.
- Owns context-pinned C strings used by matcher out-parameters.
- Provides shared `SType` and `SOperator` enum definitions.
- Provides null-safe helper functions for C out-pointers.

## Non-goals

- Does not export public libfaust C symbols.
- Does not construct Box or Signal nodes directly.
- Does not own backend lowering or source generation.

API crates such as `box-ffi` and `signal-ffi` remain responsible for their
public `Cbox*`, `Csig*`, matcher, conversion, and source-generation symbols.
