# S0 baseline artifacts — SIGGEN table initialization

Frozen 2026-08-05 for
`porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`.

Read [`baseline-2026-08-05-en.md`](./baseline-2026-08-05-en.md) first: it holds
the pins, the measurements, and what they mean.

| Directory | Contents |
|---|---|
| `dsp/` | the 13 fixtures (plan §8.1 plus the mixed-type naming probe) |
| `ref/` | `faust 2.87.1` output, `.cpp` and `.c`, kept in full — the S4a emission oracle |
| `rs/` | `faust-rs 0cb97dd7` output, plus `.err` for the three rejected fixtures |

Regenerate any single artifact:

```sh
faust     -lang cpp dsp/f01_osc_table.dsp -o ref/f01_osc_table.cpp
faust-rs  -lang cpp dsp/f01_osc_table.dsp -o rs/f01_osc_table.cpp
```

Three `rs/` outputs are stored head+tail with their full byte count recorded
inline, because folding a large table produces up to 1.4 MB of literal list.
The elided part is the list itself; its size is the measurement.

These files are a snapshot, not a gate. Nothing reads them automatically. When
a phase changes the emitted shape, add the new output beside the old one rather
than overwriting it — the point is to keep the before/after diff available.
