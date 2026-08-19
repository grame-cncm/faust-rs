# docs/

Standalone technical and user documentation for `faust-rs`, independent of
the day-by-day `porting/journal/` and `porting/phases/` history. Most files
come as an English original (`-en.md`) with a French translation (`-fr.md`)
alongside it.

## Start here

- [`developer-workflows-en.md`](developer-workflows-en.md) — build commands,
  diagnostic runs, the `xtask` golden/runtime/alignment workflows, and the
  `scripts/loc_report.py` line-count report. The entry point for working in
  this repository.

## User guides

- [`user-cli-guide-en.md`](user-cli-guide-en.md) — the `faust-rs` compiler
  CLI, option by option.
- [`user-diagnostics-guide-en.md`](user-diagnostics-guide-en.md) — reading
  and using compiler diagnostics.
- [`user-mem0-guide-en.md`](user-mem0-guide-en.md) — the `-mem0` custom
  memory manager.
- [`faustprobe-user-guide-en.md`](faustprobe-user-guide-en.md) — `faustprobe`,
  the generic runtime introspection/probing tool.

## Diagnostics reference

- [`diagnostics-codes-reference-en.md`](diagnostics-codes-reference-en.md) —
  engineering reference for every `FRS-*` diagnostic code.
- [`faust-error-model-en.md`](faust-error-model-en.md) — the error model
  shared across the seven textual backends.
- [`diagnostics-v2.schema.json`](diagnostics-v2.schema.json) /
  [`diagnostics-v2-example.json`](diagnostics-v2-example.json) — the JSON
  diagnostics v2 schema and a worked example, referenced by the two guides
  above.

## Automatic differentiation (FAD/RAD)

- [`fad-note-en.md`](fad-note-en.md) — forward-mode AD (`fad`) design and
  implementation.
- [`rad-note-en.md`](rad-note-en.md) — reverse-mode AD (`rad`) design and
  implementation.
- [`rad-usage-en.md`](rad-usage-en.md) — using `rad(expr, seeds)` for
  gradient descent.
- [`fad-rad-synthesis-en.md`](fad-rad-synthesis-en.md) /
  [`-fr.md`](fad-rad-synthesis-fr.md) — FAD and RAD use cases, a gentler
  synthesis of the two notes above.
- [`fad-debruijn-recursion-en.md`](fad-debruijn-recursion-en.md) —
  forward-mode AD over de Bruijn-encoded recursive signals.

## Recursion and de Bruijn encoding

- [`recursion-debruijn-lowering-en.md`](recursion-debruijn-lowering-en.md) /
  [`-fr.md`](recursion-debruijn-lowering-fr.md) — lowering recursion from
  `Rec` boxes to signals via de Bruijn encoding.
- [`debruijn-recursion-faust-note-en.md`](debruijn-recursion-faust-note-en.md)
  / [`-fr.md`](debruijn-recursion-faust-note-fr.md) (with matching `.pdf`) —
  de Bruijn notation and recursion in the Faust compiler.
- [`debruijn-synthesis-note-en.md`](debruijn-synthesis-note-en.md) /
  [`-fr.md`](debruijn-synthesis-note-fr.md) (with matching `.pdf`) — a
  synthesis of the same subject.
- [`flatnode-rec-to-signals-en.md`](flatnode-rec-to-signals-en.md) /
  [`-fr.md`](flatnode-rec-to-signals-fr.md) — lowering `FlatNodeKind::Rec` to
  signal form.
- [`debruijn-vers-symbolique.pdf`](debruijn-vers-symbolique.pdf) — standalone
  French PDF note, from de Bruijn to symbolic form.

## Vectorization, scheduling, and clock domains

- [`vector-scheduling-synthesis-en.md`](vector-scheduling-synthesis-en.md) /
  [`-fr.md`](vector-scheduling-synthesis-fr.md) — gentle synthesis of
  vectorization, scheduling, and multi-rate in `faust-rs`.
- [`vector-mode-scheduling-formal-spec-guide-en.md`](vector-mode-scheduling-formal-spec-guide-en.md)
  / [`-fr.md`](vector-mode-scheduling-formal-spec-guide-fr.md) — how to read
  the Lean formal specification: the stakes, not the proof details.
- [`ondemand-note-en.md`](ondemand-note-en.md) /
  [`-fr.md`](ondemand-note-fr.md) — clock domains in `faust-rs`: `ondemand`,
  `upsampling`, `downsampling`.
- [`ondemand-fft-spectral-comparison-en.md`](ondemand-fft-spectral-comparison-en.md)
  — frame-rate FFT via `ondemand`, compared with existing spectral
  environments.
- [`signal-to-fir-recent-progress-en.md`](signal-to-fir-recent-progress-en.md)
  — compact summary of Signal → FIR fast-lane work (placement, CSE, delays,
  recursion extraction).

## Formal methods

- [`lean-usage-methodology-en.md`](lean-usage-methodology-en.md) — using
  Lean in the Faust-to-Rust port: methodology, benefits, costs. The
  methodology behind the Lean formalization initiative tracked in
  `porting/`.

## Porting history

- [`faust-cpp-to-rust-port-history-en.md`](faust-cpp-to-rust-port-history-en.md)
  — the full C++-to-Rust porting history.
- [`faust-cpp-to-rust-port-history-overview-en.md`](faust-cpp-to-rust-port-history-overview-en.md)
  — a concise version of the same.

## Generated artifacts

- [`code-graphs/`](code-graphs/README.md) — workspace crate graphs, internal
  dependency graphs, an IR overview, and the public-API baseline used by
  CI's `code-graphs --check` gate. Generated and verified via
  `cargo run -p xtask -- code-graphs`; see its own README for details.
