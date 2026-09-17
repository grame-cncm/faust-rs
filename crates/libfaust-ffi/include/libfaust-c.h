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

/**
 * Return the complete text of the last error reported on the calling thread
 * through an `error_msg` buffer of this header: the message that buffer
 * received, followed by the compiler's rendered diagnostics (location, source
 * snippet, notes, fixes) when the failure had some. `error_msg` is 4096 bytes
 * by contract and truncates; this text is whole.
 *
 * An addition of the Rust port: the reference libfaust has no equivalent. The
 * backend headers have their own (`getCCompleteCraneliftDSPFactoryError`,
 * `getCCompleteInterpreterDSPFactoryError`), each for its own entry points.
 *
 * The pointer is owned by the library: do NOT free it, not with freeCMemory
 * either. It is NULL while no error was reported on this thread and stays
 * valid until the next error reported on this thread. A successful call does
 * not reset it: read it after a call that failed.
 */
const char* getCCompleteDSPError(void);

/**
 * Return the typed form of the last error reported on the calling thread
 * through an `error_msg` buffer of this API: the compiler's diagnostics-v2 JSON report.
 * For each diagnostic: its `code` (FRS-...), its `labels` with byte ranges in
 * each of `sources[]`, its `facts`, `notes` and `help`, and its `fixes`, each
 * with its `edits` (range, replacement) and its `applicability`, so that a
 * machine-applicable fix is applied without reading the rendered text.
 *
 * An addition of this port: the reference libfaust has no equivalent.
 *
 * It is NULL when that error carried no typed diagnostics (an argument error),
 * even if an earlier one did: a report never outlives the failure it
 * describes. Otherwise the contract of the complete text above: owned by the
 * library (do NOT free it), per thread, valid until the next error reported on
 * this thread, not reset by a successful call.
 *
 * The document carries its own `schema_version` (2 today); fields may be added
 * within a version, so read what you know and check the version rather than
 * assuming it. `request.backend` names the surface that failed ("libfaust").
 */
const char* getCDSPErrorDiagnostics(void);

#ifdef __cplusplus
}
#endif

#endif
