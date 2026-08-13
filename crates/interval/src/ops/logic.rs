//! Logic, bitwise shift, comparison, and min/max operators.
//!
//! # C++ source
//! `intervalAnd.cpp`, `intervalOr.cpp`, `intervalXor.cpp`, `intervalNot.cpp`,
//! `intervalLsh.cpp`, `intervalRsh.cpp`,
//! `intervalEq.cpp`, `intervalNe.cpp`, `intervalLt.cpp`, `intervalLe.cpp`,
//! `intervalGt.cpp`, `intervalGe.cpp`,
//! `intervalMin.cpp`, `intervalMax.cpp`

use crate::bitwise::{SInterval, bitwise_signed_and, bitwise_signed_or, bitwise_signed_xor};
use crate::ops::arithmetic::mul;
use crate::{
    Interval, empty, saturated_int_cast, saturated_precision_add, saturated_precision_sub,
};

// -------------------------------------------------------------------------
// Bitwise AND
// -------------------------------------------------------------------------

/// Bitwise AND on integer intervals.
///
/// # C++ source
/// `intervalAnd.cpp`
#[must_use]
pub fn and(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    let x0 = saturated_int_cast(x.lo());
    let x1 = saturated_int_cast(x.hi());
    let y0 = saturated_int_cast(y.lo());
    let y1 = saturated_int_cast(y.hi());
    let z = bitwise_signed_and(SInterval { lo: x0, hi: x1 }, SInterval { lo: y0, hi: y1 });
    Interval::new(z.lo as f64, z.hi as f64, x.lsb().min(y.lsb()))
}

// -------------------------------------------------------------------------
// Bitwise OR
// -------------------------------------------------------------------------

/// Bitwise OR on integer intervals.
///
/// # C++ source
/// `intervalOr.cpp`
#[must_use]
pub fn or(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    let x0 = saturated_int_cast(x.lo());
    let x1 = saturated_int_cast(x.hi());
    let y0 = saturated_int_cast(y.lo());
    let y1 = saturated_int_cast(y.hi());
    let z = bitwise_signed_or(SInterval { lo: x0, hi: x1 }, SInterval { lo: y0, hi: y1 });
    Interval::new(z.lo as f64, z.hi as f64, x.lsb().min(y.lsb()))
}

// -------------------------------------------------------------------------
// Bitwise XOR
// -------------------------------------------------------------------------

/// Bitwise XOR on integer intervals.
///
/// Precision logic: singletons inherit their element's trailing-zero count;
/// one singleton transmits the other operand's lsb.
///
/// # C++ source
/// `intervalXor.cpp`
#[must_use]
pub fn xor(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    let x0 = saturated_int_cast(x.lo());
    let x1 = saturated_int_cast(x.hi());
    let y0 = saturated_int_cast(y.lo());
    let y1 = saturated_int_cast(y.hi());
    let z = bitwise_signed_xor(SInterval { lo: x0, hi: x1 }, SInterval { lo: y0, hi: y1 });

    let mut precision = x.lsb().min(y.lsb());

    // Both singletons: count trailing zeros of result.
    if x0 == x1 && y0 == y1 {
        let mut v = x0 ^ y0;
        precision = 0;
        while v != 0 && (v & 1) == 0 {
            v >>= 1;
            precision += 1;
        }
    } else if x0 == x1 {
        precision = y.lsb();
    } else if y0 == y1 {
        precision = x.lsb();
    }

    Interval::new(z.lo as f64, z.hi as f64, precision)
}

// -------------------------------------------------------------------------
// Bitwise NOT
// -------------------------------------------------------------------------

/// Bitwise NOT on integer intervals.
///
/// For `[a, b]`, `~x` is a descending sequence: result is `[~b, ~a]`.
///
/// # C++ source
/// `intervalNot.cpp`
#[must_use]
pub fn not(x: Interval) -> Interval {
    if x.is_empty() {
        return empty();
    }
    let x0 = saturated_int_cast(x.lo());
    let x1 = saturated_int_cast(x.hi());
    let z0 = !x1;
    let z1 = !x0;
    Interval::new(z0 as f64, z1 as f64, 0i32.max(x.lsb()))
}

// -------------------------------------------------------------------------
// Left shift
// -------------------------------------------------------------------------

/// Interval left shift: multiply by `2^k`.
///
/// # C++ source
/// `intervalLsh.cpp`
#[must_use]
pub fn lsh(x: Interval, k: Interval) -> Interval {
    if x.is_empty() || k.is_empty() {
        return empty();
    }
    let j = Interval::new(2.0_f64.powf(k.lo()), 2.0_f64.powf(k.hi()), 0);
    let z = mul(x, j);
    Interval::new(
        z.lo(),
        z.hi(),
        saturated_precision_add(x.lsb(), k.lo() as i32),
    )
}

// -------------------------------------------------------------------------
// Right shift
// -------------------------------------------------------------------------

/// Interval right shift: multiply by `2^(-k)`.
///
/// # C++ source
/// `intervalRsh.cpp`
#[must_use]
pub fn rsh(x: Interval, k: Interval) -> Interval {
    if x.is_empty() || k.is_empty() {
        return empty();
    }
    let j = Interval::new(2.0_f64.powf(-k.hi()), 2.0_f64.powf(-k.lo()), 0);
    let z = mul(x, j);
    Interval::new(
        z.lo(),
        z.hi(),
        saturated_precision_sub(x.lsb(), k.hi() as i32),
    )
}

// -------------------------------------------------------------------------
// Comparison operators — boolean results in {0,1} or singleton
// -------------------------------------------------------------------------

/// Interval equality comparison.
///
/// Returns `[0, 1]` in the general case, `[1]` if the intervals are
/// identical singletons, `[0]` if the intervals are disjoint.
///
/// # C++ source
/// `intervalEq.cpp`
#[must_use]
pub fn eq(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    if x.is_const() && y.is_const() && x == y {
        return Interval::new(1.0, 1.0, 0);
    }
    if x.hi() < y.lo() || y.hi() < x.lo() {
        return Interval::new(0.0, 0.0, 0);
    }
    Interval::new(0.0, 1.0, 0)
}

/// Interval not-equal comparison.
///
/// # C++ source
/// `intervalNe.cpp`
#[must_use]
pub fn ne(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    if x.is_const() && y.is_const() && x != y {
        return Interval::new(1.0, 1.0, 0);
    }
    if x.hi() < y.lo() || y.hi() < x.lo() {
        return Interval::new(1.0, 1.0, 0);
    }
    if x.is_const() && y.is_const() {
        return Interval::new(0.0, 0.0, 0);
    }
    Interval::new(0.0, 1.0, 0)
}

/// `x ≥ y`: `[0,1]` in general, or `[0]` / `[1]` when provable.
///
/// # C++ source
/// `intervalGe.cpp`
#[must_use]
pub fn ge(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    if x.lo() >= y.hi() {
        return Interval::new(1.0, 1.0, 0);
    }
    if x.hi() < y.lo() {
        return Interval::new(0.0, 0.0, 0);
    }
    Interval::new(0.0, 1.0, 0)
}

/// `x > y`: `[0,1]` in general, or `[0]` / `[1]` when provable.
///
/// # C++ source
/// `intervalGt.cpp`
#[must_use]
pub fn gt(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    if x.lo() > y.hi() {
        return Interval::new(1.0, 1.0, 0);
    }
    if x.hi() <= y.lo() {
        return Interval::new(0.0, 0.0, 0);
    }
    Interval::new(0.0, 1.0, 0)
}

/// `x ≤ y`: delegates to `ge(y, x)`.
///
/// # C++ source
/// `intervalLe.cpp`
#[must_use]
pub fn le(x: Interval, y: Interval) -> Interval {
    ge(y, x)
}

/// `x < y`: delegates to `gt(y, x)`.
///
/// # C++ source
/// `intervalLt.cpp`
#[must_use]
pub fn lt(x: Interval, y: Interval) -> Interval {
    gt(y, x)
}

// -------------------------------------------------------------------------
// Min / Max
// -------------------------------------------------------------------------

/// Pointwise minimum. C++ `intervalMin.cpp`.
#[must_use]
pub fn min(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    Interval::new(x.lo().min(y.lo()), x.hi().min(y.hi()), x.lsb().min(y.lsb()))
}

/// Pointwise maximum. C++ `intervalMax.cpp`.
#[must_use]
pub fn max(x: Interval, y: Interval) -> Interval {
    if x.is_empty() || y.is_empty() {
        return empty();
    }
    Interval::new(x.lo().max(y.lo()), x.hi().max(y.hi()), x.lsb().min(y.lsb()))
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::empty;

    #[test]
    fn and_empty() {
        assert!(and(empty(), Interval::new(1.0, 5.0, 0)).is_empty());
    }

    #[test]
    fn not_basic() {
        // For [0, 0]: ~0 = -1.
        let r = not(Interval::new(0.0, 0.0, 0));
        assert_eq!(r.lo(), -1.0);
        assert_eq!(r.hi(), -1.0);
    }

    #[test]
    fn ge_proven_true() {
        let r = ge(Interval::new(5.0, 10.0, 0), Interval::new(1.0, 4.0, 0));
        assert_eq!(r.lo(), 1.0);
        assert_eq!(r.hi(), 1.0);
    }

    #[test]
    fn ge_proven_false() {
        let r = ge(Interval::new(1.0, 3.0, 0), Interval::new(5.0, 10.0, 0));
        assert_eq!(r.lo(), 0.0);
        assert_eq!(r.hi(), 0.0);
    }

    #[test]
    fn min_basic() {
        let r = min(Interval::new(3.0, 8.0, 0), Interval::new(1.0, 6.0, 0));
        assert_eq!(r.lo(), 1.0);
        assert_eq!(r.hi(), 6.0);
    }

    #[test]
    fn max_basic() {
        let r = max(Interval::new(3.0, 8.0, 0), Interval::new(1.0, 6.0, 0));
        assert_eq!(r.lo(), 3.0);
        assert_eq!(r.hi(), 8.0);
    }

    #[test]
    fn lsh_basic() {
        // [0, 1] << [4,4] = [0, 16]
        let r = lsh(Interval::new(0.0, 1.0, 0), Interval::new(4.0, 4.0, 0));
        assert_eq!(r.lo(), 0.0);
        assert_eq!(r.hi(), 16.0);
    }

    #[test]
    fn rsh_uses_upper_shift_bound_for_precision() {
        let r = rsh(Interval::new(8.0, 16.0, 2), Interval::new(-2.0, 4.0, 0));
        assert_eq!(r.lsb(), -2);
    }
}
