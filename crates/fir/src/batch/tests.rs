//! Tests of the voice layout producer (design
//! `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`, G3.2) on
//! a hand-built module with every kind of state: scalars, recursion buffers of
//! both element types, a delay line with its counter, a control, a table
//! generator's working state used by `staticInit` only, and a `Static` table.

use super::{
    BatchError, CONTROLS_TABLE, INSTANCE_FUNCTIONS, VOICE_ARG, VOICES_FIELD, layout_voices,
};
use crate::{
    AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, NamedType, SliderRange,
    SliderType, canonical_fir_fingerprint, match_fir,
};

const FF: FirType = FirType::FaustFloat;
const I32: FirType = FirType::Int32;

fn arr(t: FirType, n: usize) -> FirType {
    FirType::Array(Box::new(t), n)
}

fn dsp_arg() -> NamedType {
    NamedType {
        name: "dsp".to_owned(),
        typ: FirType::Ptr(Box::new(FirType::Obj)),
    }
}

fn fun(b: &mut FirBuilder<'_>, name: &str, extra: &[NamedType], body: FirId) -> FirId {
    let mut args = vec![dsp_arg()];
    args.extend_from_slice(extra);
    let typ = FirType::Fun {
        args: args.iter().map(|a| a.typ.clone()).collect(),
        ret: Box::new(FirType::Void),
    };
    b.declare_fun(name, typ, &args, Some(body), false)
}

fn sample_rate_arg() -> NamedType {
    NamedType {
        name: "sample_rate".to_owned(),
        typ: I32,
    }
}

/// Options that make the module wrong in one way, for the refusal tests.
#[derive(Default, Clone, Copy)]
struct Twist {
    /// `staticInit` also writes `fIOTA`, an instance field.
    shared_and_instance: bool,
    /// `compute` takes the address of `fRec0`.
    address: bool,
    /// The module already declares `fVoices`.
    voices_taken: bool,
    /// `fHslider0` is declared `Quad`.
    quad: bool,
    /// `instanceResetUserInterface` stores a `Float32` constant into the
    /// `FaustFloat` control, as the producer does with a `StoreVar`.
    uncast_store: bool,
}

/// The module of the file doc, and its store.
fn module(twist: Twist) -> (FirStore, FirId) {
    let mut s = FirStore::new();
    let mut b = FirBuilder::new(&mut s);

    let mut fields = vec![
        b.declare_var("fSampleRate", I32, AccessType::Struct, None),
        b.declare_var(
            "fHslider0",
            if twist.quad { FirType::Quad } else { FF },
            AccessType::Struct,
            None,
        ),
        b.declare_var("fRec0", arr(FF, 2), AccessType::Struct, None),
        b.declare_var("iRec1", arr(I32, 2), AccessType::Struct, None),
        b.declare_var("fVec0", arr(FF, 8), AccessType::Struct, None),
        b.declare_var("fIOTA", I32, AccessType::Struct, None),
        b.declare_var("mydspSIG0_iRec2", arr(I32, 2), AccessType::Struct, None),
    ];
    if twist.voices_taken {
        fields.push(b.declare_var(VOICES_FIELD, I32, AccessType::Struct, None));
    }
    let table = b.declare_var("ftbl0mydspSIG0", arr(FF, 4), AccessType::Static, None);

    // staticInit: the generator's working state and the shared table.
    let one = b.int32(1);
    let zero = b.int32(0);
    let half = b.float32(0.5);
    let half = b.cast(FF, half);
    let mut static_body = vec![
        b.store_table("mydspSIG0_iRec2", AccessType::Struct, zero, one),
        b.store_table("ftbl0mydspSIG0", AccessType::Static, zero, half),
    ];
    if twist.shared_and_instance {
        static_body.push(b.store_var("fIOTA", AccessType::Struct, zero));
    }
    let static_body = b.block(&static_body);
    let static_init = fun(&mut b, "staticInit", &[sample_rate_arg()], static_body);

    let sr = b.load_var("sample_rate", AccessType::FunArgs, I32);
    let set_sr = b.store_var("fSampleRate", AccessType::Struct, sr);
    let constants_body = b.block(&[set_sr]);
    let constants = fun(
        &mut b,
        "instanceConstants",
        &[sample_rate_arg()],
        constants_body,
    );

    let raw_half = b.float32(0.5);
    let set_slider = b.store_var(
        "fHslider0",
        AccessType::Struct,
        if twist.uncast_store { raw_half } else { half },
    );
    let reset_body = b.block(&[set_slider]);
    let reset = fun(&mut b, "instanceResetUserInterface", &[], reset_body);

    let l0 = b.load_var("l0", AccessType::Loop, I32);
    let fzero = b.float32(0.0);
    let fzero = b.cast(FF, fzero);
    let clear_rec = b.store_table("fRec0", AccessType::Struct, l0, fzero);
    let loop_body = b.block(&[clear_rec]);
    let two = b.int32(2);
    let clear_loop = b.simple_for_loop("l0", two, loop_body, false);
    let clear_iota = b.store_var("fIOTA", AccessType::Struct, zero);
    let clear_body = b.block(&[clear_loop, clear_iota]);
    let clear = fun(&mut b, "instanceClear", &[], clear_body);

    let slider = b.add_slider(
        SliderType::Horizontal,
        "gain",
        "fHslider0",
        SliderRange {
            init: 0.5,
            lo: 0.0,
            hi: 1.0,
            step: 0.01,
        },
    );
    let ui_body = b.block(&[slider]);
    let ui_arg = NamedType {
        name: "ui".to_owned(),
        typ: FirType::UI,
    };
    let ui = fun(&mut b, "buildUserInterface", &[ui_arg], ui_body);

    // compute: fRec0[0] = fHslider0 + fRec0[1]; fVec0[fIOTA & 7] = fRec0[0];
    // fIOTA = fIOTA + 1; iRec1[0] = iRec1[1] + 1.
    let gain = b.load_var("fHslider0", AccessType::Struct, FF);
    let rec1 = b.load_table("fRec0", AccessType::Struct, one, FF);
    let sum = b.binop(FirBinOp::Add, gain, rec1, FF);
    let set_rec = b.store_table("fRec0", AccessType::Struct, zero, sum);
    let iota = b.load_var("fIOTA", AccessType::Struct, I32);
    let seven = b.int32(7);
    let at = b.binop(FirBinOp::And, iota, seven, I32);
    let rec0 = b.load_table("fRec0", AccessType::Struct, zero, FF);
    let set_vec = b.store_table("fVec0", AccessType::Struct, at, rec0);
    let next = b.binop(FirBinOp::Add, iota, one, I32);
    let set_iota = b.store_var("fIOTA", AccessType::Struct, next);
    let irec1 = b.load_table("iRec1", AccessType::Struct, one, I32);
    let inc = b.binop(FirBinOp::Add, irec1, one, I32);
    let set_irec = b.store_table("iRec1", AccessType::Struct, zero, inc);
    let mut compute_body = vec![set_rec, set_vec, set_iota, set_irec];
    if twist.address {
        let addr = b.load_var_address("fRec0", AccessType::Struct, arr(FF, 2));
        compute_body.push(b.drop_(addr));
    }
    let compute_body = b.block(&compute_body);
    let streams = FirType::Ptr(Box::new(FirType::Ptr(Box::new(FF))));
    let compute = fun(
        &mut b,
        "compute",
        &[
            NamedType {
                name: "count".to_owned(),
                typ: I32,
            },
            NamedType {
                name: "inputs".to_owned(),
                typ: streams.clone(),
            },
            NamedType {
                name: "outputs".to_owned(),
                typ: streams,
            },
        ],
        compute_body,
    );

    let dsp_struct = b.block(&fields);
    let globals = b.block(&[]);
    let functions = b.block(&[static_init, constants, reset, clear, ui, compute]);
    let static_decls = b.block(&[table]);
    let root = b.module(
        1,
        1,
        "mydsp",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (s, root)
}

fn sections(s: &FirStore, root: FirId) -> (Vec<FirId>, Vec<FirId>, Vec<FirId>) {
    let FirMatch::Module {
        dsp_struct,
        functions,
        static_decls,
        ..
    } = match_fir(s, root)
    else {
        panic!("not a module");
    };
    let items = |id| match match_fir(s, id) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    };
    (items(dsp_struct), items(functions), items(static_decls))
}

fn function(s: &FirStore, root: FirId, name: &str) -> (Vec<NamedType>, FirType, FirId) {
    let (_, functions, _) = sections(s, root);
    functions
        .into_iter()
        .find_map(|f| match match_fir(s, f) {
            FirMatch::DeclareFun {
                name: n,
                args,
                typ,
                body: Some(body),
                ..
            } if n == name => Some((args, typ, body)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no function {name}"))
}

/// Every `Struct` access reachable from `id`, by name.
fn struct_names(s: &FirStore, id: FirId, out: &mut Vec<String>) {
    match match_fir(s, id) {
        FirMatch::LoadVar {
            name,
            access: AccessType::Struct,
            ..
        }
        | FirMatch::LoadTable {
            name,
            access: AccessType::Struct,
            ..
        }
        | FirMatch::StoreVar {
            name,
            access: AccessType::Struct,
            ..
        }
        | FirMatch::StoreTable {
            name,
            access: AccessType::Struct,
            ..
        } => out.push(name),
        _ => {}
    }
    for child in s
        .arena
        .children(id)
        .map(<[FirId]>::to_vec)
        .unwrap_or_default()
    {
        struct_names(s, child, out);
    }
}

#[test]
fn layout_places_each_field_by_element_type_in_declaration_order() {
    let (s, root) = module(Twist::default());
    let (_, _, layout) = layout_voices(&s, root).expect("the module is supported");
    let placed: Vec<(&str, &str, usize, usize)> = layout
        .placements
        .iter()
        .map(|p| (p.field.as_str(), p.arena.as_str(), p.base, p.len))
        .collect();
    assert_eq!(
        placed,
        [
            ("fSampleRate", "fStateInt32", 0, 1),
            ("fHslider0", "fStateFaustFloat", 0, 1),
            ("fRec0", "fStateFaustFloat", 1, 2),
            ("iRec1", "fStateInt32", 1, 2),
            ("fVec0", "fStateFaustFloat", 3, 8),
            ("fIOTA", "fStateInt32", 3, 1),
        ]
    );
    let arenas: Vec<(&str, usize)> = layout
        .arenas
        .iter()
        .map(|a| (a.name.as_str(), a.elements))
        .collect();
    assert_eq!(arenas, [("fStateInt32", 4), ("fStateFaustFloat", 11)]);
    assert_eq!(layout.shared, ["mydspSIG0_iRec2"]);
    assert_eq!(layout.controls, ["fHslider0"]);
}

#[test]
fn dsp_struct_holds_the_shared_fields_the_voice_count_and_the_arenas() {
    let (s, root) = module(Twist::default());
    let (out, root, _) = layout_voices(&s, root).unwrap();
    let (dsp_struct, _, static_decls) = sections(&out, root);
    let decls: Vec<(String, FirType)> = dsp_struct
        .iter()
        .filter_map(|&d| match match_fir(&out, d) {
            FirMatch::DeclareVar { name, typ, .. } => Some((name, typ)),
            _ => None,
        })
        .collect();
    assert_eq!(
        decls,
        [
            ("mydspSIG0_iRec2".to_owned(), arr(I32, 2)),
            (VOICES_FIELD.to_owned(), I32),
            ("fStateInt32".to_owned(), FirType::Ptr(Box::new(I32))),
            ("fStateFaustFloat".to_owned(), FirType::Ptr(Box::new(FF))),
        ]
    );
    // The shared table stays, and the control bases follow it.
    let tables: Vec<String> = static_decls
        .iter()
        .filter_map(|&d| match match_fir(&out, d) {
            FirMatch::DeclareVar { name, .. } | FirMatch::DeclareTable { name, .. } => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(tables, ["ftbl0mydspSIG0", CONTROLS_TABLE]);
    let controls = static_decls
        .iter()
        .find_map(|&d| match match_fir(&out, d) {
            FirMatch::DeclareTable { name, values, .. } if name == CONTROLS_TABLE => Some(values),
            _ => None,
        })
        .unwrap();
    let bases: Vec<FirMatch> = controls.iter().map(|&v| match_fir(&out, v)).collect();
    assert!(matches!(
        bases.as_slice(),
        [FirMatch::Int32 { value: 0, .. }]
    ));
}

#[test]
fn instance_functions_gain_the_voice_after_dsp_and_the_others_do_not() {
    let (s, src_root) = module(Twist::default());
    let (out, root, _) = layout_voices(&s, src_root).unwrap();
    for name in INSTANCE_FUNCTIONS {
        let (args, typ, _) = function(&out, root, name);
        assert_eq!(args[0].name, "dsp", "{name}");
        assert_eq!(args[1].name, VOICE_ARG, "{name}");
        assert_eq!(args[1].typ, I32, "{name}");
        let FirType::Fun { args: types, .. } = typ else {
            panic!("{name} has no function type");
        };
        assert_eq!(types.len(), args.len(), "{name}");
        assert_eq!(types[1], I32, "{name}");
    }
    for name in ["staticInit", "buildUserInterface"] {
        let (before, _, body_before) = function(&s, src_root, name);
        let (after, _, body_after) = function(&out, root, name);
        assert_eq!(before, after, "{name}");
        assert_eq!(
            canonical_fir_fingerprint(&s, body_before),
            canonical_fir_fingerprint(&out, body_after),
            "{name}"
        );
    }
}

#[test]
fn no_access_to_a_per_voice_field_remains() {
    let (s, root) = module(Twist::default());
    let (out, root, layout) = layout_voices(&s, root).unwrap();
    for name in INSTANCE_FUNCTIONS {
        let (_, _, body) = function(&out, root, name);
        let mut names = Vec::new();
        struct_names(&out, body, &mut names);
        for n in &names {
            assert!(
                layout.placement(n).is_none(),
                "{name} still accesses per-voice field {n}"
            );
            assert!(
                n == VOICES_FIELD || layout.arenas.iter().any(|a| &a.name == n),
                "{name} accesses {n}"
            );
        }
        assert!(!names.is_empty(), "{name} lost its state accesses");
    }
}

/// `fRec0[1]` (base 1 in the FaustFloat arena) reads
/// `fStateFaustFloat[(1 + 1) * fVoices + voice]`.
#[test]
fn an_element_access_is_based_strided_and_offset_by_the_voice() {
    let (s, root) = module(Twist::default());
    let (out, root, _) = layout_voices(&s, root).unwrap();
    let (_, _, body) = function(&out, root, "compute");
    // Rebuild the expected index in the output store and look for it.
    let mut out = out;
    let expected = {
        let mut b = FirBuilder::new(&mut out);
        let base = b.int32(1);
        let one = b.int32(1);
        let row = b.binop(FirBinOp::Add, base, one, I32);
        let n = b.load_var(VOICES_FIELD, AccessType::Struct, I32);
        let scaled = b.binop(FirBinOp::Mul, row, n, I32);
        let v = b.load_var(VOICE_ARG, AccessType::FunArgs, I32);
        let index = b.binop(FirBinOp::Add, scaled, v, I32);
        b.load_table("fStateFaustFloat", AccessType::Struct, index, FF)
    };
    assert!(
        reaches(&out, body, expected),
        "fRec0[1] was not rewritten as expected"
    );
    // A scalar: fHslider0 (base 0) reads fStateFaustFloat[0 * fVoices + voice].
    let expected_scalar = {
        let mut b = FirBuilder::new(&mut out);
        let base = b.int32(0);
        let n = b.load_var(VOICES_FIELD, AccessType::Struct, I32);
        let scaled = b.binop(FirBinOp::Mul, base, n, I32);
        let v = b.load_var(VOICE_ARG, AccessType::FunArgs, I32);
        let index = b.binop(FirBinOp::Add, scaled, v, I32);
        b.load_table("fStateFaustFloat", AccessType::Struct, index, FF)
    };
    assert!(
        reaches(&out, body, expected_scalar),
        "fHslider0 was not rewritten as expected"
    );
}

fn reaches(s: &FirStore, from: FirId, target: FirId) -> bool {
    from == target
        || s.arena
            .children(from)
            .map(<[FirId]>::to_vec)
            .unwrap_or_default()
            .into_iter()
            .any(|c| reaches(s, c, target))
}

#[test]
fn the_layout_is_deterministic() {
    let (s, root) = module(Twist::default());
    let (a, ra, la) = layout_voices(&s, root).unwrap();
    let (b, rb, lb) = layout_voices(&s, root).unwrap();
    assert_eq!(la, lb);
    assert_eq!(
        canonical_fir_fingerprint(&a, ra),
        canonical_fir_fingerprint(&b, rb)
    );
}

#[test]
fn a_field_touched_by_static_init_and_the_lifecycle_is_refused() {
    let (s, root) = module(Twist {
        shared_and_instance: true,
        ..Twist::default()
    });
    assert_eq!(
        layout_voices(&s, root).unwrap_err(),
        BatchError::SharedAndInstance("fIOTA".to_owned())
    );
}

#[test]
fn the_address_of_a_per_voice_field_is_refused() {
    let (s, root) = module(Twist {
        address: true,
        ..Twist::default()
    });
    assert!(matches!(
        layout_voices(&s, root).unwrap_err(),
        BatchError::UnsupportedAccess { field, .. } if field == "fRec0"
    ));
}

#[test]
fn an_added_name_already_declared_is_refused() {
    let (s, root) = module(Twist {
        voices_taken: true,
        ..Twist::default()
    });
    assert_eq!(
        layout_voices(&s, root).unwrap_err(),
        BatchError::NameTaken(VOICES_FIELD.to_owned())
    );
}

#[test]
fn a_per_voice_field_of_a_type_no_arena_holds_is_refused() {
    let (s, root) = module(Twist {
        quad: true,
        ..Twist::default()
    });
    assert!(matches!(
        layout_voices(&s, root).unwrap_err(),
        BatchError::UnsupportedFieldType { field, .. } if field == "fHslider0"
    ));
}

#[test]
fn a_module_with_sub_modules_is_refused() {
    let (mut s, root) = module(Twist::default());
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        ..
    } = match_fir(&s, root)
    else {
        panic!();
    };
    let mut b = FirBuilder::new(&mut s);
    let sub = b.module(
        0,
        1,
        "mydspSIG0",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    let root = b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[sub],
    );
    assert_eq!(layout_voices(&s, root).unwrap_err(), BatchError::SubModules);
}

#[test]
fn a_non_module_root_is_refused() {
    let mut s = FirStore::new();
    let one = FirBuilder::new(&mut s).int32(1);
    assert_eq!(layout_voices(&s, one).unwrap_err(), BatchError::NotAModule);
}

#[test]
fn the_output_passes_the_fir_verifier_when_the_source_does() {
    let (s, root) = module(Twist::default());
    let before = crate::checker::verify_fir_module(&s, root);
    let (out, root, _) = layout_voices(&s, root).unwrap();
    let after = crate::checker::verify_fir_module(&out, root);
    let errors = |r: &crate::checker::FirVerifyReport| {
        r.errors().map(|d| format!("{d:?}")).collect::<Vec<_>>()
    };
    assert_eq!(
        errors(&before),
        Vec::<String>::new(),
        "the test module itself"
    );
    assert_eq!(errors(&after), Vec::<String>::new());
}

/// A `StoreVar` of a `Float32` into a `FaustFloat` field becomes a typed
/// `StoreTable` with an explicit cast, which the verifier accepts.
#[test]
fn a_store_of_another_type_is_cast_to_the_arena_type() {
    let (s, root) = module(Twist {
        uncast_store: true,
        ..Twist::default()
    });
    let (mut out, root, _) = layout_voices(&s, root).unwrap();
    assert!(!crate::checker::verify_fir_module(&out, root).has_errors());
    let (_, _, body) = function(&out, root, "instanceResetUserInterface");
    let cast = {
        let mut b = FirBuilder::new(&mut out);
        let raw = b.float32(0.5);
        b.cast(FF, raw)
    };
    assert!(reaches(&out, body, cast), "the stored constant is not cast");
}
