//! Every FIR statement a textual backend meets is either emitted or refused.
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
