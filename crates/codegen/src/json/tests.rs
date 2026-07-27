use super::{JsonBuildOptions, JsonDescription, build_json_description_from_fir};
use crate::fixtures::{build_gain_bias_ui_meta_test_module, build_passthrough_test_module};

use fir::{FirMatch, match_fir};

#[test]
fn json_description_renders_minimal_shape() {
    let json = JsonDescription {
        name: "passthrough".to_owned(),
        filename: None,
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: Some(4),
        inputs: 1,
        outputs: 2,
        sr_index: None,
        meta: Vec::new(),
        ui: Vec::new(),
    }
    .render();

    assert_eq!(
        json,
        "{\n\t\"name\": \"passthrough\",\n\t\"size\": 4,\n\t\"inputs\": 1,\n\t\"outputs\": 2,\n\t\"meta\": [],\n\t\"ui\": []\n}"
    );
    assert!(json.contains("\n\t\"ui\": []\n}"));
}

#[test]
fn json_description_escapes_strings() {
    let json = JsonDescription {
        name: "quote\"slash\\tab\tline\n".to_owned(),
        filename: None,
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: Some(0),
        inputs: 0,
        outputs: 0,
        sr_index: None,
        meta: Vec::new(),
        ui: Vec::new(),
    }
    .render();

    assert!(json.contains("\"name\": \"quote\\\"slash\\\\tab\\tline\\n\""));
}

#[test]
fn json_builder_replays_fir_ui_and_metadata() {
    let (store, module) = build_gain_bias_ui_meta_test_module();
    let FirMatch::Module {
        functions,
        num_inputs,
        num_outputs,
        ..
    } = match_fir(&store, module)
    else {
        panic!("module root expected");
    };
    let FirMatch::Block(function_items) = match_fir(&store, functions) else {
        panic!("function block expected");
    };

    let json = build_json_description_from_fir(
        &store,
        &function_items,
        JsonBuildOptions {
            name: "gain_bias_ui_meta".to_owned(),
            filename: None,
            version: None,
            compile_options: None,
            library_list: Vec::new(),
            include_pathnames: Vec::new(),
            top_level_meta: Vec::new(),
            size: Some(16),
            inputs: num_inputs,
            outputs: num_outputs,
            sr_index: Some(0),
        },
        |var| match var {
            "fGain" => Some(0),
            "fBias" => Some(4),
            "fGate" => Some(8),
            "fLevel" => Some(12),
            _ => None,
        },
    )
    .expect("FIR JSON builder should succeed");

    let rendered = json.render();
    assert!(rendered.contains("\t\"meta\": [ \n\t\t{ \"name\": \"gain-bias-ui-meta\" },"));
    assert!(rendered.contains("\"address\": \"/GainBias/gain\""));
    assert!(rendered.contains("\"index\": 0"));
    assert!(rendered.contains("\"address\": \"/GainBias/level\""));
    assert!(rendered.contains("\"index\": 12"));
    assert!(rendered.contains("\"name\": \"gain-bias-ui-meta\""));
}

#[test]
fn json_builder_emits_empty_ui_when_build_ui_function_is_missing() {
    let (store, module) = build_passthrough_test_module();
    let FirMatch::Module {
        functions,
        num_inputs,
        num_outputs,
        ..
    } = match_fir(&store, module)
    else {
        panic!("module root expected");
    };
    let FirMatch::Block(function_items) = match_fir(&store, functions) else {
        panic!("function block expected");
    };

    let json = build_json_description_from_fir(
        &store,
        &function_items,
        JsonBuildOptions {
            name: "passthrough".to_owned(),
            filename: None,
            version: None,
            compile_options: None,
            library_list: Vec::new(),
            include_pathnames: Vec::new(),
            top_level_meta: Vec::new(),
            size: Some(0),
            inputs: num_inputs,
            outputs: num_outputs,
            sr_index: None,
        },
        |_var| None,
    )
    .expect("builder should tolerate missing buildUserInterface");

    assert_eq!(json.ui, Vec::new());
}

#[test]
fn json_builder_omits_widget_index_when_no_offset_resolver_is_available() {
    let (store, module) = build_gain_bias_ui_meta_test_module();
    let FirMatch::Module {
        functions,
        num_inputs,
        num_outputs,
        ..
    } = match_fir(&store, module)
    else {
        panic!("module root expected");
    };
    let FirMatch::Block(function_items) = match_fir(&store, functions) else {
        panic!("function block expected");
    };

    let json = build_json_description_from_fir(
        &store,
        &function_items,
        JsonBuildOptions {
            name: "gain_bias_ui_meta".to_owned(),
            filename: None,
            version: None,
            compile_options: None,
            library_list: Vec::new(),
            include_pathnames: Vec::new(),
            top_level_meta: Vec::new(),
            size: Some(16),
            inputs: num_inputs,
            outputs: num_outputs,
            sr_index: None,
        },
        |_var| None,
    )
    .expect("builder should allow JSON without widget offsets");

    let rendered = json.render();
    assert!(rendered.contains("\"address\": \"/GainBias/gain\""));
    assert!(!rendered.contains("\"index\":"));
}

#[test]
fn json_description_canonicalizes_soundfile_urls_for_faustwasm() {
    let json = JsonDescription {
        name: "soundfile".to_owned(),
        filename: None,
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: None,
        inputs: 0,
        outputs: 1,
        sr_index: None,
        meta: Vec::new(),
        ui: vec![super::JsonUiItem::Widget(super::JsonWidget {
            typ: "soundfile",
            label: "Drone_1".to_owned(),
            varname: "fSound0".to_owned(),
            shortname: "Drone_1".to_owned(),
            address: "/DroneLAN/Drone_1".to_owned(),
            index: Some(4),
            meta: Vec::new(),
            range: None,
            soundfile_url: Some(
                "{'Alonepad_reverb_stereo_instru1.flac'; 'Dronepad_test_stereo_instru1.flac'}"
                    .to_owned(),
            ),
        })],
    }
    .render();

    assert!(json.contains(
        "\"url\": \"{-Alonepad_reverb_stereo_instru1.flac-;-Dronepad_test_stereo_instru1.flac-}\""
    ));
    assert!(!json.contains("'Dronepad_test_stereo_instru1.flac"));
}
