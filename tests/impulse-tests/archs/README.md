# archs

This directory is intentionally (almost) empty.

The reference impulse responses and the native C/C++ backend tests are wrapped
in the **original C++ Faust 4-pass impulse architecture** (`impulsearch.cpp` +
`controlTools.h`). That architecture `#include`s headers from a C++ Faust source
tree (polyphonic wrapper, MIDI, soundfile, `libfaust.h`, ...), so it cannot be
vendored standalone here.

The makefiles therefore reference it in place via the `IMPULSE_ARCH` /
`CPP_TESTS` / `FAUST_ARCH` variables in [`../common.mk`](../common.mk), which
default to a local Faust checkout. Override them for your environment, e.g.:

```bash
make reference CPP_TESTS=/path/to/faust/tests/impulse-tests \
               FAUST_ARCH=/path/to/faust/architecture \
               FAUST_CPP=/path/to/faust/build/bin/faust
```

## Self-contained files (execution options)

The `-ec` / `-os` targets are the exception, and they are fully self-contained:

- `faust_minimal.h` — a faust-rs-owned reimplementation of the architecture
  surface the generated scalar code needs (`dsp`, `UI`, `Meta`, the `Soundfile`
  layout, and the suite's synthetic soundfile fixture). Not a copy of the C++
  Faust headers.
- `faust_minimal_cglue.h` — the `UIGlue` / `MetaGlue` layouts the C backend emits
  calls against, plus adapters onto the C++ interfaces above.
- `impulseexecopts_driver.h` — the scalar impulse driver, shared.
- `impulseexecopts.cpp` — C++ front-end (the generated class is used directly).
- `impulseexecopts_c.cpp` — C front-end (a wrapper class forwards to the
  generated C functions, as `impulsearch2.cpp` does for the classic target).

They exist because the reference architecture only ever calls
`compute(count, ...)`: it cannot drive `control()` (`-ec`) or `frame()` (`-os`),
so it would measure silence or uninitialized slow values and blame the compiler.
Being self-contained is a side benefit — verified by building these targets with
`FAUST_ARCH` and `CPP_TESTS` pointed at nonexistent paths.

Generating `reference/*.ir` still needs the C++ Faust compiler, as it does for
every other target: the oracle is unchanged.

Extending this to a full self-contained *poly/MIDI* harness (which is what would
remove the C++ Faust dependency from the classic `cpp`/`c` targets) remains a
future phase in the porting plan.

## Self-contained files (`-mem0`)

The `Make.mem0` lane is also self-contained and deliberately uses a smaller
scalar driver because its subject is manager ownership rather than polyphony:

- `faust_mem0.h` implements the legacy C++ `dsp_memory_manager` contract plus
  faust-rs checked allocation extensions;
- `faust_mem0_c_api.h` publishes the versioned context-carrying C callback ABI
  and is force-included while compiling generated C as strict C11;
- `impulsemem0.cpp` and `impulsemem0_c.cpp` poison fresh memory, reconcile each
  runtime allocation with the compiler description, reject unknown or duplicate
  destruction, and require no live allocation at shutdown.

The C++ driver does not require global LIFO destruction: generated temporary
subcontainers have lexical lifetimes that can end before the owning DSP. It
still verifies each allocation is destroyed exactly once. The C and Cranelift
paths additionally enforce their explicit reverse-order ownership contracts.
