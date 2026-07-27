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
    pub ui_interface: *mut c_void,
    pub open_tab_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    pub open_horizontal_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    pub open_vertical_box: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    pub close_box: Option<unsafe extern "C" fn(*mut c_void)>,
    pub add_button: Option<unsafe extern "C" fn(*mut c_void, *const c_char, *mut FfiFaustFloat)>,
    pub add_check_button:
        Option<unsafe extern "C" fn(*mut c_void, *const c_char, *mut FfiFaustFloat)>,
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
    pub add_horizontal_bargraph: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    pub add_vertical_bargraph: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *mut FfiFaustFloat,
            FfiFaustFloat,
            FfiFaustFloat,
        ),
    >,
    pub add_soundfile:
        Option<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *mut *mut c_void)>,
    pub declare:
        Option<unsafe extern "C" fn(*mut c_void, *mut FfiFaustFloat, *const c_char, *const c_char)>,
}

/// C-ABI metadata callback table used by generated/runtime DSP code (mirrors Faust `MetaGlue`).
#[repr(C)]
pub struct MetaGlue {
    pub meta_interface: *mut c_void,
    pub declare: Option<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char)>,
}

#[cfg(test)]
mod tests {
    use super::{MetaGlue, UIGlue};

    #[test]
    fn ffi_glue_types_are_constructible() {
        let _ = std::mem::size_of::<UIGlue>();
        let _ = std::mem::size_of::<MetaGlue>();
    }
}
