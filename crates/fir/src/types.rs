//! Public FIR semantic type and operation enums.
//!
//! These definitions describe storage classes, value types, operations, and UI
//! metadata used by canonical FIR nodes. They are intentionally target-neutral
//! so backends can map them to their own concrete representations.

/// Memory-access class for FIR variable nodes.
///
/// This is a semantic storage class, not a target-specific address space:
/// backends may map these categories to different concrete layouts as long as
/// lifetime/visibility semantics remain equivalent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessType {
    Stack,
    Struct,
    Static,
    FunArgs,
    Loop,
    Global,
}

/// What a [`FirMatch::DeclareDependency`](crate::FirMatch::DeclareDependency) names.
///
/// API mapping status: `adapted`. C++ keeps the two sets on its code
/// container (`CodeContainer::fIncludeFileSet`, `fLibrarySet`); FIR carries
/// them as declarations in the module's `globals`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FirDependencyKind {
    /// A header to include, with its delimiters: `"file.h"` or `<file.h>`.
    Include,
    /// A library to link with, as written in the source.
    Library,
}

/// FIR primitive type model.
///
/// FIR keeps result types explicit on value nodes so backend passes can emit
/// code without reconstructing types from context.
#[derive(Clone, Debug, PartialEq)]
pub enum FirType {
    Int32,
    Int64,
    Float32,
    Float64,
    /// Backend-defined scalar sample/control type (`FAUSTFLOAT` in C++).
    FaustFloat,
    Quad,
    FixedPoint,
    Bool,
    Void,
    Obj,
    /// C++ parity note: API handle kind equivalent to `kSound_ptr`.
    ///
    /// Semantics: this variant already models a pointer-shaped handle.
    /// Use `FirType::Ptr(Box::new(FirType::Sound))` only for explicit extra
    /// pointer indirection (for example `Soundfile**`).
    Sound,
    /// C++ parity note: API handle kind equivalent to `kUI_ptr`.
    ///
    /// Semantics: this variant already models a pointer-shaped handle.
    /// Canonical FIR API signatures therefore use `UI` directly
    /// (`buildUserInterface(UI)`), not `Ptr(UI)`.
    UI,
    /// C++ parity note: API handle kind equivalent to `kMeta_ptr`.
    ///
    /// Semantics: this variant already models a pointer-shaped handle.
    /// Use `Ptr(Meta)` only when an additional pointer level is intended.
    Meta,
    /// Generic explicit pointer constructor.
    ///
    /// This is used for pointer depth that is semantically relevant in FIR
    /// values/signatures (for example `FAUSTFLOAT**` in `compute`), while
    /// `UI`/`Sound`/`Meta` already encode their base API-handle pointer level.
    Ptr(Box<FirType>),
    Array(Box<FirType>, usize),
    Vector(Box<FirType>, usize),
    Struct(String, Vec<FirType>),
    Fun {
        args: Vec<FirType>,
        ret: Box<FirType>,
    },
}

/// FIR binary operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FirBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Lsh,
    ARsh,
    LRsh,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Canonical FIR math operation identifiers used by backend-agnostic lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FirMathOp {
    Pow,
    Min,
    Max,
    Sin,
    Cos,
    Acos,
    Asin,
    Atan,
    Atan2,
    Tan,
    Exp,
    Exp10,
    Log,
    Log10,
    Sqrt,
    Abs,
    Fmod,
    Remainder,
    Floor,
    Ceil,
    Rint,
    Round,
}

impl FirMathOp {
    /// Returns the backend-agnostic FIR symbol used for this operation.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Pow => "pow",
            Self::Min => "fmin",
            Self::Max => "fmax",
            Self::Sin => "sin",
            Self::Cos => "cos",
            Self::Acos => "acos",
            Self::Asin => "asin",
            Self::Atan => "atan",
            Self::Atan2 => "atan2",
            Self::Tan => "tan",
            Self::Exp => "exp",
            Self::Exp10 => "exp10",
            Self::Log => "log",
            Self::Log10 => "log10",
            Self::Sqrt => "sqrt",
            Self::Abs => "fabs",
            Self::Fmod => "fmod",
            Self::Remainder => "remainder",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::Rint => "rint",
            Self::Round => "round",
        }
    }

    /// Parses canonical/`std::` FIR math names and the single-precision
    /// `powf`/`rintf` aliases used alongside integer-power double calls.
    #[must_use]
    pub fn from_symbol(name: &str) -> Option<Self> {
        let symbol = name.strip_prefix("std::").unwrap_or(name);
        match symbol {
            "pow" | "powf" => Some(Self::Pow),
            "fmin" => Some(Self::Min),
            "fmax" => Some(Self::Max),
            "sin" => Some(Self::Sin),
            "cos" => Some(Self::Cos),
            "acos" => Some(Self::Acos),
            "asin" => Some(Self::Asin),
            "atan" => Some(Self::Atan),
            "atan2" => Some(Self::Atan2),
            "tan" => Some(Self::Tan),
            "exp" => Some(Self::Exp),
            "exp10" => Some(Self::Exp10),
            "log" => Some(Self::Log),
            "log10" => Some(Self::Log10),
            "sqrt" => Some(Self::Sqrt),
            "fabs" => Some(Self::Abs),
            "fmod" => Some(Self::Fmod),
            "remainder" => Some(Self::Remainder),
            "floor" => Some(Self::Floor),
            "ceil" => Some(Self::Ceil),
            "rint" | "rintf" => Some(Self::Rint),
            "round" => Some(Self::Round),
            _ => None,
        }
    }
}

/// UI box orientation for FIR UI instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiBoxType {
    Vertical,
    Horizontal,
    Tab,
}

/// FIR UI button kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ButtonType {
    Button,
    Checkbox,
}

/// FIR UI slider kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SliderType {
    Horizontal,
    Vertical,
    NumEntry,
}

/// FIR UI bargraph kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BargraphType {
    Horizontal,
    Vertical,
}

/// Slider range/value payload used by FIR UI slider instructions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderRange {
    /// Initial widget value.
    pub init: f64,
    /// Lower bound.
    pub lo: f64,
    /// Upper bound.
    pub hi: f64,
    /// Step increment.
    pub step: f64,
}

/// Named type used for function signatures.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedType {
    /// Source-level or ABI-level item name.
    pub name: String,
    /// Declared FIR type for the named item.
    pub typ: FirType,
}
