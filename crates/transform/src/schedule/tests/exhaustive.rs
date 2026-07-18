//! Item 4: exhaustive enumeration of every upper-triangular DAG on up to six
//! nodes (`n` nodes, edge `i -> j` allowed only when `i > j`, so every
//! subset of the `n*(n-1)/2` possible edges is acyclic by construction — no
//! generated graph needs a cycle guard). For `n = 6` that is `2^15 = 32768`
//! graphs; combined with `n = 1..=5` (`1 + 2 + 8 + 64 + 1024` more graphs)
//! and four strategies each, this is a little over 135000 schedule +
//! `verify_schedule` calls, comfortably inside the plan's 60s budget (a few
//! seconds in a debug build) — full exhaustive coverage up to six nodes, no
//! random sampling needed.

use super::fixtures::{ALL_STRATEGIES, VecDag};
use crate::schedule::{schedule, verify_schedule};

/// Builds the upper-triangular DAG on `n` nodes selected by `edge_mask`:
/// bit `b` (in the fixed enumeration order `(i, j)` for `1 <= i < n`,
/// `0 <= j < i`, `i` outer) turns on edge `i -> j`.
fn upper_triangular(n: u32, edge_mask: u32) -> VecDag {
    let mut g = VecDag::new();
    for k in 0..n {
        g = g.node(k);
    }
    let mut bit = 0u32;
    for i in 1..n {
        for j in 0..i {
            if edge_mask & (1 << bit) != 0 {
                g = g.deps(i, &[j]);
            }
            bit += 1;
        }
    }
    g
}

#[test]
fn every_upper_triangular_dag_up_to_six_nodes_schedules_and_verifies() {
    let mut graphs_checked = 0u64;
    for n in 1u32..=6 {
        let edge_bits = n * n.saturating_sub(1) / 2;
        let total_masks: u32 = 1 << edge_bits;
        for mask in 0..total_masks {
            let g = upper_triangular(n, mask);
            for strategy in ALL_STRATEGIES {
                let order = schedule(strategy, &g).unwrap_or_else(|e| {
                    panic!("n={n} mask={mask:#x} strategy={strategy:?} failed to schedule: {e:?}")
                });
                verify_schedule(&g, &order).unwrap_or_else(|e| {
                    panic!(
                        "n={n} mask={mask:#x} strategy={strategy:?} produced an order \
                         verify_schedule rejects ({order:?}): {e:?}"
                    )
                });
            }
            graphs_checked += 1;
        }
    }
    // n=1..6: 1 + 2 + 8 + 64 + 1024 + 32768 = 33867 graphs.
    assert_eq!(graphs_checked, 33_867);
}
