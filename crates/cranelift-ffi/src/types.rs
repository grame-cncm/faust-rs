//! Opaque FFI types owned by the Cranelift factory cache.
//!
//! This module provides the runtime ownership layer used by exported C ABI
//! functions:
//! - cache-owned opaque factory/instance pointers,
//! - per-instance aligned `dsp*` state buffers,
//! - shared callback glue structs (`UIGlue`, `MetaGlue`).
//!
//! # API mapping status
//! - External compatibility surface: `adapted` during scaffolding.
//! - Naming and V1 family coverage are driven by
//!   `porting/cranelift-dsp-ffi-parity-matrix-en.md`.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ffi::{CString, c_char, c_void};
use std::ptr::NonNull;
use std::sync::Mutex;

use codegen::backends::cranelift::{
    JitDspModule, StaticMemorySlot, StructFieldKind, StructLayoutPlan,
};
use codegen::memory_layout::{
    Mem0Analysis, MemoryRole, MemoryScope, MemoryType, MemoryZone, MemoryZoneId,
};
use ffi_common::{
    FaustMemoryManager, FaustMemoryManagerAllocateFn, FaustMemoryManagerBeginFn,
    FaustMemoryManagerDestroyFn, FaustMemoryManagerEndFn, FaustMemoryManagerInfoFn,
    FaustMemoryType,
};

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
    /// Custom-manager binding and class allocation lifecycle.
    ///
    /// This field precedes `compiled_jit` so its drop clears and destroys all
    /// class allocations before the JIT data slots cease to exist.
    pub(crate) memory_state: Mutex<FactoryMemoryState>,
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

impl Drop for CraneliftDspInstance {
    fn drop(&mut self) {
        if let Some(factory) = unsafe { self.factory.as_ref() }
            && let Ok(mut state) = factory.memory_state.lock()
        {
            state.live_instances = state.live_instances.saturating_sub(1);
        }
    }
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
///
/// Under `-mem0` this is an adapted Rust-native ownership boundary: Faust C++
/// has no Cranelift backend oracle. The host manager owns the logical JIT state
/// object and external payloads, while this Rust value retains only their drop
/// metadata inside the opaque cache-owned instance wrapper.
pub(crate) struct DspStateBuffer {
    // Declared before `main` so Rust drops the external allocation owner first.
    // Its custom `Drop` then releases individual buffers in reverse order.
    external: ExternalAllocations,
    main: OwnedAllocation,
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
            external: ExternalAllocations::default(),
            main: OwnedAllocation {
                ptr,
                size,
                align,
                owner: AllocationOwner::Rust(layout),
            },
        })
    }

    /// Allocates the object and every non-empty instance zone through `manager`.
    pub(crate) unsafe fn new_managed(
        layout: &StructLayoutPlan,
        analysis: &Mem0Analysis,
        manager: MemoryManagerBinding,
    ) -> Result<Self, String> {
        let object = analysis
            .memory_layout
            .zones
            .iter()
            .find(|zone| zone.role == MemoryRole::DspObject)
            .ok_or_else(|| "mem0 layout has no DSP object zone".to_owned())?;
        if object.size_bytes != u64::from(layout.size_bytes())
            || object.alignment != u64::from(layout.align_bytes())
        {
            return Err(format!(
                "mem0 DSP object/layout mismatch: analysis={}/{} JIT={}/{}",
                object.size_bytes,
                object.alignment,
                layout.size_bytes(),
                layout.align_bytes()
            ));
        }
        let main = unsafe { OwnedAllocation::manager(manager, object)? };
        let mut result = Self {
            external: ExternalAllocations::default(),
            main,
        };
        for zone in analysis.memory_layout.zones.iter().filter(|zone| {
            zone.runtime_allocated
                && zone.scope == MemoryScope::Instance
                && zone.role == MemoryRole::InstanceBuffer
        }) {
            let field = layout
                .field(&zone.name)
                .ok_or_else(|| format!("mem0 instance zone `{}` has no JIT field", zone.name))?;
            let StructFieldKind::ExternalTable { zone_id, .. } = field.kind else {
                return Err(format!(
                    "mem0 instance zone `{}` is not an external JIT table",
                    zone.name
                ));
            };
            if zone_id != zone.id {
                return Err(format!("mem0 zone identity mismatch for `{}`", zone.name));
            }
            let allocation = unsafe { OwnedAllocation::manager(manager, zone)? };
            unsafe {
                result
                    .main
                    .ptr
                    .as_ptr()
                    .add(field.offset_bytes as usize)
                    .cast::<*mut u8>()
                    .write_unaligned(allocation.ptr.as_ptr());
            }
            result.external.0.push(ExternalAllocation {
                zone_id: zone.id,
                slot_offset: field.offset_bytes as usize,
                allocation,
            });
        }
        Ok(result)
    }

    /// Returns the mutable base pointer to pass as `dsp*` to JIT code.
    ///
    /// For empty buffers this returns null.
    #[must_use]
    pub(crate) fn as_mut_ptr(&self) -> *mut u8 {
        self.main.ptr.as_ptr()
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
        let mut cloned = Self {
            external: ExternalAllocations::default(),
            main: unsafe { self.main.allocate_like()? },
        };
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.main.ptr.as_ptr(),
                cloned.main.ptr.as_ptr(),
                self.main.size,
            );
        }
        for source in &self.external.0 {
            let allocation = unsafe { source.allocation.allocate_like()? };
            unsafe {
                std::ptr::copy_nonoverlapping(
                    source.allocation.ptr.as_ptr(),
                    allocation.ptr.as_ptr(),
                    source.allocation.size,
                );
                cloned
                    .main
                    .ptr
                    .as_ptr()
                    .add(source.slot_offset)
                    .cast::<*mut u8>()
                    .write_unaligned(allocation.ptr.as_ptr());
            }
            cloned.external.0.push(ExternalAllocation {
                zone_id: source.zone_id,
                slot_offset: source.slot_offset,
                allocation,
            });
        }
        Ok(cloned)
    }

    /// Resolves an inline payload or the pointer stored in an external slot.
    pub(crate) fn field_ptr(
        &self,
        field: &codegen::backends::cranelift::StructFieldLayout,
    ) -> *mut u8 {
        match field.kind {
            StructFieldKind::ExternalTable { .. } => unsafe {
                self.ptr_at(field.offset_bytes as usize)
                    .cast::<*mut u8>()
                    .read_unaligned()
            },
            _ => self.ptr_at(field.offset_bytes as usize),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MemoryManagerBinding {
    context: *mut c_void,
    begin: FaustMemoryManagerBeginFn,
    info: FaustMemoryManagerInfoFn,
    end: FaustMemoryManagerEndFn,
    allocate: FaustMemoryManagerAllocateFn,
    destroy: FaustMemoryManagerDestroyFn,
}

unsafe impl Send for MemoryManagerBinding {}
unsafe impl Sync for MemoryManagerBinding {}

impl MemoryManagerBinding {
    pub(crate) fn copy_from(table: &FaustMemoryManager) -> Result<Self, String> {
        if !table.is_compatible_v1() {
            return Err("incompatible faust_memory_manager ABI v1 table".to_owned());
        }
        Ok(Self {
            context: table.context,
            begin: table.begin.expect("validated callback"),
            info: table.info.expect("validated callback"),
            end: table.end.expect("validated callback"),
            allocate: table.allocate.expect("validated callback"),
            destroy: table.destroy.expect("validated callback"),
        })
    }

    pub(crate) fn same_identity(self, other: Self) -> bool {
        self.context == other.context
            && self.begin as usize == other.begin as usize
            && self.info as usize == other.info as usize
            && self.end as usize == other.end as usize
            && self.allocate as usize == other.allocate as usize
            && self.destroy as usize == other.destroy as usize
    }

    pub(crate) unsafe fn describe(self, analysis: &Mem0Analysis) -> Result<(), String> {
        let zones: Vec<_> = analysis
            .memory_layout
            .zones
            .iter()
            .filter(|zone| zone.runtime_allocated)
            .collect();
        unsafe { (self.begin)(self.context, zones.len()) };
        for zone in zones {
            let name = CString::new(zone.name.as_str())
                .map_err(|_| format!("mem0 zone name contains NUL: `{}`", zone.name))?;
            unsafe {
                (self.info)(
                    self.context,
                    name.as_ptr(),
                    memory_type_to_ffi(zone.memory_type),
                    usize::try_from(zone.element_count)
                        .map_err(|_| format!("mem0 count too large for `{}`", zone.name))?,
                    usize::try_from(zone.size_bytes)
                        .map_err(|_| format!("mem0 size too large for `{}`", zone.name))?,
                    usize::try_from(zone.alignment)
                        .map_err(|_| format!("mem0 alignment too large for `{}`", zone.name))?,
                    zone.reads,
                    zone.writes,
                )
            };
        }
        unsafe { (self.end)(self.context) };
        Ok(())
    }
}

enum AllocationOwner {
    Rust(Layout),
    Manager(MemoryManagerBinding),
}

struct OwnedAllocation {
    ptr: NonNull<u8>,
    size: usize,
    align: usize,
    owner: AllocationOwner,
}

// SAFETY: ownership is unique and callback access is serialized by the
// factory cache / per-factory mutex. The host manager must satisfy the same
// cross-thread contract as the public Cranelift factory API.
unsafe impl Send for OwnedAllocation {}

impl OwnedAllocation {
    unsafe fn manager(binding: MemoryManagerBinding, zone: &MemoryZone) -> Result<Self, String> {
        let size = usize::try_from(zone.size_bytes)
            .map_err(|_| format!("mem0 allocation too large for `{}`", zone.name))?;
        let align = usize::try_from(zone.alignment)
            .map_err(|_| format!("mem0 alignment too large for `{}`", zone.name))?;
        let raw = unsafe { (binding.allocate)(binding.context, size, align) }.cast::<u8>();
        let Some(ptr) = NonNull::new(raw) else {
            return Err(format!("memory manager failed to allocate `{}`", zone.name));
        };
        if !(ptr.as_ptr() as usize).is_multiple_of(align) {
            unsafe { (binding.destroy)(binding.context, raw.cast(), size, align) };
            return Err(format!(
                "memory manager returned misaligned storage for `{}`",
                zone.name
            ));
        }
        unsafe { ptr.as_ptr().write_bytes(0, size) };
        Ok(Self {
            ptr,
            size,
            align,
            owner: AllocationOwner::Manager(binding),
        })
    }

    unsafe fn allocate_like(&self) -> Result<Self, String> {
        match self.owner {
            AllocationOwner::Rust(_) => DspStateBuffer::new(self.size, self.align).map(|v| v.main),
            AllocationOwner::Manager(binding) => {
                let raw = unsafe { (binding.allocate)(binding.context, self.size, self.align) }
                    .cast::<u8>();
                let ptr = NonNull::new(raw)
                    .ok_or_else(|| "memory manager failed while cloning DSP storage".to_owned())?;
                if !(ptr.as_ptr() as usize).is_multiple_of(self.align) {
                    unsafe {
                        (binding.destroy)(binding.context, raw.cast(), self.size, self.align)
                    };
                    return Err("memory manager returned misaligned clone storage".to_owned());
                }
                Ok(Self {
                    ptr,
                    size: self.size,
                    align: self.align,
                    owner: AllocationOwner::Manager(binding),
                })
            }
        }
    }
}

impl Drop for OwnedAllocation {
    fn drop(&mut self) {
        unsafe {
            match self.owner {
                AllocationOwner::Rust(layout) => dealloc(self.ptr.as_ptr(), layout),
                AllocationOwner::Manager(binding) => (binding.destroy)(
                    binding.context,
                    self.ptr.as_ptr().cast(),
                    self.size,
                    self.align,
                ),
            }
        }
    }
}

struct ExternalAllocation {
    zone_id: MemoryZoneId,
    slot_offset: usize,
    allocation: OwnedAllocation,
}

#[derive(Default)]
struct ExternalAllocations(Vec<ExternalAllocation>);

impl Drop for ExternalAllocations {
    fn drop(&mut self) {
        // Instance buffers are allocated in canonical zone order. Pop rather
        // than relying on `Vec` element-drop order so the callback contract is
        // explicitly the reverse of allocation.
        while self.0.pop().is_some() {}
    }
}

pub(crate) struct ManagedClassStorage {
    allocations: Vec<(usize, OwnedAllocation)>,
}

impl ManagedClassStorage {
    pub(crate) unsafe fn create(
        binding: MemoryManagerBinding,
        analysis: &Mem0Analysis,
        slots: &[StaticMemorySlot],
    ) -> Result<Self, String> {
        let mut result = Self {
            allocations: Vec::new(),
        };
        for zone in analysis.memory_layout.zones.iter().filter(|zone| {
            zone.runtime_allocated
                && zone.scope == MemoryScope::Class
                && zone.role == MemoryRole::StaticTable
        }) {
            let slot = slots
                .iter()
                .find(|slot| slot.zone_id == zone.id)
                .ok_or_else(|| format!("mem0 class zone `{}` has no JIT slot", zone.name))?;
            let allocation = unsafe { OwnedAllocation::manager(binding, zone)? };
            unsafe {
                (slot.address as *mut *mut u8).write_unaligned(allocation.ptr.as_ptr());
            }
            result.allocations.push((slot.address, allocation));
        }
        Ok(result)
    }
}

impl Drop for ManagedClassStorage {
    fn drop(&mut self) {
        for (slot, _) in self.allocations.iter().rev() {
            unsafe { (*slot as *mut *mut u8).write_unaligned(std::ptr::null_mut()) };
        }
        while self.allocations.pop().is_some() {}
    }
}

#[derive(Default)]
pub(crate) struct FactoryMemoryState {
    pub(crate) binding: Option<MemoryManagerBinding>,
    pub(crate) class_storage: Option<ManagedClassStorage>,
    pub(crate) class_sample_rate: Option<i32>,
    pub(crate) class_busy: bool,
    pub(crate) live_instances: usize,
}

fn memory_type_to_ffi(memory_type: MemoryType) -> FaustMemoryType {
    match memory_type {
        MemoryType::Int32 => FaustMemoryType::Int32,
        MemoryType::Int32Ptr => FaustMemoryType::Int32Ptr,
        MemoryType::Float32 => FaustMemoryType::Float32,
        MemoryType::Float32Ptr => FaustMemoryType::Float32Ptr,
        MemoryType::Float64 => FaustMemoryType::Float64,
        MemoryType::Float64Ptr => FaustMemoryType::Float64Ptr,
        MemoryType::Quad => FaustMemoryType::Quad,
        MemoryType::QuadPtr => FaustMemoryType::QuadPtr,
        MemoryType::FixedPoint => FaustMemoryType::FixedPoint,
        MemoryType::FixedPointPtr => FaustMemoryType::FixedPointPtr,
        MemoryType::Object => FaustMemoryType::Object,
        MemoryType::ObjectPtr => FaustMemoryType::ObjectPtr,
        MemoryType::Sound => FaustMemoryType::Sound,
        MemoryType::SoundPtr => FaustMemoryType::SoundPtr,
        MemoryType::Int64 => FaustMemoryType::Int64,
        MemoryType::Int64Ptr => FaustMemoryType::Int64Ptr,
        MemoryType::Bool => FaustMemoryType::Bool,
        MemoryType::BoolPtr => FaustMemoryType::BoolPtr,
    }
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
    use super::{
        CraneliftDspFactory, CraneliftDspInstance, DspStateBuffer, MetaGlue, UIGlue, alloc_c_string,
    };

    #[test]
    fn scaffold_types_are_constructible_in_type_system() {
        fn assert_send<T: Send>() {}

        let _ = std::mem::size_of::<CraneliftDspFactory>();
        let _ = std::mem::size_of::<CraneliftDspInstance>();
        let _ = std::mem::size_of::<UIGlue>();
        let _ = std::mem::size_of::<MetaGlue>();
        assert_send::<CraneliftDspFactory>();
        assert_send::<CraneliftDspInstance>();
    }

    #[test]
    fn owned_state_and_c_string_helpers_roundtrip() {
        let _state = DspStateBuffer::new(32, 8).expect("test allocation");
        let s = alloc_c_string("ok");
        unsafe {
            ffi_common::free_c_string(s);
        }
    }
}
