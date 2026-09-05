# Project-local Faust libraries

This directory contains Faust libraries that exercise `faust-rs` extensions
and are versioned with this repository:

- `optimizers.lib` (prefix `op`) provides in-graph optimization on top of
  `fad` and `rad`: update engines (LMS/NLMS, Adam and its variants, Lion,
  ...), losses and regularizers, reparameterizations (stable biquad poles
  from reflection coefficients), learning-rate schedules, one- to
  five-parameter least-squares and loss-first loops, damped Gauss-Newton
  loops, bus loops for `N` parameters in forward or reverse mode
  (`lsq_N`/`lsq_N_rad`, `descend_N`/`descend_N_rad`), clocked loops whose
  update runs once per firing of an `ondemand` clock, and a Newton solver.
  It imports `signals.lib`, `basics.lib`, `routes.lib` and `maths.lib`, so
  the Faust standard libraries must be on the import path too;
  `tests/corpus/opt_*.dsp` and `crates/compiler/tests/optimizers_lib.rs`
  exercise it (the tests skip when no faustlibraries checkout is found);
- `interleave.lib` provides frame-rate serialization around `ondemand` blocks.

Add this directory to the Faust import search path when compiling a DSP that
uses either library:

```sh
faust-rs -I libraries -I <faustlibraries> -lang cpp program.dsp
cargo run -p compiler -- --check program.dsp -I libraries -I <faustlibraries>
```

The library source keeps ordinary basename imports, for example
`op = library("optimizers.lib")`, `import("optimizers.lib")` or
`il = library("interleave.lib")`.

Three companion documents introduce `optimizers.lib` to readers new to
machine learning and differentiable DSP, in English and French:

- [optimizers-overview-en.md](optimizers-overview-en.md) /
  [optimizers-overview-fr.md](optimizers-overview-fr.md) — what `fad`/`rad`
  bring to differentiable DSP, how the library is organized, where each
  algorithm comes from and why it was chosen, measured behaviour, pitfalls;
- [optimizers-ddsp-tutorial-en.md](optimizers-ddsp-tutorial-en.md) /
  [optimizers-ddsp-tutorial-fr.md](optimizers-ddsp-tutorial-fr.md) — a
  step-by-step tutorial, from a hand-written gradient to a five-coefficient
  biquad, every program run with `faustprobe`;
- [ddsp-examples-en.md](ddsp-examples-en.md) /
  [ddsp-examples-fr.md](ddsp-examples-fr.md) — six complete DDSP programs
  (`tests/corpus/ddsp_*.dsp`, run by `crates/compiler/tests/ddsp_examples.rs`):
  an adaptive notch, a mode calibrated by Gauss-Newton and an amp model with
  `fad`; a 64-tap echo canceller, a neural waveshaper and block gradients
  handed to a host with `rad`.

Both libraries follow the Faust libraries documentation conventions
(<https://faustlibraries.grame.fr/contributing/>): a `declare name`/`version`
header, section banners, and one documented block per public function with
`Usage`, `Where`, `Test` and `References` entries.
