//! `faust-rs` CLI launcher.

// The system allocator is the single largest cost in this compiler on macOS.
//
// Propagation makes ~30 million `propagate_inner` calls on a 331-line DSP, each
// returning a `Vec<SigId>` whose mean length is 1.19 — 83 % hold exactly one
// signal. That churn puts the platform allocator at roughly half of propagation
// self time; swapping it takes `virtualAnalogForBrowser.dsp` from 13.4 s to
// 7.6 s and the impulse corpus from 1.21x the C++ reference to 0.82x, with
// byte-identical output.
//
// This is the binary's choice, not the library's: `compiler` deliberately does
// not carry `mimalloc` as an ordinary dependency, because a library must not
// impose an allocator on its consumers. FFI embedders keep their own, and for
// them the underlying fix — not allocating 30 million one-element vectors —
// still matters. See `porting/propagation-cost-analysis-2026-08-06-en.md` §8.
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cli;

fn main() {
    // The evaluator's structural-lowering pass (`a2sb`) can recurse deeply for
    // large programs (e.g. auto-panning with many channels). 64 MiB is the CLI
    // stack contract for the evaluator's guarded recursion budgets; library
    // embedders that run the compiler on their own threads must provide
    // comparable stack headroom or use a lower evaluator depth budget.
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(cli::runner::run_main)
        .expect("failed to spawn compiler thread")
        .join()
        .expect("compiler thread panicked");
}
