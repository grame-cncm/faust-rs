//! `FbcDspInstance<R>` — runtime DSP instance with heaps and compute loop.
//!
//! # Source provenance (C++)
//! - `interpreter_dsp_aux<REAL, TRACE>` in `interpreter_dsp_aux.hh`
//!
//! # Design notes
//! - Holds a reference to its parent [`FbcDspFactory`] and owns an
//!   [`FbcExecutor`] with heaps sized from the factory.
//! - Lifecycle: `new()` → `init(sr)` → `compute()` loop.
//! - The factory must be optimized before creating an instance; this is
//!   enforced by requiring `&mut FbcDspFactory` in `new()`.
//! - No `TRACE` template parameter — tracing is a future runtime option.

use super::executor::{FbcExecError, FbcExecutor};
use super::factory::FbcDspFactory;
use super::real::FbcReal;
use super::soundfile::Soundfile;

/// Runtime DSP instance with its own heaps.
///
/// # Source provenance (C++)
/// - `interpreter_dsp_aux<REAL, TRACE>` in `interpreter_dsp_aux.hh`
///
/// # Lifetime
/// The instance borrows the factory for the duration of its lifetime.
/// The factory must outlive all its instances.
///
/// The split between [`FbcDspFactory`] and [`FbcExecutor`] mirrors the C++
/// interpreter design:
/// - the factory owns immutable bytecode and metadata,
/// - the instance owns mutable heaps and lifecycle state.
pub struct FbcDspInstance<'a, R: FbcReal> {
    factory: &'a FbcDspFactory<R>,
    executor: FbcExecutor<R>,
    initialized: bool,
    cycle: usize,
}

/// Structured runtime error returned by [`FbcDspInstance::try_compute`].
///
/// This currently wraps executor failures only, but keeping a dedicated
/// instance-level error type leaves room for future lifecycle or host-I/O
/// validation errors without changing the public compute signature.
#[derive(Debug)]
pub enum FbcDspRuntimeError {
    /// Bytecode executor reported a structured execution failure.
    Exec(FbcExecError),
}

impl std::fmt::Display for FbcDspRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exec(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for FbcDspRuntimeError {}

impl From<FbcExecError> for FbcDspRuntimeError {
    fn from(value: FbcExecError) -> Self {
        Self::Exec(value)
    }
}

impl<'a, R: FbcReal> FbcDspInstance<'a, R> {
    /// Creates a new DSP instance from a factory.
    ///
    /// The factory is optimized (if not already) before the instance is created.
    /// The executor is allocated with heap sizes from the factory.
    ///
    /// Requiring `&mut FbcDspFactory` preserves the one-shot optimization
    /// contract before instances start borrowing immutable factory state.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux(interpreter_dsp_factory_aux*)` constructor
    ///   in `interpreter_dsp_aux.hh` (lines 536–545).
    #[must_use]
    pub fn new(factory: &'a mut FbcDspFactory<R>) -> Self {
        // Done before createFBCExecutor that may compile blocks...
        factory.optimize();

        let mut executor = FbcExecutor::new(
            factory.int_heap_size as usize,
            factory.real_heap_size as usize,
        );

        // Populate soundfile slots with default silence until the host provides
        // real audio files via the UI callback.
        let sf_count = factory.soundfile_count();
        executor.soundfiles = (0..sf_count)
            .map(|_| Box::new(Soundfile::default_silence()))
            .collect();

        Self {
            factory,
            executor,
            initialized: false,
            cycle: 0,
        }
    }

    /// Full initialization entrypoint used by the public DSP lifecycle.
    ///
    /// This marks the instance initialized, then runs the canonical C++ backend
    /// sequence: `classInit(sample_rate); instanceInit(sample_rate);`.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux::init()` in `interpreter_dsp_aux.hh` (lines 653–668).
    pub fn init(&mut self, sample_rate: i32) {
        self.initialized = true;
        self.class_init(sample_rate);
        self.instance_init(sample_rate);
    }

    /// Instance initialization: `constants + reset UI + clear`.
    ///
    /// This matches the Faust DSP lifecycle contract rather than merely setting
    /// heap defaults: stateful tables, controls, and delay memory are all
    /// re-established here.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux::instanceInit()` in `interpreter_dsp_aux.hh`
    ///   (lines 637–651).
    pub fn instance_init(&mut self, sample_rate: i32) {
        self.instance_constants(sample_rate);
        self.instance_reset_user_interface();
        self.instance_clear();
    }

    /// Executes the static init block.
    ///
    /// The sample rate is published to the heap before the block runs.
    /// `classInit` takes a `sample_rate` in every backend, and a generated
    /// table whose content depends on it reads that argument — the C++ output
    /// forwards it as `sig0->instanceInitmydspSIG0(sample_rate)`. Here a
    /// `sample_rate` function argument compiles to a load of the `fSampleRate`
    /// heap slot, which otherwise only `instanceConstants` writes, and that
    /// runs afterwards; `subcontainer1.dsp` then fills its table from 0 and
    /// reads back 1 (`fmax(1, 0)`) instead of the sample rate.
    ///
    /// This departs from the C++ interpreter, whose `classInit` ignores its
    /// argument — it can afford to because it never had a generator to feed.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux::classInit()` in `interpreter_dsp_aux.hh`
    ///   (lines 570–584).
    pub fn class_init(&mut self, sample_rate: i32) {
        self.executor.int_heap[self.factory.sr_offset as usize] = sample_rate;
        self.executor
            .execute_block(&self.factory.arena, self.factory.static_init_block);
    }

    /// Sets sample rate and executes the init (constants) block.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux::instanceConstants()` in `interpreter_dsp_aux.hh`
    ///   (lines 586–603).
    pub fn instance_constants(&mut self, sample_rate: i32) {
        // Store sample_rate in 'fSampleRate' variable at correct offset in fIntHeap.
        self.executor.int_heap[self.factory.sr_offset as usize] = sample_rate;

        self.executor
            .execute_block(&self.factory.arena, self.factory.init_block);
    }

    /// Executes the reset UI block (sets UI controls to default values).
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux::instanceResetUserInterface()` in
    ///   `interpreter_dsp_aux.hh` (lines 605–619).
    pub fn instance_reset_user_interface(&mut self) {
        self.executor
            .execute_block(&self.factory.arena, self.factory.reset_ui_block);
    }

    /// Executes the clear block (zeros delay lines, state variables).
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux::instanceClear()` in `interpreter_dsp_aux.hh`
    ///   (lines 621–635).
    pub fn instance_clear(&mut self) {
        self.executor
            .execute_block(&self.factory.arena, self.factory.clear_block);
    }

    /// Processes one buffer of audio samples.
    ///
    /// # Arguments
    /// - `count`: number of frames to compute.
    /// - `inputs`: audio input buffers (`inputs[channel][frame]`).
    /// - `outputs`: audio output buffers (`outputs[channel][frame]`).
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux::compute()` in `interpreter_dsp_aux.hh`
    ///   (lines 706–790).
    ///
    /// The interpreter keeps the historical two-stage compute split:
    /// - a per-buffer control block,
    /// - then the DSP/sample-loop block with actual audio I/O.
    pub fn compute(&mut self, count: i32, inputs: &[&[R]], outputs: &mut [&mut [R]]) {
        if count == 0 {
            return; // Beware: compiled loop does not work with an index of 0.
        }

        // Set count in 'count' variable at the correct offset in fIntHeap.
        self.executor.int_heap[self.factory.count_offset as usize] = count;

        // Executes the 'control' block.
        self.executor
            .execute_block(&self.factory.arena, self.factory.compute_block);

        // Executes the 'DSP' block (with audio I/O).
        self.executor.execute_block_io(
            &self.factory.arena,
            self.factory.compute_dsp_block,
            inputs,
            outputs,
        );

        self.cycle += 1;
    }

    /// Processes one buffer of audio samples and returns a structured runtime
    /// error for detected execution failures.
    ///
    /// Unlike [`Self::compute`], this surface is intended for tests and hosts
    /// that want to handle malformed/generated-invalid bytecode without panic.
    pub fn try_compute(
        &mut self,
        count: i32,
        inputs: &[&[R]],
        outputs: &mut [&mut [R]],
    ) -> Result<(), FbcDspRuntimeError> {
        if count == 0 {
            return Ok(()); // Beware: compiled loop does not work with an index of 0.
        }

        // Set count in 'count' variable at the correct offset in fIntHeap.
        self.executor.int_heap[self.factory.count_offset as usize] = count;

        // Executes the 'control' block.
        self.executor
            .execute_block(&self.factory.arena, self.factory.compute_block);

        // Executes the 'DSP' block (with audio I/O).
        self.executor.try_execute_block_io(
            &self.factory.arena,
            self.factory.compute_dsp_block,
            inputs,
            outputs,
        )?;

        self.cycle += 1;
        Ok(())
    }

    /// Returns the current sample rate.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_aux::getSampleRate()` in `interpreter_dsp_aux.hh`.
    #[must_use]
    pub fn get_sample_rate(&self) -> i32 {
        self.executor.int_heap[self.factory.sr_offset as usize]
    }

    /// Returns the number of audio inputs.
    #[must_use]
    pub fn get_num_inputs(&self) -> i32 {
        self.factory.num_inputs
    }

    /// Returns the number of audio outputs.
    #[must_use]
    pub fn get_num_outputs(&self) -> i32 {
        self.factory.num_outputs
    }

    /// Returns whether `init()` has been called.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns the compute cycle counter.
    #[must_use]
    pub fn cycle(&self) -> usize {
        self.cycle
    }

    /// Returns the UI instruction list collected at compile time.
    ///
    /// Hosts use this to discover slider/numentry labels and their bound
    /// real-heap offsets. The returned slice is the same one stored on the
    /// factory; layout matches the C++ `buildUserInterface` traversal order.
    #[must_use]
    pub fn ui_instructions(&self) -> &[super::bytecode::FbcUiInstruction<R>] {
        &self.factory.ui_block
    }

    /// Reads the current value of a real-heap slot.
    ///
    /// Useful for inspecting slider state after a compute cycle, or for
    /// reading bargraph metering zones. Out-of-range offsets return `None`
    /// instead of panicking.
    #[must_use]
    pub fn get_real_zone(&self, offset: i32) -> Option<R> {
        let idx = usize::try_from(offset).ok()?;
        self.executor.real_heap.get(idx).copied()
    }

    /// Writes a value into a real-heap slot.
    ///
    /// Used by host gradient-descent loops to update slider parameters
    /// between compute cycles. Returns `true` when the offset was in range
    /// and the write happened.
    pub fn set_real_zone(&mut self, offset: i32, value: R) -> bool {
        let Ok(idx) = usize::try_from(offset) else {
            return false;
        };
        let Some(slot) = self.executor.real_heap.get_mut(idx) else {
            return false;
        };
        *slot = value;
        true
    }

    /// Replaces one runtime soundfile slot with host-provided data.
    ///
    /// # Source provenance (C++)
    /// - `SoundUI::addSoundfile` writes a `Soundfile*` into the DSP soundfile
    ///   zone before processing starts.
    ///
    /// The slot index is the `offset` carried by the `AddSoundfile` UI
    /// instruction.
    pub fn set_soundfile(&mut self, slot: usize, soundfile: Soundfile) -> bool {
        let Some(current) = self.executor.soundfiles.get_mut(slot) else {
            return false;
        };
        **current = soundfile;
        true
    }
}

#[cfg(test)]
mod tests;
