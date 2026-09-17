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
    // large programs (e.g. finite-difference meshes whose route() expands to
    // tens of thousands of connections). 512 MiB is the CLI stack contract for
    // the evaluator's guarded recursion budgets: the release budget of 32 768
    // logical frames costs ~4 KiB of real stack each on the diverging-`case`
    // worst path (see `crates/eval/src/loop_detector.rs`), so 128 MiB may
    // actually be touched; the rest is margin, and an untouched stack is
    // virtual memory that costs nothing. Library embedders that run the
    // compiler on their own threads must provide comparable stack headroom or
    // use a lower evaluator depth budget.
    let outcome = std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(cli::runner::run_main)
        .expect("failed to spawn compiler thread")
        .join();
    if let Err(payload) = outcome {
        // A panic of the compiler thread is an internal error. Its own report
        // was printed by the panic hook when it has one; a typed payload that
        // escaped the boundary meant to catch it has none (the hook of
        // `normalize::DivisionByZero` is silent on purpose), and `expect` would
        // print it as `Any { .. }`, which tells the user nothing.
        eprintln!(
            "faust-rs: internal error: the compiler thread panicked: {}",
            panic_message(payload.as_ref())
        );
        std::process::exit(101);
    }
}

/// What a panic payload says: the message of `panic!`, or for a typed payload
/// the fact that it is one.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a typed payload left the boundary that should have caught it".to_owned()
    }
}

#[cfg(test)]
mod launcher_tests {
    use super::panic_message;

    #[test]
    fn a_panic_payload_is_reported_by_its_message() {
        let from_str = std::panic::catch_unwind(|| panic!("plain message")).unwrap_err();
        assert_eq!(panic_message(from_str.as_ref()), "plain message");
        let from_string = std::panic::catch_unwind(|| panic!("formatted {}", 7)).unwrap_err();
        assert_eq!(panic_message(from_string.as_ref()), "formatted 7");
        let typed = std::panic::catch_unwind(|| std::panic::panic_any(42_u8)).unwrap_err();
        assert!(panic_message(typed.as_ref()).contains("typed payload"));
    }
}
