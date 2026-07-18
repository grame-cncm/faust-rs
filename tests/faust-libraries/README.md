# Vendored Faust standard-library subset

A minimal, unmodified subset of the [Faust standard libraries]
(https://github.com/grame-cncm/faustlibraries) vendored so that tests which
compile library-importing DSP fixtures (e.g.
`crates/compiler/tests/signal_fir_lane.rs` compiling
`tests/impulse-tests/dsp/karplus.dsp`) are hermetic: the default compiler
search paths (`/usr/local/share/faust`, ...) only resolve on machines with a
local Faust installation, which CI runners do not have.

Contents (the transitive import closure of `karplus.dsp` and `APF.dsp`):

| File | Imported by |
|---|---|
| `music.lib` | `karplus.dsp`, `bandfilter.dsp`, ... (legacy compatibility library) |
| `math.lib` | `music.lib` (legacy compatibility library) |
| `maths.lib` | `math.lib` |
| `maxmsp.lib` | `APF.dsp`, `LPF.dsp`, ... |
| `platform.lib` | `maths.lib` |

Copied verbatim from a Faust 2.83.x installation (Homebrew,
`/opt/homebrew/share/faust`). Each file keeps its own GRAME LGPL (with the
Faust compilation exception) license header. Do not edit these files; refresh
them from an up-to-date Faust installation instead, and re-run
`cargo test -p compiler --test signal_fir_lane` — the load-CSE tests assert
exact generated variable names, which depend on the library contents.
