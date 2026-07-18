# Scalar Compilation History and Cost Audit

Date: 2026-07-16

Scope: Phase 6 of
[`vector-corpus-coverage-analysis-and-recovery-plan-2026-07-15-en.md`](vector-corpus-coverage-analysis-and-recovery-plan-2026-07-15-en.md).

## Method

The two historical baselines were built in isolated worktrees at the commits
immediately before the relevant feature activations:

| Label | Commit | Relationship |
| --- | --- | --- |
| `pre-ss` | `8bbb170eddc21310cb04aa924b38f2c484926e3b` | parent of `e7edbf08` (`Finalize authoritative scalar scheduling`) |
| `pre-vector` | `e4181cc641dba5143107bbac7ee85c47a9545000` | parent of `07223abb` (vector-mode option plumbing) |
| current | `9cfdc38e` plus this Phase 6 worktree change | implementation under audit |

All binaries used `cargo build --release -p compiler`. Measurements used the
same current corpus input, frontend settings, C++ output, FIR lane, standard
library include path, and double precision:

```sh
/usr/bin/time -p "$FAUST_RS" -time -lang cpp -double \
  -I /usr/local/share/faust \
  tests/impulse-tests/dsp/reverb_designer.dsp > /tmp/reverb.cpp
```

The host was macOS 12.6.0 on Apple M1 (`arm64`), with Rust
`1.96.0 (ac68faa20 2026-05-25)`. Each release binary was measured twice after
its build; the figures below are arithmetic means of the compiler's internal
timings. Wall-clock output was retained as a sanity check, rather than being
used to attribute individual stages.

## Historical result

`reverb_designer.dsp` was the slow representative. Its dominant evaluation
cost predates both `-ss` and vector-mode activation:

| Binary | Total (ms) | Evaluation (ms) | Signal→FIR (ms) |
| --- | ---: | ---: | ---: |
| pre-vector | 7247.0 | 7082.5 | 111.2 |
| pre-ss | 7238.5 | 7058.1 | 128.3 |
| current | 7354.8 | 7169.0 | 136.3 |

The current total is within 1.5% of the historical releases, while evaluation
alone remains approximately 97.5% of its total. This classifies the expensive
scalar corpus behaviour as historical evaluator cost, not a regression caused
by scheduling strategies or checked vectorization.

`cubic_distortion.dsp` provides a shorter independent check: the current
release measured 602.8 ms total, 442.7 ms evaluation, and 140.3 ms
signal→FIR. The pre-vector measurement was approximately 560.5 ms total,
422.7 ms evaluation, and 117.2 ms signal→FIR. Its small remaining FIR delta
is verified preparation/normalization work, not vector certification.

## Attributed scalar regression and correction

Initial current-release stage timing on `reverb_designer.dsp` showed that the
post-`-ss` FIR cost was not vector code generation. Scalar mode was building
the complete vector occurrence/condition analysis merely to obtain direct
effect facts for effect-conflict orientation. It also compared every pair of
stateful nodes, even when their resources differed.

The correction is generic:

1. `ScalarSchedulingEffects` derives only direct effects from the verified
   prepared forest. It does not construct vector occurrence records, clock
   facts, or DNF execution conditions.
2. Effect orientation groups nodes by state/table/UI/output resource and adds
   the transitive-reduced baseline constraints. Foreign impure/unknown effects
   remain barriers against every direct-effect node.
3. `-time` now exposes `fir-plan`, `fir-prepare-normalize`, `fir-hgraph`,
   `fir-scalar-effects`, `fir-effect-orientation`, `fir-scheduling`,
   `fir-clock-analysis`, `fir-vector-certification` (vector mode only), and
   `fir-lowering` below the existing parse, evaluation, propagation, FIR
   verification, and backend-emission stages.

After the correction, `reverb_designer.dsp` reports 2.1 ms for scalar effects,
7.8 ms for effect orientation, 3.8 ms for scheduling, and 136.1--136.5 ms for
the entire signal→FIR phase. The uncorrected diagnostic run had spent about
140 ms in scalar effect analysis and 157 ms in effect orientation alone.

The remaining roughly 20--25 ms over the old FIR timings is measured verified
preparation/normalization (`fir-prepare-normalize`, about 104--122 ms on these
cases). It is not vector-only work, is required by the active FIR lowering
contract, and is explicitly retained rather than hidden in the aggregate.

## Safety evidence

- `scalar_mode_does_not_run_vector_certification` proves scalar FIR results
  retain `NotRequested`, scalar effective mode, and no vector fallback detail
  across all four scheduling strategies.
- `scalar_effect_analysis_preserves_vector_direct_effect_facts` compares the
  new scalar facts to full vector analysis for every prepared signal in a
  stateful fixture.
- The hierarchical-graph test retains strategy-independent ordering of
  unknown foreign barriers.
- The C++ impulse oracle compared 15,000 scalar samples for both
  `reverb_designer.dsp` and `cubic_distortion.dsp` under `-ss 0` with no
  differences.

No corpus-specific scheduling or vector exceptions were introduced.
