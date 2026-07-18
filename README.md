# faust-rs

Rust workspace for the Faust compiler port.

[![CI](https://github.com/sletz/faust-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/sletz/faust-rs/actions/workflows/ci.yml)

## Build

```bash
# Entire workspace
cargo build --workspace

# Entire workspace (release)
cargo build --workspace --release

# Compiler binary crate only
cargo build -p compiler

# Compiler binary crate only (release)
cargo build -p compiler --release

# Raw Rust compiler module for faustwasm embedded-compiler mode
cargo run -p xtask -- build-faustwasm-compiler-module
```

## Validate

Recommended local checks before committing:

```bash
cargo check --workspace --all-targets
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- golden-check
```

Use `cargo run -p xtask -- golden-check-cpp` for the long-run C++ parity target
when `tests/golden/cpp/` is expected to match the current Rust output.

## Install

```bash
# Install the `faust-rs` binary into Cargo's bin directory
cargo install --path crates/compiler
```

## Use faust-rs

```bash
# Run without installation (from the repository)
cargo run -p compiler

# Run the installed binary
faust-rs
```

DSP compilation examples:

```bash

# Generate AssemblyScript
faust-rs -lang asc foo.dsp

# Generate C
faust-rs -lang c foo.dsp

# Generate C++
faust-rs -lang cpp foo.dsp

# Generate experimental Cranelift backend report
faust-rs -lang cranelift foo.dsp

# Generate interpreter bytecode (.fbc)
faust-rs -lang interp foo.dsp
# alias: -lang interp-fbc
# shorthand flag: --dump-interp

# Dump FIR text IR
faust-rs -lang fir foo.dsp

# Generate Julia source
faust-rs -lang julia foo.dsp -o foo.jl

# Generate WebAssembly plus companion JSON
faust-rs -lang wasm foo.dsp -o foo.wasm

# Generate textual WAT/WAST from the same WASM backend
faust-rs -lang wast foo.dsp -o foo.wat

# Emit strict Faust JSON description
faust-rs --json foo.dsp

# Emit a backend artifact plus companion JSON next to the output path
faust-rs -lang cpp --json foo.dsp -o foo.cpp

# Convert interpreter bytecode (.fbc) to self-contained native C++
faust-rs --dump-cpp-from-fbc foo.fbc --cpp-class-name MyInterpDsp

# Generate block-diagram SVG files
faust-rs -svg foo.dsp

# Write output to a file
faust-rs -lang cpp foo.dsp -o foo.cpp
faust-rs -lang interp foo.dsp -o foo.fbc
```

Scheduling and vector code generation:

```bash
# Select a scheduling strategy in scalar or vector mode.
faust-rs -ss 0 foo.dsp                    # depth-first (default)
faust-rs --scheduling-strategy 1 foo.dsp  # breadth-first
faust-rs -ss 2 foo.dsp                    # special/interleaved
faust-rs -ss 3 foo.dsp                    # reverse breadth-first

# Request checked vector lowering with 64-sample chunks.
faust-rs -vec -vs 64 -lv 0 foo.dsp
faust-rs -vec -vs 64 -lv 1 foo.dsp
```

`-ss` accepts non-negative integers: `0`, `1`, and `2` select the strategies
shown above, while `3` and greater select reverse breadth-first. Missing,
negative, and non-integer values are hard errors; this is deliberately stricter
than the C++ compiler's `atoi` fallback. `-vec` defaults to `-vs 32 -lv 0`;
the supported loop variants are `-lv 0` (constant-trip main loop plus scalar
remainder) and `-lv 1` (runtime-bounded chunk loop).

faust-rs deliberately applies the same default `-ss 0` depth-first policy in
both scalar and vector modes. This differs from the C++ vector default, which
uses `CodeLoop::sortGraph`; `-ss 3` is the closest faust-rs match for that C++
levelization policy.

Built-in FIR backend fixtures (for backend debugging / bring-up):

```bash
# List internal FIR fixtures
faust-rs --list-fir-fixtures

# Dump a built-in FIR fixture
faust-rs --fir-fixture sine_phasor -lang fir

# Generate backend output directly from a built-in FIR fixture
faust-rs --fir-fixture control_flow -lang c
faust-rs --fir-fixture gain_bias_ui_meta -lang cpp
faust-rs --fir-fixture sine_phasor -lang interp
faust-rs --fir-fixture sine_phasor -lang cranelift
faust-rs --fir-fixture sine_phasor -lang julia
faust-rs --fir-fixture sine_phasor -lang wasm
```

Notes:

- `--fir-fixture` bypasses the Faust front-end pipeline and feeds a hand-written
  FIR module from `codegen::fixtures` directly into the selected backend.
- It is intended for backend debugging and parity bring-up, not end-user DSP
  compilation workflows.

If your installed command is named `faust` (for example via a symlink/wrapper),
the same model applies:

```bash
faust -lang asc foo.dsp
faust -lang c foo.dsp
faust -lang cpp foo.dsp
faust -lang cranelift foo.dsp
faust -lang fir foo.dsp
faust -lang interp foo.dsp
faust -lang julia foo.dsp
faust -lang wasm foo.dsp
faust -lang wast foo.dsp
```

Without installation (equivalent):

```bash
cargo run -p compiler -- -lang asc foo.dsp
cargo run -p compiler -- -lang c foo.dsp
cargo run -p compiler -- -lang cpp foo.dsp
cargo run -p compiler -- -lang cranelift foo.dsp
cargo run -p compiler -- -lang fir foo.dsp
cargo run -p compiler -- -lang interp foo.dsp
cargo run -p compiler -- -lang julia foo.dsp
cargo run -p compiler -- -lang wasm foo.dsp
cargo run -p compiler -- -lang wast foo.dsp

```

## Environment variables

Use the following variables to increase the evaluation depth stack:

`export FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH=XX` (default: 4096)
`export FAUST_RS_DEFAULT_EVAL_MAX_DEPTH=XX` (default: 1024)

## Documentation

- User CLI reference: `docs/user-cli-guide-en.md`
- User diagnostics guide: `docs/user-diagnostics-guide-en.md`
- Supported Faust subset: `porting/faust-rs-supported-faust-subset-en.md`
- Technical/developer workflows: `docs/developer-workflows-en.md`
- Code graphs and public API index: `docs/code-graphs/`
- Raw `faustwasm` compiler-module build notes: `crates/wasm-ffi/README.md`

## Workspace crates

| Crate | Role |
|---|---|
| `tlib` | Hash-consed tree arena, symbols, lists, recursive tree helpers |
| `errors` | Structured diagnostics model |
| `interval` | Interval arithmetic |
| `algebra` | Shared algebra/rewrite scaffold |
| `graph` | Shared graph algorithms scaffold |
| `boxes` | Faust box IR builders and matchers |
| `parser` | Faust source parser and import handling |
| `signals` | Faust signal IR builders, matchers, extended math nodes, and shared local RAD rule helpers |
| `ui` | Grouped UI IR |
| `eval` | Box-level evaluator and pattern matcher |
| `propagate` | Box-to-signal propagation, including FAD/RAD expansion |
| `normalize` | Signal normalization and preparation helpers |
| `sigtype` | Signal type lattice and inference |
| `transform` | Signal preparation and signal-to-FIR lowering |
| `fir` | Faust Intermediate Representation |
| `foreign-call` | Raw C ABI foreign-function invocation bridge |
| `codegen` | C, C++, interpreter, Cranelift, WASM, and Julia backend generation |
| `draw` | SVG block-diagram rendering |
| `doc` | Documentation/reporting scaffold |
| `utils` | Shared FFI utilities |
| `compiler` | Top-level compiler facade and CLI |
| `xtask` | Developer and CI automation |
| `interp-ffi` | Interpreter backend C/C++ API |
| `cranelift-ffi` | Experimental Cranelift backend C/C++ API |
| `box-ffi` | Box manipulation C/C++ API |
| `faust-ffi` | Unified `libfaust` distribution crate |
| `wasm-ffi` | Raw WASM ABI for `faustwasm` embedded compiler mode |

## Generate API docs

Generate Rustdoc for workspace crates only (recommended):

```bash
cargo doc --workspace --no-deps
```

Generate Rustdoc including dependencies:

```bash
cargo doc --workspace
```

Open the generated HTML entry point:

```bash
open target/doc/index.html
```

Crate-specific entry point example:

- `target/doc/compiler/index.html`

## Porting references

- Porting plan: `porting/faust-rust-porting-plan-en.md`
- Critical points: `porting/faust-rust-points-critiques-en.md`
- Phases: `porting/phases/`
- Supported Faust subset: `porting/faust-rs-supported-faust-subset-en.md`
- Porting journal index: `JOURNAL.md`
