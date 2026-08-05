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

const static float fTbl9[5] = {10.0f, 20.0f, 30.0f, 40.0f, 50.0f};

class mydsp : public dsp {
private:
    int fSampleRate;
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
    }
    virtual void buildUserInterface(UI* ui_interface) {
        ui_interface->openVerticalBox("f05_waveform_content");
        ui_interface->closeBox();
    }
    virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
        FAUSTFLOAT* output0 = outputs[0];
        for (int i0 = 0; i0 < count; ++i0) {
            output0[i0] = ((FAUSTFLOAT)(fTbl9[0]));
        }
    }
};

#endif
