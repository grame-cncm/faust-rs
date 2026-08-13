//! Instance-level `extern "C"` functions for `cranelift_dsp`.
//!
//! This module owns the runtime DSP instance contract:
//! - allocate one backend `dsp*` state buffer per instance,
//! - invoke finalized Cranelift `compute` entry points,
//! - dispatch UI/meta callbacks from the native FIR-derived runtime descriptor.
//!
//! The design keeps one factory -> multiple instances semantics and isolates all
//! function pointer invocation in documented `unsafe` boundaries. Instances are
//! registered for automatic deletion with their owning cached factory.

use std::ffi::c_void;
use std::os::raw::c_int;

use codegen::backends::cranelift::{StructFieldKind, StructFieldLayout, StructLayoutPlan};
use fir::FirType;

use crate::cache::{cache_register_instance, cache_remove_instance};
use crate::runtime::{RuntimeDescriptor, RuntimeFieldInit, RuntimeUiItem};
use crate::types::{
    CraneliftDspFactory, CraneliftDspInstance, DspStateBuffer, FaustFloat, ManagedClassStorage,
    MetaGlue, UIGlue,
};

/// Typed JIT `compute` signature used by the standalone Cranelift runtime.
///
/// This matches the standard Faust DSP ABI:
/// `compute(dsp*, count, inputs**, outputs**)`.
type ComputeFn =
    unsafe extern "C" fn(*mut c_void, c_int, *mut *mut FaustFloat, *mut *mut FaustFloat);

/// Typed JIT `instanceConstants` signature used by the standalone runtime.
type InstanceConstantsFn = unsafe extern "C" fn(*mut c_void, c_int);

/// Typed JIT `instanceClear` signature used by the standalone runtime.
type InstanceClearFn = unsafe extern "C" fn(*mut c_void);

/// Converts a Rust channel count to the C ABI integer type with saturation.
fn arity_to_c_int(value: usize) -> c_int {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Create a new Cranelift DSP instance from a factory.
///
/// # Safety
/// `factory` must be a valid non-null factory pointer. The returned instance
/// remains valid until it is manually deleted, its factory is finally
/// released, or all factories are cleared.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn createCCraneliftDSPInstance(
    factory: *mut CraneliftDspFactory,
) -> *mut CraneliftDspInstance {
    unsafe {
        if factory.is_null() {
            return std::ptr::null_mut();
        }
        let Some(jit) = (*factory).compiled_jit.as_ref() else {
            return std::ptr::null_mut();
        };
        if ensure_class_storage(factory).is_err() {
            return std::ptr::null_mut();
        }
        let layout = jit.struct_layout();
        let state_result = match jit.mem0_analysis() {
            Some(analysis) => {
                let binding = match (*factory).memory_state.lock() {
                    Ok(state) => state.binding,
                    Err(_) => return std::ptr::null_mut(),
                };
                let Some(binding) = binding else {
                    return std::ptr::null_mut();
                };
                DspStateBuffer::new_managed(layout, analysis, binding)
            }
            None => {
                DspStateBuffer::new(layout.size_bytes() as usize, layout.align_bytes() as usize)
            }
        };
        let state = match state_result {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        if let Ok(mut memory) = (*factory).memory_state.lock() {
            memory.live_instances = memory.live_instances.saturating_add(1);
        } else {
            return std::ptr::null_mut();
        }
        cache_register_instance(
            factory,
            CraneliftDspInstance {
                factory: factory.cast_const(),
                sample_rate: 0,
                initialized: false,
                cycle: 0,
                dsp_state: state,
            },
        )
    }
}

/// Delete a Cranelift DSP instance.
///
/// # Safety
/// `dsp` must be a valid pointer returned by
/// [`createCCraneliftDSPInstance`] and must not be used after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn deleteCCraneliftDSPInstance(dsp: *mut CraneliftDspInstance) {
    if !dsp.is_null() {
        let _ = cache_remove_instance(dsp);
    }
}

/// Clone a Cranelift DSP instance.
///
/// # Safety
/// `dsp` must be a valid non-null instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cloneCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
) -> *mut CraneliftDspInstance {
    unsafe {
        if dsp.is_null() {
            return std::ptr::null_mut();
        }
        let state = match (*dsp).dsp_state.deep_clone() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        if let Some(factory) = (*dsp).factory.as_ref() {
            if let Ok(mut memory) = factory.memory_state.lock() {
                memory.live_instances = memory.live_instances.saturating_add(1);
            } else {
                return std::ptr::null_mut();
            }
        }
        cache_register_instance(
            (*dsp).factory.cast_mut(),
            CraneliftDspInstance {
                factory: (*dsp).factory,
                sample_rate: (*dsp).sample_rate,
                initialized: (*dsp).initialized,
                cycle: (*dsp).cycle,
                dsp_state: state,
            },
        )
    }
}

/// Return the number of audio inputs.
///
/// # Safety
/// `dsp` must be a valid instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getNumInputsCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
) -> c_int {
    unsafe {
        if dsp.is_null() || (*dsp).factory.is_null() {
            return 0;
        }
        arity_to_c_int((*(*dsp).factory).num_inputs)
    }
}

/// Return the number of audio outputs.
///
/// # Safety
/// `dsp` must be a valid instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getNumOutputsCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
) -> c_int {
    unsafe {
        if dsp.is_null() || (*dsp).factory.is_null() {
            return 0;
        }
        arity_to_c_int((*(*dsp).factory).num_outputs)
    }
}

/// Return the current sample rate recorded in the instance.
///
/// # Safety
/// `dsp` must be a valid instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getSampleRateCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
) -> c_int {
    unsafe {
        if dsp.is_null() {
            return 0;
        }
        (*dsp).sample_rate
    }
}

/// Full initialization entry point (`init`): records sample rate and marks initialized.
///
/// # Safety
/// `dsp` must be a valid instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn initCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
    sample_rate: c_int,
) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        if !class_init_instance(dsp, sample_rate) {
            return;
        }
        instanceInitCCraneliftDSPInstance(dsp, sample_rate);
        (*dsp).initialized = true;
    }
}

/// Instance init entry point (runs class-init/constants/reset/clear sequence).
///
/// # Safety
/// `dsp` must be a valid instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn instanceInitCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
    sample_rate: c_int,
) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        instanceConstantsCCraneliftDSPInstance(dsp, sample_rate);
        instanceResetUserInterfaceCCraneliftDSPInstance(dsp);
        instanceClearCCraneliftDSPInstance(dsp);
    }
}

/// Record the sample rate in the instance and run sidecar init block.
///
/// # Safety
/// `dsp` must be a valid instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn instanceConstantsCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
    sample_rate: c_int,
) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        (*dsp).sample_rate = sample_rate;
        let Some(factory) = (*dsp).factory.as_ref() else {
            return;
        };
        let Some(jit) = factory.compiled_jit.as_ref() else {
            return;
        };
        if let Some(instance_constants) =
            instance_constants_fn_from_addr(jit.instance_constants_entry_addr())
        {
            let dsp_ptr = (*dsp).dsp_state.as_mut_ptr().cast::<c_void>();
            if !dsp_ptr.is_null() {
                instance_constants(dsp_ptr, sample_rate);
            }
        } else {
            apply_constant_inits(&mut (*dsp).dsp_state, jit.struct_layout(), &factory.runtime);
            apply_sample_rate(
                &mut (*dsp).dsp_state,
                jit.struct_layout(),
                &factory.runtime,
                sample_rate,
            );
        }
    }
}

/// Applies all constant/global initializers recorded in the runtime descriptor.
fn apply_constant_inits(
    dsp_state: &mut DspStateBuffer,
    layout: &StructLayoutPlan,
    runtime: &RuntimeDescriptor,
) {
    for (name, init) in &runtime.field_inits {
        let Some(field) = layout.field(name) else {
            continue;
        };
        write_field_init(dsp_state, field, init);
    }
}

/// Writes the current sample rate into all FIR-recognized sample-rate fields.
///
/// Faust DSP modules may expose the rate through differently typed struct
/// fields (`Int32`, `Float32`, `Float64`, `FaustFloat`), so this helper
/// normalizes one external `c_int` request across those storage forms.
fn apply_sample_rate(
    dsp_state: &mut DspStateBuffer,
    layout: &StructLayoutPlan,
    runtime: &RuntimeDescriptor,
    sample_rate: c_int,
) {
    for name in &runtime.sample_rate_fields {
        let Some(field) = layout.field(name) else {
            continue;
        };
        match &field.kind {
            StructFieldKind::Scalar(FirType::Int32) => {
                write_i32(dsp_state, field.offset_bytes as usize, sample_rate);
            }
            StructFieldKind::Scalar(FirType::Float32 | FirType::FaustFloat) => {
                write_f32(dsp_state, field.offset_bytes as usize, sample_rate as f32);
            }
            StructFieldKind::Scalar(FirType::Float64) => {
                write_f64(dsp_state, field.offset_bytes as usize, sample_rate as f64);
            }
            _ => {}
        }
    }
}

/// Reset UI state by executing sidecar reset-ui instructions when available.
///
/// # Safety
/// `dsp` must be a valid instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn instanceResetUserInterfaceCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
) {
    unsafe {
        let Some(dsp) = dsp.as_mut() else {
            return;
        };
        let Some(factory) = dsp.factory.as_ref() else {
            return;
        };
        let Some(jit) = factory.compiled_jit.as_ref() else {
            return;
        };
        apply_control_defaults(&mut dsp.dsp_state, jit.struct_layout(), &factory.runtime);
    }
}

/// Clear DSP state and reset cycle counter.
///
/// # Safety
/// `dsp` must be a valid instance pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn instanceClearCCraneliftDSPInstance(dsp: *mut CraneliftDspInstance) {
    unsafe {
        if dsp.is_null() {
            return;
        }
        let Some(factory) = (*dsp).factory.as_ref() else {
            return;
        };
        let Some(jit) = factory.compiled_jit.as_ref() else {
            return;
        };
        // Match the generated C++ backend: `instanceClear` is the compiled FIR
        // clear body. Runtime-side clearing would duplicate or contradict that
        // backend contract.
        if let Some(instance_clear) = instance_clear_fn_from_addr(jit.instance_clear_entry_addr()) {
            let dsp_ptr = (*dsp).dsp_state.as_mut_ptr().cast::<c_void>();
            if !dsp_ptr.is_null() {
                instance_clear(dsp_ptr);
            }
        }
        (*dsp).cycle = 0;
    }
}

/// Trigger UI callbacks for the instance from sidecar UI instruction lists.
///
/// # Safety
/// `dsp` and `ui` may be null; null values are ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn buildUserInterfaceCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
    ui: *mut UIGlue,
) {
    unsafe {
        if dsp.is_null() || ui.is_null() {
            return;
        }
        let Some(factory) = (*dsp).factory.as_ref() else {
            return;
        };
        let Some(jit) = factory.compiled_jit.as_ref() else {
            return;
        };
        dispatch_ui_runtime(
            &factory.runtime,
            jit.struct_layout(),
            &mut (*dsp).dsp_state,
            ui,
        );
    }
}

/// Trigger metadata callbacks for the instance.
///
/// # Safety
/// `meta` may be null. If non-null and `declare` is set, callback contract is
/// the caller's responsibility.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn metadataCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
    meta: *mut MetaGlue,
) {
    unsafe {
        if meta.is_null() || dsp.is_null() {
            return;
        }
        let Some(factory) = (*dsp).factory.as_ref() else {
            return;
        };
        let Some(declare) = (*meta).declare else {
            return;
        };
        for (key, value) in &factory.runtime.meta_entries {
            let key = std::ffi::CString::new(key.as_str()).ok();
            let value = std::ffi::CString::new(value.as_str()).ok();
            if let (Some(key), Some(value)) = (key, value) {
                declare((*meta).meta_interface, key.as_ptr(), value.as_ptr());
            }
        }
        let key = c"backend";
        let value = c"cranelift";
        declare((*meta).meta_interface, key.as_ptr(), value.as_ptr());
        let key = c"cranelift-jit-compiled";
        let value = if factory.compiled_jit.is_some() {
            c"true"
        } else {
            c"false"
        };
        declare((*meta).meta_interface, key.as_ptr(), value.as_ptr());
        let key = c"cranelift-compute-body-lowered";
        let value = if factory.compute_body_lowered {
            c"true"
        } else {
            c"false"
        };
        declare((*meta).meta_interface, key.as_ptr(), value.as_ptr());
    }
}

/// Compute audio for one block by invoking the finalized Cranelift JIT entry.
///
/// # Safety
/// `dsp` must be a valid instance pointer and `inputs`/`outputs` must follow
/// the standard Faust `FAUSTFLOAT**` contract for `count` frames.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn computeCCraneliftDSPInstance(
    dsp: *mut CraneliftDspInstance,
    count: c_int,
    input: *mut *mut FaustFloat,
    output: *mut *mut FaustFloat,
) {
    unsafe {
        if dsp.is_null() || count <= 0 {
            return;
        }
        let Some(factory) = (*dsp).factory.as_ref() else {
            return;
        };
        let Some(jit) = factory.compiled_jit.as_ref() else {
            return;
        };
        let compute = match compute_fn_from_addr(jit.compute_entry_addr()) {
            Some(f) => f,
            None => return,
        };
        let dsp_ptr = (*dsp).dsp_state.as_mut_ptr().cast::<c_void>();
        if dsp_ptr.is_null() {
            return;
        }
        compute(dsp_ptr, count, input, output);
        (*dsp).cycle = (*dsp).cycle.saturating_add(1);
    }
}

/// Instance scaffold status string kept for module-presence tests.
#[must_use]
pub fn instance_status() -> &'static str {
    "cranelift-ffi instance runtime"
}

/// Runs the JIT-compiled `staticInit`, which fills the generated tables.
///
/// A runtime-filled table is a zeroed JIT data object; without this call
/// nothing ever writes it and every read returns 0. `staticInit` shares the
/// `(dsp, sample_rate)` ABI with `instanceConstants`, and takes the sample rate
/// because a generator's content may depend on it — `subcontainer1.dsp` is the
/// case that does.
///
/// Modules with no generated table have no `staticInit`, so the address is 0
/// and this is the no-op it has always been.
unsafe fn class_init_instance(dsp: *mut CraneliftDspInstance, sample_rate: c_int) -> bool {
    unsafe {
        let Some(factory) = (*dsp).factory.as_ref() else {
            return false;
        };
        let Some(jit) = factory.compiled_jit.as_ref() else {
            return false;
        };
        if jit.mem0_analysis().is_none() {
            if let Some(static_init) = instance_constants_fn_from_addr(jit.static_init_entry_addr())
            {
                let dsp_ptr = (*dsp).dsp_state.as_mut_ptr().cast::<c_void>();
                if !dsp_ptr.is_null() {
                    static_init(dsp_ptr, sample_rate);
                }
            }
            return true;
        }
        {
            let Ok(mut state) = factory.memory_state.lock() else {
                return false;
            };
            if state.class_sample_rate == Some(sample_rate) {
                return true;
            }
            if state.class_sample_rate.is_some() || state.class_busy {
                return false;
            }
            state.class_busy = true;
        }
        let dsp_ptr = (*dsp).dsp_state.as_mut_ptr().cast::<c_void>();
        if let Some(static_init) = instance_constants_fn_from_addr(jit.static_init_entry_addr())
            && !dsp_ptr.is_null()
        {
            static_init(dsp_ptr, sample_rate);
        }
        let Ok(mut state) = factory.memory_state.lock() else {
            return false;
        };
        state.class_sample_rate = Some(sample_rate);
        state.class_busy = false;
        true
    }
}

/// Allocates factory-owned generated tables exactly once, outside locks while
/// invoking host callbacks. A `-mem0` factory remains intentionally unbound
/// after compilation/deserialization and cannot create instances until its
/// manager has been set.
unsafe fn ensure_class_storage(factory: *mut CraneliftDspFactory) -> Result<(), String> {
    unsafe {
        let factory_ref = factory
            .as_ref()
            .ok_or_else(|| "null Cranelift factory".to_owned())?;
        let Some(jit) = factory_ref.compiled_jit.as_ref() else {
            return Err("Cranelift factory has no JIT module".to_owned());
        };
        let Some(analysis) = jit.mem0_analysis() else {
            return Ok(());
        };
        let binding = {
            let mut state = factory_ref
                .memory_state
                .lock()
                .map_err(|_| "Cranelift memory-manager state is poisoned".to_owned())?;
            if state.class_storage.is_some() {
                return Ok(());
            }
            if state.class_busy {
                return Err("Cranelift class allocation is already in progress".to_owned());
            }
            let binding = state
                .binding
                .ok_or_else(|| "-mem0 factory has no bound memory manager".to_owned())?;
            state.class_busy = true;
            binding
        };
        let storage = ManagedClassStorage::create(binding, analysis, jit.static_memory_slots());
        let mut state = factory_ref
            .memory_state
            .lock()
            .map_err(|_| "Cranelift memory-manager state is poisoned".to_owned())?;
        state.class_busy = false;
        match storage {
            Ok(storage) => {
                state.class_storage = Some(storage);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

/// Replays FIR-derived UI items through the exported `UIGlue` callback table.
///
/// Zone pointers are resolved directly against the native `dsp_state` buffer so
/// controls and bargraphs share the same storage seen by JIT `compute`.
fn dispatch_ui_runtime(
    runtime: &RuntimeDescriptor,
    layout: &StructLayoutPlan,
    dsp_state: &mut DspStateBuffer,
    ui: *mut UIGlue,
) {
    unsafe {
        let ui = &*ui;
        for item in &runtime.ui_items {
            match item {
                RuntimeUiItem::OpenTabBox { label } => {
                    if let Some(f) = ui.open_tab_box
                        && let Ok(label) = std::ffi::CString::new(label.as_str())
                    {
                        f(ui.ui_interface, label.as_ptr());
                    }
                }
                RuntimeUiItem::OpenHorizontalBox { label } => {
                    if let Some(f) = ui.open_horizontal_box
                        && let Ok(label) = std::ffi::CString::new(label.as_str())
                    {
                        f(ui.ui_interface, label.as_ptr());
                    }
                }
                RuntimeUiItem::OpenVerticalBox { label } => {
                    if let Some(f) = ui.open_vertical_box
                        && let Ok(label) = std::ffi::CString::new(label.as_str())
                    {
                        f(ui.ui_interface, label.as_ptr());
                    }
                }
                RuntimeUiItem::CloseBox => {
                    if let Some(f) = ui.close_box {
                        f(ui.ui_interface);
                    }
                }
                RuntimeUiItem::Button { label, zone } => {
                    if let Some(f) = ui.add_button
                        && let (Ok(label), Some(zone)) = (
                            std::ffi::CString::new(label.as_str()),
                            zone_ptr(dsp_state, layout, zone),
                        )
                    {
                        f(ui.ui_interface, label.as_ptr(), zone);
                    }
                }
                RuntimeUiItem::CheckButton { label, zone } => {
                    if let Some(f) = ui.add_check_button
                        && let (Ok(label), Some(zone)) = (
                            std::ffi::CString::new(label.as_str()),
                            zone_ptr(dsp_state, layout, zone),
                        )
                    {
                        f(ui.ui_interface, label.as_ptr(), zone);
                    }
                }
                RuntimeUiItem::VerticalSlider {
                    label,
                    zone,
                    init,
                    lo,
                    hi,
                    step,
                } => {
                    if let Some(f) = ui.add_vertical_slider
                        && let (Ok(label), Some(zone)) = (
                            std::ffi::CString::new(label.as_str()),
                            zone_ptr(dsp_state, layout, zone),
                        )
                    {
                        f(
                            ui.ui_interface,
                            label.as_ptr(),
                            zone,
                            *init,
                            *lo,
                            *hi,
                            *step,
                        );
                    }
                }
                RuntimeUiItem::HorizontalSlider {
                    label,
                    zone,
                    init,
                    lo,
                    hi,
                    step,
                } => {
                    if let Some(f) = ui.add_horizontal_slider
                        && let (Ok(label), Some(zone)) = (
                            std::ffi::CString::new(label.as_str()),
                            zone_ptr(dsp_state, layout, zone),
                        )
                    {
                        f(
                            ui.ui_interface,
                            label.as_ptr(),
                            zone,
                            *init,
                            *lo,
                            *hi,
                            *step,
                        );
                    }
                }
                RuntimeUiItem::NumEntry {
                    label,
                    zone,
                    init,
                    lo,
                    hi,
                    step,
                } => {
                    if let Some(f) = ui.add_num_entry
                        && let (Ok(label), Some(zone)) = (
                            std::ffi::CString::new(label.as_str()),
                            zone_ptr(dsp_state, layout, zone),
                        )
                    {
                        f(
                            ui.ui_interface,
                            label.as_ptr(),
                            zone,
                            *init,
                            *lo,
                            *hi,
                            *step,
                        );
                    }
                }
                RuntimeUiItem::HorizontalBargraph {
                    label,
                    zone,
                    lo,
                    hi,
                } => {
                    if let Some(f) = ui.add_horizontal_bargraph
                        && let (Ok(label), Some(zone)) = (
                            std::ffi::CString::new(label.as_str()),
                            zone_ptr(dsp_state, layout, zone),
                        )
                    {
                        f(ui.ui_interface, label.as_ptr(), zone, *lo, *hi);
                    }
                }
                RuntimeUiItem::VerticalBargraph {
                    label,
                    zone,
                    lo,
                    hi,
                } => {
                    if let Some(f) = ui.add_vertical_bargraph
                        && let (Ok(label), Some(zone)) = (
                            std::ffi::CString::new(label.as_str()),
                            zone_ptr(dsp_state, layout, zone),
                        )
                    {
                        f(ui.ui_interface, label.as_ptr(), zone, *lo, *hi);
                    }
                }
                RuntimeUiItem::Soundfile { label, url, zone } => {
                    if let Some(f) = ui.add_soundfile
                        && let (Ok(label), Ok(url)) = (
                            std::ffi::CString::new(label.as_str()),
                            std::ffi::CString::new(url.as_str()),
                        )
                    {
                        // Pass the address of the fSoundN pointer field in the
                        // DSP state buffer so SoundUI::addSoundfile can write
                        // the loaded Soundfile* directly into the JIT struct.
                        let zone = soundfile_zone_ptr(dsp_state, layout, zone)
                            .unwrap_or(std::ptr::null_mut());
                        f(ui.ui_interface, label.as_ptr(), url.as_ptr(), zone);
                    }
                }
                RuntimeUiItem::Declare { zone, key, value } => {
                    if let Some(f) = ui.declare
                        && let (Ok(key), Ok(value)) = (
                            std::ffi::CString::new(key.as_str()),
                            std::ffi::CString::new(value.as_str()),
                        )
                    {
                        let zone = zone
                            .as_deref()
                            .and_then(|name| zone_ptr(dsp_state, layout, name))
                            .unwrap_or(std::ptr::null_mut());
                        f(ui.ui_interface, zone, key.as_ptr(), value.as_ptr());
                    }
                }
            }
        }
    }
}

/// Resolves one UI zone name to a mutable `FAUSTFLOAT*` pointer.
///
/// The V1 C API glue expects all zones to be presented as `FAUSTFLOAT*`, even
/// when the underlying field is stored as `Int32` or `Bool`. This matches the
/// existing Faust C ABI convention used by other backends.
fn zone_ptr(
    dsp_state: &mut DspStateBuffer,
    layout: &StructLayoutPlan,
    name: &str,
) -> Option<*mut FaustFloat> {
    let field = layout.field(name)?;
    match &field.kind {
        StructFieldKind::Scalar(FirType::Float32 | FirType::FaustFloat)
        | StructFieldKind::Scalar(FirType::Int32)
        | StructFieldKind::Scalar(FirType::Bool) => Some(
            dsp_state
                .ptr_at(field.offset_bytes as usize)
                .cast::<FaustFloat>(),
        ),
        _ => None,
    }
}

/// Resolves a soundfile zone name to a `Soundfile**` pointer.
///
/// Soundfile fields (`FirType::Sound`) occupy one pointer-sized slot in the
/// `dsp*` state buffer.  The C API `add_soundfile` callback receives this
/// address as `void**` so the host (e.g. `SoundUI::addSoundfile`) can write
/// the loaded `Soundfile*` directly into the DSP state — exactly where the
/// JIT-compiled `compute` code will read it from.
fn soundfile_zone_ptr(
    dsp_state: &mut DspStateBuffer,
    layout: &StructLayoutPlan,
    name: &str,
) -> Option<*mut *mut c_void> {
    let field = layout.field(name)?;
    match &field.kind {
        StructFieldKind::Scalar(FirType::Sound) => Some(
            dsp_state
                .ptr_at(field.offset_bytes as usize)
                .cast::<*mut c_void>(),
        ),
        _ => None,
    }
}

/// Restores UI control defaults recorded from FIR `buildUserInterface`.
fn apply_control_defaults(
    dsp_state: &mut DspStateBuffer,
    layout: &StructLayoutPlan,
    runtime: &RuntimeDescriptor,
) {
    for (name, value) in &runtime.control_defaults {
        let Some(field) = layout.field(name) else {
            continue;
        };
        match &field.kind {
            StructFieldKind::Scalar(FirType::Float32) => {
                write_f32(dsp_state, field.offset_bytes as usize, *value);
            }
            // A `FaustFloat` control zone resolves to f32 or f64 depending on the
            // precision the backend compiled with; write it at the field's actual
            // width so a `-double` zone (8 bytes) is not corrupted by an f32 store.
            StructFieldKind::Scalar(FirType::FaustFloat) => {
                if field.size_bytes >= 8 {
                    write_f64(dsp_state, field.offset_bytes as usize, f64::from(*value));
                } else {
                    write_f32(dsp_state, field.offset_bytes as usize, *value);
                }
            }
            StructFieldKind::Scalar(FirType::Float64) => {
                write_f64(dsp_state, field.offset_bytes as usize, f64::from(*value));
            }
            StructFieldKind::Scalar(FirType::Int32) => {
                write_i32(dsp_state, field.offset_bytes as usize, *value as i32);
            }
            _ => {}
        }
    }
}

/// Writes one decoded runtime initializer payload into a concrete struct field.
///
/// Scalar and array payloads both use unaligned stores because the backend
/// layout contract is byte-addressed and may not align every field to the host
/// native alignment of the Rust scalar type.
fn write_field_init(
    dsp_state: &mut DspStateBuffer,
    field: &StructFieldLayout,
    init: &RuntimeFieldInit,
) {
    match init {
        RuntimeFieldInit::I32(v) => write_i32(dsp_state, field.offset_bytes as usize, *v),
        RuntimeFieldInit::I64(v) => write_i64(dsp_state, field.offset_bytes as usize, *v),
        RuntimeFieldInit::F32(v) => write_f32(dsp_state, field.offset_bytes as usize, *v),
        RuntimeFieldInit::F64(v) => write_f64(dsp_state, field.offset_bytes as usize, *v),
        RuntimeFieldInit::Bool(v) => write_bool(dsp_state, field.offset_bytes as usize, *v),
        RuntimeFieldInit::I32Array(values) => {
            let base = dsp_state.field_ptr(field);
            for (i, v) in values.iter().enumerate() {
                unsafe { std::ptr::write_unaligned(base.add(i * 4).cast::<i32>(), *v) };
            }
        }
        RuntimeFieldInit::F32Array(values) => {
            let base = dsp_state.field_ptr(field);
            for (i, v) in values.iter().enumerate() {
                unsafe { std::ptr::write_unaligned(base.add(i * 4).cast::<f32>(), *v) };
            }
        }
        RuntimeFieldInit::F64Array(values) => {
            let base = dsp_state.field_ptr(field);
            for (i, v) in values.iter().enumerate() {
                unsafe { std::ptr::write_unaligned(base.add(i * 8).cast::<f64>(), *v) };
            }
        }
    }
}

/// Writes one `i32` at byte offset `offset` inside the `dsp*` state buffer.
fn write_i32(dsp_state: &mut DspStateBuffer, offset: usize, value: i32) {
    unsafe { std::ptr::write_unaligned(dsp_state.ptr_at(offset).cast::<i32>(), value) };
}

/// Writes one `i64` at byte offset `offset` inside the `dsp*` state buffer.
fn write_i64(dsp_state: &mut DspStateBuffer, offset: usize, value: i64) {
    unsafe { std::ptr::write_unaligned(dsp_state.ptr_at(offset).cast::<i64>(), value) };
}

/// Writes one `f32` at byte offset `offset` inside the `dsp*` state buffer.
fn write_f32(dsp_state: &mut DspStateBuffer, offset: usize, value: f32) {
    unsafe { std::ptr::write_unaligned(dsp_state.ptr_at(offset).cast::<f32>(), value) };
}

/// Writes one `f64` at byte offset `offset` inside the `dsp*` state buffer.
fn write_f64(dsp_state: &mut DspStateBuffer, offset: usize, value: f64) {
    unsafe { std::ptr::write_unaligned(dsp_state.ptr_at(offset).cast::<f64>(), value) };
}

/// Writes one boolean as the backend's byte-sized `0/1` storage convention.
fn write_bool(dsp_state: &mut DspStateBuffer, offset: usize, value: bool) {
    unsafe { std::ptr::write_unaligned(dsp_state.ptr_at(offset).cast::<u8>(), u8::from(value)) };
}

/// Reconstructs a typed callable `compute` function pointer from one finalized address.
///
/// The address originates from [`codegen::backends::cranelift::JitDspModule`]
/// after Cranelift finalization, so the transmute is sound as long as factory
/// and instance code keep the ABI contract in sync.
fn compute_fn_from_addr(addr: usize) -> Option<ComputeFn> {
    if addr == 0 {
        None
    } else {
        // SAFETY: address comes from finalized Cranelift symbol for `compute` with
        // known ABI/signature in this backend module.
        Some(unsafe { std::mem::transmute::<usize, ComputeFn>(addr) })
    }
}

fn instance_constants_fn_from_addr(addr: usize) -> Option<InstanceConstantsFn> {
    if addr == 0 {
        None
    } else {
        Some(unsafe { std::mem::transmute::<usize, InstanceConstantsFn>(addr) })
    }
}

fn instance_clear_fn_from_addr(addr: usize) -> Option<InstanceClearFn> {
    if addr == 0 {
        None
    } else {
        Some(unsafe { std::mem::transmute::<usize, InstanceClearFn>(addr) })
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::{Layout, alloc_zeroed, dealloc};
    use std::collections::HashMap;
    use std::ffi::{CStr, CString, c_char, c_void};

    use super::{
        buildUserInterfaceCCraneliftDSPInstance, cloneCCraneliftDSPInstance,
        computeCCraneliftDSPInstance, createCCraneliftDSPInstance, deleteCCraneliftDSPInstance,
        getNumInputsCCraneliftDSPInstance, getNumOutputsCCraneliftDSPInstance,
        getSampleRateCCraneliftDSPInstance, initCCraneliftDSPInstance, instance_status,
        metadataCCraneliftDSPInstance,
    };
    use crate::factory::{
        createCCraneliftDSPFactoryFromFile, createCCraneliftDSPFactoryFromString,
        deleteCCraneliftDSPFactory, freeCMemory, readCCraneliftDSPFactoryFromBitcode,
        setCCraneliftMemoryManager, writeCCraneliftDSPFactoryToBitcode,
    };
    use crate::types::{FaustFloat, MetaGlue, UIGlue};
    use ffi_common::{FAUST_MEMORY_MANAGER_ABI_VERSION, FaustMemoryManager, FaustMemoryType};

    fn workspace_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root")
    }

    fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
        let start = source.find(signature).expect("function signature");
        let open = source[start..].find('{').expect("function body open") + start;
        let mut depth = 0_i32;
        for (offset, ch) in source[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[open..=open + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("function body should close");
    }

    #[test]
    fn instance_status_is_stable() {
        let _guard = crate::test_serial_guard();
        assert_eq!(instance_status(), "cranelift-ffi instance runtime");
    }

    #[test]
    fn lifecycle_scaffold_matches_faust_cpp_backend_contract() {
        let source = include_str!("instance.rs");
        let init_body = function_body(
            source,
            "pub unsafe extern \"C\" fn initCCraneliftDSPInstance",
        );
        let init_class_i = init_body
            .find("class_init_instance(dsp, sample_rate)")
            .expect("init should call classInit");
        let init_instance_i = init_body
            .find("instanceInitCCraneliftDSPInstance(dsp, sample_rate);")
            .expect("init should call instanceInit");
        assert!(
            init_class_i < init_instance_i,
            "init must call classInit before instanceInit"
        );

        let instance_init_body = function_body(
            source,
            "pub unsafe extern \"C\" fn instanceInitCCraneliftDSPInstance",
        );
        assert!(
            !instance_init_body.contains("class_init_instance(dsp,"),
            "instanceInit must not call classInit"
        );
        let constants_i = instance_init_body
            .find("instanceConstantsCCraneliftDSPInstance(dsp, sample_rate);")
            .expect("instanceInit should call instanceConstants");
        let reset_i = instance_init_body
            .find("instanceResetUserInterfaceCCraneliftDSPInstance(dsp);")
            .expect("instanceInit should call instanceResetUserInterface");
        let clear_i = instance_init_body
            .find("instanceClearCCraneliftDSPInstance(dsp);")
            .expect("instanceInit should call instanceClear");
        assert!(
            constants_i < reset_i && reset_i < clear_i,
            "instanceInit must call constants, resetUI, clear in order"
        );

        let clear_body = function_body(
            source,
            "pub unsafe extern \"C\" fn instanceClearCCraneliftDSPInstance",
        );
        assert!(
            !clear_body.contains("clear_runtime_state"),
            "instanceClear must delegate to the compiled FIR body, not a runtime-side clear policy"
        );
    }

    #[derive(Default)]
    struct TestMemoryManager {
        described: usize,
        infos: usize,
        allocations: HashMap<usize, Layout>,
        destroys: usize,
    }

    unsafe extern "C" fn test_memory_begin(context: *mut c_void, count: usize) {
        unsafe {
            let manager = &mut *context.cast::<TestMemoryManager>();
            manager.described = count;
            manager.infos = 0;
        }
    }

    unsafe extern "C" fn test_memory_info(
        context: *mut c_void,
        _name: *const c_char,
        _typ: FaustMemoryType,
        _elements: usize,
        _size: usize,
        _alignment: usize,
        _reads: u64,
        _writes: u64,
    ) {
        unsafe { (&mut *context.cast::<TestMemoryManager>()).infos += 1 };
    }

    unsafe extern "C" fn test_memory_end(_context: *mut c_void) {}

    unsafe extern "C" fn test_memory_allocate(
        context: *mut c_void,
        size: usize,
        alignment: usize,
    ) -> *mut c_void {
        unsafe {
            let layout = Layout::from_size_align(size.max(1), alignment.max(1)).unwrap();
            let pointer = alloc_zeroed(layout);
            if !pointer.is_null() {
                (&mut *context.cast::<TestMemoryManager>())
                    .allocations
                    .insert(pointer as usize, layout);
            }
            pointer.cast()
        }
    }

    unsafe extern "C" fn test_memory_destroy(
        context: *mut c_void,
        address: *mut c_void,
        _size: usize,
        _alignment: usize,
    ) {
        unsafe {
            let manager = &mut *context.cast::<TestMemoryManager>();
            let layout = manager
                .allocations
                .remove(&(address as usize))
                .expect("destroy matches one manager allocation");
            dealloc(address.cast(), layout);
            manager.destroys += 1;
        }
    }

    #[test]
    fn mem0_factory_requires_binding_and_owns_deep_clone_allocations() {
        let _guard = crate::test_serial_guard();
        let name = CString::new("mem0_delay").unwrap();
        let source = CString::new("process = rdtable(8, 0.25, int(_)) : @(7);").unwrap();
        let mem0 = CString::new("-mem0").unwrap();
        let argv = [mem0.as_ptr()];
        let mut error = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                source.as_ptr(),
                argv.len() as i32,
                argv.as_ptr(),
                error.as_mut_ptr(),
                0,
            )
        };
        assert!(!factory.is_null(), "{}", unsafe {
            CStr::from_ptr(error.as_ptr()).to_string_lossy()
        });
        assert!(unsafe { createCCraneliftDSPInstance(factory) }.is_null());

        let mut owner = TestMemoryManager::default();
        let table = FaustMemoryManager {
            abi_version: FAUST_MEMORY_MANAGER_ABI_VERSION,
            struct_size: std::mem::size_of::<FaustMemoryManager>(),
            context: (&mut owner as *mut TestMemoryManager).cast(),
            begin: Some(test_memory_begin),
            info: Some(test_memory_info),
            end: Some(test_memory_end),
            allocate: Some(test_memory_allocate),
            destroy: Some(test_memory_destroy),
        };
        assert!(unsafe { setCCraneliftMemoryManager(factory, &table, error.as_mut_ptr()) });
        assert_eq!(owner.infos, owner.described);
        assert!(
            owner.described >= 3,
            "object, generated table, and delay buffer are described"
        );

        let dsp = unsafe { createCCraneliftDSPInstance(factory) };
        assert!(!dsp.is_null());
        unsafe { initCCraneliftDSPInstance(dsp, 48_000) };
        let cloned = unsafe { cloneCCraneliftDSPInstance(dsp) };
        assert!(!cloned.is_null());
        assert!(
            owner.allocations.len() >= 4,
            "class table plus object and buffer per instance"
        );

        unsafe {
            deleteCCraneliftDSPInstance(cloned);
            deleteCCraneliftDSPInstance(dsp);
            assert!(deleteCCraneliftDSPFactory(factory));
        }
        assert!(owner.allocations.is_empty());
        assert!(owner.destroys >= 4);
    }

    #[test]
    fn serialized_mem0_factory_retains_mode_but_requires_a_fresh_binding() {
        let _guard = crate::test_serial_guard();
        let name = c"mem0_serialized";
        let source = c"process = rdtable(8, 0.25, int(_)) : @(7);";
        let mem0 = c"-mem0";
        let argv = [mem0.as_ptr()];
        let mut error = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                source.as_ptr(),
                argv.len() as i32,
                argv.as_ptr(),
                error.as_mut_ptr(),
                3,
            )
        };
        assert!(!factory.is_null(), "{}", unsafe {
            CStr::from_ptr(error.as_ptr()).to_string_lossy()
        });

        let mut first_owner = TestMemoryManager::default();
        let first_table = test_memory_table(&mut first_owner);
        assert!(unsafe { setCCraneliftMemoryManager(factory, &first_table, error.as_mut_ptr()) });
        assert!(
            first_owner.allocations.is_empty(),
            "binding describes but must not allocate"
        );

        let payload = unsafe { writeCCraneliftDSPFactoryToBitcode(factory) };
        assert!(!payload.is_null());
        let payload_text = unsafe { CStr::from_ptr(payload) }.to_string_lossy();
        assert!(payload_text.contains("arg0=-mem0"));
        assert!(payload_text.contains("opt_level=3"));
        assert!(unsafe { deleteCCraneliftDSPFactory(factory) });
        assert!(first_owner.allocations.is_empty());

        let restored = unsafe {
            readCCraneliftDSPFactoryFromBitcode(payload.cast_const(), error.as_mut_ptr())
        };
        unsafe { freeCMemory(payload.cast()) };
        assert!(!restored.is_null(), "{}", unsafe {
            CStr::from_ptr(error.as_ptr()).to_string_lossy()
        });
        assert!(
            unsafe { createCCraneliftDSPInstance(restored) }.is_null(),
            "serialized factories never retain host callback pointers"
        );

        let mut second_owner = TestMemoryManager::default();
        let second_table = test_memory_table(&mut second_owner);
        assert!(unsafe { setCCraneliftMemoryManager(restored, &second_table, error.as_mut_ptr()) });
        let dsp = unsafe { createCCraneliftDSPInstance(restored) };
        assert!(!dsp.is_null());
        unsafe {
            initCCraneliftDSPInstance(dsp, 48_000);
            deleteCCraneliftDSPInstance(dsp);
            assert!(deleteCCraneliftDSPFactory(restored));
        }
        assert!(second_owner.allocations.is_empty());
        assert!(second_owner.destroys >= 3);
    }

    fn test_memory_table(owner: &mut TestMemoryManager) -> FaustMemoryManager {
        FaustMemoryManager {
            abi_version: FAUST_MEMORY_MANAGER_ABI_VERSION,
            struct_size: std::mem::size_of::<FaustMemoryManager>(),
            context: (owner as *mut TestMemoryManager).cast(),
            begin: Some(test_memory_begin),
            info: Some(test_memory_info),
            end: Some(test_memory_end),
            allocate: Some(test_memory_allocate),
            destroy: Some(test_memory_destroy),
        }
    }

    fn render_delay_with_memory_mode(managed: bool, opt_level: i32) -> Vec<f32> {
        let name = CString::new(format!(
            "delay_{}_opt{opt_level}",
            if managed { "mem0" } else { "ordinary" }
        ))
        .unwrap();
        let source = CString::new("process = _ : @(7);").unwrap();
        let mem0 = CString::new("-mem0").unwrap();
        let argv = [mem0.as_ptr()];
        let mut error = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                source.as_ptr(),
                i32::from(managed),
                if managed {
                    argv.as_ptr()
                } else {
                    std::ptr::null()
                },
                error.as_mut_ptr(),
                opt_level,
            )
        };
        assert!(!factory.is_null(), "{}", unsafe {
            CStr::from_ptr(error.as_ptr()).to_string_lossy()
        });
        assert!(unsafe { (*factory).compute_body_lowered });

        let mut owner = TestMemoryManager::default();
        if managed {
            let table = FaustMemoryManager {
                abi_version: FAUST_MEMORY_MANAGER_ABI_VERSION,
                struct_size: std::mem::size_of::<FaustMemoryManager>(),
                context: (&mut owner as *mut TestMemoryManager).cast(),
                begin: Some(test_memory_begin),
                info: Some(test_memory_info),
                end: Some(test_memory_end),
                allocate: Some(test_memory_allocate),
                destroy: Some(test_memory_destroy),
            };
            assert!(unsafe { setCCraneliftMemoryManager(factory, &table, error.as_mut_ptr()) });
        }
        let dsp = unsafe { createCCraneliftDSPInstance(factory) };
        assert!(!dsp.is_null());
        unsafe { initCCraneliftDSPInstance(dsp, 48_000) };

        let frames = 32;
        let mut input = vec![0.0_f32; frames];
        input[0] = 1.0;
        let mut output = vec![0.0_f32; frames];
        let mut inputs = [input.as_mut_ptr()];
        let mut outputs = [output.as_mut_ptr()];
        unsafe {
            computeCCraneliftDSPInstance(
                dsp,
                frames as i32,
                inputs.as_mut_ptr(),
                outputs.as_mut_ptr(),
            );
            deleteCCraneliftDSPInstance(dsp);
            assert!(deleteCCraneliftDSPFactory(factory));
        }
        assert!(owner.allocations.is_empty());
        output
    }

    #[test]
    fn mem0_delay_matches_ordinary_at_minimum_and_maximum_optimization() {
        let _guard = crate::test_serial_guard();
        let ordinary_none = render_delay_with_memory_mode(false, 0);
        let ordinary_max = render_delay_with_memory_mode(false, 3);
        let managed_none = render_delay_with_memory_mode(true, 0);
        let managed_max = render_delay_with_memory_mode(true, 3);
        assert_eq!(ordinary_none, ordinary_max);
        assert_eq!(ordinary_none, managed_none);
        assert_eq!(ordinary_none, managed_max);
        assert_eq!(ordinary_none[7], 1.0);
    }

    unsafe extern "C" fn capture_meta(ctx: *mut c_void, key: *const c_char, value: *const c_char) {
        unsafe {
            let out = &mut *(ctx.cast::<Vec<(String, String)>>());
            out.push((
                CStr::from_ptr(key).to_str().unwrap().to_owned(),
                CStr::from_ptr(value).to_str().unwrap().to_owned(),
            ));
        }
    }

    #[test]
    fn instance_lifecycle_scaffold_roundtrip() {
        let _guard = crate::test_serial_guard();
        let name = CString::new("demo").unwrap();
        let src = CString::new("process = _;").unwrap();
        let mut err = [0_i8; 4096];

        let factory = unsafe {
            createCCraneliftDSPFactoryFromString(
                name.as_ptr(),
                src.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!factory.is_null());

        let dsp = unsafe { createCCraneliftDSPInstance(factory) };
        assert!(!dsp.is_null());
        assert_eq!(unsafe { getNumInputsCCraneliftDSPInstance(dsp) }, 1);
        assert_eq!(unsafe { getNumOutputsCCraneliftDSPInstance(dsp) }, 1);
        assert_eq!(unsafe { getSampleRateCCraneliftDSPInstance(dsp) }, 0);

        unsafe { initCCraneliftDSPInstance(dsp, 48_000) };
        assert_eq!(unsafe { getSampleRateCCraneliftDSPInstance(dsp) }, 48_000);

        let clone = unsafe { cloneCCraneliftDSPInstance(dsp) };
        assert!(!clone.is_null());
        assert_eq!(unsafe { getSampleRateCCraneliftDSPInstance(clone) }, 48_000);

        let mut ui = UIGlue {
            ui_interface: std::ptr::null_mut(),
            open_tab_box: None,
            open_horizontal_box: None,
            open_vertical_box: None,
            close_box: None,
            add_button: None,
            add_check_button: None,
            add_vertical_slider: None,
            add_horizontal_slider: None,
            add_num_entry: None,
            add_horizontal_bargraph: None,
            add_vertical_bargraph: None,
            add_soundfile: None,
            declare: None,
        };
        unsafe { buildUserInterfaceCCraneliftDSPInstance(dsp, &mut ui) };

        let mut entries: Vec<(String, String)> = Vec::new();
        let mut meta = MetaGlue {
            meta_interface: (&mut entries as *mut Vec<(String, String)>).cast::<c_void>(),
            declare: Some(capture_meta),
        };
        unsafe { metadataCCraneliftDSPInstance(dsp, &mut meta) };
        assert!(!entries.is_empty());

        let mut in_buf = [0.0_f32; 8];
        let mut out_buf = [0.0_f32; 8];
        let mut inputs: [*mut FaustFloat; 1] = [in_buf.as_mut_ptr()];
        let mut outputs: [*mut FaustFloat; 1] = [out_buf.as_mut_ptr()];
        unsafe { computeCCraneliftDSPInstance(dsp, 8, inputs.as_mut_ptr(), outputs.as_mut_ptr()) };
        assert!(out_buf.iter().any(|x| x.is_finite()));

        unsafe {
            deleteCCraneliftDSPInstance(clone);
            deleteCCraneliftDSPInstance(dsp);
            assert!(deleteCCraneliftDSPFactory(factory));
        }
    }

    #[test]
    fn runtime_rep38_produces_non_silent_output() {
        let _guard = crate::test_serial_guard();
        let case = workspace_root().join("tests/corpus/rep_38_sine_phasor.dsp");
        assert_runtime_case_non_silent(&case);
    }

    #[test]
    fn runtime_rep55_produces_non_silent_output() {
        let _guard = crate::test_serial_guard();
        let case = workspace_root().join("tests/corpus/rep_55_sine_phasor_echo_feedback.dsp");
        assert_runtime_case_non_silent(&case);
    }

    fn assert_runtime_case_non_silent(case: &std::path::Path) {
        let case_c = CString::new(case.to_string_lossy().as_bytes()).expect("path CString");
        let mut err = [0_i8; 4096];
        let factory = unsafe {
            createCCraneliftDSPFactoryFromFile(
                case_c.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
                1,
            )
        };
        assert!(!factory.is_null(), "factory failed: {:?}", unsafe {
            CStr::from_ptr(err.as_ptr()).to_str().ok()
        });

        let dsp = unsafe { createCCraneliftDSPInstance(factory) };
        assert!(!dsp.is_null());
        unsafe { initCCraneliftDSPInstance(dsp, 48_000) };

        let num_inputs = unsafe { getNumInputsCCraneliftDSPInstance(dsp) }.max(0) as usize;
        let num_outputs = unsafe { getNumOutputsCCraneliftDSPInstance(dsp) }.max(0) as usize;
        let frames = 256usize;
        let mut input_buffers = vec![vec![0.0_f32; frames]; num_inputs];
        let mut output_buffers = vec![vec![0.0_f32; frames]; num_outputs.max(1)];
        let mut input_ptrs: Vec<*mut FaustFloat> = input_buffers
            .iter_mut()
            .map(|buf| buf.as_mut_ptr())
            .collect();
        let mut output_ptrs: Vec<*mut FaustFloat> = output_buffers
            .iter_mut()
            .map(|buf| buf.as_mut_ptr())
            .collect();

        unsafe {
            computeCCraneliftDSPInstance(
                dsp,
                frames as i32,
                if input_ptrs.is_empty() {
                    std::ptr::null_mut()
                } else {
                    input_ptrs.as_mut_ptr()
                },
                output_ptrs.as_mut_ptr(),
            );
        }

        let non_silent = output_buffers
            .iter()
            .flat_map(|buf| buf.iter())
            .any(|sample| sample.abs() > 1.0e-6);
        assert!(non_silent, "{} output stayed silent", case.display());

        unsafe {
            deleteCCraneliftDSPInstance(dsp);
            assert!(deleteCCraneliftDSPFactory(factory));
        }
    }
}
