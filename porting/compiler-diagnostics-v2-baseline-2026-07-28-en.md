# Compiler Diagnostics V2 Baseline

> Date: 2026-07-28
>
> Source commit: `b14a45b3`
>
> Plan:
> `compiler-diagnostics-v2-analysis-and-improvement-plan-2026-07-28-en.md`
>
> Phase: G0

## 1. Gate decision

The provenance prototype selects a hybrid representation:

- a compact origin set belongs to each hash-consed semantic node and retains
  every candidate source occurrence;
- an explicit located occurrence `(node, selected_origin)` is carried at
  ambiguity-sensitive phase boundaries and worklist entries;
- renderers may show alternative candidate origins as related locations;
- a single mutable source property per `TreeId` is not an acceptable v2
  provenance representation.

This decision preserves semantic hash-consing. It does not embed source paths
or diagnostic policy in `tlib`.

The production representation must be implemented only after G1 defines
stable `SourceId`, `SourceRange`, and `OriginId` types.

## 2. Structural ambiguity baseline

`TreeArena` maps identical `(NodeKind, children)` values to one `TreeId`.
`PropertyStore::set_with_key` stores one value for `(TreeId, PropertyKey)` and
replaces the previous value.

The G0 probe demonstrates:

```text
source occurrence at origin 11 ─┐
                                ├─ same hash-consed TreeId
source occurrence at origin 29 ─┘

single SOURCE_ORIGIN property after both writes = 29
```

The first occurrence is lost. An origin set retains `[11, 29]` but cannot by
itself say which occurrence a particular evaluation path selected. A located
occurrence retains that selection. This is why the hybrid is required.

Regression tests live in
`crates/xtask/src/diagnostics_provenance.rs`.

## 3. Provenance micro-probe

Command:

```text
cargo run -p xtask -- diagnostics-provenance-probe \
  --iterations 250000 \
  --semantic-nodes 4096
```

The command reports:

- build and query duration for both representations;
- a representation-only byte estimate;
- deterministic checksums proving the query loops execute;
- the one-property overwrite result.

Timing is observational and must not become a cross-machine CI threshold.
Phase G3 must rerun the probe in release mode and pair it with full compiler
wall-time and peak-memory measurements before enabling production provenance.

Recorded local observation on 2026-07-28 (`arm64`, macOS 12.7.6):

| Cargo profile | Representation | Build | Query | Estimated bytes | Checksum |
|---|---|---:|---:|---:|---:|
| debug | dense origin sets | 9,347,208 ns | 10,535,083 ns | 1,146,904 | 31,249,875,000 |
| debug | located occurrences | 5,953,000 ns | 3,150,917 ns | 2,000,000 | 31,761,715,456 |
| release | dense origin sets | 5,444,042 ns | 755,375 ns | 1,146,904 | 31,249,875,000 |
| release | located occurrences | 1,589,667 ns | 124,042 ns | 2,000,000 | 31,761,715,456 |

These values are a representation comparison, not an end-to-end compiler
benchmark. They show the expected tradeoff on this sample: dense origin sets
use less estimated storage, while direct located-occurrence traversal is
faster. The hybrid decision does not depend on a timing threshold.

Representation cost model:

| Representation | Approximate payload | Precision |
|---|---:|---|
| one property | one `OriginId` per semantic node | loses repeated occurrences |
| dense origin sets | vector header per semantic node + one `OriginId` per occurrence | retains candidates, ambiguous without path |
| located occurrences | one `(TreeId, OriginId)` per active occurrence | exact selected occurrence |
| selected hybrid | dense origin sets + located handles only where needed | candidates plus exact blame on active paths |

## 4. Current surfaced-quality sample

This sample freezes the v1 quality cliff; it is not a claim that only these
failures exist.

| Stage/class | Representative path | Source label | Correct actionable occurrence | Typed facts in JSON | Actionable help | Typed category |
|---|---|---:|---:|---:|---:|---:|
| source/unresolved import | `SourceReaderError::UnresolvedImport` | yes | yes | no | yes | no |
| lexer/parser | `err_01_parse_missing_rhs.dsp` | yes | partial: parser stop token | no | no | no |
| eval/undefined symbol | `err_13_eval_undefined_symbol_alias_chain_nested.dsp` | yes | yes | no; facts are note strings | yes | no |
| propagate/split arity | `err_03_propagate_split_mismatch.dsp` | yes | yes | no; facts are note strings | yes | no |
| signal type/range | `rep_74_soundfile_basic.dsp` | no | no | no | no | no |
| signal-to-FIR transform | `err_fad_rad_temporal.dsp` | no | no | no | no | no |
| FIR verifier strict warning | CLI diagnostics channel fixture | no | no | no; FIR subcode is a note | no | no |
| backend emission | backend typed-error tests | no | no | no; backend subcode is a note | generic | no |
| compiler invariant | `CompilerError::MissingRoot` unit fixture | no | not applicable | no | report-oriented | no |

For these nine surfaced classes:

- source label coverage: 4/9 (44%);
- known-correct actionable occurrence: 3/9 (33%), with parser stop-token
  location recorded separately as partial;
- typed expected/actual facts in the v1 JSON schema: 0/9;
- directly actionable help: 4/9 (44%);
- typed user/option/environment/unsupported/bug category: 0/9.

Eval and propagate contain valuable facts, but v1 serializes them as prose
notes. G2 must not count parsed note prefixes as typed coverage.

## 5. Official-manual coverage matrix

| Manual class | Current owner | V2 owner | G0 expected quality target |
|---|---|---|---|
| syntax/delimiters | `parser` | `parser` + shared source/fix model | exact cause token and conservative edit |
| undefined symbol | `eval` | `eval` | use/definition labels, scope facts, ranked visible suggestion |
| sequential connection | `propagate` | `propagate` | operator label, typed A/B arities, equality rule |
| split connection | `propagate` | `propagate` | operator label, typed modulo facts, safe help |
| merge connection | `propagate` | `propagate` | operator label, typed multiple/remainder facts |
| recursive connection | `propagate` | `propagate` | both inequalities and recursion trace |
| route parameters | `eval`/`propagate` | triggering phase | parameter labels, expected forms/types |
| iteration | `parser`/`eval` | triggering phase | identifier/count label and constant/type facts |
| pattern redefinition | `eval` | `eval` | both definition sites |
| pattern loop/depth | `eval` | `eval` | call/evaluation trace and bug-safe truncation |
| soundfile part range | `sigtype` | `sigtype` | source label plus actual/required interval |
| delay boundedness | `sigtype`/`transform` | triggering phase | source label, inferred interval, recursive cause |
| table construction | `sigtype`/`transform` | triggering phase | table argument labels and required type/arity |
| duplicate UI path | UI/lowering owner | triggering phase | both widget sites and normalized path |
| math domain | normalize/type owner | triggering phase | operand range/value and error/warning distinction |
| FIR/backend limitation | `fir`/`codegen` | owning crate | source→Signal→FIR trace and alternative |
| compiler options | `compiler::execution` | `compiler::execution` | option labels/facts and compatible alternatives |
| warnings | emitting phase | emitting phase | source label, likelihood/range, non-fatal policy |

## 6. Rust-only extension matrix

| Class | Current owner | Required v2 context |
|---|---|---|
| clock-domain incompatibility | `propagate`/`transform` | source wrapper, parent/child domains, violated ordering |
| FAD/RAD unsupported boundary | `propagate` | differentiated expression, boundary node/domain, rewrite guidance |
| recursion/de-Bruijn coherence | `propagate`/`transform` | source recursion group, projection slot, producing pass |
| vector plan/certificate rejection | `transform` | source Signal origins, plan region/loop, invariant |
| FIR verification | `fir` | source origin, FIR node/function/variable, stable detail code |
| C++ backend | `codegen::cpp` | source/FIR origin and backend detail code |
| C backend | `codegen::c` | source/FIR origin and backend detail code |
| Julia backend | `codegen::julia` | source/FIR origin and backend detail code |
| AssemblyScript backend | `codegen::asc` | source/FIR origin and backend detail code |
| Codebox backend | `codegen::codebox` | source/FIR origin and backend detail code |
| Rust backend | `codegen::rust` | source/FIR origin and backend detail code |
| interpreter backend | `codegen::interp` | source/FIR origin and opcode/compiler context |
| Cranelift backend | `codegen::cranelift` | source/FIR origin and lowering/JIT context |
| Wasm backend | `codegen::wasm` | source/FIR origin and import/layout context |

## 7. Baseline contracts at G0

> G2 update (2026-07-28): the project owner subsequently authorized removal
> of JSON v1. The bullets below record the G0 baseline used to detect the
> intended breaking change; they no longer require a compatibility renderer.

G1 and G2 must preserve:

- all codes in `diagnostics::codes::all_codes()`;
- `docs/diagnostics-codes-en.md`;
- human and JSON snapshots in `crates/compiler/src/cli/tests.rs`;
- clean JSON channel tests in
  `crates/compiler/tests/cli_diagnostics_channel.rs`;
- negative diagnostic corpus and golden snapshots;
- current CLI exit status;
- standard pre-G2 JSON field names and note ordering, except for the explicitly
  authorized G2 replacement.

## 8. G0 pass result

- manual and Rust-only matrices have an owner and target quality;
- source-occurrence ambiguity has a structural regression test;
- both candidate representations have an executable measurement harness;
- the hybrid decision is recorded without changing production IR;
- baseline contracts and performance measurements required before G3 are
  explicit.

G1 may begin.
