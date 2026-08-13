/************************** BEGIN cranelift-dsp.h **************************
 * C++ wrapper for the Faust Cranelift backend — Rust port
 * (modeled after faust/architecture/faust/dsp/interpreter-dsp.h)
 *
 * This header provides C++ classes that wrap the Cranelift C API and is
 * self-contained (it does not include `cranelift-dsp-c.h`).
 * C projects should include `cranelift-dsp-c.h` directly.
 ************************************************************************/

#ifndef CRANELIFT_DSP_H
#define CRANELIFT_DSP_H

#ifdef _WIN32
#define DEPRECATED(fun) __declspec(deprecated) fun
#else
#define DEPRECATED(fun) fun __attribute__((deprecated));
#endif

#include <string>
#include <vector>
#include <cstring>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include "faust/gui/CInterface.h"
#include "faust/gui/CGlue.h"
#include "faust-memory-manager.h"

// Self-contained C API declarations (mirrors crates/cranelift-ffi/include/cranelift-dsp-c.h)
// This header intentionally does not include cranelift-dsp-c.h so it can be
// used standalone from C++ codebases expecting a single include.

#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif

#ifdef __cplusplus
extern "C" {
#endif

// Opaque C API types
#ifdef _MSC_VER
typedef void ccranelift_dsp_factory;
typedef void ccranelift_dsp;
#else
typedef struct CraneliftDspFactory ccranelift_dsp_factory;
typedef struct CraneliftDspInstance ccranelift_dsp;
#endif

// C API functions used by this C++ wrapper
const char* getCLibFaustVersion(void);

ccranelift_dsp_factory* getCCraneliftDSPFactoryFromSHAKey(const char* sha_key);
ccranelift_dsp_factory* readCCraneliftDSPFactoryFromBitcode(
    const char* bit_code, char* error_msg);
ccranelift_dsp_factory* readCCraneliftDSPFactoryFromBitcodeFile(
    const char* bit_code_path, char* error_msg);
ccranelift_dsp_factory* createCCraneliftDSPFactoryFromFile(
    const char* filename, int argc, const char* argv[], char* error_msg, int opt_level);
ccranelift_dsp_factory* createCCraneliftDSPFactoryFromString(
    const char* name_app, const char* dsp_content,
    int argc, const char* argv[], char* error_msg, int opt_level);
char* writeCCraneliftDSPFactoryToBitcode(ccranelift_dsp_factory* factory);
bool writeCCraneliftDSPFactoryToBitcodeFile(
    ccranelift_dsp_factory* factory, const char* bit_code_path);
bool deleteCCraneliftDSPFactory(ccranelift_dsp_factory* factory);
bool setCCraneliftMemoryManager(ccranelift_dsp_factory* factory,
                                const faust_memory_manager* manager,
                                char* error_msg);
void deleteAllCCraneliftDSPFactories(void);
char** getAllCCraneliftDSPFactories(void);
char* getCCraneliftDSPFactoryName(ccranelift_dsp_factory* factory);
char* getCCraneliftDSPFactorySHAKey(ccranelift_dsp_factory* factory);
char* getCCraneliftDSPFactoryDSPCode(ccranelift_dsp_factory* factory);
char* getCCraneliftDSPFactoryJSON(ccranelift_dsp_factory* factory);
char* getCCraneliftDSPFactoryCompileOptions(ccranelift_dsp_factory* factory);
const char** getCCraneliftDSPFactoryLibraryList(ccranelift_dsp_factory* factory);
const char** getCCraneliftDSPFactoryIncludePathnames(ccranelift_dsp_factory* factory);
const char** getCCraneliftDSPFactoryWarningMessages(ccranelift_dsp_factory* factory);
bool startMTDSPFactories(void);
void stopMTDSPFactories(void);
void freeCMemory(void* ptr);

ccranelift_dsp* createCCraneliftDSPInstance(ccranelift_dsp_factory* factory);
void deleteCCraneliftDSPInstance(ccranelift_dsp* dsp);
ccranelift_dsp* cloneCCraneliftDSPInstance(ccranelift_dsp* dsp);

int getNumInputsCCraneliftDSPInstance(ccranelift_dsp* dsp);
int getNumOutputsCCraneliftDSPInstance(ccranelift_dsp* dsp);
int getSampleRateCCraneliftDSPInstance(ccranelift_dsp* dsp);
void initCCraneliftDSPInstance(ccranelift_dsp* dsp, int sample_rate);
void instanceInitCCraneliftDSPInstance(ccranelift_dsp* dsp, int sample_rate);
void instanceConstantsCCraneliftDSPInstance(ccranelift_dsp* dsp, int sample_rate);
void instanceResetUserInterfaceCCraneliftDSPInstance(ccranelift_dsp* dsp);
void instanceClearCCraneliftDSPInstance(ccranelift_dsp* dsp);
void buildUserInterfaceCCraneliftDSPInstance(ccranelift_dsp* dsp, UIGlue* glue);
void metadataCCraneliftDSPInstance(ccranelift_dsp* dsp, MetaGlue* meta);
void computeCCraneliftDSPInstance(ccranelift_dsp* dsp, int count,
                                  FAUSTFLOAT** inputs, FAUSTFLOAT** outputs);
void registerCCraneliftForeignFunction(const char* name, void* fn_ptr);
void unregisterCCraneliftForeignFunction(const char* name);
void clearCCraneliftForeignFunctions(void);

char* expandCCraneliftDSPFromFile(
    const char* filename, int argc, const char* argv[],
    char* sha_key, char* error_msg);
char* expandCCraneliftDSPFromString(
    const char* name_app, const char* dsp_content, int argc, const char* argv[],
    char* sha_key, char* error_msg);
bool generateCCraneliftAuxFilesFromFile(
    const char* filename, int argc, const char* argv[], char* error_msg);
bool generateCCraneliftAuxFilesFromString(
    const char* name_app, const char* dsp_content, int argc, const char* argv[],
    char* error_msg);

#ifdef __cplusplus
}
#endif

// ── Compatibility note ─────────────────────────────────────────────────────
// This self-contained header reuses `faust/gui/CInterface.h` for UIGlue/MetaGlue
// definitions to remain source-compatible with `CGlue.h`, `GTKUI`, JACK helpers,
// etc., while still embedding the Cranelift C API declarations directly.
// ───────────────────────────────────────────────────────────────────────────

#ifdef __cplusplus

namespace cranelift_dsp_detail {

inline std::string from_owned_c_string(char* raw)
{
    if (!raw) {
        return {};
    }
    std::string result(raw);
    freeCMemory(raw);
    return result;
}

inline std::vector<std::string> from_c_string_array(const char** items)
{
    std::vector<std::string> result;
    if (!items) {
        return result;
    }
    for (const char** it = items; *it != nullptr; ++it) {
        result.emplace_back(*it);
    }
    return result;
}

inline std::vector<std::string> from_owned_c_string_array(char** items)
{
    std::vector<std::string> result;
    if (!items) {
        return result;
    }
    for (char** it = items; *it != nullptr; ++it) {
        result.emplace_back(*it);
        freeCMemory(*it);
    }
    freeCMemory(items);
    return result;
}

inline void copy_error_message(std::string& error_msg, const char* buffer)
{
    error_msg = buffer ? buffer : "";
}

} // namespace cranelift_dsp_detail

class cranelift_dsp;

// ── cranelift_dsp_factory ──────────────────────────────────────────────────

/**
 * C++ wrapper for the Cranelift DSP factory.
 *
 * Instances are obtained via the free functions below.
 * Each returned wrapper owns one cache reference; release it with
 * deleteCraneliftDSPFactory().
 */
class LIBFAUST_API cranelift_dsp_factory : public dsp_factory {
public:
    explicit cranelift_dsp_factory(ccranelift_dsp_factory* impl)
        : impl_(impl), memory_manager_(nullptr)
    {
        memory_api_.abi_version = FAUST_MEMORY_MANAGER_ABI_VERSION;
        memory_api_.struct_size = sizeof(memory_api_);
        memory_api_.context = this;
        memory_api_.begin = memory_begin;
        memory_api_.info = memory_info;
        memory_api_.end = memory_end;
        memory_api_.allocate = memory_allocate;
        memory_api_.destroy = memory_destroy;
    }

    ~cranelift_dsp_factory() override = default;

    cranelift_dsp_factory(const cranelift_dsp_factory&) = delete;
    cranelift_dsp_factory& operator=(const cranelift_dsp_factory&) = delete;

    ccranelift_dsp_factory* get() const { return impl_; }

    std::string getName() override
    {
        return cranelift_dsp_detail::from_owned_c_string(
            getCCraneliftDSPFactoryName(impl_));
    }

    std::string getSHAKey() override
    {
        return cranelift_dsp_detail::from_owned_c_string(
            getCCraneliftDSPFactorySHAKey(impl_));
    }

    std::string getDSPCode() override
    {
        return cranelift_dsp_detail::from_owned_c_string(
            getCCraneliftDSPFactoryDSPCode(impl_));
    }

    std::string getJSON() override
    {
        return cranelift_dsp_detail::from_owned_c_string(
            getCCraneliftDSPFactoryJSON(impl_));
    }

    std::string getCompileOptions() override
    {
        return cranelift_dsp_detail::from_owned_c_string(
            getCCraneliftDSPFactoryCompileOptions(impl_));
    }

    std::vector<std::string> getLibraryList() override
    {
        return cranelift_dsp_detail::from_c_string_array(
            getCCraneliftDSPFactoryLibraryList(impl_));
    }

    std::vector<std::string> getIncludePathnames() override
    {
        return cranelift_dsp_detail::from_c_string_array(
            getCCraneliftDSPFactoryIncludePathnames(impl_));
    }

    std::vector<std::string> getWarningMessages() override
    {
        return cranelift_dsp_detail::from_c_string_array(
            getCCraneliftDSPFactoryWarningMessages(impl_));
    }

    ::dsp* createDSPInstance() override;

    void setMemoryManager(dsp_memory_manager* manager) override
    {
        char error_msg[4096] = {0};
        dsp_memory_manager* previous = memory_manager_;
        memory_manager_ = manager;
        if (!manager || !setCCraneliftMemoryManager(impl_, &memory_api_, error_msg)) {
            memory_manager_ = previous;
        }
    }

    dsp_memory_manager* getMemoryManager() override { return memory_manager_; }

private:
    static cranelift_dsp_factory* self(void* context)
    {
        return static_cast<cranelift_dsp_factory*>(context);
    }

    static void memory_begin(void* context, size_t count)
    {
        self(context)->memory_manager_->begin(count);
    }

    static void memory_info(void* context, const char* name,
                            faust_memory_type type, size_t count,
                            size_t size_bytes, size_t /*alignment*/,
                            uint64_t reads, uint64_t writes)
    {
        self(context)->memory_manager_->info(
            name, legacy_memory_type(type), count,
            size_bytes, static_cast<size_t>(reads), static_cast<size_t>(writes));
    }

    static dsp_memory_manager::MemType legacy_memory_type(faust_memory_type type)
    {
        switch (type) {
            case kMemInt64:
            case kMemBool:
                return dsp_memory_manager::kObj;
            case kMemInt64Ptr:
            case kMemBoolPtr:
                return dsp_memory_manager::kObj_ptr;
            default:
                return static_cast<dsp_memory_manager::MemType>(type);
        }
    }

    static void memory_end(void* context)
    {
        self(context)->memory_manager_->end();
    }

    static void* memory_allocate(void* context, size_t size_bytes, size_t /*alignment*/)
    {
        return self(context)->memory_manager_->allocate(size_bytes);
    }

    static void memory_destroy(void* context, void* address,
                               size_t /*size_bytes*/, size_t /*alignment*/)
    {
        self(context)->memory_manager_->destroy(address);
    }

    ccranelift_dsp_factory* impl_;
    dsp_memory_manager* memory_manager_;
    faust_memory_manager memory_api_;
};

// ── cranelift_dsp ──────────────────────────────────────────────────────────

/**
 * C++ wrapper for a Cranelift DSP instance.
 *
 * Instances are created via `cranelift_dsp_factory::createDSPInstance()`.
 * The caller owns the instance; call `delete` to release it.
 */
class LIBFAUST_API cranelift_dsp : public dsp {
public:
    explicit cranelift_dsp(ccranelift_dsp* impl)
        : impl_(impl)
    {}

    ~cranelift_dsp() override
    {
        if (impl_) {
            deleteCCraneliftDSPInstance(impl_);
            impl_ = nullptr;
        }
    }

    cranelift_dsp(const cranelift_dsp&) = delete;
    cranelift_dsp& operator=(const cranelift_dsp&) = delete;

    int getNumInputs() override
    {
        return getNumInputsCCraneliftDSPInstance(impl_);
    }

    int getNumOutputs() override
    {
        return getNumOutputsCCraneliftDSPInstance(impl_);
    }

    int getSampleRate() override
    {
        return getSampleRateCCraneliftDSPInstance(impl_);
    }

    void init(int sample_rate) override
    {
        initCCraneliftDSPInstance(impl_, sample_rate);
    }

    void instanceInit(int sample_rate) override
    {
        instanceInitCCraneliftDSPInstance(impl_, sample_rate);
    }

    void instanceConstants(int sample_rate) override
    {
        instanceConstantsCCraneliftDSPInstance(impl_, sample_rate);
    }

    void instanceResetUserInterface() override
    {
        instanceResetUserInterfaceCCraneliftDSPInstance(impl_);
    }

    void instanceClear() override
    {
        instanceClearCCraneliftDSPInstance(impl_);
    }

    cranelift_dsp* clone() override
    {
        ccranelift_dsp* cloned = cloneCCraneliftDSPInstance(impl_);
        return cloned ? new cranelift_dsp(cloned) : nullptr;
    }

    void buildUserInterface(UI* ui_interface) override
    {
        if (!ui_interface) {
            return;
        }
        UIGlue glue;
        buildUIGlue(&glue, ui_interface, sizeof(FAUSTFLOAT) == sizeof(double));
        buildUserInterfaceCCraneliftDSPInstance(impl_, &glue);
    }

    void metadata(Meta* meta) override
    {
        if (!meta) {
            return;
        }
        MetaGlue glue;
        buildMetaGlue(&glue, meta);
        metadataCCraneliftDSPInstance(impl_, &glue);
    }

    void buildUserInterface(UIGlue* glue)
    {
        buildUserInterfaceCCraneliftDSPInstance(impl_, glue);
    }

    void metadata(MetaGlue* meta)
    {
        metadataCCraneliftDSPInstance(impl_, meta);
    }

    void compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs) override
    {
        computeCCraneliftDSPInstance(impl_, count, inputs, outputs);
    }

    ccranelift_dsp* get() const { return impl_; }

private:
    ccranelift_dsp* impl_;
};

// ── cranelift_dsp_factory::createDSPInstance ──────────────────────────────

inline ::dsp* cranelift_dsp_factory::createDSPInstance()
{
    ccranelift_dsp* impl = createCCraneliftDSPInstance(impl_);
    return impl ? new cranelift_dsp(impl) : nullptr;
}

// ── Free functions ────────────────────────────────────────────────────────

inline cranelift_dsp_factory* getCraneliftDSPFactoryFromSHAKey(
    const std::string& sha_key)
{
    ccranelift_dsp_factory* impl =
        getCCraneliftDSPFactoryFromSHAKey(sha_key.c_str());
    return impl ? new cranelift_dsp_factory(impl) : nullptr;
}

inline cranelift_dsp_factory* createCraneliftDSPFactoryFromFile(
    const std::string& filename,
    int argc,
    const char* argv[],
    std::string& error_msg,
    int opt_level = 0)
{
    char buffer[4096] = {};
    ccranelift_dsp_factory* impl = createCCraneliftDSPFactoryFromFile(
        filename.c_str(), argc, argv, buffer, opt_level);
    if (!impl) {
        cranelift_dsp_detail::copy_error_message(error_msg, buffer);
        return nullptr;
    }
    error_msg.clear();
    return new cranelift_dsp_factory(impl);
}

inline cranelift_dsp_factory* createCraneliftDSPFactoryFromString(
    const std::string& name_app,
    const std::string& dsp_content,
    int argc,
    const char* argv[],
    std::string& error_msg,
    int opt_level = 0)
{
    char buffer[4096] = {};
    ccranelift_dsp_factory* impl = createCCraneliftDSPFactoryFromString(
        name_app.c_str(), dsp_content.c_str(), argc, argv, buffer, opt_level);
    if (!impl) {
        cranelift_dsp_detail::copy_error_message(error_msg, buffer);
        return nullptr;
    }
    error_msg.clear();
    return new cranelift_dsp_factory(impl);
}

/**
 * Release one factory reference.
 *
 * @return true only if this was the final reference
 */
inline bool deleteCraneliftDSPFactory(cranelift_dsp_factory* factory)
{
    if (!factory) {
        return false;
    }
    bool result = deleteCCraneliftDSPFactory(factory->get());
    delete factory;
    return result;
}

/**
 * Delete all factories and remaining DSP instances.
 * All previously returned wrapper payloads become invalid.
 */
inline void deleteAllCraneliftDSPFactories()
{
    deleteAllCCraneliftDSPFactories();
}

/**
 * Enable multi-thread-safe access to the global Cranelift factory cache.
 */
inline bool startCraneliftMTDSPFactories()
{
    return startMTDSPFactories();
}

/**
 * Disable multi-thread-safe access to the global Cranelift factory cache.
 */
inline void stopCraneliftMTDSPFactories()
{
    stopMTDSPFactories();
}

inline std::vector<std::string> getAllCraneliftDSPFactories()
{
    return cranelift_dsp_detail::from_owned_c_string_array(
        getAllCCraneliftDSPFactories());
}

inline cranelift_dsp_factory* readCraneliftDSPFactoryFromBitcode(
    const std::string& bit_code,
    std::string& error_msg)
{
    char buffer[4096] = {};
    ccranelift_dsp_factory* impl =
        readCCraneliftDSPFactoryFromBitcode(bit_code.c_str(), buffer);
    if (!impl) {
        cranelift_dsp_detail::copy_error_message(error_msg, buffer);
        return nullptr;
    }
    error_msg.clear();
    return new cranelift_dsp_factory(impl);
}

inline std::string writeCraneliftDSPFactoryToBitcode(
    cranelift_dsp_factory* factory)
{
    return factory
        ? cranelift_dsp_detail::from_owned_c_string(
              writeCCraneliftDSPFactoryToBitcode(factory->get()))
        : std::string();
}

inline cranelift_dsp_factory* readCraneliftDSPFactoryFromBitcodeFile(
    const std::string& bit_code_path,
    std::string& error_msg)
{
    char buffer[4096] = {};
    ccranelift_dsp_factory* impl =
        readCCraneliftDSPFactoryFromBitcodeFile(bit_code_path.c_str(), buffer);
    if (!impl) {
        cranelift_dsp_detail::copy_error_message(error_msg, buffer);
        return nullptr;
    }
    error_msg.clear();
    return new cranelift_dsp_factory(impl);
}

inline bool writeCraneliftDSPFactoryToBitcodeFile(
    cranelift_dsp_factory* factory,
    const std::string& bit_code_path)
{
    return factory
        ? writeCCraneliftDSPFactoryToBitcodeFile(factory->get(), bit_code_path.c_str())
        : false;
}

inline void registerCraneliftForeignFunction(
    const std::string& name,
    void* fn_ptr)
{
    registerCCraneliftForeignFunction(name.c_str(), fn_ptr);
}

inline void unregisterCraneliftForeignFunction(const std::string& name)
{
    unregisterCCraneliftForeignFunction(name.c_str());
}

inline void clearCraneliftForeignFunctions()
{
    clearCCraneliftForeignFunctions();
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
inline std::string expandCraneliftDSPFromFile(
    const std::string& filename,
    int argc,
    const char* argv[],
    std::string& sha_key,
    std::string& error_msg)
{
    char sha_buf[64] = {};
    char err_buf[4096] = {};
    char* raw = expandCCraneliftDSPFromFile(
        filename.c_str(), argc, argv, sha_buf, err_buf);
    if (!raw) {
        cranelift_dsp_detail::copy_error_message(error_msg, err_buf);
        sha_key.clear();
        return {};
    }
    sha_key = sha_buf;
    error_msg.clear();
    return cranelift_dsp_detail::from_owned_c_string(raw);
}

/**
 * Validate and expand a Faust DSP source string.
 *
 * On success returns the source text; on failure returns an empty string
 * and fills `error_msg`.
 *
 * @param name_app   logical DSP name
 * @param dsp_content  Faust source text
 * @param argc       number of compiler arguments
 * @param argv       compiler argument array
 * @param sha_key    receives a hex digest of the source (may be empty)
 * @param error_msg  receives an error description on failure
 * @return expanded DSP source, or empty string on failure
 */
inline std::string expandCraneliftDSPFromString(
    const std::string& name_app,
    const std::string& dsp_content,
    int argc,
    const char* argv[],
    std::string& sha_key,
    std::string& error_msg)
{
    char sha_buf[64] = {};
    char err_buf[4096] = {};
    char* raw = expandCCraneliftDSPFromString(
        name_app.c_str(), dsp_content.c_str(), argc, argv, sha_buf, err_buf);
    if (!raw) {
        cranelift_dsp_detail::copy_error_message(error_msg, err_buf);
        sha_key.clear();
        return {};
    }
    sha_key = sha_buf;
    error_msg.clear();
    return cranelift_dsp_detail::from_owned_c_string(raw);
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
inline bool generateCraneliftAuxFilesFromFile(
    const std::string& filename,
    int argc,
    const char* argv[],
    std::string& error_msg)
{
    char buf[4096] = {};
    bool ok = generateCCraneliftAuxFilesFromFile(filename.c_str(), argc, argv, buf);
    if (!ok) cranelift_dsp_detail::copy_error_message(error_msg, buf);
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
inline bool generateCraneliftAuxFilesFromString(
    const std::string& name_app,
    const std::string& dsp_content,
    int argc,
    const char* argv[],
    std::string& error_msg)
{
    char buf[4096] = {};
    bool ok = generateCCraneliftAuxFilesFromString(
        name_app.c_str(), dsp_content.c_str(), argc, argv, buf);
    if (!ok) cranelift_dsp_detail::copy_error_message(error_msg, buf);
    else error_msg.clear();
    return ok;
}

#endif // __cplusplus
#endif // CRANELIFT_DSP_H

/************************** END cranelift-dsp.h ****************************/
