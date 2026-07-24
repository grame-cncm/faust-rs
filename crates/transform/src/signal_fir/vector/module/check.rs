//! Final-module verification: sound-field immutability, mutable-table
//! and UI-write attribution, read-only table stores, and final shape.
//! Called from the producer's terminal step in `build.rs` (plan §4.8).

use super::model::VectorModuleFailure;
use crate::signal_fir::VectorFallbackReason;
use crate::signal_fir::vector::analysis::EffectAtom;
use crate::signal_fir::vector::assemble::{VectorFirAssembly, fir_reachable};
use crate::signal_fir::vector::ui::VectorUiFir;
use crate::signal_fir::vector::verify::VectorPlan;
use fir::checker::verify_fir_module;
use fir::{FirId, FirMatch, FirStore, FirType, match_fir};
use std::collections::{BTreeMap, BTreeSet};
/// Rejects any compute-time store into a `Sound`-typed DSP-struct field.
///
/// Soundfile data is loaded at lifecycle time and immutable during `compute`;
/// admitting soundfile reads widened what compute may contain, so the
/// read-only claim must be carried by the emitted FIR itself. The field set is
/// derived from the emitted struct declarations alone, exactly as the table
/// checks derive theirs.
pub(super) fn verify_sound_field_immutability(
    store: &FirStore,
    compute: FirId,
    struct_fields: &[FirId],
) -> Result<(), VectorModuleFailure> {
    let sound_fields = struct_fields
        .iter()
        .filter_map(|field| match match_fir(store, *field) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Sound,
                ..
            } => Some(name),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if sound_fields.is_empty() {
        return Ok(());
    }
    for node in fir_reachable(store, compute) {
        let FirMatch::StoreVar { name, .. } = match_fir(store, node) else {
            continue;
        };
        if sound_fields.contains(&name) {
            return Err(module_shape(format!(
                "compute stores into soundfile field {name}"
            )));
        }
    }
    Ok(())
}
/// Verifies mutable-table writes and initialization against the emitted FIR.
///
/// The mutable-table set is read from the emitted DSP-struct declarations, not
/// from the effect model or the lowerer registry, so a misprojection on either
/// side surfaces as a count mismatch here. Three obligations: every claimed
/// direct `WriteTable` performer is backed by exactly one physical `StoreTable`
/// on its table's field in `compute` and vice versa (the table analogue of the
/// UI-write attribution check), and `instanceConstants` initializes every cell
/// of every mutable table exactly once. Initial values themselves are numeric
/// claims the certificate does not carry; the native oracle matrix is their
/// arbiter.
pub(super) fn verify_mutable_table_attribution(
    store: &FirStore,
    compute: FirId,
    instance_constants: FirId,
    table_declarations: &[FirId],
    plan: &VectorPlan,
) -> Result<(), VectorModuleFailure> {
    let mut declared = BTreeMap::<String, usize>::new();
    for declaration in table_declarations {
        let FirMatch::DeclareVar {
            name,
            typ: FirType::Array(_, length),
            ..
        } = match_fir(store, *declaration)
        else {
            return Err(module_shape(
                "mutable table declaration is not a struct array field",
            ));
        };
        declared.insert(name, length);
    }
    let mut claimed = BTreeMap::<String, usize>::new();
    for signal in &plan.signals {
        for effect in &signal.direct_effects {
            let EffectAtom::WriteTable(table) = effect else {
                continue;
            };
            let table_id = u64::from(*table);
            let name = [FirType::Int32, FirType::Float32]
                .iter()
                .map(|elem| crate::signal_fir::vector::lower::mutable_table_name(table_id, elem))
                .chain(std::iter::once(
                    crate::signal_fir::vector::lower::mutable_table_name(
                        table_id,
                        &FirType::Float64,
                    ),
                ))
                .find(|name| declared.contains_key(name));
            let Some(name) = name else {
                return Err(module_shape(format!(
                    "claimed mutable table writer {table_id} has no declared struct field"
                )));
            };
            *claimed.entry(name).or_default() += 1;
        }
    }
    let mut physical = BTreeMap::<String, usize>::new();
    for node in fir_reachable(store, compute) {
        let FirMatch::StoreTable { name, .. } = match_fir(store, node) else {
            continue;
        };
        if declared.contains_key(&name) {
            *physical.entry(name).or_default() += 1;
        }
    }
    for (name, count) in &claimed {
        let emitted = physical.get(name).copied().unwrap_or(0);
        if emitted != *count {
            return Err(module_shape(format!(
                "mutable table {name} has {count} claimed writers but compute stores it {emitted} times"
            )));
        }
    }
    for (name, emitted) in &physical {
        if claimed.get(name).copied().unwrap_or(0) != *emitted {
            return Err(module_shape(format!(
                "compute stores mutable table {name} {emitted} times with no matching claimed writer"
            )));
        }
    }
    let FirMatch::Block(init_statements) = match_fir(store, instance_constants) else {
        return Err(module_shape("instanceConstants is not a block"));
    };
    let mut covered = BTreeMap::<String, BTreeSet<i32>>::new();
    for statement in &init_statements {
        let FirMatch::StoreTable { name, index, .. } = match_fir(store, *statement) else {
            continue;
        };
        if !declared.contains_key(&name) {
            return Err(module_shape(format!(
                "instanceConstants stores unknown table {name}"
            )));
        }
        let FirMatch::Int32 { value, .. } = match_fir(store, index) else {
            return Err(module_shape(format!(
                "mutable table {name} initialization index is not a constant"
            )));
        };
        if !covered.entry(name.clone()).or_default().insert(value) {
            return Err(module_shape(format!(
                "mutable table {name} cell {value} initialized twice"
            )));
        }
    }
    for (name, length) in &declared {
        let cells = covered.get(name).map_or(0, BTreeSet::len);
        let complete = cells == *length
            && covered.get(name).is_some_and(|cells| {
                cells.first().is_some_and(|&first| first == 0)
                    && cells
                        .last()
                        .is_some_and(|&last| last + 1 == i32::try_from(*length).unwrap_or(i32::MAX))
            });
        if !complete {
            return Err(module_shape(format!(
                "mutable table {name} initialization covers {cells} of {length} cells"
            )));
        }
    }
    Ok(())
}
/// Rejects a UI write event attributed to a signal that performs no such write.
///
/// The event model now attributes an effect operation to the signal performing
/// it, read from the plan's `direct_effects`. Producer and checker both derive
/// that projection from the same analysis, so their agreement cannot catch a
/// misprojection. This counts the physical zone stores in the emitted `compute`
/// body instead and requires one per claimed performer: attributing a zone
/// write to every transitive carrier of it is exactly what made `mixer` report
/// 38 operations on a zone the compiler stores once.
///
/// FIR nodes are interned, so a body duplicated across chunk drivers yields one
/// node per physical store rather than one per driver.
pub(super) fn verify_ui_write_attribution(
    store: &FirStore,
    compute: FirId,
    ui: &ui::UiProgram,
    plan: &VectorPlan,
) -> Result<(), VectorModuleFailure> {
    let mut claimed = BTreeMap::<u32, usize>::new();
    for signal in &plan.signals {
        for effect in &signal.direct_effects {
            if let EffectAtom::WriteUi(control) = effect {
                *claimed.entry(*control).or_default() += 1;
            }
        }
    }
    let mut physical = BTreeMap::<String, usize>::new();
    for node in fir_reachable(store, compute) {
        if let FirMatch::StoreVar { name, .. } = match_fir(store, node) {
            *physical.entry(name).or_default() += 1;
        }
    }
    let mut zones = BTreeMap::<String, u32>::new();
    for spec in &ui.controls {
        zones.insert(
            crate::signal_fir::vector::ui::zone_name(spec.kind, spec.id),
            spec.id,
        );
    }
    for (control, count) in &claimed {
        let Some(spec) = ui.controls.iter().find(|spec| spec.id == *control) else {
            return Err(module_shape(format!(
                "event certificate writes unknown UI control {control}"
            )));
        };
        let zone = crate::signal_fir::vector::ui::zone_name(spec.kind, spec.id);
        let emitted = physical.get(&zone).copied().unwrap_or(0);
        if emitted != *count {
            return Err(module_shape(format!(
                "UI control {control} has {count} claimed writers but compute stores its zone {emitted} times"
            )));
        }
    }
    for (zone, emitted) in &physical {
        let Some(control) = zones.get(zone) else {
            continue;
        };
        if claimed.get(control).copied().unwrap_or(0) != *emitted {
            return Err(module_shape(format!(
                "compute stores UI zone {zone} {emitted} times with no matching claimed writer"
            )));
        }
    }
    Ok(())
}
/// Rejects any store into a table the lowering declared read-only.
///
/// The signal-level effect model and the pure lowerer share one read-only
/// predicate, so their agreement cannot catch a misclassification of one. This
/// reads the emitted body instead: a table whose content is declared once with
/// initializers must carry no `StoreTable` anywhere in `compute`, whatever the
/// signal-level model believed. Every table reaching a checked vector module is
/// read-only today because `lower_readonly_table_definition` rejects live write
/// ports; admitting mutable tables must extend this check rather than drop it.
pub(super) fn verify_readonly_table_stores(
    store: &FirStore,
    compute: FirId,
    static_declarations: &[FirId],
) -> Result<(), VectorModuleFailure> {
    let readonly = static_declarations
        .iter()
        .filter_map(|declaration| match match_fir(store, *declaration) {
            FirMatch::DeclareTable { name, .. } => Some(name),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if readonly.is_empty() {
        return Ok(());
    }
    for node in fir_reachable(store, compute) {
        let FirMatch::StoreTable { name, .. } = match_fir(store, node) else {
            continue;
        };
        if readonly.contains(&name) {
            return Err(module_shape(format!(
                "compute stores into read-only table {name}"
            )));
        }
    }
    Ok(())
}
/// Everything the final independent module check compares the emitted FIR
/// against.
pub(super) struct FinalModuleExpectations<'a> {
    pub(super) assembly: &'a VectorFirAssembly,
    pub(super) output_stores: &'a [FirId],
    pub(super) ui_fir: &'a VectorUiFir,
    pub(super) static_declarations: &'a [FirId],
    pub(super) table_declarations: &'a [FirId],
    pub(super) ui: &'a ui::UiProgram,
    pub(super) plan: &'a VectorPlan,
}
pub(super) fn verify_final_module(
    store: &FirStore,
    module: FirId,
    expected: &FinalModuleExpectations<'_>,
) -> Result<(), VectorModuleFailure> {
    let FinalModuleExpectations {
        assembly,
        output_stores,
        ui_fir,
        static_declarations: expected_static_declarations,
        table_declarations,
        ui,
        plan,
    } = *expected;
    let report = verify_fir_module(store, module);
    let has_dead_pure_drop = report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "FIR-D01");
    if report.has_errors() || has_dead_pure_drop {
        let detail = report
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.severity == fir::checker::Severity::Error || diagnostic.code == "FIR-D01"
            })
            .map(|diagnostic| format!("{} {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(VectorModuleFailure::new(
            VectorFallbackReason::ModuleVerification,
            detail,
        ));
    }
    let FirMatch::Module {
        dsp_struct,
        functions,
        static_decls,
        ..
    } = match_fir(store, module)
    else {
        return Err(module_shape("root is not a FIR module"));
    };
    if match_fir(store, static_decls) != FirMatch::Block(expected_static_declarations.to_vec()) {
        return Err(module_shape(
            "module does not contain the exact checked static declarations",
        ));
    }
    let FirMatch::Block(fields) = match_fir(store, dsp_struct) else {
        return Err(module_shape("DSP struct is not a block"));
    };
    if assembly
        .state_declarations
        .iter()
        .any(|declaration| !fields.contains(declaration))
    {
        return Err(module_shape("P6 state declaration missing from DSP struct"));
    }
    if ui_fir
        .struct_declarations
        .iter()
        .any(|declaration| !fields.contains(declaration))
    {
        return Err(module_shape("UI zone declaration missing from DSP struct"));
    }
    if table_declarations
        .iter()
        .any(|declaration| !fields.contains(declaration))
    {
        return Err(module_shape(
            "mutable table declaration missing from DSP struct",
        ));
    }
    let FirMatch::Block(functions) = match_fir(store, functions) else {
        return Err(module_shape("function section is not a block"));
    };
    let bodies = functions
        .iter()
        .filter_map(|function| match match_fir(store, *function) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } => Some((name, body)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    for required in [
        "metadata",
        "instanceConstants",
        "instanceResetUserInterface",
        "instanceClear",
        "buildUserInterface",
        "compute",
    ] {
        if !bodies.contains_key(required) {
            return Err(module_shape(format!(
                "missing lifecycle function {required}"
            )));
        }
    }
    if match_fir(store, bodies["instanceClear"])
        != FirMatch::Block(assembly.clear_statements.clone())
    {
        return Err(module_shape(
            "instanceClear does not contain exact P6 clears",
        ));
    }
    if match_fir(store, bodies["instanceResetUserInterface"])
        != FirMatch::Block(ui_fir.reset_statements.clone())
    {
        return Err(module_shape(
            "instanceResetUserInterface does not contain exact UI resets",
        ));
    }
    if match_fir(store, bodies["buildUserInterface"])
        != FirMatch::Block(ui_fir.build_statements.clone())
    {
        return Err(module_shape(
            "buildUserInterface does not contain exact grouped UI program",
        ));
    }
    let compute = bodies["compute"];
    if !contains_statement(store, compute, assembly.top_level_statement) {
        return Err(module_shape(
            "compute does not contain the accepted P6.3b body",
        ));
    }
    verify_readonly_table_stores(store, compute, expected_static_declarations)?;
    verify_sound_field_immutability(store, compute, &fields)?;
    verify_ui_write_attribution(store, compute, ui, plan)?;
    verify_mutable_table_attribution(
        store,
        compute,
        bodies["instanceConstants"],
        table_declarations,
        plan,
    )?;
    for output in output_stores {
        if !contains_statement(store, compute, *output) {
            return Err(module_shape("compute does not cover every output store"));
        }
    }
    Ok(())
}
pub(super) fn contains_statement(store: &FirStore, root: FirId, target: FirId) -> bool {
    if root == target {
        return true;
    }
    match match_fir(store, root) {
        FirMatch::Block(body) => body
            .into_iter()
            .any(|child| contains_statement(store, child, target)),
        FirMatch::If {
            then_block,
            else_block,
            ..
        } => {
            contains_statement(store, then_block, target)
                || else_block.is_some_and(|body| contains_statement(store, body, target))
        }
        FirMatch::Control { stmt, .. } => contains_statement(store, stmt, target),
        FirMatch::ForLoop { body, .. }
        | FirMatch::SimpleForLoop { body, .. }
        | FirMatch::IteratorForLoop { body, .. }
        | FirMatch::WhileLoop { body, .. } => contains_statement(store, body, target),
        FirMatch::Switch { cases, default, .. } => {
            cases
                .into_iter()
                .any(|(_, body)| contains_statement(store, body, target))
                || default.is_some_and(|body| contains_statement(store, body, target))
        }
        _ => false,
    }
}
pub(super) fn module_shape(detail: impl Into<String>) -> VectorModuleFailure {
    VectorModuleFailure::new(VectorFallbackReason::ModuleVerification, detail)
}
