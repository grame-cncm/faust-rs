//! A `-double` program run through the C entry point converts its samples
//! through buffers the instance keeps: once they have grown to the largest
//! block, a compute call allocates nothing that depends on the frame count.
//! It used to allocate one `f64` vector per channel on every call, in the
//! audio thread.
//!
//! What a call still allocates, whatever the block size, is the executor's
//! evaluation stacks (`FbcExecutor::execute_block*`), the same for a
//! single-precision program: a separate follow-up.
//!
//! A test binary of its own: the counting allocator is global.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::{CString, c_char};

use faust_interp::factory::{createCInterpreterDSPFactoryFromString, deleteCInterpreterDSPFactory};
use faust_interp::instance::{
    computeCInterpreterDSPInstance, createCInterpreterDSPInstance, deleteCInterpreterDSPInstance,
    initCInterpreterDSPInstance,
};

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

#[test]
fn a_double_program_s_c_compute_reuses_its_conversion_buffers() {
    const LARGE: usize = 8192;
    const SMALL: usize = 4096;
    let name = CString::new("allocation").unwrap();
    let code = CString::new("process = _, _ :> _ * 0.5 <: _, _;").unwrap();
    let double = CString::new("-double").unwrap();
    let argv = [double.as_ptr()];
    let mut error = [0 as c_char; 4096];
    let mut inputs = vec![vec![0.25_f32; LARGE]; 2];
    let mut outputs = vec![vec![0.0_f32; LARGE]; 2];
    unsafe {
        let factory = createCInterpreterDSPFactoryFromString(
            name.as_ptr(),
            code.as_ptr(),
            1,
            argv.as_ptr(),
            error.as_mut_ptr(),
        );
        assert!(!factory.is_null());
        let dsp = createCInterpreterDSPInstance(factory);
        initCInterpreterDSPInstance(dsp, 48_000);
        let mut ins: Vec<*mut f32> = inputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let mut outs: Vec<*mut f32> = outputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let mut compute = |frames: usize| {
            allocated_by(|| {
                computeCInterpreterDSPInstance(
                    dsp,
                    frames as i32,
                    ins.as_mut_ptr(),
                    outs.as_mut_ptr(),
                )
            })
        };

        let first = compute(LARGE);
        let large = compute(LARGE);
        let small = compute(SMALL);
        assert!(
            first >= large + 4 * LARGE * 8,
            "the first block sizes the four f64 buffers: {first} bytes, then {large}"
        );
        assert_eq!(
            large, small,
            "a later block allocated in proportion to its frame count"
        );

        deleteCInterpreterDSPInstance(dsp);
        deleteCInterpreterDSPFactory(factory);
    }
    assert!(outputs.iter().all(|c| c.iter().all(|&y| y == 0.25)));
}
