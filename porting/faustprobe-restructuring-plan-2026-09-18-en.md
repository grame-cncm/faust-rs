# faustprobe: restructuring and factoring, plan (2026-09-18)

Status: **done** (2026-09-18, three commits). §6 records what was done.

## 1. Why

Five phases of feedback work (F1–F5) and their leftovers landed in one day, all
of them in the same two files. Measured on `4213cdfd`:

| file | lines | what makes it large |
|---|---:|---|
| `src/bin/faustprobe.rs` | 3545 | `run` 890 lines, `run_freqresp` 518, `Args` 425, `run_train` 355, `run_poly` 263 |
| `src/probe/engine.rs` | 1513 | `Factory`, `Probe` (460) and `PolyProbe` (551) in one file |
| `tests/*_probe.rs` | 5299 | `Fixtures`, `probe(args)`, `json`, `lines_of`, `workspace` rewritten in up to eight files |

The house standard (`xtask structure-check`, which guards `crates/transform`
and `crates/compiler`, not this crate) is 2000 lines a file and 200 lines a
function. The binary is over the first and four of its functions over the
second, one of them four times over.

The size is a symptom. The four modes (plain render and sweep, polyphony,
`--freqresp`, `--train`) each grew their own copy of what they share:

1. the table of `--list-params` (scalar, poly);
2. the failure of a render: `--fail-above`, then non-finite (scalar, poly),
   and the context that goes with it, `failure_context` and
   `poly_failure_context`, two thirds of which are the same text assembly;
3. the statistics of a channel, as `# outN:` lines and as JSON (scalar, poly);
4. where annotations go, stdout under `--quiet` and stderr otherwise: four
   closures (`annotate`, `emit` twice, `emit` of `--freqresp`);
5. the validation of a sweep's values into `clamped_axes`, and "the clamps this
   point ran under" (plain sweep, `--freqresp`);
6. the applied values of a point, `check_write(q, v).map_or(v, |w| w.applied)`,
   four times;
7. "reset, write `--set`, write the point" before a render, three times;
8. the total of `--time` over several renders, and its closing lines, twice;
9. the JSON document's head (`schema_version`, `dsp`, `sr`, ...), its optional
   `eval` and `timing.compile_s`, and its printing, four times;
10. the refusal of flags a mode does not take, a `for (flag, set) in [...]`
    loop written six times;
11. compile-and-time, four times; `--set` parsed into pairs, six times.

One defect found while reading: the doc comment of `reduce_channel` ("One
channel of a rendered window, reduced to a single number...") sits on
`PointResponse`, 1000 lines above the function it describes.

## 2. Invariant

**No output changes.** Not a message, not a digit, not the order of two lines,
not which of two errors is reported when a command line has both, not an exit
status. This is a refactoring, and the one property that makes it checkable.

Inconsistencies found on the way are recorded in §6 and left for a change of
their own (known so far: `--freqresp` words an `impulse:CH` beyond the inputs
differently from a plain render).

## 3. The check, written before the change

`cargo test` runs the ten `*_probe.rs` files (about 140 tests through the
binary). They assert on fragments, which is right for tests and not enough
here: a line that moved or a note that disappeared passes them.

So a harness (scratch, not committed: it cites absolute paths) runs **369
commands** (391 by the end, cases being added as the work showed what was not
covered) against a binary and records for each the exit status, stdout and
stderr: every mode, every format, every refusal table entry, the failure paths
(out of range, clamp, `--fail-above`, non-finite, not linear, not
time-invariant, ringing, a descent that reaches a bound, a gradient check that
fails, a loss that stops being finite), `--eval`, `--compare`/`--ref`/`--check`,
polyphony, and the 133 programs of `tests/impulse-tests/dsp` through
`--protocol impulse-test`. 231 exit 0, 138 do not. The numbers of `--time` are
masked, and nothing else.

Recorded with the binary of `4213cdfd`; the harness run twice on that binary
gives identical directories. It did not at first: the parser lists the repair
suggestions of a syntax error in an order that changes from run to run
(reported apart, not this plan's subject); those lists are sorted before the
comparison.

Every step below ends with `diff -r before after` empty, the crate's tests,
clippy with `-D warnings`, and fmt. The harness is shown to reject: a line of a
shared helper moved by hand must show in the diff.

## 4. Target layout

The binary becomes a directory, `src/bin/faustprobe/main.rs`, which is what
Cargo expects of a multi-file binary and removes the `#[path]` that
`determinism.rs` needed:

| module | holds |
|---|---|
| `main.rs` | `main`, the large-stack thread, the choice of a mode |
| `cli.rs` | `Args` and its enums, the refusal helper, `--protocol`'s conflicts |
| `setup.rs` | what the command line means: excitation, assignments, schedule, compilation (timed), output labels, number format |
| `writes.rs` | `Clamped`, the writes of a run checked before any render, a point's clamps and applied values, priming an instance |
| `report.rs` | where lines go, channel statistics (lines, JSON), `--time` (lines, JSON), the control table, the JSON document, silence notes |
| `failure.rs` | the failure of a render and its context, scalar and polyphonic |
| `verify.rs` | `--compare`, `--ref`, `--check`: setup, one render's verdict |
| `render.rs` | the plain render and the sweep |
| `poly.rs` | `--nvoices` |
| `freqresp.rs` | `--freqresp` |
| `train.rs` | `--train`, `--fd-check` |
| `determinism.rs` | as it is |

`probe/engine.rs` becomes `probe/engine/` (`mod.rs` with `RenderSpec` and the
re-exports, `factory.rs`, `probe.rs`, `poly_probe.rs`): every public path stays
(`probe::engine::Probe`), so no caller changes.

The integration tests share `tests/common/mod.rs`.

Targets: no function above 200 lines, no file of the binary above 700.

## 5. Order

1. Move to `faustprobe/main.rs`; `cli.rs` out (pure move).
2. The helpers out, as they are (pure moves): `setup`, `writes`, `report`,
   `failure`, `verify`.
3. Factor the eleven duplications of §1 into those modules.
4. The four modes out, each long function cut along its phases.
5. `engine/`.
6. `tests/common`.
7. Documents that cite the old paths; journal; structure numbers after.

## 6. What was done

Three commits, in the order of §5, each ending on the gate of §3.

| | before | after |
|---|---:|---:|
| largest file of the binary | 3545 | 647 (`render.rs`) |
| longest function of the binary | 890 (`run`) | 116 (`run_train`) |
| functions over 200 lines | 4 | 0 |
| `probe/engine.rs` | 1513 | 667 + 469 + 280 + 125 |
| lines of the binary | 3545 | 4333 |
| lines of the probe tests | 5299 | 5146 |
| harness cases identical | | 391 of 391 |
| crate tests | 382 | 382 |

The binary's total **grew**: eleven module headers, the types the phases pass
to each other (`Plan`, `Run`, `Rendered`, `Family`, `Report`, `Verification`,
`Subject`, `Verdict`, `Clamps`) and the signatures of some sixty functions
outweigh the two to three hundred duplicated lines that went. What fell is the
size of what has to be held in mind to change one thing.

Departures from §4: `setup.rs` also holds `compile_timed` and `timed`; the
refusal helper is `cli::refuse`, generic over what a table carries besides the
flag, because `--freqresp` says *why* it refuses each one.

**What the harness caught.** One change of mine: rewriting `--train` I parsed
every `--set` before checking any. The original does them one at a time, so of
two faulty `--set` it reports the first. No case covered it; five now do, for
the four modes. **What it missed**, shown by a mutation that survived: no case
gave a forwarded compiler argument (`--bra-tape`) a value other than the
default, so dropping the extra arguments changed nothing. Two cases added.

Seventeen mutations in all, ten on the binary's shared helpers, six on the
factored library code, one on the shared test helper: all rejected, the
survivor above once its cases existed.

**Inconsistencies found**, each an output, so left as they were by the
refactoring and fixed by a change of their own the same day (§7):

1. `--set fb=1.5 --set fb=2 --at 3 fb=2.5` and a failure: the context lists
   the control twice, `fb=2.5 fb=2`. A scheduled write updates the first of
   two entries of one control.
2. `--freqresp --in impulse:5` says `the program has 2 input(s)`; a plain
   render says `the program has 2 inputs, channels 0 to 1`.
3. A polyphonic CSV dump prints its header before `--in` is validated: a
   faulty `--in` leaves a header line on stdout.
4. `--train` checks its `--set` one at a time; the other modes parse them all
   first. Of two faulty `--set`, which is reported depends on the mode.
5. `Probe::set` clamps a NaN (to NaN); `ControlMap::check_write` answers the
   control's initial value for one. The command line goes through the second.

**Outside this plan**, found by the harness disagreeing with itself: the
parser lists the repair suggestions of a syntax error in an order that changes
from run to run.

Not done, deliberately: `probe/train.rs` (812 lines), `probe/poly.rs` (760)
and `probe/params.rs` (700) are each one subject with its unit tests, under
the threshold, and were left alone. `cranelift-ffi` is still outside
`structure-check`; adding the crate to it is a one-line change that would make
the two thresholds hold from now on.

## 7. The five inconsistencies, fixed

Asked for the same day, once the restructuring was in. Each is a behaviour
change, decided:

1. A control given twice is one control: the failure context lists it once,
   with the value it holds (`fb=2.5`). Scalar and polyphonic contexts now
   accumulate the same way.
2. One wording, `setup::check_impulse_channel`, for an impulse on an input
   the program does not have, in every mode.
3. The polyphonic dump validates `--in` before printing its header: a faulty
   command line prints nothing on stdout, in every mode.
4. `--train` parses every `--set` before checking any, as the other modes
   do: of a malformed `--set` and one out of range, the malformed one is
   reported.
5. The render writes what the check reported. `Probe::set` used to clamp on
   its own, so `--set gain=nan --clamp` said `# clamped /gain/gain: NaN ->
   0.5`, the failure context said `gain=0.5`, and the render ran with NaN and
   failed; the same for a scheduled write, a sweep point, a `--freqresp`, and
   the starting point of a descent (`train::resolve_params`). All five entry
   points now run with the initial value the line names, and the render
   succeeds.

Nine harness cases change, all of them these; a test per fix, each failing
with its defect put back (six mutations). The guide's `--clamp` paragraph
says what a `nan` becomes.

## 8. The harness, committed

Asked for the same day: "make the harness a committed tool". Not an xtask,
which was the form offered: an integration test, `crates/cranelift-ffi/tests/
output_snapshots.rs`, with its recordings in `tests/output/expected/` and its
fixtures in `tests/output/dsp/`. `cargo test` builds the binary, the test runs
in the Test step of the CI on every platform but Windows, and nothing was
added to `xtask`, its subcommand table or the CI file.

The same 399 cases, paths relative to the crate so that no recording holds a
machine's path; one file per case (`$ faustprobe ARGS`, `exit=`, stdout,
stderr, the fingerprint of a `--out` file); a stream over 32 KiB recorded as
a fingerprint (the 133 corpus programs, the two protocol renders of 15000
frames); the numbers of `--time` masked with the unit that follows them,
which a duration near a millisecond changes from one run to the next (the one
flake seen, in one run of five, before the unit was masked). The cases run on
every core, each in a temporary directory of its own: 11 s in debug for the
hand-written cases and one corpus program in six, 20 s for all of them
(`FAUSTPROBE_CORPUS=all`), 1.3 s in release; the debug and release binaries
give the same recordings, and so do sequential and parallel runs. Blessing is
`FAUSTPROBE_BLESS=1`. Three findings shown: an altered recording (the case,
the line, expected and actual), a case without a recording, a recording
without a case.

