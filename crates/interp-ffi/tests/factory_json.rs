//! `getCInterpreterDSPFactoryJSON` returns what the C++
//! `interpreter_dsp_factory::getJSON` returns: the flat `JSONUI` description.
//! The expected strings are the output of the C++ libfaust 2.89.2 for the
//! same programs, with two differences that come from the compiler, not from
//! this function: faust-rs does not yet put the program's metadata in the FIR
//! (`declare`s, and the `filename` and `name` entries the compiler adds), so
//! the top-level `meta` is empty and `filename` too. Once it does, the
//! expected strings become the C++ output verbatim.

use std::ffi::{CStr, CString, c_char};

use faust_interp::factory::{
    createCInterpreterDSPFactoryFromString, deleteCInterpreterDSPFactory, freeCMemory,
    getCInterpreterDSPFactoryJSON,
};

/// The JSON of `code` compiled as `name` with `args`.
fn factory_json(name: &str, code: &str, args: &[&str]) -> String {
    let c_name = CString::new(name).unwrap();
    let c_code = CString::new(code).unwrap();
    let owned: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
    let argv: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
    let mut error = [0 as c_char; 4096];
    unsafe {
        let factory = createCInterpreterDSPFactoryFromString(
            c_name.as_ptr(),
            c_code.as_ptr(),
            argv.len() as i32,
            argv.as_ptr(),
            error.as_mut_ptr(),
        );
        assert!(
            !factory.is_null(),
            "{}",
            CStr::from_ptr(error.as_ptr()).to_string_lossy()
        );
        let json = getCInterpreterDSPFactoryJSON(factory);
        let text = CStr::from_ptr(json).to_string_lossy().into_owned();
        freeCMemory(json.cast());
        deleteCInterpreterDSPFactory(factory);
        text
    }
}

#[test]
fn every_widget_type_with_its_range_and_metadata() {
    let code = r#"process = hslider("x", 0.1, 0, 1, 0.01) + vslider("v [unit:dB]", -6, -70, 6, 0.1) + nentry("n", 3, 1, 10, 1) + button("b") + checkbox("c [tooltip:on or off]") : hbargraph("hb", 0, 2) : vbargraph("vb", -1, 1);"#;
    let expected = r#"{"name": "widgets","filename": "","inputs": 0,"outputs": 1,"meta": [],"ui": [ {"type": "vgroup","label": "widgets","items": [ {"type": "button","label": "b","shortname": "b","address": "/widgets/b"},{"type": "checkbox","label": "c","shortname": "c","address": "/widgets/c","meta": [{ "tooltip": "on or off" }]},{"type": "hbargraph","label": "hb","shortname": "hb","address": "/widgets/hb","min": 0,"max": 2},{"type": "nentry","label": "n","shortname": "n","address": "/widgets/n","init": 3,"min": 1,"max": 10,"step": 1},{"type": "vslider","label": "v","shortname": "v","address": "/widgets/v","meta": [{ "unit": "dB" }],"init": -6,"min": -70,"max": 6,"step": 0.1},{"type": "vbargraph","label": "vb","shortname": "vb","address": "/widgets/vb","min": -1,"max": 1},{"type": "hslider","label": "x","shortname": "x","address": "/widgets/x","init": 0.1,"min": 0,"max": 1,"step": 0.01}]}]}"#;
    assert_eq!(factory_json("widgets", code, &[]), expected);
}

#[test]
fn nested_groups_their_metadata_and_disambiguated_short_names_in_double() {
    let code = r#"process = hgroup("H [style:foo]", hslider("a", 0.5, 0, 1, 0.01), vgroup("V", nentry("a", 1, 0, 2, 1), tgroup("T", button("go"), checkbox("on")))) :> _;"#;
    let expected = r#"{"name": "groups","filename": "","inputs": 0,"outputs": 1,"meta": [],"ui": [ {"type": "hgroup","label": "H","meta": [{ "style": "foo" }],"items": [ {"type": "vgroup","label": "V","items": [ {"type": "tgroup","label": "T","items": [ {"type": "button","label": "go","shortname": "go","address": "/H/V/T/go"},{"type": "checkbox","label": "on","shortname": "on","address": "/H/V/T/on"}]},{"type": "nentry","label": "a","shortname": "V_a","address": "/H/V/a","init": 1,"min": 0,"max": 2,"step": 1}]},{"type": "hslider","label": "a","shortname": "H_a","address": "/H/a","init": 0.5,"min": 0,"max": 1,"step": 0.01}]}]}"#;
    assert_eq!(factory_json("groups", code, &["-double"]), expected);
}

#[test]
fn labels_awkward_in_an_address_ordering_prefixes_and_arities() {
    let code = r#"process = _, _ : *(hslider("my gain (dB)", 0.5, 0, 1, 0.1)), *(nentry("a/b", 0.25, 0, 1, 0.05)) : +(hslider("[2]first", 0, 0, 1, 0.1)), +(hslider("[1]second", 0.3, 0, 1, 0.1));"#;
    let expected = r#"{"name": "labels","filename": "","inputs": 2,"outputs": 2,"meta": [],"ui": [ {"type": "vgroup","label": "labels","items": [ {"type": "hslider","label": "second","shortname": "second","address": "/labels/second","meta": [{ "1": "" }],"init": 0.3,"min": 0,"max": 1,"step": 0.1},{"type": "hslider","label": "first","shortname": "first","address": "/labels/first","meta": [{ "2": "" }],"init": 0,"min": 0,"max": 1,"step": 0.1},{"type": "nentry","label": "a/b","shortname": "a_b","address": "/labels/a_b","init": 0.25,"min": 0,"max": 1,"step": 0.05},{"type": "hslider","label": "my gain (dB)","shortname": "my_gain_dB","address": "/labels/my_gain__dB_","init": 0.5,"min": 0,"max": 1,"step": 0.1}]}]}"#;
    assert_eq!(factory_json("labels", code, &[]), expected);
}
