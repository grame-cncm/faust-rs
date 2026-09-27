//! The factory JSON, in the format of the C++ `interpreter_dsp_factory::getJSON`.
//!
//! # Source provenance (C++)
//! - `interpreter_dsp_factory::getJSON` (`interpreter_dsp_aux.cpp`): a
//!   `JSONUI(getName(), "", inputs, outputs)` filled by `buildUserInterface`
//!   and `metadata` on a temporary instance, returned as `JSON(true)`;
//! - `JSONUIReal` (`architecture/faust/gui/JSONUI.h`) and `PathBuilder`
//!   (`architecture/faust/gui/PathBuilder.h`) for the fields, the addresses
//!   and the short names.
//!
//! # API mapping status
//! - adapted: the same description is rebuilt from the factory's UI and
//!   metadata instructions, which is what those two calls replay, without an
//!   instance, and rendered by [`codegen::json::JsonDescription::render_flat`].
//!   A single-precision value is written as the shortest decimal that reads
//!   back as the same `float` (`0.1`, as the C++ stream writes it), a double
//!   one exactly; the C++ stream rounds both to six significant digits.

use codegen::backends::interp::{FbcMetaInstruction, FbcOpcode, FbcReal, FbcUiInstruction};
use codegen::json::{
    JsonDescription, JsonMetaEntry, JsonRange, JsonUiItem, JsonWidget, assign_short_names,
};

use crate::types::FbcDspFactoryAny;

/// The JSON description of `factory`, as the C++ `getJSON` returns it.
pub(crate) fn factory_json(factory: &FbcDspFactoryAny) -> String {
    let ui = match factory {
        FbcDspFactoryAny::Float32(f) => ui_tree(&f.ui_block, shortest_f32),
        FbcDspFactoryAny::Float64(f) => ui_tree(&f.ui_block, |x| x),
    };
    let meta = meta_entries(factory.meta_block());
    // `JSONUI::declare` takes the file name from the metadata when it was
    // given none (the name is the factory's, never empty)
    let filename = meta
        .iter()
        .find(|m| m.key == "filename")
        .map_or_else(String::new, |m| m.value.clone());
    JsonDescription {
        name: factory.name().to_owned(),
        backend: None,
        jit_compiled: None,
        compute_body_lowered: None,
        // `JSONUI(name, "", ...)`: no version, no options
        filename: Some(filename),
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: None,
        inputs: usize::try_from(factory.num_inputs()).unwrap_or(0),
        outputs: usize::try_from(factory.num_outputs()).unwrap_or(0),
        sr_index: None,
        memory: None,
        meta,
        ui,
    }
    .render_flat()
}

/// The `float` `x` as the shortest `f64` decimal reading back as `x`.
fn shortest_f32(x: f32) -> f64 {
    x.to_string().parse().unwrap_or(f64::from(x))
}

/// The program's `declare`s, as `metadata` replays them.
fn meta_entries(meta: &[FbcMetaInstruction]) -> Vec<JsonMetaEntry> {
    meta.iter()
        .map(|m| JsonMetaEntry {
            key: m.key.clone(),
            value: m.value.clone(),
        })
        .collect()
}

/// A group still open while the UI instructions are read.
struct OpenGroup {
    typ: &'static str,
    label: String,
    meta: Vec<JsonMetaEntry>,
    items: Vec<JsonUiItem>,
}

/// The UI tree `buildUserInterface` describes, with the addresses and short
/// names `JSONUI` gives the widgets. A `declare` goes to the next group or
/// widget, as `JSONUI::declare` holds it until then.
fn ui_tree<R: FbcReal>(ui: &[FbcUiInstruction<R>], widen: impl Fn(R) -> f64) -> Vec<JsonUiItem> {
    let mut top: Vec<JsonUiItem> = Vec::new();
    let mut open: Vec<OpenGroup> = Vec::new();
    let mut pending_meta: Vec<JsonMetaEntry> = Vec::new();
    for instr in ui {
        let group = match instr.opcode {
            FbcOpcode::OpenTabBox => Some("tgroup"),
            FbcOpcode::OpenHorizontalBox => Some("hgroup"),
            FbcOpcode::OpenVerticalBox => Some("vgroup"),
            _ => None,
        };
        if let Some(typ) = group {
            open.push(OpenGroup {
                typ,
                label: instr.label.clone(),
                meta: std::mem::take(&mut pending_meta),
                items: Vec::new(),
            });
            continue;
        }
        let ranged = |init: bool| JsonRange {
            init: init.then(|| widen(instr.init)),
            min: widen(instr.min),
            max: widen(instr.max),
            step: init.then(|| widen(instr.step)),
        };
        let (typ, range, url) = match instr.opcode {
            FbcOpcode::CloseBox => {
                if let Some(group) = open.pop() {
                    let item = JsonUiItem::Group {
                        typ: group.typ,
                        label: group.label,
                        meta: group.meta,
                        items: group.items,
                    };
                    add_item(&mut open, &mut top, item);
                }
                continue;
            }
            FbcOpcode::Declare => {
                pending_meta.push(JsonMetaEntry {
                    key: instr.key.clone(),
                    value: instr.value.clone(),
                });
                continue;
            }
            FbcOpcode::AddButton => ("button", None, None),
            FbcOpcode::AddCheckButton => ("checkbox", None, None),
            FbcOpcode::AddVerticalSlider => ("vslider", Some(ranged(true)), None),
            FbcOpcode::AddHorizontalSlider => ("hslider", Some(ranged(true)), None),
            FbcOpcode::AddNumEntry => ("nentry", Some(ranged(true)), None),
            FbcOpcode::AddHorizontalBargraph => ("hbargraph", Some(ranged(false)), None),
            FbcOpcode::AddVerticalBargraph => ("vbargraph", Some(ranged(false)), None),
            // the URL travels in `key`, as `dispatch_ui_*` passes it
            FbcOpcode::AddSoundfile => ("soundfile", None, Some(instr.key.clone())),
            _ => continue,
        };
        let widget = JsonWidget {
            typ,
            label: instr.label.clone(),
            // `buildUserInterface` passes no variable name
            varname: String::new(),
            // filled once every address is known
            shortname: String::new(),
            address: build_path(&open, &instr.label),
            index: None,
            meta: std::mem::take(&mut pending_meta),
            range,
            soundfile_url: url,
        };
        add_item(&mut open, &mut top, JsonUiItem::Widget(widget));
    }
    // a group left open by a malformed block is closed at the end
    while let Some(group) = open.pop() {
        let item = JsonUiItem::Group {
            typ: group.typ,
            label: group.label,
            meta: group.meta,
            items: group.items,
        };
        add_item(&mut open, &mut top, item);
    }
    assign_short_names(&mut top);
    top
}

/// Appends `item` to the innermost open group, or to the top level.
fn add_item(open: &mut [OpenGroup], top: &mut Vec<JsonUiItem>, item: JsonUiItem) {
    match open.last_mut() {
        Some(group) => group.items.push(item),
        None => top.push(item),
    }
}

/// The address of `label` in the open groups: `PathBuilder::buildPath`. A
/// `/` in a label becomes `_`, every group label counts (an empty one too),
/// then the characters awkward in an OSC address become `_`.
fn build_path(open: &[OpenGroup], label: &str) -> String {
    let mut path = String::from("/");
    for group in open {
        path.push_str(&group.label.replace('/', "_"));
        path.push('/');
    }
    path.push_str(&label.replace('/', "_"));
    path.chars()
        .map(|c| {
            if matches!(
                c,
                ' ' | '#' | '*' | ',' | '?' | '[' | ']' | '{' | '}' | '(' | ')'
            ) {
                '_'
            } else {
                c
            }
        })
        .collect()
}
