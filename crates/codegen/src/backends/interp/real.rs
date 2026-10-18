//! [`FbcReal`] trait — generic REAL type for the FBC interpreter.
//!
//! # Source provenance (C++)
//! - `template <class REAL>` parameter used throughout
//!   `compiler/generator/interpreter/` (instantiated as `float` or `double`).
//!
//! # Design notes
//! - Replaces the C++ `template <class REAL>` pattern with a Rust trait bound.
//! - Both `f32` and `f64` implement this trait, delegating to `std` math.
//! - The trait carries all math operations needed by the interpreter dispatch
//!   loop so that the executor remains generic and monomorphized at compile time.
//!
//! # API mapping status
//! - No single C++ type; this trait encodes the implicit contract on `REAL`.

use std::fmt;

/// C++ `std::max(a, b)`, which is `(a < b) ? b : a`: on a NaN operand the
/// result depends on the operand order, unlike Rust's `f32::max`. The
/// interpreter executes `Maxf` with it, as the C++ interpreter does, and its
/// constant folding must give the same value.
pub(crate) fn cpp_max<R: PartialOrd>(a: R, b: R) -> R {
    if a < b { b } else { a }
}

/// C++ `std::min(a, b)`, which is `(b < a) ? b : a` (see [`cpp_max`]).
pub(crate) fn cpp_min<R: PartialOrd>(a: R, b: R) -> R {
    if b < a { b } else { a }
}

/// Trait bound for the interpreter's REAL type parameter.
///
/// Replaces the C++ `template <class REAL>` pattern used in
/// `FBCBasicInstruction<REAL>`, `FBCBlockInstruction<REAL>`,
/// `interpreter_dsp_aux<REAL, TRACE>`, etc.
///
/// # Implementors
/// - [`f32`] — single-precision (default Faust `FAUSTFLOAT`).
/// - [`f64`] — double-precision (`-double` mode).
///
/// # Example
/// ```
/// use codegen::backends::interp::FbcReal;
///
/// fn add_reals<R: FbcReal>(a: R, b: R) -> R {
///     a + b
/// }
///
/// let result: f32 = add_reals(1.0_f32, 2.0_f32);
/// assert!((result - 3.0).abs() < 1e-6);
/// ```
pub trait FbcReal:
    Copy
    + Default
    + PartialOrd
    + std::ops::Add<Output = Self>
    + std::ops::Sub<Output = Self>
    + std::ops::Mul<Output = Self>
    + std::ops::Div<Output = Self>
    + std::ops::Rem<Output = Self>
    + std::ops::Neg<Output = Self>
    + fmt::Debug
    + fmt::Display
    + std::str::FromStr
    + Send
    + Sync
    + 'static
{
    /// Name of this type for diagnostics (`"f32"` or `"f64"`).
    const TYPE_NAME: &'static str;

    /// Converts from `i32`.
    fn from_i32(v: i32) -> Self;

    /// Converts to `i32` (truncating).
    fn to_i32(self) -> i32;

    /// Converts from `f64`.
    fn from_f64(v: f64) -> Self;

    /// Converts to `f64`.
    fn to_f64(self) -> f64;

    // ── Unary math ───────────────────────────────────────────────────────

    /// Absolute value, integer-style (`kAbs` opcode).
    ///
    /// Truncates `self` to `i32`, applies `i32::abs()`, then casts the result
    /// back to `Self`.  Differs from [`Self::fbc_absf`] which keeps full
    /// floating-point precision.
    fn fbc_abs(self) -> Self;

    /// Absolute value (float-style for `kAbsf`).
    fn fbc_absf(self) -> Self;

    /// Arc cosine.
    fn fbc_acos(self) -> Self;

    /// Hyperbolic arc cosine.
    fn fbc_acosh(self) -> Self;

    /// Arc sine.
    fn fbc_asin(self) -> Self;

    /// Hyperbolic arc sine.
    fn fbc_asinh(self) -> Self;

    /// Arc tangent.
    fn fbc_atan(self) -> Self;

    /// Hyperbolic arc tangent.
    fn fbc_atanh(self) -> Self;

    /// Ceiling.
    fn fbc_ceil(self) -> Self;

    /// Cosine.
    fn fbc_cos(self) -> Self;

    /// Hyperbolic cosine.
    fn fbc_cosh(self) -> Self;

    /// Exponential (e^x).
    fn fbc_exp(self) -> Self;

    /// Floor.
    fn fbc_floor(self) -> Self;

    /// Natural logarithm.
    fn fbc_log(self) -> Self;

    /// Base-10 logarithm.
    fn fbc_log10(self) -> Self;

    /// Round to nearest integer (ties to even).
    fn fbc_rint(self) -> Self;

    /// Round to nearest integer (ties away from zero).
    fn fbc_round(self) -> Self;

    /// Sine.
    fn fbc_sin(self) -> Self;

    /// Hyperbolic sine.
    fn fbc_sinh(self) -> Self;

    /// Square root.
    fn fbc_sqrt(self) -> Self;

    /// Tangent.
    fn fbc_tan(self) -> Self;

    /// Hyperbolic tangent.
    fn fbc_tanh(self) -> Self;

    /// Returns `true` if NaN.
    fn fbc_is_nan(self) -> bool;

    /// Returns `true` if infinite.
    fn fbc_is_infinite(self) -> bool;

    // ── Binary math ──────────────────────────────────────────────────────

    /// Two-argument arc tangent.
    fn fbc_atan2(self, other: Self) -> Self;

    /// Floating-point modulus (truncated division), matching C++ `std::fmod()`.
    fn fbc_fmod(self, other: Self) -> Self;

    /// IEEE 754 remainder, matching C++ `std::remainder()`.
    ///
    /// This differs from [`fbc_fmod`](Self::fbc_fmod) which uses truncated
    /// division. The IEEE remainder is `a - round_ties_even(a/b) * b`.
    fn fbc_remainder(self, other: Self) -> Self {
        self - (self / other).fbc_rint() * other
    }

    /// Power.
    fn fbc_pow(self, exp: Self) -> Self;

    /// Minimum (real).
    fn fbc_min(self, other: Self) -> Self;

    /// Maximum (real).
    fn fbc_max(self, other: Self) -> Self;

    /// Copy sign.
    fn fbc_copysign(self, sign: Self) -> Self;

    // ── Bitcast ──────────────────────────────────────────────────────────

    /// Reinterpret bits as `i32` (for `kBitcastInt`).
    ///
    /// For `f64`, this truncates to 32-bit representation matching C++
    /// `*reinterpret_cast<int*>(&val)` behavior.
    fn to_bits_i32(self) -> i32;

    /// Reinterpret `i32` bits as this REAL type (for `kBitcastReal`).
    fn from_bits_i32(v: i32) -> Self;
}

// ── Macro to avoid duplicating 30+ identical method bodies for f32/f64 ───

macro_rules! impl_fbc_real {
    ($ty:ty, $type_name:literal, $bitcast_to:expr, $bitcast_from:expr) => {
        impl FbcReal for $ty {
            const TYPE_NAME: &'static str = $type_name;

            /// Converts from `i32` using Rust's primitive cast semantics.
            #[inline]
            fn from_i32(v: i32) -> Self {
                v as Self
            }
            /// Converts to `i32` using Rust's primitive cast semantics.
            #[inline]
            fn to_i32(self) -> i32 {
                self as i32
            }
            /// Converts from `f64` using Rust's primitive cast semantics.
            #[inline]
            fn from_f64(v: f64) -> Self {
                v as Self
            }
            /// Converts to `f64` using Rust's primitive cast semantics.
            #[inline]
            fn to_f64(self) -> f64 {
                self as f64
            }

            // ── Unary math ───────────────────────────────────────────────
            /// Implements Faust `kAbs` integer-style absolute value.
            #[inline]
            fn fbc_abs(self) -> Self {
                // C++ kAbs: truncate to int, abs, cast back.
                (self as i32).unsigned_abs() as Self
            }
            /// Implements Faust `kAbsf` floating absolute value.
            #[inline]
            fn fbc_absf(self) -> Self {
                self.abs()
            }
            /// Computes arc cosine.
            #[inline]
            fn fbc_acos(self) -> Self {
                self.acos()
            }
            /// Computes hyperbolic arc cosine.
            #[inline]
            fn fbc_acosh(self) -> Self {
                self.acosh()
            }
            /// Computes arc sine.
            #[inline]
            fn fbc_asin(self) -> Self {
                self.asin()
            }
            /// Computes hyperbolic arc sine.
            #[inline]
            fn fbc_asinh(self) -> Self {
                self.asinh()
            }
            /// Computes arc tangent.
            #[inline]
            fn fbc_atan(self) -> Self {
                self.atan()
            }
            /// Computes hyperbolic arc tangent.
            #[inline]
            fn fbc_atanh(self) -> Self {
                self.atanh()
            }
            /// Rounds toward positive infinity.
            #[inline]
            fn fbc_ceil(self) -> Self {
                self.ceil()
            }
            /// Computes cosine.
            #[inline]
            fn fbc_cos(self) -> Self {
                self.cos()
            }
            /// Computes hyperbolic cosine.
            #[inline]
            fn fbc_cosh(self) -> Self {
                self.cosh()
            }
            /// Computes the natural exponential.
            #[inline]
            fn fbc_exp(self) -> Self {
                self.exp()
            }
            /// Rounds toward negative infinity.
            #[inline]
            fn fbc_floor(self) -> Self {
                self.floor()
            }
            /// Computes the natural logarithm.
            #[inline]
            fn fbc_log(self) -> Self {
                self.ln()
            }
            /// Computes the base-10 logarithm.
            #[inline]
            fn fbc_log10(self) -> Self {
                self.log10()
            }
            /// Rounds to nearest with ties to even.
            #[inline]
            fn fbc_rint(self) -> Self {
                self.round_ties_even()
            }
            /// Rounds to nearest using the primitive `round` behavior.
            #[inline]
            fn fbc_round(self) -> Self {
                self.round()
            }
            /// Computes sine.
            #[inline]
            fn fbc_sin(self) -> Self {
                self.sin()
            }
            /// Computes hyperbolic sine.
            #[inline]
            fn fbc_sinh(self) -> Self {
                self.sinh()
            }
            /// Computes square root.
            #[inline]
            fn fbc_sqrt(self) -> Self {
                self.sqrt()
            }
            /// Computes tangent.
            #[inline]
            fn fbc_tan(self) -> Self {
                self.tan()
            }
            /// Computes hyperbolic tangent.
            #[inline]
            fn fbc_tanh(self) -> Self {
                self.tanh()
            }
            /// Returns whether the value is NaN.
            #[inline]
            fn fbc_is_nan(self) -> bool {
                self.is_nan()
            }
            /// Returns whether the value is infinite.
            #[inline]
            fn fbc_is_infinite(self) -> bool {
                self.is_infinite()
            }

            // ── Binary math ──────────────────────────────────────────────
            /// Computes two-argument arc tangent.
            #[inline]
            fn fbc_atan2(self, other: Self) -> Self {
                self.atan2(other)
            }
            /// Computes floating modulus using primitive remainder.
            #[inline]
            fn fbc_fmod(self, other: Self) -> Self {
                self % other
            }
            /// Computes exponentiation.
            #[inline]
            fn fbc_pow(self, exp: Self) -> Self {
                self.powf(exp)
            }
            /// Computes the numeric minimum.
            #[inline]
            fn fbc_min(self, other: Self) -> Self {
                self.min(other)
            }
            /// Computes the numeric maximum.
            #[inline]
            fn fbc_max(self, other: Self) -> Self {
                self.max(other)
            }
            /// Copies the sign bit from `sign`.
            #[inline]
            fn fbc_copysign(self, sign: Self) -> Self {
                self.copysign(sign)
            }

            // ── Bitcast (type-specific) ──────────────────────────────────
            /// Reinterprets the REAL bits as an `i32` according to the implementation hook.
            #[inline]
            fn to_bits_i32(self) -> i32 {
                ($bitcast_to)(self)
            }
            /// Reinterprets `i32` bits as a REAL according to the implementation hook.
            #[inline]
            fn from_bits_i32(v: i32) -> Self {
                ($bitcast_from)(v)
            }
        }
    };
}

impl_fbc_real!(f32, "f32", f32_to_bits_i32, f32_from_bits_i32);
impl_fbc_real!(f64, "f64", f64_to_bits_i32, f64_from_bits_i32);

#[inline]
fn f32_to_bits_i32(v: f32) -> i32 {
    i32::from_ne_bytes(v.to_ne_bytes())
}

#[inline]
fn f32_from_bits_i32(v: i32) -> f32 {
    f32::from_ne_bytes(v.to_ne_bytes())
}

#[inline]
fn f64_to_bits_i32(v: f64) -> i32 {
    // C++ does `*reinterpret_cast<int*>(&val)` which reads the low 32 bits.
    let bytes = v.to_ne_bytes();
    i32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[inline]
fn f64_from_bits_i32(v: i32) -> f64 {
    // C++ does `*reinterpret_cast<double*>(&val)` — we zero-extend to 8 bytes.
    let int_bytes = v.to_ne_bytes();
    f64::from_ne_bytes([
        int_bytes[0],
        int_bytes[1],
        int_bytes[2],
        int_bytes[3],
        0,
        0,
        0,
        0,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── f32 tests ────────────────────────────────────────────────────────

    #[test]
    fn f32_from_i32_and_back() {
        assert_eq!(f32::from_i32(42), 42.0_f32);
        assert_eq!(42.0_f32.to_i32(), 42);
        // Truncation toward zero.
        assert_eq!(3.9_f32.to_i32(), 3);
        assert_eq!((-3.9_f32).to_i32(), -3);
    }

    #[test]
    fn f32_from_f64_and_back() {
        let v = 3.125_f64;
        let f: f32 = FbcReal::from_f64(v);
        assert!((f - 3.125_f32).abs() < 1e-5);
        assert!((f.to_f64() - 3.125).abs() < 1e-5);
    }

    #[test]
    fn f32_unary_math() {
        let half_pi = std::f32::consts::FRAC_PI_2;
        assert!((half_pi.fbc_sin() - 1.0).abs() < 1e-6);
        assert!((0.0_f32.fbc_cos() - 1.0).abs() < 1e-6);
        assert!((1.0_f32.fbc_exp() - std::f32::consts::E).abs() < 1e-5);
        assert!((1.0_f32.fbc_log() - 0.0).abs() < 1e-6);
        assert!((100.0_f32.fbc_log10() - 2.0).abs() < 1e-5);
        assert!((4.0_f32.fbc_sqrt() - 2.0).abs() < 1e-6);
        assert!(((-3.5_f32).fbc_absf() - 3.5).abs() < 1e-6);
        assert!((1.6_f32.fbc_floor() - 1.0).abs() < 1e-6);
        assert!((1.1_f32.fbc_ceil() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn f32_abs_is_integer_style() {
        // kAbs: truncate to int, abs, cast back.
        assert!(((-3.7_f32).fbc_abs() - 3.0).abs() < 1e-6);
        assert!((3.7_f32.fbc_abs() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn f32_rint() {
        // rint rounds to nearest even.
        assert!((2.5_f32.fbc_rint() - 2.0).abs() < 1e-6);
        assert!((3.5_f32.fbc_rint() - 4.0).abs() < 1e-6);
        assert!((0.5_f32.fbc_rint() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn f32_round() {
        // round goes away from zero on ties.
        assert!((2.5_f32.fbc_round() - 3.0).abs() < 1e-6);
        assert!(((-2.5_f32).fbc_round() - (-3.0)).abs() < 1e-6);
    }

    #[test]
    fn f32_binary_math() {
        assert!((8.0_f32.fbc_fmod(3.0) - 2.0).abs() < 1e-6);
        assert!((2.0_f32.fbc_pow(10.0) - 1024.0).abs() < 1e-3);
        assert!((3.0_f32.fbc_min(5.0) - 3.0).abs() < 1e-6);
        assert!((3.0_f32.fbc_max(5.0) - 5.0).abs() < 1e-6);
        assert!(((-1.0_f32).fbc_copysign(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn f32_nan_inf() {
        assert!(f32::NAN.fbc_is_nan());
        assert!(!1.0_f32.fbc_is_nan());
        assert!(f32::INFINITY.fbc_is_infinite());
        assert!(!1.0_f32.fbc_is_infinite());
    }

    #[test]
    fn f32_bitcast_roundtrip() {
        let v = 3.125_f32;
        let bits = v.to_bits_i32();
        let back = f32::from_bits_i32(bits);
        assert_eq!(v, back);
    }

    // ── f64 tests ────────────────────────────────────────────────────────

    #[test]
    fn f64_from_i32_and_back() {
        assert_eq!(f64::from_i32(42), 42.0_f64);
        assert_eq!(42.0_f64.to_i32(), 42);
    }

    #[test]
    fn f64_unary_math() {
        assert!((1.0_f64.fbc_exp() - std::f64::consts::E).abs() < 1e-10);
        assert!((4.0_f64.fbc_sqrt() - 2.0).abs() < 1e-10);
        assert!(((-7.3_f64).fbc_absf() - 7.3).abs() < 1e-10);
    }

    #[test]
    fn f64_rint() {
        assert!((2.5_f64.fbc_rint() - 2.0).abs() < 1e-10);
        assert!((3.5_f64.fbc_rint() - 4.0).abs() < 1e-10);
    }

    #[test]
    fn f64_binary_math() {
        assert!((8.0_f64.fbc_fmod(3.0) - 2.0).abs() < 1e-10);
        assert!((2.0_f64.fbc_pow(10.0) - 1024.0).abs() < 1e-6);
    }

    #[test]
    fn f64_bitcast_to_i32_reads_low_bytes() {
        // For f64, to_bits_i32 reads the low 32 bits.
        let v = 1.0_f64;
        let bits = v.to_bits_i32();
        // 1.0 in f64 is 0x3FF0_0000_0000_0000 in big-endian.
        // Low 4 bytes (little-endian) are 0x00000000.
        assert_eq!(bits, 0);
    }

    #[test]
    fn type_name() {
        assert_eq!(f32::TYPE_NAME, "f32");
        assert_eq!(f64::TYPE_NAME, "f64");
    }

    // ── Generic function test ────────────────────────────────────────────

    #[test]
    fn generic_add() {
        fn add_reals<R: FbcReal>(a: R, b: R) -> R {
            a + b
        }
        assert!((add_reals(1.0_f32, 2.0_f32) - 3.0_f32).abs() < 1e-6);
        assert!((add_reals(1.0_f64, 2.0_f64) - 3.0_f64).abs() < 1e-10);
    }
}
