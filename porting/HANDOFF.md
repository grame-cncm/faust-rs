# Session Handoff

Date: 2026-11-07

## Repo State

- Branch: `mojo-backend`, from `main` at `16c414d7`, in the main checkout.
  Not pushed (the user pushes).
- HEAD: the commit containing this handoff. Its parent is `32b1eea1`.
- **Ready to merge into `main-dev`** as a fast-forward: `main-dev` is still at
  `16c414d7`, so the 22 commits sit directly on it and no rebase is needed.

Commits since `main` (oldest first):

- `8fefe4ed` — the Mojo backend analysis and port plan.
- `b3f04be4` — `-lang mojo`, the backend of grame-cncm/faust#1293.
- `ebbc6995` — the Mojo lanes on 132 programs, on the Mojo 1.1 and 1.3 toolchains.
- `50e73099` — `make mojo` stops early when the Mojo command is missing.
- `dfd4ae6d`, `7a97d8df` — the GPU batch plan; the decision to use Mojo 1.3.
- `45c494f6` — G1, `make mojo-gpu`.
- `8a104629` — G2, `make mojo-gpu-bench`.
- `a1767d8f`, `c88696be` — the G3 design and its decisions D1 to D4.
- `30ec34db` — G3.2, `fir::batch::layout_voices`.
- `e1fbc5a2` — G3.3, its independent checker.
- `854e23b3` — G3.4, batch modules in the interpreter and the CPU differential.
- `052df77a` — G3.5, `MojoOptions::batch` and the GPU batch lanes.
- `8477baac` — G4, `make mojo-gpu-poly`.
- `6a935ed5` — the first version of this handoff.
- `fa47453a` — `porting/faust-gpu-audio-synthesis-2026-11-07-en.md`, does GPU
  audio with Faust make sense.
- `a77cb7bd` — the GPU voices in the golden corpus (`tests/corpus/gpu_*.dsp`).
- `4a2f8eb6`, `42af1df7` — the carillon, `make mojo-gpu-bells`, and its two
  tunes.
- `32b1eea1` — the carillon against an ordinary CPU program: the GPU gains
  ×1.15 to ×1.9 from about 220 bells, not ×5.7 to ×9.7; the synthesis states
  which CPU baseline each figure uses.

## Working Tree

- Tracked changes: `crates/sigtype/src/rules.rs`. This change predates the
  session, belongs to the user, and was kept out of every commit.
- Untracked: `tests/impulse-tests/reference-spectral/`. Also the user's, not
  touched.

## Current Goal(s)

- None open; the branch waits for the user's merge. Both streams are complete:
  - the Mojo backend (`porting/mojo-backend-port-analysis-and-plan-2026-11-06-en.md`, §9);
  - the GPU batch plan, G1 to G4 (`porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md`
    §7 to §9; G3 in `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`
    §7 to §10).

## What Changed This Session

- `-lang mojo`: an emitter with the contract of PR 1293, pinned at `05ac86fb1`, in
  `crates/codegen/src/backends/mojo/` (`mod.rs`, `batch.rs`, `names.rs`, `tests.rs`).
  Registry entry: DIFF-BACK-010.
- `fir::batch` (`crates/fir/src/batch/`): `layout_voices` lays out per-voice state
  as a structure of arrays, `(base + k) * N + v`. `check::check_voice_layout` is its
  independent checker, and `layout_voices` fails closed on it.
- The interpreter runs batch modules: `generate_interp_batch_module`,
  `instance_init_voice`, `compute_voice`.
- `MojoOptions::batch` emits the Rust-only batch contract, `struct
  mydsp(FaustDspBatch)`:
  - the struct is built over one state buffer of `state_bytes(voices)` bytes;
  - `voice` comes after `dsp` in every lifecycle method that runs per voice;
  - controls go through `set_control`.
  It has no CLI switch (decision D3). `crates/compiler/examples/mojo_batch.rs`
  emits it for the lanes.
- `tests/impulse-tests` gains six opt-in lanes, all needing `MOJO=`:

  | lane | what it does |
  |---|---|
  | `mojo` | the scalar backend; also `mojo-vec0` and `mojo-vec1` |
  | `mojo-gpu` | G1, the original per-voice structs on the GPU |
  | `mojo-gpu-batch` | the batch contract on the GPU |
  | `mojo-gpu-bench` | G2, timings for the per-voice structs |
  | `mojo-gpu-bench-batch` | timings for the batch contract |
  | `mojo-gpu-poly` | G4, the real-time runner |

  Harnesses live in `archs/*mojo*.mojo`. The GPU voices (the runner's
  instrument, the additive partial, the modal resonator) are in
  `tests/corpus/gpu_*.dsp`.

## Decisions / Constraints (important for resume)

- **Mojo toolchain.** The GPU work uses the latest 1.3 nightly
  (`1.3.0.dev2026110705`) with `modular` `26.8.0.dev2026110705`. The scalar Mojo
  lanes were also run on 1.1, the version PR 1293 pins.
- **Receiver alias.** Methods with loops alias their receiver (`ref dsp = self`).
  This works around a Mojo 1.1 compiler hang that is fixed in 1.3; keep the alias
  while the PR pins 1.1.
- **G3 decisions** (user, 2026-11-07):
  - D1: N is a run-time value.
  - D2: controls are per voice, through the control table.
  - D3: emitter option only, no CLI switch.
  - D4: the CPU differential runs in the Rust suite, through the interpreter.
- **G3.5 landed with a gap.** `zita_rev1` at 1,024 voices gains ×1.9, under the ×2
  criterion. The user decided to land it, and the gap is recorded in the design
  (§10) and in DIFF-BACK-010.
- **G4 choices** (user): PortAudio from conda-forge in a pixi environment, test
  runs muted, scripted notes.
- **GPU harness rules found on Metal.** Each was a silent failure:
  - Channel tables must live in device memory. A pointer into thread-private
    memory loses its writes through a `compute` that is not inlined.
  - Every buffer a kernel reaches must be one of its arguments, otherwise it reads
    zeros or stale data.
  - Kernel arguments must be fixed-width types, not `Int`.
- Never push. Run the AGENTS §3 gates before every commit.

## Validation Run

Merge readiness, on `32b1eea1`, in a clean worktree (without the user's
`rules.rs` change):

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -D
  warnings`: clean.
- `cargo test --workspace --all-targets`: 3,576 passed, 0 failed.
- The five structure gates (`cli-parser-check`, `error-model-check`,
  `ffi-boundary-check`, `structure-check`, `code-graphs --check`) and
  `golden-check` pass.
- Compilation cost, by hand because `compile-budget-check` cannot run on this
  machine: the 18 measurements of the basket (5 codegen cases, scalar and
  vector, and 8 `--check` cases), best of 5, release `faust-rs` of `main-dev`
  against HEAD: every ratio between 0.991 and 1.010, total 0.996.

Earlier:

- Codegen identity: default Mojo output is byte-identical to the build before
  G3.5 over the 634 `.dsp` of `tests/` and `examples/`. The 79 outputs that differ
  are errors in both builds, from missing libraries.
- Opt-in lanes (Apple M5, Mojo 1.3), program counts out of 132 with
  `CPP_ORACLE_UNSUPPORTED=` empty:
  - `make mojo` 132/132; `mojo-vec0` and `mojo-vec1` 132/132 on Mojo 1.1;
  - `mojo-gpu` 131/131 and `mojo-gpu-batch` 131/131 (`sound` and `math` excluded);
  - `mojo-gpu-poly`: 256 voices for 30 s with no underruns, at 256 frames and 48 kHz
    with 43 ms latency.
- The carillon (`make mojo-gpu-bells`): one bell matches `pm.standardBell` to
  1.5e-4; 37, 64, 256 and 1,024 bells play without an underrun.

## Open Issues / Blockers

- No blocker.
- **Not reported upstream (Mojo):**
  - writes lost through a private-memory pointer across a call;
  - a buffer that is not a kernel argument is not visible to the kernel.
  Each needs a reduced reproducer first.
- **Possible follow-ups, none requested:**
  - a callback-based PortAudio runner, for latency near the block length (the
    blocking API gives 43 ms at 256 frames);
  - sharing `fConst*` across voices;
  - DIFF-BACK-010 must be re-checked when PR 1293 merges.

## Next Steps (ordered)

1. The user merges `mojo-backend` into `main-dev` (fast-forward) and pushes.
2. If wanted: reduce and report the two Metal/Mojo findings above.
3. If wanted: a callback-based PortAudio runner (G4 follow-up).
4. If the GPU's relevance is to be settled: a CPU baseline with SIMD across
   voices (the most important caveat of the synthesis, §4), then an offline
   batch case (variant rendering or population calibration with `fad`/`rad`),
   the use case most favourable to the GPU and the only one not yet shown.

## Useful Commands to Resume

The scratch pixi environments of the session were under `/private/tmp` and are
gone. Recreate one with Mojo 1.3, `modular` and `portaudio` (for example
`pixi init`, then `pixi add mojo modular portaudio` with the channels
`https://conda.modular.com/max-nightly` and `conda-forge`), then:

```bash
M="pixi run --manifest-path <env>/pixi.toml -q mojo"
```

```bash
make -C tests/impulse-tests mojo-gpu-batch MOJO="$M" CPP_ORACLE_UNSUPPORTED=
```

```bash
make -C tests/impulse-tests mojo-gpu-bench-batch MOJO="$M"
```

```bash
make -C tests/impulse-tests mojo-gpu-poly MOJO="$M" PORTAUDIO_LIB=<env>/.pixi/envs/default/lib
```

```bash
cargo test -p compiler --test batch_voices_interp
```

## Notes

- `tests/impulse-tests/build/ref/cpp-oracle-manifest.mk` is a local, untracked file.
  It excludes 40 clock-domain programs from the lanes. Pass
  `CPP_ORACLE_UNSUPPORTED=` to include them.
- `RUST/faust-pr1293` is a git worktree of the C++ repository at `05ac86fb1`, with
  PR 1293's compiler and its Mojo 1.1 pixi environment. It is used only for
  comparisons. Remove it with:
  `git -C ../faust worktree remove --force ../faust-pr1293`.
