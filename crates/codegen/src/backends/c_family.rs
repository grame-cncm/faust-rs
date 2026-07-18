//! Shared emission helpers for the C-family textual backends (`c`, `cpp`).
//!
//! # Purpose
//! `c` and `cpp` are near-parallel implementations that track each other
//! function for function (see
//! `porting/c-family-emitter-core-plan-2026-07-04-en.md` for the full
//! duplication analysis and the drift this caused). This module is the shared
//! core: Phase 1 moved the syntax-parameterless functions
//! ([`emit_binop`]/[`emit_binop_expr`]) verbatim; Phase 2 introduced the
//! [`CFamilySyntax`] descriptor and moved the pure leaf-lookup functions
//! ([`emit_type`], [`emit_static_tables`], [`trim_float`], [`format_float32`],
//! [`string_literal`]). Functions whose per-language variation is behavioral
//! rather than a token leaf (variable-reference rendering, function-name
//! rewriting, the DSP lifecycle shells) stay in `cpp`/`c`.
//!
//! # Invariant
//! Functions here keep `cpp`'s and `c`'s generated output byte-identical to
//! what each backend emitted separately, with two deliberate exceptions fixed
//! by Phase 2 unification (both verified against the upstream C++ compiler as
//! oracle; see the plan document §2.3/§2.4 and §4 Phase 2 outcome):
//! - [`trim_float`] normalizes `-0.0` to `"0.0"` — previously only `c` (and
//!   `julia`) did; upstream emits `0.0f` for a folded negative zero.
//! - [`string_literal`] escapes `\r`/`\t` — previously only `c` (and `julia`)
//!   did; `cpp` emitted the raw bytes into the literal.
//!
//! # Source provenance (C++)
//! Upstream `/Users/letz/faust/compiler/generator/Text.hh` plays the same
//! role for every one of upstream's textual backends: one shared
//! literal/operator-formatting module instead of one copy per backend.

use std::fmt::Write as _;

use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, match_fir};

use crate::backends::purity::is_obviously_side_effect_free_value;

/// Syntax parameters distinguishing the C-family textual backends (`c`, `cpp`).
///
/// Every field is a fixed token leaf, not behavior — plain data, matching the
/// style already used by `CppOptions`/`COptions`. Each per-language module
/// owns one `const` instance and threads it through the shared emission
/// functions in this module. Leaves that are runtime-configurable
/// (`quad_type_name`/`fixed_type_name`) stay in the per-language `Options`
/// structs and are passed alongside the descriptor.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CFamilySyntax {
    /// Spelling for `FirType::Bool` (`"bool"` in C++, `"int"` in C).
    pub bool_type: &'static str,
    /// Spelling for `FirType::UI` (`"UI*"` in C++, `"UIGlue*"` in C).
    pub ui_type: &'static str,
    /// Spelling for `FirType::Meta` (`"Meta*"` in C++, `"MetaGlue*"` in C).
    pub meta_type: &'static str,
    /// Keyword order for a top-level static const array declaration
    /// (`"const static"` in C++, `"static const"` in C — cosmetic, kept
    /// data-driven so it is never hand-duplicated again).
    pub static_table_keywords: &'static str,
    /// Spelling for a true `FirMatch::Bool` literal (`"true"` in C++, `"1"` in C).
    pub bool_true: &'static str,
    /// Spelling for a false `FirMatch::Bool` literal (`"false"` in C++, `"0"` in C).
    pub bool_false: &'static str,
    /// Spelling for `FirMatch::NullValue` (`"nullptr"` in C++, `"NULL"` in C).
    pub null_value: &'static str,
    /// Extra leading argument for UI glue calls taking further arguments
    /// (`"ui_interface->uiInterface, "` in C, where the glue struct threads an
    /// explicit interface handle; `""` in C++, where `ui_interface` is an
    /// object).
    pub ui_glue_arg: &'static str,
    /// Same glue handle when it is the *only* argument (`closeBox`):
    /// `"ui_interface->uiInterface"` in C, `""` in C++.
    pub ui_glue_solo: &'static str,
    /// Opening token of the `FAUSTFLOAT` conversion wrapped around UI widget
    /// numeric arguments (`"FAUSTFLOAT("` in C++ — functional-cast style,
    /// matching upstream `cast2FAUSTFLOAT`; `"(FAUSTFLOAT)"` in C).
    pub faustfloat_cast_open: &'static str,
    /// Closing token of the `FAUSTFLOAT` conversion (`")"` in C++, `""` in C).
    pub faustfloat_cast_close: &'static str,
    /// Whether the `default:` arm of a `switch` emits a `break;` (`true` in
    /// C, `false` in C++ — a pre-existing cosmetic divergence preserved
    /// data-driven; see the plan document §2.8).
    pub switch_default_break: bool,
    /// Opening token of a `Bitcast` type-punning expression, up to the target
    /// type (`"*reinterpret_cast<"` in C++ — matching upstream `-ftz 2`
    /// output; `"*(("` in C).
    pub bitcast_open: &'static str,
    /// Middle token of a `Bitcast`, between the target type and the operand
    /// (`"*>(&"` in C++, `"*)&"` in C).
    pub bitcast_mid: &'static str,
    /// Closing token of a `Bitcast` (`")"` in both today, kept as a leaf for
    /// symmetry with the other two).
    pub bitcast_close: &'static str,
}

/// Rendering mode for statement/expression emission.
///
/// `Compute` enables the subset of formatting rules that are specific to the
/// sample loop and output buffer writes. `Metadata` and `Ui` preserve the C++
/// split between `m->declare(...)` in `metadata()` and
/// `ui_interface->declare(...)` in `buildUserInterface()`.
///
/// Shared by the `c` and `cpp` backends (the two per-backend copies were
/// identical); the mode is threaded through [`emit_stmt_common`] and only
/// *read* by the per-language `AddMetaDeclare` arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmitMode {
    Default,
    Metadata,
    Ui,
    Compute,
}

/// One scalar state-field initialization that must be replayed by the
/// synthesized `instanceResetUserInterface` fallback when the FIR module does
/// not supply an explicit body (shared by `c` and `cpp`; `julia` implements
/// the same pattern independently).
#[derive(Debug, Clone)]
pub(crate) struct StructInit {
    pub name: String,
    pub typ: FirType,
    pub init: FirId,
}

/// One table declaration plus its initializer payload, replayed element by
/// element by the same reset fallback as [`StructInit`].
#[derive(Clone, Debug)]
pub(crate) struct TableInit {
    pub name: String,
    pub access: AccessType,
    pub elem_type: FirType,
    pub values: Vec<FirId>,
}

/// Collects scalar struct/global initializers used by reset lifecycle
/// fallbacks.
///
/// `invalid_section` builds the caller's backend error when a state section
/// is not a FIR block (each backend owns its stable error codes).
pub(crate) fn collect_struct_initializers<E>(
    store: &FirStore,
    dsp_struct: FirId,
    globals: FirId,
    invalid_section: impl Fn(FirId) -> E,
) -> Result<Vec<StructInit>, E> {
    let mut out = Vec::new();
    for section in [dsp_struct, globals] {
        let FirMatch::Block(items) = match_fir(store, section) else {
            return Err(invalid_section(section));
        };
        for item in items {
            if let FirMatch::DeclareVar {
                name,
                typ,
                init: Some(init),
                ..
            } = match_fir(store, item)
            {
                out.push(StructInit { name, typ, init });
            }
        }
    }
    Ok(out)
}

/// Collects table initializers from FIR state declarations, for the same
/// reset fallback as [`collect_struct_initializers`].
pub(crate) fn collect_table_initializers<E>(
    store: &FirStore,
    dsp_struct: FirId,
    globals: FirId,
    invalid_section: impl Fn(FirId) -> E,
) -> Result<Vec<TableInit>, E> {
    let mut out = Vec::new();
    for section in [dsp_struct, globals] {
        let FirMatch::Block(items) = match_fir(store, section) else {
            return Err(invalid_section(section));
        };
        for item in items {
            if let FirMatch::DeclareTable {
                name,
                access,
                elem_type,
                values,
            } = match_fir(store, item)
            {
                out.push(TableInit {
                    name,
                    access,
                    elem_type,
                    values,
                });
            }
        }
    }
    Ok(out)
}

/// Recursion seam into a caller's block/statement emitter: the caller's
/// environment (`store`, `Options`, module name) is captured by the closure;
/// `out`, the node id, the indent, and the mode are passed per call so their
/// mutable borrows stay with the shared driver.
pub(crate) type EmitNodeFn<'a, E> =
    &'a dyn Fn(&mut String, FirId, usize, &mut EmitMode) -> Result<(), E>;

/// Per-language seams for the shared statement emitter [`emit_stmt_common`].
///
/// Same design as [`CFamilyValueCtx`]: plain fn pointers for capture-free
/// per-language rendering rules, `&dyn Fn` for the seams that need the
/// caller's `Options`/`module_name` environment. `out`/`mode` are passed per
/// call (not captured) so the mutable borrows stay with the shared driver.
pub(crate) struct CFamilyStmtCtx<'a, E> {
    /// Fixed token leaves for this language.
    pub syntax: &'a CFamilySyntax,
    /// Renders a variable reference under its storage class (see
    /// [`CFamilyValueCtx::var_ref`]).
    pub var_ref: fn(&str, AccessType) -> String,
    /// Renders the increment expression of a non-reverse `ForLoop`
    /// (`i += step` in C++, `i = i + step` in C — both goldens preserved).
    pub for_loop_step: fn(&str, &str) -> String,
    /// Renders the increment expression of a non-reverse `SimpleForLoop`
    /// (`++i` in C++, `i = i + 1` in C).
    pub simple_loop_increment: fn(&str) -> String,
    /// Renders `type name` declarations with array suffixes (the caller's
    /// `emit_named_type` wrapper).
    pub render_named_type: &'a dyn Fn(&FirType, &str) -> String,
    /// Renders a FIR type (the caller's `emit_type` wrapper).
    pub render_type: &'a dyn Fn(&FirType) -> String,
    /// Renders a FIR value expression (the caller's `emit_value` wrapper).
    pub render_value: &'a dyn Fn(FirId) -> Result<String, E>,
    /// Recurses into the caller's block emitter (`emit_block_with_mode`).
    pub emit_block: EmitNodeFn<'a, E>,
    /// Recurses into the caller's statement emitter, so per-language arms
    /// stay visible under shared containers (`Control`).
    pub emit_stmt: EmitNodeFn<'a, E>,
}

/// Per-language seams for the shared value-expression emitter
/// [`emit_value_common`].
///
/// Everything a value arm needs beyond fixed token leaves lives here:
/// variable-reference rendering, function-name rewriting, type rendering (the
/// caller's `Options`-aware `emit_type` wrapper), and recursion back through
/// the caller's own `emit_value` so per-language extra arms stay visible to
/// nested sub-expressions. `E` is the caller's backend error type — the
/// shared core never constructs errors itself (see [`emit_value_common`]'s
/// `None` contract).
pub(crate) struct CFamilyValueCtx<'a, E> {
    /// Fixed token leaves for this language.
    pub syntax: &'a CFamilySyntax,
    /// Renders a variable reference under its storage class
    /// (`dsp->name` for `AccessType::Struct` in C; bare `name` in C++,
    /// where `this` is implicit).
    pub var_ref: fn(&str, AccessType) -> String,
    /// Rewrites a bare FIR function-call name to this language's spelling
    /// (`std::` prefixing in C++; `min_i`/`max_i` aliases and `std::`
    /// stripping in C).
    pub fun_name: fn(&str) -> String,
    /// Renders a FIR type with the caller's configured `Quad`/`FixedPoint`
    /// spellings (the caller's `emit_type` wrapper).
    pub render_type: &'a dyn Fn(&FirType) -> String,
    /// Recurses into the caller's own `emit_value`, so language-only arms
    /// (e.g. C++'s `Quad`/array literals) keep working inside shared arms.
    pub recurse: &'a dyn Fn(FirId) -> Result<String, E>,
}

/// Maps one FIR binary operator to its C-family infix token spelling.
///
/// Shared verbatim by `c` and `cpp`: both languages use the same infix
/// operator tokens for every `FirBinOp` variant (including logical/arithmetic
/// right shift, which both render as `>>` and disambiguate in
/// [`emit_binop_expr`]).
#[must_use]
pub(crate) fn emit_binop(op: FirBinOp) -> &'static str {
    match op {
        FirBinOp::Add => "+",
        FirBinOp::Sub => "-",
        FirBinOp::Mul => "*",
        FirBinOp::Div => "/",
        FirBinOp::Rem => "%",
        FirBinOp::And => "&",
        FirBinOp::Or => "|",
        FirBinOp::Xor => "^",
        FirBinOp::Lsh => "<<",
        FirBinOp::ARsh => ">>",
        FirBinOp::LRsh => ">>",
        FirBinOp::Eq => "==",
        FirBinOp::Ne => "!=",
        FirBinOp::Lt => "<",
        FirBinOp::Le => "<=",
        FirBinOp::Gt => ">",
        FirBinOp::Ge => ">=",
    }
}

/// Renders one FIR binary operator expression as a parenthesized C-family
/// expression string.
///
/// `FirBinOp::LRsh` (logical/unsigned right shift) needs an explicit
/// `uint32_t`/`int32_t` round-trip in both `c` and `cpp` because neither
/// language's `>>` on a signed `int32_t` operand is guaranteed to be a
/// logical shift; every other operator renders as a plain infix expression
/// via [`emit_binop`].
#[must_use]
pub(crate) fn emit_binop_expr(op: FirBinOp, lhs: &str, rhs: &str) -> String {
    match op {
        FirBinOp::LRsh => format!("((int32_t)(((uint32_t)({lhs})) >> ({rhs})))"),
        _ => format!("({lhs} {} {rhs})", emit_binop(op)),
    }
}

/// Renders a FIR type into its C-family spelling.
///
/// All structural rendering (pointers, arrays, vectors, function types) is
/// identical between `c` and `cpp`; the three leaves that differ
/// (`Bool`/`UI`/`Meta`) come from `syntax`, and the two runtime-configurable
/// leaves (`Quad`/`FixedPoint`) come from the caller's `Options` struct as
/// `quad`/`fixed`.
///
/// FIR handle kinds (`Sound`/`UI`/`Meta`) are already pointer-shaped at the
/// type-model level, so `Ptr(UI)` renders as a double pointer.
#[must_use]
pub(crate) fn emit_type(typ: &FirType, syntax: &CFamilySyntax, quad: &str, fixed: &str) -> String {
    match typ {
        FirType::Int32 => "int".to_owned(),
        FirType::Int64 => "long long".to_owned(),
        FirType::Float32 => "float".to_owned(),
        FirType::Float64 => "double".to_owned(),
        FirType::FaustFloat => "FAUSTFLOAT".to_owned(),
        FirType::Quad => quad.to_owned(),
        FirType::FixedPoint => fixed.to_owned(),
        FirType::Bool => syntax.bool_type.to_owned(),
        FirType::Void => "void".to_owned(),
        FirType::Obj => "void*".to_owned(),
        FirType::Sound => "Soundfile*".to_owned(),
        FirType::UI => syntax.ui_type.to_owned(),
        FirType::Meta => syntax.meta_type.to_owned(),
        FirType::Ptr(inner) => format!("{}*", emit_type(inner, syntax, quad, fixed)),
        FirType::Array(inner, size) => format!("{}[{size}]", emit_type(inner, syntax, quad, fixed)),
        FirType::Vector(inner, lanes) => {
            format!("Vec<{},{lanes}>", emit_type(inner, syntax, quad, fixed))
        }
        FirType::Struct(name, _fields) => name.clone(),
        FirType::Fun { args, ret } => {
            let args = args
                .iter()
                .map(|arg| emit_type(arg, syntax, quad, fixed))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({args})", emit_type(ret, syntax, quad, fixed))
        }
    }
}

/// Formats one `f64` as a C-family floating literal.
///
/// Rust's `Display` formatting emits the shortest round-trippable decimal and
/// the code appends `.0` for integral-looking values. Special values use the
/// `math.h` spellings (`NAN`, `INFINITY`, `-INFINITY`), and negative zero is
/// normalized to `"0.0"` — matching the upstream C++ compiler, which emits
/// `0.0f` for a constant folded to `-0.0`.
#[must_use]
pub(crate) fn trim_float(value: f64) -> String {
    if value.is_nan() {
        return "NAN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-INFINITY".to_owned()
        } else {
            "INFINITY".to_owned()
        };
    }
    let mut s = format!("{value}");
    if !s.contains(['.', 'e', 'E']) {
        s.push_str(".0");
    }
    if s == "-0.0" { "0.0".to_owned() } else { s }
}

/// Formats one single-precision value as a C-family `float` literal
/// (`{value}f`), delegating special values to [`trim_float`] unsuffixed.
#[must_use]
pub(crate) fn format_float32(value: f64) -> String {
    if !value.is_finite() {
        return trim_float(value);
    }
    format!("{}f", trim_float(value))
}

/// Escapes a Rust string into a C-family double-quoted string literal.
///
/// Escapes `\`, `"`, `\n`, `\r`, and `\t`; used for user-authored strings
/// (UI labels, metadata keys/values, soundfile URLs), which the faust-rs
/// pipeline passes through verbatim, so control characters can genuinely
/// reach the emitter.
#[must_use]
pub(crate) fn string_literal(input: &str) -> String {
    let escaped = input
        .chars()
        .flat_map(|c| match c {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            _ => vec![c],
        })
        .collect::<String>();
    format!("\"{escaped}\"")
}

/// Emits `DeclareTable(AccessType::Static)` nodes as static const arrays with
/// inline initializers, placed before the class/struct definition.
///
/// The declaration keyword order comes from `syntax.static_table_keywords`;
/// element types render through the shared [`emit_type`]; element values
/// render through the caller's `render_value` (value emission is still
/// per-backend until the plan's Phase 3), which also carries the caller's
/// error type `E`.
pub(crate) fn emit_static_tables<E>(
    store: &FirStore,
    out: &mut String,
    syntax: &CFamilySyntax,
    quad: &str,
    fixed: &str,
    block: FirId,
    mut render_value: impl FnMut(FirId) -> Result<String, E>,
) -> Result<(), E> {
    let FirMatch::Block(stmts) = match_fir(store, block) else {
        return Ok(());
    };
    let keywords = syntax.static_table_keywords;
    for stmt in stmts {
        if let FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } = match_fir(store, stmt)
        {
            let type_str = emit_type(&elem_type, syntax, quad, fixed);
            let n = values.len();
            if n == 0 {
                let _ = writeln!(out, "{keywords} {type_str} {name}[0] = {{}};");
            } else {
                let _ = write!(out, "{keywords} {type_str} {name}[{n}] = {{");
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        let _ = write!(out, ", ");
                    }
                    let rendered = render_value(*v)?;
                    let _ = write!(out, "{rendered}");
                }
                let _ = writeln!(out, "}};");
            }
        }
    }
    Ok(())
}

/// Emits the FIR value arms shared verbatim (modulo [`CFamilyValueCtx`]
/// seams) by `c` and `cpp`.
///
/// # Contract
/// Returns `None` when `value` is not one of the shared arms — the caller
/// then handles its language-only arms (C++'s `Quad`/`FixedPoint`/array
/// literals, `NewDsp`) or produces its own unsupported-node error.
/// This keeps each backend's error behavior and language-only surface exactly
/// where it was: the shared core owns only the intersection, plus the
/// per-drift closures decided with their own oracle checks — `Bitcast`
/// (DRIFT 2, plan §2.2) is shared here with per-language spelling leaves:
/// `c` gains support it never had, `cpp` replaces its former `bitcast<T>(v)`
/// spelling (which named a helper neither the backend nor upstream defines)
/// with the upstream `-ftz 2` form.
pub(crate) fn emit_value_common<E>(
    store: &FirStore,
    ctx: &CFamilyValueCtx<'_, E>,
    value: FirId,
) -> Option<Result<String, E>> {
    let result = match match_fir(store, value) {
        FirMatch::Int32 { value, .. } => Ok(value.to_string()),
        FirMatch::Int64 { value, .. } => Ok(value.to_string()),
        FirMatch::Float32 { value, .. } => Ok(format_float32(f64::from(value))),
        FirMatch::Float64 { value, .. } => Ok(trim_float(value)),
        FirMatch::Bool { value, .. } => Ok(if value {
            ctx.syntax.bool_true
        } else {
            ctx.syntax.bool_false
        }
        .to_owned()),
        FirMatch::LoadVar { name, access, .. } | FirMatch::LoadVarAddress { name, access, .. } => {
            Ok((ctx.var_ref)(&name, access))
        }
        FirMatch::LoadTable {
            name,
            access,
            index,
            ..
        } => match (ctx.recurse)(index) {
            Ok(index) => Ok(format!("{}[{index}]", (ctx.var_ref)(&name, access))),
            Err(err) => Err(err),
        },
        FirMatch::TeeVar {
            name,
            access,
            value,
            ..
        } => match (ctx.recurse)(value) {
            Ok(value) => Ok(format!("({} = {value})", (ctx.var_ref)(&name, access))),
            Err(err) => Err(err),
        },
        FirMatch::BinOp { op, lhs, rhs, .. } => match ((ctx.recurse)(lhs), (ctx.recurse)(rhs)) {
            (Ok(lhs), Ok(rhs)) => Ok(emit_binop_expr(op, &lhs, &rhs)),
            (Err(err), _) | (_, Err(err)) => Err(err),
        },
        FirMatch::Neg { value, .. } => match (ctx.recurse)(value) {
            Ok(value) => Ok(format!("(-{value})")),
            Err(err) => Err(err),
        },
        FirMatch::Cast { typ, value } => match (ctx.recurse)(value) {
            Ok(value) => Ok(format!("(({})({value}))", (ctx.render_type)(&typ))),
            Err(err) => Err(err),
        },
        // Type punning via pointer reinterpretation, matching upstream
        // `-ftz 2` output (`*reinterpret_cast<int*>(&v)` in C++; C uses the
        // corrected `*((int*)&v)` spelling of the same intent — upstream C's
        // own `BitcastInst` visitor is a known-broken TODO). Like upstream,
        // this requires the operand to render as an addressable expression;
        // FIR producers cache bitcast operands into named temporaries.
        FirMatch::Bitcast { typ, value } => match (ctx.recurse)(value) {
            Ok(value) => Ok(format!(
                "{}{}{}{value}{}",
                ctx.syntax.bitcast_open,
                (ctx.render_type)(&typ),
                ctx.syntax.bitcast_mid,
                ctx.syntax.bitcast_close
            )),
            Err(err) => Err(err),
        },
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => {
            match (
                (ctx.recurse)(cond),
                (ctx.recurse)(then_value),
                (ctx.recurse)(else_value),
            ) {
                (Ok(cond), Ok(then_value), Ok(else_value)) => {
                    Ok(format!("({cond} ? {then_value} : {else_value})"))
                }
                (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => Err(err),
            }
        }
        FirMatch::FunCall { name, args, .. } => {
            let mut rendered = Vec::with_capacity(args.len());
            for arg in args {
                match (ctx.recurse)(arg) {
                    Ok(arg) => rendered.push(arg),
                    Err(err) => return Some(Err(err)),
                }
            }
            Ok(format!(
                "{}({})",
                (ctx.fun_name)(&name),
                rendered.join(", ")
            ))
        }
        FirMatch::NullValue { .. } => Ok(ctx.syntax.null_value.to_owned()),
        FirMatch::LoadSoundfileLength { var, part } => match (ctx.recurse)(part) {
            Ok(part) => Ok(format!(
                "{}->fLength[{part}]",
                (ctx.var_ref)(&var, AccessType::Struct)
            )),
            Err(err) => Err(err),
        },
        FirMatch::LoadSoundfileRate { var, part } => match (ctx.recurse)(part) {
            Ok(part) => Ok(format!(
                "{}->fSR[{part}]",
                (ctx.var_ref)(&var, AccessType::Struct)
            )),
            Err(err) => Err(err),
        },
        FirMatch::LoadSoundfileBuffer {
            var,
            chan,
            part,
            idx,
            ..
        } => match ((ctx.recurse)(chan), (ctx.recurse)(part), (ctx.recurse)(idx)) {
            (Ok(chan), Ok(part), Ok(idx)) => {
                let sf = (ctx.var_ref)(&var, AccessType::Struct);
                Ok(format!(
                    "((FAUSTFLOAT**){sf}->fBuffers)[{chan}][{sf}->fOffset[{part}] + {idx}]"
                ))
            }
            (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => Err(err),
        },
        _ => return None,
    };
    Some(result)
}

/// Emits the FIR statement arms shared (modulo [`CFamilyStmtCtx`] seams) by
/// `c` and `cpp`.
///
/// # Contract
/// Returns `None` when `stmt` is not one of the shared arms — the caller then
/// handles its language-only arms (`DeclareFun` nesting in C++, the
/// structurally different `AddMetaDeclare`/`Label` renderings in both) or
/// produces its own unsupported-node error. Like [`emit_value_common`], the
/// shared core owns only the intersection; the deliberate exceptions are the
/// drift closures below.
///
/// # Drift closures (plan document §2, Phase 4–5)
/// - `DeclareTable` renders initializer values for non-`Struct` access
///   (DRIFT 1: `cpp` previously dropped them; `Struct`-access declarations —
///   class fields — stay bare in both, which is also why `c`'s output is
///   unchanged: its struct fields never route through statement emission).
/// - `Control`/`WhileLoop` are shared (DRIFT 7: `c` previously had no arms
///   for them and hard-failed).
/// - `AddSlider`/`AddBargraph` numeric arguments are wrapped in the
///   `FAUSTFLOAT` conversion from the syntax leaves (DRIFT 5: `cpp`
///   previously emitted bare literals; upstream C++ wraps every argument via
///   `cast2FAUSTFLOAT`, `cpp_instructions.hh:44`).
pub(crate) fn emit_stmt_common<E>(
    store: &FirStore,
    out: &mut String,
    ctx: &CFamilyStmtCtx<'_, E>,
    stmt: FirId,
    indent: usize,
    mode: &mut EmitMode,
) -> Option<Result<(), E>> {
    let tab = "    ".repeat(indent);
    let result = match match_fir(store, stmt) {
        FirMatch::DeclareVar {
            name,
            typ,
            access: _,
            init,
        } => {
            let _ = write!(out, "{tab}{}", (ctx.render_named_type)(&typ, &name));
            if let Some(init) = init {
                match (ctx.render_value)(init) {
                    Ok(init) => {
                        let _ = write!(out, " = {init}");
                    }
                    Err(err) => return Some(Err(err)),
                }
            }
            let _ = writeln!(out, ";");
            Ok(())
        }
        FirMatch::DeclareTable {
            name,
            access,
            elem_type,
            values,
        } => {
            if access == AccessType::Struct {
                // Class/struct fields declare storage only; initialization is
                // the job of the lifecycle methods.
                let _ = writeln!(
                    out,
                    "{tab}{} {}[{}];",
                    (ctx.render_type)(&elem_type),
                    name,
                    values.len()
                );
            } else {
                let mut rendered = Vec::with_capacity(values.len());
                for value in &values {
                    match (ctx.render_value)(*value) {
                        Ok(value) => rendered.push(value),
                        Err(err) => return Some(Err(err)),
                    }
                }
                let _ = writeln!(
                    out,
                    "{tab}{} {}[{}] = {{{}}};",
                    (ctx.render_type)(&elem_type),
                    name,
                    values.len(),
                    rendered.join(", ")
                );
            }
            Ok(())
        }
        FirMatch::StoreVar {
            name,
            access,
            value,
        } => match (ctx.render_value)(value) {
            Ok(value) => {
                let _ = writeln!(out, "{tab}{} = {value};", (ctx.var_ref)(&name, access));
                Ok(())
            }
            Err(err) => Err(err),
        },
        FirMatch::StoreTable {
            name,
            access,
            index,
            value,
        } => match ((ctx.render_value)(index), (ctx.render_value)(value)) {
            (Ok(index), Ok(value)) => {
                let _ = writeln!(
                    out,
                    "{tab}{}[{index}] = {value};",
                    (ctx.var_ref)(&name, access)
                );
                Ok(())
            }
            (Err(err), _) | (_, Err(err)) => Err(err),
        },
        FirMatch::Drop(value) => {
            // Vector FIR uses Drop roots to materialize and verify a shared
            // expression DAG. Once the roots have become ordinary loads,
            // constants, or pure arithmetic, emitting `(void)(...)` only
            // bloats generated C/C++ and cannot affect execution. Keep every
            // call or unrecognised node: FIR has no general purity annotation
            // for foreign functions, so the conservative fallback preserves
            // potential side effects.
            if is_obviously_side_effect_free_value(store, value) {
                Ok(())
            } else {
                match (ctx.render_value)(value) {
                    Ok(value) => {
                        let _ = writeln!(out, "{tab}(void)({value});");
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            }
        }
        FirMatch::NullStatement => {
            let _ = writeln!(out, "{tab};");
            Ok(())
        }
        FirMatch::Return(value) => {
            if let Some(value) = value {
                match (ctx.render_value)(value) {
                    Ok(value) => {
                        let _ = writeln!(out, "{tab}return {value};");
                    }
                    Err(err) => return Some(Err(err)),
                }
            } else {
                let _ = writeln!(out, "{tab}return;");
            }
            Ok(())
        }
        FirMatch::Block(_) => (ctx.emit_block)(out, stmt, indent, mode),
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => match (ctx.render_value)(cond) {
            Ok(cond) => {
                let _ = writeln!(out, "{tab}if ({cond}) {{");
                if let Err(err) = (ctx.emit_block)(out, then_block, indent + 1, mode) {
                    return Some(Err(err));
                }
                let _ = writeln!(out, "{tab}}}");
                if let Some(else_block) = else_block {
                    let _ = writeln!(out, "{tab}else {{");
                    if let Err(err) = (ctx.emit_block)(out, else_block, indent + 1, mode) {
                        return Some(Err(err));
                    }
                    let _ = writeln!(out, "{tab}}}");
                }
                Ok(())
            }
            Err(err) => Err(err),
        },
        FirMatch::Control { cond, stmt } => match (ctx.render_value)(cond) {
            Ok(cond) => {
                let _ = writeln!(out, "{tab}if ({cond}) {{");
                if let Err(err) = (ctx.emit_stmt)(out, stmt, indent + 1, mode) {
                    return Some(Err(err));
                }
                let _ = writeln!(out, "{tab}}}");
                Ok(())
            }
            Err(err) => Err(err),
        },
        FirMatch::ForLoop {
            var,
            init,
            end,
            step,
            body,
            is_reverse,
        } => {
            // init is a DeclareVar(kLoop) per FIR contract; extract its value.
            let init_source =
                if let FirMatch::DeclareVar { init: Some(v), .. } = match_fir(store, init) {
                    v
                } else {
                    init
                };
            match (
                (ctx.render_value)(init_source),
                (ctx.render_value)(end),
                (ctx.render_value)(step),
            ) {
                (Ok(init_val), Ok(end), Ok(step)) => {
                    if is_reverse {
                        let _ = writeln!(
                            out,
                            "{tab}for (int {var} = {init_val}; {var} > {end}; {var} = {var} + {step}) {{"
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "{tab}for (int {var} = {init_val}; {var} < {end}; {}) {{",
                            (ctx.for_loop_step)(&var, &step)
                        );
                    }
                    if let Err(err) = (ctx.emit_block)(out, body, indent + 1, mode) {
                        return Some(Err(err));
                    }
                    let _ = writeln!(out, "{tab}}}");
                    Ok(())
                }
                (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => Err(err),
            }
        }
        FirMatch::SimpleForLoop {
            var,
            upper,
            body,
            is_reverse,
        } => match (ctx.render_value)(upper) {
            Ok(upper) => {
                if is_reverse {
                    let _ = writeln!(
                        out,
                        "{tab}for (int {var} = ({upper}) - 1; {var} >= 0; {var} = {var} - 1) {{"
                    );
                } else {
                    let _ = writeln!(
                        out,
                        "{tab}for (int {var} = 0; {var} < {upper}; {}) {{",
                        (ctx.simple_loop_increment)(&var)
                    );
                }
                if let Err(err) = (ctx.emit_block)(out, body, indent + 1, mode) {
                    return Some(Err(err));
                }
                let _ = writeln!(out, "{tab}}}");
                Ok(())
            }
            Err(err) => Err(err),
        },
        FirMatch::WhileLoop { cond, body } => match (ctx.render_value)(cond) {
            Ok(cond) => {
                let _ = writeln!(out, "{tab}while ({cond}) {{");
                if let Err(err) = (ctx.emit_block)(out, body, indent + 1, mode) {
                    return Some(Err(err));
                }
                let _ = writeln!(out, "{tab}}}");
                Ok(())
            }
            Err(err) => Err(err),
        },
        FirMatch::Switch {
            cond,
            ref cases,
            default,
        } => match (ctx.render_value)(cond) {
            Ok(cond) => {
                let _ = writeln!(out, "{tab}switch ({cond}) {{");
                for (value, block) in cases {
                    let _ = writeln!(out, "{tab}case {value}: {{");
                    if let Err(err) = (ctx.emit_block)(out, *block, indent + 1, mode) {
                        return Some(Err(err));
                    }
                    let _ = writeln!(out, "{tab}    break;");
                    let _ = writeln!(out, "{tab}}}");
                }
                if let Some(default) = default {
                    let _ = writeln!(out, "{tab}default: {{");
                    if let Err(err) = (ctx.emit_block)(out, default, indent + 1, mode) {
                        return Some(Err(err));
                    }
                    if ctx.syntax.switch_default_break {
                        let _ = writeln!(out, "{tab}    break;");
                    }
                    let _ = writeln!(out, "{tab}}}");
                }
                let _ = writeln!(out, "{tab}}}");
                Ok(())
            }
            Err(err) => Err(err),
        },
        FirMatch::OpenBox { typ, label } => {
            let api = match typ {
                fir::UiBoxType::Vertical => "openVerticalBox",
                fir::UiBoxType::Horizontal => "openHorizontalBox",
                fir::UiBoxType::Tab => "openTabBox",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface->{api}({}{});",
                ctx.syntax.ui_glue_arg,
                string_literal(&label)
            );
            Ok(())
        }
        FirMatch::CloseBox => {
            let _ = writeln!(
                out,
                "{tab}ui_interface->closeBox({});",
                ctx.syntax.ui_glue_solo
            );
            Ok(())
        }
        FirMatch::AddButton { typ, label, var } => {
            let api = match typ {
                fir::ButtonType::Button => "addButton",
                fir::ButtonType::Checkbox => "addCheckButton",
            };
            let _ = writeln!(
                out,
                "{tab}ui_interface->{api}({}{}, &{});",
                ctx.syntax.ui_glue_arg,
                string_literal(&label),
                (ctx.var_ref)(&var, AccessType::Struct)
            );
            Ok(())
        }
        FirMatch::AddSlider {
            typ,
            label,
            var,
            init,
            lo,
            hi,
            step,
        } => {
            let api = match typ {
                fir::SliderType::Horizontal => "addHorizontalSlider",
                fir::SliderType::Vertical => "addVerticalSlider",
                fir::SliderType::NumEntry => "addNumEntry",
            };
            let co = ctx.syntax.faustfloat_cast_open;
            let cc = ctx.syntax.faustfloat_cast_close;
            let _ = writeln!(
                out,
                "{tab}ui_interface->{api}({}{}, &{}, {co}{}{cc}, {co}{}{cc}, {co}{}{cc}, {co}{}{cc});",
                ctx.syntax.ui_glue_arg,
                string_literal(&label),
                (ctx.var_ref)(&var, AccessType::Struct),
                trim_float(init),
                trim_float(lo),
                trim_float(hi),
                trim_float(step)
            );
            Ok(())
        }
        FirMatch::AddBargraph {
            typ,
            label,
            var,
            lo,
            hi,
        } => {
            let api = match typ {
                fir::BargraphType::Horizontal => "addHorizontalBargraph",
                fir::BargraphType::Vertical => "addVerticalBargraph",
            };
            let co = ctx.syntax.faustfloat_cast_open;
            let cc = ctx.syntax.faustfloat_cast_close;
            let _ = writeln!(
                out,
                "{tab}ui_interface->{api}({}{}, &{}, {co}{}{cc}, {co}{}{cc});",
                ctx.syntax.ui_glue_arg,
                string_literal(&label),
                (ctx.var_ref)(&var, AccessType::Struct),
                trim_float(lo),
                trim_float(hi)
            );
            Ok(())
        }
        FirMatch::AddSoundfile { label, url, var } => {
            let _ = writeln!(
                out,
                "{tab}ui_interface->addSoundfile({}{}, {}, &{});",
                ctx.syntax.ui_glue_arg,
                string_literal(&label),
                string_literal(&url),
                (ctx.var_ref)(&var, AccessType::Struct)
            );
            Ok(())
        }
        _ => return None,
    };
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::{
        CFamilySyntax, emit_binop, emit_binop_expr, emit_type, format_float32, string_literal,
        trim_float,
    };
    use crate::backends::purity::is_obviously_side_effect_free_value;
    use fir::{AccessType, FirBinOp, FirBuilder, FirStore, FirType};

    const TEST_SYNTAX: CFamilySyntax = CFamilySyntax {
        bool_type: "bool",
        ui_type: "UI*",
        meta_type: "Meta*",
        static_table_keywords: "const static",
        bool_true: "true",
        bool_false: "false",
        null_value: "nullptr",
        ui_glue_arg: "",
        ui_glue_solo: "",
        faustfloat_cast_open: "FAUSTFLOAT(",
        faustfloat_cast_close: ")",
        switch_default_break: false,
        bitcast_open: "*reinterpret_cast<",
        bitcast_mid: "*>(&",
        bitcast_close: ")",
    };

    #[test]
    fn emit_binop_covers_every_token() {
        assert_eq!(emit_binop(FirBinOp::Add), "+");
        assert_eq!(emit_binop(FirBinOp::Sub), "-");
        assert_eq!(emit_binop(FirBinOp::Mul), "*");
        assert_eq!(emit_binop(FirBinOp::Div), "/");
        assert_eq!(emit_binop(FirBinOp::Rem), "%");
        assert_eq!(emit_binop(FirBinOp::And), "&");
        assert_eq!(emit_binop(FirBinOp::Or), "|");
        assert_eq!(emit_binop(FirBinOp::Xor), "^");
        assert_eq!(emit_binop(FirBinOp::Lsh), "<<");
        assert_eq!(emit_binop(FirBinOp::ARsh), ">>");
        assert_eq!(emit_binop(FirBinOp::LRsh), ">>");
        assert_eq!(emit_binop(FirBinOp::Eq), "==");
        assert_eq!(emit_binop(FirBinOp::Ne), "!=");
        assert_eq!(emit_binop(FirBinOp::Lt), "<");
        assert_eq!(emit_binop(FirBinOp::Le), "<=");
        assert_eq!(emit_binop(FirBinOp::Gt), ">");
        assert_eq!(emit_binop(FirBinOp::Ge), ">=");
    }

    #[test]
    fn emit_binop_expr_renders_plain_infix() {
        assert_eq!(emit_binop_expr(FirBinOp::Add, "a", "b"), "(a + b)");
        assert_eq!(emit_binop_expr(FirBinOp::Lt, "x", "1"), "(x < 1)");
    }

    #[test]
    fn emit_binop_expr_renders_logical_right_shift_specially() {
        assert_eq!(
            emit_binop_expr(FirBinOp::LRsh, "n", "3"),
            "((int32_t)(((uint32_t)(n)) >> (3)))"
        );
    }

    #[test]
    fn redundant_drop_purity_is_conservative_about_calls() {
        let mut store = FirStore::new();
        let mut builder = FirBuilder::new(&mut store);
        let one = builder.float32(1.0);
        let local = builder.load_var("local", AccessType::Stack, FirType::Float32);
        let sum = builder.binop(FirBinOp::Add, one, local, FirType::Float32);
        let foreign = builder.fun_call("foreign_side_effect", &[], FirType::Float32);

        assert!(is_obviously_side_effect_free_value(&store, sum));
        assert!(!is_obviously_side_effect_free_value(&store, foreign));
    }

    #[test]
    fn emit_type_renders_shared_leaves_and_structure() {
        assert_eq!(
            emit_type(&FirType::Int32, &TEST_SYNTAX, "quad", "fixed"),
            "int"
        );
        assert_eq!(
            emit_type(&FirType::FaustFloat, &TEST_SYNTAX, "quad", "fixed"),
            "FAUSTFLOAT"
        );
        assert_eq!(
            emit_type(&FirType::Quad, &TEST_SYNTAX, "myquad", "fixed"),
            "myquad"
        );
        assert_eq!(
            emit_type(
                &FirType::Ptr(Box::new(FirType::Float32)),
                &TEST_SYNTAX,
                "quad",
                "fixed"
            ),
            "float*"
        );
        assert_eq!(
            emit_type(
                &FirType::Array(Box::new(FirType::Int32), 8),
                &TEST_SYNTAX,
                "quad",
                "fixed"
            ),
            "int[8]"
        );
    }

    #[test]
    fn emit_type_renders_syntax_leaves_from_descriptor() {
        let c_like = CFamilySyntax {
            bool_type: "int",
            ui_type: "UIGlue*",
            meta_type: "MetaGlue*",
            static_table_keywords: "static const",
            bool_true: "1",
            bool_false: "0",
            null_value: "NULL",
            ui_glue_arg: "ui_interface->uiInterface, ",
            ui_glue_solo: "ui_interface->uiInterface",
            faustfloat_cast_open: "(FAUSTFLOAT)",
            faustfloat_cast_close: "",
            switch_default_break: true,
            bitcast_open: "*((",
            bitcast_mid: "*)&",
            bitcast_close: ")",
        };
        assert_eq!(emit_type(&FirType::Bool, &TEST_SYNTAX, "q", "f"), "bool");
        assert_eq!(emit_type(&FirType::Bool, &c_like, "q", "f"), "int");
        assert_eq!(emit_type(&FirType::UI, &TEST_SYNTAX, "q", "f"), "UI*");
        assert_eq!(emit_type(&FirType::UI, &c_like, "q", "f"), "UIGlue*");
        assert_eq!(emit_type(&FirType::Meta, &TEST_SYNTAX, "q", "f"), "Meta*");
        assert_eq!(emit_type(&FirType::Meta, &c_like, "q", "f"), "MetaGlue*");
    }

    /// DRIFT 3 regression (plan §2.3): `-0.0` must normalize to `0.0` in all
    /// C-family backends; verified against the upstream C++ compiler, which
    /// emits `0.0f` for `process = -0.0;`.
    #[test]
    fn trim_float_normalizes_negative_zero() {
        assert_eq!(trim_float(-0.0), "0.0");
        assert_eq!(trim_float(0.0), "0.0");
        assert_eq!(format_float32(-0.0), "0.0f");
    }

    #[test]
    fn trim_float_spells_special_values() {
        assert_eq!(trim_float(f64::NAN), "NAN");
        assert_eq!(trim_float(f64::INFINITY), "INFINITY");
        assert_eq!(trim_float(f64::NEG_INFINITY), "-INFINITY");
        assert_eq!(format_float32(f64::INFINITY), "INFINITY");
    }

    #[test]
    fn trim_float_preserves_shortest_roundtrip_decimals() {
        assert_eq!(trim_float(0.5), "0.5");
        assert_eq!(trim_float(3.0), "3.0");
        // Rust `Display` expands small magnitudes positionally (no exponent);
        // this is the pre-existing c-backend spelling, preserved as-is (the
        // full-precision behavior commit 61407a68 relied on).
        assert_eq!(
            trim_float(4.656612875245797e-10),
            "0.0000000004656612875245797"
        );
        assert_eq!(format_float32(0.5), "0.5f");
    }

    /// DRIFT 4 regression (plan §2.4): control characters in user-authored
    /// strings (UI labels, metadata) must be escaped, never emitted raw.
    #[test]
    fn string_literal_escapes_control_characters() {
        assert_eq!(string_literal("a\tb"), "\"a\\tb\"");
        assert_eq!(string_literal("a\rb"), "\"a\\rb\"");
        assert_eq!(string_literal("a\nb"), "\"a\\nb\"");
        assert_eq!(string_literal("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(string_literal("plain"), "\"plain\"");
    }
}
