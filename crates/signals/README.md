# signals

Signal-graph construction and pattern-matching helpers backed by `tlib::TreeArena`.

Signals are the Faust compiler's representation between box propagation and
code-generation.  They carry the full DSP computation graph: math operations,
delays, tables, UI bindings, and recursion projections.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/signals/signals.hh` / `signals.cpp` | Signal node families and constructors |
| `compiler/signals/binop.hh` | Binary operator opcodes |
| `compiler/extended/*prim.hh`, `xtended.hh` | Extended math functions, integrated as first-class signal nodes |
| `compiler/signals/sigFIR.*` / `sigIIR.*` | FIR/IIR carrier algebra (`filter_algebra`) |

## Public API

| Item | Description |
|---|---|
| `SigBuilder` | Construction API — aligned with the `BoxBuilder` style in `crates/boxes` |
| `SigMatch` / `match_sig` | Pattern-matching API — structural inspection of a signal node |
| `SigId` | `TreeId` alias for signal nodes |
| `dump_sig_readable` | Debug pretty-printer for a signal subtree |
| `add_sig_fir`, `extract_iir_filter`, `FirFilter`, `IirFilter`, … | FIR/IIR carrier algebra, re-exported from the private `filter_algebra` module |
| `ad_rules` | Backend-neutral local reverse-AD rule classification and formulas shared by `propagate` and FIR/BRA lowering |

## Parity invariants

- Signal nodes are represented as tagged trees with **deterministic child order**.
- Numeric constants are direct `Int` / `FloatBits` nodes (no wrapper indirection).
- Extended math (`compiler/extended/`, C++ `xtended`) is not a separate layer:
  `sin`, `pow`, `fmod`, `min`/`max`, … are ordinary `SigMatch` / `SigBuilder`
  nodes of this crate.
- UI control signals carry a stable `ui::ControlId` only; labels, ranges and
  layout live in `ui::UiProgram`.
- Public integer surface uses `i32` semantics; underlying storage remains `NodeKind::Int(i64)`.
- `BlockReverseAD` is a semantic Signal-IR carrier for `rad(...)` block
  fallback; concrete tapes, loop scheduling, and storage typing are owned by
  `transform::signal_fir`.
- `ad_rules` is the narrow exception to this crate being representation-only:
  it contains backend-neutral local RAD algebra for math/binop nodes, but does
  not allocate tapes or inspect temporal scheduling.

Signal typing lives in `sigtype`, interval arithmetic in `interval`.

## Position in the pipeline

Canonical pipeline (AGENTS.md): parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend.

Signals are the IR produced by `propagate` and read by every later stage
(`normalize`, `sigtype`, `transform`), up to the signal → FIR lowering.
