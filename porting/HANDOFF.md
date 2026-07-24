# Session Handoff

Date: 2026-07-24

## Repo State

- Branch: `external-control-one-sample` (linear on top of `main`).
- Executing `porting/external-control-one-sample-port-plan-2026-07-23-en.md`
  (D1-D3 approved; the plan's §5.7 AssemblyScript amendment was merged via
  PR #11, so the asc one-sample follow-up is officially reserved).

## Plan progress

| Phase | State |
|---|---|
| 0 — baselines, reference toolchain | done |
| 1 — typed options, capability table, CLI, count/D2 diagnostics | done |
| 2 — FIR execution contract (tagged sections, control/frame, promotion, one-sample lowering) | done |
| 3 — C, C++, FIR emitters + differentials | done |
| 4 — Rust backend (D3 inherent methods) | done |
| 5 — vector external control + promoted-control-event certificate | done |
| 6 — hardening/docs | in progress (CLI guide + README done; coverage rerun pending) |

Follow-up reserved by plan §5.7: the AssemblyScript one-sample target
(`-ec -os` as primary combination, additive to the block contract; an
`adapted` contract decision to design explicitly).

## Key mechanisms (where things live)

- Options: `ControlRateMode`/`ProcessingApi` next to `ComputeMode`
  (transform::signal_fir), carried by SignalFirOptions/Compiler.
- Capability model: `compiler::execution` (single declarative table;
  FRS-EXEC-* diagnostics; `-os -vec` always rejected).
- Scalar split: tagged `ControlStatement { ownership, statement }` list in
  `signal_fir/module/state.rs`; promotion generalizes the konst-escape
  path (`materialize_in_bucket`); assembly in `module/build.rs`.
- One-sample: direct channel I/O in `module/core_lowering.rs`; empty
  canonical compute; `frame` before compute in the functions block.
- FIR contract: faust_api reserves `control(dsp)` /
  `frame(dsp, FAUSTFLOAT*, FAUSTFLOAT*)`; fir checker rules FIR-F08/F09.
- Vector `-ec`: UI snapshots + struct-promoted control roots
  (`vector/lower/signal.rs`, CSE Struct mode in `signal_fir/cse.rs`),
  `control` emission in `vector/module/lifecycle.rs`, certificate rules in
  `vector/module/check.rs` (`verify_external_control`) with corruption
  tests in `vector/module/tests.rs`.
- D2 classifier: `signal_fir/one_sample.rs` (FRS-SFIR-0010); foreign
  `count` rejection in `lower_fvar` (FRS-SFIR-0009).

## Reference toolchains

- Pinned dev reference: `../faust` @ `8eebea429`
  (`master-dev-ocpp-od-fir-2-FIR19`), built at `../faust/build/bin/faust`.
  CAVEATS recorded in the journal: its own `-ec -os` output does not
  compile for recursive DSPs (reported upstream:
  https://github.com/grame-cncm/faust/issues/1277 — sletz: does not happen
  on official master-dev; branch is experimental), its `-vec` is disabled
  unconditionally, and its `-os` Rust output looks self-inconsistent
  (unverified; check with a real cargo build before reporting).
- Behavioral oracle for `-ec -vec`: stable Faust 2.83.1
  (`/opt/homebrew/bin/faust`).

## Validation status (latest)

- Every commit: fmt, clippy workspace 0, workspace tests, golden 196.
- vector_mode oracle 36/36 at phases 4-5.
- Runtime differentials (external harness in the session scratchpad):
  scalar `-ec`/`-os`/`-ec -os` bit-exact vs block AND vs pinned reference
  (stateless case; recursive case arbitrated by the internal block-vs-frame
  oracle); Rust `control+frame×N` bit-exact (rustc -D warnings);
  `-ec -vec` bit-exact vs classic vector, scalar, and stable 2.83.1.
- Architecture projects (AGENTS.md): block and `-ec -os` Rust outputs pass
  `cargo check` inside a real `faust2jackrust -source` project; the jack
  LINK step is blocked locally (universal libjack without arm64 slice) —
  environment limitation, recorded.
- Golden-of-new-shapes decision: the golden harness snapshots default
  emission only (per-case option plumbing does not exist); the emitted
  `-ec`/`-os` shapes are locked by `crates/compiler/tests/execution_options.rs`
  and the transform structural tests instead (adapted, recorded).

## Commands

```bash
cargo test -p transform --lib                      # 403 tests
cargo test -p compiler --test execution_options    # shape locks
cargo test -p compiler --test vector_mode          # 36 oracle tests
cargo run -q -p xtask -- golden-check              # 196 OK
cargo run -q -p xtask -- vector-coverage-check     # 1,536 pairs expected
```

## Next steps

1. Finish phase 6: coverage rerun result, journal, this handoff.
2. AssemblyScript one-sample target (plan §5.7): design the asc
   `frame`/`control` contract (flat channel arrays; `-ec -os` primary),
   wire the capability table (`explicit` for asc), implement the emitter
   on the phase-2 FIR contract, differential against the wasm-music
   MidiVoice host expectations.
3. Optional upstream: verify the pinned reference's Rust `-os` output with
   a real cargo build; if broken, report as a sibling of issue #1277
   (noting sletz's triage: the branch is experimental).
