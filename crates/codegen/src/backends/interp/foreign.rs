//! Foreign-function registry and signature helpers for the interpreter backend.
//!
//! This is a Rust extension over the historical C++ interpreter path: the
//! original FBC interpreter only recognized a closed math-opcode table.
//! Here we keep that table intact and add an explicit host-function registry
//! used by `ffunction(...)` lowering and by runtime execution of serialized
//! bytecode carrying foreign call instructions.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::backends::host_function::{HostFunctionSignature, HostScalarType};
use fir::FirType;

/// Scalar types supported by interpreter foreign calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignScalarType {
    Int32,
    Float32,
    Float64,
    FaustFloat,
    Bool,
    Void,
}

impl ForeignScalarType {
    /// Encodes the scalar type as one stable ASCII code for bytecode names.
    #[must_use]
    pub fn code(self) -> char {
        match self {
            Self::Int32 => 'i',
            Self::Float32 => 'f',
            Self::Float64 => 'd',
            Self::FaustFloat => 'r',
            Self::Bool => 'b',
            Self::Void => 'v',
        }
    }

    /// Decodes one stable ASCII code back into a scalar type.
    #[must_use]
    pub fn from_code(code: char) -> Option<Self> {
        match code {
            'i' => Some(Self::Int32),
            'f' => Some(Self::Float32),
            'd' => Some(Self::Float64),
            'r' => Some(Self::FaustFloat),
            'b' => Some(Self::Bool),
            'v' => Some(Self::Void),
            _ => None,
        }
    }

    /// Maps one FIR type into a supported foreign scalar type.
    #[must_use]
    pub fn from_fir_type(typ: &FirType) -> Option<Self> {
        match typ {
            FirType::Int32 => Some(Self::Int32),
            FirType::Float32 => Some(Self::Float32),
            FirType::Float64 => Some(Self::Float64),
            FirType::FaustFloat => Some(Self::FaustFloat),
            FirType::Bool => Some(Self::Bool),
            FirType::Void => Some(Self::Void),
            _ => None,
        }
    }
}

/// Decoded foreign-call signature carried by one FBC foreign call instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignSignature {
    pub name: String,
    pub ret: ForeignScalarType,
    pub args: Vec<ForeignScalarType>,
}

/// Returns `true` if the interpreter runtime can execute this foreign-call
/// signature.
///
/// `foreign_call::invoke` calls a function through a pointer type known at
/// compile time, so it calls the signatures it enumerates only:
/// [`foreign_call::is_supported`] (up to [`foreign_call::MAX_ARGS`] `int`,
/// `float` or `double` arguments in any combination, and the `bool` shapes).
/// `FaustFloat` is `float` or `double`, which are supported alike.
#[must_use]
pub fn is_supported_signature(ret: ForeignScalarType, args: &[ForeignScalarType]) -> bool {
    let abi = |ty: ForeignScalarType| match ty {
        ForeignScalarType::Int32 => foreign_call::ScalarType::Int32,
        ForeignScalarType::Float32 | ForeignScalarType::FaustFloat => {
            foreign_call::ScalarType::Float32
        }
        ForeignScalarType::Float64 => foreign_call::ScalarType::Float64,
        ForeignScalarType::Bool => foreign_call::ScalarType::Bool,
        ForeignScalarType::Void => foreign_call::ScalarType::Void,
    };
    let args: Vec<_> = args.iter().map(|ty| abi(*ty)).collect();
    foreign_call::is_supported(abi(ret), &args)
}

impl ForeignSignature {
    /// Encodes one signature into the stable `instr.name` payload.
    #[must_use]
    pub fn encode(&self) -> String {
        let args: String = self.args.iter().map(|arg| arg.code()).collect();
        format!("{}|{}|{}", self.name, self.ret.code(), args)
    }

    /// Decodes one signature from the stable `instr.name` payload.
    #[must_use]
    pub fn decode(encoded: &str) -> Option<Self> {
        let mut parts = encoded.split('|');
        let name = parts.next()?.to_owned();
        let ret = ForeignScalarType::from_code(parts.next()?.chars().next()?)?;
        let args_part = parts.next().unwrap_or_default();
        if parts.next().is_some() {
            return None;
        }
        let mut args = Vec::with_capacity(args_part.len());
        for code in args_part.chars() {
            args.push(ForeignScalarType::from_code(code)?);
        }
        Some(Self { name, ret, args })
    }
}

/// One host function: its address, and the signature the host gave, if any.
#[derive(Clone)]
struct RegisteredFunction {
    address: usize,
    signature: Option<HostFunctionSignature>,
}

fn foreign_function_registry() -> &'static Mutex<HashMap<String, RegisteredFunction>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, RegisteredFunction>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register one process-global foreign function name -> host address binding.
///
/// A call uses the types of the `ffunction` declaration, which the host must
/// match, as with C++ `registerForeignFunction`;
/// [`register_foreign_function_with_signature`] checks them.
pub fn register_foreign_function(name: &str, fn_ptr: *mut std::ffi::c_void) {
    insert_foreign_function(name, fn_ptr, None);
}

/// Register one process-global foreign function with its C signature.
///
/// A call then converts its arguments and result to these types, as a C
/// prototype makes C do, and a declaration of another shape is a compile
/// error naming both. The interpreter calls the shapes
/// [`is_supported_signature`] accepts only.
pub fn register_foreign_function_with_signature(
    name: &str,
    fn_ptr: *mut std::ffi::c_void,
    signature: HostFunctionSignature,
) {
    insert_foreign_function(name, fn_ptr, Some(signature));
}

fn insert_foreign_function(
    name: &str,
    fn_ptr: *mut std::ffi::c_void,
    signature: Option<HostFunctionSignature>,
) {
    if name.is_empty() || fn_ptr.is_null() {
        return;
    }
    foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .insert(
            name.to_owned(),
            RegisteredFunction {
                address: fn_ptr as usize,
                signature,
            },
        );
}

/// Remove one process-global foreign function binding.
pub fn unregister_foreign_function(name: &str) {
    foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .remove(name);
}

/// Clear all process-global foreign function bindings.
pub fn clear_foreign_functions() {
    foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .clear();
}

/// Returns `true` when one foreign function binding exists for `name`.
#[must_use]
pub fn is_registered_foreign_function(name: &str) -> bool {
    foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .contains_key(name)
}

/// The signature `name` is registered with: `None` when it is not
/// registered, `Some(None)` when it is, without a signature.
#[must_use]
pub fn registered_foreign_signature(name: &str) -> Option<Option<HostFunctionSignature>> {
    foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .get(name)
        .map(|function| function.signature.clone())
}

/// Looks up one previously registered foreign function address.
#[must_use]
pub fn lookup_foreign_function(name: &str) -> Option<usize> {
    foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .get(name)
        .map(|function| function.address)
}

/// Looks up the address of `name` for a call of the host types
/// `ret(args)`, in the registry, else among the C math library functions:
/// `Some(Err(()))` when it has another signature than the bytecode was
/// compiled for (registered again since), `None` when it is neither.
#[must_use]
pub fn lookup_foreign_function_for_call(
    name: &str,
    ret: foreign_call::ScalarType,
    args: &[foreign_call::ScalarType],
) -> Option<Result<usize, ()>> {
    let registry = foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex");
    let Some(function) = registry.get(name) else {
        // A C math library function, after the registrations.
        let libm = foreign_call::libm_function(name)?.function;
        return Some(if libm.ret() == ret && libm.params() == args {
            Ok(libm.address())
        } else {
            Err(())
        });
    };
    let matches = function.signature.as_ref().is_none_or(|signature| {
        signature.ret.abi() == ret
            && signature.params.len() == args.len()
            && signature
                .params
                .iter()
                .zip(args)
                .all(|(param, arg)| param.abi() == *arg)
    });
    Some(if matches {
        Ok(function.address)
    } else {
        Err(())
    })
}

/// The signature of the system's C math library function `name`
/// ([`foreign_call::libm_functions`]), which the interpreter calls when no
/// host registered a function under that name. None on wasm32.
#[must_use]
pub fn libm_signature(name: &str) -> Option<HostFunctionSignature> {
    let function = foreign_call::libm_function(name)?.function;
    Some(HostFunctionSignature::of_typed(function))
}

/// The interpreter's foreign scalar type of a host function type.
#[must_use]
pub fn foreign_scalar(ty: HostScalarType) -> ForeignScalarType {
    match ty {
        HostScalarType::Int => ForeignScalarType::Int32,
        HostScalarType::Float => ForeignScalarType::Float32,
        HostScalarType::Double => ForeignScalarType::Float64,
    }
}

/// A fingerprint of the signatures hosts registered, empty when none did:
/// the factory cache key holds it, so a factory compiled under one
/// signature is not served under another. Untyped registrations leave the
/// key as libfaust computes it.
#[must_use]
pub fn registered_signature_fingerprint() -> String {
    let mut entries: Vec<String> = foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .iter()
        .filter_map(|(name, function)| {
            function
                .signature
                .as_ref()
                .map(|signature| format!("{name}:{signature}"))
        })
        .collect();
    entries.sort();
    entries.join(",")
}

#[cfg(test)]
pub fn clear_registered_foreign_functions_for_tests() {
    clear_foreign_functions();
}
