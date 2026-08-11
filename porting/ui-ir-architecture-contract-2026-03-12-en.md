# UI IR Architecture Contract (Rust)

**Status**: active design contract for grouped UI parity rewrite.

**Reference C++ baseline**: `master-dev-ocpp-od-fir-2-FIR19` (`8eebea429`)

**Primary C++ anchors**:

- `/Users/letz/Developpements/RUST/faust/compiler/propagate/propagate.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/signals/signals.hh`
- `/Users/letz/Developpements/RUST/faust/compiler/signals/signals.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/transform/signalFIRCompiler.hh`
- `/Users/letz/Developpements/RUST/faust/compiler/generator/instructions_compiler.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/generator/compile.cpp`

## 1. Goal

Define the canonical Rust architecture for grouped UI so that:

- grouped UI becomes a first-class compilation artifact,
- `buildUserInterface` no longer depends on backend-local reconstruction from
  flat widgets,
- the active Rust compile path converges behaviorally toward the C++ model,
- UI ownership and phase boundaries are frozen before implementation work.

This document is the Phase 0 gate for grouped UI parity work.

## 2. Frozen decisions

The following decisions are frozen by this architecture note.

### 2.1 UI is a first-class IR, not a backend heuristic

Rust will introduce a dedicated UI IR as the canonical source of truth for UI
layout and UI declarations.

Backend-local heuristics such as:

- "if widgets exist but no groups exist, synthesize a root group here"
- "recover group structure from widget leaves alone"

are not allowed in the target architecture.

### 2.2 UI IR is introduced at the `propagate` boundary

The new canonical ownership point is:

- `eval` still produces evaluated box syntax with explicit groups/widgets,
- `propagate` consumes that syntax,
- `propagate` produces both:
  - DSP signal outputs,
  - UI IR and control metadata.

This is the closest Rust ownership point to the C++ design, where UI path
context is carried during propagation.

### 2.3 `signals` is not the owner of UI layout

Rust `signals` will no longer be the owner of UI group structure.

Target rule:

- `signals` carries DSP/control semantics only,
- UI layout, grouping, labels, metadata, and ordering belong to UI IR,
- `signals` may reference controls by stable IDs, but it does not own layout.

This is a deliberate architectural adaptation relative to C++ internals, but it
is the canonical Rust target.

### 2.4 Canonical grouped UI root is constructed once, early

Root-group synthesis, when required by Faust semantics, happens exactly once
while constructing canonical UI IR.

It does not happen in FIR lowering or backend code generation.

This means the current fallback in
`crates/transform/src/signal_fir/module.rs` is transitional and must be removed
or reduced to an adapter during migration.

### 2.5 UI labels, pathnames, and metadata are owned by UI IR

By the time UI IR is created:

- labels have already been evaluated/interpolated,
- pathname-bearing labels have already been normalized into structural UI
  placement plus terminal label text,
- metadata embedded in labels has already been extracted from the normalized
  terminal labels,
- UI IR stores:
  - simplified display labels,
  - metadata declarations attached to the owning group/widget node.

No later phase may reparse raw label strings to rediscover path structure or
metadata.

### 2.6 FIR consumes explicit UI IR, not flat widget side-effects

`transform` / signal-to-FIR lowering will consume explicit UI IR and emit FIR UI
instructions from it.

No backend may infer group nesting from DSP/control signal structure.

### 2.7 Deterministic control identity is mandatory

Every UI control must receive a deterministic `ControlId`.

`ControlId` assignment:

- must be stable across deterministic source traversal,
- must not depend on hash map iteration order,
- must not depend on raw `TreeId` values from unrelated arenas,
- must be suitable as the join key between DSP control references and UI IR.

## 3. C++ behavior to preserve

The relevant C++ property is not a specific data layout, but a behavioral
contract:

- UI context survives propagation,
- widgets are associated with a group path,
- `buildUserInterface` is emitted from explicit grouped structure,
- root-group naming follows C++ rules rather than backend-local defaults.

Important C++ observations:

- `signals.hh` and `signals.cpp` document/store UI path in clock environments,
- `propagate.cpp` threads `path` during propagation,
- `signalFIRCompiler.hh` consumes widget signals together with path context,
- `instructions_compiler.cpp` / `compile.cpp` emit explicit open/close box
  instructions,
- `description.cpp` / `compile.cpp` split widget labels into:
  - simplified display label,
  - extracted UI metadata such as `style:knob`,
  - explicit `declare(...)` UI statements attached to the target zone,
- at root level, empty labels are resolved with C++ root naming rules, not by a
  generic module-name fallback.

Rust does not need to mirror the exact C++ internal representation, but it must
mirror these observable semantics.

## 4. Canonical Rust architecture

### 4.1 New dedicated crate ownership

Introduce a dedicated crate:

- `crates/ui`

Canonical API pattern:

- `UiBuilder`
- `UiMatch` + `match_ui`
- `UiId = TreeId`
- `UiProgram`

This follows the same builder/matcher discipline as:

- `boxes`
- `signals`
- `fir`

and keeps the UI layer aligned with the TreeArena-based architecture already
used across the Rust port.

### 4.2 Canonical products of `propagate`

`propagate` no longer conceptually returns "just signals".

Target output shape:

```rust
pub struct PropagateOutput {
    pub signals: Vec<SigId>,
    pub ui: UiProgram,
}
```

Equivalent naming is acceptable, but the ownership split is frozen:

- DSP signals
- UI IR

must both be explicit products of the propagation boundary.

### 4.3 `UiProgram` source-of-truth model

`UiProgram` is the canonical grouped UI artifact.

It contains:

- one canonical root UI group,
- one arena-backed tree of groups/widgets,
- one stable control registry indexed by `ControlId`,
- optional program-level metadata needed for root naming and validation.

Target top-level shape:

```rust
pub struct UiProgram {
    pub arena: TreeArena,
    pub root: UiId,
    pub controls: Vec<ControlSpec>,
}
```

Exact field names may vary, but the semantics are fixed.

### 4.4 UI node families

The canonical UI IR must represent at least:

- `Group`
  - kind: `Vertical | Horizontal | Tab`
  - simplified label
  - metadata declarations
  - ordered children
- `InputControl`
  - `Button`
  - `Checkbox`
  - `HSlider`
  - `VSlider`
  - `NumEntry`
- `OutputControl`
  - `HBargraph`
  - `VBargraph`
- `Soundfile`

Recommended node shape:

```rust
enum UiNodeKind {
    Group {
        kind: UiGroupKind,
        label: String,
        metadata: Vec<(String, String)>,
        children: Vec<UiId>,
    },
    InputControl {
        control: ControlId,
    },
    OutputControl {
        control: ControlId,
    },
    Soundfile {
        control: ControlId,
    },
}
```

This shape keeps layout in the tree and control parameters in the registry.

### 4.5 `ControlSpec` ownership

`ControlSpec` is the source of truth for one control's semantics.

It owns:

- stable `ControlId`,
- control kind,
- simplified label,
- control metadata,
- numeric ranges / init / step where applicable,
- declared UI direction (`input`, `output`, `soundfile`),
- any FIR/backend-relevant control configuration.

Recommended shape:

```rust
pub struct ControlSpec {
    pub id: ControlId,
    pub kind: ControlKind,
    pub label: String,
    pub metadata: Vec<(String, String)>,
    pub range: Option<ControlRange>,
}
```

The exact split between `UiNode` and `ControlSpec` may vary, but:

- layout belongs to the UI tree,
- control semantics belong to the control registry.

## 5. Phase boundary contract

Pipeline contract becomes:

`parse -> boxes -> eval -> propagate -> normalize -> type/interval -> transform -> fir -> backend`

with grouped UI carried explicitly in parallel after `propagate`.

### 5.1 Phase-by-phase ownership table

| Phase | Consumes | Produces | UI ownership status |
|---|---|---|---|
| `parser` | source | box AST | no canonical UI ownership yet |
| `eval` | box AST | evaluated boxes | explicit groups/widgets still in box syntax |
| `propagate` | evaluated boxes | `PropagateOutput { signals, ui }` | **canonical UI owner begins here** |
| `normalize` | `PropagateOutput` | normalized signals + unchanged `ui` | UI passthrough |
| `interval` / signal typing | normalized signals + unchanged `ui` | typed signals + unchanged `ui` | UI passthrough |
| `transform` | typed/prepared signals + `ui` | FIR module | lowers DSP and UI from parallel artifacts |
| `fir` | FIR module | FIR UI instructions already explicit | no UI reconstruction |
| `backend` | FIR module | text/binary output | emits UI only from FIR |

### 5.2 Hard boundary rule

After `propagate`:

- no phase may need to inspect group/widget box syntax to understand UI,
- no phase may need to recover UI grouping from raw labels or widget leaves.

`UiProgram` is the source of truth from that point onward.

## 6. Signals contract after the rewrite

### 6.1 Signals becomes DSP/control-semantic only

The current signal widget nodes carrying labels/ranges are transitional.

Target signal contract:

- no signal node owns group hierarchy,
- no signal node owns widget display labels,
- signal nodes reference controls by `ControlId`.

Recommended target node families:

- `SigMatch::ControlRef(ControlId)` for input controls,
- `SigMatch::ControlSink(ControlId, value)` for output controls/bargraphs,
- optional dedicated soundfile reference node if needed for DSP semantics.

Equivalent naming is acceptable, but the ownership split is frozen.

### 6.2 Migration policy for existing widget signal nodes

Current widget-like signal variants such as:

- `Checkbox`
- `Button`
- `HSlider`
- `VSlider`
- `NumEntry`
- `HBargraph`
- `VBargraph`

must be treated as transitional compatibility surface.

Target policy:

- they may coexist during migration,
- but backends must not depend on them for grouped UI structure,
- long-term target is `ControlId`-based signal references.

## 7. Root-group policy

### 7.1 Canonical rule

`UiProgram` always has exactly one canonical root group.

This root is:

- the explicit top-level group when the Faust program already provides one,
- otherwise a synthesized root group created during UI IR construction using
  C++ root naming rules.

### 7.2 Root synthesis rules

When synthesis is required:

- orientation follows the C++ default root-group rule,
- the root label follows C++ behavior:
  - use declared `name` metadata when appropriate,
  - otherwise fall back to filename-derived module name,
  - never use a backend-local arbitrary default.

### 7.3 Consequence for Rust implementation

The current helper:

- `maybe_wrap_ui_in_root_group(...)`

is not part of the target architecture.

It is allowed only as a migration adapter until `UiProgram` root synthesis is
implemented. The end state is:

- root-group synthesis belongs to UI IR construction,
- FIR lowering and backends do not synthesize root groups.

## 8. Label, metadata, and pathname policy

### 8.1 Ownership point

Label interpolation stays in `eval`.

Pathname normalization and metadata extraction for UI labels are completed
before or during UI IR construction in `propagate`.

### 8.2 Canonical storage rule

UI IR stores:

- canonical structural placement for pathname-bearing labels,
- simplified display label,
- extracted metadata declarations,
- no raw unevaluated label template.

Concrete parity rule for labels carrying UI metadata:

- source label: `gain [style:knob]`
- canonical UI IR label: `gain`
- canonical UI IR metadata: `("style", "knob")`

The full raw label string is not the canonical display label after UI IR
construction.

### 8.3 Forbidden downstream behavior

Forbidden after UI IR construction:

- reparsing `%...` substitutions,
- reparsing `[key:value]`-style metadata from labels,
- inferring group path from slash-split labels alone,
- late pathname normalization in FIR or backend code.

### 8.4 Canonical rule for metadata-bearing UI labels

The grouped-UI rewrite already closed the previous Rust metadata gap.

The frozen canonical rule is now:

1. Rust applies a C++-equivalent of `extractMetadata(...)` during UI IR
   construction in `propagate`
2. `ControlSpec.label` stores the simplified display label
3. `ControlSpec.metadata` stores the extracted UI metadata key/value pairs
4. `UiProgram` keeps that information canonical rather than reparsing labels
   later
5. FIR/UI lowering emits explicit `AddMetaDeclare` instructions attached to the
   same control zone/group
6. backends emit `declare(...)` calls from FIR before or alongside the
   corresponding `addButton` / `addSlider` / `addBargraph` / `addSoundfile`
   instruction sequence

End-state parity requirement for the example above:

- C++/C backends emit `declare(..., "style", "knob")`
- C++/C backends emit `addHorizontalSlider("gain", ...)`
- they do not expose `gain [style:knob]` as the final display label

### 8.5 C++ pathname baseline to preserve

The Faust language also allows UI labels to behave as pathnames rather than
pure display strings. The specification is documented in:

- <https://faustdoc.grame.fr/manual/syntax/#labels-as-pathnames>

Relevant C++ provenance:

- `compiler/propagate/labels.cpp`
- `compiler/propagate/propagate.cpp`
- `compiler/generator/uitree.cpp`
- `compiler/generator/compile.cpp`

Observed C++ behavior to preserve:

- widget labels are normalized during `propagate` with
  `normalizePath(cons(label, path))`
- the pathname grammar includes:
  - absolute reset with `/`
  - current-directory prefix `./`
  - parent-directory prefix `../`
  - typed path segments such as `h:`, `v:`, and `t:`
- metadata is not stripped during pathname normalization; it remains attached
  to the terminal segment and is extracted later during UI tree/code
  generation
- the grouped UI tree is then built from the normalized widget path, so a
  label such as `../volume` can reparent a widget out of the immediately
  enclosing source group

Concrete C++ example:

```faust
process = hgroup("Foo",
    vgroup("Faa",
        hslider("../volume", 0.5, 0, 1, 0.01)
    )
);
```

Faust C++ emits:

- `openHorizontalBox("Foo")`
- `addHorizontalSlider("volume", ...)`

and does not keep the widget inside `Faa`.

Important current C++ limitation:

- explicit group labels themselves are still treated as direct folder labels in
  grouped UI construction and UI code generation
- C++ does not currently interpret group labels such as `../Foo` as relative
  pathname navigation for the group node itself

This boundary matches the Faust manual's wording: the
[`Labels as Pathnames`](https://faustdoc.grame.fr/manual/syntax/#labels-as-pathnames)
section permits an absolute or relative pathname directly in the label of a
*widget* and illustrates it with `hslider("../volume", ...)`; it does not grant
the same behavior to group labels. It was re-verified on 2026-08-11 against the
pinned C++ tree (`master-dev-ocpp-od-fir-2-FIR19`, `8eebea429`):

- `compiler/propagate/propagate.cpp` calls `normalizePath(cons(label, path))`
  for terminal UI elements;
- its `isBoxVGroup`, `isBoxHGroup`, and `isBoxTGroup` branches only prepend the
  typed group label to `path`;
- compiling `rep_63_ui_relative_group_rebase.dsp` therefore emits
  `openVerticalBox("../Bar")`, whereas Rust emits a root-level `Bar` sibling of
  `Foo`.

This limitation is part of the current C++ baseline and must be documented
explicitly before any Rust extension is added on top of it.

### 8.6 Current Rust pathname parity gap

Current Rust grouped UI construction in `crates/propagate/src/lib.rs` does:

- `decode_box_label(...)`
- immediately `split_label_metadata(...)`
- direct recursive group/widget construction in source-tree order

It does not yet:

- parse pathname grammar from widget labels,
- normalize `./`, `../`, `/`, or typed path segments against the current group
  stack,
- reparent widgets or soundfiles according to the normalized path.

Therefore Rust currently emits the previous example as:

- `openHorizontalBox("Foo")`
- `openVerticalBox("Faa")`
- `addHorizontalSlider("../volume", ...)`

This is a true parity bug relative to Faust C++.

There is also an architectural consequence:

- the current `collect_ui_nodes(...) -> Vec<UiId>` model is sufficient for
  direct recursive nesting,
- but it is not sufficient for pathname-aware insertion because widgets may
  need to be attached under a different ancestor than their immediate source
  parent.

### 8.7 Frozen correction path for C++ widget pathname parity

Widget pathname parity must be implemented before any Rust-only extension for
group labels.

The canonical Rust correction path is frozen as follows:

1. treat one raw evaluated UI label as going through three logical stages:
   - raw evaluated label string,
   - pathname-normalized structural placement,
   - terminal display label plus extracted metadata
2. add a dedicated pathname parser/normalizer in `ui` or `propagate`
3. keep pathname normalization separate from `split_label_metadata(...)`
4. preserve raw segment text during pathname parsing so metadata extraction can
   still run on the normalized terminal segment afterward
5. make grouped UI construction path-aware:
   - widgets, bargraphs, and soundfiles are inserted at a normalized target
     path
   - direct recursive source nesting is no longer the sole placement rule
6. keep FIR and backend phases unchanged:
   - they consume only canonical `UiProgram`
   - they never normalize pathname labels themselves

The canonical ordering is therefore:

`eval label string -> pathname normalization -> metadata extraction -> UiProgram`

and not:

`eval label string -> metadata extraction -> late slash splitting`

### 8.8 Rust extension: relative pathname navigation in explicit group labels

Rust may deliberately extend the C++ baseline by allowing relative pathname
navigation in explicit group labels themselves.

This is an `adapted` behavior, not a `1:1` C++ parity rule.

Motivation:

- widget labels already admit relative pathname navigation in Faust
- extending that navigation model to explicit group labels makes grouped UI
  placement more coherent and easier to reason about in the Rust architecture

This extension is frozen with the following scope for the first iteration:

- supported navigation prefixes on group labels:
  - `./`
  - one or more `../`
  - `/` to reset to the canonical UI root
- the explicit group constructor (`vgroup`, `hgroup`, `tgroup`) remains the
  owner of the emitted group's orientation
- the initial extension does **not** infer orientation from typed pathname
  segments embedded inside a group label
- the initial extension does **not** synthesize arbitrary intermediate groups
  from a multi-segment group pathname such as `foo/bar`

In other words, the initial Rust extension is navigation-only for group
labels, not full arbitrary path construction.

Canonical semantics for the extension:

- relative navigation changes the parent insertion site of the explicit group
  node before its children are attached
- the terminal group label still goes through metadata extraction and empty
  label handling like any other group label
- navigation that climbs above the canonical root clamps at the canonical root
- the source `vgroup` / `hgroup` / `tgroup` node still determines the
  orientation of the inserted group

Example desired Rust-only behavior:

```faust
process = hgroup("Foo",
    vgroup("../Bar",
        hslider("gain", 0.5, 0, 1, 0.01)
    )
);
```

Target Rust UI structure:

- `Foo` remains an explicit group under the root
- `Bar` becomes a sibling group rebased to the parent of `Foo`
- `gain` is emitted inside `Bar`

This behavior is intentionally beyond current C++ semantics and must be tested
as a Rust-specific extension, not as a differential C++ parity case.

### 8.9 Consequences for internal APIs and invariants

Pathname support changes the grouped UI construction contract in three ways:

1. `split_label_metadata(...)` cannot be the first and only normalization step
   for pathname-bearing UI labels
2. `collect_ui_nodes(...)` needs explicit current-path context plus a path-aware
   insertion API, rather than only returning immediate child `UiId` lists
3. `UiProgram` construction must support deterministic insertion/merging at a
   target group path while preserving stable source order for siblings emitted
   at the same structural location

Recommended internal direction:

- keep pathname parsing/normalization near `ui` / `propagate`
- add an insertion-oriented grouped UI builder rather than pushing pathname
  handling into FIR
- keep canonical root synthesis and label/metadata extraction in the same UI IR
  construction phase so all layout decisions remain frozen before `transform`

### 8.10 Known root-sibling ordering parity gap

A corpus-wide generated-UI comparison on 2026-08-11 exposed one ordering gap
outside the pathname extension itself.

The C++ UI tree orders every group's children, including controls placed under
the implicit top-level group, by the raw label/order key. Rust's
`UiProgramBuilder` applies that ordering to children of an explicit draft group,
but its root forest is currently a plain insertion-ordered list. When
`UiProgram::finish` synthesizes the top-level group around otherwise ungrouped
controls, their dataflow-discovery order is retained instead of being sorted.

Confirmed corpus cases:

- `rep_38_sine_phasor.dsp`: Rust starts with `gain`; C++ starts with `freq1`;
- `rep_55_sine_phasor_echo_feedback.dsp`: Rust starts with `gain`; C++ starts
  with `feedback`;
- `rep_57_additive_synth.dsp`: Rust starts with `gain`; C++ starts with `freq`;
- `rep_66_variable_delay_feedback.dsp`: Rust emits `mod_delay` before
  `feedback`; C++ emits `feedback` first;
- `rep_75_ui_widget_family_breadth.dsp`: Rust retains the source/dataflow order
  `gate`, `pitch`, `delay`, `meter`; C++ emits `delay`, `gate`, `meter`, `pitch`.

This is `DIFF-GAP-011`, a parity defect rather than a deliberate Rust behavior.
It changes UI/JSON presentation order but not control labels, addresses, or DSP
semantics. A correction must sort the synthesized root's children with the same
raw-label key used for explicit groups while retaining stable insertion order
for equal keys.

## 9. Transform and FIR contract

### 9.1 Transform input

`transform` consumes:

- prepared/typed DSP signals,
- `UiProgram`.

It does not synthesize grouped UI from DSP signals.

### 9.2 FIR output

FIR remains the only backend-facing IR.

UI in FIR is emitted from `UiProgram` into explicit FIR nodes:

- `OpenBox`
- `CloseBox`
- `AddButton`
- `AddSlider`
- `AddBargraph`
- `AddSoundfile`
- `AddMetaDeclare`

### 9.3 Backend rule

Backends consume FIR UI instructions only.

No backend may inspect `signals` or `UiProgram` directly to recover UI
structure in production code paths.

## 10. Public/internal API classification

The following mapping statuses are frozen.

| Surface | Status | Rationale |
|---|---|---|
| `eval` explicit group/widget preservation | `1:1` behaviorally | already matches C++ intent |
| `propagate` returning UI as explicit parallel artifact | `adapted` | Rust uses explicit IR rather than C++ clockenv representation |
| `signals` owning only control references | `adapted` | cleaner Rust ownership split, same external behavior target |
| widget label pathname normalization | `1:1` behaviorally | must match C++ `normalizePath(...)` semantics for widgets/soundfiles/bargraphs |
| group-label relative pathname navigation | `adapted` | deliberate Rust extension for coherent relative rebasing of explicit groups |
| FIR explicit UI instructions | `1:1` behaviorally | same backend-visible contract |
| backend UI emission from FIR only | `1:1` behaviorally | same output contract as C++ |

External compatibility note:

- `UiProgram` is initially a workspace-internal architectural API,
- not yet a stable user-facing external API.

## 11. Validation contract

Required validation for the rewrite:

- differential C++ comparison on grouped UI corpus cases,
- explicit event-order checks for `buildUserInterface`,
- nested-group parity checks,
- empty-label root-group parity checks,
- metadata-on-group/widget parity checks,
- widget-pathname parity checks against C++,
- Rust-only group-path rebasing checks for the explicit extension,
- deterministic `ControlId` and emission-order tests.

Minimum corpus cases:

- `tests/corpus/rep_58_higher_order_named_direct_apply.dsp`
- `tests/corpus/rep_59_higher_order_named_argument_apply.dsp`
- `tests/corpus/rep_17_ui_groups.dsp`
- `tests/corpus/rep_28_nested_ui_groups.dsp`
- `tests/corpus/rep_56_noise_smoo_slider.dsp`

Additional targeted cases required for pathname work:

- one nested-group widget case using `../volume`
- one absolute pathname widget case using a typed segment such as
  `h:Oscillator/freq`
- one widget case combining relative pathname navigation with inline metadata,
  for example `../gain [style:knob]`
- one Rust-only extension case rebasing an explicit group with `../Bar`
- one Rust-only extension case proving root-clamp behavior for repeated
  `../../..` group navigation

These pathname cases should be added as dedicated DSP fixtures under
`tests/corpus/` rather than staying inline-only in test sources, so they can
participate in:

- Rust golden coverage,
- future C++ golden refreshes,
- direct CLI/manual inspection during debugging,
- reuse across `signal_pipeline`, FIR-lane, and differential test suites.

## 12. Recommended implementation slices

Implementation should follow this order:

1. add failing grouped-UI parity tests
2. add `crates/ui` with canonical builder/matcher API
3. add `PropagateOutput { signals, ui }`
4. add canonical root-group synthesis in UI IR construction
5. add `ControlId`-based signal references and migration adapters
6. lower `UiProgram` to FIR explicit UI statements
7. remove backend/root-group fallback heuristics
8. widen C++ differential coverage

### 12.1 Additional pathname follow-up slices

The baseline grouped-UI rewrite above is already complete. Pathname support is
the next follow-up slice on top of that baseline.

Pathname work should proceed in this order:

1. add failing C++ differential tests for widget pathname semantics
2. add failing Rust-only tests for relative group-label rebasing
3. introduce a dedicated pathname parser/normalizer with Rustdoc provenance to
   `labels.cpp`
4. refactor grouped UI construction from direct recursive assembly to
   path-aware insertion into canonical `UiProgram`
5. close widget-path parity first and keep it green against C++
6. then add the Rust-only group-label navigation extension
7. widen coverage for metadata-bearing pathname labels and mixed nested groups

### 12.2 Concrete implementation plan by file

The following concrete plan is the recommended execution order for the Rust
implementation.

#### Step A. Freeze failing tests before refactoring

Files to touch:

- `crates/compiler/tests/cpp_signal_differential.rs`
- `crates/compiler/tests/signal_pipeline.rs`
- `tests/corpus/`

Work:

- add dedicated DSP corpus fixtures under `tests/corpus/` for:
  - relative widget rebasing with `../volume`
  - absolute/typed widget pathname such as `h:Oscillator/freq`
  - relative widget pathname plus inline metadata
  - explicit group-label rebasing with `../Bar`
  - explicit group-label root clamp with repeated `../../..`
- add one C++ differential asserting widget pathname parity for each C++-owned
  corpus fixture
- add Rust-only regressions in `signal_pipeline.rs` for the group-rebasing
  corpus fixtures
- assign stable `rep_*` names when adding these corpus files so they can join
  the existing golden and differential workflow cleanly

Exit criteria:

- the new tests fail on current Rust for pathname behavior
- existing grouped-UI tests stay green

#### Step B. Introduce canonical pathname parsing and normalization helpers

Files to touch:

- `crates/ui/src/lib.rs`
- optionally `crates/ui/tests/core_api.rs`

Work:

- add a dedicated pathname model, for example:
  - `UiPathStep`
  - `UiPathAnchor`
  - `UiLabelPath`
- add a parser for Faust pathname grammar modeled after `labels.cpp`
- keep this helper separate from `split_label_metadata(...)`
- document the exact ordering and provenance with Rustdoc
- add structural tests covering:
  - `/`
  - `./`
  - `../`
  - typed segments for widgets
  - metadata preserved on the terminal segment

Recommended rule split:

- `split_label_metadata(...)` remains responsible only for
  label/metadata extraction
- new pathname helpers become responsible only for path parsing and
  normalization

Exit criteria:

- pathname parsing/normalization is testable in isolation from `propagate`
- no FIR/backend code changes are needed yet

#### Step C. Add path-aware grouped UI insertion primitives

Files to touch:

- `crates/ui/src/lib.rs`
- `crates/ui/tests/core_api.rs`

Work:

- add an insertion-oriented builder layer on top of the current tree matcher,
  for example:
  - `UiProgramBuilder`
  - `insert_group_at_path(...)`
  - `insert_control_at_path(...)`
- preserve deterministic sibling ordering for multiple insertions into the same
  target group
- preserve existing root synthesis and metadata invariants
- do not leak raw pathname strings into stored `UiProgram`

Important design constraint:

- this builder must support inserting a node under an ancestor other than the
  immediate syntactic parent
- that is the core requirement the current `Vec<UiId>` recursion model does not
  satisfy

Exit criteria:

- `UiProgram` can be assembled by normalized target path, not only by direct
  recursive nesting
- builder-level tests prove deterministic order and root-clamp behavior

#### Step D. Refactor `propagate` grouped UI construction onto the path-aware builder

Files to touch:

- `crates/propagate/src/lib.rs`
- `crates/propagate/tests/core_api.rs`

Work:

- split the current grouped UI collection flow into explicit stages:
  1. decode evaluated label string
  2. normalize pathname relative to current UI group path
  3. extract metadata from the terminal segment
  4. insert canonical group/control into the UI builder
- replace the current `collect_ui_nodes(...) -> Vec<UiId>` direct assembly for
  UI-bearing families with insertion through the path-aware builder
- keep `ControlId` allocation deterministic and unchanged
- keep DSP signal propagation behavior unchanged

Recommended helper split inside `propagate`:

- one helper to track the current explicit source group stack
- one helper to normalize a widget pathname against that stack
- one helper to normalize a group label for the Rust extension path
- one helper to insert the resulting canonical node into the UI builder

Exit criteria:

- widget pathname tests pass at the `propagate` / `signal_pipeline` level
- no grouped UI reconstruction is reintroduced in later phases

#### Step E. Close C++ widget pathname parity first

Files to touch:

- `crates/compiler/tests/cpp_signal_differential.rs`
- `crates/compiler/tests/signal_pipeline.rs`
- possibly `crates/transform/src/signal_fir/module.rs` only if test fixtures
  need additional inspection helpers

Work:

- keep the Rust-only group-label extension disabled while widget-path parity is
  being closed
- make all C++ differentials pass for:
  - relative widget rebasing with `../`
  - absolute reset with `/`
  - typed path segments on widgets
  - pathname + metadata interaction

Exit criteria:

- widget pathname semantics match C++ in ordered UI event output
- no Rust-only behavior is yet visible in the C++ differential suite

#### Step F. Enable the Rust-only extension for explicit group labels

Files to touch:

- `crates/ui/src/lib.rs`
- `crates/propagate/src/lib.rs`
- `crates/propagate/tests/core_api.rs`
- `crates/compiler/tests/signal_pipeline.rs`

Work:

- add navigation-only handling for group labels:
  - `./`
  - `../`
  - `/`
- keep orientation owned by the source `vgroup` / `hgroup` / `tgroup`
- clamp above-root group navigation at the canonical root
- keep typed segments and arbitrary `foo/bar` group paths out of scope for the
  first iteration

Exit criteria:

- Rust-only rebasing tests for explicit group labels pass
- existing C++ differential tests remain unchanged because this extension is
  tested only on Rust-owned cases

#### Step G. Harden and document the final behavior

Files to touch:

- `crates/ui/src/lib.rs`
- `crates/propagate/src/lib.rs`
- `crates/compiler/tests/cpp_signal_differential.rs`
- `crates/compiler/tests/signal_pipeline.rs`
- `porting/journal/YYYY-MM-DD.md`

Work:

- add Rustdoc on the final pathname helper types and insertion invariants
- add mixed nested-group cases combining:
  - pathname rebasing
  - inline metadata
  - explicit root synthesis
- add explicit regression coverage for soundfile pathnames
- journal the implementation slices as they land

Recommended commit slicing:

1. failing tests plus new corpus DSP fixtures
2. pathname parser/normalizer in `ui`
3. path-aware `UiProgram` builder
4. `propagate` refactor for widget parity
5. Rust-only group-label extension
6. hardening/doc pass

## 13. Non-goals

This architecture change does not aim to:

- expose a stable end-user UI API immediately,
- redesign audio-rate DSP semantics,
- change backend-visible FIR UI node families,
- copy the exact C++ internal storage layout for clock environments.

The target is behavioral parity with clearer Rust ownership.
