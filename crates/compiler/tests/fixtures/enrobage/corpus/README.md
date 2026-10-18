# Enrobage fixtures

This fixture set captures the C++ architecture-wrapping behaviors of
`compiler/parser/enrobage.cpp` and of the wrapping sequence in
`compiler/libcode.cpp`, as ported to `compiler::enrobage`:

- architecture header stripping via `streamCopyLicense`,
- stream-copy sentinels (`<<includeIntrinsic>>`, `<<includeclass>>`),
- architecture include injection (`#include <faust/...>` and quoted variant),
- class-name replacement (`mydsp` forced, `dsp` word-boundary),
- Julia wrapping (`wrapper_wrapped.expected.jl`).

Architecture inputs live in `../arch/`. The Cmajor `<<includeclass>>`
insertion has no expected file here: `tests/enrobage_integration.rs` checks
it inline.
