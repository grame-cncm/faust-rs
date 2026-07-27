/************************** BEGIN interpreter-dsp.h **********************
 * C++ wrapper for the Faust FBC interpreter — Rust port
 * (mirrors faust/architecture/faust/dsp/interpreter-dsp.h)
 *
 * This header provides C++ classes that wrap the interpreter C API and is
 * self-contained (it does not include `interpreter-dsp-c.h`).
 * C projects should include `interpreter-dsp-c.h` directly.
 ************************************************************************/

#ifndef INTERPRETER_DSP_H
#define INTERPRETER_DSP_H

#include <string>
#include <vector>
#include <cstring>    // strdup, strnlen
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include "faust/gui/CInterface.h"
#include "faust/gui/CGlue.h"

// Self-contained C API declarations (mirrors crates/interp-ffi/include/interpreter-dsp-c.h)
// This header intentionally does not include interpreter-dsp-c.h so it can be
// used standalone from C++ codebases expecting a single include.

#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif

#ifdef __cplusplus
extern "C" {
#endif

// Opaque C API types
#ifdef _MSC_VER
typedef void cinterpreter_dsp_factory;
typedef void cinterpreter_dsp;
#else
typedef struct InterpreterDspFactory cinterpreter_dsp_factory;
typedef struct InterpreterDspInstance cinterpreter_dsp;
#endif

// C API functions used by this C++ wrapper
const char* getCLibFaustVersion(void);

cinterpreter_dsp_factory* getCInterpreterDSPFactoryFromSHAKey(const char* sha_key);
cinterpreter_dsp_factory* readCInterpreterDSPFactoryFromBitcode(
    const char* bitcode, char* error_msg);
cinterpreter_dsp_factory* readCInterpreterDSPFactoryFromBitcodeFile(
    const char* bit_code_path, char* error_msg);
cinterpreter_dsp_factory* createCInterpreterDSPFactoryFromFile(
    const char* filename, int argc, const char* argv[], char* error_msg);
cinterpreter_dsp_factory* createCInterpreterDSPFactoryFromString(
    const char* name_app, const char* dsp_content,
    int argc, const char* argv[], char* error_msg);
char* writeCInterpreterDSPFactoryToBitcode(cinterpreter_dsp_factory* factory);
bool writeCInterpreterDSPFactoryToBitcodeFile(
    cinterpreter_dsp_factory* factory, const char* bit_code_path);
bool deleteCInterpreterDSPFactory(cinterpreter_dsp_factory* factory);
void deleteAllCInterpreterDSPFactories(void);
char** getAllCInterpreterDSPFactories(void);
char* getCInterpreterDSPFactoryJSON(cinterpreter_dsp_factory* factory);
const char** getCInterpreterDSPFactoryLibraryList(cinterpreter_dsp_factory* factory);
bool startMTDSPFactories(void);
void stopMTDSPFactories(void);
void freeCMemory(void* ptr);

cinterpreter_dsp* createCInterpreterDSPInstance(cinterpreter_dsp_factory* factory);
void deleteCInterpreterDSPInstance(cinterpreter_dsp* dsp);
cinterpreter_dsp* cloneCInterpreterDSPInstance(cinterpreter_dsp* dsp);

int getNumInputsCInterpreterDSPInstance(cinterpreter_dsp* dsp);
int getNumOutputsCInterpreterDSPInstance(cinterpreter_dsp* dsp);
int getSampleRateCInterpreterDSPInstance(cinterpreter_dsp* dsp);
void initCInterpreterDSPInstance(cinterpreter_dsp* dsp, int sample_rate);
void instanceInitCInterpreterDSPInstance(cinterpreter_dsp* dsp, int sample_rate);
void instanceConstantsCInterpreterDSPInstance(cinterpreter_dsp* dsp, int sample_rate);
void instanceResetUserInterfaceCInterpreterDSPInstance(cinterpreter_dsp* dsp);
void instanceClearCInterpreterDSPInstance(cinterpreter_dsp* dsp);
void buildUserInterfaceCInterpreterDSPInstance(cinterpreter_dsp* dsp, UIGlue* glue);
void metadataCInterpreterDSPInstance(cinterpreter_dsp* dsp, MetaGlue* meta);
void computeCInterpreterDSPInstance(cinterpreter_dsp* dsp, int count,
                                    FAUSTFLOAT** inputs, FAUSTFLOAT** outputs);
void registerCInterpreterForeignFunction(const char* name, void* fn_ptr);
void unregisterCInterpreterForeignFunction(const char* name);
void clearCInterpreterForeignFunctions(void);

char* expandCInterpreterDSPFromFile(
    const char* filename, int argc, const char* argv[],
    char* sha_key, char* error_msg);
char* expandCInterpreterDSPFromString(
    const char* name_app, const char* dsp_content, int argc, const char* argv[],
    char* sha_key, char* error_msg);
bool generateCInterpreterAuxFilesFromFile(
    const char* filename, int argc, const char* argv[], char* error_msg);
bool generateCInterpreterAuxFilesFromString(
    const char* name_app, const char* dsp_content, int argc, const char* argv[],
    char* error_msg);

#ifdef __cplusplus
}
#endif

// ── Compatibility note ────────────────────────────────────────────────────────
// This self-contained header reuses `faust/gui/CInterface.h` for UIGlue/MetaGlue
// definitions to remain source-compatible with `CGlue.h`, `GTKUI`, JACK helpers,
// etc., while still embedding the interpreter C API declarations directly.
// ─────────────────────────────────────────────────────────────────────────────

#ifdef __cplusplus

class interpreter_dsp;

// ── interpreter_dsp_factory ───────────────────────────────────────────────────

/**
 * C++ wrapper for the FBC DSP factory.
 *
 * Instances are obtained via the free functions below (readInterpreterDSP...).
 * Each returned wrapper owns one cache reference; release it with
 * deleteInterpreterDSPFactory().
 */
class interpreter_dsp_factory : public dsp_factory {
public:
    explicit interpreter_dsp_factory(cinterpreter_dsp_factory* impl)
        : impl_(impl) {}

    ~interpreter_dsp_factory() override = default;

    // Non-copyable: ownership managed explicitly by the C API.
    interpreter_dsp_factory(const interpreter_dsp_factory&) = delete;
    interpreter_dsp_factory& operator=(const interpreter_dsp_factory&) = delete;

    /// Return the underlying C pointer (for passing to C API functions).
    cinterpreter_dsp_factory* get() const { return impl_; }

    /// Return factory name (not yet exposed in the C API).
    std::string getName() override {
        return "";
    }

    /// Return factory SHA key.
    std::string getSHAKey() override {
        // SHA key is embedded in the JSON; extract it from getJSON for now.
        // (A dedicated getCInterpreterDSPFactoryName function is not in the
        // initial C API — use the JSON field.)
        return "";  // TODO: expose getSHAKey via C API
    }

    /// Return expanded DSP code (not yet exposed in the C API).
    std::string getDSPCode() override {
        return "";
    }

    /// Return factory JSON description.
    std::string getJSON() override {
        char* raw = getCInterpreterDSPFactoryJSON(impl_);
        if (!raw) return "{}";
        std::string result(raw);
        freeCMemory(raw);
        return result;
    }

    /// Return factory compile options (not yet exposed in the C API).
    std::string getCompileOptions() override {
        return "";
    }

    std::vector<std::string> getLibraryList() override {
        return {};
    }

    std::vector<std::string> getIncludePathnames() override {
        return {};
    }

    std::vector<std::string> getWarningMessages() override {
        return {};
    }

    /// Serialize factory to a bitcode string (in-memory .fbc format).
    std::string writeToMemory() const {
        char* raw = writeCInterpreterDSPFactoryToBitcode(impl_);
        if (!raw) return "";
        std::string result(raw);
        freeCMemory(raw);
        return result;
    }

    /// Write factory to a .fbc file.
    bool writeToFile(const std::string& path) const {
        return writeCInterpreterDSPFactoryToBitcodeFile(impl_, path.c_str());
    }

    /// Create a new DSP instance from this factory.
    ///
    /// The caller owns the returned object and is responsible for deletion.
    ::dsp* createDSPInstance() override;

    void setMemoryManager(dsp_memory_manager* /*manager*/) override {}

    dsp_memory_manager* getMemoryManager() override { return nullptr; }

private:
    cinterpreter_dsp_factory* impl_;
};

// ── interpreter_dsp ───────────────────────────────────────────────────────────

/**
 * C++ wrapper for a FBC DSP instance.
 *
 * Instances are created via `interpreter_dsp_factory::createDSPInstance()`.
 * The caller owns the instance; call `delete` to release it.
 */
class interpreter_dsp : public dsp {
public:
    explicit interpreter_dsp(cinterpreter_dsp* impl) : impl_(impl) {}

    ~interpreter_dsp() override {
        if (impl_) {
            deleteCInterpreterDSPInstance(impl_);
            impl_ = nullptr;
        }
    }

    // Non-copyable.
    interpreter_dsp(const interpreter_dsp&) = delete;
    interpreter_dsp& operator=(const interpreter_dsp&) = delete;

    int getNumInputs() override {
        return getNumInputsCInterpreterDSPInstance(impl_);
    }

    int getNumOutputs() override {
        return getNumOutputsCInterpreterDSPInstance(impl_);
    }

    int getSampleRate() override {
        return getSampleRateCInterpreterDSPInstance(impl_);
    }

    void init(int sample_rate) override {
        initCInterpreterDSPInstance(impl_, sample_rate);
    }

    void instanceInit(int sample_rate) override {
        instanceInitCInterpreterDSPInstance(impl_, sample_rate);
    }

    void instanceConstants(int sample_rate) override {
        instanceConstantsCInterpreterDSPInstance(impl_, sample_rate);
    }

    void instanceResetUserInterface() override {
        instanceResetUserInterfaceCInterpreterDSPInstance(impl_);
    }

    void instanceClear() override {
        instanceClearCInterpreterDSPInstance(impl_);
    }

    interpreter_dsp* clone() override {
        cinterpreter_dsp* c = cloneCInterpreterDSPInstance(impl_);
        return c ? new interpreter_dsp(c) : nullptr;
    }

    void buildUserInterface(UI* ui_interface) override {
        UIGlue glue;
        buildUIGlue(&glue, ui_interface, sizeof(FAUSTFLOAT) == sizeof(double));
        buildUserInterfaceCInterpreterDSPInstance(impl_, &glue);
    }

    void metadata(Meta* meta) override {
        MetaGlue glue;
        buildMetaGlue(&glue, meta);
        metadataCInterpreterDSPInstance(impl_, &glue);
    }

    // Optional direct C-glue entrypoints kept for convenience.
    void buildUserInterface(UIGlue* glue) {
        buildUserInterfaceCInterpreterDSPInstance(impl_, glue);
    }

    void metadata(MetaGlue* meta) {
        metadataCInterpreterDSPInstance(impl_, meta);
    }

    /**
     * Process one buffer of audio.
     *
     * @param count   number of frames
     * @param inputs  array of num_inputs non-interleaved float* buffers
     * @param outputs array of num_outputs non-interleaved float* buffers
     */
    void compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs) override {
        computeCInterpreterDSPInstance(impl_, count, inputs, outputs);
    }

    cinterpreter_dsp* get() const { return impl_; }

private:
    cinterpreter_dsp* impl_;
};

// ── interpreter_dsp_factory::createDSPInstance (needs interpreter_dsp defined) ─

inline ::dsp* interpreter_dsp_factory::createDSPInstance() {
    cinterpreter_dsp* c = createCInterpreterDSPInstance(impl_);
    return c ? new interpreter_dsp(c) : nullptr;
}

// ── Free functions ────────────────────────────────────────────────────────────

/**
 * Create a factory from a bitcode string (.fbc in-memory format).
 *
 * @param bit_code  the bitcode string
 * @param error_msg output error message (filled on failure)
 * @return a factory on success, nullptr on failure
 */
inline interpreter_dsp_factory* readInterpreterDSPFactoryFromBitcode(
    const std::string& bit_code,
    std::string& error_msg)
{
    char buf[4096] = {};
    cinterpreter_dsp_factory* raw =
        readCInterpreterDSPFactoryFromBitcode(bit_code.c_str(), buf);
    if (!raw) {
        error_msg = buf;
        return nullptr;
    }
    return new interpreter_dsp_factory(raw);
}

/**
 * Create a factory from a DSP source file.
 *
 * This forwards to the interpreter C API constructor. In the current Rust port,
 * this entry point may return `nullptr` with an error message if the full Faust
 * compiler pipeline is not available in the linked C API build.
 */
inline interpreter_dsp_factory* createInterpreterDSPFactoryFromFile(
    const std::string& filename,
    int argc,
    const char* argv[],
    std::string& error_msg)
{
    char buf[4096] = {};
    cinterpreter_dsp_factory* raw =
        createCInterpreterDSPFactoryFromFile(filename.c_str(), argc, argv, buf);
    if (!raw) {
        error_msg = buf;
        return nullptr;
    }
    return new interpreter_dsp_factory(raw);
}

/**
 * Create a factory from DSP source code provided as a string.
 *
 * This forwards to the interpreter C API constructor. In the current Rust port,
 * this entry point may return `nullptr` with an error message if the full Faust
 * compiler pipeline is not available in the linked C API build.
 */
inline interpreter_dsp_factory* createInterpreterDSPFactoryFromString(
    const std::string& name_app,
    const std::string& dsp_content,
    int argc,
    const char* argv[],
    std::string& error_msg)
{
    char buf[4096] = {};
    cinterpreter_dsp_factory* raw = createCInterpreterDSPFactoryFromString(
        name_app.c_str(), dsp_content.c_str(), argc, argv, buf);
    if (!raw) {
        error_msg = buf;
        return nullptr;
    }
    return new interpreter_dsp_factory(raw);
}

/**
 * Serialize a factory to a bitcode string.
 *
 * @param factory the factory to serialize
 * @return the bitcode as a std::string
 */
inline std::string writeInterpreterDSPFactoryToBitcode(
    interpreter_dsp_factory* factory)
{
    return factory ? factory->writeToMemory() : "";
}

/**
 * Create a factory from a .fbc file.
 *
 * @param path      path to the .fbc file
 * @param error_msg output error message (filled on failure)
 * @return a factory on success, nullptr on failure
 */
inline interpreter_dsp_factory* readInterpreterDSPFactoryFromBitcodeFile(
    const std::string& path,
    std::string& error_msg)
{
    char buf[4096] = {};
    cinterpreter_dsp_factory* raw =
        readCInterpreterDSPFactoryFromBitcodeFile(path.c_str(), buf);
    if (!raw) {
        error_msg = buf;
        return nullptr;
    }
    return new interpreter_dsp_factory(raw);
}

/**
 * Write a factory to a .fbc file.
 *
 * @param factory the factory to write
 * @param path    destination file path
 * @return true on success
 */
inline bool writeInterpreterDSPFactoryToBitcodeFile(
    interpreter_dsp_factory* factory,
    const std::string& path)
{
    return factory && factory->writeToFile(path);
}

/**
 * Look up a factory in the cache by SHA key.
 *
 * @param sha_key the SHA key
 * @return a newly acquired factory reference, or nullptr if not found
 */
inline interpreter_dsp_factory* getInterpreterDSPFactoryFromSHAKey(
    const std::string& sha_key)
{
    cinterpreter_dsp_factory* raw =
        getCInterpreterDSPFactoryFromSHAKey(sha_key.c_str());
    return raw ? new interpreter_dsp_factory(raw) : nullptr;
}

/**
 * Release one factory reference.
 *
 * @param factory the factory to delete
 * @return true only if this was the final reference
 */
inline bool deleteInterpreterDSPFactory(interpreter_dsp_factory* factory) {
    if (!factory) return false;
    bool result = deleteCInterpreterDSPFactory(factory->get());
    delete factory;
    return result;
}

/**
 * Delete all factories and remaining DSP instances in the cache.
 * All previously returned wrapper payloads become invalid.
 */
inline void deleteAllInterpreterDSPFactories() {
    deleteAllCInterpreterDSPFactories();
}

/**
 * Return all SHA keys in the cache.
 */
inline std::vector<std::string> getAllInterpreterDSPFactories() {
    char** raw = getAllCInterpreterDSPFactories();
    std::vector<std::string> result;
    if (!raw) return result;
    for (int i = 0; raw[i]; ++i) {
        result.push_back(raw[i]);
        freeCMemory(raw[i]);
    }
    freeCMemory(raw);
    return result;
}

/**
 * Enable multi-thread-safe access to the global interpreter factory cache.
 */
inline bool startInterpreterMTDSPFactories()
{
    return startMTDSPFactories();
}

/**
 * Disable multi-thread-safe access to the global interpreter factory cache.
 */
inline void stopInterpreterMTDSPFactories()
{
    stopMTDSPFactories();
}

/**
 * Register a host foreign function for interpreter `ffunction(...)` calls.
 *
 * The pointer must remain valid for the duration of any compiled factory or
 * DSP instance using the symbol.
 */
inline void registerInterpreterForeignFunction(
    const std::string& name,
    void* fn_ptr)
{
    registerCInterpreterForeignFunction(name.c_str(), fn_ptr);
}

/**
 * Remove one registered interpreter foreign function by symbol name.
 */
inline void unregisterInterpreterForeignFunction(const std::string& name)
{
    unregisterCInterpreterForeignFunction(name.c_str());
}

/**
 * Clear the entire interpreter foreign-function registry.
 */
inline void clearInterpreterForeignFunctions()
{
    clearCInterpreterForeignFunctions();
}

/**
 * Validate and expand a Faust DSP source file.
 *
 * Parses and evaluates the file using the supplied compiler arguments.
 * On success returns the source text; on failure returns an empty string
 * and fills `error_msg`.
 *
 * @param filename   path to the .dsp file
 * @param argc       number of compiler arguments
 * @param argv       compiler argument array
 * @param sha_key    receives a hex digest of the source (may be empty)
 * @param error_msg  receives an error description on failure
 * @return expanded DSP source, or empty string on failure
 */
inline std::string expandInterpreterDSPFromFile(
    const std::string& filename,
    int argc,
    const char* argv[],
    std::string& sha_key,
    std::string& error_msg)
{
    char sha_buf[64] = {};
    char err_buf[4096] = {};
    char* raw = expandCInterpreterDSPFromFile(
        filename.c_str(), argc, argv, sha_buf, err_buf);
    if (!raw) {
        error_msg = err_buf;
        sha_key.clear();
        return {};
    }
    sha_key = sha_buf;
    error_msg.clear();
    std::string result(raw);
    freeCMemory(raw);
    return result;
}

/**
 * Validate and expand a Faust DSP source string.
 *
 * On success returns the source text; on failure returns an empty string
 * and fills `error_msg`.
 *
 * @param name_app     logical DSP name
 * @param dsp_content  Faust source text
 * @param argc         number of compiler arguments
 * @param argv         compiler argument array
 * @param sha_key      receives a hex digest of the source (may be empty)
 * @param error_msg    receives an error description on failure
 * @return expanded DSP source, or empty string on failure
 */
inline std::string expandInterpreterDSPFromString(
    const std::string& name_app,
    const std::string& dsp_content,
    int argc,
    const char* argv[],
    std::string& sha_key,
    std::string& error_msg)
{
    char sha_buf[64] = {};
    char err_buf[4096] = {};
    char* raw = expandCInterpreterDSPFromString(
        name_app.c_str(), dsp_content.c_str(), argc, argv, sha_buf, err_buf);
    if (!raw) {
        error_msg = err_buf;
        sha_key.clear();
        return {};
    }
    sha_key = sha_buf;
    error_msg.clear();
    std::string result(raw);
    freeCMemory(raw);
    return result;
}

/**
 * Generate auxiliary output files from a Faust DSP source file.
 *
 * Output formats are selected by argv flags: -cpp, -c, -wasm, -json, -svg.
 * Output directory is taken from -O <path> (defaults to ".").
 *
 * @param filename   path to the .dsp file
 * @param argc       number of compiler arguments
 * @param argv       compiler argument array
 * @param error_msg  receives an error description on failure
 * @return true on success
 */
inline bool generateInterpreterAuxFilesFromFile(
    const std::string& filename,
    int argc,
    const char* argv[],
    std::string& error_msg)
{
    char buf[4096] = {};
    bool ok = generateCInterpreterAuxFilesFromFile(filename.c_str(), argc, argv, buf);
    if (!ok) error_msg = buf;
    else error_msg.clear();
    return ok;
}

/**
 * Generate auxiliary output files from a Faust DSP source string.
 *
 * Output formats are selected by argv flags: -cpp, -c, -wasm, -json, -svg.
 * Output directory is taken from -O <path> (defaults to ".").
 *
 * @param name_app     logical DSP name
 * @param dsp_content  Faust source text
 * @param argc         number of compiler arguments
 * @param argv         compiler argument array
 * @param error_msg    receives an error description on failure
 * @return true on success
 */
inline bool generateInterpreterAuxFilesFromString(
    const std::string& name_app,
    const std::string& dsp_content,
    int argc,
    const char* argv[],
    std::string& error_msg)
{
    char buf[4096] = {};
    bool ok = generateCInterpreterAuxFilesFromString(
        name_app.c_str(), dsp_content.c_str(), argc, argv, buf);
    if (!ok) error_msg = buf;
    else error_msg.clear();
    return ok;
}

#endif // __cplusplus
#endif // INTERPRETER_DSP_H

/************************** END interpreter-dsp.h **************************/
