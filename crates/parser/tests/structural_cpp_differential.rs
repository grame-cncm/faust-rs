//! Structural parser differential checks against C++ acceptance on representative cases.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use boxes::{BoxMatch, dump_box, match_box};
use parser::{parse_file, parse_program};
use tlib::{NodeKind, TreeArena, TreeId};

fn list_head(arena: &TreeArena, list: TreeId) -> TreeId {
    arena.hd(list).expect("list must be non-empty")
}

fn list_tail(arena: &TreeArena, list: TreeId) -> TreeId {
    arena.tl(list).expect("list tail must exist")
}

fn definition_expr(arena: &TreeArena, def: TreeId) -> TreeId {
    let payload = list_tail(arena, def);
    list_tail(arena, payload)
}

fn definition_name(arena: &TreeArena, def: TreeId) -> String {
    let name = list_head(arena, def);
    match match_box(arena, name) {
        BoxMatch::Ident(text) => text.to_owned(),
        other => panic!("expected definition name ident, got {:?}", other),
    }
}

fn case_rules(arena: &TreeArena, expr: TreeId) -> TreeId {
    match match_box(arena, expr) {
        BoxMatch::Case(rules) => rules,
        other => panic!("expected BOXCASE, got {:?}", other),
    }
}

fn string_like_text(arena: &TreeArena, id: TreeId) -> Option<&str> {
    match arena.kind(id) {
        Some(NodeKind::Symbol(text)) => Some(text.as_ref()),
        Some(NodeKind::StringLiteral(text)) => Some(text.as_ref()),
        _ => None,
    }
}

fn rule_first_pattern(arena: &TreeArena, rule: TreeId) -> TreeId {
    let lhs = list_head(arena, rule);
    list_head(arena, lhs)
}

fn collect_rule_patterns(arena: &TreeArena, mut rules: TreeId) -> Vec<TreeId> {
    let mut out = Vec::new();
    while !arena.is_nil(rules) {
        let rule = list_head(arena, rules);
        out.push(rule_first_pattern(arena, rule));
        rules = list_tail(arena, rules);
    }
    out
}

fn find_definition_expr(arena: &TreeArena, mut defs: TreeId, expected: &str) -> Option<TreeId> {
    while !arena.is_nil(defs) {
        let def = arena.hd(defs)?;
        if definition_name(arena, def) == expected {
            return Some(definition_expr(arena, def));
        }
        defs = arena.tl(defs)?;
    }
    None
}

fn collect_import_files(arena: &TreeArena, mut defs: TreeId) -> Vec<String> {
    let mut out = Vec::new();
    while !arena.is_nil(defs) {
        let Some(def) = arena.hd(defs) else {
            break;
        };
        if let BoxMatch::ImportFile(filename) = match_box(arena, def)
            && let Some(text) = string_like_text(arena, filename)
        {
            out.push(text.to_owned());
        }
        defs = arena.tl(defs).unwrap_or_else(|| arena.nil());
    }
    out
}

fn count_definitions_named(arena: &TreeArena, mut defs: TreeId, expected: &str) -> usize {
    let mut count = 0usize;
    while !arena.is_nil(defs) {
        let Some(def) = arena.hd(defs) else {
            break;
        };
        if definition_name(arena, def) == expected {
            count = count.saturating_add(1);
        }
        defs = arena.tl(defs).unwrap_or_else(|| arena.nil());
    }
    count
}

fn parse_process_expr(source: &str, source_name: &str) -> (TreeArena, TreeId) {
    let output = parse_program(source, source_name);
    assert!(
        output.errors.is_empty(),
        "unexpected parse errors for {}: {:?}",
        source_name,
        output.errors
    );
    let root = output.root.expect("root should be present");
    let def = list_head(&output.state.arena, root);
    let expr = definition_expr(&output.state.arena, def);
    (output.state.arena, expr)
}

fn cpp_bin() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FAUST_CPP_BIN") {
        return Some(PathBuf::from(path));
    }
    let default = PathBuf::from("/usr/local/bin/faust");
    default.exists().then_some(default)
}

fn cpp_accepts_source(cpp_bin: &Path, source: &str, case_name: &str) -> Result<bool, String> {
    let mut input_path = std::env::temp_dir();
    input_path.push(format!(
        "faust_rs_parser_structural_{}_{}.dsp",
        std::process::id(),
        case_name
    ));
    let mut out_path = std::env::temp_dir();
    out_path.push(format!(
        "faust_rs_parser_structural_{}_{}.c",
        std::process::id(),
        case_name
    ));
    fs::write(&input_path, source).map_err(|e| format!("write input failed: {e}"))?;

    let output = Command::new(cpp_bin)
        .arg(&input_path)
        .arg("-lang")
        .arg("c")
        .arg("-o")
        .arg(&out_path)
        .output()
        .map_err(|e| format!("failed to run {}: {e}", cpp_bin.display()))?;

    let _ = fs::remove_file(&input_path);
    let _ = fs::remove_file(&out_path);
    Ok(output.status.success())
}

#[test]
fn production_parser_structural_shapes_align_with_cpp_acceptance() {
    let cases = [
        ("prec", "process = 1 + 2 * 3;"),
        ("unary", "process = -1;"),
        ("appl", "process = abs(1);"),
    ];

    let cpp = cpp_bin();
    for (name, source) in cases {
        if let Some(cpp_bin) = &cpp {
            let cpp_ok = cpp_accepts_source(cpp_bin, source, name)
                .unwrap_or_else(|e| panic!("C++ run failed for {name}: {e}"));
            assert!(cpp_ok, "C++ should accept structural case {name}");
        }

        let (arena, expr) = parse_process_expr(source, &format!("structural_{name}.dsp"));
        match name {
            "prec" => {
                let (lhs, rhs) = match match_box(&arena, expr) {
                    BoxMatch::Seq(lhs, rhs) => (lhs, rhs),
                    other => panic!("expected top BOXSEQ for prec, got {:?}", other),
                };
                assert!(matches!(match_box(&arena, rhs), BoxMatch::Add));
                let (one, mul_chain) = match match_box(&arena, lhs) {
                    BoxMatch::Par(one, mul_chain) => (one, mul_chain),
                    other => panic!("expected BOXPAR for prec lhs, got {:?}", other),
                };
                assert!(matches!(arena.kind(one), Some(NodeKind::Int(1))));
                let (_mul_inputs, mul_op) = match match_box(&arena, mul_chain) {
                    BoxMatch::Seq(mul_inputs, mul_op) => (mul_inputs, mul_op),
                    other => panic!("expected mul chain BOXSEQ, got {:?}", other),
                };
                assert!(matches!(match_box(&arena, mul_op), BoxMatch::Mul));
            }
            "unary" => match match_box(&arena, expr) {
                BoxMatch::Seq(_lhs, rhs) => {
                    assert!(matches!(match_box(&arena, rhs), BoxMatch::Sub));
                }
                BoxMatch::Int(v) => {
                    assert_eq!(v, -1);
                }
                other => panic!("expected unary lowering shape, got {:?}", other),
            },
            "appl" => {
                let (_callee, args) = match match_box(&arena, expr) {
                    BoxMatch::Appl(callee, args) => (callee, args),
                    other => panic!("expected BOXAPPL, got {:?}", other),
                };
                assert!(
                    !arena.is_nil(args),
                    "application must carry at least one argument"
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn production_parser_recognizes_fad_and_rad_like_cpp() {
    let (arena_fad, fad_expr) = parse_process_expr(
        "process = fad(hslider(\"freq\", 440, 50, 2000, 0.01) : sin, hslider(\"freq\", 440, 50, 2000, 0.01));",
        "fad_structural.dsp",
    );
    let BoxMatch::ForwardAD(fad_inner, _fad_seed) = match_box(&arena_fad, fad_expr) else {
        panic!("expected BOXFAUTODIFF shape");
    };
    assert!(matches!(
        match_box(&arena_fad, fad_inner),
        BoxMatch::Seq(_, _)
    ));

    let (arena_rad, rad_expr) = parse_process_expr(
        "process = rad(hslider(\"freq\", 440, 50, 2000, 0.01) : sin, hslider(\"p\", 0, -1, 1, 0.01));",
        "rad_structural.dsp",
    );
    let BoxMatch::ReverseAD(rad_body, _rad_seeds) = match_box(&arena_rad, rad_expr) else {
        panic!("expected BOXRAUTODIFF shape");
    };
    assert!(matches!(
        match_box(&arena_rad, rad_body),
        BoxMatch::Seq(_, _)
    ));
}

#[test]
fn production_parser_reinjects_definition_metadata_like_cpp() {
    let source = r#"
        declare foo author "Alice";
        foo = _;
        process = foo;
    "#;

    let output = parse_program(source, "metadata_definition.dsp");
    assert!(
        output.errors.is_empty(),
        "unexpected parse errors: {:?}",
        output.errors
    );

    let root = output.root.expect("root should be present");
    let foo_expr = find_definition_expr(&output.state.arena, root, "foo")
        .expect("foo definition should exist");
    let (body, md_pair) = match match_box(&output.state.arena, foo_expr) {
        BoxMatch::Metadata(body, md_pair) => (body, md_pair),
        other => panic!("expected BOXMETADATA wrapper on foo, got {:?}", other),
    };
    assert!(matches!(
        match_box(&output.state.arena, body),
        BoxMatch::Wire
    ));

    let key = list_head(&output.state.arena, md_pair);
    let value = list_tail(&output.state.arena, md_pair);
    assert_eq!(
        string_like_text(&output.state.arena, key),
        Some("metadata_definition.dsp/foo:author")
    );
    assert_eq!(string_like_text(&output.state.arena, value), Some("Alice"));
}

#[test]
fn production_parser_expands_letrec_to_with_local_def_like_cpp() {
    let source = r#"
        process = y letrec {
            'y = (x - s) * G + s;
            's = 2 * (x - s) * G + s;
        } with {
            x = _;
            G = 0.5;
        };
    "#;

    let (arena, expr) = parse_process_expr(source, "structural_letrec.dsp");
    let dumped = dump_box(&arena, expr);
    assert!(
        !dumped.contains("BOXWITHRECDEF"),
        "parser should not leave BOXWITHRECDEF on the production path: {dumped}"
    );
}

#[test]
fn production_parser_import_heavy_fixture_matches_cpp_acceptance() {
    let root = {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "faust_rs_parser_structural_imports_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should move forward")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp root should be created");
        path
    };

    let main = root.join("src/main.dsp");
    let lib = root.join("libs/gain.lib");
    let core = root.join("libs/core/base.lib");
    fs::create_dir_all(main.parent().expect("main parent")).expect("create src");
    fs::create_dir_all(lib.parent().expect("lib parent")).expect("create libs");
    fs::create_dir_all(core.parent().expect("core parent")).expect("create core");

    fs::write(&main, "import(\"gain.lib\");\nprocess = gain + gain;\n")
        .expect("main should be written");
    fs::write(&lib, "import(\"core/base.lib\");\ngain = base;\n").expect("lib should be written");
    fs::write(&core, "base = _;\n").expect("core should be written");

    let out = parse_file(
        &main,
        &parser::ParseOptions::default().with_search_paths(&[root.join("libs")]),
    )
    .expect("parse should succeed");
    assert!(out.root.is_some(), "root should be present");
    assert!(
        out.errors.is_empty(),
        "unexpected parser errors for import-heavy fixture: {:?}",
        out.errors
    );

    if let Some(cpp_bin) = cpp_bin() {
        let out_c = root.join("out.c");
        let output = Command::new(cpp_bin)
            .arg(&main)
            .arg("-lang")
            .arg("c")
            .arg("-o")
            .arg(&out_c)
            .arg("-I")
            .arg(root.join("libs"))
            .output()
            .expect("failed to run c++ reference parser");
        assert!(
            output.status.success(),
            "C++ should accept import-heavy fixture, status={:?}, stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn production_file_parser_expands_inline_environment_import_structurally() {
    let root = {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "faust_rs_parser_inline_env_import_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should move forward")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp root should be created");
        path
    };

    let main = root.join("main.dsp");
    let child = root.join("child.dsp");
    fs::write(
        &main,
        "GEN = environment { import(\"child.dsp\"); }.process;\nprocess = GEN;\n",
    )
    .expect("main should be written");
    fs::write(&child, "process = _;\n").expect("child should be written");

    let output = parse_file(
        &main,
        &parser::ParseOptions::default().with_search_paths(std::slice::from_ref(&root)),
    )
    .expect("parse should succeed");
    assert!(
        output.errors.is_empty(),
        "unexpected parse errors: {:?}",
        output.errors
    );

    let root_defs = output.root.expect("root should be present");
    let gen_expr = find_definition_expr(&output.state.arena, root_defs, "GEN")
        .expect("GEN definition should be present");
    let (body, field) = match match_box(&output.state.arena, gen_expr) {
        BoxMatch::Access(body, field) => (body, field),
        other => panic!("expected BOXACCESS for GEN, got {other:?}"),
    };
    assert!(matches!(
        match_box(&output.state.arena, field),
        BoxMatch::Ident("process")
    ));
    let (_env_body, defs) = match match_box(&output.state.arena, body) {
        BoxMatch::WithLocalDef(env_body, defs) => (env_body, defs),
        other => panic!("expected BOXWITHLOCALDEF, got {other:?}"),
    };
    assert!(
        collect_import_files(&output.state.arena, defs).is_empty(),
        "structural file-backed expansion should remove import-file nodes from local defs"
    );
    assert_eq!(
        count_definitions_named(&output.state.arena, defs, "process"),
        1,
        "expanded local environment should contain imported process definition"
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn production_parser_preserves_top_level_import_file_node_before_structural_expansion() {
    let output = parse_program(
        "import(\"ops.lib\");\nprocess = _;\n",
        "top_level_import_node.dsp",
    );
    assert!(
        output.errors.is_empty(),
        "unexpected parse errors: {:?}",
        output.errors
    );

    let imports = collect_import_files(
        &output.state.arena,
        output.root.expect("root should be present"),
    );
    assert_eq!(imports, vec!["ops.lib".to_owned()]);
    assert!(
        find_definition_expr(
            &output.state.arena,
            output.root.expect("root should be present"),
            "process"
        )
        .is_some(),
        "process definition should remain present alongside import node"
    );
}

#[test]
fn production_parser_preserves_inline_environment_import_file_node() {
    let output = parse_program(
        "GEN = environment { import(\"child.dsp\"); }.process;\nprocess = GEN;\n",
        "inline_environment_import_node.dsp",
    );
    assert!(
        output.errors.is_empty(),
        "unexpected parse errors: {:?}",
        output.errors
    );
    let root = output.root.expect("root should be present");
    let gen_expr = find_definition_expr(&output.state.arena, root, "GEN")
        .expect("GEN definition should be present");
    let (body, field) = match match_box(&output.state.arena, gen_expr) {
        BoxMatch::Access(body, field) => (body, field),
        other => panic!("expected BOXACCESS for GEN, got {other:?}"),
    };
    assert!(matches!(
        match_box(&output.state.arena, field),
        BoxMatch::Ident("process")
    ));
    let (env_body, defs) = match match_box(&output.state.arena, body) {
        BoxMatch::WithLocalDef(env_body, defs) => (env_body, defs),
        other => panic!("expected BOXWITHLOCALDEF, got {other:?}"),
    };
    assert!(matches!(
        match_box(&output.state.arena, env_body),
        BoxMatch::Environment
    ));
    let imports = collect_import_files(&output.state.arena, defs);
    assert_eq!(imports, vec!["child.dsp".to_owned()]);
}

#[test]
fn production_parser_groups_patterned_and_multi_clause_definitions_like_cpp() {
    let cases = [
        (
            "patterned_constant_clause",
            "foo(0) = _;\nfoo(x) = x;\nprocess = foo;\n",
        ),
        (
            "grouped_identifier_clauses",
            "foo(x) = x;\nfoo(y) = y;\nprocess = foo;\n",
        ),
    ];

    let cpp = cpp_bin();
    for (name, source) in cases {
        if let Some(cpp_bin) = &cpp {
            let cpp_ok = cpp_accepts_source(cpp_bin, source, name)
                .unwrap_or_else(|e| panic!("C++ run failed for {name}: {e}"));
            assert!(cpp_ok, "C++ should accept grouped-definition case {name}");
        }

        let output = parse_program(source, &format!("structural_{name}.dsp"));
        assert!(
            output.errors.is_empty(),
            "unexpected parse errors for {}: {:?}",
            name,
            output.errors
        );
        let root = output.root.expect("root should be present");
        let foo_expr = find_definition_expr(&output.state.arena, root, "foo")
            .unwrap_or_else(|| panic!("missing foo definition for {name}"));
        assert!(
            matches!(match_box(&output.state.arena, foo_expr), BoxMatch::Case(_)),
            "foo should be normalized to BOXCASE for {name}, got {:?}",
            match_box(&output.state.arena, foo_expr)
        );
        assert_eq!(
            count_definitions_named(&output.state.arena, root, "foo"),
            1,
            "foo should be grouped into one definition for {name}"
        );

        let rules = case_rules(&output.state.arena, foo_expr);
        let mut cursor = rules;
        let mut saw_pattern_var = false;
        let mut saw_zero_literal = false;
        while !output.state.arena.is_nil(cursor) {
            let rule = list_head(&output.state.arena, cursor);
            let pattern = rule_first_pattern(&output.state.arena, rule);
            saw_pattern_var |= matches!(
                match_box(&output.state.arena, pattern),
                BoxMatch::PatternVar(_)
            );
            saw_zero_literal |= matches!(output.state.arena.kind(pattern), Some(NodeKind::Int(0)));
            cursor = list_tail(&output.state.arena, cursor);
        }
        assert!(
            saw_pattern_var,
            "at least one clause should be wrapped as BOXPATVAR for {name}"
        );

        if name == "patterned_constant_clause" {
            assert!(
                saw_zero_literal,
                "constant clause should stay literal for {name}"
            );
        }
    }
}

#[test]
fn production_parser_recognizes_modif_local_def_like_cpp() {
    let source = "e = environment { a = _; };\nprocess = (e [ a = 1; ]).a;\n";

    if let Some(cpp_bin) = cpp_bin() {
        let cpp_ok = cpp_accepts_source(cpp_bin.as_path(), source, "modif_local_def")
            .unwrap_or_else(|e| panic!("C++ run failed for modif_local_def: {e}"));
        assert!(cpp_ok, "C++ should accept modif-local-def syntax");
    }

    let output = parse_program(source, "structural_modif_local_def.dsp");
    assert!(
        output.errors.is_empty(),
        "unexpected parse errors for modif-local-def: {:?}",
        output.errors
    );
    let root = output.root.expect("root should be present");
    let process_expr = find_definition_expr(&output.state.arena, root, "process")
        .expect("missing process definition");
    let (modif_expr, field) = match match_box(&output.state.arena, process_expr) {
        BoxMatch::Access(body, field) => (body, field),
        other => panic!("expected BOXACCESS over BOXMODIFLOCALDEF, got {:?}", other),
    };
    assert!(
        matches!(match_box(&output.state.arena, field), BoxMatch::Ident("a")),
        "access field should remain `a`"
    );
    let (body, defs) = match match_box(&output.state.arena, modif_expr) {
        BoxMatch::ModifLocalDef(body, defs) => (body, defs),
        other => panic!("expected BOXMODIFLOCALDEF, got {:?}", other),
    };
    assert!(
        matches!(match_box(&output.state.arena, body), BoxMatch::Ident("e")),
        "modif-local-def body should remain the source expression"
    );
    assert!(
        !output.state.arena.is_nil(defs),
        "modif-local-def should carry formatted definitions"
    );
}

#[test]
fn production_parser_prepare_pattern_keeps_access_component_and_environment_opaque_like_cpp() {
    let cases = [(
        "opaque_environment_pattern",
        "foo = case { (environment { a = x; }) => _; (x) => x; };\nprocess = foo;\n",
        "environment",
    )];

    let cpp = cpp_bin();
    for (name, source, expected_kind) in cases {
        if let Some(cpp_bin) = &cpp {
            let cpp_ok = cpp_accepts_source(cpp_bin, source, name)
                .unwrap_or_else(|e| panic!("C++ run failed for {name}: {e}"));
            assert!(cpp_ok, "C++ should accept prepare-pattern case {name}");
        }

        let output = parse_program(source, &format!("structural_{name}.dsp"));
        assert!(
            output.errors.is_empty(),
            "unexpected parse errors for {}: {:?}",
            name,
            output.errors
        );
        let root = output.root.expect("root should be present");
        let foo_expr = find_definition_expr(&output.state.arena, root, "foo")
            .unwrap_or_else(|| panic!("missing foo definition for {name}"));
        let rules = case_rules(&output.state.arena, foo_expr);
        let pattern = collect_rule_patterns(&output.state.arena, rules)
            .into_iter()
            .find(|pattern| {
                matches!(
                    match_box(&output.state.arena, *pattern),
                    BoxMatch::WithLocalDef(_, _)
                )
            })
            .unwrap_or_else(|| panic!("missing environment pattern for {name}"));

        match expected_kind {
            "environment" => {
                let (body, defs) = match match_box(&output.state.arena, pattern) {
                    BoxMatch::WithLocalDef(body, defs) => (body, defs),
                    other => {
                        panic!(
                            "expected BOXWITHLOCALDEF environment wrapper for {name}, got {:?}",
                            other
                        )
                    }
                };
                assert!(
                    matches!(match_box(&output.state.arena, body), BoxMatch::Environment),
                    "environment wrapper body should stay BOXENVIRONMENT for {name}"
                );
                let def = list_head(&output.state.arena, defs);
                let expr = definition_expr(&output.state.arena, def);
                assert!(
                    matches!(match_box(&output.state.arena, expr), BoxMatch::Ident("x")),
                    "environment local definition should keep raw ident payload for {name}"
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn production_parser_prepare_pattern_keeps_component_opaque_like_cpp() {
    let root = {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "faust_rs_parser_prepare_component_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should move forward")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp root should be created");
        path
    };
    let main = root.join("main.dsp");
    let child = root.join("child.dsp");
    fs::write(
        &main,
        "foo = case { (component(\"child.dsp\")) => _; (x) => x; };\nprocess = foo;\n",
    )
    .expect("main should be written");
    fs::write(&child, "process = _;\n").expect("child should be written");

    if let Some(cpp_bin) = cpp_bin() {
        let out_c = root.join("out.c");
        let output = Command::new(cpp_bin)
            .arg(&main)
            .arg("-lang")
            .arg("c")
            .arg("-o")
            .arg(&out_c)
            .output()
            .expect("failed to run c++ reference parser");
        assert!(
            output.status.success(),
            "C++ should accept opaque component pattern, status={:?}, stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = fs::remove_file(out_c);
    }

    let output = parse_file(
        &main,
        &parser::ParseOptions::default().with_search_paths(std::slice::from_ref(&root)),
    )
    .expect("parse should succeed");
    assert!(
        output.errors.is_empty(),
        "unexpected parse errors for opaque component pattern: {:?}",
        output.errors
    );
    let root_defs = output.root.expect("root should be present");
    let foo_expr =
        find_definition_expr(&output.state.arena, root_defs, "foo").expect("missing foo");
    let rules = case_rules(&output.state.arena, foo_expr);
    let pattern = collect_rule_patterns(&output.state.arena, rules)
        .into_iter()
        .find(|pattern| {
            matches!(
                match_box(&output.state.arena, *pattern),
                BoxMatch::Component(_)
            )
        })
        .expect("missing component pattern");
    assert!(
        matches!(
            match_box(&output.state.arena, pattern),
            BoxMatch::Component(_)
        ),
        "component pattern should stay opaque"
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn production_parser_prepare_pattern_keeps_access_opaque_structurally() {
    let source = "foo = case { (e.a) => e; (x) => x; };\nprocess = foo;\n";
    let output = parse_program(source, "structural_access_pattern.dsp");
    assert!(
        output.errors.is_empty(),
        "unexpected parse errors for structural access pattern: {:?}",
        output.errors
    );
    let root = output.root.expect("root should be present");
    let foo_expr =
        find_definition_expr(&output.state.arena, root, "foo").expect("missing foo definition");
    let rules = case_rules(&output.state.arena, foo_expr);
    let pattern = collect_rule_patterns(&output.state.arena, rules)
        .into_iter()
        .find(|pattern| {
            matches!(
                match_box(&output.state.arena, *pattern),
                BoxMatch::Access(_, _)
            )
        })
        .expect("missing access pattern");
    let (body, field) = match match_box(&output.state.arena, pattern) {
        BoxMatch::Access(body, field) => (body, field),
        other => panic!("expected BOXACCESS pattern, got {:?}", other),
    };
    assert!(
        matches!(match_box(&output.state.arena, body), BoxMatch::Ident("e")),
        "access body should remain raw ident after prepare_pattern"
    );
    assert!(
        matches!(match_box(&output.state.arena, field), BoxMatch::Ident("a")),
        "access field should remain raw ident after prepare_pattern"
    );
}
