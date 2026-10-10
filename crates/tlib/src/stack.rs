//! Deep recursion on trees without overflowing the native stack.
//!
//! The compiler's passes recurse on the structure of the program: a chain
//! `_ : sin : sin : ... : sin` nests one level per term, in every pass from
//! evaluation to code generation. C++ Faust runs each compilation on a thread
//! whose stack it sizes itself (`callFun`, `MAX_STACK_SIZE`). Rust has no
//! portable stack introspection, and a native stack overflow aborts the
//! process, the host process too when `libfaust-rs` is embedded in an
//! application whose thread has 8 MiB.
//!
//! Two mechanisms, both `stacker::maybe_grow` (what rustc uses for its own
//! recursion):
//!
//! - [`on_compile_stack`] at the entry of each stage of the pipeline
//!   (propagation, signal preparation, FIR lowering and verification, each
//!   backend's code generation). A stage entered with less than
//!   [`COMPILE_STACK_RESERVE`] of stack left runs on a fresh stack of
//!   [`COMPILE_STACK_SIZE`], the stack the CLI gives its worker thread. After
//!   evaluation, the depth of the program is bounded by the evaluator's
//!   structural budget, so a fixed stack is enough for every later stage,
//!   whatever the calling thread, and a new stage or backend is covered by
//!   wrapping its entry.
//! - [`on_deep_stack`] inside the recursions that no budget bounds: the
//!   evaluator (its nesting budget is 400 000 entries) and the parser's
//!   import expansion. They continue on 8 MiB heap segments as long as memory
//!   allows.
//!
//! On targets without stack switching (`wasm32`), both run the call in
//! place.

/// Stack kept in reserve by [`on_deep_stack`] before a recursive step
/// continues on a fresh segment: the deepest native call chain between two
/// guarded entries must fit in it, with a wide margin for debug builds.
pub const STACK_RED_ZONE: usize = 256 * 1024;

/// Size of the heap-allocated segments [`on_deep_stack`] grows onto.
pub const STACK_SEGMENT: usize = 8 * 1024 * 1024;

/// Stack a pipeline stage needs at its entry; with less left,
/// [`on_compile_stack`] moves to a fresh stack.
pub const COMPILE_STACK_RESERVE: usize = 256 * 1024 * 1024;

/// Size of the stack a pipeline stage runs on when its caller's is too
/// small: the stack of the CLI's worker thread. It is virtual memory, touched
/// only as deep as the program goes.
pub const COMPILE_STACK_SIZE: usize = 512 * 1024 * 1024;

/// Runs `f`, on a freshly allocated stack segment if the current stack has
/// less than [`STACK_RED_ZONE`] bytes left.
///
/// Its cost is a thread-local read and a pointer comparison per call.
#[inline]
pub fn on_deep_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, f)
}

/// Runs a pipeline stage `f`, on a fresh stack of [`COMPILE_STACK_SIZE`] if
/// the current stack has less than [`COMPILE_STACK_RESERVE`] bytes left.
///
/// In the CLI's worker thread, and in a stage called from another one, it
/// runs in place.
#[inline]
pub fn on_compile_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(COMPILE_STACK_RESERVE, COMPILE_STACK_SIZE, f)
}

#[cfg(test)]
mod tests {
    use super::{on_compile_stack, on_deep_stack};

    fn descend_unguarded(n: u64) -> u64 {
        let pad = std::hint::black_box([n; 32]);
        if n == 0 {
            0
        } else {
            descend_unguarded(n - 1) + pad[(n % 32) as usize] % 2
        }
    }

    fn on_small_thread(f: impl FnOnce() -> u64 + Send + 'static) -> u64 {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(f)
            .expect("spawn")
            .join()
            .expect("the recursion did not overflow")
    }

    #[test]
    fn a_guarded_recursion_runs_deeper_than_its_thread_stack() {
        // 200 000 frames of more than 256 bytes each are well past the 1 MiB
        // stack of the thread
        fn descend(n: u64) -> u64 {
            let pad = std::hint::black_box([n; 32]);
            if n == 0 {
                0
            } else {
                on_deep_stack(|| descend(n - 1) + pad[(n % 32) as usize] % 2)
            }
        }
        assert_eq!(on_small_thread(|| descend(200_000)), 100_000);
    }

    #[test]
    fn a_stage_runs_on_the_compile_stack_from_a_small_thread() {
        // the recursion itself is not guarded: only its entry is
        assert_eq!(
            on_small_thread(|| on_compile_stack(|| descend_unguarded(200_000))),
            100_000
        );
    }
}
