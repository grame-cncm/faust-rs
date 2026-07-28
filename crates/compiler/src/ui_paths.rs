//! Runtime UI address conflict checking.
//!
//! # Source provenance (C++)
//! - `compiler/generator/json_instructions.hh`
//! - `"ERROR : path '<address>' is already used"`
//!
//! # Role in pipeline
//! Runs immediately after propagation has produced the grouped [`UiProgram`],
//! before any FIR lowering or backend selection. C++ discovers the same
//! conflict while serializing JSON, which makes rejection depend on whether
//! JSON is generated; checking the `UiProgram` makes it depend only on the
//! program.
//!
//! # Design invariants
//! - Conflicts are ordered by address, and controls within one conflict keep UI
//!   declaration order, so the diagnostic is deterministic.
//! - Labels come from written widget declarations that carry the conflicting
//!   label. A declaration expanded several times (inside `par`, say) is labeled
//!   once; when no declaration carries the label, the diagnostic keeps its
//!   typed facts and emits no label rather than pointing at a nearby span.

use super::*;

use diagnostics::codes;
use ui::{DuplicateControlPath, UiProgram};

/// Rejects a program whose UI controls do not have distinct runtime addresses.
///
/// Returns `Ok(())` for the overwhelmingly common case of a conflict-free
/// program. The check is skipped for UI-free compilation paths, where
/// [`UiProgram::is_empty`] holds and no `buildUserInterface` is emitted.
pub(crate) fn check_ui_control_paths(
    source: &str,
    program: &UiProgram,
    ctx: &parser::ParserCtx,
    source_map: &SourceMap,
) -> Result<(), CompilerError> {
    // Bargraph-only collisions are ambiguous rather than broken, exactly as in
    // C++; they belong to the warning channel, not to rejection.
    let conflicts = ui::find_duplicate_control_paths(program)
        .into_iter()
        .filter(|conflict| conflict.kind == ui::DuplicatePathKind::InputConflict)
        .collect::<Vec<_>>();
    if conflicts.is_empty() {
        return Ok(());
    }

    let mut diagnostics = DiagnosticBundle::new();
    for conflict in &conflicts {
        diagnostics.push(duplicate_path_diagnostic(program, ctx, conflict));
    }
    diagnostics.set_source_map(source_map.clone());
    Err(CompilerError::UiLayout {
        source: source.into(),
        conflicts,
        diagnostics,
    })
}

/// Builds the `FRS-UI-0001` diagnostic for one conflicting address.
///
/// The last declaration is primary because it is the one that made the address
/// ambiguous; the earlier ones stay as `ConflictsWith` context. That mirrors
/// how the parser reports a redefined symbol, so the two duplicate-declaration
/// diagnostics read the same way.
fn duplicate_path_diagnostic(
    program: &UiProgram,
    ctx: &parser::ParserCtx,
    conflict: &DuplicateControlPath,
) -> Diagnostic {
    let address = conflict.address.clone();
    let label = address.rsplit('/').next().unwrap_or_default();
    let spans = widget_declaration_spans(ctx, label);

    let mut diagnostic = Diagnostic::new(
        Severity::Error,
        Stage::Propagate,
        codes::UI_DUPLICATE_PATH,
        format!(
            "UI path '{address}' is claimed by {} controls",
            conflict.controls.len()
        ),
    )
    .with_category(DiagnosticCategory::UserCode)
    .with_detail_code("duplicate-ui-path")
    .with_note("cause: two user-interface controls resolve to the same runtime address")
    .with_note("rule: every UI control must have a unique group path plus label")
    .with_note(format!(
        "computed: normalized path = {address}, claimed {} times",
        conflict.controls.len()
    ))
    .with_fact("ui_path", address.clone())
    .with_fact(
        "control_count",
        u64::try_from(conflict.controls.len()).unwrap_or(u64::MAX),
    )
    .with_fact(
        "control_labels",
        conflict
            .controls
            .iter()
            .map(|id| {
                program
                    .control(*id)
                    .map_or_else(|| "<unknown>".to_owned(), |control| control.label.clone())
            })
            .collect::<Vec<_>>(),
    )
    .with_help("rename one control, or place them in different groups")
    .with_help("group placement example: hgroup(\"left\", ...) and hgroup(\"right\", ...)");

    // The primary label is emitted first so renderers that show only the head
    // label point at the declaration that introduced the ambiguity.
    if let Some((last, earlier)) = spans.split_last() {
        diagnostic = diagnostic.with_label(
            Label::new(
                LabelStyle::Primary,
                last.clone(),
                if earlier.is_empty() {
                    "this declaration is instantiated more than once"
                } else {
                    "duplicate claim"
                },
            )
            .with_role(LabelRole::PrimaryCause),
        );
        for span in earlier {
            diagnostic = diagnostic.with_label(
                Label::new(
                    LabelStyle::Secondary,
                    span.clone(),
                    "first claim of this path",
                )
                .with_role(LabelRole::ConflictsWith),
            );
        }
    } else {
        diagnostic = diagnostic
            .with_note("note: no written widget declaration carries this label; the controls come from generated or loaded code");
    }
    diagnostic
}

/// Returns the written declarations whose effective label matches `label`.
///
/// The recorded label is the raw one, so it still carries the group pathname
/// and inline metadata a Faust label may embed. Both are stripped the same way
/// the UI builder strips them, so `hslider("h:Grp/gain [style:knob]", ...)`
/// matches the control named `gain`.
fn widget_declaration_spans(ctx: &parser::ParserCtx, label: &str) -> Vec<SourceSpan> {
    ctx.widget_declarations()
        .iter()
        .filter(|declaration| effective_widget_label(&declaration.raw_label) == label)
        .map(|declaration| {
            SourceSpan::new(
                declaration.location.file(),
                declaration.location.line(),
                declaration.location.col(),
                declaration.location.end_line(),
                declaration.location.end_col(),
            )
        })
        .collect()
}

/// Reduces one raw Faust widget label to the name the runtime address uses.
fn effective_widget_label(raw_label: &str) -> String {
    let path = ui::normalize_widget_label_path(raw_label, &[]);
    ui::split_label_metadata(&path.raw_label).0
}
