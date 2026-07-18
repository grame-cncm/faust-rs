# Plan - Certified Serial Fusion for Recursive Delay Reads in Vector Mode

Date: 2026-07-15

Status: implemented for the direct top-rate fusion slice

Working branch: `ondemand-vec-fad-synthesis`

Related documents:

- [`vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`](vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md)
- [`lean-rust-certified-porting-plan-2026-07-11-en.md`](lean-rust-certified-porting-plan-2026-07-11-en.md)
- [`ondemand-vec-fad-implementation-roadmap-2026-06-10-en.md`](ondemand-vec-fad-implementation-roadmap-2026-06-10-en.md)
- [`schemas/vector-verification-certificate-v1.schema.json`](schemas/vector-verification-certificate-v1.schema.json)

## 1. Problem

The following faustlibraries test exposes a case that the current vector
pipeline does not yet cover safely:

```faust
ba = library("basics.lib");
process = ba.pulse_countup_loop(4, 1) + 0.001;
```

Under `-vec -lv 1 -ss 3`, the vector plan can produce three loops:

```text
loop 2: reads the recursive delay and fills transport_s23_l2_l1
loop 1: computes the recurrence and writes vstate_s22_tmp
loop 0: reads transport_s58_l1_l0 and writes the output
```

This order is semantically wrong even when `DelayCopyIn` is emitted before the
chunk loop bodies. The delayed read is precomputed for the whole chunk before
the intra-chunk recurrence writes have happened. The vector result can
therefore read stale history or uninitialized temporary storage, which was
observed as `-inf`.

Commit `b5a0a8b3` added a conservative guard: when a cross-loop transport
depends on a delayed recursive carrier, the checked vector pipeline rejects the
case and the compiler falls back to the scalar path. This plan describes the
work needed to replace that fallback with a correct vector-mode execution.

## 2. Goal

Allow the vector pipeline to certify and emit a fused serial loop for patterns
of this form:

```text
delayed recursive carrier -> pure/current computation -> recursive projection/output
```

The target generated shape is:

```cpp
copy history into tmp;
for (int i0 = vindex; i0 < vindex + vcount; ++i0) {
    prev = tmp[history + local - delay];
    next = input_and_pure_terms(i0, prev);
    tmp[history + local] = next;
    output_or_safe_transport[i0] = f(next);
}
copy tmp tail back to perm;
```

The essential requirement is that the delayed read and the write of the next
recursive state remain in the same `for i0`, preserving the per-sample temporal
dependency. No value that reads the delayed carrier may be materialized into a
chunk buffer before that serial loop.

## 3. Non-Goals

- Do not change the public semantics of `-ss`.
- Do not remove the current fallback guard before the new certificate is active
  and tested.
- Do not vectorize the recurrence itself. The recurrence remains serial; only
  independent upstream/downstream parts may remain vectorizable.
- Do not fuse all recursive loops indiscriminately. This plan targets only
  subgraphs where a recursive delayed read crosses a dangerous loop boundary.

## 4. Target Model

### 4.1 New Concept: Fused Serial Group

Add an explicit grouping at the vector-plan level, for example:

```rust
FusedSerialGroup {
    group_id,
    owner_loop_id,
    member_loop_ids,
    recursive_carriers,
    delayed_read_signals,
    state_write_signals,
    output_or_transport_roots,
}
```

This grouping does not replace existing `LoopRecord`s. It adds an emission
constraint: selected producers and consumers must be lowered into the same
serial `i0` loop.

### 4.2 Core Invariant

For every recursive carrier `c` and every delayed read `d = delay(c, k)` used
through a cross-loop transport:

```text
read(d, i0) happens before write(c, i0)
write(c, i0) happens before read(d, i0 + k)
```

For short-delay `_tmp/_perm` chunk storage, this means:

```text
DelayCopyIn(c)
for each i0 in chunk:
    read tmp[history + local - k]
    compute next carrier value
    write tmp[history + local]
DelayCopyOut(c)
```

The certificate must reject the shape:

```text
for whole chunk:
    precompute delayed reads into transport
for whole chunk:
    write recursive state
```

## 5. Data Model Changes

### 5.1 Decoration Certificate

The current facts already contain:

- `max_delay`;
- `recursive_projection`;
- `dependencies` with `DepKind::Delayed`;
- `signal_id` identities.

If useful, add an explicit derived fact so the pattern is not reconstructed in
several modules:

```rust
RecursiveDelayedReadFact {
    read_signal_id,
    carrier_signal_id,
    group,
    projection_index,
    delay,
}
```

This fact must be verified from the existing dependencies rather than trusted
as producer output.

### 5.2 VectorPlan

Extend `VectorPlan` with an optional section:

```rust
fused_serial_groups: Vec<FusedSerialGroupRecord>
```

A `FusedSerialGroupRecord` must list:

- the fused loops;
- the recursive carrier;
- delayed-read signals;
- state-writing signals;
- transports removed or rematerialized inline;
- the loop that remains visible in epoch order.

### 5.3 JSON Schema

Extend `vector-verification-certificate-v1.schema.json`, or introduce `v2` if
strict compatibility is preferred. The schema must reject:

- empty groups;
- loops absent from the plan;
- carriers without `max_delay > 0`;
- delayed reads without a `DepKind::Delayed` dependency;
- groups that overlap incompatible clock islands.

## 6. Verification

### 6.1 Rust L2 Checker

Add an independent checker, probably in `vector_verify` or a nearby dedicated
module:

```rust
verify_fused_serial_groups(plan, decorations, groups)
```

The checker must not call the producer. It must reconstruct obligations from
finite facts:

1. each `read_signal_id` depends on a declared carrier through
   `DepKind::Delayed`;
2. the carrier has `max_delay > 0` and `recursive_projection.is_some()`;
3. the carrier and its recursive writer belong to the same recursion group;
4. no remaining transport carries the delayed read out of the group;
5. the epoch topological order treats the fused group as one serial unit;
6. values exported from the group use either post-serial transports produced
   after the carrier write, or direct output stores.

### 6.2 Assembled FIR Verification

Extend `verify_vector_fir_assembly` to recognize this shape:

```text
pre actions
serial fused loop body
post actions
```

The checker must verify that:

- `DelayCopyIn` precedes the fused loop;
- `DelayCopyOut` follows the fused loop;
- the body contains the delayed read and the corresponding `DelayWrite` in the
  same `ForLoop`;
- no transport `StoreTable` materializes the delayed read before `DelayWrite`;
- outputs or downstream transports read the `next` value produced in the same
  iteration, or a buffer filled after the recurrence.

## 7. Lowering and Emission

### 7.1 VectorPlan Builder

Replace the current guard with a two-step path:

1. detect dangerous transports;
2. attempt to build a `FusedSerialGroupRecord`;
3. keep the fallback only when the pattern is still not representable.

The producer should prefer minimal fusion: fuse only the loops required to
preserve the temporal dependency.

### 7.2 VectorRouteSession

Adapt routing:

- a signal inside a fused group must not create an `OuterChunk` transport
  between internal group loops;
- internal loads must become inline values or stack temporaries in the `for i0`;
- outgoing transports to non-fused loops remain allowed, but only after the
  serial loop.

### 7.3 vector_lower

Add a lowering path for fused serial regions:

```text
lower_fused_serial_group(group):
    lower delayed read inline
    lower recurrence computation
    materialize recursion step value
    emit delay write in the same iteration
    emit direct output/store or safe outgoing transport
```

This path should reuse the existing region caches, but must forbid CSE that
hoists a delayed read out of the fused serial body.

### 7.4 vector_assemble

Add an assembled representation:

```rust
AssembledFusedSerialGroup {
    group_id,
    pre,
    serial_body,
    post,
    outgoing_stores,
}
```

Top-level assembly should emit:

```text
pre
sample_loop(serial_body)
post
safe vectorizable tails
```

Pure tails may remain in separate vectorizable loops if their inputs are
produced by a transport filled after the serial loop.

## 8. Implementation Plan

### Phase A - Characterization and Fixtures

- Add a minimal corpus:
  `tests/corpus/vector_recursive_delay_fusion_pulse_countup_loop.dsp`.
- Add tests that document the current status:
  `Fallback(VectorFallbackReason::VectorPlan)` for this pattern.
- Add a snapshot or plan dump showing the dangerous `transport_s23_l2_l1` and
  the delayed recursive carrier.

Exit criteria:

- the current guard remains active;
- the pattern is documented by tests and snapshots.

### Phase B - Facts and Certificate

- Add `RecursiveDelayedReadFact` or an equivalent derived structure.
- Add `FusedSerialGroupRecord` to the Rust DTO.
- Extend the JSON schema or open `v2`.
- Add the independent L2 checker.
- Add rejection tests:
  - empty group;
  - non-recursive carrier;
  - delayed read without delayed dependency;
  - dangerous transport still present;
  - unknown or duplicated loops.

Exit criteria:

- a correct synthetic fused group is accepted;
- all structural mutations are rejected.

### Phase C - Plan Producer

- Detect dangerous transports in `build_vector_plan`.
- Build a minimal fused group.
- Remove or reclassify internal group transports.
- Verify that the epoch schedule treats the group as one serial unit.

Exit criteria:

- the `pulse_countup_loop` plan contains a fused group;
- the dangerous transport no longer appears as an `OuterChunk` transport.

### Phase D - FIR Routing and Lowering

- Adapt `VectorRouteSession` to resolve internal group uses without transport.
- Add fused serial lowering in `vector_lower`.
- Forbid CSE/hoisting of delayed reads out of the serial body.
- Produce outgoing stores after the recursive computation.

Exit criteria:

- assembled FIR contains one serial loop for the pattern;
- `verify_routed_fir` and `verify_vector_fir_assembly` accept the result.

### Phase E - Backend and Bit-Exactness

- Verify generated C++:

```cpp
for i0:
    prev = tmp[history + local - 1];
    next = ...
    tmp[history + local] = next;
    output[i0] = ...
```

- Add interpreter and C/C++ tests:
  - `pulse_countup_loop`;
  - `pulse_countdown_loop`;
  - recurrence with a pure vectorizable tail;
  - `count < vec_size`;
  - `count % vec_size != 0`;
  - `-lv 0` and `-lv 1`;
  - all four `-ss` strategies.

Exit criteria:

- `cargo test -p compiler --test vector_mode` passes without fallback for this
  case;
- `make check-rs-cpp` and `make check-rse-cpp` pass on scalar, `vec0`, `vec1`,
  and `ss0..ss3` variants for the relevant tests;
- no `-inf` or `nan` appears in outputs;
- outputs remain within the current `0.0001` tolerance.

### Phase F - Lift the Fallback

- Replace `reject_cross_loop_delay_read_transports` with:
  - acceptance when a certified fused group covers the transport;
  - fallback otherwise.
- Add a status test:
  `VectorPipelineStatus::Certified` for `pulse_countup_loop`.
- Document in the journal that the fallback was replaced by the certified path.

Exit criteria:

- fallback remains fail-closed for unsupported patterns;
- `pulse_countup_loop` is certified and bit-exact.

## 9. Risks

### CSE Breaking Temporal Semantics

The main risk is that CSE turns a local delayed read into a value materialized
outside the serial loop. The FIR checker must therefore verify the final shape,
not only the plan.

### Over-Fusion

Fusing too many loops can hide vectorization opportunities and make the plan
harder to verify. Fusion should be minimal and structural.

### Clock Islands

`ondemand`, `upsample`, and `downsample` domains add guards and cursors. The
first implementation should reject fusions that cross incompatible clock
islands. Clocked fused groups can be a later phase.

### C++ Parity

C++ builds loops online and can absorb loops during `closeLoop`. The Rust port
does not need to copy that mutability, but it must preserve the observable
invariant: delayed recursive reads stay in the same per-sample order as the
recursive write.

## 10. Done Definition

This work is complete when:

- the conservative guard no longer triggers for `pulse_countup_loop`;
- the vector pipeline returns `VectorPipelineStatus::Certified`;
- generated C++ contains a fused serial loop with read/compute/write in the
  same `for i0`;
- Rust and faustlibraries tests pass on `-lv 0`, `-lv 1`, and `ss0..ss3`;
- L2 checkers reject all structural mutations of the fused group;
- fallback remains active for patterns that do not yet satisfy these
  obligations.
