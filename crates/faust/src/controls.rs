//! The controls of an instance, discovered through the backend's UI
//! builder and addressed by path.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};

use ffi_common::ControlRange;
use ffi_common::abi::{FfiFaustFloat, MetaGlue, UIGlue};

use crate::Precision;

/// The kind of a control: what the DSP reads or writes through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlKind {
    /// `button`: 1 while pressed, 0 otherwise.
    Button,
    /// `checkbox`: 0 or 1.
    CheckButton,
    /// `hslider`.
    HorizontalSlider,
    /// `vslider`.
    VerticalSlider,
    /// `nentry`.
    NumEntry,
    /// `hbargraph`: written by the DSP, read by the host.
    HorizontalBargraph,
    /// `vbargraph`: written by the DSP, read by the host.
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

/// One control of an instance. `init`, `min`, `max` and `step` are the
/// values the program declares, at its compiled precision: exact for a
/// `-double` program, the `f32` values its zones hold otherwise. A bargraph
/// declares no initial value nor step: `init` is its `min`, `step` is 0.
///
/// Built by this crate only (`#[non_exhaustive]`): fields may be added
/// without breaking the hosts that read them.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Control {
    /// Its address, `/group/.../label`, as the C++ `MapUI` and OSC build it:
    /// the characters an OSC address cannot hold are replaced by `_`, so the
    /// label cannot be read back from it.
    pub path: String,
    /// Its shortest unambiguous name, as the C++ `MapUI` builds it
    /// (`PathBuilder::computeShortNames`): the last segment of its path
    /// (`freq`), or, when another control has the same one, as many of its
    /// last segments as tell them apart, joined by `_` (`osc0_freq`,
    /// `osc1_freq`). Only letters and digits are kept, a run of other
    /// characters becoming one `_`.
    pub shortname: String,
    /// The label as the program wrote it, without its `[key:value]`
    /// metadata: `"my gain"` for `hslider("my gain [unit:dB]", ...)`.
    pub label: String,
    /// The widget that declares it.
    pub kind: ControlKind,
    /// The value it takes at initialisation and after
    /// [`Dsp::instance_reset_user_interface`](crate::Dsp::instance_reset_user_interface).
    pub init: f64,
    /// The lower bound of its range.
    pub min: f64,
    /// The upper bound of its range.
    pub max: f64,
    /// The step of its range: 1 for a button or a checkbox, whose range is
    /// `[0, 1]`, 0 for a bargraph.
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

/// The controls of one instance, in the order the UI builder declared them
/// (the order of the UI tree, where Faust sorts a group's widgets by label,
/// `[n]` prefixes included), with the three indexes of the C++ `MapUI`: by
/// path, by shortname and by label.
pub(crate) struct ControlMap {
    entries: Vec<Entry>,
    /// Position in `entries` of each path.
    by_path: HashMap<String, usize>,
    /// Position in `entries` of each shortname, filled by `finish`.
    by_shortname: HashMap<String, usize>,
    /// Position in `entries` of each label, filled by `finish`: of the last
    /// control declared with it, as the C++ `MapUI`'s `std::map` keeps.
    by_label: HashMap<String, usize>,
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
            entries: Vec::new(),
            by_path: HashMap::new(),
            by_shortname: HashMap::new(),
            by_label: HashMap::new(),
            groups: Vec::new(),
            pending_metadata: Vec::new(),
            precision,
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Control> {
        self.entries.iter().map(|e| &e.control)
    }

    /// Computes the shortnames and the label index, once the UI builder has
    /// declared every control (`MapUI::closeBox` of the outermost group).
    pub(crate) fn finish(&mut self) {
        let paths: Vec<String> = self
            .entries
            .iter()
            .map(|e| e.control.path.clone())
            .collect();
        let shortnames = codegen::shortname::compute_short_names(&paths);
        for (i, entry) in self.entries.iter_mut().enumerate() {
            let shortname = shortnames[&entry.control.path].clone();
            self.by_shortname.insert(shortname.clone(), i);
            self.by_label.insert(entry.control.label.clone(), i);
            entry.control.shortname = shortname;
        }
    }

    /// The control `key` names, looked up as the C++ `MapUI::setParamValue`
    /// does: as a path, then as a shortname, then as a label.
    fn entry(&self, key: &str) -> Option<&Entry> {
        self.by_path
            .get(key)
            .or_else(|| self.by_shortname.get(key))
            .or_else(|| self.by_label.get(key))
            .map(|&i| &self.entries[i])
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Control> {
        self.entry(key).map(|e| &e.control)
    }

    pub(crate) fn read(&self, key: &str) -> Option<f64> {
        let entry = self.entry(key)?;
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

    /// Writes `value`; `None` when `key` names no control, `Some(false)` when
    /// the control is read-only.
    pub(crate) fn write(&self, key: &str, value: f64) -> Option<bool> {
        let entry = self.entry(key)?;
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

    /// Replaces the ranges the UI builder passed, narrowed to the C ABI's
    /// `float`, by `ranges`, at the program's precision, matched by zone. A
    /// bargraph keeps its convention: its initial value is its minimum.
    pub(crate) fn apply_ranges(&mut self, ranges: &[ControlRange]) {
        for range in ranges {
            let Some(entry) = self.entries.iter_mut().find(|e| e.zone == range.zone) else {
                continue;
            };
            let control = &mut entry.control;
            control.min = range.min;
            control.max = range.max;
            if range.bargraph {
                control.init = range.min;
            } else {
                control.init = range.init;
                control.step = range.step;
            }
        }
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

    /// The path of `label` in the groups currently open, as the C++ `MapUI`
    /// builds it ([`codegen::shortname::build_path`]).
    fn path_for(&self, label: &str) -> String {
        codegen::shortname::build_path(&self.groups, label)
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
            shortname: String::new(),
            label: label.to_owned(),
            kind,
            init,
            min,
            max,
            step,
            metadata,
        };
        // The compiler refuses two widgets with one path. Should one arrive,
        // the later zone replaces the earlier, as in the C++ `MapUI`, at the
        // earlier's position.
        let entry = Entry { control, zone };
        match self.by_path.get(&path) {
            Some(&i) => self.entries[i] = entry,
            None => {
                self.by_path.insert(path, self.entries.len());
                self.entries.push(entry);
            }
        }
    }
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
