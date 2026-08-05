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

const static float fTbl84[64] = {0.0f, 0.5f, 1.0f, 1.5f, 2.0f, 2.5f, 3.0f, 3.5f, 4.0f, 4.5f, 5.0f, 5.5f, 6.0f, 6.5f, 7.0f, 7.5f, 8.0f, 8.5f, 9.0f, 9.5f, 10.0f, 10.5f, 11.0f, 11.5f, 12.0f, 12.5f, 13.0f, 13.5f, 14.0f, 14.5f, 15.0f, 15.5f, 16.0f, 16.5f, 17.0f, 17.5f, 18.0f, 18.5f, 19.0f, 19.5f, 20.0f, 20.5f, 21.0f, 21.5f, 22.0f, 22.5f, 23.0f, 23.5f, 24.0f, 24.5f, 25.0f, 25.5f, 26.0f, 26.5f, 27.0f, 27.5f, 28.0f, 28.5f, 29.0f, 29.5f, 30.0f, 30.5f, 31.0f, 31.5f};

class mydsp : public dsp {
private:
    int fSampleRate;
    int iVec5[2];
    float fConst0;
    float fRec145;
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
        fConst0 = (1.0f / std::fmin(192000.0f, std::fmax(1.0f, ((float)(fSampleRate)))));
    }
    virtual void instanceResetUserInterface() {
    }
    virtual void instanceClear() {
        for (int lDelay0 = 0; lDelay0 < 2; ++lDelay0) {
            iVec5[lDelay0] = 0;
        }
        fRec145 = 0.0f;
    }
    virtual void buildUserInterface(UI* ui_interface) {
        ui_interface->openVerticalBox("f08_nested_tables");
        ui_interface->closeBox();
    }
    virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
        FAUSTFLOAT* output0 = outputs[0];
        for (int i0 = 0; i0 < count; ++i0) {
            iVec5[0] = 1;
            float fTemp0 = ((1 - iVec5[1]) ? 0.0f : (fRec145 + fConst0));
            float fRecCur145 = (fTemp0 - std::floor(fTemp0));
            output0[i0] = ((FAUSTFLOAT)(fTbl84[std::max<int>(std::min<int>(63, ((int)((64.0f * fRecCur145)))), 0)]));
            iVec5[1] = iVec5[0];
            fRec145 = fRecCur145;
        }
    }
};

#endif
