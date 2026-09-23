//! Widget modulation against the reference compiler.
//!
//! The 42 `tests/corpus/modulation_*.dsp` fixtures cover the rules of the
//! manual's "Widget Modulation" section and its edge cases: the default `*`
//! modulator, several targets and the order of their inputs, modulators with
//! zero, one and two inputs and where their own widgets land, group paths,
//! labels with metadata (leading, trailing, multi-line), labels that open a
//! group (`h:sub/x`), bargraphs, buttons, nested and parallel modulations,
//! interpolated targets, a recursion, and targets that match nothing.
//!
//! Three checks:
//! - the arity and the interface (paths in UI order, kinds, init/min/max/step)
//!   of every fixture equal a table frozen from the reference compiler 2.88.1
//!   (`faust -json`, 2026-09-23; the samples of the impulse-test protocol
//!   were equal to the bit the same day);
//! - when a reference `faust` binary is available (`FAUST_CPP_BIN`, else
//!   `/usr/local/bin/faust`), the same triple is compared live with its
//!   `-json` output, so a drift of either compiler is seen;
//! - the three `err_2{7,8,9}_modulation_*.dsp` fixtures are refused, and the
//!   no-match fixture carries `FRS-EVAL-0008` under the semantic warnings.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use compiler::{Compiler, SignalFirLane};
use serde_json::Value;

/// One control as both compilers describe it, the numbers as `f64` whatever
/// the JSON spelling (the reference writes them as strings).
#[derive(Debug, PartialEq)]
struct Control {
    address: &'static str,
    kind: &'static str,
    init: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
}

/// One control as read from a JSON description: address, kind, init, min,
/// max, step.
type ControlRow = (
    String,
    String,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

#[derive(Debug, PartialEq)]
struct Observed {
    inputs: u64,
    outputs: u64,
    controls: Vec<ControlRow>,
}

struct Case {
    fixture: &'static str,
    inputs: u64,
    outputs: u64,
    controls: &'static [Control],
    /// `Some(reason)` when faust-rs's interface is known to differ from the
    /// reference's for a cause outside modulation; the live differential then
    /// compares the arity only.
    ui_differs_from_cpp: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        fixture: "modulation_01_default_mul.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_02_two_targets_default.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_03_explicit_add.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_04_mixed_default_and_add.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_05_zero_const.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[Control {
            address: "/g/b",
            kind: "hslider",
            init: Some(0.1),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_06_zero_with_ui.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/z",
                kind: "hslider",
                init: Some(0.3),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_07_one_input_const.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_08_one_input_ui.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/depth",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_09_one_input_lfo.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_10_two_input_lambda.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_11_two_input_drop.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: Some(
            "the widget dropped by `(!, _)` reaches nothing; faust-rs keeps such a widget in the interface where the reference drops it, a difference of the UI builder, not of the modulation",
        ),
    },
    Case {
        fixture: "modulation_12_two_input_ui.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/k",
                kind: "hslider",
                init: Some(1.0),
                min: Some(0.0),
                max: Some(2.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_13_path_typed_no_match.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/top/in/x",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/top/out/x",
                kind: "hslider",
                init: Some(0.25),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_14_path_group.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/top/in/x",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/top/out/x",
                kind: "hslider",
                init: Some(0.25),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_15_path_wrong_type_no_match.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/top/in/x",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/top/out/x",
                kind: "hslider",
                init: Some(0.25),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_16_path_partial.dsp",
        inputs: 6,
        outputs: 3,
        controls: &[Control {
            address: "/a/b/c/x",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_17_same_label_two_groups.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/top/in/x",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/top/out/x",
                kind: "hslider",
                init: Some(0.25),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_18_widget_used_twice.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_18_widget_used_twice/a",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_19_metadata_in_widget.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_19_metadata_in_widget/a",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_20_metadata_in_target.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_20_metadata_in_target/a",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_21_leading_metadata.dsp",
        inputs: 3,
        outputs: 2,
        controls: &[Control {
            address: "/Freeverb/Wet",
            kind: "vslider",
            init: Some(0.3333),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.025),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_22_manual_example.dsp",
        inputs: 2,
        outputs: 2,
        controls: &[Control {
            address: "/Freeverb/Wet",
            kind: "vslider",
            init: Some(0.3333),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.025),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_23_bargraph_target.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_23_bargraph_target/lev",
            kind: "hbargraph",
            init: None,
            min: Some(0.0),
            max: Some(1.0),
            step: None,
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_24_bargraph_two_input.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_24_bargraph_two_input/lev",
            kind: "hbargraph",
            init: None,
            min: Some(0.0),
            max: Some(1.0),
            step: None,
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_25_button_checkbox.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/modulation_25_button_checkbox/go",
                kind: "button",
                init: None,
                min: None,
                max: None,
                step: None,
            },
            Control {
                address: "/modulation_25_button_checkbox/on",
                kind: "checkbox",
                init: None,
                min: None,
                max: None,
                step: None,
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_26_nested.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_27_same_target_twice.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_28_interpolated_label.dsp",
        inputs: 4,
        outputs: 2,
        controls: &[
            Control {
                address: "/modulation_28_interpolated_label/a0",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/modulation_28_interpolated_label/a1",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_29_function_body.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_29_function_body/a",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_30_widget_label_with_path.dsp",
        inputs: 6,
        outputs: 3,
        controls: &[Control {
            address: "/g/sub/x",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_31_kinds.dsp",
        inputs: 3,
        outputs: 1,
        controls: &[
            Control {
                address: "/modulation_31_kinds/n",
                kind: "nentry",
                init: Some(2.0),
                min: Some(0.0),
                max: Some(4.0),
                step: Some(1.0),
            },
            Control {
                address: "/modulation_31_kinds/v",
                kind: "vslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_32_group_as_target.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_33_parallel_twice.dsp",
        inputs: 4,
        outputs: 2,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_34_one_input_identity.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_35_in_recursion.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_35_in_recursion/a",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_36_no_match.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_37_no_match_one_input.dsp",
        inputs: 1,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_38_smoothed_target.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_38_smoothed_target/a",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_39_target_in_with.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[Control {
            address: "/modulation_39_target_in_with/cut",
            kind: "hslider",
            init: Some(0.9),
            min: Some(0.0),
            max: Some(0.999),
            step: Some(0.001),
        }],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_40_tgroup_path.dsp",
        inputs: 3,
        outputs: 2,
        controls: &[
            Control {
                address: "/tabs/one/g",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/tabs/two/g",
                kind: "hslider",
                init: Some(0.25),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_41_two_input_stateful.dsp",
        inputs: 2,
        outputs: 1,
        controls: &[
            Control {
                address: "/g/a",
                kind: "hslider",
                init: Some(0.5),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            Control {
                address: "/g/b",
                kind: "hslider",
                init: Some(0.1),
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
        ],
        ui_differs_from_cpp: None,
    },
    Case {
        fixture: "modulation_42_double_group_prefix.dsp",
        inputs: 4,
        outputs: 2,
        controls: &[Control {
            address: "/a/a/x",
            kind: "hslider",
            init: Some(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
        }],
        ui_differs_from_cpp: None,
    },
];

const ERROR_FIXTURES: &[&str] = &[
    "err_27_modulation_three_inputs.dsp",
    "err_28_modulation_two_outputs.dsp",
    "err_29_modulation_constant_on_bargraph.dsp",
];

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("corpus")
        .join(file)
}

fn cpp_bin() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FAUST_CPP_BIN") {
        return Some(PathBuf::from(path));
    }
    let default = PathBuf::from("/usr/local/bin/faust");
    default.exists().then_some(default)
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn flatten_ui(items: &[Value], out: &mut Vec<ControlRow>) {
    for item in items {
        let kind = item["type"].as_str().unwrap_or_default();
        if matches!(kind, "vgroup" | "hgroup" | "tgroup") {
            if let Some(children) = item["items"].as_array() {
                flatten_ui(children, out);
            }
        } else {
            out.push((
                item["address"].as_str().unwrap_or_default().to_owned(),
                kind.to_owned(),
                number(item.get("init")),
                number(item.get("min")),
                number(item.get("max")),
                number(item.get("step")),
            ));
        }
    }
}

fn observe(json: &str) -> Observed {
    let value: Value = serde_json::from_str(json).expect("a JSON description");
    let mut controls = Vec::new();
    if let Some(items) = value["ui"].as_array() {
        flatten_ui(items, &mut controls);
    }
    Observed {
        inputs: value["inputs"].as_u64().expect("inputs"),
        outputs: value["outputs"].as_u64().expect("outputs"),
        controls,
    }
}

fn expected(case: &Case) -> Observed {
    Observed {
        inputs: case.inputs,
        outputs: case.outputs,
        controls: case
            .controls
            .iter()
            .map(|c| {
                (
                    c.address.to_owned(),
                    c.kind.to_owned(),
                    c.init,
                    c.min,
                    c.max,
                    c.step,
                )
            })
            .collect(),
    }
}

fn rust_json(compiler: &Compiler, fixture: &str) -> String {
    compiler
        .compile_file_to_json(&corpus_path(fixture), &[], SignalFirLane::TransformFastLane)
        .unwrap_or_else(|e| panic!("{fixture}: faust-rs failed: {e}"))
}

fn cpp_json(cpp_bin: &Path, fixture: &str) -> Result<String, String> {
    let out_dir = std::env::temp_dir().join(format!(
        "faust-rs-modulation-json-{}-{}",
        std::process::id(),
        fixture.trim_end_matches(".dsp")
    ));
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let output = Command::new(cpp_bin)
        .arg("-json")
        .arg(corpus_path(fixture))
        .arg("-O")
        .arg(&out_dir)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", cpp_bin.display()))?;
    let json_path = out_dir.join(format!("{fixture}.json"));
    let text = fs::read_to_string(&json_path);
    let _ = fs::remove_dir_all(&out_dir);
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    text.map_err(|e| format!("cannot read {}: {e}", json_path.display()))
}

#[test]
fn every_modulation_fixture_matches_the_frozen_reference_arity_and_interface() {
    let compiler = Compiler::new();
    let mut failures = Vec::new();
    for case in CASES {
        let observed = observe(&rust_json(&compiler, case.fixture));
        let wanted = expected(case);
        if observed != wanted {
            failures.push(format!(
                "{}\n  expected {wanted:?}\n  observed {observed:?}",
                case.fixture
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture(s) differ from the frozen reference:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_modulation_fixture_matches_the_live_reference_compiler() {
    let Some(cpp_bin) = cpp_bin() else {
        eprintln!(
            "Skipping live modulation differential: FAUST_CPP_BIN not set and /usr/local/bin/faust not found"
        );
        return;
    };
    let compiler = Compiler::new();
    let mut failures = Vec::new();
    for case in CASES {
        let rust = observe(&rust_json(&compiler, case.fixture));
        let cpp = match cpp_json(&cpp_bin, case.fixture) {
            Ok(json) => observe(&json),
            Err(e) => {
                failures.push(format!("{}: reference compiler failed: {e}", case.fixture));
                continue;
            }
        };
        let same = match case.ui_differs_from_cpp {
            Some(_) => rust.inputs == cpp.inputs && rust.outputs == cpp.outputs,
            None => rust == cpp,
        };
        if !same {
            failures.push(format!(
                "{}\n  reference {cpp:?}\n  faust-rs  {rust:?}",
                case.fixture
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture(s) differ from the reference compiler:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn invalid_modulators_are_refused_as_in_the_reference() {
    let compiler = Compiler::new();
    for fixture in ERROR_FIXTURES {
        let result = compiler.compile_file_default_to_signals(&corpus_path(fixture));
        assert!(result.is_err(), "{fixture}: faust-rs should refuse it");
        if let Some(cpp_bin) = cpp_bin() {
            let output = Command::new(&cpp_bin)
                .arg(corpus_path(fixture))
                .arg("-lang")
                .arg("cpp")
                .output()
                .expect("run the reference compiler");
            assert!(
                !output.status.success(),
                "{fixture}: the reference compiler should refuse it too"
            );
        }
    }
}

#[test]
fn a_target_without_match_warns_under_the_semantic_warnings_only() {
    let fixture = corpus_path("modulation_36_no_match.dsp");
    let quiet = Compiler::new()
        .compile_file_default_to_signals(&fixture)
        .expect("the program compiles, with a dangling input");
    assert!(
        quiet.warnings.is_empty(),
        "no warning without the option: {:?}",
        quiet.warnings
    );
    let verbose = Compiler::new()
        .with_semantic_warnings(true)
        .compile_file_default_to_signals(&fixture)
        .expect("the program compiles, with a dangling input");
    let codes: Vec<&str> = verbose
        .warnings
        .as_slice()
        .iter()
        .map(|d| d.code.0)
        .collect();
    assert_eq!(codes, vec!["FRS-EVAL-0008"], "{:?}", verbose.warnings);
    let message = verbose.warnings.as_slice()[0].message.to_string();
    assert!(message.contains("`zzz`"), "{message}");
}
