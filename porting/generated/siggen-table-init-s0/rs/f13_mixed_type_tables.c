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

static const float fTbl60[32] = {0.0f, 0.8414709568023682f, 0.9092974066734314f, 0.14112000167369843f, -0.756802499294281f, -0.9589242935180664f, -0.279415488243103f, 0.6569865942001343f, 0.9893582463264465f, 0.41211849451065063f, -0.5440211296081543f, -0.9999902248382568f, -0.5365729331970215f, 0.4201670289039612f, 0.9906073808670044f, 0.6502878665924072f, -0.2879033088684082f, -0.9613974690437317f, -0.7509872317314148f, 0.14987720549106598f, 0.9129452705383301f, 0.8366556167602539f, -0.008851309306919575f, -0.8462204337120056f, -0.9055783748626709f, -0.13235175609588623f, 0.7625584602355957f, 0.9563759565353394f, 0.2709057927131653f, -0.6636338829994202f, -0.9880316257476807f, -0.4040376543998718f};
static const int iTbl108[64] = {0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98, 100, 102, 104, 106, 108, 110, 112, 114, 116, 118, 120, 122, 124, 126};
static const int iTbl112[16] = {0, 3, 6, 9, 12, 15, 18, 21, 24, 27, 30, 33, 36, 39, 42, 45};

typedef struct {
    int fSampleRate;
    int iRec47;
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
    return 3;
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
    dsp->iRec47 = 0;
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
    ui_interface->openVerticalBox(ui_interface->uiInterface, "f13_mixed_type_tables");
    ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
    // signal_fir_fastlane_step2a: executable base slice
    // io: inputs=0 outputs=3
    // signals: 3
    FAUSTFLOAT* output0 = outputs[0];
    FAUSTFLOAT* output1 = outputs[1];
    FAUSTFLOAT* output2 = outputs[2];
    for (int i0 = 0; i0 < count; i0 = i0 + 1) {
        int iRecCur47 = (dsp->iRec47 + 1);
        output0[i0] = ((FAUSTFLOAT)(iTbl108[(dsp->iRec47 % 64)]));
        output1[i0] = ((FAUSTFLOAT)(fTbl60[(dsp->iRec47 % 32)]));
        output2[i0] = ((FAUSTFLOAT)(iTbl112[(dsp->iRec47 % 16)]));
        dsp->iRec47 = iRecCur47;
    }
}


#ifdef __cplusplus
}
#endif

#endif
