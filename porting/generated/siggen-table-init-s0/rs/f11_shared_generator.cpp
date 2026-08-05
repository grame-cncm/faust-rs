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

const static float fTbl82[64] = {0.0f, 0.0980171412229538f, 0.19509032368659973f, 0.290284663438797f, 0.3826834261417389f, 0.4713967442512512f, 0.5555702447891235f, 0.6343932747840881f, 0.7071067690849304f, 0.7730104327201843f, 0.8314695954322815f, 0.8819212913513184f, 0.9238795042037964f, 0.9569403529167175f, 0.9807852506637573f, 0.9951847195625305f, 1.0f, 0.9951847195625305f, 0.9807852506637573f, 0.9569403529167175f, 0.9238795042037964f, 0.8819212913513184f, 0.8314695954322815f, 0.7730104327201843f, 0.7071067690849304f, 0.6343932747840881f, 0.5555702447891235f, 0.4713967442512512f, 0.3826834261417389f, 0.290284663438797f, 0.19509032368659973f, 0.0980171412229538f, 0.00000000000000012246468525851679f, -0.0980171412229538f, -0.19509032368659973f, -0.290284663438797f, -0.3826834261417389f, -0.4713967442512512f, -0.5555702447891235f, -0.6343932747840881f, -0.7071067690849304f, -0.7730104327201843f, -0.8314695954322815f, -0.8819212913513184f, -0.9238795042037964f, -0.9569403529167175f, -0.9807852506637573f, -0.9951847195625305f, -1.0f, -0.9951847195625305f, -0.9807852506637573f, -0.9569403529167175f, -0.9238795042037964f, -0.8819212913513184f, -0.8314695954322815f, -0.7730104327201843f, -0.7071067690849304f, -0.6343932747840881f, -0.5555702447891235f, -0.4713967442512512f, -0.3826834261417389f, -0.290284663438797f, -0.19509032368659973f, -0.0980171412229538f};

class mydsp : public dsp {
private:
    int fSampleRate;
    int iRec39;
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
        return 2;
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
        iRec39 = 0;
    }
    virtual void buildUserInterface(UI* ui_interface) {
        ui_interface->openVerticalBox("f11_shared_generator");
        ui_interface->closeBox();
    }
    virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
        FAUSTFLOAT* output0 = outputs[0];
        FAUSTFLOAT* output1 = outputs[1];
        for (int i0 = 0; i0 < count; ++i0) {
            int iRecCur39 = (iRec39 + 1);
            output0[i0] = ((FAUSTFLOAT)(fTbl82[(iRec39 % 64)]));
            output1[i0] = ((FAUSTFLOAT)(fTbl82[((iRec39 + 7) % 64)]));
            iRec39 = iRecCur39;
        }
    }
};

#endif
