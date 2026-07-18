/* ------------------------------------------------------------
name: "short_delay"
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
	
	// Copy Delay
	float fVec0SEState[4];
	int fSampleRate;
	
 public:
	mydsp() {
	}
	
	mydsp(const mydsp&) = default;
	
	virtual ~mydsp() = default;
	
	mydsp& operator=(const mydsp&) = default;
	
	void metadata(Meta* m) { 
		m->declare("compile_options", "-lang cpp -fpga-mem-th 4 -ct 1 -es 1 -mcl 4 -mcd 9 -mfs 1024 -huf 0 -irt 4 -fls 4 -udd 1 -mdd 1024 -mdy 90 -mca 8 -ss 3 -single -ftz 0");
		m->declare("filename", "short_delay.dsp");
		m->declare("name", "short_delay");
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
		for (int l0 = 0; l0 < 4; l0 = l0 + 1) {
			fVec0SEState[l0] = 0.0f;
		}
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
		ui_interface->openVerticalBox("short_delay");
		ui_interface->closeBox();
	}
	
	virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
		float fVec0SE[5];
		for (int j0 = 0; j0 < 4; j0 = j0 + 1) {
			fVec0SE[j0 + 1] = fVec0SEState[j0];
		}
		FAUSTFLOAT* input0 = inputs[0];
		FAUSTFLOAT* output0 = outputs[0];
		for (int i0 = 0; i0 < count; i0 = i0 + 1) {
			float fTemp0SE = static_cast<float>(input0[i0]);
			fVec0SE[0] = fTemp0SE;
			output0[i0] = static_cast<FAUSTFLOAT>(fVec0SE[0] + fVec0SE[4]);
			for (int j1 = 4; j1 > 0; j1 = j1 - 1) {
				fVec0SE[j1] = fVec0SE[j1 - 1];
			}
		}
		for (int j2 = 0; j2 < 4; j2 = j2 + 1) {
			fVec0SEState[j2] = fVec0SE[j2 + 1];
		}
	}

};

#endif
