//! `statements` rules of the FIR checker.
//!
//! Rules for statements and control flow: declarations, stores, loops, branches, returns.
//!
//! Split out of `checker.rs` on 2026-08-18, where all 67 methods sat in one
//! 2674-line `impl`. Bodies are moved verbatim; the only edit is visibility,
//! private to `pub(super)`, so sibling rule modules can still call across.

use super::*;

impl<'s> VerifyCtx<'s> {
    /// Traverses one statement node and dispatches statement-level checks.
    ///
    /// This method is the main recursive entry point for Phase 2/3 body walks.
    pub(super) fn check_stmt(&mut self, id: FirId) {
        match match_fir(self.store, id) {
            FirMatch::Block(stmts) => self.check_block(stmts),
            FirMatch::DeclareVar {
                name,
                typ,
                access,
                init,
            } => {
                self.check_declare_var(id, name, typ, access, init);
            }
            FirMatch::DeclareTable {
                name,
                access,
                elem_type,
                values,
            } => {
                for v in values {
                    self.check_value(v);
                }
                self.scope_stack.declare_table(name, elem_type, access);
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                self.check_value(value);
                self.check_required_value(id, value, "StoreVar value");
                self.check_store_var(id, &name, access);
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                self.check_value(index);
                self.check_value(value);
                self.check_store_var(id, &name, access);
                self.check_store_table_types(id, &name, access, index, value);
            }
            FirMatch::ShiftArrayVar { name, access, .. } => {
                // ShiftArrayVar modifies an array variable in-place; treat as a store.
                self.check_store_var(id, &name, access);
            }
            FirMatch::Drop(val) => {
                self.check_value(val);
                self.check_fun_call_drop_use(id, val);
                self.check_pure_drop(id, val);
            }
            FirMatch::Return(val) => self.check_return(id, val),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                self.check_value(cond);
                self.check_int_or_bool_condition(id, cond, "FIR-C04", "If");
                self.check_if(then_block, else_block);
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                ..
            } => {
                self.check_for_loop(id, &var, init, end, step, body);
            }
            FirMatch::SimpleForLoop {
                var, upper, body, ..
            } => {
                self.check_simple_for_loop(id, &var, upper, body);
            }
            FirMatch::IteratorForLoop {
                iterators, body, ..
            } => {
                self.check_iterator_for_loop(body, &iterators);
            }
            FirMatch::WhileLoop { cond, body } => {
                self.check_value(cond);
                self.check_int_or_bool_condition(id, cond, "FIR-L03", "WhileLoop");
                self.scope_stack.push(FrameKind::Block);
                self.check_stmt(body);
                self.scope_stack.pop();
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                self.check_value(cond);
                self.check_switch_condition_type(id, cond);
                self.check_switch(id, cases, default);
            }
            FirMatch::Control { cond, stmt } => {
                self.check_value(cond);
                self.check_stmt(stmt);
            }
            FirMatch::AddSoundfile { var, .. } => {
                self.check_soundfile_slot(id, &var);
            }
            // UI, meta, null — no scope-relevant content
            _ => {}
        }
    }
    /// Verifies a lexical `Block` with a fresh frame and return-flow tracking.
    pub(super) fn check_block(&mut self, stmts: Vec<FirId>) {
        self.scope_stack.push(FrameKind::Block);
        let mut returned = false;
        for (i, stmt_id) in stmts.iter().enumerate() {
            if returned {
                // R03: dead code after Return
                self.warn("FIR-R03", "unreachable statement after Return", *stmt_id);
                break;
            }
            // Detect a Return statement to set the `returned` flag for the next iteration.
            if matches!(match_fir(self.store, *stmt_id), FirMatch::Return(_)) {
                returned = true;
                // Still check the Return itself.
                let _ = i;
            }
            self.check_stmt(*stmt_id);
        }
        self.scope_stack.pop();
    }
    /// Registers and validates a local `DeclareVar` inside a function body.
    pub(super) fn check_declare_var(
        &mut self,
        id: FirId,
        name: String,
        typ: FirType,
        access: AccessType,
        init: Option<FirId>,
    ) {
        // SC07: kFunArgs must not be re-declared inside a function body
        if access == AccessType::FunArgs {
            self.error(
                "FIR-SC07",
                format!("kFunArgs variable '{name}' re-declared inside function body"),
                id,
            );
        }
        if !matches!(
            access,
            AccessType::Stack | AccessType::Loop | AccessType::FunArgs
        ) {
            self.error(
                "FIR-SC10",
                format!(
                    "local DeclareVar '{name}' uses non-local access type {access:?} \
                     (expected Stack or Loop)"
                ),
                id,
            );
        }

        if let Some(init_id) = init {
            self.check_value(init_id);
            self.check_required_value(id, init_id, "DeclareVar initializer");
        }

        let init_status = if init.is_some() {
            InitStatus::Yes
        } else {
            InitStatus::No
        };
        self.scope_stack.declare(name, typ, access, init_status);
    }
    /// Validates a variable load (`LoadVar` / `LoadVarAddress`) against scope state.
    pub(super) fn check_load_var(&mut self, id: FirId, name: &str, access: AccessType) {
        if access == AccessType::Struct && !self.symbols.struct_field_names.contains(name) {
            self.warn(
                "FIR-SC09",
                format!("kStruct variable '{name}' is not declared in dsp_struct"),
                id,
            );
            return;
        }
        match self.resolve(name, access) {
            None => {
                if let Some(entry) = self.resolve_any_by_name(name) {
                    self.error(
                        "FIR-SC02",
                        format!(
                            "variable '{name}' accessed as {access:?} \
                             but declared as {:?}",
                            entry.access
                        ),
                        id,
                    );
                } else {
                    // SC01: variable not declared
                    self.error(
                        "FIR-SC01",
                        format!("use of undeclared variable '{name}'"),
                        id,
                    );
                }
            }
            Some(entry) => {
                // SC02: access type must match declaration
                if entry.access != access {
                    self.error(
                        "FIR-SC02",
                        format!(
                            "variable '{name}' accessed as {access:?} \
                             but declared as {:?}",
                            entry.access
                        ),
                        id,
                    );
                }
                // SC03: warn if kStack variable is uninitialized
                if access == AccessType::Stack && entry.init == InitStatus::No {
                    self.warn(
                        "FIR-SC03",
                        format!("variable '{name}' may be used before initialization"),
                        id,
                    );
                }
            }
        }
    }
    /// Validates a variable store target and updates initialization state.
    pub(super) fn check_store_var(&mut self, id: FirId, name: &str, access: AccessType) {
        if access == AccessType::Struct && !self.symbols.struct_field_names.contains(name) {
            self.warn(
                "FIR-SC09",
                format!("kStruct variable '{name}' is not declared in dsp_struct"),
                id,
            );
            return;
        }
        match self.resolve(name, access) {
            None => {
                if let Some(entry) = self.resolve_any_by_name(name) {
                    self.error(
                        "FIR-SC05",
                        format!(
                            "variable '{name}' stored as {access:?} \
                             but declared as {:?}",
                            entry.access
                        ),
                        id,
                    );
                } else {
                    self.error(
                        "FIR-SC04",
                        format!("store to undeclared variable '{name}'"),
                        id,
                    );
                }
            }
            Some(entry) => {
                // SC05: access type must match declaration
                if entry.access != access {
                    self.error(
                        "FIR-SC05",
                        format!(
                            "variable '{name}' stored as {access:?} \
                             but declared as {:?}",
                            entry.access
                        ),
                        id,
                    );
                }
                // Mark as initialized (for kStack / kLoop vars in the scope stack)
                if matches!(access, AccessType::Stack | AccessType::Loop) {
                    self.scope_stack.mark_initialized(name);
                }
            }
        }
    }
    /// Validates a full `ForLoop` statement and checks its body in a loop frame.
    pub(super) fn check_for_loop(
        &mut self,
        id: FirId,
        var: &str,
        init: FirId,
        end: FirId,
        step: FirId,
        body: FirId,
    ) {
        // L01 / L02: the init node should be DeclareVar(kLoop)
        match match_fir(self.store, init) {
            FirMatch::DeclareVar {
                name: ref decl_name,
                ref typ,
                access,
                ..
            } if decl_name == var => {
                if access != AccessType::Loop {
                    self.error(
                        "FIR-L01",
                        format!(
                            "ForLoop variable '{var}' init is not a kLoop DeclareVar \
                             (got {access:?})"
                        ),
                        init,
                    );
                }
                if !matches!(typ, FirType::Int32 | FirType::Int64) {
                    self.error(
                        "FIR-L02",
                        format!(
                            "ForLoop variable '{var}' type should be Int32 or Int64, \
                             got {typ:?}"
                        ),
                        init,
                    );
                }
            }
            _ => {
                // init is not a DeclareVar for the expected loop variable
                self.error(
                    "FIR-L01",
                    format!("ForLoop '{var}' init is not a DeclareVar for the loop variable"),
                    init,
                );
            }
        }

        // Push a Loop frame containing the loop variable
        self.scope_stack.push(FrameKind::Loop);

        // Process the init statement (registers the loop variable in the loop frame)
        self.check_stmt(init);

        // Traverse end condition and step
        self.check_value(end);
        self.check_stmt(step);

        // L04: body must be non-empty
        if let FirMatch::Block(ref stmts) = match_fir(self.store, body)
            && stmts.is_empty()
        {
            self.warn("FIR-L04", format!("ForLoop '{var}' body is empty"), id);
        }
        self.check_stmt(body);

        self.scope_stack.pop();
    }
    /// Validates a `SimpleForLoop` and introduces its implicit loop variable.
    pub(super) fn check_simple_for_loop(
        &mut self,
        id: FirId,
        var: &str,
        upper: FirId,
        body: FirId,
    ) {
        self.scope_stack.push(FrameKind::Loop);

        // Implicit loop variable: kLoop, Int32, initialized
        self.scope_stack.declare(
            var.to_string(),
            FirType::Int32,
            AccessType::Loop,
            InitStatus::Yes,
        );

        self.check_value(upper);

        // L04: body must be non-empty
        if let FirMatch::Block(ref stmts) = match_fir(self.store, body)
            && stmts.is_empty()
        {
            self.warn(
                "FIR-L04",
                format!("SimpleForLoop '{var}' body is empty"),
                id,
            );
        }
        self.check_stmt(body);

        self.scope_stack.pop();
    }
    /// Validates an `IteratorForLoop` by predeclaring all iterator names as loop vars.
    pub(super) fn check_iterator_for_loop(&mut self, body: FirId, iterators: &[String]) {
        self.scope_stack.push(FrameKind::Loop);
        for iter in iterators {
            self.scope_stack.declare(
                iter.clone(),
                FirType::Int32,
                AccessType::Loop,
                InitStatus::Yes,
            );
        }
        self.check_stmt(body);
        self.scope_stack.pop();
    }
    /// Verifies both branches of an `If` and merges variable init states.
    ///
    /// Declarations remain branch-local; only initialization information for
    /// pre-existing variables is merged back into the outer frame.
    pub(super) fn check_if(&mut self, then_block: FirId, else_block: Option<FirId>) {
        let pre = self.scope_stack.snapshot_inits();

        // Traverse then branch
        self.scope_stack.push(FrameKind::Block);
        self.check_stmt(then_block);
        self.scope_stack.pop();
        let then_changes = self.scope_stack.diff_inits(&pre);
        self.scope_stack.restore_inits(&pre);

        // Traverse else branch
        let else_changes = if let Some(else_id) = else_block {
            self.scope_stack.push(FrameKind::Block);
            self.check_stmt(else_id);
            self.scope_stack.pop();
            let changes = self.scope_stack.diff_inits(&pre);
            self.scope_stack.restore_inits(&pre);
            changes
        } else {
            Vec::new()
        };

        // Merge: both branches initialized the var → Yes; only one → Maybe
        let all: HashSet<(usize, String)> = then_changes
            .iter()
            .chain(else_changes.iter())
            .cloned()
            .collect();
        for (fi, name) in all {
            let in_then = then_changes.contains(&(fi, name.clone()));
            let in_else = else_changes.contains(&(fi, name.clone()));
            let status = if in_then && in_else {
                InitStatus::Yes
            } else {
                InitStatus::Maybe
            };
            self.scope_stack.set_init(fi, &name, status);
        }
    }
    /// Validates return statements against the current function return type.
    pub(super) fn check_return(&mut self, id: FirId, value: Option<FirId>) {
        if let Some(val_id) = value {
            self.check_value(val_id);
            self.check_required_value(id, val_id, "Return expression");
            if let Some(ret_ty) = &self.current_return_type
                && let Some(val_ty) = self.infer_value_type(val_id)
                && val_ty != *ret_ty
            {
                self.error(
                    "FIR-R01",
                    format!(
                        "Return value type {val_ty:?} does not match function return type {ret_ty:?}"
                    ),
                    id,
                );
            }
        } else {
            // R02: Return(None) in a non-Void function
            if let Some(ret_ty) = &self.current_return_type
                && *ret_ty != FirType::Void
            {
                self.warn(
                    "FIR-R02",
                    format!(
                        "Return without value in function '{}' whose return type is {:?}",
                        self.current_function.as_deref().unwrap_or("?"),
                        ret_ty
                    ),
                    id,
                );
            }
        }
        // R03 is handled by check_block which detects statements after a Return.
    }
    /// Validates `Switch` case structure and traverses all branch bodies.
    pub(super) fn check_switch(
        &mut self,
        id: FirId,
        cases: Vec<(i64, FirId)>,
        default: Option<FirId>,
    ) {
        // SW03: at least one case
        if cases.is_empty() {
            self.warn("FIR-SW03", "Switch has no cases", id);
        }

        // SW02: no duplicate case values
        let mut seen_vals: HashSet<i64> = HashSet::new();
        for &(val, case_body) in &cases {
            if !seen_vals.insert(val) {
                self.error(
                    "FIR-SW02",
                    format!("Switch has duplicate case value {val}"),
                    id,
                );
            }
            self.scope_stack.push(FrameKind::Block);
            self.check_stmt(case_body);
            self.scope_stack.pop();
        }

        if let Some(default_body) = default {
            self.scope_stack.push(FrameKind::Block);
            self.check_stmt(default_body);
            self.scope_stack.pop();
        }
    }
}
