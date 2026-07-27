//! Shared Faust tree-handle support for C ABI crates.
//!
//! This crate owns the representation-level FFI mechanics that are common to
//! Box and Signal APIs: a `TreeArena`, stable opaque handles, context-owned C
//! strings, null-terminated handle arrays, and null-safe out-pointer helpers.
//! It intentionally does not export libfaust symbols; API crates remain
//! responsible for their public `Cbox*`, `Csig*`, and backend entry points.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Mutex, OnceLock};

use tlib::{TreeArena, TreeId};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(non_camel_case_types)]
/// Scalar type tags shared by libfaust Box and Signal foreign APIs.
pub enum SType {
    kSInt = 0,
    kSReal = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(non_camel_case_types)]
/// Binary operator tags shared by libfaust Box and Signal APIs.
pub enum SOperator {
    kAdd = 0,
    kSub = 1,
    kMul = 2,
    kDiv = 3,
    kRem = 4,
    kLsh = 5,
    kARsh = 6,
    kLRsh = 7,
    kGT = 8,
    kLT = 9,
    kGE = 10,
    kLE = 11,
    kEQ = 12,
    kNE = 13,
    kAND = 14,
    kOR = 15,
    kXOR = 16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// UI/control families registered by Signal FFI constructors.
pub enum FfiSignalControlKind {
    Button,
    Checkbox,
    VSlider,
    HSlider,
    NumEntry,
    VBargraph,
    HBargraph,
    Soundfile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Context-owned metadata for one Signal FFI UI/control constructor.
pub struct FfiSignalControl {
    /// Stable control id embedded in Signal UI leaves.
    pub id: u32,
    /// Control family.
    pub kind: FfiSignalControlKind,
    /// Original C label stored as an arena symbol tree.
    pub label: TreeId,
    /// Optional initial value expression for sliders/numentries.
    pub init: Option<TreeId>,
    /// Optional minimum expression for sliders, numentries, and bargraphs.
    pub min: Option<TreeId>,
    /// Optional maximum expression for sliders, numentries, and bargraphs.
    pub max: Option<TreeId>,
    /// Optional step expression for sliders/numentries.
    pub step: Option<TreeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FfiSignalControlKey {
    kind: FfiSignalControlKind,
    label: u32,
    init: Option<u32>,
    min: Option<u32>,
    max: Option<u32>,
    step: Option<u32>,
}

impl FfiSignalControlKey {
    fn new(
        kind: FfiSignalControlKind,
        label: TreeId,
        init: Option<TreeId>,
        min: Option<TreeId>,
        max: Option<TreeId>,
        step: Option<TreeId>,
    ) -> Self {
        Self {
            kind,
            label: label.as_u32(),
            init: init.map(TreeId::as_u32),
            min: min.map(TreeId::as_u32),
            max: max.map(TreeId::as_u32),
            step: step.map(TreeId::as_u32),
        }
    }
}

/// Process-global arena and ownership state used behind opaque C tree handles.
pub struct FfiTreeContext {
    /// Hash-consed Box/Signal tree arena.
    pub arena: TreeArena,
    by_handle: HashMap<usize, TreeId>,
    by_tree: HashMap<u32, usize>,
    string_pool: Vec<CString>,
    handle_array_allocs: HashMap<usize, usize>,
    signal_controls: Vec<FfiSignalControl>,
    signal_control_by_key: HashMap<FfiSignalControlKey, u32>,
    next_handle: usize,
}

static GLOBAL_TREE_CONTEXT: OnceLock<Mutex<FfiTreeContext>> = OnceLock::new();

/// Returns the lazily initialized global FFI tree context.
///
/// This process-global context is shared by Box and Signal C ABI crates so
/// opaque handles produced by one API surface can be decoded by shared helpers
/// such as `CprintSignal` and `freeCMemory`.
pub fn global_context() -> &'static Mutex<FfiTreeContext> {
    GLOBAL_TREE_CONTEXT.get_or_init(|| Mutex::new(FfiTreeContext::new()))
}

/// Executes one closure while holding the shared global FFI tree context mutex.
pub fn with_global_context<R>(f: impl FnOnce(&mut FfiTreeContext) -> R) -> R {
    let mut guard = global_context().lock().expect("FFI tree context poisoned");
    f(&mut guard)
}

/// Resets the shared global FFI tree context.
pub fn reset_global_context() {
    with_global_context(|ctx| {
        *ctx = FfiTreeContext::new();
    });
}

impl FfiTreeContext {
    /// Creates a fresh context with the canonical `nil` handle reserved.
    pub fn new() -> Self {
        let mut ctx = Self {
            arena: TreeArena::new(),
            by_handle: HashMap::new(),
            by_tree: HashMap::new(),
            string_pool: Vec::new(),
            handle_array_allocs: HashMap::new(),
            signal_controls: Vec::new(),
            signal_control_by_key: HashMap::new(),
            next_handle: 1,
        };
        let nil = ctx.arena.nil();
        let _ = ctx.encode(nil);
        ctx
    }

    /// Encodes one arena tree id as a stable opaque FFI handle.
    pub fn encode(&mut self, id: TreeId) -> *mut c_void {
        if let Some(handle) = self.by_tree.get(&id.as_u32()).copied() {
            return handle as *mut c_void;
        }
        let handle = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1);
        self.by_handle.insert(handle, id);
        self.by_tree.insert(id.as_u32(), handle);
        handle as *mut c_void
    }

    /// Decodes one opaque FFI handle back into the corresponding arena tree id.
    pub fn decode(&self, ptr: *mut c_void) -> Option<TreeId> {
        if ptr.is_null() {
            return None;
        }
        let handle = ptr as usize;
        self.by_handle.get(&handle).copied()
    }

    /// Interns a C string as an arena symbol node.
    ///
    /// # Safety
    /// `label` must be null or point to a valid NUL-terminated C string.
    pub unsafe fn label_tree(&mut self, label: *const c_char) -> Option<TreeId> {
        if label.is_null() {
            return None;
        }
        let txt = unsafe { CStr::from_ptr(label) }.to_str().ok()?;
        Some(self.arena.symbol(txt))
    }

    /// Interns a Rust string in the context-owned C string pool.
    pub fn intern_c_str_ptr(&mut self, s: &str) -> *const c_char {
        let safe = s.replace('\0', "\\0");
        match CString::new(safe) {
            Ok(cstr) => {
                self.string_pool.push(cstr);
                self.string_pool
                    .last()
                    .map_or(std::ptr::null(), |v| v.as_ptr())
            }
            Err(_) => std::ptr::null(),
        }
    }

    /// Allocates a null-terminated array of opaque handles owned by the context.
    pub fn alloc_handle_ptr_array(&mut self, mut values: Vec<*mut c_void>) -> *mut *mut c_void {
        values.push(std::ptr::null_mut());
        let len = values.len();
        let mut vec = values;
        let raw = vec.as_mut_ptr();
        std::mem::forget(vec);
        self.handle_array_allocs.insert(raw as usize, len);
        raw
    }

    /// Frees a pointer previously returned by [`Self::alloc_handle_ptr_array`].
    pub fn free_if_handle_ptr_array(&mut self, ptr: *mut c_void) -> bool {
        let key = ptr as usize;
        let Some(len) = self.handle_array_allocs.remove(&key) else {
            return false;
        };
        unsafe {
            drop(Vec::from_raw_parts(ptr as *mut *mut c_void, len, len));
        }
        true
    }

    /// Registers or reuses one Signal FFI UI/control descriptor.
    pub fn register_signal_control(
        &mut self,
        kind: FfiSignalControlKind,
        label: TreeId,
        init: Option<TreeId>,
        min: Option<TreeId>,
        max: Option<TreeId>,
        step: Option<TreeId>,
    ) -> u32 {
        let key = FfiSignalControlKey::new(kind, label, init, min, max, step);
        if let Some(id) = self.signal_control_by_key.get(&key).copied() {
            return id;
        }
        let id = u32::try_from(self.signal_controls.len()).unwrap_or(u32::MAX);
        self.signal_controls.push(FfiSignalControl {
            id,
            kind,
            label,
            init,
            min,
            max,
            step,
        });
        self.signal_control_by_key.insert(key, id);
        id
    }

    /// Returns one Signal FFI UI/control descriptor by id.
    pub fn signal_control(&self, id: u32) -> Option<&FfiSignalControl> {
        self.signal_controls.get(usize::try_from(id).ok()?)
    }

    /// Returns all Signal FFI UI/control descriptors in id order.
    pub fn signal_controls(&self) -> &[FfiSignalControl] {
        &self.signal_controls
    }
}

impl Default for FfiTreeContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads one optional C string and converts it to owned UTF-8.
///
/// # Safety
/// `label` must be null or point to a valid NUL-terminated C string.
pub unsafe fn read_c_string(label: *const c_char) -> Option<String> {
    if label.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(label) }
        .to_str()
        .ok()
        .map(ToOwned::to_owned)
}

/// Writes one tree handle result to an optional out-pointer.
///
/// # Safety
/// `out` must be null or point to writable memory for one handle.
pub unsafe fn write_out_handle(ctx: &mut FfiTreeContext, out: *mut *mut c_void, value: TreeId) {
    if !out.is_null() {
        unsafe {
            *out = ctx.encode(value);
        }
    }
}

/// Writes one integer result to an optional out-pointer.
///
/// # Safety
/// `out` must be null or point to writable memory for one integer.
pub unsafe fn write_out_int(out: *mut c_int, value: i32) {
    if !out.is_null() {
        unsafe {
            *out = value;
        }
    }
}

/// Writes one floating-point result to an optional out-pointer.
///
/// # Safety
/// `out` must be null or point to writable memory for one `f64`.
pub unsafe fn write_out_real(out: *mut f64, value: f64) {
    if !out.is_null() {
        unsafe {
            *out = value;
        }
    }
}

/// Frees context-owned handle arrays before falling back to C-string frees.
///
/// # Safety
/// `ptr` must be null or a pointer previously returned by this library for a
/// handle array or C string result.
pub unsafe fn free_context_or_c_string(ctx: &mut FfiTreeContext, ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    if !ctx.free_if_handle_ptr_array(ptr) {
        unsafe { ffi_common::free_c_memory_c_string_only(ptr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_tree_handles_stably() {
        let mut ctx = FfiTreeContext::new();
        let a = ctx.arena.int(1);
        let first = ctx.encode(a);
        let second = ctx.encode(a);

        assert_eq!(first, second);
        assert_eq!(ctx.decode(first), Some(a));
    }

    #[test]
    fn tracks_null_terminated_handle_arrays() {
        let mut ctx = FfiTreeContext::new();
        let raw = ctx.alloc_handle_ptr_array(vec![std::ptr::dangling_mut::<c_void>()]);

        assert!(!raw.is_null());
        assert!(ctx.free_if_handle_ptr_array(raw.cast()));
        assert!(!ctx.free_if_handle_ptr_array(raw.cast()));
    }

    #[test]
    fn reuses_signal_control_ids_for_identical_metadata() {
        let mut ctx = FfiTreeContext::new();
        let label = ctx.arena.symbol("gain");
        let init = ctx.arena.float(0.5);

        let first = ctx.register_signal_control(
            FfiSignalControlKind::HSlider,
            label,
            Some(init),
            None,
            None,
            None,
        );
        let second = ctx.register_signal_control(
            FfiSignalControlKind::HSlider,
            label,
            Some(init),
            None,
            None,
            None,
        );

        assert_eq!(first, second);
        assert_eq!(ctx.signal_controls().len(), 1);
    }
}
