//! Host symbols registered for Cranelift JIT modules.
//!
//! The backend imports math helpers and caller-provided symbols through the JIT
//! symbol registry. These wrappers keep the host ABI explicit and backend-local.

use super::*;

// ── Host math wrappers ────────────────────────────────────────────────────────
//
// Thin `extern "C"` shims that delegate to the Rust standard library.  These
// are registered by [`register_host_symbols`] so that the Cranelift JIT can
// resolve imported math symbols at finalization time.
//
// Both `f32` (`*f`) and `f64` variants are provided.  The implementations are
// intentionally minimal: no error handling, NaN propagation matches the
// interpreter and C++ backend paths.

extern "C" fn host_sinf(x: f32) -> f32 {
    x.sin()
}

extern "C" fn host_sin(x: f64) -> f64 {
    x.sin()
}

extern "C" fn host_cosf(x: f32) -> f32 {
    x.cos()
}

extern "C" fn host_cos(x: f64) -> f64 {
    x.cos()
}

extern "C" fn host_expf(x: f32) -> f32 {
    x.exp()
}

extern "C" fn host_exp(x: f64) -> f64 {
    x.exp()
}

// Rust's standard library has no direct `exp10`, so it is expressed as `10^x`.
extern "C" fn host_exp10f(x: f32) -> f32 {
    10.0_f32.powf(x)
}

extern "C" fn host_exp10(x: f64) -> f64 {
    10.0_f64.powf(x)
}

extern "C" fn host_logf(x: f32) -> f32 {
    x.ln()
}

extern "C" fn host_log(x: f64) -> f64 {
    x.ln()
}

extern "C" fn host_log10f(x: f32) -> f32 {
    x.log10()
}

extern "C" fn host_log10(x: f64) -> f64 {
    x.log10()
}

extern "C" fn host_sqrtf(x: f32) -> f32 {
    x.sqrt()
}

extern "C" fn host_sqrt(x: f64) -> f64 {
    x.sqrt()
}

extern "C" fn host_fabsf(x: f32) -> f32 {
    x.abs()
}

extern "C" fn host_fabs(x: f64) -> f64 {
    x.abs()
}

// `i32::MIN.abs()` would overflow/panic; `checked_abs` falls back to the
// original (still-negative) value instead, matching C's `abs()` behavior.
extern "C" fn host_abs(a: i32) -> i32 {
    a.checked_abs().unwrap_or(a)
}

extern "C" fn host_min_i(a: i32, b: i32) -> i32 {
    a.min(b)
}

extern "C" fn host_max_i(a: i32, b: i32) -> i32 {
    a.max(b)
}

extern "C" fn host_floorf(x: f32) -> f32 {
    x.floor()
}

extern "C" fn host_floor(x: f64) -> f64 {
    x.floor()
}

extern "C" fn host_ceilf(x: f32) -> f32 {
    x.ceil()
}

extern "C" fn host_ceil(x: f64) -> f64 {
    x.ceil()
}

extern "C" fn host_tanf(x: f32) -> f32 {
    x.tan()
}

extern "C" fn host_tan(x: f64) -> f64 {
    x.tan()
}

extern "C" fn host_atanf(x: f32) -> f32 {
    x.atan()
}

extern "C" fn host_atan(x: f64) -> f64 {
    x.atan()
}

extern "C" fn host_asinf(x: f32) -> f32 {
    x.asin()
}

extern "C" fn host_asin(x: f64) -> f64 {
    x.asin()
}

extern "C" fn host_acosf(x: f32) -> f32 {
    x.acos()
}

extern "C" fn host_acos(x: f64) -> f64 {
    x.acos()
}

extern "C" fn host_roundf(x: f32) -> f32 {
    x.round()
}

extern "C" fn host_round(x: f64) -> f64 {
    x.round()
}

extern "C" fn host_fminf(a: f32, b: f32) -> f32 {
    a.min(b)
}

extern "C" fn host_fmin(a: f64, b: f64) -> f64 {
    a.min(b)
}

extern "C" fn host_fmaxf(a: f32, b: f32) -> f32 {
    a.max(b)
}

extern "C" fn host_fmax(a: f64, b: f64) -> f64 {
    a.max(b)
}

extern "C" fn host_powf(a: f32, b: f32) -> f32 {
    a.powf(b)
}

extern "C" fn host_pow(a: f64, b: f64) -> f64 {
    a.powf(b)
}

extern "C" fn host_atan2f(a: f32, b: f32) -> f32 {
    a.atan2(b)
}

extern "C" fn host_atan2(a: f64, b: f64) -> f64 {
    a.atan2(b)
}

extern "C" fn host_fmodf(a: f32, b: f32) -> f32 {
    a % b
}

extern "C" fn host_fmod(a: f64, b: f64) -> f64 {
    a % b
}

// C's `rint` rounds half-to-even (banker's rounding) under the default
// rounding mode, unlike Rust's `round()` which rounds half away from zero.
extern "C" fn host_rintf(a: f32) -> f32 {
    a.round_ties_even()
}

extern "C" fn host_rint(a: f64) -> f64 {
    a.round_ties_even()
}

// C's `remainder` (IEEE 754 remainder) differs from `%`: it is computed as
// `a - round_ties_even(a / b) * b`, so the result can be negative even when
// `a` is positive, and has smaller magnitude than `b / 2`.
extern "C" fn host_remainderf(a: f32, b: f32) -> f32 {
    a - (a / b).round_ties_even() * b
}

extern "C" fn host_remainder(a: f64, b: f64) -> f64 {
    a - (a / b).round_ties_even() * b
}

extern "C" fn host_isnanf(x: f32) -> i32 {
    i32::from(x.is_nan())
}

extern "C" fn host_isnan(x: f64) -> i32 {
    i32::from(x.is_nan())
}

extern "C" fn host_isinff(x: f32) -> i32 {
    i32::from(x.is_infinite())
}

extern "C" fn host_isinf(x: f64) -> i32 {
    i32::from(x.is_infinite())
}

extern "C" fn host_copysignf(a: f32, b: f32) -> f32 {
    a.copysign(b)
}

extern "C" fn host_copysign(a: f64, b: f64) -> f64 {
    a.copysign(b)
}

extern "C" fn host_acoshf(x: f32) -> f32 {
    x.acosh()
}

extern "C" fn host_acosh(x: f64) -> f64 {
    x.acosh()
}

extern "C" fn host_asinhf(x: f32) -> f32 {
    x.asinh()
}

extern "C" fn host_asinh(x: f64) -> f64 {
    x.asinh()
}

extern "C" fn host_atanhf(x: f32) -> f32 {
    x.atanh()
}

extern "C" fn host_atanh(x: f64) -> f64 {
    x.atanh()
}

extern "C" fn host_coshf(x: f32) -> f32 {
    x.cosh()
}

extern "C" fn host_cosh(x: f64) -> f64 {
    x.cosh()
}

extern "C" fn host_sinhf(x: f32) -> f32 {
    x.sinh()
}

extern "C" fn host_sinh(x: f64) -> f64 {
    x.sinh()
}

extern "C" fn host_tanhf(x: f32) -> f32 {
    x.tanh()
}

extern "C" fn host_tanh(x: f64) -> f64 {
    x.tanh()
}

/// Registers Rust host math functions as JIT-importable symbols.
///
/// The Cranelift lowering emits imported calls for many FIR math operations
/// (`sin`, `pow`, `fmin`, etc.). This function binds those symbol names to Rust
/// implementations so the JIT can resolve them during finalization.
///
/// Both `f32` (`*f`) and `f64` symbol variants are registered where the subset
/// lowering supports both result types. The `host_*` helpers immediately above
/// are intentionally tiny wrappers whose semantics match the interpreter/C++
/// path (notably `rint` and `remainder`).
pub(crate) fn register_host_symbols(jit_builder: &mut JITBuilder) {
    jit_builder.symbol("sinf", host_sinf as *const u8);
    jit_builder.symbol("sin", host_sin as *const u8);
    jit_builder.symbol("cosf", host_cosf as *const u8);
    jit_builder.symbol("cos", host_cos as *const u8);
    jit_builder.symbol("expf", host_expf as *const u8);
    jit_builder.symbol("exp", host_exp as *const u8);
    jit_builder.symbol("exp10f", host_exp10f as *const u8);
    jit_builder.symbol("exp10", host_exp10 as *const u8);
    jit_builder.symbol("logf", host_logf as *const u8);
    jit_builder.symbol("log", host_log as *const u8);
    jit_builder.symbol("log10f", host_log10f as *const u8);
    jit_builder.symbol("log10", host_log10 as *const u8);
    jit_builder.symbol("sqrtf", host_sqrtf as *const u8);
    jit_builder.symbol("sqrt", host_sqrt as *const u8);
    jit_builder.symbol("fabsf", host_fabsf as *const u8);
    jit_builder.symbol("fabs", host_fabs as *const u8);
    jit_builder.symbol("abs", host_abs as *const u8);
    jit_builder.symbol("floorf", host_floorf as *const u8);
    jit_builder.symbol("floor", host_floor as *const u8);
    jit_builder.symbol("ceilf", host_ceilf as *const u8);
    jit_builder.symbol("ceil", host_ceil as *const u8);
    jit_builder.symbol("tanf", host_tanf as *const u8);
    jit_builder.symbol("tan", host_tan as *const u8);
    jit_builder.symbol("atanf", host_atanf as *const u8);
    jit_builder.symbol("atan", host_atan as *const u8);
    jit_builder.symbol("asinf", host_asinf as *const u8);
    jit_builder.symbol("asin", host_asin as *const u8);
    jit_builder.symbol("acosf", host_acosf as *const u8);
    jit_builder.symbol("acos", host_acos as *const u8);
    jit_builder.symbol("rintf", host_rintf as *const u8);
    jit_builder.symbol("rint", host_rint as *const u8);
    jit_builder.symbol("roundf", host_roundf as *const u8);
    jit_builder.symbol("round", host_round as *const u8);
    jit_builder.symbol("min_i", host_min_i as *const u8);
    jit_builder.symbol("max_i", host_max_i as *const u8);
    jit_builder.symbol("fminf", host_fminf as *const u8);
    jit_builder.symbol("fmin", host_fmin as *const u8);
    jit_builder.symbol("fmaxf", host_fmaxf as *const u8);
    jit_builder.symbol("fmax", host_fmax as *const u8);
    jit_builder.symbol("powf", host_powf as *const u8);
    jit_builder.symbol("pow", host_pow as *const u8);
    jit_builder.symbol("atan2f", host_atan2f as *const u8);
    jit_builder.symbol("atan2", host_atan2 as *const u8);
    jit_builder.symbol("fmodf", host_fmodf as *const u8);
    jit_builder.symbol("fmod", host_fmod as *const u8);
    jit_builder.symbol("remainderf", host_remainderf as *const u8);
    jit_builder.symbol("remainder", host_remainder as *const u8);
    jit_builder.symbol("isnanf", host_isnanf as *const u8);
    jit_builder.symbol("isnan", host_isnan as *const u8);
    jit_builder.symbol("isinff", host_isinff as *const u8);
    jit_builder.symbol("isinf", host_isinf as *const u8);
    jit_builder.symbol("copysignf", host_copysignf as *const u8);
    jit_builder.symbol("copysign", host_copysign as *const u8);
    jit_builder.symbol("acoshf", host_acoshf as *const u8);
    jit_builder.symbol("acosh", host_acosh as *const u8);
    jit_builder.symbol("asinhf", host_asinhf as *const u8);
    jit_builder.symbol("asinh", host_asinh as *const u8);
    jit_builder.symbol("atanhf", host_atanhf as *const u8);
    jit_builder.symbol("atanh", host_atanh as *const u8);
    jit_builder.symbol("coshf", host_coshf as *const u8);
    jit_builder.symbol("cosh", host_cosh as *const u8);
    jit_builder.symbol("sinhf", host_sinhf as *const u8);
    jit_builder.symbol("sinh", host_sinh as *const u8);
    jit_builder.symbol("tanhf", host_tanhf as *const u8);
    jit_builder.symbol("tanh", host_tanh as *const u8);
}
