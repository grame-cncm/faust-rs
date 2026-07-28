//! Integration tests for `core_eval`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use boxes::{BoxBuilder, BoxMatch, match_box};
use diagnostics::{IntoDiagnostic, Severity, Stage, codes};
use eval::{
    Environment, EvalError, EvalSourceContext, LoopDetector, eval_box, eval_process,
    eval_process_with_source_context, eval_process_with_stats,
};
use parser::{CompilationMetadataKey, parse_file_with_imports, parse_program};
use propagate::ArityCache;
use tlib::{NodeKind, TreeArena, TreeId};

fn make_ident(arena: &mut TreeArena, name: &str) -> tlib::TreeId {
    BoxBuilder::new(arena).ident(name)
}

fn make_wire(arena: &mut TreeArena) -> tlib::TreeId {
    BoxBuilder::new(arena).wire()
}

fn make_def(
    arena: &mut TreeArena,
    name: &str,
    args: tlib::TreeId,
    expr: tlib::TreeId,
) -> tlib::TreeId {
    let ident = make_ident(arena, name);
    let payload = arena.cons(args, expr);
    arena.cons(ident, payload)
}

fn make_defs(arena: &mut TreeArena, defs: &[tlib::TreeId]) -> tlib::TreeId {
    let mut out = arena.nil();
    for def in defs.iter().rev() {
        out = arena.cons(*def, out);
    }
    out
}

fn make_rev_list2(arena: &mut TreeArena, a: TreeId, b: TreeId) -> TreeId {
    let nil = arena.nil();
    let t = arena.cons(a, nil);
    arena.cons(b, t)
}

fn make_rev_list3(arena: &mut TreeArena, a: TreeId, b: TreeId, c: TreeId) -> TreeId {
    let nil = arena.nil();
    let t1 = arena.cons(a, nil);
    let t2 = arena.cons(b, t1);
    arena.cons(c, t2)
}

fn expect_int(arena: &TreeArena, id: TreeId, expected: i32) {
    assert_eq!(match_box(arena, id), BoxMatch::Int(expected));
}

fn expect_label(arena: &TreeArena, id: TreeId, expected: &str) {
    match arena.kind(id) {
        Some(NodeKind::StringLiteral(text)) | Some(NodeKind::Symbol(text)) => {
            assert_eq!(text.as_ref(), expected)
        }
        other => panic!("expected string-like label, got {other:?}"),
    }
}

fn count_add_nodes(arena: &TreeArena, root: TreeId) -> usize {
    let mut n = 0usize;
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if matches!(match_box(arena, id), BoxMatch::Add) {
            n += 1;
        }
        if let Some(node) = arena.node(id) {
            for child in node.children.as_slice() {
                stack.push(*child);
            }
        }
    }
    n
}

fn make_rule1(arena: &mut TreeArena, pattern: TreeId, rhs: TreeId) -> TreeId {
    let nil = arena.nil();
    let lhs = arena.cons(pattern, nil);
    arena.cons(lhs, rhs)
}

fn make_rules_parser_order(arena: &mut TreeArena, source_rules: &[TreeId]) -> TreeId {
    let mut out = arena.nil();
    for rule in source_rules {
        out = arena.cons(*rule, out);
    }
    out
}

fn temp_root(test_name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock drift")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "faust_rs_eval_{test_name}_{}_{}",
        std::process::id(),
        stamp
    ));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn cpp_bin() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FAUST_CPP_BIN") {
        return Some(PathBuf::from(path));
    }
    let default = PathBuf::from("/usr/local/bin/faust");
    default.exists().then_some(default)
}

fn cpp_compiles_file(cpp_bin: &Path, input_path: &Path, out_path: &Path) -> Result<bool, String> {
    let output = Command::new(cpp_bin)
        .arg(input_path)
        .arg("-lang")
        .arg("c")
        .arg("-o")
        .arg(out_path)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", cpp_bin.display()))?;
    Ok(output.status.success())
}

#[test]
fn eval_process_resolves_named_definition() {
    let mut arena = TreeArena::new();
    let wire = make_wire(&mut arena);
    let nil = arena.nil();
    let foo = make_def(&mut arena, "foo", nil, wire);
    let foo_ident = make_ident(&mut arena, "foo");
    let process = make_def(&mut arena, "process", nil, foo_ident);
    let root = make_defs(&mut arena, &[foo, process]);

    let out = eval_process(&mut arena, root).expect("evaluation should succeed");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn eval_process_treats_metadata_wrapper_as_evaluation_transparent() {
    let mut arena = TreeArena::new();
    let wire = make_wire(&mut arena);
    let md_key = arena.symbol("test/foo:author");
    let md_value = arena.string_lit("Alice");
    let md_pair = arena.cons(md_key, md_value);
    let foo_expr = BoxBuilder::new(&mut arena).metadata(wire, md_pair);
    let nil = arena.nil();
    let foo = make_def(&mut arena, "foo", nil, foo_expr);
    let foo_ident = make_ident(&mut arena, "foo");
    let process = make_def(&mut arena, "process", nil, foo_ident);
    let root = make_defs(&mut arena, &[foo, process]);

    let out = eval_process(&mut arena, root).expect("metadata wrapper should not block eval");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn eval_process_preserves_forward_ad_wrapper_around_evaluated_body() {
    let parsed = parse_program(
        "process = fad(hslider(\"f\", 440, 50, 2000, 1) : sin, hslider(\"f\", 440, 50, 2000, 1));",
        "eval_fad_wrapper.dsp",
    );
    assert!(
        parsed.errors.is_empty(),
        "parser should accept fad fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");

    let out = eval_process(&mut arena, root).expect("fad process should evaluate");
    let BoxMatch::ForwardAD(inner, _seed) = match_box(&arena, out) else {
        panic!("expected eval to preserve BOXFAUTODIFF");
    };
    assert!(
        matches!(match_box(&arena, inner), BoxMatch::Seq(_, _)),
        "inner fad expression should be evaluated to a flat first-order box"
    );
}

#[test]
fn eval_process_preserves_reverse_ad_wrapper_around_evaluated_body() {
    let parsed = parse_program(
        "p = hslider(\"p\", 0, -1, 1, 0.01); process = rad(hslider(\"f\", 1, 0, 10, 0.1) : sin, p);",
        "eval_rad_wrapper.dsp",
    );
    assert!(
        parsed.errors.is_empty(),
        "parser should accept rad fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");

    let out = eval_process(&mut arena, root).expect("rad process should evaluate");
    let BoxMatch::ReverseAD(body, seeds) = match_box(&arena, out) else {
        panic!("expected eval to preserve BOXRAUTODIFF as a two-child node");
    };
    assert!(
        matches!(match_box(&arena, body), BoxMatch::Seq(_, _)),
        "rad body should be evaluated to a flat first-order box"
    );
    assert!(
        matches!(match_box(&arena, seeds), BoxMatch::HSlider(_, _, _, _, _)),
        "rad seeds child should be evaluated to its UI shape"
    );
}

#[test]
fn eval_process_accepts_parser_lowered_letrec_without_eval_fallback() {
    let source = r#"
        process = y letrec {
            'y = (x - s) * G + s;
            's = 2 * (x - s) * G + s;
        } with {
            x = _;
            G = 0.5;
        };
    "#;
    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept letrec fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");

    let out = eval_process(&mut arena, root)
        .expect("parser-lowered letrec should evaluate without eval fallback");
    assert!(!matches!(
        match_box(&arena, out),
        BoxMatch::WithRecDef(_, _, _)
    ));
}

#[test]
fn eval_process_rejects_legacy_with_rec_def_nodes() {
    let mut arena = TreeArena::new();
    let wire = make_wire(&mut arena);
    let nil = arena.nil();
    let tag = arena.intern_tag("BOXWITHRECDEF");
    let legacy = arena.intern(NodeKind::Tag(tag), &[wire, nil, nil]);
    let process = make_def(&mut arena, "process", nil, legacy);
    let root = make_defs(&mut arena, &[process]);

    let err = eval_process(&mut arena, root).expect_err("legacy BOXWITHRECDEF should be rejected");
    let EvalError::InternalError { message } = err else {
        panic!("expected InternalError for legacy BOXWITHRECDEF");
    };
    assert!(message.contains("BOXWITHRECDEF"));
}

#[test]
fn eval_process_keeps_multi_output_parallel_add_network_structured() {
    let source = r#"
        process = (0,0,1,1) : par(i,2,+);
    "#;
    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept multi-output fold fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");

    let out =
        eval_process(&mut arena, root).expect("multi-output seq/par composition should evaluate");
    assert!(
        !matches!(match_box(&arena, out), BoxMatch::Int(_) | BoxMatch::Real(_)),
        "multi-output network must not collapse to a scalar constant"
    );
}

#[test]
fn eval_process_route_spec_lowers_computed_connections_like_cpp() {
    let source = r#"
        process = 0.3,-0.2,0.8,-0.5
            : route(4,4, ((0,0,1,1,2,2,3,3),(1,1,1,1,1,1,1,1)) : par(i,8,+));
    "#;
    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept route fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let ctx = EvalSourceContext::memory();

    let out = eval_process_with_source_context(&mut arena, root, ctx)
        .expect("route spec should evaluate after a2sb lowering");
    assert!(
        !matches!(match_box(&arena, out), BoxMatch::Int(_) | BoxMatch::Real(_)),
        "computed route specifications must not collapse to a scalar constant"
    );
}

#[test]
fn eval_process_component_loads_file_in_captured_source_context() {
    let root_dir = temp_root("component_source_context");
    let entry = root_dir.join("main.dsp");
    let child = root_dir.join("child.dsp");
    fs::write(&entry, "process = component(\"child.dsp\");\n").expect("write entry");
    fs::write(&child, "process = _;\n").expect("write child");

    let mut arena = TreeArena::new();
    let child_name = arena.string_lit("child.dsp");
    let component = BoxBuilder::new(&mut arena).component(child_name);
    let nil = arena.nil();
    let process = make_def(&mut arena, "process", nil, component);
    let root = make_defs(&mut arena, &[process]);
    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let out = eval_process_with_source_context(&mut arena, root, ctx)
        .expect("component should load child file");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn eval_process_component_reuses_cached_loaded_source_in_same_context() {
    let root_dir = temp_root("component_source_cache");
    let entry = root_dir.join("main.dsp");
    let child = root_dir.join("child_cached.dsp");
    fs::write(&entry, "process = component(\"child_cached.dsp\");\n").expect("write entry");
    fs::write(&child, "process = _;\n").expect("write child");

    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let mut arena_first = TreeArena::new();
    let child_name = arena_first.string_lit("child_cached.dsp");
    let component = BoxBuilder::new(&mut arena_first).component(child_name);
    let nil = arena_first.nil();
    let process = make_def(&mut arena_first, "process", nil, component);
    let root = make_defs(&mut arena_first, &[process]);
    let first = eval_process_with_source_context(&mut arena_first, root, ctx.clone())
        .expect("first component load should succeed");
    assert!(matches!(match_box(&arena_first, first), BoxMatch::Wire));

    fs::remove_file(&child).expect("remove cached child file");

    let mut arena_second = TreeArena::new();
    let child_name = arena_second.string_lit("child_cached.dsp");
    let component = BoxBuilder::new(&mut arena_second).component(child_name);
    let nil = arena_second.nil();
    let process = make_def(&mut arena_second, "process", nil, component);
    let root = make_defs(&mut arena_second, &[process]);
    let second = eval_process_with_source_context(&mut arena_second, root, ctx)
        .expect("second component load should reuse cached source");
    assert!(matches!(match_box(&arena_second, second), BoxMatch::Wire));
}

#[test]
fn eval_process_library_loads_environment_from_file() {
    let root_dir = temp_root("library_source_context");
    let entry = root_dir.join("main.dsp");
    let child = root_dir.join("child_lib.dsp");
    fs::write(
        &entry,
        "lib = library(\"child_lib.dsp\"); process = lib.a;\n",
    )
    .expect("write entry");
    fs::write(&child, "a = _;\n").expect("write child");

    let mut arena = TreeArena::new();
    let child_name = arena.string_lit("child_lib.dsp");
    let library = BoxBuilder::new(&mut arena).library(child_name);
    let nil = arena.nil();
    let lib_def = make_def(&mut arena, "lib", nil, library);
    let lib_ident = make_ident(&mut arena, "lib");
    let a_ident = make_ident(&mut arena, "a");
    let access = BoxBuilder::new(&mut arena).access(lib_ident, a_ident);
    let process = make_def(&mut arena, "process", nil, access);
    let root = make_defs(&mut arena, &[lib_def, process]);
    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let out =
        eval_process_with_source_context(&mut arena, root, ctx).expect("library should load child");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn eval_source_context_collects_top_level_metadata_from_loaded_component() {
    let root_dir = temp_root("component_metadata");
    let entry = root_dir.join("main.dsp");
    let child = root_dir.join("child.dsp");
    fs::write(&entry, "process = component(\"child.dsp\");\n").expect("write entry");
    fs::write(&child, "declare author \"child-author\";\nprocess = _;\n").expect("write child");

    let mut arena = TreeArena::new();
    let child_name = arena.string_lit("child.dsp");
    let component = BoxBuilder::new(&mut arena).component(child_name);
    let nil = arena.nil();
    let process = make_def(&mut arena, "process", nil, component);
    let root = make_defs(&mut arena, &[process]);
    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let out = eval_process_with_source_context(&mut arena, root, ctx.clone())
        .expect("component should load child and collect metadata");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));

    let key = CompilationMetadataKey::scoped(
        child
            .canonicalize()
            .expect("child should canonicalize")
            .to_string_lossy()
            .into_owned(),
        "author",
    );
    let snapshot = ctx.metadata_snapshot();
    let values = snapshot
        .entries()
        .get(&key)
        .expect("loaded component metadata should be aggregated");
    assert!(values.contains("child-author"));
}

#[test]
fn eval_component_load_accepts_inline_environment_import_in_loaded_file() {
    let root_dir = temp_root("component_inline_environment_import");
    let entry = root_dir.join("main.dsp");
    let child = root_dir.join("child.dsp");
    let voice = root_dir.join("voice.lib");
    fs::write(&entry, "process = component(\"child.dsp\");\n").expect("write entry");
    fs::write(
        &child,
        "GEN = environment { import(\"voice.lib\"); }.process;\nprocess = GEN;\n",
    )
    .expect("write child");
    fs::write(&voice, "process = _;\n").expect("write voice");

    let mut arena = TreeArena::new();
    let child_name = arena.string_lit("child.dsp");
    let component = BoxBuilder::new(&mut arena).component(child_name);
    let nil = arena.nil();
    let process = make_def(&mut arena, "process", nil, component);
    let root = make_defs(&mut arena, &[process]);
    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let out = eval_process_with_source_context(&mut arena, root, ctx)
        .expect("component should load child with inline environment import");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn eval_library_load_accepts_inline_environment_import_in_loaded_file() {
    let root_dir = temp_root("library_inline_environment_import");
    let entry = root_dir.join("main.dsp");
    let child = root_dir.join("child_lib.dsp");
    let voice = root_dir.join("voice.lib");
    fs::write(
        &entry,
        "lib = library(\"child_lib.dsp\"); process = lib.a;\n",
    )
    .expect("write entry");
    fs::write(
        &child,
        "A = environment { import(\"voice.lib\"); }.process;\na = A;\n",
    )
    .expect("write child");
    fs::write(&voice, "process = _;\n").expect("write voice");

    let mut arena = TreeArena::new();
    let child_name = arena.string_lit("child_lib.dsp");
    let library = BoxBuilder::new(&mut arena).library(child_name);
    let nil = arena.nil();
    let lib_def = make_def(&mut arena, "lib", nil, library);
    let lib_ident = make_ident(&mut arena, "lib");
    let a_ident = make_ident(&mut arena, "a");
    let access = BoxBuilder::new(&mut arena).access(lib_ident, a_ident);
    let process = make_def(&mut arena, "process", nil, access);
    let root = make_defs(&mut arena, &[lib_def, process]);
    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let out = eval_process_with_source_context(&mut arena, root, ctx)
        .expect("library should load child with inline environment import");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn eval_process_case_supports_incremental_partial_application() {
    let mut arena = TreeArena::new();
    let one = BoxBuilder::new(&mut arena).int(1);
    let two = BoxBuilder::new(&mut arena).int(2);
    let x = make_ident(&mut arena, "x");
    let y = make_ident(&mut arena, "y");
    let px = BoxBuilder::new(&mut arena).pattern_var(x);
    let py = BoxBuilder::new(&mut arena).pattern_var(y);
    let lhs = make_rev_list2(&mut arena, px, py);
    let rule = arena.cons(lhs, y);
    let rules = make_rules_parser_order(&mut arena, &[rule]);
    let case_expr = BoxBuilder::new(&mut arena).case(rules);

    let arg1 = arena.cons(one, arena.nil());
    let partial = BoxBuilder::new(&mut arena).appl(case_expr, arg1);
    let arg2 = arena.cons(two, arena.nil());
    let process_expr = BoxBuilder::new(&mut arena).appl(partial, arg2);
    let nil = arena.nil();
    let process = make_def(&mut arena, "process", nil, process_expr);
    let root = make_defs(&mut arena, &[process]);

    let out = eval_process(&mut arena, root).expect("curried case application should succeed");
    expect_int(&arena, out, 2);
}

#[test]
fn eval_box_resolves_with_local_scope() {
    let mut arena = TreeArena::new();
    let wire = make_wire(&mut arena);
    let nil = arena.nil();
    let local_def = make_def(&mut arena, "a", nil, wire);
    let locals = make_defs(&mut arena, &[local_def]);
    let a_ident = make_ident(&mut arena, "a");
    let expr = BoxBuilder::new(&mut arena).with_local_def(a_ident, locals);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("with local should resolve");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn environment_push_scope_assigns_stable_ids_and_preserves_parent_lookup() {
    let mut arena = TreeArena::new();
    let sym_x = arena.intern_symbol("x");
    let wire = make_wire(&mut arena);

    let mut root = Environment::empty();
    let root_id = root.id();
    root.bind(sym_x, wire);

    let child = root.push_scope();
    let child_id = child.id();
    let barrier = child.push_barrier_scope();
    let barrier_id = barrier.id();

    assert_ne!(root_id, child_id, "child scope should get a fresh EnvId");
    assert_ne!(
        child_id, barrier_id,
        "barrier scope should get its own stable EnvId"
    );
    assert_eq!(child.lookup(sym_x), Some(wire));
    assert_eq!(barrier.lookup(sym_x), Some(wire));
    assert_eq!(
        barrier.lookup_until_barrier(sym_x),
        None,
        "barrier lookup should stop before reaching the parent binding"
    );
}

#[test]
fn environment_child_scopes_preserve_source_context() {
    let ctx = EvalSourceContext::for_file(
        std::path::Path::new("/tmp/main.dsp"),
        &[PathBuf::from("/tmp/imports")],
    );
    let root = Environment::empty_with_source_context(ctx.clone());
    let child = root.push_scope();
    let barrier = child.push_barrier_scope();

    assert_eq!(root.source_context(), &ctx);
    assert_eq!(child.source_context(), &ctx);
    assert_eq!(barrier.source_context(), &ctx);
}

#[test]
fn eval_process_reports_missing_process() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let foo_wire = make_wire(&mut arena);
    let foo = make_def(&mut arena, "foo", nil, foo_wire);
    let root = make_defs(&mut arena, &[foo]);
    let err = eval_process(&mut arena, root).expect_err("missing process should fail");
    assert!(matches!(
        err,
        EvalError::MissingProcessDefinition { definitions, .. } if definitions == root
    ));
}

#[test]
fn eval_process_detects_recursive_loop() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let process_ident = make_ident(&mut arena, "process");
    let foo = make_def(&mut arena, "foo", nil, process_ident);
    let foo_ident = make_ident(&mut arena, "foo");
    let process = make_def(&mut arena, "process", nil, foo_ident);
    let root = make_defs(&mut arena, &[foo, process]);

    let err = eval_process(&mut arena, root).expect_err("recursive cycle should fail");
    assert!(matches!(err, EvalError::LoopDetected { .. }));
}

#[test]
fn eval_process_applies_function_arguments_in_cpp_order() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let x = make_ident(&mut arena, "x");
    let y = make_ident(&mut arena, "y");
    let params_rev = make_rev_list2(&mut arena, x, y); // parser-style reverse list: [y, x]
    let body = {
        let par = BoxBuilder::new(&mut arena).par(x, y);
        let add = BoxBuilder::new(&mut arena).add();
        BoxBuilder::new(&mut arena).seq(par, add)
    };
    let foo = make_def(&mut arena, "foo", params_rev, body);

    let one = BoxBuilder::new(&mut arena).int(1);
    let two = BoxBuilder::new(&mut arena).int(2);
    let args_rev = make_rev_list2(&mut arena, one, two); // parser-style reverse list: [2, 1]
    let process_expr = {
        let foo_ident = make_ident(&mut arena, "foo");
        BoxBuilder::new(&mut arena).appl(foo_ident, args_rev)
    };
    let process = make_def(&mut arena, "process", nil, process_expr);
    let root = make_defs(&mut arena, &[foo, process]);

    let out = eval_process(&mut arena, root).expect("application should evaluate");
    // Step 7 (C++ parity): seq(par(int(1), int(2)), add) folds to int(3) at
    // eval time via `isNumericalTuple` + `boxPropagateSig` + `simplify` in C++.
    // Previously this test expected the un-folded Seq; now it should fold.
    match match_box(&arena, out) {
        BoxMatch::Int(3) => {
            // Correctly folded: foo(1,2) = 1+2 = 3
        }
        BoxMatch::Seq(lhs, rhs) => {
            // Fallback: not folded — verify argument order is correct.
            assert!(matches!(match_box(&arena, rhs), BoxMatch::Add));
            let (a, b) = match match_box(&arena, lhs) {
                BoxMatch::Par(a, b) => (a, b),
                other => panic!("expected BOXPAR, got {other:?}"),
            };
            expect_int(&arena, a, 1);
            expect_int(&arena, b, 2);
        }
        other => panic!("expected Int(3) or Seq(Par(1,2), Add), got {other:?}"),
    }
}

#[test]
fn eval_process_forces_definitions_in_their_captured_scope() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();

    let one = BoxBuilder::new(&mut arena).int(1);
    let root_x = make_def(&mut arena, "x", nil, one);

    let x_ident = make_ident(&mut arena, "x");
    let f = make_def(&mut arena, "f", nil, x_ident);

    let two = BoxBuilder::new(&mut arena).int(2);
    let local_x = make_def(&mut arena, "x", nil, two);
    let locals = make_defs(&mut arena, &[local_x]);
    let f_ident = make_ident(&mut arena, "f");
    let process_expr = BoxBuilder::new(&mut arena).with_local_def(f_ident, locals);
    let process = make_def(&mut arena, "process", nil, process_expr);

    let root = make_defs(&mut arena, &[root_x, f, process]);
    let out = eval_process(&mut arena, root).expect("captured definition should use outer scope");
    expect_int(&arena, out, 1);
}

#[test]
fn eval_process_applies_abstractions_in_their_captured_scope() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();

    let one = BoxBuilder::new(&mut arena).int(1);
    let root_y = make_def(&mut arena, "y", nil, one);

    let x_ident = make_ident(&mut arena, "x");
    let y_ident = make_ident(&mut arena, "y");
    let params_rev = arena.cons(x_ident, nil);
    let f = make_def(&mut arena, "f", params_rev, y_ident);

    let zero = BoxBuilder::new(&mut arena).int(0);
    let args = arena.cons(zero, nil);
    let f_ident = make_ident(&mut arena, "f");
    let app = BoxBuilder::new(&mut arena).appl(f_ident, args);
    let two = BoxBuilder::new(&mut arena).int(2);
    let local_y = make_def(&mut arena, "y", nil, two);
    let locals = make_defs(&mut arena, &[local_y]);
    let process_expr = BoxBuilder::new(&mut arena).with_local_def(app, locals);
    let process = make_def(&mut arena, "process", nil, process_expr);

    let root = make_defs(&mut arena, &[root_y, f, process]);
    let out =
        eval_process(&mut arena, root).expect("abstraction application should use captured scope");
    expect_int(&arena, out, 1);
}

#[test]
fn eval_process_access_uses_captured_environment_scope() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();

    let env_box = BoxBuilder::new(&mut arena).environment();
    let one = BoxBuilder::new(&mut arena).int(1);
    let env_a = make_def(&mut arena, "a", nil, one);
    let env_defs = make_defs(&mut arena, &[env_a]);
    let env_value = BoxBuilder::new(&mut arena).with_local_def(env_box, env_defs);
    let e = make_def(&mut arena, "e", nil, env_value);

    let field = make_ident(&mut arena, "a");
    let e_ident = make_ident(&mut arena, "e");
    let access = BoxBuilder::new(&mut arena).access(e_ident, field);

    let two = BoxBuilder::new(&mut arena).int(2);
    let local_a = make_def(&mut arena, "a", nil, two);
    let locals = make_defs(&mut arena, &[local_a]);
    let process_expr = BoxBuilder::new(&mut arena).with_local_def(access, locals);
    let process = make_def(&mut arena, "process", nil, process_expr);

    let root = make_defs(&mut arena, &[e, process]);
    let out = eval_process(&mut arena, root).expect("access should resolve through captured env");
    expect_int(&arena, out, 1);
}

#[test]
fn eval_box_access_on_non_closure_reports_error() {
    let mut arena = TreeArena::new();
    let wire = make_wire(&mut arena);
    let ident = make_ident(&mut arena, "a");
    let expr = BoxBuilder::new(&mut arena).access(wire, ident);

    let mut loop_detector = LoopDetector::new();
    let err = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect_err("access on plain box value should fail");
    assert!(matches!(
        err,
        EvalError::ExpectedClosureValue {
            context: "access",
            ..
        }
    ));
}

#[test]
fn eval_process_modif_local_def_rewrites_enclosed_closure_envs() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();

    let one = BoxBuilder::new(&mut arena).int(1);
    let env_a = make_def(&mut arena, "a", nil, one);
    let a_ident = make_ident(&mut arena, "a");
    let env_b = make_def(&mut arena, "b", nil, a_ident);
    let env_defs = make_defs(&mut arena, &[env_a, env_b]);
    let env_box = BoxBuilder::new(&mut arena).environment();
    let env_value = BoxBuilder::new(&mut arena).with_local_def(env_box, env_defs);
    let e = make_def(&mut arena, "e", nil, env_value);

    let two = BoxBuilder::new(&mut arena).int(2);
    let replacement_a = make_def(&mut arena, "a", nil, two);
    let replacements = make_defs(&mut arena, &[replacement_a]);
    let e_ident = make_ident(&mut arena, "e");
    let rewritten = BoxBuilder::new(&mut arena).modif_local_def(e_ident, replacements);
    let b_ident = make_ident(&mut arena, "b");
    let process_expr = BoxBuilder::new(&mut arena).access(rewritten, b_ident);
    let process = make_def(&mut arena, "process", nil, process_expr);

    let root = make_defs(&mut arena, &[e, process]);
    let out =
        eval_process(&mut arena, root).expect("rewritten env should update enclosed closures");
    expect_int(&arena, out, 2);
}

#[test]
fn eval_process_modif_local_def_rewrites_abstraction_capture() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();

    let one = BoxBuilder::new(&mut arena).int(1);
    let root_y = make_def(&mut arena, "y", nil, one);

    let x_ident = make_ident(&mut arena, "x");
    let y_ident = make_ident(&mut arena, "y");
    let params_rev = arena.cons(x_ident, nil);
    let f = make_def(&mut arena, "f", params_rev, y_ident);

    let two = BoxBuilder::new(&mut arena).int(2);
    let replacement_y = make_def(&mut arena, "y", nil, two);
    let replacements = make_defs(&mut arena, &[replacement_y]);
    let f_ident = make_ident(&mut arena, "f");
    let rewritten = BoxBuilder::new(&mut arena).modif_local_def(f_ident, replacements);
    let zero = BoxBuilder::new(&mut arena).int(0);
    let args = arena.cons(zero, nil);
    let process_expr = BoxBuilder::new(&mut arena).appl(rewritten, args);
    let process = make_def(&mut arena, "process", nil, process_expr);

    let root = make_defs(&mut arena, &[root_y, f, process]);
    let out = eval_process(&mut arena, root)
        .expect("rewritten abstraction should use the copied captured environment");
    expect_int(&arena, out, 2);
}

#[test]
fn eval_process_modif_local_def_replacement_defs_capture_current_scope() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();

    let ten = BoxBuilder::new(&mut arena).int(10);
    let root_a = make_def(&mut arena, "a", nil, ten);

    let one = BoxBuilder::new(&mut arena).int(1);
    let env_a = make_def(&mut arena, "a", nil, one);
    let wire = make_wire(&mut arena);
    let env_b = make_def(&mut arena, "b", nil, wire);
    let env_defs = make_defs(&mut arena, &[env_a, env_b]);
    let env_box = BoxBuilder::new(&mut arena).environment();
    let env_value = BoxBuilder::new(&mut arena).with_local_def(env_box, env_defs);
    let e = make_def(&mut arena, "e", nil, env_value);

    let a_ident = make_ident(&mut arena, "a");
    let replacement_b = make_def(&mut arena, "b", nil, a_ident);
    let replacements = make_defs(&mut arena, &[replacement_b]);
    let e_ident = make_ident(&mut arena, "e");
    let rewritten = BoxBuilder::new(&mut arena).modif_local_def(e_ident, replacements);
    let b_ident = make_ident(&mut arena, "b");
    let access_b = BoxBuilder::new(&mut arena).access(rewritten, b_ident);

    let twenty = BoxBuilder::new(&mut arena).int(20);
    let local_a = make_def(&mut arena, "a", nil, twenty);
    let locals = make_defs(&mut arena, &[local_a]);
    let process_expr = BoxBuilder::new(&mut arena).with_local_def(access_b, locals);
    let process = make_def(&mut arena, "process", nil, process_expr);

    let root = make_defs(&mut arena, &[root_a, e, process]);
    let out = eval_process(&mut arena, root)
        .expect("replacement defs should capture the current rewrite scope");
    expect_int(&arena, out, 20);
}

#[test]
fn eval_box_non_closure_application_falls_back_to_seq_par() {
    let mut arena = TreeArena::new();
    let one = BoxBuilder::new(&mut arena).int(1);
    let two = BoxBuilder::new(&mut arena).int(2);
    let args_rev = make_rev_list2(&mut arena, one, two); // [2,1]
    let add = BoxBuilder::new(&mut arena).add();
    let expr = BoxBuilder::new(&mut arena).appl(add, args_rev);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("non-closure apply should lower to seq(par(args), fun)");
    let (lhs, rhs) = match match_box(&arena, out) {
        BoxMatch::Seq(lhs, rhs) => (lhs, rhs),
        other => panic!("expected BOXSEQ, got {other:?}"),
    };
    assert!(matches!(match_box(&arena, rhs), BoxMatch::Add));
    let (a, b) = match match_box(&arena, lhs) {
        BoxMatch::Par(a, b) => (a, b),
        other => panic!("expected BOXPAR, got {other:?}"),
    };
    expect_int(&arena, a, 1);
    expect_int(&arena, b, 2);
}

#[test]
fn eval_box_non_closure_partial_binary_primitive_prepends_missing_wire() {
    let mut arena = TreeArena::new();
    let half = BoxBuilder::new(&mut arena).real(0.5);
    let nil = arena.nil();
    let args = arena.cons(half, nil);
    let mul = BoxBuilder::new(&mut arena).mul();
    let expr = BoxBuilder::new(&mut arena).appl(mul, args);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("partial binary primitive should insert missing wire");
    let (lhs, rhs) = match match_box(&arena, out) {
        BoxMatch::Seq(lhs, rhs) => (lhs, rhs),
        other => panic!("expected BOXSEQ, got {other:?}"),
    };
    assert!(matches!(match_box(&arena, rhs), BoxMatch::Mul));
    let (a, b) = match match_box(&arena, lhs) {
        BoxMatch::Par(a, b) => (a, b),
        other => panic!("expected BOXPAR, got {other:?}"),
    };
    assert!(matches!(match_box(&arena, a), BoxMatch::Wire));
    assert!(matches!(match_box(&arena, b), BoxMatch::Real(v) if (v - 0.5).abs() < f64::EPSILON));
}

#[test]
fn eval_box_partial_binary_primitive_uses_single_output_fallback_for_symbolic_argument() {
    let source = r#"
upfront(x) = (x-x') > 0.0;
decay(n,x) = x - (x>0.0)/n;
release(n) = + ~ decay(n);
trigger(n) = upfront : release(n) : >(0.0);
process = *(button("play") : trigger(128));
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept trigger partial-application repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("partial mul with symbolic trigger argument should evaluate");

    let (lhs, rhs) = match match_box(&arena, out) {
        BoxMatch::Seq(lhs, rhs) => (lhs, rhs),
        other => panic!("expected BOXSEQ, got {other:?}"),
    };
    assert!(matches!(match_box(&arena, rhs), BoxMatch::Mul));
    let (a, b) = match match_box(&arena, lhs) {
        BoxMatch::Par(a, b) => (a, b),
        other => panic!("expected BOXPAR, got {other:?}"),
    };
    assert!(
        matches!(match_box(&arena, a), BoxMatch::Wire),
        "under-applied mul should prepend an implicit wire"
    );
    assert!(
        matches!(match_box(&arena, b), BoxMatch::Seq(_, _)),
        "the explicit trigger argument should remain the second mul operand"
    );
}

#[test]
fn eval_box_non_closure_application_counts_residual_closure_outputs() {
    let source = r#"
compressor_stereo(ratio,thresh,att,rel,x,y) = cgm*x, cgm*y with {
  cgm = abs(x) + abs(y);
};
displaygain = _,_ <: _,_,(abs,abs:+) : _,_,_ : _,attach;
process = displaygain(compressor_stereo(5,-30,0.01,0.1));
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept closure-arity repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("residual closure argument should not receive an extra implicit wire");

    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("evaluated process should remain well-typed");
    assert_eq!(arity.inputs, 2);
    assert_eq!(arity.outputs, 2);
}

#[test]
fn eval_process_iterative_count_accepts_inputs_of_residual_closure_like_cpp() {
    let source = r#"
f(n,x) = x,x;
g = par(i, inputs(f(1)), _);
process = g;
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept inputs(closure) iteration repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("inputs(residual closure) should reduce through a2sb like C++");

    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("evaluated process should remain well-typed");
    assert_eq!(arity.inputs, 1);
    assert_eq!(arity.outputs, 1);
}

#[test]
fn eval_process_applies_rdtable_waveform_argument_without_extra_wire() {
    let source = r#"
process = rdtable(waveform{2,3,5,7}, 1);
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept rdtable waveform repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("rdtable(waveform, x) should not receive an extra implicit wire");

    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("evaluated process should remain well-typed");
    assert_eq!(arity.inputs, 0);
    assert_eq!(arity.outputs, 1);
}

#[test]
fn eval_process_keeps_large_waveform_nodes_as_leaf_values() {
    let mut source = String::from("process = rdtable(waveform{");
    for i in 0..2048 {
        if i > 0 {
            source.push(',');
        }
        source.push_str(&i.to_string());
    }
    source.push_str("}, 1);");

    let parsed = parse_program(&source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept large waveform repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("large waveform should stay in normal form during eval");

    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("evaluated large waveform process should remain well-typed");
    assert_eq!(arity.inputs, 0);
    assert_eq!(arity.outputs, 1);
}

#[test]
fn eval_process_seq_zero_iteration_uses_cpp_neutral_bus_identity() {
    let source = r#"
foo(O,N) = bar(O,N) with {
  bar(N) = baz(1), _, *(-1.0);
  baz(i) = seq(j,N-1-i,_);
};
process = foo(3,2);
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept seq-zero neutral repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("seq(j,0,body) should lower to the C++ neutral identity bus");

    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("evaluated process should remain well-typed");
    assert_eq!(arity.inputs, 2);
    assert_eq!(arity.outputs, 3);
}

#[test]
fn eval_process_reuses_residual_case_argument_inside_local_abstraction() {
    let source = r#"
poly(1,x)=11;
poly(6,x)=x*x;
foo(v) = bar(v) with { bar(x)=x-x; };
process = foo(poly(6))(3);
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept residual case reuse repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("reused residual case argument should evaluate like Faust C++");
    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("shared residual argument should still lower to a valid box");
    assert_eq!(arity.inputs, 1);
    assert_eq!(arity.outputs, 1);
}

#[test]
fn eval_process_counts_residual_selectbus_outputs_via_a2sb_like_cpp() {
    let source = r#"
selectnX(N,i,sel) = S(N,0) with {
  S(1,offset) = _;
  S(n,offset) = S(left, offset), S(right, offset+left) : sel(i, offset+left)
  with {
    right = int(n/2);
    left = n-right;
  };
};
selectn(N,i) = selectnX(N,i,selector) with {
  selector(i,j,x,y) = select2((i >= j), x, y);
};
interleave(1,2) = _,_;
interleave(R,C) = route(R*C, R*C, par(i, R*C, (i+1, (i%R)*C + int(i/R) + 1)));
selectbus(BUS_SIZE, NUM_BUSES, id) =
  interleave(BUS_SIZE, NUM_BUSES) : par(j, BUS_SIZE, selectn(NUM_BUSES, id));
dot(N) = interleave(N,2) : par(i,N,*) :> _;
process = dot(2, selectbus(2,2,0), selectbus(2,2,1));
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept selectbus/dot repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("residual symbolic selectbus arguments should count outputs via a2sb");

    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("evaluated process should remain well-typed");
    assert_eq!(arity.inputs, 8);
    assert_eq!(arity.outputs, 1);
}

#[test]
fn eval_process_matches_exact_integer_real_case_arguments_like_cpp() {
    let source = r#"
comparatorDes(x0, x1) = max(x0, x1) , min(x0, x1);
comparatorAsc(x0, x1) = min(x0, x1) , max(x0, x1);
comparator(0, x0, x1) = comparatorDes(x0, x1);
comparator(1, x0, x1) = comparatorAsc(x0, x1);
dir(i) = (i % (2 ^ 1) < (2 ^ 0));
process = comparator(dir(0));
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept exact-integer-real case repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("exact integer reals should match integer case constants like Faust C++");

    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("evaluated comparator should stay well-typed");
    assert_eq!(arity.inputs, 2);
    assert_eq!(arity.outputs, 2);
}

#[test]
fn eval_process_preserves_parser_case_rule_priority_for_recursive_local_cases() {
    let source = r#"
process = p(0, 0, 0.0) with {
  p(l, m, cphi) = pcalcul(
      ((l != 0) & (l == abs(m)))
    + ((l != 0) & (l == abs(m)+1)) * 2
    + ((l != 0) & (l != abs(m)) & (l != abs(m)+1)) * 3,
    l, m, cphi)
  with {
    pcalcul(0, l, m, cphi) = 1;
    pcalcul(1, l, m, cphi) = -1 * (2 * (l-1) + 1) * sqrt(1 - cphi*cphi) * p(l-1, l-1, cphi);
    pcalcul(2, l, m, cphi) = cphi * (2 * (l-1) + 1) * p(l-1, l-1, cphi);
    pcalcul(s, l, m, cphi) = (cphi * (2 * (l-1) + 1) * p(l-1, abs(m), cphi) - ((l-1) + abs(m)) * p(l-2, abs(m), cphi)) / ((l-1) - abs(m) + 1);
  };
};
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept recursive local case priority fixture: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("parser-lowered case rules should match in source order before catch-all rules");
    expect_int(&arena, out, 1);
}

#[test]
fn eval_process_parser_case_rule_priority_matches_cpp_reference() {
    let root_dir = temp_root("parser_case_rule_priority_cpp_parity");
    let entry = root_dir.join("main.dsp");
    let source = r#"
process = p(0, 0, 0.0) with {
  p(l, m, cphi) = pcalcul(
      ((l != 0) & (l == abs(m)))
    + ((l != 0) & (l == abs(m)+1)) * 2
    + ((l != 0) & (l != abs(m)) & (l != abs(m)+1)) * 3,
    l, m, cphi)
  with {
    pcalcul(0, l, m, cphi) = 1;
    pcalcul(1, l, m, cphi) = -1 * (2 * (l-1) + 1) * sqrt(1 - cphi*cphi) * p(l-1, l-1, cphi);
    pcalcul(2, l, m, cphi) = cphi * (2 * (l-1) + 1) * p(l-1, l-1, cphi);
    pcalcul(s, l, m, cphi) = (cphi * (2 * (l-1) + 1) * p(l-1, abs(m), cphi) - ((l-1) + abs(m)) * p(l-2, abs(m), cphi)) / ((l-1) - abs(m) + 1);
  };
};
"#;
    fs::write(&entry, source).expect("write entry");

    if let Some(cpp) = cpp_bin() {
        let out_path = root_dir.join("main.c");
        let cpp_ok = cpp_compiles_file(&cpp, &entry, &out_path)
            .unwrap_or_else(|e| panic!("C++ run failed for parser case rule priority parity: {e}"));
        assert!(
            cpp_ok,
            "C++ reference should compile recursive local case priority fixture"
        );
    }

    let parsed = parse_file_with_imports(&entry, std::slice::from_ref(&root_dir))
        .expect("file-backed parse should succeed");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept recursive local case priority parity fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let out = eval_process_with_source_context(&mut arena, root, ctx)
        .expect("Rust eval should accept same recursive local case priority fixture as C++");
    expect_int(&arena, out, 1);
}

#[test]
fn eval_process_accepts_numeric_seq_as_iterator_count() {
    let source = r#"
count = max(1, 0);
process = par(i, count, _):>_;
"#;

    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept iterator-count repro: {:?}",
        parsed.errors
    );

    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let out = eval_process(&mut arena, root)
        .expect("numeric iterator count should evaluate through eval2int-like folding");
    let flat_out = propagate::try_build_flat_box(&arena, out).expect("eval output should be flat");
    let arity = propagate::box_arity_typed(&arena, flat_out, &mut ArityCache::new())
        .expect("iterator-count repro should lower to a valid box");
    assert_eq!(arity.inputs, 1);
    assert_eq!(arity.outputs, 1);
}

#[test]
fn eval_box_non_closure_partial_prefix_appends_missing_wire() {
    let mut arena = TreeArena::new();
    let zero = BoxBuilder::new(&mut arena).int(0);
    let nil = arena.nil();
    let args = arena.cons(zero, nil);
    let prefix = BoxBuilder::new(&mut arena).prefix();
    let expr = BoxBuilder::new(&mut arena).appl(prefix, args);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("partial prefix should insert missing wire");
    let (lhs, rhs) = match match_box(&arena, out) {
        BoxMatch::Seq(lhs, rhs) => (lhs, rhs),
        other => panic!("expected BOXSEQ, got {other:?}"),
    };
    assert!(matches!(match_box(&arena, rhs), BoxMatch::Prefix));
    let (a, b) = match match_box(&arena, lhs) {
        BoxMatch::Par(a, b) => (a, b),
        other => panic!("expected BOXPAR, got {other:?}"),
    };
    expect_int(&arena, a, 0);
    assert!(matches!(match_box(&arena, b), BoxMatch::Wire));
}

#[test]
fn eval_box_non_closure_application_reports_too_many_arguments() {
    let mut arena = TreeArena::new();
    let one = BoxBuilder::new(&mut arena).int(1);
    let two = BoxBuilder::new(&mut arena).int(2);
    let three = BoxBuilder::new(&mut arena).int(3);
    let args_rev = make_rev_list3(&mut arena, one, two, three); // [3,2,1]
    let add = BoxBuilder::new(&mut arena).add();
    let expr = BoxBuilder::new(&mut arena).appl(add, args_rev);

    let mut loop_detector = LoopDetector::new();
    let err = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect_err("add with 3 arguments should fail");
    assert_eq!(
        err,
        EvalError::TooManyArguments {
            node: add,
            expected: 2,
            got: 3
        }
    );
}

#[test]
fn eval_box_access_reads_environment_local_binding() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();

    let env_box = BoxBuilder::new(&mut arena).environment();
    let wire = make_wire(&mut arena);
    let a_def = make_def(&mut arena, "a", nil, wire);
    let defs = make_defs(&mut arena, &[a_def]);
    let env_with_defs = BoxBuilder::new(&mut arena).with_local_def(env_box, defs);
    let field = make_ident(&mut arena, "a");
    let expr = BoxBuilder::new(&mut arena).access(env_with_defs, field);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("access should resolve from local environment");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn eval_iterative_par_expands_with_index_binding() {
    let mut arena = TreeArena::new();
    let i = make_ident(&mut arena, "i");
    let three = BoxBuilder::new(&mut arena).int(3);
    let expr = BoxBuilder::new(&mut arena).ipar(i, three, i);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("ipar should expand");
    let (a0, r1) = match match_box(&arena, out) {
        BoxMatch::Par(a0, r1) => (a0, r1),
        other => panic!("expected top BOXPAR, got {other:?}"),
    };
    expect_int(&arena, a0, 0);
    let (a1, a2) = match match_box(&arena, r1) {
        BoxMatch::Par(a1, a2) => (a1, a2),
        other => panic!("expected second BOXPAR, got {other:?}"),
    };
    expect_int(&arena, a1, 1);
    expect_int(&arena, a2, 2);
}

#[test]
fn eval_iterative_sum_builds_add_chain() {
    let mut arena = TreeArena::new();
    let i = make_ident(&mut arena, "i");
    let three = BoxBuilder::new(&mut arena).int(3);
    let expr = BoxBuilder::new(&mut arena).isum(i, three, i);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("isum should expand");
    assert_eq!(count_add_nodes(&arena, out), 2);
}

#[test]
fn eval_case_uses_source_rule_priority() {
    let mut arena = TreeArena::new();
    let p0 = BoxBuilder::new(&mut arena).int(0);
    let r1 = BoxBuilder::new(&mut arena).int(1);
    let x_ident = make_ident(&mut arena, "x");
    let px = BoxBuilder::new(&mut arena).pattern_var(x_ident);
    let r2 = BoxBuilder::new(&mut arena).int(2);
    let rule1 = make_rule1(&mut arena, p0, r1); // source first
    let rule2 = make_rule1(&mut arena, px, r2); // source second
    let rules = make_rules_parser_order(&mut arena, &[rule1, rule2]);
    let case_expr = BoxBuilder::new(&mut arena).case(rules);
    let arg0 = BoxBuilder::new(&mut arena).int(0);
    let nil = arena.nil();
    let args = arena.cons(arg0, nil);
    let expr = BoxBuilder::new(&mut arena).appl(case_expr, args);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("case application should match first source rule");
    expect_int(&arena, out, 1);
}

#[test]
fn eval_case_pattern_var_binds_argument() {
    let mut arena = TreeArena::new();
    let x_ident = make_ident(&mut arena, "x");
    let px = BoxBuilder::new(&mut arena).pattern_var(x_ident);
    let rhs = make_ident(&mut arena, "x");
    let rule = make_rule1(&mut arena, px, rhs);
    let rules = make_rules_parser_order(&mut arena, &[rule]);
    let case_expr = BoxBuilder::new(&mut arena).case(rules);
    let arg = BoxBuilder::new(&mut arena).int(7);
    let nil = arena.nil();
    let args = arena.cons(arg, nil);
    let expr = BoxBuilder::new(&mut arena).appl(case_expr, args);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("pattern var should bind");
    expect_int(&arena, out, 7);
}

#[test]
fn eval_case_evaluates_numeric_patterns_before_matching() {
    let mut arena = TreeArena::new();
    let one_a = BoxBuilder::new(&mut arena).int(1);
    let one_b = BoxBuilder::new(&mut arena).int(1);
    let plus_inputs = BoxBuilder::new(&mut arena).par(one_a, one_b);
    let plus = BoxBuilder::new(&mut arena).add();
    let numeric_pattern = BoxBuilder::new(&mut arena).seq(plus_inputs, plus);
    let rhs = BoxBuilder::new(&mut arena).int(7);
    let rule = make_rule1(&mut arena, numeric_pattern, rhs);
    let rules = make_rules_parser_order(&mut arena, &[rule]);
    let case_expr = BoxBuilder::new(&mut arena).case(rules);
    let arg = BoxBuilder::new(&mut arena).int(2);
    let nil = arena.nil();
    let args = arena.cons(arg, nil);
    let expr = BoxBuilder::new(&mut arena).appl(case_expr, args);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &Environment::empty(), &mut loop_detector)
        .expect("numeric pattern should be simplified before matching");
    expect_int(&arena, out, 7);
}

#[test]
fn eval_case_pattern_var_lookup_stops_at_barrier_scope() {
    let mut arena = TreeArena::new();
    let outer_x = BoxBuilder::new(&mut arena).int(1);
    let sym_x = arena.intern_symbol("x");
    let mut env = Environment::empty();
    env.bind(sym_x, outer_x);

    let x_ident = make_ident(&mut arena, "x");
    let px = BoxBuilder::new(&mut arena).pattern_var(x_ident);
    let rhs = make_ident(&mut arena, "x");
    let rule = make_rule1(&mut arena, px, rhs);
    let rules = make_rules_parser_order(&mut arena, &[rule]);
    let case_expr = BoxBuilder::new(&mut arena).case(rules);
    let arg = BoxBuilder::new(&mut arena).int(2);
    let nil = arena.nil();
    let args = arena.cons(arg, nil);
    let expr = BoxBuilder::new(&mut arena).appl(case_expr, args);

    let mut loop_detector = LoopDetector::new();
    let out = eval_box(&mut arena, expr, &env, &mut loop_detector)
        .expect("pattern-variable matching should ignore outer bindings");
    expect_int(&arena, out, 2);
}

#[test]
fn eval_process_preserves_tupled_higher_order_locals_for_case_matching() {
    let source = r#"
        foo((a, b)) = 123;
        process = foo((res1, res2))
        with {
            res1(i) = i;
            res2(i) = i + 1;
        };
    "#;
    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept higher-order tuple fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");

    let out = eval_process(&mut arena, root)
        .expect("tupled higher-order locals should remain matchable in case patterns");
    expect_int(&arena, out, 123);
}

#[test]
fn eval_process_tupled_higher_order_locals_match_cpp_reference() {
    let root_dir = temp_root("tupled_higher_order_cpp_parity");
    let entry = root_dir.join("main.dsp");
    let child = root_dir.join("child_lib.dsp");
    fs::write(
        &child,
        r#"
            resistor =
            case {
                (2, R) => R0
                with {
                    R0 = R;
                };
            };
        "#,
    )
    .expect("write child library");
    let source = r#"
        wd = library("child_lib.dsp");
        foo((a, b)) = 123;
        process = foo((res1, res2))
        with {
            res1(i) = wd.resistor(i, 1000);
            res2(i) = wd.resistor(i, 2000);
        };
    "#;
    fs::write(&entry, source).expect("write entry");

    if let Some(cpp) = cpp_bin() {
        let out_path = root_dir.join("main.c");
        let cpp_ok = cpp_compiles_file(&cpp, &entry, &out_path)
            .unwrap_or_else(|e| panic!("C++ run failed for tupled higher-order eval parity: {e}"));
        assert!(
            cpp_ok,
            "C++ reference should compile tupled higher-order local closure case"
        );
    }

    let parsed = parse_file_with_imports(&entry, std::slice::from_ref(&root_dir))
        .expect("file-backed parse should succeed");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept higher-order tuple parity fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let out = eval_process_with_source_context(&mut arena, root, ctx)
        .expect("Rust eval should accept same higher-order tuple case as C++");
    expect_int(&arena, out, 123);
}

#[test]
fn eval_process_preserves_unapplied_local_case_environment_through_higher_order_call() {
    let source = r#"
        apply_twice(f) = f(1, 1);
        process = apply_twice(transformer_scatter)
        with {
            n = 2;
            transformer_scatter(R0, R1) = matrix(2, 2, s)
            with {
                s(1,1) = -1*(R0-n^2*R1)/(R0+n^2*R1);
                s(1,2) = (2*n*R0*R1)/(R0+n^2*R1);
                s(2,1) = (2*n*R0*R1)/(R0+n^2*R1);
                s(2,2) = (R0-n^2*R1)/(R0+n^2*R1);
                s(i,j) = 10;
            };
            matrix(M,N,f) = par(n, N, par(m, M, *(f(m+1, n+1))));
        };
    "#;
    let parsed = parse_program(source, "<memory>");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept higher-order local case fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");

    let out = eval_process(&mut arena, root)
        .expect("unapplied local case values should keep captured env when passed higher-order");
    assert!(matches!(match_box(&arena, out), BoxMatch::Par(_, _)));
}

#[test]
fn eval_process_unapplied_local_case_higher_order_matches_cpp_reference() {
    let root_dir = temp_root("local_case_higher_order_cpp_parity");
    let entry = root_dir.join("main.dsp");
    let source = r#"
        apply_twice(f) = f(1, 1);
        process = apply_twice(transformer_scatter)
        with {
            n = 2;
            transformer_scatter(R0, R1) = matrix(2, 2, s)
            with {
                s(1,1) = -1*(R0-n^2*R1)/(R0+n^2*R1);
                s(1,2) = (2*n*R0*R1)/(R0+n^2*R1);
                s(2,1) = (2*n*R0*R1)/(R0+n^2*R1);
                s(2,2) = (R0-n^2*R1)/(R0+n^2*R1);
                s(i,j) = 10;
            };
            matrix(M,N,f) = par(n, N, par(m, M, *(f(m+1, n+1))));
        };
    "#;
    fs::write(&entry, source).expect("write entry");

    if let Some(cpp) = cpp_bin() {
        let out_path = root_dir.join("main.c");
        let cpp_ok = cpp_compiles_file(&cpp, &entry, &out_path).unwrap_or_else(|e| {
            panic!("C++ run failed for local case higher-order eval parity: {e}")
        });
        assert!(
            cpp_ok,
            "C++ reference should compile higher-order local case closure case"
        );
    }

    let parsed = parse_file_with_imports(&entry, std::slice::from_ref(&root_dir))
        .expect("file-backed parse should succeed");
    assert!(
        parsed.errors.is_empty(),
        "parser should accept local case higher-order parity fixture: {:?}",
        parsed.errors
    );
    let mut arena = parsed.state.arena;
    let root = parsed.root.expect("parse should return a root");
    let ctx = EvalSourceContext::for_file(&entry, std::slice::from_ref(&root_dir));

    let out = eval_process_with_source_context(&mut arena, root, ctx)
        .expect("Rust eval should accept same higher-order local case as C++");
    assert!(matches!(match_box(&arena, out), BoxMatch::Par(_, _)));
}

#[test]
fn eval_case_under_application_preserves_residual_case_and_no_match_still_errors() {
    let mut arena = TreeArena::new();
    let x_ident = make_ident(&mut arena, "x");
    let y_ident = make_ident(&mut arena, "y");
    let x = BoxBuilder::new(&mut arena).pattern_var(x_ident);
    let y = BoxBuilder::new(&mut arena).pattern_var(y_ident);
    let lhs_rev = make_rev_list2(&mut arena, x, y); // parser-style reverse list for (x, y)
    let rhs = BoxBuilder::new(&mut arena).int(99);
    let rule = arena.cons(lhs_rev, rhs);
    let rules = make_rules_parser_order(&mut arena, &[rule]);
    let case_expr = BoxBuilder::new(&mut arena).case(rules);
    let one = BoxBuilder::new(&mut arena).int(1);
    let nil = arena.nil();
    let args_one = arena.cons(one, nil);
    let expr_arity = BoxBuilder::new(&mut arena).appl(case_expr, args_one);

    let mut loop_detector = LoopDetector::new();
    let lowered = eval_box(
        &mut arena,
        expr_arity,
        &Environment::empty(),
        &mut loop_detector,
    )
    .expect("under-applied case should preserve a residual value");
    // After the boxPatternMatcher side-table change, a partially-applied PM
    // is stored as a boxPatternMatcher(key) node (not the original case_expr).
    assert!(
        matches!(match_box(&arena, lowered), BoxMatch::PatternMatcher(_)),
        "eval_box should produce a boxPatternMatcher for a partially-applied case"
    );

    // No-match branch: (0) => 1 applied to 2.
    let p0 = BoxBuilder::new(&mut arena).int(0);
    let r1 = BoxBuilder::new(&mut arena).int(1);
    let one_rule = make_rule1(&mut arena, p0, r1);
    let one_rules = make_rules_parser_order(&mut arena, &[one_rule]);
    let case_no_match = BoxBuilder::new(&mut arena).case(one_rules);
    let two = BoxBuilder::new(&mut arena).int(2);
    let nil2 = arena.nil();
    let args_two = arena.cons(two, nil2);
    let expr_no_match = BoxBuilder::new(&mut arena).appl(case_no_match, args_two);

    let mut loop_detector2 = LoopDetector::new();
    let err2 = eval_box(
        &mut arena,
        expr_no_match,
        &Environment::empty(),
        &mut loop_detector2,
    )
    .expect_err("no matching rule should fail");
    assert!(matches!(err2, EvalError::PatternMatchFailed { .. }));
}

#[test]
fn eval_error_converts_to_structured_diagnostic_codes() {
    let arena = TreeArena::new();
    let missing = EvalError::MissingProcessDefinition {
        entrypoint: "process".to_owned(),
        definitions: arena.nil(),
        available_defs: vec!["foo".to_owned()],
    }
    .into_diagnostic();
    assert_eq!(missing.severity, Severity::Error);
    assert_eq!(missing.stage, Stage::Eval);
    assert_eq!(missing.code, codes::EVAL_MISSING_PROCESS);
    assert!(!missing.help.is_empty());

    let missing_custom = EvalError::MissingProcessDefinition {
        entrypoint: "dsp".to_owned(),
        definitions: arena.nil(),
        available_defs: vec!["foo".to_owned()],
    }
    .into_diagnostic();
    assert!(
        missing_custom.message.contains("missing `dsp` definition"),
        "custom entrypoint message should mention the requested name"
    );

    let undef = EvalError::UndefinedSymbol {
        symbol: "foo".to_owned(),
        node: arena.nil(),
        local_scope: vec!["x".to_owned()],
        visible_scope: vec!["x".to_owned(), "y".to_owned()],
        top_level_scope: vec!["y".to_owned()],
    }
    .into_diagnostic();
    assert_eq!(undef.code, codes::EVAL_UNDEFINED_SYMBOL);
    assert!(
        undef
            .notes
            .iter()
            .any(|n| n.starts_with("cause: unresolved identifier")),
        "undefined-symbol diagnostics should expose explicit cause note"
    );

    let iter = EvalError::NegativeIterationCount { value: -1 }.into_diagnostic();
    assert_eq!(iter.code, codes::EVAL_ITERATION_INVALID);
    assert!(!iter.help.is_empty());
    assert!(
        iter.notes
            .iter()
            .any(|n| n.starts_with("cause: iterative combinator count")),
        "iteration diagnostics should expose explicit cause note"
    );

    let arity = EvalError::TooManyArguments {
        node: arena.nil(),
        expected: 2,
        got: 3,
    }
    .into_diagnostic();
    assert_eq!(arity.code, codes::EVAL_ARITY_MISMATCH);
    assert!(!arity.notes.is_empty());

    let redef = EvalError::RedefinedSymbol {
        symbol: "x".to_owned(),
        first_def: arena.nil(),
        second_def: arena.nil(),
    }
    .into_diagnostic();
    assert_eq!(redef.code, codes::EVAL_REDEFINED_SYMBOL);
    assert_eq!(redef.severity, Severity::Error);
    assert!(
        redef
            .notes
            .iter()
            .any(|n| n.starts_with("cause: the same symbol is bound twice")),
        "redefinition diagnostic should expose explicit cause note"
    );
    assert!(
        !redef.help.is_empty(),
        "redefinition diagnostic should suggest a fix"
    );
}

// ── RedefinedSymbol tests ─────────────────────────────────────────────────────

/// C++ parity: `addLayerDef` in `environment.cpp` throws when the same name is bound to a
/// different definition in the same layer.
/// Rust equivalent: `bind_definitions` returns `EvalError::RedefinedSymbol`.
#[test]
fn bind_definitions_rejects_conflicting_redefinition_in_same_scope() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    // Two definitions of "x" with different values in the same definition list
    let wire = BoxBuilder::new(&mut arena).wire();
    let cut = BoxBuilder::new(&mut arena).cut();
    let def_x1 = make_def(&mut arena, "x", nil, wire);
    let def_x2 = make_def(&mut arena, "x", nil, cut); // ← different value: cut ≠ wire
    let process_ident = make_ident(&mut arena, "x");
    let def_process = make_def(&mut arena, "process", nil, process_ident);
    let defs = make_defs(&mut arena, &[def_x1, def_x2, def_process]);

    let err = eval_process(&mut arena, defs).expect_err("conflicting redefinition should fail");
    assert!(
        matches!(
            err,
            EvalError::RedefinedSymbol { ref symbol, .. } if symbol == "x"
        ),
        "expected RedefinedSymbol for `x`, got {err:?}"
    );
}

/// C++ parity: `addLayerDef` silently accepts identical redefinitions (same expression by
/// structural equality / hash-consing).
/// Rust equivalent: `bind_definitions` silently skips the duplicate.
#[test]
fn bind_definitions_accepts_identical_redefinition_silently() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    // Two definitions of "x" with the SAME value (same TreeId = same hash-consed node)
    let wire = BoxBuilder::new(&mut arena).wire();
    let def_x1 = make_def(&mut arena, "x", nil, wire);
    let def_x2 = make_def(&mut arena, "x", nil, wire); // ← identical TreeId
    let process_ident = make_ident(&mut arena, "x");
    let def_process = make_def(&mut arena, "process", nil, process_ident);
    let defs = make_defs(&mut arena, &[def_x1, def_x2, def_process]);

    // Should succeed: identical re-binding is silently accepted
    let out =
        eval_process(&mut arena, defs).expect("identical redefinition should be silently accepted");
    assert!(
        matches!(match_box(&arena, out), BoxMatch::Wire),
        "resolved definition should be the wire node"
    );
}

/// Shadowing: a name defined in an outer scope may be redefined in a nested `with {}` scope.
/// This should NOT produce a `RedefinedSymbol` error — it is standard lexical shadowing.
#[test]
fn bind_definitions_allows_shadowing_from_outer_scope() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    // Outer: x = wire. Inner with: x = cut. Body resolves to inner x.
    let wire = BoxBuilder::new(&mut arena).wire();
    let cut = BoxBuilder::new(&mut arena).cut();
    let def_x_outer = make_def(&mut arena, "x", nil, wire);

    // build: x with { x = cut } — inner x shadows outer x
    let inner_def = make_def(&mut arena, "x", nil, cut);
    let inner_defs = make_defs(&mut arena, &[inner_def]);
    let x_ident = make_ident(&mut arena, "x");
    let with_expr = BoxBuilder::new(&mut arena).with_local_def(x_ident, inner_defs);
    let def_process = make_def(&mut arena, "process", nil, with_expr);

    let defs = make_defs(&mut arena, &[def_x_outer, def_process]);
    let out =
        eval_process(&mut arena, defs).expect("shadowing should not produce a redefinition error");
    assert!(
        matches!(match_box(&arena, out), BoxMatch::Cut),
        "inner `x = cut` should shadow outer `x = wire`"
    );
}

// ── EvalStats tests ───────────────────────────────────────────────────────────

/// `eval_process_with_stats` should return the same result as `eval_process` plus stats with
/// at least one env_lookup (the `process` lookup itself) and at least one layer pushed.
#[test]
fn eval_process_with_stats_returns_consistent_result_and_stats() {
    let mut arena = TreeArena::new();
    let wire = BoxBuilder::new(&mut arena).wire();
    let nil = arena.nil();
    let def_process = make_def(&mut arena, "process", nil, wire);
    let defs = make_defs(&mut arena, &[def_process]);

    let (out, stats) =
        eval_process_with_stats(&mut arena, defs).expect("eval_process_with_stats should succeed");
    assert!(
        matches!(match_box(&arena, out), BoxMatch::Wire),
        "result should be the wire node"
    );
    assert!(
        stats.env_lookups >= 1,
        "at least the `process` lookup should be counted, got {}",
        stats.env_lookups
    );
    assert!(
        stats.env_layers_pushed >= 1,
        "at least the root scope should be counted, got {}",
        stats.env_layers_pushed
    );
}

#[test]
fn eval_process_lowers_residual_abstraction_to_symbolic_box() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let x = make_ident(&mut arena, "x");
    let lambda = BoxBuilder::new(&mut arena).abstr(x, x);
    let def_process = make_def(&mut arena, "process", nil, lambda);
    let defs = make_defs(&mut arena, &[def_process]);

    let out = eval_process(&mut arena, defs).expect("residual abstraction should lower via a2sb");
    let BoxMatch::Symbolic(slot, body) = match_box(&arena, out) else {
        panic!("expected symbolic box after a2sb lowering");
    };
    assert_eq!(
        body, slot,
        "identity lambda should lower to symbolic(slot, slot)"
    );
    assert!(matches!(match_box(&arena, slot), BoxMatch::Slot(_)));
}

#[test]
fn eval_process_lowers_residual_case_to_symbolic_box() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let x = make_ident(&mut arena, "x");
    let px = BoxBuilder::new(&mut arena).pattern_var(x);
    let rule = make_rule1(&mut arena, px, x);
    let rules = make_rules_parser_order(&mut arena, &[rule]);
    let case_expr = BoxBuilder::new(&mut arena).case(rules);
    let def_process = make_def(&mut arena, "process", nil, case_expr);
    let defs = make_defs(&mut arena, &[def_process]);

    let out = eval_process(&mut arena, defs).expect("residual case should lower via a2sb");
    let BoxMatch::Symbolic(slot, body) = match_box(&arena, out) else {
        panic!("expected symbolic box after case lowering");
    };
    assert_eq!(
        body, slot,
        "identity case should lower to symbolic(slot, slot)"
    );
    assert!(matches!(match_box(&arena, slot), BoxMatch::Slot(_)));
}

#[test]
fn eval_process_modulation_without_matching_widget_leaves_body_unchanged() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let label = arena.string_lit("gain");
    let modulation_var = arena.cons(label, nil);
    let wire = make_wire(&mut arena);
    let modulation = BoxBuilder::new(&mut arena).modulation(modulation_var, wire);
    let def_process = make_def(&mut arena, "process", nil, modulation);
    let defs = make_defs(&mut arena, &[def_process]);

    let out = eval_process(&mut arena, defs).expect("modulation should evaluate");
    assert!(matches!(match_box(&arena, out), BoxMatch::Wire));
}

#[test]
fn eval_process_modulation_implants_default_mul_around_matching_slider() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let label = arena.string_lit("gain");
    let modulation_var = arena.cons(label, nil);
    let slider = {
        let mut b = BoxBuilder::new(&mut arena);
        let cur = b.real(0.5);
        let min = b.real(0.0);
        let max = b.real(1.0);
        let step = b.real(0.01);
        b.hslider(label, cur, min, max, step)
    };
    let modulation = BoxBuilder::new(&mut arena).modulation(modulation_var, slider);
    let def_process = make_def(&mut arena, "process", nil, modulation);
    let defs = make_defs(&mut arena, &[def_process]);

    let out = eval_process(&mut arena, defs).expect("matching modulation should evaluate");
    let BoxMatch::Symbolic(slot, body) = match_box(&arena, out) else {
        panic!("default modulation should produce a symbolic wrapper");
    };
    let BoxMatch::Seq(pair, mul) = match_box(&arena, body) else {
        panic!("matching modulation should sequence par(widget, slot) into mul");
    };
    assert!(matches!(match_box(&arena, mul), BoxMatch::Mul));
    let BoxMatch::Par(widget, slot_ref) = match_box(&arena, pair) else {
        panic!("modulated widget should be paired with slot");
    };
    assert_eq!(
        slot_ref, slot,
        "slot used in par(widget, slot) should match wrapper slot"
    );
    assert!(matches!(
        match_box(&arena, widget),
        BoxMatch::HSlider(_, _, _, _, _)
    ));
}

#[test]
fn eval_process_widget_label_substitutes_ident_placeholders() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let three = BoxBuilder::new(&mut arena).int(3);
    let n_def = make_def(&mut arena, "n", nil, three);
    let label = arena.string_lit("gain%n");
    let slider = {
        let mut b = BoxBuilder::new(&mut arena);
        let cur = b.real(0.5);
        let min = b.real(0.0);
        let max = b.real(1.0);
        let step = b.real(0.01);
        b.hslider(label, cur, min, max, step)
    };
    let process_def = make_def(&mut arena, "process", nil, slider);
    let defs = make_defs(&mut arena, &[n_def, process_def]);

    let out = eval_process(&mut arena, defs).expect("label interpolation should evaluate");
    let BoxMatch::HSlider(label, _, _, _, _) = match_box(&arena, out) else {
        panic!("process should evaluate to hslider");
    };
    expect_label(&arena, label, "gain3");
}

#[test]
fn eval_process_widget_label_applies_clamped_field_width() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let three = BoxBuilder::new(&mut arena).int(3);
    let n_def = make_def(&mut arena, "n", nil, three);
    let label = arena.string_lit("gain%9n");
    let slider = {
        let mut b = BoxBuilder::new(&mut arena);
        let cur = b.real(0.5);
        let min = b.real(0.0);
        let max = b.real(1.0);
        let step = b.real(0.01);
        b.hslider(label, cur, min, max, step)
    };
    let process_def = make_def(&mut arena, "process", nil, slider);
    let defs = make_defs(&mut arena, &[n_def, process_def]);

    let out = eval_process(&mut arena, defs).expect("label interpolation should evaluate");
    let BoxMatch::HSlider(label, _, _, _, _) = match_box(&arena, out) else {
        panic!("process should evaluate to hslider");
    };
    expect_label(&arena, label, "gain   3");
}

#[test]
fn eval_process_widget_label_truncates_real_placeholder_like_cpp() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let att = BoxBuilder::new(&mut arena).real(0.001);
    let att_def = make_def(&mut arena, "att", nil, att);
    let label = arena.string_lit("attack:%att");
    let slider = {
        let mut b = BoxBuilder::new(&mut arena);
        let cur = b.real(0.5);
        let min = b.real(0.0);
        let max = b.real(1.0);
        let step = b.real(0.01);
        b.hslider(label, cur, min, max, step)
    };
    let process_def = make_def(&mut arena, "process", nil, slider);
    let defs = make_defs(&mut arena, &[att_def, process_def]);

    let out = eval_process(&mut arena, defs).expect("real placeholder should truncate like C++");
    let BoxMatch::HSlider(label, _, _, _, _) = match_box(&arena, out) else {
        panic!("process should evaluate to hslider");
    };
    expect_label(&arena, label, "attack:0");
}

#[test]
fn eval_process_widget_label_keeps_malformed_percent_sequence_literal() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let label = arena.string_lit("gain%/");
    let slider = {
        let mut b = BoxBuilder::new(&mut arena);
        let cur = b.real(0.5);
        let min = b.real(0.0);
        let max = b.real(1.0);
        let step = b.real(0.01);
        b.hslider(label, cur, min, max, step)
    };
    let process_def = make_def(&mut arena, "process", nil, slider);
    let defs = make_defs(&mut arena, &[process_def]);

    let out = eval_process(&mut arena, defs).expect("malformed placeholder should stay literal");
    let BoxMatch::HSlider(label, _, _, _, _) = match_box(&arena, out) else {
        panic!("process should evaluate to hslider");
    };
    expect_label(&arena, label, "gain%/");
}

#[test]
fn eval_process_widget_label_undefined_placeholder_surfaces_eval_error() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let label = arena.string_lit("gain%n");
    let slider = {
        let mut b = BoxBuilder::new(&mut arena);
        let cur = b.real(0.5);
        let min = b.real(0.0);
        let max = b.real(1.0);
        let step = b.real(0.01);
        b.hslider(label, cur, min, max, step)
    };
    let process_def = make_def(&mut arena, "process", nil, slider);
    let defs = make_defs(&mut arena, &[process_def]);

    let err = eval_process(&mut arena, defs).expect_err("undefined label placeholder should fail");
    assert!(matches!(err, EvalError::UndefinedSymbol { .. }));
}

#[test]
fn eval_process_group_label_substitutes_ident_placeholders() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let three = BoxBuilder::new(&mut arena).int(3);
    let n_def = make_def(&mut arena, "n", nil, three);
    let group_label = arena.string_lit("main%n");
    let inner = make_wire(&mut arena);
    let group = BoxBuilder::new(&mut arena).vgroup(group_label, inner);
    let process_def = make_def(&mut arena, "process", nil, group);
    let defs = make_defs(&mut arena, &[n_def, process_def]);

    let out = eval_process(&mut arena, defs).expect("group label interpolation should evaluate");
    let BoxMatch::VGroup(label, _) = match_box(&arena, out) else {
        panic!("process should evaluate to vgroup");
    };
    expect_label(&arena, label, "main3");
}

#[test]
fn eval_process_modulation_target_label_substitutes_placeholders() {
    let mut arena = TreeArena::new();
    let nil = arena.nil();
    let three = BoxBuilder::new(&mut arena).int(3);
    let n_def = make_def(&mut arena, "n", nil, three);
    let mod_label = arena.string_lit("gain%n");
    let modulation_var = arena.cons(mod_label, nil);
    let slider_label = arena.string_lit("gain3");
    let slider = {
        let mut b = BoxBuilder::new(&mut arena);
        let cur = b.real(0.5);
        let min = b.real(0.0);
        let max = b.real(1.0);
        let step = b.real(0.01);
        b.hslider(slider_label, cur, min, max, step)
    };
    let modulation = BoxBuilder::new(&mut arena).modulation(modulation_var, slider);
    let process_def = make_def(&mut arena, "process", nil, modulation);
    let defs = make_defs(&mut arena, &[n_def, process_def]);

    let out = eval_process(&mut arena, defs).expect("matching modulation should evaluate");
    let BoxMatch::Symbolic(_, body) = match_box(&arena, out) else {
        panic!("default modulation should produce a symbolic wrapper");
    };
    let BoxMatch::Seq(pair, _) = match_box(&arena, body) else {
        panic!("modulated widget should sequence into mul");
    };
    let BoxMatch::Par(widget, _) = match_box(&arena, pair) else {
        panic!("modulated widget should be paired with slot");
    };
    let BoxMatch::HSlider(label, _, _, _, _) = match_box(&arena, widget) else {
        panic!("expected modulated hslider");
    };
    expect_label(&arena, label, "gain3");
}
