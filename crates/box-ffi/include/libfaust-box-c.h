#ifndef LIBFAUST_BOX_C_H
#define LIBFAUST_BOX_C_H

/*
 * C interface for the libfaust Box API.
 *
 * This header mirrors the Faust C++ `architecture/faust/dsp/libfaust-box-c.h`
 * surface maintained by GRAME, adapted to the Rust port's unified `faust-ffi`
 * library. Box and Signal values are opaque handles owned by the process-global
 * libfaust context unless explicitly documented otherwise.
 */

#include <stdbool.h>

#ifndef LIBFAUST_TREE_C_TYPES_H
#define LIBFAUST_TREE_C_TYPES_H

/* Opaque tree node used by both Box and Signal APIs. */
#ifdef _MSC_VER
typedef void CTree;
#else
typedef struct CTree CTree;
#endif

typedef CTree* Signal;
typedef CTree* Box;

/* Scalar type and primitive binary-operator tags shared with the Signal API. */
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
 * createLibContext() must be called before using the Box/Signal constructors
 * and paired with destroyLibContext() when the process is done with the API.
 * Strings and arrays returned by this API remain caller-owned and must be
 * released with freeCMemory() when documented as returned allocations.
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
bool CisNil(Box b);
const char* Ctree2str(Box b);
int Ctree2int(Box b);
void* CgetUserData(Box b);

/* Core Box constructors: constants and block-diagram composition. */
Box CboxInt(int n);
Box CboxReal(double n);
Box CboxWire(void);
Box CboxCut(void);
Box CboxSeq(Box x, Box y);
Box CboxPar(Box x, Box y);
Box CboxPar3(Box x, Box y, Box z);
Box CboxPar4(Box a, Box b, Box c, Box d);
Box CboxPar5(Box a, Box b, Box c, Box d, Box e);
Box CboxSplit(Box x, Box y);
Box CboxMerge(Box x, Box y);
Box CboxRec(Box x, Box y);
Box CboxRoute(Box n, Box m, Box r);

// Forward/reverse automatic differentiation (faust-rs extension).
Box CboxFad(Box exp, Box seed);
Box CboxRad(Box exp, Box seed);

/* Signal-processing primitive boxes and their applied Aux forms. */
Box CboxDelay(void);
Box CboxDelayAux(Box b, Box del);
Box CboxIntCast(void);
Box CboxIntCastAux(Box b);
Box CboxFloatCast(void);
Box CboxFloatCastAux(Box b);

Box CboxReadOnlyTable(void);
Box CboxReadOnlyTableAux(Box n, Box init, Box ridx);
Box CboxWriteReadTable(void);
Box CboxWriteReadTableAux(Box n, Box init, Box widx, Box wsig, Box ridx);

Box CboxWaveform(Box* wf);
Box CboxSoundfile(const char* label, Box chan);

Box CboxSelect2(void);
Box CboxSelect2Aux(Box selector, Box b1, Box b2);
Box CboxSelect3(void);
Box CboxSelect3Aux(Box selector, Box b1, Box b2, Box b3);

Box CboxFFun(enum SType rtype, const char** names, enum SType* atypes,
             const char* incfile, const char* libfile);
Box CboxFConst(enum SType type, const char* name, const char* incfile);
Box CboxFVar(enum SType type, const char* name, const char* incfile);

/* Primitive binary operator constructors. */
Box CboxBinOp(enum SOperator op);
Box CboxBinOpAux(enum SOperator op, Box b1, Box b2);

Box CboxAdd(void); Box CboxAddAux(Box b1, Box b2);
Box CboxSub(void); Box CboxSubAux(Box b1, Box b2);
Box CboxMul(void); Box CboxMulAux(Box b1, Box b2);
Box CboxDiv(void); Box CboxDivAux(Box b1, Box b2);
Box CboxRem(void); Box CboxRemAux(Box b1, Box b2);
Box CboxLeftShift(void); Box CboxLeftShiftAux(Box b1, Box b2);
Box CboxLRightShift(void); Box CboxLRightShiftAux(Box b1, Box b2);
Box CboxARightShift(void); Box CboxARightShiftAux(Box b1, Box b2);
Box CboxGT(void); Box CboxGTAux(Box b1, Box b2);
Box CboxLT(void); Box CboxLTAux(Box b1, Box b2);
Box CboxGE(void); Box CboxGEAux(Box b1, Box b2);
Box CboxLE(void); Box CboxLEAux(Box b1, Box b2);
Box CboxEQ(void); Box CboxEQAux(Box b1, Box b2);
Box CboxNE(void); Box CboxNEAux(Box b1, Box b2);
Box CboxAND(void); Box CboxANDAux(Box b1, Box b2);
Box CboxOR(void); Box CboxORAux(Box b1, Box b2);
Box CboxXOR(void); Box CboxXORAux(Box b1, Box b2);

Box CboxAbs(void); Box CboxAbsAux(Box x);
Box CboxAcos(void); Box CboxAcosAux(Box x);
Box CboxTan(void); Box CboxTanAux(Box x);
Box CboxSqrt(void); Box CboxSqrtAux(Box x);
Box CboxSin(void); Box CboxSinAux(Box x);
Box CboxRint(void); Box CboxRintAux(Box x);
Box CboxRound(void); Box CboxRoundAux(Box x);
Box CboxLog(void); Box CboxLogAux(Box x);
Box CboxLog10(void); Box CboxLog10Aux(Box x);
Box CboxFloor(void); Box CboxFloorAux(Box x);
Box CboxExp(void); Box CboxExpAux(Box x);
Box CboxExp10(void); Box CboxExp10Aux(Box x);
Box CboxCos(void); Box CboxCosAux(Box x);
Box CboxCeil(void); Box CboxCeilAux(Box x);
Box CboxAtan(void); Box CboxAtanAux(Box x);
Box CboxAsin(void); Box CboxAsinAux(Box x);

Box CboxRemainder(void); Box CboxRemainderAux(Box b1, Box b2);
Box CboxPow(void); Box CboxPowAux(Box b1, Box b2);
Box CboxMin(void); Box CboxMinAux(Box b1, Box b2);
Box CboxMax(void); Box CboxMaxAux(Box b1, Box b2);
Box CboxFmod(void); Box CboxFmodAux(Box b1, Box b2);
Box CboxAtan2(void); Box CboxAtan2Aux(Box b1, Box b2);

/* UI, grouping, and attachment Box constructors. */
Box CboxButton(const char* label);
Box CboxCheckbox(const char* label);
Box CboxVSlider(const char* label, Box init, Box min, Box max, Box step);
Box CboxHSlider(const char* label, Box init, Box min, Box max, Box step);
Box CboxNumEntry(const char* label, Box init, Box min, Box max, Box step);
Box CboxVBargraph(const char* label, Box min, Box max);
Box CboxVBargraphAux(const char* label, Box min, Box max, Box x);
Box CboxHBargraph(const char* label, Box min, Box max);
Box CboxHBargraphAux(const char* label, Box min, Box max, Box x);
Box CboxVGroup(const char* label, Box group);
Box CboxHGroup(const char* label, Box group);
Box CboxTGroup(const char* label, Box group);
Box CboxAttach(void);
Box CboxAttachAux(Box b1, Box b2);

/*
 * Structural Box predicates.
 *
 * Functions return true when the input matches the requested Box family and
 * write matched children into the non-null output parameters. Output handles
 * remain owned by the libfaust context.
 */
bool CisBoxAbstr(Box t, Box* x, Box* y);
bool CisBoxAccess(Box t, Box* exp, Box* id);
bool CisBoxAppl(Box t, Box* x, Box* y);
bool CisBoxButton(Box b, Box* lbl);
bool CisBoxCase(Box b, Box* rules);
bool CisBoxCheckbox(Box b, Box* lbl);
bool CisBoxComponent(Box b, Box* filename);
bool CisBoxCut(Box t);
bool CisBoxEnvironment(Box b);
bool CisBoxError(Box t);
bool CisBoxFConst(Box b, Box* type, Box* name, Box* file);
bool CisBoxFFun(Box b, Box* ff);
bool CisBoxFVar(Box b, Box* type, Box* name, Box* file);
bool CisBoxHBargraph(Box b, Box* lbl, Box* min, Box* max);
bool CisBoxHGroup(Box b, Box* lbl, Box* x);
bool CisBoxHSlider(Box b, Box* lbl, Box* cur, Box* min, Box* max, Box* step);
bool CisBoxIdent(Box t, const char** str);
bool CisBoxInputs(Box t, Box* x);
bool CisBoxInt(Box t, int* i);
bool CisBoxIPar(Box t, Box* x, Box* y, Box* z);
bool CisBoxIProd(Box t, Box* x, Box* y, Box* z);
bool CisBoxISeq(Box t, Box* x, Box* y, Box* z);
bool CisBoxISum(Box t, Box* x, Box* y, Box* z);
bool CisBoxLibrary(Box b, Box* filename);
bool CisBoxMerge(Box t, Box* x, Box* y);
bool CisBoxMetadata(Box b, Box* exp, Box* mdlist);
bool CisBoxNumEntry(Box b, Box* lbl, Box* cur, Box* min, Box* max, Box* step);
bool CisBoxOutputs(Box t, Box* x);
bool CisBoxPar(Box t, Box* x, Box* y);
bool CisBoxPatternMatcher(Box b);
bool CisBoxPatternVar(Box b, Box* id);
bool CisBoxPrim0(Box b);
bool CisBoxPrim1(Box b);
bool CisBoxPrim2(Box b);
bool CisBoxPrim3(Box b);
bool CisBoxPrim4(Box b);
bool CisBoxPrim5(Box b);
bool CisBoxReal(Box t, double* r);
bool CisBoxRec(Box t, Box* x, Box* y);
bool CisBoxRoute(Box b, Box* n, Box* m, Box* r);
bool CisBoxSeq(Box t, Box* x, Box* y);
bool CisBoxSlot(Box t, int* id);
bool CisBoxSoundfile(Box b, Box* label, Box* chan);
bool CisBoxSplit(Box t, Box* x, Box* y);
bool CisBoxSymbolic(Box t, Box* slot, Box* body);
bool CisBoxTGroup(Box b, Box* lbl, Box* x);
bool CisBoxVBargraph(Box b, Box* lbl, Box* min, Box* max);
bool CisBoxVGroup(Box b, Box* lbl, Box* x);
bool CisBoxVSlider(Box b, Box* lbl, Box* cur, Box* min, Box* max, Box* step);
bool CisBoxWaveform(Box b);
bool CisBoxWire(Box t);
bool CisBoxWithLocalDef(Box t, Box* body, Box* ldef);

/*
 * Front-end and lowering helpers.
 *
 * CDSPToBoxes parses Faust source into a Box tree and reports input/output
 * arity. CboxesToSignals* lower Box trees to null-terminated Signal arrays
 * released with freeCMemory(). CcreateSourceFromBoxes returns generated source
 * text released with freeCMemory().
 */
Box CDSPToBoxes(const char* name_app, const char* dsp_content, int argc,
                const char* argv[], int* inputs, int* outputs, char* error_msg);
bool CgetBoxType(Box box, int* inputs, int* outputs);
Signal* CboxesToSignals(Box box, char* error_msg);
Signal* CboxesToSignals2(Box box, char* error_msg);
char* CcreateSourceFromBoxes(const char* name_app, Box box, const char* lang, int argc,
                             const char* argv[], char* error_msg);

/**
 * Return the complete text of the last error reported on the calling thread
 * through an `error_msg` buffer of this header: the message that buffer
 * received, not cut at 4096 bytes, followed, for a CDSPToBoxes
 * failure, by the compiler's rendered diagnostics (location, source snippet,
 * notes, fixes).
 *
 * An addition of the Rust port: the reference libfaust has no equivalent. Same
 * contract as getCCompleteDSPError (libfaust-c.h) and the backends' entry
 * points. The pointer is owned by the library: do NOT free it, not with
 * freeCMemory either. It is NULL while no error was reported on this thread and
 * stays valid until the next error reported on this thread. A successful call
 * does not reset it: read it after a call that failed.
 */
const char* getCCompleteBoxError(void);

/**
 * Return the typed form of the last error reported on the calling thread
 * through an `error_msg` buffer of the Box API: the compiler's diagnostics-v2 JSON report.
 * For each diagnostic: its `code` (FRS-...), its `labels` with byte ranges in
 * each of `sources[]`, its `facts`, `notes` and `help`, and its `fixes`, each
 * with its `edits` (range, replacement) and its `applicability`, so that a
 * machine-applicable fix is applied without reading the rendered text.
 *
 * An addition of this port: the reference libfaust has no equivalent.
 * CDSPToBoxes is the entry point of this API that compiles a source, and the
 * one whose failures have a report.
 *
 * It is NULL when that error carried no typed diagnostics (an argument error),
 * even if an earlier one did: a report never outlives the failure it
 * describes. Otherwise the contract of the complete text above: owned by the
 * library (do NOT free it), per thread, valid until the next error reported on
 * this thread, not reset by a successful call.
 *
 * The document carries its own `schema_version` (2 today); fields may be added
 * within a version, so read what you know and check the version rather than
 * assuming it. `request.backend` names the surface that failed ("box").
 */
const char* getCBoxErrorDiagnostics(void);

#ifdef __cplusplus
}
#endif

#endif
