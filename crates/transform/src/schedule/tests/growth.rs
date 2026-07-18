//! Item 8: `Special`'s growth guardrail (plan §4.1: "benchmark `Special` on
//! path-heavy DAGs before accepting its literal duplicate list construction
//! as production-safe"). `special.rs`'s module docs derive the closed form:
//! a `layers`-layer, two-wide `ladder` (`2 * layers` nodes) produces a
//! duplicate-laden `raw(G)` list of length `2^(layers + 1) - 2`.
//!
//! `layers = 10` (20 nodes, in the plan's "~18-20 nodes" target) gives a
//! `raw(G)` of length `2^11 - 2 = 2046` — this measures and pins that
//! number directly, so a future change to the algorithm's growth shape gets
//! caught here — and completes in low single-digit milliseconds. This is
//! not yet the size where the literal algorithm becomes impractical: at
//! this size `Special` is still cheap. The point of the guardrail is the
//! *closed form*, not this one data point — extrapolating it flags the
//! practical limit for a future caller: `layers = 20` (40 nodes) would
//! already produce a `raw(G)` of length `2^21 - 2`, about two million
//! entries, and `layers = 30` (60 nodes) about two *billion* — squarely
//! impractical. Any caller that might present `Special` with a DAG whose
//! nodes are shared this densely across paths must bound `layers`-like
//! depth. The production implementation must remain compact while preserving
//! the exact order of the literal C++ sequence (`special.rs` module docs).

use std::time::Instant;

use super::fixtures::ladder;
use crate::schedule::{ScheduleDag, SchedulingStrategy, schedule, verify_schedule};

#[test]
fn special_completes_on_a_path_heavy_eighteen_to_twenty_node_ladder() {
    let g = ladder(10); // 20 nodes, per the plan's target size.

    let raw_len = crate::schedule::special::raw(&g, &g.nodes()).len();
    assert_eq!(raw_len, 2_046, "closed form 2^(layers+1) - 2 for layers=10");

    let start = Instant::now();
    let order = schedule(SchedulingStrategy::Special, &g).expect("ladder(10) schedules");
    let elapsed = start.elapsed();

    assert_eq!(order.len(), 20, "dedup must bring it back down to 20 nodes");
    assert!(verify_schedule(&g, &order).is_ok());
    assert!(
        elapsed.as_secs() < 5,
        "special on ladder(10) took {elapsed:?}, expected well under a second"
    );
}

#[test]
fn compact_special_matches_the_literal_sequence_on_shared_dags() {
    for layers in 1..=10 {
        let g = ladder(layers);
        let raw = crate::schedule::special::raw(&g, &g.nodes());
        let mut seen = ahash::AHashSet::new();
        let literal = raw
            .iter()
            .rev()
            .copied()
            .filter(|node| seen.insert(*node))
            .collect::<Vec<_>>();
        let compact = schedule(SchedulingStrategy::Special, &g).expect("ladder schedules");
        assert_eq!(compact, literal, "mismatch at {layers} layers");
    }
}

#[test]
fn compact_special_avoids_path_count_growth() {
    let g = ladder(80);
    let start = Instant::now();
    let order = schedule(SchedulingStrategy::Special, &g).expect("large ladder schedules");
    let elapsed = start.elapsed();

    assert_eq!(order.len(), 160);
    assert!(verify_schedule(&g, &order).is_ok());
    assert!(
        elapsed.as_secs() < 5,
        "compact special on ladder(80) took {elapsed:?}"
    );
}
