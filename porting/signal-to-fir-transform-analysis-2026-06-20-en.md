# Signal → FIR transformation: complete step-by-step analysis

**Date:** 2026-06-20
**Scope:** `crates/transform` (`signal_prepare` + `signal_fir`)
**Audience:** maintainers of the fast-lane lowering pipeline
**Companion docs:** `faust-rust-fir-architecture-en.md`, `factorization-god-files-plan-2026-05-25-en.md`,
`fir-cse-runtime-optimizations-plan-2026-04-03-en.md`, `delay-manager-design-2026-04-06-en.md`,
`rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md`.
**Formal companion:** [`signal-to-fir-rewriting-calculus-2026-06-20-en.md`](signal-to-fir-rewriting-calculus-2026-06-20-en.md)
— the same pipeline framed as typed rewriting between refinement sorts, deriving the `W#` findings
below as type/coverage/soundness obligations (and surfacing one new gap: `verify` enforces a
predicate weaker than the lowering precondition, omitting `P` and `D1`).

> **Update (2026-06-21, branch `delay_rewrite1`):** the **delay subsystem** described below has since
> been restructured. The two delay-analysis walks (`analyze_signals` + `scan_signals`) were unified
> into one `plan_delays` pass returning a `DelayPlan` (**resolving W7** and the still-open part of
> I2), the five strategy abstractions were collapsed into a single `DelayKind` enum, and `delay.rs`
> was split into a `signal_fir/delay/` module. The delay-specific sections, their `file:line`
> references, and the `DelayStrategyEmitter` description below predate that work — see
> [`delay-rs-simplification-experiment-2026-06-21-en.md`](delay-rs-simplification-experiment-2026-06-21-en.md)
> and the 2026-06-21 journal entry. The rest of this document (staging, RAD/BRA, placement) is
> unaffected.

---

## 0. Position in the compiler pipeline

```
boxes → propagate → signals (+ UiProgram)
                       │
                       ▼
        ┌─────────────────────────────────────────────┐
        │ transform                                     │
        │                                               │
        │  signal_prepare  ──►  signal_fir  ──►         │
        │   (staging)           (lowering)              │
        └─────────────────────────────────────────────┘
                       │
                       ▼
                   fir → codegen (C / C++ / WASM / Cranelift / FBC)
```

`transform` is the **mid-level lowering layer**: it sits between propagation (which owns the
signal model and recursion-as-de-Bruijn) and the FIR backends (which own code generation).
The single public entry point is:

```rust
compile_signals_to_fir_fastlane_with_ui(arena, signals, num_inputs, num_outputs, ui, options)
    -> Result<SignalFirOutput, SignalFirError>     // signal_fir/mod.rs:211
```

It is a **fast-lane**: a deliberately partial port of the C++ `signalFIRCompiler` family that
covers an executable subset (arithmetic, delays, recursion, tables, UI, foreign calls, and
reverse-mode AD). Unsupported node families are rejected with typed `FRS-SFIR-*` errors rather
than mis-compiled.

The top-level function performs exactly three things ([`mod.rs:211`](../crates/transform/src/signal_fir/mod.rs)):

1. `planner::plan_signals(...)` — contract gate.
2. `signal_prepare::prepare_signals_for_fir_verified(...)` — staging + verification.
3. `module::build_module(...)` — FIR emission.

The rest of this document walks every sub-step of stages 2 and 3, then analyses weaknesses
and refactoring opportunities.

---

## 1. End-to-end step map

### Stage 1 — Contract gate (`planner.rs`)

| # | Step | Function |
|---|------|----------|
| 1 | Validate options + I/O arity | `plan_signals` |

### Stage 2 — Preparation / staging (`signal_prepare.rs`)

The body of `prepare_signals_for_fir_unverified` ([`signal_prepare.rs:383`](../crates/transform/src/signal_prepare.rs)) is a fixed
linear sequence. It performs ~16 full-forest traversals:

| # | Step | Function | Kind |
|---|------|----------|------|
| 2.1 | Clone output forest into a private arena | `TreeArena::clone_forest_from` | rebuild |
| 2.2 | de Bruijn → symbolic recursion | `tlib::de_bruijn_to_sym` | rewrite |
| 2.3 | Canonicalize degenerate (unary) projections | `canonicalize_unary_rec_projections` | rewrite |
| 2.4 | Type-annotate (#1) | `infer_full_types` → `sigtype::TypeAnnotator` | analysis |
| 2.5 | Promote / insert casts (#1) | `normalize::promote_signals_fastlane` | rewrite |
| 2.6 | Type-annotate (#2) | `infer_full_types` | analysis |
| 2.7 | Algebraic simplify (#1) | `normalize::simplify_signals_fastlane` | rewrite |
| 2.8 | Merge isomorphic recursion groups | `normalize::merge_isomorphic_symrec_groups` | rewrite |
| 2.9 | Type-annotate (#3) | `infer_full_types` | analysis |
| 2.10 | Algebraic simplify (#2) | `simplify_signals_fastlane` | rewrite |
| 2.11 | Canonicalize `Delay(x,1)` → `Delay1(x)` | `canonicalize_one_sample_delays` | rewrite |
| 2.12 | Type-annotate (#4) | `infer_full_types` | analysis |
| 2.13 | Promote / insert casts (#2) | `promote_signals_fastlane` | rewrite |
| 2.14 | Type-annotate (#5, final) | `infer_full_types` | analysis |
| 2.15 | Derive reduced `SimpleSigType` map | `derive_simple_types` | projection |
| 2.16 | Verify postconditions | `PreparedSignals::verify` | analysis |

### Stage 3 — Lowering / emission (`signal_fir/module/build.rs::build_module`)

| # | Step | Function |
|---|------|----------|
| 3.1 | Sharing / variability pre-pass | `placement::analyze_signal_sharing` |
| 3.2 | Construct lowering engine | `SignalToFirLower::new` |
| 3.3 | Ensure `fSampleRate` field | `ensure_sample_rate_var` |
| 3.4 | Delay analysis + scan + alloc | `prepare_delay_lines` (`DelayManager::analyze_signals` + `scan_signals` + `ensure_delay_line`) |
| 3.5 | Classify reverse-time (RAD) outputs | `classify_reverse_time_outputs` |
| 3.6 | Lower forward outputs | `lower_output_signal` → `lower_signal` (recursive dispatch) |
| 3.7 | Lower reverse-time outputs (RAD) | `lower_output_signal` with `lowering_reverse_loop=true` |
| 3.8 | Declare output channel pointers | inline in `build_module` |
| 3.9 | Emit compute()-entry adjoint resets | `emit_reverse_time_rec_compute_resets`, `emit_bra_compute_resets` |
| 3.10 | CSE materialization per bucket | `cse::count_fir_value_uses` + `materialize_shared_values` |
| 3.11 | Assemble lifecycle functions | `build_module` (metadata / instanceConstants / reset / clear / buildUI / compute) |
| 3.12 | Emit math prototypes + module | `build_module` (`MATH_PROTO_ORDER`, `INT_FUN_PROTO_ORDER`, `b.module`) |

---

## 2. Stage 1 — Contract gate

### Step 1 · `plan_signals` ([`planner.rs:38`](../crates/transform/src/signal_fir/planner.rs))

- **Input:** `signals: &[SigId]`, `num_inputs`, `num_outputs`, `&SignalFirOptions`.
- **Output:** `SignalFirPlan { signal_count, num_inputs, num_outputs }`.
- **Invariants guaranteed on success:**
  - `options.module_name` non-empty after trim;
  - `signals` non-empty;
  - `num_outputs == signals.len()` (strict top-level output contract).
- **Justification:** fail fast on malformed entry conditions before any expensive staging
  or arena cloning, and produce a stable, cheap "shape" snapshot that downstream code can
  trust. The planner is explicitly a *contract gate, not an optimizer*.
- **Position:** first, because it is the cheapest filter and the rest of the pipeline assumes
  its invariants (e.g. `build_module` iterates `0..plan.num_outputs`).

---

## 3. Stage 2 — Preparation (`signal_prepare`)

The preparation stage transforms the *propagated* signal forest (which may still contain de
Bruijn recursion, mixed int/real arithmetic, and non-canonical delay forms) into a **staged
forest that satisfies the fast-lane lowering contract**. It is performed in a *private* arena
so the caller's source arena is never mutated.

The C++ provenance is a fusion of `normalform.cpp` (`deBruijn2Sym`, `typeAnnotation`),
`box_signal_api.cpp` (`boxesToSignalsMLIR`), and `sigtyperules.cpp` (reduced type inference).

### Step 2.1 · Clone forest into a private arena ([`signal_prepare.rs:388`](../crates/transform/src/signal_prepare.rs))

- **In:** source arena + output roots. **Out:** cloned roots in a fresh `TreeArena`.
- **Invariant:** the source arena is left untouched; downstream rewrites are isolated.
- **Justification:** all later steps mutate the arena (interning new nodes for casts,
  symbolic recursion, canonical delays). Isolation makes the pass re-entrant — notably the
  SIGGEN interpreter (§5.5) re-enters `prepare_signals_for_fir` on sub-forests.
- **Position:** must be first; everything else writes into this arena.

### Step 2.2 · de Bruijn → symbolic recursion ([`signal_prepare.rs:391`](../crates/transform/src/signal_prepare.rs))

- **In:** cloned forest packed into a single cons-list root (`vec_to_list`). **Out:** the same
  forest with recursion expressed as `SYMREC(var, body_list)` / `SYMREF(var)` instead of de
  Bruijn `REC`/`REF`.
- **Invariant:** no de Bruijn recursion form remains anywhere reachable (checked in `verify`).
- **Justification:** the FIR lowerer reasons over *named* recursion carriers keyed by a stable
  binder id; de Bruijn indices are positional and brittle under sharing. Converting the whole
  output list through a single memo table preserves cross-root sharing exactly as the C++
  normalization does.
- **Position:** before typing, because typing and all later rewrites assume the symbolic form.

### Step 2.3 · Canonicalize unary recursion projections ([`signal_prepare.rs:979`](../crates/transform/src/signal_prepare.rs))

- **In/Out:** forest with `proj(k, group)` rewritten to `proj(0, group)` whenever `group` is a
  symbolic recursion binder whose body list has arity 1.
- **Invariant:** every projection of a one-output recursion group uses slot 0 (checked in
  `verify`: "non-canonical index" error).
- **Justification:** the lowerer stores recursion carriers in dense `Vec<slot>` form; allowing
  arbitrary logical indices on degenerate groups would force every consumer to re-canonicalize.
- **Explicit limitation (documented in-file):** this is **not** a port of the C++
  `inlineDegenerateRecursions(...)`. It does *not* build the recursive dependency graph, detect
  degenerate projections through graph analysis, or rewrite projection definitions under delays.
  It only normalizes already-materialized symbolic shapes of arity 1. See §6 for the consequence.
- **Position:** immediately after `de_bruijn_to_sym`, so all later steps see canonical indices.

### Steps 2.4 / 2.6 / 2.9 / 2.12 / 2.14 · Type annotation ([`signal_prepare.rs:1210`](../crates/transform/src/signal_prepare.rs))

- **In:** forest + `UiProgram`. **Out:** `HashMap<SigId, SigType>` with interval bounds,
  variability, and nature for every reachable node, produced by the canonical
  `sigtype::TypeAnnotator`.
- **Invariant:** after each typing pass, every reachable node has a full `SigType`.
- **Justification:** typing is the single source of truth that drives the *next* rewrite:
  - #1 guides promotion (where casts go),
  - #2 drives simplification on the promoted graph,
  - #3 re-establishes types after the merge,
  - #4 re-establishes types after delay canonicalization, before the second promotion,
  - #5 is the final map exposed to FIR lowering (interval-based delay sizing, variability
    placement) and reduced into `SimpleSigType`.
- **Position:** interleaved — each rewrite that can change types is followed by re-typing.
  `transform` no longer runs its own recursive typer; it defers entirely to `sigtype`/`normalize`.
- **Cost note:** five full annotations. This is the dominant cost of the stage (§6, finding W1).

### Steps 2.5 / 2.13 · Signal promotion ([`normalform.rs:202`](../crates/normalize/src/normalform.rs))

- **In:** forest + current type map. **Out:** forest with explicit `IntCast` / `FloatCast` /
  `BitCast` nodes inserted at every domain boundary.
- **Invariant (the "promotion invariant", documented on `build_module`):**
  - every `BinOp` operand is domain-consistent with the op (mixed Int/Real → `FloatCast`,
    bitwise/shift → `IntCast`, `Div` operands always Real);
  - every integer-context operand of `Delay`/`RdTbl`/`WrTbl`/`Select2`/`Enable` is `IntCast`;
  - `Delay1(x)` and `Prefix(init, x)` satisfy `type(init) == type(x)`.
- **Justification:** with all coercions materialized as tree nodes, the FIR lowerer **never
  inserts implicit casts** — `lower_binop`, `lower_delay_state`, etc. simply assert the contract
  and emit a typed instruction. This keeps the lowerer simple and backend-neutral.
- **Why twice:** the first promotion runs on the freshly typed symbolic forest; simplify/merge/
  delay-canonicalization can then alter the shape, so a second promotion re-establishes the
  invariant on the final forest.
- **Position:** #1 right after the first typing; #2 as the penultimate rewrite (after delay
  canonicalization) so the forest handed to lowering provably satisfies the invariant.

### Steps 2.7 / 2.10 · Algebraic simplification ([`normalform.rs:212`](../crates/normalize/src/normalform.rs))

- **In:** forest + type map. **Out:** algebraically simplified forest (identity/zero
  elimination, constant folding, etc.), memoized per pass.
- **Invariant:** semantics-preserving; failures fall back to the unsimplified signal
  (`catch_unwind` guard returns the original `SigId`).
- **Justification:** reduces the number of FIR nodes emitted and exposes further canonical
  forms (e.g. degenerate delays for step 2.11). Running it after promotion means it simplifies
  the *cast-annotated* graph, matching C++ ordering.
- **Position:** #1 after promotion+typing; #2 after the recursion merge+typing, to clean up
  the merged graph before delay canonicalization.

### Step 2.8 · Merge isomorphic recursion groups ([`rec_merge.rs:26`](../crates/normalize/src/rec_merge.rs))

- **In/Out:** forest with structurally-identical `SYMREC` groups unified.
- **Justification:** independent recursive sub-circuits that propagation emitted separately but
  that are structurally identical should share one carrier, reducing state and duplicate
  per-sample work.
- **Position:** after the first simplify (so isomorphism is detected on canonical bodies) and
  before re-typing.

### Step 2.11 · Canonicalize one-sample delays ([`signal_prepare.rs:996`](../crates/transform/src/signal_prepare.rs))

- **In/Out:** `Delay(x, Int(1))` rewritten to `Delay1(x)`.
- **Invariant:** downstream consumers (recursion-feedback resolution, SIGGEN interpreter) see a
  single canonical one-sample-delay form.
- **Justification:** `normalize` may legally expose unary feedback as `Delay(x, 1)`; the merged
  recursion-carrier fast paths and `match_recursion_delay_key` only recognize `Delay1`.
- **Position:** after simplification (which can produce `Delay(x,1)` from folding) and before the
  final promotion/typing.

### Step 2.15 · Derive reduced `SimpleSigType` ([`signal_prepare.rs:1227`](../crates/transform/src/signal_prepare.rs))

- **In:** final full type map. **Out:** `HashMap<SigId, SimpleSigType>` over `{Int, Real, Sound}`.
- **Rules:** `Soundfile` → `Sound`; an *unresolved recursive projection* (a fully unconstrained
  self-loop, see `is_unresolved_recursive_projection`) is forced to `Real`; otherwise reduce by
  `Nature` (`Int`→`Int`, `Real`/`Any`→`Real`).
- **Justification:** the FIR lowerer only needs the three-way distinction for state/table/result
  type selection (`Int32` vs `real_ty` vs `Sound`). Keeping a separate reduced view avoids
  threading the full lattice through every lowering method while preserving the canonical map
  for interval-based delay sizing.
- **Position:** last, derived from the final type map.

### Step 2.16 · Verify postconditions ([`signal_prepare.rs:177`](../crates/transform/src/signal_prepare.rs))

- **In:** the built `PreparedSignals` + `UiProgram`. **Out:** `Ok(())` or `SignalPrepareError::Validation`.
- **Checks (a structural boundary verifier):**
  - no de Bruijn rec/ref remain; no *bare* `SYMREF` (must sit under a `Proj`); no legacy `SIGREC`;
  - every reachable node has both a reduced and a full type, and the stored reduced type equals
    the freshly-derived one;
  - recursion group arities are internally consistent; projection indices are in bounds; unary
    groups use index 0;
  - `BlockReverseAD` structural invariants (non-empty body, `body.len() == cotangents.len()`,
    `primal_count == body.len()`, projection slots within `M+N`);
  - write-table read-only/write placeholder pairs are consistent;
  - referenced UI control ids exist in the `UiProgram`;
  - output arity is unchanged across staging.
- **Justification:** turns "the constructor probably produced something valid" into a typed
  guarantee carried by `VerifiedPreparedSignals`, failing *at the stage boundary* rather than
  deep inside lowering. This is the contract that `build_module`'s promotion-invariant documentation
  depends on.
- **Position:** terminal; `prepare_signals_for_fir_verified` returns the verified wrapper.

---

## 4. Stage 3 — Lowering (`signal_fir/module`)

`build_module` ([`build.rs:102`](../crates/transform/src/signal_fir/module/build.rs)) drives a stateful engine,
`SignalToFirLower` ([`mod.rs:316`](../crates/transform/src/signal_fir/module/mod.rs)), with ~45 side-channel
fields (statement buckets, state maps, delay/recursion/BRA sub-state, counters). Lowering is
stateful because FIR output has many parallel channels (value expressions, six lifecycle
sections, struct/static/global declarations, math prototypes) that are filled during one DAG
traversal and assembled at the end.

### Step 3.1 · Sharing / variability pre-pass ([`placement.rs:173`](../crates/transform/src/signal_fir/placement.rs))

- **In:** prepared forest + full type map. **Out:** three maps:
  - `ref_counts[sig]` — parent fan-out (a node with ≥2 parents is *shared*);
  - `has_higher_parent` — nodes at a variability boundary (a parent runs at a faster tier);
  - `konst_escapes` — `Konst` nodes consumed by a faster tier, **plus** all `Konst` descendants
    of a `BlockReverseAD` carrier (conservative, because BRA synthesizes compute-time uses not
    visible as DAG edges).
- **Invariants/justification:** these feed the Phase-1 placement gate (§5.1) so each value is
  emitted **once, in the correct lifecycle tier** (`instanceConstants` / compute preamble /
  sample loop), matching Faust's Konst/Block/Samp execution tiers.
- **Position:** before lowering, so the gate has stable counts. One DFS; children descended once.

### Step 3.2–3.3 · Engine construction + sample rate ([`setup.rs:13`](../crates/transform/src/signal_fir/module/setup.rs))

- Builds the lowering engine and registers the canonical `fSampleRate` struct field that all
  backends consume (rather than synthesizing their own).

### Step 3.4 · Delay analysis, scan, allocation ([`setup.rs:109`](../crates/transform/src/signal_fir/module/setup.rs))

`prepare_delay_lines` runs two read-only DAG walks then allocates:

1. **`DelayManager::analyze_signals`** ([`delay.rs:1231`](../crates/transform/src/signal_fir/delay.rs)) —
   accumulates a delay counter down the tree (`Delay(+amount)`, `Delay1(+1)`, `Prefix(+1)` on the
   carried edge; non-delay nodes reset to 0; recursion bodies re-enter at 0). Records the maximum
   delayed access per reachable signal (`delay_analysis`, keyed by `SigId`) and per recursion
   output (`rec_output_analysis`, keyed by `(var_id, proj_index)`).
2. **`DelayManager::scan_signals`** ([`delay.rs:1262`](../crates/transform/src/signal_fir/delay.rs)) —
   returns `carried_signal → max_delay` for standalone delay lines (`Delay(value, amount)` and
   shift-strategy `Delay1(value)`), while `try_record_rec_delay` detects the
   `Delay1^k(Proj(i, SYMREF))` feedback-through-delay pattern and *excludes* those carriers from
   `fVec` allocation (they will be merged into the recursion array).
3. **`ensure_delay_line`** per carrier — selects a strategy from the `-mcd`/`-dlt` thresholds and
   declares the `fVec*`/`iVec*` struct array + its `instanceClear` zeroing loop.

- **Delay strategy selection** ([`delay.rs:1578`](../crates/transform/src/signal_fir/delay.rs)):

  | Delay range | Strategy | Buffer size | Pointer |
  |-------------|----------|-------------|---------|
  | `[1, mcd)` | `Shift` | `delay+1` (exact) | none (shift/copy each sample) |
  | `[mcd, dlt)` | `CircularPow2` | `next_pow2(delay+1)` | shared `fIOTA` + mask |
  | `[dlt, ∞)` | `IfWrapping` | `delay+1` (exact) | per-line `fIdx*` counter |

- **Amount resolution** ([`delay.rs:356`](../crates/transform/src/signal_fir/delay.rs)): literal `Int` →
  interval upper bound (`check_delay_interval`) → structural `min(Int(n), _)` fallback. Unbounded
  variable amounts are rejected as `UnsupportedSignalNode`.
- **Invariant:** every delay line is sized and declared **before** any read/write is emitted, so
  lowering only *queries* geometry, never allocates opportunistically.
- **Justification:** decoupling sizing (here) from emission (during lowering) lets a recursion
  output carry its own delayed history without a separate buffer (recursion+delay merging), and
  keeps geometry decisions deterministic and single-sourced.
- **Position:** after engine construction, before any `lower_signal`.

### Step 3.5 · Classify reverse-time outputs ([`mod.rs:233`](../crates/transform/src/signal_fir/module/mod.rs))

- **Out:** a `bool` mask parallel to `signals`: `true` for outputs that are gradient projections
  of a `ReverseTimeRec` or a public `BlockReverseAD` (slot ≥ `primal_count`).
- **Key subtlety:** the recursive classifier **stops at `SYMREC` boundaries**. A `Proj(slot, SYMREC)`
  primal output is always forward-time, even if its body internally consumes a BRA gradient
  (the adaptive-update case). Recursing into the body would misclassify the output and suppress
  the causal forward loop.
- **Justification:** decides whether `compute()` needs a second loop running `count-1 .. 0`.
- **Position:** after delay prep, before lowering, because it partitions the output set into the
  forward and reverse lowering slices.

### Step 3.6 · Forward-output lowering ([`core_lowering.rs:209`](../crates/transform/src/signal_fir/module/core_lowering.rs))

For each non-reverse output, `lower_output_signal` → `lower_signal` (§5), then casts the value at
the external `FaustFloat` boundary and stores `output{i}[i0]`. After all forward outputs:
`delay.emit_sample_end_updates` appends `fIOTA`/`fIdx` advances to the `sample_end` phase, and the
flattened phases (`immediate` → `post_output` → `sample_end`) become the forward sample loop body.

### Step 3.7 · Reverse-output lowering (RAD) ([`build.rs:197`](../crates/transform/src/signal_fir/module/build.rs))

If any output is reverse-time: the cache is cleared, `lowering_reverse_loop` is set, and the
reverse outputs are lowered into a fresh phase set, becoming a second sample loop emitted with
`is_reverse = true`. `lower_forward_output_delay1_for_reverse_loop` ([`core_lowering.rs:717`](../crates/transform/src/signal_fir/module/core_lowering.rs))
replays `Delay1(primal)` from the already-written forward output buffer rather than running a
recursion carrier backward.

### Steps 3.8–3.9 · Output pointers + adjoint resets

Declares `output{i}` channel pointer aliases, then `emit_reverse_time_rec_compute_resets`
(zeroes LTI adjoint carriers; dormant) and `emit_bra_compute_resets` (zeroes BRA `fBraCarry*` /
`fBraDelayCarry*` fields at every `compute()` entry, treating each host call as one TBPTT block).

### Step 3.10 · CSE materialization per bucket ([`cse.rs:163`](../crates/transform/src/signal_fir/cse.rs))

Phase 2 runs *after* placement (Phase 1) has finalized each bucket. For each of
`constants_statements`, `control_statements`, and each sample loop independently:
`count_fir_value_uses` counts FIR-level fan-out, then `materialize_shared_values` wraps every
non-trivial value referenced ≥2 times in `DeclareVar(prefix<N>) + LoadVar(prefix<N>)`, inserted
at the point of first use. Counters continue from where Phase-1 placement left off
(`fConst`/`iConst` for constants, `fSlow`/`iSlow` for control, fresh `fTemp`/`iTemp` for loops).

- **Justification:** placement decides *which tier* a value lives in; CSE deduplicates shared
  sub-expressions *within* that tier so backends emit each once. Operating on the `FirStore`
  means all backends benefit.
- **Position:** after both sample loops are built (so reference counts are stable) and before
  function assembly.

### Steps 3.11–3.12 · Lifecycle assembly + prototypes ([`build.rs:291`](../crates/transform/src/signal_fir/module/build.rs))

Assembles the six Faust lifecycle functions in deterministic order — `metadata`,
`instanceConstants` (with a prepended `fSampleRate` store), `instanceResetUserInterface`,
`instanceClear`, `buildUserInterface` (from `emit_ui_program`), `compute` (control statements +
the one or two `for` loops) — plus math/integer/foreign prototypes (in the stable
`MATH_PROTO_ORDER` / `INT_FUN_PROTO_ORDER`) and the DSP struct, globals, and static tables, then
`b.module(...)`.

---

## 5. `lower_signal` internals (the recursive core)

`lower_signal` ([`core_lowering.rs:23`](../crates/transform/src/signal_fir/module/core_lowering.rs)) is the single
memoized dispatcher. It (a) returns the cached `FirId` if present, (b) dispatches on `match_sig`,
(c) applies the Phase-1 placement gate, and (d) caches the result (except recursive projections).

### 5.1 · Placement gate (Phase 1 application)

A non-trivial lowered value is hoisted into its variability bucket when
`!is_trivial_fir && !is_recursive_projection && !WrTbl && (shared || at_boundary)`:
`Konst` → `materialize_in_bucket(Constants)` (struct storage iff it escapes), `Block` →
`materialize_in_bucket(Control)`, `Samp` → stays inline. This is the runtime half of the
placement analysis from §3.1.

### 5.2 · Delay lowering ([`core_lowering.rs:491`](../crates/transform/src/signal_fir/module/core_lowering.rs), `delay.rs`)

`Delay`/`Delay1`/`Prefix` resolve through three layers: (1) recursion-carrier merge
(`resolve_recursion_delay_ref` — a `Delay1^k(Proj)` chain reads the recursion array at an offset),
(2) the dedicated single-sample state slot (`lower_delay_state`, a 2-element `fIOTA`-indexed
circular cell), or (3) a pre-allocated standalone line emitted via the
`DelayStrategyEmitter` trait (`Shift` / `RingDelay<CircularPow2 | IfWrapping>`).

### 5.3 · Recursion lowering ([`arithmetic.rs:235`](../crates/transform/src/signal_fir/module/arithmetic.rs), `recursion.rs`, `state.rs`)

`lower_proj` has four fast paths (active `SYMREF` carrier, materialized scalar current value,
materialized array carrier, `BlockReverseAD`) before the general path: decode the group, allocate
one carrier per body slot (scalar / exact-shift / circular, *sized from the delay analysis*),
push the group onto the active stack, lower **all** bodies once (multi-output groups snapshot
every body before any carrier store, so lanes don't read each other's updated slot), emit current
writes + finalize shifts, then return the requested slot. The `with_active_recursion_group`
helper keeps the push/pop stack balanced even on error. Recursion state is keyed by `(group, index)`
in `RecursionState`, intentionally **disjoint** from `state_name_by_node` (the tf22 aliasing
hazard).

### 5.4 · Block Reverse AD ([`bra.rs`](../crates/transform/src/signal_fir/module/bra.rs), `block_reverse_ad.rs`, `rad_formula_builder.rs`)

`lower_block_reverse_ad_proj`: primal slots lower the body and schedule forward **tape stores**
(`fBraTape*`) for non-trivially-reverse-evaluable operands; gradient slots run
`ensure_bra_backward_sweep` once and read the per-seed adjoint from `bra_grad_cache`. The sweep:
collect a unified postorder (`collect_bra_postorder`), lower cotangents, pre-seed recursive
feedback carries (matching `SYMREF(var)` to its `SYMREC(var)` body by binder id, **not** by flat
slot), seed cotangent contributions, walk the postorder in reverse calling `propagate_bra_adj`
(per-node chain rule via the shared `signals::ad_rules`, with tape-aware operand loads), and cache
seed gradients. The same sweep code lands in the reverse loop (public gradient) or inline in the
forward loop (adaptive update) depending only on the caller's scheduling context.

### 5.5 · Tables ([`tables.rs`](../crates/transform/src/signal_fir/module/tables.rs), `siggen.rs`)

`Waveform`/`RdTbl`/`WrTbl` lower to FIR tables; constant-size `WrTbl(size, gen, …)` generators are
evaluated at **compile time** by `interpret_generator` ([`siggen.rs:51`](../crates/transform/src/signal_fir/siggen.rs)),
a small step interpreter that — notably — re-enters `prepare_signals_for_fir` on the generator
sub-forest so it observes the same promoted/typed shape as the lowering path.

### 5.6 · UI, foreign, leaves

UI controls/bargraphs/soundfiles lower through `ui_lowering.rs` against the `UiProgram`; inputs
load `input{c}[i0]` with a `FaustFloat → real_ty` cast; foreign functions/variables/constants
lower to extern prototypes/globals; `fSamplingFreq` specially loads the `fSampleRate` int field.

### Cross-cutting invariants

- **Type duality:** internal computation uses `real_ty` (Float32/Float64); only audio buffers and
  UI zones use `FaustFloat`. Casts are emitted at exactly four boundaries (input load, output
  store, UI read, bargraph write).
- **Sample phases:** every per-sample body is assembled `immediate` → `post_output` → `sample_end`
  so that state writes observe the current sample's outputs before shifting.
- **No implicit casts in the lowerer** (relies on the promotion invariant from §3, step 2.5).

---

## 6. Weaknesses and gaps

> Findings are labelled **W#**. Each was verified against the current source.

**W1 — Five full type annotations per preparation (performance).**
`prepare_signals_for_fir_unverified` runs `TypeAnnotator::annotate` over the whole forest five
times, interleaved with two promotions and two simplifications, for ~16 full-forest traversals
total. Typing is the dominant cost and is recomputed wholesale after each rewrite even when only
a sub-region changed. (Cf. the in-flight `MEMOIZATION.md` / `propagate memoize` work.)

**W2 — `canonicalize_unary_rec_projections` is not `inlineDegenerateRecursions`.**
Documented explicitly in-file ([`signal_prepare.rs:54`](../crates/transform/src/signal_prepare.rs)). It performs only
arity-1 index canonicalization, with no recursive dependency-graph analysis or projection-definition
rewriting under delays. Programs that the C++ compiler would simplify via degenerate-recursion
elimination may lower to less optimal FIR (extra carriers) or expose shapes the fast-lane then
rejects.

**W3 — `is_unresolved_recursive_projection` is a fragile, divergent fallback.**
([`signal_prepare.rs:1258`](../crates/transform/src/signal_prepare.rs)) forces a fully unconstrained self-loop to
`Real` by structural pattern match, *overriding* the canonical `sigtype` result (which, following
the C++ `TREC` approximation, keeps integer-preserving feedback in `Int`). This is a hand-tuned
compatibility hack; an edge case could silently get the wrong reduced type.

**Update (2026-06-20): removed.** The C++ `TREC` seed types a recursive component as `Int`,
interval `{0}` (see `sigtype::rules::initial_rec_type`), and an unconstrained `x = x` fixpoint
converges to exactly that — so the canonical typer already yields `Int`, matching C++. The fast-lane
override to `Real` was therefore a *divergence from C++*, not a clarification. It has been deleted:
`derive_simple_types` (α) is now a pure homomorphism of the canonical map, and the pinning test
asserts `Int`. No regression across `transform`, `compiler`, or `box-ffi`.

**W4 — The preparation sequence is a hand-ordered pseudo-fixpoint, not a proven one.**
The order promote→type→simplify→merge→type→simplify→canonicalize→type→promote→type is tuned by
hand. There is no argument that it is closed: e.g. nothing re-canonicalizes a `Delay(x,1)` if one
were (re)introduced after step 2.11, and nothing re-merges recursion groups made isomorphic by the
second simplify. It works for the current corpus but is brittle to add new rewrites into.

**W5 — `MAX_BRA_TAPE_BLOCK_SIZE = 8192` is a silent correctness cliff.**
([`mod.rs:195`](../crates/transform/src/signal_fir/module/mod.rs)) BRA forward tapes are fixed-size struct arrays.
A host calling `compute(count > 8192)` with a `BlockReverseAD` carrier overflows the tape with no
runtime guard and no compile-time rejection — "the host must not" is the only protection.

**Update (2026-06-20):** the tape index is now masked (`i0 & (MAX_BRA_TAPE_BLOCK_SIZE - 1)`, a
power-of-two no-op for the supported block size), so an over-long block wraps *within* the array —
instead of writing out of bounds. The UB is gone, but correctness is not: after one wrap, the
forward tail overwrites the tape slots needed by the backward prefix. For example, `count = 9000`
corrupts gradients at samples `0..807`, not the tail that performed the overwrite. The
`count ≤ MAX` precondition still governs exact gradient correctness, and arbitrary-length support
remains a larger change (explicit chunked TBPTT or a dynamically sized tape). The seed-liveness,
dead-tape, and remediation analysis is continued in
[`rad-bra-tape-safety-liveness-analysis-2026-07-27-en.md`](rad-bra-tape-safety-liveness-analysis-2026-07-27-en.md).

**W6 — Dead/unused analysis surface.**
- `DelayManager::rec_group_max_delay` is **write-only**: populated by `try_record_rec_delay`
  ([`delay.rs:1531`](../crates/transform/src/signal_fir/delay.rs)) but never read (recursion sizing uses
  `rec_output_analysis` instead). The `merged` *boolean* return is used; the stored *value* is not.
- The per-signal `delay_analysis` map produced by `analyze_signals` has no readers (its accessor
  carries `#[allow(dead_code)]`), so `record_delay_analysis` runs on every delayed node for nothing.
- `SignalFirOptions::strict_mode` is never read in logic (set in defaults/tests only).
- `get_delay_line` still carries `#[allow(dead_code)]` although it now has a caller (stale allow).

**Update (2026-06-21): all four items resolved.** `rec_group_max_delay` and the per-signal
`delay_analysis` map were removed; `SignalFirOptions::strict_mode` was dropped; the stale
`#[allow(dead_code)]` on `get_delay_line` was cleaned up and the function now has a live caller
(`delay_line_info` in `state.rs`). The only remaining `#[allow(dead_code)]` in transform is in
`block_reverse_ad.rs`, unrelated to this item. See also I12 (resolved).

**W7 — Two overlapping delay-analysis passes.**
`prepare_delay_lines` walks the DAG twice: `analyze_signals` (accumulated-delay analysis,
recursion-output sizing) and `scan_signals` (standalone-line ownership + legacy merge bookkeeping).
Their responsibilities overlap (both detect recursion-feedback-through-delay), and the legacy half
feeds the dead `rec_group_max_delay` (W6). This is two traversals doing one job's worth of useful work.

*Resolved (2026-06-21, `delay_rewrite1`)*: the two walks were folded into one `plan_delays` pass
returning a `DelayPlan` (a `DelayPlanner` visitor); equivalence with the old walks was proven by a
differential `debug_assert_eq!` across the full test suite before they were deleted.

**W8 — Lowering-vs-verification boundary mismatch.**
`prepare`'s verifier explicitly accepts `OnDemand`, `Upsampling`, `Downsampling`, `Clocked`,
`ZeroPad`, `Fir`, `Iir`, and `AssertBounds` nodes, but `lower_signal`'s dispatch has **no arm**
for them — they fall through to the generic `UnsupportedSignalNode` error deep inside lowering.
A program using those families passes the staged contract and then fails late with a generic
message instead of being rejected (or supported) at a clear boundary. (The clock-domain gap is
also tracked in `project_ondemand_clock_domains`.)

**Update (2026-06-20):** this acceptance is *by design* — preparation is a deliberate superset of
the fast-lane lowerer, enforced by `prepare_signals_for_fir_accepts_filter_carrier_children` and
`prepare_signals_for_fir_recovers_shared_zero_pad_amount_from_float_context` (an attempt to reject
these families in `verify` broke both). So the resolution is to *lower* a family when a DSP needs it,
not to reject it; the genuine residual defect is only the *late, generic* failure. See the formal
companion's §8.2 and the `lowering_coverage` drift-guard test.

**W9 — `SignalToFirLower` is a god object.**
~45 fields spanning delay, recursion, BRA, UI, CSE counters, placement, and statement buckets,
with behavior spread over eight `impl` submodules. The split-borrow `*Ctx` bundles
(`DelayFirCtx`, `RecursionLoweringCtx`, `RecursionAllocCtx`, `DelayLoweringCtx`) are manual
struct-literal workarounds for borrowing disjoint fields — explicitly documented as
"do not construct via `&mut self`". This is recognized in `factorization-god-files-plan`.

**Update (2026-06-21): resolved** by plan
`signal-to-fir-lower-struct-decomposition-plan-2026-06-20-en.md` (7 commits).
`SignalToFirLower` went from ~51 fields to 14 by extracting 7 cohesive sub-state structs:
`ModuleSections` (`state.rs`), `UiLoweringState` (`ui_lowering.rs`), `UsedPrototypes`
(`arithmetic.rs`), `NameGen` + `PlacementInfo` (`setup.rs`), `RadReverseState` (`build.rs`),
`BraState` (`bra.rs`). The `*Ctx` split-borrow bundles remain (inherent to `store + sub-manager`
disjointness) but are now documented as such. See I4 (resolved).

**W10 — Structural-string identity for reverse-loop primal replay.**
`forward_output_by_sig_key` is keyed by `dump_sig_readable(...)` strings to survive "equivalent but
non-identical `SigId`s" ([`mod.rs:434`](../crates/transform/src/signal_fir/module/mod.rs)). String-equality identity is
O(string) and can collide; it also implies the prepared arena does **not** guarantee that
structurally-identical signals share a `SigId`, which (if true) undermines other interning
assumptions, or (if false) makes this fallback dead weight.

**W11 — Reverse loop clears the whole cache.**
`build_module` does `lower.cache.clear()` before the reverse slice ([`build.rs:203`](../crates/transform/src/signal_fir/module/build.rs)),
so any sub-expression shared between forward and reverse loops is lowered twice into duplicate FIR.
Likely needed for loop-scope correctness, but it is a blunt instrument and undocumented as to why
a per-loop scratch cache wouldn't suffice.

**W12 — Recursive re-preparation in the SIGGEN interpreter.**
`interpret_generator` calls `prepare_signals_for_fir` (the full ~16-traversal stage) on each
generator sub-forest, and `eval_rdtbl` recurses into `interpret_generator` for nested writable
tables ([`siggen.rs:448`](../crates/transform/src/signal_fir/siggen.rs)). Nested tables therefore re-run the entire
preparation pipeline per nesting level with no caching — a potential blow-up.

**W13 — Coarse error taxonomy.**
Almost every lowering failure maps to `UnsupportedSignalNode` (FRS-SFIR-0004): "delay too large",
"unbounded variable delay", "soundfile used as state", "unknown node", and "clock family not
lowered" are indistinguishable to a caller keying on the stable code.

**W14 — Order-dependent `konst_escapes` under shared BRA subgraphs.**
In `analyze_sig_rec` the `inside_block_reverse_ad` flag only propagates to children on the single
descent (`visited` gate), while the `konst_escapes` insertion is checked on every visit. For a
`Konst` reachable both under and outside a `BlockReverseAD`, whether its *children* are marked
escaping depends on which parent descended first. The pass is conservative (so likely safe), but
the correctness argument rests on visit order rather than on the analysis being order-independent.

**W15 — Unreachable surplus-output path.**
`plan_signals` enforces `num_outputs == signals.len()`, which makes the `signal_index >= num_outputs`
"evaluate-and-drop" branch in `lower_output_signal` ([`core_lowering.rs:229`](../crates/transform/src/signal_fir/module/core_lowering.rs))
unreachable. The contract and the lowerer disagree about whether surplus signals are supported.

---

## 7. Improvements and factorizations

> Labelled **I#**, roughly ordered by value/effort. Several map directly to the W-findings.

**I1 — Collapse the preparation type passes (addresses W1, W4).**
Replace the five wholesale annotations with either (a) an interleaved type+rewrite fixpoint driven
to convergence, or (b) incremental re-annotation that only re-types subtrees a rewrite touched.
Even a measured reduction to two annotations (post-promotion, final) would roughly halve the
stage's dominant cost. Make the sequence's closure explicit (or assert it) so new rewrites can be
inserted safely.

**I2 — Unify the two delay-analysis passes and delete dead state (addresses W6, W7).**
Fold `analyze_signals` and `scan_signals` into one DAG walk that yields *both* per-carrier max
delays and recursion-output analysis; drop `rec_group_max_delay` and the per-signal
`delay_analysis` map (or wire the latter to a real consumer); turn `try_record_rec_delay` into a
pure predicate. Removes one full traversal and a write-only map.

*Resolved (2026-06-21, `delay_rewrite1`)*: the dead state (`rec_group_max_delay`, `delay_analysis`
map, `strict_mode`) was deleted, and `analyze_signals` + `scan_signals` were folded into one
`plan_delays` pass (a `DelayPlanner` visitor) — W7 closed. I2 fully resolved.

**I3 — Make the tape size safe (addresses W5).**
Either size `fBraTape*` from an explicit max-block-size option, emit a compile-time rejection /
runtime guard when `count` can exceed the cap, or switch to a dynamically-sized scratch buffer.
A silent overflow on large blocks is the highest-severity latent bug here.

**I4 — Decompose `SignalToFirLower` (addresses W9).**
Group fields into sub-managers — `BraState` (the six `bra_*` fields + caches), a
`MaterializationState` (the `fConst/iConst/fSlow/iSlow` counters + caches), `UiLoweringState`,
joining the existing `DelayManager`/`RecursionState`. The manual `*Ctx` split-borrow bundles then
become ordinary methods on those sub-managers, removing the "do not construct via `&mut self`"
footgun. (Already scoped in `factorization-god-files-plan`.)

**Resolved (2026-06-21).** See W9 update.

**I5 — Unify placement (Phase 1) and CSE (Phase 2) into one section-aware materializer
(addresses W11, and the `konst_escapes` over-conservatism).**
A single value cache keyed by `(section, FirId)` would (a) let forward/reverse loops share lowered
values without a blunt `cache.clear()`, (b) make the BRA `Konst`-escape rule precise instead of
"every descendant", and (c) remove the fragile counter handoff between the two phases.

**I6 — Reconcile the lowering/verification boundary (addresses W8).**
Either add lowering arms for the clock/filter families the verifier accepts, or have the verifier
(or a dedicated capability check) reject them up front with a specific error code, so "accepted by
prepare" implies "lowerable". Tie this to the clock-domain port plan.

**I7 — Replace string-keyed signal identity (addresses W10).**
Investigate whether the prepared arena already interns structurally-identical signals. If so,
delete `forward_output_by_sig_key` and rely on `SigId`. If not, use a structural hash rather than
`dump_sig_readable` strings, and document the interning guarantee either way.

**I8 — Cache prepared generators (addresses W12).**
Memoize `interpret_generator` results (and the prepared generator forest) by source `SigId` so
nested/repeated tables don't re-run preparation. Alternatively, hoist generator preparation so it
shares the parent forest's type map.

**I9 — Refine the error taxonomy (addresses W13).**
Split `UnsupportedSignalNode` into a few stable codes (`UnsupportedDelay`, `UnboundedDelay`,
`UnsupportedStateType`, `UnsupportedNodeFamily`) so callers and diagnostics can react precisely.

**I10 — Pin or fix the recursion-type divergence (addresses W3).**
Either move the unconstrained-self-loop→`Real` rule into the canonical `sigtype` close (with a test
documenting the intended C++ `TREC` divergence) or gate it behind a clearly named compatibility
flag with a regression test, rather than leaving a silent structural override in `derive_simple_types`.

**I11 — Reconcile the surplus-output contract (addresses W15).**
Either relax `plan_signals` to allow `signals.len() > num_outputs` and keep the drop path, or remove
the unreachable drop branch and assert `signal_index < num_outputs`.

**I12 — Remove stale annotations and dead config (addresses W6).**
Drop `SignalFirOptions::strict_mode` (or wire it), and clean stale `#[allow(dead_code)]` on
`get_delay_line`.

**Resolved (2026-06-21).** See W6 update.

---

## 8. Summary

The fast-lane is a clean, well-documented two-stage design: a **preparation** stage that
normalizes the propagated forest into a verified contract (symbolic recursion, explicit casts,
canonical delays, reduced types) and a **lowering** stage that emits a complete, deterministic FIR
module through a single memoized dispatcher with disciplined variability placement, delay/recursion
state management, and reverse-mode AD scheduling. The verifier-as-boundary and the
promotion-invariant are particular strengths: they let the lowerer stay free of implicit coercions
and fail close to the source of any contract regression.

The principal weaknesses are **cost** (the preparation stage re-types the whole forest five times
and walks the delay DAG twice, W1/W7), **latent correctness cliffs** (the fixed BRA tape size W5,
the divergent recursion-type fallback W3, the order-dependent BRA constant escape W14),
**accreted dead state** (W6) and **boundary inconsistencies** (verifier vs lowerer W8, planner vs
lowerer W15). None of these block the current corpus, but they are exactly the seams that make the
stage hard to extend. The highest-leverage work is I1 (collapse type passes), I2 (unify delay
analysis + delete dead state), I3 (safe tape sizing), and I4/I5 (decompose the god struct and
unify materialization) — the first two reclaim performance, the last three reduce the long-term
maintenance and correctness risk.
