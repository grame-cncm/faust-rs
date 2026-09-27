//! The widths of a compute call: the C entry point exchanges `f32` whatever
//! the program's precision (C++ parity), and the Rust-only `compute_f64`
//! exchanges `f64`, unrounded for a `-double` program.

use std::ffi::{CString, c_char};

use faust_interp::factory::{createCInterpreterDSPFactoryFromString, deleteCInterpreterDSPFactory};
use faust_interp::instance::{
    compute_f64, computeCInterpreterDSPInstance, createCInterpreterDSPInstance,
    deleteCInterpreterDSPInstance, initCInterpreterDSPInstance,
};
use faust_interp::types::{InterpreterDspFactory, InterpreterDspInstance};

/// A compiled program and one initialised instance, deleted on drop.
struct Program {
    factory: *mut InterpreterDspFactory,
    dsp: *mut InterpreterDspInstance,
}

impl Program {
    fn new(code: &str, args: &[&str]) -> Self {
        let name = CString::new("compute_io").unwrap();
        let code = CString::new(code).unwrap();
        let owned: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
        let argv: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        let mut error = [0 as c_char; 4096];
        unsafe {
            let factory = createCInterpreterDSPFactoryFromString(
                name.as_ptr(),
                code.as_ptr(),
                argv.len() as i32,
                argv.as_ptr(),
                error.as_mut_ptr(),
            );
            assert!(!factory.is_null(), "the program does not compile");
            let dsp = createCInterpreterDSPInstance(factory);
            assert!(!dsp.is_null());
            initCInterpreterDSPInstance(dsp, 48_000);
            Self { factory, dsp }
        }
    }

    /// One block through the C entry point.
    fn compute_c(&self, inputs: &mut [Vec<f32>], outputs: &mut [Vec<f32>]) {
        let count = outputs[0].len() as i32;
        let mut ins: Vec<*mut f32> = inputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let mut outs: Vec<*mut f32> = outputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        unsafe {
            computeCInterpreterDSPInstance(self.dsp, count, ins.as_mut_ptr(), outs.as_mut_ptr())
        };
    }

    /// One block through the Rust `f64` entry point.
    fn compute_f64(&self, inputs: &[Vec<f64>], outputs: &mut [Vec<f64>]) {
        let count = outputs[0].len() as i32;
        let ins: Vec<*const f64> = inputs.iter().map(|c| c.as_ptr()).collect();
        let outs: Vec<*mut f64> = outputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        unsafe { compute_f64(self.dsp, count, ins.as_ptr(), outs.as_ptr()) };
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        unsafe {
            deleteCInterpreterDSPInstance(self.dsp);
            deleteCInterpreterDSPFactory(self.factory);
        }
    }
}

/// 1 + 2^-40: not representable in `f32`.
fn tiny() -> f64 {
    1.0 + 2.0_f64.powi(-40)
}

#[test]
fn compute_f64_carries_a_double_program_s_samples_unrounded() {
    let wire = Program::new("process = _;", &["-double"]);
    let mut out = vec![vec![0.0_f64; 16]];
    wire.compute_f64(&[vec![tiny(); 16]], &mut out);
    assert_eq!(out[0], vec![tiny(); 16]);

    let constant = Program::new("process = 16777217.0;", &["-double"]);
    let mut out = vec![vec![0.0_f64; 16]];
    constant.compute_f64(&[], &mut out);
    assert_eq!(out[0], vec![16_777_217.0; 16]);
}

#[test]
fn compute_f64_on_a_single_program_rounds_its_inputs_to_f32() {
    let wire = Program::new("process = _;", &[]);
    let input = vec![tiny(), 0.1, -3.5, 16_777_217.0];
    let mut out = vec![vec![0.0_f64; 4]];
    wire.compute_f64(std::slice::from_ref(&input), &mut out);
    assert_eq!(
        out[0],
        input
            .iter()
            .map(|&x| f64::from(x as f32))
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_c_entry_point_exchanges_f32_for_a_double_program() {
    // C++ `interpreter_dsp` parity: `FAUSTFLOAT` is `float`, so the constant
    // computed in f64 arrives rounded to the nearest f32
    let constant = Program::new("process = 16777217.0;", &["-double"]);
    let mut out = vec![vec![0.0_f32; 16]];
    constant.compute_c(&mut [], &mut out);
    assert_eq!(out[0], vec![16_777_216.0_f32; 16]);

    let wire = Program::new("process = _ * 3;", &["-double"]);
    let mut out = vec![vec![0.0_f32; 4]];
    wire.compute_c(&mut [vec![0.1_f32, 1.0, -2.0, 1e-3]], &mut out);
    let expected: Vec<f32> = [0.1_f32, 1.0, -2.0, 1e-3]
        .iter()
        .map(|&x| (f64::from(x) * 3.0) as f32)
        .collect();
    assert_eq!(out[0], expected);
}
