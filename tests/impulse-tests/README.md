# faust-rs impulse-response tests

This is a faust-rs port of the C++ Faust `tests/impulse-tests` machinery. It
checks that faust-rs backends generate correct code by comparing the impulse
response of each test DSP against a **reference** produced by the genuine C++
Faust compiler.

See the design write-up in
[`porting/impulse-tests-harness-port-plan-2026-06-14-en.md`](../../porting/impulse-tests-harness-port-plan-2026-06-14-en.md).

## What it does

1. **Reference (the oracle).** `make reference` compiles every `dsp/*.dsp` with
   the C++ Faust compiler wrapped in the original 4-pass impulse architecture
   (`impulsearch.cpp` + `controlTools.h`), builds a native binary, and runs it
   for 60000 frames: impulse pass + random-split pass + polyphonic 4-voice pass
   + polyphonic 1-voice pass. Output goes to `reference/*.ir`.

2. **Backend checks.** Each backend target regenerates the impulse response with
   faust-rs and compares it to the reference with `tools/filesCompare`
   (tolerance `2e-06`):
   - `make cpp` / `make c` — faust-rs generates C++/C, it is wrapped in the
     *same* 4-pass architecture, compiled and run; the full 60000-frame output
     is compared **exactly**.
   - `make interp` — the faust-rs interpreter runs in-process via
     `impulse-runner`. It has no polyphonic/MIDI runtime, so it reproduces only
     the scalar impulse pass (first 15000 frames) and is compared as a **prefix**
     with `filesCompare -part` (same approach the C++ suite uses for its Rust
     target).
   - `make cranelift` — the faust-rs Cranelift JIT runs in-process via
     `impulse_cranelift` in 64-bit (`-double`), scalar prefix, `-part`.
   - `make wasm` — the faust-rs WASM backend is compiled to `.wasm + .json`
     and executed through Node's native WebAssembly runtime in 64-bit
     (`-double`), scalar prefix, `-part`.
   - `make assemblyscript` — the faust-rs AssemblyScript backend is compiled
     with `asc`, executed through Node's native WebAssembly runtime, and
     compared on the scalar prefix with `filesCompare -part`.
   - `make rust` — the faust-rs Rust backend output is appended to the
     `archs/impulserust.rs` architecture (host `Meta`/`UI` traits + the shared
     impulse/soundfile fixture), compiled natively with `rustc -O`, and
     compared on the scalar prefix with `filesCompare -part` in 64-bit
     (`-double`).

## Requirements

- A built faust-rs workspace: `make build` (runs
  `cargo build --release -p compiler -p impulse-runner`).
- A C++ Faust checkout for the reference oracle and the native C/C++ paths
  (architecture headers + `impulsearch.cpp`). Paths are configured in
  [`common.mk`](common.mk) and overridable:
  `CPP_TESTS`, `FAUST_ARCH`, `FAUST_CPP`, `FAUSTLIBS`.
- `c++` and the Faust standard libraries (default `/usr/local/share/faust`).
- Node.js for the WASM and AssemblyScript impulse runners.
- `rustc` (already required to build the workspace) for the Rust backend gate.
- `asc` (AssemblyScript compiler) on `PATH`, or `ASC=/path/to/asc`.

## Usage

```bash
cd tests/impulse-tests
make build         # build the faust-rs binaries the harness drives
make reference     # generate the reference .ir oracle  (run once)
make interp        # check the interpreter backend
make cpp           # check the C++ backend
make c             # check the C backend
make cranelift     # check the Cranelift JIT backend (64-bit)
make wasm          # check the WASM backend (64-bit scalar prefix)
make assemblyscript # check the AssemblyScript backend (scalar prefix)
make rust          # check the Rust backend (scalar prefix, rustc)
make bench         # compare C++ Faust and faust-rs performance with faustbench -single
make compile-bench # compare C++ Faust and faust-rs compile time
make all           # cpp + c + interp + cranelift + wasm + assemblyscript + rust
make -k -j8 cpp    # parallel, keep going past failures
make help          # list targets and variables
make clean         # remove ir/ and build/
```

There is no `reference` rebuild on every run: delete `reference/` (or
`make distclean`) to regenerate.

## Layout

| Path | Purpose |
|---|---|
| `dsp/` | 93 test DSP programs (from the C++ suite) |
| `common.mk` | shared, overridable configuration |
| `known.mk` | per-DSP tolerances + known-failure exclusion lists |
| `KNOWN_FAILURES.md` | documented gaps/tolerances with causes |
| `Make.ref` | genuine C++ 4-pass reference generation |
| `Make.gcc` | faust-rs C / C++ backends (full 4-pass, exact compare) |
| `Make.interp` | faust-rs interpreter backend (scalar prefix, `-part`) |
| `Make.cranelift` | faust-rs Cranelift JIT backend (scalar prefix, 64-bit, `-part`) |
| `Make.wasm` | faust-rs WASM backend (scalar prefix, 64-bit, Node WebAssembly, `-part`) |
| `Make.assemblyscript` | faust-rs AssemblyScript backend (scalar prefix, `asc` + Node WebAssembly, `-part`) |
| `Make.rust` | faust-rs Rust backend (scalar prefix, `rustc`, `-part`) |
| `Make.bench` | generated-code performance comparison with `faustbench -single` |
| `tools/filesCompare.cpp` | the comparator |
| `tools/impulsewasm.js` | Node WebAssembly scalar impulse runner |
| `tools/impulseasc.js` | AssemblyScript/Node scalar impulse runner |
| `archs/impulserust.rs` | Rust scalar impulse architecture (host traits + fixture + driver) |
| `reference/`, `ir/`, `build/` | generated, gitignored |

## Status

Raw sweep over the 93 DSPs at the default `2e-06` tolerance:

| Backend | Match | Mismatch | Compile-fail |
|---|---|---|---|
| C++ (full 4-pass, exact) | **92** | 0 | 1 (`subcontainer1`) |
| C (full 4-pass, exact) | **92** | 0 | 1 (`subcontainer1`) |
| interpreter (scalar prefix, `-part`) | **92** | 0 | 1 (`subcontainer1`) |
| Cranelift JIT (scalar prefix, `-part`, 64-bit) | **92** | 0 | 1 (`subcontainer1`) |
| WASM (scalar prefix, `-part`, 64-bit, Node) | **92** | 0 | 1 (`subcontainer1`) |
| AssemblyScript (scalar prefix, `-part`, `asc` + Node) | **92** | 0 | 1 (`subcontainer1`) |
| Rust (scalar prefix, `-part`, `rustc`) | **92** | 0 | 1 (`subcontainer1`) |

The C++ backend reproduces the full 60000-frame reference exactly on 92/93 DSPs,
so the remaining mismatches are backend-specific divergences the harness
pinpoints. Each was classified by its *max* delta and either given a per-DSP
tolerance (bounded rounding) or listed as a known failure (real gap) in
[`known.mk`](known.mk) / [`KNOWN_FAILURES.md`](KNOWN_FAILURES.md). With those
applied, the aggregate targets are **green gates**: `make cpp` (92), `make c`
(92), `make cranelift` (92), `make interp` (92), `make wasm` (92),
`make assemblyscript` (92), and `make rust` (92) build and pass; excluded cases
are documented in `known.mk` to fix later.

## Performance Bench

`make bench` runs the impulse DSP corpus through `faustbench -single` twice:
once with `FAUST_CPP` and once with `FAUST_RS`. Because `faustbench` finds a
binary named `faust` on `PATH`, the target creates temporary wrappers under
`build/bench/` and writes:

- `build/bench/summary.csv` — DSP name, C++ Faust throughput, faust-rs
  throughput, and relative delta.
- `build/bench/logs/*.log` — full `faustbench` output for each compiler.

The default precision option is `BENCH_OPTIONS=-double`; the recipe also passes
`-I dsp -I $(FAUSTLIBS)`. Override as needed:

```bash
make bench BENCH_OPTIONS="-double -run 3" BENCH_WARN_MIN=10
```

`make compile-bench` measures compiler wall-clock time on the same corpus. It
generates C++ with `-lang cpp -double` through both `FAUST_CPP` and
`FAUST_RS`, writes generated sources under `build/bench/compile/`, and records:

- `build/bench/compile-summary.csv` — DSP name, C++ Faust compile time,
  faust-rs compile time, and relative delta.
- `build/bench/logs/*.compile.*.log` — compiler stderr plus high-resolution
  wall-clock timing output.
