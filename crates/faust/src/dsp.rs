//! An instance of a compiled program.

use std::sync::Arc;

use crate::backend::{RawInstance, Sample};
use crate::controls::{Control, ControlMap, MetadataSink};
use crate::factory::{Factory, FactoryInner};
use crate::{Backend, Error, ErrorKind, Precision};

/// An instance: its state, its sample rate, its controls. Owns a reference
/// to its factory, so it can outlive the host's [`Factory`] handles. `Send`
/// and `Sync`: it can be moved to another thread, and shared, since every
/// `&self` method only reads; `compute_*`, `set` and the initialisations
/// take `&mut self`.
pub struct Dsp {
    // Declared first: dropped before the factory reference below.
    raw: RawInstance,
    factory: Arc<FactoryInner>,
    controls: ControlMap,
    inputs: usize,
    outputs: usize,
    /// Conversion buffers for the width the backend does not exchange.
    scratch_f32: Vec<Vec<f32>>,
    scratch_f64: Vec<Vec<f64>>,
}

// SAFETY: the instance pointer is only ever used through `&mut self` or
// `&self` of one `Dsp`, and both backends' instances are `Send`.
unsafe impl Send for Dsp {}

// SAFETY: through `&self`, a `Dsp` only reads, so concurrent `&self` calls
// are concurrent reads of memory nothing writes while they last (writing
// takes `&mut self`):
// - `get` reads one zone of the instance's state;
// - `controls`, `control`, `num_inputs`, `num_outputs`, `backend`,
//   `precision`, `factory` read this value, the control map (filled once, in
//   `create`) and the factory's `Arc`;
// - `sample_rate` reads the instance: `getSampleRateCInterpreterDSPInstance`
//   an int-heap slot, `getSampleRateCCraneliftDSPInstance` a field;
// - `metadata` walks the factory's metadata into a sink local to the call:
//   `metadataCInterpreterDSPInstance` its `meta_block`,
//   `metadataCCraneliftDSPInstance` its runtime descriptor.
// A new `&self` method must keep to reads, or this impl goes.
unsafe impl Sync for Dsp {}

impl Drop for Dsp {
    fn drop(&mut self) {
        self.raw.delete();
    }
}

impl Dsp {
    pub(crate) fn create(factory: Arc<FactoryInner>, sample_rate: i32) -> Result<Self, Error> {
        let raw = {
            factory.raw.instantiate().ok_or_else(|| {
                Error::new(
                    ErrorKind::Instantiate,
                    format!(
                        "the {} backend refused to instantiate `{}`",
                        factory.raw.backend(),
                        factory.name
                    ),
                )
            })?
        };
        raw.init(sample_rate);
        let inputs = usize::try_from(raw.num_inputs()).unwrap_or(0);
        let outputs = usize::try_from(raw.num_outputs()).unwrap_or(0);
        // the zones are cells of the instance's state, of the compiled precision
        let mut controls = ControlMap::new(factory.precision);
        let mut glue = controls.glue();
        // SAFETY: the glue borrows `controls`, which does not move during the call.
        unsafe { raw.build_user_interface(&mut glue) };
        // the builder's ranges went through the C ABI's `float`
        controls.apply_ranges(&raw.control_ranges());
        let dsp = Self {
            raw,
            factory,
            controls,
            inputs,
            outputs,
            scratch_f32: Vec::new(),
            scratch_f64: Vec::new(),
        };
        // The Cranelift backend compiles a program whose `compute` falls outside
        // its lowering subset to an empty stub and says so in the metadata only.
        if dsp.backend() == Backend::Cranelift
            && dsp
                .metadata()
                .iter()
                .any(|(k, v)| k == "cranelift-compute-body-lowered" && v == "false")
        {
            return Err(Error::new(
                ErrorKind::Instantiate,
                format!(
                    "the Cranelift backend did not lower the `compute` of `{}`: the instance would be silent",
                    dsp.factory.name
                ),
            ));
        }
        Ok(dsp)
    }

    /// Another handle on the program this instance runs.
    pub fn factory(&self) -> Factory {
        Factory {
            inner: Arc::clone(&self.factory),
        }
    }

    /// The engine the instance runs on.
    pub fn backend(&self) -> Backend {
        self.factory.raw.backend()
    }

    /// The type the instance computes with.
    pub fn precision(&self) -> Precision {
        self.factory.precision
    }

    /// The number of input channels `compute_*` expects.
    pub fn num_inputs(&self) -> usize {
        self.inputs
    }

    /// The number of output channels `compute_*` expects.
    pub fn num_outputs(&self) -> usize {
        self.outputs
    }

    /// The sample rate of the last initialisation.
    pub fn sample_rate(&self) -> i32 {
        self.raw.sample_rate()
    }

    /// Full initialisation at `sample_rate`: the class-level and the
    /// instance-level constants, the controls at their initial values, the
    /// state cleared (the `init` of the C++ `dsp` class).
    pub fn init(&mut self, sample_rate: i32) {
        self.raw.init(sample_rate);
    }

    /// The instance-level part of [`Dsp::init`].
    pub fn instance_init(&mut self, sample_rate: i32) {
        self.raw.instance_init(sample_rate);
    }

    /// Every control back to its initial value.
    pub fn reset_controls(&mut self) {
        self.raw.instance_reset_user_interface();
    }

    /// The state (delay lines, recursions) cleared; the controls are kept.
    pub fn clear(&mut self) {
        self.raw.instance_clear();
    }

    /// The controls, in the order of the UI tree: the order
    /// `buildUserInterface` declares them, where Faust sorts the widgets of a
    /// group by label (`[n]` prefixes included).
    pub fn controls(&self) -> impl Iterator<Item = &Control> {
        self.controls.iter()
    }

    /// The control at `path`, `None` when the program has none there.
    pub fn control(&self, path: &str) -> Option<&Control> {
        self.controls.get(path)
    }

    /// The current value of a control (for a bargraph, what the DSP last wrote).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::UnknownControl`] when the program has no control at `path`.
    pub fn get(&self, path: &str) -> Result<f64, Error> {
        self.controls
            .read(path)
            .ok_or_else(|| Error::new(ErrorKind::UnknownControl, path))
    }

    /// Sets a control, exactly as given: no clamping, see [`Control::clamp`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::UnknownControl`] when the program has no control at
    /// `path`, [`ErrorKind::ReadOnlyControl`] when it is a bargraph.
    pub fn set(&mut self, path: &str, value: f64) -> Result<(), Error> {
        match self.controls.write(path, value) {
            None => Err(Error::new(ErrorKind::UnknownControl, path)),
            Some(false) => Err(Error::new(ErrorKind::ReadOnlyControl, path)),
            Some(true) => Ok(()),
        }
    }

    /// The `declare` metadata of the program, plus the backend's own entries.
    pub fn metadata(&self) -> Vec<(String, String)> {
        let mut sink = MetadataSink(Vec::new());
        let mut glue = sink.glue();
        // SAFETY: the glue borrows `sink`, which does not move during the call.
        unsafe { self.raw.metadata(&mut glue) };
        sink.0
    }

    /// The width of the buffers the backend exchanges: the compiled
    /// precision, on both backends.
    fn exchanged(&self) -> Precision {
        self.factory.precision
    }

    fn check_buffers(
        &self,
        inputs: usize,
        outputs: usize,
        frames: Option<usize>,
    ) -> Result<usize, Error> {
        if inputs != self.inputs || outputs != self.outputs {
            return Err(Error::new(
                ErrorKind::Buffers,
                format!(
                    "{inputs} input and {outputs} output buffers for a DSP with {} inputs and {} outputs",
                    self.inputs, self.outputs
                ),
            ));
        }
        frames.ok_or_else(|| {
            Error::new(
                ErrorKind::Buffers,
                "a buffer is shorter than the frame count",
            )
        })
    }

    /// Runs the DSP over `f32` buffers: as many input and output channels as
    /// the arities, the frame count being the shortest buffer's length (the
    /// rest of a longer buffer is left as it is). On a `-double` program the
    /// samples are converted to and from `f64`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Buffers`] when the number of input or output channels
    /// differs from [`Dsp::num_inputs`] or [`Dsp::num_outputs`].
    pub fn compute_f32(
        &mut self,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
    ) -> Result<(), Error> {
        let frames = shortest(
            inputs.iter().map(|c| c.len()),
            outputs.iter().map(|c| c.len()),
        );
        let frames = self.check_buffers(inputs.len(), outputs.len(), frames)?;
        match self.exchanged() {
            Precision::F32 => self.run_native(inputs, outputs, frames),
            Precision::F64 => {
                self.scratch_f64
                    .resize_with(inputs.len() + outputs.len(), Vec::new);
                let (ins, outs) = self.scratch_f64.split_at_mut(inputs.len());
                for (buf, ch) in ins.iter_mut().zip(inputs) {
                    buf.clear();
                    buf.extend(ch[..frames].iter().map(|&x| f64::from(x)));
                }
                for buf in outs.iter_mut() {
                    buf.clear();
                    buf.resize(frames, 0.0);
                }
                run_channels(
                    self.raw,
                    ins.iter().map(|b| b.as_ptr().cast_mut()),
                    outs.iter_mut().map(|b| b.as_mut_ptr()),
                    frames,
                );
                for (ch, buf) in outputs.iter_mut().zip(outs.iter()) {
                    for (dst, &src) in ch[..frames].iter_mut().zip(buf) {
                        *dst = src as f32;
                    }
                }
                Ok(())
            }
        }
    }

    /// Runs the DSP over `f64` buffers; see [`Dsp::compute_f32`]. Exact on a
    /// `-double` program, on both backends; on a single-precision one the
    /// samples are converted to and from `f32`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Buffers`] when the number of input or output channels
    /// differs from [`Dsp::num_inputs`] or [`Dsp::num_outputs`].
    pub fn compute_f64(
        &mut self,
        inputs: &[&[f64]],
        outputs: &mut [&mut [f64]],
    ) -> Result<(), Error> {
        let frames = shortest(
            inputs.iter().map(|c| c.len()),
            outputs.iter().map(|c| c.len()),
        );
        let frames = self.check_buffers(inputs.len(), outputs.len(), frames)?;
        match self.exchanged() {
            Precision::F64 => self.run_native(inputs, outputs, frames),
            Precision::F32 => {
                self.scratch_f32
                    .resize_with(inputs.len() + outputs.len(), Vec::new);
                let (ins, outs) = self.scratch_f32.split_at_mut(inputs.len());
                for (buf, ch) in ins.iter_mut().zip(inputs) {
                    buf.clear();
                    buf.extend(ch[..frames].iter().map(|&x| x as f32));
                }
                for buf in outs.iter_mut() {
                    buf.clear();
                    buf.resize(frames, 0.0);
                }
                run_channels(
                    self.raw,
                    ins.iter().map(|b| b.as_ptr().cast_mut()),
                    outs.iter_mut().map(|b| b.as_mut_ptr()),
                    frames,
                );
                for (ch, buf) in outputs.iter_mut().zip(outs.iter()) {
                    for (dst, &src) in ch[..frames].iter_mut().zip(buf) {
                        *dst = f64::from(src);
                    }
                }
                Ok(())
            }
        }
    }

    fn run_native<T: Sample>(
        &mut self,
        inputs: &[&[T]],
        outputs: &mut [&mut [T]],
        frames: usize,
    ) -> Result<(), Error> {
        run_channels(
            self.raw,
            inputs.iter().map(|c| c.as_ptr().cast_mut()),
            outputs.iter_mut().map(|c| c.as_mut_ptr()),
            frames,
        );
        Ok(())
    }
}

/// Channel counts up to which [`run_channels`] gathers the channel pointers
/// on the stack; beyond, it allocates the lists.
const STACK_CHANNELS: usize = 64;

/// The C call over channels of the exchanged width: their pointers gathered
/// in arrays, on the stack for at most [`STACK_CHANNELS`] inputs and outputs,
/// so that a compute call allocates nothing. Inputs are only read by the
/// backends.
fn run_channels<T: Sample>(
    raw: RawInstance,
    inputs: impl ExactSizeIterator<Item = *mut T>,
    outputs: impl ExactSizeIterator<Item = *mut T>,
    frames: usize,
) {
    let count = i32::try_from(frames).unwrap_or(i32::MAX);
    // SAFETY: every channel holds at least `frames` elements of the width
    // the backend exchanges (checked by the callers); the backends do not
    // write to the input channels; the pointer arrays outlive the call.
    let run = |ins: &mut [*mut T], outs: &mut [*mut T]| unsafe { raw.compute(count, ins, outs) };
    let (n_in, n_out) = (inputs.len(), outputs.len());
    if n_in <= STACK_CHANNELS && n_out <= STACK_CHANNELS {
        let mut ins = [std::ptr::null_mut(); STACK_CHANNELS];
        let mut outs = [std::ptr::null_mut(); STACK_CHANNELS];
        for (slot, p) in ins.iter_mut().zip(inputs) {
            *slot = p;
        }
        for (slot, p) in outs.iter_mut().zip(outputs) {
            *slot = p;
        }
        run(&mut ins[..n_in], &mut outs[..n_out]);
    } else {
        run(
            &mut inputs.collect::<Vec<_>>(),
            &mut outputs.collect::<Vec<_>>(),
        );
    }
}

impl std::fmt::Debug for Dsp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dsp")
            .field("factory", &self.factory.name)
            .field("backend", &self.backend())
            .field("inputs", &self.inputs)
            .field("outputs", &self.outputs)
            .field("sample_rate", &self.sample_rate())
            .finish()
    }
}

/// The shortest of all channel lengths; `None` without any channel would
/// make a frame count meaningless, so a DSP with no inputs nor outputs gets 0.
fn shortest(
    inputs: impl Iterator<Item = usize>,
    outputs: impl Iterator<Item = usize>,
) -> Option<usize> {
    let mut all = inputs.chain(outputs).peekable();
    if all.peek().is_none() {
        return Some(0);
    }
    all.min()
}
