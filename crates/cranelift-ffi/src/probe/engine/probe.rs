//! One instance of a compiled program: its controls, and the render loop.

use std::rc::Rc;
use std::time::Instant;

use crate::instance::{
    buildUserInterfaceCCraneliftDSPInstance, computeCCraneliftDSPInstance,
    createCCraneliftDSPInstance, deleteCCraneliftDSPInstance, getNumInputsCCraneliftDSPInstance,
    getNumOutputsCCraneliftDSPInstance, initCCraneliftDSPInstance,
    instanceClearCCraneliftDSPInstance, instanceResetUserInterfaceCCraneliftDSPInstance,
};
use crate::types::{CraneliftDspInstance, FaustFloat};
use ffi_common::abi::FfiFaustFloat;

use super::{Factory, Frame, RenderSpec, next_block, push_block};
use crate::probe::params::{Control, ControlKind, ControlMap};
use crate::probe::render::{RenderStats, StatsAccumulator};
use crate::probe::schedule::Event;
use crate::probe::timing::BlockTimer;

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
        Self::compile_with_args(path, import_dirs, &[], sample_rate, double, opt_level)
    }

    /// [`Probe::compile`] with extra compiler arguments appended verbatim
    /// (see [`Factory::compile_with_args`]).
    ///
    /// # Errors
    /// As [`Probe::compile`].
    pub fn compile_with_args(
        path: &str,
        import_dirs: &[String],
        extra_args: &[String],
        sample_rate: i32,
        double: bool,
        opt_level: i32,
    ) -> Result<Self, String> {
        let factory = Rc::new(Factory::compile_with_args(
            path,
            import_dirs,
            extra_args,
            double,
            opt_level,
        )?);
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

    /// Read a zone, at the width the factory was compiled with.
    ///
    /// The counterpart of [`Probe::set_zone`], for the zones the program
    /// writes: a bargraph's value is whatever its signal was at the last
    /// sample of the last `compute` call.
    pub(crate) fn get_zone(&self, zone: *mut FfiFaustFloat) -> f64 {
        if zone.is_null() {
            return 0.0;
        }
        // SAFETY: as for `set_zone`: the zone came from this instance's
        // `buildUserInterface` and stays valid until the instance is dropped.
        unsafe {
            if self.factory.double {
                *zone.cast::<f64>()
            } else {
                f64::from(*zone)
            }
        }
    }

    /// The current value of every bargraph, ordered by path.
    ///
    /// A bargraph is updated by `compute`, once per sample, so between two
    /// calls its zone holds the value of the last sample of the last block:
    /// read after a render it is the value at the render's end, and its time
    /// resolution during one is the block size. Before any `compute` it holds
    /// whatever the instance was initialised with.
    #[must_use]
    pub fn bargraphs(&self) -> Vec<(String, f64)> {
        self.controls
            .iter()
            .filter(|c| c.kind == ControlKind::Bargraph)
            .map(|c| (c.path.clone(), self.get_zone(c.zone)))
            .collect()
    }

    /// The current value of every writable control, ordered by path.
    ///
    /// What a silent render is explained with: a button or a checkbox still
    /// at 0 is the usual reason an instrument outputs exact zeros.
    #[must_use]
    pub fn control_values(&self) -> Vec<(&Control, f64)> {
        self.controls
            .iter()
            .filter(|c| c.kind.is_writable())
            .map(|c| (c, self.get_zone(c.zone)))
            .collect()
    }

    /// Check that `query` names exactly one control that can be written.
    ///
    /// What `--set`, `--sweep` and `--at` validate before any render: a
    /// bargraph resolves like a control but is an output, and a write to it
    /// would be overwritten by the next `compute`, so a sweep over one would
    /// print rows that look like a measurement and are not.
    ///
    /// # Errors
    /// Names the bargraph, the candidates of an ambiguous fragment, or the
    /// query when nothing matches.
    pub fn check_writable(&self, query: &str) -> Result<(), String> {
        self.controls.writable(query).map(|_| ())
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
    /// The value written is the one [`ControlMap::check_write`] reports as
    /// applied, so that what the command line says of a clamp (`# clamped
    /// PATH: NaN -> 0.5`) is what the render ran with: this used to clamp on
    /// its own, and a NaN, which no range holds, went to the zone as it was.
    ///
    /// # Errors
    /// Returns a message naming the candidates when the query is ambiguous,
    /// stating the query when nothing matches, or naming the bargraph when the
    /// query is one (an output of the program, which `compute` overwrites).
    pub fn set(&self, query: &str, value: f64) -> Result<(), String> {
        let write = self.controls.check_write(query, value)?;
        self.set_zone(write.control.zone, write.applied);
        Ok(())
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
        let double = self.factory.double;
        let mut acc =
            StatsAccumulator::with_limit(self.outputs, spec.skip, spec.limit).at_width(double);
        let block = spec.block.max(1);
        let sample_rate = f64::from(self.sample_rate);
        let mut timer = spec.time.then(|| BlockTimer::new(sample_rate));
        let mut frame: Frame = vec![0.0; self.outputs];

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
                    let n = next_block(&spec.schedule, block, spec.frames, written);
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
                    // the `compute` call and nothing else: not the
                    // excitation, not the statistics, not the dump
                    let started = timer.is_some().then(Instant::now);
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
                    if let (Some(timer), Some(started)) = (timer.as_mut(), started) {
                        timer.record(written, n, started.elapsed());
                    }
                    push_block(
                        &mut acc,
                        &mut frame,
                        (written, n, spec.skip),
                        |ch, j| outs[ch][j] as f64,
                        &mut on_frame,
                    );
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
        let mut stats = acc.finish();
        stats.timing = timer.map(BlockTimer::finish);
        stats
    }

    /// [`Probe::render`] that keeps the window's samples, for a comparison.
    #[must_use]
    pub fn collect(&self, spec: &RenderSpec) -> (RenderStats, crate::probe::compare::Samples) {
        let mut channels =
            vec![Vec::with_capacity(spec.frames.saturating_sub(spec.skip)); self.outputs];
        let stats = self.render(spec, |_, samples| {
            for (channel, value) in channels.iter_mut().zip(samples) {
                channel.push(*value);
            }
        });
        (
            stats,
            crate::probe::compare::Samples {
                start: spec.skip,
                channels,
            },
        )
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
