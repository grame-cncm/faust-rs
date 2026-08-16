//! Cranelift JIT lifecycle: factory, instance, control discovery, rendering.
//!
//! This is the only module in the crate that touches FFI. It owns the
//! factory ([`Factory`]) and every instance created from it ([`Probe`]) for
//! their lifetimes and frees them on drop, so a caller cannot leak a JIT
//! module by returning early on an error.
//!
//! # Sample width
//! The JIT reads and writes I/O buffers at the width it was compiled for —
//! `f64` under `--double`, `f32` otherwise — while
//! `computeCCraneliftDSPInstance` merely forwards pointers. Choosing the wrong
//! buffer element type is therefore not a type error but silent memory
//! corruption, which is why [`Probe::render`] dispatches on
//! [`Probe::is_double`] rather than on any caller-supplied type.
//!
//! # One factory, many instances
//! [`Factory`] and [`Probe`] are split apart, rather than [`Probe`] owning
//! its factory outright, because the polyphonic wrapper ([`PolyProbe`]) needs
//! N independent instances from a single JIT compile — the whole reason this
//! tool is built on Cranelift rather than the interpreter (module doc,
//! `crate::probe`, design §2). [`Probe`] holds an `Rc<Factory>` so the
//! factory outlives every instance created from it and is freed exactly once,
//! when the last one drops.

use std::ffi::{CStr, CString, c_char, c_int};
use std::rc::Rc;

use crate::factory::{createCCraneliftDSPFactoryFromFile, deleteCCraneliftDSPFactory};
use crate::instance::{
    buildUserInterfaceCCraneliftDSPInstance, computeCCraneliftDSPInstance,
    createCCraneliftDSPInstance, deleteCCraneliftDSPInstance, getNumInputsCCraneliftDSPInstance,
    getNumOutputsCCraneliftDSPInstance, initCCraneliftDSPInstance,
    instanceClearCCraneliftDSPInstance, instanceResetUserInterfaceCCraneliftDSPInstance,
};
use crate::types::{CraneliftDspFactory, CraneliftDspInstance, FaustFloat};
use ffi_common::abi::FfiFaustFloat;

use crate::probe::params::{ControlKind, ControlMap, Resolution};
use crate::probe::poly;
use crate::probe::render::{InputMode, RenderStats, StatsAccumulator};
use crate::probe::schedule::{Event, Schedule};

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
    /// Events to apply at exact frames during the render.
    ///
    /// Only [`Event::SetParam`] is meaningful on a scalar `Probe`; note
    /// events need [`PolyProbe`]. The render loop shortens its block so a
    /// boundary always lands on the next scheduled frame, which is what makes
    /// the timing sample-exact rather than rounded to the block grid.
    pub schedule: Schedule,
    /// Hold every `button` at 1.0 for the first block, then release it.
    ///
    /// This is `FUI::setButtons` as the reference impulse protocol drives it:
    /// buttons only, not checkboxes or sliders, and for exactly one block.
    /// Without it an instrument renders silence, because nothing ever gates a
    /// voice.
    pub drive_buttons: bool,
}

impl Default for RenderSpec {
    fn default() -> Self {
        Self {
            frames: 15_000,
            block: 64,
            input: InputMode::Impulse,
            skip: 0,
            schedule: Schedule::new(),
            drive_buttons: false,
        }
    }
}

/// One rendered frame, as `f64` regardless of the compiled sample width.
pub type Frame = Vec<f64>;

/// A JIT-compiled DSP factory, shared by every [`Probe`] instantiated from
/// it.
///
/// Split out from [`Probe`] so a caller — chiefly [`PolyProbe`] — can create
/// several independent instances from one compile. `double` is recorded here
/// rather than per-instance because it is a compile-time argument (`-double`
/// on the front end's own `argv`, `Probe::compile`'s doc), fixed for every
/// instance the factory produces.
pub struct Factory {
    factory: *mut CraneliftDspFactory,
    double: bool,
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
        let mut argv: Vec<CString> = Vec::new();
        for dir in import_dirs {
            argv.push(CString::new("-I").map_err(|e| e.to_string())?);
            argv.push(CString::new(dir.as_str()).map_err(|e| e.to_string())?);
        }
        if double {
            argv.push(CString::new("-double").map_err(|e| e.to_string())?);
        }
        let argv_ptrs: Vec<*const c_char> = argv.iter().map(|a| a.as_ptr()).collect();

        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let c_source = CString::new(source).map_err(|e| e.to_string())?;
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            crate::factory::createCCraneliftDSPFactoryFromString(
                c_name.as_ptr(),
                c_source.as_ptr(),
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
        Ok(Self { factory, double })
    }

    /// Whether instances from this factory were compiled for double-precision
    /// samples.
    #[must_use]
    pub const fn is_double(&self) -> bool {
        self.double
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

/// A compiled DSP with its controls resolved, ready to render.
pub struct Probe {
    factory: Rc<Factory>,
    dsp: *mut CraneliftDspInstance,
    controls: ControlMap,
    inputs: usize,
    outputs: usize,
    sample_rate: i32,
}

impl Probe {
    /// JIT-compile `path` and instantiate it at `sample_rate`.
    ///
    /// `import_dirs` become `-I` arguments; `double` selects the sample width
    /// and must match how the caller intends to read buffers. Equivalent to
    /// [`Factory::compile`] followed by [`Probe::instantiate`], for the
    /// common case of one factory feeding exactly one instance.
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
        let factory = Rc::new(Factory::compile(path, import_dirs, double, opt_level)?);
        Self::instantiate(&factory, sample_rate)
    }

    /// Create one more instance from an already-compiled `factory`.
    ///
    /// This is what lets a polyphonic bus pay the JIT cost once for N voices:
    /// each call creates an independent instance — its own zones, its own
    /// state — sharing only the compiled code.
    ///
    /// # Errors
    /// Returns a short message when instantiation fails.
    pub fn instantiate(factory: &Rc<Factory>, sample_rate: i32) -> Result<Self, String> {
        let dsp = unsafe { createCCraneliftDSPInstance(factory.factory) };
        if dsp.is_null() {
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
            factory: Rc::clone(factory),
            dsp,
            controls,
            inputs,
            outputs,
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
    pub fn is_double(&self) -> bool {
        self.factory.double
    }

    /// Write `value` into a control zone, respecting the compiled width.
    ///
    /// Crate-internal: a public function taking a raw pointer would be
    /// unsound, since nothing stops a caller passing a pointer this instance
    /// never produced. Outside callers go through [`Probe::set`] or
    /// [`Probe::set_exact`], which resolve the zone from the discovered
    /// control map.
    ///
    /// A null zone is ignored; controls always carry a non-null zone by
    /// construction ([`crate::probe::params`] rejects null at discovery).
    pub(crate) fn set_zone(&self, zone: *mut FfiFaustFloat, value: f64) {
        if zone.is_null() {
            return;
        }
        // SAFETY: the zone came from this instance's `buildUserInterface` and
        // stays valid until the instance is dropped. The width matches how
        // the factory was compiled.
        unsafe {
            if self.factory.double {
                *zone.cast::<f64>() = value;
            } else {
                *zone = value as FfiFaustFloat;
            }
        }
    }

    /// Return the instance to the state it had just after `init`.
    ///
    /// Controls go back to their declared defaults and every piece of internal
    /// state — delay lines, filter integrators, phase accumulators — is
    /// zeroed. A sweep must do this between points: without it a resonant
    /// filter carries its ringing into the next configuration, and every
    /// measurement after the first silently describes the previous one as much
    /// as its own.
    pub fn reset(&self) {
        // SAFETY: `self.dsp` is a live instance owned by this `Probe`.
        unsafe {
            instanceResetUserInterfaceCCraneliftDSPInstance(self.dsp);
            instanceClearCCraneliftDSPInstance(self.dsp);
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

    /// Write to a control by its exact discovered path, unclamped.
    ///
    /// [`Probe::set`] resolves a fragment and clamps to the widget's declared
    /// range, matching how a command-line user sets a value. The polyphonic
    /// wrapper needs neither: C++ `MapUI::setParamValue` — what
    /// `dsp_voice::keyOn`/`keyOff` call — writes the zone directly, with the
    /// exact path already known and no clamp. A synthesized frequency (say,
    /// `midiToFreq(127)` ≈ 12.5 kHz) must reach the zone exactly as computed,
    /// not silently reshaped by a slider's declared range.
    ///
    /// # Errors
    /// Returns a message when no control has exactly this path.
    pub fn set_exact(&self, path: &str, value: f64) -> Result<(), String> {
        match self.controls.get(path) {
            Some(control) => {
                self.set_zone(control.zone, value);
                Ok(())
            }
            None => Err(format!("no control at exact path `{path}`")),
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
        let double = self.factory.double;

        // The two widths differ only in buffer element type; the loop is
        // identical, hence the macro rather than a generic (the FFI takes a
        // fixed pointer type).
        macro_rules! run {
            ($elem:ty) => {{
                let mut ins = vec![vec![<$elem>::default(); block]; self.inputs];
                let mut outs = vec![vec![<$elem>::default(); block]; self.outputs];
                let buttons: Vec<*mut FfiFaustFloat> = if spec.drive_buttons {
                    self.controls
                        .iter()
                        .filter(|c| c.kind == ControlKind::Button)
                        .map(|c| c.zone)
                        .collect()
                } else {
                    Vec::new()
                };
                let mut written = 0usize;
                let mut cycle = 0usize;
                while written < spec.frames {
                    // Apply anything due exactly here, then shorten the block
                    // so the next event also lands on a boundary.
                    for event in spec.schedule.at(written) {
                        if let Event::SetParam { path, value } = event {
                            let _ = self.set(path, *value);
                        }
                    }
                    let mut n = block.min(spec.frames - written);
                    if let Some(next) = spec.schedule.next_after(written) {
                        if next > written {
                            n = n.min(next - written);
                        }
                    }
                    if spec.drive_buttons {
                        let value = f64::from(u8::from(cycle == 0));
                        for &zone in &buttons {
                            self.set_zone(zone, value);
                        }
                    }
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
                    cycle += 1;
                }
            }};
        }

        if double {
            run!(f64);
        } else {
            run!(f32);
        }
        acc.finish()
    }

    /// Run exactly one `compute` call over `frames` samples of caller-supplied
    /// input, returning `frames` samples per output channel as `f64`.
    ///
    /// The primitive [`PolyProbe`] is built on. [`Probe::render`] owns a
    /// whole-render loop with button-driving and statistics baked in, which
    /// the polyphonic wrapper cannot reuse: its own block cadence is dictated
    /// by voice legato splits (`computeLegato`, `poly-dsp.h:213`, issues two
    /// `compute` calls per host block around a mid-block note change), not by
    /// a fixed excitation over the whole render.
    ///
    /// `inputs[ch]` must hold at least `frames` samples for every input
    /// channel (`inputs.len()` must equal [`Probe::inputs`]).
    #[must_use]
    pub fn compute_raw(&self, inputs: &[Vec<f64>], frames: usize) -> Vec<Vec<f64>> {
        debug_assert_eq!(inputs.len(), self.inputs, "input arity mismatch");
        let double = self.factory.double;

        macro_rules! run {
            ($elem:ty) => {{
                let mut ins: Vec<Vec<$elem>> = inputs
                    .iter()
                    .map(|c| c.iter().take(frames).map(|&v| v as $elem).collect())
                    .collect();
                let mut outs: Vec<Vec<$elem>> =
                    vec![vec![<$elem>::default(); frames]; self.outputs];
                let mut in_ptrs: Vec<*mut FaustFloat> = ins
                    .iter_mut()
                    .map(|c| c.as_mut_ptr().cast::<FaustFloat>())
                    .collect();
                let mut out_ptrs: Vec<*mut FaustFloat> = outs
                    .iter_mut()
                    .map(|c| c.as_mut_ptr().cast::<FaustFloat>())
                    .collect();
                // SAFETY: both pointer arrays have the arity the instance
                // reported, and every input/output buffer holds `frames`
                // elements of the compiled width.
                unsafe {
                    computeCCraneliftDSPInstance(
                        self.dsp,
                        frames as i32,
                        in_ptrs.as_mut_ptr(),
                        out_ptrs.as_mut_ptr(),
                    );
                }
                outs.iter()
                    .map(|c| c.iter().map(|&v| f64::from(v)).collect())
                    .collect()
            }};
        }

        if double { run!(f64) } else { run!(f32) }
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
        // SAFETY: `self.dsp` was produced by this module and is freed exactly
        // once. The factory it came from is freed separately, by `Factory`'s
        // own `Drop`, once every `Probe` holding an `Rc` to it — including
        // this one — has already dropped.
        unsafe {
            deleteCCraneliftDSPInstance(self.dsp);
        }
    }
}

/// One voice: its instance and the control paths [`poly::extract_paths`]
/// found on it.
struct Voice {
    probe: Probe,
    paths: poly::VoiceControlPaths,
}

/// A polyphonic wrapper over N instances of one factory, plus an optional
/// effect run once on their sum.
///
/// Ported from `architecture/faust/dsp/poly-dsp.h`'s `mydsp_poly` in its
/// `fVoiceControl` (dynamically MIDI-allocated) mode — the mode a host
/// controller drives and the only one worth a test tool exposing. The other
/// mode `poly-dsp.h` offers, where every voice always runs, has no audible
/// behaviour distinct from N independent [`Probe`]s and is not ported.
///
/// The allocation and mixing *decisions* live in [`poly::PolyState`], kept
/// deliberately free of FFI so they are unit-testable without a JIT; this
/// type is the thin layer that carries a decision out as an actual zone
/// write or `compute` call.
pub struct PolyProbe {
    voices: Vec<Voice>,
    state: poly::PolyState,
    effect: Option<Probe>,
    stop_level: f64,
    key_fun: poly::KeyConversion,
    vel_fun: poly::VelConversion,
    inputs: usize,
    outputs: usize,
    sample_rate: i32,
}

impl PolyProbe {
    /// JIT-compile `path` once and instantiate `nvoices` independent voices
    /// from it, plus an effect if one is available.
    ///
    /// The effect comes from `effect_path` when given (`--effect FILE`).
    /// Otherwise this follows `FaustPolyDspGenerator`'s trick for a
    /// single-file instrument that declares both `process` and `effect`
    /// (`dsp_poly_factory::getEffectCode`, `poly-dsp.h:1020`): the source is
    /// re-read, wrapped in `environment{}`, and `dsp_code.effect` is
    /// extracted through the same `adapt`/`adaptor` combinator C++ uses. If
    /// that wrapped compile fails — the ordinary case for an instrument with
    /// no integrated effect — it is treated as "no effect" rather than an
    /// error, since there is no reliable way to distinguish "no `effect`
    /// declared" from "the wrapper is broken" from the compiler's diagnostic
    /// text alone; pass `--effect FILE` explicitly if that guess is wrong.
    ///
    /// # Errors
    /// Returns the compiler's own diagnostic text when the process DSP or an
    /// explicitly given effect fails to compile, and a message when an
    /// explicit effect's input arity does not match the poly bus's output
    /// arity — this port does not implement the C++ `adapt`/`adaptor`
    /// channel-count auto-adaptation for that case (design's stated scope:
    /// "a test wrapper, not an audio engine"), only for the inline-effect
    /// extraction above, which already produces code adapted to the process
    /// DSP's own arity by construction.
    #[allow(clippy::too_many_arguments)]
    pub fn compile(
        path: &str,
        import_dirs: &[String],
        sample_rate: i32,
        double: bool,
        opt_level: i32,
        nvoices: usize,
        effect_path: Option<&str>,
        voice_stop_level: f64,
    ) -> Result<Self, String> {
        if nvoices == 0 {
            return Err("nvoices must be at least 1".to_owned());
        }
        let factory = Rc::new(Factory::compile(path, import_dirs, double, opt_level)?);
        let mut voices = Vec::with_capacity(nvoices);
        for _ in 0..nvoices {
            let probe = Probe::instantiate(&factory, sample_rate)?;
            let paths = poly::extract_paths(probe.controls().iter().map(|c| c.path.as_str()));
            voices.push(Voice { probe, paths });
        }
        let inputs = voices[0].probe.inputs();
        let outputs = voices[0].probe.outputs();
        let key_fun = voices[0].paths.key_fun;
        let vel_fun = voices[0].paths.vel_fun;

        let effect = if let Some(effect_path) = effect_path {
            let probe = Probe::compile(effect_path, import_dirs, sample_rate, double, opt_level)?;
            if probe.inputs() != outputs {
                return Err(format!(
                    "--effect expects {} input(s) but the poly bus produces {outputs}; \
                     channel-arity auto-adaptation is out of scope for this tool, pass a \
                     matching effect",
                    probe.inputs()
                ));
            }
            Some(probe)
        } else {
            Self::try_extract_inline_effect(
                path,
                import_dirs,
                sample_rate,
                double,
                opt_level,
                outputs,
            )
        };

        Ok(Self {
            voices,
            state: poly::PolyState::new(nvoices),
            effect,
            stop_level: voice_stop_level,
            key_fun,
            vel_fun,
            inputs,
            outputs,
            sample_rate,
        })
    }

    /// Build a polyphonic probe from Faust source held in memory.
    ///
    /// Same construction as [`PolyProbe::compile`], minus the file: no inline
    /// `effect` extraction is attempted, since that path re-reads the source
    /// from disk. Pass `effect` explicitly if the bus needs one.
    ///
    /// This exists so the polyphonic path can be tested without a `.dsp` on
    /// disk — AGENTS.md section 3 requires tests to be self-contained and not
    /// to depend on a locally installed Faust.
    ///
    /// # Errors
    /// As [`PolyProbe::compile`], plus any compile diagnostic from `source`.
    #[allow(clippy::too_many_arguments)]
    pub fn compile_from_string(
        name: &str,
        source: &str,
        import_dirs: &[String],
        sample_rate: i32,
        double: bool,
        opt_level: i32,
        nvoices: usize,
        effect: Option<&str>,
        voice_stop_level: f64,
    ) -> Result<Self, String> {
        if nvoices == 0 {
            return Err("nvoices must be at least 1".to_owned());
        }
        let factory = Rc::new(Factory::compile_from_string(
            name,
            source,
            import_dirs,
            double,
            opt_level,
        )?);
        let mut voices = Vec::with_capacity(nvoices);
        for _ in 0..nvoices {
            let probe = Probe::instantiate(&factory, sample_rate)?;
            let paths = poly::extract_paths(probe.controls().iter().map(|c| c.path.as_str()));
            voices.push(Voice { probe, paths });
        }
        let inputs = voices[0].probe.inputs();
        let outputs = voices[0].probe.outputs();
        let key_fun = voices[0].paths.key_fun;
        let vel_fun = voices[0].paths.vel_fun;

        let effect = match effect {
            Some(path) => {
                let probe = Probe::compile(path, import_dirs, sample_rate, double, opt_level)?;
                if probe.inputs() != outputs {
                    return Err(format!(
                        "effect expects {} input(s) but the poly bus produces {outputs}",
                        probe.inputs()
                    ));
                }
                Some(probe)
            }
            None => None,
        };

        Ok(Self {
            voices,
            state: poly::PolyState::new(nvoices),
            effect,
            stop_level: voice_stop_level,
            key_fun,
            vel_fun,
            inputs,
            outputs,
            sample_rate,
        })
    }

    /// Attempt the `environment{}` effect extraction described on
    /// [`PolyProbe::compile`]; `None` on any failure, including "no `effect`
    /// declared".
    fn try_extract_inline_effect(
        path: &str,
        import_dirs: &[String],
        sample_rate: i32,
        double: bool,
        opt_level: i32,
        expected_inputs: usize,
    ) -> Option<Probe> {
        let source = std::fs::read_to_string(path).ok()?;
        // Verbatim structure of `dsp_poly_factory::getEffectCode`
        // (`poly-dsp.h:1020`): `adapt`/`adaptor` reconcile the process DSP's
        // output arity with the effect's input arity so `dsp_code.effect` can
        // follow `dsp_code.process` in a chain regardless of channel counts,
        // then `process` is redefined to be the effect alone (fed by that
        // adaptor), which is what makes `dsp_code.effect` reachable as a
        // standalone compiled `process`.
        let wrapped = format!(
            "adapt(1,1) = _; adapt(2,2) = _,_; adapt(1,2) = _ <: _,_; adapt(2,1) = _,_ :> _;\n\
             adaptor(F,G) = adapt(outputs(F),inputs(G));\n\
             dsp_code = environment{{ {source} }};\n\
             process = adaptor(dsp_code.process, dsp_code.effect) : dsp_code.effect;\n"
        );
        let factory = Factory::compile_from_string(
            "faustprobe-effect",
            &wrapped,
            import_dirs,
            double,
            opt_level,
        )
        .ok()?;
        let probe = Probe::instantiate(&Rc::new(factory), sample_rate).ok()?;
        // The adaptor already reconciled arity against the process DSP, so a
        // mismatch here would mean the extraction produced something
        // unexpected; be conservative and decline rather than mix buffers of
        // the wrong width.
        (probe.inputs() == expected_inputs).then_some(probe)
    }

    /// Number of voices in the bus.
    #[must_use]
    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Per-voice audio inputs (almost always 0 for a synthesizer voice).
    #[must_use]
    pub const fn inputs(&self) -> usize {
        self.inputs
    }

    /// Audio outputs of the mixed bus (after the effect, if any).
    #[must_use]
    pub const fn outputs(&self) -> usize {
        self.outputs
    }

    /// Sample rate every voice (and the effect, if any) was initialised with.
    #[must_use]
    pub const fn sample_rate(&self) -> i32 {
        self.sample_rate
    }

    /// Current allocation state of every voice, in voice-table order.
    #[must_use]
    pub fn voice_states(&self) -> &[poly::VoiceState] {
        &self.state.voices
    }

    /// Number of voices not [`poly::FREE_VOICE`].
    #[must_use]
    pub fn active_voice_count(&self) -> usize {
        self.state.active_count()
    }

    /// Whether an effect DSP (explicit or inline-extracted) is chained after
    /// the mix.
    #[must_use]
    pub const fn has_effect(&self) -> bool {
        self.effect.is_some()
    }

    /// The first voice's discovered controls, representative of every voice
    /// since each is an independent clone of the same DSP.
    ///
    /// For `--list-params`: printing one voice's control map rather than N
    /// identical copies.
    #[must_use]
    pub fn voice_controls(&self) -> &ControlMap {
        self.voices[0].probe.controls()
    }

    /// Write `value` to `path` on every voice for which it resolves exactly,
    /// and on the effect if it resolves there instead.
    ///
    /// This is the poly bus's equivalent of the scalar `Probe::set` for
    /// controls that are not the gate/freq/gain triple — a shared filter
    /// cutoff, say — broadcasting to every voice the way the C++ "Voices" tab
    /// group does for a live UI (`dsp_voice_group::buildUserInterface`,
    /// `poly-dsp.h:379`), minus that class's GUI-grouping machinery, which is
    /// a live-performance display convenience out of this tool's scope.
    ///
    /// # Errors
    /// Returns a message if `path` resolves on no voice and not on the
    /// effect.
    pub fn set_all(&self, query: &str, value: f64) -> Result<(), String> {
        // Voices are identical instances of the same DSP, so a fragment that
        // is unambiguous on one is unambiguous on all: resolve once against
        // voice 0, then apply the resulting exact path everywhere.
        //
        // Resolving per voice would be equivalent but wasteful; requiring an
        // exact path here would be worse than either, because `--set` accepts
        // a fragment in scalar mode and the same command would then fail the
        // moment `--nvoices` was raised — a trap, not a safety feature.
        let resolved = match self.voices.first() {
            Some(voice) => match voice.probe.controls().resolve(query) {
                Resolution::Unique(control) => Some(control.path.clone()),
                Resolution::Ambiguous(candidates) => {
                    return Err(format!(
                        "`{query}` is ambiguous on a voice, matches: {}",
                        candidates.join(", ")
                    ));
                }
                Resolution::NotFound => None,
            },
            None => None,
        };
        let path = resolved.as_deref().unwrap_or(query);

        let mut hit = false;
        for voice in &self.voices {
            if voice.probe.set_exact(path, value).is_ok() {
                hit = true;
            }
        }
        // The effect is a different DSP with its own control map, so it gets
        // its own resolution rather than the voice's exact path.
        if let Some(effect) = &self.effect
            && effect.set(query, value).is_ok()
        {
            hit = true;
        }
        if hit {
            Ok(())
        } else {
            Err(format!(
                "no control matching `{query}` on any voice or the effect"
            ))
        }
    }

    /// Note on: allocate a voice and sound `pitch` at `velocity` (0-127).
    ///
    /// Returns the allocated voice index. Mirrors `mydsp_poly::keyOn`
    /// (`poly-dsp.h:900`) — see [`poly::PolyState::key_on`] for why this
    /// never reuses an already-sounding voice for the same pitch.
    pub fn key_on(&mut self, pitch: i32, velocity: i32) -> usize {
        let (voice, write) = self
            .state
            .key_on(pitch, velocity, self.key_fun, self.vel_fun);
        if let Some(write) = write {
            apply_write(&self.voices[voice], write);
        }
        voice
    }

    /// Note off for `pitch`: release the oldest voice still sounding it.
    ///
    /// `hard` frees the voice immediately rather than letting it decay below
    /// the stop level, matching `dsp_voice::keyOff(hard)`. Returns the
    /// released voice index, or `None` if no voice is sounding `pitch`.
    pub fn key_off(&mut self, pitch: i32, hard: bool) -> Option<usize> {
        let (voice, write) = self.state.key_off(pitch, hard)?;
        apply_write(&self.voices[voice], write);
        Some(voice)
    }

    /// Render one host block across every voice, mix, and run the effect.
    ///
    /// Mirrors `mydsp_poly::compute` (`poly-dsp.h:828`) in its
    /// `fVoiceControl` branch. `frames` should not exceed the reference
    /// `MIX_BUFFER_SIZE` (4096); nothing here enforces that bound the way
    /// `poly-dsp.h`'s `assert` does; it is a design constraint of a
    /// fixed-size C mix buffer that Rust's `Vec`-backed buffers do not share.
    #[must_use]
    pub fn compute(&mut self, frames: usize) -> Vec<Vec<f64>> {
        let mut mixed = vec![vec![0.0_f64; frames]; self.outputs];
        let silence: Vec<Vec<f64>> = vec![vec![0.0; frames]; self.inputs];

        for i in 0..self.voices.len() {
            let cur_note = self.state.voices[i].cur_note;
            if cur_note == poly::FREE_VOICE {
                continue;
            }
            let voice_out = if cur_note == poly::LEGATO_VOICE {
                self.compute_legato(i, frames, &silence)
            } else {
                self.voices[i].probe.compute_raw(&silence, frames)
            };
            let level = poly::mix_check_voice(&voice_out, &mut mixed);
            self.state.record_level(i, level, self.stop_level);
        }

        match &self.effect {
            Some(effect) => effect.compute_raw(&mixed, frames),
            None => mixed,
        }
    }

    /// Render a voice being stolen: the outgoing note's tail on the first
    /// half of the block, the incoming note's onset on the second half,
    /// faded across the splice.
    ///
    /// Mirrors `dsp_voice::computeLegato` (`poly-dsp.h:213`) plus the
    /// `fadeOut(count/2, ...)` call `mydsp_poly::compute` makes immediately
    /// after it (`poly-dsp.h:843`) — kept together here because in C++ they
    /// are two calls the caller must remember to sequence, and getting that
    /// sequencing wrong (fading before rendering, say) would silently mute
    /// the wrong half.
    fn compute_legato(
        &mut self,
        voice: usize,
        frames: usize,
        silence: &[Vec<f64>],
    ) -> Vec<Vec<f64>> {
        // Reset envelope: gate off before rendering the outgoing note's tail,
        // exactly as `computeLegato`'s first act.
        for path in self.voices[voice].paths.gate.clone() {
            let _ = self.voices[voice].probe.set_exact(&path, 0.0);
        }

        let half = frames / 2;
        let rest = frames - half;
        let first_input: Vec<Vec<f64>> = silence.iter().map(|c| c[..half].to_vec()).collect();
        let mut first = self.voices[voice].probe.compute_raw(&first_input, half);

        // Apply the queued note now that the outgoing tail has rendered.
        let write = self.state.apply_legato(voice, self.key_fun, self.vel_fun);
        apply_write(&self.voices[voice], write);

        let second_input: Vec<Vec<f64>> = silence
            .iter()
            .map(|c| c[half..half + rest].to_vec())
            .collect();
        let second = self.voices[voice].probe.compute_raw(&second_input, rest);
        for (channel, tail) in first.iter_mut().zip(second) {
            channel.extend(tail);
        }

        poly::fade_out(&mut first, half);
        first
    }
}

/// Carry out a [`poly::VoiceWrite`] on one voice's zones.
fn apply_write(voice: &Voice, write: poly::VoiceWrite) {
    match write {
        poly::VoiceWrite::KeyOn { freq, gain } => {
            for path in &voice.paths.freq {
                let _ = voice.probe.set_exact(path, freq);
            }
            for path in &voice.paths.gate {
                let _ = voice.probe.set_exact(path, 1.0);
            }
            for path in &voice.paths.gain {
                let _ = voice.probe.set_exact(path, gain);
            }
        }
        poly::VoiceWrite::KeyOff => {
            for path in &voice.paths.gate {
                let _ = voice.probe.set_exact(path, 0.0);
            }
        }
    }
}
