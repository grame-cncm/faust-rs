//! `table_fill` rules of the FIR checker.
//!
//! Generated-table `fill` rules: coverage, call extent, signature, and write confinement.
//!
//! Split out of `checker.rs` on 2026-08-18, where all 67 methods sat in one
//! 2674-line `impl`. Bodies are moved verbatim; the only edit is visibility,
//! private to `pub(super)`, so sibling rule modules can still call across.

use super::*;

impl<'s> VerifyCtx<'s> {
    /// FIR-SM01: every declared sub-module must have its `fill` called from a
    /// lifecycle body.
    ///
    /// A generated table is declared without an initializer and populated at
    /// initialization time by its sub-module. If the call is missing — a
    /// backend that dropped it, a lowering that declared the table but never
    /// registered the call — the program still compiles and runs, and silently
    /// reads zeros. That is exactly what upstream 2.87.1 does for nested
    /// generated tables (`porting/generated/siggen-table-init-s0/`, `f08`),
    /// and this rule is what makes it impossible here.
    ///
    /// The check keys on the **sub-module**, not on the shape of the array:
    /// an earlier draft flagged any uninitialized array read by `compute` and
    /// immediately fired on ordinary delay lines (`iVec6`), which are also
    /// uninitialized arrays but are zeroed by `instanceClear` rather than
    /// filled. Sub-modules exist only for generated tables, so requiring one
    /// fill call per sub-module is both precise and producer-independent.
    pub(super) fn check_table_fill_coverage(&mut self) {
        let FirMatch::Module {
            functions,
            sub_modules,
            ..
        } = match_fir(self.store, self.module_id)
        else {
            return;
        };
        let FirMatch::Block(subs) = match_fir(self.store, sub_modules) else {
            return;
        };
        if subs.is_empty() {
            return;
        }
        let called = self.collect_called_fills(functions);
        for sub in subs {
            let FirMatch::SubModule { name, .. } = match_fir(self.store, sub) else {
                continue;
            };
            let expected = format!("fill{name}");
            if !called.contains(&expected) {
                self.error(
                    "FIR-SM01",
                    format!(
                        "sub-module '{name}' fills a generated table but '{expected}' is never \
                         called from staticInit or instanceConstants; the table would be read \
                         uninitialized"
                    ),
                    sub,
                );
            }
        }
    }
    /// FIR-SM06: each `fill` call must cover its table's whole declared length.
    ///
    /// Invariant I2 of the port plan. FIR-SM01 proves a fill *happens*; it says
    /// nothing about how much it writes. The sub-module's loop runs `0..count`,
    /// so the elements actually initialized are decided entirely by the `count`
    /// the call site passes — pass `size - 1` and the last cell keeps whatever
    /// the target's uninitialized storage held, which no numeric test on a
    /// 65536-entry table is likely to notice.
    ///
    /// The table's length comes from its own declaration rather than from the
    /// call, so producer and check do not share a source: the call would have
    /// to be wrong in the same direction as the declaration to slip through.
    pub(super) fn check_fill_call_extent(&mut self) {
        let FirMatch::Module {
            functions,
            dsp_struct,
            globals,
            static_decls,
            ..
        } = match_fir(self.store, self.module_id)
        else {
            return;
        };
        let mut lengths: HashMap<String, usize> = HashMap::new();
        for block in [dsp_struct, globals, static_decls] {
            let FirMatch::Block(items) = match_fir(self.store, block) else {
                continue;
            };
            for item in items {
                match match_fir(self.store, item) {
                    FirMatch::DeclareVar {
                        name,
                        typ: FirType::Array(_, size),
                        ..
                    } => {
                        lengths.insert(name, size);
                    }
                    FirMatch::DeclareTable { name, values, .. } => {
                        lengths.insert(name, values.len());
                    }
                    _ => {}
                }
            }
        }
        if lengths.is_empty() {
            return;
        }
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
            if name != "staticInit" && name != "instanceConstants" {
                continue;
            }
            let mut stack = vec![body];
            while let Some(id) = stack.pop() {
                if let FirMatch::FunCall {
                    name: callee, args, ..
                } = match_fir(self.store, id)
                    && callee.starts_with("fill")
                    && args.len() == 3
                    && let FirMatch::Int32 { value: count, .. } = match_fir(self.store, args[1])
                    && let FirMatch::LoadVar { name: table, .. } = match_fir(self.store, args[2])
                    && let Some(&length) = lengths.get(&table)
                    && usize::try_from(count).map(|c| c != length).unwrap_or(true)
                {
                    self.error(
                        "FIR-SM06",
                        format!(
                            "'{callee}' fills {count} of the {length} cells of table \
                             '{table}'; the remaining cells would be read uninitialized"
                        ),
                        id,
                    );
                }
                stack.extend(child_ids(&match_fir(self.store, id)));
            }
        }
    }
    /// Collects the names of `fill…` functions called from a lifecycle body.
    pub(super) fn collect_called_fills(&self, functions: FirId) -> HashSet<String> {
        let mut out = HashSet::new();
        let FirMatch::Block(items) = match_fir(self.store, functions) else {
            return out;
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
            if name != "staticInit" && name != "instanceConstants" {
                continue;
            }
            let mut stack = vec![body];
            while let Some(id) = stack.pop() {
                if let FirMatch::FunCall { name: callee, .. } = match_fir(self.store, id)
                    && callee.starts_with("fill")
                {
                    out.insert(callee);
                }
                stack.extend(child_ids(&match_fir(self.store, id)));
            }
        }
        out
    }
    /// FIR-SM05: a sub-module that owns nested generators must call each of
    /// their fills from its own `instanceInit`.
    ///
    /// A sub-module has no `classInit`, so a nested generated table can only be
    /// populated from `instanceInit{name}`, which the parent invokes before
    /// `fill{name}`. Without this, the inner table is declared and never
    /// written and the outer table is computed from zeros — the upstream
    /// 2.87.1 behavior this port deliberately does not reproduce
    /// (`porting/generated/siggen-table-init-s0/`, fixture `f08`). The first
    /// implementation of the producer had exactly this bug.
    pub(super) fn check_nested_fill_coverage(
        &mut self,
        name: &str,
        functions: FirId,
        sub_modules: FirId,
    ) {
        let FirMatch::Block(nested) = match_fir(self.store, sub_modules) else {
            return;
        };
        if nested.is_empty() {
            return;
        }
        let mut called: HashSet<String> = HashSet::new();
        if let FirMatch::Block(items) = match_fir(self.store, functions) {
            for item in items {
                let FirMatch::DeclareFun {
                    name: fun_name,
                    body: Some(body),
                    ..
                } = match_fir(self.store, item)
                else {
                    continue;
                };
                if fun_name != format!("instanceInit{name}") {
                    continue;
                }
                let mut stack = vec![body];
                while let Some(id) = stack.pop() {
                    if let FirMatch::FunCall { name: callee, .. } = match_fir(self.store, id)
                        && callee.starts_with("fill")
                    {
                        called.insert(callee);
                    }
                    stack.extend(child_ids(&match_fir(self.store, id)));
                }
            }
        }
        for inner in nested {
            let FirMatch::SubModule {
                name: inner_name, ..
            } = match_fir(self.store, inner)
            else {
                continue;
            };
            let expected = format!("fill{inner_name}");
            if !called.contains(&expected) {
                self.error(
                    "FIR-SM05",
                    format!(
                        "sub-module '{name}' owns nested generator '{inner_name}' but never \
                         calls '{expected}' from 'instanceInit{name}'; the nested table would \
                         be read uninitialized"
                    ),
                    inner,
                );
            }
        }
    }
    /// FIR-SM03: `fill{name}` takes `(dsp, count: Int32, table: Ptr(elem_type))`.
    pub(super) fn check_fill_signature(
        &mut self,
        node: FirId,
        name: &str,
        elem_type: &FirType,
        args: &[NamedType],
    ) {
        let Some(count) = args.iter().find(|a| a.name == "count") else {
            self.error(
                "FIR-SM03",
                format!("sub-module '{name}' fill function has no 'count' argument"),
                node,
            );
            return;
        };
        if count.typ != FirType::Int32 {
            self.error(
                "FIR-SM03",
                format!(
                    "sub-module '{name}' fill argument 'count' has type {:?}, expected Int32",
                    count.typ
                ),
                node,
            );
        }
        let Some(table) = args.iter().find(|a| a.name == "table") else {
            self.error(
                "FIR-SM03",
                format!("sub-module '{name}' fill function has no 'table' argument"),
                node,
            );
            return;
        };
        let expected = FirType::Ptr(Box::new(elem_type.clone()));
        if table.typ != expected {
            self.error(
                "FIR-SM03",
                format!(
                    "sub-module '{name}' fill argument 'table' has type {:?}, \
                     expected {expected:?} from the sub-module element type",
                    table.typ
                ),
                node,
            );
        }
    }
    /// FIR-SM03: the fill body may write its own state and the `table`
    /// argument, never a table belonging to the enclosing module.
    ///
    /// A sub-module runs before the DSP exists as far as the caller is
    /// concerned — in the C++ shape it is a separate object built on the stack
    /// of `classInit` — so a store into any other named table is either a name
    /// collision or a lowering bug that would corrupt parent state.
    pub(super) fn check_fill_writes_only_table(&mut self, body: FirId, name: &str) {
        let mut stack = vec![body];
        while let Some(id) = stack.pop() {
            if let FirMatch::StoreTable {
                name: target,
                access,
                ..
            } = match_fir(self.store, id)
                && access == AccessType::FunArgs
                && target != "table"
            {
                self.error(
                    "FIR-SM03",
                    format!(
                        "sub-module '{name}' fill body writes through argument table \
                         '{target}', expected only 'table'"
                    ),
                    id,
                );
            }
            stack.extend(child_ids(&match_fir(self.store, id)));
        }
    }
}
