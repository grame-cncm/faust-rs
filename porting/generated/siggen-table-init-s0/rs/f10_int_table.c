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

static const int iTbl57[64] = {0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98, 100, 102, 104, 106, 108, 110, 112, 114, 116, 118, 120, 122, 124, 126};

typedef struct {
    int fSampleRate;
    int iRec28;
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
}

void instanceResetUserInterfacemydsp(mydsp* dsp) {
}

void instanceClearmydsp(mydsp* dsp) {
    dsp->iRec28 = 0;
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
    ui_interface->openVerticalBox(ui_interface->uiInterface, "f10_int_table");
    ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
    // signal_fir_fastlane_step2a: executable base slice
    // io: inputs=0 outputs=1
    // signals: 1
    FAUSTFLOAT* output0 = outputs[0];
    for (int i0 = 0; i0 < count; i0 = i0 + 1) {
        int iRecCur28 = (dsp->iRec28 + 1);
        output0[i0] = ((FAUSTFLOAT)(iTbl57[(dsp->iRec28 % 64)]));
        dsp->iRec28 = iRecCur28;
    }
}


#ifdef __cplusplus
}
#endif

#endif
