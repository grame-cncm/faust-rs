# interval

Interval analysis library — ported from `compiler/interval/` in the Faust C++ compiler.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/interval/interval_def.hh` | Core `Interval` type and set operations |
| `compiler/interval/interval_algebra.hh` | Algebra surface (operator dispatch) |
| `compiler/interval/interval*.cpp` | Concrete operator implementations |

## What this crate does

Provides a closed-interval type `Interval` carrying `[lo, hi]` bounds and a
least-significant-bit precision (`lsb`). Implements the Faust interval-algebra
surface: arithmetic, bitwise, logic, trigonometric, math, delay/memory, and
UI-relevant operators. Operator families that are placeholders in the C++
implementation remain explicit zero-interval placeholders in `ops::missing`.

## Public API

### Core type

| Item | Description |
|---|---|
| `Interval` | Closed interval `[lo, hi]` with `lsb` precision |
| `Interval::new(lo, hi, lsb)` | Construct an interval |
| `Interval::from_scalar(v)` | Construct a degenerate point interval |
| `Interval::lo()` / `hi()` / `lsb()` | Bound and precision accessors |
| `Interval::is_valid()` / `is_empty()` / `is_unbounded()` / `is_bounded()` | State predicates |
| `Interval::has(v)` / `is(v)` / `has_zero()` / `is_zero()` / `is_const()` | Value predicates |
| `Interval::is_power_of2()` / `is_bitmask()` | Structural predicates |

### Operator modules

| Item | Description |
|---|---|
| `ops::arithmetic` | `+`, `-`, `*`, `/`, `%`, `abs`, `neg`, `inv` |
| `bitwise` | Signed/unsigned `&`, `\|`, `^`, `~`, and shift interval operations |
| `ops::logic` | Boolean algebra on intervals |
| `ops::math` | `floor`, `ceil`, `round`, `rint`, `pow`, `sqrt`, `exp`, `log`, … |
| `ops::trig` | `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2` |
| `ops::delay_table` | Delay and memory interval propagation |
| `ops::missing` | C++-compatible zero placeholders for unimplemented operator families |
| `ops::ui` | UI-widget interval constraints |
| `ops::casts` | Type-cast interval operations |

### Utilities

| Item | Description |
|---|---|
| `saturated_int_cast(value)` | Saturating `f64` to `i32` conversion |
| `saturated_precision_add(a, b)` | Saturating precision addition |
| `saturated_precision_sub(a, b)` | Saturating precision subtraction |

## Position in the pipeline

Canonical pipeline (AGENTS.md): parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend.

`interval` is pure arithmetic with no workspace dependencies. Interval
inference over signals lives in `sigtype::rules` (`TypeAnnotator`), which
calls these operators and stores the result in each `SimpleType`.
