/* ------------------------------------------------------------
name: "f12_two_generators"
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

typedef struct {
	int iRec0[2];
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
	/* C99 loop */
	{
		int l0;
		for (l0 = 0; l0 < 2; l0 = l0 + 1) {
			dsp->iRec0[l0] = 0;
		}
	}
}

static void fillmydspSIG0(mydspSIG0* dsp, int count, float* table) {
	/* C99 loop */
	{
		int i1;
		for (i1 = 0; i1 < count; i1 = i1 + 1) {
			dsp->iRec0[0] = dsp->iRec0[1] + 1;
			table[i1] = sinf(0.09817477f * (float)(dsp->iRec0[1]));
			dsp->iRec0[1] = dsp->iRec0[0];
		}
	}
}

typedef struct {
	int iRec2[2];
	int fSampleRate;
} mydspSIG1;

static mydspSIG1* newmydspSIG1() { return (mydspSIG1*)calloc(1, sizeof(mydspSIG1)); }
static void deletemydspSIG1(mydspSIG1* dsp) { free(dsp); }

int getNumInputsmydspSIG1(mydspSIG1* RESTRICT dsp) {
	return 0;
}
int getNumOutputsmydspSIG1(mydspSIG1* RESTRICT dsp) {
	return 1;
}

static void instanceInitmydspSIG1(mydspSIG1* dsp, int sample_rate) {
	dsp->fSampleRate = sample_rate;
	/* C99 loop */
	{
		int l2;
		for (l2 = 0; l2 < 2; l2 = l2 + 1) {
			dsp->iRec2[l2] = 0;
		}
	}
}

static void fillmydspSIG1(mydspSIG1* dsp, int count, float* table) {
	/* C99 loop */
	{
		int i2;
		for (i2 = 0; i2 < count; i2 = i2 + 1) {
			dsp->iRec2[0] = dsp->iRec2[1] + 1;
			table[i2] = cosf(0.19634955f * (float)(dsp->iRec2[1]));
			dsp->iRec2[1] = dsp->iRec2[0];
		}
	}
}

static float ftbl0mydspSIG0[64];
static float ftbl1mydspSIG1[32];

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
	int iRec1[2];
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
	m->declare(m->metaInterface, "basics.lib/name", "Faust Basic Element Library");
	m->declare(m->metaInterface, "basics.lib/version", "1.23.0");
	m->declare(m->metaInterface, "compile_options", "-lang c -fpga-mem-th 4 -ct 1 -es 1 -mcd 16 -mdd 1024 -mdy 33 -single -ftz 0");
	m->declare(m->metaInterface, "filename", "f12_two_generators.dsp");
	m->declare(m->metaInterface, "maths.lib/author", "GRAME");
	m->declare(m->metaInterface, "maths.lib/copyright", "GRAME");
	m->declare(m->metaInterface, "maths.lib/license", "LGPL with exception");
	m->declare(m->metaInterface, "maths.lib/name", "Faust Math Library");
	m->declare(m->metaInterface, "maths.lib/version", "2.9.0");
	m->declare(m->metaInterface, "name", "f12_two_generators");
}

int getSampleRatemydsp(mydsp* RESTRICT dsp) {
	return dsp->fSampleRate;
}

int getNumInputsmydsp(mydsp* RESTRICT dsp) {
	return 0;
}
int getNumOutputsmydsp(mydsp* RESTRICT dsp) {
	return 2;
}

void classInitmydsp(int sample_rate) {
	mydspSIG0* sig0 = newmydspSIG0();
	instanceInitmydspSIG0(sig0, sample_rate);
	fillmydspSIG0(sig0, 64, ftbl0mydspSIG0);
	mydspSIG1* sig1 = newmydspSIG1();
	instanceInitmydspSIG1(sig1, sample_rate);
	fillmydspSIG1(sig1, 32, ftbl1mydspSIG1);
	deletemydspSIG0(sig0);
	deletemydspSIG1(sig1);
}

void instanceResetUserInterfacemydsp(mydsp* dsp) {
}

void instanceClearmydsp(mydsp* dsp) {
	/* C99 loop */
	{
		int l1;
		for (l1 = 0; l1 < 2; l1 = l1 + 1) {
			dsp->iRec1[l1] = 0;
		}
	}
}

void instanceConstantsmydsp(mydsp* dsp, int sample_rate) {
	dsp->fSampleRate = sample_rate;
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
	ui_interface->openVerticalBox(ui_interface->uiInterface, "f12_two_generators");
	ui_interface->closeBox(ui_interface->uiInterface);
}

void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** RESTRICT inputs, FAUSTFLOAT** RESTRICT outputs) {
	FAUSTFLOAT* output0 = outputs[0];
	FAUSTFLOAT* output1 = outputs[1];
	/* C99 loop */
	{
		int i0;
		for (i0 = 0; i0 < count; i0 = i0 + 1) {
			dsp->iRec1[0] = dsp->iRec1[1] + 1;
			output0[i0] = (FAUSTFLOAT)(ftbl0mydspSIG0[faustmaxi(0, faustmini(dsp->iRec1[1] % 64, 63))]);
			output1[i0] = (FAUSTFLOAT)(ftbl1mydspSIG1[faustmaxi(0, faustmini(dsp->iRec1[1] % 32, 31))]);
			dsp->iRec1[1] = dsp->iRec1[0];
		}
	}
}

#ifdef __cplusplus
}
#endif

#endif
