//! Runtime bridge from symbolic Faust foreign bindings to host C functions.
//!
//! This crate supports only the scalar signatures represented by [`ScalarType`]
//! and [`Value`]. A caller must supply an address with the matching `extern "C"`
//! ABI; a mismatch is unsupported and can violate the host ABI contract.
//!
//! Rust has no call whose signature is built at run time (what libffi does in
//! C): each signature is a function pointer type known at compile time. The
//! supported ones are therefore enumerated, by the macros below rather than
//! by hand: up to [`MAX_ARGS`] arguments, each `i32`, `f32` or `f64`, in any
//! combination, returning one of these or nothing; and, from the first
//! version, the `bool` functions of zero to two `bool` arguments. See
//! [`is_supported`].

#![allow(unsafe_code)] // Explicit runtime bridge from symbolic foreign bindings to raw host pointers.

/// Scalar types supported by the foreign-call ABI dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarType {
    /// A signed 32-bit integer.
    Int32,
    /// An IEEE-754 single-precision value.
    Float32,
    /// An IEEE-754 double-precision value.
    Float64,
    /// A C-compatible boolean value.
    Bool,
    /// No return value.
    Void,
}

/// A scalar argument or return value accepted by [`invoke`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// A signed 32-bit integer value.
    Int32(i32),
    /// A single-precision floating-point value.
    Float32(f32),
    /// A double-precision floating-point value.
    Float64(f64),
    /// A boolean value.
    Bool(bool),
    /// The value returned by a `void` foreign function.
    Void,
}

/// The largest argument count [`invoke`] can call with.
pub const MAX_ARGS: usize = 4;

/// Whether [`invoke`] can call a function of this signature.
///
/// The arguments are at most [`MAX_ARGS`], each `Int32`, `Float32` or
/// `Float64`, and the result one of these or `Void`; or the signature is one
/// of the `bool` ones: `bool()`, `bool(bool)`, `bool(bool, bool)`,
/// `void(bool, bool)`.
#[must_use]
pub fn is_supported(ret: ScalarType, args: &[ScalarType]) -> bool {
    let numeric = |ty: &ScalarType| {
        matches!(
            ty,
            ScalarType::Int32 | ScalarType::Float32 | ScalarType::Float64
        )
    };
    let general = args.len() <= MAX_ARGS
        && args.iter().all(numeric)
        && (numeric(&ret) || ret == ScalarType::Void);
    general || bool_signature(ret, args)
}

/// The `bool` signatures of the first version of this crate.
fn bool_signature(ret: ScalarType, args: &[ScalarType]) -> bool {
    use ScalarType::{Bool, Void};
    matches!(
        (ret, args),
        (Bool, [] | [Bool] | [Bool, Bool]) | (Void, [Bool, Bool])
    )
}

/// Calls `$addr` as an `extern "C" fn($($ty),*) -> ret` on `$($arg),*`,
/// the result type chosen by `$ret`.
macro_rules! call_with_ret {
    ($addr:ident, $ret:ident, [$($ty:ty),*], [$($arg:ident),*]) => {
        match $ret {
            ScalarType::Int32 => {
                // SAFETY: the caller guarantees `addr` has this signature.
                let f: extern "C" fn($($ty),*) -> i32 = unsafe { std::mem::transmute($addr) };
                Some(Value::Int32(f($($arg),*)))
            }
            ScalarType::Float32 => {
                // SAFETY: as above.
                let f: extern "C" fn($($ty),*) -> f32 = unsafe { std::mem::transmute($addr) };
                Some(Value::Float32(f($($arg),*)))
            }
            ScalarType::Float64 => {
                // SAFETY: as above.
                let f: extern "C" fn($($ty),*) -> f64 = unsafe { std::mem::transmute($addr) };
                Some(Value::Float64(f($($arg),*)))
            }
            ScalarType::Void => {
                // SAFETY: as above.
                let f: extern "C" fn($($ty),*) = unsafe { std::mem::transmute($addr) };
                f($($arg),*);
                Some(Value::Void)
            }
            ScalarType::Bool => None,
        }
    };
}

/// Matches the type of each argument in `[$next, $rest...]` in turn,
/// accumulating the parameter types `$ty` and the values `$done`, then calls.
/// Each argument's value is bound under the name of its slot.
macro_rules! call_with_args {
    ($addr:ident, $ret:ident, [], [$($ty:ty),*], [$($done:ident),*]) => {
        call_with_ret!($addr, $ret, [$($ty),*], [$($done),*])
    };
    ($addr:ident, $ret:ident, [$next:ident $(, $rest:ident)*], [$($ty:ty),*], [$($done:ident),*]) => {
        match *$next {
            Value::Int32($next) => {
                call_with_args!($addr, $ret, [$($rest),*], [$($ty,)* i32], [$($done,)* $next])
            }
            Value::Float32($next) => {
                call_with_args!($addr, $ret, [$($rest),*], [$($ty,)* f32], [$($done,)* $next])
            }
            Value::Float64($next) => {
                call_with_args!($addr, $ret, [$($rest),*], [$($ty,)* f64], [$($done,)* $next])
            }
            Value::Bool(_) | Value::Void => None,
        }
    };
}

/// Invokes a supported scalar `extern "C"` function at `addr`.
///
/// Returns `None`, without calling, when `ret` and `args` are not a
/// signature [`is_supported`] accepts. `addr` must be a valid function
/// address whose ABI, argument types, and return type match `ret` and `args`.
#[must_use]
pub fn invoke(addr: usize, ret: ScalarType, args: &[Value]) -> Option<Value> {
    if let Some(value) = invoke_bool(addr, ret, args) {
        return Some(value);
    }
    match args {
        [] => call_with_args!(addr, ret, [], [], []),
        [a0] => call_with_args!(addr, ret, [a0], [], []),
        [a0, a1] => call_with_args!(addr, ret, [a0, a1], [], []),
        [a0, a1, a2] => call_with_args!(addr, ret, [a0, a1, a2], [], []),
        [a0, a1, a2, a3] => call_with_args!(addr, ret, [a0, a1, a2, a3], [], []),
        _ => None,
    }
}

/// The `bool` signatures, kept from the first version of this crate.
fn invoke_bool(addr: usize, ret: ScalarType, args: &[Value]) -> Option<Value> {
    // SAFETY (each transmute): the caller guarantees `addr` has the signature.
    match (ret, args) {
        (ScalarType::Bool, []) => {
            let f: extern "C" fn() -> bool = unsafe { std::mem::transmute(addr) };
            Some(Value::Bool(f()))
        }
        (ScalarType::Bool, [Value::Bool(a0)]) => {
            let f: extern "C" fn(bool) -> bool = unsafe { std::mem::transmute(addr) };
            Some(Value::Bool(f(*a0)))
        }
        (ScalarType::Bool, [Value::Bool(a0), Value::Bool(a1)]) => {
            let f: extern "C" fn(bool, bool) -> bool = unsafe { std::mem::transmute(addr) };
            Some(Value::Bool(f(*a0, *a1)))
        }
        (ScalarType::Void, [Value::Bool(a0), Value::Bool(a1)]) => {
            let f: extern "C" fn(bool, bool) = unsafe { std::mem::transmute(addr) };
            f(*a0, *a1);
            Some(Value::Void)
        }
        _ => None,
    }
}

/// A C function pointer typed by its signature: the signature a caller reads
/// from it is the one of the function called.
#[derive(Clone, Copy, Debug)]
pub enum TypedFunction {
    /// `float(float)`.
    Float(extern "C" fn(f32) -> f32),
    /// `double(double)`.
    Double(extern "C" fn(f64) -> f64),
    /// `float(float, float)`.
    Float2(extern "C" fn(f32, f32) -> f32),
    /// `double(double, double)`.
    Double2(extern "C" fn(f64, f64) -> f64),
    /// `int(int)`.
    Int(extern "C" fn(i32) -> i32),
    /// `int(int, int)`.
    Int2(extern "C" fn(i32, i32) -> i32),
    /// `int(float)`.
    FloatToInt(extern "C" fn(f32) -> i32),
    /// `int(double)`.
    DoubleToInt(extern "C" fn(f64) -> i32),
    /// `float(float, int)`.
    FloatInt(extern "C" fn(f32, i32) -> f32),
    /// `double(double, int)`.
    DoubleInt(extern "C" fn(f64, i32) -> f64),
    /// `double(int, double)`.
    IntDouble(extern "C" fn(i32, f64) -> f64),
}

impl TypedFunction {
    /// The result type.
    #[must_use]
    pub fn ret(self) -> ScalarType {
        match self {
            Self::Float(_) | Self::Float2(_) | Self::FloatInt(_) => ScalarType::Float32,
            Self::Double(_) | Self::Double2(_) | Self::DoubleInt(_) | Self::IntDouble(_) => {
                ScalarType::Float64
            }
            Self::Int(_) | Self::Int2(_) | Self::FloatToInt(_) | Self::DoubleToInt(_) => {
                ScalarType::Int32
            }
        }
    }

    /// The parameter types, in order.
    #[must_use]
    pub fn params(self) -> &'static [ScalarType] {
        use ScalarType::{Float32, Float64, Int32};
        match self {
            Self::Float(_) | Self::FloatToInt(_) => &[Float32],
            Self::Double(_) | Self::DoubleToInt(_) => &[Float64],
            Self::Float2(_) => &[Float32, Float32],
            Self::Double2(_) => &[Float64, Float64],
            Self::Int(_) => &[Int32],
            Self::Int2(_) => &[Int32, Int32],
            Self::FloatInt(_) => &[Float32, Int32],
            Self::DoubleInt(_) => &[Float64, Int32],
            Self::IntDouble(_) => &[Int32, Float64],
        }
    }

    /// The function's address, for a JIT symbol or [`invoke`].
    #[must_use]
    pub fn address(self) -> usize {
        match self {
            Self::Float(f) => f as usize,
            Self::Double(f) => f as usize,
            Self::Float2(f) => f as usize,
            Self::Double2(f) => f as usize,
            Self::Int(f) => f as usize,
            Self::Int2(f) => f as usize,
            Self::FloatToInt(f) => f as usize,
            Self::DoubleToInt(f) => f as usize,
            Self::FloatInt(f) => f as usize,
            Self::DoubleInt(f) => f as usize,
            Self::IntDouble(f) => f as usize,
        }
    }
}

/// A C function under its symbol.
#[derive(Clone, Copy, Debug)]
pub struct NamedFunction {
    /// The C symbol.
    pub symbol: &'static str,
    /// The function.
    pub function: TypedFunction,
}

/// The functions of the system's C math library a Faust `ffunction` may name
/// and no backend implements itself (`cbrt`, `hypot`, `erf`, `tgamma`,
/// `ldexp`, the Bessel functions, ...: the ones `maths.lib` declares), in
/// their `float` and `double` variants. The C++ LLVM JIT finds them among
/// the process's symbols; calling the same library gives the results the C
/// and C++ code generated on the same machine computes. None on wasm32,
/// which has no C math library.
#[must_use]
pub fn libm_functions() -> &'static [NamedFunction] {
    #[cfg(not(target_arch = "wasm32"))]
    {
        libm::FUNCTIONS
    }
    #[cfg(target_arch = "wasm32")]
    {
        &[]
    }
}

/// The function of [`libm_functions`] under the C symbol `symbol`.
#[must_use]
pub fn libm_function(symbol: &str) -> Option<&'static NamedFunction> {
    libm_functions()
        .iter()
        .find(|function| function.symbol == symbol)
}

#[cfg(not(target_arch = "wasm32"))]
mod libm {
    use super::{NamedFunction, TypedFunction};

    // Pure functions of their arguments: no input makes a call undefined.
    #[cfg_attr(all(unix, not(target_vendor = "apple")), link(name = "m"))]
    unsafe extern "C" {
        safe fn cbrtf(x: f32) -> f32;
        safe fn cbrt(x: f64) -> f64;
        // MSVC defines `hypotf` inline, over `_hypotf`.
        #[cfg_attr(windows, link_name = "_hypotf")]
        safe fn hypotf(x: f32, y: f32) -> f32;
        safe fn hypot(x: f64, y: f64) -> f64;
        safe fn erff(x: f32) -> f32;
        safe fn erf(x: f64) -> f64;
        safe fn erfcf(x: f32) -> f32;
        safe fn erfc(x: f64) -> f64;
        safe fn tgammaf(x: f32) -> f32;
        safe fn tgamma(x: f64) -> f64;
        safe fn lgammaf(x: f32) -> f32;
        safe fn lgamma(x: f64) -> f64;
        safe fn logbf(x: f32) -> f32;
        safe fn logb(x: f64) -> f64;
        safe fn ilogbf(x: f32) -> i32;
        safe fn ilogb(x: f64) -> i32;
        safe fn ldexp(x: f64, e: i32) -> f64;
        safe fn scalbn(x: f64, e: i32) -> f64;
        safe fn nextafterf(x: f32, y: f32) -> f32;
        safe fn nextafter(x: f64, y: f64) -> f64;
        // POSIX, `double` only; MSVC names them with an underscore.
        #[cfg_attr(windows, link_name = "_j0")]
        safe fn j0(x: f64) -> f64;
        #[cfg_attr(windows, link_name = "_j1")]
        safe fn j1(x: f64) -> f64;
        #[cfg_attr(windows, link_name = "_jn")]
        safe fn jn(n: i32, x: f64) -> f64;
        #[cfg_attr(windows, link_name = "_y0")]
        safe fn y0(x: f64) -> f64;
        #[cfg_attr(windows, link_name = "_y1")]
        safe fn y1(x: f64) -> f64;
        #[cfg_attr(windows, link_name = "_yn")]
        safe fn yn(n: i32, x: f64) -> f64;
    }

    // `ldexpf` and `scalbnf` are inline in MSVC's header, not exported. Over
    // `double` they are exact: `x * 2^e` of a `float` is a `double` without
    // rounding, rounded once to `float`, as `ldexpf` rounds it.
    extern "C" fn ldexpf(x: f32, e: i32) -> f32 {
        ldexp(f64::from(x), e) as f32
    }

    extern "C" fn scalbnf(x: f32, e: i32) -> f32 {
        scalbn(f64::from(x), e) as f32
    }

    macro_rules! functions {
        ($($variant:ident($symbol:ident)),* $(,)?) => {
            &[$(NamedFunction {
                symbol: stringify!($symbol),
                function: TypedFunction::$variant($symbol),
            }),*]
        };
    }

    pub(super) static FUNCTIONS: &[NamedFunction] = functions![
        Float(cbrtf),
        Double(cbrt),
        Float2(hypotf),
        Double2(hypot),
        Float(erff),
        Double(erf),
        Float(erfcf),
        Double(erfc),
        Float(tgammaf),
        Double(tgamma),
        Float(lgammaf),
        Double(lgamma),
        Float(logbf),
        Double(logb),
        FloatToInt(ilogbf),
        DoubleToInt(ilogb),
        FloatInt(ldexpf),
        DoubleInt(ldexp),
        FloatInt(scalbnf),
        DoubleInt(scalbn),
        Float2(nextafterf),
        Double2(nextafter),
        Double(j0),
        Double(j1),
        IntDouble(jn),
        Double(y0),
        Double(y1),
        IntDouble(yn),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    extern "C" fn jn_like(n: i32, x: f32) -> f32 {
        n as f32 * 100.0 + x
    }

    extern "C" fn ldexp_like(x: f64, e: i32) -> f64 {
        x * f64::from(e)
    }

    extern "C" fn ilogb_like(x: f32) -> i32 {
        x as i32 + 1
    }

    extern "C" fn four(a: f64, b: i32, c: f32, d: i32) -> f64 {
        a + 10.0 * f64::from(b) + 100.0 * f64::from(c) + 1000.0 * f64::from(d)
    }

    static SEEN: AtomicI64 = AtomicI64::new(0);

    extern "C" fn sink(a: i32, b: f64, c: i32) {
        SEEN.store(
            i64::from(a) + (b as i64) * 10 + i64::from(c) * 100,
            Ordering::SeqCst,
        );
    }

    extern "C" fn three() -> i32 {
        3
    }

    extern "C" fn not(b: bool) -> bool {
        !b
    }

    fn addr(f: *const ()) -> usize {
        f as usize
    }

    /// Mixed signatures, as `jn`, `ldexp` and `ilogb` have, are called.
    #[test]
    fn a_mixed_signature_is_called_with_its_types() {
        use Value::{Float32, Float64, Int32};
        let cases: [(usize, ScalarType, Vec<Value>, Value); 5] = [
            (
                addr(jn_like as *const ()),
                ScalarType::Float32,
                vec![Int32(2), Float32(0.5)],
                Float32(200.5),
            ),
            (
                addr(ldexp_like as *const ()),
                ScalarType::Float64,
                vec![Float64(1.5), Int32(4)],
                Float64(6.0),
            ),
            (
                addr(ilogb_like as *const ()),
                ScalarType::Int32,
                vec![Float32(7.9)],
                Int32(8),
            ),
            (
                addr(four as *const ()),
                ScalarType::Float64,
                vec![Float64(1.0), Int32(2), Float32(3.0), Int32(4)],
                Float64(4321.0),
            ),
            (
                addr(three as *const ()),
                ScalarType::Int32,
                vec![],
                Int32(3),
            ),
        ];
        for (address, ret, args, expected) in cases {
            assert_eq!(invoke(address, ret, &args), Some(expected), "{args:?}");
        }
        assert_eq!(
            invoke(
                addr(sink as *const ()),
                ScalarType::Void,
                &[Int32(1), Float64(2.0), Int32(3)]
            ),
            Some(Value::Void)
        );
        assert_eq!(SEEN.load(Ordering::SeqCst), 321);
        assert_eq!(
            invoke(
                addr(not as *const ()),
                ScalarType::Bool,
                &[Value::Bool(true)]
            ),
            Some(Value::Bool(false))
        );
    }

    /// An unsupported signature returns `None` without calling: the address
    /// is never used.
    #[test]
    fn an_unsupported_signature_is_not_called() {
        use Value::{Bool, Float32, Int32};
        let never = 0;
        for (ret, args) in [
            (ScalarType::Float32, vec![Float32(0.0); MAX_ARGS + 1]),
            (ScalarType::Bool, vec![Int32(1)]),
            (ScalarType::Int32, vec![Bool(true)]),
            (ScalarType::Void, vec![Bool(true)]),
            (ScalarType::Float32, vec![Value::Void]),
        ] {
            let types: Vec<ScalarType> = args.iter().map(scalar_type).collect();
            assert!(!is_supported(ret, &types), "{ret:?} {args:?}");
            assert_eq!(invoke(never, ret, &args), None, "{ret:?} {args:?}");
        }
    }

    fn scalar_type(value: &Value) -> ScalarType {
        match value {
            Value::Int32(_) => ScalarType::Int32,
            Value::Float32(_) => ScalarType::Float32,
            Value::Float64(_) => ScalarType::Float64,
            Value::Bool(_) => ScalarType::Bool,
            Value::Void => ScalarType::Void,
        }
    }

    /// Every C math library function is called through `invoke`, by the
    /// signature its pointer carries.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn the_c_math_library_functions_are_called_by_their_signature() {
        let mut symbols: Vec<&str> = libm_functions().iter().map(|f| f.symbol).collect();
        symbols.sort_unstable();
        let len = symbols.len();
        symbols.dedup();
        assert_eq!(symbols.len(), len, "unique symbols");
        let call = |symbol: &str, args: &[Value]| {
            let function = libm_function(symbol).expect(symbol).function;
            let types: Vec<ScalarType> = args.iter().map(scalar_type).collect();
            assert_eq!(types, function.params(), "{symbol}");
            assert!(is_supported(function.ret(), function.params()), "{symbol}");
            invoke(function.address(), function.ret(), args).expect(symbol)
        };
        use Value::{Float32, Float64, Int32};
        // C math implementations need not round these approximations to the
        // same bit pattern: glibc's cbrt(27) is one ulp above 3. Keep the
        // signature checks above and allow only a few rounding units here.
        let approximate = |symbol: &str, args: &[Value], expected: Value| {
            let result = call(symbol, args);
            let (actual, expected, epsilon) = match (result, expected) {
                (Float32(actual), Float32(expected)) => (
                    f64::from(actual),
                    f64::from(expected),
                    f64::from(f32::EPSILON),
                ),
                (Float64(actual), Float64(expected)) => (actual, expected, f64::EPSILON),
                _ => panic!("{symbol}: unexpected return type {result:?}"),
            };
            let tolerance = 4.0 * epsilon * expected.abs();
            assert!(
                (actual - expected).abs() <= tolerance,
                "{symbol}: expected {expected}, got {actual} (tolerance {tolerance})"
            );
        };
        approximate("cbrtf", &[Float32(8.0)], Float32(2.0));
        approximate("cbrt", &[Float64(27.0)], Float64(3.0));
        approximate("hypotf", &[Float32(3.0), Float32(4.0)], Float32(5.0));
        assert_eq!(call("ilogbf", &[Float32(10.0)]), Int32(3));
        assert_eq!(call("logb", &[Float64(10.0)]), Float64(3.0));
        assert_eq!(call("ldexpf", &[Float32(1.5), Int32(3)]), Float32(12.0));
        assert_eq!(call("scalbnf", &[Float32(1.5), Int32(2)]), Float32(6.0));
        assert_eq!(call("ldexp", &[Float64(1.5), Int32(-1)]), Float64(0.75));
        approximate("tgamma", &[Float64(5.0)], Float64(24.0));
        assert_eq!(
            call("nextafterf", &[Float32(1.0), Float32(2.0)]),
            Float32(1.0 + f32::EPSILON)
        );
        // j0(0) = 1, j1(0) = 0, jn(2, 0) = 0.
        assert_eq!(call("j0", &[Float64(0.0)]), Float64(1.0));
        assert_eq!(call("jn", &[Int32(2), Float64(0.0)]), Float64(0.0));
        // An exact `ldexpf`, also at the edges of `float`.
        assert_eq!(
            call("ldexpf", &[Float32(1.0), Int32(-149)]),
            Float32(f32::from_bits(1))
        );
        assert_eq!(
            call("ldexpf", &[Float32(1.0), Int32(128)]),
            Float32(f32::INFINITY)
        );
    }

    /// `is_supported` accepts every numeric signature up to `MAX_ARGS`
    /// arguments, and the `bool` ones.
    #[test]
    fn is_supported_covers_every_numeric_combination() {
        use ScalarType::{Bool, Float32, Float64, Int32, Void};
        let numeric = [Int32, Float32, Float64];
        let mut lists: Vec<Vec<ScalarType>> = vec![vec![]];
        let mut all = lists.clone();
        for _ in 0..MAX_ARGS {
            lists = lists
                .iter()
                .flat_map(|list| {
                    numeric.iter().map(move |ty| {
                        let mut next = list.clone();
                        next.push(*ty);
                        next
                    })
                })
                .collect();
            all.extend(lists.iter().cloned());
        }
        assert_eq!(all.len(), 1 + 3 + 9 + 27 + 81);
        for args in &all {
            for ret in [Int32, Float32, Float64, Void] {
                assert!(is_supported(ret, args), "{ret:?} {args:?}");
            }
        }
        for (ret, args) in [
            (Bool, vec![]),
            (Bool, vec![Bool]),
            (Bool, vec![Bool, Bool]),
            (Void, vec![Bool, Bool]),
        ] {
            assert!(is_supported(ret, &args));
        }
    }
}
