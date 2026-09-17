//! N voices of one factory and an optional effect: the polyphonic wrapper.

use std::rc::Rc;
use std::time::Instant;

use super::{Factory, Frame, Probe, next_block, push_block};
use crate::probe::params::{ControlMap, Resolution, Write};
use crate::probe::poly;
use crate::probe::render::{InputMode, RenderStats, StatsAccumulator};
use crate::probe::schedule::{Event, Schedule};
use crate::probe::timing::BlockTimer;

/// One voice: its instance and the control paths [`poly::extract_paths`]
/// found on it.
struct Voice {
    probe: Probe,
    paths: poly::VoiceControlPaths,
}

/// How a polyphonic render should be driven: what [`RenderSpec`] holds, less
/// the driving of buttons (nothing presses a voice's gate but a note).
#[derive(Debug, Clone)]
pub struct PolyRenderSpec {
    /// Total frames to render.
    pub frames: usize,
    /// Frames per host block, before the schedule shortens one.
    pub block: usize,
    /// Excitation of the instrument's inputs, handed to every playing voice.
    pub input: InputMode,
    /// First frame included in statistics and dump.
    pub skip: usize,
    /// Notes and control writes, applied at their exact frames.
    pub schedule: Schedule,
    /// Magnitude the window must stay under (`--fail-above`).
    pub limit: crate::probe::render::RenderLimit,
    /// Time every host block (`--time`).
    pub time: bool,
}

/// Where a broadcast write lands in a polyphonic instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyTarget {
    /// The control of that path on every voice.
    Voices,
    /// A control of the effect the voices' mix runs through.
    Effect,
}

impl PolyTarget {
    /// `on every voice` or `on the effect`, for a message.
    #[must_use]
    pub const fn place(self) -> &'static str {
        match self {
            Self::Voices => "on every voice",
            Self::Effect => "on the effect",
        }
    }
}

/// One control a broadcast write reaches, with the value it would take.
#[derive(Debug, Clone)]
pub struct PolyWrite<'a> {
    pub target: PolyTarget,
    pub write: Write<'a>,
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
        let voices = Self::voices_of(&factory, sample_rate, nvoices)?;
        let outputs = voices[0].probe.outputs();

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

        Ok(Self::assemble(
            voices,
            effect,
            voice_stop_level,
            sample_rate,
        ))
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
        let voices = Self::voices_of(&factory, sample_rate, nvoices)?;
        let outputs = voices[0].probe.outputs();

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

        Ok(Self::assemble(
            voices,
            effect,
            voice_stop_level,
            sample_rate,
        ))
    }

    /// `nvoices` independent instances of one compile, each with the control
    /// paths a note writes to.
    fn voices_of(
        factory: &Rc<Factory>,
        sample_rate: i32,
        nvoices: usize,
    ) -> Result<Vec<Voice>, String> {
        let mut voices = Vec::with_capacity(nvoices);
        for _ in 0..nvoices {
            let probe = Probe::instantiate(factory, sample_rate)?;
            let paths = poly::extract_paths(probe.controls().iter().map(|c| c.path.as_str()));
            voices.push(Voice { probe, paths });
        }
        Ok(voices)
    }

    /// The bus over `voices` (at least one): its arity and its key and
    /// velocity conversions are those of the first, the voices being instances
    /// of one DSP.
    fn assemble(
        voices: Vec<Voice>,
        effect: Option<Probe>,
        stop_level: f64,
        sample_rate: i32,
    ) -> Self {
        Self {
            state: poly::PolyState::new(voices.len()),
            effect,
            stop_level,
            key_fun: voices[0].paths.key_fun,
            vel_fun: voices[0].paths.vel_fun,
            inputs: voices[0].probe.inputs(),
            outputs: voices[0].probe.outputs(),
            sample_rate,
            voices,
        }
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

    /// What [`PolyProbe::set_all`] would write for `query`, and where: on the
    /// voices (one entry, the voices being instances of one DSP), on the
    /// effect, or on both, each with the range of its own control.
    ///
    /// The caller decides what a value outside a range means
    /// ([`Write::in_range`]): the command line refuses it, or accepts it under
    /// `--clamp` and says so. Nothing is written.
    ///
    /// # Errors
    /// A fragment that is ambiguous on a voice or on the effect (with its
    /// candidates), a bargraph, or a query that resolves nowhere.
    pub fn check_write(&self, query: &str, value: f64) -> Result<Vec<PolyWrite<'_>>, String> {
        // Voices are identical instances of the same DSP, so a fragment that
        // is unambiguous on one is unambiguous on all: resolve once against
        // voice 0. The effect is a different DSP with its own control map, so
        // it gets its own resolution rather than the voice's exact path.
        //
        // Requiring an exact path here would be worse, because `--set`
        // accepts a fragment in scalar mode and the same command would then
        // fail the moment `--nvoices` was raised: a trap, not a safety
        // feature.
        let maps = [
            (
                PolyTarget::Voices,
                self.voices.first().map(|voice| voice.probe.controls()),
            ),
            (
                PolyTarget::Effect,
                self.effect.as_ref().map(Probe::controls),
            ),
        ];
        let mut writes = Vec::new();
        for (target, controls) in maps {
            let Some(controls) = controls else { continue };
            if matches!(controls.resolve(query), Resolution::NotFound) {
                continue;
            }
            let write = controls
                .check_write(query, value)
                .map_err(|error| format!("{error} ({})", target.place()))?;
            writes.push(PolyWrite { target, write });
        }
        if writes.is_empty() {
            return Err(format!(
                "no control matching `{query}` on any voice or the effect"
            ));
        }
        Ok(writes)
    }

    /// Write `value` to the control `query` names on every voice, and on the
    /// effect if it resolves there, clamped to each control's declared range
    /// as [`Probe::set`] clamps it.
    ///
    /// This is the poly bus's equivalent of the scalar `Probe::set` for
    /// controls that are not the gate/freq/gain triple — a shared filter
    /// cutoff, say — broadcasting to every voice the way the C++ "Voices" tab
    /// group does for a live UI (`dsp_voice_group::buildUserInterface`,
    /// `poly-dsp.h:379`), minus that class's GUI-grouping machinery, which is
    /// a live-performance display convenience out of this tool's scope.
    ///
    /// The clamp is that of a widget: a user's write stays in the range, which
    /// this used to guarantee on the effect only, a voice taking whatever it
    /// was given. What a **note** writes (`key_on`: the frequency of its
    /// pitch, its gain, its gate) is not a widget's doing and stays unclamped,
    /// as in `poly-dsp.h`.
    ///
    /// # Errors
    /// As [`PolyProbe::check_write`].
    pub fn set_all(&self, query: &str, value: f64) -> Result<(), String> {
        for PolyWrite { target, write } in self.check_write(query, value)? {
            match target {
                PolyTarget::Voices => {
                    for voice in &self.voices {
                        voice.probe.set_exact(&write.control.path, write.applied)?;
                    }
                }
                PolyTarget::Effect => {
                    if let Some(effect) = &self.effect {
                        effect.set_exact(&write.control.path, write.applied)?;
                    }
                }
            }
        }
        Ok(())
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

    /// Whether the instrument was compiled for double-precision samples.
    #[must_use]
    pub fn is_double(&self) -> bool {
        self.voices
            .first()
            .is_some_and(|voice| voice.probe.is_double())
    }

    /// Render `spec`: the scheduled notes and writes applied at their exact
    /// frames, `on_frame` invoked for each frame at or after the skip point,
    /// and the statistics of that window returned, **the very statistics a
    /// scalar render has** ([`RenderStats`]): the first non-finite sample and
    /// the number of frames affected, the first sample above the limit, the
    /// peak's frame, the subnormal samples at the instrument's width.
    ///
    /// The loop used to live in the command line with statistics of its own,
    /// a peak and an RMS over the finite samples: a render that ran away to
    /// infinity reported `rms=inf` and succeeded.
    ///
    /// # Errors
    /// A scheduled write that does not resolve ([`PolyProbe::set_all`]); the
    /// command line checks them all before rendering.
    pub fn render<F>(
        &mut self,
        spec: &PolyRenderSpec,
        mut on_frame: F,
    ) -> Result<RenderStats, String>
    where
        F: FnMut(usize, &[f64]),
    {
        let mut acc = StatsAccumulator::with_limit(self.outputs, spec.skip, spec.limit)
            .at_width(self.is_double());
        let block = spec.block.max(1);
        let sample_rate = f64::from(self.sample_rate);
        let mut timer = spec.time.then(|| BlockTimer::new(sample_rate));
        let mut frame: Frame = vec![0.0; self.outputs];
        let mut written = 0usize;
        while written < spec.frames {
            // Apply what is due exactly here, then shorten the block so the
            // next event also lands on a boundary: the note timing is what a
            // release measurement reads, so rounding it to the block grid
            // would put a systematic error straight into the result.
            for event in spec.schedule.at(written) {
                match event {
                    Event::NoteOn { pitch, velocity } => {
                        self.key_on(*pitch, *velocity);
                    }
                    Event::NoteOff { pitch } => {
                        self.key_off(*pitch, false);
                    }
                    Event::SetParam { path, value } => self.set_all(path, *value)?,
                }
            }
            let n = next_block(&spec.schedule, block, spec.frames, written);
            // position-addressed, as for a scalar render: the excitation at a
            // frame does not depend on how the render was cut into blocks
            let inputs: Vec<Vec<f64>> = (0..self.inputs)
                .map(|ch| {
                    (0..n)
                        .map(|j| spec.input.sample(ch, written + j, sample_rate))
                        .collect()
                })
                .collect();
            // the voices, their mix and the effect: what a host's callback runs
            let started = timer.is_some().then(Instant::now);
            let block_out = self.compute_with_inputs(&inputs, n);
            if let (Some(timer), Some(started)) = (timer.as_mut(), started) {
                timer.record(written, n, started.elapsed());
            }
            push_block(
                &mut acc,
                &mut frame,
                (written, n, spec.skip),
                |ch, j| block_out[ch][j],
                &mut on_frame,
            );
            written += n;
        }
        let mut stats = acc.finish();
        stats.timing = timer.map(BlockTimer::finish);
        Ok(stats)
    }

    /// [`PolyProbe::compute_with_inputs`] with silence on every input: what an
    /// instrument without inputs is given, there being nothing else to give.
    #[must_use]
    pub fn compute(&mut self, frames: usize) -> Vec<Vec<f64>> {
        let silence: Vec<Vec<f64>> = vec![vec![0.0; frames]; self.inputs];
        self.compute_with_inputs(&silence, frames)
    }

    /// Render one host block across every voice, mix, and run the effect.
    ///
    /// Mirrors `mydsp_poly::compute` (`poly-dsp.h:828`) in its
    /// `fVoiceControl` branch, **the host's inputs included**: the reference
    /// hands the same `inputs` to every playing voice
    /// (`voice->compute(count, inputs, fMixBuffer)`), which is what makes a
    /// voice with an input (a vocoder band, a per-note filter on an external
    /// signal) an instrument at all. This port gave every voice silence, and
    /// the command line's `--in` meant nothing under `--nvoices`.
    ///
    /// `inputs[ch]` holds at least `frames` samples for each of
    /// [`PolyProbe::inputs`] channels. `frames` should not exceed the
    /// reference `MIX_BUFFER_SIZE` (4096); nothing here enforces that bound
    /// the way `poly-dsp.h`'s `assert` does; it is a design constraint of a
    /// fixed-size C mix buffer that Rust's `Vec`-backed buffers do not share.
    #[must_use]
    pub fn compute_with_inputs(&mut self, inputs: &[Vec<f64>], frames: usize) -> Vec<Vec<f64>> {
        debug_assert_eq!(inputs.len(), self.inputs, "input arity mismatch");
        let mut mixed = vec![vec![0.0_f64; frames]; self.outputs];

        for i in 0..self.voices.len() {
            let cur_note = self.state.voices[i].cur_note;
            if cur_note == poly::FREE_VOICE {
                continue;
            }
            let voice_out = if cur_note == poly::LEGATO_VOICE {
                self.compute_legato(i, frames, inputs)
            } else {
                self.voices[i].probe.compute_raw(inputs, frames)
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
        inputs: &[Vec<f64>],
    ) -> Vec<Vec<f64>> {
        // Reset envelope: gate off before rendering the outgoing note's tail,
        // exactly as `computeLegato`'s first act.
        for path in self.voices[voice].paths.gate.clone() {
            let _ = self.voices[voice].probe.set_exact(&path, 0.0);
        }

        let half = frames / 2;
        let rest = frames - half;
        // each half of the block reads its own half of the inputs, as
        // `computeSlice(offset, slice, inputs, outputs)` does
        let first_input: Vec<Vec<f64>> = inputs.iter().map(|c| c[..half].to_vec()).collect();
        let mut first = self.voices[voice].probe.compute_raw(&first_input, half);

        // Apply the queued note now that the outgoing tail has rendered.
        let write = self.state.apply_legato(voice, self.key_fun, self.vel_fun);
        apply_write(&self.voices[voice], write);

        let second_input: Vec<Vec<f64>> = inputs
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
