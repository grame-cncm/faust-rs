//! Integration tests for `core_api`.
//!
//! Scope:
//! - Exercises public APIs and structural invariants for the targeted module.
//! - Guards regression/parity behavior on representative fixtures and corpus cases.

use boxes::{BoxBuilder, BoxMatch, dump_box, match_box};
use tlib::{TreeArena, TreeId};

fn list_nth(arena: &TreeArena, mut list: TreeId, mut n: usize) -> Option<TreeId> {
    loop {
        if arena.is_nil(list) {
            return None;
        }
        let head = arena.hd(list)?;
        let tail = arena.tl(list)?;
        if n == 0 {
            return Some(head);
        }
        n -= 1;
        list = tail;
    }
}

fn parser_definition(arena: &mut TreeArena, name: TreeId, expr: TreeId) -> TreeId {
    let nil = arena.nil();
    let payload = arena.cons(nil, expr);
    arena.cons(name, payload)
}

#[test]
fn builder_and_match_cover_core_shapes() {
    let mut arena = TreeArena::new();
    let (ident, int, real, wire, cut, seq, par, rec, split, merge, appl, access) = {
        let nil = arena.nil();
        let one = arena.int(1);
        let arg = arena.cons(one, nil);
        let mut b = BoxBuilder::new(&mut arena);
        let ident = b.ident("freq");
        let int = b.int(42);
        let real = b.real(0.5);
        let wire = b.wire();
        let cut = b.cut();
        let seq = b.seq(wire, int);
        let par = b.par(wire, int);
        let rec = b.rec(wire, int);
        let split = b.split(wire, int);
        let merge = b.merge(wire, int);
        let appl = b.appl(ident, arg);
        let bar = b.ident("bar");
        let access = b.access(ident, bar);
        (
            ident, int, real, wire, cut, seq, par, rec, split, merge, appl, access,
        )
    };

    assert_eq!(match_box(&arena, ident), BoxMatch::Ident("freq"));
    assert_eq!(match_box(&arena, int), BoxMatch::Int(42));
    assert_eq!(match_box(&arena, real), BoxMatch::Real(0.5));
    assert_eq!(match_box(&arena, wire), BoxMatch::Wire);
    assert_eq!(match_box(&arena, cut), BoxMatch::Cut);
    assert_eq!(match_box(&arena, seq), BoxMatch::Seq(wire, int));
    assert_eq!(match_box(&arena, par), BoxMatch::Par(wire, int));
    assert_eq!(match_box(&arena, rec), BoxMatch::Rec(wire, int));
    assert_eq!(match_box(&arena, split), BoxMatch::Split(wire, int));
    assert_eq!(match_box(&arena, merge), BoxMatch::Merge(wire, int));
    assert!(matches!(match_box(&arena, appl), BoxMatch::Appl(_, _)));
    assert!(matches!(match_box(&arena, access), BoxMatch::Access(_, _)));
}

#[test]
fn builder_matches_all_primitive_families() {
    let mut arena = TreeArena::new();
    let mut b = BoxBuilder::new(&mut arena);
    let prims = [
        b.add(),
        b.sub(),
        b.mul(),
        b.div(),
        b.rem(),
        b.and(),
        b.or(),
        b.xor(),
        b.lsh(),
        b.rsh(),
        b.lrsh(),
        b.lt(),
        b.le(),
        b.gt(),
        b.ge(),
        b.eq(),
        b.ne(),
        b.pow(),
        b.acos(),
        b.asin(),
        b.atan(),
        b.atan2(),
        b.cos(),
        b.sin(),
        b.tan(),
        b.exp(),
        b.exp10(),
        b.log(),
        b.log10(),
        b.sqrt(),
        b.abs(),
        b.fmod(),
        b.remainder(),
        b.floor(),
        b.ceil(),
        b.rint(),
        b.round(),
        b.delay(),
        b.delay1(),
        b.min(),
        b.max(),
        b.prefix(),
        b.int_cast(),
        b.float_cast(),
        b.read_only_table(),
        b.write_read_table(),
        b.select2(),
        b.select3(),
        b.assert_bounds(),
        b.lowest(),
        b.highest(),
        b.attach(),
        b.enable(),
        b.control(),
    ];
    for p in prims {
        assert!(
            !matches!(match_box(&arena, p), BoxMatch::Unknown),
            "primitive should decode"
        );
    }
}

#[test]
fn builder_matches_iterative_scope_and_module_families() {
    let mut arena = TreeArena::new();
    let idx = arena.int(0);
    let count = arena.int(4);
    let filename_component = arena.string_lit("m.lib");
    let filename_library = arena.string_lit("x.lib");
    let one = arena.int(1);
    let zero = arena.int(0);
    let half = arena.float(0.5);

    let (ipar, iseq, isum, iprod, local, recdef, env, comp, lib, wave, route) = {
        let wire = BoxBuilder::new(&mut arena).wire();
        let ipar = BoxBuilder::new(&mut arena).ipar(idx, count, wire);
        let iseq = BoxBuilder::new(&mut arena).iseq(idx, count, wire);
        let isum = BoxBuilder::new(&mut arena).isum(idx, count, wire);
        let iprod = BoxBuilder::new(&mut arena).iprod(idx, count, wire);
        let a_ident = BoxBuilder::new(&mut arena).ident("a");
        let defs = {
            let def = parser_definition(&mut arena, a_ident, one);
            arena.cons(def, arena.nil())
        };
        let b_ident = BoxBuilder::new(&mut arena).ident("b");
        let defs2 = {
            let def = parser_definition(&mut arena, b_ident, one);
            arena.cons(def, arena.nil())
        };
        let local = BoxBuilder::new(&mut arena).with_local_def(wire, defs);
        let recdef = BoxBuilder::new(&mut arena).with_rec_def(wire, defs, defs2);
        let env = BoxBuilder::new(&mut arena).environment();
        let comp = BoxBuilder::new(&mut arena).component(filename_component);
        let lib = BoxBuilder::new(&mut arena).library(filename_library);
        let wave = BoxBuilder::new(&mut arena).waveform(&[zero, one, half]);
        let route_spec = BoxBuilder::new(&mut arena).par(zero, zero);
        let route = BoxBuilder::new(&mut arena).route(one, one, route_spec);
        (
            ipar, iseq, isum, iprod, local, recdef, env, comp, lib, wave, route,
        )
    };

    assert!(matches!(match_box(&arena, ipar), BoxMatch::IPar(_, _, _)));
    assert!(matches!(match_box(&arena, iseq), BoxMatch::ISeq(_, _, _)));
    assert!(matches!(match_box(&arena, isum), BoxMatch::ISum(_, _, _)));
    assert!(matches!(match_box(&arena, iprod), BoxMatch::IProd(_, _, _)));
    assert!(matches!(
        match_box(&arena, local),
        BoxMatch::WithLocalDef(_, _)
    ));
    let BoxMatch::WithLocalDef(_, rec_defs) = match_box(&arena, recdef) else {
        panic!("with_rec_def should expand eagerly to WithLocalDef");
    };
    let letrecbody_def = arena.hd(rec_defs).expect("LETRECBODY def");
    let letrecbody_name = arena.hd(letrecbody_def).expect("LETRECBODY name");
    assert_eq!(
        match_box(&arena, letrecbody_name),
        BoxMatch::Ident("LETRECBODY")
    );
    assert_eq!(match_box(&arena, env), BoxMatch::Environment);
    assert!(matches!(match_box(&arena, comp), BoxMatch::Component(_)));
    assert!(matches!(match_box(&arena, lib), BoxMatch::Library(_)));
    assert!(matches!(match_box(&arena, wave), BoxMatch::Waveform(_)));
    assert!(matches!(match_box(&arena, route), BoxMatch::Route(_, _, _)));
}

#[test]
fn builder_matches_foreign_case_and_stream_wrappers() {
    let mut arena = TreeArena::new();
    let ty = arena.int(1);
    let incfile = arena.symbol("<math.h>");
    let libfile = arena.symbol("\"\"");
    let cname = arena.symbol("SR");
    let vname = arena.symbol("count");
    let zero = arena.int(0);
    let signature = {
        let nil = arena.nil();
        let n3 = arena.cons(cname, nil);
        let n2 = arena.cons(cname, n3);
        let n1 = arena.cons(cname, n2);
        let names = arena.cons(cname, n1);
        let payload = arena.cons(names, nil);
        arena.cons(ty, payload)
    };
    let case_rules = {
        let nil = arena.nil();
        let lhs = arena.cons(zero, nil);
        let rule = arena.cons(lhs, zero);
        arena.cons(rule, nil)
    };

    let (ff, wrapped, fconst, fvar, case_expr, patvar, wrappers) = {
        let mut b = BoxBuilder::new(&mut arena);
        let ff = b.ffunction(signature, incfile, libfile);
        let wrapped = b.ffun(ff);
        let fconst = b.fconst(zero, cname, incfile);
        let fvar = b.fvar(zero, vname, incfile);
        let ident = b.ident("x");
        let patvar = b.pattern_var(ident);
        let case_expr = b.case(case_rules);
        let wire = b.wire();
        let wrappers = [
            b.inputs(wire),
            b.outputs(wire),
            b.forward_ad(wire, wire),
            b.reverse_ad(wire, wire),
            b.ondemand(wire),
            b.upsampling(wire),
            b.downsampling(wire),
        ];
        (ff, wrapped, fconst, fvar, case_expr, patvar, wrappers)
    };

    assert!(matches!(
        match_box(&arena, ff),
        BoxMatch::Ffunction(_, _, _)
    ));
    assert!(matches!(match_box(&arena, wrapped), BoxMatch::FFun(_)));
    assert!(matches!(
        match_box(&arena, fconst),
        BoxMatch::FConst(_, _, _)
    ));
    assert!(matches!(match_box(&arena, fvar), BoxMatch::FVar(_, _, _)));
    assert!(matches!(match_box(&arena, case_expr), BoxMatch::Case(_)));
    assert!(matches!(match_box(&arena, patvar), BoxMatch::PatternVar(_)));
    for w in wrappers {
        assert!(
            matches!(
                match_box(&arena, w),
                BoxMatch::Inputs(_)
                    | BoxMatch::Outputs(_)
                    | BoxMatch::ForwardAD(_, _)
                    | BoxMatch::ReverseAD(_, _)
                    | BoxMatch::Ondemand(_)
                    | BoxMatch::Upsampling(_)
                    | BoxMatch::Downsampling(_)
            ),
            "wrapper should decode"
        );
    }
}

#[test]
fn builder_matches_ui_and_groups() {
    let mut arena = TreeArena::new();
    let label = arena.symbol("gain");
    let cur = arena.float(0.5);
    let min = arena.float(0.0);
    let max = arena.float(1.0);
    let step = arena.float(0.01);

    let (vslider, hslider, nentry, button, checkbox, vgroup, hgroup, tgroup, vbar, hbar, soundfile) = {
        let mut b = BoxBuilder::new(&mut arena);
        let wire = b.wire();
        let vslider = b.vslider(label, cur, min, max, step);
        let hslider = b.hslider(label, cur, min, max, step);
        let nentry = b.num_entry(label, cur, min, max, step);
        let button = b.button(label);
        let checkbox = b.checkbox(label);
        let vgroup = b.vgroup(label, wire);
        let hgroup = b.hgroup(label, wire);
        let tgroup = b.tgroup(label, wire);
        let vbar = b.vbargraph(label, min, max);
        let hbar = b.hbargraph(label, min, max);
        let soundfile = b.soundfile(label, min);
        (
            vslider, hslider, nentry, button, checkbox, vgroup, hgroup, tgroup, vbar, hbar,
            soundfile,
        )
    };

    assert!(matches!(
        match_box(&arena, vslider),
        BoxMatch::VSlider(_, _, _, _, _)
    ));
    assert!(matches!(
        match_box(&arena, hslider),
        BoxMatch::HSlider(_, _, _, _, _)
    ));
    assert!(matches!(
        match_box(&arena, nentry),
        BoxMatch::NumEntry(_, _, _, _, _)
    ));
    assert!(matches!(match_box(&arena, button), BoxMatch::Button(_)));
    assert!(matches!(match_box(&arena, checkbox), BoxMatch::Checkbox(_)));
    assert!(matches!(match_box(&arena, vgroup), BoxMatch::VGroup(_, _)));
    assert!(matches!(match_box(&arena, hgroup), BoxMatch::HGroup(_, _)));
    assert!(matches!(match_box(&arena, tgroup), BoxMatch::TGroup(_, _)));
    assert!(matches!(
        match_box(&arena, vbar),
        BoxMatch::VBargraph(_, _, _)
    ));
    assert!(matches!(
        match_box(&arena, hbar),
        BoxMatch::HBargraph(_, _, _)
    ));
    assert!(matches!(
        match_box(&arena, soundfile),
        BoxMatch::Soundfile(_, _)
    ));
}

#[test]
fn build_abstr_and_modulation_order_match_cpp_and_dump_is_deterministic() {
    let mut arena = TreeArena::new();
    let (abstr, modulation) = {
        let x = arena.symbol("x");
        let y = arena.symbol("y");
        let nil = arena.nil();
        // Parser-style reversed arglist for source `(x, y)`: [y, x].
        let tail = arena.cons(x, nil);
        let args = arena.cons(y, tail);
        let mut b = BoxBuilder::new(&mut arena);
        let body = b.wire();
        let abstr = b.build_abstr(args, body);
        let modulation = b.build_modulation(args, body);
        (abstr, modulation)
    };

    let dump_a = dump_box(&arena, abstr);
    let dump_b = dump_box(&arena, abstr);
    assert_eq!(dump_a, dump_b);
    let (outer_arg, outer_body) = match match_box(&arena, abstr) {
        BoxMatch::Abstr(arg, body) => (arg, body),
        other => panic!("expected outer abstraction, got {other:?}"),
    };
    assert_eq!(outer_arg, arena.symbol("x"));
    let (inner_arg, inner_body) = match match_box(&arena, outer_body) {
        BoxMatch::Abstr(arg, body) => (arg, body),
        other => panic!("expected inner abstraction, got {other:?}"),
    };
    assert_eq!(inner_arg, arena.symbol("y"));
    assert!(matches!(match_box(&arena, inner_body), BoxMatch::Wire));

    let (outer_mod_arg, outer_mod_body) = match match_box(&arena, modulation) {
        BoxMatch::Modulation(arg, body) => (arg, body),
        other => panic!("expected outer modulation, got {other:?}"),
    };
    assert_eq!(outer_mod_arg, arena.symbol("x"));
    let (inner_mod_arg, inner_mod_body) = match match_box(&arena, outer_mod_body) {
        BoxMatch::Modulation(arg, body) => (arg, body),
        other => panic!("expected inner modulation, got {other:?}"),
    };
    assert_eq!(inner_mod_arg, arena.symbol("y"));
    assert!(matches!(match_box(&arena, inner_mod_body), BoxMatch::Wire));
}

#[test]
fn waveform_payload_keeps_parse_order() {
    let mut arena = TreeArena::new();
    let one = arena.int(1);
    let two = arena.int(2);
    let three = arena.int(3);
    let waveform = {
        let mut b = BoxBuilder::new(&mut arena);
        b.waveform(&[one, two, three])
    };

    let list = match match_box(&arena, waveform) {
        BoxMatch::Waveform(values) => values,
        other => panic!("expected waveform, got {other:?}"),
    };
    assert_eq!(list_nth(&arena, list, 0), Some(one));
    assert_eq!(list_nth(&arena, list, 1), Some(two));
    assert_eq!(list_nth(&arena, list, 2), Some(three));
}

#[test]
fn builder_matches_and_prints_control_list_primitives() {
    // faust-rs extensions `cinputs(e)`, `cinput(i, e)`, `coutputs(e)`,
    // `coutput(i, e)`: built, decoded to their own variants with their
    // operands in order, and printed back in source syntax.
    let mut arena = TreeArena::new();
    let (wire, index, nodes) = {
        let mut b = BoxBuilder::new(&mut arena);
        let wire = b.wire();
        let index = b.int(2);
        let nodes = [
            b.cinputs(wire),
            b.cinput(index, wire),
            b.coutputs(wire),
            b.coutput(index, wire),
        ];
        (wire, index, nodes)
    };
    assert_eq!(match_box(&arena, nodes[0]), BoxMatch::CInputs(wire));
    assert_eq!(match_box(&arena, nodes[1]), BoxMatch::CInput(index, wire));
    assert_eq!(match_box(&arena, nodes[2]), BoxMatch::COutputs(wire));
    assert_eq!(match_box(&arena, nodes[3]), BoxMatch::COutput(index, wire));
    let printed: Vec<String> = nodes
        .iter()
        .map(|&node| boxes::box_pp(&arena, node, 0, boxes::FloatSize::Single).expect("printable"))
        .collect();
    assert_eq!(
        printed,
        ["cinputs(_)", "cinput(2, _)", "coutputs(_)", "coutput(2, _)"]
    );
}
