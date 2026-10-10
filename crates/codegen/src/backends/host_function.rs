//! The C signature of a host function, shared by the backends that call
//! host functions by address (Cranelift, the interpreter), and how a call of
//! another shape is explained.
//!
//! A host registers a foreign function under a name, with or without its
//! signature. With one, a call follows the rule C applies through a
//! prototype: the arguments and the result convert to the registered types,
//! and a call is refused when its argument count differs or when it declares
//! an `int` result for a real one or the reverse.

/// The C type of a host function's parameter or result.
///
/// API mapping status: `extension`; the C++ backends'
/// `registerForeignFunction` takes a name only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostScalarType {
    /// `int` (32 bits).
    Int,
    /// `float`.
    Float,
    /// `double`.
    Double,
}

impl HostScalarType {
    /// The C spelling.
    #[must_use]
    pub fn c_name(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Double => "double",
        }
    }

    /// Whether this is a real type (`float` or `double`) rather than `int`.
    pub(crate) fn is_real(self) -> bool {
        !matches!(self, Self::Int)
    }
}

/// The C signature of a host function, as a host registers it:
/// `float(int, float, float)`.
///
/// API mapping status: `extension` (see [`HostScalarType`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostFunctionSignature {
    /// The result type.
    pub ret: HostScalarType,
    /// The parameter types, in order.
    pub params: Vec<HostScalarType>,
}

impl HostFunctionSignature {
    /// Parses a C prototype without or with its function name:
    /// `float(int, float)`, `double w_d(double)`, `int()`, `int(void)`.
    /// The types are `int`, `float` and `double`; `name`, when given, must be
    /// the one written.
    ///
    /// # Errors
    /// The reason, when `text` is not such a prototype.
    pub fn parse(text: &str, name: Option<&str>) -> Result<Self, String> {
        let scalar = |word: &str| match word {
            "int" => Ok(HostScalarType::Int),
            "float" => Ok(HostScalarType::Float),
            "double" => Ok(HostScalarType::Double),
            other => Err(format!(
                "`{other}` is not a host function type (`int`, `float` or `double`)"
            )),
        };
        let malformed = || format!("`{text}` is not a C prototype such as `float(int, float)`");
        let (head, rest) = text.split_once('(').ok_or_else(malformed)?;
        let params = rest
            .trim_end()
            .strip_suffix(')')
            .ok_or_else(malformed)?
            .trim();
        let mut head = head.split_whitespace();
        let ret = scalar(head.next().ok_or_else(malformed)?)?;
        if let Some(written) = head.next()
            && let Some(name) = name.filter(|name| *name != written)
        {
            return Err(format!(
                "the prototype `{text}` names `{written}`, not `{name}`"
            ));
        }
        if head.next().is_some() {
            return Err(malformed());
        }
        let params = if params.is_empty() || params == "void" {
            Vec::new()
        } else {
            params
                .split(',')
                .map(|param| scalar(param.trim()))
                .collect::<Result<_, _>>()?
        };
        Ok(Self { ret, params })
    }

    /// The prototype under `name`: `float name(int, float)`.
    #[must_use]
    pub fn prototype(&self, name: &str) -> String {
        let params: Vec<&str> = self.params.iter().map(|ty| ty.c_name()).collect();
        format!("{} {name}({})", self.ret.c_name(), params.join(", "))
    }
}

impl std::fmt::Display for HostFunctionSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let params: Vec<&str> = self.params.iter().map(|ty| ty.c_name()).collect();
        write!(f, "{}({})", self.ret.c_name(), params.join(", "))
    }
}

/// A declaration in the `ffunction` syntax, where `float` is the real type
/// of either precision.
pub(crate) fn faust_declaration(
    name: &str,
    ret: HostScalarType,
    params: &[HostScalarType],
) -> String {
    let faust = |ty: HostScalarType| if ty.is_real() { "float" } else { "int" };
    let params: Vec<&str> = params.iter().map(|ty| faust(*ty)).collect();
    format!("{} {name}({})", faust(ret), params.join(", "))
}

/// Explains a call whose shape is not the one of the function it calls,
/// `against`, and how to declare it.
pub(crate) fn signature_mismatch(
    name: &str,
    against: &str,
    ret: HostScalarType,
    params: &[HostScalarType],
    call_ret: HostScalarType,
    arity: usize,
) -> String {
    let declaration = faust_declaration(name, ret, params);
    let arguments = if arity == 1 { "argument" } else { "arguments" };
    format!(
        "`{name}` does not have the signature of {against}: the call has {arity} {arguments} \
         and {} result; declare it `{declaration}` in the `ffunction` (the arguments and the \
         precision convert as in C)",
        if call_ret.is_real() {
            "a `float`"
        } else {
            "an `int`"
        }
    )
}
