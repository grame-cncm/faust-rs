/* Scalar impulse driver for the -ec / -os execution options.
 * ==========================================================
 *
 * The regular C/C++ impulse targets use the genuine C++ Faust 4-pass
 * architecture (`impulsearch.cpp`), which calls `compute(count, ...)` and
 * nothing else. That architecture cannot exercise the execution options:
 *
 *   -ec  moves the block-rate control work into a separate `control()` entry
 *        point the host must call before each block. A host that never calls it
 *        reads uninitialized slow values.
 *   -os  moves the sample body into `frame(inputs, outputs)` over flat arrays
 *        and leaves the canonical `compute()` deliberately EMPTY. A host that
 *        only calls `compute()` therefore gets silence — the test would fail
 *        for the harness's reason, not the compiler's.
 *
 * This driver reproduces the scalar impulse pass only (the first frames of the
 * reference, compared with `filesCompare -part`, like the interp/rust/wasm
 * targets), and drives whichever entry points the selected shape provides:
 *
 *   (no macro)            control-free, block compute      — the classic shape
 *   -DIMPULSE_EC          control() then block compute
 *   -DIMPULSE_OS          per-sample frame()
 *   -DIMPULSE_EC + _OS    control() then per-sample frame()
 *
 * Being able to build the *same* source in all four shapes is the point: the
 * emitted impulse response must be identical in each, and identical to the
 * reference. That is the property the execution-options port claimed and that
 * `crates/compiler/tests/execution_options.rs` cannot check, because it only
 * inspects emitted signatures.
 *
 * Output format matches the reference exactly (header lines then one line per
 * frame), so `tools/filesCompare` can consume it unchanged.
 */

/* The interface type is architecture-controlled, and the whole suite runs in
 * double (`-double` selects double for the DSP core only). Leaving the
 * generated default of `float` here costs precision at the I/O boundary: it
 * shows up as a ~4e-6 absolute divergence on large-magnitude outputs
 * (`downsampling_06_multi_branch`), well past the 2e-6 tolerance, while every
 * small-signal DSP still passes — a failure mode that looks like a codegen bug
 * and is not one. `impulsearch.cpp` does the same on its very first line. */
#ifndef FAUSTFLOAT
#define FAUSTFLOAT double
#endif

#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

// Self-contained: faust-rs owns this architecture surface, so these targets
// need no C++ Faust checkout (unlike the classic cpp/c targets, which reuse the
// reference 4-pass architecture).
#include "faust_minimal.h"

#ifndef FAUSTCLASS
#define FAUSTCLASS mydsp
#endif

// The generated class is appended after this header by the makefile.
<<includeIntrinsic>>
<<includeclass>>

/* C++ backend front-end: `<<includeclass>>` above emitted the DSP class itself,
 * so the shared driver can use it as `FAUSTCLASS` unchanged. */
#include "impulseexecopts_driver.h"
