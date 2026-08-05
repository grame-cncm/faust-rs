/* ------------------------------------------------------------
name: "f06_waveform_direct"
Code generated with Faust 2.87.1 (https://faust.grame.fr)
Compilation options: -lang cpp -fpga-mem-th 4 -ct 1 -es 1 -mcd 16 -mdd 1024 -mdy 33 -single -ftz 0
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

const static float fmydspWave0[3] = {1.0f,2.0f,3.0f};
const static int imydspWave1[3] = {7,8,9};

class mydsp : public dsp {
	
 private:
	
	int fmydspWave0_idx;
	int imydspWave1_idx;
	int fSampleRate;
	
 public:
	mydsp() {
	}
	
	mydsp(const mydsp&) = default;
	
	virtual ~mydsp() = default;
	
	mydsp& operator=(const mydsp&) = default;
	
	void metadata(Meta* m) { 
		m->declare("compile_options", "-lang cpp -fpga-mem-th 4 -ct 1 -es 1 -mcd 16 -mdd 1024 -mdy 33 -single -ftz 0");
		m->declare("filename", "f06_waveform_direct.dsp");
		m->declare("name", "f06_waveform_direct");
	}

	virtual int getNumInputs() {
		return 0;
	}
	virtual int getNumOutputs() {
		return 4;
	}
	
	static void classInit(int sample_rate) {
	}
	
	virtual void instanceConstants(int sample_rate) {
		fSampleRate = sample_rate;
		fmydspWave0_idx = 0;
		imydspWave1_idx = 0;
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
		ui_interface->openVerticalBox("f06_waveform_direct");
		ui_interface->closeBox();
	}
	
	virtual void compute(int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
		FAUSTFLOAT* output0 = outputs[0];
		FAUSTFLOAT* output1 = outputs[1];
		FAUSTFLOAT* output2 = outputs[2];
		FAUSTFLOAT* output3 = outputs[3];
		for (int i0 = 0; i0 < count; i0 = i0 + 1) {
			output0[i0] = static_cast<FAUSTFLOAT>(3);
			output1[i0] = static_cast<FAUSTFLOAT>(fmydspWave0[fmydspWave0_idx]);
			output2[i0] = static_cast<FAUSTFLOAT>(3);
			output3[i0] = static_cast<FAUSTFLOAT>(imydspWave1[imydspWave1_idx]);
			fmydspWave0_idx = (1 + fmydspWave0_idx) % 3;
			imydspWave1_idx = (1 + imydspWave1_idx) % 3;
		}
	}

};

#endif
