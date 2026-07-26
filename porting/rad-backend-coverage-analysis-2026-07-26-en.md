# RAD backend coverage: what every backend does with reverse-mode AD

**Date**: 2026-07-26
**Status**: analysis complete, two remediations identified
**Question asked**: does the WASM backend fully implement `rad` at the
signal-to-FIR-to-backend level, and what about the others?

## Summary

Two independent gaps, of very different character.

1. **WASM and WAST reject `SimpleForLoop { is_reverse: true }`**, which is the
   only shape RAD ever emits for its reverse-time loop. 5 of the 28 `rad`
   corpus programs fail to compile. The failure is a typed, loud
   `FRS-CGEN-WASM-0003`, so nothing wrong is emitted — but the backend also
   *appears* to support reverse loops, because it handles the general `ForLoop`
   reverse form completely. RAD never produces that form.

2. **`rad` is validated numerically on the interpreter only.** Seven other
   backends emit RAD code that no test checks for correctness. This is the
   quieter of the two, and the larger risk: the WASM gap announces itself, this
   one does not.

Codebox's 12 failures are *not* a gap: they are a semantic incompatibility,
correctly diagnosed at the transform level. See §4.

## 1. Method

Every corpus program matching `^rad_` or `^fad_rad` (28 files, excluding the
four `err_*` negative fixtures) was compiled to all 11 backends:

```sh
faust-rs --timeout 90 -lang <backend> tests/corpus/<program>.dsp -o <out>
```

Reverse-loop shapes were then read out of the FIR dump (`-lang fir`) to
determine *which* FIR node each backend was being asked to emit, rather than
inferring it from the error text.

## 2. Result matrix

| Backend | Emits all 28 | Failures |
| --- | --- | --- |
| cpp, c, rust, julia, asc, interp, cranelift, fir | yes | — |
| wasm, wast | no | 5 × `FRS-CGEN-WASM-0003` |
| codebox | no | 12 × `FRS-SFIR-0010` (see §4) |

The 5 WASM/WAST failures are `rad_lti_recursive_one_pole`,
`rad_lti_recursive_multi_output`, `rad_lti_recursive_multi_output1`,
`rad_lti_recursive_state_space` and `rad_delay1_block_fallback` — the recursive
LTI family plus the block-fallback delay case.

## 3. The WASM gap, precisely

`crates/codegen/src/backends/wasm/mod.rs` refuses the node in two places, both
by pattern:

- **`collect_compute_locals`, line 998** — the `SimpleForLoop` arm matches
  `is_reverse: false` only, so a reverse one falls through to the catch-all
  `other =>` error arm at line 1048.
- **`lower_statement`, line 1120** — the same arm, the same restriction.

### Why it looks supported when it is not

Three facts sit next to each other in that file:

- the collector's **`ForLoop`** arm matches `is_reverse: false | true` — both
  directions accepted;
- `lower_for_loop` (line ~1244) genuinely implements the reversal, selecting
  `I32LeS` instead of `I32GeS` for the exit test;
- `lower_simple_for` (line 1179) does not take `is_reverse` at all. It hardcodes
  `i = 0`, exit on `i >= upper`, step `+1`.

So the backend does support reverse loops — for the general `ForLoop` shape,
which RAD never emits.

Measured over the whole corpus, not just the RAD subset:

- **6 programs** contain a `SimpleForLoop` with `is_reverse: true` (the 5 above
  plus the `err_rad_delay_temporal_unsupported` fixture);
- **1 program** contains a general `ForLoop` with `is_reverse: true`:
  `rep_68_variable_delay_audio_rate.dsp`, a variable delay unrelated to RAD.
  It compiles to WASM without error, so `lower_for_loop`'s reverse branch is
  reachable and is not dead code — just unreachable *from RAD*.

### Shape of the fix

`lower_simple_for` must take `is_reverse` and, when set, emit the descending
form the C++ backend already produces for the same FIR node:

```cpp
for (int i0 = (count) - 1; i0 >= 0; i0 = i0 - 1) { … }
```

so: initialise to `upper - 1`, exit when `i < 0`, step `-1`. Both pattern
matches (lines 998 and 1120) then accept `is_reverse: false | true`, matching
what the `ForLoop` arms already do.

## 4. Codebox is not a gap

The 12 codebox failures are `FRS-SFIR-0010`, raised in **transform**, not in a
backend:

```
'-os' is not supported for programs containing block reverse-mode AD
(BlockReverseAD/ReverseTimeRec): their semantics require the block boundary
```

Reverse-mode AD over a block accumulates cotangents by walking the block
backwards. A one-sample API has no block to walk. This is a genuine semantic
incompatibility and the diagnostic is the right outcome.

It surfaces now because the codebox C5 wiring (`449915e8`) makes the backend
force `ProcessingApi::OneSample`, which is what its `Intrinsic` capability
means. Before that, no path reached the check with codebox selected.

The same reasoning already exists as a `debug_assert!` in
`crates/transform/src/signal_fir/module/build.rs`:

```rust
debug_assert!(
    !(one_sample && has_reverse_outputs),
    "D2 rejects -os for block-sensitive reverse-AD programs before lowering"
);
```

No action.

## 5. The second gap: numeric validation is interpreter-only

Emitting is not being right. Where RAD correctness is actually checked:

| Test | Backend exercised | What it checks |
| --- | --- | --- |
| `crates/compiler/tests/rad_runtime.rs` | `interp` | RAD vs FAD lane parity, and RAD vs central finite differences |
| `crates/compiler/tests/block_reverse_ad.rs` | `interp` | block reverse-AD runtime behaviour |

`rad_runtime.rs` also calls `compile_file_default_to_cpp_with_lane` twice, but
those are stress cases asserting only that compilation completes — no numbers
are compared. `cpp_signal_differential.rs` contains no `rad` case at all.

So cpp, c, rust, julia, asc, cranelift and (once §3 lands) wasm all emit RAD
code whose gradients nothing verifies. The interpreter is transitively
validated against C++ Faust by the impulse suite, which makes it a sound
oracle — but only for itself.

### Remediation

Extend numeric validation to every backend that emits RAD, comparing gradient
lanes against the interpreter for the same program and seed list. The
interpreter is the reference because it is the side already checked two ways
(against FAD, and against finite differences).

Backends and how each can be run in-process:

- **cranelift** — JIT, runnable directly;
- **cpp, c, rust** — require an external toolchain, so they belong in the
  differential harness rather than in `cargo test`;
- **asc, wasm/wast** — a WASM runtime can execute the module in-process;
- **julia** — external toolchain.

## 6. Outcome

Both remediations landed the same day.

### §3 — the WASM fix

`lower_simple_for` now takes `is_reverse` and emits the descending form; both
pattern matches accept either direction. All 5 previously rejected programs
compile, and their gradients match the interpreter sample for sample.

### §5 — the RAD lane

Built as a second lane of the existing impulse-tests harness rather than a new
harness. `common.mk` grew `dspdir`/`refdir`, so every backend Makefile serves
both lanes unchanged; the RAD lane is `dsp-rad/` (symlinks into
`tests/corpus/`) plus `reference-rad/` from `Make.ref-rad`.

Result — 28 programs × 7 backends, all matching the interpreter:

| Backend | Result |
| --- | --- |
| cpp, c, rust, julia, wasm, asc, cranelift | 28/28 |

`rad-interp` deliberately has no target: it would compare the reference against
itself.

### What the lane found on its first run

Julia failed one program, `rad_tbptt_softclip_drive`, with "non-finite DSP
output" at frame 0. The cause is not RAD-specific and not confined to this
program:

**`min`/`max` were mapped to Julia's own, which propagate NaN. Faust's `min`
and `max` are C's `fmin`/`fmax`, which absorb a NaN operand and return the
other.**

```
Julia:  min(10.0, NaN) = NaN         C:  fmin(10.0, NaN) = 10.0
```

That program divides by `abs(fTemp3)`, legitimately `0` on the first frame. The
resulting NaN is absorbed by the next `fmax(0.01, fmin(10.0, …))` on every
C-family backend; on Julia it poisoned the recursion permanently. Fixed with
`faust_fmin`/`faust_fmax` helpers in the preamble. Integer `min_i`/`max_i` stay
mapped to Julia's own, since no NaN is representable there.

The ordinary 133-program Julia suite is unchanged (132 pass, `subcontainer1`
being a declared `KNOWN_FAIL_all`), so the fix is additive.

### Rejecting mutation

Making the reverse `SimpleForLoop` start at `0` instead of `upper - 1` — a
loop that still compiles and still runs, just in the wrong direction — takes
`rad-wasm` from 28/28 to 5/28, with the differences on the gradient lane. The
lane detects a wrong reverse loop, not merely a missing one.
