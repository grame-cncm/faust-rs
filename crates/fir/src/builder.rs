//! Canonical FIR node builder.
//!
//! Builder methods encode the normalized tree shapes consumed by `match_fir`
//! and downstream backends. This is the preferred construction surface for FIR
//! nodes so type/access encoding remains centralized.

use super::*;

/// Canonical builder API for constructing FIR nodes.
///
/// Builder methods create the normalized node shapes expected by `match_fir`
/// and downstream backends, including explicit types on value nodes and stable
/// encodings for lists and declarations.
pub struct FirBuilder<'a> {
    store: &'a mut FirStore,
}

impl<'a> FirBuilder<'a> {
    #[must_use]
    /// Creates a new instance of this type.
    pub fn new(store: &'a mut FirStore) -> Self {
        Self { store }
    }

    /// C++ parity: `Int32NumInst`.
    #[must_use]
    pub fn int32(&mut self, value: i32) -> FirId {
        let typ = encode_type(&mut self.store.arena, &FirType::Int32);
        let val = self.store.arena.int(i64::from(value));
        intern_tag(&mut self.store.arena, FIR_V_INT32_TAG, &[typ, val])
    }

    /// C++ parity: `Int64NumInst`.
    #[must_use]
    pub fn int64(&mut self, value: i64) -> FirId {
        let typ = encode_type(&mut self.store.arena, &FirType::Int64);
        let val = self.store.arena.int(value);
        intern_tag(&mut self.store.arena, FIR_V_INT64_TAG, &[typ, val])
    }

    /// C++ parity: `FloatNumInst`.
    #[must_use]
    pub fn float32(&mut self, value: f32) -> FirId {
        let typ = encode_type(&mut self.store.arena, &FirType::Float32);
        let bits = self.store.arena.int(i64::from(value.to_bits()));
        intern_tag(&mut self.store.arena, FIR_V_FLOAT32_TAG, &[typ, bits])
    }

    /// C++ parity: `DoubleNumInst`.
    #[must_use]
    pub fn float64(&mut self, value: f64) -> FirId {
        let typ = encode_type(&mut self.store.arena, &FirType::Float64);
        let val = self.store.arena.float(value);
        intern_tag(&mut self.store.arena, FIR_V_FLOAT64_TAG, &[typ, val])
    }

    /// C++ parity: `BoolNumInst`.
    #[must_use]
    pub fn bool_(&mut self, value: bool) -> FirId {
        let typ = encode_type(&mut self.store.arena, &FirType::Bool);
        let val = self.store.arena.int(if value { 1 } else { 0 });
        intern_tag(&mut self.store.arena, FIR_V_BOOL_TAG, &[typ, val])
    }

    /// C++ parity: `QuadNumInst`.
    #[must_use]
    pub fn quad(&mut self, value: f64) -> FirId {
        let typ = encode_type(&mut self.store.arena, &FirType::Quad);
        let val = self.store.arena.float(value);
        intern_tag(&mut self.store.arena, FIR_V_QUAD_TAG, &[typ, val])
    }

    /// C++ parity: `FixedPointNumInst`.
    #[must_use]
    pub fn fixed_point(&mut self, value: f64) -> FirId {
        let typ = encode_type(&mut self.store.arena, &FirType::FixedPoint);
        let val = self.store.arena.float(value);
        intern_tag(&mut self.store.arena, FIR_V_FIXED_POINT_TAG, &[typ, val])
    }

    /// C++ parity: `ValueArrayInst`.
    #[must_use]
    pub fn value_array(&mut self, values: &[FirId], typ: FirType) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let values_id = encode_list(&mut self.store.arena, values);
        intern_tag(
            &mut self.store.arena,
            FIR_V_VALUE_ARRAY_TAG,
            &[typ_id, values_id],
        )
    }

    /// C++ parity: `Int32ArrayNumInst`.
    #[must_use]
    pub fn int32_array(&mut self, values: &[i32]) -> FirId {
        let typ = encode_type(
            &mut self.store.arena,
            &FirType::Array(Box::new(FirType::Int32), values.len()),
        );
        let value_ids: Vec<_> = values
            .iter()
            .map(|v| self.store.arena.int(i64::from(*v)))
            .collect();
        let values_id = encode_list(&mut self.store.arena, &value_ids);
        intern_tag(
            &mut self.store.arena,
            FIR_V_INT32_ARRAY_TAG,
            &[typ, values_id],
        )
    }

    /// C++ parity: `FloatArrayNumInst`.
    #[must_use]
    pub fn float32_array(&mut self, values: &[f32]) -> FirId {
        let typ = encode_type(
            &mut self.store.arena,
            &FirType::Array(Box::new(FirType::Float32), values.len()),
        );
        let value_ids: Vec<_> = values
            .iter()
            .map(|v| self.store.arena.int(i64::from(v.to_bits())))
            .collect();
        let values_id = encode_list(&mut self.store.arena, &value_ids);
        intern_tag(
            &mut self.store.arena,
            FIR_V_FLOAT32_ARRAY_TAG,
            &[typ, values_id],
        )
    }

    /// C++ parity: `DoubleArrayNumInst`.
    #[must_use]
    pub fn float64_array(&mut self, values: &[f64]) -> FirId {
        let typ = encode_type(
            &mut self.store.arena,
            &FirType::Array(Box::new(FirType::Float64), values.len()),
        );
        let value_ids: Vec<_> = values.iter().map(|v| self.store.arena.float(*v)).collect();
        let values_id = encode_list(&mut self.store.arena, &value_ids);
        intern_tag(
            &mut self.store.arena,
            FIR_V_FLOAT64_ARRAY_TAG,
            &[typ, values_id],
        )
    }

    /// C++ parity: `QuadArrayNumInst`.
    #[must_use]
    pub fn quad_array(&mut self, values: &[f64]) -> FirId {
        let typ = encode_type(
            &mut self.store.arena,
            &FirType::Array(Box::new(FirType::Quad), values.len()),
        );
        let value_ids: Vec<_> = values.iter().map(|v| self.store.arena.float(*v)).collect();
        let values_id = encode_list(&mut self.store.arena, &value_ids);
        intern_tag(
            &mut self.store.arena,
            FIR_V_QUAD_ARRAY_TAG,
            &[typ, values_id],
        )
    }

    /// C++ parity: `FixedPointArrayNumInst`.
    #[must_use]
    pub fn fixed_point_array(&mut self, values: &[f64]) -> FirId {
        let typ = encode_type(
            &mut self.store.arena,
            &FirType::Array(Box::new(FirType::FixedPoint), values.len()),
        );
        let value_ids: Vec<_> = values.iter().map(|v| self.store.arena.float(*v)).collect();
        let values_id = encode_list(&mut self.store.arena, &value_ids);
        intern_tag(
            &mut self.store.arena,
            FIR_V_FIXED_POINT_ARRAY_TAG,
            &[typ, values_id],
        )
    }

    /// C++ parity: `LoadVarInst`.
    #[must_use]
    pub fn load_var(&mut self, name: impl Into<String>, access: AccessType, typ: FirType) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let name_id = self.store.arena.symbol(name);
        let access_id = encode_access(&mut self.store.arena, access);
        intern_tag(
            &mut self.store.arena,
            FIR_V_LOAD_VAR_TAG,
            &[typ_id, name_id, access_id],
        )
    }

    /// C++ parity helper: explicit table read expression.
    #[must_use]
    pub fn load_table(
        &mut self,
        name: impl Into<String>,
        access: AccessType,
        index: FirId,
        typ: FirType,
    ) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let name_id = self.store.arena.symbol(name);
        let access_id = encode_access(&mut self.store.arena, access);
        intern_tag(
            &mut self.store.arena,
            FIR_V_LOAD_TABLE_TAG,
            &[typ_id, name_id, access_id, index],
        )
    }

    /// C++ parity: `LoadVarAddressInst`.
    #[must_use]
    pub fn load_var_address(
        &mut self,
        name: impl Into<String>,
        access: AccessType,
        typ: FirType,
    ) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let name_id = self.store.arena.symbol(name);
        let access_id = encode_access(&mut self.store.arena, access);
        intern_tag(
            &mut self.store.arena,
            FIR_V_LOAD_VAR_ADDRESS_TAG,
            &[typ_id, name_id, access_id],
        )
    }

    /// C++ parity: `TeeVarInst`.
    #[must_use]
    pub fn tee_var(
        &mut self,
        name: impl Into<String>,
        access: AccessType,
        value: FirId,
        typ: FirType,
    ) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let name_id = self.store.arena.symbol(name);
        let access_id = encode_access(&mut self.store.arena, access);
        intern_tag(
            &mut self.store.arena,
            FIR_V_TEE_VAR_TAG,
            &[typ_id, name_id, access_id, value],
        )
    }

    /// C++ parity: `BinopInst`.
    #[must_use]
    pub fn binop(&mut self, op: FirBinOp, lhs: FirId, rhs: FirId, typ: FirType) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let op_id = encode_binop(&mut self.store.arena, op);
        intern_tag(
            &mut self.store.arena,
            FIR_V_BINOP_TAG,
            &[typ_id, op_id, lhs, rhs],
        )
    }

    /// C++ parity: `NegInst`.
    #[must_use]
    pub fn neg(&mut self, value: FirId, typ: FirType) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        intern_tag(&mut self.store.arena, FIR_V_NEG_TAG, &[typ_id, value])
    }

    /// C++ parity: `CastInst`.
    #[must_use]
    pub fn cast(&mut self, typ: FirType, value: FirId) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        intern_tag(&mut self.store.arena, FIR_V_CAST_TAG, &[typ_id, value])
    }

    /// C++ parity: `BitcastInst`.
    #[must_use]
    pub fn bitcast(&mut self, typ: FirType, value: FirId) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        intern_tag(&mut self.store.arena, FIR_V_BITCAST_TAG, &[typ_id, value])
    }

    /// C++ parity: `Select2Inst`.
    #[must_use]
    pub fn select2(
        &mut self,
        cond: FirId,
        then_value: FirId,
        else_value: FirId,
        typ: FirType,
    ) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        intern_tag(
            &mut self.store.arena,
            FIR_V_SELECT2_TAG,
            &[typ_id, cond, then_value, else_value],
        )
    }

    /// C++ parity: `FunCallInst`.
    #[must_use]
    pub fn fun_call(&mut self, name: impl Into<String>, args: &[FirId], typ: FirType) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let name_id = self.store.arena.symbol(name);
        let args_id = encode_list(&mut self.store.arena, args);
        intern_tag(
            &mut self.store.arena,
            FIR_V_FUNCALL_TAG,
            &[typ_id, name_id, args_id],
        )
    }

    /// C++ parity helper: typed math call that avoids stringly-typed lowering sites.
    #[must_use]
    pub fn math_call(&mut self, op: FirMathOp, args: &[FirId], typ: FirType) -> FirId {
        self.fun_call(op.symbol(), args, typ)
    }

    /// C++ parity: `NullValueInst`.
    #[must_use]
    pub fn null_value(&mut self, typ: FirType) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        intern_tag(&mut self.store.arena, FIR_V_NULL_TAG, &[typ_id])
    }

    /// C++ parity: `NewDSPInst`.
    #[must_use]
    pub fn new_dsp(&mut self, name: impl Into<String>, typ: FirType) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let name_id = self.store.arena.symbol(name);
        intern_tag(&mut self.store.arena, FIR_V_NEW_DSP_TAG, &[typ_id, name_id])
    }

    /// C++ parity: `DeclareVarInst`.
    #[must_use]
    pub fn declare_var(
        &mut self,
        name: impl Into<String>,
        typ: FirType,
        access: AccessType,
        init: Option<FirId>,
    ) -> FirId {
        let name_id = self.store.arena.symbol(name);
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let access_id = encode_access(&mut self.store.arena, access);
        let init_id = init.unwrap_or_else(|| self.store.arena.nil());
        intern_tag(
            &mut self.store.arena,
            FIR_DECLARE_VAR_TAG,
            &[name_id, typ_id, access_id, init_id],
        )
    }

    /// C++ parity helper: explicit table declaration with literal initial values.
    #[must_use]
    pub fn declare_table(
        &mut self,
        name: impl Into<String>,
        access: AccessType,
        elem_type: FirType,
        values: &[FirId],
    ) -> FirId {
        let name_id = self.store.arena.symbol(name);
        let access_id = encode_access(&mut self.store.arena, access);
        let typ_id = encode_type(&mut self.store.arena, &elem_type);
        let values_id = encode_list(&mut self.store.arena, values);
        intern_tag(
            &mut self.store.arena,
            FIR_DECLARE_TABLE_TAG,
            &[name_id, access_id, typ_id, values_id],
        )
    }

    /// C++ parity: `DeclareFunInst`.
    ///
    /// Pass `body: Some(id)` for a full function definition or `body: None` for
    /// a pure prototype (forward declaration / pure-virtual equivalent).
    #[must_use]
    pub fn declare_fun(
        &mut self,
        name: impl Into<String>,
        typ: FirType,
        args: &[NamedType],
        body: Option<FirId>,
        is_inline: bool,
    ) -> FirId {
        let name_id = self.store.arena.symbol(name);
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let args_id = encode_named_types(&mut self.store.arena, args);
        let inline_id = self.store.arena.int(if is_inline { 1 } else { 0 });
        match body {
            Some(body_id) => intern_tag(
                &mut self.store.arena,
                FIR_DECLARE_FUN_TAG,
                &[name_id, typ_id, args_id, body_id, inline_id],
            ),
            None => intern_tag(
                &mut self.store.arena,
                FIR_DECLARE_FUN_PROTO_TAG,
                &[name_id, typ_id, args_id, inline_id],
            ),
        }
    }

    /// C++ parity: `DeclareStructTypeInst`.
    #[must_use]
    pub fn declare_struct_type(&mut self, typ: FirType) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        intern_tag(
            &mut self.store.arena,
            FIR_DECLARE_STRUCT_TYPE_TAG,
            &[typ_id],
        )
    }

    /// C++ parity: `DeclareBufferIterators`.
    #[must_use]
    pub fn declare_buffer_iterators(
        &mut self,
        name1: impl Into<String>,
        name2: impl Into<String>,
        channels: i32,
        typ: FirType,
        mutable: bool,
        chunk: bool,
    ) -> FirId {
        let name1_id = self.store.arena.symbol(name1);
        let name2_id = self.store.arena.symbol(name2);
        let channels_id = self.store.arena.int(i64::from(channels));
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let mutable_id = self.store.arena.int(if mutable { 1 } else { 0 });
        let chunk_id = self.store.arena.int(if chunk { 1 } else { 0 });
        intern_tag(
            &mut self.store.arena,
            FIR_DECLARE_BUFFER_ITERATORS_TAG,
            &[
                name1_id,
                name2_id,
                channels_id,
                typ_id,
                mutable_id,
                chunk_id,
            ],
        )
    }

    /// C++ parity: `StoreVarInst`.
    #[must_use]
    pub fn store_var(
        &mut self,
        name: impl Into<String>,
        access: AccessType,
        value: FirId,
    ) -> FirId {
        let name_id = self.store.arena.symbol(name);
        let access_id = encode_access(&mut self.store.arena, access);
        intern_tag(
            &mut self.store.arena,
            FIR_STORE_VAR_TAG,
            &[name_id, access_id, value],
        )
    }

    /// C++ parity helper: explicit table write statement.
    #[must_use]
    pub fn store_table(
        &mut self,
        name: impl Into<String>,
        access: AccessType,
        index: FirId,
        value: FirId,
    ) -> FirId {
        let name_id = self.store.arena.symbol(name);
        let access_id = encode_access(&mut self.store.arena, access);
        intern_tag(
            &mut self.store.arena,
            FIR_STORE_TABLE_TAG,
            &[name_id, access_id, index, value],
        )
    }

    /// C++ parity: `ShiftArrayVarInst`.
    #[must_use]
    pub fn shift_array_var(
        &mut self,
        name: impl Into<String>,
        access: AccessType,
        delay: i32,
    ) -> FirId {
        let name_id = self.store.arena.symbol(name);
        let access_id = encode_access(&mut self.store.arena, access);
        let delay_id = self.store.arena.int(i64::from(delay));
        intern_tag(
            &mut self.store.arena,
            FIR_SHIFT_ARRAY_VAR_TAG,
            &[name_id, access_id, delay_id],
        )
    }

    /// C++ parity: `DropInst`.
    #[must_use]
    pub fn drop_(&mut self, value: FirId) -> FirId {
        intern_tag(&mut self.store.arena, FIR_DROP_TAG, &[value])
    }

    /// C++ parity: `NullStatementInst`.
    #[must_use]
    pub fn null_statement(&mut self) -> FirId {
        intern_tag(&mut self.store.arena, FIR_NULL_STATEMENT_TAG, &[])
    }

    /// C++ parity: `RetInst`.
    #[must_use]
    pub fn ret(&mut self, value: Option<FirId>) -> FirId {
        let value_id = value.unwrap_or_else(|| self.store.arena.nil());
        intern_tag(&mut self.store.arena, FIR_RETURN_TAG, &[value_id])
    }

    /// C++ parity: `BlockInst`.
    #[must_use]
    pub fn block(&mut self, body: &[FirId]) -> FirId {
        let list = encode_list(&mut self.store.arena, body);
        intern_tag(&mut self.store.arena, FIR_BLOCK_TAG, &[list])
    }

    /// C++ parity: `IfInst`.
    #[must_use]
    pub fn if_(&mut self, cond: FirId, then_block: FirId, else_block: Option<FirId>) -> FirId {
        let else_id = else_block.unwrap_or_else(|| self.store.arena.nil());
        intern_tag(
            &mut self.store.arena,
            FIR_IF_TAG,
            &[cond, then_block, else_id],
        )
    }

    /// C++ parity: `ControlInst`.
    #[must_use]
    pub fn control(&mut self, cond: FirId, stmt: FirId) -> FirId {
        intern_tag(&mut self.store.arena, FIR_CONTROL_TAG, &[cond, stmt])
    }

    /// C++ parity: `ForLoopInst`.
    #[must_use]
    pub fn for_loop(
        &mut self,
        var: impl Into<String>,
        init: FirId,
        end: FirId,
        step: FirId,
        body: FirId,
        is_reverse: bool,
    ) -> FirId {
        let var_id = self.store.arena.symbol(var);
        let reverse = self.store.arena.int(if is_reverse { 1 } else { 0 });
        intern_tag(
            &mut self.store.arena,
            FIR_FOR_LOOP_TAG,
            &[var_id, init, end, step, body, reverse],
        )
    }

    /// C++ parity: `SimpleForLoopInst`.
    #[must_use]
    pub fn simple_for_loop(
        &mut self,
        var: impl Into<String>,
        upper: FirId,
        body: FirId,
        is_reverse: bool,
    ) -> FirId {
        let var_id = self.store.arena.symbol(var);
        let reverse = self.store.arena.int(if is_reverse { 1 } else { 0 });
        intern_tag(
            &mut self.store.arena,
            FIR_SIMPLE_FOR_LOOP_TAG,
            &[var_id, upper, body, reverse],
        )
    }

    /// C++ parity: `IteratorForLoopInst`.
    #[must_use]
    pub fn iterator_for_loop(
        &mut self,
        iterators: &[&str],
        is_reverse: bool,
        body: FirId,
    ) -> FirId {
        let iter_ids: Vec<_> = iterators
            .iter()
            .map(|name| self.store.arena.symbol(*name))
            .collect();
        let iter_list = encode_list(&mut self.store.arena, &iter_ids);
        let reverse = self.store.arena.int(if is_reverse { 1 } else { 0 });
        intern_tag(
            &mut self.store.arena,
            FIR_ITERATOR_FOR_LOOP_TAG,
            &[iter_list, reverse, body],
        )
    }

    /// C++ parity: `WhileLoopInst`.
    #[must_use]
    pub fn while_loop(&mut self, cond: FirId, body: FirId) -> FirId {
        intern_tag(&mut self.store.arena, FIR_WHILE_LOOP_TAG, &[cond, body])
    }

    /// C++ parity: `SwitchInst`.
    #[must_use]
    pub fn switch(&mut self, cond: FirId, cases: &[(i64, FirId)], default: Option<FirId>) -> FirId {
        let cases_id = encode_switch_cases(&mut self.store.arena, cases);
        let default_id = default.unwrap_or_else(|| self.store.arena.nil());
        intern_tag(
            &mut self.store.arena,
            FIR_SWITCH_TAG,
            &[cond, cases_id, default_id],
        )
    }

    /// C++ parity: `ModuleInst`.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    /// Pass `&[]` for `sub_modules` unless the program declares a table filled
    /// at initialization time; see [`Self::sub_module`].
    pub fn module(
        &mut self,
        num_inputs: usize,
        num_outputs: usize,
        name: impl Into<String>,
        dsp_struct: FirId,
        globals: FirId,
        functions: FirId,
        static_decls: FirId,
        sub_modules: &[FirId],
    ) -> FirId {
        let name_id = self.store.arena.symbol(name);
        let num_inputs_id = self.store.arena.int(num_inputs as i64);
        let num_outputs_id = self.store.arena.int(num_outputs as i64);
        let sub_modules_id = self.block(sub_modules);
        intern_tag(
            &mut self.store.arena,
            FIR_MODULE_TAG,
            &[
                num_inputs_id,
                num_outputs_id,
                name_id,
                dsp_struct,
                globals,
                functions,
                static_decls,
                sub_modules_id,
            ],
        )
    }

    /// C++ parity: the `CodeContainer` produced by `signal2Container` for one
    /// `SIGGEN` table generator.
    ///
    /// `functions` must hold exactly `instanceInit{name}(dsp, sample_rate)` and
    /// `fill{name}(dsp, count, table)`; `elem_type` is the element type of the
    /// filled table and therefore of the `table` argument. Nested generators go
    /// in `sub_modules` and are filled before this one runs.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn sub_module(
        &mut self,
        name: impl Into<String>,
        elem_type: FirType,
        dsp_struct: FirId,
        static_decls: FirId,
        globals: FirId,
        functions: FirId,
        sub_modules: &[FirId],
    ) -> FirId {
        let name_id = self.store.arena.symbol(name);
        let elem_type_id = encode_type(&mut self.store.arena, &elem_type);
        let sub_modules_id = self.block(sub_modules);
        intern_tag(
            &mut self.store.arena,
            FIR_SUB_MODULE_TAG,
            &[
                name_id,
                elem_type_id,
                dsp_struct,
                static_decls,
                globals,
                functions,
                sub_modules_id,
            ],
        )
    }

    /// C++ parity: `OpenboxInst`.
    #[must_use]
    pub fn open_box(&mut self, typ: UiBoxType, label: impl Into<String>) -> FirId {
        let typ_id = encode_ui_box_type(&mut self.store.arena, typ);
        let label_id = self.store.arena.symbol(label);
        intern_tag(&mut self.store.arena, FIR_OPEN_BOX_TAG, &[typ_id, label_id])
    }

    /// C++ parity: `CloseboxInst`.
    #[must_use]
    pub fn close_box(&mut self) -> FirId {
        intern_tag(&mut self.store.arena, FIR_CLOSE_BOX_TAG, &[])
    }

    /// C++ parity: `AddButtonInst`.
    #[must_use]
    pub fn add_button(
        &mut self,
        typ: ButtonType,
        label: impl Into<String>,
        var: impl Into<String>,
    ) -> FirId {
        let typ_id = encode_button_type(&mut self.store.arena, typ);
        let label_id = self.store.arena.symbol(label);
        let var_id = self.store.arena.symbol(var);
        intern_tag(
            &mut self.store.arena,
            FIR_ADD_BUTTON_TAG,
            &[typ_id, label_id, var_id],
        )
    }

    /// C++ parity: `AddSliderInst`.
    #[must_use]
    pub fn add_slider(
        &mut self,
        typ: SliderType,
        label: impl Into<String>,
        var: impl Into<String>,
        range: SliderRange,
    ) -> FirId {
        let typ_id = encode_slider_type(&mut self.store.arena, typ);
        let label_id = self.store.arena.symbol(label);
        let var_id = self.store.arena.symbol(var);
        let init_id = self.store.arena.float(range.init);
        let lo_id = self.store.arena.float(range.lo);
        let hi_id = self.store.arena.float(range.hi);
        let step_id = self.store.arena.float(range.step);
        intern_tag(
            &mut self.store.arena,
            FIR_ADD_SLIDER_TAG,
            &[typ_id, label_id, var_id, init_id, lo_id, hi_id, step_id],
        )
    }

    /// C++ parity: `AddBargraphInst`.
    #[must_use]
    pub fn add_bargraph(
        &mut self,
        typ: BargraphType,
        label: impl Into<String>,
        var: impl Into<String>,
        lo: f64,
        hi: f64,
    ) -> FirId {
        let typ_id = encode_bargraph_type(&mut self.store.arena, typ);
        let label_id = self.store.arena.symbol(label);
        let var_id = self.store.arena.symbol(var);
        let lo_id = self.store.arena.float(lo);
        let hi_id = self.store.arena.float(hi);
        intern_tag(
            &mut self.store.arena,
            FIR_ADD_BARGRAPH_TAG,
            &[typ_id, label_id, var_id, lo_id, hi_id],
        )
    }

    /// C++ parity: compatibility helper for `AddSoundfileInst` when URL is not provided.
    #[must_use]
    pub fn add_soundfile(&mut self, label: impl Into<String>, var: impl Into<String>) -> FirId {
        self.add_soundfile_with_url(label, "", var)
    }

    /// C++ parity: `AddSoundfileInst`.
    #[must_use]
    pub fn add_soundfile_with_url(
        &mut self,
        label: impl Into<String>,
        url: impl Into<String>,
        var: impl Into<String>,
    ) -> FirId {
        let label_id = self.store.arena.symbol(label);
        let url_id = self.store.arena.symbol(url);
        let var_id = self.store.arena.symbol(var);
        intern_tag(
            &mut self.store.arena,
            FIR_ADD_SOUNDFILE_TAG,
            &[label_id, url_id, var_id],
        )
    }

    /// C++ parity: `LoadSoundfileInst` / `fSoundN->fLength[part]`.
    /// Always returns `Int32` (length is stored as `int` in the Soundfile struct).
    #[must_use]
    pub fn load_soundfile_length(&mut self, var: impl Into<String>, part: FirId) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &FirType::Int32);
        let var_id = self.store.arena.symbol(var);
        intern_tag(
            &mut self.store.arena,
            FIR_V_LOAD_SOUNDFILE_LENGTH_TAG,
            &[typ_id, var_id, part],
        )
    }

    /// C++ parity: `LoadSoundfileInst` / `fSoundN->fSR[part]`.
    /// Always returns `Int32` (sample-rate is stored as `int` in the Soundfile struct).
    #[must_use]
    pub fn load_soundfile_rate(&mut self, var: impl Into<String>, part: FirId) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &FirType::Int32);
        let var_id = self.store.arena.symbol(var);
        intern_tag(
            &mut self.store.arena,
            FIR_V_LOAD_SOUNDFILE_RATE_TAG,
            &[typ_id, var_id, part],
        )
    }

    /// C++ parity: `LoadSoundfileInst` / `((FAUSTFLOAT**)fSoundN->fBuffers)[chan][fSoundN->fOffset[part] + idx]`.
    #[must_use]
    pub fn load_soundfile_buffer(
        &mut self,
        var: impl Into<String>,
        chan: FirId,
        part: FirId,
        idx: FirId,
        typ: FirType,
    ) -> FirId {
        let typ_id = encode_type(&mut self.store.arena, &typ);
        let var_id = self.store.arena.symbol(var);
        intern_tag(
            &mut self.store.arena,
            FIR_V_LOAD_SOUNDFILE_BUFFER_TAG,
            &[typ_id, var_id, chan, part, idx],
        )
    }

    /// C++ parity: `AddMetaDeclareInst`.
    #[must_use]
    pub fn add_meta_declare(
        &mut self,
        var: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> FirId {
        let var_id = self.store.arena.symbol(var);
        let key_id = self.store.arena.symbol(key);
        let value_id = self.store.arena.symbol(value);
        intern_tag(
            &mut self.store.arena,
            FIR_ADD_META_DECLARE_TAG,
            &[var_id, key_id, value_id],
        )
    }

    /// C++ parity: `LabelInst`.
    #[must_use]
    pub fn label(&mut self, label: impl Into<String>) -> FirId {
        let label_id = self.store.arena.symbol(label);
        intern_tag(&mut self.store.arena, FIR_LABEL_TAG, &[label_id])
    }
}
