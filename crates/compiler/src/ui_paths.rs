//! Runtime UI address conflict checking.
//!
//! # Source provenance (C++)
//! - `compiler/generator/json_instructions.hh`
//! - `"ERROR : path '<address>' is already used"`
//!
//! # Role in pipeline
//! The conflict is found by the transform's fast lane, once dead widgets have
//! been pruned, on the interface the module shows
//! (`SignalFirErrorCode::UiDuplicatePath`, `crates/transform/src/signal_fir/mod.rs`):
//! the reference checks the paths it writes to its JSON, which are those of
//! the widgets it generated, so a dead widget at a live widget's address is no
//! conflict in either compiler. This module renders that error for the facade
//! (`transform_error_to_compiler`), labeling every declaration. Until
//! 2026-09-23 the check ran right after propagation, on every widget of the
//! box tree, and rejected such programs where the reference accepts them.
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

/// The facade error for controls the interface shows at one runtime address.
///
/// `conflicts` are the transform's, found on the pruned interface and already
/// limited to input conflicts (bargraph-only collisions are ambiguous rather
/// than broken, exactly as in C++, and belong to the warning channel). The
/// labels come from `program`, the whole registry: the control ids are the
/// same before and after pruning.
pub(crate) fn ui_layout_error(
    source: &str,
    program: &UiProgram,
    ctx: &parser::ParserCtx,
    source_map: &SourceMap,
    conflicts: Vec<DuplicateControlPath>,
) -> CompilerError {
    let mut diagnostics = DiagnosticBundle::new();
    for conflict in &conflicts {
        diagnostics.push(duplicate_path_diagnostic(program, ctx, conflict));
    }
    diagnostics.set_source_map(source_map.clone());
    CompilerError::UiLayout {
        source: source.into(),
        conflicts,
        diagnostics,
    }
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
