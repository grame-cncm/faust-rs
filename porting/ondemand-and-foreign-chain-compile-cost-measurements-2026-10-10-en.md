# `ondemand` and foreign-chain compilation cost: measurements

Date: 2026-10-10

Scope: W7 of the
[2026-10-08 plan](foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md#w7--measure-and-correct-foreign-chain--clock-domain-compilation-cost).
The original report gave about 40 s for a recursive foreign "write chain" at
`N = 1024`, and about 180 s once wrapped in `ondemand`, without the program.
Its author later isolated a second observation: `ondemand(si.bus(N))` clocked
by `il.frame_clock(N)`, an identity, costs O(N²) to compile (0.53 s at 256,
13.7 s at 2048, 56.1 s at 4096, in their build), while `il.serialize_in(N)`
alone is nearly linear.

## Setup

- Host: Apple M5, 10 cores, 32 GiB, macOS 26.7.1. Load average 4 to 5 during
  the runs (an interactive machine, not an idle one); each figure is the
  median of three runs, whose spread stayed under 3%.
- faust-rs: release build of `ffbd4bbe` (rustc 1.99.0), then of the fixes below;
  `--timeout 0 -lang cpp -o /dev/null`, default depth limits.
- C++: the pinned reference build, `faust` 2.84.3
  (`master-dev-ocpp-od-fir-2-FIR19`, `8eebea429`), `-lang cpp -o /dev/null`.
- Code generation only; the generated C++ is not compiled. Peak memory is the
  maximum resident set size from `/usr/bin/time -l`.
- The programs are self-contained (no library): `si.bus(N)` is
  `par(i, N, _)`, the frame clock `((+(1) ~ _) % N) == 0`, and the foreign
  function `ffunction(float w7f|w7d|w7l(float), "w7.h", "")`, side-effect
  free. The generator and the measurement script are in the session
  scratchpad, not in the repository.

## 1. `ondemand` around an identity

`process = (clk(N), par(i, N, _)) : ondemand(par(i, N, _));`, against the same
bus and clock without `ondemand`, and `ondemand` with the constant clock `1`.

| N | bus, Rust | bus, C++ | `ondemand`, Rust before | `ondemand`, Rust after | `ondemand`, C++ | constant clock, Rust |
|---:|---:|---:|---:|---:|---:|---:|
| 256 | 0.03 s | 0.01 s | 0.03 s | | 0.02 s | 0.01 s |
| 1024 | 0.04 s | 0.07 s | 0.28 s | | 0.11 s | 0.04 s |
| 2048 | 0.10 s | 0.21 s | 1.01 s | 0.20 s | 0.36 s | 0.09 s |
| 4096 | 0.28 s | 0.75 s | 3.95 s | 0.52 s | 1.25 s | 0.28 s |
| 8192 | 0.80 s | | 14 s | 1.81 s | | |

The constant clock folds the block away. With a real clock, the Rust cost was
quadratic, about ×3.9 per doubling, and 3 times the C++ one at 4096.
`--compilation-time` placed it in `fir-lowering`: 0.88 s at 2048 and 3.47 s at
4096. Sampling the release binary at 8192 put
`classify_reverse_time_outputs` on top. It asks, for each output, whether a
reverse-time gradient projection is below it, with a fresh visited set per
output. The outputs of an `ondemand` block share its clocked group, which
holds all its inputs, so each output walked the whole block: N × N.

Fix: the verdict depends on the node alone (hash-consed trees cannot contain
themselves), so one memo serves every output. `fir-lowering` falls to 0.017 s
at 2048 and 0.035 s at 4096; the total is 0.52 s at 4096, under the C++ 1.25 s.
`golden-check` is unchanged.

What remained quadratic was propagation, and it was not specific to
`ondemand`: the bus without `ondemand` went from 0.80 s at 8192 to 3.10 s at
16384 in propagation. `par(i, N, _)` is a chain of N `Par` nodes, and each
level went through `propagate_in_slot_env`: it interned its input bus, N − k
wide, as a memo key (`result_memo::BusInterner::key`), stored its outputs,
recorded their origins and copied them into its parent's vector. `ondemand`
propagates its body once more, which doubled it. The C++ reference is also
quadratic on these programs.

Second fix: `Par` is associative, so the `Par` arm of `propagate_inner`
collects the leaves of the nested chain (an explicit stack, either nesting),
propagates each on its slice and concatenates once. The leaves and the
outermost `Par` keep their memo entries and origins; the inner `Par` nodes,
which only concatenate, have none.

| N | bus, propagation | bus, total | `ondemand`, total | `ondemand`, C++ |
|---:|---:|---:|---:|---:|
| 4096 | | | 0.16 s | 1.25 s |
| 8192 | 0.80 s → 0.007 s | 0.15 s | 0.34 s | |
| 16384 | 3.10 s → 0.006 s | 0.25 s | 0.74 s | |

At 16384 the rest is in `signal-fir` (0.66 s, of which `fir-prepare-normalize`
0.28 s and `fir-lowering` 0.16 s), about linear (×2.2 from 8192).
`golden-check` is unchanged.

## 2. Recursive foreign chains

`chain(n) = chain(n-1) : stage`, `process = chain(N)`.

| N = 2048 | Rust before | Rust after | C++ |
|---|---:|---:|---:|
| `stage = w` (no state) | 0.18 s, 39 MB | | 0.03 s, 31 MB |
| `stage = + ~ w` | 23.7 s, 911 MB | 12.2 s, 869 MB | 55.2 s, 7.6 GB |
| the same in `ondemand`, button clock | 31.2 s | 14.4 s | 61.5 s |
| the same, constant clock | 24.0 s | 12.2 s | 53.3 s |
| control `stage = + ~ sin` | 14.1 s | 2.6 s | 51.1 s |
| control in `ondemand` | 16.9 s, 879 MB | 3.8 s, 840 MB | 50.8 s, 8.0 GB |

Every recursive variant was quadratic in both compilers, ×4 to ×5 per doubling
(`+ ~ w`: Rust 1.08 s at 512, 4.83 s at 1024, 23.7 s at 2048; C++ 3.41 s,
13.5 s, 55.2 s). The foreign call is not the cause (the `sin` control has the
same shape), nor is `ondemand` (it adds 20 to 30% in Rust, 10% in C++). The
`ondemand` fix of section 1 does not change these figures (`sin_rec_od` at
1024: 6.1 s before, 4.2 s after).

`--compilation-time` put the `sin` control's cost in `fir-prepare-normalize`
(4.0 s of 4.2 s at 1024), sampling in `sigtype::TypeAnnotator`: the fixpoint
of the recursive groups' types, which the preparation runs six times. As C++
`typeAnnotation` does, it iterated `updateRecTypes` over every group, each
reading the previous iteration's approximations, until none changes. In a
chain, group k reads group k − 1, so a change crosses one group per
iteration: N + 1 iterations over N groups.

Fix (`solve_recursive_groups`): the same iteration, recomputing only the
groups one of whose read approximations changed in the previous iteration,
and clearing only their part of the memo. `==` on types ignores `lsb` and
`res`, as in C++, so the end of the iteration is decided by `==` while a
change of any field, `lsb` included, reaches the readers, as the full
iteration assigns every new approximation. The types are those of the full
iteration, field for field: checked on every annotation of the corpus (4 918
in 610 files) and of the 2 227 `faustlibraries` tests (18 547), whose
generated C++ is unchanged in scalar and `-vec` mode. A chain whose
approximations settle costs about 30 body inferences per group (`+ ~ sin`);
one whose `lsb` falls at every iteration (`+ ~ *(0.5)`) still changes all
the way, and is recomputed as the full iteration recomputes it, half its work.

`fir-prepare-normalize` falls to 1.1 s at 2048. What remains of `+ ~ w` is
`fir-effect-orientation` (9.8 s at 2048, ×5 per doubling): every foreign
call is an ordering barrier against every stateful node, N × N. The `sin`
control has no barrier.

## 3. What the report's figures were

The author's build was not stated; the plan's audit used a debug `faust-rs`,
and their `ondemand(si.bus(4096))` took 56 s where the release build took
3.95 s before the fix. Their program also went through `il.serialize_in` and
`il.frame_clock` from `interleave.lib`, which these measurements replace by
library-free equivalents. The quadratic factor they isolated in `ondemand`
is the one fixed above.

## Not done

- No case in the release `compile-budget-check` gate: on this machine its
  calibration program takes 3 ms, under the 4 ms floor, and the tool stops
  before measuring, so no baseline in calibration units could be recorded.
  The fixes are guarded instead by counting tests:
  `outputs_sharing_a_wide_group_classify_each_node_once` (transform) counts
  the nodes classified, 3N + 1 against 8 004 000 for N = 2 000 with a search
  per output; `a_nested_par_propagates_each_leaf_once_and_no_inner_par`
  (propagate) counts the memo probes of a 4 000-leaf chain, 2 977 against
  6 975 with a propagation per `Par` level.
- The foreign-call barriers of `fir-effect-orientation` (section 2).
- The report's original write-chain program (`fft_foreign-on-demand.dsp`) was
  not available.
