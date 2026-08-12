#ifndef LIBFAUST_C_H
#define LIBFAUST_C_H

/*
 * C interface for the backend-agnostic libfaust API.
 *
 * This header mirrors the Faust C++ `architecture/faust/dsp/libfaust-c.h`
 * surface maintained by GRAME, adapted to the Rust port's unified `faust-ffi`
 * library.
 *
 * Buffer contracts, unchanged from the reference API:
 * - `sha_key` is caller-allocated and at least 64 bytes; it receives the
 *   40-character uppercase SHA-1 hex digest plus a terminating NUL.
 * - `error_msg` is caller-allocated and at least 4096 bytes.
 * Both may be null, in which case the corresponding output is discarded.
 *
 * Every returned `const char*` is heap-allocated and must be released with
 * freeCMemory(), declared in the backend headers of this same distribution.
 */

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Compute a SHA-1 key from a string.
 *
 * @param data - the string to be converted into a SHA-1 key
 * @param sha_key - a 64-character buffer filled with the computed key
 */
void generateCSHA1(const char* data, char* sha_key);

/*
 * Expand a DSP source into a self-contained DSP where all library imports have
 * been inlined, starting from a filename.
 *
 * @param filename - the DSP filename
 * @param argc - the number of parameters in the argv array
 * @param argv - the array of parameters (aux-file generation options such as
 *               -svg are not honored here; use generateCAuxFilesXX)
 * @param sha_key - a SHA key filled for the resulting DSP
 * @param error_msg - the error string to be filled
 *
 * @return the expanded DSP, or NULL on failure (free with freeCMemory).
 */
const char* expandCDSPFromFile(const char* filename, int argc, const char* argv[], char* sha_key,
                               char* error_msg);

/*
 * Expand a DSP source into a self-contained DSP where all library imports have
 * been inlined, starting from a string.
 *
 * @param name_app - the name of the Faust program
 * @param dsp_content - the Faust program as a string
 * @param argc - the number of parameters in the argv array
 * @param argv - the array of parameters (aux-file generation options such as
 *               -svg are not honored here; use generateCAuxFilesXX)
 * @param sha_key - a SHA key filled for the resulting DSP
 * @param error_msg - the error string to be filled
 *
 * @return the expanded DSP, or NULL on failure (free with freeCMemory).
 */
const char* expandCDSPFromString(const char* name_app, const char* dsp_content, int argc,
                                 const char* argv[], char* sha_key, char* error_msg);

/*
 * Generate additional files (other backends, SVG, JSON...) from a filename.
 *
 * @param filename - the DSP filename
 * @param argc - the number of parameters in the argv array
 * @param argv - the array of parameters; -O <path> selects the output directory
 * @param error_msg - the error string to be filled
 *
 * @return true on success, false with an error message on failure.
 */
bool generateCAuxFilesFromFile(const char* filename, int argc, const char* argv[],
                               char* error_msg);

/*
 * Generate one additional file from a filename and return it as a string.
 *
 * Exactly one output must be requested; asking for none or several is an
 * error, since this entry point delivers a single string.
 *
 * @param filename - the DSP filename
 * @param argc - the number of parameters in the argv array
 * @param argv - the array of parameters
 * @param error_msg - the error string to be filled
 *
 * @return the result, or NULL on failure (free with freeCMemory).
 */
const char* generateCAuxFilesFromFile2(const char* filename, int argc, const char* argv[],
                                       char* error_msg);

/*
 * Generate additional files (other backends, SVG, JSON...) from a string.
 *
 * @param name_app - the name of the Faust program
 * @param dsp_content - the Faust program as a string
 * @param argc - the number of parameters in the argv array
 * @param argv - the array of parameters; -O <path> selects the output directory
 * @param error_msg - the error string to be filled
 *
 * @return true on success, false with an error message on failure.
 */
bool generateCAuxFilesFromString(const char* name_app, const char* dsp_content, int argc,
                                 const char* argv[], char* error_msg);

/*
 * Generate one additional file from a string and return it as a string.
 *
 * @param name_app - the name of the Faust program
 * @param dsp_content - the Faust program as a string
 * @param argc - the number of parameters in the argv array
 * @param argv - the array of parameters
 * @param error_msg - the error string to be filled
 *
 * @return the result, or NULL on failure (free with freeCMemory).
 */
const char* generateCAuxFilesFromString2(const char* name_app, const char* dsp_content, int argc,
                                         const char* argv[], char* error_msg);

#ifdef __cplusplus
}
#endif

#endif
