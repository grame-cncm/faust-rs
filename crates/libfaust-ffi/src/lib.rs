//! Backend-agnostic libfaust C API — `generateSHA1`, `expandDSP*`,
//! `generateAuxFiles*`.
//!
//! # Source provenance (C++)
//! - `compiler/generator/libfaust.h` (the C++ surface)
//! - `architecture/faust/dsp/libfaust-c.h` (the C surface this crate exports)
//! - `compiler/generator/dsp_aux.cpp` (implementations)
//!
//! # Why this is its own crate
//! These entry points belong to no backend: expansion and auxiliary-file
//! generation run the shared front end, and the SHA key is computed from text.
//! The existing per-backend spellings (`expandCInterpreterDSPFromString`,
//! `expandCCraneliftDSPFromString`) are the same operation reached through a
//! backend-specific name; this crate provides the names the reference API
//! actually documents.
//!
//! An rlib rather than code inside `faust-ffi` because `faust-ffi` is a
//! cdylib/staticlib aggregator with no Rust of its own, and a separate library
//! keeps this surface testable in isolation.
//!
//! # Memory contract
//! Every returned `char*` is allocated by [`ffi_common::alloc_c_string`] and
//! must be released with `freeCMemory`, which this crate does **not** define:
//! `interp-ffi` exports it unconditionally for the whole distribution, and a
//! second definition would collide at link time.
//!
//! # Buffer contract
//! `sha_key` is a caller-provided buffer of at least 64 bytes and `error_msg`
//! at least 4096, matching `libfaust-c.h`. Both may be null, in which case the
//! corresponding output is dropped rather than written.

use std::ffi::{c_char, c_int};

use compiler::{Compiler as FaustCompiler, ExpandDspRequest, GenerateAuxFilesRequest};
use ffi_common::{
    alloc_c_string, decode_c_argv, optional_c_string_arg, required_c_string_arg, sha1_hex,
    write_error_4096,
};

mod aux_files;

use aux_files::{single_artifact_text, write_artifacts_to_disk};

/// Default program name when a caller passes null or an empty `name_app`.
///
/// Mirrors the fallback the other FFI surfaces in this workspace use, so the
/// same anonymous source produces the same diagnostics whichever entry point
/// compiled it.
const DEFAULT_APP_NAME: &str = "FaustDSP";

/// Computes the SHA-1 key of `data` into `sha_key`.
///
/// Mirrors C++ `generateCSHA1`. The digest is 40 uppercase hex characters
/// followed by a NUL; the rest of the 64-byte buffer is left untouched, as in
/// C++.
///
/// # Safety
/// - `data` must be a valid null-terminated C string.
/// - `sha_key` may be null; otherwise it must reference at least 64 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generateCSHA1(data: *const c_char, sha_key: *mut c_char) {
    unsafe {
        let Ok(data) = required_c_string_arg(data, "data") else {
            return;
        };
        write_sha_key(sha_key, &data);
    }
}

/// Expands a DSP file into a self-contained program.
///
/// Mirrors C++ `expandCDSPFromFile`. Returns a heap C string the caller frees
/// with `freeCMemory`, or null on failure with `error_msg` filled.
///
/// # Safety
/// - `filename` must be a valid null-terminated C string.
/// - `argv` must point to `argc` valid C strings (or be null when `argc == 0`).
/// - `sha_key` may be null; otherwise it must reference at least 64 bytes.
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expandCDSPFromFile(
    filename: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    sha_key: *mut c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    unsafe {
        let Some(filename) = take_required(filename, "filename", error_msg) else {
            return std::ptr::null_mut();
        };
        let Some(args) = take_argv(argc, argv, error_msg) else {
            return std::ptr::null_mut();
        };
        let source = match std::fs::read_to_string(&filename) {
            Ok(source) => source,
            Err(error) => {
                write_error(error_msg, &format!("cannot read '{filename}': {error}"));
                return std::ptr::null_mut();
            }
        };
        expand(&filename, source, &args, sha_key, error_msg)
    }
}

/// Expands a DSP source string into a self-contained program.
///
/// Mirrors C++ `expandCDSPFromString`.
///
/// # Safety
/// - `name_app` may be null; otherwise it must be a valid C string.
/// - `dsp_content` must be a valid null-terminated C string.
/// - `argv` must point to `argc` valid C strings (or be null when `argc == 0`).
/// - `sha_key` may be null; otherwise it must reference at least 64 bytes.
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expandCDSPFromString(
    name_app: *const c_char,
    dsp_content: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    sha_key: *mut c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    unsafe {
        let Some(name_app) = take_optional_name(name_app, error_msg) else {
            return std::ptr::null_mut();
        };
        let Some(source) = take_required(dsp_content, "dsp_content", error_msg) else {
            return std::ptr::null_mut();
        };
        let Some(args) = take_argv(argc, argv, error_msg) else {
            return std::ptr::null_mut();
        };
        expand(&name_app, source, &args, sha_key, error_msg)
    }
}

/// Generates auxiliary files from a DSP file, writing them to disk.
///
/// Mirrors C++ `generateCAuxFilesFromFile`. The output directory comes from
/// `-O <path>` in `argv` and defaults to the working directory.
///
/// # Safety
/// - `filename` must be a valid null-terminated C string.
/// - `argv` must point to `argc` valid C strings (or be null when `argc == 0`).
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generateCAuxFilesFromFile(
    filename: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> bool {
    unsafe {
        let Some((name, source, args)) = file_request(filename, argc, argv, error_msg) else {
            return false;
        };
        match aux_files(&name, source, &args) {
            Ok(artifacts) => write_artifacts_to_disk(&artifacts, &args, error_msg),
            Err(message) => {
                write_error(error_msg, &message);
                false
            }
        }
    }
}

/// Generates auxiliary files from a DSP file, returning the single result.
///
/// Mirrors C++ `generateCAuxFilesFromFile2`. Returns a heap C string the
/// caller frees with `freeCMemory`, or null on failure.
///
/// # Safety
/// Same as [`generateCAuxFilesFromFile`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generateCAuxFilesFromFile2(
    filename: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    unsafe {
        let Some((name, source, args)) = file_request(filename, argc, argv, error_msg) else {
            return std::ptr::null_mut();
        };
        aux_files_text(&name, source, &args, error_msg)
    }
}

/// Generates auxiliary files from a DSP string, writing them to disk.
///
/// Mirrors C++ `generateCAuxFilesFromString`.
///
/// # Safety
/// - `name_app` may be null; otherwise it must be a valid C string.
/// - `dsp_content` must be a valid null-terminated C string.
/// - `argv` must point to `argc` valid C strings (or be null when `argc == 0`).
/// - `error_msg` may be null; otherwise it must reference at least 4096 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generateCAuxFilesFromString(
    name_app: *const c_char,
    dsp_content: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> bool {
    unsafe {
        let Some((name, source, args)) =
            string_request(name_app, dsp_content, argc, argv, error_msg)
        else {
            return false;
        };
        match aux_files(&name, source, &args) {
            Ok(artifacts) => write_artifacts_to_disk(&artifacts, &args, error_msg),
            Err(message) => {
                write_error(error_msg, &message);
                false
            }
        }
    }
}

/// Generates auxiliary files from a DSP string, returning the single result.
///
/// Mirrors C++ `generateCAuxFilesFromString2`.
///
/// # Safety
/// Same as [`generateCAuxFilesFromString`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generateCAuxFilesFromString2(
    name_app: *const c_char,
    dsp_content: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    unsafe {
        let Some((name, source, args)) =
            string_request(name_app, dsp_content, argc, argv, error_msg)
        else {
            return std::ptr::null_mut();
        };
        aux_files_text(&name, source, &args, error_msg)
    }
}

// ── Shared request handling ───────────────────────────────────────────────────

/// Runs one expansion and marshals the result across the boundary.
unsafe fn expand(
    name: &str,
    source: String,
    args: &[String],
    sha_key: *mut c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    let request = ExpandDspRequest {
        source_name: name.to_owned(),
        source,
        args: args.join(" "),
    };
    match FaustCompiler::new().expand_dsp(&request) {
        Ok(expanded) => {
            // C++ keys the expansion, not the input: two sources that expand
            // to the same program must share a cache entry.
            unsafe { write_sha_key(sha_key, &expanded) };
            alloc_c_string(&expanded)
        }
        Err(error) => {
            unsafe { write_error(error_msg, &error.message) };
            std::ptr::null_mut()
        }
    }
}

/// Runs one auxiliary-file generation, returning artifacts or a message.
fn aux_files(
    name: &str,
    source: String,
    args: &[String],
) -> Result<Vec<compiler::AuxFileArtifact>, String> {
    let request = GenerateAuxFilesRequest {
        source_name: name.to_owned(),
        source,
        args: args.join(" "),
        ..Default::default()
    };
    FaustCompiler::new()
        .generate_aux_files(&request)
        .map_err(|error| error.message)
}

/// Auxiliary-file generation for the `*2` entry points, which return text.
unsafe fn aux_files_text(
    name: &str,
    source: String,
    args: &[String],
    error_msg: *mut c_char,
) -> *mut c_char {
    match aux_files(name, source, args) {
        Ok(artifacts) => match single_artifact_text(&artifacts) {
            Ok(text) => alloc_c_string(&text),
            Err(message) => {
                unsafe { write_error(error_msg, &message) };
                std::ptr::null_mut()
            }
        },
        Err(message) => {
            unsafe { write_error(error_msg, &message) };
            std::ptr::null_mut()
        }
    }
}

/// Decodes the `(filename, argv)` argument pair and reads the file.
unsafe fn file_request(
    filename: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> Option<(String, String, Vec<String>)> {
    unsafe {
        let filename = take_required(filename, "filename", error_msg)?;
        let args = take_argv(argc, argv, error_msg)?;
        match std::fs::read_to_string(&filename) {
            Ok(source) => Some((filename, source, args)),
            Err(error) => {
                write_error(error_msg, &format!("cannot read '{filename}': {error}"));
                None
            }
        }
    }
}

/// Decodes the `(name_app, dsp_content, argv)` argument triple.
unsafe fn string_request(
    name_app: *const c_char,
    dsp_content: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> Option<(String, String, Vec<String>)> {
    unsafe {
        let name = take_optional_name(name_app, error_msg)?;
        let source = take_required(dsp_content, "dsp_content", error_msg)?;
        let args = take_argv(argc, argv, error_msg)?;
        Some((name, source, args))
    }
}

/// Decodes one required C string, reporting through `error_msg` on failure.
unsafe fn take_required(
    value: *const c_char,
    field: &str,
    error_msg: *mut c_char,
) -> Option<String> {
    match unsafe { required_c_string_arg(value, field) } {
        Ok(value) => Some(value),
        Err(message) => {
            unsafe { write_error(error_msg, &message) };
            None
        }
    }
}

/// Decodes `name_app`, substituting the default for null or empty.
unsafe fn take_optional_name(name_app: *const c_char, error_msg: *mut c_char) -> Option<String> {
    match unsafe { optional_c_string_arg(name_app, "name_app") } {
        Ok(Some(name)) if !name.is_empty() => Some(name),
        Ok(_) => Some(DEFAULT_APP_NAME.to_owned()),
        Err(message) => {
            unsafe { write_error(error_msg, &message) };
            None
        }
    }
}

/// Decodes the argument vector, reporting through `error_msg` on failure.
unsafe fn take_argv(
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> Option<Vec<String>> {
    match unsafe { decode_c_argv(argc, argv) } {
        Ok(args) => Some(args),
        Err(message) => {
            unsafe { write_error(error_msg, &message) };
            None
        }
    }
}

/// Writes one message into the caller's 4096-byte error buffer.
unsafe fn write_error(error_msg: *mut c_char, message: &str) {
    unsafe { write_error_4096(error_msg, message) };
}

/// Writes the SHA-1 key of `text` into the caller's 64-byte buffer.
unsafe fn write_sha_key(sha_key: *mut c_char, text: &str) {
    if sha_key.is_null() {
        return;
    }
    let key = sha1_hex(text.as_bytes());
    let bytes = key.as_bytes();
    let len = bytes.len().min(63);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), sha_key, len);
        *sha_key.add(len) = 0;
    }
}
