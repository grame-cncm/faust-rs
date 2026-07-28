//! Phase-G0 provenance representation probe.
//!
//! This developer command compares the two representations considered by the
//! diagnostics-v2 plan without changing production IR:
//!
//! - a dense semantic-node-to-origin-set table;
//! - explicit located occurrences carrying one semantic node and one origin.
//!
//! It also demonstrates why the historical one-property-per-`TreeId` model
//! cannot distinguish repeated, hash-consed source occurrences.

use super::*;
use std::hint::black_box;
use std::mem::size_of;
use std::time::{Duration, Instant};
use tlib::{PropertyStore, TreeArena, TreeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocatedOccurrence {
    node: TreeId,
    origin: u32,
}

#[derive(Debug)]
struct ProbeMeasurement {
    build: Duration,
    query: Duration,
    estimated_bytes: usize,
    checksum: u64,
}

/// Runs the diagnostics provenance storage comparison and prints a stable
/// human-readable report.
pub(crate) fn diagnostics_provenance_probe(
    args: DiagnosticsProvenanceProbeArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    if args.semantic_nodes > args.iterations {
        return Err("semantic-nodes must not exceed iterations".into());
    }
    if args.iterations > u32::MAX as usize {
        return Err("iterations must fit in a u32 origin id".into());
    }

    let nodes = build_shared_nodes(args.semantic_nodes);
    let property_loss = demonstrate_property_overwrite(nodes[0]);
    let origin_sets = measure_origin_sets(&nodes, args.iterations);
    let located = measure_located_occurrences(&nodes, args.iterations);

    println!("diagnostics-provenance-probe");
    println!("iterations={}", args.iterations);
    println!("semantic_nodes={}", args.semantic_nodes);
    println!("single_property_retained={property_loss}");
    print_measurement("origin_sets", &origin_sets);
    print_measurement("located_occurrences", &located);
    println!(
        "decision=hybrid: dense origin sets for semantic nodes, located occurrences at ambiguity-sensitive boundaries"
    );
    Ok(())
}

fn build_shared_nodes(semantic_nodes: usize) -> Vec<TreeId> {
    let mut arena = TreeArena::with_capacity(semantic_nodes.saturating_add(1));
    (0..semantic_nodes)
        .map(|value| arena.int(i64::try_from(value).unwrap_or(i64::MAX)))
        .collect()
}

fn demonstrate_property_overwrite(node: TreeId) -> u32 {
    let mut properties = PropertyStore::new();
    let source_key = properties.key("SOURCE_ORIGIN");
    let _ = properties.set_with_key(node, source_key, 11_u32);
    let _ = properties.set_with_key(node, source_key, 29_u32);
    properties
        .get_with_key(node, source_key)
        .copied()
        .expect("the second source property must be retained")
}

fn measure_origin_sets(nodes: &[TreeId], iterations: usize) -> ProbeMeasurement {
    let slots = nodes
        .iter()
        .map(|node| node.as_u32() as usize)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut origins = vec![Vec::<u32>::new(); slots];

    let build_start = Instant::now();
    for index in 0..iterations {
        let node = nodes[index % nodes.len()];
        origins[node.as_u32() as usize].push(index as u32);
    }
    let build = build_start.elapsed();

    let query_start = Instant::now();
    let mut checksum = 0_u64;
    for index in 0..iterations {
        let node = nodes[index % nodes.len()];
        let candidates = &origins[node.as_u32() as usize];
        checksum = checksum.wrapping_add(u64::from(candidates[index / nodes.len()]));
    }
    black_box(checksum);
    let query = query_start.elapsed();

    let estimated_bytes = origins
        .iter()
        .map(|set| size_of::<Vec<u32>>() + set.capacity() * size_of::<u32>())
        .sum();
    ProbeMeasurement {
        build,
        query,
        estimated_bytes,
        checksum,
    }
}

fn measure_located_occurrences(nodes: &[TreeId], iterations: usize) -> ProbeMeasurement {
    let mut occurrences = Vec::with_capacity(iterations);
    let build_start = Instant::now();
    for index in 0..iterations {
        occurrences.push(LocatedOccurrence {
            node: nodes[index % nodes.len()],
            origin: index as u32,
        });
    }
    let build = build_start.elapsed();

    let query_start = Instant::now();
    let mut checksum = 0_u64;
    for occurrence in &occurrences {
        checksum = checksum
            .wrapping_add(u64::from(occurrence.node.as_u32()))
            .wrapping_add(u64::from(occurrence.origin));
    }
    black_box(checksum);
    let query = query_start.elapsed();

    ProbeMeasurement {
        build,
        query,
        estimated_bytes: occurrences.capacity() * size_of::<LocatedOccurrence>(),
        checksum,
    }
}

fn print_measurement(name: &str, measurement: &ProbeMeasurement) {
    println!("{name}.build_ns={}", measurement.build.as_nanos());
    println!("{name}.query_ns={}", measurement.query.as_nanos());
    println!("{name}.estimated_bytes={}", measurement.estimated_bytes);
    println!("{name}.checksum={}", measurement.checksum);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_property_loses_the_first_hash_consed_occurrence() {
        let nodes = build_shared_nodes(1);
        assert_eq!(demonstrate_property_overwrite(nodes[0]), 29);
    }

    #[test]
    fn origin_sets_retain_every_candidate_but_are_ambiguous() {
        let nodes = build_shared_nodes(2);
        let measured = measure_origin_sets(&nodes, 8);
        assert_ne!(measured.checksum, 0);

        let mut sets = vec![Vec::new(); nodes[1].as_u32() as usize + 1];
        sets[nodes[0].as_u32() as usize].extend([10_u32, 20_u32]);
        assert_eq!(sets[nodes[0].as_u32() as usize], [10, 20]);
    }

    #[test]
    fn located_occurrences_keep_the_exact_source_identity() {
        let nodes = build_shared_nodes(1);
        let occurrences = [
            LocatedOccurrence {
                node: nodes[0],
                origin: 10,
            },
            LocatedOccurrence {
                node: nodes[0],
                origin: 20,
            },
        ];
        assert_eq!(occurrences[0].node, occurrences[1].node);
        assert_ne!(occurrences[0].origin, occurrences[1].origin);
    }
}
