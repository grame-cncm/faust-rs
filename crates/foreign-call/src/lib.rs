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
