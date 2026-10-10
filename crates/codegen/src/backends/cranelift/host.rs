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

/// A host math function, typed by its pointer: the signature the subset
/// pre-check and the lowering read is the one of the function the JIT calls.
#[derive(Clone, Copy)]
pub(crate) enum HostMathFn {
    Float(extern "C" fn(f32) -> f32),
    Double(extern "C" fn(f64) -> f64),
    Float2(extern "C" fn(f32, f32) -> f32),
    Double2(extern "C" fn(f64, f64) -> f64),
    Int(extern "C" fn(i32) -> i32),
    Int2(extern "C" fn(i32, i32) -> i32),
    FloatToInt(extern "C" fn(f32) -> i32),
    DoubleToInt(extern "C" fn(f64) -> i32),
}

impl HostMathFn {
    /// The result type.
    pub(crate) fn ret(self) -> HostScalarType {
        match self {
            Self::Float(_) | Self::Float2(_) => HostScalarType::Float,
            Self::Double(_) | Self::Double2(_) => HostScalarType::Double,
            Self::Int(_) | Self::Int2(_) | Self::FloatToInt(_) | Self::DoubleToInt(_) => {
                HostScalarType::Int
            }
        }
    }

    /// The parameter types, in order.
    pub(crate) fn params(self) -> &'static [HostScalarType] {
        match self {
            Self::Float(_) | Self::FloatToInt(_) => &[HostScalarType::Float],
            Self::Double(_) | Self::DoubleToInt(_) => &[HostScalarType::Double],
            Self::Float2(_) => &[HostScalarType::Float, HostScalarType::Float],
            Self::Double2(_) => &[HostScalarType::Double, HostScalarType::Double],
            Self::Int(_) => &[HostScalarType::Int],
            Self::Int2(_) => &[HostScalarType::Int, HostScalarType::Int],
        }
    }

    /// The address the JIT binds the symbol to.
    fn address(self) -> *const u8 {
        match self {
            Self::Float(f) => f as *const u8,
            Self::Double(f) => f as *const u8,
            Self::Float2(f) => f as *const u8,
            Self::Double2(f) => f as *const u8,
            Self::Int(f) => f as *const u8,
            Self::Int2(f) => f as *const u8,
            Self::FloatToInt(f) => f as *const u8,
            Self::DoubleToInt(f) => f as *const u8,
        }
    }
}

/// One math function the backend implements, under its C symbol.
pub(crate) struct HostMath {
    pub(crate) symbol: &'static str,
    pub(crate) function: HostMathFn,
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
pub(crate) static HOST_MATH: &[HostMath] = &[
    HostMath {
        symbol: "sinf",
        function: HostMathFn::Float(host_sinf),
    },
    HostMath {
        symbol: "sin",
        function: HostMathFn::Double(host_sin),
    },
    HostMath {
        symbol: "cosf",
        function: HostMathFn::Float(host_cosf),
    },
    HostMath {
        symbol: "cos",
        function: HostMathFn::Double(host_cos),
    },
    HostMath {
        symbol: "expf",
        function: HostMathFn::Float(host_expf),
    },
    HostMath {
        symbol: "exp",
        function: HostMathFn::Double(host_exp),
    },
    HostMath {
        symbol: "exp10f",
        function: HostMathFn::Float(host_exp10f),
    },
    HostMath {
        symbol: "exp10",
        function: HostMathFn::Double(host_exp10),
    },
    HostMath {
        symbol: "logf",
        function: HostMathFn::Float(host_logf),
    },
    HostMath {
        symbol: "log",
        function: HostMathFn::Double(host_log),
    },
    HostMath {
        symbol: "log10f",
        function: HostMathFn::Float(host_log10f),
    },
    HostMath {
        symbol: "log10",
        function: HostMathFn::Double(host_log10),
    },
    HostMath {
        symbol: "sqrtf",
        function: HostMathFn::Float(host_sqrtf),
    },
    HostMath {
        symbol: "sqrt",
        function: HostMathFn::Double(host_sqrt),
    },
    HostMath {
        symbol: "fabsf",
        function: HostMathFn::Float(host_fabsf),
    },
    HostMath {
        symbol: "fabs",
        function: HostMathFn::Double(host_fabs),
    },
    HostMath {
        symbol: "abs",
        function: HostMathFn::Int(host_abs),
    },
    HostMath {
        symbol: "floorf",
        function: HostMathFn::Float(host_floorf),
    },
    HostMath {
        symbol: "floor",
        function: HostMathFn::Double(host_floor),
    },
    HostMath {
        symbol: "ceilf",
        function: HostMathFn::Float(host_ceilf),
    },
    HostMath {
        symbol: "ceil",
        function: HostMathFn::Double(host_ceil),
    },
    HostMath {
        symbol: "tanf",
        function: HostMathFn::Float(host_tanf),
    },
    HostMath {
        symbol: "tan",
        function: HostMathFn::Double(host_tan),
    },
    HostMath {
        symbol: "atanf",
        function: HostMathFn::Float(host_atanf),
    },
    HostMath {
        symbol: "atan",
        function: HostMathFn::Double(host_atan),
    },
    HostMath {
        symbol: "asinf",
        function: HostMathFn::Float(host_asinf),
    },
    HostMath {
        symbol: "asin",
        function: HostMathFn::Double(host_asin),
    },
    HostMath {
        symbol: "acosf",
        function: HostMathFn::Float(host_acosf),
    },
    HostMath {
        symbol: "acos",
        function: HostMathFn::Double(host_acos),
    },
    HostMath {
        symbol: "rintf",
        function: HostMathFn::Float(host_rintf),
    },
    HostMath {
        symbol: "rint",
        function: HostMathFn::Double(host_rint),
    },
    HostMath {
        symbol: "roundf",
        function: HostMathFn::Float(host_roundf),
    },
    HostMath {
        symbol: "round",
        function: HostMathFn::Double(host_round),
    },
    HostMath {
        symbol: "min_i",
        function: HostMathFn::Int2(host_min_i),
    },
    HostMath {
        symbol: "max_i",
        function: HostMathFn::Int2(host_max_i),
    },
    HostMath {
        symbol: "fminf",
        function: HostMathFn::Float2(host_fminf),
    },
    HostMath {
        symbol: "fmin",
        function: HostMathFn::Double2(host_fmin),
    },
    HostMath {
        symbol: "fmaxf",
        function: HostMathFn::Float2(host_fmaxf),
    },
    HostMath {
        symbol: "fmax",
        function: HostMathFn::Double2(host_fmax),
    },
    HostMath {
        symbol: "powf",
        function: HostMathFn::Float2(host_powf),
    },
    HostMath {
        symbol: "pow",
        function: HostMathFn::Double2(host_pow),
    },
    HostMath {
        symbol: "atan2f",
        function: HostMathFn::Float2(host_atan2f),
    },
    HostMath {
        symbol: "atan2",
        function: HostMathFn::Double2(host_atan2),
    },
    HostMath {
        symbol: "fmodf",
        function: HostMathFn::Float2(host_fmodf),
    },
    HostMath {
        symbol: "fmod",
        function: HostMathFn::Double2(host_fmod),
    },
    HostMath {
        symbol: "remainderf",
        function: HostMathFn::Float2(host_remainderf),
    },
    HostMath {
        symbol: "remainder",
        function: HostMathFn::Double2(host_remainder),
    },
    HostMath {
        symbol: "isnanf",
        function: HostMathFn::FloatToInt(host_isnanf),
    },
    HostMath {
        symbol: "isnan",
        function: HostMathFn::DoubleToInt(host_isnan),
    },
    HostMath {
        symbol: "isinff",
        function: HostMathFn::FloatToInt(host_isinff),
    },
    HostMath {
        symbol: "isinf",
        function: HostMathFn::DoubleToInt(host_isinf),
    },
    HostMath {
        symbol: "copysignf",
        function: HostMathFn::Float2(host_copysignf),
    },
    HostMath {
        symbol: "copysign",
        function: HostMathFn::Double2(host_copysign),
    },
    HostMath {
        symbol: "acoshf",
        function: HostMathFn::Float(host_acoshf),
    },
    HostMath {
        symbol: "acosh",
        function: HostMathFn::Double(host_acosh),
    },
    HostMath {
        symbol: "asinhf",
        function: HostMathFn::Float(host_asinhf),
    },
    HostMath {
        symbol: "asinh",
        function: HostMathFn::Double(host_asinh),
    },
    HostMath {
        symbol: "atanhf",
        function: HostMathFn::Float(host_atanhf),
    },
    HostMath {
        symbol: "atanh",
        function: HostMathFn::Double(host_atanh),
    },
    HostMath {
        symbol: "coshf",
        function: HostMathFn::Float(host_coshf),
    },
    HostMath {
        symbol: "cosh",
        function: HostMathFn::Double(host_cosh),
    },
    HostMath {
        symbol: "sinhf",
        function: HostMathFn::Float(host_sinhf),
    },
    HostMath {
        symbol: "sinh",
        function: HostMathFn::Double(host_sinh),
    },
    HostMath {
        symbol: "tanhf",
        function: HostMathFn::Float(host_tanhf),
    },
    HostMath {
        symbol: "tanh",
        function: HostMathFn::Double(host_tanh),
    },
];

/// The math function the backend implements under the C symbol `symbol`.
pub(crate) fn host_math(symbol: &str) -> Option<&'static HostMath> {
    HOST_MATH.iter().find(|math| math.symbol == symbol)
}

/// Registers the functions of [`HOST_MATH`] as JIT-importable symbols, so the
/// JIT resolves the calls the lowering emits during finalization.
pub(crate) fn register_host_symbols(jit_builder: &mut JITBuilder) {
    for math in HOST_MATH {
        jit_builder.symbol(math.symbol, math.function.address());
    }
}
