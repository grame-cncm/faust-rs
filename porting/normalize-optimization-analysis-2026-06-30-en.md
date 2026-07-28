# Optimization Analysis for `crates/normalize`

Date: 2026-06-30

## Short Conclusion

Yes, the normalization algorithms can be improved to produce potentially more
efficient signal graphs, but two goals should stay separate:

1. **C++ parity normal form**: preserve the current canonical form, which makes
   structural equality and differential testing reliable.
2. **Cost-oriented normal form**: add optional, measured, type-aware
   transformations to reduce sample-rate operations, delays, or FIR size.

The best short-term compromise is not a broad e-graph optimizer. The most
realistic gains are:

- replace the quadratic search for the best `Aterm` GCD with a factor index and
  an explicit cost model;
- add multi-term factorization driven by estimated benefit, not only by the
  maximum pairwise GCD;
- recognize selected univariate polynomial and bilinear forms;
- strengthen structural CSE around recursive subgraphs and shared normalized
  expressions;
- keep equality saturation as a bounded prototype, not the default path, until
  C++ parity and compilation cost are under control.

## Current Code State

The module has a clear decomposition:

- `simplify.rs`: memoized DFS traversal, local rewrites, constant folding,
  neutral/absorbing elements, then `normalize_add_term` for remaining `BinOp`
  expressions.
- `normalize.rs`: additive normalization and delay normalization coordination.
- `aterm.rs`: sum-of-`Mterm` representation, merge of terms with identical
  signatures, reconstruction by variability order.
- `mterm.rs`: multiplicative representation `coef * product(base^exp)`, with
  signed exponents and a numeric coefficient.
- `rec_merge.rs`: structural merge of isomorphic `SYMREC` groups.

Important observations:

- `simplify_with_cache` reuses one cache per pass and type context
  (`crates/normalize/src/simplify.rs`). This is already the right basis for
  avoiding repeated work across shared roots.
- The default path for binary operations not handled by local rules calls
  `normalize_add_term`, so many arithmetic expressions flow through
  `Aterm/Mterm`.
- `Aterm::greatest_divisor` scans every pair of terms and picks the GCD with
  maximum complexity.
- `normalize_add_term` loops: build an `Aterm`, find a GCD, factorize, rebuild,
  then repeat while the GCD has positive complexity.
- `Mterm::complexity` weights factors by variability:
  `Konst = 0`, `Block = 1`, `Samp = 2`, using weight `1 + order`. This already
  hints that normalization should prefer saving sample-rate work.

`porting/MEMOIZATION.md` already records that macOS profiling on expanded
RAD/FAD DSPs was dominated by `sig_map`, `Aterm::add_sig`,
`normalize_add_term`, `greatest_divisor`, and `mterm::gcd`. That confirms that
the first improvements should target factor search and reuse of intermediate
normal forms before adding expensive new rewrite systems.

## Limits of the Current Algorithm

### 1. Quadratic Search for the Best Factor

`Aterm::greatest_divisor` compares every pair of `Mterm`s. For `n` terms, the
search is `O(n^2 * gcd_cost)`, then `factorize` reconstructs a new sum and the
loop starts again. In expanded FAD/RAD expressions, `n` can grow quickly and the
same factor information is recomputed many times.

Proposed improvement:

- build an index `factor_key -> occurrences`, where `factor_key` encodes
  `(base SigId, exponent sign, minimum absolute exponent, variability order)`;
- compute an approximate benefit:
  `gain = (occurrences - 1) * factor_cost - introduction_cost`;
- choose the factor, or factor product, with the best gain;
- run exact GCD only on the candidate subset.

Expected effect:

- reduce compile time on large sums;
- make factorization more global than "best GCD of two terms";
- keep reconstruction through `Aterm/Mterm`, which keeps the implementation
  close to the C++ parity model.

Risk:

- a different factor choice can produce a structurally different form from C++,
  even if it is semantically equivalent.

Validation:

- focused `Aterm` unit tests with multi-term forms;
- `cargo run -p xtask -- golden-check`;
- `golden-check-cpp` on a targeted FAD/RAD corpus;
- measure `SigId` node count, `BinOp::Mul/Add` count, and
  `simplify_signals_fastlane` time.

### 2. Multi-Term Factorization, Not Only Pairwise GCD

The current factorization extracts a GCD chosen from one pair. It can then
iterate, but the order of choices can miss a globally better form or spend time
on unnecessary reconstructions.

Example:

```text
x*a + x*b + x*c + y*a + y*b + y*c
```

Possible form:

```text
(x + y) * (a + b + c)
```

The current path may extract `x` from one subset, then `a` from another, without
necessarily reaching the compact product-of-sums form.

Another example:

```text
a*b + a*c + d*b + d*c
```

Possible form:

```text
(a + d) * (b + c)
```

This is not a simple common GCD across all terms. It is a support-matrix
factorization, closer to symbolic factorization and algebraic CSE.

Proposed improvement:

- detect rectangular blocks in the `term x factor` support matrix;
- limit the first version to simple cases: two factor groups, compatible
  coefficients, no division, no negative `pow`;
- apply only when the estimated cost strictly decreases;
- keep it experimental until C++ parity is proven.

Important risk:

- for floating-point signals, reassociating
  `a*b + a*c + d*b + d*c` into `(a+d)*(b+c)` changes rounding, and an imperfect
  detector could introduce cross terms. This optimization must not be enabled
  in the parity path without strict numeric validation.

### 3. Polynomial Factorization and Horner Forms

`Mterm` already represents `x^n`, so it can recognize:

```text
c0 + c1*x + c2*x^2 + c3*x^3
```

and produce:

```text
c0 + x*(c1 + x*(c2 + x*c3))
```

Benefits:

- fewer multiplications in univariate polynomials;
- better FMA opportunities in some backends;
- useful for FAD/RAD if expansions produce many powers of one variable.

Limits:

- Horner form is sequential and can reduce ILP/SIMD opportunities;
- floating-point operation order changes;
- coefficients should ideally be `Konst` or `Block`, with the variable being
  more expensive (`Samp`), so the gain is clear.

Recommendation:

- prototype behind an option or in a FIR pass, not inside default
  `normalize_add_term`;
- apply only when the type map says the variable is sample-rate and the
  coefficients are less variable;
- compare `opt_level=0` and `opt_level=max`, as required by the project rules
  for optimization-sensitive execution paths.

### 4. Variability-Guided Reassociation

The code already reconstructs sums and products by variability order. This can
be made into a real cost model:

- prioritize factorization of `Samp` subexpressions;
- extract `Konst`/`Block` factors only when it reduces sample-rate work;
- avoid transformations that move a `Konst` computation into `Samp`;
- expose counters: operations by `Konst`, `Block`, `Samp`, delay count, table
  count, cast count.

This direction fits the current architecture: `Mterm::complexity` already
contains the base model. The next step is to make it explicit and usable by
`Aterm::greatest_divisor` or its replacement.

### 5. Structural CSE Around Normal Forms

`TreeArena` hash-conses nodes, so strictly identical forms become the same
`SigId`. Normalization already helps `x - x -> 0`, and
`merge_isomorphic_symrec_groups` extends this idea to recursive groups.

Possible improvements:

- add a post-simplification usage-count pass to identify expensive subgraphs
  used more than once;
- annotate or expose those subgraphs to FIR so backends can emit temporaries in
  the right place;
- avoid forcing algebraic factorization when FIR-level CSE already produces
  better code with lower register pressure.

This must be coordinated with the existing FIR/CSE plans. Expression
normalization and temporary introduction are different problems. A more
factorized `normalize` expression is not automatically faster if it increases
sequential dependencies or register pressure.

### 6. Delay Normalization

`clock_normalize_delay_term` already handles:

- `s @ 0`;
- `0 @ d`;
- multiplicative factor extraction when the factor is less variable than
  sample-rate;
- division by a less-variable factor;
- nested delay folding when the index is less variable.

Prudent extensions:

- detect `(x + k) @ d` or `(x - k) @ d` with less-variable `k`, only if the
  temporal semantics of `k` match C++ expectations;
- detect `select2(c, a, b) @ d` with a less-variable selector, but only if this
  does not duplicate sample-rate delay lines;
- compare memory cost and compute cost: distributing a delay can reduce one
  addition but increase buffer count.

Recommendation:

- do not add additive delay distribution without explicit C++ analysis. Delays
  are parity-sensitive.

## External References Consulted

- LLVM documents passes close to these goals: `gvn` eliminates redundant
  instructions, `reassociate` reorders expressions to help constant
  propagation, GCSE, LICM, and PRE:
  <https://llvm.org/docs/Passes.html>
- MLIR exposes a general `-cse` pass and a `-canonicalize` pass that applies
  rewrite patterns to a bounded fixpoint:
  <https://mlir.llvm.org/docs/Passes/>
- The `egg` project presents e-graphs as a compact representation of many
  equivalent expressions, usable for optimization and synthesis:
  <https://egraphs-good.github.io/>
- Willsey et al., "egg: Fast and Extensible Equality Saturation", POPL 2021:
  <https://arxiv.org/abs/2004.03082>
- Kuipers, Ueda, Vermaseren, "Code Optimization in FORM", describes
  multivariate Horner schemes combined with CSE for symbolic code optimization:
  <https://arxiv.org/abs/1310.7007>
- General CSE/value-numbering literature reinforces the key point: sharing an
  expression is profitable only when temporary cost and register pressure stay
  below recomputation cost.

## Estimated CPU Cost Gains in Generated Code

The CPU gain of the generated DSP code is expected to be more modest than the
compile-time gain, and highly motif-dependent. Normalization mainly changes the
shape of arithmetic expressions before FIR/backend lowering. It does not, by
itself, guarantee better machine code if the FIR/backend CSE, LLVM, or the C
compiler already recover the same sharing.

Estimated ranges:

| Optimization class | Expected generated-code CPU gain | Where it applies | Main caveat |
|---|---:|---|---|
| Exact same-signature merge (`x + x -> 2*x`, coefficient folding) | 0-5% locally, usually <1% globally | Small algebraic redundancies already covered today | Mostly already implemented |
| Better common-factor extraction (`a*x + b*x -> x*(a+b)`) | 1-10% on affected arithmetic kernels | FAD/RAD-expanded formulas, repeated sample-rate factors | Global DSP gain depends on how much time is spent in the affected kernel |
| Variability-guided factorization (`Samp` work moved behind `Konst/Block` factors) | 2-15% on affected kernels | Expressions mixing UI/block constants with sample-rate terms | Must not move computations to a more variable rate |
| Univariate Horner forms | 5-30% on polynomial-heavy kernels | Polynomial approximations, expanded derivative expressions | Can reduce ILP/SIMD and changes floating-point association |
| Bilinear/product-of-sums factorization | 5-25% on strictly matched dense algebraic blocks | Forms like `a*b + a*c + d*b + d*c` | High semantic risk if detection is too broad |
| Delay distribution/factorization | 0-20% but can be negative | Delay expressions with less-variable factors | May increase delay buffers or change temporal semantics |
| FIR/backend CSE of shared normalized subgraphs | 2-20% on graphs with repeated expensive subexpressions | Multi-output DSPs, FAD/RAD, repeated filters/partials | Register pressure can erase the gain |

Practical global expectations:

- On ordinary DSPs dominated by filters, oscillators, tables, and delays,
  normalization-only CPU gains are likely **0-3%**.
- On algebraically expanded FAD/RAD DSPs where generated code contains many
  repeated sample-rate arithmetic subexpressions, realistic gains are more like
  **5-15%** if cost-guided factorization and FIR CSE cooperate.
- On synthetic polynomial or dense symbolic kernels, local wins can reach
  **20-30%**, but those numbers should not be extrapolated to full programs.
- Some transformations can be CPU-negative because they increase dependency
  depth, register pressure, or delay-buffer count. Every optimization needs an
  operation-count and runtime benchmark gate.

Recommended measurement model:

1. Count operations before/after normalization by variability:
   `Konst`, `Block`, `Samp`.
2. Weight sample-rate operations by per-sample execution and block/init
   operations by amortized cost.
3. Track memory-affecting nodes separately: delays, tables, recursive state,
   and temporary count.
4. Compare generated code runtime on representative kernels with
   `opt_level=0` and `opt_level=max`.
5. Report both local kernel delta and whole-DSP delta, because a 20% gain in a
   kernel that is 25% of runtime is only a 5% whole-DSP gain.

This means the first implementation milestone should not promise CPU speedups.
It should promise a cost report. Once the report identifies repeated
sample-rate arithmetic as a real runtime cost, the factorization options can be
enabled experimentally and benchmarked.

## Proposed Plan

### Phase A: Instrumentation Without Semantic Changes

Add optional internal counters:

- number of terms per `Aterm`;
- number of calls to `gcd`;
- number of `normalize_add_term` iterations;
- before/after size in nodes and operations by variability;
- per-pass time for `simplify_signals_fastlane`.

Pass criteria:

- no golden output change;
- measurements available on a short corpus: `rad_fxlms1.dsp`, recursive FAD
  cases, and the standard golden corpus.

### Phase B: Factor Index to Accelerate GCD Search

Replace or short-circuit `greatest_divisor` with a candidate index:

- preserve the same result as the pairwise algorithm first;
- reduce the number of exact `gcd` calls;
- add structural tests proving that the chosen factor stays identical on
  existing cases.

Pass criteria:

- identical Rust golden output;
- no `golden-check-cpp` regression;
- measured normalization-time reduction on FAD/RAD cases.

### Phase C: Most Profitable Factor, Behind an Option

Allow a different factor choice when the cost model predicts a strict decrease.

Pass criteria:

- disabled by default;
- numeric differential tests;
- before/after size and cost report;
- `JOURNAL.md` documentation if the option becomes public.

### Phase D: Specialized Prototypes

Separate prototypes:

- univariate Horner;
- simple bilinear factorization;
- CSE of normalized subgraphs toward FIR;
- bounded local e-graph over pure arithmetic expressions.

Pass criteria:

- each prototype declares its validity domain;
- no change to the parity path without explicit approval;
- benchmarks and structural non-regression tests.

## Final Recommendation

The priority should be:

1. Instrument and measure `Aterm::greatest_divisor`.
2. Add a factor index that first preserves the same choice as C++.
3. Then introduce cost-guided factorization behind an option.
4. Evaluate Horner and bilinear factorization only on strictly detected motifs.
5. Keep equality saturation as a research or validation tool, not as the
   default normalizer.

This path improves compilation performance and opens the door to more efficient
generated code without breaking the central rule of the port: semantic parity
with the C++ compiler remains the default objective.
