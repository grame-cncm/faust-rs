# Faust-rs Developer Workflows

This document centralizes technical/developer-oriented usage that was previously in `README.md`.

## 1. Repository map

- `porting/faust-rust-porting-plan-en.md`: full porting plan
- `porting/faust-rust-points-critiques-en.md`: critical technical points and risks
- `porting/faust-rust-recursion-model-note-en.md`: recursion model analysis (`sigRec/sigProj` vs RouteIR rec groups)
- `porting/faust-rust-bilan-effort-en.md`: effort assessment
- `porting/faust-rust-bilan-global-en.md`: overall status summary
- `porting/faust-rust-error-flow-en.md`: concise parser -> eval -> propagate error flow
- `porting/phases/`: detailed phase-by-phase execution notes (`phase-0` to `phase-9`)
- `docs/signal-to-fir-recent-progress-en.md`: compact summary of recent
  Signal -> FIR fast-lane work (placement, CSE, delays, recursion extraction)

## 2. Suggested reading order

1. `porting/faust-rust-porting-plan-en.md`
2. `porting/faust-rust-points-critiques-en.md`
3. `porting/phases/phase-0-validation-en.md`
4. Remaining files in `porting/phases/` in numeric order

## 3. Build commands

```bash
# All crates (debug)
cargo build --workspace

# All crates (release)
cargo build --workspace --release

# Compiler crate only
cargo build -p compiler
```

## 4. Diagnostic runs

```bash
# Human diagnostics (default), concise note stream
cargo run -p compiler -- --dump-sig tests/corpus/err_03_propagate_split_mismatch.dsp --error-format human --error-verbosity standard

# Human diagnostics with internal debug notes
cargo run -p compiler -- --dump-sig tests/corpus/err_03_propagate_split_mismatch.dsp --error-format human --error-verbosity debug

# JSON diagnostics (stable contract)
cargo run -p compiler -- --dump-sig tests/corpus/err_03_propagate_split_mismatch.dsp --error-format json

# JSON diagnostics with debug enrichment (`diagnostics[*].debug`)
cargo run -p compiler -- --dump-sig tests/corpus/err_03_propagate_split_mismatch.dsp --error-format json --error-verbosity debug
```

See:

- `docs/user-diagnostics-guide-en.md`
- `docs/user-cli-guide-en.md`

## 5. CLI language model

The compiler currently supports:

- `-lang asc`
- `-lang c`
- `-lang cmajor`
- `-lang codebox` / `-lang codebox-test`
- `-lang cpp`
- `-lang cranelift`
- `-lang fir`
- `-lang interp`
- `-lang julia`
- `-lang rust`
- `-lang wasm`
- `-lang wast`

```bash
cargo run -p compiler -- -lang asc tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang c tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang cmajor tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang codebox tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang cpp tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang cranelift tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang fir tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang interp tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang julia tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang rust tests/corpus/rep_01_passthrough.dsp
cargo run -p compiler -- -lang wasm tests/corpus/rep_01_passthrough.dsp -o /tmp/out.wasm
cargo run -p compiler -- -lang wast tests/corpus/rep_01_passthrough.dsp
```

See `docs/user-cli-guide-en.md` for the full flag surface (`-vec`, `-ec`/`-os`,
`--table-init`, `--svg`, architecture wrapping, legacy flag spellings, etc.).

Useful current CLI extras for developer workflows:

- `--check` for front-end + FIR verification with no codegen, the preferred
  mode for CI/tooling validity checks (schema shared with success and failure)
- `--json` for strict Faust JSON output, optionally alongside `-lang <backend>`
- `--dump-fir-verify` for FIR verifier reports without backend emission
- `--dump-cranelift` for the experimental backend status report
- `--fir-fixture <name>` / `--list-fir-fixtures` for backend-only debugging
- `--signal-fir-lane fast` for the transform-owned lowering route in FIR-backed modes

## 6. Golden workflow

Corpus and golden layout:

- `tests/corpus/*.dsp`: input DSP corpus
- `tests/golden/rust/<case>/compiler_stdout.txt`: current Rust scaffold reference used by CI
- `tests/golden/cpp/<case>/compiler_stdout.txt`: C++ Faust reference outputs (parity target)
- `tests/golden/METADATA.toml`: pinned reference metadata and command policy

Commands:

```bash
# Check Rust output against stored golden references
cargo run -p xtask -- golden-check

# Check Rust output against C++ reference goldens (expected to fail until parity)
cargo run -p xtask -- golden-check-cpp

# Generate corpus-wide C++ vs Rust status differential report
cargo run -p xtask -- corpus-status-report

# Bootstrap/update golden files from current Rust scaffold output
cargo run -p xtask -- golden-gen-rust

# Update golden files from C++ Faust reference binary
FAUST_CPP_BIN=/path/to/faust cargo run -p xtask -- golden-gen-cpp -- <extra-args>
```

Note: CI runs `cargo run -p xtask -- golden-check` (Rust reference mode) on every platform.

## 7. Runtime and alignment workflows

Key `xtask` commands beyond golden snapshots:

```bash
cargo run -p xtask -- interp-trace-gen
cargo run -p xtask -- interp-trace-check
cargo run -p xtask -- fir-dump-scan --lane fast
cargo run -p xtask -- backend-align-smoke
```

Notes:

- `interp-trace-gen` / `interp-trace-check` operate on `tests/runtime_corpus/`
  and persist/validate traces under `tests/runtime_traces/rust/`.
- `fir-dump-scan` is a structural regression guard on textual FIR dumps.
- `backend-align-smoke` and `backend-align-nightly` orchestrate broader
  alignment checks, including runtime/FIR-dump coverage.
