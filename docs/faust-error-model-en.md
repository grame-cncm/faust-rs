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
## 7. The error codes

Every diagnostic carries a stable `FRS-*` code. The code identifies *what kind
of problem* it is, independently of the message text, which is free to improve.
Match on the code, never on the wording.

**The freeze rule:** new codes may be added, existing ones are never renumbered
or given a new meaning. A code you match on today will mean the same thing in
every later release.

The family prefix (`PARSE`, `EVAL`, …) is a naming convention. The `stage` field
in the JSON payload is a separate value and does not always share the family's
name — both are listed below.

### 7.1 Reading a program (`FRS-LEX-*`, `FRS-PARSE-*`, `FRS-SRC-*`)

| Code | Stage | Means |
|---|---|---|
| `FRS-LEX-0001` | `lexer` | An invalid token sequence. |
| `FRS-PARSE-0001` | `parser` | An unexpected token: a missing `;`, an unbalanced delimiter, a malformed expression. Also carries duplicate top-level definitions. |
| `FRS-PARSE-0002` | `parser` | The parser recovered and is reporting what it did (warning/remark, not an error). |
| `FRS-PARSE-0003` | `parser` | An invalid literal, such as a number that cannot be represented. |
| `FRS-SRC-0001` | `source_reader` | A source file could not be read. |
| `FRS-SRC-0002` | `source_reader` | An `import(...)` could not be resolved. Lists every directory searched. |
| `FRS-SRC-0003` | `source_reader` | The imports form a cycle. Reports the complete cycle, one location per edge. |
| `FRS-SRC-0004` | `source_reader` | A remote source URL is invalid or uses an unsupported scheme. |
| `FRS-SRC-0005` | `source_reader` | A remote source was requested without an injected network capability. |
| `FRS-SRC-0006` | `source_reader` | An injected remote transport failed to fetch a source. |

`FRS-SRC-*` diagnostics are categorized `environment`, not `user_code`: your DSP
may be correct and your search path wrong.

### 7.2 Evaluating the program (`FRS-EVAL-*`)

| Code | Stage | Means |
|---|---|---|
| `FRS-EVAL-0001` | `eval` | No `process` definition. Suggests a near-miss name when one exists. |
| `FRS-EVAL-0002` | `eval` | An undefined symbol. Lists the visible scope and suggests near names. |
| `FRS-EVAL-0003` | `eval` | An arity mismatch — too many arguments, or a `case` pattern of the wrong width. |
| `FRS-EVAL-0004` | `eval` | An invalid iteration construct: `par`/`seq`/`sum`/`prod` whose count is not a compile-time non-negative integer. |
| `FRS-EVAL-0005` | `eval` | A symbol redefined with a different value in the same scope. |
| `FRS-EVAL-0006` | `eval` | A slider or `nentry` whose init value is outside its `[min, max]` range. |
| `FRS-EVAL-0099` | `eval` | Any other evaluation failure, including a failed `case` match, an unresolvable `component`/`library`, and evaluator recursion limits. |

### 7.3 Connecting the blocks (`FRS-PROP-*`)

| Code | Stage | Means |
|---|---|---|
| `FRS-PROP-0001` | `propagate` | A box construct propagation does not support. |
| `FRS-PROP-0002` | `propagate` | A composition arity mismatch — `:`, `<:`, `:>`, or UI wiring. The rule and the computed values come with it. |
| `FRS-PROP-0003` | `propagate` | A recursion (`~`) whose feedback arities do not satisfy the recursive contract. |
| `FRS-PROP-0004` | `propagate` | Automatic differentiation (`fad`/`rad`) reached a clock-domain boundary it cannot cross. |
| `FRS-PROP-0099` | `propagate` | Any other propagation failure. |

### 7.4 Signal values and the user interface (`FRS-COMP-*`, `FRS-UI-*`)

| Code | Stage | Means |
|---|---|---|
| `FRS-COMP-0004` | `type_inference` | A signal type or interval violates a typing rule: an out-of-range `soundfile` part, an unbounded variable delay, an invalid table operand, a math operand outside its domain. Also the severity-`warning` form of the last one under `--warn`. |
| `FRS-COMP-0005` | `compiler` | An internal invariant guard. Reaching it means a compiler bug, not a DSP mistake. |
| `FRS-COMP-0006` | `transform` | Under `--warn`, a const generated table has frozen `ma.SR` to the explicitly requested `--table-init-sample-rate` value rather than using the host initialization rate. |
| `FRS-COMP-0007` | `compiler` | `-e` expansion cannot serialize the evaluated program: no output signal, or a box shape with no Faust source syntax. |
| `FRS-UI-0001` | `propagate` | Two or more controls claim the same runtime address. Every conflicting declaration is located. |

### 7.5 Lowering and code generation (`FRS-SFIR-*`, `FRS-FIR-*`, `FRS-CODEGEN-*`)

These are the deepest stages. A diagnostic here usually means "this construct,
or this combination of options, is not supported on the path you selected" —
rewriting the DSP is rarely the answer; changing the backend or the options
often is.

| Code | Stage | Means |
|---|---|---|
| `FRS-SFIR-0001` | `transform` | Invalid options passed to signal→FIR lowering. |
| `FRS-SFIR-0002` | `transform` | An empty signal list reached lowering. |
| `FRS-SFIR-0003` | `transform` | A signal output arity mismatch. |
| `FRS-SFIR-0004` | `transform` | A signal construct lowering does not support. |
| `FRS-SFIR-0005` | `transform` | A binary operator lowering does not support. |
| `FRS-SFIR-0006` | `transform` | An input index out of range. |
| `FRS-SFIR-0007` | `transform` | A clocked construct (`ondemand`/`upsampling`/`downsampling`) on a path that cannot lower it yet. |
| `FRS-SFIR-0008` | `transform` | Clock-domain inference or graph validation failed. |
| `FRS-SFIR-0009` | `transform` | The foreign variable `count` was used under `-ec`/`-os`, where no block count exists. |
| `FRS-SFIR-0010` | `transform` | A block-sensitive reverse-AD operation under `-os`, where block-boundary semantics have no one-sample meaning. |
| `FRS-FIR-0001` | `fir` | The FIR verifier rejected the lowered module (fatal). |
| `FRS-FIR-0002` | `fir` | A FIR verifier warning; fatal under `--fir-verify-strict`. |
| `FRS-CODEGEN-0001` | `codegen` | Backend code generation failed. |

`FRS-CODEGEN-0001` covers every backend on purpose: the failure class is the
same and the backend is a parameter. Which backend, and its own finer code,
travel as the typed `backend` and `codegen_code` facts. The FIR verifier codes
work the same way, inside `FRS-FIR-000{1,2}` as `fir_code`.

### 7.6 When there is no code

A few failures still arrive as unstructured text — mostly deep backend errors.
Under `--error-format json` they still produce a well-formed envelope, but with
`"code": null`. Treat `code == null` as "legacy error text, read `message`",
never as a code to look up.

### 7.7 Codes that will never be reused

`FRS-COMP-0001`, `FRS-COMP-0002` and `FRS-COMP-0003` were retired in 2026-07-21
and their numbers are burned permanently — reusing a number for a new meaning is
the same silent break the freeze rule prevents, only delayed. The gap in
`FRS-COMP-*` numbering is deliberate.

---

For the engineering view of this table — where each code is raised, which ones
are currently unreachable and why, the extraction command that derives the set
from source, and the checks that keep all of it honest — see
[`docs/diagnostics-codes-reference-en.md`](diagnostics-codes-reference-en.md).

## Related documents

- `docs/user-diagnostics-guide-en.md` — the short operational guide: how to run
  the compiler, read one error, and choose a verbosity.
- `docs/diagnostics-codes-reference-en.md` — the engineering reference for the
  frozen code table.
- `docs/diagnostics-v2.schema.json` — the machine contract, with
  `docs/diagnostics-v2-example.json` as a worked payload.
- `porting/mcp-server-analysis-and-plan-2026-07-21-en.md` — the MCP surface
  planned on top of this model.
- <https://faustdoc.grame.fr/manual/errors/> — the reference C++ compiler's
  error chapter, the baseline compared against throughout this document.
