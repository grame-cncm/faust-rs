//! `SchedulingStrategy::Special`: C++ `spschedule` / `recschedule` /
//! `recschedulenode` / `interleave`
//! (`compiler/DirectedGraph/Schedule.hh`, `DirectedGraphAlgorythm.hh`).
//!
//! # Compact parity algorithm
//! C++ first builds an unmemoized duplicate list whose size follows the path
//! count, then scans it backwards and retains each node's last occurrence.
//! Materializing that list is exponential on shared DAGs. `run` computes the
//! same last-occurrence positions compositionally: each recursive result is a
//! `(logical length, node -> last position)` summary, and interleaving maps
//! positions exactly without expanding the sequence. The literal algorithm is
//! retained under `cfg(test)` as an executable oracle for equivalence tests.
//!
//! Logical positions use `u128`. If even the logical duplicate sequence would
//! exceed that range, `run` falls back to deterministic DFS. Such a sequence
//! cannot be materialized by the C++/literal algorithm on a `usize` machine;
//! the fallback preserves totality and a valid schedule beyond the parity
//! domain instead of overflowing or hanging.

use ahash::AHashMap;

use super::common::roots_of;
use super::dag::ScheduleDag;

/// C++ `interleave` (`DirectedGraphAlgorythm.hh`): alternates elements of
/// `a` and `b`, then appends the remainder of whichever list is longer.
#[cfg(test)]
pub(crate) fn interleave<N: Copy>(a: &[N], b: &[N]) -> Vec<N> {
    let mut result = Vec::with_capacity(a.len() + b.len());
    for (&x, &y) in a.iter().zip(b.iter()) {
        result.push(x);
        result.push(y);
    }
    let n = a.len().min(b.len());
    result.extend_from_slice(&a[n..]);
    result.extend_from_slice(&b[n..]);
    result
}

/// C++ `recschedulenode`: `rec(v) = [v] ++ fold(interleave, [], [rec(d) | d
/// in deps(v)])`. Literal recursion, matching the C++ recursive function —
/// duplicates on purpose; `run` deduplicates afterward with a reverse scan.
#[cfg(test)]
fn rec_schedule_node<D: ScheduleDag>(dag: &D, n: D::Node) -> Vec<D::Node> {
    let mut acc: Vec<D::Node> = Vec::new();
    for d in dag.dependencies(n) {
        let sub = rec_schedule_node(dag, d);
        acc = interleave(&acc, &sub);
    }
    let mut out = Vec::with_capacity(acc.len() + 1);
    out.push(n);
    out.extend(acc);
    out
}

/// C++ `recschedule`: `raw(G) = fold(interleave, [], [rec(r) | r in
/// roots(G)])` — the duplicate-laden list before the reverse-scan dedup.
/// Exposed (crate-visible) so `tests::growth` can measure its length
/// directly, separately from the deduplicated schedule `run` returns.
#[cfg(test)]
pub(crate) fn raw<D: ScheduleDag>(dag: &D, nodes: &[D::Node]) -> Vec<D::Node> {
    let roots = roots_of(dag, nodes);
    let mut raw: Vec<D::Node> = Vec::new();
    for &root in &roots {
        let q = rec_schedule_node(dag, root);
        raw = interleave(&raw, &q);
    }
    raw
}

#[derive(Clone, Debug)]
struct SequenceSummary<N> {
    len: u128,
    last: AHashMap<N, u128>,
}

impl<N> SequenceSummary<N> {
    fn empty() -> Self {
        Self {
            len: 0,
            last: AHashMap::new(),
        }
    }
}

fn interleaved_position(index: u128, own_len: u128, other_len: u128, right: bool) -> Option<u128> {
    let shared = own_len.min(other_len);
    if index < shared {
        index.checked_mul(2)?.checked_add(u128::from(right))
    } else {
        index.checked_add(shared)
    }
}

fn interleave_summaries<N: Copy + Eq + core::hash::Hash>(
    left: SequenceSummary<N>,
    right: SequenceSummary<N>,
) -> Option<SequenceSummary<N>> {
    let len = left.len.checked_add(right.len)?;
    let mut last = AHashMap::with_capacity(left.last.len() + right.last.len());
    for (node, index) in left.last {
        last.insert(
            node,
            interleaved_position(index, left.len, right.len, false)?,
        );
    }
    for (node, index) in right.last {
        let index = interleaved_position(index, right.len, left.len, true)?;
        last.entry(node)
            .and_modify(|current| *current = (*current).max(index))
            .or_insert(index);
    }
    Some(SequenceSummary { len, last })
}

fn rec_summary<D: ScheduleDag>(
    dag: &D,
    node: D::Node,
    memo: &mut AHashMap<D::Node, SequenceSummary<D::Node>>,
) -> Option<SequenceSummary<D::Node>> {
    if let Some(summary) = memo.get(&node) {
        return Some(summary.clone());
    }

    let mut dependencies = SequenceSummary::empty();
    for dependency in dag.dependencies(node) {
        let child = rec_summary(dag, dependency, memo)?;
        dependencies = interleave_summaries(dependencies, child)?;
    }
    for position in dependencies.last.values_mut() {
        *position = position.checked_add(1)?;
    }
    dependencies.len = dependencies.len.checked_add(1)?;
    dependencies.last.insert(node, 0);
    memo.insert(node, dependencies.clone());
    Some(dependencies)
}

fn compact<D: ScheduleDag>(dag: &D, nodes: &[D::Node]) -> Option<Vec<D::Node>> {
    let roots = roots_of(dag, nodes);
    let mut memo = AHashMap::new();
    let mut summary = SequenceSummary::empty();
    for root in roots {
        summary = interleave_summaries(summary, rec_summary(dag, root, &mut memo)?)?;
    }

    let mut positioned = summary.last.into_iter().collect::<Vec<_>>();
    positioned.sort_unstable_by(|(left_node, left_position), (right_node, right_position)| {
        right_position
            .cmp(left_position)
            .then_with(|| left_node.cmp(right_node))
    });
    Some(positioned.into_iter().map(|(node, _)| node).collect())
}

/// C++ `spschedule`: the reverse order of each node's last occurrence in the
/// logical `raw(G)` sequence, computed without materializing that sequence.
pub(super) fn run<D: ScheduleDag>(dag: &D, nodes: &[D::Node]) -> Vec<D::Node> {
    compact(dag, nodes).unwrap_or_else(|| super::depth_first::run(dag, nodes))
}
