# Foreign functions, CLI limits, and compilation cost: analysis and correction plan

Date: 2026-10-08

Status: analysis completed for the small reproductions; corrections proposed,
not implemented. The large-program cost investigation remains open.

Rust source baseline: `main-dev`, `2199d069e462b34d960321da06d7b0ebbc08d0aa`.

C++ source baseline: `master-dev-ocpp-od-fir-2-FIR19`,
`8eebea4294a44a5260484c750d332781ed9f8ffd`.
C++ source paths below are relative to that reference repository.

Input: the user's eight-point bug report, supplied on 2026-10-08. Its
observations used a debug `faust-rs` and a release `faustprobe`. This analysis
checks the current source and small, library-independent programs rather than
assuming that every explanation in the report is correct.

## 1. Findings and priorities

The most serious defect is successful execution of a Cranelift no-op `compute`
body. Missing foreign bindings can disable the entire computation, producing
plausible zero statistics. The ordinary `-lang cranelift` CLI also returns success
for that body. Supported math foreign calls must remain accepted; rejection
depends on executable support, not the presence of the `ffunction` keyword.
Two other confirmed compatibility defects are lost
foreign headers and missing implicit wires during foreign-function application.

Two conclusions in the original report need correction: the pinned C++ compiler
rejects bare `ffunction` statements too, and it accepts partial application of a
foreign-function block. The latter is a Rust parity bug, not a Faust restriction
to document as intended behavior.

| Report | Current finding | Evidence level | Priority / work package |
|---|---|---|---|
| 1. `faustprobe` silently returns zero | Confirmed for a foreign call in `compute`. The whole body falls back to a no-op; this is broader than an unresolved call returning zero. Constant calls in `instanceConstants` already fail. | Fresh debug reproduction, existing release reproduction, source and existing backend tests | P0, W1 |
| 2. `--timeout 0` fails | The watchdog is disabled, but the phase timer still enforces a zero duration. | Fresh debug and existing release reproduction, source | P1, W2 |
| 3. Hidden structural depth cap | Confirmed. Structural lowering uses `min(evaluator_limit, structural_limit)` and loses the limit's identity in its error. A third, syntactic nesting budget has the same diagnostic problem. | Small controlled debug reproduction, source | P2, W5 |
| 4. Debug/release depth defaults | Confirmed profile-dependent constants. This analysis does not establish that raising debug defaults is safe or necessary. | Source | P2, W5 |
| 5. Bare `ffunction` statements | Rejected by both compilers. Rust additionally reports a fictitious duplicate empty-name definition during recovery. | Fresh debug and pinned C++ binary reproduction, grammar inspection | P2, W6 |
| 6. Missing foreign `#include` | Confirmed. Header/library operands are dropped while constructing the foreign prototype. Generated C++ fails to build. | Fresh debug reproduction and C++ compilation, source | P1, W3 |
| 7. Foreign partial application | Confirmed Rust defect. C++ accepts `f(1)` for a three-input foreign block and produces a two-input DSP. | Fresh debug and C++ binary reproduction, evaluator source | P1, W4 |
| 8. Nonlinear cost / `ondemand` overhead | Reported, not reproduced on the original program: the report contains no complete DSP/header or measurement protocol. Treat as a performance investigation, not an established asymptotic diagnosis. | User measurements only for the large cases | W7: baseline first; optimization priority follows evidence |

P0 means contain silent incorrect execution first. The repaired library import
problem mentioned in the report is outside this plan unless a fresh reproduction
shows that it remains open.

## 2. Validation performed

Built the current debug binaries with:

```sh
cargo build -p compiler --bin faust-rs -p cranelift-ffi --bin faustprobe
```

The build passed. All Rust findings described as fresh debug reproductions use
those binaries. Existing release binaries report version `0.8.0` and provide
corroborating observations only; they were not rebuilt or proven to correspond
exactly to the audited Rust commit. The C++ checkout is at the pinned commit;
its existing `build/bin/faust` reports version `2.84.3`. Its observed behavior
also agrees with the pinned source inspected here; it was not rebuilt.

Temporary DSPs, a small header, and C++ harnesses were created outside the
repository. No standard libraries were needed. No compiler source, tests,
goldens, or cost baselines were modified by this analysis.

### 2.1 Runtime foreign call and omitted header

`frs_probe.h`:

```cpp
static inline float frs_probe(float x) { return x + 1.0f; }
static inline float frs_three(int n, float a, float b) {
    return n + 10*a + 100*b;
}
```

`foreign_runtime.dsp`:

```faust
f = ffunction(float frs_probe(float), "frs_probe.h", "");
process = f;
```

Observed with fresh debug and existing release `faustprobe`, at both
`--opt-level 0` and `--opt-level 3`:

```text
faustprobe foreign_runtime.dsp -n 4 --quiet
exit: 0
# out0: peak=0.0 rms=0.0 dc=0.0 finite=yes peak_at=none
# note: every output is exactly zero over the window
```

The input is the default impulse. The correct samples are `[2, 1, 1, 1]`.
A pure Faust control, `process = +(1);`, produces the correct nonzero statistics
at both optimization levels.

Fresh debug C++ emission succeeds and contains a `frs_probe(...)` call but no
`#include "frs_probe.h"`. Compiling that output with a minimal local DSP/UI/meta
harness fails with `use of undeclared identifier 'frs_probe'`. Adding the header
to the harness makes it compile and produces `2 1 1 1`. The C++ Faust generator
emits the requested header without that workaround.

A different fixture, `process = f(2);`, fails in both tested probe binaries:

```text
[FRS-CGEN-CLIF-0002] Cranelift strict mode rejected fallback to
`instanceConstants` stub: unsupported math call in subset: frs_probe
```

Thus "any `ffunction` returns zero" is too broad. Variability determines which
generated lifecycle function contains the call, and those functions currently
have different fallback policies.

The follow-up question about math functions was checked with:

```faust
t = ffunction(float tanhf|tanh|tanhl(float), <math.h>, "");
process = t;
```

Fresh debug `faust-rs -lang cranelift` succeeds with
`compute_body_lowered: true`, in single and double precision. Fresh debug
`faustprobe` renders the impulse as `[tanh(1), 0, 0, 0]` at opt levels 0 and 3:
the peak is `0.7615942` in single precision and `0.7615941559557649` in double.
Equivalent declarations of `sinh`, `cosh`, `asinh`, `acosh`, and `atanh` also
produce lowered bodies in both precisions; their full runtime/domain behavior
was not exercised by this follow-up.

Conversely, `faust-rs -lang cranelift foreign_runtime.dsp` currently exits 0
in both precisions while reporting:

```text
compute_body_lowered: false
subset_gap: unsupported math call in subset: frs_probe
```

It discloses the gap in its report but still reports compilation success.
W1 must cover this ordinary CLI path as well as the probe.

### 2.2 Zero timeout

```faust
process = 1;
```

```text
faust-rs --timeout 0 -lang cpp zero_timeout.dsp -o output.cpp
exit: 1
ERROR: compilation timeout (... > 0s limit) after phase 'cpp-codegen'
```

The failure occurs even on this trivial program, in fresh debug and existing
release binaries. It is independent of the reported 270-second workload.

### 2.3 Bare statements versus partial application

Both compilers reject:

```faust
ffunction(float foo(int), "f.h", "");
process = 1;
```

The pinned C++ binary reports `syntax error, unexpected FFUNCTION`. With two
consecutive bare statements, Rust also reports `multiple definitions of symbol
''` and advice to retain one ` = ...;` clause. That additional diagnostic is
recovery noise, not evidence of a supported anonymous-definition syntax.

For this valid binding:

```faust
f = ffunction(float frs_three(int, float, float), "frs_probe.h", "");
process = f(1);
```

C++ Faust succeeds with `getNumInputs() == 2`. Rust fails with
`FRS-PROP-0002`, `left outputs (1) != right inputs (3)`.
Both accept the fully applied `f(1, 2, 3)` and the explicit block equivalent
`(1, _, _) : f`.

### 2.4 Indistinguishable depth failures

```faust
chain(0) = 0;
chain(n) = chain(n-1) + 1;
process = chain(200);
```

This succeeds with default debug limits. With evaluator limit `50000` and
structural limit `64`, it fails with `FRS-EVAL-0099`, `depth budget 64`, without
naming `FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH`. With evaluator and structural limits
both `50000` and nesting limit `64`, it produces the same summary and notes.
These overrides were confined to child processes.

The experiment establishes the missing diagnostic distinction. It does not
reproduce the original `N=1024/2048` chain or measure its cost.

## 3. Source analysis

### 3.1 Cranelift uses an explicit registry and a permissive compute fallback

- [`CraneliftOptions`](../crates/codegen/src/backends/cranelift/core.rs) exposes
  `extern_function_symbols` and `fail_on_subset_gap`. The foreign binding model
  is an explicit name-to-address map, not simply an automatic dynamic lookup.
- [`compile_fir_module_to_cranelift`](../crates/cranelift-ffi/src/factory.rs)
  snapshots the registered functions and otherwise uses default backend
  options. `faustprobe` does not compile header text into host functions.
- [`declare_jit_function`](../crates/codegen/src/backends/cranelift/jit_data.rs)
  emits a return-only body on a subset gap unless strict mode is requested.
  `compute_body_lowered` becomes false. Zero-initialized output buffers then
  make a successful render look like silence; the stub does not itself fill
  the buffers with zero.
- [`generate_cranelift_module`](../crates/codegen/src/backends/cranelift/api.rs)
  uses the configurable policy for `compute`, but strict checks for generated
  `instanceConstants`, `staticInit`, and `instanceClear` functions.
- The panic recovery path retries with `force_stub=true`; the strict check
  explicitly excludes that forced path. Setting one strict flag alone is
  insufficient to guarantee executable code was emitted.
- [`Factory::create`](../crates/cranelift-ffi/src/probe/engine/factory.rs) accepts
  any non-null factory. The existing factory JSON exposes
  `compute_body_lowered`, but the probe does not validate it there.
- Existing [backend tests](../crates/codegen/src/backends/cranelift/tests.rs)
  and [FFI tests](../crates/cranelift-ffi/src/factory/tests.rs) explicitly expect
  successful fallback after a foreign binding is missing, unregistered, or
  cleared. They must be considered when changing the production policy.

Header-only linkage explains why the source cannot supply an address by itself,
but changing export visibility alone does not solve this registry/fallback
problem. Automatic header compilation or library loading would be a separate
feature with its own ABI and ownership decisions.

### 3.2 Timeout enforcement is split

[`spawn_timeout_watchdog`](../crates/compiler/src/cli/validate.rs) correctly
checks `timeout_secs > 0`.
[`CompilationTimer::phase`](../crates/compiler/src/cli/timer.rs) compares elapsed
time to `Duration::from_secs(0)` unconditionally.
The [CLI guide](../docs/user-cli-guide-en.md) documents zero as disabling the
watchdog; the coherent user contract should be that zero disables the compilation
deadline in both enforcement paths.

### 3.3 Three depth budgets share one error representation

[`LoopDetector`](../crates/eval/src/loop_detector.rs) contains:

| Budget | Debug fallback | Release fallback | Actual use |
|---|---:|---:|---|
| `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH` | 1,024 | 32,768 | Identity-tracked evaluator frames; also bounds structural lowering |
| `FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH` | 4,096 | 32,768 | Structural limit is the minimum of this value and the requested evaluator limit |
| `FAUST_RS_DEFAULT_EVAL_NESTING_DEPTH` | 400,000 | 400,000 | Nested `eval_value` entries, separate from identity-tracked frames |

Environment values must be positive integers; missing, invalid, and zero values
fall back to compiled defaults. An explicit `with_max_depth` request replaces
the evaluator limit but still leaves the structural cap in force.

All three entry guards construct
[`EvalError::RecursionDepthExceeded { max_depth }`](../crates/eval/src/error.rs).
The summary, notes, and help cannot identify which guard fired. They also imply
nontermination even for a finite program. The CLI has a large worker stack, and
the evaluator uses segmented stack growth; budget changes still require evidence
across CLI and embedding environments, not a blanket claim that release accepts
every rejected debug program.

### 3.4 Foreign dependency information is discarded before emission

[`decode_foreign_fun_proto`](../crates/transform/src/signal_fir/module/core_lowering.rs)
matches `(signature, _, _)` and returns a
[`ForeignFunProto`](../crates/transform/src/signal_fir/module/mod.rs) containing
only name, result type, and argument types.
[`build_module`](../crates/transform/src/signal_fir/module/build.rs) emits
prototype-only declarations; the FIR and C++ emitter do not receive those header
and library operands. The [C++ prelude](../crates/codegen/src/backends/cpp/mod.rs)
emits a fixed list of standard headers.

In C++ Faust, `InstructionsCompiler::generateFFun` in
`compiler/generator/instructions_compiler.cpp` calls both `addIncludeFile` and
`addLibrary` before lowering the call. Header loss therefore belongs at the
signal-to-FIR/representation boundary, not solely in architecture wrapping.
The analogous `fconstant`/`fvariable` dependency paths need an audit; this report
does not establish that each of them fails in the same way.

### 3.5 Application already supports implicit wires, except for foreign blocks

[`apply_list` and its arity inference](../crates/eval/src/apply.rs) already
implement the C++ non-closure rule: determine block arity, reject excess outputs,
and append wires for missing inputs. `infer_box_arity_uncached` handles many
primitive families but has no `BoxMatch::FFun` case, so application falls back
to a plain sequential composition.

C++ `applyList` in `compiler/evaluate/eval.cpp` uses `getBoxType`, then
`concat(larg, nwires(ins - outs))`. Foreign blocks participate in that rule.
Restoring it does not require turning foreign blocks into lexical closures.

### 3.6 Parser recovery invents names

The Rust [grammar](../crates/parser/src/grammar/faustparser.y) and C++
`compiler/parser/faustparser.y` allow `ffunction` as an expression, not as a
standalone statement. Rust's recovery can insert `IDENT` and `DEF`.
[`format_definitions`](../crates/parser/src/lib.rs) then groups recovered
zero-argument definitions under an empty textual key and diagnoses redefinition.
Suppressing this invented conflict must preserve genuine duplicate-definition
and pattern-clause diagnostics.

### 3.7 Supported math calls must survive strict foreign-call validation

[`register_host_symbols`](../crates/codegen/src/backends/cranelift/host.rs)
provides callable host wrappers for supported math operations, including both
`tanhf` and `tanh`. The
[`subset matcher`](../crates/codegen/src/backends/cranelift/subset.rs) recognizes
those symbols, and
[`lower_fun_call`](../crates/codegen/src/backends/cranelift/lowering.rs) imports
them with f32/f64 signatures. The working math fixture therefore needs no
header compilation or user registration. This proves a supported subset, not
complete coverage of every declaration in `maths.lib` or `<math.h>`.

| Required call | Target `-lang cranelift` / probe behavior |
|---|---|
| Supported math symbol and compatible signature/precision | Compile and execute the backend's known implementation |
| Custom function with an explicit compatible host binding | Compile through the embedding API's registry; the host owns the ABI/address contract |
| Custom function with only a header or unbound library name | Fail with the missing symbol and required binding; naming a header supplies no implementation |
| Unsupported math signature or precision | Fail clearly, without a no-op fallback |
| Declared function that is unreachable from the compiled program | No blanket source-keyword rejection |

The ordinary CLI currently passes no custom host-function map; host registration
is an embedding capability, not a new command-line feature assumed by this plan.

C++ provenance for the LLVM comparison: `InstructionsCompiler::generateFFun`
builds external declarations/calls; `LLVMCodeContainer::produceFactory` in
`compiler/generator/llvm/llvm_code_container.cpp` links `.bc`/`.ll` dependencies;
`llvm_dynamic_dsp_factory_aux::initJIT` in
`compiler/generator/llvm/llvm_dynamic_dsp_aux.cpp` constructs LLVM's MCJIT and
uses target library information. These are implementation routes for callable
functions, not a reason to reject the source keyword. They do not establish
identical symbol coverage between LLVM and Cranelift. The local reference binary
has no embedded LLVM backend, so this session did not run an LLVM differential.

## 4. Implementation work packages

All behavior below is proposed. This document does not authorize unresolved
external API or ABI policy changes. Resolve the listed decisions with the user
before implementing the affected part, as required by `AGENTS.md` section 12.

### W1 — reject non-executable probe factories (P0)

Owner: `compiler` CLI, `cranelift-ffi` probe/factory, and `codegen` Cranelift
maintainers.

Target rule, refined following the user's math-library question: accept each
reachable foreign call only when a compatible implementation exists, either
through supported backend math helpers or an explicit host binding. Ordinary
`-lang cranelift` compilation and `faustprobe` must fail when a required call
cannot be lowered or bound. Do not reject unused declarations or ban all
`ffunction` expressions.

1. Add a single validation boundary shared by file/string compilation and all
   probe modes. Require a lowered executable `compute` body before constructing
   a usable probe. Reject missing or invalid readiness metadata rather than
   assuming success. Prefer an existing internal accessor or the existing
   factory status channel; do not widen the public C API merely to add a guard.
2. Preserve the actual subset-gap reason so an error names the missing foreign
   symbol, backend, and relevant signature/header when available. Distinguish
   "no registered binding" from "unsupported signature" and ordinary FIR
   subset gaps. Explain that `-I` does not compile a C/C++ header into JIT code.
3. Close the forced-panic fallback escape in production execution paths. A
   return-only body must not become a valid render because a JIT panic was
   caught. Factor policy tests so they do not require a huge AArch64 program or
   matching a compiler panic's text.
4. Propagate failures through human and JSON diagnostics. Do not print output
   statistics, successful comparison checks, or write a rendered artifact after
   compilation has been rejected. Validate polyphonic effect fallback too: an
   invalid effect must not disappear behind a generic extraction failure.
5. Audit every C factory constructor and factory restoration route. Identify
   the intentional scaffold behavior and the safe Rust facade's expectations.
6. Apply the executable-support requirement to ordinary `-lang cranelift` in
   both source and fixture CLI paths. The current
   [`source_mode`](../crates/compiler/src/cli/source_mode.rs) and
   [`fixture_mode`](../crates/compiler/src/cli/fixture_mode.rs) use default
   options. Keep any retained inspection-only scaffold behavior explicit; it
   must not make normal compilation or rendering look successful.
7. Inventory supported math symbols by exact spelling, signature, and precision
   from the matcher, lowerer, and host registry. Share their support decision
   where practical so the pre-check and lowering cannot drift. Preserve the
   supported `tanhf`/`tanh` paths. A math-like name or `<math.h>` header alone
   is not evidence of support for an arbitrary declaration or ABI.

Decision before changing C/low-level APIs: should all production constructors
reject missing bindings/subset gaps by default, or should a documented explicit
scaffold mode remain available? Recommendation: production compilation fails;
scaffold use, if retained, requires an explicit opt-in and cannot be rendered by
`faustprobe`. The immediate probe guard can use existing readiness information
without first settling the whole public factory policy.

Pass criteria: runtime and constant foreign calls with absent bindings fail
clearly; a genuine `process = 0;` still renders successfully; a missing function
in one lane cannot silently disable a valid second lane. Registered host
functions execute correctly at opt levels 0 and 3, in single/double precision.
Tests cover `--eval`, comparison, frequency response, training, polyphonic and
string/file factory paths through the common validation boundary.
The CLI succeeds for supported math declarations and fails for an unknown
runtime function. Include supported math signatures, rejected incompatible
signatures, selected f32/f64 symbols, and unknown names in the acceptance tests.

### W2 — make zero timeout disable all deadline checks (P1)

Owner: `compiler` CLI maintainers.

Use one timeout interpretation for the timer and watchdog. Represent a disabled
deadline explicitly, or guard the timer's zero value consistently. Keep
`--compilation-time` reporting active when the deadline is disabled. Update
Clap help and the CLI guide to say that zero disables the compilation timeout.

Pass criteria: zero succeeds on the tiny fixture in source and FIR-fixture
modes; positive deadlines still expire; `-timeout 0` has the same meaning as
`--timeout 0`. Prefer a pure expiration predicate with controlled elapsed time
and CLI subprocess tests over multi-second sleeps. Cover every timer caller
without duplicating one full test per backend. Library compilation must never
acquire the CLI's `process::exit` behavior.

### W3 — carry foreign dependencies through FIR and emit headers (P1)

Owner: `transform`, `fir`, and C/C++ backend maintainers.

1. Trace dependencies through scalar/vector emission, nested table generators,
   submodule flattening, FIR cloning/rewrites, persistence, and architecture
   wrapping before choosing a representation.
2. Proposed representation: keep include/library data with the owning foreign
   declaration, and derive module-level dependency sets from reachable
   declarations. Avoid a process-global accumulator or a detached TreeId side
   table. Specify the exact FIR representation and public matcher/builder impact
   in a short design amendment before coding; review any public API expansion.
3. Preserve quoted versus angle-bracket headers, deduplicate deterministically,
   and include dependencies from initialization and subcontainers as well as
   `compute`. Match the reference's empty-header and ordering behavior. Keep
   distinct headers even when two descriptors select the same symbol.
4. Emit dependencies in C++ before their uses, with and without `-a`. Audit the
   C path in the same change. Retain library metadata without claiming that the
   generator or JIT automatically links arbitrary libraries. Confirm what the
   pinned reference does with that metadata before promising a new artifact.
5. Audit `fconstant` and `fvariable`; add confirmed defects to the same dependency
   model rather than adding a textual source scan as a second mechanism.

Pass criteria: generated C++ builds and runs with the small static-inline
header, without a manual architecture include; quoted/angle headers, repeated
uses, scalar/vector paths, initialization calls, and nested generator
dependencies are covered. A structural test proves descriptor dependencies
survive FIR construction and relevant rewrites. Empty headers do not emit
`#include ""`. Test fixtures provide their own compact headers and DSPs.

### W4 — restore foreign-block application parity (P1)

Owner: `eval` maintainers, with `propagate` arity review.

Add `BoxMatch::FFun` arity inference using its descriptor through the canonical
builder/matcher model. Reuse the existing application rule; preserve argument
order and the ordinary block result, rather than adding a foreign-specific
closure/currying implementation. Check the existing propagation descriptor
decoder for shared invariants without adding a dependency cycle or widening an
internal helper only to move code.

Pass criteria: for a three-input function, `f(1)` has two inputs and
`f(1, 2)` has one. Full application has zero inputs; over-application yields the
existing evaluator argument error rather than a later composition mismatch.
Use the asymmetric `frs_three` arithmetic to detect swaps. Include multi-output
arguments, nested applications, direct `ffunction` application, and malformed
descriptors. Record C++ differential expectations before implementation.
Check the full expected samples in generated C++ or a registered JIT host
fixture, at opt levels 0 and 3 where applicable.

Documentation must explain block application with implicit wires separately
from closure application, without claiming that foreign blocks cannot be
partially applied. Generic `A : B` width diagnostics remain valid elsewhere.

### W5 — explain the active depth guard and build profile (P2)

Owner: `eval`, diagnostics, and CLI documentation maintainers.

1. Carry typed depth context from the guard that fired: identity recursion,
   structural lowering, or syntactic nesting; requested and effective limits;
   configuration origin (compiled default, environment, explicit API request);
   and, for structural lowering, which operand of the minimum was limiting.
   Define deterministic reporting for equal limits. Keep this data with the
   error/configuration that owns it.
2. Render it in the existing human and JSON diagnostic channels. Keep
   `FRS-EVAL-0099` unless the diagnostics contract explicitly requires a new
   code. Do not diagnose every finite deep program as missing a base case.
3. Document all three environment variables, both profiles, positive-value and
   fallback rules, and explicit API overrides. Include a help pointer in the
   actual diagnostic. Report the actual build profile/limits when relevant;
   avoid a warning on every ordinary compilation.
4. Keep the current numerical defaults for this correction. Raising or unifying
   budgets is a separate decision requiring CLI/host stack, segmented-stack
   target support, memory, pathological recursion, and cost measurements.

Example target information, with wording subject to diagnostic review:

```text
structural lowering depth limit exceeded (effective limit 4096)
requested evaluator limit: 50000 (FAUST_RS_DEFAULT_EVAL_MAX_DEPTH)
limiting cap: 4096 (FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH, debug default)
```

Pass criteria: separate low-budget tests distinguish all three guards; explicit
API limits and structural ties are covered; debug/release fallback configuration
is checked; finite deep programs fail cleanly and cycles remain detected.
Use subprocess environment overrides rather than mutating process-global
environment in concurrent tests. Measure error-context cost on successful
compilation too, given the prior provenance regression.

### W6 — diagnose bare foreign statements without invented definitions (P2)

Owner: `parser` and diagnostics maintainers.

Keep the pinned grammar contract. Provide an actionable diagnostic at the bare
`ffunction` token, explaining `name = ffunction(...);` or its use in an
expression. Treat recovered placeholder identifiers as recovery artifacts;
do not format them into semantic duplicate-definition diagnostics. Preserve
their recovery provenance rather than suppressing every empty-name error by
message matching. Apply the narrowest change that also handles repeated invalid
statements and does not discard unrelated valid declarations.

Pass criteria: one and several bare statements fail with appropriate locations
and no `multiple definitions of symbol ''`; named bindings, real duplicate names,
pattern clauses, and import recovery retain their existing contracts. Human and
JSON diagnostics agree. Test correctness independent of lrpar repair ordering
or whether its recovery search exhausts its time budget, following the recent
[repair-order tests](../crates/parser/src/lib.rs).

Supporting bare declarations as a language extension is not part of this
correction. It would require a separate explicit source-language decision.

### W7 — measure and correct foreign-chain / clock-domain compilation cost

Owner: `xtask` compilation-budget maintainers and the owner of the measured
hotspot (`eval`, `propagate`, `transform`, or `codegen`).

The reported approximately 40 seconds versus 180 seconds at `N=1024` is a
starting observation, not a performance target or proof of a particular cause.

1. Obtain or reconstruct a minimized version of the recursive write chain,
   including its header/signatures, recursion, clock wiring, flags, and depth
   overrides. Remove external library dependencies. Record whether the function
   has side effects and which calls the generated program must preserve.
2. Measure `N=64,128,256,512,1024,2048` until a documented resource cap is reached.
   Include with/without `ondemand`, a pure-arithmetic control, and a foreign-call
   control. Compare debug and fresh release Rust builds and pinned C++ on the
   same machine. A constant clock and the exact original clock are separate
   variants: compiler folding can make them different workloads.
3. Record commit/profile, host, flags, effective limits, allocator, elapsed
   phases, peak memory, arena nodes, cache activity, FIR/call counts, and generated
   output size. Separate code generation from building the emitted C++. Use
   repeated warm measurements and report spread. Use timing sinks already in
   the compiler; add counters only where measurements need them.
4. Profile the dominant phase. Candidate mechanisms include repeated `a2sb` or
   arity probing, linear scans in the recursion detector, environment-sensitive
   memo misses, provenance unions, clock graph construction, FIR scheduling,
   emission, and allocation churn. These are hypotheses, not confirmed causes.
   Re-check the current implementation before reapplying historical fixes.
5. Optimize one demonstrated source of repeated work at a time. Preserve clock
   domain identity, state advancement, foreign-call effects, deterministic
   ordering, and recursion boundaries. Shared or cached work must not merge
   distinct domains or remove required writes.
6. Add a compact representative family to the release compilation-budget gate,
   covering both frontend and codegen baskets as appropriate. Extend the fixture
   location/selection mechanism if needed rather than importing the user's
   libraries. Capture calibration units before the optimization; lower the
   baseline as improvements land. Retain all existing required cases.

Pass criteria: the original slowdown has a reproducible baseline and a measured
explanation; an optimization improves its dominant cost without runtime/clock
drift; representative cases are guarded in calibration units. Until the baseline
exists, no fixed speedup or linear scaling guarantee is justified. The existing
25% per-case threshold detects changes beyond noise; it does not prove that
smaller slowdowns are absent. Do not increase a baseline to obtain a green gate.

Related analysis:
[provenance cost regression](compile-time-provenance-regression-analysis-and-plan-2026-07-30-en.md),
[propagation cost](propagation-cost-analysis-2026-08-06-en.md), and
[on-demand clock domains](ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md).

## 5. Sequence and decision gates

| Step | Deliverable | Exit condition |
|---|---|---|
| A. Freeze reproductions and Phase 0 scope | Self-contained DSP/header cases and pinned reference expectations; start W7 measurements | Confirm active file/string/FIR routes, lifecycle and ownership, reference acceptance, per-session state, and TreeArena cost observations for the touched scope |
| B. Contain silent execution | W1 probe readiness and ordinary `-lang cranelift` validation; proposed production factory policy | Invalid executable bodies fail; supported math calls still work; broader C/scaffold compatibility decision recorded before changing it |
| C. Fix zero timeout | W2, independently reviewable | Disabled and positive deadlines verified |
| D. Restore foreign source/output parity | W3 dependency representation, then W4 application arity | Structural retention tests and C++ compilation/runtime differential pass |
| E. Make failures explain themselves | W5 depth context and W6 parser recovery | All guard types and real/recovered definitions correctly distinguished |
| F. Optimize measured workload | W7 profile, fix, calibrated retention cases | Runtime/clock parity and compilation-budget gate pass |

The [Phase 0 checklist](phases/phase-0-validation-en.md) remains mandatory before
substantial implementation. This source audit establishes concrete production
routes and fixtures; it does not claim that the entire historical Phase 0 gate
or a new FIR architecture review has been completed. Trace global state to
session-owned configuration, except for the existing explicitly registered
host-function registry whose lifetime and cache contract must be preserved.

Open decisions for implementation:

- Default failure policy across low-level Cranelift and C constructors (W1).
- Exact co-localized FIR dependency representation and persistence impact (W3).
- Public `EvalError` compatibility if richer depth context changes enum fields
  (W5); assess callers before claiming the change is internal.
- Any requested change to default budgets, automatic foreign linking/loading, or
  bare-statement acceptance requires a separate decision. None is assumed here.

## 6. Compatibility, assurance, and delivery

| Surface | Mapping target | Compatibility impact |
|---|---|---|
| Foreign block application | `1:1` with C++ `applyList` / block arity | Previously rejected partial applications become accepted; input order must match C++ |
| Foreign header collection/emission | `1:1` observable behavior; `adapted` internal FIR representation | Generated C/C++ gains required dependencies; builders/matchers/persistence may change and must be inventoried |
| Cranelift foreign binding and readiness | `adapted` Rust backend; automatic header compilation `deferred` from this plan | Missing bindings must not produce valid probe results; broader factory success policy remains a decision |
| CLI zero timeout | `adapted` Rust CLI deadline machinery | Restores the documented disabled-timeout behavior; verify legacy spelling against the reference before claiming flag parity |
| Depth limits and diagnostics | `adapted` logical resource limits versus C++ stack-address checking | Richer error context and explicit profile limits; current budgets retained |
| Bare foreign statements | `1:1` rejection; `adapted` recovery diagnostics | No source extension; removes misleading recovery errors |
| Cost corrections | Behavioral parity required | Performance gains must preserve foreign effects and clock-domain semantics |

Update the [compatibility difference registry](faust-rs-vs-faust-cpp-differences-en.md)
as each gap changes or closes. This analysis records current defects there;
it does not mark the proposed corrections as implemented.

For every implementation step, add focused tests and C++ provenance comments,
record public API mapping changes, and use small linear commits. Standard unit,
differential, golden, and runtime optimization-parity checks are appropriate
here. A finite scheduling/routing artifact needs the stronger existing checker
discipline only if the performance work actually changes that artifact.

Required implementation gates:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- cli-parser-check
cargo run -p xtask -- error-model-check
cargo run -p xtask -- ffi-boundary-check
cargo run -p xtask -- structure-check
cargo run -p xtask -- code-graphs --check
cargo run -p xtask -- golden-check
cargo run --release -p xtask -- compile-budget-check
```

Use pinned C++ differential tests for W3/W4 and clocked W7 fixtures. Where public
items change, regenerate code graphs and review the API baseline before checking
it. Document any justified golden refresh with reference commit and flags.
Generated-code tests must account for Linux/macOS/Windows C++ toolchains and
paths; headers and DSP fixtures must be test-local. CI must be green before
declaring an implementation ready.

This documentation-only session ran the debug build and the reproductions above,
not the full implementation gate suite. The original large-program timing and
cross-platform behavior remain validation tasks, not completed checks.
