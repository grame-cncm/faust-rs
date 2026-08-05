#ifndef  __mydsp_H__
#define  __mydsp_H__

#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32)
#define RESTRICT __restrict
#else
#define RESTRICT __restrict__
#endif

#include <math.h>
#include <stdint.h>
#include <stdlib.h>

#ifndef FAUSTCLASS
#define FAUSTCLASS mydsp
#endif

#ifdef __APPLE__
#define exp10f __exp10f
#define exp10 __exp10
#endif

static inline int faustmini(int a, int b) { return (a < b) ? a : b; }
static inline int faustmaxi(int a, int b) { return (a > b) ? a : b; }

static const float fTbl94[64] = {0.0f, 0.0980171412229538f, 0.19509032368659973f, 0.290284663438797f, 0.3826834261417389f, 0.4713967442512512f, 0.5555702447891235f, 0.6343932747840881f, 0.7071067690849304f, 0.7730104327201843f, 0.8314695954322815f, 0.8819212913513184f, 0.9238795042037964f, 0.9569403529167175f, 0.9807852506637573f, 0.9951847195625305f, 1.0f, 0.9951847195625305f, 0.9807852506637573f, 0.9569403529167175f, 0.9238795042037964f, 0.8819212913513184f, 0.8314695954322815f, 0.7730104327201843f, 0.7071067690849304f, 0.6343932747840881f, 0.5555702447891235f, 0.4713967442512512f, 0.3826834261417389f, 0.290284663438797f, 0.19509032368659973f, 0.0980171412229538f, 0.00000000000000012246468525851679f, -0.0980171412229538f, -0.19509032368659973f, -0.290284663438797f, -0.3826834261417389f, -0.4713967442512512f, -0.5555702447891235f, -0.6343932747840881f, -0.7071067690849304f, -0.7730104327201843f, -0.8314695954322815f, -0.8819212913513184f, -0.9238795042037964f, -0.9569403529167175f, -0.9807852506637573f, -0.9951847195625305f, -1.0f, -0.9951847195625305f, -0.9807852506637573f, -0.9569403529167175f, -0.9238795042037964f, -0.8819212913513184f, -0.8314695954322815f, -0.7730104327201843f, -0.7071067690849304f, -0.6343932747840881f, -0.5555702447891235f, -0.4713967442512512f, -0.3826834261417389f, -0.290284663438797f, -0.19509032368659973f, -0.0980171412229538f};
static const float fTbl99[32] = {1.0f, 0.9807852506637573f, 0.9238795042037964f, 0.8314695954322815f, 0.7071067690849304f, 0.5555702447891235f, 0.3826834261417389f, 0.19509032368659973f, 0.00000000000000006123234262925839f, -0.19509032368659973f, -0.3826834261417389f, -0.5555702447891235f, -0.7071067690849304f, -0.8314695954322815f, -0.9238795042037964f, -0.9807852506637573f, -1.0f, -0.9807852506637573f, -0.9238795042037964f, -0.8314695954322815f, -0.7071067690849304f, -0.5555702447891235f, -0.3826834261417389f, -0.19509032368659973f, -0.00000000000000018369701465288538f, 0.19509032368659973f, 0.3826834261417389f, 0.5555702447891235f, 0.7071067690849304f, 0.8314695954322815f, 0.9238795042037964f, 0.9807852506637573f};

typedef struct {
    int fSampleRate;
    int iRec43;
} mydsp;

mydsp* newmydsp() {
    mydsp* dsp = (mydsp*)calloc(1, sizeof(mydsp));
    return dsp;
}

void deletemydsp(mydsp* dsp) {
    free(dsp);
}

void metadatamydsp(MetaGlue* m) {
}

int getSampleRatemydsp(mydsp* RESTRICT dsp) {
    return dsp->fSampleRate;
}

int getNumInputsmydsp(mydsp* RESTRICT dsp) {
    (void)dsp;
    return 0;
}

int getNumOutputsmydsp(mydsp* RESTRICT dsp) {
    (void)dsp;
    return 2;
}

void classInitmydsp(int sample_rate) {
    (void)sample_rate;
}

void instanceConstantsmydsp(mydsp* dsp, int sample_rate) {
    dsp->fSampleRate = sample_rate;
}

void instanceResetUserInterfacemydsp(mydsp* dsp) {
}

void instanceClearmydsp(mydsp* dsp) {
    dsp->iRec43 = 0;
}

void instanceInitmydsp(mydsp* dsp, int sample_rate) {
    instanceConstantsmydsp(dsp, sample_rate);
    instanceResetUserInterfacemydsp(dsp);
    instanceClearmydsp(dsp);
}

void initmydsp(mydsp* dsp, int sample_rate) {
    classInitmydsp(sample_rate);
    instanceInitmydsp(dsp, sample_rate);
}

void buildUserInterfacemydsp(mydsp* dsp, UIGlue* ui_interface) {
    ui_interface->openVerticalBox(ui_interface->uiInterface, "f12_two_generators");
    ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
    // signal_fir_fastlane_step2a: executable base slice
    // io: inputs=0 outputs=2
    // signals: 2
    FAUSTFLOAT* output0 = outputs[0];
    FAUSTFLOAT* output1 = outputs[1];
    for (int i0 = 0; i0 < count; i0 = i0 + 1) {
        int iRecCur43 = (dsp->iRec43 + 1);
        output0[i0] = ((FAUSTFLOAT)(fTbl94[(dsp->iRec43 % 64)]));
        output1[i0] = ((FAUSTFLOAT)(fTbl99[(dsp->iRec43 % 32)]));
        dsp->iRec43 = iRecCur43;
    }
}


#ifdef __cplusplus
}
#endif

#endif
