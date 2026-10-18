# foreign-call

Runtime bridge from symbolic Faust foreign-function bindings to raw host
function pointers.

## Scope

This crate provides the low-level dispatch helper used when a compiled DSP needs
to call a foreign C ABI function by address.

## Public API

| Item | Description |
|---|---|
| `ScalarType` | Supported scalar ABI types: `Int32`, `Float32`, `Float64`, `Bool`, `Void` |
| `Value` | Runtime value wrapper for supported scalar arguments and returns |
| `invoke(addr, ret, args)` | Dispatch an `extern "C"` function pointer with a supported signature |
| `is_supported(ret, args)` | Whether `invoke` can call that signature |
| `MAX_ARGS` | The largest argument count `invoke` calls with (4) |
| `TypedFunction`, `NamedFunction` | A C function pointer typed by its signature, under its symbol |
| `libm_functions()`, `libm_function(symbol)` | The system's C math library functions the backends call for `maths.lib` (`cbrt`, `hypot`, `erf`, `tgamma`, `ldexp`, the Bessel functions, ...); none on wasm32 |

## Supported signatures

Rust has no call whose signature is built at run time (what libffi does in
C), so each signature is a function pointer type known at compile time, and
the supported ones are enumerated, by macros: up to `MAX_ARGS` arguments, each
`Int32`, `Float32` or `Float64` in any combination, returning one of these or
`Void`, plus the `Bool` shapes `bool()`, `bool(bool)`, `bool(bool, bool)` and
`void(bool, bool)`.

## Safety boundary

`foreign-call` is one of the crates exempt from the global
`unsafe_code = "forbid"` lint, and the only one outside the FFI and `faust`
crates. It must transmute raw addresses into function pointers, so the caller is
responsible for ensuring that:

- `addr` points to a valid `extern "C"` function,
- `ret` matches the real return type,
- `args` exactly match the real argument types and arity,
- the pointed function remains valid for the duration of the call.

Unsupported signatures return `None` instead of attempting a call.

