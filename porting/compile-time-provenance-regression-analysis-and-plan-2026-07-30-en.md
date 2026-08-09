# Compile-time regression of the diagnostics-provenance arc — analysis and correction plan (2026-07-30)

> **Status: executed 2026-07-30.** Corpus cost went from 4.53x to **1.10x** of the
> pre-arc reference, and the overhead is now flat across every program-size
> bucket (1.06x–1.17x) instead of reaching 9.55x on the largest — the complexity
> defect is gone, only a uniform constant remains. `dx7_alg5` compiles in 1.67 s
> against 1.97 s before the arc. All four steps are resolved: 1 and 2
> implemented, 3 and 4 closed as not-to-be-implemented with the measurements
> that decided it. See "Execution outcome" at the end, which also records where
> this plan was wrong.

## Summary

The compiler-diagnostics-v2 arc (`8a909843..41e4373d`, 29 commits, 98 files,
+10 464 / −1 263) multiplied front-end compilation cost by **4.5x over the
measured corpus**, and by **9x to 17x on the largest programs**. `dx7_alg5`,
reported in `faust-rs#15`, is the most visible symptom, not the whole defect:
eleven other corpus DSPs regressed between 2x and 17x.

`faust-rs#15` ("Bound provenance evidence") is correct and recovers roughly half
of the loss. It is **necessary but not sufficient**: with its caps applied, the
corpus is still 1.74x slower than before the arc, and every large program stays
above 2x. The residue has two further causes, neither addressed by that PR.

The regressions are size-dependent, not fixed overheads, so they get worse as
user programs get larger.

## Measurement method

Four release binaries were built and run over 352 DSP files
(`tests/impulse-tests/dsp`, `tests/corpus`, `examples/rust`) with
`--check --timeout 25`:

1. `98496b4a` — parent of the arc's first commit, the pre-regression reference.
2. `41e4373d` — `main-dev` at the time of writing.
3. `41e4373d` + the two caps from `faust-rs#15`, applied by hand.
4. the same, plus the per-propagation provenance walk disabled, to isolate its
   share.

| Configuration | Corpus total | vs reference |
| --- | --- | --- |
| `98496b4a` (before the arc) | 26.1 s | 1.00x |
| `41e4373d` (`main-dev`) | 118.0 s | **4.53x** |
| `41e4373d` + `#15` caps | 45.3 s | **1.74x** |
| + per-propagation walk disabled | 31.0 s | **1.19x** |

Per file, `--check` wall clock in milliseconds:

| DSP | before | `main-dev` | + `#15` | + walk disabled |
| --- | --- | --- | --- | --- |
| `dx7_alg5` | 1 972 | 27 047 (timeout) | 4 290 | 2 539 |
| `reverb_designer` | 7 146 | 18 286 | 16 165 | 9 066 |
| `virtual_analog_oscillators` | 723 | 12 148 (**16.8x**) | 1 544 | 839 |
| `spectral_level` | 902 | 10 899 | 2 276 | 1 205 |
| `vcf_wah_pedals` | 760 | 7 905 | 1 508 | 888 |
| `parametric_eq` | 724 | 6 725 | 1 698 | 864 |
| `bells` | 364 | 5 220 (**14.3x**) | 446 | 395 |

The overhead grows with program size, which is the signature of a complexity
defect rather than a constant cost:

| Reference cost bucket | n | `main-dev` | + `#15` |
| --- | --- | --- | --- |
| < 25 ms | 264 | 1.79x | 1.06x |
| 100–250 ms | 23 | 1.27x | 1.10x |
| 250–500 ms | 7 | 3.66x | 1.14x |
| 500–1000 ms | 7 | **9.55x** | **2.19x** |
| > 1 s | 4 | 4.57x | 2.07x |

## Cause 1 — unbounded provenance unions (fixed by `faust-rs#15`)

`SignalOrigins::inherit_forest` runs after each of the eight preparation passes
(`crates/transform/src/signal_prepare/mod.rs`), and `FirOrigins::derive_reachable`
unions descendant derivations into every reachable FIR parent. Both used
linear-scan dedup over lists whose length grew with program size, making
preparation super-quadratic.

Capping both tables at 8 entries makes the walks O(N) and takes the corpus from
4.53x to 1.74x. This cause is understood and handled; the remainder of this
document concerns what `#15` does not touch.

## Cause 2 — per-propagation provenance forest walk (dominant residue)

`crates/propagate/src/engine.rs` calls `record_derived_forest` at the end of
**every box propagation**. Each call performs a full DFS over the signal forest
reachable from that box's outputs, allocating a fresh
`std::collections::HashSet` — with the default SipHash hasher, while the crate
uses `AHashMap` elsewhere. Total cost is O(boxes x reachable subforest).

`sample` profile of `reverb_designer`, **with the `#15` caps already applied**:

- `record_derived_forest`: **6 211 of 9 181 samples (68 %)** inclusive;
- heaviest self time: `HashMap<TreeId, _>::insert` under `RandomState` (847),
  `reserve_rehash` (745), then the allocator traffic those tables generate
  (`tiny_free_list_add_ptr` 724, `tiny_free_no_lock` 700, …).

Phase breakdown with the caps applied, `spectral_level`:

| Phase | before the arc | + `#15` caps |
| --- | --- | --- |
| `evaluation` | 417 ms | 1 153 ms (2.8x) |
| `propagation` | 25.7 ms | 613.5 ms (**23.9x**) |
| remainder (prepare + FIR verify) | ~457 ms | ~518 ms |

**Most of this work is discarded.** `propagate_typed`
(`crates/propagate/src/api.rs`) ends with `.map(|output| output.signals)`: the
whole `SignalOrigins` table is built and dropped. `eval` calls `propagate_typed`
for constant folding (`crates/eval/src/simplify.rs`, the C++ `boxPropagateSig`
path), so every constant fold pays for a full provenance forest walk whose
result no caller can observe. This single fact explains both the `propagation`
and the `evaluation` regressions.

Disabling this one call takes the corpus from 1.74x to **1.19x** and
`reverb_designer` from 16.2 s to 9.1 s.

## Cause 3 — parser and eval provenance recording (diffuse residue)

With the caps applied and the per-propagation walk disabled, `reverb_designer`'s
`evaluation` phase is still 8.62 s against 7.28 s before the arc (**+18 %**), with
no dominant symbol in the profile — allocator traffic spread across the phase.
Identified by reading, **not yet isolated by measurement**:

- `BoxProvenance::by_node` (`crates/parser/src/context.rs`) is a
  `HashMap<TreeId, Vec<BoxOriginId>>` whose `record` pushes **without dedup and
  without a cap**. This is precisely the failure family `#15` just fixed on the
  signal side: on hash-consed Box nodes (`_`, literals, shared subexpressions) a
  single `TreeId` accumulates one entry per syntactic occurrence.
- `import_box_provenance` copies the entire origin table of each imported file,
  cloning a `SourceLocation` whose `file` field is a `Box<str>` — one string
  allocation per copied origin, per import.
- `SourceMapBuilder::add` (`crates/diagnostics/src/source.rs`) scans linearly and
  compares **whole source texts**. Minor at one entry per file, but O(n²) in
  bytes by construction.

## Why CI did not catch any of this

`compile-budget-check` existed but could not see it. Its ceilings were
absolute wall clock and therefore had to be loose enough for the slowest runner:
`reverb_designer` went from 7.1 s to 18.3 s **against a `scalar_max_ms` of
45 000** and never turned the job red. It also measured only the full
file-to-C++ path in scalar and vector mode, on five cases, none of which
isolates the front end where all three causes live.

## Correction plan

Ordered by measured benefit per unit of risk. Each step ends with a baseline
regeneration so the gate ratchets downward and cannot silently drift back up.

### Step 1 — do not build provenance nobody will read

Thread an explicit switch through `PropagateContext`; `propagate_typed`, which
discards the table, sets it off. Removes the entire eval-side constant-folding
cost and part of the propagation cost.

- Expected: corpus 1.74x → ~1.3x, `dx7_alg5` from 4.3 s to ~2.6 s.
- Risk: low. The affected callers provably discard the table today.
- Gate: re-enable `dx7_alg5` in the front-end basket; regenerate the baseline.

### Step 2 — take the forest walk out of the per-box loop

`record_derived_forest` should run once at the end of propagation rather than
once per box, or reuse a `visited` set across calls. Use `AHashSet`, not
`std::collections::HashSet`, matching the rest of the crate.

- Expected: removes the remaining `propagation` x20.
- Risk: medium. Changes which node receives which origin when a signal is
  reachable from several boxes; needs a provenance-quality check on the negative
  corpus (`cli_diagnostics_channel`, `diagnostic_errors`,
  `machine_applicable_fixes`) before and after.

### Step 3 — bound and dedup `BoxProvenance`

Apply the contract `#15` established for signals to the parser side: cap
`by_node` entries and dedup on insertion. Make `import_box_provenance` share
locations (`Arc<str>` for the file field) instead of deep-cloning per origin.

- Expected: most of the residual +18 % in `evaluation`.
- Risk: low, but changes which occurrence a diagnostic selects when a node has
  many; must be validated on the negative corpus.

### Step 4 — decide the long-term shape

Steps 1–3 make the eager tables affordable; they do not remove the fact that
every successful compilation pays to build evidence only a failing one reads.
The alternative discussed on `faust-rs#15` — keeping only direct records plus a
rewrite-edge log, and deriving provenance on demand at diagnostic time — costs
nothing on the success path and needs no cap at all. It requires keeping
intermediate arenas or their remap tables alive, which is a real memory
trade-off and a design decision rather than a patch.

Do not treat `MAX_ORIGINS_PER_SIGNAL = 8` as permanent until this is decided:
tests written against truncated output would turn the constant into a de facto
specification.

## Non-regression gate

`compile-budget-check` is extended rather than duplicated
(`crates/xtask/src/vector_compile_budget.rs`, baseline schema 2):

- a **front-end basket** measures the `--check` path only, over
  `tests/impulse-tests/dsp` cases that actually regressed;
- every measurement is normalized against a calibration DSP
  (`tests/impulse-tests/dsp/karplus.dsp`, unaffected by this arc: 0.997x)
  measured in the same process, so machine speed cancels and the tolerance can
  be tight (**30 %**) instead of an order of magnitude;
- the calibration is measured first and identically in enforcing and `--update`
  mode; measuring it after the codegen basket in one mode only moved it by 44 %
  and shifted every ratio with it;
- `--update` rewrites the baseline explicitly, never automatically.

Observed run-to-run spread on the recorded basket is ±8 % worst case, against a
30 % tolerance — tight enough to reject the 2.5x residue that `#15` alone leaves,
as asserted by
`frontend_tolerance_rejects_the_2026_07_30_provenance_regression`.

`dx7_alg5` is present but `enabled: false`: it exceeds the 120 s compilation
timeout on `main-dev`. Step 1 re-enables it.

## Execution outcome (2026-07-30)

| Configuration | Corpus total | vs reference |
| --- | --- | --- |
| `98496b4a` (before the arc) | 26.1 s | 1.00x |
| `41e4373d` (`main-dev`, start of this work) | 118.0 s | 4.53x |
| + step 1 | 100.4 s | 3.86x |
| + cause 1 caps | — | — |
| + step 2 | **28.7 s** | **1.10x** |

Per file, `--check` wall clock in milliseconds:

| DSP | before the arc | start | final | final vs before |
| --- | --- | --- | --- | --- |
| `dx7_alg5` | 1 972 | >120 000 (timeout) | **1 674** | **0.85x** |
| `reverb_designer` | 7 146 | 18 286 | 7 576 | 1.06x |
| `spectral_level` | 902 | 10 899 | 1 038 | 1.15x |
| `virtual_analog_oscillators` | 723 | 12 148 | 811 | 1.12x |
| `vcf_wah_pedals` | 760 | 7 905 | 860 | 1.13x |
| `bells` | 364 | 5 220 | 421 | 1.16x |
| `parametric_eq` | 724 | 6 725 | 901 | 1.24x |

The size-dependence is what mattered most, and it is gone:

| Reference cost bucket | n | at start | final |
| --- | --- | --- | --- |
| < 25 ms | 264 | 1.79x | 1.11x |
| 250–500 ms | 7 | 3.66x | 1.12x |
| 500–1000 ms | 7 | **9.55x** | **1.17x** |
| > 1 s | 4 | 4.57x | 1.06x |

### Where this plan was wrong

**The step order was wrong.** Step 2 was implemented and measured first and
produced *no improvement at all*. Profiling then put 4 056 samples of self time
in `SignalOrigins::record` — the unbounded-list dedup of cause 1, which this
document had assigned to `faust-rs#15` and assumed present. It is not on
`main-dev`. Step 2 was set aside, the caps landed as their own commit, and only
then did the pruning show its value (−15 % to −53 %). A plan step that measures
as worthless may be blocked by another step rather than useless; the profile,
not the plan, decides which.

**Step 3 was not implemented, because its premise did not survive
measurement.** Cause 3 was explicitly recorded above as "identified by reading,
not yet isolated by measurement", and that caution was warranted:

- `BoxProvenance`, `import_box_provenance`, `SourceMap` and `SourceLocation`
  appear **nowhere** in the profile of the final binary — zero samples;
- a direct probe of the table on the two largest programs gives
  `dx7_alg5`: 80 origins over 41 nodes, **max 3 per node**;
  `reverb_designer`: 15 origins over 6 nodes, max 3. The proposed cap of 8 would
  never fire.

The table is populated at definition and use sites, not per AST node, so the
"unbounded accumulation" is a structural property with no measured cost. Adding
a cap would have changed which occurrence a diagnostic selects — a real risk —
in exchange for nothing. The concern is left recorded here rather than acted on.

The residual 1.10x is **not attributed**. The profile shows diffuse allocator
traffic plus `ui::split_label_metadata` and `boxes::match_box`, neither of which
this arc introduced. Claiming a cause without evidence is how cause 3 got into
this document in the first place.

## Step 4 — decision: do not implement lazy derivation

Step 4 proposed replacing the eager capped tables with direct records plus a
rewrite-edge log, deriving provenance on demand when a diagnostic is built. The
argument was that a successful compilation should not pay to build evidence
only a failing one reads.

That argument is sound in principle and does not survive measurement.

**Upper bound on the benefit.** Disabling *all* provenance recording on the
compiler path — `SignalOrigins` and `FirOrigins` both — is strictly better than
any lazy scheme could be, since a lazy scheme still records direct origins and
still maintains an edge log. Measured against the current state, `--check` wall
clock, best of three:

| DSP | current | no provenance at all | gain |
| --- | --- | --- | --- |
| `dx7_alg5` | 1 674 ms | 1 456 ms | −13.0 % |
| `parametric_eq` | 901 ms | 807 ms | −10.4 % |
| `spectral_level` | 1 038 ms | 997 ms | −4.0 % |
| `bells` | 421 ms | 406 ms | −3.7 % |
| `virtual_analog_oscillators` | 811 ms | 795 ms | −2.0 % |
| `vcf_wah_pedals` | 860 ms | 856 ms | −0.5 % |
| `reverb_designer` | 7 576 ms | 7 931 ms | +4.7 % |
| **total** | **13 281 ms** | **13 247 ms** | **−0.3 %** |

The total is noise. `reverb_designer` measuring *slower* without provenance is
the clearest statement of the signal-to-noise ratio at this scale.

**Memory, the other half of the trade-off.** Peak RSS, same comparison:

| DSP | current | no provenance at all |
| --- | --- | --- |
| `dx7_alg5` | 681.1 MB | 682.1 MB (+0.1 %) |
| `reverb_designer` | 321.9 MB | 318.9 MB (−0.9 %) |

Provenance is under 1 % of peak memory. Lazy derivation would have to keep
intermediate arenas or their remap tables alive, so on the dimension where it
costs, it costs; on the dimension where it saves, there is nothing to save.

**Decision: closed, not implemented.** Buying at most 0.3 % of compile time and
nothing in memory does not justify a rewrite-edge log, extended arena
lifetimes, and a redesign of how diagnostics reach source occurrences — each of
which risks the diagnostic quality the whole arc was built to deliver.

**Consequence: the caps are now the design, not a stopgap.** The earlier warning
against treating `MAX_ORIGINS_PER_SIGNAL = 8` as permanent is therefore
withdrawn, and the guarantee it was standing in for has been made explicit
instead. `SignalOrigins::remap` now iterates its clone mapping in ascending
`SigId` order rather than in `HashMap` order: the mapping is expected to be
injective, but nothing in the type enforces it, and if two sources ever shared a
destination the cap would let hash order decide which candidates a diagnostic
can name. `remap_is_independent_of_node_map_hash_order` builds a deliberately
many-to-one mapping that overflows the cap and fails without the sort.

Reopen this decision only if a future change makes provenance a materially
larger share of compile time or memory — the numbers above, not the argument,
are what would have to move.

### Still open

The residual ~1.10x is **not provenance**. With provenance entirely removed,
every measured case still sits at 1.10x–1.13x of the pre-arc reference
(`dx7_alg5` at 0.74x). Something else in the arc, or outside it, accounts for
that flat constant; the profile points at diffuse allocator traffic plus
`ui::split_label_metadata` and `boxes::match_box`, neither introduced here.

That is a separate investigation with its own question — a uniform ~10 %, not a
complexity defect — and it is deliberately not folded into this plan. The
front-end budget gate now holds the line at the current numbers, so it can be
picked up on its own merits rather than under time pressure.
