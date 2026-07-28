# Compiler Diagnostics V2: Analysis and Improvement Plan

> Date: 2026-07-28
>
> Baseline: `main-dev` at `9ef96def`
>
> Scope: compilation diagnostics from source loading through backend emission
>
> Audience: Faust programmers, compiler developers, IDEs, and AI coding agents
>
> Status: proposed plan; no implementation is authorized by this document

## 1. Executive conclusion

`faust-rs` already reports several important classes of errors substantially
better than the historical C++ compiler. In particular, its eval and propagate
diagnostics can preserve:

- exact source ranges, including columns;
- primary and secondary locations;
- the definition that owns a failing expression;
- a binding path such as `process -> baz -> bar -> foo`;
- visible lexical scopes;
- the violated semantic rule;
- the values computed while checking that rule;
- a readable Box expression and a debug representation;
- deterministic help and correction templates;
- stable codes and a machine-readable JSON channel.

The next large improvement should not be a wording sweep. The limiting factor
is provenance loss across IR boundaries:

```text
source occurrence
    -> hash-consed Box
    -> evaluated/flat Box
    -> Signal
    -> normalized/prepared Signal
    -> FIR
    -> backend node
```

The front-end currently reconstructs source context late from parser
properties and tree-shape heuristics. This works well for many eval/propagate
errors, but type, signal-to-FIR, FIR-verifier, and backend errors often retain
only an internal node id or formatted string. Once this happens, a renderer
cannot recover the correct source occurrence reliably.

The recommended direction is therefore:

1. introduce an immutable compilation `SourceMap`;
2. define an explicit, occurrence-aware provenance model propagated across IR
   construction;
3. replace machine information encoded in note strings with typed diagnostic
   fields;
4. add structured traces and machine-applicable fixes;
5. improve terminal rendering only after the underlying information is
   reliable;
6. retain a bounded debug mode for IR-level evidence.

The objective is not to print the largest possible amount of text. It is to
provide the smallest complete explanation that identifies:

```text
what failed
why it failed
where the user can act
which values/rules led to the conclusion
how the failing expression was reached
what change is likely to repair it
whether the problem belongs to the DSP, options, environment, or compiler
```

## 2. Sources and method

This analysis uses:

- the current Rust diagnostic model in `crates/diagnostics`;
- parser location/origin handling in `crates/parser`;
- eval/propagate enrichment in
  `crates/compiler/src/diagnostic_enrichment.rs`;
- type, signal-to-FIR, FIR, and codegen error mapping in `crates/compiler`;
- the existing diagnostics roadmap in
  `porting/faust-rust-diagnostics-model-en.md`;
- the pinned C++ reference implementation, especially
  `compiler/errors/errormsg.cpp` and `compiler/parser/faustparser.y`;
- the official Faust manual chapter
  <https://faustdoc.grame.fr/manual/errors/>.

The official manual divides failures into syntax, Box connection,
pattern-matching, Signal typing/range, FIR/backend, option, and warning
classes. It also states the central historical limitation: when the compiler
cannot trace the origin back to DSP source, it prints an internal Box or Signal
expression without a file/line location.

That limitation is visible in the current Rust pipeline too, but at later
stages than in C++.

## 3. C++ baseline: semantic oracle, not UX ceiling

The historical C++ implementation is intentionally modest:

- `FAUSTerror` formats `filename:line` and throws;
- `evalerror` adds a pretty-printed Box expression;
- `setDefProp` and `setUseProp` attach one file/line property to a tree;
- errors are mostly strings carried by `faustexception`;
- columns, ranges, structured causes, stable codes, typed facts, related
  locations, and machine-applicable fixes are not first-class data.

The manual contains useful domain explanations that are richer than some raw
compiler messages:

- a missing semicolon may only become visible at the next token;
- each Box composition has a precise arity equation;
- iterative forms require an identifier and compile-time integer count;
- signal parameters can violate interval or boundedness requirements;
- math operations can be invalid at compile time or only potentially invalid
  at runtime;
- backend and option compatibility failures are distinct from DSP errors.

`faust-rs` should preserve the C++ compiler as the acceptance/rejection oracle,
while treating the manual's explanations as input to a better diagnostic
taxonomy. Byte-for-byte C++ error parity is neither required nor desirable.

## 4. Current Rust strengths

### 4.1 Canonical report model

`crates/diagnostics/src/lib.rs` already provides:

- `Severity`;
- stable `Stage`;
- stable `DiagnosticCode`;
- file/line/column ranges;
- primary/secondary labels;
- notes and help;
- deterministic aggregation through `DiagnosticBundle`;
- borrowing conversion through `ToDiagnostic`.

Every `CompilerError` now carries a bundle, and concrete operational causes are
available through `std::error::Error::source()` when one exists.

### 4.2 Exact parser and imported-file locations

The Rust parser extends the C++ `(filename, lineno)` model with start/end
columns. Structural import expansion keeps per-line source origins, allowing a
parser error in an imported file to point back to that file rather than to an
artificial expanded buffer.

### 4.3 Eval explainability

Representative undefined-symbol diagnostics already expose:

- the undefined identifier;
- local, visible, and top-level scope contents;
- a primary definition-site span;
- a secondary call-site span;
- the owner definition;
- a binding path from the configured entry point;
- a concrete definition template.

This is a strong model for AI consumption because it describes resolution
state instead of merely saying "undefined symbol".

### 4.4 Propagate explainability

Representative Box composition diagnostics already expose:

- the operator span (`:`, `<:`, `:>`, or `~`);
- readable A/B expressions;
- both arities;
- the algebraic rule;
- actual values and remainder/delta;
- a computed target;
- the owner definition and binding trace;
- operator-specific help.

For example, a split mismatch explains
`inputs(B) % outputs(A) == 0`, reports `3 % 2 = 1`, and proposes the next valid
input count.

### 4.5 Stable human and machine channels

The CLI provides:

- concise and debug human modes;
- a clean single-document JSON mode;
- stable `FRS-*` codes;
- `--check` for diagnostics without code generation;
- snapshot and golden coverage.

This is already a much better automation contract than parsing C++ exception
text.

## 5. Current gaps, by importance

### 5.1 Provenance is attached to semantic nodes, not source occurrences

`TreeArena` hash-conses structurally identical trees. Parser definition/use
properties are keyed by `TreeId`, and later writes replace earlier values.
Consequently, two identical expressions written at different locations can
share one semantic node but cannot retain two distinct occurrence locations
through the current single-value property.

This is not a parser bug; it is a representation mismatch:

```text
semantic identity: one hash-consed node
source identity: potentially many written occurrences
```

Choosing the "first labeled descendant" or searching for a definition whose
tree contains the node is useful fallback behavior, but it cannot establish
which occurrence actually caused a late failure.

### 5.2 Provenance is reconstructed late and heuristically

`diagnostic_enrichment.rs` performs bounded tree searches to find:

- a node or descendant with a span;
- an enclosing definition;
- an entry-point binding;
- a reference path.

These searches are intentionally capped at 4096 visited nodes and may return
`None`. More importantly, transformations can replace, fold, duplicate, or
synthesize nodes without recording why the new node exists. A late lookup can
then find only a nearby ancestor or structurally equal expression.

Source provenance must be propagated at construction time; it cannot always be
recovered afterward.

### 5.3 Source text is not part of the diagnostic session

`SourceSpan` carries a path and coordinates. The human renderer reopens that
path when it needs a snippet. This has four consequences:

- in-memory sources such as `<memory>` cannot be rendered;
- virtual library sources may have no host file;
- a file can change between compilation and rendering;
- a diagnostic result is not fully self-contained for a remote/AI consumer.

The parser already has the source text at the right time. It should enter an
immutable `SourceMap` owned by the compilation session.

### 5.4 Coordinate semantics are not explicit enough

The model documentation describes inclusive spans, while parser and caret
logic behave like half-open end coordinates in several paths. Parser columns
are derived from byte offsets, while at least one import-location path counts
characters. Terminal display columns, Unicode scalar columns, UTF-8 byte
offsets, and LSP UTF-16 columns are not equivalent.

The next schema must make one coordinate system authoritative:

- immutable UTF-8 source;
- half-open byte range as canonical identity;
- derived 1-based Unicode/display line and column for humans;
- derived 0-based UTF-16 line and character for LSP clients.

### 5.5 The human renderer discards valid labels

The JSON renderer exports all labels, but the human renderer displays only the
first label and one source line. It does not render:

- secondary definition/call sites;
- multi-line spans;
- import-chain frames;
- multiple files;
- related declarations;
- tab-aware or wide-Unicode carets.

This makes the terminal less informative than the data already available.

### 5.6 Structured machine data is encoded in prose notes

The current JSON renderer derives fields by parsing note prefixes:

- `binding_trace=...`;
- `scope.local=...`;
- `node_id=...`;
- `box_expr=...`;
- exact label text such as `"call site"`.

FIR and codegen subcodes likewise travel as `fir_code=...` or
`codegen_code=...` notes.

This is fragile for tools and AI agents. Wording should be free to improve
without changing machine meaning. Note strings should remain presentation
data, not a hidden serialization protocol.

### 5.7 Diagnostic quality drops sharply after propagate

Two current examples demonstrate the cliff.

Signal type validation can produce:

```text
FRS-COMP-0004
stage: compiler
labels: []
help: []
message: ... SIGSOUNDFILELENGTH(SIGSOUNDFILE(...), SIGINPUT(...))
```

Signal-to-FIR preparation can produce:

```text
FRS-SFIR-0004
stage: transform
labels: []
help: []
message: projection 12 does not target symbolic recursion
```

The programmer needs the originating Faust expression, the source location,
the semantic rule, and whether the problem is unsupported source, an option
limitation, or an internal compiler invariant.

The causes are visible in the types:

- `InferenceError` is still a one-field `String`;
- `SignalFirError` stores only `(code, message)`;
- FIR verifier diagnostics retain a `FirId` plus FIR names, but no source
  origin;
- backend errors retain a backend code and message, but usually no offending
  FIR node or source origin.

### 5.8 Some source/import context is flattened

`ParseOutput` still exposes raw parser strings in addition to the canonical
bundle. `EvalError::SourceParseFailure` stores loaded-file parse failures as
`Vec<String>`, so nested component/library parsing can lose labels and stable
codes.

An import cycle reports the repeated path, but not yet the complete cycle with
one labeled `import(...)` site per edge.

### 5.9 Suggestions are human text, not edits

Current help is often excellent, but an IDE or AI still has to infer:

- which source range to replace;
- the replacement text;
- whether the edit is guaranteed, likely, or only illustrative;
- whether several edits must be applied atomically.

No current schema distinguishes a machine-applicable fix from a general
explanation.

### 5.10 More text can reduce usefulness

Parser recovery can emit cascades, and later phases can expose internal
invariants triggered by one upstream cause. A "maximum information" mode must
not simply dump every fact.

Diagnostics need causal grouping and suppression metadata so consumers can
prioritize the root issue while retaining related evidence on demand.

## 6. Coverage against the official error manual

| Manual class | Current Rust quality | Main remaining work |
|---|---|---|
| syntax and delimiters | precise ranges and stable codes | expected-token structure, previous-token blame, delimiter pairing, edits |
| undefined symbols | strong scope and binding context | ranked near-name suggestions and exact rename edits |
| Box connections | strong rule/computed/help model | fully occurrence-correct locations and typed A/B facts |
| iteration/pattern matching | typed eval failures | complete node/span coverage, matched-rule trace, non-cascading reports |
| imports/components/libraries | searched paths and some spans | complete import chain, nested structured bundles, virtual-source snippets |
| Signal types/ranges | mostly internal Signal text | typed error variants, source provenance, intervals as facts, source expression |
| math domain checks/warnings | partial and phase-specific | structured compile-time errors and possible-runtime warnings |
| duplicate UI paths | semantics exist in pipeline | both declarations labeled, computed normalized path, rename fix |
| FIR verifier | stable wrapper/subcode | source origin chain and typed FIR context |
| backend/options | stable backend/subcodes | error classification, offending FIR/source node, alternative option/backend fixes |
| compiler bugs/cancellation | typed facade distinctions | explicit category, reproducibility bundle, no user-blaming help |

## 7. Target architecture

### 7.1 Immutable source map

One compilation session should own:

```rust
struct SourceMap {
    sources: Vec<SourceFile>,
}

struct SourceFile {
    id: SourceId,
    display_name: PathBuf,
    canonical_path: Option<PathBuf>,
    text: Arc<str>,
    content_hash: ContentHash,
    line_index: LineIndex,
    origin: SourceKind, // file, memory, virtual library, generated
}
```

Diagnostics should reference `SourceId` plus a canonical half-open byte range.
Paths and human/LSP coordinates become derived views.

This makes snippets deterministic and supports file, memory, Wasm, virtual
library, IDE, and remote compilation uniformly.

### 7.2 Explicit origin graph

Use a compact origin arena:

```rust
struct OriginId(u32);

enum Origin {
    Direct {
        span: SourceRange,
        role: OriginRole,
    },
    Derived {
        pass: PassId,
        operation: DerivationKind,
        inputs: SmallVec<[OriginId; 2]>,
    },
    Synthetic {
        pass: PassId,
        owner: Option<OriginId>,
        reason: SyntheticReason,
    },
}
```

The graph records both direct source locations and how a transformed node was
derived. It must be bounded and cycle-safe.

### 7.3 Occurrence-aware IR references

A single source span must not be stored as a property of a hash-consed
semantic node. The prototype must compare two representations:

1. `NodeId -> SmallVec<OriginId>` origin sets, accumulating all possible
   occurrences;
2. `Located<NodeId> { node, origin }` occurrence handles on phase worklists and
   boundaries.

Origin sets are cheaper and improve current behavior immediately. Located
occurrences are precise when the same interned node appears in different
source/evaluation contexts. Phase G0 must benchmark both before the project
commits to a pervasive API change.

The expected target is a hybrid:

- builders preserve semantic hash-consing;
- phase traversal carries an occurrence origin where ambiguity matters;
- each produced Box/Signal/FIR value records a derived origin;
- shared semantic nodes may have several candidate origins;
- diagnostics retain the selected occurrence plus alternative related
  origins when selection is ambiguous.

### 7.4 Typed diagnostic payload

Keep the current core fields, then add typed structure rather than more note
prefixes:

```rust
struct Diagnostic {
    severity: Severity,
    stage: Stage,
    code: DiagnosticCode,
    detail_code: Option<DetailCode>,
    category: DiagnosticCategory,
    message: Box<str>,
    labels: Vec<Label>,
    facts: BTreeMap<FactKey, DiagnosticValue>,
    traces: Vec<DiagnosticTrace>,
    fixes: Vec<SuggestedFix>,
    related: Vec<RelatedDiagnostic>,
    notes: Vec<Box<str>>,
    debug: Option<DebugContext>,
}
```

Suggested categories:

- `UserCode`;
- `UnsupportedFeature`;
- `InvalidOptions`;
- `Environment`;
- `Cancelled`;
- `CompilerBug`.

Suggested typed facts include:

- `expected`, `actual`, `delta`, `remainder`;
- `left_inputs`, `left_outputs`, `right_inputs`, `right_outputs`;
- `interval_actual`, `interval_required`;
- `symbol`, `visible_symbols`;
- `backend`, `backend_code`;
- `fir_code`, `fir_node`, `fir_function`, `fir_variable`;
- `pass`, `invariant`;
- `search_paths`;
- `clock_domain`;
- `recursion_group` and projection slot.

Existing stable `FRS-*` codes must not be renumbered. A typed `detail_code`
replaces subcodes hidden in notes without breaking the top-level registry.

### 7.5 Typed label roles

Replace role inference from label message text with:

```rust
enum LabelRole {
    PrimaryCause,
    UseSite,
    DefinitionSite,
    CallSite,
    Operator,
    ExpectedHere,
    ConflictsWith,
    ImportSite,
    PreviousToken,
    MatchingDelimiter,
    DerivedFrom,
}
```

The user-facing label message remains independent prose.

### 7.6 Structured traces

Do not overload one `binding_trace` string. Support typed frames:

```rust
enum TraceKind {
    Binding,
    Import,
    Expansion,
    Evaluation,
    Transformation,
    Causal,
}

struct TraceFrame {
    name: Option<Box<str>>,
    span: Option<SourceRange>,
    ir: Option<IrReference>,
    description: Box<str>,
}
```

Examples:

- `process -> synth -> filter -> onePole`;
- `main.dsp -> stdfaust.lib -> filters.lib`;
- source Box -> evaluated Box -> Signal -> FIR instruction;
- recursive projection -> recursion group -> generated state field.

### 7.7 Machine-applicable fixes

Add:

```rust
struct SuggestedFix {
    title: Box<str>,
    applicability: Applicability,
    edits: Vec<TextEdit>,
    explanation: Option<Box<str>>,
}

enum Applicability {
    MachineApplicable,
    MaybeIncorrect,
    HasPlaceholders,
    Manual,
}
```

Only deterministic fixes should be marked machine-applicable. Examples:

- insert a missing `;`;
- replace a misspelled identifier with one unambiguous visible symbol;
- remove one unmatched delimiter;
- change one invalid literal;
- add `-I <dir>` is a command/config fix, not a source edit.

Arity rewrites are usually explanatory or placeholder fixes, not guaranteed
edits.

### 7.8 Versioned AI/tool envelope

Add a v2 envelope while preserving the current v1 JSON contract during
migration:

```json
{
  "schema_version": 2,
  "compiler": {
    "name": "faust-rs",
    "version": "...",
    "target": "..."
  },
  "request": {
    "mode": "check",
    "backend": null,
    "normalized_options": []
  },
  "status": "failed",
  "sources": [],
  "diagnostics": []
}
```

For reproducibility and privacy:

- use source ids inside diagnostics;
- include source text only when requested or already supplied in-memory;
- permit repository-relative/redacted path rendering;
- keep internal ids/IR dumps in explicit debug/full modes;
- include content hashes so an agent can detect stale diagnostics;
- define deterministic ordering for sources, facts, labels, traces, and fixes.

## 8. Human rendering target

The standard human rendering should use progressive disclosure:

1. error category, stable code, and concise root message;
2. primary source snippet with exact operator/token;
3. secondary snippets only when they explain the cause;
4. one rule line;
5. concrete computed facts;
6. shortest safe fix;
7. a compact trace when crossing definitions/imports;
8. debug IR only under `--error-verbosity debug`.

Example target for a signal interval failure:

```text
foo.dsp:18:27: error [FRS-COMP-0004] soundfile part may be outside 0..255
  18 | process = _, part : soundfile("foo.wav", 2);
     |                ^^^^ this signal has interval [-1, 1]
     |
     = rule: soundfile part must stay within [0, 255]
     = computed: inferred interval = [-1, 1]
     = trace: process -> player -> soundfile part
     = help: clamp or otherwise prove the part signal is within 0..255
```

The internal `SIGSOUNDFILELENGTH(...)` form belongs in debug output:

```text
     = debug: signal 42 = SIGSOUNDFILELENGTH(...)
```

The renderer should support:

- multi-line source ranges;
- all relevant labels, including different files;
- tab expansion and Unicode display width;
- bounded context lines and elision;
- color when enabled, no color in snapshots/JSON;
- clear distinction between a DSP error, unsupported feature, bad option,
  missing environment resource, and compiler bug.

## 9. AI-oriented diagnostic target

An AI coding agent benefits from different information than a terminal user.
The v2 JSON should answer these questions without parsing prose:

| Question | Structured answer |
|---|---|
| What failed? | `code`, `detail_code`, `category`, `stage` |
| Where should I edit? | typed primary label with source id and byte range |
| Is another location involved? | typed secondary labels and related diagnostics |
| What rule was violated? | rule id plus typed facts |
| How was this code reached? | binding/import/transform trace frames |
| What did the compiler infer? | types, intervals, arities, domains, constants |
| Can I apply a fix automatically? | applicability plus exact text edits |
| Is the error caused by my DSP? | diagnostic category |
| Is this result stale? | source content hash |
| What extra evidence exists? | opt-in debug context and bounded IR slices |

AI-oriented output should also avoid two traps:

- do not offer a plausible edit as guaranteed when it changes DSP semantics;
- do not drown the root cause in hundreds of derived diagnostics.

## 10. Implementation plan

### G0 — Freeze the diagnostics-quality baseline

Deliverables:

- build a negative corpus matrix from every class in the official manual;
- add representative current Rust-only failures (clock domains, FAD/RAD,
  vector planning, FIR verification, every backend);
- measure per-stage:
  - percentage with a source label,
  - percentage whose primary label is the correct occurrence,
  - percentage with typed expected/actual facts,
  - percentage with actionable help,
  - percentage classified as user/option/environment/unsupported/bug;
- freeze v1 human/JSON snapshots and stable codes;
- benchmark compilation time and memory before provenance tracking;
- prototype origin sets versus located occurrences on repeated identical
  expressions, aliases, imports, generated iteration bodies, and recursion.

Pass criteria:

- the matrix names one owner and expected quality level per error class;
- source-occurrence ambiguity is reproduced by a structural test;
- provenance prototype results and performance costs are recorded;
- no large IR API decision is taken before the benchmark.

Suggested commit:

```text
Freeze compiler diagnostics v2 baseline
```

### G1 — Introduce SourceMap and coordinate contracts

Deliverables:

- add immutable `SourceMap`, `SourceId`, `SourceRange`, content hash, and line
  index;
- register file, memory, virtual-library, and imported sources;
- make half-open UTF-8 byte ranges canonical;
- derive human display and LSP UTF-16 positions;
- retain a compatibility conversion to existing `SourceSpan`;
- add fixtures with tabs, combining characters, non-ASCII identifiers, CRLF,
  and multi-line ranges.

Pass criteria:

- snippets work for file, memory, and virtual sources;
- rendering uses the exact compiled source snapshot even if the file changes;
- human and LSP coordinates are correct on the Unicode/CRLF corpus;
- the G1 source-map change alone does not alter the then-current JSON output.

Suggested commit:

```text
Add immutable diagnostic source maps
```

### G2 — Add diagnostics schema v2

Deliverables:

- add typed category, detail code, facts, label roles, traces, fixes, related
  diagnostics, and debug context;
- stop deriving machine fields from note strings;
- add `schema_version`;
- replace the previous unversioned JSON payload with schema v2 under the
  existing `--error-format json` spelling;
- publish a JSON Schema and examples;
- keep current `FRS-*` codes frozen.

Pass criteria:

- v2 consumers never need to parse note/help prose;
- schema validation passes for every negative corpus entry;
- deterministic serialization is cross-platform.

Compatibility decision (2026-07-28):

- the project owner explicitly authorized removal of JSON v1;
- there is no parallel `json-v2` CLI value and no v1 renderer to maintain;
- this is an intentional machine-channel breaking change before an external
  stability commitment;
- stable `FRS-*` meanings, human diagnostics, exit status, C/C++ and Wasm API
  behavior, and successful non-diagnostic output remain unchanged.

Suggested commit:

```text
Add typed diagnostics v2 schema
```

### G3 — Make Box provenance occurrence-aware

Deliverables:

- implement the G0-selected occurrence/provenance representation;
- record direct origins for grammar-produced Box occurrences;
- propagate origins through definition grouping, eval, pattern matching,
  iteration expansion, local definitions, aliases, and Box simplification;
- replace late subtree ownership guesses with recorded origins when available;
- retain explicit fallback notes when provenance is incomplete.

Pass criteria:

- identical hash-consed expressions at different source sites report the
  correct occurrence;
- nested `with`/`letrec`/`case` and iteration-generated failures trace to the
  actionable source construct;
- imported definitions keep both definition and call/import frames;
- TreeArena time/memory remain within the limits decided in G0.

Suggested commit:

```text
Track occurrence provenance through Box evaluation
```

### G4 — Carry provenance through Signal typing

Deliverables:

- attach derived origins when Box propagation constructs Signal nodes;
- propagate origin sets through signal normalization, recursion conversion,
  AD transforms, and preparation;
- add an explicit type-inference stage in the v2 stage taxonomy instead of
  classifying these failures as generic compiler orchestration;
- replace `InferenceError(String)` with typed variants carrying offending
  `SigId`, rule, expected/actual type or interval, and relevant operands;
- emit Signal/type detail codes and typed facts;
- map internal Signal expressions to debug context, not the primary message.

Pass criteria:

- the manual's soundfile-part, delay-bound, table, and math-domain failures
  point to Faust source;
- type diagnostics expose actual/required intervals or types structurally;
- recursive errors include the relevant group/projection trace;
- no public failure exposes only a raw `SIG...` expression in standard mode.

Suggested commits:

```text
Carry source provenance into Signal IR
Type signal inference failures structurally
```

### G5 — Carry provenance into FIR and backends

Deliverables:

- attach source/Signal origins to emitted FIR values and statements;
- preserve derivation through scheduling, vector planning, lifecycle
  placement, and optimization;
- let FIR verifier diagnostics reference origin ids;
- let backend errors optionally identify an offending `FirId`;
- promote `fir_code` and `codegen_code` from note protocols to typed detail
  codes;
- classify unsupported backend constructs separately from compiler invariants.

Pass criteria:

- a verifier/backend failure can show source -> Signal -> FIR trace;
- unsupported source constructs point to the Faust construct and propose a
  supported backend or rewrite when known;
- compiler bugs identify the failing pass/invariant and request a reproducible
  report without blaming DSP syntax;
- FIR/backend debug evidence is bounded and deterministic.

Suggested commits:

```text
Preserve origins through FIR construction
Connect verifier and backend errors to source
```

### G6 — Improve parser and import recovery diagnostics

Deliverables:

- expose expected tokens as typed data;
- identify the most likely previous-token cause for missing semicolons;
- track delimiter opening/closing pairs;
- add conservative typo suggestions for keywords and identifiers;
- preserve nested component/library parse bundles instead of `Vec<String>`;
- record complete import-cycle paths with one location per import edge;
- suppress parser cascades while retaining related recovery evidence.

Pass criteria:

- official syntax examples point to the actionable token, not only the token
  where parsing finally failed;
- unambiguous missing delimiters/semicolons offer machine-applicable edits;
- nested imported-file parse errors retain their original codes and labels;
- import cycles render the full cycle deterministically.

Suggested commit:

```text
Make parser recovery diagnostics actionable
```

### G7 — Complete semantic guidance and warning coverage

Deliverables:

- add ranked nearest-symbol suggestions using only visible scopes;
- give pattern matching a rule/attempt trace without exposing the full
  evaluator state;
- label both conflicting UI path declarations;
- add structured compile-time math-domain errors;
- add opt-in potential-runtime domain warnings equivalent in intent to the
  C++ `-wall -me` class;
- normalize cause/rule/computed/context/help ordering across stages;
- add exact edits only when semantics are unambiguous.

Pass criteria:

- every high-frequency user-code diagnostic has a rule, relevant facts, and
  at least one safe help item;
- warnings are distinguishable from compile-blocking errors;
- duplicate declarations show both sites;
- no suggestion names a symbol outside the actual visible scope.

Suggested commit:

```text
Complete semantic diagnostic guidance
```

### G8 — Upgrade renderers and AI/tool integration

Deliverables:

- render all useful labels, multi-line spans, traces, and fixes in human mode;
- provide concise, standard, debug, and full-trace verbosity policies;
- add path redaction/repository-relative display;
- publish JSON Schema, compatibility rules, and AI consumption examples;
- add optional SARIF/LSP adapters outside the core model;
- add an `xtask diagnostics-quality-check` that enforces coverage metadata and
  forbids new note-prefix machine protocols.

Pass criteria:

- standard human output is shorter than debug output but contains the complete
  actionable cause;
- every v2 payload validates against the published schema;
- an automated test can apply machine-applicable edits and verify that the
  targeted diagnostic disappears without introducing a new parse error;
- Linux/macOS/Windows snapshots are stable;
- mandatory workspace, golden, and differential gates pass.

Suggested commit:

```text
Finalize human and machine diagnostic rendering
```

## 11. Validation strategy

### 11.1 Negative corpus dimensions

Each phase must cover combinations of:

- direct source versus imported/virtual source;
- one-line versus multi-line construct;
- ASCII, tabs, Unicode, and CRLF;
- direct definition versus alias chain;
- repeated structurally identical expressions at distinct sites;
- local scope, `with`, `letrec`, `case`, iteration, and recursion;
- scalar/vector and optimized/unoptimized flows where the error phase differs;
- user error, unsupported feature, invalid option, environment error,
  cancellation, and compiler invariant.

### 11.2 Accuracy assertions

Tests should assert more than code presence:

- exact primary source occurrence;
- label roles and order;
- typed rule/facts;
- complete trace frames;
- fix applicability;
- no internal-only expression in standard output when source is available;
- fallback reason when source is genuinely unavailable.

### 11.3 Differential policy

For each C++-shared failure:

- acceptance/rejection class must match C++;
- the Rust diagnostic may be richer;
- wording need not match;
- Rust source anchoring must never knowingly point to an unrelated occurrence.

### 11.4 Performance policy

G0 must establish explicit budgets before provenance implementation. Measure:

- parser and full compile wall time;
- peak resident memory;
- origin count and average parents per derived origin;
- ambiguous origin-set frequency;
- serialized diagnostic size on errors;
- successful-compilation overhead.

The always-on path should store compact origin ids. Expensive full derivation
detail may be retained only under a diagnostic trace mode if the benchmark
shows material production cost.

## 12. Compatibility

### Stable contracts to preserve

- existing `FRS-*` code meanings and numbers;
- current CLI exit status;
- C/C++ and Wasm compiler API behavior;
- C++ acceptance/rejection parity;
- default successful-compilation output.

### Intentional adapted Rust APIs

- source coordinates move toward `SourceId + byte range`;
- late-stage errors gain typed variants and offending IR ids;
- diagnostic context becomes typed instead of encoded in notes;
- provenance accompanies IR construction without changing semantic
  hash-consing.

Every public change must be documented as `adapted`, with a compatibility
period where appropriate.

## 13. Risks and mitigations

| Risk | Mitigation |
|---|---|
| provenance increases successful compile cost | compact ids, G0 benchmark, optional full trace |
| hash-consing conflates occurrences | occurrence handles or origin sets selected by measured prototype |
| origin graph becomes cyclic or huge | arena ids, bounded parents, cycle checks, deterministic truncation |
| transformations propagate misleading blame | explicit derivation kinds and multi-origin ambiguity, never fabricate one exact site |
| schema v2 breaks current tools | one documented pre-stability break; publish schema/version and reject unknown consumers explicitly |
| automated fixes alter DSP semantics | strict applicability levels; only deterministic edits are machine-applicable |
| rich output overwhelms humans | progressive disclosure and standard/debug/full modes |
| absolute paths/source text leak | redaction policy, source ids, opt-in source embedding |
| stale locations after file edits | immutable source snapshots and content hashes |
| generic diagnostic crate absorbs phase semantics | phase crates keep typed errors; diagnostics owns only report vocabulary |

## 14. Non-goals

- Do not reproduce C++ strings byte-for-byte.
- Do not put all operational errors into `diagnostics`.
- Do not attach one mutable source span directly to a hash-consed semantic
  node and call the provenance problem solved.
- Do not infer machine fields by parsing human prose.
- Do not promise automatic repairs for semantic design choices.
- Do not expose unlimited IR dumps or evaluator state by default.
- Do not delay semantic parity work for cosmetic wording changes.
- Do not describe a nearby fallback span as the exact origin.

## 15. Recommended order and stopping points

The highest-value sequence is:

```text
G0 baseline
  -> G1 SourceMap
  -> G2 schema v2
  -> G3 Box occurrence provenance
  -> G4 Signal/type provenance
  -> G5 FIR/backend provenance
  -> G6 parser/import intelligence
  -> G7 semantic guidance/warnings
  -> G8 renderers/tooling
```

G0 through G4 form the first meaningful milestone. At that point, the largest
user-facing quality cliff — precise eval/propagate diagnostics followed by
source-less Signal/type diagnostics — should be closed.

G5 closes the end-to-end source-to-backend chain. G6 through G8 then convert
the reliable information into better recovery, fixes, warnings, terminal UX,
and AI integrations.

The central design rule for all phases is:

> Preserve provenance when constructing an IR value; do not assume it can be
> reconstructed correctly when an error is finally rendered.
