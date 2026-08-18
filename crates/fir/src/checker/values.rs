//! `values` rules of the FIR checker.
//!
//! Rules for value-producing constructs: operators, calls, casts, table and soundfile access.
//!
//! Split out of `checker.rs` on 2026-08-18, where all 67 methods sat in one
//! 2674-line `impl`. Bodies are moved verbatim; the only edit is visibility,
//! private to `pub(super)`, so sibling rule modules can still call across.

use super::*;

impl<'s> VerifyCtx<'s> {
    /// Shared condition-type check for `If`/`Select2`/`WhileLoop`.
    ///
    /// `code` and `what` parameterize the emitted diagnostic.
    pub(super) fn check_int_or_bool_condition(
        &mut self,
        id: FirId,
        cond: FirId,
        code: &'static str,
        what: &str,
    ) {
        if let Some(cond_ty) = self.infer_value_type(cond)
            && !self.is_int_or_bool_type(&cond_ty)
        {
            self.error(
                code,
                format!("{what} condition should be Int32, Int64, or Bool, got {cond_ty:?}"),
                id,
            );
        }
    }
    /// Specialized condition-type check for `Switch` (integers only).
    pub(super) fn check_switch_condition_type(&mut self, id: FirId, cond: FirId) {
        if let Some(cond_ty) = self.infer_value_type(cond)
            && !self.is_integer_type(&cond_ty)
        {
            self.error(
                "FIR-SW01",
                format!("Switch condition should be Int32 or Int64, got {cond_ty:?}"),
                id,
            );
        }
    }
    /// Rejects `Void`-typed expressions in positions that require a real value.
    pub(super) fn check_required_value(&mut self, id: FirId, value: FirId, what: &str) {
        if matches!(self.infer_value_type(value), Some(FirType::Void)) {
            self.error(
                "FIR-V01",
                format!("{what} must produce a non-Void value"),
                id,
            );
        }
    }
    /// Validates operand/result typing for one `BinOp` node.
    pub(super) fn check_binop_types(
        &mut self,
        id: FirId,
        op: FirBinOp,
        lhs: FirId,
        rhs: FirId,
        declared: &FirType,
    ) {
        let lhs_ty = self.infer_value_type(lhs);
        let rhs_ty = self.infer_value_type(rhs);
        let (Some(lhs_ty), Some(rhs_ty)) = (lhs_ty, rhs_ty) else {
            return;
        };

        if !self.same_or_int_bool_mix(&lhs_ty, &rhs_ty) {
            self.error(
                "FIR-B01",
                format!("BinOp operands have incompatible types: {lhs_ty:?} vs {rhs_ty:?}"),
                id,
            );
        }
        if !self.is_numeric_type(&lhs_ty) || !self.is_numeric_type(&rhs_ty) {
            self.error(
                "FIR-B02",
                format!("BinOp operands must be numeric, got {lhs_ty:?} and {rhs_ty:?}"),
                id,
            );
        }
        if let Some(expected) = self.expected_binop_result_type(op, &lhs_ty, &rhs_ty)
            && &expected != declared
        {
            self.warn(
                "FIR-B03",
                format!(
                    "BinOp declared result type {declared:?} is inconsistent with operands \
                     ({lhs_ty:?}, {rhs_ty:?}); expected {expected:?}"
                ),
                id,
            );
        }
        if op == FirBinOp::Div && self.const_is_zero(rhs) {
            self.warn("FIR-B04", "division by constant zero in BinOp", id);
        }
    }
    /// Validates unary negation operand typing.
    pub(super) fn check_neg_type(&mut self, id: FirId, value: FirId) {
        if let Some(val_ty) = self.infer_value_type(value)
            && !self.is_numeric_type(&val_ty)
        {
            self.error(
                "FIR-U01",
                format!("Neg operand must be numeric, got {val_ty:?}"),
                id,
            );
        }
    }
    /// Validates numeric cast usage and emits no-op cast warnings.
    pub(super) fn check_cast_type(&mut self, id: FirId, target: &FirType, value: FirId) {
        if let Some(src_ty) = self.infer_value_type(value) {
            if &src_ty == target {
                self.warn("FIR-U02", format!("Cast is a no-op to {target:?}"), id);
            }
            if !self.is_numeric_type(&src_ty) || !self.is_numeric_type(target) {
                self.error(
                    "FIR-U03",
                    format!(
                        "Cast requires numeric source/target types, got {src_ty:?} -> {target:?}"
                    ),
                    id,
                );
            }
        }
    }
    /// Validates bitcast width compatibility (warning-only on mismatch).
    pub(super) fn check_bitcast_type(&mut self, id: FirId, target: &FirType, value: FirId) {
        if let Some(src_ty) = self.infer_value_type(value) {
            let src_w = self.bit_width(&src_ty);
            let dst_w = self.bit_width(target);
            if let (Some(sw), Some(dw)) = (src_w, dst_w)
                && sw != dw
            {
                self.warn(
                    "FIR-U04",
                    format!("Bitcast width mismatch: {src_ty:?} ({sw}) -> {target:?} ({dw})"),
                    id,
                );
            }
        }
    }
    /// Validates `Select2` condition/branch/result typing.
    pub(super) fn check_select2_types(
        &mut self,
        id: FirId,
        cond: FirId,
        then_value: FirId,
        else_value: FirId,
        declared: &FirType,
    ) {
        self.check_int_or_bool_condition(id, cond, "FIR-C01", "Select2");
        let then_ty = self.infer_value_type(then_value);
        let else_ty = self.infer_value_type(else_value);
        if let (Some(tt), Some(et)) = (then_ty, else_ty) {
            if tt != et {
                self.warn(
                    "FIR-C02",
                    format!("Select2 branches have different types: {tt:?} vs {et:?}"),
                    id,
                );
            }
            if &tt != declared && &et != declared {
                self.warn(
                    "FIR-C03",
                    format!(
                        "Select2 declared result type {declared:?} does not match branch types \
                         ({tt:?}, {et:?})"
                    ),
                    id,
                );
            }
        }
    }
    /// Validates function call arity/signature/result typing and math-call conventions.
    pub(super) fn check_fun_call_types(
        &mut self,
        id: FirId,
        name: &str,
        args: &[FirId],
        declared: &FirType,
    ) {
        if let Some(sig) = self.symbols.functions.get(name).cloned() {
            if sig.params.len() != args.len() {
                self.error(
                    "FIR-FC02",
                    format!(
                        "call to '{name}' has {} args, expected {}",
                        args.len(),
                        sig.params.len()
                    ),
                    id,
                );
            }
            for (i, (arg_id, (_pname, pty))) in args.iter().zip(sig.params.iter()).enumerate() {
                if let Some(actual_ty) = self.infer_value_type(*arg_id)
                    && !self.types_compatible(&actual_ty, pty)
                {
                    self.warn(
                        "FIR-FC03",
                        format!(
                            "call to '{name}' arg #{i} has type {actual_ty:?}, expected {pty:?}"
                        ),
                        id,
                    );
                }
            }
            if &sig.return_type != declared {
                self.warn(
                    "FIR-FC04",
                    format!(
                        "call to '{name}' declared result type {declared:?} differs from function \
                         signature return type {:?}",
                        sig.return_type
                    ),
                    id,
                );
            }
        } else {
            self.error(
                "FIR-FC01",
                format!("call to undeclared function '{name}'"),
                id,
            );
        }

        self.check_math_call(id, name, args);
    }
    /// Applies math-specific naming/arity/argument diagnostics (`MA*`) to a call.
    ///
    /// This runs in addition to generic function-call checks and accepts both
    /// canonical and `std::`-prefixed symbols.
    pub(super) fn check_math_call(&mut self, id: FirId, name: &str, args: &[FirId]) {
        let raw = name.strip_prefix("std::").unwrap_or(name);
        let Some(op) = FirMathOp::from_symbol(name) else {
            if raw == "abs"
                && let Some(arg) = args.first().and_then(|arg| self.infer_value_type(*arg))
                && self.is_float_like_type(&arg)
            {
                self.warn(
                    "FIR-MA04",
                    "use 'fabs' for floating-point absolute value (got 'abs')",
                    id,
                );
            }
            return;
        };

        let expected_arity = match op {
            FirMathOp::Pow
            | FirMathOp::Min
            | FirMathOp::Max
            | FirMathOp::Atan2
            | FirMathOp::Fmod
            | FirMathOp::Remainder => 2,
            _ => 1,
        };
        match expected_arity {
            1 if args.len() != 1 => self.warn(
                "FIR-MA01",
                format!(
                    "math op '{}' expects 1 arg, got {}",
                    op.symbol(),
                    args.len()
                ),
                id,
            ),
            2 if args.len() != 2 => self.warn(
                "FIR-MA02",
                format!(
                    "math op '{}' expects 2 args, got {}",
                    op.symbol(),
                    args.len()
                ),
                id,
            ),
            _ => {}
        }

        let is_float_math = !matches!(op, FirMathOp::Abs);
        if is_float_math && expected_arity == args.len() {
            for (i, arg_id) in args.iter().enumerate() {
                if let Some(arg_ty) = self.infer_value_type(*arg_id)
                    && (self.is_integer_type(&arg_ty) || arg_ty == FirType::Bool)
                {
                    self.warn(
                        "FIR-MA03",
                        format!(
                            "math op '{}' arg #{i} is integer-like ({arg_ty:?}); \
                             floating-point argument expected",
                            op.symbol()
                        ),
                        id,
                    );
                }
            }
        }

        if raw == "fabs"
            && let Some(arg_ty) = args.first().and_then(|arg| self.infer_value_type(*arg))
            && (self.is_integer_type(&arg_ty) || arg_ty == FirType::Bool)
        {
            self.warn(
                "FIR-MA04",
                format!("'fabs' called with integer-like argument {arg_ty:?}"),
                id,
            );
        }
    }
    /// Resolves one table access to its declared element type.
    ///
    /// FIR-T04 (error): the name has no declaration at all - not in the scope
    /// stack, not a global, and not a DSP-struct field. An access to a name no
    /// declaration backs cannot compile in any backend, so silence here lets an
    /// invalid module through behind a green report. FIR-T03 (warning): the
    /// name is declared but not as a table or indexable container.
    pub(super) fn table_access_elem_type(
        &mut self,
        id: FirId,
        name: &str,
        access: AccessType,
        what: &str,
    ) -> Option<FirType> {
        let declared = match access {
            AccessType::Struct => self
                .symbols
                .struct_field_types
                .get(name)
                .cloned()
                .map(|typ| (typ, false)),
            _ => self
                .resolve(name, access)
                .map(|entry| (entry.typ, entry.is_table)),
        };
        let Some((typ, is_table)) = declared else {
            self.error("FIR-T04", format!("{what} '{name}' has no declaration"), id);
            return None;
        };
        let elem = if is_table {
            Some(typ.clone())
        } else {
            self.is_indexable_container_type(&typ)
        };
        if elem.is_none() {
            self.warn(
                "FIR-T03",
                format!("{what} '{name}' refers to a non-table declaration"),
                id,
            );
        }
        elem
    }
    /// Validates `LoadTable` index typing and declaration/table consistency.
    pub(super) fn check_load_table_types(
        &mut self,
        id: FirId,
        name: &str,
        access: AccessType,
        index: FirId,
        declared_elem_type: &FirType,
    ) {
        if let Some(index_ty) = self.infer_value_type(index)
            && !self.is_integer_type(&index_ty)
        {
            self.error(
                "FIR-T01",
                format!("table index must be Int32 or Int64, got {index_ty:?}"),
                id,
            );
        }
        if let Some(expected_elem_type) = self.table_access_elem_type(id, name, access, "LoadTable")
            && expected_elem_type != *declared_elem_type
        {
            self.warn(
                "FIR-T03",
                format!(
                    "LoadTable '{name}' element type {declared_elem_type:?} differs from \
                     declaration {:?}",
                    expected_elem_type
                ),
                id,
            );
        }
    }
    /// Validates `StoreTable` index typing and stored element type compatibility.
    pub(super) fn check_store_table_types(
        &mut self,
        id: FirId,
        name: &str,
        access: AccessType,
        index: FirId,
        value: FirId,
    ) {
        if let Some(index_ty) = self.infer_value_type(index)
            && !self.is_integer_type(&index_ty)
        {
            self.error(
                "FIR-T01",
                format!("table index must be Int32 or Int64, got {index_ty:?}"),
                id,
            );
        }
        if let Some(expected_elem_type) =
            self.table_access_elem_type(id, name, access, "StoreTable")
            && let Some(val_ty) = self.infer_value_type(value)
            && val_ty != expected_elem_type
        {
            self.error(
                "FIR-T02",
                format!(
                    "StoreTable value type {val_ty:?} does not match element type {:?}",
                    expected_elem_type
                ),
                id,
            );
        }
    }
    /// Validates that one `soundfile` slot name resolves to a DSP struct field
    /// of type [`FirType::Sound`].
    pub(super) fn check_soundfile_slot(&mut self, id: FirId, var: &str) {
        match self.symbols.struct_field_types.get(var) {
            Some(FirType::Sound) => {}
            Some(found) => self.warn(
                "FIR-SF01",
                format!(
                    "soundfile access '{var}' refers to struct field of type {found:?}, expected Sound"
                ),
                id,
            ),
            None => self.warn(
                "FIR-SC09",
                format!("kStruct variable '{var}' is not declared in dsp_struct"),
                id,
            ),
        }
    }
    /// Validates one soundfile subscript-like operand (`part`, `chan`, `idx`).
    pub(super) fn check_soundfile_index_like(&mut self, id: FirId, value: FirId, what: &str) {
        self.check_value(value);
        if let Some(index_ty) = self.infer_value_type(value)
            && !self.is_integer_type(&index_ty)
        {
            self.error(
                "FIR-T01",
                format!("soundfile {what} must be Int32 or Int64, got {index_ty:?}"),
                id,
            );
        }
    }
    /// Warns when a `Drop` discards a non-void function return value.
    pub(super) fn check_fun_call_drop_use(&mut self, id: FirId, value: FirId) {
        if let FirMatch::FunCall { name, typ, .. } = match_fir(self.store, value)
            && typ != FirType::Void
        {
            self.warn(
                "FIR-FC04",
                format!("discarded non-void return value from '{name}' ({typ:?})"),
                id,
            );
        }
    }
    /// Warns when a `Drop` evaluates a value with no observable side effect.
    pub(super) fn check_pure_drop(&mut self, id: FirId, value: FirId) {
        if is_obviously_side_effect_free_value(self.store, value) {
            self.warn("FIR-D01", "Drop discards a side-effect-free expression", id);
        }
    }
    /// Traverses one value expression and dispatches value-level checks.
    ///
    /// This recursively descends into expression children before applying local
    /// typing/scope checks for the current value node.
    pub(super) fn check_value(&mut self, id: FirId) {
        match match_fir(self.store, id) {
            FirMatch::LoadVar { name, access, .. } => {
                self.check_load_var(id, &name, access);
            }
            FirMatch::LoadVarAddress { name, access, .. } => {
                self.check_load_var(id, &name, access);
            }
            FirMatch::TeeVar {
                name,
                access,
                value,
                ..
            } => {
                // TeeVar = store + load: check that the target is declared
                self.check_value(value);
                self.check_required_value(id, value, "TeeVar value");
                self.check_store_var(id, &name, access);
            }
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } => {
                self.check_load_var(id, &name, access);
                self.check_value(index);
                self.check_load_table_types(id, &name, access, index, &typ);
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => {
                self.check_value(lhs);
                self.check_value(rhs);
                self.check_binop_types(id, op, lhs, rhs, &typ);
            }
            FirMatch::Neg { value, .. } => {
                self.check_value(value);
                self.check_neg_type(id, value);
            }
            FirMatch::Cast { typ, value } => {
                self.check_value(value);
                self.check_cast_type(id, &typ, value);
            }
            FirMatch::Bitcast { typ, value } => {
                self.check_value(value);
                self.check_bitcast_type(id, &typ, value);
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                typ,
            } => {
                self.check_value(cond);
                self.check_value(then_value);
                self.check_value(else_value);
                self.check_select2_types(id, cond, then_value, else_value, &typ);
            }
            FirMatch::FunCall { name, args, typ } => {
                for &arg in &args {
                    self.check_value(arg);
                }
                self.check_fun_call_types(id, &name, &args, &typ);
            }
            FirMatch::ValueArray { values, .. } => {
                for (index, v) in values.into_iter().enumerate() {
                    self.check_value(v);
                    self.check_required_value(id, v, &format!("ValueArray element #{index}"));
                }
            }
            FirMatch::LoadSoundfileLength { var, part }
            | FirMatch::LoadSoundfileRate { var, part } => {
                self.check_soundfile_slot(id, &var);
                self.check_soundfile_index_like(id, part, "part");
            }
            FirMatch::LoadSoundfileBuffer {
                var,
                chan,
                part,
                idx,
                ..
            } => {
                self.check_soundfile_slot(id, &var);
                self.check_soundfile_index_like(id, chan, "channel");
                self.check_soundfile_index_like(id, part, "part");
                self.check_soundfile_index_like(id, idx, "index");
            }
            // Leaf value nodes (literals, NullValue, NewDsp, etc.) — nothing to check
            _ => {}
        }
    }
}
