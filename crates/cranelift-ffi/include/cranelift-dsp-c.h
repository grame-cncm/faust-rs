/************************** BEGIN cranelift-dsp-c.h ************************
 * C API for the Faust Cranelift backend — Rust port
 * (modeled after faust/architecture/faust/dsp/interpreter-dsp-c.h)
 *
 * NOTE: This header is written manually because cbindgen does not yet handle
 * Rust edition 2024's `#[unsafe(no_mangle)]` attribute.
 ************************************************************************/

#ifndef CRANELIFT_DSP_C_H
#define CRANELIFT_DSP_C_H

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include "faust-memory-manager.h"

#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* ── Opaque types ─────────────────────────────────────────────────────────── */

#ifdef _MSC_VER
typedef void cranelift_dsp_factory;
typedef void cranelift_dsp;
#else
typedef struct CraneliftDspFactory cranelift_dsp_factory;
typedef struct CraneliftDspInstance cranelift_dsp;
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
 * Create a factory from a bitcode string.
 *
 * The current Rust implementation uses a temporary scaffold format marker
 * (`CRANELIFT_FFI_SCAFFOLD_V1`) for API-family validation only. It is not the
 * final backend serialization format.
 *
 * @param bit_code  null-terminated bitcode string
 * @param error_msg buffer of at least 4096 bytes for error output (may be NULL)
 * @return factory pointer on success, NULL on failure
 */
cranelift_dsp_factory* readCCraneliftDSPFactoryFromBitcode(
    const char* bit_code, char* error_msg);

/**
 * Serialize a factory to a bitcode string.
 *
 * The current Rust implementation uses a temporary scaffold format marker
 * (`CRANELIFT_FFI_SCAFFOLD_V1`) for API-family validation only. It is not the
 * final backend serialization format.
 *
 * @param factory  the factory to serialize
 * @return heap-allocated C string; caller must free with freeCMemory
 */
char* writeCCraneliftDSPFactoryToBitcode(cranelift_dsp_factory* factory);

/**
 * Create a factory from a bitcode file on disk.
 *
 * @param bit_code_path  null-terminated path to the bitcode file
 * @param error_msg      buffer of at least 4096 bytes (may be NULL)
 * @return factory pointer on success, NULL on failure
 */
cranelift_dsp_factory* readCCraneliftDSPFactoryFromBitcodeFile(
    const char* bit_code_path, char* error_msg);

/**
 * Write a factory to a bitcode file on disk.
 *
 * @param factory        the factory to write
 * @param bit_code_path  destination file path
 * @return true on success
 */
bool writeCCraneliftDSPFactoryToBitcodeFile(
    cranelift_dsp_factory* factory, const char* bit_code_path);

/* ── Factory — source constructors ───────────────────────────────────────── */

/**
 * Create a factory from a DSP source file.
 *
 * Cranelift source constructors expose `opt_level` but do not carry the LLVM
 * `target` parameter.
 */
cranelift_dsp_factory* createCCraneliftDSPFactoryFromFile(
    const char* filename, int argc, const char* argv[], char* error_msg, int opt_level);

/**
 * Create a factory from DSP source code provided as a string.
 *
 * Cranelift source constructors expose `opt_level` but do not carry the LLVM
 * `target` parameter.
 */
cranelift_dsp_factory* createCCraneliftDSPFactoryFromString(
    const char* name_app, const char* dsp_content,
    int argc, const char* argv[], char* error_msg, int opt_level);

/* ── Factory — cache management ──────────────────────────────────────────── */

/**
 * Look up a factory in the global cache by SHA key and acquire a reference.
 * The returned pointer must be released with deleteCCraneliftDSPFactory.
 */
cranelift_dsp_factory* getCCraneliftDSPFactoryFromSHAKey(const char* sha_key);

/**
 * Release one factory reference.
 * The final release deletes remaining DSP instances, removes the cache entry,
 * and invalidates all pointers associated with that factory.
 * @return true only if this was the final reference
 */
bool deleteCCraneliftDSPFactory(cranelift_dsp_factory* factory);

/** Bind and describe a custom allocator for a factory compiled with `-mem0`. */
bool setCCraneliftMemoryManager(cranelift_dsp_factory* factory,
                                const faust_memory_manager* manager,
                                char* error_msg);

/**
 * Delete all factories and remaining DSP instances in the global cache.
 * All outstanding factory and instance pointers become invalid.
 */
void deleteAllCCraneliftDSPFactories(void);

/**
 * Return all factory SHA keys as a null-terminated array of C strings.
 *
 * Each element must be freed with freeCMemory, then the array pointer itself.
 * Returns NULL if the cache is empty.
 */
char** getAllCCraneliftDSPFactories(void);

/**
 * Return the factory name string.
 * The returned string must be freed with freeCMemory.
 */
char* getCCraneliftDSPFactoryName(cranelift_dsp_factory* factory);

/**
 * Return the factory SHA key string.
 * The returned string must be freed with freeCMemory.
 */
char* getCCraneliftDSPFactorySHAKey(cranelift_dsp_factory* factory);

/**
 * Return expanded DSP code.
 * The returned string must be freed with freeCMemory.
 */
char* getCCraneliftDSPFactoryDSPCode(cranelift_dsp_factory* factory);

/**
 * Return a JSON description of the factory's UI and metadata.
 * The returned string must be freed with freeCMemory.
 */
char* getCCraneliftDSPFactoryJSON(cranelift_dsp_factory* factory);

/**
 * Return factory compile options.
 * The returned string must be freed with freeCMemory.
 */
char* getCCraneliftDSPFactoryCompileOptions(cranelift_dsp_factory* factory);

/**
 * Return library dependencies as a null-terminated array of strings.
 * Do NOT free the returned pointer.
 */
const char** getCCraneliftDSPFactoryLibraryList(cranelift_dsp_factory* factory);

/**
 * Return include pathnames as a null-terminated array of strings.
 * Do NOT free the returned pointer.
 */
const char** getCCraneliftDSPFactoryIncludePathnames(cranelift_dsp_factory* factory);

/**
 * Return warning messages as a null-terminated array of strings.
 * Do NOT free the returned pointer.
 */
const char** getCCraneliftDSPFactoryWarningMessages(cranelift_dsp_factory* factory);

/* ── Multi-thread mode ───────────────────────────────────────────────────── */

bool startMTDSPFactories(void);
void stopMTDSPFactories(void);

/* Rust extension: foreign-function registry used by `ffunction(...)`
 * when compiling/executing with the Cranelift backend.
 */
void registerCCraneliftForeignFunction(const char* name, void* fn_ptr);
void unregisterCCraneliftForeignFunction(const char* name);
void clearCCraneliftForeignFunctions(void);

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
cranelift_dsp* createCCraneliftDSPInstance(cranelift_dsp_factory* factory);

/**
 * Delete a DSP instance.
 */
void deleteCCraneliftDSPInstance(cranelift_dsp* dsp);

/* ── Instance — audio layout ─────────────────────────────────────────────── */

int getNumInputsCCraneliftDSPInstance(cranelift_dsp* dsp);
int getNumOutputsCCraneliftDSPInstance(cranelift_dsp* dsp);
int getSampleRateCCraneliftDSPInstance(cranelift_dsp* dsp);

/* ── Instance — initialization lifecycle ────────────────────────────────── */

void initCCraneliftDSPInstance(cranelift_dsp* dsp, int sample_rate);
void instanceInitCCraneliftDSPInstance(cranelift_dsp* dsp, int sample_rate);
void instanceConstantsCCraneliftDSPInstance(cranelift_dsp* dsp, int sample_rate);
void instanceResetUserInterfaceCCraneliftDSPInstance(cranelift_dsp* dsp);
void instanceClearCCraneliftDSPInstance(cranelift_dsp* dsp);

/* ── Instance — clone ────────────────────────────────────────────────────── */

cranelift_dsp* cloneCCraneliftDSPInstance(cranelift_dsp* dsp);

/* ── Instance — UI / metadata ────────────────────────────────────────────── */

void buildUserInterfaceCCraneliftDSPInstance(cranelift_dsp* dsp, UIGlue* glue);
void metadataCCraneliftDSPInstance(cranelift_dsp* dsp, MetaGlue* meta);

/* ── Instance — audio computation ───────────────────────────────────────── */

/**
 * Process one buffer of audio samples.
 *
 * @param dsp     DSP instance
 * @param count   number of frames
 * @param inputs  array of num_inputs non-interleaved float* buffers
 * @param outputs array of num_outputs non-interleaved float* buffers
 */
void computeCCraneliftDSPInstance(cranelift_dsp* dsp, int count,
                                   FAUSTFLOAT** inputs, FAUSTFLOAT** outputs);

/* ── DSP expansion ───────────────────────────────────────────────────────── */

/**
 * Validate and expand a Faust DSP source file.
 *
 * Parses and evaluates the file; on success returns a heap-allocated C string
 * containing the (unexpanded) source.  Caller must free with freeCMemory.
 *
 * @param filename   path to the Faust DSP file
 * @param argc       number of compiler arguments
 * @param argv       compiler argument array (may be NULL when argc == 0)
 * @param sha_key    buffer of at least 64 bytes for SHA key output (may be NULL)
 * @param error_msg  buffer of at least 4096 bytes for error output (may be NULL)
 * @return heap-allocated expanded source, or NULL on failure
 */
char* expandCCraneliftDSPFromFile(
    const char* filename, int argc, const char* argv[],
    char* sha_key, char* error_msg);

/**
 * Validate and expand a Faust DSP source string.
 *
 * On success returns a heap-allocated C string.  Caller must free with
 * freeCMemory.
 *
 * @param name_app   logical DSP name (may be NULL)
 * @param dsp_content  Faust source text
 * @param argc       number of compiler arguments
 * @param argv       compiler argument array (may be NULL when argc == 0)
 * @param sha_key    buffer of at least 64 bytes for SHA key output (may be NULL)
 * @param error_msg  buffer of at least 4096 bytes for error output (may be NULL)
 * @return heap-allocated expanded source, or NULL on failure
 */
char* expandCCraneliftDSPFromString(
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
bool generateCCraneliftAuxFilesFromFile(
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
bool generateCCraneliftAuxFilesFromString(
    const char* name_app, const char* dsp_content, int argc, const char* argv[],
    char* error_msg);

#ifdef __cplusplus
}
#endif

#endif /* CRANELIFT_DSP_C_H */

/************************** END cranelift-dsp-c.h **************************/
