# Faust-rs Error Flow (Parser -> Eval -> Propagate)

> Scope: concise technical reference for contributors.
> See also: `porting/faust-rust-diagnostics-model-en.md` for the full diagnostics architecture.

## 1. End-to-end flow

1. `parser::parse_program` / `parse_file_with_imports` builds `ParseOutput`.
2. `ParseOutput` carries:
   - `root`: parsed definitions root (`TreeId`),
   - `state.arena`: shared tree arena used by later stages,
   - `state.ctx`: parser metadata (`def_prop`/`use_prop`, cursor, parser counters),
   - `diagnostics`: structured parser diagnostics (`DiagnosticBundle`).
3. `compiler::Compiler::pipeline_to_signals` runs:
   - `eval::eval_process` (boxes resolution),
   - `propagate::box_arity` then `propagate::propagate` (signals and arity checks).
4. `EvalError` / `PropagateError` are converted to structured diagnostics through
   `diagnostics::IntoDiagnostic`, then enriched in compiler with context:
   - node id and compact expression previews,
   - owner definition / alias binding trace,
   - source labels resolved from parser properties.
5. CLI (`crates/compiler/src/main.rs`) renders diagnostics in:
   - human mode (`--error-format human`),
   - JSON mode (`--error-format json`),
   - with verbosity (`--error-verbosity standard|debug`).

## 2. Parser context contract

`ParserCtx` (in `crates/parser-proto/src/context.rs`) is the parser-local replacement for
legacy parser globals:

- source cursor (`file`, `line`, `col`, `end_line`, `end_col`),
- parse/recovery counters,
- definition/use source properties (`DEFLINEPROP`, `USELINEPROP`),
- parser diagnostics stream.

Important boundary:

- parser phase records source metadata,
- eval/propagate do not recompute source positions,
- compiler aggregation resolves spans from parser metadata and adds fallback notes when unavailable.

## 3. Error classes and code families

Stable diagnostics identifiers are defined in
`crates/diagnostics/src/codes.rs`.

- Source reader/import: `FRS-SRC-*`
- Lexer: `FRS-LEX-*`
- Parser: `FRS-PARSE-*`
- Eval: `FRS-EVAL-*`
- Propagate: `FRS-PROP-*`
- Compiler orchestration: `FRS-COMP-*`

Main phase-local error enums:

- `eval::EvalError`
  - missing `process`,
  - undefined symbol / scope visibility,
  - application/pattern arity issues,
  - malformed intermediate nodes,
  - iterative/count validity issues.
- `propagate::PropagateError`
  - unsupported box family,
  - input/output/seq/split/merge/rec composition mismatches,
  - invalid integer payload constraints.

## 4. Diagnostic payload model

All phases converge to `diagnostics::Diagnostic`:

- `severity`, `stage`, `code`, `message`,
- `labels` (primary/secondary source spans),
- `notes` (cause/rule/computed/context),
- `help` (actionable fixes).

`DiagnosticBundle` is the transport object exposed by `CompilerError::diagnostics()`.

## 5. Current source-label strategy

Compiler aggregation tries, in order:

1. direct node span from parser `use_prop` / `def_prop`,
2. nearest descendant span,
3. owning definition span,
4. process-level fallback span.

If no reliable origin span is available, diagnostics remain explicit with fallback notes
instead of silently dropping context.

## 6. Practical debugging contract

For diagnostics work and parity investigations:

- use human mode for readability (`--error-format human`),
- use debug verbosity when low-level node data is needed (`--error-verbosity debug`),
- use JSON mode to lock machine-consumable contracts in tests/snapshots.
