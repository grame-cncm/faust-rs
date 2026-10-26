//! The C/C++ `-uim` user-interface macros: the `#ifdef FAUST_UIMACROS` block
//! that architectures such as `minimal-static.cpp`, `teensy.cpp`,
//! `juce-plugin.cpp` or `clap-arch.cpp` expand to reach the DSP fields
//! directly.
//!
//! # C++ provenance
//! `CodeContainer::printMacros` (`compiler/generator/code_container.cpp`),
//! fed by `InstructionsCompiler::generateWidgetMacro`
//! (`compiler/generator/instructions_compiler.cpp`). Mapping status:
//! `adapted` — same macros, entries, order and arguments; the numbers use the
//! backend's own literal spelling (`440.0f` where the reference prints
//! `4.4e+02f`, the spelling `buildUserInterface` already uses), labels and
//! paths are escaped as C strings, and lines are indented with spaces.
//!
//! # Invariants
//! - `FAUST_ACTIVES` counts every non-bargraph widget, soundfiles included,
//!   while `FAUST_LIST_ACTIVES` leaves soundfiles out, as the reference does.
//! - A `p(...)` list line ends with `\`, so the line after each list is
//!   blank: it ends the macro.

use std::fmt::Write as _;

use crate::backends::c_family::{format_float32, string_literal, trim_float};

/// Widget family of one `-uim` macro entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiMacroKind {
    Button,
    Checkbox,
    VSlider,
    HSlider,
    NumEntry,
    VBargraph,
    HBargraph,
    Soundfile,
}

/// One widget of the `-uim` macro table, in interface order.
///
/// Equality compares the numbers bit for bit, so the backend option structs
/// holding a table stay `Eq`.
#[derive(Clone, Debug)]
pub struct UiMacroWidget {
    pub kind: UiMacroKind,
    /// Raw enclosing group labels and the widget label, `/`-separated
    /// (`"[1]Main [style:knob]/gain"`).
    pub path: String,
    /// Widget label without its metadata; its spaces become `_` in the
    /// identifier argument of `p(...)`.
    pub label: String,
    /// DSP state field holding the widget's value.
    pub zone: String,
    /// Slider/entry init, 0 otherwise.
    pub init: f64,
    /// Slider/entry/bargraph minimum, 0 otherwise.
    pub min: f64,
    /// Slider/entry/bargraph maximum, 0 otherwise.
    pub max: f64,
    /// Slider/entry step, 0 otherwise.
    pub step: f64,
}

impl PartialEq for UiMacroWidget {
    fn eq(&self, other: &Self) -> bool {
        let numbers = |w: &Self| [w.init, w.min, w.max, w.step].map(f64::to_bits);
        self.kind == other.kind
            && self.path == other.path
            && self.label == other.label
            && self.zone == other.zone
            && numbers(self) == numbers(other)
    }
}

impl Eq for UiMacroWidget {}

/// Everything the macro block prints besides the widgets.
pub(crate) struct UiMacroBlock<'a> {
    /// Source basename (`FAUST_FILE_NAME`), omitted when unknown.
    pub file_name: Option<&'a str>,
    pub class_name: &'a str,
    /// The `Compilation options:` line of the header.
    pub compile_options: &'a str,
    pub num_inputs: usize,
    pub num_outputs: usize,
    /// Internal precision: `double` literals unsuffixed, `float` ones with `f`.
    pub double_precision: bool,
    pub widgets: &'a [UiMacroWidget],
}

/// Appends the `#ifdef FAUST_UIMACROS ... #endif` block.
pub(crate) fn emit_ui_macros(out: &mut String, block: &UiMacroBlock<'_>) {
    let real = |value: f64| {
        if block.double_precision {
            trim_float(value)
        } else {
            format_float32(value)
        }
    };
    let is_passive =
        |kind: UiMacroKind| matches!(kind, UiMacroKind::VBargraph | UiMacroKind::HBargraph);
    let passives = block.widgets.iter().filter(|w| is_passive(w.kind)).count();

    let _ = writeln!(out);
    let _ = writeln!(out, "#ifdef FAUST_UIMACROS");
    let _ = writeln!(out);
    if let Some(file_name) = block.file_name {
        let _ = writeln!(
            out,
            "    #define FAUST_FILE_NAME {}",
            string_literal(file_name)
        );
    }
    let _ = writeln!(
        out,
        "    #define FAUST_CLASS_NAME {}",
        string_literal(block.class_name)
    );
    // The misspelling is the reference's, and architectures test this name.
    let _ = writeln!(
        out,
        "    #define FAUST_COMPILATION_OPIONS {}",
        string_literal(block.compile_options)
    );
    let _ = writeln!(out, "    #define FAUST_INPUTS {}", block.num_inputs);
    let _ = writeln!(out, "    #define FAUST_OUTPUTS {}", block.num_outputs);
    let _ = writeln!(
        out,
        "    #define FAUST_ACTIVES {}",
        block.widgets.len() - passives
    );
    let _ = writeln!(out, "    #define FAUST_PASSIVES {passives}");
    let _ = writeln!(out);

    for w in block.widgets {
        let path = string_literal(&w.path);
        let zone = &w.zone;
        let line = match w.kind {
            UiMacroKind::Button => format!("FAUST_ADDBUTTON({path}, {zone});"),
            UiMacroKind::Checkbox => format!("FAUST_ADDCHECKBOX({path}, {zone});"),
            UiMacroKind::VSlider | UiMacroKind::HSlider | UiMacroKind::NumEntry => {
                let name = match w.kind {
                    UiMacroKind::VSlider => "FAUST_ADDVERTICALSLIDER",
                    UiMacroKind::HSlider => "FAUST_ADDHORIZONTALSLIDER",
                    _ => "FAUST_ADDNUMENTRY",
                };
                format!(
                    "{name}({path}, {zone}, {}, {}, {}, {});",
                    real(w.init),
                    real(w.min),
                    real(w.max),
                    real(w.step)
                )
            }
            UiMacroKind::VBargraph | UiMacroKind::HBargraph => {
                let name = if w.kind == UiMacroKind::VBargraph {
                    "FAUST_ADDVERTICALBARGRAPH"
                } else {
                    "FAUST_ADDHORIZONTALBARGRAPH"
                };
                format!("{name}({path}, {zone}, {}, {});", real(w.min), real(w.max))
            }
            UiMacroKind::Soundfile => format!("FAUST_ADDSOUNDFILE({path}, {zone});"),
        };
        let _ = writeln!(out, "    {line}");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "    #define FAUST_LIST_ACTIVES(p) \\");
    for w in block.widgets {
        let (name, init, min, max, step) = match w.kind {
            UiMacroKind::Button => ("BUTTON", 0.0, 0.0, 1.0, 1.0),
            UiMacroKind::Checkbox => ("CHECKBOX", 0.0, 0.0, 1.0, 1.0),
            UiMacroKind::VSlider => ("VERTICALSLIDER", w.init, w.min, w.max, w.step),
            UiMacroKind::HSlider => ("HORIZONTALSLIDER", w.init, w.min, w.max, w.step),
            UiMacroKind::NumEntry => ("NUMENTRY", w.init, w.min, w.max, w.step),
            UiMacroKind::VBargraph | UiMacroKind::HBargraph | UiMacroKind::Soundfile => {
                continue;
            }
        };
        let _ = writeln!(
            out,
            "        p({name}, {}, {}, {}, {}, {}, {}, {}) \\",
            macro_identifier(&w.label),
            string_literal(&w.path),
            w.zone,
            real(init),
            real(min),
            real(max),
            real(step)
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "    #define FAUST_LIST_PASSIVES(p) \\");
    for w in block.widgets {
        let name = match w.kind {
            UiMacroKind::VBargraph => "VERTICALBARGRAPH",
            UiMacroKind::HBargraph => "HORIZONTALBARGRAPH",
            _ => continue,
        };
        let _ = writeln!(
            out,
            "        p({name}, {}, {}, {}, 0.0, {}, {}, 0.0) \\",
            macro_identifier(&w.label),
            string_literal(&w.path),
            w.zone,
            real(w.min),
            real(w.max)
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "#endif");
}

/// The identifier argument of `p(...)`: the label with its spaces replaced by
/// `_` (C++ `replace(rawlabel.begin(), rawlabel.end(), ' ', '_')`).
fn macro_identifier(label: &str) -> String {
    label.replace(' ', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(kind: UiMacroKind, path: &str, label: &str, zone: &str) -> UiMacroWidget {
        UiMacroWidget {
            kind,
            path: path.to_owned(),
            label: label.to_owned(),
            zone: zone.to_owned(),
            init: 0.5,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }
    }

    /// The block of the reference's `t1.dsp` example, entry for entry.
    #[test]
    fn block_lists_every_widget_in_the_reference_layout() {
        let widgets = [
            widget(UiMacroKind::HSlider, "Main/gain", "gain", "fHslider0"),
            widget(UiMacroKind::Button, "gate", "gate", "fButton1"),
            widget(
                UiMacroKind::HBargraph,
                "my level",
                "my level",
                "fHbargraph2",
            ),
            widget(UiMacroKind::Soundfile, "snd", "snd", "fSound3"),
        ];
        let mut out = String::new();
        emit_ui_macros(
            &mut out,
            &UiMacroBlock {
                file_name: Some("t1.dsp"),
                class_name: "mydsp",
                compile_options: "-lang cpp -uim -single",
                num_inputs: 1,
                num_outputs: 2,
                double_precision: false,
                widgets: &widgets,
            },
        );
        let expected = r#"
#ifdef FAUST_UIMACROS

    #define FAUST_FILE_NAME "t1.dsp"
    #define FAUST_CLASS_NAME "mydsp"
    #define FAUST_COMPILATION_OPIONS "-lang cpp -uim -single"
    #define FAUST_INPUTS 1
    #define FAUST_OUTPUTS 2
    #define FAUST_ACTIVES 3
    #define FAUST_PASSIVES 1

    FAUST_ADDHORIZONTALSLIDER("Main/gain", fHslider0, 0.5f, 0.0f, 1.0f, 0.01f);
    FAUST_ADDBUTTON("gate", fButton1);
    FAUST_ADDHORIZONTALBARGRAPH("my level", fHbargraph2, 0.0f, 1.0f);
    FAUST_ADDSOUNDFILE("snd", fSound3);

    #define FAUST_LIST_ACTIVES(p) \
        p(HORIZONTALSLIDER, gain, "Main/gain", fHslider0, 0.5f, 0.0f, 1.0f, 0.01f) \
        p(BUTTON, gate, "gate", fButton1, 0.0f, 0.0f, 1.0f, 1.0f) \

    #define FAUST_LIST_PASSIVES(p) \
        p(HORIZONTALBARGRAPH, my_level, "my level", fHbargraph2, 0.0, 0.0f, 1.0f, 0.0) \

#endif
"#;
        assert_eq!(out, expected);
    }

    #[test]
    fn double_precision_literals_are_unsuffixed() {
        let widgets = [widget(UiMacroKind::NumEntry, "n", "n", "fEntry0")];
        let mut out = String::new();
        emit_ui_macros(
            &mut out,
            &UiMacroBlock {
                file_name: None,
                class_name: "mydsp",
                compile_options: "-lang c -double",
                num_inputs: 0,
                num_outputs: 1,
                double_precision: true,
                widgets: &widgets,
            },
        );
        assert!(!out.contains("FAUST_FILE_NAME"));
        assert!(out.contains("FAUST_ADDNUMENTRY(\"n\", fEntry0, 0.5, 0.0, 1.0, 0.01);"));
    }
}
