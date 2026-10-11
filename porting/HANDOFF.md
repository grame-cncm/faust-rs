# Session Handoff

Date: 2026-10-11

## Repo State

- Branch: `main-dev`
- Commit: the commit containing this handoff, parent
  `e7c320f093a2240789a858b9aab3ce500852f858`.
- The user requested committing the correction with the validation limits
  reported below. No push is part of this session.

## Working Tree

- The changes listed below are included in this commit; no other tracked or
  untracked changes are left after committing.
- Tracked implementation changes: `crates/sigtype/src/rules.rs`,
  `crates/normalize/src/{normalform,simplify}.rs`, `crates/fir/src/types.rs`,
  the shared/scalar/vector power emission and prototype assembly under
  `crates/transform/src/signal_fir/`, its table-generator interpreter and tests,
  `crates/codegen/src/backends/{cpp/mod,rust/mod,cranelift/math}.rs`.
- Documentation: compatibility registry DIFF-BEH-020,
  `porting/journal/2026-10-11.md`, the generated public API index (positions
  only; no public-item inventory change), and this handoff.
- New tests: `crates/compiler/tests/integer_pow.rs` and
  `crates/compiler/tests/integer_pow_rust.rs`.
- Ignored local scripts `build_all`, `dummy_fdn`, `push_main.sh` are untouched.

## What Changed / Decisions

Port C++ `eb21fcccff1ead7fe286f35614a464bea70a5fe2` (#1340): runtime integer
pow uses double pow/rint before int32 conversion in scalar and checked vector
FIR. Typing/promotion had unconditionally forced pow to Real; that earlier
parity gap is closed too. Positive integer constant folding and small constant
powers retain wrapping int32 arithmetic. Compile-time table generators follow
the same integer semantics. CSE keeps a foreign base evaluated only once.

Single pow/rint calls have distinct FIR names (`powf`/`rintf`) from double
calls, without changing the public math-op inventory or the host DSP APIs.
Cranelift and C++ keep explicit suffixed C symbols at their fixed host precision.
Rust single-precision output now supplies the double rint bridge.

FBC is still uniformly f32 or f64 per factory, as in C++: no mixed-precision
opcode/ABI extension. Double FBC passes at optimizer levels 0/max. Single FBC
still loses a unit for 25^6; the current C++ build has no interpreter backend,
so its matching precision limitation was established from the source.

## Validation Run

- Native generated C++: single/double, scalar/vector, `-O2` and
  `-O3 -ffast-math`: exact zero residuals for 250^1, 25^6, (-3)^3; small-power
  wrap and negative runtime exponents covered too.
- Generated Rust: built/run with the repository's FaustDsp host surface,
  single/double, `rustc -C opt-level=0/3`: exact residuals.
- Type/promotion/constant-fold/table-generator tests: pass.
- Formatting, clippy, all five static gates, and golden-check: pass.
- Manual Cranelift/faustprobe at both precisions: exact zero residuals.
- Full workspace: two reference tests fail before this port as well; the
  rerun skipping only those tests passes (3,201 tests). All codegen unit tests
  and both native regression suites pass after the final C++ safeguard too.
- Isolated release cost gate: cannot calibrate (3 ms, required floor 4 ms).
  Supplementary alternating release CLI before/after medians are stable:
  karplus 8.625/8.628 ms, spectral_level 325.153/321.422 ms. These include
  process startup and are not a substitute for the gate.

## Open Issues / Blockers

- The unmodified expansion oracle `020_library_import` does not match the
  local current Faust libraries (the oscillator's phasor implementation has
  changed). The pre-change release binary `19efd550` reproduces exactly the
  same mismatch.
- `cranelift-ffi::output_snapshots` reports 53 stale recordings. All 53
  match the pre-change release faustprobe too (46 exactly; seven after
  removing timing fields). No snapshot refresh is part of this correction.
- The sandbox prevented the fixture HTTP server in `remote_source_baseline`;
  it passed in the authorized rerun with permission to bind that local server.
- A release cost measurement during concurrent test/build load exceeded
  spectral_level's front-end ceiling (255 vs 233.1 units). Three isolated runs
  stop at the calibration floor instead. The versioned baseline is untouched;
  a successful gate on a suitable runner and green CI remain unconfirmed.

## Next Steps

1. Resolve the independent reference drift and rerun the complete workspace
   suite; obtain a successful release cost gate on a suitable runner and CI.
2. Review CI results before publication. No baseline or golden refresh is
   part of this correction.
3. The expansion-oracle and faustprobe recording drift need separate
   resolution; single FBC precision extension is outside this C++ commit's
   port scope.
