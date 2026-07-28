# Compiler diagnostics v2 — G8 renderers and tooling

**Date:** 2026-07-28

**Baseline:** `19e4d567` (`Complete semantic diagnostic guidance`)

## Objective

G1–G7 built the information: source maps, occurrence provenance, typed facts,
traces, fixes, categories, and warnings. G8 is the phase where that information
becomes visible and consumable, and where a gate starts defending the contract.

The terminal renderer was the last place still carrying less than the JSON
channel: it printed one label, one line, no traces and no fixes. That is fixed
here, along with the verbosity ladder, path policy, published compatibility
rules, and the two checkers the plan asks for.

## Human rendering

Rendering moved out of `cli/diagnostics.rs` into `cli/human.rs`, which keeps the
machine channel and the terminal channel from sharing accidental policy.

**Every relevant label.** Labels are ordered primary-first then by source
position, deduplicated, and rendered with their message. Labels landing on one
line share that line's snippet, so a duplicate declaration shows one source line
with two caret rows rather than the same line twice. A label in a different file
opens a `--> path` line, which is what makes an import-chain diagnostic
readable.

**Multi-line spans.** A span crossing lines shows its first and last line with
an elision marker between them. The two boundaries identify the construct;
echoing everything in between would bury the diagnostic.

**Caret placement.** Columns count Unicode scalars in the raw line while the
rendered line has its tabs expanded, so the caret offset is the display width of
the expanded raw prefix — not a scalar count. A zero-width span still gets one
caret, because an insertion point is a position the reader needs.

**Traces and fixes.** Traces render as one arrow-joined line per trace, bounded
below `Full`. A fix prints its applicability, which is the difference between an
edit that can be applied blindly and one that needs thought.

## Verbosity ladder

`--error-verbosity` now takes four values, each a superset of the one below, so
raising the level never hides something the reader just saw:

| Level | Adds |
| --- | --- |
| `concise` | header, blamed location, first help |
| `standard` | every relevant label, cause/rule/computed notes, traces, fixes |
| `debug` | internal ids and previews, typed debug context |
| `full` | untruncated traces, related diagnostics |

`standard` remains the default and remains the contract: the complete
actionable cause and nothing else.

## Path policy

`--diagnostic-paths` selects `absolute` (default), `relative` to the working
directory, or `basename`. It is presentation only. The JSON channel always
reports the path the compiler used, because a tool resolving a range needs that
path, not a prettier one.

## Published contract

`docs/diagnostics-v2.schema.json` was published in G2 and is validated per
negative-corpus entry by `crates/compiler/tests/cli_diagnostics_channel.rs`.
G8 adds what a consumer still had to guess:

- a question-to-field table, so an agent never parses prose;
- the fix-application rules (which applicability levels are safe, and why edits
  are applied back to front);
- explicit compatibility rules — `schema_version` gates breaking changes, codes
  are frozen, unknown fields and unknown enum values must degrade gracefully,
  `range` is canonical and `compatibility_span` is derived.

These live in `docs/user-diagnostics-guide-en.md`, next to the guidance a human
reader needs, rather than in a separate machine-only document that would drift.

## Checkers

**`xtask diagnostics-quality-check`** enforces two silent-breakage risks:

1. every variant of a serialized diagnostics enum (`Severity`, `Stage`,
   `DiagnosticCategory`, `LabelRole`, `TraceKind`, `Applicability`,
   `SourceKind`) appears in the matching schema enum, and every declared
   `FRS-*` code is both registered in `all_codes()` and documented;
2. no code outside a named two-file allowlist derives machine meaning from note
   text. The allowlist entries are presentation decisions — the paired
   composition block and the canonical note order — and must not grow.

The gate was verified to reject: removing `conflicts_with` from the schema's
label-role enum makes it fail with the exact variant named.

**`crates/compiler/tests/machine_applicable_fixes.rs`** holds the fix contract
end to end. It drives the real binary, applies every `machine_applicable` fix's
edits to the source bytes, recompiles, and requires both that the targeted
diagnostic is gone and that no new parse error appeared. A companion test
requires that a rename suggestion is never marked machine-applicable, since it
changes which definition runs.

## API mapping

| Surface | Mapping | Compatibility impact |
| --- | --- | --- |
| `--error-verbosity` | `adapted` | gains `concise` and `full`; `standard` and `debug` unchanged |
| `--diagnostic-paths` | Rust extension | human presentation only |
| human diagnostic text | `adapted` | now shows secondary labels, traces, and fixes |
| JSON v2 payload | `1:1` | unchanged; only its documentation grew |
| `cli::diagnostics` renderer entry points | internal | human rendering moved to `cli::human` |

Stable diagnostic codes, exit status, the JSON schema, generated code, and
external C/C++ ABI surfaces are unchanged.

## Failure modes and mitigation

| Risk | Mitigation |
| --- | --- |
| richer output overwhelms the reader | four-level ladder; `standard` shows only the actionable cause |
| carets drift on tabs or wide glyphs | offsets computed as display width of the expanded raw prefix, with a tab-line test |
| a long trace floods the terminal | bounded to six frames with an explicit `(+N more)` below `full` |
| a fix is claimed applicable but is not | applied for real and recompiled by an independent checker |
| the schema drifts from the model | `diagnostics-quality-check`, verified against a rejecting mutation |
| a note prefix becomes a machine protocol again | same gate, with a closed two-file allowlist |
| absolute paths leak into shared logs | `--diagnostic-paths relative` / `basename` |

## Not done

SARIF and LSP adapters are listed as optional in the plan and are not
implemented. Nothing in the model blocks them: both are pure projections of the
v2 payload — SARIF from `code`/`category`/`labels`/`fixes`, LSP from
`lsp_position` plus the same labels — and neither needs a change to the core
diagnostic type. They belong to whichever integration first needs them, not to
this phase.

## Structural coverage

- secondary labels appear at `standard` and are suppressed at `concise`;
- two labels on one line share a single snippet;
- a caret lands at display column four on a tab-indented line;
- a multi-line span renders both boundaries with an elision;
- a fix renders with its applicability;
- `--diagnostic-paths basename` shortens the header;
- applying a machine-applicable fix clears its diagnostic and adds no new one;
- a rename suggestion is never machine-applicable;
- the quality gate fails on a schema enum that lost a live variant.

## Phase gate

G8 passes when formatting, Rustdoc, Clippy, workspace tests, golden snapshots,
CLI transcripts, and every structural checker — including the new
`diagnostics-quality-check` — are green. With it, the G0–G8 plan is complete:
provenance is preserved from source to backend, machine meaning lives in typed
fields, and both channels render what the compiler actually knows.
