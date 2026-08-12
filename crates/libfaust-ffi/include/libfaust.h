#ifndef LIBFAUST_H
#define LIBFAUST_H

/*
 * C++ convenience interface for the backend-agnostic libfaust API.
 *
 * The reference header (`compiler/generator/libfaust.h`) declares functions
 * taking and returning `std::string`. Those cannot be exported from Rust: the
 * symbol names are mangled and `std::string` has no stable ABI. This Rust-port
 * header therefore keeps the same shape as thin inline wrappers over
 * `libfaust-c.h`, exactly as `libfaust-box.h` does for the Box API, preserving
 * the C ABI as the single implementation boundary.
 *
 * Each wrapper owns the `const char*` it receives and releases it through
 * freeCMemory() before returning, so callers see only `std::string` and no
 * allocation crosses back over the boundary.
 */

#include "libfaust-c.h"

#ifdef __cplusplus
#include <string>

/*
 * freeCMemory() is exported once for the whole distribution by the backend
 * FFI layer. Declaring it here keeps this header self-sufficient for callers
 * that include no backend header.
 */
#ifndef LIBFAUST_FREE_C_MEMORY_DECLARED
#define LIBFAUST_FREE_C_MEMORY_DECLARED
extern "C" void freeCMemory(void* ptr);
#endif

/*
 * Buffer sizes fixed by the C API contract. They are wrapper-local because a
 * C++ caller of this header never sees the raw buffers.
 */
#define LIBFAUST_SHA_KEY_SIZE 64
#define LIBFAUST_ERROR_MSG_SIZE 4096

/* Adopts one C string result, releasing it through the library's allocator. */
inline std::string libfaustAdoptString(const char* owned)
{
    if (owned == nullptr) {
        return std::string();
    }
    std::string result(owned);
    freeCMemory(const_cast<char*>(owned));
    return result;
}

/**
 * Generate a SHA-1 key from a string.
 */
inline std::string generateSHA1(const std::string& data)
{
    char key[LIBFAUST_SHA_KEY_SIZE] = {0};
    generateCSHA1(data.c_str(), key);
    return std::string(key);
}

/**
 * Expand a DSP source into a self-contained DSP, starting from a filename.
 *
 * Returns the expanded DSP, or an empty string on failure with error_msg set.
 */
inline std::string expandDSPFromFile(const std::string& filename, int argc, const char* argv[],
                                     std::string& sha_key, std::string& error_msg)
{
    char key[LIBFAUST_SHA_KEY_SIZE] = {0};
    char error[LIBFAUST_ERROR_MSG_SIZE] = {0};
    std::string result =
        libfaustAdoptString(expandCDSPFromFile(filename.c_str(), argc, argv, key, error));
    sha_key = key;
    error_msg = error;
    return result;
}

/**
 * Expand a DSP source into a self-contained DSP, starting from a string.
 *
 * Returns the expanded DSP, or an empty string on failure with error_msg set.
 */
inline std::string expandDSPFromString(const std::string& name_app, const std::string& dsp_content,
                                       int argc, const char* argv[], std::string& sha_key,
                                       std::string& error_msg)
{
    char key[LIBFAUST_SHA_KEY_SIZE] = {0};
    char error[LIBFAUST_ERROR_MSG_SIZE] = {0};
    std::string result = libfaustAdoptString(
        expandCDSPFromString(name_app.c_str(), dsp_content.c_str(), argc, argv, key, error));
    sha_key = key;
    error_msg = error;
    return result;
}

/**
 * Generate additional files (other backends, SVG, JSON...) from a filename.
 */
inline bool generateAuxFilesFromFile(const std::string& filename, int argc, const char* argv[],
                                     std::string& error_msg)
{
    char error[LIBFAUST_ERROR_MSG_SIZE] = {0};
    bool ok = generateCAuxFilesFromFile(filename.c_str(), argc, argv, error);
    error_msg = error;
    return ok;
}

/**
 * Generate one additional file from a filename and return it as a string.
 */
inline std::string generateAuxFilesFromFile2(const std::string& filename, int argc,
                                             const char* argv[], std::string& error_msg)
{
    char error[LIBFAUST_ERROR_MSG_SIZE] = {0};
    std::string result =
        libfaustAdoptString(generateCAuxFilesFromFile2(filename.c_str(), argc, argv, error));
    error_msg = error;
    return result;
}

/**
 * Generate additional files (other backends, SVG, JSON...) from a string.
 */
inline bool generateAuxFilesFromString(const std::string& name_app, const std::string& dsp_content,
                                       int argc, const char* argv[], std::string& error_msg)
{
    char error[LIBFAUST_ERROR_MSG_SIZE] = {0};
    bool ok =
        generateCAuxFilesFromString(name_app.c_str(), dsp_content.c_str(), argc, argv, error);
    error_msg = error;
    return ok;
}

/**
 * Generate one additional file from a string and return it as a string.
 */
inline std::string generateAuxFilesFromString2(const std::string& name_app,
                                               const std::string& dsp_content, int argc,
                                               const char* argv[], std::string& error_msg)
{
    char error[LIBFAUST_ERROR_MSG_SIZE] = {0};
    std::string result = libfaustAdoptString(
        generateCAuxFilesFromString2(name_app.c_str(), dsp_content.c_str(), argc, argv, error));
    error_msg = error;
    return result;
}

#endif /* __cplusplus */

#endif
