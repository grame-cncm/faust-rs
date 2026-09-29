# Session Handoff

Date: 2026-09-30

## Repo State

- Branch: `main-dev`
- Parent HEAD: `c0a86088`
- This commit closes the safe Rust facade's precision mismatch from
  [issue #19](https://github.com/grame-cncm/faust-rs/issues/19).

## Working Tree

- Tracked changes after this commit: none.
- Pre-existing untracked files `build_all`, `dummy_fdn`, and
  `push_main.sh` remain untouched.

## Decision and Changes

- `CompileOptions::args` is the sole precision input; the default is
  single precision and the last `-single`/`-double` flag wins.
- The interpreter factory variant and the compiled Cranelift JIT module
  report the actual sample width. `Factory::precision`,
  `Dsp::compute`, and parameter zone access use that value.
- Migrated the Rust tests and embedding examples; recorded the public API
  break in the difference registry and regenerated code graphs.

## Validation

- `cargo fmt --all`, workspace Clippy, all five structure checks and
  `golden-check`: passed.
- `cargo test --workspace --all-targets`: passed with loopback binding
  allowed for the repository's local HTTP fixture.
- Canonical release `compile-budget-check`: attempted twice, stopped
  before case measurements because this machine's 3 ms calibration is below
  the checked-in 4 ms floor.
- A diagnostic run using a temporary baseline copy with only that floor
  set to 3 ms passed all eight front-end and five codegen cases. The
  repository baseline was not changed.

## Next Step

- Let CI run the canonical compilation-cost gate on its runners before
  treating the commit as release-ready. No code follow-up is pending.
