//! After its first block, a compute call allocates nothing: the conversion
//! buffers of a `-double` program behind the C entry point are kept by the
//! instance (they used to be allocated on every call, one `f64` vector per
//! channel), the executor keeps its evaluation stacks (they used to be
//! allocated for every block executed, twice per call), and the channel slice
//! lists are gathered on the stack.
//!
//! A test binary of its own: the counting allocator is global.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::{CString, c_char};

use faust_interp::factory::{createCInterpreterDSPFactoryFromString, deleteCInterpreterDSPFactory};
use faust_interp::instance::{
    compute_f64, computeCInterpreterDSPInstance, createCInterpreterDSPInstance,
    deleteCInterpreterDSPInstance, initCInterpreterDSPInstance,
};
use faust_interp::types::{InterpreterDspFactory, InterpreterDspInstance};

/// Counts the bytes allocated by the current thread while `COUNTING` is set.
struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCATED.with(|a| a.set(a.get() + layout.size()));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Bytes allocated by `f` on this thread.
fn allocated_by(f: impl FnOnce()) -> usize {
    ALLOCATED.with(|a| a.set(0));
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    ALLOCATED.with(Cell::get)
}

/// A stateful stereo program (a recursion and a delay line), 2 in, 2 out.
const PROGRAM: &str = "process = par(i, 2, (+ ~ *(0.5)) : @(7));";

const LARGE: usize = 8192;
const SMALL: usize = 4096;

/// Compiles `PROGRAM` with `args` and returns an initialised instance.
fn instance(args: &[&str]) -> (*mut InterpreterDspFactory, *mut InterpreterDspInstance) {
    let name = CString::new("allocation").unwrap();
    let code = CString::new(PROGRAM).unwrap();
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
        assert!(!factory.is_null());
        let dsp = createCInterpreterDSPInstance(factory);
        initCInterpreterDSPInstance(dsp, 48_000);
        (factory, dsp)
    }
}

fn delete((factory, dsp): (*mut InterpreterDspFactory, *mut InterpreterDspInstance)) {
    unsafe {
        deleteCInterpreterDSPInstance(dsp);
        deleteCInterpreterDSPFactory(factory);
    }
}

/// Bytes allocated by the first block, then by a later large and small one,
/// through the C entry point.
fn c_entry_allocations(args: &[&str]) -> [usize; 3] {
    let (factory, dsp) = instance(args);
    let mut inputs = vec![vec![0.25_f32; LARGE]; 2];
    let mut outputs = vec![vec![0.0_f32; LARGE]; 2];
    let mut ins: Vec<*mut f32> = inputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
    let mut outs: Vec<*mut f32> = outputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
    let mut compute = |frames: usize| {
        allocated_by(|| unsafe {
            computeCInterpreterDSPInstance(dsp, frames as i32, ins.as_mut_ptr(), outs.as_mut_ptr())
        })
    };
    let bytes = [compute(LARGE), compute(LARGE), compute(SMALL)];
    delete((factory, dsp));
    assert!(outputs.iter().all(|c| c.iter().any(|&y| y != 0.0)));
    bytes
}

#[test]
fn a_double_program_s_c_compute_allocates_nothing_after_its_first_block() {
    let [first, large, small] = c_entry_allocations(&["-double"]);
    assert!(
        first >= 4 * LARGE * 8,
        "the first block sizes the four f64 conversion buffers: {first} bytes"
    );
    assert_eq!((large, small), (0, 0), "a later block allocated");
}

#[test]
fn a_single_program_s_c_compute_allocates_nothing_after_its_first_block() {
    let [_, large, small] = c_entry_allocations(&[]);
    assert_eq!((large, small), (0, 0), "a later block allocated");
}

#[test]
fn compute_f64_allocates_nothing_after_its_first_block() {
    for args in [&["-double"][..], &[][..]] {
        let (factory, dsp) = instance(args);
        let inputs = vec![vec![0.25_f64; LARGE]; 2];
        let mut outputs = vec![vec![0.0_f64; LARGE]; 2];
        let ins: Vec<*const f64> = inputs.iter().map(|c| c.as_ptr()).collect();
        let outs: Vec<*mut f64> = outputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let compute = |frames: usize| {
            allocated_by(|| unsafe { compute_f64(dsp, frames as i32, ins.as_ptr(), outs.as_ptr()) })
        };
        let [_, large, small] = [compute(LARGE), compute(LARGE), compute(SMALL)];
        delete((factory, dsp));
        assert_eq!((large, small), (0, 0), "{args:?}: a later block allocated");
    }
}
