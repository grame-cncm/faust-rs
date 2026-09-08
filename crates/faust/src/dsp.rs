//! An instance of a compiled program.

use std::sync::Arc;

use crate::backend::RawInstance;
use crate::controls::{Control, ControlMap, MetadataSink};
use crate::factory::{Factory, FactoryInner};
use crate::{Backend, Error, ErrorKind, Precision};

/// An instance: its state, its sample rate, its controls. Owns a reference
/// to its factory, so it can outlive the host's [`Factory`] handles; can be
/// moved and sent to another thread, not shared between threads.
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

    pub fn backend(&self) -> Backend {
        self.factory.raw.backend()
    }

    pub fn precision(&self) -> Precision {
        self.factory.precision
    }

    pub fn num_inputs(&self) -> usize {
        self.inputs
    }

    pub fn num_outputs(&self) -> usize {
        self.outputs
    }

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

    /// The controls, in path order.
    pub fn controls(&self) -> impl Iterator<Item = &Control> {
        self.controls.iter()
    }

    pub fn control(&self, path: &str) -> Option<&Control> {
        self.controls.get(path)
    }

    /// The current value of a control (for a bargraph, what the DSP last wrote).
    pub fn get(&self, path: &str) -> Result<f64, Error> {
        self.controls
            .read(path)
            .ok_or_else(|| Error::new(ErrorKind::UnknownControl, path))
    }

    /// Sets a control, exactly as given: no clamping, see [`Control::clamp`].
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

    /// The width of the buffers the backend exchanges.
    fn exchanged(&self) -> Precision {
        match self.backend() {
            Backend::Interp => Precision::F32,
            Backend::Cranelift => self.factory.precision,
        }
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
    /// the arities, the frame count being the shortest buffer's length.
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
                let in_refs: Vec<&[f64]> = ins.iter().map(|b| b.as_slice()).collect();
                let mut out_refs: Vec<&mut [f64]> =
                    outs.iter_mut().map(|b| b.as_mut_slice()).collect();
                Self::run_raw(self.raw, &in_refs, &mut out_refs, frames);
                for (ch, buf) in outputs.iter_mut().zip(outs.iter()) {
                    for (dst, &src) in ch[..frames].iter_mut().zip(buf) {
                        *dst = src as f32;
                    }
                }
                Ok(())
            }
        }
    }

    /// Runs the DSP over `f64` buffers; see [`Dsp::compute_f32`].
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
                let in_refs: Vec<&[f32]> = ins.iter().map(|b| b.as_slice()).collect();
                let mut out_refs: Vec<&mut [f32]> =
                    outs.iter_mut().map(|b| b.as_mut_slice()).collect();
                Self::run_raw(self.raw, &in_refs, &mut out_refs, frames);
                for (ch, buf) in outputs.iter_mut().zip(outs.iter()) {
                    for (dst, &src) in ch[..frames].iter_mut().zip(buf) {
                        *dst = f64::from(src);
                    }
                }
                Ok(())
            }
        }
    }

    fn run_native<T>(
        &mut self,
        inputs: &[&[T]],
        outputs: &mut [&mut [T]],
        frames: usize,
    ) -> Result<(), Error> {
        Self::run_raw(self.raw, inputs, outputs, frames);
        Ok(())
    }

    /// The C call: channel pointer arrays of the exchanged width, cast to the
    /// ABI's `float*`. Inputs are only read by the backends.
    fn run_raw<T>(raw: RawInstance, inputs: &[&[T]], outputs: &mut [&mut [T]], frames: usize) {
        let mut in_ptrs: Vec<*mut f32> = inputs
            .iter()
            .map(|c| c.as_ptr().cast_mut().cast::<f32>())
            .collect();
        let mut out_ptrs: Vec<*mut f32> = outputs
            .iter_mut()
            .map(|c| c.as_mut_ptr().cast::<f32>())
            .collect();
        let count = i32::try_from(frames).unwrap_or(i32::MAX);
        // SAFETY: every channel holds at least `frames` elements of the width
        // the backend exchanges (checked by the callers); the backends do not
        // write to the input channels; the pointer arrays outlive the call.
        unsafe { raw.compute(count, in_ptrs.as_mut_ptr(), out_ptrs.as_mut_ptr()) };
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
