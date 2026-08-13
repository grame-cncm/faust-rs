/* C-only architecture declarations injected before raw generated C. */
#ifndef FAUST_RS_MEM0_C_API_H
#define FAUST_RS_MEM0_C_API_H

#ifndef FAUSTFLOAT
#define FAUSTFLOAT double
#endif

#include "../../../crates/ffi-common/include/faust-memory-manager.h"

typedef struct Soundfile Soundfile;

typedef struct {
    void* metaInterface;
    void (*declare)(void* metaInterface, const char* key, const char* value);
} MetaGlue;

typedef struct {
    void* uiInterface;
    void (*openTabBox)(void*, const char*);
    void (*openHorizontalBox)(void*, const char*);
    void (*openVerticalBox)(void*, const char*);
    void (*closeBox)(void*);
    void (*addButton)(void*, const char*, FAUSTFLOAT*);
    void (*addCheckButton)(void*, const char*, FAUSTFLOAT*);
    void (*addVerticalSlider)(void*, const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT,
                              FAUSTFLOAT);
    void (*addHorizontalSlider)(void*, const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT,
                                FAUSTFLOAT, FAUSTFLOAT);
    void (*addNumEntry)(void*, const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT,
                        FAUSTFLOAT);
    void (*addHorizontalBargraph)(void*, const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT);
    void (*addVerticalBargraph)(void*, const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT);
    void (*addSoundfile)(void*, const char*, const char*, Soundfile**);
    void (*declare)(void*, FAUSTFLOAT*, const char*, const char*);
} UIGlue;

#endif
