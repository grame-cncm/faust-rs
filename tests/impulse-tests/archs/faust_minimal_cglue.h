/* Minimal self-contained C glue for the impulse drivers.
 * =====================================================
 *
 * The C backend emits calls through function-pointer structs rather than C++
 * virtuals: `ui_interface->addHorizontalSlider(ui_interface->uiInterface, …)`.
 * This is a faust-rs-owned reimplementation of that contract — the `UIGlue` and
 * `MetaGlue` layouts plus the adapters that forward them onto the C++ [`UI`] and
 * [`Meta`] interfaces of `faust_minimal.h`.
 *
 * Field order and signatures must match what `crates/codegen/src/backends/c`
 * emits. They are a stable contract on that side, so a mismatch is a compile
 * error here rather than a silent behavior change.
 */

#ifndef FAUST_RS_MINIMAL_CGLUE_H
#define FAUST_RS_MINIMAL_CGLUE_H

#include "faust_minimal.h"

/* ── Metadata glue ──────────────────────────────────────────────────────── */

typedef struct {
    void* metaInterface;
    void (*declare)(void* metaInterface, const char* key, const char* value);
} MetaGlue;

/* ── UI glue ────────────────────────────────────────────────────────────── */

typedef struct {
    void* uiInterface;

    void (*openTabBox)(void* ui_interface, const char* label);
    void (*openHorizontalBox)(void* ui_interface, const char* label);
    void (*openVerticalBox)(void* ui_interface, const char* label);
    void (*closeBox)(void* ui_interface);

    void (*addButton)(void* ui_interface, const char* label, FAUSTFLOAT* zone);
    void (*addCheckButton)(void* ui_interface, const char* label, FAUSTFLOAT* zone);
    void (*addVerticalSlider)(void* ui_interface, const char* label, FAUSTFLOAT* zone,
                              FAUSTFLOAT init, FAUSTFLOAT min, FAUSTFLOAT max, FAUSTFLOAT step);
    void (*addHorizontalSlider)(void* ui_interface, const char* label, FAUSTFLOAT* zone,
                                FAUSTFLOAT init, FAUSTFLOAT min, FAUSTFLOAT max, FAUSTFLOAT step);
    void (*addNumEntry)(void* ui_interface, const char* label, FAUSTFLOAT* zone, FAUSTFLOAT init,
                        FAUSTFLOAT min, FAUSTFLOAT max, FAUSTFLOAT step);

    void (*addHorizontalBargraph)(void* ui_interface, const char* label, FAUSTFLOAT* zone,
                                  FAUSTFLOAT min, FAUSTFLOAT max);
    void (*addVerticalBargraph)(void* ui_interface, const char* label, FAUSTFLOAT* zone,
                                FAUSTFLOAT min, FAUSTFLOAT max);

    void (*addSoundfile)(void* ui_interface, const char* label, const char* url,
                         Soundfile** sf_zone);

    void (*declare)(void* ui_interface, FAUSTFLOAT* zone, const char* key, const char* value);
} UIGlue;

/* ── Adapters: C function pointers -> C++ interfaces ────────────────────── */

namespace faust_rs_glue {

inline UI* ui(void* self) { return static_cast<UI*>(self); }
inline Meta* meta(void* self) { return static_cast<Meta*>(self); }

inline void openTabBox(void* s, const char* l) { ui(s)->openTabBox(l); }
inline void openHorizontalBox(void* s, const char* l) { ui(s)->openHorizontalBox(l); }
inline void openVerticalBox(void* s, const char* l) { ui(s)->openVerticalBox(l); }
inline void closeBox(void* s) { ui(s)->closeBox(); }

inline void addButton(void* s, const char* l, FAUSTFLOAT* z) { ui(s)->addButton(l, z); }
inline void addCheckButton(void* s, const char* l, FAUSTFLOAT* z) { ui(s)->addCheckButton(l, z); }

inline void addVerticalSlider(void* s, const char* l, FAUSTFLOAT* z, FAUSTFLOAT i, FAUSTFLOAT mn,
                              FAUSTFLOAT mx, FAUSTFLOAT st)
{
    ui(s)->addVerticalSlider(l, z, i, mn, mx, st);
}
inline void addHorizontalSlider(void* s, const char* l, FAUSTFLOAT* z, FAUSTFLOAT i, FAUSTFLOAT mn,
                                FAUSTFLOAT mx, FAUSTFLOAT st)
{
    ui(s)->addHorizontalSlider(l, z, i, mn, mx, st);
}
inline void addNumEntry(void* s, const char* l, FAUSTFLOAT* z, FAUSTFLOAT i, FAUSTFLOAT mn,
                        FAUSTFLOAT mx, FAUSTFLOAT st)
{
    ui(s)->addNumEntry(l, z, i, mn, mx, st);
}

inline void addHorizontalBargraph(void* s, const char* l, FAUSTFLOAT* z, FAUSTFLOAT mn,
                                  FAUSTFLOAT mx)
{
    ui(s)->addHorizontalBargraph(l, z, mn, mx);
}
inline void addVerticalBargraph(void* s, const char* l, FAUSTFLOAT* z, FAUSTFLOAT mn, FAUSTFLOAT mx)
{
    ui(s)->addVerticalBargraph(l, z, mn, mx);
}

inline void addSoundfile(void* s, const char* l, const char* u, Soundfile** z)
{
    ui(s)->addSoundfile(l, u, z);
}
inline void declareZone(void* s, FAUSTFLOAT* z, const char* k, const char* v)
{
    ui(s)->declare(z, k, v);
}
inline void declareMeta(void* s, const char* k, const char* v) { meta(s)->declare(k, v); }

}  // namespace faust_rs_glue

/// Wires a [`UIGlue`] so the generated C code drives `ui_interface`.
inline void buildUIGlue(UIGlue* glue, UI* ui_interface)
{
    glue->uiInterface = ui_interface;
    glue->openTabBox = faust_rs_glue::openTabBox;
    glue->openHorizontalBox = faust_rs_glue::openHorizontalBox;
    glue->openVerticalBox = faust_rs_glue::openVerticalBox;
    glue->closeBox = faust_rs_glue::closeBox;
    glue->addButton = faust_rs_glue::addButton;
    glue->addCheckButton = faust_rs_glue::addCheckButton;
    glue->addVerticalSlider = faust_rs_glue::addVerticalSlider;
    glue->addHorizontalSlider = faust_rs_glue::addHorizontalSlider;
    glue->addNumEntry = faust_rs_glue::addNumEntry;
    glue->addHorizontalBargraph = faust_rs_glue::addHorizontalBargraph;
    glue->addVerticalBargraph = faust_rs_glue::addVerticalBargraph;
    glue->addSoundfile = faust_rs_glue::addSoundfile;
    glue->declare = faust_rs_glue::declareZone;
}

/// Wires a [`MetaGlue`] so the generated C code drives `meta_interface`.
inline void buildMetaGlue(MetaGlue* glue, Meta* meta_interface)
{
    glue->metaInterface = meta_interface;
    glue->declare = faust_rs_glue::declareMeta;
}

#endif  // FAUST_RS_MINIMAL_CGLUE_H
