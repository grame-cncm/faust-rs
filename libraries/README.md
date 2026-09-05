# Project-local Faust libraries

This directory contains Faust libraries that exercise `faust-rs` extensions
and are versioned with this repository:

- `optimizers.lib` (prefix `op`) provides in-graph optimization on top of
  `fad`: update engines (LMS/NLMS, Adam and its variants, Lion, ...), losses
  and regularizers, reparameterizations (stable biquad poles from reflection
  coefficients), learning-rate schedules, one- to five-parameter
  least-squares and loss-first loops, damped Gauss-Newton loops, and a Newton
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

Both libraries follow the Faust libraries documentation conventions
(<https://faustlibraries.grame.fr/contributing/>): a `declare name`/`version`
header, section banners, and one documented block per public function with
`Usage`, `Where`, `Test` and `References` entries.
