# signal-ffi

Faust-style C/C++ export for signal construction, matching, normalization, and
source generation, backed by Rust `signals::SigBuilder` and `signals::match_sig`.

## Scope

- Exposes a `Csig*` constructor family (`libfaust-signal-c.h` style):
  constants, inputs, delays, recursion (`CsigRecursion`, `CsigSelf`, and their
  `N` multi-output variants), tables, UI widgets, soundfiles, and foreign
  primitives (`CsigFFun`, `CsigFConst`, `CsigFVar`).
- Exposes a matcher family (`CisSig*`, `CisProj`, `CisRec`) that decodes signal
  trees back into their structural form for C callers.
- Exposes normalization and source helpers
  (`CsimplifyToNormalForm`, `CsimplifyToNormalForm2`,
  `CcreateSourceFromSignals`).
- Exposes a thin C++ convenience wrapper (`libfaust-signal.h`).
- Uses `tree-ffi` for shared tree handles, context-owned C strings,
  null-terminated handle arrays, and common Signal enum definitions
  (`SType`, `SOperator`).

## Context model

Signal handles share the same process-global context
(`createLibContext` / `destroyLibContext`) as `box-ffi`, exported from
`tree-ffi`. This lets `Box` and `Signal` handles be printed and freed through
the common libfaust helpers (`CprintSignal`, `freeCMemory`) without a separate
context lifecycle.

## Notes

- `Signal` handles are opaque and stable within one active context.
- Constructors return `null` when given invalid handles; matcher
  out-parameters are written only on a successful match.
- This is an incremental parity layer; rows marked as exact candidates in the
  generated Signal API matrix still need focused semantic parity tests.

## Errors

`CcreateSourceFromSignals` reports through a 4096-byte `error_msg`.
`getCCompleteSignalError()`, an addition of this port, returns the complete text
of the last error this thread reported through it, not cut at 4096 bytes: per
thread, owned by the library (do not free it), null before the thread's first
error, valid until its next one, not reset by a success. Signals are built
through the API and have no source text, so no failure here comes with compiler
diagnostics: the complete text is the message. It exists so that a host reads
every libfaust-rs error the same way (`getCCompleteDSPError`,
`getCCompleteBoxError`, `getCCompleteCraneliftDSPFactoryError`,
`getCCompleteInterpreterDSPFactoryError`).

## Source provenance

The exported symbol names and signatures mirror the C++ reference header
`architecture/faust/dsp/libfaust-signal-c.h` (Faust commit `8eebea429`).

## Relationship to other crates

- `signals` — owns `SigBuilder`/`match_sig`; this crate only adapts them to the
  C ABI.
- `tree-ffi` — shared handle model, global context, and enum definitions.
- `box-ffi` — sibling C ABI crate sharing the same context.
- `faust-ffi` — re-exports these symbols (`signal_api`) in the unified
  `libfaust` distribution artifact.
