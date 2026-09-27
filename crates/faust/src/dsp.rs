//! An instance of a compiled program.

use std::sync::Arc;

use crate::Sample;
use crate::backend::{RawInstance, Width};
use crate::controls::{Control, ControlMap, MetadataSink};
use crate::factory::{Factory, FactoryInner};
use crate::{Backend, Error, ErrorKind, Precision};

/// An instance: its state, its sample rate, its controls. Owns a reference
/// to its factory, so it can outlive the host's [`Factory`] handles. `Send`
/// and `Sync`: it can be moved to another thread, and shared, since every
/// `&self` method only reads; `compute`, `set_param_value` and the initialisations
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
// - `get_param_value` reads one zone of the instance's state;
// - `controls`, `control`, `get_num_inputs`, `get_num_outputs`, `backend`,
//   `precision`, `factory` read this value, the control map (filled once, in
//   `create`) and the factory's `Arc`;
// - `get_sample_rate` reads the instance: `getSampleRateCInterpreterDSPInstance`
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
        controls.finish();
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

    /// The number of audio inputs of the instance: the input buffers
    /// [`Dsp::compute`] takes (`getNumInputs`).
    pub fn get_num_inputs(&self) -> usize {
        self.inputs
    }

    /// The number of audio outputs of the instance: the output buffers
    /// [`Dsp::compute`] takes (`getNumOutputs`).
    pub fn get_num_outputs(&self) -> usize {
        self.outputs
    }

    /// The sample rate currently used by the instance, in Hz: the one of
    /// the last initialisation (`getSampleRate`).
    pub fn get_sample_rate(&self) -> i32 {
        self.raw.sample_rate()
    }

    /// Global init at `sample_rate`, in Hz: the static tables of the program
    /// (`classInit`), then [`Dsp::instance_init`] (`init`).
    pub fn init(&mut self, sample_rate: i32) {
        self.raw.init(sample_rate);
    }

    /// Init instance state at `sample_rate`, in Hz: [`Dsp::instance_constants`],
    /// [`Dsp::instance_reset_user_interface`], then [`Dsp::instance_clear`]
    /// (`instanceInit`).
    pub fn instance_init(&mut self, sample_rate: i32) {
        self.raw.instance_init(sample_rate);
    }

    /// Init instance constant state at `sample_rate`, in Hz: the constants
    /// that depend on it; the control parameter values and the state are
    /// kept (`instanceConstants`).
    pub fn instance_constants(&mut self, sample_rate: i32) {
        self.raw.instance_constants(sample_rate);
    }

    /// Init default control parameter values: every control back to the
    /// initial value the program declares, [`Control::init`]; the state is
    /// kept (`instanceResetUserInterface`).
    pub fn instance_reset_user_interface(&mut self) {
        self.raw.instance_reset_user_interface();
    }

    /// Init instance state (like delay lines, recursions...) but keep the
    /// control parameter values (`instanceClear`).
    pub fn instance_clear(&mut self) {
        self.raw.instance_clear();
    }

    /// The controls, in the order of the UI tree: the order
    /// `buildUserInterface` declares them, where Faust sorts the widgets of a
    /// group by label (`[n]` prefixes included).
    pub fn controls(&self) -> impl Iterator<Item = &Control> {
        self.controls.iter()
    }

    /// The control `name` designates, `None` when it designates none; `name`
    /// is looked up as by [`Dsp::set_param_value`].
    pub fn control(&self, name: &str) -> Option<&Control> {
        self.controls.get(name)
    }

    /// The current value of a control (for a bargraph, what the DSP last
    /// wrote), `name` being looked up as by [`Dsp::set_param_value`]
    /// (`MapUI::getParamValue`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::UnknownControl`] when `name` designates no control.
    pub fn get_param_value(&self, name: &str) -> Result<f64, Error> {
        self.controls
            .read(name)
            .ok_or_else(|| Error::new(ErrorKind::UnknownControl, name))
    }

    /// Sets a control, exactly as given: no clamping, see [`Control::clamp`]
    /// (`MapUI::setParamValue`).
    ///
    /// `name` is looked up as the C++ `MapUI` does: as a [`Control::path`]
    /// (`/synth/osc0/freq`), then as a [`Control::shortname`] (`osc0_freq`),
    /// then as a [`Control::label`] (`freq`). A label several controls share
    /// designates the last one declared, as in `MapUI`; a path or a
    /// shortname designates one control only.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::UnknownControl`] when `name` designates no control,
    /// [`ErrorKind::ReadOnlyControl`] when it is a bargraph (which `MapUI`
    /// would write, and the DSP overwrite at its next block).
    pub fn set_param_value(&mut self, name: &str, value: f64) -> Result<(), Error> {
        match self.controls.write(name, value) {
            None => Err(Error::new(ErrorKind::UnknownControl, name)),
            Some(false) => Err(Error::new(ErrorKind::ReadOnlyControl, name)),
            Some(true) => Ok(()),
        }
    }

    /// The `declare` (key, value) metadata of the instance, plus the
    /// backend's own entries (`metadata`, which calls a `Meta` where this
    /// returns the pairs).
    pub fn metadata(&self) -> Vec<(String, String)> {
        let mut sink = MetadataSink(Vec::new());
        let mut glue = sink.glue();
        // SAFETY: the glue borrows `sink`, which does not move during the call.
        unsafe { self.raw.metadata(&mut glue) };
        sink.0
    }

    /// DSP instance computation, to be called with successive input and
    /// output audio buffers (`compute`): `count` frames, one non-interleaved
    /// buffer per input and per output, each holding at least `count`
    /// samples; the rest of a longer buffer is left as it is. Inputs and
    /// outputs are distinct buffers, as the borrows require.
    ///
    /// `T` is `f32` or `f64`, the host's `FAUSTFLOAT`. When it is not the
    /// compiled precision, the samples are converted on the way in and out:
    /// a `-double` program exchanges exact `f64` samples, on both backends.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Buffers`] when the number of input or output buffers
    /// differs from [`Dsp::get_num_inputs`] or [`Dsp::get_num_outputs`], or
    /// when a buffer holds fewer than `count` samples.
    pub fn compute<T: Sample>(
        &mut self,
        count: usize,
        inputs: &[&[T]],
        outputs: &mut [&mut [T]],
    ) -> Result<(), Error> {
        self.check_buffers(count, inputs, outputs)?;
        let raw = self.raw;
        match (T::PRECISION, self.factory.precision) {
            (Precision::F32, Precision::F32) | (Precision::F64, Precision::F64) => {
                run_channels(
                    raw,
                    inputs.iter().map(|c| c.as_ptr().cast_mut()),
                    outputs.iter_mut().map(|c| c.as_mut_ptr()),
                    count,
                );
            }
            (_, Precision::F32) => {
                run_converted(raw, &mut self.scratch_f32, count, inputs, outputs);
            }
            (_, Precision::F64) => {
                run_converted(raw, &mut self.scratch_f64, count, inputs, outputs);
            }
        }
        Ok(())
    }

    fn check_buffers<T>(
        &self,
        count: usize,
        inputs: &[&[T]],
        outputs: &[&mut [T]],
    ) -> Result<(), Error> {
        if inputs.len() != self.inputs || outputs.len() != self.outputs {
            return Err(Error::new(
                ErrorKind::Buffers,
                format!(
                    "{} input and {} output buffers for a DSP with {} inputs and {} outputs",
                    inputs.len(),
                    outputs.len(),
                    self.inputs,
                    self.outputs
                ),
            ));
        }
        let shortest = inputs
            .iter()
            .map(|c| c.len())
            .chain(outputs.iter().map(|c| c.len()))
            .min();
        match shortest {
            Some(frames) if frames < count => Err(Error::new(
                ErrorKind::Buffers,
                format!("a buffer holds {frames} samples, fewer than the {count} frames asked"),
            )),
            _ => Ok(()),
        }
    }
}

/// [`Dsp::compute`] over host buffers of width `T` for a program compiled at
/// width `U`: the inputs converted into `scratch`, the call, the outputs
/// converted back. `scratch` keeps its capacity from one call to the next,
/// so a steady block size allocates nothing.
fn run_converted<T: Sample, U: Width>(
    raw: RawInstance,
    scratch: &mut Vec<Vec<U>>,
    count: usize,
    inputs: &[&[T]],
    outputs: &mut [&mut [T]],
) {
    scratch.resize_with(inputs.len() + outputs.len(), Vec::new);
    let (ins, outs) = scratch.split_at_mut(inputs.len());
    for (buf, ch) in ins.iter_mut().zip(inputs) {
        buf.clear();
        buf.extend(ch[..count].iter().map(|&x| U::from_f64(x.to_f64())));
    }
    for buf in outs.iter_mut() {
        buf.clear();
        buf.resize(count, U::default());
    }
    run_channels(
        raw,
        ins.iter().map(|b| b.as_ptr().cast_mut()),
        outs.iter_mut().map(|b| b.as_mut_ptr()),
        count,
    );
    for (ch, buf) in outputs.iter_mut().zip(outs.iter()) {
        for (dst, &src) in ch[..count].iter_mut().zip(buf) {
            *dst = T::from_f64(src.to_f64());
        }
    }
}

/// Channel counts up to which [`run_channels`] gathers the channel pointers
/// on the stack; beyond, it allocates the lists.
const STACK_CHANNELS: usize = 64;

/// The C call over channels of the exchanged width: their pointers gathered
/// in arrays, on the stack for at most [`STACK_CHANNELS`] inputs and outputs,
/// so that a compute call allocates nothing. Inputs are only read by the
/// backends.
fn run_channels<T: Width>(
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
            .field("sample_rate", &self.get_sample_rate())
            .finish()
    }
}
