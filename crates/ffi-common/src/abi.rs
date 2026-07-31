//! Shared C ABI callback-table definitions.

use std::ffi::{c_char, c_void};

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

    use super::{FfiFaustFloat, MetaGlue, UIGlue};

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
}
