# xtask

Developer and CI automation for the `faust-rs` workspace.

`xtask` is not part of the compiler runtime. It hosts repository maintenance
workflows: golden snapshot generation/checks, runtime trace validation,
backend-alignment smoke runs, differential reports, code-graph generation, and
the `faustwasm` compiler-module build helper.

## Usage

Run from the workspace root:

```bash
cargo run -p xtask -- <command> [options]
```

Show the command summary:

```bash
cargo run -p xtask
```

## Prerequisites

- Rust toolchain with the workspace default target installed.
- `FAUST_CPP_BIN` when running C++ reference workflows.
- Graphviz `dot` when running `code-graphs`, because SVG rendering is produced
  from DOT files.
- `wasm32-unknown-unknown` target when running
  `build-faustwasm-compiler-module`.
- Clang C++ when running `lockstep-simd-check` (override `clang++` with
  `CLANGXX`).

Useful setup commands:

```bash
rustup target add wasm32-unknown-unknown
dot -V
```

## Commands

| Command | Description |
|---|---|
| `golden-check` | Verify Rust golden snapshots are up to date |
| `golden-check-cpp` | Verify Rust output matches C++ reference goldens |
| `golden-gen-rust` | Regenerate Rust golden snapshots from current output |
| `golden-gen-cpp` | Regenerate C++ reference goldens using `FAUST_CPP_BIN` |
| `interp-trace-dump` | Execute one DSP through the Rust interpreter backend and print a JSON trace |
| `interp-trace-dump-cppfbc` | Generate C++ Faust `.fbc`, execute it with the Rust interpreter runtime, and print a JSON trace |
| `interp-trace-gen-cppfbc` | Batch-generate persisted runtime traces from C++ Faust `.fbc` files |
| `interp-trace-gen` | Generate Rust runtime trace snapshots for `tests/runtime_corpus/` |
| `interp-trace-check` | Compare Rust runtime traces against persisted snapshots |
| `fir-dump-scan` | Scan `dump_fir` output for loop-body expansion regressions |
| `build-faustwasm-compiler-module` | Build `wasm-ffi` for `wasm32-unknown-unknown` and verify the raw export ABI |
| `backend-align-smoke` | CI-friendly alignment orchestration |
| `backend-align-nightly` | Broader alignment orchestration intended for longer jobs |
| `code-graphs` | Generate Mermaid/DOT/SVG crate graphs, IR overview graphs, and a public API source-scan index |
| `libfaust-api-matrix` | Generate Box and Signal C API parity matrices from libfaust reference headers |
| `parser-parity-report` | Write parser parity report vs C++ |
| `corpus-status-report` | Write corpus status diff report |
| `cpp-backend-diff-report` | Write C++ backend diff report |
| `c-fastlane-diff-report` | Write C fast-lane diff report |
| `backend-full-corpus-diff-report` | Write full corpus diff for all backends |
| `table-fastlane-diff-report` | Write table fast-lane diff report |
| `vector-coverage-merge` | Validate and merge `count_vector_corpus` JSON reports into the checked vector-coverage baseline |
| `vector-coverage-check` | Recompile every baseline-certified mode/DSP pair and require checked vector chunk-driver structure |
| `vector-compile-budget-check` | Measure the versioned release scalar/vector compile-time basket and reject unexplained regressions |
| `vector-interp-opt-check` | Compare interpreter `opt_level=0` and max optimization on representative checked-vector cases |
| `lockstep-simd-check` | Require Clang to emit four-wide LLVM floating-point operations for complex lockstep corpus cases |

## Vector Coverage Retention

The checked vector coverage baseline is
`tests/vector-coverage/corpus-baseline.json`. It contains all float/double,
`-lv 0/1`, and `-ss 0..3` results, including each fallback reason. Generate
the sixteen input reports with the compiler diagnostic example, then merge them:

```bash
cargo run -p compiler --example count_vector_corpus -- 0 0 --precision=f32 --json > /tmp/vector-f32-lv0-ss0.json
# Repeat for both precisions, -lv 0/1, and -ss 0..3.
cargo run -p xtask -- vector-coverage-merge --reports /tmp/vector-reports
```

`vector-coverage-check` validates the baseline is complete, checks its
universally certified benchmark list, recompiles every claimed certified pair,
and requires `Certified` status, `CertifiedVector` effective mode, no fallback
detail, and the canonical `vindex`/`vcount` chunk driver. It checks up to four
modes concurrently while preserving deterministic mode-ordered reporting. Each
worker owns its compiler instances and an explicit 16 MiB stack for the
compiler's recursive traversals, and every mode/DSP pair still receives the same
fail-closed checks. The Ubuntu CI job installs the Faust standard libraries
before running this check.

```bash
cargo run -p xtask -- vector-coverage-check
cargo run -p xtask -- vector-interp-opt-check
cargo run -p xtask -- lockstep-simd-check
```

`vector-compile-budget-check` warms each basket entry before measuring scalar
and vector compilation, then applies versioned absolute ceilings and a
noise-tolerant vector/scalar ratio. It must run with release optimizations:

```bash
cargo run --release -p xtask -- vector-compile-budget-check
```

## Environment Variables

| Variable | Used by | Description |
|---|---|---|
| `FAUST_CPP_BIN` | `golden-gen-cpp`, `interp-trace-dump-cppfbc`, `interp-trace-gen-cppfbc` | Path to the reference C++ `faust` binary |
| `GOLDEN_REF` | `golden-check` | `rust` (default) or `cpp` |
| `CLANGXX` | `lockstep-simd-check` | Clang C++ executable used to emit optimized LLVM IR (default: `clang++`) |

## Design Invariants

- Deterministic corpus ordering: files are sorted before processing.
- Normalized snapshot text: CRLF is normalized to LF before comparison.
- Fail-fast golden checks: the first diverging case aborts the run.
- Repository-relative paths are preferred in generated documentation.
- Runtime trace comparison uses exact metadata/shape checks and tolerant float
  sample comparison.

## Golden Snapshots

Golden snapshots validate stable compiler text output on `tests/corpus/`.

### `golden-check`

Checks Rust reference snapshots under `tests/golden/rust/`.

```bash
cargo run -p xtask -- golden-check
```

Equivalent explicit reference selection:

```bash
GOLDEN_REF=rust cargo run -p xtask -- golden-check
```

### `golden-check-cpp`

Checks current Rust output against C++ reference snapshots under
`tests/golden/cpp/`.

```bash
cargo run -p xtask -- golden-check-cpp
```

This is the long-run parity gate. It is only expected to pass when the checked
corpus and `tests/golden/cpp/` are intentionally aligned.

### `golden-gen-rust`

Regenerates Rust reference snapshots from current Rust output.

```bash
cargo run -p xtask -- golden-gen-rust
```

Use this only when intentionally refreshing Rust snapshots. Document the refresh
in the journal and PR notes.

### `golden-gen-cpp`

Regenerates C++ reference snapshots by invoking the reference Faust compiler.

```bash
FAUST_CPP_BIN=/path/to/faust cargo run -p xtask -- golden-gen-cpp
```

Extra arguments can be passed to the C++ compiler after `--`:

```bash
FAUST_CPP_BIN=/path/to/faust cargo run -p xtask -- golden-gen-cpp -- -vec
```

## Runtime Traces

Runtime trace workflows validate interpreter execution behavior on deterministic
input scenarios.

Supported scenarios:

- `zeros`
- `impulse`
- `ramp`
- `sine`

Supported Rust lowering lane:

- `fast`

### `interp-trace-dump`

Compiles one DSP through Rust, builds an interpreter factory, executes a
scenario, and prints JSON.

```bash
cargo run -p xtask -- interp-trace-dump \
  --case tests/corpus/rep_31_extended_primitives.dsp \
  --scenario impulse \
  --lane fast
```

Optional strict FIR type guard:

```bash
cargo run -p xtask -- interp-trace-dump \
  --case tests/corpus/rep_31_extended_primitives.dsp \
  --scenario impulse \
  --strict-fir-types
```

`--strict-fir-types` rejects traces when FIR verification emits type-related
diagnostics such as `FIR-B03`.

### `interp-trace-dump-cppfbc`

Generates `.fbc` bytecode with the C++ Faust compiler, loads the bytecode with
the Rust interpreter runtime, executes a scenario, and prints JSON.

```bash
FAUST_CPP_BIN=/path/to/faust \
cargo run -p xtask -- interp-trace-dump-cppfbc \
  --case tests/corpus/rep_31_extended_primitives.dsp \
  --scenario impulse
```

Use `--faust-bin` to override `FAUST_CPP_BIN` for one run:

```bash
cargo run -p xtask -- interp-trace-dump-cppfbc \
  --case tests/corpus/rep_31_extended_primitives.dsp \
  --scenario impulse \
  --faust-bin /path/to/faust
```

This validates the Rust interpreter runtime independently from Rust FIR
lowering.

### `interp-trace-gen-cppfbc`

Batch-generates persisted traces from C++ `.fbc` files. Defaults:

- corpus: `tests/corpus/rep_*.dsp`
- output directory: `tests/runtime_traces/cppfbc`
- one scenario per invocation

Examples:

```bash
cargo run -p xtask -- interp-trace-gen-cppfbc \
  --case tests/corpus/rep_01_passthrough.dsp \
  --scenario impulse \
  --out-dir /tmp/runtime_traces_cppfbc
```

```bash
FAUST_CPP_BIN=/path/to/faust \
cargo run -p xtask -- interp-trace-gen-cppfbc \
  --scenario impulse
```

### `interp-trace-gen`

Generates Rust runtime trace snapshots under `tests/runtime_traces/rust/`.

```bash
cargo run -p xtask -- interp-trace-gen
```

With an explicit runtime corpus case:

```bash
cargo run -p xtask -- interp-trace-gen \
  --case tests/runtime_corpus/<case>.dsp \
  --lane fast \
  --strict-fir-types
```

### `interp-trace-check`

Regenerates Rust runtime traces and compares them with persisted snapshots.

```bash
cargo run -p xtask -- interp-trace-check
```

With an explicit runtime corpus case:

```bash
cargo run -p xtask -- interp-trace-check \
  --case tests/runtime_corpus/<case>.dsp \
  --lane fast \
  --strict-fir-types
```

Comparison rules:

- metadata and shape must match exactly;
- sample values use tolerant float comparison;
- `opt_level=0` vs `opt_level=max` drift checks are used by alignment workflows
  on the covered subset.

## FIR Dump Scan

`fir-dump-scan` scans `dump_fir` output on selected corpus cases to catch
structural regressions in loop-body emission and textual FIR shape.

```bash
cargo run -p xtask -- fir-dump-scan \
  --case tests/corpus/rep_01_passthrough.dsp \
  --lane fast
```

Multiple `--case` arguments are accepted. When no case is provided, the command
uses its built-in scan set.

## Backend Alignment

### `backend-align-smoke`

Runs the CI-friendly alignment subset.

```bash
cargo run -p xtask -- backend-align-smoke
```

Useful options:

```bash
cargo run -p xtask -- backend-align-smoke \
  --case tests/runtime_corpus/<case>.dsp \
  --strict-fir-types \
  --skip-golden \
  --skip-fir-dump-scan
```

The smoke workflow can combine:

- golden checks;
- FIR dump scans;
- Rust interpreter runtime trace checks;
- interpreter `opt_level=0` vs `opt_level=max` drift checks;
- Cranelift subset/runtime smoke checks where applicable.

### `backend-align-nightly`

Runs a broader alignment orchestration intended for longer jobs.

```bash
cargo run -p xtask -- backend-align-nightly
```

Useful options:

```bash
cargo run -p xtask -- backend-align-nightly \
  --strict-fir-types \
  --skip-golden \
  --skip-fir-dump-scan
```

## Faustwasm Compiler Module

`build-faustwasm-compiler-module` builds the raw Rust compiler module consumed
by the `faustwasm` embedded-compiler path and verifies its exported ABI.

```bash
cargo run -p xtask -- build-faustwasm-compiler-module
```

What it does:

- runs `cargo build -p wasm-ffi --target wasm32-unknown-unknown --release`;
- verifies the raw exports expected by the `faustwasm` Rust adapter;
- prints the resulting module path.

Debug profile:

```bash
cargo run -p xtask -- build-faustwasm-compiler-module --debug
```

Expected release artifact:

```text
target/wasm32-unknown-unknown/release/faust_wasm_ffi.wasm
```

## Code Graphs

`code-graphs` generates developer navigation artifacts under
`docs/code-graphs/` by default.

```bash
cargo run -p xtask -- code-graphs
```

Optional output directory:

```bash
cargo run -p xtask -- code-graphs --out-dir /tmp/faust-rs-code-graphs
```

Generated files:

| File | Description |
|---|---|
| `workspace-crates.mmd` | Mermaid workspace crate nodes from `cargo metadata` |
| `workspace-crates.dot` | DOT workspace crate nodes from `cargo metadata` |
| `workspace-crates.svg` | Rendered SVG workspace crate graph |
| `internal-crate-deps.mmd` | Mermaid internal crate dependency graph |
| `internal-crate-deps.dot` | DOT internal crate dependency graph |
| `internal-crate-deps.svg` | Rendered SVG internal crate dependency graph |
| `ir-overview.mmd` | Mermaid curated IR overview |
| `ir-overview.dot` | DOT curated IR overview |
| `ir-overview.svg` | Rendered SVG curated IR overview |
| `public-api-index.md` | Lightweight source-scan index of public items |
| `README.md` | Generated entry point with embedded SVG references |

The public API index is a quick map, not a replacement for:

```bash
cargo doc --workspace --no-deps
```

## libfaust API Matrices

`libfaust-api-matrix` compares `LIBFAUST_API` C symbols declared by the
reference Faust headers with Rust C exports currently present in the `*-ffi`
crates.

```bash
cargo run -p xtask -- libfaust-api-matrix \
  --cpp-root /Users/letz/Developpements/RUST/faust \
  --out porting/generated
```

Generated files:

| File | Description |
|---|---|
| `porting/generated/libfaust-box-c-api-matrix.md` | Box C API symbol coverage and known adapted mappings |
| `porting/generated/libfaust-signal-c-api-matrix.md` | Signal C API symbol coverage and missing first-slice targets |

The scanner is deliberately conservative: it classifies symbol presence, not
semantic parity. Rows marked `implemented-exact-candidate` still need focused
API tests before being treated as final parity.

## libfaust Export Check

`libfaust-export-check` validates the maintained local C/C++ distribution
surface. It builds `faust-ffi`, extracts dynamic exports from the produced
`libfaust` library, compares them against the Box and Signal C headers, and
syntax-checks tiny C11 and C++17 clients using the maintained headers.

```bash
cargo run -p xtask -- libfaust-export-check
```

## Differential Reports

Report commands write Markdown reports under `porting/phases/`.

| Command | Output |
|---|---|
| `parser-parity-report` | `porting/phases/phase-3-parser-parity-report-en.md` |
| `corpus-status-report` | `porting/phases/phase-4-corpus-status-diff-report-en.md` |
| `cpp-backend-diff-report` | `porting/phases/phase-6-cpp-backend-diff-report-en.md` |
| `c-fastlane-diff-report` | `porting/phases/phase-6-c-fastlane-diff-report-en.md` |
| `backend-full-corpus-diff-report` | `porting/phases/phase-6-backend-full-corpus-diff-report-en.md` |
| `table-fastlane-diff-report` | `porting/phases/phase-6-table-fastlane-diff-report-en.md` |

Examples:

```bash
cargo run -p xtask -- parser-parity-report
cargo run -p xtask -- corpus-status-report
cargo run -p xtask -- cpp-backend-diff-report
cargo run -p xtask -- c-fastlane-diff-report
cargo run -p xtask -- backend-full-corpus-diff-report
cargo run -p xtask -- table-fastlane-diff-report
```

Some report commands use the local C++ reference source tree configured in
`crates/xtask/src/main.rs` and/or a C++ Faust binary when available. Keep the
generated report paths repository-relative.

## Validation Before Commit

For changes to this crate, run at least:

```bash
cargo fmt --all
cargo check -p xtask
cargo clippy -p xtask --all-targets -- -D warnings
```

For workflow-specific changes, run the affected command as well. Examples:

```bash
cargo run -p xtask -- code-graphs
cargo run -p xtask -- fir-dump-scan
cargo run -p xtask -- backend-align-smoke --skip-golden
```
