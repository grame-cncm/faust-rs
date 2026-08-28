# Table Index Clamping — Move to Signal Level + `-ct` Option

**Date**: 2026-08-28
**Status**: Executed — G1..G4 landed 2026-08-28 (see `porting/journal/2026-08-28.md`)
**Motivation**: `faust-rs --dump-sig-dag tests/lean/table_unclamped.dsp` shows a raw
`SIGRDTBL(tbl, idx)` with no bounds protection, yet the generated C++ contains
`std::min<int>(..., 15)`. The clamp is currently invented late, during FIR
lowering, so it is invisible at the signal level and cannot be disabled. The
C++ reference compiler instead rewrites the *signal* (`signalTablePromote`,
gated by `-ct`, default on). This plan moves faust-rs to the same architecture
and adds the `-ct` option.

## C++ Reference

| Site | Behaviour |
|------|-----------|
| `compiler/global.hh:312` | `bool gCheckTable; // -ct to check rtable/rwtable index range` — default `true` (`global.cpp:477`), parsed as `-ct <0|1>` (`global.cpp:1660`), echoed in the compile-options line (`global.cpp:842`) |
| `compiler/normalize/normalform.cpp:115` | in `simplifyToNormalFormAux`, **after** simplification (so the `size` signal is a constant) and re-typing: `if (gGlobal->gCheckTable) L4 = signalTablePromote(L4);` followed by a fresh `typeAnnotation` |
| `compiler/transform/sigPromotion.cpp:577` | `SignalTablePromotion::safeSigRDTbl`: reads the index interval; if `lo < 0 || hi >= size`, rewrites to `sigRDTbl(tbl, sigMax(sigInt(0), sigMin(ri, sigInt(size-1))))` and (under `-wall`) pushes a `WARNING : RDTbl read index [lo:hi] is outside of table size (N)` |
| `compiler/transform/sigPromotion.cpp:608` | `safeSigWRTbl`: same treatment for the **write** index |
| `compiler/extended/{min,max}prim.hh:95` | the interval-based `min`/`max` pruning is *disabled* in C++ ("interval computation is buggy"), so the full `max(0, min(...))` pair survives to codegen |

Reference output (faust 2.87.10) for `tests/lean/table_unclamped.dsp`
(`rdtable(16, 1.0, int(hslider("i",0,0,100,1)))`):

```cpp
ftbl0mydspSIG0[std::max<int>(0, std::min<int>(static_cast<int>(...), 15))]
```

Key architectural point: in C++ the protection is a **signal→signal rewrite**
in the normal form. Codegen (`compileSigRDTbl`) contains no bounds logic at
all; with `-ct 0` the access is emitted raw and unsafe, by design.

## Current faust-rs Behaviour

The protection lives entirely in signal→FIR lowering
(`crates/transform/src/signal_fir/`):

| Site | Behaviour |
|------|-----------|
| `module/tables.rs:81` `lower_rdtbl` | lowers the index, then calls `table_index_with_bounds(ridx_fir, ridx_sig, table_len)` |
| `module/tables.rs:549` `table_index_with_bounds` | interval-driven strategy: interval ⊆ `[0, N-1]` → direct access; `lo ≥ 0, hi > N-1` → `min_i(idx, N-1)` only; `lo < 0` (finite) → `max_i(min_i(N-1, idx), 0)`; unknown/infinite interval → modular wrap `((idx % N) + N) % N` via `normalized_table_index` |
| `module/tables.rs:129` `lower_wrtbl` | write index always goes through `normalized_table_index` (modular wrap, no interval refinement) |
| `vector/lower/signal.rs:1504,1866` | duplicate of the same strategy for the vector lane |

Divergences from C++:

1. **Layer**: decided at FIR lowering, not in the signal forest →
   `--dump-sig-dag` and every signal-level consumer (Lean export, drawing,
   interval reasoning) see an unprotected access.
2. **Shape**: faust-rs emits the *minimal* clamp (`min` only when `lo ≥ 0`);
   C++ always emits the full `max(0, min(...))` pair when out of range.
3. **Unknown intervals**: faust-rs falls back to **modular wrapping**; C++
   treats the unknown interval as `[INT32_MIN, INT32_MAX]` and emits the full
   **clamp**. Different run-time semantics for wild indexes.
4. **No `-ct`**: the protection cannot be disabled, and `-ct 0/1` on the
   command line is currently rejected/ignored instead of honoured.

## Design

### 1. Port `SignalTablePromotion` into the `normalize` crate

New module `crates/normalize/src/table_promote.rs` (the crate already owns the
normal-form pipeline and mirrors `compiler/normalize/` + the signal-level
rewrites; `transform` depends on `normalize`, not the reverse):

```rust
/// Port of C++ SignalTablePromotion (transform/sigPromotion.cpp:577-640).
/// Rewrites SIGRDTBL/SIGWRTBL indexes that the interval analysis cannot
/// prove in-bounds into clamped form, using the same memoized
/// tree-traversal shape as promote_signals_fastlane.
pub fn promote_table_signals(
    arena: &mut TreeArena,
    types: &HashMap<SigId, SigType>,
    sigs: &[SigId],
    warnings: &mut Vec<TableRangeWarning>,
) -> Result<Vec<SigId>, NormalFormError>
```

Per-node rule, strict C++ parity (`safeSigRDTbl` / `safeSigWRTbl`):

- resolve `size` from the table producer (`WrTbl` size child / `Waveform`
  length); it must be a positive integer constant at this point (the pass runs
  after simplify, same guarantee C++ relies on). `size <= 0` → hard error,
  same message shape as C++ (`RDTbl size = N should be > 0`).
- index interval known and `⊆ [0, size-1]` → identity (no node inserted).
- otherwise (including unknown/missing type, treated as
  `[INT32_MIN, INT32_MAX]` exactly like C++ `sigPromotion.cpp:591`) → rebuild
  as `rdtbl(tbl, max(int(0), min(ri, int(size-1))))` with
  `SigBuilder::{min,max,int}` (`crates/signals/src/lib.rs:591,597`), and
  record a `TableRangeWarning { lo, hi, size, sig }`.

Decisions folded into this rule (see "Decisions" below for rationale):

- **Full clamp, not minimal clamp** — byte-parity with reference C++ output.
- **Clamp replaces modular wrap** for unknown intervals — parity with C++
  semantics; behaviour change documented in G2.
- The existing `min`/`max` typing rules already compute clamped intervals
  (`crates/sigtype/src/rules.rs:865-877`), so the retype that follows the pass
  proves the new index in-bounds — downstream consumers need no special case.

### 2. New step in the `signal_prepare` staging pipeline

`crates/transform/src/signal_prepare/mod.rs` — insert after step 2.10
(simplify #2), mirroring the C++ ordering ("must be done after simplification
so that 'size' is properly simplified to a constant"):

```text
2.10  simplify #2
2.10a retype       (fresh sig_types before the typed table pass)   [new]
2.10b table_promote (gated: options.check_table)                   [new]
2.11  canon_one_sample_delays
2.12  retype #4 …                                                  (unchanged)
```

Implementation: one `Staging::promote_tables(&mut self)` method calling
`normalize::table_promote::promote_table_signals` on `self.outputs`, driven by
the existing `retype` discipline (the driver already guarantees fresh types
before every typed pass). Origins propagation: conservative inherit, same as
the promote/simplify steps (`inherit_origins` pattern).

Options threading: `prepare_signals_for_fir*` currently takes no options.
Add a `PrepareOptions { pub check_table: bool }` struct (`Default → true`)
and `_with_options` variants so the many existing test callers stay valid;
`signal_fir::compile_fastlane_inner` (`signal_fir/mod.rs:781`) forwards
`SignalFirOptions.check_table` into it.

Warning channel: surface `TableRangeWarning`s through the same path as the
other semantic warnings (`Compiler.semantic_warnings`, CLI `--warn`,
`DiagnosticBundle`) — this is the faust-rs home of the C++ `-wall` class, per
`cli/args.rs:317-323`.

### 3. Simplify FIR lowering to C++ codegen parity

Once the signal forest is protected, lowering must stop re-deciding:

- `module/tables.rs:96` (`lower_rdtbl`) and `vector/lower/signal.rs:1504`:
  with `check_table = true` the retyped index interval is provably
  `⊆ [0, N-1]`, so `table_index_with_bounds` degenerates to the direct-access
  branch. Replace the call with the raw index plus a `debug_assert!` that the
  interval is in-bounds (belt-and-braces during the transition; C++ has no
  check here at all). Keep `table_index_with_bounds` dead-code-free by
  deleting it and its vector twin once G2 goldens are green.
- `module/tables.rs:129` (`lower_wrtbl`): drop `normalized_table_index` the
  same way — the write index is now clamped at signal level (C++
  `safeSigWRTbl` parity; today's wrap-on-write was already a divergence).
- `check_table = false` (`-ct 0`): raw direct access, no wrap, no clamp —
  exact C++ semantics. `normalized_table_index` therefore has no remaining
  caller in the rdtbl/wrtbl path (audit `siggen.rs` and the iwave path,
  `module/tables.rs:44-77`, which manage their own internal indexes and are
  out of scope — their indexes are compiler-generated and already in-bounds).

### 4. CLI `-ct` option

`crates/compiler/src/cli/args.rs`:

- clap field: `#[arg(long = "check-table")] pub check_table: Option<String>`
  accepting `0`/`1` (validate like the other enum-ish flags in
  `cli/validate.rs`), default `1`.
- single-dash normalization: add `-ct` → `--check-table` in the rewriter loop
  (`args.rs:480` — same value-forwarding shape as `-cn`, `args.rs:502`).
- `Compiler::with_check_table(bool)` (default `true`, next to
  `with_semantic_warnings`, `lib.rs:744`), threaded through
  `SignalLoweringContext` (`signal_lowering.rs:159`) → `SignalFirOptions` →
  `PrepareOptions`.
- Compile-options header parity: the generated banner currently prints
  `-lang cpp -single`; append `-ct 0` when disabled (C++ prints the flag via
  `global.cpp:842`; printing only the non-default keeps our banner stable).
- `runner.rs:268` echo path: include `-ct 0` when set, for provenance parity.

### 5. `--dump-sig-dag` visibility (the original complaint)

`--dump-sig-dag` dumps the **propagated** forest (`source_mode.rs:223`),
before `signal_prepare` — the clamp would still be invisible there, exactly
as C++'s clamp is invisible before `simplifyToNormalForm`. Two-part fix:

- add `--dump-sig-dag-prepared`: runs `prepare_signals_for_fir` (honouring
  `-ct`) on the propagated forest and dumps the prepared arena with the same
  `dump_sig_dag` renderer. This is generally useful (shows promotion casts,
  SYMREC form, and now the clamp) and keeps the existing flag's meaning
  stable for current consumers/tests.
- document in the flag help that `--dump-sig-dag` is pre-normal-form.

## Gates

**G1 — pass ported, unit-tested in isolation.**
`normalize::table_promote` + tests covering: in-bounds interval (identity,
node untouched — assert same `SigId`), `lo ≥ 0 / hi > N-1`, `lo < 0`, unknown
type (full clamp), `wrtbl` write index, nested rdtbl-inside-wrtbl generator
(C++ handles the recursive case via `self(tbl)`), `size ≤ 0` error, warning
payloads. `cargo test -p normalize`.

**G2 — wired into staging, FIR lowering simplified.**
Staging steps 2.10a/2.10b behind `PrepareOptions`; `table_index_with_bounds`
and `normalized_table_index` retired from rdtbl/wrtbl; vector lane updated.
Expected golden churn, to regenerate deliberately:
- `min_i(x, N-1)` → `max_i(min_i(x, N-1), 0)` where an upper-only clamp was
  emitted (matches reference C++);
- modular-wrap sequences on reads/writes with unknown intervals → clamp
  (semantic change for out-of-range indexes: wrap → saturate; this **aligns**
  with reference Faust, flag it in JOURNAL.md);
- `used_protos` churn (`max_i` now required where only `min_i` was).
Full check: `cargo test -p transform -p codegen -p compiler`, then the
impulse-comparison harness against reference faust on the examples corpus
(`examples_compare_full.csv` refresh) — the wrap→clamp cases should *reduce*
diffs vs C++.

**G3 — `-ct` end-to-end.**
CLI parsing (`-ct 0`, `-ct 1`, `--check-table 0`), `Compiler` builder, banner
echo. Behavioural tests: `table_unclamped.dsp` with default/`-ct 1` produces
`std::max<int>(0, std::min<int>(..., 15))` (byte-comparable to reference
faust); with `-ct 0` produces the raw unclamped access, same as
`faust -ct 0`. `cargo test -p compiler -- cli`.

**G4 — dump visibility + docs.**
`--dump-sig-dag-prepared` shows `SIGRDTBL(tbl, MAX(0, MIN(idx, 15)))` for the
motivating file; help text; JOURNAL.md entry recording the wrap→clamp
semantic change and the C++ provenance mapping
(`sigPromotion.cpp:577-640` → `normalize/src/table_promote.rs`).

## Decisions (with recommendation)

1. **Full clamp vs interval-minimal clamp.** Recommended: **full clamp**
   (strict C++ parity). The minimal-`min` refinement was only safe because it
   lived after typing; keeping it at signal level would preserve today's
   output but permanently diverge from reference codegen and complicate
   byte-level compare tooling. The cost (one extra `max` the C++ compiler
   also pays) is negligible; if it ever matters, re-enabling interval-driven
   `min`/`max` pruning in `simplify` is the principled place — C++ has the
   same pruning sketched but disabled (`maxprim.hh:95`).
2. **Unknown interval: wrap vs clamp.** Recommended: **clamp** (C++ parity,
   and it is what `-ct` means). The current wrap is an invention of the FIR
   lowerer; keeping it would make `-ct 1` mean two different protections in
   the two compilers.
3. **Keep a defensive check in FIR lowering?** Recommended: `debug_assert!`
   only, then delete the dead strategy code. A silent second clamp would mask
   pipeline bugs (a signal reaching lowering unclamped under `-ct 1` is a
   staging-order bug we want to see, not absorb).

## Non-goals

- `-cir` / `signalIntCastPromote` (the sibling pass at `normalform.cpp:127`):
  same architecture, separate plan.
- Interval-analysis improvements (e.g. tightening recursive-index intervals
  so more accesses prove in-bounds): orthogonal; this plan only relocates the
  decision.
- The interpreter/cranelift/wasm backends consume the same FIR, so they are
  covered by G2 without backend-specific work; only goldens move.
