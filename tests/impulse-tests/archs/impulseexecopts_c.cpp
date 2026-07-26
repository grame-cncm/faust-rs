/* C backend front-end for the -ec / -os scalar impulse driver.
 * ==========================================================
 *
 * The C backend emits a flat C API — `newmydsp()`, `initmydsp(dsp, sr)`,
 * `computemydsp(dsp, n, ins, outs)` and, under the execution options,
 * `controlmydsp(dsp)` / `framemydsp(dsp, in, out)` — not a C++ class. This file
 * wraps that API in the small class shape the shared driver expects, exactly as
 * the reference architecture's `impulsearch2.cpp` does for the classic target.
 *
 * `faust/gui/CGlue.h` supplies `buildUIGlue` / `buildMetaGlue`, the adapters
 * that let the C `buildUserInterfacemydsp` drive a C++ `UI`.
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

// Self-contained: see `faust_minimal.h`. `faust_minimal_cglue.h` provides the
// `UIGlue`/`MetaGlue` layouts the C backend emits calls against.
#include "faust_minimal_cglue.h"

// The generated C code is injected here by the makefile.
<<includeIntrinsic>>
<<includeclass>>

/* Forwards the class shape the driver uses onto the generated C functions.
 *
 * Only the members the driver actually calls are wrapped. `control` and `frame`
 * exist solely in the matching shapes, so their forwarders are compiled in only
 * when the corresponding macro is set — otherwise they would reference C
 * functions the compiler did not emit.
 */
struct CDspWrapper {
    mydsp* fDSP;

    CDspWrapper() : fDSP(newmydsp()) {}
    ~CDspWrapper() { deletemydsp(fDSP); }

    void init(int sample_rate) { initmydsp(fDSP, sample_rate); }
    int getNumInputs() { return getNumInputsmydsp(fDSP); }
    int getNumOutputs() { return getNumOutputsmydsp(fDSP); }

    void buildUserInterface(UI* ui_interface)
    {
        UIGlue glue;
        buildUIGlue(&glue, ui_interface);
        buildUserInterfacemydsp(fDSP, &glue);
    }

    void compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs)
    {
        computemydsp(fDSP, count, inputs, outputs);
    }

#ifdef IMPULSE_EC
    void control() { controlmydsp(fDSP); }
#endif

#ifdef IMPULSE_OS
    void frame(FAUSTFLOAT* inputs, FAUSTFLOAT* outputs) { framemydsp(fDSP, inputs, outputs); }
#endif
};

#undef FAUSTCLASS
#define FAUSTCLASS CDspWrapper

#include "impulseexecopts_driver.h"
