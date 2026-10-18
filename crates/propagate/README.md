# propagate

Box-to-signal propagation — Phase 4, stage 4.3 of the porting plan.

Takes the evaluated box tree from `eval` and produces a flat signal list by
recursively applying the Faust composition algebra.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/propagate/propagate.hh` / `propagate.cpp` | Core propagation logic |
| `compiler/boxes/boxtype.cpp` | Box arity inference |

## What this crate does

- **Arity inference** (`box_arity_typed`) — determines `(inputs, outputs)` for any flat box.
- **Signal lowering** (`propagate_typed_with_ui`) — walks the box tree and emits `signals` nodes via `SigBuilder`.
- **Composition algebra** — `seq`, `par`, `split`, `merge` with explicit bus routing.
- **Recursive forms** — De Bruijn-style placeholders (`sigRec` / `sigProj` shape).
- **Grouped UI** — builds a canonical `UiProgram` as an explicit propagation product.
- **Forward-mode AD** — expands `fad(expr, seed)` directly in Signal IR with
  one tangent lane per seed output.
- **Reverse-mode AD** — expands `rad(expr, seeds)` to
  `[primals..., gradients...]`; feed-forward local algebra is symbolic, while
  temporal/recursive bodies route through the `BlockReverseAD` carrier for FIR
  block replay.

## Public API

### Entry points (post-`eval/a2sb`, on a validated `FlatBoxId`)

| Function | Description |
|---|---|
| `try_build_flat_box(arena, box_id) -> Result<FlatBoxId, FlatBoxBuildError>` | Validate a raw `BoxId` against the flat box subset |
| `box_arity_typed(arena, flat, &mut cache) -> Result<BoxArity, PropagateError>` | Infer `(inputs, outputs)` (memoized in `ArityCache`) |
| `propagate_typed_with_ui(arena, flat, inputs, &mut cache, &ui_options) -> Result<PropagateOutput, PropagateError>` | Lower to signals, signal origins, grouped UI and clock domains |
| `propagate_typed(arena, flat, inputs, &mut cache) -> Result<Vec<SigId>, PropagateError>` | DSP signals only (no UI, no provenance) |
| `propagate_fold(arena, box_id, &mut session) -> Result<Vec<SigId>, PropagateError>` | Constant-folding propagation for `eval`, caches kept in a `FoldSession` |
| `control_widgets(arena, flat) -> ControlWidgets` | Control inputs and bargraphs of a box, in interface order (for `cinputs`/`coutputs` and `"*"` modulation) |
| `make_sig_input_list(arena, n)` | Ordered input signal list (`sigInput(0)` … `sigInput(n-1)`) |

### Types

| Item | Description |
|---|---|
| `FlatBoxId`, `FlatBoxBuildError` | Validated flat-box handle and its rejection error |
| `BoxArity` | `{ inputs: usize, outputs: usize }` pair |
| `ArityCache` | Arity memo tables: typed, wiring, and the two forward-AD reachability walks |
| `PropagateOutput` | `signals`, `signal_origins: SignalOrigins`, `ui: UiProgram`, `clock_domains: ClockDomainTable` |
| `PropagateUiOptions` | Grouped-UI construction policy (synthesized root label) |
| `FoldSession` | Caches of constant folding, kept for one evaluation pass |
| `ControlWidget`, `ControlWidgets`, `UiGroupContext` | Control list returned by `control_widgets` |
| `PropagateError`, `DependentSeed` | Typed errors (arity, unsupported nodes, AD coherence, dependent seeds) |
| `ClockDomain`, `ClockDomainId`, `ClockDomainKind`, `ClockDomainTable` | Clock-domain side table (`clock_domain` module) |

### Modules

| Module | Visibility | Description |
|---|---|---|
| `clock_domain` | public | Clock-domain side table for `ondemand` / `upsampling` / `downsampling` |
| `stateful_rad` | public | Linearity classifier for recursive groups (RAD diagnostics, LTI gate) |
| `transpose_ad` | public | LTI transpose of a recursive group; used only by dormant helpers, not by `rad` lowering |
| `forward_ad` | internal | FAD transform over Signal IR, including multi-seed and recursive layouts |
| `reverse_ad` | internal | RAD symbolic sweep plus `BlockReverseAD` fallback construction |

RAD local math rule classification and formulas are shared with FIR/BRA
lowering through `signals::ad_rules`; this crate owns the symbolic
Signal-IR traversal and adjoint accumulation.

`CRATE_NAME` / `crate_id()` give the crate identity string.

## Integer convention

Integer signals emitted by this pass use `i32` semantics.  Conversions from
`usize` indices are explicit and fallible to preserve deterministic diagnostics
on overflow.

## Position in the pipeline

Canonical pipeline (AGENTS.md): parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend.

`propagate` is also called by `eval` for constant folding (`propagate_fold`),
and builds the grouped `UiProgram` (`ui_build.rs`) used from here onward.
