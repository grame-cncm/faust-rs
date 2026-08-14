//! Control discovery: the `UIGlue` walk that yields a path → zone map.
//!
//! # Why this exists
//! The Cranelift runtime hands controls to a host through the `UIGlue`
//! callback table ([`ffi_common::abi::UIGlue`]): group opens and closes
//! interleaved with widget additions, each widget carrying the raw
//! `*mut FfiFaustFloat` zone it is bound to. Nothing in that stream is
//! addressable on its own — the full path of a control is only defined by the
//! stack of groups enclosing it at the moment it appears.
//!
//! Accumulating that stack while walking the callbacks reconstructs the
//! address of every control, which is the algorithm C++ Faust implements in
//! `architecture/faust/gui/PathBuilder.h` and exposes through `MapUI`.
//!
//! `getCCraneliftDSPFactoryJSON` also describes the controls, with richer
//! metadata, but it does not carry zone pointers. It can therefore back a
//! descriptive listing and nothing that needs to *write* a value.
//!
//! # Safety
//! The callbacks are `extern "C"` and receive an opaque host pointer that this
//! module sets to a [`ControlMap`]. Every callback rejects null pointers and
//! performs exactly one `&mut` reborrow, which is sound because the runtime
//! invokes them synchronously from `buildUserInterface` on the calling thread
//! and never retains them.

use std::collections::BTreeMap;
use std::ffi::{CStr, c_char, c_void};

use ffi_common::abi::{FfiFaustFloat, UIGlue};

use crate::probe::soundfile::{TestSoundfile, soundfile_part_count};

/// What kind of widget a control came from.
///
/// The distinction matters to callers: a [`ControlKind::Button`] is momentary
/// and is what the impulse-test protocol drives, while sliders and entries
/// carry a range that bounds any value written to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// Momentary `button`.
    Button,
    /// Toggle `checkbox`.
    CheckButton,
    /// `vslider` or `hslider`.
    Slider,
    /// `nentry`.
    NumEntry,
    /// `vbargraph` or `hbargraph` — an output, writing to it is meaningless.
    Bargraph,
}

impl ControlKind {
    /// Whether writing to this control affects the DSP.
    ///
    /// Bargraphs are DSP outputs: the runtime overwrites their zone on every
    /// `compute`, so a host-written value is discarded.
    #[must_use]
    pub const fn is_writable(self) -> bool {
        !matches!(self, Self::Bargraph)
    }
}

/// One discovered control: its address, the zone it writes to, and its range.
#[derive(Debug, Clone)]
pub struct Control {
    /// Full path, e.g. `/TIMBRE/filter_cutoff_hz`.
    pub path: String,
    /// Widget kind.
    pub kind: ControlKind,
    /// Raw DSP zone. Valid for as long as the instance that produced it.
    pub zone: *mut FfiFaustFloat,
    /// Initial value declared by the DSP.
    pub init: f64,
    /// Lower bound; `0.0` for buttons and checkboxes.
    pub min: f64,
    /// Upper bound; `1.0` for buttons and checkboxes.
    pub max: f64,
    /// Declared step; `1.0` for buttons and checkboxes.
    pub step: f64,
}

impl Control {
    /// Clamp `value` into the declared range.
    ///
    /// Faust hosts are expected to respect widget bounds; writing outside them
    /// is not rejected by the runtime but produces states the DSP was never
    /// compiled to expect (a negative delay length, say). Clamping here keeps
    /// a mistyped command line from being silently absurd.
    #[must_use]
    pub fn clamp(&self, value: f64) -> f64 {
        value.clamp(self.min, self.max)
    }
}

/// Controls discovered from one instance, addressable by path.
///
/// Ordered by path so `--list-params` output and any serialized form are
/// stable across runs, which matters for diffable test artifacts.
#[derive(Debug, Default)]
pub struct ControlMap {
    controls: BTreeMap<String, Control>,
    /// Group stack maintained during the walk; empty once it completes.
    groups: Vec<String>,
    /// Soundfile fixtures kept alive for as long as the DSP may read them.
    ///
    /// A DSP declaring `soundfile(...)` gets a pointer the host must fill in.
    /// Leaving it null is a segfault on the first `compute`, not a diagnostic,
    /// so the same in-memory reader the impulse runner installs is installed
    /// here — and owned here, because the DSP holds a bare pointer to it.
    soundfiles: Vec<TestSoundfile>,
}

/// How a lookup by fragment resolved.
#[derive(Debug)]
pub enum Resolution<'a> {
    /// Exactly one control matched.
    Unique(&'a Control),
    /// No control matched.
    NotFound,
    /// Several controls matched; the candidates are listed for the error.
    Ambiguous(Vec<String>),
}

impl ControlMap {
    /// Every control, ordered by path.
    pub fn iter(&self) -> impl Iterator<Item = &Control> {
        self.controls.values()
    }

    /// Number of discovered controls.
    #[must_use]
    pub fn len(&self) -> usize {
        self.controls.len()
    }

    /// Whether the DSP exposes no control at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }

    /// Look a control up by full path.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&Control> {
        self.controls.get(path)
    }

    /// Resolve a full path, or a trailing fragment of one.
    ///
    /// Full addresses are unwieldy on a command line and change whenever a
    /// group is renamed, so a fragment such as `filter_cutoff_hz` resolves
    /// against `/TIMBRE/filter_cutoff_hz`. Ambiguity is reported rather than
    /// resolved arbitrarily: silently picking the first of several matches is
    /// how a probe ends up measuring the wrong control.
    #[must_use]
    pub fn resolve(&self, query: &str) -> Resolution<'_> {
        if let Some(exact) = self.controls.get(query) {
            return Resolution::Unique(exact);
        }
        let suffix = if query.starts_with('/') {
            query.to_owned()
        } else {
            format!("/{query}")
        };
        let matches: Vec<&Control> = self
            .controls
            .values()
            .filter(|c| c.path.ends_with(&suffix))
            .collect();
        match matches.as_slice() {
            [] => Resolution::NotFound,
            [only] => Resolution::Unique(only),
            many => Resolution::Ambiguous(many.iter().map(|c| c.path.clone()).collect()),
        }
    }

    /// Paths whose last segment equals `segment`.
    ///
    /// The polyphonic wrapper selects its voice controls this way: `poly-dsp.h`
    /// matches `/gate`, `/freq`, `/key`, `/gain`, `/vel` and `/velocity` as
    /// path suffixes rather than by exact address, because the voice DSP is
    /// free to nest them in any group.
    #[must_use]
    pub fn ending_with(&self, segment: &str) -> Vec<&Control> {
        let suffix = format!("/{segment}");
        self.controls
            .values()
            .filter(|c| c.path.ends_with(&suffix))
            .collect()
    }

    /// Build the path currently in scope for `label`.
    ///
    /// Reproduces `PathBuilder::buildPath` (`architecture/faust/gui/PathBuilder.h:212`)
    /// exactly, because these addresses are the contract a host sees and must
    /// agree with what C++ `MapUI` reports for the same DSP:
    ///
    /// 1. `/` inside the *label* becomes `_`, so a label like `osc0/volume`
    ///    stays one path segment instead of silently introducing a group;
    /// 2. the enclosing group levels are joined with `/`;
    /// 3. a second pass over the *whole* path replaces the characters that
    ///    would be awkward in an OSC address.
    ///
    /// Skipping step 1 was the first bug this function had: `faust-probe`
    /// reported `/TIMBRE/amp_env/attack_s` where the C++ host reports
    /// `/TIMBRE/amp_env_attack_s`, so every address a user had from another
    /// tool failed to resolve.
    fn path_for(&self, label: &str) -> String {
        let label = replace_chars(label, &['/'], '_');
        let mut path = String::new();
        for group in &self.groups {
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

    fn insert(&mut self, label: &str, control: Control) {
        let _ = label;
        self.controls.insert(control.path.clone(), control);
    }

    /// Callback table bound to this map.
    ///
    /// # Safety
    /// The returned glue borrows `self` mutably for as long as it is used. The
    /// caller must pass it to exactly one `buildUserInterface` call and must
    /// not move `self` while that call is in flight.
    pub fn glue(&mut self) -> UIGlue {
        UIGlue {
            ui_interface: (self as *mut Self).cast::<c_void>(),
            open_tab_box: Some(open_box),
            open_horizontal_box: Some(open_box),
            open_vertical_box: Some(open_box),
            close_box: Some(close_box),
            add_button: Some(add_button),
            add_check_button: Some(add_check_button),
            add_vertical_slider: Some(add_slider),
            add_horizontal_slider: Some(add_slider),
            add_num_entry: Some(add_num_entry),
            add_horizontal_bargraph: Some(add_bargraph),
            add_vertical_bargraph: Some(add_bargraph),
            add_soundfile: Some(add_soundfile),
            declare: None,
        }
    }
}

/// Replace every character in `targets` with `replacement`.
///
/// Mirrors `replaceCharList` from `architecture/faust/gui/PathBuilder.h`.
fn replace_chars(text: &str, targets: &[char], replacement: char) -> String {
    text.chars()
        .map(|c| if targets.contains(&c) { replacement } else { c })
        .collect()
}

/// Decode a callback label, mapping null or invalid UTF-8 to an empty string.
///
/// Faust labels are ASCII in practice; tolerating the pathological case keeps
/// a malformed label from aborting discovery of every later control.
unsafe fn label_of(label: *const c_char) -> String {
    if label.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(label) }
        .to_str()
        .unwrap_or_default()
        .to_owned()
}

/// Reborrow the host pointer as the control map, or bail on null.
unsafe fn map_of<'a>(ui: *mut c_void) -> Option<&'a mut ControlMap> {
    if ui.is_null() {
        return None;
    }
    Some(unsafe { &mut *ui.cast::<ControlMap>() })
}

unsafe extern "C" fn open_box(ui: *mut c_void, label: *const c_char) {
    let Some(map) = (unsafe { map_of(ui) }) else {
        return;
    };
    let name = unsafe { label_of(label) };
    // C++ PathBuilder drops empty group labels rather than emitting `//`.
    if !name.is_empty() {
        map.groups.push(name);
    } else {
        // Push a marker so the matching close pops the right depth.
        map.groups.push(String::new());
    }
}

unsafe extern "C" fn close_box(ui: *mut c_void) {
    if let Some(map) = unsafe { map_of(ui) } {
        map.groups.pop();
    }
}

/// Build a control whose zone is non-null, or return `None`.
fn control(
    path: String,
    kind: ControlKind,
    zone: *mut FfiFaustFloat,
    init: f64,
    min: f64,
    max: f64,
    step: f64,
) -> Option<Control> {
    if zone.is_null() {
        return None;
    }
    Some(Control {
        path,
        kind,
        zone,
        init,
        min,
        max,
        step,
    })
}

unsafe extern "C" fn add_button(ui: *mut c_void, label: *const c_char, zone: *mut FfiFaustFloat) {
    let Some(map) = (unsafe { map_of(ui) }) else {
        return;
    };
    let name = unsafe { label_of(label) };
    let path = map.path_for(&name);
    if let Some(c) = control(path, ControlKind::Button, zone, 0.0, 0.0, 1.0, 1.0) {
        map.insert(&name, c);
    }
}

unsafe extern "C" fn add_check_button(
    ui: *mut c_void,
    label: *const c_char,
    zone: *mut FfiFaustFloat,
) {
    let Some(map) = (unsafe { map_of(ui) }) else {
        return;
    };
    let name = unsafe { label_of(label) };
    let path = map.path_for(&name);
    if let Some(c) = control(path, ControlKind::CheckButton, zone, 0.0, 0.0, 1.0, 1.0) {
        map.insert(&name, c);
    }
}

unsafe extern "C" fn add_slider(
    ui: *mut c_void,
    label: *const c_char,
    zone: *mut FfiFaustFloat,
    init: FfiFaustFloat,
    min: FfiFaustFloat,
    max: FfiFaustFloat,
    step: FfiFaustFloat,
) {
    let Some(map) = (unsafe { map_of(ui) }) else {
        return;
    };
    let name = unsafe { label_of(label) };
    let path = map.path_for(&name);
    if let Some(c) = control(
        path,
        ControlKind::Slider,
        zone,
        f64::from(init),
        f64::from(min),
        f64::from(max),
        f64::from(step),
    ) {
        map.insert(&name, c);
    }
}

unsafe extern "C" fn add_num_entry(
    ui: *mut c_void,
    label: *const c_char,
    zone: *mut FfiFaustFloat,
    init: FfiFaustFloat,
    min: FfiFaustFloat,
    max: FfiFaustFloat,
    step: FfiFaustFloat,
) {
    let Some(map) = (unsafe { map_of(ui) }) else {
        return;
    };
    let name = unsafe { label_of(label) };
    let path = map.path_for(&name);
    if let Some(c) = control(
        path,
        ControlKind::NumEntry,
        zone,
        f64::from(init),
        f64::from(min),
        f64::from(max),
        f64::from(step),
    ) {
        map.insert(&name, c);
    }
}

unsafe extern "C" fn add_soundfile(
    ui: *mut c_void,
    label: *const c_char,
    url: *const c_char,
    zone: *mut *mut c_void,
) {
    let Some(map) = (unsafe { map_of(ui) }) else {
        return;
    };
    if zone.is_null() {
        return;
    }
    let _ = unsafe { label_of(label) };
    let url = unsafe { label_of(url) };
    map.soundfiles
        .push(TestSoundfile::impulse_test_memory_reader(
            soundfile_part_count(&url),
        ));
    let fixture = map
        .soundfiles
        .last_mut()
        .expect("just pushed soundfile")
        .as_mut_ptr();
    // SAFETY: `zone` is the DSP's soundfile slot, and the fixture outlives the
    // instance because the map owns it.
    unsafe {
        *zone = fixture;
    }
}

unsafe extern "C" fn add_bargraph(
    ui: *mut c_void,
    label: *const c_char,
    zone: *mut FfiFaustFloat,
    min: FfiFaustFloat,
    max: FfiFaustFloat,
) {
    let Some(map) = (unsafe { map_of(ui) }) else {
        return;
    };
    let name = unsafe { label_of(label) };
    let path = map.path_for(&name);
    if let Some(c) = control(
        path,
        ControlKind::Bargraph,
        zone,
        0.0,
        f64::from(min),
        f64::from(max),
        0.0,
    ) {
        map.insert(&name, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(path: &str) -> Control {
        Control {
            path: path.to_owned(),
            kind: ControlKind::Slider,
            zone: std::ptr::null_mut(),
            init: 0.0,
            min: 0.0,
            max: 1.0,
            step: 0.01,
        }
    }

    fn map_of_paths(paths: &[&str]) -> ControlMap {
        let mut map = ControlMap::default();
        for p in paths {
            let c = probe(p);
            map.controls.insert(c.path.clone(), c);
        }
        map
    }

    #[test]
    fn resolves_exact_path() {
        let map = map_of_paths(&["/dsp/gain", "/dsp/freq"]);
        assert!(matches!(map.resolve("/dsp/gain"), Resolution::Unique(_)));
    }

    #[test]
    fn resolves_trailing_fragment() {
        let map = map_of_paths(&["/TIMBRE/filter_cutoff_hz", "/TIMBRE/osc0_volume"]);
        match map.resolve("filter_cutoff_hz") {
            Resolution::Unique(c) => assert_eq!(c.path, "/TIMBRE/filter_cutoff_hz"),
            other => panic!("expected unique, got {other:?}"),
        }
    }

    #[test]
    fn reports_ambiguity_instead_of_guessing() {
        let map = map_of_paths(&["/poly/voice0/gate", "/poly/voice1/gate"]);
        match map.resolve("gate") {
            Resolution::Ambiguous(candidates) => assert_eq!(candidates.len(), 2),
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn fragment_matches_whole_segments_only() {
        // `volume` must not resolve against `osc0_volume`: a fragment is a
        // path suffix at a segment boundary, not a substring.
        let map = map_of_paths(&["/TIMBRE/osc0_volume"]);
        assert!(matches!(map.resolve("volume"), Resolution::NotFound));
    }

    #[test]
    fn ending_with_selects_by_last_segment() {
        let map = map_of_paths(&["/dsp/gate", "/dsp/freq", "/dsp/gain"]);
        assert_eq!(map.ending_with("gate").len(), 1);
        assert_eq!(map.ending_with("nope").len(), 0);
    }

    #[test]
    fn path_flattens_slashes_inside_a_label() {
        // `hslider("osc0/volume", ...)` is one control named `osc0_volume`,
        // not a group `osc0` containing `volume`.
        let mut map = ControlMap::default();
        map.groups.push("TIMBRE".to_owned());
        assert_eq!(map.path_for("osc0/volume"), "/TIMBRE/osc0_volume");
    }

    #[test]
    fn path_sanitizes_osc_hostile_characters() {
        let mut map = ControlMap::default();
        map.groups.push("My Group".to_owned());
        assert_eq!(map.path_for("cut[off]"), "/My_Group/cut_off_");
    }

    #[test]
    fn path_without_groups_is_rooted() {
        let map = ControlMap::default();
        assert_eq!(map.path_for("gain"), "/gain");
    }

    #[test]
    fn clamp_respects_declared_range() {
        let c = probe("/dsp/gain");
        assert!((c.clamp(2.0) - 1.0).abs() < f64::EPSILON);
        assert!((c.clamp(-1.0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bargraphs_are_not_writable() {
        assert!(!ControlKind::Bargraph.is_writable());
        assert!(ControlKind::Slider.is_writable());
    }
}
