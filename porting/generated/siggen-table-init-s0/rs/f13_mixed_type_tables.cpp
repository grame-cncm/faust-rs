/* ------------------------------------------------------------
name: "mydsp"
Code generated with Faust (https://faust.grame.fr)
------------------------------------------------------------ */

#ifndef  __mydsp_H__
#define  __mydsp_H__

#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif

#include <algorithm>
#include <cmath>
#include <cstdint>

#ifndef FAUSTCLASS
#define FAUSTCLASS mydsp
#endif

#ifdef __APPLE__
#define exp10f __exp10f
#define exp10 __exp10
#endif

#if defined(_WIN32)
#define RESTRICT __restrict
#else
#define RESTRICT __restrict__
#endif

const static float fTbl60[32] = {0.0f, 0.8414709568023682f, 0.9092974066734314f, 0.14112000167369843f, -0.756802499294281f, -0.9589242935180664f, -0.279415488243103f, 0.6569865942001343f, 0.9893582463264465f, 0.41211849451065063f, -0.5440211296081543f, -0.9999902248382568f, -0.5365729331970215f, 0.4201670289039612f, 0.9906073808670044f, 0.6502878665924072f, -0.2879033088684082f, -0.9613974690437317f, -0.7509872317314148f, 0.14987720549106598f, 0.9129452705383301f, 0.8366556167602539f, -0.008851309306919575f, -0.8462204337120056f, -0.9055783748626709f, -0.13235175609588623f, 0.7625584602355957f, 0.9563759565353394f, 0.2709057927131653f, -0.6636338829994202f, -0.9880316257476807f, -0.4040376543998718f};
const static int iTbl108[64] = {0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98, 100, 102, 104, 106, 108, 110, 112, 114, 116, 118, 120, 122, 124, 126};
const static int iTbl112[16] = {0, 3, 6, 9, 12, 15, 18, 21, 24, 27, 30, 33, 36, 39, 42, 45};

class mydsp : public dsp {
private:
    int fSampleRate;
    int iRec47;
public:
    mydsp() {
    }

    mydsp(const mydsp&) = default;

    virtual ~mydsp() = default;

    mydsp& operator=(const mydsp&) = default;

    virtual int getNumInputs() {
        return 0;
    }
    virtual int getNumOutputs() {
        return 3;
    }
    static void classInit(int sample_rate) {
        (void)sample_rate;
    }
    virtual int getSampleRate() {
        return fSampleRate;
    }
    virtual void init(int sample_rate) {
        classInit(sample_rate);
        instanceInit(sample_rate);
    }
    virtual void instanceInit(int sample_rate) {
        instanceConstants(sample_rate);
        instanceResetUserInterface();
        instanceClear();
    }
    virtual mydsp* clone() {
        return new mydsp(*this);
    }
    virtual void metadata(Meta* m) {
        (void)m;
        m->declare("filename", "mydsp.dsp");
        m->declare("name", "mydsp");
    }
    virtual void instanceConstants(int sample_rate) {
        fSampleRate = sample_rate;
    }
    virtual void instanceResetUserInterface() {
    }
    virtual void instanceClear() {
        iRec47 = 0;
    }
    virtual void buildUserInterface(UI* ui_interface) {
        ui_interface->openVerticalBox("f13_mixed_type_tables");
        ui_interface->closeBox();
    }
    virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
        FAUSTFLOAT* output0 = outputs[0];
        FAUSTFLOAT* output1 = outputs[1];
        FAUSTFLOAT* output2 = outputs[2];
        for (int i0 = 0; i0 < count; ++i0) {
            int iRecCur47 = (iRec47 + 1);
            output0[i0] = ((FAUSTFLOAT)(iTbl108[(iRec47 % 64)]));
            output1[i0] = ((FAUSTFLOAT)(fTbl60[(iRec47 % 32)]));
            output2[i0] = ((FAUSTFLOAT)(iTbl112[(iRec47 % 16)]));
            iRec47 = iRecCur47;
        }
    }
};

#endif
