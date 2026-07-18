//! Generic scheduler contract: [`ScheduleDag`].

/// A finite dependency DAG adapter consumed by [`crate::schedule::schedule`].
///
/// # Edge convention
/// An edge `consumer -> dependency` (i.e. `dependencies(consumer)` contains
/// `dependency`) means **the dependency must be scheduled before the
/// consumer**. This is the C++ `digraph<N>::destinations` convention used
/// throughout `compiler/DirectedGraph/*`: `G.destinations(n)` lists what `n`
/// depends on, not what depends on `n`. A **root** is a terminal consumer
/// with no incoming edge (nothing depends on it) — C++ `roots(G)`
/// (`DirectedGraphAlgorythm.hh`).
///
/// # Contract adapters must uphold
/// - `nodes()` returns every node of the DAG **exactly once**, in stable
///   ascending order (`Ord` on [`Self::Node`]). This is the tie-break used
///   to pick roots (for `DepthFirst`/`Special`) and to resolve
///   `BreadthFirst`/`ReverseBreadthFirst` level ties.
/// - `dependencies(n)` returns `n`'s dependency targets in a stable,
///   adapter-defined order (not required to be ascending — an adapter may
///   preserve a semantically meaningful child order, e.g. operand order).
///   Every returned node must itself appear in `nodes()`: a dependency
///   outside the node set is a malformed adapter, not a scheduler concern
///   (see the `hgraph::Digraph` adapter's `contains` filter for an adapter
///   that enforces this by construction, dropping foreign/placement-only
///   edges before they reach the generic scheduler).
/// - Both methods are deterministic: equal receivers return equal `Vec`s
///   across calls, and no `HashMap`/`HashSet` iteration order may leak
///   through either method.
pub trait ScheduleDag {
    /// Node identifier. `Copy` keeps the adapter cheap to query repeatedly;
    /// `Ord` is the stable tie-break key; `Hash`/`Eq` back the algorithms'
    /// internal visited/seen sets.
    type Node: Copy + Ord + Eq + core::hash::Hash + core::fmt::Debug;

    /// Every node of the DAG, stable ascending order.
    fn nodes(&self) -> Vec<Self::Node>;

    /// `n`'s dependency targets (must be scheduled before `n`), stable order.
    fn dependencies(&self, n: Self::Node) -> Vec<Self::Node>;
}
