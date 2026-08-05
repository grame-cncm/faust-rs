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

static const float fTbl5[3] = {1.0f, 2.0f, 3.0f};
static const int iTbl9[3] = {7, 8, 9};

typedef struct {
    int fSampleRate;
    int iWave5;
    int iWave9;
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
    return 4;
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
    dsp->iWave5 = 0;
    dsp->iWave9 = 0;
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
    ui_interface->openVerticalBox(ui_interface->uiInterface, "f06_waveform_direct");
    ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
    // signal_fir_fastlane_step2a: executable base slice
    // io: inputs=0 outputs=4
    // signals: 4
    FAUSTFLOAT* output0 = outputs[0];
    FAUSTFLOAT* output1 = outputs[1];
    FAUSTFLOAT* output2 = outputs[2];
    FAUSTFLOAT* output3 = outputs[3];
    for (int i0 = 0; i0 < count; i0 = i0 + 1) {
        FAUSTFLOAT fTemp0 = ((FAUSTFLOAT)(3));
        output0[i0] = fTemp0;
        output1[i0] = ((FAUSTFLOAT)(fTbl5[dsp->iWave5]));
        output2[i0] = fTemp0;
        output3[i0] = ((FAUSTFLOAT)(iTbl9[dsp->iWave9]));
        dsp->iWave5 = ((dsp->iWave5 + 1) % 3);
        dsp->iWave9 = ((dsp->iWave9 + 1) % 3);
    }
}


#ifdef __cplusplus
}
#endif

#endif
