//! Opaque FFI types owned by the factory cache.
//!
//! `InterpreterDspFactory` and `InterpreterDspInstance` are heap-allocated
//! Rust objects exposed to C as opaque pointer types.  Ownership rules mirror
//! the original Faust C API:
//! - Factory creation and SHA lookup acquire cache references.
//! - `deleteCInterpreterDSPFactory` releases one reference; final release
//!   drops the factory and all remaining instances.
//! - `deleteCInterpreterDSPInstance` may delete an instance earlier.
//!
//! # Float/Double runtime dispatch
//!
//! The reference Faust C++ library resolves float/double at compile time.
//! This Rust port handles it at runtime via the `FbcDspFactoryAny` and
//! `FbcExecutorAny` enums so that a single shared library supports both modes
//! without requiring two separate compilations.
//!
//! Audio I/O (`FAUSTFLOAT*` buffers) always use `f32` at the C ABI boundary.
//! In double mode, samples are converted `f32 → f64` on input and
//! `f64 → f32` on output inside `computeCInterpreterDSPInstance`, through
//! buffers the instance keeps ([`IoScratch`]). Rust callers can exchange `f64`
//! instead with [`crate::instance::compute_f64`], which no C symbol exports.
//!
//! UI zones (sliders, buttons …) live in the instance's `real_heap`.
//! In double mode, the `real_heap` elements are `f64`; the raw pointer passed
//! to `UIGlue` callbacks is a `*mut f64` reinterpreted as `*mut f32` — the
//! application must be compiled with `FAUSTFLOAT=double` to read them
//! correctly, matching the upstream C++ contract.

use std::ffi::{c_char, c_void};

use codegen::backends::interp::{
    BlockId, FbcDspFactory, FbcExecutor, FbcMetaInstruction, Soundfile,
};

/// `FAUSTFLOAT` type at the C ABI boundary (always `f32`).
pub type FaustFloat = f32;

/// Shared UI callback table (`UIGlue`) for Faust C FFI.
pub use ffi_common::UIGlue;

/// Shared metadata callback table (`MetaGlue`) for Faust C FFI.
pub use ffi_common::MetaGlue;

// ── Runtime-polymorphic factory ──────────────────────────────────────────────

/// Runtime-polymorphic wrapper around `FbcDspFactory<f32>` or `FbcDspFactory<f64>`.
///
/// Allows a single shared library to support both `float` and `double` internal
/// DSP arithmetic, selected at factory-creation time from the `.fbc` header or
/// the `-double` flag.
pub enum FbcDspFactoryAny {
    Float32(FbcDspFactory<f32>),
    Float64(FbcDspFactory<f64>),
}

impl FbcDspFactoryAny {
    // ── Scalar metadata accessors (shared between f32/f64) ──────────────

    pub fn num_inputs(&self) -> i32 {
        match self {
            Self::Float32(f) => f.num_inputs,
            Self::Float64(f) => f.num_inputs,
        }
    }

    pub fn num_outputs(&self) -> i32 {
        match self {
            Self::Float32(f) => f.num_outputs,
            Self::Float64(f) => f.num_outputs,
        }
    }

    pub fn int_heap_size(&self) -> i32 {
        match self {
            Self::Float32(f) => f.int_heap_size,
            Self::Float64(f) => f.int_heap_size,
        }
    }

    pub fn real_heap_size(&self) -> i32 {
        match self {
            Self::Float32(f) => f.real_heap_size,
            Self::Float64(f) => f.real_heap_size,
        }
    }

    pub fn sr_offset(&self) -> i32 {
        match self {
            Self::Float32(f) => f.sr_offset,
            Self::Float64(f) => f.sr_offset,
        }
    }

    pub fn count_offset(&self) -> i32 {
        match self {
            Self::Float32(f) => f.count_offset,
            Self::Float64(f) => f.count_offset,
        }
    }

    pub fn static_init_block(&self) -> BlockId {
        match self {
            Self::Float32(f) => f.static_init_block,
            Self::Float64(f) => f.static_init_block,
        }
    }

    pub fn init_block(&self) -> BlockId {
        match self {
            Self::Float32(f) => f.init_block,
            Self::Float64(f) => f.init_block,
        }
    }

    pub fn reset_ui_block(&self) -> BlockId {
        match self {
            Self::Float32(f) => f.reset_ui_block,
            Self::Float64(f) => f.reset_ui_block,
        }
    }

    pub fn clear_block(&self) -> BlockId {
        match self {
            Self::Float32(f) => f.clear_block,
            Self::Float64(f) => f.clear_block,
        }
    }

    pub fn compute_block(&self) -> BlockId {
        match self {
            Self::Float32(f) => f.compute_block,
            Self::Float64(f) => f.compute_block,
        }
    }

    pub fn compute_dsp_block(&self) -> BlockId {
        match self {
            Self::Float32(f) => f.compute_dsp_block,
            Self::Float64(f) => f.compute_dsp_block,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Float32(f) => &f.name,
            Self::Float64(f) => &f.name,
        }
    }

    pub fn sha_key(&self) -> &str {
        match self {
            Self::Float32(f) => &f.sha_key,
            Self::Float64(f) => &f.sha_key,
        }
    }

    /// Sets the cache identity of this factory.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_factory::setSHAKey` (`interpreter_dsp_aux.hh`), called
    ///   by every creation path in `interpreter_dynamic_dsp_aux.cpp` and
    ///   `interpreter_dsp_aux.cpp` right after the factory is registered.
    pub fn set_sha_key(&mut self, sha_key: impl Into<String>) {
        let sha_key = sha_key.into();
        match self {
            Self::Float32(f) => f.sha_key = sha_key,
            Self::Float64(f) => f.sha_key = sha_key,
        }
    }

    pub fn compile_options(&self) -> &str {
        match self {
            Self::Float32(f) => &f.compile_options,
            Self::Float64(f) => &f.compile_options,
        }
    }

    /// Non-generic: `FbcMetaInstruction` contains only `String` fields.
    pub fn meta_block(&self) -> &[FbcMetaInstruction] {
        match self {
            Self::Float32(f) => &f.meta_block,
            Self::Float64(f) => &f.meta_block,
        }
    }

    /// Returns `true` when this factory uses double-precision arithmetic.
    pub fn is_double(&self) -> bool {
        matches!(self, Self::Float64(_))
    }

    /// Returns the number of soundfile slots required by this factory.
    pub fn soundfile_count(&self) -> usize {
        match self {
            Self::Float32(f) => f.soundfile_count(),
            Self::Float64(f) => f.soundfile_count(),
        }
    }

    /// Trigger one-shot bytecode optimization (idempotent).
    pub fn optimize(&mut self) {
        match self {
            Self::Float32(f) => f.optimize(),
            Self::Float64(f) => f.optimize(),
        }
    }

    // ── Executor operations (type-safe paired dispatch) ───────────────────

    /// Execute a bytecode block using the paired executor.
    ///
    /// The factory and executor must be the same precision variant; mismatches
    /// are silently ignored (should never occur in correct usage).
    pub fn execute_block_on(&self, exec: &mut FbcExecutorAny, block_id: BlockId) {
        match (self, exec) {
            (Self::Float32(f), FbcExecutorAny::Float32(e)) => {
                e.execute_block(&f.arena, block_id);
            }
            (Self::Float64(f), FbcExecutorAny::Float64(e)) => {
                e.execute_block(&f.arena, block_id);
            }
            _ => {} // precision mismatch — bug in calling code
        }
    }

    /// Execute the DSP block with `f32` audio I/O, the width of the C ABI.
    ///
    /// A `Float64` program computes in `f64`: its inputs are widened and its
    /// outputs narrowed through the instance's `scratch` buffers, which are
    /// reused from one block to the next.
    ///
    /// # Safety
    /// The caller must ensure `inputs`/`outputs` are valid for the duration
    /// of this call (same contract as `FbcExecutor::execute_block_io`).
    pub fn execute_block_io_f32(
        &self,
        exec: &mut FbcExecutorAny,
        scratch: &mut IoScratch,
        block_id: BlockId,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
    ) {
        match (self, exec) {
            (Self::Float32(f), FbcExecutorAny::Float32(e)) => {
                e.execute_block_io(&f.arena, block_id, inputs, outputs);
            }
            (Self::Float64(f), FbcExecutorAny::Float64(e)) => run_converted(
                &mut scratch.f64,
                inputs,
                outputs,
                f64::from,
                |x| x as f32,
                |ins, outs| e.execute_block_io(&f.arena, block_id, ins, outs),
            ),
            _ => {
                // Precision mismatch between factory and executor — indicates a
                // bug in the calling code. Asserts in debug, silent in release
                // (no audio produced for this call).
                debug_assert!(
                    false,
                    "execute_block_io_f32: factory/executor precision mismatch"
                );
            }
        }
    }

    /// Execute the DSP block with `f64` audio I/O: native for a `Float64`
    /// program, converted through `scratch` for a `Float32` one (inputs
    /// rounded to the nearest `f32`, outputs widened exactly).
    ///
    /// No C entry point reaches it: the C ABI exchanges `f32`. It serves the
    /// Rust callers of [`crate::instance::compute_f64`].
    pub fn execute_block_io_f64(
        &self,
        exec: &mut FbcExecutorAny,
        scratch: &mut IoScratch,
        block_id: BlockId,
        inputs: &[&[f64]],
        outputs: &mut [&mut [f64]],
    ) {
        match (self, exec) {
            (Self::Float64(f), FbcExecutorAny::Float64(e)) => {
                e.execute_block_io(&f.arena, block_id, inputs, outputs);
            }
            (Self::Float32(f), FbcExecutorAny::Float32(e)) => run_converted(
                &mut scratch.f32,
                inputs,
                outputs,
                |x| x as f32,
                f64::from,
                |ins, outs| e.execute_block_io(&f.arena, block_id, ins, outs),
            ),
            _ => {
                debug_assert!(
                    false,
                    "execute_block_io_f64: factory/executor precision mismatch"
                );
            }
        }
    }

    /// Dispatch UI instructions to `UIGlue` callbacks.
    ///
    /// In double mode, scalar parameters (`init`, `min`, `max`, `step`) are
    /// narrowed `f64 → f32` for the UIGlue callbacks.  Zone pointers point
    /// into the `f64` real_heap; applications compiled with `FAUSTFLOAT=double`
    /// will interpret them correctly.
    ///
    /// # Safety
    /// `glue` must be non-null and point to a valid `UIGlue`.
    pub unsafe fn dispatch_ui_glue(
        &self,
        exec: &mut FbcExecutorAny,
        soundfile_zones: &mut [*mut c_void],
        glue: *mut UIGlue,
    ) {
        unsafe {
            match (self, exec) {
                (Self::Float32(f), FbcExecutorAny::Float32(e)) => {
                    crate::ui::dispatch_ui_f32(
                        &f.ui_block,
                        &mut e.real_heap,
                        soundfile_zones,
                        glue,
                    );
                }
                (Self::Float64(f), FbcExecutorAny::Float64(e)) => {
                    crate::ui::dispatch_ui_f64(
                        &f.ui_block,
                        &mut e.real_heap,
                        soundfile_zones,
                        glue,
                    );
                }
                _ => {}
            }
        }
    }
}

// ── Runtime-polymorphic executor ─────────────────────────────────────────────

/// Runtime-polymorphic wrapper around `FbcExecutor<f32>` or `FbcExecutor<f64>`.
pub enum FbcExecutorAny {
    Float32(FbcExecutor<f32>),
    Float64(FbcExecutor<f64>),
}

impl FbcExecutorAny {
    /// Allocate a new executor matching the precision of `factory`.
    ///
    /// Soundfile slots are pre-populated with default silence so that
    /// `LoadSoundFieldInt`/`LoadSoundFieldReal` opcodes never index out-of-bounds
    /// before real audio data is provided by the host.
    pub fn new_for_factory(factory: &FbcDspFactoryAny) -> Self {
        let sf_count = factory.soundfile_count();
        match factory {
            FbcDspFactoryAny::Float32(f) => {
                let mut e = FbcExecutor::new(f.int_heap_size as usize, f.real_heap_size as usize);
                e.soundfiles = (0..sf_count)
                    .map(|_| Box::new(Soundfile::default_silence()))
                    .collect();
                Self::Float32(e)
            }
            FbcDspFactoryAny::Float64(f) => {
                let mut e = FbcExecutor::new(f.int_heap_size as usize, f.real_heap_size as usize);
                e.soundfiles = (0..sf_count)
                    .map(|_| Box::new(Soundfile::default_silence()))
                    .collect();
                Self::Float64(e)
            }
        }
    }

    /// Shared integer heap (present in both variants).
    pub fn int_heap(&self) -> &[i32] {
        match self {
            Self::Float32(e) => &e.int_heap,
            Self::Float64(e) => &e.int_heap,
        }
    }

    /// Mutable shared integer heap.
    pub fn int_heap_mut(&mut self) -> &mut Vec<i32> {
        match self {
            Self::Float32(e) => &mut e.int_heap,
            Self::Float64(e) => &mut e.int_heap,
        }
    }

    /// Replace the `Soundfile` at `slot` with new data.
    ///
    /// Used after `buildUserInterface` to swap in real audio loaded by the host.
    pub fn set_soundfile(&mut self, slot: usize, sf: Soundfile) {
        let entry = match self {
            Self::Float32(e) => e.soundfiles.get_mut(slot),
            Self::Float64(e) => e.soundfiles.get_mut(slot),
        };
        if let Some(entry) = entry {
            **entry = sf;
        }
    }

    /// Copy heap state from another executor of the same precision.
    ///
    /// Silently ignores precision mismatches (should not occur in correct usage).
    pub fn copy_from(&mut self, src: &FbcExecutorAny) {
        match (self, src) {
            (Self::Float32(dst), Self::Float32(src)) => {
                dst.int_heap.copy_from_slice(&src.int_heap);
                dst.real_heap.copy_from_slice(&src.real_heap);
            }
            (Self::Float64(dst), Self::Float64(src)) => {
                dst.int_heap.copy_from_slice(&src.int_heap);
                dst.real_heap.copy_from_slice(&src.real_heap);
            }
            _ => {}
        }
    }
}

// ── Audio I/O conversion ─────────────────────────────────────────────────────

/// Conversion buffers of one instance, for audio I/O whose width is not the
/// width its program computes with: one buffer per channel, kept from one
/// block to the next, so a compute call allocates sample memory only when the
/// block size or the channel count grows.
#[derive(Debug, Default)]
pub struct IoScratch {
    f32: Vec<Vec<f32>>,
    f64: Vec<Vec<f64>>,
}

/// Runs `run` on `inputs` and `outputs` converted to the width `W` through
/// `scratch`: inputs converted with `widen`, outputs zeroed, then converted
/// back with `narrow` into the caller's buffers. Each converted channel has
/// the length of the channel it mirrors.
fn run_converted<H: Copy, W: Copy + Default>(
    scratch: &mut Vec<Vec<W>>,
    inputs: &[&[H]],
    outputs: &mut [&mut [H]],
    widen: impl Fn(H) -> W,
    narrow: impl Fn(W) -> H,
    run: impl FnOnce(&[&[W]], &mut [&mut [W]]),
) {
    let channels = inputs.len() + outputs.len();
    if scratch.len() < channels {
        scratch.resize_with(channels, Vec::new);
    }
    let (ins, outs) = scratch[..channels].split_at_mut(inputs.len());
    for (buf, channel) in ins.iter_mut().zip(inputs) {
        buf.clear();
        buf.extend(channel.iter().map(|&x| widen(x)));
    }
    for (buf, channel) in outs.iter_mut().zip(outputs.iter()) {
        buf.clear();
        buf.resize(channel.len(), W::default());
    }
    let in_refs: Vec<&[W]> = ins.iter().map(Vec::as_slice).collect();
    let mut out_refs: Vec<&mut [W]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
    run(&in_refs, &mut out_refs);
    for (channel, buf) in outputs.iter_mut().zip(outs.iter()) {
        for (dst, &src) in channel.iter_mut().zip(buf) {
            *dst = narrow(src);
        }
    }
}

// ── Opaque wrapper types ──────────────────────────────────────────────────────

/// Opaque DSP factory, exported as `interpreter_dsp_factory*` in C.
///
/// Owns an `FbcDspFactoryAny` (either `f32` or `f64`) inside the global
/// reference-counted factory cache.
pub struct InterpreterDspFactory {
    pub(crate) inner: FbcDspFactoryAny,
}

/// Opaque DSP instance, exported as `interpreter_dsp*` in C.
///
/// Holds a non-owning raw pointer to its parent `InterpreterDspFactory`.
/// The factory cache owns this value until manual instance deletion or final
/// factory deletion, and drops it before dropping the parent factory.
pub struct InterpreterDspInstance {
    /// Non-owning pointer to the parent factory while this instance is alive.
    pub(crate) factory: *const InterpreterDspFactory,
    /// Execution heaps (int + real) — precision matches the factory.
    pub(crate) executor: FbcExecutorAny,
    /// One `*mut c_void` slot per soundfile, written by `SoundUI::addSoundfile`.
    ///
    /// Each element is the `Soundfile*` provided by the host audio layer after
    /// `buildUserInterface` completes.  Allocated once at instance creation with
    /// `null_mut()` sentinels so the vector's address is stable before the first
    /// UI traversal.
    pub(crate) soundfile_zones: Vec<*mut c_void>,
    /// Whether `init()` has been called.
    pub(crate) initialized: bool,
    /// Number of `compute()` cycles executed.
    pub(crate) cycle: usize,
    /// Conversion buffers for audio I/O of the other width (see [`IoScratch`]).
    pub(crate) io_scratch: IoScratch,
}

// SAFETY: DSP instances are not shared between threads (Faust API contract).
unsafe impl Send for InterpreterDspInstance {}

/// Allocates a C string on the Rust heap and returns a raw owning pointer.
///
/// The returned pointer must be freed with [`free_c_string`].
pub(crate) fn alloc_c_string(s: &str) -> *mut c_char {
    ffi_common::alloc_c_string(s)
}

// ── Write helpers (generic, used by factory.rs) ───────────────────────────────

/// Serialize any factory variant to `.fbc` text.
pub(crate) fn write_fbc_any(
    factory: &FbcDspFactoryAny,
    writer: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    use codegen::backends::interp::write_fbc;
    match factory {
        FbcDspFactoryAny::Float32(f) => write_fbc(f, writer, false),
        FbcDspFactoryAny::Float64(f) => write_fbc(f, writer, false),
    }
}
