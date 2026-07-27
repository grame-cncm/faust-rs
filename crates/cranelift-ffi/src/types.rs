//! Opaque FFI types and allocation helpers for `cranelift_dsp`.
//!
//! This module provides the runtime ownership layer used by exported C ABI
//! functions:
//! - heap-owned opaque factory/instance pointers,
//! - per-instance aligned `dsp*` state buffers,
//! - shared callback glue structs (`UIGlue`, `MetaGlue`).
//!
//! # API mapping status
//! - External compatibility surface: `adapted` during scaffolding.
//! - Naming and V1 family coverage are driven by
//!   `porting/cranelift-dsp-ffi-parity-matrix-en.md`.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ffi::c_char;
use std::ptr::NonNull;

use codegen::backends::cranelift::JitDspModule;

use crate::runtime::RuntimeDescriptor;

/// `FAUSTFLOAT` used by the exported C API (v1 planned default).
pub type FaustFloat = f32;

/// Shared UI callback table (`UIGlue`) for Faust C FFI backends.
pub use ffi_common::UIGlue;

/// Shared metadata callback table (`MetaGlue`) for Faust C FFI backends.
pub use ffi_common::MetaGlue;

/// Opaque Cranelift DSP factory wrapper exported as `cranelift_dsp_factory*`.
///
/// This scaffold stores lightweight metadata so that the C ABI can already be
/// exercised end-to-end (factory create -> instance create -> lifecycle calls)
/// before real Cranelift code generation is connected.
pub struct CraneliftDspFactory {
    /// Display name (`declare name`, file stem, or `name_app` fallback).
    pub(crate) name: String,
    /// Factory hash key used by the cache layer.
    pub(crate) sha_key: String,
    /// Expanded DSP source text (or source marker for file-based creation).
    pub(crate) dsp_code: String,
    /// Compiled options summary string.
    pub(crate) compile_options: String,
    /// JSON UI/metadata payload exposed by the C API query family.
    pub(crate) json: String,
    /// Whether `dsp_code` contains canonical Faust source text.
    ///
    /// When this is `false`, bitcode persistence currently cannot rebuild a
    /// runnable factory because no FIR-text parser exists yet in this port.
    pub(crate) source_is_faust: bool,
    /// Canonical source/module name used for source-string compilation.
    pub(crate) source_name: String,
    /// Original FFI compile `argv` options.
    pub(crate) compile_argv: Vec<String>,
    /// Original FFI compile optimization level.
    pub(crate) opt_level: i32,
    /// Compiled Cranelift JIT module (present for real file/string compilation paths).
    pub(crate) compiled_jit: Option<JitDspModule>,
    /// Native runtime descriptor derived from FIR for UI/meta/state handling.
    pub(crate) runtime: RuntimeDescriptor,
    /// Whether the backend lowered the FIR `compute` body (vs stub fallback).
    pub(crate) compute_body_lowered: bool,
    /// Audio input count.
    pub(crate) num_inputs: usize,
    /// Audio output count.
    pub(crate) num_outputs: usize,
}

/// Opaque Cranelift DSP instance wrapper exported as `cranelift_dsp*`.
///
/// The instance owns exactly one backend `dsp*` memory block and reuses the
/// parent factory's compiled JIT module/runtime descriptor.
pub struct CraneliftDspInstance {
    /// Non-owning pointer to the parent factory (same C API lifetime contract
    /// as `llvm_dsp`/`interpreter_dsp`).
    pub(crate) factory: *const CraneliftDspFactory,
    /// Current sample rate configured through `init`/`instance*`.
    pub(crate) sample_rate: i32,
    /// Whether `init()` has been called.
    pub(crate) initialized: bool,
    /// Number of `compute()` calls observed.
    pub(crate) cycle: usize,
    /// Owned backend `dsp*` state allocation passed to the JIT `compute` entry.
    pub(crate) dsp_state: DspStateBuffer,
}

// SAFETY: Instances are opaque and not internally synchronized. The C API
// contract does not require shared concurrent access to the same instance.
unsafe impl Send for CraneliftDspInstance {}

/// Owned, aligned state buffer used as the Cranelift backend `dsp*` instance memory.
///
/// The allocation policy mirrors the backend layout contract:
/// - size and alignment come from [`codegen::backends::cranelift::StructLayoutPlan`],
/// - bytes are zero-initialized on allocation,
/// - the memory is released when the instance is dropped.
#[derive(Debug)]
pub(crate) struct DspStateBuffer {
    ptr: Option<NonNull<u8>>,
    layout: Option<Layout>,
}

impl DspStateBuffer {
    /// Allocates one zeroed state buffer.
    ///
    /// # Parameters
    /// - `size`: requested byte size (`0` is allowed and produces an empty buffer)
    /// - `align`: requested byte alignment (`0` is treated as `1`)
    pub(crate) fn new(size: usize, align: usize) -> Result<Self, String> {
        // Keep a non-null allocation even for empty logical layouts so runtime
        // code can always pass a stable `dsp*` pointer to JIT entry points.
        let size = size.max(1);
        let align = align.max(1);
        let layout = Layout::from_size_align(size, align)
            .map_err(|e| format!("invalid DSP state layout size={size} align={align}: {e}"))?;
        // SAFETY: layout is valid and non-zero-sized; zeroed allocation is intentional.
        let raw = unsafe { alloc_zeroed(layout) };
        let ptr = NonNull::new(raw).ok_or_else(|| {
            format!("failed to allocate Cranelift DSP state ({size} bytes, align {align})")
        })?;
        Ok(Self {
            ptr: Some(ptr),
            layout: Some(layout),
        })
    }

    /// Returns the mutable base pointer to pass as `dsp*` to JIT code.
    ///
    /// For empty buffers this returns null.
    #[must_use]
    pub(crate) fn as_mut_ptr(&self) -> *mut u8 {
        self.ptr.map_or(std::ptr::null_mut(), NonNull::as_ptr)
    }

    #[must_use]
    /// Returns a typed offset pointer inside the allocated `dsp*` state block.
    ///
    /// The caller is responsible for keeping accesses within bounds dictated by
    /// the matching [`StructLayoutPlan`](codegen::backends::cranelift::StructLayoutPlan).
    pub(crate) fn ptr_at(&self, offset: usize) -> *mut u8 {
        self.as_mut_ptr().wrapping_add(offset)
    }

    /// Clones the allocation and bytes into a new owned buffer.
    pub(crate) fn deep_clone(&self) -> Result<Self, String> {
        let (Some(src_ptr), Some(layout)) = (self.ptr, self.layout) else {
            return Self::new(0, 1);
        };
        let cloned = Self::new(layout.size(), layout.align())?;
        if let (Some(dst_ptr), Some(dst_layout)) = (cloned.ptr, cloned.layout) {
            debug_assert_eq!(layout, dst_layout);
            // SAFETY: source/destination are valid non-overlapping buffers of equal size.
            unsafe {
                std::ptr::copy_nonoverlapping(src_ptr.as_ptr(), dst_ptr.as_ptr(), layout.size());
            }
        }
        Ok(cloned)
    }
}

impl Drop for DspStateBuffer {
    fn drop(&mut self) {
        let (Some(ptr), Some(layout)) = (self.ptr.take(), self.layout.take()) else {
            return;
        };
        // SAFETY: pointer/layout pair originated from `alloc_zeroed` with same layout.
        unsafe { dealloc(ptr.as_ptr(), layout) };
    }
}

/// Boxes a Cranelift factory and returns an owning raw pointer.
#[must_use]
pub(crate) fn alloc_factory(factory: CraneliftDspFactory) -> *mut CraneliftDspFactory {
    ffi_common::alloc_opaque(factory)
}

/// Frees a factory pointer previously returned by [`alloc_factory`].
///
/// # Safety
/// `ptr` must be a valid pointer returned by [`alloc_factory`], and must not be
/// used after this call.
pub(crate) unsafe fn free_factory(ptr: *mut CraneliftDspFactory) {
    unsafe { ffi_common::free_opaque(ptr) }
}

/// Boxes a Cranelift instance and returns an owning raw pointer.
#[must_use]
pub(crate) fn alloc_instance(
    factory: *const CraneliftDspFactory,
    sample_rate: i32,
    dsp_state: DspStateBuffer,
) -> *mut CraneliftDspInstance {
    ffi_common::alloc_opaque(CraneliftDspInstance {
        factory,
        sample_rate,
        initialized: false,
        cycle: 0,
        dsp_state,
    })
}

/// Frees an instance pointer previously returned by [`alloc_instance`].
///
/// # Safety
/// `ptr` must be a valid pointer returned by [`alloc_instance`], and must not
/// be used after this call.
pub(crate) unsafe fn free_instance(ptr: *mut CraneliftDspInstance) {
    unsafe { ffi_common::free_opaque(ptr) }
}

/// Allocates a heap C string that can be returned through the C ABI.
///
/// Embedded NUL bytes are replaced by the textual sequence `\\0`.
#[must_use]
pub(crate) fn alloc_c_string(s: &str) -> *mut c_char {
    ffi_common::alloc_c_string(s)
}

#[cfg(test)]
mod tests {
    use crate::runtime::RuntimeDescriptor;

    use super::{
        CraneliftDspFactory, CraneliftDspInstance, DspStateBuffer, MetaGlue, UIGlue,
        alloc_c_string, alloc_factory, alloc_instance, free_factory, free_instance,
    };

    #[test]
    fn scaffold_types_are_constructible_in_type_system() {
        let _ = std::mem::size_of::<CraneliftDspFactory>();
        let _ = std::mem::size_of::<CraneliftDspInstance>();
        let _ = std::mem::size_of::<UIGlue>();
        let _ = std::mem::size_of::<MetaGlue>();
    }

    #[test]
    fn alloc_and_free_helpers_roundtrip() {
        let factory = alloc_factory(CraneliftDspFactory {
            name: "n".into(),
            sha_key: "sha".into(),
            dsp_code: "process=_;".into(),
            compile_options: "-vec 0".into(),
            json: "{}".into(),
            source_is_faust: true,
            source_name: "n".into(),
            compile_argv: vec!["-vec".into()],
            opt_level: 0,
            compiled_jit: None,
            runtime: RuntimeDescriptor::default(),
            compute_body_lowered: false,
            num_inputs: 1,
            num_outputs: 1,
        });
        let instance = alloc_instance(
            factory,
            48_000,
            DspStateBuffer::new(32, 8).expect("test allocation"),
        );
        let s = alloc_c_string("ok");
        unsafe {
            ffi_common::free_c_string(s);
            free_instance(instance);
            free_factory(factory);
        }
    }
}
