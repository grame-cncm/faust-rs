use super::*;
use crate::checker::{Severity, verify_fir_module};
use crate::{AccessType, FirBuilder, FirStore, FirType, NamedType, dump_fir};

fn fun(
    b: &mut FirBuilder<'_>,
    name: &str,
    args: &[NamedType],
    ret: FirType,
    body: Option<FirId>,
    is_inline: bool,
) -> FirId {
    let sig = FirType::Fun {
        args: args.iter().map(|a| a.typ.clone()).collect(),
        ret: Box::new(ret),
    };
    b.declare_fun(name, sig, args, body, is_inline)
}

fn assert_no_checker_errors(store: &FirStore, module: FirId) {
    let report = verify_fir_module(store, module);
    let errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "expected no FIR checker errors after hygienic clone, got: {errors:?}"
    );
}

#[test]
fn scaffolding_drop_sweep_removes_only_pure_block_roots() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let one = b.float32(1.0);
    let pure = b.binop(crate::FirBinOp::Add, one, one, FirType::Float32);
    let pure_math = b.math_call(crate::FirMathOp::Pow, &[one, one], FirType::Float32);
    let foreign = b.fun_call("observable", &[], FirType::Void);
    let pure_drop = b.drop_(pure);
    let pure_math_drop = b.drop_(pure_math);
    let foreign_drop = b.drop_(foreign);
    let body = b.block(&[pure_drop, pure_math_drop, foreign_drop]);
    let function = fun(&mut b, "compute", &[], FirType::Void, Some(body), false);
    let globals = b.block(&[]);
    let functions = b.block(&[function]);
    let static_decls = b.block(&[]);
    let module = b.module(
        0,
        0,
        "drop_sweep",
        globals,
        globals,
        functions,
        static_decls,
    );

    let (swept_store, swept_module) = sweep_scaffolding_drop_roots(&store, module);
    let FirMatch::Module { functions, .. } = match_fir(&swept_store, swept_module) else {
        panic!("sweep must preserve module root");
    };
    let FirMatch::Block(functions) = match_fir(&swept_store, functions) else {
        panic!("module functions must remain a block");
    };
    let FirMatch::DeclareFun {
        body: Some(body), ..
    } = match_fir(&swept_store, functions[0])
    else {
        panic!("function definition must remain present");
    };
    let FirMatch::Block(stmts) = match_fir(&swept_store, body) else {
        panic!("function body must remain a block");
    };
    assert_eq!(stmts.len(), 1, "pure Drop root must be swept");
    assert!(matches!(
        match_fir(&swept_store, stmts[0]),
        FirMatch::Drop(_)
    ));
}

#[test]
fn analyzes_call_graph_sizes_and_candidates() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let ff = FirType::FaustFloat;
    let x_arg = NamedType {
        name: "x".to_string(),
        typ: ff.clone(),
    };
    let y_arg = NamedType {
        name: "y".to_string(),
        typ: ff.clone(),
    };

    let h_proto = fun(
        &mut b,
        "h",
        std::slice::from_ref(&x_arg),
        ff.clone(),
        None,
        false,
    );

    let g_body = {
        let x = b.load_var("x", crate::AccessType::FunArgs, ff.clone());
        let one = b.float64(1.0);
        let add = b.binop(crate::FirBinOp::Add, x, one, ff.clone());
        let ret = b.ret(Some(add));
        b.block(&[ret])
    };
    let g_fun = fun(
        &mut b,
        "g",
        std::slice::from_ref(&x_arg),
        ff.clone(),
        Some(g_body),
        true,
    );

    let f_body = {
        let x = b.load_var("x", crate::AccessType::FunArgs, ff.clone());
        let y = b.load_var("y", crate::AccessType::FunArgs, ff.clone());
        let call_g = b.fun_call("g", &[x], ff.clone());
        let call_h = b.fun_call("h", &[y], ff.clone());
        let add = b.binop(crate::FirBinOp::Add, call_g, call_h, ff.clone());
        let ret = b.ret(Some(add));
        b.block(&[ret])
    };
    let f_fun = fun(
        &mut b,
        "f",
        &[x_arg.clone(), y_arg.clone()],
        ff.clone(),
        Some(f_body),
        false,
    );

    let dsp_struct = b.block(&[]);
    let globals = b.block(&[h_proto]);
    let decls = b.block(&[g_fun, f_fun]);
    let module = {
        let static_decls = b.block(&[]);
        b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
    };

    let analysis = analyze_fir_inliner(&store, module, &FirInlineOptions::default())
        .expect("valid module should analyze");

    assert_eq!(analysis.functions.len(), 3);
    assert_eq!(
        analysis
            .call_graph
            .get("f")
            .expect("f in graph")
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["g".to_string(), "h".to_string()]
    );
    assert!(
        analysis
            .functions
            .get("g")
            .expect("g exists")
            .body_node_count
            > 0,
        "body node metric should be collected for defined functions"
    );
    assert_eq!(
        analysis
            .functions
            .get("h")
            .expect("h exists")
            .body_node_count,
        0,
        "prototype body metric should be zero"
    );
    assert!(
        analysis.is_callee_candidate("g"),
        "small non-recursive helper should be a candidate"
    );
    assert!(
        !analysis.is_callee_candidate("h"),
        "prototype-only extern should be skipped"
    );
}

#[test]
fn detects_recursive_sccs_and_marks_skipped() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let ff = FirType::FaustFloat;
    let x_arg = NamedType {
        name: "x".to_string(),
        typ: ff.clone(),
    };

    let f_body = {
        let x = b.load_var("x", crate::AccessType::FunArgs, ff.clone());
        let call_g = b.fun_call("g", &[x], ff.clone());
        let ret = b.ret(Some(call_g));
        b.block(&[ret])
    };
    let g_body = {
        let x = b.load_var("x", crate::AccessType::FunArgs, ff.clone());
        let call_f = b.fun_call("f", &[x], ff.clone());
        let ret = b.ret(Some(call_f));
        b.block(&[ret])
    };
    let f_fun = fun(
        &mut b,
        "f",
        std::slice::from_ref(&x_arg),
        ff.clone(),
        Some(f_body),
        true,
    );
    let g_fun = fun(
        &mut b,
        "g",
        std::slice::from_ref(&x_arg),
        ff.clone(),
        Some(g_body),
        true,
    );
    let module = {
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[f_fun, g_fun]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };

    let analysis = analyze_fir_inliner(&store, module, &FirInlineOptions::default())
        .expect("analysis should succeed");

    let scc_f = analysis.functions.get("f").unwrap().scc_index;
    let scc_g = analysis.functions.get("g").unwrap().scc_index;
    assert_eq!(
        scc_f, scc_g,
        "mutually recursive functions should share SCC"
    );
    assert!(analysis.sccs[scc_f].is_recursive);
    assert!(
        analysis
            .candidate_decisions
            .get("f")
            .unwrap()
            .reasons
            .contains(&FirInlineSkipReason::RecursiveScc)
    );
}

#[test]
fn candidate_policy_respects_marked_only_size_and_reserved_api() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let ff = FirType::FaustFloat;
    let x_arg = NamedType {
        name: "x".to_string(),
        typ: ff.clone(),
    };

    let helper_body = {
        let x = b.load_var("x", crate::AccessType::FunArgs, ff.clone());
        let ret = b.ret(Some(x));
        b.block(&[ret])
    };
    let helper = fun(
        &mut b,
        "helper",
        std::slice::from_ref(&x_arg),
        ff.clone(),
        Some(helper_body),
        false,
    );

    let compute_body = {
        let x = b.load_var("x", crate::AccessType::FunArgs, ff.clone());
        let ret = b.ret(Some(x));
        b.block(&[ret])
    };
    let compute = fun(
        &mut b,
        "compute",
        std::slice::from_ref(&x_arg),
        ff.clone(),
        Some(compute_body),
        true,
    );

    let large_body = {
        let x = b.load_var("x", crate::AccessType::FunArgs, ff.clone());
        let c0 = b.float64(0.0);
        let c1 = b.float64(1.0);
        let c2 = b.float64(2.0);
        let a0 = b.binop(crate::FirBinOp::Add, x, c0, ff.clone());
        let a1 = b.binop(crate::FirBinOp::Add, a0, c1, ff.clone());
        let a2 = b.binop(crate::FirBinOp::Add, a1, c2, ff.clone());
        let ret = b.ret(Some(a2));
        b.block(&[ret])
    };
    let large = fun(
        &mut b,
        "large",
        std::slice::from_ref(&x_arg),
        ff.clone(),
        Some(large_body),
        true,
    );

    let module = {
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[helper, compute, large]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };

    let opts = FirInlineOptions {
        inline_marked_only: true,
        max_callee_nodes: 4,
        ..FirInlineOptions::default()
    };
    let analysis = analyze_fir_inliner(&store, module, &opts).expect("analysis should succeed");

    let helper_dec = analysis.candidate_decisions.get("helper").unwrap();
    assert!(!helper_dec.eligible);
    assert!(
        helper_dec
            .reasons
            .contains(&FirInlineSkipReason::NotMarkedInline)
    );

    let compute_dec = analysis.candidate_decisions.get("compute").unwrap();
    assert!(!compute_dec.eligible);
    assert!(
        compute_dec
            .reasons
            .contains(&FirInlineSkipReason::ReservedApi)
    );

    let large_dec = analysis.candidate_decisions.get("large").unwrap();
    assert!(!large_dec.eligible);
    assert!(
        large_dec
            .reasons
            .iter()
            .any(|r| matches!(r, FirInlineSkipReason::TooLarge { .. }))
    );
}

#[test]
fn hygienic_clone_renames_local_decls_and_rewrites_local_uses() {
    let mut src = FirStore::new();
    let src_block = {
        let mut b = FirBuilder::new(&mut src);
        let zero = b.int32(0);
        let decl = b.declare_var("tmp", FirType::Int32, AccessType::Stack, Some(zero));
        let load = b.load_var("tmp", AccessType::Stack, FirType::Int32);
        let dropv = b.drop_(load);
        b.block(&[decl, dropv])
    };

    let mut dst = FirStore::new();
    let cloned = clone_fir_hygienic(&src, src_block, &mut dst).expect("clone should succeed");

    assert_eq!(cloned.local_renames.len(), 1);
    let rename = &cloned.local_renames[0];
    assert_eq!(rename.original, "tmp");
    assert_ne!(rename.renamed, "tmp");
    assert!(rename.renamed.starts_with("__fir_inl"));

    let dump = dump_fir(&dst, cloned.root);
    assert!(dump.contains(&format!("DeclareVar {{ name: \"{}\"", rename.renamed)));
    assert!(dump.contains(&format!("LoadVar {{ name: \"{}\"", rename.renamed)));
    assert!(!dump.contains("DeclareVar { name: \"tmp\""));
}

#[test]
fn hygienic_clone_state_avoids_name_collisions_across_repeated_clones() {
    let mut src = FirStore::new();
    let src_block = {
        let mut b = FirBuilder::new(&mut src);
        let zero = b.int32(0);
        let decl = b.declare_var("tmp", FirType::Int32, AccessType::Stack, Some(zero));
        let upper = b.int32(4);
        let body = {
            let i = b.load_var("i", AccessType::Loop, FirType::Int32);
            let st = b.store_var("tmp", AccessType::Stack, i);
            b.block(&[st])
        };
        let loop_stmt = b.simple_for_loop("i", upper, body, false);
        let load = b.load_var("tmp", AccessType::Stack, FirType::Int32);
        let dropv = b.drop_(load);
        b.block(&[decl, loop_stmt, dropv])
    };

    let mut dst = FirStore::new();
    let mut state = FirHygienicCloneState::default();
    let c1 = clone_fir_hygienic_with_state(&src, src_block, &mut dst, &mut state)
        .expect("first clone should succeed");
    let c2 = clone_fir_hygienic_with_state(&src, src_block, &mut dst, &mut state)
        .expect("second clone should succeed");

    let c1_names: HashSet<_> = c1.local_renames.iter().map(|r| r.renamed.clone()).collect();
    let c2_names: HashSet<_> = c2.local_renames.iter().map(|r| r.renamed.clone()).collect();
    assert!(
        c1_names.is_disjoint(&c2_names),
        "reused clone state should generate disjoint fresh locals"
    );

    let module = {
        let mut b = FirBuilder::new(&mut dst);
        let body = b.block(&[c1.root, c2.root]);
        let f = fun(&mut b, "helper", &[], FirType::Void, Some(body), false);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[f]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };
    assert_no_checker_errors(&dst, module);
}

#[test]
fn hygienic_clone_renames_loop_vars_and_iterators_consistently() {
    let mut src = FirStore::new();
    let src_block = {
        let mut b = FirBuilder::new(&mut src);
        let zero = b.int32(0);
        let for_init = b.declare_var("j", FirType::Int32, AccessType::Loop, Some(zero));
        let end = b.int32(4);
        let j_load = b.load_var("j", AccessType::Loop, FirType::Int32);
        let one = b.int32(1);
        let j_next = b.binop(crate::FirBinOp::Add, j_load, one, FirType::Int32);
        let step = b.store_var("j", AccessType::Loop, j_next);
        let for_body = {
            let j = b.load_var("j", AccessType::Loop, FirType::Int32);
            let dj = b.drop_(j);
            b.block(&[dj])
        };
        let for_loop = b.for_loop("j", for_init, end, step, for_body, false);

        let iter_body = {
            let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
            let i1 = b.load_var("i1", AccessType::Loop, FirType::Int32);
            let sum = b.binop(crate::FirBinOp::Add, i0, i1, FirType::Int32);
            let ds = b.drop_(sum);
            b.block(&[ds])
        };
        let iter_loop = b.iterator_for_loop(&["i0", "i1"], false, iter_body);
        b.block(&[for_loop, iter_loop])
    };

    let mut dst = FirStore::new();
    let cloned = clone_fir_hygienic(&src, src_block, &mut dst).expect("clone should succeed");
    let renamed_originals: HashSet<_> = cloned
        .local_renames
        .iter()
        .map(|r| r.original.as_str())
        .collect();
    assert!(renamed_originals.contains("j"));
    assert!(renamed_originals.contains("i0"));
    assert!(renamed_originals.contains("i1"));

    let dump = dump_fir(&dst, cloned.root);
    assert!(!dump.contains("ForLoop { var: \"j\""));
    assert!(!dump.contains("IteratorForLoop { iterators: [\"i0\", \"i1\"]"));

    let module = {
        let mut b = FirBuilder::new(&mut dst);
        let body = b.block(&[cloned.root]);
        let f = fun(&mut b, "helper", &[], FirType::Void, Some(body), false);
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[f]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };
    assert_no_checker_errors(&dst, module);
}

#[test]
fn prepare_callee_body_materializes_args_and_substitutes_funargs() {
    let mut src = FirStore::new();
    let (callee_decl, actual0, actual1) = {
        let mut b = FirBuilder::new(&mut src);
        let ff = FirType::FaustFloat;
        let x = NamedType {
            name: "x".to_string(),
            typ: ff.clone(),
        };
        let y = NamedType {
            name: "y".to_string(),
            typ: ff.clone(),
        };
        let body = {
            let lx = b.load_var("x", AccessType::FunArgs, ff.clone());
            let ly = b.load_var("y", AccessType::FunArgs, ff.clone());
            let sum = b.binop(crate::FirBinOp::Add, lx, ly, ff.clone());
            let ret = b.ret(Some(sum));
            b.block(&[ret])
        };
        let callee = fun(&mut b, "add2", &[x, y], ff.clone(), Some(body), true);

        let c0 = b.float64(0.5);
        let c1 = b.float64(1.5);
        let c2 = b.float64(2.5);
        let arg0 = b.binop(crate::FirBinOp::Add, c0, c1, ff.clone());
        (callee, arg0, c2)
    };

    let mut dst = FirStore::new();
    let mut state = FirHygienicCloneState::default();
    let prepared = prepare_callee_body_for_inlining(
        &src,
        callee_decl,
        &[actual0, actual1],
        &mut dst,
        &mut state,
    )
    .expect("preparation should succeed");

    assert_eq!(prepared.arg_materialization_stmts.len(), 2);
    assert_eq!(prepared.param_bindings.len(), 2);
    for binding in &prepared.param_bindings {
        assert!(binding.temp_name.starts_with("__fir_inl"));
    }

    let dump = dump_fir(&dst, prepared.body);
    assert!(
        !dump.contains("access: FunArgs"),
        "prepared body should no longer reference substituted kFunArgs"
    );
    for binding in &prepared.param_bindings {
        assert!(dump.contains(&binding.temp_name));
    }

    let module = {
        let mut b = FirBuilder::new(&mut dst);
        let mut body_stmts = prepared.arg_materialization_stmts.clone();
        body_stmts.push(prepared.body);
        let wrapper_body = b.block(&body_stmts);
        let wrapper = fun(
            &mut b,
            "wrapper",
            &[],
            FirType::FaustFloat,
            Some(wrapper_body),
            false,
        );
        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[wrapper]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };
    assert_no_checker_errors(&dst, module);
}

#[test]
fn prepare_callee_body_rejects_bad_arity_and_prototype() {
    let mut src = FirStore::new();
    let (proto, body_fun, arg) = {
        let mut b = FirBuilder::new(&mut src);
        let ff = FirType::FaustFloat;
        let x = NamedType {
            name: "x".to_string(),
            typ: ff.clone(),
        };
        let proto = fun(
            &mut b,
            "proto",
            std::slice::from_ref(&x),
            ff.clone(),
            None,
            false,
        );
        let body = {
            let lx = b.load_var("x", AccessType::FunArgs, ff.clone());
            let ret = b.ret(Some(lx));
            b.block(&[ret])
        };
        let body_fun = fun(&mut b, "id", &[x], ff.clone(), Some(body), false);
        let arg = b.float64(0.0);
        (proto, body_fun, arg)
    };

    let mut dst = FirStore::new();
    let mut state = FirHygienicCloneState::default();
    let err = prepare_callee_body_for_inlining(&src, proto, &[arg], &mut dst, &mut state)
        .expect_err("prototype should be rejected");
    assert!(matches!(err, FirInlinePrepareError::CalleeHasNoBody { .. }));

    let err = prepare_callee_body_for_inlining(&src, body_fun, &[], &mut dst, &mut state)
        .expect_err("arity mismatch should be rejected");
    assert!(matches!(
        err,
        FirInlinePrepareError::ArgCountMismatch { .. }
    ));
}

#[test]
fn inline_module_once_inlines_canonical_helper_calls() {
    let mut src = FirStore::new();
    let module = {
        let mut b = FirBuilder::new(&mut src);
        let ff = FirType::FaustFloat;
        let x = NamedType {
            name: "x".to_string(),
            typ: ff.clone(),
        };

        let helper_body = {
            let lx = b.load_var("x", AccessType::FunArgs, ff.clone());
            let ret = b.ret(Some(lx));
            b.block(&[ret])
        };
        let helper = fun(
            &mut b,
            "helper",
            std::slice::from_ref(&x),
            ff.clone(),
            Some(helper_body),
            true,
        );

        let wrapper_body = {
            let raw = b.float64(4.0);
            let arg = b.cast(ff.clone(), raw);
            let call = b.fun_call("helper", &[arg], ff.clone());
            let ret = b.ret(Some(call));
            b.block(&[ret])
        };
        let wrapper = fun(
            &mut b,
            "wrapper",
            &[],
            ff.clone(),
            Some(wrapper_body),
            false,
        );

        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[helper, wrapper]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };

    let (dst, rewritten, stats) =
        inline_fir_module_once(&src, module, &FirInlineOptions::default())
            .expect("rewrite should succeed");
    assert_eq!(stats.callsites_seen, 1);
    assert_eq!(stats.callsites_inlined, 1);
    assert_eq!(stats.callsites_skipped_non_candidate, 0);
    assert_eq!(stats.callsites_skipped_unsupported_shape, 0);

    let dump = dump_fir(&dst, rewritten);
    assert!(
        !dump.contains("FunCall { name: \"helper\""),
        "helper call should have been inlined once:\n{dump}"
    );
    assert!(
        dump.contains("DeclareVar { name: \"__fir_inl"),
        "argument materialization temp should be emitted in rewritten body:\n{dump}"
    );
    assert_no_checker_errors(&dst, rewritten);
}

#[test]
fn inline_module_once_preserves_soundfile_nodes_and_inlines_soundfile_helper() {
    let mut src = FirStore::new();
    let module = {
        let mut b = FirBuilder::new(&mut src);

        let helper_body = {
            let zero = b.int32(0);
            let rate = b.load_soundfile_rate("fSound0", zero);
            let ret = b.ret(Some(rate));
            b.block(&[ret])
        };
        let helper = fun(
            &mut b,
            "helper",
            &[],
            FirType::Int32,
            Some(helper_body),
            true,
        );

        let build_ui_body = {
            let add_sf = b.add_soundfile_with_url("sample", "demo.wav", "fSound0");
            b.block(&[add_sf])
        };
        let build_ui = fun(
            &mut b,
            "buildUserInterface",
            &[NamedType {
                name: "ui".to_string(),
                typ: FirType::UI,
            }],
            FirType::Void,
            Some(build_ui_body),
            false,
        );

        let wrapper_body = {
            let call = b.fun_call("helper", &[], FirType::Int32);
            let drop_call = b.drop_(call);
            b.block(&[drop_call])
        };
        let wrapper = fun(
            &mut b,
            "wrapper",
            &[],
            FirType::Void,
            Some(wrapper_body),
            false,
        );

        let sound_slot = b.declare_var("fSound0", FirType::Sound, AccessType::Struct, None);
        let dsp_struct = b.block(&[sound_slot]);
        let globals = b.block(&[]);
        let decls = b.block(&[helper, build_ui, wrapper]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };

    assert_no_checker_errors(&src, module);

    let (dst, rewritten, stats) =
        inline_fir_module_once(&src, module, &FirInlineOptions::default())
            .expect("rewrite should succeed");
    assert_eq!(stats.callsites_seen, 1);
    assert_eq!(stats.callsites_inlined, 1);

    let dump = dump_fir(&dst, rewritten);
    assert!(dump.contains("AddSoundfile"), "{dump}");
    assert!(dump.contains("LoadSoundfileRate"), "{dump}");
    assert!(!dump.contains("FunCall { name: \"helper\""), "{dump}");
    assert_no_checker_errors(&dst, rewritten);
}

#[test]
fn inline_module_once_skips_non_canonical_return_shape() {
    let mut src = FirStore::new();
    let module = {
        let mut b = FirBuilder::new(&mut src);
        let ff = FirType::FaustFloat;
        let x = NamedType {
            name: "x".to_string(),
            typ: ff.clone(),
        };

        // Two top-level returns: valid enough for the checker, but intentionally
        // non-canonical for Milestone-4 extraction/splicing.
        let helper_body = {
            let lx0 = b.load_var("x", AccessType::FunArgs, ff.clone());
            let ret0 = b.ret(Some(lx0));
            let lx1 = b.load_var("x", AccessType::FunArgs, ff.clone());
            let ret1 = b.ret(Some(lx1));
            b.block(&[ret0, ret1])
        };
        let helper = fun(
            &mut b,
            "helper",
            std::slice::from_ref(&x),
            ff.clone(),
            Some(helper_body),
            true,
        );

        let wrapper_body = {
            let arg = b.float64(2.0);
            let call = b.fun_call("helper", &[arg], ff.clone());
            let ret = b.ret(Some(call));
            b.block(&[ret])
        };
        let wrapper = fun(
            &mut b,
            "wrapper",
            &[],
            ff.clone(),
            Some(wrapper_body),
            false,
        );

        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[helper, wrapper]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };

    assert_no_checker_errors(&src, module);

    let (dst, rewritten, stats) =
        inline_fir_module_once(&src, module, &FirInlineOptions::default())
            .expect("rewrite should succeed");
    assert_eq!(stats.callsites_seen, 1);
    assert_eq!(stats.callsites_inlined, 0);
    assert_eq!(stats.callsites_skipped_unsupported_shape, 1);

    let dump = dump_fir(&dst, rewritten);
    assert!(
        dump.contains("FunCall { name: \"helper\""),
        "non-canonical helper should remain as call:\n{dump}"
    );
    assert_no_checker_errors(&dst, rewritten);
}

#[test]
fn function_rewrite_order_is_callees_first_across_scc_dag() {
    let mut store = FirStore::new();
    let module = {
        let mut b = FirBuilder::new(&mut store);
        let ff = FirType::FaustFloat;
        let x = NamedType {
            name: "x".to_string(),
            typ: ff.clone(),
        };

        let leaf_body = {
            let lx = b.load_var("x", AccessType::FunArgs, ff.clone());
            let ret = b.ret(Some(lx));
            b.block(&[ret])
        };
        let leaf = fun(
            &mut b,
            "leaf",
            std::slice::from_ref(&x),
            ff.clone(),
            Some(leaf_body),
            true,
        );

        let helper_body = {
            let lx = b.load_var("x", AccessType::FunArgs, ff.clone());
            let call = b.fun_call("leaf", &[lx], ff.clone());
            let ret = b.ret(Some(call));
            b.block(&[ret])
        };
        let helper = fun(
            &mut b,
            "helper",
            std::slice::from_ref(&x),
            ff.clone(),
            Some(helper_body),
            true,
        );

        let wrapper_body = {
            let raw = b.float64(3.0);
            let arg = b.cast(ff.clone(), raw);
            let call = b.fun_call("helper", &[arg], ff.clone());
            let ret = b.ret(Some(call));
            b.block(&[ret])
        };
        let wrapper = fun(
            &mut b,
            "wrapper",
            &[],
            ff.clone(),
            Some(wrapper_body),
            false,
        );

        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[wrapper, helper, leaf]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };

    let analysis =
        analyze_fir_inliner(&store, module, &FirInlineOptions::default()).expect("analysis ok");
    let order = function_rewrite_order_by_scc(&analysis);

    let leaf_pos = order.iter().position(|n| n == "leaf").unwrap();
    let helper_pos = order.iter().position(|n| n == "helper").unwrap();
    let wrapper_pos = order.iter().position(|n| n == "wrapper").unwrap();
    assert!(
        leaf_pos < helper_pos && helper_pos < wrapper_pos,
        "{order:?}"
    );
}

#[test]
fn inline_module_fixpoint_inlines_call_chain_across_multiple_passes() {
    let mut src = FirStore::new();
    let module = {
        let mut b = FirBuilder::new(&mut src);
        let ff = FirType::FaustFloat;
        let x = NamedType {
            name: "x".to_string(),
            typ: ff.clone(),
        };

        let leaf_body = {
            let lx = b.load_var("x", AccessType::FunArgs, ff.clone());
            let ret = b.ret(Some(lx));
            b.block(&[ret])
        };
        let leaf = fun(
            &mut b,
            "leaf",
            std::slice::from_ref(&x),
            ff.clone(),
            Some(leaf_body),
            true,
        );

        let helper_body = {
            let lx = b.load_var("x", AccessType::FunArgs, ff.clone());
            let call = b.fun_call("leaf", &[lx], ff.clone());
            let ret = b.ret(Some(call));
            b.block(&[ret])
        };
        let helper = fun(
            &mut b,
            "helper",
            std::slice::from_ref(&x),
            ff.clone(),
            Some(helper_body),
            true,
        );

        let wrapper_body = {
            let raw = b.float64(9.0);
            let arg = b.cast(ff.clone(), raw);
            let call = b.fun_call("helper", &[arg], ff.clone());
            let ret = b.ret(Some(call));
            b.block(&[ret])
        };
        let wrapper = fun(
            &mut b,
            "wrapper",
            &[],
            ff.clone(),
            Some(wrapper_body),
            false,
        );

        let dsp_struct = b.block(&[]);
        let globals = b.block(&[]);
        let decls = b.block(&[wrapper, helper, leaf]);
        {
            let static_decls = b.block(&[]);
            b.module(0, 0, "mydsp", dsp_struct, globals, decls, static_decls)
        }
    };

    let (dst, rewritten, stats) =
        inline_fir_module(&src, module, &FirInlineOptions::default()).expect("fixpoint ok");
    assert!(
        stats.total_callsites_inlined >= 2,
        "expected at least chain-length worth of inlines, got {:?}",
        stats
    );
    assert!(
        stats.iterations >= 2,
        "expected at least one progress pass plus fixpoint pass"
    );
    assert_eq!(stats.stop_reason, FirInlineFixpointStopReason::Fixpoint);
    assert!(stats.passes_with_progress >= 2);

    let dump = dump_fir(&dst, rewritten);
    assert!(!dump.contains("FunCall { name: \"helper\""), "{dump}");
    assert!(!dump.contains("FunCall { name: \"leaf\""), "{dump}");
    assert_no_checker_errors(&dst, rewritten);
}
