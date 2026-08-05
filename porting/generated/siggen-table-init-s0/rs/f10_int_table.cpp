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

const static int iTbl57[64] = {0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98, 100, 102, 104, 106, 108, 110, 112, 114, 116, 118, 120, 122, 124, 126};

class mydsp : public dsp {
private:
    int fSampleRate;
    int iRec28;
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
        return 1;
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
        iRec28 = 0;
    }
    virtual void buildUserInterface(UI* ui_interface) {
        ui_interface->openVerticalBox("f10_int_table");
        ui_interface->closeBox();
    }
    virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
        FAUSTFLOAT* output0 = outputs[0];
        for (int i0 = 0; i0 < count; ++i0) {
            int iRecCur28 = (iRec28 + 1);
            output0[i0] = ((FAUSTFLOAT)(iTbl57[(iRec28 % 64)]));
            iRec28 = iRecCur28;
        }
    }
};

#endif
