//! Schedule certificate DTO and independent checker.
//!
//! Vectorization port plan phase R1 (certified plan
//! `lean-rust-certified-porting-plan-2026-07-11-en.md`, section
//! "R1 - Schedule certificate at L2"): "Implement the generic Rust
//! `GraphSnapshot`, `ScheduleCertificate`, and `verify_schedule` before
//! activating generalized `-ss`." [`verify_schedule`](super::verify_schedule)
//! already exists (phase P1); this module adds the canonical, hashable
//! artifact layer around it.
//!
//! Field shapes mirror the `graphSnapshot` / `scheduleCertificate`
//! definitions of
//! `porting/schemas/vector-verification-certificate-v1.schema.json`. Two
//! things are deliberately **not** attempted here, per the certified plan's
//! own phase split:
//!
//! - **No JSON (de)serialization or `artifact_kind` discriminator field.**
//!   R0/R2 own the canonical-JSON / cross-language / multi-artifact-kind
//!   layer ("Complete the deferred canonical boundary here... RV must
//!   already be green before this work becomes a phase blocker" — R2). R1's
//!   own bullet list is about the graph/schedule invariants below, not JSON
//!   framing; a certificate here is identified by its Rust type, not a
//!   runtime tag.
//! - **No `producer`/`program` format validation** (git-commit hex length,
//!   `case_id` path shape). Those are `JSON Schema` string-pattern concerns
//!   (already expressed there); this module keeps them as typed fields for
//!   shape parity but does not re-validate their contents.
//!
//! # Producer/checker separation
//! [`certify_schedule`] is the producer: it calls the (possibly complex,
//! optimized) [`super::schedule`] and packages the result. [`verify_schedule_certificate`]
//! is the checker: it never calls [`super::schedule`] or any of the four
//! literal strategies — it only re-derives facts from the certificate's own
//! fields (recomputing `graph_hash`, checking canonical order, checking
//! `ordered_nodes` against `graph` via the already-independent
//! [`super::verify_schedule`]). A certificate that fails the checker must
//! never reach a later phase.

use std::fmt;

use sha2::{Digest, Sha256};

use super::{ScheduleDag, ScheduleError, SchedulingStrategy, schedule, verify_schedule};
use crate::schedule::VerifyError;

/// Largest value representable as a JSON Schema `uint53`
/// (`2^53 - 1`, the largest integer a canonical JSON reader can round-trip
/// exactly). Node ids, region/epoch ids, and counts are checked against
/// this bound.
pub const MAX_UINT53: u64 = (1u64 << 53) - 1;

impl SchedulingStrategy {
    /// Canonical JSON Schema string for this strategy
    /// (`porting/schemas/vector-verification-certificate-v1.schema.json`,
    /// `$defs/strategy`). The inverse of the CLI-integer mapping
    /// [`SchedulingStrategy::decode`]: this one names the *strategy itself*,
    /// independent of which `-ss` integer selected it (`-ss 3` and `-ss 42`
    /// both canonicalize to `"reverse_breadth_first"`).
    #[must_use]
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::DepthFirst => "depth_first",
            Self::BreadthFirst => "breadth_first",
            Self::Special => "special",
            Self::ReverseBreadthFirst => "reverse_breadth_first",
        }
    }
}

/// `$defs/dependencyEdge`'s `kind` enum. Declared in the schema's own
/// `["data", "effect", "control"]` order so the derived [`Ord`] matches the
/// canonical edge sort key `(consumer, dependency, kind)` (plan
/// §4.3) without a separate ordinal table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    Data,
    Effect,
    Control,
}

impl EdgeKind {
    #[must_use]
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Effect => "effect",
            Self::Control => "control",
        }
    }
}

/// `$defs/dependencyEdge`. `consumer -> dependency`: `dependency` must be
/// scheduled before `consumer` (the same convention as [`ScheduleDag`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DependencyEdge {
    pub consumer: u64,
    pub dependency: u64,
    pub kind: EdgeKind,
}

/// `$defs/graphSnapshot`. A canonical, numerically-identified projection of
/// any [`ScheduleDag`]: `nodes` ascending, `edges` ascending by
/// `(consumer, dependency, kind)`, both duplicate-free when produced by
/// [`certify_schedule`] — but a certificate carrying a *non-canonical*
/// snapshot is a distinct, rejectable artifact (see
/// [`verify_schedule_certificate`]), not silently equivalent to its
/// canonical form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphSnapshot {
    pub nodes: Vec<u64>,
    pub edges: Vec<DependencyEdge>,
}

impl GraphSnapshot {
    /// SHA-256 of the canonical byte encoding of `{"edges": [...], "nodes":
    /// [...]}` (object keys in ascending lexicographic order, matching RFC
    /// 8785 for this fixed, narrow, all-ASCII-key shape), lowercase hex.
    ///
    /// This is a dedicated, hand-written serializer rather than
    /// `serde_json::to_string` — the certified plan is explicit that the
    /// latter is not assumed canonical (RFC 8785 needs sorted keys and no
    /// insignificant whitespace, neither of which `serde_json`'s default
    /// output guarantees). The full cross-language canonical-JSON layer
    /// (arbitrary artifact shapes, Lean-side recomputation, multi-OS byte
    /// identity) is R2 scope; this hash is self-consistent within Rust
    /// today, which is what R1's "graph hash recomputation succeeds" bullet
    /// requires.
    #[must_use]
    pub fn graph_hash(&self) -> String {
        sha256_hex(&self.canonical_bytes())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = String::from("{\"edges\":[");
        for (i, e) in self.edges.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"consumer\":{},\"dependency\":{},\"kind\":\"{}\"}}",
                e.consumer,
                e.dependency,
                e.kind.canonical_name()
            ));
        }
        out.push_str("],\"nodes\":[");
        for (i, n) in self.nodes.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&n.to_string());
        }
        out.push_str("]}");
        out.into_bytes()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

impl ScheduleDag for GraphSnapshot {
    type Node = u64;

    fn nodes(&self) -> Vec<u64> {
        self.nodes.clone()
    }

    fn dependencies(&self, n: u64) -> Vec<u64> {
        self.edges
            .iter()
            .filter(|e| e.consumer == n)
            .map(|e| e.dependency)
            .collect()
    }
}

/// `$defs/scheduleScope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleScope {
    ScalarControl,
    ScalarRegion { region_id: u64 },
    VectorEpoch { epoch_id: u64 },
}

/// `$defs/producer`. See the module docs: format is not re-validated here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Producer {
    pub name: String,
    pub version: String,
    pub git_commit: String,
}

/// `$defs/program`. See the module docs: format is not re-validated here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub case_id: String,
    pub source_sha256: String,
}

/// `$defs/scheduleCertificate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleCertificate {
    pub schema_version: u32,
    pub producer: Producer,
    pub program: Program,
    pub graph: GraphSnapshot,
    pub graph_hash: String,
    pub scope: ScheduleScope,
    pub strategy: SchedulingStrategy,
    pub node_count: u64,
    pub ordered_nodes: Vec<u64>,
}

/// Why [`verify_schedule_certificate`] rejected a certificate. Each variant
/// corresponds to one bullet of the R1 "Required checks" list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertificateError {
    /// `schema_version` is not the one version this module accepts.
    UnsupportedSchemaVersion { found: u32 },
    /// A node, region id, epoch id, or count exceeds [`MAX_UINT53`].
    OutOfRange { value: u64 },
    /// `graph.nodes` is not strictly ascending (this also catches
    /// duplicates: a strictly ascending sequence cannot repeat a value).
    NodesNotCanonical { at: usize },
    /// `graph.edges` is not strictly ascending by
    /// `(consumer, dependency, kind)`.
    EdgesNotCanonical { at: usize },
    /// An edge references a node absent from `graph.nodes`.
    EdgeEndpointMissing { edge: DependencyEdge, missing: u64 },
    /// `node_count` does not match `graph.nodes.len()`.
    NodeCountMismatchGraph { declared: u64, actual: usize },
    /// `node_count` does not match `ordered_nodes.len()`.
    NodeCountMismatchOrder { declared: u64, actual: usize },
    /// `ordered_nodes` is not a valid schedule of `graph` (not a
    /// duplicate-free permutation, or a dependency does not precede its
    /// consumer) — wraps the independent [`VerifyError`] from
    /// [`verify_schedule`], reused rather than re-derived.
    ScheduleInvalid(VerifyError<u64>),
    /// The declared `graph_hash` does not match the hash recomputed from
    /// `graph`.
    GraphHashMismatch {
        declared: String,
        recomputed: String,
    },
}

impl fmt::Display for CertificateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { found } => {
                write!(f, "unsupported schema_version {found} (expected 1)")
            }
            Self::OutOfRange { value } => {
                write!(f, "value {value} exceeds the uint53 bound {MAX_UINT53}")
            }
            Self::NodesNotCanonical { at } => write!(
                f,
                "graph.nodes is not strictly ascending at index {at} (noncanonical or duplicate)"
            ),
            Self::EdgesNotCanonical { at } => write!(
                f,
                "graph.edges is not strictly ascending by (consumer, dependency, kind) at index {at}"
            ),
            Self::EdgeEndpointMissing { edge, missing } => write!(
                f,
                "edge {edge:?} references node {missing}, absent from graph.nodes"
            ),
            Self::NodeCountMismatchGraph { declared, actual } => write!(
                f,
                "node_count {declared} does not match graph.nodes.len() {actual}"
            ),
            Self::NodeCountMismatchOrder { declared, actual } => write!(
                f,
                "node_count {declared} does not match ordered_nodes.len() {actual}"
            ),
            Self::ScheduleInvalid(inner) => {
                write!(f, "ordered_nodes is not a valid schedule of graph: {inner}")
            }
            Self::GraphHashMismatch {
                declared,
                recomputed,
            } => write!(
                f,
                "graph_hash mismatch: declared {declared}, recomputed {recomputed}"
            ),
        }
    }
}

impl std::error::Error for CertificateError {}

/// The producer: runs [`super::schedule`] and packages a canonical
/// [`ScheduleCertificate`]. `node_id` must be injective over `dag.nodes()`
/// (distinct `D::Node`s must map to distinct `u64`s) — violating this is a
/// malformed-caller bug, not a scheduling concern, exactly like
/// [`ScheduleDag`]'s own adapter contract; checked with a `debug_assert`.
///
/// # Errors
/// Whatever [`super::schedule`] returns for a malformed or cyclic `dag`
/// (typed cycle/self-edge errors — the last R1 bullet: "all four strategies
/// return typed cycle/malformed-graph errors").
pub fn certify_schedule<D: ScheduleDag>(
    dag: &D,
    strategy: SchedulingStrategy,
    scope: ScheduleScope,
    producer: Producer,
    program: Program,
    node_id: impl Fn(D::Node) -> u64,
    edge_kind: impl Fn(D::Node, D::Node) -> EdgeKind,
) -> Result<ScheduleCertificate, ScheduleError<D::Node>> {
    let order = schedule(strategy, dag)?;

    let dag_nodes = dag.nodes();
    let mut nodes: Vec<u64> = dag_nodes.iter().map(|&n| node_id(n)).collect();
    nodes.sort_unstable();
    nodes.dedup();
    debug_assert_eq!(
        nodes.len(),
        dag_nodes.len(),
        "node_id must be injective over dag.nodes() (malformed caller, not a scheduling concern)"
    );

    let mut edges: Vec<DependencyEdge> = Vec::new();
    for &n in &dag_nodes {
        for d in dag.dependencies(n) {
            edges.push(DependencyEdge {
                consumer: node_id(n),
                dependency: node_id(d),
                kind: edge_kind(n, d),
            });
        }
    }
    edges.sort_unstable();
    edges.dedup();

    let graph = GraphSnapshot { nodes, edges };
    let graph_hash = graph.graph_hash();
    let node_count = graph.nodes.len() as u64;
    let ordered_nodes: Vec<u64> = order.iter().map(|&n| node_id(n)).collect();

    Ok(ScheduleCertificate {
        schema_version: 1,
        producer,
        program,
        graph,
        graph_hash,
        scope,
        strategy,
        node_count,
        ordered_nodes,
    })
}

/// The independent checker (plan §5.10 `verify_schedule`, R1's own
/// "Required checks" list). Never calls [`super::schedule`] or any of the
/// four literal strategies.
///
/// # Errors
/// The first [`CertificateError`] found, in the order documented on
/// [`CertificateError`]'s variants.
pub fn verify_schedule_certificate(cert: &ScheduleCertificate) -> Result<(), CertificateError> {
    if cert.schema_version != 1 {
        return Err(CertificateError::UnsupportedSchemaVersion {
            found: cert.schema_version,
        });
    }

    for &n in &cert.graph.nodes {
        if n > MAX_UINT53 {
            return Err(CertificateError::OutOfRange { value: n });
        }
    }
    if cert.node_count > MAX_UINT53 {
        return Err(CertificateError::OutOfRange {
            value: cert.node_count,
        });
    }
    let scope_id = match cert.scope {
        ScheduleScope::ScalarControl => None,
        ScheduleScope::ScalarRegion { region_id } => Some(region_id),
        ScheduleScope::VectorEpoch { epoch_id } => Some(epoch_id),
    };
    if let Some(id) = scope_id
        && id > MAX_UINT53
    {
        return Err(CertificateError::OutOfRange { value: id });
    }

    // Canonical node ordering (plan §4.3: "The verifier rejects noncanonical
    // set ordering even when the represented set is equivalent").
    for (i, w) in cert.graph.nodes.windows(2).enumerate() {
        if w[0] >= w[1] {
            return Err(CertificateError::NodesNotCanonical { at: i + 1 });
        }
    }

    // Canonical edge ordering by (consumer, dependency, kind).
    for (i, w) in cert.graph.edges.windows(2).enumerate() {
        if w[0] >= w[1] {
            return Err(CertificateError::EdgesNotCanonical { at: i + 1 });
        }
    }

    // Every edge endpoint belongs to the node set.
    for edge in &cert.graph.edges {
        if cert.graph.nodes.binary_search(&edge.consumer).is_err() {
            return Err(CertificateError::EdgeEndpointMissing {
                edge: *edge,
                missing: edge.consumer,
            });
        }
        if cert.graph.nodes.binary_search(&edge.dependency).is_err() {
            return Err(CertificateError::EdgeEndpointMissing {
                edge: *edge,
                missing: edge.dependency,
            });
        }
    }

    // node_count agrees with both graph and order.
    if cert.node_count != cert.graph.nodes.len() as u64 {
        return Err(CertificateError::NodeCountMismatchGraph {
            declared: cert.node_count,
            actual: cert.graph.nodes.len(),
        });
    }
    if cert.node_count != cert.ordered_nodes.len() as u64 {
        return Err(CertificateError::NodeCountMismatchOrder {
            declared: cert.node_count,
            actual: cert.ordered_nodes.len(),
        });
    }

    // ordered_nodes is a duplicate-free permutation of graph.nodes, and
    // every dependency precedes its consumer — reuse the independent
    // checker rather than re-deriving the same two properties.
    verify_schedule(&cert.graph, &cert.ordered_nodes).map_err(CertificateError::ScheduleInvalid)?;

    // Graph hash recomputation succeeds.
    let recomputed = cert.graph.graph_hash();
    if recomputed != cert.graph_hash {
        return Err(CertificateError::GraphHashMismatch {
            declared: cert.graph_hash.clone(),
            recomputed,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal `ScheduleDag` over `char` nodes for certificate tests.
    struct CharDag {
        nodes: Vec<char>,
        edges: Vec<(char, char)>,
    }

    impl ScheduleDag for CharDag {
        type Node = char;
        fn nodes(&self) -> Vec<char> {
            self.nodes.clone()
        }
        fn dependencies(&self, n: char) -> Vec<char> {
            self.edges
                .iter()
                .filter(|(c, _)| *c == n)
                .map(|(_, d)| *d)
                .collect()
        }
    }

    fn diamond() -> CharDag {
        // 3 -> 1, 3 -> 2, 1 -> 0, 2 -> 0 (same shape as schedule::tests::fixtures::diamond).
        CharDag {
            nodes: vec!['0', '1', '2', '3'],
            edges: vec![('3', '1'), ('3', '2'), ('1', '0'), ('2', '0')],
        }
    }

    fn node_id(c: char) -> u64 {
        u64::from(c as u32)
    }

    fn test_producer() -> Producer {
        Producer {
            name: "faust-rs".to_owned(),
            version: "0.5.0".to_owned(),
            git_commit: "0".repeat(40),
        }
    }

    fn test_program() -> Program {
        Program {
            case_id: "tests/corpus/example.dsp".to_owned(),
            source_sha256: "a".repeat(64),
        }
    }

    fn certify(dag: &CharDag, strategy: SchedulingStrategy) -> ScheduleCertificate {
        certify_schedule(
            dag,
            strategy,
            ScheduleScope::ScalarControl,
            test_producer(),
            test_program(),
            node_id,
            |_, _| EdgeKind::Data,
        )
        .expect("diamond is acyclic")
    }

    #[test]
    fn certify_then_verify_round_trips_for_all_strategies() {
        let dag = diamond();
        for strategy in [
            SchedulingStrategy::DepthFirst,
            SchedulingStrategy::BreadthFirst,
            SchedulingStrategy::Special,
            SchedulingStrategy::ReverseBreadthFirst,
        ] {
            let cert = certify(&dag, strategy);
            assert_eq!(cert.strategy, strategy);
            assert_eq!(cert.node_count, 4);
            assert_eq!(cert.graph.nodes, vec![48, 49, 50, 51]); // ASCII '0'..'3'
            verify_schedule_certificate(&cert)
                .unwrap_or_else(|e| panic!("{strategy:?} certificate must verify: {e}"));
        }
    }

    #[test]
    fn changing_strategy_does_not_change_the_graph_or_its_hash() {
        let dag = diamond();
        let df = certify(&dag, SchedulingStrategy::DepthFirst);
        let sp = certify(&dag, SchedulingStrategy::Special);
        assert_eq!(df.graph, sp.graph);
        assert_eq!(df.graph_hash, sp.graph_hash);
        // DepthFirst gives [0,1,2,3] on the diamond, Special gives [0,2,1,3]
        // (see schedule::tests::exact_orders::diamond_all_four_strategies):
        // a genuinely different, still-valid order under the same graph.
        assert_ne!(
            df.ordered_nodes, sp.ordered_nodes,
            "orders may legitimately differ"
        );
    }

    #[test]
    fn cyclic_graph_is_rejected_by_the_producer_for_every_strategy() {
        let cyclic = CharDag {
            nodes: vec!['a', 'b'],
            edges: vec![('a', 'b'), ('b', 'a')],
        };
        for strategy in [
            SchedulingStrategy::DepthFirst,
            SchedulingStrategy::BreadthFirst,
            SchedulingStrategy::Special,
            SchedulingStrategy::ReverseBreadthFirst,
        ] {
            let err = certify_schedule(
                &cyclic,
                strategy,
                ScheduleScope::ScalarControl,
                test_producer(),
                test_program(),
                node_id,
                |_, _| EdgeKind::Data,
            )
            .expect_err("a 2-cycle must be rejected");
            assert!(matches!(err, ScheduleError::Cycle { .. }));
        }
    }

    #[test]
    fn self_edge_is_rejected_by_the_producer() {
        let selfy = CharDag {
            nodes: vec!['a'],
            edges: vec![('a', 'a')],
        };
        let err = certify_schedule(
            &selfy,
            SchedulingStrategy::DepthFirst,
            ScheduleScope::ScalarControl,
            test_producer(),
            test_program(),
            node_id,
            |_, _| EdgeKind::Data,
        )
        .expect_err("a self-edge must be rejected");
        assert!(matches!(err, ScheduleError::SelfEdge { .. }));
    }

    // ---- negative mutation tests: one per CertificateError variant ----
    // "A checker without a demonstrated rejecting mutation is not complete
    // enough to serve as a trust boundary" (plan §8).

    #[test]
    fn rejects_wrong_schema_version() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        cert.schema_version = 2;
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::UnsupportedSchemaVersion { found: 2 })
        ));
    }

    #[test]
    fn rejects_out_of_range_node() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        cert.graph.nodes[0] = MAX_UINT53 + 1;
        cert.graph.nodes.sort_unstable();
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::OutOfRange { .. })
        ));
    }

    #[test]
    fn rejects_reordered_nodes() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        cert.graph.nodes.swap(0, 1); // still a duplicate-free set, just not ascending
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::NodesNotCanonical { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_node() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        let dup = cert.graph.nodes[1];
        cert.graph.nodes.insert(1, dup);
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::NodesNotCanonical { .. })
        ));
    }

    #[test]
    fn rejects_reordered_edges() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        if cert.graph.edges.len() >= 2 {
            cert.graph.edges.swap(0, 1);
        }
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::EdgesNotCanonical { .. })
        ));
    }

    #[test]
    fn rejects_edge_endpoint_outside_node_set() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        cert.graph.edges[0].dependency = 9999;
        cert.graph.edges.sort_unstable();
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::EdgeEndpointMissing { .. })
        ));
    }

    #[test]
    fn rejects_node_count_mismatch_with_graph() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        cert.node_count += 1;
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::NodeCountMismatchGraph { .. })
        ));
    }

    #[test]
    fn rejects_node_count_mismatch_with_order() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        cert.ordered_nodes.pop();
        cert.node_count = cert.graph.nodes.len() as u64; // keep node_count vs graph consistent
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::NodeCountMismatchOrder { .. })
        ));
    }

    #[test]
    fn rejects_a_consumer_scheduled_before_its_dependency() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        cert.ordered_nodes.reverse(); // '3' (the sole root) now scheduled first
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::ScheduleInvalid(
                VerifyError::OutOfOrder { .. }
            ))
        ));
    }

    #[test]
    fn rejects_altered_graph_hash() {
        let mut cert = certify(&diamond(), SchedulingStrategy::DepthFirst);
        cert.graph_hash = "0".repeat(64);
        assert!(matches!(
            verify_schedule_certificate(&cert),
            Err(CertificateError::GraphHashMismatch { .. })
        ));
    }

    #[test]
    fn graph_hash_is_stable_and_content_addressed() {
        let a = certify(&diamond(), SchedulingStrategy::DepthFirst);
        let mut different = diamond();
        different.edges.push(('2', '1')); // add one edge: a different graph
        let b = certify_schedule(
            &different,
            SchedulingStrategy::DepthFirst,
            ScheduleScope::ScalarControl,
            test_producer(),
            test_program(),
            node_id,
            |_, _| EdgeKind::Data,
        )
        .expect("still acyclic: 2 -> 1 -> 0, 2 -> 0, 3 -> 1, 3 -> 2");
        assert_ne!(a.graph_hash, b.graph_hash);
        assert_eq!(a.graph_hash.len(), 64);
        assert!(
            a.graph_hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn scope_variants_carry_their_ids() {
        let dag = diamond();
        let cert = certify_schedule(
            &dag,
            SchedulingStrategy::DepthFirst,
            ScheduleScope::VectorEpoch { epoch_id: 3 },
            test_producer(),
            test_program(),
            node_id,
            |_, _| EdgeKind::Data,
        )
        .expect("diamond is acyclic");
        assert_eq!(cert.scope, ScheduleScope::VectorEpoch { epoch_id: 3 });
        verify_schedule_certificate(&cert).expect("valid certificate");
    }

    #[test]
    fn strategy_canonical_names_match_the_schema_enum() {
        assert_eq!(
            SchedulingStrategy::DepthFirst.canonical_name(),
            "depth_first"
        );
        assert_eq!(
            SchedulingStrategy::BreadthFirst.canonical_name(),
            "breadth_first"
        );
        assert_eq!(SchedulingStrategy::Special.canonical_name(), "special");
        assert_eq!(
            SchedulingStrategy::ReverseBreadthFirst.canonical_name(),
            "reverse_breadth_first"
        );
    }
}
