# sigtype

Signal type system for the Faust compiler — ported from `compiler/signals/sigtype*`.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/signals/sigtype.hh` / `sigtype.cpp` | Type hierarchy, enums, factories, casts |
| `compiler/signals/sigtyperules.cpp` | Type inference and fixed-point loop |

## What this crate does

Implements the Faust signal type lattice: nature, variability, computability,
vectorability, resolution, and interval bounds. Provides factory functions,
lattice merge/union/product operations, cast helpers, and a bottom-up type
annotator (`TypeAnnotator`) for full signal-graph inference.

The split with the neighbouring crates: the signal IR itself is defined in
`signals`, and the interval arithmetic used for interval bounds lives in
`interval`; this crate owns the lattice and the inference rules, including
interval inference over signals.

## Public API

### Core types

| Item | Description |
|---|---|
| `SigType` | Unified signal type: `Simple`, `Table`, or `Tuplet` |
| `SimpleType` | Scalar type carrying five lattice qualifiers, interval, and fixed-point resolution |
| `TableType` | Indexed table type with content `SigType` |
| `TupletType` | Aggregate of multiple component `SigType`s |
| `Nature` | `Int` / `Real` / `Any` (foreign-function wildcard) |
| `Variability` | `Konst` / `Block` / `Samp` |
| `Computability` | `Comp` / `Init` / `Exec` |
| `Vectorability` | `Vect` / `Scal` / `TrueScal` |
| `Boolean` | `Num` / `Bool` |
| `Res` | Quantization resolution (LSB bit position) |

### Factories

| Function | Description |
|---|---|
| `make_simple(nature, variability, computability, vectorability, boolean, interval)` | Create a `SimpleType` |
| `make_simple_with_res(…, interval, res)` | Create a `SimpleType` with explicit resolution |
| `make_table_type(content)` | Create a `TableType` |
| `make_table_type_with(content, nature, variability, computability, vectorability, boolean, interval)` | Create a `TableType` with explicit aggregate qualifiers |
| `make_tuplet(components)` | Create a `TupletType` |
| `make_maximal()` | Create the lattice top element |

### Lattice operations (`ops`)

| Function | Description |
|---|---|
| `merge_nature` / `merge_variability` / `merge_computability` / `merge_vectorability` / `merge_boolean` / `merge_interval` | Component-wise lattice joins |
| `union_types(a, b)` | Type union (meet on compatible shapes) |
| `product_types(a, b)` | Type product (flattens tuplets) |
| `int_cast` / `float_cast` / `bit_cast` / `samp_cast` / `bool_cast` / `num_cast` / `cast_interval` | Explicit cast coercions |
| `check_int` / `check_konst` / `check_init` / `check_int_param` / `check_delay_interval` | Lattice check helpers |
| `TypeError` | Error type returned by check helpers |

### Type inference

| Item | Description |
|---|---|
| `TypeAnnotator<'a>` | Bottom-up signal type inference with memoization: `TypeAnnotator::new(arena, ui_program)`, then `annotate(outputs)` returns the `SigId → SigType` map |
| `InferenceError` / `InferenceRule` | Typed inference failure and its stable rule identifier |
| `InferenceWarning` | Possible out-of-domain math operation, reported when the caller audits for it |

### Accessors (`api`)

Uniform `SigType` accessors (`nature()`, `variability()`, …) and `promote*`
methods, ported from the `AudioType` virtual methods of `sigtype.hh`.

## Position in the pipeline

Canonical pipeline (AGENTS.md): parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend.

`sigtype` is a library, not a standalone pass: `transform::signal_prepare`
re-types the forest with `TypeAnnotator` between its normalization passes,
and `normalize` itself depends on it.
