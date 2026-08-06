//! The box-simplification memo must not survive its arena.
//!
//! Obligation V3 of
//! `porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`.
//! `box_simplification` memoizes on the arena so a subtree is simplified once
//! per compilation. `TreeId`s are indices into the arena that issued them, so a
//! memo outliving one compilation answers a later one with unrelated trees —
//! and does it silently, returning a well-formed tree that means something
//! else.
//!
//! Nothing else in the suite covers this. Every backend test, the golden
//! snapshots and the whole impulse corpus compile one program per process, and
//! a leaking memo is invisible to all of them; the CLI is one compilation per
//! process too. Only compiling more than once in a single process can tell an
//! arena-scoped memo from a process-scoped one.
//!
//! The mutation this test exists to reject is the `thread_local` memo that the
//! analysis used as its measuring instrument (§2.3). Under it,
//! `xtask compile-profile` — 133 compilations in one process — dies with a
//! stack overflow.

use compiler::{Compiler, SignalFirLane};
use fir::dump_fir;
use std::path::{Path, PathBuf};

/// Corpus DSPs rather than toy sources, because toys do not discriminate.
///
/// Small programs build small arenas whose low `TreeId`s are canonical things
/// — `nil`, small integers — that simplify to themselves in any arena, so a
/// leaked memo returns the right answer by accident. Four hand-written pairs
/// were tried under the mutation below and none of them noticed. These two do:
/// `freeverb` memoizes enough nodes that `spectral_tilt`'s arena then reads
/// entries belonging to trees it never built.
const OTHER: &str = "freeverb.dsp";
const SUBJECT: &str = "spectral_tilt.dsp";

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/impulse-tests/dsp")
        .canonicalize()
        .expect("the impulse corpus must exist")
}

fn compile(name: &str) -> String {
    let dir = corpus_dir();
    let search = vec![dir.clone(), PathBuf::from("/usr/local/share/faust")];
    let fir = Compiler::new()
        .compile_file_to_fir_with_lane(&dir.join(name), &search, SignalFirLane::TransformFastLane)
        .unwrap_or_else(|error| panic!("{name} must compile: {error}"));
    dump_fir(&fir.store, fir.module)
}

/// Runs `body` on one worker carrying the compiler's stack contract.
///
/// Every compilation in a scenario must share that worker. An earlier version
/// gave each compilation its own thread and the mutation stopped being
/// detected: a `thread_local` memo is per-thread, so thread-per-compile
/// isolates exactly the leak the test exists to catch. The stack size is
/// needed because these are real library-using DSPs; the sharing is what makes
/// the test mean anything.
fn on_one_worker<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(body)
        .expect("worker must spawn")
        .join()
        .expect("compilation must not panic")
}

#[test]
fn a_compilation_is_unaffected_by_the_one_before_it() {
    // Subject, other, subject — all in this process. Framing it that way
    // rather than other-then-subject makes the assertion independent of
    // whatever ran earlier in this test binary: the two subject results must
    // agree with each other whatever the process did beforehand.
    //
    // Under a process-scoped memo this does not fail its assertion — it aborts
    // with a stack overflow, because the entries it reads are indices into an
    // arena that no longer exists and the evaluator recurses through whatever
    // they happen to address. A crash is the honest manifestation and still a
    // rejected mutation; the assertion below covers the rarer case where the
    // wrong tree is merely wrong rather than malformed.
    let (first, second) = on_one_worker(|| {
        let first = compile(SUBJECT);
        let _ = compile(OTHER);
        (first, compile(SUBJECT))
    });

    assert_eq!(
        first, second,
        "compiling another program in between changed the result; a per-node \
         memo has outlived its arena and is answering with another \
         compilation's trees"
    );
}

#[test]
fn repeated_compilation_of_one_program_is_stable() {
    // The degenerate case of the same obligation: a memo surviving into a
    // fresh arena for the *same* source is still keyed by ids that arena
    // reissued independently.
    let runs: Vec<String> = on_one_worker(|| (0..3).map(|_| compile(SUBJECT)).collect());
    assert_eq!(runs[0], runs[1], "second compilation diverged");
    assert_eq!(runs[1], runs[2], "third compilation diverged");
}
