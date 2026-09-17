#ifndef LIBFAUST_SIGNAL_C_H
#define LIBFAUST_SIGNAL_C_H

/*
 * C interface for the libfaust Signal API.
 *
 * This header mirrors the Faust C++ `architecture/faust/dsp/libfaust-signal-c.h`
 * surface maintained by GRAME, adapted to the Rust port's unified `faust-ffi`
 * library. Signal and Box values are opaque tree handles owned by the
 * process-global libfaust context unless explicitly documented otherwise.
 */

#include <stdbool.h>
#include <stdint.h>

#ifndef LIBFAUST_TREE_C_TYPES_H
#define LIBFAUST_TREE_C_TYPES_H

/* Opaque tree node used by both Signal and Box APIs. */
#ifdef _MSC_VER
typedef void CTree;
#else
typedef struct CTree CTree;
#endif

typedef CTree* Signal;
typedef CTree* Box;

/* Scalar type and primitive binary-operator tags shared with the Box API. */
enum SType { kSInt, kSReal };
enum SOperator {
    kAdd,
    kSub,
    kMul,
    kDiv,
    kRem,
    kLsh,
    kARsh,
    kLRsh,
    kGT,
    kLT,
    kGE,
    kLE,
    kEQ,
    kNE,
    kAND,
    kOR,
    kXOR
};

#endif

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Context and memory management.
 *
 * createLibContext() must be called before using constructors and paired with
 * destroyLibContext(). Strings and arrays returned by this API remain
 * caller-owned and must be released with freeCMemory() when documented as
 * returned allocations.
 */
void createLibContext(void);
void destroyLibContext(void);
void freeCMemory(void* ptr);

/*
 * Tree printing helpers. Returned strings are allocated by the library and must
 * be released with freeCMemory().
 */
char* CprintBox(Box box, bool shared, int max_size);
char* CprintSignal(Signal sig, bool shared, int max_size);

/* Generic tree helpers. */
bool CisNil(Signal s);
const char* Ctree2str(Signal s);
void* CgetUserData(Signal s);

/* Core Signal constructors: constants, inputs, delays, casts, and tables. */
Signal CsigInt(int n);
Signal CsigInt64(int64_t n);
Signal CsigReal(double n);
Signal CsigInput(int idx);
Signal CsigDelay(Signal s, Signal del);
Signal CsigDelay1(Signal s);
Signal CsigIntCast(Signal s);
Signal CsigFloatCast(Signal s);
Signal CsigReadOnlyTable(Signal n, Signal init, Signal ridx);
Signal CsigWriteReadTable(Signal n, Signal init, Signal widx, Signal wsig, Signal ridx);
Signal CsigWaveform(Signal* wf);

/* Soundfile selectors and buffer accessors. */
Signal CsigSoundfile(const char* label);
Signal CsigSoundfileLength(Signal sf, Signal part);
Signal CsigSoundfileRate(Signal sf, Signal part);
Signal CsigSoundfileBuffer(Signal sf, Signal chan, Signal part, Signal ridx);

/* Signal selection, foreign values/functions, and primitive operators. */
Signal CsigSelect2(Signal selector, Signal s1, Signal s2);
Signal CsigSelect3(Signal selector, Signal s1, Signal s2, Signal s3);
Signal CsigFFun(enum SType rtype, const char** names, enum SType* atypes,
                 const char* incfile, const char* libfile, Signal* largs);
Signal CsigFConst(enum SType type, const char* name, const char* file);
Signal CsigFVar(enum SType type, const char* name, const char* file);
Signal CsigBinOp(enum SOperator op, Signal x, Signal y);
Signal CsigAdd(Signal x, Signal y);
Signal CsigSub(Signal x, Signal y);
Signal CsigMul(Signal x, Signal y);
Signal CsigDiv(Signal x, Signal y);
Signal CsigRem(Signal x, Signal y);
Signal CsigLeftShift(Signal x, Signal y);
Signal CsigLRightShift(Signal x, Signal y);
Signal CsigARightShift(Signal x, Signal y);
Signal CsigGT(Signal x, Signal y);
Signal CsigLT(Signal x, Signal y);
Signal CsigGE(Signal x, Signal y);
Signal CsigLE(Signal x, Signal y);
Signal CsigEQ(Signal x, Signal y);
Signal CsigNE(Signal x, Signal y);
Signal CsigAND(Signal x, Signal y);
Signal CsigOR(Signal x, Signal y);
Signal CsigXOR(Signal x, Signal y);
Signal CsigAbs(Signal x);
Signal CsigAcos(Signal x);
Signal CsigTan(Signal x);
Signal CsigSqrt(Signal x);
Signal CsigSin(Signal x);
Signal CsigRint(Signal x);
Signal CsigLog(Signal x);
Signal CsigLog10(Signal x);
Signal CsigFloor(Signal x);
Signal CsigExp(Signal x);
Signal CsigExp10(Signal x);
Signal CsigCos(Signal x);
Signal CsigCeil(Signal x);
Signal CsigAtan(Signal x);
Signal CsigAsin(Signal x);
Signal CsigRemainder(Signal x, Signal y);
Signal CsigPow(Signal x, Signal y);
Signal CsigMin(Signal x, Signal y);
Signal CsigMax(Signal x, Signal y);
Signal CsigFmod(Signal x, Signal y);
Signal CsigAtan2(Signal x, Signal y);

/* Recursion constructors. */
Signal CsigSelf(void);
Signal CsigSelfN(int id);
Signal CsigRecursion(Signal s);
Signal* CsigRecursionN(Signal* rf);

/* UI and attachment Signal constructors. */
Signal CsigButton(const char* label);
Signal CsigCheckbox(const char* label);
Signal CsigVSlider(const char* label, Signal init, Signal min, Signal max, Signal step);
Signal CsigHSlider(const char* label, Signal init, Signal min, Signal max, Signal step);
Signal CsigNumEntry(const char* label, Signal init, Signal min, Signal max, Signal step);
Signal CsigVBargraph(const char* label, Signal min, Signal max, Signal s);
Signal CsigHBargraph(const char* label, Signal min, Signal max, Signal s);
Signal CsigAttach(Signal s0, Signal s1);

/*
 * Structural Signal predicates.
 *
 * Functions return true when the input matches the requested Signal family and
 * write matched children into the non-null output parameters. Output handles
 * remain owned by the libfaust context. Unsupported/documentation-only families
 * are exposed for reference API parity and return deterministic false until the
 * Rust IR grows matching nodes.
 */
bool CisSigInt(Signal t, int* i);
bool CisSigInt64(Signal t, int64_t* i);
bool CisSigReal(Signal t, double* r);
bool CisSigInput(Signal t, int* i);
bool CisSigOutput(Signal t, int* i, Signal* t0);
bool CisSigDelay1(Signal t, Signal* t0);
bool CisSigDelay(Signal t, Signal* t0, Signal* t1);
bool CisSigPrefix(Signal t, Signal* t0, Signal* t1);
bool CisSigRDTbl(Signal s, Signal* t, Signal* i);
bool CisSigWRTbl(Signal u, Signal* id, Signal* t, Signal* i, Signal* s);
bool CisSigGen(Signal t, Signal* x);
bool CisSigGen1(Signal t);
bool CisSigSelect2(Signal t, Signal* selector, Signal* s1, Signal* s2);
bool CisSigAssertBounds(Signal t, Signal* s1, Signal* s2, Signal* s3);
bool CisSigHighest(Signal s, Signal* x);
bool CisSigLowest(Signal s, Signal* x);
bool CisSigBinOp(Signal s, int* op, Signal* x, Signal* y);
bool CisSigIntCast(Signal s, Signal* x);
bool CisSigFloatCast(Signal s, Signal* x);
bool CisSigFFun(Signal s, Signal* ff, Signal* largs);
bool CisSigFConst(Signal s, Signal* type, Signal* name, Signal* file);
bool CisSigFVar(Signal s, Signal* type, Signal* name, Signal* file);
bool CisProj(Signal s, int* i, Signal* rgroup);
bool CisRec(Signal s, Signal* var, Signal* body);
bool CisSigButton(Signal s, Signal* lbl);
bool CisSigCheckbox(Signal s, Signal* lbl);
bool CisSigWaveform(Signal s);
bool CisSigHSlider(Signal s, Signal* lbl, Signal* init, Signal* min, Signal* max, Signal* step);
bool CisSigVSlider(Signal s, Signal* lbl, Signal* init, Signal* min, Signal* max, Signal* step);
bool CisSigNumEntry(Signal s, Signal* lbl, Signal* init, Signal* min, Signal* max, Signal* step);
bool CisSigHBargraph(Signal s, Signal* lbl, Signal* min, Signal* max, Signal* x);
bool CisSigVBargraph(Signal s, Signal* lbl, Signal* min, Signal* max, Signal* x);
bool CisSigAttach(Signal s, Signal* s0, Signal* s1);
bool CisSigEnable(Signal s, Signal* s0, Signal* s1);
bool CisSigControl(Signal s, Signal* s0, Signal* s1);
bool CisSigSoundfile(Signal s, Signal* label);
bool CisSigSoundfileLength(Signal s, Signal* sf, Signal* part);
bool CisSigSoundfileRate(Signal s, Signal* sf, Signal* part);
bool CisSigSoundfileBuffer(Signal s, Signal* sf, Signal* chan, Signal* part, Signal* ridx);
bool CisSigDocConstantTbl(Signal t, Signal* n, Signal* sig);
bool CisSigDocWriteTbl(Signal t, Signal* n, Signal* sig, Signal* widx, Signal* wsig);
bool CisSigDocAccessTbl(Signal t, Signal* tbl, Signal* ridx);

/*
 * Normal-form and source-generation helpers.
 *
 * CsimplifyToNormalForm2 accepts and returns null-terminated Signal arrays.
 * CcreateSourceFromSignals returns generated source text released with
 * freeCMemory().
 */
Signal CsimplifyToNormalForm(Signal s);
Signal* CsimplifyToNormalForm2(Signal* siglist);
char* CcreateSourceFromSignals(const char* name_app, Signal* osigs, const char* lang,
                               int argc, const char* argv[], char* error_msg);

/**
 * Return the complete text of the last error reported on the calling thread
 * through an `error_msg` buffer of this header: the message that buffer
 * received, not cut at 4096 bytes.
 *
 * An addition of the Rust port: the reference libfaust has no equivalent. Same
 * contract as getCCompleteDSPError (libfaust-c.h) and the backends' entry
 * points. The pointer is owned by the library: do NOT free it, not with
 * freeCMemory either. It is NULL while no error was reported on this thread and
 * stays valid until the next error reported on this thread. A successful call
 * does not reset it: read it after a call that failed.
 */
const char* getCCompleteSignalError(void);

#ifdef __cplusplus
}
#endif

#endif
