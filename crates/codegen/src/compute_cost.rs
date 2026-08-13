//! Canonical scalar-FIR compute-cost analysis used by `mem0` consumers.
//!
//! # Source provenance (C++)
//!
//! This is the Rust port of the intent of
//! `compiler/generator/instructions_complexity.hh`'s
//! `InstComplexityVisitor`. It deliberately fixes the reference visitor's
//! broken `IfInst` accounting and normalizes loop operations synthesized by
//! the Rust C-family emitters. The version-2 semantics are specified in
//! `porting/custom-memory-manager-mem0-analysis-and-porting-plan-2026-08-13-en.md`
//! decision D6.

use std::collections::BTreeMap;

use fir::{FirBinOp, FirId, FirMatch, FirStore, FirType, match_fir};

/// Version of the corrected, backend-neutral cost semantics.
pub const COMPUTE_COST_VERSION: u32 = 2;

/// Stable name of the metric serialized beside [`ComputeCost`].
pub const COMPUTE_COST_METRIC: &str = "static_scalar_fir_structure";

/// Checked structural cost of one occurrence of the effective scalar compute
/// body.
///
/// Loop bodies are visited once and are not multiplied by their trip count.
/// Mutually exclusive branches are merged component-wise by maximum. Operation
/// maps are ordered lexically so C, C++, Cranelift, and JSON observe identical
/// deterministic data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComputeCost {
    pub version: u32,
    pub load: u64,
    pub store: u64,
    pub declare: u64,
    pub number: u64,
    pub cast: u64,
    pub select: u64,
    pub loops: u64,
    pub binop_total: u64,
    pub binops: BTreeMap<String, u64>,
    pub mathop_total: u64,
    pub mathops: BTreeMap<String, u64>,
}

impl ComputeCost {
    fn empty_v2() -> Self {
        Self {
            version: COMPUTE_COST_VERSION,
            ..Self::default()
        }
    }

    fn checked_add_assign(&mut self, rhs: &Self) -> Result<(), ComputeCostError> {
        macro_rules! add_field {
            ($field:ident) => {
                self.$field = self
                    .$field
                    .checked_add(rhs.$field)
                    .ok_or(ComputeCostError::Overflow(stringify!($field)))?;
            };
        }
        add_field!(load);
        add_field!(store);
        add_field!(declare);
        add_field!(number);
        add_field!(cast);
        add_field!(select);
        add_field!(loops);
        for (key, value) in &rhs.binops {
            add_map_value(&mut self.binops, key, *value, "binop")?;
        }
        for (key, value) in &rhs.mathops {
            add_map_value(&mut self.mathops, key, *value, "mathop")?;
        }
        self.recompute_totals()
    }

    fn max_assign(&mut self, rhs: &Self) -> Result<(), ComputeCostError> {
        self.load = self.load.max(rhs.load);
        self.store = self.store.max(rhs.store);
        self.declare = self.declare.max(rhs.declare);
        self.number = self.number.max(rhs.number);
        self.cast = self.cast.max(rhs.cast);
        self.select = self.select.max(rhs.select);
        self.loops = self.loops.max(rhs.loops);
        for (key, value) in &rhs.binops {
            let entry = self.binops.entry(key.clone()).or_default();
            *entry = (*entry).max(*value);
        }
        for (key, value) in &rhs.mathops {
            let entry = self.mathops.entry(key.clone()).or_default();
            *entry = (*entry).max(*value);
        }
        self.recompute_totals()
    }

    fn recompute_totals(&mut self) -> Result<(), ComputeCostError> {
        self.binop_total = checked_sum(self.binops.values().copied(), "binop_total")?;
        self.mathop_total = checked_sum(self.mathops.values().copied(), "mathop_total")?;
        Ok(())
    }
}

/// Failure produced when executable FIR cannot be described without lying or
/// when a checked counter overflows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComputeCostError {
    MissingCompute,
    InvalidFunctionSection,
    UnsupportedFirNode { node: u32, kind: String },
    Overflow(&'static str),
}

impl std::fmt::Display for ComputeCostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCompute => f.write_str("FIR module has no body-bearing compute function"),
            Self::InvalidFunctionSection => {
                f.write_str("FIR module functions section is not a block")
            }
            Self::UnsupportedFirNode { node, kind } => {
                write!(f, "unsupported executable FIR node {node}: {kind}")
            }
            Self::Overflow(counter) => write!(f, "compute-cost counter overflow: {counter}"),
        }
    }
}

impl std::error::Error for ComputeCostError {}

/// Finds and analyzes the body-bearing `compute` declaration in a FIR function
/// section.
///
/// Every FIR occurrence is traversed even when hash-consing gives two
/// occurrences the same [`FirId`]. Helper declarations are excluded; a helper
/// call in the compute body is counted once under its exact callee name.
pub fn analyze_compute_cost(
    store: &FirStore,
    functions: FirId,
) -> Result<ComputeCost, ComputeCostError> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Err(ComputeCostError::InvalidFunctionSection);
    };
    let body = items
        .into_iter()
        .find_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } if name == "compute" => Some(body),
            _ => None,
        });
    let body = body.ok_or(ComputeCostError::MissingCompute)?;
    analyze_compute_body(store, effective_scalar_compute_root(store, body))
}

/// Selects the generated scalar loop while excluding block-rate prelude code.
///
/// Faust C++ applies `InstComplexityVisitor` to
/// `fCurLoop->generateScalarLoop("count")`, not to the surrounding `compute`
/// function. Production scalar FIR has exactly one direct loop statement after
/// optional slow/control declarations. Synthetic FIR without that shape keeps
/// its whole body so focused builder tests remain meaningful.
pub(crate) fn effective_scalar_compute_root(store: &FirStore, body: FirId) -> FirId {
    let FirMatch::Block(items) = match_fir(store, body) else {
        return body;
    };
    let mut loops = items.into_iter().filter(|item| {
        matches!(
            match_fir(store, *item),
            FirMatch::ForLoop { .. }
                | FirMatch::SimpleForLoop { .. }
                | FirMatch::WhileLoop { .. }
                | FirMatch::IteratorForLoop { .. }
        )
    });
    let Some(loop_root) = loops.next() else {
        return body;
    };
    if loops.next().is_some() {
        body
    } else {
        loop_root
    }
}

/// Analyzes an already selected effective scalar compute body.
pub fn analyze_compute_body(
    store: &FirStore,
    body: FirId,
) -> Result<ComputeCost, ComputeCostError> {
    CostVisitor { store }.visit(body)
}

struct CostVisitor<'a> {
    store: &'a FirStore,
}

impl CostVisitor<'_> {
    fn visit(&self, id: FirId) -> Result<ComputeCost, ComputeCostError> {
        use FirMatch as M;

        let mut cost = ComputeCost::empty_v2();
        match match_fir(self.store, id) {
            M::Int32 { .. }
            | M::Int64 { .. }
            | M::Float32 { .. }
            | M::Float64 { .. }
            | M::Bool { .. }
            | M::Quad { .. }
            | M::FixedPoint { .. } => increment(&mut cost.number, "number")?,
            M::ValueArray { values, .. } => self.add_children(&mut cost, values)?,
            M::Int32Array { values, .. } => add_usize(&mut cost.number, values.len(), "number")?,
            M::Float32Array { values, .. } => add_usize(&mut cost.number, values.len(), "number")?,
            M::Float64Array { values, .. }
            | M::QuadArray { values, .. }
            | M::FixedPointArray { values, .. } => {
                add_usize(&mut cost.number, values.len(), "number")?
            }
            M::LoadVar { .. } => increment(&mut cost.load, "load")?,
            M::LoadTable { index, .. } => {
                increment(&mut cost.load, "load")?;
                cost.checked_add_assign(&self.visit(index)?)?;
            }
            M::LoadVarAddress { .. } | M::NullValue { .. } => {}
            M::TeeVar { value, .. } => {
                increment(&mut cost.store, "store")?;
                cost.checked_add_assign(&self.visit(value)?)?;
            }
            M::BinOp { op, lhs, rhs, .. } => {
                cost.checked_add_assign(&self.visit(lhs)?)?;
                cost.checked_add_assign(&self.visit(rhs)?)?;
                self.add_binop(&mut cost, op, lhs, rhs)?;
            }
            M::Neg { value, .. } => cost.checked_add_assign(&self.visit(value)?)?,
            M::Cast { value, .. } | M::Bitcast { value, .. } => {
                increment(&mut cost.cast, "cast")?;
                cost.checked_add_assign(&self.visit(value)?)?;
            }
            M::Select2 {
                cond,
                then_value,
                else_value,
                ..
            } => {
                increment(&mut cost.select, "select")?;
                cost.checked_add_assign(&self.visit(cond)?)?;
                cost.checked_add_assign(&self.branch_max([then_value, else_value])?)?;
            }
            M::FunCall { name, args, .. } => {
                add_map_value(&mut cost.mathops, &name, 1, "mathop")?;
                self.add_children(&mut cost, args)?;
                cost.recompute_totals()?;
            }
            M::DeclareVar { init, .. } => {
                increment(&mut cost.declare, "declare")?;
                if let Some(init) = init {
                    cost.checked_add_assign(&self.visit(init)?)?;
                }
            }
            M::DeclareTable { values, .. } => {
                increment(&mut cost.declare, "declare")?;
                self.add_children(&mut cost, values)?;
            }
            M::StoreVar { value, .. } => {
                increment(&mut cost.store, "store")?;
                cost.checked_add_assign(&self.visit(value)?)?;
            }
            M::StoreTable { index, value, .. } => {
                increment(&mut cost.store, "store")?;
                cost.checked_add_assign(&self.visit(index)?)?;
                cost.checked_add_assign(&self.visit(value)?)?;
            }
            M::Drop(value) => cost.checked_add_assign(&self.visit(value)?)?,
            M::NullStatement | M::Return(None) | M::Label(_) => {}
            M::Return(Some(value)) => cost.checked_add_assign(&self.visit(value)?)?,
            M::Block(items) => self.add_children(&mut cost, items)?,
            M::If {
                cond,
                then_block,
                else_block,
            } => {
                increment(&mut cost.select, "select")?;
                cost.checked_add_assign(&self.visit(cond)?)?;
                let mut branches = vec![then_block];
                if let Some(else_block) = else_block {
                    branches.push(else_block);
                }
                cost.checked_add_assign(&self.branch_max(branches)?)?;
            }
            M::Control { cond, stmt } => {
                increment(&mut cost.select, "select")?;
                cost.checked_add_assign(&self.visit(cond)?)?;
                cost.checked_add_assign(&self.visit(stmt)?)?;
            }
            M::ForLoop {
                init,
                end,
                step,
                body,
                is_reverse,
                ..
            } => {
                increment(&mut cost.loops, "loops")?;
                // C/C++ emit: declaration/init, one conceptual comparison,
                // and one load/add/store update. The end and step operands
                // occur in those synthesized expressions.
                cost.checked_add_assign(&self.visit(init)?)?;
                cost.checked_add_assign(&self.visit(end)?)?;
                self.add_synthesized_int_binop(
                    &mut cost,
                    if is_reverse {
                        FirBinOp::Gt
                    } else {
                        FirBinOp::Lt
                    },
                )?;
                increment(&mut cost.load, "load")?;
                cost.checked_add_assign(&self.visit(step)?)?;
                self.add_synthesized_int_binop(&mut cost, FirBinOp::Add)?;
                increment(&mut cost.load, "load")?;
                increment(&mut cost.store, "store")?;
                cost.checked_add_assign(&self.visit(body)?)?;
            }
            M::SimpleForLoop {
                upper,
                body,
                is_reverse,
                ..
            } => {
                increment(&mut cost.loops, "loops")?;
                increment(&mut cost.declare, "declare")?;
                if is_reverse {
                    // int i = upper - 1; i >= 0; i = i - 1
                    cost.checked_add_assign(&self.visit(upper)?)?;
                    add_usize(&mut cost.number, 3, "number")?;
                    self.add_synthesized_int_binop(&mut cost, FirBinOp::Sub)?;
                    self.add_synthesized_int_binop(&mut cost, FirBinOp::Ge)?;
                    self.add_synthesized_int_binop(&mut cost, FirBinOp::Sub)?;
                } else {
                    // int i = 0; i < upper; ++i / i = i + 1
                    increment(&mut cost.number, "number")?;
                    cost.checked_add_assign(&self.visit(upper)?)?;
                    self.add_synthesized_int_binop(&mut cost, FirBinOp::Lt)?;
                    increment(&mut cost.number, "number")?;
                    self.add_synthesized_int_binop(&mut cost, FirBinOp::Add)?;
                }
                // One loop-variable load in the comparison and one in the
                // update, followed by one store.
                add_usize(&mut cost.load, 2, "load")?;
                increment(&mut cost.store, "store")?;
                cost.checked_add_assign(&self.visit(body)?)?;
            }
            M::WhileLoop { cond, body } => {
                increment(&mut cost.loops, "loops")?;
                cost.checked_add_assign(&self.visit(cond)?)?;
                cost.checked_add_assign(&self.visit(body)?)?;
            }
            M::ShiftArrayVar { delay, .. } => {
                // Scalar FIR spells a delay-line shift as one dedicated node,
                // but emitted native code performs a bounded loop containing
                // one table load and store. Keep this syntactic (not multiplied
                // by `delay`) like all other per-frame cost entries.
                if delay > 0 {
                    increment(&mut cost.loops, "loops")?;
                    increment(&mut cost.load, "load")?;
                    increment(&mut cost.store, "store")?;
                }
            }
            M::Switch {
                cond,
                cases,
                default,
            } => {
                increment(&mut cost.select, "select")?;
                cost.checked_add_assign(&self.visit(cond)?)?;
                let mut branches: Vec<_> = cases.into_iter().map(|(_, body)| body).collect();
                if let Some(default) = default {
                    branches.push(default);
                }
                cost.checked_add_assign(&self.branch_max(branches)?)?;
            }
            M::LoadSoundfileLength { part, .. } | M::LoadSoundfileRate { part, .. } => {
                increment(&mut cost.load, "load")?;
                cost.checked_add_assign(&self.visit(part)?)?;
            }
            M::LoadSoundfileBuffer {
                chan, part, idx, ..
            } => {
                increment(&mut cost.load, "load")?;
                self.add_children(&mut cost, [chan, part, idx])?;
            }
            unsupported @ (M::Unknown
            | M::NewDsp { .. }
            | M::DeclareFun { .. }
            | M::DeclareStructType { .. }
            | M::DeclareBufferIterators { .. }
            | M::IteratorForLoop { .. }
            | M::OpenBox { .. }
            | M::CloseBox
            | M::AddButton { .. }
            | M::AddSlider { .. }
            | M::AddBargraph { .. }
            | M::AddSoundfile { .. }
            | M::AddMetaDeclare { .. }
            | M::Module { .. }
            | M::SubModule { .. }) => {
                return Err(ComputeCostError::UnsupportedFirNode {
                    node: id.as_u32(),
                    kind: format!("{unsupported:?}"),
                });
            }
        }
        cost.recompute_totals()?;
        Ok(cost)
    }

    fn add_children(
        &self,
        cost: &mut ComputeCost,
        children: impl IntoIterator<Item = FirId>,
    ) -> Result<(), ComputeCostError> {
        for child in children {
            cost.checked_add_assign(&self.visit(child)?)?;
        }
        Ok(())
    }

    fn branch_max(
        &self,
        branches: impl IntoIterator<Item = FirId>,
    ) -> Result<ComputeCost, ComputeCostError> {
        let mut merged = ComputeCost::empty_v2();
        for branch in branches {
            merged.max_assign(&self.visit(branch)?)?;
        }
        Ok(merged)
    }

    fn add_binop(
        &self,
        cost: &mut ComputeCost,
        op: FirBinOp,
        lhs: FirId,
        rhs: FirId,
    ) -> Result<(), ComputeCostError> {
        let real = self
            .store
            .value_type(lhs)
            .is_some_and(|typ| is_real_type(&typ))
            || self
                .store
                .value_type(rhs)
                .is_some_and(|typ| is_real_type(&typ));
        let key = format!(
            "{}({})",
            if real { "Real" } else { "Int" },
            binop_symbol(op)
        );
        add_map_value(&mut cost.binops, &key, 1, "binop")?;
        cost.recompute_totals()
    }

    fn add_synthesized_int_binop(
        &self,
        cost: &mut ComputeCost,
        op: FirBinOp,
    ) -> Result<(), ComputeCostError> {
        let key = format!("Int({})", binop_symbol(op));
        add_map_value(&mut cost.binops, &key, 1, "binop")?;
        cost.recompute_totals()
    }
}

fn is_real_type(typ: &FirType) -> bool {
    matches!(
        typ,
        FirType::Float32
            | FirType::Float64
            | FirType::FaustFloat
            | FirType::Quad
            | FirType::FixedPoint
    )
}

fn binop_symbol(op: FirBinOp) -> &'static str {
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
        FirBinOp::LRsh => ">>>",
        FirBinOp::Eq => "==",
        FirBinOp::Ne => "!=",
        FirBinOp::Lt => "<",
        FirBinOp::Le => "<=",
        FirBinOp::Gt => ">",
        FirBinOp::Ge => ">=",
    }
}

fn increment(value: &mut u64, name: &'static str) -> Result<(), ComputeCostError> {
    *value = value
        .checked_add(1)
        .ok_or(ComputeCostError::Overflow(name))?;
    Ok(())
}

fn add_usize(value: &mut u64, amount: usize, name: &'static str) -> Result<(), ComputeCostError> {
    let amount = u64::try_from(amount).map_err(|_| ComputeCostError::Overflow(name))?;
    *value = value
        .checked_add(amount)
        .ok_or(ComputeCostError::Overflow(name))?;
    Ok(())
}

fn add_map_value(
    map: &mut BTreeMap<String, u64>,
    key: &str,
    amount: u64,
    name: &'static str,
) -> Result<(), ComputeCostError> {
    let value = map.entry(key.to_owned()).or_default();
    *value = value
        .checked_add(amount)
        .ok_or(ComputeCostError::Overflow(name))?;
    Ok(())
}

fn checked_sum(
    values: impl IntoIterator<Item = u64>,
    name: &'static str,
) -> Result<u64, ComputeCostError> {
    values.into_iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(value)
            .ok_or(ComputeCostError::Overflow(name))
    })
}

#[cfg(test)]
mod tests {
    use fir::{AccessType, FirBuilder};

    use super::*;

    #[test]
    fn branch_merge_is_componentwise_and_maps_remain_summed() {
        let mut store = FirStore::new();
        let body = {
            let mut b = FirBuilder::new(&mut store);
            let cond = b.bool_(true);
            let one = b.int32(1);
            let two = b.int32(2);
            let add = b.binop(FirBinOp::Add, one, two, FirType::Int32);
            let then_store = b.store_var("x", AccessType::Struct, add);
            let then_block = b.block(&[then_store]);
            let arg = b.float32(0.5);
            let call = b.fun_call("sin", &[arg], FirType::Float32);
            let else_store1 = b.store_var("x", AccessType::Struct, call);
            let else_store2 = b.store_var("y", AccessType::Struct, one);
            let else_block = b.block(&[else_store1, else_store2]);
            let if_stmt = b.if_(cond, then_block, Some(else_block));
            b.block(&[if_stmt])
        };

        let cost = analyze_compute_body(&store, body).unwrap();
        assert_eq!(cost.select, 1);
        assert_eq!(cost.store, 2, "else has the maximum store count");
        assert_eq!(
            cost.number, 3,
            "condition plus the then branch's maximum literal count"
        );
        assert_eq!(cost.binops["Int(+)"], 1);
        assert_eq!(cost.mathops["sin"], 1);
        assert_eq!(cost.binop_total, cost.binops.values().sum::<u64>());
        assert_eq!(cost.mathop_total, cost.mathops.values().sum::<u64>());
    }

    #[test]
    fn simple_loop_counts_synthesized_control_operations_once() {
        let mut store = FirStore::new();
        let body = {
            let mut b = FirBuilder::new(&mut store);
            let upper = b.load_var("count", AccessType::FunArgs, FirType::Int32);
            let loop_body = b.block(&[]);
            let loop_stmt = b.simple_for_loop("i", upper, loop_body, false);
            b.block(&[loop_stmt])
        };

        let cost = analyze_compute_body(&store, body).unwrap();
        assert_eq!(cost.loops, 1);
        assert_eq!(cost.declare, 1);
        assert_eq!(cost.number, 2, "synthesized zero and one");
        assert_eq!(cost.load, 3, "upper plus comparison/update loop loads");
        assert_eq!(cost.store, 1);
        assert_eq!(cost.binops["Int(<)"], 1);
        assert_eq!(cost.binops["Int(+)"], 1);
    }

    #[test]
    fn repeated_hash_consed_occurrences_are_not_deduplicated() {
        let mut store = FirStore::new();
        let body = {
            let mut b = FirBuilder::new(&mut store);
            let one = b.int32(1);
            let first = b.store_var("x", AccessType::Struct, one);
            let second = b.store_var("y", AccessType::Struct, one);
            b.block(&[first, second])
        };
        let cost = analyze_compute_body(&store, body).unwrap();
        assert_eq!(cost.number, 2);
        assert_eq!(cost.store, 2);
    }

    #[test]
    fn module_analysis_excludes_compute_prelude_outside_the_scalar_loop() {
        let mut store = FirStore::new();
        let functions = {
            let mut b = FirBuilder::new(&mut store);
            let slow_load = b.load_var("fControl", AccessType::Struct, FirType::Float32);
            let slow = b.declare_var(
                "fSlow",
                FirType::Float32,
                AccessType::Stack,
                Some(slow_load),
            );
            let sample_load = b.load_var("fState", AccessType::Struct, FirType::Float32);
            let loop_body = b.block(&[sample_load]);
            let upper = b.load_var("count", AccessType::FunArgs, FirType::Int32);
            let loop_stmt = b.simple_for_loop("i", upper, loop_body, false);
            let body = b.block(&[slow, loop_stmt]);
            let compute = b.declare_fun(
                "compute",
                FirType::Fun {
                    args: Vec::new(),
                    ret: Box::new(FirType::Void),
                },
                &[],
                Some(body),
                false,
            );
            b.block(&[compute])
        };

        let cost = analyze_compute_cost(&store, functions).unwrap();
        assert_eq!(cost.loops, 1);
        assert_eq!(cost.declare, 1, "only the synthesized loop variable counts");
        assert_eq!(cost.load, 4, "control prelude load is excluded");
    }

    #[test]
    fn if_cost_is_invariant_when_the_expensive_branch_is_swapped() {
        fn cost(expensive_is_then: bool) -> ComputeCost {
            let mut store = FirStore::new();
            let body = {
                let mut b = FirBuilder::new(&mut store);
                let cond = b.bool_(true);
                let one = b.int32(1);
                let cheap_store = b.store_var("cheap", AccessType::Struct, one);
                let cheap = b.block(&[cheap_store]);
                let arg = b.float32(0.5);
                let sin = b.fun_call("sin", &[arg], FirType::Float32);
                let expensive_store1 = b.store_var("x", AccessType::Struct, sin);
                let expensive_store2 = b.store_var("y", AccessType::Struct, arg);
                let expensive = b.block(&[expensive_store1, expensive_store2]);
                let branch = if expensive_is_then {
                    b.if_(cond, expensive, Some(cheap))
                } else {
                    b.if_(cond, cheap, Some(expensive))
                };
                b.block(&[branch])
            };
            analyze_compute_body(&store, body).unwrap()
        }

        assert_eq!(cost(true), cost(false));
    }

    #[test]
    fn d6_counts_select_control_switch_bitcast_and_extended_literals() {
        let mut store = FirStore::new();
        let body = {
            let mut b = FirBuilder::new(&mut store);
            let cond = b.bool_(true);
            let int64 = b.int64(7);
            let quad = b.quad(0.25);
            let fixed = b.fixed_point(0.5);
            let bitcast = b.bitcast(FirType::Int64, quad);
            let selected = b.select2(cond, int64, bitcast, FirType::Int64);
            let selected_drop = b.drop_(selected);

            let controlled_store = b.store_var("x", AccessType::Struct, fixed);
            let controlled = b.control(cond, controlled_store);

            let cheap_store = b.store_var("y", AccessType::Struct, int64);
            let cheap = b.block(&[cheap_store]);
            let expensive_store1 = b.store_var("z", AccessType::Struct, quad);
            let expensive_store2 = b.store_var("w", AccessType::Struct, fixed);
            let expensive = b.block(&[expensive_store1, expensive_store2]);
            let switched = b.switch(int64, &[(0, cheap), (1, expensive)], None);
            b.block(&[selected_drop, controlled, switched])
        };

        let cost = analyze_compute_body(&store, body).unwrap();
        assert_eq!(cost.select, 3);
        assert_eq!(cost.cast, 1);
        assert_eq!(cost.store, 3, "control plus maximum switch branch");
        assert!(cost.number >= 6, "all extended literal occurrences count");
    }
}
