//! End-to-end P6.2 checks that require the compiler propagation boundary.

use compiler::Compiler;
use propagate::ClockDomainTable;
use signals::{SigMatch, match_sig};
use transform::clk_env::annotate;
use transform::signal_fir::decoration_verify::certify_decorations;
use transform::signal_fir::vector_clock_ad::{
    ClockGuard, ForwardAdPolicy, build_vector_clock_ad_plan,
};
use transform::signal_fir::vector_plan::build_vector_plan;
use transform::signal_prepare::prepare_signals_for_fir_verified;

#[test]
fn propagated_fad_is_an_ordinary_vector_signal_graph() {
    let out = Compiler::new()
        .compile_source_to_signals("p6_2_fad.dsp", "process = fad(*, (_,_,_));")
        .expect("pointwise FAD fixture must propagate");
    let prepared = prepare_signals_for_fir_verified(&out.parse.state.arena, &out.signals, &out.ui)
        .expect("expanded FAD signal graph must prepare");
    let empty_domains = ClockDomainTable::new();
    let clocks = annotate(prepared.arena(), &empty_domains, prepared.outputs())
        .expect("pointwise FAD has only the top-rate environment");
    let decorations = certify_decorations(&prepared, &clocks).expect("FAD decorations");
    let vector_plan = build_vector_plan(&decorations, 8).expect("FAD vector plan");
    let p6 = build_vector_clock_ad_plan(&prepared, &empty_domains, &decorations, &vector_plan)
        .expect("FAD is accepted as an expanded signal graph");

    assert_eq!(p6.plan().forward_ad, ForwardAdPolicy::ExpandedSignalGraph);
    assert!(p6.plan().clock_islands.is_empty());
    assert!(p6.plan().reverse_ad_fallbacks.is_empty());
    assert!(prepared.sig_types_map().keys().all(|&signal| !matches!(
        match_sig(prepared.arena(), signal),
        SigMatch::ReverseTimeRec(_) | SigMatch::BlockReverseAD { .. }
    )));
}

#[test]
fn propagated_button_ondemand_uses_boolean_guard() {
    let out = Compiler::new()
        .compile_source_to_signals(
            "p6_2_boolean_od.dsp",
            r#"process = (button("gate"), _) : ondemand(*(2));"#,
        )
        .expect("boolean on-demand fixture must propagate");
    let prepared = prepare_signals_for_fir_verified(&out.parse.state.arena, &out.signals, &out.ui)
        .expect("clocked signal graph must prepare");
    let clocks = annotate(prepared.arena(), &out.clock_domains, prepared.outputs())
        .expect("on-demand graph must have a valid clock environment");
    let decorations = certify_decorations(&prepared, &clocks).expect("clock decorations");
    let vector_plan = build_vector_plan(&decorations, 8).expect("clock vector plan");
    let p6 = build_vector_clock_ad_plan(&prepared, &out.clock_domains, &decorations, &vector_plan)
        .expect("boolean on-demand island must be accepted");

    assert_eq!(p6.plan().clock_islands.len(), 1);
    assert_eq!(
        p6.plan().clock_islands[0].guard,
        ClockGuard::BooleanOnDemand
    );
}
