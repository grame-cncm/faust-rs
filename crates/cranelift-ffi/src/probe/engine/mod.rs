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
//!
//! # Layout
//! [`Factory`] (`factory.rs`) compiles, [`Probe`] (`probe.rs`) is one
//! instance and its render loop, [`PolyProbe`] (`poly_probe.rs`) is N of them
//! and an effect. What the two render loops share is here.

mod factory;
mod poly_probe;
mod probe;

pub use factory::{CompileFailure, Factory, last_compile_failure};
pub use poly_probe::{PolyProbe, PolyRenderSpec, PolyTarget, PolyWrite};
pub use probe::Probe;

use crate::probe::render::{InputMode, StatsAccumulator};
use crate::probe::schedule::Schedule;

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
    /// Magnitude the window must stay under; the first sample above it is
    /// located in the statistics (`--fail-above`).
    pub limit: crate::probe::render::RenderLimit,
    /// Time every `compute` call (`--time`); the statistics then carry a
    /// [`crate::probe::timing::Timing`].
    pub time: bool,
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
            limit: None,
            time: false,
        }
    }
}

/// One rendered frame, as `f64` regardless of the compiled sample width.
pub type Frame = Vec<f64>;

/// The length of the block that starts at `written`: `block` frames, fewer at
/// the end of the render, and fewer again so that the next scheduled event
/// lands on a block boundary, which is what makes its timing sample-exact
/// rather than rounded to the block grid.
fn next_block(schedule: &Schedule, block: usize, frames: usize, written: usize) -> usize {
    let mut n = block.min(frames - written);
    if let Some(next) = schedule.next_after(written)
        && next > written
    {
        n = n.min(next - written);
    }
    n
}

/// Hands the `n` frames of a computed block, which starts at `written`, to
/// the statistics and, from the skip point on, to the caller. `sample` reads
/// output `ch` at frame `j` of the block.
fn push_block(
    acc: &mut StatsAccumulator,
    frame: &mut Frame,
    (written, n, skip): (usize, usize, usize),
    sample: impl Fn(usize, usize) -> f64,
    on_frame: &mut impl FnMut(usize, &[f64]),
) {
    for j in 0..n {
        for (ch, value) in frame.iter_mut().enumerate() {
            *value = sample(ch, j);
        }
        acc.push(written + j, frame);
        if written + j >= skip {
            on_frame(written + j, frame);
        }
    }
}
