//! Internal FIR tree encoding constants and helpers.
//!
//! This module is the single place that maps semantic FIR concepts to raw
//! `TreeArena` tags. Keeping it private prevents backend code from depending on
//! tag spelling instead of the builder/matcher APIs.

use super::*;

pub(crate) const FIR_TYPE_INT32_TAG: &str = "FIRTYPE_INT32";
pub(crate) const FIR_TYPE_INT64_TAG: &str = "FIRTYPE_INT64";
pub(crate) const FIR_TYPE_FLOAT32_TAG: &str = "FIRTYPE_FLOAT32";
pub(crate) const FIR_TYPE_FLOAT64_TAG: &str = "FIRTYPE_FLOAT64";
pub(crate) const FIR_TYPE_FAUSTFLOAT_TAG: &str = "FIRTYPE_FAUSTFLOAT";
pub(crate) const FIR_TYPE_QUAD_TAG: &str = "FIRTYPE_QUAD";
pub(crate) const FIR_TYPE_FIXED_POINT_TAG: &str = "FIRTYPE_FIXEDPOINT";
pub(crate) const FIR_TYPE_BOOL_TAG: &str = "FIRTYPE_BOOL";
pub(crate) const FIR_TYPE_VOID_TAG: &str = "FIRTYPE_VOID";
pub(crate) const FIR_TYPE_OBJ_TAG: &str = "FIRTYPE_OBJ";
pub(crate) const FIR_TYPE_SOUND_TAG: &str = "FIRTYPE_SOUND";
pub(crate) const FIR_TYPE_UI_TAG: &str = "FIRTYPE_UI";
pub(crate) const FIR_TYPE_META_TAG: &str = "FIRTYPE_META";
pub(crate) const FIR_TYPE_PTR_TAG: &str = "FIRTYPE_PTR";
pub(crate) const FIR_TYPE_ARRAY_TAG: &str = "FIRTYPE_ARRAY";
pub(crate) const FIR_TYPE_VECTOR_TAG: &str = "FIRTYPE_VECTOR";
pub(crate) const FIR_TYPE_STRUCT_TAG: &str = "FIRTYPE_STRUCT";
pub(crate) const FIR_TYPE_FUN_TAG: &str = "FIRTYPE_FUN";

pub(crate) const FIR_V_INT32_TAG: &str = "FIRV_INT32";
pub(crate) const FIR_V_INT64_TAG: &str = "FIRV_INT64";
pub(crate) const FIR_V_FLOAT32_TAG: &str = "FIRV_FLOAT32";
pub(crate) const FIR_V_FLOAT64_TAG: &str = "FIRV_FLOAT64";
pub(crate) const FIR_V_BOOL_TAG: &str = "FIRV_BOOL";
pub(crate) const FIR_V_QUAD_TAG: &str = "FIRV_QUAD";
pub(crate) const FIR_V_FIXED_POINT_TAG: &str = "FIRV_FIXEDPOINT";
pub(crate) const FIR_V_VALUE_ARRAY_TAG: &str = "FIRV_VALUEARRAY";
pub(crate) const FIR_V_INT32_ARRAY_TAG: &str = "FIRV_INT32ARRAY";
pub(crate) const FIR_V_FLOAT32_ARRAY_TAG: &str = "FIRV_FLOAT32ARRAY";
pub(crate) const FIR_V_FLOAT64_ARRAY_TAG: &str = "FIRV_FLOAT64ARRAY";
pub(crate) const FIR_V_QUAD_ARRAY_TAG: &str = "FIRV_QUADARRAY";
pub(crate) const FIR_V_FIXED_POINT_ARRAY_TAG: &str = "FIRV_FIXEDPOINTARRAY";
pub(crate) const FIR_V_LOAD_VAR_TAG: &str = "FIRV_LOADVAR";
pub(crate) const FIR_V_LOAD_TABLE_TAG: &str = "FIRV_LOADTABLE";
pub(crate) const FIR_V_LOAD_VAR_ADDRESS_TAG: &str = "FIRV_LOADVARADDRESS";
pub(crate) const FIR_V_TEE_VAR_TAG: &str = "FIRV_TEEVAR";
pub(crate) const FIR_V_BINOP_TAG: &str = "FIRV_BINOP";
pub(crate) const FIR_V_NEG_TAG: &str = "FIRV_NEG";
pub(crate) const FIR_V_CAST_TAG: &str = "FIRV_CAST";
pub(crate) const FIR_V_BITCAST_TAG: &str = "FIRV_BITCAST";
pub(crate) const FIR_V_SELECT2_TAG: &str = "FIRV_SELECT2";
pub(crate) const FIR_V_FUNCALL_TAG: &str = "FIRV_FUNCALL";
pub(crate) const FIR_V_NULL_TAG: &str = "FIRV_NULL";
pub(crate) const FIR_V_NEW_DSP_TAG: &str = "FIRV_NEWDSP";

pub(crate) const FIR_DECLARE_VAR_TAG: &str = "FIRST_DECLAREVAR";
pub(crate) const FIR_DECLARE_TABLE_TAG: &str = "FIRST_DECLARETABLE";
pub(crate) const FIR_DECLARE_FUN_TAG: &str = "FIRST_DECLAREFUN";
pub(crate) const FIR_DECLARE_FUN_PROTO_TAG: &str = "FIRST_DECLAREFUN_PROTO";
pub(crate) const FIR_DECLARE_STRUCT_TYPE_TAG: &str = "FIRST_DECLARESTRUCTTYPE";
pub(crate) const FIR_DECLARE_BUFFER_ITERATORS_TAG: &str = "FIRST_DECLAREBUFFERITERATORS";
pub(crate) const FIR_STORE_VAR_TAG: &str = "FIRST_STOREVAR";
pub(crate) const FIR_STORE_TABLE_TAG: &str = "FIRST_STORETABLE";
pub(crate) const FIR_SHIFT_ARRAY_VAR_TAG: &str = "FIRST_SHIFTARRAYVAR";
pub(crate) const FIR_DROP_TAG: &str = "FIRST_DROP";
pub(crate) const FIR_NULL_STATEMENT_TAG: &str = "FIRST_NULLSTATEMENT";
pub(crate) const FIR_RETURN_TAG: &str = "FIRST_RETURN";
pub(crate) const FIR_BLOCK_TAG: &str = "FIRST_BLOCK";
pub(crate) const FIR_IF_TAG: &str = "FIRST_IF";
pub(crate) const FIR_CONTROL_TAG: &str = "FIRST_CONTROL";
pub(crate) const FIR_FOR_LOOP_TAG: &str = "FIRST_FORLOOP";
pub(crate) const FIR_SIMPLE_FOR_LOOP_TAG: &str = "FIRST_SIMPLEFOR";
pub(crate) const FIR_ITERATOR_FOR_LOOP_TAG: &str = "FIRST_ITERATORFOR";
pub(crate) const FIR_WHILE_LOOP_TAG: &str = "FIRST_WHILELOOP";
pub(crate) const FIR_SWITCH_TAG: &str = "FIRST_SWITCH";
pub(crate) const FIR_OPEN_BOX_TAG: &str = "FIRST_OPENBOX";
pub(crate) const FIR_CLOSE_BOX_TAG: &str = "FIRST_CLOSEBOX";
pub(crate) const FIR_ADD_BUTTON_TAG: &str = "FIRST_ADDBUTTON";
pub(crate) const FIR_ADD_SLIDER_TAG: &str = "FIRST_ADDSLIDER";
pub(crate) const FIR_ADD_BARGRAPH_TAG: &str = "FIRST_ADDBARGRAPH";
pub(crate) const FIR_ADD_SOUNDFILE_TAG: &str = "FIRST_ADDSOUNDFILE";
pub(crate) const FIR_V_LOAD_SOUNDFILE_LENGTH_TAG: &str = "FIRST_LOADSOUNDFILELEN";
pub(crate) const FIR_V_LOAD_SOUNDFILE_RATE_TAG: &str = "FIRST_LOADSOUNDFILERATE";
pub(crate) const FIR_V_LOAD_SOUNDFILE_BUFFER_TAG: &str = "FIRST_LOADSOUNDFILEBUF";
pub(crate) const FIR_ADD_META_DECLARE_TAG: &str = "FIRST_ADDMETA";
pub(crate) const FIR_LABEL_TAG: &str = "FIRST_LABEL";
pub(crate) const FIR_MODULE_TAG: &str = "FIRST_MODULE";
pub(crate) const FIR_SUB_MODULE_TAG: &str = "FIRST_SUBMODULE";
pub(crate) const FIR_NAMED_TYPE_TAG: &str = "FIR_NAMEDTYPE";
pub(crate) const FIR_SWITCH_CASE_TAG: &str = "FIR_SWITCHCASE";

/// Returns `true` when `tag` names a FIR value-producing node.
///
/// [`FirStore::value_type`] relies on this whitelist to decide whether the
/// first encoded child stores a result type.
pub(crate) fn is_value_tag(tag: &str) -> bool {
    matches!(
        tag,
        FIR_V_INT32_TAG
            | FIR_V_INT64_TAG
            | FIR_V_FLOAT32_TAG
            | FIR_V_FLOAT64_TAG
            | FIR_V_BOOL_TAG
            | FIR_V_QUAD_TAG
            | FIR_V_FIXED_POINT_TAG
            | FIR_V_VALUE_ARRAY_TAG
            | FIR_V_INT32_ARRAY_TAG
            | FIR_V_FLOAT32_ARRAY_TAG
            | FIR_V_FLOAT64_ARRAY_TAG
            | FIR_V_QUAD_ARRAY_TAG
            | FIR_V_FIXED_POINT_ARRAY_TAG
            | FIR_V_LOAD_VAR_TAG
            | FIR_V_LOAD_TABLE_TAG
            | FIR_V_LOAD_VAR_ADDRESS_TAG
            | FIR_V_TEE_VAR_TAG
            | FIR_V_BINOP_TAG
            | FIR_V_NEG_TAG
            | FIR_V_CAST_TAG
            | FIR_V_BITCAST_TAG
            | FIR_V_SELECT2_TAG
            | FIR_V_FUNCALL_TAG
            | FIR_V_NULL_TAG
            | FIR_V_NEW_DSP_TAG
            | FIR_V_LOAD_SOUNDFILE_LENGTH_TAG
            | FIR_V_LOAD_SOUNDFILE_RATE_TAG
            | FIR_V_LOAD_SOUNDFILE_BUFFER_TAG
    )
}

/// Interns one tag node in the underlying [`TreeArena`].
///
/// This is the one place where FIR tag spelling meets TreeArena hash-consing.
/// All builder-side encoders route through it so identical tag/child shapes are
/// structurally shared automatically.
pub(crate) fn intern_tag(arena: &mut TreeArena, tag: &str, children: &[FirId]) -> FirId {
    let tag_id = arena.intern_tag(tag);
    arena.intern(NodeKind::Tag(tag_id), children)
}

/// Encodes an ordered FIR id slice as a canonical `cons`/`nil` list.
///
/// FIR keeps list structure explicit in the TreeArena representation so it can
/// round-trip through hash-consing without side tables.
pub(crate) fn encode_list(arena: &mut TreeArena, values: &[FirId]) -> FirId {
    let mut out = arena.nil();
    for value in values.iter().rev() {
        out = arena.cons(*value, out);
    }
    out
}

/// Decodes a canonical `cons`/`nil` list back into a flat FIR id vector.
///
/// Returns `None` if `list` is not a well-formed canonical list.
pub(crate) fn decode_list(arena: &TreeArena, mut list: FirId) -> Option<Vec<FirId>> {
    let mut out = Vec::new();
    while !arena.is_nil(list) {
        let head = arena.hd(list)?;
        out.push(head);
        list = arena.tl(list)?;
    }
    Some(out)
}

/// Decodes a FIR list whose payload nodes must all be `i32` literals.
pub(crate) fn decode_i32_list(arena: &TreeArena, list: FirId) -> Option<Vec<i32>> {
    let ids = decode_list(arena, list)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(decode_i32(arena, id)?);
    }
    Some(out)
}

/// Decodes a FIR list whose payload nodes store `f32` values as bit patterns.
pub(crate) fn decode_f32_bits_list(arena: &TreeArena, list: FirId) -> Option<Vec<f32>> {
    let ids = decode_list(arena, list)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(decode_f32_bits(arena, id)?);
    }
    Some(out)
}

/// Decodes a FIR list whose payload nodes must all be `f64`-compatible scalars.
pub(crate) fn decode_f64_list(arena: &TreeArena, list: FirId) -> Option<Vec<f64>> {
    let ids = decode_list(arena, list)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(decode_f64(arena, id)?);
    }
    Some(out)
}

/// Decodes a FIR list whose payload nodes must all be symbols/string literals.
pub(crate) fn decode_symbols_list(arena: &TreeArena, list: FirId) -> Option<Vec<String>> {
    let ids = decode_list(arena, list)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(decode_symbol(arena, id)?);
    }
    Some(out)
}

/// Encodes one `(name, type)` pair for function signatures and similar payloads.
pub(crate) fn encode_named_type(arena: &mut TreeArena, value: &NamedType) -> FirId {
    let name_id = arena.symbol(value.name.clone());
    let type_id = encode_type(arena, &value.typ);
    intern_tag(arena, FIR_NAMED_TYPE_TAG, &[name_id, type_id])
}

/// Encodes a stable ordered list of [`NamedType`] values.
pub(crate) fn encode_named_types(arena: &mut TreeArena, values: &[NamedType]) -> FirId {
    let ids: Vec<_> = values.iter().map(|v| encode_named_type(arena, v)).collect();
    encode_list(arena, &ids)
}

/// Decodes one encoded [`NamedType`] pair.
pub(crate) fn decode_named_type(arena: &TreeArena, id: FirId) -> Option<NamedType> {
    let node = arena.node(id)?;
    let NodeKind::Tag(tag_id) = &node.kind else {
        return None;
    };
    let tag = arena.tag_name(*tag_id)?;
    let [name, typ] = node.children.as_slice() else {
        return None;
    };
    if tag != FIR_NAMED_TYPE_TAG {
        return None;
    }
    Some(NamedType {
        name: decode_symbol(arena, *name)?,
        typ: decode_type(arena, *typ)?,
    })
}

/// Decodes a canonical list of encoded [`NamedType`] nodes.
pub(crate) fn decode_named_types(arena: &TreeArena, list: FirId) -> Option<Vec<NamedType>> {
    let ids = decode_list(arena, list)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(decode_named_type(arena, id)?);
    }
    Some(out)
}

/// Encodes one `switch` case pair `(constant_value, block_id)`.
pub(crate) fn encode_switch_case(arena: &mut TreeArena, value: i64, block: FirId) -> FirId {
    let value_id = arena.int(value);
    intern_tag(arena, FIR_SWITCH_CASE_TAG, &[value_id, block])
}

/// Encodes all `switch` cases as a canonical ordered list.
pub(crate) fn encode_switch_cases(arena: &mut TreeArena, cases: &[(i64, FirId)]) -> FirId {
    let ids: Vec<_> = cases
        .iter()
        .map(|(value, block)| encode_switch_case(arena, *value, *block))
        .collect();
    encode_list(arena, &ids)
}

/// Decodes one encoded `switch` case node.
pub(crate) fn decode_switch_case(arena: &TreeArena, id: FirId) -> Option<(i64, FirId)> {
    let node = arena.node(id)?;
    let NodeKind::Tag(tag_id) = &node.kind else {
        return None;
    };
    let tag = arena.tag_name(*tag_id)?;
    let [value, block] = node.children.as_slice() else {
        return None;
    };
    if tag != FIR_SWITCH_CASE_TAG {
        return None;
    }
    Some((decode_i64(arena, *value)?, *block))
}

/// Decodes a canonical ordered list of encoded `switch` cases.
pub(crate) fn decode_switch_cases(arena: &TreeArena, list: FirId) -> Option<Vec<(i64, FirId)>> {
    let ids = decode_list(arena, list)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(decode_switch_case(arena, id)?);
    }
    Some(out)
}

/// Encodes the explicit FIR type model into its canonical tree representation.
///
/// The representation is intentionally self-describing and recursive so value
/// nodes can carry types inline with no auxiliary type table.
pub(crate) fn encode_type(arena: &mut TreeArena, typ: &FirType) -> FirId {
    match typ {
        FirType::Int32 => intern_tag(arena, FIR_TYPE_INT32_TAG, &[]),
        FirType::Int64 => intern_tag(arena, FIR_TYPE_INT64_TAG, &[]),
        FirType::Float32 => intern_tag(arena, FIR_TYPE_FLOAT32_TAG, &[]),
        FirType::Float64 => intern_tag(arena, FIR_TYPE_FLOAT64_TAG, &[]),
        FirType::FaustFloat => intern_tag(arena, FIR_TYPE_FAUSTFLOAT_TAG, &[]),
        FirType::Quad => intern_tag(arena, FIR_TYPE_QUAD_TAG, &[]),
        FirType::FixedPoint => intern_tag(arena, FIR_TYPE_FIXED_POINT_TAG, &[]),
        FirType::Bool => intern_tag(arena, FIR_TYPE_BOOL_TAG, &[]),
        FirType::Void => intern_tag(arena, FIR_TYPE_VOID_TAG, &[]),
        FirType::Obj => intern_tag(arena, FIR_TYPE_OBJ_TAG, &[]),
        FirType::Sound => intern_tag(arena, FIR_TYPE_SOUND_TAG, &[]),
        FirType::UI => intern_tag(arena, FIR_TYPE_UI_TAG, &[]),
        FirType::Meta => intern_tag(arena, FIR_TYPE_META_TAG, &[]),
        FirType::Ptr(inner) => {
            let inner_id = encode_type(arena, inner);
            intern_tag(arena, FIR_TYPE_PTR_TAG, &[inner_id])
        }
        FirType::Array(inner, size) => {
            let inner_id = encode_type(arena, inner);
            let size_id = arena.int(i64::try_from(*size).unwrap_or(i64::MAX));
            intern_tag(arena, FIR_TYPE_ARRAY_TAG, &[inner_id, size_id])
        }
        FirType::Vector(inner, lanes) => {
            let inner_id = encode_type(arena, inner);
            let lanes_id = arena.int(i64::try_from(*lanes).unwrap_or(i64::MAX));
            intern_tag(arena, FIR_TYPE_VECTOR_TAG, &[inner_id, lanes_id])
        }
        FirType::Struct(name, fields) => {
            let name_id = arena.symbol(name.clone());
            let field_ids: Vec<_> = fields.iter().map(|f| encode_type(arena, f)).collect();
            let fields_list = encode_list(arena, &field_ids);
            intern_tag(arena, FIR_TYPE_STRUCT_TAG, &[name_id, fields_list])
        }
        FirType::Fun { args, ret } => {
            let args_ids: Vec<_> = args.iter().map(|a| encode_type(arena, a)).collect();
            let args_list = encode_list(arena, &args_ids);
            let ret_id = encode_type(arena, ret);
            intern_tag(arena, FIR_TYPE_FUN_TAG, &[args_list, ret_id])
        }
    }
}

/// Decodes a canonical tree-encoded FIR type.
pub(crate) fn decode_type(arena: &TreeArena, id: FirId) -> Option<FirType> {
    let node = arena.node(id)?;
    let NodeKind::Tag(tag_id) = &node.kind else {
        return None;
    };
    let tag = arena.tag_name(*tag_id)?;
    let ch = node.children.as_slice();
    match (tag, ch) {
        (FIR_TYPE_INT32_TAG, []) => Some(FirType::Int32),
        (FIR_TYPE_INT64_TAG, []) => Some(FirType::Int64),
        (FIR_TYPE_FLOAT32_TAG, []) => Some(FirType::Float32),
        (FIR_TYPE_FLOAT64_TAG, []) => Some(FirType::Float64),
        (FIR_TYPE_FAUSTFLOAT_TAG, []) => Some(FirType::FaustFloat),
        (FIR_TYPE_QUAD_TAG, []) => Some(FirType::Quad),
        (FIR_TYPE_FIXED_POINT_TAG, []) => Some(FirType::FixedPoint),
        (FIR_TYPE_BOOL_TAG, []) => Some(FirType::Bool),
        (FIR_TYPE_VOID_TAG, []) => Some(FirType::Void),
        (FIR_TYPE_OBJ_TAG, []) => Some(FirType::Obj),
        (FIR_TYPE_SOUND_TAG, []) => Some(FirType::Sound),
        (FIR_TYPE_UI_TAG, []) => Some(FirType::UI),
        (FIR_TYPE_META_TAG, []) => Some(FirType::Meta),
        (FIR_TYPE_PTR_TAG, [inner]) => Some(FirType::Ptr(Box::new(decode_type(arena, *inner)?))),
        (FIR_TYPE_ARRAY_TAG, [inner, size]) => {
            let size = usize::try_from(decode_i64(arena, *size)?).ok()?;
            Some(FirType::Array(Box::new(decode_type(arena, *inner)?), size))
        }
        (FIR_TYPE_VECTOR_TAG, [inner, lanes]) => {
            let lanes = usize::try_from(decode_i64(arena, *lanes)?).ok()?;
            Some(FirType::Vector(
                Box::new(decode_type(arena, *inner)?),
                lanes,
            ))
        }
        (FIR_TYPE_STRUCT_TAG, [name, fields]) => {
            let name = decode_symbol(arena, *name)?;
            let field_ids = decode_list(arena, *fields)?;
            let mut decoded_fields = Vec::with_capacity(field_ids.len());
            for fid in field_ids {
                decoded_fields.push(decode_type(arena, fid)?);
            }
            Some(FirType::Struct(name, decoded_fields))
        }
        (FIR_TYPE_FUN_TAG, [args, ret]) => {
            let args_ids = decode_list(arena, *args)?;
            let mut out = Vec::with_capacity(args_ids.len());
            for arg in args_ids {
                out.push(decode_type(arena, arg)?);
            }
            let ret = decode_type(arena, *ret)?;
            Some(FirType::Fun {
                args: out,
                ret: Box::new(ret),
            })
        }
        _ => None,
    }
}

/// Encodes one [`AccessType`] as its stable small integer code.
///
/// The numeric mapping is an internal representation contract and must remain
/// synchronized with [`decode_access`].
pub(crate) fn encode_access(arena: &mut TreeArena, access: AccessType) -> FirId {
    arena.int(match access {
        AccessType::Stack => 0,
        AccessType::Struct => 1,
        AccessType::Static => 2,
        AccessType::FunArgs => 3,
        AccessType::Loop => 4,
        AccessType::Global => 5,
    })
}

/// Decodes one small integer access-code back into [`AccessType`].
pub(crate) fn decode_access(arena: &TreeArena, id: FirId) -> Option<AccessType> {
    match decode_i64(arena, id)? {
        0 => Some(AccessType::Stack),
        1 => Some(AccessType::Struct),
        2 => Some(AccessType::Static),
        3 => Some(AccessType::FunArgs),
        4 => Some(AccessType::Loop),
        5 => Some(AccessType::Global),
        _ => None,
    }
}

/// Encodes one [`FirBinOp`] as its stable small integer code.
pub(crate) fn encode_binop(arena: &mut TreeArena, op: FirBinOp) -> FirId {
    arena.int(match op {
        FirBinOp::Add => 0,
        FirBinOp::Sub => 1,
        FirBinOp::Mul => 2,
        FirBinOp::Div => 3,
        FirBinOp::Rem => 4,
        FirBinOp::And => 5,
        FirBinOp::Or => 6,
        FirBinOp::Xor => 7,
        FirBinOp::Lsh => 8,
        FirBinOp::ARsh => 9,
        FirBinOp::LRsh => 10,
        FirBinOp::Eq => 11,
        FirBinOp::Ne => 12,
        FirBinOp::Lt => 13,
        FirBinOp::Le => 14,
        FirBinOp::Gt => 15,
        FirBinOp::Ge => 16,
    })
}

/// Decodes one small integer opcode back into [`FirBinOp`].
pub(crate) fn decode_binop(arena: &TreeArena, id: FirId) -> Option<FirBinOp> {
    match decode_i64(arena, id)? {
        0 => Some(FirBinOp::Add),
        1 => Some(FirBinOp::Sub),
        2 => Some(FirBinOp::Mul),
        3 => Some(FirBinOp::Div),
        4 => Some(FirBinOp::Rem),
        5 => Some(FirBinOp::And),
        6 => Some(FirBinOp::Or),
        7 => Some(FirBinOp::Xor),
        8 => Some(FirBinOp::Lsh),
        9 => Some(FirBinOp::ARsh),
        10 => Some(FirBinOp::LRsh),
        11 => Some(FirBinOp::Eq),
        12 => Some(FirBinOp::Ne),
        13 => Some(FirBinOp::Lt),
        14 => Some(FirBinOp::Le),
        15 => Some(FirBinOp::Gt),
        16 => Some(FirBinOp::Ge),
        _ => None,
    }
}

/// Encodes one UI container orientation as a compact integer atom.
pub(crate) fn encode_ui_box_type(arena: &mut TreeArena, typ: UiBoxType) -> FirId {
    arena.int(match typ {
        UiBoxType::Vertical => 0,
        UiBoxType::Horizontal => 1,
        UiBoxType::Tab => 2,
    })
}

/// Decodes one encoded UI container orientation.
pub(crate) fn decode_ui_box_type(arena: &TreeArena, id: FirId) -> Option<UiBoxType> {
    match decode_i64(arena, id)? {
        0 => Some(UiBoxType::Vertical),
        1 => Some(UiBoxType::Horizontal),
        2 => Some(UiBoxType::Tab),
        _ => None,
    }
}

/// Encodes one UI button kind as a compact integer atom.
pub(crate) fn encode_button_type(arena: &mut TreeArena, typ: ButtonType) -> FirId {
    arena.int(match typ {
        ButtonType::Button => 0,
        ButtonType::Checkbox => 1,
    })
}

/// Decodes one encoded UI button kind.
pub(crate) fn decode_button_type(arena: &TreeArena, id: FirId) -> Option<ButtonType> {
    match decode_i64(arena, id)? {
        0 => Some(ButtonType::Button),
        1 => Some(ButtonType::Checkbox),
        _ => None,
    }
}

/// Encodes one UI slider kind as a compact integer atom.
pub(crate) fn encode_slider_type(arena: &mut TreeArena, typ: SliderType) -> FirId {
    arena.int(match typ {
        SliderType::Horizontal => 0,
        SliderType::Vertical => 1,
        SliderType::NumEntry => 2,
    })
}

/// Decodes one encoded UI slider kind.
pub(crate) fn decode_slider_type(arena: &TreeArena, id: FirId) -> Option<SliderType> {
    match decode_i64(arena, id)? {
        0 => Some(SliderType::Horizontal),
        1 => Some(SliderType::Vertical),
        2 => Some(SliderType::NumEntry),
        _ => None,
    }
}

/// Encodes one UI bargraph kind as a compact integer atom.
pub(crate) fn encode_bargraph_type(arena: &mut TreeArena, typ: BargraphType) -> FirId {
    arena.int(match typ {
        BargraphType::Horizontal => 0,
        BargraphType::Vertical => 1,
    })
}

/// Decodes one encoded UI bargraph kind.
pub(crate) fn decode_bargraph_type(arena: &TreeArena, id: FirId) -> Option<BargraphType> {
    match decode_i64(arena, id)? {
        0 => Some(BargraphType::Horizontal),
        1 => Some(BargraphType::Vertical),
        _ => None,
    }
}

/// Decodes a symbol-bearing atom.
///
/// FIR accepts both interned symbols and string literals here because some UI
/// payloads are stored as string literals in the TreeArena.
pub(crate) fn decode_symbol(arena: &TreeArena, id: FirId) -> Option<String> {
    match arena.kind(id)? {
        NodeKind::Symbol(s) => Some(s.to_string()),
        NodeKind::StringLiteral(s) => Some(s.to_string()),
        _ => None,
    }
}

/// Decodes one integer atom as `i64`.
pub(crate) fn decode_i64(arena: &TreeArena, id: FirId) -> Option<i64> {
    tree_to_int(arena, id)
}

/// Decodes one integer atom as `i32`, failing on out-of-range values.
pub(crate) fn decode_i32(arena: &TreeArena, id: FirId) -> Option<i32> {
    i32::try_from(decode_i64(arena, id)?).ok()
}

/// Decodes one integer atom as the raw IEEE-754 bits of an `f32`.
pub(crate) fn decode_f32_bits(arena: &TreeArena, id: FirId) -> Option<f32> {
    let bits = u32::try_from(decode_i64(arena, id)?).ok()?;
    Some(f32::from_bits(bits))
}

/// Decodes one numeric atom as `f64`.
///
/// The fallback from integer to float preserves the permissive literal handling
/// historically used by the C++ FIR printers/builders.
pub(crate) fn decode_f64(arena: &TreeArena, id: FirId) -> Option<f64> {
    tree_to_double(arena, id).or_else(|| tree_to_int(arena, id).map(|v| v as f64))
}

/// Decodes one integer atom as a canonical boolean (`0`/`1` only).
pub(crate) fn decode_bool(arena: &TreeArena, id: FirId) -> Option<bool> {
    match decode_i64(arena, id)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}
