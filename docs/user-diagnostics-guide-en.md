# Faust-rs Diagnostics Guide (User)

This guide is the short operational reference: how to run the compiler, read one
error, and choose a verbosity.

For the full picture — why the model differs from the C++ compiler, what each
severity/category/verbosity level is for, how the JSON channel and MCP fit
together, and a worked example of every error family — see
[`docs/faust-error-model-en.md`](faust-error-model-en.md).

## 1. Run with diagnostics

```bash
# Human output (default)
cargo run -p compiler -- --dump-sig tests/corpus/err_03_propagate_split_mismatch.dsp --error-format human

# Human output with extra internal context (debug)
cargo run -p compiler -- --dump-sig tests/corpus/err_03_propagate_split_mismatch.dsp --error-format human --error-verbosity debug

# JSON output (for tooling/automation)
cargo run -p compiler -- --dump-sig tests/corpus/err_03_propagate_split_mismatch.dsp --error-format json
```

## 2. How to read one error

Typical human output includes:

- header: `error [FRS-...-....] message`,
- source label with line/caret when available,
- notes:
  - `cause:` why the compiler failed,
  - `rule:` semantic rule being checked,
  - `computed:` concrete values computed by the compiler,
  - context notes (`expr=...`, owner definition, alias trace),
- `help:` concrete fix suggestions.

Example interpretation:

- `FRS-PROP-0002` means a Phase 4 connection/arity mismatch.
- If notes say `split(A, B) requires inputs(B) % outputs(A) == 0`, the fix is to make
  `inputs(B)` a multiple of `outputs(A)`.

## 3. Error families (quick map)

- `FRS-PARSE-*`: lexer/parser syntax/recovery issues.
- `FRS-EVAL-*`: box evaluation issues (`process`, symbols, arity, iteration).
- `FRS-PROP-*`: signal propagation/connectivity issues.
- `FRS-SRC-*`: source loading/import resolution issues.
- `FRS-UI-*`: user-interface layout issues (duplicated control addresses).
- `FRS-COMP-0004`: signal typing and interval issues, including math-domain
  errors and, under `--warn`, potential run-time domain warnings.

## 4. Verbosity levels

`--error-verbosity` selects one of four levels. Each is a superset of the one
below it, so raising the level never hides something you just saw.

| Level | Shows |
|---|---|
| `concise` | header, the blamed location, and the first help line |
| `standard` (default) | every relevant label, `cause`/`rule`/`computed` notes, traces, and fixes |
| `debug` | plus internal ids and previews (`node_id`, `box_expr`) and the typed debug object |
| `full` | plus untruncated traces and related diagnostics |

Use `standard` day to day, `concise` when you only want to be routed to the
failing line, and `debug`/`full` for bug reports and parity investigations.

`--diagnostic-paths` controls how source paths are spelled in human output:
`absolute` (default), `relative` to the working directory, or `basename` when
sharing a diagnostic without disclosing directory structure. The JSON channel
always reports the path the compiler actually used, because a tool resolving a
range needs it.

## 5. Warnings

`--warn` reports non-blocking observations, currently the "operand may leave
its mathematical domain at run time" class that the reference compiler reports
under `-wall` / `-me`. Warnings:

- go to **stderr** in both formats, because on success stdout carries the
  generated output;
- never change the exit status;
- are off by default, since they describe values that only exist at run time
  and would otherwise be noise on programs that clamp their operands in ways
  interval inference cannot see.

## 6. JSON contract (for tools and agents)

The payload is schema v2, published as `docs/diagnostics-v2.schema.json` with a
worked example in `docs/diagnostics-v2-example.json`.

### 6.1 Read typed fields, never prose

Every machine fact has a typed home. `message`, `notes`, and `help` are
presentation text whose wording may change at any time; nothing in them is part
of the contract.

| Question | Field |
|---|---|
| What failed? | `code`, `detail_code`, `category`, `stage` |
| Where do I edit? | the label with `role: "primary_cause"`, then its `range` |
| What else is involved? | other labels, by `role` — `conflicts_with`, `call_site`, `import_site` |
| What rule was violated? | `facts` (for example `expected`, `actual`, `ui_path`, `interval`) |
| How was this reached? | `traces[]` frames |
| Can I apply a fix? | `fixes[].applicability` plus `fixes[].edits` |
| Is this my DSP's fault? | `category` — `compiler_bug` means report it upstream |
| Is this result stale? | `sources[].content_hash` |

### 6.2 Applying fixes

Apply `machine_applicable` edits without review; they are deterministic
repairs. `maybe_incorrect` may change DSP semantics — a rename to a visible
symbol changes which definition runs — so it needs a human or an explicit
opt-in. `has_placeholders` and `manual` are templates and guidance, not edits
to run.

Edits within one fix are ordered and non-overlapping, and are applied together
or not at all. Apply them back to front so an earlier edit cannot shift a later
one's offsets.

### 6.3 Compatibility rules

- `schema_version` changes only for a breaking change. Reject an unknown value
  rather than guessing.
- `FRS-*` code meanings are frozen. New codes may appear; existing ones are
  never renumbered or repurposed.
- New fields may be added to any object. Ignore unknown fields.
- New values may appear in any enum (a new `stage`, `role`, or `category`).
  Degrade gracefully rather than failing.
- `detail_code` is pass-local and free to change; treat it as a refinement of
  `code`, never as a substitute.
- `range` (source id plus half-open UTF-8 byte offsets) is canonical.
  `compatibility_span` is a derived 1-based line/column view for humans.

`cargo run -p xtask -- diagnostics-quality-check` enforces that the schema
keeps up with the model and that no code recovers machine meaning from note
text.

Under `--error-format json`, the payload is written to **stdout alone**: no
human-readable prefix line precedes it, and stdout is a single well-formed
JSON document with no leading or trailing non-JSON bytes, on both success and
failure paths that emit one. (`--error-format human` is unaffected by this
and keeps writing to stderr exactly as before.) A `CompilerError` variant
that carries no structured bundle (backend codegen failures, unresolved
imports) still gets a minimal envelope with `"code": null` rather than
silence, so a JSON consumer never has to special-case "no output". See the
frozen code table in `docs/faust-error-model-en.md` for the full `FRS-*`
list, including which codes are reachable in practice today.

## 7. `--check`: diagnostics without codegen

`--check` runs the full front-end (parse → eval → propagate → type) plus FIR
verification, does no code generation, and exits `0` (no errors) or `1`
(errors). Under `--error-format json` it **always** emits a payload, with an
empty `diagnostics` array on success, so success and failure share exactly
one schema:

```bash
cargo run -p compiler -- tests/corpus/rep_01_passthrough.dsp --check --error-format json
# {"diagnostics": []}

cargo run -p compiler -- tests/corpus/err_03_propagate_split_mismatch.dsp --check --error-format json
# {"diagnostics": [{"code": "FRS-PROP-0002", ...}]}
```

This is the mode automated tooling (CI, an IDE, a future MCP server) should
prefer over `--dump-cpp`/`--dump-sig` when it only needs to know whether a
DSP is valid: it is the same front-end work with no codegen or dump-text
side channel to filter out.
