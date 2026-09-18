//! The propagation result memo admits clocked wrappers (2026-09-18): the same
//! wrapper box reached again in the same slot environment, UI path, parent
//! domain and inputs replays the outputs of its first propagation, and with
//! them its clock domain. This is the C++ `makeClockEnv` identity, the tuple
//! `(parent, slotenv, path, box, inputs)`. Before, every reference allocated
//! a fresh domain and re-propagated the body, which made a `fad` over a
//! network held by `op.on_change` under twenty `frame_sum` blocks grow the
//! signal DAG thirty-fold (`porting/journal/2026-09-18.md`).

use compiler::Compiler;

/// Propagates on a large stack: `par` nests its instances to the right and
/// the propagation recurses through them.
fn compile_to_signals(name: &'static str, source: &'static str) -> compiler::SignalCompileOutput {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            Compiler::new()
                .compile_source_to_signals(name, source)
                .unwrap_or_else(|e| panic!("{name} compiles to signals: {e}"))
        })
        .expect("spawn the propagation thread")
        .join()
        .expect("the propagation thread completes")
}

/// The memo activates after a warm-up of 1 024 eligible calls, so the shared
/// box has to be referenced often enough for most references to land after
/// it. `par(i, N, g)` with a `g` that does not mention `i` is the same box
/// in the same context every time.
#[test]
fn a_shared_wrapper_box_is_one_clock_domain_after_the_memo_warms_up() {
    let out = compile_to_signals(
        "shared_od",
        r#"g = (button("c"), _) : ondemand(+ ~ _);
process = _ <: par(i, 600, g) :> _;"#,
    );
    let domains = out.clock_domains.len();
    assert!(
        domains < 600,
        "600 references of one wrapper box must share domains once the memo \
         is warm, got {domains} domains"
    );
    assert!(
        domains >= 1,
        "at least the first reference allocates a domain, got {domains}"
    );
}

/// The identity is contextual, not structural: the same wrapper box under
/// two different inputs is two domains (C++ de Bruijn collision class, plan
/// `ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md` §3.4).
#[test]
fn different_inputs_of_a_shared_wrapper_box_stay_distinct_domains() {
    let out = compile_to_signals(
        "two_inputs_od",
        r#"g = (button("c"), _) : ondemand(+ ~ _);
process = (_, _) : (g, g);"#,
    );
    assert_eq!(out.clock_domains.len(), 2);
}
