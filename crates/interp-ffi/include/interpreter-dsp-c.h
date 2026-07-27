/************************** BEGIN interpreter-dsp-c.h **********************
 * C API for the Faust FBC interpreter — Rust port
 * (mirrors faust/architecture/faust/dsp/interpreter-dsp-c.h)
 *
 * NOTE: This header is written manually because cbindgen does not yet handle
 * Rust edition 2024's `#[unsafe(no_mangle)]` attribute.
 ************************************************************************/

#ifndef INTERPRETER_DSP_C_H
#define INTERPRETER_DSP_C_H

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* ── Opaque types ─────────────────────────────────────────────────────────── */

#ifdef _MSC_VER
typedef void interpreter_dsp_factory;
typedef void interpreter_dsp;
#else
typedef struct InterpreterDspFactory interpreter_dsp_factory;
typedef struct InterpreterDspInstance interpreter_dsp;
#endif

/* ── UIGlue (mirrors faust/gui/CInterface.h) ──────────────────────────────── */

/* Note: field names use snake_case to match Rust conventions.
 * The binary layout is identical to the original CInterface.h UIGlue. */

typedef void (*openTabBoxFn)(void* ui_interface, const char* label);
typedef void (*openHorizontalBoxFn)(void* ui_interface, const char* label);
typedef void (*openVerticalBoxFn)(void* ui_interface, const char* label);
typedef void (*closeBoxFn)(void* ui_interface);
typedef void (*addButtonFn)(void* ui_interface, const char* label, FAUSTFLOAT* zone);
typedef void (*addCheckButtonFn)(void* ui_interface, const char* label, FAUSTFLOAT* zone);
typedef void (*addVerticalSliderFn)(void* ui_interface, const char* label,
                                     FAUSTFLOAT* zone, FAUSTFLOAT init,
                                     FAUSTFLOAT min, FAUSTFLOAT max, FAUSTFLOAT step);
typedef void (*addHorizontalSliderFn)(void* ui_interface, const char* label,
                                       FAUSTFLOAT* zone, FAUSTFLOAT init,
                                       FAUSTFLOAT min, FAUSTFLOAT max, FAUSTFLOAT step);
typedef void (*addNumEntryFn)(void* ui_interface, const char* label,
                               FAUSTFLOAT* zone, FAUSTFLOAT init,
                               FAUSTFLOAT min, FAUSTFLOAT max, FAUSTFLOAT step);
typedef void (*addHorizontalBargraphFn)(void* ui_interface, const char* label,
                                         FAUSTFLOAT* zone, FAUSTFLOAT min, FAUSTFLOAT max);
typedef void (*addVerticalBargraphFn)(void* ui_interface, const char* label,
                                       FAUSTFLOAT* zone, FAUSTFLOAT min, FAUSTFLOAT max);
typedef void (*addSoundfileFn)(void* ui_interface, const char* label,
                                const char* url, void** sf_zone);
typedef void (*declareFn)(void* ui_interface, FAUSTFLOAT* zone,
                           const char* key, const char* value);

typedef struct {
    void* ui_interface;
    openTabBoxFn        open_tab_box;
    openHorizontalBoxFn open_horizontal_box;
    openVerticalBoxFn   open_vertical_box;
    closeBoxFn          close_box;
    addButtonFn         add_button;
    addCheckButtonFn    add_check_button;
    addVerticalSliderFn add_vertical_slider;
    addHorizontalSliderFn add_horizontal_slider;
    addNumEntryFn       add_num_entry;
    addHorizontalBargraphFn add_horizontal_bargraph;
    addVerticalBargraphFn   add_vertical_bargraph;
    addSoundfileFn      add_soundfile;
    declareFn           declare;
} UIGlue;

typedef void (*metaDeclareFn)(void* meta_interface, const char* key, const char* value);

typedef struct {
    void* meta_interface;
    metaDeclareFn declare;
} MetaGlue;

/* ── Version ──────────────────────────────────────────────────────────────── */

/**
 * Get the library version string.
 * The returned pointer is valid for the lifetime of the process.
 */
const char* getCLibFaustVersion(void);

/* ── Factory — bitcode I/O ────────────────────────────────────────────────── */

/**
 * Create a factory from a bitcode string (in-memory .fbc format).
 *
 * @param bitcode   null-terminated bitcode string
 * @param error_msg buffer of at least 4096 bytes for error output (may be NULL)
 * @return factory pointer on success, NULL on failure
 */
interpreter_dsp_factory* readCInterpreterDSPFactoryFromBitcode(
    const char* bitcode, char* error_msg);

/**
 * Serialize a factory to a bitcode string.
 *
 * @param factory  the factory to serialize
 * @return heap-allocated C string; caller must free with freeCMemory
 */
char* writeCInterpreterDSPFactoryToBitcode(interpreter_dsp_factory* factory);

/**
 * Create a factory from a .fbc file on disk.
 *
 * @param bit_code_path  null-terminated path to the .fbc file
 * @param error_msg      buffer of at least 4096 bytes (may be NULL)
 * @return factory pointer on success, NULL on failure
 */
interpreter_dsp_factory* readCInterpreterDSPFactoryFromBitcodeFile(
    const char* bit_code_path, char* error_msg);

/**
 * Write a factory to a .fbc file on disk.
 *
 * @param factory        the factory to write
 * @param bit_code_path  destination file path
 * @return true on success
 */
bool writeCInterpreterDSPFactoryToBitcodeFile(
    interpreter_dsp_factory* factory, const char* bit_code_path);

/* ── Factory — source constructors ────────────────────────────────── */

/**
 * Create a factory from a DSP source file.
 *
 * In the current Rust port, this entry point may return NULL with an error
 * message if the full Faust compiler pipeline is not available in the linked
 * C API build.
 */
interpreter_dsp_factory* createCInterpreterDSPFactoryFromFile(
    const char* filename, int argc, const char* argv[], char* error_msg);

/**
 * Create a factory from DSP source code provided as a string.
 *
 * In the current Rust port, this entry point may return NULL with an error
 * message if the full Faust compiler pipeline is not available in the linked
 * C API build.
 */
interpreter_dsp_factory* createCInterpreterDSPFactoryFromString(
    const char* name_app, const char* dsp_content,
    int argc, const char* argv[], char* error_msg);

/* ── Factory — cache management ──────────────────────────────────────────── */

/**
 * Look up a factory in the global cache by SHA key and acquire a reference.
 * The returned pointer must be released with deleteCInterpreterDSPFactory.
 */
interpreter_dsp_factory* getCInterpreterDSPFactoryFromSHAKey(const char* sha_key);

/**
 * Release one factory reference.
 * The final release deletes remaining DSP instances, removes the cache entry,
 * and invalidates all pointers associated with that factory.
 * @return true only if this was the final reference
 */
bool deleteCInterpreterDSPFactory(interpreter_dsp_factory* factory);

/**
 * Delete all factories and remaining DSP instances in the global cache.
 * All outstanding factory and instance pointers become invalid.
 */
void deleteAllCInterpreterDSPFactories(void);

/**
 * Return all factory SHA keys as a null-terminated array of C strings.
 *
 * Each element must be freed with freeCMemory, then the array pointer itself.
 * Returns NULL if the cache is empty.
 */
char** getAllCInterpreterDSPFactories(void);

/**
 * Return a JSON description of the factory's UI and metadata.
 * The returned string must be freed with freeCMemory.
 */
char* getCInterpreterDSPFactoryJSON(interpreter_dsp_factory* factory);

/**
 * Return library dependencies (always an empty null-terminated array for
 * the interpreter backend).  Do NOT free the returned pointer.
 */
const char** getCInterpreterDSPFactoryLibraryList(interpreter_dsp_factory* factory);

/* ── Multi-thread mode ───────────────────────────────────────────────────── */

bool startMTDSPFactories(void);
void stopMTDSPFactories(void);

/* Rust extension: foreign-function registry used by `ffunction(...)`
 * when compiling/executing with the interpreter backend.
 */
void registerCInterpreterForeignFunction(const char* name, void* fn_ptr);
void unregisterCInterpreterForeignFunction(const char* name);
void clearCInterpreterForeignFunctions(void);

/* ── Memory management ───────────────────────────────────────────────────── */

/**
 * Free a C string or array pointer previously returned by this library.
 *
 * For char** arrays: free each element first, then the array pointer.
 */
void freeCMemory(void* ptr);

/* ── Instance — creation / deletion ─────────────────────────────────────── */

/**
 * Create a new DSP instance from a factory.
 * The instance may be deleted manually. Otherwise the cache deletes it when
 * the final factory reference is released or all factories are deleted.
 */
interpreter_dsp* createCInterpreterDSPInstance(interpreter_dsp_factory* factory);

/**
 * Delete a DSP instance.
 */
void deleteCInterpreterDSPInstance(interpreter_dsp* dsp);

/* ── Instance — audio layout ─────────────────────────────────────────────── */

int getNumInputsCInterpreterDSPInstance(interpreter_dsp* dsp);
int getNumOutputsCInterpreterDSPInstance(interpreter_dsp* dsp);
int getSampleRateCInterpreterDSPInstance(interpreter_dsp* dsp);

/* ── Instance — initialization lifecycle ────────────────────────────────── */

void initCInterpreterDSPInstance(interpreter_dsp* dsp, int sample_rate);
void instanceInitCInterpreterDSPInstance(interpreter_dsp* dsp, int sample_rate);
void instanceConstantsCInterpreterDSPInstance(interpreter_dsp* dsp, int sample_rate);
void instanceResetUserInterfaceCInterpreterDSPInstance(interpreter_dsp* dsp);
void instanceClearCInterpreterDSPInstance(interpreter_dsp* dsp);

/* ── Instance — clone ────────────────────────────────────────────────────── */

interpreter_dsp* cloneCInterpreterDSPInstance(interpreter_dsp* dsp);

/* ── Instance — UI / metadata ────────────────────────────────────────────── */

void buildUserInterfaceCInterpreterDSPInstance(interpreter_dsp* dsp, UIGlue* glue);
void metadataCInterpreterDSPInstance(interpreter_dsp* dsp, MetaGlue* meta);

/* ── Instance — audio computation ───────────────────────────────────────── */

/**
 * Process one buffer of audio samples.
 *
 * @param dsp     DSP instance
 * @param count   number of frames
 * @param inputs  array of num_inputs non-interleaved float* buffers
 * @param outputs array of num_outputs non-interleaved float* buffers
 */
void computeCInterpreterDSPInstance(interpreter_dsp* dsp, int count,
                                     FAUSTFLOAT** inputs, FAUSTFLOAT** outputs);

/* ── DSP expansion ───────────────────────────────────────────────────────── */

/**
 * Validate and expand a Faust DSP source file.
 *
 * On success returns a heap-allocated C string; caller must free with
 * freeCMemory.
 *
 * @param filename   path to the Faust DSP file
 * @param argc       number of compiler arguments
 * @param argv       compiler argument array (may be NULL when argc == 0)
 * @param sha_key    buffer of at least 64 bytes for SHA key output (may be NULL)
 * @param error_msg  buffer of at least 4096 bytes for error output (may be NULL)
 * @return heap-allocated expanded source, or NULL on failure
 */
char* expandCInterpreterDSPFromFile(
    const char* filename, int argc, const char* argv[],
    char* sha_key, char* error_msg);

/**
 * Validate and expand a Faust DSP source string.
 *
 * On success returns a heap-allocated C string; caller must free with
 * freeCMemory.
 *
 * @param name_app     logical DSP name (may be NULL)
 * @param dsp_content  Faust source text
 * @param argc         number of compiler arguments
 * @param argv         compiler argument array (may be NULL when argc == 0)
 * @param sha_key      buffer of at least 64 bytes for SHA key output (may be NULL)
 * @param error_msg    buffer of at least 4096 bytes for error output (may be NULL)
 * @return heap-allocated expanded source, or NULL on failure
 */
char* expandCInterpreterDSPFromString(
    const char* name_app, const char* dsp_content, int argc, const char* argv[],
    char* sha_key, char* error_msg);

/* ── Auxiliary file generation ───────────────────────────────────────────── */

/**
 * Generate auxiliary output files from a Faust DSP source file.
 *
 * Output formats are selected by argv flags: -cpp, -c, -wasm, -json, -svg.
 * Output directory is taken from -O <path> (defaults to ".").
 *
 * @param filename   path to the Faust DSP file
 * @param argc       number of compiler arguments
 * @param argv       compiler argument array (may be NULL when argc == 0)
 * @param error_msg  buffer of at least 4096 bytes for error output (may be NULL)
 * @return true on success, false on failure
 */
bool generateCInterpreterAuxFilesFromFile(
    const char* filename, int argc, const char* argv[], char* error_msg);

/**
 * Generate auxiliary output files from a Faust DSP source string.
 *
 * Output formats are selected by argv flags: -cpp, -c, -wasm, -json, -svg.
 * Output directory is taken from -O <path> (defaults to ".").
 *
 * @param name_app     logical DSP name (may be NULL)
 * @param dsp_content  Faust source text
 * @param argc         number of compiler arguments
 * @param argv         compiler argument array (may be NULL when argc == 0)
 * @param error_msg    buffer of at least 4096 bytes for error output (may be NULL)
 * @return true on success, false on failure
 */
bool generateCInterpreterAuxFilesFromString(
    const char* name_app, const char* dsp_content, int argc, const char* argv[],
    char* error_msg);

#ifdef __cplusplus
}
#endif

#endif /* INTERPRETER_DSP_C_H */

/************************** END interpreter-dsp-c.h **************************/
