# The faust-rs Error Model

**Audience:** Faust programmers, tool authors, and the LLM agents that now write
a large share of Faust code.

**What this document is.** A complete description of how `faust-rs` reports
problems: what it can tell you that the reference C++ compiler cannot, which
levels of detail exist and who each one is for, how the JSON channel works and
where it is consumed (CLI, CI, editors, MCP servers), and a worked example of
every error family. The frozen `FRS-*` code table — previously this document's
only content — is now the reference section at the end.

The reference for the C++ behaviour compared against throughout is the official
manual chapter: <https://faustdoc.grame.fr/manual/errors/>.

---

## 1. Why the model changed

### 1.1 The C++ baseline

The historical Faust compiler reports a failure by formatting a string and
throwing it. That is enough for a human at a terminal who wrote the file two
minutes ago, and it has served the language well. But it has a ceiling, and the
manual states the central limitation itself: when the compiler cannot trace an
error back to the DSP source, it prints an internal Box or Signal expression
with no file and no line.

Concretely, for this program:

```faust
A = _,_;
B = _,_,_;
process = A : B;
```

the C++ compiler prints:

```text
ERROR : sequential composition A:B
The number of outputs [2] of A must be equal to the number of inputs [3] of B

Here  A = _,_;
has 2 outputs

while B = _,_,_;
has 3 inputs
```

The explanation is good. What is missing is everything a machine needs: no file,
no line, no column, no stable code, no typed values — the arities are inside the
prose. A tool that wants to place a squiggle in an editor, or an agent that
wants to know whether to change `A` or `B`, has to parse English.

### 1.2 What that costs today

This is not hypothetical. The Faust MCP servers that exist today
([`orlarey/faustcode`](https://github.com/orlarey/faustcode),
[`grame-cncm/faustbrowser-mcp`](https://github.com/grame-cncm/faustbrowser-mcp))
both compile through `libfaust-wasm` — the C++ compiler — and can only pass its
output through. Asking `faustbrowser-mcp` to check the program above returns:

```json
{
  "status": "error",
  "error": "ERROR : sequential composition A:B\nThe number of outputs [2] of A must be equal to the number of inputs [3] of B\n\nHere  A = _,_;\nhas 2 outputs\n\nwhile B = _,_,_;\nhas 3 inputs\n"
}
```

One opaque string. The agent receiving this must regex out the arities to know
what to change, and has no location at all to change it *at*. For a misspelled
identifier the same tool returns `"doc-undefined:3 : ERROR : undefined symbol :
cutof\n"` — a line number, embedded in prose, and no hint that `cutoff` exists
two lines above.

### 1.3 What faust-rs does instead

`faust-rs` keeps the C++ compiler as the **acceptance oracle** — a program
rejected by one is rejected by the other, and vice versa — while treating the
message itself as data rather than text. The same sequential-composition failure
becomes:

```text
seq.dsp:2:8: error [FRS-PROP-0002] sequential composition mismatch at node 20: left outputs (2) != right inputs (3)
  2 | B = _,_,_;
    |        ^ related source
  = note: Here  A = (_, _)
  = note: has inputs=2 outputs=2
  = note: while B = (_, (_, _))
  = note: has inputs=3 outputs=3
  = note: cause: sequential composition bus widths do not match
  = note: rule: seq(A, B) requires outputs(A) == inputs(B)
  = note: computed: 2 == 3 -> false
  = note: suggested target: make outputs(A) and inputs(B) equal (common target: 3)
  = help: for `A : B`, enforce outputs(A) == inputs(B)
  = help: fix: adjust A or B channel count to same bus width
  = help: template: process = A : B; // outputs(A) == inputs(B)
```

and, in JSON, the same failure carries a stable code, a category, a source
range in bytes, and the arities as *numbers* rather than as words in a sentence.

Three design rules produce that difference, and they are worth stating because
they explain every behaviour in the rest of this document:

1. **Provenance is recorded when an IR value is built, not reconstructed when an
   error is printed.** A source location that has to be guessed at the end is a
   location that will sometimes be wrong.
2. **Machine meaning lives in typed fields.** `message`, `notes`, and `help` are
   prose whose wording may change at any time. Nothing that a tool needs is
   *only* available there.
3. **The compiler says what it knows, and admits what it does not.** When a
   location cannot be established, the diagnostic says so rather than pointing
   at a plausible nearby span.

---

## 2. The four axes of an error

A `faust-rs` diagnostic is classified along four independent axes. Confusing
them is the usual source of "why did this not fail?" questions.

### 2.1 Severity — does this stop the build?

| Severity | Meaning | Exit status |
|---|---|---|
| `error` | Compilation cannot produce output. | `1` |
| `warning` | Compilation succeeded; something may still be wrong at run time. | `0` |
| `remark` | Informational; attached to recoverable flows. | `0` |

Warnings never change the exit status. A CI job that fails on warnings should
inspect the `severity` field, not the exit code.

### 2.2 Category — whose problem is it?

This axis has no equivalent in the C++ compiler, and it is the single most
useful field for an automated consumer: it answers "should I edit the DSP, edit
my command line, install something, or file a bug?"

| Category | Meaning | What to do |
|---|---|---|
| `user_code` | The Faust source is wrong. | Fix the DSP. |
| `unsupported_feature` | Valid Faust the selected backend or lane cannot lower. | Change backend/options, or rewrite the construct. |
| `invalid_options` | The command line is inconsistent. | Fix the invocation. |
| `environment` | A file, import, or resource is missing. | Fix paths / `-I`. |
| `cancelled` | Cooperative cancellation (timeout, abort). | Retry, or raise the budget. |
| `compiler_bug` | An internal invariant failed. | Report it; the DSP is probably fine. |

An agent that retries by editing the DSP after a `compiler_bug` or
`invalid_options` diagnostic is wasting a turn. The category is there to stop
that.

### 2.3 Stage — where in the pipeline?

`source_reader`, `lexer`, `parser`, `eval`, `propagate`, `normalize`,
`type_inference`, `transform`, `fir`, `codegen`, `compiler`.

The stage tells you how far the program got. A `parser` failure means nothing
downstream ran; a `type_inference` failure means the program is structurally
valid and the problem is in the values it computes.

### 2.4 Verbosity — how much do you want to see?

Severity, category and stage are properties of the *diagnostic*. Verbosity is a
property of the *rendering*, chosen with `--error-verbosity`. The four levels
form a ladder — each shows everything the one below it does, plus more — so
raising the level never hides something you just saw.

| Level | Shows | For |
|---|---|---|
| `concise` | Header, blamed location, first help line. | Editors, status lines, "just take me there". |
| `standard` *(default)* | Every relevant label, `cause`/`rule`/`computed` notes, traces, fixes. | Humans at a terminal; the complete actionable cause. |
| `debug` | Plus internal ids and IR previews, plus the typed debug object. | Bug reports, parity investigations. |
| `full` | Plus untruncated traces and related diagnostics. | Deep dives. |

The same undefined-symbol failure at `concise`:

```text
lowpass.dsp:3:25: error [FRS-EVAL-0002] undefined symbol `cutof`
  3 | process = fi.lowpass(1, cutof);
    |                         ^^^^^ failing use
  = help: define the symbol in scope or fix the identifier name
```

and at `standard`:

```text
lowpass.dsp:3:25: error [FRS-EVAL-0002] undefined symbol `cutof`
  3 | process = fi.lowpass(1, cutof);
    |                         ^^^^^ failing use
    | ^^^^^^^ enclosing definition
    | ^^^^^^^ call site
  = note: cause: unresolved identifier in current lexical scope
  = note: rule: referenced identifier must be present in visible lexical scope
  = note: computed: `cutof` is not present in current visible scope
  = note: did you mean: cutoff?
  = note: scope.local=aa, an, ba, co, cutoff, db, ...
  = note: error originates from definition 'process'
  = note: binding_trace=process
  = fix (maybe-incorrect): rename to `cutoff`
    `cutoff` is visible from this site, but renaming changes which definition runs
  = help: define the symbol in scope or fix the identifier name
  = help: template: cutof = ...; // define before use
  = help: for top-level aliases: define target before first use
```

Notes always appear in a canonical order — `cause`, `rule`, `computed`,
`suggested target`, then context — regardless of which compiler stage produced
them, so two failures of the same kind read the same way.

### 2.5 Opt-in warnings

`--warn` enables the class the reference compiler reports under `-wall` / `-me`:
an operation whose operand *may* leave its mathematical domain at run time.

```faust
process = sqrt;
```

```text
$ faust-rs --check --warn sqrt.dsp
sqrt.dsp:1:1: warning [FRS-COMP-0004] sqrt may be called outside its mathematical domain: operand interval is interval(-1,1,-24), expected [0, +infinity)
  1 | process = sqrt;
    | ^^^^^^^ related source
  = note: cause: the operand interval extends outside the operation's domain
  = note: rule: sqrt requires its operand to stay within [0, +infinity)
  = note: computed: inferred operand interval = interval(-1,1,-24)
  = help: constrain the operand to [0, +infinity), for example with `max`/`min`, so the domain holds for every sample
Check OK: 0 diagnostics
```

It is off by default for a reason: these warnings describe values that only
exist at run time, and interval inference cannot see every way a programmer
clamps an operand. On by default they would be noise on correct programs.

Note the last line — the compilation **succeeded**. Warnings go to stderr in
both output formats, because on success stdout carries the generated code.

---

## 3. Using the compiler from the command line

### 3.1 `--check` is the mode you want for validation

```bash
faust-rs --check mydsp.dsp
```

Runs the full front end (parse → eval → propagate → type) plus FIR
verification, generates no code, and exits `0` or `1`. It is the cheapest way
to answer "is this DSP valid?", and it is what CI, editors, and agents should
call instead of `--dump-cpp` and discarding the output.

### 3.2 Streams

| Format | Diagnostics go to | stdout carries |
|---|---|---|
| `human` (default) | stderr | generated output |
| `json` | stdout, as exactly one JSON document | nothing else |

Under `--error-format json` the payload is the only thing on stdout: no prefix
line, no trailing bytes, on both the success and failure paths. That contract
exists so a consumer can pipe stdout straight into a parser.

Warnings are the one exception: they always go to **stderr**, in the selected
format, because on a successful compile stdout belongs to the generated code.

### 3.3 Path presentation

`--diagnostic-paths absolute|relative|basename` controls how source paths are
spelled in human output. `relative` is the pragmatic choice for CI logs;
`basename` is for sharing a diagnostic without disclosing directory structure.
The JSON channel is unaffected — it always reports the path the compiler
actually used, because a tool resolving a byte range needs that exact path.

---

## 4. The JSON channel

### 4.1 Where it is produced

One place: the CLI, under `--error-format json`. Every mode that can fail emits
it, and `--check` emits it on success too, with an empty `diagnostics` array —
so success and failure share one schema and a consumer never needs a second
code path for "no output".

The payload is **schema v2**, published as `docs/diagnostics-v2.schema.json`
with a worked example in `docs/diagnostics-v2-example.json`. It validates
against the schema for every entry of the negative corpus, enforced by
`crates/compiler/tests/cli_diagnostics_channel.rs`.

### 4.2 Shape

```jsonc
{
  "schema_version": 2,
  "compiler": { "name": "faust-rs", "version": "...", "target": "..." },
  "request": { "mode": null, "backend": null, "normalized_options": [] },
  "status": "failed",
  "sources": [
    { "id": 0, "name": "lowpass.dsp", "kind": "file",
      "content_hash": "9f2b…", "text": null }
  ],
  "diagnostics": [ /* ... */ ]
}
```

`sources` is the immutable snapshot of what was actually compiled. Diagnostic
ranges index into it by `source_id`, and `content_hash` lets a tool detect that
a diagnostic is stale without re-reading the file. `text` is echoed only for
sources the caller supplied in memory — file-backed sources are never copied
back.

One diagnostic, abridged, for the misspelled `cutof` above:

```jsonc
{
  "severity": "error",
  "stage": "eval",
  "code": "FRS-EVAL-0002",
  "detail_code": "undefined-binding",
  "category": "user_code",
  "message": "undefined symbol `cutof`",
  "labels": [
    { "style": "primary", "role": "use_site",
      "range": { "source_id": 0, "start": 96, "end": 101 },
      "compatibility_span": { "file": "lowpass.dsp", "line": 3, "col": 25,
                              "end_line": 3, "end_col": 30 },
      "message": "failing use" },
    { "style": "secondary", "role": "definition_site", "…": "…" }
  ],
  "facts": {
    "symbol":            { "type": "string",      "value": "cutof" },
    "suggested_symbols": { "type": "string_list", "value": ["cutoff"] },
    "scope_visible":     { "type": "string_list", "value": ["cutoff", "fi", "…"] },
    "binding_trace_path":{ "type": "string_list", "value": ["process"] }
  },
  "traces": [],
  "fixes": [
    { "title": "rename to `cutoff`",
      "applicability": "maybe_incorrect",
      "edits": [ { "range": { "source_id": 0, "start": 96, "end": 101 },
                   "replacement": "cutoff" } ],
      "explanation": "`cutoff` is visible from this site, but renaming changes which definition runs" }
  ],
  "related": [],
  "notes": [ "…" ],
  "help":  [ "…" ],
  "debug": null
}
```

### 4.3 Read fields, never prose

| Question | Field |
|---|---|
| What failed? | `code`, `detail_code`, `category`, `stage` |
| Where do I edit? | the label with `role: "primary_cause"` (or `use_site`), then its `range` |
| What else is involved? | other labels, by `role` — `conflicts_with`, `call_site`, `import_site`, `definition_site` |
| What rule was violated? | `facts` — `expected`, `actual`, `ui_path`, `scope_visible`, … |
| How was this code reached? | `traces[]` frames |
| Can I apply a fix? | `fixes[].applicability` plus `fixes[].edits` |
| Is this my DSP's fault? | `category` |
| Is this result stale? | `sources[].content_hash` |

`message`, `notes` and `help` are presentation text. Their wording is free to
improve without a schema change, so nothing in them is part of the contract.
A workspace gate (`cargo run -p xtask -- diagnostics-quality-check`) enforces
that no compiler code recovers machine meaning from note text.

### 4.4 Applying fixes

`applicability` is a promise, and the levels differ in kind:

| Level | Promise | Apply automatically? |
|---|---|---|
| `machine_applicable` | Deterministic repair; the diagnostic will disappear. | Yes. |
| `maybe_incorrect` | A concrete edit that may change DSP semantics. | Only with review or explicit opt-in. |
| `has_placeholders` | A template with holes to fill. | No. |
| `manual` | Guidance, no edit. | No. |

A missing semicolon is `machine_applicable` — there is one repair and it cannot
mean anything else. Renaming `cutof` to `cutoff` is `maybe_incorrect` even
though `cutoff` is one character away: the compiler knows the name is reachable,
not that it is what you meant.

Edits within one fix are ordered and non-overlapping; apply them **back to
front** so an earlier edit cannot shift a later one's offsets, and apply all of
one fix or none.

This is verified, not asserted:
`crates/compiler/tests/machine_applicable_fixes.rs` applies every
`machine_applicable` fix to the real source through the real binary,
recompiles, and requires both that the targeted diagnostic is gone and that no
new parse error appeared.

### 4.5 Compatibility rules

- `schema_version` changes only for a breaking change. **Reject an unknown
  value** rather than guessing.
- `FRS-*` code meanings are frozen. New codes may appear; existing ones are
  never renumbered or repurposed.
- New fields may be added to any object — ignore unknown fields.
- New values may appear in any enum (a new `stage`, `role`, `category`) —
  degrade gracefully rather than failing.
- `detail_code` is pass-local and free to change; treat it as a refinement of
  `code`, never as a substitute.
- `range` (source id plus half-open UTF-8 byte offsets) is canonical.
  `compatibility_span` is the derived 1-based line/column view for humans.

---

## 5. For LLM agents, and for MCP

### 5.1 The two audiences want different things

A human at a terminal wants the shortest complete explanation and a caret under
the right character. An agent wants the opposite of prose: stable identifiers,
numbers as numbers, an exact range to edit, and a signal about whether editing
the DSP is even the right response.

Both are served from the same diagnostic. `standard` human rendering and the v2
JSON payload are two projections of one typed value — they cannot disagree,
because neither is derived from the other's text.

### 5.2 What an agent should do

1. Call `--check --error-format json`. Never parse human output.
2. Branch on `category` **first**. `user_code` means edit the DSP;
   `invalid_options` means fix the command line; `environment` means fix paths;
   `compiler_bug` means stop and report.
3. Use the primary label's `range` to locate the edit. It is a byte range into
   the exact text that was compiled, not into whatever is on disk now.
4. Read `facts` for the numbers. Arities, intervals, scopes, UI paths and
   suggested symbols are all typed — there is nothing to extract from a
   sentence.
5. Apply `machine_applicable` fixes directly; treat `maybe_incorrect` as a
   proposal to confirm.
6. Re-check. `content_hash` tells you whether a diagnostic you are holding still
   describes the current source.

Two traps worth naming, because both cost turns:

- **Do not treat `maybe_incorrect` as a repair.** A rename that compiles is not
  a rename that is correct.
- **Do not chase derived diagnostics.** Several parser recovery reports at the
  *same* span collapse into one primary diagnostic, with the others kept as
  `related` evidence rather than repeated as errors. Distinct failures stay
  distinct — but a later one is often a consequence of the first, so fix the
  root and re-check instead of working down the list.

### 5.3 MCP today, and what changes

The Faust MCP servers in use today wrap the C++ compiler:

| Server | Compiles via | Diagnostics returned |
|---|---|---|
| [`orlarey/faustcode`](https://github.com/orlarey/faustcode) | `libfaust-wasm` in the browser | an `errors.log` retrieval tool — C++ text |
| [`grame-cncm/faustbrowser-mcp`](https://github.com/grame-cncm/faustbrowser-mcp) | `libfaust-wasm` in the browser | `check_syntax` → `{ "status", "error" }` with C++ text |

Both are well-designed servers limited by what the underlying compiler can say.
An agent using them today gets one string per failure and has to reverse-engineer
it.

A `faust-rs`-backed MCP server does not need to invent a diagnostic format: the
v2 payload *is* the tool response. The planned surface
(`porting/mcp-server-analysis-and-plan-2026-07-21-en.md`) centres on a
`faust_check` tool that runs the same front end this document describes and
returns the same `diagnostics` array — codes, labels with the offending source
line inlined, typed facts, traces, and fixes — plus warnings on success. That
server is **not yet implemented**; its prerequisite, a clean single-document
machine channel on stdout, is what shipped and is described in §4.

Until then, the CLI *is* the machine interface, and it is already usable from
any agent that can run a subprocess:

```bash
faust-rs --check --error-format json --warn mydsp.dsp
```

---

## 6. Error families, with examples

Every example below is a real program and a real, current `faust-rs` output,
next to what the C++ compiler prints for the same file. Paths are shortened for
readability. The families follow the taxonomy of the official manual.

### 6.1 Syntax — a missing separator

```faust
box1 = 1
box2 = 2;
process = box1, box2;
```

C++:

```text
error.dsp:2 : ERROR : syntax error, unexpected IDENT
```

faust-rs:

```text
e_semicolon.dsp:2:1: error [FRS-PARSE-0001] Parsing error at line 2 column 1. Repair sequences found:
   1: Insert ENDDEF
   2: Insert LCROC
  2 | box2 = 2;
    | ^^^^ unexpected token
```

Both point at line 2, which is where the parser *noticed*. The Faust manual
calls this out explicitly: a missing semicolon only becomes visible at the next
token.

### 6.2 Syntax — an unmatched delimiter

```faust
t1 = _~(+(1);
process = t1 / 2147483647;
```

C++:

```text
errors.dsp:1 : ERROR : syntax error, unexpected ENDDEF
```

faust-rs:

```text
e_paren.dsp:1:13: error [FRS-PARSE-0001] Parsing error at line 1 column 13. Repair sequences found:
   1: Insert RPAR
  1 | t1 = _~(+(1);
    |             ^ unexpected token
    |        ^ `(` opened here
  = fix (machine-applicable): insert `)`
    the parser found only this insertion repair
```

Two things the C++ output cannot give: the **opening** delimiter is labeled, and
because exactly one repair exists, the fix is `machine_applicable` — a tool can
apply it without asking.

### 6.3 Undefined symbol

```faust
import("stdfaust.lib");
cutoff = hslider("cutoff", 1000, 50, 10000, 1);
process = fi.lowpass(1, cutof);
```

C++:

```text
e_undefined.dsp:3 : ERROR : undefined symbol : cutof
```

faust-rs — see §2.4 for the full rendering. The additions are: the visible
scope as a typed list, the binding trace from the entry point, a ranked
near-name suggestion, and an exact rename edit.

The suggestion is deliberately conservative. Candidates come **only** from the
scopes the evaluator actually recorded as visible, so a suggestion can never
name something you cannot reach from that site; and when two candidates are
equally close, no edit is offered at all, because the compiler cannot know
which you meant.

### 6.4 Missing entry point

```faust
gain = 0.5;
proces = *(gain);
```

C++:

```text
????:-1 : ERROR : undefined symbol : process
```

faust-rs:

```text
e_noprocess.dsp:2:1: error [FRS-EVAL-0001] missing `process` definition
  2 | proces = *(gain);
    | ^^^^^^ call site
  = note: cause: required top-level `process` definition is missing
  = note: did you mean: proces?
  = note: entrypoint contract: one top-level `process = ...;` definition is required
  = note: available top-level definitions: gain, proces
  = fix (maybe-incorrect): rename to `process`
    `proces` looks like a misspelling of the required `process` entry point
  = help: define `process = ...;` in the top-level definitions
  = help: template: process = _;
```

Note the C++ location: `????:-1`. There is no location to give, because the
failure is the *absence* of a definition. `faust-rs` instead points at the
near-miss definition, which is where the edit belongs — and the rename goes in
the right direction (`proces` → `process`), not the other way round.

### 6.5 Duplicate definitions

```faust
gain = 0.5;
gain = 0.8;
process = *(gain);
```

C++:

```text
ERROR : [file e_redef.dsp : 4] : multiple definitions of symbol 'gain'
gain = 0.5f;
gain = 0.8f;
```

faust-rs:

```text
e_redef.dsp:2:1: error [FRS-PARSE-0001] multiple definitions of symbol 'gain'
  2 | gain = 0.8;
    | ^^^^ conflicting declaration
  1 | gain = 0.5;
    | ^^^^ previous declaration
  = note: declaration: gain = float_bits(0x3fe0000000000000);
  = note: declaration: gain = float_bits(0x3fe999999999999a);
  = help: keep one `gain = ...;` clause, or give the clauses distinct patterns
```

**Both** declarations are labeled, at their real lines — the later one as the
cause, the earlier one as context. The clause listing that C++ folds into the
message body is here a typed `declarations` fact, so the message stays one line.
(The clauses are rendered from normalized internal boxes, which is why literals
appear as bit patterns; the labels are what you act on.)

### 6.6 Box connection — sequential, split, recursive

```faust
A = _,_;
B = _,_,_;
process = A : B;      // or A <: B, or A ~ B
```

C++ prints the three variants described in §1.1. `faust-rs` adds, for each, the
algebraic rule as a separate note, the computed values, and a concrete target:

| Operator | `rule:` | `computed:` | `suggested target:` |
|---|---|---|---|
| `A : B` | `seq(A, B) requires outputs(A) == inputs(B)` | `2 == 3 -> false` | make them equal (common target: 3) |
| `A <: B` | `split(A, B) requires inputs(B) % outputs(A) == 0` | `3 % 2 = 1` | set inputs(B) to 4 |
| `A ~ B` | `rec(A, B) requires right_inputs <= left_outputs and right_outputs <= left_inputs` | `3 <= 2 is false, 3 <= 2 is false` | outputs(A) >= 3 and inputs(A) >= 3 |

The split case is the clearest illustration: C++ says the divisibility rule was
violated; `faust-rs` says `3 % 2 = 1` and tells you the next valid input count.

A caveat worth stating plainly: for these composition failures the source label
currently points at a sub-expression of the composition (here, inside
`B = _,_,_;`) rather than at the `:` operator itself. The arities and the rule
are exact; the span is approximate for this family.

### 6.7 Pattern matching

```faust
sel = case {
    (0, x) => x;
    (1, x) => x * 0.5;
};
process = sel(2, _);
```

C++:

```text
ERROR : pattern matching failed, no rule of case {(<x>,1) => x,0.5f : *; (<x>,0) => x; } matches argument list (2)
```

faust-rs:

```text
e_case.dsp:1:1: error [FRS-EVAL-0099] no case rule matches arguments
  1 | sel = case {
    | ^^^ definition site
  5 | process = sel(2, _);
    | ^^^^^^^ call site
  = note: cause: no case rule matched the provided argument tuple
  = note: rule: at least one case pattern must match the provided argument tuple
  = note: computed: provided tuple did not match any declared case pattern
  = note: computed: no rule survived after 1 of 2 argument(s)
  = note: expr=…
  = note: error originates from definition 'sel'
  = note: binding_trace=process -> sel
  = trace (evaluation): arguments -> rule 1 -> rule 2
  = help: add a matching case rule or add a catch-all pattern
```

Both the definition and the call site are located. The C++ message renders the
rules in *reverse* internal order with evaluator wrappers (`<x>`); `faust-rs`
exposes them as the typed fact `pattern_rules = ["(0, x)", "(1, x)"]`, in
written order, with pattern variables as bare names. The `computed:` line
answers the question that actually matters: the matcher died on the **first**
argument, because `2` matches neither `0` nor `1`.

### 6.8 Imports

```faust
import("nosuchlib.lib");
process = _;
```

C++:

```text
ERROR : unable to open file nosuchlib.lib
```

faust-rs:

```text
e_import.dsp:1:9: error [FRS-SRC-0002] cannot resolve import `nosuchlib.lib`
  1 | import("nosuchlib.lib");
    |         ^^^^^^^^^^^^^ unresolved import
  = note: import name: nosuchlib.lib
  = note: imported from: …/e_import.dsp
  = note: searched 5 directories:
  = note:   …/doc
  = note:   …/target/share/faust
  = note:   /usr/local/share/faust
  = note:   /usr/share/faust
  = help: add the directory containing the file with `-I <dir>`
  = help: or correct the import name
```

The category here is `environment`, not `user_code` — the DSP may be perfectly
correct and the search path wrong. That distinction is exactly what an agent
needs before it starts editing the file.

Parse errors *inside* a loaded `component(...)` or `library(...)` keep their own
codes, labels and source snapshots rather than being flattened into the parent
error, and import cycles are reported as the complete ordered cycle with one
labeled `import(...)` site per edge.

### 6.9 Iteration

```faust
process = par(i, +, 8);
```

C++:

```text
e_iter.dsp:1 : ERROR : not a constant expression of type : (0->1) : +
```

faust-rs:

```text
e_iter.dsp:1:1: error [FRS-EVAL-0004] iteration count is not an int node: 5
  1 | process = par(i, +, 8);
    | ^^^^^^^ definition site
  = note: cause: iterative combinator count is not a valid non-negative integer
  = note: rule: iterator count must be integer, non-negative, and within supported range
  = note: error originates from definition 'process'
  = help: iteration count must be a non-negative integer in target range
```

### 6.10 Signal types and intervals

```faust
process = _, 0 : soundfile("foo.wav", 2);
```

C++:

```text
ERROR : out of range soundfile part number (interval(-1,1,-24) instead of interval(0,255)) in expression : length(soundfile("foo.wav"),IN[0])
```

faust-rs:

```text
e_soundfile.dsp:1:16: error [FRS-COMP-0004] out of range soundfile part number (interval(-1,1,-24) instead of interval(0,255))
  1 | process = _, 0 : soundfile("foo.wav", 2);
    |                ^ source expression
    | ^^^^^^^ enclosing definition
  = note: cause: an inferred signal type or interval violates a typing rule
  = note: rule: a soundfile part selector must stay within the integer interval [0, 255]
  = note: computed: inferred interval = interval(-1,1,-24), expected integer interval [0, 255]
  = help: clamp the part selector into 0..255, for example with `min(255, max(0, part))`
```

This is the family where the difference is largest. C++ ends the message with an
internal Signal expression (`length(soundfile(...),IN[0])`) and no location.
`faust-rs` puts the Faust source under a caret, states the rule, reports the
interval as a typed `actual_interval` fact and the bound as a typed
`required_interval`, and keeps the internal Signal form for
`--error-verbosity debug`.

### 6.11 Mathematical domain

```faust
process = _ % 0;
```

C++:

```text
ERROR : % by 0 in IN[0] % 0
```

faust-rs:

```text
e_modzero.dsp:1:1: error [FRS-COMP-0004] % by 0
  1 | process = _ % 0;
    | ^^^^^^^ related source
  = note: cause: an inferred signal type or interval violates a typing rule
  = note: rule: an operand must stay inside its operation's mathematical domain
  = note: computed: inferred operand intervals = interval(-1,1,-24), interval(0,0,0), expected denominator must be non-zero
  = help: constrain the operand so the domain holds for every sample, for example with `max`/`min`
```

The compile-time case is an **error** (the operand is provably outside the
domain). The run-time case — an operand that merely straddles the boundary — is
the opt-in **warning** of §2.5. Keeping them distinct matters: one is a broken
program, the other is a risk.

### 6.12 Duplicate user-interface paths

```faust
process = *(hslider("gain", 0.5, 0, 1, 0.01))
        : *(vslider("gain", 1.0, 0, 2, 0.01));
```

C++:

```text
ERROR : path '/e_uipath/gain' is already used
```

faust-rs:

```text
e_uipath.dsp:2:13: error [FRS-UI-0001] UI path '/e_uipath/gain' is claimed by 2 controls
  2 |         : *(vslider("gain", 1.0, 0, 2, 0.01));
    |             ^^^^^^^ duplicate claim
  1 | process = *(hslider("gain", 0.5, 0, 1, 0.01))
    |             ^^^^^^^ first claim of this path
  = note: cause: two user-interface controls resolve to the same runtime address
  = note: rule: every UI control must have a unique group path plus label
  = note: computed: normalized path = /e_uipath/gain, claimed 2 times
  = help: rename one control, or place them in different groups
  = help: group placement example: hgroup("left", ...) and hgroup("right", ...)
```

Same address, same rejection — but both widget declarations are located.

The asymmetry C++ defines is preserved exactly: two **input** controls sharing
an address is an error, while two **bargraphs** sharing one is merely ambiguous
and still compiles. `faust-rs` also runs this check on the UI layout rather than
during JSON serialization, so the same program is rejected whichever backend you
select.

### 6.13 Backend and option failures

Failures from code generation and from incompatible option combinations are
categorized `unsupported_feature` or `invalid_options` rather than `user_code`.
The backend's own fine-grained code travels as `detail_code` and as a typed
`codegen_code` fact — the top-level `FRS-CODEGEN-0001` names the class, the
detail code names the specific case.

The practical consequence: a diagnostic in this family usually means "try
another backend or another option", not "rewrite the DSP".

### 6.14 Compiler bugs

An internal invariant failure is categorized `compiler_bug`, names the failing
pass, and asks for a reproducible report. It never suggests that the DSP syntax
is at fault. If you see one, the DSP that triggered it is the bug report.

---

## 7. Reference: the frozen `FRS-*` code table

This section is the authoritative, frozen list of stable diagnostic codes
(`FRS-*`) emitted by the compiler's structured diagnostics (`--error-format json`,
`--error-format human`, and the `--check` mode). It is part of the P0 phase of
`porting/mcp-server-analysis-and-plan-2026-07-21-en.md` (§1.4.5: "Stable codes
become a public contract"), and exists so that a consumer — CI, an IDE, or a
future MCP server — can treat the code set as a versioned API rather than
re-deriving it from source on every change.

**Freeze rule.** Adding a new code is fine. Renumbering or repurposing an
existing code is not — it silently breaks every consumer that matched on it.
`crates/compiler/src/cli/tests.rs::frozen_frs_code_table_matches_source`
enforces this by re-running the exact extraction command below and diffing
the result against the table in this document; both adding an undocumented
code and renumbering a documented one fail that test.

**Source of truth / how this table was generated.** The canonical way to
enumerate every code actually present in source is:

```bash
grep -rhoE 'FRS-[A-Z]+-[0-9]+' --include=*.rs crates/ | sort -u
```

This currently returns **33 codes** across **10 stage-family namespaces**:
`FRS-LEX-*` (1), `FRS-PARSE-*` (3), `FRS-SRC-*` (3), `FRS-EVAL-*` (8),
`FRS-PROP-*` (5), `FRS-COMP-*` (2), `FRS-UI-*` (1), `FRS-FIR-*` (2),
`FRS-SFIR-*` (8), `FRS-CODEGEN-*` (1).

Backend emitters additionally own a **separate, finer taxonomy** of 27 codes
shaped `FRS-CGEN-<LANG>-NNNN` (ASC, C, CLIF, CPP, INTERP, JULIA, RUST, WASM).
Those are *not* part of this table and do not appear in `diagnostics::codes`: they
travel inside `FRS-CODEGEN-0001` diagnostics as typed `detail_code` and
`codegen_code` fact values, the same way FIR verifier codes travel inside
`FRS-FIR-000{1,2}` as `detail_code` and `fir_code`.
Note they do not match the extraction regex either (the extra `-<LANG>` segment
means `FRS-[A-Z]+-[0-9]+` never matches), so they cannot silently leak into the
frozen set.

Note the family prefix (`LEX`, `PARSE`, ...) is a naming convention only; the
JSON payload's `"stage"` field comes from the independent `diagnostics::Stage`
enum and does not always equal the family name (e.g. every `FRS-SFIR-*` code
reports `"stage": "transform"`, not `"stage": "sfir"` — there is no `Sfir`
`Stage` variant). Both are listed per code below.

### Important caveat: a few codes are currently unreachable or unused

The extraction command above is a textual grep over `.rs` source, not a
reachability analysis. Building this table required tracing every code from
its `diagnostics::codes::*` constant to an actual call site, and that surfaced
real gaps, recorded here rather than papered over:

- **`FRS-SRC-0001`, `FRS-SRC-0002`, `FRS-SRC-0003`** are defined in
  `crates/diagnostics/src/codes.rs` and listed in `codes::all_codes()`, but no
  code anywhere in the workspace ever constructs a `Diagnostic` with them.

  **Wired up 2026-07-21.** These were never dead reservations:
  `parser::source_reader::SourceReaderError` has exactly three variants that map
  one-to-one onto the three codes, and all three fire in practice.
  `SourceReaderError::to_diagnostics` now builds a real bundle for each, and
  `CompilerError::import` attaches it, so source-loading failures no longer fall
  through to the `code: null` envelope:

  | Variant | Code | Diagnostic content |
  |---|---|---|
  | `Io { path, message }` | `FRS-SRC-0001` | path note + readability help |
  | `UnresolvedImport { .. }` | `FRS-SRC-0002` | span on the `import(...)` directive, import name, importing file, ordered list of searched directories, `-I` help |
  | `ImportCycle { path }` | `FRS-SRC-0003` | cycle note + help to break it |

  The reference C++ compiler reports the same conditions as bare strings
  (`ERROR : unable to open file <name>`), with no location and no searched
  paths, so this is deliberately more informative than parity rather than a
  port of it.

- **`FRS-COMP-0001`, `FRS-COMP-0002`, `FRS-COMP-0003` were retired**
  (2026-07-21) — see "Retired codes" below.
- **`FRS-LEX-0001`** is defined and its call site
  (`crates/parser/src/lib.rs:1926`) is live code, but it is not reachable
  from any DSP text found during this audit: `crates/parser/src/grammar/faustlexer.l`
  ends with a catch-all `. 'EXTRA'` rule, so every single byte the lexer
  sees matches *some* token (an `EXTRA` token in the worst case) and the
  failure surfaces one layer up as a `FRS-PARSE-0001` parse error instead of
  a `lrpar::LexParseError::LexError`. Genuinely invalid bytes (e.g. a
  non-UTF-8 byte sequence) are rejected even earlier, at file read time,
  before lexing starts, with no diagnostics bundle at all.

  **Decision (2026-07-21): kept deliberately.** Unlike the dormant `FRS-SRC-*`
  / `FRS-COMP-000{1,2,3}` declarations above, this one is not an unused
  constant: it is one arm of an exhaustive `match` over `lrpar::LexParseError`,
  a third-party enum (`parser_code_for_lex_parse_error`). Removing the code
  would not remove any code path — it would only force that arm to report a
  less accurate code. It becomes reachable again if the lexer's catch-all rule
  is ever narrowed.
- **`FRS-FIR-0001`** (verifier *error*, as opposed to `FRS-FIR-0002`
  warnings) requires the FIR verifier to reject FIR text that a
  *successful* front-end run produced — i.e. a compiler bug, not a user
  DSP mistake. No corpus file triggers it; only `--fir-fixture` bring-up
  fixtures could, and the eight built-in fixtures
  (`--list-fir-fixtures`) are all valid by construction.
- **`FRS-EVAL-0100` was removed from this table** (2026-07-21). It never came
  from `diagnostics::codes`: it was a literal string in
  `crates/diagnostics/src/lib.rs`'s
  own unit test `bundle_counts_error_severity_only`, picked up only because the
  extraction is textual. Documenting it made the table promise a public code
  that nothing emits. The test now uses a real registered code
  (`EVAL_GENERIC_FAILURE`), so the extraction no longer sees a phantom.

Nothing here blocks freezing: a dormant or unreachable code is still a valid,
stable reservation. But a consumer should not assume every documented code is
observable in practice today.

### Code table

#### `FRS-LEX-*` — Lexer (1 code)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-LEX-0001` | `lexer` (via `Stage::Parser` in practice — see caveat) | Lexer encountered an invalid token sequence. | `crates/parser/src/lib.rs:1926` (`parser_code_for_lex_parse_error`); currently unreachable, see caveat above. |

#### `FRS-PARSE-*` — Parser (3 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-PARSE-0001` | `parser` | Parser encountered an unexpected token. | `crates/parser/src/lib.rs:1917` (default case), `:1927` (`LexParseError::ParseError`) |
| `FRS-PARSE-0002` | `parser` | Parser recovered from an error and emitted recovery diagnostics (warning/remark severity). | `crates/parser/src/lib.rs:1913` |
| `FRS-PARSE-0003` | `parser` | Parser encountered an invalid literal form. | `crates/parser/src/lib.rs:1915` |

#### `FRS-SRC-*` — Source reader (3 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-SRC-0001` | `source_reader` | Source reader I/O failure (unreadable file, directory passed as input). | `SourceReaderError::Io` → `to_diagnostics` (`crates/parser/src/source_reader.rs`) |
| `FRS-SRC-0002` | `source_reader` | Imported file could not be resolved. Carries a span on the `import(...)` directive and the ordered list of searched directories. | `SourceReaderError::UnresolvedImport` → `to_diagnostics` |
| `FRS-SRC-0003` | `source_reader` | Import graph contains a cycle. | `SourceReaderError::ImportCycle` → `to_diagnostics` |

#### `FRS-EVAL-*` — Box evaluation (8 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-EVAL-0001` | `eval` | `process` definition is missing. | `crates/eval/src/error.rs:403` |
| `FRS-EVAL-0002` | `eval` | Symbol lookup failed during eval (undefined symbol). | `crates/eval/src/error.rs:433` |
| `FRS-EVAL-0003` | `eval` | Arity mismatch detected during eval (e.g. too many arguments). | `crates/eval/src/error.rs:471,488` |
| `FRS-EVAL-0004` | `eval` | Invalid iteration construct detected during eval. | `crates/eval/src/error.rs:658` |
| `FRS-EVAL-0005` | `eval` | Symbol redefined with a different value in the same lexical scope. | `crates/eval/src/error.rs:620` |
| `FRS-EVAL-0006` | `eval` | Slider/numentry init value is outside the `[min, max]` range. | `crates/eval/src/error.rs:692` |
| `FRS-EVAL-0099` | `eval` | Generic eval failure fallback code (covers eval-error variants without a dedicated code). | `crates/eval/src/error.rs` (multiple sites, e.g. `:508,517,530,539,554,584,592,603,646,669,704`) |

#### `FRS-PROP-*` — Box-to-signal propagation (5 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-PROP-0001` | `propagate` | Unsupported box node encountered in propagate. | `crates/propagate/src/error.rs:227,436` |
| `FRS-PROP-0002` | `propagate` | Arity mismatch in propagate composition rules (`seq`/`split`/`merge`/UI wiring). | `crates/propagate/src/error.rs:235,247,268,301,398,406,414` |
| `FRS-PROP-0003` | `propagate` | Recursion/projection contract mismatch in propagate (`rec` arity/alias). | `crates/propagate/src/error.rs:339` |
| `FRS-PROP-0004` | `propagate` | Automatic differentiation (`fad`/`rad`) reached a clock-domain boundary it cannot cross. | `crates/propagate/src/error.rs:548` |
| `FRS-PROP-0099` | `propagate` | Generic propagate failure fallback code. | `crates/propagate/src/error.rs:372,380,390,422` |

#### `FRS-COMP-*` — Top-level compiler pipeline (2 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-COMP-0004` | `type_inference` | Signal type validation failed. | `crates/compiler/src/error_mapping.rs` |
| `FRS-COMP-0005` | `compiler` | Parse reported no errors yet exposed no root node. Internal invariant guard — reaching it means a compiler bug, not a DSP mistake (an empty file fails later with `FRS-EVAL-0001`). | `CompilerError::missing_root` |

#### `FRS-CODEGEN-*` — Backend emission (1 code)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-CODEGEN-0001` | `codegen` | Backend code generation failed while emitting from FIR. Carries typed `backend`, `detail_code`, and `codegen_code` fields. | `CompilerError::codegen_diagnostics`, via all backend variants |

One code covers every backend deliberately: the failure class is identical and
the backend is a parameter, so the discriminating detail rides in typed fields rather
than multiplying near-identical `FRS-*` codes.

`FRS-COMP-0001`..`0003` are retired; the numbering gap is deliberate (see
below).

#### `FRS-UI-*` — User-interface layout (1 code)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-UI-0001` | `propagate` | Two or more UI controls claim the same runtime address, so they are indistinguishable to every host. Carries typed `ui_path`, `control_count`, and `control_labels` fields, and one label per conflicting declaration. | `crates/compiler/src/ui_paths.rs` |

The C++ compiler raises the equivalent `path '...' is already used` while
serializing JSON. Rust checks the grouped `UiProgram` right after propagation
instead, so the same program is rejected regardless of the selected backend.

#### `FRS-FIR-*` — FIR verifier (2 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-FIR-0001` | `fir` | FIR verifier error diagnostic (fatal; verifier code in `detail_code` and typed `fir_code`). | `crates/compiler/src/json_naming.rs:27`; currently unreachable from any known DSP input — see caveat. |
| `FRS-FIR-0002` | `fir` | FIR verifier warning diagnostic (verifier code in `detail_code` and typed `fir_code`); promoted to fatal under `--fir-verify-strict`. | `crates/compiler/src/json_naming.rs:28`; reachable for an ordinary DSP under `--fir-verify-strict`. |

#### `FRS-SFIR-*` — Signal-to-FIR lowering (8 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-SFIR-0001` | `transform` | Invalid options passed to signal→FIR lowering. | `crates/compiler/src/json_naming.rs:51` |
| `FRS-SFIR-0002` | `transform` | Empty signal list provided to signal→FIR lowering. | `crates/compiler/src/json_naming.rs:52` |
| `FRS-SFIR-0003` | `transform` | Signal outputs arity mismatch in signal→FIR lowering. | `crates/compiler/src/json_naming.rs:53` |
| `FRS-SFIR-0004` | `transform` | Unsupported signal node in signal→FIR lowering. | `crates/compiler/src/json_naming.rs:54`; reachable, e.g. `tests/corpus/err_fad_rad_temporal.dsp`. |
| `FRS-SFIR-0005` | `transform` | Unsupported binary operator in signal→FIR lowering. | `crates/compiler/src/json_naming.rs:55` |
| `FRS-SFIR-0006` | `transform` | Input index out of range in signal→FIR lowering. | `crates/compiler/src/json_naming.rs:56` |
| `FRS-SFIR-0007` | `transform` | Clocked node (`ondemand`/`upsampling`/`downsampling`) reached signal→FIR lowering before the clock-domain back half is ported. | `crates/compiler/src/json_naming.rs:57` |
| `FRS-SFIR-0008` | `transform` | Clock-environment inference / hierarchical-graph validation failed. | `crates/compiler/src/json_naming.rs:58` |
| `FRS-SFIR-0009` | `transform` | Foreign runtime variable `count` accessed under `-ec`/`-os` (no block count exists in `control`/`frame`). | `crates/compiler/src/json_naming.rs:59` |
| `FRS-SFIR-0010` | `transform` | Block-sensitive reverse-AD operation (`BlockReverseAD`/`ReverseTimeRec`) under `-os`: block-boundary semantics have no one-sample meaning (D2). | `crates/compiler/src/json_naming.rs:62` |

### The no-bundle fallback (`code: null`)

Some `CompilerError` variants carry no `DiagnosticBundle` at all — backend
codegen failures (`Codegen`, `CodegenC`, `CodegenJulia`, `CodegenInterp`,
`CodegenWasm`) and source/import failures (`Import`, `MissingRoot`). None of
the codes in this table apply to them. Under `--error-format json`,
`crates/compiler/src/cli/diagnostics.rs::format_fallback_diagnostics_json`
still emits a single-diagnostic envelope for these so stdout is always valid
JSON (D1), but with `"code": null` instead of a real `FRS-*` code — this is
intentional, not an omission from this table, and consumers should treat
`code == null` as "unstructured legacy error text" rather than look it up
here.

### Retired codes — never reassign

Deleting a code that was never emitted is safe: no consumer can have matched on
it. Reusing its *number* for a different meaning later is not — that is the same
silent break the freeze rule prevents, just delayed. Retired numbers are
therefore burned permanently.

| Code | Retired | Why |
|---|---|---|
| `FRS-COMP-0001` | 2026-07-21 | "parse stage failed" — already covered by `FRS-PARSE-*`, with spans the wrapper lacked |
| `FRS-COMP-0002` | 2026-07-21 | "eval stage failed" — already covered by `FRS-EVAL-*` (incl. the `0099` fallback) |
| `FRS-COMP-0003` | 2026-07-21 | "propagate stage failed" — already covered by `FRS-PROP-*` (incl. `0099`) |
| `FRS-EVAL-0100` | 2026-07-21 | never a code — a literal in a unit test, captured by the textual extraction |

`FRS-COMP-0004` is deliberately **not** renumbered into the gap left by
`0001`..`0003`: renumbering a live code is the one operation the freeze rule
forbids. A gap in the numbering is the correct end state.

### Where this is enforced

- `crates/compiler/src/cli/tests.rs::frozen_frs_code_table_matches_source` —
  re-runs the extraction grep and diffs it against the set documented above;
  fails on an undocumented new code or a renumbered existing one.
- `crates/compiler/src/cli/tests.rs::code_registry_matches_frozen_table` —
  checks that the runtime registry `diagnostics::codes::all_codes()` lists exactly
  the codes documented here, in both directions. Added 2026-07-21 after the two
  were found to have silently diverged (`FRS-EVAL-0006` was emitted but absent
  from the registry).
- `crates/diagnostics/src/codes.rs`'s own `all_codes_follow_stable_format` /
  `all_codes_are_unique` unit tests check the format/uniqueness invariants of
  the registered set.
- `cargo run -p xtask -- diagnostics-quality-check` — requires every declared
  code to be registered *and* documented here, requires every variant of a
  serialized diagnostics enum (`Severity`, `Stage`, `DiagnosticCategory`,
  `LabelRole`, `TraceKind`, `Applicability`, `SourceKind`) to appear in
  `docs/diagnostics-v2.schema.json`, and rejects any new code that recovers
  machine meaning from note text.
- `crates/compiler/tests/cli_diagnostics_channel.rs` — validates the v2 payload
  of every negative-corpus entry against the published schema, and enforces the
  single-JSON-document-on-stdout contract.
- `crates/compiler/tests/machine_applicable_fixes.rs` — applies every
  `machine_applicable` fix for real and requires the diagnostic it targets to
  disappear without introducing a new one.

## Related documents

- `docs/user-diagnostics-guide-en.md` — the short operational guide: how to run
  the compiler, read one error, and choose a verbosity.
- `docs/diagnostics-v2.schema.json` — the machine contract, with
  `docs/diagnostics-v2-example.json` as a worked payload.
- `porting/mcp-server-analysis-and-plan-2026-07-21-en.md` — the MCP surface
  planned on top of this model.
- <https://faustdoc.grame.fr/manual/errors/> — the reference C++ compiler's
  error chapter, the baseline compared against throughout this document.
