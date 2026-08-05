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

const static float fTbl5[3] = {1.0f, 2.0f, 3.0f};
const static int iTbl9[3] = {7, 8, 9};

class mydsp : public dsp {
private:
    int fSampleRate;
    int iWave5;
    int iWave9;
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
        return 4;
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
        iWave5 = 0;
        iWave9 = 0;
    }
    virtual void buildUserInterface(UI* ui_interface) {
        ui_interface->openVerticalBox("f06_waveform_direct");
        ui_interface->closeBox();
    }
    virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
        FAUSTFLOAT* output0 = outputs[0];
        FAUSTFLOAT* output1 = outputs[1];
        FAUSTFLOAT* output2 = outputs[2];
        FAUSTFLOAT* output3 = outputs[3];
        for (int i0 = 0; i0 < count; ++i0) {
            FAUSTFLOAT fTemp0 = ((FAUSTFLOAT)(3));
            output0[i0] = fTemp0;
            output1[i0] = ((FAUSTFLOAT)(fTbl5[iWave5]));
            output2[i0] = fTemp0;
            output3[i0] = ((FAUSTFLOAT)(iTbl9[iWave9]));
            iWave5 = ((iWave5 + 1) % 3);
            iWave9 = ((iWave9 + 1) % 3);
        }
    }
};

#endif
