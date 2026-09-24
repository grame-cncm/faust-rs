//! Private node constructors, list encoding, and tag interning.
//!
//! All items here are `pub(crate)`.  External callers use `BoxBuilder` for
//! construction and `match_box` / `BoxMatch` for inspection.

use tlib::TreeArena;

use crate::BoxId;
use crate::tags::*;

// ── Tag interning ────────────────────────────────────────────────────────────

/// Interns a tagged box node with deterministic child ordering.
///
/// Shared low-level constructor for all `node_*` helpers. Mirrors the C++
/// `tree(tag, ...)` construction idiom while using arena tag interning and
/// hash-consing (`TreeArena::intern`) for canonicalization.
pub(crate) fn intern_tag(arena: &mut TreeArena, tag: &str, children: &[BoxId]) -> BoxId {
    let tag_id = arena.intern_tag(tag);
    arena.intern(tlib::NodeKind::Tag(tag_id), children)
}

// ── List encoding ────────────────────────────────────────────────────────────

/// Builds a canonical 4-element Faust list payload (`cons(a, cons(b, ...))`).
///
/// Used for slider parameter encoding to preserve C++/Faust list shape exactly.
pub(crate) fn list4(arena: &mut TreeArena, a: BoxId, b: BoxId, c: BoxId, d: BoxId) -> BoxId {
    let nil = arena.nil();
    let l3 = arena.cons(d, nil);
    let l2 = arena.cons(c, l3);
    let l1 = arena.cons(b, l2);
    arena.cons(a, l1)
}

/// Decodes a canonical `list4` slider payload into `(cur, min, max, step)`.
///
/// Returns `None` when `params` is not the expected nested `Cons` shape.
pub(crate) fn slider_params4(
    arena: &TreeArena,
    params: BoxId,
) -> Option<(BoxId, BoxId, BoxId, BoxId)> {
    let node0 = arena.node(params)?;
    if !matches!(node0.kind, tlib::NodeKind::Cons) || node0.children.len() != 2 {
        return None;
    }
    let cur = node0.children.get(0)?;

    let node1 = arena.node(node0.children.get(1)?)?;
    if !matches!(node1.kind, tlib::NodeKind::Cons) || node1.children.len() != 2 {
        return None;
    }
    let min = node1.children.get(0)?;

    let node2 = arena.node(node1.children.get(1)?)?;
    if !matches!(node2.kind, tlib::NodeKind::Cons) || node2.children.len() != 2 {
        return None;
    }
    let max = node2.children.get(0)?;

    let node3 = arena.node(node2.children.get(1)?)?;
    if !matches!(node3.kind, tlib::NodeKind::Cons) || node3.children.len() != 2 {
        return None;
    }
    let step = node3.children.get(0)?;

    Some((cur, min, max, step))
}

// ── node_* constructors ──────────────────────────────────────────────────────

/// Equivalent to C++ `boxIdent(const char*)`.
#[must_use]
pub(crate) fn node_ident(arena: &mut TreeArena, name: &str) -> BoxId {
    let sym = arena.symbol(name);
    intern_tag(arena, BOX_IDENT_TAG, &[sym])
}

/// Equivalent to C++ `boxSlot`.
#[must_use]
pub(crate) fn node_slot(arena: &mut TreeArena, id: i32) -> BoxId {
    let raw = arena.int(i64::from(id));
    intern_tag(arena, BOX_SLOT_TAG, &[raw])
}

/// Equivalent to C++ `boxSymbolic`.
#[must_use]
pub(crate) fn node_symbolic(arena: &mut TreeArena, slot: BoxId, body: BoxId) -> BoxId {
    intern_tag(arena, BOX_SYMBOLIC_TAG, &[slot, body])
}

/// Equivalent to C++ `boxInt`.
#[must_use]
pub(crate) fn node_int(arena: &mut TreeArena, value: i32) -> BoxId {
    arena.int(i64::from(value))
}

/// Equivalent to C++ `boxReal`.
#[must_use]
pub(crate) fn node_real(arena: &mut TreeArena, value: f64) -> BoxId {
    arena.float(value)
}

/// Equivalent to C++ `boxWire`.
#[must_use]
pub(crate) fn node_wire(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_WIRE_TAG, &[])
}

/// Equivalent to C++ `boxCut`.
#[must_use]
pub(crate) fn node_cut(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_CUT_TAG, &[])
}

/// Equivalent to C++ `boxSeq`.
#[must_use]
pub(crate) fn node_seq(arena: &mut TreeArena, left: BoxId, right: BoxId) -> BoxId {
    intern_tag(arena, BOX_SEQ_TAG, &[left, right])
}

/// Equivalent to C++ `boxPar`.
#[must_use]
pub(crate) fn node_par(arena: &mut TreeArena, left: BoxId, right: BoxId) -> BoxId {
    intern_tag(arena, BOX_PAR_TAG, &[left, right])
}

/// Equivalent to C++ `boxRec`.
#[must_use]
pub(crate) fn node_rec(arena: &mut TreeArena, left: BoxId, right: BoxId) -> BoxId {
    intern_tag(arena, BOX_REC_TAG, &[left, right])
}

/// Equivalent to C++ `boxSplit`.
#[must_use]
pub(crate) fn node_split(arena: &mut TreeArena, left: BoxId, right: BoxId) -> BoxId {
    intern_tag(arena, BOX_SPLIT_TAG, &[left, right])
}

/// Equivalent to C++ `boxMerge`.
#[must_use]
pub(crate) fn node_merge(arena: &mut TreeArena, left: BoxId, right: BoxId) -> BoxId {
    intern_tag(arena, BOX_MERGE_TAG, &[left, right])
}

/// Equivalent to C++ `boxAppl`.
#[must_use]
pub(crate) fn node_appl(arena: &mut TreeArena, fun: BoxId, arglist: BoxId) -> BoxId {
    intern_tag(arena, BOX_APPL_TAG, &[fun, arglist])
}

/// Equivalent to C++ `boxAccess`.
#[must_use]
pub(crate) fn node_access(arena: &mut TreeArena, expr: BoxId, ident: BoxId) -> BoxId {
    intern_tag(arena, BOX_ACCESS_TAG, &[expr, ident])
}

/// Equivalent to C++ `boxAdd`.
#[must_use]
pub(crate) fn node_add(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ADD_TAG, &[])
}

/// Equivalent to C++ `boxSub`.
#[must_use]
pub(crate) fn node_sub(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_SUB_TAG, &[])
}

/// Equivalent to C++ `boxMul`.
#[must_use]
pub(crate) fn node_mul(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_MUL_TAG, &[])
}

/// Equivalent to C++ `boxDiv`.
#[must_use]
pub(crate) fn node_div(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_DIV_TAG, &[])
}

/// Equivalent to C++ `boxRem`.
#[must_use]
pub(crate) fn node_rem(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_REM_TAG, &[])
}

/// Equivalent to C++ `boxAND`.
#[must_use]
pub(crate) fn node_and(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_AND_TAG, &[])
}

/// Equivalent to C++ `boxOR`.
#[must_use]
pub(crate) fn node_or(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_OR_TAG, &[])
}

/// Equivalent to C++ `boxXOR`.
#[must_use]
pub(crate) fn node_xor(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_XOR_TAG, &[])
}

/// Equivalent to C++ `boxLeftShift`.
#[must_use]
pub(crate) fn node_lsh(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_LSH_TAG, &[])
}

/// Equivalent to C++ `boxARightShift`.
#[must_use]
pub(crate) fn node_rsh(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_RSH_TAG, &[])
}

/// Equivalent to C++ `boxLRightShift`.
#[must_use]
pub(crate) fn node_lrsh(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_LRSH_TAG, &[])
}

/// Equivalent to C++ `boxLT`.
#[must_use]
pub(crate) fn node_lt(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_LT_TAG, &[])
}

/// Equivalent to C++ `boxLE`.
#[must_use]
pub(crate) fn node_le(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_LE_TAG, &[])
}

/// Equivalent to C++ `boxGT`.
#[must_use]
pub(crate) fn node_gt(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_GT_TAG, &[])
}

/// Equivalent to C++ `boxGE`.
#[must_use]
pub(crate) fn node_ge(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_GE_TAG, &[])
}

/// Equivalent to C++ `boxEQ`.
#[must_use]
pub(crate) fn node_eq(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_EQ_TAG, &[])
}

/// Equivalent to C++ `boxNE`.
#[must_use]
pub(crate) fn node_ne(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_NE_TAG, &[])
}

/// Equivalent to C++ `boxPow`.
#[must_use]
pub(crate) fn node_pow(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_POW_TAG, &[])
}

/// Equivalent to C++ `gAcosPrim->box()`.
#[must_use]
pub(crate) fn node_acos(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ACOS_TAG, &[])
}

/// Equivalent to C++ `gAsinPrim->box()`.
#[must_use]
pub(crate) fn node_asin(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ASIN_TAG, &[])
}

/// Equivalent to C++ `gAtanPrim->box()`.
#[must_use]
pub(crate) fn node_atan(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ATAN_TAG, &[])
}

/// Equivalent to C++ `gAtan2Prim->box()`.
#[must_use]
pub(crate) fn node_atan2(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ATAN2_TAG, &[])
}

/// Equivalent to C++ `gCosPrim->box()`.
#[must_use]
pub(crate) fn node_cos(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_COS_TAG, &[])
}

/// Equivalent to C++ `gSinPrim->box()`.
#[must_use]
pub(crate) fn node_sin(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_SIN_TAG, &[])
}

/// Equivalent to C++ `gTanPrim->box()`.
#[must_use]
pub(crate) fn node_tan(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_TAN_TAG, &[])
}

/// Equivalent to C++ `gExpPrim->box()`.
#[must_use]
pub(crate) fn node_exp(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_EXP_TAG, &[])
}

/// Equivalent to C++ `gExp10Prim->box()`.
#[must_use]
pub(crate) fn node_exp10(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_EXP10_TAG, &[])
}

/// Equivalent to C++ `gLogPrim->box()`.
#[must_use]
pub(crate) fn node_log(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_LOG_TAG, &[])
}

/// Equivalent to C++ `gLog10Prim->box()`.
#[must_use]
pub(crate) fn node_log10(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_LOG10_TAG, &[])
}

/// Equivalent to C++ `gSqrtPrim->box()`.
#[must_use]
pub(crate) fn node_sqrt(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_SQRT_TAG, &[])
}

/// Equivalent to C++ `gAbsPrim->box()`.
#[must_use]
pub(crate) fn node_abs(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ABS_TAG, &[])
}

/// Equivalent to C++ `gFmodPrim->box()`.
#[must_use]
pub(crate) fn node_fmod(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_FMOD_TAG, &[])
}

/// Equivalent to C++ `gRemainderPrim->box()`.
#[must_use]
pub(crate) fn node_remainder(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_REMAINDER_TAG, &[])
}

/// Equivalent to C++ `gFloorPrim->box()`.
#[must_use]
pub(crate) fn node_floor(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_FLOOR_TAG, &[])
}

/// Equivalent to C++ `gCeilPrim->box()`.
#[must_use]
pub(crate) fn node_ceil(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_CEIL_TAG, &[])
}

/// Equivalent to C++ `gRintPrim->box()`.
#[must_use]
pub(crate) fn node_rint(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_RINT_TAG, &[])
}

/// Equivalent to C++ `gRoundPrim->box()`.
#[must_use]
pub(crate) fn node_round(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ROUND_TAG, &[])
}

/// Equivalent to C++ `boxDelay`.
#[must_use]
pub(crate) fn node_delay(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_DELAY_TAG, &[])
}

/// Equivalent to C++ `boxDelay1`.
#[must_use]
pub(crate) fn node_delay1(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_DELAY1_TAG, &[])
}

/// Equivalent to C++ `boxMin`.
#[must_use]
pub(crate) fn node_min(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_MIN_TAG, &[])
}

/// Equivalent to C++ `boxMax`.
#[must_use]
pub(crate) fn node_max(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_MAX_TAG, &[])
}

/// Equivalent to C++ `boxPrefix`.
#[must_use]
pub(crate) fn node_prefix(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_PREFIX_TAG, &[])
}

/// Equivalent to C++ `boxIntCast`.
#[must_use]
pub(crate) fn node_int_cast(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_INT_CAST_TAG, &[])
}

/// Equivalent to C++ `boxFloatCast`.
#[must_use]
pub(crate) fn node_float_cast(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_FLOAT_CAST_TAG, &[])
}

/// Equivalent to C++ `boxReadOnlyTable`.
#[must_use]
pub(crate) fn node_read_only_table(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_READ_ONLY_TABLE_TAG, &[])
}

/// Equivalent to C++ `boxWriteReadTable`.
#[must_use]
pub(crate) fn node_write_read_table(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_WRITE_READ_TABLE_TAG, &[])
}

/// Equivalent to C++ `boxSelect2`.
#[must_use]
pub(crate) fn node_select2(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_SELECT2_TAG, &[])
}

/// Equivalent to C++ `boxSelect3`.
#[must_use]
pub(crate) fn node_select3(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_SELECT3_TAG, &[])
}

/// Equivalent to C++ `boxAssertBound`.
#[must_use]
pub(crate) fn node_assert_bounds(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ASSERT_BOUNDS_TAG, &[])
}

/// Equivalent to C++ `boxLowest`.
#[must_use]
pub(crate) fn node_lowest(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_LOWEST_TAG, &[])
}

/// Equivalent to C++ `boxHighest`.
#[must_use]
pub(crate) fn node_highest(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_HIGHEST_TAG, &[])
}

/// Equivalent to C++ `boxAttach`.
#[must_use]
pub(crate) fn node_attach(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ATTACH_TAG, &[])
}

/// Equivalent to C++ `boxEnable`.
#[must_use]
pub(crate) fn node_enable(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ENABLE_TAG, &[])
}

/// Equivalent to C++ `boxControl`.
#[must_use]
pub(crate) fn node_control(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_CONTROL_TAG, &[])
}

/// Equivalent to C++ `boxIPar`.
#[must_use]
pub(crate) fn node_ipar(arena: &mut TreeArena, index: BoxId, count: BoxId, body: BoxId) -> BoxId {
    intern_tag(arena, BOX_IPAR_TAG, &[index, count, body])
}

/// Equivalent to C++ `boxISeq`.
#[must_use]
pub(crate) fn node_iseq(arena: &mut TreeArena, index: BoxId, count: BoxId, body: BoxId) -> BoxId {
    intern_tag(arena, BOX_ISEQ_TAG, &[index, count, body])
}

/// Equivalent to C++ `boxISum`.
#[must_use]
pub(crate) fn node_isum(arena: &mut TreeArena, index: BoxId, count: BoxId, body: BoxId) -> BoxId {
    intern_tag(arena, BOX_ISUM_TAG, &[index, count, body])
}

/// Equivalent to C++ `boxIProd`.
#[must_use]
pub(crate) fn node_iprod(arena: &mut TreeArena, index: BoxId, count: BoxId, body: BoxId) -> BoxId {
    intern_tag(arena, BOX_IPROD_TAG, &[index, count, body])
}

/// Equivalent to C++ `boxWithLocalDef`.
#[must_use]
pub(crate) fn node_with_local_def(arena: &mut TreeArena, body: BoxId, ldef: BoxId) -> BoxId {
    intern_tag(arena, BOX_WITH_LOCAL_DEF_TAG, &[body, ldef])
}

/// Equivalent to C++ `boxModifLocalDef`.
#[must_use]
pub(crate) fn node_modif_local_def(arena: &mut TreeArena, body: BoxId, ldef: BoxId) -> BoxId {
    intern_tag(arena, BOX_MODIF_LOCAL_DEF_TAG, &[body, ldef])
}

/// Equivalent to C++ `boxWithRecDef` — eagerly expands to `with_local_def` form.
///
/// See [`box_with_rec_def_expanded`] for the expansion algorithm.
#[must_use]
pub(crate) fn node_with_rec_def(
    arena: &mut TreeArena,
    body: BoxId,
    ldef: BoxId,
    ldef2: BoxId,
) -> BoxId {
    box_with_rec_def_expanded(arena, body, ldef, ldef2)
}

fn box_with_rec_def_expanded(
    arena: &mut TreeArena,
    body: BoxId,
    ldef: BoxId,
    ldef2: BoxId,
) -> BoxId {
    let names = def2names(arena, ldef);
    let exprs = def2exp(arena, ldef);
    let n = list_len(arena, ldef);
    let recursive_body_def = make_recursive_body_def(arena, n, names, exprs, ldef2);
    let projections = make_rec_projections_list(arena, n, 0, names, arena.nil());
    let defs = arena.cons(recursive_body_def, projections);
    node_with_local_def(arena, body, defs)
}

fn list_len(arena: &TreeArena, mut list: BoxId) -> usize {
    let mut n = 0usize;
    while !arena.is_nil(list) {
        n += 1;
        list = arena
            .tl(list)
            .expect("definition list should be a well-formed cons/nil list");
    }
    n
}

fn def2names(arena: &mut TreeArena, ldef: BoxId) -> BoxId {
    if arena.is_nil(ldef) {
        arena.nil()
    } else {
        let def = arena.hd(ldef).expect("definition list head");
        let name = arena.hd(def).expect("definition name");
        let rest = arena.tl(ldef).expect("definition list tail");
        let tail = def2names(arena, rest);
        arena.cons(name, tail)
    }
}

fn def2exp(arena: &mut TreeArena, ldef: BoxId) -> BoxId {
    if arena.is_nil(ldef) {
        arena.nil()
    } else {
        let def = arena.hd(ldef).expect("definition list head");
        let payload = arena.tl(def).expect("definition payload");
        let args = arena.hd(payload).expect("definition args");
        let body = arena.tl(payload).expect("definition body");
        let expr = if arena.is_nil(args) {
            body
        } else {
            build_box_abstr(arena, args, body)
        };
        let rest = arena.tl(ldef).expect("definition list tail");
        let tail = def2exp(arena, rest);
        arena.cons(expr, tail)
    }
}

fn make_bus(arena: &mut TreeArena, n: usize) -> BoxId {
    if n <= 1 {
        node_wire(arena)
    } else {
        let left = node_wire(arena);
        let right = make_bus(arena, n - 1);
        node_par(arena, left, right)
    }
}

fn make_par_list(arena: &mut TreeArena, lexp: BoxId) -> BoxId {
    let l2 = arena.tl(lexp).expect("expression list tail");
    if arena.is_nil(l2) {
        arena.hd(lexp).expect("expression list head")
    } else {
        let head = arena.hd(lexp).expect("expression list head");
        let tail = make_par_list(arena, l2);
        node_par(arena, head, tail)
    }
}

fn make_box_abstr(arena: &mut TreeArena, largs: BoxId, body: BoxId) -> BoxId {
    if arena.is_nil(largs) {
        body
    } else {
        let arg = arena.hd(largs).expect("abstraction arg");
        let tail = arena.tl(largs).expect("abstraction arg tail");
        let nested = make_box_abstr(arena, tail, body);
        node_abstr(arena, arg, nested)
    }
}

fn make_selector(arena: &mut TreeArena, n: usize, i: i32) -> BoxId {
    let op = if i == 0 {
        node_wire(arena)
    } else {
        node_cut(arena)
    };
    if n <= 1 {
        op
    } else {
        let tail = make_selector(arena, n - 1, i - 1);
        node_par(arena, op, tail)
    }
}

fn make_rec_projections_list(
    arena: &mut TreeArena,
    n: usize,
    i: usize,
    lnames: BoxId,
    ldef: BoxId,
) -> BoxId {
    if i == n {
        ldef
    } else {
        let letrecbody = node_ident(arena, "LETRECBODY");
        let selector = make_selector(arena, n, i as i32);
        let sel = node_seq(arena, letrecbody, selector);
        let name = arena.hd(lnames).expect("recursive projection name");
        let def = make_parser_definition(arena, name, arena.nil(), sel);
        let tail_names = arena.tl(lnames).expect("recursive projection name tail");
        let tail_defs = make_rec_projections_list(arena, n, i + 1, tail_names, ldef);
        arena.cons(def, tail_defs)
    }
}

fn make_recursive_body_def(
    arena: &mut TreeArena,
    n: usize,
    lnames: BoxId,
    lexp: BoxId,
    ldef2: BoxId,
) -> BoxId {
    let body = make_par_list(arena, lexp);
    let body = if arena.is_nil(ldef2) {
        body
    } else {
        node_with_local_def(arena, body, ldef2)
    };
    let abstr = make_box_abstr(arena, lnames, body);
    let bus = make_bus(arena, n);
    let rec = node_rec(arena, abstr, bus);
    let letrecbody = node_ident(arena, "LETRECBODY");
    let nil = arena.nil();
    make_parser_definition(arena, letrecbody, nil, rec)
}

fn make_parser_definition(arena: &mut TreeArena, name: BoxId, args: BoxId, expr: BoxId) -> BoxId {
    let payload = arena.cons(args, expr);
    arena.cons(name, payload)
}

/// Equivalent to C++ `boxMetadata`.
#[must_use]
pub(crate) fn node_metadata(arena: &mut TreeArena, expr: BoxId, mdlist: BoxId) -> BoxId {
    intern_tag(arena, BOX_METADATA_TAG, &[expr, mdlist])
}

/// Equivalent to C++ `boxEnvironment`.
#[must_use]
pub(crate) fn node_environment(arena: &mut TreeArena) -> BoxId {
    intern_tag(arena, BOX_ENVIRONMENT_TAG, &[])
}

/// Equivalent to C++ `boxComponent`.
#[must_use]
pub(crate) fn node_component(arena: &mut TreeArena, filename: BoxId) -> BoxId {
    intern_tag(arena, BOX_COMPONENT_TAG, &[filename])
}

/// Equivalent to C++ `boxLibrary`.
#[must_use]
pub(crate) fn node_library(arena: &mut TreeArena, filename: BoxId) -> BoxId {
    intern_tag(arena, BOX_LIBRARY_TAG, &[filename])
}

/// Equivalent to C++ `importFile`.
#[must_use]
pub(crate) fn node_import_file(arena: &mut TreeArena, filename: BoxId) -> BoxId {
    intern_tag(arena, IMPORT_FILE_TAG, &[filename])
}

/// Equivalent to C++ `boxWaveform`.
#[must_use]
pub(crate) fn node_waveform(arena: &mut TreeArena, values: &[BoxId]) -> BoxId {
    let mut list = arena.nil();
    for value in values.iter().rev() {
        list = arena.cons(*value, list);
    }
    intern_tag(arena, BOX_WAVEFORM_TAG, &[list])
}

/// Equivalent to C++ `boxRoute`.
#[must_use]
pub(crate) fn node_route(arena: &mut TreeArena, n: BoxId, m: BoxId, route_spec: BoxId) -> BoxId {
    intern_tag(arena, BOX_ROUTE_TAG, &[n, m, route_spec])
}

/// Equivalent to C++ `ffunction(signature, incfile, libfile)`.
#[must_use]
pub(crate) fn ffunction(
    arena: &mut TreeArena,
    signature: BoxId,
    incfile: BoxId,
    libfile: BoxId,
) -> BoxId {
    intern_tag(arena, FFUN_TAG, &[signature, incfile, libfile])
}

/// Equivalent to C++ `boxFFun`.
#[must_use]
pub(crate) fn node_ffun(arena: &mut TreeArena, ff: BoxId) -> BoxId {
    intern_tag(arena, BOX_FFUN_TAG, &[ff])
}

/// Equivalent to C++ `boxFConst`.
#[must_use]
pub(crate) fn node_fconst(arena: &mut TreeArena, ty: BoxId, name: BoxId, file: BoxId) -> BoxId {
    intern_tag(arena, BOX_FCONST_TAG, &[ty, name, file])
}

/// Equivalent to C++ `boxFVar`.
#[must_use]
pub(crate) fn node_fvar(arena: &mut TreeArena, ty: BoxId, name: BoxId, file: BoxId) -> BoxId {
    intern_tag(arena, BOX_FVAR_TAG, &[ty, name, file])
}

/// Equivalent to C++ `boxCase`.
#[must_use]
pub(crate) fn node_case(arena: &mut TreeArena, rules: BoxId) -> BoxId {
    intern_tag(arena, BOX_CASE_TAG, &[rules])
}

/// Builds a `boxPatternMatcher(key)` node referencing the evaluator PM store.
#[must_use]
pub(crate) fn node_pattern_matcher(arena: &mut TreeArena, key: BoxId) -> BoxId {
    intern_tag(arena, BOX_PATTERN_MATCHER_TAG, &[key])
}

/// Builds a `boxClosure(key)` node referencing the evaluator closure store.
#[must_use]
pub(crate) fn node_closure(arena: &mut TreeArena, key: BoxId) -> BoxId {
    intern_tag(arena, BOX_CLOSURE_TAG, &[key])
}

/// Equivalent to C++ `boxPatternVar`.
#[must_use]
pub(crate) fn node_pattern_var(arena: &mut TreeArena, ident: BoxId) -> BoxId {
    intern_tag(arena, BOX_PATTERN_VAR_TAG, &[ident])
}

/// Equivalent to C++ `boxAbstr`.
#[must_use]
pub(crate) fn node_abstr(arena: &mut TreeArena, arg: BoxId, body: BoxId) -> BoxId {
    intern_tag(arena, BOX_ABSTR_TAG, &[arg, body])
}

/// Equivalent to C++ `boxModulation`.
#[must_use]
pub(crate) fn node_modulation(arena: &mut TreeArena, arg: BoxId, body: BoxId) -> BoxId {
    intern_tag(arena, BOX_MODULATION_TAG, &[arg, body])
}

/// Equivalent to C++ `buildBoxAbstr(largs, body)` using parser-built arg list.
///
/// Preserves C++ nesting order by consuming list tail first.
#[must_use]
pub(crate) fn build_box_abstr(arena: &mut TreeArena, args: BoxId, body: BoxId) -> BoxId {
    if arena.is_nil(args) {
        return body;
    }
    let Some(head) = arena.hd(args) else {
        return body;
    };
    let Some(tail) = arena.tl(args) else {
        return body;
    };
    let nested = node_abstr(arena, head, body);
    build_box_abstr(arena, tail, nested)
}

/// Equivalent to C++ `buildBoxModulation(largs, body)` using parser-built arg list.
#[must_use]
pub(crate) fn build_box_modulation(arena: &mut TreeArena, args: BoxId, body: BoxId) -> BoxId {
    if arena.is_nil(args) {
        return body;
    }
    let Some(head) = arena.hd(args) else {
        return body;
    };
    let Some(tail) = arena.tl(args) else {
        return body;
    };
    let nested = node_modulation(arena, head, body);
    build_box_modulation(arena, tail, nested)
}

/// Equivalent to C++ `boxInputs`.
#[must_use]
pub(crate) fn node_inputs(arena: &mut TreeArena, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_INPUTS_TAG, &[expr])
}

/// Equivalent to C++ `boxOutputs`.
#[must_use]
pub(crate) fn node_outputs(arena: &mut TreeArena, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_OUTPUTS_TAG, &[expr])
}

/// Builds `cinputs(expr)`: the control inputs of `expr` as a list of widget
/// boxes. faust-rs extension, no C++ equivalent.
#[must_use]
pub(crate) fn node_cinputs(arena: &mut TreeArena, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_CINPUTS_TAG, &[expr])
}

/// Builds `cinput(index, expr)`: one control input of `expr` as the list
/// `(widget, init, min, max, step)`. faust-rs extension, no C++ equivalent.
#[must_use]
pub(crate) fn node_cinput(arena: &mut TreeArena, index: BoxId, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_CINPUT_TAG, &[index, expr])
}

/// Builds `coutputs(expr)`: the bargraphs of `expr` as a list of bargraph
/// boxes. faust-rs extension, no C++ equivalent.
#[must_use]
pub(crate) fn node_coutputs(arena: &mut TreeArena, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_COUTPUTS_TAG, &[expr])
}

/// Builds `coutput(index, expr)`: one bargraph of `expr` as the list
/// `(bargraph, min, max)`. faust-rs extension, no C++ equivalent.
#[must_use]
pub(crate) fn node_coutput(arena: &mut TreeArena, index: BoxId, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_COUTPUT_TAG, &[index, expr])
}

/// Equivalent to C++ `boxOndemand`.
#[must_use]
pub(crate) fn node_ondemand(arena: &mut TreeArena, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_ONDEMAND_TAG, &[expr])
}

/// Equivalent to C++ `boxUpsampling`.
#[must_use]
pub(crate) fn node_upsampling(arena: &mut TreeArena, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_UPSAMPLING_TAG, &[expr])
}

/// Equivalent to C++ `boxDownsampling`.
#[must_use]
pub(crate) fn node_downsampling(arena: &mut TreeArena, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_DOWNSAMPLING_TAG, &[expr])
}

/// Equivalent to C++ `boxForwardAD` (`fad(expr, seed)`).
#[must_use]
pub(crate) fn node_forward_ad(arena: &mut TreeArena, expr: BoxId, seed: BoxId) -> BoxId {
    intern_tag(arena, BOX_FORWARD_AD_TAG, &[expr, seed])
}

/// Equivalent to C++ `boxReverseAD` (`rad(expr, seeds)`).
#[must_use]
pub(crate) fn node_reverse_ad(arena: &mut TreeArena, expr: BoxId, seeds: BoxId) -> BoxId {
    intern_tag(arena, BOX_REVERSE_AD_TAG, &[expr, seeds])
}

/// Equivalent to C++ `boxButton`.
#[must_use]
pub(crate) fn node_button(arena: &mut TreeArena, label: BoxId) -> BoxId {
    intern_tag(arena, BOX_BUTTON_TAG, &[label])
}

/// Equivalent to C++ `boxCheckbox`.
#[must_use]
pub(crate) fn node_checkbox(arena: &mut TreeArena, label: BoxId) -> BoxId {
    intern_tag(arena, BOX_CHECKBOX_TAG, &[label])
}

/// Equivalent to C++ `boxVSlider`.
#[must_use]
pub(crate) fn node_vslider(
    arena: &mut TreeArena,
    label: BoxId,
    cur: BoxId,
    min: BoxId,
    max: BoxId,
    step: BoxId,
) -> BoxId {
    let params = list4(arena, cur, min, max, step);
    intern_tag(arena, BOX_VSLIDER_TAG, &[label, params])
}

/// Equivalent to C++ `boxHSlider`.
#[must_use]
pub(crate) fn node_hslider(
    arena: &mut TreeArena,
    label: BoxId,
    cur: BoxId,
    min: BoxId,
    max: BoxId,
    step: BoxId,
) -> BoxId {
    let params = list4(arena, cur, min, max, step);
    intern_tag(arena, BOX_HSLIDER_TAG, &[label, params])
}

/// Equivalent to C++ `boxNumEntry`.
#[must_use]
pub(crate) fn node_num_entry(
    arena: &mut TreeArena,
    label: BoxId,
    cur: BoxId,
    min: BoxId,
    max: BoxId,
    step: BoxId,
) -> BoxId {
    let params = list4(arena, cur, min, max, step);
    intern_tag(arena, BOX_NUM_ENTRY_TAG, &[label, params])
}

/// Equivalent to C++ `boxVGroup`.
#[must_use]
pub(crate) fn node_vgroup(arena: &mut TreeArena, label: BoxId, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_VGROUP_TAG, &[label, expr])
}

/// Equivalent to C++ `boxHGroup`.
#[must_use]
pub(crate) fn node_hgroup(arena: &mut TreeArena, label: BoxId, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_HGROUP_TAG, &[label, expr])
}

/// Equivalent to C++ `boxTGroup`.
#[must_use]
pub(crate) fn node_tgroup(arena: &mut TreeArena, label: BoxId, expr: BoxId) -> BoxId {
    intern_tag(arena, BOX_TGROUP_TAG, &[label, expr])
}

/// Equivalent to C++ `boxVBargraph`.
#[must_use]
pub(crate) fn node_vbargraph(arena: &mut TreeArena, label: BoxId, min: BoxId, max: BoxId) -> BoxId {
    intern_tag(arena, BOX_VBARGRAPH_TAG, &[label, min, max])
}

/// Equivalent to C++ `boxHBargraph`.
#[must_use]
pub(crate) fn node_hbargraph(arena: &mut TreeArena, label: BoxId, min: BoxId, max: BoxId) -> BoxId {
    intern_tag(arena, BOX_HBARGRAPH_TAG, &[label, min, max])
}

/// Equivalent to C++ `boxSoundfile`.
#[must_use]
pub(crate) fn node_soundfile(arena: &mut TreeArena, label: BoxId, chan: BoxId) -> BoxId {
    intern_tag(arena, BOX_SOUNDFILE_TAG, &[label, chan])
}
