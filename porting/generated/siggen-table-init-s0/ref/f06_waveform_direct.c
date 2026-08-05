/* ------------------------------------------------------------
name: "f06_waveform_direct"
Code generated with Faust 2.87.1 (https://faust.grame.fr)
Compilation options: -lang c -fpga-mem-th 4 -ct 1 -es 1 -mcd 16 -mdd 1024 -mdy 33 -single -ftz 0
------------------------------------------------------------ */

#ifndef  __mydsp_H__
#define  __mydsp_H__

#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif 


#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32)
#define RESTRICT __restrict
#else
#define RESTRICT __restrict__
#endif

#include <math.h>
#include <stdint.h>
#include <stdlib.h>

static float fmydspWave0[3] = {1.0f,2.0f,3.0f};
static int imydspWave1[3] = {7,8,9};

#ifndef FAUSTCLASS 
#define FAUSTCLASS mydsp
#endif

#ifdef __APPLE__ 
#define exp10f __exp10f
#define exp10 __exp10
#endif
#ifndef FAUSTMAXI
#define FAUSTMAXI
static inline int faustmaxi(int a, int b) { return (a > b) ? a : b; }
static inline int faustmini(int a, int b) { return (a < b) ? a : b; }
#endif

typedef struct {
	int fmydspWave0_idx;
	int imydspWave1_idx;
	int fSampleRate;
} mydsp;

mydsp* newmydsp() { 
	mydsp* dsp = (mydsp*)calloc(1, sizeof(mydsp));
	return dsp;
}

void deletemydsp(mydsp* dsp) { 
	free(dsp);
}

void metadatamydsp(MetaGlue* m) { 
	m->declare(m->metaInterface, "compile_options", "-lang c -fpga-mem-th 4 -ct 1 -es 1 -mcd 16 -mdd 1024 -mdy 33 -single -ftz 0");
	m->declare(m->metaInterface, "filename", "f06_waveform_direct.dsp");
	m->declare(m->metaInterface, "name", "f06_waveform_direct");
}

int getSampleRatemydsp(mydsp* RESTRICT dsp) {
	return dsp->fSampleRate;
}

int getNumInputsmydsp(mydsp* RESTRICT dsp) {
	return 0;
}
int getNumOutputsmydsp(mydsp* RESTRICT dsp) {
	return 4;
}

void classInitmydsp(int sample_rate) {
}

void instanceResetUserInterfacemydsp(mydsp* dsp) {
}

void instanceClearmydsp(mydsp* dsp) {
}

void instanceConstantsmydsp(mydsp* dsp, int sample_rate) {
	dsp->fSampleRate = sample_rate;
	dsp->fmydspWave0_idx = 0;
	dsp->imydspWave1_idx = 0;
}
	
void instanceInitmydsp(mydsp* dsp, int sample_rate) {
	instanceConstantsmydsp(dsp, sample_rate);
	instanceResetUserInterfacemydsp(dsp);
	instanceClearmydsp(dsp);
}

void initmydsp(mydsp* dsp, int sample_rate) {
	classInitmydsp(sample_rate);
	instanceInitmydsp(dsp, sample_rate);
}

void buildUserInterfacemydsp(mydsp* dsp, UIGlue* ui_interface) {
	ui_interface->openVerticalBox(ui_interface->uiInterface, "f06_waveform_direct");
	ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
	FAUSTFLOAT* output0 = outputs[0];
	FAUSTFLOAT* output1 = outputs[1];
	FAUSTFLOAT* output2 = outputs[2];
	FAUSTFLOAT* output3 = outputs[3];
	/* C99 loop */
	{
		int i0;
		for (i0 = 0; i0 < count; i0 = i0 + 1) {
			output0[i0] = (FAUSTFLOAT)(3);
			output1[i0] = (FAUSTFLOAT)(fmydspWave0[dsp->fmydspWave0_idx]);
			output2[i0] = (FAUSTFLOAT)(3);
			output3[i0] = (FAUSTFLOAT)(imydspWave1[dsp->imydspWave1_idx]);
			dsp->fmydspWave0_idx = (1 + dsp->fmydspWave0_idx) % 3;
			dsp->imydspWave1_idx = (1 + dsp->imydspWave1_idx) % 3;
		}
	}
}

#ifdef __cplusplus
}
#endif

#endif
