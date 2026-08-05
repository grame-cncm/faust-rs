# SIGGEN table init — S0 frozen baseline

Date: 2026-08-05
Phase: S0 of `porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`
Purpose: freeze the "before" state so every later phase is measured against
numbers taken once, on one machine, with pinned compilers. No code changed.

## Pins

| Item | Value |
|---|---|
| `faust-rs` | `0cb97dd7` (release build) |
| Faust C++ reference | `2.87.1`, `/usr/local/bin/faust` |
| C++ source tree consulted | `/Users/letz/Developpements/faust` at `3db77f58` |
| Host | Darwin 21.6.0 |
| Reference flags | `-lang cpp` / `-lang c`, compiler defaults |

## Layout

```
dsp/    the twelve fixtures of plan §8.1, one file per row
ref/    faust 2.87.1 output, .cpp and .c, kept in full — the S4a oracle
rs/     faust-rs output at 0cb97dd7, plus .err for the rejected cases
```

Three `rs/` outputs exceed 32 KB because the table is folded into a literal
initializer list; they are stored head+tail with the full byte count recorded
inline. Nothing is lost: the omitted body is the literal list itself, and the
size is the measurement.

## Emitted size, `-lang cpp`

| Fixture | Reference | faust-rs | Ratio | Status |
|---|---:|---:|---:|---|
| `f01_osc_table` | 4 301 | 1 420 248 | **330×** | ok |
| `f02_subcontainer1` | 3 177 | — | — | **FRS-SFIR-0004** |
| `f03_sr_dependent` | 4 285 | — | — | **FRS-SFIR-0004** |
| `f04_rwtable_seeded` | 4 253 | 91 625 | **21.5×** | ok |
| `f05_waveform_content` | 3 127 | 2 189 | 0.70× | ok |
| `f06_waveform_direct` | 2 751 | 2 671 | 0.97× | ok |
| `f07_constant_content` | 3 175 | 395 499 | **124.6×** | ok |
| `f08_nested_tables` | 4 218 | 3 144 | 0.74× | ok |
| `f09_ffunction_gen` | 4 167 | — | — | **FRS-SFIR-0004** |
| `f10_int_table` | 3 270 | 2 535 | 0.77× | ok |
| `f11_shared_generator` | 3 739 | 3 738 | 1.00× | ok |
| `f12_two_generators` | 4 586 | 4 458 | 0.97× | ok |
| `f13_mixed_type_tables` | 5 258 | 3 564 | 0.68× | ok |

`f13` extends the plan's twelve-row matrix. It was added during S0 because
§5.6 rule 1 — one `tbl` counter shared by integer and real tables — was
inferred from `getTypedNames` (`instructions_compiler.cpp:1040`) and from a
same-type example only. `f13` is the direct evidence:

```
static int   itbl0mydspSIG0[64];
static float ftbl1mydspSIG1[32];
static int   itbl2mydspSIG2[16];
```

The counter advances 0, 1, 2 across the type change, confirming that `i`/`f` is
a prefix and not part of the counter key. Current faust-rs output for the same
program is `fTbl60[32]`, `iTbl108[64]`, `iTbl112[16]` — `SigId`-derived,
unordered, and the shape the S2 rename replaces.

`-lang c` reference sizes, for the S4a oracle: 5 151 / 3 706 / 5 065 / 5 053 /
3 663 / 3 155 / 3 773 / 5 022 / 4 968 / 3 926 / 4 450 / 5 440. The faust-rs `c`
outputs track the `cpp` ones within ~50 bytes, including the three rejections.

The rejection message is identical for all three failing fixtures:

```
[FRS-SFIR-0004] SIGGEN interpreter: foreign functions/constants/variables not supported
```

## Compile wall time, `-lang cpp`, mean of 3

| Fixture | Reference | faust-rs |
|---|---:|---:|
| `f01_osc_table` | 25 ms | 203 ms |
| `f02_subcontainer1` | 11 ms | 11 ms (rejects early) |
| `f03_sr_dependent` | 19 ms | 154 ms (rejects late) |
| `f04_rwtable_seeded` | 19 ms | 152 ms |
| `f05_waveform_content` | 10 ms | 10 ms |
| `f06_waveform_direct` | 10 ms | 11 ms |
| `f07_constant_content` | 14 ms | 99 ms |
| `f08_nested_tables` | 20 ms | 130 ms |
| `f09_ffunction_gen` | 20 ms | 122 ms |
| `f10_int_table` | 15 ms | 59 ms |
| `f11_shared_generator` | 16 ms | 85 ms |
| `f12_two_generators` | 16 ms | 85 ms |

Time tracks folded table size, as expected: the cost is materializing and
printing N FIR constant nodes. `f03` is worth noting — 154 ms spent before
rejecting, because the interpreter runs the whole preparation pipeline before
meeting the `FConst` it cannot evaluate.

## Structural counts

Extracted from the reference output (`class …SIG…` and file-scope `…tbl…`
declarations) and from the faust-rs output (folded `const static` tables).

| Fixture | ref sub-containers | ref static tables | faust-rs folded tables |
|---|---:|---:|---:|
| `f01_osc_table` | 1 | 1 | 1 |
| `f02_subcontainer1` | 1 | 1 | — |
| `f03_sr_dependent` | 1 | 1 | — |
| `f04_rwtable_seeded` | 1 | 0 (struct field) | 1 (+ copy loop) |
| `f05_waveform_content` | 1 | 1 | 1 |
| `f06_waveform_direct` | 0 | 0 | 2 (waveform path, C7) |
| `f07_constant_content` | 1 | 1 | 1 |
| `f08_nested_tables` | **1** | **2** | 1 |
| `f09_ffunction_gen` | 1 | 1 | — |
| `f10_int_table` | 1 | 1 | 1 |
| `f11_shared_generator` | 1 | 1 | 1 |
| `f12_two_generators` | 2 | 2 | 2 |

Three rows carry information beyond the counts:

- **`f11`** — one sub-container and one table on both sides: generator sharing
  by tree identity already works in each compiler. The port must preserve it
  (contract C1 plus the `SigId`-keyed table map).
- **`f12`** — two sub-containers, `SIG0`/`SIG1`, and two tables: this is the
  fixture that pins deterministic numbering once §5.6 naming lands.
- **`f08`** — two tables declared upstream but only **one** filler class. The
  reference emits `static float ftbl0mydspSIG0SIG0[64];` at line 34 and never
  fills it; `classInit` calls `fillmydspSIG0(64, ftbl1mydspSIG0)` only. faust-rs
  folds the same program correctly to `{0.0f, 0.5f, 1.0f, 1.5f, …}`. This
  confirms the upstream nesting defect recorded in plan §2.4 and makes `f08` a
  regression guard, not a parity target: the port must fill both tables.

## Impulse-test baselines

Re-measured because the figures quoted in earlier documents (cpp 92/93,
c 87/93, interp 74/93) predate corpus growth and could not be trusted as a
starting point.

First attempt returned green in seconds with `Nothing to be done` — the cached
`ir/` trees from an earlier session, the false-green trap recorded in the
project memory. The measurement below was taken after removing `ir/{cpp,c,interp}`
and `build/{cpp,c,interp}`, keeping the expensive C++ oracle cache in
`reference/` and `build/ref/`.

Corpus: `tests/impulse-tests/dsp` holds 133 DSPs; the differential gate runs 93
of them, the remainder being excluded upstream-side (`CPP_ORACLE_UNSUPPORTED`)
or by `known.mk`. `subcontainer1` is listed in `KNOWN_FAILURES.md` as a shared
compile gap for every backend — it is the fixture this port exists to fix, and
removing that entry is part of S7.

### Result

| Backend | Gated | Passing | Command |
|---|---:|---:|---|
| `cpp` | 93 | **93** | `make cpp` |
| `c` | 93 | **93** | `make c` |
| `interp` | 93 | **93** | `make interp` |

All three gates exit 0 with 93 `.ir` files each. The count is the evidence, not
just the exit code: `Make.gcc` and `Make.interp` wrap every comparison as
`filesCompare … || (rm -f <ir>; false)`, so a mismatch both fails the build and
deletes its `.ir`. 93 surviving files means 93 comparisons passed.

### Denominators

| Set | Count | Note |
|---|---:|---|
| `dsp/` corpus | 133 | every fixture in the directory |
| No C++ oracle | 39 | `CPP_ORACLE_UNSUPPORTED` — the `ondemand_*`, `upsampling_*`, `downsampling_*` families, which upstream rejects with `ERROR : undefined symbol : ondemand`. These are faust-rs clock-domain extensions, not failures. |
| `KNOWN_FAIL_all` | 1 | `subcontainer1` — "faust-rs sub-container codegen gap (compile-fail)", `known.mk:35` |
| **Differential gate** | **93** | 133 − 39 − 1 |

### Correction to the inherited figures

Earlier documents quote cpp 92/93, c 87/93, interp 74/93, and the project
memory repeats them. They are stale: the three gates are at 93/93 today. The
`93` denominator survived only by coincidence — the corpus has since grown from
93 to 133 fixtures while 39 clock-domain DSPs and `subcontainer1` were excluded,
landing back on 93 gated cases.

Two consequences for the plan:

- Plan §8.2 layer 4 must not be read as "recover 92/93". Nothing is failing.
  The impulse target for this port is narrower and sharper: remove the
  `KNOWN_FAIL_all := subcontainer1` line from `known.mk`, take the gate from 93
  to 94 cases, and keep all three backends at 100%. Any drop is a regression
  caused by this port, with no pre-existing failures to hide behind.
- The `KNOWN_FAILURES.md` "Shared compile gap" table and the `known.mk` entry
  are both S7 deliverables. They are the observable definition of done.

## What S0 does not establish

- No numeric oracle for the three rejected fixtures: they have no faust-rs
  output at all today, so S5's numeric gate compares against the reference only.
- No `-double` variants: `f03` rejects in both precisions, and the remaining
  fixtures fold identically. S2 adds the `-double` pair once `runtime` mode can
  compile `f03`.
- No vector-mode outputs: the vector lowerer folds through its own path
  (`vector/lower/signal.rs`), measured in S6 rather than here.
