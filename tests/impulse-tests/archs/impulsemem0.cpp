#ifndef FAUSTFLOAT
#define FAUSTFLOAT double
#endif

#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include "faust_minimal.h"
#include "faust_mem0.h"

#ifndef FAUSTCLASS
#define FAUSTCLASS mydsp
#endif

<<includeIntrinsic>>
<<includeclass>>

class Mem0CppDsp {
    AuditCppMemoryManager fManager;
    mydsp* fDSP;

   public:
    Mem0CppDsp() : fDSP(nullptr)
    {
        mydsp::fManager = &fManager;
        mydsp::memoryInfo();
        mydsp::classInit(44100);
        fDSP = mydsp::create();
        if (!fDSP) std::abort();
    }

    ~Mem0CppDsp()
    {
        mydsp::destroy(fDSP);
        mydsp::classDestroy();
        mydsp::fManager = nullptr;
        fManager.verify();
    }

    void init(int sampleRate) { fDSP->init(sampleRate); }
    int getNumInputs() { return fDSP->getNumInputs(); }
    int getNumOutputs() { return fDSP->getNumOutputs(); }
    void buildUserInterface(UI* ui) { fDSP->buildUserInterface(ui); }
    void compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs)
    {
        fDSP->compute(count, inputs, outputs);
    }
};

#undef FAUSTCLASS
#define FAUSTCLASS Mem0CppDsp
#include "impulseexecopts_driver.h"
