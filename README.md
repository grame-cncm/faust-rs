# faust-rs

Rust workspace for the Faust compiler port.

> ⚠️ **Experimental — work in progress.** faust-rs is a research port of the
> Faust compiler to Rust. It is not ready for production use, and its APIs and
> behavior may change at any time.

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

## Build the compiler for WebAssembly

The `wasm-ffi` crate packages the Faust compiler itself as a raw WebAssembly
module. This is different from `faust-rs -lang wasm`, which compiles one Faust
DSP to WebAssembly.

Install the Rust target and run the verified packaging workflow:

```bash
rustup target add wasm32-unknown-unknown
cargo run -p xtask -- build-faustwasm-compiler-module
```

The release module is published as:

```text
target/wasm32-unknown-unknown/release/libfaust-rs.wasm
```

The workflow builds the module with a 16 MiB WebAssembly stack, renames Cargo's
crate-normalized `faust_wasm_ffi.wasm` artifact to `libfaust-rs.wasm`, and
verifies the exported compiler ABI. The equivalent unverified manual build and
rename is:

```bash
cargo build -p wasm-ffi --target wasm32-unknown-unknown --release
mv target/wasm32-unknown-unknown/release/faust_wasm_ffi.wasm \
   target/wasm32-unknown-unknown/release/libfaust-rs.wasm
```

A Web host can load `libfaust-rs.wasm` with the standard `WebAssembly` API and
call its handle-based `faust_wasm_*` exports to compile Faust source strings to
DSP WebAssembly and JSON artifacts. See the
[`wasm-ffi` guide](crates/wasm-ffi/README.md) for the raw allocation, result,
and lifetime contract, plus options for embedding Faust libraries.

## Use `libfaust-rs` from C and C++

The `faust-ffi` crate builds one unified C ABI library named `libfaust-rs`, with
C++ wrappers over the same ABI. It exports the factory and DSP APIs for both the
bytecode Interpreter and the experimental native Cranelift JIT backend:

```bash
cargo run -p xtask -- build-libfaust --release
```

This produces the platform static library (`libfaust-rs.a` or `faust-rs.lib`)
and dynamic library (`libfaust-rs.dylib`, `libfaust-rs.so`, or `faust-rs.dll`) under
`target/release/`. The C++ headers are
`crates/interp-ffi/include/interpreter-dsp.h` and
`crates/cranelift-ffi/include/cranelift-dsp.h`; the corresponding C headers are
`interpreter-dsp-c.h` and `cranelift-dsp-c.h` in the same directories. The C++
wrappers also use the standard Faust architecture headers, so add the Faust
`architecture/` directory to the include path. For example:

```bash
c++ -std=c++17 app.cpp \
  -I crates/interp-ffi/include \
  -I crates/cranelift-ffi/include \
  -I /path/to/faust/architecture \
  -L target/release -lfaust-rs \
  -Wl,-rpath,"$PWD/target/release" \
  -o app
```

### Interpreter API

Create an `interpreter_dsp_factory` from a Faust source file or string, create a
standard Faust `dsp` instance from it, then initialize and process audio:

```cpp
#include "interpreter-dsp.h"

#include <array>
#include <iostream>
#include <memory>
#include <string>

int main()
{
    std::string error;
    auto* factory = createInterpreterDSPFactoryFromString(
        "gain", "process = _ * 0.5;", 0, nullptr, error);
    if (!factory) {
        std::cerr << error << '\n';
        return 1;
    }

    {
        std::unique_ptr<dsp> processor(factory->createDSPInstance());
        if (!processor) {
            deleteInterpreterDSPFactory(factory);
            return 1;
        }

        processor->init(48000);
        std::array<FAUSTFLOAT, 64> input{};
        std::array<FAUSTFLOAT, 64> output{};
        FAUSTFLOAT* inputs[] = {input.data()};
        FAUSTFLOAT* outputs[] = {output.data()};
        processor->compute(64, inputs, outputs);
    } // Destroy the DSP instance before its factory.

    deleteInterpreterDSPFactory(factory);
}
```

Use `createInterpreterDSPFactoryFromFile(...)` to compile a `.dsp` file. The
Interpreter API also provides `readInterpreterDSPFactoryFromBitcodeFile(...)`
and `writeInterpreterDSPFactoryToBitcodeFile(...)` for `.fbc` files.

### Cranelift API

The Cranelift wrapper exposes the same `dsp` lifecycle, so only the header and
factory functions change in the example above:

```cpp
#include "cranelift-dsp.h"

#include <iostream>
#include <memory>
#include <string>

int main()
{
    std::string error;
    auto* factory = createCraneliftDSPFactoryFromString(
        "gain", "process = _ * 0.5;", 0, nullptr, error, 2);
    if (!factory) {
        std::cerr << error << '\n';
        return 1;
    }

    {
        std::unique_ptr<dsp> processor(factory->createDSPInstance());
        if (!processor) {
            deleteCraneliftDSPFactory(factory);
            return 1;
        }

        processor->init(48000);
        // Use processor->compute(...) exactly as in the Interpreter example.
    } // Destroy the DSP instance before its factory.

    deleteCraneliftDSPFactory(factory);
}
```

The final `opt_level` argument is optional and defaults to `0`; values `1` and
`2` currently select speed optimization, while values of `3` or more select
speed-and-size optimization. `createCraneliftDSPFactoryFromFile(...)` is the
file-based equivalent.

### C API

The C API uses opaque factory and instance pointers and does not require the
Faust architecture headers. This complete Interpreter example follows the same
factory-before-instance destruction order as the C++ example:

```c
#include "interpreter-dsp-c.h"

#include <stdio.h>

int main(void)
{
    char error[4096] = {0};
    interpreter_dsp_factory* factory =
        createCInterpreterDSPFactoryFromString(
            "gain", "process = _ * 0.5;", 0, NULL, error);
    if (factory == NULL) {
        fprintf(stderr, "%s\n", error);
        return 1;
    }

    interpreter_dsp* processor = createCInterpreterDSPInstance(factory);
    if (processor == NULL) {
        deleteCInterpreterDSPFactory(factory);
        return 1;
    }

    initCInterpreterDSPInstance(processor, 48000);
    FAUSTFLOAT input[64] = {0};
    FAUSTFLOAT output[64] = {0};
    FAUSTFLOAT* inputs[] = {input};
    FAUSTFLOAT* outputs[] = {output};
    computeCInterpreterDSPInstance(processor, 64, inputs, outputs);

    deleteCInterpreterDSPInstance(processor);
    deleteCInterpreterDSPFactory(factory);
}
```

Compile it against the same unified library:

```bash
cc -std=c11 app.c \
  -I crates/interp-ffi/include \
  -L target/release -lfaust-rs \
  -Wl,-rpath,"$PWD/target/release" \
  -o app
```

The Cranelift C API has the same lifecycle with backend-specific names:

| Operation | Interpreter C API | Cranelift C API |
|---|---|---|
| Header | `interpreter-dsp-c.h` | `cranelift-dsp-c.h` |
| Factory from string | `createCInterpreterDSPFactoryFromString(..., error)` | `createCCraneliftDSPFactoryFromString(..., error, opt_level)` |
| Factory from file | `createCInterpreterDSPFactoryFromFile(..., error)` | `createCCraneliftDSPFactoryFromFile(..., error, opt_level)` |
| Create instance | `createCInterpreterDSPInstance(factory)` | `createCCraneliftDSPInstance(factory)` |
| Initialize | `initCInterpreterDSPInstance(dsp, sample_rate)` | `initCCraneliftDSPInstance(dsp, sample_rate)` |
| Process | `computeCInterpreterDSPInstance(...)` | `computeCCraneliftDSPInstance(...)` |
| Delete instance | `deleteCInterpreterDSPInstance(dsp)` | `deleteCCraneliftDSPInstance(dsp)` |
| Delete factory | `deleteCInterpreterDSPFactory(factory)` | `deleteCCraneliftDSPFactory(factory)` |

Unlike the C++ wrapper, the Cranelift C constructor always takes the
`opt_level` argument. Returned strings such as factory JSON or serialized
factory data are owned by `libfaust-rs` and must be released with `freeCMemory()`
when the corresponding header says so.

Cranelift support is experimental: native JIT execution works for the currently
supported compiler/FIR subset, but full runtime parity and its serialized
factory format are not yet final. Always check the returned factory and report
the supplied error string. See the detailed
[`Interpreter C/C++ API guide`](crates/interp-ffi/README.md) and
[`Cranelift C/C++ API guide`](crates/cranelift-ffi/README.md), as well as the
corresponding `*-dsp-c.h` headers when calling `libfaust-rs` from C.

## Install

```bash
# Install the `faust-rs` binary into Cargo's bin directory
cargo install --path crates/compiler
```

## Use faust-rs

```bash
# Show the CLI without installation (from the repository)
cargo run -p compiler -- --help

# Show the installed binary's CLI
faust-rs --help

# Build/run the optional native HTTP(S) source support (still runtime-off by default)
cargo run -p compiler --features network-imports -- \
  --allow-network-imports -lang cpp https://example.test/main.dsp
```

The feature and the CLI flag are deliberately separate opt-ins: compiling with
`network-imports` does not enable network access by itself. Once enabled, the
CLI accepts a direct HTTP(S) DSP source, explicit URL imports, relative imports
inside a remote source graph, and a remote main architecture passed with `-a`.
Requests and redirects are bounded and URL credentials are rejected. Server or
embedded hosts should inject their own fetcher and URL authorization policy
instead of using the unrestricted native convenience transport. See the
[`compiler` HTTP(S) API documentation](crates/compiler/README.md#https-sources-and-architectures)
and the [`--allow-network-imports` CLI reference](docs/user-cli-guide-en.md#--allow-network-imports).
The browser compiler module remains network-free but accepts asynchronously
prefetched URL/source graphs as documented in the
[`wasm-ffi` README](crates/wasm-ffi/README.md#prefetched-https-source-graphs).

DSP compilation examples:

```bash

# Use the project-local Faust libraries (optimizers.lib and interleave.lib)
faust-rs -I libraries -lang cpp foo.dsp

# Generate AssemblyScript
faust-rs -lang asc foo.dsp

# Generate C
faust-rs -lang c foo.dsp

# Generate a scalar Cmajor processor
faust-rs -lang cmajor foo.dsp -o foo.cmajor

# Generate RNBO codebox source
faust-rs -lang codebox foo.dsp -o foo.codebox

# Generate codebox with the RNBO wrapper's `RB_` parameter names
faust-rs -lang codebox-test foo.dsp -o foo.codebox

# Generate C++
faust-rs -lang cpp foo.dsp

# Generate C++ with mode-zero host-managed buffers plus layout/cost JSON
faust-rs -lang cpp -mem0 -json foo.dsp -o foo.cpp

# The same scalar mode is available for C and native Cranelift
faust-rs -lang c -mem0 foo.dsp -o foo.c
faust-rs -lang cranelift -mem0 -json foo.dsp -o foo.cranelift

# Generate experimental Cranelift backend report
faust-rs -lang cranelift foo.dsp

# Generate interpreter bytecode (.fbc)
faust-rs -lang interp foo.dsp -o foo.fbc

# Dump FIR text IR
faust-rs -lang fir foo.dsp

# Generate Julia source
faust-rs -lang julia foo.dsp -o foo.jl

# Generate Rust source
faust-rs -lang rust foo.dsp -o foo.rs

# Generate WebAssembly plus companion JSON
faust-rs -lang wasm foo.dsp -o foo.wasm

# Generate textual WAT/WAST from the same WASM backend
faust-rs -lang wast foo.dsp -o foo.wat

# Emit strict Faust JSON description
faust-rs --json foo.dsp

# Emit a backend artifact plus companion JSON next to the output path
faust-rs -lang cpp --json foo.dsp -o foo.cpp

# Generate block-diagram SVG files
faust-rs -svg foo.dsp

# Write output to a file
faust-rs -lang cpp foo.dsp -o foo.cpp
faust-rs -lang interp foo.dsp -o foo.fbc
```

`-mem`, `-mem0`, `--memory-manager`, and `--memory-manager0` are equivalent.
Only mode zero is currently supported, for scalar C, C++, and Cranelift. Its
JSON companion includes a versioned target layout and `compute_cost`; see the
[`-mem0` custom memory manager guide](docs/user-mem0-guide-en.md) for how to
write a custom allocator against it, the
[porting analysis](porting/custom-memory-manager-mem0-analysis-and-porting-plan-2026-08-13-en.md)
for the full design rationale, and the
[Cranelift C/C++ API guide](crates/cranelift-ffi/README.md).

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

# Execution options (c/cpp/rust/fir/asc backends): external control and
# one-sample processing, as C++ Faust.
faust-rs -ec foo.dsp            # separate host-scheduled control() function
faust-rs -os foo.dsp            # one-sample frame(inputs, outputs) API
faust-rs -ec -os foo.dsp        # control() once, then frame() per sample
faust-rs -ec -vec foo.dsp       # external control with vector compute
```

The RNBO `codebox` target is inherently external-control and one-sample: its
output is identical with or without `-ec` and `-os`. It rejects `-vec`.
`-lang codebox-test` changes only parameter names to the `RB_` convention used
by Faust's `rnbo-dsp.h` wrapper for manual RNBO round-trips. The backend has
in-tree structural and numeric tests; the RNBO import/export validation remains
manual because the RNBO SDK is not part of this workspace.

The scalar `cmajor` target has the same intrinsic execution shape: event-based
external control and one sample per processor tick, independent of `-ec` and
`-os`; it rejects `-vec`. It supports the default single precision and
`-double`, `-cn`, UI events,
rate-limited bargraphs, tables, `--json`, and `-a` architecture wrapping.
Polyphonic and Cmajor SDK application tooling remain deferred.

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
faust-rs --fir-fixture gain_bias_ui_meta -lang cranelift
faust-rs --fir-fixture sine_phasor -lang julia
faust-rs --fir-fixture gain_bias_ui_meta -lang codebox
faust-rs --fir-fixture gain_bias_ui_meta -lang wasm
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
faust -lang cmajor foo.dsp
faust -lang codebox foo.dsp
faust -lang codebox-test foo.dsp
faust -lang cpp foo.dsp
faust -lang cranelift foo.dsp
faust -lang fir foo.dsp
faust -lang interp foo.dsp
faust -lang julia foo.dsp
faust -lang rust foo.dsp
faust -lang wasm foo.dsp
faust -lang wast foo.dsp
```

Without installation (equivalent):

```bash
cargo run -p compiler -- -lang asc foo.dsp
cargo run -p compiler -- -lang c foo.dsp
cargo run -p compiler -- -lang cmajor foo.dsp
cargo run -p compiler -- -lang codebox foo.dsp
cargo run -p compiler -- -lang codebox-test foo.dsp
cargo run -p compiler -- -lang cpp foo.dsp
cargo run -p compiler -- -lang cranelift foo.dsp
cargo run -p compiler -- -lang fir foo.dsp
cargo run -p compiler -- -lang interp foo.dsp
cargo run -p compiler -- -lang julia foo.dsp
cargo run -p compiler -- -lang rust foo.dsp
cargo run -p compiler -- -lang wasm foo.dsp
cargo run -p compiler -- -lang wast foo.dsp

```

## Clock domains and automatic differentiation

`faust-rs` adds clock-domain and automatic-differentiation primitives to the
Faust language. They are compiler extensions: do not expect the C++ Faust
reference compiler to accept them.

- `ondemand(C)` (**OD**) runs `C` only when its clock input fires; it holds the
  last computed value between firings.
- `upsampling(C)` (**US**) runs `C` at an increased local rate.
- `downsampling(C)` (**DS**) runs `C` at a reduced local rate.
- `fad(expr, seeds)` emits every primal output followed by one forward-mode
  tangent per seed lane. It is useful when a local derivative is consumed
  directly inside the DSP.
- `rad(expr, seeds)` emits the primal outputs followed by reverse-mode
  gradients for the sum of the primals. Delays and recursion use a
  block-local reverse sweep, which is particularly useful for host-driven
  optimization.

Inside a `US` or `DS` block, `ma.SR` is adapted automatically to the local
clock domain: an upsampling factor `H` makes the block observe `SR * H`, while
a downsampling factor `H` makes it observe `SR / H`. Filters, oscillators, and
other algorithms that derive their coefficients from `ma.SR` therefore use the
effective sample rate of the block without requiring a manual correction.

There is currently one practical limitation in the Faust libraries:
`platform.lib` defines `ma.SR` with an upper and lower clamp equivalent to
`min(192000, max(1, fSamplingFreq))`. The compiler adapts the sample-rate value
inside `US`/`DS`, but the generated expression retains that surrounding
`min`/`max`. Consequently, a large `US` factor can still clamp the effective
local rate to 192 kHz—for example, `US(8)` at 48 kHz should observe 384 kHz but
currently observes 192 kHz through this definition. The `ma.SR` definition in
`platform.lib` must therefore be relaxed or redesigned before large
upsampling factors can expose their full local sample rate.

See [the clock-domain note](docs/ondemand-note-en.md) for OD/US/DS timing and
rate semantics, and [the FAD/RAD synthesis](docs/fad-rad-synthesis-en.md) for
output layouts, examples, and current limits.

Project-local Faust helpers live in [`libraries/`](libraries/README.md). Add
that directory to the import path with `-I libraries` when a DSP imports
`optimizers.lib` or loads `interleave.lib`.

## Frame-rate FFT and spectral processing

`libraries/interleave.lib` combines frame serialization with `ondemand` so an
`N`-point FFT, spectral effect, and inverse FFT run once per frame or hop rather
than once per audio sample. This supports analysis-only FFTs, spectral masks,
fast convolution, overlap-add STFT effects, phase-vocoder state, and
differentiable spectral losses in ordinary Faust graphs.

The current compiler expands the FFT into a specialized scalar butterfly graph:
this works well for small and medium transforms, but large FFTs increase
compilation time, generated-code size, instruction-cache pressure, and the
worst-case work performed on a frame tick. See
[the clock-domain and spectral-processing note](docs/ondemand-note-en.md#5-spectral-processing-with-ondemand-and-interleavelib)
for executable DSP examples, framing semantics, and a comparison with optimized
FFT implementations.

## Environment variables

Use the following variables to increase the evaluation depth stack:

- `export FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH=XX` (default: 4096)
- `export FAUST_RS_DEFAULT_EVAL_MAX_DEPTH=XX` (default: 1024)

## Documentation

- [User CLI reference](docs/user-cli-guide-en.md)
- [User diagnostics guide](docs/user-diagnostics-guide-en.md)
- [`-mem0` custom memory manager guide](docs/user-mem0-guide-en.md)
- Clock domains (`ondemand`/`upsampling`/`downsampling`): [English](docs/ondemand-note-en.md) / [French](docs/ondemand-note-fr.md)
- Automatic differentiation (`fad`/`rad`): [English](docs/fad-rad-synthesis-en.md) / [French](docs/fad-rad-synthesis-fr.md)
- [Supported Faust subset](porting/faust-rs-supported-faust-subset-en.md)
- [Technical/developer workflows](docs/developer-workflows-en.md)
- [Concise porting history](docs/faust-cpp-to-rust-port-history-overview-en.md)
- [Detailed technical porting history](docs/faust-cpp-to-rust-port-history-en.md)
- [Code graphs and public API index](docs/code-graphs/)
- [`faustwasm` compiler-module build notes](crates/wasm-ffi/README.md)

## Contributing

Pull requests are welcome, including contributions developed with AI
assistance. Human-authored and AI-assisted changes are held to the same
standards, and contributors remain responsible for the correctness,
maintainability, tests, and documentation of the submitted work.

Every pull request must follow
[Commit and Documentation Hygiene](AGENTS.md#11-commit-and-documentation-hygiene).
In particular, keep the Git history linear, submit small and coherent commits,
update the README and porting journal when required, and keep documentation
concise, factual, and implementation-oriented.

## Workspace crates

The workspace follows a one-way dependency rule:
`compiler core <- FFI adapters <- distribution`. Run
`cargo run -p xtask -- ffi-boundary-check` to verify that no dependency points
rightward and that unsafe-code opt-ins remain confined to the explicit FFI
boundary plus the `foreign-call` runtime bridge.

### Compiler core

| Crate | Role |
|---|---|
| `tlib` | Hash-consed tree arena, symbols, lists, recursive tree helpers |
| `diagnostics` | Structured diagnostic reports, stable codes, labels, and bundles |
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
| `codegen` | AssemblyScript, C, C++, Cmajor, Codebox (RNBO), Rust, interpreter, Cranelift, WASM, and Julia backend generation |
| `draw` | SVG block-diagram rendering |
| `doc` | Documentation/reporting scaffold |
| `compiler` | Top-level compiler facade and CLI |

### FFI adapters

| Crate | Role |
|---|---|
| `ffi-common` | Shared ABI, marshalling, allocation, and factory-cache support for FFI adapters |
| `tree-ffi` | Shared opaque tree-handle support for Box and Signal C APIs |
| `box-ffi` | Box manipulation C/C++ API |
| `signal-ffi` | Signal manipulation C/C++ API |
| `interp-ffi` | Interpreter backend C/C++ API |
| `cranelift-ffi` | Experimental Cranelift backend C/C++ API |

### Distribution and tooling

| Crate | Role |
|---|---|
| `impulse-runner` | Interpreter-backed scalar impulse-test runner |
| `xtask` | Developer and CI automation |
| `faust-ffi` | Unified `libfaust-rs` distribution crate |
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

- [Porting plan](porting/faust-rust-porting-plan-en.md)
- [Critical points](porting/faust-rust-points-critiques-en.md)
- [Porting phases](porting/phases/)
- [Supported Faust subset](porting/faust-rs-supported-faust-subset-en.md)
- [Porting journal index](JOURNAL.md)
