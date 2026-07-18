//! Type-level operators: union, product, cast helpers, merge, check assertions.
//!
//! # C++ source
//! - `operator|`, `operator*`, `table()` — `sigtype.hh`
//! - `intCast`, `bitCast`, `floatCast`, `sampCast`, `boolCast`, `numCast`,
//!   `castInterval` — inline helpers in `sigtype.hh`
//! - `mergenature`, `mergevariability`, … — `sigtype.cpp`
//! - `checkInt`, `checkKonst`, `checkInit`, `checkIntParam`,
//!   `checkDelayInterval` — `sigtype.cpp`

use std::fmt;

use interval::Interval;

use crate::enums::{Boolean, Computability, Nature, Variability, Vectorability};
use crate::factory::{make_simple, make_table_type, make_tuplet};
use crate::types::SigType;

// ─────────────────────────────────────────────────────────────────────────────
// TypeError
// ─────────────────────────────────────────────────────────────────────────────

/// Error returned by type-level assertion functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError(pub String);

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "type error: {}", self.0)
    }
}

impl std::error::Error for TypeError {}

// ─────────────────────────────────────────────────────────────────────────────
// Merge helpers (aggregate over a slice of types)
// ─────────────────────────────────────────────────────────────────────────────

/// Join all natures — `Int` if all `Int`, `Real` if any `Real`.
///
/// # C++ source
/// `mergenature(ConstTypes v)`
#[must_use]
pub fn merge_nature(types: &[SigType]) -> Nature {
    types
        .iter()
        .fold(Nature::Int, |acc, t| acc.join(t.nature()))
}

/// Join all variabilities.
///
/// # C++ source
/// `mergevariability(ConstTypes v)`
#[must_use]
pub fn merge_variability(types: &[SigType]) -> Variability {
    types
        .iter()
        .fold(Variability::Konst, |acc, t| acc.join(t.variability()))
}

/// Join all computabilities.
#[must_use]
pub fn merge_computability(types: &[SigType]) -> Computability {
    types
        .iter()
        .fold(Computability::Comp, |acc, t| acc.join(t.computability()))
}

/// Join all vectorabilities.
#[must_use]
pub fn merge_vectorability(types: &[SigType]) -> Vectorability {
    types
        .iter()
        .fold(Vectorability::Vect, |acc, t| acc.join(t.vectorability()))
}

/// Join all booleans.
#[must_use]
pub fn merge_boolean(types: &[SigType]) -> Boolean {
    types
        .iter()
        .fold(Boolean::Num, |acc, t| acc.join(t.boolean()))
}

/// Reunion of all intervals.
///
/// # C++ source
/// `mergeinterval(ConstTypes v)`
#[must_use]
pub fn merge_interval(types: &[SigType]) -> Interval {
    types.iter().fold(interval::empty(), |acc, t| {
        interval::reunion(acc, t.interval())
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Union operator  (C++ `operator|`)
// ─────────────────────────────────────────────────────────────────────────────

/// Type union: raises all lattice qualifiers to the join of both arguments.
///
/// For `Tuplet | Tuplet`, joins element-by-element (up to the shorter arity).
/// For mixed variants, falls back to joining the aggregate qualifiers.
///
/// # C++ source
/// `Type operator|(const Type& t1, const Type& t2)`
#[must_use]
pub fn union_types(a: SigType, b: SigType) -> SigType {
    match (a, b) {
        (SigType::Tuplet(ta), SigType::Tuplet(tb)) => {
            let len = ta.components.len().min(tb.components.len());
            let components: Vec<SigType> = ta
                .components
                .into_iter()
                .zip(tb.components)
                .take(len)
                .map(|(ca, cb)| union_types(ca, cb))
                .collect();
            make_tuplet(components)
        }
        (SigType::Table(ta), SigType::Table(tb)) => {
            let content = union_types(*ta.content, *tb.content);
            make_table_type(content)
        }
        (a, b) => {
            let nature = a.nature().join(b.nature());
            let variability = a.variability().join(b.variability());
            let computability = a.computability().join(b.computability());
            let vectorability = a.vectorability().join(b.vectorability());
            let boolean = a.boolean().join(b.boolean());
            let itv = interval::reunion(a.interval(), b.interval());
            make_simple(
                nature,
                variability,
                computability,
                vectorability,
                boolean,
                itv,
            )
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Product operator  (C++ `operator*`)
// ─────────────────────────────────────────────────────────────────────────────

/// Type product: builds a `TupletType` from two types, flattening existing
/// tuplets.
///
/// # C++ source
/// `Type operator*(const Type& t1, const Type& t2)`
#[must_use]
pub fn product_types(a: SigType, b: SigType) -> SigType {
    let mut components = flatten_tuplet(a);
    components.extend(flatten_tuplet(b));
    make_tuplet(components)
}

fn flatten_tuplet(t: SigType) -> Vec<SigType> {
    match t {
        SigType::Tuplet(tt) => tt.components,
        other => vec![other],
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cast helpers  (C++ inline functions in sigtype.hh)
// ─────────────────────────────────────────────────────────────────────────────

/// Force nature to `Int`, converting the interval with `int_cast`.
///
/// # C++ source
/// `Type intCast(Type t)`
#[must_use]
pub fn int_cast(t: SigType) -> SigType {
    let itv = interval::ops::casts::int_cast(t.interval());
    let t = t.with_nature(Nature::Int);
    t.with_interval(itv)
}

/// Force nature to `Int`, keeping the interval unchanged.
///
/// # C++ source
/// `Type bitCast(Type t)`
#[must_use]
pub fn bit_cast(t: SigType) -> SigType {
    t.with_nature(Nature::Int)
}

/// Force nature to `Real`.
///
/// # C++ source
/// `Type floatCast(Type t)`
#[must_use]
pub fn float_cast(t: SigType) -> SigType {
    t.with_nature(Nature::Real)
}

/// Raise variability to `Samp`.
///
/// # C++ source
/// `Type sampCast(Type t)`
#[must_use]
pub fn samp_cast(t: SigType) -> SigType {
    t.promote_variability(Variability::Samp)
}

/// Force nature to `Int` and boolean to `Bool`.
///
/// # C++ source
/// `Type boolCast(Type t)`
#[must_use]
pub fn bool_cast(t: SigType) -> SigType {
    let t = t.with_nature(Nature::Int);
    t.with_boolean(Boolean::Bool)
}

/// Force boolean to `Num`.
///
/// # C++ source
/// `Type numCast(Type t)`
#[must_use]
pub fn num_cast(t: SigType) -> SigType {
    t.with_boolean(Boolean::Num)
}

/// Replace the interval while keeping all other qualifiers.
///
/// # C++ source
/// `Type castInterval(Type t, const interval& i)`
#[must_use]
pub fn cast_interval(t: SigType, i: Interval) -> SigType {
    t.with_interval(i)
}

// ─────────────────────────────────────────────────────────────────────────────
// Check assertions  (C++ `sigtype.cpp`)
// ─────────────────────────────────────────────────────────────────────────────

/// Assert that the signal nature is at most `Int`.
///
/// # C++ source
/// `Type checkInt(Type t)`
pub fn check_int(t: &SigType) -> Result<(), TypeError> {
    if t.nature() == Nature::Real {
        Err(TypeError(format!("expected integer signal, got real: {t}")))
    } else {
        Ok(())
    }
}

/// Assert that the signal variability is at most `Konst`.
///
/// # C++ source
/// `Type checkKonst(Type t)`
pub fn check_konst(t: &SigType) -> Result<(), TypeError> {
    if t.variability() != Variability::Konst {
        Err(TypeError(format!(
            "expected constant signal, got {}: {t}",
            variability_name(t.variability())
        )))
    } else {
        Ok(())
    }
}

/// Assert that the signal computability is at most `Init`.
///
/// # C++ source
/// `Type checkInit(Type t)`
pub fn check_init(t: &SigType) -> Result<(), TypeError> {
    if t.computability() == Computability::Exec {
        Err(TypeError(format!(
            "expected init-time signal, got exec: {t}"
        )))
    } else {
        Ok(())
    }
}

/// Assert Int + Konst + Init (integer compile-time parameter).
///
/// # C++ source
/// `Type checkIntParam(Type t)`
pub fn check_int_param(t: &SigType) -> Result<(), TypeError> {
    check_int(t)?;
    check_konst(t)?;
    check_init(t)
}

/// Assert that the delay amount has a bounded, non-negative upper bound and
/// return its C++-compatible rounded upper bound as `i32`.
///
/// Unlike C++, faust-rs deliberately accepts an interval whose lower bound is
/// negative when its upper bound is non-negative. Existing scalar variable-
/// delay lowering clamps the runtime index and relies on this adapted contract.
///
/// # C++ source
/// `int checkDelayInterval(Type t)`
pub fn check_delay_interval(t: &SigType) -> Result<i32, TypeError> {
    let itv = t.interval();
    if itv.is_empty() {
        return Err(TypeError("delay interval is empty".to_string()));
    }
    if !itv.is_bounded() {
        return Err(TypeError(format!(
            "delay amount must have a bounded interval, got {}",
            itv
        )));
    }
    if itv.hi() < 0.0 {
        return Err(TypeError(format!(
            "delay amount must have a non-negative upper bound, got hi={}",
            itv.hi()
        )));
    }
    Ok(interval::saturated_int_cast(itv.hi() + 0.5))
}

fn variability_name(v: Variability) -> &'static str {
    match v {
        Variability::Konst => "konst",
        Variability::Block => "block",
        Variability::Samp => "samp",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enums::*;
    use crate::factory::make_simple;

    fn simple_int() -> SigType {
        make_simple(
            Nature::Int,
            Variability::Konst,
            Computability::Comp,
            Vectorability::Vect,
            Boolean::Num,
            interval::singleton(42.0),
        )
    }

    fn simple_real() -> SigType {
        make_simple(
            Nature::Real,
            Variability::Samp,
            Computability::Exec,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new_default(),
        )
    }

    #[test]
    fn union_raises_nature() {
        let u = union_types(simple_int(), simple_real());
        assert_eq!(u.nature(), Nature::Real);
        assert_eq!(u.variability(), Variability::Samp);
    }

    #[test]
    fn int_cast_changes_nature_and_interval() {
        let t = simple_real();
        let c = int_cast(t);
        assert_eq!(c.nature(), Nature::Int);
        // int_cast clips the default unbounded interval
        assert!(!c.interval().is_empty());
    }

    #[test]
    fn float_cast_raises_nature() {
        let t = simple_int();
        let c = float_cast(t);
        assert_eq!(c.nature(), Nature::Real);
    }

    #[test]
    fn samp_cast_raises_variability() {
        let t = simple_int();
        let c = samp_cast(t);
        assert_eq!(c.variability(), Variability::Samp);
    }

    #[test]
    fn check_delay_interval_bounded() {
        let t = make_simple(
            Nature::Int,
            Variability::Block,
            Computability::Init,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new(0.0, 1000.0, 0),
        );
        assert_eq!(check_delay_interval(&t), Ok(1000));
    }

    #[test]
    fn check_delay_interval_unbounded_fails() {
        // Use a truly unbounded interval (infinite bound).
        let t = make_simple(
            Nature::Real,
            Variability::Samp,
            Computability::Exec,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new(f64::NEG_INFINITY, f64::INFINITY, -24),
        );
        assert!(check_delay_interval(&t).is_err());
    }

    #[test]
    fn check_delay_interval_accepts_negative_lower_bound_with_nonnegative_hi() {
        let t = make_simple(
            Nature::Int,
            Variability::Block,
            Computability::Init,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new(-1.0, 1000.0, 0),
        );
        assert_eq!(check_delay_interval(&t), Ok(1000));
    }

    #[test]
    fn check_delay_interval_rounds_like_cpp() {
        let t = make_simple(
            Nature::Real,
            Variability::Block,
            Computability::Init,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new(0.0, 3.6, -1),
        );
        assert_eq!(check_delay_interval(&t), Ok(4));
    }

    #[test]
    fn check_delay_interval_saturates_rounded_i32_overflow() {
        let t = make_simple(
            Nature::Real,
            Variability::Block,
            Computability::Init,
            Vectorability::Vect,
            Boolean::Num,
            interval::Interval::new(0.0, f64::from(i32::MAX) - 0.25, -2),
        );
        assert_eq!(check_delay_interval(&t), Ok(i32::MAX));
    }

    #[test]
    fn product_flattens_tuplets() {
        let a = make_tuplet(vec![simple_int(), simple_real()]);
        let b = simple_int();
        let p = product_types(a, b);
        if let SigType::Tuplet(tt) = p {
            assert_eq!(tt.components.len(), 3);
        } else {
            panic!("expected Tuplet");
        }
    }
}
