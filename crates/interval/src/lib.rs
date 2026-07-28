//! Interval analysis library.
//!
//! Ports the C++ interval subsystem from `compiler/interval/` in the Faust
//! reference implementation.
//!
//! # Source provenance (C++)
//! - Core type: `compiler/interval/interval_def.hh`
//! - Algebra surface: `compiler/interval/interval_algebra.hh`
//! - Operator implementations: `compiler/interval/interval*.cpp`
//!
//! # API mapping status
//! - `Interval` type: ported from `itv::interval` (C++)
//! - All set operations: ported from `interval_def.hh`
//! - All algebra operators: ported from concrete `interval*.cpp` files

pub mod bitwise;
pub mod ops;
pub mod utils;

use std::fmt;

/// Saturated cast of a `f64` to `i32`. Matches C++ `itv::saturatedIntCast`.
#[inline]
#[must_use]
pub fn saturated_int_cast(d: f64) -> i32 {
    d.clamp(-2_147_483_648.0, 2_147_483_647.0) as i32
}

/// Saturated addition on interval precision fields.
///
/// The C++ interval code uses unchecked `int` arithmetic on `lsb` values. In
/// Rust debug builds that would panic on overflow, so the port clamps to the
/// nearest representable precision instead. This preserves release/debug
/// consistency while keeping the same "best effort" role for precision fields.
#[inline]
#[must_use]
pub fn saturated_precision_add(lhs: i32, rhs: i32) -> i32 {
    lhs.saturating_add(rhs)
}

/// Saturated subtraction on interval precision fields.
///
/// See [`saturated_precision_add`] for the rationale.
#[inline]
#[must_use]
pub fn saturated_precision_sub(lhs: i32, rhs: i32) -> i32 {
    lhs.saturating_sub(rhs)
}

/// Interval value `[lo, hi]` with LSB precision field.
///
/// Empty when either bound is NaN.
/// Default (C++ default ctor): `[f64::MIN, f64::MAX, lsb=-24]`.
///
/// # C++ source
/// `compiler/interval/interval_def.hh` — `itv::interval`
#[derive(Clone, Copy)]
pub struct Interval {
    lo: f64,
    hi: f64,
    lsb: i32,
}

impl Interval {
    /// Default interval: `[f64::MIN, f64::MAX]` with `lsb = -24`.
    #[inline]
    #[must_use]
    pub fn new_default() -> Self {
        Self {
            lo: f64::MIN,
            hi: f64::MAX,
            lsb: -24,
        }
    }

    /// Construct an ordered interval usable in a `const` item.
    ///
    /// Unlike [`Self::new`], this performs no normalization: callers must pass
    /// `lo <= hi` and no NaN. It exists for compile-time constants such as the
    /// domains of `sqrt` or `asin`, where the bounds are written by hand.
    #[must_use]
    pub const fn const_new(lo: f64, hi: f64) -> Self {
        Self { lo, hi, lsb: -24 }
    }

    /// Construct from two bounds and LSB precision.
    ///
    /// Matches C++ `interval(double n, double m, int lsb)`.
    #[must_use]
    pub fn new(n: f64, m: f64, lsb: i32) -> Self {
        if n == 0.0 && m == 0.0 {
            return Self {
                lo: 0.0,
                hi: 0.0,
                lsb: 0,
            };
        }
        let lsb = if lsb == i32::MIN { -24 } else { lsb };
        if n.is_nan() || m.is_nan() {
            return Self {
                lo: f64::NAN,
                hi: f64::NAN,
                lsb: 0,
            };
        }
        Self {
            lo: n.min(m),
            hi: n.max(m),
            lsb,
        }
    }

    /// Singleton interval whose precision accommodates `x` exactly.
    ///
    /// Matches C++ `explicit interval(double x)`.
    #[must_use]
    pub fn from_scalar(x: f64) -> Self {
        if x == 0.0 {
            return Self {
                lo: 0.0,
                hi: 0.0,
                lsb: 0,
            };
        }
        let mut p: i32 = 0;
        let mut y = x;
        while y.fract() != 0.0 {
            y *= 2.0;
            p -= 1;
        }
        Self {
            lo: x,
            hi: x,
            lsb: p,
        }
    }

    #[inline]
    #[must_use]
    pub fn lo(&self) -> f64 {
        self.lo
    }
    #[inline]
    #[must_use]
    pub fn hi(&self) -> f64 {
        self.hi
    }
    #[inline]
    #[must_use]
    pub fn lsb(&self) -> i32 {
        self.lsb
    }
    #[inline]
    #[must_use]
    pub fn size(&self) -> f64 {
        self.hi - self.lo
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lo.is_nan() || self.hi.is_nan()
    }

    #[inline]
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.is_empty()
    }

    #[inline]
    #[must_use]
    pub fn is_unbounded(&self) -> bool {
        self.lo.is_infinite() || self.hi.is_infinite()
    }

    #[inline]
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        !self.is_unbounded()
    }

    #[inline]
    #[must_use]
    pub fn has(&self, x: f64) -> bool {
        self.lo <= x && self.hi >= x
    }

    #[inline]
    #[must_use]
    pub fn is(&self, x: f64) -> bool {
        self.lo == x && self.hi == x
    }

    #[inline]
    #[must_use]
    pub fn has_zero(&self) -> bool {
        self.has(0.0)
    }

    #[inline]
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.is(0.0)
    }

    #[inline]
    #[must_use]
    pub fn is_const(&self) -> bool {
        self.lo == self.hi && !self.lo.is_nan()
    }

    #[inline]
    #[must_use]
    pub fn is_power_of2(&self) -> bool {
        let n = self.hi as i32;
        self.is_const() && (n & n.wrapping_neg()) == n
    }

    #[inline]
    #[must_use]
    pub fn is_bitmask(&self) -> bool {
        let n = (self.hi as i32).wrapping_add(1);
        self.is_const() && (n & n.wrapping_neg()) == n
    }

    /// Position of the MSB of the interval's value range.
    ///
    /// Matches C++ `itv::interval::msb()`.
    #[must_use]
    pub fn msb(&self) -> i32 {
        if self.lo == 0.0 && self.hi == 0.0 {
            return 0;
        }
        let range = self.lo.abs().max(self.hi.abs());
        if range.is_infinite() {
            return 31;
        }
        range.log2().ceil() as i32
    }
}

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            write!(f, "interval()")
        } else {
            write!(f, "interval({},{},{})", self.lo, self.hi, self.lsb)
        }
    }
}

impl fmt::Debug for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// -------------------------------------------------------------------------
// Set operations (interval_def.hh)
// -------------------------------------------------------------------------

/// Empty interval with NaN bounds. C++ `itv::empty()`.
#[inline]
#[must_use]
pub fn empty() -> Interval {
    Interval {
        lo: f64::NAN,
        hi: f64::NAN,
        lsb: 0,
    }
}

/// Intersection. Precision = min(lsb). C++ `itv::intersection()`.
#[must_use]
pub fn intersection(i: Interval, j: Interval) -> Interval {
    if i.is_empty() {
        return i;
    }
    if j.is_empty() {
        return j;
    }
    let l = i.lo.max(j.lo);
    let h = i.hi.min(j.hi);
    let p = i.lsb.min(j.lsb);
    if l > h {
        empty()
    } else {
        Interval::new(l, h, p)
    }
}

/// Union. Precision = min(lsb). C++ `itv::reunion()`.
#[must_use]
pub fn reunion(i: Interval, j: Interval) -> Interval {
    if i.is_empty() {
        return j;
    }
    if j.is_empty() {
        return i;
    }
    let l = i.lo.min(j.lo);
    let h = i.hi.max(j.hi);
    let p = i.lsb.min(j.lsb);
    Interval::new(l, h, p)
}

/// Singleton for scalar x. C++ `itv::singleton()`.
#[must_use]
pub fn singleton(x: f64) -> Interval {
    if x == 0.0 {
        return Interval::new(0.0, 0.0, 0);
    }
    let m = x.abs().log2().floor() as i32;
    Interval::new(x, x, m - 32)
}

// -------------------------------------------------------------------------
// Comparison predicates
// -------------------------------------------------------------------------

impl PartialEq for Interval {
    fn eq(&self, other: &Self) -> bool {
        (self.is_empty() && other.is_empty()) || (self.lo == other.lo && self.hi == other.hi)
    }
}
impl Eq for Interval {}

/// Subset test (C++ `operator<=` on intervals).
#[must_use]
pub fn is_subset(i: Interval, j: Interval) -> bool {
    i.lo >= j.lo && i.hi <= j.hi
}

/// Strict subset.
#[must_use]
pub fn is_strict_subset(i: Interval, j: Interval) -> bool {
    is_subset(i, j) && i != j
}

// -------------------------------------------------------------------------
// Convenience re-exports
// -------------------------------------------------------------------------

pub use ops::{
    arithmetic::{abs, add, div, inv, mod_interval, mul, neg, sub},
    casts::{float_cast, float_num, int_cast, int_num, int64_num, label},
    delay_table::{delay, mem},
    logic::{and, eq, ge, gt, le, lsh, lt, max, min, ne, not, or, rsh, xor},
    math::{ceil, exp, exp10, floor, log, log10, pow, rint, round, sqrt},
    missing::{
        attach, bit_cast, fix_point_update, foreign_const, foreign_function, foreign_var, r#gen,
        highest, input, lowest, nil, output, prefix, rd_tbl, select2, sound_file,
        sound_file_buffer, sound_file_length, sound_file_rate, waveform, wr_tbl,
    },
    trig::{acos, acosh, asin, asinh, atan, atan2, atanh, cos, cosh, sin, sinh, tan, tanh},
    ui::{button, checkbox, hbargraph, hslider, num_entry, vbargraph, vslider},
};

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_default() {
        let i = Interval::new_default();
        assert_eq!(i.lo(), f64::MIN);
        assert_eq!(i.hi(), f64::MAX);
        assert_eq!(i.lsb(), -24);
    }

    #[test]
    fn interval_new_zero_zero() {
        let i = Interval::new(0.0, 0.0, -5);
        assert_eq!(i.lo(), 0.0);
        assert_eq!(i.hi(), 0.0);
        assert_eq!(i.lsb(), 0);
    }

    #[test]
    fn interval_new_swaps_bounds() {
        let i = Interval::new(10.0, 3.0, -5);
        assert_eq!(i.lo(), 3.0);
        assert_eq!(i.hi(), 10.0);
    }

    #[test]
    fn empty_interval() {
        let e = empty();
        assert!(e.is_empty());
        assert!(!e.is_valid());
    }

    #[test]
    fn singleton_zero() {
        let s = singleton(0.0);
        assert_eq!(s.lo(), 0.0);
        assert_eq!(s.hi(), 0.0);
        assert_eq!(s.lsb(), 0);
    }

    #[test]
    fn reunion_with_empty() {
        let i = Interval::new(1.0, 3.0, -5);
        let r = reunion(i, empty());
        assert_eq!(r.lo(), 1.0);
        assert_eq!(r.hi(), 3.0);
    }

    #[test]
    fn intersection_overlap() {
        let a = Interval::new(0.0, 10.0, -5);
        let b = Interval::new(5.0, 20.0, -5);
        let c = intersection(a, b);
        assert_eq!(c.lo(), 5.0);
        assert_eq!(c.hi(), 10.0);
    }

    #[test]
    fn intersection_disjoint() {
        let a = Interval::new(0.0, 5.0, 0);
        let b = Interval::new(6.0, 10.0, 0);
        assert!(intersection(a, b).is_empty());
    }

    #[test]
    fn saturated_cast() {
        assert_eq!(saturated_int_cast(3.7), 3);
        assert_eq!(saturated_int_cast(-3.7), -3);
        assert_eq!(saturated_int_cast(f64::INFINITY), i32::MAX);
        assert_eq!(saturated_int_cast(f64::NEG_INFINITY), i32::MIN);
    }

    #[test]
    fn msb_zero() {
        assert_eq!(Interval::new(0.0, 0.0, 0).msb(), 0);
    }

    #[test]
    fn msb_inf() {
        assert_eq!(Interval::new(f64::NEG_INFINITY, f64::INFINITY, 0).msb(), 31);
    }
}
