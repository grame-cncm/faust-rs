//! `control_ranges`, the Rust-only list of a Cranelift instance's control
//! ranges at the program's precision: the zones are those
//! `buildUserInterface` passes, and the values are exact for a `-double`
//! program, which the C callbacks receive narrowed to `float` (C++ parity).

use std::ffi::{CString, c_char, c_void};

use cranelift_ffi::factory::{createCCraneliftDSPFactoryFromString, deleteCCraneliftDSPFactory};
use cranelift_ffi::instance::{
    buildUserInterfaceCCraneliftDSPInstance, control_ranges, createCCraneliftDSPInstance,
    deleteCCraneliftDSPInstance, initCCraneliftDSPInstance,
};
use ffi_common::{ControlRange, FfiFaustFloat, UIGlue};

const PROGRAM: &str = r#"process = hslider("x", 0.1, 0, 1, 0.01) : vbargraph("v", -0.3, 0.7);"#;

/// What the C callbacks received, in the UI order (labels sorted within a
/// group, so `v` before `x`): each zone with its `init` (slider) or `min`
/// (bargraph).
#[derive(Default)]
struct Received(Vec<(*mut FfiFaustFloat, f32)>);

unsafe extern "C" fn slider(
    ui: *mut c_void,
    _label: *const c_char,
    zone: *mut FfiFaustFloat,
    init: FfiFaustFloat,
    _min: FfiFaustFloat,
    _max: FfiFaustFloat,
    _step: FfiFaustFloat,
) {
    unsafe { (*ui.cast::<Received>()).0.push((zone, init)) };
}

unsafe extern "C" fn bargraph(
    ui: *mut c_void,
    _label: *const c_char,
    zone: *mut FfiFaustFloat,
    min: FfiFaustFloat,
    _max: FfiFaustFloat,
) {
    unsafe { (*ui.cast::<Received>()).0.push((zone, min)) };
}

/// Compiles `PROGRAM` with `args`, and returns the control ranges and what
/// `buildUserInterface` passed.
fn ranges_and_callbacks(args: &[&str]) -> (Vec<ControlRange>, Vec<(*mut FfiFaustFloat, f32)>) {
    let name = CString::new("ranges").unwrap();
    let code = CString::new(PROGRAM).unwrap();
    let owned: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
    let argv: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
    let mut error = [0 as c_char; 4096];
    let mut received = Received::default();
    let mut glue = UIGlue {
        ui_interface: (&raw mut received).cast::<c_void>(),
        open_tab_box: None,
        open_horizontal_box: None,
        open_vertical_box: None,
        close_box: None,
        add_button: None,
        add_check_button: None,
        add_vertical_slider: None,
        add_horizontal_slider: Some(slider),
        add_num_entry: None,
        add_horizontal_bargraph: None,
        add_vertical_bargraph: Some(bargraph),
        add_soundfile: None,
        declare: None,
    };
    unsafe {
        let factory = createCCraneliftDSPFactoryFromString(
            name.as_ptr(),
            code.as_ptr(),
            argv.len() as _,
            argv.as_ptr(),
            error.as_mut_ptr(),
            0,
        );
        assert!(!factory.is_null());
        let dsp = createCCraneliftDSPInstance(factory);
        initCCraneliftDSPInstance(dsp, 48_000);
        buildUserInterfaceCCraneliftDSPInstance(dsp, &mut glue);
        let ranges = control_ranges(dsp);
        deleteCCraneliftDSPInstance(dsp);
        deleteCCraneliftDSPFactory(factory);
        (ranges, received.0)
    }
}

#[test]
fn a_double_program_s_ranges_are_exact_and_its_callbacks_get_floats() {
    let (ranges, received) = ranges_and_callbacks(&["-double"]);
    assert_eq!(ranges.len(), 2);
    let zones: Vec<_> = ranges.iter().map(|r| r.zone).collect();
    assert_eq!(zones, received.iter().map(|r| r.0).collect::<Vec<_>>());
    let x = ranges[1];
    assert!(!x.bargraph);
    assert_eq!((x.init, x.min, x.max, x.step), (0.1, 0.0, 1.0, 0.01));
    let v = ranges[0];
    assert!(v.bargraph);
    assert_eq!((v.init, v.min, v.max, v.step), (0.0, -0.3, 0.7, 0.0));
    // the C callbacks keep their `float` signature
    assert_eq!(received[1].1, 0.1_f32);
    assert_eq!(received[0].1, -0.3_f32);
}

#[test]
fn a_single_program_s_ranges_are_its_float_values() {
    let (ranges, received) = ranges_and_callbacks(&[]);
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[1].zone, received[1].0);
    assert_eq!(ranges[1].init, f64::from(0.1_f32));
    assert_eq!(ranges[1].step, f64::from(0.01_f32));
    assert_eq!(ranges[0].min, f64::from(-0.3_f32));
}
