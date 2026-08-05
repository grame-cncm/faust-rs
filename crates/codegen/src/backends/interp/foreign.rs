//! Foreign-function registry and signature helpers for the interpreter backend.
//!
//! This is a Rust extension over the historical C++ interpreter path: the
//! original FBC interpreter only recognized a closed math-opcode table.
//! Here we keep that table intact and add an explicit host-function registry
//! used by `ffunction(...)` lowering and by runtime execution of serialized
//! bytecode carrying foreign call instructions.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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
/// signature with the current bounded scalar ABI.
///
/// The shape restrictions (0-2 args, all args the same type, and that type
/// matching `ret` unless `ret` is `Void`) mirror the finite set of
/// `extern "C" fn` signatures that `foreign_call::invoke` hand-enumerates and
/// dispatches to via `transmute`; there is no generic FFI call path, so any
/// signature outside that fixed table cannot actually be invoked at runtime.
#[must_use]
pub fn is_supported_signature(ret: ForeignScalarType, args: &[ForeignScalarType]) -> bool {
    match args {
        [] => true,
        [arg] => *arg != ForeignScalarType::Void && (*arg == ret || ret == ForeignScalarType::Void),
        [arg0, arg1] => {
            *arg0 != ForeignScalarType::Void
                && *arg1 != ForeignScalarType::Void
                && arg0 == arg1
                && (*arg0 == ret || ret == ForeignScalarType::Void)
        }
        _ => false,
    }
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

fn foreign_function_registry() -> &'static Mutex<HashMap<String, usize>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register one process-global foreign function name -> host address binding.
pub fn register_foreign_function(name: &str, fn_ptr: *mut std::ffi::c_void) {
    if name.is_empty() || fn_ptr.is_null() {
        return;
    }
    foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .insert(name.to_owned(), fn_ptr as usize);
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

/// Looks up one previously registered foreign function address.
#[must_use]
pub fn lookup_foreign_function(name: &str) -> Option<usize> {
    foreign_function_registry()
        .lock()
        .expect("interp foreign registry mutex")
        .get(name)
        .copied()
}

#[cfg(test)]
pub fn clear_registered_foreign_functions_for_tests() {
    clear_foreign_functions();
}
