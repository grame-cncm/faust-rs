//! Instance-level `extern "C"` functions.
//!
//! Implements the C API from `interpreter-dsp-c.h` for DSP instance lifecycle,
//! audio computation, UI, and metadata.
//!
//! # Lifetime contract
//! Instances are registered with their factory. They may be deleted manually,
//! or are invalidated and dropped when the last factory reference is deleted,
//! matching the maintained C++ API contract.
//!
//! # Float/Double dispatch
//! All operations delegate to `FbcDspFactoryAny` / `FbcExecutorAny` helpers
//! so that a single shared library handles both `float` and `double` factories
//! transparently.  Audio I/O buffers remain `f32*` at the C ABI boundary;
//! in double mode, samples are converted `f32 → f64` on entry and
//! `f64 → f32` on exit inside `computeCInterpreterDSPInstance`.

use std::ffi::{c_char, c_void};
use std::os::raw::c_int;

use codegen::backends::interp::Soundfile;

use crate::cache::{cache_register_instance, cache_remove_instance};
use crate::types::{
    FaustFloat, FbcExecutorAny, InterpreterDspFactory, InterpreterDspInstance, MetaGlue, UIGlue,
};
use crate::ui::dispatch_meta;

// ── C++ Soundfile struct mirror ───────────────────────────────────────────────

/// Mirrors the packed C++ `Soundfile` struct from `<faust/gui/Soundfile.h>`.
///
/// Fields are laid out identically to `__attribute__((__packed__))` in C++,
/// which on 64-bit targets matches natural alignment (no trailing padding is
/// inserted between these particular types).
#[repr(C, packed)]
struct CSoundfile {
    f_buffers: *mut c_void, // float** or double** depending on f_is_double
    f_length: *mut i32,     // [MAX_SOUNDFILE_PARTS] — length in frames per part
    f_sr: *mut i32,         // [MAX_SOUNDFILE_PARTS] — sample rate per part
    f_offset: *mut i32,     // [MAX_SOUNDFILE_PARTS] — frame offset per part
    f_channels: i32,        // number of channels
    f_parts: i32,           // number of parts
    f_is_double: bool,      // true → buffers are f64, false → f32
}

/// After `buildUserInterface` the host (`SoundUI`) has written loaded
/// `Soundfile*` pointers into `zones`.  This function reads each C++ struct
/// and replaces the corresponding `default_silence` entry in the executor.
///
/// # Safety
/// Every non-null element of `zones` must point to a valid `Soundfile`
/// object whose lifetime extends at least until this function returns.
unsafe fn sync_soundfiles_from_zones(exec: &mut FbcExecutorAny, zones: &[*mut c_void]) {
    use std::ptr::addr_of;

    for (slot, &zone) in zones.iter().enumerate() {
        if zone.is_null() {
            continue;
        }

        // SAFETY: non-null zone was written by SoundUI::addSoundfile with a
        // valid heap-allocated Soundfile*.
        let csf: *const CSoundfile = zone as *const CSoundfile;

        // Use read_unaligned via addr_of! in case the compiler sees a packed ref.
        let channels = unsafe { std::ptr::read_unaligned(addr_of!((*csf).f_channels)) } as usize;
        let parts = unsafe { std::ptr::read_unaligned(addr_of!((*csf).f_parts)) } as usize;
        let is_double = unsafe { std::ptr::read_unaligned(addr_of!((*csf).f_is_double)) };
        let len_ptr = unsafe { std::ptr::read_unaligned(addr_of!((*csf).f_length)) };
        let sr_ptr = unsafe { std::ptr::read_unaligned(addr_of!((*csf).f_sr)) };
        let off_ptr = unsafe { std::ptr::read_unaligned(addr_of!((*csf).f_offset)) };
        let buf_ptr = unsafe { std::ptr::read_unaligned(addr_of!((*csf).f_buffers)) };

        if parts == 0
            || channels == 0
            || len_ptr.is_null()
            || sr_ptr.is_null()
            || off_ptr.is_null()
            || buf_ptr.is_null()
        {
            continue;
        }

        let lengths: Vec<i32> = unsafe { std::slice::from_raw_parts(len_ptr, parts).to_vec() };
        let sample_rates: Vec<i32> = unsafe { std::slice::from_raw_parts(sr_ptr, parts).to_vec() };
        let offsets: Vec<i32> = unsafe { std::slice::from_raw_parts(off_ptr, parts).to_vec() };

        // Total contiguous buffer length per channel.
        let total_frames = (offsets[parts - 1] + lengths[parts - 1]).max(0) as usize;

        let mut buffers: Vec<Vec<f64>> = Vec::with_capacity(channels);
        for c in 0..channels {
            let chan_buf: Vec<f64> = if is_double {
                let ptrs = buf_ptr as *const *const f64;
                let ptr = unsafe { *ptrs.add(c) };
                unsafe { std::slice::from_raw_parts(ptr, total_frames) }.to_vec()
            } else {
                let ptrs = buf_ptr as *const *const f32;
                let ptr = unsafe { *ptrs.add(c) };
                unsafe { std::slice::from_raw_parts(ptr, total_frames) }
                    .iter()
                    .map(|&x| x as f64)
                    .collect()
            };
            buffers.push(chan_buf);
        }

        let sf = Soundfile {
            num_channels: channels,
            num_parts: parts,
            lengths,
            sample_rates,
            offsets,
            buffers,
        };
        exec.set_soundfile(slot, sf);
    }
}

// ── Instance creation / deletion ─────────────────────────────────────────────

/// Create a new DSP instance from a factory.
///
/// Triggers one-shot factory optimization.
///
/// # Safety
/// `factory` must be a valid non-null factory pointer. The returned instance
/// remains valid until it is manually deleted, its factory is finally
/// released, or all factories are cleared.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn createCInterpreterDSPInstance(
    factory: *mut InterpreterDspFactory,
) -> *mut InterpreterDspInstance {
    unsafe {
        if factory.is_null() {
            return std::ptr::null_mut();
        }
        // Trigger one-shot optimization (idempotent after first call).
        (*factory).inner.optimize();

        let sf_count = (*factory).inner.soundfile_count();
        let soundfile_zones = vec![std::ptr::null_mut(); sf_count];
        let executor = FbcExecutorAny::new_for_factory(&(*factory).inner);
        cache_register_instance(
            factory,
            InterpreterDspInstance {
                factory: factory.cast_const(),
                executor,
                soundfile_zones,
                initialized: false,
                cycle: 0,
            },
        )
    }
}

/// Delete a DSP instance.
///
/// # Safety
/// `dsp` must be a valid non-null pointer previously returned by
/// `createCInterpreterDSPInstance`, and must not be used after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn deleteCInterpreterDSPInstance(dsp: *mut InterpreterDspInstance) {
    if !dsp.is_null() {
        let _ = cache_remove_instance(dsp);
    }
}

// ── Audio layout ──────────────────────────────────────────────────────────────

/// Return the number of audio inputs.
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getNumInputsCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
) -> c_int {
    unsafe {
        if dsp.is_null() {
            return 0;
        }
        (*(*dsp).factory).inner.num_inputs()
    }
}

/// Return the number of audio outputs.
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getNumOutputsCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
) -> c_int {
    unsafe {
        if dsp.is_null() {
            return 0;
        }
        (*(*dsp).factory).inner.num_outputs()
    }
}

/// Return the current sample rate.
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getSampleRateCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
) -> c_int {
    unsafe {
        if dsp.is_null() {
            return 0;
        }
        let sr_off = (*(*dsp).factory).inner.sr_offset() as usize;
        (*dsp).executor.int_heap().get(sr_off).copied().unwrap_or(0)
    }
}

// ── Initialization lifecycle ───────────────────────────────────────────────────

/// Full initialization: sets sample rate, runs all init sub-steps.
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn initCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
    sample_rate: c_int,
) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        (*dsp).initialized = true;
        instanceInitCInterpreterDSPInstance(dsp, sample_rate);
    }
}

/// Run all instance init sub-steps (class_init + constants + reset UI + clear).
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn instanceInitCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
    sample_rate: c_int,
) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        class_init_instance(dsp, sample_rate);
        instanceConstantsCInterpreterDSPInstance(dsp, sample_rate);
        instanceResetUserInterfaceCInterpreterDSPInstance(dsp);
        instanceClearCInterpreterDSPInstance(dsp);
    }
}

/// Set the sample rate and execute the init (constants) block.
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn instanceConstantsCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
    sample_rate: c_int,
) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        let factory = &(*(*dsp).factory).inner;
        let sr_off = factory.sr_offset() as usize;
        if let Some(slot) = (*dsp).executor.int_heap_mut().get_mut(sr_off) {
            *slot = sample_rate;
        }
        let init_block = factory.init_block();
        factory.execute_block_on(&mut (*dsp).executor, init_block);
    }
}

/// Execute the reset-UI block (sets controls to default values).
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn instanceResetUserInterfaceCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        let factory = &(*(*dsp).factory).inner;
        let block = factory.reset_ui_block();
        factory.execute_block_on(&mut (*dsp).executor, block);
    }
}

/// Execute the clear block (zeros delay lines and state variables).
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn instanceClearCInterpreterDSPInstance(dsp: *mut InterpreterDspInstance) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        let factory = &(*(*dsp).factory).inner;
        let block = factory.clear_block();
        factory.execute_block_on(&mut (*dsp).executor, block);
    }
}

// ── Clone ─────────────────────────────────────────────────────────────────────

/// Clone a DSP instance.
///
/// The clone shares the same factory but has independent heaps.
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cloneCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
) -> *mut InterpreterDspInstance {
    unsafe {
        if dsp.is_null() {
            return std::ptr::null_mut();
        }
        let factory_ptr = (*dsp).factory;
        let factory = &(*factory_ptr).inner;

        let mut new_executor = FbcExecutorAny::new_for_factory(factory);
        new_executor.copy_from(&(*dsp).executor);

        // Clone soundfile zones: copy the pointers (same C++ Soundfile* targets).
        let new_soundfile_zones = (*dsp).soundfile_zones.clone();

        cache_register_instance(
            factory_ptr.cast_mut(),
            InterpreterDspInstance {
                factory: factory_ptr,
                executor: new_executor,
                soundfile_zones: new_soundfile_zones,
                initialized: (*dsp).initialized,
                cycle: 0,
            },
        )
    }
}

// ── User interface ─────────────────────────────────────────────────────────────

/// Traverse the UI instruction list and invoke `UIGlue` callbacks.
///
/// In double mode, scalar params are narrowed `f64 → f32`; zone pointers
/// are `*mut f64` reinterpreted as `*mut f32` (app must use `FAUSTFLOAT=double`).
///
/// # Safety
/// - `dsp` must be a valid non-null instance pointer.
/// - `glue` must be a valid non-null `UIGlue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buildUserInterfaceCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
    glue: *mut UIGlue,
) {
    unsafe {
        if dsp.is_null() || glue.is_null() {
            return;
        }
        let factory = &(*(*dsp).factory).inner;
        factory.dispatch_ui_glue(&mut (*dsp).executor, &mut (*dsp).soundfile_zones, glue);
        // Sync real audio data from the C++ Soundfile objects now that the
        // host has finished populating soundfile_zones.
        sync_soundfiles_from_zones(&mut (*dsp).executor, &(*dsp).soundfile_zones);
    }
}

/// Traverse the metadata instruction list and invoke `MetaGlue.declare`.
///
/// # Safety
/// - `dsp` must be a valid non-null instance pointer.
/// - `meta` must be a valid non-null `MetaGlue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn metadataCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
    meta: *mut MetaGlue,
) {
    unsafe {
        if dsp.is_null() || meta.is_null() {
            return;
        }
        let meta_block = (*(*dsp).factory).inner.meta_block();
        dispatch_meta(meta_block, meta);
    }
}

// ── Audio computation ──────────────────────────────────────────────────────────

/// Process one buffer of audio samples.
///
/// - `count`   — number of frames.
/// - `inputs`  — array of `num_inputs` non-interleaved `float*` buffers.
/// - `outputs` — array of `num_outputs` non-interleaved `float*` buffers.
///
/// In double mode, samples are converted `f32 → f64` on input and
/// `f64 → f32` on output transparently.
///
/// # Safety
/// - `dsp` must be a valid non-null instance pointer.
/// - Each `inputs[i]` and `outputs[i]` must point to at least `count` floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn computeCInterpreterDSPInstance(
    dsp: *mut InterpreterDspInstance,
    count: c_int,
    inputs: *mut *mut FaustFloat,
    outputs: *mut *mut FaustFloat,
) {
    unsafe {
        if dsp.is_null() || count <= 0 {
            return;
        }

        let factory = &(*(*dsp).factory).inner;
        let n = count as usize;
        let num_in = factory.num_inputs() as usize;
        let num_out = factory.num_outputs() as usize;

        // Build input/output slice views.
        let input_slices: Vec<&[FaustFloat]> = (0..num_in)
            .map(|i| std::slice::from_raw_parts(*inputs.add(i), n))
            .collect();
        let mut output_slices: Vec<&mut [FaustFloat]> = (0..num_out)
            .map(|i| std::slice::from_raw_parts_mut(*outputs.add(i), n))
            .collect();

        // Store frame count in the 'count' heap slot.
        // count_offset == -1 means the factory has no dedicated count slot
        // (uncommon but valid for DSPs without audio loops).  Casting -1_i32
        // to usize gives usize::MAX so get_mut safely returns None.
        let count_off_raw = factory.count_offset();
        if count_off_raw >= 0 {
            let count_off = count_off_raw as usize;
            if let Some(slot) = (*dsp).executor.int_heap_mut().get_mut(count_off) {
                *slot = count;
            } else {
                debug_assert!(
                    false,
                    "computeCInterpreterDSPInstance: count_offset {count_off} out of int_heap bounds"
                );
            }
        } else {
            debug_assert!(
                count_off_raw == -1,
                "computeCInterpreterDSPInstance: unexpected negative count_offset {count_off_raw}"
            );
        }

        // Execute control block then DSP block with audio I/O.
        let compute_block = factory.compute_block();
        let compute_dsp_block = factory.compute_dsp_block();

        factory.execute_block_on(&mut (*dsp).executor, compute_block);
        factory.execute_block_io_f32(
            &mut (*dsp).executor,
            compute_dsp_block,
            &input_slices,
            &mut output_slices,
        );

        (*dsp).cycle += 1;
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Execute the static init block (classInit equivalent).
///
/// The sample rate is published to the heap first. `classInit` takes a
/// `sample_rate` in every backend, and a generated table whose content depends
/// on it reads that argument: the C++ output forwards it as
/// `sig0->instanceInitmydspSIG0(sample_rate)`. In the interpreter a
/// `sample_rate` function argument is compiled to a load of the `fSampleRate`
/// heap slot, so without this store the generator fills its table from a slot
/// that only `instanceConstants` writes — and that runs afterwards.
/// `subcontainer1.dsp` then reads 1 (`fmax(1, 0)`) instead of the sample rate.
unsafe fn class_init_instance(dsp: *mut InterpreterDspInstance, sample_rate: c_int) {
    unsafe {
        let factory = &(*(*dsp).factory).inner;
        let sr_off = factory.sr_offset() as usize;
        if let Some(slot) = (*dsp).executor.int_heap_mut().get_mut(sr_off) {
            *slot = sample_rate;
        }
        let block = factory.static_init_block();
        factory.execute_block_on(&mut (*dsp).executor, block);
    }
}

// ── C-string helper re-exported for the C++ wrapper ──────────────────────────

/// Expose a non-null version string for the C++ wrapper header.
///
/// # Safety
/// The returned pointer is process-static and must not be freed or mutated.
#[unsafe(no_mangle)]
pub extern "C" fn getInterpreterDSPInstanceVersion() -> *const c_char {
    crate::factory::getCLibFaustVersion()
}
