//! The contract every textual backend keeps, checked on all seven at once.
//!
//! # Every FIR statement is either emitted or refused
//!
//! The module contract (`backends/mod.rs`) says an unsupported FIR node must
//! fail with a stable backend error. Some backends instead accepted a few
//! statements and emitted nothing for them — a delay line that never shifts,
//! a dropped call with side effects — which compiles and computes the wrong
//! thing. This test builds one `compute` body per statement kind and checks,
//! for each backend, that the output either differs from the output for an
//! empty body or is an error. "Accepted and vanished" is the failure.
//!
//! Each statement's body stores to a state field, so a construct that is
//! emitted is visible in the text even when its own syntax is empty.

use codegen::backends::{asc, c, cmajor, codebox, cpp, julia, rust};
use codegen::fixtures;
use fir::{AccessType, FirBuilder, FirId, FirStore, FirType};

type Generator = fn(&FirStore, FirId) -> Result<String, String>;

/// Each backend with the function its sample loop is read from: codebox
/// (one sample per call, by contract) emits `frame`, the others `compute`.
fn backends() -> Vec<(&'static str, &'static str, Generator)> {
    vec![
        ("c", "compute", |s, m| {
            c::generate_c_module(s, m, &c::COptions::default()).map_err(|e| e.to_string())
        }),
        ("cpp", "compute", |s, m| {
            cpp::generate_cpp_module(s, m, &cpp::CppOptions::default()).map_err(|e| e.to_string())
        }),
        ("rust", "compute", |s, m| {
            rust::generate_rust_module(s, m, &rust::RustOptions::default())
                .map_err(|e| e.to_string())
        }),
        ("julia", "compute", |s, m| {
            julia::generate_julia_module(s, m, &julia::JuliaOptions::default())
                .map_err(|e| e.to_string())
        }),
        ("asc", "compute", |s, m| {
            asc::generate_asc_module(s, m, &asc::AscOptions::default()).map_err(|e| e.to_string())
        }),
        ("cmajor", "compute", |s, m| {
            cmajor::generate_cmajor_module(s, m, &cmajor::CmajorOptions::default())
                .map_err(|e| e.to_string())
        }),
        ("codebox", "frame", |s, m| {
            codebox::generate_codebox_module(s, m, &codebox::CodeboxOptions::default())
                .map_err(|e| e.to_string())
        }),
    ]
}

/// A module whose `entry` function's body is `stmts`, with the state the
/// statements write to.
fn module_with(
    entry: &str,
    build: impl FnOnce(&mut FirBuilder<'_>) -> Vec<FirId>,
) -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let f_state = b.declare_var("fState", FirType::Float32, AccessType::Struct, None);
    let i_state = b.declare_var("iState", FirType::Int32, AccessType::Struct, None);
    let f_vec = b.declare_var(
        "fVec",
        FirType::Array(Box::new(FirType::Float32), 4),
        AccessType::Struct,
        None,
    );
    let stmts = build(&mut b);
    let body = b.block(&stmts);
    let compute = b.declare_fun(
        entry,
        fixtures_compute_type(),
        &fixtures_compute_args(),
        Some(body),
        false,
    );
    let dsp_struct = b.block(&[f_state, i_state, f_vec]);
    let globals = b.block(&[]);
    let functions = b.block(&[compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        1,
        "coverage",
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

fn fixtures_compute_type() -> FirType {
    let (store, module) = fixtures::build_passthrough_test_module();
    compute_decl(&store, module).0
}

fn fixtures_compute_args() -> Vec<fir::NamedType> {
    let (store, module) = fixtures::build_passthrough_test_module();
    compute_decl(&store, module).1
}

fn compute_decl(store: &FirStore, module: FirId) -> (FirType, Vec<fir::NamedType>) {
    let fir::FirMatch::Module { functions, .. } = fir::match_fir(store, module) else {
        unreachable!()
    };
    let fir::FirMatch::Block(items) = fir::match_fir(store, functions) else {
        unreachable!()
    };
    items
        .into_iter()
        .find_map(|item| match fir::match_fir(store, item) {
            fir::FirMatch::DeclareFun {
                name, typ, args, ..
            } if name == "compute" => Some((typ, args)),
            _ => None,
        })
        .expect("passthrough fixture has a compute")
}

/// `fState = 1.0` — the visible effect inside every construct.
fn effect(b: &mut FirBuilder<'_>) -> FirId {
    let one = b.float32(1.0);
    b.store_var("fState", AccessType::Struct, one)
}

fn effect_block(b: &mut FirBuilder<'_>) -> FirId {
    let store = effect(b);
    b.block(&[store])
}

/// Builds the statements of one `compute` body.
type StatementBuilder = fn(&mut FirBuilder<'_>) -> Vec<FirId>;

fn statement_kinds() -> Vec<(&'static str, StatementBuilder)> {
    vec![
        ("StoreVar", |b| vec![effect(b)]),
        ("StoreTable", |b| {
            let index = b.int32(1);
            let value = b.float32(2.0);
            vec![b.store_table("fVec", AccessType::Struct, index, value)]
        }),
        ("DeclareVar", |b| {
            let init = b.float32(3.0);
            let decl = b.declare_var("fLocal", FirType::Float32, AccessType::Stack, Some(init));
            let read = b.load_var("fLocal", AccessType::Stack, FirType::Float32);
            let assign = b.store_var("fState", AccessType::Struct, read);
            vec![decl, assign]
        }),
        ("Drop", |b| {
            let x = b.float32(0.5);
            let call = b.fun_call("my_effect", &[x], FirType::Float32);
            vec![b.drop_(call)]
        }),
        ("If", |b| {
            let cond = b.load_var("iState", AccessType::Struct, FirType::Int32);
            let then_block = effect_block(b);
            vec![b.if_(cond, then_block, None)]
        }),
        ("Control", |b| {
            let cond = b.load_var("iState", AccessType::Struct, FirType::Int32);
            let stmt = effect(b);
            vec![b.control(cond, stmt)]
        }),
        ("ForLoop", |b| {
            let zero = b.int32(0);
            let init = b.declare_var("j0", FirType::Int32, AccessType::Loop, Some(zero));
            let four = b.int32(4);
            let one = b.int32(1);
            let body = effect_block(b);
            vec![b.for_loop("j0", init, four, one, body, false)]
        }),
        ("SimpleForLoop", |b| {
            let four = b.int32(4);
            let body = effect_block(b);
            vec![b.simple_for_loop("j0", four, body, false)]
        }),
        ("WhileLoop", |b| {
            let cond = b.load_var("iState", AccessType::Struct, FirType::Int32);
            let body = effect_block(b);
            vec![b.while_loop(cond, body)]
        }),
        ("Switch", |b| {
            let cond = b.load_var("iState", AccessType::Struct, FirType::Int32);
            let case = effect_block(b);
            vec![b.switch(cond, &[(1, case)], None)]
        }),
        ("IteratorForLoop", |b| {
            let body = effect_block(b);
            vec![b.iterator_for_loop(&["it0"], false, body)]
        }),
        ("ShiftArrayVar", |b| {
            vec![b.shift_array_var("fVec", AccessType::Struct, 3)]
        }),
        ("DeclareStructType", |b| {
            vec![b.declare_struct_type(FirType::Float32)]
        }),
    ]
}

/// The (backend, statement) pairs a backend accepts and then emits nothing
/// for.
fn silent_drops() -> Vec<String> {
    let mut found = Vec::new();
    for (backend, entry, generate) in backends() {
        let (store, empty) = module_with(entry, |_| Vec::new());
        let baseline = generate(&store, empty).unwrap_or_else(|e| {
            panic!("{backend}: the empty-body module must compile: {e}");
        });
        for (kind, build) in statement_kinds() {
            let (store, module) = module_with(entry, build);
            if let Ok(text) = generate(&store, module)
                && text == baseline
            {
                found.push(format!("{backend}/{kind}"));
            }
        }
    }
    found
}

#[test]
fn no_textual_backend_accepts_a_statement_and_emits_nothing() {
    assert_eq!(silent_drops(), Vec::<String>::new());
}

/// `IteratorForLoop` is a vector-mode loop no scalar textual backend lowers.
/// Emitting its body once (asc did) compiles and computes the wrong thing, so
/// every backend refuses it.
#[test]
fn every_textual_backend_refuses_an_iterator_for_loop() {
    let accepted: Vec<&str> = backends()
        .into_iter()
        .filter(|(_, entry, generate)| {
            let (store, module) = module_with(entry, |b| {
                let body = effect_block(b);
                vec![b.iterator_for_loop(&["it0"], false, body)]
            });
            generate(&store, module).is_ok()
        })
        .map(|(backend, _, _)| backend)
        .collect();
    assert_eq!(accepted, Vec::<&str>::new());
}

/// A foreign variable (`fvariable`, `DeclareVar` with `AccessType::Global`)
/// is a host symbol the code reads; c and cpp never declared it, while rust,
/// julia and asc made it a struct field initialized to zero.
#[test]
fn a_foreign_variable_is_read_never_declared_as_state() {
    for (backend, _, generate) in backends()
        .into_iter()
        .filter(|(name, _, _)| ["c", "cpp", "rust", "julia", "asc"].contains(name))
    {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let f_state = b.declare_var("fState", FirType::Float32, AccessType::Struct, None);
        let foreign = b.declare_var("gBar", FirType::Float32, AccessType::Global, None);
        let read = b.load_var("gBar", AccessType::Global, FirType::Float32);
        let assign = b.store_var("fState", AccessType::Struct, read);
        let body = b.block(&[assign]);
        let compute = b.declare_fun(
            "compute",
            fixtures_compute_type(),
            &fixtures_compute_args(),
            Some(body),
            false,
        );
        let dsp_struct = b.block(&[f_state]);
        let globals = b.block(&[foreign]);
        let functions = b.block(&[compute]);
        let static_decls = b.block(&[]);
        let module = b.module(
            0,
            1,
            "foreign",
            dsp_struct,
            globals,
            functions,
            static_decls,
            &[],
        );
        let text = generate(&store, module).unwrap_or_else(|e| panic!("{backend}: {e}"));
        assert_eq!(text.matches("gBar").count(), 1, "{backend}:\n{text}");
    }
}

/// A body-less declaration of a lifecycle function (a prototype) changes
/// nothing: the backend synthesizes the same stub it synthesizes when the
/// function is absent. cpp let the prototype suppress the stub and emitted a
/// declaration with no definition; asc emitted nothing while `instanceInit`
/// still called the method.
#[test]
fn a_lifecycle_prototype_does_not_suppress_its_stub() {
    for (backend, entry, generate) in backends() {
        let (store, plain) = module_with(entry, |_| Vec::new());
        let expected = generate(&store, plain).unwrap_or_else(|e| panic!("{backend}: {e}"));
        let (store, with_prototype) = module_with_prototype(entry, "instanceClear");
        let text = generate(&store, with_prototype).unwrap_or_else(|e| panic!("{backend}: {e}"));
        assert_eq!(text, expected, "{backend}");
    }
}

/// [`module_with`] an empty body, plus a body-less declaration of `name`.
fn module_with_prototype(entry: &str, name: &str) -> (FirStore, FirId) {
    let (mut store, module) = module_with(entry, |_| Vec::new());
    let fir::FirMatch::Module {
        num_inputs,
        num_outputs,
        name: module_name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        ..
    } = fir::match_fir(&store, module)
    else {
        unreachable!()
    };
    let fir::FirMatch::Block(mut items) = fir::match_fir(&store, functions) else {
        unreachable!()
    };
    let mut b = FirBuilder::new(&mut store);
    let prototype = b.declare_fun(
        name,
        FirType::Fun {
            args: vec![FirType::Ptr(Box::new(FirType::Obj))],
            ret: Box::new(FirType::Void),
        },
        &[fir::NamedType {
            name: "dsp".to_owned(),
            typ: FirType::Ptr(Box::new(FirType::Obj)),
        }],
        None,
        false,
    );
    items.insert(0, prototype);
    let functions = b.block(&items);
    let module = b.module(
        num_inputs,
        num_outputs,
        module_name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    );
    (store, module)
}

/// A `Switch` condition is evaluated once, as C's `switch` evaluates it:
/// julia and cmajor rendered the cases as an `if`/`elseif` chain that
/// re-evaluated a call in the condition once per case.
#[test]
fn a_switch_condition_is_evaluated_once() {
    for (backend, entry, generate) in backends() {
        let (store, module) = module_with(entry, |b| {
            let cond = b.fun_call("my_cond", &[], FirType::Int32);
            let first = effect_block(b);
            let second = effect_block(b);
            vec![b.switch(cond, &[(1, first), (2, second)], None)]
        });
        // A backend that refuses `Switch` (codebox) has nothing to evaluate.
        if let Ok(text) = generate(&store, module) {
            assert_eq!(text.matches("my_cond").count(), 1, "{backend}:\n{text}");
        }
    }
}

/// The canonical FIR names of a real `min`/`max` are `fmin`/`fmax`
/// (`FirMathOp::{Min, Max}`); codebox has only `min`/`max`, which upstream
/// emits, and passed `fmin`/`fmax` through unchanged.
#[test]
fn codebox_spells_a_real_min_and_max_as_its_own_builtins() {
    let (_, entry, generate) = backends()
        .into_iter()
        .find(|(name, _, _)| *name == "codebox")
        .expect("codebox is listed");
    let (store, module) = module_with(entry, |b| {
        let x = b.load_var("fState", AccessType::Struct, FirType::Float32);
        let half = b.float32(0.5);
        let low = b.fun_call("fmin", &[x, half], FirType::Float32);
        let high = b.fun_call("fmax", &[low, half], FirType::Float32);
        vec![b.store_var("fState", AccessType::Struct, high)]
    });
    let text = generate(&store, module).expect("min/max compile");
    assert!(text.contains("max(min(fState"), "{text}");
    assert!(!text.contains("fmin") && !text.contains("fmax"), "{text}");
}

/// `exp2` is a method of the float type like `log2`; the rust backend
/// mapped `log2f` to `f32::log2` but left `exp2f` a bare call to a function
/// nothing defines.
#[test]
fn rust_maps_exp2_like_log2() {
    let (_, entry, generate) = backends()
        .into_iter()
        .find(|(name, _, _)| *name == "rust")
        .expect("rust is listed");
    let (store, module) = module_with(entry, |b| {
        let x = b.load_var("fState", AccessType::Struct, FirType::Float32);
        let up = b.fun_call("exp2f", &[x], FirType::Float32);
        let down = b.fun_call("log2f", &[up], FirType::Float32);
        vec![b.store_var("fState", AccessType::Struct, down)]
    });
    let text = generate(&store, module).expect("exp2/log2 compile");
    assert!(text.contains("f32::log2(f32::exp2("), "{text}");
}

/// A module without a `metadata` function gets the compilation metadata,
/// as with an empty one: the c backend declared a bring-up placeholder
/// (`"faust-rs": "module-first c backend prototype"`) instead.
#[test]
fn c_replays_the_compilation_metadata_when_metadata_is_absent() {
    let (_, entry, generate) = backends()
        .into_iter()
        .find(|(name, _, _)| *name == "c")
        .expect("c is listed");
    let (store, module) = module_with(entry, |_| Vec::new());
    let text = generate(&store, module).expect("compiles");
    assert!(
        text.contains("m->declare(m->metaInterface, \"filename\", \"mydsp.dsp\");"),
        "{text}"
    );
    assert!(!text.contains("module-first c backend prototype"), "{text}");
}
