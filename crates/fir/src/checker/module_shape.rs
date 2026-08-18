//! `module_shape` rules of the FIR checker.
//!
//! Module shape: sub-modules, the DSP struct, globals, function registration, the compute I/O contract.
//!
//! Split out of `checker.rs` on 2026-08-18, where all 67 methods sat in one
//! 2674-line `impl`. Bodies are moved verbatim; the only edit is visibility,
//! private to `pub(super)`, so sibling rule modules can still call across.

use super::*;

impl<'s> VerifyCtx<'s> {
    /// Validates the sub-module block of a module or of another sub-module.
    ///
    /// Sub-modules are the table generators of
    /// `porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`.
    /// They are checked structurally here — shape, entry points, uniqueness —
    /// while the fill/read ordering contract lives in
    /// [`Self::check_table_fill_coverage`] (FIR-SM01), which needs the module's
    /// lifecycle bodies and therefore runs from the module level.
    pub(super) fn check_sub_modules(&mut self, block_id: FirId, stmts: Vec<FirId>) {
        for stmt_id in stmts {
            let FirMatch::SubModule {
                name,
                elem_type,
                functions,
                sub_modules,
                dsp_struct,
                static_decls,
                globals,
            } = match_fir(self.store, stmt_id)
            else {
                self.error(
                    "FIR-SM02",
                    "sub_modules block contains a node that is not a SubModule",
                    stmt_id,
                );
                continue;
            };

            // FIR-SM04: names identify generated classes and their two entry
            // points, so a duplicate would collapse two generators onto one
            // table filler.
            if !self.sub_module_names.insert(name.clone()) {
                self.error(
                    "FIR-SM04",
                    format!("duplicate sub-module name '{name}'"),
                    stmt_id,
                );
            }

            for (section, section_id) in [
                ("dsp_struct", dsp_struct),
                ("static_decls", static_decls),
                ("globals", globals),
                ("functions", functions),
            ] {
                if !matches!(match_fir(self.store, section_id), FirMatch::Block(_)) {
                    self.error(
                        "FIR-SM02",
                        format!("sub-module '{name}' section '{section}' is not a Block"),
                        section_id,
                    );
                }
            }

            // The parent's `staticInit`/`instanceConstants` call these, so
            // their signatures must be visible when those bodies are checked.
            if let FirMatch::Block(items) = match_fir(self.store, functions) {
                for item in items {
                    if let FirMatch::DeclareFun {
                        name: fun_name,
                        typ,
                        args,
                        body,
                        ..
                    } = match_fir(self.store, item)
                    {
                        self.register_function_signature(
                            item, &fun_name, &typ, &args, body, None, false,
                        );
                    }
                }
            }

            self.check_sub_module_entry_points(stmt_id, &name, &elem_type, functions);
            self.check_nested_fill_coverage(&name, functions, sub_modules);

            // Nested generators: a sub-module that reads another generated
            // table owns that table's sub-module in turn.
            match match_fir(self.store, sub_modules) {
                FirMatch::Block(nested) => self.check_sub_modules(sub_modules, nested),
                _ => self.error(
                    "FIR-SM02",
                    format!("sub-module '{name}' sub_modules is not a Block"),
                    sub_modules,
                ),
            }
        }
        let _ = block_id;
    }
    /// FIR-SM02/SM03: a sub-module exposes exactly `instanceInit{name}` and
    /// `fill{name}`, and its fill writes only through the `table` argument.
    pub(super) fn check_sub_module_entry_points(
        &mut self,
        node: FirId,
        name: &str,
        elem_type: &FirType,
        functions: FirId,
    ) {
        let FirMatch::Block(items) = match_fir(self.store, functions) else {
            return;
        };
        let expected_init = format!("instanceInit{name}");
        let expected_fill = format!("fill{name}");
        let mut found_init = false;
        let mut found_fill = false;

        for item in items {
            let FirMatch::DeclareFun {
                name: fun_name,
                args,
                body,
                ..
            } = match_fir(self.store, item)
            else {
                self.error(
                    "FIR-SM02",
                    format!("sub-module '{name}' functions block holds a non-function node"),
                    item,
                );
                continue;
            };
            if fun_name == expected_init {
                found_init = true;
            } else if fun_name == expected_fill {
                found_fill = true;
                self.check_fill_signature(item, name, elem_type, &args);
                if let Some(body) = body {
                    self.check_fill_writes_only_table(body, name);
                }
            } else {
                // A `compute` here would mean the generator was lowered as an
                // ordinary DSP and would never fill anything.
                self.error(
                    "FIR-SM02",
                    format!(
                        "sub-module '{name}' declares unexpected function '{fun_name}'; \
                         expected only '{expected_init}' and '{expected_fill}'"
                    ),
                    item,
                );
            }
        }

        if !found_init {
            self.error(
                "FIR-SM02",
                format!("sub-module '{name}' is missing '{expected_init}'"),
                node,
            );
        }
        if !found_fill {
            self.error(
                "FIR-SM02",
                format!("sub-module '{name}' is missing '{expected_fill}'"),
                node,
            );
        }
    }
    /// Validates `dsp_struct` layout and records declared struct field names.
    pub(super) fn check_dsp_struct(&mut self, id: FirId) {
        let FirMatch::Block(stmts) = match_fir(self.store, id) else {
            self.error("FIR-M02", "dsp_struct is not a Block", id);
            return;
        };

        let mut seen = HashSet::new();
        let mut field_types_by_name = HashMap::new();
        let mut field_types = Vec::new();
        for stmt_id in stmts {
            let (field_name, field_type) = match match_fir(self.store, stmt_id) {
                FirMatch::DeclareVar {
                    name, typ, access, ..
                } => {
                    if access != AccessType::Struct {
                        self.error(
                            "FIR-S01",
                            format!(
                                "dsp_struct field '{name}' has access type {access:?}, expected Struct"
                            ),
                            stmt_id,
                        );
                    }
                    (name, typ)
                }
                FirMatch::DeclareTable {
                    name,
                    access,
                    elem_type,
                    ..
                } => {
                    if access != AccessType::Struct {
                        self.error(
                            "FIR-S01",
                            format!(
                                "dsp_struct table '{name}' has access type {access:?}, expected Struct"
                            ),
                            stmt_id,
                        );
                    }
                    (name, elem_type)
                }
                _ => {
                    self.error(
                        "FIR-S01",
                        "dsp_struct contains a node that is not DeclareVar or DeclareTable",
                        stmt_id,
                    );
                    continue;
                }
            };

            if !seen.insert(field_name.clone()) {
                self.error(
                    "FIR-S02",
                    format!("duplicate dsp_struct field name '{field_name}'"),
                    stmt_id,
                );
            }

            match &field_type {
                FirType::Void => {
                    self.error(
                        "FIR-S03",
                        format!("dsp_struct field '{field_name}' has Void type"),
                        stmt_id,
                    );
                }
                FirType::Array(_, 0) => {
                    self.warn(
                        "FIR-S04",
                        format!("dsp_struct array field '{field_name}' has size 0"),
                        stmt_id,
                    );
                }
                _ => {}
            }
            field_types_by_name.insert(field_name.clone(), field_type.clone());
            field_types.push(field_type);
        }

        self.symbols.struct_field_names = seen;
        self.symbols.struct_field_types = field_types_by_name;
        self.symbols.struct_fields = field_types;
    }
    /// Validates `globals` declarations and registers global symbols/functions.
    ///
    /// `globals` may contain variable/table declarations and prototype-only
    /// `DeclareFun` externs (for example math functions used by FIR calls).
    pub(super) fn check_globals(&mut self, _block_id: FirId, stmts: Vec<FirId>) {
        let mut seen: HashSet<String> = HashSet::new();

        for stmt_id in stmts {
            match match_fir(self.store, stmt_id) {
                FirMatch::DeclareVar {
                    name, typ, access, ..
                } => {
                    if !matches!(access, AccessType::Static | AccessType::Global) {
                        self.error(
                            "FIR-G02",
                            format!(
                                "global variable '{name}' has access type {access:?}, \
                                 expected Static or Global"
                            ),
                            stmt_id,
                        );
                    }
                    if seen.insert(name.clone()) {
                        self.symbols.globals.insert(name, (access, typ));
                    } else {
                        self.error(
                            "FIR-G03",
                            format!("duplicate global variable name '{name}'"),
                            stmt_id,
                        );
                    }
                }
                FirMatch::DeclareTable {
                    name,
                    access,
                    elem_type,
                    ..
                } => {
                    if !matches!(access, AccessType::Static | AccessType::Global) {
                        self.error(
                            "FIR-G02",
                            format!(
                                "global table '{name}' has access type {access:?}, \
                                 expected Static or Global"
                            ),
                            stmt_id,
                        );
                    }
                    if seen.insert(name.clone()) {
                        self.symbols.global_tables.insert(name.clone());
                        self.symbols.globals.insert(name, (access, elem_type));
                    } else {
                        self.error(
                            "FIR-G03",
                            format!("duplicate global variable name '{name}'"),
                            stmt_id,
                        );
                    }
                }
                FirMatch::DeclareFun {
                    name,
                    typ,
                    args,
                    body,
                    ..
                } => {
                    self.register_function_signature(
                        stmt_id, &name, &typ, &args, body, None, false,
                    );
                }
                _ => {
                    self.error(
                        "FIR-G01",
                        "globals block contains a node that is not DeclareVar, DeclareTable, or DeclareFun",
                        stmt_id,
                    );
                }
            }
        }
    }
    /// Validates the module `functions` block and registers all signatures.
    pub(super) fn check_functions(
        &mut self,
        _block_id: FirId,
        stmts: Vec<FirId>,
        _module_name: &str,
    ) {
        let mut seen: HashSet<String> = HashSet::new();

        for stmt_id in stmts {
            let FirMatch::DeclareFun {
                name,
                typ,
                args,
                body,
                ..
            } = match_fir(self.store, stmt_id)
            else {
                self.error(
                    "FIR-M05",
                    "functions block contains a non-DeclareFun node",
                    stmt_id,
                );
                continue;
            };

            self.register_function_signature(
                stmt_id,
                &name,
                &typ,
                &args,
                body,
                Some(&mut seen),
                true,
            );
        }

        for &api_fn in DSP_API_FUNCTIONS {
            if !self.symbols.functions.contains_key(api_fn) {
                self.warn(
                    "FIR-M07",
                    format!("expected DSP API function '{api_fn}' is not declared"),
                    self.module_id,
                );
            }
        }

        self.check_execution_entry_points();
    }
    /// Validates one `DeclareFun` signature and stores it in `symbols.functions`.
    ///
    /// This helper is shared by `globals` (extern prototypes) and
    /// `functions` (regular function declarations). It validates function
    /// signature shape, records signatures into `symbols.functions`, and
    /// optionally tracks duplicate names in `seen_names`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn register_function_signature(
        &mut self,
        stmt_id: FirId,
        name: &str,
        typ: &FirType,
        args: &[NamedType],
        body: Option<FirId>,
        seen_names: Option<&mut HashSet<String>>,
        warn_extern: bool,
    ) {
        if let Some(seen) = seen_names
            && !seen.insert(name.to_string())
        {
            self.warn(
                "FIR-M06",
                format!("duplicate function name '{name}'"),
                stmt_id,
            );
        }

        let FirType::Fun {
            args: param_types,
            ret,
        } = typ
        else {
            self.error(
                "FIR-F01",
                format!("function '{name}' has type that is not FirType::Fun"),
                stmt_id,
            );
            return;
        };

        let mut param_names: HashSet<String> = HashSet::new();
        let mut params_list: Vec<(String, FirType)> = Vec::with_capacity(args.len());
        for arg in args {
            if !param_names.insert(arg.name.clone()) {
                self.error(
                    "FIR-F04",
                    format!(
                        "function '{name}' has duplicate parameter name '{}'",
                        arg.name
                    ),
                    stmt_id,
                );
            }
            params_list.push((arg.name.clone(), arg.typ.clone()));
        }

        if name == "compute" {
            if **ret != FirType::Void {
                self.warn(
                    "FIR-F05",
                    format!("'compute' return type should be Void, got {ret:?}"),
                    stmt_id,
                );
            }
            if param_types.len() != 4 {
                self.warn(
                    "FIR-F06",
                    format!(
                        "'compute' should have 4 parameters \
                         (dsp*, count, inputs, outputs), got {}",
                        param_types.len()
                    ),
                    stmt_id,
                );
            }
        }

        let is_extern = body.is_none();
        if is_extern && warn_extern {
            self.warn(
                "FIR-F07",
                format!("function '{name}' has no body (prototype/extern declaration)"),
                stmt_id,
            );
        }

        self.symbols
            .functions
            .entry(name.to_string())
            .or_insert_with(|| FunctionSig {
                params: params_list,
                return_type: *ret.clone(),
                is_extern,
                body,
            });
    }
    /// Checks that `compute` body aliases and `inputs[]`/`outputs[]` indices
    /// stay within the module-level `(num_inputs, num_outputs)` contract.
    pub(super) fn check_compute_io_arity_contract(
        &mut self,
        functions: FirId,
        num_inputs: usize,
        num_outputs: usize,
    ) {
        let FirMatch::Block(items) = match_fir(self.store, functions) else {
            return;
        };
        for item in items {
            let FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } = match_fir(self.store, item)
            else {
                continue;
            };
            if name != "compute" {
                continue;
            }
            self.check_compute_body_io_access(body, num_inputs, num_outputs);
            break;
        }
    }
    /// Recursively walks the `compute` body and emits:
    /// - `FIR-M08` when an input alias/index exceeds `num_inputs`
    /// - `FIR-M09` when an output alias/index exceeds `num_outputs`.
    pub(super) fn check_compute_body_io_access(
        &mut self,
        id: FirId,
        num_inputs: usize,
        num_outputs: usize,
    ) {
        match match_fir(self.store, id) {
            FirMatch::Block(items) => {
                for stmt in items {
                    self.check_compute_body_io_access(stmt, num_inputs, num_outputs);
                }
            }
            FirMatch::DeclareVar { name, init, .. } => {
                if let Some(index) = input_alias_index(name.as_str())
                    && index >= num_inputs
                {
                    self.error(
                        "FIR-M08",
                        format!(
                            "compute declares input alias '{name}' out of module input arity {num_inputs}"
                        ),
                        id,
                    );
                }
                if let Some(index) = output_alias_index(name.as_str())
                    && index >= num_outputs
                {
                    self.error(
                        "FIR-M09",
                        format!(
                            "compute declares output alias '{name}' out of module output arity {num_outputs}"
                        ),
                        id,
                    );
                }
                if let Some(init) = init {
                    self.check_compute_body_io_access(init, num_inputs, num_outputs);
                }
            }
            FirMatch::LoadTable {
                name,
                access: AccessType::FunArgs,
                index,
                ..
            } => {
                if let Some(index) = funargs_constant_index(self.store, index) {
                    if name == "inputs" && index >= num_inputs {
                        self.error(
                            "FIR-M08",
                            format!(
                                "compute reads inputs[{index}] but module has {num_inputs} inputs"
                            ),
                            id,
                        );
                    } else if name == "outputs" && index >= num_outputs {
                        self.error(
                            "FIR-M09",
                            format!(
                                "compute reads outputs[{index}] but module has {num_outputs} outputs"
                            ),
                            id,
                        );
                    }
                }
                self.check_compute_body_io_access(index, num_inputs, num_outputs);
            }
            FirMatch::StoreTable { index, value, .. } => {
                self.check_compute_body_io_access(index, num_inputs, num_outputs);
                self.check_compute_body_io_access(value, num_inputs, num_outputs);
            }
            FirMatch::SimpleForLoop { upper, body, .. } => {
                self.check_compute_body_io_access(upper, num_inputs, num_outputs);
                self.check_compute_body_io_access(body, num_inputs, num_outputs);
            }
            FirMatch::ForLoop {
                init,
                end,
                step,
                body,
                ..
            } => {
                self.check_compute_body_io_access(init, num_inputs, num_outputs);
                self.check_compute_body_io_access(end, num_inputs, num_outputs);
                self.check_compute_body_io_access(step, num_inputs, num_outputs);
                self.check_compute_body_io_access(body, num_inputs, num_outputs);
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                self.check_compute_body_io_access(then_block, num_inputs, num_outputs);
                if let Some(else_block) = else_block {
                    self.check_compute_body_io_access(else_block, num_inputs, num_outputs);
                }
            }
            FirMatch::Control { cond, stmt } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                self.check_compute_body_io_access(stmt, num_inputs, num_outputs);
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                for (_, block) in cases {
                    self.check_compute_body_io_access(block, num_inputs, num_outputs);
                }
                if let Some(default) = default {
                    self.check_compute_body_io_access(default, num_inputs, num_outputs);
                }
            }
            FirMatch::WhileLoop { cond, body } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                self.check_compute_body_io_access(body, num_inputs, num_outputs);
            }
            FirMatch::BinOp { lhs, rhs, .. } => {
                self.check_compute_body_io_access(lhs, num_inputs, num_outputs);
                self.check_compute_body_io_access(rhs, num_inputs, num_outputs);
            }
            FirMatch::Neg { value, .. }
            | FirMatch::Cast { value, .. }
            | FirMatch::Bitcast { value, .. }
            | FirMatch::StoreVar { value, .. }
            | FirMatch::TeeVar { value, .. }
            | FirMatch::Drop(value)
            | FirMatch::Return(Some(value)) => {
                self.check_compute_body_io_access(value, num_inputs, num_outputs);
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                ..
            } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                self.check_compute_body_io_access(then_value, num_inputs, num_outputs);
                self.check_compute_body_io_access(else_value, num_inputs, num_outputs);
            }
            FirMatch::FunCall { args, .. } | FirMatch::ValueArray { values: args, .. } => {
                for arg in args {
                    self.check_compute_body_io_access(arg, num_inputs, num_outputs);
                }
            }
            FirMatch::DeclareTable { values, .. } => {
                for value in values {
                    self.check_compute_body_io_access(value, num_inputs, num_outputs);
                }
            }
            _ => {}
        }
    }
}
