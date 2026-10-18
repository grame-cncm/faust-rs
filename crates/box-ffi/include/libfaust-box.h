#ifndef LIBFAUST_BOX_H
#define LIBFAUST_BOX_H

/*
 * C++ convenience interface for the libfaust Box API.
 *
 * The reference Faust C++ header exposes named C++ functions over the same tree
 * API. This Rust-port header keeps that shape as thin inline wrappers over
 * `libfaust-box-c.h`, preserving the C ABI as the single implementation
 * boundary.
 */

#include "libfaust-box-c.h"

#ifdef __cplusplus
#include <string>

/* Common helpers shared with libfaust-signal.h. */
#ifndef LIBFAUST_COMMON_CPP_WRAPPERS_H
#define LIBFAUST_COMMON_CPP_WRAPPERS_H
inline bool isNil(Box b) { return CisNil(b); }
inline const char* tree2str(Box b) { return Ctree2str(b); }
inline void* getUserData(Box b) { return CgetUserData(b); }
#endif
inline int tree2int(Box b) { return Ctree2int(b); }

/* Core Box constructors: constants and block-diagram composition. */
inline Box boxInt(int n) { return CboxInt(n); }
inline Box boxReal(double n) { return CboxReal(n); }
inline Box boxWire() { return CboxWire(); }
inline Box boxCut() { return CboxCut(); }
inline Box boxSeq(Box x, Box y) { return CboxSeq(x, y); }
inline Box boxPar(Box x, Box y) { return CboxPar(x, y); }
inline Box boxPar3(Box x, Box y, Box z) { return CboxPar3(x, y, z); }
inline Box boxPar4(Box a, Box b, Box c, Box d) { return CboxPar4(a, b, c, d); }
inline Box boxPar5(Box a, Box b, Box c, Box d, Box e) { return CboxPar5(a, b, c, d, e); }
inline Box boxSplit(Box x, Box y) { return CboxSplit(x, y); }
inline Box boxMerge(Box x, Box y) { return CboxMerge(x, y); }
inline Box boxRec(Box x, Box y) { return CboxRec(x, y); }
inline Box boxFad(Box exp, Box seed) { return CboxFad(exp, seed); }
inline Box boxRad(Box exp, Box seed) { return CboxRad(exp, seed); }
inline Box boxRoute(Box n, Box m, Box r) { return CboxRoute(n, m, r); }

/*
 * Primitive Box constructors. As in the reference header, each primitive has
 * an unapplied form (`boxDelay()`) and an applied overload of the same name
 * (`boxDelay(b, d)`). The applied forms are also available under the C API
 * spelling, `boxDelayAux(b, d)`, which earlier faust-rs releases offered
 * instead of the overloads.
 */
inline Box boxDelay() { return CboxDelay(); }
inline Box boxDelayAux(Box b, Box d) { return CboxDelayAux(b, d); }
inline Box boxDelay(Box b, Box d) { return CboxDelayAux(b, d); }
inline Box boxIntCast() { return CboxIntCast(); }
inline Box boxIntCastAux(Box b) { return CboxIntCastAux(b); }
inline Box boxIntCast(Box b) { return CboxIntCastAux(b); }
inline Box boxFloatCast() { return CboxFloatCast(); }
inline Box boxFloatCastAux(Box b) { return CboxFloatCastAux(b); }
inline Box boxFloatCast(Box b) { return CboxFloatCastAux(b); }
inline Box boxReadOnlyTable() { return CboxReadOnlyTable(); }
inline Box boxReadOnlyTableAux(Box n, Box i, Box r) { return CboxReadOnlyTableAux(n, i, r); }
inline Box boxReadOnlyTable(Box n, Box i, Box r) { return CboxReadOnlyTableAux(n, i, r); }
inline Box boxWriteReadTable() { return CboxWriteReadTable(); }
inline Box boxWriteReadTableAux(Box n, Box i, Box widx, Box wsig, Box ridx) { return CboxWriteReadTableAux(n, i, widx, wsig, ridx); }
inline Box boxWriteReadTable(Box n, Box i, Box widx, Box wsig, Box ridx) { return CboxWriteReadTableAux(n, i, widx, wsig, ridx); }
inline Box boxWaveform(Box* wf) { return CboxWaveform(wf); }
inline Box boxSoundfile(const char* label, Box chan) { return CboxSoundfile(label, chan); }
inline Box boxSoundfile(const char* label, Box chan, Box part, Box ridx) { return boxSeq(boxPar(part, ridx), CboxSoundfile(label, chan)); }
inline Box boxSoundfile(const std::string& label, Box chan) { return CboxSoundfile(label.c_str(), chan); }
inline Box boxSoundfile(const std::string& label, Box chan, Box part, Box ridx) { return boxSoundfile(label.c_str(), chan, part, ridx); }
inline Box boxSelect2() { return CboxSelect2(); }
inline Box boxSelect2Aux(Box s, Box b1, Box b2) { return CboxSelect2Aux(s, b1, b2); }
inline Box boxSelect2(Box s, Box b1, Box b2) { return CboxSelect2Aux(s, b1, b2); }
inline Box boxSelect3() { return CboxSelect3(); }
inline Box boxSelect3Aux(Box s, Box b1, Box b2, Box b3) { return CboxSelect3Aux(s, b1, b2, b3); }
inline Box boxSelect3(Box s, Box b1, Box b2, Box b3) { return CboxSelect3Aux(s, b1, b2, b3); }
inline Box boxFFun(enum SType r, const char** n, enum SType* a, const char* i, const char* l) { return CboxFFun(r, n, a, i, l); }
inline Box boxFConst(enum SType t, const char* n, const char* i) { return CboxFConst(t, n, i); }
inline Box boxFVar(enum SType t, const char* n, const char* i) { return CboxFVar(t, n, i); }
inline Box boxBinOp(enum SOperator op) { return CboxBinOp(op); }
inline Box boxBinOpAux(enum SOperator op, Box b1, Box b2) { return CboxBinOpAux(op, b1, b2); }
inline Box boxBinOp(enum SOperator op, Box b1, Box b2) { return CboxBinOpAux(op, b1, b2); }

/*
 * Unary and binary math wrappers generated from the C API names: the
 * unapplied form, the applied overload of the same name (the reference
 * spelling), and the applied form under its C spelling (`boxAddAux`).
 */
#define FAUST_BOX_WRAP_0(name) inline Box name() { return C##name(); }
#define FAUST_BOX_WRAP_1(name)                                    \
    inline Box name##Aux(Box x) { return C##name##Aux(x); }       \
    inline Box name(Box x) { return C##name##Aux(x); }
#define FAUST_BOX_WRAP_2(name)                                    \
    inline Box name##Aux(Box x, Box y) { return C##name##Aux(x, y); } \
    inline Box name(Box x, Box y) { return C##name##Aux(x, y); }

FAUST_BOX_WRAP_0(boxAdd) FAUST_BOX_WRAP_2(boxAdd)
FAUST_BOX_WRAP_0(boxSub) FAUST_BOX_WRAP_2(boxSub)
FAUST_BOX_WRAP_0(boxMul) FAUST_BOX_WRAP_2(boxMul)
FAUST_BOX_WRAP_0(boxDiv) FAUST_BOX_WRAP_2(boxDiv)
FAUST_BOX_WRAP_0(boxRem) FAUST_BOX_WRAP_2(boxRem)
FAUST_BOX_WRAP_0(boxLeftShift) FAUST_BOX_WRAP_2(boxLeftShift)
FAUST_BOX_WRAP_0(boxLRightShift) FAUST_BOX_WRAP_2(boxLRightShift)
FAUST_BOX_WRAP_0(boxARightShift) FAUST_BOX_WRAP_2(boxARightShift)
FAUST_BOX_WRAP_0(boxGT) FAUST_BOX_WRAP_2(boxGT)
FAUST_BOX_WRAP_0(boxLT) FAUST_BOX_WRAP_2(boxLT)
FAUST_BOX_WRAP_0(boxGE) FAUST_BOX_WRAP_2(boxGE)
FAUST_BOX_WRAP_0(boxLE) FAUST_BOX_WRAP_2(boxLE)
FAUST_BOX_WRAP_0(boxEQ) FAUST_BOX_WRAP_2(boxEQ)
FAUST_BOX_WRAP_0(boxNE) FAUST_BOX_WRAP_2(boxNE)
FAUST_BOX_WRAP_0(boxAND) FAUST_BOX_WRAP_2(boxAND)
FAUST_BOX_WRAP_0(boxOR) FAUST_BOX_WRAP_2(boxOR)
FAUST_BOX_WRAP_0(boxXOR) FAUST_BOX_WRAP_2(boxXOR)
FAUST_BOX_WRAP_0(boxAbs) FAUST_BOX_WRAP_1(boxAbs)
FAUST_BOX_WRAP_0(boxAcos) FAUST_BOX_WRAP_1(boxAcos)
FAUST_BOX_WRAP_0(boxTan) FAUST_BOX_WRAP_1(boxTan)
FAUST_BOX_WRAP_0(boxSqrt) FAUST_BOX_WRAP_1(boxSqrt)
FAUST_BOX_WRAP_0(boxSin) FAUST_BOX_WRAP_1(boxSin)
FAUST_BOX_WRAP_0(boxRint) FAUST_BOX_WRAP_1(boxRint)
FAUST_BOX_WRAP_0(boxRound) FAUST_BOX_WRAP_1(boxRound)
FAUST_BOX_WRAP_0(boxLog) FAUST_BOX_WRAP_1(boxLog)
FAUST_BOX_WRAP_0(boxLog10) FAUST_BOX_WRAP_1(boxLog10)
FAUST_BOX_WRAP_0(boxFloor) FAUST_BOX_WRAP_1(boxFloor)
FAUST_BOX_WRAP_0(boxExp) FAUST_BOX_WRAP_1(boxExp)
FAUST_BOX_WRAP_0(boxExp10) FAUST_BOX_WRAP_1(boxExp10)
FAUST_BOX_WRAP_0(boxCos) FAUST_BOX_WRAP_1(boxCos)
FAUST_BOX_WRAP_0(boxCeil) FAUST_BOX_WRAP_1(boxCeil)
FAUST_BOX_WRAP_0(boxAtan) FAUST_BOX_WRAP_1(boxAtan)
FAUST_BOX_WRAP_0(boxAsin) FAUST_BOX_WRAP_1(boxAsin)
FAUST_BOX_WRAP_0(boxRemainder) FAUST_BOX_WRAP_2(boxRemainder)
FAUST_BOX_WRAP_0(boxPow) FAUST_BOX_WRAP_2(boxPow)
FAUST_BOX_WRAP_0(boxMin) FAUST_BOX_WRAP_2(boxMin)
FAUST_BOX_WRAP_0(boxMax) FAUST_BOX_WRAP_2(boxMax)
FAUST_BOX_WRAP_0(boxFmod) FAUST_BOX_WRAP_2(boxFmod)
FAUST_BOX_WRAP_0(boxAtan2) FAUST_BOX_WRAP_2(boxAtan2)

#undef FAUST_BOX_WRAP_0
#undef FAUST_BOX_WRAP_1
#undef FAUST_BOX_WRAP_2

/* UI, grouping, and attachment wrappers. */
inline Box boxButton(const char* label) { return CboxButton(label); }
inline Box boxCheckbox(const char* label) { return CboxCheckbox(label); }
inline Box boxVSlider(const char* l, Box i, Box mn, Box mx, Box st) { return CboxVSlider(l, i, mn, mx, st); }
inline Box boxHSlider(const char* l, Box i, Box mn, Box mx, Box st) { return CboxHSlider(l, i, mn, mx, st); }
inline Box boxNumEntry(const char* l, Box i, Box mn, Box mx, Box st) { return CboxNumEntry(l, i, mn, mx, st); }
inline Box boxVBargraph(const char* l, Box mn, Box mx) { return CboxVBargraph(l, mn, mx); }
inline Box boxVBargraphAux(const char* l, Box mn, Box mx, Box x) { return CboxVBargraphAux(l, mn, mx, x); }
inline Box boxVBargraph(const char* l, Box mn, Box mx, Box x) { return CboxVBargraphAux(l, mn, mx, x); }
inline Box boxHBargraph(const char* l, Box mn, Box mx) { return CboxHBargraph(l, mn, mx); }
inline Box boxHBargraphAux(const char* l, Box mn, Box mx, Box x) { return CboxHBargraphAux(l, mn, mx, x); }
inline Box boxHBargraph(const char* l, Box mn, Box mx, Box x) { return CboxHBargraphAux(l, mn, mx, x); }
inline Box boxVGroup(const char* l, Box g) { return CboxVGroup(l, g); }
inline Box boxHGroup(const char* l, Box g) { return CboxHGroup(l, g); }
inline Box boxTGroup(const char* l, Box g) { return CboxTGroup(l, g); }
inline Box boxAttach() { return CboxAttach(); }
inline Box boxAttachAux(Box b1, Box b2) { return CboxAttachAux(b1, b2); }
inline Box boxAttach(Box b1, Box b2) { return CboxAttachAux(b1, b2); }

/* Structural Box matchers. Output references receive context-owned handles. */
inline bool isBoxAbstr(Box t, Box& x, Box& y) { return CisBoxAbstr(t, &x, &y); }
inline bool isBoxAccess(Box t, Box& e, Box& id) { return CisBoxAccess(t, &e, &id); }
inline bool isBoxAppl(Box t, Box& x, Box& y) { return CisBoxAppl(t, &x, &y); }
inline bool isBoxButton(Box b, Box& l) { return CisBoxButton(b, &l); }
inline bool isBoxCase(Box b, Box& r) { return CisBoxCase(b, &r); }
inline bool isBoxCheckbox(Box b, Box& l) { return CisBoxCheckbox(b, &l); }
inline bool isBoxComponent(Box b, Box& f) { return CisBoxComponent(b, &f); }
inline bool isBoxCut(Box b) { return CisBoxCut(b); }
inline bool isBoxEnvironment(Box b) { return CisBoxEnvironment(b); }
inline bool isBoxIdent(Box b, const char** s) { return CisBoxIdent(b, s); }
inline bool isBoxInt(Box b, int* v) { return CisBoxInt(b, v); }
inline bool isBoxReal(Box b, double* v) { return CisBoxReal(b, v); }
inline bool isBoxWire(Box b) { return CisBoxWire(b); }

/* Printing helpers copy C-allocated strings into std::string and free them. */
inline std::string printBox(Box b, bool shared = true, int max_size = 4096) {
    char* raw = CprintBox(b, shared, max_size);
    if (!raw) {
        return std::string();
    }
    std::string out(raw);
    freeCMemory(raw);
    return out;
}

inline std::string printSignal(Signal s, bool shared = true, int max_size = 4096) {
    char* raw = CprintSignal(s, shared, max_size);
    if (!raw) {
        return std::string();
    }
    std::string out(raw);
    freeCMemory(raw);
    return out;
}

#endif

#endif
