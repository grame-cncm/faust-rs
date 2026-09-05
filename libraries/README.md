# Project-local Faust libraries

This directory contains Faust libraries that exercise `faust-rs` extensions
and are versioned with this repository:

- `optimizers.lib` (prefix `op`) provides in-graph optimization on top of
  `fad`: update engines (LMS/NLMS, Adam and its variants, Lion, ...), losses
  and regularizers, reparameterizations (stable biquad poles from reflection
  coefficients), learning-rate schedules, one- to five-parameter
  least-squares and loss-first loops, damped Gauss-Newton loops, clocked loops
  whose update runs once per firing of an `ondemand` clock, and a Newton
  solver. It imports no standard library, so it compiles with `-I libraries`
  alone; `tests/corpus/opt_*.dsp` and `crates/compiler/tests/optimizers_lib.rs`
  exercise it;
- `interleave.lib` provides frame-rate serialization around `ondemand` blocks.

Add this directory to the Faust import search path when compiling a DSP that
uses either library:

```sh
faust-rs -I libraries -lang cpp program.dsp
cargo run -p compiler -- --check program.dsp -I libraries
```

The library source keeps ordinary basename imports, for example
`op = library("optimizers.lib")`, `import("optimizers.lib")` or
`il = library("interleave.lib")`.

Two companion documents introduce `optimizers.lib` to readers new to
machine learning and differentiable DSP, in English and French:

- [optimizers-overview-en.md](optimizers-overview-en.md) /
  [optimizers-overview-fr.md](optimizers-overview-fr.md) — what `fad`/`rad`
  bring to differentiable DSP, how the library is organized, where each
  algorithm comes from and why it was chosen, measured behaviour, pitfalls;
- [optimizers-ddsp-tutorial-en.md](optimizers-ddsp-tutorial-en.md) /
  [optimizers-ddsp-tutorial-fr.md](optimizers-ddsp-tutorial-fr.md) — a
  step-by-step tutorial, from a hand-written gradient to a five-coefficient
  biquad, every program run with `faustprobe`.

Both libraries follow the Faust libraries documentation conventions
(<https://faustlibraries.grame.fr/contributing/>): a `declare name`/`version`
header, section banners, and one documented block per public function with
`Usage`, `Where`, `Test` and `References` entries.
