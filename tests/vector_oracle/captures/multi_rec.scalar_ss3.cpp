/* ------------------------------------------------------------
name: "multi_rec"
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
	
	// Recursion delay fRec0SE is of type kZeroDelay
	// While its definition is of type kZeroDelay
	// Recursion delay fRec1SE is of type kZeroDelay
	// While its definition is of type kZeroDelay
	// Single Delay
	float fVec0SEState;
	// Single Delay
	float fVec1SEState;
	int fSampleRate;
	
 public:
	mydsp() {
	}
	
	mydsp(const mydsp&) = default;
	
	virtual ~mydsp() = default;
	
	mydsp& operator=(const mydsp&) = default;
	
	void metadata(Meta* m) { 
		m->declare("compile_options", "-lang cpp -fpga-mem-th 4 -ct 1 -es 1 -mcl 4 -mcd 9 -mfs 1024 -huf 0 -irt 4 -fls 4 -udd 1 -mdd 1024 -mdy 90 -mca 8 -ss 3 -single -ftz 0");
		m->declare("filename", "multi_rec.dsp");
		m->declare("name", "multi_rec");
	}

	virtual int getNumInputs() {
		return 1;
	}
	virtual int getNumOutputs() {
		return 1;
	}
	
	static void classInit(int sample_rate) {
	}
	
	virtual void instanceConstants(int sample_rate) {
		fSampleRate = sample_rate;
	}
	
	virtual void instanceResetUserInterface() {
	}
	
	virtual void instanceClear() {
		fVec0SEState = 0.0f;
		fVec1SEState = 0.0f;
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
		ui_interface->openVerticalBox("multi_rec");
		ui_interface->closeBox();
	}
	
	virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
		float fVec0SE[2];
		fVec0SE[1] = fVec0SEState;
		float fVec1SE[2];
		fVec1SE[1] = fVec1SEState;
		FAUSTFLOAT* input0 = inputs[0];
		FAUSTFLOAT* output0 = outputs[0];
		for (int i0 = 0; i0 < count; i0 = i0 + 1) {
			float fTemp0SE = static_cast<float>(input0[i0]);
			float fRec0SE = fTemp0SE + 0.5f * fVec0SE[1];
			float fRec1SE = fTemp0SE + 0.9f * fVec1SE[1];
			float fTemp1SE = fRec0SE;
			fVec0SE[0] = fTemp1SE;
			float fTemp2SE = fRec1SE;
			fVec1SE[0] = fTemp2SE;
			output0[i0] = static_cast<FAUSTFLOAT>(fVec1SE[0] + fVec0SE[0]);
			fVec0SE[1] = fVec0SE[0];
			fVec1SE[1] = fVec1SE[0];
		}
		fVec0SEState = fVec0SE[1];
		fVec1SEState = fVec1SE[1];
	}

};

#endif
