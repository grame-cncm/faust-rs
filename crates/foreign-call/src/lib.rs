//! Runtime bridge from symbolic Faust foreign bindings to host C functions.
//!
//! This crate supports only the scalar signatures represented by [`ScalarType`]
//! and [`Value`]. A caller must supply an address with the matching `extern "C"`
//! ABI; a mismatch is unsupported and can violate the host ABI contract.

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

/// Invokes a supported scalar `extern "C"` function at `addr`.
///
/// Returns `None` when `args` does not match one of the supported homogeneous
/// zero-, one-, or two-argument signatures. `addr` must be a valid function
/// address whose ABI, argument types, and return type match `ret` and `args`.
#[must_use]
pub fn invoke(addr: usize, ret: ScalarType, args: &[Value]) -> Option<Value> {
    match (ret, args) {
        (ScalarType::Void, []) => {
            let f: extern "C" fn() = unsafe { std::mem::transmute(addr) };
            f();
            Some(Value::Void)
        }
        (ScalarType::Int32, []) => {
            let f: extern "C" fn() -> i32 = unsafe { std::mem::transmute(addr) };
            Some(Value::Int32(f()))
        }
        (ScalarType::Float32, []) => {
            let f: extern "C" fn() -> f32 = unsafe { std::mem::transmute(addr) };
            Some(Value::Float32(f()))
        }
        (ScalarType::Float64, []) => {
            let f: extern "C" fn() -> f64 = unsafe { std::mem::transmute(addr) };
            Some(Value::Float64(f()))
        }
        (ScalarType::Bool, []) => {
            let f: extern "C" fn() -> bool = unsafe { std::mem::transmute(addr) };
            Some(Value::Bool(f()))
        }

        (ScalarType::Void, [Value::Float32(a0)]) => {
            let f: extern "C" fn(f32) = unsafe { std::mem::transmute(addr) };
            f(*a0);
            Some(Value::Void)
        }
        (ScalarType::Float32, [Value::Float32(a0)]) => {
            let f: extern "C" fn(f32) -> f32 = unsafe { std::mem::transmute(addr) };
            Some(Value::Float32(f(*a0)))
        }
        (ScalarType::Float64, [Value::Float64(a0)]) => {
            let f: extern "C" fn(f64) -> f64 = unsafe { std::mem::transmute(addr) };
            Some(Value::Float64(f(*a0)))
        }
        (ScalarType::Int32, [Value::Int32(a0)]) => {
            let f: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(addr) };
            Some(Value::Int32(f(*a0)))
        }
        (ScalarType::Bool, [Value::Bool(a0)]) => {
            let f: extern "C" fn(bool) -> bool = unsafe { std::mem::transmute(addr) };
            Some(Value::Bool(f(*a0)))
        }

        (ScalarType::Void, [Value::Float32(a0), Value::Float32(a1)]) => {
            let f: extern "C" fn(f32, f32) = unsafe { std::mem::transmute(addr) };
            f(*a0, *a1);
            Some(Value::Void)
        }
        (ScalarType::Float32, [Value::Float32(a0), Value::Float32(a1)]) => {
            let f: extern "C" fn(f32, f32) -> f32 = unsafe { std::mem::transmute(addr) };
            Some(Value::Float32(f(*a0, *a1)))
        }
        (ScalarType::Void, [Value::Float64(a0), Value::Float64(a1)]) => {
            let f: extern "C" fn(f64, f64) = unsafe { std::mem::transmute(addr) };
            f(*a0, *a1);
            Some(Value::Void)
        }
        (ScalarType::Float64, [Value::Float64(a0), Value::Float64(a1)]) => {
            let f: extern "C" fn(f64, f64) -> f64 = unsafe { std::mem::transmute(addr) };
            Some(Value::Float64(f(*a0, *a1)))
        }
        (ScalarType::Void, [Value::Int32(a0), Value::Int32(a1)]) => {
            let f: extern "C" fn(i32, i32) = unsafe { std::mem::transmute(addr) };
            f(*a0, *a1);
            Some(Value::Void)
        }
        (ScalarType::Int32, [Value::Int32(a0), Value::Int32(a1)]) => {
            let f: extern "C" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(addr) };
            Some(Value::Int32(f(*a0, *a1)))
        }
        (ScalarType::Void, [Value::Bool(a0), Value::Bool(a1)]) => {
            let f: extern "C" fn(bool, bool) = unsafe { std::mem::transmute(addr) };
            f(*a0, *a1);
            Some(Value::Void)
        }
        (ScalarType::Bool, [Value::Bool(a0), Value::Bool(a1)]) => {
            let f: extern "C" fn(bool, bool) -> bool = unsafe { std::mem::transmute(addr) };
            Some(Value::Bool(f(*a0, *a1)))
        }

        _ => None,
    }
}
