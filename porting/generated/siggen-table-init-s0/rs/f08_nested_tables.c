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

static const float fTbl84[64] = {0.0f, 0.5f, 1.0f, 1.5f, 2.0f, 2.5f, 3.0f, 3.5f, 4.0f, 4.5f, 5.0f, 5.5f, 6.0f, 6.5f, 7.0f, 7.5f, 8.0f, 8.5f, 9.0f, 9.5f, 10.0f, 10.5f, 11.0f, 11.5f, 12.0f, 12.5f, 13.0f, 13.5f, 14.0f, 14.5f, 15.0f, 15.5f, 16.0f, 16.5f, 17.0f, 17.5f, 18.0f, 18.5f, 19.0f, 19.5f, 20.0f, 20.5f, 21.0f, 21.5f, 22.0f, 22.5f, 23.0f, 23.5f, 24.0f, 24.5f, 25.0f, 25.5f, 26.0f, 26.5f, 27.0f, 27.5f, 28.0f, 28.5f, 29.0f, 29.5f, 30.0f, 30.5f, 31.0f, 31.5f};

typedef struct {
    int fSampleRate;
    int iVec5[2];
    float fConst0;
    float fRec145;
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
    return 1;
}

void classInitmydsp(int sample_rate) {
    (void)sample_rate;
}

void instanceConstantsmydsp(mydsp* dsp, int sample_rate) {
    dsp->fSampleRate = sample_rate;
    dsp->fConst0 = (1.0f / fmin(192000.0f, fmax(1.0f, ((float)(dsp->fSampleRate)))));
}

void instanceResetUserInterfacemydsp(mydsp* dsp) {
}

void instanceClearmydsp(mydsp* dsp) {
    for (int lDelay0 = 0; lDelay0 < 2; lDelay0 = lDelay0 + 1) {
        dsp->iVec5[lDelay0] = 0;
    }
    dsp->fRec145 = 0.0f;
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
    ui_interface->openVerticalBox(ui_interface->uiInterface, "f08_nested_tables");
    ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
    // signal_fir_fastlane_step2a: executable base slice
    // io: inputs=0 outputs=1
    // signals: 1
    FAUSTFLOAT* output0 = outputs[0];
    for (int i0 = 0; i0 < count; i0 = i0 + 1) {
        dsp->iVec5[0] = 1;
        float fTemp0 = ((1 - dsp->iVec5[1]) ? 0.0f : (dsp->fRec145 + dsp->fConst0));
        float fRecCur145 = (fTemp0 - floor(fTemp0));
        output0[i0] = ((FAUSTFLOAT)(fTbl84[faustmaxi(faustmini(63, ((int)((64.0f * fRecCur145)))), 0)]));
        dsp->iVec5[1] = dsp->iVec5[0];
        dsp->fRec145 = fRecCur145;
    }
}


#ifdef __cplusplus
}
#endif

#endif
