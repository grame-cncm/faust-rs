//! Integration tests for `ui` canonical builder/matcher APIs.

use tlib::TreeArena;
use ui::{
    ControlKind, ControlRange, ControlSpec, DuplicatePathKind, UiBuilder, UiGroupKind,
    UiGroupPathSegment, UiGroupSpec, UiMatch, UiProgram, UiProgramBuilder, UiRootOrigin,
    canonicalize_group_spec, find_duplicate_control_paths, match_ui,
    normalize_group_label_navigation, normalize_widget_label_path, split_label_metadata,
};

#[test]
fn builder_and_match_cover_grouped_ui_shapes() {
    let mut arena = TreeArena::new();
    let mut b = UiBuilder::new(&mut arena);

    let input = b.input_control(7);
    let output = b.output_control(8);
    let soundfile = b.soundfile(9);
    let vgroup = b.vgroup("root", &[input, output, soundfile]);
    let hgroup = b.hgroup("row", &[input]);
    let tgroup = b.tgroup("tabs", &[output]);

    assert_eq!(match_ui(&arena, input), UiMatch::InputControl(7));
    assert_eq!(match_ui(&arena, output), UiMatch::OutputControl(8));
    assert_eq!(match_ui(&arena, soundfile), UiMatch::Soundfile(9));
    assert_eq!(
        match_ui(&arena, vgroup),
        UiMatch::Group {
            kind: UiGroupKind::Vertical,
            label: "root",
            metadata: vec![],
            children: vec![input, output, soundfile],
        }
    );
    assert_eq!(
        match_ui(&arena, hgroup),
        UiMatch::Group {
            kind: UiGroupKind::Horizontal,
            label: "row",
            metadata: vec![],
            children: vec![input],
        }
    );
    assert_eq!(
        match_ui(&arena, tgroup),
        UiMatch::Group {
            kind: UiGroupKind::Tab,
            label: "tabs",
            metadata: vec![],
            children: vec![output],
        }
    );
}

#[test]
fn group_children_preserve_source_order() {
    let mut arena = TreeArena::new();
    let mut b = UiBuilder::new(&mut arena);

    let c0 = b.input_control(1);
    let c1 = b.input_control(2);
    let c2 = b.output_control(3);
    let root = b.hgroup("ordered", &[c0, c1, c2]);

    let UiMatch::Group { children, .. } = match_ui(&arena, root) else {
        panic!("group expected");
    };
    assert_eq!(children, vec![c0, c1, c2]);
}

#[test]
fn group_metadata_round_trips_through_builder_and_matcher() {
    let mut arena = TreeArena::new();
    let mut b = UiBuilder::new(&mut arena);

    let control = b.input_control(1);
    let root = b.group_with_metadata(
        UiGroupKind::Horizontal,
        "top",
        &[
            ("style".to_owned(), "tabbed".to_owned()),
            ("tooltip".to_owned(), "hello".to_owned()),
        ],
        &[control],
    );

    assert_eq!(
        match_ui(&arena, root),
        UiMatch::Group {
            kind: UiGroupKind::Horizontal,
            label: "top",
            metadata: vec![
                ("style".to_owned(), "tabbed".to_owned()),
                ("tooltip".to_owned(), "hello".to_owned()),
            ],
            children: vec![control],
        }
    );
}

#[test]
fn ui_program_keeps_root_and_control_registry() {
    let mut arena = TreeArena::new();
    let mut b = UiBuilder::new(&mut arena);
    let checkbox = b.input_control(0);
    let root = b.hgroup("top", &[checkbox]);

    let controls = vec![ControlSpec {
        id: 0,
        kind: ControlKind::Checkbox,
        label: "c".to_owned(),
        metadata: vec![("style".to_owned(), "knob".to_owned())],
        range: Some(ControlRange {
            init: 0.0,
            min: 0.0,
            max: 1.0,
            step: 1.0,
        }),
        source_node: None,
    }];
    let program = UiProgram {
        arena,
        root,
        controls,
        root_origin: UiRootOrigin::Explicit,
        emit_ui: true,
    };

    assert_eq!(program.controls.len(), 1);
    assert_eq!(program.controls[0].kind, ControlKind::Checkbox);
    assert_eq!(program.controls[0].label, "c");
    assert_eq!(
        match_ui(&program.arena, program.root),
        UiMatch::Group {
            kind: UiGroupKind::Horizontal,
            label: "top",
            metadata: vec![],
            children: vec![checkbox],
        }
    );
    assert_eq!(
        program.control(0).map(|control| control.label.as_str()),
        Some("c")
    );
}

#[test]
fn ui_program_empty_is_non_emitting_placeholder_with_empty_vertical_root() {
    let program = UiProgram::empty();

    assert!(program.is_empty());
    assert!(program.controls.is_empty());
    assert_eq!(program.root_origin, UiRootOrigin::Synthesized);
    assert_eq!(
        match_ui(&program.arena, program.root),
        UiMatch::Group {
            kind: UiGroupKind::Vertical,
            label: "",
            metadata: vec![],
            children: vec![],
        }
    );
}

#[test]
fn control_lookup_uses_stable_ids_across_input_output_and_soundfile_controls() {
    let mut arena = TreeArena::new();
    let mut b = UiBuilder::new(&mut arena);
    let checkbox = b.input_control(0);
    let meter = b.output_control(1);
    let soundfile = b.soundfile(2);
    let root = b.vgroup("root", &[checkbox, meter, soundfile]);

    let program = UiProgram {
        arena,
        root,
        controls: vec![
            ControlSpec {
                id: 0,
                kind: ControlKind::Checkbox,
                label: "gate".to_owned(),
                metadata: Vec::new(),
                range: None,
                source_node: None,
            },
            ControlSpec {
                id: 1,
                kind: ControlKind::HBargraph,
                label: "level".to_owned(),
                metadata: Vec::new(),
                range: Some(ControlRange {
                    init: 0.0,
                    min: -60.0,
                    max: 6.0,
                    step: 0.0,
                }),
                source_node: None,
            },
            ControlSpec {
                id: 2,
                kind: ControlKind::Soundfile,
                label: "sample".to_owned(),
                metadata: vec![("url".to_owned(), "{'tests/assets/silence.wav'}".to_owned())],
                range: None,
                source_node: None,
            },
        ],
        root_origin: UiRootOrigin::Explicit,
        emit_ui: true,
    };

    assert_eq!(
        program.control(0).map(|control| control.kind),
        Some(ControlKind::Checkbox)
    );
    assert_eq!(
        program.control(1).map(|control| control.kind),
        Some(ControlKind::HBargraph)
    );
    assert_eq!(
        program.control(2).map(|control| control.kind),
        Some(ControlKind::Soundfile)
    );
    assert!(program.control(3).is_none());
}

#[test]
fn split_label_metadata_matches_cpp_style_widget_metadata_extraction() {
    let (label, metadata) = split_label_metadata("gain [style:knob]");

    assert_eq!(label, "gain");
    assert_eq!(metadata, vec![("style".to_owned(), "knob".to_owned())]);
}

#[test]
fn split_label_metadata_handles_nested_brackets_and_escapes() {
    let (label, metadata) =
        split_label_metadata(r#"tab \[main\] [tooltip: use [fast\:slow]] [unit: dB]"#);

    assert_eq!(label, "tab [main]");
    assert_eq!(
        metadata,
        vec![
            ("tooltip".to_owned(), "use [fast:slow]".to_owned()),
            ("unit".to_owned(), "dB".to_owned()),
        ]
    );
}

#[test]
fn normalize_widget_label_path_rebases_relative_groups() {
    let current = vec![
        UiGroupPathSegment {
            kind: UiGroupKind::Horizontal,
            raw_label: "Foo".to_owned(),
        },
        UiGroupPathSegment {
            kind: UiGroupKind::Vertical,
            raw_label: "Faa".to_owned(),
        },
    ];

    let normalized = normalize_widget_label_path("../volume", &current);

    assert_eq!(
        normalized.groups,
        vec![UiGroupPathSegment {
            kind: UiGroupKind::Horizontal,
            raw_label: "Foo".to_owned(),
        }]
    );
    assert_eq!(normalized.raw_label, "volume");
}

#[test]
fn normalize_widget_label_path_preserves_typed_segments_and_metadata() {
    let normalized = normalize_widget_label_path("h:Oscillator/../v:Main/gain [style:knob]", &[]);

    assert_eq!(
        normalized.groups,
        vec![UiGroupPathSegment {
            kind: UiGroupKind::Vertical,
            raw_label: "Main".to_owned(),
        }]
    );
    assert_eq!(normalized.raw_label, "gain [style:knob]");
}

#[test]
fn normalize_group_label_navigation_rebases_and_clamps_root() {
    let current = vec![
        UiGroupPathSegment {
            kind: UiGroupKind::Horizontal,
            raw_label: "Foo".to_owned(),
        },
        UiGroupPathSegment {
            kind: UiGroupKind::Vertical,
            raw_label: "Bar".to_owned(),
        },
    ];

    let rebased = normalize_group_label_navigation("../Baz", &current, UiGroupKind::Tab);
    assert_eq!(
        rebased.parent_groups,
        vec![UiGroupPathSegment {
            kind: UiGroupKind::Horizontal,
            raw_label: "Foo".to_owned(),
        }]
    );
    assert_eq!(
        rebased.group,
        UiGroupPathSegment {
            kind: UiGroupKind::Tab,
            raw_label: "Baz".to_owned(),
        }
    );

    let clamped =
        normalize_group_label_navigation("../../../../Rooted", &current, UiGroupKind::Vertical);
    assert!(clamped.parent_groups.is_empty());
    assert_eq!(clamped.group.raw_label, "Rooted");
}

#[test]
fn canonicalize_group_spec_splits_metadata_after_path_normalization() {
    let spec = canonicalize_group_spec(&UiGroupPathSegment {
        kind: UiGroupKind::Horizontal,
        raw_label: "Main [style:tabbed]".to_owned(),
    });

    assert_eq!(
        spec,
        UiGroupSpec {
            kind: UiGroupKind::Horizontal,
            label: "Main".to_owned(),
            metadata: vec![("style".to_owned(), "tabbed".to_owned())],
        }
    );
}

#[test]
fn ui_program_builder_merges_group_paths_and_preserves_leaf_order() {
    let mut builder = UiProgramBuilder::new();
    let path = vec![
        UiGroupSpec {
            kind: UiGroupKind::Horizontal,
            label: "Foo".to_owned(),
            metadata: Vec::new(),
        },
        UiGroupSpec {
            kind: UiGroupKind::Vertical,
            label: "Bar".to_owned(),
            metadata: Vec::new(),
        },
    ];
    builder.insert_input_control(&path, 0, String::new());
    builder.insert_output_control(&path, 1, String::new());
    builder.insert_soundfile(&path, 2, String::new());

    let (arena, roots) = builder.finish();
    assert_eq!(roots.len(), 1);

    let UiMatch::Group {
        kind,
        label,
        children,
        ..
    } = match_ui(&arena, roots[0])
    else {
        panic!("root group expected");
    };
    assert_eq!(kind, UiGroupKind::Horizontal);
    assert_eq!(label, "Foo");
    assert_eq!(children.len(), 1);

    let UiMatch::Group {
        kind,
        label,
        children,
        ..
    } = match_ui(&arena, children[0])
    else {
        panic!("nested group expected");
    };
    assert_eq!(kind, UiGroupKind::Vertical);
    assert_eq!(label, "Bar");
    assert_eq!(children.len(), 3);
    assert_eq!(match_ui(&arena, children[0]), UiMatch::InputControl(0));
    assert_eq!(match_ui(&arena, children[1]), UiMatch::OutputControl(1));
    assert_eq!(match_ui(&arena, children[2]), UiMatch::Soundfile(2));
}

#[test]
fn ui_program_builder_sorts_root_siblings_by_cpp_order_key() {
    let mut builder = UiProgramBuilder::new();
    builder.insert_input_control(&[], 0, "gain".to_owned());
    builder.insert_input_control(&[], 1, "freq".to_owned());
    builder.insert_output_control(&[], 2, "freq".to_owned());

    let (arena, roots) = builder.finish();
    assert_eq!(roots.len(), 3);
    assert_eq!(match_ui(&arena, roots[0]), UiMatch::InputControl(1));
    assert_eq!(match_ui(&arena, roots[1]), UiMatch::OutputControl(2));
    assert_eq!(match_ui(&arena, roots[2]), UiMatch::InputControl(0));
}

#[test]
fn ui_program_builder_can_drop_empty_group_placeholders() {
    let mut builder = UiProgramBuilder::new();
    let foo = vec![UiGroupSpec {
        kind: UiGroupKind::Horizontal,
        label: "Foo".to_owned(),
        metadata: Vec::new(),
    }];
    let bar = vec![
        UiGroupSpec {
            kind: UiGroupKind::Horizontal,
            label: "Foo".to_owned(),
            metadata: Vec::new(),
        },
        UiGroupSpec {
            kind: UiGroupKind::Vertical,
            label: "Bar".to_owned(),
            metadata: Vec::new(),
        },
    ];

    let foo_id = builder
        .ensure_group_path(&foo)
        .expect("terminal Foo placeholder should exist");
    let bar_id = builder
        .ensure_group_path(&bar)
        .expect("terminal Bar placeholder should exist");
    builder.insert_input_control(&bar, 0, String::new());

    assert!(builder.group_has_children(foo_id));
    assert!(builder.group_has_children(bar_id));
    assert!(!builder.remove_group_if_empty(foo_id));
    assert!(!builder.remove_group_if_empty(bar_id));

    let empty = vec![UiGroupSpec {
        kind: UiGroupKind::Tab,
        label: "Empty".to_owned(),
        metadata: Vec::new(),
    }];
    let empty_id = builder
        .ensure_group_path(&empty)
        .expect("empty placeholder should exist");
    assert!(builder.remove_group_if_empty(empty_id));
    assert!(builder.find_group_path(&empty).is_none());
}

/// Builds a one-group program from `(kind, label)` pairs for path-conflict tests.
fn program_with_controls(specs: &[(ControlKind, &str)]) -> UiProgram {
    let mut arena = TreeArena::new();
    let controls = specs
        .iter()
        .enumerate()
        .map(|(index, (kind, label))| {
            ControlSpec::synthetic(
                u32::try_from(index).expect("test control index fits in u32"),
                *kind,
                (*label).to_owned(),
                Vec::new(),
                None,
            )
        })
        .collect::<Vec<_>>();
    let root = {
        let mut b = UiBuilder::new(&mut arena);
        let nodes = specs
            .iter()
            .enumerate()
            .map(|(index, (kind, _))| {
                let id = u32::try_from(index).expect("test control index fits in u32");
                match kind {
                    ControlKind::VBargraph | ControlKind::HBargraph => b.output_control(id),
                    ControlKind::Soundfile => b.soundfile(id),
                    _ => b.input_control(id),
                }
            })
            .collect::<Vec<_>>();
        b.vgroup("dsp", &nodes)
    };
    UiProgram {
        arena,
        root,
        controls,
        root_origin: UiRootOrigin::Synthesized,
        emit_ui: true,
    }
}

#[test]
fn two_input_controls_sharing_one_address_conflict() {
    let program = program_with_controls(&[
        (ControlKind::HSlider, "gain"),
        (ControlKind::VSlider, "gain"),
    ]);
    let conflicts = find_duplicate_control_paths(&program);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].address, "/dsp/gain");
    assert_eq!(conflicts[0].controls, vec![0, 1]);
    assert_eq!(conflicts[0].kind, DuplicatePathKind::InputConflict);
}

#[test]
fn distinct_labels_do_not_conflict() {
    let program = program_with_controls(&[
        (ControlKind::HSlider, "gain"),
        (ControlKind::VSlider, "pan"),
    ]);
    assert!(find_duplicate_control_paths(&program).is_empty());
}

#[test]
fn bargraphs_sharing_one_address_are_only_ambiguous() {
    let program = program_with_controls(&[
        (ControlKind::VBargraph, "level"),
        (ControlKind::HBargraph, "level"),
    ]);
    let conflicts = find_duplicate_control_paths(&program);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].kind, DuplicatePathKind::BargraphOnly);
}

#[test]
fn a_bargraph_and_an_input_control_conflict() {
    let program = program_with_controls(&[
        (ControlKind::VBargraph, "level"),
        (ControlKind::HSlider, "level"),
    ]);
    let conflicts = find_duplicate_control_paths(&program);
    assert_eq!(conflicts[0].kind, DuplicatePathKind::InputConflict);
}

#[test]
fn a_soundfile_shares_the_input_namespace() {
    let program = program_with_controls(&[
        (ControlKind::Soundfile, "sample"),
        (ControlKind::HSlider, "sample"),
    ]);
    let conflicts = find_duplicate_control_paths(&program);
    assert_eq!(conflicts[0].kind, DuplicatePathKind::InputConflict);
}

#[test]
fn anonymous_controls_are_excluded_until_cpp_naming_is_ported() {
    let program = program_with_controls(&[(ControlKind::HSlider, ""), (ControlKind::VSlider, "")]);
    assert!(
        find_duplicate_control_paths(&program).is_empty(),
        "C++ renames unlabeled widgets before they can collide"
    );
}

#[test]
fn a_ui_free_program_reports_no_conflict() {
    assert!(find_duplicate_control_paths(&UiProgram::empty()).is_empty());
}
