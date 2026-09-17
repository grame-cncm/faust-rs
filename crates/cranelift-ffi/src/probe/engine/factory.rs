//! The JIT-compiled factory, and the text of a compilation that failed.

use std::ffi::{CStr, CString, c_char, c_int};

use crate::factory::{
    createCCraneliftDSPFactoryFromFile, createCCraneliftDSPFactoryFromString,
    deleteCCraneliftDSPFactory, getCCompleteCraneliftDSPFactoryError,
    getCCraneliftDSPFactoryErrorDiagnostics,
};
use crate::types::CraneliftDspFactory;

/// A JIT-compiled DSP factory, shared by every [`Probe`] instantiated from
/// it.
///
/// Split out from [`Probe`] so a caller — chiefly [`PolyProbe`] — can create
/// several independent instances from one compile. `double` is recorded here
/// rather than per-instance because it is a compile-time argument (`-double`
/// on the front end's own `argv`, `Probe::compile`'s doc), fixed for every
/// instance the factory produces.
pub struct Factory {
    pub(super) factory: *mut CraneliftDspFactory,
    pub(super) double: bool,
}

/// The text of a factory creation that just failed on this thread.
///
/// `error_msg` holds the one-line summary, which for a syntax error is a
/// count; the complete text adds the compiler's rendered diagnostics (location,
/// source snippet, notes, fixes), so the probe prints what `faust-rs` prints.
/// The complete text is not reset by a success, so it is taken only when it
/// extends what the buffer of *this* failure received, an empty buffer being
/// extended by nothing.
fn compile_error(error_msg: &[c_char; 4096]) -> String {
    let summary = unsafe { CStr::from_ptr(error_msg.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let complete = getCCompleteCraneliftDSPFactoryError();
    let text = if complete.is_null() {
        summary
    } else {
        let complete = unsafe { CStr::from_ptr(complete) }.to_string_lossy();
        if !summary.is_empty() && complete.starts_with(summary.as_str()) {
            complete.trim_end().to_owned()
        } else {
            summary
        }
    };
    // The typed channel of the same failure, through the entry point any host
    // of the C API has, and read here because here is where the failure is
    // known to be this one: the report is per thread and a later failure
    // replaces it (or clears it, when it has none).
    let report = getCCraneliftDSPFactoryErrorDiagnostics();
    let diagnostics_json = (!report.is_null()).then(|| {
        unsafe { CStr::from_ptr(report) }
            .to_string_lossy()
            .into_owned()
    });
    LAST_COMPILE_FAILURE.with(|last| {
        *last.borrow_mut() = Some(CompileFailure {
            text: text.clone(),
            diagnostics_json,
        });
    });
    text
}

/// A factory creation that failed: the text the caller was given and the
/// compiler's diagnostics-v2 JSON report of the same failure, when it had
/// typed diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileFailure {
    /// What `Factory::compile*` returned as its error.
    pub text: String,
    /// Code, ranges, facts and machine-applicable fixes, as one JSON document.
    pub diagnostics_json: Option<String>,
}

thread_local! {
    static LAST_COMPILE_FAILURE: std::cell::RefCell<Option<CompileFailure>> =
        const { std::cell::RefCell::new(None) };
}

/// The last factory creation that failed on this thread.
///
/// Errors travel through this module as strings, and a failure may be
/// recovered from (the polyphonic wrapper tries to extract an `effect` and
/// carries on without one): a caller that ends on an error decides whether it
/// is this failure by looking for [`CompileFailure::text`] in it.
#[must_use]
pub fn last_compile_failure() -> Option<CompileFailure> {
    LAST_COMPILE_FAILURE.with(|last| last.borrow().clone())
}

impl Factory {
    /// JIT-compile `path`.
    ///
    /// # Errors
    /// Returns the compiler's own diagnostic text when the front end or the
    /// JIT rejects the source.
    pub fn compile(
        path: &str,
        import_dirs: &[String],
        double: bool,
        opt_level: i32,
    ) -> Result<Self, String> {
        Self::compile_with_args(path, import_dirs, &[], double, opt_level)
    }

    /// [`Factory::compile`] with extra compiler arguments appended verbatim
    /// (`-bra-tape 32768`, `-mcd 64`, ...), for the options the probe does
    /// not model as flags of its own.
    ///
    /// # Errors
    /// As [`Factory::compile`].
    pub fn compile_with_args(
        path: &str,
        import_dirs: &[String],
        extra_args: &[String],
        double: bool,
        opt_level: i32,
    ) -> Result<Self, String> {
        let c_path = CString::new(path).map_err(|e| e.to_string())?;
        Self::create(import_dirs, extra_args, double, |argc, argv, err| unsafe {
            createCCraneliftDSPFactoryFromFile(c_path.as_ptr(), argc, argv, err, opt_level)
        })
    }

    /// Builds the compiler's `argv` (`-I DIR`..., `-double`, the extra
    /// arguments verbatim), calls `create` with it and with the buffer the
    /// error summary goes to, and owns the factory that comes back.
    fn create(
        import_dirs: &[String],
        extra_args: &[String],
        double: bool,
        create: impl FnOnce(c_int, *const *const c_char, *mut c_char) -> *mut CraneliftDspFactory,
    ) -> Result<Self, String> {
        let mut argv: Vec<CString> = Vec::new();
        for dir in import_dirs {
            argv.push(CString::new("-I").map_err(|e| e.to_string())?);
            argv.push(CString::new(dir.as_str()).map_err(|e| e.to_string())?);
        }
        if double {
            argv.push(CString::new("-double").map_err(|e| e.to_string())?);
        }
        for arg in extra_args {
            argv.push(CString::new(arg.as_str()).map_err(|e| e.to_string())?);
        }
        let argv_ptrs: Vec<*const c_char> = argv.iter().map(|a| a.as_ptr()).collect();
        let mut err = [0_i8; 4096];
        let factory = create(
            c_int::try_from(argv_ptrs.len()).map_err(|_| "too many -I arguments")?,
            if argv_ptrs.is_empty() {
                std::ptr::null()
            } else {
                argv_ptrs.as_ptr()
            },
            err.as_mut_ptr(),
        );
        if factory.is_null() {
            return Err(compile_error(&err));
        }
        Ok(Self { factory, double })
    }

    /// JIT-compile `source` directly, without reading a file.
    ///
    /// Used to compile the `environment{}`-wrapped effect extraction
    /// ([`PolyProbe::compile`]'s doc): the wrapper is synthesised text, not
    /// something on disk.
    ///
    /// # Errors
    /// Returns the compiler's own diagnostic text when the front end or the
    /// JIT rejects the source.
    pub fn compile_from_string(
        name: &str,
        source: &str,
        import_dirs: &[String],
        double: bool,
        opt_level: i32,
    ) -> Result<Self, String> {
        Self::compile_from_string_with_args(name, source, import_dirs, &[], double, opt_level)
    }

    /// [`Factory::compile_from_string`] with extra compiler arguments
    /// appended verbatim, as [`Factory::compile_with_args`] does for a file.
    ///
    /// `name` is more than a label: a caller that read the source from a file
    /// passes that file's path, against whose directory the compiler resolves
    /// the source's relative imports, and which diagnostics cite.
    ///
    /// # Errors
    /// As [`Factory::compile`].
    pub fn compile_from_string_with_args(
        name: &str,
        source: &str,
        import_dirs: &[String],
        extra_args: &[String],
        double: bool,
        opt_level: i32,
    ) -> Result<Self, String> {
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let c_source = CString::new(source).map_err(|e| e.to_string())?;
        Self::create(import_dirs, extra_args, double, |argc, argv, err| unsafe {
            createCCraneliftDSPFactoryFromString(
                c_name.as_ptr(),
                c_source.as_ptr(),
                argc,
                argv,
                err,
                opt_level,
            )
        })
    }

    /// Whether instances from this factory were compiled for double-precision
    /// samples.
    #[must_use]
    pub const fn is_double(&self) -> bool {
        self.double
    }

    /// The key of the compiled program: a digest of its canonical FIR and of
    /// the options that shape the code.
    ///
    /// Two compilations of one source give the same key when the compiler is
    /// deterministic, and then share one cached factory within a process.
    /// `--check determinism` uses a separate process so its second render
    /// executes an independent JIT even when the keys agree.
    #[must_use]
    pub fn sha_key(&self) -> String {
        // SAFETY: `self.factory` is live for as long as `self`; the returned
        // string is owned by the caller and released with `freeCMemory`.
        unsafe {
            let raw = crate::factory::getCCraneliftDSPFactorySHAKey(self.factory);
            if raw.is_null() {
                return String::new();
            }
            let key = CStr::from_ptr(raw).to_string_lossy().into_owned();
            crate::factory::freeCMemory(raw.cast());
            key
        }
    }
}

impl Drop for Factory {
    fn drop(&mut self) {
        // SAFETY: `self.factory` was produced by this module and is freed
        // exactly once, after every `Probe` referencing it (via `Rc`) has
        // already freed its own instance.
        unsafe {
            let _ = deleteCCraneliftDSPFactory(self.factory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Factory, c_char, compile_error};

    /// The complete text survives a success and any failure that does not
    /// report through `error_msg`, so it may be stale: it is used only when
    /// it extends the summary of the failure at hand.
    #[test]
    fn a_stale_complete_text_is_not_attributed_to_another_failure() {
        let stale = Factory::compile_from_string("stale", "process = _ : *(0.5 ;", &[], true, 0)
            .err()
            .expect("a syntax error");
        assert!(stale.contains("FRS-PARSE-0001"));

        let mut error_msg = [0 as c_char; 4096];
        for (slot, byte) in error_msg.iter_mut().zip(b"some other failure") {
            *slot = *byte as c_char;
        }
        assert_eq!(compile_error(&error_msg), "some other failure");

        // A failure that wrote nothing to its buffer: every text extends the
        // empty one, and none of them is about this failure.
        assert_eq!(compile_error(&[0 as c_char; 4096]), "");
    }
}
