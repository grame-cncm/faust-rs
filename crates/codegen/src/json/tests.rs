use super::{
    JsonBuildOptions, JsonDescription, JsonMemoryDescription, build_json_description_from_fir,
};
use crate::fixtures::{
    build_gain_bias_ui_meta_test_module, build_passthrough_test_module,
    build_table_state_delay_test_module,
};
use crate::memory_layout::{Mem0AnalysisOptions, MemoryLayoutFlavor, analyze_effective_mem0};

use fir::{FirMatch, match_fir};

#[test]
fn json_description_renders_minimal_shape() {
    let json = JsonDescription {
        name: "passthrough".to_owned(),
        backend: None,
        jit_compiled: None,
        compute_body_lowered: None,
        filename: None,
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: Some(4),
        inputs: 1,
        outputs: 2,
        sr_index: None,
        memory: None,
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
        backend: None,
        jit_compiled: None,
        compute_body_lowered: None,
        filename: None,
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: Some(0),
        inputs: 0,
        outputs: 0,
        sr_index: None,
        memory: None,
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
            backend: None,
            jit_compiled: None,
            compute_body_lowered: None,
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
            memory: None,
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
            backend: None,
            jit_compiled: None,
            compute_body_lowered: None,
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
            memory: None,
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
            backend: None,
            jit_compiled: None,
            compute_body_lowered: None,
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
            memory: None,
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
        backend: None,
        jit_compiled: None,
        compute_body_lowered: None,
        filename: None,
        version: None,
        compile_options: None,
        library_list: Vec::new(),
        include_pathnames: Vec::new(),
        size: None,
        inputs: 0,
        outputs: 1,
        sr_index: None,
        memory: None,
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

#[test]
fn mem0_json_is_valid_versioned_and_cost_stable_across_native_backends() {
    let (store, module) = build_table_state_delay_test_module();
    let FirMatch::Module {
        name,
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

    let mut common_cost = None;
    for (flavor, backend, manager_abi) in [
        (MemoryLayoutFlavor::C, "c", "faust_memory_manager_v1"),
        (MemoryLayoutFlavor::Cpp, "cpp", "dsp_memory_manager_v1"),
        (
            MemoryLayoutFlavor::Cranelift,
            "cranelift",
            "faust_memory_manager_v1",
        ),
    ] {
        let analysis =
            analyze_effective_mem0(&store, module, &Mem0AnalysisOptions::native(flavor, false))
                .expect("fixture should have a complete mem0 analysis");
        match &common_cost {
            Some(expected) => assert_eq!(&analysis.compute_cost, expected),
            None => common_cost = Some(analysis.compute_cost.clone()),
        }
        let description = build_json_description_from_fir(
            &store,
            &function_items,
            JsonBuildOptions {
                name: name.clone(),
                backend: None,
                jit_compiled: None,
                compute_body_lowered: None,
                filename: None,
                version: None,
                compile_options: Some(format!("-lang {backend} -mem0 -single")),
                library_list: Vec::new(),
                include_pathnames: Vec::new(),
                top_level_meta: Vec::new(),
                size: None,
                inputs: num_inputs,
                outputs: num_outputs,
                sr_index: None,
                memory: Some(JsonMemoryDescription {
                    backend: backend.to_owned(),
                    manager_abi: manager_abi.to_owned(),
                    analysis,
                }),
            },
            |_var| None,
        )
        .expect("mem0 JSON should build");
        let rendered = description.render();
        assert_eq!(
            rendered,
            description.render(),
            "rendering must be deterministic"
        );
        let value: serde_json::Value =
            serde_json::from_str(&rendered).expect("mem0 output must be valid JSON");

        assert_eq!(value["memory_layout_version"], 2);
        assert_eq!(value["memory_manager"]["mode"], "mem0");
        assert_eq!(value["memory_manager"]["backend"], backend);
        assert_eq!(value["memory_manager"]["manager_abi"], manager_abi);
        assert_eq!(
            value["memory_manager"]["access_metric"],
            "static_accesses_per_scalar_frame"
        );
        let zones = value["memory_layout"]
            .as_array()
            .expect("memory_layout array");
        assert!(!zones.is_empty());
        assert!(zones.iter().all(|zone| zone.get("scope").is_some()));
        assert!(zones.iter().all(|zone| zone.get("role").is_some()));
        assert!(zones.iter().all(|zone| zone.get("alignment").is_some()));
        assert!(zones.iter().all(|zone| zone.get("size_exact").is_some()));
        assert_eq!(value["compute_cost_version"], 2);
        assert_eq!(value["compute_cost_metric"], "static_scalar_fir_structure");
        let costs = value["compute_cost"]
            .as_array()
            .expect("compute_cost array");
        assert_eq!(costs.len(), 1);
        for key in ["binop", "mathop"] {
            let breakdown = costs[0][key][0]
                .as_object()
                .expect("one cost breakdown object");
            let total = breakdown["total"].as_u64().expect("numeric total");
            let sum: u64 = breakdown
                .iter()
                .filter(|(name, _)| name.as_str() != "total")
                .map(|(_, value)| value.as_u64().expect("numeric operation count"))
                .sum();
            assert_eq!(total, sum, "{key} total must equal named entries");
        }
    }
}
