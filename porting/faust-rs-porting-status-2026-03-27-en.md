# `faust-rs` Porting Status — 2026-03-27

## 1. Purpose

This document is a current-state assessment of the Faust C++ -> Rust port in
this repository.

It is intentionally different from the original planning documents:

- it describes what is implemented now, not what was planned initially,
- it is based on executable evidence from the current tree,
- it highlights where the codebase has outgrown older status reports,
- it separates production-capable areas from bring-up and scaffolded areas.

Reference baseline:

- C++ branch target: `master-dev-ocpp-od-fir-2-FIR19`
- pinned reference commit in project docs: `8eebea429`

Date of this assessment:

- `2026-03-27`

---

## 2. Executive Summary

The Rust port is no longer a skeleton. It already contains a large, coherent,
testable compiler stack with real end-to-end output paths.

What is effectively in place today:

- the core front-end pipeline is substantial and stable:
  `parse -> eval -> propagate -> signals`,
- the active Rust-native signal -> FIR fast lane is real and broadly useful,
- FIR is modeled as a large first-class IR with verification and inlining,
- C, C++, interpreter, WASM, and experimental Cranelift backends all exist and
  are executable,
- selected C/C++ FFI surfaces exist for boxes, interpreter, and Cranelift,
- the workspace test suite is green across all crates and targets.

What is still incomplete:

- the signal -> FIR lowering remains the main parity bottleneck,
- one known valid end-to-end corpus gap remains on the production fast lane:
  `rep_18_stream_wrappers.dsp`,
- the corpus now also exposes one Rust/C++ compatibility divergence on
  `rep_74_soundfile_basic.dsp` where Rust accepts/compiles a case that the C++
  reference rejects,
- several planned modules are still scaffold-only or largely deferred:
  `algebra`, `graph`, `draw`, `doc`, major non-primary backends, broad
  `libfaust` API parity, vectorization/work-stealing, and network import
  support.

Overall judgement:

- **Phases 1-4 are largely realized in code**
- **Phase 5 is substantial but not fully consumed by the active fast lane**
- **Phase 6 is the main active implementation front**
- **Phase 7 is partially started**
- **Phase 8 is mostly pending**
- **Phase 9 is partially implemented, but not yet full libfaust parity**

---

## 3. Evidence Used

This assessment is based on the current repository state plus direct local
validation performed on `2026-03-27`.

Repository-level evidence:

- workspace manifest: `Cargo.toml`
- current compiler facade and lane wiring: `crates/compiler/src/lib.rs`
- current CLI and diagnostics UX: `crates/compiler/src/main.rs`
- fast-lane lowering: `crates/transform/src/signal_prepare.rs`,
  `crates/transform/src/signal_fir/`
- FIR model and verifier: `crates/fir/src/lib.rs`,
  `crates/fir/src/checker.rs`,
  `crates/fir/src/inliner.rs`
- backend implementations: `crates/codegen/src/backends/`
- FFI layers: `crates/interp-ffi/`, `crates/cranelift-ffi/`,
  `crates/box-ffi/`, `crates/faust-ffi/`, `crates/wasm-ffi/`
- porting status docs and journals under `porting/`

Commands run for this assessment:

```bash
cargo test --workspace --all-targets
cargo test --workspace --all-targets -- --list
```

Additional local corpus checks were run with:

```bash
target/debug/faust-rs --dump-sig tests/corpus/<case>.dsp
target/debug/faust-rs tests/corpus/<case>.dsp -lang cpp -o /tmp/out.cpp
target/debug/faust-rs tests/corpus/<case>.dsp -lang c -o /tmp/out.c
../faust/build/bin/faust tests/corpus/<case>.dsp -lang cpp -o /tmp/ref.cpp
../faust/build/bin/faust tests/corpus/<case>.dsp -lang c -o /tmp/ref.c
```

---

## 4. Repository Snapshot

Current workspace size:

- workspace crates: `26`
- Rust source files under `crates/`: `171`
- Rust LOC under `crates/`: `112,558`
- compile corpus files under `tests/corpus/`: `104`
- runtime corpus files under `tests/runtime_corpus/`: `8`
- declared tests discovered by `cargo test -- --list`: `1002`

Largest crates by Rust LOC:

| Crate | LOC | Interpretation |
|---|---:|---|
| `codegen` | `31,676` | large real backend implementation, not a stub crate |
| `compiler` | `11,390` | substantial orchestration/CLI/diagnostics facade |
| `fir` | `10,871` | large IR + verifier + inliner surface |
| `eval` | `9,542` | real evaluator, not a placeholder |
| `transform` | `7,035` | real signal preparation and fast-lane lowering |
| `cranelift-ffi` | `4,980` | sizeable experimental FFI/runtime layer |
| `xtask` | `4,479` | significant parity/golden/report tooling |
| `propagate` | `4,377` | real box -> signal lowering implementation |
| `normalize` | `3,705` | non-trivial simplification/normal-form work |
| `parser` | `3,676` | real parser/import/context stack |

Small crates that are still explicitly scaffold-only:

- `algebra`
- `graph`
- `draw`
- `doc`

Small crate with a real but narrow role:

- `faust-ffi` is intentionally only an aggregator/distribution crate over
  `box-ffi`, `interp-ffi`, and `cranelift-ffi`

---

## 5. Health of the Current Tree

Local validation result for the current tree:

- `cargo test --workspace --all-targets` passed

This matters because it shows the port is not just a collection of crates:

- the parser/eval/propagate/transform/FIR/codegen path is executable,
- the compiler facade and CLI are exercised,
- differential tests against the C++ reference exist and pass for covered
  cases,
- FFI layers build and test,
- WASM and Cranelift code paths are part of the testable tree.

Important qualification:

- a green test suite does **not** mean full C++ parity,
- it means the implemented subset is internally consistent and regression-guarded.

---

## 6. Status by Subsystem

| Subsystem | Current status | Notes |
|---|---|---|
| `tlib`, `errors`, `interval` | **implemented and stable** | Real foundations, with tests and direct consumption by upper crates |
| `boxes` | **implemented** | Builder/matcher API is established and tested |
| `parser` | **near parity on local-file production flow** | Real grammar, metadata handling, imports, structural differentials against C++ |
| `eval` | **near parity on tracked production corpus** | Environments, closures, patterns, iterator expansion, source loading, diagnostics |
| `propagate` + `ui` | **near parity on tracked production corpus** | Real box -> signal lowering and grouped UI artifact extraction |
| `signals` | **implemented** | Rich signal IR with stream wrappers, recursion, tables, UI, soundfile forms |
| `sigtype` | **substantial but not fully exploited downstream** | Full type lattice exists, but fast-lane lowering still consumes only a reduced view |
| `normalize` | **substantial but not fully on the hot path** | Real simplification/normal form work exists; full C++ normalization pipeline is not fully wired into production lowering |
| `transform::signal_prepare` | **implemented** | Important staging bridge: de Bruijn conversion, type recovery, promotion, recursion cleanup |
| `transform::signal_fir` | **substantial, main parity bottleneck** | Real lowering pipeline, but still the largest remaining semantic gap |
| `fir` | **implemented** | Large IR surface, verifier, inliner, matcher/builder APIs |
| `codegen::c` / `cpp` | **production-capable subset** | Current primary code-emission path |
| `codegen::interp` | **implemented** | FIR -> FBC, runtime executor, `.fbc` I/O, AOT C++ emitter from bytecode |
| `codegen::wasm` | **real bring-up backend** | No longer scaffold-only; emits `.wasm` + JSON with tested ABI/layout paths |
| `codegen::cranelift` | **experimental bring-up backend** | Real JIT-backed execution exists, but parity/coverage is still evolving |
| `interp-ffi` | **functional parity slice** | Factory/instance lifecycle, compute, UI/meta callbacks, bitcode paths |
| `cranelift-ffi` | **experimental but real** | JIT-backed factory/instance layer, cache, runtime descriptor, headers |
| `box-ffi` | **incremental parity layer** | Broad constructor surface exists; advanced matcher parity is incomplete |
| `faust-ffi` | **aggregation layer only** | Not yet a full Rust replacement for the historical libfaust surface |
| `wasm-ffi` | **real embedded-compiler slice** | Raw compile service for `faustwasm`-style integration, but helper parity is partial |
| `algebra`, `graph`, `draw`, `doc` | **scaffold-only** | Planned architecture slots, not yet meaningful implementations |

---

## 7. Current Parity Snapshot

### 7.1 Front-end acceptance parity on the current corpus

Measured as:

- C++ reference: success/failure of `faust <case> -lang cpp`
- Rust front-end: success/failure of `faust-rs --dump-sig <case>`

Current result on `104` corpus cases:

| Class | Count |
|---|---:|
| `OK/OK` | `88` |
| `ERR/ERR` | `15` |
| `OK/ERR` | `0` |
| `ERR/OK` | `1` |

Interpretation:

- there is **no current case** where C++ accepts the tracked corpus and Rust
  fails before the signal boundary,
- there is now **one inverse divergence**:
  `tests/corpus/rep_74_soundfile_basic.dsp`

That divergence is not harmless bookkeeping. It means:

- the current statement "front-end parity is complete on the tracked corpus" is
  no longer strictly true for the expanded `104`-case corpus,
- Rust is currently more permissive than the reference on this soundfile case,
- this needs an explicit policy decision: parity bug in Rust, or intentional
  behavioral improvement that should remain documented as a divergence.

Observed reference failure on `rep_74_soundfile_basic.dsp`:

- C++ rejects it with an out-of-range soundfile-part diagnostic
- Rust currently lowers it successfully to signals and then through all checked
  Rust backends

### 7.2 End-to-end C++ backend parity on the current corpus

Measured as:

- C++ reference: success/failure of `faust <case> -lang cpp`
- Rust: success/failure of `faust-rs <case> -lang cpp`

Current result on `104` corpus cases:

| Class | Count |
|---|---:|
| `OK/OK` | `87` |
| `ERR/ERR` | `15` |
| `OK/ERR` | `1` |
| `ERR/OK` | `1` |

Current mismatches:

- `tests/corpus/rep_18_stream_wrappers.dsp`
  - C++: `OK`
  - Rust: `ERR`
  - current known production fast-lane gap
- `tests/corpus/rep_74_soundfile_basic.dsp`
  - C++: `ERR`
  - Rust: `OK`
  - compatibility divergence, not a regression in Rust robustness terms

### 7.3 End-to-end C backend parity on the current corpus

Measured as:

- C++ reference: success/failure of `faust <case> -lang c`
- Rust: success/failure of `faust-rs <case> -lang c`

Current result on `104` corpus cases:

| Class | Count |
|---|---:|
| `OK/OK` | `87` |
| `ERR/ERR` | `15` |
| `OK/ERR` | `1` |
| `ERR/OK` | `1` |

The mismatch set is the same as the C++ backend route:

- `rep_18_stream_wrappers.dsp`
- `rep_74_soundfile_basic.dsp`

### 7.4 What this means

The primary production story today is:

- the Rust front-end is very strong,
- the production fast lane is strong enough for most valid tracked corpus
  programs,
- the fast-lane still has **one clear valid end-to-end gap** relative to the
  C++ reference: stream wrappers,
- the corpus has now uncovered **one compatibility divergence** on soundfile
  semantics that should be treated explicitly in parity tracking.

---

## 8. Non-Primary Backend Snapshot

Targeted spot checks on recent edge cases show a more nuanced picture than the
older "implemented vs scaffold" split.

Checked cases:

- `rep_74_soundfile_basic.dsp`
- `rep_77_foreign_variable.dsp`
- `rep_78_foreign_function.dsp`
- `rep_79_multi_output_recursion.dsp`

Observed Rust backend status:

| Case | `c` | `cpp` | `interp` | `cranelift` | `wasm` | `wast` |
|---|---|---|---|---|---|---|
| `rep_74_soundfile_basic` | OK | OK | OK | OK | OK | OK |
| `rep_77_foreign_variable` | OK | OK | OK | OK | ERR | ERR |
| `rep_78_foreign_function` | OK | OK | OK | OK | ERR | ERR |
| `rep_79_multi_output_recursion` | OK | OK | OK | OK | OK | OK |

Implications:

- Cranelift has advanced beyond earlier same-day journal notes and now handles
  the checked foreign-symbol cases,
- the current WASM backend is real and already covers soundfile and
  multi-output recursion cases in the checked subset,
- WASM/WAST still have explicit remaining gaps on foreign variable/function
  support.

Important documentation drift:

- older analysis documents that still describe the Rust WASM backend as
  "scaffold only" are no longer accurate for the current tree.

---

## 9. Main Strengths of the Current Port

### 9.1 The core compiler pipeline is real

The repository now contains an executable compiler stack, not just planning:

- parser
- evaluator
- propagation
- signal preparation
- signal -> FIR lowering
- FIR verification
- backend emission

### 9.2 The project has strong regression infrastructure

Evidence already present in the tree:

- `1002` declared tests across crates and targets
- golden workflows in `xtask`
- parser/eval/compiler differential tests against the C++ reference
- runtime trace infrastructure
- focused backends and FFI tests

### 9.3 Provenance discipline is better than in many ports

Large parts of the codebase explicitly document:

- C++ source provenance
- mapping status (`1:1`, `adapted`, `deferred`)
- invariants and parity notes

This materially reduces the risk of accidental semantic drift.

### 9.4 The project already has multiple viable output stories

Today the repository can already produce:

- C source
- C++ source
- interpreter bytecode
- C++ from `.fbc`
- WASM binary + companion JSON
- experimental Cranelift runtime output

That is a strong sign that the port is beyond proof-of-concept stage.

---

## 10. Main Weaknesses and Remaining Risks

### 10.1 `signal_fir` is still the main parity choke point

The project has moved the hardest problem into a concentrated location:

- front-end parity is comparatively strong,
- IR representation is strong,
- backends exist,
- but the active Rust-native signal -> FIR lowering still decides whether many
  valid programs reach those backends.

The current visible manifestation remains:

- `rep_18_stream_wrappers.dsp`

### 10.2 The soundfile divergence is now a real parity issue

`rep_74_soundfile_basic.dsp` proves that the expanded corpus has outpaced older
status claims.

Current situation:

- Rust accepts and compiles the case,
- C++ rejects it,
- the divergence is observable at both front-end and backend acceptance level.

That should now be tracked as one of:

- a Rust parity bug,
- or an explicit behavioral deviation with justification.

It should not remain implicit.

### 10.3 Several planned crates are still architectural placeholders

These crates still contribute almost no functional porting value today:

- `algebra`
- `graph`
- `draw`
- `doc`

This is not a problem by itself, but it means the workspace shape is ahead of
the implementation in those areas.

### 10.4 Fast-lane consumers do not yet exploit the full analysis stack

Two examples matter:

- `sigtype` is more complete than the effective type information consumed by
  lowering,
- `normalize` contains real logic, but the active production path still does
  not look like full C++ normalization parity.

So the repository contains more semantic machinery than the hot path fully uses.

### 10.5 Top-level `libfaust` parity is still incomplete

What exists today:

- `box-ffi`
- `interp-ffi`
- `cranelift-ffi`
- aggregation through `faust-ffi`

What does not exist yet at C++-reference scale:

- broad parity with the historical `box_signal_api.cpp` / `libfaust` surface
- full lifecycle/API coverage for all major compiler entry points

This is one of the clearest gaps between "compiler port" and
"drop-in libfaust replacement".

### 10.6 Some major planned backend areas remain pending

Still mostly absent or scaffolded:

- LLVM backend
- Rust backend
- many secondary language backends
- vectorized compilation path
- work-stealing / parallel compilation path

### 10.7 Some documents are now stale

The current tree has outpaced several status documents:

- some corpus-based reports still reflect a `98`-case corpus,
- `faust-rs-supported-faust-subset-en.md` still reflects the older parity
  picture and does not account for the new `rep_74` divergence,
- older analysis docs still classify WASM as scaffold-only, which is no longer
  accurate.

This is a documentation risk because readers can otherwise underestimate or
misclassify the current state of the port.

---

## 11. Phase-Level Assessment

### Phase 1 — Foundations

Assessment:

- **largely realized**

Reason:

- `tlib`, `errors`, `interval`, and supporting utilities are real, used, and
  tested

### Phase 2 — Block diagrams

Assessment:

- **realized**

Reason:

- `boxes` provides a broad builder/matcher surface and is actively used by
  parser/eval/propagate

### Phase 3 — Parser

Assessment:

- **largely realized on the local-file production path**

Residual exclusions:

- network import/sourcefetcher remains explicitly deferred

Follow-up (2026-08-11): network imports are still unimplemented, but the
deferral now has a concrete native implementation contract and staged closure
plan in
[`sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md`](sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md).

### Phase 4 — Signals / eval / propagate

Assessment:

- **largely realized**

Reason:

- the tracked corpus and differential tests show a strong implementation
  envelope here

### Phase 5 — Normalization / recursive preparation / typing

Assessment:

- **substantial but incomplete in production integration**

Reason:

- real code exists, but the active fast lane still does not consume all of it
  with full C++ parity behavior

### Phase 6 — FIR and backends

Assessment:

- **substantial and active**

Reason:

- FIR itself is strong,
- C/C++/interp are real,
- WASM is real bring-up,
- Cranelift is real experimental bring-up,
- but fast-lane lowering remains the critical missing parity piece

### Phase 7 — Additional backends

Assessment:

- **partially started**

Real progress:

- interpreter
- WASM
- Cranelift

Still pending/scaffolded:

- LLVM and many secondary language backends

### Phase 8 — Draw / doc

Assessment:

- **mostly pending**

### Phase 9 — Integration / enrobage / FFI

Assessment:

- **partially realized**

Real progress:

- compiler facade
- CLI
- diagnostics model in production use
- enrobage integration
- selected FFI slices

Still missing:

- broad `libfaust` API completeness
- full top-level compatibility tiering

---

## 12. Recommended Next Actions

Priority order based on the current codebase:

1. **Decide the soundfile parity policy for `rep_74_soundfile_basic.dsp`.**
   The current Rust/C++ acceptance split should be classified explicitly as
   either bug or intentional divergence.

2. **Close the remaining valid fast-lane backend gap on
   `rep_18_stream_wrappers.dsp`.**
   This is the cleanest visible proof that `signal_fir` still needs more
   stream-wrapper coverage.

3. **Refresh stale status documents to the `104`-case corpus.**
   In particular:
   - `porting/faust-rs-supported-faust-subset-en.md`
   - `porting/phases/phase-4-corpus-status-diff-report-en.md`
   - `porting/phases/phase-6-backend-full-corpus-diff-report-en.md`

4. **Push more of `sigtype` and `normalize` into the production fast lane.**
   The code already exists; the remaining issue is effective integration and
   parity behavior.

5. **Keep backend maturity labels explicit.**
   Current recommended labels:
   - `c`, `cpp`, `interp`: production-capable subset
   - `wasm`: real bring-up backend
   - `cranelift`: experimental backend
   - remaining secondary backends: scaffolded

6. **Continue narrowing the gap between "compiler works" and
   "libfaust replacement".**
   That means broad API-surface triage, not just more backend code generation.

---

## 13. Bottom Line

`faust-rs` is already a substantial compiler port with a real executable
pipeline, a large IR and backend stack, a green workspace test suite, and
meaningful differential validation against the C++ reference.

It should no longer be described as an early prototype.

It should also not yet be described as feature-complete C++ parity.

The most accurate short description today is:

- **front-end port largely realized**
- **backend core substantially realized**
- **fast-lane parity still incomplete**
- **FFI/libfaust parity still partial**
- **documentation now needs a refresh to catch up with the code**
