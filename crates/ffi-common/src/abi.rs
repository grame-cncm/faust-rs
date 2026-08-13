//! Shared C ABI callback-table definitions.

use std::ffi::{c_char, c_void};

/// Version of the Faust custom-memory-manager C callback table.
///
/// Source provenance: Faust C++ `dsp_memory_manager` in
/// `architecture/faust/dsp/dsp.h`. Versioning and `struct_size` are an
/// `adapted` extension: consumers can reject an incompatible table before
/// calling through host-provided function pointers.
pub const FAUST_MEMORY_MANAGER_ABI_VERSION: u32 = 1;

/// Manager-visible element category used by [`FaustMemoryManagerInfoFn`].
///
/// The first ten discriminants retain Faust C++ `MemoryDesc::MemoryType`
/// ordering. `Int64`, `Int64Ptr`, `Bool`, and `BoolPtr` are append-only
/// extensions needed to describe the complete faust-rs FIR type vocabulary.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaustMemoryType {
    Int32 = 0,
    Int32Ptr = 1,
    Float32 = 2,
    Float32Ptr = 3,
    Float64 = 4,
    Float64Ptr = 5,
    Quad = 6,
    QuadPtr = 7,
    FixedPoint = 8,
    FixedPointPtr = 9,
    Object = 10,
    ObjectPtr = 11,
    Sound = 12,
    SoundPtr = 13,
    Int64 = 14,
    Int64Ptr = 15,
    Bool = 16,
    BoolPtr = 17,
}

/// Starts one deterministic memory-description transaction.
pub type FaustMemoryManagerBeginFn = unsafe extern "C" fn(context: *mut c_void, count: usize);
/// Reports one allocation zone, including byte alignment and static access counts.
pub type FaustMemoryManagerInfoFn = unsafe extern "C" fn(
    context: *mut c_void,
    name: *const c_char,
    memory_type: FaustMemoryType,
    element_count: usize,
    size_bytes: usize,
    alignment: usize,
    reads: u64,
    writes: u64,
);
/// Ends one memory-description transaction.
pub type FaustMemoryManagerEndFn = unsafe extern "C" fn(context: *mut c_void);
/// Allocates one zone with the requested byte size and alignment.
pub type FaustMemoryManagerAllocateFn =
    unsafe extern "C" fn(context: *mut c_void, size_bytes: usize, alignment: usize) -> *mut c_void;
/// Releases one zone using the same size/alignment pair used to allocate it.
pub type FaustMemoryManagerDestroyFn = unsafe extern "C" fn(
    context: *mut c_void,
    address: *mut c_void,
    size_bytes: usize,
    alignment: usize,
);

/// Versioned C ABI shared by generated C and Cranelift `-mem0` instances.
///
/// All callbacks are mandatory in ABI v1. The generated DSP captures the
/// table pointer that created it; allocation and destruction therefore use
/// the same host context even if another factory is compiled later. Hosts must
/// keep the table and context alive through the last DSP/class destruction.
#[repr(C)]
pub struct FaustMemoryManager {
    /// Must equal [`FAUST_MEMORY_MANAGER_ABI_VERSION`].
    pub abi_version: u32,
    /// Must be at least `size_of::<FaustMemoryManager>()` for ABI v1.
    pub struct_size: usize,
    /// Opaque host value forwarded to every callback.
    pub context: *mut c_void,
    pub begin: Option<FaustMemoryManagerBeginFn>,
    pub info: Option<FaustMemoryManagerInfoFn>,
    pub end: Option<FaustMemoryManagerEndFn>,
    pub allocate: Option<FaustMemoryManagerAllocateFn>,
    pub destroy: Option<FaustMemoryManagerDestroyFn>,
}

impl FaustMemoryManager {
    /// Checks whether this table satisfies the complete ABI-v1 contract.
    #[must_use]
    pub fn is_compatible_v1(&self) -> bool {
        self.abi_version == FAUST_MEMORY_MANAGER_ABI_VERSION
            && self.struct_size >= std::mem::size_of::<Self>()
            && self.begin.is_some()
            && self.info.is_some()
            && self.end.is_some()
            && self.allocate.is_some()
            && self.destroy.is_some()
    }
}

/// `FAUSTFLOAT` type used by current Rust FFI exports (`f32`).
pub type FfiFaustFloat = f32;

/// C-ABI UI callback table used by generated/runtime DSP code (mirrors Faust `UIGlue`).
///
/// Backend FFI crates re-export this type so the external C ABI remains stable
/// while the callback-table definition is maintained in a single place.
#[repr(C)]
pub struct UIGlue {
    /// Opaque host context passed as the first argument to every callback.
    pub ui_interface: *mut c_void,
    /// Opens a tab group with the supplied label.
    pub open_tab_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    /// Opens a horizontal group with the supplied label.
    pub open_horizontal_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    /// Opens a vertical group with the supplied label.
    pub open_vertical_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    /// Closes the most recently opened UI group.
    pub close_box: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Adds a momentary button bound to a DSP zone.
    pub add_button: Option<unsafe extern "C" fn(*mut c_void, *const c_char, *mut FfiFaustFloat)>,
    /// Adds a toggle button bound to a DSP zone.
    pub add_check_button:
        Option<unsafe extern "C" fn(*mut c_void, *const c_char, *mut FfiFaustFloat)>,
    /// Adds a vertical slider with label, zone, initial value, range, and step.
    pub add_vertical_slider: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    /// Adds a horizontal slider with label, zone, initial value, range, and step.
    pub add_horizontal_slider: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    /// Adds a numeric entry with label, zone, initial value, range, and step.
    pub add_num_entry: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    /// Adds a horizontal bargraph with label, zone, and display range.
    pub add_horizontal_bargraph: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    /// Adds a vertical bargraph with label, zone, and display range.
    pub add_vertical_bargraph: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    /// Adds a soundfile control and returns its host-managed handle through the out-pointer.
    pub add_soundfile:
        Option<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *mut *mut c_void)>,
    /// Attaches key/value metadata to an optional DSP zone.
    pub declare:
        Option<unsafe extern "C" fn(*mut c_void, *mut FfiFaustFloat, *const c_char, *const c_char)>,
}

/// C-ABI metadata callback table used by generated/runtime DSP code (mirrors Faust `MetaGlue`).
#[repr(C)]
pub struct MetaGlue {
    /// Opaque host context passed to [`MetaGlue::declare`].
    pub meta_interface: *mut c_void,
    /// Publishes one DSP metadata key/value pair.
    pub declare: Option<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char)>,
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::mem::{align_of, offset_of, size_of};

    use super::{
        FAUST_MEMORY_MANAGER_ABI_VERSION, FaustMemoryManager, FaustMemoryType, FfiFaustFloat,
        MetaGlue, UIGlue,
    };

    /// Mirrors the pointer-slot assertions compiled from both maintained C
    /// backend headers by `xtask libfaust-export-check`.
    #[test]
    fn ffi_glue_layout_matches_the_c_pointer_slot_contract() {
        let slot = size_of::<*const c_void>();
        assert_eq!(size_of::<UIGlue>(), 14 * slot);
        assert_eq!(align_of::<UIGlue>(), align_of::<*const c_void>());
        assert_eq!(
            [
                offset_of!(UIGlue, ui_interface),
                offset_of!(UIGlue, open_tab_box),
                offset_of!(UIGlue, open_horizontal_box),
                offset_of!(UIGlue, open_vertical_box),
                offset_of!(UIGlue, close_box),
                offset_of!(UIGlue, add_button),
                offset_of!(UIGlue, add_check_button),
                offset_of!(UIGlue, add_vertical_slider),
                offset_of!(UIGlue, add_horizontal_slider),
                offset_of!(UIGlue, add_num_entry),
                offset_of!(UIGlue, add_horizontal_bargraph),
                offset_of!(UIGlue, add_vertical_bargraph),
                offset_of!(UIGlue, add_soundfile),
                offset_of!(UIGlue, declare),
            ],
            std::array::from_fn::<_, 14, _>(|index| index * slot)
        );

        assert_eq!(size_of::<MetaGlue>(), 2 * slot);
        assert_eq!(align_of::<MetaGlue>(), align_of::<*const c_void>());
        assert_eq!(offset_of!(MetaGlue, meta_interface), 0);
        assert_eq!(offset_of!(MetaGlue, declare), slot);
    }

    #[test]
    fn ffi_faust_float_is_the_header_default_float() {
        assert_eq!(size_of::<FfiFaustFloat>(), size_of::<f32>());
        assert_eq!(align_of::<FfiFaustFloat>(), align_of::<f32>());
    }

    #[test]
    fn memory_manager_v1_layout_is_an_append_only_c_table() {
        let pointer_alignment = align_of::<*const c_void>();
        assert_eq!(size_of::<FaustMemoryType>(), size_of::<u32>());
        assert_eq!(FaustMemoryType::Int32 as u32, 0);
        assert_eq!(FaustMemoryType::FixedPointPtr as u32, 9);
        assert_eq!(FaustMemoryType::Object as u32, 10);
        assert_eq!(FaustMemoryType::BoolPtr as u32, 17);
        assert_eq!(offset_of!(FaustMemoryManager, abi_version), 0);
        assert_eq!(
            offset_of!(FaustMemoryManager, struct_size) % align_of::<usize>(),
            0
        );
        assert_eq!(
            offset_of!(FaustMemoryManager, context) % pointer_alignment,
            0
        );
        for offset in [
            offset_of!(FaustMemoryManager, begin),
            offset_of!(FaustMemoryManager, info),
            offset_of!(FaustMemoryManager, end),
            offset_of!(FaustMemoryManager, allocate),
            offset_of!(FaustMemoryManager, destroy),
        ] {
            assert_eq!(offset % pointer_alignment, 0);
        }
        assert_eq!(align_of::<FaustMemoryManager>(), pointer_alignment);
        assert!(size_of::<FaustMemoryManager>() >= 7 * size_of::<*const c_void>());
    }

    #[test]
    fn memory_manager_rejects_incomplete_or_mismatched_tables() {
        unsafe extern "C" fn begin(_: *mut c_void, _: usize) {}
        unsafe extern "C" fn info(
            _: *mut c_void,
            _: *const std::ffi::c_char,
            _: FaustMemoryType,
            _: usize,
            _: usize,
            _: usize,
            _: u64,
            _: u64,
        ) {
        }
        unsafe extern "C" fn end(_: *mut c_void) {}
        unsafe extern "C" fn allocate(_: *mut c_void, _: usize, _: usize) -> *mut c_void {
            std::ptr::null_mut()
        }
        unsafe extern "C" fn destroy(_: *mut c_void, _: *mut c_void, _: usize, _: usize) {}

        let mut table = FaustMemoryManager {
            abi_version: FAUST_MEMORY_MANAGER_ABI_VERSION,
            struct_size: size_of::<FaustMemoryManager>(),
            context: std::ptr::null_mut(),
            begin: Some(begin),
            info: Some(info),
            end: Some(end),
            allocate: Some(allocate),
            destroy: Some(destroy),
        };
        assert!(table.is_compatible_v1());
        table.abi_version += 1;
        assert!(!table.is_compatible_v1());
        table.abi_version = FAUST_MEMORY_MANAGER_ABI_VERSION;
        table.destroy = None;
        assert!(!table.is_compatible_v1());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn canonical_memory_manager_header_compiles_as_c_and_cpp() {
        use std::process::Command;

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let include = manifest.join("include");
        let source_text = r#"
#include "faust-memory-manager.h"
#include <stddef.h>
#if defined(__cplusplus)
#define ABI_ASSERT static_assert
#else
#define ABI_ASSERT _Static_assert
#endif
ABI_ASSERT(FAUST_MEMORY_MANAGER_ABI_VERSION == 1u, "ABI version");
ABI_ASSERT(kMemInt32 == 0, "first enum value");
ABI_ASSERT(kMemBoolPtr == 17, "append-only enum tail");
ABI_ASSERT(offsetof(faust_memory_manager, abi_version) == 0, "version first");
ABI_ASSERT(offsetof(faust_memory_manager, context) > offsetof(faust_memory_manager, struct_size), "context order");
ABI_ASSERT(offsetof(faust_memory_manager, destroy) > offsetof(faust_memory_manager, allocate), "callback order");
int main(void) { return 0; }
"#;
        for (compiler, extension, standard) in [
            (
                std::env::var("CC").unwrap_or_else(|_| "cc".to_owned()),
                "c",
                "-std=c11",
            ),
            (
                std::env::var("CXX").unwrap_or_else(|_| "c++".to_owned()),
                "cpp",
                "-std=c++17",
            ),
        ] {
            if Command::new(&compiler).arg("--version").output().is_err() {
                eprintln!("skipping ABI header smoke: `{compiler}` is unavailable");
                continue;
            }
            let stem = format!(
                "faust-memory-manager-abi-{}-{extension}",
                std::process::id()
            );
            let source = std::env::temp_dir().join(format!("{stem}.{extension}"));
            std::fs::write(&source, source_text).expect("write ABI smoke source");
            let output = Command::new(&compiler)
                .arg(standard)
                .args(["-Wall", "-Wextra", "-Werror", "-pedantic", "-fsyntax-only"])
                .arg("-I")
                .arg(&include)
                .arg(&source)
                .output()
                .expect("run ABI smoke compiler");
            assert!(
                output.status.success(),
                "{extension} header smoke failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let _ = std::fs::remove_file(source);
        }
    }
}
