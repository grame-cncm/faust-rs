# faust-rs impulse-response tests

This is a faust-rs port of the C++ Faust `tests/impulse-tests` machinery. It
checks that faust-rs backends generate correct code by comparing the impulse
response of each test DSP against a **reference** produced by the genuine C++
Faust compiler.

See the design write-up in
[`porting/impulse-tests-harness-port-plan-2026-06-14-en.md`](../../porting/impulse-tests-harness-port-plan-2026-06-14-en.md).

## What it does

1. **Reference (the oracle).** `make reference` first preflights every
   `dsp/*.dsp` with the configured C++ Faust compiler. DSPs it rejects are
   recorded as unavailable C++ oracles, with one diagnostic log per DSP; they
   are excluded from differential targets without being classified as faust-rs
   failures. It then compiles each supported DSP with the C++ Faust compiler
   wrapped in the original 4-pass impulse architecture
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
   - `make rust` — the faust-rs Rust backend is appended to the native Rust
     impulse architecture, compiled with `rustc -O`, and compared on the
     scalar prefix with `filesCompare -part` in 64-bit (`-double`) mode.
   - `make julia` — the faust-rs Julia backend is appended to a self-contained
     Julia impulse runtime, run by `julia`, and compared on the scalar prefix
     with `filesCompare -part` in 64-bit (`-double`) mode.
   - `make cmajor` — faust-rs emits scalar-double Cmajor, `cmaj generate
     --target=cpp` produces a native class, and the pinned C++ Faust
     `cmajor_cpp_dsp` adapter runs the upstream impulse protocol. Each DSP uses
     a private build directory, so this external lane supports `make -j`. It is
     intentionally outside `make all` and the vector/scheduling matrix.
   - `make <backend>-vec0` / `make <backend>-vec1` — run the same backend with
     `-vec -lv 0` or `-vec -lv 1` respectively. Available for `cpp`, `c`,
     `interp`, `cranelift`, `wasm`, `assemblyscript`, `rust`, and `julia`; `make all-vec` runs
     both vector loop variants across all backends.
   - `make <backend>-ssN` / `make <backend>-vecL-ssN` — cross scalar mode or
     vector loop variant `L` with scheduling strategy `N`. `make all-ss` runs
     scalar `-ss 0..3`, `make all-vec-ss` runs `-lv 0/1 x -ss 0..3`, and
     `make backend-matrix` runs all 96 backend/mode/strategy combinations.
   - `make <backend>-ec` / `-os` / `-ec-os` — the execution options, for the
     backends whose capability table marks them Explicit and that have a
     runnable impulse target: `cpp`, `c` and `rust`. `make all-execopts` runs
     all nine. These need their own driver, because the reference architecture
     only ever calls `compute(count, ...)`: under `-ec` the block-rate work
     lives in a separate `control()`, and under `-os` the canonical `compute` is
     deliberately empty while the sample body sits in `frame()`. A driver that
     ignored those entry points would measure silence, or uninitialized slow
     values, and blame the compiler for it.

     Each shape is compared against the **same** classic `reference/*.ir` as
     every other target, on the scalar prefix. That is the property worth
     checking: selecting an execution option must not move the impulse response
     by one bit. `crates/compiler/tests/execution_options.rs` already locks the
     emitted *signatures*; these targets are what checks the numbers.

     Unlike the classic `cpp`/`c` targets, they are **self-contained**: the
     architecture surface they need lives in `archs/faust_minimal.h` and
     `archs/faust_minimal_cglue.h`, so they build with no C++ Faust
     `architecture/` tree — verified by running them with `FAUST_ARCH` and
     `CPP_TESTS` pointed at nonexistent paths. Producing `reference/*.ir` in the
     first place still needs the C++ oracle, exactly as for every other
     target.
   - `make cpp-mem0` / `c-mem0` / `cranelift-mem0` — mode-zero custom
     memory-manager lanes over every `dsp/` input supported by both the C++
     oracle and the selected backend. Each lane compares the managed 15,000
     frame prefix with `reference/*.ir` and validates the version-2 memory JSON
     plus `compute_cost`. `make all-mem0` runs all three full-corpus lanes,
     requires identical FIR costs across backends, then runs the focused
     `dsp-mem0/` audit. That audit poisons allocations, reconciles descriptions,
     allocations, reverse destruction and leaks, and compares C/C++ `-O0`/`-O3`
     with Cranelift optimization levels 0/3. `make mem0-smoke` runs only this
     self-contained audit; `make mem0-opt` reruns its optimization matrix;
     `make mem0-sanitize` runs its C/C++ lanes under ASan and UBSan where the
     host toolchain supports them.

## Requirements

- A built faust-rs workspace: `make build` (builds `compiler`,
  `impulse-runner`, `impulse_cranelift`, and the mem0 JSON checker in release
  mode).
- A C++ Faust checkout for the reference oracle and the native C/C++ paths
  (architecture headers + `impulsearch.cpp`). Paths are configured in
  [`common.mk`](common.mk) and overridable:
  `CPP_TESTS`, `FAUST_ARCH`, `FAUST_CPP`, `FAUSTLIBS`.
- `c++` and the Faust standard libraries (default `/usr/local/share/faust`).
- The full `*-mem0` and `all-mem0` targets use the standard `dsp/` reference
  corpus and therefore have the same C++ oracle and Faust-library requirements
  as the ordinary impulse lanes. `mem0-smoke`, the three `*-mem0-smoke`
  targets, `mem0-opt`, and `mem0-sanitize` use only the import-free
  `dsp-mem0/` fixtures.
- Node.js for the WASM and AssemblyScript impulse runners.
- `rustc` (already required to build the workspace) for the Rust backend gate.
- Julia with the `StaticArrays` package for the Julia backend gate.
- Cmajor's `cmaj` command for the optional Cmajor backend gate; override its
  path with `CMAJ_BIN=/path/to/cmaj` and its C++ compiler with `CMAJ_CXX`.
- `asc` (AssemblyScript compiler) on `PATH`, or `ASC=/path/to/asc`.
- The Node runners use a 600-second compiler timeout so heavily parallel
  backend-matrix runs do not inherit the interactive CLI's 120-second limit.
  Override it with
  `FAUST_RS_TIMEOUT_SECONDS=<positive-seconds>`.

## Usage

```bash
cd tests/impulse-tests
make build         # build the faust-rs binaries the harness drives
make reference     # generate the reference .ir oracle  (run once)
make reference-report # show C++-oracle-supported and rejected DSPs
make interp        # check the interpreter backend
make cpp           # check the C++ backend
make c             # check the C backend
make cranelift     # check the Cranelift JIT backend (64-bit)
make wasm          # check the WASM backend (64-bit scalar prefix)
make assemblyscript # check the AssemblyScript backend (scalar prefix)
make rust          # check the Rust backend (scalar prefix, rustc)
make julia         # check the Julia backend (scalar prefix, Julia)
make cmajor        # check scalar Cmajor via cmaj-generated C++
make all-mem0      # full dsp/ corpus on C, C++, Cranelift, then focused audit
make mem0-smoke    # focused allocation/JSON/O0-O3 audit on dsp-mem0/
make mem0-opt      # rerun the focused C/C++/Cranelift -mem0 O0/O3 matrix
make mem0-sanitize # audit C/C++ -mem0 ownership with ASan and UBSan
make cpp-vec0      # check the C++ backend with -vec -lv 0
make cpp-vec1      # check the C++ backend with -vec -lv 1
make all-vec       # check -vec -lv 0 and -vec -lv 1 across all backends
make cpp-ss2       # check scalar C++ with scheduling strategy 2
make cpp-vec1-ss3  # check C++ with -vec -lv 1 -ss 3
make backend-matrix-smoke # run the representative backend matrix corpus
make backend-matrix       # run all 96 backend/mode/strategy combinations
make -j8 backend-matrix-full # fresh full matrix plus the audited report
make bench         # compare C++ Faust and faust-rs performance with faustbench -single
make bench-self-test # validate benchmark ordering, statuses, and aggregate calculations
make vec-bench     # compare scalar/vec0/vec1 C++ throughput under -ss 0..3 for checked vector DSPs
make compile-bench # compare C++ Faust and faust-rs compile time
make all           # cpp + c + interp + cranelift + wasm + assemblyscript + rust
make -k -j8 cpp    # parallel, keep going past failures
make help          # list targets and variables
make clean         # remove ir/ and build/
```

Differential backend targets invoke `make reference` first. The C++ oracle
preflight is cached and only reruns when the selected corpus or oracle
configuration changes; ordinary reference responses still follow normal Make
timestamp rules. Delete `reference/` (or `make distclean`) to regenerate all
supported responses.

## Layout

| Path | Purpose |
|---|---|
| `dsp/` | 133 test DSP programs used by the ordinary and full `mem0` lanes |
| `common.mk` | shared, overridable configuration |
| `known.mk` | per-DSP tolerances + faust-rs known-failure exclusion lists |
| `build/ref/cpp-oracle-manifest.mk` | generated C++ oracle support classification |
| `KNOWN_FAILURES.md` | documented gaps, tolerances, and oracle exclusions |
| `Make.ref` | genuine C++ 4-pass reference generation |
| `Make.gcc` | faust-rs C / C++ backends (full 4-pass, exact compare) |
| `Make.interp` | faust-rs interpreter backend (scalar prefix, `-part`) |
| `Make.cranelift` | faust-rs Cranelift JIT backend (scalar prefix, 64-bit, `-part`) |
| `Make.wasm` | faust-rs WASM backend (scalar prefix, 64-bit, Node WebAssembly, `-part`) |
| `Make.assemblyscript` | faust-rs AssemblyScript backend (scalar prefix, `asc` + Node WebAssembly, `-part`) |
| `dsp-mem0/` | focused import-free memory-manager audit corpus (state/UI, delays, tables) |
| `Make.mem0` | full-corpus and focused C/C++/Cranelift `-mem0` runtime/JSON gates |
| `archs/faust_mem0*.h`, `archs/impulsemem0*.cpp` | audited custom-manager contracts and native drivers |
| `Make.bench` | generated-code performance comparison with `faustbench -single` |
| `tools/filesCompare.cpp` | the comparator |
| `tools/impulsewasm.js` | Node WebAssembly scalar impulse runner |
| `tools/impulseasc.js` | AssemblyScript/Node scalar impulse runner |
| `reference/`, `ir/`, `build/` | generated, gitignored |

## Status

Historical raw sweep over the original 93 DSPs at the default `2e-06`
tolerance (before the 21-case `ondemand_*` extension):

| Backend | Match | Mismatch | Compile-fail |
|---|---|---|---|
| C++ (full 4-pass, exact) | **92** | 0 | 1 (`subcontainer1`) |
| C (full 4-pass, exact) | **92** | 0 | 1 (`subcontainer1`) |
| interpreter (scalar prefix, `-part`) | **92** | 0 | 1 (`subcontainer1`) |
| Cranelift JIT (scalar prefix, `-part`, 64-bit) | **92** | 0 | 1 (`subcontainer1`) |
| WASM (scalar prefix, `-part`, 64-bit, Node) | **92** | 0 | 1 (`subcontainer1`) |
| AssemblyScript (scalar prefix, `-part`, `asc` + Node) | **92** | 0 | 1 (`subcontainer1`) |
| Rust (scalar prefix, `-part`, `rustc`) | **92** | 0 | 1 (`subcontainer1`) |
| Vector variants (`-vec -lv 0` / `-vec -lv 1`) | inherit backend gates |  |  |

The corpus now also includes `ondemand_01_basic.dsp` through
`ondemand_21_nested_delay_counter.dsp`. Their genuine C++ 60000-frame
references all contain finite non-zero signal, and their faust-rs interpreter
15000-frame scalar responses are finite, non-silent, and match the C++
reference prefixes at the default tolerance. The smallest non-zero sample
count is 144 (`ondemand_03_input_filter`), so none of these cases can pass as a
silent-response test.

The 18 multirate cases, `upsampling_01_accumulator.dsp` through
`upsampling_09_sr_filter.dsp` and their `downsampling_*` counterparts, cover
input-consuming and domain-free state, parallel recursion, delay lines,
dynamic UI rates, multiple branches, nested domains, delayed selectors, and a
two-oscillator filtered signal whose phase and filter coefficients depend on
the domain-local `ma.SR`. Every 15000-frame interpreter response is finite and
non-silent (between 3 and 30000 non-zero output samples), and every response
matches the genuine C++ reference prefix at the default tolerance. Dedicated
runtime and C++ differential compiler tests additionally assert that `ma.SR`
is `SR*H` under upsampling and `SR/H` under downsampling, including nested
factor composition.

In the historical baseline, the C++ backend reproduced the full 60000-frame
reference exactly on 92/93 DSPs. The remaining mismatches were classified by
their *max* delta and either given a per-DSP tolerance (bounded rounding) or
listed as a known failure (real gap) in [`known.mk`](known.mk) /
[`KNOWN_FAILURES.md`](KNOWN_FAILURES.md). With those applied, the original
93-DSP aggregate targets were green: `make cpp` (92), `make c` (92),
`make cranelift` (92), `make interp` (92), `make wasm` (92),
`make assemblyscript` (92), and `make rust` (92). The vector-mode gates use suffixed
outdirs such as `cpp-vec0` / `cpp-vec1`, inherit the base backend known-failure
lists, and can be run per backend or together with `make all-vec`; excluded
cases are documented in `known.mk` to fix later.

The backend matrix uses separate outdirs such as `cpp-ss2` and
`wasm-vec1-ss3`. `BACKEND_MATRIX_SMOKE_DSPFILES` defaults to `APF`, `delays`, and
`select2`, covering recursion, delay storage, and conditional selection. The
full-corpus gate is `make -j8 backend-matrix`; `dspfiles` can also be overridden
explicitly for a targeted run.

`make -j8 backend-matrix-full` is the reproducible full gate. It removes only scheduling
matrix outdirs, executes all 6,624 comparisons from fresh artifacts, and writes
`porting/generated/p7-executable-backend-matrix-2026-07-14-en.md`. The report
checks every expected response and records one aggregate SHA-256 per
backend/mode/strategy combination.

## Performance Bench

`make bench` runs every impulse DSP through `faustbench -single` with both
`FAUST_CPP` and `FAUST_RS`. It alternates which compiler runs first for each
successive DSP, avoiding a corpus-wide thermal or frequency bias. Because
`faustbench` finds a binary named `faust` on `PATH`, the target creates
temporary wrappers under `build/bench/` and writes:

- `build/bench/summary.csv` — DSP name, C++ Faust throughput, faust-rs
  throughput, relative delta, explicit status, and run order.
- `build/bench/aggregate.csv` — comparable count, win/loss counts, geometric
  mean, median, regression count, and counts for every non-comparable status.
- `build/bench/logs/*.log` — full `faustbench` output for each compiler.

Only pairs of finite positive measurements with status `ok` enter the
performance aggregates. The corpus itself is not silently reduced:

- `unsupported_cpp` identifies a C++ Faust `undefined symbol` diagnostic when
  faust-rs produced a measurement;
- `failed_cpp`, `failed_faust_rs`, and `failed_both` identify missing
  measurements;
- `nonfinite_cpp`, `nonfinite_faust_rs`, and `nonfinite_both` retain explicit
  `inf` or `nan` results without treating them as numeric performance.

The first five columns of `summary.csv` retain their previous order; the
`run_order` audit column is appended. `BENCH_DIR`, `BENCH_CSV`, and
`BENCH_AGGREGATE_CSV` are all overridable.

The default `BENCH_OPTIONS=-double` is convenient for a quick exploratory
pass, but it does not provide the repeated measurements expected for a normal
performance comparison. For the usual benchmark workflow, use five runs and a
fixed 512-sample block:

```bash
make bench BENCH_OPTIONS="-double -run 5 -bs 512"
```

This is the recommended command for results intended to guide optimization or
report a regression. The recipe also passes `-I dsp -I $(FAUSTLIBS)`.
`BENCH_WARN_MIN` remains independently overridable when a different reporting
threshold is wanted.

`make bench-self-test` uses a synthetic `faustbench` fixture to verify
alternating order, all principal statuses, geometric-mean/median calculation,
and the CSV contracts without requiring either compiler.

`make vec-bench` keeps the `faust-rs` compiler and native C++ build settings
fixed and measures the 12 combinations formed by scalar, `-vec -lv 0`, and
`-vec -lv 1` crossed with `-ss 0..3`. It writes:

- `build/bench/vector-scheduling.csv` — one row per DSP/combination, including
  throughput, gain versus `scalar -ss 0`, and vector gain versus scalar at the
  same `-ss` value.
- `build/bench/vector-scheduling-summary.csv` — the fastest mode and scheduling
  strategy for each DSP.
- `build/bench/vector-scheduling-aggregate.csv` — arithmetic and geometric mean
  speedups, plus win counts, for each of the 12 mode/strategy combinations.
- `build/bench/logs/*.scalar.ss*.log` and `*.vec*.ss*.log` — raw faustbench
  output for every measurement.

The benchmark input is deliberately restricted to
`../vector-coverage/certified-dspfiles.txt`, the intersection certified by the
complete float/double, `-lv`, and `-ss` retention matrix. Consequently its
vector speedup aggregates cannot include scalar fallback modules. Regenerate
that list only through `cargo run -p xtask -- vector-coverage-merge` after an
intentional, reviewed coverage-baseline update.

This is a developer performance benchmark, not a correctness gate. Use several
runs and a fixed block size when comparing changes:

```bash
make vec-bench VEC_BENCH_OPTIONS="-double -run 5 -bs 512"
```

`make compile-bench` measures compiler wall-clock time on the same corpus. It
generates C++ with `-lang cpp -double` through both `FAUST_CPP` and
`FAUST_RS`, writes generated sources under `build/bench/compile/`, and records:

- `build/bench/compile-summary.csv` — DSP name, C++ Faust compile time,
  faust-rs compile time, and relative delta.
- `build/bench/logs/*.compile.*.log` — compiler stderr plus high-resolution
  wall-clock timing output.
