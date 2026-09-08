//! The Rust API of faust-rs: compile a Faust program to a [`Factory`],
//! instantiate it as [`Dsp`] values, set their controls and run them.
//!
//! This crate is the supported way to embed faust-rs from Rust. The other
//! crates of the workspace (`compiler`, `codegen`, `interp-ffi`,
//! `cranelift-ffi`, ...) are implementation details with no stability
//! promise; the C API of `libfaust-rs` and this crate are the two contracts.
//!
//! # Model
//!
//! One model for the two backends, the bytecode interpreter and the Cranelift
//! JIT, chosen by [`CompileOptions::backend`]:
//!
//! - a [`Factory`] is a compiled program: its arities, its JSON description,
//!   and the code (bytecode or machine code) every instance runs. Factories
//!   are cheap to clone; a clone is another handle on the same compiled
//!   program;
//! - a [`Dsp`] is an instance: its state, its sample rate, its controls.
//!   [`Factory::instantiate`] creates and initialises one. A `Dsp` owns a
//!   reference to its factory, so the factory's code lives as long as any of
//!   its instances, whatever the host does with its own `Factory` handles.
//!   No lifetime parameter, no `unsafe` for the host: a `Dsp` is a plain
//!   value that can be stored, moved and sent to another thread;
//! - controls are addressed by the paths the C++ `MapUI` and OSC use,
//!   `/group/label`; [`Dsp::set`] and [`Dsp::get`] read and write them, and
//!   [`Dsp::controls`] lists them with their kind and range.
//!
//! # Precision
//!
//! [`CompileOptions::precision`] chooses the type the program computes with.
//! [`Dsp::compute_f32`] and [`Dsp::compute_f64`] accept host buffers of
//! either width and convert when it differs from what the backend exchanges:
//! the Cranelift JIT exchanges buffers of the compiled precision; the
//! interpreter's C ABI exchanges `f32` buffers whatever the precision, so an
//! `f64` interpreter program computes in `f64` between `f32` boundaries.
//!
//! # Known gap
//!
//! [`Dsp::metadata`] returns what the backend's `metadata` entry point
//! declares. The C++ and other text backends receive the program's
//! `declare` lines from the compiler, but the FIR the interpreter and the
//! Cranelift backend consume carries an empty `metadata` function, so for
//! them only the backend's own entries appear (the C API has the same gap).
//! The name and the control metadata (`[unit:dB]`...) are not affected.
//!
//! # Lifecycle behind the scenes
//!
//! Both backends are driven through their C entry points, so a `Factory` has
//! exactly the semantics of the C API: compiled programs are shared by SHA
//! key and reference counted, and a `Dsp` holds one reference to its
//! factory's entry. All the `unsafe` lives in this crate.
//!
//! ```no_run
//! use faust::{Backend, CompileOptions, Factory};
//!
//! let options = CompileOptions { backend: Backend::Cranelift, ..Default::default() };
//! let factory = Factory::from_source("gain", r#"process = _ * hslider("gain", 0.5, 0, 1, 0.01);"#, &options)?;
//! let mut dsp = factory.instantiate(48_000)?;
//! dsp.set("/gain/gain", 0.25)?;
//! let input = [1.0_f32; 64];
//! let mut output = [0.0_f32; 64];
//! dsp.compute_f32(&[&input], &mut [&mut output])?;
//! assert_eq!(output[0], 0.25);
//! # Ok::<(), faust::Error>(())
//! ```

mod backend;
mod controls;
mod dsp;
mod factory;

pub use controls::{Control, ControlKind};
pub use dsp::Dsp;
pub use factory::Factory;

use std::fmt;
use std::path::PathBuf;

/// The two engines a program can be compiled for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Backend {
    /// The bytecode interpreter: portable, no code generation at run time.
    #[default]
    Interp,
    /// The Cranelift JIT: native code, compiled when the factory is created.
    Cranelift,
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Backend::Interp => "interp",
            Backend::Cranelift => "cranelift",
        })
    }
}

/// The floating-point type a program computes with (`-double` or not).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Precision {
    #[default]
    F32,
    F64,
}

/// How a program is compiled.
#[derive(Clone, Debug, Default)]
pub struct CompileOptions {
    pub backend: Backend,
    pub precision: Precision,
    /// Directories searched by `import(...)`, in order (`-I`).
    pub import_dirs: Vec<PathBuf>,
    /// The Cranelift optimisation level, 0 to 3; ignored by the interpreter.
    pub opt_level: i32,
    /// Further compiler arguments, verbatim, after the ones the fields above
    /// produce (for instance `-vec`, `-vs`, `-ss`, `-bra-tape`).
    pub args: Vec<String>,
}

impl CompileOptions {
    /// The options for one backend, everything else at its default.
    pub fn for_backend(backend: Backend) -> Self {
        Self {
            backend,
            ..Self::default()
        }
    }

    /// The argument vector handed to the backend's C entry point.
    pub(crate) fn argv(&self) -> Vec<String> {
        let mut argv = Vec::new();
        for dir in &self.import_dirs {
            argv.push("-I".to_owned());
            argv.push(dir.to_string_lossy().into_owned());
        }
        if self.precision == Precision::F64 {
            argv.push("-double".to_owned());
        }
        argv.extend(self.args.iter().cloned());
        argv
    }
}

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The program did not compile; the message is the compiler's.
    Compile,
    /// The factory could not be instantiated.
    Instantiate,
    /// No control has this path.
    UnknownControl,
    /// The control is a bargraph, written by the DSP only.
    ReadOnlyControl,
    /// The buffers passed to `compute` do not match the DSP's arities or
    /// hold fewer frames than requested.
    Buffers,
}

/// An error of this API, with the kind and a message for humans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for Error {}

/// This crate's version, the workspace's.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
