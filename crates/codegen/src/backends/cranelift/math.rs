//! Which math function a FIR call names, and whether its shape is the one the
//! backend implements.
//!
//! The subset pre-check and the lowering both decide through
//! [`resolve_math_call`], over the one inventory [`HOST_MATH`], so a call the
//! pre-check accepts is one the lowering can emit.
//!
//! A call reaches here under two kinds of names:
//! - FIR's own math names (`sin`, `pow`, `fabs`, ...: [`fir::FirMathOp`]),
//!   which are generic over precision: the result type picks the `f32` or
//!   `f64` variant;
//! - the C symbol of an `ffunction` declaration (`tanhf`, `isnan`,
//!   `copysignf`, ...), which names one variant.
//!
//! As in the C and C++ code the other backends generate, where the
//! declaration of `<math.h>` decides, the arguments convert to the parameter
//! types and the result to the declared one, the precision included:
//! `float tanhf(float)` compiled with `-double` calls `tanhf` on the argument
//! converted to `float`. A call is rejected when its argument count differs
//! (the generated C does not compile either) or when it declares an `int`
//! result for a real one or the reverse (the generated C++ keeps the
//! implementation's type there, against the Faust typing of the call).

use super::host::{HostMath, host_math};
use super::{ExternFunction, HostScalarType};
use crate::backends::host_function::{faust_declaration, signature_mismatch};
use fir::{FirMathOp, FirType};
use std::collections::HashMap;

/// Maps a FIR unary math op to its `f32` host symbol.
fn unary_math_symbol_f32(math: FirMathOp) -> Option<&'static str> {
    Some(match math {
        FirMathOp::Sin => "sinf",
        FirMathOp::Cos => "cosf",
        FirMathOp::Acos => "acosf",
        FirMathOp::Asin => "asinf",
        FirMathOp::Atan => "atanf",
        FirMathOp::Tan => "tanf",
        FirMathOp::Exp => "expf",
        FirMathOp::Exp10 => "exp10f",
        FirMathOp::Log => "logf",
        FirMathOp::Log10 => "log10f",
        FirMathOp::Sqrt => "sqrtf",
        FirMathOp::Abs => "fabsf",
        FirMathOp::Floor => "floorf",
        FirMathOp::Ceil => "ceilf",
        FirMathOp::Rint => "rintf",
        FirMathOp::Round => "roundf",
        _ => return None,
    })
}

/// Maps a FIR unary math op to its `f64` host symbol.
fn unary_math_symbol_f64(math: FirMathOp) -> Option<&'static str> {
    Some(match math {
        FirMathOp::Sin => "sin",
        FirMathOp::Cos => "cos",
        FirMathOp::Acos => "acos",
        FirMathOp::Asin => "asin",
        FirMathOp::Atan => "atan",
        FirMathOp::Tan => "tan",
        FirMathOp::Exp => "exp",
        FirMathOp::Exp10 => "exp10",
        FirMathOp::Log => "log",
        FirMathOp::Log10 => "log10",
        FirMathOp::Sqrt => "sqrt",
        FirMathOp::Abs => "fabs",
        FirMathOp::Floor => "floor",
        FirMathOp::Ceil => "ceil",
        FirMathOp::Rint => "rint",
        FirMathOp::Round => "round",
        _ => return None,
    })
}

/// Maps a FIR binary math op to its `f32` host symbol.
fn binary_math_symbol_f32(math: FirMathOp) -> Option<&'static str> {
    Some(match math {
        FirMathOp::Pow => "powf",
        FirMathOp::Min => "fminf",
        FirMathOp::Max => "fmaxf",
        FirMathOp::Atan2 => "atan2f",
        FirMathOp::Fmod => "fmodf",
        FirMathOp::Remainder => "remainderf",
        _ => return None,
    })
}

/// Maps a FIR binary math op to its `f64` host symbol.
fn binary_math_symbol_f64(math: FirMathOp) -> Option<&'static str> {
    Some(match math {
        FirMathOp::Pow => "pow",
        FirMathOp::Min => "fmin",
        FirMathOp::Max => "fmax",
        FirMathOp::Atan2 => "atan2",
        FirMathOp::Fmod => "fmod",
        FirMathOp::Remainder => "remainder",
        _ => return None,
    })
}

/// The host symbol of a FIR math op at one precision.
fn generic_math_symbol(math: FirMathOp, double: bool) -> Option<&'static str> {
    if double {
        unary_math_symbol_f64(math).or_else(|| binary_math_symbol_f64(math))
    } else {
        unary_math_symbol_f32(math).or_else(|| binary_math_symbol_f32(math))
    }
}

/// A call the JIT emits to a host function of known signature.
pub(crate) struct HostCall<'a> {
    pub(crate) symbol: &'a str,
    pub(crate) ret: HostScalarType,
    pub(crate) params: &'a [HostScalarType],
}

/// Resolves the FIR call `name(args) -> ret`, with `arity` arguments, to the
/// host function the JIT calls: a math function the backend implements, or
/// else one the host registered (`externs`).
///
/// `Ok(Some(call))`: a function of known signature, to which the arguments
/// and the result convert. `Ok(None)`: a host function registered without a
/// signature, called with the types of the declaration. `Err(reason)`: no
/// such function, or one of another shape. `double` resolves a `FaustFloat`
/// result; it only picks between two math variants of the same shape, so a
/// caller without the precision may pass either value to check a call.
pub(crate) fn resolve_call<'a>(
    name: &'a str,
    ret: &FirType,
    arity: usize,
    double: bool,
    externs: &'a HashMap<String, ExternFunction>,
) -> Result<Option<HostCall<'a>>, String> {
    if let Some(math) = resolve_math_call(name, ret, arity, double) {
        let math = math?;
        return Ok(Some(HostCall {
            symbol: math.symbol,
            ret: math.function.ret(),
            params: math.function.params(),
        }));
    }
    match externs.get(name) {
        Some(ExternFunction {
            signature: Some(signature),
            ..
        }) => {
            let call_ret = call_result(name, ret, double)?;
            if signature.params.len() == arity && signature.ret.is_real() == call_ret.is_real() {
                Ok(Some(HostCall {
                    symbol: name,
                    ret: signature.ret,
                    params: &signature.params,
                }))
            } else {
                Err(signature_mismatch(
                    name,
                    &format!(
                        "the host function registered under that name, `{}`",
                        signature.prototype(name)
                    ),
                    signature.ret,
                    &signature.params,
                    call_ret,
                    arity,
                ))
            }
        }
        Some(_) => Ok(None),
        // Not a math function the backend implements: a foreign function,
        // which the JIT can call only at an address the host registered. The
        // header an `ffunction` names is for the C and C++ backends; the JIT
        // never compiles it.
        None => Err(format!(
            "`{name}` has no Cranelift implementation: it is not a supported math function \
             and no host function is registered under that name (the JIT does not compile \
             the header an `ffunction` names, `-I` included); compile with `-lang cpp` or \
             `-lang c`, or, when embedding, register it with \
             `registerCCraneliftForeignFunction`"
        )),
    }
}

/// The C type of a call's result: `FaustFloat` is `double` under `-double`.
fn call_result(name: &str, ret: &FirType, double: bool) -> Result<HostScalarType, String> {
    match ret {
        FirType::Int32 => Ok(HostScalarType::Int),
        FirType::Float32 => Ok(HostScalarType::Float),
        FirType::Float64 => Ok(HostScalarType::Double),
        FirType::FaustFloat if double => Ok(HostScalarType::Double),
        FirType::FaustFloat => Ok(HostScalarType::Float),
        _ => Err(format!("`{name}` is called with a {ret:?} result")),
    }
}

/// Resolves the FIR call `name(args) -> ret`, with `arity` arguments, to the
/// host math function the JIT calls.
///
/// `None`: `name` is not a math function the backend implements (a foreign
/// function, bound or not by the host). `Some(Err(reason))`: it is one, but
/// the call does not have its shape.
pub(crate) fn resolve_math_call(
    name: &str,
    ret: &FirType,
    arity: usize,
    double: bool,
) -> Option<Result<&'static HostMath, String>> {
    let math = match FirMathOp::from_symbol(name) {
        Some(op) => {
            let wide = match ret {
                FirType::Float64 => true,
                FirType::FaustFloat => double,
                _ => false,
            };
            let symbol = generic_math_symbol(op, wide)?;
            host_math(symbol).expect("every FIR math op variant is in HOST_MATH")
        }
        None => host_math(name)?,
    };
    let call_ret = match call_result(name, ret, double) {
        Ok(call_ret) => call_ret,
        Err(reason) => return Some(Err(reason)),
    };
    let function = math.function;
    if function.params().len() == arity && function.ret().is_real() == call_ret.is_real() {
        Some(Ok(math))
    } else {
        let faust = faust_declaration(name, function.ret(), function.params());
        Some(Err(signature_mismatch(
            name,
            &format!("the math function the Cranelift backend implements, `{faust}`"),
            function.ret(),
            function.params(),
            call_ret,
            arity,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::cranelift::host::HOST_MATH;

    const ALL_MATH_OPS: &[FirMathOp] = &[
        FirMathOp::Pow,
        FirMathOp::Min,
        FirMathOp::Max,
        FirMathOp::Sin,
        FirMathOp::Cos,
        FirMathOp::Acos,
        FirMathOp::Asin,
        FirMathOp::Atan,
        FirMathOp::Atan2,
        FirMathOp::Tan,
        FirMathOp::Exp,
        FirMathOp::Exp10,
        FirMathOp::Log,
        FirMathOp::Log10,
        FirMathOp::Sqrt,
        FirMathOp::Abs,
        FirMathOp::Fmod,
        FirMathOp::Remainder,
        FirMathOp::Floor,
        FirMathOp::Ceil,
        FirMathOp::Rint,
        FirMathOp::Round,
    ];

    enum Kind {
        Unary,
        Binary,
    }

    /// Expected `(kind, f32 symbol, f64 symbol)` for every `FirMathOp`.
    ///
    /// This match has no `_` arm on purpose: adding a new `FirMathOp`
    /// variant breaks this function at compile time, forcing the new op to
    /// be classified here instead of silently falling through the
    /// `_ => None` arms in the `*_math_symbol_*` tables.
    fn expected(op: FirMathOp) -> (Kind, &'static str, &'static str) {
        match op {
            FirMathOp::Sin => (Kind::Unary, "sinf", "sin"),
            FirMathOp::Cos => (Kind::Unary, "cosf", "cos"),
            FirMathOp::Acos => (Kind::Unary, "acosf", "acos"),
            FirMathOp::Asin => (Kind::Unary, "asinf", "asin"),
            FirMathOp::Atan => (Kind::Unary, "atanf", "atan"),
            FirMathOp::Tan => (Kind::Unary, "tanf", "tan"),
            FirMathOp::Exp => (Kind::Unary, "expf", "exp"),
            FirMathOp::Exp10 => (Kind::Unary, "exp10f", "exp10"),
            FirMathOp::Log => (Kind::Unary, "logf", "log"),
            FirMathOp::Log10 => (Kind::Unary, "log10f", "log10"),
            FirMathOp::Sqrt => (Kind::Unary, "sqrtf", "sqrt"),
            FirMathOp::Abs => (Kind::Unary, "fabsf", "fabs"),
            FirMathOp::Floor => (Kind::Unary, "floorf", "floor"),
            FirMathOp::Ceil => (Kind::Unary, "ceilf", "ceil"),
            FirMathOp::Rint => (Kind::Unary, "rintf", "rint"),
            FirMathOp::Round => (Kind::Unary, "roundf", "round"),
            FirMathOp::Pow => (Kind::Binary, "powf", "pow"),
            FirMathOp::Min => (Kind::Binary, "fminf", "fmin"),
            FirMathOp::Max => (Kind::Binary, "fmaxf", "fmax"),
            FirMathOp::Atan2 => (Kind::Binary, "atan2f", "atan2"),
            FirMathOp::Fmod => (Kind::Binary, "fmodf", "fmod"),
            FirMathOp::Remainder => (Kind::Binary, "remainderf", "remainder"),
        }
    }

    #[test]
    fn math_symbol_tables_match_every_fir_math_op() {
        for op in ALL_MATH_OPS {
            let (kind, f32_sym, f64_sym) = expected(*op);
            match kind {
                Kind::Unary => {
                    assert_eq!(
                        unary_math_symbol_f32(*op),
                        Some(f32_sym),
                        "{op:?}: wrong/missing f32 unary symbol"
                    );
                    assert_eq!(
                        unary_math_symbol_f64(*op),
                        Some(f64_sym),
                        "{op:?}: wrong/missing f64 unary symbol"
                    );
                    assert_eq!(
                        binary_math_symbol_f32(*op),
                        None,
                        "{op:?}: unary op unexpectedly has an f32 binary symbol"
                    );
                    assert_eq!(
                        binary_math_symbol_f64(*op),
                        None,
                        "{op:?}: unary op unexpectedly has an f64 binary symbol"
                    );
                }
                Kind::Binary => {
                    assert_eq!(
                        binary_math_symbol_f32(*op),
                        Some(f32_sym),
                        "{op:?}: wrong/missing f32 binary symbol"
                    );
                    assert_eq!(
                        binary_math_symbol_f64(*op),
                        Some(f64_sym),
                        "{op:?}: wrong/missing f64 binary symbol"
                    );
                    assert_eq!(
                        unary_math_symbol_f32(*op),
                        None,
                        "{op:?}: binary op unexpectedly has an f32 unary symbol"
                    );
                    assert_eq!(
                        unary_math_symbol_f64(*op),
                        None,
                        "{op:?}: binary op unexpectedly has an f64 unary symbol"
                    );
                }
            }
        }
    }

    /// Each FIR math op has an `f32` and an `f64` variant of the same shape,
    /// each of its own precision: the pre-check may check a call without the
    /// precision.
    #[test]
    fn each_fir_math_op_has_two_variants_of_one_shape() {
        for &op in ALL_MATH_OPS {
            let f = host_math(generic_math_symbol(op, false).unwrap()).unwrap();
            let d = host_math(generic_math_symbol(op, true).unwrap()).unwrap();
            assert_eq!(
                f.function.params().len(),
                d.function.params().len(),
                "{op:?}"
            );
            assert_eq!(f.function.ret(), HostScalarType::Float, "{op:?}");
            assert_eq!(d.function.ret(), HostScalarType::Double, "{op:?}");
            assert!(
                f.function
                    .params()
                    .iter()
                    .all(|t| *t == HostScalarType::Float)
            );
            assert!(
                d.function
                    .params()
                    .iter()
                    .all(|t| *t == HostScalarType::Double)
            );
        }
    }

    #[test]
    fn host_math_symbols_are_unique() {
        let mut symbols: Vec<&str> = HOST_MATH.iter().map(|m| m.symbol).collect();
        symbols.sort_unstable();
        let len = symbols.len();
        symbols.dedup();
        assert_eq!(symbols.len(), len);
    }

    #[test]
    fn a_call_resolves_by_name_arity_and_result_kind() {
        let ok = |name, ret: FirType, arity, double| {
            resolve_math_call(name, &ret, arity, double)
                .unwrap()
                .unwrap()
                .symbol
        };
        // FIR's names: the result picks the variant.
        assert_eq!(ok("sin", FirType::Float32, 1, true), "sinf");
        assert_eq!(ok("sin", FirType::FaustFloat, 1, true), "sin");
        assert_eq!(ok("std::pow", FirType::Float64, 2, false), "pow");
        // C symbols name one variant; the precision converts.
        assert_eq!(ok("tanhf", FirType::Float64, 1, true), "tanhf");
        assert_eq!(ok("sinf", FirType::FaustFloat, 1, false), "sinf");
        assert_eq!(ok("isnan", FirType::Int32, 1, false), "isnan");
        assert_eq!(ok("abs", FirType::Int32, 1, false), "abs");
        // Not a math function.
        assert!(resolve_math_call("w4_three_f", &FirType::Float32, 3, false).is_none());
        assert!(resolve_math_call("cbrtf", &FirType::Float32, 1, false).is_none());
        // A math function with another shape.
        for (name, ret, arity) in [
            ("tanhf", FirType::Int32, 1),
            ("tanhf", FirType::Float32, 2),
            ("isnanf", FirType::Float32, 1),
            ("sin", FirType::Int32, 1),
            ("pow", FirType::Float64, 1),
        ] {
            let reason = resolve_math_call(name, &ret, arity, false)
                .unwrap()
                .err()
                .expect("a signature mismatch");
            assert!(
                reason.contains(&format!("`{name}` does not have")),
                "{reason}"
            );
        }
        let reason = resolve_math_call("tanhf", &FirType::Int32, 2, false)
            .unwrap()
            .err()
            .expect("a signature mismatch");
        assert!(
            reason.contains(
                "implements, `float tanhf(float)`: the call has 2 arguments and an `int` result"
            ),
            "{reason}"
        );
    }
}
