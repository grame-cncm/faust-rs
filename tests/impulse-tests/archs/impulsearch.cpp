// faust-rs copy of the C++ Faust tests/impulse-tests/archs/impulsearch.cpp
// (identical at 8eebea429 and on master-dev), for the C++ (`make cpp`) lane and
// for reference generation (`Make.ref`). The one addition is
// faustrs_soundfile_fixture.h after controlTools.h: the per-channel
// soundfile fixture the `sound` reference needs, whatever C++ checkout
// CPP_TESTS points at. See tests/impulse-tests/README.md, Requirements.

#ifndef FAUSTFLOAT
#define FAUSTFLOAT double
#endif

#include "controlTools.h"
#include "faustrs_soundfile_fixture.h"

//----------------------------------------------------------------------------
//FAUST generated code
//----------------------------------------------------------------------------

<<includeIntrinsic>>

<<includeclass>>

int main(int argc, char* argv[])
{
    int linenum = 0;
    int nbsamples = 60000;
    
    // print general informations
    printHeader(new mydsp(), nbsamples);
    
    // linenum is incremented in runDSP and runPolyDSP
    runDSP(new mydsp(), argv[0], linenum, nbsamples/4);
    runDSP(new mydsp(), argv[0], linenum, nbsamples/4, false, true);
    runPolyDSP(new mydsp(), linenum, nbsamples/4, 4);
    runPolyDSP(new mydsp(), linenum, nbsamples/4, 1);
    
    return 0;
}
