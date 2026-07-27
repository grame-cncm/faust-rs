//! Opaque FFI types and allocation helpers.
//!
//! `InterpreterDspFactory` and `InterpreterDspInstance` are heap-allocated
//! Rust objects exposed to C as opaque pointer types.  Ownership rules mirror
//! the original Faust C API:
//! - A factory is created by one of the `read/create*Factory` functions and
//!   deleted by `deleteCInterpreterDSPFactory`.
//! - An instance is created by `createCInterpreterDSPInstance` and deleted by
//!   `deleteCInterpreterDSPInstance`.
//! - The factory **must** outlive all of its instances.
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
//! `f64 → f32` on output inside `computeCInterpreterDSPInstance`.
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

    /// Execute the DSP block with audio I/O.
    ///
    /// Audio buffers are always `f32` at the C ABI level.  In double mode,
    /// inputs are widened `f32 → f64` before execution and outputs are
    /// narrowed `f64 → f32` after execution.
    ///
    /// # Safety
    /// The caller must ensure `inputs`/`outputs` are valid for the duration
    /// of this call (same contract as `FbcExecutor::execute_block_io`).
    pub fn execute_block_io_f32(
        &self,
        exec: &mut FbcExecutorAny,
        block_id: BlockId,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
    ) {
        match (self, exec) {
            (Self::Float32(f), FbcExecutorAny::Float32(e)) => {
                e.execute_block_io(&f.arena, block_id, inputs, outputs);
            }
            (Self::Float64(f), FbcExecutorAny::Float64(e)) => {
                // Widen f32 inputs → f64.
                let f64_inputs: Vec<Vec<f64>> = inputs
                    .iter()
                    .map(|sl| sl.iter().map(|&x| x as f64).collect())
                    .collect();
                let f64_input_refs: Vec<&[f64]> = f64_inputs.iter().map(|v| v.as_slice()).collect();

                // Allocate f64 output buffers.
                let n = outputs.first().map_or(0, |s| s.len());
                let num_out = outputs.len();
                let mut f64_outputs: Vec<Vec<f64>> =
                    (0..num_out).map(|_| vec![0.0f64; n]).collect();
                let mut f64_output_refs: Vec<&mut [f64]> =
                    f64_outputs.iter_mut().map(|v| v.as_mut_slice()).collect();

                e.execute_block_io(&f.arena, block_id, &f64_input_refs, &mut f64_output_refs);

                // Narrow f64 outputs → f32.
                for (src, dst) in f64_outputs.iter().zip(outputs.iter_mut()) {
                    for (&s, d) in src.iter().zip(dst.iter_mut()) {
                        *d = s as f32;
                    }
                }
            }
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

// ── Opaque wrapper types ──────────────────────────────────────────────────────

/// Opaque DSP factory, exported as `interpreter_dsp_factory*` in C.
///
/// Owns an `FbcDspFactoryAny` (either `f32` or `f64`) on the Rust heap.
/// Allocated via `alloc_factory`, freed via `free_factory`.
pub struct InterpreterDspFactory {
    pub(crate) inner: FbcDspFactoryAny,
}

/// Opaque DSP instance, exported as `interpreter_dsp*` in C.
///
/// Holds a non-owning raw pointer to its parent `InterpreterDspFactory`.
/// The factory MUST outlive this instance (same contract as the C++ API).
/// Allocated via `alloc_instance`, freed via `free_instance`.
pub struct InterpreterDspInstance {
    /// Non-owning pointer to the parent factory (factory outlives instance).
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
}

// SAFETY: DSP instances are not shared between threads (Faust API contract).
unsafe impl Send for InterpreterDspInstance {}

// ── Allocation / deallocation helpers ────────────────────────────────────────

/// Boxes an `FbcDspFactoryAny` and returns a raw owning pointer.
///
/// The caller is responsible for eventually calling [`free_factory`].
pub(crate) fn alloc_factory(inner: FbcDspFactoryAny) -> *mut InterpreterDspFactory {
    ffi_common::alloc_opaque(InterpreterDspFactory { inner })
}

/// Drops the boxed `InterpreterDspFactory`.
///
/// # Safety
/// `ptr` must be a valid non-null pointer previously returned by [`alloc_factory`],
/// and must not be used after this call.
pub(crate) unsafe fn free_factory(ptr: *mut InterpreterDspFactory) {
    unsafe { ffi_common::free_opaque(ptr) }
}

/// Boxes a new `InterpreterDspInstance` and returns a raw owning pointer.
pub(crate) fn alloc_instance(
    factory: *const InterpreterDspFactory,
    executor: FbcExecutorAny,
    soundfile_zones: Vec<*mut c_void>,
) -> *mut InterpreterDspInstance {
    ffi_common::alloc_opaque(InterpreterDspInstance {
        factory,
        executor,
        soundfile_zones,
        initialized: false,
        cycle: 0,
    })
}

/// Drops the boxed `InterpreterDspInstance`.
///
/// # Safety
/// `ptr` must be a valid non-null pointer previously returned by [`alloc_instance`],
/// and must not be used after this call.
pub(crate) unsafe fn free_instance(ptr: *mut InterpreterDspInstance) {
    unsafe { ffi_common::free_opaque(ptr) }
}

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
