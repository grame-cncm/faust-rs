//! The widget table behind the C/C++ `-uim` user-interface macros.
//!
//! # C++ provenance
//! `InstructionsCompiler::generateMacroInterfaceTree`,
//! `generateMacroInterfaceElements` and `generateWidgetMacro`
//! (`compiler/generator/instructions_compiler.cpp`): a walk of the UI tree
//! that the C and C++ containers print as `FAUST_ADD*` lines and
//! `FAUST_LIST_ACTIVES`/`FAUST_LIST_PASSIVES` entries
//! (`CodeContainer::printMacros`).
//!
//! # Invariants
//! - The table is built from the interface the module shows, after dead
//!   widgets were pruned, in the order of `buildUserInterface`.
//! - Zone names are those of the emitted DSP state
//!   ([`super::vector::ui::zone_name`]), so every macro names a real field.
//! - A path is the raw labels of the enclosing groups, inline metadata
//!   included, each followed by `/`, then the widget label without its
//!   metadata (`"[1]Main [style:knob]/gain"`), as the reference prints it. A
//!   group with an empty raw label (the synthesized root, `hgroup("", ...)`)
//!   adds no segment.

use ui::{ControlKind, ControlRange, UiMatch, UiProgram, match_ui};

/// One widget of the `-uim` macro table.
#[derive(Clone, Debug, PartialEq)]
pub struct UiMacroWidget {
    /// Widget family, which picks the macro (`FAUST_ADDHORIZONTALSLIDER`, ...).
    pub kind: ControlKind,
    /// Raw enclosing group labels and the widget label, `/`-separated.
    pub path: String,
    /// Widget label with its inline metadata removed.
    pub label: String,
    /// Name of the DSP state field holding the widget's value.
    pub zone: String,
    /// Init/min/max/step of a slider or numeric entry, min/max of a
    /// bargraph; `None` for buttons, checkboxes and soundfiles.
    pub range: Option<ControlRange>,
}

/// The `-uim` widget table of `program`, in interface order.
#[must_use]
pub fn ui_macro_widgets(program: &UiProgram) -> Vec<UiMacroWidget> {
    let mut widgets = Vec::new();
    collect(program, program.root, &mut String::new(), &mut widgets);
    widgets
}

fn collect(program: &UiProgram, node: ui::UiId, path: &mut String, out: &mut Vec<UiMacroWidget>) {
    match match_ui(&program.arena, node) {
        UiMatch::Group {
            children,
            raw_label,
            ..
        } => {
            let len = path.len();
            if !raw_label.is_empty() {
                path.push_str(raw_label);
                path.push('/');
            }
            for child in children {
                collect(program, child, path, out);
            }
            path.truncate(len);
        }
        UiMatch::InputControl(control)
        | UiMatch::OutputControl(control)
        | UiMatch::Soundfile(control) => {
            if let Some(spec) = program.control(control) {
                out.push(UiMacroWidget {
                    kind: spec.kind,
                    path: format!("{path}{}", spec.label),
                    label: spec.label.clone(),
                    zone: super::vector::ui::zone_name(spec.kind, control),
                    range: spec.range,
                });
            }
        }
        UiMatch::Unknown => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tlib::TreeArena;
    use ui::{ControlSpec, UiBuilder, UiGroupKind, UiRootOrigin};

    fn spec(id: u32, kind: ControlKind, label: &str) -> ControlSpec {
        ControlSpec {
            id,
            kind,
            label: label.to_owned(),
            metadata: Vec::new(),
            range: None,
            source_node: None,
        }
    }

    /// The structure of the reference's paths: raw group labels with their
    /// metadata, unnamed groups skipped, interface order kept.
    #[test]
    fn paths_keep_raw_group_labels_and_skip_unnamed_groups() {
        let mut arena = TreeArena::new();
        let mut b = UiBuilder::new(&mut arena);
        let gain = b.input_control(0);
        let freq = b.input_control(1);
        let level = b.output_control(2);
        let main = b.group_with_raw_label(
            UiGroupKind::Vertical,
            "Main",
            "[1]Main [style:knob]",
            &[
                ("1".to_owned(), String::new()),
                ("style".to_owned(), "knob".to_owned()),
            ],
            &[gain],
        );
        let unnamed = b.group_with_raw_label(UiGroupKind::Horizontal, "", "", &[], &[freq]);
        let root = b.group_with_raw_label(
            UiGroupKind::Vertical,
            "t1",
            "",
            &[],
            &[unnamed, main, level],
        );
        let program = UiProgram {
            arena,
            root,
            controls: vec![
                spec(0, ControlKind::HSlider, "gain"),
                spec(1, ControlKind::NumEntry, "freq"),
                spec(2, ControlKind::HBargraph, "level"),
            ],
            root_origin: UiRootOrigin::Synthesized,
            emit_ui: true,
            pinned: Default::default(),
        };

        let widgets = ui_macro_widgets(&program);
        let paths: Vec<(&str, &str)> = widgets
            .iter()
            .map(|w| (w.path.as_str(), w.zone.as_str()))
            .collect();
        assert_eq!(
            paths,
            [
                ("freq", "fEntry1"),
                ("[1]Main [style:knob]/gain", "fHslider0"),
                ("level", "fHbargraph2"),
            ]
        );
    }
}
