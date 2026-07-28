# Compiler diagnostics v2 — G7 semantic guidance and warnings

**Date:** 2026-07-28

**Baseline:** `d4b7d9ab` (`Make parser recovery diagnostics actionable`)

## Objective

G6 made the parser's own recovery actionable. G7 does the same for the
semantic phases: it turns the facts eval, propagation, and type inference
already hold into ranked guidance, exact edits where the meaning is
unambiguous, and a warning class that reports risks instead of blocking.

It also closes one acceptance-parity gap: duplicated user-interface addresses,
which C++ rejects and `faust-rs` silently accepted.

## Near-name suggestions

`crates/eval/src/suggestions.rs` ranks candidates by bounded
Damerau-Levenshtein distance, ordered by `(distance, name)` so the result never
depends on hash iteration. The edit budget is length-relative — one edit for
two-to-four characters, two beyond that, none for a single character, where
every other one-character name would be a "match".

Candidates come only from the scopes the error already recorded (`local`,
`visible`, `top_level` for an unresolved identifier; the top-level definition
list for a missing entry point). A suggestion therefore cannot name a symbol
the programmer is unable to reach from the failing site.

The evaluator publishes the ranking as the `suggested_symbols` and
`suggestion_distance` facts. It deliberately does not build the edit: an exact
rename needs the use-site range, which only the compiler facade owns.

`crates/compiler/src/eval_guidance.rs` adds the edit, and only when the best
candidate is strictly closer than the runner-up. Two equally close names mean
the compiler cannot know which was meant, so no edit is offered. Even a
distance-one match stays `MaybeIncorrect`: reachability is proven, intent is
not.

## Pattern-match attempt traces

`EvalError::PatternMatchFailed` now carries the arguments the matcher actually
dispatched on, in application order, including the one that killed the last
surviving rule.

The compiler renders a `TraceKind::Evaluation` trace from the rules tree the
error already points at: one frame for the provided arguments, then one per
declared rule with its pattern list. Rule lists and pattern lists are both
stored reversed by the evaluator and are re-reversed for display, and a pattern
variable renders as its bare identifier rather than as the internal
`BOXPATVAR` wrapper. The trace describes the program, never evaluator
environments or automaton state.

The accompanying `computed:` note reports how many arguments were consumed
before every rule died — which is not the same as how many the call supplied,
because the matcher fails as soon as no rule survives.

## Duplicate declarations

Two duplicate-declaration diagnostics now read the same way: the later
declaration is primary because it introduced the conflict, and earlier ones are
`ConflictsWith` context.

**Symbols.** `report_zero_arity_redefinition` previously folded the conflicting
clauses into a multi-line message and labeled whatever token the cursor
happened to sit on. The message is now one line; the clauses became the typed
`declarations` fact plus one note each, and every participating declaration
site is labeled. `ParserDiagnostic` gained typed `detail_code`,
`related_sites`, `facts`, `notes`, and `help` fields to carry this without a
second diagnostic channel.

**UI addresses.** `ui::find_duplicate_control_paths` walks the grouped
`UiProgram` and reports every runtime `/group/.../label` address claimed more
than once. Checking the `UiProgram` rather than the JSON serializer makes
rejection depend on the program instead of on whether JSON is generated.

The C++ outcome depends on which control families collide, and
`DuplicatePathKind` reproduces it exactly: an input control against anything is
an error (`FRS-UI-0001`), while bargraph against bargraph is only ambiguous and
does not reject. Anonymous controls are excluded, because C++ renames unlabeled
widgets (`0x00`, `vbargraph0`, ...) before they can ever collide and `faust-rs`
has not ported that naming; without the exclusion the check would reject
programs C++ accepts.

Widget boxes are rebuilt during evaluation, so the hash-consed node the UI
builder sees is not the node the grammar produced and box provenance cannot be
followed across that boundary. The grammar therefore records written widget
declarations separately (`ParserCtx::widget_declarations`), and the diagnostic
labels the declarations whose effective label — after group-path and metadata
stripping — matches the conflicting one. A declaration expanded several times
is labeled once, and when nothing matches, the diagnostic keeps its typed facts
and emits no label rather than pointing at a nearby span.

## Potential out-of-domain warnings

`infer_domain_math` now takes the operation's domain explicitly, which splits
three cases that were previously two:

- a compile-time constant entirely outside the domain stays an error;
- an operand merely straddling the boundary becomes
  `InferenceWarning::PotentialMathDomain`, the class C++ reports as
  `WARNING : potential out of domain in sqrt(...)`;
- an operand fully inside the domain is silent.

Warnings are deduplicated, so a shared sub-expression inferred once per
reference reports one line. They reuse the source-labeling and typed-fact
vocabulary of the type errors, so a warning and an error about the same rule
read identically.

`Compiler::with_semantic_warnings` gates collection and `--warn` gates the CLI,
matching the C++ policy of reporting this class only on request. The CLI runs
one extra front-end pass under `--warn` rather than threading a bundle through
every artifact type; the cost is confined to the opt-in flag and all output
modes behave identically. Warnings go to stderr in both formats: on success
stdout carries generated output, and under `--error-format json` it is reserved
for the single diagnostics document the D1 contract promises.

## Canonical note order

`DiagnosticBundle::push` sorts notes into `cause`, `rule`, `computed`,
`suggested target`, then context. The sort is stable, so notes sharing a rank
keep their producer's order — the ranks impose a skeleton, not a total order.
Ordering once at the bundle boundary means a stage can add notes in whatever
order is convenient and every consumer still sees the same shape.

## API mapping

| Surface | Mapping | Compatibility impact |
| --- | --- | --- |
| Faust acceptance for duplicated input-control paths | `1:1` | now rejected, matching C++ |
| Faust acceptance for duplicated bargraph paths | `1:1` | still accepted, matching C++ |
| `FRS-UI-0001` | Rust extension | new stable code |
| `CompilerError::UiLayout` | Rust extension | new variant |
| `EvalError::PatternMatchFailed` | `adapted` | gains dispatched arguments |
| `ui::ControlSpec` | `adapted` | gains `source_node` |
| `sigtype::InferenceWarning` | Rust extension | non-fatal inference observations |
| `--warn` | `adapted` | covers the C++ `-wall` / `-me` domain class |
| note ordering | Rust extension | presentation only, no machine meaning |

`CompilerError`, `EvalError`, and `ControlSpec` are source-breaking for
exhaustive external matches and struct literals; `ControlSpec::synthetic`
exists for callers with no source declaration. Stable diagnostic codes, CLI
exit status, generated code, and external C/C++ ABI surfaces are unchanged.

## Failure modes and mitigation

| Risk | Mitigation |
| --- | --- |
| a rename edit changes DSP semantics | edits stay `MaybeIncorrect` and require a strictly closest candidate |
| a suggestion names an unreachable symbol | candidates come only from recorded scopes |
| the new UI check rejects programs C++ accepts | bargraph-only collisions excluded; anonymous controls excluded; whole corpus rescanned |
| a UI label points at an unrelated widget | labels require a written declaration whose effective label matches |
| domain warnings drown a valid DSP | opt-in flag, deduplicated, never affects exit status |
| `--warn` doubles front-end work | confined to the flag; documented at the call site |
| a pattern trace leaks evaluator state | frames are built from the rules tree, not from environments |
| note reordering hides a producer's intent | stable sort inside each rank |

## Structural coverage

- a misspelled identifier yields a ranked suggestion and an exact rename range;
- two equally close candidates yield no edit;
- a name local to another definition is never suggested;
- a failed `case` exposes its rules in written order plus an evaluation trace;
- a redefined symbol labels both clauses and names the symbol as a fact;
- two input controls at one address are rejected with both declarations
  labeled, and two bargraphs at one address still compile;
- `sqrt` over a signed input warns only under `--warn`;
- notes are emitted in canonical order.

## Incidental cleanup

`tests/cli-transcripts` had drifted from the compiler since G2: the
`--error-format json` doc comment added there and the JSON `meta` parity fix
landed later were never re-recorded, leaving 36 stale transcripts before this
phase. They are regenerated here, so `cli-transcript-check` is green again and
the next phase starts from an accurate baseline.

## Phase gate

G7 passes when formatting, Rustdoc, Clippy, workspace tests, golden snapshots,
CLI transcripts, and structural checkers are green, and when a full corpus scan
shows the new UI check rejects nothing C++ accepts. G8 can then upgrade the
renderers without reopening any of this: every fact, label, trace, and fix G8
needs to display is now produced.
