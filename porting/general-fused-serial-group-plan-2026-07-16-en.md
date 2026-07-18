# General Fused Serial Group Certificate Plan

Date: 2026-07-16
Status: complete; D3 qualified on 2026-07-17 in `2d0a2a49`
Scope: checked serial fusion for every immediate state-mediated delay crossing

## 1. Objective and baseline

After the general compact-event work, 13 of the 93 impulse DSPs still stop at
`FRS-VEC-FALLBACK-PLAN` with `UnfusedImmediateDelayCrossing`:

- `carre_volterra`, `comb_delay1`, `comb_delay2`, `constant`, `delays`;
- `echo_bug`, `grain3`, `modulations`, `norm3`, `pitch_shifter`;
- `thru_zero_flanger`, `virtual_analog_oscillators`, and `zita_rev1`.

The direct top-rate slice described by
[`vector-fused-recursive-delay-plan-2026-07-15-en.md`](vector-fused-recursive-delay-plan-2026-07-15-en.md)
already preserves one delayed recursive carrier and every same-sample path loop
inside one physical sample iteration. Its producer is seeded primarily from
`DepKind::Delayed` facts, records only one canonical carrier, and excludes all
clocked signals. Live D0 characterization found that 12 first uncovered edges
use ordinary bounded-delay state (`max_delay > 0`) rather than a recursive
projection; `echo_bug` exposes two immediate delayed reads of one recursive
carrier. The remaining corpus therefore needs complete coverage of all checked
state carriers, multiple coupled carriers, longer path closures, and groups
wholly contained in one clock island.

This phase does not weaken `UnfusedImmediateDelayCrossing`. Every dangerous
edge must be covered by a checked group before vector routing starts; otherwise
the compiler retains the same fail-closed scalar fallback.

## 2. Versioned data model

Vector-plan schema v3 replaces the singular recursive-only carrier field with:

```rust
state_carrier_signal_ids: Vec<u64>
```

The array is non-empty, strictly ascending, and contains every `max_delay > 0`
state carrier reached by a delayed read in the group. Recursive projections
are one supported carrier class, not a prerequisite. `owner_loop_id` remains
the canonical minimum carrier owner used for stable group identity. Existing
fields retain their meanings:

- `member_loop_ids`: exact loop closure emitted in one sample-time unit;
- `delayed_read_signal_ids`: all delayed reads that seed the closure;
- `state_write_signal_ids`: every listed carrier plus every recursive
  projection written by a recursive member loop;
- `internal_transport_ids`: every planned transport whose producer and
  consumer are both group members, rematerialized as a scalar value;
- `output_or_transport_roots`: the complete owned-root envelope.

A versioned `vector-verification-certificate-v3.schema.json` documents the
finite DTO shape. Rust remains authoritative for graph, recursion, delay, and
clock obligations.

## 3. Producer construction

The producer derives groups only from the accepted decoration certificate and
the strategy-independent plan graph.

1. Reconstruct every dangerous immediate delay edge using the same certified
   occurrence-delay relation that feeds `immediate_delay_edges`.
2. For each edge, identify every delayed-read/carrier pair, its read loop, its
   state-writer loop, and its exact nonzero clock id (zero means top rate).
3. Compute the complete same-sample path closure between the carrier writer and
   delayed-read consumer for immediate state crossings, and from read to writer
   for delayed recursive dependencies. Paths may contain any finite number of
   pure or serial loops.
4. Merge closures that overlap, share a carrier, or are connected by another
   dangerous crossing. Union every carrier, read, recursive writer, root, and
   internal transport.
5. Admit the component only when every member and grouped signal has the same
   clock id. A nonzero id means all members belong to one exact clock island;
   parent/child or sibling island crossings are not approximated.
6. Canonicalize groups and all set-like fields by stable numeric identity.

The requested vector size, loop identities, transport identities, and
scheduling-strategy independence remain unchanged.

## 4. Independent checker obligations

The checker must not call the producer or consume producer discovery state. It
reconstructs from decorations plus the accepted raw plan:

1. the full dangerous immediate-delay edge set;
2. every delayed-read/state-carrier relation and any recursive projection
   group;
3. the complete read-to-writer path closure, including arbitrary chain length;
4. connected components formed by overlapping closures and carriers;
5. the exact carrier, read, writer, member, root, and internal-transport sets;
6. one common clock id for every member-owned signal and every grouped signal;
7. coverage of every dangerous edge by exactly one canonical group.

It rejects missing or extra carriers, a carrier without `max_delay > 0`,
truncated paths, omitted transports, overlapping groups, incompatible clocks,
a recursive member without its writer, and any delayed read without a positive
certified occurrence-delay or delayed dependency to a listed carrier. The
checker continues to run after the ordinary finite-shape plan verifier and
before routing.

## 5. Routing, state, and FIR assembly

Top-rate groups keep the existing lowering shape: all member iteration blocks
are concatenated under one physical `i0` loop, with state copy-in/register load
before it and copy-out/register store after it.

For a group with one nonzero clock id:

- the clock-plan checker must place all members in exactly one
  `ClockIsland::nested_loop_ids` set;
- internal transports use `FusedScalar` storage even though their values are
  clock-local;
- the island guard contains the member iteration blocks in checked scheduled
  order during the same outer sample step;
- no member is emitted as a separate whole-chunk loop;
- delayed reads, all corresponding state writes, and every internal scalar
  store/load must occur inside that guarded sample-time body.

The assembled-FIR checker independently distinguishes top-rate and island
groups. It verifies one physical sample envelope, pre/post state-action
placement, all delayed read definitions, every listed carrier write, scalar
transport rematerialization, and exact island ownership. A group spanning
multiple islands or escaping its guarded body is rejected.

## 6. Rejecting mutations and focused tests

At minimum, tests must reject:

- a missing, duplicated, reordered, or extra carrier id;
- one dangerous edge absent from every group;
- a missing intermediate loop in a three-or-more-loop path;
- one omitted internal transport;
- one recursive member without its projection writer;
- a carrier/read/writer moved to another clock id;
- a same-domain group moved across two clock islands;
- an assembled delayed read, state write, or scalar transport moved outside the
  fused sample-time body.

Positive structural fixtures must include a multi-carrier component, a long
transport chain, two overlapping dangerous crossings that merge into one
group, and a group wholly contained in one clock island. Production tests must
also require the 13 baseline PLAN cases to pass the plan gate in both loop
variants and all four scheduling strategies.

## 7. Rollout

### D0 — plan and characterization

- [x] Freeze this certificate and checker contract before implementation.
- [x] Record the exact 13-case baseline and each first uncovered edge.

### D1 — schema, producer, and independent checker

- [x] Introduce vector-plan schema v3 and complete state-carrier sets.
- [x] Rebuild components from all dangerous edges and arbitrary path closures.
- [x] Add the independent reconstruction and rejecting mutations.

### D2 — same-island routing and assembled FIR

- [x] Permit `FusedScalar` transports within one exact clock domain.
- [x] Assemble and independently verify top-rate and same-island fused bodies.
- [x] Add focused structural and end-to-end tests for multi-carrier, long-chain, and
  clock-island shapes.

### D3 — corpus qualification

- Sweep all 16 precision/loop/scheduling coverage modes.
- Record each former PLAN case as certified or as a later explicit fail-closed
  reason; no remaining case may retain `UnfusedImmediateDelayCrossing` when its
  obligations are representable.
- Refresh the versioned coverage baseline and universal certified list.
- Run scalar `-ss 0..3` plus vector `-lv 0/1 x -ss 0..3` native C++ impulse
  comparisons at 60,000 frames for every newly certified DSP.

## 8. Acceptance gates

Phase D is complete only when:

- formatting, warning-denied workspace Clippy, and all workspace tests pass;
- Rust golden output remains byte-identical unless a separate refresh is
  explicitly approved;
- all 16 vector-coverage modes pass against the refreshed baseline;
- `vector-interp-opt-check` and the release compile-budget gate pass;
- every newly certified DSP passes the complete native C++ impulse matrix;
- producer/checker mutations fail closed and assembled FIR proves the required
  physical sample envelope;
- before/after coverage and every deviation from the 13-case target are
  recorded in the daily English journal.

## 9. Risks and mitigations

- **Incomplete carrier closure:** make generic state-carrier sets explicit in
  schema v3 and compare them with an independently reconstructed set.
- **Unsound long-chain fusion:** require exact graph reachability closure, not
  only direct transported reads.
- **Clock-rate mismatch:** admit only one exact clock id and require one exact
  island owner at the clock-plan and FIR boundaries.
- **Transport escaping the serial envelope:** list every internal transport and
  require scalar store/load nodes inside the fused body.
- **Coverage pressure weakening checks:** retain the existing PLAN fallback for
  every component whose complete obligations cannot be established.
