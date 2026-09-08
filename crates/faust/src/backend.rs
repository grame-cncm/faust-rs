//! The two C entry-point sets behind one dispatch: a raw factory and a raw
//! instance are enums over the interpreter's and the Cranelift backend's
//! pointers, and every operation matches on the variant. Nothing here is
//! public; the safe types of the crate own these pointers.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Mutex, MutexGuard};

use ffi_common::abi::{MetaGlue, UIGlue};

use crate::{Backend, Error, ErrorKind};

/// Size of the C error buffer, the `MIX_BUFFER_SIZE` of the C++ API.
const ERROR_BUFFER: usize = 4096;

/// One factory creation or instantiation at a time, process-wide.
///
/// The backends share compiled programs by SHA key in a process-global cache,
/// so two `Factory` values built from the same source are two references to
/// one entry, and instantiating them from two threads at once races on that
/// entry (the interpreter optimises its blocks on first instantiation, the
/// Cranelift backend counts live instances). Neither is a hot path; a lock
/// here is what makes the safe API safe whatever the host's threads do.
static LIFECYCLE: Mutex<()> = Mutex::new(());

fn lifecycle() -> MutexGuard<'static, ()> {
    LIFECYCLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A compiled program, owned by the backend's factory cache; the pointer is
/// one reference of that cache, released by [`RawFactory::delete`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum RawFactory {
    Interp(*mut interp_ffi::types::InterpreterDspFactory),
    Cranelift(*mut cranelift_ffi::types::CraneliftDspFactory),
}

/// An instance, owned by the same cache under its factory's entry.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RawInstance {
    Interp(*mut interp_ffi::types::InterpreterDspInstance),
    Cranelift(*mut cranelift_ffi::types::CraneliftDspInstance),
}

/// Where a program comes from.
pub(crate) enum Source<'a> {
    File(&'a str),
    Text { name: &'a str, source: &'a str },
}

impl RawFactory {
    /// Compiles `source` for `backend` with the given argument vector.
    pub(crate) fn create(
        backend: Backend,
        source: &Source<'_>,
        argv: &[String],
        opt_level: i32,
    ) -> Result<Self, Error> {
        let c_args: Vec<CString> = argv
            .iter()
            .map(|a| CString::new(a.as_str()))
            .collect::<Result<_, _>>()
            .map_err(|_| {
                Error::new(
                    ErrorKind::Compile,
                    "a compiler argument contains a NUL byte",
                )
            })?;
        let c_argv: Vec<*const c_char> = c_args.iter().map(|a| a.as_ptr()).collect();
        let argc = c_int::try_from(c_argv.len())
            .map_err(|_| Error::new(ErrorKind::Compile, "too many compiler arguments"))?;
        let mut error = vec![0 as c_char; ERROR_BUFFER];
        let (c_name, c_source) = match source {
            Source::File(path) => (c_string(path)?, None),
            Source::Text { name, source } => (c_string(name)?, Some(c_string(source)?)),
        };
        let _serialised = lifecycle();
        // SAFETY: every pointer is valid for the duration of the call, the
        // error buffer has the size the C API expects, and the argument
        // vector holds `argc` NUL-terminated strings.
        let raw = unsafe {
            match (backend, &c_source) {
                (Backend::Interp, None) => {
                    RawFactory::Interp(interp_ffi::factory::createCInterpreterDSPFactoryFromFile(
                        c_name.as_ptr(),
                        argc,
                        c_argv.as_ptr(),
                        error.as_mut_ptr(),
                    ))
                }
                (Backend::Interp, Some(text)) => {
                    RawFactory::Interp(interp_ffi::factory::createCInterpreterDSPFactoryFromString(
                        c_name.as_ptr(),
                        text.as_ptr(),
                        argc,
                        c_argv.as_ptr(),
                        error.as_mut_ptr(),
                    ))
                }
                (Backend::Cranelift, None) => RawFactory::Cranelift(
                    cranelift_ffi::factory::createCCraneliftDSPFactoryFromFile(
                        c_name.as_ptr(),
                        argc,
                        c_argv.as_ptr(),
                        error.as_mut_ptr(),
                        opt_level,
                    ),
                ),
                (Backend::Cranelift, Some(text)) => RawFactory::Cranelift(
                    cranelift_ffi::factory::createCCraneliftDSPFactoryFromString(
                        c_name.as_ptr(),
                        text.as_ptr(),
                        argc,
                        c_argv.as_ptr(),
                        error.as_mut_ptr(),
                        opt_level,
                    ),
                ),
            }
        };
        if raw.is_null() {
            // SAFETY: the C API wrote a NUL-terminated message into the buffer.
            let message = unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_string_lossy()
                .trim_end()
                .to_owned();
            let message = if message.is_empty() {
                format!("the {backend} backend rejected the program without a message")
            } else {
                message
            };
            return Err(Error::new(ErrorKind::Compile, message));
        }
        Ok(raw)
    }

    fn is_null(self) -> bool {
        match self {
            RawFactory::Interp(p) => p.is_null(),
            RawFactory::Cranelift(p) => p.is_null(),
        }
    }

    pub(crate) fn backend(self) -> Backend {
        match self {
            RawFactory::Interp(_) => Backend::Interp,
            RawFactory::Cranelift(_) => Backend::Cranelift,
        }
    }

    /// The factory's JSON description (UI tree and metadata).
    pub(crate) fn json(self) -> String {
        // SAFETY: the factory pointer is a live cache reference; the returned
        // string is heap-allocated by the C API and freed with its allocator.
        unsafe {
            let ptr = match self {
                RawFactory::Interp(f) => interp_ffi::factory::getCInterpreterDSPFactoryJSON(f),
                RawFactory::Cranelift(f) => cranelift_ffi::factory::getCCraneliftDSPFactoryJSON(f),
            };
            take_c_string(self, ptr)
        }
    }

    /// Releases this reference; the cache frees the program with the last one.
    pub(crate) fn delete(self) {
        let _serialised = lifecycle();
        // SAFETY: the pointer is a live cache reference, released exactly once.
        unsafe {
            match self {
                RawFactory::Interp(f) => {
                    interp_ffi::factory::deleteCInterpreterDSPFactory(f);
                }
                RawFactory::Cranelift(f) => {
                    cranelift_ffi::factory::deleteCCraneliftDSPFactory(f);
                }
            }
        }
    }

    /// Creates an instance, null when the backend refuses (a `-mem0` factory
    /// without its memory manager, for instance).
    pub(crate) fn instantiate(self) -> Option<RawInstance> {
        let _serialised = lifecycle();
        // SAFETY: the factory pointer is a live cache reference.
        let raw = unsafe {
            match self {
                RawFactory::Interp(f) => {
                    RawInstance::Interp(interp_ffi::instance::createCInterpreterDSPInstance(f))
                }
                RawFactory::Cranelift(f) => {
                    RawInstance::Cranelift(cranelift_ffi::instance::createCCraneliftDSPInstance(f))
                }
            }
        };
        (!raw.is_null()).then_some(raw)
    }
}

/// Copies a heap string returned by the C API and frees it.
unsafe fn take_c_string(factory: RawFactory, ptr: *mut c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: the caller got `ptr` from the backend of `factory`, which
    // allocated it with the allocator its `freeCMemory` frees.
    unsafe {
        let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        match factory {
            RawFactory::Interp(_) => interp_ffi::factory::freeCMemory(ptr.cast::<c_void>()),
            RawFactory::Cranelift(_) => cranelift_ffi::factory::freeCMemory(ptr.cast::<c_void>()),
        }
        text
    }
}

fn c_string(text: &str) -> Result<CString, Error> {
    CString::new(text)
        .map_err(|_| Error::new(ErrorKind::Compile, "the program text contains a NUL byte"))
}

macro_rules! on_instance {
    ($raw:expr, $dsp:ident => $interp:expr, $cranelift:expr) => {
        match $raw {
            RawInstance::Interp($dsp) => $interp,
            RawInstance::Cranelift($dsp) => $cranelift,
        }
    };
}

impl RawInstance {
    fn is_null(self) -> bool {
        on_instance!(self, p => p.is_null(), p.is_null())
    }

    pub(crate) fn delete(self) {
        let _serialised = lifecycle();
        // SAFETY: the instance is live and deleted exactly once, by `Dsp`'s Drop.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::deleteCInterpreterDSPInstance(p), cranelift_ffi::instance::deleteCCraneliftDSPInstance(p));
        }
    }

    pub(crate) fn num_inputs(self) -> c_int {
        // SAFETY: live instance.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::getNumInputsCInterpreterDSPInstance(p), cranelift_ffi::instance::getNumInputsCCraneliftDSPInstance(p))
        }
    }

    pub(crate) fn num_outputs(self) -> c_int {
        // SAFETY: live instance.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::getNumOutputsCInterpreterDSPInstance(p), cranelift_ffi::instance::getNumOutputsCCraneliftDSPInstance(p))
        }
    }

    pub(crate) fn sample_rate(self) -> c_int {
        // SAFETY: live instance.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::getSampleRateCInterpreterDSPInstance(p), cranelift_ffi::instance::getSampleRateCCraneliftDSPInstance(p))
        }
    }

    pub(crate) fn init(self, sample_rate: c_int) {
        // SAFETY: live instance.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::initCInterpreterDSPInstance(p, sample_rate), cranelift_ffi::instance::initCCraneliftDSPInstance(p, sample_rate));
        }
    }

    pub(crate) fn instance_init(self, sample_rate: c_int) {
        // SAFETY: live instance.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::instanceInitCInterpreterDSPInstance(p, sample_rate), cranelift_ffi::instance::instanceInitCCraneliftDSPInstance(p, sample_rate));
        }
    }

    pub(crate) fn instance_reset_user_interface(self) {
        // SAFETY: live instance.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::instanceResetUserInterfaceCInterpreterDSPInstance(p), cranelift_ffi::instance::instanceResetUserInterfaceCCraneliftDSPInstance(p));
        }
    }

    pub(crate) fn instance_clear(self) {
        // SAFETY: live instance.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::instanceClearCInterpreterDSPInstance(p), cranelift_ffi::instance::instanceClearCCraneliftDSPInstance(p));
        }
    }

    /// Runs the UI builder with `glue`, whose callbacks must stay valid for
    /// the duration of the call.
    pub(crate) unsafe fn build_user_interface(self, glue: *mut UIGlue) {
        // SAFETY: live instance; the caller guarantees the glue.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::buildUserInterfaceCInterpreterDSPInstance(p, glue), cranelift_ffi::instance::buildUserInterfaceCCraneliftDSPInstance(p, glue));
        }
    }

    pub(crate) unsafe fn metadata(self, glue: *mut MetaGlue) {
        // SAFETY: live instance; the caller guarantees the glue.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::metadataCInterpreterDSPInstance(p, glue), cranelift_ffi::instance::metadataCCraneliftDSPInstance(p, glue));
        }
    }

    /// Runs `count` frames. The pointers address channels of the width the
    /// backend exchanges (see the crate documentation), cast to the C ABI's
    /// `float*`; the caller guarantees `count` frames in each.
    pub(crate) unsafe fn compute(
        self,
        count: c_int,
        inputs: *mut *mut f32,
        outputs: *mut *mut f32,
    ) {
        // SAFETY: live instance; the caller guarantees the buffers.
        unsafe {
            on_instance!(self, p => interp_ffi::instance::computeCInterpreterDSPInstance(p, count, inputs, outputs), cranelift_ffi::instance::computeCCraneliftDSPInstance(p, count, inputs, outputs));
        }
    }
}
