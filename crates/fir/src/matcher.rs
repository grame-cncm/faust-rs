//! Canonical FIR structural matcher.
//!
//! `match_fir` decodes hash-consed tree nodes into typed Rust enums. Downstream
//! passes should pattern match on `FirMatch` instead of reaching into raw
//! `TreeArena` tags.

use super::*;

/// FIR structural matcher result.
#[derive(Clone, Debug, PartialEq)]
pub enum FirMatch {
    Unknown,
    Int32 {
        value: i32,
        typ: FirType,
    },
    Int64 {
        value: i64,
        typ: FirType,
    },
    Float32 {
        value: f32,
        typ: FirType,
    },
    Float64 {
        value: f64,
        typ: FirType,
    },
    Bool {
        value: bool,
        typ: FirType,
    },
    Quad {
        value: f64,
        typ: FirType,
    },
    FixedPoint {
        value: f64,
        typ: FirType,
    },
    ValueArray {
        values: Vec<FirId>,
        typ: FirType,
    },
    Int32Array {
        values: Vec<i32>,
        typ: FirType,
    },
    Float32Array {
        values: Vec<f32>,
        typ: FirType,
    },
    Float64Array {
        values: Vec<f64>,
        typ: FirType,
    },
    QuadArray {
        values: Vec<f64>,
        typ: FirType,
    },
    FixedPointArray {
        values: Vec<f64>,
        typ: FirType,
    },
    LoadVar {
        name: String,
        access: AccessType,
        typ: FirType,
    },
    LoadTable {
        name: String,
        access: AccessType,
        index: FirId,
        typ: FirType,
    },
    LoadVarAddress {
        name: String,
        access: AccessType,
        typ: FirType,
    },
    TeeVar {
        name: String,
        access: AccessType,
        value: FirId,
        typ: FirType,
    },
    BinOp {
        op: FirBinOp,
        lhs: FirId,
        rhs: FirId,
        typ: FirType,
    },
    Neg {
        value: FirId,
        typ: FirType,
    },
    Cast {
        typ: FirType,
        value: FirId,
    },
    Bitcast {
        typ: FirType,
        value: FirId,
    },
    Select2 {
        cond: FirId,
        then_value: FirId,
        else_value: FirId,
        typ: FirType,
    },
    FunCall {
        name: String,
        args: Vec<FirId>,
        typ: FirType,
    },
    NullValue {
        typ: FirType,
    },
    NewDsp {
        name: String,
        typ: FirType,
    },
    DeclareVar {
        name: String,
        typ: FirType,
        access: AccessType,
        init: Option<FirId>,
    },
    DeclareTable {
        name: String,
        access: AccessType,
        elem_type: FirType,
        values: Vec<FirId>,
    },
    DeclareFun {
        name: String,
        typ: FirType,
        args: Vec<NamedType>,
        /// `None` when this is a prototype-only declaration (no body).
        body: Option<FirId>,
        is_inline: bool,
    },
    DeclareStructType {
        typ: FirType,
    },
    DeclareBufferIterators {
        name1: String,
        name2: String,
        channels: i32,
        typ: FirType,
        mutable: bool,
        chunk: bool,
    },
    StoreVar {
        name: String,
        access: AccessType,
        value: FirId,
    },
    StoreTable {
        name: String,
        access: AccessType,
        index: FirId,
        value: FirId,
    },
    ShiftArrayVar {
        name: String,
        access: AccessType,
        delay: i32,
    },
    Drop(FirId),
    NullStatement,
    Return(Option<FirId>),
    Block(Vec<FirId>),
    If {
        cond: FirId,
        then_block: FirId,
        else_block: Option<FirId>,
    },
    Control {
        cond: FirId,
        stmt: FirId,
    },
    ForLoop {
        var: String,
        init: FirId,
        end: FirId,
        step: FirId,
        body: FirId,
        is_reverse: bool,
    },
    SimpleForLoop {
        var: String,
        upper: FirId,
        body: FirId,
        is_reverse: bool,
    },
    IteratorForLoop {
        iterators: Vec<String>,
        is_reverse: bool,
        body: FirId,
    },
    WhileLoop {
        cond: FirId,
        body: FirId,
    },
    Switch {
        cond: FirId,
        cases: Vec<(i64, FirId)>,
        default: Option<FirId>,
    },
    OpenBox {
        typ: UiBoxType,
        label: String,
    },
    CloseBox,
    AddButton {
        typ: ButtonType,
        label: String,
        var: String,
    },
    AddSlider {
        typ: SliderType,
        label: String,
        var: String,
        init: f64,
        lo: f64,
        hi: f64,
        step: f64,
    },
    AddBargraph {
        typ: BargraphType,
        label: String,
        var: String,
        lo: f64,
        hi: f64,
    },
    AddSoundfile {
        label: String,
        url: String,
        var: String,
    },
    /// C++ parity: `LoadSoundfileInst` / `fSoundN->fLength[part]`.
    LoadSoundfileLength {
        var: String,
        part: FirId,
    },
    /// C++ parity: `LoadSoundfileInst` / `fSoundN->fSR[part]`.
    LoadSoundfileRate {
        var: String,
        part: FirId,
    },
    /// C++ parity: `LoadSoundfileInst` / `((FAUSTFLOAT**)fSoundN->fBuffers)[chan][fSoundN->fOffset[part] + idx]`.
    LoadSoundfileBuffer {
        var: String,
        chan: FirId,
        part: FirId,
        idx: FirId,
        typ: FirType,
    },
    AddMetaDeclare {
        var: String,
        key: String,
        value: String,
    },
    Label(String),
    Module {
        num_inputs: usize,
        num_outputs: usize,
        name: String,
        dsp_struct: FirId,
        globals: FirId,
        functions: FirId,
        static_decls: FirId,
        /// `Block` of [`FirMatch::SubModule`] nodes owned by this module.
        ///
        /// Empty for every program without a generated table. A backend that
        /// has not been migrated to emit sub-modules must fail on a non-empty
        /// block rather than ignore it: skipping it would emit a table
        /// declaration that nothing ever fills.
        sub_modules: FirId,
    },
    /// A self-contained 0-input / 1-output program that fills one table at
    /// initialization time.
    ///
    /// C++ parity: the `CodeContainer` built by `signal2Container` for a
    /// `SIGGEN` table generator, exposing `instanceInit<name>(sample_rate)` and
    /// `fill<name>(count, table)`. Sub-modules nest: a generator that reads
    /// another generated table owns that table's sub-module in turn.
    SubModule {
        /// Class/struct name, e.g. `mydspSIG0`.
        name: String,
        /// Element type of the table this sub-module fills, and therefore of
        /// the `table` argument of its `fill` function.
        elem_type: FirType,
        /// `Block` of `DeclareVar` — the generator's own state.
        dsp_struct: FirId,
        /// `Block` of file-scope declarations the generator owns, e.g. the
        /// literal waveform table behind a waveform payload.
        static_decls: FirId,
        /// `Block` of prototypes/global declarations the generator needs.
        globals: FirId,
        /// `Block` holding exactly `instanceInit<name>` and `fill<name>`.
        functions: FirId,
        /// `Block` of nested [`FirMatch::SubModule`] nodes.
        sub_modules: FirId,
    },
}

/// Decodes one [`FirId`] into canonical [`FirMatch`] shape.
///
/// This is the only structural decoder other crates should need. Malformed or
/// partially built trees degrade to [`FirMatch::Unknown`] instead of panicking.
#[must_use]
pub fn match_fir(store: &FirStore, id: FirId) -> FirMatch {
    let Some(node) = store.arena.node(id) else {
        return FirMatch::Unknown;
    };
    let NodeKind::Tag(tag_id) = &node.kind else {
        return FirMatch::Unknown;
    };
    let Some(tag) = store.arena.tag_name(*tag_id) else {
        return FirMatch::Unknown;
    };
    let ch = node.children.as_slice();

    match (tag, ch) {
        (FIR_V_INT32_TAG, [typ, v]) => {
            let (Some(typ), Some(value)) = (
                decode_type(&store.arena, *typ),
                decode_i32(&store.arena, *v),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Int32 { value, typ }
        }
        (FIR_V_INT64_TAG, [typ, v]) => {
            let (Some(typ), Some(value)) = (
                decode_type(&store.arena, *typ),
                decode_i64(&store.arena, *v),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Int64 { value, typ }
        }
        (FIR_V_FLOAT32_TAG, [typ, bits]) => {
            let (Some(typ), Some(value)) = (
                decode_type(&store.arena, *typ),
                decode_f32_bits(&store.arena, *bits),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Float32 { value, typ }
        }
        (FIR_V_FLOAT64_TAG, [typ, v]) => {
            let (Some(typ), Some(value)) = (
                decode_type(&store.arena, *typ),
                decode_f64(&store.arena, *v),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Float64 { value, typ }
        }
        (FIR_V_BOOL_TAG, [typ, v]) => {
            let (Some(typ), Some(value)) = (
                decode_type(&store.arena, *typ),
                decode_bool(&store.arena, *v),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Bool { value, typ }
        }
        (FIR_V_QUAD_TAG, [typ, v]) => {
            let (Some(typ), Some(value)) = (
                decode_type(&store.arena, *typ),
                decode_f64(&store.arena, *v),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Quad { value, typ }
        }
        (FIR_V_FIXED_POINT_TAG, [typ, v]) => {
            let (Some(typ), Some(value)) = (
                decode_type(&store.arena, *typ),
                decode_f64(&store.arena, *v),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::FixedPoint { value, typ }
        }
        (FIR_V_VALUE_ARRAY_TAG, [typ, values]) => {
            let (Some(typ), Some(values)) = (
                decode_type(&store.arena, *typ),
                decode_list(&store.arena, *values),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::ValueArray { values, typ }
        }
        (FIR_V_INT32_ARRAY_TAG, [typ, values]) => {
            let (Some(typ), Some(values)) = (
                decode_type(&store.arena, *typ),
                decode_i32_list(&store.arena, *values),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Int32Array { values, typ }
        }
        (FIR_V_FLOAT32_ARRAY_TAG, [typ, values]) => {
            let (Some(typ), Some(values)) = (
                decode_type(&store.arena, *typ),
                decode_f32_bits_list(&store.arena, *values),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Float32Array { values, typ }
        }
        (FIR_V_FLOAT64_ARRAY_TAG, [typ, values]) => {
            let (Some(typ), Some(values)) = (
                decode_type(&store.arena, *typ),
                decode_f64_list(&store.arena, *values),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Float64Array { values, typ }
        }
        (FIR_V_QUAD_ARRAY_TAG, [typ, values]) => {
            let (Some(typ), Some(values)) = (
                decode_type(&store.arena, *typ),
                decode_f64_list(&store.arena, *values),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::QuadArray { values, typ }
        }
        (FIR_V_FIXED_POINT_ARRAY_TAG, [typ, values]) => {
            let (Some(typ), Some(values)) = (
                decode_type(&store.arena, *typ),
                decode_f64_list(&store.arena, *values),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::FixedPointArray { values, typ }
        }
        (FIR_V_LOAD_VAR_TAG, [typ, name, access]) => {
            let (Some(typ), Some(name), Some(access)) = (
                decode_type(&store.arena, *typ),
                decode_symbol(&store.arena, *name),
                decode_access(&store.arena, *access),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::LoadVar { name, access, typ }
        }
        (FIR_V_LOAD_TABLE_TAG, [typ, name, access, index]) => {
            let (Some(typ), Some(name), Some(access)) = (
                decode_type(&store.arena, *typ),
                decode_symbol(&store.arena, *name),
                decode_access(&store.arena, *access),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::LoadTable {
                name,
                access,
                index: *index,
                typ,
            }
        }
        (FIR_V_LOAD_VAR_ADDRESS_TAG, [typ, name, access]) => {
            let (Some(typ), Some(name), Some(access)) = (
                decode_type(&store.arena, *typ),
                decode_symbol(&store.arena, *name),
                decode_access(&store.arena, *access),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::LoadVarAddress { name, access, typ }
        }
        (FIR_V_TEE_VAR_TAG, [typ, name, access, value]) => {
            let (Some(typ), Some(name), Some(access)) = (
                decode_type(&store.arena, *typ),
                decode_symbol(&store.arena, *name),
                decode_access(&store.arena, *access),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::TeeVar {
                name,
                access,
                value: *value,
                typ,
            }
        }
        (FIR_V_BINOP_TAG, [typ, op, lhs, rhs]) => {
            let (Some(typ), Some(op)) = (
                decode_type(&store.arena, *typ),
                decode_binop(&store.arena, *op),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::BinOp {
                op,
                lhs: *lhs,
                rhs: *rhs,
                typ,
            }
        }
        (FIR_V_NEG_TAG, [typ, value]) => {
            let Some(typ) = decode_type(&store.arena, *typ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Neg { value: *value, typ }
        }
        (FIR_V_CAST_TAG, [typ, value]) => {
            let Some(typ) = decode_type(&store.arena, *typ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Cast { typ, value: *value }
        }
        (FIR_V_BITCAST_TAG, [typ, value]) => {
            let Some(typ) = decode_type(&store.arena, *typ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Bitcast { typ, value: *value }
        }
        (FIR_V_SELECT2_TAG, [typ, cond, then_value, else_value]) => {
            let Some(typ) = decode_type(&store.arena, *typ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Select2 {
                cond: *cond,
                then_value: *then_value,
                else_value: *else_value,
                typ,
            }
        }
        (FIR_V_FUNCALL_TAG, [typ, name, args]) => {
            let (Some(typ), Some(name), Some(args)) = (
                decode_type(&store.arena, *typ),
                decode_symbol(&store.arena, *name),
                decode_list(&store.arena, *args),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::FunCall { name, args, typ }
        }
        (FIR_V_NULL_TAG, [typ]) => {
            let Some(typ) = decode_type(&store.arena, *typ) else {
                return FirMatch::Unknown;
            };
            FirMatch::NullValue { typ }
        }
        (FIR_V_NEW_DSP_TAG, [typ, name]) => {
            let (Some(typ), Some(name)) = (
                decode_type(&store.arena, *typ),
                decode_symbol(&store.arena, *name),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::NewDsp { name, typ }
        }
        (FIR_DECLARE_VAR_TAG, [name, typ, access, init]) => {
            let (Some(name), Some(typ), Some(access)) = (
                decode_symbol(&store.arena, *name),
                decode_type(&store.arena, *typ),
                decode_access(&store.arena, *access),
            ) else {
                return FirMatch::Unknown;
            };
            let init = if store.arena.is_nil(*init) {
                None
            } else {
                Some(*init)
            };
            FirMatch::DeclareVar {
                name,
                typ,
                access,
                init,
            }
        }
        (FIR_DECLARE_TABLE_TAG, [name, access, typ, values]) => {
            let (Some(name), Some(access), Some(elem_type), Some(values)) = (
                decode_symbol(&store.arena, *name),
                decode_access(&store.arena, *access),
                decode_type(&store.arena, *typ),
                decode_list(&store.arena, *values),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::DeclareTable {
                name,
                access,
                elem_type,
                values,
            }
        }
        (FIR_DECLARE_FUN_TAG, [name, typ, args, body, is_inline]) => {
            let (Some(name), Some(typ), Some(args), Some(is_inline)) = (
                decode_symbol(&store.arena, *name),
                decode_type(&store.arena, *typ),
                decode_named_types(&store.arena, *args),
                decode_bool(&store.arena, *is_inline),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::DeclareFun {
                name,
                typ,
                args,
                body: Some(*body),
                is_inline,
            }
        }
        (FIR_DECLARE_FUN_PROTO_TAG, [name, typ, args, is_inline]) => {
            let (Some(name), Some(typ), Some(args), Some(is_inline)) = (
                decode_symbol(&store.arena, *name),
                decode_type(&store.arena, *typ),
                decode_named_types(&store.arena, *args),
                decode_bool(&store.arena, *is_inline),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::DeclareFun {
                name,
                typ,
                args,
                body: None,
                is_inline,
            }
        }
        (FIR_DECLARE_STRUCT_TYPE_TAG, [typ]) => {
            let Some(typ) = decode_type(&store.arena, *typ) else {
                return FirMatch::Unknown;
            };
            FirMatch::DeclareStructType { typ }
        }
        (FIR_DECLARE_BUFFER_ITERATORS_TAG, [name1, name2, channels, typ, mutable, chunk]) => {
            let (Some(name1), Some(name2), Some(channels), Some(typ), Some(mutable), Some(chunk)) = (
                decode_symbol(&store.arena, *name1),
                decode_symbol(&store.arena, *name2),
                decode_i32(&store.arena, *channels),
                decode_type(&store.arena, *typ),
                decode_bool(&store.arena, *mutable),
                decode_bool(&store.arena, *chunk),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::DeclareBufferIterators {
                name1,
                name2,
                channels,
                typ,
                mutable,
                chunk,
            }
        }
        (FIR_STORE_VAR_TAG, [name, access, value]) => {
            let (Some(name), Some(access)) = (
                decode_symbol(&store.arena, *name),
                decode_access(&store.arena, *access),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::StoreVar {
                name,
                access,
                value: *value,
            }
        }
        (FIR_STORE_TABLE_TAG, [name, access, index, value]) => {
            let (Some(name), Some(access)) = (
                decode_symbol(&store.arena, *name),
                decode_access(&store.arena, *access),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::StoreTable {
                name,
                access,
                index: *index,
                value: *value,
            }
        }
        (FIR_SHIFT_ARRAY_VAR_TAG, [name, access, delay]) => {
            let (Some(name), Some(access), Some(delay)) = (
                decode_symbol(&store.arena, *name),
                decode_access(&store.arena, *access),
                decode_i32(&store.arena, *delay),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::ShiftArrayVar {
                name,
                access,
                delay,
            }
        }
        (FIR_DROP_TAG, [value]) => FirMatch::Drop(*value),
        (FIR_NULL_STATEMENT_TAG, []) => FirMatch::NullStatement,
        (FIR_RETURN_TAG, [value]) => {
            let value = if store.arena.is_nil(*value) {
                None
            } else {
                Some(*value)
            };
            FirMatch::Return(value)
        }
        (FIR_BLOCK_TAG, [body]) => {
            let Some(body) = decode_list(&store.arena, *body) else {
                return FirMatch::Unknown;
            };
            FirMatch::Block(body)
        }
        (FIR_IF_TAG, [cond, then_block, else_block]) => {
            let else_block = if store.arena.is_nil(*else_block) {
                None
            } else {
                Some(*else_block)
            };
            FirMatch::If {
                cond: *cond,
                then_block: *then_block,
                else_block,
            }
        }
        (FIR_CONTROL_TAG, [cond, stmt]) => FirMatch::Control {
            cond: *cond,
            stmt: *stmt,
        },
        (FIR_FOR_LOOP_TAG, [var, init, end, step, body, is_reverse]) => {
            let (Some(var), Some(is_reverse)) = (
                decode_symbol(&store.arena, *var),
                decode_bool(&store.arena, *is_reverse),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::ForLoop {
                var,
                init: *init,
                end: *end,
                step: *step,
                body: *body,
                is_reverse,
            }
        }
        (FIR_SIMPLE_FOR_LOOP_TAG, [var, upper, body, is_reverse]) => {
            let (Some(var), Some(is_reverse)) = (
                decode_symbol(&store.arena, *var),
                decode_bool(&store.arena, *is_reverse),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::SimpleForLoop {
                var,
                upper: *upper,
                body: *body,
                is_reverse,
            }
        }
        (FIR_ITERATOR_FOR_LOOP_TAG, [iterators, is_reverse, body]) => {
            let (Some(iterators), Some(is_reverse)) = (
                decode_symbols_list(&store.arena, *iterators),
                decode_bool(&store.arena, *is_reverse),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::IteratorForLoop {
                iterators,
                is_reverse,
                body: *body,
            }
        }
        (FIR_WHILE_LOOP_TAG, [cond, body]) => FirMatch::WhileLoop {
            cond: *cond,
            body: *body,
        },
        (FIR_SWITCH_TAG, [cond, cases, default]) => {
            let Some(cases) = decode_switch_cases(&store.arena, *cases) else {
                return FirMatch::Unknown;
            };
            let default = if store.arena.is_nil(*default) {
                None
            } else {
                Some(*default)
            };
            FirMatch::Switch {
                cond: *cond,
                cases,
                default,
            }
        }
        (FIR_OPEN_BOX_TAG, [typ, label]) => {
            let (Some(typ), Some(label)) = (
                decode_ui_box_type(&store.arena, *typ),
                decode_symbol(&store.arena, *label),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::OpenBox { typ, label }
        }
        (FIR_CLOSE_BOX_TAG, []) => FirMatch::CloseBox,
        (FIR_ADD_BUTTON_TAG, [typ, label, var]) => {
            let (Some(typ), Some(label), Some(var)) = (
                decode_button_type(&store.arena, *typ),
                decode_symbol(&store.arena, *label),
                decode_symbol(&store.arena, *var),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::AddButton { typ, label, var }
        }
        (FIR_ADD_SLIDER_TAG, [typ, label, var, init, lo, hi, step]) => {
            let (Some(typ), Some(label), Some(var), Some(init), Some(lo), Some(hi), Some(step)) = (
                decode_slider_type(&store.arena, *typ),
                decode_symbol(&store.arena, *label),
                decode_symbol(&store.arena, *var),
                decode_f64(&store.arena, *init),
                decode_f64(&store.arena, *lo),
                decode_f64(&store.arena, *hi),
                decode_f64(&store.arena, *step),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::AddSlider {
                typ,
                label,
                var,
                init,
                lo,
                hi,
                step,
            }
        }
        (FIR_ADD_BARGRAPH_TAG, [typ, label, var, lo, hi]) => {
            let (Some(typ), Some(label), Some(var), Some(lo), Some(hi)) = (
                decode_bargraph_type(&store.arena, *typ),
                decode_symbol(&store.arena, *label),
                decode_symbol(&store.arena, *var),
                decode_f64(&store.arena, *lo),
                decode_f64(&store.arena, *hi),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::AddBargraph {
                typ,
                label,
                var,
                lo,
                hi,
            }
        }
        (FIR_ADD_SOUNDFILE_TAG, [label, url, var]) => {
            let (Some(label), Some(url), Some(var)) = (
                decode_symbol(&store.arena, *label),
                decode_symbol(&store.arena, *url),
                decode_symbol(&store.arena, *var),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::AddSoundfile { label, url, var }
        }
        // Compatibility with older rust snapshots where URL was not encoded.
        (FIR_ADD_SOUNDFILE_TAG, [label, var]) => {
            let (Some(label), Some(var)) = (
                decode_symbol(&store.arena, *label),
                decode_symbol(&store.arena, *var),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::AddSoundfile {
                label,
                url: String::new(),
                var,
            }
        }
        (FIR_V_LOAD_SOUNDFILE_LENGTH_TAG, [_typ, var, part]) => {
            let Some(var) = decode_symbol(&store.arena, *var) else {
                return FirMatch::Unknown;
            };
            FirMatch::LoadSoundfileLength { var, part: *part }
        }
        (FIR_V_LOAD_SOUNDFILE_RATE_TAG, [_typ, var, part]) => {
            let Some(var) = decode_symbol(&store.arena, *var) else {
                return FirMatch::Unknown;
            };
            FirMatch::LoadSoundfileRate { var, part: *part }
        }
        (FIR_V_LOAD_SOUNDFILE_BUFFER_TAG, [typ, var, chan, part, idx]) => {
            let (Some(typ), Some(var)) = (
                decode_type(&store.arena, *typ),
                decode_symbol(&store.arena, *var),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::LoadSoundfileBuffer {
                var,
                chan: *chan,
                part: *part,
                idx: *idx,
                typ,
            }
        }
        (FIR_ADD_META_DECLARE_TAG, [var, key, value]) => {
            let (Some(var), Some(key), Some(value)) = (
                decode_symbol(&store.arena, *var),
                decode_symbol(&store.arena, *key),
                decode_symbol(&store.arena, *value),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::AddMetaDeclare { var, key, value }
        }
        (FIR_LABEL_TAG, [label]) => {
            let Some(label) = decode_symbol(&store.arena, *label) else {
                return FirMatch::Unknown;
            };
            FirMatch::Label(label)
        }
        (
            FIR_SUB_MODULE_TAG,
            [
                name,
                elem_type,
                dsp_struct,
                static_decls,
                globals,
                functions,
                sub_modules,
            ],
        ) => {
            let Some(name) = decode_symbol(&store.arena, *name) else {
                return FirMatch::Unknown;
            };
            let Some(elem_type) = decode_type(&store.arena, *elem_type) else {
                return FirMatch::Unknown;
            };
            FirMatch::SubModule {
                name,
                elem_type,
                dsp_struct: *dsp_struct,
                static_decls: *static_decls,
                globals: *globals,
                functions: *functions,
                sub_modules: *sub_modules,
            }
        }
        (
            FIR_MODULE_TAG,
            [
                num_inputs,
                num_outputs,
                name,
                dsp_struct,
                globals,
                functions,
                static_decls,
                sub_modules,
            ],
        ) => {
            let Some(name) = decode_symbol(&store.arena, *name) else {
                return FirMatch::Unknown;
            };
            let Some(raw_num_inputs) = decode_i64(&store.arena, *num_inputs) else {
                return FirMatch::Unknown;
            };
            let Some(raw_num_outputs) = decode_i64(&store.arena, *num_outputs) else {
                return FirMatch::Unknown;
            };
            let (Ok(num_inputs), Ok(num_outputs)) = (
                usize::try_from(raw_num_inputs),
                usize::try_from(raw_num_outputs),
            ) else {
                return FirMatch::Unknown;
            };
            FirMatch::Module {
                num_inputs,
                num_outputs,
                name,
                dsp_struct: *dsp_struct,
                globals: *globals,
                functions: *functions,
                static_decls: *static_decls,
                sub_modules: *sub_modules,
            }
        }
        _ => FirMatch::Unknown,
    }
}

/// Lists every child [`FirId`] of one decoded FIR node.
///
/// This is the canonical **total** traversal primitive: the match is
/// deliberately exhaustive with no wildcard arm, so adding a new
/// [`FirMatch`] variant fails compilation here instead of letting a
/// traversal silently skip the new node kind (the failure mode that once
/// hid a transport from a vector body verifier). Callers that need a
/// narrower policy must still enumerate variants explicitly rather than
/// falling back to this function's complement.
#[must_use]
pub fn fir_match_children(store: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(store, id) {
        FirMatch::Unknown
        | FirMatch::Int32 { .. }
        | FirMatch::Int64 { .. }
        | FirMatch::Float32 { .. }
        | FirMatch::Float64 { .. }
        | FirMatch::Bool { .. }
        | FirMatch::Quad { .. }
        | FirMatch::FixedPoint { .. }
        | FirMatch::Int32Array { .. }
        | FirMatch::Float32Array { .. }
        | FirMatch::Float64Array { .. }
        | FirMatch::QuadArray { .. }
        | FirMatch::FixedPointArray { .. }
        | FirMatch::LoadVar { .. }
        | FirMatch::LoadVarAddress { .. }
        | FirMatch::NullValue { .. }
        | FirMatch::NewDsp { .. }
        | FirMatch::DeclareStructType { .. }
        | FirMatch::DeclareBufferIterators { .. }
        | FirMatch::ShiftArrayVar { .. }
        | FirMatch::NullStatement
        | FirMatch::OpenBox { .. }
        | FirMatch::CloseBox
        | FirMatch::AddButton { .. }
        | FirMatch::AddSlider { .. }
        | FirMatch::AddBargraph { .. }
        | FirMatch::AddSoundfile { .. }
        | FirMatch::AddMetaDeclare { .. }
        | FirMatch::Label(_) => Vec::new(),
        FirMatch::ValueArray { values, .. } => values,
        FirMatch::LoadTable { index, .. } => vec![index],
        FirMatch::TeeVar { value, .. } => vec![value],
        FirMatch::BinOp { lhs, rhs, .. } => vec![lhs, rhs],
        FirMatch::Neg { value, .. }
        | FirMatch::Cast { value, .. }
        | FirMatch::Bitcast { value, .. } => vec![value],
        FirMatch::Select2 {
            cond,
            then_value,
            else_value,
            ..
        } => vec![cond, then_value, else_value],
        FirMatch::FunCall { args, .. } => args,
        FirMatch::DeclareVar { init, .. } => init.into_iter().collect(),
        FirMatch::DeclareTable { values, .. } => values,
        FirMatch::DeclareFun { body, .. } => body.into_iter().collect(),
        FirMatch::StoreVar { value, .. } => vec![value],
        FirMatch::StoreTable { index, value, .. } => vec![index, value],
        FirMatch::Drop(value) => vec![value],
        FirMatch::Return(value) => value.into_iter().collect(),
        FirMatch::Block(items) => items,
        FirMatch::If {
            cond,
            then_block,
            else_block,
        } => {
            let mut out = vec![cond, then_block];
            out.extend(else_block);
            out
        }
        FirMatch::Control { cond, stmt } => vec![cond, stmt],
        FirMatch::ForLoop {
            init,
            end,
            step,
            body,
            ..
        } => vec![init, end, step, body],
        FirMatch::SimpleForLoop { upper, body, .. } => vec![upper, body],
        FirMatch::IteratorForLoop { body, .. } => vec![body],
        FirMatch::WhileLoop { cond, body } => vec![cond, body],
        FirMatch::Switch {
            cond,
            cases,
            default,
        } => {
            let mut out = vec![cond];
            out.extend(cases.iter().map(|(_, block)| *block));
            out.extend(default);
            out
        }
        FirMatch::LoadSoundfileLength { part, .. } | FirMatch::LoadSoundfileRate { part, .. } => {
            vec![part]
        }
        FirMatch::LoadSoundfileBuffer {
            chan, part, idx, ..
        } => vec![chan, part, idx],
        FirMatch::Module {
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
            ..
        } => vec![dsp_struct, globals, functions, static_decls, sub_modules],
        FirMatch::SubModule {
            dsp_struct,
            static_decls,
            globals,
            functions,
            sub_modules,
            ..
        } => vec![dsp_struct, static_decls, globals, functions, sub_modules],
    }
}
