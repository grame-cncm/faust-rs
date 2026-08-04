# Enrobage fixtures

This fixture set captures C++ `enrobage.cpp` behaviors used by the Rust
porting steps:

- architecture header stripping via `streamCopyLicense`,
- stream-copy sentinels (`<<includeIntrinsic>>`, `<<includeclass>>`),
- architecture include injection (`#include <faust/...>` and quoted variant),
- class-name replacement (`mydsp` forced, `dsp` word-boundary).
- language-neutral `<<includeclass>>` insertion for Cmajor source.
