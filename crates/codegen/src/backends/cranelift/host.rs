//! Host symbols registered for Cranelift JIT modules.
//!
//! The backend imports math helpers and caller-provided symbols through the JIT
//! symbol registry. These wrappers keep the host ABI explicit and backend-local.

use super::*;
use foreign_call::{NamedFunction, TypedFunction};

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

/// The FIR type of a value of `ty`.
pub(crate) fn fir_type(ty: HostScalarType) -> FirType {
    match ty {
        HostScalarType::Int => FirType::Int32,
        HostScalarType::Float => FirType::Float32,
        HostScalarType::Double => FirType::Float64,
    }
}

/// The CLIF type a value of `ty` is passed or returned as.
pub(crate) fn clif_type(ty: HostScalarType) -> Type {
    match ty {
        HostScalarType::Int => types::I32,
        HostScalarType::Float => types::F32,
        HostScalarType::Double => types::F64,
    }
}

/// Every math function the Cranelift backend implements: the one inventory
/// that the subset pre-check and the lowering (through
/// [`super::math::resolve_math_call`]) and the JIT registration
/// ([`register_host_symbols`]) read, so the three cannot disagree.
///
/// Both `f32` (`*f`) and `f64` variants are listed where the backend supports
/// both result types. The `host_*` helpers above are intentionally tiny
/// wrappers whose semantics match the interpreter/C++ path (notably `rint`
/// and `remainder`).
pub(crate) static HOST_MATH: &[NamedFunction] = &[
    NamedFunction {
        symbol: "sinf",
        function: TypedFunction::Float(host_sinf),
    },
    NamedFunction {
        symbol: "sin",
        function: TypedFunction::Double(host_sin),
    },
    NamedFunction {
        symbol: "cosf",
        function: TypedFunction::Float(host_cosf),
    },
    NamedFunction {
        symbol: "cos",
        function: TypedFunction::Double(host_cos),
    },
    NamedFunction {
        symbol: "expf",
        function: TypedFunction::Float(host_expf),
    },
    NamedFunction {
        symbol: "exp",
        function: TypedFunction::Double(host_exp),
    },
    NamedFunction {
        symbol: "exp10f",
        function: TypedFunction::Float(host_exp10f),
    },
    NamedFunction {
        symbol: "exp10",
        function: TypedFunction::Double(host_exp10),
    },
    NamedFunction {
        symbol: "logf",
        function: TypedFunction::Float(host_logf),
    },
    NamedFunction {
        symbol: "log",
        function: TypedFunction::Double(host_log),
    },
    NamedFunction {
        symbol: "log10f",
        function: TypedFunction::Float(host_log10f),
    },
    NamedFunction {
        symbol: "log10",
        function: TypedFunction::Double(host_log10),
    },
    NamedFunction {
        symbol: "sqrtf",
        function: TypedFunction::Float(host_sqrtf),
    },
    NamedFunction {
        symbol: "sqrt",
        function: TypedFunction::Double(host_sqrt),
    },
    NamedFunction {
        symbol: "fabsf",
        function: TypedFunction::Float(host_fabsf),
    },
    NamedFunction {
        symbol: "fabs",
        function: TypedFunction::Double(host_fabs),
    },
    NamedFunction {
        symbol: "abs",
        function: TypedFunction::Int(host_abs),
    },
    NamedFunction {
        symbol: "floorf",
        function: TypedFunction::Float(host_floorf),
    },
    NamedFunction {
        symbol: "floor",
        function: TypedFunction::Double(host_floor),
    },
    NamedFunction {
        symbol: "ceilf",
        function: TypedFunction::Float(host_ceilf),
    },
    NamedFunction {
        symbol: "ceil",
        function: TypedFunction::Double(host_ceil),
    },
    NamedFunction {
        symbol: "tanf",
        function: TypedFunction::Float(host_tanf),
    },
    NamedFunction {
        symbol: "tan",
        function: TypedFunction::Double(host_tan),
    },
    NamedFunction {
        symbol: "atanf",
        function: TypedFunction::Float(host_atanf),
    },
    NamedFunction {
        symbol: "atan",
        function: TypedFunction::Double(host_atan),
    },
    NamedFunction {
        symbol: "asinf",
        function: TypedFunction::Float(host_asinf),
    },
    NamedFunction {
        symbol: "asin",
        function: TypedFunction::Double(host_asin),
    },
    NamedFunction {
        symbol: "acosf",
        function: TypedFunction::Float(host_acosf),
    },
    NamedFunction {
        symbol: "acos",
        function: TypedFunction::Double(host_acos),
    },
    NamedFunction {
        symbol: "rintf",
        function: TypedFunction::Float(host_rintf),
    },
    NamedFunction {
        symbol: "rint",
        function: TypedFunction::Double(host_rint),
    },
    NamedFunction {
        symbol: "roundf",
        function: TypedFunction::Float(host_roundf),
    },
    NamedFunction {
        symbol: "round",
        function: TypedFunction::Double(host_round),
    },
    NamedFunction {
        symbol: "min_i",
        function: TypedFunction::Int2(host_min_i),
    },
    NamedFunction {
        symbol: "max_i",
        function: TypedFunction::Int2(host_max_i),
    },
    NamedFunction {
        symbol: "fminf",
        function: TypedFunction::Float2(host_fminf),
    },
    NamedFunction {
        symbol: "fmin",
        function: TypedFunction::Double2(host_fmin),
    },
    NamedFunction {
        symbol: "fmaxf",
        function: TypedFunction::Float2(host_fmaxf),
    },
    NamedFunction {
        symbol: "fmax",
        function: TypedFunction::Double2(host_fmax),
    },
    NamedFunction {
        symbol: "powf",
        function: TypedFunction::Float2(host_powf),
    },
    NamedFunction {
        symbol: "pow",
        function: TypedFunction::Double2(host_pow),
    },
    NamedFunction {
        symbol: "atan2f",
        function: TypedFunction::Float2(host_atan2f),
    },
    NamedFunction {
        symbol: "atan2",
        function: TypedFunction::Double2(host_atan2),
    },
    NamedFunction {
        symbol: "fmodf",
        function: TypedFunction::Float2(host_fmodf),
    },
    NamedFunction {
        symbol: "fmod",
        function: TypedFunction::Double2(host_fmod),
    },
    NamedFunction {
        symbol: "remainderf",
        function: TypedFunction::Float2(host_remainderf),
    },
    NamedFunction {
        symbol: "remainder",
        function: TypedFunction::Double2(host_remainder),
    },
    NamedFunction {
        symbol: "isnanf",
        function: TypedFunction::FloatToInt(host_isnanf),
    },
    NamedFunction {
        symbol: "isnan",
        function: TypedFunction::DoubleToInt(host_isnan),
    },
    NamedFunction {
        symbol: "isinff",
        function: TypedFunction::FloatToInt(host_isinff),
    },
    NamedFunction {
        symbol: "isinf",
        function: TypedFunction::DoubleToInt(host_isinf),
    },
    NamedFunction {
        symbol: "copysignf",
        function: TypedFunction::Float2(host_copysignf),
    },
    NamedFunction {
        symbol: "copysign",
        function: TypedFunction::Double2(host_copysign),
    },
    NamedFunction {
        symbol: "acoshf",
        function: TypedFunction::Float(host_acoshf),
    },
    NamedFunction {
        symbol: "acosh",
        function: TypedFunction::Double(host_acosh),
    },
    NamedFunction {
        symbol: "asinhf",
        function: TypedFunction::Float(host_asinhf),
    },
    NamedFunction {
        symbol: "asinh",
        function: TypedFunction::Double(host_asinh),
    },
    NamedFunction {
        symbol: "atanhf",
        function: TypedFunction::Float(host_atanhf),
    },
    NamedFunction {
        symbol: "atanh",
        function: TypedFunction::Double(host_atanh),
    },
    NamedFunction {
        symbol: "coshf",
        function: TypedFunction::Float(host_coshf),
    },
    NamedFunction {
        symbol: "cosh",
        function: TypedFunction::Double(host_cosh),
    },
    NamedFunction {
        symbol: "sinhf",
        function: TypedFunction::Float(host_sinhf),
    },
    NamedFunction {
        symbol: "sinh",
        function: TypedFunction::Double(host_sinh),
    },
    NamedFunction {
        symbol: "tanhf",
        function: TypedFunction::Float(host_tanhf),
    },
    NamedFunction {
        symbol: "tanh",
        function: TypedFunction::Double(host_tanh),
    },
];

/// The math function the backend implements under the C symbol `symbol`.
pub(crate) fn host_math(symbol: &str) -> Option<&'static NamedFunction> {
    HOST_MATH.iter().find(|math| math.symbol == symbol)
}

/// Registers the functions of [`HOST_MATH`] and of the C math library
/// ([`foreign_call::libm_functions`]) as JIT-importable symbols, so the JIT
/// resolves the calls the lowering emits during finalization.
pub(crate) fn register_host_symbols(jit_builder: &mut JITBuilder) {
    for math in HOST_MATH.iter().chain(foreign_call::libm_functions()) {
        jit_builder.symbol(math.symbol, math.function.address() as *const u8);
    }
}
