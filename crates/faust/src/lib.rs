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
//!   [`Factory::create_dsp_instance`] creates and initialises one. A `Dsp` owns a
//!   reference to its factory, so the factory's code lives as long as any of
//!   its instances, whatever the host does with its own `Factory` handles.
//!   No lifetime parameter, no `unsafe` for the host: a `Dsp` is a plain
//!   value that can be stored, moved, sent to another thread and shared
//!   (`Send + Sync`: its `&self` methods only read);
//! - controls are addressed as the C++ `MapUI` addresses them: by path
//!   (`/group/label`, the OSC address), by shortname or by label;
//!   [`Dsp::set_param_value`] and [`Dsp::get_param_value`] write and read
//!   them, and [`Dsp::controls`] lists them with their names, kind and range.
//!
//! # Precision
//!
//! [`CompileOptions::precision`] chooses the type the program computes with.
//! [`Dsp::compute`] accepts host buffers of either width, `f32` or `f64`
//! (see [`Sample`]), and converts when it differs from the compiled
//! precision, which is what both backends exchange: a `-double` program run
//! over `f64` buffers sees its samples unrounded on either backend. (The
//! interpreter's C ABI exchanges `f32` whatever the precision; this crate
//! reaches its `f64` path through a Rust entry point of `interp-ffi`.)
//!
//! # Names
//!
//! The methods that have a counterpart in the C++ `dsp` and `dsp_factory`
//! classes (`architecture/faust/dsp/dsp.h`), or in the `MapUI` class a C++
//! host drives the controls with (`architecture/faust/gui/MapUI.h`), carry
//! its name in snake case, as the `FaustDsp` trait of the Rust architectures
//! does:
//!
//! | C++ | this crate |
//! | --- | --- |
//! | `getNumInputs`, `getNumOutputs`, `getSampleRate` | [`Dsp::get_num_inputs`], [`Dsp::get_num_outputs`], [`Dsp::get_sample_rate`] |
//! | `init`, `instanceInit`, `instanceConstants` | [`Dsp::init`], [`Dsp::instance_init`], [`Dsp::instance_constants`] |
//! | `instanceResetUserInterface`, `instanceClear` | [`Dsp::instance_reset_user_interface`], [`Dsp::instance_clear`] |
//! | `metadata(Meta*)` | [`Dsp::metadata`], which returns the pairs |
//! | `compute(count, inputs, outputs)` | [`Dsp::compute`], generic over [`Sample`] as `FAUSTFLOAT` |
//! | `dsp_factory::getName`, `getJSON` | [`Factory::get_name`], [`Factory::get_json`] |
//! | `dsp_factory::createDSPInstance` | [`Factory::create_dsp_instance`], which also initialises |
//! | `MapUI::setParamValue`, `getParamValue` | [`Dsp::set_param_value`], [`Dsp::get_param_value`]: by path, shortname or label, in that order |
//! | `MapUI` paths, shortnames and labels | [`Control::path`], [`Control::shortname`], [`Control::label`] |
//!
//! `buildUserInterface` has no counterpart: [`Dsp::controls`] and the
//! `MapUI`-like methods above replace the `UI` a host would implement. Nor has
//! `clone`, whose C++ semantics (a fresh instance of the same factory) a Rust
//! `clone` would misname: `dsp.factory().create_dsp_instance(rate)` is it.
//! What has no counterpart in C++ (the backend, the precision, the list of
//! the controls) is named the Rust way.
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
//! let mut dsp = factory.create_dsp_instance(48_000)?;
//! dsp.set_param_value("gain", 0.25)?; // a path, a shortname or a label
//! let input = [1.0_f32; 64];
//! let mut output = [0.0_f32; 64];
//! dsp.compute(64, &[&input], &mut [&mut output])?;
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

/// A sample type [`Dsp::compute`] exchanges with the host: `f32` or `f64`,
/// the two widths `FAUSTFLOAT` takes in C++. Sealed: implemented for those
/// two only.
pub trait Sample: backend::Width {}

impl Sample for f32 {}

impl Sample for f64 {}

/// The floating-point type a program computes with (`-double` or not).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Precision {
    /// Single precision, `f32`: the default, as with `faust-rs`.
    #[default]
    F32,
    /// Double precision, `f64`: the program is compiled with `-double`.
    F64,
}

/// How a program is compiled.
#[derive(Clone, Debug, Default)]
pub struct CompileOptions {
    /// The engine the program is compiled for.
    pub backend: Backend,
    /// The type the program computes with.
    pub precision: Precision,
    /// Directories searched by `import(...)`, `library(...)` and
    /// `component(...)`, the first of the list first (`-I`). A name is
    /// looked up relative to the working directory, then in these
    /// directories, then in the installed Faust libraries, then, for
    /// [`Factory::from_file`], in the file's own directory: the order of the
    /// C++ compiler.
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
        // The C entry points read `-I` as the C++ compiler does, the last one
        // first: emitted backwards, the first directory of the list wins.
        for dir in self.import_dirs.iter().rev() {
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

/// What went wrong. `#[non_exhaustive]`: a match needs a `_` arm, so new
/// kinds can be added without breaking hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    /// What went wrong, for a host to act on.
    pub kind: ErrorKind,
    /// What went wrong, for a human: for [`ErrorKind::Compile`], the
    /// compiler's own message; for a control, its path.
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
