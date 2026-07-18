//! FIR-to-Cranelift function-body lowering.
//!
//! This module owns the current supported compute-body subset and emits CLIF for
//! expressions, statements, stack slots, and struct-field access. Unsupported
//! shapes are filtered by `subset` before this lowering path is used.

use cranelift_frontend::Variable;

use super::*;
use crate::backends::purity::is_obviously_side_effect_free_value;

/// Lowered expression value tracked in the local Cranelift lowering environment.
///
/// FIR names in the current subset can denote either:
/// - scalar SSA values (ints/floats/bools), or
/// - pointer values (for stack aliases like `input0` / `output0`, fun args, etc.)
///
/// This enum preserves that distinction so statement lowering can reject invalid
/// uses early (for example writing a scalar as if it were a pointer table base).
#[derive(Clone, Copy, Debug)]
pub(crate) enum LoweredExpr {
    Scalar(Value),
    Ptr {
        value: Value,
        pointee: Option<FirTypeRef>,
    },
}

impl LoweredExpr {
    /// Returns the underlying CLIF value regardless of scalar/pointer tagging.
    ///
    /// Use this only when the consumer already knows the semantic category.
    fn value(self) -> Value {
        match self {
            Self::Scalar(v) => v,
            Self::Ptr { value, .. } => value,
        }
    }

    /// Returns the pointer CLIF value when this expression represents a pointer.
    ///
    /// This is mainly used by table alias lowering on stack/function-argument
    /// pointers.
    fn ptr(self) -> Option<Value> {
        match self {
            Self::Ptr { value, .. } => Some(value),
            Self::Scalar(_) => None,
        }
    }

    /// Returns the tracked pointee category for pointer expressions.
    ///
    /// This metadata is backend-local and intentionally coarse (see
    /// [`FirTypeRef`]); it is sufficient for current alias/table lowering.
    fn pointee(self) -> Option<FirTypeRef> {
        match self {
            Self::Ptr { pointee, .. } => pointee,
            Self::Scalar(_) => None,
        }
    }
}

/// One compute-local name binding.
///
/// FIR stack/loop locals carry C mutable-variable semantics: a local may be
/// written inside one loop body and read after the loop, as the lockstep
/// register-carry shape does with its per-sample state locals persisted at the
/// chunk tail. A raw CLIF `Value` cannot cross a block boundary without a
/// block parameter, so every FIR local is backed by a builder SSA `Variable`
/// (`def_var`/`use_var`) and the SSA constructor inserts parameters where flow
/// demands them. Function arguments stay direct entry-block values: they are
/// immutable and dominate the whole body by construction.
#[derive(Clone, Copy, Debug)]
enum LocalBinding {
    /// Immutable entry-block function argument.
    Entry(LoweredExpr),
    /// FIR stack/loop local backed by a builder SSA variable.
    Ssa {
        var: Variable,
        clif_ty: Type,
        is_ptr: bool,
        pointee: Option<FirTypeRef>,
    },
}

/// Lightweight FIR type classifier used in local lowering metadata.
///
/// This is intentionally smaller than [`FirType`] and mainly exists to annotate
/// pointer aliases (`LoweredExpr::Ptr`) with enough information to safely lower
/// `LoadTable`/`StoreTable` on stack aliases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FirTypeRef {
    Bool,
    Int32,
    Int64,
    Float32,
    Float64,
    FaustFloat,
    Obj,
    UI,
    Meta,
    Sound,
    Ptr,
}

impl FirTypeRef {
    /// Converts a full FIR type to the reduced local classification.
    ///
    /// Unsupported/compound FIR types collapse to `Ptr` because the current
    /// pointer alias metadata only needs rough categories, not full fidelity.
    fn from_fir_type(typ: &FirType) -> Self {
        match typ {
            FirType::Bool => Self::Bool,
            FirType::Int32 => Self::Int32,
            FirType::Int64 => Self::Int64,
            FirType::Float32 => Self::Float32,
            FirType::Float64 => Self::Float64,
            FirType::FaustFloat => Self::FaustFloat,
            FirType::Obj => Self::Obj,
            FirType::UI => Self::UI,
            FirType::Meta => Self::Meta,
            FirType::Sound => Self::Sound,
            FirType::Ptr(_) => Self::Ptr,
            _ => Self::Ptr,
        }
    }
}

/// Internal lowering failure type used while emitting the Cranelift `compute` body.
///
/// `Unsupported` means "valid FIR, but outside current subset"; callers may
/// convert this into a stub fallback. `Jit` represents structural/codegen
/// failures that should surface as backend errors.
#[derive(Debug)]
pub(crate) enum LoweringError {
    Unsupported(String),
    Jit(String),
}

/// Emits a valid no-op `return` for the current CLIF function.
///
/// This is the canonical stub body used by the early fallback policy.
pub(crate) fn emit_return_stub(fb: &mut FunctionBuilder<'_>) {
    fb.ins().return_(&[]);
}

/// Returns `true` when the current block already ends with a `return`.
///
/// Used to avoid emitting duplicate terminators after lowering control-flow
/// constructs that may already have returned.
pub(crate) fn is_return_terminated(fb: &FunctionBuilder<'_>) -> bool {
    let Some(block) = fb.current_block() else {
        return false;
    };
    let Some(inst) = fb.func.layout.last_inst(block) else {
        return false;
    };
    matches!(
        fb.func.dfg.insts[inst].opcode(),
        cranelift_codegen::ir::Opcode::Return
    )
}

/// Stateful FIR -> Cranelift lowering context for a single `compute` function.
///
/// This context owns:
/// - references to FIR storage and the active Cranelift JIT module,
/// - the current CLIF function builder,
/// - the backend `dsp*` layout contract for `AccessType::Struct`,
/// - a local name environment (`vars`) for FIR variables/aliases,
/// - cached imported function refs for math calls.
///
/// It is intentionally function-scoped: one instance per `compute` lowering.
pub(crate) struct ComputeLowering<'a, 'b, 'c> {
    /// Read-only access to the full FIR node store.
    pub(crate) store: &'a FirStore,
    /// Mutable JIT module used to declare/import functions and finalize code.
    pub(crate) jit: &'a mut JITModule,
    /// Active CLIF function builder for the `compute` function body.
    fb: &'a mut FunctionBuilder<'b>,
    /// Backend `dsp*` struct layout contract: field names → offsets/types.
    pub(crate) struct_layout: &'a StructLayoutPlan,
    /// Native pointer width (`I32` on 32-bit targets, `I64` on 64-bit).
    pub(crate) ptr_ty: Type,
    /// Local FIR variable → binding mapping (built up during lowering).
    vars: HashMap<String, LocalBinding>,
    /// Monotonic index for fresh builder SSA variables.
    next_var: u32,
    /// Cache of already-imported host function refs keyed by signature string.
    ///
    /// Cranelift requires explicit `declare_function` + `declare_func_in_func`
    /// calls per import; this cache avoids re-declaring the same symbol twice
    /// within the same function body.
    import_refs: HashMap<String, FuncRef>,
    /// Pre-declared JIT data IDs for `AccessType::Static` tables.
    pub(crate) static_data_ids: &'a HashMap<String, DataId>,
    /// Imported JIT data IDs for FIR `AccessType::Global` scalar symbols.
    pub(crate) extern_data_ids: &'a HashMap<String, DataId>,
    /// Registered host addresses for foreign function symbols resolved through
    /// `CraneliftOptions::extern_function_symbols`.
    pub(crate) extern_function_symbols: &'a HashMap<String, *const c_void>,
    /// 64-bit (`double`) precision flag: resolves `FaustFloat` to `F64`/8 bytes.
    double: bool,
    _marker: std::marker::PhantomData<&'c ()>,
}

impl<'a, 'b, 'c> ComputeLowering<'a, 'b, 'c> {
    /// Converts a CLIF boolean (`b1`) to the backend FIR-bool representation (`i8` 0/1).
    fn bool_b1_to_i8(&mut self, b1: Value) -> Value {
        let one = self.fb.ins().iconst(types::I8, 1);
        let zero = self.fb.ins().iconst(types::I8, 0);
        self.fb.ins().select(b1, one, zero)
    }

    /// Emits an integer comparison and returns FIR-style boolean `i8` (0/1).
    fn int_cmp_to_i8(&mut self, cc: IntCC, lhs: Value, rhs: Value) -> Value {
        let b1 = self.fb.ins().icmp(cc, lhs, rhs);
        self.bool_b1_to_i8(b1)
    }

    /// Emits a floating-point comparison and returns FIR-style boolean `i8` (0/1).
    fn float_cmp_to_i8(
        &mut self,
        cc: cranelift_codegen::ir::condcodes::FloatCC,
        lhs: Value,
        rhs: Value,
    ) -> Value {
        let b1 = self.fb.ins().fcmp(cc, lhs, rhs);
        self.bool_b1_to_i8(b1)
    }

    /// Maps a FIR type to the corresponding Cranelift value type used by this backend.
    ///
    /// This reflects current bring-up decisions, especially `FAUSTFLOAT -> F32`.
    fn fir_type_to_clif(&self, typ: &FirType) -> Result<Type, LoweringError> {
        fir_type_to_clif_type(self.ptr_ty, typ, self.double).map_err(LoweringError::Unsupported)
    }

    /// Resolves the abstract `FaustFloat` type to its concrete real type for the
    /// active precision (`Float64` under `-double`, else `Float32`), so type-keyed
    /// op/const selection below routes `FaustFloat` to the matching width.
    fn canon_real(&self, typ: &FirType) -> FirType {
        match typ {
            FirType::FaustFloat if self.double => FirType::Float64,
            FirType::FaustFloat => FirType::Float32,
            other => other.clone(),
        }
    }

    /// Returns the CLIF `dsp*` base pointer argument from the local environment.
    fn dsp_base_ptr(&self) -> Result<Value, LoweringError> {
        match self.vars.get("dsp") {
            Some(LocalBinding::Entry(expr)) => expr.ptr().ok_or_else(|| {
                LoweringError::Unsupported("`dsp` argument is not a pointer".to_string())
            }),
            _ => Err(LoweringError::Unsupported(
                "missing `dsp` base pointer argument".to_string(),
            )),
        }
    }

    /// CLIF storage type of one FIR local declaration.
    ///
    /// Scalars use their canonical width; every pointer-shaped declaration
    /// (explicit pointers, stack arrays, object-like refs) is held as one
    /// native pointer.
    fn local_clif_type(&self, typ: &FirType) -> Type {
        match self.canon_real(typ) {
            FirType::Int32 => types::I32,
            FirType::Int64 => types::I64,
            FirType::Bool => types::I8,
            FirType::Float32 => types::F32,
            FirType::Float64 => types::F64,
            _ => self.ptr_ty,
        }
    }

    /// Converts a value to a local's declared CLIF type with C assignment
    /// semantics.
    ///
    /// FIR `StoreVar` may assign a differently typed value to a local, exactly
    /// as a C assignment converts to the declared type; SSA variables reject
    /// that silently retyped rebinding, so the conversion must be explicit.
    /// Int/float mixing stays an error: FIR carries explicit casts for it.
    fn coerce_to_local_type(&mut self, value: Value, to: Type) -> Result<Value, LoweringError> {
        let from = self.fb.func.dfg.value_type(value);
        if from == to {
            return Ok(value);
        }
        let coerced = match (from, to) {
            (types::I8, types::I32) | (types::I8, types::I64) => self.fb.ins().uextend(to, value),
            (types::I32, types::I64) => self.fb.ins().sextend(to, value),
            (types::I64, types::I32) | (types::I64, types::I8) | (types::I32, types::I8) => {
                self.fb.ins().ireduce(to, value)
            }
            (types::F32, types::F64) => self.fb.ins().fpromote(to, value),
            (types::F64, types::F32) => self.fb.ins().fdemote(to, value),
            _ => {
                return Err(LoweringError::Unsupported(format!(
                    "cannot assign {from} value to {to} local"
                )));
            }
        };
        Ok(coerced)
    }

    /// Binds `name` to a fresh SSA variable of the declared type, defined with
    /// `expr`'s value converted to it.
    ///
    /// Returns the displaced binding so loop lowerings can shadow their
    /// induction variable and restore the outer binding afterwards.
    fn bind_ssa_local(
        &mut self,
        name: &str,
        expr: LoweredExpr,
        clif_ty: Type,
    ) -> Result<Option<LocalBinding>, LoweringError> {
        let (value, is_ptr, pointee) = match expr {
            LoweredExpr::Scalar(value) => (value, false, None),
            LoweredExpr::Ptr { value, pointee } => (value, true, pointee),
        };
        let value = self.coerce_to_local_type(value, clif_ty)?;
        let var = Variable::from_u32(self.next_var);
        self.next_var += 1;
        self.fb.declare_var(var, clif_ty);
        self.fb.def_var(var, value);
        Ok(self.vars.insert(
            name.to_string(),
            LocalBinding::Ssa {
                var,
                clif_ty,
                is_ptr,
                pointee,
            },
        ))
    }

    /// Reads the current value of a bound name, if any.
    fn read_local(&mut self, name: &str) -> Option<LoweredExpr> {
        match self.vars.get(name).copied()? {
            LocalBinding::Entry(expr) => Some(expr),
            LocalBinding::Ssa {
                var,
                is_ptr,
                pointee,
                ..
            } => {
                let value = self.fb.use_var(var);
                Some(if is_ptr {
                    LoweredExpr::Ptr { value, pointee }
                } else {
                    LoweredExpr::Scalar(value)
                })
            }
        }
    }

    /// Reads a bound name that must denote a pointer.
    fn read_local_ptr(&mut self, name: &str, what: &str) -> Result<Value, LoweringError> {
        let expr = self
            .read_local(name)
            .ok_or_else(|| LoweringError::Unsupported(format!("{what} `{name}` not found")))?;
        expr.ptr()
            .ok_or_else(|| LoweringError::Unsupported(format!("{what} `{name}` is not a pointer")))
    }

    /// Looks up a named `dsp*` field in the backend struct layout contract.
    fn struct_field(&self, name: &str) -> Result<&StructFieldLayout, LoweringError> {
        self.struct_layout.field(name).ok_or_else(|| {
            LoweringError::Unsupported(format!(
                "struct field `{name}` not present in Cranelift dsp* layout contract"
            ))
        })
    }

    /// Coerces a CLIF value to the CLIF type expected by a target FIR type.
    ///
    /// This is a small, explicit conversion set used by current subset lowering
    /// (not a general FIR coercion engine). Unsupported conversions are surfaced
    /// as subset-lowering failures.
    ///
    /// Numeric FIR `Cast` nodes are inserted upstream by signal/FIR preparation,
    /// for example when integer expressions feed float arithmetic or when float
    /// indices are truncated before table access. The subset matcher already
    /// accepts those `Cast` nodes, so the backend must lower the same signed
    /// int/real coercions explicitly to avoid matcher/lowerer drift.
    fn coerce_value_to_fir_type(
        &mut self,
        value: Value,
        target: &FirType,
    ) -> Result<Value, LoweringError> {
        let src_ty = self.fb.func.dfg.value_type(value);
        let dst_ty = self.fir_type_to_clif(target)?;
        if src_ty == dst_ty {
            return Ok(value);
        }
        match (src_ty, dst_ty) {
            (types::F32, types::F64) => Ok(self.fb.ins().fpromote(types::F64, value)),
            (types::F64, types::F32) => Ok(self.fb.ins().fdemote(types::F32, value)),
            (types::I8, types::I32) => Ok(self.fb.ins().uextend(types::I32, value)),
            (types::I8, types::I64) => Ok(self.fb.ins().uextend(types::I64, value)),
            // Bool (i8) → float: widen to i32 first, then convert to float.
            (types::I8, types::F32) => {
                let wide = self.fb.ins().uextend(types::I32, value);
                Ok(self.fb.ins().fcvt_from_sint(types::F32, wide))
            }
            (types::I8, types::F64) => {
                let wide = self.fb.ins().uextend(types::I32, value);
                Ok(self.fb.ins().fcvt_from_sint(types::F64, wide))
            }
            (types::I32, types::F32) | (types::I64, types::F32) => {
                Ok(self.fb.ins().fcvt_from_sint(types::F32, value))
            }
            (types::I32, types::F64) | (types::I64, types::F64) => {
                Ok(self.fb.ins().fcvt_from_sint(types::F64, value))
            }
            (types::F32, types::I32) | (types::F64, types::I32) => {
                // Use the saturating variant so that NaN → 0 and out-of-range
                // floats saturate to INT_MIN/INT_MAX instead of trapping.
                // This matches C/C++ cast semantics and the interpreter backend.
                Ok(self.fb.ins().fcvt_to_sint_sat(types::I32, value))
            }
            (types::F32, types::I64) | (types::F64, types::I64) => {
                Ok(self.fb.ins().fcvt_to_sint_sat(types::I64, value))
            }
            _ => Err(LoweringError::Unsupported(format!(
                "unsupported Cranelift coercion {src_ty} -> {dst_ty} for FIR target {target:?}"
            ))),
        }
    }

    /// Produces a conservative default value for uninitialized local variables.
    ///
    /// Current policy:
    /// - scalars => zero
    /// - pointers/object-like refs => null
    ///
    /// This supports FIR patterns where `DeclareVar { access: Stack|Loop, init: None }`
    /// appears in `compute` and is assigned before use.
    fn default_lowered_value_for_type(
        &mut self,
        typ: &FirType,
    ) -> Result<LoweredExpr, LoweringError> {
        let typ = self.canon_real(typ);
        match &typ {
            FirType::Int32 => Ok(LoweredExpr::Scalar(self.fb.ins().iconst(types::I32, 0))),
            FirType::Int64 => Ok(LoweredExpr::Scalar(self.fb.ins().iconst(types::I64, 0))),
            FirType::Bool => Ok(LoweredExpr::Scalar(self.fb.ins().iconst(types::I8, 0))),
            FirType::Float32 | FirType::FaustFloat => {
                Ok(LoweredExpr::Scalar(self.fb.ins().f32const(0.0)))
            }
            FirType::Float64 => Ok(LoweredExpr::Scalar(self.fb.ins().f64const(0.0))),
            FirType::Ptr(inner) => Ok(LoweredExpr::Ptr {
                value: self.fb.ins().iconst(self.ptr_ty, 0),
                pointee: Some(FirTypeRef::from_fir_type(inner)),
            }),
            FirType::Array(inner, len) if *len > 0 => {
                let elem_ty = self.fir_type_to_clif(inner)?;
                let elem_size = elem_ty.bytes();
                let len = u32::try_from(*len).map_err(|_| {
                    LoweringError::Unsupported(format!(
                        "Cranelift local array length does not fit in u32: {len}"
                    ))
                })?;
                let size = elem_size.checked_mul(len).ok_or_else(|| {
                    LoweringError::Unsupported(
                        "Cranelift local array stack-slot size overflow".to_string(),
                    )
                })?;
                let align_shift = match elem_size {
                    8 => 3,
                    4 => 2,
                    2 => 1,
                    _ => 0,
                };
                let slot = self.fb.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    size,
                    align_shift,
                ));
                let value = self.fb.ins().stack_addr(self.ptr_ty, slot, 0);
                Ok(LoweredExpr::Ptr {
                    value,
                    pointee: Some(FirTypeRef::from_fir_type(inner)),
                })
            }
            FirType::Obj | FirType::UI | FirType::Meta | FirType::Sound => Ok(LoweredExpr::Ptr {
                value: self.fb.ins().iconst(self.ptr_ty, 0),
                pointee: None,
            }),
            other => Err(LoweringError::Unsupported(format!(
                "unsupported default stack init type in Cranelift subset lowering: {other:?}"
            ))),
        }
    }

    /// Lowers a FIR statement node in the current subset.
    ///
    /// This is the central dispatcher for statement lowering. Unsupported
    /// variants return `LoweringError::Unsupported`, which callers may route to
    /// the stub fallback policy.
    fn lower_stmt(&mut self, id: FirId) -> Result<(), LoweringError> {
        match match_fir(self.store, id) {
            FirMatch::Block(items) => {
                for item in items {
                    self.lower_stmt(item)?;
                }
                Ok(())
            }
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Stack,
                init: Some(init),
            }
            | FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Loop,
                init: Some(init),
            } => {
                let init_v = self.lower_expr(init, Some(&typ))?;
                let stored = match (&typ, init_v) {
                    (FirType::Ptr(inner), LoweredExpr::Ptr { value, .. }) => LoweredExpr::Ptr {
                        value,
                        pointee: Some(FirTypeRef::from_fir_type(inner)),
                    },
                    (_, other) => other,
                };
                let clif_ty = self.local_clif_type(&typ);
                let _ = self.bind_ssa_local(&name, stored, clif_ty)?;
                Ok(())
            }
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Stack,
                init: None,
            }
            | FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Loop,
                init: None,
            } => {
                let init_v = self.default_lowered_value_for_type(&typ)?;
                let clif_ty = self.local_clif_type(&typ);
                let _ = self.bind_ssa_local(&name, init_v, clif_ty)?;
                Ok(())
            }
            FirMatch::Label(_) => Ok(()),
            FirMatch::SimpleForLoop {
                var,
                upper,
                body,
                is_reverse,
            } => self.lower_simple_for(var, upper, body, is_reverse),
            FirMatch::StoreTable {
                name,
                access: AccessType::Stack,
                index,
                value,
            } => self.lower_store_table_stack(&name, index, value),
            FirMatch::StoreTable {
                name,
                access: AccessType::Struct,
                index,
                value,
            } => self.lower_store_table_struct(&name, index, value),
            FirMatch::StoreVar {
                name,
                access: AccessType::Struct,
                value,
            } => self.lower_store_var_struct(&name, value),
            FirMatch::StoreVar {
                name,
                access: AccessType::Stack | AccessType::Loop,
                value,
            } => self.lower_store_var_local(&name, value),
            FirMatch::ShiftArrayVar {
                name,
                access: AccessType::Struct,
                delay,
            } => self.lower_shift_array_var_struct(&name, delay),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => self.lower_if_stmt(cond, then_block, else_block),
            FirMatch::Control { cond, stmt } => self.lower_control_stmt(cond, stmt),
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => self.lower_switch_stmt(cond, &cases, default),
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => self.lower_for_loop(var, init, end, step, body, is_reverse),
            FirMatch::WhileLoop { cond, body } => self.lower_while_loop(cond, body),
            FirMatch::ShiftArrayVar { .. } => Err(LoweringError::Unsupported(
                "ShiftArrayVar is currently only supported for AccessType::Struct tables"
                    .to_string(),
            )),
            FirMatch::Drop(value) => {
                if is_obviously_side_effect_free_value(self.store, value) {
                    return Ok(());
                }
                let _ = self.lower_expr(value, None)?;
                Ok(())
            }
            FirMatch::NullStatement => Ok(()),
            FirMatch::Return(None) => {
                emit_return_stub(self.fb);
                Ok(())
            }
            other => Err(LoweringError::Unsupported(format!(
                "unsupported FIR statement in Cranelift subset lowering: {other:?}"
            ))),
        }
    }

    /// Lowers `SimpleForLoop` (`for i in 0..upper`) in forward or reverse direction.
    fn lower_simple_for(
        &mut self,
        var: String,
        upper: FirId,
        body: FirId,
        is_reverse: bool,
    ) -> Result<(), LoweringError> {
        let upper_v = self.lower_expr(upper, Some(&FirType::Int32))?.value();
        let zero = self.fb.ins().iconst(types::I32, 0);
        let one = self.fb.ins().iconst(types::I32, 1);
        let init = if is_reverse {
            self.fb.ins().isub(upper_v, one)
        } else {
            zero
        };

        let header = self.fb.create_block();
        let body_block = self.fb.create_block();
        let exit = self.fb.create_block();
        self.fb.append_block_param(header, types::I32);
        self.fb.ins().jump(header, &[init]);

        self.fb.switch_to_block(header);
        let i_val = self.fb.block_params(header)[0];
        let cond = if is_reverse {
            self.fb
                .ins()
                .icmp(IntCC::SignedGreaterThanOrEqual, i_val, zero)
        } else {
            self.fb.ins().icmp(IntCC::SignedLessThan, i_val, upper_v)
        };
        self.fb.ins().brif(cond, body_block, &[], exit, &[]);

        self.fb.switch_to_block(body_block);
        let prev = self.bind_ssa_local(&var, LoweredExpr::Scalar(i_val), types::I32)?;
        self.lower_stmt(body)?;
        if !is_return_terminated(self.fb) {
            let next = if is_reverse {
                self.fb.ins().isub(i_val, one)
            } else {
                self.fb.ins().iadd(i_val, one)
            };
            self.fb.ins().jump(header, &[next]);
        }
        if let Some(old) = prev {
            self.vars.insert(var, old);
        } else {
            self.vars.remove(&var);
        }
        self.fb.seal_block(body_block);
        self.fb.seal_block(header);

        self.fb.switch_to_block(exit);
        self.fb.seal_block(exit);
        Ok(())
    }

    /// Lowers `StoreTable` for stack pointer aliases (for example `output0[i] = ...`).
    ///
    /// The alias must resolve to a pointer-valued local in `vars`.
    fn lower_store_table_stack(
        &mut self,
        name: &str,
        index: FirId,
        value: FirId,
    ) -> Result<(), LoweringError> {
        let alias = self.read_local(name).ok_or_else(|| {
            LoweringError::Unsupported(format!("stack pointer alias `{name}` not found"))
        })?;
        let base_ptr = alias.ptr().ok_or_else(|| {
            LoweringError::Unsupported(format!("stack alias `{name}` is not a pointer"))
        })?;
        let index_v = self.lower_expr(index, Some(&FirType::Int32))?.value();
        let mut value_v = self.lower_expr(value, None)?.value();
        let elem_ty = if let Some(pointee) = alias.pointee() {
            let fir_target = match pointee {
                FirTypeRef::FaustFloat => FirType::FaustFloat,
                FirTypeRef::Float32 => FirType::Float32,
                FirTypeRef::Float64 => FirType::Float64,
                FirTypeRef::Int32 => FirType::Int32,
                FirTypeRef::Int64 => FirType::Int64,
                FirTypeRef::Bool => FirType::Bool,
                _ => {
                    return Err(LoweringError::Unsupported(format!(
                        "unsupported stack pointer alias pointee for store_table `{name}`: {pointee:?}"
                    )));
                }
            };
            value_v = self.coerce_value_to_fir_type(value_v, &fir_target)?;
            self.fb.func.dfg.value_type(value_v)
        } else {
            self.fb.func.dfg.value_type(value_v)
        };
        let elem_size = i64::from(elem_ty.bytes());
        let addr = self.indexed_addr(base_ptr, index_v, elem_size);
        self.fb.ins().store(MemFlags::new(), value_v, addr, 0);
        Ok(())
    }

    /// Lowers `StoreVar` into a scalar field of the backend `dsp*` struct.
    fn lower_store_var_struct(&mut self, name: &str, value: FirId) -> Result<(), LoweringError> {
        let field = self.struct_field(name)?.clone();
        let scalar_ty = match &field.kind {
            StructFieldKind::Scalar(typ) => typ.clone(),
            StructFieldKind::Table { .. } => {
                return Err(LoweringError::Unsupported(format!(
                    "`StoreVar` cannot target table field `{name}`"
                )));
            }
        };
        let dsp = self.dsp_base_ptr()?;
        let addr = self.fb.ins().iadd_imm(dsp, i64::from(field.offset_bytes));
        let mut value_v = self.lower_expr(value, Some(&scalar_ty))?.value();
        value_v = self.coerce_value_to_fir_type(value_v, &scalar_ty)?;
        self.fb.ins().store(MemFlags::new(), value_v, addr, 0);
        Ok(())
    }

    /// Lowers `StoreTable` into an inline table field of the backend `dsp*` struct.
    fn lower_store_table_struct(
        &mut self,
        name: &str,
        index: FirId,
        value: FirId,
    ) -> Result<(), LoweringError> {
        let field = self.struct_field(name)?.clone();
        let elem_type = match &field.kind {
            StructFieldKind::Table { elem_type, .. } => elem_type.clone(),
            StructFieldKind::Scalar(_) => {
                return Err(LoweringError::Unsupported(format!(
                    "`StoreTable` cannot target scalar field `{name}`"
                )));
            }
        };
        let dsp = self.dsp_base_ptr()?;
        let base = self.fb.ins().iadd_imm(dsp, i64::from(field.offset_bytes));
        let index_v = self.lower_expr(index, Some(&FirType::Int32))?.value();
        let mut value_v = self.lower_expr(value, Some(&elem_type))?.value();
        value_v = self.coerce_value_to_fir_type(value_v, &elem_type)?;
        let elem_clif = self.fir_type_to_clif(&elem_type)?;
        let addr = self.indexed_addr(base, index_v, i64::from(elem_clif.bytes()));
        self.fb.ins().store(MemFlags::new(), value_v, addr, 0);
        Ok(())
    }

    /// Lowers `ShiftArrayVar` on an inline `Struct` table using a descending loop.
    ///
    /// Semantics (current implementation):
    /// - shifts right by one (`tbl[i] = tbl[i-1]`) for `i = shift_len .. 1`
    /// - clamps the effective shift range to the declared table length
    fn lower_shift_array_var_struct(
        &mut self,
        name: &str,
        delay: i32,
    ) -> Result<(), LoweringError> {
        let field = self.struct_field(name)?.clone();
        let (elem_type, len) = match &field.kind {
            StructFieldKind::Table { elem_type, len } => (elem_type.clone(), *len),
            StructFieldKind::Scalar(_) => {
                return Err(LoweringError::Unsupported(format!(
                    "`ShiftArrayVar` cannot target scalar field `{name}`"
                )));
            }
        };
        let max_shift = delay.max(0) as u32;
        if len <= 1 || max_shift == 0 {
            return Ok(());
        }
        let shift_len = max_shift.min(len.saturating_sub(1));
        if shift_len == 0 {
            return Ok(());
        }

        let dsp = self.dsp_base_ptr()?;
        let base = self.fb.ins().iadd_imm(dsp, i64::from(field.offset_bytes));
        let elem_clif = self.fir_type_to_clif(&elem_type)?;
        let elem_bytes = i64::from(elem_clif.bytes());

        let header = self.fb.create_block();
        let body = self.fb.create_block();
        let exit = self.fb.create_block();
        let init_i = self.fb.ins().iconst(types::I32, i64::from(shift_len));
        self.fb.append_block_param(header, types::I32);
        self.fb.ins().jump(header, &[init_i]);

        self.fb.switch_to_block(header);
        let i_val = self.fb.block_params(header)[0];
        let cond = self.fb.ins().icmp_imm(IntCC::SignedGreaterThan, i_val, 0);
        self.fb.ins().brif(cond, body, &[], exit, &[]);

        self.fb.switch_to_block(body);
        let src_idx = self.fb.ins().iadd_imm(i_val, -1);
        let src_addr = self.indexed_addr(base, src_idx, elem_bytes);
        let dst_addr = self.indexed_addr(base, i_val, elem_bytes);
        let v = self.fb.ins().load(elem_clif, MemFlags::new(), src_addr, 0);
        self.fb.ins().store(MemFlags::new(), v, dst_addr, 0);
        let next = self.fb.ins().iadd_imm(i_val, -1);
        self.fb.ins().jump(header, &[next]);
        self.fb.seal_block(body);
        self.fb.seal_block(header);

        self.fb.switch_to_block(exit);
        self.fb.seal_block(exit);
        Ok(())
    }

    /// Updates a local stack/loop variable in the lowering environment.
    ///
    /// This mutates the name->value mapping only; it does not emit memory
    /// traffic because stack locals in the current subset are modelled as SSA
    /// values/pointers in the lowering environment.
    fn lower_store_var_local(&mut self, name: &str, value: FirId) -> Result<(), LoweringError> {
        let binding = self.vars.get(name).copied().ok_or_else(|| {
            LoweringError::Unsupported(format!("local variable `{name}` not found"))
        })?;
        let LocalBinding::Ssa { var, clif_ty, .. } = binding else {
            return Err(LoweringError::Unsupported(format!(
                "function argument `{name}` is not assignable"
            )));
        };
        let new_value = self.lower_expr(value, None)?.value();
        let new_value = self.coerce_to_local_type(new_value, clif_ty)?;
        self.fb.def_var(var, new_value);
        Ok(())
    }

    /// Lowers FIR `If` with explicit then/else/continuation CLIF blocks.
    fn lower_if_stmt(
        &mut self,
        cond: FirId,
        then_block: FirId,
        else_block: Option<FirId>,
    ) -> Result<(), LoweringError> {
        let cond_v = self.lower_expr(cond, Some(&FirType::Bool))?.value();
        let cond_b1 = self.fb.ins().icmp_imm(IntCC::NotEqual, cond_v, 0);
        let then_b = self.fb.create_block();
        let else_b = self.fb.create_block();
        let cont_b = self.fb.create_block();
        self.fb.ins().brif(cond_b1, then_b, &[], else_b, &[]);

        self.fb.switch_to_block(then_b);
        self.lower_stmt(then_block)?;
        if !is_return_terminated(self.fb) {
            self.fb.ins().jump(cont_b, &[]);
        }
        self.fb.seal_block(then_b);

        self.fb.switch_to_block(else_b);
        if let Some(else_block) = else_block {
            self.lower_stmt(else_block)?;
        }
        if !is_return_terminated(self.fb) {
            self.fb.ins().jump(cont_b, &[]);
        }
        self.fb.seal_block(else_b);

        self.fb.switch_to_block(cont_b);
        self.fb.seal_block(cont_b);
        Ok(())
    }

    /// Lowers FIR `Control` as an `if (cond) stmt`.
    fn lower_control_stmt(&mut self, cond: FirId, stmt: FirId) -> Result<(), LoweringError> {
        self.lower_if_stmt(cond, stmt, None)
    }

    /// Lowers FIR `Switch` as a chain of integer comparisons and branches.
    ///
    /// This is a simple, explicit lowering intended for bring-up and debugging.
    /// It favors clarity and deterministic diagnostics over optimal jump-table
    /// generation (which can be considered later).
    fn lower_switch_stmt(
        &mut self,
        cond: FirId,
        cases: &[(i64, FirId)],
        default: Option<FirId>,
    ) -> Result<(), LoweringError> {
        let cond_v = self.lower_expr(cond, None)?.value();
        let cond_ty = self.fb.func.dfg.value_type(cond_v);
        match cond_ty {
            types::I8 | types::I32 | types::I64 => {}
            other => {
                return Err(LoweringError::Unsupported(format!(
                    "unsupported `Switch` condition CLIF type in subset lowering: {other}"
                )));
            }
        }

        let cont_b = self.fb.create_block();
        let default_b = default.map(|_| self.fb.create_block());
        if cases.is_empty() {
            if let Some(default_stmt) = default {
                self.lower_stmt(default_stmt)?;
            }
            if !is_return_terminated(self.fb) {
                self.fb.ins().jump(cont_b, &[]);
            }
            self.fb.switch_to_block(cont_b);
            self.fb.seal_block(cont_b);
            return Ok(());
        }

        for (idx, (case_value, case_stmt)) in cases.iter().enumerate() {
            let then_b = self.fb.create_block();
            let is_last = idx + 1 == cases.len();
            let next_b = if is_last {
                None
            } else {
                Some(self.fb.create_block())
            };
            let fallthrough_b = next_b.or(default_b).unwrap_or(cont_b);

            let cond_match = self.fb.ins().icmp_imm(IntCC::Equal, cond_v, *case_value);
            self.fb
                .ins()
                .brif(cond_match, then_b, &[], fallthrough_b, &[]);

            self.fb.switch_to_block(then_b);
            self.lower_stmt(*case_stmt)?;
            if !is_return_terminated(self.fb) {
                self.fb.ins().jump(cont_b, &[]);
            }
            self.fb.seal_block(then_b);

            if let Some(next_b) = next_b {
                self.fb.switch_to_block(next_b);
                self.fb.seal_block(next_b);
            }
        }

        if let Some(default_stmt) = default {
            self.fb
                .switch_to_block(default_b.expect("present when default stmt exists"));
            self.lower_stmt(default_stmt)?;
            self.fb
                .seal_block(default_b.expect("present when default stmt exists"));
        }
        if !is_return_terminated(self.fb) {
            self.fb.ins().jump(cont_b, &[]);
        }
        self.fb.switch_to_block(cont_b);
        self.fb.seal_block(cont_b);
        Ok(())
    }

    /// Lowers FIR `ForLoop` with explicit header/body/exit blocks.
    ///
    /// The loop variable is installed in the local environment as a loop-local
    /// scalar and restored/removed after lowering the loop.
    fn lower_for_loop(
        &mut self,
        var: String,
        init: FirId,
        end: FirId,
        step: FirId,
        body: FirId,
        is_reverse: bool,
    ) -> Result<(), LoweringError> {
        // init is a DeclareVar(kLoop) per FIR contract; extract its value.
        let init_inner =
            if let FirMatch::DeclareVar { init: Some(v), .. } = match_fir(self.store, init) {
                v
            } else {
                init
            };
        let init_v = self.lower_expr(init_inner, Some(&FirType::Int32))?.value();
        let header = self.fb.create_block();
        let body_block = self.fb.create_block();
        let exit = self.fb.create_block();
        self.fb.append_block_param(header, types::I32);
        self.fb.ins().jump(header, &[init_v]);

        self.fb.switch_to_block(header);
        let i_val = self.fb.block_params(header)[0];
        let prev = self.bind_ssa_local(&var, LoweredExpr::Scalar(i_val), types::I32)?;
        let end_v = self.lower_expr(end, Some(&FirType::Int32))?.value();
        let cc = if is_reverse {
            IntCC::SignedGreaterThan
        } else {
            IntCC::SignedLessThan
        };
        let cond = self.fb.ins().icmp(cc, i_val, end_v);
        self.fb.ins().brif(cond, body_block, &[], exit, &[]);

        self.fb.switch_to_block(body_block);
        self.lower_stmt(body)?;
        let step_v = self.lower_expr(step, Some(&FirType::Int32))?.value();
        let next = self.fb.ins().iadd(i_val, step_v);
        self.fb.ins().jump(header, &[next]);
        self.fb.seal_block(body_block);
        self.fb.seal_block(header);

        if let Some(old) = prev {
            self.vars.insert(var, old);
        } else {
            self.vars.remove(&var);
        }

        self.fb.switch_to_block(exit);
        self.fb.seal_block(exit);
        Ok(())
    }

    /// Lowers FIR `WhileLoop` with explicit header/body/exit blocks.
    fn lower_while_loop(&mut self, cond: FirId, body: FirId) -> Result<(), LoweringError> {
        let header = self.fb.create_block();
        let body_block = self.fb.create_block();
        let exit = self.fb.create_block();
        self.fb.ins().jump(header, &[]);

        self.fb.switch_to_block(header);
        let cond_v = self.lower_expr(cond, Some(&FirType::Bool))?.value();
        let cond_b1 = self.fb.ins().icmp_imm(IntCC::NotEqual, cond_v, 0);
        self.fb.ins().brif(cond_b1, body_block, &[], exit, &[]);

        self.fb.switch_to_block(body_block);
        self.lower_stmt(body)?;
        if !is_return_terminated(self.fb) {
            self.fb.ins().jump(header, &[]);
        }
        self.fb.seal_block(body_block);
        self.fb.seal_block(header);

        self.fb.switch_to_block(exit);
        self.fb.seal_block(exit);
        Ok(())
    }

    /// Computes an element address `base + index * elem_size`.
    ///
    /// `index_i32` is always an `i32` integer (FIR index convention).  It is
    /// widened to [`Self::ptr_ty`] width before the multiply so that the
    /// arithmetic is consistent with the native pointer size — on 64-bit hosts
    /// this prevents silent 32-bit overflow when indices are large.
    fn indexed_addr(&mut self, base_ptr: Value, index_i32: Value, elem_size: i64) -> Value {
        let idx_ptr = if self.ptr_ty == types::I64 {
            self.fb.ins().uextend(types::I64, index_i32)
        } else {
            self.fb.ins().uextend(types::I32, index_i32)
        };
        let scale = self.fb.ins().iconst(self.ptr_ty, elem_size);
        let offset = self.fb.ins().imul(idx_ptr, scale);
        self.fb.ins().iadd(base_ptr, offset)
    }

    /// Lowers a FIR expression node in the current subset.
    ///
    /// `expected` is a backend-local hint used in a few places to guide type
    /// coercions and pointer/scalar handling; it is not a full FIR typechecker.
    fn lower_expr(
        &mut self,
        id: FirId,
        expected: Option<&FirType>,
    ) -> Result<LoweredExpr, LoweringError> {
        match match_fir(self.store, id) {
            FirMatch::Int32 { value, .. } => Ok(LoweredExpr::Scalar(
                self.fb.ins().iconst(types::I32, i64::from(value)),
            )),
            FirMatch::Bool { value, .. } => Ok(LoweredExpr::Scalar(
                self.fb.ins().iconst(types::I8, if value { 1 } else { 0 }),
            )),
            FirMatch::Float32 { value, .. } => {
                Ok(LoweredExpr::Scalar(self.fb.ins().f32const(value)))
            }
            FirMatch::Float64 { value, .. } => {
                Ok(LoweredExpr::Scalar(self.fb.ins().f64const(value)))
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct,
                typ,
            } => {
                let field = self.struct_field(&name)?.clone();
                let scalar_ty = match &field.kind {
                    StructFieldKind::Scalar(t) => t.clone(),
                    StructFieldKind::Table { .. } => {
                        return Err(LoweringError::Unsupported(format!(
                            "`LoadVar` cannot target table field `{name}`"
                        )));
                    }
                };
                let dsp = self.dsp_base_ptr()?;
                let addr = self.fb.ins().iadd_imm(dsp, i64::from(field.offset_bytes));
                let field_clif_ty = self.fir_type_to_clif(&scalar_ty)?;
                let raw = self.fb.ins().load(field_clif_ty, MemFlags::new(), addr, 0);
                let coerced = self.coerce_value_to_fir_type(raw, &typ)?;
                Ok(LoweredExpr::Scalar(coerced))
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::Global,
                typ,
            } => {
                let data_id = self.extern_data_ids.get(&name).copied().ok_or_else(|| {
                    LoweringError::Unsupported(format!(
                        "external data symbol `{name}` not found in Cranelift options"
                    ))
                })?;
                let gv = self.jit.declare_data_in_func(data_id, self.fb.func);
                let addr = self.fb.ins().global_value(self.ptr_ty, gv);
                let elem_clif = self.fir_type_to_clif(&typ)?;
                let raw = self.fb.ins().load(elem_clif, MemFlags::new(), addr, 0);
                let coerced = self.coerce_value_to_fir_type(raw, &typ)?;
                Ok(LoweredExpr::Scalar(coerced))
            }
            FirMatch::LoadVar { name, .. } => self.read_local(&name).ok_or_else(|| {
                LoweringError::Unsupported(format!("load of unknown variable `{name}`"))
            }),
            FirMatch::LoadTable {
                name,
                access: AccessType::Stack,
                index,
                typ,
            } => {
                let alias = self.read_local(&name).ok_or_else(|| {
                    LoweringError::Unsupported(format!("stack table base `{name}` not found"))
                })?;
                let base_ptr = alias.ptr().ok_or_else(|| {
                    LoweringError::Unsupported(format!(
                        "stack table base `{name}` is not a pointer alias"
                    ))
                })?;
                let index_v = self.lower_expr(index, Some(&FirType::Int32))?.value();
                let elem_fir_ty = match alias.pointee() {
                    Some(FirTypeRef::FaustFloat) => FirType::FaustFloat,
                    Some(FirTypeRef::Float32) => FirType::Float32,
                    Some(FirTypeRef::Float64) => FirType::Float64,
                    Some(FirTypeRef::Int32) => FirType::Int32,
                    Some(FirTypeRef::Int64) => FirType::Int64,
                    Some(FirTypeRef::Bool) => FirType::Bool,
                    Some(other) => {
                        return Err(LoweringError::Unsupported(format!(
                            "unsupported stack pointer alias pointee for load_table `{name}`: {other:?}"
                        )));
                    }
                    None => typ.clone(),
                };
                let elem_clif = self.fir_type_to_clif(&elem_fir_ty)?;
                let addr = self.indexed_addr(base_ptr, index_v, i64::from(elem_clif.bytes()));
                let raw = self.fb.ins().load(elem_clif, MemFlags::new(), addr, 0);
                let coerced = self.coerce_value_to_fir_type(raw, &typ)?;
                Ok(LoweredExpr::Scalar(coerced))
            }
            FirMatch::LoadTable {
                name,
                access: AccessType::FunArgs,
                index,
                typ,
            } => {
                let base_ptr = self.read_local_ptr(&name, "function-arg table base")?;
                let index_v = self.lower_expr(index, Some(&FirType::Int32))?.value();
                let elem_ty = self.fir_type_to_clif(&typ)?;
                let addr = self.indexed_addr(base_ptr, index_v, i64::from(self.ptr_ty.bytes()));
                let loaded = self.fb.ins().load(elem_ty, MemFlags::new(), addr, 0);
                let pointee = match &typ {
                    FirType::Ptr(inner) => Some(FirTypeRef::from_fir_type(inner)),
                    _ => None,
                };
                Ok(LoweredExpr::Ptr {
                    value: loaded,
                    pointee,
                })
            }
            FirMatch::LoadTable {
                name,
                access: AccessType::Struct,
                index,
                typ,
            } => {
                let field = self.struct_field(&name)?.clone();
                let elem_type = match &field.kind {
                    StructFieldKind::Table { elem_type, .. } => elem_type.clone(),
                    StructFieldKind::Scalar(_) => {
                        return Err(LoweringError::Unsupported(format!(
                            "`LoadTable` cannot target scalar field `{name}`"
                        )));
                    }
                };
                let dsp = self.dsp_base_ptr()?;
                let base = self.fb.ins().iadd_imm(dsp, i64::from(field.offset_bytes));
                let index_v = self.lower_expr(index, Some(&FirType::Int32))?.value();
                let elem_clif = self.fir_type_to_clif(&elem_type)?;
                let addr = self.indexed_addr(base, index_v, i64::from(elem_clif.bytes()));
                let raw = self.fb.ins().load(elem_clif, MemFlags::new(), addr, 0);
                let coerced = self.coerce_value_to_fir_type(raw, &typ)?;
                Ok(LoweredExpr::Scalar(coerced))
            }
            FirMatch::LoadTable {
                name,
                access: AccessType::Static,
                index,
                typ,
            } => {
                let data_id = self.static_data_ids.get(&name).copied().ok_or_else(|| {
                    LoweringError::Unsupported(format!(
                        "static table `{name}` not found in pre-declared JIT data"
                    ))
                })?;
                let gv = self.jit.declare_data_in_func(data_id, self.fb.func);
                let base = self.fb.ins().global_value(self.ptr_ty, gv);
                let index_v = self.lower_expr(index, Some(&FirType::Int32))?.value();
                let elem_clif = self.fir_type_to_clif(&typ)?;
                let addr = self.indexed_addr(base, index_v, i64::from(elem_clif.bytes()));
                let raw = self.fb.ins().load(elem_clif, MemFlags::new(), addr, 0);
                let coerced = self.coerce_value_to_fir_type(raw, &typ)?;
                Ok(LoweredExpr::Scalar(coerced))
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                typ,
            } => {
                let cond_v = self.lower_expr(cond, Some(&FirType::Bool))?.value();
                let then_v = self.lower_expr(then_value, Some(&typ))?.value();
                let then_v = self.coerce_value_to_fir_type(then_v, &typ)?;
                let else_v = self.lower_expr(else_value, Some(&typ))?.value();
                let else_v = self.coerce_value_to_fir_type(else_v, &typ)?;
                let bool_cond = self.fb.ins().icmp_imm(IntCC::NotEqual, cond_v, 0);
                let out = self.fb.ins().select(bool_cond, then_v, else_v);
                Ok(LoweredExpr::Scalar(out))
            }
            FirMatch::Neg { value, typ } => {
                let v = self.lower_expr(value, Some(&typ))?.value();
                let v = self.coerce_value_to_fir_type(v, &typ)?;
                let out = match typ {
                    FirType::Int32 | FirType::Int64 => self.fb.ins().ineg(v),
                    FirType::Float32 | FirType::Float64 | FirType::FaustFloat => {
                        self.fb.ins().fneg(v)
                    }
                    _ => {
                        return Err(LoweringError::Unsupported(format!(
                            "unsupported negation type in subset lowering: {typ:?}"
                        )));
                    }
                };
                Ok(LoweredExpr::Scalar(out))
            }
            FirMatch::Cast { typ, value } => {
                let src = self.lower_expr(value, None)?.value();
                let out = self.coerce_value_to_fir_type(src, &typ)?;
                Ok(LoweredExpr::Scalar(out))
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => self.lower_binop(op, lhs, rhs, &typ),
            FirMatch::FunCall { name, args, typ } => self.lower_fun_call(&name, &args, &typ),

            // ── Soundfile field loads ─────────────────────────────────────────
            //
            // The C++ Soundfile struct layout (packed, host-allocated):
            //   offset  0  void*  fBuffers   (float** or double**)
            //   offset  8  int*   fLength    [MAX_SOUNDFILE_PARTS]
            //   offset 16  int*   fSR        [MAX_SOUNDFILE_PARTS]
            //   offset 24  int*   fOffset    [MAX_SOUNDFILE_PARTS]
            //   offset 32  int    fChannels
            //   offset 36  int    fParts
            //   offset 40  bool   fIsDouble
            //
            // `dsp->fSoundN` is a pointer-sized field in the DSP struct that
            // holds the Soundfile* written by `SoundUI::addSoundfile`.
            FirMatch::LoadSoundfileLength { var, part } => {
                self.lower_load_soundfile_length(&var, part)
            }
            FirMatch::LoadSoundfileRate { var, part } => self.lower_load_soundfile_rate(&var, part),
            FirMatch::LoadSoundfileBuffer {
                var,
                chan,
                part,
                idx,
                typ,
            } => self.lower_load_soundfile_buffer(&var, chan, part, idx, &typ),

            other => Err(LoweringError::Unsupported(format!(
                "unsupported FIR expression in Cranelift subset lowering: {other:?}; expected={expected:?}"
            ))),
        }
    }

    /// Lowers FIR `BinOp` arithmetic/comparisons to CLIF instructions.
    ///
    /// Comparisons return FIR-style booleans (`i8` 0/1), not CLIF `b1`.
    fn lower_binop(
        &mut self,
        op: FirBinOp,
        lhs: FirId,
        rhs: FirId,
        typ: &FirType,
    ) -> Result<LoweredExpr, LoweringError> {
        // Resolve `FaustFloat` to its concrete real width so type-keyed op
        // selection below uses f32/f64 ops consistently under `-double`.
        let canon = self.canon_real(typ);
        let typ = &canon;
        let l = self.lower_expr(lhs, Some(typ))?.value();
        let r = self.lower_expr(rhs, Some(typ))?.value();
        let lty = self.fb.func.dfg.value_type(l);
        // Comparison ops must dispatch on the actual CLIF operand type, not the
        // FIR result type.  A `BinOp { op: Lt, lhs: f32, rhs: f32, typ: Int32 }`
        // (float comparison yielding an integer-boolean result) must use
        // `float_cmp_to_i8`, not `int_cmp_to_i8` on truncated operands.
        let is_cmp_op = matches!(
            op,
            FirBinOp::Eq | FirBinOp::Ne | FirBinOp::Lt | FirBinOp::Le | FirBinOp::Gt | FirBinOp::Ge
        );
        if matches!(typ, FirType::Bool) || (is_cmp_op && lty.is_float()) {
            let out = if lty.is_int() {
                match op {
                    FirBinOp::Eq => self.int_cmp_to_i8(IntCC::Equal, l, r),
                    FirBinOp::Ne => self.int_cmp_to_i8(IntCC::NotEqual, l, r),
                    FirBinOp::Lt => self.int_cmp_to_i8(IntCC::SignedLessThan, l, r),
                    FirBinOp::Le => self.int_cmp_to_i8(IntCC::SignedLessThanOrEqual, l, r),
                    FirBinOp::Gt => self.int_cmp_to_i8(IntCC::SignedGreaterThan, l, r),
                    FirBinOp::Ge => self.int_cmp_to_i8(IntCC::SignedGreaterThanOrEqual, l, r),
                    _ => {
                        return Err(LoweringError::Unsupported(format!(
                            "unsupported bool-result int comparison in subset lowering: {op:?}"
                        )));
                    }
                }
            } else if lty.is_float() {
                match op {
                    FirBinOp::Eq => {
                        self.float_cmp_to_i8(cranelift_codegen::ir::condcodes::FloatCC::Equal, l, r)
                    }
                    FirBinOp::Ne => self.float_cmp_to_i8(
                        cranelift_codegen::ir::condcodes::FloatCC::NotEqual,
                        l,
                        r,
                    ),
                    FirBinOp::Lt => self.float_cmp_to_i8(
                        cranelift_codegen::ir::condcodes::FloatCC::LessThan,
                        l,
                        r,
                    ),
                    FirBinOp::Le => self.float_cmp_to_i8(
                        cranelift_codegen::ir::condcodes::FloatCC::LessThanOrEqual,
                        l,
                        r,
                    ),
                    FirBinOp::Gt => self.float_cmp_to_i8(
                        cranelift_codegen::ir::condcodes::FloatCC::GreaterThan,
                        l,
                        r,
                    ),
                    FirBinOp::Ge => self.float_cmp_to_i8(
                        cranelift_codegen::ir::condcodes::FloatCC::GreaterThanOrEqual,
                        l,
                        r,
                    ),
                    _ => {
                        return Err(LoweringError::Unsupported(format!(
                            "unsupported bool-result float comparison in subset lowering: {op:?}"
                        )));
                    }
                }
            } else {
                return Err(LoweringError::Unsupported(format!(
                    "unsupported comparison operand CLIF type in subset lowering: {lty}"
                )));
            };
            return Ok(LoweredExpr::Scalar(out));
        }
        let l = self.coerce_value_to_fir_type(l, typ)?;
        let r = self.coerce_value_to_fir_type(r, typ)?;
        let out = match typ {
            FirType::Int32 => match op {
                FirBinOp::Add => self.fb.ins().iadd(l, r),
                FirBinOp::Sub => self.fb.ins().isub(l, r),
                FirBinOp::Mul => self.fb.ins().imul(l, r),
                FirBinOp::Div => self.fb.ins().sdiv(l, r),
                FirBinOp::Rem => self.fb.ins().srem(l, r),
                FirBinOp::And => self.fb.ins().band(l, r),
                FirBinOp::Or => self.fb.ins().bor(l, r),
                FirBinOp::Xor => self.fb.ins().bxor(l, r),
                FirBinOp::Lsh => self.fb.ins().ishl(l, r),
                FirBinOp::ARsh => self.fb.ins().sshr(l, r),
                FirBinOp::LRsh => self.fb.ins().ushr(l, r),
                FirBinOp::Eq => self.int_cmp_to_i8(IntCC::Equal, l, r),
                FirBinOp::Ne => self.int_cmp_to_i8(IntCC::NotEqual, l, r),
                FirBinOp::Lt => self.int_cmp_to_i8(IntCC::SignedLessThan, l, r),
                FirBinOp::Le => self.int_cmp_to_i8(IntCC::SignedLessThanOrEqual, l, r),
                FirBinOp::Gt => self.int_cmp_to_i8(IntCC::SignedGreaterThan, l, r),
                FirBinOp::Ge => self.int_cmp_to_i8(IntCC::SignedGreaterThanOrEqual, l, r),
            },
            FirType::Float32 | FirType::FaustFloat => match op {
                FirBinOp::Add => self.fb.ins().fadd(l, r),
                FirBinOp::Sub => self.fb.ins().fsub(l, r),
                FirBinOp::Mul => self.fb.ins().fmul(l, r),
                FirBinOp::Div => self.fb.ins().fdiv(l, r),
                FirBinOp::Eq => {
                    self.float_cmp_to_i8(cranelift_codegen::ir::condcodes::FloatCC::Equal, l, r)
                }
                FirBinOp::Ne => {
                    self.float_cmp_to_i8(cranelift_codegen::ir::condcodes::FloatCC::NotEqual, l, r)
                }
                FirBinOp::Lt => {
                    self.float_cmp_to_i8(cranelift_codegen::ir::condcodes::FloatCC::LessThan, l, r)
                }
                FirBinOp::Le => self.float_cmp_to_i8(
                    cranelift_codegen::ir::condcodes::FloatCC::LessThanOrEqual,
                    l,
                    r,
                ),
                FirBinOp::Gt => self.float_cmp_to_i8(
                    cranelift_codegen::ir::condcodes::FloatCC::GreaterThan,
                    l,
                    r,
                ),
                FirBinOp::Ge => self.float_cmp_to_i8(
                    cranelift_codegen::ir::condcodes::FloatCC::GreaterThanOrEqual,
                    l,
                    r,
                ),
                _ => {
                    return Err(LoweringError::Unsupported(format!(
                        "unsupported float32/faustfloat binop in subset lowering: {op:?}"
                    )));
                }
            },
            FirType::Float64 => match op {
                FirBinOp::Add => self.fb.ins().fadd(l, r),
                FirBinOp::Sub => self.fb.ins().fsub(l, r),
                FirBinOp::Mul => self.fb.ins().fmul(l, r),
                FirBinOp::Div => self.fb.ins().fdiv(l, r),
                FirBinOp::Eq => {
                    self.float_cmp_to_i8(cranelift_codegen::ir::condcodes::FloatCC::Equal, l, r)
                }
                FirBinOp::Ne => {
                    self.float_cmp_to_i8(cranelift_codegen::ir::condcodes::FloatCC::NotEqual, l, r)
                }
                FirBinOp::Lt => {
                    self.float_cmp_to_i8(cranelift_codegen::ir::condcodes::FloatCC::LessThan, l, r)
                }
                FirBinOp::Le => self.float_cmp_to_i8(
                    cranelift_codegen::ir::condcodes::FloatCC::LessThanOrEqual,
                    l,
                    r,
                ),
                FirBinOp::Gt => self.float_cmp_to_i8(
                    cranelift_codegen::ir::condcodes::FloatCC::GreaterThan,
                    l,
                    r,
                ),
                FirBinOp::Ge => self.float_cmp_to_i8(
                    cranelift_codegen::ir::condcodes::FloatCC::GreaterThanOrEqual,
                    l,
                    r,
                ),
                _ => {
                    return Err(LoweringError::Unsupported(format!(
                        "unsupported Float64 binop in subset lowering: {op:?}"
                    )));
                }
            },
            _ => {
                return Err(LoweringError::Unsupported(format!(
                    "unsupported binop result type in subset lowering: {typ:?}"
                )));
            }
        };
        Ok(LoweredExpr::Scalar(out))
    }

    /// Lowers FIR math calls to imported host functions (`sinf`, `pow`, ...).
    ///
    /// Supported operations are determined by the `*_math_symbol_*` helpers and
    /// are intentionally explicit to keep subset coverage auditable.
    fn lower_fun_call(
        &mut self,
        name: &str,
        args: &[FirId],
        typ: &FirType,
    ) -> Result<LoweredExpr, LoweringError> {
        // Resolve `FaustFloat` to its concrete real width so the f32/f64 math
        // arms below are selected consistently under `-double`.
        let canon = self.canon_real(typ);
        let typ = &canon;
        match (name, typ, args) {
            ("abs", FirType::Int32, [x]) => {
                let xv = self.lower_expr(*x, Some(typ))?.value();
                let xv = self.coerce_value_to_fir_type(xv, typ)?;
                let fref = self.ensure_import("abs", &[types::I32], types::I32)?;
                let call = self.fb.ins().call(fref, &[xv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            ("min_i", FirType::Int32, [x, y]) | ("max_i", FirType::Int32, [x, y]) => {
                let xv = self.lower_expr(*x, Some(typ))?.value();
                let xv = self.coerce_value_to_fir_type(xv, typ)?;
                let yv = self.lower_expr(*y, Some(typ))?.value();
                let yv = self.coerce_value_to_fir_type(yv, typ)?;
                let fref = self.ensure_import(name, &[types::I32, types::I32], types::I32)?;
                let call = self.fb.ins().call(fref, &[xv, yv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            ("isnanf", FirType::Int32, [x]) => {
                let xv = self.lower_expr(*x, Some(&FirType::Float32))?.value();
                let xv = self.coerce_value_to_fir_type(xv, &FirType::Float32)?;
                let fref = self.ensure_import("isnanf", &[types::F32], types::I32)?;
                let call = self.fb.ins().call(fref, &[xv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            ("isnan", FirType::Int32, [x]) => {
                let xv = self.lower_expr(*x, Some(&FirType::Float64))?.value();
                let xv = self.coerce_value_to_fir_type(xv, &FirType::Float64)?;
                let fref = self.ensure_import("isnan", &[types::F64], types::I32)?;
                let call = self.fb.ins().call(fref, &[xv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            ("isinff", FirType::Int32, [x]) => {
                let xv = self.lower_expr(*x, Some(&FirType::Float32))?.value();
                let xv = self.coerce_value_to_fir_type(xv, &FirType::Float32)?;
                let fref = self.ensure_import("isinff", &[types::F32], types::I32)?;
                let call = self.fb.ins().call(fref, &[xv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            ("isinf", FirType::Int32, [x]) => {
                let xv = self.lower_expr(*x, Some(&FirType::Float64))?.value();
                let xv = self.coerce_value_to_fir_type(xv, &FirType::Float64)?;
                let fref = self.ensure_import("isinf", &[types::F64], types::I32)?;
                let call = self.fb.ins().call(fref, &[xv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            ("copysignf", FirType::FaustFloat | FirType::Float32, [x, y]) => {
                let mut xv = self.lower_expr(*x, Some(&FirType::Float32))?.value();
                let mut yv = self.lower_expr(*y, Some(&FirType::Float32))?.value();
                xv = self.coerce_value_to_fir_type(xv, &FirType::Float32)?;
                yv = self.coerce_value_to_fir_type(yv, &FirType::Float32)?;
                let fref =
                    self.ensure_import("copysignf", &[types::F32, types::F32], types::F32)?;
                let call = self.fb.ins().call(fref, &[xv, yv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            ("copysign", FirType::Float64, [x, y]) => {
                let mut xv = self.lower_expr(*x, Some(&FirType::Float64))?.value();
                let mut yv = self.lower_expr(*y, Some(&FirType::Float64))?.value();
                xv = self.coerce_value_to_fir_type(xv, &FirType::Float64)?;
                yv = self.coerce_value_to_fir_type(yv, &FirType::Float64)?;
                let fref = self.ensure_import("copysign", &[types::F64, types::F64], types::F64)?;
                let call = self.fb.ins().call(fref, &[xv, yv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            (
                "acoshf" | "asinhf" | "atanhf" | "coshf" | "sinhf" | "tanhf",
                FirType::FaustFloat | FirType::Float32,
                [x],
            ) => {
                let xv = self.lower_expr(*x, Some(&FirType::Float32))?.value();
                let xv = self.coerce_value_to_fir_type(xv, &FirType::Float32)?;
                let fref = self.ensure_import(name, &[types::F32], types::F32)?;
                let call = self.fb.ins().call(fref, &[xv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            ("acosh" | "asinh" | "atanh" | "cosh" | "sinh" | "tanh", FirType::Float64, [x]) => {
                let xv = self.lower_expr(*x, Some(&FirType::Float64))?.value();
                let xv = self.coerce_value_to_fir_type(xv, &FirType::Float64)?;
                let fref = self.ensure_import(name, &[types::F64], types::F64)?;
                let call = self.fb.ins().call(fref, &[xv]);
                return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
            }
            _ => {}
        }
        if self.extern_function_symbols.contains_key(name) {
            let mut lowered_args = Vec::with_capacity(args.len());
            let mut param_types = Vec::with_capacity(args.len());
            for arg in args {
                let value = self.lower_expr(*arg, None)?.value();
                param_types.push(self.fb.func.dfg.value_type(value));
                lowered_args.push(value);
            }
            let ret = self.fir_type_to_clif(typ)?;
            let fref = self.ensure_import(name, &param_types, ret)?;
            let call = self.fb.ins().call(fref, &lowered_args);
            return Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]));
        }
        let math = fir::FirMathOp::from_symbol(name).ok_or_else(|| {
            LoweringError::Unsupported(format!("unsupported function call `{name}`"))
        })?;
        match (math, typ, args) {
            (math, FirType::FaustFloat | FirType::Float32, [x])
                if unary_math_symbol_f32(math).is_some() =>
            {
                let mut xv = self.lower_expr(*x, Some(typ))?.value();
                xv = self.coerce_value_to_fir_type(xv, typ)?;
                let fref = self.ensure_unary_import(
                    unary_math_symbol_f32(math).expect("guarded by is_some"),
                    types::F32,
                )?;
                let call = self.fb.ins().call(fref, &[xv]);
                Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]))
            }
            (math, FirType::Float64, [x]) if unary_math_symbol_f64(math).is_some() => {
                let mut xv = self.lower_expr(*x, Some(typ))?.value();
                xv = self.coerce_value_to_fir_type(xv, typ)?;
                let fref = self.ensure_unary_import(
                    unary_math_symbol_f64(math).expect("guarded by is_some"),
                    types::F64,
                )?;
                let call = self.fb.ins().call(fref, &[xv]);
                Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]))
            }
            (math, FirType::FaustFloat | FirType::Float32, [x, y])
                if binary_math_symbol_f32(math).is_some() =>
            {
                let mut xv = self.lower_expr(*x, Some(typ))?.value();
                let mut yv = self.lower_expr(*y, Some(typ))?.value();
                xv = self.coerce_value_to_fir_type(xv, typ)?;
                yv = self.coerce_value_to_fir_type(yv, typ)?;
                let fref = self.ensure_binary_import(
                    binary_math_symbol_f32(math).expect("guarded by is_some"),
                    types::F32,
                )?;
                let call = self.fb.ins().call(fref, &[xv, yv]);
                Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]))
            }
            (math, FirType::Float64, [x, y]) if binary_math_symbol_f64(math).is_some() => {
                let mut xv = self.lower_expr(*x, Some(typ))?.value();
                let mut yv = self.lower_expr(*y, Some(typ))?.value();
                xv = self.coerce_value_to_fir_type(xv, typ)?;
                yv = self.coerce_value_to_fir_type(yv, typ)?;
                let fref = self.ensure_binary_import(
                    binary_math_symbol_f64(math).expect("guarded by is_some"),
                    types::F64,
                )?;
                let call = self.fb.ins().call(fref, &[xv, yv]);
                Ok(LoweredExpr::Scalar(self.fb.inst_results(call)[0]))
            }
            _ => Err(LoweringError::Unsupported(format!(
                "unsupported function call lowering `{name}` with typ={typ:?} args={}",
                args.len()
            ))),
        }
    }

    /// Ensures a cached unary imported function reference exists.
    fn ensure_unary_import(&mut self, symbol: &str, ty: Type) -> Result<FuncRef, LoweringError> {
        self.ensure_import(symbol, &[ty], ty)
    }

    /// Ensures a cached binary imported function reference exists.
    fn ensure_binary_import(&mut self, symbol: &str, ty: Type) -> Result<FuncRef, LoweringError> {
        self.ensure_import(symbol, &[ty, ty], ty)
    }

    /// Declares/imports a CLIF function and caches the `FuncRef` by signature key.
    ///
    /// Cranelift imports are identified by both symbol and signature, so the
    /// cache key includes parameter and return types.
    fn ensure_import(
        &mut self,
        symbol: &str,
        params: &[Type],
        ret: Type,
    ) -> Result<FuncRef, LoweringError> {
        let cache_key = format!(
            "{symbol}({})->{}",
            params
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            ret
        );
        if let Some(fref) = self.import_refs.get(&cache_key).copied() {
            return Ok(fref);
        }
        let mut sig = self.jit.make_signature();
        for param in params {
            sig.params.push(AbiParam::new(*param));
        }
        sig.returns.push(AbiParam::new(ret));
        let func_id = self
            .jit
            .declare_function(symbol, Linkage::Import, &sig)
            .map_err(|e| LoweringError::Jit(format!("declare import `{symbol}` failed: {e}")))?;
        let fref = self.jit.declare_func_in_func(func_id, self.fb.func);
        self.import_refs.insert(cache_key, fref);
        Ok(fref)
    }

    // ── Soundfile field loads ─────────────────────────────────────────────────

    /// Loads the `Soundfile*` stored in `dsp->fSoundN`.
    ///
    /// Helper shared by `lower_load_soundfile_{length,rate,buffer}`.
    fn load_soundfile_ptr(&mut self, var: &str) -> Result<Value, LoweringError> {
        let field = self.struct_field(var)?.clone();
        let dsp = self.dsp_base_ptr()?;
        let sf_addr = self.fb.ins().iadd_imm(dsp, i64::from(field.offset_bytes));
        let sf_ptr = self.fb.ins().load(self.ptr_ty, MemFlags::new(), sf_addr, 0);
        Ok(sf_ptr)
    }

    /// Lowers `LoadSoundfileLength { var, part }` → `fSoundN->fLength[part]`.
    ///
    /// Returns an `i32` scalar (FIR Int32).
    fn lower_load_soundfile_length(
        &mut self,
        var: &str,
        part: FirId,
    ) -> Result<LoweredExpr, LoweringError> {
        let sf_ptr = self.load_soundfile_ptr(var)?;
        // fLength is an `int*` at byte offset 8 from the Soundfile*.
        let len_ptr = self.fb.ins().load(self.ptr_ty, MemFlags::new(), sf_ptr, 8);
        let part_v = self.lower_expr(part, Some(&FirType::Int32))?.value();
        let addr = self.indexed_addr(len_ptr, part_v, 4);
        let result = self.fb.ins().load(types::I32, MemFlags::new(), addr, 0);
        Ok(LoweredExpr::Scalar(result))
    }

    /// Lowers `LoadSoundfileRate { var, part }` → `fSoundN->fSR[part]`.
    ///
    /// Returns an `i32` scalar (FIR Int32).
    fn lower_load_soundfile_rate(
        &mut self,
        var: &str,
        part: FirId,
    ) -> Result<LoweredExpr, LoweringError> {
        let sf_ptr = self.load_soundfile_ptr(var)?;
        // fSR is an `int*` at byte offset 16 from the Soundfile*.
        let sr_ptr = self.fb.ins().load(self.ptr_ty, MemFlags::new(), sf_ptr, 16);
        let part_v = self.lower_expr(part, Some(&FirType::Int32))?.value();
        let addr = self.indexed_addr(sr_ptr, part_v, 4);
        let result = self.fb.ins().load(types::I32, MemFlags::new(), addr, 0);
        Ok(LoweredExpr::Scalar(result))
    }

    /// Lowers `LoadSoundfileBuffer { var, chan, part, idx, typ }` →
    /// `((FAUSTFLOAT**)fSoundN->fBuffers)[chan][fSoundN->fOffset[part] + idx]`.
    ///
    /// The element type is inferred from `typ` (typically `FaustFloat` = `f32`).
    /// The buffer pointer array uses pointer-sized strides; individual samples
    /// use the natural stride of the element type.
    ///
    /// # Note on double precision
    /// When the host loads a soundfile with `fIsDouble=true`, the buffer
    /// contains `f64` samples.  The current Cranelift bring-up targets
    /// `FAUSTFLOAT=float` exclusively; a double-precision path can be added
    /// when needed.
    fn lower_load_soundfile_buffer(
        &mut self,
        var: &str,
        chan: FirId,
        part: FirId,
        idx: FirId,
        typ: &FirType,
    ) -> Result<LoweredExpr, LoweringError> {
        let sf_ptr = self.load_soundfile_ptr(var)?;

        // fBuffers is a `void*` (= float** or double**) at byte offset 0.
        let bufs = self.fb.ins().load(self.ptr_ty, MemFlags::new(), sf_ptr, 0);

        // chan_buf = ((FAUSTFLOAT**)bufs)[chan]  — one pointer per channel.
        let ptr_stride = i64::from(self.ptr_ty.bytes());
        let chan_v = self.lower_expr(chan, Some(&FirType::Int32))?.value();
        let chan_ptr_addr = self.indexed_addr(bufs, chan_v, ptr_stride);
        let chan_buf = self
            .fb
            .ins()
            .load(self.ptr_ty, MemFlags::new(), chan_ptr_addr, 0);

        // part_offset = fOffset[part]  — fOffset is `int*` at byte offset 24.
        let off_ptr = self.fb.ins().load(self.ptr_ty, MemFlags::new(), sf_ptr, 24);
        let part_v = self.lower_expr(part, Some(&FirType::Int32))?.value();
        let part_off_addr = self.indexed_addr(off_ptr, part_v, 4);
        let part_off = self
            .fb
            .ins()
            .load(types::I32, MemFlags::new(), part_off_addr, 0);

        // actual_idx = fOffset[part] + idx
        let idx_v = self.lower_expr(idx, Some(&FirType::Int32))?.value();
        let actual_idx = self.fb.ins().iadd(part_off, idx_v);

        // Load sample — stride is size of element type (f32=4 or f64=8).
        let elem_clif = self.fir_type_to_clif(typ)?;
        let elem_stride = i64::from(elem_clif.bytes());
        let sample_addr = self.indexed_addr(chan_buf, actual_idx, elem_stride);
        let raw = self
            .fb
            .ins()
            .load(elem_clif, MemFlags::new(), sample_addr, 0);
        let result = self.coerce_value_to_fir_type(raw, typ)?;
        Ok(LoweredExpr::Scalar(result))
    }
}

/// Maps a FIR unary math op to the imported `f32` host symbol used by lowering.
///
/// Returns `None` when the operation is not yet supported in the current
/// subset-lowering implementation.
pub(crate) fn unary_math_symbol_f32(math: fir::FirMathOp) -> Option<&'static str> {
    Some(match math {
        fir::FirMathOp::Sin => "sinf",
        fir::FirMathOp::Cos => "cosf",
        fir::FirMathOp::Acos => "acosf",
        fir::FirMathOp::Asin => "asinf",
        fir::FirMathOp::Atan => "atanf",
        fir::FirMathOp::Tan => "tanf",
        fir::FirMathOp::Exp => "expf",
        fir::FirMathOp::Exp10 => "exp10f",
        fir::FirMathOp::Log => "logf",
        fir::FirMathOp::Log10 => "log10f",
        fir::FirMathOp::Sqrt => "sqrtf",
        fir::FirMathOp::Abs => "fabsf",
        fir::FirMathOp::Floor => "floorf",
        fir::FirMathOp::Ceil => "ceilf",
        fir::FirMathOp::Rint => "rintf",
        fir::FirMathOp::Round => "roundf",
        _ => return None,
    })
}

/// Maps a FIR unary math op to the imported `f64` host symbol used by lowering.
///
/// Returns `None` when the operation is not yet supported in the current
/// subset-lowering implementation.
pub(crate) fn unary_math_symbol_f64(math: fir::FirMathOp) -> Option<&'static str> {
    Some(match math {
        fir::FirMathOp::Sin => "sin",
        fir::FirMathOp::Cos => "cos",
        fir::FirMathOp::Acos => "acos",
        fir::FirMathOp::Asin => "asin",
        fir::FirMathOp::Atan => "atan",
        fir::FirMathOp::Tan => "tan",
        fir::FirMathOp::Exp => "exp",
        fir::FirMathOp::Exp10 => "exp10",
        fir::FirMathOp::Log => "log",
        fir::FirMathOp::Log10 => "log10",
        fir::FirMathOp::Sqrt => "sqrt",
        fir::FirMathOp::Abs => "fabs",
        fir::FirMathOp::Floor => "floor",
        fir::FirMathOp::Ceil => "ceil",
        fir::FirMathOp::Rint => "rint",
        fir::FirMathOp::Round => "round",
        _ => return None,
    })
}

/// Maps a FIR binary math op to the imported `f32` host symbol used by lowering.
pub(crate) fn binary_math_symbol_f32(math: fir::FirMathOp) -> Option<&'static str> {
    Some(match math {
        fir::FirMathOp::Pow => "powf",
        fir::FirMathOp::Min => "fminf",
        fir::FirMathOp::Max => "fmaxf",
        fir::FirMathOp::Atan2 => "atan2f",
        fir::FirMathOp::Fmod => "fmodf",
        fir::FirMathOp::Remainder => "remainderf",
        _ => return None,
    })
}

/// Maps a FIR binary math op to the imported `f64` host symbol used by lowering.
pub(crate) fn binary_math_symbol_f64(math: fir::FirMathOp) -> Option<&'static str> {
    Some(match math {
        fir::FirMathOp::Pow => "pow",
        fir::FirMathOp::Min => "fmin",
        fir::FirMathOp::Max => "fmax",
        fir::FirMathOp::Atan2 => "atan2",
        fir::FirMathOp::Fmod => "fmod",
        fir::FirMathOp::Remainder => "remainder",
        _ => return None,
    })
}

/// Attempts to lower the FIR `compute` body into the current Cranelift subset.
///
/// Returns `Ok(true)` when lowering succeeds and a real body is emitted.
///
/// This function assumes the caller already created and switched to a valid
/// Cranelift entry block with function params matching the `compute` ABI.
/// It binds FIR arguments (`dsp`, `count`, `inputs`, `outputs`) into the local
/// lowering environment before recursively lowering the statement body.
///
/// Any unsupported FIR node shape returns `LoweringError::Unsupported`, which
/// the caller may convert into a controlled stub fallback.
pub(crate) struct FunctionBodyLoweringContext<'a> {
    pub(crate) store: &'a FirStore,
    pub(crate) jit: &'a mut JITModule,
    pub(crate) struct_layout: &'a StructLayoutPlan,
    pub(crate) ptr_ty: Type,
    pub(crate) static_data_ids: &'a HashMap<String, DataId>,
    pub(crate) extern_data_ids: &'a HashMap<String, DataId>,
    pub(crate) extern_function_symbols: &'a HashMap<String, *const c_void>,
    /// 64-bit (`double`) precision: resolves `FaustFloat` to `F64`.
    pub(crate) double: bool,
}

pub(crate) fn try_lower_function_body(
    cx: FunctionBodyLoweringContext<'_>,
    fb: &mut FunctionBuilder<'_>,
    function_decl: FirId,
) -> Result<bool, LoweringError> {
    let (args, body) = match match_fir(cx.store, function_decl) {
        FirMatch::DeclareFun {
            args,
            body: Some(body),
            ..
        } => (args, body),
        other => {
            return Err(LoweringError::Unsupported(format!(
                "function declaration shape unsupported: {other:?}"
            )));
        }
    };

    let entry = fb
        .current_block()
        .ok_or_else(|| LoweringError::Jit("missing active entry block".to_string()))?;
    let params = fb.block_params(entry).to_vec();
    if params.len() != args.len() {
        return Err(LoweringError::Unsupported(format!(
            "function arg count mismatch: clif={} fir={}",
            params.len(),
            args.len()
        )));
    }
    let mut vars = HashMap::new();
    for (arg, value) in args.iter().zip(params) {
        let lowered = match arg.typ {
            FirType::Ptr(ref inner) => LoweredExpr::Ptr {
                value,
                pointee: Some(FirTypeRef::from_fir_type(inner)),
            },
            FirType::Obj | FirType::UI | FirType::Meta | FirType::Sound => LoweredExpr::Ptr {
                value,
                pointee: None,
            },
            _ => LoweredExpr::Scalar(value),
        };
        vars.insert(arg.name.clone(), LocalBinding::Entry(lowered));
    }

    let mut lowering = ComputeLowering {
        store: cx.store,
        jit: cx.jit,
        fb,
        struct_layout: cx.struct_layout,
        ptr_ty: cx.ptr_ty,
        vars,
        next_var: 0,
        import_refs: HashMap::new(),
        static_data_ids: cx.static_data_ids,
        extern_data_ids: cx.extern_data_ids,
        extern_function_symbols: cx.extern_function_symbols,
        double: cx.double,
        _marker: std::marker::PhantomData,
    };
    lowering.lower_stmt(body)?;
    if !is_return_terminated(lowering.fb) {
        emit_return_stub(lowering.fb);
    }
    Ok(true)
}
