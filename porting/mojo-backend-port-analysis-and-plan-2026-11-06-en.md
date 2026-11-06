# The Mojo backend of pull request 1293, ported to faust-rs. Analysis and plan

Date: 2026-11-06

Status: **plan, nothing landed.** §6 lists the decisions to take before
phase M1 starts; §5 gives the phases, each with its deliverable and pass
criterion.

## Scope and relation to the earlier analysis

`porting/mojo-gpu-pr-1293-analysis-and-batch-axis-2026-09-18-en.md` read
the same pull request for its ideas (benchmark, explicit SIMD, GPU) and
ranked the Mojo backend itself last (its §5 item 4: "not now", because
scalar Mojo brings no speed and adds a toolchain). This document follows
a request to port the backend anyway. It does not revisit the benchmark,
the GPU axis or the DDSP comparison. It reads the backend at the level of
the generated text: what the code generator emits, what contract the Mojo
architecture expects, which parts of the C++ code should not be copied,
and how the backend fits into the faust-rs emitter set.

### Reference pins

- Pull request: `grame-cncm/faust#1293`, head `05ac86fb1` ("Fix naming
  issue", 2026-09-18), still **open**. It targets `master-dev` and branches
  from `fbd198a27`. The pinned C++ reference of faust-rs
  (`master-dev-ocpp-od-fir-2-FIR19`, `8eebea429`) has **no Mojo backend**.
  The PR is therefore the only reference for the Mojo contract, and it can
  still change before it is merged.
- Fetched locally as the ref `pr-1293-mojo` in `RUST/faust`, with a
  detached worktree at `RUST/faust-pr1293` for reading. Nothing from the PR
  has been built or run (see §6 D4).

## 1. What the C++ backend is

Compiler side, under `compiler/generator/mojo/` (3,500 lines without the
README):

| File | Lines | Role |
|---|---|---|
| `mojo_code_container.{hh,cpp}` | 635 | `MojoCodeContainer` (abstract) + scalar, vec and GPU containers; `produceClass` writes the whole file |
| `mojo_instructions.{hh,cpp}` | 957 | `MojoInstVisitor` (scalar text visitor), `MojoInitFieldsVisitor` (constructor field init), math-name table |
| `mojo_instructions_vec.cpp` | 578 | `MojoVecInstVisitor`: explicit SIMD for `-vec` |
| `mojo_instructions_gpu.cpp` | 493 | `MojoGpuInstVisitor`: `-gpu` task stages and kernels |
| `__mojo_utils.hh`, `__mojo_macro.hh`, `__mojo_hal.hh` | 576 | writers (`wtab`, `wrewind`), `snakeCase`, `ensureReal`, option banner, platform macros |

Changes outside the directory (`git diff fbd198a27 pr-1293-mojo -- compiler`):

- `libcode.cpp` `compileMojo`: `gNeedManualPow = false`, `gBool2Int = true`,
  `InstructionsCompiler` (scalar) or `DAGInstructionsCompiler` (`-vec`).
- `global.cpp`: `-lang mojo`; `mojo` added to `internal_math_ff` (math
  functions are not emitted as foreign declarations); `-a` allowed for
  `mojo`; a new `-gpu` option, valid only with `-lang mojo`, that forces
  the vector compiler.
- `Text.cpp` `encodeFloat`: in single precision a real literal is written
  `F32(x)` (Mojo has no implicit narrowing); in double, bare `x`.
- `floats.cpp`: `FaustFloat` / `F32` / `F64` names, no suffixes.
- `code_container.cpp` `generateFillFun`: Mojo joins Julia and the Python
  backends in the fill-function shape.
- `type_manager.hh`: a `MojoStringTypeManager`. The same hunk also rewrites
  the C++ **Julia and JSFX** type tables from `Int32`/`Int64` to
  `S32`/`S64`, and the Cmajor `vector<i32>` to `vector<S32>`. Those names do
  not exist in those languages, so this looks like a C++ regression in the
  PR rather than an intended change. It does not affect faust-rs, but it
  should be reported on the PR.

Architecture side, under `architecture/mojo/`: `conf/prelude.mojo` (type
aliases), `dsp/dsp.mojo` (trait `FaustDsp`), `gui/gui.mojo` (trait
`FaustGui`), `meta/meta.mojo` (trait `FaustMeta`), `help/math.mojo`
(`pow_unrolled`), PortAudio and a terminal GUI through C FFI, bench,
inspect and impulse mains, and the GPU device runtime. Tests side:
`tests/impulse-tests/Make.mojo` (`-double`, `DFAUST=DType.float64`,
`grain3`, `osc_enable` and `prefix` excluded) and 133 `_custom/*_vec4.ir`
references for the SIMD path.

## 2. The generated contract

This is what faust-rs must reproduce so that the PR's `architecture/mojo`
files work unchanged with faust-rs output (AGENTS.md §5, "same public
contract"). The generated file has **no imports**: the architecture
supplies the prelude through `<<includeclass>>`.

### 2.1 File layout

```
# ==============================================================================
# Code generated with Faust <version> (https://faust.grame.fr)
# name: ..., author: ...           (only name/author/copyright/license/version)
# Compilation options:
#   -lang mojo -ct 1 -es 1 ...     (wrapped at 81 columns, "# " prefix)
# ==============================================================================

@fieldwise_init
struct mydsp(FaustDsp):
    var null_val: FaustFloat
    var <field>: <Type>            (one per DSP struct field, then globals)

    @always_inline
    def __init__(out dsp):
        dsp.null_val = FaustFloat(0.0)
        dsp.<field> = <init or zero>

    def get_sample_rate(imm dsp) -> S32
    def get_num_outputs(imm dsp) -> S32
    def get_num_inputs(imm dsp) -> S32
    def class_init(mut dsp, imm sample_rate: S32) -> None
    def instance_constants(mut dsp, imm sample_rate: S32) -> None
    def instance_reset_user_interface(mut dsp) -> None
    def instance_clear(mut dsp) -> None
    def instance_init(mut dsp, imm sample_rate: S32) -> None
    def init(mut dsp, imm sample_rate: S32) -> None
    def get_json(imm dsp) -> String
    def metadata(imm dsp, mut meta: Some[FaustMeta]) -> None
    def build_user_interface(mut dsp, mut ui: Some[FaustGui]) -> None
    def compute(mut dsp, var count: S32, var inputs: ImmStreams, var outputs: MutStreams) -> None
```

Every method is preceded by `@always_inline`. An empty body is `pass`.
`instance_init` calls `instance_constants`, `instance_reset_user_interface`
and `instance_clear` in that order; `init` calls `class_init` then
`instance_init`. This is the lifecycle contract of
`porting/backend-lifecycle-contract-en.md` with the names in snake case.

### 2.2 Names and types

- Every FIR name goes through `snakeCase`: camel case is split with `_`, a
  digit run is kept together, and a leading `f_` is removed. So `fRec0` →
  `rec0`, `fHslider0` → `hslider0`, `iRec1` → `i_rec1`, `fConst2` →
  `const2`, `fSampleRate` → `sample_rate`. Struct fields are read and
  written as `dsp.<name>`.
- Types: `S32`, `S64`, `F32`, `F64`, `Bool`, `FaustFloat`; a sized array is
  `Arr[T, N]`; a pointer is `Ptr[T]`; channel buffers are
  `ImmStreams` / `MutStreams`.
- Array fields are initialized `Arr[T, N](fill=0)` (or `fill=0.0`), or
  `Arr[T, N](uninitialized=True)` for a declaration without a value; an
  initialized table is a `[v0,v1,...]` list literal.

### 2.3 Statements and expressions

| FIR | Mojo |
|---|---|
| declare local | `var name = value` / `var name: T` |
| store | `target = value` |
| array access | `a[i]`; a pointer access `p[unsafe_offset = i]` |
| integer literal | `S32(n)`, `S64(n)` |
| real literal | `x` in `-double`, `F32(x)` in single precision (always with a `.`) |
| cast | `T(value)` |
| binary op | `(a) op (b)`, with the C operator spelling |
| `select2` | `then if cond else else_` |
| `if` | `if (cond != 0):` / `else:` |
| counted loop | `for var i in range(S32(lo), hi):`; reversed `range(S32(hi), lo - 1, -1)` |
| general loop | `init` then `while cond:` with the increment at the end of the body |
| math call | through the name table (`fabsf` → `abs`, `fmodf` → `mod`, `min_i` → `min`, …) |
| `pow(x, n)` with a constant positive whole `n` | `pow_unrolled[n](x)` (defined in `help/math.mojo`) |
| helper function | `@always_inline def name(T0, T1) -> R:` |
| label | nothing |
| soundfile | **rejected** (the C++ visitor panics) |

C++ sets `gBool2Int = true`, so the instruction compiler casts boolean
results to integers before they reach integer arithmetic. faust-rs FIR
already carries explicit casts; M1 must check that no `Bool` reaches an
`S32` operand unconverted, since Mojo would reject it.

### 2.4 UI and metadata

- `ui.open_vertical_box("label")`, `open_horizontal_box`, `open_tab_box`,
  `ui.close_box()`.
- `ui.add_button("label", dsp.zone)`, `add_check_button`.
- `ui.add_horizontal_slider("label", dsp.zone, FaustFloat(init),
  FaustFloat(min), FaustFloat(max), FaustFloat(step))`, and the vertical
  slider and `add_num_entry` with the same arguments.
- `ui.add_horizontal_bargraph("label", dsp.zone, FaustFloat(min),
  FaustFloat(max))` and the vertical one.
- `ui.declare(dsp.zone, "key", "value")`, with `dsp.null_val` for the `0`
  zone. This is why every struct carries a `null_val` field.
- `metadata`: one `meta.declare("key", "value")` per metadata entry;
  `compile_options` replays the option string; the first `author` is
  `author`, the following ones are `contributor`.
- `get_json` returns the one-line JSON description as a string literal.

### 2.5 `compute`

Scalar: the compute-block declarations (control reads), then
`for var i0 in range(S32(0), count):` over the sample body, then the
post-compute block. Channel pointers come from `inputs[unsafe_offset = c]`
and `outputs[unsafe_offset = c]`.

## 3. What not to copy

These are properties of the C++ code. A port should reproduce the
generated contract (§2), not the following implementation choices:

1. **`snakeCase` is not injective, and nothing checks it.** `fRec0` and a
   name `rec0`, or `fA_b` and `fA_B`, give the same Mojo identifier, and the
   `f_` stripping widens the collision space. The emitter will build the
   rename map once per module, check that it is injective, and fail closed
   with a backend error if it is not, with a unit test that forces a
   collision.
2. **`instance_reset_user_interface` asserts that every store is a cast.**
   The C++ writer crashes (`faustassert`) on any other shape. faust-rs FIR
   does not promise that shape. The port emits the reset block through the
   ordinary statement path.
3. **Text rewinding.** `wrewind` seeks the output stream back to delete an
   indentation already written. The Rust emitter writes into a `String`
   and indents at the point of writing, as the Julia emitter does.
4. **Global mutable visitor state.** `gScalarProducer`, `gVectorProducer`,
   the static function-symbol table, and the SIMD flags of the vec visitor
   are all globals. The port keeps emission state in a per-call context.
5. **A latent bad name.** `floatptrptrname[0]` is `"FaustFloat"` where
   `Ptr[Ptr[FaustFloat]]` is meant. Nothing appears to read it on the
   scalar path, but the port takes its types from FIR, not from that table.
6. **Explicit SIMD keyed on names and shapes.** The `-vec` visitor finds
   the main loop by the name `vindex`, drops the first DAG element because
   it assumes that element initializes the index, assumes the last store of
   each loop decides the strategy, finds bargraphs by field name, does not
   check that `-vs` equals `simd_width_of[f32]()`, emits no tail loop, and
   is validated only for `-double -vec -dfs -vs 4 -mcd 4` on Apple
   silicon. The 2026-09-18 analysis (§5 item 2) already proposed the
   alternative: explicit SIMD as a FIR-to-FIR pass with an independent
   checker. That matches the project rule that optimizations go in a FIR
   pass rather than in an emitter.
7. **The GPU path.** Its parallel axis is the loop DAG of a single
   instance, which gives near-zero occupancy and is dominated by launch
   cost (2026-09-18 analysis, §1.3). It is not part of this port.

## 4. Where the backend goes in faust-rs

### 4.1 Emitter

`crates/codegen/src/backends/mojo/mod.rs`, modelled on the Julia emitter
(`backends/julia/mod.rs`), the closest existing backend: a struct holding
all the state, lifecycle methods taking the DSP explicitly, and table
generators inlined into `class_init`. The C++ Mojo container does the same
thing: `writeClassInit` runs `inlineSubcontainersFunCalls` on the
static-init instructions, and `mergeSubContainers` merges the generator
state.

- Entry point `generate_mojo_module(store, module, &MojoOptions)`, run under
  `fir::on_compile_stack` like every other emitter.
- Table generators: `fir::subcontainer::flatten_sub_modules_owned(...,
  MergedStructFields)` followed by `promote_static_tables_to_struct`, as in
  Julia; `surviving_sub_modules` refuses anything left over.
- Module skeleton: `module_view::decode_module`,
  `collect_defined_functions`, `SkeletonCodes`.
- Literals: one `StringDialect` / `RealSpelling` entry in
  `backends/literals.rs` (Mojo real spelling: always with a `.`, `F32(..)`
  in single precision, special values through `inf` / `nan` helpers to be
  checked against the architecture).
- Errors: `CodegenErrorCode` `FRS-CGEN-MOJO-0001` (root not a module),
  `-0002` (invalid module section), `-0003` (unsupported node, which
  includes soundfiles), `-0004` (snake-case name collision); registered in
  `backend_error.rs` and `compiler/src/error_mapping.rs`.
- Options: `MojoOptions { class_name, real_type, compile_options,
  metadata_name, metadata_filename, metadata_entries, dsp_json }`, the same
  transport as `JuliaOptions`.

### 4.2 Wiring

The integration points, listed from the AssemblyScript and Julia rows:

- CLI: `CliLang::Mojo` (`cli/args.rs`), `cli_lang_name` (`cli/runner.rs`),
  `cli/validate.rs`, `cli/source_mode.rs`, `cli/fixture_mode.rs`, the `-a`
  wrapper path (`<<includeIntrinsic>>` / `<<includeclass>>`, which the
  impulse architecture uses).
- Facade: `compiler/src/emitters.rs`, `signal_lowering.rs`
  (`lower_signals_to_mojo`), `lib.rs`, the JSON companion.
- Capability row in `compiler/src/execution.rs` (§6 D2 for `-vec`).
- Tests that enumerate the backends: `codegen/tests/textual_backend_contract.rs`,
  `compiler/tests/lifecycle_leak_guard.rs`, `execution_options.rs`,
  `deep_expressions.rs`, `xtask/src/cli_transcript.rs`.
- Documentation: `docs/user-cli-guide-en.md`, the compatibility registry
  (`porting/faust-rs-vs-faust-cpp-differences-en.md`), the public-API
  baseline (`cargo run -p xtask -- code-graphs`).

## 5. Phases

Each phase lands as one or more commits on the branch `mojo-backend`, with
the AGENTS.md §3 gates, and the compile-budget gate whenever `codegen` or
`compiler` is touched (by hand pre/post on this machine, since the gate's
calibration falls under its floor here).

### M1 — Scalar emitter

- Deliverable: `backends/mojo/mod.rs` covering every FIR node the Julia
  emitter covers, minus soundfiles (rejected); unit tests in the crate:
  the struct shape and method set of §2.1, the literal and cast spellings
  of §2.3 in both precisions, the UI calls of §2.4, the snake-case rename
  (`fRec0` → `rec0`, `iRec1` → `i_rec1`, `fSampleRate` → `sample_rate`)
  and a forced collision rejected with `FRS-CGEN-MOJO-0004`, a table
  generator inlined into `class_init`, `pow_unrolled` only for a constant
  positive whole exponent, and soundfile rejection.
- Pass criterion: `cargo test -p codegen` green, and every FIR fixture used
  by `textual_backend_contract.rs` emits without error.

### M2 — CLI, facade and contract gates

- Deliverable: `-lang mojo` end to end (§4.2), the capability row, the
  lifecycle conformance test required by AGENTS.md §5 before a backend
  enters any gate (`init = class_init → instance_init`, `instance_init =
  instance_constants → instance_reset_user_interface → instance_clear`, no
  `class_init` from `instance_init`, no ad hoc clearing in `compute`), the
  backend added to the lifecycle leak guard and the textual contract
  test, a registry entry, and the CLI guide.
- Pass criterion: `faust-rs -lang mojo` produces output for every
  generatable `tests/corpus` and `tests/impulse-tests/dsp` program (soundfile
  programs refused with the backend error), and all repository gates pass.

### M3 — Compiled and run (needs the Mojo toolchain, §6 D4)

- Deliverable: a self-contained impulse runtime
  `tests/impulse-tests/archs/impulsemojo.mojo`, the Mojo counterpart of
  `impulsejulia.jl`. It holds the prelude aliases, the three traits, a
  control GUI that drives the zones, and the impulse driver, written for
  the test and not copied from the PR's architecture tree. Also a
  `Make.mojo` (`-double`, `DFAUST=DType.float64`) and a `make mojo`
  target. Separately, a compile check against the PR's own
  `architecture/mojo/impulse.mojo`, so that the *contract* is checked
  against the upstream runtime and not only against ours.
- Pass criterion: every non-soundfile impulse program compiles with
  `mojo build` and matches `reference/*.ir` under the scalar-prefix rule
  the Julia gate uses; exclusions listed in `KNOWN_FAILURES.md` with a
  reason (the PR itself excludes `grain3`, `osc_enable` and `prefix`).

### M4 — Differential text against the C++ PR (optional, §6 D3)

- Deliverable: C++ PR outputs for the golden corpus, stored with their
  pin, and a structural diff (normalizing variable numbering, which
  differs by design under DIFF-BEH-009) wired into a test.
- Pass criterion: no structural difference other than the ones recorded
  in the registry.

### M5 — `-vec` (separate plan)

Two levels. (a) The vector FIR of faust-rs emitted with the scalar loop
emitter: correct, but not SIMD, because Mojo does not autovectorize. (b)
Explicit SIMD as a FIR pass (`SimdLoad` / `SimdStore` / lane-typed
operations) with an independent checker, shared with Cranelift and WASM,
as proposed in the 2026-09-18 analysis. Level (b) gets its own plan
document and is not part of this branch.

### Not ported

`-gpu` (§3 item 7). The CLI refuses it with the existing unknown-option
path; the registry records it as `excluded`.

## 6. Decisions to take before M1

- **D1 — Contract reference.** The PR is open and can still change. The
  proposal is to pin the contract to `05ac86fb1`, record that pin in the
  registry entry, and re-check it when the PR is merged.
- **D2 — `-vec`.** Either (a) accepted, with the vector FIR emitted as
  plain loops (correct, no SIMD; the capability row is `Explicit`, the
  registry records that faust-rs Mojo `-vec` is not explicit SIMD and does
  not require `-double` as C++ does), or (b) refused (`Unsupported`) until
  M5(b) exists. (a) matches what Julia does; (b) avoids output that is
  slower than the flag suggests.
- **D3 — C++ reference outputs.** M4 needs the PR's compiler, built from
  `RUST/faust-pr1293` (`build/backends/regular.cmake` has
  `MOJO_BACKEND`). Building and running that code is your call; without
  it, M4 is dropped and parity rests on M3.
- **D4 — Mojo toolchain.** M3 needs `mojo`. `pixi` is installed;
  `pixi global install mojo` (or the PR's `pixi.toml`) would download
  Modular's compiler from its conda channel. Without it, the branch stops
  at M2: structural tests only, nothing compiled. The Mojo version
  matters, because the prelude uses recent syntax (`comptime`, `def` with
  `imm`/`mut`/`out`, `Some[Trait]`).
- **D5 — Precision.** C++ Mojo scalar accepts `-single` and `-double`;
  `-quad` and fixed point are refused. The proposal is to do the same.

## 7. Risks

- **Moving target.** Mojo's syntax has changed release to release. The
  emitted text is tied to the version the PR used. The registry entry will
  name that version once it is known (the PR's `pixi.lock`).
- **Silent semantic drift in casts.** Mojo has no implicit numeric
  conversion. A missing cast is a compile error, not a wrong result, so M3
  catches it, but M1/M2 cannot.
- **Integer semantics.** Faust integer arithmetic wraps. Whether Mojo
  `Int32` overflow, division and remainder by zero, and shifts beyond the
  width behave the same is not established here; M3 must check it on the
  impulse programs that exercise them (DIFF-BEH-010) before the gate is
  called green.
- **Name rewriting.** `snakeCase` is applied to every FIR name, so the
  injectivity check of §3 item 1 is the guard against the one failure that
  would compile and run wrongly: two variables merged into one.

## 8. References

- `grame-cncm/faust#1293` at `05ac86fb1`: `compiler/generator/mojo/`,
  `compiler/generator/mojo/README.md`, `architecture/mojo/README.md`,
  `architecture/mojo/dsp/dsp.mojo`, `conf/prelude.mojo`, `gui/gui.mojo`,
  `meta/meta.mojo`, `help/math.mojo`, `tests/impulse-tests/Make.mojo`.
- `porting/mojo-gpu-pr-1293-analysis-and-batch-axis-2026-09-18-en.md`.
- `porting/julia-backend-plan-2026-05-13-en.md` (the emitter this plan
  follows).
- `porting/backend-lifecycle-contract-en.md`.
- `porting/faust-rs-vs-faust-cpp-differences-en.md` (DIFF-BEH-003,
  DIFF-BEH-009, DIFF-BEH-010).
