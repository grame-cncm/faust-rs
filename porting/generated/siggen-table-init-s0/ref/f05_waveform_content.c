/* ------------------------------------------------------------
name: "f05_waveform_content"
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

static float fmydspSIG0Wave0[5] = {1e+01f,2e+01f,3e+01f,4e+01f,5e+01f};
typedef struct {
	int fmydspSIG0Wave0_idx;
	int fSampleRate;
} mydspSIG0;

static mydspSIG0* newmydspSIG0() { return (mydspSIG0*)calloc(1, sizeof(mydspSIG0)); }
static void deletemydspSIG0(mydspSIG0* dsp) { free(dsp); }

int getNumInputsmydspSIG0(mydspSIG0* RESTRICT dsp) {
	return 0;
}
int getNumOutputsmydspSIG0(mydspSIG0* RESTRICT dsp) {
	return 1;
}

static void instanceInitmydspSIG0(mydspSIG0* dsp, int sample_rate) {
	dsp->fSampleRate = sample_rate;
	dsp->fmydspSIG0Wave0_idx = 0;
}

static void fillmydspSIG0(mydspSIG0* dsp, int count, float* table) {
	/* C99 loop */
	{
		int i1;
		for (i1 = 0; i1 < count; i1 = i1 + 1) {
			table[i1] = fmydspSIG0Wave0[dsp->fmydspSIG0Wave0_idx];
			dsp->fmydspSIG0Wave0_idx = (1 + dsp->fmydspSIG0Wave0_idx) % 5;
		}
	}
}

static float ftbl0mydspSIG0[5];

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
	float fConst0;
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
	m->declare(m->metaInterface, "filename", "f05_waveform_content.dsp");
	m->declare(m->metaInterface, "name", "f05_waveform_content");
}

int getSampleRatemydsp(mydsp* RESTRICT dsp) {
	return dsp->fSampleRate;
}

int getNumInputsmydsp(mydsp* RESTRICT dsp) {
	return 0;
}
int getNumOutputsmydsp(mydsp* RESTRICT dsp) {
	return 1;
}

void classInitmydsp(int sample_rate) {
	mydspSIG0* sig0 = newmydspSIG0();
	instanceInitmydspSIG0(sig0, sample_rate);
	fillmydspSIG0(sig0, 5, ftbl0mydspSIG0);
	deletemydspSIG0(sig0);
}

void instanceResetUserInterfacemydsp(mydsp* dsp) {
}

void instanceClearmydsp(mydsp* dsp) {
}

void instanceConstantsmydsp(mydsp* dsp, int sample_rate) {
	dsp->fSampleRate = sample_rate;
	dsp->fConst0 = ftbl0mydspSIG0[0];
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
	ui_interface->openVerticalBox(ui_interface->uiInterface, "f05_waveform_content");
	ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
	FAUSTFLOAT* output0 = outputs[0];
	/* C99 loop */
	{
		int i0;
		for (i0 = 0; i0 < count; i0 = i0 + 1) {
			output0[i0] = (FAUSTFLOAT)(dsp->fConst0);
		}
	}
}

#ifdef __cplusplus
}
#endif

#endif
