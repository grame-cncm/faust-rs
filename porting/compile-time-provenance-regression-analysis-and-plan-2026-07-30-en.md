# Compile-time regression of the diagnostics-provenance arc — analysis and correction plan (2026-07-30)

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

`vector-compile-budget-check` existed but could not see it. Its ceilings were
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

`vector-compile-budget-check` is extended rather than duplicated
(`crates/xtask/src/vector_compile_budget.rs`, baseline schema 2):

- a **front-end basket** measures the `--check` path only, over
  `tests/impulse-tests/dsp` cases that actually regressed;
- every measurement is normalized against a calibration DSP
  (`tests/impulse-tests/dsp/karplus.dsp`, unaffected by this arc: 0.997x)
  measured in the same process, so machine speed cancels and the tolerance can
  be tight (**25 %**) instead of an order of magnitude;
- the calibration is measured first and identically in enforcing and `--update`
  mode; measuring it after the codegen basket in one mode only moved it by 44 %
  and shifted every ratio with it;
- `--update` rewrites the baseline explicitly, never automatically.

Observed run-to-run spread on the recorded basket is ±8 % worst case, against a
25 % tolerance — tight enough to reject the 2.5x residue that `#15` alone leaves,
as asserted by
`frontend_tolerance_rejects_the_2026_07_30_provenance_regression`.

`dx7_alg5` is present but `enabled: false`: it exceeds the 120 s compilation
timeout on `main-dev`. Step 1 re-enables it.
