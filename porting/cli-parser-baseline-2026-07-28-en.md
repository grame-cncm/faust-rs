# CLI Parser Baseline

Date: 2026-07-28

Phase: C0 of
`porting/cli-parser-consolidation-analysis-and-porting-plan-2026-07-28-en.md`

Source commit: `907fa4bf`

## 1. Accepted decision

The consolidation will preserve successful invocations, defaults, legacy
Faust spellings, option repetition, and generated command payloads.

Invalid process command lines intentionally adopt standard Clap behavior:

- syntax and validation failures exit with status 2;
- `--help` and `--version` exit with status 0;
- an unknown `xtask` command becomes an error instead of printing usage and
  returning status 0.

Repository CI and scripts do not invoke unknown commands or branch on their
current success status. This adaptation is therefore accepted for C1-C4.

## 2. Parser classification

| Class | Targets | C0 decision |
|---|---|---|
| Already Clap-based | `faust-rs` | Keep legacy normalization followed by `CliArgs::parse_from` |
| Ordinary binary CLI | `impulse-runner`, `impulse_cranelift`, `xtask`, `treearena_bench` | Migrate to Clap |
| Repeatable example CLI | `interp_baseline`, `count_vector_corpus`, `corpus_scan_cranelift`, `compute_bench`, `export_clif` | Migrate to Clap |
| Embedded C argument protocol | `ffi_common::parse_ffi_compile_args` and FFI factory `argc`/`argv` consumers | Preserve; not a process CLI |
| Already-received compiler options | JSON naming and auxiliary-output option inspection | Preserve; not a process CLI |
| Data parsers | FIR, FBC, CLIF, JSON, reports, runtime traces | Preserve; not a process CLI |
| Standalone generated architecture | `tests/impulse-tests/archs/impulserust.rs` | Keep dependency-free `-n` parsing as an explicit exception |

No new shared CLI crate is authorized. Declarative argument structures remain
with their owning tools.

## 3. Repository call sites

### 3.1 CI

`.github/workflows/ci.yml` invokes:

| Command | Options |
|---|---|
| `cpp-backend-diff-report` | none |
| `golden-check` | none |
| `ffi-boundary-check` | none |
| `structure-check` | none |
| `backend-align-smoke` | `--skip-golden` |
| `vector-coverage-check` | none |
| `vector-interp-opt-check` | none |
| `vector-compile-budget-check` | release profile, no command options |

All are successful-workflow contracts and must remain accepted byte for byte
at the command-dispatch boundary.

### 3.2 Impulse Makefiles

`tests/impulse-tests/common.mk`, `Make.interp`, and `Make.cranelift` invoke the
release runners with a DSP positional path and Faust-style options forwarded
from the harness. Required accepted forms are:

```text
<file.dsp>
<file.dsp> -n N
<file.dsp> -double
<file.dsp> -single
<file.dsp> -I DIR              # repeatable
<file.dsp> -vec
<file.dsp> -vs N
<file.dsp> -lv N
<file.dsp> -ss N
<file.dsp> --scheduling-strategy N
```

The options may appear before or after the DSP positional argument. Unknown
flags and a second DSP positional argument remain errors.

### 3.3 Documented `xtask` invocations

The following command names are present in the root README, `xtask` README,
tests, CI, generated-file provenance, or active porting documents and must
remain accepted:

```text
golden-check
golden-check-cpp
golden-gen-rust
golden-gen-cpp
interp-trace-dump
interp-trace-dump-cppfbc
interp-trace-gen-cppfbc
interp-trace-gen
interp-trace-check
fir-dump-scan
build-faustwasm-compiler-module
build-libfaust
backend-align-smoke
backend-align-nightly
code-graphs
parser-parity-report
corpus-status-report
corpus-status-query
cpp-backend-diff-report
c-fastlane-diff-report
backend-full-corpus-diff-report
table-fastlane-diff-report
libfaust-api-matrix
libfaust-export-check
p7-matrix-report
vector-coverage-merge
vector-coverage-check
vector-interp-opt-check
vector-compile-budget-check
lockstep-simd-check
ffi-boundary-check
structure-check
cli-transcript-gen
cli-transcript-check
emission-determinism
```

`golden-gen-cpp -- <args>` must preserve every trailing argument as
`OsString`, including leading hyphens.

Some older planning documents mention commands that do not exist in the
current dispatcher (`codebox-snapshot-*`, `interp-trace-diff-*`,
`verify-fir-corpus`). They are not C0 compatibility commitments.

### 3.4 Secondary tool invocations

| Tool | Required successful forms |
|---|---|
| `treearena_bench` | no args; `N`; `N --prealloc`; `--prealloc N` |
| `interp_baseline` | no args; `--fixture sine_phasor`; `--fixture heavy_bench`; `--fbc PATH` |
| `count_vector_corpus` | optional positional `lv ss`; documented `--precision=`, `--json`, `--filter=`, `--shard=`, `--compare-scalar-time` |
| `corpus_scan_cranelift` | zero or more free substring filters |
| `compute_bench` | DSP path; `--fixture sine_phasor`; `--fixture heavy_bench` |
| `export_clif` | input DSP path and output CLIF path |

## 4. Current invalid-invocation observations

At source commit `907fa4bf`:

| Invocation | Current status | Current behavior | C1-C4 target |
|---|---:|---|---|
| `xtask definitely-unknown` | 0 | prints the global handwritten usage to stdout | status 2, Clap error to stderr |
| `xtask` with no command | 0 | prints global usage | status 2 with required subcommand, while `--help` is status 0 |
| `impulse-runner` with no DSP | 1 | `impulse-runner: missing <file.dsp> argument` | status 2, Clap required-positional diagnostic |
| `impulse_cranelift` with no DSP | 1 | `impulse-cranelift: missing <file.dsp> argument` | status 2, Clap required-positional diagnostic |
| runner with invalid numeric value | 1 | backend-specific handwritten string | status 2, typed Clap diagnostic |
| examples with invalid arity | panic or explicit status 2 depending on target | handwritten usage | status 2 without panic |

Exact Clap wrapping and punctuation are not frozen. Exit category, accepted
tokens, and semantic reason are.

## 5. Successful runner output baseline

Both runners were executed with:

```text
tests/corpus/rep_01_passthrough.dsp -n 8
```

Both returned status 0 and exactly:

```text
number_of_inputs  :   1
number_of_outputs :   1
number_of_frames  :      8
     0 :  1.000000
     1 :  0.000000
     2 :  0.000000
     3 :  0.000000
     4 :  0.000000
     5 :  0.000000
     6 :  0.000000
     7 :  0.000000
```

C1 must retain this text exactly for both backends.

## 6. Value/default contract

### 6.1 Impulse runners

| Field | Default/current rule |
|---|---|
| frames | 15000 |
| precision | single unless the last `-double`/`-single` selector says otherwise |
| imports | zero or more, in command-line order |
| compute mode | scalar unless `-vec` is present |
| vector size | `ComputeMode::DEFAULT_VEC_SIZE` |
| loop variant | 0 |
| scheduling strategy | depth first; integer values use `SchedulingStrategy::decode`, so values >= 3 map to reverse breadth first |

### 6.2 `xtask`

Existing option structures and their `Default` implementations are the C2
source of truth. C2 may replace their parser functions with Clap derives but
must not silently change defaults, repeated-option order, case selection, or
output locations.

### 6.3 Secondary tools

C3 must preserve the defaults encoded in the existing sources, including:

- `treearena_bench`: 200000 nodes, no preallocation;
- `count_vector_corpus`: loop variant 0, scheduling strategy 0, `f64`;
- default fixture pairs for `interp_baseline`;
- the existing fixed benchmark constants in `compute_bench`.

## 7. C0 pass criteria

- Every direct `std::env::args` consumer is classified above.
- Active CI, Makefile, README, test, and generated-provenance calls are
  inventoried.
- The invalid-command adaptation is accepted.
- Legacy runner forms and trailing C++ Faust arguments are frozen.
- Clap remains version 4 with derive support; no upgrade is part of C1.
- FFI `argc`/`argv` behavior is explicitly out of scope.
