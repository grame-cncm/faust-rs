//! After its first block, `Dsp::compute` over `f32` and over `f64` allocates
//! nothing, on both backends, with buffers of the exchanged width or of the
//! other one: a host may call them from an audio thread.
//!
//! A test binary of its own: the counting allocator is global.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use faust::{Backend, CompileOptions, Factory, Precision};

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
fn compute_allocates_nothing_after_the_first_block() {
    const FRAMES: usize = 1024;
    let program = "process = par(i, 2, (+ ~ *(0.5)) : @(7));";
    for backend in [Backend::Interp, Backend::Cranelift] {
        for precision in [Precision::F32, Precision::F64] {
            let options = CompileOptions {
                backend,
                precision,
                ..CompileOptions::default()
            };
            let factory = Factory::from_source("alloc", program, &options).unwrap();
            let mut dsp = factory.create_dsp_instance(48_000).unwrap();
            let what = format!("{backend} {precision:?}");

            let in32 = vec![vec![0.25_f32; FRAMES]; 2];
            let mut out32 = vec![vec![0.0_f32; FRAMES]; 2];
            let in64 = vec![vec![0.25_f64; FRAMES]; 2];
            let mut out64 = vec![vec![0.0_f64; FRAMES]; 2];
            let ins32: Vec<&[f32]> = in32.iter().map(Vec::as_slice).collect();
            let ins64: Vec<&[f64]> = in64.iter().map(Vec::as_slice).collect();
            // the first block sizes the conversion buffers of the other width
            {
                let mut outs32: Vec<&mut [f32]> = out32.iter_mut().map(Vec::as_mut_slice).collect();
                let mut outs64: Vec<&mut [f64]> = out64.iter_mut().map(Vec::as_mut_slice).collect();
                dsp.compute(FRAMES, &ins32, &mut outs32).unwrap();
                dsp.compute(FRAMES, &ins64, &mut outs64).unwrap();
            }
            let mut outs32: Vec<&mut [f32]> = out32.iter_mut().map(Vec::as_mut_slice).collect();
            let mut outs64: Vec<&mut [f64]> = out64.iter_mut().map(Vec::as_mut_slice).collect();
            let bytes32 = allocated_by(|| dsp.compute(FRAMES, &ins32, &mut outs32).unwrap());
            let bytes64 = allocated_by(|| dsp.compute(FRAMES, &ins64, &mut outs64).unwrap());
            assert_eq!(
                (bytes32, bytes64),
                (0, 0),
                "{what}: bytes allocated by a later block"
            );
        }
    }
}
