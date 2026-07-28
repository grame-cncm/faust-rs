//! Micro-benchmark for `tlib::TreeArena` interning and property operations.
//!
//! # Scope
//! - Exercises symbol/tag/list interning workloads under configurable sizes.
//! - Measures baseline timings for creation and lookup patterns.
//! - Supports optional preallocation mode (`--prealloc`) for capacity studies.

use std::hint::black_box;
use std::time::Instant;

use clap::Parser;
use tlib::{NodeKind, PropertyStore, TreeArena};

#[derive(Debug, Parser)]
#[command(
    name = "treearena_bench",
    about = "Benchmark TreeArena interning and property operations"
)]
struct CliArgs {
    /// Number of nodes in each benchmark phase.
    #[arg(default_value_t = 200_000)]
    nodes: usize,

    /// Reserve the expected arena and property capacities up front.
    #[arg(long)]
    prealloc: bool,
}

fn make_arena(n: usize, prealloc: bool) -> TreeArena {
    if !prealloc {
        return TreeArena::new();
    }
    // For this benchmark workload:
    // nodes ~= nil + ints(n+1) + pairs(n) + cons(n) = 3n + 2
    // arity0 ~= nil + ints = n + 2
    // arity2 (phase 1) ~= pairs = n
    TreeArena::with_capacities(
        n.saturating_mul(3).saturating_add(2),
        n.saturating_add(2),
        0,
        n.saturating_add(2),
        0,
    )
}

fn main() {
    let args = CliArgs::parse();
    let n = args.nodes;
    let prealloc = args.prealloc;

    let mut arena = make_arena(n, prealloc);
    let mut nodes = Vec::with_capacity(n);
    let pair_tag_id = arena.intern_tag("pair");
    let pair_kind = NodeKind::Tag(pair_tag_id);

    let create_start = Instant::now();
    for i in 0..n {
        let a = arena.int(i as i64);
        let b = arena.int((i as i64) + 1);
        let node = arena.intern(pair_kind.clone(), &[a, b]);
        nodes.push(node);
    }
    let create_elapsed = create_start.elapsed();

    let lookup_start = Instant::now();
    for i in 0..n {
        let a = arena.int(i as i64);
        let b = arena.int((i as i64) + 1);
        let node = arena.intern(pair_kind.clone(), &[a, b]);
        black_box(node);
    }
    let lookup_elapsed = lookup_start.elapsed();

    if prealloc {
        // Phase 2 adds n extra arity-2 cons nodes.
        arena.reserve(0, 0, 0, n.saturating_add(2), 0);
    }

    let traversal_start = Instant::now();
    let mut list = arena.nil();
    for node in nodes.iter().copied() {
        list = arena.cons(node, list);
    }
    let mut count = 0usize;
    let mut cur = list;
    while !arena.is_nil(cur) {
        count += 1;
        cur = arena.tl(cur).expect("proper cons list");
    }
    let traversal_elapsed = traversal_start.elapsed();
    black_box(count);

    let mut props = if prealloc {
        PropertyStore::<usize>::with_key_capacity(1)
    } else {
        PropertyStore::<usize>::new()
    };
    let hot_key = props.key("hot");
    if prealloc {
        props.reserve_slots(hot_key, arena.len());
    }
    let prop_set_start = Instant::now();
    for (i, node) in nodes.iter().copied().enumerate() {
        let _ = props.set_with_key(node, hot_key, i);
    }
    let prop_set_elapsed = prop_set_start.elapsed();

    let prop_get_start = Instant::now();
    let mut checksum = 0usize;
    for node in nodes.iter().copied() {
        if let Some(v) = props.get_with_key(node, hot_key) {
            checksum ^= *v;
        }
    }
    let prop_get_elapsed = prop_get_start.elapsed();
    black_box(checksum);

    println!("TreeArena micro-bench (n={n})");
    println!("prealloc={prealloc}");
    println!("create_ms={:.3}", create_elapsed.as_secs_f64() * 1_000.0);
    println!("lookup_ms={:.3}", lookup_elapsed.as_secs_f64() * 1_000.0);
    println!(
        "traversal_ms={:.3}",
        traversal_elapsed.as_secs_f64() * 1_000.0
    );
    println!(
        "property_set_ms={:.3}",
        prop_set_elapsed.as_secs_f64() * 1_000.0
    );
    println!(
        "property_get_ms={:.3}",
        prop_get_elapsed.as_secs_f64() * 1_000.0
    );
    println!("arena_nodes={}", arena.len());
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::CliArgs;

    #[test]
    fn cli_preserves_defaults_and_option_order() {
        CliArgs::command().debug_assert();
        let defaults = CliArgs::try_parse_from(["treearena_bench"]).unwrap();
        assert_eq!(defaults.nodes, 200_000);
        assert!(!defaults.prealloc);

        let reordered = CliArgs::try_parse_from(["treearena_bench", "--prealloc", "12"]).unwrap();
        assert_eq!(reordered.nodes, 12);
        assert!(reordered.prealloc);
    }
}
