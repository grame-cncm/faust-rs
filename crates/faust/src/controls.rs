//! The controls of an instance, discovered through the backend's UI
//! builder and addressed by path.

use std::collections::BTreeMap;
use std::ffi::{CStr, c_char, c_void};

use ffi_common::abi::{FfiFaustFloat, MetaGlue, UIGlue};

use crate::Precision;

/// The kind of a control: what the DSP reads or writes through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlKind {
    Button,
    CheckButton,
    HorizontalSlider,
    VerticalSlider,
    NumEntry,
    HorizontalBargraph,
    VerticalBargraph,
}

impl ControlKind {
    /// Whether the host writes it (a bargraph is written by the DSP).
    pub fn is_writable(self) -> bool {
        !matches!(
            self,
            ControlKind::HorizontalBargraph | ControlKind::VerticalBargraph
        )
    }
}

/// One control of an instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    /// Its address, `/group/.../label`, as the C++ `MapUI` and OSC build it.
    pub path: String,
    pub kind: ControlKind,
    pub init: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// The `[key:value]` metadata declared on the control, in order.
    pub metadata: Vec<(String, String)>,
}

impl Control {
    /// `value` brought into `[min, max]`.
    pub fn clamp(&self, value: f64) -> f64 {
        value.clamp(self.min, self.max)
    }
}

/// A control with its zone, the memory cell of the instance's state it maps to.
struct Entry {
    control: Control,
    zone: *mut FfiFaustFloat,
}

/// The controls of one instance, by path.
pub(crate) struct ControlMap {
    entries: BTreeMap<String, Entry>,
    /// Group labels currently open, innermost last, while building.
    groups: Vec<String>,
    /// Metadata declared before the widget owning its zone arrives.
    pending_metadata: Vec<(*mut FfiFaustFloat, String, String)>,
    /// The width of every zone: the precision the backend exchanges.
    precision: Precision,
}

impl ControlMap {
    pub(crate) fn new(precision: Precision) -> Self {
        Self {
            entries: BTreeMap::new(),
            groups: Vec::new(),
            pending_metadata: Vec::new(),
            precision,
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Control> {
        self.entries.values().map(|e| &e.control)
    }

    pub(crate) fn get(&self, path: &str) -> Option<&Control> {
        self.entries.get(path).map(|e| &e.control)
    }

    pub(crate) fn read(&self, path: &str) -> Option<f64> {
        let entry = self.entries.get(path)?;
        // SAFETY: the zone points into the state of the instance this map
        // belongs to, alive as long as the `Dsp` that owns the map, and its
        // width is the precision recorded at construction.
        Some(unsafe {
            match self.precision {
                Precision::F32 => f64::from(entry.zone.read()),
                Precision::F64 => entry.zone.cast::<f64>().read(),
            }
        })
    }

    /// Writes `value`; `None` when the path is unknown, `Some(false)` when the
    /// control is read-only.
    pub(crate) fn write(&self, path: &str, value: f64) -> Option<bool> {
        let entry = self.entries.get(path)?;
        if !entry.control.kind.is_writable() {
            return Some(false);
        }
        // SAFETY: as in `read`.
        unsafe {
            match self.precision {
                Precision::F32 => entry.zone.write(value as f32),
                Precision::F64 => entry.zone.cast::<f64>().write(value),
            }
        }
        Some(true)
    }

    /// The callback table that fills this map; `self` must not move while a
    /// `buildUserInterface` call uses it.
    pub(crate) fn glue(&mut self) -> UIGlue {
        UIGlue {
            ui_interface: (self as *mut Self).cast::<c_void>(),
            open_tab_box: Some(open_box),
            open_horizontal_box: Some(open_box),
            open_vertical_box: Some(open_box),
            close_box: Some(close_box),
            add_button: Some(add_button),
            add_check_button: Some(add_check_button),
            add_vertical_slider: Some(add_vertical_slider),
            add_horizontal_slider: Some(add_horizontal_slider),
            add_num_entry: Some(add_num_entry),
            add_horizontal_bargraph: Some(add_horizontal_bargraph),
            add_vertical_bargraph: Some(add_vertical_bargraph),
            add_soundfile: None,
            declare: Some(declare),
        }
    }

    /// The path of `label` in the groups currently open: `PathBuilder::buildPath`
    /// of the C++ architecture files. A `/` inside the label becomes `_` so the
    /// label stays one segment; then the characters awkward in an OSC address
    /// are replaced over the whole path.
    fn path_for(&self, label: &str) -> String {
        let label = replace_chars(label, &['/'], '_');
        let mut path = String::new();
        for group in self.groups.iter().filter(|g| !g.is_empty()) {
            path.push('/');
            path.push_str(group);
        }
        path.push('/');
        path.push_str(&label);
        replace_chars(
            &path,
            &[' ', '#', '*', ',', '?', '[', ']', '{', '}', '(', ')'],
            '_',
        )
    }

    fn add(&mut self, label: &str, kind: ControlKind, zone: *mut FfiFaustFloat, range: [f32; 4]) {
        if zone.is_null() {
            return;
        }
        let path = self.path_for(label);
        let mut metadata = Vec::new();
        self.pending_metadata.retain(|(z, key, value)| {
            if *z == zone {
                metadata.push((key.clone(), value.clone()));
                false
            } else {
                true
            }
        });
        let [init, min, max, step] = range.map(f64::from);
        let control = Control {
            path: path.clone(),
            kind,
            init,
            min,
            max,
            step,
            metadata,
        };
        self.entries.insert(path, Entry { control, zone });
    }
}

fn replace_chars(text: &str, targets: &[char], replacement: char) -> String {
    text.chars()
        .map(|c| if targets.contains(&c) { replacement } else { c })
        .collect()
}

unsafe fn text_of(label: *const c_char) -> String {
    if label.is_null() {
        return String::new();
    }
    // SAFETY: the backend passes NUL-terminated labels.
    unsafe { CStr::from_ptr(label) }
        .to_string_lossy()
        .into_owned()
}

unsafe fn map_of<'a>(ui: *mut c_void) -> Option<&'a mut ControlMap> {
    if ui.is_null() {
        return None;
    }
    // SAFETY: `ui` is the `ControlMap` pointer `glue` installed.
    Some(unsafe { &mut *ui.cast::<ControlMap>() })
}

unsafe extern "C" fn open_box(ui: *mut c_void, label: *const c_char) {
    if let Some(map) = unsafe { map_of(ui) } {
        let name = unsafe { text_of(label) };
        map.groups.push(name);
    }
}

unsafe extern "C" fn close_box(ui: *mut c_void) {
    if let Some(map) = unsafe { map_of(ui) } {
        map.groups.pop();
    }
}

macro_rules! two_state {
    ($name:ident, $kind:expr) => {
        unsafe extern "C" fn $name(
            ui: *mut c_void,
            label: *const c_char,
            zone: *mut FfiFaustFloat,
        ) {
            if let Some(map) = unsafe { map_of(ui) } {
                let name = unsafe { text_of(label) };
                map.add(&name, $kind, zone, [0.0, 0.0, 1.0, 1.0]);
            }
        }
    };
}

macro_rules! ranged {
    ($name:ident, $kind:expr) => {
        unsafe extern "C" fn $name(
            ui: *mut c_void,
            label: *const c_char,
            zone: *mut FfiFaustFloat,
            init: FfiFaustFloat,
            min: FfiFaustFloat,
            max: FfiFaustFloat,
            step: FfiFaustFloat,
        ) {
            if let Some(map) = unsafe { map_of(ui) } {
                let name = unsafe { text_of(label) };
                map.add(&name, $kind, zone, [init, min, max, step]);
            }
        }
    };
}

macro_rules! bargraph {
    ($name:ident, $kind:expr) => {
        unsafe extern "C" fn $name(
            ui: *mut c_void,
            label: *const c_char,
            zone: *mut FfiFaustFloat,
            min: FfiFaustFloat,
            max: FfiFaustFloat,
        ) {
            if let Some(map) = unsafe { map_of(ui) } {
                let name = unsafe { text_of(label) };
                map.add(&name, $kind, zone, [min, min, max, 0.0]);
            }
        }
    };
}

two_state!(add_button, ControlKind::Button);
two_state!(add_check_button, ControlKind::CheckButton);
ranged!(add_vertical_slider, ControlKind::VerticalSlider);
ranged!(add_horizontal_slider, ControlKind::HorizontalSlider);
ranged!(add_num_entry, ControlKind::NumEntry);
bargraph!(add_horizontal_bargraph, ControlKind::HorizontalBargraph);
bargraph!(add_vertical_bargraph, ControlKind::VerticalBargraph);

unsafe extern "C" fn declare(
    ui: *mut c_void,
    zone: *mut FfiFaustFloat,
    key: *const c_char,
    value: *const c_char,
) {
    if let Some(map) = unsafe { map_of(ui) } {
        // a null zone is a declaration on the enclosing group: not a control's
        if !zone.is_null() {
            let key = unsafe { text_of(key) };
            let value = unsafe { text_of(value) };
            map.pending_metadata.push((zone, key, value));
        }
    }
}

/// Collects the `declare` metadata of an instance.
pub(crate) struct MetadataSink(pub(crate) Vec<(String, String)>);

impl MetadataSink {
    pub(crate) fn glue(&mut self) -> MetaGlue {
        MetaGlue {
            meta_interface: (self as *mut Self).cast::<c_void>(),
            declare: Some(declare_meta),
        }
    }
}

unsafe extern "C" fn declare_meta(meta: *mut c_void, key: *const c_char, value: *const c_char) {
    if meta.is_null() {
        return;
    }
    // SAFETY: `meta` is the `MetadataSink` pointer `glue` installed.
    let sink = unsafe { &mut *meta.cast::<MetadataSink>() };
    sink.0
        .push((unsafe { text_of(key) }, unsafe { text_of(value) }));
}
