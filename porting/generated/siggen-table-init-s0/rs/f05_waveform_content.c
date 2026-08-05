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

static const float fTbl9[5] = {10.0f, 20.0f, 30.0f, 40.0f, 50.0f};

typedef struct {
    int fSampleRate;
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
    ui_interface->openVerticalBox(ui_interface->uiInterface, "f05_waveform_content");
    ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
    // signal_fir_fastlane_step2a: executable base slice
    // io: inputs=0 outputs=1
    // signals: 1
    FAUSTFLOAT* output0 = outputs[0];
    for (int i0 = 0; i0 < count; i0 = i0 + 1) {
        output0[i0] = ((FAUSTFLOAT)(fTbl9[0]));
    }
}


#ifdef __cplusplus
}
#endif

#endif
