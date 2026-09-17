#include "cranelift-dsp-c.h"

int main(void) {
    cranelift_dsp_factory* factory = 0;
    cranelift_dsp* dsp = 0;
    char* str = 0;
    char* arr0 = 0;
    char** arr = &arr0;
    char err[4096];
    (void)getCLibFaustVersion();
    (void)startMTDSPFactories();
    stopMTDSPFactories();
    freeCMemory(0);
    (void)getAllCCraneliftDSPFactories();
    (void)getCCompleteCraneliftDSPFactoryError();
    (void)getCCraneliftDSPFactoryJSON(factory);
    (void)getCCraneliftDSPFactoryLibraryList(factory);
    (void)writeCCraneliftDSPFactoryToBitcode(factory);
    (void)readCCraneliftDSPFactoryFromBitcode((const char*)str, err);
    (void)writeCCraneliftDSPFactoryToBitcodeFile(factory, (const char*)str);
    (void)readCCraneliftDSPFactoryFromBitcodeFile((const char*)str, err);
    (void)createCCraneliftDSPInstance(factory);
    deleteCCraneliftDSPInstance(dsp);
    (void)arr;
    return 0;
}
