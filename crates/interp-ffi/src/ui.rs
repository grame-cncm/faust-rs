//! UI and metadata dispatch helpers.
//!
//! Iterates `FbcUiInstruction` and `FbcMetaInstruction` lists and calls the
//! corresponding C callbacks in `UIGlue` / `MetaGlue`.
//!
//! # Float/Double dispatch
//! - `dispatch_ui_f32` — float mode (`f32` heap, `f32` scalars, `*mut f32` zones).
//! - `dispatch_ui_f64` — double mode: `f64` heap; scalar parameters narrowed to
//!   `f32` for callbacks; zone pointers are `*mut f64` reinterpreted as
//!   `*mut f32` (application must use `FAUSTFLOAT=double`).
//! - `control_ranges` — the same controls' ranges at the program's precision,
//!   for Rust callers (`instance::control_ranges`), outside the C ABI.

use std::ffi::{CString, c_void};

use codegen::backends::interp::{FbcMetaInstruction, FbcOpcode, FbcReal, FbcUiInstruction};
use ffi_common::ControlRange;

use crate::types::{FaustFloat, MetaGlue, UIGlue};

/// Dispatch a slice of `FbcUiInstruction<f32>` to a `UIGlue` callback table.
///
/// Each instruction maps to the corresponding `UIGlue` function pointer.
/// The `zone` pointer for widgets points into `real_heap` at `instr.offset`.
///
/// For `AddSoundfile` instructions, `soundfile_zones[instr.offset]` is passed
/// as the `void**` zone so the host's `SoundUI::addSoundfile` can write the
/// loaded `Soundfile*` to a stable address.
///
/// # Safety
/// - `glue` must be non-null and point to a valid `UIGlue`.
/// - `real_heap` must have at least `instr.offset + 1` elements for widget instructions.
pub(crate) unsafe fn dispatch_ui_f32(
    ui: &[FbcUiInstruction<FaustFloat>],
    real_heap: &mut [FaustFloat],
    soundfile_zones: &mut [*mut c_void],
    glue: *mut UIGlue,
) {
    unsafe {
        let glue = &*glue;
        for instr in ui {
            let label = c_str(&instr.label);
            let zone: *mut FaustFloat = if instr.offset >= 0 {
                real_heap
                    .get_mut(instr.offset as usize)
                    .map_or(std::ptr::null_mut(), |r| r as *mut FaustFloat)
            } else {
                std::ptr::null_mut()
            };

            match instr.opcode {
                FbcOpcode::OpenTabBox => {
                    if let Some(f) = glue.open_tab_box {
                        f(glue.ui_interface, label.as_ptr());
                    }
                }
                FbcOpcode::OpenHorizontalBox => {
                    if let Some(f) = glue.open_horizontal_box {
                        f(glue.ui_interface, label.as_ptr());
                    }
                }
                FbcOpcode::OpenVerticalBox => {
                    if let Some(f) = glue.open_vertical_box {
                        f(glue.ui_interface, label.as_ptr());
                    }
                }
                FbcOpcode::CloseBox => {
                    if let Some(f) = glue.close_box {
                        f(glue.ui_interface);
                    }
                }
                FbcOpcode::AddButton => {
                    if let Some(f) = glue.add_button {
                        f(glue.ui_interface, label.as_ptr(), zone);
                    }
                }
                FbcOpcode::AddCheckButton => {
                    if let Some(f) = glue.add_check_button {
                        f(glue.ui_interface, label.as_ptr(), zone);
                    }
                }
                FbcOpcode::AddVerticalSlider => {
                    if let Some(f) = glue.add_vertical_slider {
                        f(
                            glue.ui_interface,
                            label.as_ptr(),
                            zone,
                            instr.init,
                            instr.min,
                            instr.max,
                            instr.step,
                        );
                    }
                }
                FbcOpcode::AddHorizontalSlider => {
                    if let Some(f) = glue.add_horizontal_slider {
                        f(
                            glue.ui_interface,
                            label.as_ptr(),
                            zone,
                            instr.init,
                            instr.min,
                            instr.max,
                            instr.step,
                        );
                    }
                }
                FbcOpcode::AddNumEntry => {
                    if let Some(f) = glue.add_num_entry {
                        f(
                            glue.ui_interface,
                            label.as_ptr(),
                            zone,
                            instr.init,
                            instr.min,
                            instr.max,
                            instr.step,
                        );
                    }
                }
                FbcOpcode::AddHorizontalBargraph => {
                    if let Some(f) = glue.add_horizontal_bargraph {
                        f(
                            glue.ui_interface,
                            label.as_ptr(),
                            zone,
                            instr.min,
                            instr.max,
                        );
                    }
                }
                FbcOpcode::AddVerticalBargraph => {
                    if let Some(f) = glue.add_vertical_bargraph {
                        f(
                            glue.ui_interface,
                            label.as_ptr(),
                            zone,
                            instr.min,
                            instr.max,
                        );
                    }
                }
                FbcOpcode::AddSoundfile => {
                    if let Some(f) = glue.add_soundfile {
                        let url = c_str(&instr.key);
                        // Pass a pointer to the soundfile_zones slot so the host
                        // (e.g. SoundUI::addSoundfile) can write the Soundfile* to it.
                        let slot = instr.offset as usize;
                        let zone_ptr: *mut *mut c_void = soundfile_zones
                            .get_mut(slot)
                            .map_or(std::ptr::null_mut(), |z| z as *mut *mut c_void);
                        f(glue.ui_interface, label.as_ptr(), url.as_ptr(), zone_ptr);
                    }
                }
                FbcOpcode::Declare => {
                    if let Some(f) = glue.declare {
                        let key = c_str(&instr.key);
                        let val = c_str(&instr.value);
                        f(glue.ui_interface, zone, key.as_ptr(), val.as_ptr());
                    }
                }
                _ => {} // non-UI opcodes are silently ignored
            }
        }
    }
}

/// Dispatch `FbcMetaInstruction` list to a `MetaGlue` callback.
///
/// # Safety
/// `meta` must be non-null and point to a valid `MetaGlue`.
pub(crate) unsafe fn dispatch_meta(meta_block: &[FbcMetaInstruction], glue: *mut MetaGlue) {
    unsafe {
        let glue = &*glue;
        if let Some(declare) = glue.declare {
            for instr in meta_block {
                let key = c_str(&instr.key);
                let val = c_str(&instr.value);
                declare(glue.meta_interface, key.as_ptr(), val.as_ptr());
            }
        }
    }
}

/// Dispatch a slice of `FbcUiInstruction<f64>` to a `UIGlue` callback table.
///
/// Used in double mode (`--double`).  Scalar parameters (`init`, `min`, `max`,
/// `step`) are narrowed `f64 → f32` before being passed to the callbacks.
/// Zone pointers are `*mut f64` elements of the `f64` real_heap, cast to
/// `*mut f32` so the `UIGlue` signatures remain unchanged — the application
/// must be compiled with `FAUSTFLOAT=double` to interpret them correctly.
///
/// For `AddSoundfile` instructions, `soundfile_zones[instr.offset]` is passed
/// as the `void**` zone (same as in `dispatch_ui_f32`).
///
/// # Safety
/// - `glue` must be non-null and point to a valid `UIGlue`.
/// - `real_heap` must have at least `instr.offset + 1` elements for widget instructions.
pub(crate) unsafe fn dispatch_ui_f64(
    ui: &[FbcUiInstruction<f64>],
    real_heap: &mut [f64],
    soundfile_zones: &mut [*mut c_void],
    glue: *mut UIGlue,
) {
    unsafe {
        let glue = &*glue;
        for instr in ui {
            let label = c_str(&instr.label);
            // Zone pointer: *mut f64 reinterpreted as *mut f32 for UIGlue ABI.
            // Applications compiled with FAUSTFLOAT=double read this as double*.
            let zone: *mut FaustFloat = if instr.offset >= 0 {
                real_heap
                    .get_mut(instr.offset as usize)
                    .map_or(std::ptr::null_mut(), |r| r as *mut f64 as *mut FaustFloat)
            } else {
                std::ptr::null_mut()
            };

            // Scalar params narrowed f64 → f32 for UIGlue callback signatures.
            let init = instr.init as FaustFloat;
            let min = instr.min as FaustFloat;
            let max = instr.max as FaustFloat;
            let step = instr.step as FaustFloat;

            match instr.opcode {
                FbcOpcode::OpenTabBox => {
                    if let Some(f) = glue.open_tab_box {
                        f(glue.ui_interface, label.as_ptr());
                    }
                }
                FbcOpcode::OpenHorizontalBox => {
                    if let Some(f) = glue.open_horizontal_box {
                        f(glue.ui_interface, label.as_ptr());
                    }
                }
                FbcOpcode::OpenVerticalBox => {
                    if let Some(f) = glue.open_vertical_box {
                        f(glue.ui_interface, label.as_ptr());
                    }
                }
                FbcOpcode::CloseBox => {
                    if let Some(f) = glue.close_box {
                        f(glue.ui_interface);
                    }
                }
                FbcOpcode::AddButton => {
                    if let Some(f) = glue.add_button {
                        f(glue.ui_interface, label.as_ptr(), zone);
                    }
                }
                FbcOpcode::AddCheckButton => {
                    if let Some(f) = glue.add_check_button {
                        f(glue.ui_interface, label.as_ptr(), zone);
                    }
                }
                FbcOpcode::AddVerticalSlider => {
                    if let Some(f) = glue.add_vertical_slider {
                        f(
                            glue.ui_interface,
                            label.as_ptr(),
                            zone,
                            init,
                            min,
                            max,
                            step,
                        );
                    }
                }
                FbcOpcode::AddHorizontalSlider => {
                    if let Some(f) = glue.add_horizontal_slider {
                        f(
                            glue.ui_interface,
                            label.as_ptr(),
                            zone,
                            init,
                            min,
                            max,
                            step,
                        );
                    }
                }
                FbcOpcode::AddNumEntry => {
                    if let Some(f) = glue.add_num_entry {
                        f(
                            glue.ui_interface,
                            label.as_ptr(),
                            zone,
                            init,
                            min,
                            max,
                            step,
                        );
                    }
                }
                FbcOpcode::AddHorizontalBargraph => {
                    if let Some(f) = glue.add_horizontal_bargraph {
                        f(glue.ui_interface, label.as_ptr(), zone, min, max);
                    }
                }
                FbcOpcode::AddVerticalBargraph => {
                    if let Some(f) = glue.add_vertical_bargraph {
                        f(glue.ui_interface, label.as_ptr(), zone, min, max);
                    }
                }
                FbcOpcode::AddSoundfile => {
                    if let Some(f) = glue.add_soundfile {
                        let url = c_str(&instr.key);
                        let slot = instr.offset as usize;
                        let zone_ptr: *mut *mut c_void = soundfile_zones
                            .get_mut(slot)
                            .map_or(std::ptr::null_mut(), |z| z as *mut *mut c_void);
                        f(glue.ui_interface, label.as_ptr(), url.as_ptr(), zone_ptr);
                    }
                }
                FbcOpcode::Declare => {
                    if let Some(f) = glue.declare {
                        let key = c_str(&instr.key);
                        let val = c_str(&instr.value);
                        f(glue.ui_interface, zone, key.as_ptr(), val.as_ptr());
                    }
                }
                _ => {}
            }
        }
    }
}

/// Converts a Rust `&str` to a temporary `CString` for passing to C callbacks.
///
/// Embedded NUL bytes are replaced with `\0` literals.
fn c_str(s: &str) -> CString {
    let safe = s.replace('\0', "\\0");
    CString::new(safe).unwrap_or_else(|_| CString::new("").unwrap())
}

/// The ranges of the ranged controls of `ui` (sliders, numeric entries,
/// bargraphs), each with the zone [`dispatch_ui_f32`] / [`dispatch_ui_f64`]
/// pass for it, and its values widened to `f64` by `widen` (exact for both
/// precisions).
pub(crate) fn control_ranges<R: FbcReal>(
    ui: &[FbcUiInstruction<R>],
    real_heap: &mut [R],
    widen: impl Fn(R) -> f64,
) -> Vec<ControlRange> {
    ui.iter()
        .filter_map(|instr| {
            let bargraph = match instr.opcode {
                FbcOpcode::AddVerticalSlider
                | FbcOpcode::AddHorizontalSlider
                | FbcOpcode::AddNumEntry => false,
                FbcOpcode::AddHorizontalBargraph | FbcOpcode::AddVerticalBargraph => true,
                _ => return None,
            };
            let zone = real_heap
                .get_mut(usize::try_from(instr.offset).ok()?)
                .map(|r| (r as *mut R).cast::<FaustFloat>())?;
            let (init, step) = if bargraph {
                (0.0, 0.0)
            } else {
                (widen(instr.init), widen(instr.step))
            };
            Some(ControlRange {
                zone,
                bargraph,
                init,
                min: widen(instr.min),
                max: widen(instr.max),
                step,
            })
        })
        .collect()
}
