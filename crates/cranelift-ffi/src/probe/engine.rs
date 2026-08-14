//! Cranelift JIT lifecycle: factory, instance, control discovery, rendering.
//!
//! This is the only module in the crate that touches FFI. It owns the factory
//! and instance for the lifetime of a [`Probe`] and frees both on drop, so a
//! caller cannot leak a JIT module by returning early on an error.
//!
//! # Sample width
//! The JIT reads and writes I/O buffers at the width it was compiled for —
//! `f64` under `--double`, `f32` otherwise — while
//! `computeCCraneliftDSPInstance` merely forwards pointers. Choosing the wrong
//! buffer element type is therefore not a type error but silent memory
//! corruption, which is why [`Probe::render`] dispatches on
//! [`Probe::is_double`] rather than on any caller-supplied type.

use std::ffi::{CStr, CString, c_char, c_int};

use crate::factory::{createCCraneliftDSPFactoryFromFile, deleteCCraneliftDSPFactory};
use crate::instance::{
    buildUserInterfaceCCraneliftDSPInstance, computeCCraneliftDSPInstance,
    createCCraneliftDSPInstance, deleteCCraneliftDSPInstance, getNumInputsCCraneliftDSPInstance,
    getNumOutputsCCraneliftDSPInstance, initCCraneliftDSPInstance,
};
use crate::types::{CraneliftDspFactory, CraneliftDspInstance, FaustFloat};
use ffi_common::abi::FfiFaustFloat;

use crate::probe::params::ControlMap;
use crate::probe::render::{InputMode, RenderStats, StatsAccumulator};

/// How a render should be driven.
#[derive(Debug, Clone)]
pub struct RenderSpec {
    /// Total frames to render.
    pub frames: usize,
    /// Frames per `compute` call.
    pub block: usize,
    /// Excitation applied to the DSP inputs.
    pub input: InputMode,
    /// First frame included in statistics and dump.
    pub skip: usize,
}

impl Default for RenderSpec {
    fn default() -> Self {
        Self {
            frames: 15_000,
            block: 64,
            input: InputMode::Impulse,
            skip: 0,
        }
    }
}

/// One rendered frame, as `f64` regardless of the compiled sample width.
pub type Frame = Vec<f64>;

/// A compiled DSP with its controls resolved, ready to render.
pub struct Probe {
    factory: *mut CraneliftDspFactory,
    dsp: *mut CraneliftDspInstance,
    controls: ControlMap,
    inputs: usize,
    outputs: usize,
    double: bool,
    sample_rate: i32,
}

impl Probe {
    /// JIT-compile `path` and instantiate it at `sample_rate`.
    ///
    /// `import_dirs` become `-I` arguments; `double` selects the sample width
    /// and must match how the caller intends to read buffers.
    ///
    /// # Errors
    /// Returns the compiler's own diagnostic text when the front end or the
    /// JIT rejects the source, and a short message when instantiation fails.
    pub fn compile(
        path: &str,
        import_dirs: &[String],
        sample_rate: i32,
        double: bool,
        opt_level: i32,
    ) -> Result<Self, String> {
        let mut argv: Vec<CString> = Vec::new();
        for dir in import_dirs {
            argv.push(CString::new("-I").map_err(|e| e.to_string())?);
            argv.push(CString::new(dir.as_str()).map_err(|e| e.to_string())?);
        }
        if double {
            argv.push(CString::new("-double").map_err(|e| e.to_string())?);
        }
        let argv_ptrs: Vec<*const c_char> = argv.iter().map(|a| a.as_ptr()).collect();

        let c_path = CString::new(path).map_err(|e| e.to_string())?;
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromFile(
                c_path.as_ptr(),
                c_int::try_from(argv_ptrs.len()).map_err(|_| "too many -I arguments")?,
                if argv_ptrs.is_empty() {
                    std::ptr::null()
                } else {
                    argv_ptrs.as_ptr()
                },
                err.as_mut_ptr(),
                opt_level,
            )
        };
        if factory.is_null() {
            return Err(unsafe { CStr::from_ptr(err.as_ptr()) }
                .to_string_lossy()
                .into_owned());
        }

        let dsp = unsafe { createCCraneliftDSPInstance(factory) };
        if dsp.is_null() {
            unsafe {
                let _ = deleteCCraneliftDSPFactory(factory);
            }
            return Err("Cranelift instance creation failed".to_owned());
        }
        unsafe { initCCraneliftDSPInstance(dsp, sample_rate) };

        let inputs = usize::try_from(unsafe { getNumInputsCCraneliftDSPInstance(dsp) })
            .map_err(|_| "negative input arity".to_owned())?;
        let outputs = usize::try_from(unsafe { getNumOutputsCCraneliftDSPInstance(dsp) })
            .map_err(|_| "negative output arity".to_owned())?;

        // Discovery must happen after init: zones are only bound once the
        // instance owns its DSP struct.
        let mut controls = ControlMap::default();
        let mut glue = controls.glue();
        unsafe { buildUserInterfaceCCraneliftDSPInstance(dsp, &mut glue) };

        Ok(Self {
            factory,
            dsp,
            controls,
            inputs,
            outputs,
            double,
            sample_rate,
        })
    }

    /// Discovered controls.
    #[must_use]
    pub const fn controls(&self) -> &ControlMap {
        &self.controls
    }

    /// Number of audio inputs.
    #[must_use]
    pub const fn inputs(&self) -> usize {
        self.inputs
    }

    /// Number of audio outputs.
    #[must_use]
    pub const fn outputs(&self) -> usize {
        self.outputs
    }

    /// Whether the DSP was compiled for double-precision samples.
    #[must_use]
    pub const fn is_double(&self) -> bool {
        self.double
    }

    /// Write `value` into a control zone, respecting the compiled width.
    ///
    /// Crate-internal: a public function taking a raw pointer would be
    /// unsound, since nothing stops a caller passing a pointer this instance
    /// never produced. Outside callers go through [`Probe::set`], which
    /// resolves the zone from the discovered control map.
    ///
    /// A null zone is ignored; controls always carry a non-null zone by
    /// construction ([`crate::probe::params`] rejects null at discovery).
    pub(crate) fn set_zone(&self, zone: *mut FfiFaustFloat, value: f64) {
        if zone.is_null() {
            return;
        }
        // SAFETY: the zone came from this instance's `buildUserInterface` and
        // stays valid until the instance is dropped. The width matches how the
        // factory was compiled.
        unsafe {
            if self.double {
                *zone.cast::<f64>() = value;
            } else {
                *zone = value as FfiFaustFloat;
            }
        }
    }

    /// Apply a value to a control by path, clamped to its declared range.
    ///
    /// # Errors
    /// Returns a message naming the candidates when the query is ambiguous, or
    /// stating the query when nothing matches.
    pub fn set(&self, query: &str, value: f64) -> Result<(), String> {
        use crate::probe::params::Resolution;
        match self.controls.resolve(query) {
            Resolution::Unique(control) => {
                self.set_zone(control.zone, control.clamp(value));
                Ok(())
            }
            Resolution::NotFound => Err(format!("no control matching `{query}`")),
            Resolution::Ambiguous(candidates) => Err(format!(
                "`{query}` is ambiguous, matches: {}",
                candidates.join(", ")
            )),
        }
    }

    /// Render `spec`, invoking `on_frame` for each frame at or after the skip
    /// point, and return the statistics over that same window.
    ///
    /// The callback receives absolute frame indices so a caller can decimate
    /// or annotate without tracking its own counter.
    pub fn render<F>(&self, spec: &RenderSpec, mut on_frame: F) -> RenderStats
    where
        F: FnMut(usize, &[f64]),
    {
        let mut acc = StatsAccumulator::new(self.outputs, spec.skip);
        let block = spec.block.max(1);
        let sample_rate = f64::from(self.sample_rate);

        // The two widths differ only in buffer element type; the loop is
        // identical, hence the macro rather than a generic (the FFI takes a
        // fixed pointer type).
        macro_rules! run {
            ($elem:ty) => {{
                let mut ins = vec![vec![<$elem>::default(); block]; self.inputs];
                let mut outs = vec![vec![<$elem>::default(); block]; self.outputs];
                let mut written = 0usize;
                while written < spec.frames {
                    let n = block.min(spec.frames - written);
                    for (ch, channel) in ins.iter_mut().enumerate() {
                        for (j, sample) in channel.iter_mut().enumerate().take(n) {
                            *sample = spec.input.sample(ch, written + j, sample_rate) as $elem;
                        }
                    }
                    let mut in_ptrs: Vec<*mut FaustFloat> = ins
                        .iter_mut()
                        .map(|c| c.as_mut_ptr().cast::<FaustFloat>())
                        .collect();
                    let mut out_ptrs: Vec<*mut FaustFloat> = outs
                        .iter_mut()
                        .map(|c| c.as_mut_ptr().cast::<FaustFloat>())
                        .collect();
                    // SAFETY: both pointer arrays have the arity the instance
                    // reported, and each buffer holds at least `n` elements of
                    // the compiled width.
                    unsafe {
                        computeCCraneliftDSPInstance(
                            self.dsp,
                            n as i32,
                            in_ptrs.as_mut_ptr(),
                            out_ptrs.as_mut_ptr(),
                        );
                    }
                    let mut frame: Frame = vec![0.0; self.outputs];
                    for j in 0..n {
                        for (ch, channel) in outs.iter().enumerate() {
                            frame[ch] = channel[j] as f64;
                        }
                        acc.push(written + j, &frame);
                        if written + j >= spec.skip {
                            on_frame(written + j, &frame);
                        }
                    }
                    written += n;
                }
            }};
        }

        if self.double {
            run!(f64);
        } else {
            run!(f32);
        }
        acc.finish()
    }

    /// Sample rate the instance was initialised with.
    ///
    /// The FFI exposes no getter, so this mirrors what `compile` passed. It is
    /// only used to generate time-dependent excitation.
    #[must_use]
    pub const fn sample_rate(&self) -> i32 {
        self.sample_rate
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        // SAFETY: both handles were produced by this module and are freed
        // exactly once, instance before factory as the C API requires.
        unsafe {
            deleteCCraneliftDSPInstance(self.dsp);
            let _ = deleteCCraneliftDSPFactory(self.factory);
        }
    }
}
