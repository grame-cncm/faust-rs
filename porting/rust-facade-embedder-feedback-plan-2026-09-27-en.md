# `faust` crate: first embedder feedback. Plan

Date: 2026-09-27

## Scope

The `faust` crate (`crates/faust`, branch `rust-facade`, commits `ae8cd278`
and `0f240664`) is the supported Rust API over the interpreter and the
Cranelift JIT, written in answer to issue
[grame-cncm/faust-rs#17](https://github.com/grame-cncm/faust-rs/issues/17).
Its first embedder, `py-faust-rs`, has been ported onto it by its author
([shakfu/faust-rs@py-facade](https://github.com/shakfu/faust-rs/tree/py-facade),
commit `99e9419a` on top of `0f240664`), with every Python test run on both
backends: 150 pass, 3 strict xfails. The report (issue comment of
2026-09-27) confirms what the crate was for: state persists across blocks on
both backends, a `Dsp` outlives its `Factory` handles, the `sha_key` fix
keeps two programs apart, and `import("stdfaust.lib")` resolves from a
source string.

It makes five requests. This document checks each against the code of
`0f240664`, adds two defects found while doing so, and plans the fixes.
Nothing here changes the C ABI of `libfaust-rs`.

## 1. Requests, checked against the code

### 1.1 `f64` I/O for the interpreter in `-double` (regression)

`py-faust-rs` was `f64` end to end before; through the facade, a `-double`
interpreter program rounds its input and output to `f32`:
`process = _;` returns `1.0` for `1.0 + 2^-40`, `process = 16777217.0;`
outputs `16777216`. Cranelift is exact.

Cause, two narrowings in a row:

- `Dsp::exchanged()` (`crates/faust/src/dsp.rs`) answers `F32` for the
  interpreter whatever the precision, so `compute_f64` narrows the host's
  buffers to `f32`;
- `computeCInterpreterDSPInstance` (`crates/interp-ffi/src/instance.rs`) takes
  `float**`, as the C++ `interpreter_dsp` does with the default
  `FAUSTFLOAT`, and `FbcDspFactoryAny::execute_block_io_f32`
  (`crates/interp-ffi/src/types.rs`) widens the inputs to `f64` and narrows
  the outputs back.

The C entry point is right as it is (C++ parity). What is missing is a way for
a Rust caller to hand `f64` buffers to an `f64` executor.

### 1.2 `Dsp: Sync`

PyO3 0.29 requires `Send + Sync` on a `#[pyclass]`; `Dsp` is `Send` only
(its `RawInstance` holds raw pointers), so `py-faust-rs` wraps it in a
`Mutex`. The `&self` methods of `Dsp` are: `factory`, `backend`,
`precision`, `num_inputs`, `num_outputs` (fields and the `Arc`),
`controls`, `control` (the control map), `get` (reads one zone of the
instance), `sample_rate` and `metadata` (C calls). Every method that writes
the instance (`compute_*`, `set`, `init`, `instance_init`, `reset_controls`,
`clear`) already takes `&mut self`. On the interpreter side,
`metadataCInterpreterDSPInstance` only walks the factory's `meta_block`.
To be checked: the Cranelift `getSampleRate` and `metadata` entries, and the
interpreter's `getSampleRate`, must read and nothing else.

### 1.3 A `label` on `Control`

`ControlMap::path_for` (`crates/faust/src/controls.rs`) replaces `/`, space,
`#`, `*`, `,`, `?`, brackets, braces and parentheses by `_`, as the C++
`PathBuilder::buildPath` does, so `"my gain"` cannot be recovered from
`/…/my_gain`. The label is at hand in `ControlMap::add` and dropped.

### 1.4 Controls in declaration order

`ControlMap::entries` is a `BTreeMap<String, Entry>`, so `Dsp::controls()`
is in path order. Declaration order is the order of the UI tree, of the C++
`MapUI` walk and of the previous binding.

### 1.5 A program Cranelift refuses

`Dsp::create` refuses a Cranelift instance whose metadata says
`cranelift-compute-body-lowered = false`, that is whose `compute` fell
outside the lowering subset and was compiled to an empty stub
(`crates/codegen/src/backends/cranelift/jit_data.rs`). No test builds such a
program from Faust source, and the embedder knows none. The codegen tests
reach the stub through a FIR fixture calling a foreign function with no
bound symbol (`compile_module_falls_back_when_custom_foreign_fun_symbol_is_missing`);
`subset.rs` also refuses any math call outside `FirMathOp` and its list of
extra names. Candidate from source, to be confirmed:
`process = ffunction(float frs_unknown_fn(float), "", "");`.

## 2. Defects found while checking

### 2.1 Control ranges are rounded to `f32` in `-double`

The `UIGlue` callbacks take `FfiFaustFloat`, which is `f32`
(`crates/ffi-common/src/abi.rs`), so for a `-double` program
`Control::init`, `min`, `max` and `step` arrive narrowed: an
`hslider("x", 0.1, 0, 1, 0.01)` reports `init = 0.10000000149…`. The zones
themselves are read and written at the compiled width (`ControlMap::read`
and `write` dispatch on `Precision`), and `reset_controls` writes the exact
`f64` initial value from the bytecode, so `get` right after
`reset_controls` disagrees with `control(path).init`.

### 2.2 An allocation per block in the interpreter's `-double` compute

`execute_block_io_f32` builds its `f64` input and output vectors with
`collect` and `vec!` on every call, in the audio thread. The C entry point
has this cost today, independently of the facade.

Measured while fixing it (P1): a compute call also allocates the executor's
evaluation stacks, `Vec::with_capacity` of the real, int and address stacks in
`FbcExecutor::execute_block*` (`crates/codegen/src/backends/interp/executor.rs`),
once per block executed, so twice per `compute` (control block, DSP block):
10 304 bytes per call for a single-precision program, 14 464 in `-double`,
independent of the frame count. The C++ interpreter keeps these stacks in the
executor. Separate from this plan's requests: follow-up F1 (§6).

## 3. Plan

One commit per step, in this order, each with its journal entry and passing
the local gates (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p faust -p interp-ffi -p codegen`). Every fix comes with a
test checked to fail on `0f240664`. `rust-facade` is pushed and an embedder
builds on it: fix forward, no history rewrite.

### P1: native `f64` compute for the interpreter (§1.1, §2.2)

- `interp-ffi/src/types.rs`: `FbcDspFactoryAny::execute_block_io_f64`, the
  mirror of `execute_block_io_f32`: native on a `Float64` factory, widening
  and narrowing on a `Float32` one. Both conversion paths use buffers kept in
  the instance (`InterpreterDspInstance`), resized, never reallocated once
  large enough.
- `interp-ffi/src/instance.rs`: the `count` slot store and the control block
  factored into one private helper; a Rust-only
  `pub unsafe fn compute_f64(dsp, count, inputs: *const *const f64, outputs: *mut *mut f64)`,
  neither `extern "C"` nor `no_mangle`, so absent from the cbindgen header,
  documented as the facade's entry point. The C entry point keeps its
  signature and behaviour.
- `faust/src/backend.rs`: `RawInstance::compute` takes the width and calls
  `computeCInterpreterDSPInstance` or `interp_ffi::instance::compute_f64`.
- `faust/src/dsp.rs`: `exchanged()` answers the compiled precision on both
  backends. `lib.rs`, section *Precision*: both backends exchange the compiled
  precision; the note on `-double` interpreter programs goes.
- Tests (`crates/faust/tests/api.rs`), on both backends, `-double`:
  `process = _;` returns `1.0 + 2^-40` exactly; `process = 16777217.0;`
  outputs `16777217`. `interp-ffi`: the C entry point still narrows (C++
  parity) and, once its buffers have grown, allocates nothing that depends
  on the frame count (the executor's own per-block stacks remain, §2.2).

API mapping: `compute_f64` is **adapted** (no C++ counterpart: the C++
`interpreter_dsp` exchanges `FAUSTFLOAT`); additive, no C ABI impact.

### P2: exact control ranges in `-double` (§2.1)

- The facade reads `init`, `min`, `max` and `step` from the factory's JSON
  (`Factory::json`, already exposed), matched to the controls by path. First
  check that both backends write these values there at the compiled
  precision (not yet verified). The zones
  and the rest of `ControlMap` are unchanged.
- Test: in `-double`, `hslider("x", 0.1, 0, 1, 0.01)` reports `init == 0.1`,
  and `get` after `reset_controls` equals `control(path).init`, on both
  backends.
- If the JSON does not carry the full precision on one backend, that is a
  backend defect, fixed there with its own test, not worked around in the
  facade.

### P3: `Dsp: Sync` (§1.2)

- Audit the C entries reached from `&self`: `getSampleRate*` and
  `metadata*` on both backends must read only. If one writes (a lazily built
  cache, a counter), that entry is fixed first.
- `unsafe impl Sync for Dsp` with its `SAFETY` argument: every `&self` path
  reads, every write takes `&mut self`, and the lifecycle calls that touch
  the shared factory entry go through the process-wide lock.
- The doc of `Dsp` (“not shared between threads”) and of `lib.rs` updated.
- Tests: a compile-time `assert_send_sync::<Dsp>()` and `::<Factory>()`; a
  `thread::scope` test calling `get`, `controls`, `metadata` and
  `sample_rate` from several threads on one `&Dsp`, both backends.

### P4: `Control::label`, and room to grow (§1.3)

- `pub label: String` on `Control`, the label as the program wrote it.
- `#[non_exhaustive]` on `Control` and on `ErrorKind`, so later fields and
  kinds are not breaking changes; the crate builds its `Control` values
  itself, hosts only read them. Decided now, before the surface is declared
  stable.
- Test: `hslider("my gain", …)` has `label == "my gain"` and a path ending in
  `/my_gain`.

### P5: controls in declaration order (§1.4)

- `ControlMap`: a `Vec<Entry>` in declaration order plus a
  `HashMap<String, usize>` index; `get`, `read` and `write` keep their
  lookup by path.
- Two widgets with the same path: the C++ `MapUI` keeps the last zone
  (`fPathZoneMap[path] = zone`). Same here: the later widget replaces the
  entry, which keeps the position of the first.
- `Dsp::controls()` documented as “in declaration order”.
- Tests: three widgets declared out of alphabetical order across groups come
  back in declaration order; a duplicated path resolves to the last zone and
  appears once.

### P6: a test for the refused Cranelift instance (§1.5)

- Confirm the candidate program reaches the stub (Cranelift factory created,
  `cranelift-compute-body-lowered = false`); otherwise derive one from the
  subset-gap fixtures of `crates/codegen/src/backends/cranelift/tests.rs`
  that can be written in Faust.
- Test in `crates/faust/tests/api.rs`: `instantiate` returns
  `ErrorKind::Instantiate` on Cranelift; the same program on the interpreter
  is either accepted or refused at compile time, whichever it does today,
  asserted.
- The program is given to the embedder in the issue answer.

## 4. Compatibility

- C ABI: unchanged (`computeCInterpreterDSPInstance` keeps its `float**`
  and its narrowing; no new exported symbol).
- `interp-ffi` Rust surface: one additive item, `compute_f64`, used by the
  facade only; `interp-ffi` stays an internal crate.
- `faust` crate: `compute_f64` becomes exact on the interpreter (behaviour
  change, the documented one), `controls()` changes order, `Control` gains a
  field and becomes `#[non_exhaustive]`. The crate is not released; the only
  known embedder asked for these changes.

## 5. Open questions

- P2 relies on the JSON for exact ranges. The alternative, a Rust-only `f64`
  `UIGlue`, would touch `ffi-common`, shared by both backends and by the C
  ABI; not proposed unless the JSON route fails.
- P1 keeps the `f64` entry Rust-only. A C entry
  (`computeCInterpreterDSPInstanceDouble`) would help C hosts too but has no
  C++ counterpart; out of scope unless asked.

## 6. After the plan

- F1: the interpreter executor keeps its evaluation stacks across blocks
  instead of allocating them on every block (§2.2); a test with a counting
  allocator in the style of `crates/interp-ffi/tests/compute_allocation.rs`
  asserts a later `compute` allocates nothing.

- Answer on issue #17: the commits, the program for P6, what `py-faust-rs`
  can drop (the `Mutex`, the last-segment label, the three xfails).
- The embedder's own requests not taken: `Dsp::clone()`, soundfiles,
  per-instance JSON (“none is needed for the binding now”); program-level
  `declare` metadata remains the compiler-side gap noted in `lib.rs`.

## 7. Status

### P1, implemented 2026-09-27

- `interp-ffi`: `FbcDspFactoryAny::execute_block_io_f64` beside
  `execute_block_io_f32`, both converting through `IoScratch`, buffers
  the instance keeps (`InterpreterDspInstance::io_scratch`); the compute
  body shared by `computeCInterpreterDSPInstance` and the Rust-only
  `instance::compute_f64` (`compute_with`). C ABI and header unchanged.
- `faust`: `RawInstance::compute` generic over a crate-private `Sample`
  (`f32`, `f64`), dispatching the interpreter's `f64` channels to
  `compute_f64`; `Dsp::exchanged` is the compiled precision on both backends.
- Tests: `a_double_program_exchanges_f64_samples_exactly_on_both_backends`
  (fails on `0f240664`: `interp: the input was narrowed`) and
  `f64_buffers_on_a_single_precision_program_are_converted`
  (`crates/faust/tests/api.rs`); `crates/interp-ffi/tests/compute_io.rs`
  (the `f64` entry, and the C entry still narrowing);
  `crates/interp-ffi/tests/compute_allocation.rs` (fails on `0f240664`:
  276 704 bytes on every call of a 2-in 2-out 8192-frame block).

P2 to P6 not started.
