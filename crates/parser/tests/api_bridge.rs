//! Integration tests for `api_bridge`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use std::fs;
use std::path::PathBuf;

use boxes::{BoxBuilder, BoxMatch, dump_box, match_box};
use diagnostics::SourceKind;
use parser::{
    CompilationMetadataKey, CompilationMetadataStore, SourceReaderError, VirtualSourceMap,
    parse_file_with_imports, parse_minimal, parse_program, parse_program_with_imports_and_metadata,
};
use tlib::{TreeArena, TreeId};

fn make_temp_root(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "faust_rs_parser_bridge_{}_{}_{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos()
    ));
    fs::create_dir_all(&path).expect("temp root should be created");
    path
}

fn list_head(arena: &TreeArena, list: TreeId) -> TreeId {
    arena.hd(list).expect("list must be non-empty")
}

fn definition_name(arena: &TreeArena, def: TreeId) -> Option<&str> {
    match match_box(arena, list_head(arena, def)) {
        BoxMatch::Ident(text) => Some(text),
        _ => None,
    }
}

fn count_definitions_named(arena: &TreeArena, mut defs: TreeId, expected: &str) -> usize {
    let mut count = 0usize;
    while !arena.is_nil(defs) {
        let Some(def) = arena.hd(defs) else {
            break;
        };
        if definition_name(arena, def) == Some(expected) {
            count = count.saturating_add(1);
        }
        defs = arena.tl(defs).unwrap_or_else(|| arena.nil());
    }
    count
}

#[test]
fn bridge_exposes_minimal_parse_helper() {
    assert!(parse_minimal("process = _;"));
}

#[test]
fn bridge_exposes_parse_program() {
    let out = parse_program("process = _;", "bridge_program.dsp");
    assert!(out.root.is_some(), "root should be present");
    assert!(
        out.errors.is_empty(),
        "unexpected parse errors: {:?}",
        out.errors
    );
    let source = out
        .diagnostics
        .source_map()
        .find_by_name(std::path::Path::new("bridge_program.dsp"))
        .expect("memory source snapshot should be registered");
    assert_eq!(source.kind(), SourceKind::Memory);
    assert_eq!(source.text(), "process = _;");
}

#[test]
fn parse_program_recognizes_ad_wrappers_like_cpp() {
    let out = parse_program(
        "process = fad(hslider(\"freq\", 440, 50, 2000, 0.01) : sin, hslider(\"freq\", 440, 50, 2000, 0.01));",
        "bridge_fad_program.dsp",
    );
    assert!(
        out.errors.is_empty(),
        "unexpected parse errors: {:?}",
        out.errors
    );
    let root = out.root.expect("root should be present");
    let def = list_head(&out.state.arena, root);
    let payload = out.state.arena.tl(def).expect("definition payload");
    let expr = out.state.arena.tl(payload).expect("definition expression");

    let BoxMatch::ForwardAD(inner, _seed) = match_box(&out.state.arena, expr) else {
        panic!("expected fad wrapper at process root");
    };
    assert!(
        matches!(match_box(&out.state.arena, inner), BoxMatch::Seq(_, _)),
        "fad body should preserve wrapped expression structure"
    );
}

#[test]
fn parse_program_recognizes_rad_wrapper_and_missing_body_is_an_error() {
    let ok = parse_program(
        "process = rad(hslider(\"f\", 1, 0, 10, 0.1) : sin, hslider(\"p\", 0, -1, 1, 0.01));",
        "bridge_rad_program.dsp",
    );
    assert!(
        ok.errors.is_empty(),
        "unexpected parse errors: {:?}",
        ok.errors
    );
    let root = ok.root.expect("root should be present");
    let def = list_head(&ok.state.arena, root);
    let payload = ok.state.arena.tl(def).expect("definition payload");
    let expr = ok.state.arena.tl(payload).expect("definition expression");
    assert!(matches!(
        match_box(&ok.state.arena, expr),
        BoxMatch::ReverseAD(_, _)
    ));

    let err = parse_program("process = rad(process);", "bridge_rad_legacy_one_arg.dsp");
    assert!(
        err.root.is_none() || !err.errors.is_empty() || err.state.ctx.parse_error_count() > 0,
        "single-argument rad must be rejected after the surface migration"
    );

    let err = parse_program("process = fad();", "bridge_fad_missing_body.dsp");
    assert!(
        err.root.is_none() || !err.errors.is_empty() || err.state.ctx.parse_error_count() > 0,
        "missing fad body should be rejected"
    );
}

#[test]
fn parse_program_recognizes_fad_with_explicit_seed() {
    let out = parse_program(
        "process = fad(hslider(\"x\", 0, 0, 1, 0.01) : sin, hslider(\"x\", 0, 0, 1, 0.01));",
        "bridge_fad_seed.dsp",
    );
    assert!(
        out.errors.is_empty(),
        "unexpected parse errors: {:?}",
        out.errors
    );
    let root = out.root.expect("root should be present");
    let def = list_head(&out.state.arena, root);
    let payload = out.state.arena.tl(def).expect("definition payload");
    let expr = out.state.arena.tl(payload).expect("definition expression");
    let BoxMatch::ForwardAD(inner, seed) = match_box(&out.state.arena, expr) else {
        panic!("expected fad wrapper at process root");
    };
    assert!(
        matches!(match_box(&out.state.arena, inner), BoxMatch::Seq(_, _)),
        "fad body should be a seq"
    );
    assert!(
        matches!(
            match_box(&out.state.arena, seed),
            BoxMatch::HSlider(_, _, _, _, _)
        ),
        "fad seed should be an hslider"
    );
}

#[test]
fn parse_program_exposes_master_document_metadata_snapshot() {
    let out = parse_program(
        "declare name \"main\";\nprocess = _;\n",
        "bridge_metadata_program.dsp",
    );
    assert!(
        out.errors.is_empty(),
        "unexpected parse errors: {:?}",
        out.errors
    );
    let values = out
        .compilation_metadata
        .entries()
        .get(&CompilationMetadataKey::global("name"))
        .expect("master metadata key should exist");
    assert!(values.contains("main"));
}

#[test]
fn bridge_exposes_file_import_parsing() {
    let root = make_temp_root("imports");
    let main = root.join("main.dsp");
    let lib = root.join("ops.lib");

    fs::write(&main, "import(\"ops.lib\");\nprocess = gain;\n").expect("main should be written");
    fs::write(&lib, "gain = _;\n").expect("lib should be written");

    let out =
        parse_file_with_imports(&main, std::slice::from_ref(&root)).expect("parse should succeed");
    assert!(out.root.is_some(), "root should be present");
    assert!(
        out.errors.is_empty(),
        "unexpected parse errors: {:?}",
        out.errors
    );
    assert_eq!(
        out.used_files.len(),
        2,
        "used_files should contain entry + imported file"
    );
    assert_eq!(
        out.used_files[0],
        main.canonicalize().expect("main should canonicalize")
    );
    assert_eq!(
        out.used_files[1],
        lib.canonicalize().expect("lib should canonicalize")
    );
    let sources = out.diagnostics.source_map();
    assert_eq!(sources.len(), 2);
    assert_eq!(
        sources
            .iter()
            .map(|source| source.kind())
            .collect::<Vec<_>>(),
        vec![SourceKind::File, SourceKind::ImportedFile]
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn parse_file_with_imports_scopes_top_level_metadata_like_cpp() {
    let root = make_temp_root("metadata_imports");
    let main = root.join("main.dsp");
    let lib = root.join("ops.lib");

    fs::write(
        &main,
        "declare name \"main\";\nimport(\"ops.lib\");\nprocess = gain;\n",
    )
    .expect("main should be written");
    fs::write(&lib, "declare author \"lib-author\";\ngain = _;\n").expect("lib should be written");

    let out =
        parse_file_with_imports(&main, std::slice::from_ref(&root)).expect("parse should succeed");
    assert!(
        out.errors.is_empty(),
        "unexpected parse errors: {:?}",
        out.errors
    );

    let master = out
        .compilation_metadata
        .entries()
        .get(&CompilationMetadataKey::global("name"))
        .expect("master metadata key should exist");
    assert!(master.contains("main"));

    let lib_key = CompilationMetadataKey::scoped(
        lib.canonicalize()
            .expect("lib should canonicalize")
            .to_string_lossy()
            .into_owned(),
        "author",
    );
    let imported = out
        .compilation_metadata
        .entries()
        .get(&lib_key)
        .expect("imported metadata key should exist");
    assert!(imported.contains("lib-author"));

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn parse_file_with_imports_exposes_deterministic_used_files_order() {
    let root = make_temp_root("used_files_order");
    let main = root.join("main.dsp");
    let lib_a = root.join("a.lib");
    let lib_b = root.join("b.lib");

    fs::write(
        &main,
        "import(\"a.lib\");\nimport(\"b.lib\");\nprocess = a + b;\n",
    )
    .expect("main should be written");
    fs::write(&lib_a, "a = _;\n").expect("a.lib should be written");
    fs::write(&lib_b, "b = _;\n").expect("b.lib should be written");

    let out =
        parse_file_with_imports(&main, std::slice::from_ref(&root)).expect("parse should succeed");
    assert!(
        out.errors.is_empty(),
        "unexpected parse errors: {:?}",
        out.errors
    );

    let expected = vec![
        main.canonicalize().expect("main should canonicalize"),
        lib_a.canonicalize().expect("a.lib should canonicalize"),
        lib_b.canonicalize().expect("b.lib should canonicalize"),
    ];
    assert_eq!(
        out.used_files, expected,
        "used_files order should follow deterministic expansion order"
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn parse_file_with_imports_preserves_imported_file_diagnostic_origin() {
    let root = make_temp_root("import_origin");
    let main = root.join("main.dsp");
    let lib = root.join("ops.lib");

    fs::write(&main, "import(\"ops.lib\");\nprocess = gain;\n").expect("main should be written");
    fs::write(&lib, "gain = ;\n").expect("lib should be written");

    let out =
        parse_file_with_imports(&main, std::slice::from_ref(&root)).expect("parse should succeed");

    let lib_canonical = lib.canonicalize().expect("lib path should canonicalize");
    let has_label_on_imported_file = out
        .diagnostics
        .as_slice()
        .iter()
        .flat_map(|d| d.labels.iter())
        .any(|label| label.span.file == lib_canonical);
    assert!(
        has_label_on_imported_file,
        "expected at least one parser diagnostic label on imported file {}",
        lib_canonical.display()
    );

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn parse_file_with_imports_keeps_remote_urls_out_of_scope() {
    let root = make_temp_root("remote_import_policy");
    let main = root.join("main.dsp");
    fs::write(
        &main,
        "import(\"https://example.com/stdfaust.lib\");\nprocess = _;\n",
    )
    .expect("main should be written");

    let err = parse_file_with_imports(&main, std::slice::from_ref(&root)).expect_err("must fail");
    match err {
        SourceReaderError::UnresolvedImport { name, from, .. } => {
            assert_eq!(&*name, "https://example.com/stdfaust.lib");
            assert_eq!(from, main.canonicalize().expect("main should canonicalize"));
        }
        other => panic!("unexpected error kind for remote import policy: {other:?}"),
    }

    fs::remove_dir_all(root).expect("temp root should be removable");
}

#[test]
fn parse_program_with_imports_deduplicates_transitive_virtual_imports() {
    let bundle = VirtualSourceMap::new([
        (
            PathBuf::from("stdfaust.lib"),
            "import(\"maths.lib\");\nimport(\"osc.lib\");\n".to_owned(),
        ),
        (PathBuf::from("maths.lib"), "PI = 3.14;\n".to_owned()),
        (
            PathBuf::from("osc.lib"),
            "import(\"maths.lib\");\nfreq = PI;\n".to_owned(),
        ),
    ]);

    let out = parse_program_with_imports_and_metadata(
        "import(\"stdfaust.lib\");\nprocess = freq;\n",
        "main.dsp",
        &[],
        &bundle,
        CompilationMetadataStore::new("main.dsp"),
    )
    .expect("virtual import parse should succeed");
    assert!(
        out.errors.is_empty(),
        "unexpected parse errors: {:?}",
        out.errors
    );

    let root = out.root.expect("root should be present");
    assert_eq!(
        count_definitions_named(&out.state.arena, root, "PI"),
        1,
        "transitively re-imported virtual definitions should be expanded only once"
    );
    assert_eq!(
        out.used_files,
        vec![
            PathBuf::from("main.dsp"),
            PathBuf::from("stdfaust.lib"),
            PathBuf::from("maths.lib"),
            PathBuf::from("osc.lib"),
        ],
        "virtual-source used_files order should follow structural import visitation"
    );
    assert_eq!(
        out.diagnostics
            .source_map()
            .iter()
            .map(|source| source.kind())
            .collect::<Vec<_>>(),
        vec![
            SourceKind::Memory,
            SourceKind::VirtualLibrary,
            SourceKind::VirtualLibrary,
            SourceKind::VirtualLibrary,
        ]
    );
}

#[test]
fn parse_program_with_imports_treats_inline_and_multiline_local_imports_equivalently() {
    let bundle = VirtualSourceMap::new([(PathBuf::from("child.lib"), "process = _;\n".to_owned())]);

    let inline = parse_program_with_imports_and_metadata(
        "GEN = environment { import(\"child.lib\"); }.process;\nprocess = GEN;\n",
        "inline_main.dsp",
        &[],
        &bundle,
        CompilationMetadataStore::new("inline_main.dsp"),
    )
    .expect("inline parse should succeed");
    let multiline = parse_program_with_imports_and_metadata(
        "GEN = environment {\nimport(\"child.lib\");\n}.process;\nprocess = GEN;\n",
        "multiline_main.dsp",
        &[],
        &bundle,
        CompilationMetadataStore::new("multiline_main.dsp"),
    )
    .expect("multiline parse should succeed");

    assert!(
        inline.errors.is_empty(),
        "unexpected inline parse errors: {:?}",
        inline.errors
    );
    assert!(
        multiline.errors.is_empty(),
        "unexpected multiline parse errors: {:?}",
        multiline.errors
    );

    let inline_dump = dump_box(
        &inline.state.arena,
        inline.root.expect("inline root should be present"),
    );
    let multiline_dump = dump_box(
        &multiline.state.arena,
        multiline.root.expect("multiline root should be present"),
    );
    assert_eq!(
        inline_dump, multiline_dump,
        "inline and multiline local imports should expand to the same structural tree"
    );
    assert_eq!(
        inline.used_files,
        vec![PathBuf::from("inline_main.dsp"), PathBuf::from("child.lib")],
        "inline used_files should include entry then imported local source"
    );
    assert_eq!(
        multiline.used_files,
        vec![
            PathBuf::from("multiline_main.dsp"),
            PathBuf::from("child.lib")
        ],
        "multiline used_files should include entry then imported local source"
    );
}

#[test]
fn repeated_hash_consed_identifier_uses_keep_distinct_parse_occurrences() {
    let mut output = parse_program(
        "a = missing;\nb = missing;\nprocess = a,b;\n",
        "repeated.dsp",
    );
    assert!(output.errors.is_empty(), "{:?}", output.errors);

    let shared = BoxBuilder::new(&mut output.state.arena).ident("missing");
    let ids = output.state.ctx.box_provenance().origins_for(shared);
    assert_eq!(
        ids.len(),
        2,
        "both syntactic uses must survive hash-consing"
    );
    let origins = ids
        .iter()
        .map(|id| {
            output
                .state
                .ctx
                .box_provenance()
                .get(*id)
                .expect("recorded occurrence should resolve")
        })
        .collect::<Vec<_>>();
    assert_eq!(origins[0].location.line(), 1);
    assert_eq!(origins[1].location.line(), 2);
    assert_eq!(origins[0].node, origins[1].node);
}

#[test]
fn imported_box_occurrences_are_remapped_into_the_destination_arena() {
    let bundle =
        VirtualSourceMap::new([(PathBuf::from("child.lib"), "foo = missing;\n".to_owned())]);
    let mut output = parse_program_with_imports_and_metadata(
        "import(\"child.lib\");\nprocess = foo;\n",
        "main.dsp",
        &[],
        &bundle,
        CompilationMetadataStore::new("main.dsp"),
    )
    .expect("virtual import should parse");
    assert!(output.errors.is_empty(), "{:?}", output.errors);

    let missing = BoxBuilder::new(&mut output.state.arena).ident("missing");
    let origins = output
        .state
        .ctx
        .box_provenance()
        .origins_for(missing)
        .iter()
        .filter_map(|id| output.state.ctx.box_provenance().get(*id))
        .collect::<Vec<_>>();
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0].location.file(), "child.lib");
    assert_eq!(origins[0].location.line(), 1);
}
