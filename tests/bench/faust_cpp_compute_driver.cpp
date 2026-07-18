#include "faust_cpp_test_stubs.hpp"

#ifndef FAUST_TEST_DSP
#error "FAUST_TEST_DSP must name the generated C++ source"
#endif

#include FAUST_TEST_DSP

#if defined(__GNUC__) || defined(__clang__)
#define FAUST_TEST_NOINLINE __attribute__((noinline))
#else
#define FAUST_TEST_NOINLINE
#endif

extern "C" FAUST_TEST_NOINLINE void faust_test_compute(
    mydsp* instance,
    int count,
    FAUSTFLOAT** inputs,
    FAUSTFLOAT** outputs)
{
    instance->mydsp::compute(count, inputs, outputs);
}
