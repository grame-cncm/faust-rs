# ui

UI IR construction and matching helpers backed by `tlib::TreeArena`.

Canonical grouped-UI artifact owned after the `propagate` boundary.
Controls are referenced by deterministic `ControlId` values instead of
duplicating labels and ranges in DSP signal nodes.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/propagate/propagate.cpp` | UI extraction during propagation |
| `compiler/generator/instructions_compiler.cpp` | Grouped-UI construction policy |
| `compiler/generator/compile.cpp` | Root label and compilation name policy |
| `compiler/transform/signalFIRCompiler.hh` | FIR-side UI zone references |

## What this crate does

- Builds a hierarchical `UiProgram` (groups → controls) from box-tree traversal.
- Provides a stable `ControlId` namespace joining DSP widgets with grouped layout.
- Normalizes widget label paths and group label navigation.
- Exposes `match_ui` for structural decomposition of UI trees.

## Public API

### Core types

| Item | Description |
|---|---|
| `UiProgram` | Canonical grouped-UI artifact produced by `propagate` |
| `UiProgramBuilder` | Accumulates groups and controls into a `UiProgram` |
| `UiBuilder<'a>` | Low-level builder for raw `UiId` nodes in `TreeArena` |
| `UiId` | UI node identifier (`TreeId` alias) |
| `ControlId` | Stable control identifier (dense index, `0..controls.len()`) |

### Groups and controls

| Item | Description |
|---|---|
| `UiGroupKind` | `Vertical` / `Horizontal` / `Tab` |
| `UiGroupPathSegment` | One segment of a hierarchical group path |
| `UiGroupSpec` | Full group specification (kind + path) |
| `UiRootOrigin` | Origin tag for the synthesized root group |
| `ControlKind` | Widget family: `Button`, `Checkbox`, `VSlider`, `HSlider`, `NumEntry`, `VBargraph`, `HBargraph`, `Soundfile` |
| `ControlRange` | Min/max/step/init numeric range for sliders and bargraphs |
| `ControlSpec` | Full control specification (kind, label, range) |
| `UiMetadata` | Key→value metadata map for a control (`declare` statements) |

### Matching and path utilities

| Item | Description |
|---|---|
| `match_ui(arena, id)` | Structural decode of one UI tree node into `UiMatch` |
| `UiMatch<'a>` | Decoded UI node view returned by `match_ui` |
| `normalize_widget_label_path(label, current_groups)` | Normalize a widget path relative to the current group stack |
| `normalize_group_label_navigation(label, current_groups, kind)` | Normalize group navigation relative to the current group stack |
| `split_label_metadata(label)` | Split a `"label [key:val]"` string into label + metadata |
| `canonicalize_group_spec(segment)` | Convert one normalized path segment into a `UiGroupSpec` |

### Utilities

| Item | Description |
|---|---|
| `CRATE_NAME` | Crate identity string constant |
| `crate_id()` | Returns `CRATE_NAME` (used in diagnostics) |

## Position in the pipeline

```
propagate  →  [ui]  →  transform::signal_fir  →  codegen
```
