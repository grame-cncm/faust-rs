//! Outlined blocks as methods of the struct (`MojoOptions::outline`, plan
//! `porting/mojo-large-program-outlining-plan-2026-11-09-en.md` §5).
//!
//! `fir::outline` moves the large blocks of a DSP method into new module
//! functions whose first argument is `dsp: Ptr(Obj)`; a variable the block
//! stores into is a `Ptr` of a scalar reached at index 0. The emitter prints
//! such a function as a method of the struct, without `@always_inline`, and
//! the pointer as a `mut` argument, which Mojo passes by reference.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::{CodegenError, CodegenErrorCode, EmitMode, Emitter, FunctionDecl, write_pass};
use crate::backends::module_view::unsupported_node;
use fir::{AccessType, FirId, FirMatch, FirType, match_fir};

impl Emitter<'_> {
    /// Records a module function emitted as a method: its Mojo name, which
    /// goes through the field renamer (methods and fields share the struct's
    /// namespace), and which parameters are references.
    pub(super) fn declare_method(&mut self, decl: &FunctionDecl) -> Result<(), CodegenError> {
        let name = self.fields.get(&decl.name)?;
        let refs = decl.named_args[1..]
            .iter()
            .map(|a| matches!(&a.typ, FirType::Ptr(inner) if is_scalar_type(inner)))
            .collect();
        self.methods.insert(decl.name.clone(), (name, refs));
        Ok(())
    }

    /// Emits the body of a method (an outlined block, `fir::outline`) and
    /// returns its signature: `dsp` as receiver, a `Ptr` of a scalar as a
    /// `mut` argument of that scalar (a reference to the caller's variable,
    /// reached at index 0 only), any other parameter by value.
    pub(super) fn emit_method(
        &mut self,
        out: &mut String,
        decl: &FunctionDecl,
    ) -> Result<String, CodegenError> {
        let FirType::Fun { ret, .. } = &decl.typ else {
            return Err(CodegenError::new(
                CodegenErrorCode::InvalidModuleSection,
                format!("function `{}` has no function type", decl.name),
            ));
        };
        if **ret != FirType::Void {
            return Err(CodegenError::new(
                CodegenErrorCode::InvalidModuleSection,
                format!("method `{}` returns a value", decl.name),
            ));
        }
        let (name, refs) = self.methods[&decl.name].clone();
        let mut params = String::new();
        let mut scope = HashMap::new();
        self.by_ref.clear();
        for (arg, is_ref) in decl.named_args[1..].iter().zip(refs) {
            let local = self.locals.get(&arg.name)?;
            match (&arg.typ, is_ref) {
                (FirType::Ptr(inner), true) => {
                    let _ = write!(params, ", mut {local}: {}", self.emit_type(inner)?);
                    self.by_ref.insert(arg.name.clone());
                }
                (typ, _) => {
                    let _ = write!(params, ", {local}: {}", self.emit_type(typ)?);
                }
            }
            scope.insert(arg.name.clone(), arg.typ.clone());
        }
        let body = decl
            .body
            .expect("collect_defined_functions keeps function definitions only");
        let start = out.len();
        self.scopes.clear();
        self.scopes.push(scope);
        let mut mode = EmitMode::Default;
        let result = self.emit_block_items(out, body, 2, &mut mode);
        self.scopes.clear();
        self.by_ref.clear();
        result?;
        if out.len() == start {
            write_pass(out);
        }
        Ok(format!("def {name}(mut dsp{params}) -> None:"))
    }

    /// `dsp.f(args)` for a call to a method, `None` for any other call.
    pub(super) fn method_call(&mut self, value: FirId) -> Result<Option<String>, CodegenError> {
        let FirMatch::FunCall { name, args, .. } = match_fir(self.store, value) else {
            return Ok(None);
        };
        let Some((mojo_name, refs)) = self.methods.get(&name).cloned() else {
            return Ok(None);
        };
        let receiver_ok = args.first().is_some_and(|&a| {
            matches!(
                match_fir(self.store, a),
                FirMatch::LoadVar { name, access: AccessType::FunArgs, .. } if name == "dsp"
            )
        });
        if !receiver_ok || args.len() != refs.len() + 1 {
            return Err(unsupported_node("method call", value, self.store));
        }
        let mut rendered = Vec::with_capacity(refs.len());
        for (&arg, is_ref) in args[1..].iter().zip(refs) {
            if !is_ref {
                rendered.push(self.emit_value(arg)?);
                continue;
            }
            // A reference names the caller's variable, or passes on a
            // reference of the calling method.
            let text = match match_fir(self.store, arg) {
                FirMatch::LoadVarAddress { name, access, .. }
                    if !matches!(
                        access,
                        AccessType::Struct | AccessType::Static | AccessType::Global
                    ) =>
                {
                    self.locals.get(&name)?
                }
                FirMatch::LoadVar {
                    name,
                    access: AccessType::FunArgs,
                    ..
                } if self.by_ref.contains(&name) => self.locals.get(&name)?,
                _ => {
                    return Err(unsupported_node(
                        "method reference argument",
                        arg,
                        self.store,
                    ));
                }
            };
            rendered.push(text);
        }
        Ok(Some(format!("dsp.{mojo_name}({})", rendered.join(", "))))
    }
}

/// Writes one outlined method (`fir::outline`): as [`super::write_method`], but
/// without `@always_inline`, which would merge it back into its caller.
pub(super) fn write_outlined_method(out: &mut String, signature: &str, body: &str) {
    let has_loop = body.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("for ") || line.starts_with("while ")
    });
    if has_loop {
        let _ = writeln!(
            out,
            "    {}",
            signature.replacen("(mut dsp", "(mut self", 1)
        );
        out.push_str("        ref dsp = self\n");
    } else {
        let _ = writeln!(out, "    {signature}");
    }
    out.push_str(body);
    let _ = writeln!(out);
}

/// A defined module function the emitter prints as a struct method: one
/// whose first argument is `dsp: Ptr(Obj)` (the outlined blocks of
/// `fir::outline`), the lifecycle functions aside.
pub(super) fn is_method(decl: &FunctionDecl) -> bool {
    decl.named_args
        .first()
        .is_some_and(|a| a.name == "dsp" && a.typ == FirType::Ptr(Box::new(FirType::Obj)))
}

pub(super) fn is_scalar_type(typ: &FirType) -> bool {
    matches!(
        typ,
        FirType::Int32
            | FirType::Int64
            | FirType::Float32
            | FirType::Float64
            | FirType::FaustFloat
            | FirType::Bool
    )
}
