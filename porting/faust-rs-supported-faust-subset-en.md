# Current Faust Source Subset Supported by `faust-rs`

Last updated: 2026-05-20

Version: 0.5.0

Status: living document

## 1. Purpose

This document records, in one place, what subset of Faust programs currently
compiles with `faust-rs`, how that differs from the C++ compiler, and why.

It is intentionally written as a **living status document**:

- it should be updated when the supported subset grows or shrinks,
- it should stay tied to executable evidence,
- it should distinguish clearly between front-end support and end-to-end backend
  support.

## 2. What "compiles" means here

There are two materially different questions:

1. **Front-end compilation**
   - `parse -> eval -> propagate -> signals`
   - measured here by `compiler::Compiler::compile_file_default_to_signals`
2. **End-to-end backend compilation**
   - `parse -> eval -> propagate -> signal_prepare -> signal_fir -> backend`
   - measured here by the current fast-lane route
     `compiler::SignalFirLane::TransformFastLane`

This distinction matters because the front-end language surface is now much
broader than the active signal-to-FIR lowering slice.

## 3. Evidence Used for This Snapshot

This snapshot is based on:

- local history and porting notes under `porting/journal/`
- current implementation status in:
  - `crates/parser`
  - `crates/eval`
  - `crates/propagate`
  - `crates/transform/src/signal_prepare.rs`
  - `crates/transform/src/signal_fir/mod.rs`
  - `crates/transform/src/signal_fir/module.rs`
- corpus/backend reports currently checked into `porting/phases/`:
  - `cargo run -p xtask -- corpus-status-report`
  - `cargo run -p xtask -- backend-full-corpus-diff-report`
- follow-up front-end parity fixes landed on 2026-03-29 and reviewed against:
  - `crates/parser/src/lib.rs`
  - `crates/eval/src/lib.rs`
  - `crates/eval/tests/core_eval.rs`
  - `crates/compiler/tests/inline_environment_import_parity.rs`
  - `porting/journal/2026-03-29.md`
- fast-lane/runtime fixes landed on 2026-04-03 and reviewed against:
  - `crates/transform/src/signal_prepare.rs`
  - `crates/transform/src/signal_fir/module.rs`
  - `crates/codegen/src/backends/interp/serial.rs`
  - `crates/compiler/tests/signal_fir_lane.rs`
  - `porting/journal/2026-04-03.md`
- FIR runtime optimization and signal-type correctness fixes landed on 2026-04-04–05 and reviewed against:
  - `crates/sigtype/src/rules.rs`
  - `crates/transform/src/signal_fir/module.rs`
  - `crates/transform/src/signal_fir/cse.rs`
  - `crates/fir/src/checker.rs`
  - `crates/codegen/src/backends/cranelift/mod.rs`
  - `crates/cranelift-ffi/src/instance.rs`
  - `porting/journal/2026-04-04.md`
  - `porting/journal/2026-04-05.md`
- delay-strategy / recursion-delay parity and sample-loop ordering work landed on 2026-04-08 and reviewed against:
  - `crates/transform/src/signal_fir/delay.rs`
  - `crates/transform/src/signal_fir/module.rs`
  - `crates/transform/src/signal_fir/siggen.rs`
  - `crates/transform/src/signal_fir/tests.rs`
  - `crates/compiler/tests/signal_fir_lane.rs`
  - `porting/journal/2026-04-08.md`
- transform preparation boundary/encapsulation fixes landed on 2026-04-10 and reviewed against:
  - `crates/transform/src/signal_prepare.rs`
  - `crates/transform/src/signal_fir/mod.rs`
  - `crates/transform/src/signal_fir/module.rs`
  - `crates/transform/src/signal_fir/planner.rs`
  - `crates/transform/src/signal_fir/siggen.rs`
  - `porting/journal/2026-04-10.md`
- forward-mode AD (`fad`) implementation documented and 22 corpus entries validated on 2026-04-15
  and reviewed against:
  - `crates/propagate/src/forward_ad.rs`
  - `crates/propagate/src/lib.rs`
  - `crates/propagate/tests/core_api.rs`
  - `crates/compiler/tests/signal_pipeline.rs`
  - `crates/compiler/tests/signal_fir_lane.rs`
- FAD overhaul on 2026-04-21 (De Bruijn differentiation, new rules, depth fixes) reviewed against:
  - `crates/propagate/src/forward_ad.rs`
  - `crates/propagate/tests/core_api.rs`
  - `crates/eval/src/loop_detector.rs`
  - `crates/compiler/src/main.rs`
  - `porting/journal/2026-04-21.md`
- unified recursive multi-seed FAD refactor and documentation hardening on
  2026-04-23 reviewed against:
  - `crates/propagate/src/forward_ad.rs`
  - `crates/propagate/tests/core_api.rs`
  - `crates/compiler/tests/signal_pipeline.rs`
  - `crates/compiler/tests/fad_recursive_runtime.rs`
  - `porting/fad-n-lanes-unified-rec-plan-2026-04-23-en.md`
  - `porting/journal/2026-04-23.md`
- read-only table/waveform FAD extension on 2026-04-23 reviewed against:
  - `crates/propagate/src/forward_ad.rs`
  - `crates/compiler/tests/signal_pipeline.rs`
  - `crates/compiler/tests/fad_recursive_runtime.rs`
  - `tests/corpus/fad_waveform_index_basic.dsp`
  - `tests/corpus/fad_rdtbl_index_basic.dsp`
  - `tests/corpus/fad_recursive_waveform_index.dsp`
  - `tests/corpus/fad_rwtable_index_zero_tangent.dsp`
  - `porting/journal/2026-04-23.md`
- nested-recursion slot_env capture fix on 2026-04-23 reviewed against:
  - `crates/propagate/src/lib.rs` (Rec arm, slot_env lift ordering)
  - `porting/journal/2026-04-23.md`
- eval stack-overflow fix, Infinity/NaN codegen bug, FAD white paper, and eval
  performance improvements on 2026-04-24–25 reviewed against:
  - `crates/eval/src/lib.rs` (structural recursion budget extension)
  - `crates/codegen/src/backends/c/mod.rs` and `cpp/mod.rs` (non-finite literals)
  - `docs/fad-note-en.md`
  - `porting/journal/2026-04-24.md`, `porting/journal/2026-04-25.md`
- reverse-mode AD (`rad(expr, seeds)`) full implementation (phases A–D + E0),
  corpus, docs, and nested AD tests on 2026-04-27 reviewed against:
  - `crates/propagate/src/reverse_ad.rs`
  - `crates/propagate/src/stateful_rad.rs`
  - `crates/compiler/tests/rad_runtime.rs`
  - `docs/rad-note-en.md`
  - `porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md`
  - `porting/journal/2026-04-27.md`
- RAD E1 scaffold, phase-E0 classifier integration, numeric oracle, and E2/F
  diagnostic coverage on 2026-04-28 reviewed against:
  - `crates/propagate/src/stateful_rad.rs`
  - `crates/propagate/src/transpose_ad.rs`
  - `porting/journal/2026-04-28.md`
- normalize simplify-cache sharing, wasm-ffi unblock, RAD corpus demos and
  gradient-descent example, and LTI transposition plan §20 on 2026-04-29
  reviewed against:
  - `crates/normalize/src/simplify.rs`
  - `crates/wasm-ffi/src/lib.rs`
  - `crates/compiler/examples/rad_gradient_descent.rs`
  - `docs/rad-usage-en.md`
  - `porting/journal/2026-04-29.md`
- Cranelift AArch64 oversized-body guard and evaluator depth-budget
  configurability on 2026-04-30 reviewed against:
  - `crates/codegen/src/backends/cranelift/mod.rs`
  - `crates/eval/src/lib.rs`
  - `porting/journal/2026-04-30.md`
- isomorphic `SYMREC` group merging pass, Cranelift saturating float-to-int
  cast fix, `x − x → 0` simplification rule on 2026-05-01 reviewed against:
  - `crates/normalize/src/rec_merge.rs`
  - `crates/transform/src/signal_prepare.rs`
  - `crates/codegen/src/backends/cranelift/mod.rs`
  - `porting/journal/2026-05-01.md`
- full SVG block-diagram draw module (phases A–I) and `-svg` CLI flag on
  2026-05-02 reviewed against:
  - `crates/draw/src/`
  - `crates/compiler/src/main.rs`
  - `porting/journal/2026-05-02.md`
- def-name propagation fix for SVG folding, FAD rewrite-rule table,
  `expandDSP`/`generateAuxFiles` implementation and C/C++ FFI exports on
  2026-05-03 reviewed against:
  - `crates/eval/src/lib.rs`
  - `crates/compiler/src/lib.rs`
  - `crates/cranelift-ffi/src/factory.rs`
  - `crates/interp-ffi/src/factory.rs`
  - `porting/journal/2026-05-03.md`
- RAD reverse-time-recursion safety and BRA tape typing fixes on
  2026-05-10/12 reviewed against:
  - `crates/propagate/src/reverse_ad.rs`
  - `crates/transform/src/signal_fir/module.rs`
  - `crates/compiler/tests/rad_runtime.rs`
  - `porting/rad-disable-reverse-time-rec-fast-path-plan-2026-05-10-en.md`
  - `porting/journal/2026-05-12.md`
- AD wrapper arity-slicing fix on 2026-05-12 reviewed against:
  - `crates/propagate/src/lib.rs`
  - `crates/compiler/tests/signal_pipeline.rs`
  - `porting/journal/2026-05-12.md`
- initial Julia backend, Julia architecture wrapping, UI-zone parity, double
  precision aliasing, numeric cast parity, and Julia math aliases landed on
  2026-05-13/14 and reviewed against:
  - `crates/codegen/src/backends/julia/mod.rs`
  - `crates/compiler/src/main.rs`
  - `crates/compiler/src/enrobage.rs`
  - `crates/compiler/tests/signal_fir_lane.rs`
  - `crates/compiler/tests/enrobage_integration.rs`
  - `porting/julia-backend-plan-2026-05-13-en.md`
  - `porting/journal/2026-05-13.md`
  - `porting/journal/2026-05-14.md`
- RAD local-rule factorization and shared Signal/FIR formula helpers landed on
  2026-05-17 and reviewed against:
  - `crates/signals/src/ad_rules.rs`
  - `crates/propagate/src/reverse_ad.rs`
  - `crates/transform/src/signal_fir/module.rs`
  - `porting/rad-local-rule-factorization-plan-2026-05-17-en.md`
  - `porting/journal/2026-05-17.md`

The corresponding generated reports are:

- [phase-4-corpus-status-diff-report-en.md](./phases/phase-4-corpus-status-diff-report-en.md)
- [phase-6-backend-full-corpus-diff-report-en.md](./phases/phase-6-backend-full-corpus-diff-report-en.md)

## 4. Executive Summary

### 4.1 Source-language status

At the front-end level, the checked-in corpus-status report now separates two
classes:

- Faust C++ parity cases, where both compilers accept or both reject the same
  source,
- Rust-extension cases, mostly `fad(...)` / `rad(...)` fixtures, where Rust
  accepts programs that the pinned C++ reference rejects because those symbols
  do not exist there.

- total corpus cases in the latest checked-in report: `220`
- `OK/OK` cases accepted by both Rust and C++: `103`
- `ERR/ERR` cases rejected by both Rust and C++: `39`
- `OK/ERR` mismatches: `0`
- `ERR/OK` Rust-extension cases: `78`

Important note:

- the regenerated reports cover `220` files under `tests/corpus/`
- many `ERR/OK` entries are intentional Rust language extensions used to test
  FAD/RAD, not regressions against the portable Faust C++ subset
- the "portable C++ subset" still has no `OK/ERR` front-end mismatch in this
  snapshot

In other words:

- on the current tracked corpus, `faust-rs` accepts every source that the
  pinned C++ reference accepts up to the `signals` boundary,
- it rejects the same invalid portable corpus programs,
- and it additionally accepts the Rust-only AD fixtures.

### 4.2 End-to-end backend status

At the current backend route (`TransformFastLane`), the supported subset is
still narrower. The latest checked-in full backend report says:

- total corpus cases: `220`
- end-to-end C backend parity: `OK=103`, `DIFF=0`, `UNSUPPORTED=117`
- end-to-end C++ backend parity: `OK=103`, `DIFF=0`, `UNSUPPORTED=117`

The `117` unsupported entries combine two situations:

- intentionally invalid Faust programs,
- Rust-only AD fixtures that compile in Rust but have no C++ reference backend
  because the pinned C++ compiler rejects `fad`/`rad` as undefined symbols.

So for **C++-accepted portable** corpus programs, the current backend route
compiles:

- `103 / 103` portable backend cases in the maintained corpus.

This backend-shell result does not claim numerical parity. In particular,
`rep_18_stream_wrappers.dsp` still exposes a runtime-output difference in the
corpus numerical differential.

### 4.3 WASM / JSON backend status

The Rust backend now also has a real WebAssembly output path, plus an initial
embedded-compiler path for `faustwasm`, but its maturity level still differs
from the C/C++ backend route described above.

What is now true:

- `-lang wasm` emits a matched pair:
  - `<name>.wasm`
  - companion `<name>.json`
- the companion JSON now carries the main provenance/runtime fields expected by
  current Faust web tooling:
  - `filename`
  - `version`
  - `compile_options`
  - `include_pathnames`
  - `library_list`
  - UI widget `index`
  - `size`
  - `sr_index`
- strict `-json` is wired as a first-class CLI mode and can now also be used
  alongside `-lang <backend>`, like in the C++ compiler workflow
- `-lang wast` is now also wired and renders textual WAT/WAST from the same
  backend via `wasmprinter`
- for `-lang wasm`, the companion JSON and exported ABI are now coherent enough
  to work with the standard `faustwasm` runtime on validated cases such as
  `osc.dsp`
- the Rust `wasm-ffi` compiler-module can now be built as a standalone
  `wasm32-unknown-unknown` artifact and loaded by `faustwasm` as an embedded
  compiler
- that compiler-module now embeds the standard Faust library set as read-only
  virtual sources, so source-string compilation can resolve imports such as
  `import("stdfaust.lib")` without an Emscripten-style filesystem
- the embedded-compiler path is validated end-to-end in `faustwasm` on:
  - mono DSP compilation using the standard runtime path
  - polyphonic `faust2wasm.js` generation, including packaged internal mixer
    fallback, on `organ.dsp`

What is not yet true:

- the Rust WASM backend should not yet be described as full semantic parity
  with the C++ WASM backend
- lowering coverage is still an incremental subset and is currently being
  extended from real-world DSP failures and wrapper/runtime tests
- exact byte-for-byte parity of all public UI offsets/layout details against
  the C++ backend is not yet claimed, even though the current ABI contract is
  now much closer and is already functional with `faustwasm` on the validated
  path
- helper parity for the embedded-compiler path is still incomplete:
  - `getInfos(...)` is only partially implemented
  - `expandDSP(...)` and `generateAuxFiles(...)` are now real implementations
    (landed 2026-05-03): `expandDSP` validates source and returns the original
    source string (the Rust compiler has no box→DSP serializer analogous to C++
    `printBox`); `generateAuxFiles` compiles source and writes one or more
    output files for `-cpp`, `-c`, `-wasm`, `-json`, and `-svg` flags; both
    are wired into the C/C++ FFI headers for Cranelift and interpreter backends

### 4.4 Julia backend status

The Rust compiler now has an initial Julia source backend selected with
`-lang julia`.

What is now true:

- `faust-rs -lang julia foo.dsp -o foo.jl` emits a Faust-style Julia DSP shell
  from FIR:
  - `mutable struct mydsp{T} <: dsp`
  - lifecycle/API methods
  - `compute!(dsp::mydsp{T}, count::Int32, inputs::Matrix{FAUSTFLOAT},
    outputs::Matrix{FAUSTFLOAT})`
  - one-based Julia table indexing at the final table access boundary, while
    zero-based FIR loop and offset arithmetic are preserved internally
- `-double` now propagates into the generated Julia `REAL` alias
  (`const REAL = Float64`), matching the Julia impulse architecture's
  `FAUSTFLOAT = Float64` path.
- `-a/--architecture` wrapping is supported for Julia in the same CLI enrobage
  pipeline as C/C++ backends. The architecture marker replacement, class-name
  rewrite, Julia `include(...)` scanning, and recoverable injected include
  handling are covered by tests.
- UI callbacks now pass Faust Julia runtime zone identifiers as symbols
  (`:fHslider0`, `:fButton0`, etc.), matching the C++ Julia backend and the
  installed Julia runtime signatures.
- Numeric casts are aligned with the C++ Julia backend:
  - real casts inside `mydsp{T}` emit `T(...)`
  - `Int64` casts use `trunc(Int64, ...)`
  - `Int32` casts use the generated `faust_wrap_int32(...)` helper so
    out-of-range values wrap instead of raising Julia `InexactError`
- The Julia prologue now defines Faust math aliases needed by the impulse
  corpus:
  - `fmod(x, y) = rem(x, y)`
  - `atan2(y, x) = atan(y, x)`
- Float literal emission now uses Rust's shortest round-trippable formatting,
  preserving small constants such as noise LCG scales.

What is not yet true:

- the Julia backend should be treated as a functional first slice, not as full
  semantic parity with Faust C++ `-lang julia`;
- it consumes the existing FIR fast-lane subset, so any upstream
  `signal_prepare`/`signal_fir` exclusion remains a Julia exclusion too;
- it assumes the host provides the Faust Julia runtime names (`dsp`, `UI`,
  `FMeta`, `FAUSTFLOAT`, UI callback methods). The backend emits source; it
  does not package the Julia runtime;
- full `/Users/letz/Developpements/faust/tests/impulse-tests` `make julia`
  coverage was not completed in the 2026-05-13 validation session, although
  targeted generated Julia impulse checks were run for representative cases
  including `grain3.dsp`, `noise.dsp`, `noiseabs.dsp`, `pow.dsp`,
  `priority1.dsp`, and `math.dsp`.

## 5. Synthetic Characterization of the Supported Subset

## 5.1 Source-level Faust subset currently supported

As of this snapshot, `faust-rs` front-end support is no longer a tiny parser
prototype subset. On the tracked corpus, it includes:

- standard arithmetic and signal composition forms,
- local definitions and `with` scopes,
- recursive definitions / `letrec`,
- lambda abstraction and application,
- `case` / pattern-based evaluation paths,
- metadata and `declare`,
- local-file imports, including structural expansion from parsed
  `importFile(...)` nodes,
- inline `environment { import(...) }` forms in the tracked parity scope,
- UI widgets and UI groups,
- modulation-bearing source programs,
- waveform and table forms,
- representative feedback and delay programs,
- label interpolation cases used by modulation/UI paths,
- representative noise/additive-synthesis fixtures,
- **forward-mode AD**: `fad(expr, seed)` is supported at the source and
  propagation level; 39 `fad*` corpus entries cover the full rule spectrum
  (arithmetic, trig, `pow`, `min`/`max`, `atan2`, `fmod`, `remainder`,
  delays, recursion, `select2`, bargraphs, multi-seed, lambda-bound
  recursive seeds, nested `fad` on recursive seeds, nested recursion,
  multi-output recursion, and mutual/crossed recursion). `[autodiff:false]`
  metadata is parsed but no longer gates differentiation; seed selection is
  entirely driven by the explicit `seed` argument. The `seed` sub-expression
  may itself produce
  **M ≥ 1 outputs** — a single `fad` node then bundles M independent
  differentiation variables and emits `body_outputs × (1 + M)` signals laid
  out as `[primal, ∂/∂seed₀, …, ∂/∂seed_{M−1}]` per primal output. Seeds
  with 0 outputs are rejected with a dedicated arity diagnostic.
  Differentiation is performed directly on De Bruijn recursive form
  (`DEBRUIJNREC`/`DEBRUIJNREF`) by interleaving primal and tangent slots in
  the expanded body; symbolic conversion (`de_bruijn_to_sym`) runs
  afterwards in `signal_prepare`. The current Rust implementation also
  differs intentionally from Faust C++ in one optimization-sensitive area:
  a single transform now carries all seed lanes through one unified recursive
  group instead of rebuilding one recursive primal shadow per seed. As of
  2026-04-23, read-only lookup forms are also in scope: `fad` now
  differentiates `SIGRDTBL` when the table source is a waveform literal or a
  read-only generated table, using a documented symmetric finite-difference
  approximation over the read index
  `((rdtbl(T, i + 1) - rdtbl(T, i - 1)) / 2) * i'`. This is an `adapted`
  Rust extension rather than a claim of exact Faust C++ parity.
  The supported differentiable subset remains explicit-rule-driven: unknown
  `FFun`s, writable table reads/writes, soundfiles, standalone waveforms,
  integer-only/bitwise operators, `int_cast`/`bit_cast`, discrete controls
  (`button`, `checkbox`) and other unmatched signal families currently
  preserve the primal and emit zero tangents rather than claiming a
  derivative model that has not been ported.
- **reverse-mode AD**: `rad(expr, seeds)` is supported for the feed-forward
  differentiable subset. It mirrors the explicit-seed surface of `fad` and
  returns `[primals..., gradients...]`, with one gradient lane per seed output.
  For multi-output bodies, the cotangent at the body boundary is the implicit
  all-ones vector, so the gradient is taken with respect to the sum of body
  outputs. Temporal or recursive bodies (`delay`, `prefix`, recursion /
  projection) leave the symbolic feed-forward sweep and are routed through the
  `BlockReverseAD` fallback rather than being differentiated by the local
  symbolic transpose. As of 2026-05-12, `fad` and `rad` both obey the same
  wrapper input-slicing contract: the wrapper exposes the maximum child input
  arity externally, while each child is propagated with only the prefix of the
  bus matching its own arity. Thus `rad(*, (_,_,_))` is legal: the wrapper has
  3 inputs, the body `*` consumes `x0,x1`, and the third seed lane correctly
  produces gradient zero.

Stated differently:

- the current front-end subset is best described as **"the current tracked Faust
  corpus"**, not as a small manually enumerated syntax fragment.
- the main residual front-end caveat is not basic language acceptance anymore,
  but long-tail parity outside the exercised corpus and some deferred tooling
  concerns such as remote-import parity.
- **SVG block-diagram output**: `-svg` is now a first-class CLI flag (landed
  2026-05-02). The full schema/device infrastructure is implemented in
  `crates/draw/`: all composition forms (`Seq`, `Par`, `Split`, `Merge`,
  `Rec`), leaf schemas, decorator schemas, route, multirate wrappers, UI
  widgets, and groups are rendered. Hierarchical folding (`-f N`, `-fc N`)
  splits complex diagrams into multiple linked SVG files. Visual options
  `-blur`, `-sc`, `-drf`, and `-mns N` are supported. The evaluated box
  tree (post-`eval`) is used as source so all lambda applications appear as
  resolved primitives.

- parser entry points still differ intentionally:
  - `parse_program(...)` parses one in-memory source unit and does **not**
    perform filesystem import resolution,
  - structural import resolution belongs to `parse_file_with_imports(...)` and
    `parse_program_with_imports_and_metadata(...)`.

## 5.2 End-to-end backend subset currently supported

The backend subset is narrower and is better characterized structurally.

For the production C/C++ route, a Faust program currently compiles end-to-end
when its post-eval/post-propagate signal forest stays within the active
`signal_prepare + signal_fir` lowering slice.

More precisely, `signal_prepare` now acts as an explicit verified staging
boundary before FIR emission:

- it clones the output forest into a private staging arena,
- runs the current normalization/type/promotion/canonicalization subset there,
- and verifies the resulting postconditions before `signal_fir` consumes the
  prepared outputs and type maps.

For the Rust WASM route, the same FIR preparation pipeline is reused, but the
backend lowering itself is currently a separate, narrower bring-up slice with
its own documented subset in `crates/codegen/src/backends/wasm/`.

For the Rust Julia route, the same FIR preparation pipeline is reused and the
backend emits Julia source from the resulting FIR `Module`. Its source-language
coverage is therefore bounded first by the shared fast-lane subset, then by the
Julia emitter's current FIR-node coverage. The Julia backend is now useful for
representative impulse-test DSPs, but should still be described as an initial
backend slice rather than a complete replacement for Faust C++ `-lang julia`.

That slice currently includes, in broad terms:

- numeric constants and audio inputs/outputs,
- arithmetic, comparison, bitwise, and selected math operators,
- `select2`, `min`, `max`, `abs`,
- `delay1`, `prefix`, and recursive feedback forms — including
  **multi-output recursion groups** (`SIGPROJ index > 0`),
- `SIGDELAY` with:
  - constant integer amounts lowered through the same three strategy families
    as Faust C++:
    - very small delays: direct `Shift` copies,
    - middle-range delays: power-of-two circular buffers with `fIOTA`,
    - large delays / `-dlt`: exact-size if-wrapping buffers,
  - **variable amounts with a bounded non-negative interval** — the buffer is
    allocated to `next_power_of_two(max + 1)` using the interval upper bound
    from `crates/sigtype`.  This covers:
    - UI parameters (slider, numentry) with explicit `[lo, hi]` bounds,
    - audio-rate signals (`Variability::Samp`) whose interval is bounded and
      non-negative (e.g. `@(int(_+10))` where `int(_+10) ∈ [9, 11]`),
    - expressions derived from `ma.SR` (sampling rate), which carries the
      open interval `[f64::MIN, f64::MAX]` — clamped to a finite range by the
      surrounding `min(192000, max(1, …))` algebra,
  - structural fallback: when interval analysis fails to yield a finite bound
    but the amount expression is `SIGMIN(SigInt(n), _)`, `n` is used as a
    conservative ceiling (defence-in-depth, not normally reached with correct
    `FConst` typing),
- waveform/table forms:
  - `SIGWAVEFORM`
  - `SIGRDTBL`
  - `SIGWRTBL`
- UI widget families:
  - buttons
  - checkboxes
  - sliders
  - bargraphs
- control-style wrappers already mapped in the fast-lane:
  - `attach`
  - `enable`
  - `control`
- foreign constants (`fconstant`) and foreign variables (`fvariable`), typed
  with the fully-open interval `[f64::MIN, f64::MAX]` — matching the C++
  default-constructed `interval()`.

This is why corpus cases such as:

- `rep_04_delay_echo`
- `rep_06_comb_feedback`
- `rep_20_environment_waveform`
- `rep_35_table_rwtable_runtime_write`
- `rep_46_case_pattern_constant_folding`
- `rep_55_sine_phasor_echo_feedback`
- `rep_56_noise_smoo_slider`
- `rep_65_variable_delay_slider`
- `rep_66_variable_delay_feedback`
- `rep_67_variable_delay_shifted_slider`
- `rep_68_variable_delay_audio_rate`
- `rep_69_variable_delay_sr_millisec`
- `rep_70_route_arithmetic_params`
- `rep_71_degenerate_unary_recursion`
- `rep_72_float_literal_pattern`
- `rep_73_pattern_max_min_fold`
- `rep_60_counter_rem`, `rep_61_fmin_sr`, `rep_62_select2_trigger`,
  `rep_63_rwtable`, `rep_63_store_load_table`, `rep_64_dynamic_rem`,
  `rep_65_fabs_trigger`

now compile end-to-end through the Rust backends.

Since that snapshot, the validated fast-lane/runtime path has also absorbed
several important correctness fixes without changing the high-level subset
classification:

- post-`simplify` re-typing/promotion before FIR lowering,
- canonicalization of `Delay(x, 1)` back to `Delay1(x)` at the preparation
  boundary,
- preservation of non-numeric `Konst` factors in normalization for
  `fSamplingFreq`-driven delay amounts,
- correct `instanceResetUserInterface` generation for UI-only controls in the
  wasm path,
- round-trip-precise FBC real literal serialization,
- correct previous-step semantics for non-recursive `Delay1` inside the SIGGEN
  table-generator interpreter,
- C++-parity delay strategy thresholds at the `-mcd` / `-dlt` boundaries,
- pre-scan coverage for standalone `Delay1(x)` so delay strategy/geometry is
  chosen once up front,
- accumulated delay analysis for recursion carriers, so
  `Delay1^k(Proj(...))` chains can reuse one upsized recursion array instead of
  separate `fVec` buffers,
- simple-recursion lowering aligned with Faust C++ for 2-slot feedback arrays,
- explicit sample-loop emission phases (`Immediate`, `PostOutput`,
  `SampleEnd`) so `Shift` copies and delay counter updates now have a clearer
  documented ordering model,
- nested-recursion DeBruijn scope correctness: `slot_env` is now lifted
  before propagating both the feedback and main branches of a `Rec` node,
  so outer-scope captures reached through the feedback path (e.g.
  `fi.lowpass(f_curr, x)` called from inside a `~` loop) stay bound to the
  intended outer `DEBRUIJNREC` instead of being captured by the inner one,
- isomorphic `SYMREC` group merging (`rec_merge` pass in `crates/normalize/`,
  landed 2026-05-01): `SYMREC` groups whose bodies are structurally equivalent
  after opening (replacing `Proj(i, self)` with canonical `Hole(i)` sentinels)
  are unified to a single canonical representative before simplification. This
  enables the subsequent `x − x → 0` rule to fire across independently-built
  recursive systems — most visibly on `fad_rec.dsp`-class programs where the
  primal and tangent recursive groups are isomorphic.

## 5.3 Important current backend exclusions

The backend subset is **not** "all front-end-accepted Faust programs".

The most important current exclusions are:

- **Variable delays with no statically bounded interval**
  - `SIGDELAY` whose amount expression has an unbounded interval (i.e., the
    interval upper bound is infinite or indeterminate) is rejected.
  - This is narrower than the previous `Variability::Samp` blanket rejection:
    audio-rate amounts are now accepted when their interval is provably bounded
    and non-negative.
  - A pure audio input used directly as a delay amount is no longer the blocker
    it used to be if type/interval inference yields a finite non-negative upper
    bound; the remaining exclusion is the genuinely unbounded case, where no
    finite static capacity can be proven.

- **Non-trivial stream wrappers**
  - the current valid backend corpus gap is
    `tests/corpus/rep_18_stream_wrappers.dsp`
  - trivial wrappers such as `inputs(_)` / `outputs(_)` are covered,
    but the full `ondemand(_)`, `upsampling(_)`, `downsampling(_)` family is
    not yet fully lowered end-to-end.

- **Julia backend maturity**
  - `-lang julia` is now wired and usable, including `-double` and
    architecture wrapping, but it is still an initial FIR emitter slice.
  - Programs outside the shared fast-lane subset are outside Julia too.
  - The emitted source assumes a Faust Julia runtime provided by the host; Rust
    does not package or vendor that runtime in the generated file.
  - Full Faust impulse-test parity is not yet claimed. Current validation is a
    mix of codegen/unit tests, CLI/enrobage tests, and selected generated Julia
    impulse runs.

- **Reverse-mode AD (`rad(expr, seeds)`) — feed-forward subset supported**
  - `rad(expr, seeds)` is implemented at the propagation boundary in
    `crates/propagate/src/reverse_ad.rs`. It mirrors the explicit-seed
    surface of `fad(expr, seeds)` and produces the bundle
    `[primals…, ∂ sum(primals) / ∂ s_0, …, ∂ sum(primals) / ∂ s_{N-1}]`
    with an implicit all-ones cotangent on every primal output.
  - Wrapper input arity follows the same rule as FAD: the public wrapper arity
    is the maximum of `body.inputs` and `seeds.inputs`, but the body and seeds
    are each propagated with only the input prefix required by their own arity.
    For example, `rad(*, (_,_,_))` has three public inputs and four outputs:
    `x0 * x1`, `d(x0*x1)/d(x0) = x1`, `d(x0*x1)/d(x1) = x0`,
    `d(x0*x1)/d(x2) = 0`.
  - Arity contract: `body.outputs ≥ 1`, `seeds.outputs ≥ 1`,
    `outputs = body.outputs + seeds.outputs`. Violations surface
    structured `RadBodyArity` / `RadSeedArity` diagnostics
    (`FRS-PROP-0002`).
  - Differentiable subset: feed-forward arithmetic, unary trig and
    transcendentals (sin/cos/tan/exp/log/log10/sqrt/abs and their
    inverses), `pow`, `atan2`, `min`/`max`, `select2`, `float_cast`
    (`int_cast` is zero), read-only `rdtable` (slope via symmetric
    finite difference), unary foreign functions
    (`tanh`/`sinh`/`cosh` and inverse-hyperbolic counterparts),
    pass-through wrappers (`attach`, `enable`, `control`, `Output`),
    bargraphs (zero contribution).
  - Outside the local symbolic sweep: `delay1`, variable `delay`,
    `prefix`, recursion / projection over a recursion. These kinds raise
    `RadUnsupportedNode` internally as a dispatch signal and are then routed
    to the `BlockReverseAD` fallback, never silently zeroed.
  - Still out of scope as hard unsupported families: mutable
    tables, soundfiles, non-unary or unrecognised foreign functions.
    The reverse transpose of a delay is anti-causal
    (`adj_x[n] += adj_y[n + 1]`) and would require a runtime tape; a
    finite-horizon block fallback is used by the current lowering while the
    more specialized phase-E recursive strategies remain incomplete.
  - Stateful RAD phase E0 is present as an internal feasibility
    classifier (`crates/propagate/src/stateful_rad.rs`) over
    `DEBRUIJNREC` groups. It distinguishes `LinearLti`,
    `LinearTimeVarying`, and `Nonlinear` recursive bodies, but it does
    does not enable the dormant `ReverseTimeRec` fast path yet. The paired
    `RecRadMode`
    gate maps those classes to the future E1/E2/F strategies:
    `LinearTranspose`, `BlockLinearTimeVarying`, and `BpttRequired`.
    RAD recursive fallback classification now carries that mode when it can be
    classified.
  - The experimental reverse-time recursion dispatcher fast path is disabled
    for RAD until the phase-E recursive strategies are complete. Recursive RAD
    bodies therefore route through the `BlockReverseAD` fallback instead of
    attempting an incomplete `ReverseTimeRec` lowering.
  - Block reverse AD (BRA) tape stores use real-valued tape arrays. Because
    BRA tape store buffers are introduced later during FIR lowering, after
    signal-level normalization/promotion, taped integer forward values are cast
    to the tape element type at the FIR boundary before being stored.
  - As of 2026-05-17, local RAD math rules are factored through
    `signals::ad_rules`. The pure Signal symbolic path and the FIR/BRA lowering
    now share backend-neutral rule classification and formula construction for
    unary math, binary math, and differentiable arithmetic `SIGBINOP`s. Tape
    allocation, forward-value loading, reverse-sweep scheduling, FIR helper
    registration, and backend typing remain owned by `transform::signal_fir`.
    The shared `pow` rule uses the stable base derivative form
    `y_bar * exponent * pow(base, exponent - 1)` instead of `base^exponent /
    base`.
  - Coverage:
    - structural: [crates/propagate/tests/core_api.rs](../crates/propagate/tests/core_api.rs)
      (arity + temporal/recursive BlockReverseAD fallback),
    - local-rule unit tests:
      [crates/signals/src/ad_rules.rs](../crates/signals/src/ad_rules.rs),
    - structural classifier unit tests:
      [crates/propagate/src/stateful_rad.rs](../crates/propagate/src/stateful_rad.rs),
    - runtime parity (RAD ↔ FAD lane-by-lane and RAD ↔ central
      finite difference) and corpus-driven regressions:
      [crates/compiler/tests/rad_runtime.rs](../crates/compiler/tests/rad_runtime.rs),
    - corpus fixtures `rad_basic`, `rad_product_multi_seed`,
      `rad_trig_composition`, `rad_absent_seed`, `rad_repeated_seed`,
      `rad_multi_output_sum_cotangent`, `rad_rdtbl_index_basic`, plus
      `err_rad_zero_body`, `err_rad_zero_seed`,
      `rad_delay1_block_fallback`.
  - Detailed design notes: [docs/rad-note-en.md](../docs/rad-note-en.md);
    plan: [porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md](reverse-ad-rad-implementation-plan-2026-04-27-en.md).

- **`fad(expr)` — propagation is supported; end-to-end backend is conditional**
  - `fad(expr)` correctly expands the output bundle at the propagation level:
    each primal output gains one tangent signal per reachable differentiable
    control.
  - The expanded tangent signals use the same node families as primal signals
    (BinOp, trig, pow, delays, recursion, …), so end-to-end backend compilation
    succeeds whenever the underlying node kinds are already supported.
  - No special backend handling of tangent outputs is needed; the fast-lane
    sees an ordinary (wider) signal list.
  - `fad_delay.dsp` is validated through the `signal_fir_lane` fast-lane test.
  - Caveat: `[autodiff:false]` metadata causes the box-level arity count to
    be an upper bound only — the actual signal list may be shorter.  The fast-lane
    output-arity validation accounts for this discrepancy.

- **Unproven long-tail families outside the tracked corpus**
  - even when a node family exists in Rust, if it is not exercised by the
    current corpus and differential tests, it should not yet be described as
    parity-complete.

## 6. Comparison with Faust C++

## 6.1 Where Rust is now close to C++

Relative to the tracked corpus and the current production-oriented route:

- front-end acceptance is now effectively at corpus parity,
- C and C++ backend shell signatures match on all currently supported backend
  cases,
- the Rust CLI now supports `-json` both as a standalone strict JSON mode and
  alongside `-lang <backend>`, with truthful `compile_options` in the emitted
  JSON,
- the Rust WASM backend now emits a usable companion JSON contract for web
  runtimes and writes the same `.wasm` + `.json` pair shape as the C++ CLI,
- the Rust toolchain now also exposes two additional WASM-facing delivery
  shapes:
  - `-lang wast` for textual WAT output,
  - `crates/wasm-ffi` for a raw embedded-compiler module consumed by
    `faustwasm`,
- the Rust CLI now supports `-lang julia` as a real backend mode. The generated
  source follows the Faust Julia shell shape, supports architecture wrapping,
  propagates `-double` to `const REAL = Float64`, emits Julia UI zones as
  symbols, and includes the cast/math helpers needed by representative impulse
  DSPs (`faust_wrap_int32`, `fmod`, `atan2`),
- many language families that were earlier missing are no longer front-end
  blockers:
  - `case` and recursive pattern-matching with computed numeric arguments,
    including xtended-function arguments such as `max`/`min` (full
    `patternSimplification` port)
  - float-literal patterns (`foo(1.0) = …`) now match correctly
  - lambda/closure forms (first-class `boxClosure` node)
  - modulation
  - local imports
  - structural import expansion from parsed `importFile(...)` nodes, including
    inline `environment { import(...) }` cases in the tracked parity scope
  - top-level imports now remain visible after nested transitive imports;
    duplicate-import suppression is scoped per structural definition expansion
  - metadata
- multi-output recursion groups (`SIGPROJ index > 0`) are now supported,
  enabling `freeverb`, `Birds.dsp`, and any `~ _`-based multi-output feedback,
- the algebraic `simplify` pass from `normalize/` is now wired into the
  `signal_prepare` pipeline, folding constant arithmetic before FIR lowering,
- computed `SIGGEN` table generators are now evaluated at compile time by
  `GeneratorInterpreter`, enabling `os.osc`, `os.saw`, and other
  oscillator-table patterns,
- Variable delays driven by a UI slider, numentry, **or bounded audio-rate
  expression** now compile end-to-end — that restriction has been significantly
  relaxed,
- the embedded `faustwasm` Rust compiler path can now compile DSP source
  strings that import the standard Faust libraries, because the compiler module
  ships a read-only embedded library bundle instead of depending on the legacy
  Emscripten filesystem,
- the same embedded compiler path now passes the historical `faust2wasm.js`
  polyphonic route on a validated case (`organ.dsp`), including packaged mixer
  fallback when no compiler `FS` is present,
- the full C++ interval algebra is now available in Rust through
  `crates/interval`, and the signal type lattice is fully modeled in
  `crates/sigtype` with correct parity including `FConst`/`FVar` intervals,
- signal-type purity is now enforced: `SIGWAVEFORM` correctly carries `Samp`
  variability (not `Konst`), and pure math functions (`sin`, `cos`, `sqrt`,
  etc.) preserve argument variability without spurious `samp_cast` promotion,
- the `instanceConstants` lifecycle contract is now enforced at FIR level
  (FIR-LC01 diagnostic) and the Cranelift backend compiles `instanceConstants`
  as a real JIT function, matching the C/C++ backend lifecycle model,
- the variability-driven FIR placement pass (Phase 1) is now signal-sharing
  aware, materializing only shared or boundary-crossing nodes into named
  variables and leaving single-use within-tier intermediates inline — matching
  the C++ code shape for Block-rate control chains,
- intra-bucket CSE (Phase 2) further deduplicates shared sub-expressions
  within each execution tier, reducing redundant evaluation in the sample loop,
- **forward-mode AD** (`fad(expr)`) now propagates correctly through the
  full signal graph including recursive groups, variable delays, transcendentals,
  arithmetic (`atan2`, `fmod`, `remainder`), casts, bargraphs, and control-flow —
  with 35 corpus entries validated through the `signal_pipeline` test suite and
  the fast-lane. The transform operates directly on De Bruijn recursive form,
  interleaving primal and tangent slots inside `DEBRUIJNREC` bodies before
  symbolic conversion in `signal_prepare`,
- **reverse-mode AD** (`rad(expr, seeds)`) landed as a feed-forward symbolic
  subset on 2026-04-27 and now includes the BRA temporal/recursive fallback:
  `rad(...)` produces `[primals…, ∂ sum(primals) / ∂ s_j…]` with an implicit
  all-ones cotangent. The feed-forward symbolic differentiable subset matches
  FAD outside temporal families; temporal and recursive families leave that
  local sweep and use `BlockReverseAD`. Phase E0 recursive-linearity
  classification and the E1 LTI transposition scaffold remain as dormant
  specialization infrastructure. The 2026-05-12 RAD fixes disable the
  incomplete reverse-time-recursion dispatcher fast path, cast integer BRA
  forward values into the real-valued tape-store boundary during FIR lowering,
  and fix shared FAD/RAD wrapper input slicing for cases such as
  `rad(*, (_,_,_))`. The 2026-05-17 local-rule factorization also makes the
  Signal symbolic RAD path and FIR/BRA path share one backend-neutral
  classification/formula table in `signals::ad_rules`, while preserving
  backend-local tape loading and reverse-loop scheduling. See
  [docs/rad-note-en.md](../docs/rad-note-en.md),
- **SVG block-diagram output** is now wired via `-svg` (landed 2026-05-02):
  the full `crates/draw/` module renders all box/composition/schema types,
  supports hierarchical folding (`-f`, `-fc`), and matches the C++ output
  directory convention,
- `expandDSP(...)` and `generateAuxFiles(...)` are now real implementations
  (landed 2026-05-03): both are wired into `crates/compiler/src/lib.rs` and
  exported from the Cranelift and interpreter C/C++ FFI headers,
- the Cranelift backend now uses saturating float-to-int casts (`fcvt_to_sint_sat`)
  to match C semantics on NaN and out-of-range floats (landed 2026-05-01),
- the normalize pipeline now runs a `rec_merge` pass that unifies isomorphic
  `SYMREC` groups before simplification, enabling the `x − x → 0` rule to
  fire across independently-built recursive groups.

## 6.2 Where C++ is still broader

Faust C++ still supports a broader end-to-end language/runtime envelope.

Most importantly, the C++ compiler still has:

- **specialized reverse-mode AD through time**: the feed-forward RAD subset and
  the generic `BlockReverseAD` fallback are implemented (see
  [docs/rad-note-en.md](../docs/rad-note-en.md)). What remains gated is the
  more specialized reverse-time recursion path: `delay` / `prefix` / recursion
  in the differentiated body leave the local symbolic sweep and use BRA today,
  while phase E1 LTI transposition and phase F BPTT specialization remain
  future phases (plan §20).
- fuller support for stream-wrapper lowering,
- broader mature transform/backend coverage on long-tail signal families,
- a mature Julia backend with broader impulse-test coverage and runtime
  packaging assumptions already exercised by the upstream Faust test suite,
- a fuller embedded-compiler helper surface for web tooling
  (`expandDSP` and `generateAuxFiles` are now real in Rust, but `getInfos`
  is only partially implemented and packaged FS semantics differ),
- remote HTTP(S) imports and architecture sources; the adapted native
  `sourcefetcher` contract and implementation phases are specified in
  [`sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md`](sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md),
- the historical production path beyond the active Rust fast-lane slice.

The variable-delay gap is now **substantially closed**:

- **C++**:
  - accepts `delay(x, d)` when the delay amount has a valid, bounded,
    non-negative interval; sizes the ring buffer from the interval upper bound
  - C++ `occMarkup` computes the max delay in a dedicated pre-pass
    (`incOcc`/`getMaxDelay`), with `checkDelayInterval` called there
  - `gCausality = true` in production (`compile_scal` calls
    `typeAnnotation(L2, true)`): invalid delay intervals are already caught
    during type annotation

- **Rust (current fast-lane status, 2026-04-12)**:
  - accepts constant integer delays (unchanged),
  - accepts variable delays whenever `check_delay_interval` succeeds:
    bounded non-negative interval, regardless of variability,
  - `scan_delay_lines` is the Rust equivalent of the C++ `occMarkup` pre-pass;
    `delay_size_for_amount` replaces `checkDelayInterval(getCertifiedSigType(y))`,
    driven by the same interval algebra,
  - `min_const_upper_bound` provides structural safety for any residual case
    where interval analysis yields an unbounded result despite a `SIGMIN(n, …)`
    ceiling in the expression.

The architectural difference (inline sizing vs. separate `occMarkup` pass) is
cosmetic: both derive the delay line size from the same interval algebra and
produce the same result.  The only currently unimplemented C++ feature in this
area is the `fOutDelayOcc` / `fCountDelay` optimisation that avoids allocating
a delay vector when a signal is only read at offset zero — a code-quality
difference, not a correctness gap.

### Key parity fix (2026-03-14): `FConst`/`FVar` interval

A subtle parity bug was identified and corrected: the C++ `interval()` default
constructor uses **member initializers**:

```cpp
// compiler/interval/interval_def.hh
double fLo{std::numeric_limits<double>::lowest()};  // f64::MIN
double fHi{std::numeric_limits<double>::max()};     // f64::MAX
int    fLSB{-24};
```

This means `interval()` in C++ = `[f64::MIN, f64::MAX]` (fully open),
not `[NaN, NaN]` (empty).  The Rust `inferFConstType` / `inferFVarType`
had incorrectly used `interval::empty()` after a wrong parity audit read it
as `(NaN, NaN)`.  With `Interval::new_default()` restored, expressions like
`ma.SR = min(192000, max(1, fSamplingFreq))` correctly produce a finite
interval `[1.0, 192000.0]`, which then propagates through delay arithmetic
to yield a bounded delay amount — exactly as in C++.

## 7. Historical Progression

The current shape of the subset follows a clear history.

### 7.1 February 17, 2026: fast-lane bootstrap and first executable FIR slices

The large `signalFIRCompiler` fast-lane rollout on 2026-02-17 established the
first real backend subset:

- Step 2A: inputs/constants/binops/output passthrough
- Step 2B: math + control/state bootstrap
- Step 2C: first stateful slice
- Step 2C.2: recursion support
- Step 2D..2H: breadth coverage, table lowering, non-trivial table coverage

At that point the backend subset was real, but still much narrower than the
front-end language.

### 7.2 February 28, 2026: parser parity became operational but not yet "closed"

The parser parity audit documented a strong but still not fully closed parser
state:

- production entrypoint wired,
- wide grammar coverage,
- good differential guardrails,
- but residual architecture and diagnostic-fidelity gaps remained.

This matters because, historically, the supported source subset was initially
limited by parser/eval work before backend lowering became the dominant gap.

### 7.3 March 7, 2026: parser / pattern / eval closure

By 2026-03-07, the parser/pattern/eval area was documented as having no known
remaining parity gap in its tracked scope.

This is the key point where the limiting factor stopped being "can Rust read
this Faust source?" and became "can the backend lower the resulting signal
graph end-to-end?"

### 7.4 March 9, 2026: signal preparation and fixed-delay fast-lane maturity

The 2026-03-09 work introduced the current pre-FIR preparation boundary:

- whole-forest staging after `de_bruijn_to_sym`,
- reduced simple typing,
- reduced signal promotion,
- typed delay/recursion/table handling,
- fixed-size circular delay lines for constant `SIGDELAY`.

This was also the point where the project explicitly chose to:

- support constant fixed delays first,
- reject variable delays explicitly,
- defer interval-driven variable-delay parity to later work.

### 7.5 March 10–12, 2026: remaining valid backend gap shrank to a very small tail

Subsequent work tightened integer typing, recursive state reuse, noise/sample
rate cases, UI pathname normalisation, relative group label rebasing,
class-name CLI parity, and other backend details.

By the 2026-03-12 snapshot:

- the front-end corpus mismatch count was `0` (`75` cases, `60` valid),
- the backend valid-case gap had shrunk to one tracked corpus case:
  `rep_18_stream_wrappers.dsp`.

### 7.6 March 13, 2026: interval algebra, signal type lattice, variable delay

Three interlinked architectural layers landed on 2026-03-13:

#### `crates/interval` — full C++ interval library ported

The complete interval arithmetic library from `compiler/interval/` was ported
to a native Rust crate:

- `Interval { lo: f64, hi: f64, lsb: i32 }` value type with IEEE-aware
  bounds arithmetic,
- all four arithmetic operators, comparisons, math functions
  (`sin`, `cos`, `exp`, `log`, `pow`, …), cast helpers, and UI constructors
  (`vslider`, `hslider`, `nentry`, `button`, `checkbox`, `bargraph`),
- `saturated_int_cast` for safe conversion to delay-line sizes,
- 62 unit tests all passing.

A precision overflow in `ipow_scalar` (`lsb * k` → `lsb.saturating_mul(k)`)
was also fixed during this session; the fix prevents a panic when a signal
with an extreme interval (e.g., audio input placeholder `[f64::MIN, f64::MAX]`)
is raised to a power.

#### `crates/sigtype` — full C++ signal type system ported

The complete `AudioType` class hierarchy from `compiler/signals/sigtype.hh/cpp`
and the inference rules from `sigtyperules.cpp` were ported to a new Rust
crate:

- `SigType` enum: `Simple(SimpleType) | Table(TableType) | Tuplet(TupletType)`,
  each carrying `fInterval : Interval` directly as a member (faithful to C++),
- lattice enums with bitwise-OR join semantics: `Nature`, `Variability`,
  `Computability`, `Vectorability`, `Boolean`,
- `TypeAnnotator`: bottom-up memoized inference with a fixed-point loop for
  recursive types (seeded with `make_maximal()` = Real/Samp/Exec),
- full merge/cast/check helpers (`union_types`, `int_cast`, `float_cast`,
  `check_delay_interval`, …),
- `PreparedSignals` extended with a dual `sig_types: HashMap<SigId, SigType>`
  map so FIR lowering can access full interval data without breaking existing
  callers,
- 32 unit tests all passing (27 initial + 5 added during parity audit).

#### Variable delay support via interval upper bound

With the interval contract in place, `SIGDELAY` now accepts UI-parameterized
amounts:

- `delay_size_for_amount(sig)` tries (in order): constant literal, interval
  upper bound from `sig_types`, structural `SIGMIN(n, _)` ceiling,
- the delay line is allocated to `next_power_of_two(hi + 1)`, matching C++,
- the runtime read index uses `(fIOTA - amount) & mask` where `amount` is the
  lowered amount signal evaluated each sample — identical to C++ circular
  buffer semantics.

This raised the end-to-end backend corpus score from `59 / 60` to `68 / 69`
valid cases, with the same single remaining gap (`rep_18_stream_wrappers.dsp`).

### 7.7 March 14, 2026: audio-rate delays, FConst parity fix, echo.dsp

Three further improvements landed on 2026-03-14:

#### Audio-rate delay amounts now accepted when bounded

The `variable_delay_max_bound` function previously rejected all
`Variability::Samp` delay amounts via a blanket guard.  That guard was a
workaround from before `TINPUT` carried the correct interval `[-1, 1]`.  With
`TINPUT` properly typed, audio-rate expressions such as `int(_+10)` produce
interval `[9, 11]` — bounded and non-negative — and are now accepted.

The acceptance criterion is now purely interval-based: any delay amount whose
type satisfies `check_delay_interval` (bounded, `hi ≥ 0`) compiles, regardless
of variability.  This is consistent with the C++ `checkDelayInterval` contract.

#### `FConst` / `FVar` interval parity fix

The 2026-03-13 sigtype parity audit had introduced a wrong fix: it changed
`FConst` and `FVar` intervals from `Interval::new_default()` to
`interval::empty()`, citing "C++: `interval()` = NaN, NaN".

Investigation of the actual C++ source (`interval_def.hh`) revealed that the
C++ `interval()` default constructor uses member initializers:

```cpp
double fLo{std::numeric_limits<double>::lowest()};  // NOT NaN
double fHi{std::numeric_limits<double>::max()};     // NOT NaN
```

`interval()` = `[f64::MIN, f64::MAX]` — the fully-open interval, identical to
Rust `Interval::new_default()`.  With `empty()` erroneously applied to
`fSamplingFreq`, the interval algebra propagated `empty()` through
`min(192000, max(1, fSamplingFreq))`, poisoning any delay amount derived from
`ma.SR` (e.g. `ef.echo1s`, `de.delay(n, hslider * ma.SR / 1000)`).

With `Interval::new_default()` restored, the correct interval chain is:

```
fSamplingFreq          ∈ [MIN, MAX]
max(1.0, …)            ∈ [1.0, MAX]
min(192000, …)         ∈ [1.0, 192000]   ← interval algebra clamps at 192000
hslider[0,1000] * …   ∈ [0, 192000]
int_cast(…) − 1        ∈ [−1, 191999]
max(0, …)              ∈ [0, 191999]
min(65536, …)          ∈ [0, 65536]      ← check_delay_interval returns 65536
delay line size        = next_pow2(65537) = 131072
```

This matches the C++ output for `echo.dsp` (`fRec0[131072]`, mask `131071`)
exactly.

#### Structural delay fallback (`min_const_upper_bound`)

A defence-in-depth fallback was added to `delay_size_for_amount`: if the
interval path yields no finite bound, but the delay amount expression is
structurally `SIGMIN(SigInt(n), _)` (as produced by `de.delay(n, d, x)`),
`n` is used as a conservative ceiling.  With correct `FConst` typing this path
is not reached for `ma.SR`-based patterns, but it guards against any future
case where interval analysis yields an unbounded or empty result.

#### Corpus additions

Three new corpus entries:

- `rep_67_variable_delay_shifted_slider`: slider shifted into `[-200, -100]`
  — must be rejected (hi < 0).
- `rep_68_variable_delay_audio_rate`: `process = @(int(_+10))` — audio-rate
  amount with bounded interval `[9, 11]`, accepted.
- `rep_69_variable_delay_sr_millisec`: `de.delay(65536, hslider * ma.SR / 1000)`
  — exercises the `ma.SR` interval chain, accepted.

End-to-end backend corpus: `71 / 72` valid cases, same single gap.

### 7.8 March 14, 2026 (continued): eval×normalize parity — constant folding at eval time

Six C++ call sites (`CS-1` through `CS-6`) that invoke `boxPropagateSig` +
`simplify` in the C++ evaluator (`eval.cpp`) were ported to Rust.  The work
connects `normalize::simplify_const` into `crates/eval` so that the Rust
evaluator folds constants at the same points as C++.

#### Background: C++ `boxPropagateSig` + `simplify` pattern

In C++ `eval.cpp`, several evaluation paths finish with:

```cpp
// e.g. in evalSeq, evalRoute, evalWidgetParams …
Tree sig = boxPropagateSig(sig_env, box, {});   // 0 inputs → extract scalar
return simplify(sig);                            // algebraic simplification
```

The Rust equivalent is `propagate_box_and_simplify(arena, box_id)`: builds a
flat box, propagates with 0 inputs, then calls `simplify_const` (which calls
`normalize::simplify` with an empty type map).

#### CS-1 / CS-7 — `BoxSeq` numeric folding

When both sides of a `BoxSeq` are fully numeric (integer/real literals or
parallel compositions thereof), `try_fold_seq_numeric` propagates and simplifies
the whole expression into a single scalar node.  This matches C++ `evalSeq`
which short-circuits to `boxPropagateSig` when `isNumericalTuple(e1)`.

Helper: `is_numerical_tuple_box` recursively checks that a box is a Par-spine
of Int/Real leaves, matching C++ `isNumericalTuple`.

#### CS-2 — Route arithmetic parameter normalization

`BoxMatch::Route` now has a dedicated arm in `eval_value` that evaluates all
three children (`ins`, `outs`, `routes`) with `eval_box` and then:

- converts `ins`/`outs` to a literal `boxInt(n)` via `eval_box_to_int_node`,
- normalises the route specification through `normalize_route_spec`:
  flattens the Par spine, reduces each leaf to a `boxInt`, and rebuilds a
  right-spine Par.

This matches C++ `evalRoute` which calls `normalizeRouteList` and
`boxPropagateSig` for `ins`/`outs`.  It fixes the
`PropagateError::InvalidIntegerValue` that occurred when `ins`/`outs` were
arithmetic expressions (e.g. `1+1`) rather than literal integers.

#### CS-4, CS-5, CS-6 — UI widget parameter reduction

`eval_slider_like`, `eval_vbargraph`, `eval_hbargraph`, and `eval_soundfile`
now reduce each numeric parameter (cur/min/max/step, chan) with
`simplify_slider_param`:

- evaluate the parameter expression with `eval_box`,
- if reducible to a floating-point constant, replace with `boxReal(x)`,
- otherwise keep as-is.

This matches C++ `evalSlider` / `evalBargraph` / `evalSoundfile` which call
`boxPropagateSig` on each parameter before embedding it in the UI node.

#### Box simplification family

A general memoised pass `box_simplification` was added:

- `numeric_box_simplification`: for any box that is not already a literal,
  tries `propagate_box_and_simplify`; if it reduces to a scalar Int/Real,
  rebuilds a `boxInt`/`boxReal`; otherwise recurses structurally via
  `inside_box_simplification`.
- `inside_box_simplification`: recursively simplifies all children, then
  rebuilds the same node kind.

This corresponds to C++ `simplifyToNormalForm` wired inside `propagateSignals`
at the box level.

#### `normalize::simplify_const` public API

`crates/normalize/src/simplify.rs` exposes `simplify_const(arena, sig)` as a
public entry-point: runs the full `simplify` rewriter with an empty type-map
cache, sufficient for compile-time constant folding without requiring a
`TypeAnnotator` pass.

#### Corpus addition

- `rep_70_route_arithmetic_params`: a `route(2, 2, 1+0, 1, 1+1, 2)` program
  that requires route spec normalisation to compile correctly.

#### Test coverage

26 unit tests were added to `crates/eval/src/lib.rs` in
`simplify_helpers_tests`, covering:
- `propagate_box_and_simplify` for constants and signal expressions,
- `is_numerical_tuple_box` for various box shapes,
- `try_fold_seq_numeric` for Int/Real Seq folding,
- `eval_box_to_f64` / `eval_box_to_i32` for literal extraction,
- `box_simplification` for compound boxes.

End-to-end backend corpus: `72 / 73` valid cases, same single gap.

### 7.9 March 15, 2026: SIGGEN interpreter, multi-output recursion, pattern-matcher parity

A cluster of significant features landed together:

#### Compile-time `GeneratorInterpreter` for SIGGEN

Previously, `expand_generator_values` could only handle constant/waveform
generators.  A computed generator such as the recursive phasor `+(1)~_`
inside `sin()` used by `os.osc` was rejected.  A full compile-time signal
interpreter (`GeneratorInterpreter`) was added: it evaluates 0-input
deterministic signal trees step-by-step at compile time, covering all
arithmetic operators, math functions, casts, `Delay1`, and symbolic
recursion.  `os.osc(440)`, `os.saw`, and similar oscillator patterns now
compile to correct 65536-entry sine tables.

#### Multi-output recursion groups

The FIR lowerer only handled single-output recursion (`SIGPROJ index 0`).
`freeverb.dsp`, `Birds.dsp`, and any DSP using the `~ _` feedback pattern
with 2-output groups failed.  `lower_proj`, `active_recursion_info`, and
supporting helpers were extended to handle all projection indices.

#### `boxClosure` first-class closure node

Closures were previously host-side only (`EvalValue::Closure`).  A new
`boxClosure` tree node carries the closure through the box evaluation chain,
replacing the earlier `boxSlot + substitute_tree` workaround.  This fixes
scoping for higher-order functions in `aanl.lib` and related patterns.

#### Pattern matcher: `match_num` simplification

`poly(max(1, min(6, 4)))` and similar computed numeric arguments previously
failed pattern matching because `max(1, min(6,4)) ≠ Int(4)` by tree identity.
`simplify_pattern` now reduces computed arguments to literals at match time,
matching C++ `simplifyPattern`.  A `boxPatternMatcher` side-table prevents
stack overflow in partially-applied case expressions.

#### `sigtype`: cons-list recursion bodies typed

The type annotator did not recurse into cons-list nodes, leaving all signals
inside `SYMREC` bodies without type information.  `delays.dsp` failed because
the `nentry("d1", 200, 0, 1000, 1)` interval was never propagated.  Fixed by
adding a cons-list → `TupletType` path in `infer_inner`.

#### `par(i, 0, X)` fixed

`iterate_par(n=0)` now returns the empty block (0→0) instead of `X`.

### 7.10 March 16, 2026: `simplify` wired into pipeline, waveform lowering overhaul

#### `signal_prepare`: algebraic simplification pass

`normalize::simplify` is now called inside `prepare_signals_for_fir`, between
full-type annotation and FIR lowering — matching the C++ pipeline order.  This
folds constant arithmetic (`max(0.25, 0.5)` → `Real(0.5)`, `IntCast(Real(1.5))`
→ `Int(1)`, `Delay1(x)` → `Delay(x, Int(1))`) at the signal level before any
backend sees the tree.  Both type maps are recomputed after simplification.

**Note**: the `simplify` pass addresses most constant-folding gaps, but does
not yet fold arithmetic in table-size positions (`BinOp(Mul, Int(8), Int(8))`
is still not reduced to `Int(64)` when used as a `SIGWRTBL` size argument).
This is documented in `porting/missing.md`.

#### Waveform lowering overhaul (three bugs fixed)

- **Cycling index**: bare `SIGWAVEFORM` outputs now advance with
  `(iWave + 1) % N` per sample; previously read `table[0]` every sample.
- **File-scope static tables**: waveform and rdtable-style tables are now
  emitted as `const static` arrays at file scope, matching the reference
  compiler; previously emitted as struct members with a fill loop.
- **Interval-aware bounds**: `SIGRDTBL` reads with a provably in-bounds index
  no longer emit redundant clamping — direct array access, matching C++.

### 7.11 March 17, 2026: delay-state robustness, select2 fix, CLI parity

#### Circular carriers for delay1/recursion (later refined)

The fast-lane moved away from scalar state + ad hoc deferred update ordering
for `delay1` and recursion, which had caused ordering bugs when depth-first
lowering pushed inner updates before outer ones.

That March change established the circular/state-array direction, but the final
model was refined later:

- small standalone `Delay1` now uses the Faust C++-style `Shift` strategy when
  appropriate,
- recursion carriers can be upsized from accumulated delay analysis and reused
  by `Delay1^k(Proj(...))` chains,
- simple 2-slot recursions were later realigned with the exact C++ write/read
  pattern instead of always going through `fIOTA`.

#### `select2` branch inversion fixed

`select2(sel, x, y)` was emitting `sel ? x : y` instead of `sel ? y : x`.
Fixed by swapping the destructuring of `(then, else)` arms.

#### Two-pass delay-line scan

`prepare_delay_lines` now does a collect pass (max per carried signal) then an
allocate pass, preventing the sizing-mismatch error for signals carried at
multiple different delay amounts (e.g. `frenchBell.dsp` modal synthesis).

#### CLI: `-double`, default class name `mydsp`, wrapping integer literals

- `-double` legacy flag recognized,
- default generated class/struct name is now `mydsp` (matching C++),
- integer literals that overflow 32 bits now use wrapping arithmetic (matching
  C++ `str2int` behavior).

### 7.12 March 18–21, 2026: eval correctness, signal-prepare canonicalization, physical models

#### Unary recursion projection canonicalization (`signal_prepare`)

After `de_bruijn_to_sym`, some symbolic recursion groups have physical arity 1
but are referenced via non-zero logical projection indices.  A canonicalization
step in `prepare_signals_for_fir` rewrites these to `proj(0, group)`, enabling
`zita_rev1_stereo`, `Birds.dsp`, and similar DSPs to compile.

#### Eval correctness fixes

- `a2sb` memoization boundaries tightened: residual sharing preserved.
- Residual closure arity computed correctly in non-closure application paths.
- Exact integer reals (e.g. `Real(2.0)`) canonicalized to `Int(2)` for
  case-matching parity with C++.
- Route-spec normalization aligned with C++ for computed `ins`/`outs`
  arguments.

#### Physical model fixes (2026-03-21)

- `lower_proj` spurious store eliminated: when a body signal was already
  scheduled as a state update by `lower_delay_state`, `lower_proj` no longer
  emits a second overwriting store.  Fixes `clarinetModel` and similar
  waveguide DSPs outputting all zeros.
- `GeneratorInterpreter::eval_delay1` now correctly advances recursive state
  on each step.
- `slot_env` de Bruijn lifting in `FlatNodeKind::Rec` corrected, fixing
  incorrect sample output for `spectralCentroid` and similar recursive analyzers.
- `eval` `FIR` shift binops added to backends.

### 7.13 March 22–24, 2026: correctness fixes, pattern evaluation parity, cranelift robustness

#### `lower_proj`: separate rec-array and state-slot maps (2026-03-22)

Nested recursion groups (e.g. `tf22` biquad, `filters_direct_ladder_tests.dsp`)
produced wrong audio because `ensure_recursion_array` and `ensure_state_slot`
shared the same `state_name_by_node` map.  When the outer group body was itself
the `Delay1` node, both paths resolved to the same array, causing the wrong
value to be written.

Fix: add a separate `rec_array_by_group_index: HashMap<(u32,usize), RecArrayInfo>`
keyed by `(group_id, output_index)` that never aliases `state_name_by_node`.
`lower_proj` now calls `ensure_recursion_array_for_group(group, i, …)` and the
skip-guard from the previous commit was removed (it is no longer needed).

#### Parser: `stdfaust.lib` + `demos.lib` duplicate zero-arity aliases (2026-03-22)

Both libraries independently declare the same 19 library-alias symbols (`ma`,
`ba`, `de`, `si`, …).  `make_definition_from_variants` previously errored with
"multiple definitions of a zero-argument symbol are not allowed".  Fix: mirror
C++ import-shadowing semantics — for zero-arity symbols with multiple variants,
use the newest definition.  `guitarEffectChain.dsp` now parses and compiles.

The `import_loading_cpp_gap` test was converted from a gap-tracker to a
parity-success test (2026-03-23).

#### Interval: `hi_or2` mask-rule off-by-one → exponential recursion (2026-03-23)

The short-circuit in `hi_or2` checked `a.hi == 2 * m.wrapping_sub(1)` (= 2m−2),
but the correct condition is `a.hi == 2m − 1` (all bits set below the MSB).
The off-by-one prevented early exit for full power-of-2 ranges `[0, 2^n−1]`,
causing 3 recursive sub-calls per level and O(3^32) ≈ 10^15 calls for any DSP
with bitwise-AND signals.  Fix: `ma.wrapping_add(ma).wrapping_sub(1)`.

`guitarEffectChain.dsp`: was hanging / 9.7 s → 2.0 s.
`minimoog-novation.dsp`: type-annotation time 4.7 s (C++ reference: 8.7 s).

#### Eval: integer div folding — `4/2` must produce `Int(2)` (2026-03-23)

`try_fold_seq_numeric` always returned a `Real` for division results.
Patterns from `math.lib` (`selector`, `butterfly`, `hadamard`) use tree identity
where `Real(2.0) != Int(2)`.  Fix: when all inputs are `SigInt` and the result
is an exact integer, produce `SigInt`.  Fixes `zita_rev1.dsp` sequential
composition mismatch.

#### Cranelift: float BinOp comparison type mismatch (2026-03-22)

`BinOp { op: Lt/Le/Gt/Ge/Eq/Ne, lhs: Float32, rhs: Float32, typ: Int32 }` is a
float comparison whose integer result was previously lowered via `fcvt_to_sint`,
truncating the operands instead of performing a float compare.  Fix: detect
comparison ops where the lowered CLIF operand type is float and emit `float_cmp`
regardless of the result type.  `harpeautomation.dsp` (string trigger using
`fabs(x) < 0.5`) now produces correct non-zero output.

#### Cranelift: AArch64 branch-offset overflow → graceful fallback (2026-03-22)

Cranelift's `MachBuffer` can panic (internal assertion) on AArch64 when a
very large `compute` body exceeds the ±1 MiB B.cond displacement limit. It can
also finalize an oversized monolithic body that later branches into
padding/data islands and traps with `EXC_BAD_INSTRUCTION`. Fix: wrap JIT
compilation in `catch_unwind`; on panic, retry with `force_stub = true`. On
AArch64, also reject lowered `compute` bodies whose textual CLIF exceeds the
current conservative size guard and regenerate the existing no-op stub.
`minimoog-novation.dsp` and `fad_tracking3.dsp` now compile without crashing
the process. Oversized guarded cases report `compute_body_lowered=false`.

#### FBC serial: embedded newlines in quoted UI/meta labels (2026-03-22)

`elecGuitarMIDI.fbc` contains a label with a literal `\n` byte inside a
quoted string.  `read_ui_block` called `read_line` once per instruction,
stopping at the embedded newline.  Fix: `read_quoted_logical_line` accumulates
physical lines until all double-quote characters are balanced.

#### Eval: float literal patterns match call-site arguments (2026-03-24)

`simplify_pattern` was coercing integer-valued Real constants (e.g. `1.0`,
`4.0/2.0`) to `Int` before the tree-identity check against the automaton's
`Constant(float_bits(…))` transition.  Since `int(1) != float_bits(1.0)`, any
function defined with a float-literal pattern always raised "no case rule
matches".  Fix: align with C++ `simplifyPattern` — return `boxReal` literals
unchanged without promoting integer-valued reals to `Int`.

Corpus addition: `rep_72_float_literal_pattern.dsp`.

#### Normalize: `min`/`max(Int, Int)` folds to `SigInt` (2026-03-24)

`simplify_const` previously folded `min`/`max` to `SigReal` even when both
operands were `SigInt`.  C++ parity: `sigMin`/`sigMax` return `sigInt` when
both inputs are integers.  Without this fix, `poly(max(1,min(N,4)), x)`
pattern matching failed — the argument simplified to `SigReal(2.0)` while
the pattern stored `Constant(boxInt(2))`.

#### Eval: `patternSimplification` — full C++ `isBoxNumeric` semantics (2026-03-24)

The previous `pattern_simplification` (literal arithmetic only) was replaced
with a complete port of C++ `patternSimplification` from `eval.cpp` line 773:

1. Try to fold the whole expression via `simplify_pattern` (full signal
   propagation + `simplify()` — equivalent to C++ `isBoxNumeric`).
2. If that fails, recurse into `PatternOp` children (`Par/Seq/Split/Merge/Rec`
   only — matches C++ `isBoxPatternOp`).  `HGroup/VGroup/TGroup/Route` are no
   longer recursed into, matching C++ exactly.
3. Otherwise return the pattern unchanged.

This enables `patternSimplification` to fold xtended functions such as
`max`/`min` at automaton construction time.  Example:
`f(max(1, min(6, 4)))` now correctly matches `f(4) = 40`.

~185 lines of superseded dead code removed (`simplify_numeric_pattern`,
`eval_numeric_pattern_value`, `eval_numeric_binary_op`, `NumericValue`, and
supporting helpers).

Corpus addition: `rep_73_pattern_max_min_fold.dsp`.

#### Structural refactors (2026-03-22–24, no semantic change)

- Dead-code sweep: compatibility wrappers, unused predicates, orphaned
  utilities removed from `crates/eval`, `crates/boxes`, `crates/compiler`.
- `boxes` and `eval` crates split into focused submodules.
- All embedded test suites extracted into standalone `tests.rs` files
  (`signal_fir`, `signal_prepare`, `fir`, `interp`, `cranelift` backends).

#### Corpus additions (net)

Ten new fixtures added since March 21:

| fixture | description |
|---------|-------------|
| `rep_60_counter_rem` | integer counter with `%` (rem) operator |
| `rep_61_fmin_sr` | `fmin` driven by sampling-rate constant |
| `rep_62_select2_trigger` | `select2`-based trigger |
| `rep_63_rwtable` | read/write table |
| `rep_63_store_load_table` | store-then-load table pattern |
| `rep_64_dynamic_rem` | dynamic `%` with variable operands |
| `rep_65_fabs_trigger` | `fabs(x) < 0.5` string trigger (Cranelift correctness) |
| `rep_71_degenerate_unary_recursion` | docs corpus: degenerate proj(7,W) canonicalization |
| `rep_72_float_literal_pattern` | float-literal pattern matching parity |
| `rep_73_pattern_max_min_fold` | `patternSimplification` with `max`/`min` fold |

End-to-end backend corpus: `82 / 83` valid cases, same single gap
(`rep_18_stream_wrappers.dsp`).

### 7.14 April 3, 2026: simplify integration hardening and latent fast-lane fixes

The 2026-04-03 fixes did not radically widen the declared subset, but they did
make the existing fast-lane subset materially more trustworthy on real DSPs and
web/runtime paths.

The main changes were:

- `simplify` is now a real part of the FIR-preparation pipeline, followed by a
  second `type -> promote -> type` cycle so FIR lowering still sees explicit
  coercions and reduced simple types
- one-sample delays are canonicalized back to `Delay1` at the preparation
  boundary instead of being special-cased only inside one consumer
- normalization now preserves `fSamplingFreq`-bearing konst factors and no
  longer assumes every order-0 bucket is purely numeric
- the wasm fast-lane now resets UI-only controls correctly even when they have
  been optimized out of the executable signal graph
- FBC text serialization now uses round-trip float formatting, which removed a
  latent divergence between interpreter and Cranelift runtimes
- the SIGGEN interpreter used for compile-time table generation now handles
  non-recursive `Delay1` with proper previous-step semantics, fixing
  `table.dsp`

Practically, this means the already-declared subset around:

- oscillator/table generators,
- SR-driven delay expressions,
- wasm standalone wrappers,
- interpreter/Cranelift differential tests,

is now better supported in practice than the older March backend report alone
would suggest.

### 7.15 April 4–5, 2026: FIR runtime optimizations, signal-type correctness, and CSE

#### Cranelift: JIT-compile `instanceConstants` (2026-04-04)

The Cranelift backend previously replayed `instanceConstants` through a
Rust-side subset interpreter.  The backend now compiles `instanceConstants`
as a real JIT function; the `cranelift_dsp` runtime invokes that entry
directly during instance initialization, aligning the lifecycle model with
the C/C++ backends and covering the full FIR subset that Cranelift already
knows how to lower.

#### sigtype: preserve kSamp floor through recursive fixed-point (2026-04-04)

`update_rec_types` in `crates/sigtype/src/rules.rs` was replacing the full
approximation type for each recursive-group component with the freshly
inferred body type on every fixed-point iteration, merging only the
interval.  Variability, computability, and other lattice dimensions were
discarded.  As a result, a recursion body that ignores its feedback input
(e.g. `! : 1`) could converge to `Konst`, allowing the variability
placement pass to hoist projections into `instanceConstants` and break the
sample-by-sample feedback contract.

Fix: after inferring the fresh body component, promote its variability and
computability by the old approximation's values before merging the interval,
matching C++ `joinType` semantics.  The `depends_on_recursive_projection`
workaround in `signal_fir/module.rs` was removed; `is_recursive_projection`
is retained as a belt-and-suspenders guard.

#### FIR-LC01 lifecycle-order checker (2026-04-04)

`phasor.dsp` produced wrong output (waveform index counters hoisted into
`instanceConstants` before `instanceClear` could initialize them).  The FIR
verifier had no check for this ordering violation.

Three additions to `crates/fir/`:

- `FIR-LC01 | E` diagnostic: detects `LoadVar(kStruct, name)` inside
  `instanceConstants` for fields that are only stored by `instanceClear`.
- `check_lifecycle_order` method: locates both bodies, computes the
  `cleared_only` field set, and emits an `Error`-severity diagnostic per
  violation.
- Two regression tests cover the failing and passing patterns.

The diagnostic is `Severity::Error` (not Warning) so it fails compilation
without `--fir-verify-strict`.

#### fix(sigtype): SIGWAVEFORM must carry Samp variability (2026-04-05)

`SIGWAVEFORM` was typed with `Variability::Konst` via `make_table_type`,
which correctly describes the static table content.  However, in the Rust
signal graph after propagation, `SIGWAVEFORM` represents the cycling output
— each sample yields the next element.  With `Konst` variability, FIR-LC01
detected lifecycle-order violations for waveform-driven DSPs (`phasor.dsp`,
`table1.dsp`).

Fix in `crates/sigtype/src/rules.rs`: promote the `SigMatch::Waveform`
result to `Variability::Samp` in both the empty and non-empty cases, so all
dependent expressions inherit the correct floor via `max(Samp, ·) = Samp`.

#### fix(transform): skip Phase 1 hoisting for SIGWRTBL (2026-04-05)

`SIGWRTBL` inherits `Konst` variability from `make_table_type`, but
`lower_wrtbl` returns the write signal's value, which may reference `iWave*`
cycling counters.  A targeted guard added to the Phase 1 placement check
ensures `SIGWRTBL` nodes always remain inline (in the sample loop), bypassing
the variability-based hoist.  A principled sigtype fix (propagating the write
signal's variability into the result type) is deferred.

#### fix(sigtype): remove spurious samp_cast from pure math type inference (2026-04-05)

`infer_unary_math` applied `samp_cast(tx)` unconditionally, promoting the
result variability to `Samp` for all transcendental/math operations
(`sin`, `cos`, `tan`, `sqrt`, `exp`, `log`, `floor`, `ceil`, `rint`, etc.)
regardless of the argument's variability.  The same bug was present in binary
math ops (`atan2`, `fmod`, `remainder`).

The C++ Faust source uses `floatCast(t)` — **without** `sampCast` — for all
pure math primitives: variability is inherited from the argument intact; only
the nature is promoted to Real.

Fix: replace `samp_cast(tx)` with plain `tx` in `infer_unary_math` and remove
`samp_cast` from the binary math arms.  This enables constant-at-rate
expressions such as `sin(ma.SR)` to be correctly hoisted into
`instanceConstants`.

#### feat(transform): CSE materialization pass — Phase 2 (2026-04-05)

Phase 1 variability-driven placement materialized **every** non-trivial
Block/Konst signal node into a named variable, regardless of sharing.  For
`STunedBar.dsp` this created 285 `fSlow` variables where C++ Faust only
needed 43.

Two complementary improvements landed:

1. **Signal-sharing-aware Phase 1 placement** (`analyze_signal_sharing`):
   a pre-lowering DAG pass computes per-signal reference counts and
   variability-boundary flags.  Phase 1 now only materializes a node into a
   named variable when it is shared (`ref_count ≥ 2`) **or** sits at a
   variability-transition boundary.  Single-use within-tier intermediate
   nodes stay inline, producing compact compound expressions matching C++.
   `STunedBar.dsp`: `float fSlow` reduced from 285 to 43 (exact C++ match).

2. **Intra-bucket CSE materialization pass** (`crates/transform/src/signal_fir/cse.rs`):
   after variability-driven placement, a bottom-up rewrite detects
   multi-referenced non-trivial `FirId` value nodes within each execution
   bucket and materializes them into named temporaries (`DeclareVar` +
   `LoadVar`).  Declarations are inserted at the point of first use to
   preserve sequential data dependencies.  Trivial nodes (literals, variable
   loads) are never materialized.  Naming conventions:
   `fConst*` / `fSlow*` / `fTemp*` for constants, control, and sample
   buckets respectively.

All corpus tests continue to pass after both optimizations.

### 7.16 April 8, 2026: delay-strategy parity, recursion-delay analysis, explicit emission phases

The fast-lane delay subsystem was substantially tightened to match the C++
compiler more closely.

#### Delay strategy parity with Faust C++

`SIGDELAY` now follows the same practical three-way strategy split as the C++
compiler:

- `Shift` for very small delays,
- circular power-of-two buffers with `fIOTA` for the middle range,
- exact-size if-wrapping buffers for the `-dlt` range.

The threshold behavior at the exact `-mcd` and `-dlt` boundaries was also
aligned with the C++ compiler's strict comparisons.

#### Recursion-delay carrier reuse

The delay pre-pass now performs accumulated delay analysis for recursion
outputs, allowing chains of the form:

- `Delay1(Proj(...))`
- `Delay1(Delay1(Proj(...)))`
- `Delay(Delay1^k(Proj(...)), N)`

to reuse one canonical recursion carrier when possible instead of allocating
separate standalone delay vectors. This materially improves parity on APF /
biquad-like structures.

#### Simple recursion parity restored

The special case `process = + ~ _;` is now lowered with the same 2-slot update
shape as Faust C++:

- current value written to slot 0,
- previous value read from slot 1,
- deferred copy from slot 0 to slot 1 after output observation.

#### Explicit sample-loop phases

The sample-loop assembly now uses an explicit phase model:

- `Immediate`
- `PostOutput`
- `SampleEnd`

This does not change the supported source subset by itself, but it makes the
ordering guarantees around `Shift` copies, recursion updates, and delay counter
maintenance clearer and easier to maintain.

### 7.17 April 15-16, 2026: forward-mode AD (`fad(exp, x)`) — explicit-seed propagation and current support boundary

#### `forward_ad.rs` — complete FAD rule table implemented

The full forward-mode automatic differentiation pass was implemented and
documented in `crates/propagate/src/forward_ad.rs`.

The implementation differentiates each propagated signal with respect to one
explicit seed signal at a time, producing a *dual signal* `(primal, tangent)`.
A [`ForwardADTransform`](forward_ad) instance memoizes results across the shared
signal DAG to prevent exponential blow-up.

Differentiation rules implemented:

| Category | Covered nodes |
|----------|--------------|
| Constants / audio inputs | `int`, `real`, `sigInput` → tangent 0 unless the node is the selected seed |
| UI controls | `hslider`, `vslider`, `numentry` — tangent 0 unless the propagated signal equals the selected seed |
| Discrete UI | `button`, `checkbox` → tangent 0 |
| Arithmetic | `Add`, `Sub`, `Mul` (product rule), `Div` (quotient rule), `Rem` |
| Integer/bitwise | shifts, comparisons, bitwise ops → tangent 0 |
| Unary transcendentals | `sin`, `cos`, `tan`, `exp`, `log`, `log10`, `sqrt`, `abs`, `acos`, `asin`, `atan` |
| Binary math | `pow` (general power rule), `min`, `max` (sub-gradient via `select2`) |
| Casts | `float_cast` (linear), `int_cast` → 0, `bit_cast` → 0 |
| Unit delay | `delay1(x)` → `delay1(x')` |
| Variable delay | discrete Leibniz rule: `delay(x', d) − d' · delay(x − delay1(x), d)` |
| Control-flow | `select2`, `prefix` — transparent |
| Recursion | `DEBRUIJNREC`/`DEBRUIJNREF` — interleaved primal/tangent slot expansion (see §7.17) |
| Helper nodes | `attach`, `enable`, `control` — left-operand tangent forwarded |
| Bargraphs | tangent 0 |
| Fallback | all other nodes → tangent 0, primal unchanged |

Differentiation is performed **before** `de_bruijn_to_sym`: FAD operates
directly on De Bruijn recursive form (`DEBRUIJNREC`/`DEBRUIJNREF`), and
`de_bruijn_to_sym` runs afterwards inside `signal_prepare`.  Keeping FAD on the
De Bruijn representation avoids the "phantom recursion slot" bug that arises when
the same `DEBRUIJNREC` node is given different symbolic names in the primal and
tangent lanes.

#### Output expansion and the `Rec` suppress/expand protocol

`generate_fad_signals` differentiates against one explicit seed and assembles
the output bundle:
```
[primal₀, tangent₀, primal₁, tangent₁, …]
```

For programs containing `fad(exp, x)` inside a recursive group (`~`), a
two-phase protocol prevents dangling references inside the De Bruijn group:

1. **Suppress** — the `suppress_fad` flag makes `ForwardAD` transparent during
   branch propagation; `box_arity_wiring` is used for internal port arithmetic.
2. **Expand** — `generate_fad_signals` is called once on the completed primal
   `DEBRUIJNREC` output, producing the interleaved dual group from the
   fully-formed body.

#### Explicit-seed semantics and current limit

- `fad(exp, x)` computes one directional derivative with respect to the single
  propagated seed signal produced by `x`.
- The seed can be a control, an audio input, or a derived one-output signal.
- The transform does **not** enumerate all reachable controls anymore, and
  `[autodiff:false]` is no longer consulted by the compiler.
- A narrower unsupported case remains: if the seed is reached only through a
  recursive alias that the evaluator must expand while the recursive box is
  still being formed, evaluation still loops. This is the family illustrated by
  `state = next ~ _; prev = state; grad = fad(loss(prev), prev);`.
- This limit is not currently a Rust-only regression: upstream C++ Faust also
  rejects the same recursive-alias motif with an endless evaluation cycle.

#### Corpus coverage validated

| fixture | description |
|---------|-------------|
| `fad_basic` | `fad(f : sin, f)` — one control, one explicit seed |
| `fad_product` | `fad(f * g, f)` — product rule with one selected seed |
| `fad_trig_composition` | nested trig chain with explicit seed |
| `fad_triple_chain` | long chain differentiated against one chosen factor |
| `fad_delay` | fixed delay with explicit seed |
| `fad_recursive` | feedback loop differentiated with respect to one control |
| `fad_recursive_branch` | `fad` inside one recursive branch |
| `fad_recursive_deep_right` | deeply nested recursive branch with explicit seed |
| `fad_recursive_delay` | recursive delay with explicit seed |
| `fad_gradient_host` | host-side extraction of primal/tangent pair |
| `rad_basic` | `rad(expr, seeds)` feed-forward unary case |
| `rad_product_multi_seed` | binary product with two independent seeds |
| `rad_trig_composition` | trig chain differentiated against two seeds |
| `rad_absent_seed` | unreachable seed must yield zero gradient |
| `rad_repeated_seed` | repeated seed lanes must alias the same gradient |
| `rad_multi_output_sum_cotangent` | implicit all-ones cotangent on multi-output bodies |
| `rad_vs_fad_*_ffun` | runtime RAD/FAD parity for the supported unary FFun families (`tanh`, `sinh`, `cosh`, `atanh`, `asinh`, `acosh`) |
| `rad_rdtbl_index_basic` | read-only table index slope contract |
| `fad_rad_quadratic` | mixed AD: outer FAD over inner RAD, second derivative `f''(x) = 2` |
| `rad_fad_quadratic` | mixed AD: outer RAD over inner FAD, sum-cotangent gradient `f' + f''` |
| `fad_rad_trig_second_derivative` | mixed AD on `sin(x*x)`, fourth lane is the second derivative |
| `rad_fad_multi_seed` | mixed AD with two seeds, implicit all-ones cotangent through inner FAD lanes |
| `err_rad_zero_body`, `err_rad_zero_seed`, `rad_delay1_block_fallback`, `err_fad_rad_temporal` | RAD diagnostics and fallback surface (incl. RAD-in-FAD wrapping) |

These fixtures pass through `crates/compiler/tests/signal_pipeline.rs`.
`fad_delay` is additionally validated end-to-end through the
`signal_fir_lane` fast-lane test (C++ and interpreter bytecode output
checked). RAD-specific runtime parity (RAD vs FAD, RAD vs central finite
difference) is exercised by `crates/compiler/tests/rad_runtime.rs`.

#### `rad(expr, seeds)` is now supported on the feed-forward subset

Phase 1 reverse-mode AD landed in 2026-04-27. See the dedicated section
earlier in this document for the full contract; in short:

- output bundle layout `[primals…, ∂ sum(primals) / ∂ s_j…]` with an
  implicit all-ones cotangent on every primal output,
- same differentiable subset as FAD outside the temporal/recursive
  boundary (delay, prefix, recursion) and outside mutable-table /
  soundfile / non-unary FFun families,
- structured `RadBodyArity` / `RadSeedArity` / `RadUnsupportedNode`
  diagnostics: phase 1 RAD never silently emits a misleading gradient.
- stateful RAD phase E0 adds a read-only `DEBRUIJNREC` classifier for
  future transpose/BPTT gates, plus a `RecRadMode` strategy mapping; it
  does not widen accepted `rad(...)` programs.

Detailed design notes: [docs/rad-note-en.md](../docs/rad-note-en.md).

### 7.18 April 21, 2026: recursion depth limits, 64 MiB compiler stack, FAD De Bruijn overhaul

#### Recursion depth limits raised

Two separate depth caps were raised to prevent `RecursionDepthExceeded` on
complex programs such as `auto_spat.dsp`:

- **General `max_depth`** in `LoopDetector::new()` / `with_cancel()`:
  raised from `1024` to `4096`.  This guards symbol-lookup loops.
- **`STRUCTURAL_MAX`** inside `LoopDetector::enter_structural()`:
  raised from `256` to `4096`.  This guards `a2sb`/`a2sb_value` structural
  lowering paths that create fresh `boxSlot` nodes on every iteration and
  therefore cannot use identity-based cycle detection.

#### Compiler thread spawned with 64 MiB stack

Raising `STRUCTURAL_MAX` from 256 to 4096 exposes a secondary problem: each
structural lowering hop also places several Rust frames on the OS stack
(`eval_value`, `apply_value_list_value`, etc.), consuming roughly an order of
magnitude more bytes per iteration than a symbol-lookup iteration.  The default
OS thread stack (usually 8 MiB on macOS/Linux) overflows before the 4096 cap is
reached on programs with long combinator chains.

Fix in `crates/compiler/src/main.rs`: the `main()` function now spawns the
actual compiler work (`run_main`) on a dedicated thread with an explicit 64 MiB
stack:

```rust
fn main() {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(run_main)
        .expect("failed to spawn compiler thread")
        .join()
        .expect("compiler thread panicked");
}
```

This is a standard pattern for CLI tools with deep recursive computation; it
does not affect library users (they can set stack size on their own threads).

#### FAD: differentiation on De Bruijn form — `DEBRUIJNREC`/`DEBRUIJNREF`

The FAD implementation was redesigned to operate directly on De Bruijn
recursive form rather than converting to symbolic form first.

**Why De Bruijn form?**

`propagate_in_slot_env` emits signal trees that use raw De Bruijn nodes
(`DEBRUIJNREC`/`DEBRUIJNREF`) for recursive groups.  `de_bruijn_to_sym`
converts these to symbolic form (`SYMREC`/`SYMREF`) as part of
`signal_prepare`.  If FAD were applied *after* `de_bruijn_to_sym`, the same
physical `DEBRUIJNREC` node could acquire two different symbolic names in the
primal and tangent lanes — a "phantom recursion slot" bug.  Performing FAD on
the raw De Bruijn form avoids the ambiguity; each recursive group is a single
physical node whose identity is unambiguous.

**Interleaving scheme for `DEBRUIJNREC`**

When the transformer encounters `DEBRUIJNREC([e₀, e₁, …])`, it:

1. Increments an internal `debruijn_depth` counter.
2. Transforms each body expression `eᵢ` into a dual `(primalᵢ, tangentᵢ)`.
3. Builds an expanded body `[primal₀, tangent₀, primal₁, tangent₁, …]`.
4. Wraps the expanded body in a new `DEBRUIJNREC` node.
5. Decrements `debruijn_depth`.

The result has twice as many slots: primal at even indices `2i`, tangent at
odd indices `2i+1`.

**`DEBRUIJNREF` classification — `GroupKind` enum**

When a `SigProj(index, group)` node is encountered, the group expression may be:

- **`BoundRec`** — the group is a `DEBRUIJNREC` (a newly transformed recursive
  group), or a `DEBRUIJNREF` whose De Bruijn level is ≤ `debruijn_depth`
  (a reference to an FAD-scoped recursive group).  In this case the index is
  doubled: primal at `proj(2i, dual_group.primal)`, tangent at
  `proj(2i+1, dual_group.tangent)`.
- **`UnboundRef`** — a `DEBRUIJNREF` with level > `debruijn_depth` (a reference
  to a recursive group that is outside the FAD scope).  The primal index is kept
  unchanged and the tangent is zero.
- **`Other`** — any other node.  Both primal and tangent use the original index.

This three-way classification is implemented via a local `enum GroupKind` inside
the `Proj` arm of `transform_uncached`, keeping the match arm self-contained.

#### Removal of dead SYMREC/SYMREF code

The previous FAD implementation contained defensive guard blocks for
`SYMREC`/`SYMREF` symbolic nodes and a `fad_var()` helper function.  These
paths were never exercised in practice (FAD always sees De Bruijn form before
symbolic conversion) and were removed:

- `match_sym_rec`, `match_sym_ref`, `sym_rec`, `sym_ref` imports removed.
- The `SYMREC`/`SYMREF` guard blocks removed from `transform_uncached`.
- The `fad_var()` helper removed.

Two new tests in `crates/propagate/tests/core_api.rs` cover the De Bruijn path:

- `propagate_forward_ad_on_recursive_circuit_expands_outputs`: asserts that
  a `(k * _) ~ _` circuit differentiated with respect to `k` produces 2 output
  signals (one primal, one tangent).
- `propagate_forward_ad_on_recursive_circuit_has_interleaved_debruijn_structure`:
  asserts that the tangent output is `proj(1, fad_rec)` where `fad_rec` is a
  `DEBRUIJNREC` with an expanded body of 2 slots.

#### New FAD rules: `atan2`, `fmod`, `remainder`, `HBargraph`

Four previously uncovered signal families now have explicit differentiation rules:

| Rule | Formula |
|------|---------|
| `atan2(y, x)` | `d/dp = (x·y' − y·x') / (x² + y²)` |
| `fmod(x, y)` | `d/dp = x' − y'·⌊x/y⌋` |
| `remainder(x, y)` | `d/dp = x' − y'·round(x/y)` |
| `HBargraph(label, inner)` | merged into the `VBargraph` arm — tangent 0, primal unchanged |

These rules complete the coverage of all binary math primitives for which
Faust C++ defines a signal node.

### 7.19 April 24–25, 2026: eval stack-overflow fix, Infinity/NaN codegen, FAD white paper, eval perf

#### Eval: recursive case divergence reports clean error (2026-04-24)

A recursive Faust program (e.g. `fact(0) = 1; fact(n) = n * fact(n-1);
process = par(i, 3, fact(i));`) that diverges during evaluation previously
aborted the host thread with a native stack overflow. The fix extends the
evaluator's structural recursion guard to cover recursive
application/case-dispatch paths (via `enter_structural` / `leave_structural`
on `apply_value_list_value` and `apply_pattern_matcher_value`). The displayed
error now mirrors C++: `stack overflow in eval (depth budget N)`.

#### Codegen: valid C/C++ infinity and NaN literals (2026-04-24)

`ma.MIN * 1e307` overflows to `+∞` in single precision. The Rust C and C++
backends previously emitted invalid literals such as `inf.0f` because the
float formatter appended `.0`/`f` unconditionally. Both backends now emit
`INFINITY`, `-INFINITY`, or `NAN` for non-finite values, matching the
C++ reference compiler behavior.

#### FAD white paper (2026-04-24)

`docs/fad-note-en.md` (plus PDF) — standalone technical note on forward AD in
`faust-rs`, covering semantics, compiler architecture, recursion handling,
supported boundaries, and practical DSP usage patterns including
Newton/ZDF-style implicit solving, grey-box system identification, and
host-driven adaptive DSP.

#### Eval performance: `Rc<RefCell<EnvStore>>` + binding index (2026-04-25)

- Replaced `Arc<Mutex<EnvStore>>` with `Rc<RefCell<EnvStore>>` for the
  single-threaded evaluation session, eliminating mutex overhead on every
  environment access.
- Added a per-layer `binding_index` for O(1) symbol lookup instead of reverse
  linear scans.

On `clarinetMIDI.dsp`, release evaluation time moved from ~0.57 s to ~0.53 s.
A phase-level `-time` flag was also wired to expose parser, eval, propagation,
signal-to-FIR, and backend codegen timings individually.

### 7.20 April 27, 2026: reverse-mode AD — phases A–D, corpus, documentation

#### RAD phases A–D implemented

Phase 1 reverse-mode AD landed in full on 2026-04-27:

- **Phase A**: two-child `rad(expr, seeds)` surface wired through
  parser/boxes/eval/propagate; arity contract `outputs = body.outputs +
  seeds.outputs` enforced with structured `RadBodyArity` / `RadSeedArity`
  diagnostics (`FRS-PROP-0002`).
- **Phase B**: feed-forward reverse-mode core in
  `crates/propagate/src/reverse_ad.rs`. Three-pass algorithm: (1) postorder
  DFS from each primal output; (2) initialize primals' adjoints to `1.0`,
  walk in reverse, emit local transpose contributions; (3) re-emit primals
  and append accumulated adjoint per seed.
- **Phase C**: extended rule table covering read-only `rdtable` (symmetric
  finite difference), unary foreign functions (`tanh`/`sinh`/`cosh` and
  inverse-hyperbolic), and pass-through wrappers (`attach`, `enable`,
  `control`, `Output`).
- **Phase D**: family-specific `RadUnsupportedNode` diagnostics with
  `kind`-dependent help text explaining why each temporal family (delay,
  prefix, recursion) cannot be handled by the local symbolic sweep alone
  (anti-causal transpose / BPTT boundary).

Output bundle: `[primals…, ∂ sum(primals) / ∂ s_0, …]` with implicit all-ones
cotangent on every primal output. Out-of-scope families raise
`RadUnsupportedNode`; after the 2026-05-12 dispatcher change, temporal and
recursive kinds use that error as an internal dispatch signal to
`BlockReverseAD`, while true hard-unsupported families still surface targeted
diagnostics. RAD never silently emits a misleading gradient.

#### Phase E0: recursive-linearity classifier

`crates/propagate/src/stateful_rad.rs` classifies `DEBRUIJNREC` groups as
`LinearLti`, `LinearTimeVarying`, or `Nonlinear`. The paired `RecRadMode` maps
these to future strategies (`LinearTranspose`, `BlockLinearTimeVarying`,
`BpttRequired`). This classifier now annotates the recursive fallback class;
the dormant `ReverseTimeRec` fast path remains disabled until the specialized
phase-E/F strategies are complete.

#### FAD de Bruijn invariants and multi-seed tests strengthened

`debug_assert!` invariants were added to `forward_ad.rs` verifying the
interleaved layout and `debruijn_depth` balance. New multi-seed × nested
recursion and multi-seed × mutual recursion central-difference tests were added
to `fad_recursive_runtime.rs`.

#### Corpus and documentation

- 7 positive RAD fixtures + 3 error fixtures (`err_rad_zero_body`,
  `err_rad_zero_seed`, `rad_delay1_block_fallback`).
- 4 mixed `fad(rad(...))` / `rad(fad(...))` corpus entries including
  `err_fad_rad_temporal`.
- RAD runtime test suite: 29 tests (10 inline + 14 corpus-driven + 5
  nested-AD), all green.
- `docs/rad-note-en.md` — synthesis note covering the three-pass algorithm,
  rule table, temporal boundary, and relationship to FAD.
- `porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md` updated with
  §19 feasibility analysis for stateful RAD (Tellegen transposition vs. BPTT).

### 7.21 April 28, 2026: RAD E1 scaffold, E0 integration, block-local semantics

- Block-local boundary semantics documented for phase E1: the compute block
  length is the finite reverse window, terminal adjoint at frame `count` is
  zero, no adjoint state persists across blocks.
- `crates/propagate/src/transpose_ad.rs`: scaffold that extracts affine LTI
  state-transition terms from one `DEBRUIJNREC` group and builds a same-arity
  transposed recurrence. Rejects LTV and nonlinear groups via the E0 classifier.
- Numeric oracle added: a small in-module interpreter evaluates the transposed
  recurrence backward in time and confirms the block-local seed adjoint matches
  an analytical reference (`+ ~ *(p)` first-order LTI, within `1e-4`).
- `RecRadMode` strategy gate wired: `recursive-linear-transpose`,
  `recursive-block-linear-time-varying`, and `recursive-bptt-required`
  diagnostic kinds now distinguish future phases in user-facing messages.
- Tests cover E2 (`coef`-dependent linear) and F (`sin` nonlinear) fallback
  classification with correct kind labels.

### 7.22 April 29, 2026: normalize simplify-cache sharing, wasm-ffi unblock, RAD demos

#### `normalize::simplify` cache shared across output forest

`simplify_signals_fastlane` previously created a fresh `SimplifyCache` per
output root. C++ shares a global property on tree nodes. A new `SimplifyCache`
wrapper is now allocated once and passed across all roots in `signal_prepare`,
mirroring C++ semantics. On `rad_fxlms1.dsp` this reduced compile time from
> 1 minute (killed) to ~1.6 s.

#### `wasm-ffi`: `Instant::now()` on `wasm32-unknown-unknown` fixed

`std::time::Instant::now()` panics on `wasm32-unknown-unknown`. The compiler
facade timing helper now skips `Instant` when no timing sink is installed. The
`wasm-ffi` embedded-library build also now supports embedding multiple `.lib`
roots from `FAUST_LIB_PATH` for project-local libraries.

#### RAD practical demos

- `crates/compiler/examples/rad_gradient_descent.rs` — host SGD loop fitting
  `rad_gain_bias_train.dsp` to a synthetic target; recovers `gain`/`bias` to
  ~1e-6 in 400 iterations.
- `crates/compiler/examples/rad_adaptive_notch.rs` — LMS adaptive notch
  converging to `|Δω| < 2e-4` in < 50 iterations.
- `crates/compiler/examples/rad_vs_fad_perf.rs` — side-by-side RAD vs. FAD
  benchmark; bytecode size and per-frame compute are within ~5 % on all five
  shapes, including a 6-seed multiplicative chain.
- `docs/rad-usage-en.md` — host-facing usage guide for all four demo fixtures.

### 7.23 April 30, 2026: Cranelift AArch64 guard, evaluator depth budgets

#### Cranelift: AArch64 oversized compute body guard

`fad_tracking3.dsp` triggered a `EXC_BAD_INSTRUCTION` on AArch64 when the JIT
`compute` body branched into padding/data islands. Added an AArch64-only guard:
when the first JIT lowering succeeds but the CLIF `compute` function is
oversized (`> 32 KiB` threshold), the module is discarded and replaced with the
existing no-op stub fallback. Later corrected on 2026-05-01 to remove the over-
aggressive post-success discard; the `catch_unwind` path for actual panics is
retained as the sole protection.

#### Evaluator depth budgets made configurable

Two constants renamed for clarity:

- `DEFAULT_EVAL_MAX_DEPTH` (default `1_024`) — identity-tracked evaluator
  recursion budget; also overridable via `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH`.
- `STRUCTURAL_HARD_MAX_DEPTH` (default `4_096`) — structural lowering cap for
  `a2sb` / `a2sb_value`; also overridable via
  `FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH`.

The two are kept separate so raising the identity-tracked budget for a known
deep acyclic program does not silently raise the structural-lowering stack risk.
The structural hard cap is an absolute ceiling regardless of
`with_max_depth(...)` calls.

### 7.24 May 1, 2026: isomorphic rec_merge pass, Cranelift cast fix, simplify fix

#### `normalize::rec_merge` — isomorphic `SYMREC` group merging

`crates/normalize/src/rec_merge.rs` introduces `merge_isomorphic_symrec_groups`:
opens each `SYMREC` body by replacing `Proj(i, self)` with canonical `Hole(i)`
sentinels, groups bodies by their hash-consed opened signature, elects one
canonical representative per equivalence class, and substitutes all aliases
throughout the signal forest. Runs in `signal_prepare` after the first
`simplify` pass and before a second `simplify`, so `Proj(i,W0) − Proj(i,W0)`
reduces to `Int(0)` on the second pass.

7 unit tests: single-output merge, multi-output, no spurious merge of distinct
groups, full `Int(0)` round-trip, nested groups, idempotence, hole isolation.

#### `normalize`: `x − x → 0` simplification rule

`BinOp::Sub` was the only missing case in the self-operation rule block.
Added `BinOp::Sub => SigBuilder::new(arena).int(0)` alongside the existing
`Rem`, `Xor`, `Gt`, `Lt` cases.

#### Cranelift: saturating float-to-int cast

`fcvt_to_sint` traps on NaN and out-of-range floats on AArch64 (`EXC_BAD_INSTRUCTION`).
C/C++ defines these as saturating. Both cast sites now use `fcvt_to_sint_sat`.
The Cranelift differential oracle also guards against spurious NaN mismatches
(`NaN == NaN` → skip rather than fail).

### 7.25 May 2–3, 2026: SVG block-diagram draw module, expandDSP/generateAuxFiles

#### Full SVG draw module (`crates/draw/`) — phases A–I

A complete port of `compiler/draw/` landed across 2026-05-02:

- **Core infra** (phase A): `DrawDevice` trait, `SvgDevice<W: Write>` writing
  raw XML with entity escaping, base `Schema` trait, `Orientation`, `Point`,
  `Trait`, `TraitCollector`, `Placement`, color palette.
- **Leaf schemas** (phase B): `BlockSchema`, `InverterSchema`, `CableSchema`,
  `CutSchema`, `ConnectorSchema`.
- **Composition schemas** (phase C): `SeqSchema`, `ParSchema`, `MergeSchema`,
  `SplitSchema`, `RecSchema` with feedback/feedfront wire routing.
- **Decorator schemas** (phase D): `EnlargedSchema`, `DecorateSchema`,
  `TopSchema`.
- **Specialized schemas** (phase E): `RouteSchema`, `MultiRateSchema`
  (`ondemand`, `upsampling`, `downsampling`).
- **Translation layer** (phase F): `generate_schema(arena, BoxId)` maps all
  `BoxMatch` variants to schema types; uses the evaluated box tree so lambdas
  appear as resolved primitives.
- **CLI wiring** (phase G): `--svg` / `-svg` flag; output dir `<stem>-svg/`.
- **Visual options** (phase H): `-blur`, `-sc`, `-drf`, `-mns N` (shadow,
  scaled SVG, route frame, name truncation).
- **Hierarchical folding** (phase I): `-f N` / `-fc N`; splits complex diagrams
  into linked SVG files. Requires def-name tracking from eval, which was also
  fixed on 2026-05-03 (closure re-evaluation now propagates def-names through
  `eval_value` so `karplus.dsp`-class programs fold correctly).

24 draw unit tests; all pass.

#### `expandDSP` / `generateAuxFiles` implemented (2026-05-03)

- `Compiler::expand_dsp(request)` validates source via
  `compile_source_to_signals_with_search_paths` and returns the original
  source string. The Rust compiler has no box→DSP serializer, so the
  "expanded" form equals the input — sufficient for JS tooling validation.
- `Compiler::generate_aux_files(request)` inspects argv for `-cpp`, `-c`,
  `-wasm`, `-json`, `-svg` flags, calls the appropriate compile method for
  each, and returns a `Vec<AuxFileArtifact>`.
- Four C `#[unsafe(no_mangle)]` exports added to `cranelift-ffi` and
  `interp-ffi`: `expandC*DSPFromFile`, `expandC*DSPFromString`,
  `generateC*AuxFilesFromFile`, `generateC*AuxFilesFromString`, each with
  matching C++ inline wrapper functions in the respective `.h` headers.

5 new compiler library tests cover both functions end-to-end.

### 7.26 May 10–12, 2026: RAD safety gate, BRA tape typing, AD wrapper slicing

#### Reverse-time-recursive RAD fast path disabled

The experimental `ReverseTimeRec` dispatcher path for RAD was disabled until
the planned phase-E recursive strategies are complete. Recursive RAD bodies now
route through the `BlockReverseAD` fallback instead of entering an incomplete
reverse-time-recursive lowering path.

#### BRA tape store typing

BRA forward tape arrays are real-valued FIR storage introduced during
`signal_fir` lowering. These tape buffers are not signal-IR nodes and therefore
do not pass through the earlier `normalize` / `signalPromotion` phase. The FIR
lowering boundary now casts taped integer forward values to the tape element
type before storing them, preserving the real-valued tape contract used by the
backward sweep and avoiding interpreter stack-type mismatches.

#### FAD/RAD wrapper input slicing

`fad(body, seed)` and `rad(body, seeds)` expose the maximum of the body and seed
input arities as their public wrapper input arity. Internally, each child must
still be evaluated with only the prefix of that bus matching its own arity.

The RAD regression case is:

```faust
process = rad(*, (_,_,_));
```

The wrapper has inputs `x0`, `x1`, `x2`, but the body `*` consumes only `x0`
and `x1`. The expected outputs are:

1. primal: `x0 * x1`
2. `d(body)/d(seed0)`: `d(x0*x1)/d(x0) = x1`
3. `d(body)/d(seed1)`: `d(x0*x1)/d(x1) = x0`
4. `d(body)/d(seed2)`: `d(x0*x1)/d(x2) = 0`

The same slicing contract applies to `fad(*, (_,_,_))`; FAD obtains the lanes
by propagating one tangent direction per seed output, while RAD accumulates
adjoints back from the scalar body output cotangent.

### 7.27 May 13–17, 2026: Julia backend and shared RAD local rules

#### Julia backend first slice

An initial FIR-to-Julia backend landed across 2026-05-13/14:

- `-lang julia` is accepted by the CLI and emits a Faust-style Julia DSP shell.
- `JuliaOptions` and `JuliaRealType` expose backend configuration, with
  `-double` selecting `const REAL = Float64`.
- `-a/--architecture` wrapping is accepted for Julia, using the existing
  enrobage marker replacement pipeline extended to understand Julia
  `include(...)` forms.
- UI widget and bargraph zone arguments are emitted as Julia `Symbol`s, as
  expected by the Faust Julia runtime.
- real casts inside parametric DSP methods use `T(...)`; integer casts use
  Julia truncation/wrapping helpers, including `faust_wrap_int32(...)` for
  C++-style 32-bit wraparound.
- the prologue defines `fmod` and `atan2` aliases needed by the impulse corpus,
  and float literals use shortest round-trippable formatting.

The backend is still a first slice over the existing FIR fast-lane, not a full
claim of C++ Julia backend parity.

#### Shared RAD local-rule table

The local reverse-AD operator rules were factored into
`crates/signals/src/ad_rules.rs`.

The pure Signal RAD pass in `crates/propagate/src/reverse_ad.rs` and the
FIR/BRA reverse sweep in `crates/transform/src/signal_fir/module.rs` now share:

- classification of supported unary math rules,
- classification of supported binary math rules,
- classification of differentiable arithmetic versus discrete zero-gradient
  `SIGBINOP`s,
- backend-neutral formula construction through `RadFormulaBuilder`.

This deliberately shares only local algebra. Signal-IR construction, BRA tape
loads, real tape storage, reverse-loop scheduling, FIR helper registration, and
backend typing remain owned by their respective crates.

## 8. Practical Reading Rule

Today, the simplest accurate rule is:

- If a Faust program stays within the language families already exercised by
  the tracked corpus, `faust-rs` front-end support is likely good.
- If that program also lowers to the current fast-lane signal subset
  (no non-trivial stream wrappers), end-to-end C/C++ generation is likely
  to work.
- Variable delays driven by a UI slider, numentry, audio-rate expression, or
  any expression with a provably bounded non-negative interval **are** now
  supported end-to-end.  This includes `de.delay(n, d)` patterns using
  `ma.SR` (sampling rate).
- The only variable-delay case still blocked is one where the delay amount
  has no statically determinable finite upper bound.
- `fad(exp, x)` programs propagate correctly and produce the expected
  primal/tangent bundle when the seed is an ordinary explicit signal or a
  structured recursive use already covered by the corpus, including
  lambda-bound recursive seeds (`kin(phi) = … fad(eq, phi) …`) and nested
  `fad(fad(eq, phi), phi)` on recursive accumulators. End-to-end backend
  compilation succeeds whenever the underlying tangent signal content stays
  within the supported fast-lane subset (same as for primal-only programs).
- `rad(expr, seeds)` is supported on the feed-forward subset and routes
  temporal/recursive differentiated bodies (`delay`, `prefix`, recursion) to
  the `BlockReverseAD` fallback rather than to the local symbolic transpose.
  See [docs/rad-note-en.md](../docs/rad-note-en.md).
- `-svg` produces block-diagram SVG output matching the C++ compiler output
  directory convention; hierarchical folding via `-f` splits complex diagrams.
- `-lang julia` now emits Julia source for programs in the current FIR
  fast-lane subset. Treat it as an initial backend slice: it supports the
  Faust-style Julia shell, `-double`, architecture wrapping, UI symbol zones,
  and C++-aligned numeric casts, but full upstream Julia impulse-suite parity
  is not yet claimed.
- `rad(expr, seeds)` over `hslider` / `vslider` / `numentry` seeds and a
  feed-forward body drives host-side training loops and adaptive filters
  (see `crates/compiler/examples/rad_gradient_descent.rs` and
  `examples/rad_adaptive_notch.rs`).

The most common mistake is to assume:

- "front-end accepted" means "backend supported".

That is no longer true as a general rule.

## 9. Update Policy for This Document

When this document is updated, the update should:

1. rerun:
   - `cargo run -p xtask -- corpus-status-report`
   - `cargo run -p xtask -- backend-full-corpus-diff-report`
2. update the top-level counts,
3. record any newly supported or newly unsupported valid corpus families,
4. update the "Important current backend exclusions" section,
5. add one historical note when a major boundary moves
   (for example: interval analysis landed, variable delays enabled, stream
   wrappers closed, new long-tail family supported).
