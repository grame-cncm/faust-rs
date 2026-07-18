/* ------------------------------------------------------------
name: "ondemand_simple"
Code generated with Faust 2.84.3 (https://faust.grame.fr)
Compilation options: -lang cpp -fpga-mem-th 4 -ct 1 -es 1 -mcl 4 -mcd 9 -mfs 1024 -huf 0 -irt 4 -fls 4 -udd 1 -mdd 1024 -mdy 90 -mca 8 -ss 0 -single -ftz 0
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
	
	// Perm Var
	float fPermVar0SE;
	int fSampleRate;
	
 public:
	mydsp() {
	}
	
	mydsp(const mydsp&) = default;
	
	virtual ~mydsp() = default;
	
	mydsp& operator=(const mydsp&) = default;
	
	void metadata(Meta* m) { 
		m->declare("compile_options", "-lang cpp -fpga-mem-th 4 -ct 1 -es 1 -mcl 4 -mcd 9 -mfs 1024 -huf 0 -irt 4 -fls 4 -udd 1 -mdd 1024 -mdy 90 -mca 8 -ss 0 -single -ftz 0");
		m->declare("filename", "ondemand_simple.dsp");
		m->declare("name", "ondemand_simple");
	}

	virtual int getNumInputs() {
		return 2;
	}
	virtual int getNumOutputs() {
		return 1;
	}
	
	static void classInit(int sample_rate) {
	}
	
	virtual void instanceConstants(int sample_rate) {
		fSampleRate = sample_rate;
		fPermVar0SE = 0.0f;
	}
	
	virtual void instanceResetUserInterface() {
	}
	
	virtual void instanceClear() {
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
		ui_interface->openVerticalBox("ondemand_simple");
		ui_interface->closeBox();
	}
	
	virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
		FAUSTFLOAT* input0 = inputs[0];
		FAUSTFLOAT* input1 = inputs[1];
		FAUSTFLOAT* output0 = outputs[0];
		for (int i0 = 0; i0 < count; i0 = i0 + 1) {
			float fTemp0SE = static_cast<float>(input1[i0]);
			if (static_cast<int>(static_cast<float>(input0[i0])) != 0) {
				for (int od0 = 0; od0 < static_cast<int>(static_cast<float>(input0[i0])); od0 = od0 + 1) {
					fPermVar0SE = 0.5f * fTemp0SE;
				}
			}
			output0[i0] = static_cast<FAUSTFLOAT>(fPermVar0SE);
		}
	}

};

#endif
