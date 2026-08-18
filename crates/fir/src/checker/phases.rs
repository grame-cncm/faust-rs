//! `phases` rules of the FIR checker.
//!
//! The two verification passes and the module-level ordering rules they enforce.
//!
//! Split out of `checker.rs` on 2026-08-18, where all 67 methods sat in one
//! 2674-line `impl`. Bodies are moved verbatim; the only edit is visibility,
//! private to `pub(super)`, so sibling rule modules can still call across.

use super::*;

impl<'s> VerifyCtx<'s> {
    /// Runs Phase 1 module checks and collects top-level symbols.
    ///
    /// This validates the `Module` skeleton (`dsp_struct`, `globals`,
    /// `functions`) and populates symbol tables consumed by phases 2/3.
    pub(super) fn check_phase1(&mut self) {
        let id = self.module_id;

        // M01: root must decode as Module
        let FirMatch::Module {
            num_inputs,
            num_outputs,
            name,
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
        } = match_fir(self.store, id)
        else {
            self.error("FIR-M01", "root node is not a FirMatch::Module", id);
            return;
        };

        // `Module.name` is the DSP class name and is used as the logical struct
        // name in the checker symbols table.
        self.symbols.struct_name = Some(name.clone());
        // M02: validate and register struct fields
        self.check_dsp_struct(dsp_struct);

        // M03: globals must be a Block
        match match_fir(self.store, globals) {
            FirMatch::Block(stmts) => self.check_globals(globals, stmts),
            _ => self.error("FIR-M03", "globals is not a Block", globals),
        }

        // M04: functions must be a Block
        match match_fir(self.store, functions) {
            FirMatch::Block(stmts) => {
                self.check_functions(functions, stmts, &name);
                self.check_compute_io_arity_contract(functions, num_inputs, num_outputs);
            }
            _ => self.error("FIR-M04", "functions is not a Block", functions),
        }

        // M05: static_decls must be a Block of Static/Global table declarations.
        // Reuse check_globals — it already validates AccessType::Static and
        // registers names into symbols.globals so that load_table(Static)
        // accesses in compute resolve correctly.
        match match_fir(self.store, static_decls) {
            FirMatch::Block(stmts) => self.check_globals(static_decls, stmts),
            _ => self.error("FIR-M05", "static_decls is not a Block", static_decls),
        }

        // FIR-SM02..SM04: sub_modules must be a Block of SubModule nodes.
        match match_fir(self.store, sub_modules) {
            FirMatch::Block(stmts) => self.check_sub_modules(sub_modules, stmts),
            _ => self.error("FIR-SM02", "sub_modules is not a Block", sub_modules),
        }
    }
    /// Runs Phase 2/3 on every function body declared in the module.
    ///
    /// Only functions with a body are traversed. Prototype-only `DeclareFun`
    /// nodes contribute symbols during phase 1 but are not walked here.
    pub(super) fn check_phase2(&mut self) {
        // Bail out if Phase 1 found a broken module skeleton.
        let FirMatch::Module { functions, .. } = match_fir(self.store, self.module_id) else {
            return;
        };
        let FirMatch::Block(stmts) = match_fir(self.store, functions) else {
            return;
        };

        for stmt_id in stmts {
            if let FirMatch::DeclareFun {
                name,
                typ,
                args,
                body: Some(body_id),
                ..
            } = match_fir(self.store, stmt_id)
            {
                let FirType::Fun { ret, .. } = typ else {
                    continue;
                };
                self.enter_function(
                    name,
                    *ret,
                    args.iter().map(|a| (a.name.clone(), a.typ.clone())),
                );
                self.check_stmt(body_id);
                self.leave_function();
            }
        }

        self.check_lifecycle_order();
    }
    /// **FIR-LC01** — detect struct fields read in `instanceConstants` that are
    /// only initialized in `instanceClear`.
    ///
    /// The standard DSP lifecycle is:
    /// 1. `instanceConstants(sample_rate)` — compute derived constants from SR
    /// 2. `instanceResetUserInterface()` — reset UI zones to defaults
    /// 3. `instanceClear()` — zero-initialize state arrays and counters
    /// 4. `compute(count, inputs, outputs)` — per-block DSP loop
    ///
    /// Any struct field that is **not** stored anywhere inside `instanceConstants`
    /// but **is** stored inside `instanceClear` will still hold its default
    /// zero-initialized value from C++ allocation when `instanceConstants` reads
    /// it.  In practice this means waveform index counters (e.g. `iWave48`)
    /// appear explicitly initialized only in `instanceClear`, yet a misplaced
    /// hoisting decision may cause `instanceConstants` to read them as if they
    /// had already been set — producing wrong constant values.
    ///
    /// The check emits FIR-LC01 for every `LoadVar(kStruct, name)` in
    /// `instanceConstants` where `name` ∈ (written-only-in-clear) set.
    pub(super) fn check_lifecycle_order(&mut self) {
        // Locate instanceConstants and instanceClear function bodies.
        let FirMatch::Module { functions, .. } = match_fir(self.store, self.module_id) else {
            return;
        };
        let FirMatch::Block(stmts) = match_fir(self.store, functions) else {
            return;
        };

        let mut constants_body: Option<FirId> = None;
        let mut constants_fun_id: Option<FirId> = None;
        let mut clear_body: Option<FirId> = None;

        for stmt_id in &stmts {
            if let FirMatch::DeclareFun {
                name,
                body: Some(body_id),
                ..
            } = match_fir(self.store, *stmt_id)
            {
                match name.as_str() {
                    "instanceConstants" => {
                        constants_body = Some(body_id);
                        constants_fun_id = Some(*stmt_id);
                    }
                    "instanceClear" => {
                        clear_body = Some(body_id);
                    }
                    _ => {}
                }
            }
        }

        let (Some(constants_body), Some(constants_fun_id), Some(clear_body)) =
            (constants_body, constants_fun_id, clear_body)
        else {
            // One or both functions missing — other checks cover that.
            return;
        };

        // Fields written in instanceConstants (safely computed before any read).
        let constants_stores = collect_struct_stores(self.store, constants_body);
        // Fields written in instanceClear.
        let clear_stores = collect_struct_stores(self.store, clear_body);

        // Fields that instanceClear initializes but instanceConstants never writes
        // — reading them in instanceConstants yields an uninitialized value.
        let cleared_only: HashSet<&String> = clear_stores
            .iter()
            .filter(|n| !constants_stores.contains(*n))
            .collect();

        if cleared_only.is_empty() {
            return;
        }

        // Walk instanceConstants body for loads of those fields.
        let loads = collect_struct_loads(self.store, constants_body);
        for (field_name, load_id) in loads {
            if cleared_only.contains(&field_name) {
                self.error(
                    "FIR-LC01",
                    format!(
                        "struct field '{field_name}' is read in `instanceConstants` but is only \
                         initialized in `instanceClear` (which runs later); value is \
                         zero-initialized at this point"
                    ),
                    load_id,
                );
                // Also tag the diagnostic with the enclosing function name.
                if let Some(d) = self.diags.last_mut() {
                    d.context.function_name = Some("instanceConstants".to_owned());
                    d.context.variable_name = Some(field_name);
                }
            }
        }

        let _ = constants_fun_id; // available for future use
    }
    /// Execution-options port plan §4.5: when the optional `control`/`frame`
    /// entry points are declared, enforce their structural contract.
    ///
    /// - `frame` processes exactly one sample, so the canonical block
    ///   `compute` must be emitted empty (FIR-F08): the reference compiler
    ///   never makes `compute` delegate to `frame`.
    /// - Neither `control` nor `frame` receives a block count, so their
    ///   bodies must not reference the `count` argument (FIR-F09).
    pub(super) fn check_execution_entry_points(&mut self) {
        let frame_declared = self.symbols.functions.contains_key("frame");
        if frame_declared
            && let Some(compute) = self.symbols.functions.get("compute")
            && let Some(body) = compute.body
            && !matches!(match_fir(self.store, body), FirMatch::Block(stmts) if stmts.is_empty())
        {
            self.warn(
                "FIR-F08",
                "'frame' is declared but the canonical 'compute' body is not empty".to_owned(),
                body,
            );
        }
        for name in ["control", "frame"] {
            let Some(body) = self.symbols.functions.get(name).and_then(|f| f.body) else {
                continue;
            };
            let mut stack = vec![body];
            let mut visited = HashSet::new();
            while let Some(id) = stack.pop() {
                if !visited.insert(id) {
                    continue;
                }
                if matches!(
                    match_fir(self.store, id),
                    FirMatch::LoadVar { name: ref var, access: AccessType::FunArgs, .. }
                        if var == "count"
                ) {
                    self.warn(
                        "FIR-F09",
                        format!("'{name}' body references the block 'count' argument"),
                        id,
                    );
                    break;
                }
                stack.extend(crate::matcher::fir_match_children(self.store, id));
            }
        }
    }
}
