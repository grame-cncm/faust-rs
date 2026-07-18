/* ------------------------------------------------------------
name: "multi_out"
Code generated with Faust 2.84.3 (https://faust.grame.fr)
Compilation options: -lang cpp -fpga-mem-th 4 -ct 1 -es 1 -mcl 4 -mcd 9 -mfs 1024 -huf 0 -irt 4 -fls 4 -udd 1 -mdd 1024 -mdy 90 -mca 8 -ss 3 -single -ftz 0
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


class mydsp : public dsp {
	
 private:
	
	int IOTA0;
	// Ring Delay
	float fVec1SE[16];
	// Recursion delay fRec0SE is of type kZeroDelay
	// While its definition is of type kZeroDelay
	// Single Delay
	float fVec0SEState;
	int fSampleRate;
	
 public:
	mydsp() {
	}
	
	mydsp(const mydsp&) = default;
	
	virtual ~mydsp() = default;
	
	mydsp& operator=(const mydsp&) = default;
	
	void metadata(Meta* m) { 
		m->declare("compile_options", "-lang cpp -fpga-mem-th 4 -ct 1 -es 1 -mcl 4 -mcd 9 -mfs 1024 -huf 0 -irt 4 -fls 4 -udd 1 -mdd 1024 -mdy 90 -mca 8 -ss 3 -single -ftz 0");
		m->declare("filename", "multi_out.dsp");
		m->declare("name", "multi_out");
	}

	virtual int getNumInputs() {
		return 1;
	}
	virtual int getNumOutputs() {
		return 3;
	}
	
	static void classInit(int sample_rate) {
	}
	
	virtual void instanceConstants(int sample_rate) {
		fSampleRate = sample_rate;
	}
	
	virtual void instanceResetUserInterface() {
	}
	
	virtual void instanceClear() {
		IOTA0 = 0;
		for (int l0 = 0; l0 < 16; l0 = l0 + 1) {
			fVec1SE[l0] = 0.0f;
		}
		fVec0SEState = 0.0f;
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
	
	virtual int getSampleRate() {
		return fSampleRate;
	}
	
	virtual void buildUserInterface(UI* ui_interface) {
		ui_interface->openVerticalBox("multi_out");
		ui_interface->closeBox();
	}
	
	virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
		float fVec0SE[2];
		fVec0SE[1] = fVec0SEState;
		FAUSTFLOAT* input0 = inputs[0];
		FAUSTFLOAT* output0 = outputs[0];
		FAUSTFLOAT* output1 = outputs[1];
		FAUSTFLOAT* output2 = outputs[2];
		for (int i0 = 0; i0 < count; i0 = i0 + 1) {
			float fTemp0SE = static_cast<float>(input0[i0]);
			fVec1SE[IOTA0 & 15] = fTemp0SE;
			float fRec0SE = fVec1SE[IOTA0 & 15] + 0.5f * fVec0SE[1];
			float fTemp1SE = fRec0SE;
			fVec0SE[0] = fTemp1SE;
			output0[i0] = static_cast<FAUSTFLOAT>(0.5f * fVec1SE[IOTA0 & 15]);
			output1[i0] = static_cast<FAUSTFLOAT>(fVec0SE[0]);
			output2[i0] = static_cast<FAUSTFLOAT>(fVec1SE[(IOTA0 - 10) & 15]);
			IOTA0 = IOTA0 + 1;
			fVec0SE[1] = fVec0SE[0];
		}
		fVec0SEState = fVec0SE[1];
	}

};

#endif
