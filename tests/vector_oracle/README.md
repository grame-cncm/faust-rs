# Vector-scheduling C++ oracle captures (plan phase P0)

Versioned capture matrix of Faust C++ reference behavior for the vectorization
port plan (`porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`,
phase P0). Regenerate everything with `./capture.sh` (honors `FAUST_CPP_BIN`).

## Reference pinning

- Binary: `/Users/letz/Developpements/RUST/faust/build/bin/faust`
- `faust --version`: FAUST Version 2.84.3
- C++ repo HEAD at capture time: `8eebea4294a44a5260484c750d332781ed9f8ffd`
  on branch `master-dev-ocpp-od-fir-2-FIR19` — **matches the plan's pinned
  reference `8eebea429` exactly** (no mismatch).
- `/usr/local/bin/faust` on this machine is the same research build, not a
  mainline Faust.

## Flag inventory on this build

All plan-mentioned flags exist: `-ss <n>` (0/1/2/3+ with the documented
mapping), `-phs` (prints the hierarchical signal schedule **on stderr**),
`-vec`, `-vs`, `-lv`, `-dfs`, plus `-norm3` (normalized signal print after
scheduling) noted for possible later use.

## Key oracle facts (discovered, not assumed)

1. **`-vec` is rejected wholesale on this branch.** Every `-vec` invocation
   fails with `ERROR : '-vec' is not yet supported with 'ondemand' primitive`,
   even for programs that do not use `ondemand` at all (e.g.
   `shared_expr.dsp`). The plan anticipated the rejection only for ondemand
   programs; the reality is global. Consequently there is **no C++ vector-loop
   topology oracle available from the pinned reference**: the `.err` artifacts
   are the captures for every `vec_*` configuration, and vector-topology
   validation (P5) must rely on the plan's documented C++ semantics (plan
   section 2) plus Rust invariants — or on a separately built mainline Faust,
   which is a decision to record if taken (it would be a *different* reference
   commit).
2. **`-ss` changes scalar codegen on this branch.** For every multi-path case
   checked (`fork_join`, `shared_expr`, `rec_multi_delay`, `ui_control`,
   `multi_rec`), the generated C++ differs byte-wise between `-ss 0/1/2/3`
   (statement order), and the `-phs` schedules differ correspondingly. This is
   the differential target for P3 scalar activation.
3. `-phs` prints to **stderr**, with sections `Constant and Control Signals:`
   and `Audio Signals`, one line per signal id with type/interval/max-delay
   annotations — directly comparable to a future Rust `Hsched` dump.

## Capture matrix

Configurations per case (files `captures/<case>.<config>.{cpp,txt,err}`):

| Config | Flags | Artifact |
|---|---|---|
| `scalar` | `-lang cpp` | .cpp |
| `scalar_ss1/2/3` | `-lang cpp -ss N` | .cpp |
| `phs_ss0/1/2/3` | `-phs -ss N -lang cpp` | .txt (stderr schedule) |
| `vec_lv0` / `vec_lv1` | `-vec -lv N -lang cpp` | **.err on this branch** |
| `vec_dfs` | `-vec -dfs -lang cpp` | **.err on this branch** |
| `scalar_double`, `vec_lv0_double` | `+ -double` (4 numeric-sensitive cases) | .cpp / .err |

## Corpus (one structural shape per case)

| Case | Exercises |
|---|---|
| `shared_expr` | one shared sample expression, two consumers (multi-occurrence separation) |
| `simple_delayed` | verySimple value used direct + delayed (`maxDelay>0` dominates `verySimple`) |
| `slow_delayed` | slow control value used delayed (`maxDelay>0` dominates slow rate) |
| `rec_prefix_tail` | pure prefix -> serial recursion -> pure tail |
| `multi_out` | multiple outputs of mixed shapes |
| `multi_rec` | two independent recursion groups (also a lockstep-bundling candidate) |
| `rec_multi_delay` | one recursive value read at delays 0/2/50 (delay-plan geometry) |
| `short_delay` / `long_delay` | copy-buffer vs ring-buffer delay storage |
| `table_rw` | read/write table (mutable shared resource, effect ordering) |
| `ui_control` | UI controls (control-graph scheduling, Konst/Block placement) |
| `fork_join` | asymmetric fork/join (distinct `-ss` orders, `-phs` differential) |
| `ondemand_simple` | ondemand clock-domain wrapper (vector configs expectedly rejected) |

## Parity caveats

Per plan section 4.1: C++ within-level tie order follows `Tree`/pointer
ordering and is **not** a cross-language parity target; level membership and
dependency validity are. When comparing future Rust `-ss` output against the
`phs_*` captures, compare schedule *validity* and level structure first;
byte-level order agreement is expected only where the plan declares a parity
target (scalar `-ss 0/1/2/3` semantics), not for tie order.
